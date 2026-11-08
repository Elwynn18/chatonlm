//! Binaire `llama-server` livré avec l'app.
//!
//! - Paquets : `<ressources>/llama-server/llama-server{EXE_SUFFIX}` (Linux
//!   `/usr/lib/chatonlm/llama-server/`, l'AppImage par un lien vers
//!   `usr/libexec/chatonlm/llama-server` ; Windows
//!   `<installation>\llama-server\`), ses bibliothèques et moteurs de calcul à
//!   côté, `VERSION`, et le chargeur Vulkan de secours dans `fallback/`.
//! - Builds de débogage : à défaut, la construction locale
//!   `.tools/runtimes/llama-server/` (`scripts/build-llama-server.sh`).
//! - Builds de débogage et d'essai : `CHATONLM_LLAMA_SERVER_BIN` (chemin
//!   absolu d'un exécutable, faux binaire `fake-llama` des essais ; vide :
//!   aucun binaire) remplace tout le reste.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use tauri::Manager;

use crate::process::is_executable;

/// Dossier du moteur, sous les ressources comme dans la construction locale.
pub const DIR: &str = "llama-server";

/// Variable des builds de débogage et d'essai qui donne le binaire.
pub const BIN_ENV: &str = "CHATONLM_LLAMA_SERVER_BIN";

/// Dossier du chargeur Vulkan de secours, à côté du binaire.
pub const FALLBACK_DIR: &str = "fallback";

/// Nom du binaire dans son dossier.
pub fn file_name() -> String {
    format!("llama-server{}", std::env::consts::EXE_SUFFIX)
}

/// Binaire de `llama-server` pour cette app, ou `None` s'il n'y en a pas.
pub fn llama_server_bin(app: &tauri::AppHandle) -> Option<PathBuf> {
    #[cfg(any(debug_assertions, feature = "e2e"))]
    let overridden = std::env::var_os(BIN_ENV);
    #[cfg(not(any(debug_assertions, feature = "e2e")))]
    let overridden: Option<std::ffi::OsString> = None;
    locate(
        overridden.as_deref(),
        app.path().resource_dir().ok().as_deref(),
        dev_dir().as_deref(),
    )
}

/// Construction locale (`.tools/runtimes/llama-server` du projet), dans les
/// builds de débogage seulement.
fn dev_dir() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        Some(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join(".tools")
                .join("runtimes")
                .join(DIR),
        )
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

/// Choix du binaire : `overridden` (valeur de [`BIN_ENV`], si elle est
/// permise) l'emporte (absolu et exécutable, sinon aucun ; vide : aucun),
/// puis celui des ressources, puis celui de `dev_dir`.
pub(crate) fn locate(
    overridden: Option<&OsStr>,
    resource_dir: Option<&Path>,
    dev_dir: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(path) = overridden {
        let path = PathBuf::from(path);
        return (path.is_absolute() && is_executable(&path)).then_some(path);
    }
    [
        resource_dir.map(|dir| dir.join(DIR)),
        dev_dir.map(Path::to_path_buf),
    ]
    .into_iter()
    .flatten()
    .map(|dir| dir.join(file_name()))
    .find(|bin| is_executable(bin))
}

/// Dossier du binaire `bin` (absolu), à donner comme dossier de travail
/// (`current_dir`) à `llama-server` : ggml charge les moteurs de calcul
/// (`libggml-<nom>-*.so`, `ggml-<nom>-*.dll`) du dossier de l'exécutable ET
/// du dossier courant (`ggml-backend-reg.cpp`), en exécutant leur code.
/// Lancé depuis le dossier courant de l'app (un terminal ouvert dans un
/// dossier llama.cpp, KoboldCpp ou Ollama), il chargerait ceux d'une autre
/// version avec notre `ggml-base`. Dans son propre dossier, il ne voit que
/// les nôtres. `None` pour un chemin relatif ou sans dossier : jamais le
/// dossier courant.
pub fn llama_server_dir(bin: &Path) -> Option<PathBuf> {
    let dir = bin.parent()?;
    (dir.is_absolute() && bin.file_name().is_some()).then(|| dir.to_path_buf())
}

/// Chargeur Vulkan de secours livré à côté de `bin` (dossier `fallback/`).
pub fn fallback_dir(bin: &Path) -> Option<PathBuf> {
    Some(llama_server_dir(bin)?.join(FALLBACK_DIR))
}

/// Version construite (`VERSION` à côté de `bin` :
/// `<version épinglée>+chatonlm.<empreinte>`), si elle est lisible.
pub fn version(bin: &Path) -> Option<String> {
    let text = std::fs::read_to_string(bin.parent()?.join("VERSION")).ok()?;
    let version = text.trim();
    (!version.is_empty()).then(|| version.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Faux exécutable `llama-server` dans `dir/llama-server/`.
    fn install(dir: &Path) -> PathBuf {
        let folder = dir.join(DIR);
        std::fs::create_dir_all(&folder).unwrap();
        let bin = folder.join(file_name());
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        bin
    }

    #[test]
    fn the_bundled_binary_comes_first_then_the_local_build() {
        let resources = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let dev = install(tools.path());
        let dev_dir = dev.parent().unwrap();
        // Sans ressources : la construction locale.
        assert_eq!(
            locate(None, Some(resources.path()), Some(dev_dir)),
            Some(dev.clone())
        );
        assert_eq!(locate(None, None, Some(dev_dir)), Some(dev.clone()));
        // Avec : celui des ressources.
        let bundled = install(resources.path());
        assert_eq!(
            locate(None, Some(resources.path()), Some(dev_dir)),
            Some(bundled)
        );
        // Ni l'un ni l'autre.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(locate(None, Some(empty.path()), None), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_cannot_run_is_not_a_binary() {
        use std::os::unix::fs::PermissionsExt;
        let resources = tempfile::tempdir().unwrap();
        let bin = install(resources.path());
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(locate(None, Some(resources.path()), None), None);
    }

    #[test]
    fn the_test_override_wins_and_can_disable_the_engine() {
        let resources = tempfile::tempdir().unwrap();
        install(resources.path());
        let fake = tempfile::tempdir().unwrap();
        let fake_bin = install(fake.path());
        assert_eq!(
            locate(Some(fake_bin.as_os_str()), Some(resources.path()), None),
            Some(fake_bin.clone())
        );
        // Vide, relatif ou absent : aucun binaire (jamais celui livré).
        for value in ["", "llama-server", "/nonexistent/llama-server"] {
            assert_eq!(
                locate(Some(OsStr::new(value)), Some(resources.path()), None),
                None,
                "{value:?}"
            );
        }
    }

    #[test]
    fn version_and_fallback_sit_next_to_the_binary() {
        let resources = tempfile::tempdir().unwrap();
        let bin = install(resources.path());
        assert_eq!(version(&bin), None);
        std::fs::write(
            bin.parent().unwrap().join("VERSION"),
            "v0.9.0+chatonlm.0123456789ab\n",
        )
        .unwrap();
        assert_eq!(
            version(&bin).as_deref(),
            Some("v0.9.0+chatonlm.0123456789ab")
        );
        assert_eq!(
            fallback_dir(&bin),
            Some(resources.path().join(DIR).join(FALLBACK_DIR))
        );
    }

    #[test]
    fn the_working_dir_is_the_engine_folder_never_the_current_one() {
        let resources = tempfile::tempdir().unwrap();
        let bin = install(resources.path());
        assert_eq!(llama_server_dir(&bin), Some(resources.path().join(DIR)));
        // Le binaire trouvé par `locate` a toujours un dossier à nous.
        let found = locate(None, Some(resources.path()), None).unwrap();
        assert_eq!(llama_server_dir(&found), Some(resources.path().join(DIR)));
        // Relatif (« llama-server », « ./llama-server ») ou sans nom : pas de
        // dossier, plutôt que le dossier courant (« », « . »).
        for path in [
            "llama-server",
            "./llama-server",
            "bin/llama-server",
            "/",
            "",
        ] {
            assert_eq!(llama_server_dir(Path::new(path)), None, "{path:?}");
            assert_eq!(fallback_dir(Path::new(path)), None, "{path:?}");
        }
    }

    /// La construction locale est cherchée au bon endroit du projet.
    #[cfg(debug_assertions)]
    #[test]
    fn the_local_build_is_under_the_project_tools() {
        // Lu sans le disque : le dossier peut ne pas être construit.
        let dir = dev_dir().unwrap();
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(manifest.join("Cargo.toml").is_file());
        assert_eq!(
            dir,
            manifest.join("..").join(".tools/runtimes/llama-server")
        );
    }
}
