//! Fournisseurs de recherche, d'après leur documentation officielle (relevée
//! le 8 nov. 2026, jamais essayés contre le vrai service faute de clé : banc
//! R0) :
//!
//! - **Brave Search API** : `GET https://api.search.brave.com/res/v1/web/search`,
//!   clé dans l'en-tête `X-Subscription-Token` ; paramètres `q`, `count`
//!   (20 au plus), `country` (2 lettres), `search_lang`, `safesearch`
//!   (`moderate` par défaut), `text_decorations` (marques `<strong>`),
//!   `result_filter`, `extra_snippets` ; réponse `web.results[]` `{title, url,
//!   description, extra_snippets?}`. 429 : trop de requêtes.
//! - **Mojeek** : `GET https://api.mojeek.com/search?q=…&api_key=…&fmt=json`,
//!   `t` (nombre de résultats), `lb`/`lbb` (langue favorisée, majuscules),
//!   `rb`/`rbb` (région), `safe` ; réponse `response.status` (`OK`, sinon
//!   `ERROR: …`, dont « Daily Limit Reached »), `response.results[]` `{url,
//!   title, desc}`.
//!
//! Secrets : la clé de Mojeek est dans l'adresse. Aucune erreur de `reqwest`
//! n'est donc gardée telle quelle (elle contient l'adresse) : seule sa sorte
//! ([`ProviderError`]) sort d'ici, sans texte.

use crate::budget::micro_eur;
use crate::config::{ProviderConfig, Secret};
use relay_proto::api::{Provider, Search, SearchResult};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Taille lue d'une réponse de fournisseur.
const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_TITLE_CHARS: usize = 300;
const MAX_CONTENT_CHARS: usize = 1_500;
const MAX_URL_LEN: usize = 2_048;

/// Échec d'un fournisseur, sans détail (ni adresse, ni corps).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderError {
    /// Injoignable (requête jamais partie), 5xx.
    Unavailable,
    /// Requête partie, réponse jamais reçue en entier (délai, coupure) : le
    /// fournisseur a pu la facturer.
    Lost,
    /// 429.
    RateLimited,
    /// 401, 403 : clé refusée.
    Auth,
    /// Crédit ou limite du fournisseur épuisé (402, « Daily Limit »).
    Exhausted,
    /// 4xx autre : requête refusée.
    Rejected,
    /// Réponse illisible.
    Malformed,
}

impl ProviderError {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderError::Unavailable => "unavailable",
            ProviderError::Lost => "no reply",
            ProviderError::RateLimited => "rate limited",
            ProviderError::Auth => "key refused",
            ProviderError::Exhausted => "credit exhausted",
            ProviderError::Rejected => "request rejected",
            ProviderError::Malformed => "malformed reply",
        }
    }

    /// Délai d'évitement après cet échec (une clé refusée ne revient pas
    /// toute seule : plus long).
    fn cooldown(self, base: u64) -> u64 {
        match self {
            ProviderError::Auth | ProviderError::Exhausted => base.saturating_mul(20),
            ProviderError::Rejected | ProviderError::Malformed => 0,
            ProviderError::Unavailable | ProviderError::Lost | ProviderError::RateLimited => base,
        }
    }

    /// L'appel a-t-il pu être facturé ? Une réponse reçue mais illisible, ou
    /// une requête partie sans réponse (délai dépassé, coupure) l'a peut-être
    /// été : gardée au budget, par prudence. Un refus, une erreur du
    /// fournisseur ou une connexion impossible non.
    pub fn billed(self) -> bool {
        matches!(self, ProviderError::Malformed | ProviderError::Lost)
    }
}

pub struct ProviderClient {
    pub provider: Provider,
    base_url: String,
    key: Secret,
    pub cost: u64,
    extra_snippets: bool,
    cooldown_secs: u64,
    down_until: AtomicU64,
    http: reqwest::Client,
}

impl ProviderClient {
    pub fn new(provider: Provider, config: &ProviderConfig, key: Secret) -> Result<Self, String> {
        let timeout = Duration::from_secs(config.timeout_secs.max(1));
        let http = reqwest::Client::builder()
            .timeout(timeout)
            // Connexion bornée avant la fin du délai total : une connexion
            // qui traîne (hôte muet, port fermé sous Windows qui répond par
            // un délai) finit en erreur de connexion, donc non facturée,
            // plutôt qu'en délai dépassé, compté comme une requête partie.
            .connect_timeout(Duration::from_secs(5).min(timeout / 2))
            .gzip(true)
            // Aucune redirection : une réponse 3xx n'est pas suivie (la clé
            // de Brave est dans un en-tête, celle de Mojeek dans l'adresse).
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("chatonlm-relay/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| format!("{}: cannot build the HTTP client", provider.as_str()))?;
        Ok(Self {
            provider,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
            key,
            cost: micro_eur(config.cost_per_1000_eur / 1000.0),
            extra_snippets: config.extra_snippets,
            cooldown_secs: config.cooldown_secs,
            down_until: AtomicU64::new(0),
            http,
        })
    }

    pub fn is_configured(&self) -> bool {
        !self.key.is_empty()
    }

    pub fn is_down(&self, now: u64) -> bool {
        self.down_until.load(Ordering::Relaxed) > now
    }

    pub fn mark_failure(&self, error: ProviderError, now: u64) {
        let cooldown = error.cooldown(self.cooldown_secs);
        if cooldown > 0 {
            self.down_until.store(now + cooldown, Ordering::Relaxed);
        }
    }

    pub async fn search(&self, search: &Search) -> Result<Vec<SearchResult>, ProviderError> {
        Ok(shape(
            self.search_raw(search).await?,
            usize::from(search.max_results),
        ))
    }

    /// Résultats tels que le fournisseur les donne (extraits séparés), avant
    /// [`shape`] : le banc R0 compte les extraits supplémentaires de Brave.
    pub async fn search_raw(&self, search: &Search) -> Result<Vec<RawResult>, ProviderError> {
        let request = match self.provider {
            Provider::Brave => self.brave_request(search),
            Provider::Mojeek => self.mojeek_request(search),
        }?;
        let body = send(request).await?;
        let value: Value = serde_json::from_slice(&body).map_err(|_| ProviderError::Malformed)?;
        match self.provider {
            Provider::Brave => parse_brave(&value),
            Provider::Mojeek => parse_mojeek(&value),
        }
    }

    fn brave_request(&self, search: &Search) -> Result<reqwest::RequestBuilder, ProviderError> {
        let mut url = url::Url::parse(&format!("{}/res/v1/web/search", self.base_url))
            .map_err(|_| ProviderError::Rejected)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("q", &search.query);
            query.append_pair("count", &search.max_results.to_string());
            query.append_pair("safesearch", "moderate");
            query.append_pair("text_decorations", "false");
            query.append_pair("result_filter", "web");
            if let Some(lang) = search.lang.as_deref().and_then(brave_lang) {
                query.append_pair("search_lang", lang);
            }
            if let Some(country) = &search.country {
                query.append_pair("country", country);
            }
            if self.extra_snippets {
                query.append_pair("extra_snippets", "true");
            }
        }
        Ok(self
            .http
            .get(url)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", self.key.expose()))
    }

    fn mojeek_request(&self, search: &Search) -> Result<reqwest::RequestBuilder, ProviderError> {
        let mut url = url::Url::parse(&format!("{}/search", self.base_url))
            .map_err(|_| ProviderError::Rejected)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("q", &search.query);
            query.append_pair("api_key", self.key.expose());
            query.append_pair("fmt", "json");
            query.append_pair("t", &search.max_results.to_string());
            query.append_pair("safe", "1");
            if let Some(lang) = &search.lang {
                query.append_pair("lb", &lang.to_ascii_uppercase());
                query.append_pair("lbb", "100");
            }
            if let Some(country) = &search.country {
                query.append_pair("rb", country);
                query.append_pair("rbb", "10");
            }
        }
        Ok(self.http.get(url).header("Accept", "application/json"))
    }
}

/// Envoie la requête et rend le corps d'une réponse 200, sinon la sorte
/// d'échec (rien n'est lu de l'erreur de `reqwest` : elle contient l'adresse).
pub(crate) async fn send(request: reqwest::RequestBuilder) -> Result<Vec<u8>, ProviderError> {
    // Seule une erreur de connexion dit que la requête n'est pas partie.
    let response = request.send().await.map_err(|error| {
        if error.is_connect() || error.is_builder() {
            ProviderError::Unavailable
        } else {
            ProviderError::Lost
        }
    })?;
    let status = response.status().as_u16();
    let body = read_limited(response).await?;
    match status {
        200 => Ok(body),
        401 | 403 => Err(ProviderError::Auth),
        402 => Err(ProviderError::Exhausted),
        429 => Err(ProviderError::RateLimited),
        400..=499 => Err(ProviderError::Rejected),
        _ => Err(ProviderError::Unavailable),
    }
}

async fn read_limited(mut response: reqwest::Response) -> Result<Vec<u8>, ProviderError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ProviderError::Lost)? {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(ProviderError::Malformed);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Résultat brut d'un fournisseur.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawResult {
    pub title: String,
    pub url: String,
    pub snippets: Vec<String>,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

pub fn parse_brave(value: &Value) -> Result<Vec<RawResult>, ProviderError> {
    if !value.is_object() {
        return Err(ProviderError::Malformed);
    }
    // Pas de bloc `web` : aucun résultat web (réponse valable).
    let Some(results) = value.get("web").and_then(|web| web.get("results")) else {
        return Ok(Vec::new());
    };
    let results = results.as_array().ok_or(ProviderError::Malformed)?;
    Ok(results
        .iter()
        .map(|item| {
            let mut snippets = vec![text(&item["description"])];
            if let Some(extra) = item["extra_snippets"].as_array() {
                snippets.extend(extra.iter().map(text));
            }
            RawResult {
                title: text(&item["title"]),
                url: text(&item["url"]),
                snippets,
            }
        })
        .collect())
}

pub fn parse_mojeek(value: &Value) -> Result<Vec<RawResult>, ProviderError> {
    let response = value.get("response").ok_or(ProviderError::Malformed)?;
    let status = response
        .get("status")
        .and_then(Value::as_str)
        .ok_or(ProviderError::Malformed)?;
    if status != "OK" {
        let status = status.to_ascii_lowercase();
        return Err(if status.contains("limit") {
            ProviderError::Exhausted
        } else if status.contains("key") || status.contains("auth") {
            // « Invalid API key », clé expirée : 10 min d'écart, pas 30 s.
            ProviderError::Auth
        } else {
            ProviderError::Unavailable
        });
    }
    let Some(results) = response.get("results") else {
        return Ok(Vec::new());
    };
    let results = results.as_array().ok_or(ProviderError::Malformed)?;
    Ok(results
        .iter()
        .map(|item| RawResult {
            title: text(&item["title"]),
            url: text(&item["url"]),
            snippets: vec![text(&item["desc"])],
        })
        .collect())
}

/// Langues de recherche de Brave (paramètre `search_lang`, documentation
/// officielle) : ce ne sont pas toutes des codes ISO 639-1 (`jp`, `pt-br`,
/// `zh-hans`…). Une langue absente de la liste n'est pas envoyée (Brave
/// répondrait 422 et la recherche passerait toujours à Mojeek). Liste à
/// revérifier au banc R0 contre le vrai service.
pub fn brave_lang(lang: &str) -> Option<&'static str> {
    const SAME: [&str; 44] = [
        "ar", "eu", "bn", "bg", "ca", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "gl", "de",
        "gu", "he", "hi", "hu", "is", "it", "kn", "ko", "lv", "lt", "ms", "ml", "mr", "nb", "pl",
        "pa", "ro", "ru", "sr", "sk", "sl", "es", "sv", "ta", "te", "th", "tr", "uk", "vi",
    ];
    match lang {
        "ja" => Some("jp"),
        "pt" => Some("pt-br"),
        "zh" => Some("zh-hans"),
        "no" | "nn" => Some("nb"),
        _ => SAME.iter().find(|code| **code == lang).copied(),
    }
}

/// Réponse réduite à ce dont l'app a besoin : adresses http(s) seulement,
/// sans doublon, texte sans balises, longueurs bornées.
pub fn shape(raw: Vec<RawResult>, max: usize) -> Vec<SearchResult> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for item in raw {
        if out.len() >= max {
            break;
        }
        let url = item.url.trim();
        let scheme_ok = url
            .get(..8)
            .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
            || url
                .get(..7)
                .is_some_and(|s| s.eq_ignore_ascii_case("http://"));
        if !scheme_ok
            || url.len() > MAX_URL_LEN
            || url.chars().any(|c| c.is_whitespace() || c.is_control())
            || url::Url::parse(url)
                .map(|parsed| parsed.host_str().is_none())
                .unwrap_or(true)
            || !seen.insert(url.to_owned())
        {
            continue;
        }
        let title = truncate(&clean(&item.title), MAX_TITLE_CHARS);
        let content = truncate(
            &item
                .snippets
                .iter()
                .map(|s| clean(s))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" … "),
            MAX_CONTENT_CHARS,
        );
        out.push(SearchResult {
            title: if title.is_empty() {
                url.to_owned()
            } else {
                title
            },
            url: url.to_owned(),
            content,
        });
    }
    out
}

/// Décode les entités courantes, puis retire les balises (une balise encodée,
/// `&lt;img …&gt;`, est donc retirée aussi), les caractères de contrôle et de
/// mise en forme bidirectionnelle, et resserre les espaces. Le texte rendu
/// reste du texte : l'app ne le montre jamais en HTML ni en Markdown.
pub fn clean(text: &str) -> String {
    let decoded = decode_entities(text);
    strip_tags(&decoded)
        .chars()
        .filter(|c| !is_hidden(*c))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Contrôles (sauf les espaces) et forçages de sens d'écriture (U+202E…), qui
/// retourneraient l'affichage d'un titre.
fn is_hidden(c: char) -> bool {
    (c.is_control() && !c.is_whitespace())
        || matches!(
            c,
            '\u{200e}' | '\u{200f}' | '\u{061c}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}

/// Retire `<…>` seulement si `<` ouvre vraiment une balise (lettre, `/`, `!`,
/// `?`) et qu'elle se ferme : `3 < 5` ou `<3` restent tels quels.
fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let opens = after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
        match after.find('>') {
            Some(end) if opens => rest = &after[end + 1..],
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        // Recherche dans les octets : couper la chaîne à l'octet 12 tomberait
        // au milieu d'un caractère (« & Jérôme à… ») et paniquerait. `;` est
        // de l'ASCII : `rest[1..end]` reste sur une frontière.
        let end = rest.as_bytes()[..rest.len().min(12)]
            .iter()
            .position(|&b| b == b';');
        let decoded = end.and_then(|end| {
            let name = &rest[1..end];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                    .and_then(char::from_u32)
                    .filter(|c| !c.is_control()),
            }?;
            Some((c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", text[..at].trim_end()),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn brave_reply_is_read() {
        let value = json!({
            "query": {"original": "chat"},
            "web": {"results": [
                {"title": "Le <strong>chat</strong>", "url": "https://fr.wikipedia.org/wiki/Chat",
                 "description": "Le chat &amp; ses <strong>moustaches</strong>", "extra_snippets": ["Félin."]},
                {"title": "Sans extraits", "url": "https://example.org/", "description": "x"}
            ]}
        });
        let raw = parse_brave(&value).unwrap();
        assert_eq!(
            raw[0].snippets,
            ["Le chat &amp; ses <strong>moustaches</strong>", "Félin."]
        );
        let shaped = shape(raw, 5);
        assert_eq!(shaped[0].title, "Le chat");
        assert_eq!(shaped[0].content, "Le chat & ses moustaches … Félin.");
        assert_eq!(shaped.len(), 2);
        assert_eq!(parse_brave(&json!({"query": {}})).unwrap(), []);
        assert_eq!(parse_brave(&json!([1])), Err(ProviderError::Malformed));
        assert_eq!(
            parse_brave(&json!({"web": {"results": 3}})),
            Err(ProviderError::Malformed)
        );
    }

    #[test]
    fn mojeek_reply_is_read() {
        let value = json!({"response": {"status": "OK", "head": {}, "results": [
            {"url": "https://www.mojeek.com/", "title": "Mojeek", "desc": "Moteur &#233;thique"}
        ]}});
        let shaped = shape(parse_mojeek(&value).unwrap(), 5);
        assert_eq!(shaped[0].content, "Moteur éthique");
        let limit = json!({"response": {"status": "ERROR: Daily Limit Reached."}});
        assert_eq!(parse_mojeek(&limit), Err(ProviderError::Exhausted));
        let key = json!({"response": {"status": "ERROR: Invalid key"}});
        assert_eq!(parse_mojeek(&key), Err(ProviderError::Auth));
        let other = json!({"response": {"status": "ERROR: Internal error"}});
        assert_eq!(parse_mojeek(&other), Err(ProviderError::Unavailable));
        assert_eq!(parse_mojeek(&json!({})), Err(ProviderError::Malformed));
    }

    #[test]
    fn shaping_drops_bad_addresses_and_duplicates_and_bounds_lengths() {
        let item = |url: &str| RawResult {
            title: "t".into(),
            url: url.into(),
            snippets: vec!["s".into()],
        };
        let raw = vec![
            item("javascript:alert(1)"),
            item("file:///etc/passwd"),
            item("https://a.example/"),
            item("https://a.example/"),
            item("http://b.example/x y"),
            item("https://"),
            item(&format!("https://c.example/{}", "a".repeat(MAX_URL_LEN))),
            item("HTTPS://d.example/"),
            item("https://e.example/"),
        ];
        let shaped = shape(raw, 2);
        let urls: Vec<_> = shaped.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(urls, ["https://a.example/", "HTTPS://d.example/"]);
        let long = RawResult {
            title: "é".repeat(400),
            url: "https://f.example/".into(),
            snippets: vec!["z".repeat(4000)],
        };
        let shaped = shape(vec![long], 5);
        assert_eq!(shaped[0].title.chars().count(), MAX_TITLE_CHARS + 1);
        assert_eq!(shaped[0].content.chars().count(), MAX_CONTENT_CHARS + 1);
        let untitled = RawResult {
            title: "<b></b>".into(),
            url: "https://g.example/".into(),
            snippets: vec![],
        };
        assert_eq!(shape(vec![untitled], 1)[0].title, "https://g.example/");
    }

    #[test]
    fn cleaning() {
        assert_eq!(
            clean("a <b>b</b>\n\n c&nbsp;&lt;d&gt; &unknown; & &#x1F431; &#0;"),
            "a b c &unknown; & 🐱 &#0;"
        );
    }

    /// B1 : un `&` suivi de caractères non ASCII ne fait plus paniquer.
    #[test]
    fn ampersands_before_non_ascii_text() {
        assert_eq!(clean("Tom & Jérôme à Paris"), "Tom & Jérôme à Paris");
        assert_eq!(clean("Q&Aの使い方"), "Q&Aの使い方");
        assert_eq!(clean("&éèàùç;&é"), "&éèàùç;&é");
        assert_eq!(clean("R&D – 研究開発 &amp; 方"), "R&D – 研究開発 & 方");
    }

    /// B1 : 10 000 chaînes Unicode quelconques autour de `&`, `;`, `<` : aucune
    /// panique, rien de caché qui reste.
    #[test]
    fn cleaning_never_panics() {
        const POOL: [&str; 24] = [
            "&", ";", "#", "x", "<", ">", "/", "a", "Z", "1", "é", "à", "方", "の", "🐱", " ",
            "amp", "lt", "\u{202e}", "\u{200f}", "\u{0007}", "\n", "&#", "Jérôme",
        ];
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..10_000 {
            let length = (next() % 40) as usize;
            let text: String = (0..length)
                .map(|_| POOL[(next() % POOL.len() as u64) as usize])
                .collect();
            let cleaned = clean(&text);
            assert!(!cleaned.chars().any(is_hidden), "{text:?} → {cleaned:?}");
        }
    }

    /// M4 : un `<` seul n'avale plus le reste ; une balise encodée ne revient
    /// pas ; les forçages de sens d'écriture disparaissent.
    #[test]
    fn cleaning_keeps_text_and_drops_encoded_tags() {
        assert_eq!(clean("3 < 5 et 7 > 6"), "3 < 5 et 7 > 6");
        assert_eq!(clean("I <3 cats"), "I <3 cats");
        assert_eq!(clean("a <b sans fin"), "a <b sans fin");
        assert_eq!(clean("&lt;img src=x onerror=alert(1)&gt;Titre"), "Titre");
        assert_eq!(clean("x &lt; y"), "x < y");
        assert_eq!(clean("abc\u{202e}fed\u{2066}\u{200f}"), "abcfed");
        assert_eq!(clean("a\u{0007}b"), "ab");
    }

    /// M5 : codes de langue de Brave.
    #[test]
    fn brave_languages() {
        assert_eq!(brave_lang("fr"), Some("fr"));
        assert_eq!(brave_lang("ja"), Some("jp"));
        assert_eq!(brave_lang("pt"), Some("pt-br"));
        assert_eq!(brave_lang("zh"), Some("zh-hans"));
        assert_eq!(brave_lang("xx"), None);
    }

    /// M5 : seule une requête jamais partie est rendue au budget.
    #[test]
    fn what_is_billed() {
        assert!(ProviderError::Lost.billed() && ProviderError::Malformed.billed());
        for error in [
            ProviderError::Unavailable,
            ProviderError::RateLimited,
            ProviderError::Auth,
            ProviderError::Exhausted,
            ProviderError::Rejected,
        ] {
            assert!(!error.billed(), "{error:?}");
        }
    }
}
