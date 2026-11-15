//! Audio d'une adresse (vidéo en ligne) récupéré par yt-dlp pour la
//! Transcription : un fichier de travail, jamais un enregistrement proposé à
//! l'utilisateur (rapport 105, § 6).
//!
//! - Adresse vérifiée d'abord ([`check_address`]) : http ou https, avec un
//!   hôte, sans identifiants ni espace ; jamais un fichier local ni une
//!   option déguisée (elle passe de toute façon après `--`).
//! - Outils servis par [`Tools::ready`] (mises à jour dues, ou déjà
//!   relevées, installées d'abord) ; leur bail est tenu pendant tout le
//!   téléchargement.
//! - Deux temps, mêmes options communes ([`common_args`]) :
//!   1. métadonnées (`--flat-playlist -J`, rien n'est téléchargé), écrites
//!      dans `info.json` : fiche ([`FetchProgress::Info`]), refus d'une
//!      liste de lecture, d'un direct (en cours, à venir ou juste fini) et
//!      d'une durée au-delà de `max_duration_s` (`TooLong`), avant tout
//!      téléchargement ;
//!   2. audio seul (`--load-info-json`, sans nouvelle extraction : la fiche
//!      chargée n'a plus de `webpage_url`, sans quoi yt-dlp réextrairait
//!      l'adresse après un échec du téléchargement, hors des refus du
//!      premier temps), sélection
//!      [`SELECTION`] : Opus en webm (251, 250, 249 sur YouTube), puis m4a
//!      AAC-LC (140), puis HE-AAC (139), par HTTP direct d'abord. Jamais de
//!      conversion : `--ffmpeg-location` désigne un dossier vide de l'app,
//!      pas de `-x`. Progression par `--progress-template` (lignes
//!      [`PROGRESS_PREFIX`]), chemin final par `--print after_move:`
//!      ([`FILE_PREFIX`]).
//! - Deno : `--no-js-runtimes --js-runtimes deno:<chemin>` (jamais un Deno
//!   ou un Node de la machine), `DENO_DIR` sous les données de l'app (dans
//!   [`ToolPaths::environment`]) ; cache de yt-dlp `--cache-dir` sous les
//!   données de l'app ; `TMPDIR`, `TMP` et `TEMP` dans le dossier privé du
//!   téléchargement. `HOME` n'est pas changé : avec `--ignore-config`,
//!   `--cache-dir` et `--no-cookies`, yt-dlp 2026.10.26 et Deno 2.9.8 n'y
//!   écrivent rien (vérifié par `scripts/record-ytdlp-fixtures.sh`, HOME
//!   neuf resté vide).
//! - Bornes ([`FetchLimits`]) tenues par ChatonLM pendant le téléchargement,
//!   d'après les lignes de progression et, chaque seconde, la taille du
//!   dossier privé : octets reçus (un flux sans fin, webradio ou direct mal
//!   annoncé, n'a ni durée ni taille : yt-dlp le téléchargerait sans fin, et
//!   son `--max-filesize` ne lit que la taille annoncée) et place libre du
//!   disque (avant le deuxième temps, puis pendant) ; au-delà, arbre de
//!   processus arrêté, `TooLarge` ou `NoSpace`.
//! - Tout passe par un dossier privé `<work_dir>/.fetch/` : à la fin
//!   (réussite, échec, annulation), seul `<work_dir>/audio.<ext>` reste
//!   (réussite) ; le dossier privé est effacé, `.part` compris.
//! - Annulation : tout l'arbre de processus est arrêté (module `process` :
//!   groupe de processus et SIGTERM puis SIGKILL sous Linux, Job Object
//!   sous Windows), Deno compris.
//! - Erreurs classées par motifs des messages réels ([`classify`] ;
//!   fixtures `ytdlp/fixtures/`, enregistrées par
//!   `scripts/record-ytdlp-fixtures.sh` ou relevées dans le code de yt-dlp
//!   2026.10.26) : la raison vient de YouTube et peut changer, le texte brut
//!   reste dans [`FetchError::detail`] (journal). `Outdated` : relevé de la
//!   dernière version ([`Tools::refresh`]) puis UNE nouvelle tentative.
//!
//! Écarts à l'API prévue : aucun (annulation : le `watch::Receiver<bool>`
//! des envois, comme `chat::pull`). [`FetchErrorCode`] est exporté en
//! ts-rs ; `RateLimited` et `Drm` de la recherche sont rangés dans
//! `BotCheck` et `Unsupported`.
//!
//! Windows : même code. `.no_window()` et `CREATE_NEW_PROCESS_GROUP`
//! (`process::isolate`), Job Object (Deno tué avec yt-dlp),
//! `PYTHONUTF8=1` (sortie en UTF-8 et non dans la page de code de la
//! console), `deno:C:\…\deno.exe` (yt-dlp coupe au premier « : »),
//! arguments en UTF-16 par `Command` (noms d'utilisateur non ASCII),
//! fichiers effacés patiemment (fichier encore ouvert juste après l'arrêt).

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};
use ts_rs::TS;

use crate::i18n::t;
use crate::os_util::NoWindow;
use crate::process::{Spawner, isolate, spawn_with, terminate, watch_exit};
use crate::tools::{ToolPaths, Tools, ToolsLease};

/// Fiche de la vidéo, lue avant le téléchargement.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfo {
    pub title: Option<String>,
    /// Chaîne, sinon auteur de la mise en ligne.
    pub channel: Option<String>,
    pub duration_s: Option<f64>,
    /// Adresse de la page rendue par yt-dlp (sinon celle donnée).
    pub webpage_url: String,
    /// Extracteur de yt-dlp (« youtube », « vimeo », « generic »…).
    pub site: String,
}

/// Avancement.
#[derive(Clone, Debug, PartialEq)]
pub enum FetchProgress {
    /// Métadonnées lues (premier temps), avant le téléchargement.
    Info(VideoInfo),
    /// Octets de l'audio reçus (deuxième temps) ; `total` : taille annoncée
    /// ou estimée.
    Downloading { received: u64, total: Option<u64> },
}

/// Audio de travail obtenu, `<work_dir>/audio.<ext>`.
#[derive(Clone, Debug, PartialEq)]
pub struct FetchedAudio {
    pub path: PathBuf,
    pub info: VideoInfo,
}

/// Nature d'un échec (phrase pour la page : [`message`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FetchErrorCode {
    /// yt-dlp ou Deno absents (ou en cours de retrait).
    ToolsMissing,
    /// Adresse refusée avant tout lancement.
    InvalidAddress,
    /// Site non reconnu, ou contenu protégé (DRM).
    Unsupported,
    /// Liste de lecture (une seule vidéo à la fois).
    Playlist,
    /// Direct en cours, à venir ou juste fini.
    Live,
    Private,
    /// Réservée aux adultes connectés.
    AgeRestricted,
    /// Réservée aux membres ou aux abonnés payants.
    MembersOnly,
    /// Introuvable ou supprimée.
    Unavailable,
    GeoBlocked,
    /// Le site demande une connexion (« not a bot », captcha, trop de
    /// requêtes).
    BotCheck,
    Network,
    /// yt-dlp ou Deno ne savent plus lire le site, même après le relevé de
    /// la dernière version.
    Outdated,
    /// Plus longue que la durée permise.
    TooLong,
    /// Audio plus gros que la borne de ChatonLM, ou sans fin (webradio,
    /// direct non annoncé comme tel).
    TooLarge,
    /// Plus assez de place libre sur le disque des données.
    NoSpace,
    Cancelled,
    /// Le reste (détail dans le journal).
    Failed,
}

/// Échec : nature, et texte brut (fin de la sortie d'erreur de yt-dlp, ou
/// cause), pour le journal seulement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchError {
    pub code: FetchErrorCode,
    pub detail: String,
}

impl FetchError {
    fn new(code: FetchErrorCode, detail: impl Into<String>) -> FetchError {
        FetchError {
            code,
            detail: detail.into(),
        }
    }

    fn cancelled() -> FetchError {
        FetchError::new(FetchErrorCode::Cancelled, "")
    }
}

/// Préfixe des lignes de progression (`--progress-template`).
pub(crate) const PROGRESS_PREFIX: &str = "CHATONLM-PROGRESS";
/// Préfixe de la ligne du chemin final (`--print after_move:`).
pub(crate) const FILE_PREFIX: &str = "CHATONLM-FILE";
/// Sélection de l'audio, sans ffmpeg : formats que `media-audio` décode,
/// par HTTP direct (pas de HLS, que le téléchargeur natif écrirait en
/// MPEG-TS) ; le meilleur Opus est 251, puis 250, puis 249.
pub(crate) const SELECTION: &str = "ba[acodec=opus][protocol^=http]/ba[acodec=mp4a.40.2][protocol^=http]/ba[acodec^=mp4a][protocol^=http]/ba[protocol^=http]/b[protocol^=http]/ba/b";
/// Dossier privé du téléchargement, sous `work_dir`.
const FETCH_DIR: &str = ".fetch";
/// Métadonnées du premier temps (dans le dossier privé).
const INFO_FILE: &str = "info.json";
/// Métadonnées données au deuxième temps (sans `webpage_url`).
const LOAD_FILE: &str = "load.json";
/// Nom (sans extension) de l'audio.
const AUDIO_STEM: &str = "audio";
/// Métadonnées plus grandes : refusées (une vraie fiche YouTube fait
/// quelques centaines de Ko).
const INFO_LIMIT: u64 = 64 << 20;
/// Fin de la sortie d'erreur gardée.
const STDERR_LIMIT: usize = 64 << 10;
/// Ligne de sortie standard plus longue : coupée.
const LINE_LIMIT: usize = 1 << 20;
/// Longueur d'adresse acceptée.
const ADDRESS_LIMIT: usize = 4096;

/// Octets reçus au plus : la limite d'un fichier local de la Transcription
/// (`media_audio::Limits`), au-delà de laquelle le décodage refuserait.
pub const MAX_BYTES: u64 = super::job::MAX_GIGABYTES << 30;
/// Octets reçus au plus quand ni la durée ni la taille ne sont annoncées
/// (flux) : 2 Gio, plus de six heures d'audio à 320 kb/s.
pub const STREAM_BYTES: u64 = 2 << 30;
/// Place laissée libre sur le disque des données (l'audio de travail du
/// décodage, 230 Mo par heure, s'écrit ensuite).
pub const DISK_RESERVE: u64 = 1 << 30;
/// Période des relevés de taille et de place pendant un lancement.
const GUARD_TICK: Duration = Duration::from_secs(1);

/// Délais de l'arrêt (SIGTERM puis SIGKILL ; Windows : arrêt du Job).
const STOP_GRACE: Duration = Duration::from_secs(2);
const KILL_WAIT: Duration = Duration::from_secs(5);
/// Fin des lectures après la fin du processus.
const DRAIN_WAIT: Duration = Duration::from_secs(3);

/// Fil qui crée les processus de yt-dlp (vit autant que l'app :
/// `PR_SET_PDEATHSIG` suit le fil créateur).
static SPAWNER: parking_lot::Mutex<Option<Spawner>> = parking_lot::const_mutex(None);

/// Bornes d'une récupération.
#[derive(Clone, Copy, Debug)]
pub struct FetchLimits {
    /// Durée annoncée au-delà de laquelle la vidéo est refusée avant le
    /// téléchargement (durée inconnue : acceptée, les octets la bornent).
    pub max_duration_s: Option<f64>,
    /// Octets reçus au plus (taille annoncée comprise).
    pub max_bytes: u64,
    /// Octets reçus au plus sans durée ni taille annoncées.
    pub stream_bytes: u64,
    /// Place à laisser libre sur le disque du dossier de travail.
    pub reserve_bytes: u64,
    /// Place libre d'un dossier (remplacée dans les essais).
    pub free_space: fn(&Path) -> Option<u64>,
}

impl FetchLimits {
    /// Bornes de l'app, avec `max_duration_s`.
    pub fn standard(max_duration_s: Option<f64>) -> FetchLimits {
        FetchLimits {
            max_duration_s,
            max_bytes: MAX_BYTES,
            stream_bytes: STREAM_BYTES,
            reserve_bytes: DISK_RESERVE,
            free_space: crate::disk::free_space_for,
        }
    }
}

/// Récupère l'audio de `url` dans `work_dir` (créé s'il manque) : voir
/// l'en-tête du module. `max_duration_s` : durée au-delà de laquelle la
/// vidéo est refusée avant le téléchargement (durée inconnue : acceptée) ;
/// octets et place libre bornés par [`FetchLimits::standard`].
pub async fn fetch_audio(
    tools: &Tools,
    url: &str,
    work_dir: &Path,
    max_duration_s: Option<f64>,
    cancel: watch::Receiver<bool>,
    on_progress: impl Fn(FetchProgress) + Send + Sync,
) -> Result<FetchedAudio, FetchError> {
    let limits = FetchLimits::standard(max_duration_s);
    fetch_with(tools, url, work_dir, &limits, cancel, &on_progress).await
}

/// Phrase claire pour la page, dans la langue du moment.
pub fn message(code: FetchErrorCode) -> String {
    match code {
        FetchErrorCode::ToolsMissing => t!("ytdlp.toolsMissing"),
        FetchErrorCode::InvalidAddress => t!("ytdlp.invalidAddress"),
        FetchErrorCode::Unsupported => t!("ytdlp.unsupported"),
        FetchErrorCode::Playlist => t!("ytdlp.playlist"),
        FetchErrorCode::Live => t!("ytdlp.live"),
        FetchErrorCode::Private => t!("ytdlp.private"),
        FetchErrorCode::AgeRestricted => t!("ytdlp.ageRestricted"),
        FetchErrorCode::MembersOnly => t!("ytdlp.membersOnly"),
        FetchErrorCode::Unavailable => t!("ytdlp.unavailable"),
        FetchErrorCode::GeoBlocked => t!("ytdlp.geoBlocked"),
        FetchErrorCode::BotCheck => t!("ytdlp.botCheck"),
        FetchErrorCode::Network => t!("ytdlp.network"),
        FetchErrorCode::Outdated => t!("ytdlp.outdated"),
        FetchErrorCode::TooLong => t!("ytdlp.tooLong"),
        FetchErrorCode::TooLarge => t!("ytdlp.tooLarge"),
        FetchErrorCode::NoSpace => t!("ytdlp.noSpace"),
        FetchErrorCode::Cancelled => t!("ytdlp.cancelled"),
        FetchErrorCode::Failed => t!("ytdlp.failed"),
    }
}

/// Source des outils : [`Tools`] dans l'app, un double dans les essais.
pub(crate) trait Provider: Sync {
    /// Bail rendu à la fin d'une tentative.
    type Lease: Send;
    fn ready(&self) -> impl Future<Output = Result<(ToolPaths, Self::Lease), FetchError>> + Send;
    /// Vrai si une version plus récente est maintenant en service.
    fn refresh(&self) -> impl Future<Output = bool> + Send;
}

impl Provider for Tools {
    type Lease = ToolsLease;

    async fn ready(&self) -> Result<(ToolPaths, ToolsLease), FetchError> {
        let lease = Tools::ready(self)
            .await
            .map_err(|error| FetchError::new(FetchErrorCode::ToolsMissing, error.to_string()))?;
        Ok((lease.paths.clone(), lease))
    }

    async fn refresh(&self) -> bool {
        match Tools::refresh(self).await {
            Ok(newer) => newer,
            Err(error) => {
                // i18n-ignore : journal.
                crate::app_log!("chatonlm: yt-dlp update check failed: {error}");
                false
            }
        }
    }
}

pub(crate) async fn fetch_with<P: Provider>(
    provider: &P,
    url: &str,
    work_dir: &Path,
    limits: &FetchLimits,
    mut cancel: watch::Receiver<bool>,
    on_progress: &(impl Fn(FetchProgress) + Send + Sync),
) -> Result<FetchedAudio, FetchError> {
    let url = check_address(url)?;
    let mut retried = false;
    loop {
        if *cancel.borrow() {
            return Err(FetchError::cancelled());
        }
        let (paths, lease) = tokio::select! {
            ready = provider.ready() => ready?,
            () = cancelled(&mut cancel) => return Err(FetchError::cancelled()),
        };
        let result = attempt(&paths, &url, work_dir, limits, &cancel, on_progress).await;
        drop(lease);
        match result {
            Err(error) if error.code == FetchErrorCode::Outdated && !retried => {
                retried = true;
                let newer = tokio::select! {
                    newer = provider.refresh() => newer,
                    () = cancelled(&mut cancel) => return Err(FetchError::cancelled()),
                };
                if !newer {
                    return Err(error);
                }
            }
            other => return other,
        }
    }
}

/// Une tentative : dossier privé effacé quoi qu'il arrive.
async fn attempt(
    paths: &ToolPaths,
    url: &str,
    work_dir: &Path,
    limits: &FetchLimits,
    cancel: &watch::Receiver<bool>,
    on_progress: &(impl Fn(FetchProgress) + Send + Sync),
) -> Result<FetchedAudio, FetchError> {
    let fetch_dir = work_dir.join(FETCH_DIR);
    let io = |error: std::io::Error| FetchError::new(FetchErrorCode::Failed, error.to_string());
    remove_patiently(&fetch_dir).await.map_err(io)?;
    std::fs::create_dir_all(&fetch_dir).map_err(io)?;
    let result = two_steps(
        paths,
        url,
        work_dir,
        &fetch_dir,
        limits,
        cancel,
        on_progress,
    )
    .await;
    if let Err(error) = remove_patiently(&fetch_dir).await {
        // i18n-ignore : journal.
        crate::app_log!("chatonlm: cannot remove the yt-dlp work folder: {error}");
    }
    result
}

async fn two_steps(
    paths: &ToolPaths,
    url: &str,
    work_dir: &Path,
    fetch_dir: &Path,
    limits: &FetchLimits,
    cancel: &watch::Receiver<bool>,
    on_progress: &(impl Fn(FetchProgress) + Send + Sync),
) -> Result<FetchedAudio, FetchError> {
    // 1. Métadonnées (fichier borné pendant l'écriture).
    let info_path = fetch_dir.join(INFO_FILE);
    let ended = run(
        paths,
        info_args(paths, url),
        fetch_dir,
        Output::File(&info_path),
        cancel.clone(),
        |seen| match seen {
            Seen::Line(_) => Ok(()),
            Seen::Tick => match std::fs::metadata(&info_path) {
                // i18n-ignore : journal.
                Ok(metadata) if metadata.len() > INFO_LIMIT => Err(FetchError::new(
                    FetchErrorCode::Failed,
                    "metadata too large",
                )),
                _ => Ok(()),
            },
        },
    )
    .await?;
    if !ended.success {
        return Err(failure(&ended));
    }
    let value = read_info_value(&info_path)?;
    let info = info_of(&value, url)?;
    if let Some(max) = limits.max_duration_s
        && info.duration_s.is_some_and(|duration| duration > max)
    {
        return Err(FetchError::new(
            FetchErrorCode::TooLong,
            // i18n-ignore : journal.
            format!("{:?} s > {max} s", info.duration_s),
        ));
    }
    on_progress(FetchProgress::Info(info.clone()));

    // 2. Audio seul, d'après une fiche sans `webpage_url` : sur un échec du
    // téléchargement, yt-dlp n'a plus d'adresse à réextraire
    // (`download_with_info_file` relancerait sinon une extraction complète,
    // qui échapperait aux refus ci-dessus).
    let load_path = fetch_dir.join(LOAD_FILE);
    write_load_info(&load_path, value)?;
    let mut guard = Guard::new(limits, fetch_dir, info.duration_s.is_some());
    guard.check_space(0)?;
    let mut file: Option<String> = None;
    let ended = run(
        paths,
        download_args(paths, &load_path, fetch_dir),
        fetch_dir,
        Output::Lines,
        cancel.clone(),
        |seen| match seen {
            Seen::Line(line) => match parse_line(line) {
                Some(Line::Progress { received, total }) => {
                    guard.progress(received, total)?;
                    on_progress(FetchProgress::Downloading { received, total });
                    Ok(())
                }
                Some(Line::File(path)) => {
                    file = Some(path);
                    Ok(())
                }
                None => Ok(()),
            },
            Seen::Tick => guard.tick(),
        },
    )
    .await?;
    if !ended.success {
        return Err(failure(&ended));
    }
    let audio = downloaded(fetch_dir, file.as_deref()).ok_or_else(|| {
        // i18n-ignore : journal.
        FetchError::new(FetchErrorCode::Failed, "yt-dlp ended without an audio file")
    })?;
    let ext = audio
        .extension()
        .map(OsStr::to_os_string)
        .unwrap_or_default();
    let mut dest = work_dir.join(AUDIO_STEM);
    dest.set_extension(ext);
    let io = |error: std::io::Error| FetchError::new(FetchErrorCode::Failed, error.to_string());
    if dest.exists() {
        std::fs::remove_file(&dest).map_err(io)?;
    }
    std::fs::rename(&audio, &dest).map_err(io)?;
    Ok(FetchedAudio { path: dest, info })
}

/// Fiche donnée au deuxième temps : celle du premier, sans `webpage_url`
/// (ni `original_url`) : yt-dlp ne s'en sert que pour réextraire après un
/// échec, et pour les raccourcis Internet (`--write-link`, jamais passé).
pub(crate) fn write_load_info(path: &Path, mut value: Value) -> Result<(), FetchError> {
    if let Some(object) = value.as_object_mut() {
        object.remove("webpage_url");
        object.remove("original_url");
    }
    std::fs::write(path, value.to_string())
        .map_err(|error| FetchError::new(FetchErrorCode::Failed, error.to_string()))
}

/// Bornes tenues pendant le deuxième temps (voir [`FetchLimits`]).
pub(crate) struct Guard<'a> {
    limits: &'a FetchLimits,
    dir: &'a Path,
    /// Durée annoncée par la fiche.
    known_duration: bool,
    /// Taille annoncée par la dernière ligne de progression.
    total: Option<u64>,
    /// Octets reçus d'après la dernière ligne de progression.
    received: u64,
}

impl<'a> Guard<'a> {
    pub(crate) fn new(limits: &'a FetchLimits, dir: &'a Path, known_duration: bool) -> Self {
        Guard {
            limits,
            dir,
            known_duration,
            total: None,
            received: 0,
        }
    }

    /// Plafond des octets : plus bas pour un flux (ni durée ni taille).
    fn cap(&self) -> u64 {
        if self.known_duration || self.total.is_some() {
            self.limits.max_bytes
        } else {
            self.limits.stream_bytes.min(self.limits.max_bytes)
        }
    }

    fn too_large(&self, bytes: u64) -> Result<(), FetchError> {
        if bytes > self.cap() {
            // i18n-ignore : journal.
            return Err(FetchError::new(
                FetchErrorCode::TooLarge,
                format!(
                    "{bytes} bytes > {} bytes (total announced: {:?})",
                    self.cap(),
                    self.total
                ),
            ));
        }
        Ok(())
    }

    /// Place libre : de quoi finir (`remaining` octets encore attendus) et
    /// garder la réserve.
    pub(crate) fn check_space(&self, remaining: u64) -> Result<(), FetchError> {
        if let Some(free) = (self.limits.free_space)(self.dir) {
            let needed = remaining.saturating_add(self.limits.reserve_bytes);
            if free < needed {
                // i18n-ignore : journal.
                return Err(FetchError::new(
                    FetchErrorCode::NoSpace,
                    format!("{free} bytes free, {needed} bytes needed"),
                ));
            }
        }
        Ok(())
    }

    /// Ligne de progression : taille annoncée, octets reçus, place libre.
    pub(crate) fn progress(&mut self, received: u64, total: Option<u64>) -> Result<(), FetchError> {
        if total.is_some() {
            self.total = total;
        }
        self.received = received;
        if let Some(total) = self.total {
            self.too_large(total)?;
        }
        self.too_large(received)?;
        let remaining = self.total.map_or(0, |total| total.saturating_sub(received));
        self.check_space(remaining)
    }

    /// Relevé périodique : taille du dossier privé (même sans ligne de
    /// progression) et place libre.
    pub(crate) fn tick(&self) -> Result<(), FetchError> {
        self.too_large(folder_size(self.dir))?;
        let remaining = self
            .total
            .map_or(0, |total| total.saturating_sub(self.received));
        self.check_space(remaining)
    }
}

/// Taille des fichiers sous `dir` (sous-dossiers compris, liens non suivis).
pub(crate) fn folder_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file()
                && let Ok(metadata) = entry.metadata()
            {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
}

/// Vérifie l'adresse donnée ; rend sa forme normalisée.
pub(crate) fn check_address(url: &str) -> Result<String, FetchError> {
    let invalid = |why: &str| FetchError::new(FetchErrorCode::InvalidAddress, why);
    let url = url.trim();
    // i18n-ignore : journal (les raisons ci-dessous aussi).
    if url.is_empty() {
        return Err(invalid("empty address"));
    }
    if url.len() > ADDRESS_LIMIT {
        return Err(invalid("address too long"));
    }
    if url.starts_with('-') {
        return Err(invalid("looks like an option"));
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(invalid("space or control character"));
    }
    let parsed = url::Url::parse(url).map_err(|error| invalid(&error.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(invalid("not http or https"));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(invalid("no host"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(invalid("credentials in the address"));
    }
    Ok(parsed.as_str().to_owned())
}

/// Options des deux temps.
pub(crate) fn common_args(paths: &ToolPaths) -> Vec<OsString> {
    let mut deno = OsString::from("deno:");
    deno.push(&paths.deno);
    let mut args: Vec<OsString> = [
        "--ignore-config",
        "--no-plugin-dirs",
        "--no-update",
        "--no-playlist",
        "--no-colors",
        "--newline",
        "--encoding",
        "utf-8",
        "--no-cookies",
        "--socket-timeout",
        "30",
        "--retries",
        "3",
        "--fragment-retries",
        "3",
        "--extractor-retries",
        "1",
        "--no-js-runtimes",
        "--js-runtimes",
    ]
    .map(OsString::from)
    .to_vec();
    args.push(deno);
    args.push("--cache-dir".into());
    args.push(paths.yt_dlp_cache.clone().into());
    args.push("--ffmpeg-location".into());
    args.push(paths.no_ffmpeg.clone().into());
    args
}

/// Premier temps : métadonnées seules (une liste de lecture n'est pas
/// parcourue : `--flat-playlist`).
pub(crate) fn info_args(paths: &ToolPaths, url: &str) -> Vec<OsString> {
    let mut args = common_args(paths);
    args.extend(["--flat-playlist", "-J", "--", url].map(OsString::from));
    args
}

/// Deuxième temps : audio seul, d'après les métadonnées (sans nouvelle
/// extraction). `-P` donne le dossier hors gabarit (un « % » du chemin
/// n'est pas lu comme un champ).
pub(crate) fn download_args(paths: &ToolPaths, info: &Path, fetch_dir: &Path) -> Vec<OsString> {
    let mut args = common_args(paths);
    args.push("--load-info-json".into());
    args.push(info.into());
    args.push("-P".into());
    args.push(fetch_dir.into());
    let template = format!(
        "download:{PROGRESS_PREFIX} %(progress.status)s %(progress.downloaded_bytes)s \
         %(progress.total_bytes)s %(progress.total_bytes_estimate)s"
    );
    let file = format!("after_move:{FILE_PREFIX} %(filepath)j");
    args.extend(
        [
            "-f",
            SELECTION,
            "-o",
            "audio.%(ext)s",
            "--progress",
            "--progress-delta",
            "0.5",
            "--progress-template",
            &template,
            "--print",
            &file,
        ]
        .map(OsString::from),
    );
    args
}

/// Environnement de yt-dlp : celui des outils, et les fichiers
/// temporaires (Python, Deno) dans le dossier privé.
fn environment(paths: &ToolPaths, fetch_dir: &Path) -> BTreeMap<OsString, OsString> {
    let mut environment = paths.environment();
    for key in ["TMPDIR", "TMP", "TEMP"] {
        environment.insert(key.into(), fetch_dir.as_os_str().to_owned());
    }
    environment
}

/// Ligne utile de la sortie standard.
#[derive(Debug, PartialEq)]
pub(crate) enum Line {
    Progress { received: u64, total: Option<u64> },
    File(String),
}

/// Lit une ligne de la sortie standard (le reste est ignoré).
pub(crate) fn parse_line(line: &str) -> Option<Line> {
    let line = line.trim_end_matches(['\r', '\n']);
    if let Some(rest) = line.strip_prefix(PROGRESS_PREFIX) {
        let mut fields = rest.split_whitespace();
        let _status = fields.next()?;
        let number = |field: Option<&str>| {
            field
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
                .map(|value| value as u64)
        };
        let received = number(fields.next()).unwrap_or(0);
        let total = number(fields.next());
        let estimate = number(fields.next());
        return Some(Line::Progress {
            received,
            total: total.or(estimate),
        });
    }
    let json = line.strip_prefix(FILE_PREFIX)?.trim();
    serde_json::from_str::<String>(json).ok().map(Line::File)
}

/// Fiche lue dans les métadonnées : dernière ligne JSON du fichier (rien
/// d'autre n'y est écrit par yt-dlp ; le double des essais y met une ligne
/// du lanceur d'essais avant).
#[cfg(test)]
pub(crate) fn read_info(path: &Path, url: &str) -> Result<VideoInfo, FetchError> {
    info_of(&read_info_value(path)?, url)
}

/// Dernière ligne JSON des métadonnées (voir [`read_info`]).
fn read_info_value(path: &Path) -> Result<Value, FetchError> {
    let failed = |detail: String| FetchError::new(FetchErrorCode::Failed, detail);
    let size = std::fs::metadata(path)
        .map_err(|error| failed(error.to_string()))?
        .len();
    if size > INFO_LIMIT {
        // i18n-ignore : journal.
        return Err(failed(format!("metadata too large ({size} bytes)")));
    }
    let text = std::fs::read(path).map_err(|error| failed(error.to_string()))?;
    let text = String::from_utf8_lossy(&text);
    let value = text
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
        .find_map(|line| serde_json::from_str::<Value>(line).ok())
        // i18n-ignore : journal.
        .ok_or_else(|| failed("no metadata from yt-dlp".into()))?;
    Ok(value)
}

pub(crate) fn info_of(value: &Value, url: &str) -> Result<VideoInfo, FetchError> {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let kind = text("_type").unwrap_or_else(|| "video".into());
    if matches!(kind.as_str(), "playlist" | "multi_video") {
        return Err(FetchError::new(FetchErrorCode::Playlist, kind));
    }
    if kind != "video" {
        // i18n-ignore : journal.
        return Err(FetchError::new(
            FetchErrorCode::Failed,
            format!("_type {kind}"),
        ));
    }
    let live_status = text("live_status");
    if value.get("is_live").and_then(Value::as_bool) == Some(true)
        || matches!(
            live_status.as_deref(),
            Some("is_live" | "is_upcoming" | "post_live")
        )
    {
        return Err(FetchError::new(
            FetchErrorCode::Live,
            live_status.unwrap_or_else(|| "is_live".into()),
        ));
    }
    Ok(VideoInfo {
        title: text("title").or_else(|| text("fulltitle")),
        channel: text("channel").or_else(|| text("uploader")),
        duration_s: value
            .get("duration")
            .and_then(Value::as_f64)
            .filter(|duration| duration.is_finite() && *duration >= 0.0),
        webpage_url: text("webpage_url")
            .or_else(|| text("original_url"))
            .unwrap_or_else(|| url.to_owned()),
        site: text("extractor")
            .or_else(|| text("extractor_key"))
            .unwrap_or_default()
            .to_lowercase(),
    })
}

/// Fichier final : celui annoncé par `--print after_move:` s'il est bien
/// dans le dossier privé, sinon le seul `audio.*` complet qui s'y trouve.
fn downloaded(fetch_dir: &Path, announced: Option<&str>) -> Option<PathBuf> {
    let inside = |path: &Path| {
        let (Ok(dir), Ok(path)) = (fetch_dir.canonicalize(), path.canonicalize()) else {
            return false;
        };
        path.parent() == Some(dir.as_path()) && path.is_file()
    };
    if let Some(path) = announced.map(PathBuf::from)
        && inside(&path)
    {
        return Some(path);
    }
    std::fs::read_dir(fetch_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            name.starts_with("audio.")
                && !name.ends_with(".part")
                && !name.ends_with(".ytdl")
                && !name.ends_with(".json")
                && path.is_file()
        })
}

/// Fin d'un lancement de yt-dlp.
struct Ended {
    success: bool,
    /// Fin de la sortie d'erreur.
    stderr: String,
    /// Fin du processus, en clair (journal).
    exit: String,
}

fn failure(ended: &Ended) -> FetchError {
    let code = classify(&ended.stderr);
    let detail = error_lines(&ended.stderr).unwrap_or_else(|| tail(&ended.stderr, 2000));
    // i18n-ignore : journal.
    FetchError::new(code, format!("{} ({})", detail.trim(), ended.exit))
}

enum Output<'a> {
    /// Sortie standard écrite dans ce fichier.
    File(&'a Path),
    /// Sortie standard lue ligne à ligne.
    Lines,
}

/// Ce que voit la garde d'un lancement.
enum Seen<'a> {
    /// Ligne de la sortie standard.
    Line(&'a str),
    /// Relevé périodique ([`GUARD_TICK`]).
    Tick,
}

/// Lance yt-dlp jusqu'à sa fin ; annulation : arbre de processus arrêté,
/// `Cancelled` ; `guard` (chaque ligne de la sortie standard, chaque
/// seconde) qui rend une erreur : arbre arrêté, cette erreur.
async fn run(
    paths: &ToolPaths,
    args: Vec<OsString>,
    fetch_dir: &Path,
    output: Output<'_>,
    mut cancel: watch::Receiver<bool>,
    mut guard: impl FnMut(Seen<'_>) -> Result<(), FetchError>,
) -> Result<Ended, FetchError> {
    let failed = |error: std::io::Error| FetchError::new(FetchErrorCode::Failed, error.to_string());
    if *cancel.borrow() {
        return Err(FetchError::cancelled());
    }
    let mut command = Command::new(&paths.yt_dlp);
    command
        .no_window()
        .args(&args)
        .env_clear()
        .envs(environment(paths, fetch_dir))
        .current_dir(fetch_dir)
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    match output {
        Output::File(path) => command.stdout(std::fs::File::create(path).map_err(failed)?),
        Output::Lines => command.stdout(Stdio::piped()),
    };
    // Groupe à lui (Linux) ou Job Object (Windows) : l'arrêt atteint Deno.
    isolate(&mut command);
    let mut child = spawn_with(&SPAWNER, "chatonlm-ytdlp", command)
        .await
        .map_err(|error| {
            // i18n-ignore : journal.
            FetchError::new(
                FetchErrorCode::ToolsMissing,
                format!("yt-dlp doesn't start: {error}"),
            )
        })?;
    let (line_sender, mut lines) = mpsc::unbounded_channel::<String>();
    if let Some(stdout) = child.stdout.take() {
        read_lines(stdout, line_sender);
    } else {
        drop(line_sender);
    }
    let stderr = read_tail(child.stderr.take());
    let pid = child.id();
    let mut exited = watch_exit(child);
    let mut open = true;
    let mut ticks = tokio::time::interval(GUARD_TICK);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let status = loop {
        let seen = tokio::select! {
            () = cancelled(&mut cancel) => {
                terminate(pid, &mut exited, STOP_GRACE, KILL_WAIT).await;
                return Err(FetchError::cancelled());
            }
            line = lines.recv(), if open => match line {
                Some(line) => guard(Seen::Line(&line)),
                None => {
                    open = false;
                    Ok(())
                }
            },
            _ = ticks.tick() => guard(Seen::Tick),
            status = &mut exited => break status.ok().flatten(),
        };
        if let Err(error) = seen {
            terminate(pid, &mut exited, STOP_GRACE, KILL_WAIT).await;
            return Err(error);
        }
    };
    // Lignes encore en route (le tube se ferme avec le dernier processus du
    // groupe, arrêté par `watch_exit`) : le processus est fini, une borne
    // dépassée ici n'a plus rien à arrêter mais reste un échec.
    while open {
        match tokio::time::timeout(DRAIN_WAIT, lines.recv()).await {
            Ok(Some(line)) => guard(Seen::Line(&line))?,
            _ => open = false,
        }
    }
    let stderr = tokio::time::timeout(DRAIN_WAIT, stderr)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    Ok(Ended {
        success: status.is_some_and(|status| status.success()),
        stderr,
        exit: crate::process::describe_exit(status),
    })
}

/// Lit `pipe` ligne à ligne dans un fil à part (lignes trop longues
/// coupées, UTF-8 abîmé remplacé).
fn read_lines(pipe: impl Read + Send + 'static, sender: mpsc::UnboundedSender<String>) {
    let spawned = std::thread::Builder::new()
        .name("chatonlm-ytdlp-out".into())
        .spawn(move || {
            let mut reader = BufReader::new(pipe);
            let mut buffer = Vec::new();
            loop {
                buffer.clear();
                match (&mut reader)
                    .take(LINE_LIMIT as u64)
                    .read_until(b'\n', &mut buffer)
                {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {
                        if sender
                            .send(String::from_utf8_lossy(&buffer).into_owned())
                            .is_err()
                        {
                            // Personne n'écoute plus : le tube est vidé
                            // jusqu'à sa fermeture (yt-dlp ne bloque pas).
                            let _ = std::io::copy(&mut reader, &mut std::io::sink());
                            return;
                        }
                    }
                }
            }
        });
    if let Err(error) = spawned {
        // i18n-ignore : journal.
        crate::app_log!("chatonlm: cannot read yt-dlp's output: {error}");
    }
}

/// Fin de la sortie d'erreur ([`STDERR_LIMIT`] octets), lue dans un fil à
/// part.
fn read_tail(pipe: Option<impl Read + Send + 'static>) -> oneshot::Receiver<String> {
    let (sender, receiver) = oneshot::channel();
    let Some(mut pipe) = pipe else {
        let _ = sender.send(String::new());
        return receiver;
    };
    let spawned = std::thread::Builder::new()
        .name("chatonlm-ytdlp-err".into())
        .spawn(move || {
            let mut kept: Vec<u8> = Vec::new();
            let mut buffer = [0u8; 8192];
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        kept.extend_from_slice(&buffer[..read]);
                        if kept.len() > STDERR_LIMIT * 2 {
                            kept.drain(..kept.len() - STDERR_LIMIT);
                        }
                    }
                }
            }
            if kept.len() > STDERR_LIMIT {
                kept.drain(..kept.len() - STDERR_LIMIT);
            }
            let _ = sender.send(String::from_utf8_lossy(&kept).into_owned());
        });
    if let Err(error) = spawned {
        // i18n-ignore : journal.
        crate::app_log!("chatonlm: cannot read yt-dlp's errors: {error}");
    }
    receiver
}

/// Se résout quand l'annulation est demandée (jamais si l'émetteur a
/// disparu sans la demander).
async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow_and_update() {
            return;
        }
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Efface un dossier, patiemment (Windows : fichier encore ouvert juste
/// après l'arrêt du Job) ; absent : fait.
async fn remove_patiently(dir: &Path) -> std::io::Result<()> {
    let dir = dir.to_owned();
    tokio::task::spawn_blocking(move || crate::tools::install::remove_dir_patiently(&dir))
        .await
        .unwrap_or_else(|error| Err(std::io::Error::other(error)))
}

/// Lignes `ERROR:` de la sortie d'erreur, s'il y en a.
fn error_lines(stderr: &str) -> Option<String> {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("ERROR:"))
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn tail(text: &str, chars: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(chars)).collect()
}

/// Motifs des messages de yt-dlp (en minuscules, apostrophe droite), dans
/// l'ordre où ils sont essayés : un message de YouTube peut en contenir
/// plusieurs (« Video unavailable. This video is private »).
const PATTERNS: &[(FetchErrorCode, &[&str])] = &[
    (
        FetchErrorCode::Private,
        &["private video", "this video is private"],
    ),
    (
        FetchErrorCode::AgeRestricted,
        &[
            "confirm your age",
            "age-restricted",
            "age restricted",
            "inappropriate for some users",
        ],
    ),
    (
        FetchErrorCode::BotCheck,
        &[
            "not a bot",
            "captcha",
            "rate-limited",
            "try again later",
            "too many requests",
            "http error 429",
        ],
    ),
    (
        FetchErrorCode::MembersOnly,
        &[
            "members-only",
            "members only",
            "join this channel",
            "premium members",
            "requires payment",
            "requires purchase",
        ],
    ),
    (
        FetchErrorCode::GeoBlocked,
        &[
            "in your country",
            "from your location",
            "geo restriction",
            "geo-restricted",
            "geo restricted",
        ],
    ),
    (
        FetchErrorCode::Live,
        &[
            "live event will begin",
            "this live event",
            "premieres in",
            "premiere will begin",
            "is a live stream",
            "is_live",
        ],
    ),
    (FetchErrorCode::InvalidAddress, &["is not a valid url"]),
    (
        FetchErrorCode::Unsupported,
        &["unsupported url", "drm protected"],
    ),
    (
        FetchErrorCode::Unavailable,
        &[
            "video unavailable",
            "this video is unavailable",
            "video is no longer available",
            "has been removed",
            "has been terminated",
            "does not exist",
            "http error 404",
            "http error 410",
        ],
    ),
    (
        FetchErrorCode::Outdated,
        &[
            "challenge solving failed",
            "no supported javascript runtime",
            "requested format is not available",
            "unable to extract",
            "signature extraction failed",
            "nsig extraction failed",
            "http error 403",
            "confirm you are on the latest version",
        ],
    ),
    (
        FetchErrorCode::Network,
        &[
            "unable to download webpage",
            "unable to download api page",
            "unable to download video data",
            "failed to resolve",
            "name or service not known",
            "temporary failure in name resolution",
            "getaddrinfo failed",
            "nodename nor servname",
            "timed out",
            "connection refused",
            "connection reset",
            "connection aborted",
            "network is unreachable",
            "remote end closed",
            "http error 5",
            "ssl:",
        ],
    ),
];

/// Avertissements qui signalent un outil périmé quand l'erreur, elle, n'est
/// pas reconnue (« No video formats found! » après un défi non résolu).
const OUTDATED_WARNINGS: &[&str] = &[
    "challenge solving failed",
    "no supported javascript runtime",
    "some formats may be missing",
];

/// Nature d'un échec d'après la sortie d'erreur de yt-dlp : ses lignes
/// `ERROR:` d'abord (sinon tout le texte), puis les avertissements.
pub(crate) fn classify(stderr: &str) -> FetchErrorCode {
    let normalize = |text: &str| text.to_lowercase().replace(['\u{2019}', '\u{2018}'], "'");
    let all = normalize(stderr);
    let errors = error_lines(stderr).map_or_else(|| all.clone(), |lines| normalize(&lines));
    for (code, patterns) in PATTERNS {
        if patterns.iter().any(|pattern| errors.contains(pattern)) {
            return *code;
        }
    }
    if OUTDATED_WARNINGS
        .iter()
        .any(|pattern| all.contains(pattern))
    {
        return FetchErrorCode::Outdated;
    }
    FetchErrorCode::Failed
}

#[cfg(test)]
mod tests;
