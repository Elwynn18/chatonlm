//! Textes d'un cours lus à voix haute (commande `read_aloud_study`) : la page
//! nomme un passage par sa clé, son texte est relu en base (jamais envoyé
//! par la page), dans la langue du cours.
//!
//! Clés (palier `i`, à partir de 0) :
//! - `L{i}:s{k}` : section `k` de la leçon (la clé de sa carte) : son titre,
//!   l'accroche du palier en tête de la première, son texte ;
//! - `L{i}:example` : énoncé de l'exemple résolu ; `L{i}:solution` : sa
//!   solution ;
//! - `L{i}:recap` : les « à retenir », puis le piège fréquent ;
//! - `L{i}:a{j}:explain` : explication autrement de l'essai `j` ;
//! - `L{i}:explain{n}` : explication plus simple numéro `n` (« I don't
//!   understand », dans l'ordre d'enregistrement du palier) ;
//! - `L{i}:ask{n}` : réponse à la question numéro `n` (« Ask a question »).

use rusqlite::Connection;

use super::data::LevelData;
use crate::error::AppResult;
use crate::i18n::{Lang, t};
use crate::store::study as db;

/// Passage d'un palier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Section(usize),
    Example,
    Solution,
    Recap,
    Remedy(usize),
    Explain(usize),
    Ask(usize),
}

/// Nombre écrit en chiffres seulement (pas de signe, pas d'espace).
fn number(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Palier et passage d'une clé ; `None` : clé inconnue.
fn parse(key: &str) -> Option<(usize, Part)> {
    let (level, rest) = key.strip_prefix('L')?.split_once(':')?;
    let level = number(level)?;
    let part = match rest {
        "example" => Part::Example,
        "solution" => Part::Solution,
        "recap" => Part::Recap,
        _ => {
            if let Some(attempt) = rest
                .strip_prefix('a')
                .and_then(|rest| rest.strip_suffix(":explain"))
            {
                Part::Remedy(number(attempt)?)
            } else if let Some(index) = rest.strip_prefix("explain") {
                Part::Explain(number(index)?)
            } else if let Some(index) = rest.strip_prefix("ask") {
                Part::Ask(number(index)?)
            } else {
                Part::Section(number(rest.strip_prefix('s')?)?)
            }
        }
    };
    Some((level, part))
}

/// Markdown d'un passage ; `None` : pas (encore) écrit.
fn markdown(level: &LevelData, part: Part, lang: Lang) -> Option<String> {
    let lesson = level.lesson.as_ref();
    match part {
        Part::Section(index) => {
            let lesson = lesson?;
            let section = lesson.sections.get(index)?;
            let mut blocks = vec![format!("### {}", section.heading)];
            if index == 0 && !lesson.hook.trim().is_empty() {
                blocks.push(lesson.hook.clone());
            }
            blocks.push(section.body.clone());
            Some(blocks.join("\n\n"))
        }
        Part::Example => Some(lesson?.example.problem.clone()),
        Part::Solution => Some(lesson?.example.solution.clone()),
        Part::Recap => {
            let lesson = lesson?;
            let points: Vec<String> = lesson
                .key_points
                .iter()
                .map(|point| format!("- {point}"))
                .collect();
            let mut blocks = vec![
                format!("### {}", t!("study.notes.keyPoints" in lang)),
                points.join("\n"),
            ];
            if !lesson.pitfall.trim().is_empty() {
                blocks.push(t!("study.notes.pitfall" in lang, text = lesson.pitfall));
            }
            Some(blocks.join("\n\n"))
        }
        Part::Remedy(attempt) => level.attempts.get(attempt)?.explanation.clone(),
        Part::Explain(index) => Some(level.explains.get(index)?.text.clone()),
        Part::Ask(index) => Some(level.asks.get(index)?.answer.clone()),
    }
}

/// Markdown du passage `key` du cours `course` et langue du cours ; `None` :
/// cours, palier ou passage absent (cours supprimé, palier refait, clé
/// inconnue).
pub fn load(connection: &Connection, course: &str, key: &str) -> AppResult<Option<(String, Lang)>> {
    let Some((index, part)) = parse(key) else {
        return Ok(None);
    };
    let Some(row) = db::row(connection, course)? else {
        return Ok(None);
    };
    let Some(level) = db::level(connection, course, index)? else {
        return Ok(None);
    };
    Ok(markdown(&level, part, row.language).map(|text| (text, row.language)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::study::data::tests::{level, row as course_row};
    use crate::study::data::{SourceRow, StoredAttempt};
    use crate::study::{Ask, Explain, Lesson, LessonSection, WorkedExample};

    #[test]
    fn keys_name_the_passages_of_a_level() {
        assert_eq!(parse("L0:s0"), Some((0, Part::Section(0))));
        assert_eq!(parse("L12:s2"), Some((12, Part::Section(2))));
        assert_eq!(parse("L1:example"), Some((1, Part::Example)));
        assert_eq!(parse("L1:solution"), Some((1, Part::Solution)));
        assert_eq!(parse("L1:recap"), Some((1, Part::Recap)));
        assert_eq!(parse("L3:a1:explain"), Some((3, Part::Remedy(1))));
        assert_eq!(parse("L3:explain4"), Some((3, Part::Explain(4))));
        assert_eq!(parse("L3:ask0"), Some((3, Part::Ask(0))));
        // Ni question, ni carte hors d'un palier, ni nombre signé ou absent.
        for unknown in [
            "",
            "intro",
            "end",
            "final:q0",
            "L0",
            "L0:",
            "L0:check",
            "L0:a0:q1",
            "L0:a0:result",
            "L:s0",
            "L0:s",
            "L0:s+1",
            "L-1:s0",
            "L0:sx",
            "L0:a:explain",
            "L0:explain",
            "L0:ask",
            "L0:ask 1",
            "l0:s0",
        ] {
            assert_eq!(parse(unknown), None, "{unknown}");
        }
    }

    fn lesson() -> Lesson {
        Lesson {
            hook: "Did you know?".into(),
            sections: vec![
                LessonSection {
                    heading: "Light".into(),
                    body: "Leaves use **light**.".into(),
                },
                LessonSection {
                    heading: "Water".into(),
                    body: "Roots take water.".into(),
                },
            ],
            example: WorkedExample {
                problem: "What does a leaf need?".into(),
                solution: "1. Light.\n2. Water.".into(),
            },
            key_points: vec!["Light matters.".into(), "Water too.".into()],
            pitfall: "Plants do not eat soil.".into(),
        }
    }

    /// Le texte lu vient de la base : chaque clé rend son passage, tel
    /// qu'il est enregistré ; rien pour ce qui n'existe pas.
    #[test]
    fn a_passage_is_read_from_the_database_in_the_language_of_its_course() {
        let store = Store::open_in_memory().unwrap();
        let mut row = course_row(10);
        row.id = "c1".into();
        row.language = Lang::Fr;
        let sources: Vec<SourceRow> = Vec::new();
        store.blocking(|c| db::insert(c, &row, &sources)).unwrap();
        let mut first = level(0, 1);
        first.lesson = Some(lesson());
        first.attempts.push(StoredAttempt {
            explanation: Some("Think of a kitchen.".into()),
            questions: Vec::new(),
            pass_mark: 2,
        });
        first.explains.push(Explain {
            section: 1,
            text: "Simpler words.".into(),
        });
        first.asks.push(Ask {
            question: "Why?".into(),
            answer: "Because of the sun.".into(),
        });
        // Palier pas encore écrit.
        let second = LevelData::default();
        store
            .blocking(|c| db::put_levels(c, "c1", &[first, second]))
            .unwrap();
        let read = |key: &'static str| {
            store
                .blocking(move |c| load(c, "c1", key))
                .unwrap()
                .map(|(text, lang)| {
                    assert_eq!(lang, Lang::Fr);
                    text
                })
        };
        // L'accroche du palier ouvre sa première section seulement.
        assert_eq!(
            read("L0:s0").as_deref(),
            Some("### Light\n\nDid you know?\n\nLeaves use **light**.")
        );
        assert_eq!(
            read("L0:s1").as_deref(),
            Some("### Water\n\nRoots take water.")
        );
        assert_eq!(
            read("L0:example").as_deref(),
            Some("What does a leaf need?")
        );
        assert_eq!(read("L0:solution").as_deref(), Some("1. Light.\n2. Water."));
        // Titres dans la langue du cours.
        assert_eq!(
            read("L0:recap").as_deref(),
            Some(
                "### À retenir\n\n- Light matters.\n- Water too.\n\n\
                 **Piège fréquent\u{a0}:** Plants do not eat soil."
            )
        );
        assert_eq!(
            read("L0:a1:explain").as_deref(),
            Some("Think of a kitchen.")
        );
        assert_eq!(read("L0:explain0").as_deref(), Some("Simpler words."));
        assert_eq!(read("L0:ask0").as_deref(), Some("Because of the sun."));
        // Premier essai (sans explication), passages absents, palier pas
        // écrit ou inconnu, clé inconnue, cours inconnu.
        for missing in [
            "L0:a0:explain",
            "L0:a2:explain",
            "L0:s2",
            "L0:explain1",
            "L0:ask1",
            "L1:s0",
            "L1:recap",
            "L2:s0",
            "L0:check",
        ] {
            assert_eq!(read(missing), None, "{missing}");
        }
        assert_eq!(store.blocking(|c| load(c, "nope", "L0:s0")).unwrap(), None);
    }

    #[test]
    fn an_empty_hook_or_pitfall_is_left_out() {
        let mut bare = lesson();
        bare.hook = "  ".into();
        bare.pitfall = String::new();
        let mut data = level(0, 1);
        data.lesson = Some(bare);
        assert_eq!(
            markdown(&data, Part::Section(0), Lang::En).as_deref(),
            Some("### Light\n\nLeaves use **light**.")
        );
        assert_eq!(
            markdown(&data, Part::Recap, Lang::En).as_deref(),
            Some("### Key points\n\n- Light matters.\n- Water too.")
        );
    }
}
