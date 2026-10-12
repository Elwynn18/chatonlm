//! Sélection primaire sous Linux (data-control sous Wayland, PRIMARY sous
//! X11), lue par arboard sans focus.

use super::{SelectionError, selection_error};

/// Sélection primaire du bureau (data-control sous Wayland, PRIMARY sous
/// X11). Rien de sélectionné, ou pas sous forme de texte : vide.
pub(super) fn read_primary() -> Result<String, SelectionError> {
    use arboard::{GetExtLinux, LinuxClipboardKind};
    let mut clipboard = arboard::Clipboard::new().map_err(selection_error)?;
    match clipboard
        .get()
        .clipboard(LinuxClipboardKind::Primary)
        .text()
    {
        Ok(text) => Ok(text),
        Err(arboard::Error::ContentNotAvailable) => Ok(String::new()),
        Err(error) => Err(selection_error(error)),
    }
}
