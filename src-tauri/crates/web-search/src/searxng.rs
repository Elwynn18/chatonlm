//! SearXNG, instance choisie par l'utilisateur (souvent la sienne) :
//! `GET <instance>/search?q=…&format=json` ; réponse `results[] {url, title,
//! content}`. Le format JSON n'est servi que s'il est permis par l'instance
//! (`search.formats` de sa configuration) : sinon 403, ou une page HTML
//! (protection anti-robots des instances publiques) → [`EngineError::JsonDisabled`].

use serde_json::Value;

use crate::text::RawResult;
use crate::{EngineError, Query};

/// Adresse de l'instance, normalisée : http(s), un hôte, sans `/search`,
/// requête ni fragment, ni `/` final. `None` : adresse inutilisable.
pub fn normalize(address: &str) -> Option<String> {
    let mut url = url::Url::parse(address.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/');
    let path = path.strip_suffix("/search").unwrap_or(path).to_owned();
    url.set_path(&path);
    Some(url.as_str().trim_end_matches('/').to_owned())
}

pub(crate) fn request(
    http: &reqwest::Client,
    base: &str,
    query: &Query<'_>,
) -> Result<reqwest::RequestBuilder, EngineError> {
    let mut url =
        url::Url::parse(&format!("{base}/search")).map_err(|_| EngineError::BadAddress)?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("q", query.text);
        pairs.append_pair("format", "json");
        pairs.append_pair("categories", "general");
        pairs.append_pair("safesearch", "1");
        pairs.append_pair("language", query.lang.unwrap_or("all"));
    }
    Ok(http.get(url).header("Accept", "application/json"))
}

/// Corps d'une réponse 200 : du JSON attendu ; une page HTML veut dire que
/// l'instance ne sert pas le format JSON.
pub fn parse_searxng(body: &[u8]) -> Result<Vec<RawResult>, EngineError> {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        let start = String::from_utf8_lossy(&body[..body.len().min(512)]).to_ascii_lowercase();
        return Err(if start.contains("<html") || start.contains("<!doctype") {
            EngineError::JsonDisabled
        } else {
            EngineError::Malformed
        });
    };
    let results = value
        .get("results")
        .and_then(Value::as_array)
        .ok_or(EngineError::Malformed)?;
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    Ok(results
        .iter()
        .map(|item| RawResult {
            title: text(&item["title"]),
            url: text(&item["url"]),
            snippets: vec![text(&item["content"])],
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_normalized() {
        assert_eq!(
            normalize(" https://searx.example.org/ ").as_deref(),
            Some("https://searx.example.org")
        );
        assert_eq!(
            normalize("http://localhost:8080/searxng/search?q=x#y").as_deref(),
            Some("http://localhost:8080/searxng")
        );
        assert_eq!(normalize("ftp://searx.example.org"), None);
        assert_eq!(normalize("searx.example.org"), None);
        assert_eq!(normalize(""), None);
    }

    #[test]
    fn json_replies_are_read_and_html_pages_mean_json_is_off() {
        let body = br#"{"query":"chat","number_of_results":0,"results":[
            {"url":"https://fr.wikipedia.org/wiki/Chat","title":"Chat","content":"Le chat &amp; le f\u00e9lin","engine":"wikipedia"},
            {"url":"https://example.org/","title":"Exemple"}
        ]}"#;
        let raw = parse_searxng(body).unwrap();
        assert_eq!(raw.len(), 2);
        assert_eq!(raw[0].snippets, ["Le chat &amp; le félin"]);
        assert_eq!(raw[1].snippets, [""]);
        assert_eq!(
            parse_searxng(b"<!doctype html>\n<html lang=\"en\"><head>"),
            Err(EngineError::JsonDisabled)
        );
        assert_eq!(
            parse_searxng(b"{\"answers\": []}"),
            Err(EngineError::Malformed)
        );
        assert_eq!(parse_searxng(b"oops"), Err(EngineError::Malformed));
    }
}
