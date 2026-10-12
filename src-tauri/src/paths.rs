//! Emplacement des données de l'application.
//!
//! Par défaut, Tauri range les données et celles de WebKit dans
//! `~/.local/share/<identifiant>` (journaux dans `logs/`), la configuration
//! dans `~/.config/<identifiant>` et les caches dans `~/.cache/<identifiant>`.
//! ChatonLM n'écrit que dans le premier : passer par [`data_dir`] (et
//! `app_log_dir()` pour les journaux).
//!
//! Sous Windows, `app_data_dir()` est le profil itinérant (`%APPDATA%`), qu'un
//! domaine d'entreprise recopie d'un poste à l'autre : les modèles (des
//! centaines de Mo) n'y ont pas leur place. [`data_dir`] y donne donc
//! `%LOCALAPPDATA%\<identifiant>`, comme l'app officielle
//! (`%LOCALAPPDATA%\Ollama`), où WebView2 range aussi ses données.
//!
//! La variable `CHATONLM_DATA_DIR` (chemin absolu) redirige d'un coup tous ces
//! dossiers vers un autre : la configuration et les données à sa racine, les
//! caches dans `caches/`, les journaux dans `logs/`. Le développement s'en sert
//! pour ne rien écrire hors du projet.

use std::ffi::OsString;
use std::path::PathBuf;

use tauri::utils::config::AppDirectoriesOverride;
use tauri::{Context, Runtime};

/// Variable d'environnement qui redirige les dossiers de l'application.
pub const DATA_DIR_ENV: &str = "CHATONLM_DATA_DIR";

/// Applique la redirection demandée par `CHATONLM_DATA_DIR`, s'il y en a une.
///
/// Doit être appelé avant la construction de l'application : Tauri résout les
/// dossiers (dont celui de la webview) à partir de la configuration. Une valeur
/// invalide arrête l'app plutôt que de la laisser écrire ailleurs.
pub fn apply_data_dir_override<R: Runtime>(context: &mut Context<R>) {
    match data_dir_override(std::env::var_os(DATA_DIR_ENV)) {
        Ok(None) => {}
        Ok(Some(root)) => {
            context.config_mut().app.app_directories_override =
                Some(AppDirectoriesOverride::Root(root));
        }
        Err(message) => {
            eprintln!("chatonlm: {message}");
            std::process::exit(2);
        }
    }
}

/// Dossier des modèles de transcription de la dictée :
/// `data_dir()/speech-models/<identifiant>/<fichier>`.
pub fn speech_models_dir<R: Runtime>(
    app: &tauri::AppHandle<R>,
) -> crate::error::AppResult<PathBuf> {
    Ok(data_dir(app)?.join("speech-models"))
}

/// Dossier des voix de la lecture à voix haute :
/// `data_dir()/voice-models/<identifiant>/<fichier>`.
pub fn voice_models_dir<R: Runtime>(app: &tauri::AppHandle<R>) -> crate::error::AppResult<PathBuf> {
    Ok(data_dir(app)?.join("voice-models"))
}

/// Icône de l'app écrite sur le disque pour les notifications de la dictée.
pub fn notification_icon<R: Runtime>(
    app: &tauri::AppHandle<R>,
) -> crate::error::AppResult<PathBuf> {
    Ok(data_dir(app)?.join("icons").join("chatonlm.png"))
}

/// Dossier des données de ChatonLM (base, cache du ModelHub, modèles, Ollama
/// intégré) : `~/.local/share/<identifiant>` sous Linux,
/// `%LOCALAPPDATA%\<identifiant>` sous Windows ; la racine donnée par
/// `CHATONLM_DATA_DIR` sur les deux.
pub fn data_dir<R: Runtime>(app: &tauri::AppHandle<R>) -> crate::error::AppResult<PathBuf> {
    use tauri::Manager;
    let path = app.path();
    let dir = if cfg!(windows) {
        path.app_local_data_dir()
    } else {
        path.app_data_dir()
    };
    dir.map_err(|error| crate::error::AppError::Internal(format!("no data folder: {error}")))
}

/// Une valeur vide équivaut à l'absence de la variable. Tauri lirait une valeur
/// relative par rapport au dossier du binaire, et une valeur commençant par `$`
/// comme une de ses variables : seul un chemin absolu est accepté.
fn data_dir_override(value: Option<OsString>) -> Result<Option<PathBuf>, String> {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(Some(path))
    } else {
        Err(format!(
            "{DATA_DIR_ENV} must be an absolute path, got {}",
            path.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: &str) -> Result<Option<PathBuf>, String> {
        data_dir_override(Some(OsString::from(value)))
    }

    #[test]
    fn absent_or_empty_variable_means_no_override() {
        assert_eq!(data_dir_override(None), Ok(None));
        assert_eq!(parse(""), Ok(None));
    }

    #[test]
    fn absolute_path_is_used_as_is() {
        let absolute = if cfg!(windows) {
            r"C:\chatonlm data"
        } else {
            "/tmp/chatonlm data"
        };
        assert_eq!(parse(absolute), Ok(Some(PathBuf::from(absolute))));
    }

    #[test]
    fn other_paths_are_refused() {
        for value in ["data", "./data", "$DATA/chatonlm", "~/chatonlm"] {
            assert!(parse(value).is_err(), "{value} should be refused");
        }
        // Sous Windows, un chemin sans lecteur dépend du dossier courant.
        if cfg!(windows) {
            for value in [r"\chatonlm", "/tmp/chatonlm", "C:chatonlm"] {
                assert!(parse(value).is_err(), "{value} should be refused");
            }
        }
    }
}
