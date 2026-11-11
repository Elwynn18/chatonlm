//! Pack CUDA de l'écran du moteur (état Tauri [`Cuda`]) : état, licence
//! acceptée puis téléchargement (manifeste signé de la Release de cette
//! version, puis le pack et les deux archives de NVIDIA par la file commune
//! des téléchargements : reprise, SHA-256, progression, annulation),
//! installation (extraction des seuls fichiers utiles), réglage marche/arrêt
//! et retrait. Chaque changement part vers la page ([`STATUS_EVENT`]).
//!
//! Adresses : pack sous `https://github.com/Elwynn18/chatonlm/releases/download/v<version>/`,
//! archives sous l'adresse de NVIDIA épinglée ; essais (builds de débogage et
//! e2e seulement) : `CHATONLM_CUDA_PACK_URL`, `CHATONLM_NVIDIA_REDIST_URL`
//! (dossiers) et `CHATONLM_CUDA_PUBKEY` (clé du manifeste).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use serde::Serialize;
use ts_rs::TS;

use super::archive::{self, Kind};
use super::detect::{Assessment, Compat};
use super::pack::{self, Manifest};
use super::pins::Pins;
use super::{Platform, Runtime};
use crate::dictation::downloads::{
    DownloadItem, ItemFile, Options, SpeechDownload, SpeechDownloadState, SpeechDownloads,
};
use crate::error::{AppError, AppResult};
use crate::i18n::t;

/// Événement de l'état du pack vers la page.
pub const STATUS_EVENT: &str = "engine-cuda";
/// Identifiant du pack dans la file.
const QUEUE_ID: &str = "cuda";
/// Taille estimée de l'archive du pack (inconnue avant son manifeste).
pub const PACK_ESTIMATE: u64 = 140 * 1024 * 1024;
/// Place prise une fois installé, en plus des archives (estimation).
const INSTALLED_ESTIMATE: u64 = 1024 * 1024 * 1024;
/// Délai des petites requêtes (manifeste, signature).
const SMALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Attente au plus d'une installation en cours avant un retrait.
const BUSY_WAIT: Duration = Duration::from_secs(120);

/// Étape du pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CudaPhase {
    /// Pas installé (ni en cours).
    Absent,
    /// Manifeste de la Release en cours de lecture.
    Fetching,
    /// En file, en téléchargement ou en vérification.
    Downloading,
    /// Extraction et mise en place.
    Installing,
    Installed,
}

/// État du pack (écran du moteur).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CudaStatus {
    pub compat: Compat,
    /// Carte NVIDIA nommée (la première utilisable, sinon la première vue).
    pub gpu_name: Option<String>,
    pub driver: Option<String>,
    pub driver_min: u32,
    /// Capacité de calcul de la carte nommée (« 8.6 »).
    pub compute: Option<String>,
    pub compute_min: String,
    pub phase: CudaPhase,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    /// À télécharger (archives de NVIDIA et pack estimé).
    pub download_bytes: u64,
    /// Taille du pack installé.
    pub installed_bytes: Option<u64>,
    /// Un pack d'une autre version du moteur est là (à remplacer).
    pub stale: bool,
    pub enabled: bool,
    /// Les prochains chargements passent par CUDA.
    pub active: bool,
    /// Dernière erreur (téléchargement, installation), langue du moment.
    pub error: Option<String>,
    /// CUDA a échoué dans cette session (raison du journal) : Vulkan.
    pub fallback: Option<String>,
    pub eula_url: String,
}

/// Le pack dans la file : son archive et les deux archives de NVIDIA.
struct CudaItem {
    files: Vec<ItemFile>,
}

impl DownloadItem for CudaItem {
    fn id(&self) -> &str {
        QUEUE_ID
    }

    fn name(&self) -> &str {
        "CUDA"
    }

    fn files(&self, _base: Option<&str>) -> Vec<ItemFile> {
        self.files.clone()
    }
}

#[derive(Default)]
struct State {
    fetching: Option<Arc<AtomicBool>>,
    installing: Option<Arc<AtomicBool>>,
    /// Manifeste du téléchargement en cours (nom et version de l'archive).
    manifest: Option<Manifest>,
    download: Option<SpeechDownload>,
    error: Option<String>,
    assessment: Option<Assessment>,
}

type Emit = Box<dyn Fn(&CudaStatus) + Send + Sync>;

struct Inner {
    runtime: Runtime,
    queue: OnceLock<SpeechDownloads>,
    state: parking_lot::Mutex<State>,
    emit: Emit,
    version: String,
    sources: Sources,
}

/// Pack CUDA de l'app (état Tauri).
#[derive(Clone)]
pub struct Cuda(Arc<Inner>);

/// Adresse imposée par les essais (jamais lue par le build de production).
#[cfg(any(test, debug_assertions, feature = "e2e"))]
fn test_override(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

#[cfg(not(any(test, debug_assertions, feature = "e2e")))]
fn test_override(_name: &str) -> Option<String> {
    None
}

/// Dossier avec `/` final.
fn as_dir(mut url: String) -> String {
    if !url.ends_with('/') {
        url.push('/');
    }
    url
}

/// Adresses et clé imposées (essais) ; `None` : celles de la production.
#[derive(Clone, Debug, Default)]
pub(crate) struct Sources {
    pub release: Option<String>,
    pub nvidia: Option<String>,
    pub pubkey: Option<String>,
}

impl Sources {
    fn from_env() -> Sources {
        Sources {
            release: test_override("CHATONLM_CUDA_PACK_URL"),
            nvidia: test_override("CHATONLM_NVIDIA_REDIST_URL"),
            pubkey: test_override("CHATONLM_CUDA_PUBKEY"),
        }
    }

    /// Dossier des fichiers de la Release de `version`.
    pub fn release_url(&self, version: &str) -> String {
        self.release.clone().map_or_else(
            || format!("https://github.com/Elwynn18/chatonlm/releases/download/v{version}/"),
            as_dir,
        )
    }

    /// Dossier des archives de NVIDIA.
    fn nvidia_url(&self, pins: &Pins) -> String {
        self.nvidia
            .clone()
            .map_or_else(|| pins.redist_url.clone(), as_dir)
    }
}

impl Cuda {
    /// `emit` reçoit l'état à chaque changement ; `version` : celle de l'app
    /// (Release du pack).
    pub fn new(
        runtime: Runtime,
        version: &str,
        emit: impl Fn(&CudaStatus) + Send + Sync + 'static,
    ) -> Cuda {
        Self::with_sources(runtime, version, emit, Sources::from_env())
    }

    pub(crate) fn with_sources(
        runtime: Runtime,
        version: &str,
        emit: impl Fn(&CudaStatus) + Send + Sync + 'static,
        sources: Sources,
    ) -> Cuda {
        let inner = Arc::new(Inner {
            sources,
            runtime,
            queue: OnceLock::new(),
            state: parking_lot::Mutex::new(State::default()),
            emit: Box::new(emit),
            version: version.to_owned(),
        });
        let weak: Weak<Inner> = Arc::downgrade(&inner);
        let queue = SpeechDownloads::with_options(
            move |list| {
                if let Some(inner) = weak.upgrade() {
                    inner.state.lock().download =
                        list.iter().find(|item| item.id == QUEUE_ID).cloned();
                    inner.emit_now();
                }
            },
            inner.runtime.root().join("downloads"),
            Options {
                max_running: 1,
                // Adresses complètes données par le pack (jamais d'hôte imposé).
                test_env: "CHATONLM_CUDA_QUEUE_URL",
                find: Arc::new(|_| None),
                count_ending_tasks: true,
                ..Options::default()
            },
        );
        let weak = Arc::downgrade(&inner);
        queue.on_installed(move |id| {
            if id == QUEUE_ID
                && let Some(inner) = weak.upgrade()
            {
                Cuda(inner).assemble();
            }
        });
        let _ = inner.queue.set(queue);
        Cuda(inner)
    }

    fn queue(&self) -> &SpeechDownloads {
        self.0.queue.get().expect("file créée par Cuda::new")
    }

    /// État lu maintenant (`fresh` : cartes détectées de nouveau).
    pub async fn status(&self, fresh: bool) -> CudaStatus {
        let assessment = self.0.runtime.assessment(fresh).await;
        self.0.state.lock().assessment = Some(assessment);
        self.0.snapshot()
    }

    /// Télécharge et installe le pack ; `accept` : licence de NVIDIA acceptée.
    pub async fn install(&self, accept: bool) -> AppResult<()> {
        if !accept {
            return Err(AppError::Invalid(t!("engine.cuda.licenseRequired")));
        }
        let assessment = self.0.runtime.assessment(false).await;
        let compatible = assessment.compat == Compat::Compatible;
        self.0.state.lock().assessment = Some(assessment);
        let (Some(platform), Some(pins)) =
            (self.0.runtime.platform(), self.0.runtime.pins().cloned())
        else {
            return Err(AppError::Invalid(t!("engine.cuda.incompatible")));
        };
        if !compatible {
            return Err(AppError::Invalid(t!("engine.cuda.incompatible")));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut state = self.0.state.lock();
            let downloading = state
                .download
                .as_ref()
                .is_some_and(|download| active(download.state));
            if state.fetching.is_some() || state.installing.is_some() || downloading {
                return Ok(());
            }
            state.fetching = Some(cancel.clone());
            state.error = None;
        }
        self.0.emit_now();
        let this = self.clone();
        crate::process::spawn_task(async move {
            let fetched = fetch(
                &this.0.version,
                &this.0.sources,
                platform,
                &pins,
                this.0.runtime.root(),
                &cancel,
            )
            .await;
            let mut state = this.0.state.lock();
            state.fetching = None;
            if cancel.load(Ordering::Relaxed) {
                drop(state);
                this.0.emit_now();
                return;
            }
            match fetched {
                Ok((manifest, item)) => {
                    state.manifest = Some(manifest);
                    drop(state);
                    this.queue().enqueue(Arc::new(item));
                }
                Err(reason) => {
                    state.error = Some(t!("engine.cuda.fetchFailed", reason = reason));
                    drop(state);
                    this.0.emit_now();
                }
            }
        });
        Ok(())
    }

    /// Arrête le téléchargement ou l'installation en cours (archives
    /// gardées : un nouveau téléchargement reprend).
    pub fn cancel(&self) {
        {
            let state = self.0.state.lock();
            for flag in [&state.fetching, &state.installing].into_iter().flatten() {
                flag.store(true, Ordering::Relaxed);
            }
        }
        self.queue().cancel(QUEUE_ID);
        self.0.emit_now();
    }

    /// Réglage marche/arrêt (un échec de la session est oublié à la remise
    /// en marche : nouvel essai).
    pub fn set_enabled(&self, enabled: bool) -> AppResult<()> {
        self.0.runtime.set_enabled(enabled)?;
        if enabled {
            self.0.runtime.clear_failure();
        }
        self.0.emit_now();
        Ok(())
    }

    /// Retire le pack (toutes les builds), les archives et le cache de
    /// compilation ; l'appelant a déchargé les modèles et gelé le moteur
    /// (Windows : fichiers ouverts non supprimables). Pendant le retrait,
    /// aucun lancement ne prend CUDA ; une installation en cours (extraction,
    /// contrôle, renommage : tâche bloquante) est d'abord attendue.
    pub async fn remove(&self) -> AppResult<()> {
        struct Removing<'a>(&'a Runtime);
        impl Drop for Removing<'_> {
            fn drop(&mut self) {
                self.0.set_removing(false);
            }
        }
        self.0.runtime.set_removing(true);
        let _removing = Removing(&self.0.runtime);
        self.cancel();
        let _ = self
            .queue()
            .wait_idle(QUEUE_ID, Duration::from_secs(5))
            .await;
        let deadline = std::time::Instant::now() + BUSY_WAIT;
        while self.busy() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.queue().dismiss(QUEUE_ID);
        let root = self.0.runtime.root().to_path_buf();
        let result = tauri::async_runtime::spawn_blocking(move || remove_all(&root))
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
        {
            let mut state = self.0.state.lock();
            state.download = None;
            state.manifest = None;
            state.error = None;
        }
        self.0.runtime.clear_failure();
        self.0.emit_now();
        result.map_err(|error| {
            AppError::System(t!("engine.cuda.removeFailed", reason = error.to_string()))
        })
    }

    /// Lecture du manifeste ou assemblage en cours.
    fn busy(&self) -> bool {
        let state = self.0.state.lock();
        state.installing.is_some() || state.fetching.is_some()
    }

    /// Essais : une installation en cours (tenue jusqu'à
    /// [`Cuda::release_install`]).
    #[cfg(test)]
    pub(crate) fn hold_install(&self) {
        self.0.state.lock().installing = Some(Arc::new(AtomicBool::new(false)));
    }

    #[cfg(test)]
    pub(crate) fn release_install(&self) {
        self.0.state.lock().installing = None;
    }

    /// Sortie de l'app : téléchargement arrêté (archives gardées).
    pub async fn stop_all(&self, limit: Duration) {
        if let Some(flag) = self.0.state.lock().installing.as_ref() {
            flag.store(true, Ordering::Relaxed);
        }
        self.queue().stop_all(limit).await;
    }

    /// Archives reçues : extraction et mise en place (tâche bloquante).
    fn assemble(&self) {
        let (Some(platform), Some(pins)) =
            (self.0.runtime.platform(), self.0.runtime.pins().cloned())
        else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let manifest = {
            let mut state = self.0.state.lock();
            state.installing = Some(cancel.clone());
            state.manifest.clone()
        };
        self.0.emit_now();
        let this = self.clone();
        crate::process::spawn_task(async move {
            let root = this.0.runtime.root().to_path_buf();
            let flag = cancel.clone();
            let result = match manifest {
                Some(manifest) => tauri::async_runtime::spawn_blocking(move || {
                    install_from(&root, platform, &pins, &manifest, &flag)
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string())),
                // i18n-ignore : détail technique, dans un message traduit.
                None => Err("no manifest".to_owned()),
            };
            this.queue().dismiss(QUEUE_ID);
            {
                let mut state = this.0.state.lock();
                state.installing = None;
                state.download = None;
                match result {
                    Ok(()) => {
                        state.manifest = None;
                        state.error = None;
                    }
                    Err(_) if cancel.load(Ordering::Relaxed) => {}
                    Err(reason) => {
                        state.error = Some(t!("engine.cuda.installFailed", reason = reason))
                    }
                }
            }
            this.0.runtime.clear_failure();
            this.0.emit_now();
        });
    }
}

/// Téléchargement actif dans la file.
fn active(state: SpeechDownloadState) -> bool {
    matches!(
        state,
        SpeechDownloadState::Queued
            | SpeechDownloadState::Downloading
            | SpeechDownloadState::Verifying
    )
}

impl Inner {
    /// État sans détection (dernière connue).
    fn snapshot(&self) -> CudaStatus {
        let state = self.state.lock();
        let runtime = &self.runtime;
        let pins = runtime.pins();
        let installed = runtime.installed();
        let assessment = state.assessment.clone().unwrap_or(Assessment {
            compat: if pins.is_some() && runtime.platform().is_some() {
                Compat::NoNvidia
            } else {
                Compat::Unsupported
            },
            usable: Vec::new(),
            shown: None,
        });
        let downloading = state
            .download
            .as_ref()
            .filter(|download| active(download.state));
        let phase = if state.installing.is_some() {
            CudaPhase::Installing
        } else if state.fetching.is_some() {
            CudaPhase::Fetching
        } else if downloading.is_some() {
            CudaPhase::Downloading
        } else if installed.is_some() {
            CudaPhase::Installed
        } else {
            CudaPhase::Absent
        };
        let error = state.error.clone().or_else(|| {
            state
                .download
                .as_ref()
                .filter(|download| download.state == SpeechDownloadState::Failed)
                .map(|download| {
                    t!(
                        "engine.cuda.downloadFailed",
                        reason = download.error.clone().unwrap_or_default()
                    )
                })
        });
        let enabled = runtime.enabled();
        let fallback = runtime.failure();
        let compatible = assessment.compat == Compat::Compatible;
        let shown = assessment.shown.as_ref();
        CudaStatus {
            compat: assessment.compat,
            gpu_name: shown.map(|gpu| gpu.name.clone()),
            driver: shown.map(|gpu| gpu.driver.clone()),
            driver_min: pins.map_or(0, |pins| pins.driver_min),
            compute: shown.map(|gpu| format!("{}.{}", gpu.compute.0, gpu.compute.1)),
            compute_min: pins.map_or_else(String::new, |pins| {
                format!("{}.{}", pins.compute_min.0, pins.compute_min.1)
            }),
            phase,
            completed_bytes: downloading.map_or(0, |download| download.completed_bytes),
            total_bytes: downloading.map_or(0, |download| download.total_bytes),
            download_bytes: pins.map_or(0, |pins| {
                pins.cudart.size + pins.cublas.size + PACK_ESTIMATE
            }),
            installed_bytes: installed.as_ref().map(|installed| installed.bytes),
            // Autre build, ou même build d'une autre `VERSION` (pack refait) :
            // l'écran le dit et propose de le retélécharger.
            stale: installed.is_none()
                && pins.is_some_and(|pins| {
                    pack::outdated(runtime.root(), pins)
                        || !pack::stale_builds(runtime.root(), &pins.llama_build).is_empty()
                }),
            enabled,
            active: installed.is_some() && enabled && compatible && fallback.is_none(),
            error,
            fallback,
            eula_url: pins.map_or_else(String::new, |pins| pins.eula_url.clone()),
        }
    }

    fn emit_now(&self) {
        (self.emit)(&self.snapshot());
    }
}

/// Petit fichier (manifeste, signature), [`pack::MAX_SMALL`] au plus.
async fn get_small(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        // i18n-ignore : détail technique, dans un message traduit.
        return Err(format!("HTTP {} ({url})", response.status().as_u16()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        body.extend_from_slice(&chunk);
        if body.len() > pack::MAX_SMALL {
            // i18n-ignore : détail technique, dans un message traduit.
            return Err(format!("too large ({url})"));
        }
    }
    Ok(body)
}

/// Manifeste signé de la Release, puis l'élément de la file (pack et
/// archives de NVIDIA) ; place contrôlée (archives et dossier installé).
async fn fetch(
    version: &str,
    sources: &Sources,
    platform: Platform,
    pins: &Pins,
    root: &Path,
    cancel: &AtomicBool,
) -> Result<(Manifest, CudaItem), String> {
    crate::http::ensure_crypto_provider();
    let client = reqwest::Client::builder()
        .connect_timeout(SMALL_TIMEOUT)
        .timeout(SMALL_TIMEOUT)
        .user_agent(concat!("ChatonLM/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;
    let base = sources.release_url(version);
    let name = pack::manifest_name(platform, &pins.llama_build);
    let json = get_small(&client, &format!("{base}{name}")).await?;
    let signature = get_small(&client, &format!("{base}{name}.sig")).await?;
    if cancel.load(Ordering::Relaxed) {
        // i18n-ignore : jamais montré (annulé).
        return Err("cancelled".to_owned());
    }
    let pubkey = sources
        .pubkey
        .clone()
        .or_else(pack::production_pubkey)
        // i18n-ignore : détail technique, dans un message traduit.
        .ok_or_else(|| "no public key".to_owned())?;
    pack::verify_signature(
        &json,
        &String::from_utf8_lossy(&signature),
        &pubkey,
        &name,
        version,
    )?;
    let manifest = pack::parse_manifest(&json, platform, pins)?;
    let item = item_for(&manifest, &base, &sources.nvidia_url(pins), pins);
    let downloads = root.join("downloads");
    let files = item
        .files
        .iter()
        .map(|file| (downloads.join(&file.local), file.size))
        .chain(std::iter::once((
            pack::build_dir(root, &pins.llama_build),
            INSTALLED_ESTIMATE,
        )));
    crate::image_models::downloads::space_check(files, crate::disk::free_space_for(root))?;
    Ok((manifest, item))
}

/// Fichiers de la file : l'archive du pack, puis celles de NVIDIA.
fn item_for(manifest: &Manifest, release: &str, nvidia: &str, pins: &Pins) -> CudaItem {
    let mut files = vec![ItemFile {
        path: manifest.file.clone(),
        size: manifest.size,
        sha256: manifest.sha256.clone(),
        local: PathBuf::from(&manifest.file),
        url: format!("{release}{}", manifest.file),
    }];
    for redist in [&pins.cudart, &pins.cublas] {
        files.push(ItemFile {
            path: redist.file_name().to_owned(),
            size: redist.size,
            sha256: redist.sha256.clone(),
            local: PathBuf::from(redist.file_name()),
            url: format!("{nvidia}{}", redist.path),
        });
    }
    CudaItem { files }
}

/// Nouveaux essais d'un renommage ou d'un effacement refusé un instant
/// (Windows : l'antivirus ouvre les DLL tout juste écrites).
const BUSY_TRIES: u32 = 10;
const BUSY_PAUSE: Duration = Duration::from_millis(500);

/// Erreur passagère d'un fichier ouvert par un autre programme : sous
/// Windows, accès refusé, partage ou verrou refusé (codes 5, 32, 33) ;
/// jamais sous Linux (logique pure, essayée sur les deux systèmes).
pub(crate) fn busy_error(error: &std::io::Error, windows: bool) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    windows
        && (error.kind() == std::io::ErrorKind::PermissionDenied
            || matches!(
                error.raw_os_error(),
                Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
            ))
}

/// `op`, refait jusqu'à `tries` fois en tout (`pause` entre deux) tant que
/// l'erreur est passagère ([`busy_error`]) ; comme les crochets NSIS
/// (`CHATONLM_REMOVE_LLAMA`, dix essais de 500 ms).
pub(crate) fn retry_busy<T>(
    windows: bool,
    tries: u32,
    pause: Duration,
    mut op: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    let mut done = 1;
    loop {
        match op() {
            Err(error) if done < tries && busy_error(&error, windows) => {
                done += 1;
                std::thread::sleep(pause);
            }
            result => return result,
        }
    }
}

/// [`retry_busy`] de ce système.
fn patiently<T>(op: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    retry_busy(cfg!(windows), BUSY_TRIES, BUSY_PAUSE, op)
}

/// Efface un dossier, patiemment ; absent : fait.
fn remove_dir_patiently(dir: &Path) -> std::io::Result<()> {
    patiently(|| match std::fs::remove_dir_all(dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    })
}

/// Assemble `<build>.part/` depuis les archives reçues (pack vérifié par
/// son `SHA256SUMS` et sa `VERSION`, trois bibliothèques de NVIDIA et leur
/// licence), le renomme en `<build>/`, puis retire les archives et les
/// builds périmées.
pub(crate) fn install_from(
    root: &Path,
    platform: Platform,
    pins: &Pins,
    manifest: &Manifest,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let text = |error: std::io::Error| error.to_string();
    let downloads = root.join("downloads");
    let target = pack::build_dir(root, &pins.llama_build);
    let part = root.join(format!("{}.part", pins.llama_build));
    let _ = std::fs::remove_dir_all(&part);
    std::fs::create_dir_all(&part).map_err(text)?;
    let result = (|| {
        let kind = |name: &str| {
            // i18n-ignore : détail technique, dans un message traduit.
            Kind::of(name).ok_or_else(|| format!("unknown archive: {name}"))
        };
        archive::extract(
            &downloads.join(&manifest.file),
            kind(&manifest.file)?,
            &part,
            &archive::pack_pick(platform),
            cancel,
        )
        .map_err(text)?;
        pack::check_sums(&part, platform)?;
        let version = std::fs::read_to_string(part.join("VERSION")).map_err(text)?;
        if version.trim() != manifest.version {
            // i18n-ignore : détail technique, dans un message traduit.
            return Err(format!("VERSION {} ≠ {}", version.trim(), manifest.version));
        }
        for (redist, license) in [(&pins.cudart, true), (&pins.cublas, false)] {
            archive::extract(
                &downloads.join(redist.file_name()),
                kind(redist.file_name())?,
                &part,
                &archive::nvidia_pick(platform, pins.cuda_major, license),
                cancel,
            )
            .map_err(text)?;
        }
        for file in pack::installed_files(platform, pins.cuda_major) {
            if !part.join(&file).is_file() {
                // i18n-ignore : détail technique, dans un message traduit.
                return Err(format!("{file} missing"));
            }
        }
        remove_dir_patiently(&target).map_err(text)?;
        patiently(|| std::fs::rename(&part, &target)).map_err(text)
    })();
    if result.is_err() {
        let _ = remove_dir_patiently(&part);
        return result;
    }
    // Archives et builds périmées : un reste (fichier ouvert sous Windows)
    // n'empêche rien.
    let _ = std::fs::remove_dir_all(&downloads);
    for build in pack::stale_builds(root, &pins.llama_build) {
        let _ = std::fs::remove_dir_all(root.join(build));
    }
    Ok(())
}

/// Retire tout le dossier du pack : builds, `.part`, archives, cache de
/// compilation, réglage.
fn remove_all(root: &Path) -> std::io::Result<()> {
    remove_dir_patiently(root)
}
