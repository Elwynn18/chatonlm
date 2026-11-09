//! Reprise des modèles des Ollama de ChatonLM 0.9.x (lot L6, contrat
//! `.tools/research/101-llamacpp-L3-L6-R1-contract.md` § 2) :
//!
//! - **Ollama intégré** (`<données>/ollama/models` ou le dossier réglé dans
//!   `builtin_ollama.models_dir`) : au premier lancement, ses modèles passent
//!   dans le rangement de la base locale (contrat 100 § 1), par liens
//!   physiques sur le même système de fichiers (ses blobs restent jusqu'au
//!   ménage : une coupure ne perd rien, un blob cité par un modèle non repris
//!   reste), sinon par copie vérifiée. Si son dossier est aussi celui de
//!   l'Ollama de la machine (0.9.x acceptait `~/.ollama/models`), il est
//!   traité comme ce dernier : copies seulement, jamais de ménage ;
//! - **Ollama de la machine** (lecture seule, copie) : modèles listés avec
//!   leur taille, copiés sur demande (progression, annulation, place).
//!
//! Garanties (relecture `.tools/research/101-relecture-L6.md`) : un fichier
//! déjà à sa place n'est jamais écrasé (identifiant de repli
//! `@ollama/<hôte>/<espace>/<modèle>:<étiquette>` quand deux noms prétendent
//! au même) ; chaque modèle est écrit en base avec la correspondance de ses
//! noms dès qu'il est en place ; l'intégré n'est « repris » qu'une fois
//! chaque modèle prêt repris ou abandonné (deux échecs de suite) ; seules les
//! architectures que charge le llama-server livré sont reprises.
//!
//! Pour chaque modèle : manifeste lu ([`manifest`], variante `llamacpp`
//! d'une liste), en-tête GGUF jugé ([`compat`] : les formats propres à
//! Ollama sont marqués « à retélécharger », avec un équivalent sur Hugging
//! Face quand il est connu), identifiant de la base locale ([`names`]),
//! fichiers mis en place ([`transfer`]), ligne `local_models`, réglages
//! d'Ollama gardés (`model_defaults`), table de correspondance
//! (`ollama_imports`) puis réglages de l'app réécrits
//! (`store::model_import::apply_renames`). Les anciens fichiers de l'intégré
//! ne sont effacés qu'après confirmation ([`cleanup`]). Changement du dossier
//! des modèles : [`folder`].

pub mod cleanup;
pub mod compat;
pub mod folder;
pub mod manifest;
pub mod names;
pub mod scan;
pub mod service;
pub mod sources;
pub mod transfer;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod os;
#[cfg(windows)]
#[path = "windows.rs"]
mod os;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use self::scan::{Found, Verdict};
use self::transfer::{Mode, Placed, Step, TransferError};
use crate::error::{AppError, AppResult};
use crate::hub::gguf::meta;
use crate::i18n::t;
use crate::local_models::id::{ModelId, same_model};
use crate::local_models::{ModelCapabilities, ModelSource};
use crate::store::Store;
use crate::store::local_models::{self as table, StoredLocalModel};
use crate::store::model_import::{self as records, ImportRecord, ModelDefaults, RecordStatus};

/// Place libre exigée en plus des fichiers copiés.
pub const SPACE_MARGIN: u64 = 100 * 1024 * 1024;

/// Intervalle des relevés de progression.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

/// Délai de la fiche Hugging Face d'un modèle `hf.co/…` repris.
const HF_TIMEOUT: Duration = Duration::from_secs(15);

/// Événement de l'état de la reprise (état complet à chaque changement).
pub const EVENT: &str = "model-import";

/// Ollama d'où viennent les modèles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ImportOrigin {
    /// Ollama intégré de ChatonLM 0.9.x : fichiers déplacés.
    Builtin,
    /// Ollama de la machine : fichiers copiés, jamais touchés chez lui.
    Machine,
}

/// Pourquoi un modèle n'est pas importé tel quel (code traduit par la page).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ImportProblem {
    /// Format propre au moteur d'Ollama (vision fondue dans les poids, clés
    /// manquantes) : llama.cpp ne le charge pas, à retélécharger.
    OllamaFormat,
    /// Les poids ne sont pas un fichier GGUF (safetensors, morceaux…).
    NotGguf,
    /// Un blob manque (téléchargement inachevé, effacé).
    BlobMissing,
    /// Modèle du cloud d'Ollama : rien sur le disque.
    Cloud,
    /// Adaptateur LoRA d'Ollama : non repris.
    Adapter,
    /// Nom impossible à ranger (caractère refusé, nom réservé de Windows).
    InvalidName,
    /// Manifeste ou blob illisible.
    Unreadable,
    /// Modèle d'embeddings (`nomic-embed-text`, `bert`…) : pas un modèle de
    /// conversation, non repris.
    Embedding,
    /// Architecture que le llama-server livré ne charge pas
    /// (llama3.2-vision « mllama »…), ou qui ne converse pas (voix).
    Unsupported,
}

/// Ce que l'import fera d'un modèle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CandidateStatus {
    /// Importable.
    Ready,
    /// Déjà dans la base locale.
    Installed,
    /// À retélécharger (voir `suggestion`).
    Redownload,
    /// Rien à importer (voir `problem`).
    Skipped,
    /// Reprise manquée deux fois de suite : plus reprise au lancement,
    /// n'empêche plus le ménage ; l'écran peut la retenter.
    Failed,
}

/// Modèle d'un Ollama, tel que l'écran de reprise le montre.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportCandidate {
    /// Nom d'Ollama (« qwen3:0.6b »).
    pub name: String,
    /// Autres noms des mêmes fichiers (« qwen3:latest »).
    pub aliases: Vec<String>,
    /// Identifiant dans la base locale (importable ou installé).
    pub id: Option<String>,
    /// Octets des poids et du projecteur.
    pub size_bytes: u64,
    pub status: CandidateStatus,
    pub problem: Option<ImportProblem>,
    /// Équivalent sur Hugging Face à télécharger (`ModelId`).
    pub suggestion: Option<String>,
    /// Projecteur de vision avec les poids.
    pub vision: bool,
    /// Les poids n'ont pas de gabarit de conversation : Ollama donnait le
    /// sien, llama-server en prendra un par défaut (à signaler).
    pub no_template: bool,
}

/// Dossier d'un Ollama et ses modèles.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportSource {
    pub origin: ImportOrigin,
    pub dir: String,
    /// Faux : le dossier existe mais ne se lit pas (droits).
    pub readable: bool,
    /// Même système de fichiers que le dossier des modèles : l'intégré est
    /// repris sans copie (liens physiques).
    pub same_drive: bool,
    /// Le dossier de l'intégré est aussi celui de l'Ollama de la machine :
    /// copies seulement, rien n'y sera effacé.
    pub shared: bool,
    /// Place libre dans le dossier des modèles.
    pub free_bytes: Option<u64>,
    pub models: Vec<ImportCandidate>,
}

/// Reprise en cours.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportProgress {
    pub origin: ImportOrigin,
    pub dir: String,
    /// Nom du modèle en cours.
    pub model: Option<String>,
    pub done_models: u32,
    pub total_models: u32,
    /// Octets copiés (un renommage n'en compte pas).
    pub done_bytes: u64,
    pub total_bytes: u64,
}

/// Modèle repris.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportedModel {
    pub name: String,
    pub id: String,
}

/// Modèle à retélécharger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RedownloadModel {
    pub name: String,
    pub suggestion: Option<String>,
}

/// Échec de la reprise d'un modèle (texte dans la langue du moment).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FailedModel {
    pub name: String,
    pub error: String,
}

/// Bilan d'une reprise.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportReport {
    pub origin: ImportOrigin,
    pub dir: String,
    pub imported: Vec<ImportedModel>,
    pub redownload: Vec<RedownloadModel>,
    pub failed: Vec<FailedModel>,
    pub cancelled: bool,
    /// Échec d'ensemble (place, dossier illisible), avant tout fichier.
    pub error: Option<String>,
}

/// État de la reprise, envoyé à la page (événement [`EVENT`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportState {
    pub running: Option<ImportProgress>,
    pub last: Option<ImportReport>,
}

/// Réglages d'un modèle repris de sa couche `params` d'Ollama (JSON), pour
/// les requêtes au moteur (lot L5) ; les clés inconnues sont laissées.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OllamaParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_last_n: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// Contexte voulu (`num_ctx`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_ctx: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<i64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
}

/// Arrêts gardés au plus, et leur longueur maximale.
const MAX_STOPS: usize = 16;
const MAX_STOP_LEN: usize = 256;

impl OllamaParams {
    /// Couche `params` d'Ollama ; une valeur d'un type inattendu est ignorée
    /// (pas tout l'objet), un arrêt seul (`"stop": "x"`) est accepté.
    pub fn from_json(bytes: &[u8]) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        let object = value.as_object()?;
        let float = |key: &str| object.get(key).and_then(serde_json::Value::as_f64);
        let int = |key: &str| object.get(key).and_then(serde_json::Value::as_i64);
        let stop = match object.get("stop") {
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect(),
            Some(serde_json::Value::String(text)) => vec![text.clone()],
            _ => Vec::new(),
        };
        Some(Self {
            temperature: float("temperature"),
            top_k: int("top_k"),
            top_p: float("top_p"),
            min_p: float("min_p"),
            repeat_penalty: float("repeat_penalty"),
            repeat_last_n: int("repeat_last_n"),
            presence_penalty: float("presence_penalty"),
            frequency_penalty: float("frequency_penalty"),
            seed: int("seed"),
            num_ctx: int("num_ctx").and_then(|n| u32::try_from(n).ok()),
            num_predict: int("num_predict"),
            stop: stop
                .into_iter()
                .filter(|stop| !stop.is_empty() && stop.len() <= MAX_STOP_LEN)
                .take(MAX_STOPS)
                .collect(),
        })
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Même volume Windows (lettre ou partage), logique pure essayée sous Linux.
pub fn same_volume(a: &str, b: &str) -> bool {
    use crate::ollama::models_dir::rules::volume;
    volume(a).is_some() && volume(a) == volume(b)
}

/// Même système de fichiers (renommage possible).
pub fn same_filesystem(a: &Path, b: &Path) -> bool {
    os::same_filesystem(a, b)
}

/// Dossier des modèles de l'Ollama intégré : celui réglé
/// (`builtin_ollama.models_dir`), sinon `<données>/ollama/models` (comme
/// `ollama::builtin::Config::default_models_dir`).
pub fn builtin_models_dir(
    connection: &rusqlite::Connection,
    data_dir: &Path,
) -> AppResult<PathBuf> {
    Ok(crate::store::builtin_ollama::get(connection)?
        .models_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("ollama").join("models")))
}

/// Fichiers d'un modèle installé présents à leur place.
fn files_present(models_dir: &Path, model: &StoredLocalModel) -> bool {
    model.all_files().all(|relative| {
        crate::local_models::absolute(models_dir, relative).is_some_and(|path| path.is_file())
    })
}

/// Le modèle installé a les poids de `found` (empreintes de la base ; sans
/// empreinte connue, on le croit).
fn same_weights(model: &StoredLocalModel, found: &Found) -> bool {
    model.sha256.is_empty()
        || found
            .weights
            .as_ref()
            .is_some_and(|weights| model.sha256.values().any(|sha| *sha == weights.digest))
}

/// Identifiant installé (fichiers présents) sous lequel un des noms de
/// `found` a déjà été repris, ou qui est son identifiant cible, avec les
/// mêmes poids dans les deux cas (un autre contenu sous le même nom ou le même
/// identifiant n'est pas « installé » : relecture L6, I3 et sa note sur un
/// nom de la machine déjà repris de l'intégré).
fn installed_id(found: &Found, known: &Known) -> Option<String> {
    let present = |id: &str| {
        known
            .installed
            .iter()
            .find(|model| same_model(&model.id, id))
            .filter(|model| files_present(&known.models_dir, model))
            .filter(|model| same_weights(model, found))
            .map(|model| model.id.clone())
    };
    found
        .all_names()
        .find_map(|name| known.imported.get(&name.key()).and_then(|id| present(id)))
        .or_else(|| match &found.verdict {
            Verdict::Ready { id, .. } => present(&id.to_string()),
            // Blob déjà sorti de `blobs/` (renommage sans liens, coupure
            // avant la correspondance) : retrouvé par son empreinte dans la
            // base.
            Verdict::Skip(ImportProblem::BlobMissing) => {
                let weights = found.weights.as_ref()?;
                known
                    .installed
                    .iter()
                    .find(|model| {
                        model.sha256.values().any(|sha| *sha == weights.digest)
                            && files_present(&known.models_dir, model)
                    })
                    .map(|model| model.id.clone())
            }
            _ => None,
        })
}

/// Modèle de la page d'après ce que le dossier contient et la base sait.
fn candidate(found: &Found, known: &Known) -> ImportCandidate {
    let mut candidate = ImportCandidate {
        name: found.name.ollama_name(),
        aliases: found
            .aliases
            .iter()
            .map(names::ManifestName::ollama_name)
            .collect(),
        id: None,
        size_bytes: found.size(),
        status: CandidateStatus::Skipped,
        problem: None,
        suggestion: None,
        vision: found.projector.is_some(),
        no_template: false,
    };
    if let Some(id) = installed_id(found, known) {
        candidate.id = Some(id);
        candidate.status = CandidateStatus::Installed;
        return candidate;
    }
    match &found.verdict {
        Verdict::Ready { id, .. } => {
            candidate.id = Some(id.to_string());
            candidate.no_template = found.no_template;
            candidate.status = if found
                .all_names()
                .any(|name| known.given_up.contains(&name.key()))
            {
                CandidateStatus::Failed
            } else {
                CandidateStatus::Ready
            };
        }
        Verdict::Redownload {
            problem,
            suggestion,
        } => {
            candidate.status = CandidateStatus::Redownload;
            candidate.problem = Some(*problem);
            candidate.suggestion.clone_from(suggestion);
        }
        Verdict::Skip(problem) => candidate.problem = Some(*problem),
    }
    candidate
}

/// Ce que la base sait : modèles installés, noms déjà repris (clé →
/// identifiant), noms abandonnés après deux échecs (de cette origine),
/// dossier des modèles, dossier de l'intégré vu partagé.
pub(crate) struct Known {
    installed: Vec<StoredLocalModel>,
    imported: HashMap<String, String>,
    given_up: HashSet<String>,
    models_dir: PathBuf,
    builtin_shared: bool,
}

fn known(
    connection: &rusqlite::Connection,
    data_dir: &Path,
    origin: ImportOrigin,
) -> AppResult<Known> {
    let mut imported = HashMap::new();
    let mut given_up = HashSet::new();
    for record in records::records(connection)? {
        if record.origin == origin && record.given_up() {
            given_up.insert(names::ollama_key(&record.name));
        }
        if record.status == RecordStatus::Imported
            && let Some(id) = record.model_id
        {
            imported.insert(names::ollama_key(&record.name), id);
        }
    }
    Ok(Known {
        installed: table::list(connection)?,
        imported,
        given_up,
        models_dir: crate::local_models::models_dir_in(connection, data_dir)?,
        builtin_shared: records::builtin_shared(connection)?,
    })
}

async fn known_now(store: &Store, data_dir: &Path, origin: ImportOrigin) -> AppResult<Known> {
    let data_dir = data_dir.to_owned();
    store.call(move |c| known(c, &data_dir, origin)).await
}

/// Modèles du dossier encore à reprendre (`Ready`) d'après la base :
/// la reprise de l'intégré n'est faite, et son ménage permis, que sans eux
/// (relecture L6, B2, I1 et I8).
fn still_ready(found: &[Found], known: &Known) -> Vec<String> {
    found
        .iter()
        .map(|found| candidate(found, known))
        .filter(|candidate| candidate.status == CandidateStatus::Ready)
        .map(|candidate| candidate.name)
        .collect()
}

/// Ce que le dossier `dir` de l'intégré dit pour son ménage : modèles qui
/// disparaîtront (tout ce qui n'est pas installé), ou `None` s'il ne se lit
/// pas ou qu'il reste un modèle à reprendre.
pub(crate) async fn builtin_left(
    store: &Store,
    data_dir: &Path,
    dir: &Path,
) -> AppResult<Option<Vec<ImportCandidate>>> {
    let known = known_now(store, data_dir, ImportOrigin::Builtin).await?;
    let dir = dir.to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let found = scan::scan(&dir).ok()?;
        still_ready(&found, &known).is_empty().then(|| {
            found
                .iter()
                .map(|found| candidate(found, &known))
                .filter(|candidate| candidate.status != CandidateStatus::Installed)
                .collect()
        })
    })
    .await
    .map_err(|error| AppError::Internal(error.to_string()))
}

/// Lit le dossier `dir` d'un Ollama : ses modèles, et ce que l'import en
/// fera d'après la base ; `shared` : dossier de l'intégré qui est aussi celui
/// de l'Ollama de la machine (copies seulement).
pub async fn source(
    store: &Store,
    data_dir: &Path,
    origin: ImportOrigin,
    dir: &Path,
    shared: bool,
) -> AppResult<ImportSource> {
    let known = known_now(store, data_dir, origin).await?;
    let dir = dir.to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let scanned = scan::scan(&dir);
        let readable = scanned.is_ok() || !dir.exists();
        let models = scanned
            .unwrap_or_default()
            .iter()
            .map(|found| candidate(found, &known))
            .collect();
        let shared = origin == ImportOrigin::Builtin && (shared || known.builtin_shared);
        ImportSource {
            origin,
            dir: dir.to_string_lossy().into_owned(),
            readable,
            same_drive: origin == ImportOrigin::Builtin
                && !shared
                && same_filesystem(&dir, &known.models_dir),
            shared,
            free_bytes: crate::disk::free_space_for(&known.models_dir),
            models,
        }
    })
    .await
    .map_err(|error| AppError::Internal(error.to_string()))
}

/// Reprise demandée.
#[derive(Clone, Debug)]
pub struct Job {
    pub origin: ImportOrigin,
    pub dir: PathBuf,
    /// Noms à reprendre (un nom ou un autre nom du même modèle) ; `None` :
    /// tous les importables.
    pub names: Option<Vec<String>>,
    /// Intégré dont le dossier est aussi celui de l'Ollama de la machine
    /// (`service::builtin_shared`) : copies seulement, jamais de lien ni de
    /// renommage, ni de ménage, ni de reprise du dossier (relecture L6, B1).
    pub shared: bool,
    /// Retenter aussi les modèles abandonnés après deux échecs (demande de
    /// l'écran ; jamais au lancement).
    pub retry_failed: bool,
}

/// De quoi faire une reprise.
#[derive(Clone)]
pub struct Context {
    pub store: Store,
    pub data_dir: PathBuf,
    /// Adresse de Hugging Face (fiches des modèles `hf.co/…`).
    pub hf_url: String,
    pub cancel: Arc<AtomicBool>,
}

/// Fichier d'un modèle repris : chemin relatif dans le dossier des modèles.
struct Planned {
    layer: manifest::Layer,
    relative: String,
}

/// Où ranger un modèle repris, et ce que la fiche de Hugging Face en dit.
struct Placement {
    id: ModelId,
    source: ModelSource,
    repo: String,
    revision: Option<String>,
    weights: Planned,
    projector: Option<Planned>,
}

/// Nom de dépôt sans suffixe `-GGUF` (fichiers rangés sans fiche).
fn repo_stem(repo: &str) -> &str {
    let len = repo.len();
    if len > 5 && repo[len - 5..].eq_ignore_ascii_case("-gguf") {
        &repo[..len - 5]
    } else {
        repo
    }
}

/// Fichier des poids dans la fiche d'un dépôt : celui aux mêmes octets
/// (`digest`) ; s'il y en a plusieurs (chez bartowski, Q4_K_L et Q4_K_M de
/// SmolLM2-135M sont le même fichier), la version que nomme Ollama
/// (`quant`), sinon la première. Rend son chemin et l'identifiant de sa
/// version (`fallback` si l'inventaire n'en donne pas).
fn hf_weights(
    listing: &crate::hub::files::Listing,
    repo_id: &str,
    digest: &str,
    quant: &str,
    fallback: &ModelId,
) -> Option<(String, ModelId)> {
    let entries: Vec<crate::hub::quant::FileEntry> = listing
        .files
        .iter()
        .map(|file| crate::hub::quant::FileEntry {
            path: &file.path,
            size: file.size,
        })
        .collect();
    let inventory = crate::hub::quant::inventory(repo_id, &entries);
    let candidates: Vec<(&str, Option<ModelId>)> = listing
        .files
        .iter()
        .filter(|file| file.sha256.as_deref() == Some(digest))
        .map(|file| {
            let model = inventory
                .iter()
                .find(|version| version.files.first() == Some(&file.path))
                .and_then(|version| version.model.as_deref())
                .and_then(ModelId::parse);
            (file.path.as_str(), model)
        })
        .collect();
    let named = |model: &Option<ModelId>| matches!(model, Some(ModelId::Hf { quant: found, .. }) if found.eq_ignore_ascii_case(quant));
    let (path, model) = candidates
        .iter()
        .find(|(_, model)| named(model))
        .or_else(|| candidates.first())?;
    Some((
        (*path).to_owned(),
        model.clone().unwrap_or_else(|| fallback.clone()),
    ))
}

/// Rangement d'un modèle (contrat 100 § 1) : Hugging Face retrouvé par
/// l'empreinte des poids dans la fiche du dépôt (fichiers d'origine,
/// révision suivie, identifiant de la version comme le ModelHub l'écrit) ;
/// sinon sans suivi (`source` « ollama ») sous des noms construits ;
/// importé de la bibliothèque : `@ollama/<nom>/<étiquette>/model.gguf`.
/// `listings` : fiches déjà lues, par dépôt (`None` : injoignable).
fn placement_for(
    found: &Found,
    id: &ModelId,
    listings: &HashMap<String, Option<crate::hub::files::Listing>>,
) -> Option<Placement> {
    let weights = found.weights.clone()?;
    let projector = found.projector.clone();
    match id {
        ModelId::Hf {
            author,
            repo,
            quant,
        } => {
            let repo_id = format!("{author}/{repo}");
            let listing = listings.get(&repo_id).and_then(Option::as_ref);
            let find = |digest: &str| {
                listing.as_ref().and_then(|listing| {
                    listing
                        .files
                        .iter()
                        .find(|file| file.sha256.as_deref() == Some(digest))
                        .map(|file| file.path.clone())
                })
            };
            if let Some(listing) = listing
                && let Some((path, found_id)) =
                    hf_weights(listing, &repo_id, &weights.digest, quant, id)
            {
                let projector = projector.map(|layer| Planned {
                    relative: format!(
                        "{repo_id}/{}",
                        find(&layer.digest).unwrap_or_else(|| format!("mmproj-{quant}.gguf"))
                    ),
                    layer,
                });
                return Some(Placement {
                    id: found_id,
                    source: ModelSource::Hf,
                    repo: repo_id.clone(),
                    revision: Some(listing.sha.clone()),
                    weights: Planned {
                        relative: format!("{repo_id}/{path}"),
                        layer: weights,
                    },
                    projector,
                });
            }
            Some(Placement {
                id: id.clone(),
                source: ModelSource::Ollama,
                weights: Planned {
                    relative: format!("{repo_id}/{}-{quant}.gguf", repo_stem(repo)),
                    layer: weights,
                },
                projector: projector.map(|layer| Planned {
                    relative: format!("{repo_id}/mmproj-{quant}.gguf"),
                    layer,
                }),
                repo: repo_id,
                revision: None,
            })
        }
        ModelId::Ollama { name, tag } => {
            let folder = format!("{}{name}/{tag}", crate::local_models::id::OLLAMA_PREFIX);
            Some(Placement {
                id: id.clone(),
                source: ModelSource::Ollama,
                repo: found.name.ollama_name(),
                revision: None,
                weights: Planned {
                    relative: format!("{folder}/model.gguf"),
                    layer: weights,
                },
                projector: projector.map(|layer| Planned {
                    relative: format!("{folder}/mmproj.gguf"),
                    layer,
                }),
            })
        }
    }
}

/// Texte d'un échec de mise en place.
fn transfer_text(error: &TransferError) -> String {
    match error {
        TransferError::Cancelled => t!("modelImport.cancelled"),
        TransferError::Checksum(path) => {
            t!("modelImport.checksum", file = path.to_string_lossy())
        }
        TransferError::Exists(path) => {
            t!("modelImport.fileExists", file = path.to_string_lossy())
        }
        TransferError::NoSpace { needed, free } => t!(
            "modelImport.noSpace",
            needed = crate::image_models::downloads::size_text(*needed),
            free = crate::image_models::downloads::size_text(*free)
        ),
        TransferError::Io(path, error) => {
            t!(
                "modelImport.copyFailed",
                file = path.to_string_lossy(),
                error = error
            )
        }
        TransferError::Text(text) => text.clone(),
    }
}

/// Ligne `local_models` d'un modèle mis en place.
fn stored(
    placement: &Placement,
    models_dir: &Path,
    found: &Found,
) -> Result<StoredLocalModel, String> {
    let path = |relative: &str| {
        crate::local_models::absolute(models_dir, relative)
            .ok_or_else(|| t!("modelImport.invalidName", name = relative))
    };
    let weights = meta::read(&path(&placement.weights.relative)?).map_err(|error| match error {
        meta::MetaError::NotGguf => t!("modelImport.notGguf", name = found.name.ollama_name()),
        meta::MetaError::Io(error) => t!(
            "modelImport.copyFailed",
            file = placement.weights.relative,
            error = error
        ),
    })?;
    let projector = match &placement.projector {
        Some(projector) => meta::read(&path(&projector.relative)?).ok(),
        None => None,
    };
    let mut sha256 = std::collections::BTreeMap::new();
    for planned in std::iter::once(&placement.weights).chain(&placement.projector) {
        sha256.insert(planned.relative.clone(), planned.layer.digest.clone());
    }
    Ok(StoredLocalModel {
        id: placement.id.to_string(),
        source: placement.source,
        repo: placement.repo.clone(),
        revision: placement.revision.clone(),
        quant: placement.id.quant().to_owned(),
        files: vec![placement.weights.relative.clone()],
        mmproj: placement.projector.as_ref().map(|p| p.relative.clone()),
        size_bytes: found.size(),
        sha256,
        arch: weights.architecture.clone(),
        context_max: weights.context_length.and_then(|n| u32::try_from(n).ok()),
        params_label: weights.size_label.clone(),
        file_type: weights.file_type,
        capabilities: ModelCapabilities {
            // Un projecteur sans clé d'encodeur (anciens) lit les images.
            vision: placement.projector.is_some()
                && projector.as_ref().is_none_or(meta::ModelMeta::sees),
            audio: projector.as_ref().is_some_and(meta::ModelMeta::hears),
            tools: weights.tools,
            thinking: weights.thinking,
            fim: weights.fim,
            base: weights.base,
        },
        installed_at: crate::store::now_ms(),
    })
}

/// Avancement partagé entre la tâche et ses relevés.
struct Counters {
    done_models: AtomicU64,
    done_bytes: AtomicU64,
}

/// Clé d'un identifiant : quantification sans la casse (comme
/// [`same_model`]).
fn id_key(id: &str) -> String {
    match id.rsplit_once(':') {
        Some((path, quant)) => format!("{path}:{}", quant.to_ascii_lowercase()),
        None => id.to_owned(),
    }
}

/// Identifiant de repli d'un nom dont l'identifiant est déjà pris par un
/// autre contenu (relecture L6, I3) : `@ollama/<hôte>/<espace>/<modèle>:
/// <étiquette>`, propre à ce nom.
fn fallback_id(name: &names::ManifestName) -> Option<ModelId> {
    let host = if name.is_hf() {
        "hf.co"
    } else if name.is_ollama_registry() {
        "ollama.com"
    } else {
        name.host.as_str()
    };
    ModelId::parse(&format!(
        "{}{host}/{}/{}:{}",
        crate::local_models::id::OLLAMA_PREFIX,
        name.namespace,
        name.model,
        name.tag
    ))
    .filter(|id| matches!(id, ModelId::Ollama { .. }))
}

/// Ce qui fait qu'un modèle du lot est le même qu'un autre : fichiers et
/// réglages d'Ollama.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Signature {
    weights: Option<String>,
    projector: Option<String>,
    settings: Vec<Option<String>>,
}

impl Signature {
    fn of(found: &Found) -> Self {
        Self {
            weights: found.weights.as_ref().map(|layer| layer.digest.clone()),
            projector: found.projector.as_ref().map(|layer| layer.digest.clone()),
            settings: found.settings.clone(),
        }
    }
}

/// Rangement retenu pour un modèle du lot.
enum Resolved {
    /// À mettre en place sous ce rangement.
    Place(Box<Placement>),
    /// Mêmes fichiers et réglages qu'un modèle déjà repris dans ce lot : ses
    /// noms deviennent les siens.
    Alias(String),
}

/// Identifiant d'un modèle face à ceux déjà pris (`taken` : modèles de la
/// base, `None`, et de ce lot, avec leur signature) : libre, le même modèle
/// (alias), ou un autre contenu (identifiant de repli, jamais un fichier
/// écrasé).
fn resolve(
    found: &Found,
    placement: Option<Placement>,
    taken: &HashMap<String, Option<Signature>>,
    listings: &HashMap<String, Option<crate::hub::files::Listing>>,
) -> Result<Resolved, TransferError> {
    let invalid = || {
        TransferError::Text(t!(
            "modelImport.invalidName",
            name = found.name.ollama_name()
        ))
    };
    let placement = placement.ok_or_else(invalid)?;
    match taken.get(&id_key(&placement.id.to_string())) {
        None => Ok(Resolved::Place(Box::new(placement))),
        Some(Some(other)) if *other == Signature::of(found) => {
            Ok(Resolved::Alias(placement.id.to_string()))
        }
        Some(_) => {
            let fallback = fallback_id(&found.name)
                .filter(|id| !taken.contains_key(&id_key(&id.to_string())))
                .ok_or_else(|| {
                    TransferError::Text(t!(
                        "modelImport.sameId",
                        name = found.name.ollama_name(),
                        id = placement.id.to_string()
                    ))
                })?;
            placement_for(found, &fallback, listings)
                .map(|placement| Resolved::Place(Box::new(placement)))
                .ok_or_else(invalid)
        }
    }
}

/// Reprend les modèles de `job` ; `on_progress` reçoit l'avancement (au
/// plus toutes les [`PROGRESS_INTERVAL`]). Chaque modèle repris est écrit en
/// base (ligne, réglages, correspondance de ses noms) dès qu'il est en
/// place : une coupure ne perd que le modèle en cours, remis en place.
/// L'intégré n'est marqué repris que s'il ne reste aucun modèle à reprendre
/// dans son dossier (ni non choisi, ni en échec une seule fois).
pub async fn run(
    job: Job,
    context: Context,
    on_progress: impl Fn(ImportProgress) + Send + Sync,
) -> ImportReport {
    let dir_text = job.dir.to_string_lossy().into_owned();
    let mut report = ImportReport {
        origin: job.origin,
        dir: dir_text.clone(),
        imported: Vec::new(),
        redownload: Vec::new(),
        failed: Vec::new(),
        cancelled: false,
        error: None,
    };
    let Context {
        store,
        data_dir,
        hf_url,
        cancel,
    } = context;
    let save_error = |error: AppError| t!("modelImport.saveFailed", error = error);
    let builtin = job.origin == ImportOrigin::Builtin;
    // Intégré partagé avec la machine : noté pour toujours. Sinon, son
    // dossier déplacé devient celui des modèles (base locale vide).
    if builtin {
        let (dir, shared) = (dir_text.clone(), job.shared);
        let prepared = store
            .call(move |connection| {
                if shared {
                    records::set_builtin_shared(connection)?;
                }
                if !records::builtin_shared(connection)? {
                    records::adopt_builtin_folder(connection, &dir)?;
                }
                Ok(())
            })
            .await;
        if let Err(error) = prepared {
            report.error = Some(save_error(error));
            return report;
        }
    }
    let known = match known_now(&store, &data_dir, job.origin).await {
        Ok(known) => known,
        Err(error) => {
            report.error = Some(save_error(error));
            return report;
        }
    };
    let shared = builtin && (job.shared || known.builtin_shared);
    let scanned = {
        let dir = job.dir.clone();
        tauri::async_runtime::spawn_blocking(move || scan::scan(&dir)).await
    };
    let found = match scanned {
        Ok(Ok(found)) => found,
        Ok(Err(error)) => {
            report.error = Some(t!(
                "modelImport.unreadableFolder",
                dir = dir_text,
                error = error
            ));
            return report;
        }
        Err(error) => {
            report.error = Some(error.to_string());
            return report;
        }
    };
    let wanted = |found: &&Found| {
        job.names.as_ref().is_none_or(|names| {
            names.iter().any(|name| {
                let key = names::ollama_key(name);
                found.all_names().any(|own| own.key() == key)
            })
        })
    };
    let selected: Vec<(&Found, CandidateStatus)> = found
        .iter()
        .filter(wanted)
        .map(|found| {
            let status = match candidate(found, &known).status {
                CandidateStatus::Failed if job.retry_failed => CandidateStatus::Ready,
                status => status,
            };
            (found, status)
        })
        .collect();
    let models_dir = known.models_dir.clone();
    // Même disque, dossier propre à l'intégré : liens physiques (renommage
    // seulement sans liens). Machine, ou intégré partagé : copies.
    let link = builtin && !shared && same_filesystem(&job.dir, &models_dir);
    // Modèles du dossier qui citent chaque blob : un blob cité ailleurs
    // n'est jamais renommé (relecture L6, I5).
    let mut citations: HashMap<String, usize> = HashMap::new();
    for found in &found {
        for digest in found.digests() {
            *citations.entry(digest.to_owned()).or_default() += 1;
        }
    }
    let ready: Vec<&Found> = selected
        .iter()
        .filter(|(_, status)| *status == CandidateStatus::Ready)
        .map(|(found, _)| *found)
        .collect();
    // Octets à copier : blobs distincts (un blob partagé n'est copié qu'une
    // fois, puis lié). Liens : rien (un repli en copie vérifie la place).
    let mut digests: Vec<&manifest::Layer> = ready
        .iter()
        .flat_map(|found| found.weights.iter().chain(&found.projector))
        .collect();
    digests.sort_by(|a, b| a.digest.cmp(&b.digest));
    digests.dedup_by(|a, b| a.digest == b.digest);
    let total_bytes: u64 = if link {
        0
    } else {
        digests.iter().map(|layer| layer.size).sum()
    };
    if total_bytes > 0
        && let Some(free) = crate::disk::free_space_for(&models_dir)
        && total_bytes.saturating_add(SPACE_MARGIN) > free
    {
        report.error = Some(t!(
            "modelImport.noSpace",
            needed = crate::image_models::downloads::size_text(total_bytes + SPACE_MARGIN),
            free = crate::image_models::downloads::size_text(free)
        ));
        return report;
    }
    let counters = Arc::new(Counters {
        done_models: AtomicU64::new(0),
        done_bytes: AtomicU64::new(0),
    });
    let total_models = u32::try_from(ready.len()).unwrap_or(u32::MAX);
    let progress = |model: Option<String>| ImportProgress {
        origin: job.origin,
        dir: dir_text.clone(),
        model,
        done_models: u32::try_from(counters.done_models.load(Ordering::Relaxed)).unwrap_or(0),
        total_models,
        done_bytes: counters.done_bytes.load(Ordering::Relaxed),
        total_bytes,
    };
    on_progress(progress(None));
    let now = crate::store::now_ms;
    let all_names = |found: &Found| -> Vec<String> {
        found
            .all_names()
            .map(names::ManifestName::ollama_name)
            .collect()
    };
    let record = |name: String, id: Option<String>, status, suggestion, detail| ImportRecord {
        origin: job.origin,
        name,
        model_id: id,
        status,
        suggestion,
        detail,
        at: now(),
        attempts: 0,
    };
    // Déjà installés et à retélécharger : correspondance écrite à la fin.
    let mut late_records: Vec<ImportRecord> = Vec::new();
    for (found, status) in &selected {
        match status {
            CandidateStatus::Installed => {
                if let Some(id) = installed_id(found, &known) {
                    for name in all_names(found) {
                        late_records.push(record(
                            name,
                            Some(id.clone()),
                            RecordStatus::Imported,
                            None,
                            None,
                        ));
                    }
                }
            }
            CandidateStatus::Redownload => {
                let suggestion = match &found.verdict {
                    Verdict::Redownload { suggestion, .. } => suggestion.clone(),
                    _ => None,
                };
                for name in all_names(found) {
                    late_records.push(record(
                        name.clone(),
                        None,
                        RecordStatus::Redownload,
                        suggestion.clone(),
                        None,
                    ));
                    report.redownload.push(RedownloadModel {
                        name,
                        suggestion: suggestion.clone(),
                    });
                }
            }
            CandidateStatus::Skipped | CandidateStatus::Failed | CandidateStatus::Ready => {}
        }
    }
    // Fiches de Hugging Face, une par dépôt.
    let mut listings: HashMap<String, Option<crate::hub::files::Listing>> = HashMap::new();
    for found in &ready {
        if let Verdict::Ready {
            id: ModelId::Hf { author, repo, .. },
            ..
        } = &found.verdict
        {
            let repo_id = format!("{author}/{repo}");
            if !listings.contains_key(&repo_id) && !cancel.load(Ordering::Relaxed) {
                let listing = crate::hub::files::fetch(&hf_url, &repo_id, HF_TIMEOUT)
                    .await
                    .ok();
                listings.insert(repo_id, listing);
            }
        }
    }
    // Rangements ; ceux retrouvés sur Hugging Face d'abord : ils gardent
    // leur identifiant si un autre nom y prétend.
    let mut planned: Vec<(&Found, Option<Placement>)> = ready
        .iter()
        .map(|found| {
            let placement = match &found.verdict {
                Verdict::Ready { id, .. } => placement_for(found, id, &listings),
                _ => None,
            };
            (*found, placement)
        })
        .collect();
    planned.sort_by_key(|(_, placement)| {
        !placement
            .as_ref()
            .is_some_and(|placement| placement.source == ModelSource::Hf)
    });
    let mut taken: HashMap<String, Option<Signature>> = known
        .installed
        .iter()
        .map(|model| (id_key(&model.id), None))
        .collect();
    // Blobs déjà mis en place pendant ce lot (empreinte → chemin).
    let mut placed_blobs: HashMap<String, PathBuf> = HashMap::new();
    for (found, placement) in planned {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        let name = found.name.ollama_name();
        on_progress(progress(Some(name.clone())));
        let names = all_names(found);
        let outcome = match resolve(found, placement, &taken, &listings) {
            Ok(Resolved::Place(placement)) => {
                import_one(
                    found,
                    *placement,
                    &Batch {
                        origin: job.origin,
                        dir: &job.dir,
                        link,
                        citations: &citations,
                        models_dir: &models_dir,
                        store: &store,
                        cancel: &cancel,
                        counters: &counters,
                    },
                    &mut placed_blobs,
                    &|| on_progress(progress(Some(name.clone()))),
                )
                .await
            }
            Ok(Resolved::Alias(id)) => {
                let rows: Vec<ImportRecord> = names
                    .iter()
                    .map(|name| {
                        record(
                            name.clone(),
                            Some(id.clone()),
                            RecordStatus::Imported,
                            None,
                            None,
                        )
                    })
                    .collect();
                store
                    .call(move |connection| {
                        let transaction = connection.transaction()?;
                        for row in &rows {
                            records::record(&transaction, row)?;
                        }
                        transaction.commit()?;
                        Ok(())
                    })
                    .await
                    .map(|()| id)
                    .map_err(|error| TransferError::Text(save_error(error)))
            }
            Err(error) => Err(error),
        };
        counters.done_models.fetch_add(1, Ordering::Relaxed);
        match outcome {
            Ok(id) => {
                taken.insert(id_key(&id), Some(Signature::of(found)));
                for name in names {
                    report.imported.push(ImportedModel {
                        name,
                        id: id.clone(),
                    });
                }
            }
            Err(TransferError::Cancelled) => {
                report.cancelled = true;
                break;
            }
            Err(error) => {
                let text = transfer_text(&error);
                let rows: Vec<ImportRecord> = names
                    .iter()
                    .map(|name| {
                        record(
                            name.clone(),
                            None,
                            RecordStatus::Failed,
                            None,
                            Some(text.clone()),
                        )
                    })
                    .collect();
                let saved = store
                    .call(move |connection| {
                        for row in &rows {
                            records::record(connection, row)?;
                        }
                        Ok(())
                    })
                    .await;
                if let Err(error) = saved {
                    report.error = Some(save_error(error));
                }
                for name in names {
                    report.failed.push(FailedModel {
                        name,
                        error: text.clone(),
                    });
                }
            }
        }
    }
    let saved = store
        .call(move |connection| {
            for record in &late_records {
                records::record(connection, record)?;
            }
            records::apply_renames(connection)?;
            Ok(())
        })
        .await;
    if let Err(error) = saved {
        report.error = Some(save_error(error));
    }
    // L'intégré n'est repris que s'il ne reste rien à reprendre dans son
    // dossier (relecture L6, I1 et I8).
    if builtin && !report.cancelled && report.error.is_none() {
        let finished = match known_now(&store, &data_dir, job.origin).await {
            Ok(known) => still_ready(&found, &known).is_empty(),
            Err(_) => false,
        };
        if finished
            && let Err(error) = store
                .call(|connection| records::set_builtin_import_done(connection, true))
                .await
        {
            report.error = Some(save_error(error));
        }
    }
    on_progress(progress(None));
    report
}

/// Ce que partagent les modèles d'un lot.
struct Batch<'a> {
    origin: ImportOrigin,
    dir: &'a Path,
    /// Liens physiques (même disque, dossier propre à l'intégré).
    link: bool,
    citations: &'a HashMap<String, usize>,
    models_dir: &'a Path,
    store: &'a Store,
    cancel: &'a Arc<AtomicBool>,
    counters: &'a Arc<Counters>,
}

/// Reprend un modèle : fichiers mis en place, puis ligne, réglages et
/// correspondance de ses noms écrits ensemble (une transaction) ; rend son
/// identifiant. Un échec remet ses fichiers en place, et dit si tout n'a pas
/// pu l'être.
async fn import_one(
    found: &Found,
    placement: Placement,
    batch: &Batch<'_>,
    placed_blobs: &mut HashMap<String, PathBuf>,
    tick: &(dyn Fn() + Sync),
) -> Result<String, TransferError> {
    let models_dir = batch.models_dir;
    let mut steps = Vec::new();
    for planned in std::iter::once(&placement.weights).chain(&placement.projector) {
        let target =
            crate::local_models::absolute(models_dir, &planned.relative).ok_or_else(|| {
                TransferError::Text(t!("modelImport.invalidName", name = planned.relative))
            })?;
        let mode = match placed_blobs.get(&planned.layer.digest) {
            Some(from) => Mode::Link(from.clone()),
            None if batch.link => Mode::Move {
                rename_ok: batch
                    .citations
                    .get(&planned.layer.digest)
                    .is_none_or(|count| *count <= 1),
            },
            None => Mode::Copy,
        };
        steps.push(Step {
            source: scan::blob_path(batch.dir, &planned.layer.digest),
            target,
            size: planned.layer.size,
            sha256: planned.layer.digest.clone(),
            mode,
        });
    }
    let placed = {
        let (steps, cancel, counters) =
            (steps.clone(), batch.cancel.clone(), batch.counters.clone());
        let mut task = tauri::async_runtime::spawn_blocking(move || {
            let mut placed: Vec<Placed> = Vec::new();
            for step in &steps {
                match transfer::place(step, &cancel, &counters.done_bytes) {
                    Ok(item) => placed.push(item),
                    Err(error) => return Err((error, placed)),
                }
            }
            Ok(placed)
        });
        let mut interval = tokio::time::interval(PROGRESS_INTERVAL);
        loop {
            tokio::select! {
                result = &mut task => break result,
                _ = interval.tick() => tick(),
            }
        }
    };
    let undo = |placed: &[Placed]| {
        let restored = transfer::undo(placed, models_dir);
        forget_folders(&steps, models_dir);
        restored
    };
    let placed = match placed {
        Ok(Ok(placed)) => placed,
        Ok(Err((error, placed))) => return Err(error.with_undo(undo(&placed))),
        Err(error) => return Err(TransferError::Text(error.to_string())),
    };
    let row = match stored(&placement, models_dir, found) {
        Ok(row) => row,
        Err(text) => return Err(TransferError::Text(text).with_undo(undo(&placed))),
    };
    let defaults = ModelDefaults {
        params: found.params.clone().unwrap_or_default(),
        system: found.system.clone().unwrap_or_default(),
    };
    let model_id = row.id.clone();
    let rows: Vec<ImportRecord> = found
        .all_names()
        .map(|name| ImportRecord {
            origin: batch.origin,
            name: name.ollama_name(),
            model_id: Some(model_id.clone()),
            status: RecordStatus::Imported,
            suggestion: None,
            detail: None,
            at: crate::store::now_ms(),
            attempts: 0,
        })
        .collect();
    let saved = batch
        .store
        .call(move |connection| {
            // Ligne (sa propre transaction), puis réglages et correspondance
            // des noms ensemble ; la ligne est retirée si la suite échoue.
            table::save(connection, &row)?;
            let rest = (|| {
                let transaction = connection.transaction()?;
                if defaults.params.is_empty() && defaults.system.is_empty() {
                    records::remove_defaults(&transaction, &row.id)?;
                } else {
                    records::set_defaults(&transaction, &row.id, &defaults)?;
                }
                for record in &rows {
                    records::record(&transaction, record)?;
                }
                transaction.commit()?;
                Ok(())
            })();
            if rest.is_err() {
                let _ = table::remove(connection, &row.id);
            }
            rest
        })
        .await;
    if let Err(error) = saved {
        let text = t!("modelImport.saveFailed", error = error);
        return Err(TransferError::Text(text).with_undo(undo(&placed)));
    }
    for (step, item) in steps.iter().zip(&placed) {
        let at = match item {
            Placed::Renamed { target, .. }
            | Placed::Linked(target)
            | Placed::Written(target)
            | Placed::Present(target) => target,
        };
        placed_blobs.insert(step.sha256.clone(), at.clone());
    }
    Ok(model_id)
}

/// Retire les dossiers restés vides des fichiers `steps` après un échec
/// (créés avant la copie).
fn forget_folders(steps: &[Step], models_dir: &Path) {
    for step in steps {
        transfer::remove_empty_parents(&step.target, models_dir);
    }
}

#[cfg(test)]
mod tests;
