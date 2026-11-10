// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Inspired by Ollama (https://github.com/ollama/ollama), file
// app/server/server.go (stop), commit 42e911bc3d05: SIGTERM then SIGKILL.
// Moved out of the former built-in Ollama module for ChatonLM: own process
// group and PR_SET_PDEATHSIG, bounded wait
// after SIGKILL, environment cleaned like the one of the desktop's programs.

//! Sous-processus, partie propre à Unix (Linux) : groupe de processus et
//! `PR_SET_PDEATHSIG`, fin attendue sans récolte (`waitid(WNOWAIT)`),
//! SIGTERM puis SIGKILL au groupe, environnement nettoyé de l'AppImage. Voir
//! `mod.rs` (partie commune) ; `windows.rs` en est le pendant.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, ExitStatus};
use std::time::Duration;

use tokio::sync::oneshot;

pub(crate) fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.mode() & 0o111 != 0)
}

/// Ce que ChatonLM a changé à son propre environnement : dossier de
/// l'AppImage (`APPDIR`), contournement NVIDIA appliqué.
pub(crate) fn desktop_changes() -> (Option<OsString>, bool) {
    (
        std::env::var_os("APPDIR"),
        crate::graphics::nvidia_workaround_applied(),
    )
}

/// Nettoie l'environnement hérité comme celui des programmes du bureau
/// (variables de l'AppImage, contournement NVIDIA), sans `LD_LIBRARY_PATH`
/// ni `LD_PRELOAD` (un moteur livré trouve ses bibliothèques à côté de lui).
pub(crate) fn scrub(
    environment: &mut BTreeMap<OsString, OsString>,
    appdir: Option<&OsStr>,
    nvidia_workaround: bool,
) {
    if let Some(appdir) = appdir.filter(|appdir| !appdir.is_empty()) {
        let cleaned = crate::commands::system::host_environment(
            environment.clone().into_iter(),
            &appdir.to_os_string(),
        );
        for (key, value) in cleaned {
            match value {
                Some(value) => environment.insert(key, value),
                None => environment.remove(&key),
            };
        }
        for key in crate::commands::system::APPIMAGE_ONLY {
            environment.remove(OsStr::new(key));
        }
    }
    for key in ["LD_LIBRARY_PATH", "LD_PRELOAD"] {
        environment.remove(OsStr::new(key));
    }
    if nvidia_workaround {
        environment.remove(OsStr::new(crate::graphics::NV_EXPLICIT_SYNC_ENV));
    }
}

/// Groupe à lui (ses enfants en héritent, l'arrêt les atteint) et
/// `PR_SET_PDEATHSIG` : le processus reçoit SIGTERM quand le fil qui l'a créé
/// se termine.
pub(crate) fn isolate(command: &mut Command) {
    command.process_group(0);
    let parent = std::process::id();
    // SAFETY : entre fork et exec, seulement des appels système
    // (async-signal-safe), sans allocation.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong) != 0 {
                return Err(io::Error::last_os_error());
            }
            // ChatonLM déjà mort entre fork et prctl : pas de processus orphelin.
            if libc::getppid() as u32 != parent {
                return Err(io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        });
    }
}

/// Pas de fichier `core` si le processus plante (`RLIMIT_CORE` à 0, que
/// `systemd-coredump` respecte) : la mémoire d'un moteur de texte contient
/// les conversations.
pub(crate) fn no_core_dump(command: &mut Command) {
    // SAFETY : entre fork et exec, un seul appel système, sans allocation.
    unsafe {
        command.pre_exec(|| {
            let none = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::setrlimit(libc::RLIMIT_CORE, &none) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Crée le processus (depuis le fil dédié : voir `Spawner`).
pub(crate) fn spawn(command: &mut Command) -> io::Result<Child> {
    command.spawn()
}

/// Signal qui a mis fin au processus.
pub(crate) fn exit_signal(status: &ExitStatus) -> Option<i32> {
    status.signal()
}

/// Attend la fin du processus dans un fil à part ; ce qui reste de son
/// groupe (runners) est tué avant qu'il soit récolté.
pub(crate) fn watch_exit(mut child: Child) -> oneshot::Receiver<Option<ExitStatus>> {
    let (sender, receiver) = oneshot::channel();
    let pid = child.id();
    let watcher = std::thread::Builder::new()
        .name("chatonlm-process-wait".into())
        .spawn(move || {
            wait_without_reaping(pid);
            // Le meneur, pas encore récolté, réserve le numéro du groupe.
            signal_group(pid, libc::SIGKILL);
            let _ = sender.send(child.wait().ok());
        });
    if let Err(error) = watcher {
        // i18n-ignore : journal.
        crate::app_log!("chatonlm: cannot watch the child process {pid}: {error}");
    }
    receiver
}

/// `waitid(WEXITED | WNOWAIT)` : bloque jusqu'à la fin de `pid` sans le
/// récolter.
fn wait_without_reaping(pid: u32) {
    loop {
        // SAFETY : structure de sortie remise à zéro, lue par personne.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY : appel système sur un enfant de ce processus.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if result == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return;
        }
    }
}

/// Signal au groupe de `pgid`, l'enfant (pas encore récolté) qui le mène ;
/// à lui seul si le groupe n'existe pas. Jamais à 0, 1 ni au groupe de
/// ChatonLM.
fn signal_group(pgid: u32, signal: libc::c_int) {
    let Ok(pgid) = libc::pid_t::try_from(pgid) else {
        return;
    };
    // SAFETY : appels système sans mémoire partagée.
    if pgid <= 1 || pgid == unsafe { libc::getpgrp() } {
        return;
    }
    // SAFETY : idem.
    unsafe {
        if libc::kill(-pgid, signal) != 0 {
            libc::kill(pgid, signal);
        }
    }
}

/// SIGTERM au groupe, puis SIGKILL après `grace` ; attend la récolte,
/// `kill_wait` au plus après SIGKILL (un processus bloqué dans le noyau,
/// état D, ne meurt qu'au retour de son appel système) : au-delà, abandon
/// journalisé (le fil de surveillance récoltera plus tard). Vrai si le
/// processus a été récolté.
pub(crate) async fn terminate(
    pid: u32,
    exited: &mut oneshot::Receiver<Option<ExitStatus>>,
    grace: Duration,
    kill_wait: Duration,
) -> bool {
    // Déjà fini (et récolté) : son numéro a pu être repris par un autre
    // programme, rien n'est envoyé.
    if !matches!(exited.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
        return true;
    }
    signal_group(pid, libc::SIGTERM);
    if tokio::time::timeout(grace, &mut *exited).await.is_ok() {
        return true;
    }
    signal_group(pid, libc::SIGKILL);
    if tokio::time::timeout(kill_wait, &mut *exited).await.is_ok() {
        return true;
    }
    // i18n-ignore : journal.
    crate::app_log!("chatonlm: the child process {pid} didn't exit after SIGKILL; leaving it");
    false
}
