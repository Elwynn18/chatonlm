//! Hôte réel d'un projet de l'Atelier (trait `pipeline::host::Host`) :
//! - événements numérotés en base, diffusés aux abonnés et à la page ; la
//!   phase commencée et la stratégie choisie mises aussi sur la ligne du
//!   projet (liste) ;
//! - état reprenable en base, par clé (`StateKey::name`) ;
//! - questions : enregistrées, projet `waiting`, notification, attente de la
//!   réponse ou de l'arrêt ; une réponse déjà donnée à la même question
//!   (projet relancé) est reprise sans reposer la question. Les événements
//!   `QuestionAsked` / `AnswerReceived` sont émis par le pipeline ;
//! - cession à l'utilisateur : `Interactive::idle(3 s)`, projet `yielding`
//!   (événements `Yielding` / `Resumed`) seulement s'il a fallu attendre ;
//! - écriture sous le dossier du projet avec les garde-fous de
//!   [`super::files`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::FutureExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::enc;
use super::files;
use super::pipeline::host::{Host, HostFuture};
use super::pipeline::types::{
    Answer, Event, EventKind, ProjectInfo, ProjectState, Question, StateKey,
};
use super::service::{Alert, Shared, WorkshopUpdate};
use crate::store::{now_ms, workshop as db};

/// Le socle, vu d'un projet lancé.
pub(crate) struct ProjectHost {
    shared: Arc<Shared>,
    info: ProjectInfo,
    cancel: CancellationToken,
    /// Le journal a reçu la fin du projet (`Finished`).
    finished: AtomicBool,
}

impl ProjectHost {
    pub(super) fn new(shared: Arc<Shared>, info: ProjectInfo, cancel: CancellationToken) -> Self {
        Self {
            shared,
            info,
            cancel,
            finished: AtomicBool::new(false),
        }
    }

    /// Le journal a reçu la fin du projet.
    pub(super) fn finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    fn id(&self) -> i64 {
        self.info.id
    }

    /// Enregistre l'événement, met à jour la ligne du projet, le diffuse ;
    /// rend son numéro (0 si la base l'a refusé : seulement journalisé).
    pub(super) async fn emit_kind(&self, kind: EventKind) -> u64 {
        let id = self.id();
        let at = now_ms();
        let Ok(json) = serde_json::to_string(&kind) else {
            return 0;
        };
        // Colonnes de la liste, dans la même écriture.
        let phase = match &kind {
            EventKind::PhaseStarted { phase } => Some(enc(phase)),
            _ => None,
        };
        let strategy = match &kind {
            EventKind::StrategyChosen { strategy, .. } => Some(enc(strategy)),
            _ => None,
        };
        if matches!(kind, EventKind::Finished { .. }) {
            self.finished.store(true, Ordering::SeqCst);
        }
        let saved = self
            .shared
            .store
            .call(move |c| {
                let seq = db::append_event(c, id, at, &json)?;
                if let Some(phase) = phase {
                    db::set_phase(c, id, &phase, at)?;
                }
                if let Some(strategy) = strategy {
                    db::set_strategy(c, id, &strategy, at)?;
                }
                Ok(seq)
            })
            .await;
        match saved {
            Ok(seq) => {
                self.shared.publish(WorkshopUpdate::Event {
                    project: id,
                    event: Event {
                        seq,
                        at_ms: at,
                        kind,
                    },
                });
                seq
            }
            Err(error) => {
                // i18n-ignore : journal.
                crate::app_log!("chatonlm: workshop: event of project {id} not saved: {error}");
                0
            }
        }
    }

    async fn save_value(&self, key: StateKey, value: Value) {
        let id = self.id();
        let name = key.name();
        let Ok(json) = serde_json::to_string(&value) else {
            return;
        };
        if let Err(error) = self
            .shared
            .store
            .call(move |c| db::save_state(c, id, &name, &json, now_ms()))
            .await
        {
            // i18n-ignore : journal.
            crate::app_log!("chatonlm: workshop: state of project {id} not saved: {error}");
        }
    }

    async fn load_value(&self, key: StateKey) -> Option<Value> {
        let id = self.id();
        let name = key.name();
        let json = self
            .shared
            .store
            .call(move |c| db::load_state(c, id, &name))
            .await
            .ok()
            .flatten()?;
        serde_json::from_str(&json).ok()
    }

    /// Réponse lue, question marquée lue, projet de nouveau en calcul.
    async fn answered(&self, question: i64, json: &str) -> Answer {
        let id = self.id();
        let answer: Answer = serde_json::from_str(json).unwrap_or(Answer::Cancelled);
        let _ = self
            .shared
            .store
            .call(move |c| db::use_answer(c, question))
            .await;
        self.shared.set_state(id, ProjectState::Running, None).await;
        answer
    }

    async fn ask_question(&self, question: Question) -> Answer {
        let id = self.id();
        if self.cancel.is_cancelled() {
            return Answer::Cancelled;
        }
        let Ok(json) = serde_json::to_string(&question) else {
            return Answer::Cancelled;
        };
        // Réponse déjà donnée (projet relancé) : reprise telle quelle.
        let given = {
            let json = json.clone();
            self.shared
                .store
                .call(move |c| db::take_answer(c, id, &json))
                .await
        };
        if let Ok(Some(answer)) = given {
            return serde_json::from_str(&answer).unwrap_or(Answer::Cancelled);
        }
        // Même question déjà en attente (lancement de l'app) : la même.
        let asked = {
            let json = json.clone();
            self.shared
                .store
                .call(move |c| match db::same_pending(c, id, &json)? {
                    Some(question) => Ok(question),
                    None => db::add_question(c, id, &json, now_ms()),
                })
                .await
        };
        let Ok(question_id) = asked else {
            return Answer::Cancelled;
        };
        let mut receive = self.shared.wait_answer(id, question_id);
        // Répondue entre l'enregistrement et l'inscription : lue en base.
        let early = self
            .shared
            .store
            .call(move |c| db::get_question(c, id, question_id))
            .await
            .ok()
            .flatten()
            .filter(|row| row.status == "answered")
            .and_then(|row| row.answer);
        if let Some(early) = early {
            self.shared.forget_waiter(id);
            return self.answered(question_id, &early).await;
        }
        self.shared.set_state(id, ProjectState::Waiting, None).await;
        self.shared.alert(Alert::Waiting {
            title: self.info.title.clone(),
        });
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => {
                self.shared.forget_waiter(id);
                Answer::Cancelled
            }
            received = &mut receive => match received {
                Ok(json) => self.answered(question_id, &json).await,
                Err(_) => Answer::Cancelled,
            },
            () = tokio::time::sleep(self.shared.park_after()) => {
                self.park(question_id).await;
                Answer::Cancelled
            }
        }
    }

    /// Pas de réponse depuis `PARK_AFTER` : le projet rend sa place (modèle
    /// relâché à la fin du passage, file libérée) et reste en attente ; la
    /// réponse le remet en file (`Workshop::answer`), et le passage suivant la
    /// reprend sans reposer la question (`take_answer`). Une réponse arrivée
    /// pendant le départ (envoyée à l'attente déjà quittée) est vue en base :
    /// le projet se remet lui-même en file.
    async fn park(&self, question: i64) {
        let id = self.id();
        self.shared.park(id);
        let answered = self
            .shared
            .store
            .call(move |c| db::get_question(c, id, question))
            .await
            .ok()
            .flatten()
            .is_some_and(|row| row.status == "answered");
        if answered && let Err(error) = self.shared.requeue(id, &["waiting"]).await {
            // i18n-ignore : journal.
            crate::app_log!("chatonlm: workshop: project {id} not queued again: {error}");
        }
    }

    async fn give_way(&self) {
        let id = self.id();
        let mut idle = Box::pin(self.shared.interactive().idle(self.shared.grace()));
        if (&mut idle).now_or_never().is_some() {
            return;
        }
        self.shared
            .set_state(id, ProjectState::Yielding, None)
            .await;
        self.emit_kind(EventKind::Yielding).await;
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => {}
            () = idle => {
                self.shared.set_state(id, ProjectState::Running, None).await;
                self.emit_kind(EventKind::Resumed).await;
            }
        }
    }

    async fn write(&self, relative: &str, bytes: &[u8]) -> Result<(), String> {
        let root = self.info.dir.clone();
        let relative = relative.to_owned();
        let bytes = bytes.to_vec();
        let written = {
            let relative = relative.clone();
            tauri::async_runtime::spawn_blocking(move || files::write_in(&root, &relative, &bytes))
                .await
        };
        match written {
            Ok(Ok(())) => Ok(()),
            // Code stable seulement ; le message du système part au journal.
            Ok(Err(error)) => {
                error.log(&relative);
                Err(error.code())
            }
            Err(error) => {
                // i18n-ignore : journal.
                crate::app_log!("chatonlm: workshop: {relative} not written: {error}");
                Err("io".into())
            }
        }
    }
}

impl Host for ProjectHost {
    fn project(&self) -> &ProjectInfo {
        &self.info
    }

    fn emit(&self, kind: EventKind) -> HostFuture<'_, u64> {
        Box::pin(self.emit_kind(kind))
    }

    fn save(&self, key: StateKey, value: Value) -> HostFuture<'_, ()> {
        Box::pin(self.save_value(key, value))
    }

    fn load(&self, key: StateKey) -> HostFuture<'_, Option<Value>> {
        Box::pin(self.load_value(key))
    }

    fn ask(&self, question: Question) -> HostFuture<'_, Answer> {
        Box::pin(self.ask_question(question))
    }

    fn yield_to_user(&self) -> HostFuture<'_, ()> {
        Box::pin(self.give_way())
    }

    fn write_file<'a>(
        &'a self,
        rel: &'a str,
        bytes: &'a [u8],
    ) -> HostFuture<'a, Result<(), String>> {
        Box::pin(self.write(rel, bytes))
    }

    fn cancelled(&self) -> CancellationToken {
        self.cancel.clone()
    }
}
