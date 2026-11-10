//! Changement du dossier des modèles de la base locale (`engine_settings.
//! models_dir`, lot L6 point 3 ; patron `ollama/models_dir/builtin.rs`) :
//! fichiers de chaque modèle installé déplacés (renommage sur le même
//! système de fichiers, sinon copie vérifiée puis effacement des anciens),
//! réglage écrit, tout remis en place après un échec. Les chemins de
//! `local_models` sont relatifs au dossier : la table ne change pas.
//!
//! Le dossier visé est noté avant le premier fichier déplacé
//! (`engine_settings.models_dir_moving`) et effacé avec le nouveau réglage :
//! après une coupure, [`recover`] remet dans l'ancien dossier ce qui était
//! déjà passé dans le nouveau. Un fichier du dossier choisi n'est jamais
//! effacé (refus). Annulation : [`cancel`].
//!
//! Le verrou [`is_busy`] est aussi celui de la file du ModelHub
//! (`hub::downloads`) : un téléchargement ne commence ni ne reprend pendant
//! un déplacement, et l'appelant de [`change`] vérifie, verrou pris, qu'aucun
//! téléchargement ni aucune reprise n'est en cours (`idle`). Moteurs arrêtés
//! par l'appelant (sous Windows, un fichier ouvert ne se déplace pas :
//! l'échec remet tout en place).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::Serialize;
use ts_rs::TS;

use super::transfer::{self, Mode, Placed, Step};
use super::{PROGRESS_INTERVAL, SPACE_MARGIN, same_filesystem};
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::Store;
use crate::store::local_models as table;
use crate::store::model_import as records;

/// Un changement de dossier est en cours (un seul à la fois ; pas de reprise
/// ni de téléchargement pendant ce temps).
#[cfg(not(test))]
static BUSY: AtomicBool = AtomicBool::new(false);

// Essais : un verrou par fil (les essais tournent en parallèle, chacun sur
// son fil ; un changement de dossier d'un essai ne refuse pas les
// téléchargements d'un autre).
#[cfg(test)]
thread_local! {
    static BUSY: AtomicBool = const { AtomicBool::new(false) };
}

fn busy_flag<R>(f: impl FnOnce(&AtomicBool) -> R) -> R {
    #[cfg(not(test))]
    {
        f(&BUSY)
    }
    #[cfg(test)]
    {
        BUSY.with(f)
    }
}

/// Annulation du changement en cours.
static CANCEL: parking_lot::Mutex<Option<Arc<AtomicBool>>> = parking_lot::Mutex::new(None);

/// Tant qu'il vit, le changement de dossier est en cours.
pub(crate) struct Busy;

impl Drop for Busy {
    fn drop(&mut self) {
        busy_flag(|busy| busy.store(false, Ordering::SeqCst));
    }
}

pub(crate) fn try_busy() -> Option<Busy> {
    busy_flag(|busy| {
        busy.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Busy)
    })
}

/// Un changement de dossier est en cours.
pub fn is_busy() -> bool {
    busy_flag(|busy| busy.load(Ordering::SeqCst))
}

/// Demande l'arrêt du changement de dossier en cours (tout est remis dans
/// l'ancien dossier).
pub fn cancel() {
    if let Some(cancel) = CANCEL.lock().as_ref() {
        cancel.store(true, Ordering::Relaxed);
    }
}

/// Garde l'annulation du changement en cours, effacée à la fin.
struct Cancellable;

impl Cancellable {
    fn new(cancel: &Arc<AtomicBool>) -> Self {
        *CANCEL.lock() = Some(cancel.clone());
        Self
    }
}

impl Drop for Cancellable {
    fn drop(&mut self) {
        *CANCEL.lock() = None;
    }
}

/// Dossier des modèles, pour la page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EngineModelsDir {
    pub path: String,
    /// Dossier par défaut (`<données>/models`).
    pub is_default: bool,
    pub free_bytes: Option<u64>,
    /// Modèles installés et leurs octets.
    pub models: u32,
    pub bytes: u64,
}

/// Avancement d'un changement de dossier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum FolderEvent {
    /// Octets à copier (zéro : renommages seulement).
    Total {
        bytes: u64,
    },
    Progress {
        done: u64,
    },
}

/// Bilan d'un changement réussi.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FolderMoved {
    pub path: String,
    pub renamed: u32,
    pub copied: u32,
    /// Des copies n'ont pas pu être effacées de l'ancien dossier.
    pub cleanup_failed: bool,
}

/// Dossier des modèles et ce qu'il contient.
pub async fn current(store: &Store, data_dir: &Path) -> AppResult<EngineModelsDir> {
    let data_dir = data_dir.to_owned();
    store
        .call(move |connection| {
            let path = crate::local_models::models_dir_in(connection, &data_dir)?;
            let models = table::list(connection)?;
            Ok(EngineModelsDir {
                is_default: same_path(&path, &crate::local_models::default_models_dir(&data_dir)),
                free_bytes: crate::disk::free_space_for(&path),
                models: u32::try_from(models.len()).unwrap_or(u32::MAX),
                bytes: models.iter().map(|model| model.size_bytes).sum(),
                path: path.to_string_lossy().into_owned(),
            })
        })
        .await
}

/// Même dossier (liens résolus quand les deux existent ; sous Windows, sans
/// la casse).
fn same_path(a: &Path, b: &Path) -> bool {
    let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let (a, b) = (canonical(a), canonical(b));
    if cfg!(windows) {
        crate::ollama::models_dir::rules::same(&a.to_string_lossy(), &b.to_string_lossy())
    } else {
        a == b
    }
}

/// `path` est sous `root` (liens résolus pour la partie existante).
fn inside(path: &Path, root: &Path) -> bool {
    let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if cfg!(windows) {
        crate::ollama::models_dir::rules::within(
            &canonical(path).to_string_lossy(),
            &canonical(root).to_string_lossy(),
        )
    } else {
        canonical(path).starts_with(canonical(root))
    }
}

/// Le dossier `dir` accepte un fichier (créé au besoin).
fn writable(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".chatonlm-write-test");
    let ok = fs::write(&probe, b"").is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

/// Change le dossier des modèles pour `new` (chemin absolu) ; `move_models`
/// faux : les modèles restent où ils sont (la base les cherchera dans le
/// nouveau dossier : à n'employer que pour un dossier qui les a déjà).
/// `idle` est appelé une fois le verrou pris : refus si un téléchargement
/// ou une reprise est en cours (l'un des deux voit toujours l'autre).
/// `on_event` reçoit l'avancement des copies. Essais : l'app passe par
/// [`change_with`] (moteur déchargé une fois le changement accepté).
#[cfg(test)]
pub async fn change(
    store: &Store,
    data_dir: &Path,
    new: PathBuf,
    move_models: bool,
    cancel: Arc<AtomicBool>,
    idle: impl FnOnce() -> AppResult<()>,
    on_event: impl Fn(FolderEvent) + Send + Sync,
) -> AppResult<FolderMoved> {
    change_with(
        store,
        data_dir,
        new,
        move_models,
        cancel,
        idle,
        async {},
        on_event,
    )
    .await
}

/// [`change`], avec `before_move` attendu une fois le changement accepté
/// (tous les refus passés, dossier différent), juste avant de toucher aux
/// fichiers ; son résultat est gardé jusqu'à la fin (le moteur gelé et ses
/// modèles déchargés : `Engine::freeze`). Refus : jamais attendu.
#[allow(clippy::too_many_arguments)]
pub async fn change_with<G>(
    store: &Store,
    data_dir: &Path,
    new: PathBuf,
    move_models: bool,
    cancel: Arc<AtomicBool>,
    idle: impl FnOnce() -> AppResult<()>,
    before_move: impl std::future::Future<Output = G>,
    on_event: impl Fn(FolderEvent) + Send + Sync,
) -> AppResult<FolderMoved> {
    if !new.is_absolute() {
        return Err(AppError::Invalid(t!("modelImport.folderNotAbsolute")));
    }
    let _busy = try_busy().ok_or_else(|| AppError::Invalid(t!("modelImport.busy")))?;
    idle()?;
    let _cancellable = Cancellable::new(&cancel);
    recover_locked(store, data_dir).await?;
    let data_dir_owned = data_dir.to_owned();
    let (old, models) = store
        .call(move |connection| {
            Ok((
                crate::local_models::models_dir_in(connection, &data_dir_owned)?,
                table::list(connection)?,
            ))
        })
        .await?;
    let path_text = new.to_string_lossy().into_owned();
    let unchanged = FolderMoved {
        path: path_text.clone(),
        renamed: 0,
        copied: 0,
        cleanup_failed: false,
    };
    if same_path(&old, &new) {
        return Ok(unchanged);
    }
    if inside(&new, &old) {
        return Err(AppError::Invalid(t!("modelImport.folderInside")));
    }
    // L'ancien dans le nouveau : les fichiers de l'app s'y mêleraient
    // (`<données>` choisi pour `<données>/models`).
    if inside(&old, &new) {
        return Err(AppError::Invalid(t!("modelImport.folderContains")));
    }
    if !writable(&new) {
        return Err(AppError::Invalid(t!(
            "modelImport.folderNotWritable",
            dir = path_text
        )));
    }
    // Fichiers distincts (un projecteur partagé une seule fois), empreintes
    // connues.
    let mut files: BTreeMap<String, (u64, String)> = BTreeMap::new();
    if move_models {
        for model in &models {
            for relative in model.all_files() {
                let Some(source) = crate::local_models::absolute(&old, relative) else {
                    continue;
                };
                let Ok(meta) = fs::metadata(&source) else {
                    continue;
                };
                files.insert(
                    relative.to_owned(),
                    (
                        meta.len(),
                        model.sha256.get(relative).cloned().unwrap_or_default(),
                    ),
                );
            }
        }
    }
    let rename = same_filesystem(&old, &new);
    let total: u64 = if rename {
        0
    } else {
        files.values().map(|(size, _)| size).sum()
    };
    if total > 0
        && let Some(free) = crate::disk::free_space_for(&new)
        && total.saturating_add(SPACE_MARGIN) > free
    {
        return Err(AppError::Invalid(t!(
            "modelImport.noSpace",
            needed = crate::image_models::downloads::size_text(total + SPACE_MARGIN),
            free = crate::image_models::downloads::size_text(free)
        )));
    }
    // Accepté : les modèles déchargés (fichiers libres, sous Windows aussi),
    // aucun relancé jusqu'à la fin.
    let _released = before_move.await;
    on_event(FolderEvent::Total { bytes: total });
    let steps: Vec<Step> = files
        .iter()
        .filter_map(|(relative, (size, sha256))| {
            Some(Step {
                source: crate::local_models::absolute(&old, relative)?,
                target: crate::local_models::absolute(&new, relative)?,
                size: *size,
                sha256: sha256.clone(),
                mode: if rename {
                    Mode::Move { rename_ok: true }
                } else {
                    Mode::Copy
                },
            })
        })
        .collect();
    // Journal : une coupure à partir d'ici est défaite par `recover`.
    if !steps.is_empty() {
        let target = path_text.clone();
        store
            .call(move |c| records::set_models_dir_moving(c, Some(&target)))
            .await?;
    }
    let done = Arc::new(AtomicU64::new(0));
    let result = {
        let (task_steps, task_cancel, task_done) = (steps.clone(), cancel.clone(), done.clone());
        let mut task = tauri::async_runtime::spawn_blocking(move || {
            let mut placed = Vec::new();
            for step in &task_steps {
                match transfer::place(step, &task_cancel, &task_done) {
                    Ok(item) => placed.push(item),
                    Err(error) => return Err((error, placed)),
                }
            }
            Ok(placed)
        });
        let mut interval = tokio::time::interval(PROGRESS_INTERVAL);
        let mut reported = 0;
        loop {
            tokio::select! {
                result = &mut task => break result,
                _ = interval.tick() => {
                    let now = done.load(Ordering::Relaxed);
                    if now != reported {
                        reported = now;
                        on_event(FolderEvent::Progress { done: now });
                    }
                }
            }
        }
    };
    // Défait les fichiers mis en place ; journal effacé si tout l'a été
    // (sinon `recover` réessaiera au prochain passage).
    let put_back = |placed: Vec<Placed>| {
        let root = new.clone();
        let targets: Vec<PathBuf> = steps.iter().map(|step| step.target.clone()).collect();
        async move {
            let restored = tauri::async_runtime::spawn_blocking(move || {
                let restored = transfer::undo(&placed, &root);
                for target in &targets {
                    transfer::remove_empty_parents(target, &root);
                }
                restored
            })
            .await
            .unwrap_or(false);
            if restored {
                let _ = store
                    .call(|c| records::set_models_dir_moving(c, None))
                    .await;
            }
            restored
        }
    };
    let placed = match result {
        Ok(Ok(placed)) => placed,
        Ok(Err((error, placed))) => {
            let restored = put_back(placed).await;
            return Err(AppError::System(super::transfer_text(
                &error.with_undo(restored),
            )));
        }
        Err(error) => return Err(AppError::Internal(error.to_string())),
    };
    let value = (!same_path(&new, &crate::local_models::default_models_dir(data_dir)))
        .then(|| path_text.clone());
    let saved = store
        .call(move |connection| records::finish_models_dir_move(connection, value.as_deref()))
        .await;
    if let Err(error) = saved {
        let restored = put_back(placed).await;
        if !restored {
            return Err(AppError::System(t!(
                "modelImport.undoFailed",
                error = error
            )));
        }
        return Err(error);
    }
    if total > 0 {
        on_event(FolderEvent::Progress { done: total });
    }
    let mut moved = unchanged;
    let old_root = old.clone();
    let (renamed, copied, cleanup_failed) = tauri::async_runtime::spawn_blocking(move || {
        let (mut renamed, mut copied, mut failed) = (0u32, 0u32, false);
        for (step, item) in steps.iter().zip(&placed) {
            match item {
                Placed::Renamed { .. } => renamed += 1,
                // Lien physique : l'ancien nom part (comme un renommage).
                Placed::Linked(_) => {
                    renamed += 1;
                    failed |= fs::remove_file(&step.source).is_err() && step.source.exists();
                }
                // Copié (ou déjà là) : l'ancien fichier part.
                Placed::Written(_) | Placed::Present(_) => {
                    copied += 1;
                    failed |= fs::remove_file(&step.source).is_err() && step.source.exists();
                }
            }
            transfer::remove_empty_parents(&step.source, &old_root);
        }
        (renamed, copied, failed)
    })
    .await
    .map_err(|error| AppError::Internal(error.to_string()))?;
    moved.renamed = renamed;
    moved.copied = copied;
    moved.cleanup_failed = cleanup_failed;
    Ok(moved)
}

/// Reprend un changement de dossier coupé (journal
/// `engine_settings.models_dir_moving`) : chaque fichier de la base passé
/// dans le dossier visé retourne dans le dossier réglé (déjà là, identique :
/// la copie du dossier visé part) ; fichiers partiels effacés ; journal
/// effacé si tout a été remis. Rien en cours : rien. Rend le nombre de
/// fichiers remis.
pub async fn recover(store: &Store, data_dir: &Path) -> AppResult<u32> {
    let Some(_busy) = try_busy() else {
        return Ok(0);
    };
    recover_locked(store, data_dir).await
}

async fn recover_locked(store: &Store, data_dir: &Path) -> AppResult<u32> {
    let data_dir_owned = data_dir.to_owned();
    let (moving, old, models) = store
        .call(move |connection| {
            Ok((
                records::models_dir_moving(connection)?,
                crate::local_models::models_dir_in(connection, &data_dir_owned)?,
                table::list(connection)?,
            ))
        })
        .await?;
    let Some(new) = moving.map(PathBuf::from) else {
        return Ok(0);
    };
    if same_path(&old, &new) {
        store
            .call(|c| records::set_models_dir_moving(c, None))
            .await?;
        return Ok(0);
    }
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    for model in &models {
        for relative in model.all_files() {
            files.insert(
                relative.to_owned(),
                model.sha256.get(relative).cloned().unwrap_or_default(),
            );
        }
    }
    let (restored, complete) = tauri::async_runtime::spawn_blocking(move || {
        let (mut restored, mut complete) = (0u32, true);
        let never = AtomicBool::new(false);
        let counted = AtomicU64::new(0);
        for (relative, sha256) in &files {
            let (Some(at_old), Some(at_new)) = (
                crate::local_models::absolute(&old, relative),
                crate::local_models::absolute(&new, relative),
            ) else {
                continue;
            };
            let _ = fs::remove_file(transfer::partial_path(&at_new));
            let Ok(meta) = fs::symlink_metadata(&at_new) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            // Empreinte inconnue : celle du fichier du dossier visé (une
            // copie identique déjà dans l'ancien est reconnue).
            let sha256 = if sha256.is_empty() {
                transfer::hash_file(&at_new, &never).unwrap_or_default()
            } else {
                sha256.clone()
            };
            let step = Step {
                source: at_new.clone(),
                target: at_old.clone(),
                size: meta.len(),
                sha256,
                mode: Mode::Move { rename_ok: true },
            };
            match transfer::place(&step, &never, &counted) {
                Ok(Placed::Renamed { .. }) => restored += 1,
                Ok(_) => {
                    if !matches!(fs::symlink_metadata(&at_old), Ok(meta) if meta.is_file()) {
                        complete = false;
                        continue;
                    }
                    if fs::remove_file(&at_new).is_ok() {
                        restored += 1;
                    } else {
                        complete = false;
                    }
                }
                // Deux fichiers différents : rien n'est effacé, le journal
                // reste (à voir à la main).
                Err(_) => complete = false,
            }
            transfer::remove_empty_parents(&at_new, &new);
        }
        (restored, complete)
    })
    .await
    .map_err(|error| AppError::Internal(error.to_string()))?;
    if complete {
        store
            .call(|c| records::set_models_dir_moving(c, None))
            .await?;
    }
    Ok(restored)
}
