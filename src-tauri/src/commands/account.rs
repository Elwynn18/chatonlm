//! Compte ollama.com du serveur Ollama (carte « Ollama account » des réglages,
//! bandeau de la recherche web) et état de son cloud.

use tauri::State;

use super::models::server_url;
use super::system::open_external;
use crate::error::AppResult;
use crate::ollama::account::{self, AccountStatus, CloudStatus};
use crate::store::Store;

/// Compte relié au serveur, à son adresse effective.
#[tauri::command]
pub async fn account_status(store: State<'_, Store>) -> AppResult<AccountStatus> {
    account::status(&server_url(&store).await?).await
}

/// Ouvre la page de connexion d'ollama.com dans le navigateur (création de
/// compte comprise) ; rien si un compte est déjà relié.
#[tauri::command]
pub async fn account_sign_in(store: State<'_, Store>) -> AppResult<()> {
    match account::sign_in_url(&server_url(&store).await?).await? {
        Some(url) => open_external(url).await,
        None => Ok(()),
    }
}

/// Délie le compte du serveur.
#[tauri::command]
pub async fn account_sign_out(store: State<'_, Store>) -> AppResult<()> {
    account::sign_out(&server_url(&store).await?).await
}

/// État du cloud du serveur ; `null` s'il est inconnu.
#[tauri::command]
pub async fn cloud_status(store: State<'_, Store>) -> AppResult<Option<CloudStatus>> {
    Ok(account::cloud(&server_url(&store).await?).await)
}
