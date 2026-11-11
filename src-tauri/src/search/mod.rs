//! Recherche web de l'app (outil `web_search` du chat, de l'Étude, d'Easy
//! Link et de VS Code), faite depuis cet ordinateur par le crate
//! `web-search` : aucun serveur de ChatonLM ne voit les recherches.
//!
//! DuckDuckGo par défaut, sans compte ni clé ; moteur personnel en option
//! (Settings › Web search : clé Brave Search API, clé Mojeek, instance
//! SearXNG), lu en base à chaque recherche (`store/web_search.rs`) et
//! essayé d'abord ; DuckDuckGo en secours.
//!
//! Erreurs : [`SearchError`] du crate, codes de conversation par [`kind`],
//! texte par [`message`] (section `webSearch` de `backend.json`).
//!
//! Adresses des moteurs : celles du crate ; `CHATONLM_DUCKDUCKGO_URL`,
//! `CHATONLM_BRAVE_URL` et `CHATONLM_MOJEEK_URL` les remplacent dans les
//! builds d'essai seulement (comme les adresses d'Easy Link), fermées par
//! `withApp` dans les e2e. Code commun à Linux et Windows.

use std::path::Path;

use tokio_util::sync::CancellationToken;
use web_search::{EngineError, Found, Personal, Provider, Query, SearchError};

use crate::chat::ChatErrorKind;
use crate::i18n::{Lang, t};
use crate::store::Store;
use crate::store::web_search::{self as saved, Engine};

#[cfg(test)]
mod tests;

/// Adresses remplacées par les essais (builds d'essai seulement). Dans les
/// essais Rust, les moteurs sont fermés par défaut (port 9) : aucun essai ne
/// part vers un vrai moteur sans le dire.
#[cfg(any(test, debug_assertions, feature = "e2e"))]
fn endpoints() -> web_search::Endpoints {
    let mut endpoints = web_search::Endpoints::default();
    if cfg!(test) {
        endpoints = closed_endpoints();
    }
    for (name, slot) in [
        ("CHATONLM_DUCKDUCKGO_URL", &mut endpoints.duckduckgo),
        ("CHATONLM_BRAVE_URL", &mut endpoints.brave),
        ("CHATONLM_MOJEEK_URL", &mut endpoints.mojeek),
    ] {
        if let Some(value) = std::env::var(name)
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_owned())
            .filter(|value| !value.is_empty())
        {
            *slot = value;
        }
    }
    endpoints
}

/// Moteurs fermés (port 9, `discard`, jamais servi en local).
#[cfg(any(test, debug_assertions, feature = "e2e"))]
pub(crate) fn closed_endpoints() -> web_search::Endpoints {
    let closed = "http://127.0.0.1:9".to_owned();
    web_search::Endpoints {
        duckduckgo: closed.clone(),
        brave: closed.clone(),
        mojeek: closed,
    }
}

#[cfg(not(any(test, debug_assertions, feature = "e2e")))]
fn endpoints() -> web_search::Endpoints {
    web_search::Endpoints::default()
}

/// Recherche web de l'app (une par app ; partagée par `Arc`).
pub struct WebSearch {
    client: web_search::Client,
    store: Store,
}

impl WebSearch {
    pub fn new(store: Store) -> Self {
        Self::with_endpoints(store, endpoints())
    }

    pub fn with_endpoints(store: Store, endpoints: web_search::Endpoints) -> Self {
        crate::http::ensure_crypto_provider();
        let mut options = web_search::Options::new(concat!("ChatonLM/", env!("CARGO_PKG_VERSION")));
        options.endpoints = endpoints;
        // Essais Rust : faux moteurs, intervalle court (le rythme est essayé
        // dans le crate).
        if cfg!(test) {
            options.duckduckgo_gap = std::time::Duration::from_millis(20);
        }
        WebSearch {
            client: web_search::Client::new(options),
            store,
        }
    }

    /// Moteur personnel configuré (base illisible : aucun, DuckDuckGo seul).
    async fn personal(&self) -> Option<Personal> {
        match self.store.call(|c| saved::load(c)).await {
            Ok(saved) => saved.personal(),
            Err(error) => {
                crate::app_log!("chatonlm: web search engine unreadable: {error}");
                None
            }
        }
    }

    /// Recherche web. `lang` : langue souhaitée des résultats (`fr`).
    pub async fn search(
        &self,
        query: &str,
        max_results: Option<u8>,
        lang: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Found, SearchError> {
        let query = Query::new(query, max_results, lang).ok_or(SearchError::InvalidQuery)?;
        let personal = self.personal().await;
        let found = self.client.search(personal.as_ref(), &query, cancel).await;
        if let Ok(Found {
            personal_failed: Some((provider, error)),
            ..
        }) = &found
        {
            crate::app_log!(
                "chatonlm: web search: {} failed ({}), DuckDuckGo answered",
                provider.as_str(),
                error.as_str()
            );
        }
        found
    }

    /// Essai du moteur `engine` seul, tel qu'il est configuré (bouton
    /// « Test » des réglages), sans DuckDuckGo en secours.
    pub async fn check(&self, engine: Engine, cancel: &CancellationToken) -> Checked {
        let Ok(mut configured) = self.store.call(|c| saved::load(c)).await else {
            return Checked::Failed(EngineError::Unavailable);
        };
        let Some(query) = Query::new("ChatonLM", Some(1), None) else {
            return Checked::Failed(EngineError::Rejected);
        };
        let outcome = if engine == Engine::DuckDuckGo {
            match self.client.search(None, &query, cancel).await {
                Ok(_) => Ok(()),
                Err(SearchError::Failed { duckduckgo, .. }) => Err(Some(duckduckgo)),
                Err(_) => Err(None),
            }
        } else {
            configured.engine = Some(engine);
            let Some(personal) = configured.personal() else {
                return Checked::NotConfigured;
            };
            self.client.ask(&personal, &query, cancel).await.map(|_| ())
        };
        match outcome {
            Ok(()) => Checked::Ok,
            Err(error) => Checked::Failed(error.unwrap_or(EngineError::Unavailable)),
        }
    }
}

/// Issue de l'essai d'un moteur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checked {
    Ok,
    /// Ni clé ni adresse pour ce moteur.
    NotConfigured,
    Failed(EngineError),
}

/// Nom d'un moteur (marque, jamais traduite).
pub fn engine_name(provider: Provider) -> &'static str {
    match provider {
        Provider::DuckDuckGo => "DuckDuckGo",
        Provider::Brave => "Brave Search",
        Provider::Mojeek => "Mojeek",
        Provider::Searxng => "SearXNG",
    }
}

fn provider_of(engine: Engine) -> Provider {
    match engine {
        Engine::DuckDuckGo => Provider::DuckDuckGo,
        Engine::Brave => Provider::Brave,
        Engine::Mojeek => Provider::Mojeek,
        Engine::Searxng => Provider::Searxng,
    }
}

/// Échec du moteur personnel que l'utilisateur peut corriger dans les
/// réglages (clé, crédit, format JSON, adresse).
fn settings_problem(provider: Provider, error: EngineError, lang: Lang) -> Option<String> {
    let engine = engine_name(provider);
    Some(match error {
        EngineError::Auth => t!("webSearch.keyRefused" in lang, engine = engine),
        EngineError::Exhausted => t!("webSearch.exhausted" in lang, engine = engine),
        EngineError::JsonDisabled => t!("webSearch.jsonDisabled" in lang),
        EngineError::BadAddress => t!("webSearch.badAddress" in lang),
        _ => return None,
    })
}

/// Code d'une recherche impossible dans une conversation ; `None` : un
/// arrêt.
pub fn kind(error: &SearchError) -> Option<ChatErrorKind> {
    Some(match error {
        SearchError::Cancelled => return None,
        SearchError::InvalidQuery => ChatErrorKind::WebSearchUnavailable,
        SearchError::Failed {
            personal,
            duckduckgo,
        } => {
            let fixable = personal.is_some_and(|(provider, error)| {
                settings_problem(provider, error, Lang::En).is_some()
            });
            if fixable || *duckduckgo == EngineError::Blocked {
                ChatErrorKind::WebSearchRefused
            } else {
                ChatErrorKind::WebSearchUnavailable
            }
        }
    })
}

/// Code stable d'une recherche impossible (journal, erreur donnée au
/// modèle).
pub fn code(error: &SearchError) -> String {
    match error {
        SearchError::InvalidQuery => "invalidQuery".into(),
        SearchError::Cancelled => "cancelled".into(),
        SearchError::Failed {
            personal: Some((provider, personal)),
            duckduckgo,
        } => format!(
            "{} {}, duckduckgo {}",
            provider.as_str(),
            personal.as_str(),
            duckduckgo.as_str()
        ),
        SearchError::Failed {
            personal: None,
            duckduckgo,
        } => format!("duckduckgo {}", duckduckgo.as_str()),
    }
}

/// Texte pour l'utilisateur, dans la langue du moment : d'abord ce qu'il
/// peut corriger (clé, adresse), puis le blocage de DuckDuckGo (avec le
/// conseil d'un moteur personnel), puis l'absence de réseau.
pub fn message(error: &SearchError) -> String {
    message_in(error, crate::i18n::current())
}

pub fn message_in(error: &SearchError, lang: Lang) -> String {
    match error {
        SearchError::InvalidQuery => t!("webSearch.invalidQuery" in lang),
        SearchError::Cancelled => t!("webSearch.cancelled" in lang),
        SearchError::Failed {
            personal,
            duckduckgo,
        } => {
            if let Some(text) =
                personal.and_then(|(provider, error)| settings_problem(provider, error, lang))
            {
                return text;
            }
            match (duckduckgo, personal) {
                (EngineError::Blocked, Some((provider, _))) => t!(
                    "webSearch.blockedWithPersonal" in lang,
                    engine = engine_name(*provider)
                ),
                (EngineError::Blocked, None) => t!("webSearch.blocked" in lang),
                (EngineError::Offline, None) => t!("webSearch.offline" in lang),
                (EngineError::Offline, Some((_, EngineError::Offline))) => {
                    t!("webSearch.offline" in lang)
                }
                _ => t!("webSearch.unavailable" in lang),
            }
        }
    }
}

/// Résultat de l'essai d'un moteur (bouton « Test » des réglages), dans la
/// langue du moment.
pub fn check_message(engine: Engine, outcome: Checked) -> String {
    let lang = crate::i18n::current();
    let provider = provider_of(engine);
    let name = engine_name(provider);
    let error = match outcome {
        Checked::Ok => return t!("webSearch.check.ok" in lang, engine = name),
        Checked::NotConfigured => return t!("webSearch.check.notConfigured" in lang),
        Checked::Failed(error) => error,
    };
    if let Some(text) = settings_problem(provider, error, lang) {
        return text;
    }
    match error {
        EngineError::Blocked => t!("webSearch.check.blocked" in lang),
        EngineError::Offline => t!("webSearch.check.offline" in lang, engine = name),
        _ => t!("webSearch.check.unavailable" in lang, engine = name),
    }
}

/// Dossier de la clé d'installation de l'ancien relais de recherche (0.10
/// en préparation), plus utilisée.
const RELAY_DIR: &str = "relay";
const RELAY_IDENTITY: &str = "identity.json";

/// Au démarrage : efface la clé d'installation laissée par l'ancien client du
/// relais, s'il y en a une (et son dossier s'il est vide).
pub fn remove_relay_identity(data_dir: &Path) {
    let dir = data_dir.join(RELAY_DIR);
    let file = dir.join(RELAY_IDENTITY);
    if file.exists() {
        match std::fs::remove_file(&file) {
            Ok(()) => crate::app_log!("chatonlm: old relay identity removed"),
            Err(error) => {
                crate::app_log!("chatonlm: old relay identity not removed: {}", error.kind())
            }
        }
    }
    // Échoue sans rien faire si le dossier n'est pas vide ou n'existe pas.
    let _ = std::fs::remove_dir(&dir);
}
