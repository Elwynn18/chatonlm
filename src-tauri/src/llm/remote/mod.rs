//! Crochet des modèles distants (contrat 102 § 1.2) : fournisseurs
//! compatibles OpenAI, clé de l'utilisateur, coupés par défaut. Le socle ne
//! pose que le trait et [`Unavailable`] ; le lot L7 écrit l'implémentation
//! (`llm/remote/**`).

use futures_util::future::BoxFuture;

use super::models::ChatModel;
use super::{LlmError, Session};

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
}

/// Modèles distants de l'app, construits une fois la base ouverte
/// (`llm::setup`) ; le lot L7 remplace ce corps.
pub fn for_app(
    _store: &crate::store::Store,
    _data_dir: &std::path::Path,
) -> std::sync::Arc<dyn RemoteModels> {
    std::sync::Arc::new(Unavailable)
}

/// Aucun modèle distant (socle, essais) : liste vide, ouverture refusée.
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
