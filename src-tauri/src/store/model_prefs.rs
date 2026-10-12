//! Préférences mémorisées par modèle (table `model_prefs`) : le dernier réglage
//! du raisonnement (bouton Think), retrouvé en revenant au modèle. Écart à
//! l'officiel, qui repart du défaut du modèle à chaque fois. Aussi ce qu'on a
//! observé d'un modèle (table `model_traits`) : il raisonne même avec
//! `think: false`.

use rusqlite::{Connection, OptionalExtension, params};

use crate::chat::stream::Think;
use crate::error::{AppError, AppResult};
use crate::ollama::models::display_name;

/// Longueur maximale d'un nom de modèle, en octets (comme `selected_model`).
const MODEL_LEN: usize = 256;

/// Longueur maximale d'un niveau de raisonnement (« low », « medium »...), en
/// octets.
const LEVEL_LEN: usize = 64;

/// Réglage du raisonnement mémorisé pour ce modèle, s'il y en a un. Il n'est
/// pas confronté aux valeurs annoncées par Ollama : l'interface ignore celui
/// qui n'est plus permis.
pub fn think(connection: &Connection, model: &str) -> AppResult<Option<Think>> {
    let key = key(model)?;
    let stored: Option<String> = connection
        .query_row(
            "SELECT think FROM model_prefs WHERE model = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?;
    stored
        .map(|json| {
            serde_json::from_str(&json).map_err(|error| {
                AppError::Internal(format!("unreadable think preference for {key}: {error}"))
            })
        })
        .transpose()
}

/// Mémorise le réglage du raisonnement de ce modèle, à la place du précédent.
pub fn set_think(connection: &Connection, model: &str, think: &Think) -> AppResult<()> {
    let key = key(model)?;
    if let Think::Level(level) = think
        && (level.is_empty() || level.trim() != level || level.len() > LEVEL_LEN)
    {
        // i18n-ignore : niveaux lus dans /api/show par la page.
        return Err(AppError::Invalid(format!(
            "invalid thinking level: {level}"
        )));
    }
    let json =
        serde_json::to_string(think).map_err(|error| AppError::Internal(error.to_string()))?;
    connection.execute(
        "INSERT INTO model_prefs (model, think) VALUES (?1, ?2)
         ON CONFLICT (model) DO UPDATE SET think = excluded.think",
        params![key, json],
    )?;
    Ok(())
}

/// Vrai si ce modèle a raisonné alors que `think: false` était envoyé.
pub fn always_thinks(connection: &Connection, model: &str) -> AppResult<bool> {
    let key = key(model)?;
    let stored: Option<i64> = connection
        .query_row(
            "SELECT always_thinks FROM model_traits WHERE model = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?;
    Ok(stored == Some(1))
}

/// Retient que ce modèle raisonne même avec `think: false`.
pub fn set_always_thinks(connection: &Connection, model: &str) -> AppResult<()> {
    let key = key(model)?;
    connection.execute(
        "INSERT INTO model_traits (model, always_thinks) VALUES (?1, 1)
         ON CONFLICT (model) DO UPDATE SET always_thinks = 1",
        params![key],
    )?;
    Ok(())
}

/// Clé d'un modèle : son nom affiché. `llama3:latest` et `llama3` désignent le
/// même modèle et partagent donc leurs préférences.
fn key(model: &str) -> AppResult<String> {
    if model.is_empty() || model.trim() != model || model.len() > MODEL_LEN {
        // i18n-ignore : nom donné par la page, déjà valide.
        return Err(AppError::Invalid(format!("invalid model name: {model}")));
    }
    Ok(display_name(model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn a_model_that_always_thinks_is_remembered() {
        let store = Store::open_in_memory().unwrap();
        let get = |model: &str| store.blocking(|c| always_thinks(c, model)).unwrap();
        assert!(!get("hf.co/LiquidAI/LFM2.5-2.6B-GGUF:BF16"));
        store
            .blocking(|c| set_always_thinks(c, "hf.co/LiquidAI/LFM2.5-2.6B-GGUF:BF16"))
            .unwrap();
        // Une seconde fois : sans erreur.
        store
            .blocking(|c| set_always_thinks(c, "hf.co/LiquidAI/LFM2.5-2.6B-GGUF:BF16"))
            .unwrap();
        assert!(get("hf.co/LiquidAI/LFM2.5-2.6B-GGUF:BF16"));
        // Même modèle sous `:latest`, les autres non.
        store
            .blocking(|c| set_always_thinks(c, "lfm:latest"))
            .unwrap();
        assert!(get("lfm"));
        assert!(!get("qwen3:0.6b"));
        // Le réglage du raisonnement reste à part.
        assert_eq!(store.blocking(|c| think(c, "lfm")).unwrap(), None);
        assert!(store.blocking(|c| set_always_thinks(c, " ")).is_err());
    }

    fn level(name: &str) -> Think {
        Think::Level(name.into())
    }

    fn get(store: &Store, model: &str) -> AppResult<Option<Think>> {
        store.blocking(|c| think(c, model))
    }

    fn set(store: &Store, model: &str, value: &Think) -> AppResult<()> {
        store.blocking(|c| set_think(c, model, value))
    }

    #[test]
    fn nothing_is_remembered_at_first() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(get(&store, "qwen3:0.6b").unwrap(), None);
    }

    #[test]
    fn each_model_keeps_its_last_setting() {
        let store = Store::open_in_memory().unwrap();
        set(&store, "qwen3:0.6b", &Think::Enabled(false)).unwrap();
        set(&store, "gpt-oss:20b", &level("high")).unwrap();
        assert_eq!(
            get(&store, "qwen3:0.6b").unwrap(),
            Some(Think::Enabled(false))
        );
        assert_eq!(get(&store, "gpt-oss:20b").unwrap(), Some(level("high")));

        // Nouveau réglage : il remplace l'ancien, sans toucher aux autres.
        set(&store, "qwen3:0.6b", &Think::Enabled(true)).unwrap();
        set(&store, "gpt-oss:20b", &level("low")).unwrap();
        assert_eq!(
            get(&store, "qwen3:0.6b").unwrap(),
            Some(Think::Enabled(true))
        );
        assert_eq!(get(&store, "gpt-oss:20b").unwrap(), Some(level("low")));
        let rows: i64 = store
            .blocking(|c| Ok(c.query_row("SELECT COUNT(*) FROM model_prefs", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(rows, 2);
    }

    #[test]
    fn stored_as_the_json_of_the_value() {
        let store = Store::open_in_memory().unwrap();
        set(&store, "a", &Think::Enabled(true)).unwrap();
        set(&store, "b", &level("medium")).unwrap();
        let stored: Vec<String> = store
            .blocking(|c| {
                let mut statement = c.prepare("SELECT think FROM model_prefs ORDER BY model")?;
                let rows = statement.query_map([], |r| r.get(0))?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .unwrap();
        assert_eq!(stored, ["true", "\"medium\""]);
    }

    #[test]
    fn the_latest_tag_is_the_same_model() {
        let store = Store::open_in_memory().unwrap();
        set(&store, "llama3:latest", &Think::Enabled(false)).unwrap();
        assert_eq!(get(&store, "llama3").unwrap(), Some(Think::Enabled(false)));
        set(&store, "llama3", &Think::Enabled(true)).unwrap();
        assert_eq!(
            get(&store, "llama3:latest").unwrap(),
            Some(Think::Enabled(true))
        );
        // Les autres étiquettes restent des modèles distincts.
        assert_eq!(get(&store, "llama3:8b").unwrap(), None);
    }

    #[test]
    fn invalid_names_and_levels_are_refused_without_writing() {
        let store = Store::open_in_memory().unwrap();
        let long = "m".repeat(MODEL_LEN + 1);
        for model in ["", " qwen3", "qwen3 ", long.as_str()] {
            assert!(
                matches!(
                    set(&store, model, &Think::Enabled(true)),
                    Err(AppError::Invalid(_))
                ),
                "{model:?}"
            );
            assert!(
                matches!(get(&store, model), Err(AppError::Invalid(_))),
                "{model:?}"
            );
        }
        let long = "l".repeat(LEVEL_LEN + 1);
        for value in ["", " high", "high\n", long.as_str()] {
            assert!(
                matches!(
                    set(&store, "gpt-oss:20b", &level(value)),
                    Err(AppError::Invalid(_))
                ),
                "{value:?}"
            );
        }
        assert_eq!(get(&store, "gpt-oss:20b").unwrap(), None);
    }

    /// Seuls true, false et une chaîne entrent dans la table, même écrits
    /// sans passer par ce module.
    #[test]
    fn the_database_only_accepts_think_values() {
        let store = Store::open_in_memory().unwrap();
        for json in ["1", "null", "{}", "[true]", "high", ""] {
            let result = store.blocking(|c| {
                Ok(c.execute(
                    "INSERT INTO model_prefs (model, think) VALUES ('m', ?1)",
                    params![json],
                )?)
            });
            assert!(
                matches!(result, Err(AppError::Database(_))),
                "{json:?}: {result:?}"
            );
        }
        for json in ["true", "false", "\"low\""] {
            store
                .blocking(|c| {
                    Ok(c.execute(
                        "INSERT OR REPLACE INTO model_prefs (model, think) VALUES ('m', ?1)",
                        params![json],
                    )?)
                })
                .unwrap();
        }
        assert_eq!(get(&store, "m").unwrap(), Some(level("low")));
    }
}
