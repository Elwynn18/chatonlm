// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Inspired by Ollama (https://github.com/ollama/ollama), file
// app/server/server.go (cleanup, stop), commit 42e911bc3d05: see the header
// of builtin.rs, of which this file is the Unix part.

//! Ollama intégré, partie propre à Unix (Linux) : groupe de processus et
//! `PR_SET_PDEATHSIG`, fin attendue sans récolte (`waitid(WNOWAIT)`),
//! SIGTERM puis SIGKILL au groupe, ménage d'un reste d'une session
//! précédente par `/proc`, environnement nettoyé de l'AppImage. Voir
//! `builtin.rs` (superviseur commun) ; `builtin/windows.rs` en est le
//! pendant.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use super::Config;

/// Variables qui donnent son dossier personnel au serveur.
pub(super) const HOME_VARS: [&str; 1] = ["HOME"];

/// Binaire sous le dossier `ollama` des ressources.
pub(super) fn bundled_bin(root: &Path) -> PathBuf {
    root.join("bin").join("ollama")
}

/// Dossier d'installation (`bin/`, `lib/`, `VERSION`) d'un binaire
/// canonique.
pub(super) fn install_root(bin: &Path) -> Option<PathBuf> {
    Some(bin.parent()?.parent()?.to_path_buf())
}

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
/// ni `LD_PRELOAD` (l'intégré trouve ses bibliothèques à côté de lui).
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

/// Groupe à lui (ses runners en héritent, l'arrêt les atteint) et
/// `PR_SET_PDEATHSIG` : le serveur reçoit SIGTERM quand le fil qui l'a créé
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
            // ChatonLM déjà mort entre fork et prctl : pas de serveur orphelin.
            if libc::getppid() as u32 != parent {
                return Err(io::Error::from_raw_os_error(libc::ESRCH));
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
pub(super) fn exit_signal(status: &ExitStatus) -> Option<i32> {
    status.signal()
}

/// Attend la fin du processus dans un fil à part ; ce qui reste de son
/// groupe (runners) est tué avant qu'il soit récolté.
pub(crate) fn watch_exit(mut child: Child) -> oneshot::Receiver<Option<ExitStatus>> {
    let (sender, receiver) = oneshot::channel();
    let pid = child.id();
    let watcher = std::thread::Builder::new()
        .name("chatonlm-ollama-wait".into())
        .spawn(move || {
            wait_without_reaping(pid);
            // Le meneur, pas encore récolté, réserve le numéro du groupe.
            signal_group(pid, libc::SIGKILL);
            let _ = sender.send(child.wait().ok());
        });
    if let Err(error) = watcher {
        eprintln!("chatonlm: cannot watch the built-in Ollama: {error}");
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
    eprintln!("chatonlm: the built-in Ollama (pid {pid}) didn't exit after SIGKILL; leaving it");
    false
}

// --- Reste d'une session précédente ------------------------------------------

/// Ménage d'un intégré laissé par une session précédente (ChatonLM tué avant
/// l'arrêt) : processus du groupe noté dans le fichier pid qui sont l'intégré
/// ([`Ours`]). Rien n'est touché si le meneur vit sans l'être (numéro repris
/// par un autre programme).
pub(super) fn cleanup_leftovers(config: &Config) {
    let pid_file = config.pid_file();
    let Ok(text) = fs::read_to_string(&pid_file) else {
        return;
    };
    let _ = fs::remove_file(&pid_file);
    let Ok(leader) = text.trim().parse::<u32>() else {
        return;
    };
    let ours = Ours {
        uid: fs::metadata("/proc/self").map_or(u32::MAX, |meta| meta.uid()),
        home: config.home(),
    };
    let proc = Path::new("/proc");
    let found = leftover_processes(proc, leader, &ours);
    if found.is_empty() {
        return;
    }
    eprintln!("chatonlm: stopping a built-in Ollama left over by a previous session: {found:?}");
    stop_processes(&found, config.timing.stop_grace, |pid| ours.owns(proc, pid));
}

/// Reconnaît un processus de l'intégré : même uid et `HOME` privé de
/// l'intégré, posé par ChatonLM et hérité par ses runners. L'exécutable ne
/// suffit jamais : `CHATONLM_OLLAMA_BIN=/usr/bin/ollama` (builds d'essai)
/// ferait de tout `/usr` le « dossier d'installation », et celui d'une
/// AppImage change à chaque lancement.
pub(crate) struct Ours {
    pub uid: u32,
    pub home: PathBuf,
}

impl Ours {
    /// Processus vivant de l'intégré.
    pub(super) fn owns(&self, proc: &Path, pid: u32) -> bool {
        let dir = proc.join(pid.to_string());
        fs::metadata(&dir).is_ok_and(|meta| meta.uid() == self.uid) && self.private_home(&dir)
    }

    /// `HOME=<home>` dans l'environnement du processus.
    fn private_home(&self, dir: &Path) -> bool {
        let Ok(environ) = fs::read(dir.join("environ")) else {
            return false;
        };
        let mut wanted = b"HOME=".to_vec();
        wanted.extend_from_slice(self.home.as_os_str().as_encoded_bytes());
        environ
            .split(|byte| *byte == 0)
            .any(|entry| entry == wanted)
    }
}

/// Processus du groupe `leader` qui sont l'intégré ; vide si le meneur vit
/// sans l'être, ou si `leader` désigne ChatonLM ou son groupe.
pub(crate) fn leftover_processes(proc: &Path, leader: u32, ours: &Ours) -> Vec<u32> {
    let own = std::process::id();
    // SAFETY : appel système sans argument.
    let own_group = u32::try_from(unsafe { libc::getpgrp() }).unwrap_or(0);
    if leader <= 1 || leader == own || leader == own_group {
        return Vec::new();
    }
    if proc.join(leader.to_string()).exists() && !ours.owns(proc, leader) {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(proc) else {
        return Vec::new();
    };
    let mut found: Vec<u32> = entries
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&pid| pid != own && group_of(proc, pid) == Some(leader))
        .filter(|&pid| ours.owns(proc, pid))
        .collect();
    found.sort_unstable();
    found
}

/// Groupe d'un processus (`/proc/<pid>/stat`, champ 5) ; `None` pour un
/// zombie.
pub(super) fn group_of(proc: &Path, pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(proc.join(pid.to_string()).join("stat")).ok()?;
    let fields: Vec<&str> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
    if fields.first() == Some(&"Z") {
        return None;
    }
    fields.get(2)?.parse().ok()
}

/// SIGTERM, puis SIGKILL après `grace` à ceux qui restent et sont encore les
/// mêmes (`still_ours`).
fn stop_processes(pids: &[u32], grace: Duration, still_ours: impl Fn(u32) -> bool) {
    let alive = |pid: u32| group_of(Path::new("/proc"), pid).is_some();
    for &pid in pids {
        signal_process(pid, libc::SIGTERM);
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline && pids.iter().any(|&pid| alive(pid)) {
        std::thread::sleep(Duration::from_millis(50));
    }
    for &pid in pids {
        if alive(pid) && still_ours(pid) {
            signal_process(pid, libc::SIGKILL);
        }
    }
}

fn signal_process(pid: u32, signal: libc::c_int) {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return;
    };
    if pid <= 1 || pid as u32 == std::process::id() {
        return;
    }
    // SAFETY : appel système sans mémoire partagée.
    unsafe {
        libc::kill(pid, signal);
    }
}
