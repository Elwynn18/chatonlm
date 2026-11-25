//! Atelier (moteur de tâches complexes, propre à ChatonLM) : la part de la
//! base (migration 0050). Projets, pièces jointes (texte extrait), état
//! reprenable par clé, journal d'événements numéroté, questions posées à
//! l'utilisateur, et les trois réglages de l'Atelier (colonnes de la ligne
//! des réglages).
//!
//! Les énumérations (stratégie, autonomie, état, phase) passent ici en texte,
//! tel que leur JSON (`workshop::pipeline::types`, camelCase) : la base ne
//! dépend pas du moteur, le service convertit. Les valeurs JSON (événements,
//! état, questions, réponses) sont gardées telles quelles.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::error::{AppError, AppResult};

/// États où un projet a fini (date de fin posée).
const FINISHED: [&str; 3] = ["done", "failed", "cancelled"];

/// Projet qui commence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewProject {
    pub title: String,
    pub task: String,
    /// Stratégie imposée ; `None` : choisie par le projet.
    pub strategy: Option<String>,
    pub autonomy: String,
    /// Modèle demandé ; vide : choisi au lancement.
    pub model: String,
    pub language: String,
    pub folder: String,
    /// Nom du dossier donné par l'utilisateur ; faux : nom provisoire,
    /// remplacé par le titre du cadrage.
    pub folder_named: bool,
    /// État de départ (« queued » d'habitude).
    pub state: String,
    pub queue_seq: i64,
    pub created_at: i64,
}

/// Ligne d'un projet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectRow {
    pub id: i64,
    pub title: String,
    pub task: String,
    pub strategy: Option<String>,
    pub strategy_forced: bool,
    pub autonomy: String,
    pub model: String,
    pub model_used: String,
    pub language: String,
    pub state: String,
    pub phase: Option<String>,
    pub folder: String,
    /// Faux : nom provisoire du dossier, pas encore remplacé.
    pub folder_named: bool,
    pub queue_seq: i64,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

const PROJECT_COLUMNS: &str = "id, title, task, strategy, strategy_forced, autonomy, model,
    model_used, language, state, phase, folder, queue_seq, error, created_at, updated_at,
    finished_at, folder_named";

fn project(row: &Row) -> rusqlite::Result<ProjectRow> {
    Ok(ProjectRow {
        id: row.get(0)?,
        title: row.get(1)?,
        task: row.get(2)?,
        strategy: row.get(3)?,
        strategy_forced: row.get(4)?,
        autonomy: row.get(5)?,
        model: row.get(6)?,
        model_used: row.get(7)?,
        language: row.get(8)?,
        state: row.get(9)?,
        phase: row.get(10)?,
        folder: row.get(11)?,
        queue_seq: row.get(12)?,
        error: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
        finished_at: row.get(16)?,
        folder_named: row.get(17)?,
    })
}

/// Enregistre le projet et ses pièces jointes (nom, texte), dans l'ordre, en
/// une transaction ; rend son identifiant.
pub fn insert(
    connection: &mut Connection,
    new: &NewProject,
    attachments: &[(String, String)],
) -> AppResult<i64> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO workshop_projects
             (title, task, strategy, strategy_forced, autonomy, model, language, state,
              folder, queue_seq, created_at, updated_at, folder_named)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?12)",
        params![
            new.title,
            new.task,
            new.strategy,
            new.strategy.is_some(),
            new.autonomy,
            new.model,
            new.language,
            new.state,
            new.folder,
            new.queue_seq,
            new.created_at,
            new.folder_named
        ],
    )?;
    let id = transaction.last_insert_rowid();
    for (position, (filename, text)) in attachments.iter().enumerate() {
        transaction.execute(
            "INSERT INTO workshop_attachments (project_id, position, filename, text)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, position as i64, filename, text],
        )?;
    }
    transaction.commit()?;
    Ok(id)
}

pub fn get(connection: &Connection, id: i64) -> AppResult<Option<ProjectRow>> {
    Ok(connection
        .query_row(
            &format!("SELECT {PROJECT_COLUMNS} FROM workshop_projects WHERE id = ?1"),
            [id],
            project,
        )
        .optional()?)
}

/// Tous les projets, du plus récent au plus ancien.
pub fn list(connection: &Connection) -> AppResult<Vec<ProjectRow>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {PROJECT_COLUMNS} FROM workshop_projects ORDER BY created_at DESC, id DESC"
    ))?;
    let rows = statement
        .query_map([], project)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Projets dans l'état `state`, dans l'ordre de la file.
pub fn in_state(connection: &Connection, state: &str) -> AppResult<Vec<ProjectRow>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {PROJECT_COLUMNS} FROM workshop_projects WHERE state = ?1
         ORDER BY queue_seq, created_at, id"
    ))?;
    let rows = statement
        .query_map([state], project)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Change l'état (et le code d'erreur : effacé hors échec) ; la date de fin
/// est posée pour un état final, retirée sinon (projet repris). Faux : projet
/// introuvable.
pub fn set_state(
    connection: &Connection,
    id: i64,
    state: &str,
    error: Option<&str>,
    now: i64,
) -> AppResult<bool> {
    let finished = FINISHED.contains(&state).then_some(now);
    let changed = connection.execute(
        "UPDATE workshop_projects SET state = ?2, error = ?3, finished_at = ?4, updated_at = ?5
         WHERE id = ?1",
        params![id, state, error, finished, now],
    )?;
    Ok(changed > 0)
}

/// `state IN (?, ?…)` pour les états `from`, paramètres à partir de `first`.
fn state_in(from: &[&str], first: usize) -> String {
    let marks: Vec<String> = (0..from.len()).map(|i| format!("?{}", first + i)).collect();
    format!("state IN ({})", marks.join(", "))
}

/// Change l'état comme [`set_state`], mais SEULEMENT si le projet est dans
/// l'un des états `from` (vérification et écriture en une requête : une
/// pause et un lancement simultanés ne s'écrasent pas). Faux : rien changé.
pub fn set_state_from(
    connection: &Connection,
    id: i64,
    from: &[&str],
    state: &str,
    error: Option<&str>,
    now: i64,
) -> AppResult<bool> {
    let finished = FINISHED.contains(&state).then_some(now);
    let mut values: Vec<&dyn rusqlite::ToSql> = vec![&id, &state, &error, &finished, &now];
    for from in from {
        values.push(from);
    }
    let changed = connection.execute(
        &format!(
            "UPDATE workshop_projects SET state = ?2, error = ?3, finished_at = ?4, updated_at = ?5
             WHERE id = ?1 AND {}",
            state_in(from, 6)
        ),
        values.as_slice(),
    )?;
    Ok(changed > 0)
}

/// Phase commencée.
pub fn set_phase(connection: &Connection, id: i64, phase: &str, now: i64) -> AppResult<()> {
    connection.execute(
        "UPDATE workshop_projects SET phase = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, phase, now],
    )?;
    Ok(())
}

/// Stratégie choisie par le projet (sans effet sur une stratégie imposée).
pub fn set_strategy(connection: &Connection, id: i64, strategy: &str, now: i64) -> AppResult<()> {
    connection.execute(
        "UPDATE workshop_projects SET strategy = ?2, updated_at = ?3
         WHERE id = ?1 AND strategy_forced = 0",
        params![id, strategy, now],
    )?;
    Ok(())
}

/// Modèle retenu au lancement.
pub fn set_model_used(connection: &Connection, id: i64, model: &str, now: i64) -> AppResult<()> {
    connection.execute(
        "UPDATE workshop_projects SET model_used = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, model, now],
    )?;
    Ok(())
}

/// Titre donné par le cadrage ; `folder` : nouveau chemin du dossier, qui
/// devient alors définitif (plus jamais renommé).
pub fn set_title(
    connection: &Connection,
    id: i64,
    title: &str,
    folder: Option<&str>,
    now: i64,
) -> AppResult<()> {
    connection.execute(
        "UPDATE workshop_projects
         SET title = ?2, folder = COALESCE(?3, folder),
             folder_named = CASE WHEN ?3 IS NULL THEN folder_named ELSE 1 END, updated_at = ?4
         WHERE id = ?1",
        params![id, title, folder, now],
    )?;
    Ok(())
}

/// Remet le projet en file au rang `seq`, état « queued », SEULEMENT s'il
/// est dans l'un des états `from` (une seconde reprise est sans effet). Faux :
/// rien changé.
pub fn enqueue(
    connection: &Connection,
    id: i64,
    seq: i64,
    now: i64,
    from: &[&str],
) -> AppResult<bool> {
    let mut values: Vec<&dyn rusqlite::ToSql> = vec![&id, &seq, &now];
    for from in from {
        values.push(from);
    }
    let changed = connection.execute(
        &format!(
            "UPDATE workshop_projects
             SET state = 'queued', queue_seq = ?2, error = NULL, finished_at = NULL, updated_at = ?3
             WHERE id = ?1 AND {}",
            state_in(from, 4)
        ),
        values.as_slice(),
    )?;
    Ok(changed > 0)
}

/// Sort de la file tous les projets qui attendaient leur tour (« Erase my
/// data » : rien ne doit se lancer avant l'effacement ; base gardée : repris
/// d'un clic). Rend leurs identifiants.
pub fn pause_queue(connection: &Connection, now: i64) -> AppResult<Vec<i64>> {
    let mut statement = connection.prepare(
        "UPDATE workshop_projects SET state = 'paused', updated_at = ?1
         WHERE state = 'queued' RETURNING id",
    )?;
    let ids = statement
        .query_map([now], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

/// Plus grand rang de file donné (0 : aucun).
pub fn max_queue_seq(connection: &Connection) -> AppResult<i64> {
    Ok(connection.query_row(
        "SELECT COALESCE(MAX(queue_seq), 0) FROM workshop_projects",
        [],
        |row| row.get(0),
    )?)
}

/// Supprime le projet et tout ce qui en dépend ; faux s'il n'existait pas.
pub fn delete(connection: &Connection, id: i64) -> AppResult<bool> {
    Ok(connection.execute("DELETE FROM workshop_projects WHERE id = ?1", [id])? > 0)
}

/// Pièces jointes (nom, texte), dans l'ordre.
pub fn attachments(connection: &Connection, id: i64) -> AppResult<Vec<(String, String)>> {
    let mut statement = connection.prepare(
        "SELECT filename, text FROM workshop_attachments WHERE project_id = ?1 ORDER BY position",
    )?;
    let rows = statement
        .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Écrit l'état `key` (JSON) du projet, à la place de l'ancien.
pub fn save_state(
    connection: &Connection,
    id: i64,
    key: &str,
    value: &str,
    now: i64,
) -> AppResult<()> {
    connection.execute(
        "INSERT INTO workshop_state (project_id, key, value, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (project_id, key) DO UPDATE SET value = ?3, updated_at = ?4",
        params![id, key, value, now],
    )?;
    Ok(())
}

pub fn load_state(connection: &Connection, id: i64, key: &str) -> AppResult<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT value FROM workshop_state WHERE project_id = ?1 AND key = ?2",
            params![id, key],
            |row| row.get(0),
        )
        .optional()?)
}

/// Événement enregistré : numéro, date, JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRow {
    pub seq: u64,
    pub at: i64,
    pub event: String,
}

/// Ajoute un événement au journal du projet ; rend son numéro (le suivant du
/// dernier, 1 pour le premier).
pub fn append_event(connection: &Connection, id: i64, at: i64, event: &str) -> AppResult<u64> {
    let seq: i64 = connection.query_row(
        "INSERT INTO workshop_events (project_id, seq, at, event)
         SELECT ?1, COALESCE(MAX(seq), 0) + 1, ?2, ?3 FROM workshop_events WHERE project_id = ?1
         RETURNING seq",
        params![id, at, event],
        |row| row.get(0),
    )?;
    u64::try_from(seq).map_err(|_| AppError::Internal(format!("bad event number: {seq}")))
}

/// Événements de numéro strictement plus grand que `since`, dans l'ordre, au
/// plus `limit`.
pub fn events_since(
    connection: &Connection,
    id: i64,
    since: u64,
    limit: usize,
) -> AppResult<Vec<EventRow>> {
    let since = i64::try_from(since).unwrap_or(i64::MAX);
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut statement = connection.prepare(
        "SELECT seq, at, event FROM workshop_events WHERE project_id = ?1 AND seq > ?2
         ORDER BY seq LIMIT ?3",
    )?;
    let rows = statement
        .query_map(params![id, since, limit], |row| {
            Ok(EventRow {
                seq: row.get::<_, i64>(0)?.max(0) as u64,
                at: row.get(1)?,
                event: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Dernier numéro d'événement du projet (0 : aucun).
pub fn last_seq(connection: &Connection, id: i64) -> AppResult<u64> {
    let seq: i64 = connection.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM workshop_events WHERE project_id = ?1",
        [id],
        |row| row.get(0),
    )?;
    Ok(seq.max(0) as u64)
}

/// Question posée.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionRow {
    pub id: i64,
    pub question: String,
    pub answer: Option<String>,
    pub status: String,
    pub created_at: i64,
}

/// Enregistre une question en attente ; rend son identifiant.
pub fn add_question(connection: &Connection, id: i64, question: &str, now: i64) -> AppResult<i64> {
    connection.execute(
        "INSERT INTO workshop_questions (project_id, question, created_at) VALUES (?1, ?2, ?3)",
        params![id, question, now],
    )?;
    Ok(connection.last_insert_rowid())
}

fn question(row: &Row) -> rusqlite::Result<QuestionRow> {
    Ok(QuestionRow {
        id: row.get(0)?,
        question: row.get(1)?,
        answer: row.get(2)?,
        status: row.get(3)?,
        created_at: row.get(4)?,
    })
}

/// Question en attente du projet (la plus récente), s'il y en a une.
pub fn pending_question(connection: &Connection, id: i64) -> AppResult<Option<QuestionRow>> {
    Ok(connection
        .query_row(
            "SELECT id, question, answer, status, created_at FROM workshop_questions
             WHERE project_id = ?1 AND status = 'pending' ORDER BY id DESC LIMIT 1",
            [id],
            question,
        )
        .optional()?)
}

/// Question `question_id` du projet.
pub fn get_question(
    connection: &Connection,
    id: i64,
    question_id: i64,
) -> AppResult<Option<QuestionRow>> {
    Ok(connection
        .query_row(
            "SELECT id, question, answer, status, created_at FROM workshop_questions
             WHERE project_id = ?1 AND id = ?2",
            params![id, question_id],
            question,
        )
        .optional()?)
}

/// Répond à une question EN ATTENTE ; faux si elle ne l'est pas (déjà
/// répondue, abandonnée, d'un autre projet ou inconnue).
pub fn answer_question(
    connection: &Connection,
    id: i64,
    question_id: i64,
    answer: &str,
    now: i64,
) -> AppResult<bool> {
    let changed = connection.execute(
        "UPDATE workshop_questions SET answer = ?3, status = 'answered', answered_at = ?4
         WHERE project_id = ?1 AND id = ?2 AND status = 'pending'",
        params![id, question_id, answer, now],
    )?;
    Ok(changed > 0)
}

/// Réponse déjà donnée (pendant une absence du projet : app relancée, projet
/// repris) à une question identique à `question` : marquée lue et rendue ;
/// `None` sinon.
pub fn take_answer(connection: &Connection, id: i64, question: &str) -> AppResult<Option<String>> {
    let found: Option<(i64, String)> = connection
        .query_row(
            "SELECT id, answer FROM workshop_questions
             WHERE project_id = ?1 AND status = 'answered' AND question = ?2 AND answer IS NOT NULL
             ORDER BY id DESC LIMIT 1",
            params![id, question],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((question_id, answer)) = found else {
        return Ok(None);
    };
    connection.execute(
        "UPDATE workshop_questions SET status = 'used' WHERE id = ?1",
        [question_id],
    )?;
    Ok(Some(answer))
}

/// Marque lue la question `question_id`.
pub fn use_answer(connection: &Connection, question_id: i64) -> AppResult<()> {
    connection.execute(
        "UPDATE workshop_questions SET status = 'used' WHERE id = ?1 AND status = 'answered'",
        [question_id],
    )?;
    Ok(())
}

/// Question EN ATTENTE identique à `question` (projet relancé qui la pose de
/// nouveau) : son identifiant.
pub fn same_pending(connection: &Connection, id: i64, question: &str) -> AppResult<Option<i64>> {
    Ok(connection
        .query_row(
            "SELECT id FROM workshop_questions
             WHERE project_id = ?1 AND status = 'pending' AND question = ?2
             ORDER BY id DESC LIMIT 1",
            params![id, question],
            |row| row.get(0),
        )
        .optional()?)
}

/// Abandonne les questions en attente du projet (arrêté pendant l'attente :
/// sa reprise les posera de nouveau) ; les réponses données restent.
pub fn drop_pending(connection: &Connection, id: i64) -> AppResult<()> {
    connection.execute(
        "UPDATE workshop_questions SET status = 'dropped'
         WHERE project_id = ?1 AND status = 'pending'",
        [id],
    )?;
    Ok(())
}

/// Abandonne les questions en attente ou répondues non lues du projet.
pub fn drop_questions(connection: &Connection, id: i64) -> AppResult<()> {
    connection.execute(
        "UPDATE workshop_questions SET status = 'dropped'
         WHERE project_id = ?1 AND status IN ('pending', 'answered')",
        [id],
    )?;
    Ok(())
}

/// Lancement de l'app : rien ne calcule par surprise. Un projet qui
/// calculait, cédait la place ou attendait son tour passe en pause (reprise
/// d'un clic) ; un projet qui attendait une réponse la garde s'il a encore
/// une question en attente, sinon il passe en pause aussi. Rend les
/// identifiants mis en pause.
pub fn recover(connection: &mut Connection, now: i64) -> AppResult<Vec<i64>> {
    let transaction = connection.transaction()?;
    let paused: Vec<i64> = {
        let mut statement = transaction.prepare(
            "UPDATE workshop_projects SET state = 'paused', updated_at = ?1
             WHERE state IN ('running', 'yielding', 'queued')
                OR (state = 'waiting' AND NOT EXISTS (
                        SELECT 1 FROM workshop_questions q
                        WHERE q.project_id = workshop_projects.id AND q.status = 'pending'))
             RETURNING id",
        )?;
        statement
            .query_map([now], |row| row.get(0))?
            .collect::<Result<_, _>>()?
    };
    transaction.commit()?;
    Ok(paused)
}

/// Réglages de l'Atelier (ligne des réglages de l'app).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsRow {
    /// Dossier des projets ; vide : celui par défaut.
    pub dir: String,
    pub web: bool,
    pub autonomy: String,
}

pub fn settings(connection: &Connection) -> AppResult<SettingsRow> {
    Ok(connection.query_row(
        "SELECT workshop_dir, workshop_web, workshop_autonomy FROM settings WHERE id = 1",
        [],
        |row| {
            Ok(SettingsRow {
                dir: row.get(0)?,
                web: row.get(1)?,
                autonomy: row.get(2)?,
            })
        },
    )?)
}

/// Change les réglages donnés (les autres restent).
pub fn set_settings(
    connection: &Connection,
    dir: Option<&str>,
    web: Option<bool>,
    autonomy: Option<&str>,
) -> AppResult<SettingsRow> {
    if let Some(dir) = dir {
        connection.execute("UPDATE settings SET workshop_dir = ?1 WHERE id = 1", [dir])?;
    }
    if let Some(web) = web {
        connection.execute("UPDATE settings SET workshop_web = ?1 WHERE id = 1", [web])?;
    }
    if let Some(autonomy) = autonomy {
        connection.execute(
            "UPDATE settings SET workshop_autonomy = ?1 WHERE id = 1",
            [autonomy],
        )?;
    }
    settings(connection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn new(title: &str, created_at: i64) -> NewProject {
        NewProject {
            title: title.into(),
            task: "Write a guide".into(),
            strategy: None,
            autonomy: "ask".into(),
            model: String::new(),
            language: "en".into(),
            folder: format!("/projects/{title}"),
            folder_named: true,
            state: "queued".into(),
            queue_seq: created_at,
            created_at,
        }
    }

    /// Base avec ces projets, numérotés 1, 2… dans l'ordre.
    fn store_with(projects: &[NewProject]) -> Store {
        let store = Store::open_in_memory().unwrap();
        let projects = projects.to_vec();
        store
            .blocking(move |c| {
                for (index, project) in projects.iter().enumerate() {
                    assert_eq!(insert(c, project, &[])?, index as i64 + 1);
                }
                Ok(())
            })
            .unwrap();
        store
    }

    #[test]
    fn a_project_and_its_attachments_are_saved_in_order() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                let mut project = new("a", 10);
                project.strategy = Some("longForm".into());
                let id = insert(
                    c,
                    &project,
                    &[
                        ("b.md".into(), "two".into()),
                        ("a.txt".into(), "one".into()),
                    ],
                )?;
                let row = get(c, id)?.unwrap();
                assert_eq!(row.title, "a");
                assert_eq!(row.strategy.as_deref(), Some("longForm"));
                assert!(row.strategy_forced);
                assert_eq!(row.state, "queued");
                assert_eq!(row.phase, None);
                assert_eq!((row.created_at, row.updated_at), (10, 10));
                assert_eq!(
                    attachments(c, id)?,
                    vec![
                        ("b.md".into(), "two".into()),
                        ("a.txt".into(), "one".into())
                    ]
                );
                assert!(get(c, id + 1)?.is_none());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_deleted_number_is_never_given_again() {
        let store = store_with(&[new("a", 1), new("b", 2)]);
        store
            .blocking(|c| {
                assert!(delete(c, 2)?);
                assert_eq!(insert(c, &new("c", 3), &[])?, 3);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_forced_strategy_is_kept_and_a_chosen_one_recorded() {
        let mut forced = new("forced", 1);
        forced.strategy = Some("generic".into());
        let store = store_with(&[forced, new("free", 2)]);
        store
            .blocking(|c| {
                set_strategy(c, 1, "longForm", 5)?;
                set_strategy(c, 2, "longForm", 5)?;
                assert_eq!(get(c, 1)?.unwrap().strategy.as_deref(), Some("generic"));
                assert_eq!(get(c, 2)?.unwrap().strategy.as_deref(), Some("longForm"));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn projects_are_listed_newest_first_and_the_queue_in_order() {
        let mut late = new("late", 30);
        late.queue_seq = 1;
        let store = store_with(&[new("first", 10), late, new("second", 20)]);
        store
            .blocking(|c| {
                let titles: Vec<String> = list(c)?.into_iter().map(|p| p.title).collect();
                assert_eq!(titles, ["late", "second", "first"]);
                let queue: Vec<String> = in_state(c, "queued")?
                    .into_iter()
                    .map(|p| p.title)
                    .collect();
                assert_eq!(queue, ["late", "first", "second"]);
                assert_eq!(max_queue_seq(c)?, 20);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_final_state_records_the_end_and_a_resumed_project_loses_it() {
        let store = store_with(&[new("a", 1)]);
        store
            .blocking(|c| {
                assert!(set_state(c, 1, "failed", Some("noModel"), 50)?);
                let row = get(c, 1)?.unwrap();
                assert_eq!(row.error.as_deref(), Some("noModel"));
                assert_eq!(row.finished_at, Some(50));
                assert!(enqueue(c, 1, 7, 60, &["paused", "failed"])?);
                let row = get(c, 1)?.unwrap();
                assert_eq!((row.state.as_str(), row.queue_seq), ("queued", 7));
                assert_eq!((row.error, row.finished_at), (None, None));
                assert!(set_state(c, 1, "running", None, 70)?);
                assert_eq!(get(c, 1)?.unwrap().finished_at, None);
                assert!(!set_state(c, 9, "done", None, 70)?);
                Ok(())
            })
            .unwrap();
    }

    /// Relecture du lot 2 : file et arrêts atomiques. Une écriture qui ne
    /// part pas de l'état attendu ne change rien (projet lancé entre-temps,
    /// seconde reprise).
    #[test]
    fn transitions_happen_only_from_the_expected_states() {
        let store = store_with(&[new("a", 1), new("b", 2)]);
        store
            .blocking(|c| {
                assert!(set_state(c, 1, "running", None, 5)?);
                // Mis en pause « en file » alors qu'il vient d'être lancé : rien.
                assert!(!set_state_from(
                    c,
                    1,
                    &["queued", "waiting"],
                    "paused",
                    None,
                    6
                )?);
                assert_eq!(get(c, 1)?.unwrap().state, "running");
                assert!(!enqueue(c, 1, 9, 6, &["paused", "failed"])?);
                assert_eq!(get(c, 1)?.unwrap().queue_seq, 1);
                // Depuis le bon état : écrit, une seule fois.
                assert!(set_state_from(
                    c,
                    2,
                    &["queued", "waiting"],
                    "paused",
                    None,
                    7
                )?);
                assert!(enqueue(c, 2, 9, 8, &["paused"])?);
                assert!(!enqueue(c, 2, 10, 9, &["paused"])?, "second resume");
                assert_eq!(get(c, 2)?.unwrap().queue_seq, 9);
                assert!(set_state_from(c, 2, &["queued"], "cancelled", None, 10)?);
                assert_eq!(get(c, 2)?.unwrap().finished_at, Some(10));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn the_queue_can_be_emptied_at_once() {
        let store = store_with(&[new("a", 1), new("b", 2), new("c", 3)]);
        store
            .blocking(|c| {
                set_state(c, 2, "running", None, 4)?;
                assert_eq!(pause_queue(c, 5)?, [1, 3]);
                assert_eq!(get(c, 1)?.unwrap().state, "paused");
                assert_eq!(get(c, 2)?.unwrap().state, "running");
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn an_unknown_state_is_refused_by_the_database() {
        let store = store_with(&[new("a", 1)]);
        let refused = store.blocking(|c| set_state(c, 1, "sleeping", None, 2));
        assert!(refused.is_err());
    }

    #[test]
    fn state_is_saved_by_key_and_replaced() {
        let store = store_with(&[new("a", 1), new("b", 2)]);
        store
            .blocking(|c| {
                assert_eq!(load_state(c, 1, "spec")?, None);
                save_state(c, 1, "spec", r#"{"goal":"x"}"#, 3)?;
                save_state(c, 1, "spec", r#"{"goal":"y"}"#, 4)?;
                save_state(c, 2, "spec", r#"{"goal":"z"}"#, 4)?;
                assert_eq!(
                    load_state(c, 1, "spec")?.as_deref(),
                    Some(r#"{"goal":"y"}"#)
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn events_are_numbered_per_project_and_read_from_a_number() {
        let store = store_with(&[new("a", 1), new("b", 2)]);
        store
            .blocking(|c| {
                assert_eq!(last_seq(c, 1)?, 0);
                assert_eq!(append_event(c, 1, 10, "\"one\"")?, 1);
                assert_eq!(append_event(c, 2, 11, "\"other\"")?, 1);
                assert_eq!(append_event(c, 1, 12, "\"two\"")?, 2);
                assert_eq!(append_event(c, 1, 13, "\"three\"")?, 3);
                assert_eq!(last_seq(c, 1)?, 3);
                let read = events_since(c, 1, 1, 10)?;
                assert_eq!(
                    read,
                    vec![
                        EventRow {
                            seq: 2,
                            at: 12,
                            event: "\"two\"".into()
                        },
                        EventRow {
                            seq: 3,
                            at: 13,
                            event: "\"three\"".into()
                        },
                    ]
                );
                assert_eq!(events_since(c, 1, 0, 1)?.len(), 1);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn deleting_a_project_deletes_everything_it_owns() {
        let store = store_with(&[new("a", 1), new("b", 2)]);
        store
            .blocking(|c| {
                c.execute(
                    "INSERT INTO workshop_attachments (project_id, position, filename, text)
                     VALUES (1, 0, 'x', 'y')",
                    [],
                )?;
                save_state(c, 1, "plan", "{}", 3)?;
                append_event(c, 1, 3, "{}")?;
                add_question(c, 1, "{}", 3)?;
                append_event(c, 2, 3, "{}")?;
                assert!(delete(c, 1)?);
                assert!(!delete(c, 1)?);
                for table in [
                    "workshop_attachments",
                    "workshop_state",
                    "workshop_events",
                    "workshop_questions",
                ] {
                    let left: i64 = c.query_row(
                        &format!("SELECT COUNT(*) FROM {table} WHERE project_id = 1"),
                        [],
                        |row| row.get(0),
                    )?;
                    assert_eq!(left, 0, "{table}");
                }
                assert_eq!(last_seq(c, 2)?, 1);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_question_is_answered_once_then_used() {
        let store = store_with(&[new("a", 1), new("b", 2)]);
        store
            .blocking(|c| {
                let id = add_question(c, 1, r#"{"framing":["q"]}"#, 5)?;
                let pending = pending_question(c, 1)?.unwrap();
                assert_eq!((pending.id, pending.status.as_str()), (id, "pending"));
                // Autre projet : refusé.
                assert!(!answer_question(c, 2, id, "{}", 6)?);
                assert!(answer_question(c, 1, id, r#"{"framing":["a"]}"#, 6)?);
                assert!(!answer_question(c, 1, id, "{}", 7)?, "answered once");
                assert!(pending_question(c, 1)?.is_none());
                let answered = get_question(c, 1, id)?.unwrap();
                assert_eq!(answered.answer.as_deref(), Some(r#"{"framing":["a"]}"#));
                use_answer(c, id)?;
                assert_eq!(get_question(c, 1, id)?.unwrap().status, "used");
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn an_answer_given_while_away_is_taken_by_the_same_question_only() {
        let store = store_with(&[new("a", 1)]);
        store
            .blocking(|c| {
                let id = add_question(c, 1, r#"{"framing":["q"]}"#, 5)?;
                assert!(answer_question(c, 1, id, r#"{"framing":["a"]}"#, 6)?);
                assert_eq!(take_answer(c, 1, r#"{"framing":["other"]}"#)?, None);
                assert_eq!(
                    take_answer(c, 1, r#"{"framing":["q"]}"#)?.as_deref(),
                    Some(r#"{"framing":["a"]}"#)
                );
                assert_eq!(take_answer(c, 1, r#"{"framing":["q"]}"#)?, None, "used");
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_question_asked_again_finds_the_pending_one_until_dropped() {
        let store = store_with(&[new("a", 1)]);
        store
            .blocking(|c| {
                let pending = add_question(c, 1, "\"q\"", 5)?;
                let answered = add_question(c, 1, "\"r\"", 5)?;
                answer_question(c, 1, answered, "true", 6)?;
                assert_eq!(same_pending(c, 1, "\"q\"")?, Some(pending));
                assert_eq!(same_pending(c, 1, "\"other\"")?, None);
                assert_eq!(same_pending(c, 1, "\"r\"")?, None, "answered");
                drop_pending(c, 1)?;
                assert_eq!(same_pending(c, 1, "\"q\"")?, None);
                // La réponse donnée reste à prendre.
                assert_eq!(take_answer(c, 1, "\"r\"")?.as_deref(), Some("true"));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn dropped_questions_are_neither_pending_nor_taken() {
        let store = store_with(&[new("a", 1)]);
        store
            .blocking(|c| {
                add_question(c, 1, "\"pending\"", 5)?;
                let answered = add_question(c, 1, "\"answered\"", 5)?;
                answer_question(c, 1, answered, "true", 6)?;
                drop_questions(c, 1)?;
                assert!(pending_question(c, 1)?.is_none());
                assert_eq!(take_answer(c, 1, "\"answered\"")?, None);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn at_launch_nothing_computes_and_a_pending_question_keeps_waiting() {
        // (titre, état) ; numéros 1 à 9 dans l'ordre.
        let states = [
            ("running", "running"),
            ("yielding", "yielding"),
            ("queued", "queued"),
            ("asking", "waiting"),
            ("waited", "waiting"),
            ("paused", "paused"),
            ("done", "done"),
            ("failed", "failed"),
            ("cancelled", "cancelled"),
        ];
        let projects: Vec<NewProject> = states
            .iter()
            .enumerate()
            .map(|(index, (title, state))| {
                let mut project = new(title, index as i64);
                project.state = (*state).into();
                project
            })
            .collect();
        let store = store_with(&projects);
        store
            .blocking(|c| {
                add_question(c, 4, "{}", 1)?;
                let answered = add_question(c, 5, "{}", 1)?;
                answer_question(c, 5, answered, "{}", 2)?;
                let mut paused = recover(c, 100)?;
                paused.sort_unstable();
                assert_eq!(paused, [1, 2, 3, 5]);
                let state = |id: i64| get(c, id).map(|row| row.unwrap().state);
                assert_eq!(state(4)?, "waiting");
                assert_eq!(state(1)?, "paused");
                assert_eq!(state(5)?, "paused");
                assert_eq!(state(6)?, "paused");
                assert_eq!(state(7)?, "done");
                assert_eq!(state(8)?, "failed");
                assert_eq!(state(9)?, "cancelled");
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn settings_have_defaults_and_change_one_by_one() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|c| {
                assert_eq!(
                    settings(c)?,
                    SettingsRow {
                        dir: String::new(),
                        web: true,
                        autonomy: "ask".into()
                    }
                );
                set_settings(c, None, Some(false), None)?;
                let changed = set_settings(c, Some("/home/me/Projets"), None, Some("autonomous"))?;
                assert_eq!(
                    changed,
                    SettingsRow {
                        dir: "/home/me/Projets".into(),
                        web: false,
                        autonomy: "autonomous".into()
                    }
                );
                assert!(set_settings(c, None, None, Some("reckless")).is_err());
                Ok(())
            })
            .unwrap();
    }
}
