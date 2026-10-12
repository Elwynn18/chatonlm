//! Commandes appelées par le frontend. Toutes sont asynchrones : une commande
//! synchrone s'exécuterait sur le thread principal (boucle GTK) et figerait la
//! fenêtre pendant un accès à la base ou au réseau.

pub mod account;
pub mod attachments;
pub mod chats;
pub mod desktop;
pub mod dictation;
pub mod hub;
pub mod models;
pub mod models_dir;
pub mod platform;
pub mod proofread;
pub mod server;
pub mod settings;
pub mod study;
pub mod system;
pub mod tts;
pub mod updates;
pub mod voice_mode;

/// Valeur de `OLLAMA_HOST` dans l'environnement de l'app.
pub fn ollama_host_env() -> Option<String> {
    std::env::var("OLLAMA_HOST").ok()
}
