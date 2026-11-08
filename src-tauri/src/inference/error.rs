//! Erreurs du client, classées d'après le code HTTP et le corps
//! `{"error": {"code", "message", "type"}}` de `llama-server`. Les textes
//! (`Display`) sont des détails techniques en anglais pour le journal : ce
//! qui est montré à l'utilisateur part du variant ([`InferenceError::code`]),
//! traduit par l'appelant (`t!`), jamais d'une comparaison de texte.

use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InferenceError {
    /// 503 : modèle en cours de chargement.
    #[error("the model is loading")]
    Loading,
    /// 400 « model '…' not found » (mode routeur).
    #[error("model not found: {message}")]
    ModelNotFound { message: String },
    /// 400 `exceed_context_size_error` : le prompt ne tient pas dans le
    /// contexte (le serveur ne tronque pas l'historique).
    #[error("the prompt ({n_prompt_tokens:?} tokens) exceeds the context ({n_ctx:?})")]
    ContextExceeded {
        n_prompt_tokens: Option<u32>,
        n_ctx: Option<u32>,
    },
    /// 500 « image input is not supported » : modèle lancé sans projecteur
    /// (`--mmproj`).
    #[error("this model does not accept images")]
    ImageUnsupported,
    /// 401 : clé d'API refusée.
    #[error("the API key was refused")]
    Unauthorized,
    /// Autre réponse d'erreur.
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },
    /// Connexion impossible (rien n'écoute, connexion refusée ou fermée
    /// avant la réponse).
    #[error("cannot connect: {0}")]
    Connect(String),
    /// Délai dépassé (réponse, ou silence entre deux blocs du flux).
    #[error("timed out")]
    Timeout,
    /// Annulé par l'appelant.
    #[error("cancelled")]
    Cancelled,
    /// Réponse coupée en cours de route (serveur arrêté ou planté).
    #[error("the reply was interrupted: {0}")]
    Interrupted(String),
    /// Réponse illisible ou trop grande.
    #[error("malformed reply: {0}")]
    Malformed(String),
}

impl InferenceError {
    /// Code stable de l'erreur, pour la page et les traductions.
    pub fn code(&self) -> &'static str {
        match self {
            InferenceError::Loading => "loading",
            InferenceError::ModelNotFound { .. } => "modelNotFound",
            InferenceError::ContextExceeded { .. } => "contextExceeded",
            InferenceError::ImageUnsupported => "imageUnsupported",
            InferenceError::Unauthorized => "unauthorized",
            InferenceError::Http { .. } => "http",
            InferenceError::Connect(_) => "connect",
            InferenceError::Timeout => "timeout",
            InferenceError::Cancelled => "cancelled",
            InferenceError::Interrupted(_) => "interrupted",
            InferenceError::Malformed(_) => "malformed",
        }
    }

    /// Erreur d'une réponse de code `status` et de corps `body` (JSON du
    /// serveur, ou `Null` s'il n'en est pas).
    pub fn from_response(status: u16, body: &Value) -> InferenceError {
        let error = &body["error"];
        let message = error["message"]
            .as_str()
            .or(error.as_str())
            .unwrap_or_default()
            .to_owned();
        let kind = error["type"].as_str().unwrap_or_default();
        let number = |value: &Value| value.as_u64().and_then(|n| u32::try_from(n).ok());
        match status {
            503 => InferenceError::Loading,
            401 => InferenceError::Unauthorized,
            400 if kind == "exceed_context_size_error" => InferenceError::ContextExceeded {
                n_prompt_tokens: number(&error["n_prompt_tokens"]),
                n_ctx: number(&error["n_ctx"]),
            },
            400 | 404 if message.starts_with("model '") && message.ends_with("' not found") => {
                InferenceError::ModelNotFound { message }
            }
            500 if message.contains("image input is not supported") => {
                InferenceError::ImageUnsupported
            }
            _ => InferenceError::Http { status, message },
        }
    }

    /// Erreur de reqwest pendant l'envoi ou l'attente des en-têtes.
    pub(super) fn from_send(error: &reqwest::Error) -> InferenceError {
        if error.is_timeout() {
            InferenceError::Timeout
        } else {
            InferenceError::Connect(chain(error))
        }
    }

    /// Erreur de reqwest pendant la lecture du corps.
    pub(super) fn from_body(error: &reqwest::Error) -> InferenceError {
        if error.is_timeout() {
            InferenceError::Timeout
        } else {
            InferenceError::Interrupted(chain(error))
        }
    }
}

/// Erreur et ses causes, sur une ligne (le journal).
fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn real_error_bodies_get_their_variant() {
        // Corps relevés sur llama-server b12110.
        let loading = json!({ "error": { "message": "Loading model", "type": "unavailable_error", "code": 503 } });
        assert_eq!(
            InferenceError::from_response(503, &loading),
            InferenceError::Loading
        );
        let key = json!({ "error": { "message": "Invalid API Key", "type": "authentication_error", "code": 401 } });
        assert_eq!(
            InferenceError::from_response(401, &key),
            InferenceError::Unauthorized
        );
        let context = json!({ "error": { "code": 400, "message": "request (1509 tokens) exceeds the available context size (1024 tokens), try increasing it",
            "type": "exceed_context_size_error", "n_prompt_tokens": 1509, "n_ctx": 1024 } });
        assert_eq!(
            InferenceError::from_response(400, &context),
            InferenceError::ContextExceeded {
                n_prompt_tokens: Some(1509),
                n_ctx: Some(1024)
            }
        );
        let model = json!({ "error": { "code": 400, "message": "model 'inexistant' not found", "type": "invalid_request_error" } });
        assert_eq!(
            InferenceError::from_response(400, &model),
            InferenceError::ModelNotFound {
                message: "model 'inexistant' not found".into()
            }
        );
        let image = json!({ "error": { "code": 500, "message": "image input is not supported - hint: if this is unexpected, you may need to provide the mmproj", "type": "server_error" } });
        assert_eq!(
            InferenceError::from_response(500, &image),
            InferenceError::ImageUnsupported
        );
        let required = json!({ "error": { "code": 400, "message": "'messages' is required", "type": "invalid_request_error" } });
        assert_eq!(
            InferenceError::from_response(400, &required),
            InferenceError::Http {
                status: 400,
                message: "'messages' is required".into()
            }
        );
        // Sans corps lisible.
        assert_eq!(
            InferenceError::from_response(502, &Value::Null),
            InferenceError::Http {
                status: 502,
                message: String::new()
            }
        );
        // Autre erreur 500 : pas une image refusée ; autre type 400 : pas le contexte.
        let other =
            json!({ "error": { "code": 500, "message": "out of memory", "type": "server_error" } });
        assert!(matches!(
            InferenceError::from_response(500, &other),
            InferenceError::Http { status: 500, .. }
        ));
        let other = json!({ "error": { "code": 400, "message": "exceeds", "type": "invalid_request_error" } });
        assert!(matches!(
            InferenceError::from_response(400, &other),
            InferenceError::Http { status: 400, .. }
        ));
    }

    #[test]
    fn every_variant_has_its_own_code() {
        let all = [
            InferenceError::Loading,
            InferenceError::ModelNotFound {
                message: String::new(),
            },
            InferenceError::ContextExceeded {
                n_prompt_tokens: None,
                n_ctx: None,
            },
            InferenceError::ImageUnsupported,
            InferenceError::Unauthorized,
            InferenceError::Http {
                status: 0,
                message: String::new(),
            },
            InferenceError::Connect(String::new()),
            InferenceError::Timeout,
            InferenceError::Cancelled,
            InferenceError::Interrupted(String::new()),
            InferenceError::Malformed(String::new()),
        ];
        let codes: std::collections::BTreeSet<_> = all.iter().map(InferenceError::code).collect();
        assert_eq!(codes.len(), all.len());
    }
}
