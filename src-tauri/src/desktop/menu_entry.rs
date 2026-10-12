//! Entrée du menu des applications (étape 18a) : ligne « Applications menu »
//! de Settings › General.
//!
//! - Linux (`linux.rs`) : une AppImage n'installe rien, ChatonLM écrit donc
//!   lui-même son fichier `.desktop` et son icône.
//! - Windows : sans objet, l'installateur crée le raccourci du menu Démarrer.
//!   La ligne n'est jamais proposée (`available` faux) et rien n'est écrit.

#[cfg(target_os = "linux")]
mod linux;

use serde::Serialize;
use ts_rs::TS;

#[cfg(target_os = "linux")]
pub use linux::{apply, at_startup, status};

/// Ligne de Settings › General.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MenuEntryStatus {
    /// ChatonLM tourne depuis une AppImage : la ligne est proposée.
    pub available: bool,
    /// Entrée voulue (réglage, vrai par défaut) ; faux une fois retirée.
    pub enabled: bool,
}

/// Démarrage : rien à écrire hors de Linux.
#[cfg(not(target_os = "linux"))]
pub async fn at_startup(_app: &tauri::AppHandle, _enabled: bool) {}

/// État de la ligne de Settings › General : jamais proposée hors de Linux.
#[cfg(not(target_os = "linux"))]
pub fn status(_app: &tauri::AppHandle, enabled: bool) -> MenuEntryStatus {
    MenuEntryStatus {
        available: false,
        enabled,
    }
}

/// Réglage changé : gardé, mais rien à écrire hors de Linux.
#[cfg(not(target_os = "linux"))]
pub async fn apply(app: &tauri::AppHandle, enabled: bool) -> Result<MenuEntryStatus, String> {
    Ok(status(app, enabled))
}
