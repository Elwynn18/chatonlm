//! Ouverture de la base au démarrage, et ce que l'app montre quand elle échoue.
//!
//! Une base illisible ne doit pas empêcher la fenêtre de s'ouvrir : l'erreur
//! est gardée ici et le frontend l'explique (base d'une version plus récente,
//! fichier abîmé, base occupée...). Il peut relancer l'ouverture, ou repartir
//! d'une base neuve en gardant l'ancien fichier à côté, sous un autre nom.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use rusqlite::ErrorCode;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::i18n::{self, t};
use crate::store::{self, Store, chats, settings};

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
                    eprintln!("chatonlm: cannot repair interrupted answers: {error}");
                }
                // Conversations éphémères restées après un arrêt brutal.
                if let Err(error) = store.blocking(|c| chats::forget_all(c)) {
                    eprintln!("chatonlm: cannot erase the ephemeral chats: {error}");
                }
                // Premier démarrage montré de nouveau ou passé d'office
                // (`CHATONLM_ONBOARDING`), avant que la page lise les
                // réglages ; aussi pour une base recréée après coup.
                if let Some(done) = settings::onboarding_override(
                    std::env::var("CHATONLM_ONBOARDING").ok().as_deref(),
                ) && let Err(error) = store.blocking(|c| settings::set_onboarding_done(c, done))
                {
                    eprintln!("chatonlm: CHATONLM_ONBOARDING ignored: {error}");
                }
                Some(store)
            }
            Err(error) => {
                eprintln!("chatonlm: cannot open the database: {error}");
                *self.problem.lock() = Some(DatabaseProblem {
                    kind: classify(&error),
                    message: error.to_string(),
                    path: self.database.display().to_string(),
                });
                None
            }
        }
    }

    fn problem(&self) -> Option<DatabaseProblem> {
        self.problem.lock().clone()
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

/// Renomme la base et ses fichiers -wal et -shm en `….broken-<horodatage>`.
#[cfg(not(windows))]
fn set_aside(database: &Path) -> AppResult<()> {
    let suffix = format!("broken-{}", store::now_ms());
    for extension in ["", "-wal", "-shm"] {
        let file = PathBuf::from(format!("{}{extension}", database.display()));
        if file.exists() {
            let kept = PathBuf::from(format!("{}.{suffix}", file.display()));
            std::fs::rename(&file, kept)?;
        }
    }
    Ok(())
}

/// Renomme la base et ses fichiers -wal et -shm en `….broken-<horodatage>`.
/// Sous Windows, un fichier tenu ouvert par un autre programme (antivirus,
/// indexation, autre instance) ne se renomme pas : voir [`rename_all`].
#[cfg(windows)]
fn set_aside(database: &Path) -> AppResult<()> {
    let suffix = format!("broken-{}", store::now_ms());
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
            set_aside(&database)?;
        }
        Store::open(&database)
    })
    .await
    .map_err(|error| AppError::Internal(error.to_string()))?;
    if let Some(store) = startup.record(result) {
        // Base retrouvée : sa langue remplace celle du bureau.
        i18n::apply(&app, i18n::saved(Some(&store)));
        app.manage(store);
        // Easy Link n'a pas pu démarrer sans base : il démarre maintenant.
        crate::easy_link::setup(&app);
    }
    Ok(startup.problem())
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

        set_aside(&startup.database).unwrap();
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
