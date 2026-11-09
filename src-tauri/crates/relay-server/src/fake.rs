//! Faux fournisseurs (Brave et Mojeek sur un même serveur local, aux chemins
//! de leurs vraies API) et relais d'essai lancé dans le processus. Essais
//! seulement (fonctionnalité `fake`) : aucune vraie clé, aucun appel réseau.

use crate::config::{Config, Secret, Secrets, Tiers};
use crate::log::{Clock, Log};
use crate::server::{self, Relay};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use relay_proto::api::Provider;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Clés des essais : cherchées ensuite dans les journaux et la base.
pub const BRAVE_TEST_KEY: &str = "BSA-test-secret-7f3a9c";
pub const MOJEEK_TEST_KEY: &str = "mojeek-test-secret-41d2e8";

/// Comportement d'un faux fournisseur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behavior {
    /// Douze résultats (balises, entités, une adresse `javascript:`, un doublon).
    Ok,
    /// Aucun résultat.
    Empty,
    /// Ce code HTTP, avec un corps qui recopie la clé reçue (un relais qui
    /// journaliserait les corps la laisserait fuir).
    Status(u16),
    /// Mojeek : `status` « ERROR: Daily Limit Reached. » (HTTP 200).
    DailyLimit,
    /// Corps qui n'est pas du JSON.
    Garbage,
    /// Répond après ce délai.
    Slow(Duration),
    /// Titres et extraits où `&` précède du texte non ASCII (B1).
    Unicode,
}

/// Titres de [`Behavior::Unicode`].
pub const UNICODE_TITLES: [&str; 2] = ["Tom & Jérôme à Paris", "Q&Aの使い方"];

#[derive(Clone, Debug)]
pub struct FakeRequest {
    pub provider: Provider,
    pub params: HashMap<String, String>,
    pub token: Option<String>,
}

#[derive(Default)]
struct FakeState {
    brave: Mutex<Option<Behavior>>,
    mojeek: Mutex<Option<Behavior>>,
    requests: Mutex<Vec<FakeRequest>>,
}

pub struct FakeProviders {
    pub url: String,
    state: Arc<FakeState>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeProviders {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeProviders {
    pub async fn start() -> Self {
        let state = Arc::new(FakeState::default());
        let app = Router::new()
            .route("/res/v1/web/search", get(brave))
            .route("/search", get(mojeek))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { url, state, task }
    }

    pub fn set(&self, provider: Provider, behavior: Behavior) {
        let slot = match provider {
            Provider::Brave => &self.state.brave,
            Provider::Mojeek => &self.state.mojeek,
        };
        *slot.lock().expect("fake") = Some(behavior);
    }

    pub fn requests(&self) -> Vec<FakeRequest> {
        self.state.requests.lock().expect("fake").clone()
    }

    pub fn count(&self, provider: Provider) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.provider == provider)
            .count()
    }
}

fn behavior(slot: &Mutex<Option<Behavior>>) -> Behavior {
    slot.lock().expect("fake").unwrap_or(Behavior::Ok)
}

fn results(query: &str) -> Vec<(String, String, String)> {
    let mut items: Vec<_> = (1..=10)
        .map(|n| {
            (
                format!("Résultat <strong>{n}</strong> pour {query}"),
                format!("https://example.org/{n}"),
                format!("Extrait {n} &amp; <strong>{query}</strong>"),
            )
        })
        .collect();
    items.insert(
        1,
        ("Piège".into(), "javascript:alert(1)".into(), "x".into()),
    );
    items.insert(
        3,
        ("Doublon".into(), "https://example.org/1".into(), "x".into()),
    );
    items
}

fn unicode_results() -> Vec<(String, String, String)> {
    UNICODE_TITLES
        .iter()
        .enumerate()
        .map(|(n, title)| {
            (
                (*title).to_owned(),
                format!("https://example.org/u{n}"),
                format!("{title} – R&D à l'été"),
            )
        })
        .collect()
}

async fn misbehave(behavior: Behavior, key: &str) -> Option<Response> {
    match behavior {
        Behavior::Status(code) => Some(
            (
                StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                format!("invalid subscription token {key}"),
            )
                .into_response(),
        ),
        Behavior::Garbage => Some((StatusCode::OK, "<html>not json</html>").into_response()),
        Behavior::Slow(delay) => {
            tokio::time::sleep(delay).await;
            None
        }
        Behavior::Ok | Behavior::Empty | Behavior::DailyLimit | Behavior::Unicode => None,
    }
}

async fn brave(
    State(state): State<Arc<FakeState>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let token = headers
        .get("x-subscription-token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    state.requests.lock().expect("fake").push(FakeRequest {
        provider: Provider::Brave,
        params: params.clone(),
        token: token.clone(),
    });
    let behavior = behavior(&state.brave);
    if let Some(response) = misbehave(behavior, token.as_deref().unwrap_or_default()).await {
        return response;
    }
    if token.as_deref() != Some(BRAVE_TEST_KEY) {
        return (StatusCode::UNAUTHORIZED, "bad token").into_response();
    }
    let query = params.get("q").cloned().unwrap_or_default();
    let count: usize = params
        .get("count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(20);
    let items = match behavior {
        Behavior::Empty => Vec::new(),
        Behavior::Unicode => unicode_results(),
        _ => results(&query),
    };
    let web: Vec<Value> = items
        .into_iter()
        .take(count + 2)
        .map(|(title, url, description)| json!({ "title": title, "url": url, "description": description }))
        .collect();
    Json(json!({ "query": { "original": query }, "web": { "results": web } })).into_response()
}

async fn mojeek(
    State(state): State<Arc<FakeState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let key = params.get("api_key").cloned();
    state.requests.lock().expect("fake").push(FakeRequest {
        provider: Provider::Mojeek,
        params: params.clone(),
        token: key.clone(),
    });
    let behavior = behavior(&state.mojeek);
    if let Some(response) = misbehave(behavior, key.as_deref().unwrap_or_default()).await {
        return response;
    }
    if behavior == Behavior::DailyLimit {
        return Json(json!({ "response": { "status": "ERROR: Daily Limit Reached." } }))
            .into_response();
    }
    if key.as_deref() != Some(MOJEEK_TEST_KEY) {
        return Json(json!({ "response": { "status": "ERROR: Invalid API key" } })).into_response();
    }
    let query = params.get("q").cloned().unwrap_or_default();
    let items = match behavior {
        Behavior::Empty => Vec::new(),
        Behavior::Unicode => unicode_results(),
        _ => results(&query),
    };
    let results: Vec<Value> = items
        .into_iter()
        .map(|(title, url, desc)| json!({ "url": url, "title": title, "desc": desc, "score": 1.0 }))
        .collect();
    Json(json!({ "response": { "status": "OK", "head": { "query": query }, "results": results } }))
        .into_response()
}

/// Configuration d'essai : preuve de travail minuscule, fournisseurs factices.
pub fn test_config(database: &Path, providers_url: &str) -> Config {
    let mut config: Config =
        toml::from_str(&format!("database = {:?}", database.to_string_lossy()))
            .expect("test config");
    config.pow.memory_kib = 8;
    config.pow.bits = 1;
    config.pow.count = 2;
    config.providers.brave.base_url = providers_url.to_owned();
    config.providers.mojeek.base_url = providers_url.to_owned();
    config.providers.brave.timeout_secs = 2;
    config.providers.mojeek.timeout_secs = 2;
    // Les essais s'enchaînent vite : pas de mise à l'écart d'un fournisseur.
    config.providers.brave.cooldown_secs = 0;
    config.providers.mojeek.cooldown_secs = 0;
    config.quota.per_ip_per_minute = Tiers([10_000; 3]);
    config.quota.per_key_per_minute = 10_000;
    config.quota.per_ip_daily_registrations = Tiers([100; 3]);
    config.quota.per_ip_daily_challenges = Tiers([100; 3]);
    config
}

pub fn test_secrets() -> Secrets {
    Secrets {
        brave_api_key: Secret::new(BRAVE_TEST_KEY),
        mojeek_api_key: Secret::new(MOJEEK_TEST_KEY),
        challenge_key: Secret::default(),
    }
}

/// Relais lancé dans le processus, horloge réglable, journal capturé.
pub struct TestRelay {
    pub url: String,
    pub clock: Arc<AtomicU64>,
    pub logs: Arc<Mutex<Vec<String>>>,
    pub relay: Arc<Relay>,
    pub fake: FakeProviders,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl TestRelay {
    /// `start` : heure du relais (Unix) ; `tweak` règle la configuration.
    pub async fn start(database: &Path, start: u64, tweak: impl FnOnce(&mut Config)) -> Self {
        let fake = FakeProviders::start().await;
        Self::start_with(database, start, fake, tweak).await
    }

    /// Avec des faux fournisseurs déjà lancés (redémarrage du relais).
    pub async fn start_with(
        database: &Path,
        start: u64,
        fake: FakeProviders,
        tweak: impl FnOnce(&mut Config),
    ) -> Self {
        let mut config = test_config(database, &fake.url);
        tweak(&mut config);
        let (clock, handle) = Clock::manual(start);
        let (log, logs) = Log::capturing();
        let relay = Relay::new(config, test_secrets(), clock, log).expect("test relay");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let (shutdown, stop) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn({
            let relay = relay.clone();
            async move {
                let _ = server::serve(relay, listener, async move {
                    let _ = stop.await;
                })
                .await;
            }
        });
        Self {
            url,
            clock: handle,
            logs,
            relay,
            fake,
            shutdown: Some(shutdown),
            task,
        }
    }

    pub fn set_time(&self, unix: u64) {
        self.clock.store(unix, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn advance(&self, secs: u64) {
        self.clock
            .fetch_add(secs, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn log_lines(&self) -> Vec<String> {
        self.logs.lock().expect("logs").clone()
    }

    /// Arrête le relais et rend ses faux fournisseurs (pour un redémarrage).
    pub async fn stop(mut self) -> FakeProviders {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let task = std::mem::replace(&mut self.task, tokio::spawn(async {}));
        let _ = task.await;
        // Le relais est rendu par la dernière référence (base fermée).
        let TestRelay { fake, .. } = self;
        fake
    }
}
