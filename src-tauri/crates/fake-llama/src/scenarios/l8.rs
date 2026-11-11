//! Scénarios du lot L8 (contrat 102 § 1.5) : noms `chatonlm-<nom>` propres
//! à ce lot ; un nom unique dans tous les lots et qui ne contient aucun nom
//! intégré (`scenario.rs`, cherché d'abord) : essai du socle.
//!
//! Pour les e2e réécrits (raisonnement, contexte, images) :
//!
//! | Nom | Comportement |
//! |---|---|
//! | `chatonlm-echothink` | raisonne si la réflexion n'est pas coupée, puis cite ce qu'il a reçu : « Received think=<true, false, absent ou "niveau">. » (voir [`think_received`]) ; variante `chatonlm-echothink-plain` (modèle sans réflexion) : ne raisonne jamais |
//! | `chatonlm-stubborn` | raisonne MÊME réflexion coupée (comme LFM2.5), puis cite ce qu'il a reçu |
//! | `chatonlm-dawdle` | raisonnement de 60 lignes, une toutes les 150 ms (environ 9 s), puis la réponse |
//! | `chatonlm-ponder` | long raisonnement en 40 paragraphes numérotés (24 caractères toutes les 40 ms, environ 5 s), puis « Done thinking. » : il déborde de l'aperçu replié de l'app |
//! | `chatonlm-context` | réponse de 600 jetons ([`CONTEXT_ANSWER`] ×40 sans l'espace finale, un mot par morceau), ou [`CONTEXT_SUMMARY`] si le dernier message commence par « Summarize our conversation » ; `max_tokens` et le contexte la coupent (`length`, règle commune) |
//! | `chatonlm-image` | si `tools` offre `generate_image` et que le dernier message est de l'utilisateur : court raisonnement puis appel `{"prompt": <son texte>, "width": 512, "height": 512}` ; après un message `tool` : « Here is your image. » ; sans l'outil : « Image generation is not available. » |

use std::time::Duration;

use serde_json::Value;

use super::{ChatCall, Custom, Script, Step};

/// Phrase de la réponse de `chatonlm-context` : 15 mots, donc 15 jetons du
/// faux (un mot avec l'espace qui le précède, voir `tokens`).
pub const CONTEXT_ANSWER: &str =
    "This answer fills the context of the model with fifteen words in every single sentence. ";

/// Résumé écrit par `chatonlm-context`.
pub const CONTEXT_SUMMARY: &str = "The user said hello; the model answered at length.";

pub const SCENARIOS: &[Custom] = &[
    Custom {
        name: "chatonlm-echothink",
        chat: echothink,
        infill: None,
        load_ms: None,
        fail_load: false,
    },
    Custom {
        name: "chatonlm-stubborn",
        chat: stubborn,
        infill: None,
        load_ms: None,
        fail_load: false,
    },
    Custom {
        name: "chatonlm-dawdle",
        chat: dawdle,
        infill: None,
        load_ms: None,
        fail_load: false,
    },
    Custom {
        name: "chatonlm-ponder",
        chat: ponder,
        infill: None,
        load_ms: None,
        fail_load: false,
    },
    Custom {
        name: "chatonlm-context",
        chat: context,
        infill: None,
        load_ms: None,
        fail_load: false,
    },
    Custom {
        name: "chatonlm-image",
        chat: image,
        infill: None,
        load_ms: None,
        fail_load: false,
    },
];

/// Réglage de réflexion reçu, écrit comme l'ancien faux d'Ollama citait
/// `think` : le niveau entre guillemets (`reasoning_effort`, sauf `none` qui
/// coupe : `false`), sinon `chat_template_kwargs.enable_thinking`
/// (`true`/`false`), sinon `absent`.
pub fn think_received(body: &Value) -> String {
    match body["reasoning_effort"].as_str() {
        Some("none") => "false".into(),
        Some(level) => format!("\"{level}\""),
        None => match body["chat_template_kwargs"]["enable_thinking"].as_bool() {
            Some(enabled) => enabled.to_string(),
            None => "absent".into(),
        },
    }
}

fn received(call: &ChatCall) -> Step {
    Step::Content(format!("Received think={}.", think_received(call.body)))
}

fn echothink(call: &ChatCall) -> Script {
    let mut script = Vec::new();
    // Variante `-plain` : modèle sans réflexion, qui ne raisonne jamais
    // (comme un vrai modèle sans gabarit de raisonnement).
    let plain = call.model.to_ascii_lowercase().contains("echothink-plain");
    if !plain && think_received(call.body) != "false" {
        script.push(Step::Reasoning("Let me think about it.".into()));
    }
    script.push(received(call));
    script
}

fn stubborn(call: &ChatCall) -> Script {
    vec![Step::Reasoning("I think anyway.".into()), received(call)]
}

fn dawdle(_: &ChatCall) -> Script {
    let mut script = Vec::new();
    for line in 1..=60 {
        if line > 1 {
            script.push(Step::Wait(Duration::from_millis(150)));
        }
        script.push(Step::Reasoning(format!("Thinking step {line}.\n")));
    }
    script.push(Step::Content("Thought it through.".into()));
    script
}

fn ponder(_: &ChatCall) -> Script {
    let text: String = (1..=40)
        .map(|n| format!("Paragraph {n}: weighing one more option before answering.\n\n"))
        .collect();
    let mut script = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    for (index, piece) in chars.chunks(24).enumerate() {
        if index > 0 {
            script.push(Step::Wait(Duration::from_millis(40)));
        }
        script.push(Step::Reasoning(piece.iter().collect()));
    }
    script.push(Step::Content("Done thinking.".into()));
    script
}

/// Dernier message (rôle, contenu texte).
fn last(body: &Value) -> (String, String) {
    let message = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or_default();
    let content = match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    };
    (
        message["role"].as_str().unwrap_or_default().to_owned(),
        content,
    )
}

fn context(call: &ChatCall) -> Script {
    let (_, content) = last(call.body);
    let text = if content.starts_with("Summarize our conversation") {
        CONTEXT_SUMMARY.to_owned()
    } else {
        // Sans l'espace finale, qui ferait un morceau de plus.
        CONTEXT_ANSWER.repeat(40).trim_end().to_owned()
    };
    // Un mot par morceau, donc un jeton par morceau : 600 jetons pour la
    // réponse, et la coupure de `max_tokens` (règle commune) tombe au jeton
    // près.
    crate::tokens::pieces(&text)
        .into_iter()
        .map(Step::Content)
        .collect()
}

fn image(call: &ChatCall) -> Script {
    let (role, content) = last(call.body);
    let paints = call.body["tools"].as_array().is_some_and(|tools| {
        tools
            .iter()
            .any(|tool| tool["function"]["name"] == "generate_image")
    });
    if role == "tool" {
        return vec![Step::Content("Here is your image.".into())];
    }
    if paints && role == "user" {
        return vec![
            Step::Reasoning("The user wants a picture.".into()),
            Step::ToolCall {
                name: "generate_image".into(),
                arguments: serde_json::json!({ "prompt": content, "width": 512, "height": 512 })
                    .to_string(),
            },
        ];
    }
    vec![Step::Content("Image generation is not available.".into())]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn call(body: &Value) -> ChatCall<'_> {
        ChatCall { body, model: "m" }
    }

    fn contents(script: &Script) -> String {
        script
            .iter()
            .filter_map(|step| match step {
                Step::Content(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn reasons(script: &Script) -> bool {
        script.iter().any(|step| matches!(step, Step::Reasoning(_)))
    }

    #[test]
    fn the_received_think_is_quoted() {
        for (body, expected) in [
            (json!({}), "absent"),
            (
                json!({ "chat_template_kwargs": { "enable_thinking": true } }),
                "true",
            ),
            (
                json!({ "chat_template_kwargs": { "enable_thinking": false } }),
                "false",
            ),
            (json!({ "reasoning_effort": "high" }), "\"high\""),
            (json!({ "reasoning_effort": "none" }), "false"),
        ] {
            assert_eq!(think_received(&body), expected, "{body}");
            assert_eq!(
                contents(&echothink(&call(&body))),
                format!("Received think={expected}.")
            );
            assert_eq!(
                reasons(&echothink(&call(&body))),
                expected != "false",
                "{body}"
            );
            // Têtu : raisonne toujours.
            assert!(reasons(&stubborn(&call(&body))));
            // Sans réflexion : ne raisonne jamais, cite ce qu'il a reçu.
            let plain = ChatCall {
                body: &body,
                model: "chatonlm-e2e/chatonlm-echothink-plain-GGUF:Q4_K_M",
            };
            assert!(!reasons(&echothink(&plain)), "{body}");
            assert_eq!(
                contents(&echothink(&plain)),
                format!("Received think={expected}.")
            );
        }
    }

    #[test]
    fn slow_and_long_reasoning_come_before_the_answer() {
        let body = json!({});
        let slow = dawdle(&call(&body));
        assert_eq!(
            slow.iter()
                .filter(|step| matches!(step, Step::Reasoning(_)))
                .count(),
            60
        );
        assert_eq!(
            slow.iter()
                .filter(|step| matches!(step, Step::Wait(_)))
                .count(),
            59
        );
        assert_eq!(contents(&slow), "Thought it through.");
        let long = ponder(&call(&body));
        let reasoning: String = long
            .iter()
            .filter_map(|step| match step {
                Step::Reasoning(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(reasoning.starts_with("Paragraph 1:"));
        assert!(reasoning.contains("Paragraph 40:"));
        assert_eq!(contents(&long), "Done thinking.");
        assert!(matches!(long.last(), Some(Step::Content(_))));
    }

    #[test]
    fn the_context_scenario_fills_or_summarizes() {
        let hello = json!({ "messages": [{ "role": "user", "content": "hello" }] });
        let answer = context(&call(&hello));
        assert_eq!(contents(&answer), CONTEXT_ANSWER.repeat(40).trim_end());
        assert_eq!(answer.len(), 600, "un jeton par morceau");
        let summary = json!({ "messages": [
            { "role": "user", "content": "hello" },
            { "role": "user", "content": "Summarize our conversation so far." },
        ] });
        assert_eq!(contents(&context(&call(&summary))), CONTEXT_SUMMARY);
    }

    #[test]
    fn the_image_scenario_calls_the_tool_only_when_offered() {
        let offered = json!({
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "a red cat" }] }],
            "tools": [{ "type": "function", "function": { "name": "generate_image" } }],
        });
        let script = image(&call(&offered));
        let Some(Step::ToolCall { name, arguments }) = script.last() else {
            panic!("{script:?}");
        };
        assert_eq!(name, "generate_image");
        let arguments: Value = serde_json::from_str(arguments).unwrap();
        assert_eq!(
            arguments,
            json!({ "prompt": "a red cat", "width": 512, "height": 512 })
        );
        let after = json!({ "messages": [{ "role": "tool", "content": "done" }] });
        assert_eq!(contents(&image(&call(&after))), "Here is your image.");
        let without = json!({ "messages": [{ "role": "user", "content": "a red cat" }] });
        assert_eq!(
            contents(&image(&call(&without))),
            "Image generation is not available."
        );
    }
}
