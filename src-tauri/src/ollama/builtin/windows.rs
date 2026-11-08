// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Inspired by Ollama (https://github.com/ollama/ollama), files
// app/server/server_windows.go (commandContext: hidden window and
// CREATE_NEW_PROCESS_GROUP; terminated) and app/server/server.go (cleanup),
// commit 42e911bc3d05: see the header of builtin.rs, of which this file is
// the Windows part.
// Modified for ChatonLM: the server and its runners live in a Job Object
// closed with ChatonLM (JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE), so that nothing
// outlives it even when it is killed; the stop ends the whole job at once
// (TerminateJobObject) instead of sending CTRL_BREAK then killing the server
// alone, and never uses taskkill; a leftover of a previous session is
// stopped only when its executable is the built-in Ollama.

//! Ollama intégré, partie propre à Windows : emplacement du binaire, dossier
//! personnel du serveur, ménage d'un reste d'une session précédente. Job
//! Object, fenêtre, arrêt et environnement : `process/windows.rs` (commun aux
//! moteurs). Voir `builtin.rs` (superviseur commun) ; `builtin/unix.rs` en
//! est le pendant.
//!
//! - Le serveur et ses runners vivent dans un Job Object fermé avec ChatonLM
//!   (`process/windows.rs`).
//! - Arrêt : `TerminateJobObject` (serveur et runners d'un coup), attente
//!   bornée. Écart à l'officiel, qui envoie d'abord `CTRL_BREAK_EVENT` par
//!   `AttachConsole` : cette voie n'a pas pu être essayée depuis une app
//!   sans console ; les téléchargements interrompus reprennent par leurs
//!   fichiers partiels, et l'app arrête ses travaux avant le serveur.
//! - Dossier personnel du serveur : `USERPROFILE` (celui que lit Go), et
//!   `HOME` par symétrie, sous les données de l'app.

use std::fs;
use std::path::{Path, PathBuf};

use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
};

use super::Config;
use crate::ollama::host::windows::{Owned, image_of};

/// Variables qui donnent son dossier personnel au serveur : Go lit
/// `USERPROFILE` sous Windows (clé `.ollama\id_ed25519`).
pub(super) const HOME_VARS: [&str; 2] = ["USERPROFILE", "HOME"];

/// Code de sortie donné aux processus arrêtés par ChatonLM.
const STOPPED: u32 = 1;

/// Binaire sous le dossier `ollama` des ressources (arborescence de
/// l'archive officielle : `ollama.exe`, `lib\ollama\…`).
pub(super) fn bundled_bin(root: &Path) -> PathBuf {
    root.join("ollama.exe")
}

/// Dossier d'installation (`ollama.exe`, `lib\`, `VERSION`) d'un binaire
/// canonique.
pub(super) fn install_root(bin: &Path) -> Option<PathBuf> {
    Some(bin.parent()?.to_path_buf())
}

/// Même fichier, sans tenir compte de la casse ni du préfixe `\\?\`.
fn same_file(a: &Path, b: &Path) -> bool {
    let canonical = |path: &Path| {
        dunce::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .to_lowercase()
    };
    canonical(a) == canonical(b)
}

/// Ménage d'un intégré laissé par une session précédente. Le Job Object
/// rend ce reste impossible en principe (ChatonLM mort, tout le Job l'est) :
/// garde pour le cas où le Job a été refusé. Le processus du fichier pid
/// n'est arrêté que si son exécutable est celui de l'intégré ; jamais
/// ChatonLM lui-même, jamais un programme qui aurait repris le numéro.
pub(super) fn cleanup_leftovers(config: &Config) {
    let pid_file = config.pid_file();
    let Ok(text) = fs::read_to_string(&pid_file) else {
        return;
    };
    let _ = fs::remove_file(&pid_file);
    let (Ok(pid), Some(bin)) = (text.trim().parse::<u32>(), config.bin.as_deref()) else {
        return;
    };
    if pid == 0 || pid == std::process::id() {
        return;
    }
    // Un seul descripteur pour lire l'exécutable puis arrêter : si le reste
    // meurt entre les deux et que son numéro est repris, ce n'est pas le
    // nouveau venu qui est tué.
    // SAFETY : droits de lire l'image et de terminer ; fermé par `Owned`.
    let Ok(process) = (unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
            false,
            pid,
        )
    }) else {
        return;
    };
    let process = Owned(process);
    let Ok(image) = image_of(process.0) else {
        return;
    };
    if !same_file(&image, bin) {
        return;
    }
    // i18n-ignore : journal.
    crate::app_log!("chatonlm: stopping a built-in Ollama left over by a previous session: {pid}");
    // SAFETY : descripteur ouvert ci-dessus.
    unsafe {
        let _ = TerminateProcess(process.0, STOPPED);
    }
}
