//! Texte parlé pour la lecture à voix haute de ChatonLM (fonction propre à
//! ChatonLM) : ce qu'on lit d'une réponse en Markdown, dans quelle langue, et
//! comment l'écrire pour que Pocket TTS le prononce bien.
//!
//! Crate à part, sans Tauri ni ONNX : ses essais se compilent en quelques
//! secondes. Contrat : `.tools/research/59-tts-contract.md`.

pub mod language;
pub mod markdown;
pub mod normalize;

/// Langue lue par une voix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    En,
    Es,
    Fr,
}
