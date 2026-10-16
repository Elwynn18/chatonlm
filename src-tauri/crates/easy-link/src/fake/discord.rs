//! Faux Discord : voir `fake/mod.rs`. API HTTP v10 sous `/api/v10`,
//! passerelle WebSocket sous `/gateway`, fichiers sous `/attachments`.
//!
//! Comme le vrai : une session de passerelle garde ses envois numérotés et
//! les rejoue à une reprise (op 6) ; un jeton inconnu ferme en 4004, d'autres
//! intentions que `DIRECT_MESSAGES` en 4014. Écarts voulus, pour des essais
//! sans attente : un message posté avant la première session est gardé et
//! remis après `READY` (le vrai Discord le perdrait), et le salon privé d'un
//! compte est « 9 » suivi de son identifiant (compte 42 : salon 942).
//!
//! Un message à pièce jointe (`multipart/form-data` : `payload_json` et
//! `files[0]`) est lu comme le ferait Discord : pièce jointe de 10 Mio au
//! plus (413 au-delà), texte facultatif, `attachments` qui renvoient aux
//! fichiers envoyés.
//!
//! Garde-fous propres au faux serveur : une requête sans agent `DiscordBot`
//! est refusée (400), comme un fichier demandé avec le jeton du bot (le CDN
//! n'en veut pas : il ne doit jamais sortir de l'API).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::serve::ListenerExt;
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// Seul jeton accepté.
pub const TOKEN: &str = "fake.discord.token";

/// Le bot : son compte et son application portent le même identifiant, comme
/// pour tout bot récent.
const BOT_ID: &str = "424242";
const BOT_NAME: &str = "chatonlm-test";

/// Serveur des messages qui ne sont pas privés.
const GUILD_ID: &str = "515151";

/// Seules intentions permises : `DIRECT_MESSAGES`.
const INTENTS: u64 = 1 << 12;

/// Marque d'un message vocal.
const VOICE_FLAG: u64 = 1 << 13;

/// Limite d'un message, en caractères.
const CONTENT_MAX: usize = 2000;

/// Pièce jointe la plus grosse dans un message privé.
const UPLOAD_MAX: usize = 10 * 1024 * 1024;

const IMAGE_NAME: &str = "image.png";
const VOICE_NAME: &str = "voice-message.ogg";

/// Image PNG valide de 2 × 2 pixels roux.
const PNG: [u8; 75] = [
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00, 0x00, 0x00, 0xFD, 0xD4, 0x9A,
    0x73, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0x78, 0x51, 0x65, 0xF5,
    0xA2, 0xCA, 0x8A, 0x01, 0x42, 0x01, 0x00, 0x2F, 0xD6, 0x06, 0x71, 0x9C, 0x15, 0x4F, 0x20, 0x00,
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// Message vocal du crate, lu à chaque demande : absent, le fichier manque
/// (404) sans arrêter le serveur.
fn voice_fixture() -> Option<Vec<u8>> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/voice-speech.ogg"
    ))
    .ok()
}

pub struct FakeDiscord {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
    shared: Arc<Shared>,
}

/// État du serveur, partagé entre ses routes et ses connexions.
struct Shared {
    addr: SocketAddr,
    /// Intervalle des battements annoncé par Hello, en millisecondes.
    heartbeat: AtomicU64,
    /// Faux : les battements ne sont plus accusés (connexion « zombie »).
    acks: AtomicBool,
    next_id: AtomicU64,
    /// Ce que le bot a fait (`GET /fake/sent`).
    sent: Mutex<Vec<Value>>,
    /// Charges reçues sur la passerelle, jeton masqué.
    received: Mutex<Vec<Value>>,
    /// Échecs commandés par [`FakeDiscord::fail_next`], dans l'ordre.
    failures: Mutex<Vec<(String, u16, Value)>>,
    /// Messages du bot : identifiant → salon.
    posted: Mutex<HashMap<String, String>>,
    gateway: Mutex<Gateway>,
}

#[derive(Default)]
struct Gateway {
    session: Option<Session>,
    /// Messages arrivés avant la première session.
    waiting: Vec<Value>,
    /// Connexions ouvertes.
    links: Vec<(u64, mpsc::UnboundedSender<Order>)>,
    /// Connexion qui porte la session (après `READY` ou `RESUMED`).
    live: Option<u64>,
}

struct Session {
    id: String,
    /// Tout ce qui lui a été remis, numéroté à partir de 1 : rejoué à une
    /// reprise, après le numéro que le client dit avoir reçu.
    history: Vec<Value>,
}

/// Ordre donné à une connexion de passerelle.
enum Order {
    Send(Value),
    /// Ferme, avec ce code ; sans code, coupe sans trame de fermeture.
    Close(Option<u16>),
}

/// Ce qu'une charge du client appelle en retour.
#[derive(Default)]
struct Reply {
    payloads: Vec<Value>,
    close: Option<(u16, &'static str)>,
}

impl FakeDiscord {
    /// Démarre le serveur sur un port libre de 127.0.0.1.
    pub async fn start() -> Self {
        Self::start_on(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .expect("port libre sur 127.0.0.1")
    }

    pub async fn start_on(addr: SocketAddr) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let addr = listener.local_addr()?;
        let (shutdown, stop) = oneshot::channel::<()>();
        let shared = Arc::new(Shared {
            addr,
            heartbeat: AtomicU64::new(1000),
            acks: AtomicBool::new(true),
            next_id: AtomicU64::new(700_000_000_000_000_001),
            sent: Mutex::default(),
            received: Mutex::default(),
            failures: Mutex::default(),
            posted: Mutex::default(),
            gateway: Mutex::default(),
        });
        let app = router(shared.clone());
        // Sans l'algorithme de Nagle : il retient une trame tant que la
        // précédente n'est pas acquittée (jusqu'à 40 ms), plus que
        // l'intervalle des battements d'un essai.
        let listener = listener.tap_io(|stream| {
            let _ = stream.set_nodelay(true);
        });
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

    /// Adresse de l'API, à donner telle quelle à `discord::Client::new` :
    /// `http://127.0.0.1:<port>/api/v10`.
    pub fn url(&self) -> String {
        format!("http://{}/api/v10", self.addr)
    }

    /// Fait arriver un message (même JSON que `POST /fake/message`).
    pub fn message(&self, message: serde_json::Value) {
        self.shared.message(&message);
    }

    /// Ce que le bot a fait, dans l'ordre (même JSON que `GET /fake/sent`) ;
    /// un envoi ou une modification porte aussi `"allowedMentions"` et
    /// `"flags"`, tels que reçus (`null` s'ils manquaient).
    pub fn sent(&self) -> Vec<serde_json::Value> {
        self.shared.sent.lock().unwrap().clone()
    }

    /// La prochaine requête de cette route échoue avec ce code et ce corps
    /// (et l'en-tête `Retry-After` si le corps porte `retry_after`). Routes :
    /// `send`, `photo` (message à pièce jointe), `edit`, `typing`, `gateway`
    /// (`GET /gateway/bot`).
    pub fn fail_next(&self, route: &str, status: u16, body: serde_json::Value) {
        self.shared
            .failures
            .lock()
            .unwrap()
            .push((route.to_owned(), status, body));
    }

    /// Coupe la passerelle, avec ce code de fermeture (`None` : sans trame de
    /// fermeture). La session reste valable : le client peut la reprendre.
    pub fn drop_gateway(&self, close_code: Option<u16>) {
        self.shared.close_links(close_code);
    }

    /// Charges reçues du client sur la passerelle, dans l'ordre (identify,
    /// resume, heartbeat), jeton remplacé par « *** ». Aussi
    /// `GET /fake/gateway`.
    pub fn gateway_log(&self) -> Vec<serde_json::Value> {
        self.shared.received.lock().unwrap().clone()
    }

    /// Envoie une charge telle quelle au client relié (op 7, op 9, op 1,
    /// charge abîmée). Un op 9 avec `d` faux invalide aussi la session.
    pub fn gateway_send(&self, payload: serde_json::Value) {
        let mut gateway = self.shared.gateway.lock().unwrap();
        if payload["op"] == 9 && payload["d"] == false {
            gateway.session = None;
        }
        gateway.send_live(payload);
    }

    /// Intervalle des battements annoncé aux prochaines connexions (1 s par
    /// défaut).
    pub fn set_heartbeat_interval(&self, interval: Duration) {
        let millis = u64::try_from(interval.as_millis()).unwrap_or(u64::MAX);
        self.shared.heartbeat.store(millis.max(1), Ordering::SeqCst);
    }

    /// Cesse d'accuser les battements, ou recommence.
    pub fn silence_acks(&self, silent: bool) {
        self.shared.acks.store(!silent, Ordering::SeqCst);
    }

    /// Arrête le serveur et attend la fin de sa tâche, 1 s au plus (une
    /// connexion de passerelle encore ouverte retiendrait l'arrêt en douceur :
    /// elles sont coupées d'abord).
    pub async fn stop(mut self) {
        self.shared.close_links(None);
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

impl Drop for FakeDiscord {
    fn drop(&mut self) {
        // Les connexions de passerelle vivent dans leurs propres tâches.
        self.shared.close_links(None);
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.abort();
    }
}

impl Gateway {
    /// Envoie à la connexion qui porte la session, s'il y en a une.
    fn send_live(&self, payload: Value) {
        let link = self
            .links
            .iter()
            .find(|(id, _)| Some(*id) == self.live)
            .map(|(_, orders)| orders);
        if let Some(orders) = link {
            let _ = orders.send(Order::Send(payload));
        }
    }

    /// Numérote un envoi de la session et le garde pour une reprise.
    fn record(session: &mut Session, kind: &str, data: Value) -> Value {
        let payload = json!({"op": 0, "t": kind, "s": session.history.len() + 1, "d": data});
        session.history.push(payload.clone());
        payload
    }
}

impl Shared {
    fn snowflake(&self) -> String {
        self.next_id.fetch_add(1, Ordering::SeqCst).to_string()
    }

    fn close_links(&self, code: Option<u16>) {
        let mut gateway = self.gateway.lock().unwrap();
        // Dès maintenant : un message posté juste après attend la reprise.
        gateway.live = None;
        for (_, orders) in &gateway.links {
            let _ = orders.send(Order::Close(code));
        }
    }

    fn failure(&self, route: &str) -> Option<Response> {
        let (status, body) = {
            let mut failures = self.failures.lock().unwrap();
            let index = failures.iter().position(|(name, ..)| name == route)?;
            let (_, status, body) = failures.remove(index);
            (status, body)
        };
        let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let retry = body["retry_after"].as_f64();
        let mut response = (status, Json(body)).into_response();
        if let Some(retry) = retry
            && let Ok(value) = HeaderValue::from_str(&format!("{}", retry.ceil()))
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        Some(response)
    }

    /// Construit le `MESSAGE_CREATE` d'un message de `fake/mod.rs` et le
    /// remet à la session.
    fn message(&self, message: &Value) {
        let user = match &message["user"] {
            Value::String(user) => user.clone(),
            Value::Number(user) => user.to_string(),
            _ => "42".to_owned(),
        };
        let channel = format!("9{user}");
        let mut author = json!({
            "id": user,
            "username": format!("user{user}"),
            "global_name": message["name"].as_str(),
            "discriminator": "0",
        });
        if message["bot"] == true {
            author["bot"] = json!(true);
        }
        let age = message["age"].as_f64().unwrap_or(0.0).max(0.0);
        let sent_at = SystemTime::now()
            .checked_sub(Duration::from_secs_f64(age))
            .unwrap_or(UNIX_EPOCH);
        let mut flags = 0;
        let mut attachments = Vec::new();
        if message["image"] == true {
            attachments.push(self.attachment(&channel, IMAGE_NAME, "image/png", PNG.len()));
        }
        if message["voice"] == true {
            flags |= VOICE_FLAG;
            let size = voice_fixture().map_or(0, |voice| voice.len());
            let mut voice = self.attachment(&channel, VOICE_NAME, "audio/ogg", size);
            voice["duration_secs"] = json!(1.0);
            voice["waveform"] = json!("");
            attachments.push(voice);
        }
        let mut data = json!({
            "id": self.snowflake(),
            "channel_id": channel,
            "author": author,
            "content": message["text"].as_str().unwrap_or_default(),
            "timestamp": iso8601(sent_at),
            "type": 0,
            "flags": flags,
            "attachments": attachments,
            "mentions": [],
            "pinned": false,
            "tts": false,
        });
        if message["private"] == false {
            data["guild_id"] = json!(GUILD_ID);
        }

        let mut gateway = self.gateway.lock().unwrap();
        match gateway.session.as_mut() {
            Some(session) => {
                let payload = Gateway::record(session, "MESSAGE_CREATE", data);
                gateway.send_live(payload);
            }
            None => gateway.waiting.push(data),
        }
    }

    fn attachment(&self, channel: &str, name: &str, content_type: &str, size: usize) -> Value {
        let id = self.snowflake();
        let url = format!("http://{}/attachments/{channel}/{id}/{name}", self.addr);
        json!({
            "id": id,
            "filename": name,
            "content_type": content_type,
            "size": size,
            "url": url,
            "proxy_url": url,
        })
    }

    /// Reçoit une charge du client sur la connexion `link`.
    fn receive(&self, link: u64, payload: &Value) -> Reply {
        let mut logged = payload.clone();
        if logged["d"]["token"].is_string() {
            logged["d"]["token"] = json!("***");
        }
        self.received.lock().unwrap().push(logged);

        let mut reply = Reply::default();
        let data = &payload["d"];
        match payload["op"].as_u64() {
            Some(1) if self.acks.load(Ordering::SeqCst) => reply.payloads.push(json!({"op": 11})),
            Some(2 | 6) if data["token"] != TOKEN => {
                reply.close = Some((4004, "Authentication failed."));
            }
            Some(2) if data["intents"] != INTENTS => {
                reply.close = Some((4014, "Disallowed intent(s)."));
            }
            Some(2) => {
                let mut gateway = self.gateway.lock().unwrap();
                let mut session = Session {
                    id: format!("session-{}", self.snowflake()),
                    history: Vec::new(),
                };
                let ready = json!({
                    "v": 10,
                    "user": {"id": BOT_ID, "username": BOT_NAME, "bot": true, "global_name": null},
                    "guilds": [],
                    "session_id": session.id,
                    "resume_gateway_url": format!("ws://{}/gateway", self.addr),
                    "application": {"id": BOT_ID, "flags": 0},
                });
                reply
                    .payloads
                    .push(Gateway::record(&mut session, "READY", ready));
                for data in std::mem::take(&mut gateway.waiting) {
                    reply
                        .payloads
                        .push(Gateway::record(&mut session, "MESSAGE_CREATE", data));
                }
                gateway.session = Some(session);
                gateway.live = Some(link);
            }
            Some(6) => {
                let mut gateway = self.gateway.lock().unwrap();
                let seen = data["seq"].as_u64().unwrap_or(0) as usize;
                match gateway.session.as_mut() {
                    Some(session) if data["session_id"] == session.id.as_str() => {
                        let missed = seen.min(session.history.len());
                        reply.payloads.extend_from_slice(&session.history[missed..]);
                        reply
                            .payloads
                            .push(Gateway::record(session, "RESUMED", json!({})));
                        gateway.live = Some(link);
                    }
                    _ => reply.payloads.push(json!({"op": 9, "d": false})),
                }
            }
            _ => {}
        }
        reply
    }
}

fn router(shared: Arc<Shared>) -> Router {
    let api = Router::new()
        .route("/users/@me", get(me))
        .route("/applications/@me", get(application))
        .route("/gateway/bot", get(gateway_bot))
        .route("/channels/{channel}/messages", post(send))
        .route("/channels/{channel}/messages/{message}", patch(edit))
        .route("/channels/{channel}/typing", post(typing));
    Router::new()
        .nest("/api/v10", api)
        // Le client ajoute « /?v=10&encoding=json » à l'adresse reçue.
        .route("/gateway", get(gateway))
        .route("/gateway/", get(gateway))
        .route("/attachments/{*path}", get(attachment))
        .route("/fake/message", post(fake_message))
        .route("/fake/sent", get(fake_sent))
        .route("/fake/gateway", get(fake_gateway))
        // Plafond d'axum (2 Mo) relevé : une pièce jointe au-delà de la
        // limite de Discord doit arriver jusqu'à son refus (413).
        .layer(DefaultBodyLimit::max(2 * UPLOAD_MAX))
        .with_state(shared)
}

fn refusal(status: StatusCode, message: &str, code: u32) -> Response {
    (status, Json(json!({"message": message, "code": code}))).into_response()
}

/// Refus d'une requête de l'API mal présentée : jeton inconnu, ou agent qui
/// n'est pas celui d'un bot.
fn unwelcome(headers: &HeaderMap) -> Option<Response> {
    let text = |name| headers.get(name).and_then(|value| value.to_str().ok());
    if text(header::AUTHORIZATION) != Some(format!("Bot {TOKEN}").as_str()) {
        return Some(refusal(StatusCode::UNAUTHORIZED, "401: Unauthorized", 0));
    }
    if !text(header::USER_AGENT).is_some_and(|agent| agent.starts_with("DiscordBot (")) {
        return Some(refusal(StatusCode::BAD_REQUEST, "Invalid User-Agent", 0));
    }
    None
}

async fn me(headers: HeaderMap) -> Response {
    if let Some(response) = unwelcome(&headers) {
        return response;
    }
    Json(json!({"id": BOT_ID, "username": BOT_NAME, "bot": true, "global_name": null}))
        .into_response()
}

async fn application(headers: HeaderMap) -> Response {
    if let Some(response) = unwelcome(&headers) {
        return response;
    }
    Json(json!({"id": BOT_ID, "name": BOT_NAME})).into_response()
}

async fn gateway_bot(State(shared): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if let Some(response) = unwelcome(&headers).or_else(|| shared.failure("gateway")) {
        return response;
    }
    Json(json!({
        "url": format!("ws://{}/gateway", shared.addr),
        "shards": 1,
        "session_start_limit": {
            "total": 1000,
            "remaining": 999,
            "reset_after": 14_400_000,
            "max_concurrency": 1,
        },
    }))
    .into_response()
}

/// Texte d'un envoi ou d'une modification, mentions permises et drapeaux, ou
/// le refus de Discord (code HTTP, message, code d'erreur).
fn content(body: &Bytes) -> Result<(String, Value, Value), (StatusCode, &'static str, u32)> {
    let Ok(body) = serde_json::from_slice::<Value>(body) else {
        return Err((StatusCode::BAD_REQUEST, "400: Bad Request", 0));
    };
    let content = body["content"].as_str().unwrap_or_default();
    if content.chars().count() > CONTENT_MAX {
        return Err((StatusCode::BAD_REQUEST, "Invalid Form Body", 50035));
    }
    if content.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Cannot send an empty message",
            50006,
        ));
    }
    Ok((
        content.to_owned(),
        body["allowed_mentions"].clone(),
        body["flags"].clone(),
    ))
}

async fn send(
    State(shared): State<Arc<Shared>>,
    Path(channel): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    if super::is_multipart(content_type) {
        if let Some(response) = unwelcome(&headers).or_else(|| shared.failure("photo")) {
            return response;
        }
        return send_files(&shared, channel, content_type.unwrap_or_default(), &body);
    }
    if let Some(response) = unwelcome(&headers).or_else(|| shared.failure("send")) {
        return response;
    }
    let (text, mentions, flags) = match content(&body) {
        Ok(content) => content,
        Err((status, message, code)) => return refusal(status, message, code),
    };
    let id = shared.snowflake();
    shared
        .posted
        .lock()
        .unwrap()
        .insert(id.clone(), channel.clone());
    shared.sent.lock().unwrap().push(json!({
        "kind": "send",
        "channel": channel,
        "id": id,
        "text": text,
        "allowedMentions": mentions,
        "flags": flags,
    }));
    Json(json!({"id": id, "channel_id": channel, "content": text})).into_response()
}

/// Message à pièce jointe : `payload_json` (texte, mentions, drapeaux,
/// `attachments`) et `files[n]`.
fn send_files(shared: &Shared, channel: String, content_type: &str, body: &[u8]) -> Response {
    let invalid = || refusal(StatusCode::BAD_REQUEST, "Invalid Form Body", 50035);
    let Ok(parts) = super::parse_multipart(content_type, body) else {
        return refusal(StatusCode::BAD_REQUEST, "400: Bad Request", 0);
    };
    let Some(payload) = parts
        .iter()
        .find(|part| part.name == "payload_json")
        .and_then(|part| serde_json::from_slice::<Value>(&part.data).ok())
    else {
        return invalid();
    };
    let files: Vec<_> = parts
        .iter()
        .filter(|part| part.name.starts_with("files["))
        .collect();
    let [file] = files.as_slice() else {
        // L'app n'envoie qu'une image par message.
        return invalid();
    };
    let Some(index) = file
        .name
        .strip_prefix("files[")
        .and_then(|rest| rest.strip_suffix(']'))
        .and_then(|index| index.parse::<u64>().ok())
    else {
        return invalid();
    };
    let Some(filename) = file.filename.clone().filter(|name| !name.is_empty()) else {
        return invalid();
    };
    if file.data.len() > UPLOAD_MAX {
        return refusal(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Request entity too large",
            40005,
        );
    }
    // `attachments`, s'il est donné, désigne les fichiers envoyés.
    if let Some(attachments) = payload.get("attachments") {
        let named = attachments.as_array().is_some_and(|list| {
            list.iter()
                .all(|attachment| attachment["id"].as_u64() == Some(index))
        });
        if !named {
            return invalid();
        }
    }
    let content = payload["content"].as_str().unwrap_or_default();
    if content.chars().count() > CONTENT_MAX {
        return invalid();
    }
    let id = shared.snowflake();
    let mut entry = json!({
        "kind": "photo",
        "channel": channel,
        "id": id,
        "filename": filename,
        "size": file.data.len(),
        "contentType": file.content_type,
        "upload": "attachment",
        "allowedMentions": payload["allowed_mentions"],
        "flags": payload["flags"],
    });
    if !content.is_empty() {
        entry["caption"] = content.into();
    }
    shared.sent.lock().unwrap().push(entry);
    let url = format!(
        "http://{}/attachments/{channel}/{id}/{filename}",
        shared.addr
    );
    Json(json!({
        "id": id,
        "channel_id": channel,
        "content": content,
        "attachments": [{
            "id": shared.snowflake(),
            "filename": filename,
            "size": file.data.len(),
            "content_type": file.content_type,
            "url": url,
            "proxy_url": url,
        }],
    }))
    .into_response()
}

async fn edit(
    State(shared): State<Arc<Shared>>,
    Path((channel, id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = unwelcome(&headers).or_else(|| shared.failure("edit")) {
        return response;
    }
    if shared.posted.lock().unwrap().get(&id) != Some(&channel) {
        return refusal(StatusCode::NOT_FOUND, "Unknown Message", 10008);
    }
    let (text, mentions, flags) = match content(&body) {
        Ok(content) => content,
        Err((status, message, code)) => return refusal(status, message, code),
    };
    shared.sent.lock().unwrap().push(json!({
        "kind": "edit",
        "channel": channel,
        "id": id,
        "text": text,
        "allowedMentions": mentions,
        "flags": flags,
    }));
    Json(json!({"id": id, "channel_id": channel, "content": text})).into_response()
}

async fn typing(
    State(shared): State<Arc<Shared>>,
    Path(channel): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = unwelcome(&headers).or_else(|| shared.failure("typing")) {
        return response;
    }
    // Comme le frontal de Discord : un POST sans longueur est refusé.
    if !headers.contains_key(header::CONTENT_LENGTH) {
        return StatusCode::LENGTH_REQUIRED.into_response();
    }
    shared
        .sent
        .lock()
        .unwrap()
        .push(json!({"kind": "typing", "channel": channel}));
    StatusCode::NO_CONTENT.into_response()
}

async fn attachment(Path(path): Path<String>, headers: HeaderMap) -> Response {
    if headers.contains_key(header::AUTHORIZATION) {
        return refusal(
            StatusCode::BAD_REQUEST,
            "attachment requested with a bot token",
            0,
        );
    }
    let file = match path.rsplit('/').next() {
        Some(IMAGE_NAME) => Some(("image/png", PNG.to_vec())),
        Some(VOICE_NAME) => voice_fixture().map(|voice| ("audio/ogg", voice)),
        _ => None,
    };
    match file {
        Some((content_type, bytes)) => {
            ([(header::CONTENT_TYPE, content_type)], bytes).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn fake_message(State(shared): State<Arc<Shared>>, body: Bytes) -> Response {
    match serde_json::from_slice::<Value>(&body) {
        Ok(message) if message.is_object() => {
            shared.message(&message);
            StatusCode::NO_CONTENT.into_response()
        }
        _ => StatusCode::BAD_REQUEST.into_response(),
    }
}

async fn fake_sent(State(shared): State<Arc<Shared>>) -> Json<Value> {
    Json(Value::Array(shared.sent.lock().unwrap().clone()))
}

async fn fake_gateway(State(shared): State<Arc<Shared>>) -> Json<Value> {
    Json(Value::Array(shared.received.lock().unwrap().clone()))
}

async fn gateway(State(shared): State<Arc<Shared>>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| serve_gateway(socket, shared))
}

async fn serve_gateway(mut socket: WebSocket, shared: Arc<Shared>) {
    let link = shared.next_id.fetch_add(1, Ordering::SeqCst);
    let (orders, mut inbox) = mpsc::unbounded_channel();
    shared.gateway.lock().unwrap().links.push((link, orders));
    let hello = json!({
        "op": 10,
        "d": {"heartbeat_interval": shared.heartbeat.load(Ordering::SeqCst)},
    });
    if push(&mut socket, &hello).await {
        converse(&mut socket, &mut inbox, link, &shared).await;
    }
    let mut gateway = shared.gateway.lock().unwrap();
    gateway.links.retain(|(id, _)| *id != link);
    if gateway.live == Some(link) {
        gateway.live = None;
    }
}

/// Sert une connexion jusqu'à sa fermeture, par elle ou par un ordre.
async fn converse(
    socket: &mut WebSocket,
    inbox: &mut mpsc::UnboundedReceiver<Order>,
    link: u64,
    shared: &Shared,
) {
    loop {
        tokio::select! {
            order = inbox.recv() => match order {
                Some(Order::Send(payload)) => {
                    if !push(socket, &payload).await {
                        return;
                    }
                }
                Some(Order::Close(Some(code))) => {
                    close(socket, code, "").await;
                    return;
                }
                Some(Order::Close(None)) | None => return,
            },
            message = socket.recv() => {
                let Some(Ok(message)) = message else { return };
                let Message::Text(text) = message else {
                    if matches!(message, Message::Close(_)) {
                        return;
                    }
                    continue;
                };
                let Ok(payload) = serde_json::from_str::<Value>(&text) else { continue };
                let reply = shared.receive(link, &payload);
                for payload in &reply.payloads {
                    if !push(socket, payload).await {
                        return;
                    }
                }
                if let Some((code, reason)) = reply.close {
                    close(socket, code, reason).await;
                    return;
                }
            }
        }
    }
}

async fn push(socket: &mut WebSocket, payload: &Value) -> bool {
    socket
        .send(Message::Text(payload.to_string().into()))
        .await
        .is_ok()
}

async fn close(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let frame = CloseFrame {
        code,
        reason: reason.into(),
    };
    let _ = socket.send(Message::Close(Some(frame))).await;
}

/// Date à la façon de Discord : `2026-10-13T09:41:12.345000+00:00`.
fn iso8601(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = since.as_secs();
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Jour compté depuis 1970 → date civile (algorithme de Howard Hinnant).
    let shifted = days + 719_468;
    let (era, day_of_era) = (shifted / 146_097, shifted % 146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:06}+00:00",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60,
        since.subsec_micros(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_written_like_discord_does() {
        let at = |seconds: u64, micros: u32| UNIX_EPOCH + Duration::new(seconds, micros * 1000);
        assert_eq!(iso8601(at(0, 0)), "1970-01-01T00:00:00.000000+00:00");
        assert_eq!(
            iso8601(at(1_709_251_199, 5)),
            "2024-02-29T23:59:59.000005+00:00"
        );
        assert_eq!(
            iso8601(at(1_791_884_472, 345_000)),
            "2026-10-13T09:41:12.345000+00:00"
        );
    }

    #[test]
    fn the_image_is_a_png() {
        assert_eq!(&PNG[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&PNG[PNG.len() - 8..PNG.len() - 4], b"IEND");
    }
}
