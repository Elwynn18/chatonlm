//! Fournisseurs des modèles distants (migration 0042) : sorte, nom, adresse
//! d'un fournisseur « custom », activé ou non. La clé d'API n'est jamais en
//! base (`llm::remote::keys`). Identifiants retirés (migration 0043) : jamais
//! redonnés.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppError, AppResult};

/// Sorte de fournisseur : trois connus (adresse fixe) et un serveur
/// compatible avec l'API d'OpenAI à l'adresse de l'utilisateur.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RemoteProviderKind {
    OllamaCloud,
    OpenRouter,
    Mistral,
    Custom,
}

impl RemoteProviderKind {
    pub const ALL: [RemoteProviderKind; 4] = [
        RemoteProviderKind::OllamaCloud,
        RemoteProviderKind::OpenRouter,
        RemoteProviderKind::Mistral,
        RemoteProviderKind::Custom,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RemoteProviderKind::OllamaCloud => "ollamaCloud",
            RemoteProviderKind::OpenRouter => "openRouter",
            RemoteProviderKind::Mistral => "mistral",
            RemoteProviderKind::Custom => "custom",
        }
    }

    pub fn parse(text: &str) -> Option<RemoteProviderKind> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

/// Ligne de la table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderRow {
    /// Identifiant court (`[a-z0-9-]{1,32}`), repris dans `@remote/<id>/…`.
    pub id: String,
    pub kind: RemoteProviderKind,
    pub name: String,
    /// Adresse d'un fournisseur « custom » (sans `/v1` ni `/` final).
    pub base_url: Option<String>,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderRow> {
    let kind: String = row.get(1)?;
    Ok(ProviderRow {
        id: row.get(0)?,
        // Contrainte CHECK de la table : jamais autre chose.
        kind: RemoteProviderKind::parse(&kind).unwrap_or(RemoteProviderKind::Custom),
        name: row.get(2)?,
        base_url: row.get(3)?,
        enabled: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

const COLUMNS: &str = "id, kind, name, base_url, enabled, created_at, updated_at";

/// Fournisseurs, dans l'ordre de leur ajout.
pub fn list(connection: &Connection) -> AppResult<Vec<ProviderRow>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {COLUMNS} FROM remote_providers ORDER BY created_at, id"
    ))?;
    let rows = statement.query_map([], read)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn get(connection: &Connection, id: &str) -> AppResult<Option<ProviderRow>> {
    Ok(connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM remote_providers WHERE id = ?1"),
            params![id],
            read,
        )
        .optional()?)
}

/// Ajoute un fournisseur (identifiant libre : à l'appelant de le choisir).
pub fn insert(connection: &Connection, row: &ProviderRow) -> AppResult<()> {
    connection.execute(
        &format!("INSERT INTO remote_providers ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"),
        params![
            row.id,
            row.kind.as_str(),
            row.name,
            row.base_url,
            row.enabled,
            row.created_at,
            row.updated_at
        ],
    )?;
    Ok(())
}

/// Change le nom, l'adresse et l'état d'un fournisseur (sa sorte ne change
/// pas) ; [`AppError::NotFound`] s'il n'existe pas.
pub fn update(
    connection: &Connection,
    id: &str,
    name: &str,
    base_url: Option<&str>,
    enabled: bool,
    now: i64,
) -> AppResult<()> {
    let changed = connection.execute(
        "UPDATE remote_providers SET name = ?2, base_url = ?3, enabled = ?4, updated_at = ?5
         WHERE id = ?1",
        params![id, name, base_url, enabled, now],
    )?;
    if changed == 0 {
        return Err(AppError::NotFound(id.to_owned()));
    }
    Ok(())
}

/// Retire un fournisseur ; vrai s'il existait. Son identifiant est retenu
/// ([`free_id`] ne le redonne jamais).
pub fn delete(connection: &Connection, id: &str) -> AppResult<bool> {
    let transaction = connection.unchecked_transaction()?;
    let existed =
        transaction.execute("DELETE FROM remote_providers WHERE id = ?1", params![id])? > 0;
    if existed {
        transaction.execute(
            "INSERT OR IGNORE INTO remote_retired_ids (id) VALUES (?1)",
            params![id],
        )?;
    }
    transaction.commit()?;
    Ok(existed)
}

/// Identifiant libre pour un nouveau fournisseur : `base`, sinon `base-2`,
/// `base-3`… ; jamais celui d'un fournisseur retiré (les conversations qui
/// le citent n'iront pas vers un autre serveur).
pub fn free_id(connection: &Connection, base: &str) -> AppResult<String> {
    let mut taken: Vec<String> = list(connection)?.into_iter().map(|row| row.id).collect();
    let mut statement = connection.prepare("SELECT id FROM remote_retired_ids")?;
    let retired = statement.query_map([], |row| row.get::<_, String>(0))?;
    for id in retired {
        taken.push(id?);
    }
    Ok(std::iter::once(base.to_owned())
        .chain((2..).map(|n| format!("{base}-{n}")))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| base.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn row(id: &str, kind: RemoteProviderKind) -> ProviderRow {
        ProviderRow {
            id: id.into(),
            kind,
            name: "Name".into(),
            base_url: None,
            enabled: true,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[tokio::test]
    async fn providers_are_kept_without_any_key_column() {
        let store = Store::open_in_memory().unwrap();
        store
            .call(|c| {
                insert(c, &row("mistral", RemoteProviderKind::Mistral))?;
                let mut custom = row("custom", RemoteProviderKind::Custom);
                custom.base_url = Some("http://192.168.1.2:8080".into());
                custom.created_at = 2;
                insert(c, &custom)?;
                assert_eq!(free_id(c, "mistral")?, "mistral-2");
                assert_eq!(free_id(c, "openrouter")?, "openrouter");
                let all = list(c)?;
                assert_eq!(all.len(), 2);
                assert_eq!(all[1].base_url.as_deref(), Some("http://192.168.1.2:8080"));
                update(c, "mistral", "Mine", None, false, 9)?;
                let changed = get(c, "mistral")?.unwrap();
                assert_eq!((changed.name.as_str(), changed.enabled), ("Mine", false));
                assert_eq!(changed.updated_at, 9);
                assert!(matches!(
                    update(c, "nope", "x", None, true, 1),
                    Err(AppError::NotFound(_))
                ));
                assert!(delete(c, "mistral")?);
                assert!(!delete(c, "mistral")?);
                // Identifiant retiré : jamais redonné.
                assert_eq!(free_id(c, "mistral")?, "mistral-2");
                insert(c, &row("mistral-2", RemoteProviderKind::Mistral))?;
                assert!(delete(c, "mistral-2")?);
                assert_eq!(free_id(c, "mistral")?, "mistral-3");
                // Aucune colonne pour une clé.
                let columns: Vec<String> = c
                    .prepare("SELECT name FROM pragma_table_info('remote_providers')")?
                    .query_map([], |r| r.get(0))?
                    .collect::<Result<_, _>>()?;
                assert!(
                    columns.iter().all(|name| !name.contains("key")),
                    "{columns:?}"
                );
                // Sorte inconnue refusée par la base.
                assert!(
                    c.execute(
                        "INSERT INTO remote_providers VALUES ('x', 'openai', 'x', NULL, 1, 1, 1)",
                        []
                    )
                    .is_err()
                );
                Ok(())
            })
            .await
            .unwrap();
    }
}
