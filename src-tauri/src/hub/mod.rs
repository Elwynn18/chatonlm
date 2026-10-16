//! ModelHub : catalogue des modèles GGUF de Hugging Face, téléchargements par
//! Ollama (`hf.co/<auteur>/<dépôt>:<étiquette>`) et modèles installés.
//! Contrat : `.tools/research/37-step15a-contract.md`.

pub(crate) mod cache;
mod capabilities;
pub mod catalog;
pub(crate) mod curation;
pub mod downloads;
pub mod errors;
mod fit;
pub(crate) mod flight;
mod gguf;
pub mod hardware;
pub(crate) mod hf;
pub mod installed;
pub mod manifest;
pub mod names;
pub mod quant;
mod readme;
mod repo;
pub mod state;
pub mod types;

/// Événement émis à chaque changement des téléchargements (liste complète de
/// `HubDownload`).
pub const DOWNLOADS_EVENT: &str = "hub-downloads";
