//! Pipeline d'un projet de l'Atelier (lot 2) : de la tâche au livrable,
//! contre le socle ([`Host`]) et le modèle ([`Model`]). Contrat :
//! `.tools/research/111-lot2-contrat.md` (partie P) ; conception
//! `109-moteur-taches-conception.md` (§ 0 bis prime).
//!
//! Déroulé : stratégie (3 tirages, réflexion coupée) → cadrage (questions,
//! ou hypothèses en Autonome) → spécification qui tranche → version directe
//! d'abord (≤ 4 000 mots), tenue aux mêmes vérifications que l'assemblage →
//! sinon plan (contrôles par programme, juge, 3 essais un par un, accord de
//! l'utilisateur selon l'autonomie) → parties dans l'ordre des dépendances
//! (consignes avec le registre GLOBAL, vérifications par programme + juge de
//! forme + juge d'exactitude, UNE correction en gardant la meilleure
//! version, passe chirurgicale pour quelques affirmations fausses, redécoupage
//! ADaPT, carte de sortie) → assemblage sans jointures → vérification
//! d'ensemble → relecture de la langue → livraison et rapport.
//!
//! Reprise : chaque requête au modèle a sa clé d'état ([`StateKey`]) ; lue
//! avant, sauvée après : un nouveau passage ne refait rien de fait. Cession :
//! [`Host::yield_to_user`] avant CHAQUE requête. Annulation : jeton du
//! socle, vérifié avant et pendant chaque requête ; l'état est déjà sauvé.
//! Erreur passagère du modèle (délai, chargement, coupure, 5xx) : jusqu'à
//! [`RETRIES`] nouveaux essais, attente croissante ([`retry_wait`]), avant
//! d'arrêter le projet.
//! Le moteur n'émet que des codes ; consignes au modèle en anglais
//! (`prompts.rs`). Code commun à Linux et Windows.

pub mod host;
pub mod model;
pub mod types;

mod brief;
mod build;
mod checks;
mod finish;
mod flow;
mod language_pass;
mod measure;
mod plan;
mod prompts;
mod registry;
mod surgery;
mod text;

#[cfg(test)]
mod tests;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::agent::{AgentError, Call, Write};
use crate::inference::Message;

/// Lecture de l'état sauvé par le socle (parties montrées par la page,
/// `workshop_part(s)`, lot 3) : mêmes formes que celles du pipeline.
pub(crate) use build::{BrickRecord, body_words, read_split};
pub use host::Host;
pub use model::Model;
pub(crate) use text::words;
use types::{Activity, Autonomy, EventKind, Phase, ProjectState, StateKey};

/// Fin d'un passage du pipeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunEnd {
    /// Livrable écrit (relatif au dossier du projet), rapport sauvé.
    Delivered { file: String },
    /// Jeton du socle annulé (pause ou annulation : le socle sait lequel),
    /// ou annulation pendant une question. État sauvé : un nouveau passage
    /// reprend.
    Cancelled,
    /// L'utilisateur a refusé le plan sans note.
    Rejected,
    /// Échec (code stable, aussi émis en `EventKind::Error`).
    Failed { code: String },
}

/// Nouveaux essais d'une requête après une erreur passagère du modèle.
pub const RETRIES: u32 = 3;

/// Attente avant le nouvel essai n° `n` (0 : le premier) : 5 s, 15 s, 45 s.
pub fn retry_wait(n: u32) -> std::time::Duration {
    std::time::Duration::from_secs(5 * 3u64.pow(n))
}

/// Arrêt d'une étape.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Stop {
    Cancelled,
    Rejected,
    Failed(String),
}

impl From<AgentError> for Stop {
    fn from(error: AgentError) -> Self {
        match error {
            AgentError::Cancelled => Stop::Cancelled,
            other => Stop::Failed(other.code().to_owned()),
        }
    }
}

/// Mène le projet de `host` avec `model` jusqu'au livrable, ou jusqu'à un
/// arrêt. Reprend là où un passage précédent s'est arrêté.
pub async fn run(host: &dyn Host, model: &dyn Model) -> RunEnd {
    let run = Run::new(host, model);
    match run.all().await {
        Ok(file) => {
            run.emit(EventKind::Finished {
                state: ProjectState::Done,
            })
            .await;
            RunEnd::Delivered { file }
        }
        Err(Stop::Cancelled) => RunEnd::Cancelled,
        Err(Stop::Rejected) => RunEnd::Rejected,
        Err(Stop::Failed(code)) => {
            // i18n-ignore : journal.
            crate::app_log!(
                "chatonlm: workshop: project failed with {} ({code})",
                model.model()
            );
            run.emit(EventKind::Error { code: code.clone() }).await;
            run.emit(EventKind::Finished {
                state: ProjectState::Failed,
            })
            .await;
            RunEnd::Failed { code }
        }
    }
}

/// Un passage du pipeline.
struct Run<'a> {
    host: &'a dyn Host,
    model: &'a dyn Model,
    cancel: CancellationToken,
    /// Tâche et documents de l'utilisateur : ses propres mots ne sont jamais
    /// « internes au moteur ».
    allowed: String,
}

impl<'a> Run<'a> {
    fn new(host: &'a dyn Host, model: &'a dyn Model) -> Self {
        let project = host.project();
        let mut allowed = format!("{}\n{}", project.title, project.task);
        for attachment in &project.attachments {
            allowed.push('\n');
            allowed.push_str(&attachment.text);
        }
        Run {
            host,
            model,
            cancel: host.cancelled(),
            allowed,
        }
    }

    fn project(&self) -> &types::ProjectInfo {
        self.host.project()
    }

    fn autonomous(&self) -> bool {
        self.project().autonomy == Autonomy::Autonomous
    }

    async fn emit(&self, kind: EventKind) {
        self.host.emit(kind).await;
    }

    async fn phase(&self, phase: Phase, started: bool) {
        self.emit(if started {
            EventKind::PhaseStarted { phase }
        } else {
            EventKind::PhaseDone { phase }
        })
        .await;
    }

    async fn warn(&self, code: &str) {
        self.emit(EventKind::Warning { code: code.into() }).await;
    }

    /// Avertissement gardé aussi pour le rapport ([`StateKey::Notes`]).
    async fn note(&self, code: &str) {
        self.warn(code).await;
        let mut notes: Vec<String> = self.load(StateKey::Notes).await.unwrap_or_default();
        if !notes.iter().any(|n| n == code) {
            notes.push(code.to_owned());
            self.save(StateKey::Notes, &notes).await;
        }
    }

    /// Caractères des pièces jointes dans une consigne, selon le contexte du
    /// modèle ([`prompts::attachment_room`]).
    fn attachment_room(&self) -> usize {
        prompts::attachment_room(self.model.context())
    }

    /// Les pièces jointes ne tiennent pas entières dans les consignes.
    fn attachments_cut(&self) -> bool {
        prompts::attachments_cut(&self.project().attachments, self.attachment_room())
    }

    /// Avant un nouvel essai après une erreur passagère : vrai s'il faut
    /// réessayer (avertissement, attente croissante interrompue par
    /// l'annulation).
    async fn retry(&self, error: &AgentError, attempt: u32) -> Result<bool, AgentError> {
        if !error.transient() || attempt >= RETRIES {
            return Ok(false);
        }
        self.warn("modelRetry").await;
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(AgentError::Cancelled),
            () = tokio::time::sleep(retry_wait(attempt)) => Ok(true),
        }
    }

    async fn load<T: DeserializeOwned>(&self, key: StateKey) -> Option<T> {
        let value = self.host.load(key).await?;
        match serde_json::from_value(value) {
            Ok(value) => Some(value),
            Err(error) => {
                // i18n-ignore : journal.
                crate::app_log!(
                    "chatonlm: workshop: unreadable state {} ({error})",
                    key.name()
                );
                None
            }
        }
    }

    async fn save<T: Serialize>(&self, key: StateKey, value: &T) {
        match serde_json::to_value(value) {
            Ok(value) => self.host.save(key, value).await,
            // i18n-ignore : journal.
            Err(error) => crate::app_log!(
                "chatonlm: workshop: state {} not saved ({error})",
                key.name()
            ),
        }
    }

    /// Avant une requête : céder la place à l'utilisateur, puis vérifier
    /// l'annulation.
    async fn gate(&self) -> Result<(), AgentError> {
        if self.cancel.is_cancelled() {
            return Err(AgentError::Cancelled);
        }
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Err(AgentError::Cancelled),
            () = self.host.yield_to_user() => {}
        }
        if self.cancel.is_cancelled() {
            return Err(AgentError::Cancelled);
        }
        Ok(())
    }

    /// Réponse JSON, mise en cache sous `key` : relue si elle existe, sinon
    /// demandée (après la cession) puis sauvée.
    async fn json(
        &self,
        key: StateKey,
        activity: Activity,
        call: Call,
    ) -> Result<Value, AgentError> {
        if let Some(value) = self.host.load(key).await {
            return Ok(value);
        }
        let mut attempt = 0;
        let json = loop {
            self.gate().await?;
            self.emit(EventKind::Activity {
                activity: activity.clone(),
            })
            .await;
            let result = tokio::select! {
                biased;
                () = self.cancel.cancelled() => Err(AgentError::Cancelled),
                json = self.model.ask_json(&call) => json,
            };
            match result {
                Ok(json) => break json,
                Err(error) if self.retry(&error, attempt).await? => attempt += 1,
                Err(error) => return Err(error),
            }
        };
        self.host.save(key, json.value.clone()).await;
        Ok(json.value)
    }

    /// Texte, mis en cache sous `key` comme [`Run::json`]. Coupé à la limite
    /// (après le nouvel essai du `Worker`) : le texte obtenu est gardé (les
    /// vérifications diront ce qui manque).
    async fn text(
        &self,
        key: StateKey,
        activity: Activity,
        messages: Vec<Message>,
        write: Write,
    ) -> Result<String, AgentError> {
        if let Some(text) = self.load::<String>(key).await {
            return Ok(text);
        }
        let ignore = |_| {};
        let mut attempt = 0;
        let text = loop {
            self.gate().await?;
            self.emit(EventKind::Activity {
                activity: activity.clone(),
            })
            .await;
            let result = tokio::select! {
                biased;
                () = self.cancel.cancelled() => Err(AgentError::Cancelled),
                text = self.model.write(messages.clone(), &write, &ignore) => text,
            };
            match result {
                Ok(text) => break text.content,
                Err(AgentError::Incomplete { partial }) => break partial,
                Err(error) if self.retry(&error, attempt).await? => attempt += 1,
                Err(error) => return Err(error),
            }
        };
        self.save(key, &text).await;
        Ok(text)
    }

    /// Jetons de `text` (sans cession : requête légère) ; le modèle ne sait
    /// pas compter : estimés à 3 caractères par jeton.
    async fn tokens(&self, text: &str) -> Result<usize, Stop> {
        let counted = tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(AgentError::Cancelled),
            n = self.model.count_tokens(text) => n,
        };
        match counted {
            Ok(n) => Ok(n),
            Err(AgentError::Cancelled) => Err(Stop::Cancelled),
            Err(_) => Ok(text.chars().count().div_ceil(3)),
        }
    }

    /// Tout le projet ; rend le livrable.
    async fn all(&self) -> Result<String, Stop> {
        if let Some(file) = self.load::<String>(StateKey::Delivered).await {
            return Ok(file);
        }
        if self.attachments_cut() {
            // Documents coupés dans les consignes : dit tel quel (événement,
            // rapport : `documents` non vérifiés en entier).
            self.warn("attachmentsCut").await;
        }
        let chosen = self.strategy().await?;
        let framing = self.framing(&chosen).await?;
        let mut spec = self.spec(chosen.strategy, &framing).await?;

        let mut plan = None;
        if self.project().autonomy == Autonomy::Plan {
            // Autonomie « Plan » : rien n'est produit avant l'accord du plan ;
            // la version directe suit alors le plan approuvé.
            let (approved, noted) = self.plan(chosen.strategy, &spec).await?;
            plan = Some(approved);
            spec = noted;
        }
        if self.direct_fits(&spec)
            && let Some(direct) = self.direct(&spec, plan.as_ref()).await?
        {
            return self
                .finish(chosen.strategy, &spec, finish::Output::Direct(direct))
                .await;
        }
        let plan = match plan {
            Some(plan) => plan,
            None => {
                let (approved, noted) = self.plan(chosen.strategy, &spec).await?;
                spec = noted;
                approved
            }
        };
        let progress = self.build(&spec, &plan).await?;
        self.finish(
            chosen.strategy,
            &spec,
            finish::Output::Parts { plan, progress },
        )
        .await
    }
}

/// Réglages d'une rédaction : réflexion bornée à `budget`, place pour
/// `words` mots (≈ 3 jetons par mot, au moins 10 000).
fn writing(temperature: f32, budget: u32, words: u32) -> Write {
    Write {
        temperature,
        think: crate::agent::ThinkMode::On { budget },
        max_tokens: Some(budget + prompts::ANSWER.max(words.saturating_mul(3))),
    }
}
