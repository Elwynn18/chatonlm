//! Scénarios du lot E (contrat 103 § 3) : e2e basculés du faux Ollama vers
//! le faux moteur. Noms `chatonlm-<nom>` propres à ce lot ; un nom unique
//! dans tous les lots et qui ne contient aucun nom intégré (`scenario.rs`,
//! cherché d'abord) : essai du socle.

/// Interface de l'app (lancement, fenêtre, barre latérale, historique).
pub mod app;
/// Chat de la page : Markdown, pièces jointes, modification, flux.
pub mod chat;
/// Réglages, premier démarrage, mises à jour, dictée sans carte son, images.
pub mod ui;
/// Voix : mode vocal, dictée, lecture à voix haute, correction, étude,
/// Easy Link.
pub mod voice;
/// Extension VS Code (pont).
pub mod vscode;
