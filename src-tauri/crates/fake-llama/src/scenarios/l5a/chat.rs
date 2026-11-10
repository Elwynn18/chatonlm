//! Scénarios du chat (`chat::`, envoi de la page, d'Easy Link et du pont) :
//!
//! | Nom | Comportement |
//! |---|---|
//! | `chatonlm-stubborn-think` | réflexion même avec `enable_thinking: false` (gabarit qui l'ouvre toujours), puis « Hello! » |
//! | `chatonlm-quiet-tools` | modèle à outils qui n'en appelle jamais : « Hello! You said: <dernier message> » |
//! | `chatonlm-insist-tools` | appelle `generate_image` (« a cat ») tant que des outils sont offerts, même après un résultat ; sans outils : [`INSIST_DONE`] |
//! | `chatonlm-rogue` | « Let me check. » et un appel `web_search`, même sans outils offerts |
//! | `chatonlm-sketch-think-tools` | réflexion (coupée par `enable_thinking: false`), puis un appel `generate_image` (512 × 512) avec le dernier message en description ; après un message `tool` : [`SKETCH_DONE`] |
//! | `chatonlm-pensive-think` | quatre morceaux de réflexion puis trois de réponse, 50 ms avant chacun |
//! | `chatonlm-mull-think` | réflexion qui répète sans fin le même paragraphe ([`MULL_PARAGRAPH`], un morceau chacun, 80 au plus) : boucle trouvée par le chat en quelques dizaines de morceaux |
//! | `chatonlm-gist` | [`GIST_SHORT`] ; un dernier message qui contient « at length » : 3 000 mots numérotés, tous différents, coupés à `max_tokens` (fin `length`) |
//! | `chatonlm-fetch-tools` | appelle `web_fetch` : adresse écrite après `FETCH ` dans le dernier message (`FETCH+ ` : suivie de `x` ; `FETCH FIRST` : la première du premier message de l'utilisateur ; `FETCH ANSWER` : la première écrite par le modèle dans ses réponses) ; après un message `tool` : « Fetched <adresse>: » et son contenu |

use super::super::{ChatCall, Custom, Script, Step};

/// Réponse de `chatonlm-insist-tools` sans outils offerts.
pub const INSIST_DONE: &str = "Done with the pictures.";
/// Réponse de `chatonlm-sketch-think-tools` après le résultat de l'outil.
pub const SKETCH_DONE: &str = "Here is your picture.";
/// Réflexion de `chatonlm-stubborn-think` et `chatonlm-sketch-think-tools`.
pub const CHAT_THINKING: &str = "The user wants something. Let me think.";
/// Réponse courte de `chatonlm-gist`.
pub const GIST_SHORT: &str = "OK.";
/// Paragraphe répété par `chatonlm-mull-think`.
pub const MULL_PARAGRAPH: &str = "Wait, let me check the question again. The user asked for one word, so I should answer with a single greeting. But maybe they want more context, so let me reconsider what a greeting is and check the question again before I answer. ";

pub const STUBBORN_THINK: Custom = custom("chatonlm-stubborn-think", stubborn_think);
pub const QUIET_TOOLS: Custom = custom("chatonlm-quiet-tools", quiet_tools);
pub const INSIST_TOOLS: Custom = custom("chatonlm-insist-tools", insist_tools);
pub const ROGUE: Custom = custom("chatonlm-rogue", rogue);
pub const SKETCH: Custom = custom("chatonlm-sketch-think-tools", sketch);
pub const GIST: Custom = custom("chatonlm-gist", gist);
pub const PENSIVE: Custom = custom("chatonlm-pensive-think", pensive);
pub const MULL: Custom = custom("chatonlm-mull-think", mull);
pub const FETCH: Custom = custom("chatonlm-fetch-tools", fetch);

const fn custom(name: &'static str, chat: fn(&ChatCall) -> Script) -> Custom {
    Custom {
        name,
        chat,
        infill: None,
        load_ms: None,
        fail_load: false,
    }
}

fn messages(call: &ChatCall) -> Vec<serde_json::Value> {
    call.body["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Texte du dernier message (de tout rôle).
fn last_text(call: &ChatCall) -> String {
    messages(call)
        .last()
        .map(crate::scenario::text_of)
        .unwrap_or_default()
}

/// Le dernier message est un résultat d'outil.
fn after_tool(call: &ChatCall) -> bool {
    messages(call)
        .last()
        .is_some_and(|message| message["role"] == "tool")
}

fn offered(call: &ChatCall) -> bool {
    call.body["tools"]
        .as_array()
        .is_some_and(|tools| !tools.is_empty())
}

/// Un morceau par mot (avec le blanc qui le précède), comme le faux.
fn words(text: &str) -> Script {
    crate::tokens::pieces(text)
        .into_iter()
        .map(Step::Content)
        .collect()
}

fn reasoning(call: &ChatCall) -> Script {
    if crate::scenario::thinking_off(call.body) {
        return Vec::new();
    }
    crate::tokens::pieces(CHAT_THINKING)
        .into_iter()
        .map(Step::Reasoning)
        .collect()
}

fn call_tool(name: &str, arguments: serde_json::Value) -> Step {
    Step::ToolCall {
        name: name.to_owned(),
        arguments: arguments.to_string(),
    }
}

fn stubborn_think(_: &ChatCall) -> Script {
    let mut script: Script = crate::tokens::pieces(CHAT_THINKING)
        .into_iter()
        .map(Step::Reasoning)
        .collect();
    script.extend(words("Hello!"));
    script
}

fn quiet_tools(call: &ChatCall) -> Script {
    words(&format!("Hello! You said: {}", last_text(call)))
}

fn insist_tools(call: &ChatCall) -> Script {
    if !offered(call) {
        return words(INSIST_DONE);
    }
    vec![call_tool(
        "generate_image",
        serde_json::json!({ "prompt": "a cat" }),
    )]
}

fn rogue(_: &ChatCall) -> Script {
    let mut script = words("Let me check.");
    script.push(call_tool(
        "web_search",
        serde_json::json!({ "query": "cats" }),
    ));
    script
}

fn sketch(call: &ChatCall) -> Script {
    let mut script = reasoning(call);
    if after_tool(call) {
        script.extend(words(SKETCH_DONE));
    } else {
        script.push(call_tool(
            "generate_image",
            serde_json::json!({ "prompt": last_text(call), "width": 512, "height": 512 }),
        ));
    }
    script
}

fn gist(call: &ChatCall) -> Script {
    if !last_text(call).contains("at length") {
        return words(GIST_SHORT);
    }
    // Coupé par le faux comme tout script (`scenarios::limit` : au plus
    // `max_tokens` morceaux, fin `length` comme le vrai).
    let long: Vec<String> = (1..=3000).map(|n| format!("word{n}")).collect();
    words(&long.join(" "))
}

/// `web_fetch` (relecture de l'intégration, I2). Sans résultat d'outil après
/// le dernier message de l'utilisateur, d'après ce message : `FETCH <url>`
/// lit l'adresse écrite ; `FETCH+ <url>` la même modifiée par le modèle (`x`
/// ajouté) ; `FETCH FIRST` la première adresse du premier message de
/// l'utilisateur ; `FETCH ANSWER` la première adresse écrite par le modèle
/// dans ses réponses. Ensuite : `Fetched <adresse>: <résultat ou erreur reçus>`.
fn fetch(call: &ChatCall) -> Script {
    let all = messages(call);
    // Première adresse écrite dans un message de ce rôle (un message
    // d'appel d'outil sans texte est passé).
    let first_url = |role: &str| {
        all.iter()
            .filter(|message| message["role"] == role)
            .map(crate::scenario::text_of)
            .find_map(|text| {
                text.split_whitespace()
                    .find(|word| word.starts_with("http"))
                    .map(|word| word.trim_end_matches([':', '.', ',']).to_owned())
            })
            .unwrap_or_default()
    };
    if after_tool(call) {
        let url = all
            .iter()
            .rev()
            .find_map(|message| message["tool_calls"][0]["function"]["arguments"].as_str())
            .and_then(|arguments| serde_json::from_str::<serde_json::Value>(arguments).ok())
            .and_then(|arguments| arguments["url"].as_str().map(str::to_owned))
            .unwrap_or_default();
        return words(&format!("Fetched {url}: {}", last_text(call)));
    }
    let text = last_text(call);
    let url = if text.contains("FETCH FIRST") {
        first_url("user")
    } else if text.contains("FETCH ANSWER") {
        first_url("assistant")
    } else if let Some((_, rest)) = text.split_once("FETCH+ ") {
        format!("{}x", rest.split_whitespace().next().unwrap_or_default())
    } else if let Some((_, rest)) = text.split_once("FETCH ") {
        rest.split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned()
    } else {
        return words("Nothing to fetch.");
    };
    vec![call_tool("web_fetch", serde_json::json!({ "url": url }))]
}

fn pensive(_: &ChatCall) -> Script {
    let pause = || Step::Wait(std::time::Duration::from_millis(50));
    let mut script = Vec::new();
    for piece in ["Let", " me", " think", "."] {
        script.extend([pause(), Step::Reasoning(piece.into())]);
    }
    for piece in ["Hello", " there", "!"] {
        script.extend([pause(), Step::Content(piece.into())]);
    }
    script
}

/// Réflexion qui boucle, par paragraphes entiers.
fn mull(_call: &ChatCall) -> Script {
    (0..80)
        .map(|_| Step::Reasoning(MULL_PARAGRAPH.to_owned()))
        .collect()
}
