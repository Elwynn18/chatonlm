//! « Erase my data » (Settings › General, propre à ChatonLM) : voir le
//! module `erase`.

use tauri::AppHandle;

use crate::erase::EraseReport;
use crate::error::AppResult;

/// Efface les données de l'utilisateur (jamais les modèles) ; l'app reste
/// ouverte. Erreur : rien n'a été effacé.
#[tauri::command]
pub async fn erase_user_data(app: AppHandle) -> AppResult<EraseReport> {
    crate::erase::erase(&app).await
}
