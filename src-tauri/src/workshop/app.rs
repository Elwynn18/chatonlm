//! Branchement de l'Atelier sur l'app : modèle choisi et ouvert par le
//! moteur (`agent::Worker`, relâché à la fin du lancement : pause,
//! annulation, fermeture), pipeline mené contre l'hôte réel, notifications
//! du bureau (textes par `t!`), avis vers la page (`workshop-event`),
//! dossier des projets par défaut (`Documents/ChatonLM`, Linux et Windows,
//! résolu par Tauri).

use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use tauri::{AppHandle, Emitter, Manager};

use super::model::{self, ChoiceError, ModelChoice};
use super::pipeline::host::Host;
use super::project::ProjectHost;
use super::service::{self, Alert, Deps, Outcome, Runner, Workshop};
use super::{NOTICE_EVENT, WorkshopNotice};
use crate::agent::{Worker, WorkerOptions};
use crate::i18n::t;
use crate::llm::Access;
use crate::store::Store;

/// Nom du dossier des projets sous le dossier Documents.
const PROJECTS_DIR: &str = "ChatonLM";

/// Ce qu'un projet lancé fait dans l'app.
struct AppRunner {
    access: Access,
}

impl Runner for AppRunner {
    fn choose(&self, requested: String) -> BoxFuture<'static, Result<ModelChoice, ChoiceError>> {
        let access = self.access.clone();
        Box::pin(async move { model::choose(&access, &requested).await })
    }

    fn run(&self, host: Arc<ProjectHost>, choice: ModelChoice) -> BoxFuture<'static, Outcome> {
        let access = self.access.clone();
        Box::pin(async move {
            let opened = Worker::open(
                &access,
                &choice.id,
                WorkerOptions {
                    ctx_min: choice.ctx,
                    cancel: host.cancelled(),
                },
            )
            .await;
            match opened {
                Ok(worker) => {
                    let outcome = run_pipeline(&host, &worker).await;
                    // Session rendue : le modèle redevient évinçable.
                    worker.release();
                    outcome
                }
                Err(crate::agent::AgentError::Cancelled) => Outcome::Cancelled,
                Err(error) => Outcome::Failed(error.code().to_owned()),
            }
        })
    }
}

/// Déroulement du projet par le pipeline ; le modèle est relâché par
/// l'appelant à la fin.
async fn run_pipeline(host: &ProjectHost, worker: &Worker) -> Outcome {
    use super::pipeline::RunEnd;
    match super::pipeline::run(host, worker).await {
        RunEnd::Delivered { .. } => Outcome::Delivered,
        RunEnd::Cancelled => Outcome::Cancelled,
        RunEnd::Rejected => Outcome::Rejected,
        RunEnd::Failed { code } => Outcome::Failed(code),
    }
}

/// Notification du bureau d'une alerte de l'Atelier.
fn texts(alert: &Alert) -> (String, String) {
    match alert {
        Alert::Waiting { title } => (
            t!("workshop.notify.waitingTitle"),
            t!("workshop.notify.waitingBody", title = title),
        ),
        Alert::Done { title } => (
            t!("workshop.notify.doneTitle"),
            t!("workshop.notify.doneBody", title = title),
        ),
        Alert::Failed { title } => (
            t!("workshop.notify.failedTitle"),
            t!("workshop.notify.failedBody", title = title),
        ),
        Alert::Paused { count } => (
            t!("workshop.notify.pausedTitle"),
            t!("workshop.notify.pausedBody", count = count),
        ),
    }
}

/// Dossier des projets par défaut : `<Documents>/ChatonLM` (dossier
/// Documents du bureau sous Linux, dossier connu « Documents » sous
/// Windows ; à défaut, `Documents` du dossier personnel).
fn default_root(app: &AppHandle) -> Option<PathBuf> {
    let path = app.path();
    path.document_dir()
        .ok()
        .or_else(|| path.home_dir().ok().map(|home| home.join("Documents")))
        .map(|documents| documents.join(PROJECTS_DIR))
}

/// Ce que fait un projet lancé : le pipeline avec un vrai modèle ; essais de
/// l'app réelle avec `CHATONLM_E2E_WORKSHOP` : un déroulé écrit, sans modèle
/// ([`super::scripted`]).
fn runner(access: Access) -> Arc<dyn Runner> {
    #[cfg(feature = "e2e")]
    if let Some(path) = std::env::var_os(super::scripted::SCRIPT_ENV) {
        return Arc::new(super::scripted::ScriptRunner::new(path.into()));
    }
    Arc::new(AppRunner { access })
}

/// Pose l'Atelier une fois la base ouverte et l'accès aux modèles posé
/// (démarrage, ou base récupérée ; une seule fois) : projets interrompus mis
/// en pause, notification s'il y en a.
pub(crate) fn setup(app: &AppHandle) {
    if app.try_state::<Workshop>().is_some() {
        return;
    }
    let (Some(store), Some(access)) = (app.try_state::<Store>(), app.try_state::<Access>()) else {
        return;
    };
    let (store, access) = (store.inner().clone(), access.inner().clone());
    let alerting = app.clone();
    let noticing = app.clone();
    let workshop = Workshop::new(
        store,
        Deps {
            runner: runner(access),
            alert: Arc::new(move |alert| {
                let app = alerting.clone();
                service::spawn(async move {
                    let (title, body) = texts(&alert);
                    crate::desktop::notify::notify(&app, &title, &body).await;
                });
            }),
            notice: Arc::new(move |notice: WorkshopNotice| {
                let _ = noticing.emit(NOTICE_EVENT, notice);
            }),
            interactive: app.state::<crate::resources::Interactive>().inner().clone(),
            grace: service::GRACE,
            notice_interval: service::NOTICE_INTERVAL,
            default_root: default_root(app),
            park_after: service::PARK_AFTER,
        },
    );
    workshop.recover();
    app.manage(workshop);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_alert_has_a_title_and_a_body() {
        for alert in [
            Alert::Waiting {
                title: "Guide".into(),
            },
            Alert::Done {
                title: "Guide".into(),
            },
            Alert::Failed {
                title: "Guide".into(),
            },
            Alert::Paused { count: 2 },
        ] {
            let (title, body) = texts(&alert);
            assert!(!title.contains("workshop."), "{title}");
            assert!(!body.contains("workshop."), "{body}");
            assert!(!body.contains('{'), "{body}");
        }
    }
}
