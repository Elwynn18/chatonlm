//! Base SQLite de ChatonLM.
//!
//! Une seule connexion, protégée par un verrou : les requêtes sont courtes.
//! Les commandes Tauri sont asynchrones et passent par [`Store::call`], qui
//! exécute la requête sur un thread bloquant : ni la boucle GTK ni le runtime
//! asynchrone n'attendent jamais le verrou.

pub mod bridge;
pub mod builtin_ollama;
pub mod chats;
pub mod desktop;
pub mod dictation;
pub mod easy_link;
pub mod image_models;
pub mod model_prefs;
pub mod settings;
pub mod study;
pub mod updates;
pub mod voice_mode;

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rusqlite::{Connection, ErrorCode, TransactionBehavior};

use crate::error::{AppError, AppResult};

/// Migrations, dans l'ordre ; la version du schéma est `PRAGMA user_version`.
const MIGRATIONS: &[&str] = &[
    include_str!("../../migrations/0001_settings.sql"),
    include_str!("../../migrations/0002_chats.sql"),
    include_str!("../../migrations/0003_model_prefs.sql"),
    include_str!("../../migrations/0004_model_traits.sql"),
    include_str!("../../migrations/0005_web_search.sql"),
    include_str!("../../migrations/0006_dictation.sql"),
    include_str!("../../migrations/0007_dictation_auto_copy.sql"),
    include_str!("../../migrations/0008_model_loading.sql"),
    include_str!("../../migrations/0009_chat_favorites.sql"),
    include_str!("../../migrations/0010_language.sql"),
    include_str!("../../migrations/0011_proofread.sql"),
    include_str!("../../migrations/0012_study.sql"),
    include_str!("../../migrations/0013_menu_entry.sql"),
    include_str!("../../migrations/0014_builtin_ollama.sql"),
    include_str!("../../migrations/0015_update_settings.sql"),
    include_str!("../../migrations/0016_read_aloud.sql"),
    include_str!("../../migrations/0017_voice_mode.sql"),
    include_str!("../../migrations/0018_shortcut_keys.sql"),
    include_str!("../../migrations/0019_study_pause.sql"),
    include_str!("../../migrations/0020_proofread_paste.sql"),
    include_str!("../../migrations/0021_onboarding.sql"),
    include_str!("../../migrations/0022_easy_link.sql"),
    include_str!("../../migrations/0023_context.sql"),
    include_str!("../../migrations/0024_show_uncensored.sql"),
    include_str!("../../migrations/0025_system_prompt.sql"),
    include_str!("../../migrations/0026_image_models.sql"),
    include_str!("../../migrations/0027_image_model_timing.sql"),
    include_str!("../../migrations/0028_unload_on_quit.sql"),
    include_str!("../../migrations/0029_cloud_models.sql"),
    include_str!("../../migrations/0030_ephemeral.sql"),
    include_str!("../../migrations/0031_dictation_auto_paste.sql"),
    include_str!("../../migrations/0032_close_to_tray.sql"),
    include_str!("../../migrations/0033_bridge.sql"),
    include_str!("../../migrations/0034_chat_editor.sql"),
    include_str!("../../migrations/0035_easy_link_vscode.sql"),
    include_str!("../../migrations/0036_bridge_agents.sql"),
];

/// Heure courante en millisecondes depuis l'époque Unix.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[derive(Clone)]
pub struct Store {
    connection: Arc<Mutex<Connection>>,
}

impl Store {
    /// Ouvre (ou crée) la base, puis applique les migrations manquantes.
    pub fn open(path: &Path) -> AppResult<Self> {
        Self::open_with(path, BUSY_TIMEOUT)
    }

    /// [`Store::open`], avec le délai d'attente des verrous de SQLite au choix
    /// (les essais l'abaissent).
    fn open_with(path: &Path, busy_timeout: Duration) -> AppResult<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            restrict_to_owner(dir);
        }
        // Tenu jusqu'à la fin des migrations, relâché en sortant.
        let _opening = lock_openings(path, OPENING_WAIT);
        let mut connection = Connection::open(path)?;
        configure(&connection, busy_timeout)?;
        migrate(&mut connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Base en mémoire, pour les tests.
    #[cfg(test)]
    pub fn open_in_memory() -> AppResult<Self> {
        let mut connection = Connection::open_in_memory()?;
        configure(&connection, BUSY_TIMEOUT)?;
        migrate(&mut connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Exécute `f` sur la connexion, depuis un thread bloquant.
    pub async fn call<T, F>(&self, f: F) -> AppResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> AppResult<T> + Send + 'static,
    {
        let connection = Arc::clone(&self.connection);
        tauri::async_runtime::spawn_blocking(move || f(&mut connection.lock()))
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?
    }

    /// Accès direct, pour le démarrage, la fermeture de la fenêtre et les tests
    /// (requêtes instantanées) ; jamais depuis une commande.
    pub fn blocking<T>(&self, f: impl FnOnce(&mut Connection) -> AppResult<T>) -> AppResult<T> {
        f(&mut self.connection.lock())
    }
}

/// Réserve le dossier de la base au compte de l'utilisateur (0700) : elle
/// garde les conversations et, depuis Easy Link, le jeton d'un bot. Sans
/// effet si le système le refuse (la base s'ouvre quand même). Sous Windows,
/// `%LOCALAPPDATA%` est déjà propre au compte.
#[cfg(unix)]
fn restrict_to_owner(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_to_owner(_dir: &Path) {}

/// Attente maximale d'un verrou de SQLite tenu par une autre connexion (autre
/// fenêtre, autre programme) : les requêtes sont courtes. Une ouverture en
/// cours ne l'est pas : voir [`lock_openings`].
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Attente maximale du verrou des ouvertures : bien plus que la plus longue
/// ouverture mesurée sur un disque saturé (une dizaine de secondes).
const OPENING_WAIT: Duration = Duration::from_secs(60);

/// Prend le verrou des ouvertures de la base (`<base>.opening`, verrou
/// consultatif du système), à garder de l'ouverture à la fin des migrations.
///
/// Deux ouvertures simultanées passent ainsi l'une après l'autre, et la
/// seconde attend la première. Un second lancement de ChatonLM sort d'habitude
/// avant d'ouvrir la base (instance unique) : restent un bureau sans bus de
/// session, ou deux builds sur le même dossier de données. Sous les seuls
/// verrous de SQLite, la seconde ouverture échouait alors avec « database is
/// locked » quand la première tenait le verrou d'écriture plus longtemps que
/// `BUSY_TIMEOUT` : créer le journal et valider les migrations coûtent
/// plusieurs écritures synchrones, lentes sur un disque lent ou saturé
/// (mesuré : plus de 5 s pour le seul COMMIT, `fsync` à 1 s).
///
/// Ce verrou n'ajoute aucun échec : `None` s'il ne peut pas être pris (fichier
/// impossible à créer, système de fichiers sans verrous) ou s'il est encore
/// tenu après `wait` (processus figé en pleine ouverture), et l'ouverture
/// continue alors sous les seuls verrous de SQLite. Le système le relâche à la
/// fermeture du fichier, donc aussi quand le processus meurt ; le fichier,
/// vide, reste à côté de la base.
fn lock_openings(path: &Path, wait: Duration) -> Option<File> {
    let mut name = path.as_os_str().to_owned();
    name.push(".opening");
    let without = |error: &dyn std::fmt::Display| {
        eprintln!("chatonlm: opening the database without its opening lock: {error}");
        None
    };
    let file = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(name)
    {
        Ok(file) => file,
        Err(error) => return without(&error),
    };
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Some(file),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return without(&error),
        }
    }
}

fn configure(connection: &Connection, busy_timeout: Duration) -> AppResult<()> {
    connection.busy_timeout(busy_timeout)?;
    // Passer en WAL écrit l'en-tête d'une base neuve : deux ouvertures
    // simultanées (sans le verrou des ouvertures, ou face à un autre programme)
    // peuvent alors s'interbloquer, et SQLite répond « busy » sans attendre.
    // On réessaie pendant le même délai.
    let deadline = Instant::now() + busy_timeout;
    loop {
        match connection.pragma_update(None, "journal_mode", "WAL") {
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == ErrorCode::DatabaseBusy && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            result => break result?,
        }
    }
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "foreign_keys", true)?;
    // Contenu effacé écrasé de zéros (conversations éphémères, et toute
    // suppression) au lieu de rester lisible dans les pages libres.
    connection.pragma_update(None, "secure_delete", true)?;
    Ok(())
}

/// Applique les migrations manquantes en une transaction IMMEDIATE : deux
/// ouvertures simultanées migrent l'une après l'autre, et la seconde relit la
/// version une fois le verrou obtenu, au lieu de rejouer les migrations.
fn migrate(connection: &mut Connection) -> AppResult<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let supported = MIGRATIONS.len() as i64;
    if current > supported {
        return Err(AppError::NewerDatabase {
            found: current,
            supported,
        });
    }
    if current < 0 {
        return Err(AppError::Internal(format!(
            "invalid database schema version {current}"
        )));
    }
    if current == supported {
        return Ok(());
    }
    for sql in &MIGRATIONS[current as usize..] {
        transaction.execute_batch(sql)?;
    }
    transaction.pragma_update(None, "user_version", supported)?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(store: &Store) -> i64 {
        store
            .blocking(|c| Ok(c.pragma_query_value(None, "user_version", |r| r.get(0))?))
            .unwrap()
    }

    #[test]
    fn simultaneous_openings_of_a_new_database_all_succeed() {
        for round in 0..20 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("chatonlm.sqlite");
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let openers: Vec<_> = (0..2)
                .map(|_| {
                    let (path, barrier) = (path.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        barrier.wait();
                        Store::open(&path)
                    })
                })
                .collect();
            // Comme dans l'app, où la base reste ouverte tant que le processus
            // vit, aucune n'est fermée avant que toutes soient ouvertes : la
            // fermeture écrit son point de contrôle sous un verrou exclusif,
            // que l'ouverture voisine n'attendrait pas plus de `BUSY_TIMEOUT`.
            let stores: Vec<_> = openers
                .into_iter()
                .map(|opener| opener.join().unwrap())
                .collect();
            for store in &stores {
                let result = store.as_ref().map(version);
                assert_eq!(
                    result.as_ref().ok(),
                    Some(&(MIGRATIONS.len() as i64)),
                    "round {round}: {result:?}"
                );
            }
        }
    }

    /// Une ouverture qui dure plus que le délai des requêtes (disque lent ou
    /// saturé) : la suivante l'attend, au lieu d'échouer avec « database is
    /// locked » une fois ce délai passé.
    #[test]
    fn an_opening_waits_for_one_that_outlasts_the_busy_timeout() {
        let busy_timeout = Duration::from_millis(20);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let (started, wait) = std::sync::mpsc::channel();
        let slow = {
            let path = path.clone();
            std::thread::spawn(move || {
                // Ouverture en cours ailleurs, arrêtée sous le verrou d'écriture
                // de SQLite comme par une écriture synchrone qui n'en finit pas.
                let opening = lock_openings(&path, OPENING_WAIT).unwrap();
                let mut connection = Connection::open(&path).unwrap();
                configure(&connection, busy_timeout).unwrap();
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .unwrap();
                started.send(()).unwrap();
                std::thread::sleep(busy_timeout * 20);
                drop(transaction);
                drop(connection);
                drop(opening);
            })
        };
        wait.recv().unwrap();
        let store = Store::open_with(&path, busy_timeout).unwrap();
        assert_eq!(version(&store), MIGRATIONS.len() as i64);
        slow.join().unwrap();
    }

    /// Le verrou des ouvertures n'ajoute aucun échec : encore tenu après
    /// l'attente (processus figé en pleine ouverture) ou impossible à prendre,
    /// l'ouverture continue sous les seuls verrous de SQLite.
    #[test]
    fn an_opening_goes_on_without_a_lock_it_cannot_take() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let held = lock_openings(&path, OPENING_WAIT).unwrap();
        let wait = Duration::from_millis(50);
        let started = Instant::now();
        assert!(lock_openings(&path, wait).is_none());
        let waited = started.elapsed();
        assert!(waited >= wait && waited < OPENING_WAIT, "{waited:?}");
        drop(held);
        assert!(lock_openings(&path, wait).is_some());

        // Un dossier à la place du fichier du verrou : jamais pris.
        let path = dir.path().join("other.sqlite");
        std::fs::create_dir(dir.path().join("other.sqlite.opening")).unwrap();
        assert!(lock_openings(&path, wait).is_none());
        let store = Store::open(&path).unwrap();
        assert_eq!(version(&store), MIGRATIONS.len() as i64);
    }

    #[test]
    fn creates_the_file_and_applies_every_migration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db").join("chatonlm.sqlite");
        let store = Store::open(&path).unwrap();
        assert!(path.exists());
        assert_eq!(version(&store), MIGRATIONS.len() as i64);
    }

    #[test]
    fn reopening_keeps_the_data_and_the_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        Store::open(&path)
            .unwrap()
            .blocking(|c| Ok(c.execute("UPDATE settings SET selected_model = 'qwen3:0.6b'", [])?))
            .unwrap();
        let store = Store::open(&path).unwrap();
        let model: String = store
            .blocking(|c| Ok(c.query_row("SELECT selected_model FROM settings", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(model, "qwen3:0.6b");
        assert_eq!(version(&store), MIGRATIONS.len() as i64);
    }

    /// Base laissée par l'étape 17 (schéma 7) : la migration 0008 ajoute la
    /// durée en mémoire (défaut du serveur) et le préchargement (coupé) sans
    /// toucher aux autres réglages ni aux conversations.
    #[test]
    fn a_version_7_database_gains_the_model_loading_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let connection = Connection::open(&path).unwrap();
        for sql in &MIGRATIONS[..7] {
            connection.execute_batch(sql).unwrap();
        }
        connection.pragma_update(None, "user_version", 7).unwrap();
        connection
            .execute_batch(
                "UPDATE settings SET selected_model = 'qwen3:0.6b', context_length = 8192,
                                     web_search_enabled = 1;
                 INSERT INTO chats (id, title, created_at, updated_at)
                 VALUES ('chat-1', 'Kept', 1, 2);",
            )
            .unwrap();
        drop(connection);

        let store = Store::open(&path).unwrap();
        assert_eq!(version(&store), MIGRATIONS.len() as i64);
        let saved = store.blocking(|c| settings::get(c, None)).unwrap();
        assert_eq!(saved.selected_model, "qwen3:0.6b");
        assert_eq!(saved.context_length, 8192);
        assert!(saved.web_search_enabled);
        assert_eq!(saved.keep_alive, 0);
        assert!(!saved.preload_model);
        assert_eq!(store.blocking(|c| chats::list(c)).unwrap().len(), 1);
        // Valeurs impossibles refusées par la base elle-même.
        for refused in [
            "UPDATE settings SET keep_alive = -2",
            "UPDATE settings SET preload_model = 2",
        ] {
            assert!(
                store.blocking(|c| Ok(c.execute(refused, [])?)).is_err(),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_version_8_database_gains_the_favorites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let connection = Connection::open(&path).unwrap();
        for sql in &MIGRATIONS[..8] {
            connection.execute_batch(sql).unwrap();
        }
        connection.pragma_update(None, "user_version", 8).unwrap();
        connection
            .execute_batch(
                "INSERT INTO chats (id, title, created_at, updated_at)
                 VALUES ('chat-1', 'Kept', 1, 2);",
            )
            .unwrap();
        drop(connection);

        let store = Store::open(&path).unwrap();
        assert_eq!(version(&store), MIGRATIONS.len() as i64);
        let listed = store.blocking(|c| chats::list(c)).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "Kept");
        assert!(!listed[0].favorite);
        assert!(
            store
                .blocking(|c| Ok(c.execute("UPDATE chats SET favorite = 2", [])?))
                .is_err()
        );
    }

    /// Base laissée par les corrections de l'étape 15 (schéma 4) : la migration
    /// 0005 ajoute les appels d'outils et le réglage de la recherche web
    /// (éteinte) sans toucher aux réglages ni aux conversations.
    #[test]
    fn a_version_4_database_gains_the_web_search() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let connection = Connection::open(&path).unwrap();
        for sql in &MIGRATIONS[..4] {
            connection.execute_batch(sql).unwrap();
        }
        connection.pragma_update(None, "user_version", 4).unwrap();
        connection
            .execute_batch(
                "UPDATE settings SET selected_model = 'qwen3:0.6b', theme = 'dark';
                 INSERT INTO chats (id, title, created_at, updated_at)
                 VALUES ('chat-1', 'Kept', 1, 2);
                 INSERT INTO messages (chat_id, role, content, created_at, updated_at)
                 VALUES ('chat-1', 'user', 'Hi', 1, 1);
                 INSERT INTO messages (chat_id, role, content, model, created_at, updated_at)
                 VALUES ('chat-1', 'assistant', 'Hello', 'qwen3:0.6b', 2, 2);",
            )
            .unwrap();
        drop(connection);

        let store = Store::open(&path).unwrap();
        // Migrations suivantes comprises (0006 : dictée).
        assert_eq!(version(&store), MIGRATIONS.len() as i64);
        let saved = store.blocking(|c| settings::get(c, None)).unwrap();
        assert_eq!(saved.selected_model, "qwen3:0.6b");
        assert_eq!(saved.theme, settings::Theme::Dark);
        assert!(!saved.web_search_enabled);
        let chat = store.blocking(|c| chats::get(c, "chat-1")).unwrap();
        assert_eq!(chat.title, "Kept");
        assert_eq!(chat.messages.len(), 2);
        assert!(chat.messages.iter().all(|m| m.tool_calls.is_empty()));
        // Les appels s'ajoutent à une ancienne réponse et partent avec elle.
        store
            .blocking(|c| chats::add_tool_calls(c, 2, &[chats::ToolCall::new("web_search", "{}")]))
            .unwrap();
        let chat = store.blocking(|c| chats::get(c, "chat-1")).unwrap();
        assert_eq!(chat.messages[1].tool_calls.len(), 1);
        store.blocking(|c| chats::delete(c, "chat-1")).unwrap();
        let left: i64 = store
            .blocking(|c| Ok(c.query_row("SELECT COUNT(*) FROM tool_calls", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(left, 0);
    }

    /// Base laissée par l'étape 15 (schéma 3) : la migration 0004 ajoute les
    /// comportements observés des modèles sans toucher aux préférences.
    #[test]
    fn a_version_3_database_gains_the_model_traits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let connection = Connection::open(&path).unwrap();
        for sql in &MIGRATIONS[..3] {
            connection.execute_batch(sql).unwrap();
        }
        connection.pragma_update(None, "user_version", 3).unwrap();
        connection
            .execute_batch("INSERT INTO model_prefs (model, think) VALUES ('lfm', 'false');")
            .unwrap();
        drop(connection);

        let store = Store::open(&path).unwrap();
        assert_eq!(version(&store), MIGRATIONS.len() as i64);
        assert_eq!(
            store.blocking(|c| model_prefs::think(c, "lfm")).unwrap(),
            Some(crate::chat::stream::Think::Enabled(false))
        );
        assert!(
            !store
                .blocking(|c| model_prefs::always_thinks(c, "lfm"))
                .unwrap()
        );
        store
            .blocking(|c| model_prefs::set_always_thinks(c, "lfm"))
            .unwrap();
        assert!(
            store
                .blocking(|c| model_prefs::always_thinks(c, "lfm"))
                .unwrap()
        );
    }

    /// Base laissée par l'étape 7 (schéma 2) : la migration 0003 ajoute les
    /// préférences par modèle sans toucher aux réglages ni à l'historique.
    #[test]
    fn a_version_2_database_gains_the_model_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let connection = Connection::open(&path).unwrap();
        for sql in &MIGRATIONS[..2] {
            connection.execute_batch(sql).unwrap();
        }
        connection.pragma_update(None, "user_version", 2).unwrap();
        connection
            .execute_batch(
                "UPDATE settings SET selected_model = 'qwen3:0.6b';
                 INSERT INTO chats (id, title, created_at, updated_at)
                 VALUES ('chat-1', 'Kept', 1, 2);",
            )
            .unwrap();
        drop(connection);

        let store = Store::open(&path).unwrap();
        assert_eq!(version(&store), MIGRATIONS.len() as i64);
        let (model, title): (String, String) = store
            .blocking(|c| {
                Ok((
                    c.query_row("SELECT selected_model FROM settings", [], |r| r.get(0))?,
                    c.query_row("SELECT title FROM chats WHERE id = 'chat-1'", [], |r| {
                        r.get(0)
                    })?,
                ))
            })
            .unwrap();
        assert_eq!((model.as_str(), title.as_str()), ("qwen3:0.6b", "Kept"));
        let think = crate::chat::stream::Think::Level("high".into());
        store
            .blocking(|c| model_prefs::set_think(c, "qwen3:0.6b", &think))
            .unwrap();
        assert_eq!(
            store
                .blocking(|c| model_prefs::think(c, "qwen3:0.6b"))
                .unwrap(),
            Some(think)
        );
    }

    /// Base laissée par la lecture à voix haute (schéma 16) : la migration
    /// 0017 ajoute les réglages du mode vocal (valeurs par défaut) sans
    /// toucher aux autres réglages ni à la dictée.
    #[test]
    fn a_version_16_database_gains_the_voice_mode_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let connection = Connection::open(&path).unwrap();
        for sql in &MIGRATIONS[..16] {
            connection.execute_batch(sql).unwrap();
        }
        connection.pragma_update(None, "user_version", 16).unwrap();
        connection
            .execute_batch(
                "UPDATE settings SET auto_read = 1;
                 UPDATE dictation_settings SET language = 'fr';",
            )
            .unwrap();
        drop(connection);

        let store = Store::open(&path).unwrap();
        assert_eq!(version(&store), MIGRATIONS.len() as i64);
        assert!(
            store
                .blocking(|c| settings::get(c, None))
                .unwrap()
                .auto_read
        );
        assert_eq!(
            store.blocking(|c| dictation::get(c)).unwrap().language,
            "fr"
        );
        assert_eq!(
            store.blocking(|c| voice_mode::get(c)).unwrap(),
            voice_mode::VoiceModeSettings::default()
        );
    }

    /// Base laissée par la version 0.9.0 (schéma 18) : la migration 0019
    /// ajoute la pause des cours ; un cours déjà là n'est pas en pause et
    /// garde son contenu.
    #[test]
    fn a_version_18_database_gains_the_course_pause() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let connection = Connection::open(&path).unwrap();
        for sql in &MIGRATIONS[..18] {
            connection.execute_batch(sql).unwrap();
        }
        connection.pragma_update(None, "user_version", 18).unwrap();
        connection
            .execute_batch(
                "INSERT INTO courses (id, topic, level, minutes, language, status,
                                      created_at, updated_at)
                 VALUES ('course-1', 'Kept', 'beginner', 20, 'fr', 'planning', 1, 2);",
            )
            .unwrap();
        drop(connection);

        let store = Store::open(&path).unwrap();
        assert_eq!(version(&store), MIGRATIONS.len() as i64);
        let row = store
            .blocking(|c| study::row(c, "course-1"))
            .unwrap()
            .unwrap();
        assert_eq!(row.topic, "Kept");
        assert!(!row.paused);
        store
            .blocking(|c| {
                study::update_row(c, "course-1", |row| {
                    row.paused = true;
                    Ok(())
                })
            })
            .unwrap();
        let row = store
            .blocking(|c| study::row(c, "course-1"))
            .unwrap()
            .unwrap();
        assert!(row.paused);
        // Valeur impossible refusée par la base elle-même.
        assert!(
            store
                .blocking(|c| Ok(c.execute("UPDATE courses SET paused = 2", [])?))
                .is_err()
        );
    }

    /// Base laissée par la version 0.9.0 (schéma 18) : la migration 0020
    /// ajoute « coller à la place de la sélection », activé, sans toucher aux
    /// autres réglages ; il s'enregistre ensuite comme les autres.
    #[test]
    fn a_version_18_database_gains_the_paste_setting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let connection = Connection::open(&path).unwrap();
        for sql in &MIGRATIONS[..18] {
            connection.execute_batch(sql).unwrap();
        }
        connection.pragma_update(None, "user_version", 18).unwrap();
        connection
            .execute_batch(
                "UPDATE settings SET proofread_shortcut = 1, proofread_model = 'gemma3:4b'
                 WHERE id = 1;",
            )
            .unwrap();
        drop(connection);

        let store = Store::open(&path).unwrap();
        assert_eq!(version(&store), MIGRATIONS.len() as i64);
        let saved = store.blocking(|c| settings::get(c, None)).unwrap();
        assert!(saved.proofread_paste);
        assert!(saved.proofread_shortcut);
        assert_eq!(saved.proofread_model, "gemma3:4b");
        let patch = settings::SettingsPatch {
            proofread_paste: Some(false),
            ..settings::SettingsPatch::default()
        };
        let updated = store
            .blocking(move |c| settings::update(c, patch, None))
            .unwrap();
        assert!(!updated.proofread_paste);
        assert!(updated.proofread_shortcut);
        let again = store.blocking(|c| settings::get(c, None)).unwrap();
        assert!(!again.proofread_paste);
    }

    /// Base de la version 20 (avant le premier démarrage) : une installation
    /// qui a déjà servi (modèle choisi ou conversation) ne le verra pas, une
    /// installation jamais utilisée le verra.
    #[test]
    fn a_version_20_database_sees_the_first_run_only_if_never_used() {
        let migrated = |setup: &str| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("chatonlm.sqlite");
            let connection = Connection::open(&path).unwrap();
            for sql in &MIGRATIONS[..20] {
                connection.execute_batch(sql).unwrap();
            }
            connection.pragma_update(None, "user_version", 20).unwrap();
            connection.execute_batch(setup).unwrap();
            drop(connection);
            let store = Store::open(&path).unwrap();
            assert_eq!(version(&store), MIGRATIONS.len() as i64);
            store.blocking(|c| settings::get(c, None)).unwrap()
        };

        assert!(!migrated("").onboarding_done);
        let chosen = migrated("UPDATE settings SET selected_model = 'gemma3:4b' WHERE id = 1;");
        assert!(chosen.onboarding_done);
        assert_eq!(chosen.selected_model, "gemma3:4b");
        let chatted =
            migrated("INSERT INTO chats (id, created_at, updated_at) VALUES ('0199-a', 1, 1);");
        assert!(chatted.onboarding_done);
    }

    /// Le dossier de la base (conversations, jeton d'un bot d'Easy Link) n'est
    /// lisible que par son propriétaire, même créé plus ouvert.
    #[cfg(unix)]
    #[test]
    fn the_database_folder_is_kept_for_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("db");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        Store::open(&folder.join("chatonlm.sqlite")).unwrap();
        let mode = std::fs::metadata(&folder).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    /// Easy Link : une base de la version 21 gagne les deux bots, sans jeton,
    /// et ses conversations restent celles de l'app (sans origine).
    #[test]
    fn a_version_21_database_gains_easy_link() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let connection = Connection::open(&path).unwrap();
        for sql in &MIGRATIONS[..21] {
            connection.execute_batch(sql).unwrap();
        }
        connection.pragma_update(None, "user_version", 21).unwrap();
        connection
            .execute_batch(
                "INSERT INTO chats (id, created_at, updated_at) VALUES ('0199-a', 1, 1);",
            )
            .unwrap();
        drop(connection);
        let store = Store::open(&path).unwrap();
        assert_eq!(version(&store), MIGRATIONS.len() as i64);
        store
            .blocking(|c| {
                let bots = easy_link::bots(c)?;
                assert_eq!(bots.len(), 2);
                assert!(bots.iter().all(|bot| bot.token.is_empty() && !bot.enabled));
                assert!(easy_link::accounts(c)?.is_empty());
                let listed = chats::list(c)?;
                assert_eq!(listed.len(), 1);
                assert_eq!(listed[0].source, None);
                // Une conversation ouverte par un bot porte sa messagerie.
                chats::set_source(c, "0199-a", easy_link::Platform::Discord)?;
                assert_eq!(chats::list(c)?[0].source, Some(chats::ChatSource::Discord));
                Ok(())
            })
            .unwrap();
    }

    /// Lien VS Code : une base de la version 33 garde ses conversations sans
    /// marque ; une conversation ouverte par l'extension est marquée
    /// « vscode », et seules celles-là sont listées pour l'extension.
    #[test]
    fn a_version_33_database_gains_editor_chats() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let connection = Connection::open(&path).unwrap();
        for sql in &MIGRATIONS[..33] {
            connection.execute_batch(sql).unwrap();
        }
        connection.pragma_update(None, "user_version", 33).unwrap();
        connection
            .execute_batch(
                "INSERT INTO chats (id, created_at, updated_at) VALUES ('0199-a', 1, 1);
                 INSERT INTO chats (id, created_at, updated_at, source)
                     VALUES ('0199-b', 2, 2, 'telegram');",
            )
            .unwrap();
        drop(connection);
        let store = Store::open(&path).unwrap();
        assert_eq!(version(&store), MIGRATIONS.len() as i64);
        store
            .blocking(|c| {
                let sources = |c: &Connection| -> AppResult<Vec<_>> {
                    Ok(chats::list(c)?
                        .into_iter()
                        .map(|chat| (chat.id, chat.source))
                        .collect())
                };
                assert_eq!(
                    sources(c)?,
                    [
                        ("0199-b".to_owned(), Some(chats::ChatSource::Telegram)),
                        ("0199-a".to_owned(), None)
                    ]
                );
                assert!(chats::list_editor(c, 50)?.is_empty());
                chats::set_editor(c, "0199-a")?;
                assert_eq!(
                    sources(c)?,
                    [
                        ("0199-b".to_owned(), Some(chats::ChatSource::Telegram)),
                        ("0199-a".to_owned(), Some(chats::ChatSource::Vscode))
                    ]
                );
                let editor = chats::list_editor(c, 50)?;
                assert_eq!(editor.len(), 1);
                assert_eq!(editor[0].id, "0199-a");
                assert_eq!(editor[0].source, Some(chats::ChatSource::Vscode));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn refuses_a_database_from_a_newer_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection.pragma_update(None, "user_version", 999).unwrap();
        drop(connection);
        assert!(matches!(
            Store::open(&path),
            Err(AppError::NewerDatabase { found: 999, .. })
        ));
    }

    #[test]
    fn uses_wal_and_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("chatonlm.sqlite")).unwrap();
        let (mode, keys): (String, bool) = store
            .blocking(|c| {
                Ok((
                    c.pragma_query_value(None, "journal_mode", |r| r.get(0))?,
                    c.pragma_query_value(None, "foreign_keys", |r| r.get(0))?,
                ))
            })
            .unwrap();
        assert_eq!(mode, "wal");
        assert!(keys);
    }
}
