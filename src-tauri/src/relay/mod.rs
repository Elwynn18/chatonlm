//! Client du relais de ChatonLM (recherche web sans Ollama ni compte).
//!
//! Protocole : crate `relay-proto` ; serveur : crate `relay-server`. Chaque
//! installation a sa clé Ed25519 ([`identity`], sous les données de l'app) ;
//! elle s'inscrit une fois par une preuve de travail de quelques secondes
//! (dans une tâche à part, annulable), puis signe chaque requête. Le client
//! corrige seul un décalage d'horloge (une fois, d'après l'heure donnée par le
//! relais) et se réinscrit si le relais a oublié la clé.
//!
//! Erreurs : [`RelayError`], codes stables traduits par `t!` (section `relay`
//! de `backend.json`), jamais le texte du serveur.
//!
//! Adresse : [`DEFAULT_URL`] ; `CHATONLM_RELAY_URL` la remplace dans les
//! builds d'essai seulement (comme les adresses d'Easy Link), à fermer par
//! `withApp` dans les e2e. Code commun à Linux et Windows (seule la
//! permission du fichier de clé diffère : voir [`identity`]).

// Branché sur `web_search` (chat, Étude, Easy Link, VS Code) par le lot L7 :
// d'ici là, seuls les essais s'en servent. À retirer alors.
#![allow(dead_code)]

pub mod identity;
#[cfg(test)]
mod tests;

use crate::i18n::{Lang, t};
use identity::Identity;
use relay_proto::api::{
    ChallengeResponse, ErrorBody, ErrorCode, PATH_CHALLENGE, PATH_REGISTER, PATH_SEARCH,
    PATH_STATUS, RegisterRequest, RegisterResponse, SearchRequest, SearchResponse, StatusResponse,
};
use relay_proto::pow::{self, Challenge};
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Adresse du relais officiel.
pub const DEFAULT_URL: &str = "https://relay.chatonlm.com";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Une recherche passe par un fournisseur (8 s au plus côté relais, puis le
/// secours) : marge comprise.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Réponse lue au plus (dix résultats bornés font quelques dizaines de Ko).
const MAX_REPLY: usize = 512 * 1024;
/// Inscription refusée sans date de reprise (refus de nginx) : pas de nouvel
/// essai avant ce délai.
const REGISTRATION_BACKOFF_SECS: u64 = 60;

#[cfg(any(test, debug_assertions, feature = "e2e"))]
fn test_override() -> Option<String> {
    std::env::var("CHATONLM_RELAY_URL").ok()
}

#[cfg(not(any(test, debug_assertions, feature = "e2e")))]
fn test_override() -> Option<String> {
    None
}

/// Adresse du relais, sans `/` final.
pub fn relay_url() -> String {
    test_override()
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_URL.to_owned())
}

/// Erreur du relais, à montrer par [`RelayError::message`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RelayError {
    /// Relais injoignable (réseau, délai).
    #[error("relay unreachable")]
    Offline,
    /// Quota du jour de l'installation épuisé.
    #[error("daily quota exceeded")]
    QuotaExceeded { retry_at: Option<u64> },
    /// Trop de requêtes (réseau, cadence de la clé, inscriptions du réseau,
    /// relais occupé).
    #[error("rate limited")]
    RateLimited { retry_at: Option<u64> },
    /// Budget épuisé : reprise le 1er du mois suivant, ou à minuit (UTC) pour
    /// l'enveloppe du jour.
    #[error("budget exhausted")]
    BudgetExhausted { retry_at: Option<u64> },
    /// Service mis en pause par son administrateur.
    #[error("service paused")]
    Paused,
    /// Version de l'app trop ancienne pour le relais.
    #[error("app outdated")]
    Outdated,
    /// Installation bannie.
    #[error("installation banned")]
    Banned,
    /// Aucun moteur de recherche n'a répondu.
    #[error("no search provider answered")]
    ProviderUnavailable,
    /// Inscription impossible (preuve refusée, défi illisible).
    #[error("registration failed")]
    Registration,
    /// Requête refusée par le relais (signature, rejeu, horloge, après le
    /// nouvel essai) : le code est gardé pour le journal.
    #[error("request refused: {0:?}")]
    Rejected(ErrorCode),
    /// Recherche vide ou trop longue (vérifiée avant tout envoi).
    #[error("invalid query")]
    InvalidQuery,
    /// Réponse illisible.
    #[error("malformed reply")]
    Malformed,
    /// Clé d'installation impossible à écrire : la sorte d'erreur du système
    /// (`io::ErrorKind`), pour le journal de l'app seulement ; ni le message
    /// du système ni le chemin (nom de l'utilisateur) ne sont montrés.
    #[error("installation key: {0}")]
    Storage(String),
    /// Annulé par l'appelant.
    #[error("cancelled")]
    Cancelled,
}

impl RelayError {
    /// Code stable, pour la page et les traductions.
    pub fn code(&self) -> &'static str {
        match self {
            RelayError::Offline => "offline",
            RelayError::QuotaExceeded { .. } => "quotaExceeded",
            RelayError::RateLimited { .. } => "rateLimited",
            RelayError::BudgetExhausted { .. } => "budgetExhausted",
            RelayError::Paused => "paused",
            RelayError::Outdated => "outdated",
            RelayError::Banned => "banned",
            RelayError::ProviderUnavailable => "providerUnavailable",
            RelayError::Registration => "registrationFailed",
            RelayError::Rejected(_) => "rejected",
            RelayError::InvalidQuery => "invalidQuery",
            RelayError::Malformed => "malformed",
            RelayError::Storage(_) => "storage",
            RelayError::Cancelled => "cancelled",
        }
    }

    /// Texte pour l'utilisateur, dans la langue du moment.
    pub fn message(&self) -> String {
        self.message_in(crate::i18n::current())
    }

    pub fn message_in(&self, lang: Lang) -> String {
        match self {
            RelayError::Offline => t!("relay.offline" in lang),
            RelayError::QuotaExceeded { .. } => t!("relay.quotaExceeded" in lang),
            RelayError::RateLimited { .. } => t!("relay.rateLimited" in lang),
            RelayError::BudgetExhausted { retry_at } => {
                // Reprise avant le 1er du mois suivant : c'est l'enveloppe du jour.
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_secs())
                    .unwrap_or_default();
                if retry_at.is_some_and(|at| at < relay_proto::time::next_month_start(now)) {
                    t!("relay.budgetExhaustedToday" in lang)
                } else {
                    t!("relay.budgetExhausted" in lang)
                }
            }
            RelayError::Paused => t!("relay.paused" in lang),
            RelayError::Outdated => t!("relay.outdated" in lang),
            RelayError::Banned => t!("relay.banned" in lang),
            RelayError::ProviderUnavailable => t!("relay.providerUnavailable" in lang),
            RelayError::Registration => t!("relay.registrationFailed" in lang),
            RelayError::Rejected(_) => t!("relay.rejected" in lang),
            RelayError::InvalidQuery => t!("relay.invalidQuery" in lang),
            RelayError::Malformed => t!("relay.malformed" in lang),
            RelayError::Storage(_) => t!("relay.storage" in lang),
            RelayError::Cancelled => t!("relay.cancelled" in lang),
        }
    }

    fn from_reply(body: &ErrorBody) -> Self {
        let retry_at = body.error.retry_at;
        match body.error.code {
            ErrorCode::QuotaExceeded => RelayError::QuotaExceeded { retry_at },
            ErrorCode::RateLimited => RelayError::RateLimited { retry_at },
            ErrorCode::BudgetExhausted => RelayError::BudgetExhausted { retry_at },
            ErrorCode::Paused => RelayError::Paused,
            ErrorCode::Outdated => RelayError::Outdated,
            ErrorCode::Banned => RelayError::Banned,
            ErrorCode::ProviderUnavailable => RelayError::ProviderUnavailable,
            ErrorCode::ChallengeInvalid | ErrorCode::PowInvalid => RelayError::Registration,
            ErrorCode::BadRequest => RelayError::InvalidQuery,
            code => RelayError::Rejected(code),
        }
    }
}

/// Réponse d'erreur du relais, avec ce qu'il faut pour un nouvel essai.
enum Failure {
    Error(RelayError),
    /// Horloge décalée : heure du relais.
    ClockSkew(Option<u64>),
    /// Clé oubliée par le relais : se réinscrire.
    UnknownKey,
}

impl From<RelayError> for Failure {
    fn from(error: RelayError) -> Self {
        Failure::Error(error)
    }
}

/// Client d'une installation (un par app ; partagé par `Arc`).
pub struct RelayClient {
    base: String,
    http: Option<reqwest::Client>,
    identity: identity::Store,
    /// Décalage ajouté à l'horloge locale (secondes), appris du relais.
    offset: AtomicI64,
    /// Une seule inscription à la fois.
    registering: tokio::sync::Mutex<()>,
    /// Inscription refusée par le relais (`rate_limited`) : pas de nouvel
    /// essai, ni défi ni preuve, avant cette heure (horloge du relais).
    registration_blocked_until: AtomicU64,
}

impl RelayClient {
    /// `data_dir` : dossier des données de l'app (la clé va dans `relay/`).
    pub fn new(data_dir: PathBuf) -> Self {
        Self::with_url(relay_url(), data_dir)
    }

    pub fn with_url(base: String, data_dir: PathBuf) -> Self {
        crate::http::ensure_crypto_provider();
        let local = url::Url::parse(&base)
            .ok()
            .and_then(|url| {
                url.host_str()
                    .map(|host| matches!(host, "127.0.0.1" | "localhost" | "[::1]"))
            })
            .unwrap_or(false);
        let mut builder = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            // Aucune redirection : les en-têtes signés valent pour ce chemin.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("ChatonLM/", env!("CARGO_PKG_VERSION")));
        if local {
            builder = builder.no_proxy();
        }
        Self {
            base: base.trim_end_matches('/').to_owned(),
            http: builder.build().ok(),
            identity: identity::Store::new(data_dir.join(identity::DIR).join(identity::FILE)),
            offset: AtomicI64::new(0),
            registering: tokio::sync::Mutex::new(()),
            registration_blocked_until: AtomicU64::new(0),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn now(&self) -> u64 {
        let local = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default();
        local.saturating_add_signed(self.offset.load(Ordering::Relaxed))
    }

    fn adopt_server_time(&self, server_time: Option<u64>) -> bool {
        let Some(server) = server_time else {
            return false;
        };
        let local = self
            .now()
            .saturating_add_signed(-self.offset.load(Ordering::Relaxed));
        let offset =
            i64::try_from(server).unwrap_or(i64::MAX) - i64::try_from(local).unwrap_or(i64::MAX);
        self.offset.store(offset, Ordering::Relaxed);
        crate::app_log!("chatonlm: relay clock offset adjusted to {offset} s");
        true
    }

    /// Inscription en tâche de fond (au démarrage ou à l'activation de la
    /// recherche) : un échec n'est que noté, la recherche réessaiera.
    pub fn spawn_registration(self: &Arc<Self>, cancel: CancellationToken) {
        let this = self.clone();
        tokio::spawn(async move {
            if let Err(error) = this.ensure_registered(&cancel).await
                && error != RelayError::Cancelled
            {
                crate::app_log!("chatonlm: relay registration failed: {error}");
            }
        });
    }

    /// Inscrit l'installation si ce n'est pas déjà fait auprès de ce relais.
    pub async fn ensure_registered(&self, cancel: &CancellationToken) -> Result<(), RelayError> {
        let _guard = tokio::select! {
            guard = self.registering.lock() => guard,
            () = cancel.cancelled() => return Err(RelayError::Cancelled),
        };
        let identity = self.identity.load()?;
        if identity.is_registered_with(&self.base) {
            return Ok(());
        }
        let blocked_until = self.registration_blocked_until.load(Ordering::Relaxed);
        if self.now() < blocked_until {
            return Err(RelayError::RateLimited {
                retry_at: Some(blocked_until),
            });
        }
        let result = self.register(&identity, cancel).await;
        if let Err(RelayError::RateLimited { retry_at }) = &result {
            let until = retry_at.unwrap_or_else(|| self.now() + REGISTRATION_BACKOFF_SECS);
            self.registration_blocked_until
                .store(until, Ordering::Relaxed);
        }
        result
    }

    /// Défi, preuve, inscription signée.
    async fn register(
        &self,
        identity: &identity::Loaded,
        cancel: &CancellationToken,
    ) -> Result<(), RelayError> {
        let challenge: ChallengeResponse = self.unsigned_challenge(cancel).await?;
        let parsed =
            Challenge::parse(&challenge.challenge).map_err(|_| RelayError::Registration)?;
        let nonces = solve(parsed, identity.key.clone(), cancel).await?;
        let body = serde_json::to_vec(&RegisterRequest {
            challenge: challenge.challenge,
            nonces,
        })
        .map_err(|_| RelayError::Malformed)?;
        let reply: RegisterResponse = match self
            .signed(identity, "POST", PATH_REGISTER, &body, cancel)
            .await
        {
            Err(Failure::ClockSkew(server_time)) if self.adopt_server_time(server_time) => self
                .signed(identity, "POST", PATH_REGISTER, &body, cancel)
                .await
                .map_err(final_error)?,
            reply => reply.map_err(final_error)?,
        };
        let _ = reply.quota;
        self.identity.mark_registered(&self.base)?;
        Ok(())
    }

    /// Recherche web. `lang` : langue souhaitée des résultats (`fr`).
    pub async fn search(
        &self,
        query: &str,
        max_results: Option<u8>,
        lang: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<SearchResponse, RelayError> {
        let request = SearchRequest {
            query: query.to_owned(),
            max_results,
            lang: lang.map(str::to_owned),
            country: None,
        };
        request.validate().ok_or(RelayError::InvalidQuery)?;
        let body = serde_json::to_vec(&request).map_err(|_| RelayError::Malformed)?;
        self.with_retries("POST", PATH_SEARCH, &body, cancel).await
    }

    /// État du service et quota restant.
    pub async fn status(&self, cancel: &CancellationToken) -> Result<StatusResponse, RelayError> {
        self.with_retries("GET", PATH_STATUS, b"", cancel).await
    }

    /// Requête signée, avec au plus une correction d'horloge et une
    /// réinscription.
    async fn with_retries<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        cancel: &CancellationToken,
    ) -> Result<T, RelayError> {
        let mut clock_fixed = false;
        let mut reregistered = false;
        loop {
            self.ensure_registered(cancel).await?;
            let identity = self.identity.load()?;
            match self.signed(&identity, method, path, body, cancel).await {
                Ok(reply) => return Ok(reply),
                Err(Failure::ClockSkew(server_time))
                    if !clock_fixed && self.adopt_server_time(server_time) =>
                {
                    clock_fixed = true;
                }
                Err(Failure::UnknownKey) if !reregistered => {
                    reregistered = true;
                    self.identity.forget_registration(&self.base)?;
                }
                Err(failure) => return Err(final_error(failure)),
            }
        }
    }

    fn http(&self) -> Result<&reqwest::Client, RelayError> {
        self.http.as_ref().ok_or(RelayError::Offline)
    }

    async fn unsigned_challenge(
        &self,
        cancel: &CancellationToken,
    ) -> Result<ChallengeResponse, RelayError> {
        let request = self
            .http()?
            .post(format!("{}{PATH_CHALLENGE}", self.base))
            .header("content-type", "application/json")
            // i18n-ignore : corps JSON vide, pas un texte.
            .body("{}");
        send(request, cancel).await.map_err(final_error)
    }

    async fn signed<T: DeserializeOwned>(
        &self,
        identity: &identity::Loaded,
        method: &str,
        path: &str,
        body: &[u8],
        cancel: &CancellationToken,
    ) -> Result<T, Failure> {
        let headers = identity.key.sign_request(method, path, self.now(), body);
        let url = format!("{}{path}", self.base);
        let mut request = match method {
            "GET" => self.http()?.get(url),
            _ => self
                .http()?
                .post(url)
                .header("content-type", "application/json")
                .body(body.to_vec()),
        };
        for (name, value) in headers.pairs() {
            request = request.header(name, value);
        }
        send(request, cancel).await
    }
}

fn final_error(failure: Failure) -> RelayError {
    match failure {
        Failure::Error(error) => error,
        Failure::ClockSkew(_) => RelayError::Rejected(ErrorCode::ClockSkew),
        Failure::UnknownKey => RelayError::Rejected(ErrorCode::UnknownKey),
    }
}

/// Envoie, lit la réponse (bornée), la classe.
async fn send<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
    cancel: &CancellationToken,
) -> Result<T, Failure> {
    let exchange = async {
        let mut response = request.send().await.map_err(|_| RelayError::Offline)?;
        let status = response.status();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| RelayError::Offline)? {
            if body.len() + chunk.len() > MAX_REPLY {
                return Err(RelayError::Malformed);
            }
            body.extend_from_slice(&chunk);
        }
        Ok((status, body))
    };
    let (status, body) = tokio::select! {
        result = exchange => result?,
        () = cancel.cancelled() => return Err(RelayError::Cancelled.into()),
    };
    if status.is_success() {
        return serde_json::from_slice(&body).map_err(|_| RelayError::Malformed.into());
    }
    let Ok(error) = serde_json::from_slice::<ErrorBody>(&body) else {
        // Pas une réponse du relais (mandataire, page d'erreur de nginx). Un
        // 429 reste un refus de cadence (`limit_req` de nginx), pas une
        // réponse illisible.
        return Err(if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            RelayError::RateLimited { retry_at: None }
        } else if status.is_server_error() {
            RelayError::Offline
        } else {
            RelayError::Malformed
        }
        .into());
    };
    Err(match error.error.code {
        ErrorCode::ClockSkew => Failure::ClockSkew(error.error.server_time),
        ErrorCode::UnknownKey => Failure::UnknownKey,
        _ => Failure::Error(RelayError::from_reply(&error)),
    })
}

/// Preuve de travail dans une tâche bloquante, interrompue par `cancel`.
async fn solve(
    challenge: Challenge,
    key: Arc<Identity>,
    cancel: &CancellationToken,
) -> Result<Vec<u64>, RelayError> {
    let stop = Arc::new(AtomicBool::new(false));
    let task = tokio::task::spawn_blocking({
        let stop = stop.clone();
        move || pow::solve(&challenge, &key.public_key(), &stop, |_| {})
    });
    tokio::select! {
        result = task => result.ok().flatten().ok_or(RelayError::Registration),
        () = cancel.cancelled() => {
            stop.store(true, Ordering::Relaxed);
            Err(RelayError::Cancelled)
        }
    }
}
