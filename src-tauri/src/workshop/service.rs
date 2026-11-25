//! File des projets de l'Atelier ([`Workshop`], état Tauri) et opérations
//! des commandes `workshop_*`.
//!
//! - UN projet calcule à la fois (y compris pendant qu'il cède la place ou
//!   attend une réponse : il garde son modèle ouvert, au plus
//!   [`PARK_AFTER`] ; ensuite il rend sa place, reste en attente, et la
//!   réponse le remet en file) ; les autres attendent dans la file, dans
//!   l'ordre d'arrivée (`queue_seq`), position affichée.
//! - Pause, reprise, annulation : vérification de l'état et écriture en une
//!   seule requête (`store::workshop::set_state_from`, `enqueue`), le projet
//!   actif étant regardé avant ET après (un lancement entre les deux).
//! - Un lancement : état `running`, modèle choisi ([`Runner::choose`],
//!   avertissements en événement), puis le pipeline mené par
//!   [`Runner::run`] contre l'hôte réel ([`ProjectHost`]).
//! - Fin : livré (`done`), échoué (`failed`, code), ou arrêté : pause
//!   (`paused`, reprise d'un clic, rien de refait), annulation
//!   (`cancelled`), fermeture de l'app (`paused`), suppression ou
//!   effacement des données (rien n'est écrit).
//! - Lancement de l'app ([`Workshop::recover`]) : rien ne calcule par
//!   surprise (voir `store::workshop::recover`).
//! - Changements diffusés à tous les abonnés ([`Workshop::subscribe`] :
//!   ligne de commande et API plus tard) et à la page (événement
//!   `workshop-event`, débit borné, [`Throttle`]).
//!
//! Code commun à Linux et Windows.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use parking_lot::Mutex;
use tokio::sync::{broadcast, oneshot, watch};
use tokio_util::sync::CancellationToken;

use super::model::{ChoiceError, ModelChoice};
use super::pipeline::types::{
    Answer, Attachment, Event, EventKind, ProjectInfo, ProjectState, Question, Report, Spec,
    StateKey, Strategy,
};
use super::project::ProjectHost;
use super::{
    AnswerRequest, MAX_ATTACHMENTS, MAX_EVENTS_PER_READ, MAX_TASK_CHARS, ModelCheck,
    NewProjectRequest, PendingQuestion, ProjectDetail, ProjectSummary, WorkshopNotice,
    WorkshopPart, WorkshopPartSummary, WorkshopSettings, WorkshopSettingsPatch, dec, enc, files,
    parts,
};
use crate::attachments::Staging;
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::resources::Interactive;
use crate::store::workshop::{self as db, NewProject, ProjectRow};
use crate::store::{Store, now_ms};

/// Délai de grâce de la priorité de l'utilisateur (choix B).
pub(crate) const GRACE: Duration = Duration::from_secs(3);

/// Intervalle minimal entre deux avis d'un même lot vers la page.
pub(crate) const NOTICE_INTERVAL: Duration = Duration::from_millis(250);

/// Attente d'une réponse au-delà de laquelle le projet rend sa place
/// (modèle relâché, file libérée) ; il reste en attente.
pub(crate) const PARK_AFTER: Duration = Duration::from_secs(120);

/// Attente de la fin du projet actif à la fermeture de l'app.
pub(crate) const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

/// Changements gardés pour un abonné en retard.
const UPDATES_KEPT: usize = 256;

/// Longueur maximale d'un titre tiré de la tâche (caractères).
const MAX_TITLE_CHARS: usize = 80;

/// Ce que fait un projet lancé : choisir son modèle, puis mener le pipeline
/// (l'app : `agent::Worker` et le pipeline ; les essais : un faux).
pub(crate) trait Runner: Send + Sync {
    /// Modèle retenu pour `requested` (vide : choisi par l'Atelier).
    fn choose(&self, requested: String) -> BoxFuture<'static, Result<ModelChoice, ChoiceError>>;
    /// Mène le projet avec ce modèle. Arrêt : le jeton de l'hôte (une issue
    /// autre que le livrable est alors un arrêt).
    fn run(&self, host: Arc<ProjectHost>, model: ModelChoice) -> BoxFuture<'static, Outcome>;
}

/// Issue d'un passage du pipeline (`pipeline::RunEnd`, vu du socle).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Livrable écrit.
    Delivered,
    /// Arrêté (jeton du socle, ou question abandonnée) : état sauvé.
    Cancelled,
    /// L'utilisateur a refusé le plan sans note : le projet s'arrête là.
    Rejected,
    /// Échec (code stable).
    Failed(String),
}

/// Notification du bureau à montrer (textes faits par l'app, `t!`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Alert {
    /// Une question attend l'utilisateur.
    Waiting {
        title: String,
    },
    Done {
        title: String,
    },
    Failed {
        title: String,
    },
    /// Projets mis en pause au lancement de l'app.
    Paused {
        count: usize,
    },
}

/// Ce dont le service a besoin de l'app.
pub(crate) struct Deps {
    pub runner: Arc<dyn Runner>,
    pub alert: Arc<dyn Fn(Alert) + Send + Sync>,
    /// Avis vers la page (événement `workshop-event`).
    pub notice: Arc<dyn Fn(WorkshopNotice) + Send + Sync>,
    pub interactive: Interactive,
    pub grace: Duration,
    pub notice_interval: Duration,
    /// Dossier des projets par défaut (`Documents/ChatonLM`) ; `None` :
    /// inconnu (il faut en choisir un).
    pub default_root: Option<PathBuf>,
    /// Attente d'une réponse avant de rendre la place ([`PARK_AFTER`]).
    pub park_after: Duration,
}

/// Changement d'un projet, diffusé aux abonnés.
#[derive(Clone, Debug, PartialEq)]
pub enum WorkshopUpdate {
    Event { project: i64, event: Event },
    State { project: i64, state: ProjectState },
    Deleted { project: i64 },
}

impl WorkshopUpdate {
    fn notice(&self) -> WorkshopNotice {
        match self {
            WorkshopUpdate::Event { project, event } => WorkshopNotice {
                project_id: *project,
                state: None,
                last_seq: Some(event.seq),
                deleted: false,
            },
            WorkshopUpdate::State { project, state } => WorkshopNotice {
                project_id: *project,
                state: Some(*state),
                last_seq: None,
                deleted: false,
            },
            WorkshopUpdate::Deleted { project } => WorkshopNotice {
                project_id: *project,
                state: None,
                last_seq: None,
                deleted: true,
            },
        }
    }
}

/// Pourquoi le projet actif s'arrête.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    /// Pause demandée.
    Pause,
    /// Annulation demandée.
    Cancel,
    /// Fermeture de l'app : en pause, repris d'un clic au lancement.
    Close,
    /// Suppression ou effacement des données : rien à écrire.
    Forget,
    /// Question sans réponse depuis [`PARK_AFTER`] : place rendue, le projet
    /// reste en attente (la réponse le remet en file).
    Park,
}

/// Projet en cours de calcul.
struct Active {
    id: i64,
    cancel: CancellationToken,
    stop: Option<Stop>,
    done: watch::Receiver<bool>,
}

/// Réponse attendue par un projet.
struct Waiter {
    question: i64,
    send: oneshot::Sender<String>,
}

#[derive(Default)]
struct Inner {
    active: Option<Active>,
    waiters: HashMap<i64, Waiter>,
    /// App en fermeture : plus rien ne se lance.
    closed: bool,
    /// « Erase my data » en cours : rien ne se lance jusqu'à la fin de
    /// [`Workshop::forget`].
    forgetting: bool,
    /// Dernier dossier rendu par le sélecteur natif (seul accepté comme
    /// nouveau dossier des projets).
    granted_dir: Option<String>,
}

/// Fin d'un lancement.
#[derive(Debug, PartialEq, Eq)]
enum Ending {
    /// Le projet n'était plus en file (mis en pause, annulé, supprimé).
    Skipped,
    /// La base refuse de le lancer : la file s'arrête là (pas de boucle).
    Broken,
    Done,
    Failed(String),
    /// Plan refusé par l'utilisateur.
    Rejected,
    /// Arrêté par le jeton.
    Stopped,
}

/// Avis vers la page, débit borné : le premier part aussitôt, les suivants
/// sont regroupés (le plus récent de chaque projet) au plus tous les
/// `interval`.
struct Throttle {
    pending: Mutex<(BTreeMap<i64, WorkshopNotice>, bool)>,
    emit: Arc<dyn Fn(WorkshopNotice) + Send + Sync>,
    interval: Duration,
}

impl Throttle {
    fn push(self: &Arc<Self>, notice: WorkshopNotice) {
        let start = {
            let mut pending = self.pending.lock();
            pending
                .0
                .entry(notice.project_id)
                .and_modify(|kept| kept.merge(notice.clone()))
                .or_insert(notice);
            !std::mem::replace(&mut pending.1, true)
        };
        if start {
            spawn(self.clone().drain());
        }
    }

    async fn drain(self: Arc<Self>) {
        loop {
            let batch = {
                let mut pending = self.pending.lock();
                if pending.0.is_empty() {
                    pending.1 = false;
                    return;
                }
                std::mem::take(&mut pending.0)
            };
            for notice in batch.into_values() {
                (self.emit)(notice);
            }
            tokio::time::sleep(self.interval).await;
        }
    }
}

/// Lance une tâche sur le moteur d'exécution courant (commande, essai),
/// sinon sur celui de Tauri.
pub(crate) fn spawn(future: impl Future<Output = ()> + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(future);
        }
        Err(_) => {
            tauri::async_runtime::spawn(future);
        }
    }
}

/// Le service, partagé avec les hôtes des projets.
pub(crate) struct Shared {
    pub(super) store: Store,
    deps: Deps,
    inner: Mutex<Inner>,
    updates: broadcast::Sender<WorkshopUpdate>,
    throttle: Arc<Throttle>,
}

/// L'Atelier (état Tauri, clonable).
#[derive(Clone)]
pub struct Workshop {
    shared: Arc<Shared>,
}

fn not_found() -> AppError {
    AppError::NotFound(t!("workshop.notFound"))
}

fn not_allowed() -> AppError {
    AppError::Invalid(t!("workshop.notAllowed"))
}

fn state_of(row: &ProjectRow) -> AppResult<ProjectState> {
    dec(&row.state).ok_or_else(|| {
        AppError::Internal(format!("unknown workshop state in database: {}", row.state))
    })
}

/// Titre tiré de la tâche : sa première ligne non vide, au plus
/// [`MAX_TITLE_CHARS`] caractères (coupée à un mot si possible).
pub(crate) fn title_of(task: &str) -> String {
    let line = task
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= MAX_TITLE_CHARS {
        return line;
    }
    let clipped: String = line.chars().take(MAX_TITLE_CHARS).collect();
    match clipped.rfind(' ') {
        Some(space) if space > MAX_TITLE_CHARS / 2 => format!("{}…", &clipped[..space]),
        _ => format!("{clipped}…"),
    }
}

/// Résumé d'une ligne ; `position` : rang dans la file.
fn summary(row: &ProjectRow, position: Option<u32>) -> AppResult<ProjectSummary> {
    Ok(ProjectSummary {
        id: row.id,
        title: row.title.clone(),
        strategy: row.strategy.as_deref().and_then(dec),
        strategy_forced: row.strategy_forced,
        autonomy: dec(&row.autonomy).ok_or_else(|| {
            AppError::Internal(format!("unknown autonomy in database: {}", row.autonomy))
        })?,
        state: state_of(row)?,
        phase: row.phase.as_deref().and_then(dec),
        model: row.model.clone(),
        model_used: row.model_used.clone(),
        position,
        error: row.error.clone(),
        folder: row.folder.clone(),
        created_at: row.created_at,
        updated_at: row.updated_at,
        finished_at: row.finished_at,
    })
}

/// Rangs dans la file (1 : le prochain), par projet.
fn positions(rows: &[ProjectRow]) -> HashMap<i64, u32> {
    let mut queued: Vec<&ProjectRow> = rows.iter().filter(|row| row.state == "queued").collect();
    queued.sort_by_key(|row| (row.queue_seq, row.created_at, row.id));
    queued
        .iter()
        .enumerate()
        .map(|(index, row)| (row.id, index as u32 + 1))
        .collect()
}

/// La réponse va avec la question.
fn answers(question: &Question, answer: &Answer) -> bool {
    match (question, answer) {
        (Question::Strategy(proposed), Answer::Strategy(strategy)) => {
            proposed.contains(strategy) || strategy.available()
        }
        (Question::Framing(questions), Answer::Framing(replies)) => {
            replies.len() == questions.len()
        }
        (Question::ApprovePlan(_), Answer::Plan { .. }) => true,
        (Question::ApproveAction { .. }, Answer::Action(_)) => true,
        _ => false,
    }
}

/// Copie du texte de la pièce jointe n° `index` (à partir de 0) :
/// `.atelier/sources/01-notes.pdf.txt` (`.txt` ajouté sauf s'il y est) ;
/// nom inutilisable : `fallback` (« source » dans la langue de l'app).
pub(crate) fn source_path(index: usize, filename: &str, fallback: &str) -> String {
    let name = files::folder_name(filename, fallback);
    let suffix = if name.to_ascii_lowercase().ends_with(".txt") {
        ""
    } else {
        ".txt"
    };
    format!(
        "{}/sources/{:02}-{name}{suffix}",
        files::WORK_DIR,
        index + 1
    )
}

/// Dossier d'un projet à effacer : seulement un dossier (pas un lien) qui a
/// son `.atelier/` (créé par l'Atelier).
fn removable_project_dir(folder: &Path) -> bool {
    folder.is_absolute()
        && std::fs::symlink_metadata(folder).is_ok_and(|meta| meta.is_dir())
        && std::fs::symlink_metadata(folder.join(files::WORK_DIR)).is_ok_and(|meta| meta.is_dir())
}

impl Shared {
    /// Diffuse un changement (abonnés et page).
    pub(super) fn publish(&self, update: WorkshopUpdate) {
        self.throttle.push(update.notice());
        let _ = self.updates.send(update);
    }

    /// Change l'état d'un projet en base et le diffuse.
    pub(super) async fn set_state(&self, id: i64, state: ProjectState, error: Option<String>) {
        let text = enc(&state);
        let saved = self
            .store
            .call(move |c| db::set_state(c, id, &text, error.as_deref(), now_ms()))
            .await;
        match saved {
            Ok(true) => self.publish(WorkshopUpdate::State { project: id, state }),
            Ok(false) => {}
            Err(error) => {
                // i18n-ignore : journal.
                crate::app_log!("chatonlm: workshop: state of project {id} not saved: {error}");
            }
        }
    }

    pub(super) fn alert(&self, alert: Alert) {
        (self.deps.alert)(alert);
    }

    pub(super) fn interactive(&self) -> &Interactive {
        &self.deps.interactive
    }

    pub(super) fn grace(&self) -> Duration {
        self.deps.grace
    }

    pub(super) fn park_after(&self) -> Duration {
        self.deps.park_after
    }

    /// Le projet `id` attend une réponse sans réponse : il rend sa place. Son
    /// attente est oubliée et l'arrêt posé sous le même verrou ([`Workshop::answer`]
    /// le remet alors en file) ; jeton annulé : le pipeline s'arrête, état sauvé.
    pub(super) fn park(&self, id: i64) {
        let mut inner = self.inner.lock();
        inner.waiters.remove(&id);
        if let Some(active) = inner.active.as_mut().filter(|active| active.id == id) {
            if active.stop.is_none() {
                active.stop = Some(Stop::Park);
            }
            active.cancel.cancel();
        }
    }

    /// Le projet `id` ne calcule pas, ou rend sa place (une réponse doit le
    /// remettre en file).
    fn idle_or_parked(&self, id: i64) -> bool {
        match self.inner.lock().active.as_ref() {
            Some(active) if active.id == id => active.stop == Some(Stop::Park),
            _ => true,
        }
    }

    /// Remet le projet en file, en dernier, s'il est dans l'un des états
    /// `from` (vérifié à l'écriture : sans effet sinon).
    pub(super) async fn requeue(
        self: &Arc<Self>,
        id: i64,
        from: &'static [&'static str],
    ) -> AppResult<()> {
        let queued = self
            .store
            .call(move |c| {
                let seq = db::max_queue_seq(c)? + 1;
                db::enqueue(c, id, seq, now_ms(), from)
            })
            .await?;
        if queued {
            self.publish(WorkshopUpdate::State {
                project: id,
                state: ProjectState::Queued,
            });
            self.dispatch();
        }
        Ok(())
    }

    /// Attend la réponse à `question` du projet `id` (inscrite avant que la
    /// question ne soit montrée).
    pub(super) fn wait_answer(&self, id: i64, question: i64) -> oneshot::Receiver<String> {
        let (send, receive) = oneshot::channel();
        self.inner
            .lock()
            .waiters
            .insert(id, Waiter { question, send });
        receive
    }

    /// N'attend plus de réponse pour le projet `id`.
    pub(super) fn forget_waiter(&self, id: i64) {
        self.inner.lock().waiters.remove(&id);
    }

    /// Réserve la place de calcul et lance le premier projet de la file, s'il
    /// y a de la place.
    fn dispatch(self: &Arc<Self>) {
        let shared = self.clone();
        spawn(async move {
            let queued = shared.store.call(|c| db::in_state(c, "queued")).await;
            let Some(next) = queued.ok().and_then(|rows| rows.into_iter().next()) else {
                return;
            };
            let (done_send, done) = watch::channel(false);
            let cancel = CancellationToken::new();
            {
                let mut inner = shared.inner.lock();
                if inner.active.is_some() || inner.closed || inner.forgetting {
                    return;
                }
                inner.active = Some(Active {
                    id: next.id,
                    cancel: cancel.clone(),
                    stop: None,
                    done,
                });
            }
            let id = next.id;
            let ending = shared.launch(id, cancel).await;
            let broken = ending == Ending::Broken;
            let stop = shared.finish(id, ending).await;
            shared.inner.lock().active = None;
            let _ = done_send.send(true);
            // Fermeture de l'app (ou travail arrêté avant un effacement) : rien
            // de plus ne se lance. Après un oubli (suppression, effacement),
            // la file continue : `forget` a sorti de la file les projets qui
            // attendaient, ce qui s'y trouve est neuf.
            if !broken && stop != Some(Stop::Close) {
                shared.dispatch();
            }
        });
    }

    /// Mène le projet `id` (place réservée).
    async fn launch(self: &Arc<Self>, id: i64, cancel: CancellationToken) -> Ending {
        let started = self
            .store
            .call(move |c| {
                let row = db::get(c, id)?;
                match row {
                    Some(row) if row.state == "queued" => {
                        db::set_state(c, id, "running", None, now_ms())?;
                        Ok(Some((row, db::attachments(c, id)?)))
                    }
                    _ => Ok(None),
                }
            })
            .await;
        let (row, attachments) = match started {
            Ok(Some(found)) => found,
            Ok(None) => return Ending::Skipped,
            Err(error) => {
                // i18n-ignore : journal.
                crate::app_log!("chatonlm: workshop: project {id} not started: {error}");
                return Ending::Broken;
            }
        };
        self.publish(WorkshopUpdate::State {
            project: id,
            state: ProjectState::Running,
        });
        let info = ProjectInfo {
            id,
            title: row.title.clone(),
            task: row.task.clone(),
            attachments: attachments
                .into_iter()
                .map(|(name, text)| Attachment { name, text })
                .collect(),
            strategy: row
                .strategy_forced
                .then(|| row.strategy.as_deref().and_then(dec))
                .flatten(),
            autonomy: dec(&row.autonomy).unwrap_or(super::pipeline::types::Autonomy::Ask),
            app_language: row.language.clone(),
            dir: PathBuf::from(&row.folder),
        };
        let host = Arc::new(ProjectHost::new(self.clone(), info, cancel.clone()));
        let folder = PathBuf::from(&row.folder);
        if !tauri::async_runtime::spawn_blocking(move || folder.is_dir())
            .await
            .unwrap_or(false)
        {
            return self.failed(&host, "folderMissing").await;
        }
        let choice = tokio::select! {
            biased;
            () = cancel.cancelled() => return Ending::Stopped,
            choice = self.deps.runner.choose(row.model.clone()) => choice,
        };
        let choice = match choice {
            Ok(choice) => choice,
            Err(code) => return self.failed(&host, code).await,
        };
        let model = choice.id.clone();
        let _ = self
            .store
            .call(move |c| db::set_model_used(c, id, &model, now_ms()))
            .await;
        host.emit_kind(EventKind::ModelWarnings {
            model: choice.id.clone(),
            warnings: choice.warnings.iter().map(enc).collect(),
        })
        .await;
        let outcome = self.deps.runner.run(host.clone(), choice).await;
        if cancel.is_cancelled() && outcome != Outcome::Delivered {
            return Ending::Stopped;
        }
        match outcome {
            Outcome::Delivered => {
                if !host.finished() {
                    host.emit_kind(EventKind::Finished {
                        state: ProjectState::Done,
                    })
                    .await;
                }
                Ending::Done
            }
            Outcome::Cancelled => Ending::Stopped,
            Outcome::Rejected => Ending::Rejected,
            Outcome::Failed(code) => self.failed(&host, &code).await,
        }
    }

    /// Échec `code` : erreur et fin dans le journal (si le pipeline ne les a
    /// pas déjà écrites).
    async fn failed(&self, host: &ProjectHost, code: &str) -> Ending {
        if !host.finished() {
            host.emit_kind(EventKind::Error { code: code.into() }).await;
            host.emit_kind(EventKind::Finished {
                state: ProjectState::Failed,
            })
            .await;
        }
        Ending::Failed(code.into())
    }

    /// État de fin du projet `id`, notification ; rend la raison de l'arrêt.
    async fn finish(&self, id: i64, ending: Ending) -> Option<Stop> {
        let stop = self
            .inner
            .lock()
            .active
            .as_ref()
            .and_then(|active| active.stop);
        self.forget_waiter(id);
        let title = || async move {
            self.store
                .call(move |c| db::get(c, id))
                .await
                .ok()
                .flatten()
                .map(|row| row.title)
                .unwrap_or_default()
        };
        match (ending, stop) {
            (Ending::Skipped | Ending::Broken, _) | (_, Some(Stop::Forget)) => {}
            (Ending::Done, _) => {
                self.set_state(id, ProjectState::Done, None).await;
                self.alert(Alert::Done {
                    title: title().await,
                });
            }
            // Place rendue pendant l'attente d'une réponse : la question reste
            // en attente, l'état aussi (ou déjà remis en file par la réponse).
            (_, Some(Stop::Park)) => {}
            (
                Ending::Stopped | Ending::Failed(_) | Ending::Rejected,
                Some(Stop::Pause | Stop::Close),
            )
            | (Ending::Stopped, None) => {
                let _ = self.store.call(move |c| db::drop_pending(c, id)).await;
                self.set_state(id, ProjectState::Paused, None).await;
            }
            (Ending::Stopped | Ending::Failed(_) | Ending::Rejected, Some(Stop::Cancel))
            | (Ending::Rejected, None) => {
                let _ = self.store.call(move |c| db::drop_questions(c, id)).await;
                self.set_state(id, ProjectState::Cancelled, None).await;
            }
            (Ending::Failed(code), None) => {
                let _ = self.store.call(move |c| db::drop_pending(c, id)).await;
                self.set_state(id, ProjectState::Failed, Some(code)).await;
                self.alert(Alert::Failed {
                    title: title().await,
                });
            }
        }
        stop
    }

    /// Arrête le projet actif `id` (s'il l'est) pour `stop` ; rend l'attente
    /// de sa fin.
    fn stop_active(&self, id: Option<i64>, stop: Stop) -> Option<watch::Receiver<bool>> {
        let mut inner = self.inner.lock();
        let active = inner.active.as_mut()?;
        if id.is_some_and(|id| id != active.id) {
            return None;
        }
        // Une suppression l'emporte sur tout ; une fermeture ne défait pas
        // une annulation demandée ; toute demande l'emporte sur une place
        // rendue.
        active.stop = match (active.stop, stop) {
            (Some(Stop::Forget), _) => Some(Stop::Forget),
            (Some(Stop::Cancel), Stop::Close) => Some(Stop::Cancel),
            (Some(kept), Stop::Park) => Some(kept),
            (_, stop) => Some(stop),
        };
        active.cancel.cancel();
        Some(active.done.clone())
    }

    #[cfg(test)]
    fn active_id(&self) -> Option<i64> {
        self.inner.lock().active.as_ref().map(|active| active.id)
    }
}

/// Attend `done` au plus `wait`.
async fn wait_done(done: Option<watch::Receiver<bool>>, wait: Duration) {
    if let Some(mut done) = done {
        let _ = tokio::time::timeout(wait, done.wait_for(|done| *done)).await;
    }
}

impl Workshop {
    pub(crate) fn new(store: Store, deps: Deps) -> Self {
        let throttle = Arc::new(Throttle {
            pending: Mutex::new((BTreeMap::new(), false)),
            emit: deps.notice.clone(),
            interval: deps.notice_interval,
        });
        Self {
            shared: Arc::new(Shared {
                store,
                deps,
                inner: Mutex::new(Inner::default()),
                updates: broadcast::channel(UPDATES_KEPT).0,
                throttle,
            }),
        }
    }

    /// Changements de tous les projets.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "abonnés : ligne de commande et API (lots 6 et 7)")
    )]
    pub fn subscribe(&self) -> broadcast::Receiver<WorkshopUpdate> {
        self.shared.updates.subscribe()
    }

    /// Lancement de l'app (base ouverte, avant toute commande) : rien ne
    /// calcule par surprise ; une notification dit combien de projets
    /// attendent d'être repris.
    pub fn recover(&self) {
        match self.shared.store.blocking(|c| db::recover(c, now_ms())) {
            Ok(paused) if !paused.is_empty() => {
                self.shared.alert(Alert::Paused {
                    count: paused.len(),
                });
            }
            Ok(_) => {}
            Err(error) => {
                // i18n-ignore : journal.
                crate::app_log!("chatonlm: workshop: projects not recovered: {error}");
            }
        }
    }

    /// Dossier des projets en vigueur : choisi, sinon celui par défaut.
    async fn root(&self) -> AppResult<(Option<PathBuf>, db::SettingsRow)> {
        let saved = self.shared.store.call(|c| db::settings(c)).await?;
        let root = if saved.dir.is_empty() {
            self.shared.deps.default_root.clone()
        } else {
            Some(PathBuf::from(&saved.dir))
        };
        Ok((root, saved))
    }

    pub async fn settings(&self) -> AppResult<WorkshopSettings> {
        let (root, saved) = self.root().await?;
        Ok(WorkshopSettings {
            dir: root
                .map(|root| root.to_string_lossy().into_owned())
                .unwrap_or_default(),
            custom: !saved.dir.is_empty(),
            web: saved.web,
            autonomy: dec(&saved.autonomy).unwrap_or(super::pipeline::types::Autonomy::Ask),
        })
    }

    /// Dossier rendu par le sélecteur natif : seul accepté ensuite comme
    /// dossier des projets.
    pub fn grant_dir(&self, dir: &str) {
        self.shared.inner.lock().granted_dir = Some(dir.to_owned());
    }

    /// Change les réglages ; un dossier doit venir du sélecteur natif (ou
    /// être vide : celui par défaut).
    pub async fn set_settings(&self, patch: WorkshopSettingsPatch) -> AppResult<WorkshopSettings> {
        if let Some(dir) = &patch.dir
            && !dir.is_empty()
        {
            let granted = self.shared.inner.lock().granted_dir.clone();
            if granted.as_deref() != Some(dir.as_str()) || !Path::new(dir).is_absolute() {
                return Err(AppError::Invalid(t!("workshop.badFolder")));
            }
        }
        let autonomy = patch.autonomy.as_ref().map(enc);
        self.shared
            .store
            .call(move |c| {
                db::set_settings(c, patch.dir.as_deref(), patch.web, autonomy.as_deref())
            })
            .await?;
        self.settings().await
    }

    /// Modèle qu'un projet prendrait avec `model` demandé (vide : Auto).
    pub async fn model_check(&self, model: Option<String>) -> ModelCheck {
        let requested = model.unwrap_or_default();
        let auto = requested.trim().is_empty();
        match self.shared.deps.runner.choose(requested).await {
            Ok(choice) => ModelCheck {
                model: Some(choice.id),
                auto,
                warnings: choice.warnings,
                error: None,
            },
            Err(code) => ModelCheck {
                model: None,
                auto,
                warnings: Vec::new(),
                error: Some(code.into()),
            },
        }
    }

    /// Crée un projet (dossier, sources, ligne) et le met en file ; rend son
    /// identifiant.
    pub async fn create(&self, staging: &Staging, request: NewProjectRequest) -> AppResult<i64> {
        let task = request.task.trim().to_owned();
        if task.is_empty() {
            return Err(AppError::Invalid(t!("workshop.emptyTask")));
        }
        if task.chars().count() > MAX_TASK_CHARS {
            return Err(AppError::Invalid(t!(
                "workshop.taskTooLong",
                max = MAX_TASK_CHARS
            )));
        }
        if request
            .strategy
            .is_some_and(|strategy| !Strategy::available(strategy))
        {
            return Err(AppError::Invalid(t!("workshop.strategyNotYet")));
        }
        if request.attachment_ids.len() > MAX_ATTACHMENTS {
            return Err(AppError::Invalid(t!(
                "workshop.tooManyAttachments",
                max = MAX_ATTACHMENTS
            )));
        }
        let (root, _) = self.root().await?;
        let root = root.ok_or_else(|| AppError::Invalid(t!("workshop.noFolder")))?;
        // Retirés de la zone ; remis en place si la création échoue.
        let taken = staging
            .take(&request.attachment_ids)
            .map_err(|_| AppError::Invalid(t!("workshop.unknownAttachment")))?;
        let mut sources = Vec::new();
        for file in taken.files() {
            match &file.text {
                Some(text) => sources.push((file.file.filename.clone(), text.clone())),
                None => {
                    return Err(AppError::Invalid(t!(
                        "workshop.imageAttachment",
                        name = file.file.filename
                    )));
                }
            }
        }
        let title = title_of(&task);
        // Noms de repli sur le disque : dans la langue de l'app.
        let name = files::folder_name(
            request.folder_name.as_deref().unwrap_or(&title),
            &files::folder_name(&title, &t!("workshop.disk.project")),
        );
        let source_fallback = t!("workshop.disk.source");
        let written = {
            let sources = sources.clone();
            tauri::async_runtime::spawn_blocking(move || -> std::io::Result<PathBuf> {
                let folder = files::create_project_dir(&root, &name)?;
                for (index, (filename, text)) in sources.iter().enumerate() {
                    let relative = source_path(index, filename, &source_fallback);
                    if let Err(error) = files::write_in(&folder, &relative, text.as_bytes()) {
                        error.log(&relative);
                        let _ = std::fs::remove_dir_all(&folder);
                        return Err(std::io::Error::other(error.code()));
                    }
                }
                Ok(folder)
            })
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?
        };
        // Message traduit seul : la raison du système part au journal.
        let folder = written.map_err(|error| {
            // i18n-ignore : journal.
            crate::app_log!("chatonlm: workshop: project folder not created: {error}");
            AppError::Invalid(t!("workshop.folder"))
        })?;
        let new = NewProject {
            title,
            task,
            strategy: request.strategy.as_ref().map(enc),
            autonomy: enc(&request.autonomy),
            model: request.model.unwrap_or_default().trim().to_owned(),
            language: crate::i18n::current().code().to_owned(),
            folder: folder.to_string_lossy().into_owned(),
            state: "queued".into(),
            queue_seq: 0,
            created_at: now_ms(),
        };
        let inserted = self
            .shared
            .store
            .call(move |c| {
                let mut new = new;
                new.queue_seq = db::max_queue_seq(c)? + 1;
                db::insert(c, &new, &sources)
            })
            .await;
        let id = match inserted {
            Ok(id) => id,
            Err(error) => {
                let _ =
                    tauri::async_runtime::spawn_blocking(move || std::fs::remove_dir_all(folder))
                        .await;
                return Err(error);
            }
        };
        taken.keep();
        self.shared.publish(WorkshopUpdate::State {
            project: id,
            state: ProjectState::Queued,
        });
        self.shared.dispatch();
        Ok(id)
    }

    /// Projets, du plus récent au plus ancien.
    pub async fn list(&self) -> AppResult<Vec<ProjectSummary>> {
        let rows = self.shared.store.call(|c| db::list(c)).await?;
        let positions = positions(&rows);
        rows.iter()
            .map(|row| summary(row, positions.get(&row.id).copied()))
            .collect()
    }

    /// Projet complet ; `NotFound` s'il n'existe pas.
    pub async fn get(&self, id: i64) -> AppResult<ProjectDetail> {
        let read = self
            .shared
            .store
            .call(move |c| {
                let Some(row) = db::get(c, id)? else {
                    return Ok(None);
                };
                let positions = if row.state == "queued" {
                    positions(&db::in_state(c, "queued")?)
                } else {
                    HashMap::new()
                };
                Ok(Some((
                    row,
                    positions,
                    db::attachments(c, id)?,
                    db::pending_question(c, id)?,
                    db::last_seq(c, id)?,
                    db::load_state(c, id, &StateKey::Report.name())?,
                )))
            })
            .await?;
        let (row, positions, attachments, question, last_seq, report) =
            read.ok_or_else(not_found)?;
        Ok(ProjectDetail {
            project: summary(&row, positions.get(&id).copied())?,
            task: row.task.clone(),
            attachments: attachments.into_iter().map(|(name, _)| name).collect(),
            question: question.and_then(|question| {
                Some(PendingQuestion {
                    id: question.id,
                    question: serde_json::from_str(&question.question).ok()?,
                    asked_at: question.created_at,
                })
            }),
            last_seq,
            report: report.and_then(|report| serde_json::from_str::<Report>(&report).ok()),
        })
    }

    /// Événements du projet après `since` (au plus [`MAX_EVENTS_PER_READ`]).
    pub async fn events(&self, id: i64, since: u64) -> AppResult<Vec<Event>> {
        let rows = self
            .shared
            .store
            .call(move |c| {
                if db::get(c, id)?.is_none() {
                    return Ok(None);
                }
                db::events_since(c, id, since, MAX_EVENTS_PER_READ).map(Some)
            })
            .await?
            .ok_or_else(not_found)?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                Some(Event {
                    seq: row.seq,
                    at_ms: row.at,
                    kind: serde_json::from_str(&row.event).ok()?,
                })
            })
            .collect())
    }

    /// Relit le projet, tout son journal et son état sauvé, puis applique
    /// `read` (lecteur d'état : nom de clé → JSON) ; `NotFound` s'il
    /// n'existe pas.
    async fn read_parts<T: Send + 'static>(
        &self,
        id: i64,
        read: impl FnOnce(&mut parts::Load<'_>, &[Event], &ProjectRow) -> T + Send + 'static,
    ) -> AppResult<T> {
        self.shared
            .store
            .call(move |c| {
                let Some(row) = db::get(c, id)? else {
                    return Ok(None);
                };
                let events: Vec<Event> = db::events_since(c, id, 0, usize::MAX)?
                    .into_iter()
                    .filter_map(|row| {
                        Some(Event {
                            seq: row.seq,
                            at_ms: row.at,
                            kind: serde_json::from_str(&row.event).ok()?,
                        })
                    })
                    .collect();
                let mut load = |key: &str| {
                    db::load_state(c, id, key)
                        .ok()
                        .flatten()
                        .and_then(|value| serde_json::from_str(&value).ok())
                };
                Ok(Some(read(&mut load, &events, &row)))
            })
            .await?
            .ok_or_else(not_found)
    }

    /// Parties du projet relues de l'état sauvé (version directe : id 0).
    pub async fn parts(&self, id: i64) -> AppResult<Vec<WorkshopPartSummary>> {
        self.read_parts(id, |load, events, row| {
            parts::summaries(load, events, &row.title)
        })
        .await
    }

    /// Une partie en détail ; `NotFound` si le projet ou la partie
    /// n'existe pas. Version directe livrée : le texte est le livrable.
    pub async fn part(&self, id: i64, brick: u32) -> AppResult<WorkshopPart> {
        let found = self
            .read_parts(id, move |load, events, row| {
                parts::detail(load, events, &row.title, brick).map(|(part, file)| {
                    (
                        part,
                        file.and_then(|f| parts::delivered_path(Path::new(&row.folder), &f)),
                    )
                })
            })
            .await?;
        let (mut part, file) = found.ok_or_else(not_found)?;
        if let Some(file) = file {
            let read = tauri::async_runtime::spawn_blocking(move || std::fs::read_to_string(file))
                .await
                .ok()
                .and_then(Result::ok);
            if let Some(text) = read {
                (part.text, part.truncated) = parts::clip(text);
            }
        }
        Ok(part)
    }

    /// État sauvé `key` du projet (JSON relu) ; `NotFound` si le projet
    /// n'existe pas.
    async fn saved<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        id: i64,
        key: StateKey,
    ) -> AppResult<(ProjectRow, Option<T>)> {
        let name = key.name();
        let (row, value) = self
            .shared
            .store
            .call(move |c| {
                let Some(row) = db::get(c, id)? else {
                    return Ok(None);
                };
                Ok(Some((row, db::load_state(c, id, &name)?)))
            })
            .await?
            .ok_or_else(not_found)?;
        Ok((row, value.and_then(|v| serde_json::from_str(&v).ok())))
    }

    /// Spécification retenue (`None` avant qu'elle soit écrite).
    pub async fn spec(&self, id: i64) -> AppResult<Option<Spec>> {
        Ok(self.saved(id, StateKey::Spec).await?.1)
    }

    /// Livrable du projet (chemin absolu) ; `NotFound` avant la livraison.
    pub async fn delivered(&self, id: i64) -> AppResult<PathBuf> {
        let (row, report) = self.saved::<Report>(id, StateKey::Report).await?;
        report
            .and_then(|report| parts::delivered_path(Path::new(&row.folder), &report.file))
            .ok_or_else(not_found)
    }

    /// Répond à la question en attente du projet : le projet qui l'attend la
    /// reçoit ; un projet resté en attente au lancement de l'app est remis en
    /// file (il retrouvera la réponse sans reposer la question).
    pub async fn answer(&self, id: i64, request: AnswerRequest) -> AppResult<()> {
        let question_id = request.question_id;
        let row = self
            .shared
            .store
            .call(move |c| Ok((db::get(c, id)?, db::get_question(c, id, question_id)?)))
            .await?;
        let (Some(project), Some(question)) = row else {
            return Err(not_found());
        };
        if question.status != "pending" {
            return Err(AppError::Invalid(t!("workshop.noQuestion")));
        }
        let asked: Question = serde_json::from_str(&question.question)
            .map_err(|error| AppError::Internal(error.to_string()))?;
        if !answers(&asked, &request.answer) {
            return Err(AppError::Invalid(t!("workshop.wrongAnswer")));
        }
        let text = serde_json::to_string(&request.answer)
            .map_err(|error| AppError::Internal(error.to_string()))?;
        let saved = {
            let text = text.clone();
            self.shared
                .store
                .call(move |c| db::answer_question(c, id, question_id, &text, now_ms()))
                .await?
        };
        if !saved {
            return Err(AppError::Invalid(t!("workshop.noQuestion")));
        }
        let waiter = {
            let mut inner = self.shared.inner.lock();
            match inner.waiters.remove(&id) {
                Some(waiter) if waiter.question == question_id => Some(waiter),
                Some(other) => {
                    inner.waiters.insert(id, other);
                    None
                }
                None => None,
            }
        };
        if let Some(waiter) = waiter {
            let _ = waiter.send.send(text);
        } else if project.state == "waiting" && self.shared.idle_or_parked(id) {
            // Projet qui a rendu sa place (ou app relancée) : remis en file,
            // il reprendra la réponse sans reposer la question.
            self.shared.requeue(id, &["waiting"]).await?;
        }
        Ok(())
    }

    async fn row(&self, id: i64) -> AppResult<ProjectRow> {
        self.shared
            .store
            .call(move |c| db::get(c, id))
            .await?
            .ok_or_else(not_found)
    }

    /// Met en pause : le projet actif s'arrête (état sauvé, rien ne sera
    /// refait) ; un projet en file ou en attente d'une réponse (lancement de
    /// l'app) sort de la file.
    pub async fn pause(&self, id: i64) -> AppResult<()> {
        self.row(id).await?;
        if self.shared.stop_active(Some(id), Stop::Pause).is_some() {
            return Ok(());
        }
        // État vérifié à l'écriture : un projet lancé entre-temps n'est pas
        // marqué en pause pendant qu'il calcule.
        let paused = self
            .shared
            .store
            .call(move |c| {
                let paused =
                    db::set_state_from(c, id, &["queued", "waiting"], "paused", None, now_ms())?;
                if paused {
                    db::drop_pending(c, id)?;
                }
                Ok(paused)
            })
            .await?;
        if paused {
            self.shared.publish(WorkshopUpdate::State {
                project: id,
                state: ProjectState::Paused,
            });
            return Ok(());
        }
        // Lancé entre la première vue et l'écriture : arrêté comme un actif.
        if self.shared.stop_active(Some(id), Stop::Pause).is_some() {
            return Ok(());
        }
        match state_of(&self.row(id).await?)? {
            ProjectState::Paused => Ok(()),
            _ => Err(not_allowed()),
        }
    }

    /// Reprend un projet en pause, échoué, ou en attente d'une réponse
    /// (lancement de l'app) : remis en file, il continue où il en était.
    pub async fn resume(&self, id: i64) -> AppResult<()> {
        let row = self.row(id).await?;
        // Remis en file seulement depuis l'état lu (vérifié à l'écriture) :
        // une seconde reprise, ou une reprise croisée avec un lancement, est
        // sans effet.
        match state_of(&row)? {
            ProjectState::Paused | ProjectState::Failed => {
                self.shared.requeue(id, &["paused", "failed"]).await
            }
            ProjectState::Waiting if self.shared.idle_or_parked(id) => {
                self.shared.requeue(id, &["waiting"]).await
            }
            ProjectState::Queued
            | ProjectState::Running
            | ProjectState::Yielding
            | ProjectState::Waiting => Ok(()),
            _ => Err(not_allowed()),
        }
    }

    /// Annule : le projet actif s'arrête ; ses questions sont abandonnées.
    /// Un projet fini ne s'annule pas.
    pub async fn cancel(&self, id: i64) -> AppResult<()> {
        self.row(id).await?;
        if self.shared.stop_active(Some(id), Stop::Cancel).is_some() {
            return Ok(());
        }
        // Hors calcul seulement (vérifié à l'écriture) ; un projet lancé
        // entre-temps est arrêté comme un actif.
        let cancelled = self
            .shared
            .store
            .call(move |c| {
                let cancelled = db::set_state_from(
                    c,
                    id,
                    &["queued", "waiting", "paused", "failed"],
                    "cancelled",
                    None,
                    now_ms(),
                )?;
                if cancelled {
                    db::drop_questions(c, id)?;
                }
                Ok(cancelled)
            })
            .await?;
        if cancelled {
            self.shared.publish(WorkshopUpdate::State {
                project: id,
                state: ProjectState::Cancelled,
            });
            return Ok(());
        }
        if self.shared.stop_active(Some(id), Stop::Cancel).is_some() {
            return Ok(());
        }
        Err(not_allowed())
    }

    /// Supprime le projet (arrêté d'abord s'il calcule) ; `delete_files` :
    /// son dossier aussi (seulement un dossier créé par l'Atelier, avec son
    /// `.atelier/`). Erreur après la suppression de la ligne : dossier gardé.
    pub async fn delete(&self, id: i64, delete_files: bool) -> AppResult<()> {
        let row = self.row(id).await?;
        let done = self.shared.stop_active(Some(id), Stop::Forget);
        wait_done(done, Duration::from_secs(10)).await;
        self.shared.forget_waiter(id);
        self.shared.store.call(move |c| db::delete(c, id)).await?;
        self.shared.publish(WorkshopUpdate::Deleted { project: id });
        if delete_files {
            let folder = PathBuf::from(&row.folder);
            let removed = tauri::async_runtime::spawn_blocking(move || {
                if removable_project_dir(&folder) {
                    std::fs::remove_dir_all(&folder)
                } else {
                    Ok(())
                }
            })
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
            if let Err(error) = removed {
                // i18n-ignore : journal.
                crate::app_log!("chatonlm: workshop: folder of project {id} kept: {error}");
                return Err(AppError::Invalid(t!("workshop.folderKept")));
            }
        }
        Ok(())
    }

    /// Dossier du projet (pour l'ouvrir) ; `NotFound` s'il n'existe plus.
    pub async fn folder(&self, id: i64) -> AppResult<PathBuf> {
        let folder = PathBuf::from(self.row(id).await?.folder);
        let exists = {
            let folder = folder.clone();
            tauri::async_runtime::spawn_blocking(move || folder.is_dir())
                .await
                .unwrap_or(false)
        };
        if exists {
            Ok(folder)
        } else {
            Err(AppError::NotFound(t!("workshop.folderMissing")))
        }
    }

    /// Fermeture de l'app, ou effacement des données (sans attendre) : le
    /// projet actif s'arrête et restera en pause, sans lancer le suivant.
    pub fn cancel_all(&self) {
        self.shared.stop_active(None, Stop::Close);
    }

    /// Fermeture de l'app : comme [`Self::cancel_all`], en attendant la fin
    /// du projet actif au plus `wait` (son état est enregistré).
    pub async fn shutdown(&self, wait: Duration) {
        self.shared.inner.lock().closed = true;
        let done = self.shared.stop_active(None, Stop::Close);
        wait_done(done, wait).await;
    }

    /// « Erase my data » : le projet actif s'arrête sans rien écrire (la base
    /// va être effacée), au plus `wait` ; les réponses attendues sont
    /// oubliées. L'Atelier reste ouvert aux nouveaux projets.
    pub async fn forget(&self, wait: Duration) {
        // Rien ne se lance pendant l'oubli ; l'actif s'arrête sans rien
        // écrire (aussitôt : une fermeture demandée avant ne l'emporte pas).
        self.shared.inner.lock().forgetting = true;
        let done = self.shared.stop_active(None, Stop::Forget);
        // Rien de ce qui attendait ne se lance avant l'effacement (base
        // gardée : repris d'un clic).
        let paused = self
            .shared
            .store
            .call(|c| db::pause_queue(c, now_ms()))
            .await;
        if let Err(error) = paused {
            // i18n-ignore : journal.
            crate::app_log!("chatonlm: workshop: queue not emptied: {error}");
        }
        wait_done(done, wait).await;
        {
            let mut inner = self.shared.inner.lock();
            inner.waiters.clear();
            inner.granted_dir = None;
            inner.forgetting = false;
        }
        // La file reprend (rien n'y est plus) : un projet créé après
        // l'effacement se lance, même si l'ancien a fini après `wait` (sa fin
        // relance aussi la file).
        self.shared.dispatch();
    }

    /// Essais : le projet actif.
    #[cfg(test)]
    pub(crate) fn active(&self) -> Option<i64> {
        self.shared.active_id()
    }

    /// Essais : relance la file (après un `recover` sur une base remplie à
    /// la main).
    #[cfg(test)]
    pub(crate) fn kick(&self) {
        self.shared.dispatch();
    }
}

#[cfg(test)]
mod unit {
    use super::*;
    use crate::workshop::pipeline::types::{Plan, Strategy};

    #[test]
    fn a_title_is_the_first_line_bounded_at_a_word() {
        assert_eq!(
            title_of("\n  Write   a guide \nwith details"),
            "Write a guide"
        );
        let long = format!("{} end", "word ".repeat(30));
        let title = title_of(&long);
        assert!(title.ends_with('…'));
        assert!(title.chars().count() <= MAX_TITLE_CHARS + 1);
        assert!(!title.contains("wor…"), "{title}");
    }

    #[test]
    fn an_answer_must_match_its_question() {
        let framing = Question::Framing(vec!["a".into(), "b".into()]);
        assert!(answers(
            &framing,
            &Answer::Framing(vec![String::new(), "x".into()])
        ));
        assert!(!answers(&framing, &Answer::Framing(vec!["x".into()])));
        assert!(!answers(&framing, &Answer::Action(true)));
        assert!(!answers(&framing, &Answer::Cancelled));
        let plan = Question::ApprovePlan(Plan {
            title: String::new(),
            bricks: Vec::new(),
        });
        assert!(answers(
            &plan,
            &Answer::Plan {
                approved: true,
                edited: None,
                note: None
            }
        ));
        let strategy = Question::Strategy(vec![Strategy::LongForm, Strategy::Generic]);
        assert!(answers(&strategy, &Answer::Strategy(Strategy::Generic)));
        assert!(!answers(&strategy, &Answer::Strategy(Strategy::Code)));
    }

    #[test]
    fn queue_positions_follow_the_queue_order() {
        let row = |id: i64, state: &str, seq: i64| ProjectRow {
            id,
            title: String::new(),
            task: String::new(),
            strategy: None,
            strategy_forced: false,
            autonomy: "ask".into(),
            model: String::new(),
            model_used: String::new(),
            language: "en".into(),
            state: state.into(),
            phase: None,
            folder: String::new(),
            queue_seq: seq,
            error: None,
            created_at: 0,
            updated_at: 0,
            finished_at: None,
        };
        let rows = [
            row(1, "queued", 9),
            row(2, "running", 1),
            row(3, "queued", 4),
        ];
        let positions = positions(&rows);
        assert_eq!(positions.get(&3), Some(&1));
        assert_eq!(positions.get(&1), Some(&2));
        assert_eq!(positions.get(&2), None);
    }
}
