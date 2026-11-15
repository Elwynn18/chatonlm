//! Outil Transcription (propre à ChatonLM) : transcription complète d'un
//! fichier audio ou vidéo local, ou d'une adresse de vidéo en ligne (audio
//! récupéré par yt-dlp, [`ytdlp`]), par les moteurs de la dictée (Parakeet,
//! Whisper), sur l'ordinateur.
//!
//! Une seule transcription à la fois ([`Transcription::start`] refuse
//! pendant qu'une autre tourne) : les moteurs n'ont qu'un emplacement de
//! modèle, partagé avec la dictée et le mode vocal, et deux transcriptions
//! longues se partageraient le processeur sans finir plus tôt. La page le
//! sait par l'état (`transcription_status`) et n'a pas de file à montrer.
//!
//! Étapes ([`job`]) : préparation (modèle, machine), récupération de l'audio
//! d'une adresse (fiche, téléchargement), décodage vers un fichier de travail
//! à 16 kHz avec relevé de la parole (Silero), découpage dans les pauses
//! ([`segment`]), transcription morceau par morceau (la dictée et le mode
//! vocal passent entre deux morceaux), passages horodatés enregistrés au fil
//! de l'eau. L'audio de travail (`<données>/transcription/work/<id>/`) est
//! effacé à la fin, réussite ou échec, et au lancement suivant si l'app a
//! été coupée. Chaque changement part vers la page par l'événement
//! [`STATUS_EVENT`] (4 fois par seconde au plus pour la seule progression ;
//! tout nouveau passage, tout changement d'étape aussitôt).
//!
//! Seul un fichier remis par le moteur (sélecteur natif
//! `transcription_pick_file`, dépôt natif sur la fenêtre) peut être lu, sur
//! le modèle des pièces jointes (`attachments::Staging::grant`) ; les
//! chemins remis sont oubliés au rechargement de la page.

pub mod job;
mod language;
pub mod paragraphs;
pub mod segment;
/// « Synthétiser » : document de lecture tiré d'une transcription (lot 6).
pub mod synthesis;
#[cfg(test)]
mod tests;
pub mod ytdlp;

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use ts_rs::TS;

use crate::dictation::models::{self, CatalogModel, SpeechEngine};
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::Store;
use crate::voice_mode::audio::vad::SpeechDetector;
use paragraphs::Segment;
use ytdlp::{FetchError, FetchErrorCode, FetchProgress, FetchedAudio};

/// Événement envoyé à la page à chaque changement de [`TranscriptionStatus`].
pub const STATUS_EVENT: &str = "transcription-status";

/// Écart minimal entre deux événements de simple progression.
const EMIT_GAP: Duration = Duration::from_millis(250);

/// Chemins remis gardés au plus (les plus anciens oubliés).
const MAX_GRANTED: usize = 10_000;

/// Attente maximale de la fin d'une transcription à la fermeture de l'app.
pub const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

/// D'où vient le son.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "TranscriptionSourceKind")]
pub enum SourceKind {
    File,
    Url,
}

/// Source demandée par la page.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export, rename = "TranscriptionSource")]
pub enum Source {
    /// Chemin remis par le sélecteur ou un dépôt natif.
    File { path: String },
    /// Adresse d'une vidéo (yt-dlp).
    Url { url: String },
}

impl Source {
    pub fn kind(&self) -> SourceKind {
        match self {
            Source::File { .. } => SourceKind::File,
            Source::Url { .. } => SourceKind::Url,
        }
    }

    /// Chemin ou adresse, tel que donné.
    pub fn origin(&self) -> &str {
        match self {
            Source::File { path } => path,
            Source::Url { url } => url.trim(),
        }
    }
}

/// Demande de transcription.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct TranscriptionRequest {
    pub source: Source,
    /// Modèle du catalogue de la dictée ; absent : celui de la dictée, sinon
    /// le premier installé.
    #[ts(optional)]
    pub model: Option<String>,
    /// Code de langue, ou « auto » ; absent : celle de la dictée. Parakeet
    /// détecte toujours seul ; Whisper sans langue la détecte à chaque
    /// morceau, jusqu'à ce que deux morceaux parlés donnent la même langue
    /// sûre (`language::sure`), gardée ensuite.
    #[ts(optional)]
    pub language: Option<String>,
}

/// État enregistré d'une transcription.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TranscriptionState {
    Running,
    Done,
    Failed,
    Cancelled,
    /// App fermée pendant la transcription : ce qui était fait est gardé.
    Interrupted,
}

/// Nature d'un échec (texte : [`TranscriptionError::message`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TranscriptionErrorCode {
    /// Aucun modèle de transcription installé (la page propose le ModelHub).
    NoModel,
    /// Le modèle demandé n'est pas (ou plus) installé.
    ModelNotInstalled,
    /// Ce moteur ne peut pas tourner sur cette machine (Whisper sans AVX2,
    /// Parakeet sans ONNX Runtime).
    Unsupported,
    CannotOpen,
    /// Conteneur non pris en charge (AVI, MPEG-TS, WMA…) ou fichier inconnu.
    UnsupportedFormat,
    /// Son dans un codec non pris en charge (AC-3, DTS…).
    UnsupportedCodec,
    NoAudioTrack,
    Damaged,
    TooLong,
    TooLarge,
    /// Échec de yt-dlp (détail : `fetch`).
    Fetch,
    NoSpeech,
    /// Échec du moteur de transcription.
    Failed,
    /// Écriture de l'audio de travail impossible (disque plein…).
    Io,
}

impl TranscriptionErrorCode {
    pub const ALL: [Self; 14] = [
        Self::NoModel,
        Self::ModelNotInstalled,
        Self::Unsupported,
        Self::CannotOpen,
        Self::UnsupportedFormat,
        Self::UnsupportedCodec,
        Self::NoAudioTrack,
        Self::Damaged,
        Self::TooLong,
        Self::TooLarge,
        Self::Fetch,
        Self::NoSpeech,
        Self::Failed,
        Self::Io,
    ];

    /// Nom enregistré en base (celui de la sérialisation).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoModel => "noModel",
            Self::ModelNotInstalled => "modelNotInstalled",
            Self::Unsupported => "unsupported",
            Self::CannotOpen => "cannotOpen",
            Self::UnsupportedFormat => "unsupportedFormat",
            Self::UnsupportedCodec => "unsupportedCodec",
            Self::NoAudioTrack => "noAudioTrack",
            Self::Damaged => "damaged",
            Self::TooLong => "tooLong",
            Self::TooLarge => "tooLarge",
            Self::Fetch => "fetch",
            Self::NoSpeech => "noSpeech",
            Self::Failed => "failed",
            Self::Io => "io",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|code| code.as_str() == text)
    }
}

/// Échec : code, détail de yt-dlp, texte dans la langue du moment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptionError {
    pub code: TranscriptionErrorCode,
    /// Nature de l'échec de yt-dlp (`code` = `fetch`).
    pub fetch: Option<FetchErrorCode>,
    pub message: String,
}

impl TranscriptionError {
    /// Échec de `code`, texte générique.
    pub fn of(code: TranscriptionErrorCode) -> Self {
        Self {
            code,
            fetch: None,
            message: message(code),
        }
    }

    pub fn with_message(code: TranscriptionErrorCode, message: String) -> Self {
        Self {
            code,
            fetch: None,
            message,
        }
    }

    pub fn fetch(error: &FetchError) -> Self {
        Self {
            code: TranscriptionErrorCode::Fetch,
            fetch: Some(error.code),
            message: ytdlp::message(error.code),
        }
    }
}

/// Avertissement d'une transcription finie (texte :
/// [`TranscriptionWarning::message`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TranscriptionWarningCode {
    /// Lecture du fichier arrêtée avant sa fin (abîmé, tronqué, OGG
    /// enchaîné…) : seul le début est transcrit.
    Partial,
}

impl TranscriptionWarningCode {
    pub const ALL: [Self; 1] = [Self::Partial];

    /// Nom enregistré en base (celui de la sérialisation).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Partial => "partial",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|code| code.as_str() == text)
    }
}

/// Avertissement : code, texte dans la langue du moment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptionWarning {
    pub code: TranscriptionWarningCode,
    pub message: String,
}

impl TranscriptionWarning {
    pub fn of(code: TranscriptionWarningCode) -> Self {
        Self {
            code,
            message: match code {
                TranscriptionWarningCode::Partial => t!("transcription.partial"),
            },
        }
    }
}

/// Formats lus, pour les messages (noms techniques, non traduits).
const SUPPORTED_FORMATS: &str = "MP3, M4A/AAC, MP4/MOV, MKV/WebM, OGG/Opus/Vorbis, FLAC, WAV, AIFF";

/// Texte générique d'un code, dans la langue du moment.
pub fn message(code: TranscriptionErrorCode) -> String {
    use TranscriptionErrorCode as C;
    match code {
        C::NoModel => t!("transcription.noModel"),
        C::ModelNotInstalled => t!("transcription.modelNotInstalled"),
        C::Unsupported => t!("transcription.unsupported"),
        C::CannotOpen => t!("transcription.cannotOpen"),
        C::UnsupportedFormat => t!("transcription.unknownFormat", formats = SUPPORTED_FORMATS),
        C::UnsupportedCodec => t!("transcription.unknownCodec"),
        C::NoAudioTrack => t!("transcription.noAudioTrack"),
        C::Damaged => t!("transcription.damaged"),
        C::TooLong => t!("transcription.tooLong", hours = job::MAX_HOURS),
        C::TooLarge => t!("transcription.tooLarge", gigabytes = job::MAX_GIGABYTES),
        C::Fetch => t!("transcription.fetchFailed"),
        C::NoSpeech => t!("transcription.noSpeech"),
        C::Failed => t!("transcription.stopped"),
        C::Io => t!("transcription.ioFailed"),
    }
}

/// Étape de la transcription en cours (ou de la dernière).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "TranscriptionStage")]
pub enum Stage {
    /// Modèle et machine vérifiés.
    Preparing,
    /// Fiche de la vidéo (yt-dlp, premier temps).
    Fetching,
    /// Audio de la vidéo (yt-dlp, deuxième temps).
    Downloading,
    /// Décodage vers 16 kHz et relevé de la parole.
    Decoding,
    Transcribing,
    Done,
    Failed,
    Cancelled,
}

impl Stage {
    #[cfg(test)]
    pub fn is_final(self) -> bool {
        matches!(self, Stage::Done | Stage::Failed | Stage::Cancelled)
    }
}

/// Transcription en cours, ou la dernière finie depuis le lancement.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptionJob {
    /// Identifiant de l'historique (`transcription_get`).
    pub id: String,
    pub source: SourceKind,
    /// Chemin ou adresse donnés.
    pub origin: String,
    pub stage: Stage,
    /// Modèle choisi (vide tant qu'il ne l'est pas).
    pub model: String,
    pub engine: Option<SpeechEngine>,
    /// Langue demandée, ou détectée (Whisper) ; absente : inconnue.
    pub language: Option<String>,
    /// Titre de la vidéo, ou nom du fichier.
    pub title: Option<String>,
    pub channel: Option<String>,
    /// Adresse canonique et site (yt-dlp).
    pub url: Option<String>,
    pub site: Option<String>,
    /// Durée annoncée (fiche, fichier), puis celle du son décodé.
    pub duration_s: Option<f64>,
    /// Téléchargement : octets reçus et annoncés.
    pub received_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    /// Décodage : son lu ; transcription : son traité (morceaux muets
    /// compris), en secondes.
    pub done_s: f64,
    /// Vitesse de la transcription mesurée (secondes de son par seconde),
    /// pour une estimation du temps restant.
    pub speed: Option<f64>,
    /// Passages à partir de l'indice `segments_from` : tous dans
    /// `transcription_status`, seulement les nouveaux dans un événement (la
    /// page les ajoute si `segments_from` vaut le nombre qu'elle a déjà,
    /// sinon relit l'état).
    pub segments: Vec<Segment>,
    pub segments_from: u32,
    pub error: Option<TranscriptionError>,
    /// Transcription finie, mais incomplète (voir le code).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub warning: Option<TranscriptionWarning>,
}

impl TranscriptionJob {
    fn new(id: String, source: &Source) -> Self {
        Self {
            id,
            source: source.kind(),
            origin: source.origin().to_owned(),
            stage: Stage::Preparing,
            model: String::new(),
            engine: None,
            language: None,
            title: match source {
                Source::File { path } => file_name(path),
                Source::Url { .. } => None,
            },
            channel: None,
            url: None,
            site: None,
            duration_s: None,
            received_bytes: None,
            total_bytes: None,
            done_s: 0.0,
            speed: None,
            segments: Vec::new(),
            segments_from: 0,
            error: None,
            warning: None,
        }
    }
}

/// Nom d'un fichier (titre par défaut).
pub fn file_name(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptionStatus {
    pub job: Option<TranscriptionJob>,
}

/// Transcription de l'historique (liste).
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptionItem {
    pub id: String,
    pub source: SourceKind,
    pub origin: String,
    pub title: String,
    pub channel: String,
    pub url: String,
    pub site: String,
    pub duration_s: Option<f64>,
    pub engine: SpeechEngine,
    pub model: String,
    pub language: Option<String>,
    pub state: TranscriptionState,
    pub error: Option<TranscriptionError>,
    /// Finie, mais incomplète (voir le code).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub warning: Option<TranscriptionWarning>,
    /// Début du texte (200 caractères au plus).
    pub preview: String,
    pub has_summary: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

/// Synthèse enregistrée (lot 6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "TranscriptionSummary")]
pub struct Synthesis {
    pub markdown: String,
    pub model: String,
    pub at: i64,
}

/// Transcription complète.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptionDetail {
    pub item: TranscriptionItem,
    pub segments: Vec<Segment>,
    /// Paragraphes sans horodatage.
    pub text: String,
    pub summary: Option<Synthesis>,
}

/// Format d'un texte exporté.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "TranscriptionExportFormat")]
pub enum ExportFormat {
    Txt,
    Md,
    Srt,
}

/// Modèle de transcription proposé.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptionModel {
    pub id: String,
    pub name: String,
    pub engine: SpeechEngine,
    pub installed: bool,
    /// Langues, en clair (catalogue).
    pub languages: String,
}

/// Ce que la page propose avant de lancer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptionOptions {
    /// Tout le catalogue, installés ou non (la page propose le ModelHub
    /// pour les autres).
    pub models: Vec<TranscriptionModel>,
    /// Modèle choisi d'office : celui de la dictée s'il est installé, sinon
    /// le premier installé ; absent : aucun n'est installé.
    pub default_model: Option<String>,
    /// Langue de la dictée (code, ou « auto »).
    pub language: String,
}

/// Fiche d'un fichier local, avant de lancer.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptionProbe {
    pub name: String,
    /// Durée annoncée par le fichier (absente de certains formats).
    pub duration_s: Option<f64>,
    /// Fichier illisible ou refusé d'avance.
    pub error: Option<TranscriptionError>,
}

/// Réglages de la dictée et modèles présents, lus pour chaque demande.
pub struct Prepared {
    /// Modèle de la dictée (`speechModel`).
    pub speech_model: String,
    /// Langue de la dictée (code ou « auto »).
    pub language: String,
    /// Dictionnaire de l'utilisateur (consigne de Whisper).
    pub dictionary: Vec<String>,
    /// Dossier des modèles de transcription.
    pub models_dir: PathBuf,
    /// Modèle complet sur le disque (`models::installed` ; faux dans les
    /// essais).
    pub installed: Box<dyn Fn(&CatalogModel) -> bool + Send + Sync>,
}

impl Prepared {
    /// Modèle d'office (voir [`TranscriptionOptions::default_model`]).
    pub fn default_model(&self) -> Option<&'static CatalogModel> {
        models::find(&self.speech_model)
            .filter(|model| (self.installed)(model))
            .or_else(|| models::CATALOG.iter().find(|model| (self.installed)(model)))
    }

    pub fn options(&self) -> TranscriptionOptions {
        TranscriptionOptions {
            models: models::CATALOG
                .iter()
                .map(|model| TranscriptionModel {
                    id: model.id.into(),
                    name: model.name.into(),
                    engine: model.engine,
                    installed: (self.installed)(model),
                    languages: model.languages.into(),
                })
                .collect(),
            default_model: self.default_model().map(|model| model.id.to_owned()),
            language: self.language.clone(),
        }
    }
}

/// Récupération de l'audio d'une adresse (yt-dlp, ou un faux dans les
/// essais).
pub trait Fetcher: Send + Sync {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        work_dir: &'a Path,
        max_duration_s: Option<f64>,
        cancel: watch::Receiver<bool>,
        progress: &'a (dyn Fn(FetchProgress) + Send + Sync),
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchedAudio, FetchError>> + Send + 'a>,
    >;
}

/// yt-dlp, par les outils installés de l'app.
pub struct AppFetcher(pub tauri::AppHandle);

impl Fetcher for AppFetcher {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        work_dir: &'a Path,
        max_duration_s: Option<f64>,
        cancel: watch::Receiver<bool>,
        progress: &'a (dyn Fn(FetchProgress) + Send + Sync),
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchedAudio, FetchError>> + Send + 'a>,
    > {
        use tauri::Manager;
        Box::pin(async move {
            let Some(tools) = self.0.try_state::<crate::tools::Tools>() else {
                return Err(FetchError {
                    code: FetchErrorCode::ToolsMissing,
                    detail: "no tools state".into(),
                });
            };
            ytdlp::fetch_audio(
                tools.inner(),
                url,
                work_dir,
                max_duration_s,
                cancel,
                progress,
            )
            .await
        })
    }
}

/// Fabrique du détecteur de parole (Silero, ou un faux).
pub type DetectorFactory = dyn Fn() -> Result<Box<dyn SpeechDetector + Send>, String> + Send + Sync;

/// Silero VAD, sur l'ONNX Runtime de la dictée.
pub fn silero() -> Result<Box<dyn SpeechDetector + Send>, String> {
    crate::voice_mode::audio::vad::Silero::load()
        .map(|silero| Box::new(silero) as Box<dyn SpeechDetector + Send>)
}

/// Moteur, détection, récupération et événement (faux dans les essais).
pub struct Services {
    pub engine: crate::dictation::engine::Engine,
    pub detector: Box<DetectorFactory>,
    pub fetcher: Box<dyn Fetcher>,
    /// Refus d'un moteur que cette machine ne peut pas faire tourner.
    pub support: fn(SpeechEngine) -> Result<(), String>,
    /// `<données>/transcription/work`.
    pub work_root: PathBuf,
    pub limits: media_audio::Limits,
    pub emit: Box<dyn Fn(&TranscriptionStatus) + Send + Sync>,
    pub emit_gap: Duration,
}

impl Services {
    pub fn new(
        engine: crate::dictation::engine::Engine,
        fetcher: Box<dyn Fetcher>,
        work_root: PathBuf,
        emit: impl Fn(&TranscriptionStatus) + Send + Sync + 'static,
    ) -> Self {
        Self {
            engine,
            detector: Box::new(silero),
            fetcher,
            support: crate::dictation::engine_support,
            work_root,
            limits: media_audio::Limits::default(),
            emit: Box::new(emit),
            emit_gap: EMIT_GAP,
        }
    }
}

/// État de l'outil (état géré de l'app, clonable).
#[derive(Clone)]
pub struct Transcription {
    shared: Arc<Shared>,
}

pub(crate) struct Shared {
    services: Services,
    inner: Mutex<Inner>,
    /// Détecteur gardé d'une transcription à l'autre (remis à zéro).
    detector: Mutex<Option<Box<dyn SpeechDetector + Send>>>,
}

/// Annulation vue par les étapes asynchrones (`watch`) et bloquantes
/// (drapeau lu entre deux blocs), avec sa raison : bouton, ou app qui ferme
/// (« interrompue », comme une app coupée).
pub(crate) struct Cancel {
    pub flag: Arc<AtomicBool>,
    pub sender: watch::Sender<bool>,
    closing: AtomicBool,
}

impl Cancel {
    fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            sender: watch::channel(false).0,
            closing: AtomicBool::new(false),
        }
    }

    pub fn raise(&self) {
        self.flag.store(true, Ordering::SeqCst);
        let _ = self.sender.send(true);
    }

    /// Fermeture de l'app : annulation dont la raison est notée d'abord.
    pub fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
        self.raise();
    }

    pub fn is_raised(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Annulée parce que l'app ferme.
    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct Inner {
    job: Option<TranscriptionJob>,
    running: bool,
    cancel: Option<Arc<Cancel>>,
    task: Option<tauri::async_runtime::JoinHandle<()>>,
    granted: Granted,
    last_emit: Option<Instant>,
    /// Passages déjà envoyés à la page.
    sent: usize,
}

/// Chemins remis par le moteur, comparés tels quels (comme les pièces
/// jointes) ; un chemin reste valable jusqu'au rechargement de la page (une
/// même source peut être transcrite avec un autre modèle).
#[derive(Default)]
struct Granted {
    paths: HashSet<String>,
    order: VecDeque<String>,
}

impl Granted {
    fn grant(&mut self, path: String) {
        if self.paths.insert(path.clone()) {
            self.order.push_back(path);
        }
        while self.order.len() > MAX_GRANTED {
            if let Some(oldest) = self.order.pop_front() {
                self.paths.remove(&oldest);
            }
        }
    }
}

impl Transcription {
    pub fn new(services: Services) -> Self {
        Self {
            shared: Arc::new(Shared {
                services,
                inner: Mutex::new(Inner::default()),
                detector: Mutex::new(None),
            }),
        }
    }

    /// Remet des chemins venus du moteur (sélecteur, dépôt natif).
    pub fn grant(&self, paths: impl IntoIterator<Item = String>) {
        let mut inner = self.shared.inner.lock();
        for path in paths {
            inner.granted.grant(path);
        }
    }

    /// Page rechargée : ses chemins remis n'ont plus de destinataire.
    pub fn clear_grants(&self) {
        self.shared.inner.lock().granted = Granted::default();
    }

    fn is_granted(&self, path: &str) -> bool {
        self.shared.inner.lock().granted.paths.contains(path)
    }

    /// État complet (tous les passages).
    pub fn status(&self) -> TranscriptionStatus {
        let inner = self.shared.inner.lock();
        TranscriptionStatus {
            job: inner.job.clone().map(|mut job| {
                job.segments_from = 0;
                job
            }),
        }
    }

    /// Lance une transcription ; son identifiant. Refusée si une autre
    /// tourne, pour un fichier non remis, ou une adresse vide ; tout autre
    /// échec (modèle absent, fichier illisible, yt-dlp…) arrive par l'état.
    pub fn start(
        &self,
        store: Store,
        prepared: Prepared,
        request: TranscriptionRequest,
    ) -> AppResult<String> {
        if let Some(language) = &request.language
            && !crate::dictation::languages::is_supported(language)
        {
            // i18n-ignore : la page n'envoie que les codes du moteur.
            return Err(AppError::Invalid(format!("unknown language: {language}")));
        }
        match &request.source {
            Source::File { path } if !self.is_granted(path) => {
                return Err(AppError::Invalid(t!("transcription.notAllowed")));
            }
            Source::Url { url } if url.trim().is_empty() => {
                return Err(AppError::Invalid(t!("transcription.emptyAddress")));
            }
            _ => {}
        }
        let mut inner = self.shared.inner.lock();
        if inner.running {
            return Err(AppError::Invalid(t!("transcription.busy")));
        }
        let id = uuid::Uuid::now_v7().to_string();
        let cancel = Arc::new(Cancel::new());
        inner.job = Some(TranscriptionJob::new(id.clone(), &request.source));
        inner.running = true;
        inner.cancel = Some(cancel.clone());
        inner.sent = 0;
        inner.last_emit = None;
        let shared = self.shared.clone();
        let plan = job::Plan {
            id: id.clone(),
            request,
            prepared,
        };
        inner.task = Some(tauri::async_runtime::spawn(async move {
            job::run(shared, store, plan, cancel).await;
        }));
        let payload = self.shared.payload(&mut inner);
        (self.shared.services.emit)(&payload);
        Ok(id)
    }

    /// Le modèle de transcription `model` sert à la transcription en cours
    /// (sa suppression est refusée).
    pub fn uses_model(&self, model: &str) -> bool {
        let inner = self.shared.inner.lock();
        inner.running && inner.job.as_ref().is_some_and(|job| job.model == model)
    }

    /// `id` est la transcription en cours.
    pub fn is_running(&self, id: &str) -> bool {
        let inner = self.shared.inner.lock();
        inner.running && inner.job.as_ref().is_some_and(|job| job.id == id)
    }

    /// Annule la transcription en cours (sans effet sinon).
    pub fn cancel(&self) {
        if let Some(cancel) = self.shared.inner.lock().cancel.clone() {
            cancel.raise();
        }
    }

    /// Fiche d'un fichier remis (durée), sans le décoder.
    pub async fn probe(&self, path: String) -> AppResult<TranscriptionProbe> {
        if !self.is_granted(&path) {
            return Err(AppError::Invalid(t!("transcription.notAllowed")));
        }
        let limits = self.shared.services.limits;
        let name = file_name(&path).unwrap_or_else(|| path.clone());
        let probed = tauri::async_runtime::spawn_blocking(move || {
            media_audio::probe_file(Path::new(&path), &limits)
        })
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?;
        Ok(match probed {
            Ok(info) => TranscriptionProbe {
                name,
                duration_s: info.duration.map(|duration| duration.as_secs_f64()),
                error: None,
            },
            Err(error) => {
                TranscriptionProbe {
                    name,
                    duration_s: None,
                    error: Some(job::decode_error(&error).unwrap_or_else(|| {
                        TranscriptionError::of(TranscriptionErrorCode::CannotOpen)
                    })),
                }
            }
        })
    }

    /// Fermeture de l'app : transcription arrêtée (« interrompue », ce qui
    /// est fait gardé), attendue `limit` au plus (audio de travail effacé,
    /// état enregistré) ; un morceau encore dans le moteur (Whisper sur le
    /// processeur : des dizaines de secondes) est attendu jusqu'à sa fin,
    /// [`crate::dictation::engine::PIECE_EXIT_WAIT`] au plus, comme le fait
    /// `Engine::shutdown` avant de libérer le modèle.
    pub async fn shutdown(&self, limit: Duration) {
        let task = {
            let mut inner = self.shared.inner.lock();
            if let Some(cancel) = &inner.cancel {
                cancel.close();
            }
            inner.task.take()
        };
        let Some(mut task) = task else {
            return;
        };
        if tokio::time::timeout(limit, &mut task).await.is_ok() {
            return;
        }
        if self.shared.services.engine.piece_running() {
            let wait = crate::dictation::engine::PIECE_EXIT_WAIT.saturating_sub(limit);
            if tokio::time::timeout(wait, task).await.is_err() {
                crate::app_log!("chatonlm: transcription: still running at exit");
            }
        }
    }

    /// Lancement : audio de travail laissé par une app coupée effacé,
    /// transcriptions restées « en cours » marquées interrompues (ou
    /// retirées si rien n'était transcrit). Bloquant (démarrage).
    pub fn recover(&self, store: Option<&Store>) {
        let root = &self.shared.services.work_root;
        if root.exists()
            && let Err(error) = std::fs::remove_dir_all(root)
        {
            crate::app_log!("chatonlm: transcription: work folder not removed: {error}");
        }
        if let Some(store) = store
            && let Err(error) =
                store.blocking(|connection| crate::store::transcriptions::recover(connection))
        {
            crate::app_log!("chatonlm: transcription: recovery: {error}");
        }
    }

    #[cfg(test)]
    pub(crate) fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }
}

impl Shared {
    /// Change l'état de la transcription en cours ; envoyé aussitôt si
    /// `force`, sinon 4 fois par seconde au plus.
    pub(crate) fn update(&self, force: bool, change: impl FnOnce(&mut TranscriptionJob)) {
        let mut inner = self.inner.lock();
        let Some(job) = inner.job.as_mut() else {
            return;
        };
        change(job);
        let due = inner
            .last_emit
            .is_none_or(|last| last.elapsed() >= self.services.emit_gap);
        if force || due {
            let payload = self.payload(&mut inner);
            (self.services.emit)(&payload);
        }
    }

    /// Fin de la transcription : dernier état envoyé, une autre peut
    /// commencer.
    pub(crate) fn finish(&self, change: impl FnOnce(&mut TranscriptionJob)) {
        let mut inner = self.inner.lock();
        if let Some(job) = inner.job.as_mut() {
            change(job);
        }
        inner.running = false;
        inner.cancel = None;
        let payload = self.payload(&mut inner);
        (self.services.emit)(&payload);
    }

    /// État envoyé : les seuls passages nouveaux.
    fn payload(&self, inner: &mut Inner) -> TranscriptionStatus {
        inner.last_emit = Some(Instant::now());
        let sent = inner.sent;
        let job = inner.job.as_ref().map(|job| {
            let from = sent.min(job.segments.len());
            TranscriptionJob {
                segments: job.segments[from..].to_vec(),
                segments_from: from as u32,
                ..job.clone()
            }
        });
        if let Some(job) = &inner.job {
            inner.sent = job.segments.len();
        }
        TranscriptionStatus { job }
    }

    pub(crate) fn services(&self) -> &Services {
        &self.services
    }

    /// Détecteur gardé, ou un neuf ; `None` si la détection est impossible
    /// (ONNX Runtime absent…).
    pub(crate) fn take_detector(&self) -> Option<Box<dyn SpeechDetector + Send>> {
        if let Some(detector) = self.detector.lock().take() {
            return Some(detector);
        }
        match (self.services.detector)() {
            Ok(detector) => Some(detector),
            Err(error) => {
                crate::app_log!("chatonlm: transcription: no speech detection: {error}");
                None
            }
        }
    }

    pub(crate) fn keep_detector(&self, detector: Option<Box<dyn SpeechDetector + Send>>) {
        if detector.is_some() {
            *self.detector.lock() = detector;
        }
    }
}
