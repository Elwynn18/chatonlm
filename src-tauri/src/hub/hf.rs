//! Requêtes à Hugging Face (35 § 4, R3 ; contrat § 3) : client commun,
//! classement des échecs, quota partagé après une réponse 429, page d'état.

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use reqwest::header::{self, HeaderMap};
use reqwest::{Client, Response, StatusCode};
use url::Url;

use super::types::HubProblem;
use crate::i18n::t;
use crate::ollama::client::is_local;

/// Connexion.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Page d'état.
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);
/// Attente après une 429 sans indication.
const DEFAULT_RETRY: u32 = 60;
/// Attente maximale retenue (une heure).
const MAX_RETRY: u32 = 3600;
/// Réponse de la page d'état gardée.
const STATUS_KEEP: Duration = Duration::from_secs(60);
/// Taille lue au plus de la page d'état (environ 80 Ko).
const STATUS_MAX: usize = 1024 * 1024;

/// Échec d'une requête, avant la consultation de la page d'état.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Aucune réponse (DNS, connexion refusée, TLS, connexion impossible).
    Offline,
    /// Délai dépassé, 5xx, corps illisible.
    Unavailable,
    /// 429 : attendre ce nombre de secondes.
    RateLimited(u32),
    /// 401 sans code : dépôt inexistant ou privé.
    NotFound,
    /// Dépôt à accès restreint (`x-error-code: GatedRepo`).
    Restricted,
    /// 404 (`EntryNotFound` pour un fichier absent).
    Missing,
    /// Autre 4xx : message de Hugging Face.
    Failed(String),
}

/// Réponse lue.
#[derive(Debug)]
pub enum Reply {
    Body {
        bytes: Vec<u8>,
        etag: Option<String>,
        /// Curseur de la page suivante (en-tête `Link`).
        next_cursor: Option<String>,
    },
    /// 304 : la copie en cache est à jour.
    NotModified,
}

/// Contenu attendu d'une réponse : un autre (page HTML d'un CDN...) vaut
/// `Unavailable`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    Json,
    /// Texte, pas une page HTML.
    Text,
    Bytes,
}

/// Requête à Hugging Face.
pub struct Request<'a> {
    /// Chemin et requête, commençant par `/`.
    pub path: &'a str,
    pub timeout: Duration,
    /// ETag d'une copie en cache (`If-None-Match`).
    pub etag: Option<&'a str>,
    /// Octets lus au plus.
    pub max_bytes: usize,
    /// Au-delà de `max_bytes`, garder le début (sinon : échec).
    pub truncate: bool,
    /// Contenu attendu.
    pub expect: Expect,
    /// En-tête `Range`.
    pub range: Option<&'a str>,
}

pub struct Hf {
    base: String,
    status: String,
    api: Option<Client>,
    status_client: Option<Client>,
    /// Pas de requête avant cette échéance (429).
    quota: Mutex<Option<Instant>>,
    /// Dernière lecture de la page d'état : (moment, incident). Gardé pendant
    /// la lecture : deux échecs simultanés ne la lisent qu'une fois.
    incident: tokio::sync::Mutex<Option<(Instant, bool)>>,
}

/// Installe une fois le moteur cryptographique de rustls (ring), comme le
/// client d'Ollama.
fn ensure_crypto_provider() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Client pour une adresse : gzip, agent ChatonLM, proxy du système sauf vers
/// la machine locale.
fn client_for(base: &str) -> Option<Client> {
    ensure_crypto_provider();
    let url = Url::parse(base).ok()?;
    let mut builder = Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .gzip(true)
        .user_agent(concat!("ChatonLM/", env!("CARGO_PKG_VERSION")));
    if is_local(&url) {
        builder = builder.no_proxy();
    }
    builder.build().ok()
}

impl Hf {
    /// `base` : API, `raw/`, `resolve/` ; `status` : page d'état. Sans `/`
    /// final.
    pub fn new(base: String, status: String) -> Self {
        Self {
            api: client_for(&base),
            status_client: client_for(&status),
            base,
            status,
            quota: Mutex::new(None),
            incident: tokio::sync::Mutex::new(None),
        }
    }

    /// Secondes restantes avant la fin d'une limite de requêtes (0 : aucune).
    fn waiting(&self) -> u32 {
        let mut quota = self.quota.lock();
        match *quota {
            Some(until) if until > Instant::now() => {
                let left = until - Instant::now();
                (left.as_millis().div_ceil(1000) as u32).max(1)
            }
            Some(_) => {
                *quota = None;
                0
            }
            None => 0,
        }
    }

    /// GET d'une ressource de Hugging Face. Aucune requête pendant une limite
    /// de requêtes (429) : `RateLimited` avec le temps restant.
    pub async fn get(&self, request: Request<'_>) -> Result<Reply, Failure> {
        let waiting = self.waiting();
        if waiting > 0 {
            return Err(Failure::RateLimited(waiting));
        }
        let Some(client) = &self.api else {
            return Err(Failure::Failed("invalid Hugging Face address".into()));
        };
        let mut builder = client
            .get(format!("{}{}", self.base, request.path))
            .timeout(request.timeout);
        if let Some(etag) = request.etag {
            builder = builder.header(header::IF_NONE_MATCH, etag);
        }
        if let Some(range) = request.range {
            builder = builder.header(header::RANGE, range);
        }
        let response = builder
            .send()
            .await
            .map_err(|error| before_answer(&error))?;
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry = retry_after(response.headers());
            *self.quota.lock() = Some(Instant::now() + Duration::from_secs(u64::from(retry)));
            return Err(Failure::RateLimited(retry));
        }
        if status == StatusCode::NOT_MODIFIED {
            return Ok(Reply::NotModified);
        }
        if !status.is_success() {
            return Err(failure_of(response).await);
        }
        let unexpected = match request.expect {
            Expect::Json => !is_json(response.headers()),
            Expect::Text => is_html(response.headers()),
            Expect::Bytes => false,
        };
        if unexpected {
            return Err(Failure::Unavailable);
        }
        let etag = text_header(response.headers(), header::ETAG);
        let next_cursor = text_header(response.headers(), header::LINK)
            .as_deref()
            .and_then(next_cursor);
        let bytes = read(response, request.max_bytes, request.truncate).await?;
        Ok(Reply::Body {
            bytes,
            etag,
            next_cursor,
        })
    }

    /// Problème montré pour un échec : `Unavailable` consulte la page d'état
    /// (gardée une minute).
    pub async fn problem(&self, failure: Failure) -> HubProblem {
        match failure {
            Failure::Offline => HubProblem::Offline,
            Failure::Unavailable => HubProblem::Unavailable {
                incident: self.incident().await,
            },
            Failure::RateLimited(retry_after) => HubProblem::RateLimited { retry_after },
            Failure::NotFound => HubProblem::NotFound,
            Failure::Restricted => HubProblem::Restricted,
            Failure::Missing => HubProblem::Failed {
                message: t!("downloads.hfStatus", status = 404),
            },
            Failure::Failed(message) => HubProblem::Failed { message },
        }
    }

    /// La page d'état signale-t-elle un incident ? Faux si elle est illisible.
    async fn incident(&self) -> bool {
        let mut last = self.incident.lock().await;
        if let Some((at, incident)) = *last
            && at.elapsed() < STATUS_KEEP
        {
            return incident;
        }
        let incident = self.read_status().await.unwrap_or(false);
        *last = Some((Instant::now(), incident));
        incident
    }

    async fn read_status(&self) -> Option<bool> {
        let client = self.status_client.as_ref()?;
        let response = client
            .get(format!("{}/index.json", self.status))
            .timeout(STATUS_TIMEOUT)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() || !is_json(response.headers()) {
            return None;
        }
        let bytes = read(response, STATUS_MAX, false).await.ok()?;
        incident_in(&serde_json::from_slice(&bytes).ok()?)
    }
}

/// Incident d'après `index.json` (Better Stack) : état global non
/// « operational », ou composant « Huggingface Hub » non « operational ».
fn incident_in(page: &serde_json::Value) -> Option<bool> {
    let state = page["data"]["attributes"]["aggregate_state"].as_str()?;
    let hub_down = page["included"].as_array().is_some_and(|parts| {
        parts.iter().any(|part| {
            let attributes = &part["attributes"];
            attributes["public_name"].as_str() == Some("Huggingface Hub")
                && attributes["status"]
                    .as_str()
                    .is_some_and(|status| status != "operational")
        })
    });
    Some(state != "operational" || hub_down)
}

/// Erreur avant toute réponse : délai dépassé (connexion établie) →
/// `Unavailable`, sinon `Offline`.
fn before_answer(error: &reqwest::Error) -> Failure {
    if error.is_timeout() && !error.is_connect() {
        Failure::Unavailable
    } else if error.is_connect() || error.is_request() {
        Failure::Offline
    } else {
        Failure::Unavailable
    }
}

/// Classement d'une réponse en erreur.
async fn failure_of(response: Response) -> Failure {
    let status = response.status();
    let code = text_header(response.headers(), "x-error-code");
    if code.as_deref() == Some("GatedRepo") {
        return Failure::Restricted;
    }
    if status.is_server_error() {
        return Failure::Unavailable;
    }
    match status {
        StatusCode::UNAUTHORIZED => Failure::NotFound,
        StatusCode::NOT_FOUND => Failure::Missing,
        status if status.is_client_error() => {
            let body = read(response, 16 * 1024, true).await.unwrap_or_default();
            Failure::Failed(error_message(&body, status))
        }
        // Redirection non suivie, autre statut inattendu.
        _ => Failure::Unavailable,
    }
}

/// Message d'une erreur de Hugging Face : `{"error": "..."}`, sinon le texte,
/// sinon le statut.
fn error_message(body: &[u8], status: StatusCode) -> String {
    let from_json = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value["error"].as_str().map(str::to_owned));
    let message = from_json.unwrap_or_else(|| String::from_utf8_lossy(body).trim().to_owned());
    if message.is_empty() || message.starts_with('<') {
        return t!("downloads.hfStatus", status = status.as_u16());
    }
    message.chars().take(300).collect()
}

fn text_header(headers: &HeaderMap, name: impl header::AsHeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn is_json(headers: &HeaderMap) -> bool {
    text_header(headers, header::CONTENT_TYPE)
        .is_some_and(|kind| kind.to_ascii_lowercase().contains("json"))
}

fn is_html(headers: &HeaderMap) -> bool {
    text_header(headers, header::CONTENT_TYPE)
        .is_some_and(|kind| kind.to_ascii_lowercase().contains("html"))
}

/// Attente d'une 429 : `t=` de `RateLimit`, sinon `Retry-After` (secondes),
/// sinon 60 s ; entre 1 s et une heure.
fn retry_after(headers: &HeaderMap) -> u32 {
    let from_ratelimit = text_header(headers, "ratelimit").and_then(|value| {
        value
            .split(';')
            .find_map(|part| part.trim().strip_prefix("t=")?.trim().parse::<u32>().ok())
    });
    let from_retry_after =
        || text_header(headers, header::RETRY_AFTER).and_then(|value| value.parse::<u32>().ok());
    from_ratelimit
        .or_else(from_retry_after)
        .unwrap_or(DEFAULT_RETRY)
        .clamp(1, MAX_RETRY)
}

/// Curseur `cursor` du lien `rel="next"` d'un en-tête `Link`.
fn next_cursor(link: &str) -> Option<String> {
    link.split(',').find_map(|part| {
        let (target, params) = part.split_once(';')?;
        if !params.split(';').any(|p| p.trim() == "rel=\"next\"") {
            return None;
        }
        let target = target.trim().strip_prefix('<')?.strip_suffix('>')?;
        let url = Url::parse(target).ok()?;
        url.query_pairs()
            .find(|(name, _)| name == "cursor")
            .map(|(_, value)| value.into_owned())
            .filter(|value| !value.is_empty())
    })
}

/// Corps lu par morceaux, `max` octets au plus : au-delà, garder le début
/// (`truncate`) ou échouer. Une coupure en route → `Unavailable`.
async fn read(mut response: Response, max: usize, truncate: bool) -> Result<Vec<u8>, Failure> {
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let room = max - body.len();
                if chunk.len() > room {
                    if truncate {
                        body.extend_from_slice(&chunk[..room]);
                        return Ok(body);
                    }
                    return Err(Failure::Unavailable);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok(body),
            Err(_) => return Err(Failure::Unavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use reqwest::header::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn the_wait_comes_from_ratelimit_then_retry_after() {
        assert_eq!(
            retry_after(&headers(&[("ratelimit", "\"api\";r=0;t=42")])),
            42
        );
        assert_eq!(
            retry_after(&headers(&[
                ("ratelimit", "\"api\";r=0"),
                ("retry-after", "17")
            ])),
            17
        );
        assert_eq!(retry_after(&headers(&[("retry-after", "soon")])), 60);
        assert_eq!(retry_after(&HeaderMap::new()), 60);
        assert_eq!(
            retry_after(&headers(&[("ratelimit", "\"api\";r=0;t=0")])),
            1
        );
        assert_eq!(
            retry_after(&headers(&[("ratelimit", "\"api\";r=0;t=99999")])),
            3600
        );
    }

    #[test]
    fn the_next_cursor_is_read_from_the_link() {
        let link = "<https://huggingface.co/api/models?apps=ollama&limit=30&cursor=eyIkb3Ii%3D%3D>; rel=\"next\"";
        assert_eq!(next_cursor(link).as_deref(), Some("eyIkb3Ii=="));
        assert_eq!(next_cursor("<https://x/?cursor=a>; rel=\"prev\""), None);
        assert_eq!(next_cursor("<https://x/?page=2>; rel=\"next\""), None);
        assert_eq!(next_cursor("garbage"), None);
    }

    #[test]
    fn the_status_page_tells_incidents() {
        let page = |state: &str, hub: &str| {
            serde_json::json!({
                "data": { "attributes": { "aggregate_state": state } },
                "included": [
                    { "attributes": { "public_name": "AWS CDN", "status": "degraded" } },
                    { "attributes": { "public_name": "Huggingface Hub", "status": hub } }
                ]
            })
        };
        assert_eq!(
            incident_in(&page("operational", "operational")),
            Some(false)
        );
        assert_eq!(incident_in(&page("downtime", "operational")), Some(true));
        assert_eq!(incident_in(&page("operational", "degraded")), Some(true));
        assert_eq!(incident_in(&serde_json::json!({})), None);
        let real: serde_json::Value = serde_json::from_str(include_str!(
            "../../crates/fake-ollama/fixtures/hf/status-index.json"
        ))
        .unwrap();
        assert_eq!(incident_in(&real), Some(false));
    }

    #[test]
    fn error_messages_are_short_and_readable() {
        assert_eq!(
            error_message(
                br#"{"error":"Invalid sort parameter: x"}"#,
                StatusCode::BAD_REQUEST
            ),
            "Invalid sort parameter: x"
        );
        assert_eq!(
            error_message(b"Entry not found", StatusCode::NOT_FOUND),
            "Entry not found"
        );
        assert_eq!(
            error_message(b"<html>", StatusCode::FORBIDDEN),
            "Hugging Face answered with status 403."
        );
        assert_eq!(
            error_message(&[b'x'; 1000], StatusCode::BAD_REQUEST).len(),
            300
        );
    }
}
