//! Image du presse-papiers, pour le collage d'une capture d'écran : WebKitGTK
//! ne la donne pas à la page (évènement `paste` sans type ni fichier), alors
//! que la fenêtre, qui a le focus, peut la lire dans le presse-papiers GTK.
//!
//! Sous Windows, WebView2 donne l'image collée à la page (`clipboardData`) :
//! la page ne demande jamais ce repli, qui ne rend alors rien.

use tauri::{AppHandle, Runtime};

#[cfg(target_os = "linux")]
use crate::error::AppError;
use crate::error::AppResult;

/// Type demandé d'abord : l'image telle que copiée, sans réencodage.
#[cfg(target_os = "linux")]
const PNG: &str = "image/png";

/// Image du presse-papiers en PNG : telle quelle si elle est offerte en PNG,
/// sinon toute image lisible par GdkPixbuf (JPEG, BMP...), réécrite en PNG ;
/// `None` s'il n'y a pas d'image.
#[cfg(target_os = "linux")]
pub async fn read_png<R: Runtime>(app: &AppHandle<R>) -> AppResult<Option<Vec<u8>>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    // Le presse-papiers GTK ne se lit que depuis le fil principal ; la
    // réponse de l'application qui a copié arrive par rappel, sans bloquer.
    app.run_on_main_thread(move || {
        let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
        clipboard.request_contents(&gtk::gdk::Atom::intern(PNG), move |clipboard, data| {
            let png = data.data();
            if !png.is_empty() {
                let _ = sender.send(Some(png));
                return;
            }
            clipboard.request_image(move |_, pixbuf| {
                let _ =
                    sender.send(pixbuf.and_then(|image| image.save_to_bufferv("png", &[]).ok()));
            });
        });
    })
    .map_err(|error| AppError::Internal(error.to_string()))?;
    // Rappel jamais fait (boucle d'évènements arrêtée) : pas d'image.
    Ok(receiver.await.ok().flatten())
}

/// Hors de Linux : la webview donne elle-même l'image collée à la page.
#[cfg(not(target_os = "linux"))]
pub async fn read_png<R: Runtime>(_app: &AppHandle<R>) -> AppResult<Option<Vec<u8>>> {
    Ok(None)
}
