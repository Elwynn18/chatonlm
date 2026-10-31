//! Une connexion du pont : JSON-RPC 2.0, liste blanche de méthodes.
//!
//! Avant l'authentification, seulement `initialize`, `pair` et `auth`, et
//! cinq secondes pour s'authentifier. Ensuite : `ping`, `app.state`,
//! `models.list`, `models.details`, `chat.send` (flux en notifications
//! `chat.event`), `chat.get`, `chat.list`, `chat.forget` et `chat.keep` (une
//! éphémère de l'éditeur seulement), et `$/cancelRequest` pour arrêter un
//! envoi, même pas encore commencé. Une conversation ouverte par l'éditeur
//! est marquée « VS Code ». Chaque requête authentifiée tourne dans sa
//! tâche : la lecture continue (annulation possible pendant un flux). Une
//! connexion fermée n'arrête pas un envoi en cours : il s'enregistre, comme
//! celui d'un bot (sauf dans une conversation éphémère, alors effacée).
//! Aucun message écrit ne dépasse la trame ([`frame::MAX_BODY`]) : une
//! réponse trop grosse devient un refus. Voix (lot 2) : `tour.plan`,
//! `speech.read` / `speech.stop` (évènements `speech.event`), `voice.start`,
//! `voice.stop`, `voice.mute`, `voice.resume`, `voice.context`
//! (notifications `voice.state`, `voice.level`, `voice.chat`) ; la fin de la
//! connexion, même par une panique de sa tâche, arrête sa séance et sa
//! lecture. `$/cancelRequest` arrête aussi un `tour.plan` (requête au modèle
//! abandonnée, réponse -32800). Carte graphique et autocomplétion (lot 3,
//! [`super::gpu`]) : `gpu.status` (puis `gpu.changed`), `completion.prepare`,
//! `complete.fim` (une seule en vol par connexion : la nouvelle annule
//! l'ancienne), `completion.measure`, `completion.suggest`,
//! `models.offerPull` ; une proposition ou une mesure s'arrête en pause dès
//! qu'une réponse de conversation commence. Lot 4a ([`super::work`]) :
//! `notify.send`, `notify.task` (Easy Link prévient, `/status`),
//! `study.create`, `study.list` (puis `study.event`), `image.generate` (puis
//! `image.event`) et `proofread.run` (puis `proofread.event`), ces deux-là
//! annulables par `$/cancelRequest`, et `app.show` (liste blanche de pages).
//! Lot 4b ([`super::dictation`]) : `dictation.start`, `dictation.stop`,
//! `dictation.cancel` (notifications `dictation.state`, `dictation.text`,
//! `dictation.error`) ; la fin de la connexion arrête aussi sa dictée.
//! Accès agents (corrections 97) : `agents.issue` et `agents.revoke` (séance
//! d'un éditeur seulement) ; une séance de l'accès agents n'a droit qu'à
//! [`AGENT_METHODS`] (sinon -32037), et seulement à une NOUVELLE
//! conversation éphémère, qu'elle seule peut effacer, avec un modèle de
//! cette machine ; sa correction est toujours `localOnly`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, watch};

use super::frame::{self, FrameError};
use super::proof::{self, PairError};
use super::{Host, Io, PROTOCOL, STREAM_PREFIX, Shared};
use crate::chat::stream::Think;
use crate::chat::{self, ChatEvent, Origin, SendRequest, Server};
use crate::i18n::t;
use crate::store::bridge::{self as saved, Client, Scope};
use crate::store::{chats, model_prefs, now_ms};

/// Conversations de l'éditeur rendues par `chat.list`, au plus.
pub const LISTED_CHATS: u32 = 50;

/// Délai pour s'authentifier après l'ouverture.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(5);

/// Seules méthodes d'une séance de l'accès agents (outils MCP de
/// l'extension : question, modèles, image, lecture, correction).
pub const AGENT_METHODS: [&str; 11] = [
    "ping",
    "$/cancelRequest",
    "app.state",
    "models.list",
    "models.details",
    "chat.send",
    "chat.forget",
    "image.generate",
    "speech.read",
    "speech.stop",
    "proofread.run",
];

/// Codes d'erreur (JSON-RPC, puis propres au pont). L'extension traduit
/// d'après le code ; les messages ci-dessous ne sont jamais montrés.
pub mod code {
    pub const PARSE: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const UNAUTHENTICATED: i64 = -32001;
    pub const UNKNOWN_CLIENT: i64 = -32002;
    pub const NO_PAIRING: i64 = -32003;
    pub const TOO_MANY_ATTEMPTS: i64 = -32004;
    pub const WRONG_CODE: i64 = -32005;
    pub const INCOMPATIBLE: i64 = -32010;
    /// L'autocomplétion attend (`data: { reason }`).
    pub const PAUSED: i64 = -32007;
    pub const REFUSED: i64 = -32020;
    /// Easy Link : aucun compte lié à un bot connecté (`notify.*`).
    pub const NOT_LINKED: i64 = -32034;
    /// « Let VS Code notify me through Easy Link » coupé (`notify.*`).
    pub const NOTIFY_OFF: i64 = -32035;
    /// Aucune image possible (`image.generate`, `data: { reason, message }`).
    pub const NO_IMAGE_MODEL: i64 = -32036;
    /// Méthode ou demande interdite à cette séance (accès agents : hors de
    /// sa liste blanche, conversation non éphémère ou d'un autre client ;
    /// `agents.*` depuis un accès agents).
    pub const FORBIDDEN: i64 = -32037;
    /// Requête arrêtée par l'éditeur (`voice.stop` pendant `voice.start`,
    /// `dictation.stop` avant l'ouverture du micro, `$/cancelRequest` d'un
    /// `tour.plan`) : `RequestCancelled` de LSP.
    pub const CANCELLED: i64 = -32800;
}

/// Messages à écrire, dans l'ordre.
#[derive(Clone)]
pub(super) struct Outbox(mpsc::UnboundedSender<Vec<u8>>);

impl Outbox {
    /// Faux si le message dépasse la trame : rien n'est écrit (l'éditeur
    /// fermerait la connexion, et la relirait en boucle).
    fn send(&self, value: &Value) -> bool {
        let body = value.to_string();
        if body.len() > frame::MAX_BODY {
            return false;
        }
        let _ = self.0.send(frame::encode(body.as_bytes()));
        true
    }

    /// Réponse ; trop grosse pour la trame, un refus à la place.
    pub(super) fn result(&self, id: &Value, result: Value) {
        if !self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result })) {
            self.error(
                id,
                code::REFUSED,
                "too large",
                Some(json!({ "message": t!("bridge.tooLarge") })),
            );
        }
    }

    pub(super) fn error(&self, id: &Value, code: i64, message: &str, data: Option<Value>) {
        let mut error = json!({ "code": code, "message": message });
        if let Some(data) = data {
            error["data"] = data;
        }
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "error": error }));
    }

    /// Notification ; trop grosse pour la trame, elle est perdue (la
    /// conversation se relit par `chat.get`).
    pub(super) fn notify(&self, method: &str, params: Value) {
        if !self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params })) {
            eprintln!("chatonlm: a {method} notification is too large for the bridge");
        }
    }
}

/// Où en est la connexion.
#[derive(Default)]
struct Session {
    /// Après `initialize` : (nonce de l'app, nonce de l'éditeur).
    nonces: Option<([u8; 32], Vec<u8>)>,
    /// Après `auth`.
    client: Option<String>,
    /// Séance de l'accès agents (après `auth`).
    agents: bool,
}

/// Requête annulable (`chat.send`, `tour.plan`), inscrite dès sa lecture
/// (avant toute attente).
struct Pending {
    /// Flux du moteur (`chat.send`).
    stream: Option<String>,
    /// `$/cancelRequest` reçu : l'envoi s'arrête même si le moteur ne l'a pas
    /// encore commencé.
    cancelled: bool,
    /// Réveille la requête qui attend (`tour.plan`) : elle abandonne.
    wake: Option<Arc<tokio::sync::Notify>>,
    /// Proposition d'autocomplétion (`complete.fim`) : une seule en vol.
    fim: bool,
    /// Mesure de la vitesse (`completion.measure`) : pas de proposition
    /// pendant ce temps (elle fausserait la mesure).
    measure: bool,
}

/// Requêtes annulables en cours, par identifiant de requête.
type Running = Arc<Mutex<HashMap<String, Pending>>>;

/// Retire un envoi de [`Running`] quand il finit, quelle que soit l'issue.
struct Registered<'a> {
    running: &'a Running,
    key: String,
}

impl Registered<'_> {
    fn cancelled(&self) -> bool {
        self.running
            .lock()
            .get(&self.key)
            .is_some_and(|pending| pending.cancelled)
    }

    fn cancel(&self) {
        if let Some(pending) = self.running.lock().get_mut(&self.key) {
            pending.cancelled = true;
        }
    }
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        self.running.lock().remove(&self.key);
    }
}

/// Fin de la connexion : sa voix arrêtée quand la tâche finit, même par une
/// panique (le micro n'est jamais laissé ouvert).
struct Ended<'a, H: Host> {
    shared: &'a Shared<H>,
    connection: u64,
}

impl<H: Host> Drop for Ended<'_, H> {
    fn drop(&mut self) {
        self.shared.connection_ended(self.connection);
    }
}

pub(super) async fn serve<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    stream: Box<dyn Io>,
    mut close: watch::Receiver<bool>,
) {
    let ended = Ended { shared, connection };
    let (read, mut write) = tokio::io::split(stream);
    let mut reader = BufReader::new(read);
    let (sender, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();
    let out = Outbox(sender);
    let writer = tokio::spawn(async move {
        while let Some(bytes) = outgoing.recv().await {
            if write.write_all(&bytes).await.is_err() {
                break;
            }
        }
        let _ = write.shutdown().await;
    });
    let mut session = Session::default();
    let running: Running = Arc::default();
    let deadline = tokio::time::Instant::now() + shared.config.auth_timeout;
    loop {
        let read = tokio::select! {
            // Seul `true` est jamais envoyé ; émetteur perdu : fermée aussi.
            _ = close.changed() => break,
            () = tokio::time::sleep_until(deadline), if session.client.is_none() => break,
            read = frame::read(&mut reader) => read,
        };
        match read {
            Ok(Some(body)) => {
                handle(shared, connection, &mut session, &running, &out, &body).await;
            }
            Ok(None) => break,
            Err(FrameError::Malformed | FrameError::TooLarge) => {
                out.error(&Value::Null, code::PARSE, "malformed frame", None);
                break;
            }
            Err(_) => break,
        }
    }
    // Plus rien de neuf ; les envois en cours finissent et s'enregistrent.
    drop(out);
    if *close.borrow() {
        writer.abort();
    } else {
        let _ = tokio::time::timeout(Duration::from_millis(200), writer).await;
    }
    // Micro et lecture de la connexion arrêtés (jamais laissés ouverts).
    drop(ended);
}

async fn handle<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    session: &mut Session,
    running: &Running,
    out: &Outbox,
    body: &[u8],
) {
    let Ok(message) = serde_json::from_slice::<Value>(body) else {
        out.error(&Value::Null, code::PARSE, "parse error", None);
        return;
    };
    let Some(object) = message.as_object() else {
        out.error(&Value::Null, code::INVALID_REQUEST, "invalid request", None);
        return;
    };
    let id = object.get("id").cloned();
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        // Réponse de l'éditeur (le pont ne lui demande rien) : ignorée.
        if id.is_none() || object.contains_key("result") || object.contains_key("error") {
            return;
        }
        out.error(
            &id.unwrap_or(Value::Null),
            code::INVALID_REQUEST,
            "invalid request",
            None,
        );
        return;
    };
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    if method == "$/cancelRequest" {
        if session.client.is_some()
            && let Some(id) = params.get("id")
            && let Some(pending) = running.lock().get_mut(&id.to_string())
        {
            // Sans effet sur le moteur avant `begin` : le drapeau prend le
            // relais (voir `chat_send`).
            pending.cancelled = true;
            if let Some(stream) = &pending.stream {
                shared.host.engine().cancel(stream);
            }
            if let Some(wake) = &pending.wake {
                wake.notify_one();
            }
        }
        return;
    }
    // Autres notifications : rien n'est attendu.
    let Some(id) = id else { return };
    // Accès agents : sa liste blanche seulement (ni appairage, ni
    // `agents.*`, ni historique, micro, réglages ou Easy Link).
    if session.agents && !AGENT_METHODS.contains(&method) {
        out.error(&id, code::FORBIDDEN, "not allowed", None);
        return;
    }

    match method {
        "initialize" => initialize(session, out, &id, params),
        "pair" => pair(shared, session, out, &id, params).await,
        "auth" => auth(shared, connection, session, out, &id, params).await,
        _ if session.client.is_none() => {
            out.error(&id, code::UNAUTHENTICATED, "not authenticated", None);
        }
        "ping" => out.result(&id, json!({})),
        "chat.send" => {
            // Inscrit avant toute attente : un `$/cancelRequest` lu juste
            // après le trouve.
            let number = shared.next_stream.fetch_add(1, Ordering::SeqCst);
            let stream = format!("{STREAM_PREFIX}{connection}:{number}");
            running.lock().insert(
                id.to_string(),
                Pending {
                    stream: Some(stream.clone()),
                    cancelled: false,
                    wake: None,
                    fim: false,
                    measure: false,
                },
            );
            let sender = Sender {
                connection,
                client: session.client.clone().unwrap_or_default(),
                agents: session.agents,
                stream,
            };
            let (shared, out, running) = (Arc::clone(shared), out.clone(), Arc::clone(running));
            tauri::async_runtime::spawn(async move {
                chat_send(&shared, sender, &running, &out, &id, params).await;
            });
        }
        // Dans l'ordre de lecture (rien n'y attend) : un `speech.stop` suit
        // toujours le `speech.read` qui l'a précédé.
        "speech.read" => super::voice::speech_read(shared, connection, out, &id, params),
        "speech.stop" => super::voice::speech_stop(shared, connection, out, &id, params),
        "voice.stop" => super::voice::voice_stop(shared, connection, out, &id, &params),
        // Dictée inscrite ici, avant toute attente : un `dictation.stop` lu
        // juste après la trouve.
        "dictation.start" => {
            if let Some(opened) =
                super::dictation::dictation_start(shared, connection, out, &id, params)
            {
                let out = out.clone();
                tauri::async_runtime::spawn(async move {
                    super::dictation::dictation_opened(&out, &id, opened).await;
                });
            }
        }
        "dictation.stop" => {
            super::dictation::dictation_stop(shared, connection, out, &id, &params);
        }
        "dictation.cancel" => {
            super::dictation::dictation_cancel(shared, connection, out, &id, &params);
        }
        "voice.context" => super::voice::voice_context(shared, connection, out, &id, params),
        "voice.start" => {
            // Séance inscrite ici, avant toute attente : un `voice.stop` lu
            // juste après la trouve.
            let client = session.client.clone().unwrap_or_default();
            if let Some(talk) =
                super::voice::voice_register(shared, connection, client, out, &id, params)
            {
                let (shared, out) = (Arc::clone(shared), out.clone());
                tauri::async_runtime::spawn(async move {
                    super::voice::voice_start(&shared, &out, &id, talk).await;
                });
            }
        }
        "image.generate" | "proofread.run" => {
            // Annulables par `$/cancelRequest` : inscrits avant toute attente.
            let wake = Arc::new(tokio::sync::Notify::new());
            running.lock().insert(
                id.to_string(),
                Pending {
                    stream: None,
                    cancelled: false,
                    wake: Some(Arc::clone(&wake)),
                    fim: false,
                    measure: false,
                },
            );
            let image = method == "image.generate";
            let agents = session.agents;
            let (shared, out, running) = (Arc::clone(shared), out.clone(), Arc::clone(running));
            tauri::async_runtime::spawn(async move {
                let _registered = Registered {
                    running: &running,
                    key: id.to_string(),
                };
                // Lâcher le futur arrête l'image (`sd-server`) ou la requête
                // de correction (Ollama arrête).
                tokio::select! {
                    () = async {
                        if image {
                            super::work::image_generate(&shared, &out, &id, params).await;
                        } else {
                            super::work::proofread_run(&shared, &out, &id, params, agents).await;
                        }
                    } => {}
                    () = wake.notified() => {
                        out.error(&id, code::CANCELLED, "cancelled", None);
                    }
                }
            });
        }
        "notify.send" | "notify.task" | "study.create" | "study.list" | "app.show" => {
            let (shared, out) = (Arc::clone(shared), out.clone());
            let method = method.to_owned();
            tauri::async_runtime::spawn(async move {
                match method.as_str() {
                    "notify.send" => super::work::notify_send(&shared, &out, &id, params).await,
                    "notify.task" => {
                        super::work::notify_task(&shared, connection, &out, &id, params).await;
                    }
                    "study.create" => {
                        super::work::study_create(&shared, connection, &out, &id, params).await;
                    }
                    "app.show" => super::work::app_show(&shared, &out, &id, params).await,
                    _ => {
                        super::work::study_list(&shared, connection, &out, &id, &params).await;
                    }
                }
            });
        }
        "tour.plan" => {
            // Annulable par `$/cancelRequest` : inscrit avant toute attente.
            let wake = Arc::new(tokio::sync::Notify::new());
            running.lock().insert(
                id.to_string(),
                Pending {
                    stream: None,
                    cancelled: false,
                    wake: Some(Arc::clone(&wake)),
                    fim: false,
                    measure: false,
                },
            );
            let (shared, out, running) = (Arc::clone(shared), out.clone(), Arc::clone(running));
            tauri::async_runtime::spawn(async move {
                let _registered = Registered {
                    running: &running,
                    key: id.to_string(),
                };
                // Abandonner le futur ferme la requête HTTP : Ollama arrête
                // de générer.
                tokio::select! {
                    () = tour_plan(&shared, &out, &id, params) => {}
                    () = wake.notified() => {
                        out.error(&id, code::CANCELLED, "cancelled", None);
                    }
                }
            });
        }
        "complete.fim" | "completion.measure" => {
            // Annulable (`$/cancelRequest`) : inscrite avant toute attente.
            let wake = Arc::new(tokio::sync::Notify::new());
            let fim = method == "complete.fim";
            {
                let mut running = running.lock();
                // Une mesure en cours : la proposition n'est pas envoyée
                // (elle fausserait la mesure), réponse -32800 comme une
                // proposition remplacée.
                if fim && running.values().any(|pending| pending.measure) {
                    drop(running);
                    out.error(&id, code::CANCELLED, "cancelled", None);
                    return;
                }
                // Une seule proposition en vol : la nouvelle (ou une mesure)
                // annule l'ancienne (réponse -32800).
                for pending in running.values_mut().filter(|pending| pending.fim) {
                    pending.cancelled = true;
                    if let Some(wake) = &pending.wake {
                        wake.notify_one();
                    }
                }
                running.insert(
                    id.to_string(),
                    Pending {
                        stream: None,
                        cancelled: false,
                        wake: Some(Arc::clone(&wake)),
                        fim,
                        measure: !fim,
                    },
                );
            }
            let (shared, out, running) = (Arc::clone(shared), out.clone(), Arc::clone(running));
            tauri::async_runtime::spawn(async move {
                let _registered = Registered {
                    running: &running,
                    key: id.to_string(),
                };
                // Abandonner le futur ferme la requête HTTP : Ollama arrête.
                tokio::select! {
                    () = async {
                        if fim {
                            super::gpu::complete(&shared, &out, &id, params).await;
                        } else {
                            super::gpu::measure(&shared, &out, &id, params).await;
                        }
                    } => {}
                    () = wake.notified() => {
                        out.error(&id, code::CANCELLED, "cancelled", None);
                    }
                    // Une réponse de conversation commence : calcul partagé.
                    () = super::gpu::chat_begins(&shared) => {
                        super::gpu::paused(&out, &id, crate::gpu::plan::Reason::ChatBusy);
                    }
                }
            });
        }
        "gpu.status" => {
            let (shared, out) = (Arc::clone(shared), out.clone());
            tauri::async_runtime::spawn(async move {
                super::gpu::status(&shared, connection, &out, &id, params).await;
            });
        }
        "completion.prepare" | "completion.suggest" | "models.offerPull" => {
            let (shared, out) = (Arc::clone(shared), out.clone());
            let method = method.to_owned();
            tauri::async_runtime::spawn(async move {
                match method.as_str() {
                    "completion.prepare" => super::gpu::prepare(&shared, &out, &id, params).await,
                    "completion.suggest" => super::gpu::suggest(&shared, &out, &id).await,
                    _ => super::gpu::offer_pull(&shared, &out, &id, params).await,
                }
            });
        }
        "voice.mute" | "voice.resume" => {
            let (shared, out) = (Arc::clone(shared), out.clone());
            let resume = method == "voice.resume";
            tauri::async_runtime::spawn(async move {
                if resume {
                    super::voice::voice_resume(&shared, connection, &out, &id, params).await;
                } else {
                    super::voice::voice_mute(&shared, connection, &out, &id, params).await;
                }
            });
        }
        "app.state" | "models.list" | "models.details" | "chat.get" | "chat.list"
        | "chat.forget" | "chat.keep" => {
            let (shared, out) = (Arc::clone(shared), out.clone());
            let method = method.to_owned();
            // Accès agents : seulement ses propres éphémères.
            let owner = session
                .agents
                .then(|| session.client.clone().unwrap_or_default());
            tauri::async_runtime::spawn(async move {
                match method.as_str() {
                    "app.state" => app_state(&shared, &out, &id).await,
                    "models.list" => models_list(&shared, &out, &id).await,
                    "models.details" => models_details(&shared, &out, &id, params).await,
                    "chat.get" => chat_get(&shared, &out, &id, params).await,
                    "chat.list" => chat_list(&shared, &out, &id).await,
                    "chat.forget" => chat_forget(&shared, &out, &id, params, owner).await,
                    _ => chat_keep(&shared, &out, &id, params).await,
                }
            });
        }
        "agents.issue" | "agents.revoke" => {
            let (shared, out) = (Arc::clone(shared), out.clone());
            let editor = session.client.clone().unwrap_or_default();
            let issue = method == "agents.issue";
            tauri::async_runtime::spawn(async move {
                agents_access(&shared, &out, &id, &params, &editor, issue).await;
            });
        }
        _ => out.error(&id, code::METHOD_NOT_FOUND, "method not found", None),
    }
}

fn invalid(out: &Outbox, id: &Value) {
    out.error(id, code::INVALID_PARAMS, "invalid params", None);
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializeParams {
    protocol: (u32, u32),
    client_nonce: String,
}

fn initialize(session: &mut Session, out: &Outbox, id: &Value, params: Value) {
    if session.nonces.is_some() {
        out.error(id, code::INVALID_REQUEST, "already initialized", None);
        return;
    }
    let Ok(params) = serde_json::from_value::<InitializeParams>(params) else {
        return invalid(out, id);
    };
    let (min, max) = params.protocol;
    if max < PROTOCOL.0 || min > PROTOCOL.1 || min > max {
        out.error(
            id,
            code::INCOMPATIBLE,
            "incompatible protocol",
            Some(json!({ "app": [PROTOCOL.0, PROTOCOL.1] })),
        );
        return;
    }
    let Some(client_nonce) =
        proof::from_hex(&params.client_nonce).filter(|nonce| (16..=64).contains(&nonce.len()))
    else {
        return invalid(out, id);
    };
    let server_nonce = proof::random::<32>();
    session.nonces = Some((server_nonce, client_nonce));
    out.result(
        id,
        json!({
            "protocol": max.min(PROTOCOL.1),
            "app": { "version": env!("CARGO_PKG_VERSION"), "platform": platform() },
            "serverNonce": proof::to_hex(&server_nonce),
        }),
    );
}

fn platform() -> &'static str {
    if cfg!(windows) { "windows" } else { "linux" }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairParams {
    code: String,
    #[serde(default)]
    client_name: String,
}

/// Nom d'éditeur gardé : une ligne, 64 caractères au plus.
fn client_name(name: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
    let name = name.trim();
    if name.is_empty() {
        "VS Code".to_owned()
    } else {
        name.to_owned()
    }
}

async fn pair<H: Host>(
    shared: &Arc<Shared<H>>,
    session: &Session,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    if session.nonces.is_none() {
        out.error(id, code::INVALID_REQUEST, "initialize first", None);
        return;
    }
    let Ok(params) = serde_json::from_value::<PairParams>(params) else {
        return invalid(out, id);
    };
    match shared.attempt_pairing(&params.code) {
        Ok(()) => {}
        Err(error) => {
            let (code, message) = match error {
                PairError::NoCode => (code::NO_PAIRING, "no pairing code"),
                PairError::Wrong => (code::WRONG_CODE, "wrong code"),
                PairError::TooMany => (code::TOO_MANY_ATTEMPTS, "too many attempts"),
            };
            out.error(id, code, message, None);
            // Code annulé : l'état montré change.
            if error == PairError::TooMany {
                shared.emit().await;
            }
            return;
        }
    }
    let client = Client {
        id: uuid::Uuid::now_v7().to_string(),
        name: client_name(&params.client_name),
        secret: proof::random::<32>(),
        paired_at: now_ms(),
        last_seen_at: None,
        scope: Scope::Editor,
        parent_id: None,
    };
    let saved = client.clone();
    match shared
        .store
        .call(move |c| saved::add_client(c, &saved))
        .await
    {
        Ok(()) => out.result(
            id,
            json!({ "clientId": client.id, "secret": proof::to_hex(&client.secret) }),
        ),
        Err(error) => out.error(
            id,
            code::REFUSED,
            "refused",
            Some(json!({ "message": error.to_string() })),
        ),
    }
    shared.emit().await;
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthParams {
    client_id: String,
    proof: String,
}

async fn auth<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    session: &mut Session,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Some((server_nonce, client_nonce)) = session.nonces.clone() else {
        out.error(id, code::INVALID_REQUEST, "initialize first", None);
        return;
    };
    if session.client.is_some() {
        out.error(id, code::INVALID_REQUEST, "already authenticated", None);
        return;
    }
    let Ok(params) = serde_json::from_value::<AuthParams>(params) else {
        return invalid(out, id);
    };
    // Lecture de l'éditeur et marque de la connexion d'un seul tenant face à
    // un retrait (`Bridge::unlink`) : retiré avant, l'éditeur est inconnu ;
    // après, la connexion déjà marquée est fermée par lui.
    let _changing = shared.changing.lock().await;
    let client_id = params.client_id.clone();
    let client = match shared
        .store
        .call(move |c| saved::client(c, &client_id))
        .await
    {
        Ok(Some(client)) => client,
        Ok(None) => {
            out.error(id, code::UNKNOWN_CLIENT, "unknown client", None);
            return;
        }
        Err(error) => {
            out.error(
                id,
                code::REFUSED,
                "refused",
                Some(json!({ "message": error.to_string() })),
            );
            return;
        }
    };
    let proven = proof::from_hex(&params.proof).is_some_and(|proof| {
        proof::verify_client(&client.secret, &server_nonce, &client_nonce, &proof)
    });
    if !proven {
        out.error(id, code::UNAUTHENTICATED, "bad proof", None);
        return;
    }
    session.client = Some(client.id.clone());
    session.agents = client.scope == Scope::Agents;
    // Montré « connecté » avant que l'éditeur l'apprenne.
    shared
        .authenticated(connection, &client.id, client.parent_id.as_deref())
        .await;
    let server_proof = proof::server_proof(&client.secret, &client_nonce, &server_nonce);
    out.result(id, json!({ "serverProof": proof::to_hex(&server_proof) }));
}

fn refused(out: &Outbox, id: &Value, error: &crate::error::AppError) {
    out.error(
        id,
        code::REFUSED,
        "refused",
        Some(json!({ "message": error.to_string() })),
    );
}

/// État de l'app, plus `local` : le serveur Ollama visé est sur cette
/// machine (adresse de bouclage ; l'Ollama intégré en est une), et
/// `contextLength` : contexte réglé (0 = défaut du serveur).
async fn app_state<H: Host>(shared: &Arc<Shared<H>>, out: &Outbox, id: &Value) {
    match crate::ollama::target::request_settings(&shared.store).await {
        Ok(settings) => out.result(
            id,
            json!({
                "version": env!("CARGO_PKG_VERSION"),
                "platform": platform(),
                "selectedModel": settings.selected_model,
                "language": crate::i18n::current().code(),
                "local": is_local(&settings.effective_server_url),
                "contextLength": settings.context_length,
            }),
        ),
        Err(error) => refused(out, id, &error),
    }
}

/// Adresse sur cette machine (illisible : non, par prudence).
pub(super) fn is_local(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| crate::ollama::client::is_local(&url))
}

/// Refus d'un travail qui doit rester sur cette machine (accès agents,
/// `proofread.run` `localOnly`) : Ollama sur une autre machine, ou modèle
/// qui tourne ailleurs (nom cloud, ou distant d'après `/api/show`, comme
/// `chatonlm_ask`). `None` : rien ne part ailleurs.
pub(super) async fn elsewhere(server_url: &str, model: &str) -> Option<String> {
    if !is_local(server_url) {
        return Some(t!("bridge.local.remoteServer"));
    }
    if !model.is_empty() && crate::proofread::selection::runs_elsewhere(server_url, model).await {
        return Some(t!("bridge.local.cloudModel", model = model));
    }
    None
}

/// Refus -32020 `{ message, code: "notLocal" }`.
pub(super) fn not_local(out: &Outbox, id: &Value, message: String) {
    out.error(
        id,
        code::REFUSED,
        "refused",
        Some(json!({ "message": message, "code": "notLocal" })),
    );
}

/// `agents.issue {}` → `{ clientId, secret }` (remplace l'accès agents de
/// l'éditeur) ; `agents.revoke {}` → `{ revoked }`. Paramètres non vides →
/// -32602 ; éditeur retiré entre-temps → -32002.
async fn agents_access<H: Host>(
    shared: &Arc<Shared<H>>,
    out: &Outbox,
    id: &Value,
    params: &Value,
    editor: &str,
    issue: bool,
) {
    let empty = params.is_null() || params.as_object().is_some_and(serde_json::Map::is_empty);
    if !empty {
        return invalid(out, id);
    }
    if issue {
        match shared.issue_agents(editor).await {
            Ok(Some((client, secret))) => out.result(
                id,
                json!({ "clientId": client, "secret": proof::to_hex(&secret) }),
            ),
            Ok(None) => out.error(id, code::UNKNOWN_CLIENT, "unknown client", None),
            Err(error) => refused(out, id, &error),
        }
    } else {
        match shared.revoke_agents(editor).await {
            Ok(revoked) => out.result(id, json!({ "revoked": revoked })),
            Err(error) => refused(out, id, &error),
        }
    }
}

async fn models_list<H: Host>(shared: &Arc<Shared<H>>, out: &Outbox, id: &Value) {
    let listed = match crate::ollama::target::request_settings(&shared.store).await {
        Ok(settings) => crate::ollama::models::list(&settings.effective_server_url).await,
        Err(error) => Err(error),
    };
    match listed {
        Ok(models) => out.result(id, json!(models)),
        Err(error) => refused(out, id, &error),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailsParams {
    model: String,
}

/// Détails d'un modèle, comme `model_details` de la page, plus `remote` :
/// le modèle tourne ailleurs (cloud), ce que l'extension dit avant d'envoyer ;
/// et `contextLength` : contexte que le moteur emploiera pour lui (même
/// calcul que l'envoi : réglage borné par le maximum du modèle, sinon celui
/// du modèle chargé ; `null` si inconnu, et pour un modèle du cloud).
async fn models_details<H: Host>(shared: &Arc<Shared<H>>, out: &Outbox, id: &Value, params: Value) {
    let Ok(params) = serde_json::from_value::<DetailsParams>(params) else {
        return invalid(out, id);
    };
    let settings = match crate::ollama::target::request_settings(&shared.store).await {
        Ok(settings) => settings,
        Err(error) => return refused(out, id, &error),
    };
    match crate::commands::models::details(&shared.store, &params.model).await {
        Ok(details) => {
            let server = Server::from_settings(&settings);
            let context = chat::context_size(&server, &params.model, Some(&details)).await;
            let mut result = json!(details);
            result["remote"] = json!(details.remote);
            result["contextLength"] = json!(context);
            out.result(id, result);
        }
        Err(error) => refused(out, id, &error),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatParams {
    chat_id: String,
}

/// Conversation complète, comme `get_chat` de la page : la réponse en cours y
/// est superposée. Inconnue : refus du moteur.
async fn chat_get<H: Host>(shared: &Arc<Shared<H>>, out: &Outbox, id: &Value, params: Value) {
    let Ok(params) = serde_json::from_value::<ChatParams>(params) else {
        return invalid(out, id);
    };
    let chat_id = params.chat_id;
    match shared.store.call(move |c| chats::get(c, &chat_id)).await {
        Ok(mut chat) => {
            shared.host.engine().overlay(&mut chat);
            out.result(id, json!(chat));
        }
        Err(error) => refused(out, id, &error),
    }
}

/// Conversation éphémère quittée par l'éditeur : effacée si elle est
/// éphémère et à lui (sa réponse arrêtée et finie d'abord), sinon rien
/// (`forgotten: false`). Accès agents (`owner`) : seulement une éphémère
/// qu'il a ouverte lui-même.
async fn chat_forget<H: Host>(
    shared: &Arc<Shared<H>>,
    out: &Outbox,
    id: &Value,
    params: Value,
    owner: Option<String>,
) {
    let Ok(params) = serde_json::from_value::<ChatParams>(params) else {
        return invalid(out, id);
    };
    let chat = params.chat_id;
    if let Some(owner) = owner
        && shared.ephemeral_owner(&chat).as_deref() != Some(owner.as_str())
    {
        return out.result(id, json!({ "forgotten": false }));
    }
    let checked = chat.clone();
    let ephemeral = match shared
        .store
        .call(move |c| chats::is_editor_ephemeral(c, &checked))
        .await
    {
        Ok(ephemeral) => ephemeral,
        Err(error) => return refused(out, id, &error),
    };
    if !ephemeral {
        return out.result(id, json!({ "forgotten": false }));
    }
    shared.untrack_ephemeral(&chat);
    shared.host.engine().stop_chat(&chat).await;
    match shared
        .store
        .call(move |c| chats::forget_editor(c, &chat))
        .await
    {
        Ok(forgotten) => out.result(id, json!({ "forgotten": forgotten })),
        Err(error) => refused(out, id, &error),
    }
}

/// « Keep » d'une conversation éphémère de l'éditeur : elle passe dans
/// l'historique (la page l'apprend). Une conversation qui n'est pas à
/// l'éditeur est refusée.
async fn chat_keep<H: Host>(shared: &Arc<Shared<H>>, out: &Outbox, id: &Value, params: Value) {
    let Ok(params) = serde_json::from_value::<ChatParams>(params) else {
        return invalid(out, id);
    };
    let chat = params.chat_id;
    let kept = chat.clone();
    match shared
        .store
        .call(move |c| chats::keep_editor(c, &kept))
        .await
    {
        Ok(()) => {
            shared.untrack_ephemeral(&chat);
            shared.host.chat_updated(&chat);
            out.result(id, json!({}));
        }
        Err(error) => refused(out, id, &error),
    }
}

/// Conversations ouvertes par l'éditeur, récentes d'abord.
async fn chat_list<H: Host>(shared: &Arc<Shared<H>>, out: &Outbox, id: &Value) {
    match shared
        .store
        .call(|c| chats::list_editor(c, LISTED_CHATS))
        .await
    {
        Ok(listed) => out.result(id, json!(listed)),
        Err(error) => refused(out, id, &error),
    }
}

/// Plan d'une visite guidée parlée : `{ segments: [{ lines: [a, b], text }] }`.
async fn tour_plan<H: Host>(shared: &Arc<Shared<H>>, out: &Outbox, id: &Value, params: Value) {
    let Ok(params) = serde_json::from_value::<super::tour::PlanParams>(params) else {
        return invalid(out, id);
    };
    let settings = match crate::ollama::target::request_settings(&shared.store).await {
        Ok(settings) => settings,
        Err(error) => return refused(out, id, &error),
    };
    let model = params
        .model
        .clone()
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| settings.selected_model.clone());
    let server = Server::from_settings(&settings);
    // L'autocomplétion attend pendant ce temps (calcul partagé).
    let _working = shared.host.engine().hold();
    match super::tour::plan(&server, &model, &params).await {
        Ok(segments) => {
            let segments: Vec<Value> = segments
                .into_iter()
                .map(|segment| json!({ "lines": [segment.lines.0, segment.lines.1], "text": segment.text }))
                .collect();
            out.result(id, json!({ "segments": segments }));
        }
        Err(super::tour::PlanError::Params) => invalid(out, id),
        Err(super::tour::PlanError::Refused(message)) => out.error(
            id,
            code::REFUSED,
            "refused",
            Some(json!({ "message": message })),
        ),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SendParams {
    content: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    chat_id: Option<String>,
    #[serde(default)]
    ephemeral: Option<bool>,
    #[serde(default)]
    think: Option<Think>,
}

/// Qui envoie : connexion, client authentifié (éditeur ou accès agents),
/// flux du moteur.
struct Sender {
    connection: u64,
    client: String,
    /// Accès agents : nouvelle conversation éphémère seulement, sur cette
    /// machine.
    agents: bool,
    stream: String,
}

async fn chat_send<H: Host>(
    shared: &Arc<Shared<H>>,
    sender: Sender,
    running: &Running,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let registered = Registered {
        running,
        key: id.to_string(),
    };
    let Ok(params) = serde_json::from_value::<SendParams>(params) else {
        return invalid(out, id);
    };
    // Accès agents : une nouvelle conversation éphémère, jamais l'historique.
    if sender.agents && (params.chat_id.is_some() || params.ephemeral != Some(true)) {
        return out.error(id, code::FORBIDDEN, "not allowed", None);
    }
    let settings = match crate::ollama::target::request_settings(&shared.store).await {
        Ok(settings) => settings,
        Err(error) => return refused(out, id, &error),
    };
    let model = params
        .model
        .map(|model| model.trim().to_owned())
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| settings.selected_model.clone());
    // Accès agents : rien ne quitte la machine (Ollama distant, modèle cloud).
    if sender.agents
        && let Some(message) = elsewhere(&settings.effective_server_url, &model).await
    {
        return not_local(out, id, message);
    }
    let think = match params.think {
        Some(think) => Some(think),
        None => {
            let model = model.clone();
            shared
                .store
                .call(move |c| model_prefs::think(c, &model))
                .await
                .ok()
                .flatten()
        }
    };
    // Arrêt demandé pendant la préparation (démarrage de l'Ollama intégré
    // attendu par `request_settings`) : rien n'est envoyé ni écrit.
    if registered.cancelled() {
        return out.result(
            id,
            json!({ "chatId": params.chat_id, "outcome": "cancelled" }),
        );
    }
    let ephemeral = params.chat_id.is_none() && params.ephemeral == Some(true);
    let request = SendRequest {
        chat_id: params.chat_id.clone(),
        stream_id: sender.stream.clone(),
        content: params.content,
        model,
        think,
        attachment_ids: None,
        edit_message_id: None,
        kept_attachment_ids: None,
        web_search: None,
        voice: None,
        image: None,
        ephemeral: params.ephemeral,
    };
    // Les évènements passent par un canal : une conversation créée est
    // marquée dans la base (écriture attendue, hors du flux du moteur) avant
    // que la page l'apprenne.
    let (events, mut received) = mpsc::unbounded_channel::<ChatEvent>();
    let sending = async move {
        let sink = move |event: ChatEvent| {
            let _ = events.send(event);
        };
        chat::send_to(
            &shared.store,
            shared.host.engine(),
            &shared.staging,
            Server::from_settings(&settings),
            request,
            Origin::Bridge,
            &sink,
        )
        .await
    };
    let relaying = async {
        let mut chat_id = params.chat_id;
        let mut outcome = "done";
        // Conversation éphémère dont l'éditeur est déjà parti : effacée à la
        // fin de l'envoi, arrêté.
        let mut erase = false;
        while let Some(event) = received.recv().await {
            match &event {
                ChatEvent::ChatCreated { chat_id: created } => {
                    let marked = created.clone();
                    if let Err(error) = shared
                        .store
                        .call(move |c| chats::set_editor(c, &marked))
                        .await
                    {
                        eprintln!("chatonlm: cannot mark an editor chat: {error}");
                    }
                    if ephemeral
                        && !shared.track_ephemeral(sender.connection, &sender.client, created)
                    {
                        erase = true;
                        registered.cancel();
                    }
                    chat_id = Some(created.clone());
                    shared.host.chat_updated(created);
                }
                ChatEvent::UserMessage { .. } | ChatEvent::Done { .. } => {
                    if let Some(chat) = chat_id.as_deref() {
                        shared.host.chat_updated(chat);
                    }
                }
                ChatEvent::Error { .. } => outcome = "error",
                ChatEvent::Cancelled => outcome = "cancelled",
                _ => {}
            }
            // Tout évènement autre que `chatCreated` suit `begin` : un arrêt
            // demandé avant (sans effet sur le moteur alors) est rejoué.
            if !matches!(event, ChatEvent::ChatCreated { .. }) && registered.cancelled() {
                shared.host.engine().cancel(&sender.stream);
            }
            out.notify("chat.event", json!({ "requestId": id, "event": event }));
        }
        (chat_id, outcome, erase)
    };
    let (sent, (chat_id, outcome, erase)) = tokio::join!(sending, relaying);
    drop(registered);
    if erase && let Some(chat) = chat_id.clone() {
        shared.erase_editor_chats(vec![chat]).await;
    }
    match sent {
        Ok(()) => {
            if let Some(chat) = &chat_id {
                shared.host.chat_updated(chat);
            }
            out.result(id, json!({ "chatId": chat_id, "outcome": outcome }));
        }
        Err(error) => refused(out, id, &error),
    }
}

#[cfg(test)]
mod tests {
    use super::client_name;

    #[test]
    fn a_client_name_is_one_short_line() {
        assert_eq!(client_name("  VSCodium "), "VSCodium");
        assert_eq!(client_name("a\nb\u{7}c"), "abc");
        assert_eq!(client_name(""), "VS Code");
        assert_eq!(client_name(&"x".repeat(100)).chars().count(), 64);
    }
}
