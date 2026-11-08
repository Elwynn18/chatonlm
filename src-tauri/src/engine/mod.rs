//! Moteur de texte de ChatonLM : `llama-server` de llama.cpp, livré dans les
//! paquets (`packaging/llama.env`, `scripts/build-llama-server.sh`) et lancé
//! par l'app, un processus par modèle (outils des sous-processus :
//! `process/`).
//!
//! - [`binary`] : où est le binaire livré (ressources, dossier de
//!   développement, faux binaire des essais).
//!
//! Le reste (superviseur, ordonnanceur des modèles, réglages) vient avec le
//! branchement du chat.

// Branché sur le chat par la suite : rien ne lit encore le binaire.
#[allow(dead_code, reason = "branché avec le superviseur du moteur")]
pub mod binary;
