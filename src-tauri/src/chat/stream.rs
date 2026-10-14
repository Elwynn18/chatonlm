// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Translated to Rust from Ollama (https://github.com/ollama/ollama),
// file app/ui/ui.go, commit 42e911bc3d05 (buildChatRequest: files appended
// to the message, images, tool calls and tool messages, tools).
// Modified for ChatonLM: think, num_ctx and keep_alive sent as set by the user;
// images left out for a model known not to support vision (Ollama would
// refuse them); tool results are saved with the assistant's message and sent
// back as the tool messages that follow it (the official app saves separate
// tool messages); interrupted calls (no result, no error) are left out.

//! Échange avec `POST /api/chat` : corps de la requête et lecture du flux
//! NDJSON (une ligne JSON par fragment, la dernière avec `done: true`), avec
//! les appels d'outils (`tools`, `message.tool_calls`, messages `tool`).

use std::fmt::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ts_rs::TS;

use crate::i18n::t;
use crate::store::chats::{Role, ToolCall, Turn, TurnFile};

/// Valeur de `think` : marche ou arrêt, ou niveau nommé (« low », « high »...).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(export)]
pub enum Think {
    Enabled(bool),
    Level(String),
}

/// Corps de la requête : historique (avec le raisonnement des réponses,
/// comme l'officiel), `think` seulement s'il est demandé, `num_ctx` seulement
/// s'il est réglé (0 = défaut du serveur), `keep_alive` seulement s'il est
/// réglé (0 = défaut du serveur ; secondes, -1 tel quel), `tools` seulement
/// s'ils sont donnés. Pièces jointes des messages de l'utilisateur comme
/// `buildChatRequest` : texte des fichiers ajouté au contenu, images en base64
/// dans `images` (retirées sans `send_images`). Une réponse avec des appels
/// d'outils terminés les porte dans `tool_calls`, et chacun est suivi de son
/// message `tool` (résultat, ou « Error: … »).
pub fn request_body(
    model: &str,
    turns: &[Turn],
    think: Option<&Think>,
    num_ctx: u32,
    keep_alive: i64,
    send_images: bool,
    tools: Option<&Value>,
) -> Value {
    let mut messages = Vec::with_capacity(turns.len());
    for turn in turns {
        let role = match turn.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        let mut content = turn.content.clone();
        let mut images = Vec::new();
        if turn.role == Role::User {
            for file in &turn.files {
                match file {
                    TurnFile::Image(data) if send_images => images.push(BASE64.encode(data)),
                    TurnFile::Image(_) => {}
                    TurnFile::Text { filename, text } => {
                        let _ = write!(
                            content,
                            "\n--- File: {filename} ---\n{text}\n--- End of {filename} ---"
                        );
                    }
                }
            }
        }
        let mut message = json!({ "role": role, "content": content });
        if !turn.thinking.is_empty() {
            message["thinking"] = json!(turn.thinking);
        }
        if !images.is_empty() {
            message["images"] = json!(images);
        }
        let calls: Vec<&ToolCall> = turn.tool_calls.iter().filter(|c| c.is_finished()).collect();
        if !calls.is_empty() {
            message["tool_calls"] = calls
                .iter()
                .map(|call| {
                    json!({ "function": { "name": call.name, "arguments": arguments(&call.arguments) } })
                })
                .collect();
        }
        messages.push(message);
        for call in calls {
            let content = match (&call.result, &call.error) {
                (Some(result), _) => result.clone(),
                (None, Some(error)) => format!("Error: {error}"),
                (None, None) => continue,
            };
            messages.push(json!({ "role": "tool", "content": content, "tool_name": call.name }));
        }
    }
    let mut body = json!({ "model": model, "messages": messages, "stream": true });
    if let Some(think) = think {
        body["think"] = json!(think);
    }
    if num_ctx > 0 {
        body["options"] = json!({ "num_ctx": num_ctx });
    }
    if keep_alive != 0 {
        body["keep_alive"] = json!(keep_alive);
    }
    if let Some(tools) = tools {
        body["tools"] = tools.clone();
    }
    body
}

/// Arguments enregistrés d'un appel, en objet JSON (`{}` s'ils sont
/// illisibles ou ne sont pas un objet, comme le `ToMap` de l'officiel).
pub fn arguments(text: &str) -> Value {
    serde_json::from_str::<Value>(text)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// Ligne du flux, déjà interprétée.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    Fragment {
        thinking: String,
        content: String,
        /// Appels d'outils de la ligne, dans l'ordre (le plus souvent aucun).
        tool_calls: Vec<StreamToolCall>,
        done: bool,
        /// Ligne `done` : jetons et raison de la fin.
        ending: Option<Ending>,
    },
    /// Erreur du serveur ou du modèle en cours de route.
    Error(String),
}

/// Fin d'une réponse (ligne `done`), propre à ChatonLM : contexte consommé
/// et coupure par `num_predict`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ending {
    /// `prompt_eval_count` (le prompt entier, même en cache : vérifié sur
    /// Ollama 0.40) et `eval_count` ; absent si Ollama ne les donne pas.
    pub counts: Option<(u64, u64)>,
    /// `done_reason: "length"` : limite de jetons atteinte.
    pub length: bool,
}

#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    message: Option<ChunkMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
}

#[derive(Default, Deserialize)]
struct ChunkMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    thinking: String,
    #[serde(default)]
    tool_calls: Option<Vec<ChunkToolCall>>,
}

/// `{"id": …, "function": {"index": …, "name": …, "arguments": {…}}}` ; les
/// autres champs sont ignorés.
#[derive(Deserialize)]
struct ChunkToolCall {
    #[serde(default)]
    function: ChunkFunction,
}

#[derive(Default, Deserialize)]
struct ChunkFunction {
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: Value,
}

/// Appel d'outil reçu dans le flux.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamToolCall {
    pub name: String,
    /// Toujours un objet JSON (`{}` si le modèle n'en a pas donné).
    pub arguments: Value,
}

/// Interprète une ligne ; le champ `error` est examiné en premier.
pub fn parse_line(line: &str) -> Result<Line, String> {
    let chunk: Chunk =
        serde_json::from_str(line).map_err(|error| t!("ollama.unexpectedAnswer", error = error))?;
    if let Some(error) = chunk.error {
        return Ok(Line::Error(error));
    }
    let message = chunk.message.unwrap_or_default();
    let tool_calls = message
        .tool_calls
        .unwrap_or_default()
        .into_iter()
        .map(|call| StreamToolCall {
            name: call.function.name,
            arguments: match call.function.arguments {
                arguments @ Value::Object(_) => arguments,
                _ => json!({}),
            },
        })
        .collect();
    let ending = chunk.done.then(|| Ending {
        counts: chunk.prompt_eval_count.zip(chunk.eval_count),
        length: chunk.done_reason.as_deref() == Some("length"),
    });
    Ok(Line::Fragment {
        thinking: message.thinking,
        content: message.content,
        tool_calls,
        done: chunk.done,
        ending,
    })
}

/// Découpe le flux en lignes complètes : un morceau reçu peut couper une ligne
/// (et même un caractère UTF-8) en deux.
#[derive(Default)]
pub struct LineBuffer {
    pending: Vec<u8>,
}

impl LineBuffer {
    /// Ajoute un morceau ; renvoie les lignes complètes non vides.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(end) = self.pending.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let text = String::from_utf8_lossy(&line[..end]).trim().to_owned();
            if !text.is_empty() {
                lines.push(text);
            }
        }
        lines
    }

    /// Dernière ligne sans saut de ligne final, à la fin du flux.
    pub fn finish(&mut self) -> Option<String> {
        let text = String::from_utf8_lossy(&self.pending).trim().to_owned();
        self.pending.clear();
        (!text.is_empty()).then_some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fake_ollama::fixtures;

    fn turn(role: Role, content: &str, thinking: &str) -> Turn {
        Turn {
            role,
            content: content.into(),
            thinking: thinking.into(),
            files: Vec::new(),
            tool_calls: Vec::new(),
        }
    }

    #[test]
    fn request_carries_history_think_and_context() {
        let turns = [
            turn(Role::User, "Hi", ""),
            turn(Role::Assistant, "Hello!", "Greeting."),
            turn(Role::User, "Again", ""),
        ];
        let body = request_body(
            "qwen3:0.6b",
            &turns,
            Some(&Think::Enabled(false)),
            8192,
            300,
            true,
            None,
        );
        assert_eq!(
            body,
            json!({
                "model": "qwen3:0.6b",
                "messages": [
                    {"role": "user", "content": "Hi"},
                    {"role": "assistant", "content": "Hello!", "thinking": "Greeting."},
                    {"role": "user", "content": "Again"},
                ],
                "stream": true,
                "think": false,
                "options": {"num_ctx": 8192},
                "keep_alive": 300,
            })
        );
        let level = request_body(
            "gpt-oss",
            &turns,
            Some(&Think::Level("high".into())),
            0,
            -1,
            true,
            None,
        );
        assert_eq!(level["think"], json!("high"));
        assert!(level.get("options").is_none());
        // -1 (garder chargé) tel quel.
        assert_eq!(level["keep_alive"], json!(-1));
        assert!(
            request_body("m", &turns, None, 0, 0, true, None)
                .get("think")
                .is_none()
        );
        // 0 : défaut du serveur, rien d'envoyé.
        assert!(
            request_body("m", &turns, None, 0, 0, true, None)
                .get("keep_alive")
                .is_none()
        );
    }

    #[test]
    fn recorded_streams_are_read_line_by_line() {
        let mut thinking = String::new();
        let mut content = String::new();
        let mut done = 0;
        for line in fixtures::CHAT_THINK.lines() {
            match parse_line(line).unwrap() {
                Line::Fragment {
                    thinking: t,
                    content: c,
                    tool_calls,
                    done: d,
                    ending,
                } => {
                    assert!(tool_calls.is_empty());
                    assert_eq!(ending.is_some(), d);
                    thinking.push_str(&t);
                    content.push_str(&c);
                    done += usize::from(d);
                }
                Line::Error(error) => panic!("{error}"),
            }
        }
        assert!(thinking.starts_with("Okay, the user wants me"));
        assert_eq!(content, "\"Hello\"");
        assert_eq!(done, 1);
    }

    #[test]
    fn the_last_line_tells_the_tokens_and_why_it_ended() {
        let ending = |line: &str| match parse_line(line).unwrap() {
            Line::Fragment { ending, .. } => ending,
            Line::Error(error) => panic!("{error}"),
        };
        assert_eq!(ending(r#"{"message":{"content":"a"},"done":false}"#), None);
        assert_eq!(
            ending(r#"{"done":true,"done_reason":"stop","prompt_eval_count":42,"eval_count":5}"#),
            Some(Ending {
                counts: Some((42, 5)),
                length: false
            })
        );
        assert_eq!(
            ending(r#"{"done":true,"done_reason":"length","eval_count":30}"#),
            Some(Ending {
                counts: None,
                length: true
            })
        );
    }

    #[test]
    fn tools_and_finished_tool_calls_are_sent() {
        let call =
            |name: &str, arguments: &str, result: Option<&str>, error: Option<&str>| ToolCall {
                result: result.map(str::to_owned),
                error: error.map(str::to_owned),
                ..ToolCall::new(name, arguments)
            };
        let mut calls_only = turn(Role::Assistant, "", "Need to search.");
        calls_only.tool_calls = vec![
            call(
                "web_search",
                r#"{"query":"ollama"}"#,
                Some(r#"{"results":[]}"#),
                None,
            ),
            call(
                "web_fetch",
                r#"{"url":"https://x.example"}"#,
                None,
                Some("boom"),
            ),
            // Interrompu : ni dans `tool_calls`, ni en message `tool`.
            call("web_search", r#"{"query":"later"}"#, None, None),
            // Arguments illisibles : objet vide.
            call("web_search", "not json", Some("{}"), None),
        ];
        let turns = [
            turn(Role::User, "Search", ""),
            calls_only,
            turn(Role::Assistant, "Found it.", ""),
        ];
        let tools = json!([{ "type": "function", "function": { "name": "web_search" } }]);
        let body = request_body("qwen3:0.6b", &turns, None, 0, 0, true, Some(&tools));
        assert_eq!(body["tools"], tools);
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": "Search"},
                {
                    "role": "assistant",
                    "content": "",
                    "thinking": "Need to search.",
                    "tool_calls": [
                        {"function": {"name": "web_search", "arguments": {"query": "ollama"}}},
                        {"function": {"name": "web_fetch", "arguments": {"url": "https://x.example"}}},
                        {"function": {"name": "web_search", "arguments": {}}},
                    ],
                },
                {"role": "tool", "content": r#"{"results":[]}"#, "tool_name": "web_search"},
                {"role": "tool", "content": "Error: boom", "tool_name": "web_fetch"},
                {"role": "tool", "content": "{}", "tool_name": "web_search"},
                {"role": "assistant", "content": "Found it."},
            ])
        );
        let without = request_body("qwen3:0.6b", &turns[..1], None, 0, 0, true, None);
        assert!(without.get("tools").is_none());
    }

    #[test]
    fn tool_calls_are_read_from_the_stream() {
        // Forme d'Ollama 0.35 (identifiant, index : ignorés).
        let line = r#"{"model":"qwen3:0.6b","created_at":"2026-10-05T10:00:00Z","message":{"role":"assistant","content":"","tool_calls":[{"id":"call_k3n2","function":{"index":0,"name":"web_search","arguments":{"query":"latest Ollama release","max_results":3}}},{"function":{"name":"web_fetch"}}]},"done":false}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Line::Fragment {
                thinking: String::new(),
                content: String::new(),
                tool_calls: vec![
                    StreamToolCall {
                        name: "web_search".into(),
                        arguments: json!({"query": "latest Ollama release", "max_results": 3}),
                    },
                    StreamToolCall {
                        name: "web_fetch".into(),
                        arguments: json!({}),
                    },
                ],
                done: false,
                ending: None,
            }
        );
        let null =
            r#"{"message":{"role":"assistant","content":"x","tool_calls":null},"done":true}"#;
        let Line::Fragment { tool_calls, .. } = parse_line(null).unwrap() else {
            panic!("fragment expected");
        };
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn errors_come_first() {
        assert_eq!(
            parse_line(r#"{"error":"boom","done":true}"#).unwrap(),
            Line::Error("boom".into())
        );
        assert!(parse_line("<html>").is_err());
    }

    #[test]
    fn lines_split_across_chunks_are_put_back_together() {
        let mut buffer = LineBuffer::default();
        assert!(buffer.push(b"{\"a\":").is_empty());
        // Caractère UTF-8 (« é », deux octets) coupé entre deux morceaux.
        let text = "\"é\"}\n".as_bytes();
        assert!(buffer.push(&text[..2]).is_empty());
        assert_eq!(buffer.push(&text[2..]), vec!["{\"a\":\"é\"}".to_owned()]);
        assert!(buffer.push(b"\n\n{\"x\":2}").is_empty());
        assert_eq!(buffer.finish().as_deref(), Some("{\"x\":2}"));
        assert_eq!(buffer.finish(), None);
    }
}
