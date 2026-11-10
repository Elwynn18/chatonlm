//! Faux Hugging Face pour les tests du ModelHub : API des modèles (liste,
//! recherche, tri, curseur `Link`, ETag), fiche avec tailles, README (`raw/`),
//! en-tête GGUF synthétique (`resolve/`, requêtes Range), dépôts d'images
//! (`fixtures/hf/images.json` : fichiers au vrai contenu déterministe,
//! `lfs.sha256` exact, Range ; filtres `pipeline_tag` et `filter=gguf`,
//! fiche à une révision) et page d'état (`/index.json`), et dépôts GGUF synthétisés au vrai contenu
//! pour les téléchargements directs (`chatonlm-dl/…`, voir
//! [`crate::hf_models`]). Serveur à part, sur son propre port. Dépôts : `fixtures/hf/repos.json` (réduits des échantillons réels de
//! `.tools/research/35-samples`). Ce que le serveur a reçu se lit sur
//! `GET /fake/requests`.
//!
//! Modes : `hf` (normal), `hf-status:<code>` (toutes les routes répondent ce
//! code ; page d'état « incident » pour un 5xx), `hf-ratelimit` (429 et
//! `RateLimit: "api";r=0;t=42`), `hf-html` (200 et une page HTML),
//! `hf-silent` (connexion acceptée, jamais de réponse ; page d'état
//! « incident »).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// Révision publiée de chaque dépôt synthétisé (0 par défaut).
    revisions: Mutex<HashMap<String, u32>>,
    /// Fichiers synthétisés abandonnés par le client avant leur fin.
    disconnects: AtomicUsize,
    /// Fichier retenu ([`FakeHf::hold`]) : nom de fichier qui finit par ce
    /// texte.
    held: Mutex<Option<String>>,
}

impl Shared {
    fn synth(&self, id: &str) -> Option<crate::hf_models::Synth> {
        let key = id.to_ascii_lowercase();
        let revision = self
            .revisions
            .lock()
            .unwrap()
            .get(&key)
            .copied()
            .unwrap_or(0);
        crate::hf_models::Synth::parse(id, revision)
    }

    /// Dépôt synthétisé `id` à la révision dont le commit est `commit`
    /// (`resolve/<commit>/…` : une ancienne révision reste servie, comme sur
    /// Hugging Face) ; commit inconnu (`main`…) : la dernière.
    fn synth_at(&self, id: &str, commit: &str) -> Option<crate::hf_models::Synth> {
        let latest = self.synth(id)?;
        let last = latest.revision();
        Some(
            (0..last)
                .filter_map(|revision| crate::hf_models::Synth::parse(id, revision))
                .find(|older| older.sha() == commit)
                .unwrap_or(latest),
        )
    }
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
            revisions: Mutex::new(HashMap::new()),
            disconnects: AtomicUsize::new(0),
            held: Mutex::new(None),
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

    /// Publie une nouvelle version du dépôt synthétisé `repo` (commit et
    /// poids changent).
    pub fn publish(&self, repo: &str) {
        publish(&self.shared, repo);
    }

    /// Retient les fichiers `slow` dont le nom finit par `file` : passée leur
    /// moitié (ou dès le début d'une reprise au-delà), un octet toutes les
    /// 200 ms, jusqu'à [`FakeHf::release`]. Un geste (pause, annulation) tombe
    /// ainsi toujours en plein téléchargement, même sous la charge.
    pub fn hold(&self, file: &str) {
        *self.shared.held.lock().unwrap() = Some(file.to_owned());
    }

    /// Fin de [`FakeHf::hold`] : la suite est servie normalement.
    pub fn release(&self) {
        *self.shared.held.lock().unwrap() = None;
    }

    /// Fichiers synthétisés abandonnés par le client avant leur fin.
    pub fn disconnects(&self) -> usize {
        self.shared.disconnects.load(Ordering::SeqCst)
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

/// Dépôts d'images (modèles d'images de l'app, contrat 83 § 7).
static IMAGES: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../fixtures/hf/images.json")).expect("valid images.json")
});

/// Familles d'essai qui citent les pièces des dépôts d'images
/// (`CHATONLM_IMAGE_FAMILIES` de l'app).
pub const IMAGE_FAMILIES: &str = include_str!("../fixtures/hf/image-families.json");

/// Tous les dépôts : ceux du ModelHub, puis ceux d'images.
static REPOS: LazyLock<Vec<Value>> = LazyLock::new(|| {
    let mut all = DATA["repos"].as_array().expect("repos").clone();
    all.extend(
        IMAGES["repos"]
            .as_array()
            .expect("image repos")
            .iter()
            .cloned(),
    );
    all
});

fn repos() -> &'static [Value] {
    &REPOS
}

/// Dépôt d'images : ses fichiers ont un vrai contenu.
fn has_content(repo: &Value) -> bool {
    repo["content"].as_bool().unwrap_or(false)
}

/// Contenu déterministe d'un fichier d'un dépôt d'images (`size` octets).
fn blob_content(repo: &str, path: &str, size: usize) -> Vec<u8> {
    let seed = fnv(format!("{repo}/{path}").as_bytes());
    (0..size)
        .map(|i| ((seed >> ((i % 8) * 8)) as u8) ^ (i as u8).wrapping_mul(31))
        .collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Fichier d'un dépôt d'images, tel que le faux le sert ; `None` : dépôt ou
/// fichier inconnu.
pub fn image_blob(repo: &str, path: &str) -> Option<Vec<u8>> {
    let found = find(repo).filter(|found| has_content(found))?;
    let id = found["id"].as_str()?;
    let (_, size) = files(found).into_iter().find(|(name, _)| *name == path)?;
    Some(blob_content(id, path, size as usize))
}

/// Fichier sans `lfs` (pas d'empreinte dans la fiche).
fn without_lfs(repo: &Value, path: &str) -> bool {
    repo["files"].as_array().is_some_and(|files| {
        files
            .iter()
            .any(|file| file[0].as_str() == Some(path) && file[2].as_str() == Some("nolfs"))
    })
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
    if path == "/fake/publish-hf" && method == Method::POST {
        let repo = uri
            .query()
            .and_then(|query| {
                query_pairs(query)
                    .into_iter()
                    .find(|(key, _)| key == "repo")
                    .map(|(_, value)| value)
            })
            .unwrap_or_default();
        publish(&shared, &repo);
        return json_response(StatusCode::OK, json!({ "repo": repo }).to_string());
    }
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
    // Dépôts synthétisés (`chatonlm-dl/…`).
    match segments.as_slice() {
        ["api", "models", author, name] | ["api", "models", author, name, "revision", _] => {
            if let Some(synth) = shared.synth(&format!("{author}/{name}")) {
                return crate::hf_models::detail(&synth);
            }
        }
        [author, name, "resolve", revision, file @ ..] => {
            if let Some(synth) = shared.synth_at(&format!("{author}/{name}"), revision) {
                let counter = shared.clone();
                let name = decode(&file.join("/"));
                let (holder, held_name) = (shared.clone(), name.clone());
                return crate::hf_models::resolve(
                    &synth,
                    &name,
                    &headers,
                    move || {
                        counter.disconnects.fetch_add(1, Ordering::SeqCst);
                    },
                    move || {
                        holder
                            .held
                            .lock()
                            .unwrap()
                            .as_deref()
                            .is_some_and(|held| held_name.ends_with(held))
                    },
                );
            }
        }
        _ => {}
    }
    match segments.as_slice() {
        ["api", "models"] => list(&shared, query, &headers),
        ["api", "models", author, name] => detail(&format!("{author}/{name}"), &headers),
        // Fiche à une révision : la même (le faux n'a qu'un commit).
        ["api", "models", author, name, "revision", _revision] => {
            detail(&format!("{author}/{name}"), &headers)
        }
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

/// Nouvelle révision du dépôt synthétisé `repo`.
fn publish(shared: &Shared, repo: &str) {
    *shared
        .revisions
        .lock()
        .unwrap()
        .entry(repo.to_ascii_lowercase())
        .or_insert(0) += 1;
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
            if detail && has_content(repo) {
                let sha = sha256_hex(&blob_content(id, name, size as usize));
                if without_lfs(repo, name) || !files(repo).iter().any(|(file, _)| *file == name) {
                    json!({ "rfilename": name, "blobId": &sha[..40], "size": size })
                } else {
                    json!({
                        "rfilename": name,
                        "blobId": &sha[..40],
                        "size": size,
                        "lfs": { "sha256": sha, "size": size, "pointerSize": 134 }
                    })
                }
            } else if detail {
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

/// `GET /api/models` : `apps=llama.cpp` (contexte connu), `search` (mots dans
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
    // Filtre des applications de Hugging Face (`llama.cpp`) : dépôts GGUF
    // dont le contexte est connu.
    let gguf_app = get("apps") == Some("llama.cpp");
    let pipeline = get("pipeline_tag");
    let gguf_only = get("filter") == Some("gguf");
    let mut found: Vec<&Value> = repos()
        .iter()
        .filter(|repo| !gguf_app || repo["context_length"].is_u64())
        .filter(|repo| pipeline.is_none_or(|tag| repo["pipeline_tag"].as_str() == Some(tag)))
        .filter(|repo| {
            !gguf_only
                || repo["tags"]
                    .as_array()
                    .is_some_and(|tags| tags.iter().any(|tag| tag == "gguf"))
        })
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
    let Some((_, size)) = files(repo).into_iter().find(|(name, _)| *name == file) else {
        return error(StatusCode::NOT_FOUND, "Entry not found");
    };
    let body = if has_content(repo) {
        blob_content(repo["id"].as_str().unwrap_or(id), file, size as usize)
    } else {
        gguf_header(repo)
    };
    let total = body.len();
    // `bytes=a-b`, ou `bytes=a-` (la suite, reprise d'un téléchargement).
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("bytes="))
        .and_then(|value| value.split_once('-'))
        .and_then(|(start, end)| {
            let start = start.parse::<usize>().ok()?;
            let end = if end.is_empty() {
                total.saturating_sub(1)
            } else {
                end.parse::<usize>().ok()?
            };
            Some((start, end))
        });
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

    /// Dépôts synthétisés : fiche aux vraies empreintes, fichier complet ou
    /// repris, abandon d'un fichier qui traîne compté.
    #[tokio::test]
    async fn synthesized_repositories_serve_real_files() {
        let fake = FakeHf::start(HfMode::Normal).await.unwrap();
        let repo = "chatonlm-dl/Tiny-stall-GGUF";
        let (status, _, body) = get(&fake, &format!("/api/models/{repo}?blobs=true"), "").await;
        assert_eq!(status, 200);
        let api = body_json(&body);
        let synth = crate::hf_models::Synth::parse(repo, 0).unwrap();
        assert_eq!(api["sha"].as_str(), Some(synth.sha().as_str()));
        let whole = crate::hf_models::Synth::parse("chatonlm-dl/Tiny-GGUF", 0).unwrap();
        let file = whole.content("Tiny-Q4_K_M.gguf").unwrap();
        let (status, _, body) = get(
            &fake,
            "/chatonlm-dl/Tiny-GGUF/resolve/x/Tiny-Q4_K_M.gguf",
            "",
        )
        .await;
        assert_eq!((status, body.len()), (200, file.len()));
        let (status, head, body) = get(
            &fake,
            "/chatonlm-dl/Tiny-GGUF/resolve/x/Tiny-Q4_K_M.gguf",
            "Range: bytes=100-\r\n",
        )
        .await;
        assert_eq!(status, 206);
        assert!(head.contains(&format!("bytes 100-{}/{}", file.len() - 1, file.len())));
        assert_eq!(body, file[100..]);
        // Abandon pendant que le fichier traîne : compté.
        let addr = fake.addr;
        tokio::task::spawn_blocking(move || {
            use std::io::{Read, Write};
            let mut stream = std::net::TcpStream::connect(addr).unwrap();
            stream
                .write_all(
                    b"GET /chatonlm-dl/Tiny-stall-GGUF/resolve/x/Tiny-Q4_K_M.gguf HTTP/1.1\r\nHost: x\r\n\r\n",
                )
                .unwrap();
            // Tout ce qui est arrivé est lu (fermeture propre, comme un
            // client qui abandonne entre deux octets).
            stream
                .set_read_timeout(Some(Duration::from_millis(100)))
                .unwrap();
            let mut buffer = [0; 65536];
            let until = std::time::Instant::now() + Duration::from_millis(500);
            while std::time::Instant::now() < until {
                if stream.read(&mut buffer).is_ok_and(|read| read == 0) {
                    break;
                }
            }
        })
        .await
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while fake.disconnects() == 0 {
            assert!(std::time::Instant::now() < deadline, "no disconnect seen");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(fake.disconnects(), 1);
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
        let (status, head, body) = get(
            &fake,
            "/api/models?apps=llama.cpp&sort=downloads&limit=3",
            "",
        )
        .await;
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
            "/api/models?apps=llama.cpp&search=qwen%20coder&limit=30",
            "",
        )
        .await;
        let found = body_json(&body);
        assert_eq!(found.as_array().unwrap().len(), 1);
        // Sans contexte : écarté par apps=llama.cpp.
        let (_, _, body) = get(&fake, "/api/models?apps=llama.cpp&search=minimax", "").await;
        assert_eq!(body_json(&body), json!([]));
        let (_, _, body) = get(&fake, "/api/models?search=minimax", "").await;
        assert_eq!(body_json(&body).as_array().unwrap().len(), 1);
        // ETag.
        let (_, head, _) = get(
            &fake,
            "/api/models?apps=llama.cpp&sort=trendingScore&limit=200",
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
            "/api/models?apps=llama.cpp&sort=trendingScore&limit=200",
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
        assert_eq!(HfMode::parse("other"), None);
        // Le journal se lit aussi en HTTP.
        let (_, _, body) = get(&fake, "/fake/requests", "").await;
        assert_eq!(body_json(&body).as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn image_repos_are_filtered_and_their_files_served_with_real_checksums() {
        let fake = FakeHf::start(HfMode::Normal).await.unwrap();
        let ids = |body: &[u8]| -> Vec<String> {
            body_json(body)
                .as_array()
                .unwrap()
                .iter()
                .map(|repo| repo["id"].as_str().unwrap().to_owned())
                .collect()
        };
        let (_, _, body) = get(
            &fake,
            "/api/models?pipeline_tag=text-to-image&filter=gguf&search=chatonlm&sort=downloads",
            "",
        )
        .await;
        let gguf = ids(&body);
        assert_eq!(gguf[0], "chatonlm/klein-test-GGUF");
        assert!(!gguf.contains(&"chatonlm/Tiny-SDXL".to_owned()));
        assert!(!gguf.contains(&"chatonlm/tiny-text-encoder".to_owned()));
        let (_, _, body) = get(
            &fake,
            "/api/models?pipeline_tag=text-to-image&search=sdxl",
            "",
        )
        .await;
        assert_eq!(
            ids(&body),
            ["chatonlm/Tiny-SDXL", "chatonlm/Tiny-SDXL-LoRA"]
        );
        // Sans filtre d'images ni `apps=llama.cpp` : rien ne change pour le
        // ModelHub.
        let (_, _, body) = get(&fake, "/api/models?apps=llama.cpp&search=klein", "").await;
        assert!(ids(&body).is_empty());

        let (status, _, body) = get(
            &fake,
            "/api/models/chatonlm/klein-test-GGUF/revision/main?blobs=true",
            "",
        )
        .await;
        assert_eq!(status, 200);
        let detail = body_json(&body);
        let sibling = |name: &str| {
            detail["siblings"]
                .as_array()
                .unwrap()
                .iter()
                .find(|sibling| sibling["rfilename"] == name)
                .unwrap()
                .clone()
        };
        let content = image_blob("chatonlm/klein-test-GGUF", "tiny-klein-Q4_0.gguf").unwrap();
        assert_eq!(content.len(), 3000);
        assert_eq!(
            sibling("tiny-klein-Q4_0.gguf")["lfs"]["sha256"],
            sha256_hex(&content)
        );
        assert!(sibling("tiny-klein-Q5_K_M.gguf").get("lfs").is_none());
        assert_eq!(
            detail["cardData"]["base_model"][0],
            "black-forest-labs/FLUX.2-klein-4B"
        );

        let (status, _, body) = get(
            &fake,
            "/chatonlm/klein-test-GGUF/resolve/abc/tiny-klein-Q4_0.gguf",
            "",
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, content);
        let (status, head, body) = get(
            &fake,
            "/chatonlm/klein-test-GGUF/resolve/abc/tiny-klein-Q4_0.gguf",
            "Range: bytes=1000-\r\n",
        )
        .await;
        assert_eq!(status, 206);
        assert!(
            head.contains("content-range: bytes 1000-2999/3000"),
            "{head}"
        );
        assert_eq!(body, &content[1000..]);
        let (status, _, _) = get(
            &fake,
            "/chatonlm/gated-image-GGUF/resolve/abc/restricted-Q4_0.gguf",
            "",
        )
        .await;
        assert_eq!(status, 401);
    }

    /// Les familles d'essai citent les vraies empreintes des pièces servies.
    #[test]
    fn test_families_match_the_served_pieces() {
        let families: Value = serde_json::from_str(IMAGE_FAMILIES).unwrap();
        let mut pinned: Vec<&Value> = families["pieces"].as_object().unwrap().values().collect();
        pinned.extend(families["recommendations"].as_array().unwrap());
        assert!(pinned.len() >= 4);
        for piece in pinned {
            let repo = piece["repo"].as_str().unwrap();
            let file = piece["file"].as_str().unwrap();
            let content = image_blob(repo, file).unwrap_or_else(|| panic!("{repo}/{file}"));
            assert_eq!(piece["size"].as_u64(), Some(content.len() as u64), "{file}");
            assert_eq!(
                piece["sha256"].as_str(),
                Some(sha256_hex(&content).as_str()),
                "{file}"
            );
            assert_eq!(
                piece["revision"].as_str(),
                find(repo).and_then(|found| found["sha"].as_str()),
                "{file}"
            );
        }
    }

    #[test]
    fn decoding() {
        assert_eq!(decode("qwen%20coder"), "qwen coder");
        assert_eq!(decode("a+b"), "a b");
        assert_eq!(decode("100%"), "100%");
    }
}
