//! Scénarios de la correction (`proofread`) et du nettoyage de la dictée
//! (`dictation::polish`) : texte reçu entre `<text>` (ou `<transcript>`)
//! dans le dernier message de l'utilisateur.
//!
//! | Nom | Comportement |
//! |---|---|
//! | `chatonlm-proofer` | rend le texte avec les corrections de [`PROOFREAD_FIXES`] ; consignes qui demandent une réécriture (« rewrit ») : aussi [`PROOFREAD_REPHRASINGS`] |
//! | `chatonlm-proofer-slow` | idem, 150 ms entre deux morceaux |
//! | `chatonlm-proofer-off` | rend [`PROOFREAD_OFF`] (autre chose que le texte) |
//! | `chatonlm-proofer-think` | réflexion (coupée par `enable_thinking: false`), puis la correction |
//! | `chatonlm-proofer-opened` | réflexion écrite dans le contenu, seul `</think>` écrit, puis la correction |
//! | `chatonlm-proofer-cut` | trois morceaux, 200 ms, puis connexion coupée |
//! | `chatonlm-proofer-wait` | attend 5 s avant de répondre |
//! | `chatonlm-polisher` | « Polished: » suivi de la transcription |
//! | `chatonlm-polisher-think` | réflexion (coupée par `enable_thinking: false`), puis comme `chatonlm-polisher` |
//! | `chatonlm-polisher-poem` | un poème (le garde-fou le rejette) |

use std::time::Duration;

use serde_json::Value;

use super::super::{ChatCall, Custom, Script, Step};

/// Corrections fixes, appliquées au texte reçu dans l'ordre.
pub const PROOFREAD_FIXES: &[(&str, &str)] = &[
    ("teh", "the"),
    ("recieve", "receive"),
    ("cinema", "cinéma"),
    ("allez au", "allé au"),
];

/// Réécritures fixes de « Rephrase », après les corrections.
pub const PROOFREAD_REPHRASINGS: &[(&str, &str)] = &[
    ("a lot of", "many"),
    ("in order to", "to"),
    ("est-ce que tu peux", "peux-tu"),
];

/// Réflexion de `chatonlm-proofer-think` et `-opened`.
pub const PROOFREAD_THINKING: &str = "The user wants a correction. I keep the text as it is.";

/// Réponse de `chatonlm-proofer-off`.
pub const PROOFREAD_OFF: &str = "Sure! Here is a short poem about the sea:\n\
    Waves roll in under a silver sky,\nseagulls circle and the tide runs high.";

/// Réponse de `chatonlm-polisher-poem`.
pub const POLISH_POEM: &str =
    "Roses are red,\nviolets are blue,\nthe ocean is deep\nand so is the view.";

/// Scénarios de ce fichier (listés dans `l5a.rs`).
pub const PROOFER: Custom = custom("chatonlm-proofer", proofer);
pub const PROOFER_SLOW: Custom = custom("chatonlm-proofer-slow", proofer_slow);
pub const PROOFER_OFF: Custom = custom("chatonlm-proofer-off", proofer_off);
pub const PROOFER_THINK: Custom = custom("chatonlm-proofer-think", proofer_think);
pub const PROOFER_OPENED: Custom = custom("chatonlm-proofer-opened", proofer_opened);
pub const PROOFER_CUT: Custom = custom("chatonlm-proofer-cut", proofer_cut);
pub const PROOFER_WAIT: Custom = custom("chatonlm-proofer-wait", proofer_wait);
pub const POLISHER: Custom = custom("chatonlm-polisher", polisher);
pub const POLISHER_THINK: Custom = custom("chatonlm-polisher-think", polisher_think);
pub const POLISHER_POEM: Custom = custom("chatonlm-polisher-poem", polisher_poem);

const fn custom(name: &'static str, chat: fn(&ChatCall) -> Script) -> Custom {
    Custom {
        name,
        chat,
        infill: None,
        load_ms: None,
        fail_load: false,
    }
}

/// Texte corrigé (ou reformulé) par le faux serveur.
pub fn proofread(text: &str, rephrase: bool) -> String {
    let mut text = text.to_owned();
    for (from, to) in PROOFREAD_FIXES {
        text = text.replace(from, to);
    }
    if rephrase {
        for (from, to) in PROOFREAD_REPHRASINGS {
            text = text.replace(from, to);
        }
    }
    text
}

fn messages(call: &ChatCall) -> Vec<Value> {
    call.body["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Dernier message de l'utilisateur, sans les balises `tag`.
fn last_user(call: &ChatCall, tag: &str) -> String {
    let messages = messages(call);
    let last = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .map(crate::scenario::text_of)
        .unwrap_or_default();
    let open = format!("<{tag}>\n");
    let close = format!("\n</{tag}>");
    last.strip_prefix(&open)
        .and_then(|text| text.strip_suffix(&close))
        .unwrap_or(&last)
        .to_owned()
}

/// Correction du texte reçu (réécriture si les consignes la demandent).
fn corrected(call: &ChatCall) -> String {
    let rephrase = messages(call)
        .first()
        .map(crate::scenario::text_of)
        .is_some_and(|system| system.contains("rewrit"));
    proofread(&last_user(call, "text"), rephrase)
}

/// Morceaux de 8 caractères (plusieurs fragments pour un court passage).
fn chunks(text: &str) -> Vec<String> {
    let characters: Vec<char> = text.chars().collect();
    characters
        .chunks(8)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

fn content(text: &str) -> Script {
    chunks(text).into_iter().map(Step::Content).collect()
}

fn slowly(text: &str) -> Script {
    chunks(text)
        .into_iter()
        .flat_map(|piece| [Step::Wait(Duration::from_millis(150)), Step::Content(piece)])
        .collect()
}

fn thinking_off(call: &ChatCall) -> bool {
    crate::scenario::thinking_off(call.body)
}

fn reasoning(call: &ChatCall) -> Script {
    if thinking_off(call) {
        return Vec::new();
    }
    chunks(PROOFREAD_THINKING)
        .into_iter()
        .map(Step::Reasoning)
        .collect()
}

fn proofer(call: &ChatCall) -> Script {
    content(&corrected(call))
}

fn proofer_slow(call: &ChatCall) -> Script {
    slowly(&corrected(call))
}

fn proofer_off(_: &ChatCall) -> Script {
    content(PROOFREAD_OFF)
}

fn proofer_think(call: &ChatCall) -> Script {
    let mut script = reasoning(call);
    script.extend(content(&corrected(call)));
    script
}

fn proofer_opened(call: &ChatCall) -> Script {
    content(&format!(
        "{PROOFREAD_THINKING} {PROOFREAD_THINKING}\n</think>\n\n{}",
        corrected(call)
    ))
}

fn proofer_cut(call: &ChatCall) -> Script {
    let mut script: Script = content(&corrected(call)).into_iter().take(3).collect();
    // Morceaux reçus avant la coupure (sinon en-têtes et coupure partent
    // ensemble : une connexion refusée pour le client).
    script.push(Step::Wait(Duration::from_millis(200)));
    script.push(Step::Cut);
    script
}

fn proofer_wait(call: &ChatCall) -> Script {
    let mut script = vec![Step::Wait(Duration::from_secs(5))];
    script.extend(content(&corrected(call)));
    script
}

fn polished(call: &ChatCall) -> String {
    format!("Polished: {}", last_user(call, "transcript").trim())
}

fn polisher(call: &ChatCall) -> Script {
    content(&polished(call))
}

fn polisher_think(call: &ChatCall) -> Script {
    let mut script = reasoning(call);
    script.extend(content(&polished(call)));
    script
}

fn polisher_poem(_: &ChatCall) -> Script {
    content(POLISH_POEM)
}
