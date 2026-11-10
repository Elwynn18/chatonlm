// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Translated to Rust from Ollama (https://github.com/ollama/ollama),
// file app/ui/ui.go, commit 42e911bc3d05 (buildChatRequest: files appended
// to the message, images, tool calls and tool messages, tools).
// Modified for ChatonLM: the history is sent to llama-server in the shape of
// OpenAI's API (images as image_url parts typed by their first bytes, any
// format other than PNG or JPEG converted to PNG,
// reasoning as reasoning_content, tool calls with an id, each followed by its
// tool message); images left out for a model without vision (the server
// would refuse them); tool results are saved with the assistant's message and
// sent back as the tool messages that follow it (the official app saves
// separate tool messages); interrupted calls (no result, no error) are left
// out; for a model without tools, finished generate_image calls are told as a
// short line of text (see image_calls_as_text).

//! Échange avec `llama-server` (`/v1/chat/completions`) : historique en
//! messages de l'API d'OpenAI, appels d'outils reçus et fin d'une réponse
//! (le flux lui-même est lu par [`crate::inference`]).

use std::borrow::Cow;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ts_rs::TS;

use crate::inference::{self, FinishReason, Message, Part, Timings, Usage};
use crate::store::chats::{Role, ToolCall, Turn, TurnFile};

/// Valeur de `think` : marche ou arrêt, ou niveau nommé (« low », « high »...).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(export)]
pub enum Think {
    Enabled(bool),
    Level(String),
}

/// Type d'une image d'après ses premiers octets (formats de `stb_image`,
/// plus WebP, que le serveur ne lit qu'avec un `ffmpeg` du PATH).
pub(crate) fn image_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.starts_with(b"BM") {
        "image/bmp"
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

/// Image telle que le moteur la reçoit : PNG et JPEG tels quels ; tout autre
/// format (WebP, GIF, BMP…) converti en PNG au moment de l'envoi (la pièce
/// jointe enregistrée garde son format). b12110 ne lit WebP qu'en lançant un
/// `ffmpeg` trouvé dans le PATH (absent sous Windows en général) : sans
/// conversion, la requête était refusée, puis chaque envoi suivant de la
/// conversation (relecture L5a, B2). Image illisible : envoyée telle quelle
/// (le serveur la refuse, erreur `model`).
pub(crate) fn engine_image(bytes: &[u8]) -> (&'static str, Cow<'_, [u8]>) {
    let mime = image_mime(bytes);
    if matches!(mime, "image/png" | "image/jpeg") {
        return (mime, Cow::Borrowed(bytes));
    }
    let converted = image::load_from_memory(bytes).and_then(|decoded| {
        let mut png = std::io::Cursor::new(Vec::new());
        decoded.write_to(&mut png, image::ImageFormat::Png)?;
        Ok(png.into_inner())
    });
    match converted {
        Ok(png) => ("image/png", Cow::Owned(png)),
        Err(_) => (mime, Cow::Borrowed(bytes)),
    }
}

/// Identifiant d'un appel d'outil de l'historique (la base n'en garde pas) :
/// stable d'une requête à l'autre (cache du prompt), unique dans la
/// conversation.
fn call_id(turn: usize, call: usize) -> String {
    format!("call_{turn}_{call}")
}

/// Historique en messages, comme `buildChatRequest` de l'officiel : texte
/// des fichiers ajouté au contenu, images en parties `image_url` (laissées
/// de côté sans `send_images`), raisonnement des réponses en
/// `reasoning_content` (les gabarits qui le relisent le gardent, les autres
/// l'ignorent), appels terminés d'une réponse en `tool_calls`, chacun suivi
/// de son message `tool` (résultat, ou « Error: … ») ; un appel interrompu
/// (ni résultat ni erreur) est ignoré.
pub(crate) fn messages(turns: &[Turn], send_images: bool) -> Vec<Message> {
    let mut messages = Vec::with_capacity(turns.len());
    for (index, turn) in turns.iter().enumerate() {
        match turn.role {
            Role::User => {
                let mut content = turn.content.clone();
                let mut images = Vec::new();
                for file in &turn.files {
                    match file {
                        TurnFile::Image(data) if send_images => {
                            let (mime, data) = engine_image(data);
                            images.push(Part::image(mime, &data));
                        }
                        TurnFile::Image(_) => {}
                        TurnFile::Text { filename, text } => {
                            let _ = write!(
                                content,
                                "\n--- File: {filename} ---\n{text}\n--- End of {filename} ---"
                            );
                        }
                    }
                }
                messages.push(if images.is_empty() {
                    Message::user(content)
                } else {
                    let mut parts = vec![Part::Text(content)];
                    parts.extend(images);
                    Message::user_parts(parts)
                });
            }
            Role::Assistant => {
                let calls: Vec<_> = turn
                    .tool_calls
                    .iter()
                    .filter(|call| call.is_finished())
                    .collect();
                let mut message = Message::assistant(turn.content.clone());
                if !turn.thinking.is_empty() {
                    message.reasoning_content = Some(turn.thinking.clone());
                }
                message.tool_calls = calls
                    .iter()
                    .enumerate()
                    .map(|(number, call)| inference::ToolCall {
                        id: call_id(index, number),
                        name: call.name.clone(),
                        arguments: arguments(&call.arguments).to_string(),
                    })
                    .collect();
                messages.push(message);
                for (number, call) in calls.iter().enumerate() {
                    let content = match (&call.result, &call.error) {
                        (Some(result), _) => result.clone(),
                        (None, Some(error)) => format!("Error: {error}"),
                        (None, None) => continue,
                    };
                    messages.push(Message::tool(call_id(index, number), &call.name, content));
                }
            }
        }
    }
    messages
}

/// Historique pour un modèle sans la capacité `tools` (propre à
/// ChatonLM) : ses gabarits ignorent `tool_calls` et les messages `tool`, il
/// ne saurait pas qu'une image a été faite (bouton « Image », ou appel d'un
/// envoi précédent vers un autre modèle). Chaque appel `generate_image` fini
/// est retiré des appels de sa réponse et dit en une ligne ajoutée à son
/// texte : `[Image created: <prompt>]`, ou `[Image not created: <erreur>]`
/// (consigne au modèle : en anglais). Les autres outils sont inchangés ; un
/// appel interrompu reste ignoré.
pub fn image_calls_as_text(turns: &[Turn]) -> Cow<'_, [Turn]> {
    let is_image = |call: &ToolCall| call.name == super::images::TOOL_NAME && call.is_finished();
    if !turns
        .iter()
        .any(|turn| turn.role == Role::Assistant && turn.tool_calls.iter().any(is_image))
    {
        return Cow::Borrowed(turns);
    }
    let mut turns = turns.to_vec();
    for turn in turns.iter_mut().filter(|turn| turn.role == Role::Assistant) {
        let (told, kept): (Vec<ToolCall>, Vec<ToolCall>) = std::mem::take(&mut turn.tool_calls)
            .into_iter()
            .partition(is_image);
        turn.tool_calls = kept;
        for call in told {
            let prompt = arguments(&call.arguments)["prompt"]
                .as_str()
                .map(str::trim)
                .unwrap_or_default()
                .to_owned();
            // i18n-ignore : consigne au modèle, en anglais.
            let line = match (&call.result, &call.error, prompt.is_empty()) {
                (Some(_), _, false) => format!("[Image created: {prompt}]"),
                (Some(_), _, true) => "[Image created]".to_owned(),
                (None, Some(error), _) => format!("[Image not created: {error}]"),
                (None, None, _) => continue,
            };
            if !turn.content.is_empty() && !turn.content.ends_with('\n') {
                turn.content.push('\n');
            }
            turn.content.push_str(&line);
        }
    }
    Cow::Owned(turns)
}

/// Arguments enregistrés d'un appel, en objet JSON (`{}` s'ils sont
/// illisibles ou ne sont pas un objet, comme le `ToMap` de l'officiel).
pub fn arguments(text: &str) -> Value {
    serde_json::from_str::<Value>(text)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// Appel d'outil reçu dans le flux.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamToolCall {
    pub name: String,
    /// Toujours un objet JSON (`{}` si le modèle n'en a pas donné, ou des
    /// arguments illisibles, comme le `ToMap` de l'officiel).
    pub arguments: Value,
}

impl From<inference::ToolCall> for StreamToolCall {
    fn from(call: inference::ToolCall) -> Self {
        StreamToolCall {
            arguments: arguments(&call.arguments),
            name: call.name,
        }
    }
}

/// Fin d'une réponse, propre à ChatonLM : contexte consommé et coupure par
/// `max_tokens` (ou contexte plein).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ending {
    /// Jetons du prompt (cache compris) et jetons produits ; absent si le
    /// serveur ne les donne pas.
    pub counts: Option<(u64, u64)>,
    /// `finish_reason: "length"` : limite de jetons atteinte.
    pub length: bool,
}

impl Ending {
    /// Fin d'après le dernier bloc du flux : `timings` d'abord
    /// (`cache_n + prompt_n`, `predicted_n` : le contexte occupé à la fin),
    /// sinon `usage` (`prompt_tokens`, cache compris, et
    /// `completion_tokens`).
    pub fn of(finish: &FinishReason, usage: Option<&Usage>, timings: Option<&Timings>) -> Ending {
        let counts = timings
            .map(|timings| {
                (
                    u64::from(timings.cache_n) + u64::from(timings.prompt_n),
                    u64::from(timings.predicted_n),
                )
            })
            .or_else(|| usage.map(|usage| (u64::from(usage.prompt), u64::from(usage.completion))));
        Ending {
            counts,
            length: *finish == FinishReason::Length,
        }
    }
}

/// Vraie petite WebP (4 × 3, VP8 avec perte, écrite par libwebp) : pixel
/// (0, 0) bleu, le reste roux.
#[cfg(test)]
pub(crate) fn small_webp() -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(
            "UklGRkwAAABXRUJQVlA4IEAAAADwAQCdASoEAAMAAUAmJbACdLoB+AAETAAA/uppv/fv9s12SDMnkf+dxx+K3IXzbuF+h3/2D02+g7+g7/ZIAAAA",
        )
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::ChatRequest;

    /// Images pour le moteur : PNG et JPEG tels quels ; WebP et GIF (vrais
    /// fichiers) convertis en PNG lisible, aux mêmes dimensions ; illisible :
    /// tel quel (relecture L5a, B2).
    #[test]
    fn images_other_than_png_or_jpeg_are_sent_as_png() {
        let png = crate::attachments::samples::png(2, 2, [255, 0, 0]);
        assert!(matches!(
            engine_image(&png),
            ("image/png", Cow::Borrowed(_))
        ));
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0];
        assert!(matches!(
            engine_image(&jpeg),
            ("image/jpeg", Cow::Borrowed(_))
        ));
        use base64::Engine as _;
        let gif = base64::engine::general_purpose::STANDARD
            .decode("R0lGODdhBAADAIEAAMhQFAAA/wAAAAAAACwAAAAABAADAAAICQADABhIsCCAgAA7")
            .unwrap();
        for (original, lossless) in [(small_webp(), false), (gif, true)] {
            let (mime, converted) = engine_image(&original);
            assert_eq!(mime, "image/png");
            assert_eq!(image_mime(&converted), "image/png");
            let decoded =
                image::load_from_memory_with_format(&converted, image::ImageFormat::Png).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (4, 3));
            // GIF sans perte : le pixel bleu tel quel (la WebP avec perte
            // mêle les couleurs d'une si petite image).
            if lossless {
                assert_eq!(decoded.to_rgb8().get_pixel(0, 0).0, [0, 0, 255]);
                assert_eq!(decoded.to_rgb8().get_pixel(1, 0).0, [200, 80, 20]);
            }
        }
        let broken = b"RIFF\0\0\0\0WEBPVP8 broken".to_vec();
        let (mime, kept) = engine_image(&broken);
        assert_eq!(mime, "image/webp");
        assert_eq!(&*kept, &broken[..]);
        // Dans l'historique envoyé : une partie PNG.
        let mut user = turn(Role::User, "Look", "");
        user.files = vec![TurnFile::Image(small_webp())];
        let body = ChatRequest::new(messages(&[user], true)).body(false);
        let url = body["messages"][0]["content"][1]["image_url"]["url"]
            .as_str()
            .unwrap();
        assert!(url.starts_with("data:image/png;base64,iVBOR"), "{url}");
    }

    fn turn(role: Role, content: &str, thinking: &str) -> Turn {
        Turn {
            role,
            content: content.into(),
            thinking: thinking.into(),
            files: Vec::new(),
            tool_calls: Vec::new(),
        }
    }

    fn call(name: &str, arguments: &str, result: Option<&str>, error: Option<&str>) -> ToolCall {
        ToolCall {
            result: result.map(str::to_owned),
            error: error.map(str::to_owned),
            ..ToolCall::new(name, arguments)
        }
    }

    #[test]
    fn images_are_typed_by_their_bytes() {
        assert_eq!(image_mime(b"\x89PNG\r\n"), "image/png");
        assert_eq!(image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), "image/jpeg");
        assert_eq!(image_mime(b"GIF89a"), "image/gif");
        assert_eq!(image_mime(b"BM\0\0"), "image/bmp");
        assert_eq!(image_mime(b"RIFF\0\0\0\0WEBPVP8 "), "image/webp");
        assert_eq!(image_mime(b"RIFF"), "application/octet-stream");
    }

    #[test]
    fn the_history_takes_the_openai_shape() {
        let mut user = turn(Role::User, "Look", "");
        user.files = vec![
            TurnFile::Image(b"\x89PNG".to_vec()),
            TurnFile::Text {
                filename: "a.txt".into(),
                text: "hello".into(),
            },
        ];
        let mut answer = turn(Role::Assistant, "Searching", "hm");
        answer.tool_calls = vec![
            call(
                "web_search",
                r#"{"query":"cats"}"#,
                Some("{\"ok\":1}"),
                None,
            ),
            // Interrompu : ni envoyé, ni suivi d'un message `tool`.
            call("web_fetch", r#"{"url":"x"}"#, None, None),
            // Arguments illisibles : `{}`.
            call("web_fetch", "not json", None, Some("refused")),
        ];
        let turns = [user, answer, turn(Role::User, "And?", "")];
        let body = ChatRequest::new(messages(&turns, true)).body(false);
        assert_eq!(
            body["messages"][0]["content"],
            json!([
                { "type": "text", "text": "Look\n--- File: a.txt ---\nhello\n--- End of a.txt ---" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw==" } }
            ])
        );
        assert_eq!(
            body["messages"][1],
            json!({ "role": "assistant", "content": "Searching", "reasoning_content": "hm",
                "tool_calls": [
                    { "id": "call_1_0", "type": "function", "function": { "name": "web_search", "arguments": "{\"query\":\"cats\"}" } },
                    { "id": "call_1_1", "type": "function", "function": { "name": "web_fetch", "arguments": "{}" } }
                ] })
        );
        assert_eq!(
            body["messages"][2],
            json!({ "role": "tool", "content": "{\"ok\":1}", "tool_call_id": "call_1_0", "name": "web_search" })
        );
        assert_eq!(
            body["messages"][3],
            json!({ "role": "tool", "content": "Error: refused", "tool_call_id": "call_1_1", "name": "web_fetch" })
        );
        assert_eq!(
            body["messages"][4],
            json!({ "role": "user", "content": "And?" })
        );
        assert_eq!(body["messages"].as_array().unwrap().len(), 5);
        // Sans images : texte seul, en chaîne ; jamais de raisonnement de
        // l'utilisateur.
        let mut quiet = turns[0].clone();
        quiet.thinking = "never sent".into();
        let body = ChatRequest::new(messages(&[quiet], false)).body(false);
        assert_eq!(
            body["messages"][0],
            json!({ "role": "user", "content": "Look\n--- File: a.txt ---\nhello\n--- End of a.txt ---" })
        );
    }

    #[test]
    fn image_calls_are_told_in_text_to_a_model_without_tools() {
        let mut mixed = turn(Role::Assistant, "Let me draw.", "Drawing.");
        mixed.tool_calls = vec![
            call(
                "generate_image",
                r#"{"prompt":" a red fox ","width":512}"#,
                Some(r#"{"attachmentId":1}"#),
                None,
            ),
            call("web_search", r#"{"query":"fox"}"#, Some("{}"), None),
            call(
                "generate_image",
                r#"{"prompt":"a cat"}"#,
                None,
                Some("boom"),
            ),
            // Interrompu : ni texte, ni appel.
            call("generate_image", r#"{"prompt":"a dog"}"#, None, None),
            // Description illisible.
            call("generate_image", "not json", Some("{}"), None),
        ];
        let mut button = turn(Role::Assistant, "", "");
        button.tool_calls = vec![call(
            "generate_image",
            r#"{"prompt":"a lighthouse"}"#,
            Some("{}"),
            None,
        )];
        // L'utilisateur n'a pas d'appels ; son texte n'est jamais touché.
        let turns = [turn(Role::User, "Draw", ""), mixed, button];
        let told = image_calls_as_text(&turns);
        assert_eq!(told[0], turns[0]);
        assert_eq!(
            told[1].content,
            "Let me draw.\n[Image created: a red fox]\n[Image not created: boom]\n[Image created]"
        );
        assert_eq!(told[1].thinking, "Drawing.");
        assert_eq!(
            told[1]
                .tool_calls
                .iter()
                .map(|call| call.name.as_str())
                .collect::<Vec<_>>(),
            ["web_search", "generate_image"]
        );
        assert_eq!(told[2].content, "[Image created: a lighthouse]");
        assert!(told[2].tool_calls.is_empty());
        let body = ChatRequest::new(messages(&told, true)).body(false);
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": "Draw"},
                {
                    "role": "assistant",
                    "content": "Let me draw.\n[Image created: a red fox]\n[Image not created: boom]\n[Image created]",
                    "reasoning_content": "Drawing.",
                    "tool_calls": [{"id": "call_1_0", "type": "function",
                        "function": {"name": "web_search", "arguments": "{\"query\":\"fox\"}"}}],
                },
                {"role": "tool", "content": "{}", "tool_call_id": "call_1_0", "name": "web_search"},
                {"role": "assistant", "content": "[Image created: a lighthouse]"},
            ])
        );
        // Sans appel d'image : l'historique tel quel, sans copie.
        let plain = [turn(Role::User, "Hi", "")];
        assert!(matches!(image_calls_as_text(&plain), Cow::Borrowed(_)));
    }

    #[test]
    fn received_calls_always_carry_an_object() {
        let received = |arguments: &str| {
            StreamToolCall::from(inference::ToolCall {
                id: "call_1".into(),
                name: "web_search".into(),
                arguments: arguments.into(),
            })
        };
        assert_eq!(
            received(r#"{"query":"cats","max_results":3}"#),
            StreamToolCall {
                name: "web_search".into(),
                arguments: json!({"query": "cats", "max_results": 3}),
            }
        );
        assert_eq!(received("").arguments, json!({}));
        assert_eq!(received("[1]").arguments, json!({}));
        assert_eq!(received("{\"query\":").arguments, json!({}));
    }

    #[test]
    fn the_end_tells_the_tokens_and_why_it_stopped() {
        let timings = Timings {
            cache_n: 30,
            prompt_n: 12,
            predicted_n: 5,
            ..Timings::default()
        };
        let usage = Usage {
            prompt: 40,
            completion: 6,
            cached: 30,
        };
        // Les temps d'abord : prompt entier (cache compris), puis produits.
        assert_eq!(
            Ending::of(&FinishReason::Stop, Some(&usage), Some(&timings)),
            Ending {
                counts: Some((42, 5)),
                length: false
            }
        );
        assert_eq!(
            Ending::of(&FinishReason::Length, Some(&usage), None),
            Ending {
                counts: Some((40, 6)),
                length: true
            }
        );
        assert_eq!(
            Ending::of(&FinishReason::ToolCalls, None, None),
            Ending {
                counts: None,
                length: false
            }
        );
    }
}
