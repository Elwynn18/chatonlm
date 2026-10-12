//! Mode conversation vocale (propre à ChatonLM, absent de l'app officielle) :
//! parler → transcription locale → envoi au modèle → réponse lue à voix
//! haute → nouvelle écoute. Contrat : `.tools/research/63-voice-mode-
//! contract.md`.
//!
//! - `audio` : écoute (micro en flux, annulation d'écho, détection de
//!   parole, fin de tour).
//! - `session` : la séance (machine d'états), ses prérequis et son lien avec
//!   la dictée (moteur de transcription), le chat et la lecture à voix haute.

pub mod audio;
pub mod session;

use serde::Serialize;
use ts_rs::TS;

use crate::tts::{ReadAloudError, VoiceLanguage};

pub use session::VoiceMode;

/// Événement de l'état de la séance (`VoiceModeState`), à chaque changement.
pub const STATE_EVENT: &str = "voice-mode";
/// Événement des niveaux (`VoiceModeLevel`), ~15 fois par seconde pendant une
/// séance.
pub const LEVEL_EVENT: &str = "voice-mode-level";
/// Événement d'une phrase transcrite (`VoiceModeUtterance`), à envoyer.
pub const UTTERANCE_EVENT: &str = "voice-mode-utterance";

/// Consigne vocale par défaut (anglais, comme celle du polish), ajoutée en
/// message système aux seules requêtes d'un envoi vocal.
pub const DEFAULT_INSTRUCTIONS: &str = "You are having a spoken conversation. The user talks to you through speech recognition, and your reply will be read aloud by a text-to-speech voice. Answer naturally and conversationally, and keep it brief (a few sentences) unless the user asks for more detail. Do not use Markdown, lists, tables, headings, emoji or code unless the user explicitly asks for them. The transcription may contain recognition errors: infer the most likely meaning. Always reply in the language the user speaks.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum VoiceModePhase {
    #[default]
    Off,
    /// Ouverture du micro, chargement des modèles (détection, transcription, voix).
    Starting,
    /// Micro ouvert, en attente de parole.
    Listening,
    /// Parole en cours (début détecté, ou bouton/Espace maintenu en push-to-talk).
    Hearing,
    Transcribing,
    /// Phrase envoyée : le modèle charge, réfléchit, cherche ou écrit ; rien n'est encore lu.
    Responding,
    /// La réponse est lue.
    Speaking,
    /// Micro fermé après 2 min sans parole ; reprise par `voice_mode_resume`.
    Paused,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum VoiceModeNotice {
    /// Réponse dans une autre langue que fr, en, es : écrite, pas lue.
    UnsupportedLanguage,
    /// Voix de cette langue absente : réponse écrite, pas lue.
    MissingVoice { language: VoiceLanguage },
    /// Lecture en échec (code de la lecture à voix haute).
    ReadFailed { error: ReadAloudError },
    /// Micro, transcription ou envoi en échec ; `message` déjà traduit (t!).
    Failed { message: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceModeState {
    pub phase: VoiceModePhase,
    /// Micro coupé par l'utilisateur (fermé).
    pub muted: bool,
    /// Tour en cours : 0 avant la première phrase, +1 à chaque phrase envoyée.
    pub turn: u32,
    /// Avis du dernier tour (effacé au tour suivant).
    pub notice: Option<VoiceModeNotice>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceModeLevel {
    /// Niveau du micro après annulation d'écho, 0 à 1 (échelle perceptive : dBFS -60..0).
    pub input: f32,
    /// Niveau de la voix jouée, 0 à 1.
    pub output: f32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceModeUtterance {
    pub turn: u32,
    pub text: String,
    /// Langue de la phrase (détection de `speech-text`, fr/en/es), indice de la voix.
    pub language: Option<VoiceLanguage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceModeRequirements {
    /// Modèle de transcription employé (réglage de la dictée, sinon le premier installé du
    /// catalogue, sinon le recommandé `parakeet-tdt-0.6b-v3`).
    pub speech_model: String,
    pub speech_model_installed: bool,
    /// Taille du téléchargement (0 si installé).
    pub speech_model_bytes: u64,
    /// Voix de la langue attendue (langue de la dictée si fr/en/es, sinon de l'interface).
    pub voice_model: String,
    pub voice_language: VoiceLanguage,
    pub voice_installed: bool,
    pub voice_bytes: u64,
    /// La machine ne peut pas : ONNX Runtime absent, processeur incompatible… (texte traduit).
    pub unavailable: Option<String>,
}

#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceModeInfo {
    /// Consigne vocale par défaut (anglais, comme celle du polish).
    pub default_instructions: String,
}

impl VoiceModeInfo {
    pub fn new() -> Self {
        Self {
            default_instructions: DEFAULT_INSTRUCTIONS.into(),
        }
    }
}
