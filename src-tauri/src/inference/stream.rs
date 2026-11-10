//! Réponse en flux de `/v1/chat/completions` (SSE) : blocs lus, fragments
//! des appels d'outils recollés, événements rendus à l'appelant.

use std::collections::{BTreeMap, VecDeque};
use std::pin::Pin;

use futures_util::Stream;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::sse::{SseItem, SseParser};
use super::types::{ChatRequest, FinishReason, Timings, ToolCall, Usage};
use super::{Endpoint, InferenceError, MAX_BODY, MAX_EVENT, MAX_TOOL_CALLS};

/// Ce que le flux apporte.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Morceau de la réponse.
    Content(String),
    /// Morceau de la réflexion (`reasoning_content`).
    Reasoning(String),
    /// Appels d'outils complets (fragments recollés, `id` donné s'il
    /// manquait), rendus une fois, à la fin de la réponse.
    ToolCalls(Vec<ToolCall>),
    /// Fin de la réponse : dernier événement du flux.
    Done {
        finish_reason: FinishReason,
        usage: Option<Usage>,
        timings: Option<Timings>,
    },
}

/// Flux des événements ; une erreur ou `Done` le termine.
pub type ChatStream = Pin<Box<dyn Stream<Item = Result<Event, InferenceError>> + Send>>;

/// Identifiant donné à un appel d'outil qui n'en a pas.
pub(super) fn new_call_id() -> String {
    format!("call_{}", uuid::Uuid::now_v7().simple())
}

#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

/// Assemble les blocs JSON du flux en événements.
#[derive(Default)]
pub(super) struct Assembler {
    calls: BTreeMap<u64, PartialCall>,
    calls_sent: bool,
    finish: Option<FinishReason>,
    usage: Option<Usage>,
    timings: Option<Timings>,
    /// Taille des noms, `id` et arguments reçus des appels (bornée).
    call_bytes: usize,
}

impl Assembler {
    /// Événements d'un bloc `data:`.
    pub(super) fn chunk(&mut self, chunk: &Value) -> Result<Vec<Event>, InferenceError> {
        if !chunk["error"].is_null() {
            let status = chunk["error"]["code"]
                .as_u64()
                .and_then(|code| u16::try_from(code).ok())
                .unwrap_or(500);
            return Err(InferenceError::from_response(status, chunk));
        }
        if let Some(usage) = Usage::parse(&chunk["usage"]) {
            self.usage = Some(usage);
        }
        if let Some(timings) = Timings::parse(&chunk["timings"]) {
            self.timings = Some(timings);
        }
        let mut events = Vec::new();
        // Une seule réponse demandée : le premier choix.
        let choice = &chunk["choices"][0];
        let delta = &choice["delta"];
        if let Some(text) = delta["reasoning_content"]
            .as_str()
            .filter(|text| !text.is_empty())
        {
            events.push(Event::Reasoning(text.to_owned()));
        }
        if let Some(text) = delta["content"].as_str().filter(|text| !text.is_empty()) {
            events.push(Event::Content(text.to_owned()));
        }
        for (position, fragment) in delta["tool_calls"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let index = fragment["index"].as_u64().unwrap_or(position as u64);
            if !self.calls.contains_key(&index) && self.calls.len() >= MAX_TOOL_CALLS {
                return Err(InferenceError::Malformed("too many tool calls".into()));
            }
            let id = fragment["id"].as_str().filter(|id| !id.is_empty());
            let name = fragment["function"]["name"].as_str();
            let arguments = fragment["function"]["arguments"].as_str();
            self.call_bytes += [id, name, arguments]
                .into_iter()
                .flatten()
                .map(str::len)
                .sum::<usize>();
            if self.call_bytes > MAX_BODY {
                return Err(InferenceError::Malformed("tool calls too large".into()));
            }
            let call = self.calls.entry(index).or_default();
            if let Some(id) = id {
                call.id = id.to_owned();
            }
            // Nom donné d'un bloc (llama-server) ou en morceaux : un morceau
            // qui répète le nom entier n'est pas recollé.
            if let Some(name) = name
                && name != call.name
            {
                call.name.push_str(name);
            }
            if let Some(arguments) = arguments {
                call.arguments.push_str(arguments);
            }
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish = Some(FinishReason::parse(reason));
            events.extend(self.tool_calls());
        }
        Ok(events)
    }

    /// Appels recollés, une seule fois.
    fn tool_calls(&mut self) -> Option<Event> {
        if self.calls_sent || self.calls.is_empty() {
            return None;
        }
        self.calls_sent = true;
        let calls = std::mem::take(&mut self.calls)
            .into_values()
            .map(|call| ToolCall {
                id: if call.id.is_empty() {
                    new_call_id()
                } else {
                    call.id
                },
                name: call.name,
                arguments: call.arguments,
            })
            .collect();
        Some(Event::ToolCalls(calls))
    }

    /// Fin vue par le serveur (raison de fin reçue).
    pub(super) fn finished(&self) -> bool {
        self.finish.is_some()
    }

    /// `data: [DONE]` (ou fin du corps après la raison de fin). Sans raison
    /// de fin, la réponse est incomplète : b12110 envoie toujours la raison
    /// (résultat final) avant `[DONE]`, un `[DONE]` seul n'est pas un succès.
    pub(super) fn done(&mut self) -> Result<Vec<Event>, InferenceError> {
        let Some(finish_reason) = self.finish.clone() else {
            return Err(InferenceError::Interrupted(
                "the stream ended without a finish reason".into(),
            ));
        };
        let mut events: Vec<Event> = self.tool_calls().into_iter().collect();
        events.push(Event::Done {
            finish_reason,
            usage: self.usage,
            timings: self.timings,
        });
        Ok(events)
    }
}

/// État de la lecture du flux.
struct Reader {
    response: Option<reqwest::Response>,
    parser: SseParser,
    assembler: Assembler,
    pending: VecDeque<Result<Event, InferenceError>>,
    cancel: CancellationToken,
    idle: std::time::Duration,
    over: bool,
}

impl Reader {
    fn take(&mut self, SseItem::Data(data): SseItem) {
        let events = if data == "[DONE]" {
            self.assembler.done()
        } else {
            match serde_json::from_str::<Value>(&data) {
                Ok(chunk) => self.assembler.chunk(&chunk),
                Err(_) => Err(InferenceError::Malformed("event is not JSON".into())),
            }
        };
        match events {
            Ok(events) => self.pending.extend(events.into_iter().map(Ok)),
            Err(error) => self.pending.push_back(Err(error)),
        }
    }

    /// Fin du corps.
    fn end(&mut self) {
        match self.parser.finish() {
            Ok(items) => items.into_iter().for_each(|item| self.take(item)),
            Err(error) => self.pending.push_back(Err(error)),
        }
        let closed = self
            .pending
            .iter()
            .any(|item| matches!(item, Ok(Event::Done { .. }) | Err(_)));
        if !closed {
            // Raison de fin reçue, corps fini sans `[DONE]` : réponse entière.
            match self.assembler.done() {
                Ok(events) => self.pending.extend(events.into_iter().map(Ok)),
                Err(_) => self.pending.push_back(Err(InferenceError::Interrupted(
                    "the stream ended before the reply".into(),
                ))),
            }
        }
    }

    async fn next(mut self) -> Option<(Result<Event, InferenceError>, Reader)> {
        loop {
            if let Some(item) = self.pending.pop_front() {
                if matches!(item, Ok(Event::Done { .. }) | Err(_)) {
                    // Fin : la connexion est fermée tout de suite.
                    self.over = true;
                    self.pending.clear();
                    self.response = None;
                }
                return Some((item, self));
            }
            if self.over {
                return None;
            }
            let response = self.response.as_mut()?;
            // Délai par bloc reçu (un ping compte) : remis à zéro à chaque
            // lecture.
            let read = tokio::select! {
                biased;
                () = self.cancel.cancelled() => Err(InferenceError::Cancelled),
                read = tokio::time::timeout(self.idle, response.chunk()) => match read {
                    Err(_) => Err(InferenceError::Timeout),
                    Ok(Err(error)) => Err(InferenceError::from_body(&error)),
                    Ok(Ok(chunk)) => Ok(chunk),
                },
            };
            match read {
                Err(error) => self.pending.push_back(Err(error)),
                Ok(Some(bytes)) => match self.parser.push(&bytes) {
                    Ok(items) => items.into_iter().for_each(|item| self.take(item)),
                    Err(error) => self.pending.push_back(Err(error)),
                },
                Ok(None) => self.end(),
            }
        }
    }
}

/// Envoie la conversation et rend le flux de la réponse. Les refus du
/// serveur (chargement, contexte dépassé, image refusée…) arrivent ici, avant
/// le flux. Annuler `cancel` arrête l'attente ou la lecture et ferme la
/// connexion : le serveur arrête alors la génération, ou retire la requête de
/// sa file (relevé de la preuve : emplacement libéré en moins de 300 ms).
/// Attente des en-têtes sans limite par défaut (requête en file derrière une
/// autre, modèle endormi qui se recharge : le serveur ne répond rien
/// jusque-là), ou [`Endpoint::with_header_timeout`]. Une fois le flux
/// commencé, un silence plus long que [`Endpoint::with_idle_timeout`] finit
/// en `Timeout` (le serveur envoie un ping toutes les 30 s pendant un long
/// prompt, qui remet ce délai à zéro).
pub async fn chat_stream(
    endpoint: &Endpoint,
    request: &ChatRequest,
    cancel: CancellationToken,
) -> Result<ChatStream, InferenceError> {
    let send = endpoint
        .post("/v1/chat/completions")
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .json(&request.body_for(endpoint.dialect, true))
        .send();
    let headers = async {
        match endpoint.header_timeout {
            Some(limit) => tokio::time::timeout(limit, send)
                .await
                .map_err(|_| InferenceError::Timeout)?,
            None => send.await,
        }
        .map_err(|error| InferenceError::from_send(&error))
    };
    let response = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(InferenceError::Cancelled),
        sent = headers => sent?,
    };
    let status = response.status();
    if !status.is_success() {
        let body = super::read_json(response, &cancel).await?;
        return Err(InferenceError::from_response(status.as_u16(), &body));
    }
    let event_stream = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    if !event_stream {
        return Err(InferenceError::Malformed(
            "the reply is not an event stream".into(),
        ));
    }
    let reader = Reader {
        response: Some(response),
        parser: SseParser::new(MAX_EVENT),
        assembler: Assembler::default(),
        pending: VecDeque::new(),
        cancel,
        idle: endpoint.idle_timeout,
        over: false,
    };
    Ok(Box::pin(futures_util::stream::unfold(reader, Reader::next)))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn delta(delta: Value) -> Value {
        json!({ "choices": [{ "finish_reason": null, "index": 0, "delta": delta }] })
    }

    #[test]
    fn real_tool_fragments_are_put_back_together() {
        // Fragments relevés sur llama-server b12110 (Qwen3 0.6B).
        let mut assembler = Assembler::default();
        let fragments = [
            json!({ "index": 0, "id": "PbCg", "type": "function", "function": { "name": "get_weather", "arguments": "{" } }),
            json!({ "index": 0, "function": { "arguments": "\"" } }),
            json!({ "index": 0, "function": { "arguments": "city" } }),
            json!({ "index": 0, "function": { "arguments": "\":" } }),
            json!({ "index": 0, "function": { "arguments": " \"" } }),
            json!({ "index": 0, "function": { "arguments": "Paris" } }),
            json!({ "index": 0, "function": { "arguments": "\"}" } }),
        ];
        for fragment in fragments {
            assert!(
                assembler
                    .chunk(&delta(json!({ "tool_calls": [fragment] })))
                    .unwrap()
                    .is_empty()
            );
        }
        let end =
            json!({ "choices": [{ "finish_reason": "tool_calls", "index": 0, "delta": {} }] });
        let events = assembler.chunk(&end).unwrap();
        assert_eq!(
            events,
            [Event::ToolCalls(vec![ToolCall {
                id: "PbCg".into(),
                name: "get_weather".into(),
                arguments: "{\"city\": \"Paris\"}".into(),
            }])]
        );
        // Pas une deuxième fois à [DONE].
        let done = assembler.done().unwrap();
        assert_eq!(done.len(), 1);
        assert!(matches!(
            &done[0],
            Event::Done {
                finish_reason: FinishReason::ToolCalls,
                ..
            }
        ));
    }

    #[test]
    fn calls_are_ordered_by_index_and_get_an_id_when_missing() {
        let mut assembler = Assembler::default();
        assembler
            .chunk(&delta(json!({ "tool_calls": [{ "index": 1, "function": { "name": "b", "arguments": "{}" } }] })))
            .unwrap();
        assembler
            .chunk(&delta(json!({ "tool_calls": [{ "index": 0, "function": { "name": "get_", "arguments": "{\"x\"" } }] })))
            .unwrap();
        assembler
            .chunk(&delta(json!({ "tool_calls": [{ "index": 0, "function": { "name": "weather", "arguments": ":1}" } }] })))
            .unwrap();
        // Nom répété en entier : pas recollé.
        assembler
            .chunk(&delta(
                json!({ "tool_calls": [{ "index": 1, "function": { "name": "b" } }] }),
            ))
            .unwrap();
        // Sans raison de fin, `[DONE]` n'est pas un succès.
        assert!(matches!(
            assembler.done(),
            Err(InferenceError::Interrupted(_))
        ));
        let events = assembler
            .chunk(
                &json!({ "choices": [{ "finish_reason": "tool_calls", "index": 0, "delta": {} }] }),
            )
            .unwrap();
        let Event::ToolCalls(calls) = &events[0] else {
            panic!("{events:?}")
        };
        assert_eq!(calls.len(), 2);
        assert_eq!(
            (calls[0].name.as_str(), calls[0].arguments.as_str()),
            ("get_weather", "{\"x\":1}")
        );
        assert_eq!(calls[1].name, "b");
        assert!(calls[0].id.starts_with("call_") && calls[1].id.starts_with("call_"));
        assert_ne!(calls[0].id, calls[1].id);
        assert_eq!(events.len(), 1);
        // Puis `[DONE]` : la fin seule, appels déjà rendus.
        assert!(matches!(
            assembler.done().unwrap().as_slice(),
            [Event::Done {
                finish_reason: FinishReason::ToolCalls,
                ..
            }]
        ));
    }

    #[test]
    fn content_reasoning_usage_and_errors_of_chunks() {
        let mut assembler = Assembler::default();
        // Premier bloc relevé : rôle et contenu nul, aucun événement.
        assert!(
            assembler
                .chunk(&delta(json!({ "role": "assistant", "content": null })))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            assembler
                .chunk(&delta(json!({ "reasoning_content": "Okay" })))
                .unwrap(),
            [Event::Reasoning("Okay".into())]
        );
        assert_eq!(
            assembler.chunk(&delta(json!({ "content": "Hi" }))).unwrap(),
            [Event::Content("Hi".into())]
        );
        assert!(
            assembler
                .chunk(&delta(json!({ "content": "" })))
                .unwrap()
                .is_empty()
        );
        let end = json!({ "choices": [{ "finish_reason": "length", "index": 0, "delta": {} }] });
        assert!(assembler.chunk(&end).unwrap().is_empty());
        let last = json!({ "choices": [], "usage": { "completion_tokens": 2, "prompt_tokens": 9, "prompt_tokens_details": { "cached_tokens": 8 } },
            "timings": { "cache_n": 8, "prompt_n": 1, "prompt_ms": 0.5, "predicted_n": 2, "predicted_ms": 4.0 } });
        assert!(assembler.chunk(&last).unwrap().is_empty());
        assert!(assembler.finished());
        assert_eq!(
            assembler.done().unwrap(),
            [Event::Done {
                finish_reason: FinishReason::Length,
                usage: Some(Usage {
                    prompt: 9,
                    completion: 2,
                    cached: 8
                }),
                timings: Some(Timings {
                    cache_n: 8,
                    prompt_n: 1,
                    prompt_ms: 0.5,
                    predicted_n: 2,
                    predicted_ms: 4.0
                }),
            }]
        );
        let failure =
            json!({ "error": { "code": 500, "message": "boom", "type": "server_error" } });
        assert_eq!(
            Assembler::default().chunk(&failure),
            Err(InferenceError::Http {
                status: 500,
                message: "boom".into()
            })
        );
    }

    #[test]
    fn tool_calls_are_bounded() {
        let piece = "a".repeat(MAX_BODY / 2 + 1);
        let other = "b".repeat(MAX_BODY / 2 + 1);
        // Arguments, noms (morceaux différents : recollés) et `id` comptent.
        for field in ["arguments", "name", "id"] {
            let mut assembler = Assembler::default();
            let fragment = |piece: &str| {
                let mut entry = json!({ "index": 0, "function": {} });
                if field == "id" {
                    entry["id"] = json!(piece);
                } else {
                    entry["function"][field] = json!(piece);
                }
                delta(json!({ "tool_calls": [entry] }))
            };
            assert!(assembler.chunk(&fragment(&piece)).is_ok(), "{field}");
            assert!(
                matches!(
                    assembler.chunk(&fragment(&other)),
                    Err(InferenceError::Malformed(_))
                ),
                "{field}"
            );
        }
        // Nombre d'appels distincts.
        let mut assembler = Assembler::default();
        for index in 0..MAX_TOOL_CALLS {
            let fragment = json!({ "index": index, "function": { "name": "f" } });
            assert!(
                assembler
                    .chunk(&delta(json!({ "tool_calls": [fragment] })))
                    .is_ok()
            );
        }
        // Un fragment de plus pour un appel connu passe, un nouvel appel non.
        let known = json!({ "index": 0, "function": { "arguments": "{}" } });
        assert!(
            assembler
                .chunk(&delta(json!({ "tool_calls": [known] })))
                .is_ok()
        );
        let new = json!({ "index": MAX_TOOL_CALLS, "function": { "name": "f" } });
        assert!(matches!(
            assembler.chunk(&delta(json!({ "tool_calls": [new] }))),
            Err(InferenceError::Malformed(_))
        ));
    }
}
