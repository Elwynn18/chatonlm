//! Réglages repris d'Ollama pour un modèle (`store::model_import::defaults`,
//! lot L6) appliqués à une requête : règles pures (contrat 102 § 1.2).
//!
//! - Un champ déjà posé par l'appelant gagne.
//! - `stop` du modèle seulement si la requête n'en a pas.
//! - `num_predict` (> 0) devient `max_tokens` s'il est absent ; -1 (sans
//!   limite) et -2 (remplir le contexte) ne posent rien.
//! - La consigne `system` du modèle passe en tête SEULEMENT s'il n'y a aucun
//!   message `system` (règle d'Ollama : la consigne du Modelfile ne
//!   s'ajoute pas à celle de l'appelant).
//! - `num_ctx` ne touche pas la requête : il compte à l'ouverture
//!   ([`context`]).

use crate::inference::{ChatRequest, Message, Role};
use crate::store::model_import::ModelDefaults;

/// Applique `defaults` à `request` (voir l'en-tête du module).
pub fn apply(defaults: &ModelDefaults, request: &mut ChatRequest) {
    let params = &defaults.params;
    let float = |value: Option<f64>| value.filter(|v| v.is_finite()).map(|v| v as f32);
    fill(&mut request.temperature, float(params.temperature));
    fill(&mut request.top_p, float(params.top_p));
    fill(&mut request.min_p, float(params.min_p));
    fill(&mut request.repeat_penalty, float(params.repeat_penalty));
    fill(
        &mut request.presence_penalty,
        float(params.presence_penalty),
    );
    fill(
        &mut request.frequency_penalty,
        float(params.frequency_penalty),
    );
    fill(
        &mut request.top_k,
        params.top_k.and_then(|k| u32::try_from(k).ok()),
    );
    fill(
        &mut request.repeat_last_n,
        params.repeat_last_n.and_then(|n| i32::try_from(n).ok()),
    );
    // Graine négative : « au hasard » chez Ollama, rien d'envoyé.
    fill(
        &mut request.seed,
        params.seed.and_then(|seed| u32::try_from(seed).ok()),
    );
    fill(
        &mut request.max_tokens,
        params
            .num_predict
            .filter(|n| *n > 0)
            .and_then(|n| u32::try_from(n).ok()),
    );
    if request.stop.is_empty() {
        request.stop.clone_from(&params.stop);
    }
    let system = defaults.system.trim();
    if !system.is_empty()
        && !request
            .messages
            .iter()
            .any(|message| message.role == Role::System)
    {
        request
            .messages
            .insert(0, Message::system(defaults.system.clone()));
    }
}

fn fill<T>(field: &mut Option<T>, value: Option<T>) {
    if field.is_none() {
        *field = value;
    }
}

/// Contexte demandé à l'ouverture d'un modèle local, à défaut d'un contexte
/// imposé par l'appelant : le réglage `context_length` (> 0), puis le
/// `num_ctx` repris du modèle ; `None` : contexte du moteur (`context_for`).
pub fn context(setting: u32, defaults: Option<&ModelDefaults>) -> Option<u32> {
    if setting > 0 {
        return Some(setting);
    }
    defaults
        .and_then(|defaults| defaults.params.num_ctx)
        .filter(|ctx| *ctx > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::Content;
    use crate::local_models::import::OllamaParams;

    fn defaults() -> ModelDefaults {
        ModelDefaults {
            params: OllamaParams {
                temperature: Some(0.6),
                top_k: Some(20),
                top_p: Some(0.95),
                min_p: Some(0.05),
                repeat_penalty: Some(1.1),
                repeat_last_n: Some(64),
                presence_penalty: Some(1.5),
                frequency_penalty: Some(0.2),
                seed: Some(42),
                num_ctx: Some(8192),
                num_predict: Some(256),
                stop: vec!["<|im_end|>".into()],
            },
            system: "You are a cat.".into(),
        }
    }

    #[test]
    fn every_field_fills_an_empty_request() {
        let mut request = ChatRequest::new(vec![Message::user("hi")]);
        apply(&defaults(), &mut request);
        assert_eq!(request.temperature, Some(0.6));
        assert_eq!(request.top_k, Some(20));
        assert_eq!(request.top_p, Some(0.95));
        assert_eq!(request.min_p, Some(0.05));
        assert_eq!(request.repeat_penalty, Some(1.1));
        assert_eq!(request.repeat_last_n, Some(64));
        assert_eq!(request.presence_penalty, Some(1.5));
        assert_eq!(request.frequency_penalty, Some(0.2));
        assert_eq!(request.seed, Some(42));
        assert_eq!(request.max_tokens, Some(256));
        assert_eq!(request.stop, ["<|im_end|>"]);
        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[0].role, Role::System);
        assert_eq!(
            request.messages[0].content,
            Content::Text("You are a cat.".into())
        );
        assert_eq!(request.messages[1].role, Role::User);
        // Les champs propres à llama.cpp sont dans le corps envoyé.
        let body = request.body(true);
        assert_eq!(body["presence_penalty"], 1.5);
        assert_eq!(
            body["frequency_penalty"].as_f64().map(|v| v as f32),
            Some(0.2)
        );
        assert_eq!(body["repeat_last_n"], 64);
    }

    #[test]
    fn the_caller_wins_field_by_field() {
        let mut request = ChatRequest {
            temperature: Some(0.3),
            max_tokens: Some(10),
            seed: Some(7),
            stop: vec!["END".into()],
            ..ChatRequest::new(vec![Message::system("Be brief."), Message::user("hi")])
        };
        apply(&defaults(), &mut request);
        assert_eq!(request.temperature, Some(0.3));
        assert_eq!(request.max_tokens, Some(10));
        assert_eq!(request.seed, Some(7));
        // `stop` du modèle seulement si la requête n'en a pas.
        assert_eq!(request.stop, ["END"]);
        // Une consigne de l'appelant : celle du modèle ne s'ajoute pas.
        assert_eq!(request.messages.len(), 2);
        assert_eq!(
            request.messages[0].content,
            Content::Text("Be brief.".into())
        );
        // Les autres champs restent remplis.
        assert_eq!(request.top_k, Some(20));
    }

    #[test]
    fn odd_values_send_nothing() {
        let mut odd = defaults();
        odd.params.num_predict = Some(-1);
        odd.params.seed = Some(-1);
        odd.params.top_k = Some(-5);
        odd.params.temperature = Some(f64::NAN);
        odd.system = "   ".into();
        let mut request = ChatRequest::new(vec![Message::user("hi")]);
        apply(&odd, &mut request);
        assert_eq!(request.max_tokens, None);
        assert_eq!(request.seed, None);
        assert_eq!(request.top_k, None);
        assert_eq!(request.temperature, None);
        assert_eq!(request.messages.len(), 1);
        odd.params.num_predict = Some(-2);
        let mut request = ChatRequest::new(vec![Message::user("hi")]);
        apply(&odd, &mut request);
        assert_eq!(request.max_tokens, None);
        // Rien de repris : la requête reste telle quelle.
        let mut plain = ChatRequest::new(vec![Message::user("hi")]);
        apply(&ModelDefaults::default(), &mut plain);
        assert_eq!(plain, ChatRequest::new(vec![Message::user("hi")]));
    }

    #[test]
    fn the_context_setting_comes_before_the_model() {
        assert_eq!(context(4096, Some(&defaults())), Some(4096));
        assert_eq!(context(0, Some(&defaults())), Some(8192));
        assert_eq!(context(0, None), None);
        let mut none = defaults();
        none.params.num_ctx = Some(0);
        assert_eq!(context(0, Some(&none)), None);
    }
}
