//! Types du ModelHub échangés avec l'interface (exportés par ts-rs dans
//! `src/bindings`). Contrat de l'étape 15a : `.tools/research/37-step15a-contract.md`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Usage proposé en filtre, déduit du dépôt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HubUse {
    Code,
    Vision,
    Reasoning,
    Tools,
}

/// Taille, en filtre.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HubSize {
    /// Tient entièrement dans la machine (`HubFit::Fits`) ; sans effet si la
    /// machine est inconnue (serveur distant).
    Fits,
    /// Moins de 4 milliards de paramètres.
    Small,
    /// De 4 à 15 milliards.
    Medium,
    /// Plus de 15 milliards.
    Large,
}

/// Filtres de l'accueil, d'une rubrique, d'une famille et de la recherche.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubFilters {
    /// Usages exigés, tous à la fois.
    pub uses: Vec<HubUse>,
    pub size: Option<HubSize>,
    /// Licence libre seulement (apache-2.0, mit, bsd-*...).
    pub open_license: bool,
    /// Publieurs de confiance seulement (utile à la recherche ; les autres
    /// vues n'en montrent pas d'autres).
    pub verified_only: bool,
}

/// Capacités annoncées par le dépôt ; les vraies se lisent dans `/api/show`
/// après installation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HubCapabilities {
    pub tools: bool,
    pub thinking: bool,
    pub vision: bool,
    pub code: bool,
}

/// Origine d'un dépôt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HubTrust {
    /// Laboratoire d'origine (Qwen, google...).
    Lab,
    /// Quantificateur de confiance (unsloth, bartowski...).
    Quantizer,
    /// Hors de la liste de confiance (recherche libre seulement).
    Unverified,
}

/// Place d'un modèle dans la machine (serveur local seulement).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HubFit {
    /// Tient dans la VRAM (sans GPU : dans la RAM).
    Fits,
    /// Déborde de la VRAM sur la RAM : plus lent.
    Partial,
    /// Trop gros pour cette machine.
    TooLarge,
}

/// Dépôt GGUF résumé (cartes, listes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubModel {
    /// « unsloth/Qwen3-8B-GGUF ».
    pub id: String,
    /// « unsloth ».
    pub author: String,
    /// Nom affiché : le dépôt sans son suffixe GGUF (« Qwen3-8B »).
    pub name: String,
    /// Famille (« qwen »), voir `HubFamily`.
    pub family: Option<String>,
    /// Modèle de base (« Qwen/Qwen3-8B »).
    pub base_model: Option<String>,
    pub trust: HubTrust,
    /// Nombre de paramètres.
    pub parameters: Option<u64>,
    /// `general.architecture` (« qwen3 »).
    pub architecture: Option<String>,
    /// Contexte maximal du modèle.
    pub context_length: Option<u32>,
    pub capabilities: HubCapabilities,
    /// Identifiant de licence (« apache-2.0 »).
    pub license: Option<String>,
    /// Téléchargements des 30 derniers jours.
    pub downloads: u64,
    pub likes: u64,
    /// Date ISO 8601.
    pub last_modified: Option<String>,
    /// Taille à télécharger de la version recommandée (octets, projecteur
    /// compris) : estimée dans les listes, exacte sur la fiche.
    pub estimated_size: Option<u64>,
    /// Mémoire nécessaire à cette version pour le contexte de la machine
    /// (poids, cache de contexte, marge) ; `None` : machine ou taille
    /// inconnue.
    pub estimated_memory: Option<u64>,
    /// `None` : machine inconnue (serveur distant) ou taille inconnue.
    pub fit: Option<HubFit>,
}

/// Rubrique de l'accueil.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HubSectionId {
    /// Les plus gros modèles qui tiennent dans la machine (serveur local).
    ForYou,
    Trending,
    Popular,
    Coding,
    Vision,
    Reasoning,
    /// Moins de 4 milliards de paramètres.
    Small,
    /// Dérivés sans refus d'un modèle officiel (listes à part, réglage
    /// `show_uncensored`) ; en dernier.
    Uncensored,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubSection {
    pub id: HubSectionId,
    /// Six au plus ; la liste complète par `hub_browse`.
    pub models: Vec<HubModel>,
}

/// Famille de modèles (tuile de l'accueil).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubFamily {
    /// « qwen ».
    pub id: String,
    /// « Qwen ».
    pub label: String,
    /// Dépôts de la famille dans le catalogue (après filtres).
    pub count: u32,
}

/// Pourquoi les données sont absentes ou anciennes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum HubProblem {
    /// Pas de connexion (aucune réponse : DNS, connexion refusée, TLS).
    Offline,
    /// Hugging Face ne répond pas (5xx, page HTML, délai dépassé) ;
    /// `incident` : sa page d'état signale un incident.
    Unavailable { incident: bool },
    /// Trop de requêtes (429) : nouvel essai possible dans `retry_after` s.
    #[serde(rename_all = "camelCase")]
    RateLimited { retry_after: u32 },
    /// Dépôt inexistant ou privé.
    NotFound,
    /// Dépôt à accès restreint (compte Hugging Face nécessaire).
    Restricted,
    /// Autre échec, message montré tel quel.
    Failed { message: String },
}

/// Fraîcheur des données montrées.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubStatus {
    /// Date de la réponse de Hugging Face montrée (ms depuis 1970) ; `None` :
    /// aucune donnée.
    pub fetched_at: Option<i64>,
    /// `None` : données à jour.
    pub problem: Option<HubProblem>,
}

/// Accueil du ModelHub.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubHome {
    /// Dans l'ordre d'affichage ; rubriques vides omises.
    pub sections: Vec<HubSection>,
    /// Par téléchargements décroissants ; familles vides omises.
    pub families: Vec<HubFamily>,
    pub status: HubStatus,
}

/// Ce que montre une liste complète.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum HubScope {
    Section { id: HubSectionId },
    Family { id: String },
}

/// Recherche libre sur Hugging Face.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubSearch {
    pub text: String,
    pub filters: HubFilters,
    /// Curseur de la page suivante (`next_cursor` d'une réponse précédente).
    pub cursor: Option<String>,
}

/// Liste de dépôts (rubrique, famille ou recherche).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubList {
    pub models: Vec<HubModel>,
    /// Page suivante (recherche seulement).
    pub next_cursor: Option<String>,
    pub status: HubStatus,
}

/// Qualité d'une quantification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HubQuantTier {
    /// 16 bits ou plus (F16, BF16, F32).
    Full,
    /// 6 à 8 bits.
    High,
    /// 4 à 5 bits.
    Balanced,
    /// 3 bits.
    Compact,
    /// 2 bits ou moins.
    Tiny,
}

/// Pourquoi une version ne peut pas être téléchargée.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HubQuantIssue {
    /// Découpée en plusieurs fichiers (refusé par le registre de Hugging Face).
    Sharded,
    /// Dans un sous-dossier du dépôt (aucune étiquette ne la désigne).
    Subfolder,
}

/// Version (quantification) d'un dépôt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubQuant {
    /// « Q4_K_M », « UD-Q4_K_XL », sinon le nom du fichier.
    pub label: String,
    /// Fichier (premier morceau pour une version découpée).
    pub file: String,
    /// Nom Ollama complet (« hf.co/unsloth/Qwen3-8B-GGUF:Q4_K_M ») ; `None` :
    /// non téléchargeable (`issue`).
    pub model: Option<String>,
    pub issue: Option<HubQuantIssue>,
    /// Taille du ou des fichiers du modèle, en octets.
    pub size: u64,
    /// Taille à télécharger, projecteur de vision compris (estimée).
    pub download_size: u64,
    pub tier: HubQuantTier,
    pub fit: Option<HubFit>,
    /// Mémoire estimée au chargement (poids, cache de contexte, marge).
    pub memory: Option<u64>,
    /// Version conseillée pour cette machine (une seule par dépôt).
    pub recommended: bool,
}

/// Fiche d'un dépôt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubRepo {
    pub model: HubModel,
    /// Commit du dépôt (révision du README).
    pub sha: Option<String>,
    /// Accès restreint : rien n'est téléchargeable.
    pub gated: bool,
    pub created_at: Option<String>,
    /// Codes de langue (« en », « fr »).
    pub languages: Vec<String>,
    /// Du plus gros au plus petit.
    pub quants: Vec<HubQuant>,
    /// Contexte retenu pour les estimations de mémoire.
    pub context_used: Option<u32>,
    /// Autres dépôts du même modèle de base (publieurs de confiance).
    pub other_sources: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubRepoResult {
    /// `None` : aucune donnée (voir `status.problem`).
    pub repo: Option<HubRepo>,
    pub status: HubStatus,
}

/// README d'un dépôt (Markdown sans l'en-tête YAML).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubReadme {
    /// `None` : pas de README, ou illisible (voir `problem`).
    pub text: Option<String>,
    pub problem: Option<HubProblem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubGpu {
    pub name: String,
    /// Octets.
    pub vram_total: u64,
    pub vram_free: Option<u64>,
}

/// Mémoire de la machine du serveur (locale seulement).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubHardware {
    /// Vide : pas de GPU reconnu (calcul sur le processeur).
    pub gpus: Vec<HubGpu>,
    pub ram_total: u64,
    pub ram_available: u64,
    /// Contexte retenu pour les estimations : réglage de ChatonLM, sinon celui
    /// qu'Ollama choisirait d'après la VRAM.
    pub context: u32,
}

/// Téléchargement demandé depuis le ModelHub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubDownloadRequest {
    /// Nom Ollama complet.
    pub model: String,
    /// Dépôt Hugging Face d'origine.
    pub repo: Option<String>,
    /// Version (« Q4_K_M »).
    pub label: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HubDownloadState {
    /// En file : deux téléchargements à la fois au plus.
    Queued,
    /// Vérification du manifeste, début du pull.
    Starting,
    Downloading,
    /// Octets reçus ; Ollama vérifie et écrit le manifeste.
    Finishing,
    /// En pause : requête fermée, morceaux reçus gardés par Ollama, reprise à
    /// la demande.
    Paused,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubDownload {
    pub model: String,
    pub repo: Option<String>,
    pub label: Option<String>,
    pub state: HubDownloadState,
    /// Octets reçus, toutes couches.
    pub completed: u64,
    /// Octets à recevoir ; 0 : inconnu.
    pub total: u64,
    /// Débit lissé.
    pub bytes_per_second: Option<u64>,
    /// Message lisible d'un échec.
    pub error: Option<String>,
    /// ms depuis 1970.
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

/// Modèle installé sur le serveur (onglet Installed).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HubInstalled {
    /// Nom Ollama, tel que `/api/tags` le donne.
    pub model: String,
    /// Octets sur le disque.
    pub size: u64,
    /// Date ISO 8601.
    pub modified_at: Option<String>,
    pub parameter_size: String,
    pub quantization_level: String,
    pub family: String,
    pub capabilities: Vec<String>,
    /// Dépôt Hugging Face d'un nom `hf.co/<auteur>/<dépôt>:<étiquette>`.
    pub repo: Option<String>,
}
