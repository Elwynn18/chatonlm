// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Inspired by Ollama (https://github.com/ollama/ollama), files
// cmd/start_windows.go (startApp: where "ollama app.exe" is looked for, its
// "--hide --fast-startup" arguments, no start while OllamaSetup.exe runs),
// app/ollama.iss (AppId, install folder, ollama:// protocol) and
// app/wintray/tray.go (window class of the tray application), commit
// 42e911bc3d05.
// Modified for ChatonLM: the application is also found through its
// uninstall entry and its protocol handler (custom install folder); it is
// described with the types of a systemd service so that the welcome screen
// decides the same way on Linux and Windows.

//! App de bureau d'Ollama sous Windows (`ollama app.exe`) : logique pure, sans
//! appel système, essayée aussi sous Linux. Les appels Win32 sont dans
//! `host/windows.rs`.

#![cfg_attr(not(windows), allow(dead_code))]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Service, Unit};

/// App de bureau (zone de notification), qui lance `ollama serve`.
pub const APP_EXE: &str = "ollama app.exe";

/// Ligne de commande d'Ollama.
pub const CLI_EXE: &str = "ollama.exe";

/// Installateur officiel : tant qu'il tourne, l'app n'est pas lancée.
pub const INSTALLER_EXE: &str = "OllamaSetup.exe";

/// Arguments du lancement, comme `startApp` de l'officiel : sans fenêtre,
/// sans installer une mise à jour en attente.
pub const START_ARGS: [&str; 2] = ["--hide", "--fast-startup"];

/// Classe de la fenêtre (invisible) de la zone de notification de l'app.
pub const WINDOW_CLASS: &str = "OllamaClass";

/// Désinstallation d'Inno Setup, sous `HKCU` (`AppId` de `ollama.iss` suivi
/// de `_is1`) : valeur `InstallLocation`.
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{44E83376-CE68-45EB-8FC1-393500EB558C}_is1";

/// Commande du protocole `ollama://`, sous `HKCU` (valeur par défaut).
pub const PROTOCOL_KEY: &str = r"Software\Classes\ollama\shell\open\command";

/// Pendant ce temps après son lancement, l'app est dite « en démarrage » ;
/// au-delà, elle tourne sans que son serveur réponde à l'adresse.
pub const STARTING_FOR: Duration = Duration::from_secs(30);

/// Ce que la machine dit de l'installation.
#[derive(Clone, Debug, Default)]
pub struct Clues {
    /// `%LOCALAPPDATA%`.
    pub local_app_data: Option<PathBuf>,
    /// `InstallLocation` de la clé de désinstallation.
    pub install_location: Option<String>,
    /// Commande du protocole `ollama://`.
    pub protocol_command: Option<String>,
    /// Dossiers du `PATH`.
    pub path_dirs: Vec<PathBuf>,
}

/// Dossiers où l'installateur a pu poser Ollama, par ordre de préférence :
/// dossier par défaut, dossier choisi (`OllamaSetup.exe /DIR=…`, connu par
/// la désinstallation ou le protocole), ancien dossier, dossiers du `PATH`
/// (l'installateur y ajoute le sien).
pub fn install_dirs(clues: &Clues) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut add = |dir: PathBuf| {
        if !dir.as_os_str().is_empty() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    };
    if let Some(local) = &clues.local_app_data {
        add(local.join("Programs").join("Ollama"));
    }
    if let Some(location) = clues.install_location.as_deref().map(unquoted) {
        add(PathBuf::from(location));
    }
    if let Some(dir) = clues
        .protocol_command
        .as_deref()
        .and_then(protocol_exe)
        .and_then(|exe| parent_of(&exe))
    {
        add(dir);
    }
    if let Some(local) = &clues.local_app_data {
        add(local.join("Ollama"));
    }
    for dir in &clues.path_dirs {
        add(dir.clone());
    }
    dirs
}

/// Emplacements possibles de l'app de bureau.
pub fn app_candidates(clues: &Clues) -> Vec<PathBuf> {
    install_dirs(clues)
        .into_iter()
        .map(|dir| dir.join(APP_EXE))
        .collect()
}

/// Emplacements possibles d'`ollama.exe` : le `PATH` d'abord (ce que lance
/// « ollama serve » dans un terminal), puis les dossiers d'installation.
pub fn cli_candidates(clues: &Clues) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = clues
        .path_dirs
        .iter()
        .map(|dir| dir.join(CLI_EXE))
        .collect();
    for dir in install_dirs(clues) {
        let candidate = dir.join(CLI_EXE);
        if !found.contains(&candidate) {
            found.push(candidate);
        }
    }
    found
}

/// Exécutable d'une commande du registre : `"C:\…\ollama app.exe" "%1"`, ou
/// sans guillemets jusqu'à `.exe`.
pub fn protocol_exe(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    let exe = match command.strip_prefix('"') {
        Some(rest) => rest.split_once('"')?.0,
        None => {
            let end = command.to_ascii_lowercase().find(".exe")? + 4;
            command.get(..end)?
        }
    };
    (!exe.is_empty()).then(|| PathBuf::from(exe))
}

/// Sans guillemets ni séparateur final (Inno Setup écrit `C:\…\Ollama\`).
fn unquoted(text: &str) -> &str {
    text.trim().trim_matches('"').trim_end_matches(['\\', '/'])
}

/// Dossier d'un chemin Windows (séparateur `\` ou `/`), aussi sous Linux où
/// `Path::parent` ne connaît pas `\`.
pub fn parent_of(path: &Path) -> Option<PathBuf> {
    let text = path.to_str()?;
    let end = text.rfind(['\\', '/'])?;
    (end > 0).then(|| PathBuf::from(&text[..end]))
}

/// Raccourci de lancement à l'ouverture de session, sous `%APPDATA%` (créé
/// par l'app à son premier lancement).
pub fn startup_link(app_data: &Path) -> PathBuf {
    app_data
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join("Ollama.lnk")
}

/// État de l'app de bureau.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppFacts {
    /// `ollama app.exe` est là.
    pub installed: bool,
    /// Sa fenêtre de zone de notification existe : l'app est lancée.
    pub running: bool,
    /// Depuis quand elle tourne, si on le sait.
    pub running_for: Option<Duration>,
    /// Lancée à l'ouverture de session (raccourci présent).
    pub at_login: bool,
}

/// L'app vue comme un service de l'utilisateur, avec les mots de systemd
/// que lit `detect::service_state` : arrêtée (`inactive`), en démarrage
/// (`activating`, les 30 premières secondes), lancée (`active` : si rien ne
/// répond alors, l'écran dit « Ollama ne répond pas »).
pub fn unit(facts: AppFacts) -> Unit {
    if !facts.installed {
        return Unit::NotFound;
    }
    let (active, sub) = match (facts.running, facts.running_for) {
        (false, _) => ("inactive", "dead"),
        (true, Some(age)) if age < STARTING_FOR => ("activating", "start"),
        (true, _) => ("active", "running"),
    };
    Unit::Loaded(Service {
        user: true,
        active_state: active.to_owned(),
        sub_state: sub.to_owned(),
        unit_file_state: if facts.at_login {
            "enabled"
        } else {
            "disabled"
        }
        .to_owned(),
    })
}

/// `PATH` donné à l'app lancée : son dossier d'abord, comme le fait
/// l'installateur à la fin de l'installation (`set PATH={app};%PATH%`).
pub fn path_with(dir: &Path, current: Option<&OsStr>) -> std::ffi::OsString {
    let mut path = dir.as_os_str().to_owned();
    if let Some(current) = current.filter(|current| !current.is_empty()) {
        path.push(";");
        path.push(current);
    }
    path
}

/// Même nom de programme, sans tenir compte de la casse (`OLLAMA.EXE`).
pub fn same_name(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Port d'une ligne de la table TCP de Windows (`dwLocalPort` : 16 bits de
/// poids faible, dans l'ordre du réseau).
pub fn table_port(raw: u32) -> u16 {
    u16::from_be((raw & 0xFFFF) as u16)
}

/// Adresse IPv4 d'écoute joignable par `localhost` : boucle locale ou
/// toutes les interfaces.
pub fn local_v4(address: [u8; 4]) -> bool {
    let ip = std::net::Ipv4Addr::from(address);
    ip.is_loopback() || ip.is_unspecified()
}

/// Comme [`local_v4`], en IPv6 (adresses IPv4 projetées comprises).
pub fn local_v6(address: [u8; 16]) -> bool {
    let ip = std::net::Ipv6Addr::from(address);
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.to_ipv4_mapped().is_some_and(|v4| local_v4(v4.octets()))
}

/// Processus qui écoute sur `port` d'après une table
/// `MIB_TCPTABLE_OWNER_PID` (IPv4) lue par mots de 32 bits : nombre de
/// lignes, puis par ligne état, adresse locale, port local, adresse et port
/// distants, processus. Seulement une écoute joignable par `localhost`.
pub fn listener_v4(words: &[u32], port: u16) -> Option<u32> {
    let count = *words.first()? as usize;
    let (rows, _) = words.get(1..)?.as_chunks::<6>();
    rows.iter()
        .take(count)
        .find(|row| table_port(row[2]) == port && local_v4(row[1].to_ne_bytes()))
        .map(|row| row[5])
}

/// Comme [`listener_v4`], pour une table `MIB_TCP6TABLE_OWNER_PID` : par
/// ligne adresse locale (16 octets), zone, port local, adresse distante,
/// zone, port distant, état, processus.
pub fn listener_v6(words: &[u32], port: u16) -> Option<u32> {
    let count = *words.first()? as usize;
    let (rows, _) = words.get(1..)?.as_chunks::<14>();
    rows.iter()
        .take(count)
        .find(|row| {
            let mut address = [0u8; 16];
            for (index, word) in row[..4].iter().enumerate() {
                address[index * 4..index * 4 + 4].copy_from_slice(&word.to_ne_bytes());
            }
            table_port(row[5]) == port && local_v6(address)
        })
        .map(|row| row[13])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clues() -> Clues {
        Clues {
            local_app_data: Some(PathBuf::from(r"C:\Users\me\AppData\Local")),
            install_location: Some(r"D:\Apps\Ollama\".to_owned()),
            protocol_command: Some(r#""E:\Tools\Ollama\ollama app.exe" "%1""#.to_owned()),
            path_dirs: vec![PathBuf::from(r"C:\Windows"), PathBuf::from(r"F:\bin")],
        }
    }

    #[test]
    fn the_app_is_looked_for_where_the_installer_puts_it() {
        let found = app_candidates(&clues());
        let local = PathBuf::from(r"C:\Users\me\AppData\Local");
        assert_eq!(
            found,
            vec![
                local.join("Programs").join("Ollama").join(APP_EXE),
                PathBuf::from(r"D:\Apps\Ollama").join(APP_EXE),
                PathBuf::from(r"E:\Tools\Ollama").join(APP_EXE),
                local.join("Ollama").join(APP_EXE),
                PathBuf::from(r"C:\Windows").join(APP_EXE),
                PathBuf::from(r"F:\bin").join(APP_EXE),
            ]
        );
        // Rien de connu : rien à essayer.
        assert!(app_candidates(&Clues::default()).is_empty());
    }

    #[test]
    fn the_command_line_tool_is_looked_for_in_the_path_first() {
        let found = cli_candidates(&clues());
        assert_eq!(found[0], PathBuf::from(r"C:\Windows").join(CLI_EXE));
        assert_eq!(found[1], PathBuf::from(r"F:\bin").join(CLI_EXE));
        assert!(found.contains(&PathBuf::from(r"D:\Apps\Ollama").join(CLI_EXE)));
        assert_eq!(found.len(), 6);
    }

    #[test]
    fn registry_commands_give_their_executable() {
        assert_eq!(
            protocol_exe(r#""C:\Program Files\Ollama\ollama app.exe" "%1""#),
            Some(PathBuf::from(r"C:\Program Files\Ollama\ollama app.exe"))
        );
        assert_eq!(
            protocol_exe(r"C:\Ollama\ollama app.EXE %1"),
            Some(PathBuf::from(r"C:\Ollama\ollama app.EXE"))
        );
        assert_eq!(protocol_exe(r#""""#), None);
        assert_eq!(protocol_exe("no program here"), None);
        assert_eq!(
            parent_of(Path::new(r"C:\Ollama\ollama app.exe")),
            Some(PathBuf::from(r"C:\Ollama"))
        );
        assert_eq!(parent_of(Path::new("ollama.exe")), None);
    }

    #[test]
    fn the_app_is_described_like_a_user_service() {
        assert_eq!(unit(AppFacts::default()), Unit::NotFound);
        let state = |facts| match unit(facts) {
            Unit::Loaded(service) => {
                assert!(service.user);
                (service.active_state, service.unit_file_state)
            }
            other => panic!("{other:?}"),
        };
        let installed = AppFacts {
            installed: true,
            ..AppFacts::default()
        };
        assert_eq!(state(installed), ("inactive".into(), "disabled".into()));
        let starting = AppFacts {
            running: true,
            running_for: Some(Duration::from_secs(3)),
            at_login: true,
            ..installed
        };
        assert_eq!(state(starting), ("activating".into(), "enabled".into()));
        let old = AppFacts {
            running_for: Some(Duration::from_secs(600)),
            ..starting
        };
        assert_eq!(state(old).0, "active");
        // Âge inconnu : pas « en démarrage » sans fin.
        let unknown = AppFacts {
            running_for: None,
            ..starting
        };
        assert_eq!(state(unknown).0, "active");
    }

    #[test]
    fn listening_sockets_of_this_machine() {
        // 11434 dans l'ordre du réseau, tel que le rend GetExtendedTcpTable
        // sur une machine petit-boutiste.
        let raw = u32::from(u16::to_be(11434));
        assert_eq!(table_port(raw), 11434);
        assert_eq!(table_port(raw | 0xABCD_0000), 11434);
        assert!(local_v4([127, 0, 0, 1]) && local_v4([0, 0, 0, 0]));
        assert!(!local_v4([192, 168, 1, 10]));
        assert!(local_v6(std::net::Ipv6Addr::LOCALHOST.octets()));
        assert!(local_v6(std::net::Ipv6Addr::UNSPECIFIED.octets()));
        assert!(!local_v6(
            "fe80::1".parse::<std::net::Ipv6Addr>().unwrap().octets()
        ));
    }

    #[test]
    fn the_listener_of_a_port_is_read_from_the_tcp_tables() {
        let port = |port: u16| u32::from(u16::to_be(port));
        let v4 = |address: [u8; 4]| u32::from_ne_bytes(address);
        let table = [
            3,
            // Écoute d'une autre interface seulement : pas joignable ici.
            2,
            v4([192, 168, 1, 10]),
            port(11434),
            0,
            0,
            111,
            2,
            v4([127, 0, 0, 1]),
            port(11434),
            0,
            0,
            4242,
            2,
            v4([0, 0, 0, 0]),
            port(8080),
            0,
            0,
            7,
            // Au-delà du nombre de lignes annoncé : reste du tampon, ignoré.
            2,
            v4([0, 0, 0, 0]),
            port(9999),
            0,
            0,
            9,
        ];
        assert_eq!(listener_v4(&table, 11434), Some(4242));
        assert_eq!(listener_v4(&table, 8080), Some(7));
        assert_eq!(listener_v4(&table, 9999), None);
        assert_eq!(listener_v4(&table, 1), None);
        assert_eq!(listener_v4(&[], 11434), None);

        let loopback = std::net::Ipv6Addr::LOCALHOST.octets();
        let mut row = vec![0u32; 14];
        for (word, bytes) in row.iter_mut().zip(loopback.as_chunks::<4>().0) {
            *word = u32::from_ne_bytes(*bytes);
        }
        row[5] = port(11434);
        row[13] = 77;
        let mut table6 = vec![1u32];
        table6.extend(row);
        assert_eq!(listener_v6(&table6, 11434), Some(77));
        assert_eq!(listener_v6(&table6, 11435), None);
    }

    #[test]
    fn the_app_gets_its_folder_first_in_the_path() {
        let dir = Path::new(r"C:\Ollama");
        assert_eq!(
            path_with(dir, Some(OsStr::new(r"C:\Windows;C:\bin"))),
            std::ffi::OsString::from(r"C:\Ollama;C:\Windows;C:\bin")
        );
        assert_eq!(path_with(dir, None), std::ffi::OsString::from(r"C:\Ollama"));
        assert!(same_name("OLLAMA.EXE", CLI_EXE) && !same_name("ollama", CLI_EXE));
    }
}
