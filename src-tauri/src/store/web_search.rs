//! Moteur de la recherche web (propre à ChatonLM) : DuckDuckGo par défaut,
//! moteur personnel en option (clé Brave Search API, clé Mojeek, adresse
//! d'une instance SearXNG). Une seule ligne (migration 0045).
//!
//! Comme les jetons d'Easy Link (`store/easy_link.rs`), les clés ne sortent
//! de ce module que vers le moteur de recherche (`crate::search`) : le type
//! exporté vers la page ([`EngineChoice`]) ne dit que si chacune est
//! configurée.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppError, AppResult};

/// Moteur choisi.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "WebSearchEngine")]
pub enum Engine {
    DuckDuckGo,
    Brave,
    Mojeek,
    Searxng,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::DuckDuckGo => "duckDuckGo",
            Engine::Brave => "brave",
            Engine::Mojeek => "mojeek",
            Engine::Searxng => "searxng",
        }
    }

    fn parse(value: &str) -> AppResult<Self> {
        Ok(match value {
            "duckDuckGo" => Engine::DuckDuckGo,
            "brave" => Engine::Brave,
            "mojeek" => Engine::Mojeek,
            "searxng" => Engine::Searxng,
            other => {
                return Err(AppError::Internal(format!(
                    "unknown web search engine in database: {other}"
                )));
            }
        })
    }

    /// Colonne du secret (clé ou adresse) ; aucune pour DuckDuckGo.
    fn column(self) -> Option<&'static str> {
        match self {
            Engine::DuckDuckGo => None,
            Engine::Brave => Some("brave_key"),
            Engine::Mojeek => Some("mojeek_key"),
            Engine::Searxng => Some("searxng_url"),
        }
    }
}

/// Réglage complet, secrets compris (jamais vers la page).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Saved {
    pub engine: Option<Engine>,
    pub brave_key: String,
    pub mojeek_key: String,
    pub searxng_url: String,
}

/// Jamais une clé dans un journal.
impl std::fmt::Debug for Saved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Saved")
            .field("engine", &self.engine)
            .field("brave", &!self.brave_key.is_empty())
            .field("mojeek", &!self.mojeek_key.is_empty())
            .field("searxng", &!self.searxng_url.is_empty())
            .finish()
    }
}

impl Saved {
    /// Moteur personnel à essayer avant DuckDuckGo : le moteur choisi, s'il
    /// a sa clé ou son adresse.
    pub fn personal(&self) -> Option<web_search::Personal> {
        let personal = match self.engine? {
            Engine::DuckDuckGo => return None,
            Engine::Brave => web_search::Personal::Brave {
                key: self.brave_key.clone(),
            },
            Engine::Mojeek => web_search::Personal::Mojeek {
                key: self.mojeek_key.clone(),
            },
            Engine::Searxng => web_search::Personal::Searxng {
                url: self.searxng_url.clone(),
            },
        };
        let secret = match &personal {
            web_search::Personal::Brave { key } | web_search::Personal::Mojeek { key } => key,
            web_search::Personal::Searxng { url } => url,
        };
        (!secret.is_empty()).then_some(personal)
    }

    /// Ce que la page en voit.
    pub fn choice(&self) -> EngineChoice {
        EngineChoice {
            engine: self.engine.unwrap_or(Engine::DuckDuckGo),
            brave_configured: !self.brave_key.is_empty(),
            mojeek_configured: !self.mojeek_key.is_empty(),
            searxng_configured: !self.searxng_url.is_empty(),
        }
    }
}

/// Moteur choisi et secrets configurés (`web_search_engine`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "WebSearchEngineChoice")]
pub struct EngineChoice {
    pub engine: Engine,
    pub brave_configured: bool,
    pub mojeek_configured: bool,
    pub searxng_configured: bool,
}

pub fn load(connection: &Connection) -> AppResult<Saved> {
    let (engine, brave_key, mojeek_key, searxng_url): (String, String, String, String) = connection
        .query_row(
            "SELECT engine, brave_key, mojeek_key, searxng_url FROM web_search_engine WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
    Ok(Saved {
        engine: Some(Engine::parse(&engine)?),
        brave_key,
        mojeek_key,
        searxng_url,
    })
}

/// Choisit le moteur ; `secret` (clé ou adresse, déjà vérifiée) remplace
/// celui de ce moteur s'il est donné.
pub fn choose(connection: &Connection, engine: Engine, secret: Option<&str>) -> AppResult<()> {
    connection.execute(
        "UPDATE web_search_engine SET engine = ?1 WHERE id = 1",
        params![engine.as_str()],
    )?;
    if let (Some(column), Some(secret)) = (engine.column(), secret) {
        connection.execute(
            &format!("UPDATE web_search_engine SET {column} = ?1 WHERE id = 1"),
            params![secret],
        )?;
    }
    Ok(())
}

/// Oublie la clé (ou l'adresse) d'un moteur ; s'il était choisi, DuckDuckGo
/// le remplace.
pub fn forget(connection: &mut Connection, engine: Engine) -> AppResult<()> {
    let Some(column) = engine.column() else {
        return Ok(());
    };
    // La clé effacée ne doit pas rester lisible dans le fichier : ses octets
    // sont écrasés (comme le jeton d'un bot d'Easy Link).
    connection.pragma_update(None, "secure_delete", true)?;
    let transaction = connection.transaction()?;
    transaction.execute(
        &format!("UPDATE web_search_engine SET {column} = '' WHERE id = 1"),
        [],
    )?;
    transaction.execute(
        "UPDATE web_search_engine SET engine = 'duckDuckGo' WHERE id = 1 AND engine = ?1",
        params![engine.as_str()],
    )?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn duckduckgo_by_default_then_a_personal_engine_once_configured() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                let saved = load(c)?;
                assert_eq!(saved.engine, Some(Engine::DuckDuckGo));
                assert_eq!(saved.personal(), None);
                // Brave choisi sans clé : toujours DuckDuckGo seul.
                choose(c, Engine::Brave, None)?;
                assert_eq!(load(c)?.personal(), None);
                choose(c, Engine::Brave, Some("BSA-1"))?;
                assert_eq!(
                    load(c)?.personal(),
                    Some(web_search::Personal::Brave {
                        key: "BSA-1".into()
                    })
                );
                // Changer de moteur garde la clé de l'autre.
                choose(c, Engine::Searxng, Some("http://localhost:8080"))?;
                let saved = load(c)?;
                assert_eq!(
                    saved.choice(),
                    EngineChoice {
                        engine: Engine::Searxng,
                        brave_configured: true,
                        mojeek_configured: false,
                        searxng_configured: true,
                    }
                );
                assert!(!format!("{saved:?}").contains("BSA-1"));
                // Oublier l'adresse du moteur choisi ramène à DuckDuckGo ;
                // oublier une autre clé ne change pas le choix.
                choose(c, Engine::Brave, None)?;
                forget(c, Engine::Mojeek)?;
                assert_eq!(load(c)?.engine, Some(Engine::Brave));
                forget(c, Engine::Brave)?;
                let saved = load(c)?;
                assert_eq!(saved.engine, Some(Engine::DuckDuckGo));
                assert!(saved.brave_key.is_empty());
                assert_eq!(saved.searxng_url, "http://localhost:8080");
                Ok(())
            })
            .unwrap();
    }
}
