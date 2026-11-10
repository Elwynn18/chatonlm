//! Windows : volume d'un dossier (lettre de lecteur ou partage, logique pure
//! [`super::same_volume`]) ; l'Ollama de la machine n'a pas d'autre dossier
//! connu que `%USERPROFILE%\.ollama\models` et `OLLAMA_MODELS`.

use std::path::{Path, PathBuf};

/// Même volume : un renommage suffit (un refus du système est rattrapé par
/// une copie, voir `transfer::place`).
pub fn same_filesystem(a: &Path, b: &Path) -> bool {
    super::same_volume(&a.to_string_lossy(), &b.to_string_lossy())
}

/// Le même fichier (lien physique) : l'identifiant de fichier de Windows
/// n'est pas lisible en Rust stable ; faux, et l'appelant compare le contenu
/// (empreinte), sinon refuse.
pub fn same_file(_a: &Path, _b: &Path) -> bool {
    false
}

/// Aucun dossier du système : l'app officielle d'Ollama range ses modèles
/// chez l'utilisateur.
#[cfg_attr(test, allow(dead_code))]
pub fn system_dirs() -> Vec<PathBuf> {
    Vec::new()
}
