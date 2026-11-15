//! Scénarios de la synthèse d'une transcription (« Synthétiser », outil
//! Transcription, `transcription::synthesis` de l'app) : réponses en flux
//! déterministes, d'après la consigne reçue.
//!
//! La consigne dit ce qui est demandé :
//!
//! - message de l'utilisateur avec [`PART_MARK`] (une partie de la
//!   transcription) : notes, une ligne `- ` avec les premiers mots de la
//!   partie ;
//! - consigne système avec [`DOCUMENT_MARK`] : document Markdown (titre,
//!   introduction, une ligne par note ou les premiers mots de la
//!   transcription, conclusion), dans la langue demandée (« in French », « in
//!   Spanish », sinon anglais) ;
//! - autrement (notes regroupées) : une ligne `- ` avec les premiers mots.
//!
//! | Nom | Comportement |
//! |---|---|
//! | `chatonlm-synthesis` | réponses ci-dessus, 15 ms entre deux morceaux |
//! | `chatonlm-synthesis-slow` | idem, 150 ms entre deux morceaux (annulation) |
//! | `chatonlm-synthesis-think` | modèle qui réfléchit : réflexion si elle n'est pas coupée, puis comme `chatonlm-synthesis` |
//! | `chatonlm-synthesis-stray` | document en espagnol, sauf si la demande contient [`RETRY_MARK`] (garde de la langue) |
//! | `chatonlm-synthesis-empty` | réponse vide (des blancs) |
//! | `chatonlm-synthesis-cut` | quelques morceaux du document, puis connexion coupée |

use std::time::Duration;

use serde_json::Value;

use super::{ChatCall, Custom, Script, Step};

/// Message de l'utilisateur qui porte une partie de la transcription.
pub const PART_MARK: &str = "<transcript part=\"";
/// Consigne système du document final.
pub const DOCUMENT_MARK: &str = "reading document";
/// Rappel de la langue après une réponse dans une autre langue.
pub const RETRY_MARK: &str = "was not written in";

/// Réponse de `chatonlm-synthesis-stray` sans rappel.
pub const STRAY_DOCUMENT: &str = "# Síntesis\n\nEste documento resume la grabación que \
    hemos escuchado.\n\n## Puntos principales\n\nLos puntos son muy claros y también \
    están en el texto.\n\n## Conclusión\n\nEso es todo por esta grabación.";

/// Réflexion de `chatonlm-synthesis-think` (jamais dans le document).
pub const SYNTHESIS_THINKING: &str = "Let me think about the structure first.";

pub const SYNTHESIS: Custom = custom("chatonlm-synthesis", normal);
pub const SYNTHESIS_SLOW: Custom = custom("chatonlm-synthesis-slow", slow);
pub const SYNTHESIS_THINK: Custom = custom("chatonlm-synthesis-think", think);
pub const SYNTHESIS_STRAY: Custom = custom("chatonlm-synthesis-stray", stray);
pub const SYNTHESIS_EMPTY: Custom = custom("chatonlm-synthesis-empty", empty);
pub const SYNTHESIS_CUT: Custom = custom("chatonlm-synthesis-cut", cut);

pub const SCENARIOS: &[Custom] = &[
    SYNTHESIS,
    SYNTHESIS_SLOW,
    SYNTHESIS_THINK,
    SYNTHESIS_STRAY,
    SYNTHESIS_EMPTY,
    SYNTHESIS_CUT,
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

/// Texte d'un message (`content` en texte, sinon ses parties de texte).
fn text_of(message: &Value) -> String {
    match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Consigne système et dernier message de l'utilisateur.
fn system_and_user(call: &ChatCall) -> (String, String) {
    let messages = call.body["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let system = messages
        .iter()
        .filter(|message| message["role"] == "system")
        .map(text_of)
        .collect::<Vec<_>>()
        .join("\n");
    let user = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .map(text_of)
        .unwrap_or_default();
    (system, user)
}

/// Texte entre la première balise ouvrante et la balise fermante qui suit
/// (`<transcript …>…</transcript>`, `<notes>…</notes>`) ; tout le message
/// sinon.
fn inner(user: &str) -> String {
    for tag in ["transcript", "notes"] {
        let open = format!("<{tag}");
        let close = format!("</{tag}>");
        if let Some(start) = user.find(&open)
            && let Some(after) = user[start..].find('>')
        {
            let body = &user[start + after + 1..];
            let end = body.find(&close).unwrap_or(body.len());
            return body[..end].trim().to_owned();
        }
    }
    user.trim().to_owned()
}

/// Premiers mots d'un texte (sans tirets de liste).
fn first_words(text: &str, count: usize) -> String {
    text.split_whitespace()
        .filter(|word| *word != "-")
        .take(count)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Réponse attendue, d'après la consigne.
pub fn reply(call: &ChatCall) -> String {
    let (system, user) = system_and_user(call);
    let body = inner(&user);
    if !system.contains(DOCUMENT_MARK) {
        return format!("- {}", first_words(&body, 12));
    }
    let lines: Vec<String> = if user.contains("<notes") {
        body.lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix("- "))
            .map(|line| format!("- {line}"))
            .collect()
    } else {
        vec![format!("- {}", first_words(&body, 12))]
    };
    let points = lines.join("\n");
    if system.contains("in French") {
        format!(
            "# Synthèse\n\nCe document résume l'enregistrement.\n\n## Points principaux\n\n\
             {points}\n\n## Conclusion\n\nC'est tout pour cet enregistrement."
        )
    } else if system.contains("in Spanish") {
        format!(
            "# Síntesis\n\nEste documento resume la grabación.\n\n## Puntos principales\n\n\
             {points}\n\n## Conclusión\n\nEso es todo por esta grabación."
        )
    } else {
        format!(
            "# Summary\n\nThis document sums up the recording.\n\n## Main points\n\n\
             {points}\n\n## Conclusion\n\nThat is all for this recording."
        )
    }
}

/// Texte en morceaux d'un mot (blancs qui précèdent compris), `gap` entre
/// deux morceaux.
fn streamed(text: &str, gap: Duration) -> Script {
    crate::tokens::pieces(text)
        .into_iter()
        .flat_map(|piece| [Step::Wait(gap), Step::Content(piece)])
        .collect()
}

fn normal(call: &ChatCall) -> Script {
    streamed(&reply(call), Duration::from_millis(15))
}

fn slow(call: &ChatCall) -> Script {
    streamed(&reply(call), Duration::from_millis(150))
}

fn think(call: &ChatCall) -> Script {
    let off = call.body["chat_template_kwargs"]["enable_thinking"] == false;
    let mut script = Vec::new();
    if !off {
        script.push(Step::Reasoning(SYNTHESIS_THINKING.into()));
    }
    script.extend(normal(call));
    script
}

fn stray(call: &ChatCall) -> Script {
    let (system, user) = system_and_user(call);
    if system.contains(DOCUMENT_MARK) && !user.contains(RETRY_MARK) {
        return streamed(STRAY_DOCUMENT, Duration::from_millis(5));
    }
    normal(call)
}

fn empty(_: &ChatCall) -> Script {
    vec![Step::Content("  \n".into())]
}

fn cut(call: &ChatCall) -> Script {
    let mut script: Script = streamed(&reply(call), Duration::from_millis(10))
        .into_iter()
        .take(8)
        .collect();
    script.push(Step::Cut);
    script
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn call(body: &Value) -> ChatCall<'_> {
        ChatCall {
            body,
            model: "chatonlm-synthesis.gguf",
        }
    }

    fn content(script: &Script) -> String {
        script
            .iter()
            .filter_map(|step| match step {
                Step::Content(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn parts_give_notes_and_notes_give_a_document() {
        let part = json!({ "messages": [
            { "role": "system", "content": "Take notes." },
            { "role": "user", "content": "<transcript part=\"1/2\">\nAlpha beta gamma.\n</transcript>" }
        ]});
        assert_eq!(content(&normal(&call(&part))), "- Alpha beta gamma.");
        let document = json!({ "messages": [
            { "role": "system", "content": "Write a reading document. Write everything in French." },
            { "role": "user", "content": "<notes>\n- Un point.\n- Deux points.\n</notes>" }
        ]});
        let text = content(&normal(&call(&document)));
        assert!(text.starts_with("# Synthèse\n\n"), "{text}");
        assert!(text.contains("- Un point.\n- Deux points."), "{text}");
        // Transcription courte : document direct.
        let direct = json!({ "messages": [
            { "role": "system", "content": "Write a reading document." },
            { "role": "user", "content": "<transcript>\nHello there.\n</transcript>" }
        ]});
        let text = content(&normal(&call(&direct)));
        assert!(text.starts_with("# Summary"), "{text}");
        assert!(text.contains("- Hello there."), "{text}");
    }

    #[test]
    fn stray_answers_in_spanish_until_reminded() {
        let ask = |user: &str| {
            json!({ "messages": [
                { "role": "system", "content": "Write a reading document. Write everything in English." },
                { "role": "user", "content": user }
            ]})
        };
        let first = ask("<transcript>\nHello.\n</transcript>");
        assert_eq!(content(&stray(&call(&first))), STRAY_DOCUMENT);
        let again = ask(
            "Your previous answer was not written in English.\n<transcript>\nHello.\n</transcript>",
        );
        assert!(content(&stray(&call(&again))).starts_with("# Summary"));
    }

    #[test]
    fn thinking_stops_when_it_is_turned_off() {
        let on = json!({ "messages": [{ "role": "user", "content": "x" }] });
        assert_eq!(
            think(&call(&on)).first(),
            Some(&Step::Reasoning(SYNTHESIS_THINKING.into()))
        );
        let off = json!({ "messages": [{ "role": "user", "content": "x" }],
            "chat_template_kwargs": { "enable_thinking": false } });
        assert!(
            !think(&call(&off))
                .iter()
                .any(|step| matches!(step, Step::Reasoning(_)))
        );
        assert!(cut(&call(&on)).ends_with(&[Step::Cut]));
    }
}
