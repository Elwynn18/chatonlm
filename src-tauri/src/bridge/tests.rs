//! Essais du pont : service réel, base en mémoire, faux Ollama, connexions
//! par des flux en mémoire (les points d'accès du système ont leurs essais
//! dans `linux.rs` et `windows.rs`).

use std::sync::{Arc, OnceLock};

use fake_ollama::{FakeOllama, Mode};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::io::{AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::sync::watch;

use super::connection::code;
use super::voice::{Voice, voice_code};
use super::*;
use crate::dictation::clipboard::BoxFuture;
use crate::store::chats::{self, ChatSource};
use crate::store::settings::{self, SettingsPatch};
use crate::tts::service::{ReadingEnd, Segment, SegmentEvent, SegmentSink, SegmentsRefusal};
use crate::tts::{ReadAloudError, VoiceLanguage};
use crate::voice_mode::session::{EditorLink, StartError};
use crate::voice_mode::{VoiceModePhase, VoiceModeState, VoiceModeUtterance};

/// Lecture demandée : identifiant, segments, langue.
type Read = (String, Vec<Segment>, Option<VoiceLanguage>);

/// Voix de l'app simulée : ce que le pont lui demande.
#[derive(Default)]
struct FakeVoice {
    reads: Mutex<Vec<Read>>,
    sinks: Mutex<Vec<SegmentSink>>,
    read_refusal: Mutex<Option<SegmentsRefusal>>,
    stopped_reads: Mutex<Vec<String>>,
    /// Connexions dont les lectures ont été arrêtées.
    reads_stopped_for: Mutex<Vec<u64>>,
    /// Séances tenues par l'éditeur.
    links: Mutex<Vec<Arc<dyn EditorLink>>>,
    start_refusal: Mutex<Option<StartError>>,
    /// États publiés à l'éditeur par le démarrage, avant sa fin (comme la
    /// séance : `starting`).
    start_states: Mutex<Vec<VoiceModeState>>,
    /// Démarrage retenu jusqu'au feu vert (arrêt pendant le démarrage).
    start_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    /// `stop(connection)` reçus.
    stops: Mutex<Vec<u64>>,
    mutes: Mutex<Vec<(u64, bool)>>,
    resumes: Mutex<Vec<u64>>,
    resume_refusal: Mutex<Option<String>>,
    failed_replies: Mutex<Vec<(u64, u32, String)>>,
    /// La lecture panique (panique dans la tâche d'une connexion).
    panic_on_read: std::sync::atomic::AtomicBool,
}

impl Voice for FakeVoice {
    fn read(
        &self,
        read_id: String,
        segments: Vec<Segment>,
        language: Option<VoiceLanguage>,
        sink: SegmentSink,
    ) -> Result<(), SegmentsRefusal> {
        assert!(
            !self.panic_on_read.load(Ordering::SeqCst),
            "the reading panics"
        );
        if let Some(refusal) = *self.read_refusal.lock() {
            return Err(refusal);
        }
        self.reads.lock().push((read_id, segments, language));
        self.sinks.lock().push(sink);
        Ok(())
    }

    fn stop_read(&self, read_id: &str) {
        self.stopped_reads.lock().push(read_id.to_owned());
    }

    fn stop_reads(&self, connection: u64) {
        self.reads_stopped_for.lock().push(connection);
    }

    fn start(&self, link: Arc<dyn EditorLink>) -> BoxFuture<Result<(), StartError>> {
        let refusal = self.start_refusal.lock().clone();
        for state in self.start_states.lock().iter() {
            link.changed(state);
        }
        if refusal.is_none() {
            self.links.lock().push(link);
        }
        let gate = self.start_gate.lock().clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.notified().await;
            }
            refusal.map_or(Ok(()), Err)
        })
    }

    /// Comme la séance : l'éditeur qui la tenait apprend `Off`.
    fn stop(&self, connection: u64) {
        self.stops.lock().push(connection);
        let links: Vec<_> = self.links.lock().clone();
        for link in links.iter().filter(|link| link.connection() == connection) {
            link.changed(&VoiceModeState::default());
        }
    }

    fn mute(&self, connection: u64, muted: bool) -> BoxFuture<Result<(), String>> {
        self.mutes.lock().push((connection, muted));
        Box::pin(async { Ok(()) })
    }

    fn resume(&self, connection: u64) -> BoxFuture<Result<(), String>> {
        self.resumes.lock().push(connection);
        let refusal = self.resume_refusal.lock().clone();
        Box::pin(async move { refusal.map_or(Ok(()), Err) })
    }

    fn reply_failed(&self, connection: u64, turn: u32, message: String) {
        self.failed_replies.lock().push((connection, turn, message));
    }
}

/// Conversation annoncée à la page, et sa marque à ce moment-là.
type Mark = (String, Option<ChatSource>);

#[derive(Clone, Default)]
struct TestHost {
    engine: Arc<ChatEngine>,
    states: Arc<Mutex<Vec<BridgeState>>>,
    updated: Arc<Mutex<Vec<String>>>,
    /// Base lue à chaque conversation annoncée à la page.
    store: Arc<OnceLock<Store>>,
    /// Marque de la conversation au moment où la page l'apprend.
    marks: Arc<Mutex<Vec<Mark>>>,
    voice: Arc<FakeVoice>,
    gpu: Arc<crate::gpu::GpuChief>,
    /// Machine du serveur rendue au chef (aucune par défaut).
    machine: Arc<Mutex<Option<crate::hub::hardware::Machine>>>,
    /// Modèles proposés au téléchargement par l'éditeur.
    offers: Arc<Mutex<Vec<String>>>,
    /// Easy Link simulé (lot 4a) ; absent : service indisponible.
    notifier: Arc<Mutex<Option<Arc<FakeNotifier>>>>,
    study: Arc<crate::study::StudyEngine>,
    /// Cours annoncés à la page.
    studied: Arc<Mutex<Vec<String>>>,
    /// Fenêtre montrée (`app.show`), avec la page demandée.
    shown: Arc<Mutex<Vec<Option<String>>>>,
}

/// Easy Link simulé : ce que le pont lui demande, et sa réponse.
#[derive(Default)]
struct FakeNotifier {
    notes: Mutex<Vec<crate::easy_link::notify::Note>>,
    tasks: Mutex<Vec<(u64, crate::easy_link::notify::TaskUpdate)>>,
    ended: Mutex<Vec<u64>>,
    refusal: Mutex<Option<crate::easy_link::notify::NotifyError>>,
}

impl crate::easy_link::notify::Notifier for FakeNotifier {
    fn notify(
        &self,
        note: crate::easy_link::notify::Note,
    ) -> BoxFuture<Result<u32, crate::easy_link::notify::NotifyError>> {
        self.notes.lock().push(note);
        let refusal = self.refusal.lock().clone();
        Box::pin(async move { refusal.map_or(Ok(1), Err) })
    }

    fn task(
        &self,
        connection: u64,
        update: crate::easy_link::notify::TaskUpdate,
    ) -> BoxFuture<Result<(), crate::easy_link::notify::NotifyError>> {
        self.tasks.lock().push((connection, update));
        let refusal = self.refusal.lock().clone();
        Box::pin(async move { refusal.map_or(Ok(()), Err) })
    }

    fn connection_ended(&self, connection: u64) {
        self.ended.lock().push(connection);
    }
}

impl Host for TestHost {
    fn engine(&self) -> &ChatEngine {
        &self.engine
    }
    fn changed(&self, state: &BridgeState) {
        self.states.lock().push(state.clone());
    }
    fn chat_updated(&self, chat_id: &str) {
        self.updated.lock().push(chat_id.to_owned());
        if let Some(store) = self.store.get() {
            let source = store
                .blocking(|c| chats::list(c))
                .unwrap()
                .into_iter()
                .find(|chat| chat.id == chat_id)
                .and_then(|chat| chat.source);
            self.marks.lock().push((chat_id.to_owned(), source));
        }
    }
    fn voice(&self) -> &dyn Voice {
        &*self.voice
    }
    fn gpu(&self) -> &crate::gpu::GpuChief {
        &self.gpu
    }
    fn machine(&self, _server_url: String) -> BoxFuture<Option<crate::hub::hardware::Machine>> {
        let machine = self.machine.lock().clone();
        Box::pin(async move { machine })
    }
    fn notifier(&self) -> Option<Arc<dyn crate::easy_link::notify::Notifier>> {
        self.notifier
            .lock()
            .clone()
            .map(|notifier| notifier as Arc<dyn crate::easy_link::notify::Notifier>)
    }
    fn study(&self) -> Option<&crate::study::StudyEngine> {
        Some(&self.study)
    }
    fn study_updated(&self, updated: &crate::study::StudyUpdated) {
        self.studied.lock().push(updated.course_id.clone());
    }
    fn show(&self, route: Option<&str>) {
        self.shown.lock().push(route.map(str::to_owned));
    }
    fn offer_pull(&self, model: &str, commercial_use: bool) {
        self.offers.lock().push(if commercial_use {
            model.to_owned()
        } else {
            format!("{model} (non-commercial)")
        });
    }
}

struct Bench {
    bridge: Bridge<TestHost>,
    host: TestHost,
    store: Store,
    dir: tempfile::TempDir,
    _ollama: FakeOllama,
}

/// Banc au délai d'authentification réel : les essais qui enchaînent des
/// requêtes avant de s'authentifier ne dépendent pas de la charge.
async fn bench() -> Bench {
    bench_with(connection::AUTH_TIMEOUT).await
}

async fn bench_with(auth_timeout: std::time::Duration) -> Bench {
    let ollama = FakeOllama::start(Mode::ollama()).await.unwrap();
    let store = Store::open_in_memory().unwrap();
    let patch = SettingsPatch {
        server_url: Some(ollama.url()),
        selected_model: Some("qwen3:0.6b".to_owned()),
        ..Default::default()
    };
    store
        .blocking(|c| settings::update(c, patch, None).map(drop))
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        dir: Some(dir.path().join("run")),
        data_dir: dir.path().join("data"),
        launcher: Some(PathBuf::from("/opt/chatonlm/chatonlm")),
        app_version: "9.9.9".to_owned(),
        auth_timeout,
        close_delay: std::time::Duration::ZERO,
    };
    let host = TestHost::default();
    host.store.set(store.clone()).ok();
    let bridge = Bridge::new(host.clone(), store.clone(), config);
    Bench {
        bridge,
        host,
        store,
        dir,
        _ollama: ollama,
    }
}

/// Client de test : JSON-RPC sur un flux en mémoire servi par `connection`.
struct Client {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    next: i64,
    notifications: Vec<Value>,
    nonce: Vec<u8>,
    server_nonce: Vec<u8>,
}

impl Bench {
    fn connect(&self) -> Client {
        let (client, server) = tokio::io::duplex(1 << 20);
        let shared = Arc::clone(self.bridge.shared());
        let (close, closed) = watch::channel(false);
        let id = shared.next_connection.fetch_add(1, Ordering::SeqCst);
        shared.connections.lock().insert(id, Open::new(close));
        tokio::spawn(async move {
            connection::serve(&shared, id, Box::new(server), closed).await;
            shared.connections.lock().remove(&id);
        });
        let (reader, writer) = tokio::io::split(client);
        Client {
            reader: BufReader::new(reader),
            writer,
            next: 1,
            notifications: Vec::new(),
            nonce: vec![9; 32],
            server_nonce: Vec::new(),
        }
    }

    async fn state(&self) -> BridgeState {
        self.bridge.state().await.unwrap()
    }

    /// Pont activé, code d'appairage demandé.
    async fn code(&self) -> String {
        if !self.state().await.enabled {
            self.bridge.set_enabled(true).await.unwrap();
        }
        self.bridge.pair().await.unwrap().pairing.unwrap().code
    }

    /// Éditeur appairé et authentifié.
    async fn paired(&self) -> (Client, String, [u8; 32]) {
        let code = self.code().await;
        let mut client = self.connect();
        client.initialize().await;
        let paired = client
            .request("pair", json!({ "code": code, "clientName": "VS Code" }))
            .await;
        let id = paired["result"]["clientId"].as_str().unwrap().to_owned();
        let secret: [u8; 32] = proof::from_hex(paired["result"]["secret"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        // Nouvelle connexion, comme l'extension après l'appairage.
        let mut client = self.connect();
        client.initialize().await;
        let authed = client.auth(&id, &secret).await;
        assert!(authed.get("result").is_some(), "{authed}");
        (client, id, secret)
    }
}

impl Client {
    async fn send(&mut self, value: &Value) {
        self.writer
            .write_all(&frame::encode(value.to_string().as_bytes()))
            .await
            .unwrap();
    }

    /// Message suivant ; `None` : connexion fermée.
    async fn next_message(&mut self) -> Option<Value> {
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            frame::read(&mut self.reader),
        )
        .await
        .expect("bridge answered in time");
        match read {
            Ok(Some(body)) => Some(serde_json::from_slice(&body).unwrap()),
            _ => None,
        }
    }

    /// Prochaine notification `method` (gardée ou à venir), retirée.
    async fn notification(&mut self, method: &str) -> Value {
        loop {
            if let Some(index) = self
                .notifications
                .iter()
                .position(|message| message["method"] == method)
            {
                return self.notifications.remove(index)["params"].clone();
            }
            let message = self.next_message().await.expect("connection open");
            self.notifications.push(message);
        }
    }

    /// Envoie une requête et rend sa réponse ; les notifications reçues en
    /// chemin sont gardées.
    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        self.response(id).await
    }

    async fn response(&mut self, id: i64) -> Value {
        // Réponse déjà arrivée pendant l'attente d'une autre.
        if let Some(index) = self
            .notifications
            .iter()
            .position(|message| message["id"] == id && message.get("method").is_none())
        {
            return self.notifications.remove(index);
        }
        loop {
            let message = self.next_message().await.expect("connection open");
            if message["id"] == id {
                return message;
            }
            self.notifications.push(message);
        }
    }

    async fn initialize(&mut self) -> Value {
        let nonce = proof::to_hex(&self.nonce);
        let answer = self
            .request(
                "initialize",
                json!({ "protocol": [1, 1], "client": { "name": "test" }, "clientNonce": nonce }),
            )
            .await;
        if let Some(nonce) = answer["result"]["serverNonce"].as_str() {
            self.server_nonce = proof::from_hex(nonce).unwrap();
        }
        answer
    }

    async fn auth(&mut self, id: &str, secret: &[u8; 32]) -> Value {
        let proof = proof::client_proof(secret, &self.server_nonce, &self.nonce);
        let answer = self
            .request(
                "auth",
                json!({ "clientId": id, "proof": proof::to_hex(&proof) }),
            )
            .await;
        if let Some(server) = answer["result"]["serverProof"].as_str() {
            assert_eq!(
                proof::from_hex(server).unwrap(),
                proof::server_proof(secret, &self.nonce, &self.server_nonce),
                "the app proves itself"
            );
        }
        answer
    }
}

fn error_code(answer: &Value) -> i64 {
    answer["error"]["code"]
        .as_i64()
        .unwrap_or_else(|| panic!("no error: {answer}"))
}

#[tokio::test]
async fn an_editor_pairs_proves_itself_and_chats() {
    let bench = bench().await;
    let (mut client, id, _) = bench.paired().await;

    let state = bench.state().await;
    assert_eq!(state.status, BridgeStatus::Listening);
    assert_eq!(state.pairing, None, "the code is used once");
    assert_eq!(state.clients.len(), 1);
    assert_eq!(state.clients[0].id, id);
    assert_eq!(state.clients[0].name, "VS Code");
    assert!(state.clients[0].connected);
    assert!(state.clients[0].last_seen_at.is_some());
    assert!(bench.host.states.lock().last().unwrap().clients[0].connected);

    assert_eq!(client.request("ping", json!({})).await["result"], json!({}));
    let app = client.request("app.state", json!({})).await;
    assert_eq!(app["result"]["selectedModel"], "qwen3:0.6b");
    let models = client.request("models.list", json!({})).await;
    assert!(models["result"].is_array(), "{models}");

    let sent = client
        .request("chat.send", json!({ "content": "Hello" }))
        .await;
    assert_eq!(sent["result"]["outcome"], "done", "{sent}");
    let chat_id = sent["result"]["chatId"].as_str().unwrap().to_owned();
    let events: Vec<&str> = client
        .notifications
        .iter()
        .filter(|n| n["method"] == "chat.event")
        .map(|n| n["params"]["event"]["type"].as_str().unwrap())
        .collect();
    assert_eq!(events.first(), Some(&"chatCreated"));
    assert!(events.contains(&"content"), "{events:?}");
    assert_eq!(events.last(), Some(&"done"));
    assert!(bench.host.updated.lock().contains(&chat_id));
    let saved = bench
        .store
        .call(move |c| crate::store::chats::get(c, &chat_id))
        .await
        .unwrap();
    assert_eq!(saved.messages.len(), 2);
    assert_eq!(saved.messages[0].content, "Hello");
}

#[tokio::test]
async fn nothing_but_the_handshake_is_served_before_authentication() {
    let bench = bench().await;
    let mut client = bench.connect();
    assert_eq!(
        error_code(&client.request("ping", json!({})).await),
        code::UNAUTHENTICATED
    );
    assert_eq!(
        error_code(
            &client
                .request("chat.send", json!({ "content": "hi" }))
                .await
        ),
        code::UNAUTHENTICATED
    );
    // Appairage et preuve exigent `initialize` d'abord.
    assert_eq!(
        error_code(&client.request("pair", json!({ "code": "123456" })).await),
        code::INVALID_REQUEST
    );
    assert!(client.initialize().await.get("result").is_some());
    assert_eq!(
        error_code(&client.initialize().await),
        code::INVALID_REQUEST
    );
    assert!(
        bench
            .store
            .call(|c| crate::store::chats::list(c))
            .await
            .unwrap()
            .is_empty(),
        "nothing was sent"
    );
}

#[tokio::test]
async fn unknown_methods_are_refused_after_authentication() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    for method in [
        "settings.update",
        "models.delete",
        "models.pull",
        "chat.delete",
    ] {
        assert_eq!(
            error_code(&client.request(method, json!({})).await),
            code::METHOD_NOT_FOUND,
            "{method}"
        );
    }
}

#[tokio::test]
async fn pairing_needs_the_right_code_and_ten_wrong_codes_cancel_it() {
    let bench = bench().await;
    let mut client = bench.connect();
    client.initialize().await;
    // Aucun code demandé dans l'app.
    assert_eq!(
        error_code(&client.request("pair", json!({ "code": "123456" })).await),
        code::NO_PAIRING
    );
    let code = bench.code().await;
    let wrong = if code == "000000" { "000001" } else { "000000" };
    for _ in 0..9 {
        assert_eq!(
            error_code(&client.request("pair", json!({ "code": wrong })).await),
            code::WRONG_CODE
        );
    }
    assert_eq!(
        error_code(&client.request("pair", json!({ "code": wrong })).await),
        code::TOO_MANY_ATTEMPTS
    );
    assert_eq!(
        error_code(&client.request("pair", json!({ "code": code })).await),
        code::NO_PAIRING
    );
    assert!(bench.state().await.clients.is_empty());
    assert_eq!(bench.host.states.lock().last().unwrap().pairing, None);
}

#[tokio::test]
async fn a_wrong_proof_or_an_unknown_editor_is_refused() {
    let bench = bench().await;
    let (_, id, secret) = bench.paired().await;
    let mut client = bench.connect();
    client.initialize().await;
    assert_eq!(
        error_code(&client.auth(&id, &[0; 32]).await),
        code::UNAUTHENTICATED
    );
    assert_eq!(
        error_code(&client.request("ping", json!({})).await),
        code::UNAUTHENTICATED
    );
    assert_eq!(
        error_code(&client.auth("someone", &secret).await),
        code::UNKNOWN_CLIENT
    );
    // Preuve rejouée d'une autre connexion (autres nonces) : refusée.
    let mut other = bench.connect();
    other.initialize().await;
    let stale = proof::client_proof(&secret, &[1; 32], &client.nonce);
    let answer = other
        .request(
            "auth",
            json!({ "clientId": id, "proof": proof::to_hex(&stale) }),
        )
        .await;
    assert_eq!(error_code(&answer), code::UNAUTHENTICATED);
}

#[tokio::test]
async fn an_incompatible_protocol_is_told() {
    let bench = bench().await;
    let mut client = bench.connect();
    let answer = client
        .request(
            "initialize",
            json!({ "protocol": [2, 3], "clientNonce": "00".repeat(32) }),
        )
        .await;
    assert_eq!(error_code(&answer), code::INCOMPATIBLE);
    assert_eq!(answer["error"]["data"]["app"], json!([1, 1]));
}

#[tokio::test]
async fn a_connection_that_never_authenticates_is_closed() {
    // Délai court, mais assez long pour s'appairer sous charge.
    let bench = bench_with(std::time::Duration::from_secs(1)).await;
    let mut client = bench.connect();
    client.initialize().await;
    let started = std::time::Instant::now();
    assert_eq!(client.next_message().await, None);
    assert!(started.elapsed() >= std::time::Duration::from_millis(900));
    // Authentifiée, elle reste ouverte au-delà du délai.
    let (mut client, _, _) = bench.paired().await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    assert_eq!(client.request("ping", json!({})).await["result"], json!({}));
}

#[tokio::test]
async fn removing_an_editor_closes_its_connections_and_forgets_it() {
    let bench = bench().await;
    let (mut client, id, secret) = bench.paired().await;
    let state = bench.bridge.unlink(id.clone()).await.unwrap();
    assert!(state.clients.is_empty());
    assert_eq!(client.next_message().await, None, "connection closed");
    let mut again = bench.connect();
    again.initialize().await;
    assert_eq!(
        error_code(&again.auth(&id, &secret).await),
        code::UNKNOWN_CLIENT
    );
}

#[tokio::test]
async fn switching_off_closes_everything_and_removes_the_discovery_file() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let run = bench.dir.path().join("run");
    let discovery: Value =
        serde_json::from_slice(&std::fs::read(run.join(DISCOVERY)).unwrap()).unwrap();
    assert_eq!(discovery["protocol"], json!([1, 1]));
    assert_eq!(discovery["app"], "9.9.9");
    assert_eq!(discovery["pid"], std::process::id());
    assert!(
        discovery["endpoint"]
            .as_str()
            .is_some_and(|e| !e.is_empty())
    );
    let launcher: Value = serde_json::from_slice(
        &std::fs::read(bench.dir.path().join("data").join(LAUNCHER)).unwrap(),
    )
    .unwrap();
    assert_eq!(launcher["path"], "/opt/chatonlm/chatonlm");

    let state = bench.bridge.set_enabled(false).await.unwrap();
    assert_eq!(state.status, BridgeStatus::Off);
    assert!(!state.enabled);
    assert_eq!(client.next_message().await, None);
    assert!(!run.join(DISCOVERY).exists());
    // Coupé : pas de code d'appairage.
    assert!(bench.bridge.pair().await.is_err());
    // Rallumé : l'éditeur appairé reste connu.
    let state = bench.bridge.set_enabled(true).await.unwrap();
    assert_eq!(state.status, BridgeStatus::Listening);
    assert_eq!(state.clients.len(), 1);
    assert!(!state.clients[0].connected);
    bench.bridge.shutdown();
    assert!(!run.join(DISCOVERY).exists());
}

#[tokio::test]
async fn the_bridge_is_unavailable_without_a_runtime_dir() {
    let mut bench = bench().await;
    let mut config = bench.bridge.shared().config.clone();
    config.dir = None;
    bench.bridge = Bridge::new(bench.host.clone(), bench.store.clone(), config);
    let state = bench.bridge.set_enabled(true).await.unwrap();
    assert_eq!(state.status, BridgeStatus::Unavailable);
    assert!(state.enabled);
}

#[tokio::test]
async fn a_saved_switch_opens_the_bridge_at_start() {
    let bench = bench().await;
    bench
        .store
        .blocking(|c| saved::set_enabled(c, true))
        .unwrap();
    bench.bridge.start_saved().await;
    assert_eq!(bench.state().await.status, BridgeStatus::Listening);
    bench.bridge.shutdown();
}

#[tokio::test]
async fn a_send_is_cancelled_by_cancel_request() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let id = client.next;
    client.next += 1;
    client
        .send(&json!({
            "jsonrpc": "2.0", "id": id, "method": "chat.send",
            "params": { "content": "Wait", "model": "chatonlm-hang" }
        }))
        .await;
    // La réponse a commencé.
    loop {
        let message = client.next_message().await.unwrap();
        if message["params"]["event"]["type"] == "userMessage" {
            break;
        }
    }
    client
        .send(&json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": id } }))
        .await;
    let answer = client.response(id).await;
    assert_eq!(answer["result"]["outcome"], "cancelled", "{answer}");
}

#[tokio::test]
async fn a_closed_connection_does_not_stop_a_send() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    client
        .send(&json!({
            "jsonrpc": "2.0", "id": 99, "method": "chat.send",
            "params": { "content": "Hello" }
        }))
        .await;
    loop {
        let message = client.next_message().await.unwrap();
        if message["params"]["event"]["type"] == "userMessage" {
            break;
        }
    }
    drop(client);
    // La réponse finit et s'enregistre quand même.
    let store = bench.store.clone();
    let done = tokio::time::timeout(std::time::Duration::from_secs(10), async move {
        loop {
            let chats = store.call(|c| crate::store::chats::list(c)).await.unwrap();
            if let Some(chat) = chats.first() {
                let id = chat.id.clone();
                let chat = store
                    .call(move |c| crate::store::chats::get(c, &id))
                    .await
                    .unwrap();
                if chat.messages.len() == 2 && !chat.messages[1].content.is_empty() {
                    return;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(done.is_ok(), "the reply was saved");
}

#[tokio::test]
async fn a_chat_opened_by_the_editor_is_marked_before_the_page_learns_of_it() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let sent = client
        .request("chat.send", json!({ "content": "Hello" }))
        .await;
    let chat_id = sent["result"]["chatId"].as_str().unwrap().to_owned();
    let marks = bench.host.marks.lock().clone();
    assert!(!marks.is_empty());
    assert!(
        marks
            .iter()
            .all(|mark| *mark == (chat_id.clone(), Some(ChatSource::Vscode))),
        "{marks:?}"
    );
    // Une conversation de l'app, continuée par l'éditeur, garde sa marque
    // (aucune).
    let own = bench
        .store
        .call(|c| chats::create_with(c, 1, false))
        .await
        .unwrap();
    let continued = client
        .request("chat.send", json!({ "content": "Again", "chatId": own }))
        .await;
    assert_eq!(continued["result"]["outcome"], "done", "{continued}");
    let listed = bench.store.call(|c| chats::list(c)).await.unwrap();
    let source = |id: &str| listed.iter().find(|chat| chat.id == id).unwrap().source;
    assert_eq!(source(&chat_id), Some(ChatSource::Vscode));
    assert_eq!(source(&own), None);
}

#[tokio::test]
async fn chat_get_reads_a_chat_with_the_reply_in_progress() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let id = client.next;
    client.next += 1;
    client
        .send(&json!({
            "jsonrpc": "2.0", "id": id, "method": "chat.send",
            "params": { "content": "Wait", "model": "chatonlm-stall" }
        }))
        .await;
    // La réponse a commencé à s'écrire.
    let mut chat_id = String::new();
    loop {
        let message = client.next_message().await.unwrap();
        let event = &message["params"]["event"];
        if event["type"] == "chatCreated" {
            chat_id = event["chatId"].as_str().unwrap().to_owned();
        }
        if event["type"] == "content" {
            break;
        }
    }
    let got = client
        .request("chat.get", json!({ "chatId": chat_id }))
        .await;
    let chat = &got["result"];
    assert_eq!(chat["id"], chat_id.as_str(), "{got}");
    assert_eq!(chat["messages"][0]["content"], "Wait");
    let reply = &chat["messages"][1];
    assert!(
        reply["content"].as_str().is_some_and(|c| !c.is_empty()),
        "the live reply is overlaid: {got}"
    );
    assert_eq!(chat["replyRunning"], reply["id"]);
    client
        .send(&json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": id } }))
        .await;
    assert_eq!(client.response(id).await["result"]["outcome"], "cancelled");

    // Conversation inconnue : refus du moteur, avec son message ; paramètres
    // manquants : refusés.
    let unknown = client
        .request("chat.get", json!({ "chatId": "nowhere" }))
        .await;
    assert_eq!(error_code(&unknown), code::REFUSED);
    assert!(unknown["error"]["data"]["message"].is_string(), "{unknown}");
    assert_eq!(
        error_code(&client.request("chat.get", json!({})).await),
        code::INVALID_PARAMS
    );
}

#[tokio::test]
async fn chat_list_gives_the_editor_chats_recent_first_and_fifty_at_most() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    assert_eq!(
        client.request("chat.list", json!({})).await["result"],
        json!([])
    );
    let ids = bench
        .store
        .call(|c| {
            let mut ids = Vec::new();
            for at in 0..55 {
                let id = chats::create_with(c, 1_000 + at, false)?;
                chats::set_editor(c, &id)?;
                ids.push(id);
            }
            // Ni une conversation de l'app, ni une éphémère de l'éditeur, ni
            // celle d'un bot.
            chats::create_with(c, 5_000, false)?;
            let ephemeral = chats::create_with(c, 5_001, true)?;
            chats::set_editor(c, &ephemeral)?;
            let bot = chats::create_with(c, 5_002, false)?;
            chats::set_source(c, &bot, crate::store::easy_link::Platform::Telegram)?;
            Ok(ids)
        })
        .await
        .unwrap();
    let listed = client.request("chat.list", json!({})).await;
    let listed = listed["result"].as_array().unwrap();
    assert_eq!(listed.len(), 50);
    let expected: Vec<&str> = ids.iter().rev().take(50).map(String::as_str).collect();
    let got: Vec<&str> = listed.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(got, expected);
    assert!(listed.iter().all(|chat| chat["source"] == "vscode"));
}

#[tokio::test]
async fn models_details_tell_whether_the_model_runs_elsewhere() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let local = client
        .request("models.details", json!({ "model": "qwen3:0.6b" }))
        .await;
    assert_eq!(local["result"]["model"], "qwen3:0.6b", "{local}");
    assert!(local["result"]["capabilities"].is_array());
    assert_eq!(local["result"]["remote"], false);
    for model in ["gpt-oss:20b-cloud", "chatonlm-polish-remote"] {
        let remote = client
            .request("models.details", json!({ "model": model }))
            .await;
        assert_eq!(remote["result"]["remote"], true, "{model}: {remote}");
    }
    assert_eq!(
        error_code(
            &client
                .request("models.details", json!({ "model": "missing:1b" }))
                .await
        ),
        code::REFUSED
    );
    assert_eq!(
        error_code(&client.request("models.details", json!({})).await),
        code::INVALID_PARAMS
    );
}

#[test]
fn pipe_names_and_access_lists_are_well_formed() {
    assert_eq!(pipe_name(&[0xab, 0x01]), r"\\.\pipe\chatonlm-ab01");
    assert_eq!(pipe_sddl("S-1-5-21-1"), "D:P(A;;GA;;;S-1-5-21-1)");
}

/// Tient la base occupée (une seule connexion SQLite) pendant `hold` ; rend
/// la main une fois le verrou pris. Les requêtes du pont qui lisent la base
/// attendent jusque-là.
async fn hold_store(store: &Store, hold: std::time::Duration) -> std::thread::JoinHandle<()> {
    let (taken, held) = std::sync::mpsc::channel();
    let store = store.clone();
    let thread = std::thread::spawn(move || {
        store
            .blocking(move |_| {
                taken.send(()).unwrap();
                std::thread::sleep(hold);
                Ok(())
            })
            .unwrap();
    });
    tokio::task::spawn_blocking(move || held.recv().unwrap())
        .await
        .unwrap();
    thread
}

/// Conversations de la base, éphémères comprises.
async fn all_chats(store: &Store) -> Vec<(String, bool)> {
    store
        .call(|c| {
            let mut statement = c.prepare("SELECT id, ephemeral FROM chats ORDER BY id")?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
        .unwrap()
}

async fn exists(store: &Store, chat: &str) -> bool {
    all_chats(store).await.iter().any(|(id, _)| id == chat)
}

/// Conversation éphémère ouverte par l'éditeur, répondue.
async fn ephemeral_chat(client: &mut Client) -> String {
    let sent = client
        .request(
            "chat.send",
            json!({ "content": "Secret", "ephemeral": true }),
        )
        .await;
    assert_eq!(sent["result"]["outcome"], "done", "{sent}");
    sent["result"]["chatId"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn chat_forget_erases_only_an_ephemeral_chat_of_the_editor() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let ephemeral = ephemeral_chat(&mut client).await;
    let ordinary = client
        .request("chat.send", json!({ "content": "Hello" }))
        .await["result"]["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let app = bench
        .store
        .call(|c| chats::create_with(c, 1, true))
        .await
        .unwrap();

    for kept in [&ordinary, &app] {
        let answer = client
            .request("chat.forget", json!({ "chatId": kept }))
            .await;
        assert_eq!(answer["result"], json!({ "forgotten": false }), "{answer}");
        assert!(exists(&bench.store, kept).await, "untouched");
    }
    let unknown = client
        .request("chat.forget", json!({ "chatId": "nowhere" }))
        .await;
    assert_eq!(unknown["result"], json!({ "forgotten": false }));
    assert_eq!(
        error_code(&client.request("chat.forget", json!({})).await),
        code::INVALID_PARAMS
    );

    let answer = client
        .request("chat.forget", json!({ "chatId": ephemeral }))
        .await;
    assert_eq!(answer["result"], json!({ "forgotten": true }), "{answer}");
    assert!(!exists(&bench.store, &ephemeral).await);
}

#[tokio::test]
async fn chat_forget_stops_the_reply_in_progress_first() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let id = client.next;
    client.next += 1;
    client
        .send(&json!({
            "jsonrpc": "2.0", "id": id, "method": "chat.send",
            "params": { "content": "Wait", "model": "chatonlm-hang", "ephemeral": true }
        }))
        .await;
    let mut chat = String::new();
    loop {
        let message = client.next_message().await.unwrap();
        let event = &message["params"]["event"];
        if event["type"] == "chatCreated" {
            chat = event["chatId"].as_str().unwrap().to_owned();
        }
        if event["type"] == "userMessage" {
            break;
        }
    }
    let forget = client.next;
    client.next += 1;
    client
        .send(&json!({
            "jsonrpc": "2.0", "id": forget, "method": "chat.forget",
            "params": { "chatId": chat }
        }))
        .await;
    assert_eq!(client.response(id).await["result"]["outcome"], "cancelled");
    assert_eq!(
        client.response(forget).await["result"],
        json!({ "forgotten": true })
    );
    assert!(all_chats(&bench.store).await.is_empty());
}

#[tokio::test]
async fn chat_keep_moves_an_ephemeral_chat_of_the_editor_to_the_history() {
    let bench = bench().await;
    let (mut client, id, _) = bench.paired().await;
    let ephemeral = ephemeral_chat(&mut client).await;
    assert_eq!(
        client.request("chat.list", json!({})).await["result"],
        json!([])
    );
    bench.host.updated.lock().clear();
    let kept = client
        .request("chat.keep", json!({ "chatId": ephemeral }))
        .await;
    assert_eq!(kept["result"], json!({}), "{kept}");
    assert_eq!(*bench.host.updated.lock(), vec![ephemeral.clone()]);
    let listed = client.request("chat.list", json!({})).await;
    assert_eq!(listed["result"][0]["id"], ephemeral.as_str(), "{listed}");
    // Une conversation de l'app : refusée, intacte.
    let app = bench
        .store
        .call(|c| chats::create_with(c, 1, true))
        .await
        .unwrap();
    let refused = client.request("chat.keep", json!({ "chatId": app })).await;
    assert_eq!(error_code(&refused), code::REFUSED);
    assert!(refused["error"]["data"]["message"].is_string());
    assert!(all_chats(&bench.store).await.contains(&(app.clone(), true)));
    // Gardée, le retrait de l'éditeur ne l'efface plus.
    bench.bridge.unlink(id).await.unwrap();
    assert!(all_chats(&bench.store).await.contains(&(ephemeral, false)));
}

#[tokio::test]
async fn removing_an_editor_or_switching_off_erases_its_ephemeral_chats() {
    let bench = bench().await;
    let (mut first, first_id, _) = bench.paired().await;
    let (mut second, _, _) = bench.paired().await;
    let erased = ephemeral_chat(&mut first).await;
    let ordinary = first
        .request("chat.send", json!({ "content": "Hello" }))
        .await["result"]["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    let other = ephemeral_chat(&mut second).await;
    let app = bench
        .store
        .call(|c| chats::create_with(c, 1, true))
        .await
        .unwrap();

    bench.bridge.unlink(first_id).await.unwrap();
    assert!(!exists(&bench.store, &erased).await, "its ephemeral chat");
    assert!(exists(&bench.store, &ordinary).await);
    assert!(exists(&bench.store, &other).await, "another editor's");

    bench.bridge.set_enabled(false).await.unwrap();
    assert!(!exists(&bench.store, &other).await);
    assert!(exists(&bench.store, &ordinary).await);
    assert!(
        exists(&bench.store, &app).await,
        "an ephemeral chat of the app"
    );
}

#[tokio::test]
async fn an_ephemeral_chat_created_after_the_editor_left_is_erased() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    // L'envoi attend la base ; l'éditeur part entre-temps (fenêtre fermée,
    // ou retrait) : il n'apprendra jamais la conversation, rien ne la
    // garderait.
    let held = hold_store(&bench.store, std::time::Duration::from_millis(800)).await;
    client
        .send(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "chat.send",
            "params": { "content": "Secret", "ephemeral": true }
        }))
        .await;
    drop(client);
    tokio::task::spawn_blocking(move || held.join().unwrap())
        .await
        .unwrap();
    let (store, host) = (bench.store.clone(), bench.host.clone());
    let erased = tokio::time::timeout(std::time::Duration::from_secs(10), async move {
        // Créée (la page l'apprend), puis effacée à la fin de l'envoi.
        while host.updated.lock().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        while !all_chats(&store).await.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(erased.is_ok(), "{:?}", all_chats(&bench.store).await);
}

#[tokio::test]
async fn a_cancel_before_the_engine_starts_stops_the_send() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    // Préparation lente (démarrage de l'Ollama intégré, base occupée) : Stop
    // arrive avant que le moteur ait commencé.
    let held = hold_store(&bench.store, std::time::Duration::from_millis(300)).await;
    client
        .send(&json!({
            "jsonrpc": "2.0", "id": 7, "method": "chat.send",
            "params": { "content": "Wait", "model": "chatonlm-hang" }
        }))
        .await;
    client
        .send(&json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": 7 } }))
        .await;
    let answer = client.response(7).await;
    held.join().unwrap();
    assert_eq!(answer["result"]["outcome"], "cancelled", "{answer}");
    assert_eq!(answer["result"]["chatId"], Value::Null);
    assert!(all_chats(&bench.store).await.is_empty(), "nothing written");

    // Stop lu juste après l'envoi, sans attente : arrêté aussi.
    client
        .send(&json!({
            "jsonrpc": "2.0", "id": 8, "method": "chat.send",
            "params": { "content": "Wait", "model": "chatonlm-hang" }
        }))
        .await;
    client
        .send(&json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": 8 } }))
        .await;
    assert_eq!(client.response(8).await["result"]["outcome"], "cancelled");
}

#[tokio::test]
async fn app_state_tells_where_the_server_runs_and_the_context() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let state = client.request("app.state", json!({})).await;
    assert_eq!(state["result"]["local"], true, "{state}");
    assert_eq!(state["result"]["contextLength"], 0);
    let patch = SettingsPatch {
        server_url: Some("http://192.168.1.10:11434".to_owned()),
        context_length: Some(8192),
        ..Default::default()
    };
    bench
        .store
        .call(|c| settings::update(c, patch, None).map(drop))
        .await
        .unwrap();
    let state = client.request("app.state", json!({})).await;
    assert_eq!(state["result"]["local"], false, "{state}");
    assert_eq!(state["result"]["contextLength"], 8192);
}

#[tokio::test]
async fn models_details_give_the_context_the_engine_will_use() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let context = async |client: &mut Client, model: &str| {
        let details = client
            .request("models.details", json!({ "model": model }))
            .await;
        assert!(details.get("result").is_some(), "{details}");
        details["result"]["contextLength"].clone()
    };
    // Contexte du serveur, aucun modèle chargé : inconnu.
    assert_eq!(context(&mut client, "qwen3:0.6b").await, Value::Null);
    for (set, expected) in [(8192, json!(8192)), (65536, json!(40960))] {
        let patch = SettingsPatch {
            context_length: Some(set),
            ..Default::default()
        };
        bench
            .store
            .call(|c| settings::update(c, patch, None).map(drop))
            .await
            .unwrap();
        assert_eq!(context(&mut client, "qwen3:0.6b").await, expected, "{set}");
    }
    assert_eq!(
        context(&mut client, "gpt-oss:20b-cloud").await,
        Value::Null,
        "a cloud model"
    );
}

#[tokio::test]
async fn an_answer_larger_than_a_frame_is_refused_without_closing() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let chat = bench
        .store
        .call(|c| {
            let id = chats::create_with(c, 1, false)?;
            let content = "a".repeat(frame::MAX_BODY + 1);
            chats::add_user_message(c, &id, &content, &[], 2)?;
            Ok(id)
        })
        .await
        .unwrap();
    let answer = client.request("chat.get", json!({ "chatId": chat })).await;
    assert_eq!(error_code(&answer), code::REFUSED);
    assert_eq!(
        answer["error"]["data"]["message"],
        "This chat is too large to be opened in VS Code. Open it in ChatonLM."
    );
    assert_eq!(client.request("ping", json!({})).await["result"], json!({}));
}

#[tokio::test]
async fn a_removal_during_authentication_never_leaves_it_open() {
    let bench = bench().await;
    let (_, id, secret) = bench.paired().await;
    let mut client = bench.connect();
    client.initialize().await;
    // Un retrait en cours (il tient le verrou) : la preuve attend sa fin.
    let changing = bench.bridge.shared().changing.lock().await;
    let proof = proof::client_proof(&secret, &client.server_nonce, &client.nonce);
    client
        .send(&json!({
            "jsonrpc": "2.0", "id": 50, "method": "auth",
            "params": { "clientId": id, "proof": proof::to_hex(&proof) }
        }))
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let removed = id.clone();
    bench
        .store
        .call(move |c| saved::remove_client(c, &removed))
        .await
        .unwrap();
    drop(changing);
    assert_eq!(error_code(&client.response(50).await), code::UNKNOWN_CLIENT);
    assert_eq!(
        error_code(&client.request("ping", json!({})).await),
        code::UNAUTHENTICATED
    );
}

#[test]
fn a_pipe_instance_is_replaced_after_a_failed_connect() {
    let failed = || Err(std::io::Error::other("ERROR_NO_DATA"));
    // Client servi : l'instance suivante est prête avant de rendre celle-ci.
    let mut next = 1;
    assert_eq!(after_connect(&mut next, Ok(()), || Ok(2)).unwrap(), 1);
    assert_eq!(next, 2);
    // Attente en échec : instance remplacée, erreur rendue.
    assert!(after_connect(&mut next, failed(), || Ok(3)).is_err());
    assert_eq!(next, 3);
    // Remplacement impossible : l'ancienne reste, rien n'est perdu.
    let refused = || Err(std::io::Error::other("no more instances"));
    assert!(after_connect(&mut next, failed(), refused).is_err());
    assert_eq!(next, 3);
    assert!(after_connect(&mut next, Ok(()), refused).is_err());
    assert_eq!(next, 3);
}

/// Les méthodes de la voix sont dans la liste blanche, après
/// l'authentification seulement ; paramètres vérifiés.
#[tokio::test]
async fn the_voice_methods_are_served_after_authentication_only() {
    let bench = bench().await;
    let methods = [
        "tour.plan",
        "speech.read",
        "speech.stop",
        "voice.start",
        "voice.stop",
        "voice.mute",
        "voice.resume",
        "voice.context",
    ];
    let mut client = bench.connect();
    client.initialize().await;
    for method in methods {
        assert_eq!(
            error_code(&client.request(method, json!({})).await),
            code::UNAUTHENTICATED,
            "{method}"
        );
    }
    let (mut client, _, _) = bench.paired().await;
    for method in methods {
        let answer = client.request(method, json!({ "unknown": 1 })).await;
        assert_eq!(error_code(&answer), code::INVALID_PARAMS, "{method}");
    }
    assert_eq!(
        client.request("voice.stop", json!({})).await["result"],
        json!({})
    );
    assert!(bench.host.voice.links.lock().is_empty());
}

/// `speech.read` : lecture demandée à la voix de l'app, identifiant rendu,
/// ses évènements relayés tels quels ; `speech.stop` n'arrête que les
/// lectures de la connexion ; refus traduits en codes.
#[tokio::test]
async fn speech_read_relays_the_events_of_its_reading() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let voice = bench.host.voice.clone();
    let answer = client
        .request(
            "speech.read",
            json!({ "segments": [{ "id": "s1", "text": "Hello." }, { "id": "s2", "text": "Bye." }], "language": "en" }),
        )
        .await;
    let read = answer["result"]["readId"].as_str().unwrap().to_owned();
    {
        let reads = voice.reads.lock();
        let (read_id, segments, language) = &reads[0];
        assert_eq!(*read_id, read);
        assert_eq!(
            segments.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["s1", "s2"]
        );
        assert_eq!(segments[1].text, "Bye.");
        assert_eq!(*language, Some(VoiceLanguage::En));
    }
    let sink = voice.sinks.lock()[0].clone();
    sink(SegmentEvent::Started("s1".into()));
    sink(SegmentEvent::Finished("s1".into()));
    sink(SegmentEvent::Stopped);
    sink(SegmentEvent::Failed(ReadingEnd::Failed(
        ReadAloudError::NoOutputDevice,
    )));
    let mut events = Vec::new();
    for _ in 0..4 {
        events.push(client.notification("speech.event").await);
    }
    assert_eq!(
        events[..3],
        [
            json!({ "readId": read, "type": "started", "segmentId": "s1" }),
            json!({ "readId": read, "type": "finished", "segmentId": "s1" }),
            json!({ "readId": read, "type": "stopped" }),
        ]
    );
    assert_eq!(events[3]["type"], "error");
    assert!(events[3]["message"].as_str().unwrap().contains("audio"));

    // Arrêt : la sienne seulement.
    let stop = |read: &str| json!({ "readId": read });
    assert_eq!(
        client.request("speech.stop", stop(&read)).await["result"],
        json!({})
    );
    client.request("speech.stop", stop("999999-1")).await;
    assert_eq!(*voice.stopped_reads.lock(), std::slice::from_ref(&read));

    // Refus.
    *voice.read_refusal.lock() = Some(SegmentsRefusal::MissingVoice(VoiceLanguage::Fr));
    let segments = json!({ "segments": [{ "id": "a", "text": "Bonjour." }] });
    let missing = client.request("speech.read", segments.clone()).await;
    assert_eq!(error_code(&missing), voice_code::MISSING_VOICE);
    assert_eq!(missing["error"]["data"], json!({ "language": "fr" }));
    *voice.read_refusal.lock() = Some(SegmentsRefusal::NothingToRead);
    let nothing = client.request("speech.read", segments).await;
    assert_eq!(error_code(&nothing), code::REFUSED);
    assert!(nothing["error"]["data"]["message"].is_string());
    // Mal formées : aucun segment, trop de segments, identifiant vide.
    *voice.read_refusal.lock() = None;
    let many: Vec<Value> = (0..65)
        .map(|i| json!({ "id": i.to_string(), "text": "x" }))
        .collect();
    for params in [
        json!({ "segments": [] }),
        json!({ "segments": many }),
        json!({ "segments": [{ "id": "", "text": "x" }] }),
        json!({ "segments": [{ "id": "a", "text": "x" }], "language": "de" }),
    ] {
        assert_eq!(
            error_code(&client.request("speech.read", params).await),
            code::INVALID_PARAMS
        );
    }
    assert_eq!(voice.reads.lock().len(), 1);
}

/// Attend `condition` (5 s au plus).
async fn until(what: &str, condition: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !condition() {
        assert!(std::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// Connexion authentifiée de plus pour l'éditeur `id`.
async fn reconnect(bench: &Bench, id: &str, secret: &[u8; 32]) -> Client {
    let mut client = bench.connect();
    client.initialize().await;
    assert!(client.auth(id, secret).await.get("result").is_some());
    client
}

/// Séance démarrée par `client` : son lien.
async fn voice_session(bench: &Bench, client: &mut Client, params: Value) -> Arc<dyn EditorLink> {
    let count = bench.host.voice.links.lock().len();
    let started = client.request("voice.start", params).await;
    assert!(started["result"]["sessionId"].is_string(), "{started}");
    bench.host.voice.links.lock()[count].clone()
}

/// Le micro ouvert par le pont s'arrête toujours : fin de la connexion,
/// éditeur retiré, pont coupé ; la lecture de la connexion aussi.
#[tokio::test]
async fn the_voice_of_a_connection_stops_when_it_closes() {
    let bench = bench().await;
    let voice = bench.host.voice.clone();
    let (mut client, id, secret) = bench.paired().await;
    let link = voice_session(&bench, &mut client, json!({ "profile": "duck" })).await;
    assert!(link.alive());
    let connection = link.connection();
    assert!(!voice.stops.lock().contains(&connection));
    // Fin de la connexion (VS Code fermé).
    drop(client);
    until("stopped at the end of the connection", || {
        voice.stops.lock().contains(&connection)
    })
    .await;
    assert!(voice.reads_stopped_for.lock().contains(&connection));
    assert!(!link.alive());

    // Éditeur retiré : arrêtée aussitôt.
    let mut client = reconnect(&bench, &id, &secret).await;
    let link = voice_session(&bench, &mut client, json!({ "profile": "tour" })).await;
    bench.bridge.unlink(id.clone()).await.unwrap();
    assert!(voice.stops.lock().contains(&link.connection()));
    assert!(!link.alive());

    // Pont coupé : arrêtée aussitôt.
    let (mut client, _, _) = bench.paired().await;
    let link = voice_session(&bench, &mut client, json!({ "profile": "duck" })).await;
    bench.bridge.set_enabled(false).await.unwrap();
    assert!(voice.stops.lock().contains(&link.connection()));
    assert!(!link.alive());
}

/// Refus du démarrage traduits en codes : séance tenue (par la page ou un
/// éditeur), modèle de transcription ou voix absents, micro.
#[tokio::test]
async fn a_refused_voice_session_is_told_by_its_code() {
    let bench = bench().await;
    let voice = bench.host.voice.clone();
    let (mut client, _, _) = bench.paired().await;
    let start = json!({ "profile": "duck" });
    let cases = [
        (
            StartError::Busy { page: true },
            voice_code::BUSY,
            json!({ "owner": "page" }),
        ),
        (
            StartError::Busy { page: false },
            voice_code::BUSY,
            json!({ "owner": "bridge" }),
        ),
        (
            StartError::VoiceMissing(VoiceLanguage::Es),
            voice_code::MISSING_VOICE,
            json!({ "language": "es" }),
        ),
        (
            StartError::Failed("no mic".into()),
            voice_code::MICROPHONE,
            json!({ "message": "no mic" }),
        ),
    ];
    for (refusal, code, data) in cases {
        *voice.start_refusal.lock() = Some(refusal);
        let answer = client.request("voice.start", start.clone()).await;
        assert_eq!(error_code(&answer), code, "{answer}");
        assert_eq!(answer["error"]["data"], data);
    }
    *voice.start_refusal.lock() = Some(StartError::SpeechModelMissing);
    let answer = client.request("voice.start", start.clone()).await;
    assert_eq!(error_code(&answer), voice_code::MISSING_SPEECH_MODEL);
    // Conversation inconnue : refus du moteur, rien ne démarre.
    *voice.start_refusal.lock() = None;
    let answer = client
        .request(
            "voice.start",
            json!({ "profile": "duck", "chatId": "nope" }),
        )
        .await;
    assert_eq!(error_code(&answer), code::REFUSED);
    assert!(voice.links.lock().is_empty());
}

/// Phrase d'un tour de la séance de l'éditeur : envoyée au chat par le
/// moteur (envoi vocal, consigne du profil puis contexte de l'éditeur en
/// message système, réflexion coupée), conversation créée marquée « VS
/// Code » puis gardée pour le tour suivant ; évènements relayés avec leur
/// tour ; état et niveaux relayés ; un envoi refusé d'emblée finit le tour.
#[tokio::test]
async fn a_spoken_turn_is_sent_by_the_engine_and_relayed() {
    let bench = bench().await;
    let voice = bench.host.voice.clone();
    let (mut client, _, _) = bench.paired().await;
    let link = voice_session(
        &bench,
        &mut client,
        json!({ "profile": "duck", "model": "qwen3:0.6b", "context": "fn main() {}" }),
    )
    .await;
    assert_eq!(
        client
            .request(
                "voice.context",
                json!({ "text": "error[E0425]: cannot find value `x`" })
            )
            .await["result"],
        json!({})
    );
    // État et niveaux (au plus ~15 par seconde).
    let listening = VoiceModeState {
        phase: VoiceModePhase::Listening,
        editor: true,
        ..VoiceModeState::default()
    };
    link.changed(&listening);
    let state = client.notification("voice.state").await;
    assert_eq!(state["state"]["phase"], "listening");
    assert_eq!(state["state"]["editor"], true);
    for _ in 0..5 {
        link.level(crate::voice_mode::VoiceModeLevel {
            input: 0.5,
            output: 0.0,
        });
    }
    assert_eq!(
        client.notification("voice.level").await["level"]["input"],
        0.5
    );

    let say = |turn: u32, text: &str| VoiceModeUtterance {
        turn,
        text: text.into(),
        language: Some(VoiceLanguage::En),
    };
    link.utterance(&say(1, "It crashes when it starts."));
    let mut chat = None;
    loop {
        let relayed = client.notification("voice.chat").await;
        assert_eq!(relayed["turn"], 1);
        let event = &relayed["event"];
        if event["type"] == "chatCreated" {
            chat = event["chatId"].as_str().map(str::to_owned);
        }
        if event["type"] == "done" {
            break;
        }
    }
    let chat = chat.expect("a chat was created");
    assert_eq!(
        client
            .notifications
            .iter()
            .filter(|n| n["method"] == "voice.level")
            .count(),
        0,
        "levels are throttled"
    );
    let request = bench._ollama.chat_requests().last().unwrap().clone();
    let system = request["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(request["messages"][0]["role"], "system");
    assert!(system.starts_with(crate::voice_mode::DEFAULT_INSTRUCTIONS));
    assert!(system.contains(&crate::i18n::t!("bridge.voice.duck")));
    assert!(system.contains("cannot find value"));
    assert!(!system.contains("fn main"), "the context was replaced");
    assert_eq!(request["think"], false);
    let marks = bench.host.marks.lock().clone();
    assert!(marks.contains(&(chat.clone(), Some(ChatSource::Vscode))));

    // Tour suivant : même conversation.
    link.utterance(&say(2, "Maybe the config file is missing."));
    loop {
        let relayed = client.notification("voice.chat").await;
        assert_eq!(relayed["turn"], 2);
        if relayed["event"]["type"] == "done" {
            break;
        }
    }
    let saved = bench
        .store
        .call(move |c| chats::get(c, &chat))
        .await
        .unwrap();
    assert_eq!(saved.messages.len(), 4, "two turns, no instructions saved");
    assert_eq!(
        saved.messages[2].content,
        "Maybe the config file is missing."
    );

    // Envoi refusé d'emblée (phrase vide) : le tour finit avec un avis.
    link.utterance(&say(3, "   "));
    until("reply failed", || !voice.failed_replies.lock().is_empty()).await;
    let (connection, turn, _) = voice.failed_replies.lock()[0].clone();
    assert_eq!((connection, turn), (link.connection(), 3));
}

/// `tour.plan` : plan du modèle en JSON contraint, lignes rapportées au code
/// donné ; un plan invalide est redemandé une fois, puis refusé.
#[tokio::test]
async fn tour_plan_gives_segments_checked_against_the_code() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let code = "use std::io;\nfn main() {\n    println!(\"hi\");\n}";
    let params = |model: &str| {
        json!({
            "model": model,
            "locale": "fr",
            "path": "src/main.rs",
            "language": "rust",
            "code": code,
            "firstLine": 10,
            "kind": "file",
        })
    };
    let texts = fake_ollama::tour::TOUR_TEXTS[1];
    let answer = client.request("tour.plan", params("chatonlm-tour")).await;
    assert_eq!(
        answer["result"],
        json!({ "segments": [
            { "lines": [10, 10], "text": texts[0] },
            { "lines": [11, 13], "text": texts[1] },
        ] }),
        "{answer}"
    );
    let requests = bench._ollama.chat_requests();
    let sent = requests.last().unwrap();
    assert_eq!(sent["stream"], false);
    assert!(sent["format"]["properties"]["segments"].is_object());
    assert!(
        sent["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("   10 | use std::io;")
    );
    assert!(
        sent["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("French")
    );

    let before = bench._ollama.chat_requests().len();
    let refused = client
        .request("tour.plan", params("chatonlm-tour-invalid"))
        .await;
    assert_eq!(error_code(&refused), code::REFUSED);
    assert!(refused["error"]["data"]["message"].is_string());
    assert_eq!(
        bench._ollama.chat_requests().len(),
        before + 2,
        "asked twice"
    );

    let mut bad = params("chatonlm-tour");
    bad["firstLine"] = json!(0);
    assert_eq!(
        error_code(&client.request("tour.plan", bad).await),
        code::INVALID_PARAMS
    );
    let mut empty = params("chatonlm-tour");
    empty["code"] = json!("  ");
    assert_eq!(
        error_code(&client.request("tour.plan", empty).await),
        code::INVALID_PARAMS
    );
}

/// Séance éphémère (`ephemeral`, sans `chatId`) : la conversation ouverte
/// par le premier tour est éphémère, marquée « VS Code », gardée pour le
/// tour suivant, et suivie comme les autres éphémères du pont : effacée au
/// retrait de l'éditeur.
#[tokio::test]
async fn an_ephemeral_voice_session_opens_an_ephemeral_editor_chat() {
    let bench = bench().await;
    let (mut client, id, _) = bench.paired().await;
    let link = voice_session(
        &bench,
        &mut client,
        json!({ "profile": "duck", "model": "qwen3:0.6b", "ephemeral": true }),
    )
    .await;
    let say = |turn: u32, text: &str| VoiceModeUtterance {
        turn,
        text: text.into(),
        language: Some(VoiceLanguage::En),
    };
    let mut chat = None;
    for (turn, text) in [(1, "It crashes."), (2, "Only on Windows.")] {
        link.utterance(&say(turn, text));
        loop {
            let relayed = client.notification("voice.chat").await;
            let event = &relayed["event"];
            if event["type"] == "chatCreated" {
                assert_eq!(turn, 1, "one chat for the session");
                chat = event["chatId"].as_str().map(str::to_owned);
            }
            if event["type"] == "done" {
                break;
            }
        }
    }
    let chat = chat.expect("a chat was created");
    let checked = chat.clone();
    assert!(
        bench
            .store
            .call(move |c| chats::is_editor_ephemeral(c, &checked))
            .await
            .unwrap(),
        "ephemeral and marked VS Code"
    );
    assert_eq!(
        all_chats(&bench.store).await,
        [(chat.clone(), true)],
        "only one chat, ephemeral"
    );
    let checked = chat.clone();
    let saved = bench
        .store
        .call(move |c| chats::get(c, &checked))
        .await
        .unwrap();
    assert_eq!(saved.messages.len(), 4);
    bench.bridge.unlink(id).await.unwrap();
    assert!(!exists(&bench.store, &chat).await, "erased with its editor");
}

/// Message suivant tel qu'il arrive (réponse ou notification), sans tri.
async fn raw(client: &mut Client) -> Value {
    if !client.notifications.is_empty() {
        return client.notifications.remove(0);
    }
    client.next_message().await.expect("connection open")
}

/// `voice.stop` lu pendant que `voice.start` attend la base (conversation
/// vérifiée) : rien ne démarre, la requête est arrêtée (-32800).
#[tokio::test]
async fn a_stop_while_the_start_reads_the_chat_starts_nothing() {
    let bench = bench().await;
    let voice = bench.host.voice.clone();
    let (mut client, _, _) = bench.paired().await;
    let chat = client
        .request("chat.send", json!({ "content": "Hello" }))
        .await["result"]["chatId"]
        .as_str()
        .unwrap()
        .to_owned();
    // Base lente : `voice.start` attend sa lecture.
    let held = hold_store(&bench.store, std::time::Duration::from_millis(400)).await;
    let start = client.next;
    client.next += 1;
    client
        .send(
            &json!({ "jsonrpc": "2.0", "id": start, "method": "voice.start",
            "params": { "profile": "duck", "chatId": chat } }),
        )
        .await;
    let stopped = client.request("voice.stop", json!({})).await;
    assert_eq!(stopped["result"], json!({}));
    held.join().unwrap();
    let answer = client.response(start).await;
    assert_eq!(error_code(&answer), code::CANCELLED, "{answer}");
    assert!(voice.links.lock().is_empty(), "the microphone never opened");
}

/// `voice.stop` lu pendant que le micro s'ouvre : la séance n'est plus
/// voulue (la séance réelle s'arrête alors d'elle-même, `alive`), et si le
/// micro s'est ouvert, il se referme ; la requête est arrêtée (-32800).
#[tokio::test]
async fn a_stop_while_the_microphone_opens_closes_it() {
    let bench = bench().await;
    let voice = bench.host.voice.clone();
    let (mut client, _, _) = bench.paired().await;
    let gate = Arc::new(tokio::sync::Notify::new());
    *voice.start_gate.lock() = Some(gate.clone());
    let start = client.next;
    client.next += 1;
    client
        .send(
            &json!({ "jsonrpc": "2.0", "id": start, "method": "voice.start",
            "params": { "profile": "duck" } }),
        )
        .await;
    until("the start reached the voice", || {
        !voice.links.lock().is_empty()
    })
    .await;
    let link = voice.links.lock()[0].clone();
    assert!(link.alive());
    client.request("voice.stop", json!({})).await;
    assert!(!link.alive(), "a session starting now stops by itself");
    gate.notify_one();
    let answer = client.response(start).await;
    assert_eq!(error_code(&answer), code::CANCELLED, "{answer}");
    let stops = voice
        .stops
        .lock()
        .iter()
        .filter(|&&stopped| stopped == link.connection())
        .count();
    assert_eq!(stops, 2, "stopped by voice.stop, then once it was open");
}

/// Refus après `starting` : la réponse d'erreur part d'abord, puis l'état
/// `off` (l'extension garde la raison).
#[tokio::test]
async fn a_refused_start_answers_before_the_state_goes_off() {
    let bench = bench().await;
    let voice = bench.host.voice.clone();
    let (mut client, _, _) = bench.paired().await;
    *voice.start_states.lock() = vec![VoiceModeState {
        phase: VoiceModePhase::Starting,
        editor: true,
        ..VoiceModeState::default()
    }];
    *voice.start_refusal.lock() = Some(StartError::SpeechModelMissing);
    let start = client.next;
    client.next += 1;
    client
        .send(
            &json!({ "jsonrpc": "2.0", "id": start, "method": "voice.start",
            "params": { "profile": "duck" } }),
        )
        .await;
    let first = raw(&mut client).await;
    assert_eq!(first["method"], "voice.state");
    assert_eq!(first["params"]["state"]["phase"], "starting");
    let second = raw(&mut client).await;
    assert_eq!(second["id"], start, "{second}");
    assert_eq!(error_code(&second), voice_code::MISSING_SPEECH_MODEL);
    let third = raw(&mut client).await;
    assert_eq!(third["method"], "voice.state");
    assert_eq!(third["params"]["state"]["phase"], "off");
    // Sans `starting` appris (refus d'emblée), pas d'état `off` en plus.
    voice.start_states.lock().clear();
    *voice.start_refusal.lock() = Some(StartError::Busy { page: true });
    let answer = client
        .request("voice.start", json!({ "profile": "duck" }))
        .await;
    assert_eq!(error_code(&answer), voice_code::BUSY);
    assert_eq!(client.request("ping", json!({})).await["result"], json!({}));
    assert!(
        client
            .notifications
            .iter()
            .all(|n| n["method"] != "voice.state"),
        "{:?}",
        client.notifications
    );
}

/// Second `voice.start` sur une connexion qui tient déjà une séance : refusé
/// (-32032, `bridge`), la séance en cours reste inscrite (son contexte suit).
#[tokio::test]
async fn a_second_start_on_the_same_connection_is_refused() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let link = voice_session(&bench, &mut client, json!({ "profile": "duck" })).await;
    let again = client
        .request("voice.start", json!({ "profile": "tour" }))
        .await;
    assert_eq!(error_code(&again), voice_code::BUSY);
    assert_eq!(again["error"]["data"], json!({ "owner": "bridge" }));
    assert_eq!(bench.host.voice.links.lock().len(), 1);
    assert!(
        bench
            .bridge
            .shared()
            .holds_talk(link.connection(), Arc::as_ptr(&link) as *const ()),
        "the running session is still the one kept"
    );
    assert!(link.alive());
}

/// `voice.resume` : reprise demandée pour la connexion ; refus du micro
/// traduit en -32033.
#[tokio::test]
async fn voice_resume_reopens_the_session_of_the_connection() {
    let bench = bench().await;
    let voice = bench.host.voice.clone();
    let (mut client, _, _) = bench.paired().await;
    let link = voice_session(&bench, &mut client, json!({ "profile": "duck" })).await;
    let resumed = client.request("voice.resume", json!({})).await;
    assert_eq!(resumed["result"], json!({}));
    assert_eq!(*voice.resumes.lock(), [link.connection()]);
    *voice.resume_refusal.lock() = Some("No microphone found".into());
    let refused = client.request("voice.resume", json!({})).await;
    assert_eq!(error_code(&refused), voice_code::MICROPHONE);
    assert_eq!(
        refused["error"]["data"],
        json!({ "message": "No microphone found" })
    );
}

/// Les tours continuent sur toute la connexion : une nouvelle séance part
/// du dernier tour de la précédente.
#[tokio::test]
async fn voice_turns_keep_counting_over_the_connection() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let link = voice_session(&bench, &mut client, json!({ "profile": "duck" })).await;
    assert_eq!(link.first_turn(), 0);
    link.utterance(&VoiceModeUtterance {
        turn: 2,
        text: "   ".into(),
        language: None,
    });
    client.request("voice.stop", json!({})).await;
    let next = voice_session(&bench, &mut client, json!({ "profile": "duck" })).await;
    assert_eq!(next.first_turn(), 2);
    // Une autre connexion compte les siens.
    let (mut other, _, _) = bench.paired().await;
    client.request("voice.stop", json!({})).await;
    let theirs = voice_session(&bench, &mut other, json!({ "profile": "duck" })).await;
    assert_eq!(theirs.first_turn(), 0);
}

/// Fin de la séance pendant une réponse : la réponse du tour s'arrête (le
/// panneau la montre annulée), comme `cancelled` dans `voice.chat`.
#[tokio::test]
async fn ending_the_session_stops_the_reply_of_its_turn() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let link = voice_session(
        &bench,
        &mut client,
        json!({ "profile": "duck", "model": "chatonlm-stall" }),
    )
    .await;
    link.utterance(&VoiceModeUtterance {
        turn: 1,
        text: "It crashes.".into(),
        language: Some(VoiceLanguage::En),
    });
    // La réponse a commencé (puis le modèle cale, sans fin).
    while client.notification("voice.chat").await["event"]["type"] != "content" {}
    client.request("voice.stop", json!({})).await;
    loop {
        let relayed = client.notification("voice.chat").await;
        assert_eq!(relayed["turn"], 1);
        if relayed["event"]["type"] == "cancelled" {
            break;
        }
    }
}

/// Modèle du cloud, réglage coupé : le tour finit avec un avis et rien ne
/// part, même pas `/api/show`.
#[tokio::test]
async fn a_cloud_model_is_refused_before_anything_leaves() {
    let bench = bench().await;
    let voice = bench.host.voice.clone();
    let (mut client, _, _) = bench.paired().await;
    let model = "gpt-oss:120b-cloud";
    let link = voice_session(
        &bench,
        &mut client,
        json!({ "profile": "duck", "model": model }),
    )
    .await;
    link.utterance(&VoiceModeUtterance {
        turn: 1,
        text: "It crashes.".into(),
        language: Some(VoiceLanguage::En),
    });
    until("reply failed", || !voice.failed_replies.lock().is_empty()).await;
    assert!(
        !bench
            ._ollama
            .show_requests()
            .iter()
            .any(|shown| shown.contains("cloud")),
        "{:?}",
        bench._ollama.show_requests()
    );
    assert!(
        bench
            ._ollama
            .chat_requests()
            .iter()
            .all(|sent| sent["model"] != model)
    );
}

/// Panique dans la tâche d'une connexion : sa séance est quand même
/// arrêtée (micro fermé).
#[tokio::test]
async fn a_panic_in_a_connection_still_stops_its_voice() {
    let bench = bench().await;
    let voice = bench.host.voice.clone();
    let (mut client, _, _) = bench.paired().await;
    let link = voice_session(&bench, &mut client, json!({ "profile": "duck" })).await;
    voice.panic_on_read.store(true, Ordering::SeqCst);
    client
        .send(
            &json!({ "jsonrpc": "2.0", "id": 999, "method": "speech.read",
            "params": { "segments": [{ "id": "a", "text": "Hi." }] } }),
        )
        .await;
    until("stopped after the panic", || {
        voice.stops.lock().contains(&link.connection())
    })
    .await;
    assert!(!link.alive());
}

/// `$/cancelRequest` arrête un `tour.plan` : réponse -32800 et requête au
/// modèle abandonnée.
#[tokio::test]
async fn a_tour_plan_is_cancelled_by_cancel_request() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let plan = client.next;
    client.next += 1;
    client
        .send(
            &json!({ "jsonrpc": "2.0", "id": plan, "method": "tour.plan", "params": {
            "model": "chatonlm-wait", "locale": "en", "path": "a.rs", "language": "rust",
            "code": "fn main() {}", "firstLine": 1, "kind": "file" } }),
        )
        .await;
    until("the plan was asked", || {
        bench
            ._ollama
            .chat_requests()
            .iter()
            .any(|sent| sent["model"] == "chatonlm-wait")
    })
    .await;
    client
        .send(&json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": plan } }))
        .await;
    let answer = client.response(plan).await;
    assert_eq!(error_code(&answer), code::CANCELLED, "{answer}");
    until("the model request was dropped", || {
        bench._ollama.disconnects() > 0
    })
    .await;
}

/// Envoie une requête sans attendre sa réponse ; rend son identifiant.
async fn start(client: &mut Client, method: &str, params: Value) -> i64 {
    let id = client.next;
    client.next += 1;
    client
        .send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
        .await;
    id
}

fn paused_reason(answer: &Value) -> &str {
    assert_eq!(error_code(answer), code::PAUSED, "{answer}");
    answer["error"]["data"]["reason"].as_str().unwrap()
}

#[tokio::test]
async fn the_gpu_methods_are_served_after_authentication_only() {
    let bench = bench().await;
    let mut client = bench.connect();
    for method in [
        "gpu.status",
        "completion.prepare",
        "complete.fim",
        "completion.measure",
        "completion.suggest",
        "models.offerPull",
    ] {
        assert_eq!(
            error_code(
                &client
                    .request(method, json!({ "model": "chatonlm-fim" }))
                    .await
            ),
            code::UNAUTHENTICATED,
            "{method}"
        );
    }
    assert!(bench._ollama.load_requests().is_empty());
}

/// Une proposition ne charge jamais : en pause tant que le modèle n'est pas
/// en mémoire ; « Préparer » le charge, puis elle répond ; le plan suit, et
/// l'éditeur à l'écoute apprend le changement.
#[tokio::test(flavor = "multi_thread")]
async fn completion_waits_for_prepare_and_the_plan_is_followed() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let fim = json!({ "model": "chatonlm-fim", "prefix": "def f(", "suffix": ")\n",
        "language": "python", "path": "a.py" });
    let answer = client.request("complete.fim", fim.clone()).await;
    assert_eq!(paused_reason(&answer), "notLoaded");
    assert!(bench._ollama.generate_requests().is_empty());
    assert!(bench._ollama.load_requests().is_empty());

    let status = client
        .request("gpu.status", json!({ "model": "chatonlm-fim" }))
        .await;
    let plan = &status["result"];
    assert_eq!(plan["completion"]["model"], "chatonlm-fim", "{status}");
    assert_eq!(plan["completion"]["loaded"], false);
    assert_eq!(plan["chat"]["model"], "qwen3:0.6b");
    assert_eq!(plan["reason"], "notLoaded");

    let prepared = client
        .request("completion.prepare", json!({ "model": "chatonlm-fim" }))
        .await;
    assert_eq!(
        prepared["result"]["plan"]["completion"]["loaded"], true,
        "{prepared}"
    );
    let changed = client.notification("gpu.changed").await;
    assert_eq!(changed["plan"]["completion"]["loaded"], true, "{changed}");

    let answer = client.request("complete.fim", fim).await;
    assert_eq!(
        answer["result"]["text"], "value(6|2)\n    second line",
        "{answer}"
    );
    assert_eq!(answer["result"]["timedOut"], false);
    assert!(answer["result"]["firstTokenMs"].is_u64());
    assert!(answer["result"]["totalMs"].is_u64());
    assert!(answer["result"]["cachedTokens"].is_u64());
    // Paramètres invalides.
    for params in [
        json!({ "model": "chatonlm-fim" }),
        json!({ "model": "", "prefix": "a" }),
        json!({ "model": "chatonlm-fim", "prefix": "a", "maxTokens": 0 }),
    ] {
        assert_eq!(
            error_code(&client.request("complete.fim", params.clone()).await),
            code::INVALID_PARAMS,
            "{params}"
        );
    }
    assert_eq!(
        error_code(&client.request("completion.prepare", json!({})).await),
        code::INVALID_PARAMS
    );
}

/// Une seule proposition en vol par connexion : la nouvelle arrête
/// l'ancienne (-32800) ; `$/cancelRequest` aussi.
#[tokio::test(flavor = "multi_thread")]
async fn one_completion_in_flight_and_cancel_request() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let prepared = client
        .request(
            "completion.prepare",
            json!({ "model": "chatonlm-fim-long" }),
        )
        .await;
    assert!(prepared.get("result").is_some(), "{prepared}");
    let long = json!({ "model": "chatonlm-fim-long", "prefix": "a", "suffix": "" });
    let first = start(&mut client, "complete.fim", long.clone()).await;
    until("the first request reached the model", || {
        bench._ollama.generate_requests().len() == 1
    })
    .await;
    let second = start(&mut client, "complete.fim", long.clone()).await;
    let answer = client.response(first).await;
    assert_eq!(error_code(&answer), code::CANCELLED, "{answer}");
    until("the second request reached the model", || {
        bench._ollama.generate_requests().len() == 2
    })
    .await;
    client
        .send(&json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": second } }))
        .await;
    let answer = client.response(second).await;
    assert_eq!(error_code(&answer), code::CANCELLED, "{answer}");
    // Les deux requêtes au modèle ont été abandonnées.
    until("the model requests were dropped", || {
        bench._ollama.disconnects() >= 2
    })
    .await;
    // Une autre connexion a sa propre proposition en vol.
    let (mut other, _, _) = bench.paired().await;
    let mine = start(&mut client, "complete.fim", long.clone()).await;
    let quick =
        json!({ "model": "chatonlm-fim-long", "prefix": "b", "suffix": "", "maxTokens": 1 });
    let theirs = other.request("complete.fim", quick).await;
    assert!(theirs.get("result").is_some(), "{theirs}");
    let answer = client.response(mine).await;
    assert!(answer.get("result").is_some(), "{answer}");
}

/// Pendant une réponse de conversation, l'autocomplétion attend ; une
/// proposition en cours s'arrête quand une réponse commence.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_reply_pauses_completion() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    client
        .request(
            "completion.prepare",
            json!({ "model": "chatonlm-fim-long" }),
        )
        .await;
    let long = json!({ "model": "chatonlm-fim-long", "prefix": "a", "suffix": "" });
    let fim = start(&mut client, "complete.fim", long.clone()).await;
    until("the request reached the model", || {
        bench._ollama.generate_requests().len() == 1
    })
    .await;
    let (mut chatting, _, _) = bench.paired().await;
    let send = start(
        &mut chatting,
        "chat.send",
        json!({ "content": "Wait", "model": "chatonlm-hang" }),
    )
    .await;
    let answer = client.response(fim).await;
    assert_eq!(paused_reason(&answer), "chatBusy");
    // Une nouvelle n'est même pas envoyée.
    let answer = client.request("complete.fim", long.clone()).await;
    assert_eq!(paused_reason(&answer), "chatBusy");
    let answer = client
        .request(
            "completion.measure",
            json!({ "model": "chatonlm-fim-long" }),
        )
        .await;
    assert_eq!(paused_reason(&answer), "chatBusy");
    assert_eq!(bench._ollama.generate_requests().len(), 1);
    let status = client.request("gpu.status", json!({})).await;
    assert_eq!(status["result"]["chatBusy"], true, "{status}");
    chatting
        .send(&json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": send } }))
        .await;
    chatting.response(send).await;
    // Réponse finie : le modèle chargé répond de nouveau.
    let quick = json!({ "model": "chatonlm-fim-long", "prefix": "b", "maxTokens": 1 });
    let answer = client.request("complete.fim", quick).await;
    assert!(answer.get("result").is_some(), "{answer}");
    let answer = client
        .request(
            "complete.fim",
            json!({ "model": "chatonlm-fim", "prefix": "a" }),
        )
        .await;
    // Un modèle pas chargé : en pause, mais plus pour la conversation.
    assert_eq!(paused_reason(&answer), "notLoaded");
}

/// Rapport 92, M5 et M6 : un autre travail de l'app sur Ollama
/// (Correction, Étude, polissage, chargement par la page…) met
/// l'autocomplétion en pause comme une réponse : la proposition en cours
/// s'arrête, la suivante n'est pas envoyée ; reprise à la fin.
#[tokio::test(flavor = "multi_thread")]
async fn other_work_on_ollama_pauses_completion() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    client
        .request(
            "completion.prepare",
            json!({ "model": "chatonlm-fim-long" }),
        )
        .await;
    let long = json!({ "model": "chatonlm-fim-long", "prefix": "a", "suffix": "" });
    let fim = start(&mut client, "complete.fim", long.clone()).await;
    until("the request reached the model", || {
        bench._ollama.generate_requests().len() == 1
    })
    .await;
    let working = bench.host.engine.hold();
    let answer = client.response(fim).await;
    assert_eq!(paused_reason(&answer), "chatBusy");
    let answer = client.request("complete.fim", long.clone()).await;
    assert_eq!(paused_reason(&answer), "chatBusy");
    assert_eq!(
        paused_reason(
            &client
                .request("completion.prepare", json!({ "model": "chatonlm-fim" }))
                .await
        ),
        "chatBusy"
    );
    let status = client.request("gpu.status", json!({})).await;
    assert_eq!(status["result"]["chatBusy"], true, "{status}");
    assert_eq!(bench._ollama.generate_requests().len(), 1);
    drop(working);
    let quick = json!({ "model": "chatonlm-fim-long", "prefix": "b", "maxTokens": 1 });
    let answer = client.request("complete.fim", quick).await;
    assert!(answer.get("result").is_some(), "{answer}");
}

/// Rapport 92, m6 : pendant une mesure, aucune proposition ne part de la
/// même connexion (elle fausserait la mesure) : -32800 aussitôt.
#[tokio::test(flavor = "multi_thread")]
async fn no_completion_while_measuring() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    client
        .request(
            "completion.prepare",
            json!({ "model": "chatonlm-fim-slow" }),
        )
        .await;
    let measure = start(
        &mut client,
        "completion.measure",
        json!({ "model": "chatonlm-fim-slow" }),
    )
    .await;
    until("the measure reached the model", || {
        bench._ollama.generate_requests().len() == 1
    })
    .await;
    let answer = client
        .request(
            "complete.fim",
            json!({ "model": "chatonlm-fim-slow", "prefix": "a" }),
        )
        .await;
    assert_eq!(error_code(&answer), code::CANCELLED, "{answer}");
    assert_eq!(bench._ollama.generate_requests().len(), 1);
    client
        .send(
            &json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": measure } }),
        )
        .await;
    assert_eq!(error_code(&client.response(measure).await), code::CANCELLED);
}

#[tokio::test(flavor = "multi_thread")]
async fn measure_suggest_and_offer_pull() {
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let answer = client
        .request("completion.measure", json!({ "model": "chatonlm-fim" }))
        .await;
    assert_eq!(paused_reason(&answer), "notLoaded");
    client
        .request("completion.prepare", json!({ "model": "chatonlm-fim" }))
        .await;
    let measure = client
        .request("completion.measure", json!({ "model": "chatonlm-fim" }))
        .await;
    let result = &measure["result"];
    assert_eq!(result["tokensPerSecond"], 200.0, "{measure}");
    assert!(["smooth", "usable", "tooSlow"].contains(&result["rating"].as_str().unwrap()));
    assert_eq!(result["partialGpu"], false);

    *bench.host.machine.lock() = Some(crate::hub::hardware::Machine {
        gpus: vec![],
        ram_total: 16 << 30,
        ram_available: 8 << 30,
    });
    let suggested = client.request("completion.suggest", json!({})).await;
    let result = &suggested["result"];
    assert_eq!(
        result["recommended"], "qwen2.5-coder:0.5b-base",
        "{suggested}"
    );
    let options = result["options"].as_array().unwrap();
    assert_eq!(options.len(), 4);
    let three = options
        .iter()
        .find(|option| option["model"] == "qwen2.5-coder:3b-base")
        .unwrap();
    assert_eq!(three["commercialUse"], false);
    assert_eq!(three["installed"], false);

    // Licence d'usage non commercial : dite dans la proposition de l'app.
    let offered = client
        .request(
            "models.offerPull",
            json!({ "model": "qwen2.5-coder:3b-base" }),
        )
        .await;
    assert_eq!(offered["result"]["installed"], false, "{offered}");
    assert_eq!(
        *bench.host.offers.lock(),
        ["qwen2.5-coder:3b-base (non-commercial)"]
    );
    // Une seule proposition montrée par 10 s : un éditeur qui insiste ne
    // ramène pas la fenêtre à chaque appel.
    let offered = client
        .request(
            "models.offerPull",
            json!({ "model": "qwen2.5-coder:1.5b-base" }),
        )
        .await;
    assert_eq!(offered["result"]["installed"], false, "{offered}");
    assert_eq!(bench.host.offers.lock().len(), 1);
    // Seulement un nom proposé ; jamais de téléchargement par le pont.
    for model in ["qwen3:0.6b", "chatonlm-fim", ""] {
        assert_eq!(
            error_code(
                &client
                    .request("models.offerPull", json!({ "model": model }))
                    .await
            ),
            code::INVALID_PARAMS,
            "{model}"
        );
    }
    assert_eq!(bench.host.offers.lock().len(), 1);
    assert!(bench._ollama.pull_requests().is_empty());
}

#[tokio::test]
async fn switching_off_and_on_quickly_always_reopens() {
    // Fermeture de l'ancien point d'accès retardée, comme sur une machine
    // chargée : la réouverture l'attend au lieu de le croire servi par une
    // autre instance.
    let mut bench = bench().await;
    let mut config = bench.bridge.shared().config.clone();
    config.close_delay = std::time::Duration::from_millis(300);
    bench.bridge = Bridge::new(bench.host.clone(), bench.store.clone(), config);
    for round in 0..3 {
        let state = bench.bridge.set_enabled(true).await.unwrap();
        assert_eq!(state.status, BridgeStatus::Listening, "round {round}");
        let state = bench.bridge.set_enabled(false).await.unwrap();
        assert_eq!(state.status, BridgeStatus::Off, "round {round}");
    }
    let state = bench.bridge.set_enabled(true).await.unwrap();
    assert_eq!(state.status, BridgeStatus::Listening);
    bench.bridge.shutdown();
}

// Lot 4a : Easy Link prévient, cours de l'Étude, image, correction.

impl Bench {
    async fn select_model(&self, model: &str) {
        let patch = SettingsPatch {
            selected_model: Some(model.to_owned()),
            ..Default::default()
        };
        self.store
            .call(move |c| settings::update(c, patch, None).map(drop))
            .await
            .unwrap();
    }

    fn notifier(&self) -> Arc<FakeNotifier> {
        let notifier = Arc::new(FakeNotifier::default());
        *self.host.notifier.lock() = Some(Arc::clone(&notifier));
        notifier
    }
}

async fn wait_for(what: &str, ready: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !ready() {
        assert!(std::time::Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn the_editor_notifies_through_easy_link_with_its_own_codes() {
    use crate::easy_link::notify::{NotifyError, Outcome, TaskState};
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    let done = json!({
        "title": "npm run build",
        "outcome": "success",
        "durationMs": 61_000,
        "workspace": "/home/ana/web",
    });
    // Easy Link indisponible : comme aucun compte lié.
    let answer = client.request("notify.send", done.clone()).await;
    assert_eq!(error_code(&answer), code::NOT_LINKED);

    let notifier = bench.notifier();
    let answer = client.request("notify.send", done.clone()).await;
    assert_eq!(answer["result"], json!({ "sent": 1 }), "{answer}");
    {
        let notes = notifier.notes.lock();
        assert_eq!(notes[0].title, "npm run build");
        assert_eq!(notes[0].outcome, Outcome::Success);
        assert_eq!(notes[0].duration_ms, 61_000);
        assert_eq!(notes[0].workspace.as_deref(), Some("/home/ana/web"));
    }
    *notifier.refusal.lock() = Some(NotifyError::Disabled);
    let answer = client.request("notify.send", done.clone()).await;
    assert_eq!(error_code(&answer), code::NOTIFY_OFF);
    let task = json!({ "id": "t1", "title": "build", "state": "running", "startedAt": 1 });
    let answer = client.request("notify.task", task.clone()).await;
    assert_eq!(error_code(&answer), code::NOTIFY_OFF);
    *notifier.refusal.lock() = Some(NotifyError::NotLinked);
    let answer = client.request("notify.send", done.clone()).await;
    assert_eq!(error_code(&answer), code::NOT_LINKED);
    *notifier.refusal.lock() = None;

    for bad in [
        json!({ "title": "x", "outcome": "maybe", "durationMs": 1 }),
        json!({ "title": "x", "outcome": "done", "durationMs": -1 }),
        json!({ "title": "x", "outcome": "done", "durationMs": 1, "output": "error: …" }),
        json!({ "title": "x".repeat(5000), "outcome": "done", "durationMs": 1 }),
    ] {
        let answer = client.request("notify.send", bad).await;
        assert_eq!(error_code(&answer), code::INVALID_PARAMS, "{answer}");
    }
    let answer = client.request("notify.task", task).await;
    assert_eq!(answer["result"], json!({}), "{answer}");
    let answer = client
        .request(
            "notify.task",
            json!({ "id": "", "title": "build", "state": "running", "startedAt": 1 }),
        )
        .await;
    assert_eq!(error_code(&answer), code::INVALID_PARAMS);
    let connection = {
        let tasks = notifier.tasks.lock();
        // Le refus (réglage coupé) puis l'acceptée ; l'invalide n'arrive pas.
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[1].1.state, TaskState::Running);
        assert_eq!(tasks[1].1.id, "t1");
        tasks[1].0
    };
    // La connexion finie : ses tâches en cours ne se finiront plus.
    drop(client);
    wait_for("tasks of the connection dropped", || {
        notifier.ended.lock().contains(&connection)
    })
    .await;
}

#[tokio::test]
async fn a_course_is_made_from_texts_sent_by_the_editor() {
    let bench = bench().await;
    bench.select_model("chatonlm-study").await;
    let (mut client, _, _) = bench.paired().await;
    let created = client
        .request(
            "study.create",
            json!({
                "topic": "This project",
                "level": "beginner",
                "duration": 10,
                "documents": [{ "name": "/home/ana/web/README.md", "text": "# Web\n\nA small site about photosynthesis." }],
                "language": "fr",
            }),
        )
        .await;
    let course = created["result"]["courseId"]
        .as_str()
        .unwrap_or_else(|| panic!("{created}"))
        .to_owned();
    // Progression : jusqu'au plan prêt.
    loop {
        let event = client.notification("study.event").await;
        assert_eq!(event["courseId"], course.as_str());
        if event["course"]["status"] == "planned" {
            break;
        }
        assert_eq!(event["course"]["status"], "planning", "{event}");
    }
    assert!(
        bench.host.studied.lock().contains(&course),
        "the page learns it"
    );
    let listed = client.request("study.list", json!({})).await;
    let listed = listed["result"].as_array().unwrap();
    assert!(
        listed
            .iter()
            .any(|summary| summary["id"] == course.as_str())
    );
    let row = bench
        .store
        .call({
            let course = course.clone();
            move |c| crate::store::study::load(c, &course)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.row.language, crate::i18n::Lang::Fr);
    assert_eq!(
        row.sources[0].source.title, "README.md",
        "only the file name is kept"
    );

    // Au-delà de 200 000 caractères : refus clair, rien de créé.
    let long = "word ".repeat(15_000);
    let documents: Vec<Value> = (0..3)
        .map(|index| json!({ "name": format!("{index}.md"), "text": long }))
        .collect();
    let answer = client
        .request(
            "study.create",
            json!({ "topic": "x", "level": "advanced", "duration": 20, "documents": documents }),
        )
        .await;
    assert_eq!(error_code(&answer), code::REFUSED);
    assert!(
        answer["error"]["data"]["message"]
            .as_str()
            .unwrap()
            .contains("200,000"),
        "{answer}"
    );
    let four: Vec<Value> = (0..4)
        .map(|index| json!({ "name": format!("{index}.md"), "text": "a" }))
        .collect();
    let answer = client
        .request(
            "study.create",
            json!({ "topic": "x", "level": "advanced", "duration": 20, "documents": four }),
        )
        .await;
    assert_eq!(error_code(&answer), code::REFUSED);
    let answer = client
        .request(
            "study.create",
            json!({ "topic": "x", "level": "advanced", "duration": 15 }),
        )
        .await;
    assert_eq!(error_code(&answer), code::REFUSED);
    let answer = client
        .request(
            "study.create",
            json!({ "topic": "x", "level": "expert", "duration": 10 }),
        )
        .await;
    assert_eq!(error_code(&answer), code::INVALID_PARAMS);
    let answer = client.request("study.list", json!({ "all": true })).await;
    assert_eq!(error_code(&answer), code::INVALID_PARAMS);
    let listed = client.request("study.list", json!({})).await;
    assert_eq!(listed["result"].as_array().unwrap().len(), 1);
}

/// La progression d'un cours créé par l'éditeur lui arrive même quand la
/// file de l'Étude est déjà tenue par un autre appel (onglet Study), pour deux
/// cours de suite, et sur une connexion revenue (après `study.list`).
#[tokio::test]
async fn the_editor_follows_its_courses_whoever_runs_the_study_queue() {
    use crate::i18n::Lang;
    use crate::study::StudyLevel;
    let bench = bench().await;
    bench.select_model("chatonlm-study-slow").await;
    // L'onglet Study prépare déjà un cours : la file tourne avec son `Ctx`.
    let page = crate::study::Ctx::new(bench.store.clone(), |_| {});
    let engine = Arc::clone(&bench.host.study);
    engine
        .create_from_texts(
            &page,
            "Photosynthesis",
            StudyLevel::Beginner,
            10,
            Vec::new(),
            Lang::En,
        )
        .await
        .unwrap();
    let (mut client, id, secret) = bench.paired().await;
    let mut courses = Vec::new();
    for topic in ["This project", "Rust lifetimes"] {
        let created = client
            .request(
                "study.create",
                json!({ "topic": topic, "level": "beginner", "duration": 10 }),
            )
            .await;
        courses.push(
            created["result"]["courseId"]
                .as_str()
                .unwrap_or_else(|| panic!("{created}"))
                .to_owned(),
        );
    }
    let mut planned = std::collections::HashSet::new();
    while planned.len() < courses.len() {
        let event = client.notification("study.event").await;
        let course = event["courseId"].as_str().unwrap().to_owned();
        assert_eq!(event["course"]["id"], course.as_str(), "{event}");
        if courses.contains(&course) && event["course"]["status"] == "planned" {
            planned.insert(course);
        }
    }

    // Connexion revenue : après `study.list`, elle suit de nouveau les cours.
    drop(client);
    let mut again = bench.connect();
    again.initialize().await;
    let authed = again.auth(&id, &secret).await;
    assert!(authed.get("result").is_some(), "{authed}");
    let listed = again.request("study.list", json!({})).await;
    assert_eq!(listed["result"].as_array().unwrap().len(), 3, "{listed}");
    let later = engine
        .create_from_texts(
            &page,
            "Chemistry",
            StudyLevel::Beginner,
            10,
            Vec::new(),
            Lang::En,
        )
        .await
        .unwrap();
    loop {
        let event = again.notification("study.event").await;
        if event["courseId"] == later.as_str() && event["course"]["status"] == "planned" {
            break;
        }
    }
}

/// Peintre simulé : prêt (ou refus), avancement, image retenue jusqu'au feu
/// vert ; note s'il a été lâché (arrêt).
#[derive(Default)]
struct FakePainter {
    refusal: Mutex<Option<crate::chat::images::PaintRefusal>>,
    gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    requests: Mutex<Vec<crate::chat::images::PaintRequest>>,
    dropped: std::sync::atomic::AtomicBool,
}

struct DropFlag<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for DropFlag<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// PNG de 1 × 1 (en-tête seulement : il n'est jamais décodé ici).
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake";

impl crate::chat::images::Painter for FakePainter {
    fn ready(
        &self,
    ) -> crate::chat::images::PaintFuture<
        '_,
        Result<crate::chat::images::Ready, crate::chat::images::PaintRefusal>,
    > {
        let refusal = self.refusal.lock().clone();
        Box::pin(async move {
            match refusal {
                Some(refusal) => Err(refusal),
                None => Ok(crate::chat::images::Ready {
                    model_name: "FLUX.2 klein 4B".to_owned(),
                    default_side: 1024,
                }),
            }
        })
    }

    fn cpu(&self) -> crate::chat::images::PaintFuture<'_, bool> {
        Box::pin(async { false })
    }

    fn paint<'a>(
        &'a self,
        request: crate::chat::images::PaintRequest,
        progress: &'a mut (dyn FnMut(crate::chat::images::ImageProgress) + Send),
    ) -> crate::chat::images::PaintFuture<'a, Result<crate::chat::images::Painted, String>> {
        use crate::chat::images::{ImageProgress, ImageStage, Painted};
        let gate = self.gate.lock().clone();
        Box::pin(async move {
            let _flag = DropFlag(&self.dropped);
            let (width, height) = (request.width, request.height);
            self.requests.lock().push(request);
            progress(ImageProgress::at(
                ImageStage::Starting,
                false,
                width,
                height,
            ));
            progress(ImageProgress {
                step: Some(1),
                total: Some(4),
                ..ImageProgress::at(ImageStage::Generating, false, width, height)
            });
            if let Some(gate) = gate {
                gate.notified().await;
            }
            Ok(Painted {
                png: PNG.to_vec(),
                width,
                height,
                seed: 42,
                model_name: "FLUX.2 klein 4B".to_owned(),
            })
        })
    }
}

/// Fichiers PNG écrits sous `dir`.
fn pngs_under(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(pngs_under(&path));
        } else if std::fs::read(&path).is_ok_and(|bytes| bytes.starts_with(b"\x89PNG")) {
            found.push(path);
        }
    }
    found
}

#[tokio::test]
async fn an_image_comes_back_as_bytes_and_is_never_written() {
    use base64::Engine as _;
    let bench = bench().await;
    let (mut client, _, _) = bench.paired().await;
    // Aucun moteur d'images : refus -32036.
    let answer = client
        .request("image.generate", json!({ "prompt": "a red kitten" }))
        .await;
    assert_eq!(error_code(&answer), code::NO_IMAGE_MODEL);
    assert_eq!(answer["error"]["data"]["reason"], "unavailable");

    let painter = Arc::new(FakePainter::default());
    bench
        .host
        .engine
        .set_painter(Arc::clone(&painter) as Arc<dyn crate::chat::images::Painter>);
    *painter.refusal.lock() = Some(crate::chat::images::PaintRefusal::NoModel);
    let answer = client
        .request("image.generate", json!({ "prompt": "a red kitten" }))
        .await;
    assert_eq!(error_code(&answer), code::NO_IMAGE_MODEL);
    assert_eq!(answer["error"]["data"]["reason"], "noModel");
    assert!(answer["error"]["data"]["message"].is_string());
    *painter.refusal.lock() = None;

    for bad in [
        json!({ "prompt": "  " }),
        json!({ "prompt": "x", "model": "other" }),
        json!({ "prompt": "x".repeat(5000) }),
    ] {
        let answer = client.request("image.generate", bad).await;
        assert_eq!(error_code(&answer), code::INVALID_PARAMS, "{answer}");
    }

    let gate = Arc::new(tokio::sync::Notify::new());
    *painter.gate.lock() = Some(Arc::clone(&gate));
    client
        .send(
            &json!({ "jsonrpc": "2.0", "id": 900, "method": "image.generate",
            "params": { "prompt": "a red kitten", "width": 1000 } }),
        )
        .await;
    let event = client.notification("image.event").await;
    assert_eq!(event["requestId"], 900);
    assert_eq!(event["progress"]["stage"], "starting");
    // Pendant l'image, l'autocomplétion attend (compteur d'activité).
    wait_for("image painting", || !painter.requests.lock().is_empty()).await;
    assert!(
        bench.host.engine.is_working(),
        "the image holds the activity"
    );
    gate.notify_one();
    let answer = client.response(900).await;
    let result = &answer["result"];
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(result["pngBase64"].as_str().unwrap())
            .unwrap(),
        PNG
    );
    assert_eq!(result["seed"], 42);
    assert_eq!(result["model"], "FLUX.2 klein 4B");
    // Côtés arrondis comme ceux de l'outil ; l'autre prend celui du modèle.
    assert_eq!(
        (result["width"].clone(), result["height"].clone()),
        (json!(1008), json!(1024))
    );
    assert!(!bench.host.engine.is_working(), "released after the image");
    assert!(
        pngs_under(bench.dir.path()).is_empty(),
        "the app writes no image file"
    );

    // Arrêt par l'éditeur : l'image est lâchée.
    let gate = Arc::new(tokio::sync::Notify::new());
    *painter.gate.lock() = Some(gate);
    painter.dropped.store(false, Ordering::SeqCst);
    client
        .send(
            &json!({ "jsonrpc": "2.0", "id": 901, "method": "image.generate",
            "params": { "prompt": "a red kitten" } }),
        )
        .await;
    // L'image 901 a commencé (et non un avancement resté de la 900) : sinon
    // l'arrêt arriverait avant le dessin, qui ne serait jamais « lâché ».
    loop {
        let event = client.notification("image.event").await;
        if event["requestId"] == 901 {
            break;
        }
    }
    wait_for("image 901 painting", || painter.requests.lock().len() == 2).await;
    client
        .send(&json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": 901 } }))
        .await;
    let answer = client.response(901).await;
    assert_eq!(error_code(&answer), code::CANCELLED);
    wait_for("image dropped", || painter.dropped.load(Ordering::SeqCst)).await;
    wait_for("activity released", || !bench.host.engine.is_working()).await;
}

#[tokio::test]
async fn the_editor_proofreads_a_text_part_by_part() {
    let bench = bench().await;
    bench.select_model("chatonlm-proofread").await;
    let (mut client, _, _) = bench.paired().await;
    let text = "I recieve teh letter.\n\nThe second paragraph.";
    let answer = client
        .request(
            "proofread.run",
            json!({ "text": text, "mode": "correct", "language": "en" }),
        )
        .await;
    assert_eq!(
        answer["result"],
        json!({ "text": "I receive the letter.\n\nThe second paragraph.", "model": "chatonlm-proofread" }),
        "{answer}"
    );
    let started = client.notification("proofread.event").await;
    assert_eq!(started["event"]["event"], "started");
    let first = client.notification("proofread.event").await;
    assert_eq!(first["event"]["event"], "partDone");
    assert_eq!(first["event"]["result"]["text"], "I receive the letter.");
    let second = client.notification("proofread.event").await;
    assert_eq!(second["event"]["index"], 1);

    let answer = client
        .request("proofread.run", json!({ "text": "  ", "mode": "correct" }))
        .await;
    assert_eq!(error_code(&answer), code::REFUSED);
    assert_eq!(answer["error"]["data"]["code"], "empty");
    let answer = client
        .request("proofread.run", json!({ "text": "x", "mode": "translate" }))
        .await;
    assert_eq!(error_code(&answer), code::INVALID_PARAMS);

    // Arrêt par l'éditeur pendant un passage ; l'activité tenue est lâchée.
    bench.select_model("chatonlm-proofread-slow").await;
    client
        .send(&json!({ "jsonrpc": "2.0", "id": 700, "method": "proofread.run",
            "params": { "text": "I recieve teh letter, slowly, word after word.", "mode": "rephrase" } }))
        .await;
    let _ = client.notification("proofread.event").await;
    assert!(
        bench.host.engine.is_working(),
        "proofreading holds the activity"
    );
    client
        .send(&json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": 700 } }))
        .await;
    let answer = client.response(700).await;
    assert_eq!(error_code(&answer), code::CANCELLED);
    wait_for("activity released", || !bench.host.engine.is_working()).await;
}

/// « Auto » ne retombe jamais sur un modèle du chat qui tourne hors de la
/// machine (cloud, même activé, ou distant) : refus, et rien ne part ;
/// choisi sur la page Proofread, il est utilisé.
#[tokio::test]
async fn auto_never_sends_the_text_to_a_cloud_or_remote_chat_model() {
    let bench = bench().await;
    let patch = SettingsPatch {
        cloud_models: Some(true),
        ..Default::default()
    };
    bench
        .store
        .call(move |c| settings::update(c, patch, None).map(drop))
        .await
        .unwrap();
    let (mut client, _, _) = bench.paired().await;
    for model in ["gpt-oss:20b-cloud", "chatonlm-polish-remote"] {
        bench.select_model(model).await;
        let answer = client
            .request(
                "proofread.run",
                json!({ "text": "My password is hunter2.", "mode": "correct" }),
            )
            .await;
        assert_eq!(error_code(&answer), code::REFUSED, "{answer}");
        assert_eq!(answer["error"]["data"]["code"], "noModel", "{answer}");
        assert_eq!(
            answer["error"]["data"]["message"],
            format!(
                "The chat model ({model}) runs outside this computer. To proofread with it, \
                 choose it on the Proofread page of ChatonLM."
            ),
        );
    }
    assert!(
        bench._ollama.chat_requests().is_empty(),
        "{:?}",
        bench._ollama.chat_requests()
    );
    // Choisi sur la page Proofread : il est utilisé.
    let patch = SettingsPatch {
        proofread_model: Some("chatonlm-polish-remote".to_owned()),
        ..Default::default()
    };
    bench
        .store
        .call(move |c| settings::update(c, patch, None).map(drop))
        .await
        .unwrap();
    let _ = client
        .request(
            "proofread.run",
            json!({ "text": "I recieve teh letter.", "mode": "correct" }),
        )
        .await;
    assert!(
        bench
            ._ollama
            .chat_requests()
            .iter()
            .any(|sent| sent["model"] == "chatonlm-polish-remote")
    );
}

#[test]
fn only_listed_pages_can_be_shown() {
    use super::work::{Shown, shown_route};
    let id = "019a3b2c-1d2e-7f30-8a4b-5c6d7e8f9a0b";
    assert_eq!(shown_route("/hub/images"), Some(Shown::Page));
    assert_eq!(shown_route("/settings?section=easyLink"), Some(Shown::Page));
    assert_eq!(shown_route("/settings?section=vscode"), Some(Shown::Page));
    assert_eq!(
        shown_route(&format!("/study/{id}")),
        Some(Shown::Study(id.to_owned()))
    );
    assert_eq!(
        shown_route(&format!("/c/{id}")),
        Some(Shown::Chat(id.to_owned()))
    );
    for refused in [
        "",
        "/",
        "/settings",
        "/settings?section=general",
        "/settings?section=vscode&x=1",
        "/hub/images/",
        "/hub/images/a/b",
        "/c/new",
        "/study/../settings",
        "https://example.com",
        &format!("/c/{id}/x"),
        &format!("/study/{}", id.to_uppercase()),
        &format!("/study/{}", id.replace('-', "")),
    ] {
        assert_eq!(shown_route(refused), None, "{refused}");
    }
}

#[tokio::test]
async fn the_editor_shows_the_app_on_a_listed_page_only() {
    let bench = bench().await;
    bench.select_model("chatonlm-study").await;
    let (mut client, _, _) = bench.paired().await;
    let answer = client.request("app.show", json!({})).await;
    assert_eq!(answer["result"], json!({}), "{answer}");
    let answer = client
        .request("app.show", json!({ "route": "/settings?section=vscode" }))
        .await;
    assert_eq!(answer["result"], json!({}), "{answer}");
    // Une conversation ou un cours qui n'existe pas : rien ne s'ouvre.
    let unknown = "019a3b2c-1d2e-7f30-8a4b-5c6d7e8f9a0b";
    for route in [
        format!("/c/{unknown}"),
        format!("/study/{unknown}"),
        "/settings?section=general".to_owned(),
        "/proofread".to_owned(),
    ] {
        let answer = client.request("app.show", json!({ "route": route })).await;
        assert_eq!(error_code(&answer), code::INVALID_PARAMS, "{route}");
    }
    let answer = client
        .request("app.show", json!({ "route": "/hub/images", "focus": true }))
        .await;
    assert_eq!(error_code(&answer), code::INVALID_PARAMS);
    let created = client
        .request(
            "study.create",
            json!({ "topic": "Rust", "level": "beginner", "duration": 10 }),
        )
        .await;
    let course = created["result"]["courseId"].as_str().unwrap().to_owned();
    let route = format!("/study/{course}");
    let answer = client.request("app.show", json!({ "route": route })).await;
    assert_eq!(answer["result"], json!({}), "{answer}");
    assert_eq!(
        *bench.host.shown.lock(),
        vec![
            None,
            Some("/settings?section=vscode".to_owned()),
            Some(route),
        ]
    );
}
