//! Moteur de texte de ChatonLM : `llama-server` de llama.cpp, livré dans les
//! paquets (`packaging/llama.env`, `scripts/build-llama-server.sh`) et lancé
//! par l'app, **un processus par modèle chargé** (l'app fait ce que faisait
//! l'ordonnanceur d'Ollama ; contrat `.tools/research/101-llamacpp-L3-L6-R1-
//! contract.md` § 1).
//!
//! - [`binary`] : où est le binaire livré ; [`options`] : règles pures
//!   (contexte, `keep_alive`, arguments, environnement) ; [`journal`] : ce que
//!   dit le journal (mémoire, erreurs, repli sur le processeur) ; `launch` :
//!   un processus ; [`auto`] : choix « Auto » des pages Correction et Étude.
//!
//! Ordonnanceur ([`Engine`], état de l'app) :
//! - [`Engine::ensure`] lance le modèle s'il ne l'est pas (ou le relance si le
//!   contexte ou le processeur demandés diffèrent, comme le `needsReload`
//!   d'Ollama : seulement une fois ses requêtes finies), attend qu'il soit
//!   prêt (un chargement à froid dépasse la minute sur un disque lent :
//!   [`Timing::ready_timeout`] = 10 min) et rend un [`Lease`]. Tant qu'un
//!   `Lease` vit, le modèle n'est ni évincé ni arrêté par son minuteur ; à la
//!   fin du dernier, le minuteur `keep_alive` part. Un chargement est fait
//!   par une tâche du moteur : abandonner `ensure` n'arrête pas le chargement
//!   (le modèle reste ensuite le temps de son `keep_alive`, comme chez
//!   Ollama).
//! - Un lancement à la fois. Avant, si le modèle ne tient pas dans la mémoire
//!   libre (VRAM, ou RAM pour le processeur ; estimation `hub/fit.rs`), ou si
//!   [`Config::max_loaded`] modèles sont déjà là, les modèles inactifs les
//!   moins récemment servis sont arrêtés (jamais un modèle tenu par un
//!   `Lease`) ; ensuite `--fit` (défaut de b12110) répartit les couches entre
//!   la carte et le processeur.
//! - Repli sur le processeur : un lancement sur la carte qui meurt au
//!   chargement en accusant Vulkan, ou qui plante, est refait une fois sans
//!   carte (`-ngl 0`, aucune carte Vulkan visible) ; le modèle le garde pour
//!   la session ([`RunningModel::cpu_fallback`]).
//! - Pack CUDA ([`cuda`], facultatif) : avant Vulkan quand il est installé,
//!   activé et la carte compatible ; un lancement sur CUDA qui meurt en
//!   l'accusant, ou qui plante, coupe CUDA pour la session et est refait sur
//!   Vulkan (puis, au besoin, sur le processeur).
//! - Mort d'un processus : son modèle disparaît de la liste (un `Lease` en
//!   cours voit ses requêtes échouer ; un nouvel `ensure` le relance).
//! - Chaque changement (chargement, prêt, requête finie, déchargement, mort)
//!   est annoncé par [`Engine::on_change`] (événement [`MODELS_EVENT`] de la
//!   page).

pub mod auto;
pub mod binary;
pub mod cuda;
pub mod journal;
mod launch;
pub mod options;

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use serde::Serialize;
use tokio::sync::watch;
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::hub::hardware::{Detector, Machine};
use crate::i18n::t;
use crate::inference::Endpoint;
use crate::local_models::LocalModel;
use crate::local_models::id::same_model;
use crate::process::Spawner;
use crate::store::Store;
use options::Spec;
pub use options::{KeepAlive, LoadOptions};

/// Événement de la page : la liste complète des modèles du moteur à chaque
/// changement.
pub const MODELS_EVENT: &str = "engine-models";

/// Modèles chargés au plus à la fois (`OLLAMA_MAX_LOADED_MODELS` d'Ollama :
/// 3 par carte, 3 sur le processeur).
pub const MAX_LOADED: usize = 3;

/// Marge laissée libre sur la carte par `--fit` (`--fit-target`, 1 024 Mio
/// par défaut) : comptée avant de décider s'il faut faire de la place.
const FIT_TARGET: u64 = 1024 * 1024 * 1024;

/// Délai minimal avant de décharger un modèle qui vient d'être prêt : le
/// demandeur prend son `Lease` juste après (même avec `KeepAlive::Unload`).
const READY_GRACE: Duration = Duration::from_secs(2);

/// Fraîcheur de la mémoire de la machine lue avant un lancement
/// (`nvidia-smi`…).
const MACHINE_KEEP: Duration = Duration::from_secs(2);

/// État d'un modèle du moteur.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "EngineModelState")]
pub enum ModelState {
    /// Processus lancé, modèle en cours de chargement.
    Loading,
    Ready,
}

/// Un modèle du moteur (onglet Models, jauge de la VRAM, chef de la carte
/// graphique).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "EngineRunningModel")]
pub struct RunningModel {
    /// Identifiant de la base locale (`<auteur>/<dépôt>:<QUANT>`).
    pub id: String,
    pub state: ModelState,
    /// Mémoire réservée au chargement (RAM et VRAM), en octets, lue dans le
    /// journal ; avant (ou sans) mesure : l'estimation.
    pub size_bytes: u64,
    /// Part sur la carte graphique.
    pub vram_bytes: u64,
    /// Contexte du processus (`-c`).
    pub context_length: u32,
    /// Échéance du déchargement (ms depuis 1970) ; `None` : gardé (`kept`)
    /// ou en cours de chargement. Pendant une requête : l'échéance si elle
    /// finissait maintenant.
    pub expires_at: Option<i64>,
    /// Gardé jusqu'au déchargement (`keep_alive` négatif).
    pub kept: bool,
    /// Une requête (un `Lease`) le tient.
    pub in_use: bool,
    /// Couches sur la carte, sur le total (`offloaded N/M layers`) ; `None` :
    /// inconnu (chargement, processeur seul).
    pub gpu_layers: Option<u32>,
    pub total_layers: Option<u32>,
    /// Sur le processeur après l'échec de la carte (repli).
    pub cpu_fallback: bool,
    /// Ce repli, en clair (langue du moment) ; `None` sans repli.
    pub notice: Option<String>,
    /// Sur le processeur seulement (demandé, ou repli).
    pub cpu_only: bool,
    /// Taille annoncée (« 0.6B »).
    pub params_label: Option<String>,
}

/// Échec du moteur ; messages dans la langue du moment, [`EngineError::code`]
/// pour la page.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum EngineError {
    /// Pas de binaire `llama-server`.
    #[error("{}", t!("engine.unavailable"))]
    Unavailable,
    /// Modèle absent de la base locale.
    #[error("{}", t!("engine.notInstalled", model = _0))]
    NotInstalled(String),
    #[error("{}", t!("engine.missingFile", path = _0.display().to_string()))]
    MissingFile(PathBuf),
    /// Le processus n'a pas démarré ou a refusé le modèle (raison du
    /// journal).
    #[error("{}", t!("engine.startFailed", reason = _0))]
    StartFailed(String),
    /// Pas prêt dans le délai (secondes).
    #[error("{}", t!("engine.notReady", seconds = _0))]
    NotReady(u64),
    /// Le processus a planté au chargement (aussi sur le processeur).
    #[error("{}", t!("engine.crashed", reason = _0))]
    Crashed(String),
    /// Déchargé pendant son chargement (bouton, suppression du modèle).
    #[error("{}", t!("engine.unloaded"))]
    Unloaded,
    /// Fin de l'app : plus aucun lancement.
    #[error("{}", t!("engine.closed"))]
    Closed,
}

impl EngineError {
    /// Code stable, pour la page et les essais.
    pub fn code(&self) -> &'static str {
        match self {
            EngineError::Unavailable => "unavailable",
            EngineError::NotInstalled(_) => "notInstalled",
            EngineError::MissingFile(_) => "missingFile",
            EngineError::StartFailed(_) => "startFailed",
            EngineError::NotReady(_) => "notReady",
            EngineError::Crashed(_) => "crashed",
            EngineError::Unloaded => "unloaded",
            EngineError::Closed => "closed",
        }
    }
}

impl From<EngineError> for AppError {
    fn from(error: EngineError) -> Self {
        match error {
            EngineError::NotInstalled(_) => AppError::NotFound(error.to_string()),
            error => AppError::System(error.to_string()),
        }
    }
}

/// Délais (raccourcis par les essais).
#[derive(Clone, Debug)]
pub struct Timing {
    /// Attente du modèle prêt (chargement à froid compris).
    pub ready_timeout: Duration,
    pub ready_poll: Duration,
    /// Entre SIGTERM et SIGKILL (sous Windows : sans objet).
    pub stop_grace: Duration,
    pub kill_wait: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            ready_timeout: Duration::from_secs(600),
            ready_poll: Duration::from_millis(200),
            stop_grace: Duration::from_secs(3),
            kill_wait: Duration::from_secs(2),
        }
    }
}

/// Réglages des processus.
#[derive(Clone, Debug)]
pub struct Config {
    /// Binaire ; `None` : moteur indisponible.
    pub bin: Option<PathBuf>,
    /// `<données>/logs` : journaux `llama-<n>.log`, et dossier de travail.
    pub logs: PathBuf,
    /// `<données>/engine/cache` (`LLAMA_CACHE`).
    pub cache: PathBuf,
    /// Essais : arguments placés avant ceux du serveur (relance du binaire
    /// d'essai sur le faux) et variable qui porte ceux du serveur.
    pub prefix_args: Vec<OsString>,
    pub args_in_env: Option<String>,
    /// Variables ajoutées en dernier (essais).
    pub extra_env: Vec<(OsString, OsString)>,
    pub timing: Timing,
    pub max_loaded: usize,
    /// Tout lancement sur le processeur, comme demandé (pas un repli) :
    /// `CHATONLM_LLAMA_CPU_ONLY=1` des essais de l'app
    /// ([`options::forced_cpu`]).
    pub force_cpu: bool,
    /// Pack CUDA (`<données>/engine/cuda`).
    pub cuda: cuda::Runtime,
}

impl Config {
    /// Réglages de l'app : `data_dir` = dossier des données.
    pub fn new(bin: Option<PathBuf>, data_dir: &Path) -> Config {
        Config {
            bin,
            logs: data_dir.join("logs"),
            cache: data_dir.join("engine").join("cache"),
            prefix_args: Vec::new(),
            args_in_env: None,
            extra_env: Vec::new(),
            timing: Timing::default(),
            max_loaded: MAX_LOADED,
            force_cpu: options::forced_cpu(),
            cuda: cuda::Runtime::new(data_dir.join("engine").join("cuda")),
        }
    }
}

/// Ce que le moteur lit de l'app : modèles installés et mémoire de la
/// machine (remplacés dans les essais).
pub trait Models: Send + Sync + 'static {
    fn local<'a>(&'a self, id: &'a str) -> BoxFuture<'a, AppResult<Option<LocalModel>>>;
    /// Machine lue il y a [`MACHINE_KEEP`] au plus.
    fn machine(&self) -> BoxFuture<'_, Option<Machine>>;
    /// Machine lue maintenant : la mémoire libre avant un lancement (un
    /// modèle prêt depuis la dernière lecture y est compté).
    fn machine_now(&self) -> BoxFuture<'_, Option<Machine>>;
}

/// Modèles de la base locale et machine détectée (`CHATONLM_E2E_HARDWARE`
/// dans les e2e).
pub struct AppModels {
    store: Option<Store>,
    data_dir: PathBuf,
    detector: Detector,
}

impl AppModels {
    pub fn new(store: Option<Store>, data_dir: PathBuf) -> AppModels {
        AppModels {
            store,
            data_dir,
            detector: Detector::default(),
        }
    }
}

impl Models for AppModels {
    fn local<'a>(&'a self, id: &'a str) -> BoxFuture<'a, AppResult<Option<LocalModel>>> {
        Box::pin(async move {
            match &self.store {
                Some(store) => crate::local_models::get(store, &self.data_dir, id).await,
                None => Ok(None),
            }
        })
    }

    fn machine(&self) -> BoxFuture<'_, Option<Machine>> {
        Box::pin(async move {
            #[cfg(any(debug_assertions, feature = "e2e"))]
            if let Some(injected) = crate::hub::hardware::injected() {
                return injected;
            }
            self.detector.machine_within(MACHINE_KEEP).await
        })
    }

    fn machine_now(&self) -> BoxFuture<'_, Option<Machine>> {
        Box::pin(async move {
            #[cfg(any(debug_assertions, feature = "e2e"))]
            if let Some(injected) = crate::hub::hardware::injected() {
                return injected;
            }
            self.detector.machine_within(Duration::ZERO).await
        })
    }
}

/// Arrêts de processus en cours (y compris en tâche de fond) : un nouveau
/// lancement attend leur fin, pour ne pas compter la mémoire d'un processus
/// qui meurt (ni rouvrir son journal sous Windows).
#[derive(Clone)]
pub(crate) struct Stopping(Arc<watch::Sender<usize>>);

impl Default for Stopping {
    fn default() -> Self {
        Stopping(Arc::new(watch::channel(0).0))
    }
}

impl Stopping {
    fn begin(&self) -> StopToken {
        self.0.send_modify(|count| *count += 1);
        StopToken(self.clone())
    }

    async fn settled(&self, limit: Duration) {
        let mut count = self.0.subscribe();
        let _ = tokio::time::timeout(limit, count.wait_for(|count| *count == 0)).await;
    }
}

struct StopToken(Stopping);

impl Drop for StopToken {
    fn drop(&mut self) {
        self.0
            .0
            .send_modify(|count| *count = count.saturating_sub(1));
    }
}

/// Un modèle tenu prêt pour des requêtes : tant qu'il vit, le modèle n'est
/// ni évincé ni arrêté par son minuteur. Lâché, le minuteur `keep_alive`
/// repart (ou le modèle s'arrête, pour [`KeepAlive::Unload`]).
pub struct Lease {
    /// Adresse et clé du processus.
    pub endpoint: Endpoint,
    /// Contexte du processus (jetons).
    pub ctx: u32,
    /// Identifiant du modèle.
    pub model: String,
    /// Sur le processeur après l'échec de la carte.
    pub cpu_fallback: bool,
    /// Posé quand ce processus est arrêté à la demande ([`Engine::unload`],
    /// [`Engine::unload_all`], [`Engine::shutdown`]).
    unloaded: Arc<AtomicBool>,
    _guard: LeaseGuard,
}

impl Lease {
    /// Le moteur a déchargé ce processus à la demande (bouton, suppression,
    /// fin de l'app) : une coupure de ses requêtes est un arrêt, pas une
    /// panne (contrat 102 § 1.2). Jamais vrai pour une mort du processus.
    pub fn unloaded(&self) -> bool {
        self.unloaded.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lease")
            .field("endpoint", &self.endpoint)
            .field("ctx", &self.ctx)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

struct LeaseGuard {
    inner: Weak<Inner>,
    generation: u64,
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner.release(self.generation);
        }
    }
}

type Listener = Arc<dyn Fn(Vec<RunningModel>) + Send + Sync>;

/// Moteur de texte, partagé par l'app (état Tauri).
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

struct Inner {
    config: Config,
    models: Arc<dyn Models>,
    /// Fil qui crée les processus (PDEATHSIG suit ce fil).
    spawner: parking_lot::Mutex<Option<Spawner>>,
    state: parking_lot::Mutex<State>,
    /// Un lancement à la fois (mémoire comptée sans course).
    launching: tokio::sync::Mutex<()>,
    stopping: Stopping,
    listener: parking_lot::Mutex<Option<Listener>>,
    closed: watch::Sender<bool>,
    /// Gels en cours ([`Engine::freeze`]) : aucun lancement tant qu'il y en a.
    frozen: watch::Sender<usize>,
    vram_total: parking_lot::Mutex<Option<u64>>,
    /// Soi-même, pour les tâches et les `Lease`.
    this: Weak<Inner>,
    #[cfg(test)]
    hooks: parking_lot::Mutex<Hooks>,
}

/// Crochets des essais (courses impossibles à provoquer autrement).
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Hooks {
    /// Appelé quand le processus `pid` vient d'être vu prêt, avant
    /// `loaded()`.
    pub after_ready: Option<Arc<dyn Fn(u32) + Send + Sync>>,
    /// Appelé quand le processus `pid` vient d'être créé, avant d'être noté.
    pub before_attach: Option<Arc<dyn Fn(u32) + Send + Sync>>,
    /// La fin d'un processus n'est pas encore vue par le moteur.
    pub ignore_exits: bool,
    /// Attendu par [`Engine::lease_if_ready`] juste avant de prêter (un
    /// déchargement glissé entre la vérification et la prise).
    pub before_lend: Option<Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>>,
}

#[derive(Default)]
struct State {
    instances: Vec<Instance>,
    next_generation: u64,
    /// Modèles dont la carte a échoué : sur le processeur pour la session.
    gpu_failed: HashSet<String>,
    /// Modèles en cours d'effacement : aucun lancement.
    deleting: Vec<String>,
}

/// Résultat d'un chargement, pour ceux qui l'attendent.
type Outcome = Option<Result<(), EngineError>>;

enum Phase {
    Loading,
    Ready,
}

struct Instance {
    generation: u64,
    id: String,
    spec: Spec,
    params_label: Option<String>,
    /// Numéro du journal (`llama-<n>.log`).
    slot: usize,
    phase: Phase,
    process: Option<Arc<launch::Process>>,
    leases: usize,
    /// Demandes qui attendent ce chargement pour prendre leur `Lease` : tout
    /// juste prêt, le modèle leur est réservé (ni évincé ni libéré).
    waiters: usize,
    /// Tombe à zéro : relu par ceux qui attendent pour recharger autrement.
    idle: watch::Sender<usize>,
    keep_alive: KeepAlive,
    last_used: Instant,
    expires_at: Option<i64>,
    /// Numéro du minuteur valable (les plus anciens ne font rien).
    timer: u64,
    estimate: u64,
    memory: journal::Memory,
    cpu_fallback: bool,
    outcome: watch::Sender<Outcome>,
    /// Vrai pendant qu'un processus est créé mais pas encore noté
    /// (`process` encore vide) : un arrêt l'attend.
    creating: watch::Receiver<bool>,
    /// Déchargé à la demande (partagé avec ses `Lease`).
    unloaded: Arc<AtomicBool>,
}

impl Instance {
    fn view(&self, now_ms: i64) -> RunningModel {
        let duration = self.keep_alive.duration();
        let in_use = self.leases > 0;
        let expires_at = match (&self.phase, in_use, duration) {
            (Phase::Loading, _, _) | (_, _, None) => None,
            (Phase::Ready, true, Some(duration)) => Some(now_ms.saturating_add(millis(duration))),
            (Phase::Ready, false, Some(_)) => self.expires_at,
        };
        RunningModel {
            id: self.id.clone(),
            state: match self.phase {
                Phase::Loading => ModelState::Loading,
                Phase::Ready => ModelState::Ready,
            },
            size_bytes: if self.memory.measured {
                self.memory.total
            } else {
                self.estimate
            },
            vram_bytes: self.memory.gpu,
            context_length: self.spec.ctx,
            expires_at,
            kept: duration.is_none(),
            in_use,
            gpu_layers: self.memory.gpu_layers,
            total_layers: self.memory.total_layers,
            cpu_fallback: self.cpu_fallback,
            notice: self.cpu_fallback.then(|| t!("engine.cpuFallback")),
            cpu_only: self.spec.cpu,
            params_label: self.params_label.clone(),
        }
    }

    fn idle(&self) -> bool {
        self.leases == 0 && self.waiters == 0 && matches!(self.phase, Phase::Ready)
    }
}

fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// Une demande qui attend un chargement de son `Spec` (compte de
/// [`Instance::waiters`], rendu en fin d'attente ou à l'abandon).
struct WaitGuard {
    inner: Weak<Inner>,
    generation: u64,
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner.unwait(self.generation);
        }
    }
}

/// Effacement d'un modèle en cours ([`Engine::deleting`]) : tant qu'il vit,
/// le modèle n'est pas relancé.
pub struct Deleting {
    inner: Weak<Inner>,
    id: String,
}

impl Drop for Deleting {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            let mut state = inner.state.lock();
            if let Some(index) = state.deleting.iter().position(|id| *id == self.id) {
                state.deleting.swap_remove(index);
            }
        }
    }
}

/// Gel du moteur ([`Engine::freeze`]) : levé quand il est lâché.
pub struct Frozen {
    inner: Weak<Inner>,
}

impl Drop for Frozen {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner
                .frozen
                .send_modify(|count| *count = count.saturating_sub(1));
        }
    }
}

/// Prochaine étape de `ensure`, décidée sous le verrou.
enum Next {
    Lease(Lease),
    /// Refusé (modèle en cours d'effacement).
    Fail(EngineError),
    /// Chargement en cours : attendre son résultat. `own` : de ce `Spec`
    /// (son échec est celui de la demande) ; sinon, d'un autre réglage du
    /// même modèle (son échec laisse la demande lancer le sien).
    Wait {
        outcome: watch::Receiver<Outcome>,
        own: bool,
        guard: Option<WaitGuard>,
    },
    /// Le modèle sert avec d'autres options : attendre la fin de ses
    /// requêtes.
    WaitIdle(watch::Receiver<usize>),
    /// Le modèle chargé autrement, inactif : l'arrêter, puis relancer.
    Stop(Instance),
    /// Dossier des modèles en déplacement : attendre, puis tout relire.
    Frozen,
}

/// Contexte qu'une demande accepte d'une instance déjà là.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fit {
    /// Celui demandé, exactement (contexte imposé : FIM, Étude, visite).
    Exact,
    /// Au moins celui demandé ([`Engine::ensure_at_least`]).
    AtLeast,
}

impl Fit {
    /// `busy` : l'instance sert déjà quelqu'un (requêtes en cours, ou
    /// chargement) ; sinon on l'attendrait. Libre, elle est relancée avec le
    /// contexte demandé, comme avant : un contexte réglé plus petit reprend
    /// effet (mémoire rendue).
    fn accepts(self, running: u32, wanted: u32, busy: bool) -> bool {
        match self {
            Fit::Exact => running == wanted,
            Fit::AtLeast => running == wanted || (busy && running > wanted),
        }
    }
}

impl Engine {
    pub fn new(config: Config, models: Arc<dyn Models>) -> Engine {
        Engine {
            inner: Arc::new_cyclic(|this| Inner {
                config,
                models,
                spawner: parking_lot::Mutex::new(None),
                state: parking_lot::Mutex::new(State::default()),
                launching: tokio::sync::Mutex::new(()),
                stopping: Stopping::default(),
                listener: parking_lot::Mutex::new(None),
                closed: watch::channel(false).0,
                frozen: watch::channel(0).0,
                vram_total: parking_lot::Mutex::new(None),
                this: this.clone(),
                #[cfg(test)]
                hooks: parking_lot::Mutex::new(Hooks::default()),
            }),
        }
    }

    /// Un binaire est là.
    pub fn available(&self) -> bool {
        self.inner
            .config
            .bin
            .as_deref()
            .is_some_and(crate::process::is_executable)
    }

    /// Appelé à chaque changement de la liste, hors de tout verrou.
    pub fn on_change(&self, listener: impl Fn(Vec<RunningModel>) + Send + Sync + 'static) {
        *self.inner.listener.lock() = Some(Arc::new(listener));
    }

    /// Modèles du moteur, du plus récemment servi au plus ancien.
    pub fn running(&self) -> Vec<RunningModel> {
        self.inner.snapshot()
    }

    /// Modèle `id` prêt pour des requêtes, avec ces options : lancé s'il ne
    /// l'est pas, relancé si son contexte ou son processeur diffèrent (une fois
    /// ses requêtes finies). Le `keep_alive` de la dernière demande vaut.
    pub async fn ensure(&self, id: &str, options: LoadOptions) -> Result<Lease, EngineError> {
        self.ensure_fit(id, options, Fit::Exact).await
    }

    /// Comme [`Engine::ensure`], pour un appelant qui n'impose pas son
    /// contexte (chat, Correction, nettoyage de la dictée, bouton Load) : une
    /// instance du même modèle en cours d'emploi (requêtes ou chargement),
    /// sur le même processeur, avec un contexte plus grand que celui demandé,
    /// est reprise telle quelle (le `Lease` porte son contexte) au lieu
    /// d'attendre la fin de ses requêtes pour la relancer plus petite. Libre,
    /// elle est relancée au contexte demandé comme par `ensure`. Sans cela, le chat
    /// attendait tout un palier de l'Étude (contexte imposé ≥ 8 192) sans
    /// borne, puis relançait le modèle (relecture L5a, B1).
    pub async fn ensure_at_least(
        &self,
        id: &str,
        options: LoadOptions,
    ) -> Result<Lease, EngineError> {
        self.ensure_fit(id, options, Fit::AtLeast).await
    }

    async fn ensure_fit(
        &self,
        id: &str,
        options: LoadOptions,
        fit: Fit,
    ) -> Result<Lease, EngineError> {
        let inner = &self.inner;
        let mut closed = inner.closed.subscribe();
        if *closed.borrow_and_update() {
            return Err(EngineError::Closed);
        }
        if inner.config.bin.is_none() {
            return Err(EngineError::Unavailable);
        }
        // Dossier des modèles en déplacement ([`Engine::freeze`]) : attendre
        // sa fin, puis relire le modèle (son chemin a pu changer).
        'resolve: loop {
            let mut frozen = inner.frozen.subscribe();
            tokio::select! {
                _ = closed.wait_for(|closed| *closed) => return Err(EngineError::Closed),
                _ = frozen.wait_for(|count| *count == 0) => {}
            }
            let model = inner
                .models
                .local(id)
                .await
                .map_err(|error| EngineError::StartFailed(error.to_string()))?
                .ok_or_else(|| EngineError::NotInstalled(id.to_owned()))?;
            let vram_total = match options.ctx.filter(|ctx| *ctx > 0) {
                // Contexte demandé : la machine ne sert pas ici.
                Some(_) => 0,
                None => inner.vram_total().await,
            };
            let ctx = options::context_for(
                options.ctx,
                vram_total,
                model.context_max,
                model.capabilities.vision,
            );
            // Place réservée dans le chargement attendu : rendue seulement après
            // le tour suivant (le `Lease` pris, ou une autre attente notée).
            let mut _waiting: Option<WaitGuard> = None;
            loop {
                if *closed.borrow() {
                    return Err(EngineError::Closed);
                }
                let next = inner.next(&model, ctx, options, fit);
                match next {
                    Next::Lease(lease) => {
                        inner.changed();
                        return Ok(lease);
                    }
                    Next::Fail(error) => return Err(error),
                    Next::Frozen => continue 'resolve,
                    Next::Wait {
                        mut outcome,
                        own,
                        guard,
                    } => {
                        _waiting = guard;
                        let finished = tokio::select! {
                            _ = closed.wait_for(|closed| *closed) => return Err(EngineError::Closed),
                            outcome = outcome.wait_for(Option::is_some) => outcome.map(|o| o.clone()),
                        };
                        match finished {
                            Ok(Some(Err(error))) if own => return Err(error),
                            Err(_) if own => return Err(EngineError::Unloaded),
                            // Prêt : le `Lease` est pris au tour suivant. Échec
                            // d'un autre réglage : le sien est lancé au tour
                            // suivant (l'erreur d'autrui n'est pas la sienne).
                            Ok(_) | Err(_) => {}
                        }
                    }
                    Next::WaitIdle(mut leases) => {
                        _waiting = None;
                        tokio::select! {
                            _ = closed.wait_for(|closed| *closed) => return Err(EngineError::Closed),
                            _ = leases.wait_for(|leases| *leases == 0) => {}
                        }
                    }
                    Next::Stop(instance) => {
                        _waiting = None;
                        inner.changed();
                        inner.stop(instance).await;
                    }
                }
            }
        }
    }

    /// Préchargement (réglage « Preload the chat model », bouton Load) :
    /// [`Engine::ensure`] sans requête ; le minuteur `keep_alive` part aussitôt.
    pub async fn preload(&self, id: &str, options: LoadOptions) -> Result<(), EngineError> {
        self.ensure(id, options).await.map(drop)
    }

    /// Arrête le modèle `id` (même en cours de chargement, même tenu par un
    /// `Lease` : ses requêtes échouent) et attend la fin de son processus
    /// (sous Windows, ses fichiers sont alors libres). Sans effet s'il n'est
    /// pas chargé. Voulu pour le bouton Unload et la suppression du modèle :
    /// un flux en cours sur ce `Lease` voit sa connexion coupée (erreur de
    /// transport de `inference`), que l'appelant (L5a) traite comme un arrêt
    /// de la réponse, pas comme une panne ; un `ensure` qui attendait le
    /// chargement reçoit `EngineError::Unloaded`. L'éviction pour faire de la
    /// place, elle, ne touche jamais un modèle tenu.
    pub async fn unload(&self, id: &str) {
        let taken = self.inner.take(|instance| same_model(&instance.id, id));
        mark_unloaded(&taken);
        self.inner.stop_all(taken).await;
    }

    /// Effacement de `id` (suppression du modèle) : tant que le résultat
    /// vit, `ensure` le refuse (`NotInstalled`), pour qu'aucune demande ne le
    /// relance entre son déchargement et l'effacement de ses fichiers.
    pub fn deleting(&self, id: &str) -> Deleting {
        self.inner.state.lock().deleting.push(id.to_owned());
        Deleting {
            inner: Arc::downgrade(&self.inner),
            id: id.to_owned(),
        }
    }

    /// Efface les journaux du modèle `id` (`llama-<id>.log` et `.1`), à sa
    /// suppression : à appeler sous [`Engine::deleting`], une fois le modèle
    /// déchargé (aucun processus n'y écrit). Sans cela, ils restaient sous
    /// `<données>/logs`, jusqu'à 32 Mio par modèle essayé (relecture L5a, M3).
    pub fn remove_logs(&self, id: &str) {
        let log = self.inner.config.logs.join(journal::file_name(id));
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_file(log.with_extension("log.1"));
    }

    /// Gèle le moteur et arrête tous les modèles (attendus finis), pour un
    /// déplacement du dossier des modèles : tant que le résultat vit, aucun
    /// modèle n'est lancé ni prêté ; une demande attend la fin, puis relit le
    /// modèle (nouveau chemin). À prendre seulement une fois le changement
    /// accepté (relecture L5b, M4 : la page déchargeait tout AVANT les refus,
    /// et rien n'empêchait une relance pendant la copie).
    pub async fn freeze(&self) -> Frozen {
        {
            // Sous le verrou : un `next` d'après voit le gel ; ce qui a été
            // lancé avant est arrêté ci-dessous.
            let _state = self.inner.state.lock();
            self.inner.frozen.send_modify(|count| *count += 1);
        }
        let frozen = Frozen {
            inner: Arc::downgrade(&self.inner),
        };
        self.unload_all().await;
        frozen
    }

    /// Arrête tous les modèles.
    pub async fn unload_all(&self) {
        let taken = self.inner.take(|_| true);
        mark_unloaded(&taken);
        self.inner.stop_all(taken).await;
    }

    /// Le modèle `id` est prêt avec ces options (même contexte calculé, même
    /// processeur) et vivant : [`Engine::ensure`] rendrait un `Lease` sans
    /// attendre de chargement. Lecture seule (rien n'est lancé).
    pub async fn is_ready(&self, id: &str, options: LoadOptions) -> bool {
        self.ready_fit(id, options, Fit::Exact).await
    }

    /// [`Engine::is_ready`] pour [`Engine::ensure_at_least`] : un contexte
    /// plus grand convient.
    pub async fn is_ready_at_least(&self, id: &str, options: LoadOptions) -> bool {
        self.ready_fit(id, options, Fit::AtLeast).await
    }

    async fn ready_fit(&self, id: &str, options: LoadOptions, fit: Fit) -> bool {
        let inner = &self.inner;
        if inner.config.bin.is_none() || *inner.closed.borrow() {
            return false;
        }
        let Ok(Some(model)) = inner.models.local(id).await else {
            return false;
        };
        let vram_total = match options.ctx.filter(|ctx| *ctx > 0) {
            Some(_) => 0,
            None => inner.vram_total().await,
        };
        let ctx = options::context_for(
            options.ctx,
            vram_total,
            model.context_max,
            model.capabilities.vision,
        );
        let state = inner.state.lock();
        let cpu = inner.wants_cpu(options) || state.gpu_failed.contains(&model.id);
        state.instances.iter().any(|instance| {
            same_model(&instance.id, &model.id)
                && matches!(instance.phase, Phase::Ready)
                && fit.accepts(
                    instance.spec.ctx,
                    ctx,
                    instance.leases > 0 || instance.waiters > 0,
                )
                && instance.spec.cpu == cpu
                && instance
                    .process
                    .as_ref()
                    .is_some_and(|process| process.exited().is_none())
        })
    }

    /// `Lease` du modèle `id` s'il est DÉJÀ prêt avec ces options (même
    /// contexte calculé, même processeur), vivant ; sinon `None`, sans rien
    /// lancer ni attendre. Pour l'autocomplétion, qui ne charge jamais : entre
    /// [`Engine::is_ready`] et [`Engine::ensure`], un déchargement ou une
    /// éviction faisait relancer le modèle (relecture L5a, M1).
    pub async fn lease_if_ready(&self, id: &str, options: LoadOptions) -> Option<Lease> {
        let inner = &self.inner;
        if inner.config.bin.is_none() || *inner.closed.borrow() {
            return None;
        }
        let model = inner.models.local(id).await.ok()??;
        let vram_total = match options.ctx.filter(|ctx| *ctx > 0) {
            Some(_) => 0,
            None => inner.vram_total().await,
        };
        let ctx = options::context_for(
            options.ctx,
            vram_total,
            model.context_max,
            model.capabilities.vision,
        );
        #[cfg(test)]
        let before = inner.hooks.lock().before_lend.clone();
        #[cfg(test)]
        if let Some(before) = before {
            before().await;
        }
        let lease = inner.lend_if_ready(&model, ctx, options);
        if lease.is_some() {
            inner.changed();
        }
        lease
    }

    /// La carte graphique a échoué pendant cette session (un modèle au
    /// moins est passé au processeur) : écran du moteur.
    pub fn gpu_failed(&self) -> bool {
        !self.inner.state.lock().gpu_failed.is_empty()
    }

    /// Binaire du moteur (version, écran du moteur) ; `None` : indisponible.
    pub fn binary(&self) -> Option<PathBuf> {
        self.inner.config.bin.clone()
    }

    /// Pack CUDA du moteur (écran du moteur, état Tauri [`cuda::Cuda`]).
    pub fn cuda(&self) -> &cuda::Runtime {
        &self.inner.config.cuda
    }

    /// Dossier des journaux (`<données>/logs`).
    pub fn logs_dir(&self) -> PathBuf {
        self.inner.config.logs.clone()
    }

    /// Machine vue par le moteur (cartes, mémoire), lecture récente.
    pub async fn machine(&self) -> Option<Machine> {
        self.inner.models.machine().await
    }

    /// Arrête les modèles inactifs (aucune requête en cours, chargement fini) :
    /// leur mémoire revient au moteur d'images. Rend leur nombre.
    pub async fn release_idle(&self) -> usize {
        let taken = self.inner.take(Instance::idle);
        let count = taken.len();
        self.inner.stop_all(taken).await;
        count
    }

    /// Fin de l'app (ou mise à jour) : plus aucun lancement, tous les
    /// processus arrêtés, attente bornée.
    pub async fn shutdown(&self) {
        let inner = &self.inner;
        inner.closed.send_replace(true);
        let limit = inner.stop_limit();
        let stopping = async {
            self.unload_all().await;
            inner.stopping.settled(limit).await;
        };
        if tokio::time::timeout(limit * 2, stopping).await.is_err() {
            // i18n-ignore : journal.
            crate::app_log!("chatonlm: the text engine didn't stop in time; leaving it");
        }
    }

    /// Crochets des essais.
    #[cfg(test)]
    pub(crate) fn hooks(&self) -> parking_lot::MutexGuard<'_, Hooks> {
        self.inner.hooks.lock()
    }

    /// Numéro du processus d'un modèle (essais).
    #[cfg(test)]
    pub(crate) fn pid_of(&self, id: &str) -> Option<u32> {
        let state = self.inner.state.lock();
        state
            .instances
            .iter()
            .find(|instance| same_model(&instance.id, id))
            .and_then(|instance| instance.process.as_ref().map(|process| process.pid))
    }
}

impl Inner {
    fn arc(&self) -> Arc<Inner> {
        self.this.upgrade().expect("engine alive while used")
    }

    /// VRAM totale des cartes (palier du contexte par défaut), gardée dès
    /// qu'une carte est vue. Sans carte (aucune, ou `nvidia-smi` en échec ou
    /// trop lent une fois) : relue la fois suivante, jamais gardée à 0.
    async fn vram_total(&self) -> u64 {
        if let Some(total) = *self.vram_total.lock() {
            return total;
        }
        let Some(machine) = self.models.machine().await else {
            return 0;
        };
        let total = machine.gpus.iter().map(|gpu| gpu.vram_total).sum();
        if !machine.gpus.is_empty() {
            *self.vram_total.lock() = Some(total);
        }
        total
    }

    fn stop_limit(&self) -> Duration {
        let timing = &self.config.timing;
        timing.stop_grace + timing.kill_wait + Duration::from_secs(1)
    }

    fn snapshot(&self) -> Vec<RunningModel> {
        let now = crate::store::now_ms();
        let state = self.state.lock();
        let mut instances: Vec<&Instance> = state.instances.iter().collect();
        instances.sort_by_key(|instance| std::cmp::Reverse(instance.last_used));
        instances.into_iter().map(|i| i.view(now)).collect()
    }

    /// Prévient la page (hors des verrous).
    fn changed(&self) {
        let listener = self.listener.lock().clone();
        if let Some(listener) = listener {
            listener(self.snapshot());
        }
    }

    /// Décide la suite de `ensure` sous le verrou.
    fn next(&self, model: &LocalModel, ctx: u32, options: LoadOptions, fit: Fit) -> Next {
        let mut state = self.state.lock();
        if *self.frozen.borrow() > 0 {
            return Next::Frozen;
        }
        if state.deleting.iter().any(|id| same_model(id, &model.id)) {
            return Next::Fail(EngineError::NotInstalled(model.id.clone()));
        }
        let cpu = self.wants_cpu(options) || state.gpu_failed.contains(&model.id);
        let spec = Spec {
            model: PathBuf::from(&model.path),
            mmproj: model.mmproj.as_deref().map(PathBuf::from),
            ctx,
            cpu,
        };
        let mut found = state
            .instances
            .iter()
            .position(|instance| same_model(&instance.id, &model.id));
        // Prêt mais mort (sa fin pas encore traitée) : jamais de `Lease` vers
        // une adresse morte, relancé comme s'il était absent.
        if let Some(index) = found {
            let instance = &state.instances[index];
            let dead = matches!(instance.phase, Phase::Ready)
                && instance
                    .process
                    .as_ref()
                    .is_some_and(|process| process.exited().is_some());
            if dead {
                state.instances.swap_remove(index);
                found = None;
            }
        }
        if let Some(index) = found {
            let this = self.this.clone();
            let instance = &mut state.instances[index];
            let own = |instance: &mut Instance| {
                instance.waiters += 1;
                Next::Wait {
                    outcome: instance.outcome.subscribe(),
                    own: true,
                    guard: Some(WaitGuard {
                        inner: this.clone(),
                        generation: instance.generation,
                    }),
                }
            };
            let fits = instance.spec == spec
                || (instance.spec.model == spec.model
                    && instance.spec.mmproj == spec.mmproj
                    && instance.spec.cpu == spec.cpu
                    && fit.accepts(
                        instance.spec.ctx,
                        spec.ctx,
                        matches!(instance.phase, Phase::Loading)
                            || instance.leases > 0
                            || instance.waiters > 0,
                    ));
            if fits {
                instance.keep_alive = options.keep_alive;
                return match instance.phase {
                    Phase::Loading => own(instance),
                    Phase::Ready => {
                        let Some(process) = instance.process.clone() else {
                            return own(instance);
                        };
                        Next::Lease(self.lend(instance, &process))
                    }
                };
            }
            // Chargé autrement : relancé une fois libre.
            return match instance.phase {
                Phase::Loading => Next::Wait {
                    outcome: instance.outcome.subscribe(),
                    own: false,
                    guard: None,
                },
                Phase::Ready if instance.leases > 0 => Next::WaitIdle(instance.idle.subscribe()),
                Phase::Ready => Next::Stop(state.instances.swap_remove(index)),
            };
        }
        // Nouveau lancement, fait par une tâche du moteur.
        // Sur le processeur parce que la carte a échoué (pas demandé) : un
        // repli, annoncé comme tel.
        let fallback = cpu && !self.wants_cpu(options);
        let generation = state.next_generation;
        state.next_generation += 1;
        let slot = (0..)
            .find(|slot| !state.instances.iter().any(|i| i.slot == *slot))
            .unwrap_or(0);
        let (outcome, waiting) = watch::channel(None);
        let (creating_tx, creating) = watch::channel(false);
        state.instances.push(Instance {
            generation,
            id: model.id.clone(),
            spec: spec.clone(),
            params_label: model.params_label.clone(),
            slot,
            phase: Phase::Loading,
            process: None,
            leases: 0,
            // Le demandeur, qui attend ce chargement.
            waiters: 1,
            idle: watch::channel(0).0,
            keep_alive: options.keep_alive,
            last_used: Instant::now(),
            expires_at: None,
            timer: 0,
            estimate: model.size_bytes,
            memory: journal::Memory::default(),
            cpu_fallback: fallback,
            outcome,
            creating,
            unloaded: Arc::new(AtomicBool::new(false)),
        });
        drop(state);
        let inner = self.arc();
        let model = model.clone();
        crate::process::spawn_task(async move {
            inner.changed();
            let result = inner.launch(generation, &model, spec, &creating_tx).await;
            inner.loaded(generation, result);
        });
        Next::Wait {
            outcome: waiting,
            own: true,
            guard: Some(WaitGuard {
                inner: self.this.clone(),
                generation,
            }),
        }
    }

    /// Processeur demandé (par l'appelant, ou imposé par la configuration).
    fn wants_cpu(&self, options: LoadOptions) -> bool {
        options.cpu_only || self.config.force_cpu
    }

    /// Prête l'instance prête `instance` (sous le verrou de l'état) : un
    /// `Lease` de plus, minuteur suspendu.
    fn lend(&self, instance: &mut Instance, process: &launch::Process) -> Lease {
        instance.leases += 1;
        instance.idle.send_replace(instance.leases);
        instance.last_used = Instant::now();
        instance.expires_at = None;
        instance.timer += 1;
        Lease {
            endpoint: process.endpoint.clone(),
            ctx: instance.spec.ctx,
            model: instance.id.clone(),
            cpu_fallback: instance.cpu_fallback,
            unloaded: instance.unloaded.clone(),
            _guard: LeaseGuard {
                inner: self.this.clone(),
                generation: instance.generation,
            },
        }
    }

    /// `Lease` de `model` s'il est prêt, vivant, avec exactement ce `Spec`,
    /// décidé sous le verrou : jamais de lancement ni d'attente.
    fn lend_if_ready(&self, model: &LocalModel, ctx: u32, options: LoadOptions) -> Option<Lease> {
        let mut state = self.state.lock();
        if *self.frozen.borrow() > 0 {
            return None;
        }
        if state.deleting.iter().any(|id| same_model(id, &model.id)) {
            return None;
        }
        let cpu = self.wants_cpu(options) || state.gpu_failed.contains(&model.id);
        let spec = Spec {
            model: PathBuf::from(&model.path),
            mmproj: model.mmproj.as_deref().map(PathBuf::from),
            ctx,
            cpu,
        };
        let instance = state
            .instances
            .iter_mut()
            .find(|instance| same_model(&instance.id, &model.id))?;
        if instance.spec != spec || !matches!(instance.phase, Phase::Ready) {
            return None;
        }
        let process = instance.process.clone()?;
        if process.exited().is_some() {
            return None;
        }
        instance.keep_alive = options.keep_alive;
        Some(self.lend(instance, &process))
    }

    fn alive(&self, generation: u64) -> bool {
        !*self.closed.borrow()
            && self
                .state
                .lock()
                .instances
                .iter()
                .any(|instance| instance.generation == generation)
    }

    /// Fin d'un chargement : prêt (minuteur armé, au moins [`READY_GRACE`])
    /// ou retiré ; ceux qui attendent sont prévenus.
    fn loaded(
        &self,
        generation: u64,
        result: Result<(Arc<launch::Process>, journal::Memory), EngineError>,
    ) {
        let outcome = {
            let mut state = self.state.lock();
            let index = state
                .instances
                .iter()
                .position(|instance| instance.generation == generation);
            match (index, result) {
                // Mort entre « prêt » et ici. La fin est publiée avant
                // `exited` (qui ignore un modèle en chargement) : vue ici sous
                // le verrou, ou bien `exited` verra le modèle prêt.
                (Some(index), Ok((process, _))) if process.exited().is_some() => {
                    let instance = state.instances.swap_remove(index);
                    drop(state);
                    let exited = process.exited().unwrap_or_else(launch::Exited::lost);
                    let log = journal::read(&process.log);
                    if !instance.spec.cpu && journal::vulkan_failed(&log) {
                        self.state.lock().gpu_failed.insert(instance.id.clone());
                    }
                    Some((instance, died(&log, &exited)))
                }
                (Some(index), Ok((process, memory))) => {
                    let instance = &mut state.instances[index];
                    instance.phase = Phase::Ready;
                    instance.process = Some(process);
                    instance.memory = memory;
                    instance.last_used = Instant::now();
                    let sender = instance.outcome.clone();
                    let delay = instance.keep_alive.duration().map(|d| d.max(READY_GRACE));
                    drop(state);
                    if let Some(delay) = delay {
                        self.arm(generation, delay);
                    }
                    sender.send_replace(Some(Ok(())));
                    None
                }
                (Some(index), Err(error)) => {
                    let instance = state.instances.swap_remove(index);
                    Some((instance, error))
                }
                // Retiré entre-temps (déchargé, fin de l'app) : son processus
                // est arrêté par celui qui l'a retiré, ou lâché ici.
                (None, Ok(_)) => None,
                (None, Err(_)) => None,
            }
        };
        if let Some((instance, error)) = outcome {
            instance.outcome.send_replace(Some(Err(error)));
        }
        self.changed();
    }

    /// Arme le minuteur du déchargement de `generation` dans `delay`.
    fn arm(&self, generation: u64, delay: Duration) {
        let timer = {
            let mut state = self.state.lock();
            let Some(instance) = state
                .instances
                .iter_mut()
                .find(|instance| instance.generation == generation)
            else {
                return;
            };
            instance.timer += 1;
            instance.expires_at = Some(crate::store::now_ms().saturating_add(millis(delay)));
            instance.timer
        };
        let inner = self.arc();
        crate::process::spawn_task(async move {
            tokio::time::sleep(delay).await;
            let taken = inner.take(|instance| {
                instance.generation == generation && instance.timer == timer && instance.idle()
            });
            inner.stop_all(taken).await;
        });
    }

    /// Fin d'un `Lease`.
    fn release(&self, generation: u64) {
        let delay = {
            let mut state = self.state.lock();
            let Some(instance) = state
                .instances
                .iter_mut()
                .find(|instance| instance.generation == generation)
            else {
                return;
            };
            instance.leases = instance.leases.saturating_sub(1);
            instance.idle.send_replace(instance.leases);
            instance.last_used = Instant::now();
            instance
                .idle()
                .then(|| instance.keep_alive.duration())
                .flatten()
        };
        self.after_use(generation, delay);
    }

    /// Fin d'une attente de chargement (`Lease` pris, ou demande abandonnée) :
    /// un modèle prêt laissé sans demandeur repart comme à la fin d'un `Lease`.
    fn unwait(&self, generation: u64) {
        let delay = {
            let mut state = self.state.lock();
            let Some(instance) = state
                .instances
                .iter_mut()
                .find(|instance| instance.generation == generation)
            else {
                return;
            };
            instance.waiters = instance.waiters.saturating_sub(1);
            if !instance.idle() {
                return;
            }
            instance.keep_alive.duration()
        };
        self.after_use(generation, delay);
    }

    /// Modèle devenu inactif : minuteur `keep_alive`, ou arrêt aussitôt.
    fn after_use(&self, generation: u64, delay: Option<Duration>) {
        match delay {
            Some(Duration::ZERO) => {
                let taken =
                    self.take(|instance| instance.generation == generation && instance.idle());
                let inner = self.arc();
                crate::process::spawn_task(async move { inner.stop_all(taken).await });
            }
            Some(delay) => self.arm(generation, delay),
            None => {}
        }
        self.changed();
    }

    /// Retire de l'état les modèles qui répondent à `which`.
    fn take(&self, which: impl Fn(&Instance) -> bool) -> Vec<Instance> {
        let mut state = self.state.lock();
        let (taken, kept) = std::mem::take(&mut state.instances)
            .into_iter()
            .partition(|instance| which(instance));
        state.instances = kept;
        taken
    }

    /// Arrête un modèle retiré de l'état : son processus (attendu), et son
    /// chargement s'il était en cours (ceux qui l'attendent sont prévenus).
    async fn stop(&self, instance: Instance) {
        if matches!(instance.phase, Phase::Loading) {
            instance
                .outcome
                .send_replace(Some(Err(if *self.closed.borrow() {
                    EngineError::Closed
                } else {
                    EngineError::Unloaded
                })));
        }
        // Processus créé mais pas encore noté : la tâche de lancement
        // l'arrête dès qu'elle voit le modèle retiré ; attendre (borné) qu'il
        // soit fini (sous Windows, ses fichiers sont alors libres).
        let mut creating = instance.creating.clone();
        let _ =
            tokio::time::timeout(self.stop_limit(), creating.wait_for(|creating| !*creating)).await;
        if let Some(process) = &instance.process {
            process.stop().await;
        }
    }

    async fn stop_all(&self, taken: Vec<Instance>) {
        if taken.is_empty() {
            return;
        }
        self.changed();
        futures_util::future::join_all(taken.into_iter().map(|instance| self.stop(instance))).await;
        self.changed();
    }

    /// Lancement de `generation` (un à la fois) : place faite, processus
    /// lancé, attendu prêt ; une fois refait sur le processeur si la carte
    /// échoue.
    async fn launch(
        &self,
        generation: u64,
        model: &LocalModel,
        mut spec: Spec,
        creating: &watch::Sender<bool>,
    ) -> Result<(Arc<launch::Process>, journal::Memory), EngineError> {
        let _one = self.launching.lock().await;
        let gone = || {
            if *self.closed.borrow() {
                EngineError::Closed
            } else {
                EngineError::Unloaded
            }
        };
        if !self.alive(generation) {
            return Err(gone());
        }
        // Un processus arrêté en tâche de fond meurt peut-être encore.
        self.stopping.settled(self.stop_limit()).await;
        self.make_room(generation, model, &spec).await;
        let ram_total = self.models.machine().await.map(|machine| machine.ram_total);
        let cache_ram = options::cache_ram_mib(ram_total);
        let slot = self.slot_of(generation).ok_or_else(gone)?;
        let mut retried = false;
        let mut new_ports = 0;
        loop {
            let this = self.this.clone();
            // Processus en création jusqu'à ce qu'il soit noté (ou arrêté).
            creating.send_replace(true);
            let _created = Created(creating);
            // CUDA choisi à chaque essai (un échec le coupe pour la session).
            let cuda = if spec.cpu {
                None
            } else {
                self.config.cuda.select().await
            };
            let on_cuda = cuda.is_some();
            let process = launch::spawn(
                &launch::Launch {
                    config: &self.config,
                    spawner: &self.spawner,
                    stopping: &self.stopping,
                    cache_ram,
                    cuda,
                },
                &spec,
                &model.id,
                slot,
                move |exited| {
                    if let Some(inner) = this.upgrade() {
                        inner.exited(generation, exited);
                    }
                },
            )
            .await?;
            #[cfg(test)]
            {
                let hook = self.hooks.lock().before_attach.clone();
                if let Some(hook) = hook {
                    hook(process.pid);
                }
            }
            if !self.attach(generation, &process) {
                process.stop().await;
                return Err(gone());
            }
            drop(_created);
            let ready = launch::wait_ready(&process, &self.config, &model.id, spec.ctx, || {
                !self.alive(generation)
            })
            .await;
            match ready {
                Ok(()) => {
                    #[cfg(test)]
                    {
                        let hook = self.hooks.lock().after_ready.clone();
                        if let Some(hook) = hook {
                            hook(process.pid);
                        }
                    }
                    let memory = journal::memory(&journal::read(&process.log));
                    return Ok((process, memory));
                }
                Err(launch::NotReady::Died(exited)) => {
                    let log = journal::read(&process.log);
                    // Port pris entre son choix et la liaison : un autre port.
                    if journal::port_taken(&log) && new_ports < NEW_PORTS {
                        new_ports += 1;
                        // i18n-ignore : journal.
                        crate::app_log!(
                            "chatonlm: port taken for {}; trying another one",
                            model.id
                        );
                        continue;
                    }
                    // CUDA lui-même accusé (pack, pilote, carte) : coupé
                    // pour la session, nouvel essai sur Vulkan. Un manque de
                    // mémoire ou un plantage du modèle sur CUDA : repli de ce
                    // modèle seul sur le processeur (ci-dessous), CUDA gardé.
                    if on_cuda && cuda::journal_accuses_cuda(&log) {
                        let reason = journal::errors(&log).unwrap_or_else(|| exited.text.clone());
                        self.config.cuda.fail(reason);
                        self.changed();
                        continue;
                    }
                    if !retried
                        && !spec.cpu
                        && (journal::retry_on_cpu(&log, exited.facts, cfg!(windows))
                            || (on_cuda && cuda::journal_cuda_out_of_memory(&log)))
                    {
                        let reason = journal::errors(&log).unwrap_or_else(|| exited.text.clone());
                        // i18n-ignore : journal.
                        crate::app_log!(
                            "chatonlm: {} failed on the graphics card ({reason}); retrying on the processor",
                            model.id
                        );
                        retried = true;
                        spec.cpu = true;
                        if !self.fall_back(generation, &model.id) {
                            return Err(gone());
                        }
                        // Le journal du premier essai reste en `.1`.
                        continue;
                    }
                    return Err(died(&log, &exited));
                }
                Err(launch::NotReady::Timeout) => {
                    process.stop().await;
                    // Secondes arrondies au-dessus (jamais « 0 seconde »).
                    let millis = self.config.timing.ready_timeout.as_millis();
                    return Err(EngineError::NotReady(
                        u64::try_from(millis.div_ceil(1000)).unwrap_or(u64::MAX),
                    ));
                }
                Err(launch::NotReady::Cancelled) => {
                    process.stop().await;
                    return Err(gone());
                }
                Err(launch::NotReady::NotOurs) => {
                    process.stop().await;
                    if new_ports < NEW_PORTS {
                        new_ports += 1;
                        // i18n-ignore : journal.
                        crate::app_log!(
                            "chatonlm: another server answered for {}; trying another port",
                            model.id
                        );
                        continue;
                    }
                    return Err(EngineError::StartFailed(t!("engine.detail.portTaken")));
                }
            }
        }
    }

    fn slot_of(&self, generation: u64) -> Option<usize> {
        let state = self.state.lock();
        state
            .instances
            .iter()
            .find(|instance| instance.generation == generation)
            .map(|instance| instance.slot)
    }

    /// Note le processus du lancement en cours ; faux si le modèle a été
    /// retiré entre-temps.
    fn attach(&self, generation: u64, process: &Arc<launch::Process>) -> bool {
        let mut state = self.state.lock();
        match state
            .instances
            .iter_mut()
            .find(|instance| instance.generation == generation)
        {
            Some(instance) => {
                instance.process = Some(process.clone());
                true
            }
            None => false,
        }
    }

    /// Repli sur le processeur noté pour le modèle (et pour la session).
    fn fall_back(&self, generation: u64, id: &str) -> bool {
        let found = {
            let mut state = self.state.lock();
            state.gpu_failed.insert(id.to_owned());
            match state
                .instances
                .iter_mut()
                .find(|instance| instance.generation == generation)
            {
                Some(instance) => {
                    instance.spec.cpu = true;
                    instance.cpu_fallback = true;
                    instance.process = None;
                    true
                }
                None => false,
            }
        };
        self.changed();
        found
    }

    /// Fin d'un processus : s'il est mort de lui-même, son modèle est retiré
    /// (et, si son journal accuse Vulkan, passé au processeur pour la suite).
    fn exited(&self, generation: u64, exited: &launch::Exited) {
        if exited.stopped {
            return;
        }
        #[cfg(test)]
        if self.hooks.lock().ignore_exits {
            return;
        }
        let removed = {
            let mut state = self.state.lock();
            let index = state.instances.iter().position(|instance| {
                instance.generation == generation && matches!(instance.phase, Phase::Ready)
            });
            index.map(|index| state.instances.swap_remove(index))
        };
        let Some(instance) = removed else {
            // En chargement : le lancement s'en charge.
            return;
        };
        let log = instance
            .process
            .as_ref()
            .map(|process| journal::read(&process.log))
            .unwrap_or_default();
        if !instance.spec.cpu && journal::vulkan_failed(&log) {
            self.state.lock().gpu_failed.insert(instance.id.clone());
        }
        // i18n-ignore : journal.
        crate::app_log!(
            "chatonlm: the engine of {} stopped by itself ({}){}",
            instance.id,
            exited.text,
            journal::errors(&log).map_or(String::new(), |errors| format!(": {errors}"))
        );
        self.changed();
    }

    /// Fait de la place avant de lancer `model` : arrête les modèles inactifs
    /// les moins récemment servis tant que [`Config::max_loaded`] est atteint,
    /// ou que le modèle (estimé) ne tient pas dans la mémoire libre.
    async fn make_room(&self, generation: u64, model: &LocalModel, spec: &Spec) {
        // Lecture fraîche : un modèle prêt depuis la dernière lecture (un
        // lancement juste avant celui-ci) occupe déjà la carte.
        let machine = self.models.machine_now().await;
        let on_card = !spec.cpu && machine.as_ref().is_some_and(|m| !m.gpus.is_empty());
        let need =
            estimate(model, spec.ctx)
                .await
                .saturating_add(if on_card { FIT_TARGET } else { 0 });
        // Mémoire libre où le modèle ira : VRAM (inconnue pour une carte AMD
        // sous Windows : rien n'est décidé), ou RAM.
        let free = machine.as_ref().and_then(|machine| {
            if on_card {
                machine
                    .gpus
                    .iter()
                    .map(|gpu| gpu.vram_free)
                    .sum::<Option<u64>>()
            } else {
                Some(machine.ram_available)
            }
        });
        let mut freed: u64 = 0;
        loop {
            let victim = {
                let mut state = self.state.lock();
                let others = state
                    .instances
                    .iter()
                    .filter(|instance| instance.generation != generation)
                    .count();
                let short = free.is_some_and(|free| need > free.saturating_add(freed));
                if others < self.config.max_loaded && !short {
                    None
                } else {
                    state
                        .instances
                        .iter()
                        .enumerate()
                        .filter(|(_, instance)| {
                            instance.generation != generation && instance.idle()
                        })
                        .min_by_key(|(_, instance)| instance.last_used)
                        .map(|(index, _)| index)
                        .map(|index| state.instances.swap_remove(index))
                }
            };
            let Some(victim) = victim else {
                return;
            };
            freed = freed.saturating_add(if on_card {
                victim.memory.gpu
            } else {
                victim.memory.total.saturating_sub(victim.memory.gpu)
            });
            // i18n-ignore : journal.
            crate::app_log!(
                "chatonlm: unloading {} to make room for {}",
                victim.id,
                model.id
            );
            self.changed();
            self.stop(victim).await;
        }
    }
}

/// Nouveaux ports essayés au plus quand le sien est pris (entre son choix et
/// la liaison, ou par un autre serveur qui répond à sa place).
const NEW_PORTS: u32 = 2;

/// Marque « processus en création » levée à la fin de la portée.
struct Created<'a>(&'a watch::Sender<bool>);

impl Drop for Created<'_> {
    fn drop(&mut self) {
        self.0.send_replace(false);
    }
}

/// Note « déchargé à la demande » sur les modèles retirés (leurs `Lease`
/// le voient).
fn mark_unloaded(taken: &[Instance]) {
    for instance in taken {
        instance.unloaded.store(true, Ordering::SeqCst);
    }
}

/// Erreur d'un processus mort au chargement : raison du journal, plantage ou
/// refus.
fn died(log: &str, exited: &launch::Exited) -> EngineError {
    let reason = journal::errors(log).unwrap_or_else(|| exited.text.clone());
    if journal::crashed(exited.facts, cfg!(windows)) {
        EngineError::Crashed(reason)
    } else {
        EngineError::StartFailed(reason)
    }
}

/// Mémoire estimée d'un modèle installé avec ce contexte (`hub/fit.rs`) :
/// poids (projecteur compris) + cache de contexte (lu dans l'en-tête GGUF,
/// sinon estimé d'après la taille annoncée) + marge.
async fn estimate(model: &LocalModel, ctx: u32) -> u64 {
    let path = PathBuf::from(&model.path);
    let facts = tokio::task::spawn_blocking(move || read_facts(&path))
        .await
        .ok()
        .flatten();
    let parameters = model
        .params_label
        .as_deref()
        .and_then(auto::parameters)
        .map(|parameters| parameters as u64);
    let kv = crate::hub::fit::kv_cache(facts.as_ref(), parameters, model.arch.as_deref(), ctx);
    crate::hub::fit::memory(model.size_bytes, kv, ctx)
}

/// En-tête GGUF d'un fichier local (début du fichier, borné).
fn read_facts(path: &Path) -> Option<crate::hub::gguf::GgufFacts> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(crate::hub::gguf::HEADER_BYTES as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    crate::hub::gguf::parse(&bytes)
}

#[cfg(test)]
pub(crate) mod tests;
