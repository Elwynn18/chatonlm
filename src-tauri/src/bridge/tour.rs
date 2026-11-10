//! Plan d'une visite guidée parlée (`tour.plan`, propre à ChatonLM) : le
//! modèle découpe son explication d'un fichier, d'une sélection ou d'une
//! modification en segments liés à des lignes, en sortie JSON contrainte
//! (`response_format` json_schema de `llama-server`, comme les parties des
//! cours ; numéros de ligne bornés au code par le schéma ; température 0,3 ;
//! contexte imposé à l'ouverture du modèle, assez pour le code). Le plan est assaini plutôt que rejeté (les
//! petits modèles débordent la fin, reculent ou se répètent) : clés en trop
//! ignorées ; plage remise dans l'ordre et ramenée au code ; segment écarté
//! s'il tombe hors du code, recule, répète une plage gardée ou n'a pas de
//! texte ; texte trop long coupé à une fin de phrase ; 16 segments au plus.
//! Un plan dont il ne reste rien est redemandé une fois, puis refusé.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use tokio_util::sync::CancellationToken;

use crate::chat::Server;
use crate::chat::stream::Think;
use crate::i18n::t;
use crate::inference::{self, ChatRequest, Message, ResponseFormat};
use crate::llm::{LlmError, OpenOptions, Session};

/// Segments d'un plan, au plus.
pub const MAX_SEGMENTS: usize = 16;
/// Caractères d'un segment, au plus.
pub const MAX_TEXT: usize = 700;
/// Caractères du code envoyé, au plus (au-delà : paramètres refusés).
pub const MAX_CODE: usize = 100_000;
/// Délai d'une réponse du modèle.
const TIMEOUT: Duration = Duration::from_secs(300);
/// Température : un plan proche du code, peu inventif.
const TEMPERATURE: f32 = 0.3;

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

/// Segment gardé.
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

/// Segments demandés au plus : `MAX_SEGMENTS`, ou un par ligne si le code
/// est plus court.
fn max_segments((first, last): (u32, u32)) -> usize {
    usize::try_from(last - first)
        .map_or(MAX_SEGMENTS, |lines| lines.saturating_add(1))
        .min(MAX_SEGMENTS)
}

/// Schéma de la sortie contrainte : numéros de ligne bornés à `range`
/// (Ollama les tient par sa grammaire).
pub fn schema(range: (u32, u32)) -> Value {
    let (first, last) = range;
    json!({
        "type": "object",
        "properties": {
            "segments": {
                "type": "array",
                "minItems": 1,
                "maxItems": max_segments(range),
                "items": {
                    "type": "object",
                    "properties": {
                        "lines": {
                            "type": "array",
                            "items": { "type": "integer", "minimum": first, "maximum": last },
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

/// Plan rendu par le modèle, assaini (rien n'est rejeté pour un seul
/// segment fautif) : `segments` est lu, toute autre clé ignorée (objet et
/// segments) ; chaque segment `{ lines: [a, b], text }` a deux entiers (sinon
/// écarté), remis dans l'ordre (a > b : échangés) et ramenés dans
/// `[first, last]` ; il est écarté s'il tombe entièrement hors du code, s'il
/// recule (début avant celui du dernier gardé), s'il répète une plage déjà
/// gardée, ou si son texte est vide ; un texte trop long est coupé
/// ([`spoken`]) ; `MAX_SEGMENTS` gardés au plus. Diff : `headers` marque ses
/// lignes d'en-tête ([`diff_headers`], index 0 = `first`), retirées des
/// bords d'un segment ; un segment qui n'en couvre pas d'autres est écarté.
/// `None` : aucun segment ne reste.
pub fn sanitize(value: &Value, range: (u32, u32), headers: &[bool]) -> Option<Vec<Planned>> {
    let (first, last) = range;
    let header = |line: u32| {
        usize::try_from(line - first)
            .ok()
            .and_then(|index| headers.get(index))
            .copied()
            .unwrap_or(false)
    };
    let segments = value.get("segments")?.as_array()?;
    let mut planned: Vec<Planned> = Vec::new();
    for segment in segments {
        if planned.len() == MAX_SEGMENTS {
            break;
        }
        let Some((a, b)) = segment.get("lines").and_then(line_pair) else {
            continue;
        };
        let (a, b) = (a.min(b), a.max(b));
        if b < i64::from(first) || a > i64::from(last) {
            continue;
        }
        let (mut a, mut b) = (bounded(a, range), bounded(b, range));
        while a < b && header(a) {
            a += 1;
        }
        while b > a && header(b) {
            b -= 1;
        }
        if header(a) {
            continue;
        }
        let lines = (a, b);
        let backwards = planned
            .last()
            .is_some_and(|previous| lines.0 < previous.lines.0);
        if backwards || planned.iter().any(|kept| kept.lines == lines) {
            continue;
        }
        let Some(text) = segment.get("text").and_then(Value::as_str).and_then(spoken) else {
            continue;
        };
        planned.push(Planned { lines, text });
    }
    (!planned.is_empty()).then_some(planned)
}

/// Lignes d'en-tête d'un diff unifié (`diff`, `index`, `---`, `+++`, `@@`,
/// `\ No newline…`) : tout ce qui n'est pas le contenu d'un bloc `@@`, compté
/// d'après son en-tête (une ligne retirée « -- x » devient « --- x » : ce
/// n'est pas un en-tête). Sans aucun bloc (pas un diff unifié) : aucune.
pub fn diff_headers(code: &str) -> Vec<bool> {
    // Lignes restantes du bloc en cours : (avant, après).
    let mut left = (0u64, 0u64);
    let mut hunks = false;
    let headers: Vec<bool> = code
        .lines()
        .map(|line| {
            if left != (0, 0) {
                match line.chars().next() {
                    Some('-') => left.0 = left.0.saturating_sub(1),
                    Some('+') => left.1 = left.1.saturating_sub(1),
                    Some('\\') => return true,
                    _ => left = (left.0.saturating_sub(1), left.1.saturating_sub(1)),
                }
                return false;
            }
            if let Some(counts) = hunk(line) {
                left = counts;
                hunks = true;
            }
            true
        })
        .collect();
    if hunks { headers } else { Vec::new() }
}

/// Lignes d'un bloc d'après son en-tête `@@ -a[,b] +c[,d] @@` : `(b, d)`
/// (1 si absent).
fn hunk(line: &str) -> Option<(u64, u64)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, _) = rest.split_once(" @@")?;
    let count = |range: &str| match range.split_once(',') {
        Some((start, count)) => start.parse::<u64>().ok().and(count.parse().ok()),
        None => range.parse::<u64>().ok().map(|_| 1),
    };
    Some((count(old)?, count(new)?))
}

/// `[a, b]` : deux entiers, sinon `None`.
fn line_pair(lines: &Value) -> Option<(i64, i64)> {
    match lines.as_array()?.as_slice() {
        [a, b] => Some((a.as_i64()?, b.as_i64()?)),
        _ => None,
    }
}

/// `value` ramené dans `[first, last]`.
fn bounded(value: i64, (first, last): (u32, u32)) -> u32 {
    u32::try_from(value.clamp(i64::from(first), i64::from(last))).unwrap_or(first)
}

/// Texte lu d'un segment, `MAX_TEXT` caractères au plus : trop long, il est
/// coupé après sa dernière fin de phrase qui tient, sinon à un blanc, suivi
/// de « … ». `None` : texte vide.
fn spoken(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= MAX_TEXT {
        return Some(text.to_owned());
    }
    // Fin de phrase : ponctuation suivie d'un blanc (jamais au milieu d'un
    // nombre ou d'un nom de fichier), au-delà de la moitié du texte permis :
    // sinon une phrase courte du début resterait seule.
    let sentence = (MAX_TEXT / 2..=MAX_TEXT)
        .rev()
        .find(|&end| matches!(chars[end - 1], '.' | '!' | '?' | '…') && chars[end].is_whitespace());
    if let Some(end) = sentence {
        return Some(chars[..end].iter().collect());
    }
    let end = (1..MAX_TEXT)
        .rev()
        .find(|&end| chars[end].is_whitespace())
        .unwrap_or(MAX_TEXT - 1);
    let cut: String = chars[..end].iter().collect();
    Some(format!("{}…", cut.trim_end()))
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

/// Ce qu'est un diff, pour le modèle : il explique les changements, pas le
/// fichier.
const DIFF: &str = "The code is a unified diff of the user's uncommitted changes. In the \
    code after each line number, a line starting with \"+\" was added, a line starting with \
    \"-\" was removed, and the other lines are unchanged context. Header lines (starting with \
    diff, index, ---, +++ or @@) are not code: never explain them. Explain the changes, not the \
    whole file: each change once, in a segment covering its lines, saying what changed and why \
    it matters.";

/// Consignes du plan, pour du code aux lignes `range`.
pub fn system(kind: Kind, locale: &str, range: (u32, u32)) -> String {
    let what = match kind {
        Kind::File => "The code is a whole file.",
        Kind::Selection => "The code is a part of a file selected by the user.",
        Kind::Diff => DIFF,
    };
    let (first, last) = range;
    let lines = if first == last {
        format!("The code has 1 line, numbered {first}.")
    } else {
        format!(
            "The code has {} lines, numbered {first} to {last}.",
            u64::from(last - first) + 1
        )
    };
    let segments = max_segments(range);
    // i18n-ignore : consignes du modèle, pas un texte d'interface.
    format!(
        "You explain code out loud, as a guided tour, to the developer who wrote it. {what} \
         {lines} Split your explanation into 1 to {segments} segments, in reading order. Each \
         segment covers a range of lines [start, end], using the line numbers shown at the start \
         of each line, with {first} <= start <= end <= {last}, and its text explains those lines \
         in at most {MAX_TEXT} characters. Ranges go forward: each one starts after the start of \
         the previous one, and no range is used twice. Texts are read aloud by a voice: plain \
         spoken sentences, no Markdown, no code, no lists, no line numbers. Write every text in \
         {}.",
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
    server.prefs.context_length.max(needed)
}

/// Refus d'une erreur du moteur : celles d'un modèle distant sont des
/// phrases entières, les autres le détail de `bridge.tour.failed`.
fn refused(error: &LlmError) -> PlanError {
    match error {
        LlmError::RemoteOff
        | LlmError::RemoteUnavailable
        | LlmError::RemoteKey { .. }
        | LlmError::RemoteLimit { .. }
        | LlmError::RemoteUnreachable { .. } => PlanError::Refused(error.message()),
        _ => PlanError::Refused(t!("bridge.tour.failed", detail = error.message())),
    }
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
    let access = &server.access;
    // Réflexion coupée si le modèle sait la couper (plan plus rapide).
    let think = match crate::llm::models::details(access, model).await {
        Ok(details)
            if crate::chat::call::accepted_think(Some(Think::Enabled(false)), &details)
                .is_some() =>
        {
            inference::Think::Off
        }
        _ => inference::Think::Default,
    };
    let range = range(&params.code, params.first_line);
    // Contexte imposé (assez pour le code) ; distant coupé : refus sans
    // rien appeler.
    let options = OpenOptions {
        ctx: Some(context(server, &params.code)),
        ..OpenOptions::default()
    };
    let session = access
        .open(model, options, &CancellationToken::new(), &|| {})
        .await
        .map_err(|error| refused(&error))?;
    let mut request = session.request(vec![
        Message::system(system(params.kind, &params.locale, range)),
        Message::user(user(params)),
    ]);
    request.response_format = Some(ResponseFormat::JsonSchema(schema(range)));
    request.temperature = Some(TEMPERATURE);
    request.think = think;
    let headers = if params.kind == Kind::Diff {
        diff_headers(&params.code)
    } else {
        Vec::new()
    };
    for _ in 0..2 {
        let content = ask(&session, &request).await?;
        if let Some(planned) = parse(&content).and_then(|value| sanitize(&value, range, &headers)) {
            return Ok(planned);
        }
        // i18n-ignore : journal.
        crate::app_log!(
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

/// Une requête (réponse entière, au plus [`TIMEOUT`]) ; rend le contenu
/// du message (la réflexion, séparée par le serveur, n'en fait pas partie).
async fn ask(session: &Session, request: &ChatRequest) -> Result<String, PlanError> {
    let cancel = CancellationToken::new();
    let _cancel_on_drop = cancel.clone().drop_guard();
    let answer = tokio::time::timeout(
        TIMEOUT,
        inference::chat_once(&session.endpoint, request, cancel.clone()),
    )
    .await
    .map_err(|_| inference::InferenceError::Timeout)
    .and_then(|answer| answer)
    .map_err(|error| refused(&session.error(error)))?;
    Ok(answer.content)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RANGE: (u32, u32) = (10, 20);

    fn segment(a: u32, b: u32, text: &str) -> Value {
        json!({ "lines": [a, b], "text": text })
    }

    fn kept(plan: &Value, range: (u32, u32)) -> Option<Vec<(u32, u32)>> {
        sanitize(plan, range, &[]).map(|plan| plan.into_iter().map(|kept| kept.lines).collect())
    }

    #[test]
    fn a_good_plan_is_kept_as_it_is() {
        let good =
            json!({ "segments": [segment(10, 12, "Imports."), segment(12, 20, "The loop.")] });
        assert_eq!(
            sanitize(&good, RANGE, &[]),
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
        // Exactement 16 segments, 700 caractères, même début, plages
        // différentes : tout est gardé.
        let segments: Vec<Value> = (0..16)
            .map(|end| segment(10, 10 + end, &"é".repeat(MAX_TEXT)))
            .collect();
        let full = sanitize(&json!({ "segments": segments }), (10, 40), &[]).unwrap();
        assert_eq!(full.len(), MAX_SEGMENTS);
        assert!(
            full.iter()
                .all(|kept| kept.text.chars().count() == MAX_TEXT)
        );
    }

    /// Un segment fautif est écarté ou réparé, jamais tout le plan.
    #[test]
    fn a_faulty_segment_is_dropped_or_mended_not_the_whole_plan() {
        let plan = json!({
            "segments": [
                // Clé en trop dans le segment : ignorée ; plage à l'envers.
                { "lines": [12, 10], "text": " Imports. ", "note": "y" },
                // Déborde avant le début : ramenée au code.
                segment(5, 13, "Overlaps the start."),
                // Entièrement hors du code : écartés.
                segment(21, 22, "After the end."),
                { "lines": [3, 9], "text": "Before the start." },
                { "lines": [-4, -1], "text": "Negative." },
                // Recule : écarté.
                segment(9, 9, "Backwards and outside."),
                // Lignes mal formées, texte vide ou absent : écartés.
                { "lines": [11], "text": "x" },
                { "lines": [11, "12"], "text": "x" },
                { "lines": [11.5, 12], "text": "x" },
                { "lines": [11, 12, 13], "text": "x" },
                { "text": "x" },
                segment(11, 12, "   "),
                { "lines": [11, 12] },
                { "lines": [11, 12], "text": 3 },
                "not a segment",
                // Plage déjà gardée : écartée.
                segment(10, 12, "Imports again."),
                // Va au-delà de la fin : ramenée à la dernière ligne.
                segment(15, 99, "The rest."),
            ],
            "extra": 1,
        });
        assert_eq!(
            sanitize(&plan, RANGE, &[]),
            Some(vec![
                Planned {
                    lines: (10, 12),
                    text: "Imports.".into()
                },
                Planned {
                    lines: (10, 13),
                    text: "Overlaps the start.".into()
                },
                Planned {
                    lines: (15, 20),
                    text: "The rest.".into()
                },
            ])
        );
        // Plan de gemma3:4b sur un fichier de 22 lignes : deux lignes par
        // segment, jusqu'au-delà de la fin.
        let pairs: Vec<Value> = (0..16)
            .map(|n| segment(2 * n + 1, 2 * n + 2, "Two lines."))
            .collect();
        let expected: Vec<(u32, u32)> = (0..11).map(|n| (2 * n + 1, 2 * n + 2)).collect();
        assert_eq!(kept(&json!({ "segments": pairs }), (1, 22)), Some(expected));
        // Plan de qwen3:8b sur un diff de 25 lignes : reculs, répétitions,
        // lignes au-delà de la fin.
        let diff = json!({ "segments": [
            segment(1, 16, "a"), segment(17, 24, "b"), segment(20, 21, "c"),
            segment(9, 12, "d"), segment(17, 24, "e"), segment(17, 24, "f"),
            segment(25, 25, "g"), segment(26, 26, "h"), segment(38, 38, "i"),
        ] });
        assert_eq!(
            kept(&diff, (1, 25)),
            Some(vec![(1, 16), (17, 24), (20, 21), (25, 25)])
        );
        // Plus de 16 segments utilisables : les 16 premiers.
        let many: Vec<Value> = (1..=30).map(|n| segment(n, n, "One line.")).collect();
        assert_eq!(
            kept(&json!({ "segments": many }), (1, 30)),
            Some((1..=16).map(|n| (n, n)).collect())
        );
    }

    /// Diff de deux fichiers ; une ligne retirée « -- old comment » (SQL)
    /// commence par « --- » sans être un en-tête.
    const DIFF: &str = "diff --git a/x.sql b/x.sql\nindex 1..2 100644\n--- a/x.sql\n+++ b/x.sql\n\
        @@ -1,3 +1,3 @@\n select 1;\n--- old comment\n+-- new comment\n select 2;\n\
        \\ No newline at end of file\ndiff --git a/y b/y\n--- a/y\n+++ b/y\n@@ -5 +5,2 @@\n\
        -a\n+b\n+c\n";

    #[test]
    fn the_header_lines_of_a_diff_are_found_from_its_hunks() {
        let headers: Vec<usize> = diff_headers(DIFF)
            .iter()
            .enumerate()
            .filter_map(|(index, header)| header.then_some(index + 1))
            .collect();
        assert_eq!(headers, [1, 2, 3, 4, 5, 10, 11, 12, 13, 14]);
        assert_eq!(diff_headers(DIFF).len(), 17);
        // Pas un diff unifié (aucun bloc) : rien n'est un en-tête.
        assert!(diff_headers("--- a\n+++ b\nplain text").is_empty());
    }

    /// Diff : un segment perd ses lignes d'en-tête au bord ; s'il n'a que
    /// cela, il est écarté.
    #[test]
    fn a_diff_segment_never_covers_header_lines_only() {
        let plan = json!({ "segments": [
            segment(1, 4, "The header."),
            segment(5, 7, "The first change."),
            segment(9, 12, "End of the first file."),
            segment(10, 14, "Second header."),
            segment(14, 17, "The second change."),
        ] });
        let headers = diff_headers(DIFF);
        assert_eq!(
            sanitize(&plan, (1, 17), &headers)
                .map(|plan| plan.into_iter().map(|kept| kept.lines).collect::<Vec<_>>()),
            Some(vec![(6, 7), (9, 9), (15, 17)])
        );
        // Que des en-têtes : plan inutilisable.
        let header_only = json!({ "segments": [segment(1, 5, "The header.")] });
        assert_eq!(sanitize(&header_only, (1, 17), &headers), None);
        // Hors d'un diff, ces lignes sont du code.
        assert_eq!(kept(&header_only, (1, 17)), Some(vec![(1, 5)]));
    }

    #[test]
    fn a_plan_with_nothing_left_is_unusable() {
        let unusable = [
            json!({ "segments": [] }),
            json!({ "segments": [segment(21, 22, "x"), segment(1, 9, "y")] }),
            json!({ "segments": [segment(10, 11, "  ")] }),
            json!({ "segments": "x" }),
            json!({ "plan": [segment(10, 11, "x")] }),
            json!([segment(10, 11, "x")]),
            json!(null),
        ];
        for plan in unusable {
            assert_eq!(sanitize(&plan, RANGE, &[]), None, "{plan}");
        }
    }

    #[test]
    fn a_long_text_is_cut_after_a_sentence_or_a_word() {
        // Coupé après la dernière phrase qui tient.
        let sentence = format!("{}Second one goes on.", "Word. ".repeat(116));
        let cut = spoken(&sentence).unwrap();
        assert_eq!(cut, "Word. ".repeat(116).trim_end());
        // Pas de fin de phrase : coupé à un blanc, suivi de « … ».
        let words = "word ".repeat(200);
        let cut = spoken(&words).unwrap();
        assert!(cut.ends_with("word…"), "{cut}");
        assert!(cut.chars().count() <= MAX_TEXT);
        // Un seul mot : coupé net.
        let cut = spoken(&"é".repeat(MAX_TEXT + 5)).unwrap();
        assert_eq!(cut.chars().count(), MAX_TEXT);
        assert!(cut.ends_with('…'));
        // Une fin de phrase trop tôt n'est pas gardée : coupé à un blanc.
        let early = format!("Note. {}", "word ".repeat(200));
        let cut = spoken(&early).unwrap();
        assert!(cut.chars().count() > MAX_TEXT / 2, "{cut}");
        assert!(cut.ends_with("word…"), "{cut}");
        // Un point au milieu d'un nombre n'est pas une fin de phrase.
        let number = format!("{}v1.2 b", "a ".repeat(349));
        assert!(spoken(&number).unwrap().ends_with("a…"));
        assert_eq!(spoken(" Short. "), Some("Short.".into()));
        assert_eq!(spoken(" \n "), None);
    }

    #[test]
    fn the_schema_bounds_the_line_numbers_to_the_code() {
        let bounded = schema((41, 62));
        let segments = &bounded["properties"]["segments"];
        let lines = &segments["items"]["properties"]["lines"]["items"];
        assert_eq!(lines["minimum"], 41);
        assert_eq!(lines["maximum"], 62);
        assert_eq!(segments["maxItems"], MAX_SEGMENTS);
        // Code plus court que 16 lignes : un segment par ligne au plus.
        assert_eq!(schema((7, 9))["properties"]["segments"]["maxItems"], 3);
        assert_eq!(schema((7, 7))["properties"]["segments"]["maxItems"], 1);
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
        let lines = range(&params.code, params.first_line);
        assert_eq!(lines, (41, 42));
        assert_eq!(
            user(&params),
            "File: src/main.rs (rust)\n\n   41 | fn main() {\n   42 | }\n"
        );
        let told = system(params.kind, &params.locale, lines);
        assert!(told.ends_with("in French."));
        assert!(
            told.contains("The code has 2 lines, numbered 41 to 42."),
            "{told}"
        );
        assert!(told.contains("41 <= start <= end <= 42"), "{told}");
        assert!(told.contains("1 to 2 segments"), "{told}");
        assert!(!told.contains("diff"), "{told}");
        assert!(system(Kind::File, "es", (1, 1)).contains("The code has 1 line, numbered 1."));
        assert!(system(Kind::Diff, "es", RANGE).ends_with("in Spanish."));
        assert!(system(Kind::File, "de", RANGE).ends_with("in English."));
    }

    /// Diff : le modèle sait lire « + », « - » et les en-têtes, et explique
    /// les changements, pas le fichier.
    #[test]
    fn the_diff_prompt_explains_the_changes_only() {
        let told = system(Kind::Diff, "en", (1, 25));
        for part in [
            "a line starting with \"+\" was added",
            "a line starting with \"-\" was removed",
            "Header lines (starting with diff, index, ---, +++ or @@) are not code",
            "Explain the changes, not the whole file: each change once",
            "The code has 25 lines, numbered 1 to 25.",
        ] {
            assert!(told.contains(part), "{part}: {told}");
        }
    }

    #[test]
    fn a_json_object_is_found_in_the_answer() {
        assert_eq!(
            parse("Here:\n{\"segments\": []}\n"),
            Some(json!({ "segments": [] }))
        );
        assert_eq!(parse("no json"), None);
    }

    /// Visite contre le faux moteur.
    mod engine {
        use super::*;
        use crate::engine::{KeepAlive, LoadOptions};
        use crate::llm::testing::{FakeBench, RealBench, Role};
        use crate::store::settings::SettingsPatch;

        const TOUR: &str = "test/Tour-GGUF:Q4_K_M";
        const THINKER: &str = "test/Thinker-GGUF:Q4_K_M";

        fn params(code: &str) -> PlanParams {
            PlanParams {
                model: None,
                locale: "fr".into(),
                path: "src/main.rs".into(),
                language: "rust".into(),
                code: code.into(),
                first_line: 10,
                kind: Kind::File,
            }
        }

        /// Corps de chat reçus par le processus de `id` (prêt avec ce
        /// contexte : rien n'est relancé).
        async fn chats(bench: &FakeBench, id: &str, ctx: u32) -> Vec<Value> {
            let options = LoadOptions {
                ctx: Some(ctx),
                keep_alive: KeepAlive::Default,
                cpu_only: false,
            };
            assert!(bench.engine().is_ready(id, options).await, "{id} at {ctx}");
            let lease = bench.engine().ensure(id, options).await.unwrap();
            let all: Value = reqwest::get(format!("{}/fake/requests", lease.endpoint.base_url))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            drop(lease);
            all.as_array()
                .unwrap()
                .iter()
                .filter(|request| request["path"] == "/v1/chat/completions")
                .map(|request| request["body"].clone())
                .collect()
        }

        /// Sortie JSON contrainte par le schéma (lignes bornées au code),
        /// température 0,3, contexte imposé assez grand pour le code, plan
        /// gardé ; réflexion coupée pour un modèle qui la coupe.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_tour_is_planned_under_a_json_schema() {
            let bench = FakeBench::new(&[(TOUR, "gpu-tour"), (THINKER, "gpu-tourthink")]);
            let server = Server::current(&bench.access).await.unwrap();
            let code = "fn main() {\n    run();\n}";
            let planned = plan(&server, TOUR, &params(code)).await.unwrap();
            assert_eq!(
                planned,
                [
                    Planned {
                        lines: (10, 10),
                        text: "This line starts it.".into()
                    },
                    Planned {
                        lines: (10, 12),
                        text: "And this is the whole.".into()
                    },
                ]
            );
            let sent = chats(&bench, TOUR, 8192).await;
            assert_eq!(sent.len(), 1);
            let body = &sent[0];
            assert_eq!(body["response_format"]["type"], "json_schema");
            assert_eq!(
                body["response_format"]["json_schema"]["schema"],
                schema((10, 12))
            );
            assert_eq!(body["temperature"], 0.3f32);
            assert_eq!(body["messages"][0]["role"], "system");
            assert!(
                body["messages"][1]["content"]
                    .as_str()
                    .unwrap()
                    .contains("   11 |     run();")
            );
            // Sans réflexion : rien de demandé au gabarit.
            assert!(body.get("chat_template_kwargs").is_none(), "{body}");
            // Contexte réglé plus grand : le réglage.
            bench.set(SettingsPatch {
                context_length: Some(16_384),
                ..Default::default()
            });
            let server = Server::current(&bench.access).await.unwrap();
            plan(&server, THINKER, &params(code)).await.unwrap();
            let sent = chats(&bench, THINKER, 16_384).await;
            assert_eq!(
                sent[0]["chat_template_kwargs"]["enable_thinking"], false,
                "{}",
                sent[0]
            );
            bench.shutdown().await;
        }

        /// Plan inutilisable : redemandé une fois, puis refusé ; refus
        /// d'emblée (paramètres, modèle) sans rien envoyer.
        #[tokio::test(flavor = "multi_thread")]
        async fn an_unusable_plan_is_asked_twice_then_refused() {
            let bench = FakeBench::new(&[(TOUR, "gpu-tour")]);
            let server = Server::current(&bench.access).await.unwrap();
            assert_eq!(
                plan(&server, TOUR, &params("UNUSABLE\nx")).await,
                Err(PlanError::Refused(t!("bridge.tour.invalid")))
            );
            assert_eq!(chats(&bench, TOUR, 8192).await.len(), 2);
            assert_eq!(
                plan(&server, TOUR, &params("  ")).await,
                Err(PlanError::Params)
            );
            assert_eq!(
                plan(&server, " ", &params("x")).await,
                Err(PlanError::Refused(t!("bridge.tour.noModel")))
            );
            // Modèle distant, réglage coupé : refus sans rien appeler.
            assert_eq!(
                plan(&server, "@remote/mistral/x", &params("x")).await,
                Err(PlanError::Refused(LlmError::RemoteOff.message()))
            );
            // Absent : l'erreur du moteur, dans le texte de la visite.
            let Err(PlanError::Refused(message)) =
                plan(&server, "test/Absent-GGUF:Q4_K_M", &params("x")).await
            else {
                panic!("expected a refusal");
            };
            assert!(message.contains("test/Absent-GGUF:Q4_K_M"), "{message}");
            assert_eq!(bench.engine().running().len(), 1);
            bench.shutdown().await;
        }

        /// Contrat : un vrai plan de Qwen3 0.6B sur le vrai `llama-server`
        /// (sortie contrainte par le schéma, lignes dans le code).
        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "vrai llama-server : CHATONLM_LLAMA_TEST_BIN et un modèle Text"]
        async fn contract_a_tour_from_the_real_engine() {
            let Some(bench) = RealBench::new(Role::Text) else {
                return;
            };
            let server = Server::current(&bench.access).await.unwrap();
            let code = "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn main() {\n    println!(\"{}\", add(1, 2));\n}";
            let mut asked = params(code);
            asked.locale = "en".into();
            let planned = plan(&server, &bench.id, &asked).await.unwrap();
            eprintln!("tour: {planned:?}");
            assert!(!planned.is_empty());
            assert!(
                planned
                    .iter()
                    .all(|segment| segment.lines.0 >= 10 && segment.lines.1 <= 16)
            );
            let running = bench.access.engine().running();
            assert_eq!(running[0].context_length, 8192, "{running:?}");
            bench.shutdown().await;
        }
    }
}
