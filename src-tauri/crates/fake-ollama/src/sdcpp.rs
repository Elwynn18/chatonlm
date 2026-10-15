//! Faux `sd-server` (stable-diffusion.cpp) : moteur d'images de ChatonLM
//! (`CHATONLM_SD_SERVER_BIN` des builds d'essai, essais du module
//! `image_gen`). Il sert l'API native telle que la donne master-964
//! (`.tools/research/82-image-generation-research.md` § 9) :
//! `GET /sdcpp/v1/capabilities`, `POST /sdcpp/v1/img_gen` (202 + tâche),
//! `GET /sdcpp/v1/jobs/{id}`, `POST /sdcpp/v1/jobs/{id}/cancel` (200 pour une
//! tâche en file, 409 pour une tâche en cours, comme le vrai).
//!
//! Arguments du vrai : `--listen-ip`, `--listen-port`, puis les fichiers du
//! modèle (`--model` ou `--diffusion-model`, `--vae`, `--llm`, `--clip_l`,
//! `--clip_g`, `--t5xxl`) et d'autres options, gardées telles quelles. Comme
//! le vrai, il sort aussitôt avec le code 1 si le modèle manque ou ne se lit
//! pas (ici : fichier absent, ou nom qui contient `fail-load`) ; un nom qui
//! contient `never-ready` le fait attendre sans jamais écouter.
//!
//! Scénarios d'une tâche, par un mot du prompt :
//! - `[slow]` : 400 ms par étape (30 ms sinon) ;
//! - `[fail]` : échec après la première étape
//!   (`{"code":"generation_failed","message":"fake failure"}`) ;
//! - `[crash]` : le processus sort (code 134) pendant la première étape ;
//! - `[hang]` : reste `generating` sans fin (seul l'arrêt du processus en
//!   vient à bout, comme le vrai, qui n'interrompt pas un calcul) ;
//! - `[deaf]` : comme `[hang]`, et SIGTERM est ignoré (Unix) : seul SIGKILL
//!   l'arrête (serveur lent à mourir) ;
//! - `[queued]` : la tâche reste 300 ms en file avant son calcul ;
//! - `[flaky]` : deux suivis sur trois échouent (503, serveur débordé).
//!
//! Chaque étape publie un aperçu (PNG 8×8) si la requête en demande un
//! (`preview` autre que `none`) ; l'image finale est un PNG de la taille
//! demandée, d'une couleur tirée de la graine. Journaux : `GET /fake/args`
//! (arguments reçus), `GET /fake/requests` (corps des `img_gen`) et
//! `GET /fake/received` (`{prompt, busy}` : une autre tâche était-elle en
//! file ou en cours à la réception ?).

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify};

/// Variable qui donne les arguments au faux lancé par le binaire d'essai de
/// l'app (relancé sur un essai : la ligne de commande est à libtest).
/// Arguments séparés par U+001F.
pub const ARGS_ENV: &str = "CHATONLM_FAKE_SD_ARGS";

/// Arguments reçus par la variable [`ARGS_ENV`].
pub fn args_from_env() -> Option<Vec<String>> {
    let value = std::env::var(ARGS_ENV).ok()?;
    Some(value.split('\u{1f}').map(str::to_owned).collect())
}

/// Options qui désignent un fichier du modèle.
const FILE_OPTIONS: [&str; 7] = [
    "--model",
    "--diffusion-model",
    "--vae",
    "--llm",
    "--clip_l",
    "--clip_g",
    "--t5xxl",
];

/// Tâche du faux.
#[derive(Clone)]
struct Job {
    id: String,
    status: &'static str,
    created: u64,
    started: Option<u64>,
    completed: Option<u64>,
    body: Value,
    preview: Option<Value>,
    result: Option<Value>,
    error: Option<Value>,
}

impl Job {
    fn json(&self, queue_position: usize) -> Value {
        json!({
            "id": self.id,
            "kind": "img_gen",
            "status": self.status,
            "created": self.created,
            "started": self.started,
            "completed": self.completed,
            "queue_position": queue_position,
            "result": self.result,
            "error": self.error,
            "preview": self.preview,
        })
    }
}

struct Shared {
    args: Vec<String>,
    model: String,
    jobs: Mutex<Vec<Job>>,
    queue: Mutex<VecDeque<String>>,
    requests: Mutex<Vec<Value>>,
    received: Mutex<Vec<Value>>,
    wake: Notify,
    /// Suivis reçus (scénario `[flaky]`).
    polls: std::sync::atomic::AtomicU64,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Valeur d'une option (`--option valeur`).
fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

/// Lance le faux avec les arguments du vrai ; rend le code de sortie.
pub async fn run(args: Vec<String>) -> i32 {
    let model = option(&args, "--model").or_else(|| option(&args, "--diffusion-model"));
    let Some(model) = model.map(str::to_owned) else {
        eprintln!("[E] new_sd_ctx_t failed (no model)");
        return 1;
    };
    for name in FILE_OPTIONS {
        if let Some(path) = option(&args, name)
            && !std::path::Path::new(path).is_file()
        {
            eprintln!("[W] loading model from '{path}' failed");
            eprintln!("[E] new_sd_ctx_t failed");
            return 1;
        }
    }
    if model.contains("fail-load") {
        eprintln!("[E] get sd version from file failed: ''");
        eprintln!("[E] new_sd_ctx_t failed");
        return 1;
    }
    if model.contains("never-ready") {
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }
    let ip = option(&args, "--listen-ip")
        .unwrap_or("127.0.0.1")
        .to_owned();
    let port: u16 = option(&args, "--listen-port")
        .and_then(|port| port.parse().ok())
        .unwrap_or(1234);
    let address: SocketAddr = match format!("{ip}:{port}").parse() {
        Ok(address) => address,
        Err(_) => {
            eprintln!("[E] invalid listen address");
            return 1;
        }
    };
    let listener = match tokio::net::TcpListener::bind(address).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("[E] failed to bind {address}: {error}");
            return 1;
        }
    };
    let shared = Arc::new(Shared {
        args,
        model,
        jobs: Mutex::new(Vec::new()),
        queue: Mutex::new(VecDeque::new()),
        requests: Mutex::new(Vec::new()),
        received: Mutex::new(Vec::new()),
        wake: Notify::new(),
        polls: std::sync::atomic::AtomicU64::new(0),
    });
    tokio::spawn(worker(shared.clone()));
    let app = Router::new()
        .route("/sdcpp/v1/capabilities", get(capabilities))
        .route("/sdcpp/v1/img_gen", post(img_gen))
        .route("/sdcpp/v1/jobs/{id}", get(job))
        .route("/sdcpp/v1/jobs/{id}/cancel", post(cancel))
        .route("/fake/args", get(fake_args))
        .route("/fake/requests", get(fake_requests))
        .route("/fake/received", get(fake_received))
        .with_state(shared);
    eprintln!("[I] listening on: http://{address}");
    match axum::serve(listener, app).await {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("[E] {error}");
            1
        }
    }
}

async fn capabilities(State(shared): State<Arc<Shared>>) -> Json<Value> {
    let path = std::path::Path::new(&shared.model);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    Json(json!({
        "model": { "name": name, "stem": stem, "path": shared.model },
        "current_mode": "img_gen",
        "supported_modes": ["img_gen"],
        "preview_modes": ["none", "proj", "tae", "vae"],
        "limits": {
            "min_width": 64, "max_width": 4096, "min_height": 64, "max_height": 4096,
            "max_batch_count": 8, "max_queue_size": 64,
        },
    }))
}

async fn img_gen(State(shared): State<Arc<Shared>>, Json(body): Json<Value>) -> Response {
    shared.requests.lock().await.push(body.clone());
    let mut jobs = shared.jobs.lock().await;
    let busy = jobs
        .iter()
        .any(|job| matches!(job.status, "queued" | "generating"));
    shared
        .received
        .lock()
        .await
        .push(json!({ "prompt": body["prompt"], "busy": busy }));
    let id = format!("job_fake_{:08}", jobs.len());
    let created = now();
    jobs.push(Job {
        id: id.clone(),
        status: "queued",
        created,
        started: None,
        completed: None,
        body,
        preview: None,
        result: None,
        error: None,
    });
    drop(jobs);
    shared.queue.lock().await.push_back(id.clone());
    shared.wake.notify_one();
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "id": id,
            "kind": "img_gen",
            "status": "queued",
            "created": created,
            "poll_url": format!("/sdcpp/v1/jobs/{id}"),
        })),
    )
        .into_response()
}

async fn job(State(shared): State<Arc<Shared>>, Path(id): Path<String>) -> Response {
    let queue = shared.queue.lock().await.clone();
    let jobs = shared.jobs.lock().await;
    match jobs.iter().find(|job| job.id == id) {
        Some(job) => {
            // `[flaky]` : deux suivis sur trois échouent (503), comme un
            // serveur momentanément débordé.
            if job.body["prompt"]
                .as_str()
                .is_some_and(|prompt| prompt.contains("[flaky]"))
                && shared
                    .polls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    % 3
                    != 2
            {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error": "busy"})),
                )
                    .into_response();
            }
            let position = queue.iter().position(|queued| *queued == id).unwrap_or(0);
            Json(job.json(position)).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "job not found"})),
        )
            .into_response(),
    }
}

async fn cancel(State(shared): State<Arc<Shared>>, Path(id): Path<String>) -> Response {
    let mut queue = shared.queue.lock().await;
    let mut jobs = shared.jobs.lock().await;
    let Some(job) = jobs.iter_mut().find(|job| job.id == id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "job not found"})),
        )
            .into_response();
    };
    match job.status {
        "queued" => {
            queue.retain(|queued| *queued != id);
            job.status = "cancelled";
            job.completed = Some(now());
            job.error = Some(json!({"code": "cancelled", "message": "job cancelled by client"}));
            Json(job.json(0)).into_response()
        }
        "generating" => (
            StatusCode::CONFLICT,
            Json(json!({"error": "job is currently generating and cannot be interrupted yet"})),
        )
            .into_response(),
        _ => (
            StatusCode::GONE,
            Json(json!({"error": "job already finished"})),
        )
            .into_response(),
    }
}

async fn fake_args(State(shared): State<Arc<Shared>>) -> Json<Value> {
    Json(json!(shared.args))
}

async fn fake_requests(State(shared): State<Arc<Shared>>) -> Json<Value> {
    Json(json!(*shared.requests.lock().await))
}

async fn fake_received(State(shared): State<Arc<Shared>>) -> Json<Value> {
    Json(json!(*shared.received.lock().await))
}

/// Traite les tâches une à une, comme le vrai.
async fn worker(shared: Arc<Shared>) {
    loop {
        let next = shared.queue.lock().await.front().cloned();
        let Some(id) = next else {
            shared.wake.notified().await;
            continue;
        };
        // `[queued]` : la tâche reste en file 300 ms avant son calcul.
        let held = shared
            .jobs
            .lock()
            .await
            .iter()
            .find(|job| job.id == id)
            .and_then(|job| job.body["prompt"].as_str())
            .is_some_and(|prompt| prompt.contains("[queued]"));
        if held {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let body = {
            let mut jobs = shared.jobs.lock().await;
            let Some(job) = jobs.iter_mut().find(|job| job.id == id) else {
                continue;
            };
            job.status = "generating";
            job.started = Some(now());
            job.body.clone()
        };
        shared.queue.lock().await.pop_front();
        let outcome = generate(&shared, &id, &body).await;
        let mut jobs = shared.jobs.lock().await;
        if let Some(job) = jobs.iter_mut().find(|job| job.id == id) {
            job.completed = Some(now());
            match outcome {
                Ok(result) => {
                    job.status = "completed";
                    job.result = Some(result);
                }
                Err(error) => {
                    job.status = "failed";
                    job.error = Some(error);
                }
            }
        }
    }
}

async fn generate(shared: &Shared, id: &str, body: &Value) -> Result<Value, Value> {
    let prompt = body["prompt"].as_str().unwrap_or_default();
    let steps = body["sample_params"]["sample_steps"]
        .as_u64()
        .unwrap_or(20)
        .max(1);
    let width = body["width"].as_u64().unwrap_or(512).clamp(1, 4096) as u32;
    let height = body["height"].as_u64().unwrap_or(512).clamp(1, 4096) as u32;
    let seed = body["seed"].as_i64().unwrap_or(42);
    let preview = body["preview"].as_str().is_some_and(|mode| mode != "none");
    let delay = if prompt.contains("[slow]") {
        Duration::from_millis(400)
    } else {
        Duration::from_millis(30)
    };
    if prompt.contains("[deaf]") {
        // Le gestionnaire installé par tokio remplace l'action par défaut
        // (fin du processus) jusqu'à la sortie.
        #[cfg(unix)]
        std::mem::forget(tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        ));
    }
    if prompt.contains("[hang]") || prompt.contains("[deaf]") {
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }
    for step in 1..=steps {
        tokio::time::sleep(delay).await;
        if prompt.contains("[crash]") {
            eprintln!("[E] fake crash");
            std::process::exit(134);
        }
        if preview {
            let frame = json!({
                "pass": 1,
                "step": step,
                "total_steps": steps,
                "b64_json": BASE64.encode(png(8, 8, color(seed.wrapping_add(step as i64)))),
            });
            let mut jobs = shared.jobs.lock().await;
            if let Some(job) = jobs.iter_mut().find(|job| job.id == id) {
                job.preview = Some(frame);
            }
        }
        if prompt.contains("[fail]") {
            return Err(json!({"code": "generation_failed", "message": "fake failure"}));
        }
    }
    let format = body["output_format"].as_str().unwrap_or("png");
    Ok(json!({
        "output_format": format,
        "images": [{ "index": 0, "b64_json": BASE64.encode(png(width, height, color(seed))) }],
    }))
}

/// Couleur tirée de la graine.
pub fn color(seed: i64) -> [u8; 3] {
    let bytes = seed.to_le_bytes();
    [bytes[0], bytes[1] ^ 0x5a, bytes[2] ^ 0xa5]
}

/// PNG d'une seule couleur.
pub fn png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("PNG header");
        let pixels: Vec<u8> = std::iter::repeat_n(rgb, (width * height) as usize)
            .flatten()
            .collect();
        writer.write_image_data(&pixels).expect("PNG data");
    }
    out
}
