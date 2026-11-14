//! Outils de la Transcription (état Tauri [`Tools`]) : état des deux
//! outils, installation après accord des licences (relevé de la dernière
//! version, sommes et signature, puis l'archive par la file commune des
//! téléchargements : reprise, SHA-256, progression, annulation), mise à
//! jour, retrait. Chaque changement part vers la page ([`STATUS_EVENT`]).
//!
//! Suivi des versions : la dernière version est relevée au moment de
//! servir (pas en tâche de fond), au plus une fois par
//! [`ToolId::check_period`] (un jour pour yt-dlp, une semaine pour Deno),
//! et tout de suite après une erreur de version périmée
//! ([`Tools::refresh`]) ; jamais pendant une transcription (bail
//! [`ToolsLease`] tenu : ni relevé, ni mise à jour, ni retrait). Un relevé
//! à chaque usage interrogerait GitHub à chaque vidéo (et lui dirait quand
//! l'utilisateur transcrit) pour rien : yt-dlp publie au mieux tous les
//! quelques jours.
//!
//! Le lot 4 (lancement de yt-dlp) appelle [`Tools::ready`] : mises à jour
//! dues faites d'abord (attendues, progression vers la page), puis un bail
//! qui donne les chemins vérifiés ([`ToolPaths`]) et bloque mise à jour et
//! retrait jusqu'à sa fin.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use super::install::{self, Installed, Probe};
use super::release::{self, Failure, Release, Sources};
use super::{Platform, ToolError, ToolErrorCode, ToolId, ToolPhase, ToolStatus, ToolsStatus};
use crate::dictation::downloads::{
    DownloadItem, ItemFile, Options, SpeechDownload, SpeechDownloadState, SpeechDownloads,
};
use crate::error::{AppError, AppResult};
use crate::i18n::t;

/// Événement de l'état des outils vers la page.
pub const STATUS_EVENT: &str = "tools-status";
/// Variable qui imposerait un hôte à toute la file (jamais posée : les
/// adresses viennent de [`Sources`]) ; fermée par `withApp`.
const QUEUE_ENV: &str = "CHATONLM_TOOLS_QUEUE_URL";
/// Attente au plus d'une installation en cours avant un retrait.
const BUSY_WAIT: Duration = Duration::from_secs(120);
/// Place prise une fois extrait, en plus de l'archive (yt-dlp : 2,4 fois
/// l'archive sous Linux).
const EXTRACT_FACTOR: u64 = 3;

/// Dossiers sous `<données>/tools`.
const DOWNLOADS: &str = "downloads";
const DENO_CACHE: &str = "deno-cache";
const YTDLP_CACHE: &str = "yt-dlp-cache";
const NO_FFMPEG: &str = "no-ffmpeg";

/// Ce que demande une opération.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Relever la dernière version seulement (page ouverte).
    Check,
    /// Installer la dernière version si l'outil est absent ou plus ancien.
    /// `quiet` : relevé échoué sans erreur montrée (usage, hors ligne).
    Install { quiet: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Fetching,
    Queued,
    Installing,
}

struct Op {
    mode: Mode,
    stage: Stage,
    cancel: Arc<AtomicBool>,
    release: Option<Release>,
}

#[derive(Default)]
struct ToolState {
    op: Option<Op>,
    download: Option<SpeechDownload>,
    error: Option<ToolError>,
    installed: Option<Installed>,
    checked: Option<install::Checked>,
}

#[derive(Default)]
struct State {
    tools: HashMap<ToolId, ToolState>,
    /// Baux tenus (transcriptions en cours).
    leases: usize,
    /// Nettoyage du lancement (anciennes versions) en cours.
    cleaning: bool,
    removing: bool,
}

impl State {
    fn tool(&mut self, tool: ToolId) -> &mut ToolState {
        self.tools.entry(tool).or_default()
    }
}

type Emit = Box<dyn Fn(&ToolsStatus) + Send + Sync>;

struct Inner {
    root: PathBuf,
    platform: Option<Platform>,
    sources: Sources,
    probe: Probe,
    queue: OnceLock<SpeechDownloads>,
    state: parking_lot::Mutex<State>,
    emit: Emit,
}

/// Outils de la Transcription (état Tauri).
#[derive(Clone)]
pub struct Tools(Arc<Inner>);

/// Chemins des outils installés et vérifiés, et dossiers de travail de
/// yt-dlp sous les données de l'app (créés).
// Servi par le lot 4 (lancement de yt-dlp).
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolPaths {
    /// Programme yt-dlp (onedir : `_internal/` à côté).
    pub yt_dlp: PathBuf,
    pub yt_dlp_version: String,
    /// Programme Deno (`--js-runtimes deno:<chemin>`).
    pub deno: PathBuf,
    pub deno_version: String,
    /// `DENO_DIR` (déjà dans [`ToolPaths::environment`]).
    pub deno_dir: PathBuf,
    /// `--cache-dir` de yt-dlp.
    pub yt_dlp_cache: PathBuf,
    /// Dossier vide pour `--ffmpeg-location` (aucun ffmpeg de la machine).
    pub no_ffmpeg: PathBuf,
}

#[allow(dead_code)]
impl ToolPaths {
    /// Environnement complet du lancement de yt-dlp (hérité par Deno) :
    /// voir [`install::tool_environment`] ; à passer après `env_clear()`.
    pub fn environment(&self) -> BTreeMap<OsString, OsString> {
        install::tool_environment(&self.deno_dir)
    }
}

/// Bail d'une transcription : les outils ne bougent pas tant qu'il vit.
#[allow(dead_code)]
pub struct ToolsLease {
    inner: Arc<Inner>,
    pub paths: ToolPaths,
}

impl Drop for ToolsLease {
    fn drop(&mut self) {
        {
            let mut state = self.inner.state.lock();
            state.leases = state.leases.saturating_sub(1);
        }
        self.inner.emit_now();
    }
}

/// Un outil dans la file : son archive.
struct ToolItem {
    tool: ToolId,
    file: ItemFile,
}

impl DownloadItem for ToolItem {
    fn id(&self) -> &str {
        self.tool.key()
    }

    fn name(&self) -> &str {
        self.tool.name()
    }

    fn files(&self, _base: Option<&str>) -> Vec<ItemFile> {
        vec![self.file.clone()]
    }
}

/// Nom local de l'archive dans la file (`yt-dlp-2026.10.26.zip`).
fn archive_name(release: &Release) -> String {
    format!("{}-{}.zip", release.tool.key(), release.tag)
}

fn item_for(release: &Release) -> ToolItem {
    ToolItem {
        tool: release.tool,
        file: ItemFile {
            path: release.asset.clone(),
            size: release.size,
            sha256: release.sha256.clone(),
            local: PathBuf::from(archive_name(release)),
            url: release.url.clone(),
        },
    }
}

fn tool_of(id: &str) -> Option<ToolId> {
    ToolId::ALL.into_iter().find(|tool| tool.key() == id)
}

fn active(state: SpeechDownloadState) -> bool {
    matches!(
        state,
        SpeechDownloadState::Queued
            | SpeechDownloadState::Downloading
            | SpeechDownloadState::Verifying
    )
}

/// ms depuis l'époque Unix.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// Message d'une erreur d'un outil, dans la langue du moment.
pub fn failure_message(tool: ToolId, failure: &Failure) -> String {
    let name = tool.name();
    let reason = &failure.detail;
    match failure.code {
        ToolErrorCode::Unsupported => t!("tools.unsupported"),
        ToolErrorCode::Network => t!("tools.network", tool = name, reason = reason),
        ToolErrorCode::Release => t!("tools.release", tool = name, reason = reason),
        ToolErrorCode::Signature if failure.detail == release::OTHER_KEY => t!("tools.otherKey"),
        ToolErrorCode::Signature => t!("tools.signature", reason = reason),
        ToolErrorCode::Download => t!("tools.downloadFailed", tool = name, reason = reason),
        ToolErrorCode::Archive => t!("tools.archive", tool = name, reason = reason),
        ToolErrorCode::Launch => t!("tools.launch", tool = name, reason = reason),
        ToolErrorCode::Version => t!("tools.version", tool = name, reason = reason),
        // Déjà un message traduit (contrôle de la place).
        ToolErrorCode::Space => failure.detail.clone(),
        ToolErrorCode::Io => t!("tools.io", tool = name, reason = reason),
    }
}

fn error_of(tool: ToolId, failure: &Failure) -> ToolError {
    ToolError {
        code: failure.code,
        message: failure_message(tool, failure),
    }
}

impl Tools {
    /// `root` : `<données>/tools` ; `emit` reçoit l'état à chaque
    /// changement.
    pub fn new(root: PathBuf, emit: impl Fn(&ToolsStatus) + Send + Sync + 'static) -> Tools {
        Self::with(
            root,
            Platform::current(),
            Sources::from_env(),
            install::real_probe(),
            emit,
        )
    }

    /// Essais : système, adresses et lancement de `--version` imposés.
    pub(crate) fn with(
        root: PathBuf,
        platform: Option<Platform>,
        sources: Sources,
        probe: Probe,
        emit: impl Fn(&ToolsStatus) + Send + Sync + 'static,
    ) -> Tools {
        let mut state = State {
            cleaning: true,
            ..State::default()
        };
        for tool in ToolId::ALL {
            let dir = root.join(tool.key());
            let entry = state.tool(tool);
            entry.installed =
                platform.and_then(|platform| install::installed(&dir, tool, platform));
            entry.checked = install::read_checked(&dir);
        }
        let inner = Arc::new(Inner {
            root,
            platform,
            sources,
            probe,
            queue: OnceLock::new(),
            state: parking_lot::Mutex::new(state),
            emit: Box::new(emit),
        });
        let weak: Weak<Inner> = Arc::downgrade(&inner);
        let queue = SpeechDownloads::with_options(
            move |list| {
                if let Some(inner) = weak.upgrade() {
                    Inner::on_queue(&inner, list);
                }
            },
            inner.root.join(DOWNLOADS),
            Options {
                max_running: 2,
                test_env: QUEUE_ENV,
                // Adresses complètes données par la Release.
                find: Arc::new(|_| None),
                count_ending_tasks: true,
                ..Options::default()
            },
        );
        let weak = Arc::downgrade(&inner);
        queue.on_installed(move |id| {
            if let (Some(inner), Some(tool)) = (weak.upgrade(), tool_of(id)) {
                Tools(inner).assemble(tool);
            }
        });
        let _ = inner.queue.set(queue);
        let tools = Tools(inner);
        // Anciennes versions et restes (fichiers ouverts à la dernière
        // mise à jour sous Windows) : effacés au lancement.
        let this = tools.clone();
        crate::process::spawn_task(async move {
            let root = this.0.root.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                for tool in ToolId::ALL {
                    install::cleanup(&root.join(tool.key()));
                }
            })
            .await;
            this.0.state.lock().cleaning = false;
        });
        tools
    }

    fn queue(&self) -> &SpeechDownloads {
        self.0.queue.get().expect("file créée par Tools::with")
    }

    fn tool_dir(&self, tool: ToolId) -> PathBuf {
        self.0.root.join(tool.key())
    }

    /// État ; `check` : relève la dernière version des outils installés
    /// dont le relevé est dû (page ouverte), sauf pendant une
    /// transcription.
    pub fn status(&self, check: bool) -> ToolsStatus {
        if check {
            for tool in self.due() {
                self.start(tool, Mode::Check);
            }
        }
        self.0.snapshot()
    }

    /// Outils installés, sans opération, dont le relevé est dû ; aucun
    /// pendant un bail ou un retrait.
    fn due(&self) -> Vec<ToolId> {
        let mut state = self.0.state.lock();
        if state.leases > 0 || state.removing {
            return Vec::new();
        }
        let now = now_ms();
        ToolId::ALL
            .into_iter()
            .filter(|tool| {
                let entry = state.tool(*tool);
                entry.installed.is_some()
                    && entry.op.is_none()
                    && entry.checked.as_ref().is_none_or(|checked| {
                        now.saturating_sub(checked.checked_at)
                            >= tool.check_period().as_millis() as i64
                            || checked.checked_at > now
                    })
            })
            .collect()
    }

    /// Télécharge et installe les outils absents ; `accept` : licences
    /// acceptées sur la page.
    pub fn install(&self, accept: bool) -> AppResult<()> {
        if !accept {
            return Err(AppError::Invalid(t!("tools.licenseRequired")));
        }
        if self.0.platform.is_none() {
            return Err(AppError::Invalid(t!("tools.unsupported")));
        }
        let absent: Vec<ToolId> = {
            let mut state = self.0.state.lock();
            if state.removing {
                return Err(AppError::Invalid(t!("tools.removing")));
            }
            ToolId::ALL
                .into_iter()
                .filter(|tool| state.tool(*tool).installed.is_none())
                .collect()
        };
        for tool in absent {
            self.start(tool, Mode::Install { quiet: false });
        }
        Ok(())
    }

    /// Relève tout de suite la dernière version des outils installés et
    /// installe les plus récentes ; refusé pendant une transcription.
    pub fn update(&self) -> AppResult<()> {
        let installed: Vec<ToolId> = {
            let mut state = self.0.state.lock();
            if state.leases > 0 {
                return Err(AppError::Invalid(t!("tools.inUse")));
            }
            if state.removing {
                return Err(AppError::Invalid(t!("tools.removing")));
            }
            ToolId::ALL
                .into_iter()
                .filter(|tool| state.tool(*tool).installed.is_some())
                .collect()
        };
        for tool in installed {
            self.start(tool, Mode::Install { quiet: false });
        }
        Ok(())
    }

    /// Lance l'opération `mode` de `tool` ; une opération déjà là : un
    /// relevé seul en cours devient une installation, sinon rien. Vrai si
    /// une opération a démarré.
    fn start(&self, tool: ToolId, mode: Mode) -> bool {
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut state = self.0.state.lock();
            let entry = state.tool(tool);
            if let Some(op) = entry.op.as_mut() {
                if op.mode == Mode::Check && mode != Mode::Check && op.stage == Stage::Fetching {
                    op.mode = mode;
                }
                return false;
            }
            entry.op = Some(Op {
                mode,
                stage: Stage::Fetching,
                cancel: cancel.clone(),
                release: None,
            });
            if mode != Mode::Check {
                entry.error = None;
            }
        }
        self.0.emit_now();
        let this = self.clone();
        crate::process::spawn_task(async move {
            this.pipeline(tool, cancel).await;
        });
        true
    }

    /// Relevé, puis (si l'opération le demande et qu'il y a plus récent)
    /// sommes, signature et mise en file.
    async fn pipeline(&self, tool: ToolId, cancel: Arc<AtomicBool>) {
        let result = self.fetch(tool, &cancel).await;
        let mut enqueue = None;
        {
            let mut state = self.0.state.lock();
            let entry = state.tool(tool);
            let quiet = entry
                .op
                .as_ref()
                .is_none_or(|op| matches!(op.mode, Mode::Check | Mode::Install { quiet: true }));
            if cancel.load(Ordering::Relaxed) {
                entry.op = None;
            } else {
                match result {
                    Ok(None) => entry.op = None,
                    Ok(Some(release)) => {
                        if let Some(op) = entry.op.as_mut() {
                            op.stage = Stage::Queued;
                            op.release = Some(release.clone());
                        }
                        enqueue = Some(item_for(&release));
                    }
                    Err(failure) => {
                        entry.op = None;
                        // i18n-ignore : journal.
                        crate::app_log!(
                            "chatonlm: {} release check failed: {}",
                            tool.key(),
                            failure.detail
                        );
                        if !quiet || failure.code == ToolErrorCode::Signature {
                            entry.error = Some(error_of(tool, &failure));
                        }
                    }
                }
            }
        }
        match enqueue {
            Some(item) => {
                self.queue().enqueue(Arc::new(item));
                // Annulé entre-temps : la file s'arrête aussi.
                if cancel.load(Ordering::Relaxed) {
                    self.queue().cancel(tool.key());
                }
            }
            None => self.0.emit_now(),
        }
    }

    /// Dernière version (notée), puis l'archive à installer s'il le faut.
    async fn fetch(&self, tool: ToolId, cancel: &AtomicBool) -> Result<Option<Release>, Failure> {
        let Some(platform) = self.0.platform else {
            // i18n-ignore : détail technique, jamais montré (code).
            return Err(Failure::new(ToolErrorCode::Unsupported, "unsupported"));
        };
        let tag = release::latest_tag(tool, &self.0.sources).await?;
        let checked = install::Checked {
            checked_at: now_ms(),
            latest: tag.clone(),
        };
        let _ = install::write_checked(&self.tool_dir(tool), &checked);
        let (mode, installed) = {
            let mut state = self.0.state.lock();
            let entry = state.tool(tool);
            entry.checked = Some(checked);
            (
                entry.op.as_ref().map(|op| op.mode),
                entry
                    .installed
                    .as_ref()
                    .map(|installed| installed.tag.clone()),
            )
        };
        let wanted = match (mode, installed) {
            (Some(Mode::Install { .. }), None) => true,
            (Some(Mode::Install { .. }), Some(current)) => release::newer(&tag, &current),
            _ => false,
        };
        if !wanted || cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let release = release::release(tool, &tag, platform, &self.0.sources).await?;
        let downloads = self.0.root.join(DOWNLOADS);
        crate::image_models::downloads::space_check(
            [
                (downloads.join(archive_name(&release)), release.size),
                (
                    self.tool_dir(tool).join(&release.tag),
                    release.size.saturating_mul(EXTRACT_FACTOR),
                ),
            ],
            crate::disk::free_space_for(&self.0.root),
        )
        .map_err(|message| Failure::new(ToolErrorCode::Space, message))?;
        Ok(Some(release))
    }

    /// Archive reçue et vérifiée : extraction, `--version`, bascule (tâche
    /// bloquante).
    fn assemble(&self, tool: ToolId) {
        let Some(platform) = self.0.platform else {
            return;
        };
        let (release, cancel) = {
            let mut state = self.0.state.lock();
            let Some(op) = state.tool(tool).op.as_mut() else {
                return;
            };
            if op.stage != Stage::Queued {
                return;
            }
            let Some(release) = op.release.clone() else {
                return;
            };
            op.stage = Stage::Installing;
            (release, op.cancel.clone())
        };
        self.0.emit_now();
        let this = self.clone();
        crate::process::spawn_task(async move {
            while this.0.state.lock().cleaning {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let tool_dir = this.tool_dir(tool);
            let archive = this.0.root.join(DOWNLOADS).join(archive_name(&release));
            let deno_dir = this.0.root.join(DENO_CACHE);
            let probe = this.0.probe.clone();
            let flag = cancel.clone();
            let result = tauri::async_runtime::spawn_blocking(move || {
                install::install_from(
                    &tool_dir, platform, &release, &archive, &probe, &deno_dir, &flag,
                )
            })
            .await
            .unwrap_or_else(|error| Err(Failure::new(ToolErrorCode::Io, error.to_string())));
            this.queue().dismiss(tool.key());
            {
                let mut state = this.0.state.lock();
                let entry = state.tool(tool);
                entry.op = None;
                entry.download = None;
                match result {
                    Ok(installed) => {
                        // i18n-ignore : journal.
                        crate::app_log!("chatonlm: {} {} installed", tool.key(), installed.version);
                        entry.installed = Some(installed);
                        entry.error = None;
                    }
                    Err(_) if cancel.load(Ordering::Relaxed) => {}
                    Err(failure) => {
                        // i18n-ignore : journal.
                        crate::app_log!(
                            "chatonlm: {} installation failed: {}",
                            tool.key(),
                            failure.detail
                        );
                        entry.error = Some(error_of(tool, &failure));
                    }
                }
            }
            this.0.emit_now();
        });
    }

    /// Arrête les téléchargements et installations en cours (archives
    /// gardées : un nouveau téléchargement reprend).
    pub fn cancel(&self) {
        {
            let state = self.0.state.lock();
            for entry in state.tools.values() {
                if let Some(op) = &entry.op {
                    op.cancel.store(true, Ordering::Relaxed);
                }
            }
        }
        for tool in ToolId::ALL {
            self.queue().cancel(tool.key());
        }
        self.0.emit_now();
    }

    /// Une opération est en cours pour l'un des outils.
    fn busy(&self) -> bool {
        let state = self.0.state.lock();
        state.cleaning || state.tools.values().any(|entry| entry.op.is_some())
    }

    /// Attend la fin des opérations en cours, `limit` au plus (`None` :
    /// sans limite ; chaque étape a ses propres délais).
    async fn wait_idle(&self, limit: Option<Duration>) -> bool {
        let deadline = limit.map(|limit| tokio::time::Instant::now() + limit);
        while self.busy() {
            if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        true
    }

    /// Retire les deux outils, leurs archives et leurs caches ; refusé
    /// pendant une transcription. Un fichier encore ouvert (Windows) est
    /// effacé au lancement suivant ; les outils sont « absents » dès que
    /// leur pointeur est retiré.
    pub async fn remove(&self) -> AppResult<()> {
        {
            let mut state = self.0.state.lock();
            if state.leases > 0 {
                return Err(AppError::Invalid(t!("tools.inUse")));
            }
            state.removing = true;
        }
        struct Removing<'a>(&'a Inner);
        impl Drop for Removing<'_> {
            fn drop(&mut self) {
                self.0.state.lock().removing = false;
            }
        }
        let _removing = Removing(&self.0);
        self.cancel();
        for tool in ToolId::ALL {
            let _ = self
                .queue()
                .wait_idle(tool.key(), Duration::from_secs(5))
                .await;
        }
        let _ = self.wait_idle(Some(BUSY_WAIT)).await;
        for tool in ToolId::ALL {
            self.queue().dismiss(tool.key());
        }
        let root = self.0.root.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let mut first_error = None;
            for tool in ToolId::ALL {
                let dir = root.join(tool.key());
                if let Err(error) = install::remove(&dir) {
                    if dir.join(install::CURRENT).exists() {
                        first_error.get_or_insert(error);
                    } else {
                        // Pointeur retiré : le reste part au lancement
                        // suivant (fichier ouvert sous Windows).
                        // i18n-ignore : journal.
                        crate::app_log!("chatonlm: {} partly removed: {error}", tool.key());
                    }
                }
            }
            for dir in [DOWNLOADS, DENO_CACHE, YTDLP_CACHE, NO_FFMPEG] {
                let _ = install::remove_dir_patiently(&root.join(dir));
            }
            match first_error {
                Some(error) => Err(error),
                None => Ok(()),
            }
        })
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?;
        {
            let mut state = self.0.state.lock();
            for tool in ToolId::ALL {
                let entry = state.tool(tool);
                entry.installed = install::installed(
                    &self.tool_dir(tool),
                    tool,
                    self.0.platform.unwrap_or(Platform::Linux),
                )
                .filter(|_| self.0.platform.is_some());
                entry.download = None;
                entry.error = None;
                entry.checked = None;
            }
        }
        self.0.emit_now();
        result
            .map_err(|error| AppError::System(t!("tools.removeFailed", reason = error.to_string())))
    }

    /// Sortie de l'app : téléchargements arrêtés (archives gardées).
    pub async fn stop_all(&self, limit: Duration) {
        {
            let state = self.0.state.lock();
            for entry in state.tools.values() {
                if let Some(op) = &entry.op {
                    op.cancel.store(true, Ordering::Relaxed);
                }
            }
        }
        self.queue().stop_all(limit).await;
    }

    // Servi par le lot 4 (lancement de yt-dlp).
    #[allow(dead_code)]
    /// Chemins des deux outils installés, relus sur le disque (programme
    /// présent : un antivirus a pu le retirer), sans bail ni réseau.
    pub fn installed_paths(&self) -> Option<ToolPaths> {
        let platform = self.0.platform?;
        let yt_dlp = install::installed(&self.tool_dir(ToolId::YtDlp), ToolId::YtDlp, platform)?;
        let deno = install::installed(&self.tool_dir(ToolId::Deno), ToolId::Deno, platform)?;
        Some(ToolPaths {
            yt_dlp: yt_dlp.binary,
            yt_dlp_version: yt_dlp.version,
            deno: deno.binary,
            deno_version: deno.version,
            deno_dir: self.0.root.join(DENO_CACHE),
            yt_dlp_cache: self.0.root.join(YTDLP_CACHE),
            no_ffmpeg: self.0.root.join(NO_FFMPEG),
        })
    }

    // Servi par le lot 4 (lancement de yt-dlp).
    #[allow(dead_code)]
    /// Pour une transcription (lot 4) : mises à jour dues relevées et
    /// installées d'abord (attendues ; un échec garde la version en
    /// service), puis un bail sur les chemins vérifiés. Outils absents (ou
    /// disparus du disque) : erreur, la page propose l'installation.
    pub async fn ready(&self) -> AppResult<ToolsLease> {
        for tool in self.due() {
            self.start(tool, Mode::Install { quiet: true });
        }
        loop {
            self.wait_idle(None).await;
            let mut state = self.0.state.lock();
            if state.removing {
                return Err(AppError::Invalid(t!("tools.removing")));
            }
            if state.cleaning || state.tools.values().any(|entry| entry.op.is_some()) {
                continue;
            }
            let Some(paths) = self.installed_paths() else {
                // Disparu du disque : l'état le dit aussi.
                for tool in ToolId::ALL {
                    let installed = self.0.platform.and_then(|platform| {
                        install::installed(&self.tool_dir(tool), tool, platform)
                    });
                    state.tool(tool).installed = installed;
                }
                drop(state);
                self.0.emit_now();
                return Err(AppError::NotFound(t!("tools.notInstalled")));
            };
            for dir in [&paths.deno_dir, &paths.yt_dlp_cache, &paths.no_ffmpeg] {
                std::fs::create_dir_all(dir)?;
            }
            state.leases += 1;
            drop(state);
            self.0.emit_now();
            return Ok(ToolsLease {
                inner: self.0.clone(),
                paths,
            });
        }
    }

    // Servi par le lot 4 (lancement de yt-dlp).
    #[allow(dead_code)]
    /// Après une erreur de version périmée (lot 4, aucun bail tenu) :
    /// relevé immédiat et installation des versions plus récentes,
    /// attendus. Vrai si une version plus récente est maintenant en
    /// service (la transcription peut être retentée).
    pub async fn refresh(&self) -> AppResult<bool> {
        let before = self.installed_paths();
        self.update()?;
        self.wait_idle(None).await;
        let after = self.installed_paths();
        Ok(after.is_some()
            && after.as_ref().map(|paths| (&paths.yt_dlp, &paths.deno))
                != before.as_ref().map(|paths| (&paths.yt_dlp, &paths.deno)))
    }
}

impl Inner {
    /// Liste de la file (appelé sous son verrou : rien n'y est rappelé).
    fn on_queue(inner: &Arc<Inner>, list: &[SpeechDownload]) {
        let mut ended = Vec::new();
        {
            let mut state = inner.state.lock();
            for tool in ToolId::ALL {
                let download = list.iter().find(|item| item.id == tool.key()).cloned();
                let entry = state.tool(tool);
                entry.download = download.clone();
                let queued = entry
                    .op
                    .as_ref()
                    .is_some_and(|op| op.stage == Stage::Queued);
                let Some(download) = download.filter(|_| queued) else {
                    continue;
                };
                match download.state {
                    SpeechDownloadState::Failed => {
                        entry.op = None;
                        let failure = Failure::new(
                            ToolErrorCode::Download,
                            download.error.unwrap_or_default(),
                        );
                        entry.error = Some(error_of(tool, &failure));
                        ended.push(tool);
                    }
                    SpeechDownloadState::Cancelled => {
                        entry.op = None;
                        ended.push(tool);
                    }
                    _ => {}
                }
            }
        }
        inner.emit_now();
        // Retirée de la file hors de son verrou (`.part` gardé après une
        // annulation : la prochaine fois reprend).
        for tool in ended {
            let weak = Arc::downgrade(inner);
            crate::process::spawn_task(async move {
                if let Some(inner) = weak.upgrade()
                    && let Some(queue) = inner.queue.get()
                {
                    let failed = queue.list().iter().any(|item| {
                        item.id == tool.key() && item.state == SpeechDownloadState::Failed
                    });
                    if failed {
                        queue.dismiss(tool.key());
                    }
                }
            });
        }
    }

    fn snapshot(&self) -> ToolsStatus {
        let mut state = self.state.lock();
        let platform = self.platform;
        let leases = state.leases;
        let mut tools = Vec::new();
        for tool in ToolId::ALL {
            let entry = state.tool(tool);
            let installed = entry.installed.as_ref();
            let downloading = entry
                .download
                .as_ref()
                .filter(|download| active(download.state));
            let latest = entry
                .checked
                .as_ref()
                .map(|checked| release::version_of(tool, &checked.latest).to_owned());
            let newer = match (&entry.checked, installed) {
                (Some(checked), Some(installed)) => release::newer(&checked.latest, &installed.tag),
                _ => false,
            };
            let quiet_fetch = entry.op.as_ref().is_some_and(|op| {
                op.stage == Stage::Fetching && op.mode != Mode::Install { quiet: false }
            });
            let phase = match entry.op.as_ref().map(|op| op.stage) {
                Some(Stage::Fetching) if !quiet_fetch => ToolPhase::Fetching,
                Some(Stage::Queued) => match downloading.map(|download| download.state) {
                    Some(SpeechDownloadState::Verifying) => ToolPhase::Verifying,
                    _ => ToolPhase::Downloading,
                },
                Some(Stage::Installing) => ToolPhase::Installing,
                _ if installed.is_some() && newer => ToolPhase::UpdateAvailable,
                _ if installed.is_some() => ToolPhase::Installed,
                _ if quiet_fetch => ToolPhase::Absent,
                _ if entry.error.is_some() => ToolPhase::Failed,
                _ => ToolPhase::Absent,
            };
            let release = entry.op.as_ref().and_then(|op| op.release.as_ref());
            tools.push(ToolStatus {
                tool,
                name: tool.name().to_owned(),
                phase,
                version: installed.map(|installed| installed.version.clone()),
                latest,
                checked_at: entry.checked.as_ref().map(|checked| checked.checked_at),
                completed_bytes: downloading.map_or(0, |download| download.completed_bytes),
                total_bytes: downloading.map_or(0, |download| download.total_bytes),
                download_bytes: release.map_or_else(
                    || platform.map_or(0, |platform| tool.estimate(platform)),
                    |release| release.size,
                ),
                installed_bytes: installed.map(|installed| installed.bytes),
                license: tool.license().to_owned(),
                license_url: tool.license_url().to_owned(),
                third_party_licenses: installed.and_then(|installed| {
                    tool.third_party_licenses()
                        .map(|relative| installed.dir.join(relative).to_string_lossy().into_owned())
                }),
                error: entry.error.clone(),
            });
        }
        ToolsStatus {
            supported: platform.is_some(),
            ready: tools.iter().all(|tool| tool.version.is_some()),
            in_use: leases > 0,
            tools,
        }
    }

    fn emit_now(&self) {
        (self.emit)(&self.snapshot());
    }
}
