//! Scénarios de l'Atelier (moteur de tâches, `agent::Worker` de l'app) :
//!
//! | Nom | Comportement |
//! |---|---|
//! | `chatonlm-agent-flaky` | une réponse sur deux sans JSON ([`NO_JSON`]), l'autre [`GOOD_JSON`] (la première de chaque processus est la mauvaise) |
//! | `chatonlm-agent-garbage` | toujours [`NO_JSON`] |
//! | `chatonlm-agent-cut` | [`CUT_WORDS`] morceaux : avec `response_format`, l'objet `{"answer": "w0 … w39"}`, sinon les mots ; coupés à `max_tokens` (`length`) |
//! | `chatonlm-agent-busy` | première requête du processus : 500 ; ensuite [`GOOD_JSON`] avec `response_format`, sinon [`RECOVERED`] |
//! | `chatonlm-agent-odd-tools` | appelle `no_such_tool` (`{}`) puis le premier outil offert avec des arguments illisibles ; après des messages `tool` : leurs contenus joints par «  \| » |
//! | `chatonlm-agent-think` | réflexion de [`THINK_PIECES`] morceaux, bornée à `thinking_budget_tokens`, coupée par `enable_thinking: false` ; puis [`GOOD_JSON`] avec `response_format`, sinon [`THOUGHT`] |
//! | `chatonlm-agent-late-close` | réflexion écrite dans le CONTENU ([`LATE_THOUGHT`]) puis `</think>` sans `<think>` (Qwen3 au bout de son budget), puis [`LATE_ANSWER`] |
//! | `chatonlm-agent-many-tools` | sans message `tool` : [`MANY_CALLS`] appels du premier outil offert (`{"query": "q<n>"}`) ; ensuite « <n> results. » (nombre de messages `tool`) |

use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

use super::{ChatCall, Custom, Script, Step};

/// Réponse sans objet JSON.
pub const NO_JSON: &str = "Sorry, I cannot answer in JSON.";
/// Objet JSON attendu.
pub const GOOD_JSON: &str = r#"{"answer": "fine"}"#;
/// Réponse de `chatonlm-agent-busy` après l'erreur.
pub const RECOVERED: &str = "Recovered after the error.";
/// Réponse de `chatonlm-agent-think` sans schéma.
pub const THOUGHT: &str = "Thought it through.";
/// Mots de `chatonlm-agent-cut`.
pub const CUT_WORDS: usize = 40;
/// Morceaux de réflexion de `chatonlm-agent-think` sans budget.
pub const THINK_PIECES: usize = 12;
/// Réflexion de `chatonlm-agent-late-close`, écrite dans le contenu.
pub const LATE_THOUGHT: &str = "The user wants a count. Three apples, so three.";
/// Réponse de `chatonlm-agent-late-close`, après `</think>`.
pub const LATE_ANSWER: &str = "There are 3 apples.";
/// Appels faits d'un coup par `chatonlm-agent-many-tools`.
pub const MANY_CALLS: usize = 12;

pub const FLAKY: Custom = custom("chatonlm-agent-flaky", flaky);
pub const GARBAGE: Custom = custom("chatonlm-agent-garbage", garbage);
pub const CUT: Custom = custom("chatonlm-agent-cut", cut);
pub const BUSY: Custom = custom("chatonlm-agent-busy", busy);
pub const ODD_TOOLS: Custom = custom("chatonlm-agent-odd-tools", odd_tools);
pub const THINK: Custom = custom("chatonlm-agent-think", think);
pub const LATE_CLOSE: Custom = custom("chatonlm-agent-late-close", late_close);
pub const MANY_TOOLS: Custom = custom("chatonlm-agent-many-tools", many_tools);

pub const SCENARIOS: &[Custom] = &[
    FLAKY, GARBAGE, CUT, BUSY, ODD_TOOLS, THINK, LATE_CLOSE, MANY_TOOLS,
];

const fn custom(name: &'static str, chat: fn(&ChatCall) -> Script) -> Custom {
    Custom {
        name,
        chat,
        infill: None,
        load_ms: None,
        fail_load: false,
    }
}

/// Requêtes reçues par ce processus (un processus par modèle chargé).
static FLAKY_CALLS: AtomicUsize = AtomicUsize::new(0);
static BUSY_CALLS: AtomicUsize = AtomicUsize::new(0);

fn words(text: &str) -> Script {
    crate::tokens::pieces(text)
        .into_iter()
        .map(Step::Content)
        .collect()
}

fn structured(call: &ChatCall) -> bool {
    call.body.get("response_format").is_some()
}

fn flaky(_: &ChatCall) -> Script {
    if FLAKY_CALLS.fetch_add(1, Ordering::SeqCst).is_multiple_of(2) {
        words(NO_JSON)
    } else {
        words(GOOD_JSON)
    }
}

fn garbage(_: &ChatCall) -> Script {
    words(NO_JSON)
}

fn cut(call: &ChatCall) -> Script {
    let text = (0..CUT_WORDS)
        .map(|n| format!("w{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    if structured(call) {
        words(&json!({ "answer": text }).to_string())
    } else {
        words(&text)
    }
}

fn busy(call: &ChatCall) -> Script {
    if BUSY_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
        return vec![Step::Fail {
            status: 500,
            body: json!({ "error": { "code": 500, "message": "busy", "type": "server_error" } }),
        }];
    }
    words(if structured(call) {
        GOOD_JSON
    } else {
        RECOVERED
    })
}

fn odd_tools(call: &ChatCall) -> Script {
    let messages = call.body["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let results: Vec<String> = messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(crate::scenario::text_of)
        .collect();
    if !results.is_empty() {
        return words(&results.join(" | "));
    }
    let first = call.body["tools"][0]["function"]["name"]
        .as_str()
        .unwrap_or("none")
        .to_owned();
    vec![
        Step::ToolCall {
            name: "no_such_tool".into(),
            arguments: "{}".into(),
        },
        Step::ToolCall {
            name: first,
            arguments: "{oops".into(),
        },
    ]
}

fn think(call: &ChatCall) -> Script {
    let mut script = Script::new();
    if !crate::scenario::thinking_off(call.body) {
        let budget = call.body["thinking_budget_tokens"]
            .as_u64()
            .map_or(THINK_PIECES, |budget| budget as usize);
        script
            .extend((0..THINK_PIECES.min(budget)).map(|n| Step::Reasoning(format!("step {n}. "))));
    }
    script.extend(words(if structured(call) { GOOD_JSON } else { THOUGHT }));
    script
}

fn late_close(_: &ChatCall) -> Script {
    words(&format!("{LATE_THOUGHT}</think>\n\n{LATE_ANSWER}"))
}

fn many_tools(call: &ChatCall) -> Script {
    let results = call.body["messages"].as_array().map_or(0, |messages| {
        messages
            .iter()
            .filter(|message| message["role"] == "tool")
            .count()
    });
    if results > 0 {
        return words(&format!("{results} results."));
    }
    let first = call.body["tools"][0]["function"]["name"]
        .as_str()
        .unwrap_or("none")
        .to_owned();
    (0..MANY_CALLS)
        .map(|n| Step::ToolCall {
            name: first.clone(),
            arguments: json!({ "query": format!("q{n}") }).to_string(),
        })
        .collect()
}
