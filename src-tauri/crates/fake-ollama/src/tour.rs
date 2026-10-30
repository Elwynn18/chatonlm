//! Visite guidée parlée de ChatonLM (méthode `tour.plan` du pont VS Code) :
//! réponses déterministes des modèles `chatonlm-tour*` à `/api/chat`.
//!
//! Avec un schéma `format` (`stream: false`), le plan est fait des lignes
//! numérotées du message de l'utilisateur (`   12 | code`) :
//! - `chatonlm-tour` : deux segments valides (première ligne seule, puis le
//!   reste ; un seul segment pour une ligne), en français si les consignes
//!   nomment « French », en anglais sinon ;
//! - `chatonlm-tour-invalid` : un segment hors de la plage, toujours (le
//!   moteur redemande une fois, puis refuse).
//!
//! Sans `format` : une courte réponse en flux.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};

use super::{End, Journal, json as json_response, ndjson, reply_lines};

/// Modèles de la visite guidée.
pub const TOUR_MODELS: &[&str] = &["chatonlm-tour", "chatonlm-tour-invalid"];

/// Textes des deux segments de `chatonlm-tour` (anglais, puis français).
pub const TOUR_TEXTS: [[&str; 2]; 2] = [
    [
        "This first line sets the scene.",
        "The rest of the code does the actual work.",
    ],
    [
        "Cette première ligne plante le décor.",
        "La suite du code fait le vrai travail.",
    ],
];

pub fn reply(journal: Arc<Journal>, model: &str, request: &Value) -> Response {
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let text_of = |role: &str| {
        messages
            .iter()
            .rev()
            .find(|message| message["role"] == role)
            .and_then(|message| message["content"].as_str())
            .unwrap_or_default()
            .to_owned()
    };
    if !request["format"].is_object() || request["stream"] != Value::Bool(false) {
        return ndjson(
            journal,
            reply_lines(model, "This is a guided tour.", 12),
            Duration::from_millis(10),
            End::Close,
        );
    }
    let numbers: Vec<u64> = text_of("user")
        .lines()
        .filter_map(|line| line.split_once(" | ")?.0.trim().parse().ok())
        .collect();
    let first = numbers.iter().copied().min().unwrap_or(1);
    let last = numbers.iter().copied().max().unwrap_or(first);
    let texts = TOUR_TEXTS[usize::from(text_of("system").contains("French"))];
    let segments = if model == "chatonlm-tour-invalid" {
        json!([{ "lines": [last + 1, last + 2], "text": texts[0] }])
    } else if first == last {
        json!([{ "lines": [first, last], "text": texts[0] }])
    } else {
        json!([
            { "lines": [first, first], "text": texts[0] },
            { "lines": [first + 1, last], "text": texts[1] },
        ])
    };
    json_response(
        StatusCode::OK,
        json!({
            "model": model,
            "created_at": "2026-10-30T00:00:00Z",
            "message": {
                "role": "assistant",
                "content": json!({ "segments": segments }).to_string(),
            },
            "done": true,
            "done_reason": "stop",
        })
        .to_string(),
    )
}
