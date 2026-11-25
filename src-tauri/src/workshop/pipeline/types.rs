//! Types communs de l'Atelier (lot 2), partagés entre le pipeline (logique
//! d'un projet) et le socle (base, file, commandes). Contrat :
//! `.tools/research/111-lot2-contrat.md`. Écrits par le pipeline, recopiés à
//! l'identique par le socle ; tout changement est noté dans
//! `.tools/research/111-lot2-notes.md`.
//!
//! Le moteur ne produit que des codes et des données : aucun texte
//! d'interface ici (la page traduit, lot 3). Les types lus par la page
//! dérivent `TS` (export au lot 3, avec la page).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Façon de mener un projet (gabarit). Ce lot ne sait mener que
/// [`Strategy::Generic`] et [`Strategy::LongForm`] ; les trois autres
/// arrivent aux lots 4 et 5 (refusées d'ici là : « pas encore »).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Strategy {
    Generic,
    LongForm,
    Research,
    Documents,
    Code,
}

impl Strategy {
    /// Ce lot sait la mener.
    pub fn available(self) -> bool {
        matches!(self, Strategy::Generic | Strategy::LongForm)
    }
}

/// Niveau d'autonomie (conception § 7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Autonomy {
    /// Tout jusqu'au plan, puis attend l'accord avant de produire.
    Plan,
    /// Demande l'accord du plan et de chaque action à effet.
    Ask,
    /// Demande l'accord du plan seulement.
    AcceptActions,
    /// Ne demande rien : questions de cadrage changées en hypothèses.
    Autonomous,
}

/// Phase d'un projet (frise de la page).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Collect,
    Strategy,
    Framing,
    Spec,
    Plan,
    Build,
    Assemble,
    Verify,
    Deliver,
}

/// État d'un projet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ProjectState {
    Queued,
    Running,
    /// Attend l'utilisateur (question posée).
    Waiting,
    Paused,
    /// Cède la place à l'utilisateur (entre deux requêtes).
    Yielding,
    Done,
    Failed,
    Cancelled,
}

/// État d'une partie du plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum BrickState {
    Pending,
    Running,
    Checking,
    Fixing,
    Done,
    /// Finie, avec des points à revoir (dits dans le rapport).
    ToReview,
    /// Refusée par le modèle.
    Refused,
    /// Redécoupée (ADaPT) : son texte est celui de ses sous-parties.
    Split,
}

/// Portée d'un critère : le document entier (longueur totale, nombre de
/// titres ou de sections, structure d'ensemble), ou une partie qui peut le
/// tenir seule. Un critère du document n'est jamais attribué à une partie ni
/// jugé sur elle : il est jugé sur la version directe et sur le document
/// assemblé (banc du 25 nov. : « 3 300 à 3 700 mots » jugé sur une partie de
/// 300 mots, toujours faux, redécoupée pendant une heure).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "WorkshopCriterionScope")]
pub enum CriterionScope {
    Document,
    #[default]
    Section,
}

/// Critère d'acceptation de la spécification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "WorkshopCriterion")]
pub struct Criterion {
    /// « C1 », « C2 »… (numérotés par le programme).
    pub id: String,
    pub text: String,
    /// Donnée par la spécification, `document` aussi quand le programme
    /// reconnaît un critère d'ensemble ([`super::text::document_wide`]).
    #[serde(default)]
    pub scope: CriterionScope,
}

/// Registre d'adresse au lecteur, tranché par la spécification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "WorkshopRegister")]
pub enum Register {
    /// Deuxième personne familière (« tu », « tú », « du »).
    Informal,
    /// Deuxième personne de politesse (« vous », « usted », « Sie »).
    Formal,
    /// Style impersonnel, sans s'adresser au lecteur.
    Impersonal,
}

/// Spécification : le contrat que partagent toutes les parties. Tout y est
/// TRANCHÉ (langue, registre, unités, longueur), sans « si » ni « ou ».
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "WorkshopSpec", export)]
pub struct Spec {
    pub goal: String,
    pub deliverables: Vec<String>,
    pub criteria: Vec<Criterion>,
    pub audience: String,
    /// Langue de sortie, code ISO 639-1 (« fr », « en »…).
    pub language: String,
    pub register: Register,
    pub units: String,
    /// Longueur du livrable rédigé ; `None` : pas un texte mesuré en mots.
    pub length_words: Option<u32>,
    /// (terme, définition).
    pub glossary: Vec<(String, String)>,
    /// Hypothèses écrites (réponses absentes au cadrage, mode autonome).
    pub assumptions: Vec<String>,
    /// Contraintes, dont les consignes de l'utilisateur numérotées (cadrage,
    /// note au refus d'un plan).
    pub constraints: Vec<String>,
}

/// Nature d'une partie du plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "WorkshopBrickKind")]
pub enum BrickKind {
    Text,
    /// Introduction : écrite à la fin, depuis les cartes des autres.
    Intro,
    /// Conclusion : écrite à la fin, depuis les cartes des autres.
    Conclusion,
}

/// Une partie du plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "WorkshopBrickPlan")]
pub struct BrickPlan {
    /// Entier de 1 à 99 dans le plan ; sous-partie d'un redécoupage :
    /// `parent * 100 + k` (k de 1 à 3).
    pub id: u32,
    pub title: String,
    pub goal: String,
    pub kind: BrickKind,
    pub words: u32,
    pub depends_on: Vec<u32>,
    /// Identifiants des critères de la spécification qu'elle sert.
    pub criteria: Vec<String>,
    /// Ce qu'elle ne doit pas traiter (réservé à une autre partie).
    pub forbidden: Vec<String>,
}

/// Plan du livrable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "WorkshopPlan")]
pub struct Plan {
    /// Titre du livrable (dans la langue de sortie).
    #[serde(default)]
    pub title: String,
    pub bricks: Vec<BrickPlan>,
}

/// Carte de sortie d'une partie : ce que les suivantes en sauront.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub summary: String,
    pub terms: Vec<String>,
    /// Décisions, de la forme « sujet : valeur ».
    pub decisions: Vec<String>,
    /// Listes retenues : (nom, éléments), ex. (« cultures », [...]).
    pub lists: Vec<(String, Vec<String>)>,
    pub exposes: Vec<String>,
}

/// Méthode d'une vérification (le rapport dit COMMENT chaque point l'a été).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum CheckMethod {
    Program,
    FormJudge,
    FactJudge,
}

/// Une vérification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// Ce qui est vérifié : identifiant de critère (« C2 »), code d'un
    /// contrôle par programme (`length`, `internalWord`…), ou, pour le juge
    /// d'exactitude, l'affirmation examinée.
    pub criterion: String,
    pub method: CheckMethod,
    pub passed: bool,
    /// Preuve citée (juge) ou mesure (programme).
    pub evidence: String,
    /// Endroit fautif, mot pour mot.
    #[serde(rename = "where")]
    pub where_: Option<String>,
    /// Un échec qui compte (critère de fond, affirmation fausse, longueur) ;
    /// faux pour ce qui ne bloque jamais (style, affirmation douteuse,
    /// élément hors registre : signalés, « à revoir »).
    #[serde(default)]
    pub blocking: bool,
}

/// Ce qui se passe en ce moment, en codes et données (la page traduit).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "code")]
#[ts(rename = "WorkshopActivity")]
pub enum Activity {
    /// Choix de la stratégie, tirage `draw` (1 à 3).
    ChoosingStrategy {
        draw: u32,
    },
    WritingSpec {
        attempt: u32,
    },
    /// Version directe du livrable (une requête).
    WritingDirect,
    CheckingDirect,
    WritingPlan {
        attempt: u32,
    },
    JudgingPlan {
        attempt: u32,
    },
    WritingBrick {
        id: u32,
    },
    CheckingBrick {
        id: u32,
    },
    FixingBrick {
        id: u32,
    },
    SplittingBrick {
        id: u32,
    },
    SummarizingBrick {
        id: u32,
    },
    /// Passe chirurgicale : seules les phrases jugées fausses sont
    /// corrigées ou retirées (id 0 : version directe).
    FixingSentences {
        id: u32,
    },
    Assembling,
    CheckingWhole,
    /// Relecture de la langue du livrable, passage `block` (0 : le premier).
    Proofreading {
        block: u32,
    },
    Delivering,
}

/// Question posée à l'utilisateur ([`super::host::Host::ask`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
#[ts(rename = "WorkshopQuestion")]
pub enum Question {
    /// Tirages de la stratégie non unanimes : les stratégies proposées, la
    /// plus votée d'abord.
    Strategy(Vec<Strategy>),
    /// Questions de cadrage (2 à 4).
    Framing(Vec<String>),
    /// Plan à approuver (modifiable).
    ApprovePlan(Plan),
    /// Action à effet à permettre (lots 4-5).
    ApproveAction { tool: String, target: String },
}

/// Réponse de l'utilisateur.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
#[ts(rename = "WorkshopAnswer")]
pub enum Answer {
    Strategy(Strategy),
    /// Une réponse par question de cadrage, dans l'ordre (vide : pas de
    /// réponse, une hypothèse sera écrite).
    Framing(Vec<String>),
    Plan {
        approved: bool,
        edited: Option<Plan>,
        note: Option<String>,
    },
    Action(bool),
    /// Projet annulé pendant l'attente.
    Cancelled,
}

/// Ce qui arrive à un projet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "type")]
#[ts(rename = "WorkshopEventKind")]
pub enum EventKind {
    PhaseStarted {
        phase: Phase,
    },
    PhaseDone {
        phase: Phase,
    },
    /// Stratégie retenue (`unanimous` : par trois tirages concordants).
    StrategyChosen {
        strategy: Strategy,
        unanimous: bool,
    },
    /// Plan retenu (après l'accord de l'utilisateur s'il est demandé).
    PlanReady {
        plan: Plan,
    },
    /// Une partie change d'état (`attempt` : 0 pour la première version, 1
    /// pour la correction).
    Brick {
        id: u32,
        state: BrickState,
        attempt: u32,
    },
    /// Vérifications d'une version d'une partie (id 0 : version directe ou
    /// livrable entier).
    BrickChecks {
        id: u32,
        attempt: u32,
        checks: Vec<Check>,
    },
    Activity {
        activity: Activity,
    },
    QuestionAsked {
        question: Question,
    },
    AnswerReceived {
        answer: Answer,
    },
    /// Le modèle de texte prête la carte à un autre moteur (`to` : code du
    /// moteur), puis la reprend (lot 4).
    GpuLent {
        to: String,
    },
    GpuBack,
    /// Le projet cède la place à l'utilisateur, puis reprend (émis par le
    /// socle dans `yield_to_user`).
    Yielding,
    Resumed,
    /// Avertissements du modèle retenu : codes de
    /// `engine::auto::ModelWarning` tels que sérialisés (`tooSmall`,
    /// `small`, `smallContext`, `noThinking`, `unknownSize` ; ce type ne
    /// se relit pas depuis la base).
    ModelWarnings {
        model: String,
        warnings: Vec<String>,
    },
    /// Avertissement sans arrêt (code : `strategyNotYet`, `planBestOfThree`,
    /// `specBestOfThree`, `registryCondensed`, `registryConflict`,
    /// `judgeUnavailable`, `attachmentsCut`, `lengthCapped`, `modelRetry`…).
    Warning {
        code: String,
    },
    /// Erreur qui arrête le projet (code stable).
    Error {
        code: String,
    },
    /// Fin du projet (livré, échoué). Une annulation n'en émet pas : le
    /// socle sait s'il a mis en pause ou annulé.
    Finished {
        state: ProjectState,
    },
}

/// Un événement numéroté.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "WorkshopEvent", export)]
pub struct Event {
    pub seq: u64,
    pub at_ms: i64,
    pub kind: EventKind,
}

/// Pièce jointe, texte déjà extrait par le socle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub name: String,
    pub text: String,
}

/// Ce que le pipeline sait du projet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub id: i64,
    pub title: String,
    pub task: String,
    pub attachments: Vec<Attachment>,
    /// Stratégie imposée par l'utilisateur (le choix est alors sauté).
    pub strategy: Option<Strategy>,
    pub autonomy: Autonomy,
    /// Langue de l'app (« fr »…) : langue de sortie par défaut si la tâche
    /// ne permet pas d'en juger.
    pub app_language: String,
    /// Dossier du projet (livraison).
    pub dir: PathBuf,
}

/// Clé d'un état reprenable ([`super::host::Host::save`]). Le socle la
/// range sous [`StateKey::name`] ; le pipeline seul en connaît la forme des
/// valeurs (JSON). Toute requête au modèle a sa clé : une reprise ne refait
/// rien de ce qui a été sauvé.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StateKey {
    /// Un tirage de la stratégie (1 à 3).
    StrategyDraw(u8),
    /// Stratégie retenue.
    Strategy,
    /// Questions de cadrage et réponses.
    Framing,
    /// Un essai de spécification (1 à 3).
    SpecAttempt(u8),
    /// Spécification retenue (consignes de l'utilisateur comprises).
    Spec,
    /// Un plan candidat et ses contrôles (`round` : tour de plans, un de
    /// plus à chaque note de l'utilisateur ; `attempt` : 1 à 3).
    PlanAttempt { round: u8, attempt: u8 },
    /// Jugement d'un plan candidat.
    PlanJudge { round: u8, attempt: u8 },
    /// Tour de plans en cours (0, puis un de plus à chaque note de
    /// l'utilisateur qui refuse un plan).
    PlanRound,
    /// Plan retenu, approuvé s'il fallait l'approuver.
    Plan,
    /// Texte d'une version (`id` 0 : version directe ; `version` 0 :
    /// première, 1 : correction).
    Text { id: u32, version: u8 },
    /// Verdict du juge de forme sur une version.
    FormJudge { id: u32, version: u8 },
    /// Verdict du juge d'exactitude sur une version.
    FactJudge { id: u32, version: u8 },
    /// Verdict du juge de refus sur une version.
    RefusalJudge { id: u32, version: u8 },
    /// Partie finie : version gardée, état, vérifications.
    Brick(u32),
    /// Carte de sortie d'une partie.
    Card(u32),
    /// Sous-plan d'une partie redécoupée.
    Split(u32),
    /// Passe chirurgicale d'une version (phrases fausses corrigées ou
    /// retirées ; id 0 : version directe). Son re-jugement d'exactitude est
    /// `FactJudge { id, version: 2 }`.
    Surgery(u32),
    /// Relecture de la langue d'un passage du livrable (0 : le premier).
    Proofread(u32),
    /// Contradictions du registre tranchées par le juge.
    Contradictions,
    /// Vérification d'ensemble (critères de la spécification).
    WholeForm,
    /// Rapport de vérification.
    Report,
    /// Livrable écrit sur le disque.
    Delivered,
    /// Avertissements à garder pour le rapport (codes : `lengthCapped`,
    /// `specBestOfThree`…), notés au fil du projet.
    Notes,
}

impl StateKey {
    /// Nom stable, pour la base (`strategyDraw:1`, `text:3:0`…).
    pub fn name(&self) -> String {
        match self {
            StateKey::StrategyDraw(n) => format!("strategyDraw:{n}"),
            StateKey::Strategy => "strategy".into(),
            StateKey::Framing => "framing".into(),
            StateKey::SpecAttempt(n) => format!("specAttempt:{n}"),
            StateKey::Spec => "spec".into(),
            StateKey::PlanAttempt { round, attempt } => format!("planAttempt:{round}:{attempt}"),
            StateKey::PlanJudge { round, attempt } => format!("planJudge:{round}:{attempt}"),
            StateKey::PlanRound => "planRound".into(),
            StateKey::Plan => "plan".into(),
            StateKey::Text { id, version } => format!("text:{id}:{version}"),
            StateKey::FormJudge { id, version } => format!("formJudge:{id}:{version}"),
            StateKey::FactJudge { id, version } => format!("factJudge:{id}:{version}"),
            StateKey::RefusalJudge { id, version } => format!("refusalJudge:{id}:{version}"),
            StateKey::Brick(id) => format!("brick:{id}"),
            StateKey::Card(id) => format!("card:{id}"),
            StateKey::Split(id) => format!("split:{id}"),
            StateKey::Surgery(id) => format!("surgery:{id}"),
            StateKey::Proofread(n) => format!("proofread:{n}"),
            StateKey::Contradictions => "contradictions".into(),
            StateKey::WholeForm => "wholeForm".into(),
            StateKey::Report => "report".into(),
            StateKey::Delivered => "delivered".into(),
            StateKey::Notes => "notes".into(),
        }
    }
}

/// Comment le livrable a été produit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Route {
    /// D'un jet (une requête), tenu aux vérifications.
    Direct,
    /// Par parties, selon un plan.
    Parts,
}

/// Une partie dans le rapport.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrickReport {
    pub id: u32,
    pub title: String,
    pub state: BrickState,
    /// Une correction a été tentée.
    pub fixed: bool,
    pub words: u32,
    /// Vérifications de la version gardée.
    pub checks: Vec<Check>,
}

/// Rapport de vérification (sauvé sous [`StateKey::Report`] ; la page le
/// met en forme et le traduit).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub strategy: Strategy,
    pub route: Route,
    /// Livrable, relatif au dossier du projet.
    pub file: String,
    pub words: u32,
    pub target_words: Option<u32>,
    /// Vérifications d'ensemble (critères, longueur, mots internes,
    /// contradictions).
    pub checks: Vec<Check>,
    /// Les critères n'ont été jugés que sur le squelette (titres et cartes) :
    /// livrable trop long pour un seul jugement.
    pub judged_on_outline: bool,
    pub bricks: Vec<BrickReport>,
    /// Ce qui n'a PAS été vérifié (codes : `factJudge`, `formJudge` si un
    /// juge n'a pas pu répondre, `sources`, `documents` si des pièces jointes
    /// ont été coupées…).
    pub not_checked: Vec<String>,
    /// Avertissements du projet (codes : `lengthCapped` longueur demandée
    /// bornée, `specBestOfThree` spécification gardée malgré ses défauts,
    /// `specDefect.<code>`).
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Affirmations jugées douteuses (simplification, pas universelle,
    /// dépend du contexte) : jamais bloquantes, listées au rapport.
    #[serde(default)]
    pub doubtful: u32,
    /// Points « à revoir » : échecs qui comptent restés après les
    /// corrections (affirmation fausse, critère non tenu, longueur).
    #[serde(default)]
    pub to_review: u32,
    /// Relecture de la langue : passages relus.
    #[serde(default)]
    pub proofread_blocks: u32,
    /// Passages corrigés par la relecture.
    #[serde(default)]
    pub proofread_changed: u32,
    /// Corrections de la relecture écartées par le garde-fou (le passage
    /// garde alors son texte).
    #[serde(default)]
    pub proofread_rejected: u32,
}
