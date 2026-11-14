//! Outils de la Transcription sous Linux : archives `linux` (x86-64
//! seulement) ; droits d'exécution posés à l'extraction (le zip les porte,
//! le programme principal les reçoit toujours).

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::Platform;

pub const PLATFORM: Option<Platform> = if cfg!(target_arch = "x86_64") {
    Some(Platform::Linux)
} else {
    None
};

/// `0o755` pour un exécutable (programme, bibliothèque marquée exécutable
/// dans l'archive), `0o644` sinon.
pub fn set_executable(path: &Path, executable: bool) -> std::io::Result<()> {
    let mode = if executable { 0o755 } else { 0o644 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}
