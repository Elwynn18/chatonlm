//! Essais du moteur de correction contre le faux moteur (`FakeBench`,
//! scénarios `chatonlm-proofer*` de `fake-llama`) et, en contrat, contre le
//! vrai `llama-server` (`RealBench`).

use std::time::{Duration, Instant};

use fake_llama::scenarios::l5a::proof::{PROOFREAD_OFF, PROOFREAD_THINKING};
use serde_json::{Value, json};
use tauri::ipc::InvokeResponseBody;

use super::*;
use crate::llm::testing::{FakeBench, RealBench, Role};
use crate::store::settings::SettingsPatch;

/// Deux paragraphes, chacun avec des fautes que le faux corrige.
const TEXT: &str = "I recieve teh letter.\n\nJe suis allez au cinema.\n";
const CORRECTED: &str = "I receive the letter.\n\nJe suis allé au cinéma.\n";

/// Modèles du banc (identifiant, scénario).
const PROOFER: &str = "test/Proofer-GGUF:Q4_K_M";
const SLOW: &str = "test/Slow-GGUF:Q4_K_M";
const OFF: &str = "test/Off-GGUF:Q4_K_M";
const THINKER: &str = "test/Thinker-GGUF:Q4_K_M";
const OPENED: &str = "test/Opened-GGUF:Q4_K_M";
const CUT: &str = "test/Cut-GGUF:Q4_K_M";
const WAIT: &str = "test/Wait-GGUF:Q4_K_M";
const BROKEN: &str = "test/Broken-GGUF:Q4_K_M";
const FAILING: &str = "test/Failing-GGUF:Q4_K_M";

fn bench() -> FakeBench {
    FakeBench::new(&[
        (PROOFER, "proofer"),
        (SLOW, "proofer-slow"),
        (OFF, "proofer-off"),
        (THINKER, "proofer-think"),
        (OPENED, "proofer-opened"),
        (CUT, "proofer-cut"),
        (WAIT, "proofer-wait"),
        (BROKEN, "error"),
        (FAILING, "loadfail"),
    ])
}

async fn server(bench: &FakeBench) -> Server {
    Server::current(&bench.access).await.unwrap()
}

/// Requêtes de chat reçues par le processus de `id` (ouvert s'il ne l'est
/// plus : même processus, mêmes réglages).
async fn chat_requests(bench: &FakeBench, id: &str) -> Vec<Value> {
    let session = bench
        .access
        .open(
            id,
            OpenOptions::default(),
            &CancellationToken::new(),
            &|| {},
        )
        .await
        .unwrap();
    let url = format!("{}/fake/requests", session.endpoint.base_url);
    let all: Vec<Value> = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(url)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    all.into_iter()
        .filter(|request| request["path"] == "/v1/chat/completions")
        .map(|request| request["body"].clone())
        .collect()
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

    /// Code et message de l'erreur finale, après `Started`.
    fn error(&self) -> (ProofreadErrorCode, String) {
        let all = self.all();
        assert!(matches!(all[0], ProofreadEvent::Started { .. }), "{all:?}");
        match all.last() {
            Some(ProofreadEvent::Error { code, message }) => (*code, message.clone()),
            other => panic!("{other:?}"),
        }
    }
}

async fn proofread(
    server: &Server,
    model: &str,
    mode: ProofreadMode,
    text: &str,
) -> (Outcome, Events) {
    let events = Events::default();
    let outcome = run(
        server,
        model,
        mode,
        ProofreadOptions::default(),
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

#[tokio::test(flavor = "multi_thread")]
async fn parts_are_corrected_in_order_as_they_stream() {
    let bench = bench();
    let server = server(&bench).await;
    let (outcome, events) = proofread(&server, PROOFER, ProofreadMode::Correct, TEXT).await;
    let parts = split(TEXT);
    assert_eq!(parts.len(), 2);
    assert_eq!(
        events.steps(),
        [
            ProofreadEvent::Started {
                model: PROOFER.into(),
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

    // Une requête par passage, avec les consignes de « Correct » ; réflexion
    // laissée au gabarit (modèle sans réflexion), température basse.
    let requests = chat_requests(&bench, PROOFER).await;
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0]["messages"][0]["content"],
        prompts::CORRECT_PROMPT
    );
    assert_eq!(
        requests[1]["messages"].as_array().unwrap().last().unwrap()["content"],
        "<text>\nJe suis allez au cinema.\n</text>"
    );
    assert!(requests[0].get("chat_template_kwargs").is_none());
    assert_eq!(
        requests[0]["temperature"].as_f64(),
        Some(f64::from(prompts::TEMPERATURE))
    );
    bench.shutdown().await;
}

/// Contexte réglé dans l'app : le modèle tourne avec lui (comme le chat :
/// pas de rechargement entre les deux).
#[tokio::test(flavor = "multi_thread")]
async fn the_model_runs_with_the_app_context() {
    let bench = bench();
    bench.set(SettingsPatch {
        context_length: Some(8192),
        ..Default::default()
    });
    let (outcome, _) =
        proofread(&server(&bench).await, PROOFER, ProofreadMode::Correct, TEXT).await;
    assert_eq!(outcome.error, None);
    let running = bench.engine().running();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].context_length, 8192);
    bench.shutdown().await;
}

/// Modèle tenu par l'Étude avec un contexte plus grand que celui réglé : la
/// correction le reprend sans attendre ni relancer (relecture L5a, B1).
#[tokio::test(flavor = "multi_thread")]
async fn a_model_held_by_the_study_is_shared() {
    let bench = bench();
    bench.set(SettingsPatch {
        context_length: Some(4096),
        ..Default::default()
    });
    let study = bench
        .access
        .open(
            PROOFER,
            OpenOptions {
                ctx: Some(8192),
                ..Default::default()
            },
            &CancellationToken::new(),
            &|| {},
        )
        .await
        .unwrap();
    let server = server(&bench).await;
    let (outcome, _) = tokio::time::timeout(
        Duration::from_secs(10),
        proofread(&server, PROOFER, ProofreadMode::Correct, TEXT),
    )
    .await
    .expect("the proofreading waited for the study");
    assert_eq!(outcome.error, None);
    assert_eq!(outcome.text(), CORRECTED);
    let running = bench.engine().running();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].context_length, 8192);
    drop(study);
    bench.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rephrase_sends_its_own_instructions() {
    let bench = bench();
    let server = server(&bench).await;
    let text = "I have a lot of work in order to finish teh report.";
    let (outcome, _) = proofread(&server, PROOFER, ProofreadMode::Rephrase, text).await;
    assert_eq!(outcome.text(), "I have many work to finish the report.");
    assert_eq!(
        chat_requests(&bench, PROOFER).await[0]["messages"][0]["content"],
        prompts::REPHRASE_PROMPT
    );
    // « Correct » sur le même texte : corrections seules.
    let (outcome, _) = proofread(&server, PROOFER, ProofreadMode::Correct, text).await;
    assert_eq!(
        outcome.text(),
        "I have a lot of work in order to finish the report."
    );
    bench.shutdown().await;
}

/// Modèle qui réfléchit : réflexion coupée (gabarit), jamais montrée si
/// elle arrive quand même (à part) ; vu réfléchir malgré l'arrêt : plus
/// coupée.
#[tokio::test(flavor = "multi_thread")]
async fn thinking_is_turned_off_and_never_shown() {
    let bench = bench();
    let server = server(&bench).await;
    let (outcome, events) = proofread(&server, THINKER, ProofreadMode::Correct, TEXT).await;
    assert_eq!(outcome.text(), CORRECTED);
    let requests = chat_requests(&bench, THINKER).await;
    assert_eq!(
        requests[0]["chat_template_kwargs"],
        json!({ "enable_thinking": false })
    );
    // Noté par le chat : réfléchit quand même. Plus coupée, réflexion reçue
    // à part, jamais montrée.
    bench
        .store()
        .blocking(|c| crate::store::model_prefs::set_always_thinks(c, THINKER))
        .unwrap();
    let (outcome, events_after) = proofread(&server, THINKER, ProofreadMode::Correct, TEXT).await;
    assert_eq!(outcome.text(), CORRECTED);
    let requests = chat_requests(&bench, THINKER).await;
    assert!(
        requests
            .last()
            .unwrap()
            .get("chat_template_kwargs")
            .is_none()
    );
    for event in events.all().into_iter().chain(events_after.all()) {
        if let ProofreadEvent::Delta { text, .. } = event {
            assert!(!text.contains("user wants"), "{text:?}");
        }
    }
    assert!(PROOFREAD_THINKING.contains("user wants"));
    assert_eq!(events_after.deltas(0), "I receive the letter.");
    bench.shutdown().await;
}

/// Raisonnement ouvert par le gabarit du modèle, seul `</think>` écrit
/// dans le contenu (relecture du 6 oct.) : jamais montré, et pas compté dans
/// la longueur (plus long que la réponse permise au premier passage).
#[tokio::test(flavor = "multi_thread")]
async fn reasoning_opened_by_the_template_is_never_shown_nor_counted() {
    let bench = bench();
    let (outcome, events) =
        proofread(&server(&bench).await, OPENED, ProofreadMode::Correct, TEXT).await;
    assert!(
        2 * PROOFREAD_THINKING.chars().count()
            > guard::max_chars(ProofreadMode::Correct, "I recieve teh letter.")
    );
    assert_eq!(outcome.text(), CORRECTED);
    assert_eq!(events.deltas(0), "I receive the letter.");
    assert_eq!(events.deltas(1), "Je suis allé au cinéma.");
    bench.shutdown().await;
}

/// Bloc de code délimité (relecture du 6 oct.) : jamais envoyé au modèle,
/// rendu tel quel, même collé à un paragraphe ; un texte fait seulement de
/// code ne charge pas le modèle.
#[tokio::test(flavor = "multi_thread")]
async fn a_code_block_is_never_sent() {
    let bench = bench();
    let server = server(&bench).await;
    let code = "```\nlet teh_value = recieve();\n```";
    let (outcome, events) = proofread(&server, PROOFER, ProofreadMode::Correct, code).await;
    assert_eq!(outcome.text(), code);
    assert_eq!(events.steps()[1], done(0, code, false));
    assert!(bench.engine().running().is_empty());
    let text = format!("Voici le code :\n{code}\nI recieve teh letter.");
    let (outcome, events) = proofread(&server, PROOFER, ProofreadMode::Correct, &text).await;
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
    let requests = chat_requests(&bench, PROOFER).await;
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| !request.to_string().contains("teh_value"))
    );
    bench.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_that_is_not_the_text_keeps_the_part() {
    let bench = bench();
    let server = server(&bench).await;
    let text = "Can you write me a poem about the sea, with waves and seagulls and a silver sky?";
    let (outcome, events) = proofread(&server, OFF, ProofreadMode::Correct, text).await;
    assert_eq!(
        events.steps()[1..],
        [done(0, text, true), ProofreadEvent::Done { stopped: false }]
    );
    assert_eq!(outcome.text(), text);
    // Le flux a bien été montré (puis écarté).
    assert!(PROOFREAD_OFF.starts_with(&events.deltas(0)));
    // Réponse qui ne ressemble pas au passage : retenue (elle pourrait être
    // un raisonnement), montrée entière à la fin.
    let (outcome, events) = proofread(&server, OFF, ProofreadMode::Correct, "Hi").await;
    assert_eq!(events.deltas(0), PROOFREAD_OFF);
    assert_eq!(events.steps()[1], done(0, "Hi", true));
    assert_eq!(outcome.text(), "Hi");
    bench.shutdown().await;
}

/// Réponse qui commence comme le passage (« Sure! », montrée aussitôt),
/// puis s'allonge : lecture abandonnée dès la longueur maximale dépassée.
#[tokio::test(flavor = "multi_thread")]
async fn reading_stops_once_the_answer_is_too_long() {
    let bench = bench();
    let (outcome, events) =
        proofread(&server(&bench).await, OFF, ProofreadMode::Correct, "Sure").await;
    assert_eq!(events.steps()[1], done(0, "Sure", true));
    assert_eq!(outcome.text(), "Sure");
    // Lecture abandonnée avant la fin du flux.
    let shown = events.deltas(0).chars().count();
    assert!(shown < PROOFREAD_OFF.chars().count(), "{shown}");
    assert!(shown > guard::max_chars(ProofreadMode::Correct, "Sure"));
    bench.shutdown().await;
}

/// Arrêt demandé au premier évènement d'un genre donné ; l'arrêt arrive
/// aussitôt (le passage lent durerait plusieurs secondes).
async fn stopped_at(
    bench: &FakeBench,
    when: impl Fn(&ProofreadEvent) -> bool + Send + Sync,
) -> (Outcome, Events) {
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
    let server = server(bench).await;
    let started = Instant::now();
    let outcome = run(
        &server,
        SLOW,
        ProofreadMode::Correct,
        ProofreadOptions::default(),
        TEXT,
        &cancel,
        &emit,
    )
    .await;
    assert!(outcome.stopped);
    assert_eq!(outcome.error, None);
    // Mesure grossière, sans charge : la correction lente durerait plus.
    assert!(started.elapsed() < Duration::from_secs(20));
    (outcome, events)
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_keeps_the_finished_parts() {
    let bench = bench();
    let (outcome, events) = stopped_at(&bench, |event| {
        matches!(event, ProofreadEvent::PartDone { index: 0, .. })
    })
    .await;
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

    // Arrêt au milieu d'un passage : abandonné, texte d'origine ; le
    // serveur voit la connexion fermée.
    let (outcome, events) = stopped_at(&bench, |event| {
        matches!(event, ProofreadEvent::Delta { .. })
    })
    .await;
    assert_eq!(
        events.steps()[1..],
        [ProofreadEvent::Done { stopped: true }]
    );
    assert_eq!(outcome.text(), TEXT);
    bench.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn texts_refused_at_once_get_a_lone_error() {
    let bench = bench();
    let server = server(&bench).await;
    for (model, text, code, message) in [
        (
            PROOFER,
            " \n\t",
            ProofreadErrorCode::Empty,
            "There is no text to correct",
        ),
        (
            PROOFER,
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
        (
            "@remote/mistral/mistral-small-latest",
            "Bonjour",
            ProofreadErrorCode::Failed,
            "Remote models are turned off. Turn them on in the settings to use this model.",
        ),
    ] {
        let (outcome, events) = proofread(&server, model, ProofreadMode::Correct, text).await;
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
    assert!(bench.engine().running().is_empty());
    // Juste à la limite : accepté (découpé, puis modèle absent).
    let limit = "a".repeat(MAX_CHARS);
    let (outcome, events) = proofread(
        &server,
        "nope/Nope-GGUF:Q4_K_M",
        ProofreadMode::Correct,
        &limit,
    )
    .await;
    assert!(matches!(events.all()[0], ProofreadEvent::Started { .. }));
    assert!(outcome.parts.len() >= MAX_CHARS / split::PART_CHARS);
    bench.shutdown().await;
}

/// Fichier du modèle disparu hors de l'app (la base le connaît encore) : le
/// moteur ne le trouve pas, `modelNotFound` comme un modèle absent.
#[tokio::test(flavor = "multi_thread")]
async fn a_model_whose_file_is_gone_is_not_found() {
    let bench = FakeBench::new(&[(PROOFER, "proofer")]);
    let model = crate::local_models::get(bench.store(), bench.access.data_dir(), PROOFER)
        .await
        .unwrap()
        .unwrap();
    std::fs::remove_file(&model.path).unwrap();
    let (_, events) = proofread(&server(&bench).await, PROOFER, ProofreadMode::Correct, TEXT).await;
    assert_eq!(events.error().0, ProofreadErrorCode::ModelNotFound);
    bench.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn errors_after_the_start_are_reported_with_their_code() {
    let bench = bench();
    let server = server(&bench).await;
    // Absent de la base locale.
    let (_, events) = proofread(
        &server,
        "nope/Nope-GGUF:Q4_K_M",
        ProofreadMode::Correct,
        TEXT,
    )
    .await;
    assert_eq!(
        events.error(),
        (
            ProofreadErrorCode::ModelNotFound,
            "The model nope/Nope-GGUF:Q4_K_M is not installed".into()
        )
    );
    // Distant permis, mais aucun fournisseur ne le sert.
    bench.set(SettingsPatch {
        remote_models: Some(true),
        ..Default::default()
    });
    let remote = "@remote/mistral/mistral-small-latest";
    let (_, events) = proofread(
        &Server::current(&bench.access).await.unwrap(),
        remote,
        ProofreadMode::Correct,
        TEXT,
    )
    .await;
    assert_eq!(events.error().0, ProofreadErrorCode::ModelNotFound);
    // Erreur du modèle en cours de route : passages d'origine gardés.
    let (outcome, events) = proofread(&server, BROKEN, ProofreadMode::Correct, TEXT).await;
    assert_eq!(
        events.error(),
        (
            ProofreadErrorCode::Failed,
            "Proofreading failed: the text engine answered with an error (HTTP 500)".into()
        )
    );
    assert_eq!(outcome.text(), TEXT);
    // Même échec après « Rephrase » : le message nomme la reformulation.
    let (_, events) = proofread(&server, BROKEN, ProofreadMode::Rephrase, TEXT).await;
    assert_eq!(
        events.error(),
        (
            ProofreadErrorCode::Failed,
            "Rephrasing failed: the text engine answered with an error (HTTP 500)".into()
        )
    );
    // Connexion coupée en cours de route.
    let (_, events) = proofread(&server, CUT, ProofreadMode::Correct, TEXT).await;
    assert_eq!(
        events.error(),
        (
            ProofreadErrorCode::Failed,
            "Proofreading failed: the connection to the model was lost".into()
        )
    );
    // Le moteur ne charge pas le modèle.
    let (_, events) = proofread(&server, FAILING, ProofreadMode::Correct, TEXT).await;
    let (code, message) = events.error();
    assert_eq!(code, ProofreadErrorCode::EngineFailed);
    assert!(message.starts_with("the text engine"), "{message}");
    // Délai d'un passage dépassé (raccourci pour l'essai).
    let events = Events::default();
    let job = Job {
        server: &server,
        model: WAIT,
        mode: ProofreadMode::Correct,
        options: ProofreadOptions::default(),
        cancel: &Cancel::default(),
        emit: &events.emit(),
        timeout: Duration::from_millis(300),
    };
    job.run(TEXT).await;
    assert_eq!(
        events.error(),
        (
            ProofreadErrorCode::Failed,
            "Proofreading failed: the model took too long to answer".into()
        )
    );
    bench.shutdown().await;
}

/// Recette du 1er nov. 2026 (défaut R14) : paragraphe coupé à la main, rendu
/// corrigé sur une ligne (réponse réelle de qwen3:8b). Mise en forme
/// automatique coupée : le passage est corrigé avec ses retours à la ligne
/// (CRLF du texte compris), et c'est ce texte remis en forme qui est rendu.
/// Activée (défaut) : le paragraphe reste sur une ligne.
#[test]
fn a_wrapped_paragraph_answered_on_one_line_is_corrected() {
    const ANSWER: &str = "A tiny shop backend. It calculates the cart total, retry the payment \
                          when the bank doesn't answer, and turn product titles into URL slugs.";
    let kept_lines = ProofreadOptions {
        formatting: false,
        ..ProofreadOptions::default()
    };
    for (options, ending) in [
        (kept_lines, "\n"),
        (kept_lines, "\r\n"),
        (ProofreadOptions::default(), "\n"),
        (ProofreadOptions::default(), "\r\n"),
    ] {
        let text = format!(
            "A tiny shop backend. It calcualte the cart total, retry the payement{ending}\
             when the bank dont answer, and turn product titles into URL slugs.{ending}"
        );
        let corrected = if options.formatting {
            ANSWER.to_owned()
        } else {
            format!(
                "A tiny shop backend. It calculates the cart total, retry the payment{ending}\
                 when the bank doesn't answer, and turn product titles into URL slugs."
            )
        };
        let parts = split::marked(&text);
        assert_eq!(parts.len(), 1);
        let (part, code) = &parts[0];
        assert!(!code);
        let result = settle_part(ProofreadMode::Correct, options, &part.text, ANSWER);
        assert_eq!(
            result,
            PartResult {
                text: corrected.clone(),
                kept: false
            }
        );
        let results = [Some(result)];
        assert_eq!(
            assemble(std::slice::from_ref(part), &results),
            format!("{corrected}{ending}")
        );
    }
}

/// Options de la correction, sur de vraies réponses : mots déplacés ou
/// ajoutés écartés quand l'ordre est gardé ; nombre écrit en lettres écarté
/// quand les chiffres sont gardés ; mise en forme : majuscule et point
/// ajoutés (activée) ou retirés (coupée).
#[test]
fn the_options_keep_order_digits_and_formatting() {
    let corrected = |answer: &str, text: &str, options: ProofreadOptions| {
        settle_part(ProofreadMode::Correct, options, text, answer)
    };
    let plain = ProofreadOptions::default();
    let keep_order = ProofreadOptions {
        keep_order: true,
        ..plain
    };
    let digits = ProofreadOptions {
        digits: true,
        ..plain
    };
    let raw = ProofreadOptions {
        formatting: false,
        ..plain
    };
    let text = "je sais pas si il viens demain avec ses 3 enfants";
    // Mot ajouté (« ne »), écarté seulement si l'ordre est gardé.
    let added = "Je ne sais pas s'il vient demain avec ses 3 enfants.";
    assert!(!corrected(added, text, plain).kept);
    assert!(corrected(added, text, keep_order).kept);
    // Mots déplacés.
    let moved = "Demain, je sais pas s'il vient avec ses 3 enfants.";
    assert!(corrected(moved, text, keep_order).kept);
    // Même ordre, mot coupé par une apostrophe : accepté.
    let in_place = "Je sais pas s'il vient demain avec ses 3 enfants.";
    assert_eq!(
        corrected(in_place, text, keep_order),
        PartResult {
            text: in_place.into(),
            kept: false
        }
    );
    // Nombre écrit en lettres.
    let spelled = "Je sais pas s'il vient demain avec ses trois enfants.";
    assert!(!corrected(spelled, text, plain).kept);
    assert!(corrected(spelled, text, digits).kept);
    // Mise en forme : le modèle n'a mis ni majuscule ni point.
    let bare = "je sais pas s'il vient demain avec ses 3 enfants";
    assert_eq!(corrected(bare, text, plain).text, in_place);
    // Coupée : ceux qu'il a mis sont retirés.
    assert_eq!(corrected(in_place, text, raw).text, bare);
}

/// Réglages du banc : modèles du chat et de la correction.
fn configure(bench: &FakeBench, chat: &str, proofread: &str) {
    bench.set(SettingsPatch {
        selected_model: Some(chat.into()),
        proofread_model: Some(proofread.into()),
        ..Default::default()
    });
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

async fn job(bench: &FakeBench, proofreader: &Proofreader) -> Vec<Value> {
    let (channel, events) = channel(None);
    run_job(
        bench.store(),
        &bench.access,
        proofreader,
        "job".into(),
        TEXT.into(),
        ProofreadMode::Correct,
        &channel,
    )
    .await
    .unwrap();
    events.lock().clone()
}

/// Modèle réglé ; sinon « Auto » : le meilleur modèle prêt du moteur,
/// sinon celui du chat ; aucun : refus.
#[tokio::test(flavor = "multi_thread")]
async fn a_job_uses_the_proofread_model_then_the_chat_model() {
    let bench = bench();
    let proofreader = Proofreader::default();
    // Rien de prêt : modèle réglé, puis celui du chat.
    configure(&bench, BROKEN, PROOFER);
    let events = job(&bench, &proofreader).await;
    assert_eq!(events[0]["model"], PROOFER);
    assert_eq!(events.last().unwrap()["event"], "done");
    bench.engine().unload_all().await;
    configure(&bench, PROOFER, "");
    let events = job(&bench, &proofreader).await;
    assert_eq!(events[0]["model"], PROOFER);
    // PROOFER prêt dans le moteur : pris par « Auto » à la place de celui
    // du chat.
    configure(&bench, BROKEN, "");
    let events = job(&bench, &proofreader).await;
    assert_eq!(events[0]["event"], "started");
    assert_eq!(events[0]["model"], PROOFER);
    assert_eq!(events.last().unwrap()["event"], "done");
    bench.engine().unload_all().await;
    configure(&bench, "", "");
    assert_eq!(
        job(&bench, &proofreader).await,
        [json!({
            "event": "error",
            "code": "noModel",
            "message": "Choose a model for proofreading",
        })]
    );
    bench.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_is_registered_while_it_runs_and_can_be_cancelled() {
    let bench = Arc::new(bench());
    configure(&bench, SLOW, "");
    let proofreader = Arc::new(Proofreader::default());
    let (channel, events) = channel(None);
    let task = {
        let (bench, proofreader) = (bench.clone(), proofreader.clone());
        tokio::spawn(async move {
            run_job(
                bench.store(),
                &bench.access,
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
    tokio::time::timeout(Duration::from_secs(30), first_delta)
        .await
        .unwrap();
    assert!(proofreader.jobs.lock().contains_key("job-1"));
    // Un autre identifiant : sans effet.
    proofreader.cancel("job-2");
    proofreader.cancel("job-1");
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let events = events.lock().clone();
    assert_eq!(
        events.last().unwrap(),
        &json!({ "event": "done", "stopped": true })
    );
    assert!(proofreader.jobs.lock().is_empty());
    assert_eq!(chat_requests(&bench, SLOW).await.len(), 1);
    bench.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_channel_stops_the_job() {
    let bench = bench();
    configure(&bench, SLOW, "");
    let proofreader = Proofreader::default();
    // `Started` passe, le premier `Delta` échoue.
    let (channel, events) = channel(Some(2));
    run_job(
        bench.store(),
        &bench.access,
        &proofreader,
        "job".into(),
        TEXT.into(),
        ProofreadMode::Correct,
        &channel,
    )
    .await
    .unwrap();
    assert_eq!(events.lock().len(), 1);
    assert_eq!(chat_requests(&bench, SLOW).await.len(), 1);
    assert!(proofreader.jobs.lock().is_empty());
    bench.shutdown().await;
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

/// Contrat : vraie correction par le vrai `llama-server` et Qwen3 0.6B
/// (`RealBench`, rôle `Text`), deux passages ; évènements complets et dans
/// l'ordre, raisonnement jamais montré, réflexion coupée. La qualité n'est
/// pas exigée (le garde-fou peut garder un passage).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CHATONLM_LLAMA_TEST_BIN and the Text model (RealBench)"]
async fn contract_proofread_with_a_small_model() {
    let Some(bench) = RealBench::new(Role::Text) else {
        return;
    };
    let server = Server::current(&bench.access).await.unwrap();
    let text = "I recieve your message yesterday but i didnt had time to answer.\n\n\
                Les enfant sont partis a l'école ce matin.";
    for mode in [ProofreadMode::Correct, ProofreadMode::Rephrase] {
        let (outcome, events) = proofread(&server, &bench.id, mode, text).await;
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
    // Qwen3 sait réfléchir : la réflexion lui est coupée.
    let details = llm::models::details(&bench.access, &bench.id)
        .await
        .unwrap();
    assert!(crate::dictation::polish::think_off_on(
        Some(&details),
        false
    ));
    bench.shutdown().await;
}
