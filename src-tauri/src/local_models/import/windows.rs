//! Windows : volume d'un dossier (lettre de lecteur ou partage, logique pure
//! [`super::same_volume`]) ; l'Ollama de la machine n'a pas d'autre dossier
//! connu que `%USERPROFILE%\.ollama\models` et `OLLAMA_MODELS` (variables
//! de l'utilisateur et du système, lues dans le registre : celles de l'app
//! datent de son lancement).

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

/// Dossiers réglés pour l'Ollama de la machine : `OLLAMA_MODELS` de
/// l'utilisateur, puis du système (logique pure
/// [`super::winpath::configured_dirs`]).
#[cfg_attr(test, allow(dead_code))]
pub fn configured_dirs(_data_dir: &Path) -> Vec<PathBuf> {
    let variable = |root: &windows_registry::Key, key: &str| {
        root.open(key)
            .and_then(|key| key.get_string("OLLAMA_MODELS"))
            .ok()
    };
    let user = variable(windows_registry::CURRENT_USER, "Environment");
    let machine = variable(
        windows_registry::LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
    );
    super::winpath::configured_dirs(user.as_deref(), machine.as_deref())
        .into_iter()
        .map(PathBuf::from)
        .collect()
}
