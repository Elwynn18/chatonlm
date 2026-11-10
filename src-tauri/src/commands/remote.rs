//! Fournisseurs des modèles distants (Settings › Models). La clé d'API entre
//! par `save_remote_provider` et n'est jamais rendue : la page n'en reçoit
//! que l'indice.

use tauri::State;

use crate::error::AppResult;
use crate::llm::Access;
use crate::llm::remote::providers::{self, RemoteCheck, RemoteProvider, RemoteProviderInput};

#[tauri::command]
pub async fn remote_providers(access: State<'_, Access>) -> AppResult<Vec<RemoteProvider>> {
    providers::list(access.store(), access.data_dir()).await
}

/// Ajoute ou change un fournisseur ; les listes gardées sont oubliées.
#[tauri::command]
pub async fn save_remote_provider(
    access: State<'_, Access>,
    input: RemoteProviderInput,
) -> AppResult<RemoteProvider> {
    let saved = providers::save(access.store(), access.data_dir(), input).await;
    access.remote().invalidate();
    saved
}

/// Retire un fournisseur et sa clé.
#[tauri::command]
pub async fn delete_remote_provider(access: State<'_, Access>, id: String) -> AppResult<()> {
    let deleted = providers::delete(access.store(), access.data_dir(), &id).await;
    access.remote().invalidate();
    deleted
}

/// Vérifie l'adresse et la clé d'un fournisseur ; sa liste gardée est
/// oubliée (elle sera redemandée).
#[tauri::command]
pub async fn check_remote_provider(
    access: State<'_, Access>,
    id: String,
) -> AppResult<RemoteCheck> {
    access.remote().invalidate();
    providers::check(access.store(), access.data_dir(), &id).await
}
