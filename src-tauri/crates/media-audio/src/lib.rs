//! Décodage des fichiers audio et vidéo de ChatonLM vers le son qu'attendent
//! les moteurs de la dictée (16 kHz, mono, f32 entre -1 et 1), pour l'outil
//! Transcription. Sans Tauri, en Rust pur : même code sous Linux et Windows.
//!
//! - Conteneurs : MP4/M4A/MOV (fragmenté compris), MKV/WebM, OGG, WAV, AIFF,
//!   CAF, MP3, FLAC, AAC (ADTS). Refusés et nommés : AVI, MPEG-TS, MPEG-PS,
//!   ASF/WMA, FLV, RealMedia.
//! - Codecs : AAC-LC, HE-AAC et HE-AACv2 (cœur AAC-LC, voir `heaac.rs`),
//!   MP3, FLAC, Vorbis, Opus (opus-pure), PCM, ADPCM, ALAC. Refusés : AC-3,
//!   E-AC-3, DTS, TrueHD, WMA, AAC de plus de deux canaux.
//! - Le son est rendu par blocs à un [`Sink`] au fil du décodage : la mémoire
//!   reste de quelques Mo quelle que soit la durée. Annulation entre deux
//!   paquets, bornes de durée et de taille, progression, codes d'erreur
//!   stables ([`ErrorCode`]). Une panique d'un décodeur devient un fichier
//!   abîmé ([`ErrorCode::Damaged`]).
//!
//! Choix et mesures : `.tools/research/105-transcription-research.md` § 3,
//! rapport du lot : `.tools/research/106-lot1-rapport.md`.

mod decode;
mod error;
mod heaac;
mod opus;
mod probe;
pub mod resample;

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use symphonia::core::io::MediaSource;

pub use error::{Codec, Container, DecodeError, ErrorCode};
pub use resample::{Resampler, resample};

/// Fréquence du son rendu : celle des deux moteurs de la dictée.
pub const RATE: u32 = 16_000;

/// Bornes d'un décodage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Durée au-delà de laquelle le fichier est refusé : dès l'ouverture si
    /// la durée annoncée la dépasse, sinon dès que le son lu la dépasse.
    pub max_duration: Duration,
    /// Taille de fichier au-delà de laquelle il est refusé avant lecture.
    pub max_bytes: u64,
}

impl Default for Limits {
    /// 6 h (rapport 105, § 4.7) et 20 Gio (une vidéo de plusieurs heures :
    /// toute la piste vidéo est lue et sautée).
    fn default() -> Self {
        Self {
            max_duration: Duration::from_secs(6 * 3600),
            max_bytes: 20 << 30,
        }
    }
}

/// Réglages d'un décodage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub limits: Limits,
    /// Fils du rééchantillonnage au plus ; 0 : autant que de cœurs.
    pub threads: usize,
}

impl Options {
    fn thread_count(&self) -> usize {
        match self.threads {
            0 => std::thread::available_parallelism().map_or(1, usize::from),
            threads => threads,
        }
    }
}

/// Ce que dit le fichier de sa piste audio, avant décodage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub container: Container,
    pub codec: Codec,
    /// Durée annoncée par le conteneur (absente d'un flux en direct
    /// enregistré, ou d'un MP3 sans en-tête de durée).
    pub duration: Option<Duration>,
    /// Fréquence et canaux annoncés de la piste (avant le passage en mono à
    /// 16 kHz ; fréquence du cœur pour un HE-AAC).
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
}

/// Avancement du décodage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    /// Durée du son décodé jusqu'ici.
    pub decoded: Duration,
    /// Durée annoncée du fichier, si elle est connue.
    pub total: Option<Duration>,
}

impl Progress {
    /// Fraction faite, entre 0 et 1, si la durée est connue.
    pub fn fraction(&self) -> Option<f64> {
        let total = self.total?.as_secs_f64();
        (total > 0.0).then(|| (self.decoded.as_secs_f64() / total).clamp(0.0, 1.0))
    }
}

/// Récepteur du son décodé (le fichier de travail de la transcription).
pub trait Sink {
    /// Appelé une fois, avant le premier bloc.
    fn start(&mut self, _info: &MediaInfo) -> io::Result<()> {
        Ok(())
    }

    /// Un bloc à 16 kHz, mono, dans l'ordre du son. Une erreur arrête le
    /// décodage ([`ErrorCode::Output`]).
    fn samples(&mut self, block: &[f32]) -> io::Result<()>;

    /// Après chaque bloc (au plus une fois par seconde de son décodé et par
    /// fil de rééchantillonnage).
    fn progress(&mut self, _progress: Progress) {}
}

/// Tout le son en mémoire : pour les sons courts et les essais.
impl Sink for Vec<f32> {
    fn samples(&mut self, block: &[f32]) -> io::Result<()> {
        self.extend_from_slice(block);
        Ok(())
    }
}

/// Bilan d'un décodage réussi.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub info: MediaInfo,
    /// Échantillons rendus, à 16 kHz.
    pub samples: u64,
    /// Faux si la lecture s'est arrêtée avant la fin annoncée du flux
    /// (fichier tronqué, ou abîmé en cours de route) : ce qui précède a été
    /// rendu.
    pub complete: bool,
    /// Paquets illisibles sautés.
    pub skipped_packets: u64,
}

impl Summary {
    /// Durée du son rendu.
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.samples as f64 / f64::from(RATE))
    }
}

/// Lit l'en-tête d'un fichier sans décoder le son : conteneur, codec, durée.
/// Mêmes refus que [`decode_file`] avant le premier paquet (format, codec,
/// piste absente, bornes).
pub fn probe_file(path: &Path, limits: &Limits) -> Result<MediaInfo, DecodeError> {
    let mut reopen = reopen(path, open(path, limits)?);
    decode::guarded(|| {
        let opened = decode::open(&mut reopen, extension(path).as_deref())?;
        opened.check_duration(limits)?;
        Ok(opened.info)
    })
}

/// Décode la piste audio d'un fichier vers `sink`, en blocs de 16 kHz mono.
/// Bloquant (à lancer hors de la boucle asynchrone) ; `cancel` est lu entre
/// deux paquets.
pub fn decode_file(
    path: &Path,
    options: &Options,
    cancel: &AtomicBool,
    sink: &mut dyn Sink,
) -> Result<Summary, DecodeError> {
    let mut reopen = reopen(path, open(path, &options.limits)?);
    decode::guarded(|| {
        decode::run(
            &mut reopen,
            extension(path).as_deref(),
            options,
            cancel,
            sink,
        )
    })
}

/// Comme [`decode_file`], sur un fichier déjà en mémoire ; `extension` (sans
/// point) aide à reconnaître un format sans signature (MP3, ADTS).
pub fn decode_bytes(
    data: Vec<u8>,
    extension: Option<&str>,
    options: &Options,
    cancel: &AtomicBool,
    sink: &mut dyn Sink,
) -> Result<Summary, DecodeError> {
    if data.len() as u64 > options.limits.max_bytes {
        return Err(too_large(data.len() as u64, &options.limits));
    }
    let data: Arc<[u8]> = data.into();
    let mut reopen =
        || -> io::Result<Box<dyn MediaSource>> { Ok(Box::new(io::Cursor::new(data.clone()))) };
    decode::guarded(|| decode::run(&mut reopen, extension, options, cancel, sink))
}

fn open(path: &Path, limits: &Limits) -> Result<File, DecodeError> {
    let cannot_open = |error: io::Error| DecodeError::new(ErrorCode::CannotOpen, error.to_string());
    let file = File::open(path).map_err(cannot_open)?;
    let metadata = file.metadata().map_err(cannot_open)?;
    if !metadata.is_file() {
        return Err(DecodeError::new(ErrorCode::CannotOpen, "not a file"));
    }
    if metadata.len() > limits.max_bytes {
        return Err(too_large(metadata.len(), limits));
    }
    Ok(file)
}

/// Le fichier déjà ouvert, puis le même rouvert à chaque appel suivant.
fn reopen(path: &Path, file: File) -> impl FnMut() -> io::Result<Box<dyn MediaSource>> + '_ {
    let mut first = Some(file);
    move || {
        let file = match first.take() {
            Some(file) => file,
            None => File::open(path)?,
        };
        Ok(Box::new(file))
    }
}

fn too_large(size: u64, limits: &Limits) -> DecodeError {
    DecodeError::new(
        ErrorCode::TooLarge,
        format!("{size} bytes, more than {}", limits.max_bytes),
    )
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
}
