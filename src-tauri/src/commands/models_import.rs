//! Reprise des modèles des Ollama de ChatonLM 0.9.x et dossier des modèles
//! de la base locale (lot L6 ; écran au lot L5). État de la reprise poussé
//! par l'événement `model-import` (`ImportState` complet à chaque
//! changement) ; avancement d'un changement de dossier par un `Channel`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::local_models::import::cleanup::{CleanupReport, Leftovers};
use crate::local_models::import::folder::{self, EngineModelsDir, FolderEvent, FolderMoved};
use crate::local_models::import::service::{self, ModelImport};
use crate::local_models::import::{self, ImportOrigin, ImportSource, ImportState, Job};
use crate::store::Store;

fn data_dir(app: &AppHandle) -> AppResult<PathBuf> {
    crate::paths::data_dir(app)
}

/// État de la reprise (en cours, dernier bilan).
#[tauri::command]
pub async fn model_import_state(import: State<'_, ModelImport>) -> AppResult<ImportState> {
    Ok(import.state())
}

/// Dossiers d'Ollama et leurs modèles : l'intégré (`builtin`, un dossier),
/// ou ceux de la machine trouvés (`machine`, lecture seule).
#[tauri::command]
pub async fn model_import_scan(
    app: AppHandle,
    store: State<'_, Store>,
    origin: ImportOrigin,
) -> AppResult<Vec<ImportSource>> {
    let data_dir = data_dir(&app)?;
    let dirs = match origin {
        ImportOrigin::Builtin => vec![service::builtin_dir(&store, &data_dir).await?],
        ImportOrigin::Machine => service::machine_dirs(&store, &data_dir).await?,
    };
    let mut sources = Vec::new();
    for dir in dirs {
        let shared = origin == ImportOrigin::Builtin && service::builtin_shared(&store, &dir).await;
        sources.push(import::source(&store, &data_dir, origin, &dir, shared).await?);
    }
    Ok(sources)
}

/// Lance la reprise des modèles `names` (tous les importables si vide, ceux
/// abandonnés après deux échecs compris) du dossier `dir` : pour l'intégré,
/// son dossier (l'intégré est arrêté ; copies seulement s'il est aussi celui
/// de l'Ollama de la machine) ; pour la machine, un dossier choisi ou trouvé
/// (copie seulement).
#[tauri::command]
pub async fn model_import_start(
    app: AppHandle,
    store: State<'_, Store>,
    import: State<'_, ModelImport>,
    origin: ImportOrigin,
    dir: String,
    names: Vec<String>,
) -> AppResult<()> {
    let data_dir = data_dir(&app)?;
    let dir = PathBuf::from(dir);
    if origin == ImportOrigin::Builtin && dir != service::builtin_dir(&store, &data_dir).await? {
        // i18n-ignore : la page envoie le dossier qu'elle a reçu.
        return Err(AppError::Invalid("not the built-in Ollama folder".into()));
    }
    if !dir.is_absolute() || !dir.is_dir() {
        return Err(AppError::Invalid(t!(
            "modelImport.unreadableFolder",
            dir = dir.to_string_lossy(),
            error = std::io::Error::from(std::io::ErrorKind::NotFound)
        )));
    }
    let shared = origin == ImportOrigin::Builtin && service::builtin_shared(&store, &dir).await;
    let job = Job {
        origin,
        dir,
        names: (!names.is_empty()).then_some(names),
        shared,
        retry_failed: true,
    };
    let store = store.inner().clone();
    match origin {
        ImportOrigin::Builtin => import.start(job, store, data_dir, service::stop_builtin())?,
        ImportOrigin::Machine => import.start(job, store, data_dir, async {})?,
    };
    Ok(())
}

/// Arrête la reprise en cours, ou le changement du dossier des modèles en
/// cours (tout remis dans l'ancien dossier).
#[tauri::command]
pub async fn model_import_cancel(import: State<'_, ModelImport>) -> AppResult<()> {
    import.cancel();
    folder::cancel();
    Ok(())
}

/// Ce qui reste de l'Ollama intégré après sa reprise (`null` : rien, ou
/// reprise pas encore aboutie).
#[tauri::command]
pub async fn model_import_leftovers(
    app: AppHandle,
    store: State<'_, Store>,
) -> AppResult<Option<Leftovers>> {
    service::leftovers(&store, &data_dir(&app)?).await
}

/// Efface ce qui reste de l'Ollama intégré (après confirmation de
/// l'utilisateur).
#[tauri::command]
pub async fn model_import_cleanup(
    app: AppHandle,
    store: State<'_, Store>,
    import: State<'_, ModelImport>,
) -> AppResult<CleanupReport> {
    service::clean(&import, &store, &data_dir(&app)?).await
}

/// Dossier des modèles de la base locale (un déplacement coupé est d'abord
/// défait).
#[tauri::command]
pub async fn engine_models_dir(
    app: AppHandle,
    store: State<'_, Store>,
) -> AppResult<EngineModelsDir> {
    let data_dir = data_dir(&app)?;
    folder::recover(&store, &data_dir).await?;
    folder::current(&store, &data_dir).await
}

/// Change le dossier des modèles (`path` absolu, choisi par
/// `pick_models_folder`) en y déplaçant les modèles si `move_models`. Refus
/// pendant un téléchargement du ModelHub ou une reprise. Les moteurs qui
/// servent un modèle doivent être arrêtés avant (lot L3).
#[tauri::command]
pub async fn change_engine_models_dir(
    app: AppHandle,
    store: State<'_, Store>,
    import: State<'_, ModelImport>,
    path: String,
    move_models: bool,
    on_event: Channel<FolderEvent>,
) -> AppResult<FolderMoved> {
    let data_dir = data_dir(&app)?;
    let store = store.inner().clone();
    let import = import.inner().clone();
    // Tâche à part : une page rechargée n'arrête pas le déplacement.
    tauri::async_runtime::spawn(async move {
        // Vérifié verrou pris : un téléchargement qui commence voit le
        // verrou (`hub::downloads`), ou ce changement voit le téléchargement ;
        // de même pour une reprise (`ModelImport::start`).
        let idle = || {
            if import.is_running() {
                return Err(AppError::Invalid(t!("modelImport.busy")));
            }
            let downloads = app.try_state::<crate::hub::downloads::HubDownloads>();
            if downloads.is_some_and(|downloads| !downloads.list().is_empty()) {
                return Err(AppError::Invalid(t!("modelImport.downloadsRunning")));
            }
            Ok(())
        };
        folder::change(
            &store,
            &data_dir,
            PathBuf::from(path),
            move_models,
            Arc::new(AtomicBool::new(false)),
            idle,
            |event| {
                let _ = on_event.send(event);
            },
        )
        .await
    })
    .await
    .map_err(|error| AppError::Internal(error.to_string()))?
}
