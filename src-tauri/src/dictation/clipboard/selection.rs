//! Copie du texte sélectionné sous Windows, où il n'existe pas de sélection
//! primaire : Ctrl+C simulé dans l'application au premier plan, lecture du
//! presse-papiers, puis remise de son ancien texte. Ici la logique pure
//! (touches à simuler, lecture du résultat), essayée sur tous les systèmes ;
//! les appels à Windows sont dans `windows.rs`.

// Sous Linux, seuls les essais s'en servent.
#![cfg_attr(not(windows), allow(dead_code))]

use std::time::Duration;

/// Codes virtuels de Windows.
pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
/// Alt.
pub const VK_MENU: u16 = 0x12;
pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;
pub const VK_C: u16 = 0x43;

/// Modificateurs qui doivent être relâchés avant le Ctrl+C simulé : le
/// raccourci global vient d'être pressé, et avec Alt encore tenu la copie
/// deviendrait Ctrl+Alt+C (le raccourci de la correction lui-même).
pub const MODIFIERS: [u16; 5] = [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN];

/// Attente maximale du relâchement des modificateurs.
pub const RELEASE_WAIT: Duration = Duration::from_millis(1500);
/// Attente maximale de la copie par l'application au premier plan.
pub const COPY_WAIT: Duration = Duration::from_millis(800);
/// Pas des attentes.
pub const STEP: Duration = Duration::from_millis(20);

/// Touche à presser ou à relâcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyStroke {
    pub key: u16,
    pub down: bool,
}

/// Ctrl+C : Ctrl pressé, C pressé, C relâché, Ctrl relâché.
pub fn copy_chord() -> [KeyStroke; 4] {
    let stroke = |key, down| KeyStroke { key, down };
    [
        stroke(VK_CONTROL, true),
        stroke(VK_C, true),
        stroke(VK_C, false),
        stroke(VK_CONTROL, false),
    ]
}

/// Texte sélectionné d'après le presse-papiers après la copie simulée :
/// son numéro de séquence n'a pas changé (`before == after`) quand rien
/// n'était sélectionné (ou que l'application n'a pas copié) ; un contenu qui
/// n'est pas du texte (`text` absent) ne donne rien non plus.
pub fn captured(before: u32, after: u32, text: Option<String>) -> String {
    if before == after {
        String::new()
    } else {
        text.unwrap_or_default()
    }
}

/// Faut-il remettre l'ancien texte du presse-papiers ? Seulement si la copie
/// l'a remplacé et qu'il y en avait un (une image ou des fichiers ne se
/// remettent pas : ils sont perdus).
pub fn restores(before: u32, after: u32, previous: Option<&str>) -> bool {
    before != after && previous.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_chord_releases_what_it_presses() {
        let chord = copy_chord();
        assert_eq!(
            chord.map(|stroke| (stroke.key, stroke.down)),
            [
                (VK_CONTROL, true),
                (VK_C, true),
                (VK_C, false),
                (VK_CONTROL, false)
            ]
        );
        // Aucun des modificateurs attendus n'est la touche copiée.
        assert!(!MODIFIERS.contains(&VK_C));
        assert!(MODIFIERS.contains(&VK_MENU) && MODIFIERS.contains(&VK_CONTROL));
    }

    #[test]
    fn an_unchanged_clipboard_means_nothing_was_selected() {
        // Rien de copié : l'ancien contenu n'est pas pris pour la sélection.
        assert_eq!(captured(7, 7, Some("old clipboard".into())), "");
        assert_eq!(captured(7, 8, Some("selected".into())), "selected");
        // Copié, mais pas du texte (image).
        assert_eq!(captured(7, 9, None), "");
    }

    #[test]
    fn the_previous_text_is_restored_only_after_a_copy() {
        assert!(restores(7, 8, Some("old")));
        assert!(restores(7, 8, Some("")));
        assert!(!restores(7, 7, Some("old")));
        assert!(!restores(7, 8, None));
    }

    #[test]
    fn waits_stay_within_the_selection_timeout() {
        assert!(RELEASE_WAIT + COPY_WAIT < crate::dictation::clipboard::SELECTION_TIMEOUT);
        assert!(STEP < COPY_WAIT);
    }
}
