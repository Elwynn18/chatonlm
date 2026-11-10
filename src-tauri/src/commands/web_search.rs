//! Recherche web par le relais de ChatonLM : état du service et quota du
//! jour (bouton de la recherche web), inscription en tâche de fond quand la
//! recherche est activée.
//!
//! Recherche coupée : rien n'est demandé au relais (aucune connexion tant que
//! l'utilisateur ne l'a pas choisie).

use relay_proto::api::{ServiceState, StatusResponse};
use relay_proto::version::AppVersion;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::error::AppResult;
use crate::llm::Access;
use crate::relay::RelayError;

/// État de la recherche web pour la page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WebSearchState {
    /// Recherche coupée : le relais n'a pas été interrogé.
    Off,
    Ok,
    /// Quota du jour épuisé, ou trop de recherches pour le moment.
    Limit,
    /// Budget du mois (ou du jour) épuisé.
    BudgetExhausted,
    /// Service en pause.
    Paused,
    /// Version de l'app trop ancienne pour le relais.
    Outdated,
    /// Relais injoignable, ou autre refus.
    Unavailable,
}

/// État et quota (`web_search_status`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WebSearchStatus {
    pub state: WebSearchState,
    /// Recherches permises par jour.
    pub daily_limit: Option<u32>,
    pub remaining_today: Option<u32>,
    /// Prochaine remise à zéro du quota (ms depuis 1970).
    pub resets_at: Option<i64>,
    /// Reprise annoncée d'un service limité ou en pause (ms depuis 1970).
    pub retry_at: Option<i64>,
}

impl WebSearchStatus {
    fn bare(state: WebSearchState) -> Self {
        WebSearchStatus {
            state,
            daily_limit: None,
            remaining_today: None,
            resets_at: None,
            retry_at: None,
        }
    }
}

fn ms(seconds: u64) -> i64 {
    i64::try_from(seconds.saturating_mul(1000)).unwrap_or(i64::MAX)
}

/// État d'après la réponse de `/v1/status` (`version` : celle de l'app).
pub fn from_reply(reply: &StatusResponse, version: &str) -> WebSearchStatus {
    let outdated = reply
        .min_app_version
        .as_deref()
        .and_then(AppVersion::parse)
        .zip(AppVersion::parse(version))
        .is_some_and(|(minimum, ours)| ours < minimum);
    let state = if outdated {
        WebSearchState::Outdated
    } else {
        match reply.service {
            ServiceState::Paused => WebSearchState::Paused,
            ServiceState::BudgetExhausted => WebSearchState::BudgetExhausted,
            ServiceState::Ok if reply.quota.remaining_today == 0 => WebSearchState::Limit,
            ServiceState::Ok => WebSearchState::Ok,
        }
    };
    WebSearchStatus {
        state,
        daily_limit: Some(reply.quota.daily_limit),
        remaining_today: Some(reply.quota.remaining_today),
        resets_at: Some(ms(reply.quota.resets_at)),
        retry_at: None,
    }
}

/// État d'après une erreur du relais.
pub fn from_error(error: &RelayError) -> WebSearchStatus {
    let (state, retry_at) = match error {
        RelayError::QuotaExceeded { retry_at } | RelayError::RateLimited { retry_at } => {
            (WebSearchState::Limit, *retry_at)
        }
        RelayError::BudgetExhausted { retry_at } => (WebSearchState::BudgetExhausted, *retry_at),
        RelayError::Paused => (WebSearchState::Paused, None),
        RelayError::Outdated => (WebSearchState::Outdated, None),
        _ => (WebSearchState::Unavailable, None),
    };
    WebSearchStatus {
        retry_at: retry_at.map(ms),
        ..WebSearchStatus::bare(state)
    }
}

/// État de la recherche web et quota du jour.
#[tauri::command]
pub async fn web_search_status(access: State<'_, Access>) -> AppResult<WebSearchStatus> {
    status(&access).await
}

pub async fn status(access: &Access) -> AppResult<WebSearchStatus> {
    if !access.prefs().await?.web_search {
        return Ok(WebSearchStatus::bare(WebSearchState::Off));
    }
    Ok(
        match access.relay().status(&CancellationToken::new()).await {
            Ok(reply) => from_reply(&reply, env!("CARGO_PKG_VERSION")),
            Err(error) => from_error(&error),
        },
    )
}

/// Recherche activée : inscription au relais en tâche de fond (quelques
/// secondes de calcul la première fois), pour que la première recherche
/// n'attende pas.
pub fn register_in_background(access: &Access) {
    access.relay().spawn_registration(CancellationToken::new());
}

/// Au lancement : inscription si la recherche est déjà activée.
pub fn setup(app: &AppHandle) {
    let Some(access) = app.try_state::<Access>().map(|state| state.inner().clone()) else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        if access.prefs().await.is_ok_and(|prefs| prefs.web_search) {
            register_in_background(&access);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use relay_proto::api::Quota;
    use relay_server::fake::TestRelay;

    use super::*;
    use crate::store::Store;

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    /// Accès sur une base neuve, relais à `relay_url`.
    fn access(relay_url: &str, dir: &std::path::Path) -> Access {
        let store = Store::open_in_memory().unwrap();
        let engine = crate::engine::Engine::new(
            crate::engine::Config::new(None, dir),
            Arc::new(crate::engine::AppModels::new(
                Some(store.clone()),
                dir.to_path_buf(),
            )),
        );
        let downloads =
            crate::hub::downloads::HubDownloads::with_options(|_| {}, Default::default());
        let relay = Arc::new(crate::relay::RelayClient::with_url(
            relay_url.to_owned(),
            dir.to_path_buf(),
        ));
        Access::new(
            engine,
            store,
            dir.to_path_buf(),
            Arc::new(crate::llm::remote::Unavailable),
            relay,
            downloads,
        )
    }

    async fn turn_on(access: &Access) {
        access
            .store()
            .call(|c| {
                crate::store::settings::update(
                    c,
                    crate::store::settings::SettingsPatch {
                        web_search_enabled: Some(true),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap();
    }

    fn reply(service: ServiceState, remaining: u32, minimum: Option<&str>) -> StatusResponse {
        StatusResponse {
            service,
            quota: Quota {
                daily_limit: 50,
                remaining_today: remaining,
                resets_at: 1_000,
            },
            min_app_version: minimum.map(str::to_owned),
            server_time: 1,
        }
    }

    #[test]
    fn states_follow_the_relay() {
        let ok = from_reply(&reply(ServiceState::Ok, 12, None), "0.10.0");
        assert_eq!(
            ok,
            WebSearchStatus {
                state: WebSearchState::Ok,
                daily_limit: Some(50),
                remaining_today: Some(12),
                resets_at: Some(1_000_000),
                retry_at: None,
            }
        );
        for (reply, state) in [
            (reply(ServiceState::Ok, 0, None), WebSearchState::Limit),
            (reply(ServiceState::Paused, 3, None), WebSearchState::Paused),
            (
                reply(ServiceState::BudgetExhausted, 3, None),
                WebSearchState::BudgetExhausted,
            ),
            (
                reply(ServiceState::Ok, 3, Some("0.10.1")),
                WebSearchState::Outdated,
            ),
            (
                reply(ServiceState::Ok, 3, Some("0.10.0")),
                WebSearchState::Ok,
            ),
        ] {
            assert_eq!(from_reply(&reply, "0.10.0").state, state);
        }
        for (error, state, retry) in [
            (
                RelayError::QuotaExceeded { retry_at: Some(7) },
                WebSearchState::Limit,
                Some(7_000),
            ),
            (
                RelayError::RateLimited { retry_at: None },
                WebSearchState::Limit,
                None,
            ),
            (
                RelayError::BudgetExhausted { retry_at: Some(9) },
                WebSearchState::BudgetExhausted,
                Some(9_000),
            ),
            (RelayError::Paused, WebSearchState::Paused, None),
            (RelayError::Outdated, WebSearchState::Outdated, None),
            (RelayError::Offline, WebSearchState::Unavailable, None),
            (RelayError::Banned, WebSearchState::Unavailable, None),
        ] {
            let status = from_error(&error);
            assert_eq!((status.state, status.retry_at), (state, retry), "{error:?}");
        }
    }

    #[tokio::test]
    async fn turned_off_the_relay_is_never_asked() {
        let dir = tempfile::tempdir().unwrap();
        let relay = TestRelay::start(&dir.path().join("relay.sqlite"), now(), |_| {}).await;
        let access = access(&relay.url, &dir.path().join("data"));
        let off = status(&access).await.unwrap();
        assert_eq!(off.state, WebSearchState::Off);
        // Rien d'écrit : ni clé d'installation, ni inscription.
        assert!(!dir.path().join("data").join("relay").exists());
        assert_eq!(relay.relay.spent_this_month(), 0);

        turn_on(&access).await;
        let on = status(&access).await.unwrap();
        assert_eq!(on.state, WebSearchState::Ok);
        assert_eq!(on.remaining_today, Some(50));
        relay.stop().await;
    }

    #[tokio::test]
    async fn turning_search_on_registers_in_the_background() {
        let dir = tempfile::tempdir().unwrap();
        let relay = TestRelay::start(&dir.path().join("relay.sqlite"), now(), |_| {}).await;
        let data = dir.path().join("data");
        let access = access(&relay.url, &data);
        register_in_background(&access);
        let identity = data
            .join(crate::relay::identity::DIR)
            .join(crate::relay::identity::FILE);
        let registered = async {
            loop {
                let saved = std::fs::read(&identity)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
                if saved.is_some_and(|saved| saved["registered"][0] == relay.url.as_str()) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(20), registered)
            .await
            .expect("registered in the background");
        relay.stop().await;
    }
}
