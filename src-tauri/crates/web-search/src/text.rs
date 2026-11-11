//! Résultats normalisés : adresses http(s) seulement, sans doublon, texte
//! sans balises ni caractères cachés, longueurs bornées.
//!
//! Déplacé du relais (`relay-server/src/providers.rs`, 8 nov. 2026), qui s'en
//! sert encore depuis ce crate.

/// Résultat brut d'un moteur (extraits séparés).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawResult {
    pub title: String,
    pub url: String,
    pub snippets: Vec<String>,
}

/// Un résultat, au format du `web_search` d'Ollama.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    /// Extrait pour le modèle (l'app ne le garde pas dans l'historique).
    pub content: String,
}

pub const MAX_TITLE_CHARS: usize = 300;
pub const MAX_CONTENT_CHARS: usize = 1_500;
pub const MAX_URL_LEN: usize = 2_048;

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

/// Entités courantes et numériques (`&amp;`, `&#x27;`, `&#233;`…) ; une
/// entité inconnue reste telle quelle.
pub fn decode_entities(text: &str) -> String {
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

    /// Un `&` suivi de caractères non ASCII ne fait pas paniquer.
    #[test]
    fn ampersands_before_non_ascii_text() {
        assert_eq!(clean("Tom & Jérôme à Paris"), "Tom & Jérôme à Paris");
        assert_eq!(clean("Q&Aの使い方"), "Q&Aの使い方");
        assert_eq!(clean("&éèàùç;&é"), "&éèàùç;&é");
        assert_eq!(clean("R&D – 研究開発 &amp; 方"), "R&D – 研究開発 & 方");
    }

    /// 10 000 chaînes Unicode quelconques autour de `&`, `;`, `<` : aucune
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

    /// Un `<` seul n'avale pas le reste ; une balise encodée ne revient pas ;
    /// les forçages de sens d'écriture disparaissent.
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
}
