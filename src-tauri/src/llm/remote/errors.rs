//! Erreurs d'une requête à un fournisseur distant, classées à part de celles
//! de `llama-server` (un 503 n'y est pas un modèle qui se charge, un modèle
//! inconnu n'y est pas un modèle à installer). Appelé par
//! [`crate::llm::Session::error`] pour une session distante ; `provider` est
//! le NOM du fournisseur choisi par l'utilisateur (repris dans les textes).

use crate::inference::InferenceError;
use crate::llm::LlmError;

/// Classe `error`, reçue de `provider` (arrêt déjà écarté par l'appelant) :
/// - 401, 403 → clé refusée ;
/// - 402, 429, et un 5xx qui parle de débit ou de quota → limite atteinte ;
/// - connexion, délai, et tout autre 5xx (surcharge, « aucun fournisseur
///   disponible », 503 sans corps) → injoignable pour le moment ;
/// - modèle inconnu (corps « model '…' not found », ou 404) → modèle
///   indisponible chez lui ;
/// - le reste (requête refusée, réponse illisible, coupure…) : erreur du
///   client, comme pour un modèle local.
pub fn classify(provider: &str, error: InferenceError) -> LlmError {
    let provider = provider.to_owned();
    match error {
        InferenceError::Unauthorized
        | InferenceError::Http {
            status: 401 | 403, ..
        } => LlmError::RemoteKey { provider },
        InferenceError::Http {
            status: 402 | 429, ..
        } => LlmError::RemoteLimit { provider },
        InferenceError::Http { status, message } if (500..600).contains(&status) => {
            if speaks_of_limits(&message) {
                LlmError::RemoteLimit { provider }
            } else {
                LlmError::RemoteUnreachable { provider }
            }
        }
        InferenceError::Loading | InferenceError::Connect(_) | InferenceError::Timeout => {
            LlmError::RemoteUnreachable { provider }
        }
        InferenceError::ModelNotFound { .. } | InferenceError::Http { status: 404, .. } => {
            LlmError::RemoteUnavailable
        }
        error => LlmError::Inference(error),
    }
}

/// Le texte d'une erreur du serveur parle d'une limite de l'utilisateur
/// (débit, quota), pas d'une panne.
fn speaks_of_limits(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "rate limit",
        "rate-limit",
        "quota",
        "too many requests",
        "usage limit",
    ]
    .iter()
    .any(|words| message.contains(words))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(status: u16, message: &str) -> InferenceError {
        InferenceError::Http {
            status,
            message: message.into(),
        }
    }

    /// Table entière : chaque erreur d'un fournisseur a son code.
    #[test]
    fn remote_errors_have_their_own_table() {
        let name = "My server";
        let key = || LlmError::RemoteKey {
            provider: name.into(),
        };
        let limit = || LlmError::RemoteLimit {
            provider: name.into(),
        };
        let unreachable = || LlmError::RemoteUnreachable {
            provider: name.into(),
        };
        let cases = [
            (InferenceError::Unauthorized, key()),
            (http(401, ""), key()),
            (http(403, "forbidden"), key()),
            (http(402, "no credit"), limit()),
            (http(429, ""), limit()),
            (http(503, "Rate limit exceeded, retry later"), limit()),
            (http(529, "Daily quota reached"), limit()),
            (http(503, "No available providers"), unreachable()),
            (http(500, "internal error"), unreachable()),
            (http(502, ""), unreachable()),
            (InferenceError::Loading, unreachable()),
            (InferenceError::Connect("refused".into()), unreachable()),
            (InferenceError::Timeout, unreachable()),
            (
                InferenceError::ModelNotFound {
                    message: "model 'x' not found".into(),
                },
                LlmError::RemoteUnavailable,
            ),
            (
                http(404, "The model `x` does not exist"),
                LlmError::RemoteUnavailable,
            ),
            (
                http(422, "extra fields"),
                LlmError::Inference(http(422, "extra fields")),
            ),
            (
                InferenceError::Interrupted("cut".into()),
                LlmError::Inference(InferenceError::Interrupted("cut".into())),
            ),
            (
                InferenceError::ContextExceeded {
                    n_prompt_tokens: None,
                    n_ctx: None,
                },
                LlmError::Inference(InferenceError::ContextExceeded {
                    n_prompt_tokens: None,
                    n_ctx: None,
                }),
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(classify(name, error.clone()), expected, "{error:?}");
        }
        // Jamais « le modèle se charge » ni « non installé » pour un distant.
        use crate::chat::ChatErrorKind as K;
        assert_eq!(
            classify(name, InferenceError::Loading).chat_kind(),
            Some(K::RemoteUnreachable)
        );
        assert_eq!(
            classify(name, http(503, "No available providers")).chat_kind(),
            Some(K::RemoteUnreachable)
        );
        // Les textes nomment le fournisseur par son nom.
        assert!(
            classify(name, InferenceError::Timeout)
                .message()
                .contains(name)
        );
        assert!(classify(name, http(401, "")).message().contains(name));
        assert!(classify(name, http(429, "")).message().contains(name));
    }
}
