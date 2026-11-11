//! Pack CUDA facultatif (cartes NVIDIA, CUDA 13 ; `packaging/cuda.env`) :
//! le moteur `ggml-cuda` de NOTRE Release (MIT, même commit que le
//! `llama-server` livré) et les bibliothèques de NVIDIA (`cudart`, `cublas`,
//! `cublasLt`, sous la licence du CUDA Toolkit, montrée avant), prises sur le
//! serveur de NVIDIA, jamais rehébergées.
//!
//! - [`pins`] : épingles ; [`detect`] : cartes et compatibilité (pilote ≥ 580,
//!   capacité ≥ 7.5) ; [`archive`] : extraction ; [`pack`] : manifeste signé,
//!   dossier installé ; [`manager`] : téléchargement, installation, retrait
//!   (état Tauri [`Cuda`], événement [`STATUS_EVENT`]).
//! - Dossier : `<données>/engine/cuda/` : `<build>/` (pack installé, à plat),
//!   `downloads/` (archives, reprises), `disabled` (réglage coupé),
//!   `jit-cache/` (cache de compilation de NVIDIA : `CUDA_CACHE_PATH`, jamais
//!   `~/.nv`).
//! - Lancement ([`Runtime::select`]) : pack de la build du serveur installé,
//!   réglage activé, carte compatible, et CUDA pas en échec dans la session →
//!   `GGML_BACKEND_PATH` = le moteur (ggml le charge en plus de ceux du
//!   dossier du serveur, qui reste en lecture seule), `--device CUDA0,…`
//!   (un CUDA qui ne se charge pas fait échouer le lancement, au lieu de
//!   tourner en silence sur le processeur), cartes compatibles seules
//!   (`CUDA_VISIBLE_DEVICES`), Vulkan caché. Bibliothèques de NVIDIA trouvées
//!   à côté du moteur : RUNPATH `$ORIGIN` sous Linux, dossier en tête du
//!   `PATH` de l'enfant sous Windows (`LoadLibraryW` d'un chemin absolu ne
//!   cherche pas dans le dossier de la DLL). Un échec au lancement coupe CUDA
//!   pour la session ([`Runtime::fail`]) : repli sur Vulkan, puis sur le
//!   processeur (chaîne existante), raison montrée sur l'écran du moteur.

pub mod archive;
pub mod detect;
pub mod manager;
pub mod pack;
pub mod pins;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod os;
#[cfg(windows)]
#[path = "windows.rs"]
mod os;
#[cfg(not(any(target_os = "linux", windows)))]
mod os {
    pub const PLATFORM: Option<super::Platform> = None;
}

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use detect::{Assessment, Compat, NvidiaGpu};
pub use manager::{Cuda, CudaStatus, STATUS_EVENT};
use pins::Pins;

/// Système d'un pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Linux,
    Windows,
}

impl Platform {
    /// Système de cette app (x86-64 seulement), sinon `None`.
    pub fn current() -> Option<Platform> {
        os::PLATFORM
    }

    /// Moteur ggml du pack.
    pub fn engine_file(self) -> &'static str {
        match self {
            Platform::Linux => "libggml-cuda.so",
            Platform::Windows => "ggml-cuda.dll",
        }
    }

    /// Fin du nom du pack (`linux-x64`, `windows-x64`).
    pub fn short(self) -> &'static str {
        match self {
            Platform::Linux => "linux-x64",
            Platform::Windows => "windows-x64",
        }
    }

    /// Fin du nom de l'archive du pack.
    pub fn pack_suffix(self) -> &'static str {
        match self {
            Platform::Linux => "linux-x64.tar.gz",
            Platform::Windows => "windows-x64.zip",
        }
    }
}

/// Ce qu'un lancement sur CUDA reçoit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CudaRuntime {
    pub platform: Platform,
    /// Dossier du pack installé.
    pub dir: PathBuf,
    /// Cartes données à CUDA (UUID de `nvidia-smi`).
    pub uuids: Vec<String>,
    /// Cache de compilation de NVIDIA.
    pub jit_cache: PathBuf,
}

/// Variable de ggml : un moteur chargé en plus (chemin d'un fichier).
pub const BACKEND_PATH_ENV: &str = "GGML_BACKEND_PATH";

/// Environnement d'un lancement sur CUDA (`path` : le `PATH` de l'enfant).
pub fn runtime_env(cuda: &CudaRuntime, path: Option<&OsStr>) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = vec![
        (
            BACKEND_PATH_ENV.into(),
            cuda.dir.join(cuda.platform.engine_file()).into_os_string(),
        ),
        ("CUDA_VISIBLE_DEVICES".into(), cuda.uuids.join(",").into()),
        ("CUDA_DEVICE_ORDER".into(), "PCI_BUS_ID".into()),
        (
            "CUDA_CACHE_PATH".into(),
            cuda.jit_cache.clone().into_os_string(),
        ),
        // Vulkan caché : la même carte n'est pas vue deux fois.
        (
            super::options::VK_DEVICES_ENV.into(),
            super::options::VK_DEVICES_NONE.into(),
        ),
    ];
    if cuda.platform == Platform::Windows {
        let mut joined = cuda.dir.clone().into_os_string();
        if let Some(path) = path.filter(|path| !path.is_empty()) {
            joined.push(";");
            joined.push(path);
        }
        env.push(("PATH".into(), joined));
    }
    env
}

/// Arguments d'un lancement sur CUDA : les cartes de CUDA seules.
pub fn device_args(cuda: &CudaRuntime) -> Vec<OsString> {
    let devices = (0..cuda.uuids.len().max(1))
        .map(|index| format!("CUDA{index}"))
        .collect::<Vec<_>>()
        .join(",");
    vec!["--device".into(), devices.into()]
}

/// Le journal accuse CUDA lui-même (le pack ou la machine, pour tous les
/// modèles) : moteur qui ne se charge pas (`failed to load …ggml-cuda…`),
/// carte refusée (`invalid device: CUDA0`), CUDA qui ne démarre pas
/// (`ggml_cuda_init: failed …`, `no CUDA-capable device`, pilote trop
/// ancien), code absent pour la carte (`no kernel image`, PTX d'une autre
/// chaîne), cuBLAS qui ne s'initialise pas ou ne connaît pas la carte. Le
/// manque de mémoire, un avertissement (`ggml_cuda_host_malloc: failed to
/// allocate … pinned memory`) ou un plantage du modèle ne disent rien du
/// pack : repli pour ce modèle seulement ([`journal_cuda_out_of_memory`],
/// plantage), CUDA gardé pour les autres.
pub fn journal_accuses_cuda(log: &str) -> bool {
    log.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        if lower.contains("warning") || lower.contains("host_malloc") || out_of_memory(&lower) {
            return false;
        }
        (lower.contains("failed to load") && lower.contains("ggml-cuda"))
            || lower.contains("invalid device: cuda")
            || (lower.contains("ggml_cuda_init") && lower.contains("failed"))
            || [
                "no cuda-capable device",
                "no kernel image is available",
                "unsupported toolchain",
                "ptx jit compil",
                "ptx was compiled",
                "invalid device function",
                "driver version is insufficient",
                "cublas_status_not_initialized",
                "cublas_status_arch_mismatch",
                "cublas_status_not_supported",
            ]
            .iter()
            .any(|cause| lower.contains(cause))
    })
}

/// Ligne (en minuscules) qui dit un manque de mémoire de CUDA.
fn out_of_memory(lower: &str) -> bool {
    lower.contains("out of memory") || lower.contains("cublas_status_alloc_failed")
}

/// Le journal d'un lancement mort sur CUDA dit un manque de mémoire
/// (`CUDA error: out of memory`, `cudaMalloc failed: out of memory`,
/// `CUBLAS_STATUS_ALLOC_FAILED`), avertissement de la mémoire épinglée mis à
/// part (non fatal) : nouvel essai de ce modèle sur le processeur.
pub fn journal_cuda_out_of_memory(log: &str) -> bool {
    log.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        out_of_memory(&lower) && !lower.contains("host_malloc") && !lower.contains("warning")
    })
}

/// État du pack partagé entre le moteur (lancements) et [`Cuda`] (écran).
#[derive(Clone)]
pub struct Runtime(Arc<Inner>);

struct Inner {
    root: PathBuf,
    platform: Option<Platform>,
    pins: Option<Pins>,
    /// Cartes vues (une détection par session, refaite à la demande).
    gpus: tokio::sync::Mutex<Option<Vec<NvidiaGpu>>>,
    /// Essais : cartes imposées (aucun `nvidia-smi`).
    fixed: Option<Vec<NvidiaGpu>>,
    /// Raison de l'échec de CUDA dans cette session.
    failure: parking_lot::Mutex<Option<String>>,
    /// Retrait du pack en cours : aucun lancement sur CUDA.
    removing: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("root", &self.0.root)
            .field("platform", &self.0.platform)
            .finish_non_exhaustive()
    }
}

/// Marque du réglage coupé.
const DISABLED: &str = "disabled";

impl Runtime {
    /// `root` : `<données>/engine/cuda`.
    pub fn new(root: PathBuf) -> Runtime {
        let platform = Platform::current();
        Runtime(Arc::new(Inner {
            root,
            platform,
            pins: pins::current().cloned(),
            gpus: tokio::sync::Mutex::new(None),
            fixed: None,
            failure: parking_lot::Mutex::new(None),
            removing: std::sync::atomic::AtomicBool::new(false),
        }))
    }

    /// Essais : système, épingles et cartes imposés.
    #[cfg(test)]
    pub fn fixed(root: PathBuf, platform: Platform, gpus: Vec<NvidiaGpu>) -> Runtime {
        Self::fixed_with(root, platform, gpus, tests::fixture_pins(platform))
    }

    /// Essais : comme [`Runtime::fixed`], épingles données.
    #[cfg(test)]
    pub fn fixed_with(
        root: PathBuf,
        platform: Platform,
        gpus: Vec<NvidiaGpu>,
        pins: Pins,
    ) -> Runtime {
        Runtime(Arc::new(Inner {
            root,
            platform: Some(platform),
            pins: Some(pins),
            gpus: tokio::sync::Mutex::new(None),
            fixed: Some(gpus),
            failure: parking_lot::Mutex::new(None),
            removing: std::sync::atomic::AtomicBool::new(false),
        }))
    }

    pub fn root(&self) -> &Path {
        &self.0.root
    }

    pub fn platform(&self) -> Option<Platform> {
        self.0.platform
    }

    pub fn pins(&self) -> Option<&Pins> {
        self.0.pins.as_ref()
    }

    /// Pack de la build du serveur, installé.
    pub fn installed(&self) -> Option<pack::Installed> {
        let (platform, pins) = (self.0.platform?, self.0.pins.as_ref()?);
        pack::installed(&self.0.root, pins, platform)
    }

    /// Réglage « Accélération NVIDIA » (activé tant que la marque manque).
    pub fn enabled(&self) -> bool {
        !self.0.root.join(DISABLED).exists()
    }

    pub fn set_enabled(&self, enabled: bool) -> std::io::Result<()> {
        let marker = self.0.root.join(DISABLED);
        if enabled {
            match std::fs::remove_file(&marker) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            }
        } else {
            std::fs::create_dir_all(&self.0.root)?;
            std::fs::write(marker, b"")
        }
    }

    /// Verdict sur les cartes (détection faite une fois ; `fresh` : refaite).
    pub async fn assessment(&self, fresh: bool) -> Assessment {
        let Some(pins) = self.0.pins.as_ref().filter(|_| self.0.platform.is_some()) else {
            return Assessment {
                compat: Compat::Unsupported,
                usable: Vec::new(),
                shown: None,
            };
        };
        let mut gpus = self.0.gpus.lock().await;
        if gpus.is_none() || fresh {
            *gpus = Some(match &self.0.fixed {
                Some(fixed) => fixed.clone(),
                None => detect::query().await,
            });
        }
        detect::assess(gpus.as_deref().unwrap_or_default(), pins)
    }

    /// CUDA pour le prochain lancement, si tout y est : pack à jour installé,
    /// réglage activé, carte compatible, aucun échec dans la session, pas de
    /// retrait en cours. Sans
    /// pack, rien n'est détecté (pas de `nvidia-smi`).
    pub async fn select(&self) -> Option<CudaRuntime> {
        if self.failure().is_some() || !self.enabled() || self.removing() {
            return None;
        }
        let installed = self.installed()?;
        let assessment = self.assessment(false).await;
        (assessment.compat == Compat::Compatible).then(|| CudaRuntime {
            platform: self.0.platform.unwrap_or(Platform::Linux),
            dir: installed.dir,
            uuids: assessment.usable.into_iter().map(|gpu| gpu.uuid).collect(),
            jit_cache: self.0.root.join("jit-cache"),
        })
    }

    /// CUDA a échoué au lancement : coupé pour la session (repli sur Vulkan).
    pub fn fail(&self, reason: String) {
        // i18n-ignore : journal.
        crate::app_log!("chatonlm: CUDA failed ({reason}); using Vulkan for this session");
        *self.0.failure.lock() = Some(reason);
    }

    /// Retrait du pack en cours (voir [`Cuda::remove`]).
    pub fn removing(&self) -> bool {
        self.0.removing.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn set_removing(&self, removing: bool) {
        self.0
            .removing
            .store(removing, std::sync::atomic::Ordering::SeqCst);
    }

    /// Raison de l'échec de CUDA dans la session.
    pub fn failure(&self) -> Option<String> {
        self.0.failure.lock().clone()
    }

    /// Nouvel essai permis (pack réinstallé ou retiré).
    pub fn clear_failure(&self) {
        *self.0.failure.lock() = None;
    }
}

#[cfg(test)]
mod tests;
