//! Réglages de la dictée (table `dictation_settings`, une seule ligne).

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dictation::languages;
use crate::error::{AppError, AppResult};

/// Longueur maximale des consignes du nettoyage (caractères).
pub const MAX_PROMPT_CHARS: usize = 4000;
/// Mots du dictionnaire personnalisé, au plus.
pub const MAX_DICTIONARY_WORDS: usize = 200;
/// Longueur maximale d'un mot (ou d'une expression) du dictionnaire.
pub const MAX_WORD_CHARS: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum DictationMode {
    /// Un appui commence, le suivant arrête.
    #[default]
    Toggle,
    /// Parler en maintenant le raccourci (push-to-talk).
    Hold,
}

impl DictationMode {
    fn as_str(self) -> &'static str {
        match self {
            DictationMode::Toggle => "toggle",
            DictationMode::Hold => "hold",
        }
    }

    fn parse(value: &str) -> AppResult<Self> {
        match value {
            "toggle" => Ok(DictationMode::Toggle),
            "hold" => Ok(DictationMode::Hold),
            other => Err(AppError::Internal(format!(
                "unknown dictation mode in database: {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DictationSettings {
    pub enabled: bool,
    pub mode: DictationMode,
    /// Identifiant du catalogue (`dictation::models`) ; vide = aucun.
    pub speech_model: String,
    /// Code de langue ou « auto ».
    pub language: String,
    /// Modèle Ollama du nettoyage ; vide = pas de nettoyage.
    pub polish_model: String,
    /// Consignes du nettoyage ; vide = consignes par défaut.
    pub polish_prompt: String,
    pub dictionary: Vec<String>,
    /// Texte dicté montré dans la notification.
    pub notify_text: bool,
    /// Texte copié dans le presse-papiers dès la fin de la dictée ; sinon,
    /// la notification propose « Copy ».
    pub auto_copy: bool,
}

/// Mise à jour partielle : seuls les champs présents changent.
#[derive(Clone, Debug, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct DictationPatch {
    #[ts(optional)]
    pub enabled: Option<bool>,
    #[ts(optional)]
    pub mode: Option<DictationMode>,
    #[ts(optional)]
    pub speech_model: Option<String>,
    #[ts(optional)]
    pub language: Option<String>,
    #[ts(optional)]
    pub polish_model: Option<String>,
    #[ts(optional)]
    pub polish_prompt: Option<String>,
    #[ts(optional)]
    pub dictionary: Option<Vec<String>>,
    #[ts(optional)]
    pub notify_text: Option<bool>,
    #[ts(optional)]
    pub auto_copy: Option<bool>,
}

pub fn get(connection: &Connection) -> AppResult<DictationSettings> {
    let row = connection
        .query_row(
            "SELECT enabled, mode, speech_model, language, polish_model, polish_prompt,
                    dictionary, notify_text, auto_copy
             FROM dictation_settings WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, bool>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, bool>(7)?,
                    row.get::<_, bool>(8)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::Internal("dictation settings row missing".into()))?;
    let (
        enabled,
        mode,
        speech_model,
        language,
        polish_model,
        polish_prompt,
        dictionary,
        notify,
        auto_copy,
    ) = row;
    // Un dictionnaire illisible (modifié à la main) ne bloque pas la dictée.
    let dictionary = serde_json::from_str(&dictionary).unwrap_or_default();
    Ok(DictationSettings {
        enabled,
        mode: DictationMode::parse(&mode)?,
        speech_model,
        language,
        polish_model,
        polish_prompt,
        dictionary,
        notify_text: notify,
        auto_copy,
    })
}

/// Mots du dictionnaire nettoyés : espaces retirés aux bords, vides et
/// doublons (sans tenir compte de la casse) écartés, ordre gardé.
fn clean_dictionary(words: &[String]) -> AppResult<Vec<String>> {
    let mut kept: Vec<String> = Vec::new();
    for word in words {
        let word = word.trim();
        if word.is_empty() {
            continue;
        }
        if word.chars().count() > MAX_WORD_CHARS || word.chars().any(char::is_control) {
            // i18n-ignore : la page limite et choisit déjà ces valeurs.
            return Err(AppError::Invalid(format!(
                "invalid dictionary word: {word}"
            )));
        }
        if !kept
            .iter()
            .any(|seen| seen.to_lowercase() == word.to_lowercase())
        {
            kept.push(word.to_owned());
        }
    }
    if kept.len() > MAX_DICTIONARY_WORDS {
        // i18n-ignore : la page limite et choisit déjà ces valeurs.
        return Err(AppError::Invalid(format!(
            "the dictionary holds at most {MAX_DICTIONARY_WORDS} words"
        )));
    }
    Ok(kept)
}

/// Valide puis applique `patch` en une transaction ; renvoie les réglages à
/// jour. L'existence du modèle de transcription n'est pas vérifiée ici (un
/// modèle supprimé ensuite est signalé à la dictée).
pub fn update(connection: &mut Connection, patch: DictationPatch) -> AppResult<DictationSettings> {
    if let Some(id) = &patch.speech_model
        && !id.is_empty()
        && crate::dictation::models::find(id).is_none()
    {
        // i18n-ignore : valeurs choisies dans les listes de la page.
        return Err(AppError::Invalid(format!("unknown speech model: {id}")));
    }
    if let Some(language) = &patch.language
        && !languages::is_supported(language)
    {
        // i18n-ignore : la page limite et choisit déjà ces valeurs.
        return Err(AppError::Invalid(format!(
            "unsupported language: {language}"
        )));
    }
    if let Some(model) = &patch.polish_model
        && (model.trim() != model || model.len() > 256)
    {
        // i18n-ignore
        return Err(AppError::Invalid(format!("invalid model name: {model}")));
    }
    if let Some(prompt) = &patch.polish_prompt
        && prompt.chars().count() > MAX_PROMPT_CHARS
    {
        // i18n-ignore : la page limite et choisit déjà ces valeurs.
        return Err(AppError::Invalid(format!(
            "the instructions hold at most {MAX_PROMPT_CHARS} characters"
        )));
    }
    let dictionary =
        match &patch.dictionary {
            Some(words) => Some(serde_json::to_string(&clean_dictionary(words)?).map_err(
                |error| AppError::Internal(format!("dictionary encoding failed: {error}")),
            )?),
            None => None,
        };

    let transaction = connection.transaction()?;
    let set = |column: &str, value: &dyn rusqlite::ToSql| -> AppResult<()> {
        transaction.execute(
            &format!("UPDATE dictation_settings SET {column} = ?1 WHERE id = 1"),
            params![value],
        )?;
        Ok(())
    };
    if let Some(enabled) = patch.enabled {
        set("enabled", &enabled)?;
    }
    if let Some(mode) = patch.mode {
        set("mode", &mode.as_str())?;
    }
    if let Some(id) = &patch.speech_model {
        set("speech_model", id)?;
    }
    if let Some(language) = &patch.language {
        set("language", language)?;
    }
    if let Some(model) = &patch.polish_model {
        set("polish_model", model)?;
    }
    if let Some(prompt) = &patch.polish_prompt {
        // Consignes identiques aux consignes par défaut ou vides : défaut.
        let prompt = prompt.trim();
        let stored = if prompt == crate::dictation::polish::DEFAULT_PROMPT.trim() {
            ""
        } else {
            prompt
        };
        set("polish_prompt", &stored)?;
    }
    if let Some(dictionary) = &dictionary {
        set("dictionary", dictionary)?;
    }
    if let Some(notify) = patch.notify_text {
        set("notify_text", &notify)?;
    }
    if let Some(auto_copy) = patch.auto_copy {
        set("auto_copy", &auto_copy)?;
    }
    transaction.commit()?;
    get(connection)
}

/// Vrai la première fois seulement : la notification « toujours lancée dans
/// la zone de notification » est alors à montrer.
pub fn take_tray_hint(connection: &Connection) -> AppResult<bool> {
    let changed = connection.execute(
        "UPDATE dictation_settings SET tray_hint_shown = 1 WHERE id = 1 AND tray_hint_shown = 0",
        [],
    )?;
    Ok(changed == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn defaults_then_partial_update() {
        let store = store();
        store
            .blocking(|connection| {
                let settings = get(connection)?;
                assert!(!settings.enabled);
                assert_eq!(settings.mode, DictationMode::Toggle);
                assert_eq!(settings.language, "auto");
                assert!(settings.notify_text && settings.dictionary.is_empty());
                assert!(settings.auto_copy);

                let updated = update(
                    connection,
                    DictationPatch {
                        mode: Some(DictationMode::Hold),
                        language: Some("fr".into()),
                        dictionary: Some(vec![
                            " ChatonLM ".into(),
                            "chatonlm".into(),
                            "".into(),
                            "Ollama".into(),
                        ]),
                        ..Default::default()
                    },
                )?;
                assert_eq!(updated.mode, DictationMode::Hold);
                assert_eq!(updated.language, "fr");
                assert_eq!(updated.dictionary, ["ChatonLM", "Ollama"]);
                assert!(!updated.enabled);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn invalid_values_change_nothing() {
        let store = store();
        store
            .blocking(|connection| {
                for patch in [
                    DictationPatch {
                        language: Some("xx".into()),
                        ..Default::default()
                    },
                    DictationPatch {
                        speech_model: Some("nope".into()),
                        ..Default::default()
                    },
                    DictationPatch {
                        polish_model: Some(" qwen3".into()),
                        ..Default::default()
                    },
                    DictationPatch {
                        polish_prompt: Some("x".repeat(MAX_PROMPT_CHARS + 1)),
                        ..Default::default()
                    },
                    DictationPatch {
                        dictionary: Some(vec!["a\nb".into()]),
                        ..Default::default()
                    },
                    DictationPatch {
                        dictionary: Some(
                            (0..=MAX_DICTIONARY_WORDS)
                                .map(|i| format!("w{i}"))
                                .collect(),
                        ),
                        ..Default::default()
                    },
                ] {
                    assert!(matches!(
                        update(connection, patch),
                        Err(AppError::Invalid(_))
                    ));
                }
                assert_eq!(get(connection)?.language, "auto");
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn default_prompt_is_stored_as_empty() {
        let store = store();
        store
            .blocking(|connection| {
                let prompt = crate::dictation::polish::DEFAULT_PROMPT.to_owned();
                let settings = update(
                    connection,
                    DictationPatch {
                        polish_prompt: Some(prompt),
                        ..Default::default()
                    },
                )?;
                assert_eq!(settings.polish_prompt, "");
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn auto_copy_can_be_turned_off() {
        let store = store();
        store
            .blocking(|connection| {
                let patch = DictationPatch {
                    auto_copy: Some(false),
                    ..Default::default()
                };
                assert!(!update(connection, patch)?.auto_copy);
                assert!(!get(connection)?.auto_copy);
                // Les autres réglages ne le touchent pas.
                let patch = DictationPatch {
                    notify_text: Some(false),
                    ..Default::default()
                };
                assert!(!update(connection, patch)?.auto_copy);
                Ok(())
            })
            .unwrap();
    }

    /// Base de l'étape 17 telle que commitée (schéma 6) : la dictée garde ses
    /// réglages et copie automatiquement, comme avant.
    #[test]
    fn a_version_6_database_keeps_copying_automatically() {
        use crate::store::MIGRATIONS;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let connection = Connection::open(&path).unwrap();
        for sql in &MIGRATIONS[..6] {
            connection.execute_batch(sql).unwrap();
        }
        connection.pragma_update(None, "user_version", 6).unwrap();
        connection
            .execute_batch("UPDATE dictation_settings SET enabled = 1, language = 'fr'")
            .unwrap();
        drop(connection);
        let store = Store::open(&path).unwrap();
        let settings = store.blocking(|connection| get(connection)).unwrap();
        assert!(settings.enabled && settings.auto_copy);
        assert_eq!(settings.language, "fr");
    }

    #[test]
    fn tray_hint_is_taken_once() {
        let store = store();
        store
            .blocking(|connection| {
                assert!(take_tray_hint(connection)?);
                assert!(!take_tray_hint(connection)?);
                Ok(())
            })
            .unwrap();
    }
}
