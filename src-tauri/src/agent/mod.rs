//! Atelier (moteur de tâches complexes, propre à ChatonLM), lot 1 : le
//! noyau qui fait travailler un modèle hors conversation.
//!
//! - [`Worker`] : un modèle ouvert pour un travail (sa `Session` le tient
//!   en mémoire, l'autocomplétion de l'éditeur attend : `gpu::activity`),
//!   au patron de `study::request::Gen`, mais générique : réponses en JSON
//!   contraint par un schéma ([`Worker::ask_json`]), textes en flux
//!   ([`Worker::write`]), boucle d'outils bornée ([`Worker::run_tools`]),
//!   jetons comptés par le modèle ([`Worker::count_tokens`]).
//! - Réflexion : coupée si le modèle le permet, sinon bornée ; demandée avec
//!   un budget OBLIGATOIRE ([`ThinkMode`]) : sans lui, un gros modèle
//!   réfléchit jusqu'à la limite sans rien répondre (lot 0, `.tools/research/
//!   109-lot0-preuves.md` § 2.1 et § 7).
//! - Erreurs : codes seulement ([`AgentError`]) ; un texte montré plus tard
//!   passera par `t!`.
//!
//! Toute requête lit la réponse en flux : un silence borne l'attente
//! (`inference::IDLE_TIMEOUT`), jamais la durée totale (un plan du 27B dure
//! plusieurs minutes), et le détecteur de boucle du chat
//! (`chat::repetition::LoopWatch`) surveille réflexion et réponse.
//! Contrat : `.tools/research/110-lot1-contrat.md` § A.

// Appelants : l'Atelier, lot 2.
#![cfg_attr(not(test), allow(dead_code, reason = "Atelier : appelants au lot 2"))]

mod answer;
mod tools;
mod worker;

#[allow(unused_imports, reason = "API de l'Atelier, en partie pour le lot 2")]
pub(crate) use tools::{MAX_TOOL_CALLS, MAX_TOOL_RESULT_CHARS, Tool};
#[allow(unused_imports, reason = "API de l'Atelier, en partie pour le lot 2")]
pub(crate) use worker::{
    ANSWER_TOKENS, Call, Delta, Json, OFF_BUDGET, Text, ThinkMode, Worker, WorkerOptions, Write,
    context_at_least,
};

use crate::inference::InferenceError;
use crate::llm::LlmError;

/// Échec d'un travail du modèle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AgentError {
    /// Arrêt demandé, ou modèle déchargé à la demande pendant le travail.
    Cancelled,
    /// Erreur du moteur, du client ou du fournisseur (code : `LlmError::code`).
    Model(LlmError),
    /// Réponse sans objet JSON lisible, après un nouvel essai.
    Unreadable,
    /// Le modèle boucle (répétition exacte, `chat::repetition`).
    Looping,
    /// Réponse coupée à la limite de jetons, après un nouvel essai à une
    /// limite double : le texte visible obtenu.
    Incomplete { partial: String },
    /// Modèle distant, réglage `remoteModels` coupé : rien n'est envoyé.
    RemoteOff,
}

impl AgentError {
    /// Code stable (journal, page).
    pub fn code(&self) -> &'static str {
        match self {
            AgentError::Cancelled => "cancelled",
            AgentError::Model(error) => error.code(),
            AgentError::Unreadable => "unreadable",
            AgentError::Looping => "looping",
            AgentError::Incomplete { .. } => "incomplete",
            AgentError::RemoteOff => "remoteOff",
        }
    }

    /// Erreur passagère, qui vaut un nouvel essai : délai dépassé, modèle en
    /// chargement, réponse coupée, erreur 5xx du serveur (même règle que
    /// l'Étude).
    pub fn transient(&self) -> bool {
        matches!(
            self,
            AgentError::Model(LlmError::Inference(
                InferenceError::Timeout
                    | InferenceError::Loading
                    | InferenceError::Interrupted(_)
                    | InferenceError::Http { status: 500.., .. }
            ))
        )
    }
}

impl From<LlmError> for AgentError {
    /// Arrêt (annulation, modèle déchargé ou moteur fermé : rien à montrer
    /// dans le chat) → [`AgentError::Cancelled`] ; réglage des distants
    /// coupé → [`AgentError::RemoteOff`].
    fn from(error: LlmError) -> Self {
        match error {
            LlmError::RemoteOff => AgentError::RemoteOff,
            error if error.chat_kind().is_none() => AgentError::Cancelled,
            error => AgentError::Model(error),
        }
    }
}

#[cfg(test)]
mod tests;
