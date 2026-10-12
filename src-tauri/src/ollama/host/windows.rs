// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// start_unit translated to Rust from Ollama
// (https://github.com/ollama/ollama), file cmd/start_windows.go (startApp:
// "ollama app.exe --hide --fast-startup", refused while OllamaSetup.exe
// runs), commit 42e911bc3d05; the tray window lookup follows
// app/wintray/eventloop.go (findExistingInstance).
// Modified for ChatonLM: the application is started directly (no cmd.exe)
// with its folder first in PATH like the installer does, and is never
// stopped by ChatonLM; it is also found through its uninstall entry.

//! Installation d'Ollama sous Windows. Il n'y a pas de service : l'app de
//! bureau (`ollama app.exe`, zone de notification) lance `ollama serve`.
//! Rien n'est exécuté pour la détection : fichiers, registre de
//! l'utilisateur, fenêtre de la zone de notification, table TCP du système.
//! La logique (où chercher, quel état) est dans [`super::app`].

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, E_ACCESSDENIED, FILETIME, HANDLE};
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId};
use windows::core::{PCWSTR, PWSTR};

use super::app::{self, AppFacts, Clues};
use super::{StartResult, Unit};
use crate::i18n::t;
use crate::os_util::{from_wide, wide};

/// Descripteur Win32 fermé à la fin.
pub(crate) struct Owned(pub HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY : descripteur ouvert par ce module, fermé une seule fois.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

// SAFETY : un descripteur du noyau s'emploie de n'importe quel fil.
unsafe impl Send for Owned {}
// SAFETY : idem ; les appels faits dessus sont sûrs entre fils.
unsafe impl Sync for Owned {}

fn clues() -> Clues {
    let registry = |key: &str, name: &str| {
        windows_registry::CURRENT_USER
            .open(key)
            .and_then(|key| key.get_string(name))
            .ok()
    };
    Clues {
        local_app_data: std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        install_location: registry(app::UNINSTALL_KEY, "InstallLocation"),
        protocol_command: registry(app::PROTOCOL_KEY, ""),
        path_dirs: std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default(),
    }
}

/// App de bureau d'Ollama installée (`ollama app.exe`).
pub fn find_app() -> Option<PathBuf> {
    app::app_candidates(&clues())
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// Cherche `ollama.exe` dans le PATH, puis dans les dossiers
/// d'installation.
pub fn find_binary() -> Option<PathBuf> {
    app::cli_candidates(&clues())
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// État de l'app de bureau d'Ollama, décrite comme un service de
/// l'utilisateur (voir [`app::unit`]). Bloquant, sans attente.
pub fn ollama_unit() -> Unit {
    let running_for = tray_process().map(process_age);
    let running = running_for.is_some();
    app::unit(AppFacts {
        // Lancée d'un dossier que rien n'indique : installée quand même.
        installed: running || find_app().is_some(),
        running,
        running_for: running_for.flatten(),
        at_login: std::env::var_os("APPDATA")
            .is_some_and(|data| app::startup_link(&PathBuf::from(data)).exists()),
    })
}

/// Processus de la fenêtre de zone de notification de l'app, si elle
/// tourne.
fn tray_process() -> Option<u32> {
    let class = wide(app::WINDOW_CLASS);
    // SAFETY : chaîne UTF-16 terminée par un zéro, vivante pendant l'appel.
    let window = unsafe { FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) }.ok()?;
    if window.is_invalid() {
        return None;
    }
    let mut pid = 0u32;
    // SAFETY : fenêtre rendue par FindWindowW, sortie de 4 octets.
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    Some(pid)
}

/// Temps écoulé depuis la création du processus.
fn process_age(pid: u32) -> Option<Duration> {
    if pid == 0 {
        return None;
    }
    // SAFETY : droits de lecture seule ; descripteur fermé par `Owned`.
    let process =
        Owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?);
    let mut times = [FILETIME::default(); 4];
    let [created, exited, kernel, user] = &mut times;
    // SAFETY : quatre structures de sortie valides.
    unsafe { GetProcessTimes(process.0, created, exited, kernel, user) }.ok()?;
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    // SAFETY : appel sans argument.
    let now = ticks(unsafe { GetSystemTimeAsFileTime() });
    // Unités de 100 ns.
    Some(Duration::from_micros(
        now.saturating_sub(ticks(times[0])) / 10,
    ))
}

/// « Start Ollama » : lance l'app de bureau comme la commande `ollama`
/// officielle (sans fenêtre, sans installer une mise à jour en attente).
/// L'app n'appartient pas à ChatonLM : elle lui survit et n'est jamais
/// arrêtée par lui. Bloquant, sans attente.
pub fn start_unit(_user: bool) -> StartResult {
    if processes()
        .iter()
        .any(|process| app::same_name(&process.name, app::INSTALLER_EXE))
    {
        return StartResult::Failed(t!("builtinOllama.upgradeInProgress"));
    }
    let Some(exe) = find_app() else {
        return StartResult::NoUnit;
    };
    let mut command = Command::new(&exe);
    command
        .args(app::START_ARGS)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(dir) = exe.parent() {
        command.current_dir(dir).env(
            "PATH",
            app::path_with(dir, std::env::var_os("PATH").as_deref()),
        );
    }
    match command.spawn() {
        // Le descripteur est lâché : l'app continue seule.
        Ok(_child) => StartResult::Started,
        Err(error) => StartResult::Failed(error.to_string()),
    }
}

/// Processus de la machine.
pub(crate) struct Process {
    pub pid: u32,
    pub parent: u32,
    /// Nom de l'exécutable, sans dossier.
    pub name: String,
}

/// Liste des processus (instantané du système) ; vide si elle est illisible.
pub(crate) fn processes() -> Vec<Process> {
    // SAFETY : instantané fermé par `Owned`.
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return Vec::new();
    };
    let snapshot = Owned(snapshot);
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = Vec::new();
    // SAFETY : structure de sortie à la taille annoncée.
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) }.is_ok();
    while more {
        found.push(Process {
            pid: entry.th32ProcessID,
            parent: entry.th32ParentProcessID,
            name: from_wide(&entry.szExeFile),
        });
        // SAFETY : idem.
        more = unsafe { Process32NextW(snapshot.0, &mut entry) }.is_ok();
    }
    found
}

/// Exécutable d'un processus (chemin complet). `Err(true)` : le système
/// refuse de le dire (processus d'un autre compte, service).
pub(crate) fn process_image(pid: u32) -> Result<PathBuf, bool> {
    // SAFETY : droits de lecture seule ; descripteur fermé par `Owned`.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|error| error.code() == E_ACCESSDENIED)?;
    let process = Owned(process);
    image_of(process.0)
}

/// Exécutable du processus d'un descripteur déjà ouvert (avec au moins
/// `PROCESS_QUERY_LIMITED_INFORMATION`) : ce que l'appelant fait ensuite du
/// même descripteur vise bien ce processus, même si son numéro est repris.
pub(crate) fn image_of(process: HANDLE) -> Result<PathBuf, bool> {
    let mut buffer = vec![0u16; 32768];
    let mut length = buffer.len() as u32;
    // SAFETY : tampon de `length` unités, longueur écrite en retour ;
    // descripteur tenu par l'appelant.
    unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    }
    .map_err(|error| error.code() == E_ACCESSDENIED)?;
    buffer.truncate(length as usize);
    Ok(PathBuf::from(from_wide(&buffer)))
}

/// Table des écoutes TCP d'une famille d'adresses, par mots de 32 bits.
fn tcp_listeners(family: u32) -> Option<Vec<u32>> {
    const NO_ERROR: u32 = 0;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    let mut bytes = 0u32;
    // SAFETY : sans tampon, l'appel rend la taille nécessaire.
    unsafe {
        GetExtendedTcpTable(
            None,
            &mut bytes,
            false,
            family,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    // La table peut grandir entre deux appels.
    for _ in 0..3 {
        let mut words = vec![0u32; bytes as usize / 4 + 256];
        let mut size = (words.len() * 4) as u32;
        // SAFETY : tampon de `size` octets, aligné sur 4 octets comme les
        // structures de la table.
        let code = unsafe {
            GetExtendedTcpTable(
                Some(words.as_mut_ptr().cast()),
                &mut size,
                false,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        match code {
            NO_ERROR => return Some(words),
            ERROR_INSUFFICIENT_BUFFER => bytes = size,
            _ => return None,
        }
    }
    None
}

/// Processus qui écoute sur ce port TCP de la machine (boucle locale ou
/// toutes les interfaces) : `Some(None)` si rien n'écoute, `None` si la
/// table du système est illisible.
pub(crate) fn listener_pid(port: u16) -> Option<Option<u32>> {
    let v4 = tcp_listeners(u32::from(AF_INET.0));
    let v6 = tcp_listeners(u32::from(AF_INET6.0));
    if v4.is_none() && v6.is_none() {
        return None;
    }
    Some(
        v4.and_then(|words| app::listener_v4(&words, port))
            .or_else(|| v6.and_then(|words| app::listener_v6(&words, port))),
    )
}

/// Quelque chose écoute-t-il sur ce port TCP de la machine ? `None` : table
/// du système illisible. Sous Windows, une connexion vers un port fermé de
/// la boucle locale met jusqu'à 2 s à échouer : la sonde HTTP ne distingue
/// pas « rien n'écoute » d'un serveur muet (voir `detect::settle`).
pub fn local_listener(port: u16) -> Option<bool> {
    listener_pid(port).map(|pid| pid.is_some())
}
