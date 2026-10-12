//! Serveur Ollama : écran d'accueil, choix entre l'Ollama de la machine et
//! l'Ollama intégré (Settings › General), « Start Ollama ».

use std::sync::Arc;

use tauri::State;

use crate::error::{AppError, AppResult};
use crate::ollama::detect::ServerStatus;
use crate::ollama::target::{self, OllamaRuntime, ServiceStart, Targeting};
use crate::store::Store;
use crate::store::builtin_ollama::OllamaSource;

fn targeting() -> AppResult<Arc<Targeting>> {
    // i18n-ignore : état installé au démarrage, toujours présent.
    target::current().ok_or_else(|| AppError::Internal("no Ollama target".into()))
}

/// Vérifie le serveur de la cible : Ollama intégré (démarré au besoin), ou
/// adresse réglée, sinon `OLLAMA_HOST`, sinon http://localhost:11434 ; rien
/// d'installé à l'adresse par défaut : l'intégré démarre seul.
#[tauri::command]
pub async fn server_status(store: State<'_, Store>) -> AppResult<ServerStatus> {
    targeting()?.status(&store).await
}

/// Quel Ollama sert, et l'Ollama intégré (Settings › General).
#[tauri::command]
pub async fn ollama_runtime(store: State<'_, Store>) -> AppResult<OllamaRuntime> {
    targeting()?.runtime(&store).await
}

/// « Use the built-in Ollama » (`builtin`, mémorisé) ou retour à l'Ollama de
/// la machine (`auto`).
#[tauri::command]
pub async fn set_ollama_source(
    store: State<'_, Store>,
    source: OllamaSource,
) -> AppResult<OllamaRuntime> {
    targeting()?.set_source(&store, source).await
}

/// Retry après un échec de l'Ollama intégré : arrêt puis relance.
#[tauri::command]
pub async fn restart_builtin_ollama(store: State<'_, Store>) -> AppResult<()> {
    targeting()?.restart_builtin(&store).await
}

/// « Start Ollama » : démarre le service systemd d'Ollama (invite polkit pour
/// le service du système).
#[tauri::command]
pub async fn start_ollama_service() -> AppResult<ServiceStart> {
    Ok(target::start_service().await)
}

/// Ouvre le dossier des modèles de l'Ollama intégré (celui qui sert : le
/// dossier par défaut, créé au besoin, remplace un dossier choisi absent).
#[tauri::command]
pub async fn open_builtin_folder(store: State<'_, Store>) -> AppResult<()> {
    let (dir, is_default) = targeting()?.saved_models_dir(&store).await?;
    if is_default {
        std::fs::create_dir_all(&dir)?;
    }
    super::system::open_path(&dir).await
}

/// Ouvre le journal de l'Ollama intégré (son dossier s'il n'existe pas
/// encore).
#[tauri::command]
pub async fn open_builtin_log() -> AppResult<()> {
    let targeting = targeting()?;
    let log = targeting.builtin().config().log.clone();
    if log.exists() {
        return super::system::open_path(&log).await;
    }
    let dir = log
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or(log);
    std::fs::create_dir_all(&dir)?;
    super::system::open_path(&dir).await
}
