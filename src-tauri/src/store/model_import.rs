//! Reprise des modèles des Ollama de ChatonLM 0.9.x (migration 0040) : ce
//! qui a été repris (`ollama_imports`, aussi table de correspondance ancien
//! nom → identifiant), réglages repris par modèle (`model_defaults`), état
//! de la reprise automatique de l'Ollama intégré (`engine_settings`), et
//! réécriture des réglages qui nomment un modèle.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{AppError, AppResult};
use crate::local_models::import::names::ollama_key;
use crate::local_models::import::{ImportOrigin, OllamaParams};

/// Issue de la reprise d'un nom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordStatus {
    Imported,
    Redownload,
    Failed,
}

impl RecordStatus {
    fn as_str(self) -> &'static str {
        match self {
            RecordStatus::Imported => "imported",
            RecordStatus::Redownload => "redownload",
            RecordStatus::Failed => "failed",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "imported" => RecordStatus::Imported,
            "redownload" => RecordStatus::Redownload,
            "failed" => RecordStatus::Failed,
            _ => return None,
        })
    }
}

fn origin_str(origin: ImportOrigin) -> &'static str {
    match origin {
        ImportOrigin::Builtin => "builtin",
        ImportOrigin::Machine => "machine",
    }
}

fn parse_origin(text: &str) -> Option<ImportOrigin> {
    match text {
        "builtin" => Some(ImportOrigin::Builtin),
        "machine" => Some(ImportOrigin::Machine),
        _ => None,
    }
}

/// Ligne de `ollama_imports`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportRecord {
    pub origin: ImportOrigin,
    /// Nom tel qu'Ollama l'écrit.
    pub name: String,
    pub model_id: Option<String>,
    pub status: RecordStatus,
    pub suggestion: Option<String>,
    pub detail: Option<String>,
    pub at: i64,
    /// Échecs de suite (lu ; [`record`] le tient à jour d'après `status`).
    pub attempts: u32,
}

/// Échecs de suite après lesquels un modèle n'est plus repris
/// automatiquement au lancement (relecture L6, I8).
pub const GIVE_UP_AFTER: u32 = 2;

impl ImportRecord {
    /// Repris en vain [`GIVE_UP_AFTER`] fois : plus repris au lancement, et
    /// n'empêche plus le ménage (seul l'écran le retente).
    pub fn given_up(&self) -> bool {
        self.status == RecordStatus::Failed && self.attempts >= GIVE_UP_AFTER
    }
}

/// Enregistre (ou remplace) l'issue d'un nom ; un échec ajoute un essai
/// manqué, une autre issue les remet à zéro.
pub fn record(connection: &Connection, record: &ImportRecord) -> AppResult<()> {
    connection.execute(
        "INSERT INTO ollama_imports
             (origin, name, name_key, model_id, status, suggestion, detail, at, attempts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, CASE WHEN ?5 = 'failed' THEN 1 ELSE 0 END)
         ON CONFLICT (origin, name_key) DO UPDATE SET
             name = excluded.name, model_id = excluded.model_id,
             status = excluded.status, suggestion = excluded.suggestion,
             detail = excluded.detail, at = excluded.at,
             attempts = CASE WHEN excluded.status = 'failed'
                             THEN ollama_imports.attempts + 1 ELSE 0 END",
        params![
            origin_str(record.origin),
            record.name,
            ollama_key(&record.name),
            record.model_id,
            record.status.as_str(),
            record.suggestion,
            record.detail,
            record.at,
        ],
    )?;
    Ok(())
}

/// Toutes les issues, par origine puis par nom ; une ligne illisible est
/// ignorée.
pub fn records(connection: &Connection) -> AppResult<Vec<ImportRecord>> {
    let mut statement = connection.prepare(
        "SELECT origin, name, model_id, status, suggestion, detail, at, attempts
           FROM ollama_imports ORDER BY origin, name_key",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, i64>(7)?,
        ))
    })?;
    Ok(rows
        .flatten()
        .filter_map(
            |(origin, name, model_id, status, suggestion, detail, at, attempts)| {
                Some(ImportRecord {
                    origin: parse_origin(&origin)?,
                    name,
                    model_id,
                    status: RecordStatus::parse(&status)?,
                    suggestion,
                    detail,
                    at,
                    attempts: u32::try_from(attempts).unwrap_or(0),
                })
            },
        )
        .collect())
}

/// Correspondance clé d'un ancien nom → identifiant ; l'Ollama intégré
/// l'emporte sur celui de la machine.
fn renames(connection: &Connection) -> AppResult<HashMap<String, String>> {
    let mut map = HashMap::new();
    let mut records = records(connection)?;
    records.sort_by_key(|record| record.origin == ImportOrigin::Builtin);
    for record in records {
        if record.status == RecordStatus::Imported
            && let Some(id) = record.model_id
        {
            map.insert(ollama_key(&record.name), id);
        }
    }
    Ok(map)
}

/// Nouvel identifiant d'un ancien nom (sans la casse, `:latest` compris) :
/// pour un ancien nom venu d'ailleurs (réglage VS Code, lot L5).
pub fn renamed(connection: &Connection, name: &str) -> AppResult<Option<String>> {
    Ok(renames(connection)?.remove(&ollama_key(name)))
}

/// Clé de `model_prefs` et `model_traits` (`llm::ids::legacy_key`) : un
/// identifiant tel quel (`@ollama/qwen3:latest` garde `:latest`).
fn prefs_key(id: &str) -> String {
    crate::llm::ids::legacy_key(id)
}

/// Réécrit les réglages qui nomment un modèle repris :
/// `settings.selected_model`, `proofread_model`, `study_model`,
/// `dictation_settings.polish_model`, `easy_link_accounts.model`, clés de
/// `model_prefs` et `model_traits` (une préférence déjà enregistrée sous le
/// nouvel identifiant l'emporte). Jamais l'historique (`messages.model`,
/// `courses.model`). Rend le nombre de valeurs changées.
pub fn apply_renames(connection: &mut Connection) -> AppResult<u32> {
    let map = renames(connection)?;
    if map.is_empty() {
        return Ok(0);
    }
    let lookup = |value: &str| -> Option<String> {
        (!value.trim().is_empty())
            .then(|| map.get(&ollama_key(value)).cloned())
            .flatten()
    };
    let transaction = connection.transaction()?;
    let mut changed = 0u32;
    for (table, column) in [
        ("settings", "selected_model"),
        ("settings", "proofread_model"),
        ("settings", "study_model"),
        ("dictation_settings", "polish_model"),
        ("easy_link_accounts", "model"),
    ] {
        let rows: Vec<(i64, String)> = {
            let mut statement =
                transaction.prepare(&format!("SELECT rowid, {column} FROM {table}"))?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (rowid, value) in rows {
            if let Some(id) = lookup(&value) {
                transaction.execute(
                    &format!("UPDATE {table} SET {column} = ?1 WHERE rowid = ?2"),
                    params![id, rowid],
                )?;
                changed += 1;
            }
        }
    }
    for table in ["model_prefs", "model_traits"] {
        let keys: Vec<String> = {
            let mut statement = transaction.prepare(&format!("SELECT model FROM {table}"))?;
            statement
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for key in keys {
            let Some(id) = lookup(&key) else {
                continue;
            };
            let new = prefs_key(&id);
            let taken: bool = transaction
                .query_row(
                    &format!("SELECT 1 FROM {table} WHERE model = ?1"),
                    params![new],
                    |_| Ok(true),
                )
                .optional()?
                .unwrap_or(false);
            if taken {
                transaction.execute(
                    &format!("DELETE FROM {table} WHERE model = ?1"),
                    params![key],
                )?;
            } else {
                transaction.execute(
                    &format!("UPDATE {table} SET model = ?1 WHERE model = ?2"),
                    params![new, key],
                )?;
            }
            changed += 1;
        }
    }
    transaction.commit()?;
    Ok(changed)
}

/// Réglages repris d'Ollama pour un modèle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelDefaults {
    pub params: OllamaParams,
    /// Consigne système du modèle (vide : aucune).
    pub system: String,
}

/// Enregistre (ou remplace) les réglages repris du modèle `model`.
pub fn set_defaults(
    connection: &Connection,
    model: &str,
    defaults: &ModelDefaults,
) -> AppResult<()> {
    let params = serde_json::to_string(&defaults.params)
        .map_err(|error| AppError::Internal(error.to_string()))?;
    connection.execute(
        "INSERT INTO model_defaults (model, params, system) VALUES (?1, ?2, ?3)
         ON CONFLICT (model) DO UPDATE SET params = excluded.params, system = excluded.system",
        params![model, params, defaults.system],
    )?;
    Ok(())
}

/// Réglages repris du modèle `model` (identifiant exact), s'il y en a :
/// pour les requêtes au moteur (lot L5).
pub fn defaults(connection: &Connection, model: &str) -> AppResult<Option<ModelDefaults>> {
    let row: Option<(String, String)> = connection
        .query_row(
            "SELECT params, system FROM model_defaults WHERE model = ?1",
            params![model],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(row.map(|(params, system)| ModelDefaults {
        params: serde_json::from_str(&params).unwrap_or_default(),
        system,
    }))
}

/// Retire les réglages repris du modèle `model` (modèle supprimé).
pub fn remove_defaults(connection: &Connection, model: &str) -> AppResult<()> {
    connection.execute(
        "DELETE FROM model_defaults WHERE model = ?1",
        params![model],
    )?;
    Ok(())
}

/// La reprise automatique de l'Ollama intégré a abouti.
pub fn builtin_import_done(connection: &Connection) -> AppResult<bool> {
    let state: String = connection.query_row(
        "SELECT builtin_import FROM engine_settings WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(state == "done")
}

pub fn set_builtin_import_done(connection: &Connection, done: bool) -> AppResult<()> {
    connection.execute(
        "UPDATE engine_settings SET builtin_import = ?1 WHERE id = 1",
        params![if done { "done" } else { "pending" }],
    )?;
    Ok(())
}

/// Le dossier de l'Ollama intégré a été vu partagé avec l'Ollama de la
/// machine : copies seulement, aucun ménage (relecture L6, B1).
pub fn builtin_shared(connection: &Connection) -> AppResult<bool> {
    let shared: i64 = connection.query_row(
        "SELECT builtin_shared FROM engine_settings WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(shared != 0)
}

/// Note le dossier de l'intégré partagé (pour toujours).
pub fn set_builtin_shared(connection: &Connection) -> AppResult<()> {
    connection.execute(
        "UPDATE engine_settings SET builtin_shared = 1 WHERE id = 1",
        [],
    )?;
    Ok(())
}

/// Dossier déplacé de l'intégré (0.9.x, Settings › Model location) repris
/// comme dossier des modèles, à la première reprise d'un dossier propre à
/// l'intégré : seulement s'il est réglé (`dir`), que la base locale est vide
/// et qu'aucun dossier n'est choisi. Rend vrai s'il l'a été.
pub fn adopt_builtin_folder(connection: &Connection, dir: &str) -> AppResult<bool> {
    let changed = connection.execute(
        "UPDATE engine_settings SET models_dir = ?1
          WHERE id = 1
            AND (models_dir IS NULL OR models_dir = '')
            AND NOT EXISTS (SELECT 1 FROM local_models)
            AND (SELECT COALESCE(models_dir, '') FROM builtin_ollama WHERE id = 1) = ?1
            AND ?1 <> ''",
        params![dir],
    )?;
    Ok(changed > 0)
}

/// Dossier visé par un changement du dossier des modèles resté inachevé
/// (coupure pendant le déplacement).
pub fn models_dir_moving(connection: &Connection) -> AppResult<Option<String>> {
    let dir: Option<String> = connection.query_row(
        "SELECT models_dir_moving FROM engine_settings WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(dir.filter(|dir| !dir.trim().is_empty()))
}

/// Note le dossier visé avant le premier fichier déplacé (`None` : plus
/// rien en cours).
pub fn set_models_dir_moving(connection: &Connection, dir: Option<&str>) -> AppResult<()> {
    connection.execute(
        "UPDATE engine_settings SET models_dir_moving = ?1 WHERE id = 1",
        params![dir],
    )?;
    Ok(())
}

/// Fin d'un déplacement : nouveau dossier (`None` : par défaut) et journal
/// effacé, ensemble.
pub fn finish_models_dir_move(connection: &Connection, dir: Option<&str>) -> AppResult<()> {
    connection.execute(
        "UPDATE engine_settings SET models_dir = ?1, models_dir_moving = NULL WHERE id = 1",
        params![dir],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn imported(origin: ImportOrigin, name: &str, id: &str) -> ImportRecord {
        ImportRecord {
            origin,
            name: name.into(),
            model_id: Some(id.into()),
            status: RecordStatus::Imported,
            suggestion: None,
            detail: None,
            at: 1,
            attempts: 0,
        }
    }

    #[test]
    fn records_are_replaced_by_name_key() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                record(
                    c,
                    &imported(ImportOrigin::Builtin, "qwen3", "@ollama/qwen3:latest"),
                )?;
                // Même nom autrement écrit : remplacé.
                let mut again = imported(
                    ImportOrigin::Builtin,
                    "qwen3:latest",
                    "@ollama/qwen3:latest",
                );
                again.status = RecordStatus::Failed;
                again.model_id = None;
                again.detail = Some("disk".into());
                record(c, &again)?;
                again.attempts = 1;
                assert_eq!(records(c)?, vec![again.clone()]);
                assert!(!records(c)?[0].given_up());
                // Deuxième échec de suite : plus repris au lancement ; une
                // réussite remet le compte à zéro.
                record(c, &again)?;
                assert_eq!(records(c)?[0].attempts, 2);
                assert!(records(c)?[0].given_up());
                record(
                    c,
                    &imported(ImportOrigin::Builtin, "qwen3", "@ollama/qwen3:latest"),
                )?;
                assert_eq!(records(c)?[0].attempts, 0);
                record(c, &again)?;
                assert_eq!(records(c)?[0].attempts, 1);
                // Autre origine : une autre ligne.
                record(
                    c,
                    &imported(ImportOrigin::Machine, "qwen3", "@ollama/qwen3:latest"),
                )?;
                assert_eq!(records(c)?.len(), 2);
                assert_eq!(renamed(c, "QWEN3")?, Some("@ollama/qwen3:latest".into()));
                Ok(())
            })
            .unwrap();
    }

    /// Une base 0.9.5 avec des noms d'Ollama partout : après la reprise,
    /// les réglages nomment les identifiants ; l'historique garde l'ancien
    /// nom ; une préférence déjà là sous le nouvel identifiant l'emporte.
    #[test]
    fn settings_are_rewritten_but_not_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let connection = crate::store::database_at(&path, 38);
        connection
            .execute_batch(
                "UPDATE settings SET selected_model = 'qwen3:0.6b',
                     proofread_model = 'hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M',
                     study_model = '';
                 UPDATE dictation_settings SET polish_model = 'gemma3';
                 INSERT INTO model_prefs (model, think) VALUES ('qwen3:0.6b', 'true');
                 INSERT INTO model_prefs (model, think) VALUES ('gemma3', 'false');
                 INSERT INTO model_prefs (model, think) VALUES ('@ollama/gemma3:latest', 'true');
                 INSERT INTO model_traits (model, always_thinks) VALUES ('Qwen3:0.6B', 1);
                 INSERT INTO chats (id, title, created_at, updated_at)
                     VALUES ('c1', 't', 1, 1);
                 INSERT INTO messages (chat_id, role, content, model, created_at, updated_at)
                     VALUES ('c1', 'assistant', 'hi', 'qwen3:0.6b', 1, 1);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO easy_link_accounts (platform, user_id, name, model, linked_at)
                     VALUES ('telegram', '1', 'n', 'qwen3:0.6b', 1)",
                [],
            )
            .unwrap();
        drop(connection);
        let store = Store::open(&path).unwrap();
        store
            .blocking(|c| {
                // Rien à réécrire tant que rien n'est repris.
                assert_eq!(apply_renames(c)?, 0);
                for (name, id) in [
                    ("qwen3:0.6b", "@ollama/qwen3:0.6b"),
                    ("gemma3:latest", "@ollama/gemma3:latest"),
                    (
                        "hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M",
                        "bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M",
                    ),
                ] {
                    record(c, &imported(ImportOrigin::Builtin, name, id))?;
                }
                // Un nom non repris ne réécrit rien.
                let mut failed = imported(ImportOrigin::Builtin, "llama3.2:3b", "x");
                failed.status = RecordStatus::Failed;
                record(c, &failed)?;
                assert!(apply_renames(c)? > 0);
                let row: (String, String, String) = c.query_row(
                    "SELECT selected_model, proofread_model, study_model FROM settings",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?;
                assert_eq!(
                    row,
                    (
                        "@ollama/qwen3:0.6b".into(),
                        "bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M".into(),
                        String::new()
                    )
                );
                let polish: String =
                    c.query_row("SELECT polish_model FROM dictation_settings", [], |r| {
                        r.get(0)
                    })?;
                assert_eq!(polish, "@ollama/gemma3:latest");
                let account: String =
                    c.query_row("SELECT model FROM easy_link_accounts", [], |r| r.get(0))?;
                assert_eq!(account, "@ollama/qwen3:0.6b");
                let mut prefs: Vec<(String, String)> = c
                    .prepare("SELECT model, think FROM model_prefs ORDER BY model")?
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                prefs.sort();
                assert_eq!(
                    prefs,
                    vec![
                        ("@ollama/gemma3:latest".into(), "true".into()),
                        ("@ollama/qwen3:0.6b".into(), "true".into()),
                    ]
                );
                // Relue sous le nouvel identifiant, `:latest` compris (avant
                // `legacy_key`, la clé `@ollama/gemma3` ne se relisait plus).
                assert_eq!(
                    crate::store::model_prefs::think(c, "@ollama/gemma3:latest")?,
                    Some(crate::chat::stream::Think::Enabled(true))
                );
                let traits: String =
                    c.query_row("SELECT model FROM model_traits", [], |r| r.get(0))?;
                assert_eq!(traits, "@ollama/qwen3:0.6b");
                let history: String =
                    c.query_row("SELECT model FROM messages", [], |r| r.get(0))?;
                assert_eq!(history, "qwen3:0.6b");
                // Deuxième passage : rien ne change plus.
                assert_eq!(apply_renames(c)?, 0);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn defaults_and_import_state_are_kept() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                assert!(!builtin_import_done(c)?);
                set_builtin_import_done(c, true)?;
                assert!(builtin_import_done(c)?);
                assert_eq!(defaults(c, "@ollama/qwen3:0.6b")?, None);
                let saved = ModelDefaults {
                    params: OllamaParams {
                        temperature: Some(0.6),
                        top_k: Some(20),
                        stop: vec!["<|im_end|>".into()],
                        ..OllamaParams::default()
                    },
                    system: "Be brief.".into(),
                };
                set_defaults(c, "@ollama/qwen3:0.6b", &saved)?;
                assert_eq!(defaults(c, "@ollama/qwen3:0.6b")?, Some(saved));
                remove_defaults(c, "@ollama/qwen3:0.6b")?;
                assert_eq!(defaults(c, "@ollama/qwen3:0.6b")?, None);
                Ok(())
            })
            .unwrap();
    }

    /// Bases 0.9.4 et 0.9.5 dont l'Ollama intégré avait un dossier déplacé :
    /// la migration ne le reprend plus (relecture B1) ; la première reprise
    /// d'un dossier propre à l'intégré le fait ([`adopt_builtin_folder`]),
    /// seulement sans dossier choisi ni modèle déjà installé.
    #[test]
    fn the_moved_builtin_folder_is_adopted_only_by_code() {
        for version in [36, 38, 39] {
            for (builtin_dir, has_model, asked, adopted) in [
                (Some("/mnt/big/ollama"), false, "/mnt/big/ollama", true),
                // Un autre dossier que celui réglé : rien.
                (Some("/mnt/big/ollama"), false, "/mnt/other", false),
                (None, false, "", false),
                (Some("/mnt/big/ollama"), true, "/mnt/big/ollama", false),
            ] {
                if has_model && version < 39 {
                    continue;
                }
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("chatonlm.sqlite");
                let connection = crate::store::database_at(&path, version);
                if let Some(builtin_dir) = builtin_dir {
                    connection
                        .execute(
                            "UPDATE builtin_ollama SET models_dir = ?1",
                            params![builtin_dir],
                        )
                        .unwrap();
                }
                if has_model {
                    connection
                        .execute_batch(
                            "INSERT INTO local_models (id, source, repo, quant, files, installed_at)
                             VALUES ('u/A-GGUF:Q4', 'hf', 'u/A-GGUF', 'Q4', '[\"u/A-GGUF/a.gguf\"]', 1)",
                        )
                        .unwrap();
                }
                drop(connection);
                let store = Store::open(&path).unwrap();
                let models_dir = |store: &Store| {
                    store
                        .blocking(|c| crate::store::local_models::models_dir(c))
                        .unwrap()
                };
                // La migration seule ne change rien.
                assert_eq!(models_dir(&store), None, "{version} {builtin_dir:?}");
                assert!(!store.blocking(|c| builtin_import_done(c)).unwrap());
                assert!(!store.blocking(|c| builtin_shared(c)).unwrap());
                let done = store
                    .blocking(move |c| adopt_builtin_folder(c, asked))
                    .unwrap();
                assert_eq!(done, adopted, "{version} {builtin_dir:?} {asked}");
                assert_eq!(
                    models_dir(&store).as_deref(),
                    adopted.then_some(asked),
                    "{version} {builtin_dir:?}"
                );
                // Une seconde fois : déjà choisi, rien.
                assert!(
                    !store
                        .blocking(move |c| adopt_builtin_folder(c, asked))
                        .unwrap()
                );
            }
        }
    }

    #[test]
    fn the_shared_flag_and_the_move_journal_are_kept() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                set_builtin_shared(c)?;
                assert!(builtin_shared(c)?);
                assert_eq!(models_dir_moving(c)?, None);
                set_models_dir_moving(c, Some("/mnt/new"))?;
                assert_eq!(models_dir_moving(c)?.as_deref(), Some("/mnt/new"));
                finish_models_dir_move(c, Some("/mnt/new"))?;
                assert_eq!(models_dir_moving(c)?, None);
                assert_eq!(
                    crate::store::local_models::models_dir(c)?.as_deref(),
                    Some("/mnt/new")
                );
                Ok(())
            })
            .unwrap();
    }
}
