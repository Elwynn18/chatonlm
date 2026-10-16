//! Dossier des modèles d'Ollama sous Linux (Settings > Model location) :
//! détection du lancement d'Ollama, validation du dossier choisi et
//! changement pour le service systemd du système. Partie Linux de
//! `models_dir.rs`, dont elle garde les noms (réexportés) ;
//! `models_dir/windows.rs` en est le pendant.
//!
//! Le dossier appartient au serveur, pas à ChatonLM : c'est `OLLAMA_MODELS` du
//! processus `ollama serve` auquel l'app parle. D'où trois parties :
//! - détection, sans droits ni sous-processus : qui écoute à l'adresse réglée
//!   (`/proc/net/tcp`, socket → processus → cgroup), et ce que dit le service
//!   systemd par D-Bus ; décision pure dans [`classify`] ;
//! - validation du dossier choisi, avant toute invite ([`check_folder`]), avec
//!   les mêmes règles que le script ;
//! - changement, pour le service système seulement : le script embarqué
//!   `models_dir.sh` (drop-in `zz-chatonlm-models.conf`, droits, déplacement,
//!   redémarrage, restauration en cas d'échec) est lancé en root par
//!   `pkexec --disable-internal-agent /bin/sh -c <script> chatonlm-models
//!   ollama.service <move|keep> <ancien|-> <nouveau>` : une seule invite, rien
//!   d'installé, arguments jamais interpolés dans le texte du script. Ailleurs
//!   (lancement à la main, service de l'utilisateur, pkexec absent), l'app
//!   donne les commandes à copier, rendues ici avec les mêmes règles.
//!
//! Le dossier `.ollama` du service (sa clé `id_ed25519`), que `ProtectHome=`
//! peut lui cacher, est traité à part dans [`home`] ; un dossier des modèles
//! sous `/home` le rend aussi visible (`BindPaths=`).
//!
//! Conception, preuves et banc d'essai : `.tools/research/36-modelhub-models-dir.md`.

use std::ffi::OsString;
use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use url::Url;

use super::{
    Detection, FolderCheck, FolderKind, FolderProblem, ModelsLocation, MoveEvent, MoveFailure,
    MoveOutcome, MoveStep, PROGRESS_INTERVAL, current_dir, folder_kind, home,
};
// Les essais (`use super::*`) lisent aussi les éléments communs.
#[cfg(test)]
#[allow(unused_imports)]
use super::*;
use crate::ollama::client::{self, Probe};
use crate::ollama::detect;
use crate::ollama::host::{self, ServiceDetails};

/// Script exécuté en root par pkexec, passé tel quel dans argv (le banc
/// d'essai de la recherche l'a vérifié sous `sh` et `dash`).
pub const SCRIPT: &str = include_str!("../models_dir.sh");

/// Shell qui lit le script.
pub const SHELL: &str = "/bin/sh";

/// Seule unité que le script accepte.
pub(super) const UNIT: &str = "ollama.service";

/// Drop-in écrit par le script (après `override.conf` de `systemctl edit`).
pub const DROPIN_NAME: &str = "zz-chatonlm-models.conf";

/// Emplacements de pkexec (setuid root exigé).
pub(super) const PKEXEC_PATHS: [&str; 2] = ["/usr/bin/pkexec", "/bin/pkexec"];

/// Longueur maximale d'un chemin (PATH_MAX).
pub(super) const MAX_PATH: usize = 4096;

/// Délai de la lecture des faits de la machine (D-Bus compris).
pub(super) const FACTS_TIMEOUT: Duration = Duration::from_secs(4);

/// Taille gardée de la sortie d'erreur du script (montrée dans « Details »).
pub(super) const MAX_DETAIL: usize = 64 * 1024;

// --- Détection ---------------------------------------------------------------

/// Socket en écoute sur le port de l'adresse réglée.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listener {
    pub uid: u32,
    pub inode: u64,
}

/// Variables d'Ollama dans un environnement.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Env {
    pub models: Option<String>,
    pub home: Option<String>,
    pub host: Option<String>,
}

impl Env {
    /// Affectations `NOM=valeur` ; la dernière d'une variable l'emporte. Les
    /// valeurs sont lues comme `envconfig.Var` d'Ollama (espaces et
    /// guillemets autour retirés).
    pub fn from_assignments<'a>(items: impl IntoIterator<Item = &'a str>) -> Env {
        let mut env = Env::default();
        for item in items {
            let Some((name, value)) = item.split_once('=') else {
                continue;
            };
            let value = Some(env_value(value)).filter(|value| !value.is_empty());
            match name {
                "OLLAMA_MODELS" => env.models = value,
                "HOME" => env.home = value,
                "OLLAMA_HOST" => env.host = value,
                _ => {}
            }
        }
        env
    }

    /// Dossier qu'Ollama utiliserait : `OLLAMA_MODELS`, sinon
    /// `$HOME/.ollama/models`.
    pub fn models_dir(&self) -> Option<String> {
        self.models
            .clone()
            .or_else(|| self.home.as_deref().map(home_models))
    }
}

pub(super) fn env_value(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_owned()
}

pub(super) fn home_models(home: &str) -> String {
    format!("{}/.ollama/models", home.trim_end_matches('/'))
}

/// Processus qui tient la socket (lisible seulement s'il a notre uid).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessFacts {
    /// Chemin du cgroup (ligne `0::` de `/proc/<pid>/cgroup`).
    pub cgroup: String,
    /// Nom de l'exécutable.
    pub exe: String,
    pub env: Env,
}

/// Service système `ollama.service`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitFacts {
    pub service: ServiceDetails,
    /// `Environment=` de l'unité.
    pub env: Env,
    /// Dernier `OLLAMA_MODELS` d'un fichier de `EnvironmentFile=` lisible :
    /// (fichier, valeur).
    pub env_file: Option<(String, String)>,
    /// Uid du processus principal.
    pub main_uid: Option<u32>,
    /// Environnement du processus principal, s'il est lisible (même uid).
    pub main_env: Option<Env>,
    /// Répertoire personnel de `User=` (`/root` sans `User=`).
    pub user_home: Option<String>,
    /// Uid de `User=` (0 sans `User=`) ; `None` : inconnu.
    pub user_uid: Option<u32>,
    /// Destinations des `BindPaths=` et `BindReadOnlyPaths=` dont la source
    /// existe ([`home::visible_binds`]).
    pub visible_binds: Vec<String>,
}

impl UnitFacts {
    fn running(&self) -> bool {
        matches!(
            self.service.active_state.as_str(),
            "active" | "activating" | "reloading"
        )
    }
}

/// Faits de la machine, entrées de [`classify`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// Uid de l'app.
    pub own_uid: u32,
    pub listener: Option<Listener>,
    pub process: Option<ProcessFacts>,
    /// `None` : unité absente, ou systemd injoignable.
    pub unit: Option<UnitFacts>,
    /// L'adresse répond comme Ollama (relevé seulement si rien n'écoute :
    /// conteneur sans socket sur l'hôte).
    pub answers: bool,
    pub can_elevate: bool,
}

/// Décision, sans entrée-sortie : testable pour chaque combinaison.
pub fn classify(url: &Url, facts: &Facts) -> Detection {
    let host_env = detect::serve_host(url);
    let plain = |location| Detection::new(location, host_env.clone());
    if !client::is_local(url) {
        let host = url
            .host_str()
            .unwrap_or_default()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        return plain(ModelsLocation::Remote { host });
    }
    let port = url.port_or_known_default();
    // Service dont OLLAMA_HOST donne le port de l'adresse réglée.
    let service_here = facts
        .unit
        .as_ref()
        .filter(|unit| port.is_some() && port == ollama_host_port(unit.env.host.as_deref()));

    match (facts.listener, &facts.process) {
        (Some(listener), Some(process)) if listener.uid == facts.own_uid => {
            if process.cgroup.ends_with("/system.slice/ollama.service") {
                return match &facts.unit {
                    Some(unit) => system(unit, Some(&process.env), true, facts, host_env.clone()),
                    None => plain(ModelsLocation::Unknown { running: true }),
                };
            }
            if is_user_service(&process.cgroup) {
                return plain(ModelsLocation::UserService {
                    dir: process.env.models_dir(),
                });
            }
            if is_container(process) {
                return plain(ModelsLocation::Container);
            }
            if process.exe == "ollama" {
                return plain(ModelsLocation::Manual {
                    dir: process.env.models_dir(),
                    host_env: host_env.clone(),
                });
            }
            plain(ModelsLocation::Unknown { running: true })
        }
        // Autre uid (cas du script officiel, User=ollama), ou processus
        // introuvable : le service est reconnu à son processus principal.
        (Some(listener), _) => match service_here {
            Some(unit) if unit.running() && unit.main_uid == Some(listener.uid) => {
                system(unit, None, true, facts, host_env.clone())
            }
            // Socket de root sans service correspondant : docker-proxy.
            _ if listener.uid == 0 => plain(ModelsLocation::Container),
            _ => plain(ModelsLocation::Unknown { running: true }),
        },
        // Rien n'écoute : le service configuré pour cette adresse, arrêté
        // (ou qui démarre).
        (None, _) => match service_here {
            Some(unit) => system(unit, None, unit.running(), facts, host_env.clone()),
            None => plain(ModelsLocation::Unknown {
                running: facts.answers,
            }),
        },
    }
}

pub(super) fn system(
    unit: &UnitFacts,
    process_env: Option<&Env>,
    running: bool,
    facts: &Facts,
    host_env: Option<String>,
) -> Detection {
    let service = &unit.service;
    // Le processus en marche fait foi ; sinon la configuration, dans l'ordre
    // de systemd (EnvironmentFile= l'emporte sur Environment=).
    let dir = process_env
        .and_then(Env::models_dir)
        .or_else(|| unit.main_env.as_ref().and_then(Env::models_dir))
        .or_else(|| unit.env_file.as_ref().map(|(_, value)| value.clone()))
        .or_else(|| unit.env.models_dir())
        .or_else(|| unit.user_home.as_deref().map(home_models));
    // Un service qui tourne en root (pas de `User=`, ou un nom d'uid 0) est
    // toujours « root » : l'app ne le change pas elle-même.
    let user = if service.user.is_empty() || unit.user_uid == Some(0) {
        "root".to_owned()
    } else {
        service.user.clone()
    };
    let location = ModelsLocation::SystemService {
        dir,
        user,
        group: service.group.clone(),
        running,
        managed: service
            .drop_in_paths
            .iter()
            .any(|path| path.ends_with(&format!("/{DROPIN_NAME}"))),
        can_elevate: facts.can_elevate,
        env_file: unit.env_file.as_ref().map(|(file, _)| file.clone()),
    };
    let ollama_dir = home::service_ollama_dir(
        unit.env.home.as_deref(),
        &service.user,
        unit.user_home.as_deref(),
    );
    let hidden_home = home::hidden_home(
        &location,
        ollama_dir.as_deref(),
        &service.protect_home,
        &unit.visible_binds,
    );
    Detection {
        location,
        protect_home: service.protect_home.clone(),
        protect_system: service.protect_system.clone(),
        host_env,
        ollama_dir,
        hidden_home,
    }
}

/// `…/user@<uid>.service/…/ollama.service`.
pub(super) fn is_user_service(cgroup: &str) -> bool {
    cgroup.contains("/user@") && cgroup.ends_with("/ollama.service")
}

pub(super) fn is_container(process: &ProcessFacts) -> bool {
    process.cgroup.contains("docker-")
        || process.cgroup.contains("libpod-")
        || matches!(
            process.exe.as_str(),
            "rootlessport"
                | "rootlessport-child"
                | "pasta"
                | "pasta.avx2"
                | "slirp4netns"
                | "conmon"
        )
}

/// Port d'écoute d'`ollama serve` pour cette valeur de `OLLAMA_HOST`, comme
/// `envconfig.Host` : 11434 par défaut, 80 ou 443 avec un schéma sans port,
/// défaut aussi pour un port invalide.
pub fn ollama_host_port(value: Option<&str>) -> Option<u16> {
    let value = value.map(env_value).unwrap_or_default();
    let (default, host_port) = match value.split_once("://") {
        None if value == "ollama.com" => (443, "ollama.com:443"),
        None => (11434, value.as_str()),
        Some(("http", rest)) => (80, rest),
        Some(("https", rest)) => (443, rest),
        Some((_, rest)) => (11434, rest),
    };
    let host_port = host_port.split('/').next().unwrap_or_default();
    let port = split_port(host_port).and_then(|port| port.parse::<u16>().ok());
    Some(port.unwrap_or(default))
}

/// Port de `hôte:port` ou `[ipv6]:port`, comme `net.SplitHostPort`.
pub(super) fn split_port(host_port: &str) -> Option<&str> {
    if let Some(rest) = host_port.strip_prefix('[') {
        let (_, after) = rest.split_once(']')?;
        return after.strip_prefix(':');
    }
    let (host, port) = host_port.rsplit_once(':')?;
    (!host.contains(':')).then_some(port)
}

/// Sockets en écoute sur `port` d'un fichier `/proc/net/tcp` ou `tcp6`, à une
/// adresse de bouclage ou « toutes interfaces ».
pub fn parse_proc_net(text: &str, port: u16) -> Vec<Listener> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (local, state, uid, inode) = (
                fields.get(1)?,
                fields.get(3)?,
                fields.get(7)?,
                fields.get(9)?,
            );
            if *state != "0A" {
                return None;
            }
            let (address, local_port) = local.split_once(':')?;
            if u16::from_str_radix(local_port, 16).ok()? != port || !reachable(address)? {
                return None;
            }
            Some(Listener {
                uid: uid.parse().ok()?,
                inode: inode.parse().ok()?,
            })
        })
        .collect()
}

/// Adresse hexadécimale du noyau (mots de 32 bits dans l'ordre de la
/// machine) : bouclage, ou toutes interfaces.
pub(super) fn reachable(hex: &str) -> Option<bool> {
    let words = (0..hex.len() / 8)
        .map(|i| u32::from_str_radix(hex.get(i * 8..i * 8 + 8)?, 16).ok())
        .collect::<Option<Vec<u32>>>()?;
    let local_v4 = |ip: Ipv4Addr| ip.is_loopback() || ip.is_unspecified();
    match words.as_slice() {
        [word] if hex.len() == 8 => Some(local_v4(Ipv4Addr::from(word.to_ne_bytes()))),
        [a, b, c, d] if hex.len() == 32 => {
            let mut bytes = [0u8; 16];
            for (chunk, word) in bytes.chunks_mut(4).zip([a, b, c, d]) {
                chunk.copy_from_slice(&word.to_ne_bytes());
            }
            let ip = Ipv6Addr::from(bytes);
            Some(
                ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.to_ipv4_mapped().is_some_and(local_v4),
            )
        }
        _ => None,
    }
}

/// Lit les faits de la machine. Bloquant.
pub(super) fn gather(url: &Url) -> Facts {
    let own_uid = fs::metadata("/proc/self").map_or(u32::MAX, |meta| meta.uid());
    let listener = url.port_or_known_default().and_then(|port| {
        ["/proc/net/tcp", "/proc/net/tcp6"]
            .iter()
            .filter_map(|file| fs::read_to_string(file).ok())
            .flat_map(|text| parse_proc_net(&text, port))
            .next()
    });
    let process = listener
        .filter(|listener| listener.uid == own_uid)
        .and_then(|listener| socket_owner(listener.inode, own_uid))
        .map(process_facts);
    let unit = match host::system_service_details() {
        Ok(Some(service)) => Some(unit_facts(service)),
        _ => None,
    };
    Facts {
        own_uid,
        listener,
        process,
        unit,
        answers: false,
        can_elevate: find_pkexec().is_some(),
    }
}

/// Processus de cet uid qui tient la socket `inode`.
pub(super) fn socket_owner(inode: u64, uid: u32) -> Option<u32> {
    let target = format!("socket:[{inode}]");
    for entry in fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse().ok()) else {
            continue;
        };
        if entry.metadata().map(|meta| meta.uid()).ok() != Some(uid) {
            continue;
        }
        let Ok(fds) = fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if fs::read_link(fd.path()).is_ok_and(|link| link.as_os_str() == target.as_str()) {
                return Some(pid);
            }
        }
    }
    None
}

pub(super) fn process_facts(pid: u32) -> ProcessFacts {
    let proc = PathBuf::from(format!("/proc/{pid}"));
    let cgroup = fs::read_to_string(proc.join("cgroup")).unwrap_or_default();
    let cgroup = cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .or_else(|| cgroup.lines().last())
        .unwrap_or_default()
        .to_owned();
    let exe = fs::read_link(proc.join("exe"))
        .ok()
        .and_then(|exe| {
            exe.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    ProcessFacts {
        cgroup,
        exe: exe.trim_end_matches(" (deleted)").to_owned(),
        env: read_environ(pid).unwrap_or_default(),
    }
}

pub(super) fn read_environ(pid: u32) -> Option<Env> {
    let bytes = fs::read(format!("/proc/{pid}/environ")).ok()?;
    let items: Vec<String> = bytes
        .split(|byte| *byte == 0)
        .map(|item| String::from_utf8_lossy(item).into_owned())
        .collect();
    Some(Env::from_assignments(items.iter().map(String::as_str)))
}

pub(super) fn unit_facts(service: ServiceDetails) -> UnitFacts {
    let env = Env::from_assignments(service.environment.iter().map(String::as_str));
    let env_file = service.environment_files.iter().fold(None, |found, file| {
        fs::read_to_string(file)
            .ok()
            .and_then(|text| env_file_models(&text))
            .map(|value| (file.clone(), value))
            .or(found)
    });
    let running = service.main_pid != 0;
    UnitFacts {
        visible_binds: home::visible_binds(&service.binds, home::source_exists),
        main_uid: running.then(|| process_uid(service.main_pid)).flatten(),
        main_env: running.then(|| read_environ(service.main_pid)).flatten(),
        user_home: home_of(&service.user),
        user_uid: uid_of(&service.user),
        env,
        env_file,
        service,
    }
}

/// Dernière affectation de `OLLAMA_MODELS` d'un fichier de `EnvironmentFile=`
/// (`export` admis, comme la règle du script).
pub fn env_file_models(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let line = line
                .strip_prefix("export")
                .filter(|rest| rest.starts_with(char::is_whitespace))
                .map_or(line, str::trim_start);
            line.strip_prefix("OLLAMA_MODELS=").map(env_value)
        })
        .next_back()
}

pub(super) fn process_uid(pid: u32) -> Option<u32> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Répertoire personnel d'un utilisateur du système (`/etc/passwd`, où
/// figurent toujours les utilisateurs de service) ; `/root` pour root.
pub(super) fn home_of(user: &str) -> Option<String> {
    if user.is_empty() || user == "root" || user == "0" {
        return Some("/root".to_owned());
    }
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        let [name, _, uid, _, _, home, ..] = fields.as_slice() else {
            return None;
        };
        (*name == user || *uid == user).then(|| (*home).to_owned())
    })
}

/// Uid d'un utilisateur du système (`/etc/passwd`, comme [`home_of`], ou
/// nombre) ; 0 pour root ou sans `User=`.
pub(super) fn uid_of(user: &str) -> Option<u32> {
    if user.is_empty() || user == "root" {
        return Some(0);
    }
    if let Ok(uid) = user.parse() {
        return Some(uid);
    }
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        let (name, _, uid) = (fields.next()?, fields.next()?, fields.next()?);
        if name == user { uid.parse().ok() } else { None }
    })
}

/// pkexec à utiliser : setuid root, à un emplacement fixe. Builds de débogage
/// et d'essai : `CHATONLM_PKEXEC` le remplace (faux pkexec ; vide : absent).
pub fn find_pkexec() -> Option<PathBuf> {
    #[cfg(any(debug_assertions, feature = "e2e"))]
    if let Some(path) = std::env::var_os("CHATONLM_PKEXEC") {
        return (!path.is_empty()).then(|| PathBuf::from(path));
    }
    PKEXEC_PATHS
        .iter()
        .map(Path::new)
        .find(|path| is_setuid_root(path))
        .map(Path::to_path_buf)
}

pub(super) fn is_setuid_root(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| {
        meta.is_file() && meta.uid() == 0 && meta.mode() & 0o4000 != 0 && meta.mode() & 0o001 != 0
    })
}

/// Détecte où Ollama range ses modèles pour l'adresse effective `url`.
pub async fn detect(url: &str) -> Detection {
    #[cfg(any(debug_assertions, feature = "e2e"))]
    if let Some(detection) = injected_detection() {
        return detection;
    }
    let Ok(parsed) = Url::parse(url) else {
        return Detection::new(ModelsLocation::Unknown { running: false }, None);
    };
    if !client::is_local(&parsed) {
        return classify(&parsed, &Facts::default());
    }
    let target = parsed.clone();
    let gathered = tauri::async_runtime::spawn_blocking(move || gather(&target));
    let mut facts = match tokio::time::timeout(FACTS_TIMEOUT, gathered).await {
        Ok(Ok(facts)) => facts,
        // Bus figé ou tâche perdue : rien de sûr.
        _ => Facts::default(),
    };
    if facts.listener.is_none() {
        facts.answers = matches!(client::probe(url).await, Probe::Ollama { .. });
    }
    classify(&parsed, &facts)
}

/// Détection imposée par les essais (builds de débogage et e2e), par exemple
/// `CHATONLM_E2E_MODELS_FACTS="launch=system;dir=/var/lib/ollama;user=ollama;group=ollama;running=yes"`.
#[cfg(any(debug_assertions, feature = "e2e"))]
pub(super) fn injected_detection() -> Option<Detection> {
    let value = std::env::var("CHATONLM_E2E_MODELS_FACTS").ok()?;
    let detection = parse_injected(&value, find_pkexec().is_some());
    if detection.is_none() {
        eprintln!("chatonlm: CHATONLM_E2E_MODELS_FACTS invalide, ignoré : {value}");
    }
    detection
}

/// `launch=system|user|manual|container|remote|unknown`, puis `dir=`,
/// `user=`, `group=`, `running=yes|no`, `managed=yes|no`, `env_file=`,
/// `host=`, `host_env=`, `protect_home=`, `protect_system=`, `ollama_dir=`
/// (`$HOME/.ollama` du service), `binds=` (destinations de `BindPaths=`)
/// (séparés par `;`). `dir=@<fichier>`, `protect_home=@<fichier>` : valeur
/// relue dans la première ligne du fichier à chaque détection, `binds=@…` :
/// une par ligne (e2e : l'état du faux service, que les scripts changent ;
/// fichier illisible ou vide : valeur inconnue).
#[cfg(any(test, debug_assertions, feature = "e2e"))]
pub(super) fn parse_injected(value: &str, can_elevate: bool) -> Option<Detection> {
    let mut fields = std::collections::HashMap::new();
    for part in value.split(';').filter(|part| !part.is_empty()) {
        let (name, value) = part.split_once('=')?;
        let known = [
            "launch",
            "dir",
            "user",
            "group",
            "running",
            "managed",
            "env_file",
            "host",
            "host_env",
            "protect_home",
            "protect_system",
            "ollama_dir",
            "binds",
        ];
        if !known.contains(&name) {
            return None;
        }
        fields.insert(name, value.to_owned());
    }
    let text = |name: &str| fields.get(name).cloned().filter(|value| !value.is_empty());
    // Lignes de la valeur, ou du fichier désigné par « @ ».
    let lines = |name: &str| -> Vec<String> {
        let Some(value) = text(name) else {
            return Vec::new();
        };
        match value.strip_prefix('@') {
            Some(file) => fs::read_to_string(file)
                .unwrap_or_default()
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect(),
            None => vec![value],
        }
    };
    let first = |name: &str| lines(name).into_iter().next();
    let dir = || first("dir");
    let flag = |name: &str| match fields.get(name).map(String::as_str) {
        None | Some("no") => Some(false),
        Some("yes") => Some(true),
        Some(_) => None,
    };
    let location = match text("launch")?.as_str() {
        "system" => ModelsLocation::SystemService {
            dir: dir(),
            user: text("user").unwrap_or_else(|| "root".into()),
            group: text("group").unwrap_or_default(),
            running: flag("running")?,
            managed: flag("managed")?,
            can_elevate,
            env_file: text("env_file"),
        },
        "user" => ModelsLocation::UserService { dir: dir() },
        "manual" => ModelsLocation::Manual {
            dir: dir(),
            host_env: text("host_env"),
        },
        "container" => ModelsLocation::Container,
        "remote" => ModelsLocation::Remote {
            host: text("host")?,
        },
        "unknown" => ModelsLocation::Unknown {
            running: flag("running")?,
        },
        _ => return None,
    };
    let protect_home = first("protect_home").unwrap_or_else(|| "no".into());
    let ollama_dir = text("ollama_dir");
    // Dossier simulé des essais, traité comme /home (comme par les scripts).
    let test_home = std::env::var("CHATONLM_TEST_HOME").ok();
    // Montages du faux service, dont la source est la destination.
    let binds: Vec<(String, String)> = lines("binds")
        .into_iter()
        .map(|bind| (bind.clone(), bind))
        .collect();
    let hidden_home = home::hidden_home_in(
        &location,
        ollama_dir.as_deref(),
        &protect_home,
        &home::visible_binds(&binds, home::source_exists),
        test_home.as_deref(),
    );
    Some(Detection {
        location,
        protect_home,
        protect_system: text("protect_system").unwrap_or_else(|| "no".into()),
        host_env: text("host_env"),
        ollama_dir,
        hidden_home,
    })
}

// --- Validation ----------------------------------------------------------------

/// Chemin absolu qu'un fichier d'unité peut citer : moins de 4096 octets,
/// sans caractère de contrôle ni `" \ : $` et accent grave (échappements de
/// systemd, séparateur de `BindPaths=`, valeurs que `systemctl show`
/// échappe). Le `%` est permis (doublé dans le drop-in). Comme `plain_path`
/// des scripts.
pub fn plain_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() < MAX_PATH
        && !path
            .chars()
            .any(|c| c.is_control() || matches!(c, '"' | '\\' | ':' | '$' | '`'))
}

/// Chemin que systemd et le script acceptent ([`plain_path`]), sans `.`,
/// `..`, `//` ni `/` final.
pub fn clean_path(path: &str) -> bool {
    plain_path(path)
        && (path == "/"
            || path[1..]
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."))
}

/// Dossiers du système et racines dont le propriétaire changerait (même
/// liste que `forbidden` du script ; `/run/media/<user>/<disque>/…` admis).
pub fn forbidden(path: &str) -> bool {
    if let Some(rest) = path.strip_prefix("/run/media/")
        && let Some((user, disk)) = rest.split_once('/')
        && !user.is_empty()
        && !disk.is_empty()
    {
        return false;
    }
    const ROOTS: [&str; 8] = [
        "/", "/home", "/media", "/mnt", "/opt", "/srv", "/var", "/var/lib",
    ];
    const TREES: [&str; 22] = [
        "/bin",
        "/boot",
        "/dev",
        "/efi",
        "/etc",
        "/lib",
        "/lib32",
        "/lib64",
        "/nix",
        "/proc",
        "/root",
        "/run",
        "/sbin",
        "/snap",
        "/sys",
        "/tmp",
        "/usr",
        "/var/cache",
        "/var/lock",
        "/var/log",
        "/var/run",
        "/var/tmp",
    ];
    ROOTS.contains(&path)
        || TREES.iter().any(|tree| {
            path == *tree
                || path
                    .strip_prefix(tree)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
}

/// Problème d'un dossier canonique, avant l'invite. `old` : dossier actuel.
pub fn folder_problem(path: &str, old: Option<&str>) -> Option<FolderProblem> {
    if !clean_path(path) {
        return Some(FolderProblem::BadCharacters);
    }
    if forbidden(path) {
        return Some(FolderProblem::System);
    }
    if let Some(old) = old {
        if path == old {
            return Some(FolderProblem::Same);
        }
        if Path::new(path).starts_with(old) || Path::new(old).starts_with(path) {
            return Some(FolderProblem::Inside);
        }
    }
    match folder_kind(Path::new(path)) {
        FolderKind::Empty | FolderKind::Models => None,
        FolderKind::Other => Some(FolderProblem::NotEmpty),
        FolderKind::NotFolder => Some(FolderProblem::Missing),
        FolderKind::Missing => {
            let parent = Path::new(path).parent().unwrap_or(Path::new("/"));
            (!parent.is_dir()).then_some(FolderProblem::Missing)
        }
    }
}

/// Forme canonique (liens résolus) ; un dossier absent garde son nom sous
/// son parent canonique.
pub(super) fn canonical(raw: &str) -> Result<String, FolderProblem> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(FolderProblem::Missing);
    }
    let resolved = match fs::canonicalize(path) {
        Ok(resolved) => resolved,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
                return Err(FolderProblem::Missing);
            };
            fs::canonicalize(parent)
                .map_err(|_| FolderProblem::Missing)?
                .join(name)
        }
        Err(_) => return Err(FolderProblem::Missing),
    };
    resolved
        .into_os_string()
        .into_string()
        .map_err(|_| FolderProblem::BadCharacters)
}

/// Dossier actuel sous la forme que le script accepte pour un déplacement
/// (canonique s'il est lisible) ; `None` s'il ne l'accepterait pas.
pub fn old_for_script(dir: &str) -> Option<String> {
    let resolved = fs::canonicalize(dir)
        .ok()
        .and_then(|path| path.into_os_string().into_string().ok())
        .unwrap_or_else(|| dir.trim_end_matches('/').to_owned());
    // Pas de liste noire : l'ancien dossier est souvent un dossier du système
    // (`/var/lib/ollama`, `/usr/share/ollama/.ollama/models`).
    clean_path(&resolved).then_some(resolved)
}

/// Dossier existant qui n'appartient ni à `own_uid` (l'app), ni à
/// `service_uid` (jamais root) : le script le refuserait (code 12), il ne
/// donne au service que ce que l'utilisateur a créé ou un dossier neuf.
pub fn foreign_folder(path: &Path, own_uid: u32, service_uid: Option<u32>) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| {
        meta.uid() != own_uid && Some(meta.uid()) != service_uid.filter(|&uid| uid != 0)
    })
}

/// Vérifie le dossier choisi `raw` (bloquant : lit le disque). Les champs
/// qui dépendent du serveur ou du reste de l'app (`models`,
/// `replies_running`, `downloads_running`) restent à remplir.
pub fn check_folder(raw: &str, detection: &Detection) -> FolderCheck {
    let own_uid = fs::metadata("/proc/self").map_or(u32::MAX, |meta| meta.uid());
    check_folder_as(raw, detection, own_uid)
}

/// [`check_folder`] pour l'utilisateur `own_uid`.
pub fn check_folder_as(raw: &str, detection: &Detection, own_uid: u32) -> FolderCheck {
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
    let old_dir = current_dir(&detection.location);
    check.old = old_dir.and_then(old_for_script);
    let old = check.old.as_deref().or(old_dir);
    check.problem = folder_problem(&path, old);
    if check.problem.is_some() {
        return check;
    }
    // Le script (ou les commandes sudo) donnera le dossier au service ;
    // l'intégré tourne sous le compte de l'utilisateur.
    let service_uid = match &detection.location {
        ModelsLocation::SystemService { user, .. } => Some(uid_of(user)),
        ModelsLocation::Builtin { .. } => Some(None),
        _ => None,
    };
    if let Some(service_uid) = service_uid
        && foreign_folder(Path::new(&path), own_uid, service_uid)
    {
        check.problem = Some(FolderProblem::NotYours);
        return check;
    }
    check.has_models = folder_kind(Path::new(&path)) == FolderKind::Models;
    let at = if Path::new(&path).is_dir() {
        PathBuf::from(&path)
    } else {
        Path::new(&path)
            .parent()
            .unwrap_or(Path::new("/"))
            .to_path_buf()
    };
    let device = |path: &Path| fs::metadata(path).ok().map(|meta| meta.dev());
    check.same_drive = old.is_some_and(|old| {
        let old = Path::new(old);
        // Dossier du service illisible pour l'app (home 0700 de l'utilisateur
        // `ollama` du script officiel) : son plus proche ancêtre lisible, sur
        // le même système de fichiers en pratique. Le script refait le calcul
        // en root (renommage ou copie, place).
        let source = device(&old.join("blobs")).or_else(|| old.ancestors().find_map(device));
        source.is_some() && source == device(&at)
    });
    check.free_bytes = crate::disk::free_space(&at);
    check.commands = commands_for(detection, &path);
    check
}

// --- Commandes à copier et drop-in --------------------------------------------

/// Lignes de bac à sable ajoutées au drop-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sandbox {
    None,
    /// `ProtectHome=tmpfs` + `BindPaths=` : dossier sous un `/home` masqué.
    Bind,
    /// `ReadWritePaths=` : `/home` en lecture seule, ou `ProtectSystem=strict`.
    ReadWrite,
}

/// Bac à sable nécessaire pour `path`, comme le script.
pub fn sandbox_for(path: &str, protect_home: &str, protect_system: &str) -> Sandbox {
    let home = if home::hidden_root(path).is_some() {
        match protect_home {
            "yes" | "true" | "tmpfs" => Sandbox::Bind,
            "read-only" => Sandbox::ReadWrite,
            _ => Sandbox::None,
        }
    } else {
        Sandbox::None
    };
    match (home, protect_system) {
        (Sandbox::None, "strict") => Sandbox::ReadWrite,
        (home, _) => home,
    }
}

/// Chemin pour un fichier d'unité : `%` doublé (spécificateurs de systemd).
pub(super) fn unit_escape(path: &str) -> String {
    path.replace('%', "%%")
}

/// Dossier `.ollama` du service à rendre visible avec le dossier des modèles
/// `path` (comme le script) : seulement avec `BindPaths=` (le reste de
/// `/home` devient un dossier vide en lecture seule, où `ollama serve` ne
/// peut ni lire ni créer sa clé), sous `/home`, `/root` ou `/run/user`, hors
/// de `path`.
pub fn home_bind<'a>(path: &str, sandbox: Sandbox, ollama_dir: Option<&'a str>) -> Option<&'a str> {
    let dir = ollama_dir.filter(|_| sandbox == Sandbox::Bind)?;
    (home::hidden_root(dir).is_some() && plain_path(dir) && !Path::new(dir).starts_with(path))
        .then_some(dir)
}

/// Drop-in écrit par le script, octet pour octet ; `home_bind` : dossier
/// `.ollama` du service ajouté aux `BindPaths=` ([`home_bind`]).
pub fn dropin_text(path: &str, sandbox: Sandbox, home_bind: Option<&str>) -> String {
    let quoted = unit_escape(path);
    let mut text = format!(
        "# Written by ChatonLM (Settings > Model location).\n[Unit]\nRequiresMountsFor=\"{quoted}\"\n\n[Service]\nEnvironment=\"OLLAMA_MODELS={quoted}\"\n"
    );
    match sandbox {
        Sandbox::None => {}
        Sandbox::Bind => {
            text.push_str(&format!("ProtectHome=tmpfs\nBindPaths=\"{quoted}\"\n"));
            if let Some(home) = home_bind {
                text.push_str(&format!("BindPaths=\"{}\"\n", unit_escape(home)));
            }
        }
        Sandbox::ReadWrite => text.push_str(&format!("ReadWritePaths=\"{quoted}\"\n")),
    }
    text
}

/// Mot cité pour le shell, entre apostrophes (`'` → `'\''`).
pub fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Mot gardé tel quel s'il n'a que des caractères sans effet pour le shell.
pub(super) fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:/[]@+,".contains(c));
    if plain {
        word.to_owned()
    } else {
        shell_quote(word)
    }
}

/// `ollama serve` relancé à la main sur `path`.
pub fn manual_command(path: &str, host_env: Option<&str>) -> String {
    let host = host_env
        .map(|host| format!("OLLAMA_HOST={} ", shell_word(host)))
        .unwrap_or_default();
    format!("{host}OLLAMA_MODELS={} ollama serve", shell_quote(path))
}

/// Drop-in d'un service de l'utilisateur.
pub fn user_service_commands(path: &str) -> String {
    let environment = format!("Environment=\"OLLAMA_MODELS={}\"", unit_escape(path));
    format!(
        "mkdir -p ~/.config/systemd/user/ollama.service.d\n\
         printf '%s\\n' '[Service]' {} > ~/.config/systemd/user/ollama.service.d/{DROPIN_NAME}\n\
         systemctl --user daemon-reload && systemctl --user restart ollama.service",
        shell_quote(&environment)
    )
}

/// Ce que fait le script (sans le déplacement), à lancer avec sudo : pkexec
/// absent, ou pas d'agent d'authentification dans la session. Même drop-in
/// que le script (il passe après `override.conf` et remplace un choix
/// précédent de ChatonLM).
pub fn sudo_commands(
    path: &str,
    user: &str,
    group: &str,
    sandbox: Sandbox,
    running: bool,
    home_bind: Option<&str>,
) -> String {
    let lines: Vec<String> = dropin_text(path, sandbox, home_bind)
        .lines()
        .map(shell_quote)
        .collect();
    let owner = shell_word(&format!("{user}:{group}"));
    let reload = if running {
        "sudo systemctl daemon-reload && sudo systemctl restart ollama.service"
    } else {
        "sudo systemctl daemon-reload"
    };
    // Dossier .ollama créé par le service lui-même, comme le script.
    let home = home_bind
        .map(|dir| {
            format!(
                "sudo -u {} mkdir -p {}\n",
                shell_word(user),
                shell_quote(dir)
            )
        })
        .unwrap_or_default();
    format!(
        "sudo mkdir -p {folder} && sudo chown {owner} {folder}\n\
         {home}sudo mkdir -p /etc/systemd/system/ollama.service.d\n\
         printf '%s\\n' {lines} | sudo tee /etc/systemd/system/ollama.service.d/{DROPIN_NAME} > /dev/null\n\
         {reload}",
        folder = shell_quote(path),
        lines = lines.join(" "),
    )
}

/// Commandes à copier pour ce lancement ; `None` quand il n'y en a pas.
pub fn commands_for(detection: &Detection, path: &str) -> Option<String> {
    match &detection.location {
        ModelsLocation::SystemService {
            user,
            group,
            running,
            ..
        } => {
            let sandbox = sandbox_for(path, &detection.protect_home, &detection.protect_system);
            let home = home_bind(path, sandbox, detection.ollama_dir.as_deref());
            Some(sudo_commands(path, user, group, sandbox, *running, home))
        }
        ModelsLocation::UserService { .. } => Some(user_service_commands(path)),
        ModelsLocation::Manual { host_env, .. } => Some(manual_command(path, host_env.as_deref())),
        ModelsLocation::Unknown { running: true } => {
            Some(manual_command(path, detection.host_env.as_deref()))
        }
        _ => None,
    }
}

// --- Exécution ------------------------------------------------------------------

/// Changement à exécuter.
#[derive(Clone, Debug)]
pub struct Request {
    pub pkexec: PathBuf,
    pub shell: PathBuf,
    pub move_models: bool,
    /// Ancien dossier (déplacement seulement).
    pub old: Option<String>,
    pub new: String,
    /// Variables ajoutées à l'environnement (essais).
    pub envs: Vec<(OsString, OsString)>,
}

/// Ligne de la sortie standard du script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Step(MoveStep),
    Total(u64),
    Copy { bytes: u64, path: String },
    Restored,
    Result(ScriptResult),
}

/// Ligne `result …`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptResult {
    pub restarted: bool,
    pub renamed: u32,
    pub copied: u32,
    pub skipped: u32,
    pub cleanup_failed: bool,
}

pub fn parse_line(line: &str) -> Option<Line> {
    let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
    match word {
        "step" => Some(Line::Step(match rest {
            "prepare" => MoveStep::Prepare,
            "stop" => MoveStep::Stop,
            "move" => MoveStep::Move,
            "config" => MoveStep::Config,
            "start" => MoveStep::Start,
            "cleanup" => MoveStep::Cleanup,
            _ => return None,
        })),
        "total" => rest.parse().ok().map(Line::Total),
        "copy" => {
            let (bytes, path) = rest.split_once(' ')?;
            Some(Line::Copy {
                bytes: bytes.parse().ok()?,
                path: path.to_owned(),
            })
        }
        "restored" if rest.is_empty() => Some(Line::Restored),
        "result" => {
            let mut result = ScriptResult::default();
            for pair in rest.split(' ') {
                let (name, value) = pair.split_once('=')?;
                match name {
                    "restarted" => result.restarted = value == "yes",
                    "renamed" => result.renamed = value.parse().ok()?,
                    "copied" => result.copied = value.parse().ok()?,
                    "skipped" => result.skipped = value.parse().ok()?,
                    "cleanup" => result.cleanup_failed = value == "failed",
                    _ => {}
                }
            }
            Some(Line::Result(result))
        }
        _ => None,
    }
}

/// Fin d'après le code de sortie de pkexec (celui du script, ou 126/127 de
/// pkexec), sa sortie d'erreur et la ligne `result`.
pub fn outcome(code: Option<i32>, detail: &str, result: Option<ScriptResult>) -> MoveOutcome {
    let failed = |reason, restored| MoveOutcome::Failed {
        reason,
        restored,
        detail: detail.to_owned(),
    };
    match code {
        Some(0) => match result {
            Some(result) => MoveOutcome::Done {
                restarted: result.restarted,
                renamed: result.renamed,
                copied: result.copied,
                skipped: result.skipped,
                cleanup_failed: result.cleanup_failed,
            },
            None => failed(MoveFailure::Unexpected, false),
        },
        Some(126) => MoveOutcome::Cancelled,
        Some(127) if detail.contains("No authentication agent") => {
            failed(MoveFailure::NoAgent, false)
        }
        Some(127) => failed(MoveFailure::NotAuthorized, false),
        Some(10 | 12) => failed(MoveFailure::Refused, false),
        Some(11) => failed(MoveFailure::UnitChanged, false),
        Some(13) => failed(MoveFailure::NotEmpty, false),
        Some(14) => {
            let (need, free) = space_from_detail(detail);
            failed(MoveFailure::NoSpace { need, free }, false)
        }
        Some(15) => failed(MoveFailure::CannotWrite, false),
        Some(16) => failed(
            MoveFailure::EnvironmentFile {
                file: detail.lines().find_map(|line| {
                    line.split_once("OLLAMA_MODELS is set in ")
                        .map(|(_, file)| file.to_owned())
                }),
            },
            false,
        ),
        Some(20) => failed(MoveFailure::StopFailed, true),
        Some(21) => failed(MoveFailure::MoveFailed, true),
        Some(22) => failed(MoveFailure::ConfigFailed, true),
        Some(23) => failed(MoveFailure::Overridden, true),
        Some(24) => failed(MoveFailure::StartFailed, true),
        Some(30) => failed(MoveFailure::RestoreFailed, false),
        _ => failed(MoveFailure::Unexpected, false),
    }
}

/// « not enough space: N KiB needed, M KiB free » → octets.
pub(super) fn space_from_detail(detail: &str) -> (Option<u64>, Option<u64>) {
    let Some((_, rest)) = detail.split_once("not enough space: ") else {
        return (None, None);
    };
    let kib = |text: &str| {
        text.split_whitespace()
            .next()
            .and_then(|n| n.parse::<u64>().ok())
            .map(|n| n.saturating_mul(1024))
    };
    let (need, free) = rest.split_once(", ").unwrap_or((rest, ""));
    (kib(need), kib(free))
}

/// Suit la sortie standard : étapes, progression (taille du fichier en cours
/// relevée toutes les 500 ms : un modèle tient souvent en un seul fichier de
/// plusieurs Go).
pub(super) struct Reader {
    new: PathBuf,
    completed: u64,
    current: Option<(u64, String)>,
    last_done: Option<u64>,
    result: Option<ScriptResult>,
}

impl Reader {
    fn handle(
        &mut self,
        line: &str,
        on_event: &mut impl FnMut(MoveEvent),
        on_stop: &mut impl FnMut(),
    ) {
        match parse_line(line) {
            Some(Line::Step(step)) => {
                if step != MoveStep::Move && self.current.is_some() {
                    self.finish_current(on_event);
                }
                if step == MoveStep::Stop {
                    on_stop();
                }
                on_event(MoveEvent::Step { step });
            }
            Some(Line::Total(bytes)) => on_event(MoveEvent::Total { bytes }),
            Some(Line::Copy { bytes, path }) => {
                self.finish_current(on_event);
                self.current = Some((bytes, path));
            }
            Some(Line::Result(result)) => self.result = Some(result),
            Some(Line::Restored) | None => {}
        }
    }

    fn finish_current(&mut self, on_event: &mut impl FnMut(MoveEvent)) {
        if let Some((bytes, _)) = self.current.take() {
            self.completed = self.completed.saturating_add(bytes);
        }
        self.send(self.completed, on_event);
    }

    fn poll(&mut self, on_event: &mut impl FnMut(MoveEvent)) {
        let Some((bytes, path)) = &self.current else {
            return;
        };
        let size = fs::metadata(self.new.join(path)).map_or(0, |meta| meta.len());
        self.send(self.completed.saturating_add(size.min(*bytes)), on_event);
    }

    fn send(&mut self, done: u64, on_event: &mut impl FnMut(MoveEvent)) {
        if self.last_done != Some(done) {
            self.last_done = Some(done);
            on_event(MoveEvent::Progress { done });
        }
    }
}

/// Lance le script par pkexec et suit son déroulement. `on_stop` est appelé
/// quand le script va arrêter le service (les réponses en cours s'arrêtent
/// alors proprement). Le processus n'est jamais tué : le script root doit
/// aller au bout (il ignore aussi SIGHUP et SIGPIPE).
pub async fn run(
    request: Request,
    on_event: impl FnMut(MoveEvent),
    on_stop: impl FnMut(),
) -> MoveOutcome {
    run_with(request, on_event, on_stop, |_| {}).await
}

/// [`run`], avec chaque ligne de la sortie standard remise à `on_line`
/// (essais).
pub(super) async fn run_with(
    request: Request,
    mut on_event: impl FnMut(MoveEvent),
    mut on_stop: impl FnMut(),
    mut on_line: impl FnMut(&str),
) -> MoveOutcome {
    let unexpected = |detail: String| MoveOutcome::Failed {
        reason: MoveFailure::Unexpected,
        restored: false,
        detail,
    };
    let old = match (&request.old, request.move_models) {
        (Some(old), true) => old.as_str(),
        _ => "-",
    };
    let mut command = tokio::process::Command::new(&request.pkexec);
    command
        .arg("--disable-internal-agent")
        .arg(&request.shell)
        .arg("-c")
        .arg(SCRIPT)
        .arg("chatonlm-models")
        .arg(UNIT)
        .arg(if request.move_models { "move" } else { "keep" })
        .arg(old)
        // En dernier : la fin de la ligne affichée par la boîte polkit.
        .arg(&request.new)
        .envs(request.envs)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Variables du banc d'essai : jamais transmises en production (le vrai
    // pkexec les retirerait de toute façon).
    #[cfg(not(any(debug_assertions, feature = "e2e")))]
    for name in [
        "CHATONLM_TEST_ROOT",
        "CHATONLM_TEST_BIN",
        "CHATONLM_TEST_FORCE_COPY",
        "CHATONLM_TEST_HOME",
    ] {
        command.env_remove(name);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return unexpected(format!(
                "could not start {}: {error}",
                request.pkexec.display()
            ));
        }
    };
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return unexpected("no output from pkexec".into());
    };
    let mut reader = Reader {
        new: PathBuf::from(&request.new),
        completed: 0,
        current: None,
        last_done: None,
        result: None,
    };
    let follow = async {
        let mut stdout = BufReader::new(stdout);
        // Ligne lue en octets : un nom de fichier non UTF-8 (ligne `copy`)
        // ne doit pas arrêter la lecture (la ligne `result` serait perdue et
        // un déplacement réussi rapporté comme un échec). `read_until` garde
        // dans `line` ce qu'il a lu si l'attente est interrompue par le
        // relevé périodique.
        let mut line = Vec::new();
        let mut tick = tokio::time::interval(PROGRESS_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                read = stdout.read_until(b'\n', &mut line) => {
                    let ended = !matches!(read, Ok(read) if read > 0);
                    if !line.is_empty() {
                        let text = String::from_utf8_lossy(&line);
                        let text = text.strip_suffix('\n').unwrap_or(&text);
                        let text = text.strip_suffix('\r').unwrap_or(text);
                        on_line(text);
                        reader.handle(text, &mut on_event, &mut on_stop);
                        line.clear();
                    }
                    if ended {
                        break;
                    }
                },
                _ = tick.tick() => reader.poll(&mut on_event),
            }
        }
    };
    let ((), detail) = tokio::join!(follow, read_limited(stderr, MAX_DETAIL));
    let code = child.wait().await.ok().and_then(|status| status.code());
    outcome(code, &detail, reader.result)
}

/// Lit tout le flux (le script ne doit pas bloquer sur une sortie pleine),
/// en gardant les `limit` premiers octets.
pub(super) async fn read_limited(mut stream: impl AsyncRead + Unpin, limit: usize) -> String {
    let mut kept = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let room = limit.saturating_sub(kept.len());
                kept.extend_from_slice(&buffer[..read.min(room)]);
            }
        }
    }
    String::from_utf8_lossy(&kept).trim().to_owned()
}

// Essais à côté de `models_dir.rs`, comme avant le découpage.
#[cfg(test)]
#[path = "bench.rs"]
mod bench;
#[cfg(test)]
#[path = "tests.rs"]
pub(super) mod tests;
