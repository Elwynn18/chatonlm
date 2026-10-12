//! Commandes de l'intégration au bureau (étape 18a) : entrée du menu des
//! applications d'une AppImage (ligne de Settings › General).

use tauri::{AppHandle, State};

use crate::desktop::menu_entry::{self, MenuEntryStatus};
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::Store;

/// Proposée seulement sous AppImage (`available`) ; réglage enregistré.
#[tauri::command]
pub async fn menu_entry_status(
    app: AppHandle,
    store: State<'_, Store>,
) -> AppResult<MenuEntryStatus> {
    let enabled = store
        .call(|connection| crate::store::desktop::menu_entry(connection))
        .await?;
    Ok(menu_entry::status(&app, enabled))
}

/// Enregistre le choix, puis écrit l'entrée et l'icône, ou les retire.
/// Le réglage reste enregistré si le fichier n'a pas pu changer : un retrait
/// n'est plus jamais réécrit, une entrée voulue l'est au prochain démarrage.
#[tauri::command]
pub async fn set_menu_entry(
    app: AppHandle,
    store: State<'_, Store>,
    enabled: bool,
) -> AppResult<MenuEntryStatus> {
    store
        .call(move |connection| crate::store::desktop::set_menu_entry(connection, enabled))
        .await?;
    menu_entry::apply(&app, enabled).await.map_err(|error| {
        AppError::System(if enabled {
            t!("menuEntry.writeFailed", error = error)
        } else {
            t!("menuEntry.removeFailed", error = error)
        })
    })
}
