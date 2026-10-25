//! Commandes de l'intégration au bureau (étape 18a) : entrée du menu des
//! applications d'une AppImage et croix de la fenêtre (lignes de Settings ›
//! General).

use serde::Serialize;
use tauri::{AppHandle, State};
use ts_rs::TS;

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

/// Ligne « Close button » de Settings › General.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CloseButtonStatus {
    /// Zone de notification présente : sans elle, la croix quitte toujours.
    pub tray_available: bool,
    /// Réglage : la croix masque la fenêtre (vrai, par défaut) ou quitte.
    pub close_to_tray: bool,
}

fn close_button(app: &AppHandle, close_to_tray: bool) -> CloseButtonStatus {
    CloseButtonStatus {
        tray_available: crate::desktop::state(app).tray_available(),
        close_to_tray,
    }
}

/// Réglage de la croix enregistré, et la zone de notification.
#[tauri::command]
pub async fn close_button_status(
    app: AppHandle,
    store: State<'_, Store>,
) -> AppResult<CloseButtonStatus> {
    let enabled = store
        .call(|connection| crate::store::desktop::close_to_tray(connection))
        .await?;
    Ok(close_button(&app, enabled))
}

/// Enregistre le choix ; il vaut dès la prochaine fermeture.
#[tauri::command]
pub async fn set_close_to_tray(
    app: AppHandle,
    store: State<'_, Store>,
    enabled: bool,
) -> AppResult<CloseButtonStatus> {
    store
        .call(move |connection| crate::store::desktop::set_close_to_tray(connection, enabled))
        .await?;
    crate::desktop::state(&app).set_close_to_tray(enabled);
    Ok(close_button(&app, enabled))
}
