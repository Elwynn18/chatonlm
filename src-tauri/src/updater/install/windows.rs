//! Windows : appels au système de la mise à jour (clé de désinstallation
//! lue dans le registre, installateur lancé). Les règles (ce qui est pris en
//! charge, arguments, textes) sont dans `nsis.rs`, sans appel système.
//!
//! Un programme en cours ne se remplace pas sous Windows : l'installateur
//! NSIS vérifié est copié dans un dossier à soi, lancé détaché, puis ChatonLM
//! se ferme ; l'installateur écrit les fichiers et relance ChatonLM. Ses
//! services (dont l'Ollama intégré, qui verrouille ses fichiers) sont arrêtés
//! avant, par `stop_services` ; l'installateur arrête aussi ce qui resterait
//! (crochet `packaging/windows/hooks.nsh`).

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::nsis::{self, Facts};
use super::{Support, can_create_in};
use crate::i18n::t;
use crate::updater::{UnsupportedReason, UpdateInstaller};

/// Après une installation lancée, ChatonLM se ferme : c'est l'installateur
/// qui le relance (`/R`).
pub(crate) const INSTALLER_RELAUNCHES: bool = true;

/// Installateur sans console, hors du groupe de ChatonLM : il lui survit.
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Fichier tout juste écrit encore tenu par un antivirus
/// (`ERROR_SHARING_VIOLATION`) : nouveaux essais du lancement.
const SHARING_VIOLATION: i32 = 32;
const LAUNCH_TRIES: u32 = 10;
const LAUNCH_PAUSE: Duration = Duration::from_millis(500);

/// Installateur de la mise à jour précédente, encore en cours quelques
/// secondes quand il relance ChatonLM : suppression retentée (comme
/// `DoPostUpgradeCleanup` de l'officiel).
const CLEANUP_TRIES: u32 = 10;
const CLEANUP_PAUSE: Duration = Duration::from_secs(5);

/// Programme en cours ; `product` : `productName` de la configuration (nom
/// de la clé de désinstallation écrite par l'installateur).
pub(crate) fn detect_current(product: &str) -> Support {
    let exe = std::env::current_exe().ok().map(|exe| plain(&exe));
    let key = windows_registry::CURRENT_USER
        .open(nsis::uninstall_key(product))
        .ok();
    let value = |name: &str| key.as_ref().and_then(|key| key.get_string(name).ok());
    // Même écriture des deux côtés quand le dossier existe (liens, noms
    // courts) ; sinon la valeur telle quelle.
    let install_location = value(nsis::INSTALL_LOCATION).map(|location| {
        let folder = PathBuf::from(location.trim().trim_matches('"'));
        match dunce::canonicalize(&folder) {
            Ok(folder) => folder.to_string_lossy().into_owned(),
            Err(_) => location,
        }
    });
    let main_binary = value(nsis::MAIN_BINARY_NAME);
    nsis::detect(&Facts {
        bundle: tauri::utils::platform::bundle_type(),
        exe: exe.as_deref(),
        install_location: install_location.as_deref(),
        main_binary: main_binary.as_deref(),
    })
}

/// Chemin sans liens ni préfixe `\\?\`, si le fichier existe.
fn plain(path: &Path) -> String {
    dunce::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Cibles de l'installation sous Windows.
#[derive(Clone, Debug, Default)]
pub(crate) struct Platform {
    /// Dossier d'installation (celui du programme), que l'installateur
    /// réécrit ; seulement pour ChatonLM installé.
    pub install_dir: Option<PathBuf>,
    /// Dossier où l'installateur vérifié est copié puis lancé.
    pub run_dir: Option<PathBuf>,
}

impl Platform {
    /// Programme en cours ; `data_dir` : dossier des données de ChatonLM.
    pub fn current(support: Support, data_dir: &Path) -> Self {
        let install_dir = match support {
            Support::Installable(UpdateInstaller::Nsis) => std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf)),
            _ => None,
        };
        Self {
            install_dir,
            run_dir: Some(data_dir.join(nsis::RUN_FOLDER)),
        }
    }

    /// De quoi installer par `installer` : seulement l'installateur NSIS,
    /// et un dossier où le copier.
    pub fn ready(&self, installer: UpdateInstaller) -> bool {
        installer == UpdateInstaller::Nsis && self.run_dir.is_some()
    }

    /// Dossier d'installation non inscriptible (ChatonLM installé en
    /// administrateur dans un dossier du système) : l'installateur, lancé
    /// sans élévation, ne pourrait pas y écrire.
    pub fn location_problem(&self) -> Option<UnsupportedReason> {
        self.install_dir
            .as_deref()
            .is_some_and(|folder| !can_create_in(folder))
            .then_some(UnsupportedReason::InstallFolderReadOnly)
    }

    /// Texte du refus de « Restart to update » pour ce dossier.
    pub fn read_only_text() -> String {
        t!("updates.windows.installFolderReadOnly")
    }

    /// Échec noté AVANT de lancer l'installateur : ChatonLM se ferme sans en
    /// connaître l'issue. L'installation réussie, la nouvelle version
    /// l'efface au démarrage sans le montrer ; sinon l'ancienne, relancée à
    /// la main, le montre et garde la mise à jour prête.
    pub fn pending_failure(&self) -> Option<String> {
        Some(t!("updates.windows.notCompleted"))
    }

    /// Lance l'installateur vérifié (services déjà arrêtés).
    pub fn install(
        &self,
        installer: UpdateInstaller,
        version: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        match (installer, self.run_dir.as_deref()) {
            (UpdateInstaller::Nsis, Some(folder)) => {
                run_installer(folder, version, bytes, &nsis::INSTALLER_ARGS)
            }
            _ => Err(t!("updates.notReady")),
        }
    }

    /// Démarrage : installateur de la mise à jour précédente supprimé, en
    /// fond (il peut tourner encore un instant).
    pub fn startup_cleanup(&self) {
        let Some(folder) = self.run_dir.clone().filter(|folder| folder.exists()) else {
            return;
        };
        std::thread::spawn(move || {
            for _ in 0..CLEANUP_TRIES {
                if std::fs::remove_dir_all(&folder).is_ok() || !folder.exists() {
                    return;
                }
                std::thread::sleep(CLEANUP_PAUSE);
            }
            eprintln!("chatonlm: cannot remove {}", folder.display());
        });
    }
}

/// Copie l'installateur vérifié dans `folder` (vidé d'abord), puis le lance
/// détaché avec `args`, sans attendre sa fin : il ne peut écrire les
/// fichiers de ChatonLM qu'une fois celui-ci fermé.
fn run_installer(folder: &Path, version: &str, bytes: &[u8], args: &[&str]) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(folder);
    std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    let installer = folder.join(nsis::installer_name(version));
    let written = (|| {
        let mut file = std::fs::File::create(&installer)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&installer);
        return Err(error.to_string());
    }
    let mut tries = 0;
    loop {
        let spawned = Command::new(&installer)
            .args(args)
            .current_dir(folder)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
            .spawn();
        match spawned {
            Ok(_) => return Ok(()),
            Err(error) => {
                tries += 1;
                if error.raw_os_error() == Some(SHARING_VIOLATION) && tries < LAUNCH_TRIES {
                    std::thread::sleep(LAUNCH_PAUSE);
                    continue;
                }
                let _ = std::fs::remove_file(&installer);
                return Err(nsis::launch_error_text(
                    error.raw_os_error(),
                    &error.to_string(),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_program_is_not_an_installed_chatonlm() {
        assert_eq!(
            detect_current("chatonlm-no-such-product"),
            Support::Unsupported(UnsupportedReason::NotInstalled)
        );
    }

    #[test]
    fn the_verified_installer_is_copied_then_run_with_its_arguments() {
        // Faux installateur : l'interpréteur de commandes de Windows, qui
        // écrit un témoin dans son dossier de travail.
        let system = std::env::var_os("SystemRoot").expect("SystemRoot");
        let program = std::fs::read(Path::new(&system).join("System32").join("cmd.exe")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join(nsis::RUN_FOLDER);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("old-setup.exe"), b"old").unwrap();
        run_installer(&folder, "9.9.9", &program, &["/C", "echo run>witness.txt"]).unwrap();
        let installer = folder.join("chatonlm-9.9.9-setup.exe");
        assert_eq!(std::fs::read(&installer).unwrap(), program);
        assert!(!folder.join("old-setup.exe").exists());
        let witness = folder.join("witness.txt");
        for _ in 0..100 {
            if witness.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(witness.exists(), "installateur non lancé");
    }

    #[test]
    fn a_file_that_is_not_a_program_fails_with_a_clear_text() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join(nsis::RUN_FOLDER);
        let error = run_installer(&folder, "9.9.9", b"not a program", &[]).unwrap_err();
        assert!(!error.is_empty());
        // Rien de laissé à lancer.
        assert!(!folder.join("chatonlm-9.9.9-setup.exe").exists());
    }

    #[test]
    fn a_read_only_install_folder_is_a_location_problem() {
        let dir = tempfile::tempdir().unwrap();
        let installed = Platform {
            install_dir: Some(dir.path().to_path_buf()),
            run_dir: None,
        };
        assert_eq!(installed.location_problem(), None);
        let missing = Platform {
            install_dir: Some(dir.path().join("missing")),
            run_dir: None,
        };
        assert_eq!(
            missing.location_problem(),
            Some(UnsupportedReason::InstallFolderReadOnly)
        );
        assert_eq!(Platform::default().location_problem(), None);
        assert!(!Platform::default().ready(UpdateInstaller::Nsis));
        assert!(!installed.ready(UpdateInstaller::AppImage));
    }
}
