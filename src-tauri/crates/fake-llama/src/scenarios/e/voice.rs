//! Mode conversation vocale (e2e `voice-mode`) : réponses françaises lues par
//! la voix française, reprises du faux Ollama (`chatonlm-voice*`). Comme un
//! modèle qui raisonne par défaut (GGUF d'essai écrit avec `--think`),
//! [`VOICE_THINKING`] précède la réponse sauf si la réflexion est coupée
//! (`enable_thinking: false`, ce qu'envoie un envoi vocal). Morceaux de 8
//! caractères.
//!
//! | Nom | Comportement |
//! |---|---|
//! | `chatonlm-voice` | une phrase de [`VOICE_REPLIES`] selon le nombre de messages `user` reçus (la n-ième au n-ième, puis de nouveau la première), 20 ms entre deux morceaux |
//! | `chatonlm-voice-long` | [`VOICE_LONG_REPLY`] à chaque tour, 100 ms entre deux morceaux (environ 8 s : la voix peut être coupée pendant que le texte s'écrit encore) |

use std::time::Duration;

use serde_json::Value;

use super::super::{ChatCall, Custom, Script, Step};
use crate::scenario::thinking_off;

/// Réponses de `chatonlm-voice` : la première courte (un tour vite fini), la
/// seconde lue pendant plusieurs secondes (le temps de la couper par Espace).
pub const VOICE_REPLIES: [&str; 2] = [
    "Je vous entends très bien. Que puis-je faire pour vous ?",
    "C'est une belle histoire, tirée des Mille et Une Nuits. Voulez-vous que je vous \
     raconte ce que devient ensuite le pêcheur ?",
];

/// Réponse de `chatonlm-voice-long` : sa première phrase arrive en moins
/// d'une seconde, et sa dernière, [`VOICE_LONG_END`], montre que la réponse
/// est complète (un envoi coupé ne l'a pas).
pub const VOICE_LONG_REPLY: &str = "Voici une réponse plus longue, qui laisse le temps de me \
couper la parole. Un pêcheur jetait son filet quatre fois par jour, pas une de plus. Un matin, \
il remonta un vase de cuivre fermé par un sceau de plomb. Curieux, il ouvrit le vase avec son \
couteau, et une épaisse fumée s'en échappa. La fumée prit la forme d'un génie immense, qui \
menaça aussitôt de le tuer. Le pêcheur lui demanda comment un si grand génie pouvait tenir dans \
un si petit vase. Piqué au vif, le génie redevint fumée et rentra dans le vase pour le lui \
prouver. Le pêcheur referma le vase et le rejeta à la mer. Fin de l'histoire.";

/// Dernière phrase de [`VOICE_LONG_REPLY`].
pub const VOICE_LONG_END: &str = "Fin de l'histoire.";

/// Raisonnement quand la réflexion n'est pas coupée.
pub const VOICE_THINKING: &str = "On me parle à voix haute : je réponds brièvement.";

/// Attente entre deux morceaux de `chatonlm-voice`.
pub const VOICE_DELAY: Duration = Duration::from_millis(20);
/// Attente entre deux morceaux de `chatonlm-voice-long`.
pub const VOICE_LONG_DELAY: Duration = Duration::from_millis(100);

pub const SCENARIOS: &[Custom] = &[
    Custom {
        name: "chatonlm-voice",
        chat: voice,
        infill: None,
        load_ms: None,
        fail_load: false,
    },
    Custom {
        name: "chatonlm-voice-long",
        chat: voice_long,
        infill: None,
        load_ms: None,
        fail_load: false,
    },
];

/// Morceaux de 8 caractères.
fn chunks(text: &str) -> Vec<String> {
    let characters: Vec<char> = text.chars().collect();
    characters
        .chunks(8)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

/// Raisonnement (sauf réflexion coupée) puis réponse, `delay` avant chaque
/// morceau sauf le premier.
fn reasoned(body: &Value, text: &str, delay: Duration) -> Script {
    let mut steps = Vec::new();
    if !thinking_off(body) {
        steps.extend(chunks(VOICE_THINKING).into_iter().map(Step::Reasoning));
    }
    steps.extend(chunks(text).into_iter().map(Step::Content));
    let mut script = Vec::new();
    for (index, step) in steps.into_iter().enumerate() {
        if index > 0 {
            script.push(Step::Wait(delay));
        }
        script.push(step);
    }
    script
}

fn voice(call: &ChatCall) -> Script {
    let users = call.body["messages"].as_array().map_or(0, |messages| {
        messages
            .iter()
            .filter(|message| message["role"] == "user")
            .count()
    });
    let turn = users.saturating_sub(1) % VOICE_REPLIES.len();
    reasoned(call.body, VOICE_REPLIES[turn], VOICE_DELAY)
}

fn voice_long(call: &ChatCall) -> Script {
    reasoned(call.body, VOICE_LONG_REPLY, VOICE_LONG_DELAY)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn call(body: &Value) -> ChatCall<'_> {
        ChatCall { body, model: "m" }
    }

    fn joined(script: &Script, reasoning: bool) -> String {
        script
            .iter()
            .filter_map(|step| match step {
                Step::Reasoning(text) if reasoning => Some(text.as_str()),
                Step::Content(text) if !reasoning => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn waits(script: &Script) -> Vec<Duration> {
        script
            .iter()
            .filter_map(|step| match step {
                Step::Wait(delay) => Some(*delay),
                _ => None,
            })
            .collect()
    }

    fn turns(users: usize) -> Value {
        let mut messages = vec![json!({ "role": "system", "content": "Speak." })];
        for n in 0..users {
            if n > 0 {
                messages.push(json!({ "role": "assistant", "content": "…" }));
            }
            messages.push(json!({ "role": "user", "content": format!("phrase {n}") }));
        }
        json!({
            "messages": messages,
            "chat_template_kwargs": { "enable_thinking": false },
        })
    }

    #[test]
    fn each_turn_gets_its_reply_without_thinking_when_it_is_off() {
        for (users, expected) in [
            (1, VOICE_REPLIES[0]),
            (2, VOICE_REPLIES[1]),
            (3, VOICE_REPLIES[0]),
        ] {
            let script = voice(&call(&turns(users)));
            assert_eq!(joined(&script, false), expected, "{users} users");
            assert_eq!(joined(&script, true), "", "thinking while off");
            assert!(script.iter().all(|step| match step {
                Step::Content(text) => text.chars().count() <= 8,
                _ => true,
            }));
            assert!(waits(&script).iter().all(|delay| *delay == VOICE_DELAY));
        }
        assert_ne!(VOICE_REPLIES[0], VOICE_REPLIES[1]);
        assert!(VOICE_REPLIES[1].chars().count() >= 2 * VOICE_REPLIES[0].chars().count());
    }

    #[test]
    fn the_model_thinks_unless_told_not_to() {
        let body = json!({ "messages": [{ "role": "user", "content": "Bonjour" }] });
        let script = voice(&call(&body));
        assert_eq!(joined(&script, true), VOICE_THINKING);
        assert_eq!(joined(&script, false), VOICE_REPLIES[0]);
        // Raisonnement d'abord.
        assert!(matches!(script[0], Step::Reasoning(_)));
    }

    #[test]
    fn the_long_reply_is_slow_and_ends_with_its_last_sentence() {
        let script = voice_long(&call(&turns(2)));
        let text = joined(&script, false);
        assert_eq!(text, VOICE_LONG_REPLY);
        assert!(text.ends_with(VOICE_LONG_END));
        let delays = waits(&script);
        assert_eq!(delays.len(), chunks(VOICE_LONG_REPLY).len() - 1);
        assert!(delays.iter().all(|delay| *delay == VOICE_LONG_DELAY));
        // Au moins 5 s d'écriture : la voix peut être coupée avant la fin.
        assert!(delays.iter().sum::<Duration>() >= Duration::from_secs(5));
    }
}
