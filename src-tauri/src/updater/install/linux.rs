//! Linux : ce que ChatonLM sait mettre à jour, et l'installation elle-même :
//! le type de paquet est lu dans une marque posée dans le binaire par
//! l'empaqueteur de Tauri (`tauri::utils::platform::bundle_type`) ; une
//! AppImage se remplace si son dossier est inscriptible, un .deb seulement
//! s'il est connu de dpkg et que pkexec est là. ChatonLM installe lui-même
//! (pas par le greffon) : AppImage écrite à côté puis mise à sa place, .deb
//! par `pkexec dpkg -i` seulement (le greffon enchaînerait sinon une boîte
//! zenity/kdialog non traduite, puis sudo sur le terminal hérité).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tauri::utils::config::BundleType;

use super::{Support, can_create_in};
use crate::i18n::t;
use crate::updater::{UnsupportedReason, UpdateInstaller};

/// Liste des fichiers du paquet `chatonlm`, tenue par dpkg.
const DPKG_LIST: &str = "/var/lib/dpkg/info/chatonlm.list";

/// Marque d'un bac à sable Flatpak (à la racine, vue seulement de
/// l'intérieur : jamais héritée comme une variable d'environnement).
const FLATPAK_INFO: &str = "/.flatpak-info";

/// Dossiers des programmes installés par un gestionnaire de paquets (dont
/// Flatpak, `/app`, et Snap, `/snap`).
const SYSTEM_PREFIXES: [&str; 4] = ["/usr/", "/opt/", "/app/", "/snap/"];

/// Codes de sortie de pkexec : boîte de mot de passe fermée ; refus
/// (mauvais mot de passe, aucun agent d'authentification, erreur).
const PKEXEC_DISMISSED: i32 = 126;
const PKEXEC_REFUSED: i32 = 127;

/// Ce qu'on sait du programme en cours.
pub(crate) struct Facts<'a> {
    pub bundle: Option<BundleType>,
    /// `APPIMAGE` (chemin de l'AppImage lancée), posé par son runtime.
    pub appimage: Option<&'a Path>,
    pub exe: Option<&'a Path>,
    /// Contenu de la liste des fichiers du paquet `chatonlm` de dpkg.
    pub dpkg_list: Option<&'a str>,
    /// pkexec trouvé (seule manière d'installer un .deb).
    pub pkexec: bool,
    /// Dans un bac à sable Flatpak.
    pub sandboxed: bool,
}

/// Manière de se mettre à jour, ou pourquoi c'est impossible.
pub(crate) fn detect(facts: &Facts) -> Support {
    if facts.sandboxed {
        return Support::Unsupported(UnsupportedReason::PackageManager);
    }
    match facts.bundle {
        Some(BundleType::AppImage) => match facts.appimage {
            Some(path) if path.is_absolute() => Support::Installable(UpdateInstaller::AppImage),
            // AppImage extraite (`--appimage-extract`) : rien à remplacer.
            _ => Support::Unsupported(UnsupportedReason::NoPackage),
        },
        // Le .deb publié, installé par dpkg, avec pkexec pour l'installer ;
        // repris dans un paquet d'une autre distribution (dpkg absent, ou ne
        // connaît pas le programme), ou système sans pkexec : son
        // gestionnaire de paquets.
        Some(BundleType::Deb) => match (facts.exe, facts.dpkg_list) {
            (Some(exe), Some(list)) if facts.pkexec && lists(list, exe) => {
                Support::Installable(UpdateInstaller::Deb)
            }
            _ => Support::Unsupported(UnsupportedReason::PackageManager),
        },
        Some(_) => Support::Unsupported(UnsupportedReason::PackageManager),
        None => match facts.exe {
            Some(exe) if is_system(exe) => Support::Unsupported(UnsupportedReason::PackageManager),
            _ => Support::Unsupported(UnsupportedReason::NoPackage),
        },
    }
}

/// Programme en cours.
pub(crate) fn detect_current() -> Support {
    let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
    let exe = std::env::current_exe().ok();
    let bundle = tauri::utils::platform::bundle_type();
    let dpkg_list = match bundle {
        Some(BundleType::Deb) => std::fs::read_to_string(DPKG_LIST).ok(),
        _ => None,
    };
    detect(&Facts {
        bundle,
        appimage: appimage.as_deref(),
        exe: exe.as_deref(),
        dpkg_list: dpkg_list.as_deref(),
        pkexec: find_program("pkexec").is_some(),
        sandboxed: Path::new(FLATPAK_INFO).exists(),
    })
}

/// AppImage à remplacer : `APPIMAGE` seulement pour une AppImage qui se met
/// à jour elle-même (un .deb lancé depuis le terminal d'un éditeur livré en
/// AppImage hérite de la sienne).
pub(crate) fn appimage_for(support: Support, appimage: Option<PathBuf>) -> Option<PathBuf> {
    match support {
        Support::Installable(UpdateInstaller::AppImage) => appimage,
        _ => None,
    }
}

/// `APPIMAGE` et `APPDIR` hérités d'une autre AppImage par un paquet qui
/// n'en est pas une (.deb lancé depuis le terminal d'un éditeur livré en
/// AppImage) : Tauri relancerait cette AppImage-là après une mise à jour et
/// prendrait son dossier pour celui des ressources. Un build sans marque de
/// paquet (développement, essais) les garde : les e2e de l'entrée de menu
/// (`e2e/menu-entry.test.mjs`) posent `APPIMAGE` pour imiter une AppImage.
fn foreign_appimage_env(bundle: Option<BundleType>) -> bool {
    bundle.is_some_and(|bundle| bundle != BundleType::AppImage)
}

/// Au tout début de `run()` : retire `APPIMAGE` et `APPDIR` hérités quand
/// ChatonLM est un paquet autre qu'une AppImage.
pub fn forget_foreign_appimage() {
    if foreign_appimage_env(tauri::utils::platform::bundle_type()) {
        // SAFETY : appelé depuis `run()` avant tout autre code (comme
        // `graphics::apply_nvidia_workaround`) ; aucun autre thread ne peut
        // lire l'environnement en même temps.
        unsafe {
            std::env::remove_var("APPIMAGE");
            std::env::remove_var("APPDIR");
        }
    }
}

/// Programme trouvé dans `PATH` (fichier exécutable).
pub(crate) fn find_program(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|file| {
            file.metadata()
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// `exe` figure dans la liste des fichiers d'un paquet de dpkg.
fn lists(list: &str, exe: &Path) -> bool {
    let exe = exe.to_string_lossy();
    list.lines().any(|line| line == exe)
}

fn is_system(exe: &Path) -> bool {
    let exe = exe.to_string_lossy();
    SYSTEM_PREFIXES.iter().any(|prefix| exe.starts_with(prefix))
}

/// L'AppImage peut être remplacée : la nouvelle est écrite à côté puis mise
/// à sa place, dans le même dossier. Il faut donc pouvoir y créer un
/// fichier, et, si le dossier a le « sticky bit » (comme /tmp), que
/// l'AppImage soit à soi.
pub(crate) fn can_replace(appimage: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    let Some(folder) = appimage.parent() else {
        return false;
    };
    let (Ok(file), Ok(dir)) = (appimage.metadata(), folder.metadata()) else {
        return false;
    };
    const STICKY: u32 = 0o1000;
    // SAFETY : geteuid n'a pas de précondition et ne peut pas échouer.
    let me = unsafe { libc::geteuid() };
    if dir.mode() & STICKY != 0 && file.uid() != me && dir.uid() != me {
        return false;
    }
    can_create_in(folder)
}

/// Remplace l'AppImage : nouvelle écrite à côté (mêmes droits), synchronisée,
/// puis renommée à sa place d'un coup. L'ancienne reste intacte tant que la
/// nouvelle n'est pas entière (le greffon renomme l'ancienne puis écrit, et
/// la remet en cas d'échec) ; l'AppImage en cours garde son fichier ouvert.
pub(crate) fn replace_appimage(appimage: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let folder = appimage.parent().ok_or_else(|| t!("updates.notReady"))?;
    let name = appimage
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mode = appimage
        .metadata()
        .map_err(|error| error.to_string())?
        .permissions()
        .mode()
        & 0o7777;
    let part = folder.join(format!(".{name}.chatonlm-update-{}", std::process::id()));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)?;
        file.write_all(bytes)?;
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&part, appimage)
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&part);
        return Err(error.to_string());
    }
    Ok(())
}

/// Installe le .deb par `pkexec dpkg -i` (boîte de mot de passe du système),
/// rien d'autre : fermée ou refusée, l'installation échoue avec un texte
/// clair. Le paquet vérifié est écrit dans un dossier temporaire à soi
/// (0700) le temps de l'installation. pkexec est lancé hors du terminal
/// (nouvelle session, sans entrée) : jamais de mot de passe demandé dans le
/// terminal d'où ChatonLM a pu être lancé.
pub(crate) fn install_deb(
    pkexec: Option<&Path>,
    version: &str,
    bytes: &[u8],
) -> Result<(), String> {
    use std::os::unix::process::CommandExt;

    let pkexec = pkexec.ok_or_else(|| t!("updates.deb.pkexecMissing"))?;
    let dir = tempfile::Builder::new()
        .prefix("chatonlm-update-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let package = dir.path().join(format!("chatonlm_{version}_amd64.deb"));
    std::fs::write(&package, bytes).map_err(|error| error.to_string())?;
    let mut command = Command::new(pkexec);
    command
        .arg("dpkg")
        .arg("-i")
        .arg(&package)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // SAFETY : setsid est sûr entre fork et exec (async-signal-safe).
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let output = match command.output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(t!("updates.deb.pkexecMissing"));
        }
        Err(error) => return Err(error.to_string()),
    };
    match output.status.code() {
        Some(0) => Ok(()),
        Some(PKEXEC_DISMISSED) => Err(t!("updates.deb.dismissed")),
        Some(PKEXEC_REFUSED) => Err(t!("updates.deb.refused")),
        code => {
            let code = code.map_or_else(|| output.status.to_string(), |code| code.to_string());
            let stderr = String::from_utf8_lossy(&output.stderr);
            match stderr
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty())
            {
                Some(line) => Err(t!("updates.deb.failed", code = code, output = line)),
                None => Err(t!("updates.deb.failedCode", code = code)),
            }
        }
    }
}

/// Après une installation réussie, ChatonLM se relance lui-même (l'AppImage
/// remplacée, ou le programme du .deb installé).
pub(crate) const INSTALLER_RELAUNCHES: bool = false;

/// Cibles de l'installation sous Linux.
#[derive(Clone, Debug, Default)]
pub(crate) struct Platform {
    /// AppImage lancée, à remplacer (dossier vérifié avant tout
    /// téléchargement) ; seulement pour une AppImage.
    pub appimage: Option<PathBuf>,
    /// pkexec, pour installer un .deb.
    pub pkexec: Option<PathBuf>,
}

impl Platform {
    /// Programme en cours (`data_dir` ne sert que sous Windows).
    pub fn current(support: Support, _data_dir: &Path) -> Self {
        Self {
            appimage: appimage_for(support, std::env::var_os("APPIMAGE").map(PathBuf::from)),
            pkexec: find_program("pkexec"),
        }
    }

    /// De quoi installer par `installer` : une AppImage se remplace
    /// seulement si son fichier est connu.
    pub fn ready(&self, installer: UpdateInstaller) -> bool {
        installer != UpdateInstaller::AppImage || self.appimage.is_some()
    }

    /// AppImage dans un dossier où elle ne peut pas être remplacée.
    pub fn location_problem(&self) -> Option<UnsupportedReason> {
        self.appimage
            .as_deref()
            .is_some_and(|path| !can_replace(path))
            .then_some(UnsupportedReason::ReadOnlyFolder)
    }

    /// Texte du refus de « Restart to update » pour ce dossier.
    pub fn read_only_text() -> String {
        t!("updates.readOnlyFolder")
    }

    /// Échec à noter avant l'installation (aucun : ChatonLM attend la fin de
    /// l'installation et note lui-même un échec).
    pub fn pending_failure(&self) -> Option<String> {
        None
    }

    /// Installe : AppImage remplacée sur place, .deb par `pkexec dpkg -i`.
    pub fn install(
        &self,
        installer: UpdateInstaller,
        version: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        match (installer, self.appimage.as_deref()) {
            (UpdateInstaller::AppImage, Some(appimage)) => replace_appimage(appimage, bytes),
            (UpdateInstaller::AppImage, None) => Err(t!("updates.notReady")),
            (UpdateInstaller::Deb, _) => install_deb(self.pkexec.as_deref(), version, bytes),
            (UpdateInstaller::Nsis, _) => Err(t!("updates.notReady")),
        }
    }

    /// Rien à nettoyer au démarrage.
    pub fn startup_cleanup(&self) {}
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn facts<'a>(
        bundle: Option<BundleType>,
        appimage: Option<&'a Path>,
        exe: &'a Path,
        dpkg_list: Option<&'a str>,
    ) -> Facts<'a> {
        Facts {
            bundle,
            appimage,
            exe: Some(exe),
            dpkg_list,
            pkexec: true,
            sandboxed: false,
        }
    }

    /// Faux pkexec : note ses arguments (une ligne par appel, chacun suivi
    /// de « | »), garde une copie du paquet reçu, écrit `stderr` puis sort
    /// avec `code`.
    pub(crate) struct FakePkexec {
        _dir: tempfile::TempDir,
        pub program: PathBuf,
        log: PathBuf,
        copy: PathBuf,
    }

    impl FakePkexec {
        pub fn new(code: i32) -> Self {
            Self::with_stderr(code, "")
        }

        pub fn with_stderr(code: i32, stderr: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let program = dir.path().join("pkexec");
            let log = dir.path().join("calls");
            let copy = dir.path().join("received.deb");
            let script = format!(
                "#!/bin/sh\nprintf '%s|' \"$@\" >> '{log}'\necho >> '{log}'\ncat \"$3\" > '{copy}'\nprintf '%s' '{stderr}' >&2\nexit {code}\n",
                log = log.display(),
                copy = copy.display(),
            );
            std::fs::write(&program, script).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self {
                _dir: dir,
                program,
                log,
                copy,
            }
        }

        /// Arguments de chaque appel.
        pub fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(&self.log)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        pub fn received(&self) -> Vec<u8> {
            std::fs::read(&self.copy).unwrap_or_default()
        }
    }

    #[test]
    fn a_deb_goes_through_pkexec_dpkg_and_nothing_else() {
        let deb = b"!<arch>\ndebian-binary   2.0".to_vec();
        let fake = FakePkexec::new(0);
        assert_eq!(install_deb(Some(&fake.program), "9.9.9", &deb), Ok(()));
        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        let args: Vec<&str> = calls[0].split('|').collect();
        assert_eq!(args.len(), 4, "{calls:?}");
        assert_eq!(args[..2], ["dpkg", "-i"]);
        let package = Path::new(args[2]);
        assert!(package.ends_with("chatonlm_9.9.9_amd64.deb"), "{package:?}");
        assert_eq!(fake.received(), deb);
        // Dossier temporaire supprimé ensuite.
        assert!(!package.exists());
        assert!(!package.parent().unwrap().exists());

        // Boîte fermée, refus : un texte clair, et un seul appel (jamais de
        // repli zenity, kdialog ou sudo).
        for (code, text) in [
            (PKEXEC_DISMISSED, t!("updates.deb.dismissed")),
            (PKEXEC_REFUSED, t!("updates.deb.refused")),
        ] {
            let fake = FakePkexec::new(code);
            assert_eq!(install_deb(Some(&fake.program), "9.9.9", &deb), Err(text));
            assert_eq!(fake.calls().len(), 1);
        }
        // Échec de dpkg : son code et sa dernière ligne d'erreur.
        let fake = FakePkexec::with_stderr(1, "dpkg: warning\ndpkg: dependency problems\n");
        assert_eq!(
            install_deb(Some(&fake.program), "9.9.9", &deb),
            Err(t!(
                "updates.deb.failed",
                code = "1",
                output = "dpkg: dependency problems"
            ))
        );
        let fake = FakePkexec::new(2);
        assert_eq!(
            install_deb(Some(&fake.program), "9.9.9", &deb),
            Err(t!("updates.deb.failedCode", code = "2"))
        );
        // Sans pkexec.
        assert_eq!(
            install_deb(None, "9.9.9", &deb),
            Err(t!("updates.deb.pkexecMissing"))
        );
        assert_eq!(
            install_deb(Some(Path::new("/nonexistent/pkexec")), "9.9.9", &deb),
            Err(t!("updates.deb.pkexecMissing"))
        );
    }

    #[test]
    fn pkexec_runs_without_the_terminal() {
        // Nouvelle session (setsid) : pas de terminal de contrôle, entrée
        // vide (jamais de mot de passe demandé dans le terminal hérité).
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("pkexec");
        let out = dir.path().join("out");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\n{{ awk '{{print $6}}' /proc/$$/stat; echo $$; cat; }} > '{}' 2>&1\nexit 0\n",
                out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        install_deb(Some(&program), "9.9.9", b"!<arch>\ndebian-binary").unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert_eq!(lines[0], lines[1], "chef de sa propre session : {text}");
    }

    #[test]
    fn replacing_the_appimage_keeps_its_mode_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("ChatonLM.AppImage");
        std::fs::write(&image, b"old").unwrap();
        std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o750)).unwrap();
        replace_appimage(&image, b"new").unwrap();
        assert_eq!(std::fs::read(&image).unwrap(), b"new");
        assert_eq!(
            image.metadata().unwrap().permissions().mode() & 0o777,
            0o750
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        // AppImage disparue : erreur, rien d'écrit.
        assert!(replace_appimage(&dir.path().join("missing.AppImage"), b"new").is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn only_an_appimage_uses_appimage() {
        let image = Some(PathBuf::from("/opt/Editor/Editor.AppImage"));
        assert_eq!(
            appimage_for(
                Support::Installable(UpdateInstaller::AppImage),
                image.clone()
            ),
            image
        );
        for support in [
            Support::Installable(UpdateInstaller::Deb),
            Support::Unsupported(UnsupportedReason::NoPackage),
            Support::Unsupported(UnsupportedReason::PackageManager),
        ] {
            assert_eq!(appimage_for(support, image.clone()), None, "{support:?}");
        }
        assert!(!foreign_appimage_env(Some(BundleType::AppImage)));
        assert!(foreign_appimage_env(Some(BundleType::Deb)));
        assert!(foreign_appimage_env(Some(BundleType::Rpm)));
        // Build sans paquet (e2e de l'entrée de menu) : gardées.
        assert!(!foreign_appimage_env(None));
    }

    #[test]
    fn flatpak_and_snap_builds_point_to_the_package_manager() {
        let exe = Path::new("/home/me/src/chatonlm/target/release/chatonlm");
        let sandboxed = Facts {
            sandboxed: true,
            ..facts(None, None, exe, None)
        };
        assert_eq!(
            detect(&sandboxed),
            Support::Unsupported(UnsupportedReason::PackageManager)
        );
        for exe in ["/app/bin/chatonlm", "/snap/chatonlm/12/usr/bin/chatonlm"] {
            assert_eq!(
                detect(&facts(None, None, Path::new(exe), None)),
                Support::Unsupported(UnsupportedReason::PackageManager),
                "{exe}"
            );
        }
    }

    #[test]
    fn a_program_is_found_in_the_path() {
        assert!(find_program("sh").is_some_and(|path| path.is_absolute()));
        assert_eq!(find_program("chatonlm-no-such-program"), None);
    }

    #[test]
    fn an_appimage_updates_itself_only_when_launched_from_its_file() {
        let exe = Path::new("/tmp/.mount_libreXY/usr/bin/chatonlm");
        let image = Path::new("/home/me/Apps/ChatonLM.AppImage");
        assert_eq!(
            detect(&facts(Some(BundleType::AppImage), Some(image), exe, None)),
            Support::Installable(UpdateInstaller::AppImage)
        );
        // Extraite (pas d'APPIMAGE), ou valeur relative.
        assert_eq!(
            detect(&facts(Some(BundleType::AppImage), None, exe, None)),
            Support::Unsupported(UnsupportedReason::NoPackage)
        );
        assert_eq!(
            detect(&facts(
                Some(BundleType::AppImage),
                Some(Path::new("ChatonLM.AppImage")),
                exe,
                None
            )),
            Support::Unsupported(UnsupportedReason::NoPackage)
        );
    }

    #[test]
    fn a_deb_updates_itself_only_when_dpkg_knows_the_program() {
        let exe = Path::new("/usr/bin/chatonlm");
        let list = "/.\n/usr\n/usr/bin\n/usr/bin/chatonlm\n/usr/lib/chatonlm\n";
        assert_eq!(
            detect(&facts(Some(BundleType::Deb), None, exe, Some(list))),
            Support::Installable(UpdateInstaller::Deb)
        );
        // Sans pkexec (Debian minimale) : dit avant tout téléchargement.
        let no_pkexec = Facts {
            pkexec: false,
            ..facts(Some(BundleType::Deb), None, exe, Some(list))
        };
        assert_eq!(
            detect(&no_pkexec),
            Support::Unsupported(UnsupportedReason::PackageManager)
        );
        // Paquet d'une distribution fait du .deb (Arch : pas de dpkg), ou
        // programme ailleurs que dans la liste.
        for (exe, list) in [
            (exe, None),
            (Path::new("/opt/chatonlm/chatonlm"), Some(list)),
            (exe, Some("/usr/bin/chatonlm-old\n")),
        ] {
            assert_eq!(
                detect(&facts(Some(BundleType::Deb), None, exe, list)),
                Support::Unsupported(UnsupportedReason::PackageManager),
                "{exe:?}"
            );
        }
    }

    #[test]
    fn other_builds_point_to_the_package_manager_or_say_there_is_no_package() {
        assert_eq!(
            detect(&facts(
                Some(BundleType::Rpm),
                None,
                Path::new("/usr/bin/chatonlm"),
                None
            )),
            Support::Unsupported(UnsupportedReason::PackageManager)
        );
        // Sans marque : paquet construit par une distribution (programme du
        // système), ou build de développement.
        assert_eq!(
            detect(&facts(None, None, Path::new("/usr/bin/chatonlm"), None)),
            Support::Unsupported(UnsupportedReason::PackageManager)
        );
        assert_eq!(
            detect(&facts(
                None,
                Some(Path::new("/home/me/ChatonLM.AppImage")),
                Path::new("/home/me/src/chatonlm/target/release/chatonlm"),
                None
            )),
            Support::Unsupported(UnsupportedReason::NoPackage)
        );
        // Programme des essais : jamais installable.
        assert!(matches!(detect_current(), Support::Unsupported(_)));
    }

    #[test]
    fn a_read_only_folder_cannot_take_the_new_appimage() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("apps");
        std::fs::create_dir(&folder).unwrap();
        let image = folder.join("ChatonLM.AppImage");
        std::fs::write(&image, b"old").unwrap();
        assert!(can_replace(&image));
        // Aucun fichier d'essai laissé.
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);

        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
        let replaceable = can_replace(&image);
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!replaceable);
        assert!(!can_replace(&folder.join("missing.AppImage")));
    }
}
