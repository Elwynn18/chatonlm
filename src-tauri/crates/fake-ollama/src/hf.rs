//! Faux Hugging Face pour les tests du ModelHub : API des modèles (liste,
//! recherche, tri, curseur `Link`, ETag), fiche avec tailles, README (`raw/`),
//! en-tête GGUF synthétique (`resolve/`, requêtes Range), registre d'Ollama
//! (`/v2/<auteur>/<dépôt>/manifests/<étiquette>`, erreurs exactes du vrai) et
//! page d'état (`/index.json`). Serveur à part du faux Ollama, sur son propre
//! port. Dépôts : `fixtures/hf/repos.json` (réduits des échantillons réels de
//! `.tools/research/35-samples`). Ce que le serveur a reçu se lit sur
//! `GET /fake/requests`.
//!
//! Modes : `hf` (normal), `hf-status:<code>` (toutes les routes répondent ce
//! code ; page d'état « incident » pour un 5xx), `hf-ratelimit` (429 et
//! `RateLimit: "api";r=0;t=42`), `hf-html` (200 et une page HTML),
//! `hf-silent` (connexion acceptée, jamais de réponse ; page d'état
//! « incident »).

use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Comportement du faux Hugging Face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HfMode {
    /// Répond comme Hugging Face, avec les dépôts de `fixtures/hf`.
    Normal,
    /// Toutes les routes répondent ce code ; pour un 5xx, la page d'état
    /// signale un incident.
    Status(u16),
    /// Toutes les routes répondent 429 avec `RateLimit: "api";r=0;t=<reset>`.
    RateLimited { reset: u32 },
    /// Toutes les routes répondent 200 avec une page HTML (CDN en panne).
    Html,
    /// Les connexions sont acceptées mais rien ne répond (délai dépassé),
    /// sauf la page d'état, qui signale un incident.
    Silent,
}

impl HfMode {
    /// Mode d'un argument de la ligne de commande (`hf`, `hf-status:503`,
    /// `hf-ratelimit`, `hf-html`, `hf-silent`).
    pub fn parse(argument: &str) -> Option<Self> {
        match argument {
            "hf" => Some(HfMode::Normal),
            "hf-ratelimit" => Some(HfMode::RateLimited { reset: 42 }),
            "hf-html" => Some(HfMode::Html),
            "hf-silent" => Some(HfMode::Silent),
            other => other
                .strip_prefix("hf-status:")?
                .parse()
                .ok()
                .map(HfMode::Status),
        }
    }
}

/// Requête reçue (hors `/fake/...`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HfRequest {
    pub method: String,
    /// Chemin et requête.
    pub path: String,
    pub if_none_match: Option<String>,
    pub range: Option<String>,
}

struct Shared {
    mode: HfMode,
    delay: Duration,
    addr: SocketAddr,
    requests: Mutex<Vec<HfRequest>>,
}

pub struct FakeHf {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
    shared: Arc<Shared>,
}

impl FakeHf {
    /// Démarre le serveur sur un port libre de 127.0.0.1.
    pub async fn start(mode: HfMode) -> std::io::Result<Self> {
        Self::start_with(SocketAddr::from(([127, 0, 0, 1], 0)), mode, Duration::ZERO).await
    }

    /// Avec une adresse et un délai avant chaque réponse (requêtes
    /// simultanées).
    pub async fn start_with(
        addr: SocketAddr,
        mode: HfMode,
        delay: Duration,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            mode,
            delay,
            addr,
            requests: Mutex::new(Vec::new()),
        });
        let (shutdown, stop) = oneshot::channel::<()>();
        let app = Router::new().fallback(handle).with_state(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stop.await;
                })
                .await;
        });
        Ok(Self {
            addr,
            shutdown: Some(shutdown),
            task,
            shared,
        })
    }

    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Requêtes reçues, dans l'ordre.
    pub fn requests(&self) -> Vec<HfRequest> {
        self.shared.requests.lock().unwrap().clone()
    }

    /// Arrête le serveur (1 s au plus).
    pub async fn stop(mut self) {
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

impl Drop for FakeHf {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.abort();
    }
}

/// Dépôts et gabarits de `fixtures/hf/repos.json`.
static DATA: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../fixtures/hf/repos.json")).expect("valid repos.json")
});

/// README réel de Qwen/Qwen3-8B-GGUF (en-tête YAML compris).
const README_QWEN3_8B: &str = include_str!("../fixtures/hf/readme-qwen3-8b.md");

fn repos() -> &'static [Value] {
    DATA["repos"].as_array().expect("repos")
}

fn find(id: &str) -> Option<&'static Value> {
    repos().iter().find(|repo| {
        repo["id"]
            .as_str()
            .is_some_and(|r| r.eq_ignore_ascii_case(id))
    })
}

fn json_response(status: StatusCode, body: String) -> Response {
    (
        status,
        [
            (
                header::CONTENT_TYPE.as_str(),
                "application/json; charset=utf-8",
            ),
            ("ratelimit", "\"api\";r=499;t=48"),
            ("ratelimit-policy", "\"fixed window\";\"api\";q=500;w=300"),
        ],
        body,
    )
        .into_response()
}

fn error(status: StatusCode, message: &str) -> Response {
    let mut response = json_response(status, json!({ "error": message }).to_string());
    if let Ok(value) = message.parse() {
        response.headers_mut().insert("x-error-message", value);
    }
    response
}

/// 401 d'un dépôt inexistant ou privé (sans `x-error-code`).
fn not_found() -> Response {
    error(StatusCode::UNAUTHORIZED, "Invalid username or password.")
}

/// 401 d'un dépôt restreint.
fn restricted(id: &str, as_json: bool) -> Response {
    let message = format!(
        "Access to model {id} is restricted. You must have access to it and be authenticated to access it. Please log in."
    );
    let mut response = if as_json {
        error(StatusCode::UNAUTHORIZED, &message)
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            message,
        )
            .into_response()
    };
    response
        .headers_mut()
        .insert("x-error-code", "GatedRepo".parse().expect("header"));
    response
}

/// Empreinte FNV-1a (ETag, faux condensats).
fn fnv(text: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn digest(seed: &str) -> String {
    let part = |n: u64| format!("{:016x}", fnv(format!("{seed}{n}").as_bytes()));
    format!("sha256:{}{}{}{}", part(1), part(2), part(3), part(4))
}

/// Réponse JSON avec ETag ; 304 si `If-None-Match` correspond.
fn with_etag(headers: &HeaderMap, body: String, extra: Option<(&'static str, String)>) -> Response {
    let etag = format!("W/\"{:x}\"", fnv(body.as_bytes()));
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        == Some(etag.as_str())
    {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    let mut response = json_response(StatusCode::OK, body);
    response
        .headers_mut()
        .insert(header::ETAG, etag.parse().expect("header"));
    if let Some((name, value)) = extra
        && let Ok(value) = value.parse()
    {
        response.headers_mut().insert(name, value);
    }
    response
}

async fn handle(
    State(shared): State<Arc<Shared>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let path = uri.path().to_owned();
    let path_and_query = uri
        .path_and_query()
        .map_or_else(|| path.clone(), |pq| pq.as_str().to_owned());
    if path == "/fake/requests" {
        let list: Vec<Value> = shared
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| {
                json!({ "method": r.method, "path": r.path, "ifNoneMatch": r.if_none_match, "range": r.range })
            })
            .collect();
        return json_response(StatusCode::OK, Value::Array(list).to_string());
    }
    let header_text = |name: header::HeaderName| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    shared.requests.lock().unwrap().push(HfRequest {
        method: method.to_string(),
        path: path_and_query,
        if_none_match: header_text(header::IF_NONE_MATCH),
        range: header_text(header::RANGE),
    });
    if !shared.delay.is_zero() {
        tokio::time::sleep(shared.delay).await;
    }
    match shared.mode {
        HfMode::Status(code) => {
            let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            if path == "/index.json" && status.is_server_error() {
                return json_response(StatusCode::OK, status_page(false).to_string());
            }
            return error(status, "Something went wrong.");
        }
        HfMode::RateLimited { reset } => {
            let mut response = error(StatusCode::TOO_MANY_REQUESTS, "Too Many Requests");
            response.headers_mut().insert(
                "ratelimit",
                format!("\"api\";r=0;t={reset}").parse().expect("header"),
            );
            return response;
        }
        HfMode::Html => {
            return (
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                "<!doctype html><title>502</title><p>Bad gateway</p>",
            )
                .into_response();
        }
        HfMode::Silent => {
            if path == "/index.json" {
                return json_response(StatusCode::OK, status_page(false).to_string());
            }
            return std::future::pending::<Response>().await;
        }
        HfMode::Normal => {}
    }
    if path == "/index.json" {
        return json_response(StatusCode::OK, status_page(true).to_string());
    }
    let query = uri.query().unwrap_or_default();
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match segments.as_slice() {
        ["api", "models"] => list(&shared, query, &headers),
        ["api", "models", author, name] => detail(&format!("{author}/{name}"), &headers),
        ["v2", author, name, "manifests", tag] => manifest(&format!("{author}/{name}"), tag),
        [author, name, "raw", _revision, "README.md"] => {
            readme(&format!("{author}/{name}"), &headers)
        }
        [author, name, "resolve", _revision, file @ ..] => {
            resolve(&format!("{author}/{name}"), &file.join("/"), &headers)
        }
        _ => error(
            StatusCode::NOT_FOUND,
            "Sorry, we can't find the page you are looking for.",
        ),
    }
}

/// Page d'état (`index.json` de Better Stack, réduite).
fn status_page(operational: bool) -> Value {
    let state = if operational {
        "operational"
    } else {
        "downtime"
    };
    json!({
        "data": { "id": "124692", "type": "status_page", "attributes": { "aggregate_state": state } },
        "included": [
            { "type": "status_page_resource", "attributes": { "public_name": "Huggingface Hub", "status": state } },
            { "type": "status_page_resource", "attributes": { "public_name": "AWS CDN", "status": "operational" } }
        ]
    })
}

fn query_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(name), decode(value))
        })
        .collect()
}

/// Décodage `%XX` et `+`.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(high), Some(low)) => {
                    out.push(high * 16 + low);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Fichiers d'un dépôt : (chemin, taille).
fn files(repo: &Value) -> Vec<(&str, u64)> {
    repo["files"]
        .as_array()
        .map(|files| {
            files
                .iter()
                .filter_map(|file| Some((file[0].as_str()?, file[1].as_u64()?)))
                .collect()
        })
        .unwrap_or_default()
}

/// Dépôt au format de l'API : `detail` ajoute tailles, `sha`, `cardData`.
fn api_repo(repo: &Value, detail: bool) -> Value {
    let id = repo["id"].as_str().unwrap_or_default();
    let mut gguf = json!({});
    if let Some(total) = repo["total"].as_u64() {
        gguf["total"] = json!(total);
    }
    if let Some(architecture) = repo["architecture"].as_str() {
        gguf["architecture"] = json!(architecture);
    }
    if let Some(context) = repo["context_length"].as_u64() {
        gguf["context_length"] = json!(context);
    }
    if let Some(template) = repo["template"].as_str() {
        gguf["chat_template"] = DATA["templates"][template].clone();
        gguf["eos_token"] = json!("<|im_end|>");
    }
    let siblings: Vec<Value> = std::iter::once((".gitattributes", 1519u64))
        .chain(files(repo))
        .chain(repo["readme"].as_str().map(|_| ("README.md", 7577)))
        .map(|(name, size)| {
            if detail {
                let sha = digest(&format!("{id}/{name}"));
                json!({
                    "rfilename": name,
                    "blobId": &sha[7..47],
                    "size": size,
                    "lfs": { "sha256": &sha[7..], "size": size, "pointerSize": 135 }
                })
            } else {
                json!({ "rfilename": name })
            }
        })
        .collect();
    let mut value = json!({
        "_id": format!("{:024x}", fnv(id.as_bytes()) as u128),
        "id": id,
        "author": id.split('/').next().unwrap_or_default(),
        "downloads": repo["downloads"],
        "likes": repo["likes"],
        "trendingScore": repo["trendingScore"],
        "createdAt": repo["createdAt"],
        "lastModified": repo["lastModified"],
        "tags": repo["tags"],
        "gated": repo.get("gated").cloned().unwrap_or(json!(false)),
        "gguf": gguf,
        "siblings": siblings,
    });
    if let Some(pipeline) = repo.get("pipeline_tag") {
        value["pipeline_tag"] = pipeline.clone();
    }
    if let Some(base) = repo["base"].as_str() {
        value["baseModels"] = json!({
            "relation": repo["relation"],
            "models": [{ "_id": format!("{:024x}", fnv(base.as_bytes())), "id": base }]
        });
    }
    if detail {
        value["sha"] = repo["sha"].clone();
        value["private"] = json!(false);
        let mut card = json!({});
        if let Some(license) = repo["license"].as_str() {
            card["license"] = json!(license);
        }
        if let Some(name) = repo["licenseName"].as_str() {
            card["license_name"] = json!(name);
        }
        if let Some(languages) = repo.get("languages") {
            card["language"] = languages.clone();
        }
        if let Some(base) = repo["base"].as_str() {
            card["base_model"] = json!([base]);
        }
        value["cardData"] = card;
    }
    value
}

/// `GET /api/models` : `apps=ollama` (contexte connu), `search` (mots dans
/// l'identifiant, tous), `sort`, `limit` (50 par défaut, 1000 au plus),
/// `cursor` (opaque) et lien `next`.
fn list(shared: &Shared, query: &str, headers: &HeaderMap) -> Response {
    let pairs = query_pairs(query);
    let get = |name: &str| {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let words: Vec<String> = get("search")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let ollama = get("apps") == Some("ollama");
    let mut found: Vec<&Value> = repos()
        .iter()
        .filter(|repo| !ollama || repo["context_length"].is_u64())
        .filter(|repo| {
            let id = repo["id"].as_str().unwrap_or_default().to_lowercase();
            words.iter().all(|word| id.contains(word.as_str()))
        })
        .collect();
    let sort = get("sort").unwrap_or("downloads");
    let field = match sort {
        "downloads" | "likes" | "trendingScore" | "lastModified" | "createdAt" => sort,
        other => {
            return error(
                StatusCode::BAD_REQUEST,
                &format!("✖ Invalid sort parameter: {other}"),
            );
        }
    };
    found.sort_by(|a, b| {
        let (a, b) = (&a[field], &b[field]);
        match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => y.total_cmp(&x),
            _ => b.as_str().cmp(&a.as_str()),
        }
    });
    let limit = get("limit")
        .and_then(|limit| limit.parse::<usize>().ok())
        .unwrap_or(50)
        .clamp(1, 1000);
    let offset = get("cursor")
        .and_then(|cursor| cursor.strip_prefix("page-"))
        .and_then(|offset| offset.parse::<usize>().ok())
        .unwrap_or(0);
    let page: Vec<Value> = found
        .iter()
        .skip(offset)
        .take(limit)
        .map(|repo| api_repo(repo, false))
        .collect();
    let next = (offset + limit < found.len()).then(|| {
        let kept: Vec<&str> = query
            .split('&')
            .filter(|pair| !pair.starts_with("cursor="))
            .collect();
        format!(
            "<http://{}/api/models?{}&cursor=page-{}>; rel=\"next\"",
            shared.addr,
            kept.join("&"),
            offset + limit
        )
    });
    with_etag(
        headers,
        Value::Array(page).to_string(),
        next.map(|link| ("link", link)),
    )
}

/// `GET /api/models/<id>` (avec `blobs=true` : tailles des fichiers).
fn detail(id: &str, headers: &HeaderMap) -> Response {
    match find(id) {
        Some(repo) => with_etag(headers, api_repo(repo, true).to_string(), None),
        None => not_found(),
    }
}

/// `GET /<id>/raw/<révision>/README.md`.
fn readme(id: &str, headers: &HeaderMap) -> Response {
    let Some(repo) = find(id) else {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "Invalid username or password.",
        )
            .into_response();
    };
    if is_gated(repo) {
        return restricted(id, false);
    }
    let text = match repo["readme"].as_str() {
        Some("readme-qwen3-8b.md") => README_QWEN3_8B,
        Some(text) => text,
        None => {
            let mut response = (
                StatusCode::NOT_FOUND,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                "Entry not found",
            )
                .into_response();
            response
                .headers_mut()
                .insert("x-error-code", "EntryNotFound".parse().expect("header"));
            return response;
        }
    };
    let etag = format!("\"{:x}\"", fnv(text.as_bytes()));
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        == Some(etag.as_str())
    {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    (
        [
            (
                header::CONTENT_TYPE.as_str(),
                "text/plain; charset=utf-8".to_owned(),
            ),
            (header::ETAG.as_str(), etag),
            ("ratelimit", "\"resolvers\";r=2996;t=133".to_owned()),
        ],
        text.to_owned(),
    )
        .into_response()
}

fn is_gated(repo: &Value) -> bool {
    match &repo["gated"] {
        Value::String(mode) => !mode.is_empty(),
        Value::Bool(gated) => *gated,
        _ => false,
    }
}

/// En-tête GGUF synthétique d'un dépôt (`header` du jeu de données) :
/// architecture, couches, têtes, puis 40 000 jetons (environ 640 Ko).
fn gguf_header(repo: &Value) -> Vec<u8> {
    let facts = &repo["header"];
    let arch = repo["architecture"].as_str().unwrap_or("llama");
    let number = |name: &str, default: u64| facts[name].as_u64().unwrap_or(default) as u32;
    let mut pairs: Vec<Vec<u8>> = Vec::new();
    let key = |out: &mut Vec<u8>, key: &str, kind: u32| {
        out.extend((key.len() as u64).to_le_bytes());
        out.extend(key.as_bytes());
        out.extend(kind.to_le_bytes());
    };
    let string = |key_name: &str, value: &str| {
        let mut out = Vec::new();
        key(&mut out, key_name, 8);
        out.extend((value.len() as u64).to_le_bytes());
        out.extend(value.as_bytes());
        out
    };
    let u32_pair = |key_name: &str, value: u32| {
        let mut out = Vec::new();
        key(&mut out, key_name, 4);
        out.extend(value.to_le_bytes());
        out
    };
    let mut tokens = Vec::new();
    key(&mut tokens, "tokenizer.ggml.tokens", 9);
    tokens.extend(8u32.to_le_bytes());
    tokens.extend(40_000u64.to_le_bytes());
    for index in 0..40_000 {
        let token = format!("tok{index:05}");
        tokens.extend((token.len() as u64).to_le_bytes());
        tokens.extend(token.as_bytes());
    }
    let late = facts["late"].as_bool().unwrap_or(false);
    if late {
        pairs.push(tokens.clone());
    }
    pairs.push(string("general.architecture", arch));
    pairs.push(string(
        "general.name",
        repo["id"].as_str().unwrap_or_default(),
    ));
    pairs.push(u32_pair(
        &format!("{arch}.context_length"),
        repo["context_length"].as_u64().unwrap_or(4096) as u32,
    ));
    pairs.push(u32_pair(
        &format!("{arch}.embedding_length"),
        number("embedding", 1024),
    ));
    pairs.push(u32_pair(
        &format!("{arch}.block_count"),
        number("blockCount", 28),
    ));
    pairs.push(u32_pair(
        &format!("{arch}.attention.head_count"),
        number("headCount", 16),
    ));
    pairs.push(u32_pair(
        &format!("{arch}.attention.head_count_kv"),
        number("headCountKv", 8),
    ));
    pairs.push(u32_pair(
        &format!("{arch}.attention.key_length"),
        number("keyLength", 128),
    ));
    pairs.push(u32_pair(
        &format!("{arch}.attention.value_length"),
        number("keyLength", 128),
    ));
    if !late {
        pairs.push(tokens);
    }
    let mut out = b"GGUF".to_vec();
    out.extend(3u32.to_le_bytes());
    out.extend(0u64.to_le_bytes());
    out.extend((pairs.len() as u64).to_le_bytes());
    for pair in pairs {
        out.extend(pair);
    }
    out
}

/// `GET /<id>/resolve/<révision>/<fichier>` : en-tête GGUF synthétique, avec
/// `Range: bytes=a-b` (206).
fn resolve(id: &str, file: &str, headers: &HeaderMap) -> Response {
    let Some(repo) = find(id) else {
        return not_found();
    };
    if is_gated(repo) {
        return restricted(id, false);
    }
    if !files(repo).iter().any(|(name, _)| *name == file) {
        return error(StatusCode::NOT_FOUND, "Entry not found");
    }
    let body = gguf_header(repo);
    let total = body.len();
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("bytes="))
        .and_then(|value| value.split_once('-'))
        .and_then(|(start, end)| Some((start.parse::<usize>().ok()?, end.parse::<usize>().ok()?)));
    match range {
        Some((start, end)) if start < total && start <= end => {
            let end = end.min(total - 1);
            (
                StatusCode::PARTIAL_CONTENT,
                [
                    (
                        header::CONTENT_TYPE.as_str(),
                        "application/octet-stream".to_owned(),
                    ),
                    (
                        header::CONTENT_RANGE.as_str(),
                        format!("bytes {start}-{end}/{total}"),
                    ),
                ],
                body[start..=end].to_vec(),
            )
                .into_response()
        }
        _ => ([(header::CONTENT_TYPE, "application/octet-stream")], body).into_response(),
    }
}

/// Noms de l'énumération des quantifications de huggingface.js, pour juger
/// une étiquette comme le registre.
const QUANTS: &[&str] = &[
    "F32",
    "F16",
    "Q4_0",
    "Q4_1",
    "Q4_1_SOME_F16",
    "Q8_0",
    "Q5_0",
    "Q5_1",
    "Q2_K",
    "Q3_K_S",
    "Q3_K_M",
    "Q3_K_L",
    "Q4_K_S",
    "Q4_K_M",
    "Q5_K_S",
    "Q5_K_M",
    "Q6_K",
    "IQ2_XXS",
    "IQ2_XS",
    "Q2_K_S",
    "IQ3_XS",
    "IQ3_XXS",
    "IQ1_S",
    "IQ4_NL",
    "IQ3_S",
    "IQ3_M",
    "IQ2_S",
    "IQ2_M",
    "IQ4_XS",
    "IQ1_M",
    "BF16",
    "Q4_0_4_4",
    "Q4_0_4_8",
    "Q4_0_8_8",
    "TQ1_0",
    "TQ2_0",
    "MXFP4_MOE",
    "NVFP4",
    "Q1_0",
    "Q2_0",
    "Q2_K_XL",
    "Q3_K_XL",
    "Q4_K_XL",
    "Q5_K_XL",
    "Q6_K_XL",
    "Q8_K_XL",
    "MXFP4",
];

fn valid_quant(tag: &str) -> bool {
    let tag = tag.strip_prefix("UD-").unwrap_or(tag);
    QUANTS.iter().any(|quant| {
        tag == *quant
            || tag
                .strip_prefix(quant)
                .and_then(|rest| rest.strip_prefix('_'))
                .is_some_and(|rest| {
                    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_uppercase())
                })
    })
}

fn is_auxiliary(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    ["mmproj", "mtp", "eagle", "draft", "imatrix"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
        || name.contains("mmproj")
        || name.contains("imatrix")
}

fn is_shard(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".gguf") && lower.contains("-of-0") && lower.contains("-0000")
}

/// Registre d'Ollama : `GET /v2/<id>/manifests/<étiquette>`, avec les
/// réponses du vrai (35 § 2.4 et 34 § 2-3) ; l'étiquette est cherchée dans
/// les noms de fichiers, comme le vrai (`f16` peut désigner un BF16).
fn manifest(id: &str, tag: &str) -> Response {
    const SHARDED_TAG: &str = "This tag is a sharded GGUF. Ollama does not yet support pulling sharded GGUF via the registry; please use a single-file quantization tag or \"latest\", or download the shards and merge them locally with `ollama create` (workaround detailed at https://github.com/ollama/ollama/issues/5245).";
    const ONLY_SHARDED: &str = "This repository only contains sharded GGUF files. Ollama does not yet support pulling sharded GGUF via the registry; please download the shards and merge them locally with `ollama create` (workaround detailed at https://github.com/ollama/ollama/issues/5245), or use a repository with single-file quantizations.";
    let Some(repo) = find(id) else {
        return not_found();
    };
    if is_gated(repo) {
        return restricted(repo["id"].as_str().unwrap_or(id), true);
    }
    let all = files(repo);
    let mut models: Vec<(&str, u64)> = all
        .iter()
        .copied()
        .filter(|(name, _)| name.to_ascii_lowercase().ends_with(".gguf") && !is_auxiliary(name))
        .collect();
    models.sort();
    if models.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "Repository is not GGUF or is not compatible with llama.cpp",
        );
    }
    let single: Vec<(&str, u64)> = models
        .iter()
        .copied()
        .filter(|(name, _)| !is_shard(name))
        .collect();
    if single.is_empty() {
        return error(StatusCode::BAD_REQUEST, ONLY_SHARDED);
    }
    let not_available = "The specified tag is not available in the repository. Please use another tag or \"latest\"";
    let chosen = if tag == "latest" {
        single
            .iter()
            .find(|(name, _)| name.to_ascii_uppercase().contains("Q4_K_M"))
            .or_else(|| single.first())
            .copied()
    } else if tag.to_ascii_lowercase().ends_with(".gguf") {
        match single
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(tag))
        {
            Some(found) => Some(*found),
            None => return error(StatusCode::BAD_REQUEST, not_available),
        }
    } else {
        let upper = tag.to_ascii_uppercase();
        if !valid_quant(&upper) {
            return error(
                StatusCode::BAD_REQUEST,
                "The specified tag is not a valid quantization scheme. Please use another tag or \"latest\"",
            );
        }
        match models.iter().find(|(name, _)| {
            name.rsplit('/')
                .next()
                .unwrap_or(name)
                .to_ascii_uppercase()
                .contains(&upper)
        }) {
            Some((name, _)) if is_shard(name) || name.contains('/') => {
                return error(StatusCode::BAD_REQUEST, SHARDED_TAG);
            }
            Some(found) => Some(*found),
            None => return error(StatusCode::BAD_REQUEST, not_available),
        }
    };
    let Some((file, size)) = chosen else {
        return error(StatusCode::BAD_REQUEST, not_available);
    };
    let repo_id = repo["id"].as_str().unwrap_or(id);
    let mut layers = vec![json!({
        "digest": digest(&format!("{repo_id}/{file}")),
        "mediaType": "application/vnd.ollama.image.model",
        "size": size,
    })];
    let projectors: Vec<(&str, u64)> = all
        .iter()
        .copied()
        .filter(|(name, _)| name.to_ascii_lowercase().contains("mmproj"))
        .collect();
    let projector = ["f16", "bf16"]
        .iter()
        .find_map(|quant| {
            projectors.iter().find(|(name, _)| {
                let lower = name.to_ascii_lowercase();
                lower.contains(&format!("-{quant}"))
                    && !(*quant == "f16" && lower.contains("-bf16"))
            })
        })
        .or_else(|| projectors.iter().min_by_key(|(_, size)| *size));
    if let Some((name, size)) = projector {
        layers.push(json!({
            "digest": digest(&format!("{repo_id}/{name}")),
            "mediaType": "application/vnd.ollama.image.projector",
            "size": size,
        }));
    }
    let body = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
        "config": {
            "digest": digest(&format!("{repo_id}/{file}/config")),
            "mediaType": "application/vnd.docker.container.image.v1+json",
            "size": 481,
        },
        "layers": layers,
    });
    (
        [(
            header::CONTENT_TYPE,
            "application/vnd.docker.distribution.manifest.v2+json",
        )],
        body.to_string(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GET brut (le crate n'a pas de client HTTP) : (statut, en-têtes, corps).
    async fn get(fake: &FakeHf, path: &str, extra: &str) -> (u16, String, Vec<u8>) {
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\n{extra}Connection: close\r\n\r\n",
            fake.addr
        );
        let addr = fake.addr;
        tokio::task::spawn_blocking(move || {
            use std::io::{Read, Write};
            let mut stream = std::net::TcpStream::connect(addr).expect("connect");
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("timeout");
            stream.write_all(request.as_bytes()).expect("write");
            let mut response = Vec::new();
            stream.read_to_end(&mut response).expect("read");
            let split = response
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .expect("headers");
            let head = String::from_utf8_lossy(&response[..split]).into_owned();
            let status = head[9..12].parse().expect("status");
            (status, head, response[split + 4..].to_vec())
        })
        .await
        .expect("request")
    }

    fn body_json(body: &[u8]) -> Value {
        // Corps éventuellement découpé (chunked) : on garde le JSON.
        let text = String::from_utf8_lossy(body);
        let start = text.find(['[', '{']).expect("json");
        let end = text.rfind([']', '}']).expect("json");
        serde_json::from_str(&text[start..=end]).expect("valid json")
    }

    #[tokio::test]
    async fn lists_search_sort_and_pages() {
        let fake = FakeHf::start(HfMode::Normal).await.unwrap();
        let (status, head, body) =
            get(&fake, "/api/models?apps=ollama&sort=downloads&limit=3", "").await;
        assert_eq!(status, 200);
        let page = body_json(&body);
        assert_eq!(page.as_array().unwrap().len(), 3);
        assert_eq!(page[0]["id"], "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF");
        assert!(page[0]["siblings"][1].get("size").is_none());
        assert!(head.to_lowercase().contains("link: <http://"), "{head}");
        assert!(head.contains("cursor=page-3"), "{head}");
        // Recherche : tous les mots, dans l'identifiant.
        let (_, _, body) = get(
            &fake,
            "/api/models?apps=ollama&search=qwen%20coder&limit=30",
            "",
        )
        .await;
        let found = body_json(&body);
        assert_eq!(found.as_array().unwrap().len(), 1);
        // Sans contexte : écarté par apps=ollama.
        let (_, _, body) = get(&fake, "/api/models?apps=ollama&search=minimax", "").await;
        assert_eq!(body_json(&body), json!([]));
        let (_, _, body) = get(&fake, "/api/models?search=minimax", "").await;
        assert_eq!(body_json(&body).as_array().unwrap().len(), 1);
        // ETag.
        let (_, head, _) = get(
            &fake,
            "/api/models?apps=ollama&sort=trendingScore&limit=200",
            "",
        )
        .await;
        let etag = head
            .lines()
            .find_map(|line| line.strip_prefix("etag: "))
            .expect("etag")
            .to_owned();
        let (status, _, _) = get(
            &fake,
            "/api/models?apps=ollama&sort=trendingScore&limit=200",
            &format!("If-None-Match: {etag}\r\n"),
        )
        .await;
        assert_eq!(status, 304);
        assert_eq!(fake.requests().len(), 6);
        assert_eq!(
            fake.requests()[5].if_none_match.as_deref(),
            Some(etag.as_str())
        );
    }

    #[tokio::test]
    async fn details_readmes_and_headers() {
        let fake = FakeHf::start(HfMode::Normal).await.unwrap();
        let (status, _, body) =
            get(&fake, "/api/models/unsloth/Qwen3.5-9B-GGUF?blobs=true", "").await;
        assert_eq!(status, 200);
        let detail = body_json(&body);
        assert!(detail["siblings"][1]["size"].is_u64());
        assert_eq!(detail["baseModels"]["models"][0]["id"], "Qwen/Qwen3.5-9B");
        let (status, head, _) = get(&fake, "/api/models/nobody/missing-GGUF", "").await;
        assert_eq!(status, 401);
        assert!(!head.to_lowercase().contains("x-error-code"));
        let (status, _, body) = get(&fake, "/Qwen/Qwen3-8B-GGUF/raw/main/README.md", "").await;
        assert_eq!(status, 200);
        assert!(String::from_utf8_lossy(&body).contains("# Qwen3-8B-GGUF"));
        let (status, head, _) =
            get(&fake, "/google/gemma-2-2b-it-GGUF/raw/main/README.md", "").await;
        assert_eq!(status, 401);
        assert!(head.contains("x-error-code: GatedRepo"));
        let (status, _, _) = get(&fake, "/unsloth/Qwen3.5-9B-GGUF/raw/main/README.md", "").await;
        assert_eq!(status, 404);
        let (status, head, body) = get(
            &fake,
            "/Qwen/Qwen3-8B-GGUF/resolve/main/Qwen3-8B-Q4_K_M.gguf",
            "Range: bytes=0-262143\r\n",
        )
        .await;
        assert_eq!(status, 206);
        assert!(head.contains("content-range: bytes 0-262143/"), "{head}");
        assert_eq!(&body[..4], b"GGUF");
    }

    #[tokio::test]
    async fn manifests_answer_like_the_registry() {
        let fake = FakeHf::start(HfMode::Normal).await.unwrap();
        let manifest = |path: &'static str| {
            let fake = &fake;
            async move {
                let (status, _, body) = get(fake, path, "").await;
                (status, body_json(&body))
            }
        };
        let (status, body) = manifest("/v2/unsloth/Qwen3.5-9B-GGUF/manifests/UD-Q4_K_XL").await;
        assert_eq!(status, 200);
        assert_eq!(body["layers"][0]["size"], 5966095584u64);
        assert_eq!(
            body["layers"][1]["mediaType"],
            "application/vnd.ollama.image.projector"
        );
        assert_eq!(body["layers"][1]["size"], 918166080u64);
        let (status, body) =
            manifest("/v2/bartowski/Qwen3.8-27B-GGUF/manifests/Qwen3.8-27B-Q4_K_L.gguf").await;
        assert_eq!(status, 200);
        assert_eq!(body["layers"][0]["size"], 18820703968u64);
        let (status, body) = manifest("/v2/bartowski/Qwen3.8-27B-GGUF/manifests/Q4_K_L").await;
        assert_eq!(status, 400);
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("not a valid quantization")
        );
        let (status, body) = manifest("/v2/bartowski/Qwen3.8-27B-GGUF/manifests/BF16").await;
        assert_eq!(status, 400);
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("This tag is a sharded GGUF")
        );
        let (status, body) = manifest("/v2/chatonlm/Tiny-Sharded-GGUF/manifests/Q4_K_M").await;
        assert_eq!(status, 400);
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("only contains sharded")
        );
        let (status, body) = manifest("/v2/chatonlm/Tiny-Chat-GGUF/manifests/Q5_K_M").await;
        assert_eq!(status, 400);
        assert!(body["error"].as_str().unwrap().contains("not available"));
        let (status, _) = manifest("/v2/nobody/missing-GGUF/manifests/Q4_K_M").await;
        assert_eq!(status, 401);
        let (status, body) = manifest("/v2/chatonlm/Tiny-Restricted-GGUF/manifests/Q4_K_M").await;
        assert_eq!(status, 401);
        assert!(body["error"].as_str().unwrap().contains("is restricted"));
    }

    #[tokio::test]
    async fn failure_modes() {
        let fake = FakeHf::start(HfMode::Status(503)).await.unwrap();
        assert_eq!(get(&fake, "/api/models", "").await.0, 503);
        let (status, _, body) = get(&fake, "/index.json", "").await;
        assert_eq!(status, 200);
        assert_eq!(
            body_json(&body)["data"]["attributes"]["aggregate_state"],
            "downtime"
        );
        let limited = FakeHf::start(HfMode::RateLimited { reset: 42 })
            .await
            .unwrap();
        let (status, head, _) = get(&limited, "/api/models", "").await;
        assert_eq!(status, 429);
        assert!(head.contains("ratelimit: \"api\";r=0;t=42"), "{head}");
        let html = FakeHf::start(HfMode::Html).await.unwrap();
        let (status, head, _) = get(&html, "/api/models", "").await;
        assert_eq!(status, 200);
        assert!(head.contains("text/html"));
        assert_eq!(HfMode::parse("hf-status:500"), Some(HfMode::Status(500)));
        assert_eq!(
            HfMode::parse("hf-ratelimit"),
            Some(HfMode::RateLimited { reset: 42 })
        );
        assert_eq!(HfMode::parse("hf-html"), Some(HfMode::Html));
        assert_eq!(HfMode::parse("hf-silent"), Some(HfMode::Silent));
        assert_eq!(HfMode::parse("hf"), Some(HfMode::Normal));
        assert_eq!(HfMode::parse("ollama"), None);
        // Le journal se lit aussi en HTTP.
        let (_, _, body) = get(&fake, "/fake/requests", "").await;
        assert_eq!(body_json(&body).as_array().unwrap().len(), 2);
    }

    #[test]
    fn decoding_and_quantizations() {
        assert_eq!(decode("qwen%20coder"), "qwen coder");
        assert_eq!(decode("a+b"), "a b");
        assert_eq!(decode("100%"), "100%");
        assert!(valid_quant("Q4_K_M"));
        assert!(valid_quant("UD-Q4_K_XL"));
        assert!(valid_quant("Q6_K_L"));
        assert!(!valid_quant("Q4_K_L"));
        assert!(!valid_quant("FP16"));
        assert!(!valid_quant("Q9_K_Z"));
    }
}
