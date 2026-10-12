//! Écoute du mode vocal : micro en flux, annulation d'écho (AEC3 de
//! `sonora`), détection de parole (Silero VAD) et fin de tour.
//!
//! Interface fixée par le contrat de l'étape (`.tools/research/63-voice-mode-
//! contract.md`, § 4) entre l'écoute (ce module) et la séance
//! (`voice_mode::session`) ; implémentée dans les sous-modules.

mod echo;
mod endpoint;
#[cfg(any(test, feature = "e2e"))]
mod fake_mic;
mod listener;
#[cfg(any(test, feature = "e2e"))]
mod noise;
mod system;
#[cfg(test)]
mod test_data;
mod vad;

use std::time::Duration;

pub use system::system_ear;

/// Fréquence de travail de l'écoute (celle des moteurs de transcription).
pub const RATE: u32 = 16_000;

/// Prise de parole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnMode {
    /// Fin du tour détectée par le silence.
    HandsFree,
    /// Le tour dure tant que le bouton (ou Espace) est maintenu.
    PushToTalk,
}

/// Réglages de l'écoute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListenConfig {
    pub mode: TurnMode,
    /// Silence qui termine un tour (mains libres).
    pub silence: Duration,
    /// Parler pendant la réponse la coupe (`ListenEvent::BargeIn`, mains
    /// libres seulement).
    pub barge_in: bool,
}

/// Ce que la séance attend de l'écoute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Écoute d'un tour.
    Listen,
    /// ChatonLM répond ou parle : seule une parole franche est signalée
    /// (`BargeIn`), puis l'écoute passe d'elle-même en `Listen`.
    Respond,
    /// Rien n'est signalé (transcription en cours…) ; l'annulation d'écho
    /// continue de s'adapter.
    Ignore,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ListenEvent {
    /// Niveaux (0 à 1) du micro après annulation d'écho et de la voix jouée,
    /// environ 15 fois par seconde.
    Levels { input: f32, output: f32 },
    /// Début de parole (gate `Listen`), ou appui en push-to-talk.
    SpeechStarted,
    /// Parole franche pendant `Respond` : la séance coupe la réponse ; la
    /// parole continue d'être enregistrée comme tour suivant (gate passée
    /// d'elle-même en `Listen`).
    BargeIn,
    /// Fin du tour : son à 16 kHz mono, pré-roll compris.
    Utterance(Vec<f32>),
    /// Parole trop courte, ou appui sans son : abandonnée, l'écoute continue.
    Discarded,
    /// Micro perdu ou en échec (texte traduit) : l'écoute est finie.
    Failed(String),
}

/// Rappel des événements, appelé depuis le fil de l'écoute (ne doit pas
/// bloquer).
pub type Events = Box<dyn Fn(ListenEvent) + Send + Sync>;

/// Écoute ouverte ; lâchée, elle s'arrête et ferme le micro (en ~100 ms au
/// plus, sans bloquer l'appelant plus longtemps).
pub trait Listening: Send + Sync {
    fn set_gate(&self, gate: Gate);
    /// « J'ai fini » : fin du tour tout de suite si une parole est en cours
    /// (mains libres) ; sans effet sinon.
    fn end_turn(&self);
    /// Push-to-talk : appui (début du tour, gate passée en `Listen`) ou
    /// relâche (fin du tour).
    fn hold(&self, pressed: bool);
    /// Réglages changés pendant la séance.
    fn configure(&self, config: ListenConfig);
}

/// Fabrique d'écoutes : le vrai micro (`system_ear`), ou un faux dans les
/// essais.
pub trait Ear: Send + Sync {
    /// Ouvre le micro, charge la détection et l'annulation d'écho (bloquant :
    /// appelé dans `spawn_blocking`) ; gate initiale `Listen`. Erreur
    /// traduite (`t!`).
    fn open(&self, config: ListenConfig, events: Events) -> Result<Box<dyn Listening>, String>;
}
