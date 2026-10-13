//! Faux Telegram : voir `fake/mod.rs`.
//!
//! Sert la Bot API utile à l'app (`getMe`, `getUpdates` en attente longue,
//! `sendMessage`, `editMessageText`, `sendChatAction`, `getFile` et les
//! fichiers), avec les refus du vrai service que l'app doit savoir traiter :
//! jeton inconnu, HTML illisible, message vide ou trop long, message inchangé.
//! En plus du pilotage commun : `GET /fake/requests` (toutes les requêtes
//! reçues), `POST /fake/fail` avec `{"method", "status", "body"}` (comme
//! [`FakeTelegram::fail_next`]), `POST /fake/block` avec `{"user"}` (comme
//! [`FakeTelegram::block`]) et `POST /fake/hold` avec `{"method", "held"}`
//! (comme [`FakeTelegram::hold`]).

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

/// Seul jeton accepté.
pub const TOKEN: &str = "424242:fake-telegram-token";

const BOT_ID: i64 = 424_242;
const BOT_NAME: &str = "chatonlm_test_bot";

/// Limite de Telegram, en unités UTF-16 du texte une fois la mise en forme lue.
const TEXT_LIMIT: usize = 4096;

/// Identifiants des fichiers joints par `"image"` et `"voice"`.
const PHOTO: &str = "fake-photo";
const PHOTO_THUMBNAIL: &str = "fake-photo-thumbnail";
const VOICE: &str = "fake-voice";
const PHOTO_PATH: &str = "photos/file_1.png";
const VOICE_PATH: &str = "voice/file_2.oga";

/// Message vocal joint par `"voice"`, lu à chaque demande : s'il manque, le
/// fichier répond 404.
const VOICE_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/voice-speech.ogg");

/// Image PNG valide de 2 × 2 points (un carré roux).
const PNG: [u8; 73] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00, 0x00, 0x00, 0xfd, 0xd4, 0x9a,
    0x73, 0x00, 0x00, 0x00, 0x10, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xb8, 0x99, 0xa3, 0x0d,
    0x44, 0x0c, 0x10, 0x0a, 0x00, 0x2b, 0x06, 0x05, 0xc1, 0x90, 0xd0, 0x9b, 0x7a, 0x00, 0x00, 0x00,
    0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

pub struct FakeTelegram {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
    journal: Arc<Journal>,
}

/// Ce que le serveur garde : messages à remettre au bot, ce que le bot a
/// fait, requêtes reçues, refus commandés.
struct Journal {
    /// Mises à jour pas encore acquittées par un `offset`.
    updates: Mutex<Vec<Value>>,
    next_update: AtomicI64,
    next_message: AtomicI64,
    /// Change à chaque message arrivé et à l'arrêt : réveille les attentes
    /// longues.
    wake: watch::Sender<u64>,
    stopping: AtomicBool,
    sent: Mutex<Vec<Value>>,
    /// `{"method": "sendMessage", "body": {…}}`, dans l'ordre ; un fichier lu
    /// est noté `{"method": "file", "body": {"path": "…"}}`.
    requests: Mutex<Vec<Value>>,
    /// Refus à rendre à la prochaine requête de chaque méthode, dans l'ordre.
    failures: Mutex<Vec<(String, u16, Value)>>,
    /// Contenu actuel des messages du bot, par conversation et identifiant.
    messages: Mutex<HashMap<(String, i64), Written>>,
    /// Comptes qui ont bloqué le bot.
    blocked: Mutex<HashSet<String>>,
    /// Méthodes dont les requêtes attendent avant leur réponse.
    held: watch::Sender<HashSet<String>>,
}

impl Journal {
    fn new() -> Self {
        Self {
            updates: Mutex::default(),
            next_update: AtomicI64::new(1000),
            next_message: AtomicI64::new(1),
            wake: watch::Sender::new(0),
            stopping: AtomicBool::new(false),
            sent: Mutex::default(),
            requests: Mutex::default(),
            failures: Mutex::default(),
            messages: Mutex::default(),
            blocked: Mutex::default(),
            held: watch::Sender::new(HashSet::new()),
        }
    }

    fn wake(&self) {
        self.wake.send_modify(|version| *version += 1);
    }

    /// Fait arriver un message (JSON de `POST /fake/message`).
    fn message(&self, message: &Value) {
        let user = match &message["user"] {
            Value::Number(user) => user.as_i64(),
            Value::String(user) => user.parse().ok(),
            _ => None,
        }
        .unwrap_or(42);
        let name = message["name"].as_str().unwrap_or("Ana");
        let (first_name, last_name) = name.split_once(' ').unwrap_or((name, ""));
        let text = message["text"].as_str().unwrap_or_default();
        let image = message["image"] == true;
        let voice = message["voice"] == true;

        let mut from = json!({
            "id": user,
            "is_bot": message["bot"] == true,
            "first_name": first_name,
            "language_code": "en",
        });
        if !last_name.is_empty() {
            from["last_name"] = last_name.into();
        }
        let chat = if message["private"] == false {
            json!({ "id": -1_001_234_567_890_i64, "title": "Chatons", "type": "supergroup" })
        } else {
            json!({ "id": user, "first_name": first_name, "type": "private" })
        };
        let mut content = json!({
            "message_id": self.next_message.fetch_add(1, Ordering::SeqCst),
            "from": from,
            "chat": chat,
            "date": now() - message["age"].as_i64().unwrap_or(0),
        });
        if image {
            // Comme Telegram : plusieurs tailles, de la plus petite à la plus grande.
            content["photo"] = json!([
                { "file_id": PHOTO_THUMBNAIL, "file_unique_id": "fake-1", "width": 1, "height": 1, "file_size": PNG.len() },
                { "file_id": PHOTO, "file_unique_id": "fake-2", "width": 2, "height": 2, "file_size": PNG.len() },
            ]);
        }
        if voice {
            content["voice"] = json!({
                "file_id": VOICE,
                "file_unique_id": "fake-3",
                "duration": 2,
                "mime_type": "audio/ogg",
            });
            if let Ok(file) = std::fs::metadata(VOICE_FIXTURE) {
                content["voice"]["file_size"] = file.len().into();
            }
        }
        // Le texte d'un message à fichier est sa légende.
        if image || voice {
            if !text.is_empty() {
                content["caption"] = text.into();
            }
        } else {
            content["text"] = text.into();
        }

        let update = json!({
            "update_id": self.next_update.fetch_add(1, Ordering::SeqCst),
            "message": content,
        });
        self.updates.lock().unwrap().push(update);
        self.wake();
    }

    fn fail_next(&self, method: &str, status: u16, body: Value) {
        self.failures
            .lock()
            .unwrap()
            .push((method.to_owned(), status, body));
    }

    fn take_failure(&self, method: &str) -> Option<(u16, Value)> {
        let mut failures = self.failures.lock().unwrap();
        let position = failures.iter().position(|(name, ..)| name == method)?;
        let (_, status, body) = failures.remove(position);
        Some((status, body))
    }

    fn block(&self, user: &str) {
        self.blocked.lock().unwrap().insert(user.to_owned());
    }

    fn hold(&self, method: &str, held: bool) {
        self.held.send_modify(|methods| {
            if held {
                methods.insert(method.to_owned());
            } else {
                methods.remove(method);
            }
        });
    }

    /// Attend que `method` ne soit plus retenue.
    async fn wait_released(&self, method: &str) {
        let mut held = self.held.subscribe();
        let _ = held.wait_for(|methods| !methods.contains(method)).await;
    }
}

impl FakeTelegram {
    /// Démarre le serveur sur un port libre de 127.0.0.1.
    pub async fn start() -> Self {
        Self::start_on(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .expect("free local port")
    }

    pub async fn start_on(addr: SocketAddr) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let addr = listener.local_addr()?;
        let (shutdown, stop) = oneshot::channel::<()>();
        let journal = Arc::new(Journal::new());
        let app = router(journal.clone());
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
            journal,
        })
    }

    /// Adresse à donner au client à la place de `https://api.telegram.org`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Fait arriver un message : même JSON que `POST /fake/message`.
    pub fn message(&self, message: serde_json::Value) {
        self.journal.message(&message);
    }

    /// Ce que le bot a fait, dans l'ordre : même JSON que `GET /fake/sent`.
    pub fn sent(&self) -> Vec<serde_json::Value> {
        self.journal.sent.lock().unwrap().clone()
    }

    /// Requêtes reçues, dans l'ordre : même JSON que `GET /fake/requests`.
    pub fn requests(&self) -> Vec<serde_json::Value> {
        self.journal.requests.lock().unwrap().clone()
    }

    /// La prochaine requête à cette méthode (`getUpdates`, `sendMessage`…)
    /// reçoit ce statut et ce corps au lieu de sa réponse ; plusieurs appels
    /// se suivent dans l'ordre. Sert aussi à rendre une réponse arrangée
    /// (statut 200).
    pub fn fail_next(&self, method: &str, status: u16, body: serde_json::Value) {
        self.journal.fail_next(method, status, body);
    }

    /// Le compte `user` bloque le bot : toute écriture dans sa conversation
    /// privée (`sendMessage`, `editMessageText`, `sendChatAction`) est
    /// refusée (403), comme chez Telegram.
    pub fn block(&self, user: &str) {
        self.journal.block(user);
    }

    /// `held` : les requêtes à cette méthode, notées dès leur arrivée,
    /// attendent avant leur réponse ; `false` les laisse repartir. Sert à
    /// retenir l'app à un endroit précis.
    pub fn hold(&self, method: &str, held: bool) {
        self.journal.hold(method, held);
    }

    /// Arrête le serveur. Les attentes longues en cours sont réveillées, et le
    /// serveur est abandonné au bout d'une seconde si un client garde sa
    /// connexion ouverte.
    pub async fn stop(mut self) {
        self.release();
        if tokio::time::timeout(Duration::from_secs(1), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
        }
    }

    fn release(&mut self) {
        self.journal.stopping.store(true, Ordering::SeqCst);
        self.journal.wake();
        self.journal.held.send_replace(HashSet::new());
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

impl Drop for FakeTelegram {
    fn drop(&mut self) {
        self.release();
        self.task.abort();
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

fn router(journal: Arc<Journal>) -> Router {
    Router::new()
        // Le jeton est collé à « bot » dans le chemin : il est lu à la main.
        .route("/{bot}/{method}", any(method))
        .route("/file/{bot}/{*path}", get(file))
        .route("/fake/message", post(fake_message))
        .route("/fake/sent", get(fake_sent))
        .route("/fake/requests", get(fake_requests))
        .route("/fake/fail", post(fake_fail))
        .route("/fake/block", post(fake_block))
        .route("/fake/hold", post(fake_hold))
        .with_state(journal)
}

fn ok(result: Value) -> Response {
    Json(json!({ "ok": true, "result": result })).into_response()
}

fn refuse(status: u16, description: &str) -> Response {
    let body = json!({ "ok": false, "error_code": status, "description": description });
    reply(status, body)
}

fn reply(status: u16, body: Value) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(body)).into_response()
}

fn authorized(bot: &str) -> bool {
    bot.strip_prefix("bot") == Some(TOKEN)
}

async fn method(
    State(journal): State<Arc<Journal>>,
    Path((bot, method)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    if !authorized(&bot) {
        return refuse(401, "Unauthorized");
    }
    let body: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let request = json!({ "method": method, "body": body });
    journal.requests.lock().unwrap().push(request);
    journal.wait_released(&method).await;
    if let Some((status, body)) = journal.take_failure(&method) {
        return reply(status, body);
    }
    let writes = matches!(
        method.as_str(),
        "sendMessage" | "editMessageText" | "sendChatAction"
    );
    if writes && chat(&body).is_some_and(|chat| journal.blocked.lock().unwrap().contains(&chat)) {
        return refuse(403, "Forbidden: bot was blocked by the user");
    }
    match method.as_str() {
        "getMe" => ok(json!({
            "id": BOT_ID,
            "is_bot": true,
            "first_name": "ChatonLM test",
            "username": BOT_NAME,
            "can_join_groups": true,
            "can_read_all_group_messages": false,
            "supports_inline_queries": false,
        })),
        "getUpdates" => get_updates(&journal, &body).await,
        "sendMessage" => send_message(&journal, &body),
        "editMessageText" => edit_message(&journal, &body),
        "sendChatAction" => send_chat_action(&journal, &body),
        "getFile" => get_file(&body),
        _ => refuse(404, "Not Found: method not found"),
    }
}

/// Attente longue : répond dès qu'un message est là, sinon un tableau vide à
/// la fin du `timeout` demandé. `offset` acquitte les mises à jour d'avant.
async fn get_updates(journal: &Journal, body: &Value) -> Response {
    let offset = body["offset"].as_i64();
    let deadline =
        tokio::time::Instant::now() + Duration::from_secs(body["timeout"].as_u64().unwrap_or(0));
    // Abonné avant de regarder : un message arrivé entre-temps réveille.
    let mut wake = journal.wake.subscribe();
    loop {
        let ready = {
            let mut updates = journal.updates.lock().unwrap();
            if let Some(offset) = offset {
                updates.retain(|update| update["update_id"].as_i64() >= Some(offset));
            }
            updates.clone()
        };
        if !ready.is_empty() || journal.stopping.load(Ordering::SeqCst) {
            return ok(Value::Array(ready));
        }
        tokio::select! {
            changed = wake.changed() => {
                if changed.is_err() {
                    return ok(json!([]));
                }
            }
            () = tokio::time::sleep_until(deadline) => return ok(json!([])),
        }
    }
}

/// Conversation visée, en texte (le client l'envoie en nombre ou en texte).
fn chat(body: &Value) -> Option<String> {
    match &body["chat_id"] {
        Value::Number(chat) => Some(chat.to_string()),
        Value::String(chat) if !chat.is_empty() => Some(chat.clone()),
        _ => None,
    }
}

/// Texte d'un message du bot, tel que reçu, et sa mise en forme.
#[derive(PartialEq)]
struct Written {
    text: String,
    mode: Option<String>,
}

/// Contenu d'un message à écrire, ou la description du refus de Telegram
/// (400).
fn content(body: &Value) -> Result<Written, String> {
    let text = body["text"].as_str().unwrap_or_default();
    let mode = body["parse_mode"].as_str();
    let shown = match mode {
        Some("HTML") => parse_html(text)
            .map_err(|error| format!("Bad Request: can't parse entities: {error}"))?,
        _ => text.to_owned(),
    };
    if shown.trim().is_empty() {
        return Err("Bad Request: message text is empty".to_owned());
    }
    if shown.encode_utf16().count() > TEXT_LIMIT {
        return Err("Bad Request: message is too long".to_owned());
    }
    Ok(Written {
        text: text.to_owned(),
        mode: mode.map(str::to_owned),
    })
}

fn journal_entry(kind: &str, channel: &str, id: i64, written: &Written) -> Value {
    let mut entry =
        json!({ "kind": kind, "channel": channel, "id": id.to_string(), "text": written.text });
    if let Some(mode) = &written.mode {
        entry["parseMode"] = mode.as_str().into();
    }
    entry
}

fn sent_message(id: i64, body: &Value, text: &str) -> Value {
    json!({
        "message_id": id,
        "from": { "id": BOT_ID, "is_bot": true, "first_name": "ChatonLM test", "username": BOT_NAME },
        "chat": { "id": body["chat_id"], "type": "private" },
        "date": now(),
        "text": text,
    })
}

fn send_message(journal: &Journal, body: &Value) -> Response {
    let Some(channel) = chat(body) else {
        return refuse(400, "Bad Request: chat_id is empty");
    };
    let written = match content(body) {
        Ok(written) => written,
        Err(description) => return refuse(400, &description),
    };
    let id = journal.next_message.fetch_add(1, Ordering::SeqCst);
    let entry = journal_entry("send", &channel, id, &written);
    journal.sent.lock().unwrap().push(entry);
    let result = sent_message(id, body, &written.text);
    journal
        .messages
        .lock()
        .unwrap()
        .insert((channel, id), written);
    ok(result)
}

fn edit_message(journal: &Journal, body: &Value) -> Response {
    let Some(channel) = chat(body) else {
        return refuse(400, "Bad Request: chat_id is empty");
    };
    let id = match &body["message_id"] {
        Value::Number(id) => id.as_i64(),
        Value::String(id) => id.parse().ok(),
        _ => None,
    };
    let key = (channel, id.unwrap_or(0));
    let mut messages = journal.messages.lock().unwrap();
    let Some(current) = messages.get_mut(&key) else {
        return refuse(400, "Bad Request: message to edit not found");
    };
    let new = match content(body) {
        Ok(written) => written,
        Err(description) => return refuse(400, &description),
    };
    if *current == new {
        return refuse(
            400,
            "Bad Request: message is not modified: specified new message content and reply markup are exactly the \
             same as a current content and reply markup of the message",
        );
    }
    let entry = journal_entry("edit", &key.0, key.1, &new);
    journal.sent.lock().unwrap().push(entry);
    let result = sent_message(key.1, body, &new.text);
    *current = new;
    ok(result)
}

fn send_chat_action(journal: &Journal, body: &Value) -> Response {
    let Some(channel) = chat(body) else {
        return refuse(400, "Bad Request: chat_id is empty");
    };
    if body["action"] != "typing" {
        return refuse(400, "Bad Request: wrong parameter action in request");
    }
    let entry = json!({ "kind": "typing", "channel": channel });
    journal.sent.lock().unwrap().push(entry);
    ok(json!(true))
}

/// Chemin et contenu d'un fichier joint (contenu absent : fichier d'essai
/// manquant sur le disque).
fn stored_file(file_id: &str) -> Option<(&'static str, Option<Vec<u8>>)> {
    match file_id {
        PHOTO | PHOTO_THUMBNAIL => Some((PHOTO_PATH, Some(PNG.to_vec()))),
        VOICE => Some((VOICE_PATH, std::fs::read(VOICE_FIXTURE).ok())),
        _ => None,
    }
}

fn get_file(body: &Value) -> Response {
    let file_id = body["file_id"].as_str().unwrap_or_default();
    let Some((path, content)) = stored_file(file_id) else {
        return refuse(400, "Bad Request: invalid file_id");
    };
    let mut result = json!({ "file_id": file_id, "file_unique_id": format!("unique-{file_id}"), "file_path": path });
    if let Some(content) = content {
        result["file_size"] = content.len().into();
    }
    ok(result)
}

async fn file(
    State(journal): State<Arc<Journal>>,
    Path((bot, path)): Path<(String, String)>,
) -> Response {
    if !authorized(&bot) {
        return refuse(401, "Unauthorized");
    }
    let request = json!({ "method": "file", "body": { "path": path } });
    journal.requests.lock().unwrap().push(request);
    if let Some((status, body)) = journal.take_failure("file") {
        return reply(status, body);
    }
    let content = [PHOTO, VOICE]
        .into_iter()
        .filter_map(stored_file)
        .find(|(stored, _)| *stored == path)
        .and_then(|(_, content)| content);
    match content {
        Some(content) => (
            [(header::CONTENT_TYPE, "application/octet-stream")],
            content,
        )
            .into_response(),
        None => refuse(404, "Not Found"),
    }
}

async fn fake_message(
    State(journal): State<Arc<Journal>>,
    Json(message): Json<Value>,
) -> Json<Value> {
    journal.message(&message);
    Json(json!({ "ok": true }))
}

async fn fake_sent(State(journal): State<Arc<Journal>>) -> Json<Value> {
    Json(Value::Array(journal.sent.lock().unwrap().clone()))
}

async fn fake_requests(State(journal): State<Arc<Journal>>) -> Json<Value> {
    Json(Value::Array(journal.requests.lock().unwrap().clone()))
}

async fn fake_fail(State(journal): State<Arc<Journal>>, Json(failure): Json<Value>) -> Response {
    let (Some(method), Some(status)) = (failure["method"].as_str(), failure["status"].as_u64())
    else {
        return reply(
            400,
            json!({ "ok": false, "description": "method and status are required" }),
        );
    };
    journal.fail_next(method, status as u16, failure["body"].clone());
    Json(json!({ "ok": true })).into_response()
}

async fn fake_block(State(journal): State<Arc<Journal>>, Json(block): Json<Value>) -> Response {
    let user = match &block["user"] {
        Value::Number(user) => user.to_string(),
        Value::String(user) if !user.is_empty() => user.clone(),
        _ => {
            return reply(
                400,
                json!({ "ok": false, "description": "user is required" }),
            );
        }
    };
    journal.block(&user);
    Json(json!({ "ok": true })).into_response()
}

async fn fake_hold(State(journal): State<Arc<Journal>>, Json(hold): Json<Value>) -> Response {
    let (Some(method), Some(held)) = (hold["method"].as_str(), hold["held"].as_bool()) else {
        return reply(
            400,
            json!({ "ok": false, "description": "method and held are required" }),
        );
    };
    journal.hold(method, held);
    Json(json!({ "ok": true })).into_response()
}

/// Lit un texte `parse_mode=HTML` comme Telegram : rend le texte montré, ou
/// la raison du refus (balise inconnue, non fermée, mal imbriquée). Les
/// essais de l'app attrapent ainsi un rendu que le vrai service refuserait.
fn parse_html(html: &str) -> Result<String, String> {
    let mut shown = String::new();
    let mut open: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find(['<', '&']) {
        shown.push_str(&rest[..at]);
        rest = &rest[at..];
        let offset = html.len() - rest.len();
        if rest.starts_with('&') {
            let (text, length) = entity(rest);
            shown.push_str(&text);
            rest = &rest[length..];
            continue;
        }
        let end =
            tag_end(rest).ok_or_else(|| format!("Unclosed start tag at byte offset {offset}"))?;
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        if let Some(name) = tag.strip_prefix('/') {
            let name = name.trim().to_ascii_lowercase();
            match open.pop() {
                Some(expected) if expected == name => {}
                Some(expected) => {
                    return Err(format!(
                        "Unmatched end tag at byte offset {offset}, expected \"</{expected}>\", found \"</{name}>\""
                    ));
                }
                None => return Err(format!("Unexpected end tag at byte offset {offset}")),
            }
            continue;
        }
        let (name, attributes) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
        let name = name.to_ascii_lowercase();
        let allowed = match name.as_str() {
            "b" | "strong" | "i" | "em" | "u" | "ins" | "s" | "strike" | "del" | "code" | "pre"
            | "blockquote" | "tg-spoiler" => true,
            "a" => attributes.contains("href="),
            "span" => attributes.contains("class=\"tg-spoiler\""),
            "tg-emoji" => attributes.contains("emoji-id="),
            _ => false,
        };
        if !allowed {
            return Err(format!(
                "Unsupported start tag \"{name}\" at byte offset {offset}"
            ));
        }
        open.push(name);
    }
    shown.push_str(rest);
    match open.pop() {
        Some(name) => Err(format!(
            "Can't find end tag corresponding to start tag \"{name}\""
        )),
        None => Ok(shown),
    }
}

/// Position du « > » qui finit la balise ouverte au début de `text` (un « > »
/// entre guillemets est une valeur d'attribut).
fn tag_end(text: &str) -> Option<usize> {
    let mut quoted = false;
    for (at, c) in text.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '>' if !quoted => return Some(at),
            _ => {}
        }
    }
    None
}

/// Entité au début de `text` : son texte et sa longueur. Une entité inconnue
/// reste telle quelle, comme chez Telegram.
fn entity(text: &str) -> (String, usize) {
    for (name, shown) in [
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&amp;", "&"),
        ("&quot;", "\""),
    ] {
        if text.starts_with(name) {
            return (shown.to_owned(), name.len());
        }
    }
    let numeric = text.strip_prefix("&#").and_then(|code| {
        let (digits, _) = code.split_once(';')?;
        let value = match digits.strip_prefix('x') {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => digits.parse().ok()?,
        };
        Some((char::from_u32(value)?.to_string(), digits.len() + 3))
    });
    numeric.unwrap_or_else(|| ("&".to_owned(), 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http() -> reqwest::Client {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("client")
    }

    async fn call(fake: &FakeTelegram, method: &str, body: Value) -> (u16, Value) {
        let url = format!("{}/bot{TOKEN}/{method}", fake.url());
        let response = http().post(url).json(&body).send().await.expect("response");
        let status = response.status().as_u16();
        (status, response.json().await.expect("json"))
    }

    #[test]
    fn html_is_read_like_telegram() {
        assert_eq!(
            parse_html("a &lt; b &amp;&amp; c &gt; d &quot;e&quot; &#233; &#x1F431; &nbsp;")
                .unwrap(),
            "a < b && c > d \"e\" é 🐱 &nbsp;"
        );
        assert_eq!(
            parse_html(
                "<b>gras <i>italique</i></b> <a href=\"https://exemple.fr/?a=1&amp;b=>\">lien</a>"
            )
            .unwrap(),
            "gras italique lien"
        );
        assert_eq!(
            parse_html(
                "<blockquote><pre><code class=\"language-rust\">x</code></pre></blockquote>"
            )
            .unwrap(),
            "x"
        );
        assert!(
            parse_html("<b>jamais fermé")
                .unwrap_err()
                .contains("Can't find end tag")
        );
        assert!(
            parse_html("<b><i>croisé</b></i>")
                .unwrap_err()
                .contains("Unmatched end tag")
        );
        assert!(
            parse_html("fermé</b>")
                .unwrap_err()
                .contains("Unexpected end tag")
        );
        assert!(
            parse_html("<h1>titre</h1>")
                .unwrap_err()
                .contains("Unsupported start tag \"h1\"")
        );
        assert!(
            parse_html("<a>sans adresse</a>")
                .unwrap_err()
                .contains("Unsupported start tag")
        );
        assert!(
            parse_html("1 < 2")
                .unwrap_err()
                .contains("Unclosed start tag")
        );
        assert!(
            parse_html("a < b > c")
                .unwrap_err()
                .contains("Unsupported start tag")
        );
    }

    #[test]
    fn the_png_is_a_valid_image_header() {
        assert_eq!(&PNG[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&PNG[12..16], b"IHDR");
        assert_eq!(&PNG[PNG.len() - 8..PNG.len() - 4], b"IEND");
    }

    #[tokio::test]
    async fn an_unknown_token_is_refused_like_telegram() {
        let fake = FakeTelegram::start().await;
        let url = format!("{}/bot1:other/getMe", fake.url());
        let response = http().post(url).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 401);
        let body: Value = response.json().await.unwrap();
        assert_eq!(
            body,
            json!({ "ok": false, "error_code": 401, "description": "Unauthorized" })
        );
        let file = format!("{}/file/bot1:other/{PHOTO_PATH}", fake.url());
        assert_eq!(
            http().get(file).send().await.unwrap().status().as_u16(),
            401
        );
        assert!(fake.requests().is_empty());
        fake.stop().await;
    }

    #[tokio::test]
    async fn get_updates_waits_for_a_message_and_honours_the_offset() {
        let fake = FakeTelegram::start().await;
        // Sans attente demandée : réponse vide tout de suite.
        let (_, empty) = call(&fake, "getUpdates", json!({ "timeout": 0 })).await;
        assert_eq!(empty, json!({ "ok": true, "result": [] }));

        // Attente longue : la réponse part à l'arrivée du message.
        let waiting = tokio::spawn({
            let url = format!("{}/bot{TOKEN}/getUpdates", fake.url());
            async move {
                let response = http()
                    .post(url)
                    .json(&json!({ "timeout": 30 }))
                    .send()
                    .await
                    .unwrap();
                response.json::<Value>().await.unwrap()
            }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!waiting.is_finished());
        fake.message(json!({ "user": "7", "name": "Ana Lopez", "text": "bonjour", "age": 600 }));
        let reply = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap();
        let update = &reply["result"][0];
        let message = &update["message"];
        assert_eq!(message["text"], "bonjour");
        assert_eq!(
            message["chat"],
            json!({ "id": 7, "first_name": "Ana", "type": "private" })
        );
        assert_eq!(message["from"]["last_name"], "Lopez");
        assert_eq!(message["from"]["is_bot"], false);
        assert!(message["date"].as_i64().unwrap() <= now() - 600);

        // Pas acquitté : remis encore ; acquitté par `offset` : plus jamais.
        let id = update["update_id"].as_i64().unwrap();
        let (_, again) = call(&fake, "getUpdates", json!({ "offset": id, "timeout": 0 })).await;
        assert_eq!(again["result"].as_array().unwrap().len(), 1);
        let (_, after) = call(
            &fake,
            "getUpdates",
            json!({ "offset": id + 1, "timeout": 0 }),
        )
        .await;
        assert_eq!(after["result"], json!([]));
        let (_, later) = call(&fake, "getUpdates", json!({ "timeout": 0 })).await;
        assert_eq!(later["result"], json!([]));
        fake.stop().await;
    }

    #[tokio::test]
    async fn messages_can_come_from_a_group_a_bot_or_with_files() {
        let fake = FakeTelegram::start().await;
        let posted = http()
            .post(format!("{}/fake/message", fake.url()))
            .json(&json!({ "user": 5, "name": "Zoé", "text": "du groupe", "private": false }))
            .send()
            .await
            .unwrap();
        assert!(posted.status().is_success());
        fake.message(json!({ "user": "6", "text": "bip", "bot": true }));
        fake.message(json!({ "user": "7", "text": "regarde", "image": true }));
        fake.message(json!({ "user": "7", "voice": true }));
        let (_, reply) = call(&fake, "getUpdates", json!({})).await;
        let updates = reply["result"].as_array().unwrap();
        assert_eq!(updates.len(), 4);
        assert_eq!(updates[0]["message"]["chat"]["type"], "supergroup");
        assert_eq!(updates[1]["message"]["from"]["is_bot"], true);
        assert_eq!(updates[2]["message"]["caption"], "regarde");
        assert_eq!(updates[2]["message"]["photo"][1]["file_id"], PHOTO);
        assert!(updates[2]["message"].get("text").is_none());
        assert_eq!(updates[3]["message"]["voice"]["file_id"], VOICE);
        assert!(updates[3]["message"].get("caption").is_none());
        fake.stop().await;
    }

    #[tokio::test]
    async fn what_the_bot_does_is_kept_in_order() {
        let fake = FakeTelegram::start().await;
        let (_, typing) = call(
            &fake,
            "sendChatAction",
            json!({ "chat_id": 7, "action": "typing" }),
        )
        .await;
        assert_eq!(typing["result"], true);
        let body = json!({ "chat_id": 7, "text": "<b>bonjour</b>", "parse_mode": "HTML" });
        let (_, sent) = call(&fake, "sendMessage", body.clone()).await;
        let id = sent["result"]["message_id"].as_i64().unwrap();
        let edit = json!({ "chat_id": "7", "message_id": id, "text": "au revoir" });
        let (_, edited) = call(&fake, "editMessageText", edit.clone()).await;
        assert_eq!(edited["ok"], true);

        let journal = json!([
            { "kind": "typing", "channel": "7" },
            { "kind": "send", "channel": "7", "id": id.to_string(), "text": "<b>bonjour</b>", "parseMode": "HTML" },
            { "kind": "edit", "channel": "7", "id": id.to_string(), "text": "au revoir" },
        ]);
        assert_eq!(Value::Array(fake.sent()), journal);
        let over_http: Value = http()
            .get(format!("{}/fake/sent", fake.url()))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(over_http, journal);

        // Même texte : refus de Telegram, rien de plus au journal.
        let (status, same) = call(&fake, "editMessageText", edit).await;
        assert_eq!(status, 400);
        assert!(
            same["description"]
                .as_str()
                .unwrap()
                .contains("message is not modified")
        );
        let unknown = json!({ "chat_id": 7, "message_id": 999, "text": "x" });
        let (_, missing) = call(&fake, "editMessageText", unknown).await;
        assert_eq!(
            missing["description"],
            "Bad Request: message to edit not found"
        );
        assert_eq!(fake.sent().len(), 3);
        fake.stop().await;
    }

    #[tokio::test]
    async fn a_text_telegram_would_refuse_is_refused() {
        let fake = FakeTelegram::start().await;
        for text in [
            "<b>jamais fermé",
            "<h1>titre</h1>",
            "1 < 2",
            "<b><i>croisé</b></i>",
        ] {
            let body = json!({ "chat_id": 7, "text": text, "parse_mode": "HTML" });
            let (status, refused) = call(&fake, "sendMessage", body).await;
            assert_eq!(status, 400, "{text}");
            assert_eq!(refused["error_code"], 400);
            let description = refused["description"].as_str().unwrap();
            assert!(
                description.starts_with("Bad Request: can't parse entities: "),
                "{description}"
            );
            // Le même texte sans mise en forme passe.
            let (_, plain) =
                call(&fake, "sendMessage", json!({ "chat_id": 7, "text": text })).await;
            assert_eq!(plain["ok"], true, "{text}");
        }
        for text in ["", "  \n", "<b> </b>"] {
            let body = json!({ "chat_id": 7, "text": text, "parse_mode": "HTML" });
            let (_, refused) = call(&fake, "sendMessage", body).await;
            assert_eq!(
                refused["description"], "Bad Request: message text is empty",
                "{text:?}"
            );
        }
        // 4096 unités au plus, comptées sur le texte montré.
        let long = format!("<b>{}</b>", "🐱".repeat(2048));
        let (_, fits) = call(
            &fake,
            "sendMessage",
            json!({ "chat_id": 7, "text": long, "parse_mode": "HTML" }),
        )
        .await;
        assert_eq!(fits["ok"], true);
        let longer = format!("{}é", "🐱".repeat(2048));
        let (_, refused) = call(
            &fake,
            "sendMessage",
            json!({ "chat_id": 7, "text": longer }),
        )
        .await;
        assert_eq!(refused["description"], "Bad Request: message is too long");
        let (_, refused) = call(&fake, "sendMessage", json!({ "text": "sans destinataire" })).await;
        assert_eq!(refused["error_code"], 400);
        assert_eq!(fake.sent().len(), 5);
        fake.stop().await;
    }

    #[tokio::test]
    async fn an_ordered_failure_replaces_the_next_reply_of_its_method_only() {
        let fake = FakeTelegram::start().await;
        let conflict = json!({ "ok": false, "error_code": 409, "description": "Conflict" });
        fake.fail_next("getUpdates", 409, conflict.clone());
        let ordered = http()
            .post(format!("{}/fake/fail", fake.url()))
            .json(&json!({ "method": "getUpdates", "status": 502, "body": { "ok": false } }))
            .send()
            .await
            .unwrap();
        assert!(ordered.status().is_success());

        assert_eq!(call(&fake, "getMe", json!({})).await.0, 200);
        assert_eq!(call(&fake, "getUpdates", json!({})).await, (409, conflict));
        assert_eq!(
            call(&fake, "getUpdates", json!({})).await,
            (502, json!({ "ok": false }))
        );
        assert_eq!(call(&fake, "getUpdates", json!({})).await.0, 200);
        assert_eq!(call(&fake, "noSuchMethod", json!({})).await.0, 404);
        let methods: Vec<Value> = fake
            .requests()
            .iter()
            .map(|request| request["method"].clone())
            .collect();
        assert_eq!(
            methods,
            [
                "getMe",
                "getUpdates",
                "getUpdates",
                "getUpdates",
                "noSuchMethod"
            ]
        );
        fake.stop().await;
    }

    #[tokio::test]
    async fn a_user_who_blocked_the_bot_refuses_every_write() {
        let fake = FakeTelegram::start().await;
        let (_, sent) = call(
            &fake,
            "sendMessage",
            json!({ "chat_id": 7, "text": "bonjour" }),
        )
        .await;
        let id = sent["result"]["message_id"].as_i64().unwrap();
        fake.block("7");
        let blocked = http()
            .post(format!("{}/fake/block", fake.url()))
            .json(&json!({ "user": 8 }))
            .send()
            .await
            .unwrap();
        assert!(blocked.status().is_success());

        let refusal = json!({
            "ok": false, "error_code": 403, "description": "Forbidden: bot was blocked by the user"
        });
        for chat in [json!(7), json!("8")] {
            for (method, body) in [
                ("sendMessage", json!({ "chat_id": chat, "text": "encore" })),
                (
                    "editMessageText",
                    json!({ "chat_id": chat, "message_id": id, "text": "modifié" }),
                ),
                (
                    "sendChatAction",
                    json!({ "chat_id": chat, "action": "typing" }),
                ),
            ] {
                assert_eq!(
                    call(&fake, method, body).await,
                    (403, refusal.clone()),
                    "{method}"
                );
            }
        }
        // Les autres conversations écrivent ; les refus ne sont pas au journal
        // de ce que le bot a fait, seulement à celui des requêtes.
        let (_, other) = call(
            &fake,
            "sendMessage",
            json!({ "chat_id": 9, "text": "salut" }),
        )
        .await;
        assert_eq!(other["ok"], true);
        assert_eq!(fake.sent().len(), 2);
        assert_eq!(fake.requests().len(), 8);
        fake.stop().await;
    }

    #[tokio::test]
    async fn a_held_method_answers_once_let_go() {
        let fake = FakeTelegram::start().await;
        fake.hold("getFile", true);
        let url = format!("{}/bot{TOKEN}/getFile", fake.url());
        let waiting = tokio::spawn(async move {
            let response = http()
                .post(url)
                .json(&json!({ "file_id": PHOTO }))
                .send()
                .await
                .unwrap();
            response.json::<Value>().await.unwrap()
        });
        // Notée dès son arrivée, la requête attend ; les autres méthodes
        // répondent.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while fake.requests().is_empty() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "requête jamais reçue"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(call(&fake, "getMe", json!({})).await.0, 200);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!waiting.is_finished());

        let released = http()
            .post(format!("{}/fake/hold", fake.url()))
            .json(&json!({ "method": "getFile", "held": false }))
            .send()
            .await
            .unwrap();
        assert!(released.status().is_success());
        let reply = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("relâchée")
            .unwrap();
        assert_eq!(reply["result"]["file_path"], PHOTO_PATH);

        // L'arrêt du serveur relâche aussi ce qui est retenu.
        fake.hold("getMe", true);
        let url = format!("{}/bot{TOKEN}/getMe", fake.url());
        let waiting = tokio::spawn(async move { http().post(url).send().await.ok() });
        tokio::time::sleep(Duration::from_millis(100)).await;
        tokio::time::timeout(Duration::from_secs(3), fake.stop())
            .await
            .expect("arrêté");
        tokio::time::timeout(Duration::from_secs(3), waiting)
            .await
            .expect("relâchée par l'arrêt")
            .unwrap();
    }

    #[tokio::test]
    async fn files_are_served_by_their_path() {
        let fake = FakeTelegram::start().await;
        let (_, info) = call(&fake, "getFile", json!({ "file_id": PHOTO })).await;
        assert_eq!(info["result"]["file_path"], PHOTO_PATH);
        assert_eq!(info["result"]["file_size"], PNG.len());
        let url = format!("{}/file/bot{TOKEN}/{PHOTO_PATH}", fake.url());
        let content = http().get(url).send().await.unwrap().bytes().await.unwrap();
        assert_eq!(content.as_ref(), PNG);

        let (status, unknown) = call(&fake, "getFile", json!({ "file_id": "inconnu" })).await;
        assert_eq!((status, unknown["ok"].clone()), (400, json!(false)));
        let missing = format!("{}/file/bot{TOKEN}/photos/absent.png", fake.url());
        assert_eq!(
            http().get(missing).send().await.unwrap().status().as_u16(),
            404
        );

        // Le message vocal vient du disque : servi s'il existe, 404 sinon.
        let (_, voice) = call(&fake, "getFile", json!({ "file_id": VOICE })).await;
        assert_eq!(voice["result"]["file_path"], VOICE_PATH);
        let url = format!("{}/file/bot{TOKEN}/{VOICE_PATH}", fake.url());
        let response = http().get(url).send().await.unwrap();
        match std::fs::read(VOICE_FIXTURE) {
            Ok(fixture) => assert_eq!(response.bytes().await.unwrap().as_ref(), fixture),
            Err(_) => assert_eq!(response.status().as_u16(), 404),
        }
        fake.stop().await;
    }

    #[tokio::test]
    async fn stopping_releases_a_long_wait() {
        let fake = FakeTelegram::start().await;
        let waiting = tokio::spawn({
            let url = format!("{}/bot{TOKEN}/getUpdates", fake.url());
            async move {
                let response = http()
                    .post(url)
                    .json(&json!({ "timeout": 50 }))
                    .send()
                    .await
                    .ok()?;
                response.json::<Value>().await.ok()
            }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        tokio::time::timeout(Duration::from_secs(3), fake.stop())
            .await
            .expect("stopped");
        let reply = tokio::time::timeout(Duration::from_secs(3), waiting)
            .await
            .expect("released")
            .unwrap();
        // Réveillée par l'arrêt : réponse vide, ou connexion coupée.
        assert!(reply.is_none_or(|reply| reply["result"] == json!([])));
    }
}
