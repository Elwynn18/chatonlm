//! Autocomplétion de ChatonLM (méthodes `complete.fim` et
//! `completion.measure` du pont VS Code) : `/api/generate` avec un prompt,
//! pour les modèles `chatonlm-fim*` et voisins, comme Ollama 0.40 :
//! - `chatonlm-fim` : capacité `insert` (`suffix` accepté) ; flux de petits
//!   morceaux toutes les 10 ms ;
//! - `chatonlm-fim-slow` : comme lui, mais la réponse (en-têtes compris,
//!   comme Ollama qui ne les écrit qu'avec le premier jeton) n'arrive
//!   qu'après [`FIM_SLOW_FIRST`] (délai de garde de l'app) ;
//! - `chatonlm-fim-long` : comme lui, un morceau toutes les
//!   [`FIM_LONG_STEP`] (le temps d'annuler) ;
//! - `chatonlm-qwen-coder-raw` : sans `insert` (un `suffix` est refusé, 400
//!   comme Ollama) ; avec `raw: true` et les jetons FIM de Qwen-Coder, il
//!   répond, sinon il dit que le prompt n'est pas formaté ;
//! - `chatonlm-qwen-coder-fim` : avec `insert`, et les jetons de Qwen-Coder
//!   en `raw` (famille lue dans le nom par l'app) ;
//! - `chatonlm-nofim` : sans `insert`, d'aucune famille connue.
//!
//! Proposition : `value(<prefix>|<suffix>)` (longueurs en caractères ; `raw`
//! à la place du mode), puis `\n    second line`, une ligne vide et
//! `after blank` : les arrêts (`options.stop`) et `num_predict` (un jeton =
//! un morceau de 4 caractères) coupent comme chez Ollama. La réponse finale
//! donne des durées et le cache de préfixe simulés (`prompt_eval_cached_count`
//! : début commun avec le prompt précédent du même modèle, en jetons de 4
//! caractères). Un modèle pas en mémoire, ou en mémoire avec un autre
//! contexte, est (re)chargé comme Ollama et journalisé (`kind: "byPrompt"`
//! dans `/fake/loads`) : l'app ne doit jamais le demander ainsi.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};

use super::{End, Journal, json as json_response, ndjson};

/// Modèles qui annoncent `insert`.
pub const INSERT_MODELS: &[&str] = &[
    "chatonlm-fim",
    "chatonlm-fim-slow",
    "chatonlm-fim-long",
    "chatonlm-qwen-coder-fim",
];
/// Modèles de complétion sans `insert`.
pub const RAW_MODELS: &[&str] = &["chatonlm-qwen-coder-raw", "chatonlm-nofim"];

/// Premier morceau de `chatonlm-fim-slow`.
pub const FIM_SLOW_FIRST: Duration = Duration::from_millis(2500);
/// Pas de `chatonlm-fim-long`.
pub const FIM_LONG_STEP: Duration = Duration::from_millis(400);
/// Mémoire d'un modèle d'autocomplétion chargé (`/api/ps`) : 512 Mio.
pub const FIM_SIZE: u64 = 512 * 1024 * 1024;

pub fn is_fim_model(model: &str) -> bool {
    INSERT_MODELS.contains(&model) || RAW_MODELS.contains(&model)
}

/// Texte complet que le modèle écrirait, avant arrêts et limite.
fn proposal(model: &str, request: &Value) -> String {
    let prompt = request["prompt"].as_str().unwrap_or_default();
    let head = if request["raw"] == true {
        let formatted = prompt.starts_with("<|fim_prefix|>")
            && prompt.contains("<|fim_suffix|>")
            && prompt.ends_with("<|fim_middle|>");
        if formatted && model.contains("qwen-coder") {
            let inner = &prompt["<|fim_prefix|>".len()..prompt.len() - "<|fim_middle|>".len()];
            let (prefix, suffix) = inner.split_once("<|fim_suffix|>").unwrap_or((inner, ""));
            format!(
                "value(raw {}|{})",
                prefix.chars().count(),
                suffix.chars().count()
            )
        } else {
            "value(raw unformatted)".to_owned()
        }
    } else {
        let suffix = request["suffix"].as_str().unwrap_or_default();
        format!(
            "value({}|{})",
            prompt.chars().count(),
            suffix.chars().count()
        )
    };
    format!("{head}\n    second line\n\nafter blank")
}

/// Coupe comme Ollama : au premier arrêt (exclu), puis à `num_predict`
/// jetons ; rend le texte et la raison de la fin.
fn limited(text: &str, request: &Value) -> (String, &'static str) {
    let mut end = text.len();
    let mut reason = "stop";
    for stop in request["options"]["stop"].as_array().into_iter().flatten() {
        if let Some(stop) = stop.as_str().filter(|stop| !stop.is_empty())
            && let Some(at) = text.find(stop)
        {
            end = end.min(at);
        }
    }
    let mut kept: String = text[..end].to_owned();
    if let Some(limit) = request["options"]["num_predict"].as_u64() {
        let chars: Vec<char> = kept.chars().collect();
        let limit = usize::try_from(limit)
            .unwrap_or(usize::MAX)
            .saturating_mul(4);
        if chars.len() > limit {
            kept = chars[..limit].iter().collect();
            reason = "length";
        }
    }
    (kept, reason)
}

fn chunks_of(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(4)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

/// `/api/generate` avec un prompt.
pub fn generate(journal: Arc<Journal>, model: &str, request: &Value) -> Response {
    let suffix = request["suffix"].as_str().is_some_and(|s| !s.is_empty());
    if suffix && !INSERT_MODELS.contains(&model) {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": format!("registry.ollama.ai/library/{model}:latest does not support insert") })
                .to_string(),
        );
    }
    let prompt = request["prompt"].as_str().unwrap_or_default().to_owned();
    let cached = journal.cached_prefix(model, &prompt) / 4;
    let (text, reason) = limited(&proposal(model, request), request);
    let tokens = chunks_of(&text);
    let prompt_tokens = (prompt.chars().count() / 4).max(1) as u64;
    let eval_count = tokens.len() as u64;
    let uncached = prompt_tokens.saturating_sub(cached as u64);
    let done = |model: &str| {
        json!({
            "model": model,
            "response": "",
            "done": true,
            "done_reason": reason,
            "total_duration": 1_000_000 + uncached * 1_000_000 + eval_count * 5_000_000,
            "load_duration": 500_000,
            "prompt_eval_count": prompt_tokens,
            "prompt_eval_cached_count": cached,
            "prompt_eval_duration": uncached * 1_000_000,
            "eval_count": eval_count,
            "eval_duration": eval_count * 5_000_000,
        })
    };
    if request["stream"] == false {
        let mut body = done(model);
        body["response"] = json!(text);
        return json_response(StatusCode::OK, body.to_string());
    }
    let mut lines = Vec::new();
    for token in &tokens {
        lines.push(format!(
            "{}\n",
            json!({ "model": model, "response": token, "done": false })
        ));
    }
    lines.push(format!("{}\n", done(model)));
    let step = if model == "chatonlm-fim-long" {
        FIM_LONG_STEP
    } else {
        Duration::from_millis(10)
    };
    ndjson(journal, lines, step, End::Close)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_and_limits_cut_like_ollama() {
        let text = "value(1|2)\n    second line\n\nafter blank";
        let request = json!({ "options": { "stop": ["\n\n"] } });
        assert_eq!(
            limited(text, &request),
            ("value(1|2)\n    second line".to_owned(), "stop")
        );
        let request = json!({ "options": { "stop": ["\n"], "num_predict": 2 } });
        assert_eq!(limited(text, &request), ("value(1|".to_owned(), "length"));
    }

    #[test]
    fn a_raw_prompt_must_carry_the_family_tokens() {
        let raw = |prompt: &str| {
            proposal(
                "chatonlm-qwen-coder-raw",
                &json!({ "prompt": prompt, "raw": true }),
            )
        };
        assert!(raw("<|fim_prefix|>ab<|fim_suffix|>c<|fim_middle|>").starts_with("value(raw 2|1)"));
        assert!(raw("ab").starts_with("value(raw unformatted)"));
        let insert = proposal("chatonlm-fim", &json!({ "prompt": "abc", "suffix": "de" }));
        assert!(insert.starts_with("value(3|2)\n"));
    }
}
