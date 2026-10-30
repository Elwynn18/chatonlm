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
use super::*;
use crate::store::chats::{self, ChatSource};
use crate::store::settings::{self, SettingsPatch};

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
        shared.connections.lock().insert(
            id,
            Open {
                client: None,
                close,
            },
        );
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
