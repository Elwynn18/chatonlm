//! Disque : place libre d'un dossier, pour l'utilisateur (quotas compris),
//! partagée par le changement du dossier des modèles d'Ollama et les modèles
//! d'images. Lecture propre au système dans `disk/linux.rs` (`statvfs`) et
//! `disk/windows.rs` (`GetDiskFreeSpaceExW`).

use std::path::Path;

#[cfg(target_os = "linux")]
#[path = "disk/linux.rs"]
mod os;
#[cfg(windows)]
#[path = "disk/windows.rs"]
mod os;

/// Place libre à l'emplacement de `path` ; `None` si elle ne se lit pas.
pub fn free_space(path: &Path) -> Option<u64> {
    os::free_space(path)
}

/// Place libre pour un dossier qui n'existe peut-être pas encore : celle de
/// son plus proche ancêtre existant.
pub fn free_space_for(path: &Path) -> Option<u64> {
    path.ancestors()
        .find(|ancestor| ancestor.is_dir())
        .and_then(free_space)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_space_is_read_for_missing_folders_too() {
        let dir = tempfile::tempdir().unwrap();
        let known = free_space(dir.path()).expect("free space of a temporary folder");
        assert!(known > 0);
        let missing = dir.path().join("not").join("yet");
        assert!(free_space(&missing).is_none());
        assert!(free_space_for(&missing).is_some());
    }
}
