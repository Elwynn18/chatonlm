//! Place libre sous Windows (`GetDiskFreeSpaceExW`), voir `disk.rs`.

use std::path::Path;

use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::core::PCWSTR;

use crate::os_util::wide_os;

/// Place libre pour l'utilisateur (quotas compris).
pub fn free_space(path: &Path) -> Option<u64> {
    let path = wide_os(path);
    let mut free = 0u64;
    // SAFETY : chemin UTF-16 terminé par un zéro, sortie de 8 octets.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(path.as_ptr()), Some(&mut free), None, None) }.ok()?;
    Some(free)
}
