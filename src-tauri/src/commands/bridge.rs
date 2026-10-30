//! Commandes du lien VS Code (fonction propre à ChatonLM : pont local vers
//! l'extension ChatonLM pour VS Code ; contrat
//! `.tools/research/87-lot1a-contract.md`). Le secret d'un éditeur appairé ne
//! passe jamais par ici.

use tauri::{AppHandle, Manager};

use crate::bridge::{Bridge, BridgeState};
use crate::error::{AppError, AppResult};
use crate::i18n::t;

/// Le service, absent quand la base de l'app est illisible.
fn service(app: &AppHandle) -> AppResult<Bridge> {
    app.try_state::<Bridge>()
        .map(|service| service.inner().clone())
        .ok_or_else(|| AppError::Invalid(t!("bridge.unavailable")))
}

#[tauri::command]
pub async fn bridge_state(app: AppHandle) -> AppResult<BridgeState> {
    service(&app)?.state().await
}

/// Active ou coupe le pont ; coupé, les éditeurs sont déconnectés.
#[tauri::command]
pub async fn bridge_set_enabled(app: AppHandle, enabled: bool) -> AppResult<BridgeState> {
    service(&app)?.set_enabled(enabled).await
}

/// Nouveau code d'appairage, valable dix minutes.
#[tauri::command]
pub async fn bridge_pair(app: AppHandle) -> AppResult<BridgeState> {
    service(&app)?.pair().await
}

#[tauri::command]
pub async fn bridge_cancel_pairing(app: AppHandle) -> AppResult<BridgeState> {
    service(&app)?.cancel_pairing().await
}

/// Oublie un éditeur et ferme ses connexions.
#[tauri::command]
pub async fn bridge_unlink(app: AppHandle, client_id: String) -> AppResult<BridgeState> {
    service(&app)?.unlink(client_id).await
}
