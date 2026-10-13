//! Lecture à voix haute des réponses et des textes des cours (fonction propre
//! à ChatonLM, absente de l'app officielle) : Pocket TTS de Kyutai exporté en
//! ONNX (rapport 57), prétraitement du texte par langue, sortie audio par
//! cpal.
//!
//! - `models` : catalogue des voix (une par langue), téléchargées à la demande
//!   par la file générique de `dictation::downloads` (dossier `voice-models`).
//! - crate `speech-text` : texte parlé tiré du Markdown, découpage
//!   incrémental pour la lecture automatique, langue d'une réponse,
//!   prétraitement (chiffres, unités, sigles… en toutes lettres).
//! - `engine` : synthèse (tokenizer, état de voix, boucle FlowLM, Mimi).
//! - `playback` : sortie audio en flux.
//! - `service` : une lecture à la fois, état envoyé à la page.

pub mod engine;
pub mod models;
pub mod playback;
pub mod prompt;
pub mod service;
pub mod text;
pub mod tokenizer;
pub mod voice;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use service::ReadAloud;

/// Événement de l'état de la lecture (`ReadAloudState`).
pub const READ_ALOUD_EVENT: &str = "read-aloud";
/// Événement de la file des voix (liste de `SpeechDownload`).
pub const VOICE_DOWNLOADS_EVENT: &str = "voice-downloads";
/// Variable qui remplace les hôtes des voix (essais, debug et e2e seulement).
pub const VOICE_MODELS_URL_ENV: &str = "CHATONLM_VOICE_MODELS_URL";

/// Langue d'une voix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum VoiceLanguage {
    En,
    Es,
    Fr,
}

impl VoiceLanguage {
    /// Toutes les langues, dans l'ordre du catalogue (essais).
    #[cfg(test)]
    pub const ALL: [VoiceLanguage; 3] = [VoiceLanguage::En, VoiceLanguage::Es, VoiceLanguage::Fr];

    pub fn code(self) -> &'static str {
        match self {
            VoiceLanguage::En => "en",
            VoiceLanguage::Es => "es",
            VoiceLanguage::Fr => "fr",
        }
    }
}

/// Ce qu'une lecture lit (une seule à la fois dans l'app) : l'état le nomme,
/// pour que seul son bouton montre « Stop ».
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum ReadAloudSource {
    /// Réponse enregistrée d'une conversation.
    Message { message_id: i64 },
    /// Texte d'un cours (onglet Study) : `passage` est la clé du texte dans
    /// le cours (voir `study::passage`).
    Study { course_id: String, passage: String },
}

/// Étape de la lecture en cours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ReadAloudPhase {
    Idle,
    /// Voix en téléchargement (accord donné), lecture lancée à la fin.
    Downloading,
    /// Chargement du modèle ou synthèse de la première phrase.
    Preparing,
    Playing,
}

/// Erreur d'une lecture (le texte affiché est choisi par la page).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ReadAloudError {
    /// Aucun périphérique de sortie audio.
    NoOutputDevice,
    /// ONNX Runtime indisponible ou processeur incompatible.
    RuntimeUnavailable,
    /// Téléchargement de la voix échoué ou annulé.
    DownloadFailed,
    /// Fichiers de la voix illisibles ou échec de la synthèse.
    EngineFailed,
}

/// État de la lecture, envoyé à chaque changement (`READ_ALOUD_EVENT`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReadAloudState {
    /// Texte lu (ou en préparation) ; en `Idle`, celui de l'erreur, sinon
    /// `None`.
    pub source: Option<ReadAloudSource>,
    pub phase: ReadAloudPhase,
    /// Progression du téléchargement (0 à 100) en `Downloading`.
    pub percent: Option<u8>,
    /// Erreur de la lecture qui vient de finir (alors `Idle`).
    pub error: Option<ReadAloudError>,
}

impl ReadAloudState {
    pub fn idle() -> Self {
        Self {
            source: None,
            phase: ReadAloudPhase::Idle,
            percent: None,
            error: None,
        }
    }
}

/// Réponse des commandes `read_aloud` et `read_aloud_study`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum ReadAloudOutcome {
    /// Lecture lancée : suivre `READ_ALOUD_EVENT`.
    Started,
    /// La voix de la langue du texte manque : demander l'accord, puis
    /// rappeler la commande avec `download = true`.
    NeedsVoice {
        #[serde(rename = "modelId")]
        #[ts(rename = "modelId")]
        model_id: String,
        language: VoiceLanguage,
        bytes: u64,
    },
    /// Réponse dans une autre langue que fr, en ou es.
    UnsupportedLanguage,
    /// Rien à lire (réponse vide, seulement du code…).
    NothingToRead,
}

/// File de téléchargement des voix (dossier `voice-models`), distincte de
/// celle de la dictée (Tauri range ses états par type).
pub struct VoiceDownloads(pub crate::dictation::downloads::SpeechDownloads);

impl std::ops::Deref for VoiceDownloads {
    type Target = crate::dictation::downloads::SpeechDownloads;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
