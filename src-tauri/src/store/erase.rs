//! « Erase my data » (Settings › General, propre à ChatonLM) : la part de la
//! base. Tout ce que l'utilisateur a créé ou réglé est effacé d'un seul coup,
//! dans une transaction (tout ou rien) ; ce qui décrit un modèle ou a été
//! téléchargé reste.
//!
//! Classement des tables (essai `every_table_is_classified` : une table
//! ajoutée par une migration doit y entrer) :
//! - effacées (`ERASED_TABLES`) : conversations (éphémères comprises) et
//!   tout ce qui en dépend, cours de l'Étude, transcriptions, comptes liés
//!   d'Easy Link, éditeurs appairés du lien VS Code, fournisseurs distants,
//!   préférences par modèle (bouton Think) ;
//! - remises aux valeurs par défaut (`RESET_TABLES`, une seule ligne) :
//!   réglages de l'app (sauf la langue, la visite guidée faite et les modèles
//!   choisis), de la dictée (dictionnaire compris, sauf ses modèles choisis),
//!   du mode vocal, du bureau (sauf l'entrée du
//!   menu des applications, choix d'installation), des mises à jour, d'Easy
//!   Link (bots oubliés, jetons compris), du lien VS Code (coupé) et du moteur
//!   de recherche personnel (clés comprises) ;
//! - gardées (`KEPT_TABLES`) : modèles et ce qui les décrit (avec eux, les
//!   réglages qui ne font que désigner un modèle installé : `MODEL_CHOICES`), réglages du
//!   moteur (dossier des modèles, pack CUDA), reprise des modèles d'Ollama ;
//!   les modèles d'images restent, leurs réglages avancés sont oubliés ; les
//!   identifiants des fournisseurs distants retirés y sont ajoutés (jamais
//!   redonnés, comme pour une suppression à la main).
//!
//! `secure_delete` (voir `configure`) écrase de zéros le contenu effacé ; le
//! journal WAL est ensuite reporté dans la base et vidé.

use rusqlite::Connection;

use crate::error::AppResult;

/// Tables vidées.
#[cfg(test)]
const ERASED_TABLES: &[&str] = &[
    "chats",
    "messages",
    "attachments",
    "tool_calls",
    "compactions",
    "courses",
    "course_sources",
    "course_levels",
    "course_answers",
    "transcriptions",
    "easy_link_accounts",
    "bridge_clients",
    "remote_providers",
    "model_prefs",
];

/// Tables d'une seule ligne (ou de lignes fixes) remises aux valeurs par
/// défaut.
#[cfg(test)]
const RESET_TABLES: &[&str] = &[
    "settings",
    "dictation_settings",
    "voice_mode_settings",
    "desktop_settings",
    "update_settings",
    "easy_link_bots",
    "easy_link_settings",
    "bridge_settings",
    "web_search_engine",
];

/// Tables gardées (modèles, moteur) ; `sqlite_sequence` est celle de SQLite
/// (compteurs des clés AUTOINCREMENT).
#[cfg(test)]
const KEPT_TABLES: &[&str] = &[
    "local_models",
    "model_traits",
    "model_defaults",
    "image_models",
    "engine_settings",
    "ollama_imports",
    "builtin_ollama",
    "remote_retired_ids",
    "sqlite_sequence",
];

/// Réglages gardés parce qu'ils ne font que désigner un modèle (gardé
/// lui aussi) : `table.colonne`. Le modèle de l'autocomplétion de VS Code
/// est réglé dans l'extension, pas dans cette base.
#[cfg(test)]
const MODEL_CHOICES: &[&str] = &[
    "settings.selected_model",
    "settings.proofread_model",
    "settings.study_model",
    "settings.image_model",
    "dictation_settings.speech_model",
    "dictation_settings.polish_model",
];

/// Effacement, dans l'ordre : enfants avant parents (les clés étrangères
/// effaceraient de toute façon en cascade), puis les lignes uniques
/// remplacées par une ligne neuve (valeurs par défaut des colonnes), en
/// reprenant ce qui est gardé.
const ERASE: &str = "
    DELETE FROM tool_calls;
    DELETE FROM attachments;
    DELETE FROM compactions;
    DELETE FROM messages;
    DELETE FROM easy_link_accounts;
    DELETE FROM chats;
    DELETE FROM course_answers;
    DELETE FROM course_levels;
    DELETE FROM course_sources;
    DELETE FROM courses;
    DELETE FROM transcriptions;
    DELETE FROM bridge_clients;
    INSERT OR IGNORE INTO remote_retired_ids (id) SELECT id FROM remote_providers;
    DELETE FROM remote_providers;
    DELETE FROM model_prefs;
    UPDATE image_models SET settings = NULL;

    INSERT OR REPLACE INTO settings
        (id, language, onboarding_done, selected_model, proofread_model, study_model, image_model)
        SELECT id, language, onboarding_done, selected_model, proofread_model, study_model,
               image_model
        FROM settings WHERE id = 1;
    INSERT OR REPLACE INTO desktop_settings (id, menu_entry)
        SELECT id, menu_entry FROM desktop_settings WHERE id = 1;
    INSERT OR REPLACE INTO dictation_settings (id, speech_model, polish_model)
        SELECT id, speech_model, polish_model FROM dictation_settings WHERE id = 1;
    DELETE FROM voice_mode_settings;
    INSERT INTO voice_mode_settings (id) VALUES (1);
    DELETE FROM update_settings;
    INSERT INTO update_settings (id) VALUES (1);
    DELETE FROM easy_link_bots;
    INSERT INTO easy_link_bots (platform) VALUES ('telegram'), ('discord');
    DELETE FROM easy_link_settings;
    INSERT INTO easy_link_settings (id) VALUES (1);
    DELETE FROM bridge_settings;
    INSERT INTO bridge_settings (id) VALUES (1);
    DELETE FROM web_search_engine;
    INSERT INTO web_search_engine (id) VALUES (1);
";

/// Efface les données de l'utilisateur en une transaction : une erreur n'en
/// laisse rien. Le journal WAL est vidé ensuite (sans erreur possible : voir
/// `chats::truncate_journal`).
pub fn erase_user_data(connection: &mut Connection) -> AppResult<()> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(ERASE)?;
    transaction.commit()?;
    super::chats::truncate_journal(connection);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::store::settings::{self, SettingsPatch, Theme};
    use crate::store::{dictation, voice_mode};
    use rusqlite::params;

    fn count(connection: &Connection, table: &str) -> i64 {
        connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    /// Base remplie comme par un utilisateur : une conversation avec pièce
    /// jointe, appel d'outil et compactage, une éphémère, un cours, une
    /// transcription, un bot Easy Link et son compte lié, un éditeur VS Code
    /// appairé, un fournisseur distant, un moteur de recherche personnel,
    /// des réglages partout ; et ce qui doit rester : un modèle local, un
    /// modèle d'images avec ses réglages, un comportement observé, la reprise
    /// d'un modèle d'Ollama, le dossier des modèles.
    fn fill(connection: &mut Connection) {
        settings::update(
            connection,
            SettingsPatch {
                language: Some(crate::i18n::LanguageChoice::parse("fr").unwrap()),
                onboarding_done: Some(true),
                theme: Some(Theme::Dark),
                context_length: Some(8192),
                selected_model: Some("a/b:Q4_K_M".into()),
                proofread_model: Some("c/d:Q8_0".into()),
                study_model: Some("e/f:Q4_0".into()),
                system_prompt: Some("Réponds en vers.".into()),
                auto_read: Some(true),
                proofread_shortcut: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        dictation::update(
            connection,
            dictation::DictationPatch {
                enabled: Some(true),
                speech_model: Some("whisper-base".into()),
                polish_model: Some("a/b:Q4_K_M".into()),
                dictionary: Some(vec!["Chaton".into()]),
                polish_prompt: Some("Mes consignes".into()),
                ..Default::default()
            },
        )
        .unwrap();
        connection
            .execute_batch(
                "INSERT INTO chats (id, title, created_at, updated_at) VALUES ('c1', 'Secret', 1, 1);
                 INSERT INTO chats (id, created_at, updated_at, ephemeral) VALUES ('c2', 1, 1, 1);
                 INSERT INTO messages (chat_id, role, content, created_at, updated_at)
                     VALUES ('c1', 'user', 'mon secret', 1, 1);
                 INSERT INTO attachments (message_id, filename, kind, size, data)
                     VALUES (last_insert_rowid(), 'secret.txt', 'text', 6, X'736563726574');
                 INSERT INTO tool_calls (message_id, position, name, arguments)
                     SELECT id, 0, 'web_search', '{}' FROM messages;
                 INSERT INTO compactions (chat_id, through_message_id, summary, automatic, created_at)
                     VALUES ('c1', 1, 'résumé', 0, 1);
                 INSERT INTO courses (id, topic, level, minutes, language, status, created_at, updated_at)
                     VALUES ('k1', 'Rust', 'beginner', 10, 'fr', 'active', 1, 1);
                 INSERT INTO course_sources (course_id, kind, title, text) VALUES ('k1', 'document', 'doc', 'texte');
                 INSERT INTO course_levels (course_id, idx, data) VALUES ('k1', 0, '{}');
                 INSERT INTO course_answers (course_id, question_id, data) VALUES ('k1', 'q', '{}');
                 INSERT INTO transcriptions (id, source, origin, engine, model, status, created_at, updated_at)
                     VALUES ('t1', 'file', '/home/u/réunion.mp3', 'whisper', 'whisper-base', 'done', 1, 1);
                 UPDATE easy_link_bots SET token = 'jeton', enabled = 1, bot_id = '7', bot_name = 'Bot'
                     WHERE platform = 'telegram';
                 INSERT INTO easy_link_accounts (platform, user_id, name, chat_id, linked_at)
                     VALUES ('telegram', '42', 'Alice', 'c1', 1);
                 UPDATE easy_link_settings SET vscode_notify = 1;
                 UPDATE bridge_settings SET enabled = 1;
                 INSERT INTO remote_providers (id, kind, name, enabled, created_at, updated_at)
                     VALUES ('mistral', 'mistral', 'Mistral', 1, 1, 1);
                 UPDATE web_search_engine SET engine = 'brave', brave_key = 'clé';
                 INSERT INTO model_prefs (model, think) VALUES ('a/b:Q4_K_M', 'true');
                 UPDATE voice_mode_settings SET instructions = 'Parle doucement', barge_in = 0;
                 UPDATE desktop_settings SET menu_entry = 0, close_to_tray = 0, dictation_keys = 'Alt+F9';
                 UPDATE update_settings SET auto_download = 0, notified_version = '9.9.9';
                 INSERT INTO local_models (id, source, repo, quant, files, installed_at)
                     VALUES ('a/b:Q4_K_M', 'hf', 'a/b', 'Q4_K_M', '[\"b.gguf\"]', 1);
                 INSERT INTO model_traits (model, always_thinks) VALUES ('a/b:Q4_K_M', 1);
                 INSERT INTO model_defaults (model, system) VALUES ('a/b:Q4_K_M', 'système');
                 INSERT INTO ollama_imports (origin, name, name_key, model_id, status, at)
                     VALUES ('builtin', 'qwen3', 'qwen3:latest', 'a/b:Q4_K_M', 'imported', 1);
                 UPDATE engine_settings SET models_dir = '/mnt/modèles';
                 INSERT INTO image_models (id, repo, revision, file, family, name, installed, components, created_at, settings)
                     VALUES ('i/j/m.gguf', 'i/j', 'r', 'm.gguf', 'sd15', 'M', 1, '[]', 1, '{\"steps\":30}');
                 UPDATE settings SET image_model = 'i/j/m.gguf';",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO bridge_clients (id, name, secret, paired_at) VALUES ('e1', 'VS Code', ?1, 1)",
                params![vec![7u8; 32]],
            )
            .unwrap();
    }

    /// Après l'effacement : plus aucune donnée, réglages par défaut, langue
    /// et visite guidée gardées, modèles intacts (réglages d'images oubliés).
    #[test]
    fn user_data_is_erased_and_models_are_kept() {
        let store = Store::open_in_memory().unwrap();
        let defaults = store
            .blocking(|c| Ok((settings::get(c)?, dictation::get(c)?, voice_mode::get(c)?)))
            .unwrap();
        store
            .blocking(|c| {
                fill(c);
                erase_user_data(c)?;
                for table in ERASED_TABLES {
                    assert_eq!(count(c, table), 0, "{table}");
                }

                let after = settings::get(c)?;
                assert_eq!(after.language.as_str(), "fr");
                assert!(after.onboarding_done);
                let mut expected = defaults.0.clone();
                expected.language = after.language;
                expected.effective_language = after.effective_language;
                expected.onboarding_done = true;
                // Modèles choisis gardés (les modèles restent).
                expected.selected_model = "a/b:Q4_K_M".into();
                expected.proofread_model = "c/d:Q8_0".into();
                expected.study_model = "e/f:Q4_0".into();
                assert_eq!(after, expected);
                let image_model: String =
                    c.query_row("SELECT image_model FROM settings", [], |row| row.get(0))?;
                assert_eq!(image_model, "i/j/m.gguf");
                // Dictée coupée, dictionnaire et consignes oubliés, modèles
                // gardés.
                let mut dictation_expected = defaults.1.clone();
                dictation_expected.speech_model = "whisper-base".into();
                dictation_expected.polish_model = "a/b:Q4_K_M".into();
                assert_eq!(dictation::get(c)?, dictation_expected);
                assert_eq!(voice_mode::get(c)?, defaults.2);

                // Bots oubliés (jetons compris), lien VS Code coupé, moteur
                // de recherche par défaut, clés oubliées.
                let bots: Vec<(String, String, bool, String, String)> = c
                    .prepare(
                        "SELECT platform, token, enabled, bot_id, bot_name FROM easy_link_bots
                         ORDER BY platform",
                    )?
                    .query_map([], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
                    })?
                    .collect::<Result<_, _>>()?;
                assert_eq!(
                    bots,
                    [
                        ("discord".into(), String::new(), false, String::new(), String::new()),
                        ("telegram".into(), String::new(), false, String::new(), String::new()),
                    ]
                );
                let one = |sql: &str| -> String {
                    c.query_row(sql, [], |row| row.get::<_, rusqlite::types::Value>(0))
                        .map(|value| format!("{value:?}"))
                        .unwrap()
                };
                assert_eq!(one("SELECT vscode_notify FROM easy_link_settings"), "Integer(0)");
                assert_eq!(one("SELECT enabled FROM bridge_settings"), "Integer(0)");
                assert_eq!(
                    one("SELECT engine || brave_key || mojeek_key || searxng_url FROM web_search_engine"),
                    "Text(\"duckDuckGo\")"
                );
                // Bureau : croix et combinaisons par défaut, entrée du menu
                // (choix d'installation) gardée.
                assert_eq!(
                    one("SELECT close_to_tray || ',' || menu_entry || ',' || ifnull(dictation_keys, '-') FROM desktop_settings"),
                    "Text(\"1,0,-\")"
                );
                assert_eq!(
                    one("SELECT auto_download || ifnull(notified_version, '-') FROM update_settings"),
                    "Text(\"1-\")"
                );
                // Fournisseur retiré : son identifiant n'est jamais redonné.
                assert_eq!(one("SELECT id FROM remote_retired_ids"), "Text(\"mistral\")");

                // Gardés.
                for table in ["local_models", "model_traits", "model_defaults", "ollama_imports", "image_models"] {
                    assert_eq!(count(c, table), 1, "{table}");
                }
                assert_eq!(one("SELECT models_dir FROM engine_settings"), "Text(\"/mnt/modèles\")");
                assert_eq!(
                    one("SELECT installed || ifnull(settings, '-') FROM image_models"),
                    "Text(\"1-\")"
                );
                Ok(())
            })
            .unwrap();
    }

    /// Tout ou rien : une erreur au milieu (ici, une table manquante après
    /// les premières suppressions) ne laisse rien d'effacé.
    #[test]
    fn a_failure_erases_nothing() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                fill(c);
                c.execute_batch("DROP TABLE web_search_engine")?;
                assert!(erase_user_data(c).is_err());
                assert_eq!(count(c, "chats"), 2);
                assert_eq!(count(c, "messages"), 1);
                assert_eq!(count(c, "courses"), 1);
                assert_eq!(settings::get(c)?.system_prompt, "Réponds en vers.");
                assert_eq!(dictation::get(c)?.dictionary, ["Chaton"]);
                Ok(())
            })
            .unwrap();
    }

    /// Toute colonne d'une table remise à zéro qui désigne un modèle
    /// (`…model` en texte) est un choix gardé : une nouvelle colonne de ce
    /// genre doit y entrer (ou être écartée ici, motif à l'appui).
    #[test]
    fn every_model_choice_of_a_reset_table_is_kept() {
        let store = Store::open_in_memory().unwrap();
        let mut found: Vec<String> = store
            .blocking(|c| {
                let mut found = Vec::new();
                for table in RESET_TABLES {
                    let mut statement = c.prepare(&format!("PRAGMA table_info({table})"))?;
                    let columns = statement.query_map([], |row| {
                        Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
                    })?;
                    for column in columns {
                        let (name, kind) = column?;
                        if name.ends_with("model") && kind == "TEXT" {
                            found.push(format!("{table}.{name}"));
                        }
                    }
                }
                Ok(found)
            })
            .unwrap();
        found.sort();
        let mut kept: Vec<String> = MODEL_CHOICES.iter().map(|c| (*c).to_owned()).collect();
        kept.sort();
        assert_eq!(found, kept);
    }

    /// Chaque table de la base est classée : effacée, remise à zéro ou
    /// gardée. Une nouvelle table (migration) fait échouer cet essai tant
    /// qu'elle n'y est pas.
    #[test]
    fn every_table_is_classified() {
        let store = Store::open_in_memory().unwrap();
        let mut tables: Vec<String> = store
            .blocking(|c| {
                Ok(
                    c.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?
                        .query_map([], |row| row.get(0))?
                        .collect::<Result<_, _>>()?,
                )
            })
            .unwrap();
        tables.sort();
        let mut classified: Vec<String> = ERASED_TABLES
            .iter()
            .chain(RESET_TABLES)
            .chain(KEPT_TABLES)
            .map(|name| (*name).to_owned())
            .collect();
        classified.sort();
        assert_eq!(tables, classified);
    }
}
