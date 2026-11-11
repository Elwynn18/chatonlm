//! Faux moteurs de recherche web (crate `web-search` de l'app), sur un seul
//! port, chacun sous son préfixe :
//!
//! - `/ddg/html/` : DuckDuckGo (POST `q`, `kl`), page HTML au format de la
//!   vraie (classes `result__a`, `result__snippet`) ;
//! - `/brave/res/v1/web/search` : Brave Search API (`X-Subscription-Token`) ;
//! - `/mojeek/search` : Mojeek (`api_key`, `fmt=json`) ;
//! - `/searxng/search` : SearXNG (`format=json`).
//!
//! Cinq résultats par recherche : titre `<Moteur> result <n> for <requête>`,
//! adresse `https://<moteur>.example/<n>`, extrait `Extrait <n> (<moteur>)`.
//! Une clé `bad-key` est refusée (401 ; statut d'erreur de Mojeek).
//!
//! Comportement d'un moteur ([`Behavior`]) : par [`FakeSearch::set`] dans le
//! processus, ou `POST /fake/behavior {"engine": "duckduckgo", "behavior":
//! "blocked" | "ok" | "status:<code>" | "slow" | "html"}`. Requêtes reçues :
//! `GET /fake/requests` (moteur, requête, langue ou région).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Clé refusée par les faux Brave et Mojeek.
pub const BAD_KEY: &str = "bad-key";

/// Moteurs servis.
pub const ENGINES: [&str; 4] = ["duckduckgo", "brave", "mojeek", "searxng"];

/// Comportement d'un faux moteur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behavior {
    /// Répond avec ses cinq résultats.
    Ok,
    /// Bloque (DuckDuckGo : 202 et page « anomaly » ; autres : 429).
    Blocked,
    /// Répond ce statut, corps vide.
    Status(u16),
    /// Ne répond qu'après ce délai.
    Slow(Duration),
    /// 200 et une page HTML (SearXNG sans format JSON).
    Html,
}

impl Behavior {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "ok" => Some(Behavior::Ok),
            "blocked" => Some(Behavior::Blocked),
            "slow" => Some(Behavior::Slow(Duration::from_secs(60))),
            "html" => Some(Behavior::Html),
            other => other
                .strip_prefix("status:")?
                .parse()
                .ok()
                .map(Behavior::Status),
        }
    }
}

/// Une requête reçue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Received {
    pub engine: &'static str,
    pub query: String,
    /// Langue (`search_lang`, `lb`, `language`) ou région (`kl`).
    pub lang: String,
    /// Nombre de résultats demandés (`count`, `t`) ; vide sans ce paramètre.
    pub count: String,
}

#[derive(Default)]
struct Shared {
    behaviors: Mutex<HashMap<&'static str, Behavior>>,
    requests: Mutex<Vec<Received>>,
}

pub struct FakeSearch {
    addr: SocketAddr,
    shared: Arc<Shared>,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl FakeSearch {
    pub async fn start() -> std::io::Result<Self> {
        Self::start_at(SocketAddr::from(([127, 0, 0, 1], 0))).await
    }

    pub async fn start_at(addr: SocketAddr) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared::default());
        let (shutdown, stop) = oneshot::channel::<()>();
        let app = Router::new().fallback(handle).with_state(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stop.await;
                })
                .await;
        });
        Ok(FakeSearch {
            addr,
            shared,
            shutdown: Some(shutdown),
            task,
        })
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Adresse de base d'un moteur (`duckduckgo` → `<url>/ddg`).
    pub fn base(&self, engine: &str) -> String {
        format!("{}/{}", self.url(), prefix(engine))
    }

    pub fn set(&self, engine: &'static str, behavior: Behavior) {
        self.shared
            .behaviors
            .lock()
            .unwrap()
            .insert(engine, behavior);
    }

    pub fn requests(&self) -> Vec<Received> {
        self.shared.requests.lock().unwrap().clone()
    }

    pub async fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let _ = self.task.await;
    }
}

fn prefix(engine: &str) -> &str {
    match engine {
        "duckduckgo" => "ddg",
        other => other,
    }
}

fn label(engine: &str) -> &'static str {
    match engine {
        "duckduckgo" => "DuckDuckGo",
        "brave" => "Brave",
        "mojeek" => "Mojeek",
        _ => "SearXNG",
    }
}

/// Cinq résultats (titre, adresse, extrait).
fn results(engine: &str, query: &str) -> Vec<(String, String, String)> {
    (1..=5)
        .map(|n| {
            (
                format!("{} result {n} for {query}", label(engine)),
                format!("https://{engine}.example/{n}"),
                format!("Extrait {n} ({engine})"),
            )
        })
        .collect()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn duckduckgo_page(query: &str) -> String {
    let mut page = String::from(
        "<!DOCTYPE html>\n<html><head><title>DuckDuckGo</title></head><body class=\"body--html\">\n<div id=\"links\" class=\"results\">\n",
    );
    for (title, url, snippet) in results("duckduckgo", query) {
        let url = escape(&url);
        page.push_str(&format!(
            "<div class=\"result results_links results_links_deep web-result \">\n\
             <div class=\"links_main links_deep result__body\">\n\
             <h2 class=\"result__title\"><a rel=\"nofollow\" class=\"result__a\" href=\"{url}\">{}</a></h2>\n\
             <a class=\"result__snippet\" href=\"{url}\">{}</a>\n</div></div>\n",
            escape(&title),
            escape(&snippet).replacen("Extrait", "<b>Extrait</b>", 1),
        ));
    }
    page.push_str("</div></body></html>\n");
    page
}

const ANOMALY_PAGE: &str = "<!DOCTYPE html><html><body><div class=\"anomaly-modal__title\">Unfortunately, bots use DuckDuckGo too.</div><form id=\"challenge-form\" action=\"//duckduckgo.com/anomaly.js?sv=html\"></form></body></html>";

fn pairs(text: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(text.as_bytes())
        .into_owned()
        .collect()
}

async fn handle(
    State(shared): State<Arc<Shared>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path();
    let query = pairs(uri.query().unwrap_or_default());
    if path == "/fake/requests" {
        let list: Vec<Value> = shared
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(
                |r| json!({"engine": r.engine, "query": r.query, "lang": r.lang, "count": r.count}),
            )
            .collect();
        return axum::Json(Value::Array(list)).into_response();
    }
    if path == "/fake/behavior" && method == Method::POST {
        let request: Value = serde_json::from_slice(&body).unwrap_or_default();
        let engine = request["engine"].as_str().unwrap_or_default();
        let Some(engine) = ENGINES.iter().find(|e| **e == engine) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let Some(behavior) = request["behavior"].as_str().and_then(Behavior::parse) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        shared.behaviors.lock().unwrap().insert(engine, behavior);
        return StatusCode::NO_CONTENT.into_response();
    }
    let (engine, q, lang) = match (method.as_str(), path) {
        ("POST", "/ddg/html/") => {
            let form = pairs(&String::from_utf8_lossy(&body));
            (
                "duckduckgo",
                form.get("q").cloned(),
                form.get("kl").cloned(),
            )
        }
        ("GET", "/brave/res/v1/web/search") => (
            "brave",
            query.get("q").cloned(),
            query.get("search_lang").cloned(),
        ),
        ("GET", "/mojeek/search") => ("mojeek", query.get("q").cloned(), query.get("lb").cloned()),
        ("GET", "/searxng/search") => (
            "searxng",
            query.get("q").cloned(),
            query.get("language").cloned(),
        ),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let Some(q) = q else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let count = query
        .get("count")
        .or_else(|| query.get("t"))
        .cloned()
        .unwrap_or_default();
    shared.requests.lock().unwrap().push(Received {
        engine,
        query: q.clone(),
        lang: lang.unwrap_or_default(),
        count,
    });
    let behavior = shared
        .behaviors
        .lock()
        .unwrap()
        .get(engine)
        .copied()
        .unwrap_or(Behavior::Ok);
    match behavior {
        Behavior::Ok => {}
        Behavior::Slow(delay) => tokio::time::sleep(delay).await,
        Behavior::Status(code) => {
            return StatusCode::from_u16(code)
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
                .into_response();
        }
        Behavior::Blocked if engine == "duckduckgo" => {
            return (
                StatusCode::ACCEPTED,
                [(header::CONTENT_TYPE, "text/html; charset=UTF-8")],
                ANOMALY_PAGE,
            )
                .into_response();
        }
        Behavior::Blocked => return StatusCode::TOO_MANY_REQUESTS.into_response(),
        Behavior::Html => {
            return (
                [(header::CONTENT_TYPE, "text/html; charset=UTF-8")],
                "<!doctype html>\n<html lang=\"en\"><body>Too many requests</body></html>",
            )
                .into_response();
        }
    }
    let found = results(engine, &q);
    match engine {
        "duckduckgo" => (
            [(header::CONTENT_TYPE, "text/html; charset=UTF-8")],
            duckduckgo_page(&q),
        )
            .into_response(),
        "brave" => {
            let key = headers
                .get("x-subscription-token")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            if key.is_empty() || key == BAD_KEY {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            let items: Vec<Value> = found
                .iter()
                .map(|(title, url, snippet)| {
                    json!({"title": title, "url": url, "description": snippet})
                })
                .collect();
            axum::Json(json!({"type": "search", "web": {"results": items}})).into_response()
        }
        "mojeek" => {
            if query.get("api_key").is_none_or(|key| key == BAD_KEY) {
                return axum::Json(json!({"response": {"status": "ERROR: Invalid key"}}))
                    .into_response();
            }
            let items: Vec<Value> = found
                .iter()
                .map(|(title, url, snippet)| json!({"title": title, "url": url, "desc": snippet}))
                .collect();
            axum::Json(json!({"response": {"status": "OK", "results": items}})).into_response()
        }
        _ => {
            if query.get("format").map(String::as_str) != Some("json") {
                return StatusCode::FORBIDDEN.into_response();
            }
            let items: Vec<Value> = found
                .iter()
                .map(|(title, url, snippet)| {
                    json!({"title": title, "url": url, "content": snippet, "engine": "fake"})
                })
                .collect();
            axum::Json(json!({"query": q, "results": items})).into_response()
        }
    }
}
