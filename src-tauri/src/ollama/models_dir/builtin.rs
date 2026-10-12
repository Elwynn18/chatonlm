//! Dossier des modèles de l'Ollama intégré (Settings › Model location en
//! mode intégré). Le dossier appartient à l'utilisateur : ChatonLM le change
//! lui-même, sans droits ni invite (le chemin polkit reste celui du service
//! système). Mêmes étapes et mêmes événements que le script du service
//! ([`MoveEvent`], [`MoveOutcome`]) : vérification, arrêt de l'intégré,
//! déplacement (renommage sur le même système de fichiers, sinon copie avec
//! progression), réglage, redémarrage, puis effacement des anciennes copies.
//! Un fichier déjà présent dans le nouveau dossier est gardé (fusion) ; un
//! échec remet les fichiers à leur place. Refusé avant tout arrêt ou
//! déplacement : un modèle du même nom et d'un autre contenu déjà dans le
//! nouveau dossier (Ollama effacerait, au redémarrage, les couches de la
//! version écrasée), un lien symbolique ou un fichier spécial (ni déplacé ni
//! compté : modèles cassés).

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{
    Detection, FolderCheck, ModelsLocation, ModelsSummary, MoveEvent, MoveFailure, MoveOutcome,
    MoveStep, PROGRESS_INTERVAL, SPACE_MARGIN,
};
use crate::i18n::t;
use crate::ollama::builtin::{Builtin, BuiltinState, Paused};

/// Suffixe des copies en cours (fichier renommé une fois complet).
const PARTIAL: &str = ".chatonlm-partial";

/// Détection pour l'intégré : son dossier, sans service ni commandes.
pub fn detection(dir: &Path, is_default: bool) -> Detection {
    Detection::new(
        ModelsLocation::Builtin {
            dir: dir.to_string_lossy().into_owned(),
            is_default,
        },
        None,
    )
}

/// Modèles d'un dossier d'Ollama, lus sur le disque (serveur arrêté ou
/// autre serveur) : un par fichier de `manifests/`, taille des fichiers de
/// `blobs/` (téléchargements inachevés exclus) ; zéro s'il n'y a rien (ou
/// pas de dossier).
pub fn folder_summary(dir: &Path) -> ModelsSummary {
    let mut count = 0u32;
    let mut stack = vec![dir.join("manifests")];
    while let Some(folder) = stack.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(entry.path()),
                Ok(kind) if kind.is_file() => count = count.saturating_add(1),
                _ => {}
            }
        }
    }
    let bytes = fs::read_dir(dir.join("blobs"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| !entry.file_name().to_string_lossy().contains("-partial"))
                .filter_map(|entry| entry.metadata().ok())
                .filter(fs::Metadata::is_file)
                .map(|meta| meta.len())
                .sum()
        })
        .unwrap_or(0);
    ModelsSummary { count, bytes }
}

/// Fichier à déplacer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Chemin relatif au dossier des modèles.
    pub relative: PathBuf,
    pub bytes: u64,
    /// Déjà présent dans le nouveau dossier : gardé tel quel.
    pub skipped: bool,
}

/// Déplacement impossible, vu avant de rien toucher.
#[derive(Debug)]
pub enum PlanError {
    Io(io::Error),
    /// Modèle du même nom, d'un autre contenu, dans le nouveau dossier.
    Conflict {
        model: String,
    },
    /// Lien symbolique ou fichier spécial (chemin relatif).
    Special {
        path: PathBuf,
    },
}

impl From<io::Error> for PlanError {
    fn from(error: io::Error) -> Self {
        PlanError::Io(error)
    }
}

/// Fichiers de l'ancien dossier, dans un ordre stable. Un fichier déjà
/// présent dans le nouveau dossier est gardé tel quel (blob : même nom, même
/// contenu) ; un manifeste déjà présent doit être identique, sinon la
/// fusion est refusée ([`PlanError::Conflict`]) ; un lien symbolique ou un
/// fichier spécial aussi ([`PlanError::Special`]).
pub fn plan(old: &Path, new: &Path) -> Result<Vec<Entry>, PlanError> {
    let mut entries = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(relative) = stack.pop() {
        let folder = old.join(&relative);
        let listing = match fs::read_dir(&folder) {
            Ok(listing) => listing,
            Err(error)
                if error.kind() == io::ErrorKind::NotFound && relative.as_os_str().is_empty() =>
            {
                return Ok(Vec::new());
            }
            Err(error) => return Err(error.into()),
        };
        for entry in listing {
            let entry = entry?;
            let path = relative.join(entry.file_name());
            // Ne suit pas les liens.
            let kind = entry.file_type()?;
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                let present = new.join(&path);
                let skipped = fs::symlink_metadata(&present).is_ok();
                if skipped
                    && path.starts_with("manifests")
                    && fs::read(old.join(&path))? != fs::read(&present).unwrap_or_default()
                {
                    return Err(PlanError::Conflict {
                        model: model_name(&path),
                    });
                }
                entries.push(Entry {
                    bytes: entry.metadata()?.len(),
                    relative: path,
                    skipped,
                });
            } else {
                return Err(PlanError::Special { path });
            }
        }
    }
    entries.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(entries)
}

/// Nom d'un modèle d'après son manifeste
/// (`manifests/<hôte>/<espace>/<modèle>/<étiquette>`), écrit comme Ollama :
/// `qwen3:0.6b`, `user/model:tag`, `hf.co/user/repo:tag`.
fn model_name(manifest: &Path) -> String {
    let parts: Vec<String> = manifest
        .components()
        .skip(1)
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    match parts.as_slice() {
        [host, space, model, tag] if host == "registry.ollama.ai" && space == "library" => {
            format!("{model}:{tag}")
        }
        [host, space, model, tag] if host == "registry.ollama.ai" => {
            format!("{space}/{model}:{tag}")
        }
        [host, space, model, tag] => format!("{host}/{space}/{model}:{tag}"),
        _ => manifest.display().to_string(),
    }
}

/// Octets à copier.
pub fn to_copy(entries: &[Entry]) -> u64 {
    entries
        .iter()
        .filter(|entry| !entry.skipped)
        .map(|entry| entry.bytes)
        .sum()
}

/// Ce que le déplacement a fait (pour l'annuler ou effacer les anciennes
/// copies).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Moved {
    pub renamed: Vec<PathBuf>,
    pub copied: Vec<PathBuf>,
    pub skipped: u32,
}

/// Échec du déplacement, après remise en place.
#[derive(Debug)]
pub struct MoveError {
    pub detail: String,
    pub restored: bool,
}

/// Déplace les fichiers de `entries` de `old` vers `new` : renommage si
/// `same_drive` (copie si le système le refuse), sinon copie ; `done`
/// compte les octets copiés. En cas d'échec, tout est remis en place.
pub fn move_files(
    old: &Path,
    new: &Path,
    entries: &[Entry],
    same_drive: bool,
    done: &AtomicU64,
) -> Result<Moved, MoveError> {
    let mut moved = Moved::default();
    for entry in entries {
        if entry.skipped {
            moved.skipped += 1;
            continue;
        }
        let source = old.join(&entry.relative);
        let target = new.join(&entry.relative);
        let result = (|| -> io::Result<bool> {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            if same_drive {
                match fs::rename(&source, &target) {
                    Ok(()) => return Ok(true),
                    // Autre système de fichiers (EXDEV, ou autre volume sous
                    // Windows) : copie.
                    Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {}
                    Err(error) => return Err(error),
                }
            }
            copy_file(&source, &target, done)?;
            Ok(false)
        })();
        match result {
            Ok(true) => moved.renamed.push(entry.relative.clone()),
            Ok(false) => moved.copied.push(entry.relative.clone()),
            Err(error) => {
                let detail = format!("{}: {error}", entry.relative.display());
                let restored = undo(old, new, &moved);
                return Err(MoveError { detail, restored });
            }
        }
    }
    Ok(moved)
}

/// Copie `source` vers `target` (fichier partiel renommé une fois complet et
/// écrit sur le disque).
fn copy_file(source: &Path, target: &Path, done: &AtomicU64) -> io::Result<()> {
    let mut partial = target.as_os_str().to_os_string();
    partial.push(PARTIAL);
    let partial = PathBuf::from(partial);
    let result = (|| {
        let mut reader = File::open(source)?;
        let mut writer = File::create(&partial)?;
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            let read = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            writer.write_all(&buffer[..read])?;
            done.fetch_add(read as u64, Ordering::Relaxed);
        }
        writer.sync_all()?;
        fs::rename(&partial, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

/// Remet en place : renommages défaits, copies effacées ; vrai si tout a
/// pu l'être.
pub fn undo(old: &Path, new: &Path, moved: &Moved) -> bool {
    let mut restored = true;
    for relative in &moved.renamed {
        let back = old.join(relative);
        let result = back
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::rename(new.join(relative), &back));
        restored &= result.is_ok();
    }
    for relative in &moved.copied {
        restored &= fs::remove_file(new.join(relative)).is_ok();
    }
    remove_empty_dirs(new);
    restored
}

/// Efface les anciennes copies et les dossiers vides de `old` ; vrai si tout
/// est parti.
pub fn cleanup(old: &Path, moved: &Moved) -> bool {
    let mut clean = true;
    for relative in &moved.copied {
        clean &= fs::remove_file(old.join(relative)).is_ok();
    }
    remove_empty_dirs(old);
    clean
}

/// Retire les dossiers vides sous `root` (et `root` s'il l'est).
fn remove_empty_dirs(root: &Path) {
    fn walk(dir: &Path) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    walk(&entry.path());
                }
            }
        }
        // Échoue sans dommage si le dossier n'est pas vide.
        let _ = fs::remove_dir(dir);
    }
    walk(root);
}

/// Demande de changement.
pub struct Change {
    /// Dossier actuel.
    pub old: PathBuf,
    /// Vérification du nouveau (sans problème).
    pub check: FolderCheck,
    pub move_models: bool,
}

/// Change le dossier de l'intégré : arrêt, déplacement, `save` (réglage),
/// redémarrage si `start_after` (l'intégré est la cible), effacement.
/// `on_stop` : juste avant l'arrêt (travaux en cours arrêtés). Un échec
/// relance l'intégré sur l'ancien dossier s'il tournait et reste la cible.
pub async fn change<S, F>(
    builtin: &Builtin,
    change: Change,
    start_after: bool,
    mut on_event: impl FnMut(MoveEvent),
    on_stop: impl FnOnce(),
    save: S,
) -> MoveOutcome
where
    S: FnOnce(PathBuf) -> F,
    F: std::future::Future<Output = Result<(), String>>,
{
    let failed = |reason: MoveFailure, restored: bool, detail: String| MoveOutcome::Failed {
        reason,
        restored,
        detail,
    };
    let Change {
        old,
        check,
        move_models,
    } = change;
    let new = PathBuf::from(&check.path);
    on_event(MoveEvent::Step {
        step: MoveStep::Prepare,
    });
    let entries = if move_models {
        let (old_dir, new_dir) = (old.clone(), new.clone());
        match tokio::task::spawn_blocking(move || plan(&old_dir, &new_dir)).await {
            Ok(Ok(entries)) => entries,
            Ok(Err(PlanError::Io(error))) => {
                return failed(MoveFailure::MoveFailed, false, error.to_string());
            }
            Ok(Err(PlanError::Conflict { model })) => {
                let detail = t!("modelLocation.modelConflict", model = model);
                return failed(MoveFailure::ModelConflict { model }, false, detail);
            }
            Ok(Err(PlanError::Special { path })) => {
                let path = old.join(path).to_string_lossy().into_owned();
                let detail = t!("modelLocation.specialFile", path = path);
                return failed(MoveFailure::Symlink { path }, false, detail);
            }
            Err(error) => return failed(MoveFailure::Unexpected, false, error.to_string()),
        }
    } else {
        Vec::new()
    };
    let total = if check.same_drive {
        0
    } else {
        to_copy(&entries)
    };
    if total > 0
        && let Some(free) = check.free_bytes
        && total.saturating_add(SPACE_MARGIN) > free
    {
        return failed(
            MoveFailure::NoSpace {
                need: Some(total),
                free: Some(free),
            },
            false,
            String::new(),
        );
    }

    let snapshot = builtin.snapshot();
    let was_running = matches!(
        snapshot.state,
        BuiltinState::Starting | BuiltinState::Running
    );
    on_event(MoveEvent::Step {
        step: MoveStep::Stop,
    });
    on_stop();
    // Aucune requête ne le relance sur l'ancien dossier pendant le
    // changement.
    let paused = builtin.pause();
    builtin.stop().await;
    let restart_old = |paused: Paused| {
        drop(paused);
        if was_running && start_after {
            builtin.ensure_started(&old);
        }
    };

    on_event(MoveEvent::Step {
        step: MoveStep::Move,
    });
    on_event(MoveEvent::Total { bytes: total });
    let done = Arc::new(AtomicU64::new(0));
    let moved = {
        let (old_dir, new_dir, counter) = (old.clone(), new.clone(), done.clone());
        let same_drive = check.same_drive;
        let mut task = tokio::task::spawn_blocking(move || {
            fs::create_dir_all(&new_dir).map_err(|error| MoveError {
                detail: error.to_string(),
                restored: true,
            })?;
            move_files(&old_dir, &new_dir, &entries, same_drive, &counter)
        });
        let mut tick = tokio::time::interval(PROGRESS_INTERVAL);
        let mut reported = 0;
        loop {
            tokio::select! {
                result = &mut task => break result,
                _ = tick.tick() => {
                    let now = done.load(Ordering::Relaxed);
                    if now != reported {
                        reported = now;
                        on_event(MoveEvent::Progress { done: now });
                    }
                }
            }
        }
    };
    let moved = match moved {
        Ok(Ok(moved)) => moved,
        Ok(Err(error)) => {
            restart_old(paused);
            return failed(MoveFailure::MoveFailed, error.restored, error.detail);
        }
        Err(error) => {
            restart_old(paused);
            return failed(MoveFailure::Unexpected, false, error.to_string());
        }
    };
    if total > 0 {
        on_event(MoveEvent::Progress { done: total });
    }

    on_event(MoveEvent::Step {
        step: MoveStep::Config,
    });
    if let Err(detail) = save(new.clone()).await {
        let (old_dir, new_dir) = (old.clone(), new.clone());
        let restored = tokio::task::spawn_blocking(move || undo(&old_dir, &new_dir, &moved))
            .await
            .unwrap_or(false);
        restart_old(paused);
        return failed(MoveFailure::ConfigFailed, restored, detail);
    }

    on_event(MoveEvent::Step {
        step: MoveStep::Start,
    });
    drop(paused);
    // Un intégré qui n'est plus la cible n'est pas relancé.
    let restarted = start_after;
    if restarted {
        builtin.ensure_started(&new);
        let limit = builtin.config().timing.ready_timeout + PROGRESS_INTERVAL;
        let snapshot = builtin.wait_ready(limit).await;
        if snapshot.state != BuiltinState::Running {
            return failed(
                MoveFailure::StartFailed,
                false,
                snapshot.failure.unwrap_or_default(),
            );
        }
    }

    on_event(MoveEvent::Step {
        step: MoveStep::Cleanup,
    });
    let (renamed, copied, skipped) = (
        count(moved.renamed.len()),
        count(moved.copied.len()),
        moved.skipped,
    );
    let cleanup_failed = !tokio::task::spawn_blocking(move || cleanup(&old, &moved))
        .await
        .unwrap_or(false);
    MoveOutcome::Done {
        restarted,
        renamed,
        copied,
        skipped,
        cleanup_failed,
    }
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
