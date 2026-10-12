//! Aides partagées par le code propre à Windows : programmes lancés sans
//! console, chaînes UTF-16 des API Win32. Sous Linux, [`NoWindow::no_window`]
//! ne fait rien : le code commun l'appelle sans `cfg`.
//!
//! Les conversions de chaînes sont de la logique pure, essayée sous Linux ;
//! seules celles qui partent d'un `OsStr` (noms de fichiers) sont propres à
//! Windows.

/// `CREATE_NO_WINDOW` : un programme en mode console lancé par une app
/// graphique n'ouvre pas de fenêtre de console (elle clignoterait à chaque
/// lancement).
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Lancement d'un programme sans fenêtre de console.
pub trait NoWindow {
    /// Sous Windows, pose `CREATE_NO_WINDOW` (à appeler avant tout autre
    /// `creation_flags`, qu'il remplacerait) ; ailleurs, sans effet.
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindow for std::process::Command {
    fn no_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(self, CREATE_NO_WINDOW);
        self
    }
}

impl NoWindow for tokio::process::Command {
    fn no_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        self.creation_flags(CREATE_NO_WINDOW);
        self
    }
}

/// Chaîne UTF-16 terminée par un zéro, pour un paramètre `PCWSTR`. Un zéro
/// dans le texte le couperait : il est retiré.
#[cfg(any(windows, test))]
#[cfg_attr(windows, allow(dead_code))]
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16()
        .filter(|unit| *unit != 0)
        .chain(std::iter::once(0))
        .collect()
}

/// Comme [`wide`], pour un chemin ou un argument du système (qui peut ne pas
/// être de l'UTF-16 valide).
#[cfg(windows)]
#[allow(dead_code)]
pub fn wide_os(text: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    text.as_ref()
        .encode_wide()
        .filter(|unit| *unit != 0)
        .chain(std::iter::once(0))
        .collect()
}

/// Texte d'un tampon UTF-16 rempli par une API Win32 : jusqu'au premier
/// zéro (ou la fin du tampon), unités invalides remplacées.
#[cfg(any(windows, test))]
#[cfg_attr(windows, allow(dead_code))]
pub fn from_wide(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// Textes d'une liste Win32 `PZZWSTR` : séparés par un zéro, la liste finit
/// par deux zéros (ou avec le tampon).
#[cfg(any(windows, test))]
#[cfg_attr(windows, allow(dead_code))]
pub fn from_wide_list(buffer: &[u16]) -> Vec<String> {
    buffer
        .split(|unit| *unit == 0)
        .take_while(|text| !text.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_become_zero_terminated_utf16() {
        assert_eq!(wide("ab"), [0x61, 0x62, 0]);
        assert_eq!(wide(""), [0]);
        // Hors du plan de base : paire de substitution.
        assert_eq!(wide("é😀"), [0xe9, 0xd83d, 0xde00, 0]);
        // Un zéro dans le texte ne coupe pas la chaîne.
        assert_eq!(wide("a\0b"), [0x61, 0x62, 0]);
    }

    #[test]
    fn buffers_are_read_up_to_the_first_zero() {
        assert_eq!(from_wide(&[0x61, 0xe9, 0, 0x62]), "aé");
        assert_eq!(from_wide(&[0x61, 0x62]), "ab");
        assert_eq!(from_wide(&[]), "");
        assert_eq!(from_wide(&wide("Élève 😀")), "Élève 😀");
        // Substitut isolé : remplacé, jamais d'échec.
        assert_eq!(from_wide(&[0xd83d, 0]), "\u{fffd}");
    }

    #[test]
    fn lists_end_at_the_double_zero() {
        let list: Vec<u16> = "fr-FR\0en-US\0\0reste".encode_utf16().collect();
        assert_eq!(from_wide_list(&list), ["fr-FR", "en-US"]);
        let unterminated: Vec<u16> = "es-ES".encode_utf16().collect();
        assert_eq!(from_wide_list(&unterminated), ["es-ES"]);
        assert!(from_wide_list(&[0, 0]).is_empty());
        assert!(from_wide_list(&[]).is_empty());
    }

    /// Sous Linux, `no_window` ne change rien : la commande part telle
    /// quelle.
    #[cfg(target_os = "linux")]
    #[test]
    fn no_window_leaves_a_command_usable() {
        let mut command = std::process::Command::new("true");
        assert!(command.no_window().status().unwrap().success());
    }
}
