//! État du relais et points d'accès (axum).
//!
//! Ordre des contrôles d'une requête signée, du moins cher au plus cher :
//! limite par réseau et par minute, version de l'app, taille du corps,
//! heure puis signature, bannissement, inscription, cadence par clé, rejeu
//! (seulement pour une clé inscrite : une clé neuve ne coûte rien à créer ;
//! et après la cadence, pour qu'une clé n'y mette que quelques centaines de
//! nonces) ; puis, pour une recherche : pause, requête valable, budget (mois
//! et jour), part du réseau dans l'enveloppe du jour, quota du jour de la clé,
//! quota du jour du réseau, et enfin les fournisseurs l'un après l'autre.
//!
//! Une requête abandonnée (client parti, nginx qui coupe) arrête son
//! gestionnaire où il en est : ce qui doit tenir jusqu'au bout tient sans lui
//! (le jeton d'une vérification de preuve vit dans sa tâche bloquante), et ce
//! qui n'a rien coûté est rendu (garde [`Held`] d'une recherche).
//!
//! Adresse du client : celle de la connexion, ou `X-Real-IP` quand la
//! connexion vient de 127.0.0.1 et que `trust_proxy` est vrai (nginx sur la
//! même machine). Tout processus local du serveur peut donc se dire n'importe
//! quelle adresse : les limites par réseau ne valent que contre l'extérieur.

use crate::budget::{Budget, Exhausted, Notice, Reservation};
use crate::config::{Config, Secrets};
use crate::limits::{IpKind, IpLimits, KeyCache, KeyQuotas, Replay, ReplayCache};
use crate::log::{Clock, Log};
use crate::providers::ProviderClient;
use crate::store::{KeyRecord, Store};
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use relay_proto::api::{
    ChallengeResponse, ErrorBody, ErrorCode, ErrorDetail, PATH_CHALLENGE, PATH_HEALTH,
    PATH_REGISTER, PATH_SEARCH, PATH_STATUS, Provider, Quota, RegisterRequest, RegisterResponse,
    SearchRequest, SearchResponse, ServiceState, StatusResponse,
};
use relay_proto::pow::{self, Challenge};
use relay_proto::sign::{self, PublicKey, SignatureError, SignedHeaders, Verified};
use relay_proto::time::{day_index, next_day_start};
use relay_proto::version::AppVersion;
use relay_proto::{CLOCK_TOLERANCE_SECS, MAX_REQUEST_BODY};
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

/// Mémoire des vérifications de preuve de travail menées en même temps : deux
/// à 32 Mio (défaut), une seule à 64 Mio (maximum permis par la
/// configuration). Au-delà, une inscription attend son tour…
const POW_MEMORY_KIB: u32 = 64 * 1024;
/// … quelques secondes au plus : `rate_limited` plutôt qu'une file sans fin.
const POW_RETRY_SECS: u64 = 10;
/// Recherches en cours chez les fournisseurs.
const SEARCH_SLOTS: usize = 32;
/// Attente d'une place au plus, sous le délai de l'app (30 s) et de nginx
/// (40 s) : au-delà, `rate_limited` (l'unité du jour est rendue) plutôt
/// qu'une réponse que personne n'attend plus.
const SEARCH_WAIT: Duration = Duration::from_secs(20);
/// Nouvel essai conseillé après une attente trop longue.
const SEARCH_RETRY_SECS: u64 = 30;

pub struct Relay {
    config: Config,
    clock: Clock,
    log: Log,
    challenge_key: [u8; 32],
    store: Arc<Mutex<Store>>,
    /// Clés servies récemment ; la base garde les autres (mémoire bornée).
    keys: Mutex<KeyCache>,
    banned: HashSet<PublicKey>,
    limits: Mutex<IpLimits>,
    quotas: Mutex<KeyQuotas>,
    replay: Mutex<ReplayCache>,
    budget: Arc<Mutex<Budget>>,
    providers: Vec<ProviderClient>,
    /// Partagé avec les tâches bloquantes : le jeton d'une vérification est
    /// rendu à la fin du hachage, même si la requête a été abandonnée.
    pow_slots: Arc<Semaphore>,
    search_slots: Semaphore,
    search_wait: Duration,
    min_version: Option<AppVersion>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // Un essai qui panique ne doit pas empoisonner tout le relais.
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Relay {
    /// Ouvre la base, oublie les clés périmées, recharge le budget du mois
    /// (les clés sont lues une à une, au besoin).
    pub fn new(
        config: Config,
        secrets: Secrets,
        clock: Clock,
        log: Log,
    ) -> Result<Arc<Self>, String> {
        config.validate()?;
        crate::install_crypto_provider();
        let store = Store::open(&config.database).map_err(|error| format!("database: {error}"))?;
        let now = clock.now();
        let purged = store
            .purge_keys(day_index(now).saturating_sub(config.key_retention_days))
            .map_err(|error| format!("database: {error}"))?;
        let registered = store
            .key_count()
            .map_err(|error| format!("database: {error}"))?;
        let month = relay_proto::time::month_key(now);
        let record = store
            .month(&month)
            .map_err(|error| format!("database: {error}"))?;
        let today = day_index(now);
        let day_spent = store
            .day_spent(today)
            .map_err(|error| format!("database: {error}"))?;
        let budget = Budget::new(
            config.budget.monthly_limit_eur,
            config.budget.alert_ratio,
            config.budget.daily_share,
            month,
            record,
            today,
            day_spent,
        );
        let mut providers = Vec::new();
        for name in &config.providers.order {
            let (provider, settings, key) = match name.as_str() {
                "brave" => (
                    Provider::Brave,
                    &config.providers.brave,
                    secrets.brave_api_key.clone(),
                ),
                _ => (
                    Provider::Mojeek,
                    &config.providers.mojeek,
                    secrets.mojeek_api_key.clone(),
                ),
            };
            let client = ProviderClient::new(provider, settings, key)?;
            if client.is_configured() {
                providers.push(client);
            } else {
                log.warn(&format!(
                    "provider {}: no API key, skipped",
                    provider.as_str()
                ));
            }
        }
        if providers.is_empty() {
            log.warn("no search provider configured: every search will fail");
        }
        let challenge_key = secrets
            .challenge_key_bytes()
            .unwrap_or_else(relay_proto::random_bytes);
        let banned = config
            .banned_keys
            .iter()
            .filter_map(|key| PublicKey::from_b64(key))
            .collect();
        let min_version = config
            .min_app_version
            .as_deref()
            .and_then(AppVersion::parse);
        log.info(&format!(
            "chatonlm-relay {}: {} registered keys ({} forgotten), providers: {}",
            env!("CARGO_PKG_VERSION"),
            registered,
            purged,
            providers
                .iter()
                .map(|p| p.provider.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        ));
        let pow_slots = usize::try_from(POW_MEMORY_KIB / config.pow.memory_kib.max(1))
            .unwrap_or(1)
            .clamp(1, 2);
        Ok(Arc::new(Self {
            limits: Mutex::new(IpLimits::new(&config.tables)),
            quotas: Mutex::new(KeyQuotas::new(&config.tables)),
            replay: Mutex::new(ReplayCache::new(config.tables.replay)),
            pow_slots: Arc::new(Semaphore::new(pow_slots)),
            keys: Mutex::new(KeyCache::new(config.tables.registered_keys)),
            config,
            clock,
            log,
            challenge_key,
            store: Arc::new(Mutex::new(store)),
            banned,
            budget: Arc::new(Mutex::new(budget)),
            providers,
            search_slots: Semaphore::new(SEARCH_SLOTS),
            search_wait: SEARCH_WAIT,
            min_version,
        }))
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Nonces retenus contre le rejeu (essais).
    pub fn replay_len(&self) -> usize {
        lock(&self.replay).len()
    }

    /// Dépense du mois en cours, en micro-euros (essais).
    pub fn spent_this_month(&self) -> u64 {
        lock(&self.budget).record().spent_micro_eur
    }

    /// Clés inscrites gardées en mémoire (essais).
    pub fn keys_in_memory(&self) -> usize {
        lock(&self.keys).len()
    }

    /// Entrées des tables par réseau : minute, jour, défis (essais).
    pub fn network_tables(&self) -> (usize, usize, usize) {
        lock(&self.limits).lens()
    }

    /// Occupe toutes les vérifications de preuve (essais : inscriptions en
    /// attente de leur tour).
    #[cfg(feature = "fake")]
    pub fn occupy_pow_slots(&self) -> Vec<tokio::sync::SemaphorePermit<'_>> {
        std::iter::from_fn(|| self.pow_slots.try_acquire().ok()).collect()
    }

    /// Ménage quotidien : clés oubliées après la durée de rétention.
    pub async fn purge(self: &Arc<Self>) {
        let before = day_index(self.clock.now()).saturating_sub(self.config.key_retention_days);
        let store = self.store.clone();
        let purged = tokio::task::spawn_blocking(move || lock(&store).purge_keys(before)).await;
        lock(&self.keys).retain(|record| record.last_seen_day >= before);
        if let Ok(Ok(count)) = purged
            && count > 0
        {
            self.log.info(&format!("{count} unused keys forgotten"));
        }
    }

    /// Écrit le budget du mois tel qu'il est au moment de l'écriture (la
    /// dernière écriture porte toujours la dernière valeur).
    async fn save_budget(&self) {
        let store = self.store.clone();
        let budget = self.budget.clone();
        let saved = tokio::task::spawn_blocking(move || {
            let store = lock(&store);
            let (month, record, (day, day_spent)) = {
                let budget = lock(&budget);
                (budget.month().to_owned(), *budget.record(), budget.day())
            };
            store.save_month(&month, &record)?;
            store.save_day(day, day_spent)
        })
        .await;
        if !matches!(saved, Ok(Ok(()))) {
            self.log.error("budget: cannot be saved to the database");
        }
    }

    /// Fiche d'une clé inscrite : en mémoire, sinon dans la base (gardée
    /// ensuite en mémoire). `Err` : base illisible.
    async fn find_key(&self, key: &PublicKey) -> Result<Option<KeyRecord>, ApiError> {
        if let Some(record) = lock(&self.keys).get(key) {
            return Ok(Some(record));
        }
        let store = self.store.clone();
        let wanted = *key;
        let found = tokio::task::spawn_blocking(move || lock(&store).key(&wanted)).await;
        let Ok(Ok(found)) = found else {
            self.log.error("keys: cannot be read from the database");
            return Err(ApiError::new(ErrorCode::Internal));
        };
        if let Some(record) = found {
            lock(&self.keys).insert(*key, record);
        }
        Ok(found)
    }

    fn daily_limit(&self, record: &KeyRecord, now: u64) -> u32 {
        if record.created_day == day_index(now) {
            self.config.quota.new_key_daily
        } else {
            self.config.quota.daily_per_key
        }
    }

    fn quota(&self, key: &PublicKey, record: &KeyRecord, now: u64) -> Quota {
        let limit = self.daily_limit(record, now);
        let used = lock(&self.quotas).used(key, now);
        Quota {
            daily_limit: limit,
            remaining_today: limit.saturating_sub(used),
            resets_at: relay_proto::time::next_day_start(now),
        }
    }

    fn cheapest_cost(&self) -> u64 {
        self.providers.iter().map(|p| p.cost).min().unwrap_or(0)
    }

    /// Dépense du jour permise à un réseau, à chaque niveau : sa part de
    /// l'enveloppe du jour.
    fn network_caps(&self, now: u64) -> [u64; crate::limits::LEVELS] {
        let envelope = lock(&self.budget).day_limit(now);
        self.config.budget.network_share.of(envelope)
    }

    fn service_state(&self, now: u64) -> ServiceState {
        if self.config.paused {
            ServiceState::Paused
        } else if !self.providers.is_empty()
            && lock(&self.budget)
                .exhausted(now, self.cheapest_cost())
                .is_some()
        {
            ServiceState::BudgetExhausted
        } else {
            ServiceState::Ok
        }
    }
}

/// Réponse d'erreur à code stable.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    detail: ErrorDetail,
}

impl ApiError {
    fn new(code: ErrorCode) -> Self {
        let status = match code {
            ErrorCode::BadRequest | ErrorCode::ChallengeInvalid | ErrorCode::PowInvalid => {
                StatusCode::BAD_REQUEST
            }
            ErrorCode::BadSignature
            | ErrorCode::ClockSkew
            | ErrorCode::Replay
            | ErrorCode::UnknownKey => StatusCode::UNAUTHORIZED,
            ErrorCode::Banned => StatusCode::FORBIDDEN,
            ErrorCode::QuotaExceeded | ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::Outdated => StatusCode::UPGRADE_REQUIRED,
            ErrorCode::BudgetExhausted | ErrorCode::Paused => StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::ProviderUnavailable => StatusCode::BAD_GATEWAY,
            ErrorCode::Internal | ErrorCode::Unknown => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            detail: ErrorDetail {
                code,
                retry_at: None,
                server_time: None,
            },
        }
    }

    fn retry_at(mut self, at: u64) -> Self {
        self.detail.retry_at = Some(at);
        self
    }

    fn server_time(mut self, now: u64) -> Self {
        self.detail.server_time = Some(now);
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let retry_at = self.detail.retry_at;
        let mut response = (self.status, Json(ErrorBody { error: self.detail })).into_response();
        if let Some(at) = retry_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default();
            if let Ok(value) = HeaderValue::from_str(&at.saturating_sub(now).to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
        response
    }
}

type Shared = State<Arc<Relay>>;
type Peer = ConnectInfo<SocketAddr>;

pub fn router(relay: Arc<Relay>) -> Router {
    Router::new()
        .route(PATH_HEALTH, get(health))
        .route(PATH_CHALLENGE, post(challenge))
        .route(PATH_REGISTER, post(register))
        .route(PATH_SEARCH, post(search))
        .route(PATH_STATUS, get(status))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY))
        .layer(axum::middleware::map_response(no_store))
        .with_state(relay)
}

/// Aucune réponse n'est gardée par un intermédiaire.
async fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Sert jusqu'à `shutdown`, avec le ménage quotidien.
pub async fn serve(
    relay: Arc<Relay>,
    listener: tokio::net::TcpListener,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let janitor = {
        let relay = relay.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(3_600));
            loop {
                tick.tick().await;
                relay.purge().await;
            }
        })
    };
    let app = router(relay).into_make_service_with_connect_info::<SocketAddr>();
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await;
    janitor.abort();
    served
}

/// Adresse du client : celle de la connexion, ou `X-Real-IP` posé par nginx
/// quand la connexion vient de 127.0.0.1 (pas d'une autre adresse de boucle :
/// nginx parle au relais par 127.0.0.1, voir `nginx-relay.chatonlm.com.conf`).
fn client_ip(relay: &Relay, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
    forwarded_ip(relay.config.trust_proxy, peer, headers)
}

fn forwarded_ip(trust_proxy: bool, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
    if trust_proxy
        && peer.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST)
        && let Some(ip) = headers
            .get("x-real-ip")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<IpAddr>().ok())
    {
        return ip;
    }
    peer.ip()
}

/// Contrôles communs à toute requête : cadence du réseau, version de l'app.
fn admit(relay: &Relay, ip: IpAddr, headers: &HeaderMap, now: u64) -> Result<(), ApiError> {
    lock(&relay.limits)
        .hit_minute(ip, now, &relay.config.quota.per_ip_per_minute)
        .map_err(|at| ApiError::new(ErrorCode::RateLimited).retry_at(at))?;
    if let Some(min) = relay.min_version {
        let version = headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .and_then(AppVersion::from_user_agent);
        if version.is_none_or(|version| version < min) {
            return Err(ApiError::new(ErrorCode::Outdated));
        }
    }
    Ok(())
}

fn signed_headers(headers: &HeaderMap) -> SignedHeaders {
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    SignedHeaders {
        key: get(relay_proto::HEADER_KEY),
        time: get(relay_proto::HEADER_TIME),
        nonce: get(relay_proto::HEADER_NONCE),
        signature: get(relay_proto::HEADER_SIGNATURE),
    }
}

/// Vérifie la signature ; `registered` : la clé doit être inscrite (sa fiche
/// est rendue, et le nonce est alors retenu contre le rejeu).
async fn authenticate(
    relay: &Arc<Relay>,
    method: &str,
    path: &str,
    headers: &HeaderMap,
    body: &[u8],
    now: u64,
    registered: bool,
) -> Result<(Verified, Option<KeyRecord>), ApiError> {
    let verified = sign::verify_request(method, path, &signed_headers(headers), body, now)
        .map_err(|error| match error {
            SignatureError::ClockSkew => ApiError::new(ErrorCode::ClockSkew).server_time(now),
            SignatureError::Missing | SignatureError::Malformed | SignatureError::BadSignature => {
                ApiError::new(ErrorCode::BadSignature)
            }
        })?;
    if relay.banned.contains(&verified.key) {
        return Err(ApiError::new(ErrorCode::Banned));
    }
    if !registered {
        return Ok((verified, None));
    }
    let mut record = relay
        .find_key(&verified.key)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::UnknownKey))?;
    let today = day_index(now);
    let touch = record.last_seen_day != today;
    if touch {
        record.last_seen_day = today;
        lock(&relay.keys).insert(verified.key, record);
    }
    // Cadence d'abord : une requête refusée ne laisse pas son nonce dans la
    // table (une clé y met au plus `per_key_per_minute` nonces par minute).
    lock(&relay.quotas)
        .hit_minute(&verified.key, now, relay.config.quota.per_key_per_minute)
        .map_err(|at| ApiError::new(ErrorCode::RateLimited).retry_at(at))?;
    if lock(&relay.replay).check(&verified.key, verified.nonce, verified.time, now) == Replay::Seen
    {
        return Err(ApiError::new(ErrorCode::Replay));
    }
    if touch {
        // Dernier usage, au jour près : une écriture par clé et par jour.
        let store = relay.store.clone();
        let key = verified.key;
        let day = day_index(now);
        tokio::task::spawn_blocking(move || {
            let _ = lock(&store).touch_key(&key, day);
        });
    }
    Ok((verified, Some(record)))
}

fn body_of(body: Result<Bytes, BytesRejection>) -> Result<Bytes, ApiError> {
    body.map_err(|_| ApiError::new(ErrorCode::BadRequest))
}

fn json_of<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|_| ApiError::new(ErrorCode::BadRequest))
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn challenge(
    State(relay): Shared,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
) -> Result<Json<ChallengeResponse>, ApiError> {
    let now = relay.clock.now();
    let ip = client_ip(&relay, peer, &headers);
    admit(&relay, ip, &headers, now)?;
    {
        let mut limits = lock(&relay.limits);
        // Plus d'inscription possible depuis ce réseau aujourd'hui : refusé dès
        // le défi, avant que l'app calcule sa preuve pour rien.
        limits
            .can_take_daily(
                ip,
                IpKind::Register,
                now,
                &relay.config.quota.per_ip_daily_registrations,
            )
            .and_then(|()| {
                limits.take_challenge(ip, now, &relay.config.quota.per_ip_daily_challenges)
            })
            .map_err(|at| ApiError::new(ErrorCode::RateLimited).retry_at(at))?;
    }
    let params = relay.config.pow.params();
    let (_, token) = Challenge::issue(&relay.challenge_key, now, params);
    Ok(Json(ChallengeResponse {
        challenge: token,
        params,
        expires_at: now + relay.config.pow.challenge_ttl_secs,
    }))
}

async fn register(
    State(relay): Shared,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<RegisterResponse>, ApiError> {
    let now = relay.clock.now();
    let ip = client_ip(&relay, peer, &headers);
    admit(&relay, ip, &headers, now)?;
    let body = body_of(body)?;
    let (verified, _) =
        authenticate(&relay, "POST", PATH_REGISTER, &headers, &body, now, false).await?;
    let request: RegisterRequest = json_of(&body)?;
    if let Some(record) = relay.find_key(&verified.key).await? {
        // Déjà inscrite (réponse perdue, nouvel essai) : rien à refaire.
        return Ok(Json(RegisterResponse {
            quota: relay.quota(&verified.key, &record, now),
        }));
    }
    let challenge = Challenge::open(&request.challenge, &relay.challenge_key)
        .map_err(|_| ApiError::new(ErrorCode::ChallengeInvalid))?;
    let fresh = challenge.issued_at <= now + CLOCK_TOLERANCE_SECS
        && now
            <= challenge
                .issued_at
                .saturating_add(relay.config.pow.challenge_ttl_secs);
    if !fresh || challenge.params != relay.config.pow.params() {
        return Err(ApiError::new(ErrorCode::ChallengeInvalid));
    }
    lock(&relay.limits)
        .take_daily(
            ip,
            IpKind::Register,
            now,
            &relay.config.quota.per_ip_daily_registrations,
        )
        .map_err(|at| ApiError::new(ErrorCode::RateLimited).retry_at(at))?;
    let valid = {
        // Pas de file d'attente : vérifications toutes prises, l'app revient
        // dans quelques secondes (son unité d'inscription lui est rendue).
        let Ok(slot) = relay.pow_slots.clone().try_acquire_owned() else {
            lock(&relay.limits).give_back_daily(ip, IpKind::Register, now);
            return Err(ApiError::new(ErrorCode::RateLimited).retry_at(now + POW_RETRY_SECS));
        };
        let key = verified.key;
        let nonces = request.nonces;
        // Le jeton part avec le hachage : une requête abandonnée (client
        // parti) ne le rend pas avant la fin du calcul, qui continue sans
        // elle. Sinon, des inscriptions coupées aussitôt envoyées feraient
        // tourner des dizaines de hachages de 32 Mio en même temps.
        tokio::task::spawn_blocking(move || {
            let _slot = slot;
            pow::verify(&challenge, &key, &nonces)
        })
        .await
        .unwrap_or(false)
    };
    if !valid {
        return Err(ApiError::new(ErrorCode::PowInvalid));
    }
    let day = day_index(now);
    let store = relay.store.clone();
    let key = verified.key;
    let saved = tokio::task::spawn_blocking(move || lock(&store).insert_key(&key, day)).await;
    if !matches!(saved, Ok(Ok(()))) {
        relay
            .log
            .error("registration: cannot be saved to the database");
        return Err(ApiError::new(ErrorCode::Internal));
    }
    let record = KeyRecord {
        created_day: day,
        last_seen_day: day,
    };
    lock(&relay.keys).insert(verified.key, record);
    Ok(Json(RegisterResponse {
        quota: relay.quota(&verified.key, &record, now),
    }))
}

async fn status(
    State(relay): Shared,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
) -> Result<Json<StatusResponse>, ApiError> {
    let now = relay.clock.now();
    let ip = client_ip(&relay, peer, &headers);
    admit(&relay, ip, &headers, now)?;
    let (verified, record) =
        authenticate(&relay, "GET", PATH_STATUS, &headers, b"", now, true).await?;
    let record = record.ok_or_else(|| ApiError::new(ErrorCode::UnknownKey))?;
    Ok(Json(StatusResponse {
        service: relay.service_state(now),
        quota: relay.quota(&verified.key, &record, now),
        min_app_version: relay.config.min_app_version.clone(),
        server_time: now,
    }))
}

/// Unités du jour prises par une recherche (quota de la clé, unité du
/// réseau) : rendues si la recherche s'arrête sans avoir rien coûté, même
/// abandonnée en route (client parti pendant l'attente d'une place). Gardées
/// pendant l'appel à un fournisseur (il peut facturer) et après un succès.
struct Held<'a> {
    relay: &'a Relay,
    key: PublicKey,
    ip: IpAddr,
    keep: bool,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        let now = self.relay.clock.now();
        lock(&self.relay.quotas).release(&self.key, now);
        lock(&self.relay.limits).give_back_daily(self.ip, IpKind::Search, now);
    }
}

async fn search(
    State(relay): Shared,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<SearchResponse>, ApiError> {
    let now = relay.clock.now();
    let ip = client_ip(&relay, peer, &headers);
    admit(&relay, ip, &headers, now)?;
    let body = body_of(body)?;
    let (verified, record) =
        authenticate(&relay, "POST", PATH_SEARCH, &headers, &body, now, true).await?;
    let record = record.ok_or_else(|| ApiError::new(ErrorCode::UnknownKey))?;
    if relay.config.paused {
        return Err(ApiError::new(ErrorCode::Paused));
    }
    let request: SearchRequest = json_of(&body)?;
    let search = request
        .validate()
        .ok_or_else(|| ApiError::new(ErrorCode::BadRequest))?;
    let key = verified.key;
    if relay.providers.is_empty() {
        return Err(ApiError::new(ErrorCode::ProviderUnavailable));
    }
    let exhausted = lock(&relay.budget).exhausted(now, relay.cheapest_cost());
    if let Some(exhausted) = exhausted {
        return Err(budget_exhausted(&relay, exhausted));
    }
    // Ce réseau a dépensé sa part de l'enveloppe du jour : lui seul attend.
    let caps = relay.network_caps(now);
    lock(&relay.limits)
        .spend(ip, now, relay.cheapest_cost(), caps, false)
        .map_err(|at| ApiError::new(ErrorCode::RateLimited).retry_at(at))?;
    let limit = relay.daily_limit(&record, now);
    lock(&relay.quotas)
        .reserve(&key, now, limit)
        .map_err(|at| ApiError::new(ErrorCode::QuotaExceeded).retry_at(at))?;
    if let Err(at) = lock(&relay.limits).take_daily(
        ip,
        IpKind::Search,
        now,
        &relay.config.quota.per_ip_daily_searches,
    ) {
        lock(&relay.quotas).release(&key, now);
        return Err(ApiError::new(ErrorCode::RateLimited).retry_at(at));
    }
    let mut held = Held {
        relay: &relay,
        key,
        ip,
        keep: false,
    };
    let _slot = match tokio::time::timeout(relay.search_wait, relay.search_slots.acquire()).await {
        Ok(Ok(slot)) => slot,
        Ok(Err(_)) => return Err(ApiError::new(ErrorCode::Internal)),
        Err(_) => {
            return Err(ApiError::new(ErrorCode::RateLimited)
                .retry_at(relay.clock.now() + SEARCH_RETRY_SECS));
        }
    };
    let mut over_budget = false;
    let mut over_network = false;
    for provider in &relay.providers {
        let now = relay.clock.now();
        if provider.is_down(now) {
            continue;
        }
        let caps = relay.network_caps(now);
        let day = day_index(now);
        if lock(&relay.limits)
            .spend(ip, now, provider.cost, caps, true)
            .is_err()
        {
            over_network = true;
            continue;
        }
        let reservation: Reservation = match lock(&relay.budget).reserve(now, provider.cost) {
            Ok((reservation, notice)) => {
                if notice == Notice::Alert {
                    relay.log.warn(&format!(
                        "budget: {:.0}% of the monthly cap reached",
                        relay.config.budget.alert_ratio * 100.0
                    ));
                }
                reservation
            }
            Err(_) => {
                lock(&relay.limits).refund_spend(ip, now, day, provider.cost);
                over_budget = true;
                continue;
            }
        };
        // La requête part : abandonnée pendant l'appel, elle garde ses unités
        // (le fournisseur a pu la facturer).
        held.keep = true;
        match provider.search(&search).await {
            Ok(results) => {
                lock(&relay.budget).count(relay.clock.now(), provider.provider);
                relay.save_budget().await;
                return Ok(Json(SearchResponse {
                    results,
                    provider: provider.provider,
                    quota: relay.quota(&key, &record, relay.clock.now()),
                }));
            }
            Err(error) => {
                held.keep = false;
                let now = relay.clock.now();
                if !error.billed() {
                    lock(&relay.budget).refund(now, reservation);
                    lock(&relay.limits).refund_spend(ip, now, day, provider.cost);
                }
                provider.mark_failure(error, now);
                relay.save_budget().await;
                relay.log.warn(&format!(
                    "provider {}: {}",
                    provider.provider.as_str(),
                    error.as_str()
                ));
            }
        }
    }
    // Aucun fournisseur n'a répondu : les unités du jour sont rendues.
    drop(held);
    let now = relay.clock.now();
    if over_budget {
        let exhausted = lock(&relay.budget).exhausted(now, relay.cheapest_cost());
        if let Some(exhausted) = exhausted {
            return Err(budget_exhausted(&relay, exhausted));
        }
    }
    if over_network {
        return Err(ApiError::new(ErrorCode::RateLimited).retry_at(next_day_start(now)));
    }
    Err(ApiError::new(ErrorCode::ProviderUnavailable))
}

/// `budget_exhausted` jusqu'à la reprise (1er du mois, ou minuit pour
/// l'enveloppe du jour) ; une ligne au journal par période.
fn budget_exhausted(relay: &Relay, exhausted: Exhausted) -> ApiError {
    if lock(&relay.budget).take_exhausted_notice(exhausted) {
        relay.log.warn(match exhausted {
            Exhausted::Month(_) => "budget: monthly cap reached, search paused until next month",
            Exhausted::Day(_) => "budget: daily envelope reached, search paused until midnight UTC",
        });
    }
    ApiError::new(ErrorCode::BudgetExhausted).retry_at(exhausted.retry_at())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MAX_SERVER_POW_MEMORY_KIB, Secret, Tiers};
    use relay_proto::sign::Identity;
    use std::future::Future;

    /// 8 nov. 2026, 12 h UTC.
    const T: u64 = 1_794_139_200;

    /// Relais à l'heure `T`, Brave seul (jamais appelé ici).
    fn relay(dir: &std::path::Path, tweak: impl FnOnce(&mut Config)) -> Arc<Relay> {
        let database = dir.join("relay.sqlite");
        let mut config: Config =
            toml::from_str(&format!("database = {:?}", database.to_string_lossy())).unwrap();
        config.providers.brave.base_url = "http://127.0.0.1:9".into();
        tweak(&mut config);
        let secrets = Secrets {
            brave_api_key: Secret::new("BSA-essai"),
            ..Secrets::default()
        };
        let (clock, _) = Clock::manual(T);
        let (log, _) = Log::capturing();
        Relay::new(config, secrets, clock, log).unwrap()
    }

    fn peer() -> SocketAddr {
        "127.0.0.1:4000".parse().unwrap()
    }

    fn signed(id: &Identity, method: &str, path: &str, body: &[u8]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in id.sign_request(method, path, T, body).pairs() {
            headers.insert(name, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    /// Clé inscrite la veille (dans la base seulement).
    fn registered(relay: &Relay) -> Identity {
        let id = Identity::generate();
        lock(&relay.store)
            .insert_key(&id.public_key(), day_index(T) - 1)
            .unwrap();
        id
    }

    /// Fait avancer `request` jusqu'à ce que `reached` soit vrai (sans qu'il
    /// se termine), puis l'abandonne, comme hyper quand le client coupe.
    async fn abandon_once<F: Future + Unpin>(mut request: F, reached: impl Fn() -> bool) {
        while !reached() {
            tokio::select! {
                _ = &mut request => panic!("requête finie avant d'être abandonnée"),
                () = tokio::time::sleep(Duration::from_millis(1)) => {}
            }
        }
        drop(request);
    }

    /// Constat de relecture (haute) : un client qui coupe pendant la
    /// vérification de sa preuve ne rend pas le jeton avant la fin du hachage
    /// (sinon, des inscriptions coupées aussitôt envoyées feraient tourner des
    /// hachages de 32 Mio sans limite, jusqu'à la mise à mort du relais).
    #[tokio::test]
    async fn an_abandoned_registration_keeps_its_proof_slot_until_the_hash_ends() {
        let dir = tempfile::tempdir().unwrap();
        // Un hachage lent (64 Mio, 4 passes : une centaine de ms), une place.
        let relay = relay(dir.path(), |config| {
            config.pow.memory_kib = MAX_SERVER_POW_MEMORY_KIB;
            config.pow.iterations = 4;
            config.pow.bits = 0;
            config.pow.count = 1;
        });
        assert_eq!(relay.pow_slots.available_permits(), 1);
        let id = Identity::generate();
        let (_, token) = Challenge::issue(&relay.challenge_key, T, relay.config.pow.params());
        let body = serde_json::to_vec(&RegisterRequest {
            challenge: token,
            nonces: vec![0],
        })
        .unwrap();
        let request = Box::pin(register(
            State(relay.clone()),
            ConnectInfo(peer()),
            signed(&id, "POST", PATH_REGISTER, &body),
            Ok(Bytes::from(body)),
        ));
        abandon_once(request, || relay.pow_slots.available_permits() == 0).await;
        assert_eq!(
            relay.pow_slots.available_permits(),
            0,
            "jeton rendu avant la fin du hachage"
        );
        // Rendu à la fin du calcul.
        let freed = async {
            while relay.pow_slots.available_permits() == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(60), freed)
            .await
            .expect("jeton jamais rendu");
    }

    /// Constat de relecture (basse) : une recherche abandonnée avant d'avoir
    /// rien coûté (client parti pendant l'attente d'une place) rend le quota
    /// de la clé et l'unité du réseau.
    #[tokio::test]
    async fn an_abandoned_search_gives_its_units_back() {
        let dir = tempfile::tempdir().unwrap();
        let relay = relay(dir.path(), |_| {});
        let id = registered(&relay);
        let key = id.public_key();
        let busy = relay
            .search_slots
            .try_acquire_many(SEARCH_SLOTS as u32)
            .unwrap();
        let body = serde_json::to_vec(&serde_json::json!({ "query": "x" })).unwrap();
        let request = Box::pin(search(
            State(relay.clone()),
            ConnectInfo(peer()),
            signed(&id, "POST", PATH_SEARCH, &body),
            Ok(Bytes::from(body)),
        ));
        abandon_once(request, || lock(&relay.quotas).used(&key, T) == 1).await;
        assert_eq!(lock(&relay.quotas).used(&key, T), 0, "quota de la clé");
        let one = Tiers([1, 1, 1]);
        assert!(
            lock(&relay.limits)
                .can_take_daily(peer().ip(), IpKind::Search, T, &one)
                .is_ok(),
            "unité du réseau"
        );
        drop(busy);
    }

    /// Attente d'une place bornée : `rate_limited`, et rien n'est gardé.
    #[tokio::test]
    async fn a_search_waits_for_a_slot_a_bounded_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut relay = relay(dir.path(), |_| {});
        Arc::get_mut(&mut relay).unwrap().search_wait = Duration::from_millis(50);
        let id = registered(&relay);
        let busy = relay
            .search_slots
            .try_acquire_many(SEARCH_SLOTS as u32)
            .unwrap();
        let body = serde_json::to_vec(&serde_json::json!({ "query": "x" })).unwrap();
        let request = search(
            State(relay.clone()),
            ConnectInfo(peer()),
            signed(&id, "POST", PATH_SEARCH, &body),
            Ok(Bytes::from(body)),
        );
        let reply = tokio::time::timeout(Duration::from_secs(10), request)
            .await
            .expect("attente sans fin");
        let Err(error) = reply else {
            panic!("aucune place, aucune réponse attendue");
        };
        assert_eq!(error.detail.code, ErrorCode::RateLimited);
        assert_eq!(error.detail.retry_at, Some(T + SEARCH_RETRY_SECS));
        assert_eq!(lock(&relay.quotas).used(&id.public_key(), T), 0);
        drop(busy);
    }

    /// M8 : `X-Real-IP` n'est cru que de 127.0.0.1, et seulement avec
    /// `trust_proxy`.
    #[test]
    fn forwarded_address_is_trusted_from_127_0_0_1_only() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.9"));
        let forwarded: IpAddr = "203.0.113.9".parse().unwrap();
        let peer = |text: &str| -> SocketAddr { text.parse().unwrap() };
        assert_eq!(
            forwarded_ip(true, peer("127.0.0.1:5000"), &headers),
            forwarded
        );
        for (trust, from) in [
            (false, "127.0.0.1:5000"),
            (true, "[::1]:5000"),
            (true, "127.0.0.2:5000"),
            (true, "198.51.100.4:5000"),
        ] {
            assert_eq!(
                forwarded_ip(trust, peer(from), &headers),
                peer(from).ip(),
                "{trust} {from}"
            );
        }
        // En-tête illisible : l'adresse de la connexion.
        headers.insert("x-real-ip", HeaderValue::from_static("pas une adresse"));
        assert_eq!(
            forwarded_ip(true, peer("127.0.0.1:5000"), &headers),
            peer("127.0.0.1:5000").ip()
        );
    }
}
