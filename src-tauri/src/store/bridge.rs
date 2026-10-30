//! Lien VS Code (propre à ChatonLM) : réglage du pont local et éditeurs
//! appairés.
//!
//! Le secret d'un éditeur ne sort de ce module que vers le pont
//! (`crate::bridge`) : aucun type exporté vers la page ne le porte.

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::AppResult;

/// Éditeur appairé, secret compris.
#[derive(Clone, PartialEq, Eq)]
pub struct Client {
    pub id: String,
    pub name: String,
    pub secret: [u8; 32],
    pub paired_at: i64,
    pub last_seen_at: Option<i64>,
}

// Jamais de secret dans un journal ou un message d'essai.
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("paired_at", &self.paired_at)
            .field("last_seen_at", &self.last_seen_at)
            .finish_non_exhaustive()
    }
}

pub fn enabled(connection: &Connection) -> AppResult<bool> {
    Ok(connection.query_row(
        "SELECT enabled FROM bridge_settings WHERE id = 1",
        [],
        |row| row.get(0),
    )?)
}

pub fn set_enabled(connection: &Connection, enabled: bool) -> AppResult<()> {
    connection.execute(
        "UPDATE bridge_settings SET enabled = ?1 WHERE id = 1",
        params![enabled],
    )?;
    Ok(())
}

fn client_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<Client> {
    let secret: Vec<u8> = row.get(2)?;
    let secret: [u8; 32] = secret.try_into().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Blob,
            "bridge secret is not 32 bytes".into(),
        )
    })?;
    Ok(Client {
        id: row.get(0)?,
        name: row.get(1)?,
        secret,
        paired_at: row.get(3)?,
        last_seen_at: row.get(4)?,
    })
}

/// Éditeurs appairés, du plus ancien au plus récent.
pub fn clients(connection: &Connection) -> AppResult<Vec<Client>> {
    let mut statement = connection.prepare(
        "SELECT id, name, secret, paired_at, last_seen_at FROM bridge_clients
         ORDER BY paired_at, id",
    )?;
    let clients = statement
        .query_map([], client_from)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(clients)
}

pub fn client(connection: &Connection, id: &str) -> AppResult<Option<Client>> {
    Ok(connection
        .query_row(
            "SELECT id, name, secret, paired_at, last_seen_at FROM bridge_clients WHERE id = ?1",
            params![id],
            client_from,
        )
        .optional()?)
}

pub fn add_client(connection: &Connection, client: &Client) -> AppResult<()> {
    connection.execute(
        "INSERT INTO bridge_clients (id, name, secret, paired_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            client.id,
            client.name,
            client.secret.as_slice(),
            client.paired_at,
            client.last_seen_at
        ],
    )?;
    Ok(())
}

/// Vrai si l'éditeur existait.
pub fn remove_client(connection: &Connection, id: &str) -> AppResult<bool> {
    Ok(connection.execute("DELETE FROM bridge_clients WHERE id = ?1", params![id])? > 0)
}

/// Dernière connexion authentifiée de l'éditeur.
pub fn touch_client(connection: &Connection, id: &str, now: i64) -> AppResult<()> {
    connection.execute(
        "UPDATE bridge_clients SET last_seen_at = ?2 WHERE id = ?1",
        params![id, now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn client(id: &str, paired_at: i64) -> Client {
        Client {
            id: id.to_owned(),
            name: format!("VS Code {id}"),
            secret: [7; 32],
            paired_at,
            last_seen_at: None,
        }
    }

    #[test]
    fn the_bridge_is_off_by_default_and_remembers_its_switch() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                assert!(!enabled(c)?);
                set_enabled(c, true)?;
                assert!(enabled(c)?);
                set_enabled(c, false)?;
                assert!(!enabled(c)?);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn clients_are_added_listed_touched_and_removed() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                add_client(c, &client("b", 20))?;
                add_client(c, &client("a", 10))?;
                let ids: Vec<_> = clients(c)?.into_iter().map(|client| client.id).collect();
                assert_eq!(ids, ["a", "b"]);
                touch_client(c, "a", 99)?;
                let a = super::client(c, "a")?.unwrap();
                assert_eq!(a.last_seen_at, Some(99));
                assert_eq!(a.secret, [7; 32]);
                assert!(remove_client(c, "a")?);
                assert!(!remove_client(c, "a")?);
                assert!(super::client(c, "a")?.is_none());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_secret_never_shows_in_debug_output() {
        let shown = format!("{:?}", client("a", 1));
        assert!(!shown.contains("secret"), "{shown}");
        assert!(
            !shown.contains('7'.to_string().repeat(3).as_str()),
            "{shown}"
        );
    }
}
