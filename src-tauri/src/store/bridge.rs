//! Lien VS Code (propre à ChatonLM) : réglage du pont local et éditeurs
//! appairés, avec l'accès « agents » qu'un éditeur crée pour son serveur MCP
//! (migration 0036 : au plus un par éditeur, retiré avec lui).
//!
//! Le secret d'un éditeur ne sort de ce module que vers le pont
//! (`crate::bridge`) : aucun type exporté vers la page ne le porte.

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::AppResult;

/// Portée d'un client du pont.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Éditeur appairé par un code montré dans l'app.
    Editor,
    /// Accès des agents d'un éditeur (serveur MCP de l'extension) : limité
    /// par l'app aux besoins de ses outils.
    Agents,
}

impl Scope {
    fn code(self) -> &'static str {
        match self {
            Scope::Editor => "editor",
            Scope::Agents => "agents",
        }
    }
}

/// Client du pont (éditeur ou accès agents), secret compris.
#[derive(Clone, PartialEq, Eq)]
pub struct Client {
    pub id: String,
    pub name: String,
    pub secret: [u8; 32],
    pub paired_at: i64,
    pub last_seen_at: Option<i64>,
    pub scope: Scope,
    /// Éditeur d'un accès agents (`None` pour un éditeur).
    pub parent_id: Option<String>,
}

// Jamais de secret dans un journal ou un message d'essai.
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("paired_at", &self.paired_at)
            .field("last_seen_at", &self.last_seen_at)
            .field("scope", &self.scope)
            .field("parent_id", &self.parent_id)
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
    let scope: String = row.get(5)?;
    Ok(Client {
        id: row.get(0)?,
        name: row.get(1)?,
        secret,
        paired_at: row.get(3)?,
        last_seen_at: row.get(4)?,
        scope: if scope == "agents" {
            Scope::Agents
        } else {
            Scope::Editor
        },
        parent_id: row.get(6)?,
    })
}

const COLUMNS: &str = "id, name, secret, paired_at, last_seen_at, scope, parent_id";

/// Éditeurs appairés (sans les accès agents), du plus ancien au plus récent.
pub fn clients(connection: &Connection) -> AppResult<Vec<Client>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {COLUMNS} FROM bridge_clients WHERE scope = 'editor'
         ORDER BY paired_at, id"
    ))?;
    let clients = statement
        .query_map([], client_from)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(clients)
}

/// Client (éditeur ou accès agents).
pub fn client(connection: &Connection, id: &str) -> AppResult<Option<Client>> {
    Ok(connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM bridge_clients WHERE id = ?1"),
            params![id],
            client_from,
        )
        .optional()?)
}

/// Accès agents de l'éditeur `editor`, s'il en a un (essais).
#[cfg(test)]
pub fn agents_of(connection: &Connection, editor: &str) -> AppResult<Option<Client>> {
    Ok(connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM bridge_clients WHERE parent_id = ?1"),
            params![editor],
            client_from,
        )
        .optional()?)
}

pub fn add_client(connection: &Connection, client: &Client) -> AppResult<()> {
    connection.execute(
        "INSERT INTO bridge_clients (id, name, secret, paired_at, last_seen_at, scope, parent_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            client.id,
            client.name,
            client.secret.as_slice(),
            client.paired_at,
            client.last_seen_at,
            client.scope.code(),
            client.parent_id
        ],
    )?;
    Ok(())
}

/// Nouvel accès agents de son éditeur (`agents.parent_id`), qui remplace le
/// précédent, d'un seul tenant. Rend (créé, identifiant de l'accès
/// remplacé) ; si l'éditeur n'existe plus (ou n'en est pas un), rien n'est
/// créé : `(false, None)`.
pub fn replace_agents(
    connection: &mut Connection,
    agents: &Client,
) -> AppResult<(bool, Option<String>)> {
    let editor = agents.parent_id.clone().unwrap_or_default();
    let transaction = connection.transaction()?;
    let exists = transaction
        .query_row(
            "SELECT 1 FROM bridge_clients WHERE id = ?1 AND scope = 'editor'",
            params![editor],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists {
        return Ok((false, None));
    }
    let replaced = transaction
        .query_row(
            "DELETE FROM bridge_clients WHERE parent_id = ?1 RETURNING id",
            params![editor],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    add_client(&transaction, agents)?;
    transaction.commit()?;
    Ok((true, replaced))
}

/// Retire l'accès agents de l'éditeur `editor` ; rend son identifiant s'il
/// en avait un.
pub fn remove_agents(connection: &Connection, editor: &str) -> AppResult<Option<String>> {
    Ok(connection
        .query_row(
            "DELETE FROM bridge_clients WHERE parent_id = ?1 RETURNING id",
            params![editor],
            |row| row.get(0),
        )
        .optional()?)
}

/// Retire un client et, pour un éditeur, son accès agents (même requête) ;
/// rend les identifiants retirés (vide : inconnu).
pub fn remove_client(connection: &Connection, id: &str) -> AppResult<Vec<String>> {
    let mut statement = connection
        .prepare("DELETE FROM bridge_clients WHERE id = ?1 OR parent_id = ?1 RETURNING id")?;
    let removed = statement
        .query_map(params![id], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(removed)
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
            scope: Scope::Editor,
            parent_id: None,
        }
    }

    fn agents(id: &str, editor: &str) -> Client {
        Client {
            scope: Scope::Agents,
            parent_id: Some(editor.to_owned()),
            name: format!("VS Code {editor} (agents)"),
            ..client(id, 30)
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
                assert_eq!(remove_client(c, "a")?, ["a"]);
                assert!(remove_client(c, "a")?.is_empty());
                assert!(super::client(c, "a")?.is_none());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn an_editor_has_one_agents_access_at_most_removed_with_it() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                add_client(c, &client("e", 10))?;
                add_client(c, &client("f", 20))?;
                assert_eq!(replace_agents(c, &agents("x", "e"))?, (true, None));
                // Le suivant remplace le premier, d'un seul tenant.
                assert_eq!(
                    replace_agents(c, &agents("y", "e"))?,
                    (true, Some("x".to_owned()))
                );
                assert!(super::client(c, "x")?.is_none());
                let y = agents_of(c, "e")?.unwrap();
                assert_eq!((y.id.as_str(), y.scope), ("y", Scope::Agents));
                assert_eq!(y.parent_id.as_deref(), Some("e"));
                // Un second accès du même éditeur est refusé par la base.
                assert!(add_client(c, &agents("z", "e")).is_err());
                // Éditeur inconnu, ou accès agents pris pour un éditeur :
                // rien n'est créé.
                assert_eq!(replace_agents(c, &agents("w", "nobody"))?, (false, None));
                assert_eq!(replace_agents(c, &agents("w", "y"))?, (false, None));
                assert!(super::client(c, "w")?.is_none());
                // La liste de la page : les éditeurs seuls.
                let ids: Vec<_> = clients(c)?.into_iter().map(|client| client.id).collect();
                assert_eq!(ids, ["e", "f"]);
                // Retrait seul, idempotent.
                add_client(c, &agents("g", "f"))?;
                assert_eq!(remove_agents(c, "f")?.as_deref(), Some("g"));
                assert_eq!(remove_agents(c, "f")?, None);
                assert!(super::client(c, "f")?.is_some(), "the editor stays");
                // Retirer l'éditeur retire son accès agents.
                let mut removed = remove_client(c, "e")?;
                removed.sort();
                assert_eq!(removed, ["e", "y"]);
                assert!(super::client(c, "y")?.is_none());
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
