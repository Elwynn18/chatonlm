//! Limites de l'enfant d'extraction des PDF sous Windows : un Job Object que
//! l'enfant crée et rejoint lui-même avant de lire le PDF (les jobs
//! s'imbriquent depuis Windows 8 : l'app peut déjà être dans un job).
//!
//! Écarts à Linux, sans équivalent exact :
//! - la mémoire bornée est la mémoire engagée (et non l'espace d'adressage) :
//!   une allocation au-delà échoue de la même façon ;
//! - le temps de processeur borné est celui passé en mode utilisateur, que le
//!   système contrôle périodiquement : l'enfant est alors arrêté avec le code
//!   de sortie `ERROR_NOT_ENOUGH_QUOTA` (voir `pdf::windows_end`) ;
//! - Windows n'écrit pas de vidage mémoire par défaut ; les boîtes d'erreur
//!   sont coupées (`SetErrorMode` et `DIE_ON_UNHANDLED_EXCEPTION`).

use std::fs::File;
use std::io;
use std::os::windows::io::AsHandle;

use windows::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
};
use windows::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode, THREAD_ERROR_MODE,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOB_OBJECT_LIMIT_PROCESS_TIME, JOBOBJECT_BASIC_LIMIT_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::core::PCWSTR;

use super::{MEMORY_LIMIT, job_time_ticks};

/// Limites du job : mémoire engagée, temps de processeur, fin sans boîte
/// d'erreur sur une exception non traitée.
const LIMIT_FLAGS: u32 = JOB_OBJECT_LIMIT_PROCESS_MEMORY.0
    | JOB_OBJECT_LIMIT_PROCESS_TIME.0
    | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION.0;

/// Limites posées avant de lire le PDF : mémoire engagée et temps de
/// processeur bornés, aucune boîte d'erreur si l'enfant plante.
pub(super) fn confine(cpu_seconds: u64) -> io::Result<()> {
    // SAFETY : SetErrorMode ne lit ni n'écrit de mémoire.
    unsafe {
        SetErrorMode(THREAD_ERROR_MODE(
            SEM_FAILCRITICALERRORS.0 | SEM_NOGPFAULTERRORBOX.0,
        ));
    }
    let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
            PerProcessUserTimeLimit: job_time_ticks(cpu_seconds),
            LimitFlags: JOB_OBJECT_LIMIT(LIMIT_FLAGS),
            ..Default::default()
        },
        ProcessMemoryLimit: usize::try_from(MEMORY_LIMIT).unwrap_or(usize::MAX),
        ..Default::default()
    };
    // SAFETY : job anonyme sans attributs de sécurité ; la structure des
    // limites est valide et de la taille annoncée pendant l'appel.
    unsafe {
        let job = CreateJobObjectW(None, PCWSTR::null())?;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )?;
        // La poignée du job n'est jamais fermée : il vit autant que l'enfant,
        // qui ne peut plus en sortir.
        AssignProcessToJobObject(job, GetCurrentProcess())?;
    }
    Ok(())
}

/// Sortie réservée au résultat : une copie de la sortie standard ; la sortie
/// standard elle-même est redirigée vers la sortie d'erreur (non lue), pour
/// qu'une bibliothèque qui y écrit des avertissements ne se mêle pas au
/// texte. (`println!` relit la poignée du processus à chaque écriture.)
pub(super) fn private_stdout() -> io::Result<File> {
    let saved = io::stdout().as_handle().try_clone_to_owned()?;
    // SAFETY : poignées standard du processus, qui n'a qu'un seul fil
    // d'exécution à ce moment.
    unsafe {
        let error = GetStdHandle(STD_ERROR_HANDLE)?;
        SetStdHandle(STD_OUTPUT_HANDLE, error)?;
    }
    Ok(File::from(saved))
}

/// Limites posées, telles que le système les voit (essais) : celles du job
/// de l'enfant.
#[cfg(debug_assertions)]
pub(super) fn limits_report() -> String {
    use windows::Win32::System::JobObjects::QueryInformationJobObject;

    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    // SAFETY : la structure est valide et de la taille annoncée ; sans
    // poignée, le job interrogé est celui du processus.
    let read = unsafe {
        QueryInformationJobObject(
            None,
            JobObjectExtendedLimitInformation,
            (&raw mut limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            None,
        )
    };
    match read {
        Ok(()) => format!(
            "memory={} cpu={} flags={:#x}",
            limits.ProcessMemoryLimit,
            limits.BasicLimitInformation.PerProcessUserTimeLimit,
            limits.BasicLimitInformation.LimitFlags.0
        ),
        Err(error) => format!("no job: {error}"),
    }
}
