//! Pipeline scénarisé de l'Atelier, pour les essais de l'app réelle
//! (fonctionnalité `e2e` seulement) : `CHATONLM_E2E_WORKSHOP=<script JSON>`
//! remplace le choix du modèle et le pipeline par un déroulé écrit, mené
//! contre l'hôte réel (événements en base et vers la page, état sauvé relu
//! par `workshop_parts`, questions qui attendent la réponse de la page,
//! livrable écrit avec les garde-fous du socle). Aucun modèle n'est ouvert.
//!
//! Script (relu à chaque choix et à chaque lancement) :
//!
//! ```json
//! { "model": "chatonlm-atelier", "warnings": ["small"], "chooseError": null,
//!   "steps": [
//!     {"emit": {"type": "phaseStarted", "phase": "collect"}},
//!     {"save": {"key": "plan", "value": {"title": "…", "bricks": []}}},
//!     {"ask": {"type": "approvePlan", "value": {"title": "…", "bricks": []}}},
//!     {"write": {"path": "guide.md", "text": "# Guide"}},
//!     {"wait": 500},
//!     "yield",
//!     {"fail": "planUnusable"}
//!   ] }
//! ```
//!
//! - `emit` : un `EventKind` tel que sérialisé (`WorkshopEventKind`).
//! - `save` : état sous son nom de clé (`StateKey::name` : `plan`, `brick:2`,
//!   `text:2:0`, `split:2`, `spec`, `report`…), dans la forme du pipeline.
//! - `ask` : `QuestionAsked`, attente de la réponse, `AnswerReceived` (comme
//!   le pipeline) ; plan approuvé : plan (modifié ou non) sauvé et
//!   `PlanReady` ; refusé sans note : projet arrêté (`Rejected`) ; annulé :
//!   arrêt.
//! - `write` : fichier sous le dossier du projet (refus : `deliverWriteFailed`).
//! - `wait` : attente en millisecondes, interrompue par la pause ou
//!   l'annulation ; `yield` : cession à l'utilisateur.
//! - `fail` : échec de ce code ; la fin du script vaut livraison.
//!
//! Reprise après une pause : le déroulé reprend à l'étape interrompue (rang
//! gardé en mémoire, par projet). Code commun à Linux et Windows.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;

use super::enc;
use super::model::{ChoiceError, ModelChoice, PROJECT_CONTEXT};
use super::pipeline::host::Host;
use super::pipeline::types::{Answer, EventKind, Question, StateKey};
use super::project::ProjectHost;
use super::service::{Outcome, Runner};
use crate::engine::auto::ModelWarning;

/// Variable qui désigne le script.
#[cfg_attr(not(feature = "e2e"), allow(dead_code))]
pub(crate) const SCRIPT_ENV: &str = "CHATONLM_E2E_WORKSHOP";

/// Modèle annoncé quand le script n'en nomme pas.
const DEFAULT_MODEL: &str = "chatonlm-atelier";

/// Déroulé d'un projet.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Script {
    #[serde(default)]
    pub model: Option<String>,
    /// Avertissements du modèle (codes de `ModelWarning`).
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Aucun modèle : `noModel`, `modelNotFound`, `remoteOff`,
    /// `contextTooSmall`.
    #[serde(default)]
    pub choose_error: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

/// Une étape du déroulé.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum Step {
    Emit(EventKind),
    Save { key: String, value: Value },
    Ask(Question),
    Write { path: String, text: String },
    Wait(u64),
    Yield,
    Fail(String),
}

impl Script {
    /// Script lu et vérifié (clés d'état connues) ; erreur : message.
    pub(crate) fn parse(json: &str) -> Result<Script, String> {
        let script: Script = serde_json::from_str(json).map_err(|e| e.to_string())?;
        for step in &script.steps {
            if let Step::Save { key, .. } = step
                && key_of(key).is_none()
            {
                return Err(format!("unknown state key: {key}"));
            }
        }
        for code in &script.warnings {
            if warning_of(code).is_none() {
                return Err(format!("unknown model warning: {code}"));
            }
        }
        if let Some(code) = &script.choose_error
            && choice_error(code).is_none()
        {
            return Err(format!("unknown choice error: {code}"));
        }
        Ok(script)
    }

    /// Modèle retenu pour `requested` (vide : celui du script).
    fn choose(&self, requested: &str) -> Result<ModelChoice, ChoiceError> {
        if let Some(code) = self.choose_error.as_deref().and_then(choice_error) {
            return Err(code);
        }
        let auto = requested.trim().is_empty();
        Ok(ModelChoice {
            id: if auto {
                self.model.clone().unwrap_or_else(|| DEFAULT_MODEL.into())
            } else {
                requested.trim().to_owned()
            },
            ctx: PROJECT_CONTEXT,
            warnings: self.warnings.iter().filter_map(|w| warning_of(w)).collect(),
            auto,
        })
    }
}

/// Clé d'état d'après son nom ([`StateKey::name`]).
pub(crate) fn key_of(name: &str) -> Option<StateKey> {
    let mut parts = name.split(':');
    let head = parts.next()?;
    let numbers: Vec<u32> = parts.map(str::parse).collect::<Result<_, _>>().ok()?;
    let small = |n: u32| u8::try_from(n).ok();
    let key = match (head, numbers.as_slice()) {
        ("strategyDraw", [n]) => StateKey::StrategyDraw(small(*n)?),
        ("strategy", []) => StateKey::Strategy,
        ("framing", []) => StateKey::Framing,
        ("specAttempt", [n]) => StateKey::SpecAttempt(small(*n)?),
        ("spec", []) => StateKey::Spec,
        ("planAttempt", [r, a]) => StateKey::PlanAttempt {
            round: small(*r)?,
            attempt: small(*a)?,
        },
        ("planJudge", [r, a]) => StateKey::PlanJudge {
            round: small(*r)?,
            attempt: small(*a)?,
        },
        ("planRound", []) => StateKey::PlanRound,
        ("plan", []) => StateKey::Plan,
        ("text", [id, v]) => StateKey::Text {
            id: *id,
            version: small(*v)?,
        },
        ("formJudge", [id, v]) => StateKey::FormJudge {
            id: *id,
            version: small(*v)?,
        },
        ("factJudge", [id, v]) => StateKey::FactJudge {
            id: *id,
            version: small(*v)?,
        },
        ("refusalJudge", [id, v]) => StateKey::RefusalJudge {
            id: *id,
            version: small(*v)?,
        },
        ("brick", [id]) => StateKey::Brick(*id),
        ("card", [id]) => StateKey::Card(*id),
        ("split", [id]) => StateKey::Split(*id),
        ("surgery", [id]) => StateKey::Surgery(*id),
        ("proofread", [n]) => StateKey::Proofread(*n),
        ("contradictions", []) => StateKey::Contradictions,
        ("wholeForm", []) => StateKey::WholeForm,
        ("report", []) => StateKey::Report,
        ("delivered", []) => StateKey::Delivered,
        ("notes", []) => StateKey::Notes,
        _ => return None,
    };
    Some(key)
}

/// Avertissement du modèle d'après son code (`ModelWarning` ne se relit
/// pas : comparé à sa forme sérialisée).
fn warning_of(code: &str) -> Option<ModelWarning> {
    [
        ModelWarning::TooSmall,
        ModelWarning::Small,
        ModelWarning::SmallContext,
        ModelWarning::NoThinking,
        ModelWarning::UnknownSize,
    ]
    .into_iter()
    .find(|w| enc(w) == code)
}

fn choice_error(code: &str) -> Option<ChoiceError> {
    ["noModel", "modelNotFound", "remoteOff", "contextTooSmall"]
        .into_iter()
        .find(|known| *known == code)
}

/// Fin d'une étape.
enum Next {
    Continue,
    End(Outcome),
}

/// Mène une étape contre l'hôte.
async fn play(host: &ProjectHost, step: &Step) -> Next {
    let cancel = host.cancelled();
    match step {
        Step::Emit(kind) => {
            host.emit(kind.clone()).await;
        }
        Step::Save { key, value } => {
            if let Some(key) = key_of(key) {
                host.save(key, value.clone()).await;
            }
        }
        Step::Ask(question) => {
            host.emit(EventKind::QuestionAsked {
                question: question.clone(),
            })
            .await;
            let answer = host.ask(question.clone()).await;
            if answer == Answer::Cancelled {
                return Next::End(Outcome::Cancelled);
            }
            host.emit(EventKind::AnswerReceived {
                answer: answer.clone(),
            })
            .await;
            if let (
                Question::ApprovePlan(asked),
                Answer::Plan {
                    approved,
                    edited,
                    note,
                },
            ) = (question, &answer)
            {
                if *approved {
                    let plan = edited.clone().unwrap_or_else(|| asked.clone());
                    if let Ok(value) = serde_json::to_value(&plan) {
                        host.save(StateKey::Plan, value).await;
                    }
                    host.emit(EventKind::PlanReady { plan }).await;
                } else if note.as_deref().is_none_or(|n| n.trim().is_empty()) {
                    return Next::End(Outcome::Rejected);
                }
            }
        }
        Step::Write { path, text } => {
            if host.write_file(path, text.as_bytes()).await.is_err() {
                return Next::End(Outcome::Failed("deliverWriteFailed".into()));
            }
        }
        Step::Wait(ms) => {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return Next::End(Outcome::Cancelled),
                () = tokio::time::sleep(Duration::from_millis(*ms)) => {}
            }
        }
        Step::Yield => {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return Next::End(Outcome::Cancelled),
                () = host.yield_to_user() => {}
            }
        }
        Step::Fail(code) => return Next::End(Outcome::Failed(code.clone())),
    }
    if cancel.is_cancelled() {
        Next::End(Outcome::Cancelled)
    } else {
        Next::Continue
    }
}

/// Le « pipeline » des essais : script relu à chaque appel.
pub(crate) struct ScriptRunner {
    path: PathBuf,
    /// Par projet : rang de la prochaine étape (reprise après une pause).
    next: Arc<Mutex<HashMap<i64, usize>>>,
}

impl ScriptRunner {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            next: Arc::default(),
        }
    }

    fn script(&self) -> Result<Script, String> {
        let json = std::fs::read_to_string(&self.path).map_err(|e| e.to_string())?;
        Script::parse(&json)
    }
}

impl Runner for ScriptRunner {
    fn choose(&self, requested: String) -> BoxFuture<'static, Result<ModelChoice, ChoiceError>> {
        let chosen = match self.script() {
            Ok(script) => script.choose(&requested),
            Err(error) => {
                // i18n-ignore : journal.
                crate::app_log!("chatonlm: workshop: e2e script unreadable: {error}");
                Err("noModel")
            }
        };
        Box::pin(async move { chosen })
    }

    fn run(&self, host: Arc<ProjectHost>, _model: ModelChoice) -> BoxFuture<'static, Outcome> {
        let script = self.script();
        let next = self.next.clone();
        Box::pin(async move {
            let script = match script {
                Ok(script) => script,
                Err(error) => {
                    // i18n-ignore : journal.
                    crate::app_log!("chatonlm: workshop: e2e script unreadable: {error}");
                    return Outcome::Failed("e2eScript".into());
                }
            };
            let id = host.project().id;
            loop {
                let rank = next.lock().get(&id).copied().unwrap_or(0);
                let Some(step) = script.steps.get(rank) else {
                    return Outcome::Delivered;
                };
                match play(&host, step).await {
                    Next::Continue => {
                        next.lock().insert(id, rank + 1);
                    }
                    Next::End(outcome) => {
                        // Échec ou refus : le script ne reprend pas.
                        if matches!(outcome, Outcome::Failed(_) | Outcome::Rejected) {
                            next.lock().insert(id, rank + 1);
                        }
                        return outcome;
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::Staging;
    use crate::error::AppError;
    use crate::resources::Interactive;
    use crate::store::Store;
    use crate::workshop::pipeline::types::{
        Autonomy, BrickKind, BrickPlan, BrickState, Phase, Plan, ProjectState,
    };
    use crate::workshop::service::{Deps, Workshop};
    use crate::workshop::{AnswerRequest, NewProjectRequest};
    use serde_json::json;

    #[test]
    fn every_state_key_name_reads_back() {
        let keys = [
            StateKey::StrategyDraw(2),
            StateKey::Strategy,
            StateKey::Framing,
            StateKey::SpecAttempt(3),
            StateKey::Spec,
            StateKey::PlanAttempt {
                round: 1,
                attempt: 2,
            },
            StateKey::PlanJudge {
                round: 0,
                attempt: 3,
            },
            StateKey::PlanRound,
            StateKey::Plan,
            StateKey::Text {
                id: 201,
                version: 1,
            },
            StateKey::FormJudge { id: 3, version: 0 },
            StateKey::FactJudge { id: 0, version: 2 },
            StateKey::RefusalJudge { id: 4, version: 0 },
            StateKey::Brick(7),
            StateKey::Card(7),
            StateKey::Split(2),
            StateKey::Surgery(0),
            StateKey::Proofread(5),
            StateKey::Contradictions,
            StateKey::WholeForm,
            StateKey::Report,
            StateKey::Delivered,
            StateKey::Notes,
        ];
        for key in keys {
            assert_eq!(key_of(&key.name()), Some(key), "{}", key.name());
        }
        for bad in [
            "",
            "plan:1",
            "brick",
            "brick:x",
            "text:1",
            "text:1:300",
            "other",
        ] {
            assert_eq!(key_of(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_script_is_read_and_checked() {
        let script = Script::parse(
            r##"{"model": "m", "warnings": ["small", "noThinking"], "steps": [
                {"emit": {"type": "phaseStarted", "phase": "collect"}},
                {"save": {"key": "brick:2", "value": {"a": 1}}},
                {"ask": {"type": "framing", "value": ["Pour qui ?"]}},
                {"write": {"path": "g.md", "text": "# G"}},
                {"wait": 20}, "yield", {"fail": "planUnusable"}
            ]}"##,
        )
        .unwrap();
        assert_eq!(script.steps.len(), 7);
        assert_eq!(
            script.steps[0],
            Step::Emit(EventKind::PhaseStarted {
                phase: Phase::Collect
            })
        );
        assert_eq!(
            script.steps[2],
            Step::Ask(Question::Framing(vec!["Pour qui ?".into()]))
        );
        assert_eq!(script.steps[5], Step::Yield);
        let choice = script.choose("").unwrap();
        assert_eq!(choice.id, "m");
        assert!(choice.auto);
        assert_eq!(
            choice.warnings,
            vec![ModelWarning::Small, ModelWarning::NoThinking]
        );
        let asked = script.choose(" autre ").unwrap();
        assert_eq!((asked.id.as_str(), asked.auto), ("autre", false));

        for bad in [
            r#"{"steps": [{"save": {"key": "nope", "value": 1}}]}"#,
            r#"{"warnings": ["huge"]}"#,
            r#"{"chooseError": "broken"}"#,
            r#"{"steps": [{"jump": 1}]}"#,
            r#"{"stepz": []}"#,
        ] {
            assert!(Script::parse(bad).is_err(), "{bad}");
        }
        let refused = Script::parse(r#"{"chooseError": "contextTooSmall"}"#).unwrap();
        assert_eq!(refused.choose(""), Err("contextTooSmall"));
    }

    fn plan(title: &str) -> Plan {
        Plan {
            title: "Guide".into(),
            bricks: vec![BrickPlan {
                id: 1,
                title: title.into(),
                goal: "but".into(),
                kind: BrickKind::Text,
                words: 300,
                depends_on: vec![],
                criteria: vec![],
                forbidden: vec![],
            }],
        }
    }

    struct Bench {
        workshop: Workshop,
        staging: Staging,
        _dir: tempfile::TempDir,
    }

    fn bench(script: &Value) -> Bench {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.json");
        std::fs::write(&path, script.to_string()).unwrap();
        let workshop = Workshop::new(
            Store::open_in_memory().unwrap(),
            Deps {
                runner: Arc::new(ScriptRunner::new(path)),
                alert: Arc::new(|_| {}),
                notice: Arc::new(|_| {}),
                interactive: Interactive::new(),
                grace: Duration::from_millis(20),
                notice_interval: Duration::from_millis(5),
                default_root: Some(dir.path().join("projets")),
                park_after: Duration::from_secs(60),
            },
        );
        Bench {
            workshop,
            staging: Staging::default(),
            _dir: dir,
        }
    }

    impl Bench {
        async fn create(&self) -> i64 {
            self.workshop
                .create(
                    &self.staging,
                    NewProjectRequest {
                        task: "Écris un guide".into(),
                        attachment_ids: vec![],
                        strategy: None,
                        autonomy: Autonomy::Ask,
                        model: None,
                        folder_name: None,
                    },
                )
                .await
                .unwrap()
        }

        async fn wait(&self, id: i64, state: ProjectState) {
            for _ in 0..500 {
                if self.workshop.get(id).await.unwrap().project.state == state {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("project {id} never {state:?}");
        }
    }

    #[tokio::test]
    async fn a_script_runs_asks_and_delivers_against_the_real_host() {
        let bench = bench(&json!({"warnings": ["small"], "steps": [
            {"emit": {"type": "phaseStarted", "phase": "plan"}},
            {"ask": {"type": "approvePlan", "value": plan("Avant")}},
            {"save": {"key": "brick:1", "value": {"id": 1, "title": "Après", "kind": "text",
                "state": "done", "text": "## Après\n\nun deux", "fixed": false, "checks": []}}},
            {"write": {"path": "guide.md", "text": "# Guide"}},
        ]}));
        let id = bench.create().await;
        bench.wait(id, ProjectState::Waiting).await;
        let question = bench.workshop.get(id).await.unwrap().question.unwrap();
        assert_eq!(question.question, Question::ApprovePlan(plan("Avant")));
        bench
            .workshop
            .answer(
                id,
                AnswerRequest {
                    question_id: question.id,
                    answer: Answer::Plan {
                        approved: true,
                        edited: Some(plan("Après")),
                        note: None,
                    },
                },
            )
            .await
            .unwrap();
        bench.wait(id, ProjectState::Done).await;
        let detail = bench.workshop.get(id).await.unwrap();
        assert_eq!(detail.project.model_used, DEFAULT_MODEL);
        let folder = PathBuf::from(&detail.project.folder);
        assert_eq!(std::fs::read(folder.join("guide.md")).unwrap(), b"# Guide");
        // Le plan modifié est sauvé : les parties le relisent.
        let parts = bench.workshop.parts(id).await.unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(
            (parts[0].title.as_str(), parts[0].state, parts[0].words),
            ("Après", BrickState::Done, 2)
        );
        let kinds: Vec<EventKind> = bench
            .workshop
            .events(id, 0)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(
            kinds[0],
            EventKind::ModelWarnings {
                model: DEFAULT_MODEL.into(),
                warnings: vec!["small".into()],
            }
        );
        assert!(kinds.contains(&EventKind::QuestionAsked {
            question: Question::ApprovePlan(plan("Avant"))
        }));
        assert!(kinds.contains(&EventKind::PlanReady {
            plan: plan("Après")
        }));
        assert_eq!(
            kinds.last(),
            Some(&EventKind::Finished {
                state: ProjectState::Done
            })
        );
    }

    #[tokio::test]
    async fn a_delivered_direct_version_shows_the_deliverable_and_its_spec() {
        let report = json!({"strategy": "generic", "route": "direct", "file": "guide.md",
            "words": 3, "targetWords": null, "checks": [], "judgedOnOutline": false,
            "bricks": [], "notChecked": []});
        let spec = json!({"goal": "g", "deliverables": [], "criteria": [{"id": "C1",
            "text": "Trois mots", "scope": "document"}], "audience": "a", "language": "fr",
            "register": "informal", "units": "SI", "lengthWords": null, "glossary": [],
            "assumptions": [], "constraints": []});
        let bench = bench(&json!({"steps": [
            {"wait": 300},
            {"save": {"key": "spec", "value": spec}},
            {"emit": {"type": "activity", "activity": {"code": "writingDirect"}}},
            {"save": {"key": "text:0:0", "value": "# Brouillon"}},
            {"write": {"path": "guide.md", "text": "# Guide\n\nLivré tel quel"}},
            {"save": {"key": "report", "value": report}},
        ]}));
        let id = bench.create().await;
        // Avant la livraison : ni spécification ni livrable.
        assert_eq!(bench.workshop.spec(id).await.unwrap(), None);
        assert!(matches!(
            bench.workshop.delivered(id).await,
            Err(AppError::NotFound(_))
        ));
        bench.wait(id, ProjectState::Done).await;
        let spec = bench.workshop.spec(id).await.unwrap().unwrap();
        assert_eq!(spec.criteria[0].text, "Trois mots");
        let folder = PathBuf::from(bench.workshop.get(id).await.unwrap().project.folder);
        assert_eq!(
            bench.workshop.delivered(id).await.unwrap(),
            folder.join("guide.md")
        );
        // La version directe livrée montre le livrable, pas le brouillon.
        let part = bench.workshop.part(id, 0).await.unwrap();
        assert_eq!(part.text, "# Guide\n\nLivré tel quel");
        assert_eq!(part.state, BrickState::Done);
        assert!(matches!(
            bench.workshop.part(id, 4).await,
            Err(AppError::NotFound(_))
        ));
        assert!(matches!(
            bench.workshop.parts(id + 99).await,
            Err(AppError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn a_paused_script_resumes_at_the_interrupted_step() {
        let bench = bench(&json!({"steps": [
            {"emit": {"type": "warning", "code": "first"}},
            {"wait": 60_000},
            {"emit": {"type": "warning", "code": "second"}},
        ]}));
        let id = bench.create().await;
        bench.wait(id, ProjectState::Running).await;
        // Premier événement émis, puis l'attente.
        for _ in 0..500 {
            if bench.workshop.events(id, 0).await.unwrap().len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        bench.workshop.pause(id).await.unwrap();
        bench.wait(id, ProjectState::Paused).await;
        // Script changé à chaud : l'attente reprise est courte.
        let path = bench._dir.path().join("script.json");
        std::fs::write(
            &path,
            json!({"steps": [
                {"emit": {"type": "warning", "code": "first"}},
                {"wait": 10},
                {"emit": {"type": "warning", "code": "second"}},
            ]})
            .to_string(),
        )
        .unwrap();
        bench.workshop.resume(id).await.unwrap();
        bench.wait(id, ProjectState::Done).await;
        let warnings: Vec<String> = bench
            .workshop
            .events(id, 0)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|e| match e.kind {
                EventKind::Warning { code } => Some(code),
                _ => None,
            })
            .collect();
        assert_eq!(warnings, ["first", "second"]);
    }

    #[tokio::test]
    async fn a_plan_refused_without_a_note_cancels_and_a_failure_fails() {
        let bench = bench(&json!({"steps": [
            {"ask": {"type": "approvePlan", "value": plan("A")}},
            {"fail": "never"},
        ]}));
        let id = bench.create().await;
        bench.wait(id, ProjectState::Waiting).await;
        let question = bench.workshop.get(id).await.unwrap().question.unwrap();
        let refuse = |note: Option<&str>| AnswerRequest {
            question_id: question.id,
            answer: Answer::Plan {
                approved: false,
                edited: None,
                note: note.map(str::to_owned),
            },
        };
        bench.workshop.answer(id, refuse(None)).await.unwrap();
        bench.wait(id, ProjectState::Cancelled).await;

        // Refus avec une note : le déroulé continue (ici, vers l'échec).
        let id = bench.create().await;
        bench.wait(id, ProjectState::Waiting).await;
        let question = bench.workshop.get(id).await.unwrap().question.unwrap();
        let answer = AnswerRequest {
            question_id: question.id,
            answer: Answer::Plan {
                approved: false,
                edited: None,
                note: Some("plus court".into()),
            },
        };
        bench.workshop.answer(id, answer).await.unwrap();
        bench.wait(id, ProjectState::Failed).await;
        let detail = bench.workshop.get(id).await.unwrap();
        assert_eq!(detail.project.error.as_deref(), Some("never"));
    }
}
