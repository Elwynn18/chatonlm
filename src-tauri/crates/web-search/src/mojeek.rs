//! Mojeek, d'après sa documentation officielle (relevée le 8 nov. 2026) :
//! `GET https://api.mojeek.com/search?q=…&api_key=…&fmt=json`, `t` (nombre de
//! résultats), `lb`/`lbb` (langue favorisée, majuscules), `safe` ; réponse
//! `response.status` (`OK`, sinon `ERROR: …`, dont « Daily Limit Reached »),
//! `response.results[]` `{url, title, desc}`. La clé est dans l'adresse :
//! aucune erreur de `reqwest` n'est gardée telle quelle. Analyseur déplacé du
//! relais (`relay-server`).

use serde_json::Value;

use crate::text::RawResult;
use crate::{EngineError, Query};

pub const DEFAULT_URL: &str = "https://api.mojeek.com";

pub(crate) fn request(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    query: &Query<'_>,
) -> Result<reqwest::RequestBuilder, EngineError> {
    let mut url = url::Url::parse(&format!("{base}/search")).map_err(|_| EngineError::Rejected)?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("q", query.text);
        pairs.append_pair("api_key", key);
        pairs.append_pair("fmt", "json");
        pairs.append_pair("t", &query.max_results.to_string());
        pairs.append_pair("safe", "1");
        if let Some(lang) = query.lang {
            pairs.append_pair("lb", &lang.to_ascii_uppercase());
            pairs.append_pair("lbb", "100");
        }
    }
    Ok(http.get(url).header("Accept", "application/json"))
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

pub fn parse_mojeek(value: &Value) -> Result<Vec<RawResult>, EngineError> {
    let response = value.get("response").ok_or(EngineError::Malformed)?;
    let status = response
        .get("status")
        .and_then(Value::as_str)
        .ok_or(EngineError::Malformed)?;
    if status != "OK" {
        let status = status.to_ascii_lowercase();
        return Err(if status.contains("limit") {
            EngineError::Exhausted
        } else if status.contains("key") || status.contains("auth") {
            EngineError::Auth
        } else {
            EngineError::Unavailable
        });
    }
    let Some(results) = response.get("results") else {
        return Ok(Vec::new());
    };
    let results = results.as_array().ok_or(EngineError::Malformed)?;
    Ok(results
        .iter()
        .map(|item| RawResult {
            title: text(&item["title"]),
            url: text(&item["url"]),
            snippets: vec![text(&item["desc"])],
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::shape;
    use serde_json::json;

    #[test]
    fn mojeek_reply_is_read() {
        let value = json!({"response": {"status": "OK", "head": {}, "results": [
            {"url": "https://www.mojeek.com/", "title": "Mojeek", "desc": "Moteur &#233;thique"}
        ]}});
        let shaped = shape(parse_mojeek(&value).unwrap(), 5);
        assert_eq!(shaped[0].content, "Moteur éthique");
        let limit = json!({"response": {"status": "ERROR: Daily Limit Reached."}});
        assert_eq!(parse_mojeek(&limit), Err(EngineError::Exhausted));
        let key = json!({"response": {"status": "ERROR: Invalid key"}});
        assert_eq!(parse_mojeek(&key), Err(EngineError::Auth));
        let other = json!({"response": {"status": "ERROR: Internal error"}});
        assert_eq!(parse_mojeek(&other), Err(EngineError::Unavailable));
        assert_eq!(parse_mojeek(&json!({})), Err(EngineError::Malformed));
    }
}
