//! Garde-fous des réponses du modèle : plan, leçon, questions (lues dans
//! leur JSON, vérifiées, puis mélangées de façon reproductible) et accord
//! de l'auto-vérification.

use serde_json::Value;

use super::data::{QType, StoredQuestion};
use super::grade::{self, Match};
use super::prompts::{self, LESSON_WORDS};
use super::repair::{inline_math, plain_heading};
use super::{
    AnswerValue, Clarify, Lesson, LessonSection, Plan, PlanLevel, QuestionBody, StudyKind,
    WorkedExample,
};
use crate::i18n::Lang;
use crate::proofread::guard;
use crate::proofread::language::{self, Language};

fn language_of(lang: Lang) -> Language {
    match lang {
        Lang::En => Language::English,
        Lang::Es => Language::Spanish,
        Lang::Fr => Language::French,
    }
}

/// Texte sans blocs de code, code en ligne ni formules (mots outils d'un
/// langage de programmation : `for`, `in`, `if`…).
fn prose(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut in_block = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_block = !in_block;
            continue;
        }
        if in_block {
            continue;
        }
        let mut in_code = false;
        let mut in_math = false;
        for c in line.chars() {
            match c {
                '`' if !in_math => in_code = !in_code,
                '$' if !in_code => in_math = !in_math,
                c if !in_code && !in_math => kept.push(c),
                _ => {}
            }
        }
        kept.push('\n');
    }
    kept
}

/// Texte écrit dans la langue du cours : aucune autre langue reconnue ; pour
/// un cours de langue (les deux langues s'y mêlent), des mots outils de la
/// langue du cours (deux pour un texte long, un sinon, ou langue non
/// reconnue pour un texte court).
pub fn language_ok(text: &str, lang: Lang, kind: StudyKind, long: bool) -> bool {
    let text = prose(text);
    let course = language_of(lang);
    let detected = language::detect(&text);
    if kind != StudyKind::Language {
        return detected.is_none_or(|found| found == course);
    }
    detected == Some(course)
        || language::count(&text, course) >= if long { 2 } else { 1 }
        || (!long && detected.is_none())
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().trim().to_owned()
}

/// Textes non vides d'un tableau, sans Markdown autour (titres, notions,
/// sujets « Go further », sens d'un sujet ambigu : textes simples).
fn plain_texts(value: &Value, key: &str) -> Vec<String> {
    texts(value, key)
        .iter()
        .map(|item| plain_heading(item))
        .filter(|item| !item.is_empty())
        .collect()
}

/// Textes non vides d'un tableau, nettoyés.
fn texts(value: &Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Forme de comparaison de deux options : minuscules, sans accents ni
/// ponctuation, espaces réduits.
pub fn comparable(text: &str) -> String {
    let folded: String = text
        .to_lowercase()
        .chars()
        .filter_map(guard::fold)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Comme [`comparable`], accents gardés : cours de langue (« esta » et
/// « está » sont deux réponses).
fn comparable_with_accents(text: &str) -> String {
    let lowered: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    lowered.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Éléments distincts (vides refusés) ; `strict` (cours de langue) :
/// accents comptés.
fn distinct(items: &[String], strict: bool) -> bool {
    let mut seen: Vec<String> = Vec::new();
    for item in items {
        let key = if strict {
            comparable_with_accents(item)
        } else {
            comparable(item)
        };
        if key.is_empty() || seen.contains(&key) {
            return false;
        }
        seen.push(key);
    }
    true
}

// ------------------------------------------------------------------ plan

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanVerdict {
    /// Sujet ambigu : question à poser.
    Unclear(Clarify),
    Ready(Plan),
    /// Réponse inutilisable (raison, pour les journaux).
    Invalid(&'static str),
}

/// Plan lu dans la réponse : `unclear` (seulement si `may_ask`) avec deux
/// sens au moins, sinon un plan complet de `count` paliers dans la langue
/// du cours. `which` : question posée si le modèle n'en écrit pas.
pub fn check_plan(
    value: &Value,
    count: usize,
    lang: Lang,
    may_ask: bool,
    which: &str,
) -> PlanVerdict {
    if may_ask && value["unclear"].as_bool() == Some(true) {
        let mut options: Vec<String> = Vec::new();
        for option in plain_texts(value, "options") {
            if !options
                .iter()
                .any(|seen| comparable(seen) == comparable(&option))
            {
                options.push(option);
            }
        }
        options.truncate(4);
        if options.len() >= 2 {
            let question = match text(value, "question") {
                question if question.is_empty() => which.to_owned(),
                question => question,
            };
            return PlanVerdict::Unclear(Clarify { question, options });
        }
    }
    let title = plain_heading(&text(value, "title"));
    if title.is_empty() {
        return PlanVerdict::Invalid("plan without title");
    }
    let kind = prompts::kind_of(&text(value, "kind")).unwrap_or(StudyKind::Concept);
    let summary = text(value, "summary");
    let mut objectives = plain_texts(value, "objectives");
    let mut further = plain_texts(value, "further");
    if objectives.len() < 3 || further.len() < 3 {
        return PlanVerdict::Invalid("plan without three objectives and three further topics");
    }
    objectives.truncate(3);
    further.truncate(3);
    let Some(raw_levels) = value["levels"].as_array() else {
        return PlanVerdict::Invalid("plan without levels");
    };
    if raw_levels.len() != count {
        return PlanVerdict::Invalid("plan with a wrong number of levels");
    }
    let mut levels = Vec::with_capacity(count);
    for level in raw_levels {
        let title = without_numbering(&plain_heading(&text(level, "title")));
        let goal = text(level, "goal");
        let mut points = plain_texts(level, "points");
        if title.is_empty() || goal.is_empty() || points.len() < 2 {
            return PlanVerdict::Invalid("plan level without title, goal or two points");
        }
        points.truncate(4);
        levels.push(PlanLevel {
            title,
            goal,
            points,
        });
    }
    let written = std::iter::once(summary.as_str())
        .chain(levels.iter().map(|level| level.goal.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    if !language_ok(&written, lang, kind, true) {
        return PlanVerdict::Invalid("plan in another language");
    }
    PlanVerdict::Ready(Plan {
        title,
        kind,
        summary,
        objectives,
        levels,
        scope: text(value, "scope"),
        further,
    })
}

/// Numéro en tête d'un titre de palier (« Niveau 1 : », « Level 2 - »,
/// « Paso 3: », « 1. », « 2) »…) : l'interface numérote déjà les paliers.
/// Un ou deux chiffres seulement (« 1789 : la prise de la Bastille » reste),
/// suivis d'un séparateur ; après un nombre seul, d'une espace aussi
/// (« 1.5 », « 3:4 » restent).
static NUMBERING: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)^(?:(?:level|niveau|palier|étape|etape|nivel|paso|step|part|partie|parte|lesson|leçon|lecon|lección|leccion|module|módulo|modulo|chapter|chapitre|capítulo|capitulo|unit|unité|unite|unidad)\s*(?:n[°º.]?\s*)?\d{1,2}\s*(?:[:.)\-–—·]\s*|\s+)|\d{1,2}\s*[:.)\-–—·](?:\s+|$))",
    )
    .expect("valid pattern")
});

/// Titre sans numéro en tête ; gardé tel quel s'il ne resterait rien.
pub fn without_numbering(title: &str) -> String {
    let rest = NUMBERING.replace(title.trim(), "");
    let rest = rest.trim();
    if rest.is_empty() {
        title.trim().to_owned()
    } else {
        rest.to_owned()
    }
}

// ------------------------------------------------------------------ leçon

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LessonVerdict {
    Ready(Lesson),
    /// Valide, mais sections trop courtes ou trop longues : un nouvel essai,
    /// puis gardée.
    Length(Lesson),
    Invalid(&'static str),
}

/// Mots des sections : entre la moitié et deux fois et demie de
/// [`LESSON_WORDS`].
pub fn length_ok(lesson: &Lesson) -> bool {
    let words: usize = lesson
        .sections
        .iter()
        .map(|section| section.body.split_whitespace().count())
        .sum();
    (LESSON_WORDS / 2..=LESSON_WORDS * 5 / 2).contains(&words)
}

pub fn check_lesson(value: &Value, lang: Lang, kind: StudyKind) -> LessonVerdict {
    let hook = text(value, "hook");
    let sections: Vec<LessonSection> = value["sections"]
        .as_array()
        .map(|sections| {
            sections
                .iter()
                .map(|section| LessonSection {
                    heading: plain_heading(&text(section, "heading")),
                    body: text(section, "body"),
                })
                .collect()
        })
        .unwrap_or_default();
    if !(2..=3).contains(&sections.len())
        || sections
            .iter()
            .any(|section| section.heading.is_empty() || section.body.is_empty())
    {
        return LessonVerdict::Invalid("lesson without two or three complete sections");
    }
    let example = WorkedExample {
        problem: text(&value["example"], "problem"),
        solution: text(&value["example"], "solution"),
    };
    let mut key_points = texts(value, "keyPoints");
    key_points.truncate(4);
    let pitfall = text(value, "pitfall");
    if hook.is_empty()
        || example.problem.is_empty()
        || example.solution.is_empty()
        || key_points.len() < 2
        || pitfall.is_empty()
    {
        return LessonVerdict::Invalid("lesson with an empty part");
    }
    let written = std::iter::once(hook.as_str())
        .chain(sections.iter().map(|section| section.body.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    if !language_ok(&written, lang, kind, true) {
        return LessonVerdict::Invalid("lesson in another language");
    }
    // Formules `$…$` écrites pour le rendu de l'app (titres : texte simple).
    let lesson = Lesson {
        hook: inline_math(&hook),
        sections: sections
            .into_iter()
            .map(|section| LessonSection {
                heading: section.heading,
                body: inline_math(&section.body),
            })
            .collect(),
        example: WorkedExample {
            problem: inline_math(&example.problem),
            solution: inline_math(&example.solution),
        },
        key_points: key_points.iter().map(|point| inline_math(point)).collect(),
        pitfall: inline_math(&pitfall),
    };
    if length_ok(&lesson) {
        LessonVerdict::Ready(lesson)
    } else {
        LessonVerdict::Length(lesson)
    }
}

// ------------------------------------------------------------------ questions

/// Question acceptée par le garde-fou, solution dans l'ordre du modèle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Draft {
    Choice {
        prompt: String,
        options: Vec<String>,
        answer: usize,
    },
    TrueFalse {
        statement: String,
        answer: bool,
    },
    Blank {
        prompt: String,
        accepted: Vec<String>,
    },
    Order {
        prompt: String,
        items: Vec<String>,
    },
    Match {
        prompt: String,
        pairs: Vec<(String, String)>,
    },
}

/// Options interdites (« all of the above » et équivalents), en forme
/// [`comparable`].
const CATCH_ALL: &[&str] = &[
    "all of the above",
    "none of the above",
    "both of the above",
    "all the above",
    "none of these",
    "all of these",
    "toutes les reponses",
    "aucune des reponses",
    "toutes les propositions",
    "aucune des propositions",
    "tout ce qui precede",
    "aucune de ces reponses",
    "toutes ces reponses",
    "les deux reponses",
    "aucune des reponses precedentes",
    "todas las anteriores",
    "ninguna de las anteriores",
    "todas las respuestas",
    "ninguna de las respuestas",
    "ambas respuestas",
    "todas son correctas",
    "ninguna es correcta",
];

fn catch_all(option: &str) -> bool {
    let option = comparable(option);
    CATCH_ALL.iter().any(|phrase| option.contains(phrase))
}

/// Phrase à trou : chaque suite d'au moins trois `_` ramenée à `___` ;
/// `None` s'il n'y a pas exactement un trou.
fn one_blank(prompt: &str) -> Option<String> {
    let mut result = String::with_capacity(prompt.len());
    let mut run = 0;
    let mut blanks = 0;
    let flush = |result: &mut String, run: usize, blanks: &mut usize| {
        if run >= 3 {
            result.push_str("___");
            *blanks += 1;
        } else {
            result.push_str(&"_".repeat(run));
        }
    };
    for c in prompt.chars() {
        if c == '_' {
            run += 1;
            continue;
        }
        flush(&mut result, run, &mut blanks);
        run = 0;
        result.push(c);
    }
    flush(&mut result, run, &mut blanks);
    (blanks == 1).then_some(result)
}

/// Réponses acceptées d'un trou : la première, et celles qui lui sont
/// équivalentes pour la correction elle-même (même forme commune, même
/// nombre, en chiffres ou en toutes lettres) ou, hors cours de langue
/// (`strict`), à une faute de frappe près selon la règle de l'apprenant
/// (aucune sous 5 lettres) ; le banc montre des « variantes » qui sont
/// d'autres réponses, fausses (« has » / « have », « un » / « une »).
fn accepted_answers(answers: &[String], strict: bool) -> Vec<String> {
    let Some(first) = answers.first() else {
        return Vec::new();
    };
    let mut accepted = vec![first.clone()];
    let first_form = grade::normalize_for(first, strict);
    for answer in &answers[1..] {
        let form = grade::normalize_for(answer, strict);
        let equivalent = grade::same_answer(answer, first, strict)
            || (!strict && grade::typo_close(&form, &first_form));
        if equivalent
            && !accepted
                .iter()
                .any(|kept| grade::normalize_for(kept, strict) == form)
        {
            accepted.push(answer.clone());
        }
    }
    accepted
}

/// La réponse se lit dans l'énoncé (mots entiers).
fn gives_away(prompt: &str, answer: &str) -> bool {
    let answer = comparable(&grade::normalize(answer));
    !answer.is_empty() && format!(" {} ", comparable(prompt)).contains(&format!(" {answer} "))
}

/// Question lue dans `value` et vérifiée selon son type ; `None` si le
/// garde-fou l'écarte (voir [`question_verdict`]).
#[cfg(test)]
pub fn check_question(qtype: QType, value: &Value, lang: Lang, kind: StudyKind) -> Option<Draft> {
    question_verdict(qtype, value, lang, kind).ok()
}

/// Question lue dans `value` et vérifiée selon son type ; sinon la raison
/// (pour le journal) pour laquelle le garde-fou l'écarte.
pub fn question_verdict(
    qtype: QType,
    value: &Value,
    lang: Lang,
    kind: StudyKind,
) -> Result<Draft, &'static str> {
    let strict = kind == StudyKind::Language;
    let explanation = text(value, "explanation");
    if explanation.is_empty() {
        return Err("empty explanation");
    }
    if !language_ok(&explanation, lang, kind, false) {
        return Err("explanation in another language");
    }
    let check = |ok: bool, reason: &'static str| if ok { Ok(()) } else { Err(reason) };
    let draft = match qtype {
        QType::Choice => {
            let prompt = text(value, "prompt");
            let options = texts(value, "options");
            let raw_count = value["options"].as_array().map_or(0, Vec::len);
            let answer = value["answer"].as_u64().ok_or("no answer")? as usize;
            check(!prompt.is_empty(), "empty prompt")?;
            check(
                (3..=4).contains(&options.len()) && options.len() == raw_count,
                "not 3 or 4 options",
            )?;
            check(answer < options.len(), "answer out of range")?;
            check(distinct(&options, strict), "options not distinct")?;
            check(
                !options.iter().any(|option| catch_all(option)),
                "all/none of the above",
            )?;
            Draft::Choice {
                prompt,
                options,
                answer,
            }
        }
        QType::TrueFalse => {
            let statement = text(value, "statement");
            let answer = value["answer"].as_bool().ok_or("no answer")?;
            check(!statement.is_empty(), "empty statement")?;
            Draft::TrueFalse { statement, answer }
        }
        QType::Blank => {
            let prompt = one_blank(&text(value, "prompt")).ok_or("not exactly one blank")?;
            let accepted = accepted_answers(&texts(value, "answers"), strict);
            check(!accepted.is_empty(), "no answer")?;
            check(!gives_away(&prompt, &accepted[0]), "answer in the prompt")?;
            Draft::Blank { prompt, accepted }
        }
        QType::Order => {
            let prompt = text(value, "prompt");
            let items = texts(value, "items");
            let raw_count = value["items"].as_array().map_or(0, Vec::len);
            check(!prompt.is_empty(), "empty prompt")?;
            check(
                (3..=5).contains(&items.len()) && items.len() == raw_count,
                "not 3 to 5 items",
            )?;
            check(distinct(&items, strict), "items not distinct")?;
            Draft::Order { prompt, items }
        }
        QType::Match => {
            let prompt = text(value, "prompt");
            let raw = value["pairs"].as_array().ok_or("no pairs")?;
            let pairs: Vec<(String, String)> = raw
                .iter()
                .map(|pair| (text(pair, "left"), text(pair, "right")))
                .filter(|(left, right)| !left.is_empty() && !right.is_empty())
                .collect();
            let lefts: Vec<String> = pairs.iter().map(|pair| pair.0.clone()).collect();
            let rights: Vec<String> = pairs.iter().map(|pair| pair.1.clone()).collect();
            check(!prompt.is_empty(), "empty prompt")?;
            check(
                (3..=4).contains(&pairs.len()) && pairs.len() == raw.len(),
                "not 3 or 4 pairs",
            )?;
            check(distinct(&lefts, strict), "left elements not distinct")?;
            check(distinct(&rights, strict), "right elements not distinct")?;
            Draft::Match { prompt, pairs }
        }
    };
    Ok(draft)
}

/// Début d'un énoncé pour le journal (100 caractères au plus, une ligne).
pub fn excerpt(value: &Value) -> String {
    let prompt = ["prompt", "statement"]
        .iter()
        .find_map(|key| value[*key].as_str())
        .unwrap_or_default();
    let line: String = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    line.chars().take(100).collect()
}

/// Explication commune d'une question (lue avec la question).
pub fn explanation(value: &Value) -> String {
    text(value, "explanation")
}

// ------------------------------------------------------------------ emballement

/// Un texte de la réponse atteint (à un caractère près) la limite de son
/// schéma : le modèle s'est emballé (répétitions jusqu'à la coupure, vu
/// chez qwen3:0.6b), la réponse est inutilisable.
pub fn runaway(value: &Value, schema: &Value) -> bool {
    if let (Some(text), Some(max)) = (value.as_str(), schema["maxLength"].as_u64()) {
        return text.chars().count() as u64 + 1 >= max;
    }
    match schema["type"].as_str() {
        Some("object") => schema["properties"].as_object().is_some_and(|properties| {
            properties
                .iter()
                .any(|(key, property)| runaway(&value[key.as_str()], property))
        }),
        Some("array") => value
            .as_array()
            .is_some_and(|items| items.iter().any(|item| runaway(item, &schema["items"]))),
        _ => false,
    }
}

// ------------------------------------------------------------------ relecture

/// Erreurs relevées gardées au plus (taille de la consigne réécrite).
pub const MAX_REVIEW_ERRORS: usize = 5;

/// Forme de comparaison de la relecture : formules sans délimiteurs (`\(`,
/// `\)`, `$`), apostrophes droites, minuscules, espaces réduits.
fn review_form(text: &str) -> String {
    let text = text
        .replace("\\(", "")
        .replace("\\)", "")
        .replace('$', "")
        .replace(['’', '‘'], "'")
        .to_lowercase();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Ponctuation retirée au bord d'une citation ou d'une correction.
fn edge(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '.' | ','
                | ';'
                | ':'
                | '!'
                | '?'
                | '"'
                | '\''
                | '«'
                | '»'
                | '“'
                | '”'
                | '('
                | ')'
                | '['
                | ']'
                | '…'
                | '-'
                | '–'
                | '—'
        )
}

/// Erreurs de la relecture à corriger : la citation (forme de comparaison,
/// ponctuation de bord retirée) figure dans la leçon ; la correction n'est
/// ni la citation ni la citation suivie d'autre chose (qwen3:8b rendait des
/// « erreurs » corrigées par « … (correct) »).
pub fn review_errors(value: &Value, lesson: &str) -> Vec<(String, String)> {
    let lesson = review_form(lesson);
    value["errors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|error| {
            let quote = text(error, "quote");
            let correction = text(error, "correction");
            let quote_form = review_form(&quote).trim_matches(edge).to_owned();
            let correction_form = review_form(&correction).trim_matches(edge).to_owned();
            let real = !quote_form.is_empty()
                && !correction_form.is_empty()
                && lesson.contains(&quote_form)
                && !correction_form.starts_with(&quote_form);
            real.then_some((quote, correction))
        })
        .take(MAX_REVIEW_ERRORS)
        .collect()
}

// ------------------------------------------------------------------ mélange

/// Générateur reproductible (splitmix64), graine tirée de l'identifiant de
/// la question (FNV-1a : stable d'une version de Rust à l'autre).
struct Rng(u64);

impl Rng {
    fn new(seed: &str) -> Self {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in seed.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        Self(hash)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Permutation de `0..count` (Fisher-Yates).
    fn permutation(&mut self, count: usize) -> Vec<usize> {
        let mut order: Vec<usize> = (0..count).collect();
        for index in (1..count).rev() {
            let other = (self.next() % (index as u64 + 1)) as usize;
            order.swap(index, other);
        }
        order
    }

    /// Permutation qui n'est jamais l'identité (dès deux éléments).
    fn derangement_free(&mut self, count: usize) -> Vec<usize> {
        let mut order = self.permutation(count);
        if count > 1 && order.iter().enumerate().all(|(i, o)| i == *o) {
            order.rotate_left(1);
        }
        order
    }
}

/// Position de chaque élément d'origine dans l'ordre montré.
fn positions(shown: &[usize]) -> Vec<u32> {
    let mut positions = vec![0u32; shown.len()];
    for (position, original) in shown.iter().enumerate() {
        positions[*original] = position as u32;
    }
    positions
}

/// Question enregistrée sous `id` dans le cours `course` : options de
/// `choice` mélangées (solution suivie), éléments de `order` et droite de
/// `match` jamais dans l'ordre d'origine ; graine = cours et question (même
/// question du même cours, même mélange ; d'un cours à l'autre, la grille
/// change).
pub fn finalize(draft: Draft, course: &str, id: &str, explanation: String) -> StoredQuestion {
    let mut rng = Rng::new(&format!("{course}/{id}"));
    let (body, expected, accepted) = match draft {
        Draft::Choice {
            prompt,
            options,
            answer,
        } => {
            let shown = rng.permutation(options.len());
            let index = positions(&shown)[answer];
            (
                QuestionBody::Choice {
                    prompt,
                    options: shown.iter().map(|i| options[*i].clone()).collect(),
                },
                AnswerValue::Choice { index },
                Vec::new(),
            )
        }
        Draft::TrueFalse { statement, answer } => (
            QuestionBody::TrueFalse { statement },
            AnswerValue::TrueFalse { value: answer },
            Vec::new(),
        ),
        Draft::Blank { prompt, accepted } => (
            QuestionBody::Blank { prompt },
            AnswerValue::Blank {
                text: accepted[0].clone(),
            },
            accepted,
        ),
        Draft::Order { prompt, items } => {
            let shown = rng.derangement_free(items.len());
            (
                QuestionBody::Order {
                    prompt,
                    items: shown.iter().map(|i| items[*i].clone()).collect(),
                },
                AnswerValue::Order {
                    order: positions(&shown),
                },
                Vec::new(),
            )
        }
        Draft::Match { prompt, pairs } => {
            let shown = rng.derangement_free(pairs.len());
            (
                QuestionBody::Match {
                    prompt,
                    left: pairs.iter().map(|pair| pair.0.clone()).collect(),
                    right: shown.iter().map(|i| pairs[*i].1.clone()).collect(),
                },
                AnswerValue::Match {
                    pairs: positions(&shown),
                },
                Vec::new(),
            )
        }
    };
    StoredQuestion {
        id: id.to_owned(),
        body: with_inline_math(body),
        expected,
        accepted,
        explanation: inline_math(&explanation),
    }
}

/// Textes montrés d'une question, formules `$…$` écrites pour le rendu de
/// l'app (les réponses acceptées d'un trou restent telles quelles : elles
/// sont comparées à ce que tape l'apprenant).
fn with_inline_math(body: QuestionBody) -> QuestionBody {
    let all = |items: Vec<String>| items.iter().map(|item| inline_math(item)).collect();
    match body {
        QuestionBody::Choice { prompt, options } => QuestionBody::Choice {
            prompt: inline_math(&prompt),
            options: all(options),
        },
        QuestionBody::TrueFalse { statement } => QuestionBody::TrueFalse {
            statement: inline_math(&statement),
        },
        QuestionBody::Blank { prompt } => QuestionBody::Blank {
            prompt: inline_math(&prompt),
        },
        QuestionBody::Order { prompt, items } => QuestionBody::Order {
            prompt: inline_math(&prompt),
            items: all(items),
        },
        QuestionBody::Match {
            prompt,
            left,
            right,
        } => QuestionBody::Match {
            prompt: inline_math(&prompt),
            left: all(left),
            right: all(right),
        },
    }
}

/// Réponse du vérificateur d'accord avec la solution ; association : une
/// réponse par paire, toutes vraies.
pub fn agrees(question: &StoredQuestion, reply: &Value, strict: bool) -> bool {
    match &question.expected {
        AnswerValue::Choice { index } => reply["answer"].as_u64() == Some(u64::from(*index)),
        AnswerValue::TrueFalse { value } => reply["answer"].as_bool() == Some(*value),
        AnswerValue::Blank { .. } => {
            let accepted: Vec<&str> = question.accepted.iter().map(String::as_str).collect();
            reply["answer"].as_str().is_some_and(|answer| {
                matches!(
                    grade::match_blank(answer, &accepted, strict),
                    Match::Exact | Match::Close
                )
            })
        }
        AnswerValue::Order { order } => {
            let given: Option<Vec<u32>> = reply["order"].as_array().map(|items| {
                items
                    .iter()
                    .map(|item| item.as_u64().map_or(u32::MAX, |n| n as u32))
                    .collect()
            });
            given.as_ref() == Some(order)
        }
        AnswerValue::Match { pairs } => reply["answers"].as_array().is_some_and(|answers| {
            answers.len() == pairs.len()
                && answers.iter().all(|answer| answer.as_bool() == Some(true))
        }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const EN: Lang = Lang::En;
    const CONCEPT: StudyKind = StudyKind::Concept;

    /// Question finalisée dans un cours d'essai.
    fn finalize(draft: Draft, id: &str, explanation: String) -> StoredQuestion {
        super::finalize(draft, "course", id, explanation)
    }

    /// Accord hors cours de langue.
    fn agrees(question: &StoredQuestion, reply: &Value) -> bool {
        super::agrees(question, reply, false)
    }

    fn plan_value(levels: usize) -> Value {
        json!({
            "unclear": false, "question": "", "options": [],
            "title": "**Photosynthesis**", "kind": "concept",
            "summary": "How the plants turn the light of the sun into the sugar they need.",
            "objectives": ["Explain it", "Name the parts", " "],
            "levels": (0..levels).map(|i| json!({
                "title": format!("Level {i}"),
                "goal": "Understand what the leaf does with the light",
                "points": ["light", "**water**", "", "sugar", "oxygen", "extra"],
            })).collect::<Vec<_>>(),
            "scope": "", "further": ["**Respiration**", "Cells", "## Ecology"],
        })
    }

    #[test]
    fn a_complete_plan_is_kept_and_cleaned() {
        let mut value = plan_value(4);
        value["objectives"] = json!(["Explain it", "Name the parts", "Draw it", "Fourth"]);
        let PlanVerdict::Ready(plan) = check_plan(&value, 4, EN, true, "Which?") else {
            panic!("{:?}", check_plan(&value, 4, EN, true, "Which?"));
        };
        assert_eq!(plan.title, "Photosynthesis");
        assert_eq!(plan.kind, StudyKind::Concept);
        assert_eq!(plan.objectives.len(), 3);
        assert_eq!(plan.levels.len(), 4);
        // Notions vides retirées, quatre au plus.
        assert_eq!(plan.levels[0].points, ["light", "water", "sugar", "oxygen"]);
        // Listes en texte simple (« Go further » devient un sujet).
        assert_eq!(plan.further, ["Respiration", "Cells", "Ecology"]);
        // Mauvais nombre de paliers, objectifs manquants, langue.
        assert!(matches!(
            check_plan(&value, 6, EN, true, "?"),
            PlanVerdict::Invalid(_)
        ));
        assert!(matches!(
            check_plan(&plan_value(4), 4, EN, true, "?"),
            PlanVerdict::Invalid(_)
        ));
        value["summary"] = json!(
            "Comment les plantes transforment la lumière du soleil en sucre pour vivre et pour grandir."
        );
        for level in value["levels"].as_array_mut().unwrap() {
            level["goal"] =
                json!("Comprendre ce que la feuille fait avec la lumière et avec l'eau");
        }
        assert!(matches!(
            check_plan(&value, 4, EN, true, "?"),
            PlanVerdict::Invalid(_)
        ));
        assert!(matches!(
            check_plan(&value, 4, Lang::Fr, true, "?"),
            PlanVerdict::Ready(_)
        ));
    }

    #[test]
    fn level_titles_lose_their_number() {
        for (title, expected) in [
            (
                "Niveau 1 : Qu'est-ce que la Photosynthèse ?",
                "Qu'est-ce que la Photosynthèse ?",
            ),
            ("Niveau\u{a0}2\u{a0}: Les Ingrédients", "Les Ingrédients"),
            ("Level 1: Basics", "Basics"),
            ("level 3 - Loops", "Loops"),
            ("LEVEL 4 — Sets", "Sets"),
            ("Palier 1 : La feuille", "La feuille"),
            ("Étape 2 : Couper", "Couper"),
            ("Nivel 1: La hoja", "La hoja"),
            ("Paso 3: Llamar", "Llamar"),
            ("Step 2 Check", "Check"),
            ("Part 1: Intro", "Intro"),
            ("Partie 2 : Suite", "Suite"),
            ("Parte 1: Inicio", "Inicio"),
            ("Lesson 5. Review", "Review"),
            ("1. Fractions", "Fractions"),
            ("2) Adding", "Adding"),
            ("3 - Subtracting", "Subtracting"),
            ("4 : Dividing", "Dividing"),
            ("Niveau 1 · Bases", "Bases"),
        ] {
            assert_eq!(without_numbering(title), expected, "{title}");
        }
        for kept in [
            "1789 : la prise de la Bastille",
            "1.5 times faster",
            "3:4 ratios",
            "3 étapes pour couper",
            "Levels of energy",
            "Partial fractions",
            "Niveau 1",
            "2.",
            "Python 3: the basics",
        ] {
            assert_eq!(without_numbering(kept), kept, "{kept}");
        }
        // À la lecture du plan, Markdown compris.
        let mut value = plan_value(2);
        value["objectives"] = json!(["a", "b", "c"]);
        value["levels"][0]["title"] = json!("**Niveau 1 :** La feuille");
        value["levels"][1]["title"] = json!("Level 2");
        let PlanVerdict::Ready(plan) = check_plan(&value, 2, EN, true, "?") else {
            panic!()
        };
        assert_eq!(plan.levels[0].title, "La feuille");
        assert_eq!(plan.levels[1].title, "Level 2");
    }

    #[test]
    fn an_ambiguous_topic_asks_once() {
        let mut value = plan_value(4);
        value["objectives"] = json!(["a", "b", "c"]);
        value["unclear"] = json!(true);
        value["options"] = json!(["The island", "the island", "The programming language", ""]);
        assert_eq!(
            check_plan(&value, 4, EN, true, "Which one?"),
            PlanVerdict::Unclear(Clarify {
                question: "Which one?".into(),
                options: vec!["The island".into(), "The programming language".into()],
            })
        );
        value["question"] = json!("Java the island or the language?");
        assert!(matches!(
            check_plan(&value, 4, EN, true, "?"),
            PlanVerdict::Unclear(Clarify { question, .. }) if question == "Java the island or the language?"
        ));
        // Déjà précisé : le plan est lu.
        assert!(matches!(
            check_plan(&value, 4, EN, false, "?"),
            PlanVerdict::Ready(_)
        ));
        // Un seul sens : pas de question.
        value["options"] = json!(["The island"]);
        assert!(matches!(
            check_plan(&value, 4, EN, true, "?"),
            PlanVerdict::Ready(_)
        ));
    }

    fn lesson_value(words: usize) -> Value {
        let body = |n: usize| {
            let mut text =
                "The leaf uses the light of the sun and the water from the soil. ".to_owned();
            text.push_str(&vec!["word"; n].join(" "));
            text
        };
        json!({
            "hook": "Why are the leaves green?",
            "sections": [
                {"heading": "## The leaf", "body": body(words)},
                {"heading": "**Light**", "body": body(words)},
                {"heading": "Water", "body": body(words)},
            ],
            "example": {"problem": "A plant in the dark.", "solution": "It cannot make sugar.\n"},
            "keyPoints": ["Light is needed.", "", "Water too."],
            "pitfall": "Plants do not eat soil.",
        })
    }

    #[test]
    fn lessons_are_checked_and_their_length_measured() {
        let LessonVerdict::Ready(lesson) = check_lesson(&lesson_value(100), EN, CONCEPT) else {
            panic!()
        };
        assert_eq!(lesson.sections[0].heading, "The leaf");
        assert_eq!(lesson.sections[1].heading, "Light");
        assert_eq!(lesson.key_points, ["Light is needed.", "Water too."]);
        assert_eq!(lesson.example.solution, "It cannot make sugar.");
        // Trop court (moins de 165 mots), trop long (plus de 825).
        assert!(matches!(
            check_lesson(&lesson_value(30), EN, CONCEPT),
            LessonVerdict::Length(_)
        ));
        assert!(matches!(
            check_lesson(&lesson_value(300), EN, CONCEPT),
            LessonVerdict::Length(_)
        ));
        for broken in ["hook", "pitfall"] {
            let mut value = lesson_value(100);
            value[broken] = json!(" ");
            assert!(matches!(
                check_lesson(&value, EN, CONCEPT),
                LessonVerdict::Invalid(_)
            ));
        }
        let mut value = lesson_value(100);
        value["sections"][2]["heading"] = json!("**  **");
        assert!(matches!(
            check_lesson(&value, EN, CONCEPT),
            LessonVerdict::Invalid(_)
        ));
        let mut value = lesson_value(100);
        value["keyPoints"] = json!(["Only one"]);
        assert!(matches!(
            check_lesson(&value, EN, CONCEPT),
            LessonVerdict::Invalid(_)
        ));
        // Dans une autre langue.
        assert!(matches!(
            check_lesson(&lesson_value(100), Lang::Fr, CONCEPT),
            LessonVerdict::Invalid(_)
        ));
    }

    #[test]
    fn formulas_are_written_for_the_app_but_not_titles_nor_answers() {
        let mut value = lesson_value(100);
        value["hook"] = json!("Why is $\\frac{1}{2}$ a half?");
        value["sections"][0]["heading"] = json!("The $x$ heading");
        value["sections"][0]["body"] = json!(format!(
            "{} Half is $\\frac{{1}}{{2}}$.",
            value["sections"][0]["body"].as_str().unwrap()
        ));
        value["example"]["solution"] = json!("So $x = 2$.");
        value["keyPoints"] = json!(["Use $a/b$.", "Costs $5."]);
        value["pitfall"] = json!("Never $a + b$.");
        let LessonVerdict::Ready(lesson) = check_lesson(&value, EN, CONCEPT) else {
            panic!()
        };
        assert_eq!(lesson.hook, "Why is \\(\\frac{1}{2}\\) a half?");
        assert_eq!(lesson.sections[0].heading, "The $x$ heading");
        assert!(
            lesson.sections[0]
                .body
                .ends_with("Half is \\(\\frac{1}{2}\\).")
        );
        assert_eq!(lesson.example.solution, "So \\(x = 2\\).");
        assert_eq!(lesson.key_points, ["Use \\(a/b\\).", "Costs $5."]);
        assert_eq!(lesson.pitfall, "Never \\(a + b\\).");

        let question = finalize(
            Draft::Choice {
                prompt: "What is $\\frac{1}{2}$?".into(),
                options: vec!["$0.5$".into(), "$2$".into(), "$5 coins".into()],
                answer: 0,
            },
            "f",
            "Because $1/2 = 0.5$.".into(),
        );
        let QuestionBody::Choice { prompt, options } = &question.body else {
            panic!()
        };
        assert_eq!(prompt, "What is \\(\\frac{1}{2}\\)?");
        let mut shown = options.clone();
        shown.sort();
        assert_eq!(shown, ["$5 coins", "\\(0.5\\)", "\\(2\\)"]);
        assert_eq!(question.explanation, "Because \\(1/2 = 0.5\\).");
        let blank = finalize(
            Draft::Blank {
                prompt: "$x^2$ = ___".into(),
                accepted: vec!["$4$".into()],
            },
            "b",
            "E".into(),
        );
        assert_eq!(
            blank.body,
            QuestionBody::Blank {
                prompt: "\\(x^2\\) = ___".into()
            }
        );
        assert_eq!(blank.accepted, ["$4$"]);
        let pairs = finalize(
            Draft::Match {
                prompt: "$M$".into(),
                pairs: vec![
                    ("$a$".into(), "$1$".into()),
                    ("b".into(), "2".into()),
                    ("c".into(), "3".into()),
                ],
            },
            "m",
            "E".into(),
        );
        let QuestionBody::Match {
            prompt,
            left,
            right,
        } = &pairs.body
        else {
            panic!()
        };
        assert_eq!(prompt, "\\(M\\)");
        assert_eq!(left[0], "\\(a\\)");
        assert!(right.contains(&"\\(1\\)".to_owned()));
    }

    /// Emballement : un texte à sa limite (ou à un caractère près), à
    /// n'importe quelle profondeur.
    #[test]
    fn a_text_at_its_limit_is_a_runaway() {
        let schema = json!({"type": "object", "properties": {
            "title": {"type": "string", "maxLength": 10},
            "levels": {"type": "array", "items": {"type": "object", "properties": {
                "points": {"type": "array", "items": {"type": "string", "maxLength": 5}}
            }}},
            "free": {"type": "string"},
        }});
        let value = |title: &str, point: &str| json!({"title": title, "levels": [{"points": ["ok", point]}], "free": "x".repeat(99)});
        assert!(!runaway(&value("12345678", "abc"), &schema));
        assert!(runaway(&value("123456789", "abc"), &schema));
        assert!(runaway(&value("1234567890", "abc"), &schema));
        assert!(runaway(&value("ok", "éèêë"), &schema));
        assert!(!runaway(&value("ok", "éèê"), &schema));
        assert!(!runaway(&json!({}), &schema));
    }

    #[test]
    fn only_real_errors_of_the_review_are_kept() {
        let lesson = "## Adding\nOn obtient \\(\\frac{1}{3} = \\frac{3}{6}\\) ; donc ½ + ⅓ = 1.\nL’eau  bout à 100 °C.";
        let value = json!({"errors": [
            // Citée avec `$`, sans les délimiteurs, majuscules, ponctuation.
            {"quote": "« $\\frac{1}{3} = \\frac{3}{6}$ »", "correction": "\\frac{1}{3} = \\frac{2}{6}"},
            {"quote": "Donc ½ + ⅓ = 1.", "correction": "½ + ⅓ = 5/6"},
            // Absente de la leçon.
            {"quote": "Les plantes mangent la terre", "correction": "Non"},
            // Correction égale à la citation, ou qui commence par elle.
            {"quote": "L'eau bout à 100 °C", "correction": "l'eau bout à 100 °C."},
            {"quote": "L'eau bout à 100 °C", "correction": "L'eau bout à 100 °C (correct)"},
            // Vides.
            {"quote": "  ", "correction": "x"},
            {"quote": "½ + ⅓ = 1", "correction": ""},
        ]});
        assert_eq!(
            review_errors(&value, lesson),
            [
                (
                    "« $\\frac{1}{3} = \\frac{3}{6}$ »".to_owned(),
                    "\\frac{1}{3} = \\frac{2}{6}".to_owned()
                ),
                ("Donc ½ + ⅓ = 1.".to_owned(), "½ + ⅓ = 5/6".to_owned()),
            ]
        );
        assert!(review_errors(&json!({"errors": []}), lesson).is_empty());
        assert!(review_errors(&json!({"other": 1}), lesson).is_empty());
        let many: Vec<Value> = (0..9)
            .map(|_| json!({"quote": "½ + ⅓ = 1", "correction": "5/6"}))
            .collect();
        assert_eq!(
            review_errors(&json!({ "errors": many }), lesson).len(),
            MAX_REVIEW_ERRORS
        );
    }

    #[test]
    fn code_and_formulas_do_not_count_for_the_language() {
        let text = "La boucle parcourt la liste et garde les nombres pairs :\n```python\nfor x in xs:\n    if x in the_list and x is not None: print(x)\n```\nAvec `for x in xs` et $x \\in the set$.";
        assert!(language_ok(text, Lang::Fr, StudyKind::Code, true));
        // Cours de langue : anglais cité, français expliqué.
        let mixed = "« I have eaten » se traduit par « j'ai mangé » : vous utilisez have.";
        assert!(language_ok(mixed, Lang::Fr, StudyKind::Language, true));
        assert!(!language_ok(
            "The present perfect is used with have and the past participle.",
            Lang::Fr,
            StudyKind::Language,
            true
        ));
        // Texte court sans langue reconnue : permis.
        assert!(language_ok("Photosynthèse.", Lang::Fr, CONCEPT, false));
        assert!(language_ok(
            "Present perfect.",
            Lang::Fr,
            StudyKind::Language,
            false
        ));
    }

    fn check(qtype: QType, value: Value) -> Option<Draft> {
        check_question(qtype, &value, EN, CONCEPT)
    }

    #[test]
    fn choices_need_distinct_plain_options() {
        let good = json!({"prompt": "What does the leaf need?", "options": ["Light", "Salt", "Sand"], "answer": 0, "explanation": "The leaf needs the light."});
        assert_eq!(
            check(QType::Choice, good.clone()),
            Some(Draft::Choice {
                prompt: "What does the leaf need?".into(),
                options: vec!["Light".into(), "Salt".into(), "Sand".into()],
                answer: 0
            })
        );
        for (key, broken) in [
            ("answer", json!(3)),
            ("answer", json!(-1)),
            ("options", json!(["Light", "light.", "Sand"])),
            ("options", json!(["Light", "Lìght", "Sand"])),
            ("options", json!(["Light", "Sand"])),
            ("options", json!(["Light", "", "Sand", "Salt"])),
            ("options", json!(["Light", "Sand", "All of the above"])),
            ("options", json!(["Light", "Sand", "Toutes les réponses"])),
            (
                "options",
                json!(["Light", "Sand", "Ninguna de las anteriores."]),
            ),
            ("explanation", json!("")),
            (
                "explanation",
                json!("La feuille a besoin de la lumière et de l'eau pour vivre."),
            ),
            ("prompt", json!(" ")),
        ] {
            let mut value = good.clone();
            value[key] = broken.clone();
            assert_eq!(check(QType::Choice, value), None, "{key}: {broken}");
        }
    }

    #[test]
    fn blanks_have_one_hole_and_close_variants_only() {
        let value = json!({"prompt": "Plants make ______ from light.", "answers": ["sugar", "sugars", "glucose", "Sugar"], "explanation": "It is sugar."});
        assert_eq!(
            check(QType::Blank, value),
            Some(Draft::Blank {
                prompt: "Plants make ___ from light.".into(),
                // « glucose » est une autre réponse : écartée ; « Sugar »
                // est déjà là.
                accepted: vec!["sugar".into(), "sugars".into()],
            })
        );
        let numbers = json!({"prompt": "2 + 3 = ___", "answers": ["5", "five", "5.0", "6"], "explanation": "It is five."});
        assert_eq!(
            check(QType::Blank, numbers),
            Some(Draft::Blank {
                prompt: "2 + 3 = ___".into(),
                accepted: vec!["5".into(), "five".into(), "5.0".into()],
            })
        );
        for prompt in ["No blank here.", "Two ___ and ___.", "Short __ hole."] {
            let value = json!({"prompt": prompt, "answers": ["x"], "explanation": "E"});
            assert_eq!(check(QType::Blank, value), None, "{prompt}");
        }
        // Réponse déjà écrite dans l'énoncé.
        let value = json!({"prompt": "The sugar is called ___ (sugar).", "answers": ["the sugar"], "explanation": "E"});
        assert_eq!(check(QType::Blank, value), None);
        let value =
            json!({"prompt": "Sugary drinks have ___.", "answers": ["sugar"], "explanation": "E"});
        assert!(check(QType::Blank, value).is_some());
    }

    /// rust-2 : une variante courte qui est une autre réponse n'est pas
    /// gardée ; rust-17 : le nombre en toutes lettres l'est.
    #[test]
    fn blank_variants_follow_the_learner_rule() {
        let accepted = |answers: &[&str], kind: StudyKind| {
            let value = json!({"prompt": "She ___ eaten.", "answers": answers, "explanation": "E"});
            match check_question(QType::Blank, &value, EN, kind) {
                Some(Draft::Blank { accepted, .. }) => accepted,
                other => panic!("{other:?}"),
            }
        };
        for pair in [
            ["has", "have"],
            ["does", "do"],
            ["une", "un"],
            ["noun", "nouns"],
        ] {
            assert_eq!(accepted(&pair, CONCEPT), [pair[0]], "{pair:?}");
        }
        // Une faute selon la règle de l'apprenant (5 lettres), pas deux.
        assert_eq!(
            accepted(&["sugar", "suger", "sagur"], CONCEPT),
            ["sugar", "suger"]
        );
        assert_eq!(
            accepted(&["photosynthesis", "photosinthesys"], CONCEPT),
            ["photosynthesis", "photosinthesys"]
        );
        // Équivalentes pour la correction : même forme, même nombre, nombre en
        // toutes lettres (dans les deux sens).
        assert_eq!(
            accepted(&["5", "five", "cinq", "5.0", "6"], CONCEPT),
            ["5", "five", "cinq", "5.0"]
        );
        assert_eq!(accepted(&["five", "5"], CONCEPT), ["five", "5"]);
        assert_eq!(accepted(&["the sugar", "sugar"], CONCEPT), ["the sugar"]);
        // Cours de langue : aucune faute admise comme variante.
        assert_eq!(
            accepted(&["parlent", "parlant"], StudyKind::Language),
            ["parlent"]
        );
        assert_eq!(accepted(&["has", "Has"], StudyKind::Language), ["has"]);
    }

    /// rust-3 : cours de langue : options qui ne diffèrent que par l'accent
    /// gardées ; vérificateur d'accord seulement sur l'orthographe exacte.
    #[test]
    fn a_language_course_tells_accents_apart() {
        let value = json!({"prompt": "¿Dónde ___?", "options": ["esta", "está", "es"], "answer": 1, "explanation": "Es la respuesta correcta porque la lección lo dice."});
        assert!(check_question(QType::Choice, &value, Lang::Es, StudyKind::Language).is_some());
        assert!(check_question(QType::Choice, &value, Lang::Es, StudyKind::Concept).is_none());
        let blank = finalize(
            Draft::Blank {
                prompt: "Ella ___ ayer.".into(),
                accepted: vec!["habló".into()],
            },
            "b",
            "E".into(),
        );
        assert!(super::agrees(&blank, &json!({"answer": "habló"}), true));
        assert!(!super::agrees(&blank, &json!({"answer": "hablo"}), true));
        assert!(super::agrees(&blank, &json!({"answer": "hablo"}), false));
    }

    /// rust-5 : la grille d'une question dépend du cours.
    #[test]
    fn shuffles_differ_from_one_course_to_another() {
        let draft = || Draft::Match {
            prompt: "P".into(),
            pairs: (0..4).map(|n| (format!("l{n}"), format!("r{n}"))).collect(),
        };
        let grid = |course: &str| super::finalize(draft(), course, "L0-A0-Q2", "E".into()).expected;
        assert_eq!(grid("course-a"), grid("course-a"));
        let grids: std::collections::HashSet<String> = (0..12)
            .map(|n| format!("{:?}", grid(&format!("course-{n}"))))
            .collect();
        assert!(grids.len() >= 4, "{grids:?}");
    }

    #[test]
    fn orders_and_matches_need_distinct_elements() {
        let order = json!({"prompt": "Sort", "items": ["a", "b", "c"], "explanation": "E"});
        assert!(check(QType::Order, order.clone()).is_some());
        for items in [
            json!(["a", "b"]),
            json!(["a", "A", "c"]),
            json!(["a", "b", "c", "d", "e", "f"]),
            json!(["a", "", "c"]),
        ] {
            let mut value = order.clone();
            value["items"] = items.clone();
            assert_eq!(check(QType::Order, value), None, "{items}");
        }
        let pairs = json!({"prompt": "Match", "pairs": [
            {"left": "1", "right": "one"}, {"left": "2", "right": "two"}, {"left": "3", "right": "three"}
        ], "explanation": "E"});
        assert!(check(QType::Match, pairs.clone()).is_some());
        let mut same_right = pairs.clone();
        same_right["pairs"][2]["right"] = json!("One");
        assert_eq!(check(QType::Match, same_right), None);
        let mut same_left = pairs.clone();
        same_left["pairs"][2]["left"] = json!("1");
        assert_eq!(check(QType::Match, same_left), None);
        let mut empty = pairs;
        empty["pairs"][1]["right"] = json!("");
        assert_eq!(check(QType::Match, empty), None);
        let truth = json!({"statement": "", "answer": true, "explanation": "E"});
        assert_eq!(check(QType::TrueFalse, truth), None);
        let truth = json!({"statement": "S", "answer": "yes", "explanation": "E"});
        assert_eq!(check(QType::TrueFalse, truth), None);
    }

    /// La raison d'un refus est nommée (journal).
    #[test]
    fn refusals_say_why() {
        let verdict = |qtype, value: Value| question_verdict(qtype, &value, EN, CONCEPT).err();
        let choice = json!({"prompt": "P", "options": ["a", "A", "b"], "answer": 0, "explanation": "It is the answer."});
        assert_eq!(verdict(QType::Choice, choice), Some("options not distinct"));
        let blank =
            json!({"prompt": "No hole.", "answers": ["x"], "explanation": "It is the answer."});
        assert_eq!(verdict(QType::Blank, blank), Some("not exactly one blank"));
        let truth = json!({"statement": "S", "answer": true, "explanation": ""});
        assert_eq!(verdict(QType::TrueFalse, truth), Some("empty explanation"));
        let long = json!({"prompt": format!("A\nlong   {}", "x".repeat(200))});
        assert_eq!(excerpt(&long).chars().count(), 100);
        assert!(excerpt(&long).starts_with("A long xxx"));
    }

    /// Doublons (banc : qwen3:0.6b) écartés par le garde-fou, avant tout
    /// mélange.
    #[test]
    fn duplicates_are_refused_before_shuffling() {
        let same = json!(["Step", "Step", "Step"]);
        let order = json!({"prompt": "Sort", "items": same, "explanation": "E"});
        assert_eq!(check(QType::Order, order), None);
        let choice = json!({"prompt": "P", "options": same, "answer": 0, "explanation": "E"});
        assert_eq!(check(QType::Choice, choice), None);
        let pairs = json!({"prompt": "M", "pairs": [
            {"left": "a", "right": "x"}, {"left": "b", "right": "x"}, {"left": "c", "right": "x"}
        ], "explanation": "E"});
        assert_eq!(check(QType::Match, pairs), None);
    }

    /// Mélange borné : des éléments tous égaux (garde-fou contourné) ne le
    /// font pas tourner sans fin (le banc bouclait sur « encore dans
    /// l'ordre »).
    #[test]
    fn shuffling_identical_elements_ends() {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let items = vec!["Step".to_owned(); 5];
            let order = finalize(
                Draft::Order {
                    prompt: "P".into(),
                    items: items.clone(),
                },
                "L0-A0-Q0",
                "E".into(),
            );
            let pairs = finalize(
                Draft::Match {
                    prompt: "P".into(),
                    pairs: vec![("a".into(), "x".into()); 4],
                },
                "L0-A0-Q1",
                "E".into(),
            );
            let _ = sender.send((order, pairs));
        });
        let (order, pairs) = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("shuffle never ended");
        // Les positions changent (jamais l'identité), même à texte égal.
        let AnswerValue::Order { order } = order.expected else {
            panic!()
        };
        assert_ne!(order, [0, 1, 2, 3, 4]);
        let AnswerValue::Match { pairs } = pairs.expected else {
            panic!()
        };
        assert_ne!(pairs, [0, 1, 2, 3]);
    }

    #[test]
    fn shuffles_are_reproducible_and_never_in_order() {
        let options: Vec<String> = ["Right", "Wrong 1", "Wrong 2", "Wrong 3"]
            .map(String::from)
            .into();
        let mut seen_first = std::collections::HashSet::new();
        for n in 0..40 {
            let id = format!("L0-A0-Q{n}");
            let draft = Draft::Choice {
                prompt: "P".into(),
                options: options.clone(),
                answer: 0,
            };
            let question = finalize(draft.clone(), &id, "E".into());
            assert_eq!(question, finalize(draft, &id, "E".into()));
            let (QuestionBody::Choice { options: shown, .. }, AnswerValue::Choice { index }) =
                (&question.body, &question.expected)
            else {
                panic!()
            };
            assert_eq!(shown[*index as usize], "Right");
            seen_first.insert(shown[0].clone());

            let items: Vec<String> = ["a", "b", "c"].map(String::from).into();
            let question = finalize(
                Draft::Order {
                    prompt: "P".into(),
                    items: items.clone(),
                },
                &id,
                "E".into(),
            );
            let (QuestionBody::Order { items: shown, .. }, AnswerValue::Order { order }) =
                (&question.body, &question.expected)
            else {
                panic!()
            };
            assert_ne!(shown, &items);
            let rebuilt: Vec<String> = order.iter().map(|i| shown[*i as usize].clone()).collect();
            assert_eq!(rebuilt, items);

            let question = finalize(
                Draft::Match {
                    prompt: "P".into(),
                    pairs: vec![
                        ("1".into(), "one".into()),
                        ("2".into(), "two".into()),
                        ("3".into(), "three".into()),
                    ],
                },
                &id,
                "E".into(),
            );
            let (QuestionBody::Match { left, right, .. }, AnswerValue::Match { pairs }) =
                (&question.body, &question.expected)
            else {
                panic!()
            };
            assert_eq!(left, &["1", "2", "3"]);
            assert_ne!(right, &["one", "two", "three"]);
            let rebuilt: Vec<&str> = pairs.iter().map(|i| right[*i as usize].as_str()).collect();
            assert_eq!(rebuilt, ["one", "two", "three"]);
        }
        // La bonne réponse n'est pas toujours à la même place.
        assert!(seen_first.len() >= 3, "{seen_first:?}");
    }

    #[test]
    fn the_checker_agrees_only_with_the_solution() {
        let choice = finalize(
            Draft::Choice {
                prompt: "P".into(),
                options: vec!["a".into(), "b".into(), "c".into()],
                answer: 1,
            },
            "x",
            "E".into(),
        );
        let AnswerValue::Choice { index } = choice.expected else {
            panic!()
        };
        assert!(agrees(&choice, &json!({"answer": index})));
        assert!(!agrees(&choice, &json!({"answer": (index + 1) % 3})));
        assert!(!agrees(&choice, &json!({})));
        let blank = finalize(
            Draft::Blank {
                prompt: "x ___".into(),
                accepted: vec!["la photosynthèse".into()],
            },
            "b",
            "E".into(),
        );
        assert!(agrees(&blank, &json!({"answer": "Photosynthese"})));
        assert!(agrees(&blank, &json!({"answer": "photosynthèse."})));
        assert!(!agrees(&blank, &json!({"answer": "respiration"})));
        let order = finalize(
            Draft::Order {
                prompt: "P".into(),
                items: vec!["a".into(), "b".into(), "c".into()],
            },
            "o",
            "E".into(),
        );
        let AnswerValue::Order { order: expected } = order.expected.clone() else {
            panic!()
        };
        assert!(agrees(&order, &json!({ "order": expected })));
        assert!(!agrees(&order, &json!({"order": [0, 1, 2]})) || expected == [0, 1, 2]);
        assert!(!agrees(&order, &json!({"order": [-1, 1, 2]})));
        let truth = finalize(
            Draft::TrueFalse {
                statement: "S".into(),
                answer: false,
            },
            "t",
            "E".into(),
        );
        assert!(agrees(&truth, &json!({"answer": false})));
        assert!(!agrees(&truth, &json!({"answer": true})));
        let pairs = finalize(
            Draft::Match {
                prompt: "P".into(),
                pairs: vec![
                    ("1".into(), "one".into()),
                    ("2".into(), "two".into()),
                    ("3".into(), "three".into()),
                ],
            },
            "m",
            "E".into(),
        );
        assert!(agrees(&pairs, &json!({"answers": [true, true, true]})));
        assert!(!agrees(&pairs, &json!({"answers": [true, false, true]})));
        // Une réponse par paire, ni plus ni moins.
        assert!(!agrees(&pairs, &json!({"answers": [true, true]})));
        assert!(!agrees(
            &pairs,
            &json!({"answers": [true, true, true, true]})
        ));
        assert!(!agrees(&pairs, &json!({"answers": ["true", true, true]})));
        assert!(!agrees(&pairs, &json!({"matches": [0, 1, 2]})));
    }
}
