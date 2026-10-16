//! Dossier des modèles d'Ollama sous Windows (Settings > Model location).
//! Partie Windows de `models_dir.rs` ; la logique est dans
//! [`rules`](super::rules), essayée aussi sous Linux.
//!
//! - Détection, sans droits ni sous-processus : quel programme écoute à
//!   l'adresse réglée (table TCP du système → processus → son exécutable et
//!   son parent). L'Ollama de l'app de bureau officielle range ses modèles
//!   où cette app le lui dit (son propre réglage, puis la variable
//!   d'utilisateur `OLLAMA_MODELS`) : ChatonLM ne le change jamais, il
//!   indique le dossier probable et comment le changer (écart voulu à Linux,
//!   où le service systemd se règle par polkit : rien de tel ici, et il
//!   faudrait arrêter un serveur que ChatonLM n'a pas lancé).
//! - Validation du dossier choisi ([`check_folder`]) : pour l'Ollama intégré,
//!   dont ChatonLM change lui-même le dossier (`builtin`), et pour la
//!   commande à copier d'un `ollama serve` lancé à la main.

use std::path::{Path, PathBuf};

use url::Url;

use super::rules::{self, Listener, SystemFolders};
use super::{
    Detection, FolderCheck, FolderKind, FolderProblem, ModelsLocation, current_dir, folder_kind,
};
use crate::ollama::client::{self, Probe};
use crate::ollama::detect;
use crate::ollama::host::windows::{find_app, listener_pid, process_image, processes};

/// Port par défaut d'Ollama, celui que sert l'app de bureau.
const STANDARD_PORT: u16 = 11434;

/// Détecte où Ollama range ses modèles pour l'adresse effective `url`.
pub async fn detect(url: &str) -> Detection {
    let Ok(parsed) = Url::parse(url) else {
        return Detection::new(ModelsLocation::Unknown { running: false }, None);
    };
    let host_env = detect::serve_host(&parsed);
    if !client::is_local(&parsed) {
        let host = parsed
            .host_str()
            .unwrap_or_default()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        return Detection::new(ModelsLocation::Remote { host }, host_env);
    }
    let port = parsed.port_or_known_default();
    let gathered = tauri::async_runtime::spawn_blocking(move || {
        let listener = port.and_then(listener);
        (listener, find_app().is_some(), probable_dir())
    })
    .await;
    let Ok((listener, app_installed, dir)) = gathered else {
        return Detection::new(ModelsLocation::Unknown { running: false }, host_env);
    };
    let answers = listener.is_none() && matches!(client::probe(url).await, Probe::Ollama { .. });
    let location = rules::location(
        listener.as_ref(),
        app_installed,
        port == Some(STANDARD_PORT),
        answers,
        dir,
        host_env.clone(),
    );
    Detection::new(location, host_env)
}

/// Programme qui écoute sur `port`. Bloquant.
fn listener(port: u16) -> Option<Listener> {
    let pid = listener_pid(port)??;
    let name = |path: PathBuf| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    };
    let all = processes();
    let entry = all.iter().find(|process| process.pid == pid);
    // Le nom de l'instantané suffit ; l'exécutable complet le confirme
    // quand le système le donne.
    let own = match process_image(pid) {
        Ok(path) => name(path),
        Err(true) => return Some(Listener::Hidden),
        Err(false) => None,
    }
    .or_else(|| entry.map(|process| process.name.clone()));
    let Some(own) = own else {
        return Some(Listener::Hidden);
    };
    let parent = entry
        .and_then(|process| all.iter().find(|parent| parent.pid == process.parent))
        .map(|parent| parent.name.clone());
    Some(Listener::Process { name: own, parent })
}

/// Dossier des modèles probable de l'Ollama de la machine (voir
/// [`rules::probable_dir`]).
fn probable_dir() -> Option<String> {
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
    let profile = std::env::var("USERPROFILE").ok();
    rules::probable_dir(user.as_deref(), machine.as_deref(), profile.as_deref())
}

/// Dossiers du système de cette machine.
fn system_folders() -> SystemFolders {
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let mut trees: Vec<String> = [
        "SystemRoot",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "ProgramData",
    ]
    .into_iter()
    .filter_map(var)
    .collect();
    // Dossier d'installation de ChatonLM (vidé par une mise à jour).
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .and_then(|dir| dir.into_os_string().into_string().ok())
    {
        trees.push(dir);
    }
    let profile = var("USERPROFILE");
    let profiles = profile
        .as_deref()
        .and_then(|profile| Path::new(profile).parent())
        .and_then(Path::to_str)
        .map(str::to_owned);
    SystemFolders {
        trees,
        exact: profile.into_iter().chain(profiles).collect(),
    }
}

/// Forme canonique (liens résolus, sans préfixe `\\?\`) ; un dossier absent
/// garde son nom sous son parent canonique.
fn canonical(raw: &str) -> Result<String, FolderProblem> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(FolderProblem::Missing);
    }
    let resolved = match dunce::canonicalize(path) {
        Ok(resolved) => resolved,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
                return Err(FolderProblem::Missing);
            };
            dunce::canonicalize(parent)
                .map_err(|_| FolderProblem::Missing)?
                .join(name)
        }
        Err(_) => return Err(FolderProblem::Missing),
    };
    resolved
        .into_os_string()
        .into_string()
        .map(|path| rules::plain(&path))
        .map_err(|_| FolderProblem::BadCharacters)
}

/// Problème du contenu d'un dossier : ni vide, ni dossier de modèles ;
/// absent sans dossier parent.
fn content_problem(path: &Path) -> Option<FolderProblem> {
    match folder_kind(path) {
        FolderKind::Empty | FolderKind::Models => None,
        FolderKind::Other => Some(FolderProblem::NotEmpty),
        FolderKind::NotFolder => Some(FolderProblem::Missing),
        FolderKind::Missing => {
            (!path.parent().is_some_and(Path::is_dir)).then_some(FolderProblem::Missing)
        }
    }
}

/// Vérifie le dossier choisi `raw` (bloquant : lit le disque). Les champs
/// qui dépendent du serveur ou du reste de l'app (`models`,
/// `replies_running`, `downloads_running`) restent à remplir.
pub fn check_folder(raw: &str, detection: &Detection) -> FolderCheck {
    let mut check = FolderCheck {
        path: raw.to_owned(),
        problem: None,
        old: None,
        has_models: false,
        same_drive: false,
        models: None,
        free_bytes: None,
        commands: None,
        replies_running: false,
        downloads_running: false,
    };
    let path = match canonical(raw) {
        Ok(path) => path,
        Err(problem) => {
            check.problem = Some(problem);
            return check;
        }
    };
    check.path.clone_from(&path);
    check.old = current_dir(&detection.location)
        .map(|dir| canonical(dir).unwrap_or_else(|_| rules::plain(dir)));
    check.problem = rules::path_problem(&path, check.old.as_deref(), &system_folders())
        .or_else(|| content_problem(Path::new(&path)));
    if check.problem.is_some() {
        return check;
    }
    check.has_models = folder_kind(Path::new(&path)) == FolderKind::Models;
    let at = if Path::new(&path).is_dir() {
        PathBuf::from(&path)
    } else {
        Path::new(&path)
            .parent()
            .map_or_else(|| PathBuf::from(&path), Path::to_path_buf)
    };
    // Même volume : renommage instantané (un refus du système est rattrapé
    // par une copie, voir `builtin::move_files`).
    check.same_drive = check.old.as_deref().is_some_and(|old| {
        rules::volume(old).is_some() && rules::volume(old) == rules::volume(&path)
    });
    check.free_bytes = crate::disk::free_space(&at);
    check.commands = match &detection.location {
        ModelsLocation::Manual { host_env, .. } => {
            Some(rules::manual_command(&path, host_env.as_deref()))
        }
        _ => None,
    };
    check
}
