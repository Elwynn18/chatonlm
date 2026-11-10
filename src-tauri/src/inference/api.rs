//! Appels sans flux : réponse entière, autocomplétion, santé, propriétés,
//! jetons.

use std::time::Duration;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::stream::new_call_id;
use super::types::{
    ChatRequest, ChatResponse, FinishReason, Health, InfillRequest, InfillResponse, InfillStop,
    Props, Timings, ToolCall, Usage,
};
use super::{Endpoint, InferenceError, MAX_BODY, read_bytes, read_json};

/// Délai de `/health` (sonde répétée par le moteur pendant le chargement).
const HEALTH_TIMEOUT: Duration = Duration::from_secs(if cfg!(windows) { 3 } else { 2 });
/// Délai de `/props` et `/tokenize` (réponses immédiates du serveur).
const INFO_TIMEOUT: Duration = Duration::from_secs(10);
/// Corps de `/health` : `{"status":"ok"}`.
const MAX_HEALTH_BODY: usize = 64 * 1024;

/// Envoie la requête et rend le corps JSON d'une réponse réussie. Sans
/// délai total si `timeout` est `None` (une réponse entière peut prendre des
/// minutes) : l'appelant annule par `cancel`.
async fn send(
    request: reqwest::RequestBuilder,
    timeout: Option<Duration>,
    cancel: &CancellationToken,
) -> Result<Value, InferenceError> {
    let request = match timeout {
        Some(timeout) => request.timeout(timeout),
        None => request,
    };
    let response = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(InferenceError::Cancelled),
        sent = request.send() => sent.map_err(|error| InferenceError::from_send(&error))?,
    };
    let status = response.status();
    if !status.is_success() {
        let body = read_json(response, cancel).await?;
        return Err(InferenceError::from_response(status.as_u16(), &body));
    }
    let bytes = read_bytes(response, MAX_BODY, cancel).await?;
    serde_json::from_slice(&bytes)
        .map_err(|_| InferenceError::Malformed("the reply is not JSON".into()))
}

/// Envoie la conversation et rend la réponse entière (`stream: false`).
pub async fn chat_once(
    endpoint: &Endpoint,
    request: &ChatRequest,
    cancel: CancellationToken,
) -> Result<ChatResponse, InferenceError> {
    let body = send(
        endpoint
            .post("/v1/chat/completions")
            .json(&request.body_for(endpoint.dialect, false)),
        None,
        &cancel,
    )
    .await?;
    parse_chat(&body)
}

fn parse_chat(body: &Value) -> Result<ChatResponse, InferenceError> {
    let choice = &body["choices"][0];
    if !choice.is_object() {
        return Err(InferenceError::Malformed("no choice in the reply".into()));
    }
    let message = &choice["message"];
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let tool_calls = message["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|call| ToolCall {
            id: call["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map_or_else(new_call_id, str::to_owned),
            name: text(&call["function"]["name"]),
            // Objet chez d'autres serveurs OpenAI, texte chez llama-server.
            arguments: match &call["function"]["arguments"] {
                Value::String(arguments) => arguments.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            },
        })
        .collect();
    Ok(ChatResponse {
        content: text(&message["content"]),
        reasoning: text(&message["reasoning_content"]),
        tool_calls,
        finish_reason: FinishReason::parse(choice["finish_reason"].as_str().unwrap_or_default()),
        usage: Usage::parse(&body["usage"]),
        timings: Timings::parse(&body["timings"]),
    })
}

/// Autocomplétion au curseur (`/infill`).
pub async fn infill(
    endpoint: &Endpoint,
    request: &InfillRequest,
    cancel: CancellationToken,
) -> Result<InfillResponse, InferenceError> {
    let body = send(
        endpoint.post("/infill").json(&request.body()),
        None,
        &cancel,
    )
    .await?;
    let content = body["content"]
        .as_str()
        .ok_or_else(|| InferenceError::Malformed("no content in the reply".into()))?;
    let number = |value: &Value| {
        value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0)
    };
    Ok(InfillResponse {
        content: content.to_owned(),
        stop: match body["stop_type"].as_str() {
            Some("eos") => InfillStop::Eos,
            Some("word") => InfillStop::Word,
            Some("limit") => InfillStop::Limit,
            _ => InfillStop::Other,
        },
        tokens_evaluated: number(&body["tokens_evaluated"]),
        tokens_predicted: number(&body["tokens_predicted"]),
        timings: Timings::parse(&body["timings"]),
    })
}

/// État du serveur (`/health`, public : sans clé).
pub async fn health(endpoint: &Endpoint) -> Health {
    let sent = endpoint
        .client
        .get(endpoint.url("/health"))
        .timeout(HEALTH_TIMEOUT)
        .send()
        .await;
    let response = match sent {
        Ok(response) => response,
        Err(_) => return Health::Unreachable,
    };
    let status = response.status().as_u16();
    let never = CancellationToken::new();
    let body = match read_bytes(response, MAX_HEALTH_BODY, &never).await {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        Err(InferenceError::Interrupted(_) | InferenceError::Timeout) => {
            return Health::Unreachable;
        }
        Err(error) => return Health::Unexpected(error),
    };
    match status {
        200 if body == json!({ "status": "ok" }) => Health::Ready,
        200 => Health::Unexpected(InferenceError::Malformed(
            "not the health of llama-server".into(),
        )),
        503 => Health::Loading,
        _ => Health::Unexpected(InferenceError::from_response(status, &body)),
    }
}

/// Propriétés du serveur et du modèle chargé (`/props`).
pub async fn props(endpoint: &Endpoint) -> Result<Props, InferenceError> {
    let never = CancellationToken::new();
    let body = send(endpoint.get("/props"), Some(INFO_TIMEOUT), &never).await?;
    Props::parse(&body)
        .ok_or_else(|| InferenceError::Malformed("no context size in the properties".into()))
}

/// Jetons du texte, comptés par le modèle chargé (`/tokenize`, sans jetons
/// spéciaux). Attention : contrairement à `/props` et `/health`, `/tokenize`
/// n'est pas exempté du sommeil (b12110, server-context.cpp:6403) : sur un
/// modèle endormi (`--sleep-idle-seconds`), il le **recharge** en mémoire, et
/// finit en `Timeout` au bout de 10 s si le rechargement est plus long.
/// L'appelant consulte d'abord [`props`] (`is_sleeping`) s'il ne veut pas le
/// réveiller.
pub async fn tokenize(endpoint: &Endpoint, text: &str) -> Result<Vec<u32>, InferenceError> {
    let never = CancellationToken::new();
    let request = endpoint.post("/tokenize").json(&json!({ "content": text }));
    let body = send(request, Some(INFO_TIMEOUT), &never).await?;
    body["tokens"]
        .as_array()
        .ok_or_else(|| InferenceError::Malformed("no tokens in the reply".into()))?
        .iter()
        .map(|token| {
            token
                .as_u64()
                .and_then(|id| u32::try_from(id).ok())
                .ok_or_else(|| InferenceError::Malformed("bad token".into()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_non_streamed_reply_is_read() {
        // Réponse relevée sur llama-server b12110 (appel d'outil).
        let real = json!({ "choices": [{ "finish_reason": "tool_calls", "index": 0, "message": { "role": "assistant", "content": "",
            "tool_calls": [{ "type": "function", "function": { "name": "get_weather", "arguments": "{\"city\": \"Paris\"}" }, "id": "3rjJ" }] } }],
            "object": "chat.completion",
            "usage": { "completion_tokens": 20, "prompt_tokens": 160, "total_tokens": 180, "prompt_tokens_details": { "cached_tokens": 159 } },
            "timings": { "cache_n": 159, "prompt_n": 1, "prompt_ms": 15.1, "predicted_n": 20, "predicted_ms": 261.3 } });
        let reply = parse_chat(&real).unwrap();
        assert_eq!(reply.finish_reason, FinishReason::ToolCalls);
        assert_eq!(
            reply.tool_calls,
            [ToolCall {
                id: "3rjJ".into(),
                name: "get_weather".into(),
                arguments: "{\"city\": \"Paris\"}".into()
            }]
        );
        assert_eq!(
            reply.usage,
            Some(Usage {
                prompt: 160,
                completion: 20,
                cached: 159
            })
        );
        assert_eq!(reply.timings.map(|t| t.cache_n), Some(159));
        // Arguments en objet (autre serveur), id absent.
        let other = json!({ "choices": [{ "finish_reason": "stop", "message": { "content": null, "reasoning_content": "hm",
            "tool_calls": [{ "function": { "name": "f", "arguments": { "a": 1 } } }] } }] });
        let reply = parse_chat(&other).unwrap();
        assert_eq!(reply.tool_calls[0].arguments, "{\"a\":1}");
        assert!(reply.tool_calls[0].id.starts_with("call_"));
        assert_eq!(
            (reply.content.as_str(), reply.reasoning.as_str()),
            ("", "hm")
        );
        assert!(matches!(
            parse_chat(&json!({ "choices": [] })),
            Err(InferenceError::Malformed(_))
        ));
    }
}
