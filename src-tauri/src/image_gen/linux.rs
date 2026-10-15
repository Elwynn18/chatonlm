//! Moteur d'images, partie propre à Linux : chargeur Vulkan de secours. Le
//! binaire `sd-server` est lié à `libvulkan.so.1` ; sans chargeur sur le
//! système (machine sans pilote graphique), il ne démarrerait pas. Celui livré
//! dans `<moteur>/fallback/` est alors donné par `LD_LIBRARY_PATH` (sans pilote,
//! il ne trouve aucune carte : le moteur calcule sur le processeur). Avec un
//! chargeur sur le système, rien ne change : celui du système sert. Voir
//! `windows.rs`, son pendant.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

/// Nom du chargeur Vulkan livré dans `fallback/`.
pub(super) const LOADER: &str = "libvulkan.so.1";

/// Le système a un chargeur Vulkan (il se charge).
pub(super) fn system_has_loader() -> bool {
    // SAFETY : nom constant terminé par un zéro ; la bibliothèque, ouverte
    // localement et sans résolution des symboles, est refermée aussitôt.
    unsafe {
        let handle = libc::dlopen(
            c"libvulkan.so.1".as_ptr(),
            libc::RTLD_LAZY | libc::RTLD_LOCAL,
        );
        if handle.is_null() {
            return false;
        }
        libc::dlclose(handle);
    }
    true
}

/// Environnement du serveur : chargeur de `fallback` seulement si le système
/// n'en a pas (`system_has`).
pub(super) fn vulkan_loader(
    environment: &mut BTreeMap<OsString, OsString>,
    fallback: &Path,
    system_has: bool,
) {
    if !system_has && fallback.join(LOADER).is_file() {
        super::add_search_dir(environment, "LD_LIBRARY_PATH", fallback, true, ':');
    }
}
