// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// pick_models_folder translated to Rust from Ollama
// (https://github.com/ollama/ollama), file app/cmd/app/webview.go
// (selectModelsDirectory), commit 42e911bc3d05 (dialog title, null when
// cancelled).
// Modified for ChatonLM: the folder belongs to Ollama's systemd service, not to
// the app: it is changed through polkit (one prompt) with a confirmation, an
// optional move of the models, progress and a full restore on failure,
// instead of restarting the app's own server; elsewhere (manual launch, user
// service, no pkexec, a service that runs as root) the commands to run are
// given; only the folder handed over by the dialog can then be checked and
// used, and an existing folder must belong to the user or to the service;
// hidden folders are not shown by the dialog; model downloads can't start
// while the location changes. get_ollama_home and fix_ollama_home (Settings >
// General, "Fix") are ChatonLM's own: Ollama's .ollama folder hidden from its
// service by ProtectHome=, given back through polkit. The built-in Ollama's
// folder is changed by ChatonLM itself (change_builtin_models_dir), without
// polkit, with the same steps, progress and restore on failure.

//! Settings > Model location (voir `ollama::models_dir` ; Ollama intégré :
//! `ollama::models_dir::builtin`), et dossier `.ollama` du service que
//! `ProtectHome=` lui cache (`ollama::models_dir::home`).

use parking_lot::Mutex;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

use crate::chat::ChatEngine;
use crate::error::{AppError, AppResult};
use crate::hub::downloads::HubDownloads;
use crate::i18n::t;
#[cfg(target_os = "linux")]
use crate::ollama::models_dir::home;
use crate::ollama::models_dir::{
    self, Detection, FolderCheck, HiddenHome, HomeFixOutcome, ModelsLocation, MoveEvent,
    MoveOutcome, OperationLock, builtin,
};
use crate::ollama::target;
use crate::store::Store;
use crate::store::builtin_ollama;

/// Une seule opération à la fois (dossier des modèles ou dossier `.ollama`),
/// même si la page est rechargée pendant.
static OPERATION: OperationLock = OperationLock::new();

/// Dernier dossier remis par le sélecteur : seul celui-là peut être vérifié
/// puis utilisé (la page ne peut pas en désigner un autre).
static PICKED: Mutex<Option<String>> = Mutex::new(None);

/// Un changement de dossier est en cours (de la confirmation à la fin du
/// script) : `hub_download` refuse alors un nouveau téléchargement
/// (`HubDownloads`, sous son verrou). Avec la vérification de
/// [`downloads_active`] faite après la prise du verrou, l'un des deux voit
/// toujours l'autre.
pub fn location_changing() -> bool {
    OPERATION.is_held()
}

/// Téléchargements de modèles en cours (file du ModelHub) : le redémarrage
/// d'Ollama les couperait.
pub fn downloads_active(app: &AppHandle) -> bool {
    app.try_state::<HubDownloads>()
        .is_some_and(|downloads| downloads.any_active())
}

/// Où Ollama range ses modèles, pour le serveur de la cible (Ollama intégré
/// compris).
#[tauri::command]
pub async fn get_models_location(store: State<'_, Store>) -> AppResult<ModelsLocation> {
    Ok(detection(&store).await?.location)
}

/// Sélecteur de dossier natif « Select Model Directory », au-dessus de la
/// fenêtre ; `None` si l'utilisateur annule.
#[tauri::command]
pub async fn pick_models_folder(window: WebviewWindow) -> AppResult<Option<String>> {
    // Essais de l'app réelle : le dossier « choisi » est lu dans le fichier
    // désigné (première ligne ; vide : annulé), relu à chaque appel.
    #[cfg(feature = "e2e")]
    if let Some(file) = std::env::var_os("CHATONLM_E2E_PICK_FOLDER") {
        let picked = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| text.lines().next().map(str::to_owned))
            .filter(|line| !line.is_empty());
        return Ok(remember(picked));
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(t!("dialogs.selectModelDirectory"))
        .pick_folder(move |path| {
            let _ = sender.send(path);
        });
    // Rappel jamais fait (boucle d'événements arrêtée) : comme une annulation.
    let picked = receiver
        .await
        .ok()
        .flatten()
        .and_then(|path| path.into_path().ok())
        // Chemin non UTF-8 : refusé ensuite (BadCharacters).
        .map(|path| path.to_string_lossy().into_owned());
    Ok(remember(picked))
}

fn remember(picked: Option<String>) -> Option<String> {
    if let Some(path) = &picked {
        *PICKED.lock() = Some(path.clone());
    }
    picked
}

fn ensure_picked(path: &str) -> AppResult<()> {
    if PICKED.lock().as_deref() == Some(path) {
        Ok(())
    } else {
        Err(AppError::Invalid(t!("modelLocation.browseFirst")))
    }
}

/// Vérifie le dossier choisi, sans droits ni invite : refus éventuel,
/// modèles à déplacer, place, commandes à copier.
#[tauri::command]
pub async fn check_models_folder(
    app: AppHandle,
    store: State<'_, Store>,
    engine: State<'_, ChatEngine>,
    path: String,
) -> AppResult<FolderCheck> {
    ensure_picked(&path)?;
    let detection = detection(&store).await?;
    let mut check = check(&detection, path).await?;
    if check.problem.is_none() && check.old.is_some() {
        check.models = match &detection.location {
            // Lus sur le disque : l'intégré peut être arrêté ; un dossier
            // vide compte zéro modèle (pas « inconnu »).
            ModelsLocation::Builtin { dir, .. } => {
                let dir = std::path::PathBuf::from(dir);
                tauri::async_runtime::spawn_blocking(move || builtin::folder_summary(&dir))
                    .await
                    .ok()
            }
            _ => models_dir::models_summary(&server_url(&store).await?).await,
        };
    }
    check.replies_running = engine.is_busy();
    check.downloads_running = downloads_active(&app);
    Ok(check)
}

/// Change le dossier des modèles du service système (une invite polkit) ;
/// la progression arrive par `on_event`. Refusé si une autre opération ou
/// des téléchargements sont en cours, ou si le dossier n'est pas celui remis
/// par le sélecteur. Sous Windows, toujours refusé : il n'y a pas de service,
/// et l'Ollama de la machine n'est jamais changé par ChatonLM (guidage seul).
#[tauri::command]
pub async fn change_models_location(
    app: AppHandle,
    store: State<'_, Store>,
    path: String,
    move_models: bool,
    on_event: Channel<MoveEvent>,
) -> AppResult<MoveOutcome> {
    change_system_location(app, store, path, move_models, on_event).await
}

#[cfg(not(target_os = "linux"))]
async fn change_system_location(
    _app: AppHandle,
    _store: State<'_, Store>,
    _path: String,
    _move_models: bool,
    _on_event: Channel<MoveEvent>,
) -> AppResult<MoveOutcome> {
    Err(AppError::Invalid(t!("modelLocation.systemServiceOnly")))
}

#[cfg(target_os = "linux")]
async fn change_system_location(
    app: AppHandle,
    store: State<'_, Store>,
    path: String,
    move_models: bool,
    on_event: Channel<MoveEvent>,
) -> AppResult<MoveOutcome> {
    let guard = OPERATION
        .try_acquire()
        .ok_or_else(|| AppError::Invalid(t!("modelLocation.busy")))?;
    if downloads_active(&app) {
        return Err(AppError::Invalid(t!("modelLocation.downloadsRunning")));
    }
    ensure_picked(&path)?;
    let url = server_url(&store).await?;
    let detection = detection(&store).await?;
    let ModelsLocation::SystemService {
        can_elevate: true,
        ref user,
        ..
    } = detection.location
    else {
        return Err(AppError::Invalid(t!("modelLocation.systemServiceOnly")));
    };
    // Service en root : un dossier choisi par un compte sans privilège serait
    // lu, écrit et élagué par root ; les commandes sont données à la place.
    if user == "root" {
        return Err(AppError::Invalid(t!("modelLocation.root")));
    }
    let pkexec =
        models_dir::find_pkexec().ok_or_else(|| AppError::Invalid(t!("modelLocation.noPkexec")))?;
    let mut check = check(&detection, path).await?;
    if let Some(problem) = check.problem {
        return Err(AppError::Invalid(t!(
            "modelLocation.unusableFolder",
            problem = format!("{problem:?}")
        )));
    }
    let move_models = move_models && check.old.is_some();
    if move_models && !check.same_drive {
        check.models = models_dir::models_summary(&url).await;
        if let Some(reason) = models_dir::space_shortage(&check, true) {
            return Ok(MoveOutcome::Failed {
                reason,
                restored: false,
                detail: String::new(),
            });
        }
    }
    let request = models_dir::Request {
        pkexec,
        shell: models_dir::SHELL.into(),
        move_models,
        old: check.old.clone(),
        new: check.path.clone(),
        envs: Vec::new(),
    };
    // Tâche à part : si la commande est abandonnée (page rechargée), le script
    // va au bout et le verrou reste pris jusque-là.
    let task = tauri::async_runtime::spawn(async move {
        let _guard = guard;
        let engine_app = app.clone();
        models_dir::run(
            request,
            |event| {
                let _ = on_event.send(event);
            },
            // Ollama va s'arrêter : les réponses en cours s'arrêtent proprement.
            move || engine_app.state::<ChatEngine>().cancel_all(),
        )
        .await
    });
    task.await
        .map_err(|error| AppError::Internal(format!("model location task failed: {error}")))
}

/// Dossier `.ollama` du service système, que `ProtectHome=` lui cache, pour
/// l'adresse effective du serveur ; `None` : rien à signaler (dossier
/// visible, ou Ollama lancé autrement que par ce service).
#[tauri::command]
pub async fn get_ollama_home(store: State<'_, Store>) -> AppResult<Option<HiddenHome>> {
    Ok(detection(&store).await?.hidden_home)
}

/// Rend au service système son dossier `.ollama` (une invite polkit, puis
/// redémarrage). Refusé si une autre opération ou des téléchargements sont en
/// cours ; `Done { changed: false }` si le dossier n'est plus caché. Sous
/// Windows, rien n'est jamais caché : toujours refusé.
#[tauri::command]
pub async fn fix_ollama_home(app: AppHandle, store: State<'_, Store>) -> AppResult<HomeFixOutcome> {
    fix_service_home(app, store).await
}

#[cfg(not(target_os = "linux"))]
async fn fix_service_home(_app: AppHandle, _store: State<'_, Store>) -> AppResult<HomeFixOutcome> {
    Err(AppError::Invalid(t!("modelLocation.systemServiceOnly")))
}

#[cfg(target_os = "linux")]
async fn fix_service_home(app: AppHandle, store: State<'_, Store>) -> AppResult<HomeFixOutcome> {
    let guard = OPERATION
        .try_acquire()
        .ok_or_else(|| AppError::Invalid(t!("ollamaHome.busy")))?;
    if downloads_active(&app) {
        return Err(AppError::Invalid(t!("modelLocation.downloadsRunning")));
    }
    let Some(hidden) = detection(&store).await?.hidden_home else {
        return Ok(HomeFixOutcome::Done {
            changed: false,
            restarted: false,
        });
    };
    // Comme le script : un service root n'est pas changé (un lien glissé à
    // la place de .ollama lui donnerait n'importe quel dossier en écriture).
    if hidden.user == "root" {
        return Err(AppError::Invalid(t!("ollamaHome.root")));
    }
    let pkexec =
        models_dir::find_pkexec().ok_or_else(|| AppError::Invalid(t!("modelLocation.noPkexec")))?;
    let request = home::FixRequest {
        pkexec,
        shell: models_dir::SHELL.into(),
        dir: hidden.dir,
        envs: Vec::new(),
    };
    // Tâche à part : si la commande est abandonnée (page rechargée), le script
    // va au bout et le verrou reste pris jusque-là.
    let task = tauri::async_runtime::spawn(async move {
        let _guard = guard;
        // Ollama va redémarrer : les réponses en cours s'arrêtent proprement.
        home::fix(request, move || app.state::<ChatEngine>().cancel_all()).await
    });
    task.await
        .map_err(|error| AppError::Internal(format!("Ollama folder task failed: {error}")))
}

/// Change le dossier des modèles de l'Ollama intégré, sans droits ni invite :
/// arrêt de l'intégré, déplacement des modèles s'il est demandé (renommage
/// sur le même disque, sinon copie avec progression par `on_event`),
/// réglage, redémarrage, effacement des anciennes copies ; tout est remis en
/// place en cas d'échec. Mêmes refus que `change_models_location`.
#[tauri::command]
pub async fn change_builtin_models_dir(
    app: AppHandle,
    store: State<'_, Store>,
    path: String,
    move_models: bool,
    on_event: Channel<MoveEvent>,
) -> AppResult<MoveOutcome> {
    let guard = OPERATION
        .try_acquire()
        .ok_or_else(|| AppError::Invalid(t!("modelLocation.busy")))?;
    if downloads_active(&app) {
        return Err(AppError::Invalid(t!("modelLocation.downloadsRunning")));
    }
    ensure_picked(&path)?;
    let targeting = target::current()
        // i18n-ignore : état installé au démarrage, toujours présent.
        .ok_or_else(|| AppError::Internal("no Ollama target".into()))?;
    if !targeting.builtin().available() {
        return Err(AppError::Invalid(t!("builtinOllama.unavailable")));
    }
    let (old, is_default) = targeting.saved_models_dir(&store).await?;
    let check = check(&builtin::detection(&old, is_default), path).await?;
    if let Some(problem) = check.problem {
        return Err(AppError::Invalid(t!(
            "modelLocation.unusableFolder",
            problem = format!("{problem:?}")
        )));
    }
    let start_after = targeting.builtin_is_target(&store).await?;
    let default_dir = targeting.builtin().config().default_models_dir();
    let store = store.inner().clone();
    // Tâche à part : si la commande est abandonnée (page rechargée), le
    // changement va au bout et le verrou reste pris jusque-là.
    let task = tauri::async_runtime::spawn(async move {
        let _guard = guard;
        let save = |new: std::path::PathBuf| async move {
            let value =
                (!same_folder(&new, &default_dir)).then(|| new.to_string_lossy().into_owned());
            store
                .call(move |connection| {
                    builtin_ollama::set_models_dir(connection, value.as_deref())
                })
                .await
                .map_err(|error| error.to_string())
        };
        builtin::change(
            targeting.builtin(),
            builtin::Change {
                old,
                check,
                move_models,
            },
            start_after,
            |event| {
                let _ = on_event.send(event);
            },
            // L'intégré va s'arrêter : chat, cours et corrections en cours
            // s'arrêtent proprement (une partie de cours reste à reprendre).
            || targeting.stop_work(),
            save,
        )
        .await
    });
    task.await
        .map_err(|error| AppError::Internal(format!("model location task failed: {error}")))
}

/// Même dossier (formes canoniques, si elles existent).
fn same_folder(a: &std::path::Path, b: &std::path::Path) -> bool {
    let canonical =
        |path: &std::path::Path| std::fs::canonicalize(path).unwrap_or(path.to_path_buf());
    canonical(a) == canonical(b)
}

async fn check(detection: &Detection, path: String) -> AppResult<FolderCheck> {
    let detection = detection.clone();
    tauri::async_runtime::spawn_blocking(move || models_dir::check_folder(&path, &detection))
        .await
        .map_err(|error| AppError::Internal(format!("folder check failed: {error}")))
}

/// Adresse du serveur de la cible.
async fn server_url(store: &Store) -> AppResult<String> {
    super::models::server_url(store).await
}

/// Où sont les modèles : dossier de l'Ollama intégré quand il est la cible,
/// sinon détection du serveur de la machine.
async fn detection(store: &Store) -> AppResult<Detection> {
    if let Some(targeting) = target::current()
        && targeting.builtin_is_target(store).await?
    {
        let (dir, is_default) = targeting.saved_models_dir(store).await?;
        return Ok(builtin::detection(&dir, is_default));
    }
    Ok(models_dir::detect(&server_url(store).await?).await)
}
