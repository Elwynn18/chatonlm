//! Réglages de l'Ollama intégré (table `builtin_ollama`, une seule ligne,
//! migration 0014) : à part de `settings`, que « Reset to defaults » remet à
//! zéro en partie.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppError, AppResult};

/// Serveur Ollama voulu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum OllamaSource {
    /// Celui de l'adresse réglée (ou de `OLLAMA_HOST`), sinon celui de la
    /// machine ; l'intégré seulement si rien n'est installé.
    #[default]
    Auto,
    /// Toujours l'intégré (choisi par l'utilisateur).
    Builtin,
}

impl OllamaSource {
    fn as_str(self) -> &'static str {
        match self {
            OllamaSource::Auto => "auto",
            OllamaSource::Builtin => "builtin",
        }
    }

    fn parse(value: &str) -> AppResult<Self> {
        match value {
            "auto" => Ok(OllamaSource::Auto),
            "builtin" => Ok(OllamaSource::Builtin),
            other => Err(AppError::Internal(format!(
                "unknown Ollama source in database: {other}"
            ))),
        }
    }
}

/// Ligne de la table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuiltinSettings {
    pub source: OllamaSource,
    /// Dossier des modèles choisi ; `None` : dossier par défaut.
    pub models_dir: Option<String>,
}

pub fn get(connection: &Connection) -> AppResult<BuiltinSettings> {
    let (source, models_dir) = connection.query_row(
        "SELECT source, models_dir FROM builtin_ollama WHERE id = 1",
        [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
    )?;
    Ok(BuiltinSettings {
        source: OllamaSource::parse(&source)?,
        models_dir: models_dir.filter(|dir| !dir.is_empty()),
    })
}

pub fn set_source(connection: &Connection, source: OllamaSource) -> AppResult<()> {
    connection.execute(
        "UPDATE builtin_ollama SET source = ?1 WHERE id = 1",
        params![source.as_str()],
    )?;
    Ok(())
}

/// `None` : revenir au dossier par défaut.
pub fn set_models_dir(connection: &Connection, dir: Option<&str>) -> AppResult<()> {
    connection.execute(
        "UPDATE builtin_ollama SET models_dir = ?1 WHERE id = 1",
        params![dir],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::store::settings::{self, SettingsPatch, Theme};

    #[test]
    fn automatic_source_and_default_folder_until_changed() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|connection| {
                assert_eq!(get(connection)?, BuiltinSettings::default());
                set_source(connection, OllamaSource::Builtin)?;
                set_models_dir(connection, Some("/data/models"))?;
                let changed = BuiltinSettings {
                    source: OllamaSource::Builtin,
                    models_dir: Some("/data/models".into()),
                };
                assert_eq!(get(connection)?, changed);
                // « Reset to defaults » n'y touche pas.
                let reset = SettingsPatch {
                    server_url: Some(String::new()),
                    context_length: Some(0),
                    theme: Some(Theme::System),
                    language: Some(crate::i18n::LanguageChoice::System),
                    ..SettingsPatch::default()
                };
                settings::update(connection, reset, None)?;
                assert_eq!(get(connection)?, changed);
                set_source(connection, OllamaSource::Auto)?;
                set_models_dir(connection, None)?;
                assert_eq!(get(connection)?, BuiltinSettings::default());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn the_table_holds_one_row_with_a_known_source() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|connection| {
                assert!(
                    connection
                        .execute("UPDATE builtin_ollama SET source = 'machine'", [])
                        .is_err()
                );
                assert!(
                    connection
                        .execute("INSERT INTO builtin_ollama (id) VALUES (2)", [])
                        .is_err()
                );
                Ok(())
            })
            .unwrap();
    }
}
