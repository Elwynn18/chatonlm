//! Atelier (moteur de tâches complexes, propre à ChatonLM), lot 2.
//!
//! - `pipeline` : la logique d'un projet, de la tâche au livrable (contre les
//!   traits `Host` et `Model`).
//! - Le socle : base (`store::workshop`, migration 0050), file des projets
//!   (un seul calcule à la fois, [`service`]), hôte réel d'un projet
//!   ([`project`] : événements en base et diffusés, questions, cession à
//!   l'utilisateur, écriture sur le disque avec garde-fous [`files`]), choix
//!   du modèle au lancement ([`model`]), branchement sur l'app ([`app`]) et
//!   commandes `workshop_*` (`commands/workshop.rs`).
//!
//! Le moteur ne produit que des codes et des données ; les textes montrés
//! passent par la page (lot 3) ou par `t!` (notifications, erreurs des
//! commandes). Code commun à Linux et Windows (dossier des projets par défaut
//! : `Documents/ChatonLM` de l'utilisateur, résolu par Tauri).
//! Contrat : `.tools/research/111-lot2-contrat.md`.

pub(crate) mod app;
pub(crate) mod files;
pub(crate) mod model;
pub mod pipeline;
pub(crate) mod project;
pub(crate) mod service;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::engine::auto::ModelWarning;
use pipeline::types::{Answer, Autonomy, Phase, ProjectState, Question, Report, Strategy};

pub use service::Workshop;

/// Événement Tauri : un projet a changé (débit borné : la page relit).
pub const NOTICE_EVENT: &str = "workshop-event";

/// Longueur maximale de la tâche (caractères).
pub const MAX_TASK_CHARS: usize = 20_000;

/// Pièces jointes au plus.
pub const MAX_ATTACHMENTS: usize = 10;

/// Événements rendus au plus par lecture du journal.
pub const MAX_EVENTS_PER_READ: usize = 1_000;

/// Nouveau projet (commande `workshop_create`).
#[derive(Clone, Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct NewProjectRequest {
    /// La tâche, au plus [`MAX_TASK_CHARS`] caractères.
    pub task: String,
    /// Fichiers préparés (zone des pièces jointes : `pick_files` puis
    /// `stage_paths`), texte seulement, au plus [`MAX_ATTACHMENTS`] ; repris
    /// (retirés de la zone) par la création.
    #[serde(default)]
    pub attachment_ids: Vec<String>,
    /// Stratégie imposée ; absente : choisie par le projet.
    #[ts(optional)]
    pub strategy: Option<Strategy>,
    pub autonomy: Autonomy,
    /// Modèle demandé ; absent ou vide : choisi au lancement.
    #[ts(optional)]
    pub model: Option<String>,
    /// Nom du sous-dossier voulu ; absent : tiré du titre.
    #[ts(optional)]
    pub folder_name: Option<String>,
}

/// Projet dans la liste.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectSummary {
    pub id: i64,
    pub title: String,
    /// Stratégie connue (imposée, ou choisie par le projet).
    pub strategy: Option<Strategy>,
    pub strategy_forced: bool,
    pub autonomy: Autonomy,
    pub state: ProjectState,
    pub phase: Option<Phase>,
    /// Modèle demandé ; vide : choisi au lancement.
    pub model: String,
    /// Modèle retenu au dernier lancement ; vide : jamais lancé.
    pub model_used: String,
    /// Rang dans la file (1 : le prochain) ; `None` hors de la file.
    pub position: Option<u32>,
    /// Code de l'erreur (état `failed`).
    pub error: Option<String>,
    /// Dossier du projet (chemin absolu).
    pub folder: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

/// Question en attente d'une réponse.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PendingQuestion {
    pub id: i64,
    /// `Question` du pipeline (`{type, value}`). Sans type TypeScript tant
    /// que ses types `Plan` et `Question` n'ont pas de nom ts-rs propre :
    /// ceux de l'Étude portent déjà ces noms dans `src/bindings`.
    #[ts(type = "unknown")]
    pub question: Question,
    pub asked_at: i64,
}

/// Projet complet (commande `workshop_get`).
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectDetail {
    pub project: ProjectSummary,
    pub task: String,
    /// Noms des pièces jointes, dans l'ordre.
    pub attachments: Vec<String>,
    pub question: Option<PendingQuestion>,
    /// Dernier numéro d'événement (0 : aucun).
    pub last_seq: u64,
    /// Rapport de vérification, une fois le livrable écrit.
    pub report: Option<Report>,
}

/// Réglages de l'Atelier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkshopSettings {
    /// Dossier des projets en vigueur (chemin absolu) ; vide : aucun (pas de
    /// dossier Documents connu et aucun choisi).
    pub dir: String,
    /// Le dossier a été choisi (sinon : `Documents/ChatonLM`).
    pub custom: bool,
    /// Recherche web permise aux projets.
    pub web: bool,
    /// Autonomie proposée pour un nouveau projet.
    pub autonomy: Autonomy,
}

/// Changement des réglages : seuls les champs présents changent.
#[derive(Clone, Debug, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct WorkshopSettingsPatch {
    /// Dossier rendu par `workshop_pick_dir` ; vide : celui par défaut.
    #[ts(optional)]
    pub dir: Option<String>,
    #[ts(optional)]
    pub web: Option<bool>,
    #[ts(optional)]
    pub autonomy: Option<Autonomy>,
}

/// Modèle qu'un projet prendrait (écran « nouveau projet »).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelCheck {
    /// Modèle retenu ; `None` : aucun (code dans `error`).
    pub model: Option<String>,
    /// Choisi par l'Atelier (aucun modèle demandé).
    pub auto: bool,
    pub warnings: Vec<ModelWarning>,
    /// Pourquoi aucun modèle : `noModel`, `modelNotFound`, `remoteOff`,
    /// `contextTooSmall` (contexte possible sous 16 384 jetons).
    pub error: Option<String>,
}

/// Réponse de l'utilisateur à une question (commande `workshop_answer`).
#[derive(Clone, Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct AnswerRequest {
    pub question_id: i64,
    /// `Answer` du pipeline (`{type, value}`) ; voir `PendingQuestion`.
    #[ts(type = "unknown")]
    pub answer: Answer,
}

/// Charge utile de l'événement [`NOTICE_EVENT`] : ce qui a changé d'un
/// projet (la page relit le projet ou son journal).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkshopNotice {
    pub project_id: i64,
    /// Nouvel état, s'il a changé.
    pub state: Option<ProjectState>,
    /// Dernier numéro d'événement, s'il y en a de nouveaux.
    pub last_seq: Option<u64>,
    /// Projet supprimé.
    pub deleted: bool,
}

impl WorkshopNotice {
    /// Garde le plus récent de chaque champ (débit borné).
    fn merge(&mut self, newer: WorkshopNotice) {
        if newer.state.is_some() {
            self.state = newer.state;
        }
        self.last_seq = self.last_seq.max(newer.last_seq);
        self.deleted |= newer.deleted;
    }
}

/// Énumération du moteur en texte de la base (son JSON).
pub(crate) fn enc<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Texte de la base relu en énumération du moteur.
pub(crate) fn dec<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}
