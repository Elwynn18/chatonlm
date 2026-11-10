//! Scénarios du chat de la page (e2e chat, edit, markdown, read-aloud,
//! perf-stream), repris des modèles du même nom du faux Ollama :
//!
//! | Nom | Comportement |
//! |---|---|
//! | `chatonlm-markdown` | réponse Markdown de [`MARKDOWN`] en morceaux de 16 caractères, sans attente |
//! | `chatonlm-markdown-slow` | [`MARKDOWN_STREAM`] en morceaux de 6 caractères, 100 ms entre deux (environ 8 s) |
//! | `chatonlm-markdown-long` | [`MARKDOWN_LONG`] en morceaux de 24 caractères, 15 ms entre deux (environ 4,5 s) |
//! | `chatonlm-greeter` | toujours [`GREETING`] (un morceau), quels que soient le message et ses images : réponse fixe des e2e des pièces jointes et de la modification (avec `vision` : projecteur inscrit) |
//! | `chatonlm-brisk` | réponse rapide et continue : [`BRISK_PIECES`] morceaux de réflexion puis « Hello! You said: <dernier message> », 10 ms avant chaque morceau (environ 2 s) |
//!
//! Le découpage est en caractères, comme l'ancien faux (pas en jetons) : un
//! morceau compte pour un jeton (règle commune de [`super::super::limit`]).

use std::time::Duration;

use super::super::{ChatCall, Custom, Script, Step};

/// Réponse Markdown complète (titres, listes, tableau, code, formules).
pub const MARKDOWN: &str = include_str!("../../../fixtures/markdown.md");
/// Réponse Markdown courte, écrite lentement (bloc de code ouvert pendant le
/// flux).
pub const MARKDOWN_STREAM: &str = include_str!("../../../fixtures/markdown-stream.md");
/// Longue réponse Markdown (lecture à voix haute, mesures du flux).
pub const MARKDOWN_LONG: &str = include_str!("../../../fixtures/markdown-long.md");
/// Réponse de `chatonlm-greeter`.
pub const GREETING: &str = "Hi!";
/// Morceaux de réflexion de `chatonlm-brisk`.
pub const BRISK_PIECES: usize = 160;
/// Phrase de réflexion de `chatonlm-brisk`, répétée.
pub const BRISK_THOUGHT: &str = "Let me answer quickly and keep the stream going. ";

pub const SCENARIOS: &[Custom] = &[
    custom("chatonlm-markdown", markdown),
    custom("chatonlm-markdown-slow", markdown_slow),
    custom("chatonlm-markdown-long", markdown_long),
    custom("chatonlm-brisk", brisk),
    custom("chatonlm-greeter", greeter),
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

/// `text` en morceaux de `size` caractères, `wait` avant chacun sauf le
/// premier.
fn chunked(text: &str, size: usize, wait: Duration) -> Script {
    let chars: Vec<char> = text.chars().collect();
    let mut script = Vec::new();
    for (index, piece) in chars.chunks(size).enumerate() {
        if index > 0 && !wait.is_zero() {
            script.push(Step::Wait(wait));
        }
        script.push(Step::Content(piece.iter().collect()));
    }
    script
}

fn markdown(_: &ChatCall) -> Script {
    chunked(MARKDOWN, 16, Duration::ZERO)
}

fn markdown_slow(_: &ChatCall) -> Script {
    chunked(MARKDOWN_STREAM, 6, Duration::from_millis(100))
}

fn markdown_long(_: &ChatCall) -> Script {
    chunked(MARKDOWN_LONG, 24, Duration::from_millis(15))
}

fn greeter(_: &ChatCall) -> Script {
    vec![Step::Content(GREETING.into())]
}

fn brisk(call: &ChatCall) -> Script {
    let last = call.body["messages"]
        .as_array()
        .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "user"))
        .map(crate::scenario::text_of)
        .unwrap_or_default();
    let wait = Duration::from_millis(10);
    let thought = crate::tokens::pieces(BRISK_THOUGHT);
    let mut script = Vec::new();
    for piece in thought.iter().cycle().take(BRISK_PIECES) {
        script.push(Step::Wait(wait));
        script.push(Step::Reasoning(piece.clone()));
    }
    for piece in crate::tokens::pieces(&format!("Hello! You said: {last}")) {
        script.push(Step::Wait(wait));
        script.push(Step::Content(piece));
    }
    script
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn play(scenario: fn(&ChatCall) -> Script, body: &Value) -> Script {
        scenario(&ChatCall { body, model: "m" })
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

    fn waits(script: &Script) -> Duration {
        script
            .iter()
            .filter_map(|step| match step {
                Step::Wait(wait) => Some(*wait),
                _ => None,
            })
            .sum()
    }

    fn sizes(script: &Script) -> Vec<usize> {
        script
            .iter()
            .filter_map(|step| match step {
                Step::Content(text) => Some(text.chars().count()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn markdown_replies_are_the_fixtures_in_chunks() {
        let body = json!({ "messages": [{ "role": "user", "content": "Hi" }] });
        for (scenario, text, size, every) in [
            (markdown as fn(&ChatCall) -> Script, MARKDOWN, 16, 0),
            (markdown_slow, MARKDOWN_STREAM, 6, 100),
            (markdown_long, MARKDOWN_LONG, 24, 15),
        ] {
            let script = play(scenario, &body);
            assert_eq!(contents(&script), text);
            let sizes = sizes(&script);
            let (last, rest) = sizes.split_last().unwrap();
            assert!(rest.iter().all(|n| *n == size), "{size}");
            assert!(*last <= size);
            let expected = Duration::from_millis(every) * (sizes.len() as u32 - 1);
            assert_eq!(waits(&script), expected);
        }
        // Rien d'une réflexion.
        let script = play(markdown, &body);
        assert!(!script.iter().any(|s| matches!(s, Step::Reasoning(_))));
        // Fixtures réelles (pas vides) : l'écriture lente dure plusieurs
        // secondes.
        assert!(waits(&play(markdown_slow, &body)) >= Duration::from_secs(5));
    }

    #[test]
    fn brisk_streams_continuously_then_answers() {
        let body = json!({ "messages": [
            { "role": "user", "content": "First." },
            { "role": "assistant", "content": "Ok." },
            { "role": "user", "content": "Fast again." },
        ] });
        let script = play(brisk, &body);
        assert_eq!(contents(&script), "Hello! You said: Fast again.");
        let reasoning = script
            .iter()
            .filter(|step| matches!(step, Step::Reasoning(_)))
            .count();
        assert_eq!(reasoning, BRISK_PIECES);
        // Une attente de 10 ms avant chaque morceau.
        let produced = script.iter().filter(|step| step.produces()).count();
        assert_eq!(waits(&script), Duration::from_millis(10) * produced as u32);
        assert!(matches!(script.first(), Some(Step::Wait(_))));
    }

    #[test]
    fn the_greeter_always_says_hi() {
        let image = json!({ "messages": [{ "role": "user", "content": [
            { "type": "text", "text": "Describe." },
            { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw==" } },
        ] }] });
        for body in [json!({ "messages": [] }), image] {
            assert_eq!(play(greeter, &body), vec![Step::Content("Hi!".into())]);
        }
    }

    #[test]
    fn the_scenarios_are_found_by_name() {
        for name in [
            "chatonlm-markdown",
            "chatonlm-markdown-slow",
            "chatonlm-markdown-long",
            "chatonlm-brisk",
            "chatonlm-greeter",
        ] {
            let alias = format!("chatonlm-e2e/{name}-GGUF:Q4_K_M");
            assert_eq!(
                crate::scenario::Scenario::find(Some(&alias), "x.gguf"),
                None,
                "{name} est pris par un scénario intégré"
            );
            assert_eq!(
                super::super::super::find(Some(&alias), "x.gguf").map(|c| c.name),
                Some(name)
            );
        }
    }
}
