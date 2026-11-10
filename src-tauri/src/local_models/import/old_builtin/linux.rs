// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Inspired by Ollama (https://github.com/ollama/ollama), file
// app/server/server.go (cleanup, stop), commit 42e911bc3d05: a pid file to
// clean up a server left over by a previous session, SIGTERM then SIGKILL.
// Modified for ChatonLM: only the built-in Ollama of ChatonLM 0.9.x is
// stopped (its executable and its private HOME), with the runners of its
// process group; never ChatonLM itself nor a program that took the number.

//! Ancien Ollama intégré resté en vie, partie Linux (par `/proc`). Voir
//! `old_builtin.rs` ; `windows.rs` en est le pendant.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Délai entre SIGTERM et SIGKILL (celui de l'arrêt de l'ancien intégré).
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Reconnaît un processus de l'ancien intégré : même uid, exécutable
/// `…/ollama/bin/ollama` (le binaire livré, peut-être effacé depuis par la
/// mise à jour) et `HOME` privé que ChatonLM lui donnait, hérité par ses
/// runners (le chemin du binaire d'une AppImage change à chaque lancement :
/// il ne suffit pas seul).
pub(crate) struct OldBuiltin {
    pub uid: u32,
    pub home: PathBuf,
}

impl OldBuiltin {
    fn of(data_dir: &Path) -> OldBuiltin {
        OldBuiltin {
            uid: std::fs::metadata("/proc/self").map_or(u32::MAX, |meta| meta.uid()),
            home: super::root(data_dir).join("home"),
        }
    }

    /// Processus vivant de l'ancien intégré.
    pub(crate) fn owns(&self, proc: &Path, pid: u32) -> bool {
        let dir = proc.join(pid.to_string());
        std::fs::metadata(&dir).is_ok_and(|meta| meta.uid() == self.uid)
            && std::fs::read_link(dir.join("exe")).is_ok_and(|exe| is_builtin_binary(&exe))
            && self.private_home(&dir)
    }

    /// `HOME=<home>` dans l'environnement du processus.
    fn private_home(&self, dir: &Path) -> bool {
        let Ok(environ) = std::fs::read(dir.join("environ")) else {
            return false;
        };
        let mut wanted = b"HOME=".to_vec();
        wanted.extend_from_slice(self.home.as_os_str().as_encoded_bytes());
        environ
            .split(|byte| *byte == 0)
            .any(|entry| entry == wanted)
    }
}

/// `…/ollama/bin/ollama`, avec la marque « (deleted) » du noyau quand le
/// fichier a été effacé depuis le lancement.
pub(crate) fn is_builtin_binary(exe: &Path) -> bool {
    let text = exe.to_string_lossy();
    let path = Path::new(text.strip_suffix(" (deleted)").unwrap_or(&text));
    let names: Vec<_> = path
        .components()
        .rev()
        .take(3)
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    names == ["ollama", "bin", "ollama"]
}

/// Processus du groupe `leader` qui sont l'ancien intégré ; vide si le
/// meneur vit sans l'être, ou si `leader` désigne ChatonLM ou son groupe.
pub(crate) fn leftover_processes(proc: &Path, leader: u32, ours: &OldBuiltin) -> Vec<u32> {
    let own = std::process::id();
    // SAFETY : appel système sans argument.
    let own_group = u32::try_from(unsafe { libc::getpgrp() }).unwrap_or(0);
    if leader <= 1 || leader == own || leader == own_group {
        return Vec::new();
    }
    if proc.join(leader.to_string()).exists() && !ours.owns(proc, leader) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(proc) else {
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
fn group_of(proc: &Path, pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(proc.join(pid.to_string()).join("stat")).ok()?;
    let fields: Vec<&str> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
    if fields.first() == Some(&"Z") {
        return None;
    }
    fields.get(2)?.parse().ok()
}

/// Arrête le groupe de l'ancien intégré noté `leader` (le serveur et ses
/// runners) : SIGTERM, puis SIGKILL après [`STOP_GRACE`] à ceux qui restent
/// et sont encore lui.
pub fn stop(leader: u32, data_dir: &Path) {
    stop_with(
        Path::new("/proc"),
        leader,
        &OldBuiltin::of(data_dir),
        STOP_GRACE,
    );
}

pub(crate) fn stop_with(proc: &Path, leader: u32, ours: &OldBuiltin, grace: std::time::Duration) {
    let found = leftover_processes(proc, leader, ours);
    if found.is_empty() {
        return;
    }
    // i18n-ignore : journal.
    crate::app_log!("chatonlm: stopping a built-in Ollama left over by ChatonLM 0.9.x: {found:?}");
    let alive = |pid: u32| group_of(proc, pid).is_some();
    for &pid in &found {
        signal_process(pid, libc::SIGTERM);
    }
    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline && found.iter().any(|&pid| alive(pid)) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    for &pid in &found {
        if alive(pid) && ours.owns(proc, pid) {
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
