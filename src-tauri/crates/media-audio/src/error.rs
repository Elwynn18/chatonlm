//! Erreurs du décodage : un code stable (pour le frontend, lot 2 de l'outil
//! Transcription) et, à part, le détail technique en anglais pour le journal.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Pourquoi un fichier n'a pas été décodé.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// Fichier impossible à ouvrir ou à lire (absent, droits, erreur du
    /// disque).
    CannotOpen,
    /// Conteneur non pris en charge (AVI, MPEG-TS, ASF/WMA, FLV…) ou inconnu.
    UnsupportedContainer,
    /// Piste audio dans un codec non pris en charge (AC-3, DTS, WMA…).
    UnsupportedCodec,
    /// Conteneur lu, mais sans piste audio (vidéo seule).
    NoAudioTrack,
    /// Conteneur reconnu, mais abîmé ou tronqué au point de n'en tirer aucun
    /// son.
    Damaged,
    /// Plus long que la durée permise (annoncée ou lue).
    TooLong,
    /// Plus gros que la taille permise.
    TooLarge,
    /// Annulé par l'appelant.
    Cancelled,
    /// Le récepteur des blocs a échoué (écriture du fichier de travail).
    Output,
}

/// Conteneur reconnu (ou refusé) d'un fichier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Container {
    /// MP4, M4A, MOV (fragmenté compris).
    Mp4,
    /// MKV ou WebM.
    Matroska,
    Ogg,
    Wav,
    Aiff,
    Caf,
    /// MP3 (ou MP1, MP2) sans conteneur.
    Mpeg,
    /// FLAC natif.
    Flac,
    /// AAC sans conteneur (ADTS).
    Adts,
    // Refusés.
    Avi,
    MpegTs,
    MpegPs,
    Asf,
    Flv,
    RealMedia,
    Unknown,
}

/// Codec de la piste audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Codec {
    Aac,
    /// HE-AAC (SBR) : seul le cœur AAC-LC est décodé (voir `heaac.rs`).
    HeAac,
    /// HE-AACv2 (SBR + PS) : cœur mono décodé.
    HeAacV2,
    Mp3,
    Mp2,
    Mp1,
    Flac,
    Vorbis,
    Opus,
    Pcm,
    Adpcm,
    Alac,
    // Refusés.
    Ac3,
    Eac3,
    Dts,
    TrueHd,
    Wma,
    Other,
}

/// Échec du décodage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError {
    pub code: ErrorCode,
    /// Conteneur, s'il a été reconnu.
    pub container: Option<Container>,
    /// Codec de la piste audio, s'il est connu (celui qui manque, pour
    /// [`ErrorCode::UnsupportedCodec`]).
    pub codec: Option<Codec>,
    /// Détail technique en anglais, pour le journal : jamais montré tel quel.
    pub detail: String,
}

impl DecodeError {
    pub(crate) fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            container: None,
            codec: None,
            detail: detail.into(),
        }
    }

    pub(crate) fn in_container(mut self, container: Container) -> Self {
        self.container = Some(container);
        self
    }

    pub(crate) fn with_codec(mut self, codec: Codec) -> Self {
        self.codec = Some(codec);
        self
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.code)?;
        if let Some(container) = self.container {
            write!(f, " ({container:?}")?;
            if let Some(codec) = self.codec {
                write!(f, ", {codec:?}")?;
            }
            write!(f, ")")?;
        } else if let Some(codec) = self.codec {
            write!(f, " ({codec:?})")?;
        }
        if !self.detail.is_empty() {
            write!(f, ": {}", self.detail)?;
        }
        Ok(())
    }
}

impl std::error::Error for DecodeError {}
