//! Cours générés par un modèle Ollama (onglet Study, propre à ChatonLM).
//!
//! Un cours se prépare en plusieurs temps, chacun en JSON contraint par un
//! schéma (`format` d'Ollama) : plan (titre, sorte de sujet, objectifs,
//! paliers), puis pour chaque palier une leçon et ses questions, enfin un
//! bilan. Le plan est montré avant de commencer ; le premier palier est
//! préparé aussitôt, les suivants pendant que l'apprenant lit. Les questions
//! sont vérifiées (garde-fou, puis le modèle y répond sans la solution) et
//! corrigées sans modèle. Tout est enregistré dans la base : la génération
//! reprend où elle s'était arrêtée, rien n'est généré deux fois.
//!
//! Contrat entre le moteur et l'interface : `.tools/research/47-study-contract.md`.

mod checks;
pub(crate) mod data;
mod engine;
mod generate;
mod grade;
mod notes;
pub(crate) mod passage;
mod prompts;
mod repair;
mod request;
mod sources;

pub use engine::{Ctx, StudyEngine};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::i18n::Lang;
use crate::store::settings::Settings;

/// Évènement émis à chaque changement d'un cours (génération, réponse,
/// suppression) ; charge utile : [`StudyUpdated`].
pub const UPDATED_EVENT: &str = "study-updated";

/// Longueur maximale d'un sujet (caractères).
pub const MAX_TOPIC_CHARS: usize = 300;

/// Documents au plus par cours.
pub const MAX_DOCUMENTS: usize = 3;

/// Longueur maximale d'une question posée (« Ask a question »).
pub const MAX_QUESTION_CHARS: usize = 1_000;

/// Niveau de départ choisi par l'apprenant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "StudyLevel")]
pub enum StudyLevel {
    /// « Découverte » : rien n'est supposé connu.
    Beginner,
    /// « J'ai des bases ».
    Intermediate,
    /// « Approfondir ».
    Advanced,
}

/// Sorte de sujet, choisie par le modèle dans le plan : forme de la leçon et
/// types de questions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "StudyKind")]
pub enum StudyKind {
    /// Idées, théorie, sciences.
    Concept,
    /// Calcul, formules (LaTeX entre `$`).
    Math,
    /// Programmation (blocs de code).
    Code,
    /// Langue étrangère (vocabulaire, grammaire).
    Language,
    /// Histoire, culture, géographie, événements.
    History,
    /// Savoir-faire, procédure.
    Skill,
}

/// Demande d'un nouveau cours.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NewCourse {
    /// Sujet (au plus [`MAX_TOPIC_CHARS`]) ; vide permis avec des documents.
    pub topic: String,
    pub level: StudyLevel,
    /// Durée visée : 10, 20 ou 30 minutes.
    pub minutes: u32,
    /// Recherche web (compte Ollama relié au serveur).
    pub web: bool,
    /// Fichiers préparés (zone des pièces jointes : `pick_files` puis
    /// `stage_paths`), texte seulement, au plus [`MAX_DOCUMENTS`] ; repris
    /// (retirés de la zone) par la création.
    pub document_ids: Vec<String>,
}

/// État d'un cours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CourseStatus {
    /// Préparation du plan (sources lues, recherche web, plan).
    Planning,
    /// Sujet ambigu : question posée ([`Course::clarify`]).
    Unclear,
    /// Plan prêt, pas encore commencé.
    Planned,
    /// Commencé.
    Active,
    /// Tous les paliers et le bilan faits.
    Finished,
    /// Préparation du plan impossible ([`Course::error`]).
    Failed,
}

/// Code d'une erreur : l'interface choisit l'action à proposer ; le message
/// (dans la langue de l'interface au moment de l'erreur) est montré tel quel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum StudyErrorCode {
    /// Aucun modèle réglé, ni pour les cours ni dans le chat.
    NoModel,
    /// Modèle absent du serveur.
    ModelNotFound,
    /// Serveur Ollama injoignable.
    Unreachable,
    /// Recherche web demandée sans compte relié au serveur (401).
    SignedOut,
    /// Réponses du modèle inutilisables après les essais prévus.
    Invalid,
    /// Autre échec (erreur du modèle, délai dépassé...).
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StudyError {
    pub code: StudyErrorCode,
    pub message: String,
}

/// État de génération d'une partie (palier, nouvel essai, bilan).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum GenState {
    /// À générer (en file, ou attendue plus tard).
    Pending,
    Generating,
    Ready,
    /// Échec (voir l'erreur de la partie) ; `study_retry` relance.
    Failed,
}

/// Question posée quand le sujet a plusieurs sens courants.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Clarify {
    pub question: String,
    /// Sens possibles (2 à 4).
    pub options: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlanLevel {
    pub title: String,
    pub goal: String,
    /// Notions enseignées (2 à 4).
    pub points: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Plan {
    pub title: String,
    pub kind: StudyKind,
    pub summary: String,
    /// Trois objectifs.
    pub objectives: Vec<String>,
    /// Un par palier (2, 4 ou 6 selon la durée).
    pub levels: Vec<PlanLevel>,
    /// Ce qui est laissé de côté quand le sujet est trop vaste ; vide sinon.
    pub scope: String,
    /// Trois sujets pour aller plus loin.
    pub further: Vec<String>,
}

/// Source du cours, montrée dans le plan et à la fin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StudySource {
    pub kind: SourceKind,
    /// Nom du fichier, ou titre de la page.
    pub title: String,
    /// Adresse d'une page (vide pour un document).
    pub url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SourceKind {
    Document,
    Web,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LessonSection {
    /// Titre court, sans Markdown.
    pub heading: String,
    /// Markdown (gras, listes, code, formules `$…$`).
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkedExample {
    /// Markdown.
    pub problem: String,
    /// Markdown : solution pas à pas.
    pub solution: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Lesson {
    /// Accroche du palier : question intrigante ou fait surprenant, une ou
    /// deux phrases (Markdown).
    pub hook: String,
    /// Trois sections (deux au moins).
    pub sections: Vec<LessonSection>,
    pub example: WorkedExample,
    /// À retenir (2 à 4 phrases, Markdown).
    pub key_points: Vec<String>,
    /// Piège fréquent (Markdown).
    pub pitfall: String,
}

/// Question telle que montrée (sans sa solution : la correction est faite
/// par le moteur, `study_answer`). Textes en Markdown sur une ligne
/// (formules et `code` permis).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum QuestionBody {
    /// Choix unique parmi 3 ou 4 options (déjà mélangées).
    Choice {
        prompt: String,
        options: Vec<String>,
    },
    /// Vrai ou faux.
    TrueFalse { statement: String },
    /// Phrase à trou : `prompt` contient exactement un `___`.
    Blank { prompt: String },
    /// Éléments à remettre dans l'ordre (3 à 5, déjà mélangés).
    Order { prompt: String, items: Vec<String> },
    /// Associer chaque élément de gauche à un de droite (3 ou 4 paires ;
    /// droite déjà mélangée).
    Match {
        prompt: String,
        left: Vec<String>,
        right: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Question {
    /// Unique dans le cours.
    pub id: String,
    pub body: QuestionBody,
}

/// Réponse de l'apprenant, ou solution attendue (même forme), dans l'ordre
/// montré.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum AnswerValue {
    /// Indice de l'option choisie.
    Choice {
        index: u32,
    },
    TrueFalse {
        value: bool,
    },
    Blank {
        text: String,
    },
    /// Indices des éléments montrés, dans l'ordre donné.
    Order {
        order: Vec<u32>,
    },
    /// Pour chaque élément de gauche, l'indice de l'élément de droite.
    Match {
        pairs: Vec<u32>,
    },
}

/// Réponse corrigée (la première réponse à une question est définitive).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Answered {
    pub given: AnswerValue,
    pub correct: bool,
    /// Texte à trou accepté malgré une petite faute (accent, frappe) :
    /// l'interface montre l'orthographe attendue.
    pub close: bool,
    /// Solution attendue.
    pub expected: AnswerValue,
    /// Ordre et association : juste ou non, élément par élément (vide
    /// sinon).
    pub detail: Vec<bool>,
    /// Pourquoi la bonne réponse est la bonne (Markdown).
    pub explanation: String,
    pub at: i64,
}

/// Essai de validation d'un palier : le premier (questions du palier), puis
/// un par « Explain differently » (nouvelle explication et nouvelles
/// questions).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Attempt {
    /// Explication autrement, centrée sur les erreurs (Markdown) ; `None`
    /// pour le premier essai.
    pub explanation: Option<String>,
    /// En général 3 ; au moins 2.
    pub questions: Vec<Question>,
    /// Bonnes réponses pour réussir (2 sur 3 ; arrondi supérieur des deux
    /// tiers sinon).
    pub pass_mark: u32,
}

/// « I don't understand » sur une section : explication plus simple.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Explain {
    pub section: u32,
    /// Markdown.
    pub text: String,
}

/// « Ask a question » : question de l'apprenant et réponse.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Ask {
    pub question: String,
    /// Markdown.
    pub answer: String,
}

/// Avancement d'un palier, calculé par le moteur.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LevelStatus {
    /// Après le palier en cours.
    Locked,
    /// Premier palier ni réussi ni passé.
    Current,
    /// Un essai réussi.
    Passed,
    /// « Continue anyway » après deux essais manqués : à revoir.
    Review,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Level {
    pub index: u32,
    /// Leçon, question d'entraînement et premier essai.
    pub state: GenState,
    pub error: Option<StudyError>,
    pub lesson: Option<Lesson>,
    /// Question d'entraînement (non comptée).
    pub check: Option<Question>,
    pub attempts: Vec<Attempt>,
    /// Nouvel essai demandé (« Explain differently ») : `Pending` puis
    /// `Generating`, ou `Failed` avec `remedy_error` ; `None` sans demande en
    /// cours.
    pub remedy: Option<GenState>,
    pub remedy_error: Option<StudyError>,
    pub status: LevelStatus,
    /// Essais terminés et manqués.
    pub failed_attempts: u32,
    pub explains: Vec<Explain>,
    pub asks: Vec<Ask>,
    /// Durée estimée (lecture et questions du premier essai).
    pub estimated_seconds: u32,
}

/// Bilan final : questions sur tout le cours.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FinalQuiz {
    pub state: GenState,
    pub error: Option<StudyError>,
    /// 3, 5 ou 6 selon la durée.
    pub questions: Vec<Question>,
    pub estimated_seconds: u32,
}

/// Cours complet, tel que lu par la page du cours.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Course {
    pub id: String,
    pub topic: String,
    pub level: StudyLevel,
    pub minutes: u32,
    pub language: Lang,
    pub model: String,
    pub web: bool,
    pub status: CourseStatus,
    pub error: Option<StudyError>,
    pub clarify: Option<Clarify>,
    pub plan: Option<Plan>,
    pub sources: Vec<StudySource>,
    /// Un par palier du plan (vide avant le plan).
    pub levels: Vec<Level>,
    pub final_quiz: Option<FinalQuiz>,
    /// Réponses, par identifiant de question.
    pub answers: std::collections::BTreeMap<String, Answered>,
    /// Étape où l'apprenant s'est arrêté (clé de l'interface, vide au début).
    pub position: String,
    pub elapsed_ms: i64,
    /// Durée estimée du cours entier (plan : durée visée ; ensuite, somme
    /// des paliers générés et du bilan).
    pub estimated_seconds: u32,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
    /// Génération mise en pause par l'apprenant (`study_pause`) : plus rien
    /// n'est écrit pour ce cours jusqu'à la reprise.
    pub paused: bool,
    /// Au moment de la lecture (`study_get`), la file écrit une partie d'un
    /// autre cours : celles de ce cours attendent leur tour.
    pub queue_busy: bool,
}

/// Ligne de la liste des cours (page d'accueil de l'onglet).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CourseSummary {
    pub id: String,
    /// Titre du plan, sinon le sujet (ou les noms des documents).
    pub title: String,
    pub topic: String,
    pub level: StudyLevel,
    pub minutes: u32,
    pub status: CourseStatus,
    /// Paliers réussis ou passés.
    pub levels_done: u32,
    pub levels_total: u32,
    /// Paliers écrits (prêts à être lus).
    pub levels_ready: u32,
    /// Des parties restent à écrire (en file, en cours, ou retenues par la
    /// pause) ; faux si une partie en échec attend `study_retry`.
    pub pending: bool,
    /// Génération mise en pause par l'apprenant.
    pub paused: bool,
    /// Bonnes réponses du bilan (cours fini).
    pub final_correct: Option<u32>,
    pub final_total: Option<u32>,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

/// Charge utile de [`UPDATED_EVENT`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StudyUpdated {
    pub course_id: String,
    /// Cours supprimé.
    pub deleted: bool,
}

/// Évènements d'une réponse en flux (« I don't understand », « Ask a
/// question ») : des `Delta`, puis `Done` (texte complet, enregistré) ou
/// `Error`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "event", rename_all = "camelCase")]
#[ts(export)]
pub enum StudyStreamEvent {
    Delta {
        text: String,
    },
    Done {
        text: String,
        stopped: bool,
    },
    Error {
        code: StudyErrorCode,
        message: String,
    },
}

/// Modèle des cours : celui réglé, sinon « Auto » (le meilleur modèle en
/// mémoire, sinon celui du chat, voir `ollama::auto`) ; `None` si aucun.
pub async fn model_of(settings: &Settings) -> Option<String> {
    crate::ollama::auto::model_of(
        &settings.effective_server_url,
        &settings.study_model,
        &settings.selected_model,
    )
    .await
}

#[cfg(test)]
mod tests;
