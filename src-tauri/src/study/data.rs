//! Formes enregistrées d'un cours (JSON des colonnes `plan`, `final`,
//! `clarify` de `courses`, `data` de `course_levels`), avec les solutions
//! que la page ne voit jamais ; règles de statut des paliers, estimations de
//! durée et cours tel que lu par la page ([`assemble`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    AnswerValue, Answered, Ask, Attempt, Clarify, Course, CourseStatus, CourseSummary, Explain,
    FinalQuiz, GenState, Lesson, Level, LevelStatus, Plan, Question, QuestionBody, StudyError,
    StudyLevel, StudySource,
};
use crate::i18n::Lang;

/// Type d'une question (nom de son schéma, voir `prompts::question_schema`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QType {
    Choice,
    TrueFalse,
    Blank,
    Order,
    Match,
}

impl QType {
    /// Temps de réponse estimé (secondes).
    pub fn seconds(self) -> u32 {
        match self {
            QType::Choice => 25,
            QType::TrueFalse => 15,
            QType::Blank => 30,
            QType::Order => 40,
            QType::Match => 45,
        }
    }
}

/// Question enregistrée : telle que montrée (options mélangées), avec sa
/// solution dans l'ordre montré et son explication.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredQuestion {
    pub id: String,
    pub body: QuestionBody,
    pub expected: AnswerValue,
    /// Trou : réponses acceptées (la première est la solution montrée).
    #[serde(default)]
    pub accepted: Vec<String>,
    pub explanation: String,
}

impl StoredQuestion {
    pub fn shown(&self) -> Question {
        Question {
            id: self.id.clone(),
            body: self.body.clone(),
        }
    }

    pub fn qtype(&self) -> QType {
        match self.body {
            QuestionBody::Choice { .. } => QType::Choice,
            QuestionBody::TrueFalse { .. } => QType::TrueFalse,
            QuestionBody::Blank { .. } => QType::Blank,
            QuestionBody::Order { .. } => QType::Order,
            QuestionBody::Match { .. } => QType::Match,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAttempt {
    pub explanation: Option<String>,
    pub questions: Vec<StoredQuestion>,
    pub pass_mark: u32,
}

/// Palier enregistré (`course_levels.data`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelData {
    pub state: GenState,
    #[serde(default)]
    pub error: Option<StudyError>,
    /// Enregistrée dès qu'elle est prête (les questions suivent).
    #[serde(default)]
    pub lesson: Option<Lesson>,
    #[serde(default)]
    pub check: Option<StoredQuestion>,
    #[serde(default)]
    pub attempts: Vec<StoredAttempt>,
    #[serde(default)]
    pub remedy: Option<GenState>,
    #[serde(default)]
    pub remedy_error: Option<StudyError>,
    /// « Continue anyway ».
    #[serde(default)]
    pub skipped: bool,
    #[serde(default)]
    pub explains: Vec<Explain>,
    #[serde(default)]
    pub asks: Vec<Ask>,
}

impl Default for LevelData {
    fn default() -> Self {
        Self {
            state: GenState::Pending,
            error: None,
            lesson: None,
            check: None,
            attempts: Vec::new(),
            remedy: None,
            remedy_error: None,
            skipped: false,
            explains: Vec::new(),
            asks: Vec::new(),
        }
    }
}

/// Bilan enregistré (`courses.final`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalData {
    pub state: GenState,
    #[serde(default)]
    pub error: Option<StudyError>,
    #[serde(default)]
    pub questions: Vec<StoredQuestion>,
}

impl Default for FinalData {
    fn default() -> Self {
        Self {
            state: GenState::Pending,
            error: None,
            questions: Vec::new(),
        }
    }
}

/// Sujet ambigu (`courses.clarify`) : question posée, puis précision.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClarifyState {
    pub question: String,
    pub options: Vec<String>,
    /// Précision donnée (ou sujet gardé tel quel) : plus jamais de question.
    #[serde(default)]
    pub answered: bool,
    /// Sens choisi ou écrit par l'apprenant.
    #[serde(default)]
    pub meaning: Option<String>,
}

/// Ligne de `courses`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CourseRow {
    pub id: String,
    pub topic: String,
    pub level: StudyLevel,
    pub minutes: u32,
    pub language: Lang,
    pub model: String,
    pub web: bool,
    pub status: CourseStatus,
    pub error: Option<StudyError>,
    pub clarify: Option<ClarifyState>,
    pub plan: Option<Plan>,
    pub final_quiz: Option<FinalData>,
    pub position: String,
    pub elapsed_ms: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
    /// Génération mise en pause par l'apprenant.
    pub paused: bool,
}

/// Source enregistrée (`course_sources`), texte compris.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRow {
    pub source: StudySource,
    pub text: String,
}

/// Cours entier tel qu'en base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub row: CourseRow,
    pub levels: Vec<LevelData>,
    pub answers: BTreeMap<String, Answered>,
    pub sources: Vec<SourceRow>,
}

/// Nombre de paliers pour une durée (10, 20 ou 30 minutes).
pub fn level_count(minutes: u32) -> usize {
    match minutes {
        10 => 2,
        30 => 6,
        _ => 4,
    }
}

/// Nombre de questions du bilan pour une durée.
pub fn final_count(minutes: u32) -> usize {
    match minutes {
        10 => 3,
        30 => 6,
        _ => 5,
    }
}

/// Bonnes réponses pour réussir un essai de `count` questions : 2 sur 3,
/// arrondi supérieur des deux tiers sinon.
pub fn pass_mark(count: usize) -> u32 {
    (2 * count).div_ceil(3) as u32
}

/// Avancement d'un palier d'après les réponses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Un essai réussi (toutes ses questions répondues, assez de bonnes).
    pub passed: bool,
    /// Essais terminés et manqués.
    pub failed_attempts: u32,
    /// Le dernier essai est terminé et manqué.
    pub last_failed: bool,
}

pub fn progress(level: &LevelData, answers: &BTreeMap<String, Answered>) -> Progress {
    let mut result = Progress::default();
    for (index, attempt) in level.attempts.iter().enumerate() {
        let given: Vec<&Answered> = attempt
            .questions
            .iter()
            .filter_map(|question| answers.get(&question.id))
            .collect();
        let complete = !attempt.questions.is_empty() && given.len() == attempt.questions.len();
        let correct = given.iter().filter(|answered| answered.correct).count() as u32;
        let last = index + 1 == level.attempts.len();
        if complete && correct >= attempt.pass_mark {
            result.passed = true;
        } else if complete {
            result.failed_attempts += 1;
            result.last_failed |= last;
        }
    }
    result
}

/// Statut de chaque palier : `passed` (un essai réussi), `review` (passé
/// après deux essais manqués), `current` (premier des autres), `locked`.
pub fn statuses(levels: &[LevelData], answers: &BTreeMap<String, Answered>) -> Vec<LevelStatus> {
    let mut current_seen = false;
    levels
        .iter()
        .map(|level| {
            if progress(level, answers).passed {
                LevelStatus::Passed
            } else if level.skipped {
                LevelStatus::Review
            } else if !current_seen {
                current_seen = true;
                LevelStatus::Current
            } else {
                LevelStatus::Locked
            }
        })
        .collect()
}

/// Tous les paliers réussis ou passés (le bilan peut commencer).
pub fn levels_done(levels: &[LevelData], answers: &BTreeMap<String, Answered>) -> bool {
    !levels.is_empty()
        && statuses(levels, answers)
            .iter()
            .all(|status| matches!(status, LevelStatus::Passed | LevelStatus::Review))
}

/// Cours fini : paliers faits, bilan prêt et toutes ses questions répondues.
pub fn is_finished(snapshot: &Snapshot) -> bool {
    let final_done = snapshot.row.final_quiz.as_ref().is_some_and(|final_quiz| {
        final_quiz.state == GenState::Ready
            && !final_quiz.questions.is_empty()
            && final_quiz
                .questions
                .iter()
                .all(|question| snapshot.answers.contains_key(&question.id))
    });
    final_done && levels_done(&snapshot.levels, &snapshot.answers)
}

/// Partie à écrire : en attente ou en cours (pas en échec, ni prête).
fn unwritten(state: GenState) -> bool {
    matches!(state, GenState::Pending | GenState::Generating)
}

/// Le cours a-t-il encore des parties à écrire (en file, en cours, ou
/// retenues par une pause) ? Comme la file : le plan en préparation, le
/// palier 0 d'un plan prêt, puis tous les paliers, les nouveaux essais
/// demandés et le bilan d'un cours commencé. Une partie en échec n'en est
/// pas (elle attend `study_retry`), ni le bilan qu'elle retient.
pub fn pending(snapshot: &Snapshot) -> bool {
    let levels = &snapshot.levels;
    match snapshot.row.status {
        CourseStatus::Planning => true,
        CourseStatus::Planned => levels.first().is_some_and(|level| unwritten(level.state)),
        CourseStatus::Active | CourseStatus::Finished => {
            let failed = levels.iter().any(|level| level.state == GenState::Failed);
            levels
                .iter()
                .any(|level| unwritten(level.state) || level.remedy.is_some_and(unwritten))
                || (!failed
                    && snapshot
                        .row
                        .final_quiz
                        .as_ref()
                        .is_some_and(|final_quiz| unwritten(final_quiz.state)))
        }
        CourseStatus::Unclear | CourseStatus::Failed => false,
    }
}

/// Mots d'un texte (lecture estimée à 180 mots par minute).
fn words(text: &str) -> u32 {
    text.split_whitespace().count() as u32
}

/// Durée de lecture d'une leçon (secondes) : accroche, sections, exemple,
/// à retenir, piège.
pub fn reading_seconds(lesson: &Lesson) -> u32 {
    let mut count = words(&lesson.hook) + words(&lesson.pitfall);
    count += lesson
        .sections
        .iter()
        .map(|section| words(&section.body))
        .sum::<u32>();
    count += words(&lesson.example.problem) + words(&lesson.example.solution);
    count += lesson
        .key_points
        .iter()
        .map(|point| words(point))
        .sum::<u32>();
    (count * 60).div_ceil(180)
}

/// Palier pas encore généré.
pub const UNKNOWN_LEVEL_SECONDS: u32 = 240;

/// Durée d'un palier : lecture, entraînement, premier essai et 20 s.
pub fn level_seconds(level: &LevelData) -> u32 {
    match (&level.lesson, level.state) {
        (Some(lesson), GenState::Ready) => {
            let questions = level
                .check
                .iter()
                .chain(
                    level
                        .attempts
                        .first()
                        .into_iter()
                        .flat_map(|a| &a.questions),
                )
                .map(|question| question.qtype().seconds())
                .sum::<u32>();
            reading_seconds(lesson) + questions + 20
        }
        _ => UNKNOWN_LEVEL_SECONDS,
    }
}

/// Durée du bilan : ses questions, sinon 25 s par question prévue.
pub fn final_seconds(final_quiz: Option<&FinalData>, minutes: u32) -> u32 {
    match final_quiz {
        Some(final_quiz) if final_quiz.state == GenState::Ready => final_quiz
            .questions
            .iter()
            .map(|question| question.qtype().seconds())
            .sum(),
        _ => 25 * final_count(minutes) as u32,
    }
}

/// Durée du cours : la durée visée avant le plan ; ensuite 30 s, les paliers
/// et le bilan.
pub fn course_seconds(row: &CourseRow, levels: &[LevelData]) -> u32 {
    if row.plan.is_none() {
        return row.minutes * 60;
    }
    30 + levels.iter().map(level_seconds).sum::<u32>()
        + final_seconds(row.final_quiz.as_ref(), row.minutes)
}

/// Cours tel que lu par la page : solutions retirées, statuts et durées
/// calculés.
pub fn assemble(snapshot: Snapshot) -> Course {
    let Snapshot {
        row,
        levels,
        answers,
        sources,
    } = snapshot;
    let statuses = statuses(&levels, &answers);
    let estimated_seconds = course_seconds(&row, &levels);
    let levels_shown = levels
        .iter()
        .zip(statuses)
        .enumerate()
        .map(|(index, (level, status))| Level {
            index: index as u32,
            state: level.state,
            error: level.error.clone(),
            lesson: level.lesson.clone(),
            check: level.check.as_ref().map(StoredQuestion::shown),
            attempts: level
                .attempts
                .iter()
                .map(|attempt| Attempt {
                    explanation: attempt.explanation.clone(),
                    questions: attempt
                        .questions
                        .iter()
                        .map(StoredQuestion::shown)
                        .collect(),
                    pass_mark: attempt.pass_mark,
                })
                .collect(),
            remedy: level.remedy,
            remedy_error: level.remedy_error.clone(),
            status,
            failed_attempts: progress(level, &answers).failed_attempts,
            explains: level.explains.clone(),
            asks: level.asks.clone(),
            estimated_seconds: level_seconds(level),
        })
        .collect();
    let final_quiz = row.final_quiz.as_ref().map(|final_quiz| FinalQuiz {
        state: final_quiz.state,
        error: final_quiz.error.clone(),
        questions: final_quiz
            .questions
            .iter()
            .map(StoredQuestion::shown)
            .collect(),
        estimated_seconds: final_seconds(Some(final_quiz), row.minutes),
    });
    let clarify = match (&row.clarify, row.status) {
        (Some(clarify), CourseStatus::Unclear) => Some(Clarify {
            question: clarify.question.clone(),
            options: clarify.options.clone(),
        }),
        _ => None,
    };
    Course {
        id: row.id,
        topic: row.topic,
        level: row.level,
        minutes: row.minutes,
        language: row.language,
        model: row.model,
        web: row.web,
        status: row.status,
        error: row.error,
        clarify,
        plan: row.plan,
        sources: sources.into_iter().map(|source| source.source).collect(),
        levels: levels_shown,
        final_quiz,
        answers,
        position: row.position,
        elapsed_ms: row.elapsed_ms,
        estimated_seconds,
        created_at: row.created_at,
        updated_at: row.updated_at,
        finished_at: row.finished_at,
        paused: row.paused,
        // Connu de la file seule (`StudyEngine::get`).
        queue_busy: false,
    }
}

/// Titre d'un cours dans la liste : celui du plan, sinon le sujet, sinon
/// les noms des documents.
pub fn title(snapshot: &Snapshot) -> String {
    if let Some(plan) = &snapshot.row.plan
        && !plan.title.trim().is_empty()
    {
        return plan.title.clone();
    }
    if !snapshot.row.topic.trim().is_empty() {
        return snapshot.row.topic.clone();
    }
    document_names(snapshot)
}

/// Noms des documents du cours, séparés par des virgules.
pub fn document_names(snapshot: &Snapshot) -> String {
    snapshot
        .sources
        .iter()
        .filter(|source| source.source.kind == super::SourceKind::Document)
        .map(|source| source.source.title.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Ligne de la liste des cours.
pub fn summary(snapshot: &Snapshot) -> CourseSummary {
    let statuses = statuses(&snapshot.levels, &snapshot.answers);
    let levels_done = statuses
        .iter()
        .filter(|status| matches!(status, LevelStatus::Passed | LevelStatus::Review))
        .count() as u32;
    let (final_correct, final_total) = match (&snapshot.row.final_quiz, snapshot.row.status) {
        (Some(final_quiz), CourseStatus::Finished) => {
            let correct = final_quiz
                .questions
                .iter()
                .filter(|question| {
                    snapshot
                        .answers
                        .get(&question.id)
                        .is_some_and(|answered| answered.correct)
                })
                .count() as u32;
            (Some(correct), Some(final_quiz.questions.len() as u32))
        }
        _ => (None, None),
    };
    let row = &snapshot.row;
    CourseSummary {
        id: row.id.clone(),
        title: title(snapshot),
        topic: row.topic.clone(),
        level: row.level,
        minutes: row.minutes,
        status: row.status,
        levels_done,
        levels_total: row.plan.as_ref().map_or(0, |plan| plan.levels.len() as u32),
        levels_ready: snapshot
            .levels
            .iter()
            .filter(|level| level.state == GenState::Ready)
            .count() as u32,
        pending: pending(snapshot),
        paused: row.paused,
        final_correct,
        final_total,
        created_at: row.created_at,
        updated_at: row.updated_at,
        finished_at: row.finished_at,
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::study::{LessonSection, StudyKind, WorkedExample};

    pub fn question(id: &str) -> StoredQuestion {
        StoredQuestion {
            id: id.into(),
            body: QuestionBody::TrueFalse {
                statement: "S".into(),
            },
            expected: AnswerValue::TrueFalse { value: true },
            accepted: Vec::new(),
            explanation: "E".into(),
        }
    }

    pub fn answered(correct: bool) -> Answered {
        Answered {
            given: AnswerValue::TrueFalse { value: correct },
            correct,
            close: false,
            expected: AnswerValue::TrueFalse { value: true },
            detail: Vec::new(),
            explanation: "E".into(),
            at: 1,
        }
    }

    /// Palier prêt dont l'essai `j` a les questions `L{i}-A{j}-Q0..2`.
    pub fn level(index: usize, attempts: usize) -> LevelData {
        LevelData {
            state: GenState::Ready,
            check: Some(question(&format!("L{index}-check"))),
            attempts: (0..attempts)
                .map(|j| StoredAttempt {
                    explanation: None,
                    questions: (0..3)
                        .map(|n| question(&format!("L{index}-A{j}-Q{n}")))
                        .collect(),
                    pass_mark: 2,
                })
                .collect(),
            ..LevelData::default()
        }
    }

    fn answer(answers: &mut BTreeMap<String, Answered>, id: &str, correct: bool) {
        answers.insert(id.into(), answered(correct));
    }

    #[test]
    fn pass_marks_are_two_thirds_rounded_up() {
        assert_eq!(pass_mark(3), 2);
        assert_eq!(pass_mark(2), 2);
        assert_eq!(pass_mark(4), 3);
        assert_eq!(pass_mark(5), 4);
        assert_eq!(pass_mark(6), 4);
        assert_eq!(pass_mark(1), 1);
    }

    #[test]
    fn an_attempt_counts_once_all_its_questions_are_answered() {
        let levels = vec![level(0, 1), level(1, 1), level(2, 1)];
        let mut answers = BTreeMap::new();
        let status = |answers: &BTreeMap<String, Answered>| statuses(&levels, answers);
        assert_eq!(
            status(&answers),
            [
                LevelStatus::Current,
                LevelStatus::Locked,
                LevelStatus::Locked
            ]
        );
        // Deux bonnes réponses sur deux : l'essai n'est pas fini.
        answer(&mut answers, "L0-A0-Q0", true);
        answer(&mut answers, "L0-A0-Q1", true);
        // La question d'entraînement ne compte pas.
        answer(&mut answers, "L0-check", false);
        assert_eq!(status(&answers)[0], LevelStatus::Current);
        assert_eq!(progress(&levels[0], &answers), Progress::default());
        answer(&mut answers, "L0-A0-Q2", false);
        assert_eq!(
            status(&answers),
            [
                LevelStatus::Passed,
                LevelStatus::Current,
                LevelStatus::Locked
            ]
        );
        // Une seule bonne : manqué.
        answer(&mut answers, "L1-A0-Q0", true);
        answer(&mut answers, "L1-A0-Q1", false);
        answer(&mut answers, "L1-A0-Q2", false);
        assert_eq!(
            progress(&levels[1], &answers),
            Progress {
                passed: false,
                failed_attempts: 1,
                last_failed: true
            }
        );
        assert_eq!(status(&answers)[1], LevelStatus::Current);
    }

    #[test]
    fn review_and_passed_levels_are_not_current() {
        let mut levels = vec![level(0, 2), level(1, 1), level(2, 1)];
        let mut answers = BTreeMap::new();
        for j in 0..2 {
            for n in 0..3 {
                answer(&mut answers, &format!("L0-A{j}-Q{n}"), n == 0);
            }
        }
        assert_eq!(
            progress(&levels[0], &answers),
            Progress {
                passed: false,
                failed_attempts: 2,
                last_failed: true
            }
        );
        levels[0].skipped = true;
        // Palier 2 réussi avant le palier 1 (refait après coup) : reste
        // réussi, le palier 1 est le palier en cours.
        for n in 0..3 {
            answer(&mut answers, &format!("L2-A0-Q{n}"), true);
        }
        assert_eq!(
            statuses(&levels, &answers),
            [
                LevelStatus::Review,
                LevelStatus::Current,
                LevelStatus::Passed
            ]
        );
        assert!(!levels_done(&levels, &answers));
        for n in 0..3 {
            answer(&mut answers, &format!("L1-A0-Q{n}"), true);
        }
        assert!(levels_done(&levels, &answers));
        // Un essai réussi après le saut l'emporte sur « à revoir ».
        levels[0].attempts.push(level(0, 3).attempts.remove(2));
        for n in 0..3 {
            answer(&mut answers, &format!("L0-A2-Q{n}"), true);
        }
        assert_eq!(statuses(&levels, &answers)[0], LevelStatus::Passed);
        // Un essai commencé après un essai manqué : le dernier n'est plus
        // manqué.
        let fresh = level(3, 2);
        let mut answers = BTreeMap::new();
        for n in 0..3 {
            answer(&mut answers, &format!("L3-A0-Q{n}"), false);
        }
        answer(&mut answers, "L3-A1-Q0", false);
        assert_eq!(
            progress(&fresh, &answers),
            Progress {
                passed: false,
                failed_attempts: 1,
                last_failed: false
            }
        );
    }

    fn lesson(words_per_part: usize) -> Lesson {
        let text = vec!["word"; words_per_part].join(" ");
        Lesson {
            hook: text.clone(),
            sections: vec![
                LessonSection {
                    heading: "H".into(),
                    body: text.clone(),
                },
                LessonSection {
                    heading: "H".into(),
                    body: text.clone(),
                },
            ],
            example: WorkedExample {
                problem: text.clone(),
                solution: text.clone(),
            },
            key_points: vec![text.clone(), text.clone()],
            pitfall: text,
        }
    }

    pub fn row(minutes: u32) -> CourseRow {
        CourseRow {
            id: "c".into(),
            topic: "Topic".into(),
            level: StudyLevel::Beginner,
            minutes,
            language: Lang::En,
            model: String::new(),
            web: false,
            status: CourseStatus::Planning,
            error: None,
            clarify: None,
            plan: None,
            final_quiz: None,
            position: String::new(),
            elapsed_ms: 0,
            created_at: 1,
            updated_at: 1,
            finished_at: None,
            paused: false,
        }
    }

    /// Ce que la liste des cours dit de la génération : paliers prêts, reste
    /// à écrire (comme la file : palier 0 seul avant « Start », rien après un
    /// échec tant qu'il n'est pas relancé), pause.
    #[test]
    fn the_list_tells_what_is_left_to_write() {
        let plan = Plan {
            title: "T".into(),
            kind: StudyKind::Concept,
            summary: String::new(),
            objectives: Vec::new(),
            levels: Vec::new(),
            scope: String::new(),
            further: Vec::new(),
        };
        let mut snapshot = Snapshot {
            row: row(10),
            levels: Vec::new(),
            answers: BTreeMap::new(),
            sources: Vec::new(),
        };
        // Plan en préparation.
        assert!(pending(&snapshot));
        snapshot.row.status = CourseStatus::Failed;
        assert!(!pending(&snapshot));
        // Plan prêt : seul le palier 0 est écrit avant « Start ».
        snapshot.row.status = CourseStatus::Planned;
        snapshot.row.plan = Some(plan);
        snapshot.row.final_quiz = Some(FinalData::default());
        snapshot.levels = vec![LevelData::default(), LevelData::default()];
        assert!(pending(&snapshot));
        snapshot.levels[0] = level(0, 1);
        assert!(!pending(&snapshot));
        let listed = summary(&snapshot);
        assert_eq!((listed.levels_ready, listed.pending), (1, false));
        // Commencé : le palier 1 puis le bilan restent à écrire.
        snapshot.row.status = CourseStatus::Active;
        snapshot.row.paused = true;
        let listed = summary(&snapshot);
        assert_eq!(
            (listed.levels_ready, listed.pending, listed.paused),
            (1, true, true)
        );
        snapshot.levels[1].state = GenState::Generating;
        assert!(pending(&snapshot));
        // Palier en échec : plus rien n'avance sans « Retry », bilan compris.
        snapshot.levels[1].state = GenState::Failed;
        assert!(!pending(&snapshot));
        snapshot.levels[1] = level(1, 1);
        assert!(pending(&snapshot), "bilan à écrire");
        snapshot.levels[0].remedy = Some(GenState::Failed);
        snapshot.row.final_quiz.as_mut().unwrap().state = GenState::Ready;
        assert!(!pending(&snapshot));
        snapshot.levels[0].remedy = Some(GenState::Pending);
        assert!(pending(&snapshot), "nouvel essai demandé");
        assert_eq!(summary(&snapshot).levels_ready, 2);
    }

    #[test]
    fn durations_are_estimated_from_what_is_generated() {
        // 8 parties de 45 mots = 360 mots = 120 s ; arrondi au-dessus.
        assert_eq!(reading_seconds(&lesson(45)), 120);
        assert_eq!(reading_seconds(&lesson(20)), 54);
        let mut ready = level(0, 1);
        ready.lesson = Some(lesson(45));
        // 120 s + entraînement (vrai/faux 15 s) + 3 × 15 s + 20 s.
        assert_eq!(level_seconds(&ready), 120 + 15 + 45 + 20);
        ready.state = GenState::Generating;
        assert_eq!(level_seconds(&ready), UNKNOWN_LEVEL_SECONDS);
        assert_eq!(level_seconds(&LevelData::default()), 240);

        let mut course = row(20);
        assert_eq!(course_seconds(&course, &[]), 1200);
        course.plan = Some(Plan {
            title: "T".into(),
            kind: StudyKind::Concept,
            summary: String::new(),
            objectives: Vec::new(),
            levels: Vec::new(),
            scope: String::new(),
            further: Vec::new(),
        });
        ready.state = GenState::Ready;
        let levels = vec![ready, LevelData::default()];
        // Bilan non généré : 5 questions × 25 s.
        assert_eq!(course_seconds(&course, &levels), 30 + 200 + 240 + 125);
        course.final_quiz = Some(FinalData {
            state: GenState::Ready,
            error: None,
            questions: vec![question("F-Q0"), question("F-Q1")],
        });
        assert_eq!(course_seconds(&course, &levels), 30 + 200 + 240 + 30);
        assert_eq!(final_seconds(None, 10), 75);
        assert_eq!(final_seconds(None, 30), 150);
    }

    #[test]
    fn counts_follow_the_duration() {
        assert_eq!(
            [10, 20, 30].map(level_count),
            [2, 4, 6],
            "paliers par durée"
        );
        assert_eq!([10, 20, 30].map(final_count), [3, 5, 6]);
    }

    #[test]
    fn the_page_never_sees_the_solutions() {
        let mut snapshot = Snapshot {
            row: row(10),
            levels: vec![level(0, 1)],
            answers: BTreeMap::new(),
            sources: Vec::new(),
        };
        snapshot.levels[0].check.as_mut().unwrap().accepted = vec!["secret".into()];
        let course = assemble(snapshot);
        let json = serde_json::to_string(&course).unwrap();
        assert!(
            !json.contains("expected") && !json.contains("secret"),
            "{json}"
        );
        assert_eq!(course.levels[0].status, LevelStatus::Current);
        assert_eq!(course.levels[0].attempts[0].questions.len(), 3);
    }
}
