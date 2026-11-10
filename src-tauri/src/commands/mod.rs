//! Commandes appelées par le frontend. Toutes sont asynchrones : une commande
//! synchrone s'exécuterait sur le thread principal (boucle GTK) et figerait la
//! fenêtre pendant un accès à la base ou au réseau.

pub mod attachments;
pub mod bridge;
pub mod chats;
pub mod desktop;
pub mod dictation;
pub mod easy_link;
pub mod engine;
pub mod hub;
pub mod image_models;
pub mod models;
pub mod models_import;
pub mod platform;
pub mod proofread;
pub mod remote;
pub mod report;
pub mod settings;
pub mod study;
pub mod system;
pub mod tts;
pub mod updates;
pub mod voice_mode;
pub mod web_search;
