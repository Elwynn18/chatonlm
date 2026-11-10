//! Texte d'une page lue par `web_fetch` : titre, texte visible (sans
//! scripts, styles ni balises, entités décodées, blancs resserrés) et liens
//! http(s) absolus. Lecture simple, sans arbre du document : une page mal
//! formée donne au pire un texte imparfait, jamais une erreur.
//!
//! Travail linéaire dans la taille de la page (une page hostile de 4 Mio
//! d'éléments sautés ne coûte pas plus qu'une autre), et interruptible :
//! [`html_until`] s'arrête dès que `stop` est levé (délai ou arrêt de
//! l'envoi, `fetch.rs` le lance hors du fil async).

use std::sync::atomic::{AtomicBool, Ordering};

use url::Url;

use super::fetch::Page;

/// Texte gardé d'une page, au plus (caractères).
pub const MAX_TEXT: usize = 40_000;
/// Liens gardés d'une page, au plus.
pub const MAX_LINKS: usize = 100;
/// Titre gardé, au plus (caractères).
const MAX_TITLE: usize = 300;

/// Éléments dont le contenu n'est pas du texte à lire.
const SKIPPED: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "math", "iframe", "object", "canvas",
];

/// Éléments qui commencent une nouvelle ligne.
const BLOCKS: &[&str] = &[
    "p",
    "div",
    "br",
    "li",
    "ul",
    "ol",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "tr",
    "table",
    "section",
    "article",
    "header",
    "footer",
    "nav",
    "aside",
    "main",
    "blockquote",
    "pre",
    "hr",
    "dt",
    "dd",
    "figcaption",
    "form",
];

/// Page en texte simple.
pub fn plain(source: &str) -> Page {
    Page {
        title: String::new(),
        text: tidy(source),
        links: Vec::new(),
    }
}

/// Page HTML lue à l'adresse `base` (liens relatifs résolus contre elle).
#[cfg(test)]
pub fn html(source: &str, base: &Url) -> Page {
    html_until(source, base, &AtomicBool::new(false))
}

/// Comme [`html`], interrompue dès que `stop` est levé (texte partiel, que
/// l'appelant jette).
pub fn html_until(source: &str, base: &Url, stop: &AtomicBool) -> Page {
    let mut text = String::new();
    let mut title = String::new();
    let mut links: Vec<String> = Vec::new();
    let mut in_title = false;
    // Mise en minuscules une seule fois : `to_ascii_lowercase` garde les
    // positions des octets, `rest` se retrouve dans `lower` par sa fin.
    let lower = source.to_ascii_lowercase();
    let mut rest = source;
    while let Some(start) = rest.find('<') {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let chunk = &rest[..start];
        if in_title {
            title.push_str(chunk);
        } else {
            text.push_str(chunk);
        }
        rest = &rest[start..];
        // Commentaire, déclaration, instruction.
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            rest = rest.find('>').map_or("", |end| &rest[end + 1..]);
            continue;
        }
        let Some(end) = tag_end(rest) else {
            // `<` sans balise : du texte.
            text.push('<');
            rest = &rest[1..];
            continue;
        };
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if name.is_empty() {
            text.push('<');
            text.push_str(tag);
            text.push('>');
            continue;
        }
        if name == "title" {
            in_title = !closing;
            continue;
        }
        if !closing && SKIPPED.contains(&name.as_str()) {
            let lower_rest = &lower[source.len() - rest.len()..];
            rest = skip_element(rest, lower_rest, &name);
            continue;
        }
        if BLOCKS.contains(&name.as_str()) {
            text.push('\n');
        } else if !closing
            && name == "a"
            && links.len() < MAX_LINKS
            && let Some(mut link) = attribute(tag, "href")
                .and_then(|href| base.join(&decode(href.trim())).ok())
                .filter(|url| matches!(url.scheme(), "http" | "https"))
        {
            link.set_fragment(None);
            let link = link.to_string();
            if !links.contains(&link) {
                links.push(link);
            }
        } else if matches!(name.as_str(), "td" | "th") {
            text.push(' ');
        }
    }
    text.push_str(rest);
    let title: String = tidy(&title)
        .replace('\n', " ")
        .chars()
        .take(MAX_TITLE)
        .collect();
    Page {
        title,
        text: tidy(&text),
        links,
    }
}

/// Fin (`>`) de la balise qui commence `text` (`<` suivi d'une lettre ou de
/// `/`), hors des valeurs entre guillemets ; `None` : pas une balise.
fn tag_end(text: &str) -> Option<usize> {
    let next = text[1..].chars().next()?;
    if !(next.is_ascii_alphabetic() || next == '/') {
        return None;
    }
    let mut quote: Option<char> = None;
    for (index, c) in text.char_indices().skip(1) {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return Some(index),
            (None, '<') => return None,
            _ => {}
        }
    }
    None
}

/// Saute le contenu d'un élément jusqu'à sa balise fermante (insensible à la
/// casse : cherchée dans `lower`, le même texte en minuscules, sans le
/// recopier) ; sans elle, jusqu'à la fin.
fn skip_element<'a>(text: &'a str, lower: &str, name: &str) -> &'a str {
    let closing = format!("</{name}");
    match lower.find(&closing) {
        Some(at) => {
            let after = &text[at..];
            after.find('>').map_or("", |end| &after[end + 1..])
        }
        None => "",
    }
}

/// Valeur d'un attribut d'une balise (`href="…"`, `href='…'`, `href=…`).
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let at = from + found;
        from = at + name.len();
        // Nom entier : précédé d'un blanc.
        if !lower[..at].ends_with(|c: char| c.is_ascii_whitespace()) {
            continue;
        }
        let after = tag[from..].trim_start();
        let Some(value) = after.strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        return Some(match value.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                let inner = &value[1..];
                inner.find(quote).map_or(inner, |end| &inner[..end])
            }
            _ => value
                .split(|c: char| c.is_ascii_whitespace() || c == '>')
                .next()
                .unwrap_or_default(),
        });
    }
    None
}

/// Entités HTML courantes et numériques décodées ; une entité inconnue reste
/// telle quelle.
pub fn decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        // `;` cherché dans les octets (frontière sûre : ASCII).
        let end = rest.bytes().take(12).position(|b| b == b';');
        let decoded = end.and_then(|end| entity(&rest[1..end]).map(|c| (c, end)));
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

fn entity(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        return char::from_u32(code).filter(|c| !c.is_control() || c.is_whitespace());
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "copy" => '©',
        "reg" => '®',
        "euro" => '€',
        "eacute" => 'é',
        "egrave" => 'è',
        "ecirc" => 'ê',
        "agrave" => 'à',
        "acirc" => 'â',
        "ccedil" => 'ç',
        "ocirc" => 'ô',
        "ugrave" => 'ù',
        "ucirc" => 'û',
        "icirc" => 'î',
        "iuml" => 'ï',
        "ntilde" => 'ñ',
        "aacute" => 'á',
        "iacute" => 'í',
        "oacute" => 'ó',
        "uacute" => 'ú',
        "uuml" => 'ü',
        "ouml" => 'ö',
        "auml" => 'ä',
        "szlig" => 'ß',
        _ => return None,
    })
}

/// Entités décodées, contrôles retirés, blancs d'une ligne resserrés, lignes
/// vides réduites à une, texte borné à [`MAX_TEXT`].
fn tidy(text: &str) -> String {
    let decoded = decode(text);
    let mut lines: Vec<String> = Vec::new();
    for line in decoded.lines() {
        let line: String = line
            .split(|c: char| c.is_whitespace())
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        if line.is_empty() && lines.last().is_none_or(String::is_empty) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n").chars().take(MAX_TEXT).collect()
}
