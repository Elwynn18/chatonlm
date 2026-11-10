// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Inspired by Ollama (https://github.com/ollama/ollama), file
// app/server/server.go (cleanup), commit 42e911bc3d05: a pid file to clean
// up a server left over by a previous session.
// Modified for ChatonLM: only the built-in Ollama of ChatonLM 0.9.x is stopped
// (its executable, <app folder>\ollama\ollama.exe), through one handle that
// both reads the executable and terminates; never ChatonLM itself, never
// taskkill.

//! Ancien Ollama intégré resté en vie, partie Windows. Le Job Object de 0.9.x
//! rendait ce reste impossible en principe (ChatonLM mort, tout le Job
//! l'était) : garde pour le cas où le Job avait été refusé. Voir
//! `old_builtin.rs` ; `linux.rs` en est le pendant.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, TerminateProcess,
};
use windows::core::PWSTR;

use crate::os_util::from_wide;
use crate::process::Owned;

/// Code de sortie donné au processus arrêté.
const STOPPED: u32 = 1;

/// Exécutable du processus d'un descripteur déjà ouvert (avec au moins
/// `PROCESS_QUERY_LIMITED_INFORMATION`).
fn image_of(process: &Owned) -> Result<PathBuf, bool> {
    let mut buffer = vec![0u16; 32768];
    let mut length = buffer.len() as u32;
    // SAFETY : tampon de `length` unités, longueur écrite en retour ;
    // descripteur tenu par l'appelant.
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    }
    .map_err(|error| error.code() == E_ACCESSDENIED)?;
    buffer.truncate(length as usize);
    Ok(PathBuf::from(from_wide(&buffer)))
}

/// Arrête le processus `pid` si son exécutable est l'ancien intégré livré à
/// côté de ChatonLM (`<dossier de l'app>\ollama\ollama.exe`).
pub fn stop(pid: u32, _data_dir: &Path) {
    let Some(install_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return;
    };
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
    let Ok(image) = image_of(&process) else {
        return;
    };
    if !super::is_builtin_image(&image.to_string_lossy(), &install_dir.to_string_lossy()) {
        return;
    }
    // i18n-ignore : journal.
    crate::app_log!("chatonlm: stopping a built-in Ollama left over by ChatonLM 0.9.x: {pid}");
    // SAFETY : descripteur ouvert ci-dessus.
    unsafe {
        let _ = TerminateProcess(process.0, STOPPED);
    }
}
