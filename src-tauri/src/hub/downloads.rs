//! Téléchargements du ModelHub (état Tauri `HubDownloads`), directement depuis
//! Hugging Face depuis le passage à llama-server (contrat
//! `100-llamacpp-lots-1-2-4-contract.md` § 3.3 ; file d'avant : contrat
//! `37-step15a-contract.md` § 2 et § 8). Mêmes commandes et même événement
//! qu'avec Ollama : la page ne change pas.
//!
//! - File FIFO, deux téléchargements à la fois au plus (`Queued` pour les
//!   autres).
//! - Démarrage (`Starting`) : fiche du dépôt lue (`hub::files`, commit figé,
//!   tailles et SHA-256), fichiers de la version choisis (morceaux
//!   `-0000N-of-0000M` dans l'ordre, projecteur de vision s'il y en a un),
//!   place disque vérifiée (morceaux déjà reçus déduits, ce qui reste à
//!   recevoir pour les autres téléchargements en cours aussi, marge de
//!   100 Mio). Ce plan (commit compris) reste attaché à l'entrée : une
//!   reprise après une pause ou un retour en file repart du même commit,
//!   même si la branche `main` a changé entre-temps.
//! - `Downloading` : chaque fichier arrive dans `<fichier>.part` sous
//!   `<dossier des modèles>/<auteur>/<dépôt>/<chemin d'origine>`, repris par
//!   `Range`, avec son empreinte attendue à côté (`<fichier>.part.sha256`) :
//!   un `.part` d'une autre révision (session précédente) repart de zéro.
//!   Taille et SHA-256 vérifiés avant le renommage (empreinte fausse :
//!   `.part` supprimé). Un fichier déjà là, à la bonne taille et à la bonne
//!   empreinte (connue de la base, déjà vérifiée pendant cette session à
//!   taille et date égales, sinon recalculée), n'est pas retéléchargé : un
//!   projecteur est partagé entre les versions d'un dépôt. Chaque fichier
//!   est réservé (`local_models::Reserved`) de son premier octet à la fin de
//!   la tâche : deux téléchargements ne l'écrivent jamais ensemble (le second
//!   attend), et ni une suppression ni une annulation ne l'effacent pendant
//!   ce temps. Délai d'inactivité : plus aucun octet pendant `idle_timeout`
//!   → `Failed` (reprise possible).
//! - `Finishing` : tout est reçu ; en-têtes GGUF lus (`hub::gguf::meta` :
//!   architecture, contexte, capacités, encodeurs du projecteur), fichiers
//!   revus (tous encore là, à leur taille), ligne `local_models` écrite.
//!   Un fichier qui n'est pas un GGUF : `Failed` lisible, fichiers reçus
//!   supprimés ; un fichier illisible (supprimé, verrouillé par un
//!   antivirus) : `Failed`, fichiers gardés.
//! - Annulation : en file → `Cancelled` ; en cours → `Cancelled` tout de
//!   suite, requête fermée, puis (fin de la tâche) fichiers, `.part` et
//!   empreintes attendues reçus pour ce modèle supprimés, sauf les fichiers
//!   d'un modèle installé (leur `.part` d'une mise à jour part quand même)
//!   et ceux qu'un autre téléchargement a réservés. Pendant `Finishing`,
//!   jusqu'à l'écriture de la ligne : le modèle n'est pas installé ; une fois
//!   l'écriture commencée, l'annulation arrive trop tard (sans effet).
//! - Pause (propre à ChatonLM) : en file ou en cours → `Paused`, requête
//!   fermée, `.part` gardés, progression gardée à l'écran ; le suivant de la
//!   file démarre. Reprise → `Queued` à sa place, repart des `.part`. Rien
//!   pendant `Finishing`. Pas gardé d'une session à l'autre (comme toute la
//!   file), mais les `.part` restent : un nouveau clic reprend.
//! - Priorité (« Download first ») : en tête de la file, démarré tout de
//!   suite ; si les places sont prises, le dernier en cours (hors
//!   `Finishing`) retourne en file (requête fermée, `.part` gardés).
//! - Événement : liste complète à chaque changement d'état, progression
//!   limitée à 4 émissions par seconde (la dernière n'est jamais perdue).
//! - Débit lissé sur environ 3 s, seulement pendant `Downloading`.
//! - Avant [`HubDownloads::attach`] (base illisible), un téléchargement échoue
//!   avec un message clair.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use tokio::time::Instant;
use url::Url;

use super::errors;
use super::files::{self, Plan, PlanFile};
use super::gguf::meta::{self, MetaError};
use super::names;
use super::types::{HubDownload, HubDownloadRequest, HubDownloadState};
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::image_models::downloads::{needed_bytes, part_path, space_problem};
use crate::local_models::id::{ModelId, same_model};
use crate::local_models::{self, ModelCapabilities, ModelSource, Reserved, file_key};
use crate::ollama::client;
use crate::store::local_models::{self as table, StoredLocalModel};
use crate::store::{Store, now_ms};

/// Réglages de la file (modifiables pour les tests).
#[derive(Clone, Debug)]
pub struct Options {
    /// Téléchargements simultanés au plus.
    pub max_running: usize,
    /// Plus aucun octet pendant ce délai : échec.
    pub idle_timeout: Duration,
    /// Intervalle minimal entre deux émissions de progression.
    pub progress_interval: Duration,
    /// Délai de la lecture de la fiche du dépôt.
    pub listing_timeout: Duration,
    /// Adresse de Hugging Face ; `None` : `names::hf_url()` au démarrage de
    /// chaque téléchargement.
    pub hf_url: Option<String>,
    /// Vrai pendant un changement de Settings > Model location
    /// (`commands::models_dir::location_changing`) : un nouveau
    /// téléchargement est refusé.
    pub location_changing: fn() -> bool,
    /// Place libre pour un dossier (essais : disque plein simulé).
    pub free_space: fn(&Path) -> Option<u64>,
    /// Attente pendant `Finishing`, avant la revue des fichiers et
    /// l'écriture de la ligne (essais seulement : zéro sinon).
    pub finish_delay: Duration,
}

fn never() -> bool {
    false
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_running: 2,
            idle_timeout: Duration::from_secs(120),
            progress_interval: Duration::from_millis(250),
            listing_timeout: Duration::from_secs(20),
            hf_url: None,
            location_changing: never,
            free_space: crate::disk::free_space_for,
            finish_delay: Duration::ZERO,
        }
    }
}

/// Fenêtre du débit lissé.
const RATE_WINDOW: Duration = Duration::from_secs(3);

/// Durée minimale observée avant d'annoncer un débit.
const RATE_MIN_SPAN: Duration = Duration::from_millis(500);

/// Attente entre deux essais pour un fichier qu'un autre téléchargement
/// écrit.
const FILE_WAIT: Duration = Duration::from_millis(100);

/// Fichiers vérifiés gardés en mémoire au plus (au-delà : tout est oublié,
/// une vérification de plus au pire).
const MAX_VERIFIED: usize = 4096;

/// Plan d'un téléchargement (fichiers au commit figé) et dossier des
/// modèles, attachés à l'entrée dès qu'ils sont connus.
type Pinned = Arc<(Plan, PathBuf)>;

type Emit = Box<dyn Fn(&[HubDownload]) + Send + Sync>;

/// Arrêt demandé à une tâche.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Run,
    /// Pause, retour en file, sortie : les `.part` restent.
    Keep,
    /// Annulation : ce qui a été reçu pour ce modèle est supprimé.
    Discard,
}

/// Base et dossier des données, donnés par [`HubDownloads::attach`].
struct Storage {
    store: Store,
    data_dir: PathBuf,
}

/// Téléchargements en cours, en file et terminés, partagés par les commandes.
pub struct HubDownloads {
    inner: Arc<Inner>,
}

struct Inner {
    options: Options,
    emit: Emit,
    state: Mutex<State>,
    storage: OnceLock<Storage>,
    /// Fichiers réservés par les tâches (chemins complets) : un seul
    /// téléchargement à la fois par fichier, jamais supprimés pendant ce
    /// temps (partagé avec `hub_delete_model`).
    reserved: Reserved,
    /// Fichiers complets déjà vérifiés pendant cette session (clé de
    /// [`file_key`]) : une reprise ne recalcule pas leur SHA-256.
    verified: Mutex<HashMap<String, Verified>>,
    /// Octets qui restent à recevoir, par tâche en cours (place disque
    /// comptée pour tous les téléchargements, pas chacun seul).
    claims: Mutex<HashMap<u64, u64>>,
    /// Tâches de téléchargement encore en vie (attendues à la sortie).
    live: watch::Sender<usize>,
    next_id: AtomicU64,
    /// Fichiers dont le SHA-256 a été calculé, dans l'ordre (essais).
    #[cfg(test)]
    hashed: Mutex<Vec<PathBuf>>,
}

/// Fichier complet vérifié : taille, date et empreinte d'alors.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Verified {
    size: u64,
    modified: Option<SystemTime>,
    sha256: String,
}

#[derive(Default)]
struct State {
    /// Dans l'ordre des demandes.
    entries: Vec<Entry>,
    /// Sortie de l'app : plus rien ne démarre.
    closed: bool,
    last_emit: Option<Instant>,
    /// Progression pas encore émise.
    dirty: bool,
    flush_scheduled: bool,
    /// Modèles (identifiants en minuscules) dont une tâche n'a pas fini :
    /// après une pause ou une annulation, la suivante attend la vraie fin de
    /// la précédente (mêmes `.part`).
    tasks: HashSet<String>,
}

/// Clé d'un modèle dans `State::tasks`.
fn task_key(model: &str) -> String {
    normalize(model).to_ascii_lowercase()
}

struct Entry {
    /// Distingue une entrée de celle qui l'a remplacée (même nom).
    id: u64,
    download: HubDownload,
    /// Arrêt de la tâche (en cours seulement).
    cancel: Option<watch::Sender<Stop>>,
    rate: Rate,
    /// Plan figé au premier démarrage, repris tel quel par les suivants.
    plan: Option<Pinned>,
    /// La ligne `local_models` s'écrit : trop tard pour annuler.
    committing: bool,
}

/// Fin d'un téléchargement.
#[derive(Debug, PartialEq, Eq)]
enum End {
    Done,
    Failed(String),
    Cancelled,
}

/// Effet d'une mise à jour d'une entrée.
enum Change {
    Nothing,
    Progress,
    State,
}

fn active(state: HubDownloadState) -> bool {
    matches!(
        state,
        HubDownloadState::Queued
            | HubDownloadState::Starting
            | HubDownloadState::Downloading
            | HubDownloadState::Finishing
    )
}

fn running(state: HubDownloadState) -> bool {
    active(state) && state != HubDownloadState::Queued
}

/// Pas encore fini : en file, en cours ou en pause.
fn pending(state: HubDownloadState) -> bool {
    active(state) || state == HubDownloadState::Paused
}

/// Identifiant d'une demande (`hf.co/…` ramené à `<auteur>/<dépôt>:<QUANT>`),
/// sinon le texte tel quel.
fn normalize(model: &str) -> String {
    ModelId::from_request(model).map_or_else(|| model.trim().to_owned(), |id| id.to_string())
}

/// Même modèle (quantification sans la casse).
fn same(a: &str, b: &str) -> bool {
    same_model(&normalize(a), &normalize(b))
}

impl Inner {
    /// Un changement de dossier des modèles est en cours : celui d'Ollama
    /// (`Options::location_changing`), ou celui de la base locale
    /// (`local_models::import::folder`, lot L6) ; vu sous le verrou de la
    /// file, que le changement prend avant de regarder la file.
    fn location_changing(&self) -> bool {
        (self.options.location_changing)() || crate::local_models::import::folder::is_busy()
    }
}

impl HubDownloads {
    /// `emit` reçoit la liste complète à chaque changement.
    pub fn with_options(
        emit: impl Fn(&[HubDownload]) + Send + Sync + 'static,
        options: Options,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                options,
                emit: Box::new(emit),
                state: Mutex::new(State::default()),
                storage: OnceLock::new(),
                reserved: Reserved::default(),
                verified: Mutex::new(HashMap::new()),
                claims: Mutex::new(HashMap::new()),
                live: watch::channel(0).0,
                next_id: AtomicU64::new(1),
                #[cfg(test)]
                hashed: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Base de l'app et dossier des données (dossier des modèles par défaut :
    /// `<données>/models`) ; une seule fois, une fois la base ouverte.
    pub fn attach(&self, store: Store, data_dir: PathBuf) {
        let _ = self.inner.storage.set(Storage { store, data_dir });
    }

    /// Fichiers réservés par les téléchargements en cours : à respecter par
    /// toute suppression de modèle (`local_models::delete`).
    pub fn reserved(&self) -> Reserved {
        self.inner.reserved.clone()
    }

    /// Téléchargements, dans l'ordre des demandes.
    pub fn list(&self) -> Vec<HubDownload> {
        snapshot(&self.inner.state.lock())
    }

    /// Vrai si un téléchargement est en file ou en cours.
    pub fn any_active(&self) -> bool {
        self.list().iter().any(|download| active(download.state))
    }

    /// Demande un téléchargement. Identifiant invalide, ou changement de
    /// dossier des modèles en cours : refus ; déjà en cours ou en file :
    /// rien ; en pause : repris ; terminé, échoué ou arrêté : remplacé (en fin
    /// de file).
    pub fn download(&self, request: HubDownloadRequest) -> AppResult<()> {
        let Some(id) = ModelId::from_request(&request.model).filter(|id| id.repo_id().is_some())
        else {
            // Montré par la proposition de VS Code et l'onglet Polish.
            return Err(AppError::Invalid(t!(
                "localModels.invalidId",
                model = request.model.trim()
            )));
        };
        let model = id.to_string();
        let inner = &self.inner;
        let mut state = inner.state.lock();
        if state.closed {
            return Ok(());
        }
        // Sous le verrou de la file : un changement qui commence voit ce
        // téléchargement dans `any_active`, ou ce téléchargement voit le
        // changement.
        if inner.location_changing() {
            return Err(AppError::Invalid(errors::location_changing()));
        }
        if let Some(index) = state
            .entries
            .iter()
            .position(|entry| same(&entry.download.model, &model))
        {
            if active(state.entries[index].download.state) {
                return Ok(());
            }
            if state.entries[index].download.state == HubDownloadState::Paused {
                state.entries[index].download.state = HubDownloadState::Queued;
                inner.pump(&mut state);
                inner.emit_now(&mut state);
                return Ok(());
            }
            state.entries.remove(index);
        }
        state.entries.push(Entry {
            id: inner.next_id.fetch_add(1, Ordering::Relaxed),
            download: HubDownload {
                repo: request.repo.or_else(|| id.repo_id()),
                label: request.label.or_else(|| Some(id.quant().to_owned())),
                model,
                state: HubDownloadState::Queued,
                completed: 0,
                total: 0,
                bytes_per_second: None,
                error: None,
                started_at: now_ms(),
                finished_at: None,
            },
            cancel: None,
            rate: Rate::default(),
            plan: None,
            committing: false,
        });
        inner.pump(&mut state);
        inner.emit_now(&mut state);
        Ok(())
    }

    /// Arrête un téléchargement en file, en cours (requête fermée, fichiers
    /// reçus supprimés à la fin de la tâche) ou en pause ; inconnu, déjà
    /// fini ou en train d'écrire sa ligne : rien.
    pub fn cancel(&self, model: &str) {
        let inner = &self.inner;
        let mut state = inner.state.lock();
        let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| same(&entry.download.model, model))
            .filter(|entry| pending(entry.download.state) && !entry.committing)
        else {
            return;
        };
        let paused = entry.download.state == HubDownloadState::Paused;
        finish_entry(entry, End::Cancelled, Stop::Discard);
        // En pause : aucune tâche ne le fera.
        if paused {
            inner.discard_later(&entry.download.model, entry.plan.clone());
        }
        inner.pump(&mut state);
        inner.emit_now(&mut state);
    }

    /// Met en pause un téléchargement en file ou en cours (requête fermée,
    /// `.part` gardés) ; sa progression reste affichée et le suivant de la
    /// file démarre. Sans effet pendant `Finishing` (tout est reçu), sur un
    /// inconnu ou sur un téléchargement fini.
    pub fn pause(&self, model: &str) {
        let inner = &self.inner;
        let mut state = inner.state.lock();
        let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| same(&entry.download.model, model))
            .filter(|entry| {
                matches!(
                    entry.download.state,
                    HubDownloadState::Queued
                        | HubDownloadState::Starting
                        | HubDownloadState::Downloading
                )
            })
        else {
            return;
        };
        inner.halt(entry, HubDownloadState::Paused);
        inner.pump(&mut state);
        inner.emit_now(&mut state);
    }

    /// Reprend un téléchargement en pause : de nouveau en file, à sa place
    /// (il démarre s'il y a de la place, et repart des `.part`). Changement
    /// de dossier des modèles en cours : refus. Pas en pause : rien.
    pub fn resume(&self, model: &str) -> AppResult<()> {
        let inner = &self.inner;
        let mut state = inner.state.lock();
        if state.closed {
            return Ok(());
        }
        let Some(index) = state.entries.iter().position(|entry| {
            same(&entry.download.model, model) && entry.download.state == HubDownloadState::Paused
        }) else {
            return Ok(());
        };
        if inner.location_changing() {
            return Err(AppError::Invalid(errors::location_changing()));
        }
        state.entries[index].download.state = HubDownloadState::Queued;
        inner.pump(&mut state);
        inner.emit_now(&mut state);
        Ok(())
    }

    /// Fait passer un téléchargement en premier : il prend la tête de la
    /// file et démarre tout de suite (en pause : repris). Si toutes les
    /// places sont prises, le dernier téléchargement en cours (hors
    /// `Finishing`) retourne en file, requête fermée : il reprendra de ses
    /// `.part` à la première place libre. Déjà en cours : il passe seulement
    /// en tête. Inconnu ou fini : rien ; reprise refusée pendant un
    /// changement de dossier des modèles.
    pub fn prioritize(&self, model: &str) -> AppResult<()> {
        let inner = &self.inner;
        let mut state = inner.state.lock();
        if state.closed {
            return Ok(());
        }
        let Some(index) = state
            .entries
            .iter()
            .position(|entry| same(&entry.download.model, model) && pending(entry.download.state))
        else {
            return Ok(());
        };
        let paused = state.entries[index].download.state == HubDownloadState::Paused;
        if paused && inner.location_changing() {
            return Err(AppError::Invalid(errors::location_changing()));
        }
        let mut entry = state.entries.remove(index);
        if paused {
            entry.download.state = HubDownloadState::Queued;
        }
        let waiting = entry.download.state == HubDownloadState::Queued;
        state.entries.insert(0, entry);
        let busy = state
            .entries
            .iter()
            .filter(|entry| running(entry.download.state))
            .count();
        if waiting
            && busy >= inner.options.max_running
            && let Some(last) = state.entries.iter_mut().rev().find(|entry| {
                matches!(
                    entry.download.state,
                    HubDownloadState::Starting | HubDownloadState::Downloading
                )
            })
        {
            inner.halt(last, HubDownloadState::Queued);
        }
        inner.pump(&mut state);
        inner.emit_now(&mut state);
        Ok(())
    }

    /// Retire un téléchargement terminé, échoué ou arrêté (ni un actif, ni
    /// un téléchargement en pause : il s'arrête d'abord).
    pub fn dismiss(&self, model: &str) {
        let inner = &self.inner;
        let mut state = inner.state.lock();
        let before = state.entries.len();
        state.entries.retain(|entry| {
            !(same(&entry.download.model, model) && !pending(entry.download.state))
        });
        if state.entries.len() != before {
            inner.emit_now(&mut state);
        }
    }

    /// Sans effet depuis les téléchargements directs (ils ne passent plus
    /// par un serveur Ollama) ; gardé tant que `ollama::target` l'appelle.
    pub fn retarget(&self, _server: &str) {}

    /// Sortie de l'app : plus rien ne démarre, les téléchargements en cours
    /// sont fermés (`.part` gardés), attente de leur fin, `limit` au plus.
    pub async fn stop_all(&self, limit: Duration) {
        {
            let mut state = self.inner.state.lock();
            state.closed = true;
            for entry in &state.entries {
                if let Some(cancel) = &entry.cancel {
                    let _ = cancel.send(Stop::Keep);
                }
            }
        }
        let mut live = self.inner.live.subscribe();
        let _ = tokio::time::timeout(limit, live.wait_for(|count| *count == 0)).await;
    }
}

/// Copie de la liste à émettre.
fn snapshot(state: &State) -> Vec<HubDownload> {
    state
        .entries
        .iter()
        .map(|entry| entry.download.clone())
        .collect()
}

/// Fin d'une entrée active (l'arrêt de sa tâche est demandé : `stop`).
fn finish_entry(entry: &mut Entry, end: End, stop: Stop) {
    let download = &mut entry.download;
    match end {
        End::Done => {
            download.state = HubDownloadState::Done;
            download.completed = download.total;
        }
        End::Failed(message) => {
            download.state = HubDownloadState::Failed;
            download.error = Some(message);
        }
        End::Cancelled => download.state = HubDownloadState::Cancelled,
    }
    download.bytes_per_second = None;
    download.finished_at = Some(now_ms());
    if let Some(cancel) = entry.cancel.take() {
        let _ = cancel.send(stop);
    }
}

impl Inner {
    /// Arrête la tâche d'une entrée sans la finir : l'entrée passe à `state`
    /// (en pause, ou de nouveau en file), garde sa progression, et change de
    /// numéro : la fin de l'ancienne tâche ne la concerne plus.
    fn halt(&self, entry: &mut Entry, state: HubDownloadState) {
        if let Some(cancel) = entry.cancel.take() {
            let _ = cancel.send(Stop::Keep);
        }
        entry.id = self.next_id.fetch_add(1, Ordering::Relaxed);
        entry.download.state = state;
        entry.download.bytes_per_second = None;
        entry.rate = Rate::default();
    }

    /// Démarre les téléchargements en file, dans l'ordre, tant qu'il y a de la
    /// place.
    fn pump(self: &Arc<Self>, state: &mut State) {
        if state.closed {
            return;
        }
        let mut busy = state
            .entries
            .iter()
            .filter(|entry| running(entry.download.state))
            .count();
        let State { entries, tasks, .. } = state;
        for entry in entries.iter_mut() {
            if busy >= self.options.max_running {
                break;
            }
            let key = task_key(&entry.download.model);
            if entry.download.state != HubDownloadState::Queued || tasks.contains(&key) {
                continue;
            }
            busy += 1;
            tasks.insert(key);
            entry.download.state = HubDownloadState::Starting;
            let (cancel, cancelled) = watch::channel(Stop::Run);
            entry.cancel = Some(cancel);
            self.live.send_modify(|count| *count += 1);
            let task = Task {
                inner: self.clone(),
                id: entry.id,
                model: entry.download.model.clone(),
                pinned: entry.plan.clone(),
                received: Mutex::new(None),
                unusable: AtomicBool::new(false),
            };
            tauri::async_runtime::spawn(task.run(cancelled));
        }
    }

    /// Émet la liste tout de suite (changement d'état).
    fn emit_now(&self, state: &mut State) {
        state.dirty = false;
        state.last_emit = Some(Instant::now());
        (self.emit)(&snapshot(state));
    }

    /// Progression : émise si la dernière émission date d'au moins
    /// `progress_interval`, sinon à l'échéance.
    fn emit_progress(self: &Arc<Self>, state: &mut State) {
        let now = Instant::now();
        let due = state
            .last_emit
            .map(|last| last + self.options.progress_interval);
        match due {
            Some(due) if due > now => {
                state.dirty = true;
                if !state.flush_scheduled {
                    state.flush_scheduled = true;
                    let inner = self.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep_until(due).await;
                        inner.flush();
                    });
                }
            }
            _ => self.emit_now(state),
        }
    }

    /// Échéance : progression retenue émise (ou de nouveau retenue si une
    /// émission a eu lieu entre-temps).
    fn flush(self: &Arc<Self>) {
        let mut state = self.state.lock();
        state.flush_scheduled = false;
        if state.dirty {
            self.emit_progress(&mut state);
        }
    }

    /// Applique `change` à l'entrée `id` si elle est encore active.
    fn change(self: &Arc<Self>, id: u64, change: impl FnOnce(&mut Entry) -> Change) {
        let mut state = self.state.lock();
        let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && active(entry.download.state))
        else {
            return;
        };
        match change(entry) {
            Change::Nothing => {}
            Change::Progress => self.emit_progress(&mut state),
            Change::State => self.emit_now(&mut state),
        }
    }

    /// Fin de la tâche `id` : l'entrée (si elle est encore active : pas
    /// annulée ni remplacée) prend son état final, la suivante démarre.
    fn finish(self: &Arc<Self>, id: u64, end: End) {
        let mut state = self.state.lock();
        if let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && active(entry.download.state))
        {
            finish_entry(entry, end, Stop::Keep);
            self.pump(&mut state);
            self.emit_now(&mut state);
        }
    }

    /// Fin de la tâche de `model` : le suivant du même modèle peut démarrer.
    fn task_ended(self: &Arc<Self>, model: &str) {
        let mut state = self.state.lock();
        state.tasks.remove(&task_key(model));
        let before: Vec<HubDownloadState> = state
            .entries
            .iter()
            .map(|entry| entry.download.state)
            .collect();
        self.pump(&mut state);
        let after = state.entries.iter().map(|entry| entry.download.state);
        if !after.eq(before) {
            self.emit_now(&mut state);
        }
    }

    /// Annulation d'un téléchargement en pause (sans tâche) : son plan dit
    /// quels fichiers supprimer (sans plan, jamais démarré pendant cette
    /// session : fiche relue, pour les `.part` d'une session précédente).
    fn discard_later(self: &Arc<Self>, model: &str, plan: Option<Pinned>) {
        let inner = self.clone();
        let model = model.to_owned();
        self.live.send_modify(|count| *count += 1);
        tauri::async_runtime::spawn(async move {
            if let Some(pinned) = plan {
                inner.discard(&pinned.0, &pinned.1).await;
            } else if let Some((plan, dir)) = inner.plan_of(&model).await {
                inner.discard(&plan, &dir).await;
            }
            inner.live.send_modify(|count| *count -= 1);
        });
    }

    /// Attache le plan de la tâche `id` à son entrée (encore active).
    fn pin(&self, id: u64, pinned: &Pinned) {
        let mut state = self.state.lock();
        if let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && active(entry.download.state))
        {
            entry.plan = Some(pinned.clone());
        }
    }

    /// Avant d'écrire la ligne de la tâche `id` : vrai si elle n'est ni
    /// arrêtée ni annulée, et l'entrée ne s'annule plus ensuite (sous le
    /// verrou de la file, comme `cancel`).
    fn begin_commit(&self, id: u64, cancel: &watch::Receiver<Stop>) -> bool {
        let mut state = self.state.lock();
        if *cancel.borrow() != Stop::Run {
            return false;
        }
        let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && active(entry.download.state))
        else {
            return false;
        };
        entry.committing = true;
        true
    }

    /// `path` a été vérifié complet à l'empreinte `sha256` : gardé en
    /// mémoire avec sa taille et sa date.
    fn remember(&self, path: &Path, sha256: &str) {
        let Ok(metadata) = std::fs::metadata(path) else {
            return;
        };
        let mut verified = self.verified.lock();
        if verified.len() >= MAX_VERIFIED {
            verified.clear();
        }
        verified.insert(
            file_key(&path.to_string_lossy()),
            Verified {
                size: metadata.len(),
                modified: metadata.modified().ok(),
                sha256: sha256.to_ascii_lowercase(),
            },
        );
    }

    /// `path` (de cette taille et de cette date) déjà vérifié à l'empreinte
    /// `sha256` pendant cette session.
    fn known_good(&self, path: &Path, metadata: &std::fs::Metadata, sha256: &str) -> bool {
        self.verified
            .lock()
            .get(&file_key(&path.to_string_lossy()))
            .is_some_and(|known| {
                known.size == metadata.len()
                    && known.modified.is_some()
                    && known.modified == metadata.modified().ok()
                    && known.sha256.eq_ignore_ascii_case(sha256)
            })
    }

    /// Fichiers prévus pour `model` (fiche relue) et dossier des modèles.
    async fn plan_of(&self, model: &str) -> Option<(Plan, PathBuf)> {
        let storage = self.storage.get()?;
        let id = ModelId::from_request(model)?;
        let hf_url = self.options.hf_url.clone().unwrap_or_else(names::hf_url);
        let listing = files::fetch(&hf_url, &id.repo_id()?, self.options.listing_timeout)
            .await
            .ok()?;
        let plan = files::plan(&id, &listing).ok()?;
        let dir = local_models::models_dir(&storage.store, &storage.data_dir)
            .await
            .ok()?;
        Some((plan, dir))
    }

    /// Supprime ce qui a été reçu pour `plan` (fichiers, `.part` et
    /// empreintes attendues), sauf les fichiers d'un modèle installé (mais
    /// leur `.part`, d'une mise à jour annulée, part) et tout ce qu'un autre
    /// téléchargement a réservé, puis les dossiers devenus vides sous le
    /// dossier des modèles. Réservations vérifiées et suppressions sous le
    /// même verrou : rien ne se réserve entre les deux.
    async fn discard(&self, plan: &Plan, dir: &Path) {
        let Some(storage) = self.storage.get() else {
            return;
        };
        let installed: HashSet<String> = storage
            .store
            .call(|connection| {
                Ok(table::list(connection)?
                    .iter()
                    .flat_map(|model| model.all_files().map(file_key))
                    .collect())
            })
            .await
            .unwrap_or_default();
        let files: Vec<(bool, PathBuf)> = plan
            .all()
            .filter_map(|file| {
                let relative = plan.local(file);
                local_models::absolute(dir, &relative)
                    .map(|path| (installed.contains(&file_key(&relative)), path))
            })
            .collect();
        let reserved = self.reserved.clone();
        let dir = dir.to_owned();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            reserved.while_locked(|is_reserved| discard_files(&dir, &files, is_reserved));
        })
        .await;
    }
}

/// Supprime les fichiers `files` (installé ?, chemin complet) sous `dir`
/// pour [`Inner::discard`].
fn discard_files(dir: &Path, files: &[(bool, PathBuf)], is_reserved: &dyn Fn(&Path) -> bool) {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for (installed, path) in files {
        if is_reserved(path) {
            continue;
        }
        let _ = std::fs::remove_file(part_path(path));
        let _ = std::fs::remove_file(local_models::expected_path(path));
        if *installed {
            continue;
        }
        let _ = std::fs::remove_file(path);
        for ancestor in path.ancestors().skip(1) {
            if ancestor == dir || !ancestor.starts_with(dir) {
                break;
            }
            if !dirs.iter().any(|known| known == ancestor) {
                dirs.push(ancestor.to_path_buf());
            }
        }
    }
    dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    for dir in dirs {
        let _ = std::fs::remove_dir(&dir);
    }
}

/// Fichier réservé par une tâche (voir `Inner::reserved`), libéré à la fin
/// de la tâche.
struct Writing<'a> {
    inner: &'a Inner,
    path: PathBuf,
}

impl Drop for Writing<'_> {
    fn drop(&mut self) {
        self.inner.reserved.release(&self.path);
    }
}

/// Place disque comptée pour une tâche (voir `Inner::claims`), rendue à la
/// fin.
struct Claim<'a> {
    inner: &'a Inner,
    id: u64,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.inner.claims.lock().remove(&self.id);
    }
}

/// Un téléchargement en cours.
struct Task {
    inner: Arc<Inner>,
    id: u64,
    model: String,
    /// Plan d'une tâche précédente de la même entrée (pause, retour en file).
    pinned: Option<Pinned>,
    /// Fichiers prévus et dossier des modèles, une fois connus (pour une
    /// annulation).
    received: Mutex<Option<(Plan, PathBuf)>>,
    /// Un fichier reçu n'est pas un GGUF : rien à garder.
    unusable: AtomicBool,
}

impl Task {
    async fn run(self, mut cancel: watch::Receiver<Stop>) {
        let end = self.download(&mut cancel).await;
        let discard = match &end {
            End::Cancelled => *cancel.borrow() == Stop::Discard,
            // Fichier reçu inutilisable : rien à garder.
            End::Failed(_) => self.unusable.load(Ordering::Relaxed),
            End::Done => false,
        };
        self.inner.finish(self.id, end);
        let received = self.received.lock().take();
        if discard && let Some((plan, dir)) = received {
            self.inner.discard(&plan, &dir).await;
        }
        self.inner.task_ended(&self.model);
        self.inner.live.send_modify(|count| *count -= 1);
    }

    async fn download(&self, cancel: &mut watch::Receiver<Stop>) -> End {
        let Some(storage) = self.inner.storage.get() else {
            return End::Failed(t!("localModels.noDatabase"));
        };
        let Some(id) = ModelId::from_request(&self.model) else {
            return End::Failed(errors::unavailable_version());
        };
        let Some(repo_id) = id.repo_id() else {
            return End::Failed(errors::unavailable_version());
        };
        let options = &self.inner.options;
        let data_dir = storage.data_dir.clone();
        let read = storage
            .store
            .call(move |connection| {
                let dir = local_models::models_dir_in(connection, &data_dir)?;
                let mut known: HashMap<String, String> = HashMap::new();
                for model in table::list(connection)? {
                    known.extend(
                        model
                            .sha256
                            .into_iter()
                            .map(|(path, sha)| (file_key(&path), sha)),
                    );
                }
                Ok((dir, known))
            })
            .await;
        let (dir, known) = match read {
            Ok(read) => read,
            Err(error) => return End::Failed(error.to_string()),
        };
        let pinned = match &self.pinned {
            Some(pinned) => pinned.clone(),
            None => {
                let hf_url = options.hf_url.clone().unwrap_or_else(names::hf_url);
                let listing = tokio::select! {
                    biased;
                    () = cancelled(cancel) => return End::Cancelled,
                    listing = files::fetch(&hf_url, &repo_id, options.listing_timeout) => listing,
                };
                let listing = match listing {
                    Ok(listing) => listing,
                    Err(files::ListingError(message)) => return End::Failed(message),
                };
                let plan = match files::plan(&id, &listing) {
                    Ok(plan) => plan,
                    Err(message) => return End::Failed(message),
                };
                let pinned = Arc::new((plan, dir));
                self.inner.pin(self.id, &pinned);
                pinned
            }
        };
        let (plan, dir) = (&pinned.0, &pinned.1);
        *self.received.lock() = Some((plan.clone(), dir.clone()));
        let paths: Vec<(PathBuf, u64)> = match plan
            .all()
            .map(|file| path_of(dir, plan, file).map(|path| (path, file.size)))
            .collect()
        {
            Ok(paths) => paths,
            Err(end) => return end,
        };
        let _claim = match self.claim_space(&paths, dir) {
            Ok(claim) => claim,
            Err(message) => return End::Failed(message),
        };
        let total = plan.total();
        let mut before: u64 = 0;
        // Réservations gardées jusqu'à la fin de la tâche (installation
        // comprise) : un projecteur reçu ne disparaît pas avant la ligne.
        let mut held = Vec::new();
        for (file, (path, size)) in plan.all().zip(&paths) {
            match self.reserve(path, cancel).await {
                Ok(reserved) => held.push(reserved),
                Err(end) => return end,
            }
            let trusted = known
                .get(&file_key(&plan.local(file)))
                .is_some_and(|sha| sha.eq_ignore_ascii_case(&file.sha256));
            if let Err(end) = self.fetch(file, path, trusted, before, total, cancel).await {
                return end;
            }
            before = before.saturating_add(*size);
        }
        self.finish_install(plan, dir, storage, cancel).await
    }

    /// Place disque : ce qui reste à recevoir pour `paths`, plus ce qui
    /// reste aux autres téléchargements en cours, doit tenir (marge
    /// comprise) ; compté pour cette tâche jusqu'à sa fin. Sous le verrou des
    /// places : deux tâches qui démarrent ensemble se voient.
    fn claim_space(&self, paths: &[(PathBuf, u64)], dir: &Path) -> Result<Claim<'_>, String> {
        let mut claims = self.inner.claims.lock();
        let others: u64 = claims
            .iter()
            .filter(|(id, _)| **id != self.id)
            .fold(0, |sum, (_, bytes)| sum.saturating_add(*bytes));
        let needed = needed_bytes(paths.iter().cloned());
        let free = (self.inner.options.free_space)(dir).map(|free| free.saturating_sub(others));
        if let Some(problem) = space_problem(needed, free) {
            return Err(problem);
        }
        claims.insert(self.id, needed);
        Ok(Claim {
            inner: &self.inner,
            id: self.id,
        })
    }

    /// Réserve `path` : attend qu'aucun autre téléchargement ne l'ait
    /// réservé.
    async fn reserve(
        &self,
        path: &Path,
        cancel: &mut watch::Receiver<Stop>,
    ) -> Result<Writing<'_>, End> {
        loop {
            if self.inner.reserved.try_reserve(path) {
                return Ok(Writing {
                    inner: &self.inner,
                    path: path.to_path_buf(),
                });
            }
            tokio::select! {
                biased;
                () = cancelled(cancel) => return Err(End::Cancelled),
                () = tokio::time::sleep(FILE_WAIT) => {}
            }
        }
    }

    /// Un fichier : déjà là (taille, empreinte connue de la base ou de cette
    /// session, sinon recalculée), sinon reçu dans `.part` (repris s'il est
    /// de la même révision), vérifié, renommé. `before` : octets des
    /// fichiers précédents ; `total` : de tous.
    async fn fetch(
        &self,
        file: &PlanFile,
        path: &Path,
        trusted: bool,
        before: u64,
        total: u64,
        cancel: &mut watch::Receiver<Stop>,
    ) -> Result<(), End> {
        let present = tokio::fs::metadata(path)
            .await
            .ok()
            .filter(|metadata| metadata.is_file() && metadata.len() == file.size);
        if let Some(metadata) = present
            && (trusted
                || self.inner.known_good(path, &metadata, &file.sha256)
                || self.verify(path, file, cancel).await?)
        {
            self.inner.remember(path, &file.sha256);
            self.progress(before.saturating_add(file.size), total);
            return Ok(());
        }
        if let Some(parent) = path.parent()
            && let Err(error) = tokio::fs::create_dir_all(parent).await
        {
            return Err(End::Failed(t!(
                "speechDownloads.createFolder",
                error = error
            )));
        }
        let part = part_path(path);
        let expected = local_models::expected_path(path);
        let mut have = tokio::fs::metadata(&part)
            .await
            .map_or(0, |metadata| metadata.len());
        // `.part` d'une autre révision : ses octets ne se complètent pas
        // avec ceux de celle-ci. Sans empreinte écrite : repris, jugé à la
        // fin par le SHA-256.
        if have > 0
            && let Ok(written) = tokio::fs::read_to_string(&expected).await
            && !written.trim().eq_ignore_ascii_case(&file.sha256)
        {
            let _ = tokio::fs::remove_file(&part).await;
            have = 0;
        }
        if have > file.size {
            let _ = tokio::fs::remove_file(&part).await;
            have = 0;
        }
        self.progress(before.saturating_add(have), total);
        if have < file.size {
            tokio::fs::write(&expected, file.sha256.as_bytes())
                .await
                .map_err(|error| save_failed(file, &error))?;
            self.receive(file, &part, have, before, total, cancel)
                .await?;
        }
        if !self.verify(&part, file, cancel).await? {
            let _ = tokio::fs::remove_file(&part).await;
            let _ = tokio::fs::remove_file(&expected).await;
            return Err(End::Failed(t!("speechDownloads.damaged", file = file.path)));
        }
        tokio::fs::rename(&part, path)
            .await
            .map_err(|error| save_failed(file, &error))?;
        let _ = tokio::fs::remove_file(&expected).await;
        self.inner.remember(path, &file.sha256);
        Ok(())
    }

    /// Taille et SHA-256 de `path` (sur un thread bloquant).
    async fn verify(
        &self,
        path: &Path,
        file: &PlanFile,
        cancel: &mut watch::Receiver<Stop>,
    ) -> Result<bool, End> {
        let checked = path.to_path_buf();
        #[cfg(test)]
        self.inner.hashed.lock().push(checked.clone());
        let (size, expected) = (file.size, file.sha256.clone());
        let verify =
            tauri::async_runtime::spawn_blocking(move || verify(&checked, size, &expected));
        let verified = tokio::select! {
            biased;
            () = cancelled(cancel) => return Err(End::Cancelled),
            verified = verify => verified,
        };
        match verified {
            Ok(Ok(valid)) => Ok(valid),
            Ok(Err(error)) => Err(End::Failed(t!(
                "speechDownloads.checkFailed",
                file = file.path,
                error = error
            ))),
            Err(error) => Err(End::Failed(t!(
                "speechDownloads.checkFailed",
                file = file.path,
                error = error
            ))),
        }
    }

    /// Reçoit la suite de `part` (`have` octets déjà là).
    async fn receive(
        &self,
        file: &PlanFile,
        part: &Path,
        mut have: u64,
        before: u64,
        total: u64,
        cancel: &mut watch::Receiver<Stop>,
    ) -> Result<(), End> {
        let idle = self.inner.options.idle_timeout;
        let interrupted = |error: &dyn std::fmt::Display| {
            End::Failed(t!("speechDownloads.interrupted", error = error))
        };
        let url = self.file_url(file);
        let client = Url::parse(&url)
            .map_err(|error| error.to_string())
            .and_then(|parsed| {
                client::builder(&parsed, None)
                    .connect_timeout(Duration::from_secs(15))
                    .build()
                    .map_err(|error| error.to_string())
            })
            .map_err(|error| End::Failed(t!("speechDownloads.failed", error = error)))?;
        let mut request = client.get(&url);
        if have > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let response = tokio::select! {
            biased;
            () = cancelled(cancel) => return Err(End::Cancelled),
            response = tokio::time::timeout(idle, request.send()) => response,
        };
        let mut response = match response {
            Err(_) => return Err(End::Failed(errors::timed_out())),
            Ok(Err(error)) if error.is_connect() => {
                return Err(End::Failed(t!("speechDownloads.unreachable")));
            }
            Ok(Err(error)) => return Err(interrupted(&error.without_url())),
            Ok(Ok(response)) => response,
        };
        let status = response.status();
        let append = match status.as_u16() {
            // 200 : tout le fichier (reprise ignorée par le serveur).
            200 => false,
            206 => have > 0,
            401 | 403 | 404 => return Err(End::Failed(errors::not_found_or_private())),
            // Morceau plus valable : on repart de zéro au prochain essai.
            416 => {
                let _ = tokio::fs::remove_file(part).await;
                return Err(End::Failed(t!("speechDownloads.resumeRefused")));
            }
            code => return Err(End::Failed(t!("speechDownloads.http", status = code))),
        };
        if !append {
            have = 0;
        }
        let mut output = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(part)
            .await
            .map_err(|error| save_failed(file, &error))?;
        loop {
            let chunk = tokio::select! {
                biased;
                () = cancelled(cancel) => {
                    let _ = output.flush().await;
                    return Err(End::Cancelled);
                }
                chunk = tokio::time::timeout(idle, response.chunk()) => chunk,
            };
            let bytes = match chunk {
                Err(_) => {
                    let _ = output.flush().await;
                    return Err(End::Failed(errors::timed_out()));
                }
                Ok(Ok(Some(bytes))) => bytes,
                Ok(Ok(None)) => break,
                Ok(Err(error)) => {
                    let _ = output.flush().await;
                    return Err(End::Failed(errors::connection_lost(Some(
                        &error.without_url(),
                    ))));
                }
            };
            if have + bytes.len() as u64 > file.size {
                drop(output);
                let _ = tokio::fs::remove_file(part).await;
                return Err(End::Failed(t!(
                    "speechDownloads.tooLarge",
                    file = file.path
                )));
            }
            output
                .write_all(&bytes)
                .await
                .map_err(|error| save_failed(file, &error))?;
            have += bytes.len() as u64;
            self.progress(before.saturating_add(have), total);
        }
        output
            .flush()
            .await
            .map_err(|error| save_failed(file, &error))?;
        if have < file.size {
            return Err(End::Failed(errors::connection_lost(None)));
        }
        Ok(())
    }

    /// Adresse du fichier à la révision figée.
    fn file_url(&self, file: &PlanFile) -> String {
        let base = self
            .inner
            .options
            .hf_url
            .clone()
            .unwrap_or_else(names::hf_url);
        let received = self.received.lock();
        let (repo, revision) = received
            .as_ref()
            .map(|(plan, _)| (plan.repo.as_str(), plan.revision.as_str()))
            .unwrap_or_default();
        files::resolve_url(&base, repo, revision, &file.path)
    }

    /// Progression montrée : `Downloading` à la première, `Finishing` quand
    /// tout est reçu.
    fn progress(&self, completed: u64, total: u64) {
        if let Some(claim) = self.inner.claims.lock().get_mut(&self.id) {
            *claim = (*claim).min(total.saturating_sub(completed));
        }
        self.inner
            .change(self.id, |entry| entry.progress(completed, total));
    }

    /// Tout est reçu : en-têtes GGUF lus, ligne `local_models` écrite.
    async fn finish_install(
        &self,
        plan: &Plan,
        dir: &Path,
        storage: &Storage,
        cancel: &mut watch::Receiver<Stop>,
    ) -> End {
        self.inner.change(self.id, |entry| {
            let download = &mut entry.download;
            download.completed = download.total;
            download.bytes_per_second = None;
            if download.state == HubDownloadState::Finishing {
                Change::Nothing
            } else {
                download.state = HubDownloadState::Finishing;
                Change::State
            }
        });
        let weights = match path_of(dir, plan, &plan.files[0]) {
            Ok(path) => path,
            Err(end) => return end,
        };
        let projector = match plan.mmproj.as_ref().map(|file| path_of(dir, plan, file)) {
            Some(Err(end)) => return end,
            Some(Ok(path)) => Some(path),
            None => None,
        };
        let read = tauri::async_runtime::spawn_blocking(move || {
            let weights = meta::read(&weights);
            let projector = projector.map(|path| meta::read(&path));
            (weights, projector)
        });
        let read = tokio::select! {
            biased;
            () = cancelled(cancel) => return End::Cancelled,
            read = read => read,
        };
        let (weights, projector) = match read {
            Ok(read) => read,
            Err(error) => return End::Failed(error.to_string()),
        };
        let (meta, projector) = match read_outcome(plan, weights, projector) {
            Ok(read) => read,
            Err((end, unusable)) => {
                self.unusable.store(unusable, Ordering::Relaxed);
                return end;
            }
        };
        let delay = self.inner.options.finish_delay;
        if !delay.is_zero() {
            // Sourde à l'arrêt : une annulation pendant l'attente n'est vue
            // que par `begin_commit`, comme en vrai entre la lecture des
            // en-têtes et l'écriture de la ligne.
            tokio::time::sleep(delay).await;
        }
        // Tout encore là (réservé, donc jamais supprimé par l'app ; un autre
        // programme a pu le faire) : jamais une ligne sans ses fichiers.
        for file in plan.all() {
            let Ok(path) = path_of(dir, plan, file) else {
                return End::Failed(t!("localModels.fileMissing", file = file.path));
            };
            let there = tokio::fs::metadata(&path)
                .await
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() == file.size);
            if !there {
                return End::Failed(t!("localModels.fileMissing", file = file.path));
            }
        }
        if !self.inner.begin_commit(self.id, cancel) {
            return End::Cancelled;
        }
        let model = stored_model(plan, &meta, projector.as_ref(), now_ms());
        match storage
            .store
            .call(move |connection| table::save(connection, &model))
            .await
        {
            Ok(()) => End::Done,
            Err(error) => End::Failed(error.to_string()),
        }
    }
}

/// Chemin complet d'un fichier du plan (toujours rangeable : `files::plan`
/// l'a vérifié ; sinon un échec lisible plutôt qu'un chemin hors du
/// dossier).
fn path_of(dir: &Path, plan: &Plan, file: &PlanFile) -> Result<PathBuf, End> {
    local_models::absolute(dir, &plan.local(file))
        .ok_or_else(|| End::Failed(t!("localModels.unsafeName", file = file.path)))
}

/// En-têtes lus des poids et du projecteur : leurs métadonnées, ou la fin
/// du téléchargement et s'il faut effacer ce qui a été reçu. Seul un
/// fichier qui n'est pas un GGUF est inutilisable ; une erreur de lecture
/// (fichier supprimé, verrouillé par un antivirus) garde tout pour un
/// nouvel essai.
fn read_outcome(
    plan: &Plan,
    weights: Result<meta::ModelMeta, MetaError>,
    projector: Option<Result<meta::ModelMeta, MetaError>>,
) -> Result<(meta::ModelMeta, Option<meta::ModelMeta>), (End, bool)> {
    let judge = |read: Result<meta::ModelMeta, MetaError>, file: &PlanFile| match read {
        Ok(meta) => Ok(meta),
        Err(MetaError::NotGguf) => Err((
            End::Failed(t!("localModels.notGgufFile", file = file.path)),
            true,
        )),
        Err(MetaError::Io(error)) => Err((
            End::Failed(t!(
                "speechDownloads.checkFailed",
                file = file.path,
                error = error
            )),
            false,
        )),
    };
    let meta = judge(weights, &plan.files[0])?;
    let projector = match (projector, &plan.mmproj) {
        (Some(read), Some(file)) => Some(judge(read, file)?),
        _ => None,
    };
    Ok((meta, projector))
}

/// Ligne de la base locale d'un téléchargement réussi ; `projector` : en-tête
/// du projecteur, s'il y en a un.
fn stored_model(
    plan: &Plan,
    meta: &meta::ModelMeta,
    projector: Option<&meta::ModelMeta>,
    installed_at: i64,
) -> StoredLocalModel {
    StoredLocalModel {
        id: plan.id.clone(),
        source: ModelSource::Hf,
        repo: plan.repo.clone(),
        revision: Some(plan.revision.clone()),
        quant: plan.quant.clone(),
        files: plan.files.iter().map(|file| plan.local(file)).collect(),
        mmproj: plan.mmproj.as_ref().map(|file| plan.local(file)),
        size_bytes: plan.total(),
        sha256: plan
            .all()
            .map(|file| (plan.local(file), file.sha256.to_ascii_lowercase()))
            .collect(),
        arch: meta.architecture.clone(),
        context_max: meta.context_length.and_then(|n| u32::try_from(n).ok()),
        params_label: meta.size_label.clone(),
        file_type: meta.file_type,
        capabilities: ModelCapabilities {
            vision: projector.is_some_and(meta::ModelMeta::sees),
            audio: projector.is_some_and(meta::ModelMeta::hears),
            tools: meta.tools,
            thinking: meta.thinking,
            fim: meta.fim,
            base: meta.base,
        },
        installed_at,
    }
}

impl Entry {
    /// Nouvelle progression montrée : `Downloading` à la première,
    /// `Finishing` quand tout est reçu.
    fn progress(&mut self, completed: u64, total: u64) -> Change {
        let download = &mut self.download;
        download.total = download.total.max(total);
        download.completed = completed.min(download.total);
        if download.state == HubDownloadState::Finishing {
            return Change::Nothing;
        }
        if download.completed >= download.total && download.total > 0 {
            download.state = HubDownloadState::Finishing;
            download.bytes_per_second = None;
            return Change::State;
        }
        download.bytes_per_second = self.rate.add(Instant::now(), download.completed);
        if download.state == HubDownloadState::Starting {
            download.state = HubDownloadState::Downloading;
            Change::State
        } else {
            Change::Progress
        }
    }
}

/// Se termine quand l'arrêt est demandé.
async fn cancelled(cancel: &mut watch::Receiver<Stop>) {
    if cancel.wait_for(|stop| *stop != Stop::Run).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Fichier `file` impossible à écrire ; `error` : détail technique.
fn save_failed(file: &PlanFile, error: &dyn std::fmt::Display) -> End {
    End::Failed(t!(
        "speechDownloads.saveFailed",
        file = file.path,
        error = error
    ))
}

/// Taille et empreinte SHA-256 (en hexadécimal) de `path`.
fn verify(path: &Path, size: u64, sha256: &str) -> std::io::Result<bool> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    if file.metadata()?.len() != size {
        return Ok(false);
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(hex.eq_ignore_ascii_case(sha256))
}

/// Débit moyen sur les ~3 dernières secondes.
#[derive(Default)]
struct Rate {
    samples: VecDeque<(Instant, u64)>,
}

impl Rate {
    /// Ajoute un relevé ; débit en octets par seconde, une fois au moins
    /// 0,5 s observée.
    fn add(&mut self, now: Instant, completed: u64) -> Option<u64> {
        self.samples.push_back((now, completed));
        // Garde le plus récent relevé d'au moins 3 s : la fenêtre reste pleine.
        while self.samples.len() > 2 && now.duration_since(self.samples[1].0) >= RATE_WINDOW {
            self.samples.pop_front();
        }
        let &(start, first) = self.samples.front()?;
        let span = now.duration_since(start);
        if span < RATE_MIN_SPAN {
            return None;
        }
        let bytes = completed.saturating_sub(first) as f64;
        Some((bytes / span.as_secs_f64()).round() as u64)
    }
}

#[cfg(test)]
mod tests;
