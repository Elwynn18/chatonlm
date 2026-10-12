// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Translated to Rust from Ollama (https://github.com/ollama/ollama),
// files app/tools/web_search.go, app/tools/web_fetch.go, app/tools/url_policy.go
// and app/ui/ui.go, commit 42e911bc3d05 (the web_search and web_fetch tools,
// their arguments, errors and timeouts, the URLs web_fetch may read, the tools
// as sent to /api/chat by convertToOllamaTool).
// Modified for ChatonLM: the tools are run by the Ollama server
// (/api/experimental/web_search and /api/experimental/web_fetch, signed with
// the server's key) instead of requests to ollama.com signed by the app, so
// the server's own cloud policy answers (no /api/status check first); the
// error text of the server's JSON answer is kept (the official app only gives
// the status); answers are read up to 4 MiB; gpt-oss gets these two tools like
// any other model (no browser tools); a 401 (no account linked to the server)
// stops the send instead of going to the model.

//! Outils de la recherche web, comme l'app officielle : `web_search` et
//! `web_fetch`, exécutés par le serveur Ollama (qui signe les requêtes à
//! ollama.com avec sa clé : un compte relié au serveur est requis).
//!
//! - arguments vérifiés avant toute requête, avec les textes de l'officiel ;
//! - `web_fetch` ne lit qu'une adresse écrite par l'utilisateur dans la
//!   conversation, ou reçue d'un résultat de recherche ou d'une page lue
//!   pendant l'envoi ([`AllowedUrls`], `url_policy.go`) ;
//! - délais de l'officiel (10 s pour une recherche, 30 s pour une page),
//!   réponse lue au plus 4 Mio ;
//! - une erreur d'outil n'arrête pas l'envoi : elle est donnée au modèle ;
//!   sauf un 401 (aucun compte relié au serveur), qui l'arrête : l'interface
//!   propose de se connecter ou de relancer sans recherche ;
//! - l'arrêt de l'envoi interrompt la requête en cours.

use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use tokio::sync::watch;
use url::Url;

use super::cancelled;
use crate::ollama::client;

pub const WEB_SEARCH: &str = "web_search";
pub const WEB_FETCH: &str = "web_fetch";

/// Délais de l'officiel (`http.Client{Timeout: …}`).
const SEARCH_TIMEOUT: Duration = Duration::from_secs(10);
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Taille maximale lue d'une réponse (une page lue reste bien en dessous).
const MAX_BODY: usize = 4 * 1024 * 1024;

/// Nombre de résultats sans `max_results` utilisable.
const DEFAULT_MAX_RESULTS: i64 = 5;

/// Les deux outils, tels que l'officiel les envoie dans `tools` :
/// `convertToOllamaTool` ne garde de chaque propriété que son type et sa
/// description (le `"default": 3` du schéma de `max_results` ne part pas).
pub fn definitions() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": WEB_SEARCH,
                "description": "Search the web for real-time information using ollama.com web search API.",
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

/// Adresses que `web_fetch` peut lire pendant un envoi (`url_policy.go`) :
/// celles des messages de l'utilisateur, puis celles des résultats de
/// recherche et des liens des pages lues.
#[derive(Debug, Default)]
pub struct AllowedUrls(HashSet<String>);

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
            self.0.insert(cleaned.to_owned());
        }
    }

    /// Permise : déjà propre (rien à retirer) et connue (`allowedDirectURL`).
    pub fn allows(&self, raw: &str) -> bool {
        clean_url(raw).is_some_and(|cleaned| cleaned == raw && self.0.contains(cleaned))
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
    /// JSON de la réponse (texte compact), pour l'interface et le modèle.
    Result(String),
    /// Erreur donnée au modèle (sans « Error: »).
    Error(String),
    /// Arrêt demandé pendant l'appel : ni résultat ni erreur.
    Cancelled,
    /// 401 du serveur : aucun compte ollama.com relié ; l'envoi s'arrête,
    /// l'appel reste sans résultat ni erreur.
    SignedOut,
}

/// Délais des deux outils (raccourcis dans les tests).
#[derive(Clone, Copy)]
struct Timeouts {
    search: Duration,
    fetch: Duration,
}

const TIMEOUTS: Timeouts = Timeouts {
    search: SEARCH_TIMEOUT,
    fetch: FETCH_TIMEOUT,
};

/// Exécute l'appel `name` par le serveur `base` (`Registry.Execute`).
pub async fn execute(
    base: &str,
    name: &str,
    arguments: &Value,
    allowed: &mut AllowedUrls,
    cancel: &mut watch::Receiver<bool>,
) -> Executed {
    execute_with(base, name, arguments, allowed, cancel, TIMEOUTS).await
}

async fn execute_with(
    base: &str,
    name: &str,
    arguments: &Value,
    allowed: &mut AllowedUrls,
    cancel: &mut watch::Receiver<bool>,
    timeouts: Timeouts,
) -> Executed {
    match name {
        WEB_SEARCH => web_search(base, arguments, allowed, cancel, timeouts.search).await,
        WEB_FETCH => web_fetch(base, arguments, allowed, cancel, timeouts.fetch).await,
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

/// Résultat de `web_search` (`SearchResponse`) : relu puis réécrit, comme
/// l'officiel (champs inconnus retirés, liste absente écrite `null`).
#[derive(Deserialize, Serialize)]
struct SearchResponse {
    #[serde(default)]
    results: Option<Vec<SearchResult>>,
}

#[derive(Deserialize, Serialize)]
struct SearchResult {
    #[serde(default, deserialize_with = "text_or_null")]
    title: String,
    #[serde(default, deserialize_with = "text_or_null")]
    url: String,
    #[serde(default, deserialize_with = "text_or_null")]
    content: String,
}

/// Résultat de `web_fetch` (`FetchResponse`).
#[derive(Deserialize, Serialize)]
struct FetchResponse {
    #[serde(default, deserialize_with = "text_or_null")]
    title: String,
    #[serde(default, deserialize_with = "text_or_null")]
    content: String,
    #[serde(default)]
    links: Option<Vec<String>>,
}

/// `null` lu comme un texte vide, comme le décodeur JSON de Go.
fn text_or_null<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

async fn web_search(
    base: &str,
    arguments: &Value,
    allowed: &mut AllowedUrls,
    cancel: &mut watch::Receiver<bool>,
    timeout: Duration,
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
    // Nombre JSON (float64 en Go) tronqué ; autre chose ou ≤ 0 : 5.
    let max_results = arguments
        .get("max_results")
        .and_then(Value::as_f64)
        .map(|value| value as i64)
        .filter(|count| *count > 0)
        .unwrap_or(DEFAULT_MAX_RESULTS);
    let body = json!({ "query": query, "max_results": max_results });
    let answer = match post(base, "/api/experimental/web_search", &body, timeout, cancel).await {
        Ok(answer) => answer,
        Err(Failure::Cancelled) => return Executed::Cancelled,
        Err(Failure::Request(detail)) => {
            return Executed::Error(format!("failed to execute search request: {detail}"));
        }
    };
    if answer.status == reqwest::StatusCode::UNAUTHORIZED {
        return Executed::SignedOut;
    }
    let body = match answer.into_body("search") {
        Ok(body) => body,
        Err(error) => return Executed::Error(error),
    };
    let response: SearchResponse = match serde_json::from_slice(&body) {
        Ok(response) => response,
        Err(error) => return Executed::Error(format!("failed to decode response: {error}")),
    };
    for result in response.results.iter().flatten() {
        allowed.add(&result.url);
    }
    compact(&response)
}

async fn web_fetch(
    base: &str,
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
    let body = json!({ "url": url });
    let answer = match post(base, "/api/experimental/web_fetch", &body, timeout, cancel).await {
        Ok(answer) => answer,
        Err(Failure::Cancelled) => return Executed::Cancelled,
        Err(Failure::Request(detail)) => {
            return Executed::Error(format!("failed to execute fetch request: {detail}"));
        }
    };
    if answer.status == reqwest::StatusCode::UNAUTHORIZED {
        return Executed::SignedOut;
    }
    let body = match answer.into_body("fetch") {
        Ok(body) => body,
        Err(error) => return Executed::Error(error),
    };
    let response: FetchResponse = match serde_json::from_slice(&body) {
        Ok(response) => response,
        Err(error) => return Executed::Error(format!("failed to decode response: {error}")),
    };
    for link in response.links.iter().flatten() {
        allowed.add(link);
    }
    compact(&response)
}

fn compact(response: &impl Serialize) -> Executed {
    match serde_json::to_string(response) {
        Ok(text) => Executed::Result(text),
        Err(error) => Executed::Error(format!("failed to marshal tool result: {error}")),
    }
}

/// Échec avant la réponse.
enum Failure {
    Cancelled,
    /// Connexion impossible, délai dépassé... avec le détail.
    Request(String),
}

/// Réponse du serveur, corps lu au plus [`MAX_BODY`].
struct Answer {
    status: reqwest::StatusCode,
    body: Body,
}

enum Body {
    Complete(Vec<u8>),
    TooLarge,
    /// Lecture interrompue (connexion coupée, délai dépassé).
    Failed(String),
}

impl Answer {
    /// Corps d'une réponse 200 ; sinon l'erreur pour le modèle : texte
    /// `error` du JSON du serveur, ou « search API error (status 401) ».
    fn into_body(self, what: &str) -> Result<Vec<u8>, String> {
        if self.status != reqwest::StatusCode::OK {
            let message = match &self.body {
                Body::Complete(body) => serde_json::from_slice::<Value>(body)
                    .ok()
                    .and_then(|value| value.get("error")?.as_str().map(str::to_owned))
                    .filter(|message| !message.trim().is_empty()),
                Body::TooLarge | Body::Failed(_) => None,
            };
            return Err(message
                .unwrap_or_else(|| format!("{what} API error (status {})", self.status.as_u16())));
        }
        match self.body {
            Body::Complete(body) => Ok(body),
            Body::TooLarge => Err(format!(
                "failed to decode response: the response is larger than {} MiB",
                MAX_BODY / (1024 * 1024)
            )),
            Body::Failed(detail) => Err(format!("failed to decode response: {detail}")),
        }
    }
}

/// `POST <base><path>` avec ce délai pour toute la requête ; l'arrêt
/// interrompt l'attente comme la lecture.
async fn post(
    base: &str,
    path: &str,
    body: &Value,
    timeout: Duration,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Answer, Failure> {
    let url = Url::parse(base).map_err(|_| Failure::Request(format!("invalid address: {base}")))?;
    let client =
        client::client(&url, Some(timeout)).map_err(|error| Failure::Request(describe(error)))?;
    let request = client.post(format!("{base}{path}")).json(body).send();
    let mut response = tokio::select! {
        biased;
        () = cancelled(cancel) => return Err(Failure::Cancelled),
        response = request => response.map_err(|error| Failure::Request(describe(error)))?,
    };
    let status = response.status();
    let mut data = Vec::new();
    let body = loop {
        let chunk = tokio::select! {
            biased;
            () = cancelled(cancel) => return Err(Failure::Cancelled),
            chunk = response.chunk() => chunk,
        };
        match chunk {
            Ok(Some(bytes)) => {
                if data.len() + bytes.len() > MAX_BODY {
                    break Body::TooLarge;
                }
                data.extend_from_slice(&bytes);
            }
            Ok(None) => break Body::Complete(data),
            Err(error) => break Body::Failed(describe(error)),
        }
    };
    Ok(Answer { status, body })
}

/// Échec d'une requête, sans l'adresse : l'erreur puis chaque cause distincte
/// (« error sending request: client error (Connect): tcp connect error: … »).
fn describe(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut parts = vec![error.to_string()];
    let mut source = std::error::Error::source(&error);
    while let Some(current) = source {
        let text = current.to_string();
        if !text.is_empty() && !parts.iter().any(|part| part.contains(&text)) {
            parts.push(text);
        }
        source = current.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use fake_ollama::{FakeOllama, Mode};

    use super::*;
    use crate::chat::stream::{self, Line, LineBuffer, Think};
    use crate::store::chats::{Role, ToolCall, Turn};

    /// Attente maximale d'un appel sur le faux serveur.
    const WAIT: Duration = Duration::from_secs(5);

    /// Arrêt jamais demandé (émetteur lâché : l'attente ne finit pas).
    fn idle() -> watch::Receiver<bool> {
        watch::channel(false).1
    }

    async fn run(base: &str, name: &str, arguments: Value, allowed: &mut AllowedUrls) -> Executed {
        let mut cancel = idle();
        let running = execute(base, name, &arguments, allowed, &mut cancel);
        tokio::time::timeout(WAIT, running)
            .await
            .unwrap_or_else(|_| panic!("{name} still running after {WAIT:?}"))
    }

    fn error(text: &str) -> Executed {
        Executed::Error(text.into())
    }

    #[test]
    fn the_tools_are_sent_like_the_official_app() {
        let tools = definitions();
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, [WEB_SEARCH, WEB_FETCH]);
        let search = &tools[0];
        assert_eq!(search["type"], "function");
        assert_eq!(
            search["function"]["description"],
            "Search the web for real-time information using ollama.com web search API."
        );
        assert_eq!(
            search["function"]["parameters"],
            json!({
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": { "type": "string", "description": "The search query to execute" },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of search results to return",
                    },
                },
            })
        );
        assert_eq!(
            tools[1]["function"],
            json!({
                "name": "web_fetch",
                "description": "Crawl and extract text content from web pages",
                "parameters": {
                    "type": "object",
                    "required": ["url"],
                    "properties": {
                        "url": { "type": "string", "description": "URL to crawl and extract content from" },
                    },
                },
            })
        );
    }

    #[test]
    fn urls_written_by_the_user_are_allowed_as_written() {
        // Les deux cas de url_policy_test.go.
        let allowed = AllowedUrls::from_text("summarize https://attacker.example/x");
        assert!(!allowed.allows("https://attacker.example/x!!!!"));
        assert!(allowed.allows("https://attacker.example/x"));
        let allowed = AllowedUrls::from_text("summarize `https://example.com/privacy`");
        assert!(allowed.allows("https://example.com/privacy"));

        let allowed = AllowedUrls::from_text(
            "See https://example.com/a. Then (https://example.com/b), \"https://example.com/c\" \
             and <http://example.com/d>\nhttps://example.com/e?q=1#top!\n\
             https://example.com/f\u{a0}g HTTPS://EXAMPLE.COM/H ftp://example.com/i",
        );
        for url in [
            "https://example.com/a",
            "https://example.com/b",
            "https://example.com/c",
            "http://example.com/d",
            "https://example.com/e?q=1#top",
            // Espace insécable : pas un `\s` de Go, gardée dans l'adresse.
            "https://example.com/f\u{a0}g",
        ] {
            assert!(allowed.allows(url), "{url}");
        }
        for url in [
            // Telle que le modèle l'aurait recopiée, ponctuation comprise.
            "https://example.com/a.",
            "https://example.com/b)",
            " https://example.com/a",
            "https://example.com/a ",
            "https://example.com/f",
            "HTTPS://EXAMPLE.COM/H",
            "ftp://example.com/i",
            "https://example.com/other",
            "",
            "https://",
        ] {
            assert!(!allowed.allows(url), "{url:?}");
        }

        // Adresses reçues pendant l'envoi : nettoyées, http(s) seulement.
        let mut allowed = AllowedUrls::default();
        allowed.add("https://example.com/next).");
        allowed.add("javascript:alert(1)");
        allowed.add("   ");
        assert!(allowed.allows("https://example.com/next"));
        assert_eq!(allowed.0.len(), 1);
    }

    #[test]
    fn answers_are_read_then_rewritten_like_the_official_structs() {
        let search: SearchResponse = serde_json::from_str(
            r#"{"results":[{"title":"T","url":"https://u","content":"C","score":1},{"title":null}],"extra":2}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&search).unwrap(),
            r#"{"results":[{"title":"T","url":"https://u","content":"C"},{"title":"","url":"","content":""}]}"#
        );
        let empty: SearchResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(
            serde_json::to_string(&empty).unwrap(),
            r#"{"results":null}"#
        );
        let page: FetchResponse =
            serde_json::from_str(r#"{"content":"C","links":["https://l"],"title":"T","x":1}"#)
                .unwrap();
        assert_eq!(
            serde_json::to_string(&page).unwrap(),
            r#"{"title":"T","content":"C","links":["https://l"]}"#
        );
    }

    #[tokio::test]
    async fn arguments_are_checked_before_any_request() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_signed_in(true);
        let base = fake.url();
        let mut allowed = AllowedUrls::from_text("https://example.com/page");
        for (name, arguments, expected) in [
            (WEB_SEARCH, json!({}), "query parameter is required"),
            (
                WEB_SEARCH,
                json!({ "max_results": 3 }),
                "query parameter is required",
            ),
            (
                WEB_SEARCH,
                json!({ "query": "" }),
                "query must be a non-empty string",
            ),
            (
                WEB_SEARCH,
                json!({ "query": " \n" }),
                "query must be a non-empty string",
            ),
            (
                WEB_SEARCH,
                json!({ "query": 3 }),
                "query must be a non-empty string",
            ),
            (
                WEB_SEARCH,
                json!({ "query": null }),
                "query must be a non-empty string",
            ),
            (WEB_FETCH, json!({}), "url parameter is required"),
            (
                WEB_FETCH,
                json!({ "url": "  " }),
                "url must be a non-empty string",
            ),
            (
                WEB_FETCH,
                json!({ "url": ["https://example.com/page"] }),
                "url must be a non-empty string",
            ),
            (
                WEB_FETCH,
                json!({ "url": "https://example.com/page." }),
                "web fetch is only allowed for URLs provided by the user",
            ),
            (
                WEB_FETCH,
                json!({ "url": "https://example.com/elsewhere" }),
                "web fetch is only allowed for URLs provided by the user",
            ),
            (
                "browser.search",
                json!({ "query": "x" }),
                "unknown tool: browser.search",
            ),
        ] {
            assert_eq!(
                run(&base, name, arguments.clone(), &mut allowed).await,
                error(expected),
                "{name} {arguments}"
            );
        }
        assert!(fake.web_requests().is_empty());
    }

    #[tokio::test]
    async fn a_search_goes_through_the_server_and_allows_its_results() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_signed_in(true);
        let mut allowed = AllowedUrls::default();
        let found = run(
            &fake.url(),
            WEB_SEARCH,
            json!({ "query": "ollama" }),
            &mut allowed,
        )
        .await;
        let Executed::Result(text) = found else {
            panic!("{found:?}");
        };
        // JSON compact, dans l'ordre des champs de l'officiel.
        assert_eq!(
            text,
            concat!(
                r#"{"results":[{"title":"Result 1 for ollama","url":"https://example.com/1?q=ollama","content":"Content 1 about ollama."},"#,
                r#"{"title":"Result 2 for ollama","url":"https://example.com/2?q=ollama","content":"Content 2 about ollama."}]}"#
            )
        );
        assert!(allowed.allows("https://example.com/1?q=ollama"));
        assert!(allowed.allows("https://example.com/2?q=ollama"));

        // `max_results` : nombre > 0 tronqué, sinon 5.
        for (given, sent) in [
            (json!(2.7), 2),
            (json!(1), 1),
            (json!(0), 5),
            (json!(0.5), 5),
            (json!(-3), 5),
            (json!("3"), 5),
            (Value::Null, 5),
        ] {
            let arguments = json!({ "query": "ollama", "max_results": given });
            run(&fake.url(), WEB_SEARCH, arguments, &mut allowed).await;
            let last = fake.web_requests().pop().unwrap();
            assert_eq!(last["body"]["max_results"], json!(sent), "{given}");
        }
        assert_eq!(
            fake.web_requests()[0],
            json!({
                "path": "/api/experimental/web_search",
                "body": { "query": "ollama", "max_results": 5 },
            })
        );

        // Une adresse de résultat peut être lue.
        let read = run(
            &fake.url(),
            WEB_FETCH,
            json!({ "url": "https://example.com/1?q=ollama" }),
            &mut allowed,
        )
        .await;
        assert!(matches!(read, Executed::Result(_)), "{read:?}");
    }

    #[tokio::test]
    async fn fetched_pages_allow_their_links() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_signed_in(true);
        let mut allowed = AllowedUrls::from_text("Read https://example.com/page.");
        let next = json!({ "url": "https://example.com/next" });
        let refused = run(&fake.url(), WEB_FETCH, next.clone(), &mut allowed).await;
        assert_eq!(
            refused,
            error("web fetch is only allowed for URLs provided by the user")
        );
        let page = run(
            &fake.url(),
            WEB_FETCH,
            json!({ "url": "https://example.com/page" }),
            &mut allowed,
        )
        .await;
        assert_eq!(
            page,
            Executed::Result(
                r#"{"title":"Page https://example.com/page","content":"Text of https://example.com/page.","links":["https://example.com/next"]}"#
                    .into()
            )
        );
        assert!(matches!(
            run(&fake.url(), WEB_FETCH, next, &mut allowed).await,
            Executed::Result(_)
        ));
        let urls: Vec<&str> = fake
            .web_requests()
            .iter()
            .map(|request| {
                assert_eq!(request["path"], "/api/experimental/web_fetch");
                request["body"]["url"].as_str().unwrap()
            })
            .map(|url| {
                if url.ends_with("page") {
                    "page"
                } else {
                    "next"
                }
            })
            .collect();
        assert_eq!(urls, ["page", "next"]);
    }

    #[tokio::test]
    async fn server_errors_go_to_the_model() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let mut allowed = AllowedUrls::from_text("https://example.com/page");
        let search = json!({ "query": "ollama" });
        let fetch = json!({ "url": "https://example.com/page" });
        // Compte délié (401) : à part, l'envoi s'arrêtera.
        assert_eq!(
            run(&fake.url(), WEB_SEARCH, search.clone(), &mut allowed).await,
            Executed::SignedOut
        );
        assert_eq!(
            run(&fake.url(), WEB_FETCH, fetch.clone(), &mut allowed).await,
            Executed::SignedOut
        );
        fake.set_cloud_disabled(true);
        assert_eq!(
            run(&fake.url(), WEB_SEARCH, search.clone(), &mut allowed).await,
            error("ollama cloud is disabled: web search is unavailable")
        );
        // Sans texte d'erreur (proxy) : le statut, comme l'officiel.
        let proxy = FakeOllama::start(Mode::Status(502)).await.unwrap();
        assert_eq!(
            run(&proxy.url(), WEB_SEARCH, search.clone(), &mut allowed).await,
            error("search API error (status 502)")
        );
        assert_eq!(
            run(&proxy.url(), WEB_FETCH, fetch.clone(), &mut allowed).await,
            error("fetch API error (status 502)")
        );
        // 200 illisible (un autre serveur).
        let other = FakeOllama::start(Mode::NotOllama).await.unwrap();
        for (name, arguments) in [(WEB_SEARCH, &search), (WEB_FETCH, &fetch)] {
            let Executed::Error(text) =
                run(&other.url(), name, arguments.clone(), &mut allowed).await
            else {
                panic!("{name}: an error was expected");
            };
            assert!(text.starts_with("failed to decode response: "), "{text}");
        }
        // Rien n'écoute.
        let closed = FakeOllama::start(Mode::ollama()).await.unwrap();
        let url = closed.url();
        closed.stop().await;
        for (name, arguments, start) in [
            (WEB_SEARCH, &search, "failed to execute search request: "),
            (WEB_FETCH, &fetch, "failed to execute fetch request: "),
        ] {
            let Executed::Error(text) = run(&url, name, arguments.clone(), &mut allowed).await
            else {
                panic!("{name}: an error was expected");
            };
            assert!(text.starts_with(start), "{text}");
            assert!(!text.contains(&url), "{text}");
        }
    }

    #[tokio::test]
    async fn a_slow_tool_times_out() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_signed_in(true);
        let short = Timeouts {
            search: Duration::from_millis(200),
            fetch: Duration::from_millis(200),
        };
        let mut allowed = AllowedUrls::from_text("https://example.com/slow");
        for (name, arguments, start) in [
            (
                WEB_SEARCH,
                json!({ "query": "slow" }),
                "failed to execute search request: ",
            ),
            (
                WEB_FETCH,
                json!({ "url": "https://example.com/slow" }),
                "failed to execute fetch request: ",
            ),
        ] {
            let started = Instant::now();
            let (base, mut cancel) = (fake.url(), idle());
            let running = execute_with(&base, name, &arguments, &mut allowed, &mut cancel, short);
            let done = tokio::time::timeout(WAIT, running)
                .await
                .unwrap_or_else(|_| panic!("{name}: no timeout"));
            let Executed::Error(text) = done else {
                panic!("{name}: {done:?}");
            };
            assert!(text.starts_with(start), "{text}");
            assert!(started.elapsed() < Duration::from_secs(2));
        }
        // Les délais de l'officiel.
        assert_eq!(
            (TIMEOUTS.search.as_secs(), TIMEOUTS.fetch.as_secs()),
            (10, 30)
        );
    }

    #[tokio::test]
    async fn a_stop_interrupts_the_request() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_signed_in(true);
        let (stop, mut cancel) = watch::channel(false);
        let base = fake.url();
        let task = tokio::spawn(async move {
            let mut allowed = AllowedUrls::default();
            let arguments = json!({ "query": "slow" });
            execute(&base, WEB_SEARCH, &arguments, &mut allowed, &mut cancel).await
        });
        let polling = async {
            while fake.web_requests().is_empty() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(WAIT, polling).await.unwrap();
        let asked = Instant::now();
        stop.send(true).unwrap();
        let done = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("stopped within 1 s")
            .unwrap();
        assert_eq!(done, Executed::Cancelled);
        assert!(asked.elapsed() < Duration::from_secs(1));
        let polling = async {
            while fake.disconnects() == 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(WAIT, polling)
            .await
            .expect("the server sees the request abandoned");
    }

    #[tokio::test]
    async fn answers_are_read_up_to_4_mib() {
        // Résultat de 5 Mio, fini : sans plafond il serait lu en entier et
        // rendu au modèle.
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_signed_in(true);
        let mut allowed = AllowedUrls::default();
        let read = run(
            &fake.url(),
            WEB_SEARCH,
            json!({ "query": "huge" }),
            &mut allowed,
        )
        .await;
        assert_eq!(
            read,
            error("failed to decode response: the response is larger than 4 MiB")
        );
        const { assert!(fake_ollama::WEB_HUGE > MAX_BODY) };
        assert!(!allowed.allows("https://example.com/1?q=huge"));
    }

    const TEST_INSTANCE: &str = "http://127.0.0.1:11435";

    /// Contrat : l'instance de test n'a pas de compte relié ; elle refuse la
    /// recherche (401 et la réponse d'ollama.com, `{"error":"Unauthorized"}`,
    /// reprise par le faux serveur), ce que l'outil signale à part.
    #[tokio::test]
    #[ignore = "instance Ollama de test : scripts/test-ollama.sh start"]
    async fn contract_web_search_needs_a_linked_account() {
        let url = Url::parse(TEST_INSTANCE).unwrap();
        let response = client::client(&url, Some(SEARCH_TIMEOUT))
            .unwrap()
            .post(format!("{TEST_INSTANCE}/api/experimental/web_search"))
            .json(&json!({ "query": "ollama", "max_results": 1 }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body, json!({ "error": "Unauthorized" }));
        let mut allowed = AllowedUrls::default();
        let arguments = json!({ "query": "ollama", "max_results": 1 });
        assert_eq!(
            execute(
                TEST_INSTANCE,
                WEB_SEARCH,
                &arguments,
                &mut allowed,
                &mut idle()
            )
            .await,
            Executed::SignedOut
        );
    }

    /// Lignes d'une réponse de `/api/chat` de l'instance de test.
    async fn chat_lines(body: &Value) -> Vec<Line> {
        let url = Url::parse(TEST_INSTANCE).unwrap();
        let mut response = client::client(&url, Some(Duration::from_secs(120)))
            .unwrap()
            .post(format!("{TEST_INSTANCE}/api/chat"))
            .json(body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let mut buffer = LineBuffer::default();
        let mut lines = Vec::new();
        while let Some(chunk) = response.chunk().await.unwrap() {
            lines.extend(buffer.push(&chunk));
        }
        lines.extend(buffer.finish());
        lines
            .iter()
            .map(|line| stream::parse_line(line).unwrap())
            .collect()
    }

    /// Contrat : qwen3:0.6b, avec les deux outils, appelle `web_search` (forme
    /// des lignes `tool_calls` lues par ChatonLM), et Ollama accepte en retour
    /// l'historique de ChatonLM (réponse avec `tool_calls`, message `tool`).
    #[tokio::test]
    #[ignore = "instance Ollama de test : scripts/test-ollama.sh start"]
    async fn contract_tool_calls_from_the_test_instance() {
        let tools = definitions();
        let user = Turn {
            role: Role::User,
            content: "Use the web_search tool to search for the latest Ollama release.".into(),
            thinking: String::new(),
            files: Vec::new(),
            tool_calls: Vec::new(),
        };
        let think = Think::Enabled(false);
        let body = |turns: &[Turn]| {
            let mut body = stream::request_body(
                "qwen3:0.6b",
                turns,
                Some(&think),
                4096,
                0,
                true,
                Some(&tools),
            );
            body["options"]["temperature"] = json!(0);
            body["options"]["seed"] = json!(42);
            body
        };
        let lines = chat_lines(&body(std::slice::from_ref(&user))).await;
        let calls: Vec<_> = lines
            .iter()
            .flat_map(|line| match line {
                Line::Fragment { tool_calls, .. } => tool_calls.clone(),
                Line::Error(error) => panic!("{error}"),
            })
            .collect();
        let search = calls
            .iter()
            .find(|call| call.name == WEB_SEARCH)
            .unwrap_or_else(|| panic!("no web_search call: {lines:?}"));
        let query = search.arguments["query"].as_str().unwrap_or_default();
        assert!(!query.trim().is_empty(), "{:?}", search.arguments);

        let answered = Turn {
            role: Role::Assistant,
            content: String::new(),
            thinking: String::new(),
            files: Vec::new(),
            tool_calls: vec![ToolCall {
                error: Some("Unauthorized".into()),
                ..ToolCall::new(WEB_SEARCH, search.arguments.to_string())
            }],
        };
        let lines = chat_lines(&body(&[user, answered])).await;
        let done = lines
            .iter()
            .any(|line| matches!(line, Line::Fragment { done: true, .. }));
        assert!(done, "{lines:?}");
        assert!(
            !lines.iter().any(|line| matches!(line, Line::Error(_))),
            "{lines:?}"
        );
    }
}
