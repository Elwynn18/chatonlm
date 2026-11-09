//! Base du relais (SQLite) : ce qui doit survivre à un redémarrage, et rien
//! d'autre.
//!
//! - `keys` : clé publique d'installation (pseudonyme), jour d'inscription et
//!   jour du dernier usage (au jour près, pas plus fin). Oubliée après
//!   `key_retention_days` sans usage.
//! - `budget` : par mois, dépense comptée (micro-euros) et nombre de
//!   recherches par fournisseur (compteurs agrégés).
//! - `budget_day` : dépense du jour (une ligne, le jour courant) : un
//!   redémarrage ne rouvre pas l'enveloppe du jour.
//!
//! Jamais : adresse IP, requête, résultat. Les quotas du jour et les limites
//! par réseau sont en mémoire seulement.

use relay_proto::sign::PublicKey;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::Path;

pub struct Store {
    connection: Connection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyRecord {
    pub created_day: u64,
    pub last_seen_day: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MonthRecord {
    pub spent_micro_eur: u64,
    pub brave: u64,
    pub mojeek: u64,
    pub failures: u64,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS keys (
    public_key BLOB PRIMARY KEY NOT NULL,
    created_day INTEGER NOT NULL,
    last_seen_day INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS budget (
    month TEXT PRIMARY KEY NOT NULL,
    spent_micro_eur INTEGER NOT NULL DEFAULT 0,
    brave INTEGER NOT NULL DEFAULT 0,
    mojeek INTEGER NOT NULL DEFAULT 0,
    failures INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS budget_day (
    day INTEGER PRIMARY KEY NOT NULL,
    spent_micro_eur INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
PRAGMA user_version = 2;
";

/// SQLite n'a que des entiers signés : les compteurs y passent en `i64`.
fn sql(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn unsql(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    Ok(u64::try_from(row.get::<_, i64>(index)?).unwrap_or(0))
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        // Pages libérées effacées : une clé oubliée ne reste pas dans le fichier.
        connection.pragma_update(None, "secure_delete", "ON")?;
        // Le `-wal` ramené à rien après chaque point de reprise : il ne garde
        // pas de vieilles pages (clés oubliées comprises) au-delà.
        connection.pragma_update(None, "journal_size_limit", 0)?;
        connection.execute_batch(SCHEMA)?;
        Ok(Self { connection })
    }

    /// Lecture seule, pour `chatonlm-relay stats`, lancé par root à côté du service : rien n'est
    /// créé ni écrit. Sinon un `-wal` ou un `-shm` appartenant à root resterait dans le dossier
    /// du compte jetable du service (`DynamicUser`), qui ne pourrait plus écrire sa base. Base
    /// absente : erreur (aucune base vide créée). Service en marche (`-wal` et `-shm` présents) :
    /// lecture ordinaire, qui voit les écritures en cours ; sinon la base est lue comme immuable,
    /// sans fichier `-shm` à créer.
    pub fn open_read_only(path: &Path) -> rusqlite::Result<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let side = |suffix: &str| {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            std::path::PathBuf::from(name).exists()
        };
        if side("-wal") && side("-shm") {
            return Ok(Self {
                connection: Connection::open_with_flags(path, flags)?,
            });
        }
        let absolute =
            std::path::absolute(path).map_err(|_| rusqlite::Error::InvalidPath(path.to_owned()))?;
        let mut uri = url::Url::from_file_path(&absolute)
            .map_err(|()| rusqlite::Error::InvalidPath(path.to_owned()))?;
        uri.set_query(Some("immutable=1"));
        Ok(Self {
            connection: Connection::open_with_flags(uri.as_str(), flags)?,
        })
    }

    pub fn all_keys(&self) -> rusqlite::Result<Vec<(PublicKey, KeyRecord)>> {
        let mut statement = self
            .connection
            .prepare("SELECT public_key, created_day, last_seen_day FROM keys")?;
        let rows = statement.query_map([], |row| {
            let bytes: Vec<u8> = row.get(0)?;
            Ok((
                bytes,
                KeyRecord {
                    created_day: unsql(row, 1)?,
                    last_seen_day: unsql(row, 2)?,
                },
            ))
        })?;
        let mut keys = Vec::new();
        for row in rows {
            let (bytes, record) = row?;
            if let Ok(bytes) = <[u8; 32]>::try_from(bytes) {
                keys.push((PublicKey(bytes), record));
            }
        }
        Ok(keys)
    }

    pub fn insert_key(&self, key: &PublicKey, day: u64) -> rusqlite::Result<()> {
        self.connection.execute(
            "INSERT INTO keys (public_key, created_day, last_seen_day) VALUES (?1, ?2, ?2)
             ON CONFLICT(public_key) DO UPDATE SET last_seen_day = excluded.last_seen_day",
            params![key.0.as_slice(), sql(day)],
        )?;
        Ok(())
    }

    pub fn touch_key(&self, key: &PublicKey, day: u64) -> rusqlite::Result<()> {
        self.connection.execute(
            "UPDATE keys SET last_seen_day = ?2 WHERE public_key = ?1",
            params![key.0.as_slice(), sql(day)],
        )?;
        Ok(())
    }

    /// Oublie les clés inutilisées depuis `before_day` ; rend leur nombre.
    /// `secure_delete` efface leurs pages de la base, mais le `-wal` garde les
    /// trames qui les ont écrites jusqu'à ce qu'elles soient réécrites : il est
    /// reversé dans la base et vidé aussitôt (rétention de 90 jours tenue).
    pub fn purge_keys(&self, before_day: u64) -> rusqlite::Result<usize> {
        let purged = self.connection.execute(
            "DELETE FROM keys WHERE last_seen_day < ?1",
            params![sql(before_day)],
        )?;
        if purged > 0 {
            // Occupé (un `stats` lit en même temps) : refait à la purge suivante.
            let _ = self
                .connection
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        }
        Ok(purged)
    }

    /// Dépense du jour gardée (0 si c'est un autre jour).
    pub fn day_spent(&self, day: u64) -> rusqlite::Result<u64> {
        Ok(self
            .connection
            .query_row(
                "SELECT spent_micro_eur FROM budget_day WHERE day = ?1",
                params![sql(day)],
                |row| unsql(row, 0),
            )
            .optional()?
            .unwrap_or(0))
    }

    /// Garde la dépense du jour ; les jours passés sont oubliés.
    pub fn save_day(&self, day: u64, spent: u64) -> rusqlite::Result<()> {
        self.connection.execute(
            "INSERT INTO budget_day (day, spent_micro_eur) VALUES (?1, ?2)
             ON CONFLICT(day) DO UPDATE SET spent_micro_eur = excluded.spent_micro_eur",
            params![sql(day), sql(spent)],
        )?;
        self.connection
            .execute("DELETE FROM budget_day WHERE day < ?1", params![sql(day)])?;
        Ok(())
    }

    pub fn month(&self, month: &str) -> rusqlite::Result<MonthRecord> {
        Ok(self
            .connection
            .query_row(
                "SELECT spent_micro_eur, brave, mojeek, failures FROM budget WHERE month = ?1",
                params![month],
                |row| {
                    Ok(MonthRecord {
                        spent_micro_eur: unsql(row, 0)?,
                        brave: unsql(row, 1)?,
                        mojeek: unsql(row, 2)?,
                        failures: unsql(row, 3)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    pub fn save_month(&self, month: &str, record: &MonthRecord) -> rusqlite::Result<()> {
        self.connection.execute(
            "INSERT INTO budget (month, spent_micro_eur, brave, mojeek, failures) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(month) DO UPDATE SET spent_micro_eur = excluded.spent_micro_eur,
                 brave = excluded.brave, mojeek = excluded.mojeek, failures = excluded.failures",
            params![month, sql(record.spent_micro_eur), sql(record.brave), sql(record.mojeek), sql(record.failures)],
        )?;
        Ok(())
    }

    /// Tous les mois connus (commande `stats`).
    pub fn months(&self) -> rusqlite::Result<Vec<(String, MonthRecord)>> {
        let mut statement = self.connection.prepare(
            "SELECT month, spent_micro_eur, brave, mojeek, failures FROM budget ORDER BY month",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get(0)?,
                MonthRecord {
                    spent_micro_eur: unsql(row, 1)?,
                    brave: unsql(row, 2)?,
                    mojeek: unsql(row, 3)?,
                    failures: unsql(row, 4)?,
                },
            ))
        })?;
        rows.collect()
    }

    pub fn key_count(&self) -> rusqlite::Result<u64> {
        self.connection
            .query_row("SELECT COUNT(*) FROM keys", [], |row| unsql(row, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Noms des fichiers du dossier (base, `-wal`, `-shm`).
    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn filled(path: &Path) -> Store {
        let store = Store::open(path).unwrap();
        store.insert_key(&PublicKey([7; 32]), 20_000).unwrap();
        store
            .save_month(
                "2026-11",
                &MonthRecord {
                    spent_micro_eur: 1_500_000,
                    brave: 300,
                    mojeek: 2,
                    failures: 1,
                },
            )
            .unwrap();
        store
    }

    fn assert_readable(store: &Store) {
        assert_eq!(store.key_count().unwrap(), 1);
        let months = store.months().unwrap();
        assert_eq!(months.len(), 1);
        assert_eq!(months[0].1.brave, 300);
    }

    /// Service arrêté : `stats` lit la base sans créer de `-wal` ni de `-shm`, même pendant
    /// la lecture (root les laisserait à son nom).
    #[test]
    fn read_only_creates_no_file_when_the_service_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.sqlite");
        drop(filled(&path));
        let before = files(dir.path());
        assert_eq!(before, ["relay.sqlite"]);
        let store = Store::open_read_only(&path).unwrap();
        assert_readable(&store);
        assert_eq!(files(dir.path()), before);
        assert!(store.insert_key(&PublicKey([8; 32]), 1).is_err());
        drop(store);
        assert_eq!(files(dir.path()), before);
    }

    /// Service en marche : `stats` voit ce qui n'est encore que dans le `-wal`, sans fichier de plus.
    #[test]
    fn read_only_sees_the_running_service_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.sqlite");
        let service = filled(&path);
        let before = files(dir.path());
        assert_eq!(
            before,
            ["relay.sqlite", "relay.sqlite-shm", "relay.sqlite-wal"]
        );
        let store = Store::open_read_only(&path).unwrap();
        assert_readable(&store);
        drop(store);
        assert_eq!(files(dir.path()), before);
        drop(service);
    }

    /// M11 : une clé oubliée ne reste ni dans la base ni dans le `-wal`.
    #[test]
    fn forgotten_keys_leave_no_trace_in_the_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.sqlite");
        let store = Store::open(&path).unwrap();
        let key = *b"CLEF-OUBLIEE-0123456789-abcdefgh";
        store.insert_key(&PublicKey(key), 10).unwrap();
        store.insert_key(&PublicKey([7; 32]), 30).unwrap();
        let contains = |needle: &[u8]| {
            std::fs::read_dir(dir.path()).unwrap().any(|entry| {
                let bytes = std::fs::read(entry.unwrap().path()).unwrap();
                bytes.windows(needle.len()).any(|window| window == needle)
            })
        };
        assert!(contains(&key), "la clé est d'abord écrite");
        assert_eq!(store.purge_keys(20).unwrap(), 1);
        assert!(!contains(&key), "clé oubliée encore dans un fichier");
        assert_eq!(store.key_count().unwrap(), 1);
        drop(store);
    }

    #[test]
    fn the_day_spending_is_kept_for_the_current_day_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("relay.sqlite")).unwrap();
        assert_eq!(store.day_spent(100).unwrap(), 0);
        store.save_day(100, 1_234).unwrap();
        store.save_day(100, 2_345).unwrap();
        assert_eq!(store.day_spent(100).unwrap(), 2_345);
        store.save_day(101, 5).unwrap();
        assert_eq!(store.day_spent(100).unwrap(), 0);
        assert_eq!(store.day_spent(101).unwrap(), 5);
    }

    /// Base absente (service jamais lancé) : erreur, aucune base vide créée par root.
    #[test]
    fn read_only_never_creates_the_database() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Store::open_read_only(&dir.path().join("relay.sqlite")).is_err());
        assert!(files(dir.path()).is_empty());
    }
}
