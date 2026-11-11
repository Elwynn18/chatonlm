//! DuckDuckGo sans compte ni clé : la page HTML de `html.duckduckgo.com`,
//! demandée en POST (`q`, `kl`, `b`), comme la bibliothèque `ddgs` (MIT,
//! deedy5) dont se servent Open WebUI et AnythingLLM. Relevé et essais réels
//! du 11 nov. 2026 : `.tools/research/104-recherche-locale.md`.
//!
//! Lecture simple, sans arbre du document (comme `web_fetch` dans l'app) :
//! un résultat = un bloc `class="result …"` ; titre et adresse du lien
//! `result__a`, extrait de l'élément `result__snippet`. Annonces écartées
//! (`result--ad`, liens `duckduckgo.com/y.js`) ; anciens liens de redirection
//! `duckduckgo.com/l/?uddg=…` décodés. Blocage (page « anomaly », défi,
//! statut 202) : [`EngineError::Blocked`].

use crate::text::{RawResult, decode_entities};
use crate::{EngineError, Query};

pub const DEFAULT_URL: &str = "https://html.duckduckgo.com";

pub(crate) fn request(
    http: &reqwest::Client,
    base: &str,
    query: &Query<'_>,
) -> Result<reqwest::RequestBuilder, EngineError> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("q", query.text)
        .append_pair("kl", query.lang.and_then(region).unwrap_or(""))
        .append_pair("b", "")
        .finish();
    Ok(http
        .post(format!("{base}/html/"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "text/html")
        .body(body))
}

/// Région de DuckDuckGo (`kl`, valeurs du formulaire de la page) pour une
/// langue ; aucune : toutes les régions.
pub fn region(lang: &str) -> Option<&'static str> {
    Some(match lang {
        "en" => "us-en",
        "fr" => "fr-fr",
        "es" => "es-es",
        "de" => "de-de",
        "it" => "it-it",
        "pt" => "pt-pt",
        "nl" => "nl-nl",
        _ => return None,
    })
}

/// Page de défi ou de blocage (« anomaly ») plutôt que de résultats.
pub fn is_challenge(html: &str) -> bool {
    [
        "anomaly-modal",
        "anomaly.js",
        "challenge-form",
        "bots use DuckDuckGo",
    ]
    .iter()
    .any(|marker| html.contains(marker))
}

/// Résultats d'une page de `html.duckduckgo.com`.
pub fn parse_duckduckgo(html: &str) -> Result<Vec<RawResult>, EngineError> {
    if is_challenge(html) {
        return Err(EngineError::Blocked);
    }
    let mut out = Vec::new();
    for block in html.split("class=\"result ").skip(1) {
        let classes = block.split('"').next().unwrap_or_default();
        if classes.contains("result--ad") || classes.contains("result--no-result") {
            continue;
        }
        // Le bloc va jusqu'au suivant : un lien de l'un n'est jamais pris pour
        // celui d'un autre.
        let Some((tag, title)) = element(block, "result__a") else {
            continue;
        };
        let Some(url) = attribute(tag, "href").and_then(|href| target(&href)) else {
            continue;
        };
        let snippet = element(block, "result__snippet")
            .map(|(_, inner)| inner.to_owned())
            .unwrap_or_default();
        out.push(RawResult {
            title: title.to_owned(),
            url,
            snippets: vec![snippet],
        });
    }
    // Aucune page de résultats reconnue (ni la liste, ni « aucun résultat ») :
    // page changée ou inattendue, pas une recherche vide.
    if out.is_empty() && !html.contains("id=\"links\"") && !html.contains("no-results") {
        return Err(EngineError::Malformed);
    }
    Ok(out)
}

/// Balise ouvrante et contenu du premier élément de classe `class` (le nom
/// de classe exact, dans `class="…"`).
fn element<'a>(html: &'a str, class: &str) -> Option<(&'a str, &'a str)> {
    let marker = format!("class=\"{class}\"");
    let at = html.find(&marker)?;
    let start = html[..at].rfind('<')?;
    let name: String = html[start + 1..]
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    if name.is_empty() {
        return None;
    }
    let open_end = at + html[at..].find('>')?;
    let tag = &html[start..open_end];
    let inner_start = open_end + 1;
    let close = format!("</{name}>");
    let inner_end = inner_start + html[inner_start..].find(&close)?;
    Some((tag, &html[inner_start..inner_end]))
}

/// Valeur d'un attribut entre guillemets doubles, entités décodées.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let marker = format!(" {name}=\"");
    let at = tag.find(&marker)? + marker.len();
    let end = at + tag[at..].find('"')?;
    Some(decode_entities(&tag[at..end]))
}

/// Adresse de destination d'un lien de résultat : directe, ou décodée d'une
/// redirection `duckduckgo.com/l/?uddg=` ; `None` pour une annonce ou un
/// lien interne de DuckDuckGo.
fn target(href: &str) -> Option<String> {
    let absolute = if href.starts_with("//") {
        format!("https:{href}")
    } else if href.starts_with('/') {
        format!("https://duckduckgo.com{href}")
    } else {
        href.to_owned()
    };
    let url = url::Url::parse(&absolute).ok()?;
    let host = url.host_str()?;
    if host == "duckduckgo.com" || host.ends_with(".duckduckgo.com") {
        if url.path() == "/l/" {
            return url
                .query_pairs()
                .find(|(key, _)| key == "uddg")
                .map(|(_, value)| value.into_owned());
        }
        return None;
    }
    Some(absolute)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::shape;

    /// Page réelle du 11 nov. 2026 (POST `q=chaton roux`, `kl=fr-fr`),
    /// réduite à ses trois premiers résultats.
    const REAL_PAGE: &str = include_str!("../tests/fixtures/duckduckgo-fr.html");

    #[test]
    fn a_real_page_is_read() {
        let raw = parse_duckduckgo(REAL_PAGE).unwrap();
        assert_eq!(raw.len(), 3);
        assert_eq!(
            raw[0].url,
            "https://www.chatsdumonde.com/anatomie/race-chat-roux-20912.php"
        );
        let shaped = shape(raw, 5);
        assert_eq!(
            shaped[0].title,
            "Les chats roux : caractère, races, particularités..."
        );
        // Entités décodées, balises <b> retirées.
        assert!(
            shaped[0].content.starts_with(
                "Les chats roux ne sont pas présents uniquement au sein d'une poignée"
            ),
            "{}",
            shaped[0].content
        );
        assert_eq!(shaped[1].url, "https://caats.co/chaton-roux/");
        assert!(shaped.iter().all(|result| !result.content.is_empty()));
    }

    /// Annonce (bloc `result--ad`, ou lien `y.js`), redirection `uddg` des
    /// anciennes pages.
    #[test]
    fn ads_are_dropped_and_redirects_decoded() {
        let page = r#"<div id="links" class="results">
          <div class="result results_links results_links_deep result--ad ">
            <h2 class="result__title"><a class="result__a" href="https://shop.example/promo">Pub</a></h2>
            <a class="result__snippet" href="x">Achetez</a></div>
          <div class="result results_links results_links_deep web-result ">
            <h2 class="result__title"><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org%2Fa%3Fb%3D1&amp;rut=abc">Ancien &amp; lien</a></h2>
            <div class="result__snippet">Extrait <b>gras</b></div></div>
          <div class="result results_links results_links_deep web-result ">
            <h2 class="result__title"><a class="result__a" href="https://duckduckgo.com/y.js?u3=z">Pub cachée</a></h2></div>
          <div class="result results_links results_links_deep web-result ">
            <h2 class="result__title"><a class="result__a" href="https://example.net/">Sans extrait</a></h2></div>
        </div>"#;
        let raw = parse_duckduckgo(page).unwrap();
        let urls: Vec<_> = raw.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(urls, ["https://example.org/a?b=1", "https://example.net/"]);
        assert_eq!(raw[0].title, "Ancien &amp; lien");
        assert_eq!(raw[0].snippets, ["Extrait <b>gras</b>"]);
        assert_eq!(raw[1].snippets, [""]);
    }

    /// Page « aucun résultat » (reconstituée d'après la page réelle) : vide,
    /// pas une erreur ; page inconnue : illisible.
    #[test]
    fn empty_and_unknown_pages() {
        let none = r#"<div id="links" class="results"><div class="result results_links results_links_deep web-result result--no-result"><div class="no-results">No results.</div></div></div>"#;
        assert_eq!(parse_duckduckgo(none), Ok(vec![]));
        assert_eq!(
            parse_duckduckgo("<html><body>Maintenance</body></html>"),
            Err(EngineError::Malformed)
        );
    }

    /// Page de blocage (reconstituée d'après `ddgs` et les rapports publics).
    #[test]
    fn the_anomaly_page_is_a_block() {
        let page = r#"<html><body><div class="anomaly-modal__title">Unfortunately, bots use DuckDuckGo too.</div>
            <form id="challenge-form" action="//duckduckgo.com/anomaly.js?sv=html&cc=sre"></form></body></html>"#;
        assert_eq!(parse_duckduckgo(page), Err(EngineError::Blocked));
    }

    #[test]
    fn regions() {
        assert_eq!(region("fr"), Some("fr-fr"));
        assert_eq!(region("en"), Some("us-en"));
        assert_eq!(region("zz"), None);
    }

    /// Fouillis de balises : aucune panique.
    #[test]
    fn broken_markup_never_panics() {
        for page in [
            "class=\"result ",
            "class=\"result \"><a class=\"result__a\"",
            "class=\"result x\"><a class=\"result__a\" href=\"",
            "class=\"result x\"><a class=\"result__a\" href=\"https://a.example/\">t",
            "class=\"result x\">class=\"result__a\"><",
            "<é class=\"result é\"><a class=\"result__a\" href=\"https://é.example/\">é</a>",
        ] {
            let _ = parse_duckduckgo(page);
        }
    }
}
