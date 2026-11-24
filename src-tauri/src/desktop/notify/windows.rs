//! Notification simple sous Windows : toast WinRT au nom de l'identifiant
//! de l'app (son icône est celle de l'identifiant, `icon` ne sert pas).

use std::path::Path;

pub(super) async fn show(
    identifier: &str,
    _icon: Option<&Path>,
    title: &str,
    body: &str,
) -> Result<(), String> {
    let (identifier, title, body) = (identifier.to_owned(), title.to_owned(), body.to_owned());
    // Bloquant (WinRT) : hors du fil async.
    tauri::async_runtime::spawn_blocking(move || {
        crate::dictation::notify::show_simple(&identifier, &title, &body)
    })
    .await
    .map_err(|error| error.to_string())?
}
