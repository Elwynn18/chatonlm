//! Limites de l'enfant d'extraction des PDF sous Linux : limites du noyau
//! (`setrlimit`, `prctl`) et descripteurs de fichiers.

use std::fs::File;
use std::io;
use std::os::fd::AsFd;

use super::MEMORY_LIMIT;

/// Limites posées avant de lire le PDF : pas de vidage mémoire d'un enfant
/// planté (rien écrit sur disque ni dans le journal du système), espace
/// d'adressage et temps de processeur bornés.
pub(super) fn confine(cpu_seconds: u64) -> io::Result<()> {
    // SAFETY : prctl(PR_SET_DUMPABLE, 0) ne lit ni n'écrit de mémoire.
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    lower_limit(libc::RLIMIT_CORE, 0, 0)?;
    lower_limit(libc::RLIMIT_AS, MEMORY_LIMIT, MEMORY_LIMIT)?;
    lower_limit(libc::RLIMIT_CPU, cpu_seconds, cpu_seconds + 1)
}

/// Abaisse une limite du noyau (jamais au-dessus de la limite dure actuelle,
/// qu'un processus ordinaire ne peut pas relever).
fn lower_limit(resource: libc::__rlimit_resource_t, soft: u64, hard: u64) -> io::Result<()> {
    let mut current = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY : getrlimit écrit dans une structure valide passée par adresse.
    if unsafe { libc::getrlimit(resource, &mut current) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let limit = libc::rlimit {
        rlim_cur: soft.min(current.rlim_max),
        rlim_max: hard.min(current.rlim_max),
    };
    // SAFETY : setrlimit lit une structure valide passée par adresse.
    if unsafe { libc::setrlimit(resource, &limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Sortie réservée au résultat : une copie de la sortie standard ; la sortie
/// standard elle-même est redirigée vers la sortie d'erreur (non lue), pour
/// qu'une bibliothèque qui y écrit des avertissements ne se mêle pas au
/// texte.
pub(super) fn private_stdout() -> io::Result<File> {
    let saved = io::stdout().as_fd().try_clone_to_owned()?;
    // SAFETY : dup2 sur deux descripteurs ouverts du processus, qui n'a qu'un
    // seul fil d'exécution à ce moment.
    if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(File::from(saved))
}

/// Limites posées, telles que le noyau les voit (essais).
#[cfg(debug_assertions)]
pub(super) fn limits_report() -> String {
    let limit = |resource| {
        let mut value = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY : getrlimit écrit dans une structure valide.
        unsafe { libc::getrlimit(resource, &mut value) };
        format!("{}/{}", value.rlim_cur, value.rlim_max)
    };
    // SAFETY : prctl(PR_GET_DUMPABLE) ne lit ni n'écrit de mémoire.
    let dumpable = unsafe { libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0) };
    format!(
        "dumpable={dumpable} core={} as={} cpu={}",
        limit(libc::RLIMIT_CORE),
        limit(libc::RLIMIT_AS),
        limit(libc::RLIMIT_CPU)
    )
}
