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

//! Ollama intégré, partie propre à Windows. Voir `builtin.rs` (superviseur
//! commun) ; `builtin/unix.rs` en est le pendant.
//!
//! - Aucun processus orphelin : le serveur est placé dès sa création dans un
//!   Job Object dont ChatonLM garde le seul descripteur
//!   (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`). Ses runners, créés plus tard,
//!   en font partie. Quand ChatonLM se termine, même tué, le noyau ferme le
//!   descripteur et tue tout le Job : c'est l'équivalent du groupe de
//!   processus et de `PR_SET_PDEATHSIG` d'Unix.
//! - Sans fenêtre : `CREATE_NO_WINDOW` (le serveur est un programme en mode
//!   console).
//! - Arrêt : `TerminateJobObject` (serveur et runners d'un coup), attente
//!   bornée. Écart à l'officiel, qui envoie d'abord `CTRL_BREAK_EVENT` par
//!   `AttachConsole` : cette voie n'a pas pu être essayée depuis une app
//!   sans console ; les téléchargements interrompus reprennent par leurs
//!   fichiers partiels, et l'app arrête ses travaux avant le serveur.
//! - Dossier personnel du serveur : `USERPROFILE` (celui que lit Go), et
//!   `HOME` par symétrie, sous les données de l'app.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::oneshot;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE, TerminateProcess,
};
use windows::core::PCWSTR;

use super::Config;
use crate::ollama::host::windows::{Owned, image_of};

/// Variables qui donnent son dossier personnel au serveur : Go lit
/// `USERPROFILE` sous Windows (clé `.ollama\id_ed25519`).
pub(super) const HOME_VARS: [&str; 2] = ["USERPROFILE", "HOME"];

/// Variables posées par ChatonLM : retirées d'abord sous toutes leurs casses.
const SET_VARS: [&str; 4] = ["OLLAMA_HOST", "OLLAMA_MODELS", "USERPROFILE", "HOME"];

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

pub(super) fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file())
}

/// ChatonLM ne change rien à son propre environnement sous Windows.
pub(super) fn desktop_changes() -> (Option<OsString>, bool) {
    (None, false)
}

/// Retire les variables que ChatonLM pose ensuite, quelle que soit leur
/// casse (les noms de variables n'en tiennent pas compte sous Windows).
pub(super) fn scrub(
    environment: &mut BTreeMap<OsString, OsString>,
    _appdir: Option<&OsStr>,
    _nvidia_workaround: bool,
) {
    super::remove_ignoring_case(environment, &SET_VARS);
}

/// Sans fenêtre de console, et groupe de processus à lui (comme
/// l'officiel : un Ctrl+C de la console de ChatonLM ne l'atteint pas).
pub(super) fn isolate(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW.0 | CREATE_NEW_PROCESS_GROUP.0);
}

/// Job Object de chaque serveur lancé, par numéro de processus, jusqu'à sa
/// fin. Ce sont les seuls descripteurs de ces Jobs : la fin de ChatonLM les
/// ferme, et le noyau tue alors ce qu'ils contiennent.
static JOBS: Mutex<Vec<(u32, Arc<Owned>)>> = Mutex::new(Vec::new());

fn job_of(pid: u32) -> Option<Arc<Owned>> {
    JOBS.lock()
        .iter()
        .find(|(owner, _)| *owner == pid)
        .map(|(_, job)| job.clone())
}

fn forget_job(pid: u32) -> Option<Arc<Owned>> {
    let mut jobs = JOBS.lock();
    let index = jobs.iter().position(|(owner, _)| *owner == pid)?;
    Some(jobs.swap_remove(index).1)
}

/// Job Object qui tue ses processus quand son dernier descripteur se
/// ferme, avec `child` dedans.
fn job_for(child: &Child) -> windows::core::Result<Owned> {
    // SAFETY : Job anonyme, descripteur non héritable, fermé par `Owned`.
    let job = Owned(unsafe { CreateJobObjectW(None, PCWSTR::null()) }?);
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY : structure de la classe d'information demandée, à sa taille.
    unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    }?;
    // SAFETY : descripteur du processus tenu par `child`, vivant ici.
    unsafe { AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle())) }?;
    Ok(job)
}

/// Crée le processus et le place aussitôt dans son Job Object : ses runners,
/// lancés bien après (au chargement d'un modèle), en font partie. Sans Job
/// (refus du système), le serveur démarre quand même : l'arrêt ne vise
/// alors que lui.
pub(super) fn spawn(command: &mut Command) -> io::Result<Child> {
    let child = command.spawn()?;
    match job_for(&child) {
        Ok(job) => JOBS.lock().push((child.id(), Arc::new(job))),
        // i18n-ignore : journal.
        Err(error) => eprintln!("chatonlm: no job object for the built-in Ollama: {error}"),
    }
    Ok(child)
}

/// Attente de la fin des runners après l'arrêt du Job.
const RUNNERS_WAIT: Duration = Duration::from_secs(2);

/// Attend que le Job n'ait plus de processus actif, `limit` au plus.
fn wait_empty(job: &Owned, limit: Duration) {
    let deadline = std::time::Instant::now() + limit;
    loop {
        let mut counts = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY : structure de la classe d'information demandée, à sa
        // taille.
        let read = unsafe {
            QueryInformationJobObject(
                Some(job.0),
                JobObjectBasicAccountingInformation,
                (&raw mut counts).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                None,
            )
        };
        if read.is_err() || counts.ActiveProcesses == 0 || std::time::Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Pas de signaux sous Windows.
pub(super) fn exit_signal(_status: &ExitStatus) -> Option<i32> {
    None
}

/// Attend la fin du processus dans un fil à part ; ce qui reste de son Job
/// (runners) est tué avant que la fin soit annoncée.
pub(super) fn watch_exit(mut child: Child) -> oneshot::Receiver<Option<ExitStatus>> {
    let (sender, receiver) = oneshot::channel();
    let pid = child.id();
    let watcher = std::thread::Builder::new()
        .name("chatonlm-ollama-wait".into())
        .spawn(move || {
            // Tant que `child` vit, son numéro n'est pas réattribué.
            let status = child.wait().ok();
            if let Some(job) = forget_job(pid) {
                // SAFETY : Job tenu par `job`.
                unsafe {
                    let _ = TerminateJobObject(job.0, STOPPED);
                }
                // Les runners finissent de mourir : leurs fichiers (modèles
                // ouverts) sont libres avant qu'on déplace quoi que ce soit.
                wait_empty(&job, RUNNERS_WAIT);
            }
            let _ = sender.send(status);
        });
    if let Err(error) = watcher {
        // i18n-ignore : journal.
        eprintln!("chatonlm: cannot watch the built-in Ollama: {error}");
    }
    receiver
}

/// Tue un processus par son numéro.
fn terminate_process(pid: u32) {
    if pid == 0 || pid == std::process::id() {
        return;
    }
    // SAFETY : droit de terminer seulement ; descripteur fermé par `Owned`.
    let Ok(process) = (unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) }) else {
        return;
    };
    let process = Owned(process);
    // SAFETY : descripteur ouvert ci-dessus.
    unsafe {
        let _ = TerminateProcess(process.0, STOPPED);
    }
}

/// Arrête le serveur et ses runners (tout le Job d'un coup ; sans Job, le
/// serveur seul) et attend sa fin, `kill_wait` au plus : au-delà, abandon
/// journalisé (le fil de surveillance finira plus tard). Vrai si le
/// processus est fini. `_grace` (délai entre SIGTERM et SIGKILL sous Unix)
/// ne sert pas : il n'y a pas d'arrêt demandé poliment ici (voir l'en-tête).
pub(crate) async fn terminate(
    pid: u32,
    exited: &mut oneshot::Receiver<Option<ExitStatus>>,
    _grace: Duration,
    kill_wait: Duration,
) -> bool {
    // Déjà fini : son numéro a pu être repris par un autre programme, rien
    // n'est tué.
    if !matches!(exited.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
        return true;
    }
    match job_of(pid) {
        // SAFETY : Job tenu par `job`.
        Some(job) => unsafe {
            let _ = TerminateJobObject(job.0, STOPPED);
        },
        None => terminate_process(pid),
    }
    if tokio::time::timeout(kill_wait, &mut *exited).await.is_ok() {
        return true;
    }
    // i18n-ignore : journal.
    eprintln!(
        "chatonlm: the built-in Ollama (pid {pid}) didn't exit after being terminated; leaving it"
    );
    false
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
    eprintln!("chatonlm: stopping a built-in Ollama left over by a previous session: {pid}");
    // SAFETY : descripteur ouvert ci-dessus.
    unsafe {
        let _ = TerminateProcess(process.0, STOPPED);
    }
}

/// Processus pas encore sorti (essais).
#[cfg(test)]
pub(super) fn alive(pid: u32) -> bool {
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    const STILL_ACTIVE: u32 = 259;
    // SAFETY : lecture seule ; descripteur fermé par `Owned`.
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return false;
    };
    let process = Owned(process);
    let mut code = 0u32;
    // SAFETY : sortie de 4 octets.
    unsafe { GetExitCodeProcess(process.0, &mut code) }.is_ok() && code == STILL_ACTIVE
}
