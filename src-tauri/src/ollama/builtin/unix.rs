// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Inspired by Ollama (https://github.com/ollama/ollama), file
// app/server/server.go (cleanup, stop), commit 42e911bc3d05: see the header
// of builtin.rs, of which this file is the Unix part.

//! Ollama intégré, partie propre à Unix (Linux) : emplacement du binaire,
//! ménage d'un reste d'une session précédente par `/proc`. Groupe de
//! processus, `PR_SET_PDEATHSIG`, arrêt et environnement : `process/unix.rs`
//! (commun aux moteurs). Voir `builtin.rs` (superviseur commun) ;
//! `builtin/windows.rs` en est le pendant.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
    crate::app_log!(
        "chatonlm: stopping a built-in Ollama left over by a previous session: {found:?}"
    );
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
