//! Commandes d'Easy Link (fonction propre à ChatonLM : bot Telegram ou
//! Discord relié au modèle de l'app ; contrat
//! `.tools/research/80-easy-link-contract.md`). Le jeton d'un bot entre par
//! `easy_link_connect` et ne ressort jamais.

use tauri::AppHandle;
use tauri::Manager;

use crate::easy_link::{EasyLink, EasyLinkState};
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::easy_link::Platform;

/// Le service, absent quand la base de l'app est illisible.
fn service(app: &AppHandle) -> AppResult<EasyLink> {
    app.try_state::<EasyLink>()
        .map(|service| service.inner().clone())
        .ok_or_else(|| AppError::Invalid(t!("easyLink.unavailable")))
}

#[tauri::command]
pub async fn easy_link_state(app: AppHandle) -> AppResult<EasyLinkState> {
    service(&app)?.state().await
}

/// Vérifie le jeton auprès de la messagerie, l'enregistre et active le bot.
#[tauri::command]
pub async fn easy_link_connect(
    app: AppHandle,
    platform: Platform,
    token: String,
) -> AppResult<EasyLinkState> {
    service(&app)?.connect(platform, &token).await
}

/// Oublie le jeton et délie les comptes de cette messagerie.
#[tauri::command]
pub async fn easy_link_disconnect(app: AppHandle, platform: Platform) -> AppResult<EasyLinkState> {
    service(&app)?.disconnect(platform).await
}

#[tauri::command]
pub async fn easy_link_set_enabled(
    app: AppHandle,
    platform: Platform,
    enabled: bool,
) -> AppResult<EasyLinkState> {
    service(&app)?.set_enabled(platform, enabled).await
}

/// Nouveau code d'appairage (dix minutes, un seul à la fois).
#[tauri::command]
pub async fn easy_link_pair(app: AppHandle, platform: Platform) -> AppResult<EasyLinkState> {
    service(&app)?.pair(platform).await
}

#[tauri::command]
pub async fn easy_link_cancel_pairing(app: AppHandle) -> AppResult<EasyLinkState> {
    service(&app)?.cancel_pairing().await
}

#[tauri::command]
pub async fn easy_link_unlink(app: AppHandle, account_id: i64) -> AppResult<EasyLinkState> {
    service(&app)?.unlink(account_id).await
}

/// « Let VS Code notify me through Easy Link » (lien VS Code, coupé par
/// défaut).
#[tauri::command]
pub async fn easy_link_set_vscode_notify(
    app: AppHandle,
    enabled: bool,
) -> AppResult<EasyLinkState> {
    service(&app)?.set_vscode_notify(enabled).await
}
