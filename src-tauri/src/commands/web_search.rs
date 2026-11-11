//! Réglages de la recherche web (Settings › Web search, propre à ChatonLM) :
//! moteur choisi (DuckDuckGo par défaut, ou Brave, Mojeek, SearXNG), sa clé
//! ou son adresse, essai d'un moteur. Les clés vont en base et ne
//! reviennent jamais vers la page : seulement « configurée : oui ou non »
//! ([`EngineChoice`]). La recherche elle-même : `crate::search`.

use serde::Serialize;
use tauri::State;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::llm::Access;
use crate::store::web_search::{self as saved, Engine, EngineChoice};

/// Moteur choisi et secrets configurés.
#[tauri::command]
pub async fn web_search_engine(access: State<'_, Access>) -> AppResult<EngineChoice> {
    choice(&access).await
}

async fn choice(access: &Access) -> AppResult<EngineChoice> {
    Ok(access.store().call(|c| saved::load(c)).await?.choice())
}

/// Secret vérifié avant d'être gardé : clé sans espaces autour, sans
/// caractère de contrôle ; adresse SearXNG normalisée (http(s), un hôte).
pub fn checked_secret(engine: Engine, secret: &str) -> AppResult<String> {
    let secret = secret.trim();
    match engine {
        Engine::DuckDuckGo => Ok(String::new()),
        Engine::Searxng => web_search::searxng::normalize(secret)
            .ok_or_else(|| AppError::Invalid(t!("webSearch.badAddress"))),
        Engine::Brave | Engine::Mojeek => {
            if secret.is_empty() || secret.len() > 256 || secret.chars().any(char::is_control) {
                Err(AppError::Invalid(t!("webSearch.badKey")))
            } else {
                Ok(secret.to_owned())
            }
        }
    }
}

/// Choisit le moteur ; `secret` (clé, ou adresse de l'instance SearXNG)
/// remplace celui de ce moteur s'il est donné.
#[tauri::command]
pub async fn set_web_search_engine(
    access: State<'_, Access>,
    engine: Engine,
    secret: Option<String>,
) -> AppResult<EngineChoice> {
    set(&access, engine, secret).await
}

pub async fn set(
    access: &Access,
    engine: Engine,
    secret: Option<String>,
) -> AppResult<EngineChoice> {
    let secret = secret
        .map(|secret| checked_secret(engine, &secret))
        .transpose()?;
    access
        .store()
        .call(move |c| saved::choose(c, engine, secret.as_deref()))
        .await?;
    choice(access).await
}

/// Oublie la clé (ou l'adresse) d'un moteur ; s'il était choisi, DuckDuckGo
/// le remplace.
#[tauri::command]
pub async fn forget_web_search_secret(
    access: State<'_, Access>,
    engine: Engine,
) -> AppResult<EngineChoice> {
    access
        .store()
        .call(move |c| saved::forget(c, engine))
        .await?;
    choice(&access).await
}

/// Résultat de l'essai d'un moteur.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WebSearchCheck {
    pub ok: bool,
    /// Texte à montrer (langue du moment).
    pub message: String,
}

/// Essaie le moteur `engine` seul, tel qu'il est configuré : une recherche
/// d'un résultat (« ChatonLM »), sans DuckDuckGo en secours.
#[tauri::command]
pub async fn check_web_search_engine(
    access: State<'_, Access>,
    engine: Engine,
) -> AppResult<WebSearchCheck> {
    Ok(check(&access, engine).await)
}

pub async fn check(access: &Access, engine: Engine) -> WebSearchCheck {
    let outcome = access
        .web_search()
        .check(engine, &CancellationToken::new())
        .await;
    WebSearchCheck {
        ok: outcome == crate::search::Checked::Ok,
        message: crate::search::check_message(engine, outcome),
    }
}
