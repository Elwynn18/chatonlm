//! Faux `llama-server` (llama.cpp b12110) pour les essais de ChatonLM :
//! comportements déterministes, sans modèle ni carte graphique. Il se lance
//! comme le vrai (binaire `fake-llama`, options de `llama-server`, voir
//! [`args`] et `main.rs`) ou dans le processus d'un essai ([`FakeLlama`]).
//!
//! Formats relevés sur le vrai serveur (`.tools/research/99-llamacpp-preuve.md`
//! § 3, et relevé du lot L4) : flux SSE de `/v1/chat/completions` (premier bloc
//! `delta.role`, morceaux `delta.content` et `delta.reasoning_content`,
//! fragments `delta.tool_calls`, bloc de fin `finish_reason` puis, avec
//! `stream_options.include_usage`, bloc `usage` + `timings` sans `choices`,
//! puis `data: [DONE]`) ; réponses non-flux ; erreurs au format OpenAI
//! (`{"error": {"code", "message", "type"}}`), dont 503 « Loading model »
//! pendant le chargement, 401 « Invalid API Key », 400
//! `exceed_context_size_error` (avec `n_prompt_tokens` et `n_ctx`), 500
//! « image input is not supported » sans `--mmproj`, 404 « File Not Found ».
//!
//! Comportement selon le nom du modèle : voir [`scenario`]. Modes
//! ([`Mode`]) : `ok`, `silent` (connexions gardées sans réponse), `close`
//! (connexions fermées aussitôt), `status:<code>` (tout répond ce code, corps
//! vide), `huge` (corps de 20 Mio, bloc SSE de 3 Mio), `loading:<ms>`
//! (chargement de cette durée).
//!
//! Routes : `/health` et `/v1/health` (publiques, 503 puis 200), `/props`,
//! `/v1/models` et `/models`, `/v1/chat/completions` et `/chat/completions`,
//! `/infill`, `/slots`, `/tokenize`, `/metrics` (501 sans `--metrics`). Comme
//! le vrai : pendant le chargement, tout répond 503, avant même la clé ; puis
//! clé d'API vérifiée sur toutes, sauf la santé (`Authorization: Bearer
//! <clé>` ou `X-Api-Key` ; plusieurs clés possibles : `--api-key a,b`,
//! `LLAMA_API_KEY`, `--api-key-file`). Le chat et `/infill` attendent un
//! emplacement libre (`-np`) : une requête en file ne reçoit ni en-têtes ni
//! pings avant d'être prise, comme chez le vrai. Pilotage, sans clé : `GET /fake/requests` (corps reçus par
//! le chat, `/infill` et `/tokenize` : `{"path", "body"}`), `GET
//! /fake/disconnects` (réponses lâchées par le client avant leur fin), `GET
//! /fake/args` (arguments reçus), `GET /fake/env` (environnement reçu).
//!
//! Jetons : voir [`tokens`] (le prompt compte aussi les messages, les images
//! et les outils) ; cache du prompt simulé (préfixe commun avec la requête
//! précédente, sauf `cache_prompt: false`) ; temps factices et réguliers
//! (0,5 ms par jeton du prompt, 2 ms par jeton produit).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot, watch};
use tokio::task::JoinHandle;

pub mod args;
pub mod cli;
pub mod scenario;
pub mod scenarios;
pub mod tokens;

pub use scenario::{FIM_COMPLETION, FIM_JUNK, LOOP_SENTENCE, Scenario};

/// Version annoncée (`/props` `build_info`, `system_fingerprint`).
pub const BUILD_INFO: &str = "b12110-cd959bae9";
/// Taille d'un corps en mode `huge`, et d'un bloc SSE.
pub const HUGE_BODY: usize = 20 * 1024 * 1024;
pub const HUGE_EVENT: usize = 3 * 1024 * 1024;

/// Contexte d'entraînement annoncé (`-c 0` le prend, comme le vrai).
const CTX_TRAIN: u32 = 40960;

/// Comportement réseau du serveur.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Ok,
    /// Accepte les connexions mais ne répond jamais.
    Silent,
    /// Accepte les connexions et les ferme aussitôt.
    Close,
    /// Répond à tout avec ce code et un corps vide.
    Status(u16),
    /// Corps énormes (bornes du client).
    Huge,
    /// Chargement de cette durée (`/health` 503 pendant ce temps).
    Loading(Duration),
}

impl Mode {
    /// `ok`, `silent`, `close`, `huge`, `status:<code>`, `loading:<ms>`.
    pub fn parse(text: &str) -> Option<Mode> {
        match text {
            "" | "ok" => Some(Mode::Ok),
            "silent" => Some(Mode::Silent),
            "close" => Some(Mode::Close),
            "huge" => Some(Mode::Huge),
            other => {
                if let Some(code) = other.strip_prefix("status:") {
                    return code.parse().ok().map(Mode::Status);
                }
                let ms = other.strip_prefix("loading:")?.parse().ok()?;
                Some(Mode::Loading(Duration::from_millis(ms)))
            }
        }
    }
}

/// Réglages du serveur (ce que les options de `llama-server` décrivent).
#[derive(Clone, Debug)]
pub struct Config {
    pub mode: Mode,
    /// Chemin du modèle (`-m`) : choisit le scénario avec l'alias.
    pub model: String,
    pub alias: Option<String>,
    /// Clés acceptées (aucune : pas de vérification).
    pub api_keys: Vec<String>,
    /// Contexte d'un emplacement (`-c`).
    pub ctx: u32,
    /// Emplacements (`-np`) : réponses servies en même temps, les autres
    /// attendent.
    pub slots: u32,
    pub mmproj: Option<String>,
    pub metrics: bool,
    pub sleep_idle: Option<Duration>,
    /// Chargement, quand ni le mode ni le scénario n'en donnent un.
    pub load_delay: Duration,
    /// Journal sur la sortie d'erreur, au format du vrai.
    pub log: bool,
    pub argv: Vec<String>,
}

impl Config {
    /// Serveur prêt aussitôt, contexte de 4096, un emplacement, sans clé.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            mode: Mode::Ok,
            model: model.into(),
            alias: None,
            api_keys: Vec::new(),
            ctx: 4096,
            slots: 1,
            mmproj: None,
            metrics: true,
            sleep_idle: None,
            load_delay: Duration::ZERO,
            log: false,
            argv: Vec::new(),
        }
    }

    /// Réglages lus dans les options du binaire.
    pub fn from_args(args: &args::Args, mode: Mode) -> Self {
        let model = args
            .model
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        Self {
            mode,
            model: model.unwrap_or_default(),
            alias: args.alias.clone(),
            api_keys: args.api_keys.clone(),
            ctx: if args.ctx_size == 0 {
                CTX_TRAIN
            } else {
                args.ctx_size
            },
            slots: args.parallel.unwrap_or(4),
            mmproj: args
                .mmproj
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            metrics: args.metrics,
            sleep_idle: args.sleep_idle_seconds.map(Duration::from_secs),
            load_delay: Duration::ZERO,
            log: true,
            argv: args.argv.clone(),
        }
    }

    /// Ajoute une clé acceptée.
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_keys.push(key.into());
        self
    }

    pub fn slots(mut self, slots: u32) -> Self {
        self.slots = slots;
        self
    }

    pub fn ctx(mut self, ctx: u32) -> Self {
        self.ctx = ctx;
        self
    }

    pub fn mmproj(mut self, path: impl Into<String>) -> Self {
        self.mmproj = Some(path.into());
        self
    }

    pub fn mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    pub fn alias(mut self, alias: impl Into<String>) -> Self {
        self.alias = Some(alias.into());
        self
    }

    pub fn sleep_idle(mut self, idle: Duration) -> Self {
        self.sleep_idle = Some(idle);
        self
    }

    pub fn metrics(mut self, metrics: bool) -> Self {
        self.metrics = metrics;
        self
    }
}

/// Fin du processus demandée par un scénario.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// Sortie avec ce code (`chatonlm-loadfail` : 1).
    Code(i32),
    /// Plantage (`chatonlm-crash` : le binaire appelle `abort`).
    Abort,
}

/// État partagé par les requêtes.
struct Shared {
    config: Config,
    scenario: Scenario,
    /// Scénario d'un lot (`scenarios`), quand aucun intégré n'est nommé.
    custom: Option<&'static scenarios::Custom>,
    started: Instant,
    ready_at: Instant,
    requests: Mutex<Vec<Value>>,
    disconnects: AtomicUsize,
    processing: AtomicUsize,
    /// Emplacements libres (file d'attente des réponses).
    free_slots: Arc<Semaphore>,
    last_activity: Mutex<Instant>,
    /// Prompt de la requête précédente (cache simulé).
    cache: Mutex<Vec<u32>>,
    counter: AtomicU64,
    prompt_total: AtomicU64,
    predicted_total: AtomicU64,
    exit: watch::Sender<Option<Exit>>,
}

impl Shared {
    /// État d'un serveur qui démarre, et durée de son chargement.
    fn new(config: Config) -> (Arc<Shared>, Duration) {
        let builtin = Scenario::find(config.alias.as_deref(), &config.model);
        let custom = match builtin {
            Some(_) => None,
            None => scenarios::find(config.alias.as_deref(), &config.model),
        };
        let scenario = builtin.unwrap_or(Scenario::Simple);
        let load = match config.mode {
            Mode::Loading(delay) => delay,
            _ => scenario
                .load_delay()
                .or_else(|| {
                    custom
                        .and_then(|custom| custom.load_ms)
                        .map(Duration::from_millis)
                })
                .unwrap_or(config.load_delay),
        };
        let now = Instant::now();
        let (exit, _) = watch::channel(None);
        let shared = Arc::new(Shared {
            scenario,
            custom,
            started: now,
            ready_at: now + load,
            requests: Mutex::default(),
            disconnects: AtomicUsize::new(0),
            processing: AtomicUsize::new(0),
            free_slots: Arc::new(Semaphore::new(config.slots.max(1) as usize)),
            last_activity: Mutex::new(now),
            cache: Mutex::default(),
            counter: AtomicU64::new(1),
            prompt_total: AtomicU64::new(0),
            predicted_total: AtomicU64::new(0),
            exit,
            config,
        });
        (shared, load)
    }

    fn loading(&self) -> bool {
        Instant::now() < self.ready_at || self.fails_load()
    }

    /// Chargement raté (`chatonlm-loadfail`, ou scénario d'un lot).
    fn fails_load(&self) -> bool {
        self.scenario == Scenario::LoadFail || self.custom.is_some_and(|custom| custom.fail_load)
    }

    fn log(&self, level: char, message: &str) {
        if self.config.log {
            let elapsed = self.started.elapsed();
            let micros = elapsed.as_micros();
            eprintln!(
                "{}.{:02}.{:03}.{:03} {level} srv  {message}",
                micros / 60_000_000,
                micros / 1_000_000 % 60,
                micros / 1000 % 1000,
                micros % 1000
            );
        }
    }

    fn record(&self, path: &str, body: &Value) {
        self.requests
            .lock()
            .unwrap()
            .push(json!({ "path": path, "body": body }));
    }

    fn touch(&self) {
        *self.last_activity.lock().unwrap() = Instant::now();
    }

    fn sleeping(&self) -> bool {
        self.config
            .sleep_idle
            .is_some_and(|idle| self.last_activity.lock().unwrap().elapsed() >= idle)
            && self.processing.load(Ordering::SeqCst) == 0
    }

    /// Nom du modèle dans les réponses (alias, sinon chemin).
    fn model_name(&self) -> String {
        self.config
            .alias
            .clone()
            .unwrap_or_else(|| self.config.model.clone())
    }

    fn next_id(&self, prefix: &str) -> String {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        format!("{prefix}fake{n:028}")
    }

    /// Jetons du prompt repris du cache, et jetons à calculer (au moins un,
    /// comme le vrai quand tout le prompt est en cache).
    fn cached(&self, prompt: &[u32], use_cache: bool) -> (usize, usize) {
        let mut previous = self.cache.lock().unwrap();
        let common = if use_cache {
            previous
                .iter()
                .zip(prompt)
                .take_while(|(a, b)| a == b)
                .count()
        } else {
            0
        };
        let cache_n = common.min(prompt.len().saturating_sub(1));
        *previous = prompt.to_vec();
        (cache_n, prompt.len() - cache_n)
    }

    /// Attend un emplacement libre (sans réponse ni en-têtes en attendant).
    async fn slot(&self) -> OwnedSemaphorePermit {
        self.free_slots
            .clone()
            .acquire_owned()
            .await
            .expect("the semaphore is never closed")
    }

    fn request_exit(&self, exit: Exit) {
        self.exit.send_replace(Some(exit));
    }
}

/// Serveur lancé.
pub struct FakeLlama {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
    shared: Arc<Shared>,
}

impl FakeLlama {
    /// Démarre le serveur sur un port libre de 127.0.0.1.
    pub async fn start(config: Config) -> std::io::Result<Self> {
        Self::start_on(SocketAddr::from(([127, 0, 0, 1], 0)), config).await
    }

    pub async fn start_on(addr: SocketAddr, config: Config) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let addr = listener.local_addr()?;
        let (shared, load) = Shared::new(config);
        shared.log('I', "llama_server: initializing ...");
        shared.log(
            'I',
            &format!("  load_model: loading model '{}'", shared.config.model),
        );
        shared.log('I', &format!("server_on_li: listening on http://{addr}"));
        let (shutdown, stop) = oneshot::channel::<()>();
        let mut exit_requested = shared.exit.subscribe();
        let stopped = async move {
            tokio::select! {
                _ = stop => {}
                _ = exit_requested.wait_for(Option::is_some) => {}
            }
        };
        let task = match shared.config.mode {
            Mode::Silent => tokio::spawn(silent(listener, stopped)),
            Mode::Close => tokio::spawn(close(listener, stopped)),
            _ => {
                let app = Router::new()
                    .fallback(handle)
                    .layer(DefaultBodyLimit::disable())
                    .with_state(shared.clone());
                tokio::spawn(async move {
                    let _ = axum::serve(listener, app)
                        .with_graceful_shutdown(stopped)
                        .await;
                })
            }
        };
        let loader = shared.clone();
        tokio::spawn(async move {
            tokio::time::sleep(load).await;
            if loader.fails_load() {
                let model = &loader.config.model;
                loader.log(
                    'E',
                    &format!("  load_model: failed to load model, '{model}'"),
                );
                loader.log('E', "server_run_i: exiting due to model loading error");
                loader.request_exit(Exit::Code(1));
            } else {
                let (slots, ctx) = (loader.config.slots, loader.config.ctx);
                loader.log(
                    'I',
                    &format!("  load_model: initializing, n_slots = {slots}, n_ctx_slot = {ctx}, kv_unified = 'false'"),
                );
                loader.log('I', "server_run_i: model loaded");
            }
        });
        Ok(Self {
            addr,
            shutdown: Some(shutdown),
            task,
            shared,
        })
    }

    /// Adresse, sans barre finale : `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Scénario choisi par le nom du modèle.
    pub fn scenario(&self) -> Scenario {
        self.shared.scenario
    }

    /// Corps reçus par le chat, `/infill` et `/tokenize` : `{"path", "body"}`.
    pub fn requests(&self) -> Vec<Value> {
        self.shared.requests.lock().unwrap().clone()
    }

    /// Corps reçus par le chat seulement.
    pub fn chat_requests(&self) -> Vec<Value> {
        self.requests()
            .into_iter()
            .filter(|entry| {
                entry["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("chat/completions"))
            })
            .map(|entry| entry["body"].clone())
            .collect()
    }

    /// Réponses lâchées par le client avant leur fin (annulation).
    pub fn disconnects(&self) -> usize {
        self.shared.disconnects.load(Ordering::SeqCst)
    }

    /// Réponses en cours.
    pub fn processing(&self) -> usize {
        self.shared.processing.load(Ordering::SeqCst)
    }

    /// Fin demandée par un scénario (le serveur ne répond déjà plus).
    pub fn exit(&self) -> Option<Exit> {
        *self.shared.exit.borrow()
    }

    /// Attend une fin demandée par un scénario.
    pub async fn exit_requested(&self) -> Exit {
        let mut receiver = self.shared.exit.subscribe();
        let exit = receiver.wait_for(Option::is_some).await.map(|exit| *exit);
        match exit {
            Ok(Some(exit)) => exit,
            _ => std::future::pending().await,
        }
    }

    /// Arrête le serveur et attend la fin de sa tâche, 1 s au plus (un flux
    /// sans fin encore ouvert empêcherait l'arrêt en douceur).
    pub async fn stop(mut self) {
        self.shared
            .log('I', "operator(): operator(): cleaning up before exit...");
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if tokio::time::timeout(Duration::from_secs(1), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
        }
    }
}

impl Drop for FakeLlama {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Compte la réponse en cours et garde son emplacement ; compte un abandon
/// si elle est lâchée avant sa fin.
struct Guard {
    shared: Arc<Shared>,
    finished: bool,
    _slot: OwnedSemaphorePermit,
}

impl Guard {
    fn new(shared: Arc<Shared>, slot: OwnedSemaphorePermit) -> Self {
        shared.processing.fetch_add(1, Ordering::SeqCst);
        Self {
            shared,
            finished: false,
            _slot: slot,
        }
    }

    /// La réponse est entière : son abandon ne compte plus.
    fn finish(&mut self) {
        self.finished = true;
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.shared.processing.fetch_sub(1, Ordering::SeqCst);
        self.shared.touch();
        if !self.finished {
            self.shared.disconnects.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn json_response(status: StatusCode, body: &Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        body.to_string(),
    )
        .into_response()
}

/// Erreur au format du vrai serveur.
fn error(code: u16, kind: &str, message: &str) -> Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    json_response(
        status,
        &json!({ "error": { "code": code, "message": message, "type": kind } }),
    )
}

fn loading_error() -> Response {
    error(503, "unavailable_error", "Loading model")
}

/// Clé d'API reçue (`Authorization: Bearer <clé>` ou `X-Api-Key`), l'une
/// des clés acceptées.
fn authorized(headers: &HeaderMap, keys: &[String]) -> bool {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let api_key = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok());
    [bearer, api_key]
        .into_iter()
        .flatten()
        .any(|given| keys.iter().any(|key| key == given))
}

async fn handle(
    State(shared): State<Arc<Shared>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_owned();
    if let Some(route) = path.strip_prefix("/fake/") {
        return pilot(&shared, route);
    }
    if let Mode::Status(code) = shared.config.mode {
        let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        return status.into_response();
    }
    if path == "/health" || path == "/v1/health" {
        if shared.loading() {
            return loading_error();
        }
        return json_response(StatusCode::OK, &json!({ "status": "ok" }));
    }
    // Comme le vrai (server-http.cpp) : l'état avant la clé.
    if shared.loading() {
        return loading_error();
    }
    let keys = &shared.config.api_keys;
    if !keys.is_empty() && !authorized(&headers, keys) {
        shared.log('W', "  operator(): unauthorized: Invalid API Key");
        return error(401, "authentication_error", "Invalid API Key");
    }
    if shared.config.mode == Mode::Huge {
        return huge(&path, &body);
    }
    match (method, path.as_str()) {
        (Method::GET, "/props") => json_response(StatusCode::OK, &props(&shared)),
        (Method::GET, "/v1/models" | "/models") => json_response(StatusCode::OK, &models(&shared)),
        (Method::POST, "/v1/chat/completions" | "/chat/completions") => {
            chat(shared, &path, &body).await
        }
        (Method::POST, "/infill") => infill(shared, &body).await,
        (Method::GET, "/slots") => json_response(StatusCode::OK, &slots(&shared)),
        (Method::POST, "/tokenize") => tokenize(&shared, &body),
        (Method::GET, "/metrics") => metrics(&shared),
        _ => error(404, "not_found_error", "File Not Found"),
    }
}

/// Routes de pilotage des essais.
fn pilot(shared: &Shared, route: &str) -> Response {
    match route {
        "requests" => json_response(
            StatusCode::OK,
            &Value::Array(shared.requests.lock().unwrap().clone()),
        ),
        "disconnects" => json_response(
            StatusCode::OK,
            &json!(shared.disconnects.load(Ordering::SeqCst)),
        ),
        "args" => json_response(StatusCode::OK, &json!(shared.config.argv)),
        "env" => {
            let env: serde_json::Map<String, Value> = std::env::vars_os()
                .map(|(key, value)| {
                    (
                        key.to_string_lossy().into_owned(),
                        Value::String(value.to_string_lossy().into_owned()),
                    )
                })
                .collect();
            json_response(StatusCode::OK, &Value::Object(env))
        }
        _ => error(404, "not_found_error", "File Not Found"),
    }
}

/// Mode `huge` : corps énormes (bloc SSE sans fin de ligne pour un flux).
fn huge(path: &str, body: &[u8]) -> Response {
    let stream =
        serde_json::from_slice::<Value>(body).is_ok_and(|request| request["stream"] == true);
    if path.ends_with("chat/completions") && stream {
        let mut event = String::with_capacity(HUGE_EVENT + 16);
        event.push_str("data: {\"x\":\"");
        event.extend(std::iter::repeat_n('a', HUGE_EVENT));
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(event))
            .unwrap_or_default();
    }
    let mut text = String::with_capacity(HUGE_BODY + 16);
    text.push_str("{\"huge\":\"");
    text.extend(std::iter::repeat_n('a', HUGE_BODY));
    text.push_str("\"}");
    (
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        text,
    )
        .into_response()
}

fn props(shared: &Shared) -> Value {
    let config = &shared.config;
    json!({
        "default_generation_settings": {
            "params": {
                "seed": 4_294_967_295u64, "temperature": 0.8, "top_k": 40, "top_p": 0.95,
                "min_p": 0.05, "repeat_last_n": 64, "repeat_penalty": 1.0, "max_tokens": -1,
                "n_predict": -1, "stream": false, "chat_format": "Content-only",
                "reasoning_format": "deepseek",
            },
            "n_ctx": if shared.scenario == Scenario::OtherCtx { config.ctx / 2 } else { config.ctx },
        },
        "total_slots": config.slots,
        "model_alias": if shared.scenario == Scenario::OtherAlias {
            "someone/else-GGUF:Q8_0".to_owned()
        } else {
            shared.model_name()
        },
        "model_ftype": "Q4_K - Medium",
        "model_path": config.model,
        "modalities": { "vision": config.mmproj.is_some(), "video": false, "audio": false },
        "media_marker": "<__media__>",
        "endpoint_slots": true,
        "endpoint_props": false,
        "endpoint_metrics": config.metrics,
        "chat_template": shared.scenario.chat_template(),
        "chat_template_caps": shared.scenario.caps(),
        "bos_token": "",
        "eos_token": "<|im_end|>",
        "build_info": BUILD_INFO,
        "is_sleeping": shared.sleeping(),
    })
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn models(shared: &Shared) -> Value {
    let id = shared.model_name();
    let mut input = vec!["text"];
    if shared.config.mmproj.is_some() {
        input.push("image");
    }
    json!({
        "models": [{
            "name": id, "model": id, "modified_at": "", "size": "", "digest": "", "type": "model",
            "description": "", "tags": [""], "capabilities": ["completion"], "parameters": "",
            "details": { "parent_model": "", "format": "gguf", "family": "", "families": [""],
                "parameter_size": "", "quantization_level": "" },
        }],
        "object": "list",
        "data": [{
            "id": id, "aliases": [id], "tags": [], "object": "model",
            "architecture": { "input_modalities": input, "output_modalities": ["text"] },
            "created": now_seconds(), "owned_by": "llamacpp",
            "meta": { "vocab_type": 2, "n_vocab": 151_936, "n_ctx": shared.config.ctx,
                "n_ctx_train": CTX_TRAIN, "n_embd": 1024, "n_params": 596_049_920u64,
                "size": 390_753_280u64, "ftype": "Q4_K - Medium" },
        }],
    })
}

fn slots(shared: &Shared) -> Value {
    let busy = shared.processing.load(Ordering::SeqCst) > 0;
    Value::Array(
        (0..shared.config.slots)
            .map(|id| json!({ "id": id, "n_ctx": shared.config.ctx, "speculative": false, "is_processing": busy && id == 0 }))
            .collect(),
    )
}

fn tokenize(shared: &Shared, body: &[u8]) -> Response {
    let request: Value = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(error) => return parse_error(&error),
    };
    shared.record("/tokenize", &request);
    shared.touch();
    let text = request["content"].as_str().unwrap_or_default();
    let tokens: Vec<Value> = if request["with_pieces"] == true {
        tokens::pieces(text)
            .into_iter()
            .map(|piece| json!({ "id": tokens::id(&piece), "piece": piece }))
            .collect()
    } else {
        tokens::ids(text).into_iter().map(Value::from).collect()
    };
    json_response(StatusCode::OK, &json!({ "tokens": tokens }))
}

fn metrics(shared: &Shared) -> Response {
    if !shared.config.metrics {
        return error(
            501,
            "not_supported_error",
            "This server does not support metrics endpoint. Start it with `--metrics`",
        );
    }
    let counters = [
        (
            "prompt_tokens_total",
            "counter",
            shared.prompt_total.load(Ordering::SeqCst),
        ),
        (
            "tokens_predicted_total",
            "counter",
            shared.predicted_total.load(Ordering::SeqCst),
        ),
        (
            "requests_processing",
            "gauge",
            shared.processing.load(Ordering::SeqCst) as u64,
        ),
    ];
    let text: String = counters
        .iter()
        .map(|(name, kind, value)| format!("# HELP llamacpp:{name} {name}\n# TYPE llamacpp:{name} {kind}\nllamacpp:{name} {value}\n"))
        .collect();
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], text).into_response()
}

fn parse_error(error: &serde_json::Error) -> Response {
    self::error(
        400,
        "invalid_request_error",
        &format!("[json.exception.parse_error.101] parse error: {error}"),
    )
}

/// Jetons du prompt : outils, puis messages (balises, texte, appels
/// d'outils, images), puis début de la réponse.
fn prompt_ids(request: &Value) -> Vec<u32> {
    let mut ids = Vec::new();
    if let Some(tools) = request["tools"]
        .as_array()
        .filter(|tools| !tools.is_empty())
    {
        ids.extend(tokens::ids(&Value::Array(tools.clone()).to_string()));
    }
    for message in request["messages"].as_array().into_iter().flatten() {
        ids.extend(std::iter::repeat_n(1, tokens::PER_MESSAGE));
        ids.extend(tokens::ids(&scenario::text_of(message)));
        for call in message["tool_calls"].as_array().into_iter().flatten() {
            ids.extend(tokens::ids(&call["function"].to_string()));
        }
        let images = scenario::images(std::slice::from_ref(message));
        ids.extend(std::iter::repeat_n(2, images * tokens::PER_IMAGE));
    }
    ids.extend(std::iter::repeat_n(3, tokens::GENERATION_PROMPT));
    ids
}

/// Nombres d'une réponse.
#[derive(Clone, Copy)]
struct Stats {
    prompt: usize,
    cache_n: usize,
    prompt_n: usize,
    predicted: usize,
}

impl Stats {
    fn usage(self) -> Value {
        json!({
            "completion_tokens": self.predicted,
            "prompt_tokens": self.prompt,
            "total_tokens": self.prompt + self.predicted,
            "prompt_tokens_details": { "cached_tokens": self.cache_n },
        })
    }

    fn timings(self) -> Value {
        let prompt_ms = self.prompt_n as f64 * 0.5;
        let predicted_ms = self.predicted as f64 * 2.0;
        let per = |n: usize, ms: f64| if n == 0 { 0.0 } else { ms / n as f64 };
        let rate = |n: usize, ms: f64| {
            if ms == 0.0 {
                0.0
            } else {
                n as f64 * 1000.0 / ms
            }
        };
        json!({
            "cache_n": self.cache_n,
            "prompt_n": self.prompt_n,
            "prompt_ms": prompt_ms,
            "prompt_per_token_ms": per(self.prompt_n, prompt_ms),
            "prompt_per_second": rate(self.prompt_n, prompt_ms),
            "predicted_n": self.predicted,
            "predicted_ms": predicted_ms,
            "predicted_per_token_ms": per(self.predicted, predicted_ms),
            "predicted_per_second": rate(self.predicted, predicted_ms),
        })
    }
}

/// Type JSON d'une valeur, comme le message du vrai le nomme.
fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

async fn chat(shared: Arc<Shared>, path: &str, body: &[u8]) -> Response {
    let request: Value = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(error) => return parse_error(&error),
    };
    shared.record(path, &request);
    shared.touch();
    let Some(messages) = request["messages"].as_array() else {
        return error(400, "invalid_request_error", "'messages' is required");
    };
    if let Some(value) = request["chat_template_kwargs"].get("enable_thinking")
        && !value.is_boolean()
    {
        let message = format!(
            "invalid type for \"enable_thinking\" (expected boolean, got {})",
            kind_of(value)
        );
        return error(400, "invalid_request_error", &message);
    }
    let vision = shared.config.mmproj.is_some();
    if scenario::images(messages) > 0 && !vision {
        let message = "image input is not supported - hint: if this is unexpected, you may need to provide the mmproj";
        shared.log('W', &format!("  operator(): got exception: {message}"));
        return error(500, "server_error", message);
    }
    // WebP : b12110 ne le lit qu'avec un `ffmpeg` du PATH (`mtmd-helper`) ;
    // sans lui, la requête est refusée (cas de Windows).
    if vision && scenario::webp_images(messages) > 0 {
        let message = "Failed to load image or audio file";
        shared.log('W', &format!("  operator(): got exception: {message}"));
        return error(500, "server_error", message);
    }
    // En file jusqu'à un emplacement libre (le contexte se vérifie dans
    // l'emplacement, chez le vrai).
    let slot = shared.slot().await;
    let prompt = prompt_ids(&request);
    let ctx = shared.config.ctx as usize;
    if prompt.len() >= ctx {
        let message = format!(
            "request ({} tokens) exceeds the available context size ({ctx} tokens), try increasing it",
            prompt.len()
        );
        shared.log(
            'E',
            &format!("   send_error: task id = 0, error: {message}"),
        );
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": { "code": 400, "message": message, "type": "exceed_context_size_error",
                "n_prompt_tokens": prompt.len(), "n_ctx": ctx } }),
        );
    }
    if let Some(custom) = shared.custom {
        let model = shared.model_name();
        let script = scenarios::limit(
            (custom.chat)(&scenarios::ChatCall {
                body: &request,
                model: &model,
            }),
            &request,
            ctx - prompt.len(),
        );
        let (cache_n, prompt_n) = shared.cached(&prompt, request["cache_prompt"] != false);
        let stats = Stats {
            prompt: prompt.len(),
            cache_n,
            prompt_n,
            predicted: script.iter().filter(|step| step.produces()).count(),
        };
        return if request["stream"] == true {
            let usage = request["stream_options"]["include_usage"] == true;
            script_stream(shared, slot, script, stats, usage).await
        } else {
            script_once(shared, slot, script, stats).await
        };
    }
    let reply = scenario::limit(
        scenario::plan(shared.scenario, &request, vision),
        &request,
        ctx - prompt.len(),
    );
    let (cache_n, prompt_n) = shared.cached(&prompt, request["cache_prompt"] != false);
    let predicted = reply.reasoning.len()
        + reply.content.len()
        + reply
            .calls
            .iter()
            .map(|call| call.fragments.len())
            .sum::<usize>();
    let stats = Stats {
        prompt: prompt.len(),
        cache_n,
        prompt_n,
        predicted,
    };
    shared
        .prompt_total
        .fetch_add(prompt_n as u64, Ordering::SeqCst);
    shared
        .predicted_total
        .fetch_add(predicted as u64, Ordering::SeqCst);
    if request["stream"] == true {
        let usage = request["stream_options"]["include_usage"] == true;
        stream(shared, slot, reply, stats, usage)
    } else {
        once(shared, slot, reply, stats).await
    }
}

/// Étape d'un flux.
enum Step {
    Send(String),
    Wait(Duration),
    /// Plus rien, connexion gardée.
    Hang,
    /// Ping SSE (commentaire vide), comme le vrai pendant un long prompt.
    Ping,
    /// Connexion coupée par le serveur, puis plantage.
    Crash,
    /// Connexion coupée par le serveur (scénario d'un lot), qui continue.
    Cut,
}

fn data(value: &Value) -> String {
    format!("data: {value}\n\n")
}

fn stream(
    shared: Arc<Shared>,
    slot: OwnedSemaphorePermit,
    reply: scenario::Reply,
    stats: Stats,
    usage: bool,
) -> Response {
    let id = shared.next_id("chatcmpl-");
    let created = now_seconds();
    let model = shared.model_name();
    let chunk = |delta: Value, finish: Option<&str>| {
        json!({
            "choices": [{ "finish_reason": finish, "index": 0, "delta": delta }],
            "created": created, "id": id, "model": model,
            "system_fingerprint": BUILD_INFO, "object": "chat.completion.chunk",
        })
    };
    let mut steps = vec![Step::Send(data(&chunk(
        json!({ "role": "assistant", "content": null }),
        None,
    )))];
    let piece = |steps: &mut Vec<Step>, delta: Value| {
        if !reply.delay.is_zero() {
            steps.push(Step::Wait(reply.delay));
        }
        steps.push(Step::Send(data(&chunk(delta, None))));
    };
    if reply.end == scenario::End::Pings {
        for _ in 0..scenario::PINGS {
            steps.push(Step::Wait(scenario::PING_EVERY));
            steps.push(Step::Ping);
        }
    }
    if matches!(reply.end, scenario::End::Hang | scenario::End::Pings) {
        steps.push(Step::Hang);
    }
    for text in &reply.reasoning {
        piece(&mut steps, json!({ "reasoning_content": text }));
    }
    for text in &reply.content {
        piece(&mut steps, json!({ "content": text }));
    }
    for (index, call) in reply.calls.iter().enumerate() {
        for (n, fragment) in call.fragments.iter().enumerate() {
            let mut entry = json!({ "index": index, "function": { "arguments": fragment } });
            if n == 0 {
                entry["type"] = json!("function");
                entry["function"]["name"] = json!(call.name);
                if let Some(id) = &call.id {
                    entry["id"] = json!(id);
                }
            }
            piece(&mut steps, json!({ "tool_calls": [entry] }));
        }
    }
    match reply.end {
        scenario::End::Crash => steps.push(Step::Crash),
        scenario::End::Error => steps.push(Step::Send(data(&json!({ "error": {
            "code": 500, "message": "fake failure in the middle of the reply", "type": "server_error" } })))),
        _ => {
            let mut end = chunk(json!({}), Some(reply.finish));
            if usage {
                steps.push(Step::Send(data(&end)));
                let mut last = chunk(json!({}), None);
                last["choices"] = json!([]);
                last["usage"] = stats.usage();
                last["timings"] = stats.timings();
                steps.push(Step::Send(data(&last)));
            } else {
                end["timings"] = stats.timings();
                steps.push(Step::Send(data(&end)));
            }
            steps.push(Step::Send("data: [DONE]\n\n".into()));
        }
    }
    sse(shared, slot, steps)
}

/// Retard au-delà duquel les attentes d'un flux ne sont plus rattrapées
/// (lecteur lent : pas de rafale au-delà).
const MAX_LAG: Duration = Duration::from_millis(100);

/// Réponse SSE qui joue `steps`.
///
/// Les attentes sont des échéances cumulées, pas des durées : sous Windows,
/// une attente de 5 ms en dure une quinzaine (pas de l'horloge du système),
/// et le rythme moyen d'un flux (`chatonlm-loop`) doit rester celui demandé
/// sur les deux systèmes.
fn sse(shared: Arc<Shared>, slot: OwnedSemaphorePermit, steps: Vec<Step>) -> Response {
    let guard = Guard::new(shared, slot);
    let due: Option<tokio::time::Instant> = None;
    let body = futures_util::stream::unfold(
        (steps.into_iter(), guard, due),
        |(mut steps, mut guard, mut due)| async move {
            loop {
                match steps.next() {
                    None => {
                        guard.finish();
                        return None;
                    }
                    Some(Step::Wait(delay)) => {
                        let now = tokio::time::Instant::now();
                        let start = due.filter(|due| *due + MAX_LAG >= now).unwrap_or(now);
                        let next = start + delay;
                        due = Some(next);
                        tokio::time::sleep_until(next).await;
                    }
                    Some(Step::Hang) => std::future::pending::<()>().await,
                    Some(Step::Ping) => {
                        return Some((Ok(":\n\n".to_owned()), (steps, guard, due)));
                    }
                    Some(Step::Crash) => {
                        guard.finish();
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        guard
                            .shared
                            .log('E', "fake-llama: crash in the middle of the reply");
                        guard.shared.request_exit(Exit::Abort);
                        let reset =
                            std::io::Error::new(std::io::ErrorKind::ConnectionReset, "fake crash");
                        return Some((Err(reset), (steps, guard, due)));
                    }
                    Some(Step::Cut) => {
                        guard.finish();
                        let reset =
                            std::io::Error::new(std::io::ErrorKind::ConnectionReset, "fake cut");
                        return Some((Err(reset), (steps, guard, due)));
                    }
                    Some(Step::Send(text)) => {
                        if steps.as_slice().is_empty() {
                            guard.finish();
                        }
                        return Some((Ok::<_, std::io::Error>(text), (steps, guard, due)));
                    }
                }
            }
        },
    );
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(body))
        .unwrap_or_default()
}

async fn once(
    shared: Arc<Shared>,
    slot: OwnedSemaphorePermit,
    reply: scenario::Reply,
    stats: Stats,
) -> Response {
    let mut guard = Guard::new(shared.clone(), slot);
    let pieces = reply.reasoning.len() + reply.content.len();
    tokio::time::sleep(reply.delay * pieces as u32).await;
    match reply.end {
        // Pas de ping hors flux.
        scenario::End::Hang | scenario::End::Pings => std::future::pending::<()>().await,
        scenario::End::Crash => {
            guard.finish();
            shared.request_exit(Exit::Abort);
            let reset = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "fake crash");
            let body = futures_util::stream::once(async move { Err::<String, _>(reset) });
            return Response::builder()
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from_stream(body))
                .unwrap_or_default();
        }
        scenario::End::Error => {
            guard.finish();
            return error(
                500,
                "server_error",
                "fake failure in the middle of the reply",
            );
        }
        scenario::End::Normal => {}
    }
    guard.finish();
    let mut message = json!({ "role": "assistant", "content": reply.content.concat() });
    if !reply.reasoning.is_empty() {
        message["reasoning_content"] = json!(reply.reasoning.concat());
    }
    if !reply.calls.is_empty() {
        message["content"] = json!("");
        message["tool_calls"] = reply
            .calls
            .iter()
            .enumerate()
            .map(|(index, call)| {
                let mut entry = json!({ "type": "function",
                    "function": { "name": call.name, "arguments": call.fragments.concat() } });
                entry["id"] = json!(call.id.clone().unwrap_or_else(|| format!("fakeid{index}")));
                entry
            })
            .collect();
    }
    json_response(
        StatusCode::OK,
        &json!({
            "choices": [{ "finish_reason": reply.finish, "index": 0, "message": message }],
            "created": now_seconds(), "model": shared.model_name(),
            "system_fingerprint": BUILD_INFO, "object": "chat.completion",
            "usage": stats.usage(), "id": shared.next_id("chatcmpl-"), "timings": stats.timings(),
        }),
    )
}

/// Réponse en flux d'un script ([`scenarios`]).
async fn script_stream(
    shared: Arc<Shared>,
    slot: OwnedSemaphorePermit,
    script: scenarios::Script,
    stats: Stats,
    usage: bool,
) -> Response {
    // Échec avant tout morceau : réponse HTTP d'erreur, après les attentes.
    if let Some((waited, status, body)) = early_failure(&script) {
        tokio::time::sleep(waited).await;
        drop(slot);
        return script_error(status, &body);
    }
    let id = shared.next_id("chatcmpl-");
    let created = now_seconds();
    let model = shared.model_name();
    let chunk = |delta: Value, finish: Option<&str>| {
        json!({
            "choices": [{ "finish_reason": finish, "index": 0, "delta": delta }],
            "created": created, "id": id, "model": model,
            "system_fingerprint": BUILD_INFO, "object": "chat.completion.chunk",
        })
    };
    let mut steps = vec![Step::Send(data(&chunk(
        json!({ "role": "assistant", "content": null }),
        None,
    )))];
    let mut calls = 0;
    let mut finish = None;
    let mut ended = false;
    for step in script {
        match step {
            scenarios::Step::Reasoning(text) => steps.push(Step::Send(data(&chunk(
                json!({ "reasoning_content": text }),
                None,
            )))),
            scenarios::Step::Content(text) => {
                steps.push(Step::Send(data(&chunk(json!({ "content": text }), None))));
            }
            scenarios::Step::ToolCall { name, arguments } => {
                let entry = json!({ "index": calls, "id": format!("fake-call-{calls}"),
                    "type": "function", "function": { "name": name, "arguments": arguments } });
                calls += 1;
                steps.push(Step::Send(data(&chunk(
                    json!({ "tool_calls": [entry] }),
                    None,
                ))));
            }
            scenarios::Step::Wait(delay) => steps.push(Step::Wait(delay)),
            scenarios::Step::Fail { body, .. } => {
                steps.push(Step::Send(data(&body)));
                ended = true;
                break;
            }
            scenarios::Step::Cut => {
                steps.push(Step::Cut);
                ended = true;
                break;
            }
            scenarios::Step::Finish(reason) => {
                finish = Some(reason);
                break;
            }
        }
    }
    if !ended {
        let reason = finish.unwrap_or(if calls > 0 { "tool_calls" } else { "stop" });
        let mut end = chunk(json!({}), Some(reason));
        if usage {
            steps.push(Step::Send(data(&end)));
            let mut last = chunk(json!({}), None);
            last["choices"] = json!([]);
            last["usage"] = stats.usage();
            last["timings"] = stats.timings();
            steps.push(Step::Send(data(&last)));
        } else {
            end["timings"] = stats.timings();
            steps.push(Step::Send(data(&end)));
        }
        steps.push(Step::Send("data: [DONE]\n\n".into()));
    }
    sse(shared, slot, steps)
}

/// Échec d'un script avant tout morceau : attente cumulée, code et corps.
fn early_failure(script: &[scenarios::Step]) -> Option<(Duration, u16, Value)> {
    let mut waited = Duration::ZERO;
    for step in script {
        match step {
            scenarios::Step::Wait(delay) => waited += *delay,
            scenarios::Step::Fail { status, body } => {
                return Some((waited, *status, body.clone()));
            }
            _ => return None,
        }
    }
    None
}

fn script_error(status: u16, body: &Value) -> Response {
    json_response(
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        body,
    )
}

/// Connexion coupée en guise de corps.
fn cut_body() -> Response {
    let reset = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "fake cut");
    let body = futures_util::stream::once(async move { Err::<String, _>(reset) });
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from_stream(body))
        .unwrap_or_default()
}

/// Réponse entière d'un script ([`scenarios`]).
async fn script_once(
    shared: Arc<Shared>,
    slot: OwnedSemaphorePermit,
    script: scenarios::Script,
    stats: Stats,
) -> Response {
    let mut guard = Guard::new(shared.clone(), slot);
    let (mut reasoning, mut content) = (String::new(), String::new());
    let mut calls = Vec::new();
    let mut finish = None;
    for step in script {
        match step {
            scenarios::Step::Reasoning(text) => reasoning.push_str(&text),
            scenarios::Step::Content(text) => content.push_str(&text),
            scenarios::Step::ToolCall { name, arguments } => {
                calls.push(
                    json!({ "type": "function", "id": format!("fake-call-{}", calls.len()),
                    "function": { "name": name, "arguments": arguments } }),
                );
            }
            scenarios::Step::Wait(delay) => tokio::time::sleep(delay).await,
            scenarios::Step::Fail { status, body } => {
                guard.finish();
                return script_error(status, &body);
            }
            scenarios::Step::Cut => {
                guard.finish();
                return cut_body();
            }
            scenarios::Step::Finish(reason) => {
                finish = Some(reason);
                break;
            }
        }
    }
    guard.finish();
    let reason = finish.unwrap_or(if calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    });
    let mut message = json!({ "role": "assistant", "content": content });
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }
    if !calls.is_empty() {
        message["tool_calls"] = json!(calls);
    }
    json_response(
        StatusCode::OK,
        &json!({
            "choices": [{ "finish_reason": reason, "index": 0, "message": message }],
            "created": now_seconds(), "model": shared.model_name(),
            "system_fingerprint": BUILD_INFO, "object": "chat.completion",
            "usage": stats.usage(), "id": shared.next_id("chatcmpl-"), "timings": stats.timings(),
        }),
    )
}

/// `/infill` d'un scénario de lot : contenu des morceaux, raison de fin
/// (`eos` par défaut ; `Finish("limit")`, `Finish("word")`…).
async fn script_infill(
    shared: Arc<Shared>,
    mut guard: Guard,
    script: scenarios::Script,
    evaluated: usize,
) -> Response {
    let mut content = String::new();
    let mut stop_type = "eos";
    let mut pieces = 0;
    for step in script {
        match step {
            scenarios::Step::Content(text) | scenarios::Step::Reasoning(text) => {
                content.push_str(&text);
                pieces += 1;
            }
            scenarios::Step::ToolCall { .. } => {}
            scenarios::Step::Wait(delay) => tokio::time::sleep(delay).await,
            scenarios::Step::Fail { status, body } => {
                guard.finish();
                return script_error(status, &body);
            }
            scenarios::Step::Cut => {
                guard.finish();
                return cut_body();
            }
            scenarios::Step::Finish(reason) => {
                stop_type = reason;
                break;
            }
        }
    }
    let stats = Stats {
        prompt: evaluated,
        cache_n: 0,
        prompt_n: evaluated,
        predicted: pieces,
    };
    guard.finish();
    json_response(
        StatusCode::OK,
        &json!({
            "index": 0, "content": content, "tokens": [], "id_slot": 0, "stop": true,
            "model": shared.model_name(), "tokens_predicted": pieces,
            "tokens_evaluated": evaluated, "prompt": "", "has_new_line": content.contains('\n'),
            "truncated": false, "stop_type": stop_type, "stopping_word": "",
            "tokens_cached": evaluated + pieces, "timings": stats.timings(),
        }),
    )
}

async fn infill(shared: Arc<Shared>, body: &[u8]) -> Response {
    let request: Value = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(error) => return parse_error(&error),
    };
    shared.record("/infill", &request);
    shared.touch();
    let slot = shared.slot().await;
    let mut guard = Guard::new(shared.clone(), slot);
    if let Some(script) = shared.custom.and_then(|custom| custom.infill) {
        let model = shared.model_name();
        let script = script(&scenarios::InfillCall {
            body: &request,
            model: &model,
        });
        let prefix = request["input_prefix"].as_str().unwrap_or_default();
        let suffix = request["input_suffix"].as_str().unwrap_or_default();
        let evaluated = tokens::ids(&format!("{prefix}{suffix}")).len();
        return script_infill(shared.clone(), guard, script, evaluated).await;
    }
    if shared.scenario == Scenario::FimSlow {
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let prefix = request["input_prefix"].as_str().unwrap_or_default();
    let suffix = request["input_suffix"].as_str().unwrap_or_default();
    let mut prompt = String::new();
    if shared.scenario.fim() {
        for chunk in request["input_extra"].as_array().into_iter().flatten() {
            let filename = chunk["filename"].as_str().unwrap_or_default();
            let text = chunk["text"].as_str().unwrap_or_default();
            prompt.push_str(&format!("<|file_sep|>{filename}\n{text}"));
        }
    }
    prompt.push_str(&format!(
        "<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>"
    ));
    let mut content = if shared.scenario.fim() {
        FIM_COMPLETION
    } else {
        FIM_JUNK
    }
    .to_owned();
    let mut stop_type = "eos";
    let mut stopping_word = String::new();
    let stops: Vec<&str> = request["stop"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|stop| !stop.is_empty())
        .collect();
    if let Some((cut, word)) = stops
        .iter()
        .filter_map(|stop| content.find(stop).map(|at| (at, *stop)))
        .min()
    {
        content.truncate(cut);
        stop_type = "word";
        stopping_word = word.to_owned();
    }
    let mut pieces = tokens::pieces(&content);
    if let Some(limit) = request["n_predict"]
        .as_i64()
        .and_then(|n| usize::try_from(n).ok())
        && pieces.len() > limit
    {
        pieces.truncate(limit);
        content = pieces.concat();
        stop_type = "limit";
    }
    let evaluated = tokens::ids(&prompt).len();
    let (cache_n, prompt_n) =
        shared.cached(&tokens::ids(&prompt), request["cache_prompt"] != false);
    let stats = Stats {
        prompt: evaluated,
        cache_n,
        prompt_n,
        predicted: pieces.len(),
    };
    guard.finish();
    json_response(
        StatusCode::OK,
        &json!({
            "index": 0, "content": content, "tokens": [], "id_slot": 0, "stop": true,
            "model": shared.model_name(), "tokens_predicted": pieces.len(),
            "tokens_evaluated": evaluated, "prompt": prompt, "has_new_line": content.contains('\n'),
            "truncated": false, "stop_type": stop_type, "stopping_word": stopping_word,
            "tokens_cached": evaluated + pieces.len(), "timings": stats.timings(),
        }),
    )
}

async fn close<F: Future<Output = ()>>(listener: TcpListener, stopped: F) {
    tokio::pin!(stopped);
    loop {
        tokio::select! {
            accepted = listener.accept() => drop(accepted),
            _ = &mut stopped => break,
        }
    }
}

/// Garde les connexions ouvertes sans jamais répondre.
async fn silent<F: Future<Output = ()>>(listener: TcpListener, stopped: F) {
    tokio::pin!(stopped);
    let mut held = Vec::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((socket, _)) = accepted {
                    held.push(socket);
                }
            }
            _ = &mut stopped => break,
        }
    }
}

#[cfg(test)]
mod tests;
