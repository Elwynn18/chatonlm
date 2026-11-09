//! Reprise des modèles côté app (état Tauri [`ModelImport`]) : une reprise
//! à la fois, état envoyé à la page à chaque changement (événement
//! [`super::EVENT`]), annulation ; reprise automatique de l'Ollama intégré
//! au premier lancement ([`at_startup`]) ; dossiers de l'Ollama de la
//! machine ([`machine_dirs`]) ; ménage de l'intégré.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use super::cleanup::{self, CleanupReport, Leftovers};
use super::{Context, ImportOrigin, ImportReport, ImportState, Job};
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::Store;
use crate::store::model_import as records;

/// Reprise automatique de l'Ollama intégré au lancement : coupée tant que
/// le chat passe encore par Ollama (lot L5) ; à allumer avec le retrait de
/// l'intégré (lot L10). La commande `model_import_start` marche déjà.
pub const AUTO_AT_STARTUP: bool = false;

/// Délai de la détection du dossier de l'Ollama de la machine.
#[cfg_attr(test, allow(dead_code))]
const DETECT_TIMEOUT: Duration = Duration::from_secs(5);

type Emit = Arc<dyn Fn(&ImportState) + Send + Sync>;

struct Inner {
    state: Mutex<ImportState>,
    cancel: Mutex<Option<Arc<AtomicBool>>>,
    emit: Emit,
}

/// État Tauri de la reprise.
#[derive(Clone)]
pub struct ModelImport(Arc<Inner>);

impl ModelImport {
    pub fn new(emit: impl Fn(&ImportState) + Send + Sync + 'static) -> Self {
        Self(Arc::new(Inner {
            state: Mutex::new(ImportState::default()),
            cancel: Mutex::new(None),
            emit: Arc::new(emit),
        }))
    }

    pub fn state(&self) -> ImportState {
        self.0.state.lock().clone()
    }

    /// Une reprise est en cours.
    pub fn is_running(&self) -> bool {
        self.0.cancel.lock().is_some()
    }

    fn update(&self, change: impl FnOnce(&mut ImportState)) {
        let state = {
            let mut state = self.0.state.lock();
            change(&mut state);
            state.clone()
        };
        (self.0.emit)(&state);
    }

    /// Demande l'arrêt de la reprise en cours (le fichier en cours est
    /// effacé, le modèle en cours remis en place).
    pub fn cancel(&self) {
        if let Some(cancel) = self.0.cancel.lock().as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Lance la reprise `job` ; `before` est attendu d'abord (arrêt de
    /// l'Ollama intégré dont les fichiers vont bouger) et ce qu'il rend est
    /// gardé jusqu'à la fin (l'intégré reste arrêté). Refus si une reprise
    /// est déjà en cours. Rend la tâche (attendue par les essais).
    pub fn start<F, G>(
        &self,
        job: Job,
        store: Store,
        data_dir: PathBuf,
        before: F,
    ) -> AppResult<tauri::async_runtime::JoinHandle<ImportReport>>
    where
        F: std::future::Future<Output = G> + Send + 'static,
        G: Send + 'static,
    {
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut slot = self.0.cancel.lock();
            if slot.is_some() || super::folder::is_busy() {
                return Err(AppError::Invalid(t!("modelImport.busy")));
            }
            *slot = Some(cancel.clone());
        }
        let this = self.clone();
        let context = Context {
            store,
            data_dir,
            hf_url: crate::hub::names::hf_url(),
            cancel,
        };
        Ok(tauri::async_runtime::spawn(async move {
            let guard = before.await;
            let progress = this.clone();
            let report = super::run(job, context, move |running| {
                progress.update(|state| state.running = Some(running));
            })
            .await;
            drop(guard);
            *this.0.cancel.lock() = None;
            this.update(|state| {
                state.running = None;
                state.last = Some(report.clone());
            });
            report
        }))
    }
}

/// Dossier des modèles de l'Ollama intégré (réglé, sinon par défaut).
pub async fn builtin_dir(store: &Store, data_dir: &Path) -> AppResult<PathBuf> {
    let data_dir = data_dir.to_owned();
    store
        .call(move |connection| super::builtin_models_dir(connection, &data_dir))
        .await
}

/// Arrête l'Ollama intégré s'il tourne (ses fichiers vont bouger) : chat,
/// cours et corrections en cours s'arrêtent d'abord ; il ne redémarre pas
/// tant que le résultat vit.
pub async fn stop_builtin() -> Option<crate::ollama::builtin::Paused> {
    let targeting = crate::ollama::target::current()?;
    targeting.stop_work();
    let paused = targeting.builtin().pause();
    targeting.builtin().stop().await;
    Some(paused)
}

/// Dossiers de l'Ollama de la machine trouvés (voir [`super::sources`]),
/// hors de `exclude`. Essais : jamais de détection réelle (aucun Ollama de la
/// machine interrogé), dossiers donnés par [`TEST_MACHINE_DIRS`].
async fn machine_candidates(exclude: Vec<PathBuf>) -> Vec<PathBuf> {
    #[cfg(test)]
    {
        let dirs = TEST_MACHINE_DIRS.with(|dirs| dirs.borrow().clone());
        super::sources::candidates(None, None, None, dirs, &exclude)
    }
    #[cfg(not(test))]
    {
        let detected = tokio::time::timeout(
            DETECT_TIMEOUT,
            crate::ollama::models_dir::detect("http://127.0.0.1:11434"),
        )
        .await
        .ok()
        .and_then(|detection| {
            crate::ollama::models_dir::current_dir(&detection.location).map(PathBuf::from)
        });
        let env = std::env::var_os("OLLAMA_MODELS")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        tauri::async_runtime::spawn_blocking(move || {
            super::sources::candidates(
                env,
                detected,
                super::sources::user_home(),
                super::sources::system_dirs(),
                &exclude,
            )
        })
        .await
        .unwrap_or_default()
    }
}

#[cfg(test)]
thread_local! {
    /// Dossiers de « l'Ollama de la machine » des essais (fil de l'essai).
    pub(crate) static TEST_MACHINE_DIRS: std::cell::RefCell<Vec<PathBuf>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Le dossier `dir` de l'intégré est aussi celui de l'Ollama de la machine
/// (déjà vu, ou parmi les dossiers trouvés maintenant) : copies seulement,
/// ni lien, ni renommage, ni ménage (relecture L6, B1). Base illisible :
/// vrai (dans le doute, copie).
pub async fn builtin_shared(store: &Store, dir: &Path) -> bool {
    if store
        .call(|c| records::builtin_shared(c))
        .await
        .unwrap_or(true)
    {
        return true;
    }
    let machine = machine_candidates(Vec::new()).await;
    let dir = dir.to_owned();
    tauri::async_runtime::spawn_blocking(move || super::sources::shared_dir(&dir, &machine))
        .await
        .unwrap_or(true)
}

/// Reprise automatique de l'Ollama intégré au lancement, tant qu'elle n'a
/// pas abouti : changement de dossier des modèles coupé d'abord repris
/// ([`super::folder::recover`]) ; dossier réglé absent ou illisible (disque
/// pas encore monté) : reprise laissée en attente, essayée au lancement
/// suivant ; dossier par défaut qui n'a jamais existé, ou sans manifeste :
/// marquée faite ; sinon ses modèles importables sont repris (l'intégré
/// arrêté pendant ce temps), sans retenter ceux abandonnés après deux échecs.
pub async fn at_startup(import: &ModelImport, store: Store, data_dir: PathBuf) {
    let _ = super::folder::recover(&store, &data_dir).await;
    let done = store
        .call(|c| records::builtin_import_done(c))
        .await
        .unwrap_or(true);
    if done {
        return;
    }
    let Ok(dir) = builtin_dir(&store, &data_dir).await else {
        return;
    };
    let configured = store
        .call(|c| crate::store::builtin_ollama::get(c))
        .await
        .map(|settings| settings.models_dir.is_some())
        .unwrap_or(true);
    match std::fs::read_dir(&dir) {
        Ok(_) => {}
        // Dossier par défaut jamais créé (dossier des données lisible) :
        // aucun modèle n'a pu y être tiré.
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && !configured
                && std::fs::read_dir(&data_dir).is_ok() =>
        {
            let _ = store
                .call(|connection| records::set_builtin_import_done(connection, true))
                .await;
            return;
        }
        // Disque pas monté, droits : rien n'est décidé (relecture L6, B2).
        Err(_) => return,
    }
    let empty = !dir.join("manifests").is_dir() && !dir.join("manifests-v2").is_dir();
    if empty {
        let _ = store
            .call(|connection| records::set_builtin_import_done(connection, true))
            .await;
        return;
    }
    let shared = builtin_shared(&store, &dir).await;
    let job = Job {
        origin: ImportOrigin::Builtin,
        dir,
        names: None,
        shared,
        retry_failed: false,
    };
    if let Ok(task) = import.start(job, store, data_dir, stop_builtin()) {
        let _ = task.await;
    }
}

/// Dossiers de l'Ollama de la machine qui existent (voir
/// [`super::sources`]), hors de ceux de l'app.
pub async fn machine_dirs(store: &Store, data_dir: &Path) -> AppResult<Vec<PathBuf>> {
    let builtin = builtin_dir(store, data_dir).await?;
    let models = crate::local_models::models_dir(store, data_dir).await?;
    Ok(machine_candidates(vec![builtin, models]).await)
}

/// Chemins absolus des fichiers de la base locale (gardés par le ménage).
async fn kept_files(store: &Store, data_dir: &Path) -> AppResult<HashSet<PathBuf>> {
    let data_dir = data_dir.to_owned();
    store
        .call(move |connection| {
            let dir = crate::local_models::models_dir_in(connection, &data_dir)?;
            Ok(crate::store::local_models::list(connection)?
                .iter()
                .flat_map(|model| {
                    model
                        .all_files()
                        .filter_map(|relative| crate::local_models::absolute(&dir, relative))
                        .collect::<Vec<_>>()
                })
                .collect())
        })
        .await
}

/// Le ménage de l'intégré est permis : reprise faite, dossier propre à
/// l'intégré (jamais celui de la machine), lisible, et plus aucun modèle à
/// reprendre (relecture L6, B1, B2, I1, I8). Rend son dossier et les modèles
/// qui disparaîtront, sinon le texte du refus.
async fn cleanable(
    store: &Store,
    data_dir: &Path,
) -> AppResult<Result<(PathBuf, Vec<super::ImportCandidate>), String>> {
    if !store.call(|c| records::builtin_import_done(c)).await? {
        return Ok(Err(t!("modelImport.notImportedYet")));
    }
    let dir = builtin_dir(store, data_dir).await?;
    if builtin_shared(store, &dir).await {
        return Ok(Err(t!("modelImport.sharedFolder")));
    }
    Ok(match super::builtin_left(store, data_dir, &dir).await? {
        Some(models) => Ok((dir, models)),
        None => Err(t!("modelImport.notImportedYet")),
    })
}

/// Ce qui reste de l'Ollama intégré, avec les modèles qui disparaîtront ;
/// `None` tant que le ménage n'est pas permis (voir [`cleanable`]) ou qu'il
/// ne reste rien.
pub async fn leftovers(store: &Store, data_dir: &Path) -> AppResult<Option<Leftovers>> {
    let Ok((models, names)) = cleanable(store, data_dir).await? else {
        return Ok(None);
    };
    let keep = kept_files(store, data_dir).await?;
    let root = data_dir.join("ollama");
    let mut left =
        tauri::async_runtime::spawn_blocking(move || cleanup::leftovers(&models, &root, &keep))
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
    left.models = names;
    Ok((left.files > 0).then_some(left))
}

/// Efface ce qui reste de l'Ollama intégré (après confirmation) ; refus tant
/// que le ménage n'est pas permis, ou pendant une reprise.
pub async fn clean(
    import: &ModelImport,
    store: &Store,
    data_dir: &Path,
) -> AppResult<CleanupReport> {
    if import.is_running() {
        return Err(AppError::Invalid(t!("modelImport.busy")));
    }
    let models = match cleanable(store, data_dir).await? {
        Ok((models, _)) => models,
        Err(text) => return Err(AppError::Invalid(text)),
    };
    let keep = kept_files(store, data_dir).await?;
    let root = data_dir.join("ollama");
    let paused = stop_builtin().await;
    let report =
        tauri::async_runtime::spawn_blocking(move || cleanup::cleanup(&models, &root, &keep))
            .await
            .map_err(|error| AppError::Internal(error.to_string()));
    drop(paused);
    report
}
