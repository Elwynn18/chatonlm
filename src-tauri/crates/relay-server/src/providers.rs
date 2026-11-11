//! Fournisseurs de recherche, d'après leur documentation officielle (relevée
//! le 8 nov. 2026, jamais essayés contre le vrai service faute de clé : banc
//! R0) :
//!
//! - **Brave Search API** : `GET https://api.search.brave.com/res/v1/web/search`,
//!   clé dans l'en-tête `X-Subscription-Token` ; paramètres `q`, `count`
//!   (20 au plus), `country` (2 lettres), `search_lang`, `safesearch`
//!   (`moderate` par défaut), `text_decorations` (marques `<strong>`),
//!   `result_filter`, `extra_snippets` ; réponse `web.results[]` `{title, url,
//!   description, extra_snippets?}`. 429 : trop de requêtes.
//! - **Mojeek** : `GET https://api.mojeek.com/search?q=…&api_key=…&fmt=json`,
//!   `t` (nombre de résultats), `lb`/`lbb` (langue favorisée, majuscules),
//!   `rb`/`rbb` (région), `safe` ; réponse `response.status` (`OK`, sinon
//!   `ERROR: …`, dont « Daily Limit Reached »), `response.results[]` `{url,
//!   title, desc}`.
//!
//! Analyseurs (`parse_brave`, `parse_mojeek`) et mise en forme (`shape`,
//! `clean`, `brave_lang`) : crate `web-search`, commun avec la recherche
//! locale de l'app ; seuls les clients et les sortes d'échec du relais sont
//! ici.
//!
//! Secrets : la clé de Mojeek est dans l'adresse. Aucune erreur de `reqwest`
//! n'est donc gardée telle quelle (elle contient l'adresse) : seule sa sorte
//! ([`ProviderError`]) sort d'ici, sans texte.

use crate::budget::micro_eur;
use crate::config::{ProviderConfig, Secret};
use relay_proto::api::{Provider, Search, SearchResult};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Taille lue d'une réponse de fournisseur.
const MAX_BODY: usize = 2 * 1024 * 1024;

/// Échec d'un fournisseur, sans détail (ni adresse, ni corps).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderError {
    /// Injoignable (requête jamais partie), 5xx.
    Unavailable,
    /// Requête partie, réponse jamais reçue en entier (délai, coupure) : le
    /// fournisseur a pu la facturer.
    Lost,
    /// 429.
    RateLimited,
    /// 401, 403 : clé refusée.
    Auth,
    /// Crédit ou limite du fournisseur épuisé (402, « Daily Limit »).
    Exhausted,
    /// 4xx autre : requête refusée.
    Rejected,
    /// Réponse illisible.
    Malformed,
}

impl ProviderError {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderError::Unavailable => "unavailable",
            ProviderError::Lost => "no reply",
            ProviderError::RateLimited => "rate limited",
            ProviderError::Auth => "key refused",
            ProviderError::Exhausted => "credit exhausted",
            ProviderError::Rejected => "request rejected",
            ProviderError::Malformed => "malformed reply",
        }
    }

    /// Délai d'évitement après cet échec (une clé refusée ne revient pas
    /// toute seule : plus long).
    fn cooldown(self, base: u64) -> u64 {
        match self {
            ProviderError::Auth | ProviderError::Exhausted => base.saturating_mul(20),
            ProviderError::Rejected | ProviderError::Malformed => 0,
            ProviderError::Unavailable | ProviderError::Lost | ProviderError::RateLimited => base,
        }
    }

    /// L'appel a-t-il pu être facturé ? Une réponse reçue mais illisible, ou
    /// une requête partie sans réponse (délai dépassé, coupure) l'a peut-être
    /// été : gardée au budget, par prudence. Un refus, une erreur du
    /// fournisseur ou une connexion impossible non.
    pub fn billed(self) -> bool {
        matches!(self, ProviderError::Malformed | ProviderError::Lost)
    }
}

pub struct ProviderClient {
    pub provider: Provider,
    base_url: String,
    key: Secret,
    pub cost: u64,
    extra_snippets: bool,
    cooldown_secs: u64,
    down_until: AtomicU64,
    http: reqwest::Client,
}

impl ProviderClient {
    pub fn new(provider: Provider, config: &ProviderConfig, key: Secret) -> Result<Self, String> {
        let timeout = Duration::from_secs(config.timeout_secs.max(1));
        let http = reqwest::Client::builder()
            .timeout(timeout)
            // Connexion bornée avant la fin du délai total : une connexion
            // qui traîne (hôte muet, port fermé sous Windows qui répond par
            // un délai) finit en erreur de connexion, donc non facturée,
            // plutôt qu'en délai dépassé, compté comme une requête partie.
            .connect_timeout(Duration::from_secs(5).min(timeout / 2))
            .gzip(true)
            // Aucune redirection : une réponse 3xx n'est pas suivie (la clé
            // de Brave est dans un en-tête, celle de Mojeek dans l'adresse).
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("chatonlm-relay/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| format!("{}: cannot build the HTTP client", provider.as_str()))?;
        Ok(Self {
            provider,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
            key,
            cost: micro_eur(config.cost_per_1000_eur / 1000.0),
            extra_snippets: config.extra_snippets,
            cooldown_secs: config.cooldown_secs,
            down_until: AtomicU64::new(0),
            http,
        })
    }

    pub fn is_configured(&self) -> bool {
        !self.key.is_empty()
    }

    pub fn is_down(&self, now: u64) -> bool {
        self.down_until.load(Ordering::Relaxed) > now
    }

    pub fn mark_failure(&self, error: ProviderError, now: u64) {
        let cooldown = error.cooldown(self.cooldown_secs);
        if cooldown > 0 {
            self.down_until.store(now + cooldown, Ordering::Relaxed);
        }
    }

    pub async fn search(&self, search: &Search) -> Result<Vec<SearchResult>, ProviderError> {
        Ok(shape(
            self.search_raw(search).await?,
            usize::from(search.max_results),
        ))
    }

    /// Résultats tels que le fournisseur les donne (extraits séparés), avant
    /// [`shape`] : le banc R0 compte les extraits supplémentaires de Brave.
    pub async fn search_raw(&self, search: &Search) -> Result<Vec<RawResult>, ProviderError> {
        let request = match self.provider {
            Provider::Brave => self.brave_request(search),
            Provider::Mojeek => self.mojeek_request(search),
        }?;
        let body = send(request).await?;
        let value: Value = serde_json::from_slice(&body).map_err(|_| ProviderError::Malformed)?;
        match self.provider {
            Provider::Brave => parse_brave(&value),
            Provider::Mojeek => parse_mojeek(&value),
        }
    }

    fn brave_request(&self, search: &Search) -> Result<reqwest::RequestBuilder, ProviderError> {
        let mut url = url::Url::parse(&format!("{}/res/v1/web/search", self.base_url))
            .map_err(|_| ProviderError::Rejected)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("q", &search.query);
            query.append_pair("count", &search.max_results.to_string());
            query.append_pair("safesearch", "moderate");
            query.append_pair("text_decorations", "false");
            query.append_pair("result_filter", "web");
            if let Some(lang) = search.lang.as_deref().and_then(brave_lang) {
                query.append_pair("search_lang", lang);
            }
            if let Some(country) = &search.country {
                query.append_pair("country", country);
            }
            if self.extra_snippets {
                query.append_pair("extra_snippets", "true");
            }
        }
        Ok(self
            .http
            .get(url)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", self.key.expose()))
    }

    fn mojeek_request(&self, search: &Search) -> Result<reqwest::RequestBuilder, ProviderError> {
        let mut url = url::Url::parse(&format!("{}/search", self.base_url))
            .map_err(|_| ProviderError::Rejected)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("q", &search.query);
            query.append_pair("api_key", self.key.expose());
            query.append_pair("fmt", "json");
            query.append_pair("t", &search.max_results.to_string());
            query.append_pair("safe", "1");
            if let Some(lang) = &search.lang {
                query.append_pair("lb", &lang.to_ascii_uppercase());
                query.append_pair("lbb", "100");
            }
            if let Some(country) = &search.country {
                query.append_pair("rb", country);
                query.append_pair("rbb", "10");
            }
        }
        Ok(self.http.get(url).header("Accept", "application/json"))
    }
}

/// Envoie la requête et rend le corps d'une réponse 200, sinon la sorte
/// d'échec (rien n'est lu de l'erreur de `reqwest` : elle contient l'adresse).
pub(crate) async fn send(request: reqwest::RequestBuilder) -> Result<Vec<u8>, ProviderError> {
    // Seule une erreur de connexion dit que la requête n'est pas partie.
    let response = request.send().await.map_err(|error| {
        if error.is_connect() || error.is_builder() {
            ProviderError::Unavailable
        } else {
            ProviderError::Lost
        }
    })?;
    let status = response.status().as_u16();
    let body = read_limited(response).await?;
    match status {
        200 => Ok(body),
        401 | 403 => Err(ProviderError::Auth),
        402 => Err(ProviderError::Exhausted),
        429 => Err(ProviderError::RateLimited),
        400..=499 => Err(ProviderError::Rejected),
        _ => Err(ProviderError::Unavailable),
    }
}

async fn read_limited(mut response: reqwest::Response) -> Result<Vec<u8>, ProviderError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ProviderError::Lost)? {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(ProviderError::Malformed);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub use web_search::RawResult;
pub use web_search::brave::brave_lang;
pub use web_search::text::clean;

/// Échec d'un analyseur du crate `web-search`, dans les sortes du relais.
fn from_engine(error: web_search::EngineError) -> ProviderError {
    use web_search::EngineError as E;
    match error {
        E::Exhausted => ProviderError::Exhausted,
        E::Auth => ProviderError::Auth,
        E::Unavailable => ProviderError::Unavailable,
        _ => ProviderError::Malformed,
    }
}

pub fn parse_brave(value: &Value) -> Result<Vec<RawResult>, ProviderError> {
    web_search::brave::parse_brave(value).map_err(from_engine)
}

pub fn parse_mojeek(value: &Value) -> Result<Vec<RawResult>, ProviderError> {
    web_search::mojeek::parse_mojeek(value).map_err(from_engine)
}

/// Réponse réduite à ce dont l'app a besoin ([`web_search::shape`]), au
/// format du protocole du relais.
pub fn shape(raw: Vec<RawResult>, max: usize) -> Vec<SearchResult> {
    web_search::shape(raw, max)
        .into_iter()
        .map(|result| SearchResult {
            title: result.title,
            url: result.url,
            content: result.content,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Analyseurs et mise en forme du crate `web-search` (essayés là-bas) :
    /// rendus dans les sortes et le format du relais.
    #[test]
    fn parsers_come_from_the_web_search_crate() {
        let value = json!({"web": {"results": [
            {"title": "Le <strong>chat</strong>", "url": "https://fr.wikipedia.org/wiki/Chat",
             "description": "Le chat &amp; ses moustaches", "extra_snippets": ["Félin."]}
        ]}});
        let shaped = shape(parse_brave(&value).unwrap(), 5);
        assert_eq!(
            shaped,
            [SearchResult {
                title: "Le chat".into(),
                url: "https://fr.wikipedia.org/wiki/Chat".into(),
                content: "Le chat & ses moustaches … Félin.".into(),
            }]
        );
        assert_eq!(parse_brave(&json!([1])), Err(ProviderError::Malformed));
        let limit = json!({"response": {"status": "ERROR: Daily Limit Reached."}});
        assert_eq!(parse_mojeek(&limit), Err(ProviderError::Exhausted));
        let key = json!({"response": {"status": "ERROR: Invalid key"}});
        assert_eq!(parse_mojeek(&key), Err(ProviderError::Auth));
        let other = json!({"response": {"status": "ERROR: Internal error"}});
        assert_eq!(parse_mojeek(&other), Err(ProviderError::Unavailable));
        assert_eq!(brave_lang("ja"), Some("jp"));
        assert_eq!(clean("a <b>b</b>"), "a b");
    }

    /// M5 : seule une requête jamais partie est rendue au budget.
    #[test]
    fn what_is_billed() {
        assert!(ProviderError::Lost.billed() && ProviderError::Malformed.billed());
        for error in [
            ProviderError::Unavailable,
            ProviderError::RateLimited,
            ProviderError::Auth,
            ProviderError::Exhausted,
            ProviderError::Rejected,
        ] {
            assert!(!error.billed(), "{error:?}");
        }
    }
}
