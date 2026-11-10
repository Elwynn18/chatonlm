//! Scénarios écrits par les lots (contrat 102 § 1.5), en plus de ceux
//! intégrés (`scenario.rs`) : un modèle dont l'alias, puis le nom du
//! fichier, contient `name` répond par le script de `chat` (et de `infill`
//! s'il y en a un). Recherche : scénarios intégrés d'abord, puis [`ALL`] du
//! nom le plus long au plus court.
//!
//! Script ([`Step`]) : morceaux de réflexion et de réponse, appels d'outils
//! (un bloc chacun, `id` = `fake-call-<n>`), attentes ; [`Step::Fail`]
//! avant tout morceau : réponse HTTP de ce code et de ce corps, après :
//! bloc `data:` de ce corps puis fin du flux ; [`Step::Cut`] : connexion
//! coupée (le processus continue) ; [`Step::Finish`] : raison de fin (sinon
//! `tool_calls` s'il y a eu un appel, `stop` autrement). Les règles
//! communes du faux (contexte dépassé, image sans projecteur, clé, file des
//! emplacements) valent avant le script.

pub mod l5a;
pub mod l7;
pub mod l8;
pub mod l9;

use std::time::Duration;

use serde_json::Value;

/// Requête de chat reçue.
pub struct ChatCall<'a> {
    /// Corps JSON tel que reçu.
    pub body: &'a Value,
    /// Nom du modèle que rend le serveur : l'alias (`--alias`, l'identifiant
    /// chez l'app), sinon le chemin de son fichier (`-m`).
    pub model: &'a str,
}

/// Requête `/infill` reçue.
pub struct InfillCall<'a> {
    pub body: &'a Value,
    pub model: &'a str,
}

/// Étape d'un script.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Reasoning(String),
    Content(String),
    /// Appel d'outil ; `arguments` : texte JSON tel qu'envoyé (malformé
    /// permis).
    ToolCall {
        name: String,
        arguments: String,
    },
    Wait(Duration),
    Fail {
        status: u16,
        body: Value,
    },
    Cut,
    Finish(&'static str),
}

impl Step {
    /// Morceau produit (compté dans les jetons de la réponse).
    pub fn produces(&self) -> bool {
        matches!(
            self,
            Step::Reasoning(_) | Step::Content(_) | Step::ToolCall { .. }
        )
    }
}

pub type Script = Vec<Step>;

/// Borne un script comme le vrai serveur et comme les scénarios intégrés
/// (`scenario::limit`) : au plus `max_tokens` (sinon `n_predict`) morceaux de
/// réflexion et de réponse, et pas plus que la place `room` du contexte ;
/// limite atteinte : la suite est retirée et la fin devient `length`. Un
/// script avec un appel d'outil n'est pas borné (règle des intégrés).
pub fn limit(script: Script, request: &Value, room: usize) -> Script {
    if script
        .iter()
        .any(|step| matches!(step, Step::ToolCall { .. }))
    {
        return script;
    }
    let max = request["max_tokens"]
        .as_i64()
        .or(request["n_predict"].as_i64())
        .filter(|n| *n >= 0)
        .map_or(usize::MAX, |n| n as usize)
        .min(room);
    let produced = script.iter().filter(|step| step.produces()).count();
    if produced < max {
        return script;
    }
    let mut kept = Vec::new();
    let mut count = 0;
    for step in script {
        if step.produces() {
            if count == max {
                break;
            }
            count += 1;
        } else if matches!(step, Step::Finish(_)) {
            break;
        }
        kept.push(step);
    }
    kept.push(Step::Finish("length"));
    kept
}

/// Scénario d'un lot.
#[derive(Clone, Copy)]
pub struct Custom {
    /// `chatonlm-<nom>`.
    pub name: &'static str,
    pub chat: fn(&ChatCall) -> Script,
    /// `/infill` ; `None` : le texte sans valeur d'un modèle sans FIM.
    pub infill: Option<fn(&InfillCall) -> Script>,
    /// Durée du chargement (sinon celle du faux).
    pub load_ms: Option<u64>,
    /// Chargement raté (le processus s'arrête, code 1).
    pub fail_load: bool,
}

/// Scénarios de tous les lots.
pub static ALL: &[&[Custom]] = &[l5a::SCENARIOS, l7::SCENARIOS, l8::SCENARIOS, l9::SCENARIOS];

/// Scénario d'un lot d'après l'alias puis le nom du fichier (sans la
/// casse), du nom le plus long au plus court ; à n'appeler qu'après les
/// scénarios intégrés.
pub fn find(alias: Option<&str>, model: &str) -> Option<&'static Custom> {
    find_in(all(), alias, model)
}

#[cfg(not(test))]
fn all() -> &'static [&'static [Custom]] {
    ALL
}

/// Essais du faux : les lots, plus les scénarios de ses propres essais.
#[cfg(test)]
fn all() -> &'static [&'static [Custom]] {
    static WITH_TESTS: &[&[Custom]] = &[
        l5a::SCENARIOS,
        l7::SCENARIOS,
        l8::SCENARIOS,
        l9::SCENARIOS,
        tests::LOT,
    ];
    WITH_TESTS
}

fn find_in(
    all: &'static [&'static [Custom]],
    alias: Option<&str>,
    model: &str,
) -> Option<&'static Custom> {
    let mut customs: Vec<&'static Custom> = all.iter().flat_map(|lot| lot.iter()).collect();
    customs.sort_by_key(|custom| std::cmp::Reverse(custom.name.len()));
    let file = model.rsplit(['/', '\\']).next().unwrap_or(model);
    for text in alias.into_iter().chain([file]) {
        let text = text.to_ascii_lowercase();
        if let Some(custom) = customs
            .iter()
            .find(|custom| text.contains(&custom.name.to_ascii_lowercase()))
        {
            return Some(custom);
        }
    }
    None
}

/// Noms en défaut : en double dans les lots, ou contenant un nom intégré
/// (jamais atteints).
pub fn conflicts(all: &[&[Custom]], builtin: &[&str]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut conflicts = Vec::new();
    for custom in all.iter().flat_map(|lot| lot.iter()) {
        let name = custom.name.to_ascii_lowercase();
        if !seen.insert(name.clone()) {
            conflicts.push(format!("{} twice", custom.name));
        }
        if let Some(inner) = builtin.iter().find(|inner| name.contains(*inner)) {
            conflicts.push(format!("{} contains {inner}", custom.name));
        }
        if !name.starts_with("chatonlm-") {
            conflicts.push(format!("{} lacks chatonlm-", custom.name));
        }
    }
    conflicts
}

#[cfg(test)]
mod limit_tests {
    use serde_json::json;

    use super::*;

    fn text(n: usize) -> Script {
        (0..n).map(|i| Step::Content(format!("w{i} "))).collect()
    }

    /// `max_tokens` d'abord, puis `n_predict`, puis la place du contexte ;
    /// fin `length` à la limite ; un appel d'outil n'est jamais borné.
    #[test]
    fn scripts_stop_at_the_token_limit() {
        let limited = limit(text(5), &json!({ "max_tokens": 3 }), usize::MAX);
        assert_eq!(limited.iter().filter(|s| s.produces()).count(), 3);
        assert_eq!(limited.last(), Some(&Step::Finish("length")));
        let limited = limit(text(5), &json!({ "n_predict": 2 }), usize::MAX);
        assert_eq!(limited.iter().filter(|s| s.produces()).count(), 2);
        let limited = limit(
            text(5),
            &json!({ "max_tokens": 4, "n_predict": 1 }),
            usize::MAX,
        );
        assert_eq!(limited.iter().filter(|s| s.produces()).count(), 4);
        let limited = limit(text(5), &json!({}), 2);
        assert_eq!(limited.iter().filter(|s| s.produces()).count(), 2);
        assert_eq!(limited.last(), Some(&Step::Finish("length")));
        // Sous la limite, ou sans limite : inchangé.
        assert_eq!(
            limit(text(3), &json!({ "max_tokens": 4 }), usize::MAX),
            text(3)
        );
        assert_eq!(
            limit(text(3), &json!({ "max_tokens": -1 }), usize::MAX),
            text(3)
        );
        // Raison donnée par le script remplacée à la limite.
        let mut finished = text(4);
        finished.push(Step::Finish("stop"));
        let limited = limit(finished, &json!({ "max_tokens": 2 }), usize::MAX);
        assert_eq!(limited.last(), Some(&Step::Finish("length")));
        assert_eq!(limited.len(), 3);
        // Appel d'outil : pas de limite.
        let mut tools = text(5);
        tools.push(Step::ToolCall {
            name: "f".into(),
            arguments: "{}".into(),
        });
        assert_eq!(limit(tools.clone(), &json!({ "max_tokens": 1 }), 1), tools);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderMap, Method, header};
    use serde_json::json;

    use super::*;
    use crate::{Config, Shared, handle};

    /// Scénarios des essais du faux (jamais dans [`ALL`]).
    pub(super) static LOT: &[Custom] = &[
        Custom {
            name: "chatonlm-socle-script",
            chat: script,
            infill: Some(infill),
            load_ms: Some(10),
            fail_load: false,
        },
        Custom {
            name: "chatonlm-socle-refuse",
            chat: refuse,
            infill: None,
            load_ms: Some(10),
            fail_load: false,
        },
        Custom {
            name: "chatonlm-socle-cut",
            chat: cut,
            infill: None,
            load_ms: Some(10),
            fail_load: false,
        },
        Custom {
            name: "chatonlm-socle-noload",
            chat: script,
            infill: None,
            load_ms: Some(10),
            fail_load: true,
        },
    ];

    /// Réflexion, réponse qui répète le dernier message, appel d'outil.
    fn script(call: &ChatCall) -> Script {
        let last = call.body["messages"]
            .as_array()
            .and_then(|messages| messages.last())
            .and_then(|message| message["content"].as_str())
            .unwrap_or_default()
            .to_owned();
        vec![
            Step::Reasoning("hmm".into()),
            Step::Wait(Duration::from_millis(5)),
            Step::Content(format!("echo {last} from {}", call.model)),
            Step::ToolCall {
                name: "web_search".into(),
                arguments: r#"{"query":"x"}"#.into(),
            },
        ]
    }

    fn refuse(_: &ChatCall) -> Script {
        vec![
            Step::Wait(Duration::from_millis(5)),
            Step::Fail {
                status: 422,
                body: json!({ "detail": "extra fields not permitted" }),
            },
        ]
    }

    fn cut(_: &ChatCall) -> Script {
        vec![Step::Content("part".into()), Step::Cut]
    }

    fn infill(_: &InfillCall) -> Script {
        vec![Step::Content("a + b".into()), Step::Finish("limit")]
    }

    async fn ready(model: &str) -> Arc<Shared> {
        let shared = Shared::new(Config::new(model).api_key("k")).0;
        tokio::time::sleep(Duration::from_millis(30)).await;
        shared
    }

    async fn post(shared: &Arc<Shared>, path: &str, body: serde_json::Value) -> (u16, String) {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer k".parse().unwrap());
        let response = handle(
            State(shared.clone()),
            Method::POST,
            path.parse().unwrap(),
            headers,
            Bytes::from(body.to_string()),
        )
        .await;
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await;
        (
            status,
            bytes
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_else(|_| "<cut>".into()),
        )
    }

    fn ask(stream: bool) -> serde_json::Value {
        json!({ "messages": [{ "role": "user", "content": "ping" }], "stream": stream,
            "stream_options": { "include_usage": true } })
    }

    #[tokio::test]
    async fn a_script_is_played_in_a_stream_and_whole() {
        let shared = ready("/m/chatonlm-socle-script.gguf").await;
        assert!(shared.custom.is_some());
        let (status, text) = post(&shared, "/v1/chat/completions", ask(true)).await;
        assert_eq!(status, 200);
        let blocks: Vec<serde_json::Value> = text
            .split("\n\n")
            .filter_map(|block| block.strip_prefix("data: "))
            .filter(|data| *data != "[DONE]")
            .map(|data| serde_json::from_str(data).unwrap())
            .collect();
        assert_eq!(blocks[1]["choices"][0]["delta"]["reasoning_content"], "hmm");
        assert_eq!(
            blocks[2]["choices"][0]["delta"]["content"],
            "echo ping from /m/chatonlm-socle-script.gguf"
        );
        let call = &blocks[3]["choices"][0]["delta"]["tool_calls"][0];
        assert_eq!(call["id"], "fake-call-0");
        assert_eq!(call["function"]["name"], "web_search");
        assert_eq!(call["function"]["arguments"], r#"{"query":"x"}"#);
        assert_eq!(blocks[4]["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(blocks[5]["usage"]["completion_tokens"], 3);
        assert!(text.ends_with("data: [DONE]\n\n"));
        let (status, text) = post(&shared, "/v1/chat/completions", ask(false)).await;
        assert_eq!(status, 200);
        let whole: serde_json::Value = serde_json::from_str(&text).unwrap();
        let message = &whole["choices"][0]["message"];
        assert_eq!(message["reasoning_content"], "hmm");
        assert_eq!(message["tool_calls"][0]["function"]["name"], "web_search");
        assert_eq!(whole["choices"][0]["finish_reason"], "tool_calls");
        // `/infill` du script.
        let (status, text) = post(
            &shared,
            "/infill",
            json!({ "input_prefix": "def add(a, b):\n    return ", "input_suffix": "" }),
        )
        .await;
        assert_eq!(status, 200);
        let infill: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            (infill["content"].as_str(), infill["stop_type"].as_str()),
            (Some("a + b"), Some("limit"))
        );
    }

    #[tokio::test]
    async fn failures_and_cuts_are_played() {
        let refused = ready("chatonlm-socle-refuse.gguf").await;
        for stream in [true, false] {
            let (status, text) = post(&refused, "/v1/chat/completions", ask(stream)).await;
            assert_eq!(status, 422);
            assert!(text.contains("extra fields not permitted"), "{text}");
        }
        let cut = ready("chatonlm-socle-cut.gguf").await;
        let (status, text) = post(&cut, "/v1/chat/completions", ask(true)).await;
        assert_eq!(status, 200);
        assert_eq!(text, "<cut>");
        let (_, text) = post(&cut, "/v1/chat/completions", ask(false)).await;
        assert_eq!(text, "<cut>");
        // Un nom intégré passe avant : jamais un script.
        let builtin = ready("chatonlm-think-and-chatonlm-socle-script.gguf").await;
        assert!(builtin.custom.is_none());
        assert_eq!(builtin.scenario, crate::Scenario::Think);
        // Chargement raté : toujours en chargement (503), puis arrêt.
        let failing = ready("chatonlm-socle-noload.gguf").await;
        assert!(failing.loading());
    }

    fn hello(_: &ChatCall) -> Script {
        vec![Step::Content("hello".into())]
    }

    const fn custom(name: &'static str) -> Custom {
        Custom {
            name,
            chat: hello,
            infill: None,
            load_ms: None,
            fail_load: false,
        }
    }

    /// Les scénarios des lots : uniques et jamais cachés par un intégré.
    #[test]
    fn every_lot_scenario_has_its_own_name() {
        let conflicts = conflicts(ALL, &crate::scenario::builtin_names());
        assert!(conflicts.is_empty(), "{conflicts:?}");
    }

    #[test]
    fn conflicts_are_found() {
        static A: &[Custom] = &[custom("chatonlm-strict"), custom("chatonlm-thinker")];
        static B: &[Custom] = &[custom("chatonlm-strict"), custom("plain")];
        let found = conflicts(&[A, B], &crate::scenario::builtin_names());
        assert_eq!(
            found,
            [
                "chatonlm-thinker contains chatonlm-think",
                "chatonlm-strict twice",
                "plain lacks chatonlm-"
            ]
        );
    }

    #[test]
    fn the_longest_name_wins_alias_first() {
        static LOT: &[Custom] = &[custom("chatonlm-remote"), custom("chatonlm-remote-strict")];
        static ALL_TEST: &[&[Custom]] = &[LOT];
        let found = |alias, model| find_in(ALL_TEST, alias, model).map(|custom| custom.name);
        assert_eq!(
            found(None, "/m/CHATONLM-Remote-Strict.gguf"),
            Some("chatonlm-remote-strict")
        );
        assert_eq!(
            found(None, "/m/chatonlm-remote.gguf"),
            Some("chatonlm-remote")
        );
        assert_eq!(
            found(Some("chatonlm-remote"), "chatonlm-remote-strict.gguf"),
            Some("chatonlm-remote")
        );
        assert_eq!(found(None, "/chatonlm-remote/x.gguf"), None);
        assert_eq!(found(None, "x.gguf"), None);
    }
}
