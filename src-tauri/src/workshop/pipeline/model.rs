//! Trait [`Model`] : ce que le pipeline demande au modèle de texte.
//! Implémenté par [`crate::agent::Worker`] (lot 1), et par un faux
//! scénarisé dans les essais. Futurs boxés, comme [`super::host::Host`].

use crate::agent::{AgentError, Call, Delta, Json, Text, Worker, Write};
use crate::inference::Message;

use super::host::HostFuture;

/// Rappel des fragments d'une réponse en flux.
pub type OnDelta<'a> = &'a (dyn Fn(Delta) + Send + Sync);

/// Le modèle d'un projet (le même pour tout : maître, juges, rédaction).
pub trait Model: Send + Sync {
    /// Réponse en JSON contraint par `call.schema` (un nouvel essai compris).
    fn ask_json<'a>(&'a self, call: &'a Call) -> HostFuture<'a, Result<Json, AgentError>>;

    /// Réponse en texte, en flux.
    fn write<'a>(
        &'a self,
        messages: Vec<Message>,
        options: &'a Write,
        on_delta: OnDelta<'a>,
    ) -> HostFuture<'a, Result<Text, AgentError>>;

    /// Jetons de `text` comptés par le modèle.
    fn count_tokens<'a>(&'a self, text: &'a str) -> HostFuture<'a, Result<usize, AgentError>>;

    /// Contexte du processus, en jetons.
    fn context(&self) -> u32;

    /// Identifiant du modèle.
    fn model(&self) -> &str;
}

impl Model for Worker {
    fn ask_json<'a>(&'a self, call: &'a Call) -> HostFuture<'a, Result<Json, AgentError>> {
        Box::pin(Worker::ask_json(self, call))
    }

    fn write<'a>(
        &'a self,
        messages: Vec<Message>,
        options: &'a Write,
        on_delta: OnDelta<'a>,
    ) -> HostFuture<'a, Result<Text, AgentError>> {
        Box::pin(Worker::write(self, messages, options, on_delta))
    }

    fn count_tokens<'a>(&'a self, text: &'a str) -> HostFuture<'a, Result<usize, AgentError>> {
        Box::pin(Worker::count_tokens(self, text))
    }

    fn context(&self) -> u32 {
        Worker::context(self)
    }

    fn model(&self) -> &str {
        Worker::model(self)
    }
}
