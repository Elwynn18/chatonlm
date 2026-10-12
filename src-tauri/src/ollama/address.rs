//! Adresse du serveur Ollama.
//!
//! ChatonLM accepte les mêmes formes que la variable `OLLAMA_HOST` d'Ollama
//! (envconfig.Host) : hôte seul, `hôte:port`, `[ipv6]:port`, `:port`, ou URL
//! complète avec un éventuel chemin (derrière un proxy). Sans schéma, le port
//! par défaut est 11434 ; avec `http://` ou `https://`, c'est celui du schéma ;
//! `ollama.com` seul désigne https://ollama.com. Une adresse d'écoute
//! (`0.0.0.0`, `::`) désigne, côté client, la machine locale. Un suffixe
//! `/api` ou `/v1` copié d'un autre outil est retiré : ChatonLM ajoute
//! lui-même le chemin de l'API.

use url::{Host, Url};

use crate::i18n::t;

/// Adresse utilisée quand rien n'est réglé : celle de la demande initiale.
pub const DEFAULT_SERVER_URL: &str = "http://localhost:11434";

const DEFAULT_PORT: u16 = 11434;

/// Normalise une adresse saisie ou lue dans `OLLAMA_HOST`. Renvoie `None` pour
/// une valeur vide, une erreur pour une valeur invalide (montrée par
/// Settings, dans la langue de l'interface ; détails de `url` en anglais).
pub fn normalize(input: &str) -> Result<Option<String>, String> {
    let value = input.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    if value.is_empty() {
        return Ok(None);
    }

    let mut url = if value.contains("://") {
        Url::parse(value).map_err(invalid)?
    } else {
        without_scheme(value)?
    };

    if !matches!(url.scheme(), "http" | "https") {
        return Err(t!(
            "server.address.unsupportedScheme",
            scheme = url.scheme()
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(t!("server.address.credentials"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(t!("server.address.query"));
    }

    // Adresse d'écoute sur toutes les interfaces : on se connecte en local.
    match url.host() {
        Some(Host::Ipv4(ip)) if ip.is_unspecified() => set_host(&mut url, "127.0.0.1")?,
        Some(Host::Ipv6(ip)) if ip.is_unspecified() => set_host(&mut url, "[::1]")?,
        _ => {}
    }

    let path = url.path().trim_end_matches('/');
    let path = path
        .strip_suffix("/api")
        .or_else(|| path.strip_suffix("/v1"))
        .unwrap_or(path)
        .to_owned();
    url.set_path(&path);

    Ok(Some(url.as_str().trim_end_matches('/').to_owned()))
}

/// Adresse sans schéma, lue comme le fait Ollama : `http`, port 11434 sauf
/// port explicite (80 compris), chemin éventuel après le premier `/`.
fn without_scheme(value: &str) -> Result<Url, String> {
    if value.eq_ignore_ascii_case("ollama.com") {
        return Url::parse("https://ollama.com").map_err(invalid);
    }
    let (host_port, path) = value.split_once('/').unwrap_or((value, ""));
    let (host, port) = split_host_port(host_port)?;
    let host = if host.is_empty() { "localhost" } else { host };
    // « 11435 » seul serait lu comme l'adresse IPv4 0.0.44.171.
    if host.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(t!("server.address.portOnly", value = value));
    }
    let port = match port {
        None => DEFAULT_PORT,
        Some(port) => port
            .parse::<u16>()
            .map_err(|_| t!("server.address.invalidPort", port = port))?,
    };
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    Url::parse(&format!("http://{host}:{port}/{path}")).map_err(invalid)
}

/// Comme `net.SplitHostPort` de Go, avec un port facultatif : `hôte`,
/// `hôte:port`, `[ipv6]`, `[ipv6]:port`, ou IPv6 sans crochets ni port.
fn split_host_port(value: &str) -> Result<(&str, Option<&str>), String> {
    let malformed = || invalid(value);
    if let Some(rest) = value.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or_else(malformed)?;
        return match after {
            "" => Ok((host, None)),
            _ => after
                .strip_prefix(':')
                .map(|port| (host, Some(port)))
                .ok_or_else(malformed),
        };
    }
    match value.split_once(':') {
        Some((host, port)) if !port.contains(':') => Ok((host, Some(port))),
        _ => Ok((value, None)),
    }
}

/// Adresse refusée ; `detail` : la valeur, ou l'erreur de `url` (anglais).
fn invalid(detail: impl std::fmt::Display) -> String {
    t!("server.address.invalid", detail = detail)
}

/// Adresse effective : celle réglée, sinon `OLLAMA_HOST`, sinon la valeur par
/// défaut. Une valeur invalide dans `OLLAMA_HOST` est ignorée.
pub fn effective(saved: &str, ollama_host: Option<&str>) -> String {
    configured(saved, ollama_host).url
}

/// Adresse de la machine choisie par les réglages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Configured {
    pub url: String,
    /// Réglée dans Settings, ou `OLLAMA_HOST` valide : jamais remplacée
    /// d'office par l'Ollama intégré.
    pub explicit: bool,
}

/// Comme [`effective`], en disant si l'adresse est explicite.
pub fn configured(saved: &str, ollama_host: Option<&str>) -> Configured {
    let explicit = normalize(saved)
        .ok()
        .flatten()
        .or_else(|| ollama_host.and_then(|value| normalize(value).ok().flatten()));
    match explicit {
        Some(url) => Configured {
            url,
            explicit: true,
        },
        None => Configured {
            url: default_url(),
            explicit: false,
        },
    }
}

/// Adresse par défaut. Builds de débogage et d'essai :
/// `CHATONLM_E2E_DEFAULT_URL` la remplace (adresse fermée : « rien
/// d'installé » sans dépendre d'un Ollama de la machine).
pub fn default_url() -> String {
    #[cfg(any(debug_assertions, feature = "e2e"))]
    if let Some(url) = std::env::var("CHATONLM_E2E_DEFAULT_URL")
        .ok()
        .and_then(|value| normalize(&value).ok().flatten())
    {
        return url;
    }
    DEFAULT_SERVER_URL.to_owned()
}

fn set_host(url: &mut Url, host: &str) -> Result<(), String> {
    url.set_host(Some(host)).map_err(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(value: &str) -> Option<String> {
        normalize(value).unwrap()
    }

    #[test]
    fn same_forms_as_ollama_host() {
        assert_eq!(norm(""), None);
        assert_eq!(norm("  "), None);
        assert_eq!(norm("1.2.3.4").unwrap(), "http://1.2.3.4:11434");
        assert_eq!(norm("1.2.3.4:1234").unwrap(), "http://1.2.3.4:1234");
        assert_eq!(norm("example.com").unwrap(), "http://example.com:11434");
        assert_eq!(norm("::1").unwrap(), "http://[::1]:11434");
        assert_eq!(norm("[::1]:1337").unwrap(), "http://[::1]:1337");
        assert_eq!(norm("http://1.2.3.4").unwrap(), "http://1.2.3.4");
        assert_eq!(norm("https://1.2.3.4").unwrap(), "https://1.2.3.4");
        assert_eq!(
            norm("https://example.com/ollama/").unwrap(),
            "https://example.com/ollama"
        );
    }

    #[test]
    fn explicit_ports_are_kept_even_the_scheme_default() {
        assert_eq!(norm("example.com:80").unwrap(), "http://example.com");
        assert_eq!(norm("localhost:80").unwrap(), "http://localhost");
        assert_eq!(norm(":80").unwrap(), "http://localhost");
        assert_eq!(norm("192.168.1.10:80").unwrap(), "http://192.168.1.10");
        assert_eq!(norm("http://example.com:80").unwrap(), "http://example.com");
        assert_eq!(
            norm("https://example.com:8443").unwrap(),
            "https://example.com:8443"
        );
    }

    #[test]
    fn ollama_special_cases() {
        assert_eq!(norm("ollama.com").unwrap(), "https://ollama.com");
        assert_eq!(
            norm("example.com/ollama").unwrap(),
            "http://example.com:11434/ollama"
        );
        assert_eq!(norm("[::1]").unwrap(), "http://[::1]:11434");
        assert_eq!(
            norm("::ffff:127.0.0.1").unwrap(),
            "http://[::ffff:7f00:1]:11434"
        );
    }

    #[test]
    fn api_suffixes_copied_from_other_tools_are_removed() {
        assert_eq!(
            norm("http://localhost:11434/v1").unwrap(),
            DEFAULT_SERVER_URL
        );
        assert_eq!(
            norm("http://localhost:11434/api/").unwrap(),
            DEFAULT_SERVER_URL
        );
        assert_eq!(
            norm("https://example.com/ollama/api").unwrap(),
            "https://example.com/ollama"
        );
        assert_eq!(
            norm("https://example.com/apis").unwrap(),
            "https://example.com/apis"
        );
    }

    #[test]
    fn a_port_alone_is_not_a_host() {
        let error = normalize("11435").unwrap_err();
        assert!(error.contains("localhost:11435"), "{error}");
        assert_eq!(effective("", Some("11435")), DEFAULT_SERVER_URL);
    }

    #[test]
    fn listening_addresses_mean_this_machine() {
        assert_eq!(norm("0.0.0.0").unwrap(), "http://127.0.0.1:11434");
        assert_eq!(norm("0.0.0.0:11435").unwrap(), "http://127.0.0.1:11435");
        assert_eq!(norm("[::]:8080").unwrap(), "http://[::1]:8080");
        assert_eq!(norm(":1234").unwrap(), "http://localhost:1234");
    }

    #[test]
    fn quotes_and_spaces_are_ignored() {
        assert_eq!(
            norm(" \"127.0.0.1:11435\" ").unwrap(),
            "http://127.0.0.1:11435"
        );
    }

    #[test]
    fn invalid_addresses_are_refused() {
        for value in [
            "ftp://example.com",
            "http://user:pass@example.com",
            "http://example.com/?a=1",
            "http://exa mple.com",
            "1.2.3.4:99999",
            "localhost:",
            "[::1",
            "[::1]x",
        ] {
            assert!(normalize(value).is_err(), "{value} should be refused");
        }
    }

    #[test]
    fn only_a_saved_address_or_ollama_host_is_explicit() {
        assert!(configured("192.168.1.2", None).explicit);
        assert!(configured("", Some("0.0.0.0")).explicit);
        // Même égale à l'adresse par défaut, une adresse réglée est explicite.
        assert!(configured("localhost:11434", None).explicit);
        let default = configured("", Some("ftp://nope"));
        assert!(!default.explicit);
        assert_eq!(default.url, DEFAULT_SERVER_URL);
        assert!(!configured("", None).explicit);
    }

    #[test]
    fn saved_address_then_ollama_host_then_default() {
        assert_eq!(
            effective("192.168.1.2", Some("0.0.0.0")),
            "http://192.168.1.2:11434"
        );
        assert_eq!(
            effective("", Some("0.0.0.0:11435")),
            "http://127.0.0.1:11435"
        );
        assert_eq!(effective("", Some("ftp://nope")), DEFAULT_SERVER_URL);
        assert_eq!(effective("", None), DEFAULT_SERVER_URL);
    }
}
