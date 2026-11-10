//! Clients HTTP de l'app (Hugging Face, téléchargements de la dictée, moteur
//! d'images…) : délai de connexion, `User-Agent` de ChatonLM, jamais de
//! proxy vers la machine locale.

use std::time::Duration;

use url::{Host, Url};

/// Délai d'établissement d'une connexion. Sous Windows, le refus d'un port
/// fermé de la machine n'arrive qu'après environ 2 s (la pile réseau réessaie
/// deux fois avant de rendre l'erreur) : une seconde de plus le laisse
/// arriver.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(if cfg!(windows) { 3 } else { 2 });

/// Installe une fois le moteur cryptographique de rustls (ring), exigé par
/// reqwest avant de créer un client (même s'il ne sert pas en HTTP local).
pub(crate) fn ensure_crypto_provider() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
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

/// Message d'une erreur de reqwest sans l'adresse, avec ses causes.
pub(crate) fn error_text(error: &reqwest::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = std::error::Error::source(error);
    while let Some(current) = source {
        let text = current.to_string();
        if !text.is_empty() && !parts.iter().any(|part| part.contains(&text)) {
            parts.push(text);
        }
        source = current.source();
    }
    let text = parts.join(": ");
    match error.url() {
        Some(url) => text.replace(&format!(" ({url})"), ""),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_addresses() {
        for local in [
            "http://localhost:8080",
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
            "http://0.0.0.0:8080",
            "http://app.localhost",
            "http://[::ffff:127.0.0.1]:8080",
        ] {
            assert!(is_local(&Url::parse(local).unwrap()), "{local}");
        }
        for remote in [
            "http://192.168.1.2:8080",
            "https://example.com",
            "http://[::ffff:192.168.1.2]:8080",
        ] {
            assert!(!is_local(&Url::parse(remote).unwrap()), "{remote}");
        }
    }

    /// Le texte d'une erreur ne répète pas l'adresse (montrée à côté) et
    /// garde ses causes.
    #[tokio::test]
    async fn error_texts_leave_out_the_address() {
        let url = Url::parse("http://127.0.0.1:9/some/path").unwrap();
        let error = client(&url, Some(std::time::Duration::from_secs(5)))
            .unwrap()
            .get(url.clone())
            .send()
            .await
            .unwrap_err();
        let text = error_text(&error);
        assert!(!text.contains("/some/path"), "{text}");
        assert!(text.len() > "error sending request".len(), "{text}");
    }
}
