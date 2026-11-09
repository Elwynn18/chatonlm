//! Ouverture de la base au démarrage, et ce que l'app montre quand elle échoue.
//!
//! Une base illisible ne doit pas empêcher la fenêtre de s'ouvrir : l'erreur
//! est gardée ici et le frontend l'explique (base d'une version plus récente,
//! fichier abîmé, base occupée...). Il peut relancer l'ouverture, ou repartir
//! d'une base neuve en gardant l'ancien fichier à côté, sous un autre nom ;
//! pour une base d'une version plus récente, remettre la dernière copie
//! d'avant migration que cette version sait ouvrir (`store::backup`), la base
//! actuelle gardée à côté elle aussi.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use rusqlite::ErrorCode;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::i18n::{self, t};
use crate::store::{self, Store, backup, chats, settings};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DatabaseProblemKind {
    /// Base créée par une version plus récente de ChatonLM.
    Newer,
    /// Fichier abîmé, ou qui n'est pas une base SQLite.
    Corrupt,
    /// Base verrouillée par un autre programme.
    Locked,
    /// Dossier ou fichier inaccessible, ou autre erreur.
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DatabaseProblem {
    pub kind: DatabaseProblemKind,
    pub message: String,
    /// Chemin du fichier de la base.
    pub path: String,
    /// Base d'une version plus récente : dernière copie d'avant migration que
    /// cette version sait ouvrir, s'il y en a une.
    pub backup: Option<DatabaseBackup>,
}

/// Copie de la base qui peut la remplacer (voir `store::backup`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DatabaseBackup {
    /// Version du schéma de la copie.
    pub version: i64,
    /// Date de la copie, en millisecondes depuis l'époque Unix.
    pub saved_at: i64,
}

/// État de l'ouverture de la base, géré par Tauri dès le démarrage.
pub struct Startup {
    database: PathBuf,
    problem: Mutex<Option<DatabaseProblem>>,
}

impl Startup {
    pub fn new(database: PathBuf) -> Self {
        Self {
            database,
            problem: Mutex::new(None),
        }
    }

    /// Ouvre la base ; en cas d'échec, garde le problème pour le frontend.
    pub fn open(&self) -> Option<Store> {
        self.record(Store::open(&self.database))
    }

    fn record(&self, result: AppResult<Store>) -> Option<Store> {
        match result {
            Ok(store) => {
                self.problem.lock().take();
                // Réponses interrompues par un arrêt brutal de l'app.
                if let Err(error) = store.blocking(|c| chats::repair(c)) {
                    crate::app_log!("chatonlm: cannot repair interrupted answers: {error}");
                }
                // Conversations éphémères restées après un arrêt brutal.
                if let Err(error) = store.blocking(|c| chats::forget_all(c)) {
                    crate::app_log!("chatonlm: cannot erase the ephemeral chats: {error}");
                }
                // Premier démarrage montré de nouveau ou passé d'office
                // (`CHATONLM_ONBOARDING`), avant que la page lise les
                // réglages ; aussi pour une base recréée après coup.
                if let Some(done) = settings::onboarding_override(
                    std::env::var("CHATONLM_ONBOARDING").ok().as_deref(),
                ) && let Err(error) = store.blocking(|c| settings::set_onboarding_done(c, done))
                {
                    crate::app_log!("chatonlm: CHATONLM_ONBOARDING ignored: {error}");
                }
                Some(store)
            }
            Err(error) => {
                crate::app_log!("chatonlm: cannot open the database: {error}");
                let kind = classify(&error);
                let backup = (kind == DatabaseProblemKind::Newer)
                    .then(|| self.compatible_backup())
                    .flatten()
                    .map(|backup| DatabaseBackup {
                        version: backup.version,
                        saved_at: backup.saved_at,
                    });
                *self.problem.lock() = Some(DatabaseProblem {
                    kind,
                    message: error.to_string(),
                    path: self.database.display().to_string(),
                    backup,
                });
                None
            }
        }
    }

    fn problem(&self) -> Option<DatabaseProblem> {
        self.problem.lock().clone()
    }

    /// Dernière copie d'avant migration que cette version sait ouvrir.
    fn compatible_backup(&self) -> Option<backup::Backup> {
        backup::latest_compatible(&self.database, store::supported_version())
    }
}

fn classify(error: &AppError) -> DatabaseProblemKind {
    match error {
        AppError::NewerDatabase { .. } => DatabaseProblemKind::Newer,
        AppError::Database(rusqlite::Error::SqliteFailure(failure, _)) => match failure.code {
            ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt => DatabaseProblemKind::Corrupt,
            ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => DatabaseProblemKind::Locked,
            _ => DatabaseProblemKind::Unavailable,
        },
        _ => DatabaseProblemKind::Unavailable,
    }
}

/// Base abîmée, mise de côté pour repartir d'une base neuve.
const BROKEN: &str = "broken";
/// Base d'une version plus récente, mise de côté pour remettre une copie.
const NEWER: &str = "newer";

/// Renomme la base et ses fichiers -wal et -shm en `….<label>-<horodatage>` ;
/// renvoie les renommages faits (ancien nom, nouveau nom). Tous ou aucun : un
/// renommage refusé défait ceux déjà faits.
#[cfg(not(windows))]
fn set_aside(database: &Path, label: &str) -> AppResult<Vec<(PathBuf, PathBuf)>> {
    let suffix = format!("{label}-{}", store::now_ms());
    let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
    for extension in ["", "-wal", "-shm"] {
        let file = PathBuf::from(format!("{}{extension}", database.display()));
        if file.exists() {
            let kept = PathBuf::from(format!("{}.{suffix}", file.display()));
            if let Err(error) = std::fs::rename(&file, &kept) {
                for (from, to) in done.iter().rev() {
                    let _ = std::fs::rename(to, from);
                }
                return Err(error.into());
            }
            done.push((file, kept));
        }
    }
    Ok(done)
}

/// Renomme la base et ses fichiers -wal et -shm en `….<label>-<horodatage>` ;
/// renvoie les renommages faits (ancien nom, nouveau nom). Sous Windows, un
/// fichier tenu ouvert par un autre programme (antivirus, indexation, autre
/// instance) ne se renomme pas : voir [`rename_all`].
#[cfg(windows)]
fn set_aside(database: &Path, label: &str) -> AppResult<Vec<(PathBuf, PathBuf)>> {
    let suffix = format!("{label}-{}", store::now_ms());
    let with = |path: &Path, end: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(end);
        PathBuf::from(name)
    };
    let pairs: Vec<(PathBuf, PathBuf)> = ["", "-wal", "-shm"]
        .into_iter()
        .map(|extension| with(database, extension))
        .filter(|file| file.exists())
        .map(|file| {
            let kept = with(&file, &format!(".{suffix}"));
            (file, kept)
        })
        .collect();
    rename_all(&pairs, RENAME_TRIES, RENAME_PAUSE, |from, to| {
        std::fs::rename(from, to)
    })?;
    Ok(pairs)
}

/// Remet la copie compatible `backup` à la place de la base : la base actuelle
/// (et son journal) est d'abord mise de côté, `….newer-<horodatage>` ; si la
/// copie ne peut pas prendre sa place, la base retrouve son nom.
fn restore_backup(database: &Path, backup: &backup::Backup) -> AppResult<()> {
    let aside = set_aside(database, NEWER)?;
    if let Err(error) = backup::install(backup, database) {
        for (from, to) in aside.iter().rev() {
            let _ = std::fs::rename(to, from);
        }
        return Err(error);
    }
    Ok(())
}

/// Essais d'un renommage refusé parce que le fichier est tenu un instant par
/// un autre programme, et pause entre deux essais.
#[cfg(any(windows, test))]
const RENAME_TRIES: u32 = 5;
#[cfg(windows)]
const RENAME_PAUSE: std::time::Duration = std::time::Duration::from_millis(100);

/// Renomme chaque fichier, tous ou aucun : un renommage refusé est réessayé
/// (`tries` essais en tout, `pause` entre deux) ; s'il échoue encore, ceux
/// déjà faits sont défaits, pour ne pas laisser une base sans son journal
/// (-wal), et l'erreur est rendue.
#[cfg(any(windows, test))]
fn rename_all(
    pairs: &[(PathBuf, PathBuf)],
    tries: u32,
    pause: std::time::Duration,
    rename: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    for (index, (from, to)) in pairs.iter().enumerate() {
        let mut attempt = 1;
        let failure = loop {
            match rename(from, to) {
                Ok(()) => break None,
                Err(error) if attempt >= tries => break Some(error),
                Err(_) => {
                    attempt += 1;
                    std::thread::sleep(pause);
                }
            }
        };
        if let Some(error) = failure {
            for (from, to) in pairs[..index].iter().rev() {
                let _ = rename(to, from);
            }
            return Err(error);
        }
    }
    Ok(())
}

/// Problème rencontré à l'ouverture de la base, s'il y en a un.
#[tauri::command]
pub async fn database_problem(startup: State<'_, Startup>) -> AppResult<Option<DatabaseProblem>> {
    Ok(startup.problem())
}

/// Nouvel essai d'ouverture. Avec `fresh`, l'ancien fichier est d'abord mis de
/// côté (renommé) et une base neuve est créée ; refusé pour une base d'une
/// version plus récente, qu'il suffit de rouvrir avec cette version.
#[tauri::command]
pub async fn recover_database(
    app: AppHandle,
    startup: State<'_, Startup>,
    fresh: bool,
) -> AppResult<Option<DatabaseProblem>> {
    if app.try_state::<Store>().is_some() {
        return Ok(None);
    }
    if fresh
        && startup
            .problem()
            .is_some_and(|problem| problem.kind == DatabaseProblemKind::Newer)
    {
        return Err(AppError::Invalid(t!("startup.newerDatabase")));
    }
    let database = startup.database.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        if fresh {
            set_aside(&database, BROKEN)?;
        }
        Store::open(&database)
    })
    .await
    .map_err(|error| AppError::Internal(error.to_string()))?;
    adopt(&app, &startup, result);
    Ok(startup.problem())
}

/// Base d'une version plus récente : la dernière copie d'avant migration que
/// cette version sait ouvrir la remplace (la base actuelle est gardée à côté,
/// `….newer-<horodatage>`, avec les conversations créées depuis la copie),
/// puis la base est rouverte. Refusé pour tout autre problème, ou sans copie.
#[tauri::command]
pub async fn restore_database_backup(
    app: AppHandle,
    startup: State<'_, Startup>,
) -> AppResult<Option<DatabaseProblem>> {
    if app.try_state::<Store>().is_some() {
        return Ok(None);
    }
    let Some(backup) = startup
        .problem()
        .is_some_and(|problem| problem.kind == DatabaseProblemKind::Newer)
        .then(|| startup.compatible_backup())
        .flatten()
    else {
        return Err(AppError::Invalid(t!("startup.noBackup")));
    };
    let database = startup.database.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        restore_backup(&database, &backup)?;
        Store::open(&database)
    })
    .await
    .map_err(|error| AppError::Internal(error.to_string()))?;
    adopt(&app, &startup, result);
    Ok(startup.problem())
}

/// Résultat d'une nouvelle ouverture : une base ouverte devient celle de l'app.
fn adopt(app: &AppHandle, startup: &Startup, result: AppResult<Store>) {
    if let Some(store) = startup.record(result) {
        // Base retrouvée : sa langue remplace celle du bureau.
        i18n::apply(app, i18n::saved(Some(&store)));
        if let Err(error) = crate::llm::setup(app, store.clone()) {
            crate::app_log!("chatonlm: model access unavailable: {error}");
        }
        app.manage(store);
        // Easy Link n'a pas pu démarrer sans base : il démarre maintenant.
        crate::easy_link::setup(app);
        crate::bridge::setup(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn startup_with(contents: Option<&[u8]>) -> (tempfile::TempDir, Startup) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db").join("chatonlm.sqlite");
        if let Some(contents) = contents {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
        }
        (dir, Startup::new(path))
    }

    #[test]
    fn a_damaged_file_is_reported_then_set_aside() {
        let (dir, startup) = startup_with(Some(&[0x42; 8192]));
        assert!(startup.open().is_none());
        let problem = startup.problem().unwrap();
        assert_eq!(problem.kind, DatabaseProblemKind::Corrupt);
        assert!(problem.path.ends_with("chatonlm.sqlite"));

        set_aside(&startup.database, BROKEN).unwrap();
        assert!(startup.open().is_some());
        assert_eq!(startup.problem(), None);
        let kept: Vec<_> = std::fs::read_dir(dir.path().join("db"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("chatonlm.sqlite.broken-"))
            .collect();
        assert_eq!(kept.len(), 1, "{kept:?}");
    }

    /// App arrêtée brutalement : les conversations éphémères restées en base
    /// sont effacées à l'ouverture suivante, les autres gardées.
    #[test]
    fn ephemeral_chats_left_by_a_crash_are_erased_at_the_next_start() {
        let (_dir, startup) = startup_with(None);
        let (ordinary, ephemeral) = {
            let store = startup.open().unwrap();
            store
                .blocking(|c| {
                    Ok((
                        chats::create_with(c, 1, false)?,
                        chats::create_with(c, 1, true)?,
                    ))
                })
                .unwrap()
        };
        let store = startup.open().unwrap();
        assert!(store.blocking(move |c| chats::get(c, &ordinary)).is_ok());
        assert!(matches!(
            store.blocking(move |c| chats::get(c, &ephemeral)),
            Err(crate::error::AppError::NotFound(_))
        ));
    }

    #[test]
    fn renames_are_retried_then_undone_together() {
        use std::cell::RefCell;
        use std::time::Duration;

        let pair = |name: &str| (PathBuf::from(name), PathBuf::from(format!("{name}.kept")));
        let pairs = [pair("db"), pair("db-wal"), pair("db-shm")];
        let done = RefCell::new(Vec::new());
        let log = |from: &Path, to: &Path| {
            done.borrow_mut()
                .push(format!("{} > {}", from.display(), to.display()));
        };

        // Tout passe : chaque fichier renommé une fois, dans l'ordre.
        rename_all(&pairs, RENAME_TRIES, Duration::ZERO, |from, to| {
            log(from, to);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            done.take(),
            [
                "db > db.kept",
                "db-wal > db-wal.kept",
                "db-shm > db-shm.kept"
            ]
        );

        // Refus passager du journal : réessayé, puis la suite.
        let refusals = RefCell::new(2);
        rename_all(&pairs, RENAME_TRIES, Duration::ZERO, |from, to| {
            if from == Path::new("db-wal") && *refusals.borrow() > 0 {
                *refusals.borrow_mut() -= 1;
                return Err(std::io::Error::other("sharing violation"));
            }
            log(from, to);
            Ok(())
        })
        .unwrap();
        assert_eq!(done.take().len(), 3);

        // Refus qui dure : `tries` essais, puis la base retrouve son nom et
        // le dernier fichier n'est pas touché.
        let tried = RefCell::new(0);
        let error = rename_all(&pairs, 3, Duration::ZERO, |from, to| {
            if from == Path::new("db-wal") {
                *tried.borrow_mut() += 1;
                return Err(std::io::Error::other("sharing violation"));
            }
            log(from, to);
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "sharing violation");
        assert_eq!(*tried.borrow(), 3);
        assert_eq!(done.take(), ["db > db.kept", "db.kept > db"]);
    }

    #[test]
    fn a_newer_database_is_reported_as_such() {
        let (_dir, startup) = startup_with(None);
        std::fs::create_dir_all(startup.database.parent().unwrap()).unwrap();
        let connection = rusqlite::Connection::open(&startup.database).unwrap();
        connection.pragma_update(None, "user_version", 99).unwrap();
        drop(connection);
        assert!(startup.open().is_none());
        assert_eq!(startup.problem().unwrap().kind, DatabaseProblemKind::Newer);
    }

    /// Base d'une version plus récente avec une copie compatible : le
    /// problème la propose ; une fois remise, la base s'ouvre avec le contenu
    /// de la copie, et la base plus récente est gardée à côté, intacte.
    #[test]
    fn a_newer_database_can_be_replaced_by_its_latest_compatible_backup() {
        let (dir, startup) = startup_with(None);
        let folder = dir.path().join("db");
        // Ouverte une première fois à une ancienne version : copie v20.
        {
            std::fs::create_dir_all(&folder).unwrap();
            store::database_at(&startup.database, 20)
                .execute(
                    "INSERT INTO chats (id, created_at, updated_at) VALUES ('0199-old', 1, 1)",
                    [],
                )
                .unwrap();
        }
        drop(startup.open().unwrap());
        // Copie d'une version future : jamais proposée.
        let future = store::supported_version() + 5;
        std::fs::write(backup::path_for(&startup.database, future), b"").unwrap();
        // Puis une version plus récente de ChatonLM l'a migrée plus loin.
        {
            let connection = rusqlite::Connection::open(&startup.database).unwrap();
            connection
                .execute(
                    "INSERT INTO chats (id, created_at, updated_at) VALUES ('0199-new', 2, 2)",
                    [],
                )
                .unwrap();
            connection
                .pragma_update(None, "user_version", store::supported_version() + 1)
                .unwrap();
        }
        assert!(startup.open().is_none());
        let problem = startup.problem().unwrap();
        assert_eq!(problem.kind, DatabaseProblemKind::Newer);
        let offered = problem.backup.unwrap();
        assert_eq!(offered.version, 20);
        assert!(offered.saved_at > 0);

        let chosen = startup.compatible_backup().unwrap();
        restore_backup(&startup.database, &chosen).unwrap();
        let store = startup.open().unwrap();
        assert_eq!(startup.problem(), None);
        let ids: Vec<String> = store
            .blocking(|c| Ok(chats::list(c)?.into_iter().map(|chat| chat.id).collect()))
            .unwrap();
        assert_eq!(ids, ["0199-old"]);
        // La copie reste ; la base plus récente est à côté, intacte.
        assert!(chosen.path.exists());
        let aside: Vec<_> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("chatonlm.sqlite.newer-")
            })
            .collect();
        assert_eq!(aside.len(), 1, "{aside:?}");
        let kept = rusqlite::Connection::open(&aside[0]).unwrap();
        let newer: i64 = kept
            .query_row(
                "SELECT count(*) FROM chats WHERE id = '0199-new'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(newer, 1);
    }

    /// Une base plus récente sans copie compatible n'en propose aucune ; une
    /// base abîmée non plus, même s'il y a une copie.
    #[test]
    fn a_backup_is_offered_only_for_a_newer_database() {
        let (_dir, startup) = startup_with(Some(&[0x42; 8192]));
        std::fs::write(backup::path_for(&startup.database, 3), b"").unwrap();
        assert!(startup.open().is_none());
        let problem = startup.problem().unwrap();
        assert_eq!(problem.kind, DatabaseProblemKind::Corrupt);
        assert_eq!(problem.backup, None);

        let (_dir, startup) = startup_with(None);
        std::fs::create_dir_all(startup.database.parent().unwrap()).unwrap();
        let connection = rusqlite::Connection::open(&startup.database).unwrap();
        connection.pragma_update(None, "user_version", 999).unwrap();
        drop(connection);
        std::fs::write(backup::path_for(&startup.database, 1000), b"").unwrap();
        assert!(startup.open().is_none());
        assert_eq!(startup.problem().unwrap().backup, None);
    }

    /// La copie ne peut pas prendre la place de la base : la base retrouve
    /// son nom (rien n'est perdu, l'écran reste celui d'une base plus récente).
    #[test]
    fn a_failed_restore_puts_the_database_back() {
        let (dir, startup) = startup_with(None);
        std::fs::create_dir_all(startup.database.parent().unwrap()).unwrap();
        std::fs::write(&startup.database, b"newer").unwrap();
        let missing = backup::Backup {
            version: 3,
            path: dir.path().join("gone.bak"),
            saved_at: 1,
        };
        assert!(restore_backup(&startup.database, &missing).is_err());
        assert_eq!(std::fs::read(&startup.database).unwrap(), b"newer");
        let names: Vec<_> = std::fs::read_dir(startup.database.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["chatonlm.sqlite"]);
    }

    #[test]
    fn an_unwritable_folder_is_unavailable() {
        let (_dir, startup) = startup_with(None);
        // Un fichier à la place du dossier « db ».
        std::fs::write(startup.database.parent().unwrap(), b"").unwrap();
        assert!(startup.open().is_none());
        assert_eq!(
            startup.problem().unwrap().kind,
            DatabaseProblemKind::Unavailable
        );
    }
}
