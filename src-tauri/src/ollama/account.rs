// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Translated to Rust from Ollama (https://github.com/ollama/ollama),
// files app/ui/app/src/api.ts (fetchUser, fetchConnectUrl, disconnectUser,
// getCloudStatus) and app/ui/app/src/hooks/useUser.ts, commit 42e911bc3d05.
// Modified for ChatonLM: requests made by the backend instead of the page; the
// sign-in address is opened only if it is the /connect page of ollama.com; the
// account picture is read here and handed over as a data: URL; signing out of
// a server that is already signed out succeeds.

//! Compte ollama.com du serveur Ollama, comme les réglages de l'officiel
//! (`useUser.ts`, `api.ts`) : le serveur signe ses requêtes avec sa propre clé,
//! reliée à un compte par la page `ollama.com/connect` ouverte dans le
//! navigateur. ChatonLM ne voit ni clé ni mot de passe : il lit l'état
//! (`/api/me`), ouvre l'adresse de connexion donnée par le serveur et demande
//! la déconnexion (`/api/signout`). L'état du cloud (`/api/status`) dit si le
//! serveur a coupé le cloud (`OLLAMA_NO_CLOUD`), donc la recherche web.
//!
//! Écarts à l'officiel : l'adresse de connexion n'est ouverte que si c'est la
//! page `/connect` d'ollama.com (l'officiel ouvre ce que le serveur donne) ;
//! l'image du compte est lue ici et passée en URL `data:` (la CSP de la page
//! n'autorise aucune image extérieure) ; une déconnexion d'un serveur déjà
//! délié réussit.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use url::Url;

use super::client;
use super::models::{answered, error_message, unreachable};
use crate::error::{AppError, AppResult};
use crate::i18n::t;

/// Délais de `/api/me` (le serveur interroge ollama.com) et de la déconnexion.
const ACCOUNT_TIMEOUT: Duration = Duration::from_secs(10);
/// Délai de `/api/status`, réponse locale du serveur.
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);
/// Délai de la vérification du compte avant une recherche web : au-delà,
/// l'envoi part sans savoir.
const LINKED_TIMEOUT: Duration = Duration::from_secs(5);
const AVATAR_TIMEOUT: Duration = Duration::from_secs(10);

/// Taille maximale lue pour les réponses du serveur (quelques centaines
/// d'octets chez Ollama).
const MAX_BODY: usize = 64 * 1024;
/// Taille maximale de l'image du compte.
const MAX_AVATAR: usize = 1024 * 1024;
/// Redirections suivies au plus pour l'image du compte.
const MAX_REDIRECTS: usize = 5;

const DEFAULT_OLLAMA_COM: &str = "https://ollama.com";

/// Texte de l'officiel (`fetchConnectUrl`) quand le serveur ne donne pas
/// d'adresse de connexion.
fn no_connect_url() -> AppError {
    AppError::Ollama(t!("account.noConnectUrl"))
}

/// État du compte relié au serveur.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "state", rename_all = "camelCase")]
#[ts(export)]
pub enum AccountStatus {
    SignedIn {
        name: String,
        email: String,
        /// « free » si le serveur ne le dit pas, comme lui.
        plan: String,
        /// Image du compte en URL `data:`, lue par le moteur (la page n'a
        /// accès à aucune adresse extérieure) ; absente si illisible.
        avatar: Option<String>,
    },
    SignedOut,
}

/// État du cloud du serveur.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct CloudStatus {
    pub disabled: bool,
    /// Origine du réglage (« none », « env », « config », « both »).
    #[serde(default)]
    pub source: String,
}

#[derive(Deserialize)]
struct StatusResponse {
    cloud: CloudStatus,
}

#[derive(Deserialize)]
struct UserResponse {
    #[serde(default)]
    name: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    plan: String,
    #[serde(default)]
    avatarurl: String,
}

#[derive(Deserialize)]
struct SignedOutResponse {
    #[serde(default)]
    signin_url: Option<String>,
}

/// Réponse de `/api/me`.
enum Me {
    User(UserResponse),
    /// Pas de compte relié, avec l'adresse de connexion du serveur.
    SignedOut(Option<String>),
}

/// Échec de `/api/me`.
enum MeError {
    /// Serveur injoignable, ou réponse coupée.
    Unreachable(AppError),
    /// Autre statut, ou réponse illisible.
    Answer(AppError),
}

impl From<MeError> for AppError {
    fn from(error: MeError) -> Self {
        match error {
            MeError::Unreachable(error) | MeError::Answer(error) => error,
        }
    }
}

/// Adresse d'ollama.com : `CHATONLM_OLLAMA_COM_URL` si elle est définie et non
/// vide (tests de l'app réelle, comme `OLLAMA_DOT_COM_URL` pour l'officiel),
/// sinon https://ollama.com.
fn ollama_com() -> String {
    ollama_com_from(test_override("CHATONLM_OLLAMA_COM_URL"))
}

/// Adresse imposée par les essais (faux serveur), comme dans `hub/names.rs` :
/// jamais lue par le build de production, où un environnement modifié
/// enverrait sinon la clé du serveur vers un hôte quelconque.
#[cfg(any(test, debug_assertions, feature = "e2e"))]
fn test_override(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(not(any(test, debug_assertions, feature = "e2e")))]
fn test_override(_name: &str) -> Option<String> {
    None
}

fn ollama_com_from(value: Option<String>) -> String {
    value
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_OLLAMA_COM.to_owned())
}

/// État du compte. Erreur si le serveur est injoignable ou répond autre chose
/// que 200, 401 ou 403 (ollama.com injoignable : 503 « account unavailable »).
pub async fn status(base: &str) -> AppResult<AccountStatus> {
    status_with(base, &ollama_com()).await
}

/// [`status`] avec l'adresse d'ollama.com donnée (tests : le faux serveur).
async fn status_with(base: &str, ollama_com: &str) -> AppResult<AccountStatus> {
    Ok(match me(base).await.map_err(AppError::from)? {
        Me::User(user) => {
            let avatar = avatar(ollama_com, &user.avatarurl).await;
            AccountStatus::SignedIn {
                name: user.name,
                email: user.email,
                plan: if user.plan.trim().is_empty() {
                    "free".into()
                } else {
                    user.plan
                },
                avatar,
            }
        }
        Me::SignedOut(_) => AccountStatus::SignedOut,
    })
}

/// Adresse de connexion à ouvrir dans le navigateur ; `None` si un compte est
/// déjà relié. Seule une page `/connect` d'ollama.com est acceptée : un autre
/// programme à l'adresse du serveur ne fait pas ouvrir n'importe quoi.
pub async fn sign_in_url(base: &str) -> AppResult<Option<String>> {
    sign_in_url_with(base, &ollama_com()).await
}

/// [`sign_in_url`] avec l'adresse d'ollama.com donnée (tests : le faux serveur).
async fn sign_in_url_with(base: &str, ollama_com: &str) -> AppResult<Option<String>> {
    match me(base).await {
        Ok(Me::User(_)) => Ok(None),
        Ok(Me::SignedOut(url)) => url
            .and_then(|url| connect_url(ollama_com, &url))
            .map(Some)
            .ok_or_else(no_connect_url),
        // Serveur injoignable : son message ; toute autre réponse, celui de
        // l'officiel.
        Err(MeError::Unreachable(error)) => Err(error),
        Err(MeError::Answer(_)) => Err(no_connect_url()),
    }
}

/// Compte relié au serveur, vérifié avant une recherche web : `Some(false)`
/// seulement si le serveur dit qu'aucun compte n'est relié (401 ou 403) ;
/// `None` s'il ne sait pas (injoignable, ollama.com qui ne répond pas, délai).
pub async fn linked(base: &str) -> Option<bool> {
    match me_within(base, LINKED_TIMEOUT).await {
        Ok(Me::User(_)) => Some(true),
        Ok(Me::SignedOut(_)) => Some(false),
        Err(_) => None,
    }
}

/// Délie le compte du serveur. Écart : « you are not currently signed in »
/// (401) vaut succès, l'état relu ensuite le montre déconnecté.
pub async fn sign_out(base: &str) -> AppResult<()> {
    let client = server_client(base, ACCOUNT_TIMEOUT)?;
    let response = client
        .post(format!("{base}/api/signout"))
        .send()
        .await
        .map_err(unreachable)?;
    let status = response.status();
    if status.is_success() || status == reqwest::StatusCode::UNAUTHORIZED {
        return Ok(());
    }
    let body = read_capped(response, MAX_BODY).await.unwrap_or_default();
    Err(answered(
        status.as_u16(),
        &error_message(&String::from_utf8_lossy(&body)),
    ))
}

/// État du cloud du serveur ; `None` s'il est inconnu (serveur sans cette
/// route, injoignable, réponse illisible).
pub async fn cloud(base: &str) -> Option<CloudStatus> {
    let client = server_client(base, STATUS_TIMEOUT).ok()?;
    let response = client.get(format!("{base}/api/status")).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = read_capped(response, MAX_BODY).await.ok()?;
    serde_json::from_slice::<StatusResponse>(&body)
        .ok()
        .map(|status| status.cloud)
}

async fn me(base: &str) -> Result<Me, MeError> {
    me_within(base, ACCOUNT_TIMEOUT).await
}

async fn me_within(base: &str, timeout: Duration) -> Result<Me, MeError> {
    let client = server_client(base, timeout).map_err(MeError::Unreachable)?;
    let response = client
        .post(format!("{base}/api/me"))
        .send()
        .await
        .map_err(|error| MeError::Unreachable(unreachable(error)))?;
    let status = response.status();
    let body = read_capped(response, MAX_BODY)
        .await
        .map_err(|error| MeError::Unreachable(unreachable(error)))?;
    match status.as_u16() {
        // Comme le serveur et l'officiel (`isAuthenticated`) : relié
        // seulement avec un nom.
        200 => match serde_json::from_slice::<UserResponse>(&body) {
            Ok(user) if !user.name.trim().is_empty() => Ok(Me::User(user)),
            Ok(_) => Ok(Me::SignedOut(None)),
            Err(error) => Err(MeError::Answer(AppError::Ollama(t!(
                "ollama.unexpectedAccount",
                error = error
            )))),
        },
        401 | 403 => Ok(Me::SignedOut(
            serde_json::from_slice::<SignedOutResponse>(&body)
                .ok()
                .and_then(|response| response.signin_url),
        )),
        code => Err(MeError::Answer(answered(
            code,
            &error_message(&String::from_utf8_lossy(&body)),
        ))),
    }
}

fn server_client(base: &str, timeout: Duration) -> AppResult<reqwest::Client> {
    let url = client::server_url(base)?;
    client::client(&url, Some(timeout)).map_err(unreachable)
}

/// Adresse de connexion acceptée, avec `launch=true` comme l'officiel
/// (`fetchConnectUrl`) ; `None` si ce n'est pas la page `/connect`
/// d'ollama.com (ou de son remplaçant de test).
fn connect_url(ollama_com: &str, raw: &str) -> Option<String> {
    let expected = Url::parse(ollama_com).ok()?;
    let mut url = Url::parse(raw.trim()).ok()?;
    let same_site = url.scheme() == expected.scheme()
        && url.host_str() == expected.host_str()
        && url.port_or_known_default() == expected.port_or_known_default();
    if !same_site
        || !matches!(url.scheme(), "https" | "http")
        || url.path() != format!("{}/connect", expected.path().trim_end_matches('/'))
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| key != "launch")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.query_pairs_mut()
        .clear()
        .extend_pairs(pairs)
        .append_pair("launch", "true");
    Some(url.into())
}

/// Image du compte en URL `data:`, comme l'officiel l'affiche : relative
/// (chemin qui commence par `/`), elle est sur ollama.com ; absolue,
/// seulement en https (ou chez le remplaçant de test d'ollama.com), et les
/// redirections ne sont suivies que vers https. Format reconnu à son contenu
/// (PNG, JPEG, GIF, WebP).
async fn avatar(ollama_com: &str, raw: &str) -> Option<String> {
    let raw = raw.trim();
    let address = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_owned()
    } else if raw.starts_with('/') && !raw.starts_with("//") {
        format!("{ollama_com}{raw}")
    } else {
        return None;
    };
    let url = Url::parse(&address).ok()?;
    if url.scheme() != "https" && !address.starts_with(&format!("{ollama_com}/")) {
        return None;
    }
    let client = client::builder(&url, Some(AVATAR_TIMEOUT))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.url().scheme() == "https" && attempt.previous().len() < MAX_REDIRECTS {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .ok()?;
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let image = read_capped(response, MAX_AVATAR).await.ok()?;
    let mime = image_type(&image)?;
    Some(format!("data:{mime};base64,{}", base64(&image)))
}

/// Type d'image d'après ses premiers octets.
fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Base64 standard, avec remplissage.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Corps de la réponse, refusé au-delà de `max` octets.
async fn read_capped(mut response: reqwest::Response, max: usize) -> reqwest::Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > max {
            // Coupé : traité comme illisible plus loin.
            body.clear();
            break;
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fake_ollama::{FakeOllama, Mode};

    const CONNECT: &str = "https://ollama.com/connect?name=host&key=ZmFrZQ";

    #[test]
    fn ollama_com_can_be_replaced_for_tests() {
        assert_eq!(ollama_com_from(None), "https://ollama.com");
        assert_eq!(ollama_com_from(Some("  ".into())), "https://ollama.com");
        assert_eq!(
            ollama_com_from(Some("http://127.0.0.1:4000/".into())),
            "http://127.0.0.1:4000"
        );
    }

    #[test]
    fn the_connect_page_gets_launch_true() {
        assert_eq!(
            connect_url(DEFAULT_OLLAMA_COM, CONNECT).as_deref(),
            Some("https://ollama.com/connect?name=host&key=ZmFrZQ&launch=true")
        );
        // Déjà présent : remplacé, pas doublé.
        assert_eq!(
            connect_url(
                DEFAULT_OLLAMA_COM,
                "https://ollama.com/connect?launch=false&key=a"
            )
            .as_deref(),
            Some("https://ollama.com/connect?key=a&launch=true")
        );
        assert_eq!(
            connect_url(
                "http://127.0.0.1:4000",
                "http://127.0.0.1:4000/connect?key=a"
            )
            .as_deref(),
            Some("http://127.0.0.1:4000/connect?key=a&launch=true")
        );
    }

    #[test]
    fn only_the_connect_page_of_ollama_com_is_opened() {
        for raw in [
            "http://ollama.com/connect?key=a",
            "https://ollama.com.example/connect?key=a",
            "https://evil.example/connect?key=a",
            "https://ollama.com/settings",
            "https://ollama.com/connect/../settings",
            "https://user:pass@ollama.com/connect?key=a",
            "https://ollama.com:8443/connect?key=a",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "not a url",
        ] {
            assert_eq!(connect_url(DEFAULT_OLLAMA_COM, raw), None, "{raw}");
        }
        // Le remplaçant de test n'élargit pas la règle à ollama.com.
        assert_eq!(connect_url("http://127.0.0.1:4000", CONNECT), None);
    }

    #[test]
    fn images_are_recognized_by_their_content() {
        assert_eq!(image_type(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(image_type(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(image_type(b"GIF89a..."), Some("image/gif"));
        assert_eq!(image_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_type(b"<svg xmlns="), None);
        assert_eq!(image_type(b""), None);
    }

    #[test]
    fn base64_matches_the_standard() {
        // RFC 4648, § 10.
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
        assert_eq!(base64(&[0xFF, 0xFE, 0xFD]), "//79");
    }

    async fn set_signed_in(server: &FakeOllama, signed_in: bool) {
        let response = reqwest::Client::new()
            .post(format!("{}/fake/account", server.url()))
            .json(&serde_json::json!({ "signedIn": signed_in }))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
    }

    #[tokio::test]
    async fn status_follows_the_server_account() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        let base = server.url();
        assert_eq!(
            status_with(&base, &base).await.unwrap(),
            AccountStatus::SignedOut
        );
        set_signed_in(&server, true).await;
        match status_with(&base, &base).await.unwrap() {
            AccountStatus::SignedIn {
                name,
                email,
                plan,
                avatar,
            } => {
                assert_eq!(
                    (name.as_str(), email.as_str(), plan.as_str()),
                    ("ada", "ada@example.com", "free")
                );
                assert!(avatar.is_some_and(|a| a.starts_with("data:image/png;base64,")));
            }
            other => panic!("{other:?}"),
        }
        // Déconnexion, puis déconnexion d'un serveur déjà délié : succès.
        sign_out(&base).await.unwrap();
        assert_eq!(
            status_with(&base, &base).await.unwrap(),
            AccountStatus::SignedOut
        );
        sign_out(&base).await.unwrap();
    }

    #[tokio::test]
    async fn the_avatar_is_read_from_ollama_com() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        // Relative : prise chez ollama.com (ici le faux serveur).
        let found = avatar(&server.url(), "/fake/avatar.png").await.unwrap();
        assert!(
            found.starts_with("data:image/png;base64,iVBORw0KGgo"),
            "{found}"
        );
        // En clair ailleurs que chez ollama.com : refusée sans requête.
        let elsewhere = format!("{}/fake/avatar.png", server.url());
        assert_eq!(avatar(DEFAULT_OLLAMA_COM, &elsewhere).await, None);
        // Pas une image : absente.
        assert_eq!(avatar(&server.url(), "/api/version").await, None);
        assert_eq!(avatar(&server.url(), "").await, None);
        // Relative sans « / » initial : jamais accolée à l'adresse
        // d'ollama.com (« https://ollama.com@hôte/... »).
        assert_eq!(avatar(&server.url(), "fake/avatar.png").await, None);
        assert_eq!(avatar(&server.url(), "@127.0.0.1/x.png").await, None);
        assert_eq!(avatar(&server.url(), "//127.0.0.1/x.png").await, None);
        // Redirection vers une adresse en clair : pas suivie.
        assert_eq!(avatar(&server.url(), "/fake/avatar-redirect").await, None);
    }

    #[tokio::test]
    async fn sign_in_gives_the_connect_page_until_an_account_is_linked() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        let base = server.url();
        assert_eq!(
            sign_in_url_with(&base, &base).await.unwrap(),
            Some(format!(
                "{base}/connect?name=fake-host&key=ZmFrZQ&launch=true"
            ))
        );
        // Page d'un autre site que celui d'ollama.com : refusée.
        let Err(AppError::Ollama(message)) = sign_in_url_with(&base, DEFAULT_OLLAMA_COM).await
        else {
            panic!("expected an error");
        };
        assert_eq!(message, "Failed to fetch connect URL");
        set_signed_in(&server, true).await;
        assert_eq!(sign_in_url_with(&base, &base).await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_unexpected_answer_is_an_error() {
        let failing = FakeOllama::start(Mode::Status(503)).await.unwrap();
        let base = failing.url();
        assert!(status_with(&base, &base).await.is_err());
        let Err(AppError::Ollama(message)) = sign_in_url_with(&base, &base).await else {
            panic!("expected an error");
        };
        assert_eq!(message, "Failed to fetch connect URL");
        assert!(sign_out(&base).await.is_err());
        assert_eq!(cloud(&base).await, None);
    }

    #[tokio::test]
    async fn an_unreachable_server_keeps_its_message() {
        let base = "http://127.0.0.1:9";
        let Err(AppError::Ollama(message)) = sign_in_url_with(base, base).await else {
            panic!("expected an error");
        };
        assert_eq!(message, "can't connect to Ollama");
        assert!(status_with(base, base).await.is_err());
        assert_eq!(cloud(base).await, None);
    }

    #[tokio::test]
    async fn linked_is_known_only_when_the_server_says_so() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        let base = server.url();
        assert_eq!(linked(&base).await, Some(false));
        server.set_signed_in(true);
        assert_eq!(linked(&base).await, Some(true));
        // ollama.com injoignable pour le serveur (503) : inconnu.
        server.set_account_unavailable(true);
        assert_eq!(linked(&base).await, None);
        assert_eq!(linked("http://127.0.0.1:9").await, None);
    }

    #[tokio::test]
    async fn cloud_status_is_read_from_the_server() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        assert_eq!(
            cloud(&server.url()).await,
            Some(CloudStatus {
                disabled: false,
                source: "none".into()
            })
        );
        let response = reqwest::Client::new()
            .post(format!("{}/fake/cloud", server.url()))
            .json(&serde_json::json!({ "disabled": true }))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        assert!(cloud(&server.url()).await.unwrap().disabled);
    }

    /// Contrat : l'instance de test (jamais reliée, sa clé est dans
    /// `.tools/ollama-test`) donne l'adresse de connexion d'ollama.com.
    #[tokio::test]
    #[ignore = "instance Ollama de test (scripts/test-ollama.sh start) et ollama.com : accès au réseau"]
    async fn contract_account_of_the_test_instance() {
        let base = "http://127.0.0.1:11435";
        let client = client::client(&Url::parse(base).unwrap(), None).unwrap();
        let response = client.post(format!("{base}/api/me")).send().await.unwrap();
        let code = response.status().as_u16();
        let body: serde_json::Value = response.json().await.unwrap();
        match code {
            401 => {
                let url = body["signin_url"].as_str().unwrap_or_default();
                assert!(
                    url.starts_with("https://ollama.com/connect?name=") && url.contains("&key="),
                    "{body}"
                );
                assert_eq!(status(base).await.unwrap(), AccountStatus::SignedOut);
                let opened = sign_in_url(base).await.unwrap().unwrap();
                assert!(opened.ends_with("&launch=true"), "{opened}");
            }
            200 => assert!(
                body["name"].as_str().is_some_and(|n| !n.is_empty()),
                "{body}"
            ),
            _ => panic!("/api/me answered {code}: {body}"),
        }
        let cloud = cloud(base).await.expect("/api/status");
        assert!(!cloud.source.is_empty());
    }
}
