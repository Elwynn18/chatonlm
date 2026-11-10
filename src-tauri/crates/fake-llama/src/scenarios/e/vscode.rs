//! Extension VS Code (pont) : visite guidée, reprise du modèle `chatonlm-tour`
//! de l'ancien faux Ollama (le chaton en caoutchouc et la voix emploient
//! `chatonlm-voice`, voir `e/voice.rs`).
//!
//! | Nom | Comportement |
//! |---|---|
//! | `chatonlm-vstour` | plan de visite (`tour.plan`) : bornes lues dans le schéma (`lines.items.minimum`/`maximum`) ; une ligne : un segment ; sinon la première ligne, puis le reste ; textes de [`TOUR_TEXTS`], en français si les consignes (message système) nomment « French » ; sans `response_format` : « This is a guided tour. » |

use serde_json::{Value, json};

use super::super::{ChatCall, Custom, Script, Step};
use crate::scenario::text_of;

/// Textes des deux segments de `chatonlm-vstour` (anglais, puis français).
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

pub const SCENARIOS: &[Custom] = &[Custom {
    name: "chatonlm-vstour",
    chat: tour,
    infill: None,
    load_ms: None,
    fail_load: false,
}];

fn messages(call: &ChatCall) -> Vec<Value> {
    call.body["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Texte du dernier message de ce rôle.
fn last_of(call: &ChatCall, role: &str) -> String {
    messages(call)
        .iter()
        .rev()
        .find(|message| message["role"] == role)
        .map(text_of)
        .unwrap_or_default()
}

fn tour(call: &ChatCall) -> Script {
    let schema = &call.body["response_format"]["json_schema"]["schema"];
    if !schema.is_object() {
        return vec![Step::Content("This is a guided tour.".into())];
    }
    let lines = &schema["properties"]["segments"]["items"]["properties"]["lines"]["items"];
    let first = lines["minimum"].as_u64().unwrap_or(1);
    let last = lines["maximum"].as_u64().unwrap_or(first);
    let texts = TOUR_TEXTS[usize::from(last_of(call, "system").contains("French"))];
    let segments = if first == last {
        json!([{ "lines": [first, last], "text": texts[0] }])
    } else {
        json!([
            { "lines": [first, first], "text": texts[0] },
            { "lines": [first + 1, last], "text": texts[1] },
        ])
    };
    vec![Step::Content(json!({ "segments": segments }).to_string())]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(body: &Value) -> ChatCall<'_> {
        ChatCall {
            body,
            model: "chatonlm-test",
        }
    }

    fn joined(script: &Script, reasoning: bool) -> String {
        script
            .iter()
            .filter_map(|step| match step {
                Step::Content(text) if !reasoning => Some(text.as_str()),
                Step::Reasoning(text) if reasoning => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn plan(system: &str, first: u64, last: u64) -> Value {
        let body = json!({
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": "   20 | fn main() {}" },
            ],
            "response_format": { "type": "json_schema", "json_schema": { "schema": {
                "properties": { "segments": { "items": { "properties": { "lines": {
                    "items": { "type": "integer", "minimum": first, "maximum": last }
                } } } } }
            } } },
        });
        serde_json::from_str(&joined(&tour(&call(&body)), false)).unwrap()
    }

    #[test]
    fn the_tour_follows_the_schema_bounds_and_language() {
        assert_eq!(
            plan("Write the texts in French.", 20, 23),
            json!({ "segments": [
                { "lines": [20, 20], "text": TOUR_TEXTS[1][0] },
                { "lines": [21, 23], "text": TOUR_TEXTS[1][1] },
            ] })
        );
        assert_eq!(
            plan("Write the texts in English.", 7, 7),
            json!({ "segments": [{ "lines": [7, 7], "text": TOUR_TEXTS[0][0] }] })
        );
        let free = json!({ "messages": [{ "role": "user", "content": "hi" }] });
        assert_eq!(joined(&tour(&call(&free)), false), "This is a guided tour.");
    }
}
