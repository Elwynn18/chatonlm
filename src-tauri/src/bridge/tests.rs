//! Essais du pont : service réel, base en mémoire, faux Ollama, connexions
//! par des flux en mémoire (les points d'accès du système ont leurs essais
//! dans `linux.rs` et `windows.rs`).

use std::sync::Arc;

use fake_ollama::{FakeOllama, Mode};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::io::{AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::sync::watch;

use super::connection::code;
use super::*;
use crate::store::settings::{self, SettingsPatch};

#[derive(Clone, Default)]
struct TestHost {
    engine: Arc<ChatEngine>,
    states: Arc<Mutex<Vec<BridgeState>>>,
    updated: Arc<Mutex<Vec<String>>>,
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
    }
}

struct Bench {
    bridge: Bridge<TestHost>,
    host: TestHost,
    store: Store,
    dir: tempfile::TempDir,
    _ollama: FakeOllama,
}

async fn bench() -> Bench {
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
        auth_timeout: std::time::Duration::from_millis(300),
    };
    let host = TestHost::default();
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
    let bench = bench().await;
    let mut client = bench.connect();
    client.initialize().await;
    let started = std::time::Instant::now();
    assert_eq!(client.next_message().await, None);
    assert!(started.elapsed() >= std::time::Duration::from_millis(250));
    // Authentifiée, elle reste ouverte au-delà du délai.
    let (mut client, _, _) = bench.paired().await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
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

#[test]
fn pipe_names_and_access_lists_are_well_formed() {
    assert_eq!(pipe_name(&[0xab, 0x01]), r"\\.\pipe\chatonlm-ab01");
    assert_eq!(pipe_sddl("S-1-5-21-1"), "D:P(A;;GA;;;S-1-5-21-1)");
}
