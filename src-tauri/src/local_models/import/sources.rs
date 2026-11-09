//! Dossiers de l'Ollama de la machine (lecture seule) : `OLLAMA_MODELS` de
//! l'app, dossier détecté du serveur qui tourne ou du service
//! (`ollama::models_dir::detect`, qui lit l'environnement du processus ou de
//! l'unité systemd), `~/.ollama/models` (`%USERPROFILE%\.ollama\models`
//! sous Windows), dossiers des paquets Linux (`linux.rs`). Gardés : ceux qui
//! existent, sans doublon, hors des dossiers de l'app (Ollama intégré,
//! dossier des modèles).

use std::fs;
use std::path::{Path, PathBuf};

/// `~/.ollama/models` du dossier personnel `home`.
pub fn home_dir(home: &Path) -> PathBuf {
    home.join(".ollama").join("models")
}

/// Forme comparable d'un chemin : liens résolus s'il existe ; sous Windows,
/// sans la casse.
fn comparable(path: &Path) -> String {
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let text = resolved.to_string_lossy().into_owned();
    if cfg!(windows) {
        crate::ollama::models_dir::rules::plain(&text).to_lowercase()
    } else {
        text
    }
}

/// Le dossier ressemble à celui d'un Ollama (ou existe sans se lire : droits
/// du service).
fn looks_like_ollama(dir: &Path) -> bool {
    dir.join("manifests").is_dir()
        || dir.join("manifests-v2").is_dir()
        || (dir.exists() && fs::read_dir(dir).is_err())
}

/// Dossiers candidats, dans l'ordre : `env` (`OLLAMA_MODELS`), `detected`,
/// `home`, puis `system` ; sans ceux de `exclude` ni les doublons.
pub fn candidates(
    env: Option<PathBuf>,
    detected: Option<PathBuf>,
    home: Option<PathBuf>,
    system: Vec<PathBuf>,
    exclude: &[PathBuf],
) -> Vec<PathBuf> {
    let excluded: Vec<String> = exclude.iter().map(|dir| comparable(dir)).collect();
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for dir in env
        .into_iter()
        .chain(detected)
        .chain(home.as_deref().map(home_dir))
        .chain(system)
    {
        if dir.as_os_str().is_empty() || !dir.is_absolute() {
            continue;
        }
        let key = comparable(&dir);
        if excluded.contains(&key) || seen.contains(&key) || !looks_like_ollama(&dir) {
            continue;
        }
        seen.push(key);
        out.push(dir);
    }
    out
}

/// Le dossier `dir` (celui de l'Ollama intégré) est l'un des dossiers
/// `machine` de l'Ollama de la machine, ou l'un est dans l'autre (liens
/// résolus ; sous Windows, sans la casse) : il ne lui appartient pas en
/// propre (relecture L6, B1).
pub fn shared_dir(dir: &Path, machine: &[PathBuf]) -> bool {
    let own = PathBuf::from(comparable(dir));
    machine.iter().any(|other| {
        let other = PathBuf::from(comparable(other));
        own.starts_with(&other) || other.starts_with(&own)
    })
}

/// Dossier personnel de l'utilisateur (`HOME`, `USERPROFILE` sous Windows).
pub fn user_home() -> Option<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Dossiers connus du système (paquets Linux ; aucun sous Windows).
pub fn system_dirs() -> Vec<PathBuf> {
    super::os::system_dirs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ollama_dir(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("manifests")).unwrap();
        dir
    }

    #[test]
    fn existing_ollama_folders_are_listed_once() {
        let root = tempfile::tempdir().unwrap();
        let env = ollama_dir(root.path(), "env");
        let home = root.path().join("home");
        let home_models = home_dir(&home);
        fs::create_dir_all(home_models.join("manifests-v2")).unwrap();
        let builtin = ollama_dir(root.path(), "builtin");
        let empty = root.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        let found = candidates(
            Some(env.clone()),
            // Détecté = le même que `OLLAMA_MODELS` : une seule fois.
            Some(env.join(".").join("")),
            Some(home.clone()),
            vec![builtin.clone(), empty, root.path().join("missing")],
            &[builtin],
        );
        assert_eq!(found, vec![env, home_models]);
        assert_eq!(
            candidates(Some(PathBuf::from("relative")), None, None, Vec::new(), &[]),
            Vec::<PathBuf>::new()
        );
    }

    /// Dossier de l'intégré réglé sur `~/.ollama/models` (accepté en
    /// 0.9.x), par un lien, ou parent / enfant d'un dossier de la machine.
    #[test]
    fn a_builtin_folder_inside_the_machine_ones_is_shared() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let machine = home_dir(&home);
        fs::create_dir_all(machine.join("manifests")).unwrap();
        let found = candidates(None, None, Some(home.clone()), Vec::new(), &[]);
        assert_eq!(found, vec![machine.clone()]);
        assert!(shared_dir(&machine, &found));
        assert!(shared_dir(&machine.join(".").join("blobs"), &found));
        assert!(shared_dir(&home, &found));
        #[cfg(unix)]
        {
            let link = root.path().join("link");
            std::os::unix::fs::symlink(&machine, &link).unwrap();
            assert!(shared_dir(&link, &found));
        }
        let own = root.path().join("data/ollama/models");
        fs::create_dir_all(own.join("manifests")).unwrap();
        assert!(!shared_dir(&own, &found));
        // `models` à côté de `models-old` : pas l'un dans l'autre.
        assert!(!shared_dir(&home.join(".ollama/models-old"), &found));
        assert!(!shared_dir(&own, &[]));
    }
}
