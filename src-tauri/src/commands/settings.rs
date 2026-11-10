use rusqlite::Connection;
use tauri::{AppHandle, Manager, State};

use crate::error::AppResult;
use crate::i18n::{self, Lang};
use crate::store::Store;
use crate::store::settings::{self, Settings, SettingsPatch};

/// Réglages enregistrés.
#[tauri::command]
pub async fn get_settings(store: State<'_, Store>) -> AppResult<Settings> {
    store.call(|c| settings::get(c)).await
}

#[tauri::command]
pub async fn update_settings(
    app: AppHandle,
    store: State<'_, Store>,
    patch: SettingsPatch,
) -> AppResult<Settings> {
    let proofread_shortcut = patch.proofread_shortcut;
    let read_aloud_shortcut = patch.read_aloud_shortcut;
    let web_search_on = patch.web_search_enabled == Some(true);
    let settings = store
        .call(move |connection| {
            let patch = renamed_patch(connection, patch)?;
            settings::update(connection, patch)
        })
        .await?;
    i18n::apply(&app, settings.effective_language);
    // Barre de titre du thème choisi (Windows ; rien sous Linux).
    crate::window::apply_theme(&app, settings.theme);
    app.state::<crate::tts::ReadAloud>()
        .set_auto_read(settings.auto_read);
    if let Some(enabled) = proofread_shortcut {
        crate::desktop::apply_proofread_shortcut(&app, enabled).await?;
    }
    if let Some(enabled) = read_aloud_shortcut {
        crate::desktop::apply_read_aloud_shortcut(&app, enabled);
    }
    // Recherche web activée : inscription au relais en tâche de fond.
    if web_search_on && let Some(access) = app.try_state::<crate::llm::Access>() {
        super::web_search::register_in_background(&access);
    }
    Ok(settings)
}

/// Langue de l'interface, lue par la page avant son premier affichage ;
/// répond aussi quand la base est illisible (langue du bureau).
#[tauri::command]
pub async fn ui_language() -> AppResult<Lang> {
    Ok(i18n::current())
}

/// Modèle nommé par la page : un ancien nom d'Ollama déjà repris (table de
/// la reprise, [`crate::store::model_import::renamed`]) est enregistré sous
/// son nouvel identifiant (la page peut relire un ancien nom : modèle d'une
/// conversation d'avant la 0.10, réglages lus avant la fin de la reprise) ;
/// tout autre nom, et un nom vide, restent tels quels.
pub(crate) fn current_model(connection: &Connection, name: String) -> AppResult<String> {
    if name.trim().is_empty() {
        return Ok(name);
    }
    Ok(crate::store::model_import::renamed(connection, &name)?.unwrap_or(name))
}

/// Les réglages du patch qui nomment un modèle (conversation, Correction,
/// Étude) passent par [`current_model`].
fn renamed_patch(connection: &Connection, mut patch: SettingsPatch) -> AppResult<SettingsPatch> {
    for field in [
        &mut patch.selected_model,
        &mut patch.proofread_model,
        &mut patch.study_model,
    ] {
        if let Some(name) = field.take() {
            *field = Some(current_model(connection, name)?);
        }
    }
    Ok(patch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_models::import::ImportOrigin;
    use crate::store::model_import::{self, ImportRecord, RecordStatus};

    fn imported(name: &str, id: &str) -> ImportRecord {
        ImportRecord {
            origin: ImportOrigin::Builtin,
            name: name.into(),
            model_id: Some(id.into()),
            status: RecordStatus::Imported,
            suggestion: None,
            detail: None,
            at: 1,
            attempts: 0,
        }
    }

    /// Après la reprise, un ancien nom d'Ollama écrit par la page (modèle
    /// d'une ancienne conversation…) est enregistré sous l'identifiant repris ;
    /// un nom jamais repris, un identifiant et un nom vide restent tels quels.
    #[test]
    fn an_old_ollama_name_already_imported_is_saved_under_its_new_id() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                model_import::record(c, &imported("qwen3:0.6b", "@ollama/qwen3:0.6b"))?;
                model_import::record(c, &imported("llama3.2", "@ollama/llama3.2:latest"))?;
                model_import::record(
                    c,
                    &imported(
                        "hf.co/unsloth/Qwen3-4B-GGUF:Q4_K_M",
                        "unsloth/Qwen3-4B-GGUF:Q4_K_M",
                    ),
                )?;
                let mut failed = imported("gemma3:4b", "@ollama/gemma3:4b");
                failed.status = RecordStatus::Failed;
                failed.model_id = None;
                model_import::record(c, &failed)?;
                let patch = renamed_patch(
                    c,
                    SettingsPatch {
                        selected_model: Some("qwen3:0.6b".into()),
                        proofread_model: Some("llama3.2:latest".into()),
                        study_model: Some("hf.co/unsloth/Qwen3-4B-GGUF:Q4_K_M".into()),
                        ..Default::default()
                    },
                )?;
                let saved = settings::update(c, patch)?;
                assert_eq!(saved.selected_model, "@ollama/qwen3:0.6b");
                assert_eq!(saved.proofread_model, "@ollama/llama3.2:latest");
                assert_eq!(saved.study_model, "unsloth/Qwen3-4B-GGUF:Q4_K_M");
                let patch = renamed_patch(
                    c,
                    SettingsPatch {
                        selected_model: Some("@ollama/qwen3:0.6b".into()),
                        proofread_model: Some(String::new()),
                        study_model: Some("gemma3:4b".into()),
                        ..Default::default()
                    },
                )?;
                assert_eq!(patch.selected_model.as_deref(), Some("@ollama/qwen3:0.6b"));
                assert_eq!(patch.proofread_model.as_deref(), Some(""));
                assert_eq!(patch.study_model.as_deref(), Some("gemma3:4b"));
                // Champs absents : absents.
                let patch = renamed_patch(c, SettingsPatch::default())?;
                assert_eq!(patch.selected_model, None);
                assert_eq!(current_model(c, "QWEN3:0.6B".into())?, "@ollama/qwen3:0.6b");
                Ok(())
            })
            .unwrap();
    }
}
