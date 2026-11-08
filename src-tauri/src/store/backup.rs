//! Copies de la base gardées avant une migration.
//!
//! Avant d'appliquer des migrations à une base existante, une copie complète
//! est écrite à côté d'elle, `<base>.v<version>.bak` (`VACUUM INTO`), avec la
//! version du schéma d'avant ; seules les [`KEPT`] plus récentes restent. Une
//! base venue d'une version plus récente de ChatonLM (retour en arrière) peut
//! ainsi être remplacée par la dernière copie que cette version sait ouvrir
//! (écran du problème de la base, `startup.rs`).

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::Connection;

use crate::error::{AppError, AppResult};

/// Nombre de copies gardées : les plus récentes (versions les plus hautes).
pub const KEPT: usize = 2;

/// Une copie trouvée à côté de la base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backup {
    /// Version du schéma de la copie (`user_version` d'avant la migration).
    pub version: i64,
    pub path: PathBuf,
    /// Date d'écriture de la copie, en millisecondes depuis l'époque Unix.
    pub saved_at: i64,
}

/// Chemin de la copie de `database` à la version `version`.
pub fn path_for(database: &Path, version: i64) -> PathBuf {
    with_suffix(database, &format!(".v{version}.bak"))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Version d'une copie d'après le nom de son fichier, si c'en est une de la
/// base nommée `database_name`.
fn version_of(file_name: &str, database_name: &str) -> Option<i64> {
    let digits = file_name
        .strip_prefix(database_name)?
        .strip_prefix(".v")?
        .strip_suffix(".bak")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|version| *version > 0)
}

/// Copies de `database`, de la plus récente (version la plus haute) à la plus
/// ancienne. Un dossier illisible n'en donne aucune.
pub fn list(database: &Path) -> Vec<Backup> {
    let (Some(dir), Some(name)) = (
        database.parent(),
        database.file_name().and_then(|name| name.to_str()),
    ) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Backup> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let version = version_of(entry.file_name().to_str()?, name)?;
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            let saved_at = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |since| {
                    i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
                });
            Some(Backup {
                version,
                path: entry.path(),
                saved_at,
            })
        })
        .collect();
    found.sort_by_key(|backup| std::cmp::Reverse(backup.version));
    found
}

/// Copie la plus récente que cette version de ChatonLM sait ouvrir (schéma
/// `supported` au plus).
pub fn latest_compatible(database: &Path, supported: i64) -> Option<Backup> {
    list(database)
        .into_iter()
        .find(|backup| backup.version <= supported)
}

/// Copie de la base à la version `current`, avant ses migrations ; puis seules
/// les [`KEPT`] plus récentes copies restent. Les conversations éphémères
/// laissées par un arrêt brutal (effacées de toute façon à l'ouverture) le
/// sont d'abord : une copie n'en garde jamais ; si elles ne peuvent pas
/// l'être, pas de copie. Un échec (disque plein, dossier en lecture seule) est
/// seulement journalisé : la migration a lieu quand même (ne pas migrer
/// bloquerait l'app).
pub fn before_migration(connection: &Connection, database: &Path, current: i64) {
    let target = path_for(database, current);
    match forget_ephemeral(connection).and_then(|()| write(connection, &target)) {
        Ok(()) => prune(database),
        Err(error) => crate::app_log!(
            "chatonlm: cannot back up the database before migrating it (schema {current}): {error}"
        ),
    }
}

/// Efface les conversations éphémères d'une base qui les connaît déjà
/// (schéma 30 et plus) ; sans effet avant.
fn forget_ephemeral(connection: &Connection) -> AppResult<()> {
    let known: bool = connection.query_row(
        "SELECT count(*) > 0 FROM pragma_table_info('chats') WHERE name = 'ephemeral'",
        [],
        |row| row.get(0),
    )?;
    if known {
        super::chats::forget_all(connection)?;
    }
    Ok(())
}

/// `VACUUM INTO` un fichier temporaire, renommé ensuite : jamais de copie à
/// moitié écrite sous le nom final, et une copie plus ancienne de la même
/// version (base déjà rouverte une fois en arrière) est remplacée.
fn write(connection: &Connection, target: &Path) -> AppResult<()> {
    let partial = with_suffix(target, ".partial");
    let name = partial
        .to_str()
        .ok_or_else(|| AppError::Internal("backup path is not valid UTF-8".to_owned()))?;
    // Reste d'une copie interrompue : `VACUUM INTO` refuse un fichier plein.
    let _ = std::fs::remove_file(&partial);
    let written = connection
        .execute("VACUUM INTO ?1", [name])
        .map_err(AppError::from)
        .and_then(|_| std::fs::rename(&partial, target).map_err(AppError::from));
    if written.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    written
}

/// Supprime les copies au-delà des [`KEPT`] plus récentes.
fn prune(database: &Path) {
    for old in list(database).into_iter().skip(KEPT) {
        if let Err(error) = std::fs::remove_file(&old.path) {
            crate::app_log!(
                "chatonlm: cannot remove the old database backup {}: {error}",
                old.path.display()
            );
        }
    }
}

/// Met la copie `backup` à la place de `database`, qui ne doit plus exister
/// (mise de côté avant) : copiée d'abord sous un nom temporaire, puis
/// renommée. La copie reste à sa place.
pub fn install(backup: &Backup, database: &Path) -> AppResult<()> {
    let partial = with_suffix(database, ".restoring");
    let _ = std::fs::remove_file(&partial);
    let copied =
        std::fs::copy(&backup.path, &partial).and_then(|_| std::fs::rename(&partial, database));
    if copied.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    Ok(copied?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_backups_of_this_database_are_recognised() {
        let name = "chatonlm.sqlite";
        assert_eq!(version_of("chatonlm.sqlite.v36.bak", name), Some(36));
        assert_eq!(version_of("chatonlm.sqlite.v7.bak", name), Some(7));
        for other in [
            "chatonlm.sqlite",
            "chatonlm.sqlite.v.bak",
            "chatonlm.sqlite.v0.bak",
            "chatonlm.sqlite.v-3.bak",
            "chatonlm.sqlite.v3a.bak",
            "chatonlm.sqlite.v36.bak.partial",
            "chatonlm.sqlite.broken-17000",
            "other.sqlite.v36.bak",
        ] {
            assert_eq!(version_of(other, name), None, "{other}");
        }
    }

    #[test]
    fn the_latest_compatible_backup_is_the_highest_version_this_one_opens() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("chatonlm.sqlite");
        assert_eq!(latest_compatible(&database, 40), None);
        for version in [12, 40, 36, 41] {
            std::fs::write(path_for(&database, version), b"").unwrap();
        }
        // Un dossier au nom d'une copie n'en est pas une.
        std::fs::create_dir(path_for(&database, 39)).unwrap();
        let versions: Vec<_> = list(&database).iter().map(|b| b.version).collect();
        assert_eq!(versions, [41, 40, 36, 12]);
        assert_eq!(latest_compatible(&database, 40).unwrap().version, 40);
        assert_eq!(latest_compatible(&database, 38).unwrap().version, 36);
        assert_eq!(latest_compatible(&database, 11), None);
        assert!(latest_compatible(&database, 40).unwrap().saved_at > 0);
    }

    #[test]
    fn a_failed_copy_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("chatonlm.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch("CREATE TABLE t (x); INSERT INTO t VALUES (1);")
            .unwrap();
        // Dossier de la copie absent : VACUUM INTO échoue.
        let target = dir.path().join("missing").join("chatonlm.sqlite.v3.bak");
        assert!(write(&connection, &target).is_err());
        assert!(!target.exists());
        assert!(!with_suffix(&target, ".partial").exists());

        // Copie existante de la même version : remplacée.
        let target = path_for(&database, 3);
        std::fs::write(&target, b"old").unwrap();
        write(&connection, &target).unwrap();
        let copy = Connection::open(&target).unwrap();
        let x: i64 = copy.query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(x, 1);
    }
}
