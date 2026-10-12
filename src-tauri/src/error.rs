//! Erreurs renvoyées au frontend.

use serde::{Serialize, Serializer};

use crate::i18n::t;
use ts_rs::TS;

/// Erreur d'une commande. Le frontend reçoit `{ kind, message }`.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Valeur refusée (réglage invalide...).
    #[error("{0}")]
    Invalid(String),
    /// Élément introuvable (conversation supprimée...).
    #[error("{0}")]
    NotFound(String),
    /// Base de données créée par une version plus récente de ChatonLM.
    #[error("{}", t!("errors.newerDatabase", found = found, supported = supported))]
    NewerDatabase { found: i64, supported: i64 },
    #[error("{}", t!("errors.database", error = _0))]
    Database(#[from] rusqlite::Error),
    #[error("{}", t!("errors.file", error = _0))]
    Io(#[from] std::io::Error),
    /// Un programme de la machine a échoué (xdg-open...) ; message montré tel
    /// quel.
    #[error("{0}")]
    System(String),
    /// Le serveur Ollama est injoignable ou a répondu par une erreur.
    #[error("{0}")]
    Ollama(String),
    #[error("{}", t!("errors.internal", error = _0))]
    Internal(String),
}

/// Forme sérialisée d'une [`AppError`].
#[derive(Serialize, TS)]
#[ts(export, rename = "AppError")]
pub struct AppErrorPayload {
    pub kind: AppErrorKind,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AppErrorKind {
    Invalid,
    NotFound,
    Database,
    System,
    Ollama,
    Internal,
}

impl AppError {
    pub fn kind(&self) -> AppErrorKind {
        match self {
            AppError::Invalid(_) => AppErrorKind::Invalid,
            AppError::NotFound(_) => AppErrorKind::NotFound,
            AppError::NewerDatabase { .. } | AppError::Database(_) => AppErrorKind::Database,
            AppError::System(_) => AppErrorKind::System,
            AppError::Ollama(_) => AppErrorKind::Ollama,
            AppError::Io(_) | AppError::Internal(_) => AppErrorKind::Internal,
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        AppErrorPayload {
            kind: self.kind(),
            message: self.to_string(),
        }
        .serialize(serializer)
    }
}

pub type AppResult<T> = Result<T, AppError>;
