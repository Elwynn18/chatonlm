//! Ce que ChatonLM sait mettre à jour, et l'installation elle-même, selon le
//! système : sous Linux (`linux.rs`), AppImage remplacée sur place ou .deb
//! par `pkexec dpkg -i` ; sous Windows (`windows.rs`, règles sans appel
//! système dans `nsis.rs`), installateur NSIS vérifié puis lancé. Les deux
//! donnent la même API : `detect_current`, `Platform`,
//! `INSTALLER_RELAUNCHES`. La forme des paquets (`format.rs`) ne dépend pas
//! du système.

use std::path::Path;

use super::{UnsupportedReason, UpdateInstaller};

mod format;
#[cfg(target_os = "linux")]
pub(crate) mod linux;
pub(crate) mod nsis;
#[cfg(windows)]
pub(crate) mod windows;

pub(crate) use format::has_format;
#[cfg(test)]
pub(crate) use format::tests::test_nsis;
#[cfg(target_os = "linux")]
pub use linux::forget_foreign_appimage;
#[cfg(target_os = "linux")]
pub(crate) use linux::{INSTALLER_RELAUNCHES, Platform};
#[cfg(windows)]
pub(crate) use windows::{INSTALLER_RELAUNCHES, Platform};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Support {
    Installable(UpdateInstaller),
    Unsupported(UnsupportedReason),
}

/// Programme en cours : manière de se mettre à jour, ou pourquoi c'est
/// impossible. `product` : `productName` de la configuration (sous Windows,
/// nom de la clé de désinstallation écrite par l'installateur).
pub(crate) fn detect_current(product: &str) -> Support {
    #[cfg(target_os = "linux")]
    {
        let _ = product;
        linux::detect_current()
    }
    #[cfg(windows)]
    {
        windows::detect_current(product)
    }
}

/// `APPIMAGE` hérité d'une autre AppImage : sans objet hors de Linux.
#[cfg(not(target_os = "linux"))]
pub fn forget_foreign_appimage() {}

/// Essai réel (droits, ACL, système de fichiers en lecture seule) : un
/// fichier vide créé puis retiré.
fn can_create_in(folder: &Path) -> bool {
    let probe = folder.join(format!(
        ".chatonlm-update-probe-{}-{}",
        std::process::id(),
        crate::store::now_ms()
    ));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}
