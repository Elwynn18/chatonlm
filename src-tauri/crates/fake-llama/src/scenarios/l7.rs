//! Scénarios du lot L7 (contrat 102 § 1.5) : noms `chatonlm-<nom>` propres
//! à ce lot ; un nom unique dans tous les lots et qui ne contient aucun nom
//! intégré (`scenario.rs`, cherché d'abord) : essai du socle.
//!
//! Le faux tient lieu de fournisseur distant (API d'OpenAI) :
//! - `chatonlm-remote-strict` : fournisseur strict comme Mistral, qui répond
//!   422 (`extra_forbidden`) à tout champ qu'il ne connaît pas, dans le corps
//!   ou dans un message, et à un identifiant d'appel qui n'a pas sa forme
//!   (neuf lettres ou chiffres) ; sinon « strict ok » ;
//! - `chatonlm-remote-openai` : API d'OpenAI, qui refuse de même un champ
//!   propre à llama.cpp (400, comme OpenAI) ; sinon « openai ok ».
//!
//! Et un modèle local qui cherche sur le web (e2e de la recherche) :
//! - `chatonlm-websearch` : sans résultat d'outil dans la conversation, appelle
//!   `web_search` avec le dernier message de l'utilisateur ; ensuite, répond
//!   « Found: » suivi du dernier résultat d'outil reçu (ce que le modèle voit,
//!   extraits compris).

use serde_json::{Value, json};

use super::{ChatCall, Custom, Script, Step};

pub const SCENARIOS: &[Custom] = &[
    Custom {
        name: "chatonlm-remote-strict",
        chat: strict,
        infill: None,
        load_ms: None,
        fail_load: false,
    },
    Custom {
        name: "chatonlm-remote-openai",
        chat: openai,
        infill: None,
        load_ms: None,
        fail_load: false,
    },
    Custom {
        name: "chatonlm-websearch",
        chat: websearch,
        infill: None,
        load_ms: None,
        fail_load: false,
    },
];

/// Champs du corps de Mistral.
const MISTRAL: &[&str] = &[
    "model",
    "messages",
    "stream",
    "tools",
    "response_format",
    "max_tokens",
    "temperature",
    "top_p",
    "presence_penalty",
    "frequency_penalty",
    "random_seed",
    "stop",
];

/// Champs du corps d'OpenAI (sous-ensemble utile).
const OPENAI: &[&str] = &[
    "model",
    "messages",
    "stream",
    "stream_options",
    "tools",
    "response_format",
    "max_tokens",
    "temperature",
    "top_p",
    "presence_penalty",
    "frequency_penalty",
    "seed",
    "stop",
];

const MESSAGE: &[&str] = &["role", "content", "tool_calls", "tool_call_id"];

/// Premier champ inconnu : `(endroit, nom)`.
fn unknown(body: &Value, top: &[&str]) -> Option<(String, String)> {
    let object = body.as_object()?;
    if let Some(key) = object.keys().find(|key| !top.contains(&key.as_str())) {
        return Some(("body".into(), key.clone()));
    }
    for (index, message) in body["messages"].as_array()?.iter().enumerate() {
        if let Some(key) = message
            .as_object()?
            .keys()
            .find(|key| !MESSAGE.contains(&key.as_str()))
        {
            return Some((format!("messages.{index}"), key.clone()));
        }
    }
    None
}

/// Identifiant d'appel refusé par Mistral.
fn bad_id(body: &Value) -> Option<String> {
    let valid = |id: &str| id.len() == 9 && id.bytes().all(|b| b.is_ascii_alphanumeric());
    for message in body["messages"].as_array()? {
        let ids = message["tool_calls"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|call| call["id"].as_str())
            .chain(message["tool_call_id"].as_str());
        for id in ids {
            if !valid(id) {
                return Some(id.to_owned());
            }
        }
    }
    None
}

fn strict(call: &ChatCall) -> Script {
    if let Some((place, key)) = unknown(call.body, MISTRAL) {
        return vec![Step::Fail {
            status: 422,
            body: json!({
                "object": "error",
                "message": { "detail": [{ "type": "extra_forbidden", "loc": [place, key],
                    "msg": "Extra inputs are not permitted" }] },
                "type": "invalid_request_error",
            }),
        }];
    }
    if let Some(id) = bad_id(call.body) {
        return vec![Step::Fail {
            status: 400,
            body: json!({ "object": "error", "message": format!("Tool call id was {id} but must be a-z, A-Z, 0-9, with a length of 9.") }),
        }];
    }
    vec![Step::Content("strict ok".into())]
}

fn openai(call: &ChatCall) -> Script {
    if let Some((_, key)) = unknown(call.body, OPENAI) {
        return vec![Step::Fail {
            status: 400,
            body: json!({ "error": { "message": format!("Unrecognized request argument supplied: {key}"),
                "type": "invalid_request_error" } }),
        }];
    }
    vec![Step::Content("openai ok".into())]
}

fn websearch(call: &ChatCall) -> Script {
    let messages = call.body["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let tool = messages
        .iter()
        .rev()
        .take_while(|message| message["role"] != "user")
        .find(|message| message["role"] == "tool");
    if let Some(tool) = tool {
        let content = tool["content"].as_str().unwrap_or_default();
        return vec![Step::Content(format!("Found: {content}"))];
    }
    let question = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .and_then(|message| message["content"].as_str())
        .unwrap_or_default();
    vec![Step::ToolCall {
        name: "web_search".into(),
        arguments: json!({ "query": question }).to_string(),
    }]
}
