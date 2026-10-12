//! Texte sélectionné sous Windows : Ctrl+C simulé (`SendInput`) dans
//! l'application au premier plan, lu dans le presse-papiers, dont l'ancien
//! texte est ensuite remis (voir `selection.rs`).
//!
//! Limites : `SendInput` n'atteint pas une fenêtre lancée en administrateur
//! (rien n'est alors copié : « No text selected ») ; un ancien contenu qui
//! n'est pas du texte (image, fichiers) n'est pas remis ; dans un terminal,
//! Ctrl+C interrompt le programme en cours au lieu de copier.

use std::thread;
use std::time::Instant;

use arboard::SetExtWindows;
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
};

use super::selection::{
    COPY_WAIT, KeyStroke, MODIFIERS, RELEASE_WAIT, STEP, captured, copy_chord, restores,
};
use super::{SelectionError, selection_error};
use crate::i18n::t;

/// Essais d'un accès au presse-papiers qu'une autre application tient.
const CLIPBOARD_TRIES: usize = 10;

fn sequence() -> u32 {
    // SAFETY : sans argument ni état ; rend 0 si la session n'a pas accès au
    // presse-papiers.
    unsafe { GetClipboardSequenceNumber() }
}

fn held(key: u16) -> bool {
    // SAFETY : lecture de l'état d'une touche ; bit de poids fort = tenue.
    (unsafe { GetAsyncKeyState(i32::from(key)) } as u16) & 0x8000 != 0
}

/// Attend que les modificateurs du raccourci soient relâchés ; faux s'ils
/// sont encore tenus au bout de `RELEASE_WAIT`.
fn wait_released() -> bool {
    let start = Instant::now();
    while MODIFIERS.iter().any(|key| held(*key)) {
        if start.elapsed() >= RELEASE_WAIT {
            return false;
        }
        thread::sleep(STEP);
    }
    true
}

/// Envoie les touches à l'application au premier plan ; faux si Windows n'en
/// a pris aucune.
fn send(strokes: &[KeyStroke]) -> bool {
    let inputs: Vec<INPUT> = strokes
        .iter()
        .map(|stroke| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(stroke.key),
                    wScan: 0,
                    dwFlags: if stroke.down {
                        KEYBD_EVENT_FLAGS(0)
                    } else {
                        KEYEVENTF_KEYUP
                    },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        })
        .collect();
    // SAFETY : tableau d'entrées valides, taille de la structure donnée.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    sent as usize == inputs.len()
}

/// Numéro de séquence du presse-papiers une fois changé (ou `before` au bout
/// de `COPY_WAIT`).
fn wait_changed(before: u32) -> u32 {
    let start = Instant::now();
    loop {
        let now = sequence();
        if now != before || start.elapsed() >= COPY_WAIT {
            return now;
        }
        thread::sleep(STEP);
    }
}

/// Refait `access` tant qu'une autre application tient le presse-papiers.
fn retried<T>(mut access: impl FnMut() -> Result<T, arboard::Error>) -> Result<T, arboard::Error> {
    let mut tries = 0;
    loop {
        match access() {
            Err(arboard::Error::ClipboardOccupied) if tries < CLIPBOARD_TRIES => {
                tries += 1;
                thread::sleep(STEP);
            }
            result => return result,
        }
    }
}

/// Texte sélectionné dans l'application au premier plan ; vide s'il n'y en a
/// pas. Bloquant (fil de lecture de la sélection).
pub(super) fn read_selection() -> Result<String, SelectionError> {
    // Une lecture à la fois : deux lectures croisées (appuis rapprochés du
    // raccourci) prendraient la sélection copiée par l'autre pour l'ancien
    // contenu du presse-papiers, et la remettraient à sa place.
    static READING: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
    let _reading = READING.lock();
    if !wait_released() {
        return Err(SelectionError::Other(t!("proofread.selection.keysHeld")));
    }
    let mut clipboard = arboard::Clipboard::new().map_err(selection_error)?;
    let previous = retried(|| clipboard.get_text()).ok();
    let before = sequence();
    if !send(&copy_chord()) {
        return Err(SelectionError::Other(t!("proofread.selection.copyBlocked")));
    }
    let after = wait_changed(before);
    // L'échec de la relecture n'est rendu qu'après la remise de l'ancien
    // texte : la copie simulée a déjà remplacé le presse-papiers.
    let read = if before == after {
        Ok(None)
    } else {
        match retried(|| clipboard.get_text()) {
            Ok(text) => Ok(Some(text)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(error) => Err(selection_error(error)),
        }
    };
    if restores(before, after, previous.as_deref())
        && let Some(previous) = previous
    {
        // Hors de l'historique (Win+V) : ce texte y est déjà.
        let restored = retried(|| clipboard.set().exclude_from_history().text(&previous));
        if let Err(error) = restored {
            eprintln!("chatonlm: previous clipboard text not restored: {error}");
        }
    }
    Ok(captured(before, after, read?))
}
