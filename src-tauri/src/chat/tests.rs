//! Essais du chat : sur le faux moteur (`FakeBench` : faux `llama-server`,
//! modèles de la base locale par scénario, lot L5a), contrats sur le vrai
//! (`RealBench`), et séquences d'événements du simulateur de la page.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use fake_llama::scenarios::l5a::chat as scripted;
use serde_json::{Value, json};
use tauri::ipc::InvokeResponseBody;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::attachments::tests as attachment_tests;
use crate::attachments::{StageResult, samples};
use crate::llm::testing::{FakeBench, RealBench, Role as RealRole};
use crate::store::settings::SettingsPatch;

/// Attente maximale d'un événement ou d'un état, sur le faux moteur : un
/// garde-fou contre un blocage, jamais une mesure de vitesse. Large : un
/// envoi lance un processus (faux `llama-server`), plus lent sous la charge
/// du contrôle complet.
const WAIT: Duration = Duration::from_secs(30);

// Modèles des bancs : identifiant de la base locale et scénario du faux
// (fichier `chatonlm-<scénario>.gguf` ; capacités d'après le nom).
const THINKER: &str = "Qwen/Qwen3-0.6B-GGUF:Q8_0";
const PLAIN: &str = "chat/Plain-GGUF:Q4_K_M";
const SEER: &str = "Qwen/Qwen3.5-0.8B-GGUF:Q4_K_M";
const SLOW: &str = "chat/Slow-GGUF:Q4_K_M";
const SLOW_TOO: &str = "chat/Slowtoo-GGUF:Q4_K_M";
const SLOWLOAD: &str = "chat/Slowload-GGUF:Q4_K_M";
const LONGLOAD: &str = "chat/Longload-GGUF:Q4_K_M";
const LOADFAIL: &str = "chat/Loadfail-GGUF:Q4_K_M";
const HANG: &str = "chat/Hang-GGUF:Q4_K_M";
const CRASH: &str = "chat/Crash-GGUF:Q4_K_M";
const FAILING: &str = "chat/Error-GGUF:Q4_K_M";
const LOOPING: &str = "chat/Loop-GGUF:Q4_K_M";
/// Raisonnement qui boucle par paragraphes entiers (séquence du simulateur).
const MULLING: &str = "chat/Mull-GGUF:Q4_K_M";
const LONG: &str = "chat/Long-GGUF:Q4_K_M";
const STUBBORN: &str = "chat/Stubborn-GGUF:Q4_K_M";
const TOOLED: &str = "chat/Tooled-GGUF:Q4_K_M";
const QUIET: &str = "chat/Quiet-GGUF:Q4_K_M";
const INSIST: &str = "chat/Insist-GGUF:Q4_K_M";
const ROGUE: &str = "chat/Rogue-GGUF:Q4_K_M";
const SKETCH: &str = "chat/Sketch-GGUF:Q4_K_M";
const GIST: &str = "chat/Gist-GGUF:Q4_K_M";
const PENSIVE: &str = "chat/Pensive-GGUF:Q4_K_M";
const IMPORTED: &str = "@ollama/llama3.2:latest";
/// Modèle qui cherche sur le web (scénario `chatonlm-websearch` du lot L7).
const SEARCHER: &str = "chat/Websearch-GGUF:Q4_K_M";
/// Modèle qui lit des pages (`web_fetch`, scénario `chatonlm-fetch-tools`).
const FETCHER: &str = "chat/Fetcher-GGUF:Q4_K_M";

/// Modèles et scénarios du faux.
fn scenario(id: &str) -> &'static str {
    match id {
        THINKER => "think",
        PLAIN => "simple",
        SEER => "vision",
        SLOW | SLOW_TOO => "slow",
        SLOWLOAD => "slowload",
        LONGLOAD => "longload",
        LOADFAIL => "loadfail",
        HANG => "hang",
        CRASH => "crash",
        FAILING => "error",
        LOOPING => "loop",
        MULLING => "mull-think",
        LONG => "long",
        STUBBORN => "stubborn-think",
        TOOLED => "tools",
        QUIET => "quiet-tools",
        INSIST => "insist-tools",
        ROGUE => "rogue",
        SKETCH => "sketch-think-tools",
        GIST => "gist",
        PENSIVE => "pensive-think",
        IMPORTED => "simple",
        SEARCHER => "websearch-tools",
        FETCHER => "fetch-tools",
        other => panic!("no scenario for {other}"),
    }
}

/// Banc d'un envoi : faux moteur avec ces modèles, moteur de chat (peintre
/// au choix), zone de préparation.
struct Bench {
    fake: FakeBench,
    /// Accès des envois : celui du banc, ou le même moteur avec une file de
    /// téléchargements sur un faux Hugging Face ([`Bench::downloading`]).
    access: Access,
    engine: Arc<ChatEngine>,
    staging: Arc<Staging>,
}

impl Bench {
    fn new(models: &[&str]) -> Self {
        Self::painting(models, None)
    }

    fn painting(models: &[&str], painter: Option<Arc<FakePainter>>) -> Self {
        let models: Vec<(&str, &str)> = models.iter().map(|id| (*id, scenario(id))).collect();
        let engine = ChatEngine::default();
        if let Some(painter) = painter {
            engine.set_painter(painter);
        }
        let fake = FakeBench::new(&models);
        Bench {
            access: fake.access.clone(),
            fake,
            engine: Arc::new(engine),
            staging: Arc::new(attachment_tests::staging()),
        }
    }

    /// Banc dont les téléchargements passent par `hf` (faux Hugging Face).
    fn downloading(models: &[&str], hf: &fake_services::hf::FakeHf) -> Self {
        let mut bench = Self::new(models);
        let (store, data_dir) = (
            bench.store().clone(),
            bench.fake.access.data_dir().to_owned(),
        );
        let downloads = crate::hub::downloads::HubDownloads::with_options(
            |_| {},
            crate::hub::downloads::Options {
                hf_url: Some(hf.url()),
                ..Default::default()
            },
        );
        downloads.attach(store.clone(), data_dir.clone());
        let relay = Arc::new(crate::relay::RelayClient::with_url(
            "http://127.0.0.1:9".into(),
            data_dir.clone(),
        ));
        bench.access = Access::new(
            bench.fake.engine().clone(),
            store,
            data_dir,
            Arc::new(crate::llm::remote::Unavailable),
            relay,
            downloads,
        );
        bench
    }

    /// Banc dont la recherche web passe par `relay` (faux relais, ou relais
    /// fermé).
    fn searching(models: &[&str], relay: Arc<crate::relay::RelayClient>) -> Self {
        let mut bench = Self::new(models);
        bench.access = Access::new(
            bench.fake.engine().clone(),
            bench.store().clone(),
            bench.fake.access.data_dir().to_owned(),
            Arc::new(crate::llm::remote::Unavailable),
            relay,
            bench.fake.access.downloads().clone(),
        );
        bench
    }

    fn store(&self) -> &Store {
        self.fake.store()
    }

    async fn server(&self) -> Server {
        Server::current(&self.access).await.unwrap()
    }

    fn set(&self, patch: SettingsPatch) {
        self.fake.set(patch);
    }

    /// Envoi complet, borné par [`WAIT`] : événements et résultat.
    async fn send_result(&self, request: SendRequest) -> (Recorder, AppResult<()>) {
        let (channel, recorder) = recorder();
        let server = self.server().await;
        let sending = send(
            self.store(),
            &self.engine,
            &self.staging,
            server,
            request,
            &channel,
        );
        let result = tokio::time::timeout(WAIT, sending)
            .await
            .unwrap_or_else(|_| panic!("still sending after {WAIT:?}: {:?}", recorder.events()));
        (recorder, result)
    }

    /// Envoi complet accepté : exactement un événement final.
    async fn send(&self, request: SendRequest) -> Recorder {
        let (recorder, result) = self.send_result(request).await;
        result.unwrap();
        assert_one_final(&recorder.events());
        recorder
    }

    /// Envoi lancé à part (à arrêter par le moteur de chat).
    async fn spawn(
        &self,
        request: SendRequest,
    ) -> (tokio::task::JoinHandle<AppResult<()>>, Recorder) {
        let (channel, recorder) = recorder();
        let server = self.server().await;
        let (store, engine, staging) = (
            self.store().clone(),
            self.engine.clone(),
            self.staging.clone(),
        );
        let task = tokio::spawn(async move {
            send(&store, &engine, &staging, server, request, &channel).await
        });
        (task, recorder)
    }

    /// Requêtes de chat (corps) reçues par le processus de `id`, ouvert s'il
    /// ne l'est plus (même processus avec les mêmes réglages).
    async fn requests(&self, id: &str) -> Vec<Value> {
        let session = self
            .access
            .open(
                id,
                OpenOptions::default(),
                &CancellationToken::new(),
                &|| {},
            )
            .await
            .unwrap();
        requests_of(&session).await
    }

    /// Arguments du processus de `id` (chargé).
    async fn args(&self, id: &str) -> Vec<String> {
        let session = self
            .access
            .open(
                id,
                OpenOptions::default(),
                &CancellationToken::new(),
                &|| {},
            )
            .await
            .unwrap();
        pilot(&session, "args").await
    }

    async fn shutdown(&self) {
        self.fake.shutdown().await;
    }
}

/// Route de pilotage du faux (`/fake/<route>`) d'une session.
async fn pilot<T: serde::de::DeserializeOwned>(session: &Session, route: &str) -> T {
    let url = format!("{}/fake/{route}", session.endpoint.base_url);
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(url)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// Corps des requêtes de chat reçues par le processus d'une session.
async fn requests_of(session: &Session) -> Vec<Value> {
    let all: Vec<Value> = pilot(session, "requests").await;
    all.into_iter()
        .filter(|request| request["path"] == "/v1/chat/completions")
        .map(|request| request["body"].clone())
        .collect()
}

/// État courant en retard sur la base (lecture entre une écriture et sa
/// mise à jour) : appels, résultats et fin du raisonnement de la base
/// gardés ; le texte vient toujours de l'état courant.
#[test]
fn overlay_keeps_what_the_database_already_has() {
    let engine = ChatEngine::default();
    let done = ToolCall {
        result: Some(r#"{"results":[]}"#.into()),
        ..ToolCall::new("web_search", r#"{"query":"a"}"#)
    };
    let pending = ToolCall::new("web_fetch", r#"{"url":"https://example.com"}"#);
    let saved = Message {
        id: 7,
        role: chats::Role::Assistant,
        content: "old".into(),
        thinking: "Thinking".into(),
        model: None,
        created_at: 0,
        updated_at: 0,
        thinking_started_at: Some(1),
        thinking_ended_at: Some(2),
        attachments: Vec::new(),
        tool_calls: vec![done.clone(), pending.clone()],
    };
    let mut chat = Chat {
        id: "chat".into(),
        title: String::new(),
        created_at: 0,
        updated_at: 0,
        messages: vec![saved],
        context: None,
        compactions: Vec::new(),
        reply_running: None,
        ephemeral: false,
    };
    let live = Reply {
        content: "new".into(),
        thinking: "Thinking".into(),
        thinking_started_at: Some(1),
        thinking_ended_at: None,
        tool_calls: vec![ToolCall::new("web_search", r#"{"query":"a"}"#)],
    };
    engine.update_live("chat", 7, &live);
    engine.overlay(&mut chat);
    let message = &chat.messages[0];
    assert_eq!(message.content, "new");
    assert_eq!(message.thinking_ended_at, Some(2));
    assert_eq!(message.tool_calls, vec![done, pending]);
}

/// Réponse en cours d'un bot d'Easy Link : désignée à la page
/// (`replyRunning`), qui n'a pas de flux pour elle ; pas celle de la page,
/// ni rien une fois l'envoi fini.
#[test]
fn overlay_points_the_page_to_a_remote_reply_only() {
    let chat = || Chat {
        id: "chat".into(),
        title: String::new(),
        created_at: 0,
        updated_at: 0,
        messages: vec![Message {
            id: 7,
            role: chats::Role::Assistant,
            content: String::new(),
            thinking: String::new(),
            model: None,
            created_at: 0,
            updated_at: 0,
            thinking_started_at: None,
            thinking_ended_at: None,
            attachments: Vec::new(),
            tool_calls: Vec::new(),
        }],
        context: None,
        compactions: Vec::new(),
        reply_running: None,
        ephemeral: false,
    };
    let live = Reply {
        content: "Hi".into(),
        ..Reply::default()
    };
    for (origin, expected) in [
        (Origin::Remote, Some(7)),
        (Origin::Bridge, Some(7)),
        (Origin::Page, None),
    ] {
        let engine = ChatEngine::default();
        let (_cancel, _finish) = engine.begin("chat", "stream", origin).unwrap();
        engine.update_live("chat", 7, &live);
        let mut read = chat();
        engine.overlay(&mut read);
        assert_eq!(read.messages[0].content, "Hi", "{origin:?}");
        assert_eq!(read.reply_running, expected, "{origin:?}");
        engine.end("chat");
        let mut read = chat();
        engine.overlay(&mut read);
        assert_eq!(read.reply_running, None, "{origin:?}");
    }
}

/// Événements reçus par le canal, en JSON, avec leur heure de réception
/// (ms depuis l'époque, comme les horodatages en base).
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<(i64, Value)>>>);

fn recorder() -> (Channel<ChatEvent>, Recorder) {
    let recorder = Recorder::default();
    let sink = recorder.0.clone();
    let channel = Channel::new(move |body| {
        if let InvokeResponseBody::Json(text) = body {
            sink.lock()
                .push((now_ms(), serde_json::from_str(&text).unwrap()));
        }
        Ok(())
    });
    (channel, recorder)
}

impl Recorder {
    fn events(&self) -> Vec<Value> {
        self.0
            .lock()
            .iter()
            .map(|(_, event)| event.clone())
            .collect()
    }

    fn count(&self, kind: &str) -> usize {
        self.0
            .lock()
            .iter()
            .filter(|(_, e)| e["type"] == kind)
            .count()
    }

    /// Heures de réception des événements de ce type.
    fn received(&self, kind: &str) -> Vec<i64> {
        let events = self.0.lock();
        let of_kind = events.iter().filter(|(_, e)| e["type"] == kind);
        of_kind.map(|(at, _)| *at).collect()
    }

    /// Texte de réponse reçu, bout à bout.
    fn contents(&self) -> String {
        let events = self.0.lock();
        let contents = events.iter().filter(|(_, e)| e["type"] == "content");
        contents
            .map(|(_, e)| e["text"].as_str().unwrap().to_owned())
            .collect()
    }

    /// Conversation créée par l'envoi.
    fn chat_id(&self) -> String {
        let events = self.0.lock();
        events[0].1["chatId"].as_str().unwrap().to_owned()
    }

    /// Dernier événement (le final, une fois l'envoi fini).
    fn last(&self) -> Value {
        self.events().last().cloned().unwrap()
    }

    /// Attend au moins `count` événements de ce type, [`WAIT`] au plus.
    async fn wait_for(&self, kind: &str, count: usize) {
        let polling = async {
            while self.count(kind) < count {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        if tokio::time::timeout(WAIT, polling).await.is_err() {
            let events = self.events();
            panic!("still waiting after {WAIT:?} for {count} \"{kind}\" event(s): {events:?}");
        }
    }
}

/// Attend qu'une condition soit vraie ; échoue au-delà de `limit`.
async fn wait_until(limit: Duration, what: &str, ready: impl Fn() -> bool) {
    let polling = async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    if tokio::time::timeout(limit, polling).await.is_err() {
        panic!("still waiting after {limit:?} for {what}");
    }
}

/// Attend la fin d'un envoi lancé à part ; échoue au-delà de [`WAIT`] (un
/// flux sans fin qu'une annulation n'arrête plus ferait sinon pendre les
/// essais).
async fn finished(task: tokio::task::JoinHandle<AppResult<()>>) {
    match tokio::time::timeout(WAIT, task).await {
        Ok(result) => result.unwrap().unwrap(),
        Err(_) => panic!("the send is still running after {WAIT:?}"),
    }
}

fn is_final(event: &Value) -> bool {
    matches!(event["type"].as_str(), Some("done" | "error" | "cancelled"))
}

/// Exactement un événement final, en dernier (compté sur la liste complète,
/// sans le regroupement de `types`).
fn assert_one_final(events: &[Value]) {
    let finals = events.iter().filter(|event| is_final(event)).count();
    assert_eq!(finals, 1, "{events:?}");
    assert!(events.last().is_some_and(is_final), "{events:?}");
}

fn types(events: &[Value]) -> Vec<String> {
    let mut types: Vec<String> = events
        .iter()
        .map(|event| event["type"].as_str().unwrap().to_owned())
        .collect();
    types.dedup();
    types
}

fn of_type(events: &[Value], kind: &str) -> Vec<Value> {
    events
        .iter()
        .filter(|event| event["type"] == kind)
        .cloned()
        .collect()
}

fn request(chat_id: Option<&str>, model: &str, think: Option<Think>) -> SendRequest {
    SendRequest {
        chat_id: chat_id.map(str::to_owned),
        stream_id: format!("stream-{model}"),
        content: "Say hello in one word.".into(),
        model: model.into(),
        think,
        attachment_ids: None,
        edit_message_id: None,
        kept_attachment_ids: None,
        web_search: None,
        voice: None,
        image: None,
        ephemeral: None,
    }
}

fn asking(chat_id: Option<&str>, model: &str, content: &str) -> SendRequest {
    SendRequest {
        content: content.into(),
        ..request(chat_id, model, None)
    }
}

/// Conversation telle qu'enregistrée (sans l'état courant d'une réponse en
/// cours, que seul `overlay` ajoute).
fn saved(store: &Store, chat_id: &str) -> Chat {
    let id = chat_id.to_owned();
    store.blocking(move |c| chats::get(c, &id)).unwrap()
}

fn system(content: &str) -> Value {
    json!({ "role": "system", "content": content })
}

fn systems(body: &Value) -> usize {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "system")
        .count()
}

/// Réglage du pré-prompt.
fn set_prompt(bench: &Bench, text: &str) {
    bench.set(SettingsPatch {
        system_prompt: Some(text.into()),
        ..Default::default()
    });
}

/// Prépare un fichier ; renvoie son identifiant.
async fn stage(staging: &Staging, name: &str, data: &[u8]) -> String {
    match staging.stage_bytes(name.into(), data.to_vec()).await {
        StageResult::Staged { file } => file.id,
        StageResult::Rejected { error, .. } => panic!("{name} rejected: {error}"),
    }
}

/// Image envoyée dans les essais : 4 × 4 pixels rouges.
fn red_png() -> Vec<u8> {
    samples::png(4, 4, [255, 0, 0])
}

/// Contenu des fichiers préparés dans les séquences ; `None` pour un autre
/// nom (identifiant inconnu de la zone).
fn sample_file(name: &str) -> Option<Vec<u8>> {
    match name {
        "notes.txt" => Some(b"Line 1\nLine 2".to_vec()),
        "red.png" => Some(red_png()),
        "extra.md" => Some(b"# Extra\nMore notes".to_vec()),
        _ => None,
    }
}

#[tokio::test]
async fn the_observer_sees_every_event_of_a_reply() {
    let bench = Bench::new(&[THINKER]);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let chats = Arc::new(Mutex::new(Vec::new()));
    let (sink, chat_sink) = (seen.clone(), chats.clone());
    bench.engine.set_observer(move |chat, event, _| {
        sink.lock()
            .push(serde_json::to_value(event).unwrap()["type"].clone());
        chat_sink.lock().push(chat.to_owned());
    });
    let recorder = bench
        .send(request(None, THINKER, Some(Think::Enabled(true))))
        .await;
    let sent: Vec<Value> = recorder
        .events()
        .iter()
        .map(|event| event["type"].clone())
        .collect();
    assert!(sent.contains(&json!("content")));
    assert_eq!(*seen.lock(), sent);
    // Chaque événement avec sa conversation.
    let chat = recorder.chat_id();
    assert!(chats.lock().iter().all(|seen| *seen == chat));
    bench.shutdown().await;
}

/// Envoi vocal de l'extension VS Code (séance tenue par le pont) : observé
/// (la séance et la lecture le suivent), consigne de la séance après la
/// consigne vocale ; un envoi ordinaire du pont ne l'est pas.
#[tokio::test]
async fn a_bridge_voice_send_is_observed_with_its_session_instructions() {
    let bench = Bench::new(&[THINKER]);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    bench.engine.set_observer(move |_, _, voice| {
        sink.lock()
            .push(voice.and_then(|voice| voice.system.clone()));
    });
    let quiet = |_: ChatEvent| {};
    let typed = request(None, THINKER, Some(Think::Enabled(false)));
    send_to(
        bench.store(),
        &bench.engine,
        &bench.staging,
        bench.server().await,
        typed,
        Origin::Bridge,
        &quiet,
    )
    .await
    .unwrap();
    assert!(
        seen.lock().is_empty(),
        "an ordinary bridge send is not observed"
    );
    let session = "Ask one short question at a time.";
    let spoken = SendRequest {
        voice: Some(VoiceSend {
            language: None,
            system: Some(session.into()),
        }),
        ..request(None, THINKER, Some(Think::Enabled(false)))
    };
    send_to(
        bench.store(),
        &bench.engine,
        &bench.staging,
        bench.server().await,
        spoken,
        Origin::Bridge,
        &quiet,
    )
    .await
    .unwrap();
    let observed = seen.lock().clone();
    assert!(!observed.is_empty());
    assert!(
        observed
            .iter()
            .all(|system| system.as_deref() == Some(session))
    );
    let sent = bench.requests(THINKER).await;
    let default = crate::voice_mode::DEFAULT_INSTRUCTIONS;
    assert_eq!(
        sent[1]["messages"][0],
        system(&format!("{default}\n\n{session}"))
    );
    // Jamais enregistrée : un seul message système.
    assert_eq!(systems(&sent[1]), 1);
    bench.shutdown().await;
}

/// Envoi vocal : la consigne vocale en tête de chaque requête (tours
/// d'outils compris), jamais enregistrée ; l'observateur reçoit la demande
/// avec chaque événement.
#[tokio::test]
async fn a_voice_send_puts_the_voice_instructions_first() {
    let bench = Bench::painting(&[TOOLED, THINKER], Some(FakePainter::ready()));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    bench.engine.set_observer(move |_, _, voice| {
        sink.lock().push(voice.map(|voice| voice.language));
    });
    let spoken = VoiceSend {
        language: Some(VoiceLanguage::Fr),
        ..VoiceSend::default()
    };
    // Un envoi vocal n'a pas l'outil d'image (pas de tour d'outils) : la
    // consigne est vérifiée sur sa requête, puis sur les envois suivants.
    let ask = SendRequest {
        voice: Some(spoken.clone()),
        ..asking(None, TOOLED, "Draw a fox")
    };
    let first = bench.send(ask).await;
    assert_eq!(first.last()["type"], json!("done"));
    let sent = bench.requests(TOOLED).await;
    assert_eq!(sent.len(), 1);
    let default = crate::voice_mode::DEFAULT_INSTRUCTIONS;
    assert_eq!(sent[0]["messages"][0], system(default));
    // Pas d'outil d'image pour un envoi vocal.
    assert!(sent[0].get("tools").is_none(), "{}", sent[0]);
    assert!(
        seen.lock()
            .iter()
            .all(|language| *language == Some(Some(VoiceLanguage::Fr)))
    );

    // Consigne réglée ; l'historique relu n'a pas l'ancienne consigne.
    bench
        .store()
        .blocking(|connection| {
            let patch = crate::store::voice_mode::VoiceModeSettingsPatch {
                instructions: Some("Answer like a pirate.".into()),
                ..Default::default()
            };
            crate::store::voice_mode::update(connection, patch)
        })
        .unwrap();
    let chat_id = first.chat_id();
    let again = SendRequest {
        voice: Some(VoiceSend::default()),
        ..request(Some(&chat_id), THINKER, Some(Think::Enabled(false)))
    };
    bench.send(again).await;
    let sent = bench.requests(THINKER).await;
    assert_eq!(sent[0]["messages"][0], system("Answer like a pirate."));
    assert_eq!(systems(&sent[0]), 1);

    // Envoi ordinaire : aucune consigne ; rien d'enregistré.
    seen.lock().clear();
    let typed = request(Some(&chat_id), THINKER, Some(Think::Enabled(false)));
    bench.send(typed).await;
    let sent = bench.requests(THINKER).await;
    assert_eq!(systems(&sent[1]), 0);
    assert!(seen.lock().iter().all(Option::is_none));
    let chat = saved(bench.store(), &chat_id);
    assert!(chat.messages.iter().all(|message| {
        !message.content.contains("pirate") && !message.content.contains("spoken conversation")
    }));
    bench.shutdown().await;
}

/// Pré-prompt réglé : en tête de chaque requête (tours d'outils compris),
/// suivi de la consigne vocale pour un envoi vocal, aussi pour un bot
/// d'Easy Link ; jamais enregistré ; fait d'espaces, rien n'est envoyé.
#[tokio::test]
async fn the_system_prompt_leads_every_request() {
    let bench = Bench::painting(&[TOOLED, THINKER], Some(FakePainter::ready()));
    set_prompt(&bench, "Always answer in French.");

    // Avec un tour d'outils (image) : deux requêtes, le pré-prompt en tête
    // des deux.
    let first = bench.send(asking(None, TOOLED, "Draw a fox")).await;
    assert_eq!(first.last()["type"], json!("done"));
    let sent = bench.requests(TOOLED).await;
    assert_eq!(sent.len(), 2);
    for body in &sent {
        assert_eq!(body["messages"][0], system("Always answer in French."));
        assert_eq!(systems(body), 1);
    }

    // Envoi vocal : le pré-prompt, puis la consigne vocale.
    let chat_id = first.chat_id();
    let spoken = SendRequest {
        voice: Some(VoiceSend::default()),
        ..request(Some(&chat_id), THINKER, Some(Think::Enabled(false)))
    };
    bench.send(spoken).await;
    let default = crate::voice_mode::DEFAULT_INSTRUCTIONS;
    assert_eq!(
        bench.requests(THINKER).await[0]["messages"][0],
        system(&format!("Always answer in French.\n\n{default}"))
    );

    // Bot d'Easy Link : même pré-prompt.
    let remote = request(Some(&chat_id), THINKER, Some(Think::Enabled(false)));
    let events = Mutex::new(Vec::new());
    let sink = |event: ChatEvent| events.lock().push(event);
    send_to(
        bench.store(),
        &bench.engine,
        &bench.staging,
        bench.server().await,
        remote,
        Origin::Remote,
        &sink,
    )
    .await
    .unwrap();
    assert!(matches!(events.lock().last(), Some(ChatEvent::Done { .. })));
    assert_eq!(
        bench.requests(THINKER).await[1]["messages"][0],
        system("Always answer in French.")
    );

    // Fait d'espaces : aucun message système.
    set_prompt(&bench, " \n ");
    bench
        .send(request(
            Some(&chat_id),
            THINKER,
            Some(Think::Enabled(false)),
        ))
        .await;
    assert_eq!(systems(&bench.requests(THINKER).await[2]), 0);
    let chat = saved(bench.store(), &chat_id);
    assert!(
        chat.messages
            .iter()
            .all(|message| !message.content.contains("French"))
    );
    bench.shutdown().await;
}

/// Consigne reprise d'un modèle d'Ollama (`SYSTEM` du Modelfile) : en tête
/// d'une requête sans message système ; le pré-prompt la remplace (règle
/// d'Ollama).
#[tokio::test]
async fn an_imported_model_keeps_its_system_unless_a_prompt_is_set() {
    let bench = Bench::new(&[IMPORTED]);
    let defaults = crate::store::model_import::ModelDefaults {
        system: "You are Mario.".into(),
        ..Default::default()
    };
    bench
        .store()
        .blocking(move |c| crate::store::model_import::set_defaults(c, IMPORTED, &defaults))
        .unwrap();
    let first = bench.send(request(None, IMPORTED, None)).await;
    let sent = bench.requests(IMPORTED).await;
    assert_eq!(sent[0]["messages"][0], system("You are Mario."));
    set_prompt(&bench, "Always answer in French.");
    bench
        .send(request(Some(&first.chat_id()), IMPORTED, None))
        .await;
    let sent = bench.requests(IMPORTED).await;
    assert_eq!(sent[1]["messages"][0], system("Always answer in French."));
    assert_eq!(systems(&sent[1]), 1);
    bench.shutdown().await;
}

#[tokio::test]
async fn answers_with_thinking_then_content() {
    let bench = Bench::new(&[THINKER]);
    let recorder = bench
        .send(request(None, THINKER, Some(Think::Enabled(true))))
        .await;
    let events = recorder.events();
    // Modèle pas encore en mémoire : chargé d'abord.
    assert_eq!(
        types(&events),
        [
            "chatCreated",
            "userMessage",
            "loading",
            "loaded",
            "replyStarted",
            "thinking",
            "content",
            "context",
            "done"
        ]
    );
    let chat = saved(bench.store(), &recorder.chat_id());
    assert_eq!(chat.messages.len(), 2);
    let reply = &chat.messages[1];
    assert_eq!(reply.content, "Hello! You said: Say hello in one word.");
    assert!(reply.thinking.starts_with("Let me think"));
    assert_eq!(reply.model.as_deref(), Some(THINKER));
    let (started, ended) = (
        reply.thinking_started_at.unwrap(),
        reply.thinking_ended_at.unwrap(),
    );
    assert!(started <= ended);
    assert_eq!(events.last().unwrap()["messageId"], json!(reply.id));

    let sent = &bench.requests(THINKER).await[0];
    assert_eq!(
        sent["chat_template_kwargs"],
        json!({ "enable_thinking": true })
    );
    assert_eq!(sent["stream"], json!(true));
    assert_eq!(
        sent["messages"],
        json!([{"role": "user", "content": "Say hello in one word."}])
    );
    // Local : ni nom de modèle (le processus n'en sert qu'un), ni options
    // propres à Ollama.
    assert!(sent.get("model").is_none(), "{sent}");
    assert!(sent.get("options").is_none() && sent.get("keep_alive").is_none());
    // La réponse est limitée à ce qui reste du contexte (propre à ChatonLM).
    assert!(sent["max_tokens"].as_u64().is_some(), "{sent}");
    bench.shutdown().await;
}

/// Un modèle qui raisonne malgré `enable_thinking: false` (gabarit qui
/// ouvre toujours le raisonnement) est retenu ; pas celui qui obéit, ni un
/// envoi avec la réflexion demandée. Le raisonnement reçu est affiché comme
/// d'habitude.
#[tokio::test]
async fn a_model_that_thinks_despite_think_false_is_remembered() {
    let bench = Bench::new(&[STUBBORN, THINKER]);
    let always = |model: &'static str| {
        bench
            .store()
            .blocking(move |c| model_prefs::always_thinks(c, model))
            .unwrap()
    };
    for (model, think) in [
        (STUBBORN, Think::Enabled(true)),
        (THINKER, Think::Enabled(false)),
    ] {
        bench.send(request(None, model, Some(think))).await;
        assert!(!always(model), "{model}");
    }
    let events = bench
        .send(request(None, STUBBORN, Some(Think::Enabled(false))))
        .await
        .events();
    assert!(events.iter().any(|event| event["type"] == "thinking"));
    assert!(always(STUBBORN));
    bench.shutdown().await;
}

/// Envoi éphémère : la conversation créée l'est, hors de l'historique ; la
/// demande est ignorée pour une conversation existante (une conversation
/// ordinaire ne devient jamais éphémère).
#[tokio::test]
async fn an_ephemeral_send_creates_an_ephemeral_chat_only() {
    let bench = Bench::new(&[PLAIN]);
    let store = bench.store();
    let ephemeral = |chat_id: Option<&str>| SendRequest {
        ephemeral: Some(true),
        ..request(chat_id, PLAIN, None)
    };
    let created = bench.send(ephemeral(None)).await;
    let chat = created.chat_id();
    assert!(
        store
            .blocking(move |c| chats::is_ephemeral(c, &chat))
            .unwrap()
    );
    assert!(store.blocking(|c| chats::list(c)).unwrap().is_empty());

    let ordinary = bench.send(request(None, PLAIN, None)).await.chat_id();
    bench
        .send(SendRequest {
            stream_id: "again".into(),
            ..ephemeral(Some(&ordinary))
        })
        .await;
    let chat = ordinary.clone();
    assert!(
        !store
            .blocking(move |c| chats::is_ephemeral(c, &chat))
            .unwrap()
    );
    let listed: Vec<String> = store
        .blocking(|c| chats::list(c))
        .unwrap()
        .into_iter()
        .map(|chat| chat.id)
        .collect();
    assert_eq!(listed, vec![ordinary]);
    bench.shutdown().await;
}

/// Session de l'Étude (contexte imposé, plus grand que celui réglé) tenue sur
/// le modèle du chat : un envoi la reprend aussitôt, sans chargement ni
/// relance (relecture L5a, B1 : il attendait tout le palier).
#[tokio::test(flavor = "multi_thread")]
async fn a_send_shares_the_model_a_study_holds_with_a_larger_context() {
    let bench = Bench::new(&[PLAIN]);
    bench.set(SettingsPatch {
        context_length: Some(4096),
        ..Default::default()
    });
    let study = bench
        .access
        .open(
            PLAIN,
            OpenOptions {
                ctx: Some(8192),
                ..Default::default()
            },
            &CancellationToken::new(),
            &|| {},
        )
        .await
        .unwrap();
    let pid = bench.fake.engine().pid_of(PLAIN);
    let (recorder, result) = tokio::time::timeout(
        Duration::from_secs(10),
        bench.send_result(request(None, PLAIN, None)),
    )
    .await
    .expect("the send waited for the study");
    result.unwrap();
    assert_eq!(recorder.count("loading"), 0, "{:?}", recorder.events());
    assert_eq!(recorder.last()["type"], "done");
    assert_eq!(bench.fake.engine().pid_of(PLAIN), pid);
    let running = bench.fake.engine().running();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].context_length, 8192);
    let context = of_type(&recorder.events(), "context");
    assert_eq!(context[0]["size"], json!(8192));
    drop(study);
    bench.shutdown().await;
}

/// Historique (réponse et raisonnement compris) ; contexte réglé : celui du
/// processus (`-c`), dit à la page (`context`), et la réponse limitée à ce
/// qui en reste.
#[tokio::test]
async fn history_and_context_length_are_sent() {
    let bench = Bench::new(&[THINKER]);
    let first = bench
        .send(request(None, THINKER, Some(Think::Enabled(true))))
        .await;
    let chat_id = first.chat_id();
    bench.set(SettingsPatch {
        context_length: Some(8192),
        ..Default::default()
    });
    let second = bench
        .send(request(
            Some(&chat_id),
            THINKER,
            Some(Think::Enabled(false)),
        ))
        .await;
    // Autre contexte : rechargé.
    assert_eq!(second.count("loading"), 1);
    let args = bench.args(THINKER).await;
    let at = args.iter().position(|arg| arg == "-c").unwrap();
    assert_eq!(args[at + 1], "8192");
    let context = of_type(&second.events(), "context");
    assert_eq!(context.len(), 1);
    assert_eq!(context[0]["size"], json!(8192));
    let used = context[0]["used"].as_i64().unwrap();
    assert!(used > 0 && used < 200, "{used}");

    let sent = &bench.requests(THINKER).await[0];
    let limit = sent["max_tokens"].as_i64().unwrap();
    assert!(limit > 8000 && limit < 8192, "{limit}");
    assert_eq!(
        sent["chat_template_kwargs"],
        json!({ "enable_thinking": false })
    );
    let messages = sent["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1]["role"], json!("assistant"));
    assert_eq!(
        messages[1]["content"],
        json!("Hello! You said: Say hello in one word.")
    );
    assert!(
        messages[1]["reasoning_content"]
            .as_str()
            .unwrap()
            .starts_with("Let me think")
    );
    bench.shutdown().await;
}

#[tokio::test]
async fn the_message_is_kept_as_typed() {
    let bench = Bench::new(&[PLAIN]);
    let typed = "  Say hello\n\nin one word.\n";
    let recorder = bench.send(asking(None, PLAIN, typed)).await;
    let events = recorder.events();
    assert_eq!(events[1]["message"]["content"], json!(typed));
    assert_eq!(
        saved(bench.store(), &recorder.chat_id()).messages[0].content,
        typed
    );
    let sent = &bench.requests(PLAIN).await[0];
    assert_eq!(sent["messages"][0]["content"], json!(typed));
    bench.shutdown().await;
}

/// Réglages de l'app → réglages de l'envoi ; un modèle distant n'est
/// permis que réglage ouvert.
#[tokio::test]
async fn the_send_follows_the_settings() {
    let bench = Bench::new(&[]);
    bench.set(SettingsPatch {
        context_length: Some(8192),
        keep_alive: Some(300),
        ..Default::default()
    });
    let server = bench.server().await;
    assert_eq!(
        (server.prefs.context_length, server.prefs.keep_alive),
        (8192, 300)
    );
    let remote = "@remote/openrouter/qwen/qwen3-32b";
    let off = LlmError::RemoteOff.message();
    assert_eq!(server.refuses(remote, false), Some(off.clone()));
    assert_eq!(server.refuses("any", true), Some(off));
    assert_eq!(server.refuses(PLAIN, false), None);
    bench.set(SettingsPatch {
        remote_models: Some(true),
        ..Default::default()
    });
    let server = bench.server().await;
    assert_eq!(server.refuses(remote, false), None);
}

/// Chargement annoncé : avant le premier envoi (modèle pas prêt), avec les
/// options du réglage ; rien quand il est prêt avec ces options ; de
/// nouveau pour un autre contexte.
#[tokio::test]
async fn a_model_not_in_memory_is_loaded_before_the_answer() {
    let bench = Bench::new(&[SLOWLOAD]);
    bench.set(SettingsPatch {
        context_length: Some(8192),
        keep_alive: Some(300),
        ..Default::default()
    });
    let first = bench.send(request(None, SLOWLOAD, None)).await;
    let events = first.events();
    assert_eq!(
        types(&events),
        [
            "chatCreated",
            "userMessage",
            "loading",
            "loaded",
            "replyStarted",
            "content",
            "context",
            "done"
        ]
    );
    // `loaded` à la fin du chargement (1,5 s pour ce modèle).
    let waited = first.received("loaded")[0] - first.received("loading")[0];
    assert!(waited >= 1400, "{waited} ms");
    let running = bench.fake.engine().running();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].context_length, 8192);
    // Durée en mémoire du réglage : échéance à 300 s environ.
    let expires = running[0].expires_at.unwrap() - now_ms();
    assert!(expires > 250_000 && expires <= 300_000, "{expires}");

    // Prêt avec ces options : rien d'annoncé.
    let chat_id = first.chat_id();
    let again = bench.send(request(Some(&chat_id), SLOWLOAD, None)).await;
    assert_eq!(
        types(&again.events()),
        ["userMessage", "replyStarted", "content", "context", "done"]
    );
    // Autre contexte : rechargé.
    bench.set(SettingsPatch {
        context_length: Some(16384),
        ..Default::default()
    });
    let other = bench.send(request(Some(&chat_id), SLOWLOAD, None)).await;
    assert_eq!(
        types(&other.events())[..3],
        ["userMessage", "loading", "loaded"]
    );
    assert_eq!(bench.fake.engine().running()[0].context_length, 16384);
    bench.shutdown().await;
}

/// Durée en mémoire « garder chargé » (-1) : le modèle de l'envoi est
/// gardé ; la session tenue pendant l'envoi, rendue à la fin.
#[tokio::test]
async fn keep_alive_follows_the_setting() {
    let bench = Bench::new(&[PLAIN]);
    bench.set(SettingsPatch {
        keep_alive: Some(-1),
        ..Default::default()
    });
    bench.send(request(None, PLAIN, None)).await;
    let running = bench.fake.engine().running();
    assert!(running[0].kept, "{running:?}");
    assert!(!running[0].in_use, "the send let go of its session");
    bench.shutdown().await;
}

/// Recherche web par le relais : le modèle reçoit les extraits des
/// résultats de cet envoi ; l'historique (et la page) n'en garde que les
/// titres et les liens.
#[tokio::test(flavor = "multi_thread")]
async fn a_web_search_gives_its_excerpts_to_the_model() {
    let web = crate::chat::tools::fake_web(|_| {}).await;
    let bench = Bench::searching(&[SEARCHER], web.web.relay.clone());
    let recorder = bench
        .send(SendRequest {
            web_search: Some(true),
            ..request(None, SEARCHER, None)
        })
        .await;
    let answer = recorder.contents();
    assert!(answer.starts_with("Found: "), "{answer}");
    assert!(answer.contains("Extrait 1"), "{answer}");
    let results = of_type(&recorder.events(), "toolResult");
    let stored = results[0]["result"].as_str().unwrap();
    assert!(stored.contains("example.org/1"), "{stored}");
    assert!(!stored.contains("Extrait"), "{stored}");
    bench.shutdown().await;
}

/// `web_fetch` ne lit que les adresses écrites par l'utilisateur, dans ce
/// message ou un plus ancien (`chats::user_texts`) : une adresse modifiée par
/// le modèle, ou écrite par lui dans une réponse, est refusée sans requête ;
/// une adresse permise va jusqu'à la lecture (refusée ensuite par la règle
/// des adresses privées : 127.0.0.1). Relecture de l'intégration, I2.
#[tokio::test]
async fn web_fetch_reads_only_urls_given_by_the_user() {
    const ONLY_USER: &str = "web fetch is only allowed for URLs provided by the user";
    let bench = Bench::new(&[FETCHER]);
    let ask = |chat_id: Option<&str>, content: &str| SendRequest {
        web_search: Some(true),
        ..asking(chat_id, FETCHER, content)
    };
    // Adresse modifiée par le modèle (`…/bx`) : refusée.
    let first = bench
        .send(ask(
            None,
            "See http://127.0.0.1:9/a and FETCH+ http://127.0.0.1:9/b",
        ))
        .await;
    let chat_id = first.chat_id();
    // Adresse d'un message plus ancien de l'utilisateur (`…/a`) : permise.
    bench.send(ask(Some(&chat_id), "FETCH FIRST")).await;
    // Adresse écrite par le modèle dans sa première réponse (`…/bx`) :
    // refusée.
    bench.send(ask(Some(&chat_id), "FETCH ANSWER")).await;
    let chat = saved(bench.store(), &chat_id);
    let calls: Vec<(String, String)> = chat
        .messages
        .iter()
        .flat_map(|message| &message.tool_calls)
        .map(|call| {
            let arguments: Value = serde_json::from_str(&call.arguments).unwrap();
            (
                arguments["url"].as_str().unwrap().to_owned(),
                call.error.clone().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert_eq!(calls[0], ("http://127.0.0.1:9/bx".into(), ONLY_USER.into()));
    assert_eq!(calls[1].0, "http://127.0.0.1:9/a");
    assert!(
        calls[1].1.starts_with("failed to fetch the page"),
        "{calls:?}"
    );
    assert_eq!(calls[2], ("http://127.0.0.1:9/bx".into(), ONLY_USER.into()));
    // La première réponse porte bien l'adresse du modèle (sinon l'essai ne
    // prouverait rien).
    assert!(
        chat.messages
            .iter()
            .filter(|message| message.role == chats::Role::Assistant)
            .any(|message| message.content.contains("http://127.0.0.1:9/bx"))
    );
    // `user_texts` : les trois messages de l'utilisateur, dans l'ordre, sans
    // les réponses.
    let id = chat_id.clone();
    let texts = bench
        .store()
        .blocking(move |c| chats::user_texts(c, &id))
        .unwrap();
    assert_eq!(
        texts,
        "See http://127.0.0.1:9/a and FETCH+ http://127.0.0.1:9/b\nFETCH FIRST\nFETCH ANSWER"
    );
    bench.shutdown().await;
}

/// Recherche refusée par le relais (pause, version trop ancienne) ou relais
/// injoignable à la première recherche : l'envoi s'arrête avec ce code
/// (`Executed::Stop`), sans seconde réponse du modèle.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_search_stops_the_send_with_its_code() {
    type Tweak = Box<dyn FnOnce(&mut relay_server::config::Config)>;
    let cases: Vec<(Option<Tweak>, &str)> = vec![
        (Some(Box::new(|c| c.paused = true)), "webSearchPaused"),
        (
            Some(Box::new(|c| c.min_app_version = Some("99.0.0".into()))),
            "webSearchOutdated",
        ),
        (None, "webSearchUnavailable"),
    ];
    for (tweak, kind) in cases {
        let (relay, _web) = match tweak {
            Some(tweak) => {
                let web = crate::chat::tools::fake_web(tweak).await;
                (web.web.relay.clone(), Some(web))
            }
            None => (crate::chat::tools::closed_web_tools().relay, None),
        };
        let bench = Bench::searching(&[SEARCHER], relay);
        let recorder = bench
            .send(SendRequest {
                web_search: Some(true),
                ..request(None, SEARCHER, None)
            })
            .await;
        let events = recorder.events();
        assert_eq!(recorder.last()["kind"], json!(kind), "{events:?}");
        assert_eq!(recorder.count("toolResult"), 0, "{kind}");
        assert_eq!(recorder.count("replyStarted"), 1, "{kind}");
        bench.shutdown().await;
    }
}

/// Modèle distant : réglage coupé, rien n'est demandé ni chargé
/// (`remoteOff`) ; ouvert, sans fournisseur qui le serve : `modelMissing`,
/// jamais un téléchargement.
#[tokio::test]
async fn a_remote_model_is_refused_or_missing_never_loaded() {
    let bench = Bench::new(&[]);
    let remote = "@remote/openrouter/qwen/qwen3-32b";
    let off = bench.send(request(None, remote, None)).await;
    assert_eq!(
        types(&off.events()),
        ["chatCreated", "userMessage", "error"]
    );
    assert_eq!(off.last()["kind"], json!("remoteOff"));
    bench.set(SettingsPatch {
        remote_models: Some(true),
        ..Default::default()
    });
    let on = bench.send(request(None, remote, None)).await;
    assert_eq!(on.last()["kind"], json!("modelMissing"));
    assert_eq!(on.count("download"), 0);
    assert!(bench.access.downloads().list().is_empty());
    assert!(bench.fake.engine().running().is_empty());
    bench.shutdown().await;
}

/// Arrêt pendant le chargement : `cancelled`, sans `loaded` ni réponse ; le
/// chargement continue dans le moteur (un envoi suivant le trouvera prêt).
#[tokio::test]
async fn stopping_during_the_load() {
    let bench = Bench::new(&[LONGLOAD]);
    let (task, recorder) = bench.spawn(request(None, LONGLOAD, None)).await;
    recorder.wait_for("loading", 1).await;
    let asked = Instant::now();
    bench.engine.cancel(&format!("stream-{LONGLOAD}"));
    finished(task).await;
    assert!(asked.elapsed() < Duration::from_millis(500));
    let events = recorder.events();
    assert_one_final(&events);
    assert_eq!(
        types(&events),
        ["chatCreated", "userMessage", "loading", "cancelled"]
    );
    assert_eq!(saved(bench.store(), &recorder.chat_id()).messages.len(), 1);
    assert_eq!(bench.fake.engine().running().len(), 1, "still loading");
    bench.shutdown().await;
}

/// Chargement raté (le processus s'arrête) : `loaded`, puis `engineFailed`,
/// sans réponse.
#[tokio::test]
async fn a_failed_load_ends_the_send() {
    let bench = Bench::new(&[LOADFAIL]);
    let sent = bench.send(request(None, LOADFAIL, None)).await;
    let events = sent.events();
    assert_eq!(
        types(&events),
        ["chatCreated", "userMessage", "loading", "loaded", "error"]
    );
    assert_eq!(sent.last()["kind"], json!("engineFailed"));
    assert!(!sent.last()["message"].as_str().unwrap().is_empty());
    assert_eq!(saved(bench.store(), &sent.chat_id()).messages.len(), 1);
    bench.shutdown().await;
}

/// Pas de moteur installé (`llama-server` absent) : `engineUnavailable`,
/// sans chargement annoncé.
#[tokio::test]
async fn no_engine_is_told_as_such() {
    let bench = Bench::new(&[PLAIN]);
    // Même base et mêmes fichiers, mais aucun binaire du moteur.
    let data_dir = bench.fake.access.data_dir().to_owned();
    let access = Access::detached(bench.store().clone(), data_dir);
    let server = Server::current(&access).await.unwrap();
    let (channel, recorder) = recorder();
    send(
        bench.store(),
        &bench.engine,
        &bench.staging,
        server,
        request(None, PLAIN, None),
        &channel,
    )
    .await
    .unwrap();
    let events = recorder.events();
    assert_one_final(&events);
    assert_eq!(types(&events), ["chatCreated", "userMessage", "error"]);
    assert_eq!(recorder.last()["kind"], json!("engineUnavailable"));
    bench.shutdown().await;
}

/// Plantage du serveur au milieu de la réponse (`connectionLost`), erreur
/// du modèle en cours de route (`model`) : la réponse partielle reste.
#[tokio::test]
async fn failures_are_told_apart_and_partial_answers_kept() {
    let bench = Bench::new(&[CRASH, FAILING]);
    for (model, kind) in [(CRASH, "connectionLost"), (FAILING, "model")] {
        let recorder = bench.send(request(None, model, None)).await;
        let last = recorder.last();
        assert_eq!(last["type"], json!("error"), "{model}");
        assert_eq!(last["kind"], json!(kind), "{model}");
        assert!(!last["message"].as_str().unwrap().is_empty(), "{model}");
        let chat = saved(bench.store(), &recorder.chat_id());
        // Le message de l'utilisateur reste, la réponse partielle aussi.
        assert_eq!(chat.messages.len(), 2, "{model}");
        assert_eq!(recorder.contents(), "One two three", "{model}");
        assert_eq!(chat.messages[1].content, recorder.contents(), "{model}");
    }
    bench.shutdown().await;
}

#[tokio::test]
async fn a_model_repeating_itself_is_stopped() {
    let bench = Bench::new(&[LOOPING]);
    // Sans garde, le faux modèle répète jusqu'à la fin du contexte.
    let recorder = bench.send(request(None, LOOPING, None)).await;
    let last = recorder.last();
    assert_eq!(last["type"], json!("error"));
    assert_eq!(last["kind"], json!("loop"));
    assert_eq!(
        last["message"],
        json!("The model kept repeating the same text, so its reply was stopped.")
    );
    // Réponse reçue gardée ; arrêtée peu après le début de la boucle.
    let chat = saved(bench.store(), &recorder.chat_id());
    let copies = chat.messages[1]
        .content
        .matches(fake_llama::LOOP_SENTENCE)
        .count();
    assert!((4..2000).contains(&copies), "{copies} copies");
    // Flux abandonné par ChatonLM : le serveur arrête la génération.
    let session = bench
        .access
        .open(
            LOOPING,
            OpenOptions::default(),
            &CancellationToken::new(),
            &|| {},
        )
        .await
        .unwrap();
    let mut disconnects = 0;
    for _ in 0..100 {
        disconnects = pilot::<usize>(&session, "disconnects").await;
        if disconnects == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(disconnects, 1);
    drop(session);
    bench.shutdown().await;
}

/// Envoi de `content` à `chatonlm-gist` dans `chat_id` (nouvelle
/// conversation sinon).
async fn send_gist(bench: &Bench, chat_id: Option<&str>, content: &str) -> Recorder {
    bench.send(asking(chat_id, GIST, content)).await
}

fn context_of(bench: &Bench, size: u32) {
    bench.set(SettingsPatch {
        context_length: Some(size),
        ..Default::default()
    });
}

#[tokio::test]
async fn past_eighty_percent_the_chat_is_compacted_before_the_answer() {
    let bench = Bench::new(&[GIST]);
    context_of(&bench, 4096);
    // Premier échange : 3 300 mots de question (un jeton chacun pour le
    // faux), « OK. » en réponse, sur 4 096 (81 %).
    let long = "Tell me everything. ".repeat(1100);
    let first = send_gist(&bench, None, &long).await;
    let chat_id = first.chat_id();
    let counted = saved(bench.store(), &chat_id).context.unwrap();
    assert_eq!(counted.size, 4096);
    assert!(counted.used * 5 >= 4096 * 4, "{counted:?}");
    let second = send_gist(&bench, Some(&chat_id), "Say hello in one word.").await;
    assert_eq!(
        types(&second.events()),
        [
            "userMessage",
            "compacting",
            "compacted",
            "replyStarted",
            "content",
            "context",
            "done"
        ]
    );
    // Résumé demandé de l'échange précédent, sans le nouveau message, en
    // réponse entière.
    let requests = bench.requests(GIST).await;
    assert_eq!(requests.len(), 3);
    let asked = requests[1]["messages"].as_array().unwrap();
    assert_eq!(asked.len(), 3);
    assert!(
        asked[2]["content"]
            .as_str()
            .unwrap()
            .starts_with("Summarize")
    );
    assert_eq!(requests[1]["stream"], json!(false));
    assert!(requests[1].get("chat_template_kwargs").is_none());
    // Place du résumé : un huitième du contexte.
    assert_eq!(requests[1]["max_tokens"], json!(512));
    // Réponse : le résumé en message système, puis le seul nouveau message ;
    // limitée à ce qui reste du contexte.
    let answered = requests[2]["messages"].as_array().unwrap();
    assert_eq!(answered.len(), 2);
    assert_eq!(
        answered[0],
        system(&compact::summary_message(scripted::GIST_SHORT))
    );
    assert_eq!(answered[1]["content"], json!("Say hello in one word."));
    assert!(requests[2]["max_tokens"].as_i64().unwrap() > 3000);
    let chat = saved(bench.store(), &chat_id);
    assert_eq!(chat.compactions.len(), 1);
    let compaction = &chat.compactions[0];
    assert_eq!(compaction.through_message_id, chat.messages[1].id);
    assert!(compaction.automatic);
    assert_eq!(compaction.summary, scripted::GIST_SHORT);
    // Annoncé après le compactage : le résumé, estimé.
    let events = second.events();
    let compacted = events.iter().find(|e| e["type"] == "compacted").unwrap();
    assert!(compacted["context"]["used"].as_i64().unwrap() < 200);
    // Tous les messages restent ; contexte compté après la réponse.
    assert_eq!(chat.messages.len(), 4);
    assert_eq!(chat.context.unwrap().size, 4096);
    assert!(chat.context.unwrap().used < 1000);
    bench.shutdown().await;
}

#[tokio::test]
async fn a_reply_reaching_the_end_of_the_context_is_cut() {
    let bench = Bench::new(&[GIST]);
    context_of(&bench, 4096);
    // Premier échange : 2 400 mots, « OK. » (59 % de 4 096).
    let first = send_gist(&bench, None, &"Tell me everything. ".repeat(800)).await;
    let chat_id = first.chat_id();
    let used = saved(bench.store(), &chat_id).context.unwrap().used;
    let second = send_gist(&bench, Some(&chat_id), "Tell me a story at length.").await;
    assert_eq!(
        types(&second.events()),
        ["userMessage", "replyStarted", "content", "context", "error"]
    );
    assert_eq!(second.last()["kind"], json!("contextFull"));
    // Limite : ce qui reste après l'échange compté et le nouveau message
    // estimé (corrigé par le compte du moteur).
    let limit = bench.requests(GIST).await[1]["max_tokens"]
        .as_i64()
        .unwrap();
    assert!(limit < 4096 - used && limit > 4096 - used - 40, "{limit}");
    let chat = saved(bench.store(), &chat_id);
    let words = chat.messages[3].content.split_whitespace().count();
    assert!(words as i64 <= limit && words > 300, "{words}");
    assert!(chat.compactions.is_empty());
    assert!(chat.context.unwrap().used > used + 300);
    bench.shutdown().await;
}

#[tokio::test]
async fn compacting_on_demand() {
    let bench = Bench::new(&[GIST]);
    context_of(&bench, 4096);
    let (store, engine) = (bench.store(), &bench.engine);
    // Rien encore : conversation vide.
    let chat_id = store.call(|c| chats::create(c, now_ms())).await.unwrap();
    let empty = compact_chat(store, engine, bench.server().await, &chat_id, GIST).await;
    assert!(matches!(empty, Err(AppError::Invalid(_))), "{empty:?}");
    let first = send_gist(&bench, Some(&chat_id), "Say hello in one word.").await;
    assert_eq!(first.last()["type"], json!("done"));
    // Conversation occupée : refusé.
    let (_cancel, _finish) = engine.begin(&chat_id, "other", Origin::Page).unwrap();
    let busy = compact_chat(store, engine, bench.server().await, &chat_id, GIST).await;
    assert!(matches!(busy, Err(AppError::Invalid(_))), "{busy:?}");
    engine.end(&chat_id);
    // Modèle absent : refusé, rien n'est téléchargé.
    let absent = compact_chat(store, engine, bench.server().await, &chat_id, PLAIN).await;
    assert!(matches!(absent, Err(AppError::NotFound(_))), "{absent:?}");

    let compaction = compact_chat(store, engine, bench.server().await, &chat_id, GIST)
        .await
        .unwrap();
    assert!(!compaction.automatic);
    assert_eq!(compaction.through_message_id, 2);
    let chat = saved(store, &chat_id);
    assert_eq!(chat.compactions, [compaction]);
    // Contexte estimé : le résumé seul.
    assert!(chat.context.unwrap().used < 100);
    // Plus rien de nouveau : refusé ; un envoi n'est plus refusé (verrou
    // rendu).
    let again = compact_chat(store, engine, bench.server().await, &chat_id, GIST).await;
    assert!(matches!(again, Err(AppError::Invalid(_))), "{again:?}");
    let next = send_gist(&bench, Some(&chat_id), "Say hello in one word.").await;
    assert_eq!(next.last()["type"], json!("done"));
    let sent = bench.requests(GIST).await.pop().unwrap();
    assert_eq!(sent["messages"].as_array().unwrap().len(), 2);
    let summary = compact::summary_message(scripted::GIST_SHORT);
    assert_eq!(sent["messages"][0], system(&summary));

    // Avec un pré-prompt : lui d'abord, puis le résumé, dans un seul message
    // système.
    set_prompt(&bench, "Be brief.");
    let last = send_gist(&bench, Some(&chat_id), "Say hello in one word.").await;
    assert_eq!(last.last()["type"], json!("done"));
    let sent = bench.requests(GIST).await.pop().unwrap();
    assert_eq!(systems(&sent), 1);
    assert_eq!(
        sent["messages"][0],
        system(&format!("Be brief.\n\n{summary}"))
    );
    bench.shutdown().await;
}

#[tokio::test]
async fn stop_ends_a_compaction_on_demand() {
    let bench = Bench::new(&[PLAIN, HANG]);
    let first = bench.send(request(None, PLAIN, None)).await;
    let chat_id = first.chat_id();
    let engine = &bench.engine;
    // Modèle qui ne répond jamais : arrêté par Stop (`cancel_message` sous
    // l'identifiant `compact:<conversation>`).
    let compacting = compact_chat(bench.store(), engine, bench.server().await, &chat_id, HANG);
    let stopping = async {
        wait_until(WAIT, "the compaction to start", || engine.is_busy()).await;
        // Le temps de charger le modèle et d'envoyer la requête.
        let session = bench
            .access
            .open(
                HANG,
                OpenOptions::default(),
                &CancellationToken::new(),
                &|| {},
            )
            .await
            .unwrap();
        for _ in 0..200 {
            if !requests_of(&session).await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        engine.cancel(&format!("compact:{chat_id}"));
    };
    let (stopped, ()) = tokio::time::timeout(WAIT, async { tokio::join!(compacting, stopping) })
        .await
        .expect("the compaction was not stopped");
    let Err(AppError::Invalid(message)) = stopped else {
        panic!("{stopped:?}");
    };
    assert_eq!(message, "the compaction was stopped");
    assert!(saved(bench.store(), &chat_id).compactions.is_empty());
    // Verrou rendu.
    assert!(!engine.is_busy());
    bench.shutdown().await;
}

#[tokio::test]
async fn editing_a_compacted_message_drops_the_compaction() {
    let bench = Bench::new(&[GIST]);
    context_of(&bench, 4096);
    let first = send_gist(&bench, None, "Say hello in one word.").await;
    let chat_id = first.chat_id();
    compact_chat(
        bench.store(),
        &bench.engine,
        bench.server().await,
        &chat_id,
        GIST,
    )
    .await
    .unwrap();
    let edit = SendRequest {
        edit_message_id: Some(1),
        ..asking(Some(&chat_id), GIST, "Say hello in two words.")
    };
    let recorder = bench.send(edit).await;
    assert_eq!(recorder.last()["type"], json!("done"));
    let chat = saved(bench.store(), &chat_id);
    assert!(chat.compactions.is_empty());
    // Le message modifié part seul, sans résumé.
    let sent = bench.requests(GIST).await.pop().unwrap();
    assert_eq!(
        sent["messages"],
        json!([{ "role": "user", "content": "Say hello in two words." }])
    );
    // Contexte oublié par la modification : seul le message, estimé.
    let limit = sent["max_tokens"].as_i64().unwrap();
    assert!(limit > 4096 - 40 && limit < 4096, "{limit}");
    bench.shutdown().await;
}

/// Événements de téléchargement reçus, en (total, completed, done).
fn downloads(events: &[Value]) -> Vec<(u64, u64, bool)> {
    let downloads = events.iter().filter(|event| event["type"] == "download");
    downloads
        .map(|event| {
            (
                event["total"].as_u64().unwrap(),
                event["completed"].as_u64().unwrap(),
                event["done"].as_bool().unwrap(),
            )
        })
        .collect()
}

/// Modèle de Hugging Face absent d'un envoi de la page : téléchargé par la
/// file du ModelHub (il y paraît), progression jusqu'à l'installation, puis
/// chargé et la réponse.
#[tokio::test]
async fn a_missing_model_is_downloaded_then_answers() {
    let hf = fake_services::hf::FakeHf::start(fake_services::hf::HfMode::Normal)
        .await
        .unwrap();
    let bench = Bench::downloading(&[], &hf);
    let model = "chatonlm-dl/Tiny-slow-GGUF:Q4_K_M";
    let recorder = bench.send(request(None, model, None)).await;
    let events = recorder.events();
    assert_eq!(
        types(&events),
        [
            "chatCreated",
            "userMessage",
            "download",
            "loading",
            "loaded",
            "replyStarted",
            "content",
            "context",
            "done"
        ]
    );
    let seen = downloads(&events);
    // Le dernier : complet ; les autres ne reculent pas.
    let (total, completed, done) = *seen.last().unwrap();
    assert!(done && total > 0 && completed == total, "{seen:?}");
    assert!(seen[..seen.len() - 1].iter().all(|(_, _, done)| !done));
    assert!(
        seen.windows(2).all(|pair| pair[0].1 <= pair[1].1),
        "{seen:?}"
    );
    assert!(seen.len() >= 3, "progress was shown: {seen:?}");
    // Installé dans la base locale, terminé dans la file.
    let installed = crate::local_models::get(bench.store(), bench.access.data_dir(), model)
        .await
        .unwrap();
    assert!(installed.is_some());
    let queued = bench.access.downloads().list();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].state, crate::hub::types::HubDownloadState::Done);
    assert_eq!(
        recorder.contents(),
        "Hello! You said: Say hello in one word."
    );
    // Envoi suivant : plus rien à télécharger.
    let again = bench
        .send(request(Some(&recorder.chat_id()), model, None))
        .await;
    assert_eq!(again.count("download"), 0);
    bench.shutdown().await;
}

/// Échecs du téléchargement : texte de la file, type `download`, aucune
/// réponse ; dépôt inexistant, empreinte fausse, Hugging Face injoignable.
#[tokio::test]
async fn download_failures_are_told_apart() {
    let hf = fake_services::hf::FakeHf::start(fake_services::hf::HfMode::Normal)
        .await
        .unwrap();
    let bench = Bench::downloading(&[], &hf);
    for (model, start) in [
        ("chatonlm-dl/Gone-missing-GGUF:Q4_K_M", ""),
        ("chatonlm-dl/Wrong-badsha-GGUF:Q4_K_M", ""),
    ] {
        let recorder = bench.send(request(None, model, None)).await;
        let last = recorder.last();
        assert_eq!(last["kind"], json!("download"), "{model}");
        let message = last["message"].as_str().unwrap();
        assert!(
            !message.is_empty() && message.starts_with(start),
            "{model}: {message}"
        );
        assert_eq!(
            saved(bench.store(), &recorder.chat_id()).messages.len(),
            1,
            "{model}"
        );
    }
    let closed = fake_services::hf::FakeHf::start(fake_services::hf::HfMode::Normal)
        .await
        .unwrap();
    let offline = Bench::downloading(&[], &closed);
    closed.stop().await;
    let recorder = offline
        .send(request(None, "chatonlm-dl/Tiny-GGUF:Q4_K_M", None))
        .await;
    assert_eq!(recorder.last()["kind"], json!("download"));
    let message = recorder.last()["message"].as_str().unwrap().to_owned();
    assert!(message.starts_with("can't reach Hugging Face"), "{message}");
    bench.shutdown().await;
    offline.shutdown().await;
}

/// Arrêt pendant le téléchargement : `cancelled`, téléchargement annulé
/// (demandé par cet envoi) ; un téléchargement déjà en file reste à la file.
/// Mis en pause depuis le ModelHub : l'envoi s'arrête.
#[tokio::test]
async fn cancelling_while_downloading() {
    let hf = fake_services::hf::FakeHf::start(fake_services::hf::HfMode::Normal)
        .await
        .unwrap();
    let bench = Bench::downloading(&[], &hf);
    let stall = "chatonlm-dl/Stuck-stall-GGUF:Q4_K_M";
    let state = |model: &str| {
        bench
            .access
            .downloads()
            .list()
            .into_iter()
            .find(|download| download.model == model)
            .map(|download| download.state)
    };
    let (task, recorder) = bench.spawn(request(None, stall, None)).await;
    recorder.wait_for("download", 2).await;
    bench.engine.cancel(&format!("stream-{stall}"));
    finished(task).await;
    assert_eq!(
        types(&recorder.events()),
        ["chatCreated", "userMessage", "download", "cancelled"]
    );
    use crate::hub::types::HubDownloadState as State;
    assert_eq!(state(stall), Some(State::Cancelled));

    // Déjà demandé par le ModelHub : laissé à la file.
    let other = "chatonlm-dl/Other-stall-GGUF:Q4_K_M";
    bench
        .access
        .downloads()
        .download(crate::hub::types::HubDownloadRequest {
            model: other.into(),
            repo: None,
            label: None,
        })
        .unwrap();
    let (task, recorder) = bench.spawn(request(None, other, None)).await;
    recorder.wait_for("download", 2).await;
    bench.engine.cancel(&format!("stream-{other}"));
    finished(task).await;
    assert_eq!(recorder.last()["type"], json!("cancelled"));
    assert!(matches!(
        state(other),
        Some(State::Downloading | State::Starting)
    ));

    // Mis en pause depuis le ModelHub : l'envoi en attente s'arrête.
    let (task, recorder) = bench
        .spawn(SendRequest {
            stream_id: "paused".into(),
            ..request(None, other, None)
        })
        .await;
    recorder.wait_for("download", 1).await;
    bench.access.downloads().pause(other);
    finished(task).await;
    assert_eq!(recorder.last()["type"], json!("cancelled"));
    assert_eq!(state(other), Some(State::Paused));
    bench.access.downloads().cancel(other);
    bench.shutdown().await;
}

/// Jamais de téléchargement pour un modèle repris d'Ollama, ni pour un envoi
/// venu d'ailleurs (bot, éditeur) : `modelMissing` d'emblée.
#[tokio::test]
async fn only_a_page_downloads_and_only_from_hugging_face() {
    let hf = fake_services::hf::FakeHf::start(fake_services::hf::HfMode::Normal)
        .await
        .unwrap();
    let bench = Bench::downloading(&[], &hf);
    let imported = bench.send(request(None, IMPORTED, None)).await;
    assert_eq!(
        types(&imported.events()),
        ["chatCreated", "userMessage", "error"]
    );
    assert_eq!(imported.last()["kind"], json!("modelMissing"));
    for origin in [Origin::Remote, Origin::Bridge] {
        let events = Mutex::new(Vec::new());
        let sink = |event: ChatEvent| events.lock().push(event);
        send_to(
            bench.store(),
            &bench.engine,
            &bench.staging,
            bench.server().await,
            request(None, "chatonlm-dl/Tiny-GGUF:Q4_K_M", None),
            origin,
            &sink,
        )
        .await
        .unwrap();
        let last = events.lock().last().cloned();
        assert!(
            matches!(
                last,
                Some(ChatEvent::Error {
                    kind: ChatErrorKind::ModelMissing,
                    ..
                })
            ),
            "{origin:?}: {last:?}"
        );
    }
    assert!(bench.access.downloads().list().is_empty());
    assert!(hf.requests().is_empty());
    bench.shutdown().await;
}

/// Réflexion envoyée d'après les détails du modèle : marche/arrêt pour un
/// modèle qui réfléchit (`enable_thinking`), un niveau qu'il ne connaît pas
/// retiré ; rien pour un modèle qui ne réfléchit pas (une seule requête).
#[tokio::test]
async fn think_is_filtered_by_the_model_details() {
    let bench = Bench::new(&[THINKER, PLAIN]);
    let level = |name: &str| Some(Think::Level(name.into()));
    // Modèle, réflexion demandée, réglages envoyés (None : rien).
    for (model, think, expected) in [
        (THINKER, level("high"), None),
        (
            THINKER,
            Some(Think::Enabled(false)),
            Some(json!({ "enable_thinking": false })),
        ),
        (
            THINKER,
            Some(Think::Enabled(true)),
            Some(json!({ "enable_thinking": true })),
        ),
        (THINKER, None, None),
        (PLAIN, level("high"), None),
        (PLAIN, Some(Think::Enabled(true)), None),
    ] {
        let before = bench.requests(model).await.len();
        let case = format!("{model} with {think:?}");
        let recorder = bench.send(request(None, model, think.clone())).await;
        assert_eq!(recorder.last()["type"], json!("done"), "{case}");
        let sent = bench.requests(model).await;
        assert_eq!(sent.len(), before + 1, "{case}");
        assert_eq!(
            sent[before].get("chat_template_kwargs"),
            expected.as_ref(),
            "{case}"
        );
        assert!(sent[before].get("reasoning_effort").is_none(), "{case}");
    }
    bench.shutdown().await;
}

#[tokio::test]
async fn empty_messages_and_busy_chats_are_refused() {
    let bench = Bench::new(&[SLOW, PLAIN]);
    let (channel, busy) = recorder();
    let mut empty = request(None, PLAIN, None);
    empty.content = " \n\t ".into();
    let refused = send(
        bench.store(),
        &bench.engine,
        &bench.staging,
        bench.server().await,
        empty,
        &channel,
    )
    .await;
    assert!(matches!(refused, Err(AppError::Invalid(_))), "{refused:?}");
    assert!(busy.events().is_empty());

    let chat_id = bench.store().blocking(|c| chats::create(c, 1)).unwrap();
    let (task, busy) = bench.spawn(request(Some(&chat_id), SLOW, None)).await;
    busy.wait_for("content", 1).await;
    let (second, result) = bench
        .send_result(request(Some(&chat_id), PLAIN, None))
        .await;
    assert!(matches!(result, Err(AppError::Invalid(_))), "{result:?}");
    assert!(second.events().is_empty());

    // Réponse en cours : la conversation lue montre son dernier état.
    let mut chat = saved(bench.store(), &chat_id);
    bench.engine.overlay(&mut chat);
    assert_eq!(chat.messages[1].content, busy.contents());
    finished(task).await;
    assert_one_final(&busy.events());
    bench.shutdown().await;
}

/// Bouton Unload (ou suppression) pendant une réponse : un arrêt, pas une
/// panne (contrat 102 § 3) : `cancelled`, aucune erreur, la réponse partielle
/// gardée (relecture L5a, M4).
#[tokio::test]
async fn unloading_the_model_mid_answer_stops_the_reply() {
    // `hang` : la réponse ne finit jamais d'elle-même (le faux finit sinon
    // ses réponses en cours avant de s'arrêter) ; la connexion est coupée
    // par l'arrêt du processus.
    let bench = Bench::new(&[HANG]);
    let (task, recorder) = bench.spawn(request(None, HANG, None)).await;
    recorder.wait_for("loaded", 1).await;
    let session = bench
        .access
        .open(
            HANG,
            OpenOptions::default(),
            &CancellationToken::new(),
            &|| {},
        )
        .await
        .unwrap();
    for _ in 0..200 {
        if !requests_of(&session).await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(session);
    bench.fake.engine().unload(HANG).await;
    finished(task).await;
    let events = recorder.events();
    assert_one_final(&events);
    assert_eq!(recorder.last()["type"], json!("cancelled"), "{events:?}");
    assert_eq!(recorder.count("error"), 0, "{events:?}");
    bench.shutdown().await;
}

/// Arrêt sans un morceau reçu (en-têtes seuls) ou en plein flux : `cancelled`
/// aussitôt, la réponse partielle enregistrée telle que reçue, la requête
/// abandonnée (le serveur arrête la génération).
#[tokio::test]
async fn cancelling_while_waiting_or_streaming() {
    let bench = Bench::new(&[HANG, SLOW]);
    for (model, contents) in [(HANG, 0), (SLOW, 2)] {
        let (task, recorder) = bench.spawn(request(None, model, None)).await;
        if contents > 0 {
            recorder.wait_for("content", contents).await;
        } else {
            recorder.wait_for("loaded", 1).await;
            let session = bench
                .access
                .open(
                    model,
                    OpenOptions::default(),
                    &CancellationToken::new(),
                    &|| {},
                )
                .await
                .unwrap();
            for _ in 0..200 {
                if !requests_of(&session).await.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        let asked = Instant::now();
        bench.engine.cancel(&format!("stream-{model}"));
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap_or_else(|_| panic!("{model}: still running 1 s after the cancel"))
            .unwrap()
            .unwrap();
        assert!(asked.elapsed() < Duration::from_secs(1), "{model}");
        let events = recorder.events();
        assert_one_final(&events);
        assert_eq!(recorder.last()["type"], json!("cancelled"), "{model}");
        let chat = saved(bench.store(), &recorder.chat_id());
        assert_eq!(
            chat.messages.len(),
            1 + usize::from(contents > 0),
            "{model}"
        );
        if contents > 0 {
            assert_eq!(chat.messages[1].content, recorder.contents(), "{model}");
        }
        // Le serveur a vu la requête abandonnée.
        let session = bench
            .access
            .open(
                model,
                OpenOptions::default(),
                &CancellationToken::new(),
                &|| {},
            )
            .await
            .unwrap();
        let mut disconnects = 0;
        for _ in 0..100 {
            disconnects = pilot::<usize>(&session, "disconnects").await;
            if disconnects > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(disconnects, 1, "{model}");
    }
    bench.shutdown().await;
}

#[tokio::test]
async fn stopping_a_chat_waits_for_the_last_write() {
    let bench = Bench::new(&[SLOW]);
    let (task, recorder) = bench.spawn(request(None, SLOW, None)).await;
    recorder.wait_for("content", 2).await;
    let chat_id = recorder.chat_id();
    bench.engine.stop_chat(&chat_id).await;
    // Dernière écriture faite avant le retour de stop_chat.
    assert_eq!(
        saved(bench.store(), &chat_id).messages[1].content,
        "Hello! You"
    );
    bench
        .store()
        .blocking(|c| chats::delete(c, &chat_id))
        .unwrap();
    finished(task).await;
    assert_one_final(&recorder.events());
    assert_eq!(recorder.last()["type"], json!("cancelled"));
    bench.shutdown().await;
}

#[tokio::test]
async fn stopping_everything_saves_every_answer() {
    // Deux modèles : un processus n'a qu'un emplacement (une requête à la
    // fois).
    let bench = Bench::new(&[SLOW, SLOW_TOO]);
    let mut running = Vec::new();
    for (stream, model) in [("stream-a", SLOW), ("stream-b", SLOW_TOO)] {
        let mut ask = request(None, model, None);
        ask.stream_id = stream.into();
        let (task, recorder) = bench.spawn(ask).await;
        recorder.wait_for("content", 2).await;
        running.push((task, recorder));
    }
    let asked = Instant::now();
    bench.engine.stop_all(Duration::from_secs(1)).await;
    // Dernières écritures faites avant le retour de stop_all (fermeture de
    // l'app : rien n'attend les envois ensuite).
    for (_, recorder) in &running {
        let chat = saved(bench.store(), &recorder.chat_id());
        assert_eq!(chat.messages[1].content, recorder.contents());
    }
    for (task, recorder) in running {
        finished(task).await;
        assert_one_final(&recorder.events());
        assert_eq!(recorder.last()["type"], json!("cancelled"));
        let chat = saved(bench.store(), &recorder.chat_id());
        assert_eq!(chat.messages[1].content, recorder.contents());
    }
    let elapsed = asked.elapsed();
    assert!(elapsed < Duration::from_millis(1500), "{elapsed:?}");
    bench.shutdown().await;
}

/// Phrase longue : la réponse lente de `chatonlm-slow` dure plusieurs
/// secondes (un mot toutes les 150 ms).
const LONG_SENTENCE: &str = "Please repeat after me every single word of this rather long \
    sentence slowly so that the answer keeps streaming for a few seconds.";

#[tokio::test]
async fn a_long_answer_is_saved_while_it_streams() {
    let bench = Bench::new(&[SLOW]);
    let (task, recorder) = bench.spawn(asking(None, SLOW, LONG_SENTENCE)).await;
    recorder.wait_for("content", 1).await;
    // Plus que l'intervalle des écritures (500 ms) : l'écriture périodique
    // a eu lieu pendant le flux.
    tokio::time::sleep(Duration::from_millis(900)).await;
    let chat = saved(bench.store(), &recorder.chat_id());
    assert!(!chat.messages[1].content.is_empty());
    assert!(recorder.contents().starts_with(&chat.messages[1].content));
    assert!(!recorder.events().iter().any(is_final));
    bench.engine.cancel(&format!("stream-{SLOW}"));
    finished(task).await;
    assert_one_final(&recorder.events());
    bench.shutdown().await;
}

/// Canal qui, à l'événement final, lit la réponse en base (la page relit la
/// conversation à ce moment-là) : renvoie aussi ce contenu.
fn recorder_reading(store: Store) -> (Channel<ChatEvent>, Recorder, Arc<Mutex<Option<String>>>) {
    let recorder = Recorder::default();
    let at_final = Arc::new(Mutex::new(None));
    let (sink, seen) = (recorder.0.clone(), at_final.clone());
    let channel = Channel::new(move |body| {
        if let InvokeResponseBody::Json(text) = body {
            let event: Value = serde_json::from_str(&text).unwrap();
            let mut events = sink.lock();
            if is_final(&event) {
                let chat_id = events[0].1["chatId"].as_str().unwrap().to_owned();
                let chat = store.blocking(move |c| chats::get(c, &chat_id)).unwrap();
                *seen.lock() = chat.messages.get(1).map(|m| m.content.clone());
            }
            events.push((now_ms(), event));
        }
        Ok(())
    });
    (channel, recorder, at_final)
}

#[tokio::test]
async fn the_final_event_comes_after_the_last_write() {
    let bench = Bench::new(&[PLAIN, SLOW]);
    // Fin normale (flux rapide : pas d'écriture périodique avant).
    let (channel, recorder, at_final) = recorder_reading(bench.store().clone());
    send(
        bench.store(),
        &bench.engine,
        &bench.staging,
        bench.server().await,
        request(None, PLAIN, None),
        &channel,
    )
    .await
    .unwrap();
    assert!(!recorder.contents().is_empty());
    assert_eq!(at_final.lock().clone(), Some(recorder.contents()));

    // Arrêt en plein flux, avant toute écriture périodique.
    let (channel, recorder, at_final) = recorder_reading(bench.store().clone());
    let server = bench.server().await;
    let (store, engine, staging) = (
        bench.store().clone(),
        bench.engine.clone(),
        bench.staging.clone(),
    );
    let task = tokio::spawn(async move {
        send(
            &store,
            &engine,
            &staging,
            server,
            request(None, SLOW, None),
            &channel,
        )
        .await
    });
    recorder.wait_for("content", 2).await;
    bench.engine.cancel(&format!("stream-{SLOW}"));
    finished(task).await;
    assert_eq!(at_final.lock().clone(), Some(recorder.contents()));
    bench.shutdown().await;
}

#[tokio::test]
async fn thinking_ends_with_the_first_piece_of_answer() {
    let bench = Bench::new(&[PENSIVE]);
    // Un morceau toutes les 50 ms : chaque étape a son heure.
    let recorder = bench.send(request(None, PENSIVE, None)).await;
    assert_eq!(recorder.last()["type"], json!("done"));
    let reply = &saved(bench.store(), &recorder.chat_id()).messages[1];
    let ended = reply.thinking_ended_at.unwrap();
    let last_thinking = *recorder.received("thinking").last().unwrap();
    let first_content = recorder.received("content")[0];
    // Fin datée au premier morceau de réponse : après le dernier morceau de
    // raisonnement, avant la fin du flux.
    assert!(
        last_thinking <= ended && ended <= first_content,
        "{last_thinking} <= {ended} <= {first_content}"
    );
    // Morceaux espacés de 50 ms : une fin datée au dernier morceau de
    // raisonnement serait ~50 ms trop tôt.
    assert!(first_content - ended < 25, "{ended} vs {first_content}");
    assert!(ended < reply.updated_at, "{ended} < {}", reply.updated_at);
    assert!(reply.thinking_started_at.unwrap() < ended);
    assert_eq!(reply.content, "Hello there!");
    bench.shutdown().await;
}

/// Texte du message de l'utilisateur tel qu'envoyé avec notes.txt et
/// report.pdf (format exact de `buildChatRequest`).
const DESCRIBED: &str = "Describe these files.\n--- File: notes.txt ---\nLine 1\nLine 2\n--- End of notes.txt ---\n--- File: report.pdf ---\nText of the PDF\n--- End of report.pdf ---";

fn data_url(png: &[u8]) -> String {
    format!("data:image/png;base64,{}", BASE64.encode(png))
}

#[tokio::test]
async fn attachments_go_with_the_message_and_to_the_model() {
    let bench = Bench::new(&[SEER]);
    let staging = &bench.staging;
    let ids = vec![
        stage(staging, "notes.txt", b"Line 1\nLine 2").await,
        stage(staging, "red.png", &red_png()).await,
        stage(staging, "report.pdf", b"%PDF-1.4 fake").await,
    ];
    let mut ask = asking(None, SEER, "Describe these files.");
    ask.attachment_ids = Some(ids.clone());
    let first = bench.send(ask).await;
    let events = first.events();
    assert_eq!(first.last()["type"], json!("done"), "{events:?}");

    // Le message enregistré garde le texte tapé ; ses pièces jointes
    // suivent, dans l'ordre, dans l'événement comme en base.
    let png_size = red_png().len();
    let expected = json!([
        {"id": 1, "filename": "notes.txt", "kind": "text", "mime": "text/plain", "size": 13},
        {"id": 2, "filename": "red.png", "kind": "image", "mime": "image/png", "size": png_size},
        {"id": 3, "filename": "report.pdf", "kind": "text", "mime": "application/pdf", "size": 13},
    ]);
    let user = &events[1]["message"];
    assert_eq!(user["content"], json!("Describe these files."));
    assert_eq!(user["attachments"], expected);
    let chat = saved(bench.store(), &first.chat_id());
    assert_eq!(json!(chat.messages[0].attachments), expected);
    assert_eq!(chat.messages[0].content, "Describe these files.");
    // Les fichiers ont quitté la zone de préparation.
    assert!(ids.iter().all(|id| staging.get(id).is_none()));
    // Le modèle a vu l'image (projecteur chargé).
    assert!(
        first.contents().starts_with("I see 1 image(s)."),
        "{}",
        first.contents()
    );

    let image = json!({ "type": "image_url", "image_url": { "url": data_url(&red_png()) } });
    let with_files = json!({ "role": "user", "content": [
        { "type": "text", "text": DESCRIBED }, image
    ] });
    let sent = &bench.requests(SEER).await[0];
    assert_eq!(sent["messages"], json!([with_files]));

    // Message suivant : l'historique renvoie les fichiers du premier.
    let next = asking(Some(&first.chat_id()), SEER, "Again");
    bench.send(next).await;
    let messages = bench.requests(SEER).await[1]["messages"].clone();
    assert_eq!(messages[0], with_files);
    assert_eq!(messages[2], json!({"role": "user", "content": "Again"}));
    assert_eq!(messages.as_array().unwrap().len(), 3);
    bench.shutdown().await;
}

/// Photo WebP jointe (vraie, enregistrée telle quelle) : le moteur la reçoit
/// en PNG, la réponse arrive, et le message suivant de la conversation passe
/// aussi (le faux refuse WebP comme b12110 sans `ffmpeg` ; relecture L5a,
/// B2).
#[tokio::test]
async fn a_webp_photo_reaches_the_model_as_png() {
    let bench = Bench::new(&[SEER]);
    let webp = super::stream::small_webp();
    let id = stage(&bench.staging, "photo.webp", &webp).await;
    let mut ask = asking(None, SEER, "What is it?");
    ask.attachment_ids = Some(vec![id]);
    let first = bench.send(ask).await;
    assert_eq!(first.last()["type"], json!("done"), "{:?}", first.events());
    assert!(first.contents().starts_with("I see 1 image(s)."));
    // Enregistrée telle quelle.
    let chat = saved(bench.store(), &first.chat_id());
    assert_eq!(chat.messages[0].attachments[0].mime, "image/webp");
    let next = bench
        .send(asking(Some(&first.chat_id()), SEER, "Again"))
        .await;
    assert_eq!(next.last()["type"], json!("done"), "{:?}", next.events());
    let requests = bench.requests(SEER).await;
    assert_eq!(requests.len(), 2);
    for sent in &requests {
        let url = sent["messages"][0]["content"][1]["image_url"]["url"]
            .as_str()
            .unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
    }
    bench.shutdown().await;
}

/// Images relues dans les requêtes reçues par le faux : jamais vers un
/// modèle sans la capacité « vision » (le serveur refuserait toute la
/// requête) ; le texte des fichiers part toujours.
#[tokio::test]
async fn images_are_left_out_for_a_model_without_vision() {
    let bench = Bench::new(&[THINKER, PLAIN, SEER]);
    let staging = &bench.staging;
    for (model, images) in [(THINKER, false), (PLAIN, false), (SEER, true)] {
        let ids = vec![
            stage(staging, "notes.txt", b"Line 1").await,
            stage(staging, "red.png", &red_png()).await,
        ];
        let mut ask = request(None, model, None);
        ask.attachment_ids = Some(ids);
        let before = bench.requests(model).await.len();
        let recorder = bench.send(ask).await;
        assert_eq!(recorder.last()["type"], json!("done"), "{model}");
        let sent = &bench.requests(model).await[before]["messages"][0];
        let text =
            "Say hello in one word.\n--- File: notes.txt ---\nLine 1\n--- End of notes.txt ---";
        if images {
            assert_eq!(
                sent["content"][0],
                json!({ "type": "text", "text": text }),
                "{model}"
            );
            assert_eq!(sent["content"][1]["type"], json!("image_url"), "{model}");
        } else {
            assert_eq!(sent["content"], json!(text), "{model}");
        }
        // L'image reste une pièce jointe du message enregistré.
        let chat = saved(bench.store(), &recorder.chat_id());
        assert_eq!(chat.messages[0].attachments.len(), 2, "{model}");
    }
    bench.shutdown().await;
}

#[tokio::test]
async fn unknown_attachments_are_refused_before_anything_is_written() {
    let bench = Bench::new(&[PLAIN]);
    let known = stage(&bench.staging, "notes.txt", b"Line 1").await;
    let mut ask = request(None, PLAIN, None);
    ask.attachment_ids = Some(vec![known.clone(), "unknown".into()]);
    let (recorder, refused) = bench.send_result(ask).await;
    let Err(AppError::Invalid(message)) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(message, "unknown attachment: unknown");
    assert!(recorder.events().is_empty());
    // Aucune conversation créée, aucun modèle chargé, le fichier connu reste.
    assert!(
        bench
            .store()
            .blocking(|c| chats::list(c))
            .unwrap()
            .is_empty()
    );
    assert!(bench.fake.engine().running().is_empty());
    assert!(bench.staging.get(&known).is_some());
    bench.shutdown().await;
}

#[tokio::test]
async fn files_come_back_only_when_the_send_is_refused() {
    let bench = Bench::new(&[PLAIN, SLOW]);
    let staging = bench.staging.clone();
    let file = stage(&staging, "notes.txt", b"Line 1").await;
    let with_file = |chat_id: Option<&str>, file: &str| {
        let mut ask = request(chat_id, PLAIN, None);
        ask.attachment_ids = Some(vec![file.to_owned()]);
        ask
    };

    // Conversation occupée : envoi refusé, le fichier reste préparé.
    let chat_id = bench.store().blocking(|c| chats::create(c, 1)).unwrap();
    let (task, busy) = bench.spawn(request(Some(&chat_id), SLOW, None)).await;
    busy.wait_for("content", 1).await;
    let (_, refused) = bench.send_result(with_file(Some(&chat_id), &file)).await;
    assert!(matches!(refused, Err(AppError::Invalid(_))), "{refused:?}");
    assert!(staging.get(&file).is_some());
    bench.engine.cancel(&format!("stream-{SLOW}"));
    finished(task).await;

    // Écriture du message impossible (conversation supprimée) : erreur
    // finale ; envoi accepté, le fichier quitte quand même la zone (la page a
    // vidé son composer et ne le renverra pas).
    let failed = bench.send(with_file(Some("deleted"), &file)).await;
    let events = failed.events();
    assert_eq!(types(&events), ["error"], "{events:?}");
    assert_eq!(events[0]["kind"], json!("internal"));
    assert!(staging.get(&file).is_none());

    // Enregistré : il quitte la zone.
    let file = stage(&staging, "notes.txt", b"Line 1").await;
    let sent = bench.send(with_file(None, &file)).await;
    assert_eq!(sent.last()["type"], json!("done"));
    assert!(staging.get(&file).is_none());
    bench.shutdown().await;
}

/// Conversation remplie (modèle de vision) : message 1 avec notes.txt
/// (pièce jointe 1) et red.png (2), réponse 2, message 3 avec later.txt
/// (3), réponse 4.
async fn filled_chat(bench: &Bench) -> String {
    let mut chat_id = None;
    for (content, files) in [
        ("Describe these files.", &["notes.txt", "red.png"][..]),
        ("Again", &["later.txt"][..]),
    ] {
        let mut ids = Vec::new();
        for file in files {
            let data = sample_file(file).unwrap_or_else(|| b"Later".to_vec());
            ids.push(stage(&bench.staging, file, &data).await);
        }
        let mut ask = asking(chat_id.as_deref(), SEER, content);
        ask.attachment_ids = Some(ids);
        let recorder = bench.send(ask).await;
        assert_eq!(recorder.last()["type"], json!("done"));
        chat_id.get_or_insert_with(|| recorder.chat_id());
    }
    let chat_id = chat_id.unwrap();
    let chat = saved(bench.store(), &chat_id);
    let ids: Vec<_> = chat.messages.iter().map(|m| m.id).collect();
    assert_eq!(ids, [1, 2, 3, 4]);
    chat_id
}

/// Modification du message 1 : notes.txt gardé, `file` ajouté.
fn edit_of_first(chat_id: Option<&str>, file: &str) -> SendRequest {
    let mut edit = asking(chat_id, SEER, "Describe this file instead.");
    edit.edit_message_id = Some(1);
    edit.kept_attachment_ids = Some(vec![1]);
    edit.attachment_ids = Some(vec![file.to_owned()]);
    edit
}

#[tokio::test]
async fn editing_a_message_rewrites_it_and_cuts_the_history() {
    let bench = Bench::new(&[SEER]);
    let chat_id = filled_chat(&bench).await;
    let original = saved(bench.store(), &chat_id).messages[0].clone();
    let extra = stage(&bench.staging, "extra.md", b"# Extra").await;
    let recorder = bench.send(edit_of_first(Some(&chat_id), &extra)).await;
    let events = recorder.events();
    assert_eq!(
        types(&events),
        ["userMessage", "replyStarted", "content", "context", "done"]
    );

    // Le même message, texte et pièces jointes changés (red.png retiré),
    // date de création gardée.
    let user = &events[0]["message"];
    assert_eq!(user["id"], json!(1));
    assert_eq!(user["content"], json!("Describe this file instead."));
    assert_eq!(user["createdAt"], json!(original.created_at));
    assert_eq!(
        user["attachments"],
        json!([
            {"id": 1, "filename": "notes.txt", "kind": "text", "mime": "text/plain", "size": 13},
            {"id": 4, "filename": "extra.md", "kind": "text", "mime": "text/markdown", "size": 7},
        ])
    );
    assert!(bench.staging.get(&extra).is_none());

    // En base : le message modifié, puis la nouvelle réponse (identifiant
    // jamais utilisé) ; les messages 2 à 4 et leurs pièces jointes ont
    // disparu.
    let chat = saved(bench.store(), &chat_id);
    assert_eq!(chat.messages.len(), 2);
    assert_eq!(json!(chat.messages[0]), *user);
    assert_eq!(chat.messages[1].id, 5);
    assert_eq!(
        chat.messages[1].content,
        "Hello! You said: Describe this file instead.\n--- File: notes.txt ---\nLine 1\nLine 2\n--- End of notes.txt ---\n--- File: extra.md ---\n# Extra\n--- End of extra.md ---"
    );
    assert_eq!(recorder.last()["messageId"], json!(5));
    let attachments: i64 = bench
        .store()
        .blocking(|c| Ok(c.query_row("SELECT COUNT(*) FROM attachments", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(attachments, 2);

    // Le modèle reçoit l'historique coupé au message modifié, avec son
    // nouveau texte et ses fichiers, sans l'image retirée.
    let sent = bench.requests(SEER).await;
    assert_eq!(sent.len(), 3);
    assert_eq!(
        sent[2]["messages"],
        json!([{
            "role": "user",
            "content": "Describe this file instead.\n--- File: notes.txt ---\nLine 1\nLine 2\n--- End of notes.txt ---\n--- File: extra.md ---\n# Extra\n--- End of extra.md ---",
        }])
    );
    bench.shutdown().await;
}

#[tokio::test]
async fn edits_are_refused_before_anything_is_written() {
    let bench = Bench::new(&[SEER]);
    let store = bench.store();
    let chat_id = filled_chat(&bench).await;
    // Autre conversation : message 5 avec notes.txt (pièce jointe 4).
    let mut elsewhere = request(None, SEER, None);
    elsewhere.attachment_ids = Some(vec![stage(&bench.staging, "notes.txt", b"Other").await]);
    let other_id = bench.send(elsewhere).await.chat_id();
    assert_eq!(saved(store, &other_id).messages[0].id, 5);
    assert_eq!(saved(store, &other_id).messages[0].attachments[0].id, 4);

    let both = || (saved(store, &chat_id), saved(store, &other_id));
    let before = both();
    let requests = bench.requests(SEER).await.len();
    let file = stage(&bench.staging, "extra.md", b"# Extra").await;
    let edit = |message_id: Option<i64>, kept: Option<Vec<i64>>| {
        let mut edit = edit_of_first(Some(&chat_id), &file);
        edit.edit_message_id = message_id;
        edit.kept_attachment_ids = kept;
        edit
    };
    let mut without_chat = edit(Some(1), None);
    without_chat.chat_id = None;
    let mut empty = edit(Some(1), None);
    empty.content = " \n ".into();
    for (ask, expected) in [
        (
            edit(None, Some(vec![1])),
            "keptAttachmentIds needs editMessageId",
        ),
        (without_chat, "editMessageId needs chatId"),
        (edit(Some(99), None), "unknown message: 99"),
        (edit(Some(5), None), "message 5 is in another chat"),
        (edit(Some(2), None), "message 2 is not a user message"),
        // later.txt (message 3), puis la pièce jointe de l'autre conversation.
        (
            edit(Some(1), Some(vec![1, 3])),
            "attachment 3 is not in message 1",
        ),
        (
            edit(Some(1), Some(vec![4])),
            "attachment 4 is not in message 1",
        ),
        (
            edit(Some(1), Some(vec![2, 1, 2])),
            "attachment 2 is kept twice",
        ),
        (empty, "the message is empty"),
    ] {
        let (recorder, refused) = bench.send_result(ask).await;
        let Err(AppError::Invalid(message)) = refused else {
            panic!("{expected}: {refused:?}");
        };
        assert_eq!(message, expected);
        assert!(recorder.events().is_empty(), "{expected}");
        // Rien d'écrit, le fichier préparé reste dans la zone.
        assert_eq!(both(), before, "{expected}");
        assert_eq!(store.blocking(|c| chats::list(c)).unwrap().len(), 2);
        assert!(bench.staging.get(&file).is_some(), "{expected}");
    }
    assert_eq!(bench.requests(SEER).await.len(), requests);
    bench.shutdown().await;
}

#[tokio::test]
async fn editing_a_busy_chat_is_refused() {
    let bench = Bench::new(&[SEER, SLOW]);
    let chat_id = filled_chat(&bench).await;
    let (task, busy) = bench.spawn(request(Some(&chat_id), SLOW, None)).await;
    busy.wait_for("content", 1).await;
    let file = stage(&bench.staging, "extra.md", b"# Extra").await;
    let (refused_events, refused) = bench
        .send_result(edit_of_first(Some(&chat_id), &file))
        .await;
    let Err(AppError::Invalid(message)) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(message, "this chat is already waiting for an answer");
    assert!(refused_events.events().is_empty());
    assert!(bench.staging.get(&file).is_some());
    // Rien de coupé : message 1 intact, la réponse en cours (6) reste.
    let chat = saved(bench.store(), &chat_id);
    let ids: Vec<_> = chat.messages.iter().map(|m| m.id).collect();
    assert_eq!(ids, [1, 2, 3, 4, 5, 6]);
    assert_eq!(chat.messages[0].content, "Describe these files.");
    assert_eq!(chat.messages[0].attachments.len(), 2);
    bench.engine.cancel(&format!("stream-{SLOW}"));
    finished(task).await;
    bench.shutdown().await;
}

#[tokio::test]
async fn a_stop_asked_while_an_edit_is_checked_is_not_lost() {
    let bench = Bench::new(&[SEER]);
    let chat_id = filled_chat(&bench).await;
    let file = stage(&bench.staging, "extra.md", b"# Extra").await;
    let requests = bench.requests(SEER).await.len();
    let server = bench.server().await;
    // Base tenue par un autre fil : la vérification de la modification
    // attend, l'arrêt est demandé pendant ce temps.
    let (locked, is_locked) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let holder = {
        let store = bench.store().clone();
        std::thread::spawn(move || {
            store
                .blocking(|_| {
                    locked.send(()).unwrap();
                    released.recv().unwrap();
                    Ok(())
                })
                .unwrap();
        })
    };
    is_locked.recv().unwrap();
    let edit = edit_of_first(Some(&chat_id), &file);
    let stream_id = edit.stream_id.clone();
    let (channel, recorder) = recorder();
    let task = {
        let (store, engine, staging) = (
            bench.store().clone(),
            bench.engine.clone(),
            bench.staging.clone(),
        );
        tokio::spawn(async move { send(&store, &engine, &staging, server, edit, &channel).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    bench.engine.cancel(&stream_id);
    release.send(()).unwrap();
    holder.join().unwrap();
    task.await.unwrap().unwrap();
    let events = recorder.events();
    assert_one_final(&events);
    assert_eq!(types(&events).last().map(String::as_str), Some("cancelled"));
    assert_eq!(
        bench.requests(SEER).await.len(),
        requests,
        "no request for the edit"
    );
    bench.shutdown().await;
}

#[tokio::test]
async fn a_failed_edit_changes_nothing_but_uses_up_its_files() {
    let bench = Bench::new(&[SEER]);
    let chat_id = filled_chat(&bench).await;
    let before = saved(bench.store(), &chat_id);
    // Échec à la dernière écriture (la nouvelle pièce jointe) : toute la
    // transaction est annulée.
    bench
        .store()
        .blocking(|c| {
            c.execute_batch(
                "CREATE TEMP TRIGGER no_more_files BEFORE INSERT ON attachments
                 BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
            )?;
            Ok(())
        })
        .unwrap();
    let file = stage(&bench.staging, "extra.md", b"# Extra").await;
    let requests = bench.requests(SEER).await.len();
    let recorder = bench.send(edit_of_first(Some(&chat_id), &file)).await;
    let events = recorder.events();
    assert_eq!(types(&events), ["error"], "{events:?}");
    assert_eq!(events[0]["kind"], json!("internal"));
    assert_eq!(events[0]["message"], json!("database error: disk full"));
    assert_eq!(saved(bench.store(), &chat_id), before);
    // Envoi accepté : le fichier a quitté la zone (la page a vidé son
    // composer), aucune requête au modèle.
    assert!(bench.staging.get(&file).is_none());
    assert_eq!(bench.requests(SEER).await.len(), requests);
    bench.shutdown().await;
}

/// Noms des outils offerts par une requête (`None` : aucun).
fn offered(body: &Value) -> Option<Vec<String>> {
    body.get("tools").map(|tools| {
        tools
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap().to_owned())
            .collect()
    })
}

/// Outils offerts seulement si demandés et possibles : recherche web
/// demandée, modèle à outils, aucune pièce jointe (nouvelle ou gardée) ;
/// l'outil d'image si le peintre est prêt, pièces jointes ou non.
#[tokio::test]
async fn tools_are_offered_only_when_asked_for_and_possible() {
    let painter = FakePainter::ready();
    let bench = Bench::painting(&[QUIET, PLAIN], Some(painter.clone()));
    let web = ["web_search", "web_fetch"].map(str::to_owned).to_vec();
    let with_image = ["web_search", "web_fetch", "generate_image"]
        .map(str::to_owned)
        .to_vec();
    let image = vec!["generate_image".to_owned()];
    let plain = |model: &str, web_search: Option<bool>| SendRequest {
        web_search,
        ..request(None, model, None)
    };
    for (what, ask, expected) in [
        ("asked", plain(QUIET, Some(true)), Some(with_image.clone())),
        ("not asked", plain(QUIET, None), Some(image.clone())),
        ("turned off", plain(QUIET, Some(false)), Some(image.clone())),
        ("no tools capability", plain(PLAIN, Some(true)), None),
    ] {
        let model = ask.model.clone();
        let before = bench.requests(&model).await.len();
        bench.send(ask).await;
        let sent = bench.requests(&model).await;
        assert_eq!(offered(&sent[before]), expected, "{what}");
    }
    // Offrir l'outil ne détecte pas la carte graphique (`nvidia-smi`
    // jusqu'à 3 s à chaque envoi).
    assert_eq!(painter.cpu_asked(), 0);

    // Sans peintre : la recherche seule.
    let unpainted = Bench::new(&[QUIET]);
    unpainted.send(plain(QUIET, Some(true))).await;
    assert_eq!(offered(&unpainted.requests(QUIET).await[0]), Some(web));
    unpainted.shutdown().await;

    // Avec une pièce jointe (nouvelle, puis gardée par une modification) :
    // plus d'outils web, l'outil d'image reste ; sans plus aucune pièce
    // jointe : tous.
    let mut with_file = SendRequest {
        web_search: Some(true),
        ..asking(None, QUIET, "Describe these files.")
    };
    with_file.attachment_ids = Some(vec![
        stage(&bench.staging, "notes.txt", b"Line 1\nLine 2").await,
    ]);
    let before = bench.requests(QUIET).await.len();
    let first = bench.send(with_file).await;
    assert_eq!(
        offered(&bench.requests(QUIET).await[before]),
        Some(image.clone()),
        "new attachment"
    );
    let chat_id = first.chat_id();
    let message = saved(bench.store(), &chat_id).messages[0].clone();
    let attachment = message.attachments[0].id;
    for (kept, expected) in [(vec![attachment], image), (Vec::new(), with_image)] {
        let edit = SendRequest {
            edit_message_id: Some(message.id),
            kept_attachment_ids: Some(kept.clone()),
            web_search: Some(true),
            ..asking(Some(&chat_id), QUIET, "Search instead")
        };
        let before = bench.requests(QUIET).await.len();
        bench.send(edit).await;
        assert_eq!(
            offered(&bench.requests(QUIET).await[before]),
            Some(expected),
            "edit keeping {kept:?}"
        );
    }
    bench.shutdown().await;
}

#[tokio::test]
async fn tool_turns_stop_after_ten() {
    let painter = FakePainter::ready();
    let bench = Bench::painting(&[INSIST], Some(painter.clone()));
    let recorder = bench.send(asking(None, INSIST, "Draw forever")).await;
    assert_eq!(recorder.count("toolCalls"), 10);
    assert_eq!(recorder.count("toolResult"), 10);
    assert_eq!(recorder.count("replyStarted"), 11);
    // Dix requêtes avec les outils, la onzième sans.
    let requests = bench.requests(INSIST).await;
    assert_eq!(requests.len(), 11);
    for (index, sent) in requests.iter().enumerate() {
        assert_eq!(sent.get("tools").is_some(), index < 10, "request {index}");
    }
    // Quatre images au plus par envoi, les suivantes refusées au modèle.
    assert_eq!(painter.prompts().len(), 4);
    let chat = saved(bench.store(), &recorder.chat_id());
    assert_eq!(chat.messages.len(), 12);
    let last = chat.messages.last().unwrap();
    assert_eq!(last.content, scripted::INSIST_DONE);
    assert_eq!(recorder.last()["messageId"], json!(last.id));
    bench.shutdown().await;
}

/// Appels reçus d'une requête partie sans outils : ignorés (rien ne les
/// exécuterait) ; la réponse finit l'envoi.
#[tokio::test]
async fn calls_from_a_request_without_tools_are_ignored() {
    let bench = Bench::new(&[ROGUE]);
    let recorder = bench.send(request(None, ROGUE, None)).await;
    assert_eq!(recorder.count("toolCalls"), 0);
    assert_eq!(recorder.last()["type"], json!("done"));
    let chat = saved(bench.store(), &recorder.chat_id());
    assert_eq!(chat.messages.len(), 2);
    assert!(chat.messages[1].tool_calls.is_empty());
    assert_eq!(chat.messages[1].content, "Let me check.");
    assert_eq!(recorder.last()["messageId"], json!(chat.messages[1].id));
    assert_eq!(bench.requests(ROGUE).await.len(), 1);
    bench.shutdown().await;
}

/// La réponse vivante (superposée par `get_chat`) porte ses appels, puis
/// leurs résultats, au moment où ils sont annoncés.
#[tokio::test]
async fn the_live_reply_carries_its_tool_calls() {
    let bench = Bench::painting(&[TOOLED], Some(FakePainter::ready()));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (sink, live, chat_id) = (
        seen.clone(),
        bench.engine.clone(),
        Arc::new(Mutex::new(None)),
    );
    let channel = {
        let store = bench.store().clone();
        Channel::new(move |body| {
            let InvokeResponseBody::Json(text) = body else {
                return Ok(());
            };
            let event: Value = serde_json::from_str(&text).unwrap();
            if let Some(id) = event["chatId"].as_str() {
                *chat_id.lock() = Some(id.to_owned());
            }
            if matches!(event["type"].as_str(), Some("toolCalls" | "toolResult")) {
                let id = chat_id.lock().clone().unwrap();
                let mut chat = store.blocking(move |c| chats::get(c, &id)).unwrap();
                // Seule la superposition peut les remettre.
                for message in &mut chat.messages {
                    message.tool_calls.clear();
                }
                live.overlay(&mut chat);
                sink.lock().push(chat.messages[1].tool_calls.clone());
            }
            Ok(())
        })
    };
    send(
        bench.store(),
        &bench.engine,
        &bench.staging,
        bench.server().await,
        asking(None, TOOLED, "Draw this"),
        &channel,
    )
    .await
    .unwrap();
    let seen = seen.lock().clone();
    assert_eq!(seen.len(), 2);
    let arguments = json!({ "prompt": "Draw this" }).to_string();
    assert_eq!(seen[0], [ToolCall::new("generate_image", arguments)]);
    assert!(seen[1][0].result.is_some());
    bench.shutdown().await;
}

/// Nom du modèle d'images du peintre en mémoire.
const FAKE_IMAGE_MODEL: &str = "Fake image model";

/// Peintre en mémoire : prêt (ou refus imposé) ; avancement (chargement,
/// étape 1 avec un aperçu, étape 2), puis une image rouge, graine 7.
/// Description avec `[slow]` : ne finit jamais (seul l'arrêt la lâche) ;
/// avec `[fail]` : échec après la première étape.
struct FakePainter {
    refusal: Option<images::PaintRefusal>,
    cpu: bool,
    /// Images demandées, dans l'ordre.
    asked: Mutex<Vec<images::PaintRequest>>,
    /// Images lâchées avant leur fin.
    dropped: Arc<std::sync::atomic::AtomicUsize>,
    /// Carte graphique demandée (au moment d'une image seulement).
    cpu_asked: std::sync::atomic::AtomicUsize,
    /// Lancé une fois, quand une image vient d'être finie (juste avant de
    /// la rendre).
    on_done: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

/// Compte un futur lâché.
struct Dropped(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl FakePainter {
    fn with(refusal: Option<images::PaintRefusal>) -> Arc<Self> {
        Arc::new(Self {
            refusal,
            cpu: false,
            asked: Mutex::new(Vec::new()),
            dropped: Arc::default(),
            cpu_asked: std::sync::atomic::AtomicUsize::new(0),
            on_done: Mutex::new(None),
        })
    }

    fn ready() -> Arc<Self> {
        Self::with(None)
    }

    fn refusing(refusal: images::PaintRefusal) -> Arc<Self> {
        Self::with(Some(refusal))
    }

    fn ready_value(&self) -> Ready {
        Ready {
            model_name: FAKE_IMAGE_MODEL.into(),
            default_side: 1024,
        }
    }

    fn cpu_asked(&self) -> usize {
        self.cpu_asked.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn prompts(&self) -> Vec<String> {
        self.asked
            .lock()
            .iter()
            .map(|request| request.prompt.clone())
            .collect()
    }

    fn dropped(&self) -> usize {
        self.dropped.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Painter for FakePainter {
    fn ready(&self) -> images::PaintFuture<'_, Result<Ready, images::PaintRefusal>> {
        let ready = match &self.refusal {
            Some(refusal) => Err(refusal.clone()),
            None => Ok(self.ready_value()),
        };
        Box::pin(async move { ready })
    }

    fn cpu(&self) -> images::PaintFuture<'_, bool> {
        self.cpu_asked
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let cpu = self.cpu;
        Box::pin(async move { cpu })
    }

    fn paint<'a>(
        &'a self,
        request: images::PaintRequest,
        progress: &'a mut (dyn FnMut(ImageProgress) + Send),
    ) -> images::PaintFuture<'a, Result<images::Painted, String>> {
        Box::pin(async move {
            self.asked.lock().push(request.clone());
            let at = |stage| ImageProgress::at(stage, self.cpu, request.width, request.height);
            progress(at(ImageStage::Loading));
            progress(ImageProgress {
                step: Some(1),
                total: Some(2),
                preview: Some(data_url(&red_png())),
                ..at(ImageStage::Generating)
            });
            if request.prompt.contains("[slow]") {
                let _dropped = Dropped(self.dropped.clone());
                std::future::pending::<()>().await;
            }
            if request.prompt.contains("[fail]") {
                return Err("the image couldn't be created: fake failure".into());
            }
            progress(ImageProgress {
                step: Some(2),
                total: Some(2),
                ..at(ImageStage::Generating)
            });
            let on_done = self.on_done.lock().take();
            if let Some(on_done) = on_done {
                on_done();
            }
            Ok(images::Painted {
                png: red_png(),
                width: request.width,
                height: request.height,
                seed: 7,
                model_name: FAKE_IMAGE_MODEL.into(),
            })
        })
    }
}

/// Envoi du bouton « Image » (aucun modèle de langage choisi).
fn image_button(content: &str) -> SendRequest {
    SendRequest {
        image: Some(true),
        ..asking(None, "", content)
    }
}

#[tokio::test]
async fn the_image_tool_is_offered_only_when_the_painter_is_ready() {
    for (what, painter, model, expected) in [
        ("ready", Some(FakePainter::ready()), TOOLED, true),
        (
            "no tools capability",
            Some(FakePainter::ready()),
            PLAIN,
            false,
        ),
        (
            "not ready",
            Some(FakePainter::refusing(images::PaintRefusal::NoModel)),
            TOOLED,
            false,
        ),
        ("no painter", None, TOOLED, false),
    ] {
        let bench = Bench::painting(&[model], painter);
        let recorder = bench.send(asking(None, model, "Draw a fox")).await;
        assert_eq!(recorder.last()["type"], json!("done"), "{what}");
        let sent = bench.requests(model).await;
        assert_eq!(
            sent[0].get("tools"),
            expected.then(|| json!([images::definition()])).as_ref(),
            "{what}"
        );
        assert_eq!(
            recorder.count("imageReady"),
            usize::from(expected),
            "{what}"
        );
        bench.shutdown().await;
    }
}

#[tokio::test]
async fn an_image_call_shows_its_progress_then_the_image_then_the_result() {
    let painter = FakePainter::ready();
    let bench = Bench::painting(&[SKETCH, PLAIN], Some(painter.clone()));
    let recorder = bench.send(asking(None, SKETCH, "Draw a red fox")).await;
    let events = recorder.events();
    assert_eq!(
        types(&events),
        [
            "chatCreated",
            "userMessage",
            "loading",
            "loaded",
            "replyStarted",
            "thinking",
            "context",
            "toolCalls",
            "imageProgress",
            "imageReady",
            "toolResult",
            "replyStarted",
            "thinking",
            "content",
            "context",
            "done"
        ]
    );
    // Taille demandée par le modèle (512 × 512).
    assert_eq!(
        *painter.asked.lock(),
        [images::PaintRequest {
            prompt: "Draw a red fox".into(),
            width: 512,
            height: 512
        }]
    );
    // Carte graphique demandée au moment de l'image seulement.
    assert_eq!(painter.cpu_asked(), 1);
    let chat = saved(bench.store(), &recorder.chat_id());
    assert_eq!(chat.messages.len(), 3);
    let (calls, answer) = (&chat.messages[1], &chat.messages[2]);
    // Pièce jointe de la réponse qui a fait l'appel.
    assert_eq!(calls.attachments.len(), 1);
    let attachment = &calls.attachments[0];
    assert_eq!(
        (attachment.filename.as_str(), attachment.mime.as_str()),
        ("image-7.png", "image/png")
    );
    let result = format!(
        r#"{{"attachmentId":{},"width":512,"height":512,"seed":7,"model":"Fake image model"}}"#,
        attachment.id
    );
    let call = &calls.tool_calls[0];
    assert_eq!(call.name, "generate_image");
    assert_eq!(
        (call.result.as_deref(), call.error.as_deref()),
        (Some(result.as_str()), None)
    );
    assert_eq!(answer.content, scripted::SKETCH_DONE);

    // Avancement : place réservée d'abord, puis celui du peintre, tous pour
    // cet appel ; l'image annoncée avant le résultat.
    let progress = of_type(&events, "imageProgress");
    assert_eq!(progress.len(), 4);
    assert!(
        progress
            .iter()
            .all(|event| event["messageId"] == json!(calls.id) && event["index"] == json!(0))
    );
    assert_eq!(
        progress[0]["progress"],
        json!({ "stage": "starting", "step": null, "total": null, "preview": null,
                "cpu": false, "width": 512, "height": 512 })
    );
    assert_eq!(progress[1]["progress"]["stage"], json!("loading"));
    assert_eq!(progress[2]["progress"]["step"], json!(1));
    assert!(
        progress[2]["progress"]["preview"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
    assert_eq!(
        of_type(&events, "imageReady"),
        [json!({
            "type": "imageReady",
            "messageId": calls.id,
            "index": 0,
            "attachment": serde_json::to_value(attachment).unwrap(),
        })]
    );
    let position = |kind: &str| events.iter().position(|e| e["type"] == kind).unwrap();
    assert!(position("imageReady") < position("toolResult"));
    assert_eq!(of_type(&events, "toolResult")[0]["result"], json!(result));

    // Le tour suivant reçoit le résultat, jamais l'image elle-même.
    let requests = bench.requests(SKETCH).await;
    assert_eq!(requests.len(), 2);
    let messages = &requests[1]["messages"];
    assert_eq!(messages[1]["role"], json!("assistant"));
    assert_eq!(
        messages[1]["tool_calls"][0]["function"]["name"],
        json!("generate_image")
    );
    let call_id = messages[1]["tool_calls"][0]["id"].clone();
    assert_eq!(
        messages[2],
        json!({ "role": "tool", "content": result, "tool_call_id": call_id, "name": "generate_image" })
    );
    assert_eq!(messages.as_array().unwrap().len(), 3);
    // Un envoi suivant non plus (aucune partie image).
    let ask = asking(Some(&chat.id), PLAIN, "Thanks");
    bench.send(ask).await;
    let last = bench.requests(PLAIN).await.pop().unwrap();
    assert!(!last.to_string().contains("image_url"), "{last}");
    bench.shutdown().await;
}

#[tokio::test]
async fn image_errors_go_to_the_model() {
    let bench = Bench::painting(&[SKETCH], Some(FakePainter::ready()));
    let recorder = bench.send(asking(None, SKETCH, "Draw a fox [fail]")).await;
    let events = recorder.events();
    assert_eq!(recorder.last()["type"], json!("done"));
    assert_eq!(recorder.count("imageReady"), 0);
    let failure = "the image couldn't be created: fake failure";
    let results = of_type(&events, "toolResult");
    assert_eq!(
        (&results[0]["result"], &results[0]["error"]),
        (&Value::Null, &json!(failure))
    );
    let chat = saved(bench.store(), &recorder.chat_id());
    assert!(chat.messages[1].attachments.is_empty());
    assert_eq!(chat.messages[2].content, scripted::SKETCH_DONE);
    let sent = &bench.requests(SKETCH).await[1]["messages"][2];
    assert_eq!(sent["role"], json!("tool"));
    assert_eq!(sent["content"], json!(format!("Error: {failure}")));
    bench.shutdown().await;
}

/// Conversation et réponse prêtes pour `run_tools` (sans modèle de
/// langage).
fn writer(store: &Store) -> ReplyWriter {
    let chat_id = store.blocking(|c| chats::create(c, now_ms())).unwrap();
    let id = chat_id.clone();
    store
        .blocking(move |c| chats::add_user_message(c, &id, "Draw", &[], now_ms()))
        .unwrap();
    ReplyWriter {
        chat_id,
        model: "m".into(),
        message_id: None,
        previous: None,
        reply: Reply::default(),
        last_save: Instant::now(),
        think_off: false,
        always_thinks_noted: false,
        loops: LoopWatch::default(),
    }
}

/// Appels vérifiés un à un (description, limite de 4 par envoi, outils non
/// offerts), sans modèle de langage : directement par `run_tools`.
#[tokio::test]
async fn image_calls_are_checked_and_limited_to_four_per_send() {
    let painter = FakePainter::ready();
    let bench = Bench::painting(&[], Some(painter.clone()));
    let store = bench.store();
    let web_tools = bench.server().await.web_tools();
    let mut reply = writer(store);
    let chat_id = reply.chat_id.clone();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let emit = move |event: ChatEvent| sink.lock().push(event);
    let conversation = Conversation {
        store,
        engine: &bench.engine,
        emit: &emit,
        origin: Origin::Page,
    };
    let call = |name: &str, arguments: Value| StreamToolCall {
        name: name.into(),
        arguments,
    };
    let draw = |n: usize| {
        call(
            images::TOOL_NAME,
            json!({ "prompt": format!("picture {n}") }),
        )
    };
    let (_stop, mut cancel) = watch::channel(false);
    let mut allowed = AllowedUrls::default();
    let mut images = ImageCalls {
        ready: Some(painter.ready_value()),
        count: 0,
    };
    let calls = [
        draw(1),
        call(images::TOOL_NAME, json!({ "prompt": "  " })),
        draw(2),
        draw(3),
        call(images::TOOL_NAME, json!({ "width": 512 })),
        draw(4),
        draw(5),
        // Recherche web pas offerte : refusée sans requête.
        call(tools::WEB_SEARCH, json!({ "query": "fox" })),
        call(tools::WEB_FETCH, json!({ "url": "https://example.com" })),
    ];
    let ran = conversation
        .run_tools(
            &web_tools,
            &calls,
            &mut allowed,
            false,
            &mut images,
            &mut reply,
            &mut cancel,
        )
        .await;
    assert!(ran.is_ok());
    let no_prompt = "The image description (prompt) is missing or empty.";
    let too_many = "At most 4 images can be created per message.";
    let errors = |message: usize| -> Vec<Option<String>> {
        saved(store, &chat_id).messages[message]
            .tool_calls
            .iter()
            .map(|call| call.error.clone())
            .collect()
    };
    assert_eq!(
        errors(1),
        [
            None,
            Some(no_prompt.into()),
            None,
            None,
            Some(no_prompt.into()),
            None,
            Some(too_many.into()),
            Some("unknown tool: web_search".into()),
            Some("unknown tool: web_fetch".into()),
        ]
    );
    assert_eq!(
        painter.prompts(),
        ["picture 1", "picture 2", "picture 3", "picture 4"]
    );
    assert_eq!(saved(store, &chat_id).messages[1].attachments.len(), 4);

    // La limite vaut pour tout l'envoi (tour suivant compris).
    reply.next_turn();
    let ran = conversation
        .run_tools(
            &web_tools,
            &[draw(6)],
            &mut allowed,
            false,
            &mut images,
            &mut reply,
            &mut cancel,
        )
        .await;
    assert!(ran.is_ok());
    assert_eq!(errors(2), [Some(too_many.into())]);
    // Outil d'image non offert : inconnu.
    reply.next_turn();
    let mut not_offered = ImageCalls {
        ready: None,
        count: 0,
    };
    let ran = conversation
        .run_tools(
            &web_tools,
            &[draw(7)],
            &mut allowed,
            false,
            &mut not_offered,
            &mut reply,
            &mut cancel,
        )
        .await;
    assert!(ran.is_ok());
    assert_eq!(errors(3), [Some("unknown tool: generate_image".into())]);
    assert_eq!(painter.prompts().len(), 4);
}

#[tokio::test]
async fn stopping_during_an_image_drops_it() {
    let painter = FakePainter::ready();
    let bench = Bench::painting(&[TOOLED], Some(painter.clone()));
    let (task, recorder) = bench
        .spawn(asking(None, TOOLED, "Draw slowly [slow]"))
        .await;
    recorder.wait_for("imageProgress", 3).await;
    assert_eq!(painter.dropped(), 0);
    // Pendant l'outil : le modèle reste tenu par l'envoi (le crochet des
    // images ne libère que les modèles inactifs).
    assert!(bench.fake.engine().running()[0].in_use);
    let freed = bench.fake.engine().release_idle().await;
    assert_eq!(freed, 0);
    // L'appel, sans résultat, en base comme dans la réponse vivante.
    let chat_id = recorder.chat_id();
    let waiting = vec![ToolCall::new(
        "generate_image",
        json!({ "prompt": "Draw slowly [slow]" }).to_string(),
    )];
    let mut chat = saved(bench.store(), &chat_id);
    assert_eq!(chat.messages[1].tool_calls, waiting);
    chat.messages[1].tool_calls.clear();
    bench.engine.overlay(&mut chat);
    assert_eq!(chat.messages[1].tool_calls, waiting);

    let asked = Instant::now();
    bench.engine.cancel(&format!("stream-{TOOLED}"));
    finished(task).await;
    assert!(asked.elapsed() < Duration::from_secs(1));
    assert_one_final(&recorder.events());
    assert_eq!(recorder.last()["type"], json!("cancelled"));
    assert_eq!(recorder.count("toolResult"), 0);
    assert_eq!(recorder.count("imageReady"), 0);
    assert_eq!(painter.dropped(), 1);
    let chat = saved(bench.store(), &chat_id);
    let call = &chat.messages[1].tool_calls[0];
    assert_eq!(
        (call.result.as_deref(), call.error.as_deref()),
        (None, None)
    );
    assert!(chat.messages[1].attachments.is_empty());
    assert_eq!(bench.requests(TOOLED).await.len(), 1);
    bench.shutdown().await;
}

/// Arrêt demandé pendant que l'image finissait : ni enregistrée, ni
/// annoncée (un bot d'Easy Link l'enverrait encore), appel sans résultat.
#[tokio::test]
async fn a_stop_while_the_image_finishes_keeps_it_out() {
    let painter = FakePainter::ready();
    let bench = Bench::painting(&[], Some(painter.clone()));
    let store = bench.store();
    let web_tools = bench.server().await.web_tools();
    let (stop, mut cancel) = watch::channel(false);
    let stop = Arc::new(stop);
    let stopper = stop.clone();
    *painter.on_done.lock() = Some(Box::new(move || {
        let _ = stopper.send(true);
    }));
    let mut reply = writer(store);
    let chat_id = reply.chat_id.clone();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let emit = move |event: ChatEvent| sink.lock().push(serde_json::to_value(&event).unwrap());
    let conversation = Conversation {
        store,
        engine: &bench.engine,
        emit: &emit,
        origin: Origin::Remote,
    };
    let draw = StreamToolCall {
        name: images::TOOL_NAME.into(),
        arguments: json!({ "prompt": "a fox" }),
    };
    let mut images = ImageCalls {
        ready: Some(painter.ready_value()),
        count: 0,
    };
    let ran = conversation
        .run_tools(
            &web_tools,
            &[draw],
            &mut AllowedUrls::default(),
            false,
            &mut images,
            &mut reply,
            &mut cancel,
        )
        .await;
    assert!(matches!(ran, Err(Outcome::Cancelled)));
    // Image finie (pas lâchée), mais gardée hors de la conversation.
    assert_eq!(painter.prompts(), ["a fox"]);
    assert_eq!(painter.dropped(), 0);
    let events = events.lock().clone();
    assert_eq!(of_type(&events, "imageReady"), Vec::<Value>::new());
    assert_eq!(of_type(&events, "toolResult"), Vec::<Value>::new());
    let chat = saved(store, &chat_id);
    assert!(chat.messages[1].attachments.is_empty());
    let call = &chat.messages[1].tool_calls[0];
    assert_eq!(
        (call.result.as_deref(), call.error.as_deref()),
        (None, None)
    );
}

/// Pas d'outil d'image pour un envoi du mode vocal : chaque phrase
/// prononcée arrêterait l'image en cours sans prévenir.
#[tokio::test]
async fn a_voice_send_is_not_offered_the_image_tool() {
    let painter = FakePainter::ready();
    let bench = Bench::painting(&[TOOLED], Some(painter.clone()));
    let spoken = SendRequest {
        voice: Some(VoiceSend::default()),
        ..asking(None, TOOLED, "Draw a fox")
    };
    let recorder = bench.send(spoken).await;
    assert_eq!(recorder.last()["type"], json!("done"));
    assert_eq!(bench.requests(TOOLED).await[0].get("tools"), None);
    assert_eq!(recorder.count("imageReady"), 0);
    assert!(painter.prompts().is_empty());
    assert_eq!(painter.cpu_asked(), 0);
    bench.shutdown().await;
}

/// Un modèle sans la capacité `tools` (ses gabarits ignorent les appels et
/// les messages `tool`) apprend en texte qu'une image a été faite, ou non ;
/// un modèle à outils reçoit toujours les appels.
#[tokio::test]
async fn a_model_without_tools_is_told_about_images_in_text() {
    let bench = Bench::painting(&[PLAIN, QUIET], Some(FakePainter::ready()));
    let first = bench.send(image_button("A red fox")).await;
    let chat_id = first.chat_id();
    let in_chat = |ask: SendRequest| SendRequest {
        chat_id: Some(chat_id.clone()),
        ..ask
    };
    bench.send(in_chat(image_button("A blue fox [fail]"))).await;
    bench.send(in_chat(asking(None, PLAIN, "Thanks"))).await;
    let sent = bench.requests(PLAIN).await.pop().unwrap();
    assert_eq!(
        sent["messages"],
        json!([
            {"role": "user", "content": "A red fox"},
            {"role": "assistant", "content": "[Image created: A red fox]"},
            {"role": "user", "content": "A blue fox [fail]"},
            {"role": "assistant", "content": "[Image not created: the image couldn't be created: fake failure]"},
            {"role": "user", "content": "Thanks"},
        ])
    );
    // Modèle à outils : les appels et les messages `tool`, sans texte ajouté.
    bench.send(in_chat(asking(None, QUIET, "Again"))).await;
    let sent = bench.requests(QUIET).await.pop().unwrap();
    let messages = sent["messages"].as_array().unwrap();
    assert_eq!(messages[1]["content"], json!(""));
    assert_eq!(
        messages[1]["tool_calls"][0]["function"]["name"],
        json!("generate_image")
    );
    assert_eq!(messages[2]["role"], json!("tool"));
    bench.shutdown().await;
}

/// Un bot d'Easy Link reçoit aussi l'outil d'image.
#[tokio::test]
async fn a_remote_send_is_offered_the_image_tool() {
    let bench = Bench::painting(&[TOOLED], Some(FakePainter::ready()));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink_seen = seen.clone();
    let sink = move |event: ChatEvent| {
        sink_seen
            .lock()
            .push(serde_json::to_value(&event).unwrap()["type"].clone());
    };
    send_to(
        bench.store(),
        &bench.engine,
        &bench.staging,
        bench.server().await,
        asking(None, TOOLED, "Draw a fox"),
        Origin::Remote,
        &sink,
    )
    .await
    .unwrap();
    assert_eq!(
        bench.requests(TOOLED).await[0]["tools"],
        json!([images::definition()])
    );
    let seen = seen.lock().clone();
    assert!(seen.contains(&json!("imageReady")), "{seen:?}");
    assert_eq!(seen.last(), Some(&json!("done")));
    bench.shutdown().await;
}

#[tokio::test]
async fn the_image_button_paints_without_asking_the_engine() {
    let painter = FakePainter::ready();
    // Aucun modèle de langage : la moindre requête au moteur finirait en
    // erreur.
    let bench = Bench::painting(&[], Some(painter.clone()));
    let text = "A lighthouse at dusk, oil painting";
    // Raisonnement et recherche demandés : sans effet ici.
    let ask = SendRequest {
        think: Some(Think::Enabled(true)),
        web_search: Some(true),
        ..image_button(text)
    };
    let recorder = bench.send(ask).await;
    let events = recorder.events();
    assert_eq!(
        types(&events),
        [
            "chatCreated",
            "userMessage",
            "replyStarted",
            "toolCalls",
            "imageProgress",
            "imageReady",
            "toolResult",
            "done"
        ]
    );
    assert_eq!(
        *painter.asked.lock(),
        [images::PaintRequest {
            prompt: text.into(),
            width: 1024,
            height: 1024
        }]
    );
    let chat = saved(bench.store(), &recorder.chat_id());
    assert_eq!(chat.messages.len(), 2);
    assert_eq!(chat.messages[0].content, text);
    let reply = &chat.messages[1];
    assert_eq!(reply.content, "");
    // Modèle de l'envoi gardé tel quel (aucun ici).
    assert_eq!(reply.model.as_deref(), Some(""));
    assert_eq!(reply.attachments.len(), 1);
    let call = &reply.tool_calls[0];
    assert_eq!(
        (call.name.as_str(), call.arguments.as_str()),
        (
            "generate_image",
            json!({ "prompt": text }).to_string().as_str()
        )
    );
    assert!(call.result.is_some());
    assert_eq!(recorder.last()["messageId"], json!(reply.id));
    assert!(bench.fake.engine().running().is_empty());

    // Échec de l'image : l'envoi finit quand même par `done`, l'erreur reste
    // sur l'appel.
    let ask = SendRequest {
        chat_id: Some(chat.id.clone()),
        ..image_button("A fox [fail]")
    };
    let recorder = bench.send(ask).await;
    assert_eq!(recorder.last()["type"], json!("done"));
    let failed = saved(bench.store(), &chat.id).messages[3].tool_calls[0].clone();
    assert_eq!(
        failed.error.as_deref(),
        Some("the image couldn't be created: fake failure")
    );
    bench.shutdown().await;
}

#[tokio::test]
async fn the_image_button_is_refused_before_anything_is_written() {
    let with_files =
        "An image can't be created from attached files: remove them, or turn off Image.";
    let cpu = images::PaintRefusal::CpuUnsupported("AVX2".into());
    for (what, painter, expected) in [
        (
            "no image model",
            Some(FakePainter::refusing(images::PaintRefusal::NoModel)),
            images::PaintRefusal::NoModel.message(),
        ),
        (
            "no engine",
            Some(FakePainter::refusing(images::PaintRefusal::Unavailable)),
            images::PaintRefusal::Unavailable.message(),
        ),
        (
            "processor",
            Some(FakePainter::refusing(cpu.clone())),
            cpu.message(),
        ),
        (
            "no painter",
            None,
            images::PaintRefusal::Unavailable.message(),
        ),
        ("attachment", Some(FakePainter::ready()), with_files.into()),
        (
            "kept attachment",
            Some(FakePainter::ready()),
            with_files.into(),
        ),
    ] {
        let bench = Bench::painting(&[], painter);
        let file = stage(&bench.staging, "notes.txt", b"A fox").await;
        let ask = match what {
            "attachment" => SendRequest {
                attachment_ids: Some(vec![file.clone()]),
                ..image_button("Draw this")
            },
            "kept attachment" => SendRequest {
                chat_id: Some("chat".into()),
                edit_message_id: Some(1),
                kept_attachment_ids: Some(vec![1]),
                ..image_button("Draw this")
            },
            _ => image_button("Draw a fox"),
        };
        let (recorder, result) = bench.send_result(ask).await;
        assert!(
            matches!(&result, Err(AppError::Invalid(message)) if *message == expected),
            "{what}: {result:?}"
        );
        assert!(recorder.events().is_empty(), "{what}");
        assert!(
            bench
                .store()
                .blocking(|c| chats::list(c))
                .unwrap()
                .is_empty()
        );
        assert!(bench.staging.get(&file).is_some(), "{what}");
    }
}

/// Horodatage unique des séquences exportées.
const FIXED_TIME: i64 = 1_700_000_000_000;

/// Envoi rejoué par le simulateur de la page.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SequenceRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    chat_id: Option<&'static str>,
    stream_id: &'static str,
    content: String,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<Think>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachment_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edit_message_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kept_attachment_ids: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    web_search: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ephemeral: Option<bool>,
}

#[derive(serde::Serialize)]
struct Sequence {
    description: &'static str,
    request: SequenceRequest,
    /// Événements tels qu'envoyés au canal.
    events: Value,
    /// Erreur de la commande refusée, telle que reçue par la page.
    error: Value,
}

/// Rend une valeur identique d'une exécution à l'autre : conversations
/// « chat-1 », flux « stream-1 », horodatages fixes, textes remplacés
/// (identifiant de conversation, dossier du banc).
fn normalize(value: &mut Value, replace: &[(String, &str)]) {
    match value {
        Value::Object(fields) => {
            for (key, field) in fields.iter_mut() {
                if key == "chatId" && field.is_string() {
                    *field = json!("chat-1");
                } else if key == "streamId" && field.is_string() {
                    *field = json!("stream-1");
                } else if (key.ends_with("At") || key.ends_with("_at")) && field.is_number() {
                    *field = json!(FIXED_TIME);
                } else {
                    normalize(field, replace);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize(item, replace);
            }
        }
        Value::String(text) => {
            for (from, to) in replace {
                if text.contains(from.as_str()) {
                    *text = text.replace(from.as_str(), to);
                }
            }
        }
        _ => {}
    }
}

/// Progression d'un téléchargement ramenée à une forme fixe (la file la
/// lit à son rythme : le nombre et les valeurs des étapes changent d'une
/// exécution à l'autre) : début à 0, moitié, puis la fin telle quelle ;
/// arrêté avant la fin : début et moitié de la taille reçue en dernier.
fn steady_downloads(events: Vec<Value>) -> Vec<Value> {
    let downloads: Vec<&Value> = events.iter().filter(|e| e["type"] == "download").collect();
    let Some(last) = downloads.last() else {
        return events;
    };
    let total = last["total"].as_u64().unwrap();
    let step = |completed: u64, done: bool| json!({ "type": "download", "total": total, "completed": completed, "done": done });
    let mut steady = vec![step(0, false), step(total / 2, false)];
    if last["done"] == true {
        steady.push(step(total, true));
    }
    let mut out = Vec::new();
    let mut placed = false;
    for event in events {
        if event["type"] == "download" {
            if !placed {
                out.extend(steady.clone());
                placed = true;
            }
        } else {
            out.push(event);
        }
    }
    out
}

/// Séquence normalisée, en JSON.
fn sequence_text(
    description: &'static str,
    request: &SendRequest,
    events: Vec<Value>,
    result: &AppResult<()>,
    replace: &[(String, &str)],
) -> String {
    let mut events = Value::Array(steady_downloads(events));
    normalize(&mut events, replace);
    let mut error = match result {
        Ok(()) => Value::Null,
        Err(error) => serde_json::to_value(error).unwrap(),
    };
    normalize(&mut error, replace);
    let sequence = Sequence {
        description,
        request: SequenceRequest {
            chat_id: request.chat_id.as_ref().map(|_| "chat-1"),
            stream_id: "stream-1",
            content: request.content.clone(),
            model: request.model.clone(),
            think: request.think.clone(),
            attachment_ids: request.attachment_ids.as_ref().map(|ids| {
                ids.iter()
                    .map(|id| match replace.iter().find(|(from, _)| from == id) {
                        Some((_, to)) => (*to).to_owned(),
                        None => id.clone(),
                    })
                    .collect()
            }),
            edit_message_id: request.edit_message_id,
            kept_attachment_ids: request.kept_attachment_ids.clone(),
            web_search: request.web_search,
            image: request.image,
            ephemeral: request.ephemeral,
        },
        events,
        error,
    };
    let mut text = serde_json::to_string_pretty(&sequence).unwrap();
    text.push('\n');
    text
}

/// Écrit une séquence dans `$CHATONLM_FIXTURES_DIR/chat/<name>.json`
/// (src/test/fixtures, voir .cargo/config.toml) ; rien sans la variable.
fn export_sequence(name: &str, text: &str) {
    let Some(dir) = std::env::var_os("CHATONLM_FIXTURES_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir).join("chat");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.json")), text).unwrap();
}

/// Envoi complet sur ce serveur, annulé après `cancel_after` événements du
/// type donné si demandé : événements reçus et résultat de la commande.
async fn play(
    bench: &Bench,
    server: Server,
    request: &SendRequest,
    cancel_after: Option<(&str, usize)>,
) -> (Vec<Value>, AppResult<()>) {
    let (channel, recorder) = recorder();
    let sending = send(
        bench.store(),
        &bench.engine,
        &bench.staging,
        server,
        request.clone(),
        &channel,
    );
    let cancelling = async {
        if let Some((kind, count)) = cancel_after {
            recorder.wait_for(kind, count).await;
            bench.engine.cancel(&request.stream_id);
        }
    };
    let both = async { tokio::join!(sending, cancelling) };
    let (result, ()) = tokio::time::timeout(WAIT, both)
        .await
        .unwrap_or_else(|_| panic!("{}: still running after {WAIT:?}", request.model));
    (recorder.events(), result)
}

/// Cas d'une séquence : nom, description, envoi, annulation (après tant
/// d'événements d'un type), types d'événements attendus (regroupés).
type Case<'a> = (
    &'a str,
    &'static str,
    SendRequest,
    Option<(&'a str, usize)>,
    Vec<&'a str>,
);

/// Séquences d'événements rejouées par le simulateur des essais de la page
/// (src/test/fixtures/chat) : chacune jouée deux fois sur un banc neuf, et
/// identique d'une fois à l'autre.
#[tokio::test(flavor = "multi_thread")]
async fn sequences_for_the_simulator() {
    let ask = |content: &str, model: &str, think: Option<Think>| SendRequest {
        chat_id: None,
        stream_id: "stream-1".into(),
        content: content.into(),
        model: model.into(),
        think,
        attachment_ids: None,
        edit_message_id: None,
        kept_attachment_ids: None,
        web_search: None,
        voice: None,
        image: None,
        ephemeral: None,
    };
    let hello = "Say hello in one word.";
    // Fichiers préparés avant l'envoi, désignés par leur nom dans
    // `attachment_ids` ; un autre nom reste tel quel (inconnu).
    let with_files = |names: &[&str], request: SendRequest| SendRequest {
        attachment_ids: Some(names.iter().map(|name| (*name).to_owned()).collect()),
        ..request
    };
    let describe = "Describe these files.";
    // Modification du message 1 de la conversation remplie par l'envoi de la
    // séquence « attachments ».
    let editing = |message_id: i64, kept: Option<Vec<i64>>, request: SendRequest| SendRequest {
        edit_message_id: Some(message_id),
        kept_attachment_ids: kept,
        ..request
    };
    let instead = "Describe this file instead.";
    let fox = "A red fox in the snow, watercolor";
    // Bouton « Image » : la page envoie son modèle choisi, que le bouton
    // n'emploie pas (rien n'est demandé au moteur de texte).
    let button = |content: &str| SendRequest {
        image: Some(true),
        ..ask(content, THINKER, None)
    };
    let remote = "@remote/openrouter/qwen/qwen3-32b";
    let downloaded = "chatonlm-dl/Tiny-slow-GGUF:Q4_K_M";
    let stalled = "chatonlm-dl/Tiny-stall-GGUF:Q4_K_M";
    let answered = |head: &[&'static str]| {
        let mut types = head.to_vec();
        types.extend(["replyStarted", "content", "context", "done"]);
        types
    };
    let fresh = ["chatCreated", "userMessage", "loading", "loaded"];

    // Nom, description, envoi, annulation, types d'événements attendus
    // (regroupés).
    let cases: Vec<Case> = vec![
        (
            "new-chat-thinking",
            "New chat with a thinking model (think left to the model's default), not loaded yet: loading (instant), thinking, then the answer.",
            ask(hello, THINKER, None),
            None,
            [
                &fresh[..],
                &["replyStarted", "thinking", "content", "context", "done"],
            ]
            .concat(),
        ),
        (
            "new-chat-ephemeral",
            "New ephemeral chat (ChatonLM's own): created outside the history, then the same answer as a new chat.",
            SendRequest {
                ephemeral: Some(true),
                ..ask(hello, THINKER, None)
            },
            None,
            [
                &fresh[..],
                &["replyStarted", "thinking", "content", "context", "done"],
            ]
            .concat(),
        ),
        (
            "existing-chat",
            "Second message in an existing chat, thinking turned off (the model is already loaded).",
            ask(hello, THINKER, Some(Think::Enabled(false))),
            None,
            answered(&["userMessage"]),
        ),
        (
            "cancelled",
            "Answer stopped by the user after two pieces; the partial answer is kept.",
            ask(hello, SLOW, None),
            Some(("content", 2)),
            [&fresh[..], &["replyStarted", "content", "cancelled"]].concat(),
        ),
        (
            "model-error",
            "The model fails in the middle of its answer (error block in the stream).",
            ask(hello, FAILING, None),
            None,
            [&fresh[..], &["replyStarted", "content", "error"]].concat(),
        ),
        (
            "compacted-auto",
            "Existing chat past 80% of its context (3,309 of 4,096 tokens): the earlier messages are summarized first (compacting, compacted), then the answer, then the context used.",
            ask(hello, GIST, None),
            None,
            answered(&["userMessage", "compacting", "compacted"]),
        ),
        (
            "context-full",
            "Existing chat at 59% of its context (2,409 of 4,096 tokens): the answer is limited to what is left of the context, cut there (finish_reason length), and the send ends with a contextFull error.",
            ask("Tell me a story at length.", GIST, None),
            None,
            vec!["userMessage", "replyStarted", "content", "context", "error"],
        ),
        (
            "loop-stopped",
            "The model's thinking repeats the same paragraph word for word: ChatonLM stops the reply.",
            ask(hello, MULLING, None),
            None,
            [&fresh[..], &["replyStarted", "thinking", "error"]].concat(),
        ),
        (
            "connection-lost",
            "The engine stops in the middle of the answer (the llama-server process crashed).",
            ask(hello, CRASH, None),
            None,
            [&fresh[..], &["replyStarted", "content", "error"]].concat(),
        ),
        (
            "engine-failed",
            "The engine can't load the model (llama-server stops while loading): loading, loaded, then an engineFailed error.",
            ask(hello, LOADFAIL, None),
            None,
            [&fresh[..], &["error"]].concat(),
        ),
        (
            "engine-unavailable",
            "No engine in the installation (llama-server missing): an engineUnavailable error, nothing is loaded.",
            ask(hello, PLAIN, None),
            None,
            vec!["chatCreated", "userMessage", "error"],
        ),
        (
            "model-missing",
            "A model imported from Ollama that is no longer installed: a modelMissing error, nothing is downloaded.",
            ask(hello, IMPORTED, None),
            None,
            vec!["chatCreated", "userMessage", "error"],
        ),
        (
            "remote-off",
            "Remote model while remote models are turned off in the settings: the send stops with a remoteOff error, nothing is asked.",
            ask(hello, remote, None),
            None,
            vec!["chatCreated", "userMessage", "error"],
        ),
        (
            "refused-empty",
            "A message made only of spaces is refused: the command fails, no event.",
            ask("   ", THINKER, None),
            None,
            vec![],
        ),
        (
            "download",
            "The model isn't installed: it is downloaded from Hugging Face by the ModelHub's queue (progress of all its files, normalized: 0, half, then done), loaded, then it answers.",
            ask(hello, downloaded, None),
            None,
            answered(&[
                "chatCreated",
                "userMessage",
                "download",
                "loading",
                "loaded",
            ]),
        ),
        (
            "download-offline",
            "The model isn't installed and Hugging Face can't be reached: the queue's error, before any progress.",
            ask(hello, downloaded, None),
            None,
            vec!["chatCreated", "userMessage", "error"],
        ),
        (
            "download-cancelled",
            "Download stopped by the user after two progress events (normalized); the download is cancelled, no answer is created.",
            ask(hello, stalled, None),
            Some(("download", 2)),
            vec!["chatCreated", "userMessage", "download", "cancelled"],
        ),
        (
            "attachments",
            "New chat with a text file and an image, sent to a vision model: the user message carries its attachments.",
            with_files(
                &["notes.txt", "red.png"],
                ask(describe, SEER, Some(Think::Enabled(false))),
            ),
            None,
            answered(&fresh),
        ),
        (
            "refused-unknown-attachment",
            "An attachment that is no longer staged (removed, or the page was reloaded): the command fails, no event, nothing saved.",
            with_files(
                &["notes.txt", "staged-unknown"],
                ask(describe, SEER, Some(Think::Enabled(false))),
            ),
            None,
            vec![],
        ),
        (
            "edit",
            "Edit of message 1 in the chat filled by the \"attachments\" sequence (attachments 1 notes.txt and 2 red.png, answer 2): notes.txt kept, red.png removed, extra.md added. The answer is deleted, the edited message (same id) comes back with its attachments, then a new answer. Only the edit's events.",
            editing(
                1,
                Some(vec![1]),
                with_files(
                    &["extra.md"],
                    ask(instead, SEER, Some(Think::Enabled(false))),
                ),
            ),
            None,
            answered(&["userMessage"]),
        ),
        (
            "refused-edit-unknown-message",
            "Edit of a message that is not saved (unknown id) in the chat filled by the \"attachments\" sequence: the command fails, no event, nothing changed.",
            editing(99, None, ask(instead, SEER, Some(Think::Enabled(false)))),
            None,
            vec![],
        ),
        (
            "image-tool",
            "A model with tools and an image model chosen in the ModelHub: thinking and a generate_image call (512 x 512), the context, the image's progress (place kept, loading, two steps, the first with a preview), the image saved as an attachment of that answer (imageReady), the call's result, then a second answer.",
            ask(fox, SKETCH, None),
            None,
            [
                &fresh[..],
                &[
                    "replyStarted",
                    "thinking",
                    "context",
                    "toolCalls",
                    "imageProgress",
                    "imageReady",
                    "toolResult",
                    "replyStarted",
                    "thinking",
                    "content",
                    "context",
                    "done",
                ],
            ]
            .concat(),
        ),
        (
            "image-button",
            "Image button: the text goes as it is to the image model (1024 x 1024 with a graphics card), with no request to the text engine: an answer made of one generate_image call, the image's progress, the image, the call's result, then done.",
            button(fox),
            None,
            vec![
                "chatCreated",
                "userMessage",
                "replyStarted",
                "toolCalls",
                "imageProgress",
                "imageReady",
                "toolResult",
                "done",
            ],
        ),
        (
            "image-button-no-model",
            "Image button while no image model is chosen: the command fails, no event, nothing saved.",
            button(fox),
            None,
            vec![],
        ),
        (
            "web-search",
            "Web search by ChatonLM's relay: a first answer made of a web_search call, its result (titles and links only), then a second answer that quotes what the model received (the excerpts too).",
            SendRequest {
                web_search: Some(true),
                ..ask("What is new in llama.cpp?", SEARCHER, None)
            },
            None,
            [
                &fresh[..],
                &[
                    "context",
                    "replyStarted",
                    "toolCalls",
                    "toolResult",
                    "replyStarted",
                    "content",
                    "context",
                    "done",
                ],
            ]
            .concat(),
        ),
        (
            "web-search-cancelled",
            "Web search stopped by the user while the tool runs: the call stays without result or error.",
            SendRequest {
                web_search: Some(true),
                ..ask("What is new in llama.cpp?", SEARCHER, None)
            },
            Some(("toolCalls", 1)),
            [
                &fresh[..],
                &["context", "replyStarted", "toolCalls", "cancelled"],
            ]
            .concat(),
        ),
        (
            "loading",
            "New chat with a model that isn't in memory and takes 1.5 s to load: loading, loaded, then the answer.",
            ask(hello, SLOWLOAD, None),
            None,
            answered(&fresh),
        ),
    ];
    // Identifiants des fichiers préparés, dans les séquences.
    const STAGED: [&str; 2] = ["staged-1", "staged-2"];

    for (name, description, mut request, cancel_after, expected) in cases {
        let mut texts = Vec::new();
        let files = request.attachment_ids.clone();
        for _ in 0..2 {
            // Faux Hugging Face (téléchargements) ; fermé pour
            // « download-offline ».
            let hf = fake_services::hf::FakeHf::start(fake_services::hf::HfMode::Normal)
                .await
                .unwrap();
            let painter = match name {
                "image-button-no-model" => {
                    Some(FakePainter::refusing(images::PaintRefusal::NoModel))
                }
                _ if name.starts_with("image-") => Some(FakePainter::ready()),
                _ => None,
            };
            let model = request.model.clone();
            let models: Vec<&str> = match name {
                "model-missing" | "remote-off" | "image-button" | "image-button-no-model" => {
                    vec![]
                }
                _ if name.starts_with("download") => vec![],
                _ => vec![model.as_str()],
            };
            // Faux relais de la recherche web (lent pour l'arrêt pendant
            // l'outil), gardé jusqu'à la fin de l'envoi.
            let web = if name.starts_with("web-search") {
                let web = crate::chat::tools::fake_web(|_| {}).await;
                if name == "web-search-cancelled" {
                    for provider in [
                        relay_proto::api::Provider::Brave,
                        relay_proto::api::Provider::Mojeek,
                    ] {
                        web.relay.fake.set(
                            provider,
                            relay_server::fake::Behavior::Slow(Duration::from_secs(30)),
                        );
                    }
                }
                Some(web)
            } else {
                None
            };
            let mut bench = match &web {
                Some(web) => Bench::searching(&models, web.web.relay.clone()),
                None => Bench::downloading(&models, &hf),
            };
            if let Some(painter) = painter {
                bench.engine = Arc::new(ChatEngine::default());
                bench.engine.set_painter(painter);
            }
            if name == "download-offline" {
                hf.stop().await;
            }
            let dir = bench.fake.dir().display().to_string();
            let mut replace = vec![(dir, "/data")];
            if let Some(files) = &files {
                let mut ids = Vec::new();
                for (index, file) in files.iter().enumerate() {
                    let Some(data) = sample_file(file) else {
                        ids.push(file.clone());
                        continue;
                    };
                    let id = stage(&bench.staging, file, &data).await;
                    replace.push((id.clone(), STAGED[index]));
                    ids.push(id);
                }
                request.attachment_ids = Some(ids);
            }
            // Contexte réglé des séquences du compactage ; conversation
            // existante faite d'un premier échange.
            let context_length = match name {
                "compacted-auto" => 4096,
                "context-full" => 4096,
                _ => 0,
            };
            if context_length > 0 {
                context_of(&bench, context_length);
                let words = if name == "compacted-auto" { 1100 } else { 800 };
                let first = ask(&"Tell me everything. ".repeat(words), GIST, None);
                let (events, result) = play(&bench, bench.server().await, &first, None).await;
                result.unwrap();
                let chat_id = events[0]["chatId"].as_str().unwrap().to_owned();
                replace.push((chat_id.clone(), "chat-1"));
                request.chat_id = Some(chat_id);
            }
            if name == "existing-chat" {
                let first = ask(hello, THINKER, None);
                let (events, result) = play(&bench, bench.server().await, &first, None).await;
                result.unwrap();
                let chat_id = events[0]["chatId"].as_str().unwrap().to_owned();
                replace.push((chat_id.clone(), "chat-1"));
                request.chat_id = Some(chat_id);
            }
            if request.edit_message_id.is_some() {
                // Conversation remplie par l'envoi de la séquence
                // « attachments », puis la modification enregistrée.
                let mut ids = Vec::new();
                for file in ["notes.txt", "red.png"] {
                    ids.push(stage(&bench.staging, file, &sample_file(file).unwrap()).await);
                }
                let first = SendRequest {
                    attachment_ids: Some(ids),
                    ..ask(describe, SEER, Some(Think::Enabled(false)))
                };
                let (events, result) = play(&bench, bench.server().await, &first, None).await;
                result.unwrap();
                assert_eq!(events.last().unwrap()["type"], json!("done"), "{name}");
                let chat_id = events[0]["chatId"].as_str().unwrap().to_owned();
                replace.push((chat_id.clone(), "chat-1"));
                request.chat_id = Some(chat_id);
            }
            // Sans moteur : même base, aucun binaire.
            let server = if name == "engine-unavailable" {
                let data_dir = bench.fake.access.data_dir().to_owned();
                Server::current(&Access::detached(bench.store().clone(), data_dir))
                    .await
                    .unwrap()
            } else {
                bench.server().await
            };
            let (events, result) = play(&bench, server, &request, cancel_after).await;
            assert_eq!(types(&events), expected, "{name}: {events:?}");
            if expected.is_empty() {
                assert!(matches!(result, Err(AppError::Invalid(_))), "{name}");
            } else {
                result.as_ref().unwrap();
                assert_one_final(&events);
            }
            texts.push(sequence_text(
                description,
                &request,
                events,
                &result,
                &replace,
            ));
            bench.shutdown().await;
        }
        assert_eq!(texts[0], texts[1], "{name} is not deterministic");
        export_sequence(name, &texts[0]);
    }
}

/// Attente d'un envoi au vrai moteur (modèle à charger, processeur
/// possible).
const REAL_WAIT: Duration = Duration::from_secs(300);

/// Banc du vrai moteur pour le chat ; `None` (dit par `RealBench`) si le
/// binaire ou le modèle du rôle manque.
struct RealChat {
    real: RealBench,
    engine: ChatEngine,
    staging: Staging,
}

impl RealChat {
    fn new(role: RealRole) -> Option<Self> {
        let real = RealBench::new(role)?;
        Some(RealChat {
            real,
            engine: ChatEngine::default(),
            staging: attachment_tests::staging(),
        })
    }

    fn store(&self) -> &Store {
        self.real.access.store()
    }

    async fn send(&self, request: SendRequest) -> Recorder {
        let (channel, recorder) = recorder();
        let server = Server::current(&self.real.access).await.unwrap();
        let sending = send(
            self.store(),
            &self.engine,
            &self.staging,
            server,
            request,
            &channel,
        );
        tokio::time::timeout(REAL_WAIT, sending)
            .await
            .unwrap_or_else(|_| {
                panic!("still sending after {REAL_WAIT:?}: {:?}", recorder.events())
            })
            .unwrap();
        assert_one_final(&recorder.events());
        recorder
    }

    fn set(&self, patch: SettingsPatch) {
        self.store()
            .blocking(|c| crate::store::settings::update(c, patch, None).map(drop))
            .unwrap();
    }
}

/// Contrat : une vraie réponse, raisonnement puis texte, chargement annoncé
/// la première fois seulement, contexte compté sur celui du processus.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "vrai llama-server : CHATONLM_LLAMA_TEST_BIN, modèle Text"]
async fn contract_answer_from_the_real_engine() {
    let Some(real) = RealChat::new(RealRole::Text) else {
        return;
    };
    real.set(SettingsPatch {
        context_length: Some(4096),
        ..Default::default()
    });
    let model = real.real.id.clone();
    let recorder = real
        .send(request(None, &model, Some(Think::Enabled(true))))
        .await;
    let events = recorder.events();
    assert_eq!(recorder.last()["type"], json!("done"), "{events:?}");
    assert_eq!(
        types(&events)[..4],
        ["chatCreated", "userMessage", "loading", "loaded"]
    );
    assert!(recorder.count("thinking") > 0, "{events:?}");
    assert!(!recorder.contents().trim().is_empty());
    let context = of_type(&events, "context");
    assert_eq!(context[0]["size"], json!(4096));
    assert!(context[0]["used"].as_i64().unwrap() > 10);
    // Prêt : rien d'annoncé au message suivant, réflexion coupée.
    let again = real
        .send(request(
            Some(&recorder.chat_id()),
            &model,
            Some(Think::Enabled(false)),
        ))
        .await;
    assert_eq!(again.count("loading"), 0);
    assert_eq!(again.count("thinking"), 0, "{:?}", again.events());
    assert_eq!(again.last()["type"], json!("done"));
    real.real.shutdown().await;
}

/// Contrat : contexte compté par le vrai serveur, compactage automatique
/// passé 80 %, réponse ensuite.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "vrai llama-server : CHATONLM_LLAMA_TEST_BIN, modèle Text"]
async fn contract_context_is_counted_and_compacted() {
    let Some(real) = RealChat::new(RealRole::Text) else {
        return;
    };
    real.set(SettingsPatch {
        context_length: Some(4096),
        ..Default::default()
    });
    let model = real.real.id.clone();
    // Environ 3 400 jetons de question : plus de 80 % de 4 096.
    let long = format!(
        "{} Reply with one word.",
        "The quick brown fox jumps over the lazy dog. ".repeat(340)
    );
    let first = real
        .send(SendRequest {
            content: long,
            ..request(None, &model, Some(Think::Enabled(false)))
        })
        .await;
    let chat_id = first.chat_id();
    let counted = saved(real.store(), &chat_id).context.unwrap();
    assert_eq!(counted.size, 4096);
    assert!(counted.used * 5 >= 4096 * 4, "{counted:?}");
    let second = real
        .send(request(Some(&chat_id), &model, Some(Think::Enabled(false))))
        .await;
    let events = second.events();
    assert_eq!(
        types(&events)[..3],
        ["userMessage", "compacting", "compacted"],
        "{events:?}"
    );
    assert_eq!(second.last()["type"], json!("done"), "{events:?}");
    let chat = saved(real.store(), &chat_id);
    assert_eq!(chat.compactions.len(), 1);
    assert!(!chat.compactions[0].summary.trim().is_empty());
    assert!(chat.context.unwrap().used < 3200, "{:?}", chat.context);
    real.real.shutdown().await;
}

/// Contrat : une image jointe atteint un modèle de vision (projecteur) ; un
/// modèle sans vision la voit retirée et répond quand même.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "vrai llama-server : CHATONLM_LLAMA_TEST_BIN, modèles Vision et Text"]
async fn contract_images_and_vision() {
    for role in [RealRole::Vision, RealRole::Text] {
        let Some(real) = RealChat::new(role) else {
            return;
        };
        let model = real.real.id.clone();
        let image = stage(&real.staging, "red.png", &samples::png(64, 64, [255, 0, 0])).await;
        let ask = SendRequest {
            attachment_ids: Some(vec![image]),
            ..asking(
                None,
                &model,
                "What color is this image? Answer in one word.",
            )
        };
        let recorder = real
            .send(SendRequest {
                think: Some(Think::Enabled(false)),
                ..ask
            })
            .await;
        assert_eq!(
            recorder.last()["type"],
            json!("done"),
            "{role:?}: {:?}",
            recorder.events()
        );
        assert!(!recorder.contents().trim().is_empty(), "{role:?}");
        if role == RealRole::Vision {
            assert!(
                recorder.contents().to_lowercase().contains("red"),
                "{}",
                recorder.contents()
            );
            // WebP (relecture L5a, B2) : converti en PNG à l'envoi, lu sans
            // `ffmpeg`.
            let pixels = image::RgbImage::from_pixel(64, 64, image::Rgb([255, 0, 0]));
            let mut webp = Vec::new();
            image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
                .encode(&pixels, 64, 64, image::ExtendedColorType::Rgb8)
                .unwrap();
            let image = stage(&real.staging, "red.webp", &webp).await;
            let recorder = real
                .send(SendRequest {
                    think: Some(Think::Enabled(false)),
                    attachment_ids: Some(vec![image]),
                    ..asking(
                        None,
                        &model,
                        "What color is this image? Answer in one word.",
                    )
                })
                .await;
            assert_eq!(
                recorder.last()["type"],
                json!("done"),
                "{:?}",
                recorder.events()
            );
            assert!(
                recorder.contents().to_lowercase().contains("red"),
                "{}",
                recorder.contents()
            );
        }
        real.real.shutdown().await;
    }
}

/// Contrat : un arrêt en plein flux finit l'envoi et le vrai serveur cesse
/// de générer (emplacement libéré).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "vrai llama-server : CHATONLM_LLAMA_TEST_BIN, modèle Text"]
async fn contract_cancel_stops_the_real_engine() {
    let Some(real) = RealChat::new(RealRole::Text) else {
        return;
    };
    let model = real.real.id.clone();
    let real = Arc::new(real);
    let (channel, recorder) = recorder();
    let ask = SendRequest {
        think: Some(Think::Enabled(false)),
        ..asking(None, &model, "Count from 1 to 500, one number per line.")
    };
    let task = {
        let real = real.clone();
        let server = Server::current(&real.real.access).await.unwrap();
        tokio::spawn(async move {
            send(
                real.store(),
                &real.engine,
                &real.staging,
                server,
                ask,
                &channel,
            )
            .await
        })
    };
    tokio::time::timeout(REAL_WAIT, recorder.wait_for("content", 5))
        .await
        .unwrap();
    real.engine.cancel(&format!("stream-{model}"));
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(recorder.last()["type"], json!("cancelled"));
    // L'emplacement du serveur se libère (génération arrêtée).
    let session = real
        .real
        .access
        .open(
            &model,
            OpenOptions::default(),
            &CancellationToken::new(),
            &|| {},
        )
        .await
        .unwrap();
    let slots = format!("{}/slots", session.endpoint.base_url);
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut idle = false;
    for _ in 0..50 {
        let listed: Value = client
            .get(&slots)
            .bearer_auth(&session.endpoint.api_key)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if listed
            .as_array()
            .is_some_and(|slots| slots.iter().all(|slot| slot["is_processing"] == false))
        {
            idle = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(idle, "the server kept generating");
    drop(session);
    real.real.shutdown().await;
}

/// Contrat : un modèle absent est téléchargé de Hugging Face par la file du
/// ModelHub (dans un dossier neuf : jamais celui des modèles d'essai), puis
/// il répond. Modèle `Plain` de `models.json` (lot L8) ; réseau requis.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "vrai llama-server et Hugging Face : CHATONLM_LLAMA_TEST_BIN, modèle Plain, réseau"]
async fn contract_a_missing_model_is_downloaded_from_hugging_face() {
    let models = std::env::var_os("CHATONLM_LLAMA_TEST_MODELS")
        .map(PathBuf::from)
        .and_then(|dir| std::fs::read_to_string(dir.join("models.json")).ok());
    let Some(plain) = crate::llm::testing::real_model(models.as_deref(), RealRole::Plain) else {
        eprintln!("contract: no Plain model in models.json (lot L8), test skipped");
        return;
    };
    // Banc du vrai moteur, dossier des modèles neuf et vide.
    let Some(real) = RealChat::new(RealRole::Text) else {
        return;
    };
    let fresh = tempfile::tempdir().unwrap();
    let dir = fresh.path().display().to_string();
    real.store()
        .blocking(move |c| crate::store::local_models::set_models_dir(c, Some(&dir)))
        .unwrap();
    let recorder = real.send(request(None, &plain.id, None)).await;
    let events = recorder.events();
    assert_eq!(recorder.last()["type"], json!("done"), "{events:?}");
    let seen = downloads(&events);
    assert!(
        seen.last()
            .is_some_and(|(total, completed, done)| *done && total == completed)
    );
    assert!(!recorder.contents().trim().is_empty());
    real.real.shutdown().await;
}
