//! Formes du faux comparées aux réponses enregistrées sur le vrai
//! `llama-server` (`fixtures/`, `scripts/record-fixtures.sh`) : chaque champ
//! que l'app lit (module `inference` et moteur) est rendu par le vrai ET par
//! le faux. Une fixture réenregistrée sur une nouvelle version de llama.cpp
//! qui perd un de ces champs fait échouer l'essai : l'app est à revoir.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, header};
use axum::response::Response;
use serde_json::{Value, json};

use crate::{Config, Shared, handle};

const PROPS: &str = include_str!("../fixtures/props.json");
const CHAT_ONCE: &str = include_str!("../fixtures/chat-once.json");
const CHAT_THINK: &str = include_str!("../fixtures/chat-think.sse");
const CHAT_NOTHINK: &str = include_str!("../fixtures/chat-nothink.sse");
const INFILL: &str = include_str!("../fixtures/infill.json");
const UNAUTHORIZED: &str = include_str!("../fixtures/unauthorized.json");

/// Champs de `/props` lus par l'app (contexte, alias, vision, capacités du
/// gabarit, sommeil).
const PROPS_FIELDS: &[&str] = &[
    "/default_generation_settings/n_ctx",
    "/total_slots",
    "/model_alias",
    "/model_path",
    "/modalities/vision",
    "/chat_template_caps/supports_tools",
    "/chat_template_caps/supports_reasoning_effort",
    "/is_sleeping",
];

/// Champs d'une réponse entière.
const ONCE_FIELDS: &[&str] = &[
    "/choices/0/message/content",
    "/choices/0/finish_reason",
    "/usage/prompt_tokens",
    "/usage/completion_tokens",
    "/timings/cache_n",
    "/timings/prompt_n",
    "/timings/predicted_n",
];

/// Champs de `/infill`.
const INFILL_FIELDS: &[&str] = &[
    "/content",
    "/tokens_predicted",
    "/tokens_evaluated",
    "/stop_type",
    "/timings/predicted_n",
];

fn shared(config: Config) -> Arc<Shared> {
    Shared::new(config).0
}

async fn call(
    shared: &Arc<Shared>,
    method: Method,
    path: &str,
    key: Option<&str>,
    body: Value,
) -> Response {
    let mut headers = HeaderMap::new();
    if let Some(key) = key {
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {key}").parse().unwrap(),
        );
    }
    let body = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(body.to_string())
    };
    handle(
        State(shared.clone()),
        method,
        path.parse().unwrap(),
        headers,
        body,
    )
    .await
}

async fn text(response: Response) -> (u16, String) {
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

/// Blocs JSON d'un flux SSE (sans `[DONE]`) ; vrai si le flux finit par
/// `[DONE]`.
fn events(sse: &str) -> (Vec<Value>, bool) {
    let blocks: Vec<&str> = sse
        .split("\n\n")
        .filter(|block| !block.trim().is_empty())
        .collect();
    let done = blocks
        .last()
        .is_some_and(|block| block.trim() == "data: [DONE]");
    let events = blocks
        .iter()
        .filter_map(|block| block.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    (events, done)
}

/// Champs absents de `value` parmi `fields`.
fn missing(value: &Value, fields: &[&str]) -> Vec<String> {
    fields
        .iter()
        .filter(|field| value.pointer(field).is_none())
        .map(|field| (*field).to_owned())
        .collect()
}

/// Forme d'un flux de chat : premier bloc avec le rôle, contenu (et
/// réflexion) dans `delta`, une fin, puis l'usage et les mesures dans un
/// dernier bloc sans `choices`, et `[DONE]`.
fn stream_shape(events: &[Value], done: bool) -> (bool, bool, Option<String>, bool) {
    let delta = |event: &Value| event.pointer("/choices/0/delta").cloned();
    let role = events
        .first()
        .and_then(delta)
        .is_some_and(|delta| delta["role"] == "assistant");
    let reasoning = events
        .iter()
        .filter_map(delta)
        .any(|delta| delta["reasoning_content"].is_string());
    let finish = events.iter().find_map(|event| {
        event
            .pointer("/choices/0/finish_reason")?
            .as_str()
            .map(str::to_owned)
    });
    let last = events.last();
    let usage = done
        && last.is_some_and(|last| {
            last["choices"].as_array().is_some_and(Vec::is_empty)
                && last.pointer("/usage/prompt_tokens").is_some()
                && last.pointer("/timings/predicted_n").is_some()
        });
    (role, reasoning, finish, usage)
}

fn chat_body(stream: bool, thinking: bool) -> Value {
    json!({
        "messages": [{ "role": "user", "content": "Say hello in one word." }],
        "stream": stream,
        "max_tokens": 96,
        "chat_template_kwargs": { "enable_thinking": thinking },
        "stream_options": { "include_usage": true },
    })
}

#[tokio::test]
async fn props_and_whole_answers_have_the_fields_the_app_reads() {
    let real_props: Value = serde_json::from_str(PROPS).unwrap();
    let real_once: Value = serde_json::from_str(CHAT_ONCE).unwrap();
    assert_eq!(
        missing(&real_props, PROPS_FIELDS),
        Vec::<String>::new(),
        "vrai /props"
    );
    assert_eq!(
        missing(&real_once, ONCE_FIELDS),
        Vec::<String>::new(),
        "vraie réponse entière"
    );
    let fake = shared(Config::new("/m/chatonlm-tools.gguf").alias("a/chatonlm-tools-GGUF:Q4_K_M"));
    let (_, props) = text(call(&fake, Method::GET, "/props", None, Value::Null).await).await;
    let props: Value = serde_json::from_str(&props).unwrap();
    assert_eq!(
        missing(&props, PROPS_FIELDS),
        Vec::<String>::new(),
        "faux /props"
    );
    let (status, once) = text(
        call(
            &fake,
            Method::POST,
            "/v1/chat/completions",
            None,
            chat_body(false, false),
        )
        .await,
    )
    .await;
    assert_eq!(status, 200, "{once}");
    let once: Value = serde_json::from_str(&once).unwrap();
    assert_eq!(
        missing(&once, ONCE_FIELDS),
        Vec::<String>::new(),
        "fausse réponse entière"
    );
}

#[tokio::test]
async fn streams_have_the_shape_of_the_real_ones() {
    let (real_think, done) = events(CHAT_THINK);
    assert_eq!(
        stream_shape(&real_think, done),
        (true, true, Some("stop".into()), true)
    );
    let (real_plain, done) = events(CHAT_NOTHINK);
    assert_eq!(
        stream_shape(&real_plain, done),
        (true, false, Some("stop".into()), true)
    );

    let fake = shared(Config::new("/m/chatonlm-think.gguf"));
    let (_, sse) = text(
        call(
            &fake,
            Method::POST,
            "/v1/chat/completions",
            None,
            chat_body(true, true),
        )
        .await,
    )
    .await;
    let (think, done) = events(&sse);
    assert_eq!(
        stream_shape(&think, done),
        (true, true, Some("stop".into()), true),
        "{sse}"
    );
    let (_, sse) = text(
        call(
            &fake,
            Method::POST,
            "/v1/chat/completions",
            None,
            chat_body(true, false),
        )
        .await,
    )
    .await;
    let (plain, done) = events(&sse);
    assert_eq!(
        stream_shape(&plain, done),
        (true, false, Some("stop".into()), true),
        "{sse}"
    );
}

#[tokio::test]
async fn infill_and_refusals_answer_like_the_real_server() {
    let real_infill: Value = serde_json::from_str(INFILL).unwrap();
    assert_eq!(
        missing(&real_infill, INFILL_FIELDS),
        Vec::<String>::new(),
        "vrai /infill"
    );
    let fake = shared(Config::new("/m/chatonlm-fim.gguf").api_key("k"));
    tokio::time::sleep(Duration::from_millis(10)).await;
    let body =
        json!({ "input_prefix": "def add(a, b):\n    ", "input_suffix": "\n", "n_predict": 16 });
    let (status, infill) = text(call(&fake, Method::POST, "/infill", Some("k"), body).await).await;
    assert_eq!(status, 200, "{infill}");
    let infill: Value = serde_json::from_str(&infill).unwrap();
    assert_eq!(
        missing(&infill, INFILL_FIELDS),
        Vec::<String>::new(),
        "faux /infill"
    );

    let real: Value = serde_json::from_str(UNAUTHORIZED).unwrap();
    let (status, refused) =
        text(call(&fake, Method::GET, "/v1/models", Some("bad"), Value::Null).await).await;
    assert_eq!(u64::from(status), real["status"].as_u64().unwrap());
    let refused: Value = serde_json::from_str(&refused).unwrap();
    assert_eq!(refused, real["body"]);
}
