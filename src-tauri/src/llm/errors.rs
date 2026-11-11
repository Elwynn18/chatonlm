//! Erreurs de l'accès aux modèles (contrat 102 § 1.2) : moteur local,
//! client d'inférence, modèles distants, et leur code pour la page
//! ([`ChatErrorKind`]). Table essayée en entier (`tests`).

use crate::chat::ChatErrorKind;
use crate::engine::EngineError;
use crate::error::AppError;
use crate::i18n::t;
use crate::inference::InferenceError;

/// Échec d'une ouverture ou d'une requête.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LlmError {
    #[error("{0}")]
    Engine(EngineError),
    /// Erreur du client d'inférence d'un modèle LOCAL (celles d'un distant
    /// sont classées par [`super::Session::error`]).
    #[error("{0}")]
    Inference(InferenceError),
    /// Modèle absent de la base locale.
    #[error("{}", t!("engine.notInstalled", model = _0))]
    NotInstalled(String),
    /// Modèle distant, réglage `remoteModels` coupé : rien n'est envoyé.
    #[error("{}", t!("llm.remoteOff"))]
    RemoteOff,
    /// Aucun fournisseur pour ce modèle distant (pas réglé, retiré).
    #[error("{}", t!("llm.remoteUnavailable"))]
    RemoteUnavailable,
    /// Clé refusée par le fournisseur (401, 403).
    #[error("{}", t!("llm.remoteKey", provider = provider))]
    RemoteKey { provider: String },
    /// Limite du fournisseur atteinte (402, 429).
    #[error("{}", t!("llm.remoteLimit", provider = provider))]
    RemoteLimit { provider: String },
    /// Fournisseur injoignable (connexion, délai).
    #[error("{}", t!("llm.remoteUnreachable", provider = provider))]
    RemoteUnreachable { provider: String },
    /// Arrêt demandé (ou modèle déchargé à la demande pendant la requête).
    #[error("{}", t!("llm.cancelled"))]
    Cancelled,
}

impl From<EngineError> for LlmError {
    fn from(error: EngineError) -> Self {
        LlmError::Engine(error)
    }
}

impl LlmError {
    /// Code stable (journal, essais, page).
    pub fn code(&self) -> &'static str {
        match self {
            LlmError::Engine(error) => error.code(),
            LlmError::Inference(error) => error.code(),
            LlmError::NotInstalled(_) => "notInstalled",
            LlmError::RemoteOff => "remoteOff",
            LlmError::RemoteUnavailable => "remoteUnavailable",
            LlmError::RemoteKey { .. } => "remoteKey",
            LlmError::RemoteLimit { .. } => "remoteLimit",
            LlmError::RemoteUnreachable { .. } => "remoteUnreachable",
            LlmError::Cancelled => "cancelled",
        }
    }

    /// Erreur à montrer dans une conversation ; `None` : un arrêt (rien à
    /// montrer). Une coupure d'un modèle déchargé à la demande arrive déjà
    /// en [`LlmError::Cancelled`] ([`super::Session::error`]).
    pub fn chat_kind(&self) -> Option<ChatErrorKind> {
        Some(match self {
            LlmError::Engine(error) => match error {
                EngineError::Unavailable => ChatErrorKind::EngineUnavailable,
                EngineError::NotInstalled(_) | EngineError::MissingFile(_) => {
                    ChatErrorKind::ModelMissing
                }
                EngineError::StartFailed(_)
                | EngineError::NotReady(_)
                | EngineError::Crashed(_) => ChatErrorKind::EngineFailed,
                EngineError::Unloaded | EngineError::Closed => return None,
            },
            LlmError::Inference(error) => match error {
                InferenceError::ModelNotFound { .. } => ChatErrorKind::ModelMissing,
                InferenceError::Connect(_)
                | InferenceError::Timeout
                | InferenceError::Unauthorized
                | InferenceError::Loading => ChatErrorKind::EngineFailed,
                InferenceError::Interrupted(_) => ChatErrorKind::ConnectionLost,
                InferenceError::ContextExceeded { .. } => ChatErrorKind::ContextFull,
                InferenceError::ImageUnsupported
                | InferenceError::Http { .. }
                | InferenceError::Malformed(_) => ChatErrorKind::Model,
                InferenceError::Cancelled => return None,
            },
            LlmError::NotInstalled(_) => ChatErrorKind::ModelMissing,
            LlmError::RemoteOff => ChatErrorKind::RemoteOff,
            // Fournisseur absent : le modèle n'est plus disponible.
            LlmError::RemoteUnavailable => ChatErrorKind::ModelMissing,
            LlmError::RemoteKey { .. } => ChatErrorKind::RemoteKey,
            LlmError::RemoteLimit { .. } => ChatErrorKind::RemoteLimit,
            LlmError::RemoteUnreachable { .. } => ChatErrorKind::RemoteUnreachable,
            LlmError::Cancelled => return None,
        })
    }

    /// Texte pour l'utilisateur, dans la langue du moment.
    pub fn message(&self) -> String {
        match self {
            LlmError::Inference(error) => inference_message(error),
            error => error.to_string(),
        }
    }
}

/// Texte d'une erreur du client d'inférence (son `Display` est un détail
/// technique en anglais, pour le journal).
fn inference_message(error: &InferenceError) -> String {
    match error {
        InferenceError::Loading => t!("llm.inference.loading"),
        InferenceError::ModelNotFound { .. } => t!("llm.inference.modelNotFound"),
        InferenceError::ContextExceeded { .. } => t!("llm.inference.contextExceeded"),
        InferenceError::ImageUnsupported => t!("llm.inference.imageUnsupported"),
        InferenceError::Unauthorized => t!("llm.inference.unauthorized"),
        InferenceError::Http { status, .. } => t!("llm.inference.http", status = status),
        InferenceError::Connect(_) => t!("llm.inference.connect"),
        InferenceError::Timeout => t!("llm.inference.timeout"),
        InferenceError::Cancelled => t!("llm.cancelled"),
        InferenceError::Interrupted(_) => t!("llm.inference.interrupted"),
        InferenceError::Malformed(_) => t!("llm.inference.malformed"),
    }
}

impl From<LlmError> for AppError {
    fn from(error: LlmError) -> Self {
        match &error {
            LlmError::NotInstalled(_)
            | LlmError::Engine(EngineError::NotInstalled(_))
            | LlmError::RemoteUnavailable => AppError::NotFound(error.message()),
            LlmError::RemoteOff => AppError::Invalid(error.message()),
            _ => AppError::Engine(error.message()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// Table du contrat 102 § 1.2, en entier.
    #[test]
    fn every_error_has_its_kind() {
        use ChatErrorKind as K;
        let engine = |error| LlmError::Engine(error);
        let inference = |error| LlmError::Inference(error);
        let cases: Vec<(LlmError, Option<ChatErrorKind>)> = vec![
            (engine(EngineError::Unavailable), Some(K::EngineUnavailable)),
            (
                engine(EngineError::NotInstalled("m".into())),
                Some(K::ModelMissing),
            ),
            (
                engine(EngineError::MissingFile(PathBuf::from("/m"))),
                Some(K::ModelMissing),
            ),
            (
                engine(EngineError::StartFailed("r".into())),
                Some(K::EngineFailed),
            ),
            (engine(EngineError::NotReady(3)), Some(K::EngineFailed)),
            (
                engine(EngineError::Crashed("r".into())),
                Some(K::EngineFailed),
            ),
            (engine(EngineError::Unloaded), None),
            (engine(EngineError::Closed), None),
            (LlmError::NotInstalled("m".into()), Some(K::ModelMissing)),
            (
                inference(InferenceError::ModelNotFound {
                    message: "m".into(),
                }),
                Some(K::ModelMissing),
            ),
            (
                inference(InferenceError::Connect("x".into())),
                Some(K::EngineFailed),
            ),
            (inference(InferenceError::Timeout), Some(K::EngineFailed)),
            (
                inference(InferenceError::Unauthorized),
                Some(K::EngineFailed),
            ),
            (inference(InferenceError::Loading), Some(K::EngineFailed)),
            (
                inference(InferenceError::Interrupted("x".into())),
                Some(K::ConnectionLost),
            ),
            (
                inference(InferenceError::ContextExceeded {
                    n_prompt_tokens: Some(9),
                    n_ctx: Some(4),
                }),
                Some(K::ContextFull),
            ),
            (inference(InferenceError::ImageUnsupported), Some(K::Model)),
            (
                inference(InferenceError::Http {
                    status: 500,
                    message: "x".into(),
                }),
                Some(K::Model),
            ),
            (
                inference(InferenceError::Malformed("x".into())),
                Some(K::Model),
            ),
            (inference(InferenceError::Cancelled), None),
            (LlmError::RemoteOff, Some(K::RemoteOff)),
            (LlmError::RemoteUnavailable, Some(K::ModelMissing)),
            (
                LlmError::RemoteKey {
                    provider: "p".into(),
                },
                Some(K::RemoteKey),
            ),
            (
                LlmError::RemoteLimit {
                    provider: "p".into(),
                },
                Some(K::RemoteLimit),
            ),
            (
                LlmError::RemoteUnreachable {
                    provider: "p".into(),
                },
                Some(K::RemoteUnreachable),
            ),
            (LlmError::Cancelled, None),
        ];
        for (error, kind) in cases {
            assert_eq!(error.chat_kind(), kind, "{error:?}");
            // Un texte dans la langue du moment, jamais vide ni le détail
            // technique anglais du client.
            let message = error.message();
            assert!(!message.is_empty(), "{error:?}");
            assert!(!message.contains("cannot connect: x"), "{message}");
        }
    }

    #[test]
    fn app_errors_keep_their_meaning() {
        assert!(matches!(
            AppError::from(LlmError::NotInstalled("m".into())),
            AppError::NotFound(_)
        ));
        assert!(matches!(
            AppError::from(LlmError::Engine(EngineError::NotInstalled("m".into()))),
            AppError::NotFound(_)
        ));
        assert!(matches!(
            AppError::from(LlmError::Engine(EngineError::Crashed("r".into()))),
            AppError::Engine(_)
        ));
        assert!(matches!(
            AppError::from(LlmError::RemoteOff),
            AppError::Invalid(_)
        ));
        let message =
            AppError::from(LlmError::Inference(InferenceError::Connect("x".into()))).to_string();
        assert_eq!(message, t!("llm.inference.connect"));
    }
}
