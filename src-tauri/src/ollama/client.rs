//! Requêtes HTTP vers le serveur Ollama.

use std::time::Duration;

use serde::Deserialize;
use url::{Host, Url};

use crate::i18n::t;

/// Délai d'une sonde (`/api/version`) : court, l'écran d'accueil revérifie
/// toutes les 3 s. Une machine distante a droit à un peu plus.
///
/// Sous Windows, le refus d'un port fermé de la machine n'arrive qu'après
/// environ 2 s (la pile réseau réessaie deux fois avant de rendre l'erreur) :
/// avec un délai de 2 s, « rien n'écoute » passait pour « pas de réponse »
/// (mesuré sur un runner Windows : `Timeout` au lieu de `Refused`). Une
/// seconde de plus laisse arriver le refus.
const LOCAL_PROBE_TIMEOUT: Duration = Duration::from_secs(if cfg!(windows) { 3 } else { 2 });
const REMOTE_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(if cfg!(windows) { 3 } else { 2 });

/// Taille maximale lue pour la réponse de `/api/version` (quelques dizaines
/// d'octets chez Ollama) : un autre programme ne doit pas faire lire des Go.
const MAX_VERSION_BODY: usize = 64 * 1024;

/// Résultat d'une sonde de `/api/version`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Un serveur Ollama répond.
    Ollama { version: String },
    /// Quelque chose répond en HTTP, mais pas comme Ollama ; avec le code
    /// d'état reçu (200 si c'est le contenu qui ne convient pas).
    NotOllama { status: u16 },
    /// Connexion refusée : rien n'écoute sur ce port.
    Refused,
    /// Pas de réponse dans le délai (connexion acceptée ou pas).
    Timeout,
    /// Autre échec (nom inconnu, réseau injoignable, TLS, connexion coupée...),
    /// avec le détail de la chaîne d'erreurs.
    Failed(String),
}

#[derive(Deserialize)]
struct VersionResponse {
    version: String,
}

/// Installe une fois le moteur cryptographique de rustls (ring), exigé par
/// reqwest avant de créer un client.
fn ensure_crypto_provider() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Adresse du serveur en URL. Erreur en anglais : l'adresse vient des
/// réglages, déjà normalisée (`address::normalize`), jamais refusée ici.
pub fn server_url(base: &str) -> crate::error::AppResult<Url> {
    Url::parse(base).map_err(|_| {
        // i18n-ignore
        crate::error::AppError::Invalid(format!("invalid address: {base}"))
    })
}

/// Client pour une adresse donnée. Pas de proxy vers la machine locale : un
/// HTTP_PROXY d'entreprise ne doit pas intercepter les appels à localhost.
pub fn client(base: &Url, timeout: Option<Duration>) -> reqwest::Result<reqwest::Client> {
    builder(base, timeout).build()
}

/// Réglages de [`client`], à compléter avant `build()`.
pub fn builder(base: &Url, timeout: Option<Duration>) -> reqwest::ClientBuilder {
    ensure_crypto_provider();
    let mut builder = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(concat!("ChatonLM/", env!("CARGO_PKG_VERSION")));
    if is_local(base) {
        builder = builder.no_proxy();
    }
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    builder
}

/// Interroge `GET <base>/api/version`.
pub async fn probe(base: &str) -> Probe {
    let Ok(base_url) = Url::parse(base) else {
        return Probe::Failed(format!("invalid address: {base}"));
    };
    let timeout = if is_local(&base_url) {
        LOCAL_PROBE_TIMEOUT
    } else {
        REMOTE_PROBE_TIMEOUT
    };
    let client = match client(&base_url, Some(timeout)) {
        Ok(client) => client,
        Err(error) => return Probe::Failed(describe(&error)),
    };
    let mut response = match client.get(format!("{base}/api/version")).send().await {
        Ok(response) => response,
        Err(error) => return classify(&error),
    };
    let status = response.status().as_u16();
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > MAX_VERSION_BODY as u64)
    {
        return Probe::NotOllama { status };
    }
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk);
                if body.len() > MAX_VERSION_BODY {
                    return Probe::NotOllama { status };
                }
            }
            Ok(None) => break,
            Err(error) => return classify(&error),
        }
    }
    match serde_json::from_slice::<VersionResponse>(&body) {
        Ok(version) if !version.version.trim().is_empty() => Probe::Ollama {
            version: version.version,
        },
        _ => Probe::NotOllama { status },
    }
}

fn classify(error: &reqwest::Error) -> Probe {
    if error.is_timeout() {
        return Probe::Timeout;
    }
    if error.is_connect() && is_refused(error) {
        return Probe::Refused;
    }
    Probe::Failed(describe(error))
}

/// Message d'un échec, sans l'adresse (affichée à côté) : nature de l'échec,
/// puis chaque cause distincte (« connection failed: … »).
fn describe(error: &reqwest::Error) -> String {
    // Nature de l'échec dans la langue de l'interface ; les causes viennent
    // des bibliothèques réseau (anglais).
    let kind = if error.is_connect() {
        t!("errors.connectionFailed")
    } else if error.is_request() || error.is_body() {
        t!("errors.requestFailed")
    } else {
        t!("errors.unknown")
    };
    let mut parts = vec![kind];
    let mut source = std::error::Error::source(error);
    while let Some(current) = source {
        let text = current.to_string();
        if !text.is_empty() && !parts.iter().any(|part| part.contains(&text)) {
            parts.push(text);
        }
        source = current.source();
    }
    parts.join(": ")
}

/// Cherche « connexion refusée » dans la chaîne des causes.
fn is_refused(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut source = Some(error);
    while let Some(current) = source {
        if let Some(io) = current.downcast_ref::<std::io::Error>()
            && io.kind() == std::io::ErrorKind::ConnectionRefused
        {
            return true;
        }
        source = current.source();
    }
    false
}

/// Vrai pour la machine locale : `localhost`, adresses de bouclage, adresses
/// d'écoute (`0.0.0.0`, `::`), y compris sous forme IPv4 dans IPv6
/// (`::ffff:127.0.0.1`).
pub fn is_local(url: &Url) -> bool {
    let local_v4 = |ip: std::net::Ipv4Addr| ip.is_loopback() || ip.is_unspecified();
    match url.host() {
        Some(Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain == "localhost" || domain.ends_with(".localhost")
        }
        Some(Host::Ipv4(ip)) => local_v4(ip),
        Some(Host::Ipv6(ip)) => {
            ip.is_loopback() || ip.is_unspecified() || ip.to_ipv4_mapped().is_some_and(local_v4)
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fake_ollama::{FakeOllama, Mode};

    #[tokio::test]
    async fn ollama_answers_with_its_version() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        assert_eq!(
            probe(&server.url()).await,
            Probe::Ollama {
                version: "0.35.1".into()
            }
        );
    }

    #[tokio::test]
    async fn another_program_is_not_ollama() {
        let server = FakeOllama::start(Mode::NotOllama).await.unwrap();
        assert_eq!(probe(&server.url()).await, Probe::NotOllama { status: 200 });
    }

    #[tokio::test]
    async fn http_errors_keep_their_status() {
        for code in [401, 403, 502, 503] {
            let server = FakeOllama::start(Mode::Status(code)).await.unwrap();
            assert_eq!(
                probe(&server.url()).await,
                Probe::NotOllama { status: code },
                "{code}"
            );
        }
    }

    #[tokio::test]
    async fn huge_answers_are_not_read() {
        let server = FakeOllama::start(Mode::Huge).await.unwrap();
        assert_eq!(probe(&server.url()).await, Probe::NotOllama { status: 200 });
    }

    #[tokio::test]
    async fn a_closed_connection_is_a_failure_with_details() {
        let server = FakeOllama::start(Mode::Close).await.unwrap();
        let Probe::Failed(detail) = probe(&server.url()).await else {
            panic!("expected a failure");
        };
        assert!(
            detail.starts_with("request failed") || detail.starts_with("connection failed"),
            "{detail}"
        );
        assert!(!detail.contains("http://"), "{detail}");
    }

    #[tokio::test]
    async fn closed_port_is_refused() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        let url = server.url();
        server.stop().await;
        assert_eq!(probe(&url).await, Probe::Refused);
    }

    #[tokio::test]
    async fn silent_server_times_out() {
        let server = FakeOllama::start(Mode::Silent).await.unwrap();
        let started = std::time::Instant::now();
        assert_eq!(probe(&server.url()).await, Probe::Timeout);
        assert!(started.elapsed() < LOCAL_PROBE_TIMEOUT + Duration::from_secs(1));
    }

    #[test]
    fn local_addresses() {
        for local in [
            "http://localhost:11434",
            "http://127.0.0.1:11434",
            "http://[::1]:11434",
            "http://0.0.0.0:11434",
            "http://app.localhost",
            "http://[::ffff:127.0.0.1]:11434",
        ] {
            assert!(is_local(&Url::parse(local).unwrap()), "{local}");
        }
        for remote in [
            "http://192.168.1.2:11434",
            "https://example.com",
            "http://[::ffff:192.168.1.2]:11434",
        ] {
            assert!(!is_local(&Url::parse(remote).unwrap()), "{remote}");
        }
    }
}
