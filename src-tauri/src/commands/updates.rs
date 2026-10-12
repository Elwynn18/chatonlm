//! Commandes des mises à jour de ChatonLM (étape 18c, carte « Updates » de
//! Settings › General). L'état suit aussi par l'événement `update-status`.

use tauri::AppHandle;

use crate::error::AppResult;
use crate::updater::{self, AppVersions, UpdateStatus};

/// État des mises à jour et réglage « Auto-download updates ».
#[tauri::command]
pub async fn update_status(app: AppHandle) -> AppResult<UpdateStatus> {
    updater::status(&app)
}

/// Vérifie tout de suite (« Check for updates », Retry) ; jamais deux
/// vérifications à la fois : pendant l'une, attend sa fin. Une mise à jour
/// annoncée se télécharge ensuite en fond si l'auto-download est activé.
#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> AppResult<UpdateStatus> {
    updater::check(&app).await
}

/// Télécharge en fond la mise à jour annoncée (auto-download coupé) ;
/// erreur s'il n'y en a pas.
#[tauri::command]
pub async fn download_update(app: AppHandle) -> AppResult<UpdateStatus> {
    updater::download(&app)
}

/// « Restart to update » : arrête les services, installe la mise à jour
/// prête (boîte de mot de passe du système pour un .deb), puis relance
/// ChatonLM. Erreur sans rien arrêter si elle n'est pas prête ou si le
/// dossier de l'AppImage n'est pas inscriptible.
#[tauri::command]
pub async fn restart_to_update(app: AppHandle) -> AppResult<()> {
    updater::restart_to_update(&app).await
}

/// Réglage « Auto-download updates ».
#[tauri::command]
pub async fn set_auto_download(app: AppHandle, enabled: bool) -> AppResult<UpdateStatus> {
    updater::set_auto_download(&app, enabled).await
}

/// Versions installées : ChatonLM, et l'Ollama intégré s'il est livré.
#[tauri::command]
pub async fn app_versions(app: AppHandle) -> AppResult<AppVersions> {
    Ok(updater::versions(&app))
}
