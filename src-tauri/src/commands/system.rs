//! Intégration au système : ouverture des liens dans le navigateur, fin de
//! l'application.

use std::ffi::OsString;
#[cfg(not(windows))]
use std::process::Stdio;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use tauri::AppHandle;
use url::Url;

use crate::error::{AppError, AppResult};
#[cfg(target_os = "linux")]
use crate::graphics;
use crate::i18n::t;
use crate::os_util::NoWindow;

/// Au-delà, xdg-open est considéré comme ayant lancé le navigateur (certains
/// navigateurs gardent xdg-open attaché jusqu'à leur fermeture).
#[cfg_attr(windows, allow(dead_code))]
const OPEN_WAIT: Duration = Duration::from_secs(2);

/// Variables posées par le lanceur de l'AppImage pour ChatonLM lui-même
/// (thème GTK forcé, greffons...), sans rapport avec les programmes de la
/// machine.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) const APPIMAGE_ONLY: &[&str] = &[
    "GTK_THEME",
    "GTK_DATA_PREFIX",
    "GDK_BACKEND",
    "GST_REGISTRY_REUSE_PLUGIN_SCANNER",
    "LD_PRELOAD",
];

/// Ouvre un lien `http` ou `https` avec le navigateur par défaut. Échoue si
/// xdg-open est absent ou signale une erreur dans les 2 premières secondes.
#[tauri::command]
pub async fn open_external(url: String) -> AppResult<()> {
    // Erreurs en anglais : la page ne donne que des liens http(s) valides
    // (webUrl, src/lib/openLink.ts).
    // i18n-ignore
    let parsed = Url::parse(&url).map_err(|_| AppError::Invalid(format!("invalid link: {url}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        // i18n-ignore
        return Err(AppError::Invalid(format!(
            "unsupported link scheme: {}",
            parsed.scheme()
        )));
    }

    open_target(std::ffi::OsStr::new(parsed.as_str()), || {
        t!("links.noBrowser")
    })
    .await
}

/// Ouvre un fichier ou un dossier de ChatonLM avec l'application par défaut
/// (dossier des modèles et journal de l'Ollama intégré) ; chemins choisis par
/// le moteur, jamais par la page.
pub(crate) async fn open_path(path: &std::path::Path) -> AppResult<()> {
    open_target(path.as_os_str(), || t!("links.noApplication")).await
}

/// Ce que `ShellExecuteW` a rendu (Windows).
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellOutcome {
    Opened,
    /// Aucune application n'est associée à ce type (`SE_ERR_NOASSOC`).
    NoApplication,
    /// Autre échec : code rendu (2 : fichier introuvable, 5 : accès refusé...).
    Failed(isize),
}

/// `ShellExecuteW` rend une valeur supérieure à 32 quand il réussit, un code
/// d'erreur sinon.
#[cfg_attr(not(windows), allow(dead_code))]
fn shell_outcome(code: isize) -> ShellOutcome {
    const SE_ERR_NOASSOC: isize = 31;
    match code {
        code if code > 32 => ShellOutcome::Opened,
        SE_ERR_NOASSOC => ShellOutcome::NoApplication,
        code => ShellOutcome::Failed(code),
    }
}

/// Windows : la cible (lien ou chemin) est ouverte par le shell, avec le
/// navigateur ou l'application par défaut (`ShellExecuteW`, verbe « open »).
/// Les textes de `no_application` nomment xdg-open : celui de Windows sert.
#[cfg(windows)]
async fn open_target(target: &std::ffi::OsStr, _no_application: fn() -> String) -> AppResult<()> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR, w};

    let target = HSTRING::from(target);
    let code = tauri::async_runtime::spawn_blocking(move || {
        // SAFETY : chaînes UTF-16 terminées, vivantes pendant l'appel ; pas
        // de fenêtre parente.
        let instance = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                &target,
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        instance.0 as isize
    })
    .await
    .map_err(|error| AppError::Internal(error.to_string()))?;
    match shell_outcome(code) {
        ShellOutcome::Opened => Ok(()),
        ShellOutcome::NoApplication => Err(AppError::System(t!("links.noAppWindows"))),
        ShellOutcome::Failed(code) => Err(AppError::System(t!("links.openFailed", code = code))),
    }
}

/// `xdg-open <cible>` ; `no_application` : message du code 3 (aucune
/// application pour ce type).
#[cfg(not(windows))]
async fn open_target(target: &std::ffi::OsStr, no_application: fn() -> String) -> AppResult<()> {
    let mut command = host_command("xdg-open");
    command
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = command.spawn().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => AppError::System(t!("links.noXdgOpen")),
        _ => AppError::Io(error),
    })?;
    let status = tauri::async_runtime::spawn_blocking(move || wait_briefly(child, OPEN_WAIT))
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?;
    match status.and_then(|status| status.code()) {
        None | Some(0) => Ok(()),
        Some(3) => Err(AppError::System(no_application())),
        Some(code) => Err(AppError::System(t!("links.failed", code = code))),
    }
}

/// Quitte l'application (Ctrl+Q dans la page ; « Quit ChatonLM » du menu de
/// la zone de notification fait de même). La fermeture de la fenêtre ne fait
/// que la masquer quand la zone de notification existe. La taille de la
/// fenêtre est mémorisée, puis `ExitRequested` fait le ménage (réponses,
/// téléchargements, dictée).
#[tauri::command]
pub async fn quit_app(app: AppHandle) -> AppResult<()> {
    crate::desktop::quit(&app);
    Ok(())
}

/// Code de sortie s'il arrive dans le délai ; sinon le processus est récupéré
/// plus tard, sans bloquer.
#[cfg_attr(windows, allow(dead_code))]
fn wait_briefly(mut child: Child, limit: Duration) -> Option<ExitStatus> {
    let start = Instant::now();
    while start.elapsed() < limit {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break,
        }
    }
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    None
}

/// Commande pour un programme de la machine : sans ce que ChatonLM a changé
/// dans son propre environnement (contournement NVIDIA, variables de
/// l'AppImage), pour que le programme se comporte comme lancé du bureau.
/// Sous Windows, ChatonLM ne change rien à son environnement : le programme
/// est seulement lancé sans fenêtre de console.
#[cfg_attr(windows, allow(dead_code))]
pub fn host_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.no_window();
    #[cfg(target_os = "linux")]
    {
        if graphics::nvidia_workaround_applied() {
            command.env_remove(graphics::NV_EXPLICIT_SYNC_ENV);
        }
        if let Some(appdir) = std::env::var_os("APPDIR") {
            for (key, value) in host_environment(std::env::vars_os(), &appdir) {
                match value {
                    Some(value) => command.env(key, value),
                    None => command.env_remove(key),
                };
            }
            for key in APPIMAGE_ONLY {
                command.env_remove(key);
            }
        }
    }
    command
}

/// Sous AppImage, AppRun ajoute ses propres dossiers à de nombreuses variables
/// (LD_LIBRARY_PATH, XDG_DATA_DIRS, GST_*, PYTHONHOME...). Un programme de
/// l'hôte lancé avec ces valeurs chargerait les bibliothèques de l'AppImage.
/// Renvoie, pour chaque variable concernée, la valeur nettoyée ou `None` pour
/// la supprimer.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn host_environment(
    vars: impl Iterator<Item = (OsString, OsString)>,
    appdir: &OsString,
) -> Vec<(OsString, Option<OsString>)> {
    let appdir = appdir.to_string_lossy().into_owned();
    if appdir.is_empty() {
        return Vec::new();
    }
    vars.filter_map(|(key, value)| {
        let text = value.to_string_lossy();
        if !text.contains(&appdir) {
            return None;
        }
        let kept: Vec<&str> = text
            .split(':')
            .filter(|entry| !entry.is_empty() && !entry.starts_with(&appdir))
            .collect();
        let cleaned = (!kept.is_empty()).then(|| OsString::from(kept.join(":")));
        Some((key, cleaned))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_execute_results_above_32_are_successes() {
        assert_eq!(shell_outcome(33), ShellOutcome::Opened);
        assert_eq!(shell_outcome(42), ShellOutcome::Opened);
        assert_eq!(shell_outcome(31), ShellOutcome::NoApplication);
        assert_eq!(shell_outcome(32), ShellOutcome::Failed(32));
        assert_eq!(shell_outcome(2), ShellOutcome::Failed(2));
        assert_eq!(shell_outcome(0), ShellOutcome::Failed(0));
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect()
    }

    #[test]
    fn appimage_entries_are_removed() {
        let appdir = OsString::from("/tmp/.mount_ChatonLM");
        let cleaned = host_environment(
            env(&[
                ("LD_LIBRARY_PATH", "/tmp/.mount_ChatonLM/usr/lib"),
                (
                    "XDG_DATA_DIRS",
                    "/tmp/.mount_ChatonLM/usr/share:/usr/local/share:/usr/share",
                ),
                ("PYTHONHOME", "/tmp/.mount_ChatonLM/usr"),
                ("HOME", "/home/user"),
            ])
            .into_iter(),
            &appdir,
        );
        assert_eq!(
            cleaned,
            vec![
                (OsString::from("LD_LIBRARY_PATH"), None),
                (
                    OsString::from("XDG_DATA_DIRS"),
                    Some(OsString::from("/usr/local/share:/usr/share"))
                ),
                (OsString::from("PYTHONHOME"), None),
            ]
        );
    }

    #[tokio::test]
    async fn only_web_links_are_opened() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "mailto:a@example.com?attach=/etc/passwd",
            "not a url",
        ] {
            assert!(
                matches!(open_external(url.into()).await, Err(AppError::Invalid(_))),
                "{url}"
            );
        }
    }

    /// Programmes d'Unix (`sh`, `sleep`) : Linux seulement.
    #[cfg(target_os = "linux")]
    #[test]
    fn exit_codes_within_the_delay_are_reported() {
        let child = Command::new("sh").args(["-c", "exit 3"]).spawn().unwrap();
        let status = wait_briefly(child, Duration::from_secs(2));
        assert_eq!(status.and_then(|s| s.code()), Some(3));
        let slow = Command::new("sleep").arg("5").spawn().unwrap();
        let started = Instant::now();
        assert_eq!(wait_briefly(slow, Duration::from_millis(200)), None);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
