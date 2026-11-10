//! `chatonlm-relay` : relais de recherche web de ChatonLM, sans compte.
//!
//! Protocole : crate `relay-proto`. Le relais garde les clés des moteurs de
//! recherche (Brave, Mojeek en secours) et ne laisse passer que des
//! installations inscrites par preuve de travail, dans leur quota du jour
//! (50 par défaut), sous un plafond de budget mensuel (30 € par défaut).
//!
//! Ce qu'il garde : clés publiques d'installation (pseudonymes, au jour près)
//! et compteurs du mois, en SQLite. Ce qu'il ne garde jamais : adresse IP
//! (limites par réseau en mémoire, vidées chaque jour), requête cherchée,
//! résultat, clé de fournisseur dans un journal. Déploiement :
//! `packaging/relay/`.

pub mod bench;
pub mod budget;
pub mod config;
#[cfg(feature = "fake")]
pub mod fake;
pub mod limits;
pub mod log;
pub mod providers;
pub mod server;
pub mod store;

pub use server::{Relay, router, serve};

/// Moteur cryptographique de rustls (ring), une fois par processus.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
