//! Cours de ChatonLM (onglet Study) : réponses déterministes des modèles
//! `chatonlm-study*` à `/api/chat`.
//!
//! La partie demandée est reconnue aux propriétés du schéma `format` : plan
//! (`levels`, autant de paliers que `minItems`), leçon (`sections`),
//! questions (`q1`… selon leur schéma, `explanation` en tête pour un nouvel
//! essai), vérification (`answer`, `order`, `answers` : une paire
//! d'association jugée par ligne). Les textes sont dans la langue
//! nommée par les consignes (« French », « Spanish », anglais sinon). Sans
//! `format` : réponse en flux Markdown (explication ou réponse à une
//! question).
//!
//! - `chatonlm-study` : bonne option de chaque choix « Right answer », les
//!   autres « Wrong answer… » ; trou dont la réponse est « right » (avec la
//!   variante proche « rigth » et la réponse fausse « wrong ») ; ordre
//!   « First step », « Second step », « Third step » ; association « One » =
//!   « 1 »… ; leçon avec une formule `$\frac{1}{2}$` écrite comme un petit
//!   modèle (barre oblique non doublée : saut de page une fois lue) et un
//!   bloc de code ; vérification qui répond juste ;
//! - `chatonlm-study-slow` : comme `chatonlm-study`, chaque réponse JSON après
//!   [`STUDY_SLOW`], flux de 4 caractères toutes les 150 ms ;
//! - `chatonlm-study-unclear` : plan `unclear` (deux sens) tant que le
//!   message de l'apprenant n'a pas de « Meaning: » ;
//! - `chatonlm-study-invalid` : questions toujours rejetées par le garde-fou.
//!
//! Selon le sujet (plan) : « Short… » donne des leçons trop courtes ;
//! « Disputed… » des paliers dont les choix sont contestés par la
//! vérification (elle répond une mauvaise option) ; « Wrong… » des leçons
//! où la relecture (`errors`) trouve une vraie erreur, parmi trois fausses
//! alertes (la leçon réécrite a un piège « (corrected) ») ; « Unreviewed… »
//! des leçons dont la relecture échoue (500) ; « Fragile… » des questions
//! toujours rejetées sauf une demande de choix seulement ; « Hiccup… » un
//! premier plan en erreur 500 (passagère) ; « Runaway… » un plan dont
//! le `scope` remplit sa limite `maxLength` (emballement). Ailleurs, la
//! relecture ne trouve rien.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};

use super::{DisconnectGuard, End, Journal, json as json_response, ndjson, reply_lines};

/// Attente de chaque réponse JSON de `chatonlm-study-slow`.
pub const STUDY_SLOW: Duration = Duration::from_millis(1000);

/// Modèles des cours.
pub const STUDY_MODELS: &[&str] = &[
    "chatonlm-study",
    "chatonlm-study-slow",
    "chatonlm-study-unclear",
    "chatonlm-study-invalid",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    En,
    Fr,
    Es,
}

fn lang_of(system: &str) -> Lang {
    if system.contains("French") {
        Lang::Fr
    } else if system.contains("Spanish") {
        Lang::Es
    } else {
        Lang::En
    }
}

/// Phrase d'une leçon (une vingtaine de mots, mots outils de la langue).
fn sentence(lang: Lang) -> &'static str {
    match lang {
        Lang::En => {
            "The leaf uses the light of the sun and the water from the soil to make the sugar that it needs every day."
        }
        Lang::Fr => {
            "La feuille utilise la lumière du soleil et l'eau du sol pour fabriquer le sucre dont elle a besoin chaque jour."
        }
        Lang::Es => {
            "La hoja usa la luz del sol y el agua del suelo para fabricar el azúcar que necesita cada día."
        }
    }
}

fn explanation(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "This is the right answer because the lesson says so.",
        Lang::Fr => "C'est la bonne réponse parce que la leçon le dit.",
        Lang::Es => "Es la respuesta correcta porque la lección lo dice.",
    }
}

fn statement(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "The leaf needs the light of the sun.",
        Lang::Fr => "La feuille a besoin de la lumière du soleil.",
        Lang::Es => "La hoja necesita la luz del sol.",
    }
}

/// Plan de `count` paliers ; `unclear` : sujet ambigu.
fn plan(lang: Lang, topic: &str, count: usize, unclear: bool) -> Value {
    let lower = topic.to_lowercase();
    let kind = ["math", "code", "language", "history", "skill"]
        .into_iter()
        .find(|kind| lower.contains(kind))
        .unwrap_or("concept");
    json!({
        "unclear": unclear,
        "question": if unclear { "Which Java do you mean?" } else { "" },
        "options": if unclear { json!(["The island", "The programming language"]) } else { json!([]) },
        "title": format!("**{topic}**"),
        "kind": kind,
        "summary": sentence(lang),
        "objectives": ["Objective 1", "Objective 2", "Objective 3"],
        "levels": (1..=count).map(|n| json!({
            "title": match ["Disputed", "Wrong", "Unreviewed", "Fragile"].into_iter().find(|word| topic.starts_with(word)) {
                Some(word) => format!("{word} level {n}"),
                None => format!("Level {n}"),
            },
            "goal": sentence(lang),
            "points": [format!("Point {n}a"), format!("Point {n}b")],
        })).collect::<Vec<_>>(),
        "scope": "",
        "further": ["**Further 1**", "Further 2", "Further 3"],
    })
}

/// Leçon ; `short` : sections trop courtes pour ChatonLM ; `corrected` :
/// réécrite après une relecture.
fn lesson(lang: Lang, title: &str, short: bool, corrected: bool) -> Value {
    let body = if short {
        sentence(lang).to_owned()
    } else {
        format!(
            "{0} {0} {0} $\\frac{{1}}{{2}}$ = 0,5.\n\n```python\nprint(1 / 2)\n```",
            sentence(lang)
        )
    };
    json!({
        "hook": format!("{title}?"),
        "sections": [
            {"heading": "## Part one", "body": body},
            {"heading": "**Part two**", "body": body},
            {"heading": "Part three", "body": body},
        ],
        "example": {"problem": sentence(lang), "solution": sentence(lang)},
        "keyPoints": [format!("Key point of {title}."), "Second key point."],
        "pitfall": if corrected { format!("{} (corrected)", sentence(lang)) } else { sentence(lang).to_owned() },
    })
}

/// Question du type décrit par `schema` ; `invalid` : toujours rejetée ;
/// `disputed` : choix contesté par la vérification.
fn question(lang: Lang, schema: &Value, invalid: bool, disputed: bool) -> Value {
    let properties = &schema["properties"];
    let explanation = explanation(lang);
    if properties.get("options").is_some() {
        json!({
            "prompt": if disputed { "Which answer is right? (disputed)" } else { "Which answer is right?" },
            "options": ["Right answer", "Wrong answer one", "Wrong answer two"],
            "answer": if invalid { 7 } else { 0 },
            "explanation": explanation,
        })
    } else if properties.get("statement").is_some() {
        json!({
            "statement": if invalid { "" } else { statement(lang) },
            "answer": true,
            "explanation": explanation,
        })
    } else if properties.get("answers").is_some() {
        json!({
            "prompt": if invalid { "No hole here." } else { "The ___ answer." },
            "answers": ["right", "rigth", "wrong"],
            "explanation": explanation,
        })
    } else if properties.get("items").is_some() {
        let items = if invalid {
            json!(["First step", "First step"])
        } else {
            json!(["First step", "Second step", "Third step"])
        };
        json!({"prompt": "Put the steps in order.", "items": items, "explanation": explanation})
    } else {
        let third = if invalid { "1" } else { "3" };
        json!({
            "prompt": "Match the words.",
            "pairs": [
                {"left": "One", "right": "1"},
                {"left": "Two", "right": "2"},
                {"left": "Three", "right": third},
            ],
            "explanation": explanation,
        })
    }
}

/// Relecture d'une leçon : rien, sauf pour un palier « Wrong… » : une
/// vraie erreur (citée avec des `$` que la leçon n'a plus) et trois fausses
/// alertes (citation absente, correction égale à la citation, correction
/// qui la reprend).
fn review(user: &str) -> Value {
    if !user.contains("Key point of Wrong") {
        return json!({"errors": []});
    }
    json!({"errors": [
        {"quote": "$\\frac{1}{2}$ = 0,5", "correction": "$\\frac{1}{2}$ = 0.5 with a decimal point"},
        {"quote": "Plants eat soil.", "correction": "Plants make sugar."},
        {"quote": "Second key point.", "correction": "second key point"},
        {"quote": "Second key point", "correction": "Second key point (correct)"},
    ]})
}

/// Rang d'un élément d'ordre (« First step » = 0…).
fn ordinal(item: &str) -> usize {
    ["First", "Second", "Third", "Fourth", "Fifth"]
        .iter()
        .position(|word| item.contains(word))
        .unwrap_or(usize::MAX)
}

/// Vérification : la bonne réponse, lue dans la question posée.
fn check(schema: &Value, user: &str) -> Value {
    let properties = &schema["properties"];
    // Lignes « N. texte ».
    let numbered: Vec<(usize, &str)> = user
        .lines()
        .filter_map(|line| {
            let (number, text) = line.split_once(". ")?;
            Some((number.parse().ok()?, text))
        })
        .collect();
    if properties.get("answers").is_some() {
        // Association : « N. gauche → droite », vraie si la paire est
        // celle écrite par ce serveur (One → 1…).
        let right_pair = |text: &str| {
            text.split_once(" → ").is_some_and(|(left, right)| {
                matches!((left, right), ("One", "1") | ("Two", "2") | ("Three", "3"))
            })
        };
        return json!({"answers": numbered.iter().map(|(_, text)| right_pair(text)).collect::<Vec<_>>()});
    }
    if properties.get("order").is_some() {
        let mut order = numbered.clone();
        order.sort_by_key(|(_, text)| ordinal(text));
        return json!({"order": order.iter().map(|(n, _)| *n).collect::<Vec<_>>()});
    }
    match properties["answer"]["type"].as_str() {
        Some("integer") => {
            let wanted = if user.contains("(disputed)") {
                "Wrong"
            } else {
                "Right"
            };
            let answer = numbered
                .iter()
                .find(|(_, text)| text.contains(wanted))
                .map_or(0, |(n, _)| *n);
            json!({"answer": answer})
        }
        Some("boolean") => json!({"answer": true}),
        _ => json!({"answer": "right"}),
    }
}

/// Réponse de `/api/chat` d'un modèle des cours.
pub async fn reply(journal: Arc<Journal>, model: &str, request: &Value) -> Response {
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let system = messages
        .first()
        .and_then(|message| message["content"].as_str())
        .unwrap_or_default()
        .to_owned();
    let user = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .and_then(|message| message["content"].as_str())
        .unwrap_or_default()
        .to_owned();
    let lang = lang_of(&system);
    let slow = model == "chatonlm-study-slow";
    let schema = &request["format"];
    if !schema.is_object() || request["stream"] != Value::Bool(false) {
        let text = if system.contains("Explain this section again") {
            format!(
                "**Simpler:** {} Half is $\\frac{{1}}{{2}}$, it costs $5.",
                sentence(lang)
            )
        } else {
            format!("**Answer:** {}", sentence(lang))
        };
        let (size, delay) = if slow { (4, 150) } else { (12, 10) };
        return ndjson(
            journal,
            reply_lines(model, &text, size),
            Duration::from_millis(delay),
            End::Close,
        );
    }
    if slow {
        let mut guard = DisconnectGuard::new(journal.clone());
        tokio::time::sleep(STUDY_SLOW).await;
        guard.finish();
    }
    let properties = &schema["properties"];
    let invalid = model == "chatonlm-study-invalid";
    let content = if properties.get("levels").is_some() {
        let count = properties["levels"]["minItems"].as_u64().unwrap_or(4) as usize;
        let topic_line = user.lines().next().unwrap_or_default();
        let topic = topic_line.strip_prefix("Topic: ").unwrap_or(topic_line);
        // Erreur passagère : le premier plan d'un sujet « Hiccup… » répond
        // 500 (le serveur garde la requête au journal avant de répondre).
        if topic.starts_with("Hiccup") {
            let seen = journal
                .chats
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.to_string().contains("Topic: Hiccup"))
                .count();
            if seen == 1 {
                return json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    r#"{"error":"llama runner process has terminated"}"#.into(),
                );
            }
        }
        let meaning = user.lines().find_map(|line| line.strip_prefix("Meaning: "));
        let title = match meaning {
            Some(meaning) => format!("{topic} ({meaning})"),
            None => topic.to_owned(),
        };
        let unclear = model == "chatonlm-study-unclear" && meaning.is_none();
        let mut plan = plan(lang, &title, count, unclear);
        // Emballement : « scope » jusqu'à sa limite (`maxLength`), comme un
        // petit modèle qui se répète jusqu'à la coupure.
        if topic.starts_with("Runaway")
            && let Some(max) = properties["scope"]["maxLength"].as_u64()
        {
            plan["scope"] = json!("x".repeat(max as usize));
        }
        plan.to_string()
    } else if properties.get("sections").is_some() {
        let title = user.split(": ").nth(1).unwrap_or("Level");
        let short = system.contains("Course: \"Short");
        let corrected = system.contains("A reviewer found these errors");
        // Comme un petit modèle : `\frac` écrit avec une seule barre.
        lesson(lang, title, short, corrected)
            .to_string()
            .replace("\\\\frac", "\\frac")
    } else if properties.get("q1").is_some() {
        let mut answer = serde_json::Map::new();
        if properties.get("explanation").is_some() {
            answer.insert(
                "explanation".into(),
                json!(format!("{0} {0} $x^2$", sentence(lang))),
            );
        }
        let disputed = system.contains("Disputed");
        // « Fragile… » : questions toujours rejetées, sauf une demande de
        // questions à choix seulement (dernière chance de ChatonLM).
        let only_choices = (1..)
            .map_while(|index| properties.get(format!("q{index}")))
            .all(|question| question["properties"].get("options").is_some());
        let fragile = system.contains("Fragile level") && !only_choices;
        let mut index = 1;
        while let Some(question_schema) = properties.get(format!("q{index}")) {
            answer.insert(
                format!("q{index}"),
                question(lang, question_schema, invalid || fragile, disputed),
            );
            index += 1;
        }
        Value::Object(answer).to_string()
    } else if properties.get("errors").is_some() {
        if user.contains("Key point of Unreviewed") {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                r#"{"error":"review failed"}"#.into(),
            );
        }
        review(&user).to_string()
    } else {
        check(schema, &user).to_string()
    };
    json_response(
        StatusCode::OK,
        json!({
            "model": model,
            "created_at": "2026-10-07T00:00:00Z",
            "message": {"role": "assistant", "content": content},
            "done": true,
            "done_reason": "stop",
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checker_reads_the_numbered_question() {
        let integer = json!({"properties": {"answer": {"type": "integer"}}});
        assert_eq!(
            check(
                &integer,
                "Which?\n0. Wrong a\n1. Wrong b\n2. Right answer\nGive the number."
            ),
            json!({"answer": 2})
        );
        let order = json!({"properties": {"order": {"type": "array"}}});
        assert_eq!(
            check(
                &order,
                "Sort\n0. Third step\n1. First step\n2. Second step\nGive."
            ),
            json!({"order": [1, 2, 0]})
        );
        let pairs = json!({"properties": {"answers": {"type": "array"}}});
        assert_eq!(
            check(
                &pairs,
                "Match.\nFor each...\n0. One → 1\n1. Two → 3\n2. Three → 3\nAnswer..."
            ),
            json!({"answers": [true, false, true]})
        );
        let text = json!({"properties": {"answer": {"type": "string"}}});
        assert_eq!(check(&text, "x ___"), json!({"answer": "right"}));
        assert_eq!(
            check(
                &integer,
                "Which answer is right? (disputed)\n0. Right answer\n1. Wrong answer one"
            ),
            json!({"answer": 1})
        );
    }

    #[test]
    fn lessons_carry_a_formula_read_as_a_form_feed() {
        let content = lesson(Lang::En, "Level 1", false, false)
            .to_string()
            .replace("\\\\frac", "\\frac");
        let value: Value = serde_json::from_str(&content).unwrap();
        assert!(
            value["sections"][0]["body"]
                .as_str()
                .unwrap()
                .contains("$\x0crac{1}{2}$")
        );
        assert!(
            value["sections"][0]["body"]
                .as_str()
                .unwrap()
                .contains("```python")
        );
    }
}
