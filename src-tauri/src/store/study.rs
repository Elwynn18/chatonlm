//! Cours générés (onglet Study) : tables `courses`, `course_sources`,
//! `course_levels` et `course_answers` (migration 0012). Les contenus
//! générés sont en JSON (types de `study::data`).
//!
//! Chaque changement passe par une lecture puis une écriture dans le même
//! appel ([`update_row`], [`update_level`]) : sous le verrou de la
//! connexion, rien ne s'intercale entre les deux.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::now_ms;
use crate::error::{AppError, AppResult};
use crate::i18n::Lang;
use crate::study::data::{CourseRow, LevelData, Snapshot, SourceRow};
use crate::study::{Answered, CourseStatus, GenState, SourceKind, StudyLevel, StudySource};

fn level_text(level: StudyLevel) -> &'static str {
    match level {
        StudyLevel::Beginner => "beginner",
        StudyLevel::Intermediate => "intermediate",
        StudyLevel::Advanced => "advanced",
    }
}

fn level_of(text: &str) -> AppResult<StudyLevel> {
    Ok(match text {
        "beginner" => StudyLevel::Beginner,
        "intermediate" => StudyLevel::Intermediate,
        "advanced" => StudyLevel::Advanced,
        other => return Err(unknown("level", other)),
    })
}

fn status_text(status: CourseStatus) -> &'static str {
    match status {
        CourseStatus::Planning => "planning",
        CourseStatus::Unclear => "unclear",
        CourseStatus::Planned => "planned",
        CourseStatus::Active => "active",
        CourseStatus::Finished => "finished",
        CourseStatus::Failed => "failed",
    }
}

fn status_of(text: &str) -> AppResult<CourseStatus> {
    Ok(match text {
        "planning" => CourseStatus::Planning,
        "unclear" => CourseStatus::Unclear,
        "planned" => CourseStatus::Planned,
        "active" => CourseStatus::Active,
        "finished" => CourseStatus::Finished,
        "failed" => CourseStatus::Failed,
        other => return Err(unknown("status", other)),
    })
}

fn lang_of(text: &str) -> AppResult<Lang> {
    Ok(match text {
        "en" => Lang::En,
        "es" => Lang::Es,
        "fr" => Lang::Fr,
        other => return Err(unknown("language", other)),
    })
}

fn kind_text(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Document => "document",
        SourceKind::Web => "web",
    }
}

fn unknown(what: &str, value: &str) -> AppError {
    AppError::Internal(format!("unknown course {what} in database: {value}"))
}

/// Exécute `f` dans une transaction : tout ou rien. Déjà dans une
/// transaction (appel imbriqué) : `f` en fait partie.
pub fn atomic<T>(
    connection: &Connection,
    f: impl FnOnce(&Connection) -> AppResult<T>,
) -> AppResult<T> {
    if !connection.is_autocommit() {
        return f(connection);
    }
    let transaction = connection.unchecked_transaction()?;
    let result = f(&transaction)?;
    transaction.commit()?;
    Ok(result)
}

fn to_json<T: Serialize>(value: &T) -> AppResult<String> {
    serde_json::to_string(value).map_err(|error| AppError::Internal(error.to_string()))
}

fn from_json<T: DeserializeOwned>(text: &str) -> AppResult<T> {
    serde_json::from_str(text).map_err(|error| AppError::Internal(error.to_string()))
}

fn optional_json<T: DeserializeOwned>(text: Option<String>) -> AppResult<Option<T>> {
    text.map(|text| from_json(&text)).transpose()
}

const COLUMNS: &str = "id, topic, level, minutes, language, model, web, status, error, clarify, \
                       plan, final, position, elapsed_ms, created_at, updated_at, finished_at";

/// Colonnes lues telles quelles (converties ensuite, hors de rusqlite).
type RawRow = (
    String,
    String,
    String,
    u32,
    String,
    String,
    bool,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    i64,
    i64,
    i64,
    Option<i64>,
);

fn raw(row: &Row<'_>) -> rusqlite::Result<RawRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
        row.get(15)?,
        row.get(16)?,
    ))
}

fn convert(raw: RawRow) -> AppResult<CourseRow> {
    let (
        id,
        topic,
        level,
        minutes,
        language,
        model,
        web,
        status,
        error,
        clarify,
        plan,
        final_quiz,
        position,
        elapsed_ms,
        created_at,
        updated_at,
        finished_at,
    ) = raw;
    Ok(CourseRow {
        id,
        topic,
        level: level_of(&level)?,
        minutes,
        language: lang_of(&language)?,
        model,
        web,
        status: status_of(&status)?,
        error: optional_json(error)?,
        clarify: optional_json(clarify)?,
        plan: optional_json(plan)?,
        final_quiz: optional_json(final_quiz)?,
        position,
        elapsed_ms,
        created_at,
        updated_at,
        finished_at,
    })
}

/// Enregistre un nouveau cours et ses sources.
pub fn insert(connection: &Connection, row: &CourseRow, sources: &[SourceRow]) -> AppResult<()> {
    atomic(connection, |transaction| {
        transaction.execute(
            &format!(
                "INSERT INTO courses ({COLUMNS}) VALUES \
             (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"
            ),
            params![
                row.id,
                row.topic,
                level_text(row.level),
                row.minutes,
                row.language.code(),
                row.model,
                row.web,
                status_text(row.status),
                row.error.as_ref().map(to_json).transpose()?,
                row.clarify.as_ref().map(to_json).transpose()?,
                row.plan.as_ref().map(to_json).transpose()?,
                row.final_quiz.as_ref().map(to_json).transpose()?,
                row.position,
                row.elapsed_ms,
                row.created_at,
                row.updated_at,
                row.finished_at,
            ],
        )?;
        insert_sources(transaction, &row.id, sources)?;
        Ok(())
    })
}

fn insert_sources(connection: &Connection, id: &str, sources: &[SourceRow]) -> AppResult<()> {
    let mut statement = connection.prepare(
        "INSERT INTO course_sources (course_id, kind, title, url, text) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for source in sources {
        statement.execute(params![
            id,
            kind_text(source.source.kind),
            source.source.title,
            source.source.url,
            source.text
        ])?;
    }
    Ok(())
}

/// Ajoute des sources (résultats de la recherche web).
pub fn add_sources(connection: &Connection, id: &str, sources: &[SourceRow]) -> AppResult<()> {
    atomic(connection, |transaction| {
        insert_sources(transaction, id, sources)
    })
}

fn write_row(connection: &Connection, row: &CourseRow) -> AppResult<()> {
    connection.execute(
        "UPDATE courses SET topic = ?2, model = ?3, web = ?4, status = ?5, error = ?6,
                clarify = ?7, plan = ?8, final = ?9, position = ?10, elapsed_ms = ?11,
                updated_at = ?12, finished_at = ?13
         WHERE id = ?1",
        params![
            row.id,
            row.topic,
            row.model,
            row.web,
            status_text(row.status),
            row.error.as_ref().map(to_json).transpose()?,
            row.clarify.as_ref().map(to_json).transpose()?,
            row.plan.as_ref().map(to_json).transpose()?,
            row.final_quiz.as_ref().map(to_json).transpose()?,
            row.position,
            row.elapsed_ms,
            row.updated_at,
            row.finished_at,
        ],
    )?;
    Ok(())
}

pub fn row(connection: &Connection, id: &str) -> AppResult<Option<CourseRow>> {
    let raw = connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM courses WHERE id = ?1"),
            [id],
            raw,
        )
        .optional()?;
    raw.map(convert).transpose()
}

/// Change la ligne d'un cours (dernière activité mise à l'heure) ; `None`
/// si le cours n'existe pas.
pub fn update_row<T>(
    connection: &Connection,
    id: &str,
    change: impl FnOnce(&mut CourseRow) -> AppResult<T>,
) -> AppResult<Option<T>> {
    atomic(connection, |transaction| {
        let Some(mut row) = row(transaction, id)? else {
            return Ok(None);
        };
        let result = change(&mut row)?;
        row.updated_at = now_ms();
        write_row(transaction, &row)?;
        Ok(Some(result))
    })
}

fn levels(connection: &Connection, id: &str) -> AppResult<Vec<LevelData>> {
    let mut statement =
        connection.prepare("SELECT data FROM course_levels WHERE course_id = ?1 ORDER BY idx")?;
    let texts = statement
        .query_map([id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    texts.iter().map(|text| from_json(text)).collect()
}

/// Remplace tous les paliers d'un cours.
pub fn put_levels(connection: &Connection, id: &str, levels: &[LevelData]) -> AppResult<()> {
    atomic(connection, |transaction| {
        transaction.execute("DELETE FROM course_levels WHERE course_id = ?1", [id])?;
        for (index, level) in levels.iter().enumerate() {
            transaction.execute(
                "INSERT INTO course_levels (course_id, idx, data) VALUES (?1, ?2, ?3)",
                params![id, index as i64, to_json(level)?],
            )?;
        }
        Ok(())
    })
}

/// Palier `index` d'un cours ; `None` s'il n'existe pas.
pub fn level(connection: &Connection, id: &str, index: usize) -> AppResult<Option<LevelData>> {
    let text: Option<String> = connection
        .query_row(
            "SELECT data FROM course_levels WHERE course_id = ?1 AND idx = ?2",
            params![id, index as i64],
            |row| row.get(0),
        )
        .optional()?;
    text.map(|text| from_json(&text)).transpose()
}

/// Change un palier (et la dernière activité du cours) ; `None` si le cours
/// ou le palier n'existe pas.
pub fn update_level<T>(
    connection: &Connection,
    id: &str,
    index: usize,
    change: impl FnOnce(&mut LevelData) -> AppResult<T>,
) -> AppResult<Option<T>> {
    atomic(connection, |transaction| {
        let Some(mut level) = level(transaction, id, index)? else {
            return Ok(None);
        };
        let result = change(&mut level)?;
        transaction.execute(
            "UPDATE course_levels SET data = ?3 WHERE course_id = ?1 AND idx = ?2",
            params![id, index as i64, to_json(&level)?],
        )?;
        transaction.execute(
            "UPDATE courses SET updated_at = ?2 WHERE id = ?1",
            params![id, now_ms()],
        )?;
        Ok(Some(result))
    })
}

fn answers(connection: &Connection, id: &str) -> AppResult<BTreeMap<String, Answered>> {
    let mut statement =
        connection.prepare("SELECT question_id, data FROM course_answers WHERE course_id = ?1")?;
    let rows = statement
        .query_map([id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(question, text)| Ok((question, from_json(&text)?)))
        .collect()
}

/// Enregistre la première réponse à une question ; `false` si elle avait
/// déjà une réponse (gardée).
pub fn put_answer(
    connection: &Connection,
    id: &str,
    question: &str,
    answered: &Answered,
) -> AppResult<bool> {
    let inserted = connection.execute(
        "INSERT OR IGNORE INTO course_answers (course_id, question_id, data) VALUES (?1, ?2, ?3)",
        params![id, question, to_json(answered)?],
    )?;
    Ok(inserted == 1)
}

/// Efface les réponses dont l'identifiant commence par `prefix`.
pub fn delete_answers(connection: &Connection, id: &str, prefix: &str) -> AppResult<()> {
    let escaped = prefix
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    connection.execute(
        "DELETE FROM course_answers WHERE course_id = ?1 AND question_id LIKE ?2 ESCAPE '\\'",
        params![id, format!("{escaped}%")],
    )?;
    Ok(())
}

/// Sources d'un cours ; `with_text` : avec leur texte (génération
/// seulement : jusqu'à 240 000 caractères par cours).
fn sources(connection: &Connection, id: &str, with_text: bool) -> AppResult<Vec<SourceRow>> {
    let mut statement = connection.prepare(if with_text {
        "SELECT kind, title, url, text FROM course_sources WHERE course_id = ?1 ORDER BY id"
    } else {
        "SELECT kind, title, url, '' FROM course_sources WHERE course_id = ?1 ORDER BY id"
    })?;
    let rows = statement
        .query_map([id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(kind, title, url, text)| {
            let kind = match kind.as_str() {
                "document" => SourceKind::Document,
                "web" => SourceKind::Web,
                other => return Err(unknown("source kind", other)),
            };
            Ok(SourceRow {
                source: StudySource { kind, title, url },
                text,
            })
        })
        .collect()
}

/// Retire les sources d'une sorte (résultats web d'un plan refait sans le
/// web).
pub fn delete_sources(connection: &Connection, id: &str, kind: SourceKind) -> AppResult<()> {
    connection.execute(
        "DELETE FROM course_sources WHERE course_id = ?1 AND kind = ?2",
        params![id, kind_text(kind)],
    )?;
    Ok(())
}

/// Cours entier, sauf le texte des sources (vide) ; `None` s'il n'existe
/// pas.
pub fn load(connection: &Connection, id: &str) -> AppResult<Option<Snapshot>> {
    load_with(connection, id, false)
}

/// Cours entier, texte des sources compris (génération).
pub fn load_full(connection: &Connection, id: &str) -> AppResult<Option<Snapshot>> {
    load_with(connection, id, true)
}

fn load_with(connection: &Connection, id: &str, with_text: bool) -> AppResult<Option<Snapshot>> {
    let Some(row) = row(connection, id)? else {
        return Ok(None);
    };
    Ok(Some(Snapshot {
        levels: levels(connection, id)?,
        answers: answers(connection, id)?,
        sources: sources(connection, id, with_text)?,
        row,
    }))
}

/// Tous les cours (sans le texte des sources), du plus récemment actif au
/// plus ancien.
pub fn load_all(connection: &Connection) -> AppResult<Vec<Snapshot>> {
    let ids = {
        let mut statement =
            connection.prepare("SELECT id FROM courses ORDER BY updated_at DESC, id DESC")?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut all = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(snapshot) = load(connection, &id)? {
            all.push(snapshot);
        }
    }
    Ok(all)
}

/// Supprime un cours (sources, paliers, réponses : en cascade) ; `false`
/// s'il n'existait pas.
pub fn delete(connection: &Connection, id: &str) -> AppResult<bool> {
    Ok(connection.execute("DELETE FROM courses WHERE id = ?1", [id])? == 1)
}

/// Étape et temps passé (sans autre changement).
pub fn progress(
    connection: &Connection,
    id: &str,
    position: &str,
    elapsed_ms: i64,
) -> AppResult<bool> {
    Ok(connection.execute(
        "UPDATE courses SET position = ?2, elapsed_ms = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, position, elapsed_ms.max(0), now_ms()],
    )? == 1)
}

/// Parties restées « en cours de génération » d'une session précédente :
/// remises en attente. Renvoie le nombre de parties remises.
pub fn reset_generating(connection: &Connection) -> AppResult<usize> {
    atomic(connection, |transaction| {
        let mut count = 0;
        let rows: Vec<(String, i64, String)> = {
            let mut statement =
                transaction.prepare("SELECT course_id, idx, data FROM course_levels")?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (id, index, text) in rows {
            let mut level: LevelData = from_json(&text)?;
            let mut changed = false;
            if level.state == GenState::Generating {
                level.state = GenState::Pending;
                changed = true;
            }
            if level.remedy == Some(GenState::Generating) {
                level.remedy = Some(GenState::Pending);
                changed = true;
            }
            if changed {
                count += 1;
                transaction.execute(
                    "UPDATE course_levels SET data = ?3 WHERE course_id = ?1 AND idx = ?2",
                    params![id, index, to_json(&level)?],
                )?;
            }
        }
        let finals: Vec<(String, String)> = {
            let mut statement =
                transaction.prepare("SELECT id, final FROM courses WHERE final IS NOT NULL")?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (id, text) in finals {
            let mut final_quiz: crate::study::data::FinalData = from_json(&text)?;
            if final_quiz.state == GenState::Generating {
                final_quiz.state = GenState::Pending;
                count += 1;
                transaction.execute(
                    "UPDATE courses SET final = ?2 WHERE id = ?1",
                    params![id, to_json(&final_quiz)?],
                )?;
            }
        }
        Ok(count)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::study::data::tests::{answered, level, row as course_row};
    use crate::study::data::{ClarifyState, FinalData};
    use crate::study::{StudyError, StudyErrorCode};

    fn store_with(id: &str) -> Store {
        let store = Store::open_in_memory().unwrap();
        let mut row = course_row(20);
        row.id = id.into();
        row.clarify = Some(ClarifyState {
            question: "Which?".into(),
            options: vec!["a".into(), "b".into()],
            answered: false,
            meaning: None,
        });
        row.error = Some(StudyError {
            code: StudyErrorCode::Failed,
            message: "m".into(),
        });
        let sources = vec![SourceRow {
            source: StudySource {
                kind: SourceKind::Document,
                title: "notes.md".into(),
                url: String::new(),
            },
            text: "Text".into(),
        }];
        store.blocking(|c| insert(c, &row, &sources)).unwrap();
        store
    }

    #[test]
    fn a_course_round_trips_through_the_database() {
        let store = store_with("c1");
        let snapshot = store.blocking(|c| load(c, "c1")).unwrap().unwrap();
        let mut expected = course_row(20);
        expected.id = "c1".into();
        assert_eq!(snapshot.row.topic, expected.topic);
        assert_eq!(snapshot.row.status, CourseStatus::Planning);
        assert_eq!(snapshot.row.clarify.as_ref().unwrap().options, ["a", "b"]);
        assert_eq!(
            snapshot.row.error.as_ref().unwrap().code,
            StudyErrorCode::Failed
        );
        assert_eq!(snapshot.sources[0].source.title, "notes.md");
        assert!(snapshot.levels.is_empty() && snapshot.answers.is_empty());
        assert_eq!(store.blocking(|c| load(c, "nope")).unwrap(), None);

        store
            .blocking(|c| put_levels(c, "c1", &[level(0, 1), level(1, 0)]))
            .unwrap();
        let changed = store
            .blocking(|c| {
                update_level(c, "c1", 1, |level| {
                    level.skipped = true;
                    Ok(7)
                })
            })
            .unwrap();
        assert_eq!(changed, Some(7));
        assert_eq!(
            store
                .blocking(|c| update_level(c, "c1", 5, |_| Ok(())))
                .unwrap(),
            None
        );
        store
            .blocking(|c| {
                update_row(c, "c1", |row| {
                    row.status = CourseStatus::Active;
                    row.final_quiz = Some(FinalData::default());
                    Ok(())
                })
            })
            .unwrap();
        assert_eq!(
            store.blocking(|c| update_row(c, "x", |_| Ok(()))).unwrap(),
            None
        );
        let snapshot = store.blocking(|c| load(c, "c1")).unwrap().unwrap();
        assert_eq!(snapshot.levels.len(), 2);
        assert!(snapshot.levels[1].skipped);
        assert_eq!(snapshot.row.status, CourseStatus::Active);
        assert_eq!(snapshot.row.final_quiz, Some(FinalData::default()));
        assert!(snapshot.row.updated_at > 1);
    }

    #[test]
    fn the_first_answer_is_final_and_answers_are_cleared_by_level() {
        let store = store_with("c1");
        let first = answered(true);
        assert!(
            store
                .blocking(|c| put_answer(c, "c1", "L1-A0-Q0", &first))
                .unwrap()
        );
        assert!(
            !store
                .blocking(|c| put_answer(c, "c1", "L1-A0-Q0", &answered(false)))
                .unwrap()
        );
        for question in ["L1-check", "L10-A0-Q0", "L_1-x", "F-Q0"] {
            store
                .blocking(|c| put_answer(c, "c1", question, &first))
                .unwrap();
        }
        store.blocking(|c| delete_answers(c, "c1", "L1-")).unwrap();
        let snapshot = store.blocking(|c| load(c, "c1")).unwrap().unwrap();
        // « L10- » et « L_1- » ne commencent pas par « L1- ».
        assert_eq!(
            snapshot.answers.keys().collect::<Vec<_>>(),
            ["F-Q0", "L10-A0-Q0", "L_1-x"]
        );
        // `_` et `%` sont des lettres, pas des jokers de LIKE.
        store.blocking(|c| delete_answers(c, "c1", "L_")).unwrap();
        let snapshot = store.blocking(|c| load(c, "c1")).unwrap().unwrap();
        assert_eq!(
            snapshot.answers.keys().collect::<Vec<_>>(),
            ["F-Q0", "L10-A0-Q0"]
        );
    }

    /// rust-16 : la liste et la lecture d'un cours ne lisent pas le texte
    /// des sources ; la génération, si.
    #[test]
    fn source_texts_are_read_only_for_the_generation() {
        let store = store_with("c1");
        let light = store.blocking(|c| load(c, "c1")).unwrap().unwrap();
        assert_eq!(light.sources[0].source.title, "notes.md");
        assert_eq!(light.sources[0].text, "");
        let all = store.blocking(|c| load_all(c)).unwrap();
        assert_eq!(all[0].sources[0].text, "");
        let full = store.blocking(|c| load_full(c, "c1")).unwrap().unwrap();
        assert_eq!(full.sources[0].text, "Text");
    }

    #[test]
    fn deleting_a_course_removes_everything() {
        let store = store_with("c1");
        store
            .blocking(|c| put_levels(c, "c1", &[level(0, 1)]))
            .unwrap();
        store
            .blocking(|c| put_answer(c, "c1", "L0-check", &answered(true)))
            .unwrap();
        assert!(store.blocking(|c| delete(c, "c1")).unwrap());
        assert!(!store.blocking(|c| delete(c, "c1")).unwrap());
        for table in ["course_sources", "course_levels", "course_answers"] {
            let count: i64 = store
                .blocking(|c| {
                    Ok(c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
                })
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
    }

    #[test]
    fn parts_left_generating_go_back_to_pending() {
        let store = store_with("c1");
        let mut generating = level(0, 1);
        generating.state = GenState::Generating;
        let mut remedy = level(1, 1);
        remedy.remedy = Some(GenState::Generating);
        let ready = level(2, 1);
        store
            .blocking(|c| put_levels(c, "c1", &[generating, remedy, ready.clone()]))
            .unwrap();
        store
            .blocking(|c| {
                update_row(c, "c1", |row| {
                    row.final_quiz = Some(FinalData {
                        state: GenState::Generating,
                        ..FinalData::default()
                    });
                    Ok(())
                })
            })
            .unwrap();
        assert_eq!(store.blocking(|c| reset_generating(c)).unwrap(), 3);
        let snapshot = store.blocking(|c| load(c, "c1")).unwrap().unwrap();
        assert_eq!(snapshot.levels[0].state, GenState::Pending);
        assert_eq!(snapshot.levels[1].remedy, Some(GenState::Pending));
        assert_eq!(snapshot.levels[2], ready);
        assert_eq!(snapshot.row.final_quiz.unwrap().state, GenState::Pending);
        assert_eq!(store.blocking(|c| reset_generating(c)).unwrap(), 0);
    }

    #[test]
    fn courses_are_listed_by_activity_and_progress_is_saved() {
        let store = store_with("c1");
        let mut second = course_row(10);
        second.id = "c2".into();
        second.updated_at = 5;
        store.blocking(|c| insert(c, &second, &[])).unwrap();
        let ids = |store: &Store| -> Vec<String> {
            store
                .blocking(|c| load_all(c))
                .unwrap()
                .into_iter()
                .map(|snapshot| snapshot.row.id)
                .collect()
        };
        assert_eq!(ids(&store), ["c2", "c1"]);
        assert!(store.blocking(|c| progress(c, "c1", "L0:s1", -5)).unwrap());
        assert!(!store.blocking(|c| progress(c, "zz", "x", 1)).unwrap());
        assert_eq!(ids(&store), ["c1", "c2"]);
        let row = store.blocking(|c| row(c, "c1")).unwrap().unwrap();
        assert_eq!((row.position.as_str(), row.elapsed_ms), ("L0:s1", 0));
        store
            .blocking(|c| {
                add_sources(
                    c,
                    "c1",
                    &[SourceRow {
                        source: StudySource {
                            kind: SourceKind::Web,
                            title: "W".into(),
                            url: "https://w".into(),
                        },
                        text: "x".into(),
                    }],
                )
            })
            .unwrap();
        store
            .blocking(|c| delete_sources(c, "c1", SourceKind::Document))
            .unwrap();
        let snapshot = store.blocking(|c| load(c, "c1")).unwrap().unwrap();
        assert_eq!(snapshot.sources.len(), 1);
        assert_eq!(snapshot.sources[0].source.kind, SourceKind::Web);
    }
}
