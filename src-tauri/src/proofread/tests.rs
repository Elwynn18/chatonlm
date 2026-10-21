//! Essais du moteur de correction contre le faux serveur (`fake-ollama`,
//! modèles `chatonlm-proofread*`) et, en contrat, contre l'Ollama de test.

use std::time::Instant;

use fake_ollama::{FakeOllama, Mode, PROOFREAD_OFF, PROOFREAD_THINKING};
use serde_json::Value;
use tauri::ipc::InvokeResponseBody;

use super::*;
use crate::store::settings::{self, SettingsPatch};

/// Deux paragraphes, chacun avec des fautes que le faux serveur corrige.
const TEXT: &str = "I recieve teh letter.\n\nJe suis allez au cinema.\n";
const CORRECTED: &str = "I receive the letter.\n\nJe suis allé au cinéma.\n";

fn server(url: &str) -> Server {
    Server {
        url: url.into(),
        context_length: 0,
        keep_alive: 0,
        cloud: false,
    }
}

/// Évènements reçus, dans l'ordre.
#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<ProofreadEvent>>>);

impl Events {
    fn emit(&self) -> impl Fn(ProofreadEvent) + Send + Sync + use<> {
        let events = self.0.clone();
        move |event| events.lock().push(event)
    }

    fn all(&self) -> Vec<ProofreadEvent> {
        self.0.lock().clone()
    }

    /// Fragments du passage `index`, bout à bout.
    fn deltas(&self, of: u32) -> String {
        self.all()
            .into_iter()
            .filter_map(|event| match event {
                ProofreadEvent::Delta { index, text } if index == of => Some(text),
                _ => None,
            })
            .collect()
    }

    /// Évènements sans les `Delta`.
    fn steps(&self) -> Vec<ProofreadEvent> {
        self.all()
            .into_iter()
            .filter(|event| !matches!(event, ProofreadEvent::Delta { .. }))
            .collect()
    }
}

async fn proofread(
    fake: &FakeOllama,
    model: &str,
    mode: ProofreadMode,
    text: &str,
) -> (Outcome, Events) {
    let events = Events::default();
    let outcome = run(
        &server(&fake.url()),
        model,
        mode,
        text,
        &Cancel::default(),
        &events.emit(),
    )
    .await;
    (outcome, events)
}

fn done(index: u32, text: &str, kept: bool) -> ProofreadEvent {
    ProofreadEvent::PartDone {
        index,
        result: PartResult {
            text: text.into(),
            kept,
        },
    }
}

#[tokio::test]
async fn parts_are_corrected_in_order_as_they_stream() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let (outcome, events) =
        proofread(&fake, "chatonlm-proofread", ProofreadMode::Correct, TEXT).await;
    let parts = split(TEXT);
    assert_eq!(parts.len(), 2);
    assert_eq!(
        events.steps(),
        [
            ProofreadEvent::Started {
                model: "chatonlm-proofread".into(),
                parts: parts.clone(),
            },
            done(0, "I receive the letter.", false),
            done(1, "Je suis allé au cinéma.", false),
            ProofreadEvent::Done { stopped: false },
        ]
    );
    // Chaque passage arrive en plusieurs fragments, avant son `PartDone`.
    assert_eq!(events.deltas(0), "I receive the letter.");
    assert_eq!(events.deltas(1), "Je suis allé au cinéma.");
    let all = events.all();
    let first_done = all
        .iter()
        .position(|e| matches!(e, ProofreadEvent::PartDone { .. }))
        .unwrap();
    let deltas_before = all[..first_done]
        .iter()
        .filter(|e| matches!(e, ProofreadEvent::Delta { index: 0, .. }))
        .count();
    assert!(deltas_before >= 2, "{all:?}");
    assert!(
        !all[first_done..]
            .iter()
            .any(|e| matches!(e, ProofreadEvent::Delta { index: 0, .. }))
    );

    assert_eq!(outcome.text(), CORRECTED);
    assert_eq!(outcome.error, None);
    assert!(!outcome.stopped);
    assert_eq!(outcome.parts, parts);

    // Une requête par passage, avec les consignes de « Correct » ; pas de
    // `think` (modèle sans capacité `thinking`), réglages du serveur par
    // défaut.
    let requests = fake.chat_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0]["messages"][0]["content"],
        prompts::CORRECT_PROMPT
    );
    assert_eq!(
        requests[1]["messages"].as_array().unwrap().last().unwrap()["content"],
        "<text>\nJe suis allez au cinema.\n</text>"
    );
    assert!(requests[0].get("think").is_none());
    assert_eq!(
        requests[0]["options"],
        serde_json::json!({ "temperature": 0.2 })
    );
}

#[tokio::test]
async fn rephrase_sends_its_own_instructions() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let text = "I have a lot of work in order to finish teh report.";
    let (outcome, _) = proofread(&fake, "chatonlm-proofread", ProofreadMode::Rephrase, text).await;
    assert_eq!(outcome.text(), "I have many work to finish the report.");
    assert_eq!(
        fake.chat_requests()[0]["messages"][0]["content"],
        prompts::REPHRASE_PROMPT
    );
    // « Correct » sur le même texte : corrections seules.
    let (outcome, _) = proofread(&fake, "chatonlm-proofread", ProofreadMode::Correct, text).await;
    assert_eq!(
        outcome.text(),
        "I have a lot of work in order to finish the report."
    );
}

#[tokio::test]
async fn thinking_written_in_the_content_is_never_shown() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let (outcome, events) = proofread(
        &fake,
        "chatonlm-proofread-think",
        ProofreadMode::Correct,
        TEXT,
    )
    .await;
    assert_eq!(outcome.text(), CORRECTED);
    for event in events.all() {
        if let ProofreadEvent::Delta { text, .. } = event {
            assert!(
                !text.contains('<') && !text.contains('>') && !text.contains("user wants"),
                "{text:?}"
            );
        }
    }
    assert!(PROOFREAD_THINKING.contains("user wants"));
    assert_eq!(events.deltas(0), "I receive the letter.");
}

/// Raisonnement ouvert par le gabarit du modèle, seul `</think>` écrit
/// (relecture du 6 oct.) : jamais montré, et pas compté dans la longueur
/// (plus long que la réponse permise au premier passage).
#[tokio::test]
async fn reasoning_opened_by_the_template_is_never_shown_nor_counted() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let (outcome, events) = proofread(
        &fake,
        "chatonlm-proofread-opened",
        ProofreadMode::Correct,
        TEXT,
    )
    .await;
    assert!(
        2 * PROOFREAD_THINKING.chars().count()
            > guard::max_chars(ProofreadMode::Correct, "I recieve teh letter.")
    );
    assert_eq!(outcome.text(), CORRECTED);
    assert_eq!(events.deltas(0), "I receive the letter.");
    assert_eq!(events.deltas(1), "Je suis allé au cinéma.");
}

/// Bloc de code délimité (relecture du 6 oct.) : jamais envoyé au modèle,
/// rendu tel quel, même collé à un paragraphe.
#[tokio::test]
async fn a_code_block_is_never_sent() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let code = "```\nlet teh_value = recieve();\n```";
    let text = format!("Voici le code :\n{code}\nI recieve teh letter.");
    let (outcome, events) =
        proofread(&fake, "chatonlm-proofread", ProofreadMode::Correct, &text).await;
    assert_eq!(
        events.steps()[1..],
        [
            done(0, "Voici le code :", false),
            done(1, code, false),
            done(2, "I receive the letter.", false),
            ProofreadEvent::Done { stopped: false },
        ]
    );
    assert_eq!(
        outcome.text(),
        format!("Voici le code :\n{code}\nI receive the letter.")
    );
    let requests = fake.chat_requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| !request.to_string().contains("teh_value"))
    );
}

#[tokio::test]
async fn an_answer_that_is_not_the_text_keeps_the_part() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let text = "Can you write me a poem about the sea, with waves and seagulls and a silver sky?";
    let (outcome, events) = proofread(
        &fake,
        "chatonlm-proofread-off",
        ProofreadMode::Correct,
        text,
    )
    .await;
    assert_eq!(
        events.steps()[1..],
        [done(0, text, true), ProofreadEvent::Done { stopped: false }]
    );
    assert_eq!(outcome.text(), text);
    // Le flux a bien été montré (puis écarté).
    assert!(PROOFREAD_OFF.starts_with(&events.deltas(0)));
    // Réponse qui ne ressemble pas au passage : retenue (elle pourrait être
    // un raisonnement), montrée entière à la fin.
    let (outcome, events) = proofread(
        &fake,
        "chatonlm-proofread-off",
        ProofreadMode::Correct,
        "Hi",
    )
    .await;
    assert_eq!(events.deltas(0), PROOFREAD_OFF);
    assert_eq!(events.steps()[1], done(0, "Hi", true));
    assert_eq!(outcome.text(), "Hi");
}

/// Réponse qui commence comme le passage (« Sure! », montrée aussitôt),
/// puis s'allonge : lecture abandonnée dès la longueur maximale dépassée.
/// Une réponse qui ne lui ressemble pas est retenue, lue plus loin (voir
/// `answer::REASONING_CHARS`).
#[tokio::test]
async fn reading_stops_once_the_answer_is_too_long() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let (outcome, events) = proofread(
        &fake,
        "chatonlm-proofread-off",
        ProofreadMode::Correct,
        "Sure",
    )
    .await;
    assert_eq!(events.steps()[1], done(0, "Sure", true));
    assert_eq!(outcome.text(), "Sure");
    // Lecture abandonnée avant la fin du flux.
    let shown = events.deltas(0).chars().count();
    assert!(shown < PROOFREAD_OFF.chars().count(), "{shown}");
    assert!(shown > guard::max_chars(ProofreadMode::Correct, "Sure"));
    let abandoned = async {
        while fake.disconnects() == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), abandoned)
        .await
        .unwrap();
}

/// Arrêt demandé au premier évènement d'un genre donné ; l'arrêt arrive
/// aussitôt (le passage lent durerait plusieurs secondes).
async fn stopped_at(when: impl Fn(&ProofreadEvent) -> bool + Send + Sync) -> (Outcome, Events) {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let events = Events::default();
    let cancel = Cancel::default();
    let record = events.emit();
    let emit = {
        let cancel = cancel.clone();
        move |event: ProofreadEvent| {
            let stop = when(&event);
            record(event);
            if stop {
                cancel.cancel();
            }
        }
    };
    let started = Instant::now();
    let outcome = run(
        &server(&fake.url()),
        "chatonlm-proofread-slow",
        ProofreadMode::Correct,
        TEXT,
        &cancel,
        &emit,
    )
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    assert!(outcome.stopped);
    assert_eq!(outcome.error, None);
    (outcome, events)
}

#[tokio::test]
async fn stopping_keeps_the_finished_parts() {
    let (outcome, events) =
        stopped_at(|event| matches!(event, ProofreadEvent::PartDone { index: 0, .. })).await;
    assert_eq!(
        events.steps()[1..],
        [
            done(0, "I receive the letter.", false),
            ProofreadEvent::Done { stopped: true },
        ]
    );
    assert_eq!(outcome.results[1], None);
    assert_eq!(
        outcome.text(),
        "I receive the letter.\n\nJe suis allez au cinema.\n"
    );

    // Arrêt au milieu d'un passage : abandonné, texte d'origine.
    let (outcome, events) = stopped_at(|event| matches!(event, ProofreadEvent::Delta { .. })).await;
    assert_eq!(
        events.steps()[1..],
        [ProofreadEvent::Done { stopped: true }]
    );
    assert_eq!(outcome.text(), TEXT);
}

#[tokio::test]
async fn texts_refused_at_once_get_a_lone_error() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    for (model, text, code, message) in [
        (
            "chatonlm-proofread",
            " \n\t",
            ProofreadErrorCode::Empty,
            "There is no text to correct",
        ),
        (
            "chatonlm-proofread",
            &*"a ".repeat(MAX_CHARS / 2 + 1),
            ProofreadErrorCode::TooLong,
            "The text is too long: 100,000 characters at most",
        ),
        (
            " ",
            "Bonjour",
            ProofreadErrorCode::NoModel,
            "Choose a model for proofreading",
        ),
    ] {
        let (outcome, events) = proofread(&fake, model, ProofreadMode::Correct, text).await;
        assert_eq!(
            events.all(),
            [ProofreadEvent::Error {
                code,
                message: message.into()
            }]
        );
        assert_eq!(outcome.error, Some((code, message.into())));
        assert!(outcome.parts.is_empty());
    }
    assert!(fake.chat_requests().is_empty());
    // Juste à la limite : accepté (découpé, puis modèle absent).
    let limit = "a".repeat(MAX_CHARS);
    let (outcome, events) = proofread(&fake, "nope:1b", ProofreadMode::Correct, &limit).await;
    assert!(matches!(events.all()[0], ProofreadEvent::Started { .. }));
    assert!(outcome.parts.len() >= MAX_CHARS / split::PART_CHARS);
}

#[tokio::test]
async fn errors_after_the_start_are_reported_with_their_code() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let error_of = |events: &Events| {
        let all = events.all();
        assert!(matches!(all[0], ProofreadEvent::Started { .. }), "{all:?}");
        match all.last() {
            Some(ProofreadEvent::Error { code, message }) => (*code, message.clone()),
            other => panic!("{other:?}"),
        }
    };
    // Absent de `/api/show`, ou supprimé entre `/api/show` et `/api/chat`.
    for model in ["nope:1b", "chatonlm-vanished"] {
        let (_, events) = proofread(&fake, model, ProofreadMode::Correct, TEXT).await;
        assert_eq!(
            error_of(&events),
            (
                ProofreadErrorCode::ModelNotFound,
                format!("The model {model} is not installed")
            )
        );
    }
    // Erreur du modèle en cours de route.
    let (outcome, events) = proofread(&fake, "chatonlm-error", ProofreadMode::Correct, TEXT).await;
    assert_eq!(
        error_of(&events),
        (
            ProofreadErrorCode::Failed,
            "Proofreading failed: model runner has unexpectedly stopped".into()
        )
    );
    assert_eq!(outcome.text(), TEXT);
    // Même échec après « Rephrase » : le message nomme la reformulation.
    let (_, events) = proofread(&fake, "chatonlm-error", ProofreadMode::Rephrase, TEXT).await;
    assert_eq!(
        error_of(&events),
        (
            ProofreadErrorCode::Failed,
            "Rephrasing failed: model runner has unexpectedly stopped".into()
        )
    );
    // Flux fini sans `done`.
    let (_, events) = proofread(&fake, "chatonlm-cut", ProofreadMode::Correct, TEXT).await;
    assert_eq!(
        error_of(&events),
        (
            ProofreadErrorCode::Failed,
            "Proofreading failed: the connection to Ollama was lost".into()
        )
    );
    // Délai d'un passage dépassé (raccourci pour l'essai).
    let events = Events::default();
    let job = Job {
        server: &server(&fake.url()),
        model: "chatonlm-wait",
        mode: ProofreadMode::Correct,
        cancel: &Cancel::default(),
        emit: &events.emit(),
        timeout: Duration::from_millis(300),
    };
    job.run(TEXT).await;
    assert_eq!(
        error_of(&events),
        (
            ProofreadErrorCode::Failed,
            "Proofreading failed: the model took too long to answer".into()
        )
    );
    // 404 d'un proxy ou d'une mauvaise adresse, sans erreur d'Ollama (corps
    // vide, ou page en texte) : un échec avec le statut et la page, pas un
    // modèle absent (relecture du 6 oct.).
    let proxy = FakeOllama::start(Mode::Status(404)).await.unwrap();
    let (_, events) = proofread(&proxy, "gemma3:4b", ProofreadMode::Correct, TEXT).await;
    assert_eq!(
        error_of(&events),
        (
            ProofreadErrorCode::Failed,
            "Proofreading failed: Ollama answered HTTP 404".into()
        )
    );
    let events = Events::default();
    run(
        &server(&plain_404().await),
        "gemma3:4b",
        ProofreadMode::Correct,
        TEXT,
        &Cancel::default(),
        &events.emit(),
    )
    .await;
    assert_eq!(
        error_of(&events),
        (
            ProofreadErrorCode::Failed,
            "Proofreading failed: Ollama answered HTTP 404: 404 page not found".into()
        )
    );
    // Serveur arrêté.
    let url = fake.url();
    fake.stop().await;
    let events = Events::default();
    run(
        &server(&url),
        "chatonlm-proofread",
        ProofreadMode::Correct,
        TEXT,
        &Cancel::default(),
        &events.emit(),
    )
    .await;
    assert_eq!(
        error_of(&events),
        (
            ProofreadErrorCode::Unreachable,
            format!("can't connect to Ollama at {url}")
        )
    );
}

/// Serveur qui répond à tout la page 404 en texte d'un proxy (Go, gin,
/// Traefik).
async fn plain_404() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            // Requête lue en entier (en-têtes et corps) avant de répondre.
            let mut received = Vec::new();
            let mut buffer = [0; 8192];
            while let Ok(count @ 1..) = socket.read(&mut buffer).await {
                received.extend_from_slice(&buffer[..count]);
                let text = String::from_utf8_lossy(&received).to_ascii_lowercase();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if received.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let _ = socket
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\n\
                      Content-Length: 18\r\nConnection: close\r\n\r\n404 page not found",
                )
                .await;
        }
    });
    format!("http://{address}")
}

/// Base dont le serveur réglé est `url`, avec ces modèles du chat et de la
/// correction.
fn configured(url: &str, chat: &str, proofread: &str) -> Arc<Store> {
    let store = Store::open_in_memory().unwrap();
    let patch = SettingsPatch {
        server_url: Some(url.into()),
        selected_model: Some(chat.into()),
        proofread_model: Some(proofread.into()),
        ..Default::default()
    };
    store
        .blocking(|c| settings::update(c, patch, None).map(drop))
        .unwrap();
    Arc::new(store)
}

/// Canal qui garde les évènements (en JSON) ; refuse les envois à partir du
/// `fail_from`-ième (page rechargée).
fn channel(fail_from: Option<usize>) -> (Channel<ProofreadEvent>, Arc<Mutex<Vec<Value>>>) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    let channel = Channel::new(move |body| {
        let mut events = sink.lock();
        if fail_from.is_some_and(|from| events.len() + 1 >= from) {
            return Err(tauri::Error::FailedToReceiveMessage);
        }
        if let InvokeResponseBody::Json(text) = body {
            events.push(serde_json::from_str(&text).unwrap());
        }
        Ok(())
    });
    (channel, received)
}

#[tokio::test]
async fn a_job_uses_the_proofread_model_then_the_chat_model() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    for (chat, proofread, used) in [
        (
            "chatonlm-error",
            "chatonlm-proofread",
            Some("chatonlm-proofread"),
        ),
        ("chatonlm-proofread", "", Some("chatonlm-proofread")),
        ("", "", None),
    ] {
        let store = configured(&fake.url(), chat, proofread);
        let (channel, events) = channel(None);
        run_job(
            &store,
            &Proofreader::default(),
            "job".into(),
            TEXT.into(),
            ProofreadMode::Correct,
            &channel,
        )
        .await
        .unwrap();
        let events = events.lock().clone();
        match used {
            Some(model) => {
                assert_eq!(events[0]["event"], "started");
                assert_eq!(events[0]["model"], model);
                assert_eq!(events.last().unwrap()["event"], "done");
            }
            None => assert_eq!(
                events,
                [serde_json::json!({
                    "event": "error",
                    "code": "noModel",
                    "message": "Choose a model for proofreading",
                })]
            ),
        }
    }
}

#[tokio::test]
async fn a_job_is_registered_while_it_runs_and_can_be_cancelled() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let store = configured(&fake.url(), "chatonlm-proofread-slow", "");
    let proofreader = Arc::new(Proofreader::default());
    let (channel, events) = channel(None);
    let task = {
        let (store, proofreader) = (store.clone(), proofreader.clone());
        tokio::spawn(async move {
            run_job(
                &store,
                &proofreader,
                "job-1".into(),
                TEXT.into(),
                ProofreadMode::Correct,
                &channel,
            )
            .await
        })
    };
    let first_delta = async {
        while !events.lock().iter().any(|e| e["event"] == "delta") {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), first_delta)
        .await
        .unwrap();
    assert!(proofreader.jobs.lock().contains_key("job-1"));
    // Un autre identifiant : sans effet.
    proofreader.cancel("job-2");
    proofreader.cancel("job-1");
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let events = events.lock().clone();
    assert_eq!(
        events.last().unwrap(),
        &serde_json::json!({ "event": "done", "stopped": true })
    );
    assert!(proofreader.jobs.lock().is_empty());
    assert_eq!(fake.chat_requests().len(), 1);
}

#[tokio::test]
async fn a_closed_channel_stops_the_job() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let store = configured(&fake.url(), "chatonlm-proofread-slow", "");
    let proofreader = Proofreader::default();
    // `Started` passe, le premier `Delta` échoue.
    let (channel, events) = channel(Some(2));
    let started = Instant::now();
    run_job(
        &store,
        &proofreader,
        "job".into(),
        TEXT.into(),
        ProofreadMode::Correct,
        &channel,
    )
    .await
    .unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(events.lock().len(), 1);
    assert_eq!(fake.chat_requests().len(), 1);
    assert!(proofreader.jobs.lock().is_empty());
}

/// Chargement de la page : les travaux d'une page rechargée sont arrêtés.
#[test]
fn every_job_is_cancelled_when_the_page_loads() {
    let proofreader = Proofreader::default();
    let (first, second) = (Cancel::default(), Cancel::default());
    let _first = Registered::new(&proofreader, "a".into(), first.clone());
    let _second = Registered::new(&proofreader, "b".into(), second.clone());
    proofreader.cancel_all();
    assert!(first.is_cancelled() && second.is_cancelled());
}

/// Un travail fini ne retire pas celui qui a repris son identifiant.
#[test]
fn a_finished_job_leaves_a_newer_one_with_the_same_id() {
    let proofreader = Proofreader::default();
    let (old, new) = (Cancel::default(), Cancel::default());
    let registered = Registered::new(&proofreader, "job".into(), old);
    let newer = Registered::new(&proofreader, "job".into(), new.clone());
    drop(registered);
    proofreader.cancel("job");
    assert!(new.is_cancelled());
    drop(newer);
    assert!(proofreader.jobs.lock().is_empty());
}

/// Contrat : vraie correction par qwen3:0.6b sur l'Ollama de test (port
/// 11435), deux passages ; évènements complets et dans l'ordre, raisonnement
/// jamais montré. La qualité n'est pas exigée (le garde-fou peut garder un
/// passage).
#[tokio::test]
#[ignore = "needs the test Ollama instance (scripts/test-ollama.sh) with qwen3:0.6b"]
async fn contract_proofread_with_a_small_model() {
    const TEST_INSTANCE: &str = "http://127.0.0.1:11435";
    const MODEL: &str = "qwen3:0.6b";
    let text = "I recieve your message yesterday but i didnt had time to answer.\n\n\
                Les enfant sont partis a l'école ce matin.";
    for mode in [ProofreadMode::Correct, ProofreadMode::Rephrase] {
        let events = Events::default();
        let outcome = run(
            &server(TEST_INSTANCE),
            MODEL,
            mode,
            text,
            &Cancel::default(),
            &events.emit(),
        )
        .await;
        eprintln!("{mode:?}: {:?}", outcome.text());
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.parts.len(), 2);
        let steps = events.steps();
        assert!(matches!(steps[0], ProofreadEvent::Started { .. }));
        for (index, step) in steps[1..3].iter().enumerate() {
            match step {
                ProofreadEvent::PartDone {
                    index: done,
                    result,
                } => {
                    assert_eq!(*done as usize, index);
                    if result.kept {
                        assert_eq!(result.text, outcome.parts[index].text);
                    } else {
                        assert!(guard::accept(
                            mode,
                            &outcome.parts[index].text,
                            &result.text
                        ));
                    }
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(steps[3], ProofreadEvent::Done { stopped: false });
        assert_eq!(steps.len(), 4);
        for index in 0..2 {
            assert!(!events.deltas(index).contains("<think"));
        }
    }
    // qwen3 sait raisonner : `think: false` lui est envoyé.
    let details = models::show(TEST_INSTANCE, MODEL).await.unwrap();
    assert!(crate::dictation::polish::think_off(Some(&details), false));
}

/// Modèle cloud : réglage coupé, refusé d'emblée, rien n'est envoyé (ni
/// détails, ni texte) ; activé sans compte, le 401 du serveur est dit comme
/// un compte manquant.
#[tokio::test]
async fn a_cloud_model_proofreads_only_once_turned_on() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let cloud = |on| Server {
        cloud: on,
        ..server(&fake.url())
    };
    let events = Events::default();
    let outcome = run(
        &cloud(false),
        "kimi-k2.6:cloud",
        ProofreadMode::Correct,
        TEXT,
        &Cancel::default(),
        &events.emit(),
    )
    .await;
    let message = "Cloud models are turned off: kimi-k2.6:cloud runs on ollama.com. Turn them on in Settings › Models to use it.";
    assert_eq!(
        events.all(),
        [ProofreadEvent::Error {
            code: ProofreadErrorCode::Failed,
            message: message.into(),
        }]
    );
    assert_eq!(
        outcome.error,
        Some((ProofreadErrorCode::Failed, message.into()))
    );
    assert!(fake.chat_requests().is_empty());

    let events = Events::default();
    run(
        &cloud(true),
        "kimi-k2.6:cloud",
        ProofreadMode::Correct,
        TEXT,
        &Cancel::default(),
        &events.emit(),
    )
    .await;
    let Some(ProofreadEvent::Error { message, .. }) = events.all().pop() else {
        panic!("{:?}", events.all());
    };
    assert_eq!(
        message,
        "Proofreading failed: Cloud models require an Ollama account"
    );
    assert_eq!(fake.chat_requests().len(), 1);
}
