// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Inspired by Ollama (https://github.com/ollama/ollama), file
// app/server/server_windows.go (commandContext: hidden window and
// CREATE_NEW_PROCESS_GROUP; terminated), commit 42e911bc3d05.
// Moved out of ollama/builtin/windows.rs for ChatonLM, unchanged (see the
// header of that file): the process and its children live in a Job Object
// closed with ChatonLM (JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE), so that nothing
// outlives it even when it is killed; the stop ends the whole job at once
// (TerminateJobObject), never with taskkill.

//! Sous-processus, partie propre à Windows. Voir `mod.rs` (partie commune) ;
//! `unix.rs` en est le pendant.
//!
//! - Aucun processus orphelin : le processus est placé dès sa création dans
//!   un Job Object dont ChatonLM garde le seul descripteur
//!   (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`). Ses enfants, créés plus tard
//!   (runners de l'Ollama intégré), en font partie. Quand ChatonLM se termine,
//!   même tué, le noyau ferme le descripteur et tue tout le Job : c'est
//!   l'équivalent du groupe de processus et de `PR_SET_PDEATHSIG` d'Unix.
//! - Sans fenêtre : `CREATE_NO_WINDOW` (les moteurs sont des programmes en
//!   mode console).
//! - Arrêt : `TerminateJobObject` (le processus et ses enfants d'un coup),
//!   attente bornée.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::Path;
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
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, OpenProcess, PROCESS_TERMINATE, TerminateProcess,
};
use windows::core::PCWSTR;

use crate::ollama::host::windows::Owned;

/// Variables posées par l'Ollama intégré (adresse, dossier des modèles,
/// dossier personnel) : retirées de l'environnement hérité sous toutes leurs
/// casses, pour tout moteur (aucun ne doit recevoir celles du bureau).
const SET_VARS: [&str; 4] = ["OLLAMA_HOST", "OLLAMA_MODELS", "USERPROFILE", "HOME"];

/// Code de sortie donné aux processus arrêtés par ChatonLM.
const STOPPED: u32 = 1;

pub(crate) fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file())
}

/// ChatonLM ne change rien à son propre environnement sous Windows.
pub(crate) fn desktop_changes() -> (Option<OsString>, bool) {
    (None, false)
}

/// Retire les variables que ChatonLM pose ensuite, quelle que soit leur
/// casse (les noms de variables n'en tiennent pas compte sous Windows).
pub(crate) fn scrub(
    environment: &mut BTreeMap<OsString, OsString>,
    _appdir: Option<&OsStr>,
    _nvidia_workaround: bool,
) {
    super::remove_ignoring_case(environment, &SET_VARS);
}

/// Sans fenêtre de console, et groupe de processus à lui (comme
/// l'officiel : un Ctrl+C de la console de ChatonLM ne l'atteint pas).
pub(crate) fn isolate(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW.0 | CREATE_NEW_PROCESS_GROUP.0);
}

/// Job Object de chaque processus lancé, par numéro de processus, jusqu'à sa
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

/// Crée le processus et le place aussitôt dans son Job Object : ses enfants,
/// lancés bien après (runners de l'Ollama intégré, au chargement d'un
/// modèle), en font partie. Sans Job (refus du système), le processus démarre
/// quand même : l'arrêt ne vise alors que lui.
pub(crate) fn spawn(command: &mut Command) -> io::Result<Child> {
    let child = command.spawn()?;
    match job_for(&child) {
        Ok(job) => JOBS.lock().push((child.id(), Arc::new(job))),
        // i18n-ignore : journal.
        Err(error) => crate::app_log!("chatonlm: no job object for a child process: {error}"),
    }
    Ok(child)
}

/// Attente de la fin des enfants après l'arrêt du Job.
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
/// (enfants) est tué avant que la fin soit annoncée.
pub(crate) fn watch_exit(mut child: Child) -> oneshot::Receiver<Option<ExitStatus>> {
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
                // Les enfants finissent de mourir : leurs fichiers (modèles
                // ouverts) sont libres avant qu'on déplace quoi que ce soit.
                wait_empty(&job, RUNNERS_WAIT);
            }
            let _ = sender.send(status);
        });
    if let Err(error) = watcher {
        // i18n-ignore : journal.
        crate::app_log!("chatonlm: cannot watch the child process {pid}: {error}");
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

/// Arrête le processus et ses enfants (tout le Job d'un coup ; sans Job, le
/// processus seul) et attend sa fin, `kill_wait` au plus : au-delà, abandon
/// journalisé (le fil de surveillance finira plus tard). Vrai si le
/// processus est fini. `_grace` (délai entre SIGTERM et SIGKILL sous Unix)
/// ne sert pas : il n'y a pas d'arrêt demandé poliment ici (voir l'en-tête
/// de `ollama/builtin/windows.rs`).
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
    crate::app_log!(
        "chatonlm: the child process {pid} didn't exit after being terminated; leaving it"
    );
    false
}

/// Processus pas encore sorti (essais).
#[cfg(test)]
pub(crate) fn alive(pid: u32) -> bool {
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
