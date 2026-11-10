//! Modèles distants (contrat 102 § 5) : fournisseurs compatibles avec l'API
//! d'OpenAI (Ollama Cloud, OpenRouter, Mistral, serveur à l'adresse de
//! l'utilisateur), avec la clé de l'utilisateur ; COUPÉS par défaut (réglage
//! `remoteModels`, vérifié par [`super::Access::open`] et
//! [`super::models::list`] avant tout appel ici). Aucun modèle n'est payé par
//! le projet.
//!
//! - [`providers`] : lignes de réglage (base, migration 0042), types de la
//!   page, enregistrement, vérification d'une clé ;
//! - [`keys`] : clés dans `<données>/remote/keys.json`, lisible du seul
//!   utilisateur, jamais en base, jamais envoyées à la page (indice : quatre
//!   derniers caractères), jamais au journal ;
//! - [`catalog`] : modèles d'un fournisseur (`/v1/models`), relus et classés ;
//! - [`Remote`] : le crochet de l'app (liste mise en cache, sessions par
//!   `Endpoint::remote`, dialecte de Mistral pour Mistral).

pub mod catalog;
mod errors;
pub mod keys;
pub mod providers;
mod service;
#[cfg(test)]
mod tests;

use futures_util::future::BoxFuture;

use super::models::ChatModel;
use super::{LlmError, Session};
use crate::local_models::ModelCapabilities;
pub use errors::classify;
pub use service::Remote;

/// Modèles distants de l'app (état partagé par [`super::Access`]).
pub trait RemoteModels: Send + Sync + 'static {
    /// Modèles des fournisseurs réglés ; vide si coupé ou sans fournisseur.
    fn list(&self) -> BoxFuture<'_, Vec<ChatModel>>;
    /// Point d'appel du modèle `model` chez `provider` (session distante :
    /// [`Session::remote`]).
    fn open<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
    ) -> BoxFuture<'a, Result<Session, LlmError>>;
    /// Capacités du modèle `model` de `provider` : celles de la liste du
    /// fournisseur si elle répond à temps, sinon conversation et outils
    /// ([`assumed_capabilities`], contrat 102 § 9 point 12). Jamais d'échec :
    /// un fournisseur absent, coupé, sans clé ou injoignable se dit à
    /// l'ouverture ([`RemoteModels::open`]), avec son vrai code.
    fn capabilities<'a>(
        &'a self,
        _provider: &'a str,
        _model: &'a str,
    ) -> BoxFuture<'a, ModelCapabilities> {
        Box::pin(async { assumed_capabilities() })
    }
    /// Oublie ce qui est gardé en mémoire (fournisseur ajouté, changé ou
    /// retiré).
    fn invalidate(&self) {}
}

/// Capacités supposées d'un modèle distant dont la liste ne dit rien :
/// conversation et outils ; ni vision, ni réflexion.
pub fn assumed_capabilities() -> ModelCapabilities {
    ModelCapabilities {
        tools: true,
        ..ModelCapabilities::default()
    }
}

/// Modèles distants de l'app, construits une fois la base ouverte
/// (`llm::setup`).
pub fn for_app(
    store: &crate::store::Store,
    data_dir: &std::path::Path,
) -> std::sync::Arc<dyn RemoteModels> {
    std::sync::Arc::new(Remote::new(store.clone(), data_dir.to_path_buf()))
}

/// Aucun modèle distant (essais) : liste vide, ouverture refusée.
pub struct Unavailable;

impl RemoteModels for Unavailable {
    fn list(&self) -> BoxFuture<'_, Vec<ChatModel>> {
        Box::pin(async { Vec::new() })
    }

    fn open<'a>(
        &'a self,
        _provider: &'a str,
        _model: &'a str,
    ) -> BoxFuture<'a, Result<Session, LlmError>> {
        Box::pin(async { Err(LlmError::RemoteUnavailable) })
    }
}

/// Ligne du journal des modèles distants : jamais une clé (les essais
/// relisent ces lignes).
pub(crate) fn note(line: &str) {
    #[cfg(test)]
    tests::NOTES.lock().push(line.to_owned());
    crate::app_log!("chatonlm: remote models: {line}");
}
