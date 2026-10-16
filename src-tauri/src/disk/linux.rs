//! Place libre sous Linux (`statvfs`), voir `disk.rs`.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Place libre pour un utilisateur ordinaire (`statvfs`).
pub fn free_space(path: &Path) -> Option<u64> {
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY : chemin terminé par un octet nul ; structure de sortie à la
    // taille attendue, lue seulement si l'appel réussit.
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY : remplie par statvfs, qui a réussi.
    let stat = unsafe { stat.assume_init() };
    // Types de 32 ou 64 bits selon l'architecture.
    #[allow(clippy::unnecessary_cast)]
    let (blocks, block_size) = (stat.f_bavail as u64, stat.f_frsize as u64);
    Some(blocks.saturating_mul(block_size))
}
