//! Moteur d'images, partie propre à Windows : chargeur Vulkan de secours.
//! `sd-server.exe` importe `vulkan-1.dll`, installée par les pilotes
//! graphiques dans System32 ; sans elle (machine sans pilote Vulkan), il ne
//! démarrerait pas. Le dossier `<moteur>\fallback\` est ajouté en FIN de `PATH`
//! du serveur : Windows cherche une DLL dans System32 avant `PATH`, celle du
//! système sert donc toujours quand elle existe, et celle livrée seulement à
//! défaut (sans pilote, elle ne trouve aucune carte : le moteur calcule sur le
//! processeur). Voir `linux.rs`, son pendant.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

/// Nom du chargeur Vulkan livré dans `fallback\`.
pub(super) const LOADER: &str = "vulkan-1.dll";

/// Sans objet : l'ordre de recherche de Windows décide (voir l'en-tête).
pub(super) fn system_has_loader() -> bool {
    false
}

/// Environnement du serveur : `fallback` en fin de `PATH`.
pub(super) fn vulkan_loader(
    environment: &mut BTreeMap<OsString, OsString>,
    fallback: &Path,
    _system_has: bool,
) {
    if fallback.join(LOADER).is_file() {
        super::add_search_dir(environment, "PATH", fallback, false, ';');
    }
}
