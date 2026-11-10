//! Cours de ChatonLM (onglet Study) : réponses déterministes des modèles
//! `chatonlm-study*` (repris des modèles du même nom du faux Ollama).
//!
//! La partie demandée est reconnue aux propriétés du schéma
//! (`response_format.json_schema.schema`) : plan (`levels`, autant de
//! paliers que `minItems`), leçon (`sections`), questions (`q1`… selon leur
//! schéma, `explanation` en tête pour un nouvel essai), relecture
//! (`errors`), vérification (`answer`, `order`, `answers` : une paire
//! d'association jugée par ligne). Les textes sont dans la langue nommée
//! par les consignes (« French », « Spanish », anglais sinon). Sans schéma :
//! réponse en flux Markdown (explication ou réponse à une question).
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
//! - `chatonlm-study-invalid` : questions toujours rejetées par le garde-fou ;
//! - `chatonlm-study-stall` : chaque réponse après [`STUDY_STALL`] (délais
//!   des essais dépassés).
//!
//! Selon le sujet (plan) : « Short… » donne des leçons trop courtes ;
//! « Disputed… » des paliers dont les choix sont contestés par la
//! vérification (elle répond une mauvaise option) ; « Wrong… » des leçons
//! où la relecture (`errors`) trouve une vraie erreur, parmi trois fausses
//! alertes (la leçon réécrite a un piège « (corrected) ») ; « Unreviewed… »
//! des leçons dont la relecture échoue (500) ; « Fragile… » des questions
//! toujours rejetées sauf une demande de choix seulement ; « Hiccup… » un
//! premier plan en erreur 500 (passagère) ; « Broken… » un plan toujours en
//! erreur 500 ; « Runaway… » un plan dont le `scope` remplit sa limite
//! `maxLength` (emballement). Ailleurs, la relecture ne trouve rien.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use super::super::{ChatCall, Custom, Script, Step};
use crate::scenario::text_of;

/// Attente de chaque réponse JSON de `chatonlm-study-slow`.
pub const STUDY_SLOW: Duration = Duration::from_millis(1000);
/// Attente de chaque réponse de `chatonlm-study-stall`.
pub const STUDY_STALL: Duration = Duration::from_secs(3);

pub const STUDY: Custom = custom("chatonlm-study", study);
pub const STUDY_SLOW_MODEL: Custom = custom("chatonlm-study-slow", study_slow);
pub const STUDY_UNCLEAR: Custom = custom("chatonlm-study-unclear", study_unclear);
pub const STUDY_INVALID: Custom = custom("chatonlm-study-invalid", study_invalid);
pub const STUDY_STALL_MODEL: Custom = custom("chatonlm-study-stall", study_stall);

const fn custom(name: &'static str, chat: fn(&ChatCall) -> Script) -> Custom {
    Custom {
        name,
        chat,
        infill: None,
        load_ms: None,
        fail_load: false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Model {
    Plain,
    Slow,
    Unclear,
    Invalid,
}

fn study(call: &ChatCall) -> Script {
    reply(Model::Plain, call.body)
}

fn study_slow(call: &ChatCall) -> Script {
    reply(Model::Slow, call.body)
}

fn study_unclear(call: &ChatCall) -> Script {
    reply(Model::Unclear, call.body)
}

fn study_invalid(call: &ChatCall) -> Script {
    reply(Model::Invalid, call.body)
}

fn study_stall(call: &ChatCall) -> Script {
    let mut script = vec![Step::Wait(STUDY_STALL)];
    script.extend(reply(Model::Plain, call.body));
    script
}

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

/// Erreur 500 du serveur, au format de `llama-server`.
fn server_error(message: &str) -> Step {
    Step::Fail {
        status: 500,
        body: json!({ "error": { "code": 500, "message": message, "type": "server_error" } }),
    }
}

/// Texte en morceaux de `size` caractères, `delay` entre deux.
fn pieces(text: &str, size: usize, delay: Duration) -> Script {
    let characters: Vec<char> = text.chars().collect();
    let mut script = Vec::new();
    for (index, chunk) in characters.chunks(size).enumerate() {
        if index > 0 && !delay.is_zero() {
            script.push(Step::Wait(delay));
        }
        script.push(Step::Content(chunk.iter().collect()));
    }
    script
}

/// Plans « Hiccup… » demandés dans ce processus.
static HICCUPS: AtomicUsize = AtomicUsize::new(0);

/// Réponse d'un modèle des cours.
fn reply(model: Model, request: &Value) -> Script {
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let system = messages.first().map(text_of).unwrap_or_default();
    let user = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .map(text_of)
        .unwrap_or_default();
    let lang = lang_of(&system);
    let slow = model == Model::Slow;
    let schema = &request["response_format"]["json_schema"]["schema"];
    if !schema.is_object() {
        let text = if system.contains("Explain this section again") {
            format!(
                "**Simpler:** {} Half is $\\frac{{1}}{{2}}$, it costs $5.",
                sentence(lang)
            )
        } else {
            format!("**Answer:** {}", sentence(lang))
        };
        return if slow {
            pieces(&text, 4, Duration::from_millis(150))
        } else {
            pieces(&text, 12, Duration::from_millis(10))
        };
    }
    let mut script = Vec::new();
    if slow {
        script.push(Step::Wait(STUDY_SLOW));
    }
    let properties = &schema["properties"];
    let invalid = model == Model::Invalid;
    let content = if properties.get("levels").is_some() {
        let count = properties["levels"]["minItems"].as_u64().unwrap_or(4) as usize;
        let topic_line = user.lines().next().unwrap_or_default();
        let topic = topic_line.strip_prefix("Topic: ").unwrap_or(topic_line);
        // Erreur passagère : le premier plan d'un sujet « Hiccup… » répond
        // 500.
        if topic.starts_with("Hiccup") && HICCUPS.fetch_add(1, Ordering::SeqCst) == 0 {
            script.push(server_error("llama runner process has terminated"));
            return script;
        }
        if topic.starts_with("Broken") {
            script.push(server_error("the model failed"));
            return script;
        }
        let meaning = user.lines().find_map(|line| line.strip_prefix("Meaning: "));
        let title = match meaning {
            Some(meaning) => format!("{topic} ({meaning})"),
            None => topic.to_owned(),
        };
        let unclear = model == Model::Unclear && meaning.is_none();
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
            script.push(server_error("review failed"));
            return script;
        }
        review(&user).to_string()
    } else {
        check(schema, &user).to_string()
    };
    script.push(Step::Content(content));
    script
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(script: &Script) -> String {
        script
            .iter()
            .filter_map(|step| match step {
                Step::Content(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn asked(system: &str, user: &str, schema: Value) -> Value {
        json!({
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
            "response_format": {"type": "json_schema", "json_schema": {"name": "response", "schema": schema}},
        })
    }

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
    }

    #[test]
    fn parts_are_told_apart_by_their_schema() {
        let plan_schema =
            json!({"properties": {"levels": {"minItems": 2}, "scope": {"maxLength": 5}}});
        let request = asked("Write in French.", "Topic: Leaves", plan_schema.clone());
        let plan: Value = serde_json::from_str(&content(&reply(Model::Plain, &request))).unwrap();
        assert_eq!(plan["levels"].as_array().unwrap().len(), 2);
        assert!(plan["summary"].as_str().unwrap().starts_with("La feuille"));
        let runaway = asked("", "Topic: Runaway", plan_schema.clone());
        let plan: Value = serde_json::from_str(&content(&reply(Model::Plain, &runaway))).unwrap();
        assert_eq!(plan["scope"], "xxxxx");
        let broken = reply(Model::Plain, &asked("", "Topic: Broken", plan_schema));
        assert!(matches!(broken[0], Step::Fail { status: 500, .. }));
        let lesson = asked(
            "",
            "Write level 1: Light",
            json!({"properties": {"sections": {}}}),
        );
        assert!(content(&reply(Model::Plain, &lesson)).contains("$\\frac{1}{2}$"));
        // Sans schéma : Markdown en morceaux, plus lents pour `-slow`.
        let free =
            json!({"messages": [{"role": "system", "content": "Explain this section again"}]});
        let fast = reply(Model::Plain, &free);
        assert!(content(&fast).starts_with("**Simpler:**"));
        let slow = reply(Model::Slow, &free);
        assert!(slow.contains(&Step::Wait(Duration::from_millis(150))));
        assert!(
            reply(Model::Slow, &lesson)
                .first()
                .is_some_and(|step| *step == Step::Wait(STUDY_SLOW))
        );
    }
}
