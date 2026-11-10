//! Corps d'une requête selon la langue du serveur ([`Dialect`]) : tout pour
//! `llama-server` ; pour un fournisseur distant (modèles distants, clé de
//! l'utilisateur), seuls les champs de l'API d'OpenAI, et ceux de Mistral
//! pour Mistral, qui refuse tout champ inconnu (422 « extra_forbidden »).
//!
//! Retirés pour un distant : `chat_template_kwargs`, `reasoning_effort` (un
//! modèle sans réflexion le refuse chez OpenAI), `top_k`, `min_p`,
//! `repeat_penalty`, `repeat_last_n`, `cache_prompt` (propres à llama.cpp),
//! et dans les messages `reasoning_content` et `name`. Mistral : pas de
//! `stream_options` (l'usage vient toujours en fin de flux), `seed` devient
//! `random_seed`, et les identifiants des appels d'outils prennent sa forme
//! (neuf lettres ou chiffres) : un appel fait par un modèle local plus tôt
//! dans la conversation serait refusé sinon.

use serde_json::{Map, Value};

use super::Dialect;

/// Champs du corps compris par l'API d'OpenAI (OpenRouter, Ollama, serveurs
/// compatibles).
const OPENAI_FIELDS: &[&str] = &[
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

/// Champs du corps compris par Mistral.
const MISTRAL_FIELDS: &[&str] = &[
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

/// Champs d'un message, pour tout fournisseur distant.
const MESSAGE_FIELDS: &[&str] = &["role", "content", "tool_calls", "tool_call_id"];

/// Adapte `body` (corps complet de `/v1/chat/completions`) à `dialect`.
pub(super) fn adapt(dialect: Dialect, body: &mut Value) {
    let fields = match dialect {
        Dialect::LlamaServer => return,
        Dialect::OpenAi => OPENAI_FIELDS,
        Dialect::Mistral => MISTRAL_FIELDS,
    };
    let Some(object) = body.as_object_mut() else {
        return;
    };
    if dialect == Dialect::Mistral
        && let Some(seed) = object.shift_remove("seed")
    {
        object.insert("random_seed".into(), seed);
    }
    object.retain(|key, _| fields.contains(&key.as_str()));
    let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages.iter_mut().filter_map(Value::as_object_mut) {
        message.retain(|key, _| MESSAGE_FIELDS.contains(&key.as_str()));
        if dialect == Dialect::Mistral {
            mistral_ids(message);
        }
    }
}

/// Identifiants des appels d'outils sous la forme de Mistral.
fn mistral_ids(message: &mut Map<String, Value>) {
    if let Some(Value::String(id)) = message.get_mut("tool_call_id") {
        *id = mistral_id(id);
    }
    for call in message
        .get_mut("tool_calls")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(Value::String(id)) = call.get_mut("id") {
            *id = mistral_id(id);
        }
    }
}

/// Identifiant d'appel accepté par Mistral (`[a-zA-Z0-9]{9}`) : celui donné
/// s'il l'est déjà (appel fait par Mistral), sinon une empreinte stable du
/// texte (même appel et même réponse : même identifiant).
pub(super) fn mistral_id(id: &str) -> String {
    if id.len() == 9 && id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return id.to_owned();
    }
    // FNV-1a 64 bits, écrit en base 62 sur neuf caractères.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    (0..9)
        .map(|_| {
            let digit = DIGITS[(hash % 62) as usize] as char;
            hash /= 62;
            digit
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::inference::{ChatRequest, Effort, Message, ResponseFormat, Think, ToolCall};

    /// Requête où chaque réglage est posé, appel d'outil et réflexion compris.
    fn everything() -> ChatRequest {
        let mut assistant = Message::assistant("");
        assistant.reasoning_content = Some("thinking".into());
        assistant.tool_calls = vec![ToolCall {
            id: "call_0".into(),
            name: "web_search".into(),
            arguments: "{}".into(),
        }];
        ChatRequest {
            model: Some("mistral-small-latest".into()),
            messages: vec![
                Message::user("hi"),
                assistant,
                Message::tool("call_0", "web_search", "{}"),
            ],
            tools: vec![json!({ "type": "function", "function": { "name": "web_search" } })],
            think: Think::Effort(Effort::High),
            response_format: Some(ResponseFormat::JsonObject),
            max_tokens: Some(8),
            temperature: Some(0.5),
            top_p: Some(0.5),
            top_k: Some(20),
            min_p: Some(0.125),
            repeat_penalty: Some(1.5),
            presence_penalty: Some(0.25),
            frequency_penalty: Some(0.25),
            repeat_last_n: Some(64),
            seed: Some(7),
            stop: vec!["\n".into()],
            cache_prompt: Some(true),
        }
    }

    fn keys(value: &Value) -> Vec<&str> {
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn llama_server_gets_everything() {
        let request = everything();
        assert_eq!(
            request.body_for(Dialect::LlamaServer, true),
            request.body(true)
        );
        let body = request.body(true);
        for key in ["top_k", "cache_prompt", "reasoning_effort", "repeat_last_n"] {
            assert!(body.get(key).is_some(), "{key}");
        }
    }

    #[test]
    fn a_remote_provider_gets_no_llama_field() {
        let request = everything();
        let body = request.body_for(Dialect::OpenAi, true);
        assert_eq!(
            keys(&body),
            [
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
            ]
        );
        for message in body["messages"].as_array().unwrap() {
            for key in keys(message) {
                assert!(MESSAGE_FIELDS.contains(&key), "{key}");
            }
        }
        // Les identifiants restent tels quels hors de Mistral.
        assert_eq!(body["messages"][1]["tool_calls"][0]["id"], "call_0");
        assert_eq!(body["messages"][2]["tool_call_id"], "call_0");
        // Réflexion coupée ou demandée : rien d'envoyé.
        let mut off = everything();
        off.think = Think::Off;
        assert!(
            off.body_for(Dialect::OpenAi, false)
                .get("chat_template_kwargs")
                .is_none()
        );
    }

    #[test]
    fn mistral_gets_its_own_names_and_ids() {
        let body = everything().body_for(Dialect::Mistral, true);
        assert_eq!(
            keys(&body),
            [
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
                "stop",
                "random_seed",
            ]
        );
        assert_eq!(body["random_seed"], 7);
        let call = body["messages"][1]["tool_calls"][0]["id"].as_str().unwrap();
        assert_eq!(call.len(), 9);
        assert!(call.bytes().all(|b| b.is_ascii_alphanumeric()));
        // L'appel et sa réponse gardent le même identifiant.
        assert_eq!(body["messages"][2]["tool_call_id"], call);
        assert_eq!(mistral_id("aB3dE6gH9"), "aB3dE6gH9");
        assert_ne!(mistral_id("call_0"), mistral_id("call_1"));
    }
}
