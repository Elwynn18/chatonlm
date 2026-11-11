//! Brave Search API, d'après sa documentation officielle (relevée le 8 nov.
//! 2026) : `GET https://api.search.brave.com/res/v1/web/search`, clé dans
//! l'en-tête `X-Subscription-Token` ; paramètres `q`, `count` (20 au plus),
//! `search_lang`, `safesearch` (`moderate`), `text_decorations`,
//! `result_filter` ; réponse `web.results[]` `{title, url, description,
//! extra_snippets?}`. 401/403 : clé refusée ; 402 : crédit épuisé ; 429 :
//! trop de requêtes. Analyseur déplacé du relais (`relay-server`).

use serde_json::Value;

use crate::text::RawResult;
use crate::{EngineError, Query};

pub const DEFAULT_URL: &str = "https://api.search.brave.com";

pub(crate) fn request(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    query: &Query<'_>,
) -> Result<reqwest::RequestBuilder, EngineError> {
    let mut url =
        url::Url::parse(&format!("{base}/res/v1/web/search")).map_err(|_| EngineError::Rejected)?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("q", query.text);
        pairs.append_pair("count", &query.max_results.to_string());
        pairs.append_pair("safesearch", "moderate");
        pairs.append_pair("text_decorations", "false");
        pairs.append_pair("result_filter", "web");
        if let Some(lang) = query.lang.and_then(brave_lang) {
            pairs.append_pair("search_lang", lang);
        }
    }
    Ok(http
        .get(url)
        .header("Accept", "application/json")
        .header("X-Subscription-Token", key))
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

pub fn parse_brave(value: &Value) -> Result<Vec<RawResult>, EngineError> {
    if !value.is_object() {
        return Err(EngineError::Malformed);
    }
    // Pas de bloc `web` : aucun résultat web (réponse valable).
    let Some(results) = value.get("web").and_then(|web| web.get("results")) else {
        return Ok(Vec::new());
    };
    let results = results.as_array().ok_or(EngineError::Malformed)?;
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

/// Langues de recherche de Brave (paramètre `search_lang`, documentation
/// officielle) : ce ne sont pas toutes des codes ISO 639-1 (`jp`, `pt-br`,
/// `zh-hans`…). Une langue absente de la liste n'est pas envoyée (Brave
/// répondrait 422).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::shape;
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
        assert_eq!(parse_brave(&json!([1])), Err(EngineError::Malformed));
        assert_eq!(
            parse_brave(&json!({"web": {"results": 3}})),
            Err(EngineError::Malformed)
        );
    }

    #[test]
    fn brave_languages() {
        assert_eq!(brave_lang("fr"), Some("fr"));
        assert_eq!(brave_lang("ja"), Some("jp"));
        assert_eq!(brave_lang("pt"), Some("pt-br"));
        assert_eq!(brave_lang("zh"), Some("zh-hans"));
        assert_eq!(brave_lang("xx"), None);
    }
}
