//! Plan d'une visite guidée parlée (`tour.plan`, propre à ChatonLM) : le
//! modèle découpe son explication d'un fichier, d'une sélection ou d'une
//! modification en segments liés à des lignes, en sortie JSON contrainte
//! (`format` de `/api/chat`, comme les parties des cours). Le plan est
//! validé (lignes dans la plage donnée, débuts croissants, 1 à 16 segments,
//! texte de 700 caractères au plus, rien d'autre) ; un plan invalide est
//! redemandé une fois, puis refusé.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::chat::Server;
use crate::i18n::t;
use crate::ollama::{client, models};

/// Segments d'un plan, au plus.
pub const MAX_SEGMENTS: usize = 16;
/// Caractères d'un segment, au plus.
pub const MAX_TEXT: usize = 700;
/// Caractères du code envoyé, au plus (au-delà : paramètres refusés).
pub const MAX_CODE: usize = 100_000;
/// Délai d'une réponse du modèle.
const TIMEOUT: Duration = Duration::from_secs(300);
/// Réponse lue au plus.
const MAX_BODY: usize = 4 * 1024 * 1024;

/// Ce qui est expliqué.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    File,
    Selection,
    Diff,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanParams {
    #[serde(default)]
    pub model: Option<String>,
    pub locale: String,
    pub path: String,
    pub language: String,
    pub code: String,
    pub first_line: u32,
    pub kind: Kind,
}

/// Segment validé.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub lines: (u32, u32),
    pub text: String,
}

/// Refus d'un plan (texte traduit).
#[derive(Debug, PartialEq, Eq)]
pub enum PlanError {
    /// Paramètres invalides (code vide ou trop long, première ligne nulle).
    Params,
    Refused(String),
}

/// Lignes couvertes par `code` à partir de `first`.
pub fn range(code: &str, first: u32) -> (u32, u32) {
    let count = u32::try_from(code.lines().count().max(1)).unwrap_or(u32::MAX);
    (first, first.saturating_add(count - 1))
}

/// Schéma de la sortie contrainte.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "segments": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_SEGMENTS,
                "items": {
                    "type": "object",
                    "properties": {
                        "lines": {
                            "type": "array",
                            "items": { "type": "integer" },
                            "minItems": 2,
                            "maxItems": 2,
                        },
                        "text": { "type": "string", "maxLength": MAX_TEXT },
                    },
                    "required": ["lines", "text"],
                },
            },
        },
        "required": ["segments"],
    })
}

/// Plan rendu par le modèle, validé : un objet `{ segments }` et rien
/// d'autre ; chaque segment `{ lines: [a, b], text }` et rien d'autre, avec
/// `first <= a <= b <= last`, des débuts croissants (au sens large), un
/// texte non vide de `MAX_TEXT` caractères au plus ; 1 à `MAX_SEGMENTS`
/// segments.
pub fn validate(value: &Value, (first, last): (u32, u32)) -> Option<Vec<Planned>> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    let segments = object.get("segments")?.as_array()?;
    if segments.is_empty() || segments.len() > MAX_SEGMENTS {
        return None;
    }
    let mut planned: Vec<Planned> = Vec::with_capacity(segments.len());
    for segment in segments {
        let segment = segment.as_object()?;
        if segment.len() != 2 {
            return None;
        }
        let lines = segment.get("lines")?.as_array()?;
        let [a, b] = lines.as_slice() else {
            return None;
        };
        let a = u32::try_from(a.as_u64()?).ok()?;
        let b = u32::try_from(b.as_u64()?).ok()?;
        if a < first || b > last || a > b {
            return None;
        }
        if planned.last().is_some_and(|previous| a < previous.lines.0) {
            return None;
        }
        let text = segment.get("text")?.as_str()?.trim();
        if text.is_empty() || text.chars().count() > MAX_TEXT {
            return None;
        }
        planned.push(Planned {
            lines: (a, b),
            text: text.to_owned(),
        });
    }
    Some(planned)
}

/// Nom anglais de la langue des textes (consignes du modèle en anglais,
/// comme celles des cours).
fn language_name(locale: &str) -> &'static str {
    match locale.split(['-', '_']).next().unwrap_or_default() {
        "fr" => "French",
        "es" => "Spanish",
        _ => "English",
    }
}

/// Consignes du plan.
pub fn system(kind: Kind, locale: &str) -> String {
    let what = match kind {
        Kind::File => "The code is a whole file.",
        Kind::Selection => "The code is a part of a file selected by the user.",
        Kind::Diff => {
            "The code is a diff of the user's uncommitted changes: explain what changed and why it matters."
        }
    };
    // i18n-ignore : consignes du modèle, pas un texte d'interface.
    format!(
        "You explain code out loud, as a guided tour, to the developer who wrote it. {what} \
         Split your explanation into 1 to {MAX_SEGMENTS} segments, in reading order. Each segment \
         covers a range of lines [first, last], using the line numbers shown at the start of each \
         line, and its text explains those lines in at most {MAX_TEXT} characters. Ranges go \
         forward: each one starts at or after the start of the previous one. Texts are read aloud \
         by a voice: plain spoken sentences, no Markdown, no code, no lists, no line numbers. Write \
         every text in {}.",
        language_name(locale)
    )
}

/// Message de l'utilisateur : le fichier, puis le code, ses lignes
/// numérotées (`   12 | code`).
pub fn user(params: &PlanParams) -> String {
    let mut text = format!("File: {} ({})\n\n", params.path, params.language);
    let mut number = params.first_line;
    for line in params.code.lines() {
        text.push_str(&format!("{number:>5} | {line}\n"));
        number = number.saturating_add(1);
    }
    text
}

/// Contexte demandé : celui réglé, sinon assez pour le code (borné).
fn context(server: &Server, code: &str) -> u32 {
    let needed = u32::try_from(code.chars().count() / 3 + 3_000).unwrap_or(u32::MAX);
    let needed = needed.clamp(8_192, 32_768).div_ceil(1_024) * 1_024;
    server.context_length.max(needed)
}

/// Demande le plan au modèle `model` (deux essais) ; erreur traduite.
pub async fn plan(
    server: &Server,
    model: &str,
    params: &PlanParams,
) -> Result<Vec<Planned>, PlanError> {
    if params.code.trim().is_empty()
        || params.code.chars().count() > MAX_CODE
        || params.first_line == 0
    {
        return Err(PlanError::Params);
    }
    let model = model.trim();
    if model.is_empty() {
        return Err(PlanError::Refused(t!("bridge.tour.noModel")));
    }
    // Modèle du cloud, réglage coupé : rien ne part (même pas `/api/show`).
    if let Some(message) = server.refuses(model, models::is_cloud_name(model)) {
        return Err(PlanError::Refused(message));
    }
    let details = models::show(&server.url, model).await.ok();
    let remote = models::is_cloud_name(model) || details.as_ref().is_some_and(|d| d.remote);
    if let Some(message) = server.refuses(model, remote) {
        return Err(PlanError::Refused(message));
    }
    let think_off = crate::dictation::polish::think_off(details.as_ref(), false);
    let mut body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system(params.kind, &params.locale) },
            { "role": "user", "content": user(params) },
        ],
        "stream": false,
        "format": schema(),
        "options": { "temperature": 0.3, "num_ctx": context(server, &params.code) },
    });
    if think_off {
        body["think"] = json!(false);
    }
    if server.keep_alive != 0 {
        body["keep_alive"] = json!(server.keep_alive);
    }
    let range = range(&params.code, params.first_line);
    for _ in 0..2 {
        let content = ask(server, &body).await?;
        if let Some(planned) = parse(&content).and_then(|value| validate(&value, range)) {
            return Ok(planned);
        }
        // i18n-ignore : journal.
        eprintln!(
            "chatonlm: tour plan from {model} unusable ({} characters)",
            content.chars().count()
        );
    }
    Err(PlanError::Refused(t!("bridge.tour.invalid")))
}

/// Objet JSON d'une réponse (texte autour toléré).
fn parse(content: &str) -> Option<Value> {
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    (start < end)
        .then(|| serde_json::from_str(&content[start..=end]).ok())
        .flatten()
}

/// Une requête ; rend le contenu du message.
async fn ask(server: &Server, body: &Value) -> Result<String, PlanError> {
    let failed = |detail: &dyn std::fmt::Display| {
        let message = t!("bridge.tour.failed", detail = detail);
        PlanError::Refused(message)
    };
    let base = server.url.as_str();
    let url = client::server_url(base).map_err(|error| failed(&error))?;
    let http = client::client(&url, Some(TIMEOUT)).map_err(|error| failed(&error))?;
    let response = http
        .post(format!("{base}/api/chat"))
        .json(body)
        .send()
        .await
        .map_err(|error| {
            if error.is_connect() {
                PlanError::Refused(t!("ollama.unreachableAt", address = base))
            } else {
                failed(&error.without_url())
            }
        })?;
    let status = response.status();
    let mut response = response;
    let mut data = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| failed(&error.without_url()))?
    {
        if data.len() + chunk.len() > MAX_BODY {
            return Err(failed(&t!("ollama.httpStatus", status = status.as_u16())));
        }
        data.extend_from_slice(&chunk);
    }
    let text = String::from_utf8_lossy(&data);
    if !status.is_success() {
        let message = crate::chat::error_line(&text);
        return Err(if message.trim().is_empty() {
            failed(&t!("ollama.httpStatus", status = status.as_u16()))
        } else {
            failed(&message)
        });
    }
    let answer: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if let Some(message) = answer["error"].as_str().filter(|m| !m.trim().is_empty()) {
        return Err(failed(&message));
    }
    Ok(answer["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RANGE: (u32, u32) = (10, 20);

    fn segment(a: u32, b: u32, text: &str) -> Value {
        json!({ "lines": [a, b], "text": text })
    }

    #[test]
    fn a_plan_is_validated() {
        let good =
            json!({ "segments": [segment(10, 12, "Imports."), segment(12, 20, "The loop.")] });
        assert_eq!(
            validate(&good, RANGE),
            Some(vec![
                Planned {
                    lines: (10, 12),
                    text: "Imports.".into()
                },
                Planned {
                    lines: (12, 20),
                    text: "The loop.".into()
                },
            ])
        );
        let refused = [
            // Rien, trop de segments, autre clé.
            json!({ "segments": [] }),
            json!({ "segments": vec![segment(10, 10, "x"); MAX_SEGMENTS + 1] }),
            json!({ "segments": [segment(10, 11, "x")], "extra": 1 }),
            json!([segment(10, 11, "x")]),
            // Hors de la plage, à l'envers, débuts qui reculent.
            json!({ "segments": [segment(9, 11, "x")] }),
            json!({ "segments": [segment(15, 21, "x")] }),
            json!({ "segments": [segment(14, 12, "x")] }),
            json!({ "segments": [segment(14, 15, "x"), segment(12, 13, "y")] }),
            // Lignes mal formées, texte vide ou trop long, autre champ.
            json!({ "segments": [{ "lines": [10], "text": "x" }] }),
            json!({ "segments": [{ "lines": [10, "11"], "text": "x" }] }),
            json!({ "segments": [{ "lines": [10, -1], "text": "x" }] }),
            json!({ "segments": [segment(10, 11, "  ")] }),
            json!({ "segments": [segment(10, 11, &"x".repeat(MAX_TEXT + 1))] }),
            json!({ "segments": [{ "lines": [10, 11], "text": "x", "note": "y" }] }),
        ];
        for plan in refused {
            assert_eq!(validate(&plan, RANGE), None, "{plan}");
        }
        // Exactement 16 segments, 700 caractères, même début : accepté.
        let full =
            json!({ "segments": vec![segment(10, 10, &"é".repeat(MAX_TEXT)); MAX_SEGMENTS] });
        assert_eq!(
            validate(&full, RANGE).map(|plan| plan.len()),
            Some(MAX_SEGMENTS)
        );
    }

    #[test]
    fn the_prompt_numbers_the_lines_and_names_the_language() {
        let params = PlanParams {
            model: None,
            locale: "fr-FR".into(),
            path: "src/main.rs".into(),
            language: "rust".into(),
            code: "fn main() {\n}".into(),
            first_line: 41,
            kind: Kind::Selection,
        };
        assert_eq!(range(&params.code, params.first_line), (41, 42));
        assert_eq!(
            user(&params),
            "File: src/main.rs (rust)\n\n   41 | fn main() {\n   42 | }\n"
        );
        assert!(system(params.kind, &params.locale).ends_with("in French."));
        assert!(system(Kind::Diff, "es").ends_with("in Spanish."));
        assert!(system(Kind::File, "de").ends_with("in English."));
    }

    #[test]
    fn a_json_object_is_found_in_the_answer() {
        assert_eq!(
            parse("Here:\n{\"segments\": []}\n"),
            Some(json!({ "segments": [] }))
        );
        assert_eq!(parse("no json"), None);
    }
}
