//! Releases GitHub de yt-dlp et de Deno : dernière version (redirection de
//! `releases/latest`, jamais l'API limitée à 60 requêtes par heure), sommes
//! et signature, taille de l'archive.
//!
//! - yt-dlp : `SHA2-256SUMS` (`<somme>  <nom>` par ligne), signé
//!   (`SHA2-256SUMS.sig`) par la clé épinglée de [`super::openpgp`] ; sur
//!   une autre clé, rien n'est installé (« mettez ChatonLM à jour »).
//! - Deno : `<archive>.sha256sum` à côté de l'archive, au format de Linux
//!   (`<somme>  <nom>`) ou à celui de PowerShell (`Hash : <SOMME>`, pour
//!   l'archive Windows) ; pas de signature : l'intégrité repose sur HTTPS
//!   seul (GitHub, redirections vers http refusées) et sur cette somme, qui
//!   vient de la même Release que l'archive. ChatonLM ne vérifie pas la
//!   signature Authenticode de `deno.exe`. Seule la branche 2.x est suivie
//!   (≥ 2.3.0, minimum de yt-dlp) : une 3.x publiée en dernier laisse la
//!   version installée, ou
//!   installe [`DENO_FALLBACK`] (une 3.0 ne sera prise qu'après un contrat
//!   vert dans une version de ChatonLM).
//! - Taille : `GET` avec `Range: bytes=0-0` (206 et `Content-Range`), qui
//!   marche aussi derrière les adresses signées où GitHub redirige.

use std::time::Duration;

use reqwest::StatusCode;
use url::Url;

use super::openpgp::{self, PublicKey, SigError};
use super::{Platform, ToolErrorCode, ToolId};

/// Dépôts officiels.
pub const YTDLP_REPO: &str = "https://github.com/yt-dlp/yt-dlp";
pub const DENO_REPO: &str = "https://github.com/denoland/deno";

/// Branche de Deno suivie, et version minimale exigée par yt-dlp
/// (`yt_dlp/utils/_jsruntime.py`).
pub const DENO_MAJOR: u64 = 2;
pub const DENO_MIN: [u64; 3] = [2, 3, 0];
/// Version de Deno prise si la dernière publiée n'est plus une 2.x (relevée
/// le 14 nov. 2026, contrat vert).
pub const DENO_FALLBACK: &str = "v2.9.8";

/// Taille maximale d'un fichier de sommes ou d'une signature.
pub const MAX_SMALL: usize = 64 * 1024;
/// Taille maximale annoncée pour une archive.
pub const MAX_ARCHIVE: u64 = 512 * 1024 * 1024;
/// Délai des petites requêtes.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Échec d'un relevé, d'un téléchargement ou d'une installation : nature
/// et détail (technique, en anglais, sauf [`ToolErrorCode::Space`] dont le
/// détail est déjà un message traduit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: ToolErrorCode,
    pub detail: String,
}

impl Failure {
    pub fn new(code: ToolErrorCode, detail: impl Into<String>) -> Failure {
        Failure {
            code,
            detail: detail.into(),
        }
    }
}

/// Adresses et clé imposées par les essais ; `None` : la production.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    pub yt_dlp: Option<String>,
    pub deno: Option<String>,
    /// Clé (armure) qui signe les sommes de yt-dlp à la place de la clé
    /// épinglée.
    pub pubkey: Option<String>,
}

/// Valeur imposée par les essais (jamais lue par le build de production).
#[cfg(any(test, debug_assertions, feature = "e2e"))]
fn test_override(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

#[cfg(not(any(test, debug_assertions, feature = "e2e")))]
fn test_override(_name: &str) -> Option<String> {
    None
}

impl Sources {
    pub fn from_env() -> Sources {
        Sources {
            yt_dlp: test_override("CHATONLM_YTDLP_RELEASES_URL"),
            deno: test_override("CHATONLM_DENO_RELEASES_URL"),
            pubkey: test_override("CHATONLM_TOOLS_PUBKEY"),
        }
    }

    /// Adresse du dépôt de `tool`, sans `/` final.
    pub fn repo(&self, tool: ToolId) -> String {
        let chosen = match tool {
            ToolId::YtDlp => self.yt_dlp.as_deref().unwrap_or(YTDLP_REPO),
            ToolId::Deno => self.deno.as_deref().unwrap_or(DENO_REPO),
        };
        chosen.trim().trim_end_matches('/').to_owned()
    }

    /// Clé qui doit avoir signé les sommes de yt-dlp.
    fn ytdlp_key(&self) -> Result<PublicKey, Failure> {
        match &self.pubkey {
            Some(armored) => PublicKey::parse(armored.as_bytes())
                .map_err(|error| Failure::new(ToolErrorCode::Signature, error.to_string())),
            None => Ok(openpgp::pinned_ytdlp()),
        }
    }
}

/// Étiquette sûre (dossier et adresse) : lettres, chiffres, `.`, `_`, `-`,
/// 64 caractères au plus, ni `.` ni `..`, pas de point en tête.
pub fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 64
        && !tag.starts_with('.')
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Étiquette lue dans l'adresse de redirection de `releases/latest`
/// (`…/releases/tag/<étiquette>`).
pub fn tag_from_location(location: &str) -> Option<String> {
    let path = location.split(['?', '#']).next()?;
    let (_, tag) = path.rsplit_once("/releases/tag/")?;
    valid_tag(tag).then(|| tag.to_owned())
}

/// Version montrée d'une étiquette (`v2.9.8` → `2.9.8`).
pub fn version_of(tool: ToolId, tag: &str) -> &str {
    match tool {
        ToolId::YtDlp => tag,
        ToolId::Deno => tag.strip_prefix('v').unwrap_or(tag),
    }
}

/// Parties numériques d'une version (`2026.10.26`, `v2.9.8`) ; autre
/// forme : `None`.
pub fn parts(version: &str) -> Option<Vec<u64>> {
    let version = version.strip_prefix('v').unwrap_or(version);
    version
        .split('.')
        .map(|part| {
            (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| part.parse().ok())
                .flatten()
        })
        .collect()
}

/// `candidate` est strictement plus récente que `current` (parties
/// numériques, zéros implicites : `2026.10.26` < `2026.10.26.1`) ; formes
/// incomparables : non.
pub fn newer(candidate: &str, current: &str) -> bool {
    let (Some(candidate), Some(current)) = (parts(candidate), parts(current)) else {
        return false;
    };
    let len = candidate.len().max(current.len());
    let pad = |list: &[u64]| {
        let mut list = list.to_vec();
        list.resize(len, 0);
        list
    };
    pad(&candidate) > pad(&current)
}

/// Version de Deno prise en charge : 2.x, 2.3.0 au moins.
pub fn usable_deno(version: &str) -> bool {
    parts(version).is_some_and(|parts| {
        parts.first() == Some(&DENO_MAJOR) && {
            let mut padded = parts.clone();
            padded.resize(3.max(parts.len()), 0);
            padded[..3] >= DENO_MIN[..]
        }
    })
}

/// Somme SHA-256 de `asset` dans un fichier de sommes : lignes
/// `<somme>  <nom>` (ou `<somme> *<nom>`), nom exact ; ou sortie de
/// PowerShell (`Hash : <SOMME>`, avec `Path : …\<nom>` s'il est là).
/// Rendue en minuscules.
pub fn sum_for(text: &str, asset: &str) -> Option<String> {
    let hex = |value: &str| {
        (value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| value.to_ascii_lowercase())
    };
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        if let (Some(sum), Some(name), None) = (fields.next(), fields.next(), fields.next())
            && name.strip_prefix('*').unwrap_or(name) == asset
            && let Some(sum) = hex(sum)
        {
            return Some(sum);
        }
    }
    // Format de PowerShell (`Get-FileHash | Format-List`).
    let mut hash = None;
    let mut path = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "hash" => hash = hex(value.trim()),
            "path" => path = Some(value.trim().to_owned()),
            "algorithm" if !value.trim().eq_ignore_ascii_case("sha256") => return None,
            _ => {}
        }
    }
    let named = path.is_none_or(|path| {
        path.rsplit(['\\', '/'])
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case(asset))
    });
    hash.filter(|_| named)
}

/// Archive d'une version, à télécharger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tool: ToolId,
    pub tag: String,
    pub asset: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// Adresse d'un fichier d'une Release.
pub fn download_url(repo: &str, tag: &str, file: &str) -> String {
    format!("{repo}/releases/download/{tag}/{file}")
}

/// Client des relevés ; dépôt en https (la production) : toute adresse et
/// toute redirection en http refusées (pas de rétrogradation vers un
/// transport en clair entre GitHub et ses adresses signées).
pub(crate) fn client(repo: &str, follow: bool) -> Result<reqwest::Client, Failure> {
    let url = Url::parse(repo)
        .map_err(|error| Failure::new(ToolErrorCode::Network, error.to_string()))?;
    let mut builder = crate::http::builder(&url, Some(TIMEOUT)).https_only(url.scheme() == "https");
    if !follow {
        builder = builder.redirect(reqwest::redirect::Policy::none());
    }
    builder
        .build()
        .map_err(|error| Failure::new(ToolErrorCode::Network, error.to_string()))
}

fn network(error: reqwest::Error) -> Failure {
    Failure::new(ToolErrorCode::Network, error.without_url().to_string())
}

/// Dernière étiquette publiée de `tool` (Deno : la dernière 2.x connue si
/// la dernière publiée n'en est plus une).
pub async fn latest_tag(tool: ToolId, sources: &Sources) -> Result<String, Failure> {
    let repo = sources.repo(tool);
    let response = client(&repo, false)?
        .get(format!("{repo}/releases/latest"))
        .send()
        .await
        .map_err(network)?;
    if !response.status().is_redirection() {
        // i18n-ignore : détail technique, dans un message traduit.
        return Err(Failure::new(
            ToolErrorCode::Network,
            format!("releases/latest: HTTP {}", response.status().as_u16()),
        ));
    }
    let tag = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .and_then(tag_from_location)
        // i18n-ignore : détail technique, dans un message traduit.
        .ok_or_else(|| Failure::new(ToolErrorCode::Release, "releases/latest: no tag"))?;
    if tool == ToolId::Deno && !usable_deno(&tag) {
        return Ok(DENO_FALLBACK.to_owned());
    }
    Ok(tag)
}

/// Petit fichier, [`MAX_SMALL`] au plus ; 404 : Release incomplète.
async fn get_small(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, Failure> {
    let mut response = client.get(url).send().await.map_err(network)?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        let name = url.rsplit('/').next().unwrap_or(url);
        // i18n-ignore : détail technique, dans un message traduit.
        return Err(Failure::new(
            ToolErrorCode::Release,
            format!("{name} missing"),
        ));
    }
    if !status.is_success() {
        // i18n-ignore : détail technique, dans un message traduit.
        return Err(Failure::new(
            ToolErrorCode::Network,
            format!("HTTP {}", status.as_u16()),
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_SMALL {
            // i18n-ignore : détail technique, dans un message traduit.
            return Err(Failure::new(ToolErrorCode::Release, "file too large"));
        }
    }
    Ok(body)
}

/// Taille d'un fichier distant, sans le télécharger : `Range: bytes=0-0`
/// (206 et `Content-Range: bytes 0-0/<taille>`), sinon `Content-Length`.
pub async fn remote_size(client: &reqwest::Client, url: &str) -> Result<u64, Failure> {
    let response = client
        .get(url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await
        .map_err(network)?;
    let status = response.status();
    let size = match status {
        StatusCode::PARTIAL_CONTENT => response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit_once('/'))
            .and_then(|(_, total)| total.trim().parse::<u64>().ok()),
        StatusCode::OK => response.content_length(),
        StatusCode::NOT_FOUND => {
            let name = url.rsplit('/').next().unwrap_or(url);
            // i18n-ignore : détail technique, dans un message traduit.
            return Err(Failure::new(
                ToolErrorCode::Release,
                format!("{name} missing"),
            ));
        }
        other => {
            // i18n-ignore : détail technique, dans un message traduit.
            return Err(Failure::new(
                ToolErrorCode::Network,
                format!("HTTP {}", other.as_u16()),
            ));
        }
    };
    match size {
        Some(size) if size > 0 && size <= MAX_ARCHIVE => Ok(size),
        // i18n-ignore : détail technique, dans un message traduit.
        Some(size) => Err(Failure::new(
            ToolErrorCode::Release,
            format!("archive of {size} bytes"),
        )),
        // i18n-ignore : détail technique, dans un message traduit.
        None => Err(Failure::new(ToolErrorCode::Network, "unknown size")),
    }
}

/// Détail d'une signature refusée ; [`OTHER_KEY`] : signée par une autre
/// clé (message « mettez ChatonLM à jour »).
fn signature_detail(error: &SigError) -> String {
    match error {
        SigError::OtherKey => OTHER_KEY.to_owned(),
        other => other.to_string(),
    }
}

/// Détail d'une signature d'une autre clé que celle attendue.
pub const OTHER_KEY: &str = "other key";

/// Secondes depuis l'époque Unix.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Somme vérifiée de l'archive de `tool` à `tag` (yt-dlp : signature des
/// sommes par la clé attendue).
pub async fn checksum(
    client: &reqwest::Client,
    tool: ToolId,
    tag: &str,
    asset: &str,
    sources: &Sources,
) -> Result<String, Failure> {
    let repo = sources.repo(tool);
    let text = match tool {
        ToolId::YtDlp => {
            let sums = get_small(client, &download_url(&repo, tag, "SHA2-256SUMS")).await?;
            let signature =
                get_small(client, &download_url(&repo, tag, "SHA2-256SUMS.sig")).await?;
            let key = sources.ytdlp_key()?;
            openpgp::verify_detached(&key, &sums, &signature, now()).map_err(|error| {
                Failure::new(ToolErrorCode::Signature, signature_detail(&error))
            })?;
            sums
        }
        ToolId::Deno => {
            get_small(
                client,
                &download_url(&repo, tag, &format!("{asset}.sha256sum")),
            )
            .await?
        }
    };
    sum_for(&String::from_utf8_lossy(&text), asset)
        // i18n-ignore : détail technique, dans un message traduit.
        .ok_or_else(|| Failure::new(ToolErrorCode::Release, format!("no checksum for {asset}")))
}

/// Archive de `tool` à `tag` pour `platform` : adresse, somme vérifiée,
/// taille.
pub async fn release(
    tool: ToolId,
    tag: &str,
    platform: Platform,
    sources: &Sources,
) -> Result<Release, Failure> {
    if !valid_tag(tag) {
        // i18n-ignore : détail technique, dans un message traduit.
        return Err(Failure::new(ToolErrorCode::Release, "invalid tag"));
    }
    let repo = sources.repo(tool);
    let client = client(&repo, true)?;
    let asset = tool.asset(platform);
    let sha256 = checksum(&client, tool, tag, asset, sources).await?;
    let url = download_url(&repo, tag, asset);
    let size = remote_size(&client, &url).await?;
    Ok(Release {
        tool,
        tag: tag.to_owned(),
        asset: asset.to_owned(),
        url,
        sha256,
        size,
    })
}
