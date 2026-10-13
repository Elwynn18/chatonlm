//! Moteur des cours : file unique de génération (une partie en JSON à la
//! fois, tous cours confondus), réponses en flux, et opérations des
//! commandes `study_*`.
//!
//! Priorité de la file : nouvel essai > plan > palier du cours le plus
//! récemment lu (`study_get`) > autres paliers > bilan ; à priorité égale,
//! le palier de plus petit rang, puis l'ordre d'arrivée. La suppression ou
//! le nouveau plan d'un cours retire ses parties de la file et arrête celle
//! en cours. Les flux (« I don't understand », « Ask a question ») partent
//! aussitôt, hors file.
//!
//! Pause d'un cours (`set_paused`, enregistrée) : sa partie en cours est
//! arrêtée (elle sera refaite), ses parties quittent la file et n'y
//! reviennent pas tant qu'il n'est pas repris ; les autres cours continuent.
//! Une demande de l'apprenant qui exige une génération (nouvel essai, palier
//! refait, nouveau plan, précision du sujet, « Retry ») reprend le cours.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tauri::ipc::Channel;

use super::data::{self, CourseRow, LevelData, Snapshot, StoredQuestion};
use super::generate::{self, Job, Part};
use super::request::{Gen, Stop, TIMEOUTS, Timeouts};
use super::{
    AnswerValue, Answered, Ask, Course, CourseStatus, CourseSummary, Explain, GenState, Lesson,
    LevelStatus, MAX_DOCUMENTS, MAX_QUESTION_CHARS, MAX_TOPIC_CHARS, NewCourse, SourceKind,
    StudyStreamEvent, StudyUpdated, grade, prompts, sources,
};
use crate::attachments::Staging;
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::proofread::Cancel;
use crate::store::{Store, now_ms, study as db};

/// Émet `study-updated`.
pub type Notify = Arc<dyn Fn(StudyUpdated) + Send + Sync>;

/// Base et évènements d'un appel (et des travaux qu'il met en file).
#[derive(Clone)]
pub struct Ctx {
    pub store: Store,
    pub notify: Notify,
}

impl Ctx {
    pub fn new(store: Store, notify: impl Fn(StudyUpdated) + Send + Sync + 'static) -> Self {
        Self {
            store,
            notify: Arc::new(notify),
        }
    }

    fn changed(&self, course: &str) {
        (self.notify)(StudyUpdated {
            course_id: course.to_owned(),
            deleted: false,
        });
    }
}

#[derive(Clone, Debug)]
struct Queued {
    course: String,
    part: Part,
    seq: u64,
}

#[derive(Default)]
struct Queue {
    jobs: Vec<Queued>,
    /// Partie en cours et son arrêt.
    running: Option<(String, Part, Cancel)>,
    /// Une tâche lit la file.
    worker: bool,
    /// Cours le plus récemment lu.
    last_read: Option<String>,
    seq: u64,
}

impl Queue {
    fn rank(&self, job: &Queued) -> (u8, usize, u64) {
        match job.part {
            Part::Remedy(_) => (0, 0, job.seq),
            Part::Plan => (1, 0, job.seq),
            Part::Level(index) if self.last_read.as_deref() == Some(job.course.as_str()) => {
                (2, index, job.seq)
            }
            Part::Level(index) => (3, index, job.seq),
            // Bilan du cours lu : après ses paliers, avant ceux des autres.
            Part::Final if self.last_read.as_deref() == Some(job.course.as_str()) => {
                (2, usize::MAX, job.seq)
            }
            Part::Final => (4, 0, job.seq),
        }
    }

    fn pick(&mut self) -> Option<Queued> {
        let best = (0..self.jobs.len()).min_by_key(|index| self.rank(&self.jobs[*index]))?;
        Some(self.jobs.remove(best))
    }

    fn has(&self, course: &str, part: &Part) -> bool {
        self.jobs
            .iter()
            .any(|job| job.course == course && job.part == *part)
            // Une partie arrêtée (palier refait, nouveau plan) peut revenir
            // en file avant la fin de sa requête.
            || self.running.as_ref().is_some_and(|(c, p, cancel)| {
                c == course && p == part && !cancel.is_cancelled()
            })
    }
}

/// Flux en cours.
struct StreamEntry {
    course: String,
    /// Palier dont la leçon est expliquée ou questionnée.
    level: usize,
    cancel: Cancel,
    /// Distingue deux flux inscrits sous le même identifiant.
    token: u64,
}

struct Shared {
    queue: Mutex<Queue>,
    /// Flux en cours, par identifiant (choisi par la page).
    streams: Mutex<HashMap<String, StreamEntry>>,
    tokens: std::sync::atomic::AtomicU64,
    /// Parties « en cours » d'une session précédente remises en attente.
    recovered: tokio::sync::OnceCell<()>,
    timeouts: Timeouts,
}

/// Moteur des cours (état géré par Tauri).
pub struct StudyEngine {
    shared: Arc<Shared>,
}

impl Default for StudyEngine {
    fn default() -> Self {
        Self::with_timeouts(TIMEOUTS)
    }
}

fn not_found() -> AppError {
    AppError::NotFound(t!("study.notFound"))
}

fn unavailable() -> AppError {
    AppError::Invalid(t!("study.unavailable"))
}

/// Panique dans la tâche de la file : la file reprendra au prochain ajout.
struct WorkerGuard(Arc<Shared>);

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let mut queue = self.0.queue.lock();
            queue.running = None;
            queue.worker = false;
        }
    }
}

impl StudyEngine {
    pub fn with_timeouts(timeouts: Timeouts) -> Self {
        Self {
            shared: Arc::new(Shared {
                queue: Mutex::new(Queue::default()),
                streams: Mutex::new(HashMap::new()),
                tokens: std::sync::atomic::AtomicU64::new(0),
                recovered: tokio::sync::OnceCell::new(),
                timeouts,
            }),
        }
    }

    /// Premier accès de la session à la base des cours : les parties
    /// « en cours » d'une session précédente redeviennent « en attente ».
    async fn ready(&self, store: &Store) -> AppResult<()> {
        self.shared
            .recovered
            .get_or_try_init(|| async { store.call(|c| db::reset_generating(c)).await.map(drop) })
            .await
            .map(drop)
    }

    // -------------------------------------------------------------- file

    /// Met une partie en file (une seule fois) ; lance la tâche de la file
    /// si elle ne tourne pas.
    fn enqueue(&self, ctx: &Ctx, course: &str, part: Part) {
        let start = {
            let mut queue = self.shared.queue.lock();
            if !queue.has(course, &part) {
                queue.seq += 1;
                let seq = queue.seq;
                queue.jobs.push(Queued {
                    course: course.to_owned(),
                    part,
                    seq,
                });
            }
            !std::mem::replace(&mut queue.worker, true)
        };
        if start {
            let (shared, ctx) = (self.shared.clone(), ctx.clone());
            tokio::spawn(work(shared, ctx));
        }
    }

    /// Retire les parties d'un cours de la file et arrête celle en cours ;
    /// `part` : seulement celle-là.
    fn cancel(&self, course: &str, part: Option<&Part>) {
        let mut queue = self.shared.queue.lock();
        let concerned = |c: &str, p: &Part| c == course && part.is_none_or(|part| part == p);
        queue.jobs.retain(|job| !concerned(&job.course, &job.part));
        if let Some((c, p, cancel)) = &queue.running
            && concerned(c, p)
        {
            cancel.cancel();
        }
    }

    /// Met en file ce qui manque à un cours : plan en préparation, palier 0
    /// d'un plan prêt, tous les paliers en attente d'un cours commencé,
    /// nouveaux essais demandés, bilan (paliers tous prêts). Rien pour un
    /// cours en pause.
    fn resume(&self, ctx: &Ctx, snapshot: &Snapshot) {
        if snapshot.row.paused {
            return;
        }
        let course = snapshot.row.id.as_str();
        match snapshot.row.status {
            CourseStatus::Planning => self.enqueue(ctx, course, Part::Plan),
            CourseStatus::Planned => {
                if snapshot
                    .levels
                    .first()
                    .is_some_and(|l| l.state == GenState::Pending)
                {
                    self.enqueue(ctx, course, Part::Level(0));
                }
            }
            CourseStatus::Active | CourseStatus::Finished => {
                for (index, level) in snapshot.levels.iter().enumerate() {
                    if level.state == GenState::Pending {
                        self.enqueue(ctx, course, Part::Level(index));
                    }
                    if level.remedy == Some(GenState::Pending) {
                        self.enqueue(ctx, course, Part::Remedy(index));
                    }
                }
                if generate::final_wanted(snapshot) {
                    self.enqueue(ctx, course, Part::Final);
                }
            }
            CourseStatus::Unclear | CourseStatus::Failed => {}
        }
    }

    async fn load(&self, ctx: &Ctx, course: &str) -> AppResult<Snapshot> {
        let id = course.to_owned();
        ctx.store
            .call(move |c| db::load(c, &id))
            .await?
            .ok_or_else(not_found)
    }

    /// Change un cours dans un seul appel à la base (`change` reçoit la
    /// connexion et le cours relu) ; `study-updated` émis, puis ce qui manque
    /// mis en file.
    async fn change<T: Send + 'static>(
        &self,
        ctx: &Ctx,
        course: &str,
        change: impl FnOnce(&rusqlite::Connection, Snapshot) -> AppResult<T> + Send + 'static,
    ) -> AppResult<T> {
        self.ready(&ctx.store).await?;
        let id = course.to_owned();
        let (result, snapshot) = ctx
            .store
            // Tout ou rien (palier remis à zéro et réponses effacées…).
            .call(move |c| {
                db::atomic(c, |c| {
                    let snapshot = db::load(c, &id)?.ok_or_else(not_found)?;
                    let result = change(c, snapshot)?;
                    let snapshot = db::load(c, &id)?.ok_or_else(not_found)?;
                    Ok((result, snapshot))
                })
            })
            .await?;
        ctx.changed(course);
        self.resume(ctx, &snapshot);
        Ok(result)
    }

    // -------------------------------------------------------------- lecture

    pub async fn list(&self, store: &Store) -> AppResult<Vec<CourseSummary>> {
        let all = store.call(|c| db::load_all(c)).await?;
        Ok(all.iter().map(data::summary).collect())
    }

    /// Cours entier ; relance ce qui manque (et le note comme le plus
    /// récemment lu : ses paliers passent avant ceux des autres cours).
    pub async fn get(&self, ctx: &Ctx, course: &str) -> AppResult<Course> {
        self.ready(&ctx.store).await?;
        let mut snapshot = self.load(ctx, course).await?;
        self.shared.queue.lock().last_read = Some(course.to_owned());
        let stale = self.stale(&snapshot);
        if !stale.is_empty() {
            let id = course.to_owned();
            snapshot = ctx
                .store
                .call(move |c| {
                    db::atomic(c, |c| {
                        reset_stale(c, &id, &stale)?;
                        db::load(c, &id)?.ok_or_else(not_found)
                    })
                })
                .await?;
            ctx.changed(course);
        }
        self.resume(ctx, &snapshot);
        let mut read = data::assemble(snapshot);
        // Partie d'un autre cours en cours d'écriture : celles de ce cours
        // attendent (la page le dit plutôt que « en cours d'écriture »).
        read.queue_busy = self
            .shared
            .queue
            .lock()
            .running
            .as_ref()
            .is_some_and(|(other, _, cancel)| other != course && !cancel.is_cancelled());
        Ok(read)
    }

    /// Parties « en cours » sans travail dans la file ni en cours (travail
    /// fini sur une erreur de la base, arrêté puis non relancé…) : sans
    /// cela, elles le resteraient jusqu'au prochain démarrage.
    fn stale(&self, snapshot: &Snapshot) -> Vec<Part> {
        let course = snapshot.row.id.as_str();
        let queue = self.shared.queue.lock();
        let mut parts = Vec::new();
        for (index, level) in snapshot.levels.iter().enumerate() {
            if level.state == GenState::Generating && !queue.has(course, &Part::Level(index)) {
                parts.push(Part::Level(index));
            }
            if level.remedy == Some(GenState::Generating)
                && !queue.has(course, &Part::Remedy(index))
            {
                parts.push(Part::Remedy(index));
            }
        }
        if snapshot
            .row
            .final_quiz
            .as_ref()
            .is_some_and(|final_quiz| final_quiz.state == GenState::Generating)
            && !queue.has(course, &Part::Final)
        {
            parts.push(Part::Final);
        }
        parts
    }

    // -------------------------------------------------------------- création

    pub async fn create(&self, ctx: &Ctx, staging: &Staging, new: NewCourse) -> AppResult<String> {
        self.ready(&ctx.store).await?;
        let topic = new.topic.trim().to_owned();
        if topic.chars().count() > MAX_TOPIC_CHARS {
            return Err(AppError::Invalid(t!("study.topicTooLong")));
        }
        if ![10, 20, 30].contains(&new.minutes) {
            return Err(AppError::Invalid(t!("study.badDuration")));
        }
        if new.document_ids.len() > MAX_DOCUMENTS {
            return Err(AppError::Invalid(t!("study.tooManyDocuments")));
        }
        if topic.is_empty() && new.document_ids.is_empty() {
            return Err(AppError::Invalid(t!("study.emptyTopic")));
        }
        // Réglages seulement (modèle des cours) : rien à attendre d'Ollama.
        let settings = crate::ollama::target::display_settings(&ctx.store).await?;
        if super::model_of(&settings).is_none() {
            return Err(AppError::Invalid(t!("study.noModel")));
        }
        // Retirés de la zone ; remis en place si la création échoue.
        let taken = staging
            .take(&new.document_ids)
            .map_err(|_| AppError::Invalid(t!("study.unknownDocument")))?;
        let mut files = Vec::new();
        for file in taken.files() {
            match &file.text {
                Some(text) => files.push((file.file.filename.clone(), text.clone())),
                None => {
                    return Err(AppError::Invalid(t!(
                        "study.imageDocument",
                        name = file.file.filename
                    )));
                }
            }
        }
        let (rows, cut) = sources::documents(&files);
        if cut {
            // i18n-ignore : journal.
            eprintln!(
                "chatonlm: study: documents cut to {} characters",
                sources::DOCUMENTS_CHARS
            );
        }
        let now = now_ms();
        let row = CourseRow {
            id: uuid::Uuid::now_v7().to_string(),
            topic,
            level: new.level,
            minutes: new.minutes,
            language: crate::i18n::current(),
            model: String::new(),
            web: new.web,
            status: CourseStatus::Planning,
            error: None,
            clarify: None,
            plan: None,
            final_quiz: None,
            position: String::new(),
            elapsed_ms: 0,
            created_at: now,
            updated_at: now,
            finished_at: None,
            paused: false,
        };
        let id = row.id.clone();
        ctx.store.call(move |c| db::insert(c, &row, &rows)).await?;
        taken.keep();
        ctx.changed(&id);
        self.enqueue(ctx, &id, Part::Plan);
        Ok(id)
    }

    /// Réponse à la question d'un sujet ambigu : `choice` ajouté au sujet,
    /// ou `None` (plan sans question possible) ; le plan est refait.
    pub async fn clarify(&self, ctx: &Ctx, course: &str, choice: Option<String>) -> AppResult<()> {
        let meaning = choice
            .map(|choice| choice.trim().to_owned())
            .filter(|choice| !choice.is_empty());
        if meaning
            .as_ref()
            .is_some_and(|meaning| meaning.chars().count() > MAX_TOPIC_CHARS)
        {
            return Err(AppError::Invalid(t!("study.topicTooLong")));
        }
        self.change(ctx, course, move |c, snapshot| {
            if snapshot.row.status != CourseStatus::Unclear {
                return Err(unavailable());
            }
            // Sens précisé : la recherche web est refaite avec lui.
            if meaning.is_some() {
                db::delete_sources(c, &snapshot.row.id, SourceKind::Web)?;
            }
            db::update_row(c, &snapshot.row.id, |row| {
                let clarify = row.clarify.get_or_insert_with(Default::default);
                clarify.answered = true;
                clarify.meaning = meaning;
                row.status = CourseStatus::Planning;
                row.error = None;
                row.paused = false;
                Ok(())
            })?;
            Ok(())
        })
        .await
    }

    /// Nouveau plan (cours pas encore commencé) : paliers jetés.
    pub async fn replan(&self, ctx: &Ctx, course: &str) -> AppResult<()> {
        let snapshot = self.load(ctx, course).await?;
        if !matches!(
            snapshot.row.status,
            CourseStatus::Planned | CourseStatus::Unclear | CourseStatus::Failed
        ) {
            return Err(unavailable());
        }
        self.cancel(course, None);
        self.cancel_streams_of(course, None);
        self.change(ctx, course, |c, snapshot| {
            if !matches!(
                snapshot.row.status,
                CourseStatus::Planned | CourseStatus::Unclear | CourseStatus::Failed
            ) {
                return Err(unavailable());
            }
            let id = snapshot.row.id;
            db::update_row(c, &id, |row| {
                row.status = CourseStatus::Planning;
                row.plan = None;
                row.final_quiz = None;
                row.error = None;
                row.paused = false;
                Ok(())
            })?;
            db::put_levels(c, &id, &[])?;
            db::delete_answers(c, &id, "")?;
            Ok(())
        })
        .await
    }

    /// Commence le cours : paliers suivants et bilan mis en file.
    pub async fn start(&self, ctx: &Ctx, course: &str) -> AppResult<()> {
        self.change(ctx, course, |c, snapshot| {
            if snapshot.row.status != CourseStatus::Planned {
                return Err(unavailable());
            }
            db::update_row(c, &snapshot.row.id, |row| {
                row.status = CourseStatus::Active;
                Ok(())
            })?;
            Ok(())
        })
        .await
    }

    /// Met en pause (`true`) ou reprend (`false`) la génération d'un cours.
    /// Pause : enregistrée d'abord (plus rien de ce cours n'entre en file,
    /// un travail qui démarre s'arrête là), puis sa partie en cours est
    /// arrêtée et ses parties quittent la file ; enfin ce qui était « en
    /// cours » redevient « en attente » (le travail arrêté n'écrit plus
    /// rien). Reprise : ce qui manque revient en file.
    pub async fn set_paused(&self, ctx: &Ctx, course: &str, paused: bool) -> AppResult<()> {
        self.ready(&ctx.store).await?;
        if !paused {
            return self
                .change(ctx, course, |c, snapshot| unpause(c, &snapshot.row.id))
                .await;
        }
        let id = course.to_owned();
        ctx.store
            .call(move |c| {
                db::update_row(c, &id, |row| {
                    row.paused = true;
                    Ok(())
                })
            })
            .await?
            .ok_or_else(not_found)?;
        self.cancel(course, None);
        self.change(ctx, course, |c, snapshot| {
            let mut parts: Vec<Part> = (0..snapshot.levels.len())
                .flat_map(|index| [Part::Level(index), Part::Remedy(index)])
                .collect();
            parts.push(Part::Final);
            reset_stale(c, &snapshot.row.id, &parts)
        })
        .await
    }

    // -------------------------------------------------------------- réponses

    pub async fn answer(
        &self,
        ctx: &Ctx,
        course: &str,
        question: &str,
        given: AnswerValue,
    ) -> AppResult<Answered> {
        let (id, question_id) = (course.to_owned(), question.to_owned());
        let (answered, changed) = ctx
            .store
            .call(move |c| answer_in(c, &id, &question_id, given))
            .await?;
        if changed {
            ctx.changed(course);
        }
        Ok(answered)
    }

    pub async fn progress(
        &self,
        store: &Store,
        course: &str,
        position: String,
        elapsed_ms: i64,
    ) -> AppResult<()> {
        let id = course.to_owned();
        if store
            .call(move |c| db::progress(c, &id, &position, elapsed_ms))
            .await?
        {
            Ok(())
        } else {
            Err(not_found())
        }
    }

    /// « Explain differently » : permis si le dernier essai est manqué et
    /// qu'aucune demande n'est en cours.
    pub async fn remedy(&self, ctx: &Ctx, course: &str, level: u32) -> AppResult<()> {
        let index = level as usize;
        self.change(ctx, course, move |c, snapshot| {
            let level = snapshot.levels.get(index).ok_or_else(unavailable)?;
            let progress = data::progress(level, &snapshot.answers);
            let busy = matches!(level.remedy, Some(GenState::Pending | GenState::Generating));
            if level.state != GenState::Ready || !progress.last_failed || busy {
                return Err(unavailable());
            }
            db::update_level(c, &snapshot.row.id, index, |level| {
                level.remedy = Some(GenState::Pending);
                level.remedy_error = None;
                Ok(())
            })?;
            unpause(c, &snapshot.row.id)
        })
        .await
    }

    /// « Continue anyway » : deux essais manqués au moins, palier ni réussi
    /// ni déjà passé.
    pub async fn skip(&self, ctx: &Ctx, course: &str, level: u32) -> AppResult<()> {
        let index = level as usize;
        self.change(ctx, course, move |c, snapshot| {
            let level = snapshot.levels.get(index).ok_or_else(unavailable)?;
            let progress = data::progress(level, &snapshot.answers);
            if progress.failed_attempts < 2 || progress.passed || level.skipped {
                return Err(unavailable());
            }
            db::update_level(c, &snapshot.row.id, index, |level| {
                level.skipped = true;
                Ok(())
            })?;
            Ok(())
        })
        .await
    }

    /// « Report a mistake » : palier refait (génération en cours arrêtée),
    /// ses réponses effacées.
    pub async fn regenerate(&self, ctx: &Ctx, course: &str, level: u32) -> AppResult<()> {
        let index = level as usize;
        let snapshot = self.load(ctx, course).await?;
        if index >= snapshot.levels.len()
            || !matches!(
                snapshot.row.status,
                CourseStatus::Planned | CourseStatus::Active | CourseStatus::Finished
            )
        {
            return Err(unavailable());
        }
        self.cancel(course, Some(&Part::Level(index)));
        self.cancel(course, Some(&Part::Remedy(index)));
        // Explications et réponses de l'ancienne leçon : arrêtées, jetées.
        self.cancel_streams_of(course, Some(index));
        self.change(ctx, course, move |c, snapshot| {
            if index >= snapshot.levels.len() {
                return Err(unavailable());
            }
            db::update_level(c, &snapshot.row.id, index, |level| {
                *level = LevelData::default();
                Ok(())
            })?;
            db::delete_answers(c, &snapshot.row.id, &format!("L{index}-"))?;
            unpause(c, &snapshot.row.id)
        })
        .await
    }

    /// Relance les parties en échec ; `without_web` : plan sans la recherche
    /// web (après `signedOut`).
    pub async fn retry(&self, ctx: &Ctx, course: &str, without_web: bool) -> AppResult<()> {
        self.change(ctx, course, move |c, snapshot| {
            let id = snapshot.row.id.clone();
            unpause(c, &id)?;
            if snapshot.row.status == CourseStatus::Failed {
                db::update_row(c, &id, |row| {
                    row.status = CourseStatus::Planning;
                    row.error = None;
                    if without_web {
                        row.web = false;
                    }
                    Ok(())
                })?;
                if without_web {
                    db::delete_sources(c, &id, SourceKind::Web)?;
                }
                return Ok(());
            }
            for (index, level) in snapshot.levels.iter().enumerate() {
                if level.state == GenState::Failed || level.remedy == Some(GenState::Failed) {
                    db::update_level(c, &id, index, |level| {
                        if level.state == GenState::Failed {
                            level.state = GenState::Pending;
                            level.error = None;
                        }
                        if level.remedy == Some(GenState::Failed) {
                            level.remedy = Some(GenState::Pending);
                            level.remedy_error = None;
                        }
                        Ok(())
                    })?;
                }
            }
            if snapshot
                .row
                .final_quiz
                .as_ref()
                .is_some_and(|final_quiz| final_quiz.state == GenState::Failed)
            {
                db::update_row(c, &id, |row| {
                    if let Some(final_quiz) = &mut row.final_quiz {
                        final_quiz.state = GenState::Pending;
                        final_quiz.error = None;
                    }
                    Ok(())
                })?;
            }
            Ok(())
        })
        .await
    }

    // -------------------------------------------------------------- flux

    /// « I don't understand » sur une section : explication plus simple en
    /// flux, enregistrée à la fin.
    #[allow(clippy::too_many_arguments)]
    pub async fn explain(
        &self,
        ctx: &Ctx,
        course: &str,
        level: u32,
        section: u32,
        stream_id: String,
        channel: &Channel<StudyStreamEvent>,
    ) -> AppResult<()> {
        let snapshot = self.load(ctx, course).await?;
        let index = level as usize;
        let lesson = snapshot
            .levels
            .get(index)
            .and_then(|level| level.lesson.clone())
            .ok_or_else(unavailable)?;
        let section = section as usize;
        if section >= lesson.sections.len() {
            return Err(unavailable());
        }
        let lang = snapshot.row.language;
        let messages = prompts::messages(
            &prompts::explain_prompt(&lesson, section, lang),
            prompts::EXPLAIN_USER,
        );
        self.stream(
            ctx,
            course,
            stream_id,
            channel,
            messages,
            move |text| {
                Box::new(move |level: &mut LevelData| {
                    level.explains.push(Explain {
                        section: section as u32,
                        text,
                    })
                })
            },
            index,
            lesson,
        )
        .await
    }

    /// « Ask a question » sur un palier : réponse en flux (trois dernières
    /// questions du palier en contexte), enregistrée à la fin.
    #[allow(clippy::too_many_arguments)]
    pub async fn ask(
        &self,
        ctx: &Ctx,
        course: &str,
        level: u32,
        question: String,
        stream_id: String,
        channel: &Channel<StudyStreamEvent>,
    ) -> AppResult<()> {
        let question = question.trim().to_owned();
        if question.is_empty() || question.chars().count() > MAX_QUESTION_CHARS {
            return Err(AppError::Invalid(t!("study.questionLength")));
        }
        let snapshot = self.load(ctx, course).await?;
        let index = level as usize;
        let level = snapshot.levels.get(index).ok_or_else(unavailable)?;
        let lesson = level.lesson.clone().ok_or_else(unavailable)?;
        let title = snapshot
            .row
            .plan
            .as_ref()
            .map(|plan| plan.title.clone())
            .unwrap_or_else(|| snapshot.row.topic.clone());
        let mut messages = vec![serde_json::json!({
            "role": "system",
            "content": prompts::ask_prompt(&title, &lesson, snapshot.row.language),
        })];
        let earlier = level.asks.len().saturating_sub(3);
        for ask in &level.asks[earlier..] {
            messages.push(serde_json::json!({ "role": "user", "content": ask.question }));
            messages.push(serde_json::json!({ "role": "assistant", "content": ask.answer }));
        }
        messages.push(serde_json::json!({ "role": "user", "content": question }));
        self.stream(
            ctx,
            course,
            stream_id,
            channel,
            serde_json::Value::Array(messages),
            move |answer| {
                Box::new(move |level: &mut LevelData| level.asks.push(Ask { question, answer }))
            },
            index,
            lesson,
        )
        .await
    }

    /// Flux commun : inscrit sous `stream_id` (arrêt par
    /// [`StudyEngine::cancel_stream`], ou un envoi qui échoue), `Delta` puis
    /// `Done` ; réponse complète enregistrée par `save` dans le palier
    /// `index` ; arrêtée : rien d'enregistré.
    #[allow(clippy::too_many_arguments)]
    async fn stream(
        &self,
        ctx: &Ctx,
        course: &str,
        stream_id: String,
        channel: &Channel<StudyStreamEvent>,
        messages: serde_json::Value,
        save: impl FnOnce(String) -> Box<dyn FnOnce(&mut LevelData) + Send> + Send,
        index: usize,
        lesson: Lesson,
    ) -> AppResult<()> {
        let cancel = Cancel::default();
        let _registered = Registered::new(&self.shared, stream_id, course, index, cancel.clone());
        let send = |event: StudyStreamEvent| {
            if channel.send(event).is_err() {
                cancel.cancel();
            }
        };
        let ollama = match Gen::connect(&ctx.store, &cancel, self.shared.timeouts).await {
            Ok(ollama) => ollama,
            Err(stop) => {
                stream_end(&send, stop, String::new());
                return Ok(());
            }
        };
        let partial = Mutex::new(String::new());
        let on_delta = |text: String| {
            partial.lock().push_str(&text);
            send(StudyStreamEvent::Delta { text });
        };
        match ollama.stream(messages, &on_delta).await {
            Ok(text) => {
                let change = save(text.clone());
                let id = course.to_owned();
                // Enregistrée seulement si la leçon est toujours celle
                // expliquée (palier refait pendant le flux : jetée).
                let saved = ctx
                    .store
                    .call(move |c| {
                        let current = db::level(c, &id, index)?;
                        if current.and_then(|level| level.lesson).as_ref() != Some(&lesson) {
                            return Ok(false);
                        }
                        db::update_level(c, &id, index, |level| {
                            change(level);
                            Ok(())
                        })
                        .map(|saved| saved.is_some())
                    })
                    .await?;
                if saved {
                    ctx.changed(course);
                }
                send(StudyStreamEvent::Done {
                    text,
                    stopped: !saved,
                });
            }
            Err(stop) => {
                let partial = partial.lock().clone();
                stream_end(&send, stop, partial);
            }
        }
        Ok(())
    }

    /// Arrête un flux ; sans effet s'il est fini.
    pub fn cancel_stream(&self, stream_id: &str) {
        if let Some(entry) = self.shared.streams.lock().get(stream_id) {
            entry.cancel.cancel();
        }
    }

    /// Arrête les flux d'un cours (`level` : d'un seul palier).
    fn cancel_streams_of(&self, course: &str, level: Option<usize>) {
        for entry in self.shared.streams.lock().values() {
            if entry.course == course && level.is_none_or(|level| level == entry.level) {
                entry.cancel.cancel();
            }
        }
    }

    /// Arrête tous les flux (page rechargée : plus personne ne les lit).
    pub fn cancel_streams(&self) {
        for entry in self.shared.streams.lock().values() {
            entry.cancel.cancel();
        }
    }

    /// Ollama va s'arrêter (fermeture de l'app, changement de serveur ou du
    /// dossier des modèles de l'intégré) : file vidée, partie en cours et
    /// flux arrêtés, sans rien enregistrer. Rien n'est noté en échec : une
    /// partie en attente le reste, celle en cours reste « en cours » et
    /// reprend à la prochaine lecture du cours (`study_get`) ou au lancement
    /// suivant.
    pub fn cancel_all(&self) {
        {
            let mut queue = self.shared.queue.lock();
            queue.jobs.clear();
            if let Some((_, _, cancel)) = &queue.running {
                cancel.cancel();
            }
        }
        self.cancel_streams();
    }

    // -------------------------------------------------------------- fiche, suppression

    pub async fn notes(&self, store: &Store, course: &str) -> AppResult<String> {
        Ok(self.notes_and_name(store, course).await?.0)
    }

    /// Fiche et nom de fichier proposé (titre nettoyé, `.md`).
    pub async fn notes_and_name(&self, store: &Store, course: &str) -> AppResult<(String, String)> {
        let id = course.to_owned();
        let snapshot = store
            .call(move |c| db::load(c, &id))
            .await?
            .ok_or_else(not_found)?;
        Ok((
            super::notes::markdown(&snapshot),
            super::notes::file_name(&snapshot),
        ))
    }

    /// Supprime un cours : génération et flux arrêtés, lignes supprimées.
    pub async fn delete(&self, ctx: &Ctx, course: &str) -> AppResult<()> {
        self.cancel(course, None);
        self.cancel_streams_of(course, None);
        let id = course.to_owned();
        if ctx.store.call(move |c| db::delete(c, &id)).await? {
            (ctx.notify)(StudyUpdated {
                course_id: course.to_owned(),
                deleted: true,
            });
        }
        Ok(())
    }

    /// Met une partie en file sans rien vérifier, comme une demande partie
    /// juste avant une pause et arrivée juste après.
    #[cfg(test)]
    pub(crate) fn enqueue_unchecked(&self, ctx: &Ctx, course: &str, part: Part) {
        self.enqueue(ctx, course, part);
    }

    #[cfg(test)]
    pub(crate) fn queued(&self) -> Vec<(String, Part)> {
        let queue = self.shared.queue.lock();
        let mut jobs: Vec<Queued> = queue.jobs.clone();
        jobs.sort_by_key(|job| queue.rank(job));
        jobs.into_iter().map(|job| (job.course, job.part)).collect()
    }

    #[cfg(test)]
    pub(crate) fn running(&self) -> Option<(String, Part)> {
        self.shared
            .queue
            .lock()
            .running
            .as_ref()
            .map(|(c, p, _)| (c.clone(), p.clone()))
    }

    #[cfg(test)]
    pub(crate) fn streams(&self) -> usize {
        self.shared.streams.lock().len()
    }

    #[cfg(test)]
    pub(crate) fn idle(&self) -> bool {
        let queue = self.shared.queue.lock();
        !queue.worker && queue.jobs.is_empty()
    }
}

/// Fin d'un flux arrêté ou en échec.
fn stream_end(send: &impl Fn(StudyStreamEvent), stop: Stop, partial: String) {
    match stop {
        Stop::Cancelled => send(StudyStreamEvent::Done {
            text: partial,
            stopped: true,
        }),
        Stop::Failed(error) => send(StudyStreamEvent::Error {
            code: error.code,
            message: error.message,
        }),
    }
}

/// Flux inscrit le temps de sa vie (retiré à la fin, sauf si un autre a pris
/// son identifiant).
struct Registered<'a> {
    shared: &'a Shared,
    stream_id: String,
    token: u64,
}

impl<'a> Registered<'a> {
    fn new(
        shared: &'a Shared,
        stream_id: String,
        course: &str,
        level: usize,
        cancel: Cancel,
    ) -> Self {
        let token = shared
            .tokens
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        shared.streams.lock().insert(
            stream_id.clone(),
            StreamEntry {
                course: course.to_owned(),
                level,
                cancel,
                token,
            },
        );
        Self {
            shared,
            stream_id,
            token,
        }
    }
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        let mut streams = self.shared.streams.lock();
        if streams
            .get(&self.stream_id)
            .is_some_and(|entry| entry.token == self.token)
        {
            streams.remove(&self.stream_id);
        }
    }
}

/// Tâche de la file : une partie à la fois, jusqu'à ce que la file soit
/// vide.
async fn work(shared: Arc<Shared>, ctx: Ctx) {
    let _guard = WorkerGuard(shared.clone());
    loop {
        let (job, cancel) = {
            let mut queue = shared.queue.lock();
            let Some(job) = queue.pick() else {
                queue.worker = false;
                return;
            };
            let cancel = Cancel::default();
            queue.running = Some((job.course.clone(), job.part.clone(), cancel.clone()));
            (job, cancel)
        };
        let changed = {
            let (ctx, course) = (ctx.clone(), job.course.clone());
            move || ctx.changed(&course)
        };
        let runner = Job {
            store: &ctx.store,
            course: &job.course,
            cancel: &cancel,
            timeouts: shared.timeouts,
            changed: &changed,
        };
        let next = match &job.part {
            Part::Plan => runner.plan().await,
            Part::Level(index) => runner.level(*index).await,
            Part::Remedy(index) => runner.remedy(*index).await,
            Part::Final => runner.final_quiz().await,
        };
        shared.queue.lock().running = None;
        match next {
            Ok(next) if !next.parts.is_empty() && !cancel.is_cancelled() => {
                // Ce qui manque, relu dans la base (palier 0 d'un plan prêt,
                // bilan une fois les paliers prêts…).
                let id = job.course.clone();
                if let Ok(Some(snapshot)) = ctx.store.call(move |c| db::load(c, &id)).await {
                    let engine = StudyEngine {
                        shared: shared.clone(),
                    };
                    engine.resume(&ctx, &snapshot);
                }
            }
            Ok(_) => {}
            // i18n-ignore : journal.
            Err(error) => eprintln!(
                "chatonlm: study: generation of {:?} failed: {error}",
                job.part
            ),
        }
    }
}

/// Reprend un cours en pause (sans effet sinon) : dans le changement d'une
/// demande qui exige une génération, ce qui manque revient alors en file.
fn unpause(connection: &rusqlite::Connection, course: &str) -> AppResult<()> {
    db::update_row(connection, course, |row| {
        row.paused = false;
        Ok(())
    })?;
    Ok(())
}

/// Remet en attente les parties `parts`, si elles sont toujours « en
/// cours ».
fn reset_stale(connection: &rusqlite::Connection, course: &str, parts: &[Part]) -> AppResult<()> {
    for part in parts {
        match part {
            Part::Level(index) => {
                db::update_level(connection, course, *index, |level| {
                    if level.state == GenState::Generating {
                        level.state = GenState::Pending;
                    }
                    Ok(())
                })?;
            }
            Part::Remedy(index) => {
                db::update_level(connection, course, *index, |level| {
                    if level.remedy == Some(GenState::Generating) {
                        level.remedy = Some(GenState::Pending);
                    }
                    Ok(())
                })?;
            }
            Part::Final => {
                db::update_row(connection, course, |row| {
                    if let Some(final_quiz) = &mut row.final_quiz
                        && final_quiz.state == GenState::Generating
                    {
                        final_quiz.state = GenState::Pending;
                    }
                    Ok(())
                })?;
            }
            Part::Plan => {}
        }
    }
    Ok(())
}

/// Corrige et enregistre une réponse, en une transaction (réponse et fin du
/// cours ensemble) ; `(réponse, changé)` : une question déjà répondue
/// renvoie sa première réponse (la fin du cours est revérifiée).
pub fn answer_in(
    connection: &rusqlite::Connection,
    course: &str,
    question_id: &str,
    given: AnswerValue,
) -> AppResult<(Answered, bool)> {
    db::atomic(connection, |connection| {
        answer_atomic(connection, course, question_id, given)
    })
}

/// Fin du cours : tous les paliers faits et le bilan répondu.
fn finish_if_done(connection: &rusqlite::Connection, snapshot: &Snapshot) -> AppResult<bool> {
    if snapshot.row.status != CourseStatus::Active || !data::is_finished(snapshot) {
        return Ok(false);
    }
    db::update_row(connection, &snapshot.row.id, |row| {
        row.status = CourseStatus::Finished;
        row.finished_at = Some(now_ms());
        Ok(())
    })?;
    Ok(true)
}

fn answer_atomic(
    connection: &rusqlite::Connection,
    course: &str,
    question_id: &str,
    given: AnswerValue,
) -> AppResult<(Answered, bool)> {
    let snapshot = db::load(connection, course)?.ok_or_else(not_found)?;
    if let Some(answered) = snapshot.answers.get(question_id) {
        let finished = finish_if_done(connection, &snapshot)?;
        return Ok((answered.clone(), finished));
    }
    if !matches!(
        snapshot.row.status,
        CourseStatus::Active | CourseStatus::Finished
    ) {
        return Err(unavailable());
    }
    let statuses = data::statuses(&snapshot.levels, &snapshot.answers);
    let mut found: Option<&StoredQuestion> = None;
    for (index, level) in snapshot.levels.iter().enumerate() {
        let question = level
            .check
            .iter()
            .chain(level.attempts.iter().flat_map(|attempt| &attempt.questions))
            .find(|question| question.id == question_id);
        if let Some(question) = question {
            if statuses[index] == LevelStatus::Locked {
                return Err(unavailable());
            }
            found = Some(question);
        }
    }
    if found.is_none()
        && let Some(final_quiz) = &snapshot.row.final_quiz
        && let Some(question) = final_quiz.questions.iter().find(|q| q.id == question_id)
    {
        if !data::levels_done(&snapshot.levels, &snapshot.answers) {
            return Err(unavailable());
        }
        found = Some(question);
    }
    let question = found.ok_or_else(unavailable)?;
    // Cours de langue : orthographe exacte (voir `grade`).
    let strict = snapshot
        .row
        .plan
        .as_ref()
        .is_some_and(|plan| plan.kind == super::StudyKind::Language);
    let graded = grade::grade(question, &given, strict).ok_or_else(unavailable)?;
    let answered = Answered {
        given,
        correct: graded.correct,
        close: graded.close,
        expected: question.expected.clone(),
        detail: graded.detail,
        explanation: question.explanation.clone(),
        at: now_ms(),
    };
    db::put_answer(connection, course, question_id, &answered)?;
    let after = db::load(connection, course)?.ok_or_else(not_found)?;
    if !finish_if_done(connection, &after)? {
        // Dernière activité.
        db::update_row(connection, course, |_| Ok(()))?;
    }
    Ok((answered, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(engine: &StudyEngine, course: &str, part: Part) {
        let mut queue = engine.shared.queue.lock();
        queue.seq += 1;
        let seq = queue.seq;
        queue.jobs.push(Queued {
            course: course.into(),
            part,
            seq,
        });
    }

    fn part(course: &str, part: Part) -> (String, Part) {
        (course.to_owned(), part)
    }

    #[test]
    fn parts_are_generated_by_priority() {
        let engine = StudyEngine::default();
        push(&engine, "a", Part::Final);
        push(&engine, "b", Part::Level(1));
        push(&engine, "a", Part::Level(2));
        push(&engine, "a", Part::Level(1));
        push(&engine, "c", Part::Plan);
        push(&engine, "b", Part::Remedy(0));
        push(&engine, "b", Part::Level(0));
        push(&engine, "d", Part::Plan);
        engine.shared.queue.lock().last_read = Some("a".into());
        // Nouvel essai, plans (ordre d'arrivée), paliers du cours lu (par
        // rang) puis son bilan, autres paliers, autres bilans.
        assert_eq!(
            engine.queued(),
            [
                part("b", Part::Remedy(0)),
                part("c", Part::Plan),
                part("d", Part::Plan),
                part("a", Part::Level(1)),
                part("a", Part::Level(2)),
                part("a", Part::Final),
                part("b", Part::Level(0)),
                part("b", Part::Level(1)),
            ]
        );
        // Un autre cours lu : ses paliers passent devant.
        engine.shared.queue.lock().last_read = Some("b".into());
        let picked = engine.shared.queue.lock().pick().unwrap();
        assert_eq!(
            (picked.course.as_str(), picked.part),
            ("b", Part::Remedy(0))
        );
        let order: Vec<(String, Part)> = std::iter::from_fn(|| engine.shared.queue.lock().pick())
            .map(|job| (job.course, job.part))
            .collect();
        assert_eq!(
            order[2..5],
            [
                part("b", Part::Level(0)),
                part("b", Part::Level(1)),
                part("a", Part::Level(1)),
            ]
        );
    }

    #[test]
    fn cancelling_a_course_removes_its_parts_and_stops_the_running_one() {
        let engine = StudyEngine::default();
        push(&engine, "a", Part::Level(1));
        push(&engine, "b", Part::Level(1));
        push(&engine, "a", Part::Final);
        let running = Cancel::default();
        engine.shared.queue.lock().running = Some(("a".into(), Part::Level(0), running.clone()));
        // Une seule partie arrêtée.
        engine.cancel("a", Some(&Part::Final));
        assert_eq!(
            engine.queued(),
            [part("a", Part::Level(1)), part("b", Part::Level(1))]
        );
        assert!(!running.is_cancelled());
        assert!(engine.shared.queue.lock().has("a", &Part::Level(0)));
        engine.cancel("a", None);
        assert_eq!(engine.queued(), [part("b", Part::Level(1))]);
        assert!(running.is_cancelled());
        // La partie arrêtée peut revenir en file avant la fin de sa requête.
        assert!(!engine.shared.queue.lock().has("a", &Part::Level(0)));
    }

    #[test]
    fn a_stream_id_taken_again_is_kept() {
        let engine = StudyEngine::default();
        let (old, new) = (Cancel::default(), Cancel::default());
        let first = Registered::new(&engine.shared, "s".into(), "a", 0, old);
        let second = Registered::new(&engine.shared, "s".into(), "a", 0, new.clone());
        drop(first);
        engine.cancel_stream("s");
        assert!(new.is_cancelled());
        drop(second);
        assert_eq!(engine.streams(), 0);
        let other = Cancel::default();
        let _third = Registered::new(&engine.shared, "t".into(), "a", 0, other.clone());
        engine.cancel_streams();
        assert!(other.is_cancelled());
    }
}
