//! Modèles d'images (propre à ChatonLM) : choix libre sur Hugging Face, par
//! familles connues (pièces annexes et réglages), téléchargement par l'app,
//! pièces partagées entre modèles, modèle choisi pour le moteur d'images
//! (`image_gen`). Contrat : `.tools/research/83-image-models-contract.md`.
//!
//! - `families` : familles connues (`families.json`), reconnaissance par
//!   modèles de base, recommandations ;
//! - `catalog` : recherche et fiches de Hugging Face (client et cache du
//!   ModelHub) ;
//! - `downloads` : file des téléchargements (rangement par empreinte, place) ;
//! - `service` : ce que font les commandes (`commands/image_models.rs`).

pub mod catalog;
pub mod downloads;
pub mod families;
pub mod service;
pub mod types;

pub use downloads::{DOWNLOADS_EVENT, ImageDownloads};
pub use service::ImageModels;
