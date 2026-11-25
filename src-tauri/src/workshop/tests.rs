//! Essais du socle de l'Atelier : file (un projet à la fois, ordre), hôte
//! réel (événements, état, questions, cession, écriture), arrêts (pause,
//! annulation, fermeture, effacement, suppression), reprise au lancement,
//! réglages, avis bornés vers la page. Le pipeline est remplacé par un faux
//! scénarisé ([`FakeRunner`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use parking_lot::Mutex;
use serde_json::json;
use tokio::sync::Notify;

use super::model::{ChoiceError, ModelChoice};
use super::pipeline::host::Host;
use super::pipeline::types::{
    Answer, Autonomy, EventKind, Phase, ProjectState, Question, StateKey, Strategy,
};
use super::project::ProjectHost;
use super::service::{Alert, Deps, Outcome, Runner, Workshop, WorkshopUpdate};
use super::{AnswerRequest, NewProjectRequest, WorkshopNotice, WorkshopSettingsPatch};
use crate::attachments::{StageResult, Staging};
use crate::engine::auto::ModelWarning;
use crate::error::AppError;
use crate::resources::Interactive;
use crate::store::{Store, workshop as db};

/// Projets lancés : numéro, pièces jointes (nom, texte) reçues.
type Runs = Vec<(i64, Vec<(String, String)>)>;

type Script = Arc<dyn Fn(Arc<ProjectHost>) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

/// Faux pipeline : modèle choisi d'avance, script mené contre l'hôte.
struct FakeRunner {
    choice: Mutex<Result<ModelChoice, ChoiceError>>,
    script: Mutex<Script>,
    /// Projets lancés (ordre), avec ce que l'hôte leur a donné.
    runs: Mutex<Runs>,
}

impl Runner for FakeRunner {
    fn choose(&self, _requested: String) -> BoxFuture<'static, Result<ModelChoice, ChoiceError>> {
        let choice = self.choice.lock().clone();
        Box::pin(async move { choice })
    }

    fn run(&self, host: Arc<ProjectHost>, _model: ModelChoice) -> BoxFuture<'static, Outcome> {
        let info = host.project();
        self.runs.lock().push((
            info.id,
            info.attachments
                .iter()
                .map(|a| (a.name.clone(), a.text.clone()))
                .collect(),
        ));
        let script = self.script.lock().clone();
        Box::pin(async move {
            // Script en `Result` (plus court) : issue du pipeline.
            match script(host).await {
                Ok(()) => Outcome::Delivered,
                Err(code) if code == "cancelled" => Outcome::Cancelled,
                Err(code) if code == "rejected" => Outcome::Rejected,
                Err(code) => Outcome::Failed(code),
            }
        })
    }
}

fn model(warnings: Vec<ModelWarning>) -> ModelChoice {
    ModelChoice {
        id: "big/m:Q4_K_M".into(),
        ctx: 32_768,
        warnings,
        auto: true,
    }
}

/// Script qui finit aussitôt, sans rien émettre.
fn quick() -> Script {
    Arc::new(|_host| Box::pin(async { Ok(()) }))
}

struct Harness {
    workshop: Workshop,
    store: Store,
    runner: Arc<FakeRunner>,
    alerts: Arc<Mutex<Vec<Alert>>>,
    notices: Arc<Mutex<Vec<WorkshopNotice>>>,
    interactive: Interactive,
    staging: Staging,
    root: tempfile::TempDir,
}

const GRACE: Duration = Duration::from_millis(30);

fn harness_with(store: Store, script: Script, notice_interval: Duration) -> Harness {
    harness_parking(store, script, notice_interval, super::service::PARK_AFTER)
}

fn harness_parking(
    store: Store,
    script: Script,
    notice_interval: Duration,
    park_after: Duration,
) -> Harness {
    let root = tempfile::tempdir().unwrap();
    let runner = Arc::new(FakeRunner {
        choice: Mutex::new(Ok(model(vec![ModelWarning::Small]))),
        script: Mutex::new(script),
        runs: Mutex::new(Vec::new()),
    });
    let alerts = Arc::new(Mutex::new(Vec::new()));
    let notices = Arc::new(Mutex::new(Vec::new()));
    let interactive = Interactive::new();
    let (alerting, noticing) = (alerts.clone(), notices.clone());
    let workshop = Workshop::new(
        store.clone(),
        Deps {
            runner: runner.clone(),
            alert: Arc::new(move |alert| alerting.lock().push(alert)),
            notice: Arc::new(move |notice| noticing.lock().push(notice)),
            interactive: interactive.clone(),
            grace: GRACE,
            notice_interval,
            default_root: Some(root.path().join("ChatonLM")),
            park_after,
        },
    );
    Harness {
        workshop,
        store,
        runner,
        alerts,
        notices,
        interactive,
        staging: Staging::default(),
        root,
    }
}

fn harness(script: Script) -> Harness {
    harness_with(
        Store::open_in_memory().unwrap(),
        script,
        Duration::from_millis(5),
    )
}

fn request(task: &str) -> NewProjectRequest {
    NewProjectRequest {
        task: task.into(),
        attachment_ids: Vec::new(),
        strategy: None,
        autonomy: Autonomy::Ask,
        model: None,
        folder_name: None,
    }
}

impl Harness {
    async fn create(&self, task: &str) -> i64 {
        self.workshop
            .create(&self.staging, request(task))
            .await
            .unwrap()
    }

    async fn state(&self, id: i64) -> ProjectState {
        self.workshop.get(id).await.unwrap().project.state
    }

    /// Attend que le projet soit dans `state` (5 s au plus).
    async fn wait_state(&self, id: i64, state: ProjectState) {
        for _ in 0..500 {
            if self.state(id).await == state {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "project {id} never {state:?}, still {:?}",
            self.state(id).await
        );
    }

    /// Attend que plus rien ne calcule (5 s au plus).
    async fn wait_idle(&self) {
        for _ in 0..500 {
            if self.workshop.active().is_none() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("still running");
    }

    async fn kinds(&self, id: i64) -> Vec<EventKind> {
        self.workshop
            .events(id, 0)
            .await
            .unwrap()
            .into_iter()
            .map(|event| event.kind)
            .collect()
    }
}

/// Barrières par projet : le script attend la sienne (ou l'arrêt).
#[derive(Clone, Default)]
struct Gates(Arc<Mutex<HashMap<i64, Arc<Notify>>>>);

impl Gates {
    fn gate(&self, id: i64) -> Arc<Notify> {
        self.0.lock().entry(id).or_default().clone()
    }

    fn open(&self, id: i64) {
        self.gate(id).notify_one();
    }

    /// Script qui attend sa barrière, puis finit ; arrêté : erreur.
    fn script(&self) -> Script {
        let gates = self.clone();
        Arc::new(move |host| {
            let gate = gates.gate(host.project().id);
            Box::pin(async move {
                let cancel = host.cancelled();
                tokio::select! {
                    () = gate.notified() => Ok(()),
                    () = cancel.cancelled() => Err("cancelled".into()),
                }
            })
        })
    }
}

#[tokio::test]
async fn a_created_project_runs_and_its_events_are_recorded() {
    let h = harness(Arc::new(|host| {
        Box::pin(async move {
            host.yield_to_user().await;
            host.emit(EventKind::PhaseStarted {
                phase: Phase::Strategy,
            })
            .await;
            host.emit(EventKind::StrategyChosen {
                strategy: Strategy::LongForm,
                unanimous: true,
            })
            .await;
            host.save(StateKey::Spec, json!({"goal": "guide"})).await;
            host.write_file("guide.md", b"# Guide").await?;
            host.emit(EventKind::Finished {
                state: ProjectState::Done,
            })
            .await;
            Ok(())
        })
    }));
    let mut updates = h.workshop.subscribe();
    let StageResult::Staged { file } = h
        .staging
        .stage_bytes("notes.md".into(), b"mes notes".to_vec())
        .await
    else {
        panic!("not staged");
    };
    let mut new = request("Write a guide to urban permaculture\nwith 8 sections");
    new.attachment_ids = vec![file.id];
    let id = h.workshop.create(&h.staging, new).await.unwrap();
    h.wait_state(id, ProjectState::Done).await;

    let detail = h.workshop.get(id).await.unwrap();
    assert_eq!(detail.project.title, "Write a guide to urban permaculture");
    assert_eq!(detail.project.strategy, Some(Strategy::LongForm));
    assert!(!detail.project.strategy_forced);
    assert_eq!(detail.project.phase, Some(Phase::Strategy));
    assert_eq!(detail.project.model_used, "big/m:Q4_K_M");
    assert_eq!(detail.attachments, ["notes.md"]);
    assert!(detail.project.finished_at.is_some());
    // Dossier du projet : sous le dossier des projets, nom provisoire (le
    // pipeline n'a pas donné de nom), sources gardées, livrable écrit.
    let folder = PathBuf::from(&detail.project.folder);
    assert_eq!(folder, h.root.path().join("ChatonLM").join("Project"));
    assert_eq!(
        std::fs::read_to_string(folder.join(".atelier/sources/01-notes.md.txt")).unwrap(),
        "mes notes"
    );
    assert_eq!(std::fs::read(folder.join("guide.md")).unwrap(), b"# Guide");
    // Le pipeline a reçu le texte des pièces jointes ; la pièce est sortie
    // de la zone.
    assert_eq!(
        *h.runner.runs.lock(),
        [(id, vec![("notes.md".into(), "mes notes".into())])]
    );
    // Journal : avertissements du modèle d'abord, une seule fin, aucune
    // cession (l'utilisateur ne faisait rien).
    assert_eq!(
        h.kinds(id).await,
        [
            EventKind::ModelWarnings {
                model: "big/m:Q4_K_M".into(),
                warnings: vec!["small".into()],
            },
            EventKind::PhaseStarted {
                phase: Phase::Strategy
            },
            EventKind::StrategyChosen {
                strategy: Strategy::LongForm,
                unanimous: true
            },
            EventKind::Finished {
                state: ProjectState::Done
            },
        ]
    );
    let saved = h
        .store
        .call(move |c| db::load_state(c, id, "spec"))
        .await
        .unwrap();
    assert_eq!(saved.as_deref(), Some(r#"{"goal":"guide"}"#));
    assert_eq!(
        *h.alerts.lock(),
        [Alert::Done {
            title: "Write a guide to urban permaculture".into()
        }]
    );
    // Abonnés : création, lancement, événements numérotés, fin.
    let mut seen = Vec::new();
    while let Ok(update) = updates.try_recv() {
        seen.push(update);
    }
    assert_eq!(
        seen.first(),
        Some(&WorkshopUpdate::State {
            project: id,
            state: ProjectState::Queued
        })
    );
    let seqs: Vec<u64> = seen
        .iter()
        .filter_map(|update| match update {
            WorkshopUpdate::Event { event, .. } => Some(event.seq),
            _ => None,
        })
        .collect();
    assert_eq!(seqs, [1, 2, 3, 4]);
    assert_eq!(
        seen.last(),
        Some(&WorkshopUpdate::State {
            project: id,
            state: ProjectState::Done
        })
    );
    // Lecture depuis un numéro.
    let after = h.workshop.events(id, 3).await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].seq, 4);
}

#[tokio::test]
async fn one_project_computes_at_a_time_in_arrival_order() {
    let gates = Gates::default();
    let h = harness(gates.script());
    let first = h.create("First").await;
    let second = h.create("Second").await;
    let third = h.create("Third").await;
    h.wait_state(first, ProjectState::Running).await;
    let list = h.workshop.list().await.unwrap();
    let position = |id: i64| list.iter().find(|p| p.id == id).unwrap().position;
    assert_eq!(position(first), None);
    assert_eq!(position(second), Some(1));
    assert_eq!(position(third), Some(2));
    assert_eq!(h.state(second).await, ProjectState::Queued);
    gates.open(first);
    h.wait_state(second, ProjectState::Running).await;
    assert_eq!(h.state(first).await, ProjectState::Done);
    assert_eq!(h.state(third).await, ProjectState::Queued);
    assert_eq!(
        h.workshop.get(third).await.unwrap().project.position,
        Some(1)
    );
    gates.open(second);
    h.wait_state(third, ProjectState::Running).await;
    gates.open(third);
    h.wait_state(third, ProjectState::Done).await;
    let order: Vec<i64> = h.runner.runs.lock().iter().map(|(id, _)| *id).collect();
    assert_eq!(order, [first, second, third]);
}

#[tokio::test]
async fn a_queued_project_paused_leaves_the_queue_until_resumed() {
    let gates = Gates::default();
    let h = harness(gates.script());
    let first = h.create("First").await;
    let second = h.create("Second").await;
    let third = h.create("Third").await;
    h.wait_state(first, ProjectState::Running).await;
    h.workshop.pause(second).await.unwrap();
    assert_eq!(h.state(second).await, ProjectState::Paused);
    let detail = h.workshop.get(third).await.unwrap();
    assert_eq!(detail.project.position, Some(1));
    // Le suivant de la file passe avant le projet en pause.
    gates.open(first);
    h.wait_state(third, ProjectState::Running).await;
    assert_eq!(h.state(second).await, ProjectState::Paused);
    // Repris : remis en file, en dernier.
    h.workshop.resume(second).await.unwrap();
    assert_eq!(h.state(second).await, ProjectState::Queued);
    gates.open(third);
    h.wait_state(second, ProjectState::Running).await;
    gates.open(second);
    h.wait_state(second, ProjectState::Done).await;
    let order: Vec<i64> = h.runner.runs.lock().iter().map(|(id, _)| *id).collect();
    assert_eq!(order, [first, third, second]);
}

#[tokio::test]
async fn a_model_that_cannot_be_chosen_fails_the_project_and_the_next_runs() {
    let h = harness(quick());
    *h.runner.choice.lock() = Err("noModel");
    let failed = h.create("Doomed").await;
    h.wait_state(failed, ProjectState::Failed).await;
    let detail = h.workshop.get(failed).await.unwrap();
    assert_eq!(detail.project.error.as_deref(), Some("noModel"));
    assert_eq!(
        h.kinds(failed).await,
        [
            EventKind::Error {
                code: "noModel".into()
            },
            EventKind::Finished {
                state: ProjectState::Failed
            }
        ]
    );
    assert!(h.runner.runs.lock().is_empty());
    assert_eq!(
        *h.alerts.lock(),
        [Alert::Failed {
            title: "Doomed".into()
        }]
    );
    // La file continue.
    *h.runner.choice.lock() = Ok(model(Vec::new()));
    let next = h.create("Next").await;
    h.wait_state(next, ProjectState::Done).await;
    // Reprise d'un échec : relancé.
    h.workshop.resume(failed).await.unwrap();
    h.wait_state(failed, ProjectState::Done).await;
    assert_eq!(h.workshop.get(failed).await.unwrap().project.error, None);
}

#[tokio::test]
async fn a_pipeline_failure_is_recorded_once() {
    let h = harness(Arc::new(|host| {
        Box::pin(async move {
            host.emit(EventKind::Error {
                code: "looping".into(),
            })
            .await;
            host.emit(EventKind::Finished {
                state: ProjectState::Failed,
            })
            .await;
            Err("looping".into())
        })
    }));
    let id = h.create("Loop").await;
    h.wait_state(id, ProjectState::Failed).await;
    let kinds = h.kinds(id).await;
    let finished = kinds
        .iter()
        .filter(|kind| matches!(kind, EventKind::Finished { .. }))
        .count();
    assert_eq!(finished, 1, "{kinds:?}");
    assert_eq!(
        h.workshop.get(id).await.unwrap().project.error.as_deref(),
        Some("looping")
    );
}

/// Script qui pose deux questions de cadrage et garde la réponse.
fn asking(answers: Arc<Mutex<Vec<Answer>>>) -> Script {
    Arc::new(move |host| {
        let answers = answers.clone();
        Box::pin(async move {
            let answer = host
                .ask(Question::Framing(vec![
                    "Who reads?".into(),
                    "Length?".into(),
                ]))
                .await;
            let cancelled = answer == Answer::Cancelled;
            answers.lock().push(answer);
            if cancelled {
                Err("cancelled".into())
            } else {
                Ok(())
            }
        })
    })
}

#[tokio::test]
async fn a_question_waits_for_its_answer() {
    let answers = Arc::new(Mutex::new(Vec::new()));
    let h = harness(asking(answers.clone()));
    let id = h.create("Guide").await;
    h.wait_state(id, ProjectState::Waiting).await;
    let question = h.workshop.get(id).await.unwrap().question.unwrap();
    assert_eq!(
        question.question,
        Question::Framing(vec!["Who reads?".into(), "Length?".into()])
    );
    assert_eq!(
        *h.alerts.lock(),
        [Alert::Waiting {
            title: "Guide".into()
        }]
    );
    // Mauvaise forme, mauvais nombre, mauvaise question : refusés.
    for wrong in [
        Answer::Action(true),
        Answer::Framing(vec!["one".into()]),
        Answer::Cancelled,
    ] {
        let refused = h
            .workshop
            .answer(
                id,
                AnswerRequest {
                    question_id: question.id,
                    answer: wrong,
                },
            )
            .await;
        assert!(matches!(refused, Err(AppError::Invalid(_))), "{refused:?}");
    }
    let unknown = h
        .workshop
        .answer(
            id,
            AnswerRequest {
                question_id: question.id + 1,
                answer: Answer::Framing(vec![]),
            },
        )
        .await;
    assert!(matches!(unknown, Err(AppError::NotFound(_))));
    assert_eq!(h.state(id).await, ProjectState::Waiting);
    let reply = Answer::Framing(vec!["Gardeners".into(), String::new()]);
    h.workshop
        .answer(
            id,
            AnswerRequest {
                question_id: question.id,
                answer: reply.clone(),
            },
        )
        .await
        .unwrap();
    h.wait_state(id, ProjectState::Done).await;
    assert_eq!(*answers.lock(), std::slice::from_ref(&reply));
    // Répondue une fois seulement.
    let again = h
        .workshop
        .answer(
            id,
            AnswerRequest {
                question_id: question.id,
                answer: reply.clone(),
            },
        )
        .await;
    assert!(matches!(again, Err(AppError::Invalid(_))));
    assert!(h.workshop.get(id).await.unwrap().question.is_none());
    let row = h
        .store
        .call(move |c| db::get_question(c, id, question.id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "used");
}

#[tokio::test]
async fn cancelling_while_waiting_drops_the_question() {
    let answers = Arc::new(Mutex::new(Vec::new()));
    let h = harness(asking(answers.clone()));
    let id = h.create("Guide").await;
    h.wait_state(id, ProjectState::Waiting).await;
    let question = h.workshop.get(id).await.unwrap().question.unwrap();
    h.workshop.cancel(id).await.unwrap();
    h.wait_state(id, ProjectState::Cancelled).await;
    assert_eq!(*answers.lock(), [Answer::Cancelled]);
    let row = h
        .store
        .call(move |c| db::get_question(c, id, question.id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "dropped");
    // Une annulation n'écrit pas de fin ; un projet annulé ne s'annule plus.
    assert!(
        !h.kinds(id)
            .await
            .iter()
            .any(|kind| matches!(kind, EventKind::Finished { .. }))
    );
    assert!(h.workshop.cancel(id).await.is_err());
    assert!(h.workshop.resume(id).await.is_err());
}

#[tokio::test]
async fn a_paused_project_resumes_without_redoing_saved_work() {
    let saves = Arc::new(Mutex::new(0));
    let counting = saves.clone();
    let h = harness(Arc::new(move |host| {
        let saves = counting.clone();
        Box::pin(async move {
            if host.load(StateKey::Spec).await.is_none() {
                host.save(StateKey::Spec, json!("spec")).await;
                *saves.lock() += 1;
                // Long travail, jusqu'à l'arrêt.
                host.cancelled().cancelled().await;
                return Err("cancelled".into());
            }
            Ok(())
        })
    }));
    let id = h.create("Guide").await;
    h.wait_state(id, ProjectState::Running).await;
    // Attendre l'écriture de l'état.
    for _ in 0..500 {
        if *saves.lock() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    h.workshop.pause(id).await.unwrap();
    h.wait_state(id, ProjectState::Paused).await;
    h.wait_idle().await;
    h.workshop.resume(id).await.unwrap();
    h.wait_state(id, ProjectState::Done).await;
    assert_eq!(*saves.lock(), 1, "the saved spec is not written again");
    assert_eq!(h.runner.runs.lock().len(), 2);
}

#[tokio::test]
async fn an_answer_given_after_a_restart_is_found_without_asking_again() {
    let answers = Arc::new(Mutex::new(Vec::new()));
    let store = Store::open_in_memory().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let question = serde_json::to_string(&Question::Framing(vec![
        "Who reads?".into(),
        "Length?".into(),
    ]))
    .unwrap();
    // L'app a été coupée pendant que le projet attendait sa réponse.
    let folder_path = folder.path().to_string_lossy().into_owned();
    let (id, question_id) = store
        .blocking(move |c| {
            let id = db::insert(
                c,
                &db::NewProject {
                    title: "Guide".into(),
                    task: "Write a guide".into(),
                    strategy: None,
                    autonomy: "ask".into(),
                    model: String::new(),
                    language: "en".into(),
                    folder: folder_path,
                    folder_named: true,
                    state: "waiting".into(),
                    queue_seq: 1,
                    created_at: 1,
                },
                &[],
            )?;
            Ok((id, db::add_question(c, id, &question, 2)?))
        })
        .unwrap();
    let h = harness_with(store, asking(answers.clone()), Duration::from_millis(5));
    h.workshop.recover();
    assert_eq!(h.state(id).await, ProjectState::Waiting, "still waiting");
    assert!(h.alerts.lock().is_empty(), "nothing paused");
    let reply = Answer::Framing(vec!["Gardeners".into(), "Short".into()]);
    h.workshop
        .answer(
            id,
            AnswerRequest {
                question_id,
                answer: reply.clone(),
            },
        )
        .await
        .unwrap();
    h.wait_state(id, ProjectState::Done).await;
    assert_eq!(*answers.lock(), [reply]);
    // Pas reposée : une seule question, lue ; aucune alerte « J'ai besoin de
    // toi ».
    let statuses: Vec<String> = h
        .store
        .call(move |c| {
            Ok(
                c.prepare("SELECT status FROM workshop_questions WHERE project_id = ?1")?
                    .query_map([id], |row| row.get(0))?
                    .collect::<Result<_, _>>()?,
            )
        })
        .await
        .unwrap();
    assert_eq!(statuses, ["used"]);
    assert_eq!(
        *h.alerts.lock(),
        [Alert::Done {
            title: "Guide".into()
        }]
    );
}

#[tokio::test]
async fn a_plan_rejected_without_a_note_cancels_the_project() {
    let h = harness(Arc::new(|_host| Box::pin(async { Err("rejected".into()) })));
    let id = h.create("Guide").await;
    h.wait_state(id, ProjectState::Cancelled).await;
    assert_eq!(h.workshop.get(id).await.unwrap().project.error, None);
    assert!(h.alerts.lock().is_empty());
}

#[tokio::test]
async fn at_launch_interrupted_projects_are_paused_and_announced() {
    let store = Store::open_in_memory().unwrap();
    store
        .blocking(|c| {
            for (title, state) in [("a", "running"), ("b", "queued"), ("c", "done")] {
                db::insert(
                    c,
                    &db::NewProject {
                        title: title.into(),
                        task: "t".into(),
                        strategy: None,
                        autonomy: "ask".into(),
                        model: String::new(),
                        language: "en".into(),
                        folder: "/nowhere".into(),
                        folder_named: true,
                        state: state.into(),
                        queue_seq: 1,
                        created_at: 1,
                    },
                    &[],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let h = harness_with(store, quick(), Duration::from_millis(5));
    h.workshop.recover();
    assert_eq!(*h.alerts.lock(), [Alert::Paused { count: 2 }]);
    assert_eq!(h.state(1).await, ProjectState::Paused);
    assert_eq!(h.state(2).await, ProjectState::Paused);
    assert_eq!(h.state(3).await, ProjectState::Done);
    // Rien ne se lance de lui-même.
    h.workshop.kick();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(h.runner.runs.lock().is_empty());
}

#[tokio::test]
async fn the_project_gives_way_to_the_user() {
    let h = harness(Arc::new(|host| {
        Box::pin(async move {
            host.yield_to_user().await;
            Ok(())
        })
    }));
    let hold = h.interactive.hold();
    let id = h.create("Guide").await;
    h.wait_state(id, ProjectState::Yielding).await;
    tokio::time::sleep(GRACE * 3).await;
    assert_eq!(
        h.state(id).await,
        ProjectState::Yielding,
        "the user is busy"
    );
    drop(hold);
    h.wait_state(id, ProjectState::Done).await;
    let kinds = h.kinds(id).await;
    assert_eq!(&kinds[1..3], [EventKind::Yielding, EventKind::Resumed]);
}

#[tokio::test]
async fn files_are_written_only_under_the_project_folder() {
    let results = Arc::new(Mutex::new(Vec::new()));
    let recording = results.clone();
    let h = harness(Arc::new(move |host| {
        let results = recording.clone();
        Box::pin(async move {
            let outside = host.project().dir.parent().unwrap().join("evil.md");
            for path in [
                "../evil.md".to_owned(),
                outside.to_string_lossy().into_owned(),
                "chapitres/un.md".to_owned(),
            ] {
                let written = host.write_file(&path, b"x").await;
                results.lock().push(written);
            }
            Ok(())
        })
    }));
    let id = h.create("Guide").await;
    h.wait_state(id, ProjectState::Done).await;
    assert_eq!(
        *results.lock(),
        [Err("badPath".into()), Err("badPath".into()), Ok(())]
    );
    let folder = PathBuf::from(h.workshop.get(id).await.unwrap().project.folder);
    assert!(folder.join("chapitres/un.md").is_file());
    assert!(!folder.parent().unwrap().join("evil.md").exists());
}

#[tokio::test]
async fn closing_the_app_pauses_the_project_and_launches_nothing_more() {
    let gates = Gates::default();
    let h = harness(gates.script());
    let first = h.create("First").await;
    let second = h.create("Second").await;
    h.wait_state(first, ProjectState::Running).await;
    // « running » est écrit avant que le pipeline ne parte : attendre qu'il
    // soit parti (sinon l'arrêt le devance, et rien n'est lancé).
    for _ in 0..500 {
        if !h.runner.runs.lock().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    h.workshop.shutdown(Duration::from_secs(5)).await;
    assert_eq!(h.state(first).await, ProjectState::Paused);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.state(second).await, ProjectState::Queued);
    assert!(h.workshop.active().is_none());
    // Plus rien ne se lance après la fermeture.
    let third = h.create("Third").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.state(third).await, ProjectState::Queued);
    assert_eq!(h.runner.runs.lock().len(), 1);
}

#[tokio::test]
async fn stopping_the_model_work_pauses_the_project_without_launching_the_next() {
    let gates = Gates::default();
    let h = harness(gates.script());
    let first = h.create("First").await;
    let second = h.create("Second").await;
    h.wait_state(first, ProjectState::Running).await;
    // `stop_model_work` (fermeture, ou effacement avant l'oubli) : sans
    // fermer l'Atelier.
    h.workshop.cancel_all();
    h.wait_state(first, ProjectState::Paused).await;
    h.wait_idle().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.state(second).await, ProjectState::Queued, "not launched");
    assert_eq!(h.runner.runs.lock().len(), 1);
}

#[tokio::test]
async fn erasing_stops_the_project_without_writing_and_keeps_the_workshop_open() {
    let gates = Gates::default();
    let h = harness(gates.script());
    let first = h.create("First").await;
    let second = h.create("Second").await;
    h.wait_state(first, ProjectState::Running).await;
    // Comme « Erase my data » : travail arrêté, puis l'Atelier oublié.
    h.workshop.cancel_all();
    h.workshop.forget(Duration::from_secs(5)).await;
    assert!(h.workshop.active().is_none());
    assert_eq!(
        h.state(first).await,
        ProjectState::Running,
        "nothing written"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    // Sorti de la file (rien ne se lance avant l'effacement ; base gardée :
    // repris d'un clic).
    assert_eq!(h.state(second).await, ProjectState::Paused, "not launched");
    assert_eq!(h.runner.runs.lock().len(), 1);
    h.store
        .call(crate::store::erase::erase_user_data)
        .await
        .unwrap();
    // Toujours ouvert aux nouveaux projets.
    let next = h.create("After").await;
    h.wait_state(next, ProjectState::Running).await;
    gates.open(next);
    h.wait_state(next, ProjectState::Done).await;
}

#[tokio::test]
async fn deleting_a_running_project_stops_it_and_removes_its_folder() {
    let gates = Gates::default();
    let h = harness(gates.script());
    let mut updates = h.workshop.subscribe();
    let id = h.create("Guide").await;
    h.wait_state(id, ProjectState::Running).await;
    let folder = PathBuf::from(h.workshop.get(id).await.unwrap().project.folder);
    assert!(folder.is_dir());
    h.workshop.delete(id, true).await.unwrap();
    assert!(!folder.exists());
    assert!(matches!(
        h.workshop.get(id).await,
        Err(AppError::NotFound(_))
    ));
    assert!(h.workshop.active().is_none());
    let mut deleted = false;
    while let Ok(update) = updates.try_recv() {
        deleted |= update == WorkshopUpdate::Deleted { project: id };
    }
    assert!(deleted);
}

#[tokio::test]
async fn a_folder_without_its_workspace_is_never_deleted() {
    let h = harness(quick());
    let id = h.create("Guide").await;
    h.wait_state(id, ProjectState::Done).await;
    let folder = PathBuf::from(h.workshop.get(id).await.unwrap().project.folder);
    std::fs::remove_dir_all(folder.join(".atelier")).unwrap();
    h.workshop.delete(id, true).await.unwrap();
    assert!(folder.is_dir(), "not made by the Workshop any more: kept");
    // Sans `delete_files`, le dossier reste toujours.
    let kept = h.create("Kept").await;
    h.wait_state(kept, ProjectState::Done).await;
    let folder = PathBuf::from(h.workshop.get(kept).await.unwrap().project.folder);
    h.workshop.delete(kept, false).await.unwrap();
    assert!(folder.join(".atelier").is_dir());
}

#[tokio::test]
async fn invalid_projects_are_refused_before_anything_is_written() {
    let h = harness(quick());
    for task in ["", "   ", &"x".repeat(super::MAX_TASK_CHARS + 1)] {
        let refused = h.workshop.create(&h.staging, request(task)).await;
        assert!(matches!(refused, Err(AppError::Invalid(_))));
    }
    let mut later = request("Code something");
    later.strategy = Some(Strategy::Code);
    assert!(matches!(
        h.workshop.create(&h.staging, later).await,
        Err(AppError::Invalid(_))
    ));
    let mut unknown = request("Guide");
    unknown.attachment_ids = vec!["nope".into()];
    assert!(h.workshop.create(&h.staging, unknown).await.is_err());
    // Image : refusée, et rendue à la zone.
    let StageResult::Staged { file } = h
        .staging
        .stage_bytes("photo.png".into(), vec![0x89, b'P', b'N', b'G'])
        .await
    else {
        panic!("not staged");
    };
    let mut image = request("Guide");
    image.attachment_ids = vec![file.id.clone()];
    assert!(matches!(
        h.workshop.create(&h.staging, image).await,
        Err(AppError::Invalid(_))
    ));
    assert!(h.staging.get(&file.id).is_some());
    assert!(!h.root.path().join("ChatonLM").exists());
    assert!(h.workshop.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_forced_strategy_and_a_folder_name_are_kept() {
    let h = harness(quick());
    let mut new = request("Write a long guide");
    new.strategy = Some(Strategy::Generic);
    new.folder_name = Some("Mon guide : v1".into());
    let first = h.workshop.create(&h.staging, new.clone()).await.unwrap();
    let second = h.workshop.create(&h.staging, new).await.unwrap();
    h.wait_state(second, ProjectState::Done).await;
    let first = h.workshop.get(first).await.unwrap().project;
    let second = h.workshop.get(second).await.unwrap().project;
    assert!(first.strategy_forced);
    assert_eq!(first.strategy, Some(Strategy::Generic));
    assert!(first.folder.ends_with("Mon guide - v1"));
    assert!(second.folder.ends_with("Mon guide - v1 (2)"));
}

#[tokio::test]
async fn the_name_found_at_framing_renames_only_a_provisional_folder() {
    let h = harness(Arc::new(|host| {
        Box::pin(async move {
            host.name_project("  Guide des tomates ").await;
            // Les écritures suivantes vont dans le dossier renommé.
            host.write_file("guide.md", b"# Tomates").await?;
            // Une reprise redonne le nom : rien ne bouge.
            host.name_project("Guide des tomates").await;
            Ok(())
        })
    }));
    let provisional = h
        .create("Écris-moi un guide complet pour faire pousser des tomates")
        .await;
    let mut named = request("Écris-moi un guide complet pour faire pousser des tomates");
    named.folder_name = Some("Mes tomates".into());
    let named = h.workshop.create(&h.staging, named).await.unwrap();
    h.wait_state(named, ProjectState::Done).await;
    h.wait_state(provisional, ProjectState::Done).await;
    let root = h.root.path().join("ChatonLM");

    let project = h.workshop.get(provisional).await.unwrap().project;
    assert_eq!(project.title, "Guide des tomates");
    assert_eq!(
        PathBuf::from(&project.folder),
        root.join("Guide des tomates")
    );
    assert!(!root.join("Project").exists());
    assert_eq!(
        std::fs::read(root.join("Guide des tomates/guide.md")).unwrap(),
        b"# Tomates"
    );
    // Nom donné par l'utilisateur : gardé ; le titre suit le cadrage.
    let project = h.workshop.get(named).await.unwrap().project;
    assert_eq!(project.title, "Guide des tomates");
    assert_eq!(PathBuf::from(&project.folder), root.join("Mes tomates"));
    assert!(root.join("Mes tomates/guide.md").is_file());
    // Plus jamais renommé ensuite (nom définitif).
    let row = h
        .store
        .call(move |c| db::get(c, provisional))
        .await
        .unwrap()
        .unwrap();
    assert!(row.folder_named);
}

#[tokio::test]
async fn the_projects_folder_must_come_from_the_picker() {
    let h = harness(quick());
    let default = h.workshop.settings().await.unwrap();
    assert_eq!(PathBuf::from(&default.dir), h.root.path().join("ChatonLM"));
    assert!(!default.custom);
    assert!(default.web);
    assert_eq!(default.autonomy, Autonomy::Ask);
    let chosen = h.root.path().join("Projets").to_string_lossy().into_owned();
    let patch = |dir: &str| WorkshopSettingsPatch {
        dir: Some(dir.into()),
        ..WorkshopSettingsPatch::default()
    };
    assert!(matches!(
        h.workshop.set_settings(patch(&chosen)).await,
        Err(AppError::Invalid(_))
    ));
    h.workshop.grant_dir(&chosen);
    let settings = h.workshop.set_settings(patch(&chosen)).await.unwrap();
    assert_eq!(settings.dir, chosen);
    assert!(settings.custom);
    let id = h.create("Guide").await;
    let folder = h.workshop.get(id).await.unwrap().project.folder;
    assert!(folder.starts_with(&chosen), "{folder}");
    // Vide : retour au dossier par défaut ; autres réglages un par un.
    let settings = h
        .workshop
        .set_settings(WorkshopSettingsPatch {
            dir: Some(String::new()),
            web: Some(false),
            autonomy: Some(Autonomy::Autonomous),
        })
        .await
        .unwrap();
    assert!(!settings.custom);
    assert!(!settings.web);
    assert_eq!(settings.autonomy, Autonomy::Autonomous);
}

#[tokio::test]
async fn the_model_check_tells_the_model_and_its_warnings() {
    let h = harness(quick());
    let check = h.workshop.model_check(None).await;
    assert_eq!(check.model.as_deref(), Some("big/m:Q4_K_M"));
    assert!(check.auto);
    assert_eq!(check.warnings, [ModelWarning::Small]);
    *h.runner.choice.lock() = Err("modelNotFound");
    let check = h.workshop.model_check(Some("gone/m:Q4_0".into())).await;
    assert_eq!(check.model, None);
    assert!(!check.auto);
    assert_eq!(check.error.as_deref(), Some("modelNotFound"));
}

#[tokio::test]
async fn notices_to_the_page_are_bounded_and_keep_the_latest() {
    let h = harness_with(
        Store::open_in_memory().unwrap(),
        Arc::new(|host| {
            Box::pin(async move {
                for _ in 0..40 {
                    host.emit(EventKind::Warning {
                        code: "registryCondensed".into(),
                    })
                    .await;
                }
                Ok(())
            })
        }),
        Duration::from_millis(300),
    );
    let id = h.create("Guide").await;
    h.wait_state(id, ProjectState::Done).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    let notices = h.notices.lock().clone();
    assert!(notices.len() <= 6, "{} notices", notices.len());
    let last = notices.last().unwrap();
    assert_eq!(last.project_id, id);
    assert_eq!(last.state, Some(ProjectState::Done));
    let last_seq = notices.iter().filter_map(|n| n.last_seq).max();
    // Avertissements du modèle, 40 avertissements, fin écrite par le socle.
    assert_eq!(last_seq, Some(42), "every event counted");
}

/// Relecture du lot 2 (point 12) : deux « reprendre » simultanés ne remettent
/// le projet en file qu'une fois (vérification et écriture en une requête).
#[tokio::test]
async fn a_double_resume_queues_the_project_once() {
    let gates = Gates::default();
    let h = harness(gates.script());
    let first = h.create("First").await;
    let second = h.create("Second").await;
    h.wait_state(first, ProjectState::Running).await;
    h.workshop.pause(second).await.unwrap();
    let mut updates = h.workshop.subscribe();
    let (a, b) = tokio::join!(h.workshop.resume(second), h.workshop.resume(second));
    a.unwrap();
    b.unwrap();
    let mut queued = 0;
    while let Ok(update) = updates.try_recv() {
        queued += usize::from(
            update
                == WorkshopUpdate::State {
                    project: second,
                    state: ProjectState::Queued,
                },
        );
    }
    assert_eq!(queued, 1);
    // Une reprise de plus, une fois en file : sans effet non plus.
    let seq = h
        .store
        .call(move |c| Ok(db::get(c, second)?.unwrap().queue_seq))
        .await
        .unwrap();
    h.workshop.resume(second).await.unwrap();
    let again = h
        .store
        .call(move |c| Ok(db::get(c, second)?.unwrap().queue_seq))
        .await
        .unwrap();
    assert_eq!(seq, again);
    gates.open(first);
    h.wait_state(second, ProjectState::Running).await;
    gates.open(second);
    h.wait_state(second, ProjectState::Done).await;
    assert_eq!(h.runner.runs.lock().len(), 2);
}

/// Relecture du lot 2 (point 12) : une pause ou une annulation d'un projet
/// qui calcule hors de la place active (vue périmée) n'écrit rien.
#[tokio::test]
async fn a_pause_never_marks_a_computing_project_paused() {
    let gates = Gates::default();
    let h = harness(gates.script());
    let first = h.create("First").await;
    let second = h.create("Second").await;
    h.wait_state(first, ProjectState::Running).await;
    // Le lancement a écrit « running » (le projet n'a pas encore sa place).
    h.store
        .call(move |c| db::set_state(c, second, "running", None, 5))
        .await
        .unwrap();
    assert!(h.workshop.pause(second).await.is_err());
    assert!(h.workshop.cancel(second).await.is_err());
    assert_eq!(h.state(second).await, ProjectState::Running);
    gates.open(first);
    h.wait_idle().await;
}

/// Relecture du lot 2 (point 13) : une question sans réponse rend la place
/// (le suivant de la file se lance, le modèle est relâché) ; le projet reste
/// en attente et reprend par la réponse, sans la reposer.
#[tokio::test]
async fn a_question_left_waiting_gives_up_its_place() {
    let gates = Gates::default();
    let answers = Arc::new(Mutex::new(Vec::new()));
    let ask = asking(answers.clone());
    let gated = gates.script();
    let script: Script = Arc::new(move |host| {
        if host.project().title == "Ask" {
            ask(host)
        } else {
            gated(host)
        }
    });
    let park = Duration::from_millis(300);
    let h = harness_parking(
        Store::open_in_memory().unwrap(),
        script,
        Duration::from_millis(5),
        park,
    );
    let asker = h.create("Ask").await;
    h.wait_state(asker, ProjectState::Waiting).await;
    let other = h.create("Other").await;
    // Avant le délai : le projet en attente garde sa place.
    tokio::time::sleep(park / 3).await;
    assert_eq!(h.workshop.active(), Some(asker));
    assert_eq!(h.state(other).await, ProjectState::Queued);
    // Après : place rendue, toujours en attente, question toujours posée.
    h.wait_state(other, ProjectState::Running).await;
    assert_eq!(h.workshop.active(), Some(other));
    assert_eq!(h.state(asker).await, ProjectState::Waiting);
    let question = h.workshop.get(asker).await.unwrap().question.unwrap();
    let reply = Answer::Framing(vec!["Gardeners".into(), "Short".into()]);
    h.workshop
        .answer(
            asker,
            AnswerRequest {
                question_id: question.id,
                answer: reply.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(h.state(asker).await, ProjectState::Queued);
    gates.open(other);
    h.wait_state(asker, ProjectState::Done).await;
    // Réponse reprise sans reposer la question.
    assert_eq!(*answers.lock(), [Answer::Cancelled, reply]);
    let order: Vec<i64> = h.runner.runs.lock().iter().map(|(id, _)| *id).collect();
    assert_eq!(order, [asker, other, asker]);
    let alerts = h
        .alerts
        .lock()
        .iter()
        .filter(|a| matches!(a, Alert::Waiting { .. }))
        .count();
    assert_eq!(alerts, 1, "asked once");
}

/// Relecture du lot 2 (point 14) : après « Erase my data », un projet créé
/// se lance même si l'ancien projet actif s'est arrêté après l'attente.
#[tokio::test]
async fn after_erasing_a_new_project_runs_even_if_the_old_one_stopped_late() {
    let h = harness(Arc::new(|host| {
        Box::pin(async move {
            if host.project().title == "After" {
                return Ok(());
            }
            host.cancelled().cancelled().await;
            // Arrêt lent (requête du modèle qui finit).
            tokio::time::sleep(Duration::from_millis(300)).await;
            Err("cancelled".into())
        })
    }));
    let first = h.create("First").await;
    h.wait_state(first, ProjectState::Running).await;
    h.workshop.cancel_all();
    h.workshop.forget(Duration::from_millis(10)).await;
    assert_eq!(h.workshop.active(), Some(first), "still stopping");
    h.store
        .call(crate::store::erase::erase_user_data)
        .await
        .unwrap();
    let next = h.create("After").await;
    h.wait_state(next, ProjectState::Done).await;
}
