//! Dossiers de l'Ollama de la machine (lecture seule) : `OLLAMA_MODELS` de
//! l'app, dossiers réglés pour le serveur (`linux.rs` : environnement des
//! services systemd `ollama.service` et des processus `ollama` qui tournent ;
//! `windows.rs` : variables `OLLAMA_MODELS` de l'utilisateur et du système),
//! `~/.ollama/models` (`%USERPROFILE%\.ollama\models` sous Windows),
//! dossiers des paquets Linux. Gardés : ceux qui existent, sans doublon, hors
//! des dossiers de l'app (Ollama intégré, dossier des modèles).

use std::fs;
use std::path::{Path, PathBuf};

/// `~/.ollama/models` du dossier personnel `home`.
pub fn home_dir(home: &Path) -> PathBuf {
    home.join(".ollama").join("models")
}

/// Forme comparable d'un chemin : liens résolus pour la partie existante
/// ([`super::resolved`]) ; sous Windows, sans la casse.
fn comparable(path: &Path) -> String {
    let text = super::resolved(path).to_string_lossy().into_owned();
    if cfg!(windows) {
        super::winpath::plain(&text).to_lowercase()
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
    detected: Vec<PathBuf>,
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
#[cfg_attr(test, allow(dead_code))]
pub fn user_home() -> Option<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Dossiers connus du système (paquets Linux ; aucun sous Windows).
#[cfg_attr(test, allow(dead_code))]
pub fn system_dirs() -> Vec<PathBuf> {
    super::os::system_dirs()
}

/// Dossiers réglés pour l'Ollama de la machine (services, processus,
/// variables du système), jamais ceux de l'ancien intégré de ChatonLM
/// (sous les données `data_dir`). Bloquant.
#[cfg_attr(test, allow(dead_code))]
pub fn configured_dirs(data_dir: &Path) -> Vec<PathBuf> {
    super::os::configured_dirs(data_dir)
}

/// Valeur d'une variable comme Ollama la lit (`envconfig.Var` : espaces et
/// guillemets autour retirés).
pub(crate) fn env_value(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_owned()
}

/// `OLLAMA_MODELS` parmi des affectations `NOM=valeur` (environnement d'un
/// processus, `Environment=` d'une unité) ; la dernière l'emporte, une
/// valeur vide n'en est pas une.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn env_models<'a>(items: impl IntoIterator<Item = &'a str>) -> Option<String> {
    items
        .into_iter()
        .filter_map(|item| item.strip_prefix("OLLAMA_MODELS="))
        .map(env_value)
        .last()
        .filter(|value| !value.is_empty())
}

/// Dernière affectation de `OLLAMA_MODELS` d'un fichier de
/// `EnvironmentFile=` (`export` admis).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn env_file_models(text: &str) -> Option<String> {
    env_models(text.lines().map(|line| {
        let line = line.trim_start();
        line.strip_prefix("export")
            .filter(|rest| rest.starts_with(char::is_whitespace))
            .map_or(line, str::trim_start)
    }))
}

/// Variable qui remplace la recherche des dossiers de l'Ollama de la machine
/// (e2e, recette) : posée, ses dossiers (séparés comme `PATH`) sont les seuls
/// regardés, sans détection du service ni lecture de `~/.ollama` ; vide :
/// aucun.
#[cfg_attr(test, allow(dead_code))]
pub const MACHINE_DIRS_VAR: &str = "CHATONLM_OLLAMA_MACHINE_DIRS";

/// Dossiers imposés par la valeur `value` de [`MACHINE_DIRS_VAR`] : `None`
/// sans variable (recherche normale).
pub fn forced_dirs(value: Option<std::ffi::OsString>) -> Option<Vec<PathBuf>> {
    value.map(|value| {
        std::env::split_paths(&value)
            .filter(|dir| !dir.as_os_str().is_empty())
            .collect()
    })
}

/// Dossiers candidats : ceux imposés s'il y en a (`forced`), sinon ceux de
/// la recherche `discover` (jamais appelée dans le premier cas).
pub fn chosen(
    forced: Option<Vec<PathBuf>>,
    exclude: &[PathBuf],
    discover: impl FnOnce() -> Vec<PathBuf>,
) -> Vec<PathBuf> {
    match forced {
        Some(dirs) => candidates(None, Vec::new(), None, dirs, exclude),
        None => discover(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Variable posée : seuls ses dossiers, jamais la recherche (service,
    /// `~/.ollama`) ; vide : aucun dossier ; absente : la recherche.
    #[test]
    fn forced_dirs_replace_the_search() {
        let root = tempfile::tempdir().unwrap();
        let given = ollama_dir(root.path(), "given");
        let other = ollama_dir(root.path(), "other");
        let value = std::env::join_paths([given.clone(), other.clone()]).unwrap();
        let forced = forced_dirs(Some(value));
        assert_eq!(forced, Some(vec![given.clone(), other.clone()]));
        assert_eq!(
            chosen(forced, &[other], || panic!("searched")),
            vec![given.clone()]
        );
        let none = forced_dirs(Some(std::ffi::OsString::new()));
        assert_eq!(none, Some(Vec::new()));
        assert!(chosen(none, &[], || panic!("searched")).is_empty());
        assert_eq!(forced_dirs(None), None);
        assert_eq!(chosen(None, &[], || vec![given.clone()]), vec![given]);
    }

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
            vec![env.join(".").join("")],
            Some(home.clone()),
            vec![builtin.clone(), empty, root.path().join("missing")],
            &[builtin],
        );
        assert_eq!(found, vec![env, home_models]);
        assert_eq!(
            candidates(
                Some(PathBuf::from("relative")),
                Vec::new(),
                None,
                Vec::new(),
                &[]
            ),
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
        let found = candidates(None, Vec::new(), Some(home.clone()), Vec::new(), &[]);
        assert_eq!(found, vec![machine.clone()]);
        assert!(shared_dir(&machine, &found));
        assert!(shared_dir(&machine.join(".").join("blobs"), &found));
        assert!(shared_dir(&home, &found));
        #[cfg(unix)]
        {
            let link = root.path().join("link");
            std::os::unix::fs::symlink(&machine, &link).unwrap();
            assert!(shared_dir(&link, &found));
            // Pas encore créé sous un lien : résolu par son parent (sous
            // Windows, même cas avec un nom court `RUNNER~1`).
            assert!(shared_dir(&link.join("blobs"), &found));
        }
        let own = root.path().join("data/ollama/models");
        fs::create_dir_all(own.join("manifests")).unwrap();
        assert!(!shared_dir(&own, &found));
        // `models` à côté de `models-old` : pas l'un dans l'autre.
        assert!(!shared_dir(&home.join(".ollama/models-old"), &found));
        assert!(!shared_dir(&own, &[]));
    }

    /// Dossier réglé pour le serveur : la dernière affectation de
    /// `OLLAMA_MODELS` l'emporte (unité et drop-ins, fichier d'environnement),
    /// lue comme Ollama (guillemets, espaces, `export`).
    #[test]
    fn the_configured_folder_is_read_like_ollama_does() {
        assert_eq!(
            env_models([
                "HOME=/home/me",
                "OLLAMA_MODELS=/a",
                "OLLAMA_MODELS= \"/b\" "
            ]),
            Some("/b".into())
        );
        assert_eq!(env_models(["OLLAMA_MODELS=", "OLLAMA_HOST=x"]), None);
        assert_eq!(env_models(["OLLAMA_MODELS_X=/c"]), None);
        assert_eq!(
            env_file_models("# x\nOLLAMA_MODELS=/a\nexport OLLAMA_MODELS='/srv/models'\n"),
            Some("/srv/models".into())
        );
        assert_eq!(env_file_models("exportOLLAMA_MODELS=/a"), None);
    }
}
