//! Essais de l'Atelier (noyau) contre le faux `llama-server` (`FakeBench`,
//! scénarios `chatonlm-agent-*` du faux, `scenarios/agent.rs`), et contrat
//! du budget de réflexion contre le vrai (`RealBench`, nom en `contract`).

use std::sync::Mutex;
use std::time::Duration;

use fake_llama::scenarios::agent as fake;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::inference::{self, Endpoint, FinishReason, InferenceError, Message};
use crate::llm::LlmError;
use crate::llm::testing::{FakeBench, RealBench, Role};

const SIMPLE: &str = "test/Simple-GGUF:Q4_K_M";
const THINKER: &str = "test/Thinker-GGUF:Q4_K_M";
const FLAKY: &str = "test/Flaky-GGUF:Q4_K_M";
const GARBAGE: &str = "test/Garbage-GGUF:Q4_K_M";
const CUT: &str = "test/Cut-GGUF:Q4_K_M";
const BUSY: &str = "test/Busy-GGUF:Q4_K_M";
const MULL: &str = "test/Mull-GGUF:Q4_K_M";
const TOOLED: &str = "test/Tooled-GGUF:Q4_K_M";
const INSIST: &str = "test/Insist-GGUF:Q4_K_M";
const ODD: &str = "test/Odd-GGUF:Q4_K_M";
const ROGUE: &str = "test/Rogue-GGUF:Q4_K_M";
const LATE: &str = "test/Late-GGUF:Q4_K_M";
const MANY: &str = "test/Many-GGUF:Q4_K_M";

fn bench(models: &[(&str, &str)]) -> FakeBench {
    FakeBench::new(models)
}

async fn open(bench: &FakeBench, model: &str) -> Worker {
    Worker::open(
        &bench.access,
        model,
        WorkerOptions {
            ctx_min: 4096,
            cancel: CancellationToken::new(),
        },
    )
    .await
    .unwrap()
    .with_retry_delay(Duration::from_millis(10))
}

/// Corps des requêtes de chat reçues par le faux, dans l'ordre.
async fn chats(endpoint: &Endpoint) -> Vec<Value> {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let all: Value = client
        .get(format!("{}/fake/requests", endpoint.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    all.as_array()
        .unwrap()
        .iter()
        .filter(|request| request["path"] == "/v1/chat/completions")
        .map(|request| request["body"].clone())
        .collect()
}

fn schema() -> Value {
    json!({ "type": "object", "properties": { "answer": { "type": "string" } },
        "required": ["answer"] })
}

fn call(think: ThinkMode, max_tokens: Option<u32>) -> Call {
    Call {
        system: "Answer in JSON.".into(),
        user: "How are you?".into(),
        schema: schema(),
        temperature: 0.25,
        think,
        max_tokens,
    }
}

fn write_options(think: ThinkMode, max_tokens: Option<u32>) -> Write {
    Write {
        temperature: 0.5,
        think,
        max_tokens,
    }
}

/// Fragments reçus d'une réponse en flux.
#[derive(Default)]
struct Deltas(Mutex<Vec<Delta>>);

impl Deltas {
    fn sink(&self) -> impl Fn(Delta) + Send + Sync + '_ {
        |delta| self.0.lock().unwrap().push(delta)
    }

    fn take(&self) -> Vec<Delta> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

/// Texte reçu depuis le dernier `Restart`.
fn shown(deltas: &[Delta]) -> String {
    let start = deltas
        .iter()
        .rposition(|delta| *delta == Delta::Restart)
        .map_or(0, |at| at + 1);
    deltas[start..]
        .iter()
        .map(|delta| match delta {
            Delta::Text(text) => text.as_str(),
            Delta::Restart => "",
        })
        .collect()
}

/// Outil factice : rend « result for <query> » (suivi de `padding`
/// caractères s'il en a), ou attend l'arrêt.
struct Lookup {
    name: &'static str,
    hang: bool,
    padding: usize,
    calls: Mutex<Vec<Value>>,
}

impl Lookup {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            hang: false,
            padding: 0,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<Value> {
        self.calls.lock().unwrap().clone()
    }
}

impl Tool for Lookup {
    fn name(&self) -> &str {
        self.name
    }

    fn definition(&self) -> Value {
        json!({ "type": "function", "function": { "name": self.name,
            "description": "Look something up.",
            "parameters": { "type": "object", "properties": { "query": { "type": "string" } },
                "required": ["query"] } } })
    }

    fn call(
        &self,
        args: Value,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(args.clone());
            if self.hang {
                cancel.cancelled().await;
                return Err("stopped".into());
            }
            let query = args["query"].as_str().or(args["prompt"].as_str());
            let mut result = format!("result for {}", query.unwrap_or("?"));
            // Mots de cinq caractères : un jeton chacun pour le faux.
            result.push_str(&" data".repeat(self.padding / 5));
            Ok(result)
        })
    }
}

#[test]
fn passing_errors_and_stops_are_told_apart() {
    let inference = |error| AgentError::from(LlmError::Inference(error));
    for passing in [
        InferenceError::Timeout,
        InferenceError::Loading,
        InferenceError::Interrupted("cut".into()),
        InferenceError::Http {
            status: 503,
            message: "busy".into(),
        },
    ] {
        assert!(inference(passing.clone()).transient(), "{passing:?}");
    }
    for lasting in [
        InferenceError::Http {
            status: 400,
            message: "bad".into(),
        },
        InferenceError::ContextExceeded {
            n_prompt_tokens: None,
            n_ctx: None,
        },
        InferenceError::Unauthorized,
    ] {
        assert!(!inference(lasting.clone()).transient(), "{lasting:?}");
    }
    assert!(!AgentError::Unreadable.transient());
    assert_eq!(AgentError::from(LlmError::Cancelled), AgentError::Cancelled);
    assert_eq!(
        AgentError::from(LlmError::Engine(crate::engine::EngineError::Unloaded)),
        AgentError::Cancelled
    );
    assert_eq!(AgentError::from(LlmError::RemoteOff), AgentError::RemoteOff);
    assert_eq!(
        AgentError::from(LlmError::NotInstalled("x".into())).code(),
        "notInstalled"
    );
    assert_eq!(AgentError::Looping.code(), "looping");
}

/// Ouverture : distants refusés sans rien demander si le réglage les coupe,
/// modèle absent, arrêt ; contexte demandé (jamais le réglage du chat, ou
/// une instance prête plus grande) ; jetons comptés par le modèle ; session
/// rendue par `release`.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_opens_its_model_with_enough_context() {
    let bench = bench(&[(SIMPLE, "simple")]);
    let options = |ctx_min| WorkerOptions {
        ctx_min,
        cancel: CancellationToken::new(),
    };
    assert_eq!(
        Worker::open(
            &bench.access,
            "@remote/openrouter/some-model",
            options(4096)
        )
        .await
        .unwrap_err(),
        AgentError::RemoteOff
    );
    assert_eq!(
        Worker::open(&bench.access, "test/Absent-GGUF:Q4_K_M", options(4096))
            .await
            .unwrap_err(),
        AgentError::Model(LlmError::NotInstalled("test/Absent-GGUF:Q4_K_M".into()))
    );
    let stopped = options(4096);
    stopped.cancel.cancel();
    assert_eq!(
        Worker::open(&bench.access, SIMPLE, stopped)
            .await
            .unwrap_err(),
        AgentError::Cancelled
    );
    assert!(bench.engine().running().is_empty());

    let worker = Worker::open(&bench.access, SIMPLE, options(4096))
        .await
        .unwrap();
    assert_eq!(worker.context(), 4096);
    assert_eq!(worker.model(), SIMPLE);
    assert!(!worker.thinks());
    assert_eq!(worker.count_tokens("Hello world").await.unwrap(), 2);
    assert!(bench.engine().running()[0].in_use);
    worker.release();
    assert!(!bench.engine().running()[0].in_use);
    // Réglage du chat plus grand : ignoré (le modèle a été choisi, par
    // `engine::auto::largest_fitting`, pour tenir à `ctx_min`).
    bench.set(crate::store::settings::SettingsPatch {
        context_length: Some(16_384),
        ..Default::default()
    });
    let worker = Worker::open(&bench.access, SIMPLE, options(4096))
        .await
        .unwrap();
    assert_eq!(worker.context(), 4096);
    assert_eq!(bench.engine().running()[0].context_length, 4096);
    worker.release();
    bench.set(crate::store::settings::SettingsPatch {
        context_length: Some(0),
        ..Default::default()
    });
    let worker = Worker::open(&bench.access, SIMPLE, options(8192))
        .await
        .unwrap();
    assert_eq!(worker.context(), 8192);
    worker.release();
    // Instance prête plus grande : reprise telle quelle.
    let worker = Worker::open(&bench.access, SIMPLE, options(2048))
        .await
        .unwrap();
    assert_eq!(worker.context(), 8192);
    worker.release();
    bench.shutdown().await;
}

/// JSON contraint : schéma envoyé dans la forme appliquée par b12110,
/// réponse lue ; limite par défaut = budget + 10 000 ; réflexion demandée
/// seulement à un modèle qui réfléchit, budget dès qu'elle est voulue.
#[tokio::test(flavor = "multi_thread")]
async fn json_answers_carry_the_schema_and_the_thinking_budget() {
    let bench = bench(&[(SIMPLE, "simple"), (THINKER, "agent-think")]);
    let worker = open(&bench, SIMPLE).await;
    let json = worker
        .ask_json(&call(ThinkMode::On { budget: 500 }, None))
        .await
        .unwrap();
    assert!(json.value["answer"].is_string(), "{json:?}");
    assert!(json.usage.completion > 0);
    let sent = chats(worker.endpoint()).await;
    let body = sent.last().unwrap();
    assert_eq!(
        body["response_format"],
        json!({ "type": "json_schema", "json_schema": { "name": "response", "schema": schema() } })
    );
    assert_eq!(body["temperature"], 0.25);
    // Modèle sans réflexion annoncée : réflexion laissée au gabarit, mais
    // le budget part (si ses détails se trompent, elle reste bornée).
    assert_eq!(body["max_tokens"], ANSWER_TOKENS + 500);
    assert_eq!(body["thinking_budget_tokens"], 500);
    assert!(body.get("chat_template_kwargs").is_none());
    // Réflexion coupée : ni budget ni réflexion.
    worker.ask_json(&call(ThinkMode::Off, None)).await.unwrap();
    let sent = chats(worker.endpoint()).await;
    let body = sent.last().unwrap();
    assert_eq!(body["max_tokens"], ANSWER_TOKENS);
    assert!(body.get("thinking_budget_tokens").is_none());
    assert!(body.get("chat_template_kwargs").is_none());
    worker.release();

    let worker = open(&bench, THINKER).await;
    assert!(worker.thinks());
    let json = worker
        .ask_json(&call(ThinkMode::On { budget: 3 }, None))
        .await
        .unwrap();
    assert_eq!(json.value, json!({ "answer": "fine" }));
    // Trois morceaux « step n. » : le budget a borné la réflexion du faux.
    assert_eq!(json.reasoning_chars, 3 * "step 0. ".len());
    let off = worker
        .ask_json(&call(ThinkMode::Off, Some(77)))
        .await
        .unwrap();
    assert_eq!(off.reasoning_chars, 0);
    let sent = chats(worker.endpoint()).await;
    assert_eq!(sent[0]["thinking_budget_tokens"], 3);
    assert_eq!(sent[0]["chat_template_kwargs"]["enable_thinking"], true);
    assert_eq!(sent[0]["max_tokens"], ANSWER_TOKENS + 3);
    assert!(sent[1].get("thinking_budget_tokens").is_none());
    assert_eq!(sent[1]["chat_template_kwargs"]["enable_thinking"], false);
    assert_eq!(sent[1]["max_tokens"], 77);
    worker.release();
    bench.shutdown().await;
}

/// Réponse sans JSON : UN nouvel essai, puis `Unreadable`.
#[tokio::test(flavor = "multi_thread")]
async fn unreadable_json_is_asked_once_more() {
    let bench = bench(&[(FLAKY, "agent-flaky"), (GARBAGE, "agent-garbage")]);
    let worker = open(&bench, FLAKY).await;
    let json = worker.ask_json(&call(ThinkMode::Off, None)).await.unwrap();
    assert_eq!(json.value, json!({ "answer": "fine" }));
    assert_eq!(chats(worker.endpoint()).await.len(), 2);
    worker.release();
    let worker = open(&bench, GARBAGE).await;
    assert_eq!(
        worker.ask_json(&call(ThinkMode::Off, None)).await,
        Err(AgentError::Unreadable)
    );
    assert_eq!(chats(worker.endpoint()).await.len(), 2);
    assert!(!fake::NO_JSON.contains('{'));
    worker.release();
    bench.shutdown().await;
}

/// JSON coupé à la limite : redemandé à limite double ; encore coupé :
/// `Incomplete`.
#[tokio::test(flavor = "multi_thread")]
async fn a_cut_json_is_asked_again_with_twice_the_tokens() {
    let bench = bench(&[(CUT, "agent-cut")]);
    let worker = open(&bench, CUT).await;
    let json = worker
        .ask_json(&call(ThinkMode::Off, Some(30)))
        .await
        .unwrap();
    let words = json.value["answer"].as_str().unwrap();
    assert_eq!(words.split(' ').count(), fake::CUT_WORDS);
    assert!(matches!(
        worker.ask_json(&call(ThinkMode::Off, Some(10))).await,
        Err(AgentError::Incomplete { .. })
    ));
    let limits: Vec<Value> = chats(worker.endpoint())
        .await
        .iter()
        .map(|body| body["max_tokens"].clone())
        .collect();
    assert_eq!(limits, [30, 60, 10, 20]);
    worker.release();
    bench.shutdown().await;
}

/// Erreur passagère (500) : un nouvel essai après une pause.
#[tokio::test(flavor = "multi_thread")]
async fn a_passing_error_is_tried_again_once() {
    let bench = bench(&[(BUSY, "agent-busy")]);
    let worker = open(&bench, BUSY).await;
    let json = worker.ask_json(&call(ThinkMode::Off, None)).await.unwrap();
    assert_eq!(json.value, json!({ "answer": "fine" }));
    assert_eq!(chats(worker.endpoint()).await.len(), 2);
    worker.release();
    bench.shutdown().await;
}

/// Texte en flux : fragments visibles, sans réflexion ; coupé : redemandé à
/// limite double (`Restart` annoncé), puis `Incomplete` avec le texte
/// obtenu.
#[tokio::test(flavor = "multi_thread")]
async fn writing_streams_the_visible_text_and_retries_a_cut_answer() {
    let bench = bench(&[(THINKER, "agent-think"), (CUT, "agent-cut")]);
    let deltas = Deltas::default();
    let worker = open(&bench, THINKER).await;
    let text = worker
        .write(
            vec![Message::user("Think, then answer.")],
            &write_options(ThinkMode::On { budget: 100 }, None),
            &deltas.sink(),
        )
        .await
        .unwrap();
    assert_eq!(text.content, fake::THOUGHT);
    assert_eq!(text.finish, FinishReason::Stop);
    let thought: usize = (0..fake::THINK_PIECES)
        .map(|n| format!("step {n}. ").len())
        .sum();
    assert_eq!(text.reasoning_chars, thought);
    let got = deltas.take();
    assert!(!got.contains(&Delta::Restart));
    assert_eq!(shown(&got), fake::THOUGHT);
    worker.release();

    let worker = open(&bench, CUT).await;
    let text = worker
        .write(
            vec![Message::user("Write.")],
            &write_options(ThinkMode::Off, Some(30)),
            &deltas.sink(),
        )
        .await
        .unwrap();
    assert_eq!(text.content.split(' ').count(), fake::CUT_WORDS);
    let got = deltas.take();
    assert_eq!(got.iter().filter(|d| **d == Delta::Restart).count(), 1);
    assert_eq!(shown(&got), text.content);
    let cut = worker
        .write(
            vec![Message::user("Write.")],
            &write_options(ThinkMode::Off, Some(10)),
            &deltas.sink(),
        )
        .await;
    let Err(AgentError::Incomplete { partial }) = cut else {
        panic!("{cut:?}");
    };
    assert_eq!(partial.split(' ').count(), 20);
    worker.release();
    bench.shutdown().await;
}

/// Réflexion écrite dans le contenu puis `</think>` sans `<think>` (Qwen3 au
/// bout de son budget) : la réflexion montrée est reprise (`Restart`), la
/// réponse arrive ensuite (avec `Visible` de l'Étude, elle n'arrivait
/// jamais).
#[tokio::test(flavor = "multi_thread")]
async fn an_answer_after_an_orphan_closing_tag_still_streams() {
    let bench = bench(&[(LATE, "agent-late-close")]);
    let deltas = Deltas::default();
    let worker = open(&bench, LATE).await;
    let text = worker
        .write(
            vec![Message::user("How many apples?")],
            &write_options(ThinkMode::Off, None),
            &deltas.sink(),
        )
        .await
        .unwrap();
    assert_eq!(text.content, fake::LATE_ANSWER);
    let got = deltas.take();
    assert_eq!(got.iter().filter(|d| **d == Delta::Restart).count(), 1);
    assert_eq!(shown(&got), fake::LATE_ANSWER);
    assert!(text.reasoning_chars >= fake::LATE_THOUGHT.len());
    worker.release();
    bench.shutdown().await;
}

/// Boucle dans la réflexion : arrêtée (`Looping`), requête lâchée.
#[tokio::test(flavor = "multi_thread")]
async fn a_looping_model_is_stopped() {
    let bench = bench(&[(MULL, "mull-think")]);
    let worker = open(&bench, MULL).await;
    let looped = tokio::time::timeout(
        Duration::from_secs(20),
        worker.write(
            vec![Message::user("Say hi.")],
            &write_options(ThinkMode::On { budget: 4000 }, None),
            &|_| {},
        ),
    )
    .await
    .expect("stopped in time");
    assert_eq!(looped, Err(AgentError::Looping));
    worker.release();
    bench.shutdown().await;
}

/// Outils : appelés au fil des tours, résultat rendu au modèle, réponse
/// finale ; bornés à `max_turns` tours, puis une requête sans outils.
#[tokio::test(flavor = "multi_thread")]
async fn tools_run_in_bounded_turns() {
    let bench = bench(&[(TOOLED, "tools"), (INSIST, "insist-tools")]);
    let lookup = Lookup::new("lookup");
    let worker = open(&bench, TOOLED).await;
    let text = worker
        .run_tools(
            vec![Message::user("cats")],
            &[&lookup],
            4,
            &write_options(ThinkMode::Off, None),
        )
        .await
        .unwrap();
    assert_eq!(lookup.calls(), [json!({ "query": "cats" })]);
    assert!(text.content.contains("result for cats"), "{text:?}");
    let sent = chats(worker.endpoint()).await;
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0]["tools"][0]["function"]["name"], "lookup");
    let last = sent[1]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["role"], "tool");
    assert_eq!(last["content"], "result for cats");
    worker.release();

    let painter = Lookup::new("generate_image");
    let worker = open(&bench, INSIST).await;
    let text = worker
        .run_tools(
            vec![Message::user("Draw.")],
            &[&painter],
            2,
            &write_options(ThinkMode::Off, None),
        )
        .await
        .unwrap();
    assert_eq!(painter.calls().len(), 2);
    assert_eq!(text.content, "Done with the pictures.");
    let sent = chats(worker.endpoint()).await;
    assert_eq!(sent.len(), 3);
    assert!(sent[1].get("tools").is_some() && sent[2].get("tools").is_none());
    worker.release();
    bench.shutdown().await;
}

/// Outil inconnu, arguments illisibles : erreur rendue au modèle, outil
/// jamais appelé ; modèle sans outils : rien d'offert, appels ignorés.
#[tokio::test(flavor = "multi_thread")]
async fn odd_tool_calls_get_an_error_and_unknown_tools_are_not_offered() {
    let bench = bench(&[(ODD, "agent-odd-tools"), (ROGUE, "rogue")]);
    let lookup = Lookup::new("lookup");
    let worker = open(&bench, ODD).await;
    let text = worker
        .run_tools(
            vec![Message::user("Go.")],
            &[&lookup],
            3,
            &write_options(ThinkMode::Off, None),
        )
        .await
        .unwrap();
    assert_eq!(
        text.content,
        "Error: unknown tool: no_such_tool | Error: the arguments are not valid JSON"
    );
    assert!(lookup.calls().is_empty());
    worker.release();

    let worker = open(&bench, ROGUE).await;
    let text = worker
        .run_tools(
            vec![Message::user("Go.")],
            &[&lookup],
            3,
            &write_options(ThinkMode::Off, None),
        )
        .await
        .unwrap();
    assert_eq!(text.content, "Let me check.");
    assert!(lookup.calls().is_empty());
    let sent = chats(worker.endpoint()).await;
    assert_eq!(sent.len(), 1);
    assert!(sent[0].get("tools").is_none());
    worker.release();
    bench.shutdown().await;
}

/// Trop d'appels d'un coup : les `MAX_TOOL_CALLS` premiers exécutés, les
/// autres reçoivent « trop d'appels » (chaque appel a sa réponse).
#[tokio::test(flavor = "multi_thread")]
async fn tool_calls_are_capped_per_turn() {
    let bench = bench(&[(MANY, "agent-many-tools")]);
    let lookup = Lookup::new("lookup");
    let worker = open(&bench, MANY).await;
    let text = worker
        .run_tools(
            vec![Message::user("Look everything up.")],
            &[&lookup],
            1,
            &write_options(ThinkMode::Off, None),
        )
        .await
        .unwrap();
    assert_eq!(text.content, format!("{} results.", fake::MANY_CALLS));
    let queries: Vec<Value> = (0..MAX_TOOL_CALLS)
        .map(|n| json!({ "query": format!("q{n}") }))
        .collect();
    assert_eq!(lookup.calls(), queries);
    let sent = chats(worker.endpoint()).await;
    let results: Vec<String> = sent[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["content"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(results.len(), fake::MANY_CALLS);
    for (n, result) in results.iter().enumerate() {
        if n < MAX_TOOL_CALLS {
            assert_eq!(*result, format!("result for q{n}"));
        } else {
            assert!(result.starts_with("Error: too many tool calls"), "{result}");
        }
    }
    worker.release();
    bench.shutdown().await;
}

/// Résultats d'outils trop longs pour le contexte (8 × 30 000 caractères
/// dans 4 096 jetons) : bornés ensemble à la place restante, coupés avec une
/// mention ; la requête suivante tient (sans borne : contexte dépassé).
#[tokio::test(flavor = "multi_thread")]
async fn tool_results_are_bounded_by_the_room_left_in_the_context() {
    let bench = bench(&[(MANY, "agent-many-tools")]);
    let lookup = Lookup {
        padding: 30_000,
        ..Lookup::new("lookup")
    };
    let worker = open(&bench, MANY).await;
    assert_eq!(worker.context(), 4096);
    let text = worker
        .run_tools(
            vec![Message::user("Look everything up.")],
            &[&lookup],
            1,
            &write_options(ThinkMode::Off, None),
        )
        .await
        .unwrap();
    assert_eq!(text.content, format!("{} results.", fake::MANY_CALLS));
    let sent = chats(worker.endpoint()).await;
    let results: Vec<String> = sent[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["content"].as_str().unwrap().to_owned())
        .collect();
    let total: usize = results.iter().map(|result| result.chars().count()).sum();
    // Au plus la moitié du contexte (3 caractères par jeton), plus les
    // mentions des résultats écartés.
    assert!(total <= 4096 / 2 * 3 + results.len() * 200, "{total}");
    assert!(results[0].starts_with("result for q0 data"));
    assert!(results[0].contains("[… cut: the context of the model is nearly full]"));
    assert!(
        results[1..MAX_TOOL_CALLS]
            .iter()
            .all(|result| result.starts_with("[result left out")),
        "{results:?}"
    );
    // Les messages courts (« trop d'appels ») passent toujours.
    assert!(
        results[MAX_TOOL_CALLS..]
            .iter()
            .all(|result| result.starts_with("Error: too many tool calls"))
    );
    worker.release();
    bench.shutdown().await;
}

/// Arrêt pendant un outil : le travail s'arrête, l'outil est prévenu.
#[tokio::test(flavor = "multi_thread")]
async fn stopping_interrupts_a_tool() {
    let bench = bench(&[(TOOLED, "tools")]);
    let cancel = CancellationToken::new();
    let worker = Worker::open(
        &bench.access,
        TOOLED,
        WorkerOptions {
            ctx_min: 4096,
            cancel: cancel.clone(),
        },
    )
    .await
    .unwrap();
    let hanging = Lookup {
        hang: true,
        ..Lookup::new("lookup")
    };
    let stop = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel.cancel();
    };
    let tools: [&dyn Tool; 1] = [&hanging];
    let options = write_options(ThinkMode::Off, None);
    let run = worker.run_tools(vec![Message::user("cats")], &tools, 4, &options);
    let (result, ()) = tokio::join!(tokio::time::timeout(Duration::from_secs(10), run), stop);
    assert_eq!(result.expect("stopped in time"), Err(AgentError::Cancelled));
    assert_eq!(hanging.calls().len(), 1);
    worker.release();
    bench.shutdown().await;
}

/// Contrat contre le vrai llama-server (`RealBench(Text)`, Qwen3 0.6B, qui
/// réfléchit) : avec un budget de 64 jetons, la réflexion s'arrête et la
/// réponse arrive (client seul, puis par le `Worker`) ; sans budget serré,
/// elle est bien plus longue.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "vrai llama-server : CHATONLM_LLAMA_TEST_BIN, sous verrou"]
async fn contract_agent_thinking_budget_ends_the_reasoning() {
    let Some(bench) = RealBench::new(Role::Text) else {
        return;
    };
    let worker = Worker::open(
        &bench.access,
        &bench.id,
        WorkerOptions {
            ctx_min: 4096,
            cancel: CancellationToken::new(),
        },
    )
    .await
    .unwrap();
    assert!(worker.thinks());
    let question = "What is 37 times 43, minus 19? Think it through, then give only the number.";
    // Client seul.
    let mut request = crate::inference::ChatRequest::new(vec![Message::user(question)]);
    request.think = inference::Think::On;
    request.thinking_budget = Some(64);
    request.temperature = Some(0.0);
    request.max_tokens = Some(64 + 600);
    let reply = inference::chat_once(worker.endpoint(), &request, CancellationToken::new())
        .await
        .unwrap();
    let thought = worker.count_tokens(&reply.reasoning).await.unwrap();
    eprintln!(
        "budget 64 : {thought} jetons de réflexion, fin {:?}, réponse {:?}",
        reply.finish_reason, reply.content
    );
    assert!(!reply.reasoning.is_empty());
    assert!(thought <= 64 + 8, "{thought}");
    // La réponse suit la réflexion close (un si petit modèle peut encore y
    // raisonner : sa fin n'est pas exigée ici).
    assert!(!reply.content.trim().is_empty());
    // Sans budget serré, le même modèle réfléchit bien plus longtemps : le
    // budget a coupé court.
    let mut free = request.clone();
    free.thinking_budget = Some(2000);
    free.max_tokens = Some(2000 + 600);
    let unbounded = inference::chat_once(worker.endpoint(), &free, CancellationToken::new())
        .await
        .unwrap();
    let free_thought = worker.count_tokens(&unbounded.reasoning).await.unwrap();
    eprintln!("budget 2000 : {free_thought} jetons de réflexion");
    assert!(free_thought > 2 * (64 + 8), "{free_thought}");
    // Par le Worker : réponse visible, sans réflexion (celle qu'un si petit
    // modèle continue d'écrire dans le contenu, après la réflexion close,
    // est retirée et comptée).
    let short = worker
        .write(
            vec![Message::user(question)],
            &Write {
                temperature: 0.0,
                think: ThinkMode::On { budget: 64 },
                max_tokens: Some(64 + 1500),
            },
            &|_| {},
        )
        .await
        .unwrap();
    eprintln!(
        "Worker : budget 64 → {} caractères de réflexion, réponse {:?}",
        short.reasoning_chars, short.content
    );
    assert!(!short.content.trim().is_empty());
    assert!(!short.content.contains("think>"), "{}", short.content);
    // JSON contraint avec réflexion bornée.
    let json = worker
        .ask_json(&Call {
            system: "Answer in JSON.".into(),
            user: question.into(),
            schema: json!({ "type": "object", "properties": { "result": { "type": "integer" } },
                "required": ["result"] }),
            temperature: 0.0,
            think: ThinkMode::On { budget: 64 },
            max_tokens: None,
        })
        .await
        .unwrap();
    eprintln!(
        "JSON : {} ({} caractères de réflexion)",
        json.value, json.reasoning_chars
    );
    assert!(json.value["result"].is_i64(), "{}", json.value);
    worker.release();
    bench.shutdown().await;
}
