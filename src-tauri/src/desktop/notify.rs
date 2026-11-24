//! Notification simple du bureau, sans action ni suite (propre à ChatonLM) :
//! fermeture qui masque la fenêtre (« ChatonLM is still running »), fin d'un
//! long travail de fond (Atelier). Titre et corps en texte brut, déjà
//! traduits par l'appelant.
//!
//! - Linux (`linux.rs`) : D-Bus `org.freedesktop.Notifications`, par
//!   notify-rust, au nom de l'app (entrée de bureau, icône écrite par les
//!   notifications de la dictée si elle existe) ;
//! - Windows (`windows.rs`) : toast WinRT au nom de l'identifiant de l'app
//!   (`dictation::notify::show_simple`).
//!
//! Sans serveur de notifications : rien n'est montré, une ligne au journal.

use tauri::{AppHandle, Runtime};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as system;
#[cfg(windows)]
use windows as system;

/// Montre la notification `title` / `body` (attend son envoi au système).
pub(crate) async fn notify<R: Runtime>(app: &AppHandle<R>, title: &str, body: &str) {
    let identifier = app.config().identifier.clone();
    let icon = crate::paths::notification_icon(app)
        .ok()
        .filter(|path| path.is_file());
    if let Err(error) = system::show(&identifier, icon.as_deref(), title, body).await {
        // i18n-ignore : journal.
        crate::app_log!("chatonlm: cannot show a notification: {error}");
    }
}
