//! Fournisseurs des modèles distants pour la page (Settings › Models) :
//! liste, ajout ou changement, retrait, vérification de la clé. La clé entre
//! par [`RemoteProviderInput`] et ne ressort jamais : la page n'en reçoit que
//! l'indice.

use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::catalog::{self, Problem};
use super::keys::{self, Keys};
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::inference::Dialect;
use crate::store::remote::{self as rows, ProviderRow, RemoteProviderKind};
use crate::store::{Store, now_ms};

/// Nom d'un fournisseur, au plus (caractères).
const MAX_NAME: usize = 60;
/// Clé d'API, au plus (caractères).
const MAX_KEY: usize = 512;
/// Adresse d'un serveur « custom », au plus.
const MAX_URL: usize = 300;

/// Fournisseur pour la page (jamais la clé).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteProvider {
    /// Identifiant court, repris dans `@remote/<id>/<modèle>`.
    pub id: String,
    pub kind: RemoteProviderKind,
    pub name: String,
    /// Adresse d'un serveur « custom » ; `null` pour les autres.
    pub base_url: Option<String>,
    pub enabled: bool,
    /// Une clé est enregistrée.
    pub has_key: bool,
    /// Ses quatre derniers caractères (clé d'au moins huit caractères).
    pub key_hint: Option<String>,
    /// ms depuis 1970.
    pub created_at: i64,
    pub updated_at: i64,
}

/// Fournisseur à ajouter (`id` absent) ou à changer.
#[derive(Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteProviderInput {
    /// Fournisseur à changer ; absent : un nouveau.
    pub id: Option<String>,
    /// Sorte (ignorée pour un fournisseur existant : elle ne change pas).
    pub kind: RemoteProviderKind,
    /// Nom ; vide : celui de la sorte (ou l'hôte d'un serveur « custom »).
    pub name: String,
    /// Adresse d'un serveur « custom » (`https://…`, `/v1` facultatif).
    pub base_url: Option<String>,
    /// Nouvelle clé ; absente : garder celle enregistrée (sauf si l'adresse
    /// d'un serveur « custom » change : elle est alors retirée, à ressaisir) ;
    /// vide : la retirer.
    pub api_key: Option<String>,
    pub enabled: bool,
}

impl std::fmt::Debug for RemoteProviderInput {
    /// Sans la clé (journaux, messages d'essai).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteProviderInput")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<hidden>"))
            .field("enabled", &self.enabled)
            .finish()
    }
}

/// Résultat d'une vérification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteCheck {
    pub ok: bool,
    /// Ce qui ne va pas ; `null` si tout va bien.
    pub problem: Option<RemoteProblem>,
    /// Code HTTP d'une erreur du serveur (`problem` = `server`).
    pub status: Option<u16>,
    /// Modèles de conversation trouvés.
    pub models: u32,
}

/// Problème d'un fournisseur, traduit par la page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RemoteProblem {
    /// Aucune clé enregistrée.
    NoKey,
    /// Clé refusée (401, 403).
    Key,
    /// Limite atteinte (402, 429).
    Limit,
    /// Fournisseur injoignable.
    Unreachable,
    /// Rien à cette adresse (404).
    NotFound,
    /// Autre erreur du serveur.
    Server,
    /// Réponse illisible.
    Malformed,
}

impl From<Problem> for RemoteProblem {
    fn from(problem: Problem) -> Self {
        match problem {
            Problem::Key => RemoteProblem::Key,
            Problem::Limit => RemoteProblem::Limit,
            Problem::Unreachable => RemoteProblem::Unreachable,
            Problem::NotFound => RemoteProblem::NotFound,
            Problem::Server(_) => RemoteProblem::Server,
            Problem::Malformed => RemoteProblem::Malformed,
        }
    }
}

#[cfg(any(test, debug_assertions, feature = "e2e"))]
fn test_override() -> Option<String> {
    std::env::var("CHATONLM_REMOTE_URL")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(not(any(test, debug_assertions, feature = "e2e")))]
fn test_override() -> Option<String> {
    None
}

/// Adresse d'un fournisseur connu (les requêtes ajoutent `/v1/…`) ;
/// `CHATONLM_REMOTE_URL` la remplace dans les builds d'essai seulement.
pub fn known_base(kind: RemoteProviderKind) -> Option<String> {
    let base = match kind {
        RemoteProviderKind::OllamaCloud => "https://ollama.com",
        RemoteProviderKind::OpenRouter => "https://openrouter.ai/api",
        RemoteProviderKind::Mistral => "https://api.mistral.ai",
        RemoteProviderKind::Custom => return None,
    };
    Some(test_override().unwrap_or_else(|| base.to_owned()))
}

/// Adresse des requêtes d'une ligne.
pub fn base_of(row: &ProviderRow) -> Option<String> {
    match row.kind {
        RemoteProviderKind::Custom => row.base_url.clone(),
        kind => known_base(kind),
    }
}

/// Langue du fournisseur.
pub fn dialect_of(kind: RemoteProviderKind) -> Dialect {
    match kind {
        RemoteProviderKind::Mistral => Dialect::Mistral,
        _ => Dialect::OpenAi,
    }
}

/// Clé exigée (un serveur « custom », souvent local, peut s'en passer).
pub fn needs_key(kind: RemoteProviderKind) -> bool {
    kind != RemoteProviderKind::Custom
}

/// Identifiant d'un nouveau fournisseur de cette sorte (`[a-z0-9-]`).
fn id_base(kind: RemoteProviderKind) -> &'static str {
    match kind {
        RemoteProviderKind::OllamaCloud => "ollama",
        RemoteProviderKind::OpenRouter => "openrouter",
        RemoteProviderKind::Mistral => "mistral",
        RemoteProviderKind::Custom => "custom",
    }
}

/// Nom par défaut : celui du service (noms propres, non traduits), ou
/// l'hôte d'un serveur « custom ».
fn default_name(kind: RemoteProviderKind, base_url: Option<&str>) -> String {
    match kind {
        // i18n-ignore : nom d'un service.
        RemoteProviderKind::OllamaCloud => "Ollama Cloud".to_owned(),
        // i18n-ignore : nom d'un service.
        RemoteProviderKind::OpenRouter => "OpenRouter".to_owned(),
        // i18n-ignore : nom d'un service.
        RemoteProviderKind::Mistral => "Mistral".to_owned(),
        RemoteProviderKind::Custom => base_url
            .and_then(|url| url::Url::parse(url).ok())
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_else(|| id_base(kind).to_owned()),
    }
}

/// Adresse d'un serveur « custom » telle que l'app la garde : http(s), sans
/// identifiants, requête ni fragment, sans `/` ni `/v1` final ; `None` si
/// elle n'est pas utilisable.
pub fn normalize_url(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_URL {
        return None;
    }
    let url = url::Url::parse(text).ok()?;
    let usable = matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none();
    if !usable {
        return None;
    }
    let mut base = url.as_str().trim_end_matches('/').to_owned();
    if let Some(stripped) = base.strip_suffix("/v1") {
        base = stripped.to_owned();
    }
    Some(base)
}

/// Clé utilisable : caractères ASCII visibles, sans espace.
fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_KEY && key.bytes().all(|b| b.is_ascii_graphic())
}

fn page_entry(row: ProviderRow, keys: &Keys) -> RemoteProvider {
    let key = keys.get(&row.id);
    RemoteProvider {
        has_key: key.as_deref().is_some_and(|key| !key.is_empty()),
        key_hint: key.as_deref().and_then(keys::hint),
        id: row.id,
        kind: row.kind,
        name: row.name,
        base_url: row.base_url,
        enabled: row.enabled,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// Fournisseurs, pour la page.
pub async fn list(store: &Store, data_dir: &Path) -> AppResult<Vec<RemoteProvider>> {
    let keys = Keys::new(data_dir);
    let rows = store.call(|c| rows::list(c)).await?;
    Ok(rows.into_iter().map(|row| page_entry(row, &keys)).collect())
}

fn invalid(key: &str) -> AppError {
    AppError::Invalid(match key {
        "name" => t!("remote.invalidName"),
        "url" => t!("remote.invalidUrl"),
        _ => t!("remote.invalidKey"),
    })
}

fn storage(error: std::io::Error) -> AppError {
    // Ni le message du système ni le chemin (nom de l'utilisateur).
    super::note(&format!("key file not written ({:?})", error.kind()));
    AppError::System(t!("remote.keyNotSaved"))
}

/// Ajoute ou change un fournisseur ; la clé va dans son fichier, jamais en
/// base. Adresse d'un serveur « custom » changée sans nouvelle clé : la clé
/// est retirée (elle ne part jamais vers un autre serveur que celui pour
/// lequel elle a été donnée). Pour un fournisseur existant, la clé change
/// AVANT la ligne : un échec du fichier laisse l'ancienne adresse avec
/// l'ancienne clé.
pub async fn save(
    store: &Store,
    data_dir: &Path,
    input: RemoteProviderInput,
) -> AppResult<RemoteProvider> {
    let existing = match &input.id {
        Some(id) => {
            let id = id.clone();
            Some(
                store
                    .call(move |c| rows::get(c, &id))
                    .await?
                    .ok_or_else(|| AppError::NotFound(t!("remote.notFound")))?,
            )
        }
        None => None,
    };
    let kind = existing.as_ref().map_or(input.kind, |row| row.kind);
    let base_url = match kind {
        RemoteProviderKind::Custom => Some(
            input
                .base_url
                .as_deref()
                .and_then(normalize_url)
                .ok_or_else(|| invalid("url"))?,
        ),
        _ => None,
    };
    let name = input.name.trim();
    if name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(invalid("name"));
    }
    let name = if name.is_empty() {
        default_name(kind, base_url.as_deref())
    } else {
        name.to_owned()
    };
    let key = input.api_key.as_deref().map(str::trim);
    if let Some(key) = key
        && !key.is_empty()
        && !valid_key(key)
    {
        return Err(invalid("key"));
    }
    let keys = Keys::new(data_dir);
    let now = now_ms();
    let apply_key = |id: &str, key: Option<&str>| -> AppResult<()> {
        match key {
            Some("") => keys.remove(id).map_err(storage),
            Some(key) => keys.set(id, key).map_err(storage),
            None => Ok(()),
        }
    };
    let id = match existing {
        Some(row) => {
            let moved = row.base_url != base_url;
            apply_key(&row.id, if moved { key.or(Some("")) } else { key })?;
            let (id, name, url) = (row.id.clone(), name.clone(), base_url.clone());
            let enabled = input.enabled;
            store
                .call(move |c| rows::update(c, &id, &name, url.as_deref(), enabled, now))
                .await?;
            row.id
        }
        None => {
            let row = ProviderRow {
                id: String::new(),
                kind,
                name,
                base_url,
                enabled: input.enabled,
                created_at: now,
                updated_at: now,
            };
            let id = store
                .call(move |c| {
                    let row = ProviderRow {
                        id: rows::free_id(c, id_base(kind))?,
                        ..row
                    };
                    rows::insert(c, &row)?;
                    Ok(row.id)
                })
                .await?;
            apply_key(&id, key)?;
            id
        }
    };
    let saved = store
        .call(move |c| rows::get(c, &id))
        .await?
        .ok_or_else(|| AppError::NotFound(t!("remote.notFound")))?;
    Ok(page_entry(saved, &keys))
}

/// Retire un fournisseur et sa clé : la clé d'abord (en échec, rien n'est
/// retiré, à refaire), puis la ligne ; l'identifiant n'est jamais redonné.
pub async fn delete(store: &Store, data_dir: &Path, id: &str) -> AppResult<()> {
    Keys::new(data_dir).remove(id).map_err(storage)?;
    let key = id.to_owned();
    store.call(move |c| rows::delete(c, &key)).await?;
    Ok(())
}

/// Vérifie l'adresse et la clé d'un fournisseur (liste de ses modèles).
pub async fn check(store: &Store, data_dir: &Path, id: &str) -> AppResult<RemoteCheck> {
    let key = id.to_owned();
    let row = store
        .call(move |c| rows::get(c, &key))
        .await?
        .ok_or_else(|| AppError::NotFound(t!("remote.notFound")))?;
    let failed = |problem: RemoteProblem, status: Option<u16>| RemoteCheck {
        ok: false,
        problem: Some(problem),
        status,
        models: 0,
    };
    let secret = Keys::new(data_dir).get(id).unwrap_or_default();
    if secret.is_empty() && needs_key(row.kind) {
        return Ok(failed(RemoteProblem::NoKey, None));
    }
    let Some(base) = base_of(&row) else {
        return Ok(failed(RemoteProblem::NotFound, None));
    };
    Ok(match catalog::check(row.kind, &base, &secret).await {
        Ok(models) => RemoteCheck {
            ok: true,
            problem: None,
            status: None,
            models: u32::try_from(models.len()).unwrap_or(u32::MAX),
        },
        Err(problem) => {
            super::note(&format!("check of {}: {problem:?}", row.id));
            let status = match problem {
                Problem::Server(status) => Some(status),
                _ => None,
            };
            failed(problem.into(), status)
        }
    })
}
