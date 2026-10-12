use tauri::{AppHandle, Manager, State};

use super::ollama_host_env;
use crate::error::AppResult;
use crate::i18n::{self, Lang};
use crate::ollama::target;
use crate::store::Store;
use crate::store::settings::{self, Settings, SettingsPatch};

/// Réglages ; `effectiveServerUrl` : adresse du serveur qui sert (Ollama
/// intégré compris).
#[tauri::command]
pub async fn get_settings(store: State<'_, Store>) -> AppResult<Settings> {
    target::display_settings(&store).await
}

#[tauri::command]
pub async fn update_settings(
    app: AppHandle,
    store: State<'_, Store>,
    patch: SettingsPatch,
) -> AppResult<Settings> {
    let host = ollama_host_env();
    let proofread_shortcut = patch.proofread_shortcut;
    let settings = store
        .call(move |connection| settings::update(connection, patch, host.as_deref()))
        .await?;
    i18n::apply(&app, settings.effective_language);
    // Barre de titre du thème choisi (Windows ; rien sous Linux).
    crate::window::apply_theme(&app, settings.theme);
    app.state::<crate::tts::ReadAloud>()
        .set_auto_read(settings.auto_read);
    if let Some(enabled) = proofread_shortcut {
        crate::desktop::apply_proofread_shortcut(&app, enabled).await?;
    }
    // Adresse changée : la cible aussi (téléchargements vers l'ancienne
    // fermés, événement `ollama-target`, intégré arrêté s'il ne sert plus ;
    // une vérification en cours ne note plus sa décision).
    target::settings_updated(&store).await
}

/// Langue de l'interface, lue par la page avant son premier affichage ;
/// répond aussi quand la base est illisible (langue du bureau).
#[tauri::command]
pub async fn ui_language() -> AppResult<Lang> {
    Ok(i18n::current())
}
