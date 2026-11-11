//! Recherche web de ChatonLM, faite depuis l'ordinateur de l'utilisateur
//! (aucun serveur de ChatonLM entre lui et le moteur) :
//!
//! - **DuckDuckGo** par défaut, sans compte ni clé ([`duckduckgo`]) ;
//! - un moteur personnel en option : clé **Brave Search API** ([`brave`]),
//!   clé **Mojeek** ([`mojeek`]), ou instance **SearXNG** ([`searxng`]).
//!
//! Ordre ([`Client::search`]) : le moteur personnel d'abord ; s'il échoue,
//! DuckDuckGo. Rythme prudent vers DuckDuckGo : un intervalle minimal entre
//! deux requêtes (la suivante attend) et une pause après un blocage.
//! Résultats normalisés ([`text::shape`]) : titre, adresse, extrait.
//!
//! Secrets : la clé de Mojeek est dans l'adresse, celle de Brave dans un
//! en-tête. Aucune erreur de `reqwest` n'est gardée telle quelle (elle
//! contient l'adresse) : seule sa sorte ([`EngineError`]) sort d'ici, sans
//! texte. Aucune redirection n'est suivie vers Brave, Mojeek ni DuckDuckGo.
//!
//! Crate sans Tauri, commun à Linux et Windows. Relevé des points d'accès :
//! `.tools/research/104-recherche-locale.md`.

pub mod brave;
pub mod duckduckgo;
pub mod mojeek;
pub mod searxng;
pub mod text;

use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub use text::{RawResult, SearchResult, clean, shape};

/// Nombre de résultats sans demande précise, et au plus.
pub const DEFAULT_RESULTS: u8 = 5;
pub const MAX_RESULTS: u8 = 10;
/// Requête acceptée au plus (caractères, mots).
pub const MAX_QUERY_CHARS: usize = 400;
pub const MAX_QUERY_WORDS: usize = 50;

/// Corps de réponse lu au plus.
const MAX_BODY: usize = 2 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Intervalle minimal entre deux requêtes à DuckDuckGo.
pub const DUCKDUCKGO_GAP: Duration = Duration::from_millis(1_500);
/// Après un blocage de DuckDuckGo, aucune requête pendant ce délai.
pub const DUCKDUCKGO_PAUSE: Duration = Duration::from_secs(60);

/// Moteur qui a répondu (ou échoué).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provider {
    DuckDuckGo,
    Brave,
    Mojeek,
    Searxng,
}

impl Provider {
    /// Nom stable (enregistré avec les résultats : `brave` donne la mention
    /// « Powered by Brave Search »).
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::DuckDuckGo => "duckduckgo",
            Provider::Brave => "brave",
            Provider::Mojeek => "mojeek",
            Provider::Searxng => "searxng",
        }
    }
}

/// Moteur personnel configuré par l'utilisateur.
#[derive(Clone, PartialEq, Eq)]
pub enum Personal {
    Brave {
        key: String,
    },
    Mojeek {
        key: String,
    },
    /// Adresse de l'instance (normalisée par [`searxng::normalize`] à l'usage).
    Searxng {
        url: String,
    },
}

impl Personal {
    pub fn provider(&self) -> Provider {
        match self {
            Personal::Brave { .. } => Provider::Brave,
            Personal::Mojeek { .. } => Provider::Mojeek,
            Personal::Searxng { .. } => Provider::Searxng,
        }
    }
}

/// Jamais la clé dans un journal.
impl std::fmt::Debug for Personal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Personal").field(&self.provider()).finish()
    }
}

/// Une recherche.
#[derive(Clone, Copy, Debug)]
pub struct Query<'a> {
    pub text: &'a str,
    pub max_results: u8,
    /// Langue souhaitée des résultats (`fr`).
    pub lang: Option<&'a str>,
}

impl<'a> Query<'a> {
    /// Requête vérifiée : texte non vide, sans caractère de contrôle, borné ;
    /// nombre de résultats ramené entre 1 et [`MAX_RESULTS`].
    pub fn new(text: &'a str, max_results: Option<u8>, lang: Option<&'a str>) -> Option<Self> {
        let text = text.trim();
        if text.is_empty()
            || text.chars().count() > MAX_QUERY_CHARS
            || text.split_whitespace().count() > MAX_QUERY_WORDS
            || text.chars().any(char::is_control)
        {
            return None;
        }
        let max_results = max_results.unwrap_or(DEFAULT_RESULTS).clamp(1, MAX_RESULTS);
        let lang =
            lang.filter(|code| code.len() == 2 && code.bytes().all(|b| b.is_ascii_lowercase()));
        Some(Query {
            text,
            max_results,
            lang,
        })
    }
}

/// Échec d'un moteur, sans détail (ni adresse, ni corps).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineError {
    /// Connexion impossible (pas de réseau, nom inconnu, refus).
    Offline,
    /// Requête partie, réponse jamais reçue en entier (délai, coupure).
    Timeout,
    /// DuckDuckGo bloque les recherches de cet ordinateur (page « anomaly »,
    /// défi, 202) ou s'est mis en pause après un tel blocage.
    Blocked,
    /// 429.
    RateLimited,
    /// 401, 403 : clé refusée.
    Auth,
    /// Crédit ou limite du moteur épuisé (402, « Daily Limit »).
    Exhausted,
    /// 4xx autre : requête refusée.
    Rejected,
    /// 5xx, ou erreur annoncée par le moteur.
    Unavailable,
    /// Réponse illisible.
    Malformed,
    /// L'instance SearXNG ne sert pas le format JSON.
    JsonDisabled,
    /// Adresse de l'instance SearXNG inutilisable.
    BadAddress,
}

impl EngineError {
    /// Code stable (journal, page).
    pub fn as_str(self) -> &'static str {
        match self {
            EngineError::Offline => "offline",
            EngineError::Timeout => "timeout",
            EngineError::Blocked => "blocked",
            EngineError::RateLimited => "rateLimited",
            EngineError::Auth => "keyRefused",
            EngineError::Exhausted => "exhausted",
            EngineError::Rejected => "rejected",
            EngineError::Unavailable => "unavailable",
            EngineError::Malformed => "malformed",
            EngineError::JsonDisabled => "jsonDisabled",
            EngineError::BadAddress => "badAddress",
        }
    }
}

/// Résultats d'une recherche.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Moteur qui a répondu.
    pub provider: Provider,
    pub results: Vec<SearchResult>,
    /// Le moteur personnel a échoué avant que DuckDuckGo réponde.
    pub personal_failed: Option<(Provider, EngineError)>,
}

/// Recherche impossible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchError {
    /// Requête vide ou trop longue.
    InvalidQuery,
    /// Annulée par l'appelant.
    Cancelled,
    /// Aucun moteur n'a répondu : l'échec du moteur personnel (s'il y en a
    /// un), puis celui de DuckDuckGo.
    Failed {
        personal: Option<(Provider, EngineError)>,
        duckduckgo: EngineError,
    },
}

/// Adresses des moteurs (remplaçables par les essais).
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub duckduckgo: String,
    pub brave: String,
    pub mojeek: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            duckduckgo: duckduckgo::DEFAULT_URL.to_owned(),
            brave: brave::DEFAULT_URL.to_owned(),
            mojeek: mojeek::DEFAULT_URL.to_owned(),
        }
    }
}

/// Réglages du client.
#[derive(Clone, Debug)]
pub struct Options {
    /// `ChatonLM/<version>` : honnête, et accepté par DuckDuckGo (relevé 104).
    pub user_agent: String,
    pub endpoints: Endpoints,
    pub duckduckgo_gap: Duration,
    pub duckduckgo_pause: Duration,
    pub timeout: Duration,
}

impl Options {
    pub fn new(user_agent: impl Into<String>) -> Self {
        Options {
            user_agent: user_agent.into(),
            endpoints: Endpoints::default(),
            duckduckgo_gap: DUCKDUCKGO_GAP,
            duckduckgo_pause: DUCKDUCKGO_PAUSE,
            timeout: REQUEST_TIMEOUT,
        }
    }
}

/// Clients HTTP d'une politique de redirection : par le mandataire du
/// système, ou direct vers la machine locale (instance SearXNG locale, faux
/// moteurs des essais).
struct Http {
    remote: Option<reqwest::Client>,
    local: Option<reqwest::Client>,
}

impl Http {
    fn new(options: &Options, redirect: fn() -> reqwest::redirect::Policy) -> Self {
        let build = |local: bool| {
            let mut builder = reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT.min(options.timeout))
                .timeout(options.timeout)
                .gzip(true)
                .redirect(redirect())
                .user_agent(options.user_agent.clone());
            if local {
                builder = builder.no_proxy();
            }
            builder.build().ok()
        };
        Http {
            remote: build(false),
            local: build(true),
        }
    }

    fn for_url(&self, base: &str) -> Result<&reqwest::Client, EngineError> {
        let local = url::Url::parse(base)
            .ok()
            .is_some_and(|url| match url.host() {
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
                None => false,
            });
        if local { &self.local } else { &self.remote }
            .as_ref()
            .ok_or(EngineError::Offline)
    }
}

/// Rythme des requêtes à DuckDuckGo : intervalle minimal, pause après un
/// blocage.
pub struct Pacer {
    gap: Duration,
    pause: Duration,
    state: tokio::sync::Mutex<PacerState>,
}

#[derive(Default)]
struct PacerState {
    last: Option<Instant>,
    blocked_until: Option<Instant>,
}

impl Pacer {
    pub fn new(gap: Duration, pause: Duration) -> Self {
        Pacer {
            gap,
            pause,
            state: tokio::sync::Mutex::new(PacerState::default()),
        }
    }

    /// Attend son tour (les appels concurrents passent l'un après l'autre).
    /// `Err(Some(Blocked))` : en pause après un blocage ; `Err(None)` :
    /// annulé.
    pub async fn turn(&self, cancel: &CancellationToken) -> Result<(), Option<EngineError>> {
        let mut state = tokio::select! {
            state = self.state.lock() => state,
            () = cancel.cancelled() => return Err(None),
        };
        let now = Instant::now();
        if state.blocked_until.is_some_and(|until| now < until) {
            return Err(Some(EngineError::Blocked));
        }
        if let Some(last) = state.last {
            let ready = last + self.gap;
            if now < ready {
                tokio::select! {
                    () = tokio::time::sleep_until(ready) => {}
                    () = cancel.cancelled() => return Err(None),
                }
            }
        }
        state.last = Some(Instant::now());
        Ok(())
    }

    /// DuckDuckGo a bloqué : pause.
    pub async fn blocked(&self) {
        self.state.lock().await.blocked_until = Some(Instant::now() + self.pause);
    }
}

/// Client de recherche (un par app, partagé).
pub struct Client {
    strict: Http,
    searxng: Http,
    endpoints: Endpoints,
    pacer: Pacer,
}

impl Client {
    pub fn new(options: Options) -> Self {
        // Moteur cryptographique de rustls (ring), exigé par reqwest ; sans
        // effet s'il est déjà installé (l'app le fait aussi).
        let _ = rustls::crypto::ring::default_provider().install_default();
        let trimmed = |url: &str| url.trim_end_matches('/').to_owned();
        Client {
            // Brave (clé en en-tête), Mojeek (clé dans l'adresse),
            // DuckDuckGo : aucune redirection suivie.
            strict: Http::new(&options, reqwest::redirect::Policy::none),
            // Instance de l'utilisateur, souvent passée de http à https.
            searxng: Http::new(&options, || reqwest::redirect::Policy::limited(3)),
            endpoints: Endpoints {
                duckduckgo: trimmed(&options.endpoints.duckduckgo),
                brave: trimmed(&options.endpoints.brave),
                mojeek: trimmed(&options.endpoints.mojeek),
            },
            pacer: Pacer::new(options.duckduckgo_gap, options.duckduckgo_pause),
        }
    }

    /// Recherche : le moteur personnel s'il y en a un, puis DuckDuckGo s'il a
    /// échoué (ou s'il n'y en a pas).
    pub async fn search(
        &self,
        personal: Option<&Personal>,
        query: &Query<'_>,
        cancel: &CancellationToken,
    ) -> Result<Found, SearchError> {
        let mut personal_failed = None;
        if let Some(personal) = personal {
            match self.ask(personal, query, cancel).await {
                Ok(results) => {
                    return Ok(Found {
                        provider: personal.provider(),
                        results,
                        personal_failed: None,
                    });
                }
                Err(None) => return Err(SearchError::Cancelled),
                Err(Some(error)) => personal_failed = Some((personal.provider(), error)),
            }
        }
        match self.duckduckgo(query, cancel).await {
            Ok(results) => Ok(Found {
                provider: Provider::DuckDuckGo,
                results,
                personal_failed,
            }),
            Err(None) => Err(SearchError::Cancelled),
            Err(Some(error)) => Err(SearchError::Failed {
                personal: personal_failed,
                duckduckgo: error,
            }),
        }
    }

    /// Un moteur personnel seul (sans DuckDuckGo) : `Err(None)` si annulé.
    pub async fn ask(
        &self,
        personal: &Personal,
        query: &Query<'_>,
        cancel: &CancellationToken,
    ) -> Result<Vec<SearchResult>, Option<EngineError>> {
        let max = usize::from(query.max_results);
        match personal {
            Personal::Brave { key } => {
                let base = &self.endpoints.brave;
                let request = brave::request(self.strict.for_url(base)?, base, key, query)?;
                let body = send(request, cancel, keyed_status).await?;
                let value = json(&body)?;
                Ok(shape(brave::parse_brave(&value)?, max))
            }
            Personal::Mojeek { key } => {
                let base = &self.endpoints.mojeek;
                let request = mojeek::request(self.strict.for_url(base)?, base, key, query)?;
                let body = send(request, cancel, keyed_status).await?;
                let value = json(&body)?;
                Ok(shape(mojeek::parse_mojeek(&value)?, max))
            }
            Personal::Searxng { url } => {
                let base = searxng::normalize(url).ok_or(EngineError::BadAddress)?;
                let request = searxng::request(self.searxng.for_url(&base)?, &base, query)?;
                let body = send(request, cancel, searxng_status).await?;
                Ok(shape(searxng::parse_searxng(&body)?, max))
            }
        }
    }

    async fn duckduckgo(
        &self,
        query: &Query<'_>,
        cancel: &CancellationToken,
    ) -> Result<Vec<SearchResult>, Option<EngineError>> {
        self.pacer.turn(cancel).await?;
        let base = &self.endpoints.duckduckgo;
        let request = duckduckgo::request(self.strict.for_url(base)?, base, query)?;
        let outcome = match send(request, cancel, duckduckgo_status).await {
            Ok(body) => duckduckgo::parse_duckduckgo(&String::from_utf8_lossy(&body)).map_err(Some),
            Err(error) => Err(error),
        };
        if outcome == Err(Some(EngineError::Blocked)) {
            self.pacer.blocked().await;
        }
        Ok(shape(outcome?, usize::from(query.max_results)))
    }
}

fn json(body: &[u8]) -> Result<serde_json::Value, EngineError> {
    serde_json::from_slice(body).map_err(|_| EngineError::Malformed)
}

/// Statut d'un moteur à clé (Brave, Mojeek).
fn keyed_status(status: u16) -> Option<EngineError> {
    match status {
        200 => None,
        401 | 403 => Some(EngineError::Auth),
        402 => Some(EngineError::Exhausted),
        429 => Some(EngineError::RateLimited),
        400..=499 => Some(EngineError::Rejected),
        _ => Some(EngineError::Unavailable),
    }
}

/// Statut de DuckDuckGo : 202 (page de défi), 403, 418, 429 = blocage.
fn duckduckgo_status(status: u16) -> Option<EngineError> {
    match status {
        200 => None,
        202 | 403 | 418 | 429 => Some(EngineError::Blocked),
        400..=499 => Some(EngineError::Rejected),
        _ => Some(EngineError::Unavailable),
    }
}

/// Statut d'une instance SearXNG : 403 = format JSON non permis.
fn searxng_status(status: u16) -> Option<EngineError> {
    match status {
        200 => None,
        403 => Some(EngineError::JsonDisabled),
        429 => Some(EngineError::RateLimited),
        400..=499 => Some(EngineError::Rejected),
        _ => Some(EngineError::Unavailable),
    }
}

/// Envoie la requête et rend le corps d'une réponse acceptée par `status`,
/// sinon la sorte d'échec ; `Err(None)` : annulé. Rien n'est lu de l'erreur
/// de `reqwest` (elle contient l'adresse).
async fn send(
    request: reqwest::RequestBuilder,
    cancel: &CancellationToken,
    status: fn(u16) -> Option<EngineError>,
) -> Result<Vec<u8>, Option<EngineError>> {
    let exchange = async {
        let mut response = request.send().await.map_err(|error| {
            if error.is_connect() || error.is_builder() {
                EngineError::Offline
            } else {
                EngineError::Timeout
            }
        })?;
        let code = response.status().as_u16();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| EngineError::Timeout)? {
            if body.len() + chunk.len() > MAX_BODY {
                return Err(EngineError::Malformed);
            }
            body.extend_from_slice(&chunk);
        }
        match status(code) {
            Some(error) => Err(error),
            None => Ok(body),
        }
    };
    tokio::select! {
        result = exchange => result.map_err(Some),
        () = cancel.cancelled() => Err(None),
    }
}

#[cfg(test)]
mod tests;
