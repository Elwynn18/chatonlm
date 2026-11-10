//! Lecture d'une page par l'app (`web_fetch`), gardée contre les requêtes
//! vers la machine ou son réseau (SSRF) :
//!
//! - http(s) seulement, sans identifiants dans l'adresse ;
//! - le nom est résolu par l'app, et TOUTES ses adresses doivent être
//!   publiques ([`is_public`] : ni boucle locale, ni réseau privé, ni lien
//!   local, ni ULA, ni 0/8, ni CGNAT, ni multidiffusion, ni plages réservées,
//!   y compris une IPv4 glissée dans une IPv6) ;
//! - la connexion va à l'adresse vérifiée (résolution imposée au client :
//!   pas de seconde résolution qui pourrait répondre autre chose), sans
//!   mandataire (il résoudrait lui-même le nom) ;
//! - redirections suivies à la main et revérifiées, [`MAX_REDIRECTS`] au
//!   plus ; taille lue bornée ([`MAX_BYTES`]) ; pages texte ou HTML
//!   seulement ; durée bornée par l'appelant, extraction du texte comprise
//!   (faite hors du fil async, arrêtée quand l'appelant abandonne : délai
//!   ou arrêt de l'envoi).
//!
//! Code commun à Linux et Windows.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use url::Url;

use super::page;

/// Redirections suivies au plus.
pub const MAX_REDIRECTS: usize = 5;
/// Octets lus d'une page au plus (ceux de l'officiel).
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Règles des adresses permises. L'app n'a que [`Policy::public`] ; les
/// essais y ajoutent un faux site sur la boucle locale.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    /// Noms résolus sans DNS (essais) ; un nom peut avoir plusieurs
    /// adresses (plusieurs lignes), dans l'ordre de la réponse.
    hosts: Vec<(String, IpAddr)>,
    /// Adresses permises bien que non publiques (essais : le faux site).
    exempt: Vec<SocketAddr>,
}

impl Policy {
    /// Adresses publiques seulement.
    pub const fn public() -> Policy {
        Policy {
            hosts: Vec::new(),
            exempt: Vec::new(),
        }
    }

    /// Essais : `host` résolu en `ip`, et `exempt` permise.
    #[cfg(test)]
    pub fn testing(hosts: &[(&str, IpAddr)], exempt: &[SocketAddr]) -> Policy {
        Policy {
            hosts: hosts
                .iter()
                .map(|(host, ip)| ((*host).to_owned(), *ip))
                .collect(),
            exempt: exempt.to_vec(),
        }
    }

    fn allows(&self, addr: SocketAddr) -> bool {
        is_public(addr.ip()) || self.exempt.contains(&addr)
    }
}

/// Échec d'une lecture (texte en anglais, donné au modèle comme ceux de
/// l'officiel).
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error("only http and https addresses can be read")]
    Scheme,
    #[error("invalid address")]
    Address,
    #[error("the address is not on the public internet")]
    NotPublic,
    #[error("the name could not be resolved")]
    Resolve,
    #[error("could not connect")]
    Connect,
    #[error("too many redirects")]
    Redirects,
    #[error("the server answered with status {0}")]
    Status(u16),
    #[error("unsupported content type: {0}")]
    ContentType(String),
    #[error("the page is larger than 4 MiB")]
    TooLarge,
    #[error("the reply was interrupted")]
    Interrupted,
}

/// Page lue.
#[derive(Debug, PartialEq, Eq)]
pub struct Page {
    pub title: String,
    pub text: String,
    pub links: Vec<String>,
}

/// L'adresse est sur Internet (ni la machine, ni un réseau local ou
/// réservé). Une IPv6 qui porte une IPv4 (`::ffff:a.b.c.d`, `::a.b.c.d`,
/// NAT64 `64:ff9b::/96`, 6to4 `2002::/16`) est jugée sur cette IPv4 ; le
/// NAT64 d'un réseau local (`64:ff9b:1::/48`) n'est jamais public.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => public_v6(ip),
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0                                   // 0.0.0.0/8 (« cette machine »)
        || a == 10                             // privé
        || a == 127                            // boucle locale
        || (a == 100 && (64..=127).contains(&b)) // CGNAT 100.64.0.0/10
        || (a == 169 && b == 254)              // lien local
        || (a == 172 && (16..=31).contains(&b)) // privé
        || (a == 192 && b == 168)              // privé
        || (a == 192 && b == 0 && c == 0)      // IETF 192.0.0.0/24
        || (a == 192 && b == 0 && c == 2)      // documentation
        || (a == 192 && b == 88 && c == 99)    // relais 6to4 (ancien)
        || (a == 198 && (b == 18 || b == 19))  // bancs d'essai 198.18.0.0/15
        || (a == 198 && b == 51 && c == 100)   // documentation
        || (a == 203 && b == 0 && c == 113)    // documentation
        || a >= 224) // multidiffusion 224/4, réservé 240/4, diffusion
}

fn public_v6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    let octets = ip.octets();
    let embedded = |offset: usize| {
        Ipv4Addr::new(
            octets[offset],
            octets[offset + 1],
            octets[offset + 2],
            octets[offset + 3],
        )
    };
    // IPv4 dans IPv6 : `::ffff:a.b.c.d` (mappée), `::a.b.c.d` (compatible,
    // `::` et `::1` compris), NAT64 `64:ff9b::/96`.
    if segments[..5] == [0; 5] && (segments[5] == 0xffff || segments[5] == 0) {
        return segments[5] == 0xffff && public_v4(embedded(12))
            || segments[5] == 0 && segments[6] != 0 && public_v4(embedded(12));
    }
    if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return public_v4(embedded(12));
    }
    // 6to4 : l'IPv4 suit `2002:`.
    if segments[0] == 0x2002 {
        return public_v4(embedded(2));
    }
    let first = segments[0];
    !((first & 0xfe00) == 0xfc00        // ULA fc00::/7
        || (first == 0x64 && segments[1] == 0xff9b && segments[2] == 1) // NAT64 local 64:ff9b:1::/48
        || (first & 0xffc0) == 0xfe80   // lien local fe80::/10
        || (first & 0xffc0) == 0xfec0   // site local (ancien) fec0::/10
        || (first & 0xff00) == 0xff00   // multidiffusion ff00::/8
        || (first == 0x2001 && segments[1] == 0x0db8) // documentation
        || (first == 0x2001 && segments[1] == 0)      // Teredo 2001::/32
        || (first == 0x0100 && segments[1..4] == [0, 0, 0]) // à jeter 100::/64
        || first == 0) // le reste de ::/16 (réservé)
}

/// Adresses de `host:port`, toutes vérifiées ; la première est celle où se
/// connecter.
async fn resolve(policy: &Policy, host: &str, port: u16) -> Result<SocketAddr, FetchError> {
    let literal = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    let named: Vec<SocketAddr> = policy
        .hosts
        .iter()
        .filter(|(name, _)| name == host)
        .map(|(_, ip)| SocketAddr::new(*ip, port))
        .collect();
    let addrs: Vec<SocketAddr> = if let Ok(ip) = literal.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else if !named.is_empty() {
        named
    } else {
        tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| FetchError::Resolve)?
            .collect()
    };
    if addrs.is_empty() {
        return Err(FetchError::Resolve);
    }
    // Une seule adresse non publique suffit à refuser : une réponse DNS
    // mêlée ne choisit pas pour nous.
    if !addrs.iter().all(|addr| policy.allows(*addr)) {
        return Err(FetchError::NotPublic);
    }
    Ok(addrs[0])
}

/// Vérifie une adresse à lire et rend l'hôte, le port et l'adresse IP où se
/// connecter.
async fn check(policy: &Policy, url: &Url) -> Result<SocketAddr, FetchError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(FetchError::Scheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(FetchError::Address);
    }
    let host = url.host_str().ok_or(FetchError::Address)?;
    let port = url.port_or_known_default().ok_or(FetchError::Address)?;
    resolve(policy, host, port).await
}

/// Client pour une seule requête : connexion à `addr` pour l'hôte de `url`,
/// sans mandataire, sans redirection suivie.
fn client(url: &Url, addr: SocketAddr) -> Result<reqwest::Client, FetchError> {
    crate::http::ensure_crypto_provider();
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(concat!("ChatonLM/", env!("CARGO_PKG_VERSION")));
    if let Some(host) = url.domain() {
        builder = builder.resolve(host, addr);
    }
    builder.build().map_err(|_| FetchError::Connect)
}

/// Sorte de contenu lisible : HTML, ou texte.
enum Kind {
    Html,
    Text,
}

fn kind_of(content_type: Option<&str>) -> Result<Kind, FetchError> {
    let essence = content_type
        .unwrap_or("text/plain")
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match essence.as_str() {
        "text/html" | "application/xhtml+xml" => Ok(Kind::Html),
        "" => Ok(Kind::Text),
        text if text.starts_with("text/") => Ok(Kind::Text),
        other => Err(FetchError::ContentType(other.chars().take(60).collect())),
    }
}

/// Lit la page `url` (adresse déjà permise par l'appelant).
pub async fn fetch(policy: &Policy, url: &str) -> Result<Page, FetchError> {
    let mut url = Url::parse(url).map_err(|_| FetchError::Address)?;
    for _ in 0..=MAX_REDIRECTS {
        let addr = check(policy, &url).await?;
        let mut response = client(&url, addr)?
            .get(url.clone())
            .header(
                reqwest::header::ACCEPT,
                "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.1",
            )
            .send()
            .await
            .map_err(|_| FetchError::Connect)?;
        let status = response.status();
        if status.is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(FetchError::Status(status.as_u16()))?;
            url = url.join(location).map_err(|_| FetchError::Address)?;
            continue;
        }
        if !status.is_success() {
            return Err(FetchError::Status(status.as_u16()));
        }
        let kind = kind_of(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
        )?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BYTES as u64)
        {
            return Err(FetchError::TooLarge);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| FetchError::Interrupted)?
        {
            if bytes.len() + chunk.len() > MAX_BYTES {
                return Err(FetchError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        return extract(bytes, url, kind).await;
    }
    Err(FetchError::Redirects)
}

/// Lève le drapeau d'arrêt quand l'attente est abandonnée (délai, arrêt).
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Texte de la page, calculé dans un fil bloquant : le délai et l'arrêt de
/// l'appelant rendent la main aussitôt, et le calcul s'arrête avec eux.
async fn extract(bytes: Vec<u8>, url: Url, kind: Kind) -> Result<Page, FetchError> {
    let stop = Arc::new(AtomicBool::new(false));
    let _stop = StopOnDrop(stop.clone());
    tokio::task::spawn_blocking(move || {
        let source = String::from_utf8_lossy(&bytes);
        match kind {
            Kind::Html => page::html_until(&source, &url, &stop),
            Kind::Text => page::plain(&source),
        }
    })
    .await
    .map_err(|_| FetchError::Interrupted)
}
