//! Mise à jour sous Windows, sans aucun appel système (essayée aussi sous
//! Linux) : ChatonLM y est posé par l'installateur NSIS de Tauri, par
//! utilisateur, et se met à jour en lançant l'installateur de la nouvelle
//! version. Les appels à Windows (registre, lancement) sont dans
//! `windows.rs`.
//!
//! Comme l'app officielle d'Ollama (`app/updater/updater_windows.go`) :
//! l'installateur vérifié est lancé avec une fenêtre de progression, sans
//! question, puis l'app se ferme et l'installateur la relance. Écarts
//! voulus : signature minisign de ChatonLM, version et nom de fichier signés
//! et forme de l'installateur, à la place de la signature Authenticode de
//! l'éditeur (ChatonLM n'en a pas) ; installateur copié (la mise à jour gardée
//! reste, pour un nouvel essai) au lieu d'être déplacé ; pas de journal
//! (NSIS n'en écrit pas) ; jamais d'installation seule au démarrage.
#![cfg_attr(not(windows), allow(dead_code))]

use tauri::utils::config::BundleType;

use super::Support;
use crate::i18n::t;
use crate::updater::{UnsupportedReason, UpdateInstaller};

/// Arguments de l'installateur NSIS de Tauri (ceux du greffon en mode
/// « passive ») : `/P` fenêtre de progression seule, sans question (comme
/// `/SILENT` de l'officiel) ; `/UPDATE` mise à jour (l'ancienne version
/// n'est pas désinstallée, raccourcis et données gardés) ; `/R` relance
/// ChatonLM à la fin.
pub(crate) const INSTALLER_ARGS: [&str; 3] = ["/P", "/UPDATE", "/R"];

/// Dossier (dans celui des données) où l'installateur vérifié est copié
/// puis lancé : hors de `updates`, vidé pendant qu'il tourne encore.
pub(crate) const RUN_FOLDER: &str = "update-run";

/// Clés de désinstallation de l'utilisateur (HKCU) : l'installateur y écrit
/// la sienne, au nom du produit (`UNINSTKEY` de `installer.nsi`).
const UNINSTALL_KEYS: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

/// Valeurs lues dans la clé de désinstallation.
pub(crate) const INSTALL_LOCATION: &str = "InstallLocation";
pub(crate) const MAIN_BINARY_NAME: &str = "MainBinaryName";

/// Erreurs de Windows au lancement d'un programme refusé par une règle ou
/// un antivirus : accès refusé, fichier infecté (gardé ou supprimé), règle
/// de restriction logicielle, règle d'intégrité (Smart App Control, WDAC).
const BLOCKED_ERRORS: [i32; 5] = [5, 225, 226, 1260, 4551];

/// Clé de désinstallation de `product` (`productName` de la configuration).
pub(crate) fn uninstall_key(product: &str) -> String {
    format!(r"{UNINSTALL_KEYS}\{product}")
}

/// Ce qu'on sait du programme en cours.
pub(crate) struct Facts<'a> {
    pub bundle: Option<BundleType>,
    /// Chemin complet du programme.
    pub exe: Option<&'a str>,
    /// `InstallLocation` de la clé de désinstallation (entre guillemets).
    pub install_location: Option<&'a str>,
    /// `MainBinaryName` de la même clé.
    pub main_binary: Option<&'a str>,
}

/// ChatonLM se met à jour lui-même seulement s'il a été posé par son
/// installateur (marque du binaire) ET s'il tourne depuis le dossier que la
/// clé de désinstallation de l'utilisateur désigne : l'installateur de la
/// nouvelle version écrit dans ce dossier-là. Une copie du dossier, une
/// extraction ou un build sans paquet resteraient à l'ancienne version.
pub(crate) fn detect(facts: &Facts) -> Support {
    let installed = facts.bundle == Some(BundleType::Nsis)
        && match (facts.exe, facts.install_location) {
            (Some(exe), Some(location)) => {
                let (folder, name) = split_program(exe);
                same_folder(folder, location)
                    && facts
                        .main_binary
                        .is_none_or(|main| main.trim().eq_ignore_ascii_case(name))
            }
            _ => false,
        };
    if installed {
        Support::Installable(UpdateInstaller::Nsis)
    } else {
        Support::Unsupported(UnsupportedReason::NotInstalled)
    }
}

/// Dossier et nom d'un programme donné par son chemin Windows.
pub(crate) fn split_program(exe: &str) -> (&str, &str) {
    match exe.rfind(['\\', '/']) {
        Some(at) => (&exe[..at], &exe[at + 1..]),
        None => ("", exe),
    }
}

/// Deux écritures du même dossier Windows : guillemets (ceux de la clé de
/// désinstallation), préfixe `\\?\`, sens des barres, barre finale et casse
/// ignorés. Un dossier vide n'est égal à aucun autre.
pub(crate) fn same_folder(a: &str, b: &str) -> bool {
    let (a, b) = (plain_folder(a), plain_folder(b));
    !a.is_empty() && a == b
}

fn plain_folder(folder: &str) -> String {
    let folder = folder.trim().trim_matches('"').trim().replace('/', "\\");
    let folder = folder.strip_prefix(r"\\?\").unwrap_or(&folder);
    folder.trim_end_matches('\\').to_lowercase()
}

/// Nom du fichier de l'installateur copié pour être lancé (la version,
/// validée par le greffon, n'y laisse que des caractères sûrs).
pub(crate) fn installer_name(version: &str) -> String {
    let safe: String = version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || ".-+".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("chatonlm-{safe}-setup.exe")
}

/// Texte d'un échec du lancement de l'installateur : refus de Windows ou
/// d'un antivirus (`code` : erreur de Windows), sinon l'erreur du système.
pub(crate) fn launch_error_text(code: Option<i32>, error: &str) -> String {
    match code {
        Some(code) if BLOCKED_ERRORS.contains(&code) => t!("updates.windows.blocked"),
        _ => t!("updates.windows.launchFailed", error = error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Users\Zoé\AppData\Local\chatonlm\chatonlm.exe";
    const LOCATION: &str = r#""C:\Users\Zoé\AppData\Local\chatonlm""#;

    fn facts<'a>(
        bundle: Option<BundleType>,
        exe: &'a str,
        install_location: Option<&'a str>,
    ) -> Facts<'a> {
        Facts {
            bundle,
            exe: Some(exe),
            install_location,
            main_binary: Some("chatonlm.exe"),
        }
    }

    const INSTALLED: Support = Support::Installable(UpdateInstaller::Nsis);
    const NOT_INSTALLED: Support = Support::Unsupported(UnsupportedReason::NotInstalled);

    #[test]
    fn the_installed_program_updates_itself() {
        let nsis = || Some(BundleType::Nsis);
        assert_eq!(detect(&facts(nsis(), EXE, Some(LOCATION))), INSTALLED);
        // Autres écritures du même dossier : casse, barres, préfixe long,
        // barre finale, sans guillemets.
        for (exe, location) in [
            (
                r"c:\users\zoé\appdata\local\CHATONLM\ChatonLM.exe",
                LOCATION,
            ),
            (
                r"\\?\C:\Users\Zoé\AppData\Local\chatonlm\chatonlm.exe",
                LOCATION,
            ),
            (EXE, r"C:\Users\Zoé\AppData\Local\chatonlm\"),
            (EXE, "C:/Users/Zoé/AppData/Local/chatonlm"),
        ] {
            assert_eq!(
                detect(&facts(nsis(), exe, Some(location))),
                INSTALLED,
                "{exe} {location}"
            );
        }
        // Clé écrite par un installateur plus ancien, sans nom du programme.
        let unnamed = Facts {
            main_binary: None,
            ..facts(nsis(), EXE, Some(LOCATION))
        };
        assert_eq!(detect(&unnamed), INSTALLED);
    }

    #[test]
    fn a_copy_or_a_build_without_installer_cannot_update_itself() {
        let nsis = || Some(BundleType::Nsis);
        // Dossier copié ailleurs, extraction : la clé désigne un autre
        // dossier, ou n'existe pas.
        let copy = r"D:\Portable\chatonlm\chatonlm.exe";
        assert_eq!(detect(&facts(nsis(), copy, Some(LOCATION))), NOT_INSTALLED);
        assert_eq!(detect(&facts(nsis(), EXE, None)), NOT_INSTALLED);
        assert_eq!(detect(&facts(nsis(), EXE, Some(""))), NOT_INSTALLED);
        assert_eq!(detect(&facts(nsis(), EXE, Some("\"\""))), NOT_INSTALLED);
        // Sous-dossier du dossier installé : ce n'est pas lui.
        let nested = r"C:\Users\Zoé\AppData\Local\chatonlm\copy\chatonlm.exe";
        assert_eq!(
            detect(&facts(nsis(), nested, Some(LOCATION))),
            NOT_INSTALLED
        );
        // Autre programme du dossier installé.
        let other = r"C:\Users\Zoé\AppData\Local\chatonlm\other.exe";
        assert_eq!(detect(&facts(nsis(), other, Some(LOCATION))), NOT_INSTALLED);
        // Sans marque (cargo build, essais), ou autre paquet : jamais.
        for bundle in [None, Some(BundleType::Msi), Some(BundleType::Deb)] {
            assert_eq!(
                detect(&facts(bundle.clone(), EXE, Some(LOCATION))),
                NOT_INSTALLED,
                "{bundle:?}"
            );
        }
        let unknown = Facts {
            exe: None,
            ..facts(nsis(), EXE, Some(LOCATION))
        };
        assert_eq!(detect(&unknown), NOT_INSTALLED);
    }

    #[test]
    fn the_installer_is_run_passive_as_an_update_and_restarts_the_app() {
        assert_eq!(INSTALLER_ARGS, ["/P", "/UPDATE", "/R"]);
        assert_eq!(
            uninstall_key("chatonlm"),
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall\chatonlm"
        );
        assert_eq!(installer_name("9.9.9"), "chatonlm-9.9.9-setup.exe");
        assert_eq!(
            installer_name(r"1.0.0\..\x y"),
            "chatonlm-1.0.0_.._x_y-setup.exe"
        );
        assert_eq!(
            split_program(r"C:\Apps\chatonlm.exe"),
            (r"C:\Apps", "chatonlm.exe")
        );
        assert_eq!(split_program("chatonlm.exe"), ("", "chatonlm.exe"));
    }

    #[test]
    fn a_launch_refused_by_windows_is_said_as_such() {
        for code in BLOCKED_ERRORS {
            assert_eq!(
                launch_error_text(Some(code), "Access is denied."),
                t!("updates.windows.blocked")
            );
        }
        // Autre erreur (fichier qui n'est pas un programme : 193) : dite.
        let text = launch_error_text(Some(193), "not a valid Win32 application");
        assert_eq!(
            text,
            t!(
                "updates.windows.launchFailed",
                error = "not a valid Win32 application"
            )
        );
        assert!(text.contains("not a valid Win32 application"), "{text}");
        assert_ne!(text, t!("updates.windows.blocked"));
        assert_eq!(
            launch_error_text(None, "x"),
            t!("updates.windows.launchFailed", error = "x")
        );
    }
}
