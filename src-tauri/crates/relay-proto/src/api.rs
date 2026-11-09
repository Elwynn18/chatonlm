//! Corps JSON des points d'accès (champs en camelCase).

use crate::pow::PowParams;
use crate::{DEFAULT_RESULTS, MAX_QUERY_CHARS, MAX_QUERY_WORDS, MAX_RESULTS};
use serde::{Deserialize, Serialize};

pub const PATH_HEALTH: &str = "/health";
pub const PATH_CHALLENGE: &str = "/v1/challenge";
pub const PATH_REGISTER: &str = "/v1/register";
pub const PATH_SEARCH: &str = "/v1/search";
pub const PATH_STATUS: &str = "/v1/status";

/// Réponse de `POST /v1/challenge`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChallengeResponse {
    /// Jeton à renvoyer tel quel avec la preuve.
    pub challenge: String,
    /// Réglages (aussi dans le jeton, qui fait foi).
    pub params: PowParams,
    /// Après cette date (Unix, secondes), le défi est refusé.
    pub expires_at: u64,
}

/// Corps de `POST /v1/register` (signé par la clé à inscrire).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    pub challenge: String,
    pub nonces: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterResponse {
    pub quota: Quota,
}

/// Corps de `POST /v1/search`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u8>,
    /// Langue des résultats souhaitée (ISO 639-1, minuscules : `fr`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// Pays (ISO 3166-1 alpha-2, majuscules : `FR`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
}

/// Requête vérifiée, prête pour un fournisseur.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Search {
    pub query: String,
    pub max_results: u8,
    pub lang: Option<String>,
    pub country: Option<String>,
}

impl SearchRequest {
    /// Vérifie et normalise (espaces de tête et de fin retirés, nombre de
    /// résultats borné, langue et pays au bon format ou refusés).
    pub fn validate(&self) -> Option<Search> {
        let query = self.query.trim();
        if query.is_empty()
            || query.chars().count() > MAX_QUERY_CHARS
            || query.split_whitespace().count() > MAX_QUERY_WORDS
            || query.chars().any(char::is_control)
        {
            return None;
        }
        let max_results = self.max_results.unwrap_or(DEFAULT_RESULTS);
        if max_results == 0 {
            return None;
        }
        let code = |value: &Option<String>, upper: bool| -> Result<Option<String>, ()> {
            match value.as_deref() {
                None => Ok(None),
                Some(text)
                    if text.len() == 2
                        && text.bytes().all(|b| {
                            if upper {
                                b.is_ascii_uppercase()
                            } else {
                                b.is_ascii_lowercase()
                            }
                        }) =>
                {
                    Ok(Some(text.to_owned()))
                }
                Some(_) => Err(()),
            }
        };
        Some(Search {
            query: query.to_owned(),
            max_results: max_results.min(MAX_RESULTS),
            lang: code(&self.lang, false).ok()?,
            country: code(&self.country, true).ok()?,
        })
    }
}

/// Un résultat, au format du `web_search` d'Ollama que le relais remplace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    /// Extrait pour le modèle (l'app ne le garde pas dans l'historique :
    /// conditions de Brave, seuls titres et liens y restent).
    pub content: String,
}

/// Fournisseur qui a répondu (mention « Powered by Brave »).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Brave,
    Mojeek,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Brave => "brave",
            Provider::Mojeek => "mojeek",
        }
    }
}

/// Quota de l'installation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    pub daily_limit: u32,
    pub remaining_today: u32,
    /// Prochaine remise à zéro (minuit UTC, Unix, secondes).
    pub resets_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub provider: Provider,
    pub quota: Quota,
}

/// État du service.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Ok,
    Paused,
    BudgetExhausted,
}

/// Réponse de `GET /v1/status` (signé).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse {
    pub service: ServiceState,
    pub quota: Quota,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_app_version: Option<String>,
    pub server_time: u64,
}

/// Codes d'erreur stables : l'app les traduit (jamais un texte du serveur).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Corps illisible, requête vide ou trop longue (400).
    BadRequest,
    /// En-têtes de signature absents ou illisibles, signature fausse (401).
    BadSignature,
    /// Horloge de l'app trop décalée : `serverTime` donné (401).
    ClockSkew,
    /// Requête déjà reçue (401).
    Replay,
    /// Clé pas (ou plus) inscrite : s'inscrire de nouveau (401).
    UnknownKey,
    /// Clé bannie (403).
    Banned,
    /// Défi illisible, forgé ou périmé (400).
    ChallengeInvalid,
    /// Preuve de travail fausse (400).
    PowInvalid,
    /// Quota du jour épuisé : `retryAt` (429).
    QuotaExceeded,
    /// Trop de requêtes depuis ce réseau : `retryAt` (429).
    RateLimited,
    /// Version de l'app trop ancienne (426).
    Outdated,
    /// Budget du mois épuisé : `retryAt` = 1er du mois suivant (503).
    BudgetExhausted,
    /// Service mis en pause par son administrateur (503).
    Paused,
    /// Aucun fournisseur n'a répondu (502).
    ProviderUnavailable,
    /// Erreur interne (500).
    Internal,
    /// Code inconnu de cette version de l'app.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorDetail {
    pub code: ErrorCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_time: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(query: &str) -> SearchRequest {
        SearchRequest {
            query: query.into(),
            max_results: None,
            lang: None,
            country: None,
        }
    }

    #[test]
    fn search_is_validated() {
        let ok = request("  chaton roux ").validate().unwrap();
        assert_eq!(ok.query, "chaton roux");
        assert_eq!(ok.max_results, DEFAULT_RESULTS);
        assert!(request("   ").validate().is_none());
        assert!(
            request(&"a".repeat(MAX_QUERY_CHARS + 1))
                .validate()
                .is_none()
        );
        assert!(request(&"é".repeat(MAX_QUERY_CHARS)).validate().is_some());
        assert!(
            request(&"a ".repeat(MAX_QUERY_WORDS + 1))
                .validate()
                .is_none()
        );
        assert!(request("a\u{0}b").validate().is_none());
        let many = SearchRequest {
            max_results: Some(50),
            ..request("x")
        }
        .validate()
        .unwrap();
        assert_eq!(many.max_results, MAX_RESULTS);
        assert!(
            SearchRequest {
                max_results: Some(0),
                ..request("x")
            }
            .validate()
            .is_none()
        );
        let lang = SearchRequest {
            lang: Some("fr".into()),
            country: Some("FR".into()),
            ..request("x")
        };
        assert_eq!(lang.validate().unwrap().lang.as_deref(), Some("fr"));
        for (lang, country) in [
            (Some("FR"), None),
            (Some("fra"), None),
            (None, Some("fr")),
            (None, Some("F1")),
        ] {
            let bad = SearchRequest {
                lang: lang.map(Into::into),
                country: country.map(Into::into),
                ..request("x")
            };
            assert!(bad.validate().is_none(), "{lang:?} {country:?}");
        }
    }

    #[test]
    fn error_codes_are_stable() {
        let body = ErrorBody {
            error: ErrorDetail {
                code: ErrorCode::BudgetExhausted,
                retry_at: Some(5),
                server_time: None,
            },
        };
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"error":{"code":"budget_exhausted","retryAt":5}}"#
        );
        let future: ErrorBody = serde_json::from_str(r#"{"error":{"code":"new_thing"}}"#).unwrap();
        assert_eq!(future.error.code, ErrorCode::Unknown);
        let request: SearchRequest =
            serde_json::from_str(r#"{"query":"x","maxResults":3}"#).unwrap();
        assert_eq!(request.max_results, Some(3));
    }
}
