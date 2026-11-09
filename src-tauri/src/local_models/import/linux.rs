//! Linux : système de fichiers d'un dossier (numéro de périphérique),
//! fichier identique (inode) et dossiers où l'Ollama de la machine range ses modèles.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Périphérique du plus proche ancêtre existant de `path`.
fn device(path: &Path) -> Option<u64> {
    path.ancestors()
        .find_map(|ancestor| std::fs::metadata(ancestor).ok())
        .map(|meta| meta.dev())
}

/// Même système de fichiers : un renommage suffit.
pub fn same_filesystem(a: &Path, b: &Path) -> bool {
    let (a, b) = (device(a), device(b));
    a.is_some() && a == b
}

/// Le même fichier (lien physique) : même périphérique, même inode.
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Dossiers connus de l'Ollama de la machine, hors `OLLAMA_MODELS` et
/// `~/.ollama/models` (communs aux deux systèmes) : service du script
/// officiel (`/usr/share/ollama/.ollama/models`) et paquet d'Arch Linux
/// (`OLLAMA_MODELS=/var/lib/ollama`).
pub fn system_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/share/ollama/.ollama/models"),
        PathBuf::from("/var/lib/ollama"),
        PathBuf::from("/var/lib/ollama/.ollama/models"),
    ]
}
