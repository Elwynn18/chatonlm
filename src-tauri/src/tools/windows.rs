//! Outils de la Transcription sous Windows : archives `win` / `windows-msvc`
//! (x86-64 seulement) ; aucun droit d'exécution à poser (un `.exe` se lance
//! tel quel). Fichiers ouverts ni renommables ni supprimables : voir
//! `install` (renommages patients, effacement différé au lancement suivant).

use std::path::Path;

use super::Platform;

pub const PLATFORM: Option<Platform> = if cfg!(target_arch = "x86_64") {
    Some(Platform::Windows)
} else {
    None
};

/// Sans objet sous Windows.
pub fn set_executable(_path: &Path, _executable: bool) -> std::io::Result<()> {
    Ok(())
}
