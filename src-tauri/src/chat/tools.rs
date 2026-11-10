// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Translated to Rust from Ollama (https://github.com/ollama/ollama),
// files app/tools/web_search.go, app/tools/web_fetch.go, app/tools/url_policy.go
// and app/ui/ui.go, commit 42e911bc3d05 (the web_search and web_fetch tools,
// their arguments and errors, the URLs web_fetch may read, the tools as sent
// by convertToOllamaTool).
// Modified for ChatonLM: web_search goes through ChatonLM's own relay
// (relay.chatonlm.com, no account: a key per installation) instead of
// ollama.com, and its description no longer names ollama.com; web_fetch is
// run by the app itself, with a guard against requests to the machine or its
// network (addresses checked after DNS, the connection made to the checked
// address, redirects followed by hand and checked again); the history keeps
// only the titles and links of the search results (the excerpts go to the
// model during the send only, as the search provider's terms ask); a relay
// that refuses (daily quota, pause, outdated app, unreachable at the first
// search) stops the send with its own code instead of going to the model.

//! Outils de la recherche web, comme l'app officielle : `web_search` et
//! `web_fetch`.
//!
//! - `web_search` passe par le relais de ChatonLM ([`crate::relay`]) ; le
//!   résultat enregistré (historique, page) n'a que les titres, les liens et
//!   le moteur qui a répondu (mention « Powered by Brave ») ; les extraits
//!   vont au modèle pendant l'envoi seulement ([`AllowedUrls::model_text`]) ;
//! - `web_fetch` est fait par l'app ([`fetch`]) : http(s) seulement, adresses
//!   publiques seulement, taille et durée bornées, pages texte ou HTML ;
//! - `web_fetch` ne lit qu'une adresse écrite par l'utilisateur dans la
//!   conversation, ou reçue d'un résultat de recherche ou d'une page lue
//!   pendant l'envoi ([`AllowedUrls`], `url_policy.go`) ;
//! - une erreur d'outil n'arrête pas l'envoi : elle est donnée au modèle
//!   (page illisible, recherche vide) ; sauf un refus du relais (quota,
//!   pause, budget, version trop ancienne, relais injoignable à la première
//!   recherche), qui l'arrête avec son code ([`Executed::Stop`]) ;
//! - l'arrêt de l'envoi interrompt la requête en cours.

mod fetch;
mod page;

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::{ChatErrorKind, WebTools, cancelled};
use crate::llm::errors::relay_kind;
use crate::relay::RelayError;

pub use fetch::Policy;

pub const WEB_SEARCH: &str = "web_search";
pub const WEB_FETCH: &str = "web_fetch";

/// Délai de lecture d'une page (celui de l'officiel).
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Nombre de résultats sans `max_results` utilisable.
const DEFAULT_MAX_RESULTS: i64 = relay_proto::DEFAULT_RESULTS as i64;

/// Les deux outils, tels que l'officiel les envoie dans `tools` :
/// `convertToOllamaTool` ne garde de chaque propriété que son type et sa
/// description (le `"default": 3` du schéma de `max_results` ne part pas).
pub fn definitions() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": WEB_SEARCH,
                "description": "Search the web for real-time information.",
                "parameters": {
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query to execute",
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Maximum number of search results to return",
                        },
                    },
                },
            },
        },
        {
            "type": "function",
            "function": {
                "name": WEB_FETCH,
                "description": "Crawl and extract text content from web pages",
                "parameters": {
                    "type": "object",
                    "required": ["url"],
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "URL to crawl and extract content from",
                        },
                    },
                },
            },
        },
    ])
}

/// Adresses d'URL dans un texte (`directURLPattern`). `\s` de Go : espaces
/// ASCII seulement.
static DIRECT_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://[^\t\n\x0C\r <>"'`]+"#).expect("valid pattern"));

/// État des outils pendant un envoi : adresses que `web_fetch` peut lire
/// (`url_policy.go`) — celles des messages de l'utilisateur, puis celles des
/// résultats de recherche et des liens des pages lues — et texte des
/// recherches donné au modèle (avec les extraits), qui ne vit que le temps
/// de l'envoi.
#[derive(Debug, Default)]
pub struct AllowedUrls {
    urls: HashSet<String>,
    /// Résultat enregistré d'une recherche → texte pour le modèle.
    for_model: HashMap<String, String>,
    /// Une recherche a déjà abouti pendant cet envoi.
    searched: bool,
}

impl AllowedUrls {
    /// Adresses écrites dans `text` (`WithAllowedDirectURLs`).
    pub fn from_text(text: &str) -> Self {
        let mut allowed = Self::default();
        for found in DIRECT_URL.find_iter(text) {
            allowed.add(found.as_str());
        }
        allowed
    }

    /// Ajoute une adresse, nettoyée (`addAllowedDirectURL`).
    pub fn add(&mut self, raw: &str) {
        if let Some(cleaned) = clean_url(raw) {
            self.urls.insert(cleaned.to_owned());
        }
    }

    /// Permise : déjà propre (rien à retirer) et connue (`allowedDirectURL`).
    pub fn allows(&self, raw: &str) -> bool {
        clean_url(raw).is_some_and(|cleaned| cleaned == raw && self.urls.contains(cleaned))
    }

    /// Texte à donner au modèle pour le résultat `stored` d'un appel de cet
    /// envoi : une recherche y retrouve ses extraits (jamais enregistrés) ;
    /// tout autre résultat est donné tel quel.
    pub fn model_text<'a>(&'a self, stored: &'a str) -> &'a str {
        self.for_model.get(stored).map_or(stored, String::as_str)
    }
}

/// Historique pour le modèle (requête d'un tour de l'envoi) : chaque
/// résultat de `web_search` de cet envoi y retrouve ses extraits
/// ([`AllowedUrls::model_text`]) ; le reste est inchangé. Un résultat d'un
/// envoi précédent reste tel qu'enregistré (titres et liens).
pub fn give_excerpts(messages: &mut [crate::inference::Message], allowed: &AllowedUrls) {
    use crate::inference::{Content, Role};
    for message in messages.iter_mut() {
        if message.role == Role::Tool
            && message.name.as_deref() == Some(WEB_SEARCH)
            && let Content::Text(text) = &mut message.content
        {
            let full = allowed.model_text(text).to_owned();
            *text = full;
        }
    }
}

/// Espaces autour, ponctuation finale retirés ; http(s) seulement
/// (`cleanDirectURL`).
fn clean_url(raw: &str) -> Option<&str> {
    let raw = raw.trim().trim_end_matches(|c| ".,;:!?)]}".contains(c));
    (raw.starts_with("http://") || raw.starts_with("https://")).then_some(raw)
}

/// Fin de l'exécution d'un appel.
#[derive(Debug, PartialEq, Eq)]
pub enum Executed {
    /// JSON de la réponse (texte compact), enregistré et montré ; le modèle
    /// reçoit [`AllowedUrls::model_text`] de ce texte.
    Result(String),
    /// Erreur donnée au modèle (sans « Error: »).
    Error(String),
    /// Arrêt demandé pendant l'appel : ni résultat ni erreur.
    Cancelled,
    /// L'envoi s'arrête avec ce code (recherche impossible : relais
    /// injoignable, quota, pause, version) ; l'appel reste sans résultat ni
    /// erreur.
    Stop(ChatErrorKind, String),
}

/// Exécute l'appel `name` avec les outils web de l'envoi
/// (`Registry.Execute`).
pub async fn execute(
    web: &WebTools,
    name: &str,
    arguments: &Value,
    allowed: &mut AllowedUrls,
    cancel: &mut watch::Receiver<bool>,
) -> Executed {
    execute_with(
        web,
        &Policy::public(),
        name,
        arguments,
        allowed,
        cancel,
        FETCH_TIMEOUT,
    )
    .await
}

async fn execute_with(
    web: &WebTools,
    policy: &Policy,
    name: &str,
    arguments: &Value,
    allowed: &mut AllowedUrls,
    cancel: &mut watch::Receiver<bool>,
    fetch_timeout: Duration,
) -> Executed {
    match name {
        WEB_SEARCH => web_search(web, arguments, allowed, cancel).await,
        WEB_FETCH => web_fetch(policy, arguments, allowed, cancel, fetch_timeout).await,
        other => Executed::Error(format!("unknown tool: {other}")),
    }
}

/// Texte d'un argument : absent → `missing`, pas un texte ou vide →
/// `invalid` (comme `args["query"].(string)` puis `TrimSpace`).
fn text_argument<'a>(
    arguments: &'a Value,
    key: &str,
    missing: &str,
    invalid: &str,
) -> Result<&'a str, Executed> {
    let Some(value) = arguments.get(key) else {
        return Err(Executed::Error(missing.into()));
    };
    value
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| Executed::Error(invalid.into()))
}

/// Un résultat enregistré : titre et lien seulement.
#[derive(Serialize)]
struct StoredResult<'a> {
    title: &'a str,
    url: &'a str,
}

/// Résultat enregistré d'une recherche.
#[derive(Serialize)]
struct StoredSearch<'a> {
    results: Vec<StoredResult<'a>>,
    /// Moteur qui a répondu (`brave` : mention « Powered by Brave »).
    provider: &'a str,
}

/// Un résultat pour le modèle, au format du `web_search` d'Ollama.
#[derive(Serialize)]
struct ModelResult<'a> {
    title: &'a str,
    url: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ModelSearch<'a> {
    results: Vec<ModelResult<'a>>,
}

/// Langue des résultats souhaitée : celle de la requête si ses mots la
/// désignent nettement (une requête en anglais tapée dans l'app en français
/// garde des résultats en anglais).
fn query_language(query: &str) -> Option<&'static str> {
    use crate::proofread::language::{Language, detect};
    detect(query).map(|language| match language {
        Language::English => "en",
        Language::French => "fr",
        Language::Spanish => "es",
        Language::German => "de",
        Language::Italian => "it",
        Language::Portuguese => "pt",
    })
}

/// Refus du relais : arrêt de l'envoi (quota, pause, budget, version ;
/// relais injoignable ou autre refus à la première recherche de l'envoi),
/// sinon erreur donnée au modèle.
fn refused(error: &RelayError, first_search: bool) -> Executed {
    let Some(kind) = relay_kind(error) else {
        return Executed::Cancelled;
    };
    match (kind, error) {
        // La requête du modèle est en cause : à lui de la refaire.
        (_, RelayError::InvalidQuery) => {
            Executed::Error("the search query is empty or too long".into())
        }
        (ChatErrorKind::WebSearchUnavailable, _) if !first_search => {
            Executed::Error(format!("web search failed: {}", error.code()))
        }
        (kind, error) => Executed::Stop(kind, error.message()),
    }
}

async fn web_search(
    web: &WebTools,
    arguments: &Value,
    allowed: &mut AllowedUrls,
    cancel: &mut watch::Receiver<bool>,
) -> Executed {
    let query = match text_argument(
        arguments,
        "query",
        "query parameter is required",
        "query must be a non-empty string",
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    // Nombre JSON (float64 en Go) tronqué ; autre chose ou ≤ 0 : 5 ; au plus
    // le maximum du relais.
    let max_results = arguments
        .get("max_results")
        .and_then(Value::as_f64)
        .map(|value| value as i64)
        .filter(|count| *count > 0)
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .min(i64::from(relay_proto::MAX_RESULTS));
    let max_results = u8::try_from(max_results).unwrap_or(relay_proto::DEFAULT_RESULTS);
    let stop = CancellationToken::new();
    let searching = web
        .relay
        .search(query, Some(max_results), query_language(query), &stop);
    let reply = tokio::select! {
        biased;
        () = cancelled(cancel) => {
            stop.cancel();
            return Executed::Cancelled;
        }
        reply = searching => reply,
    };
    let reply = match reply {
        Ok(reply) => reply,
        Err(error) => {
            crate::app_log!("chatonlm: web search refused: {}", error.code());
            return refused(&error, !allowed.searched);
        }
    };
    allowed.searched = true;
    for result in &reply.results {
        allowed.add(&result.url);
    }
    let stored = StoredSearch {
        results: reply
            .results
            .iter()
            .map(|result| StoredResult {
                title: &result.title,
                url: &result.url,
            })
            .collect(),
        provider: reply.provider.as_str(),
    };
    let for_model = ModelSearch {
        results: reply
            .results
            .iter()
            .map(|result| ModelResult {
                title: &result.title,
                url: &result.url,
                content: &result.content,
            })
            .collect(),
    };
    match (
        serde_json::to_string(&stored),
        serde_json::to_string(&for_model),
    ) {
        (Ok(stored), Ok(for_model)) => {
            allowed.for_model.insert(stored.clone(), for_model);
            Executed::Result(stored)
        }
        (Err(error), _) | (_, Err(error)) => {
            Executed::Error(format!("failed to marshal tool result: {error}"))
        }
    }
}

/// Page lue, au format du `web_fetch` d'Ollama (`FetchResponse`).
#[derive(Serialize)]
struct FetchResponse {
    title: String,
    content: String,
    links: Vec<String>,
}

async fn web_fetch(
    policy: &Policy,
    arguments: &Value,
    allowed: &mut AllowedUrls,
    cancel: &mut watch::Receiver<bool>,
    timeout: Duration,
) -> Executed {
    let url = match text_argument(
        arguments,
        "url",
        "url parameter is required",
        "url must be a non-empty string",
    ) {
        Ok(url) => url,
        Err(error) => return error,
    };
    if !allowed.allows(url) {
        return Executed::Error("web fetch is only allowed for URLs provided by the user".into());
    }
    let fetched = tokio::select! {
        biased;
        () = cancelled(cancel) => return Executed::Cancelled,
        fetched = tokio::time::timeout(timeout, fetch::fetch(policy, url)) => fetched,
    };
    let page = match fetched {
        Err(_) => return Executed::Error("failed to fetch the page: timed out".into()),
        Ok(Err(error)) => return Executed::Error(format!("failed to fetch the page: {error}")),
        Ok(Ok(page)) => page,
    };
    for link in &page.links {
        allowed.add(link);
    }
    let response = FetchResponse {
        title: page.title,
        content: page.text,
        links: page.links,
    };
    match serde_json::to_string(&response) {
        Ok(text) => Executed::Result(text),
        Err(error) => Executed::Error(format!("failed to marshal tool result: {error}")),
    }
}

/// Outils d'essai sur un relais donné (essais du chat et de l'Étude).
#[cfg(test)]
pub(crate) fn web_tools_on(relay_url: &str, data_dir: &std::path::Path) -> WebTools {
    let relay = crate::relay::RelayClient::with_url(relay_url.to_owned(), data_dir.to_path_buf());
    WebTools {
        relay: std::sync::Arc::new(relay),
    }
}

/// Outils d'essai sur un relais fermé (aucune recherche ne répond :
/// `webSearchUnavailable` à la première) : essais qui n'en lancent pas.
#[cfg(test)]
pub(crate) fn closed_web_tools() -> WebTools {
    // Dans le projet (`.tools/`, ignoré par git), comme les autres données
    // d'essai : le client y écrit sa clé avant de tenter l'inscription.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tools/tmp/relay");
    web_tools_on("http://127.0.0.1:9", &dir)
}

/// Faux relais complet (crate `relay-server`, faux moteurs Brave et Mojeek,
/// lancé dans le processus) et outils qui s'y adressent : essais du chat
/// (recherche de bout en bout, séquences du simulateur) et de l'Étude.
/// `tweak` règle le relais (quota, pause, budget, version minimale…).
#[cfg(test)]
pub(crate) struct FakeWeb {
    pub relay: relay_server::fake::TestRelay,
    pub web: WebTools,
    _dir: tempfile::TempDir,
}

#[cfg(test)]
pub(crate) async fn fake_web(tweak: impl FnOnce(&mut relay_server::config::Config)) -> FakeWeb {
    let dir = tempfile::tempdir().expect("temporary dir");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let relay =
        relay_server::fake::TestRelay::start(&dir.path().join("relay.sqlite"), now, tweak).await;
    let web = web_tools_on(&relay.url, &dir.path().join("data"));
    FakeWeb {
        relay,
        web,
        _dir: dir,
    }
}

#[cfg(test)]
mod tests;
