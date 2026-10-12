//! Fiche du cours en Markdown, dans la langue du cours : titre, résumé,
//! objectifs, puis pour chaque palier son titre, ses « à retenir » et son
//! piège, enfin les sources (liens).

use super::SourceKind;
use super::data::{Snapshot, title};
use crate::i18n::t;

pub fn markdown(snapshot: &Snapshot) -> String {
    let lang = snapshot.row.language;
    let mut parts = vec![format!("# {}", title(snapshot))];
    if let Some(plan) = &snapshot.row.plan {
        if !plan.summary.is_empty() {
            parts.push(plan.summary.clone());
        }
        if !plan.objectives.is_empty() {
            parts.push(format!(
                "## {}\n\n{}",
                t!("study.notes.objectives" in lang),
                list(&plan.objectives)
            ));
        }
        for (index, level) in plan.levels.iter().enumerate() {
            let mut section = vec![format!(
                "## {}",
                t!("study.notes.level" in lang, number = index + 1, title = level.title)
            )];
            if let Some(lesson) = snapshot
                .levels
                .get(index)
                .and_then(|level| level.lesson.as_ref())
            {
                section.push(format!(
                    "### {}\n\n{}",
                    t!("study.notes.keyPoints" in lang),
                    list(&lesson.key_points)
                ));
                section.push(t!("study.notes.pitfall" in lang, text = lesson.pitfall));
            }
            parts.push(section.join("\n\n"));
        }
    }
    let sources: Vec<String> = snapshot
        .sources
        .iter()
        .map(|source| match source.source.kind {
            SourceKind::Web if !source.source.url.is_empty() => {
                format!(
                    "- [{}]({})",
                    escape(&source.source.title),
                    source.source.url
                )
            }
            _ => format!("- {}", source.source.title),
        })
        .collect();
    if !sources.is_empty() {
        parts.push(format!(
            "## {}\n\n{}",
            t!("study.notes.sources" in lang),
            sources.join("\n")
        ));
    }
    let mut text = parts.join("\n\n");
    text.push('\n');
    text
}

fn list(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Titre d'un lien : crochets échappés.
fn escape(text: &str) -> String {
    text.replace('[', "\\[").replace(']', "\\]")
}

/// Nom proposé pour le fichier : titre sans caractères interdits, `.md`.
pub fn file_name(snapshot: &Snapshot) -> String {
    let cleaned: String = title(snapshot)
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned: String = cleaned.chars().take(80).collect();
    let cleaned = cleaned.trim().trim_start_matches('.');
    if cleaned.is_empty() {
        "notes.md".to_owned()
    } else {
        format!("{cleaned}.md")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::i18n::Lang;
    use crate::study::data::tests::row;
    use crate::study::data::{LevelData, SourceRow};
    use crate::study::{
        Lesson, LessonSection, Plan, PlanLevel, StudyKind, StudySource, WorkedExample,
    };

    fn snapshot(lang: Lang) -> Snapshot {
        let mut course = row(10);
        course.language = lang;
        course.plan = Some(Plan {
            title: "La photosynthèse".into(),
            kind: StudyKind::Concept,
            summary: "Comment les plantes vivent.".into(),
            objectives: vec!["Expliquer".into(), "Nommer".into(), "Dessiner".into()],
            levels: vec![
                PlanLevel {
                    title: "La feuille".into(),
                    goal: "G".into(),
                    points: vec![],
                },
                PlanLevel {
                    title: "La lumière".into(),
                    goal: "G".into(),
                    points: vec![],
                },
            ],
            scope: String::new(),
            further: vec![],
        });
        let lesson = Lesson {
            hook: "H".into(),
            sections: vec![LessonSection {
                heading: "S".into(),
                body: "B".into(),
            }],
            example: WorkedExample {
                problem: "P".into(),
                solution: "S".into(),
            },
            key_points: vec![
                "La chlorophylle capte la lumière.".into(),
                "$6 CO_2$".into(),
            ],
            pitfall: "Les plantes ne mangent pas la terre.".into(),
        };
        Snapshot {
            row: course,
            levels: vec![
                LevelData {
                    lesson: Some(lesson),
                    ..LevelData::default()
                },
                LevelData::default(),
            ],
            answers: BTreeMap::new(),
            sources: vec![
                SourceRow {
                    source: StudySource {
                        kind: SourceKind::Document,
                        title: "cours.pdf".into(),
                        url: String::new(),
                    },
                    text: "x".into(),
                },
                SourceRow {
                    source: StudySource {
                        kind: SourceKind::Web,
                        title: "Wiki [plantes]".into(),
                        url: "https://example.com/p".into(),
                    },
                    text: "x".into(),
                },
            ],
        }
    }

    #[test]
    fn notes_are_written_in_the_language_of_the_course() {
        assert_eq!(
            markdown(&snapshot(Lang::Fr)),
            "# La photosynthèse\n\nComment les plantes vivent.\n\n\
             ## Objectifs\n\n- Expliquer\n- Nommer\n- Dessiner\n\n\
             ## Palier 1\u{a0}: La feuille\n\n### À retenir\n\n\
             - La chlorophylle capte la lumière.\n- $6 CO_2$\n\n\
             **Piège fréquent\u{a0}:** Les plantes ne mangent pas la terre.\n\n\
             ## Palier 2\u{a0}: La lumière\n\n\
             ## Sources\n\n- cours.pdf\n- [Wiki \\[plantes\\]](https://example.com/p)\n"
        );
        let english = markdown(&snapshot(Lang::En));
        assert!(english.contains("## Objectives\n"));
        assert!(english.contains("## Level 1: La feuille\n\n### Key points\n"));
        assert!(english.contains("**Common mistake:** Les plantes"));
        let spanish = markdown(&snapshot(Lang::Es));
        assert!(spanish.contains("## Nivel 2: La lumière"));
        assert!(spanish.contains("### Para recordar"));
        assert!(spanish.contains("**Error frecuente:**"));
    }

    #[test]
    fn a_course_without_plan_has_a_short_sheet() {
        let mut bare = snapshot(Lang::En);
        bare.row.plan = None;
        bare.sources.clear();
        assert_eq!(markdown(&bare), "# Topic\n");
    }

    #[test]
    fn the_file_name_comes_from_the_title() {
        let mut course = snapshot(Lang::Fr);
        assert_eq!(file_name(&course), "La photosynthèse.md");
        course.row.plan.as_mut().unwrap().title = "C/C++ : pointeurs?\n".into();
        assert_eq!(file_name(&course), "C C++ pointeurs.md");
        course.row.plan.as_mut().unwrap().title = "...".into();
        assert_eq!(file_name(&course), "notes.md");
    }
}
