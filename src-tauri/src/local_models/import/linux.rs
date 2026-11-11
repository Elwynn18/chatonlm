//! Linux : système de fichiers d'un dossier (numéro de périphérique),
//! fichier identique (inode) et dossiers où l'Ollama de la machine range ses
//! modèles (dossiers des paquets, environnement de ses services systemd lu
//! par D-Bus sans droits, environnement des processus `ollama` du même
//! compte).

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
#[cfg_attr(test, allow(dead_code))]
pub fn system_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/share/ollama/.ollama/models"),
        PathBuf::from("/var/lib/ollama"),
        PathBuf::from("/var/lib/ollama/.ollama/models"),
    ]
}

/// Délai d'un appel D-Bus à systemd.
const BUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Dossiers réglés pour l'Ollama de la machine : `OLLAMA_MODELS` des unités
/// `ollama.service` (système puis utilisateur : `Environment=`, fichiers de
/// `EnvironmentFile=`, processus principal s'il est lisible), dossier
/// personnel de `User=` (`~/.ollama/models`), puis `OLLAMA_MODELS` des
/// processus `ollama` de ce compte. Bloquant ; un bus absent ou figé ne prive
/// pas du reste.
#[cfg_attr(test, allow(dead_code))]
pub fn configured_dirs(data_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for system in [true, false] {
        if let Ok(Some(unit)) = read_unit(system) {
            dirs.extend(unit_dirs(&unit));
        }
    }
    let old_home = super::old_builtin::root(data_dir).join("home");
    dirs.extend(process_dirs(Path::new("/proc"), &old_home));
    dirs
}

/// Ce que l'unité `ollama.service` dit de son dossier.
#[derive(Default)]
struct UnitFacts {
    environment: Vec<String>,
    environment_files: Vec<String>,
    main_pid: u32,
    user: String,
}

fn unit_dirs(unit: &UnitFacts) -> Vec<PathBuf> {
    let from_files = unit.environment_files.iter().fold(None, |found, file| {
        std::fs::read_to_string(file)
            .ok()
            .and_then(|text| super::sources::env_file_models(&text))
            .or(found)
    });
    let from_process = (unit.main_pid != 0)
        .then(|| environ(Path::new("/proc"), unit.main_pid))
        .flatten()
        .and_then(|items| super::sources::env_models(items.iter().map(String::as_str)));
    let configured = from_process
        .or(from_files)
        .or_else(|| super::sources::env_models(unit.environment.iter().map(String::as_str)));
    let home = (!unit.user.is_empty())
        .then(|| home_of(&unit.user))
        .flatten()
        .map(|home| super::sources::home_dir(Path::new(&home)));
    configured
        .map(PathBuf::from)
        .into_iter()
        .chain(home)
        .collect()
}

/// Lit `ollama.service` sur le bus du système (`system`) ou de la session ;
/// `Ok(None)` si l'unité n'existe pas.
fn read_unit(system: bool) -> zbus::Result<Option<UnitFacts>> {
    use zbus::blocking::connection::Builder as ConnectionBuilder;
    use zbus::blocking::proxy::Builder;
    use zbus::proxy::CacheProperties;
    let connection = if system {
        ConnectionBuilder::system()?
    } else {
        ConnectionBuilder::session()?
    }
    .method_timeout(BUS_TIMEOUT)
    .build()?;
    let proxy = |path: zbus::zvariant::OwnedObjectPath, interface: &'static str| {
        Builder::<zbus::blocking::Proxy>::new(&connection)
            .destination("org.freedesktop.systemd1")?
            .path(path)?
            .interface(interface)?
            .cache_properties(CacheProperties::No)
            .build()
    };
    let manager = proxy(
        zbus::zvariant::OwnedObjectPath::try_from("/org/freedesktop/systemd1")?,
        "org.freedesktop.systemd1.Manager",
    )?;
    // LoadUnit lit l'unité sur le disque sans la démarrer.
    let path: zbus::zvariant::OwnedObjectPath = manager.call("LoadUnit", &("ollama.service",))?;
    let unit = proxy(path.clone(), "org.freedesktop.systemd1.Unit")?;
    let load_state: String = unit.get_property("LoadState")?;
    if load_state != "loaded" {
        return Ok(None);
    }
    let service = proxy(path, "org.freedesktop.systemd1.Service")?;
    let files: Vec<(String, bool)> = service.get_property("EnvironmentFiles").unwrap_or_default();
    Ok(Some(UnitFacts {
        environment: service.get_property("Environment").unwrap_or_default(),
        environment_files: files.into_iter().map(|(file, _)| file).collect(),
        main_pid: service.get_property("MainPID").unwrap_or_default(),
        user: service.get_property("User").unwrap_or_default(),
    }))
}

/// Environnement d'un processus (`/proc/<pid>/environ`), s'il est lisible
/// (même compte).
fn environ(proc: &Path, pid: u32) -> Option<Vec<String>> {
    let bytes = std::fs::read(proc.join(pid.to_string()).join("environ")).ok()?;
    Some(
        bytes
            .split(|byte| *byte == 0)
            .map(|item| String::from_utf8_lossy(item).into_owned())
            .collect(),
    )
}

/// `OLLAMA_MODELS` des processus `ollama` lisibles (`ollama serve` lancé à
/// la main, service de l'utilisateur), sauf l'ancien intégré de ChatonLM,
/// reconnu comme par `old_builtin` à son `HOME` privé (`old_home`), pas à
/// son chemin : l'archive officielle d'Ollama extraite dans `/opt/ollama` ou
/// `~/ollama` donne aussi `…/ollama/bin/ollama`.
fn process_dirs(proc: &Path, old_home: &Path) -> Vec<PathBuf> {
    let old_home = format!("HOME={}", old_home.display());
    let Ok(entries) = std::fs::read_dir(proc) else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    for pid in entries.filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok()) {
        let Ok(exe) = std::fs::read_link(proc.join(pid.to_string()).join("exe")) else {
            continue;
        };
        let text = exe.to_string_lossy();
        let exe = Path::new(text.strip_suffix(" (deleted)").unwrap_or(&text));
        if exe.file_name().is_none_or(|name| name != "ollama") {
            continue;
        }
        let Some(items) = environ(proc, pid) else {
            continue;
        };
        if items.contains(&old_home) {
            continue;
        }
        let found = super::sources::env_models(items.iter().map(String::as_str));
        if let Some(dir) = found.map(PathBuf::from)
            && !dirs.contains(&dir)
        {
            dirs.push(dir);
        }
    }
    dirs
}

/// Répertoire personnel d'un utilisateur du système (`/etc/passwd`, où
/// figurent toujours les utilisateurs de service) ; `/root` pour root.
fn home_of(user: &str) -> Option<String> {
    if user == "root" || user == "0" {
        return Some("/root".to_owned());
    }
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        let [name, _, uid, _, _, home, ..] = fields.as_slice() else {
            return None;
        };
        (*name == user || *uid == user).then(|| (*home).to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `/proc` d'essai : un processus `ollama` lancé à la main, l'ancien
    /// intégré de ChatonLM et un autre programme.
    fn fake_proc(root: &Path, pid: u32, exe: &str, environ: &str) {
        let dir = root.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(exe, dir.join("exe")).unwrap();
        std::fs::write(dir.join("environ"), environ.replace(';', "\0")).unwrap();
    }

    #[test]
    fn the_folder_of_a_running_ollama_is_read_from_its_environment() {
        let root = tempfile::tempdir().unwrap();
        fake_proc(
            root.path(),
            10,
            "/usr/bin/ollama",
            "HOME=/home/me;OLLAMA_MODELS=/srv/a",
        );
        fake_proc(root.path(), 11, "/usr/bin/ollama", "OLLAMA_MODELS=/srv/a");
        fake_proc(
            root.path(),
            12,
            "/usr/lib/chatonlm/ollama/bin/ollama (deleted)",
            "HOME=/data/chatonlm/ollama/home;OLLAMA_MODELS=/data/chatonlm/ollama/models",
        );
        fake_proc(root.path(), 13, "/usr/bin/python3", "OLLAMA_MODELS=/srv/b");
        fake_proc(root.path(), 14, "/usr/local/bin/ollama", "HOME=/home/me");
        // Archive officielle d'Ollama extraite dans /opt/ollama : gardée
        // (même fin de chemin que l'ancien intégré, autre `HOME`).
        fake_proc(
            root.path(),
            15,
            "/opt/ollama/bin/ollama",
            "HOME=/home/me;OLLAMA_MODELS=/data/llm",
        );
        std::fs::create_dir_all(root.path().join("self")).unwrap();
        let old_home = Path::new("/data/chatonlm/ollama/home");
        let mut found = process_dirs(root.path(), old_home);
        found.sort();
        assert_eq!(found, [PathBuf::from("/data/llm"), PathBuf::from("/srv/a")]);
    }

    #[test]
    fn the_folder_of_the_service_follows_systemd() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("ollama.env");
        std::fs::write(&file, "OLLAMA_MODELS=/srv/from-file\n").unwrap();
        let mut unit = UnitFacts {
            environment: vec!["OLLAMA_MODELS=/srv/from-unit".into()],
            ..UnitFacts::default()
        };
        assert_eq!(unit_dirs(&unit), [PathBuf::from("/srv/from-unit")]);
        // `EnvironmentFile=` l'emporte sur `Environment=`.
        unit.environment_files = vec![file.to_string_lossy().into_owned(), "/missing".into()];
        assert_eq!(unit_dirs(&unit), [PathBuf::from("/srv/from-file")]);
        // Utilisateur du service : son `~/.ollama/models` en plus.
        unit.user = "root".into();
        assert_eq!(
            unit_dirs(&unit),
            [
                PathBuf::from("/srv/from-file"),
                PathBuf::from("/root/.ollama/models")
            ]
        );
    }
}
