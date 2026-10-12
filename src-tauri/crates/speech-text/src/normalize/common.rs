//! Socle commun du prétraitement : remplacements avec conditions sur le
//! contexte, chiffres romains, sigles épelés, nettoyage final.
//!
//! Le crate `regex` n'a pas de lookaround : chaque règle trouve un candidat
//! par une expression sans lookaround, puis vérifie ses voisins dans le code
//! (`sub`). Un candidat refusé laisse la recherche reprendre au caractère
//! suivant, comme le ferait `re.sub` de Python devant un lookbehind faux.

use regex::{Captures, Regex};
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

/// Expression compilée une seule fois (motif littéral ou calculé).
macro_rules! re {
    ($pattern:expr) => {{
        static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(AsRef::<str>::as_ref(&$pattern)).unwrap()
        });
        &*RE
    }};
}
pub(crate) use re;

/// Espaces d'un nombre ou d'une abréviation : espace, insécable, fine insécable.
macro_rules! sp {
    () => {
        r"[ \x{a0}\x{202f}]"
    };
}
pub(crate) use sp;

/// Caractère de mot au sens de `\w` (lettres, chiffres, soulignement).
pub(crate) fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Caractère qui précède l'octet `at`.
pub(crate) fn char_before(text: &str, at: usize) -> Option<char> {
    text[..at].chars().next_back()
}

/// Caractère qui suit l'octet `at`.
pub(crate) fn char_after(text: &str, at: usize) -> Option<char> {
    text[at..].chars().next()
}

/// Vrai si le caractère avant `at` n'est pas un caractère de mot (ni l'un de `extra`).
pub(crate) fn not_after(text: &str, at: usize, extra: &str) -> bool {
    char_before(text, at).is_none_or(|c| !is_word(c) && !extra.contains(c))
}

/// Vrai si le caractère après `at` n'est pas un caractère de mot (ni l'un de `extra`).
pub(crate) fn not_before(text: &str, at: usize, extra: &str) -> bool {
    char_after(text, at).is_none_or(|c| !is_word(c) && !extra.contains(c))
}

/// Remplace chaque correspondance de `re` acceptée par `f` (`Some`) ; un
/// refus (`None`) relance la recherche au caractère suivant le début du
/// candidat. `f` reçoit aussi le texte entier, pour lire le contexte.
pub(crate) fn sub(
    text: &str,
    re: &Regex,
    mut f: impl FnMut(&Captures, &str) -> Option<String>,
) -> String {
    sub_to(text, re, |c, t| {
        f(c, t).map(|r| (r, c.get(0).expect("groupe 0").end()))
    })
}

/// Comme `sub`, mais `f` rend aussi la fin (octet) du texte remplacé, au-delà
/// de la correspondance si besoin (l'unité qui suit un nombre).
pub(crate) fn sub_to(
    text: &str,
    re: &Regex,
    mut f: impl FnMut(&Captures, &str) -> Option<(String, usize)>,
) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut copied = 0;
    let mut pos = 0;
    let next_char = |at: usize| at + char_after(text, at).map_or(1, char::len_utf8);
    while pos <= text.len() {
        let Some(caps) = re.captures_at(text, pos) else {
            break;
        };
        let m = caps.get(0).expect("groupe 0");
        match f(&caps, text) {
            Some((replacement, end)) => {
                out.push_str(&text[copied..m.start()]);
                out.push_str(&replacement);
                copied = end;
                pos = if end == m.start() {
                    next_char(end)
                } else {
                    end
                };
            }
            None => pos = next_char(m.start()),
        }
    }
    out.push_str(&text[copied.min(text.len())..]);
    out
}

/// Remplacement littéral de toutes les correspondances.
pub(crate) fn replace(text: &str, re: &Regex, by: &str) -> String {
    re.replace_all(text, regex::NoExpand(by)).into_owned()
}

/// Valeur entière en toutes lettres d'une suite de chiffres (zéros de tête
/// ignorés, comme `int()`), par `cardinal_str` de la langue.
pub(crate) fn int_value(s: &str) -> String {
    let s = strip_spaces(s);
    let rest = s.trim_start_matches('0');
    if rest.is_empty() {
        "0".to_owned()
    } else {
        rest.to_owned()
    }
}

/// Forme canonique (NFC) et apostrophe droite.
pub(crate) fn prepare(text: &str) -> String {
    text.nfc()
        .map(|c| if c == '’' { '\'' } else { c })
        .collect()
}

/// Sigle épelé en capitales espacées (« A P I »), choix de l'utilisateur à
/// l'écoute (banc 56).
pub(crate) fn spell(letters: &str) -> String {
    let upper = letters.to_uppercase();
    let mut out = String::with_capacity(upper.len() * 2);
    for c in upper.chars() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Valeur d'un lexique : `{XYZ}` est épelé.
pub(crate) fn expand_lexicon(value: &str) -> String {
    sub(value, re!(r"\{([A-Z]+)\}"), |c, _| Some(spell(&c[1])))
}

/// Valeur d'un chiffre romain bien formé (I à MMMCMXCIX), sinon `None`.
pub(crate) fn roman(s: &str) -> Option<u32> {
    let valid = re!(r"^M{0,3}(CM|CD|D?C{0,3})(XC|XL|L?X{0,3})(IX|IV|V?I{0,3})$");
    if s.is_empty() || !valid.is_match(s) {
        return None;
    }
    let value = |c: char| match c {
        'I' => 1,
        'V' => 5,
        'X' => 10,
        'L' => 50,
        'C' => 100,
        'D' => 500,
        _ => 1000,
    };
    let chars: Vec<char> = s.chars().collect();
    let mut total: i64 = 0;
    for (i, &c) in chars.iter().enumerate() {
        let v = value(c);
        match chars.get(i + 1) {
            Some(&next) if value(next) > v => total -= v,
            _ => total += v,
        }
    }
    u32::try_from(total).ok().filter(|&n| n > 0)
}

/// Chiffre romain d'une seule lettre accepté hors des siècles : I, V, X
/// seulement (« annexe C », « partie L » ne sont pas des nombres).
pub(crate) fn roman_in_context(s: &str) -> Option<u32> {
    if s.chars().count() == 1 && !matches!(s, "I" | "V" | "X") {
        return None;
    }
    roman(s)
}

/// Symbole « autre » (catégorie Unicode So) : émojis, pictogrammes, flèches
/// décoratives, cases, ©, ®, ° ; plus les sélecteurs de variante, la liaison
/// sans chasse et les modificateurs de teinte qui les accompagnent.
pub(crate) fn is_other_symbol(c: char) -> bool {
    matches!(c as u32,
        0xA6 | 0xA9 | 0xAE | 0xB0
        | 0x200D | 0x20E3 | 0xFE0E | 0xFE0F
        | 0x2100..=0x2101 | 0x2103..=0x2106 | 0x2108..=0x2109 | 0x2114 | 0x2116..=0x2117
        | 0x211E..=0x2123 | 0x2125 | 0x2127 | 0x2129 | 0x212E | 0x213A..=0x213B
        | 0x2195..=0x2199 | 0x219C..=0x219F | 0x21A1..=0x21A2 | 0x21A4..=0x21A5
        | 0x21A7..=0x21AD | 0x21AF..=0x21CD | 0x21D0..=0x21D1 | 0x21D3 | 0x21D5..=0x21F3
        | 0x2300..=0x2307 | 0x230C..=0x231F | 0x2322..=0x2328 | 0x232B..=0x237B
        | 0x237D..=0x239A | 0x23B4..=0x23DB | 0x23E2..=0x23FF
        | 0x2400..=0x244A | 0x249C..=0x24E9
        | 0x2500..=0x25B6 | 0x25B8..=0x25C0 | 0x25C2..=0x25F7
        | 0x2600..=0x266E | 0x2670..=0x2767 | 0x2794..=0x27BF
        | 0x2800..=0x28FF | 0x2B00..=0x2B2F | 0x2B45..=0x2B46 | 0x2B4D..=0x2BFF
        | 0x1F000..=0x1F0FF | 0x1F10D..=0x1F1FF | 0x1F200..=0x1F2FF
        | 0x1F300..=0x1F3FA | 0x1F3FB..=0x1F3FF | 0x1F400..=0x1FAFF
        | 0xE0020..=0xE007F
    )
}

/// Fin commune : symboles « autres » retirés, suites d'espaces resserrées,
/// espace laissée avant un point ou une virgule par un symbole retiré
/// (« prêt 🚀. » ; « .NET » garde la sienne).
pub(crate) fn finish(text: &str) -> String {
    let kept: String = text.chars().filter(|&c| !is_other_symbol(c)).collect();
    let kept = sub(&kept, re!(r"\s+[.,]"), |c, t| {
        let m = c.get(0).unwrap();
        char_after(t, m.end())
            .is_none_or(char::is_whitespace)
            .then(|| m.as_str().trim_start().to_owned())
    });
    re!(r"\s{2,}").replace_all(&kept, " ").trim().to_owned()
}

/// Langages et cadriciels écrits avec un symbole, lus partout pareil :
/// « C# », « F# » → « C sharp », « C++ » → « C plus plus », « .NET » →
/// « dotnet ».
pub(crate) fn symbol_names(t: &str) -> String {
    let t = sub(t, re!(r"\b([CF])#"), |c, t| {
        not_before(t, c.get(0).unwrap().end(), "#").then(|| format!("{} sharp", &c[1]))
    });
    let t = sub(&t, re!(r"\bC\+\+"), |c, t| {
        not_before(t, c.get(0).unwrap().end(), "+").then(|| "C plus plus".to_owned())
    });
    sub(&t, re!(r"\.NET\b"), |c, t| {
        not_after(t, c.get(0).unwrap().start(), ".").then(|| "dotnet".to_owned())
    })
}

/// Vrai si le mot entre `start` et `end` touche un chemin ou un nom de
/// fichier (« /etc/hosts », « etc.conf », « mon_etc »).
pub(crate) fn in_path(t: &str, start: usize, end: usize) -> bool {
    let path = |c: char| matches!(c, '/' | '\\' | '_' | '.');
    char_before(t, start).is_some_and(path)
        || char_after(t, end).is_some_and(|c| matches!(c, '/' | '\\' | '_'))
}

/// Notation scientifique « 1e-5 », « 2.5E+3 » : `words(mantisse, signe,
/// exposant)` la lit ; mantisse « 1 » seule : `None`.
pub(crate) fn scientific(
    t: &str,
    mut words: impl FnMut(Option<&Captures>, bool, &str) -> String,
) -> String {
    sub(
        t,
        re!(
            r"\b(?P<int>[0-9]+)(?:(?P<sep>[.,])(?P<frac>[0-9]+))?[eE](?P<sign>[-+−]?)(?P<exp>[0-9]+)\b"
        ),
        |c, _| {
            let one = &c["int"] == "1" && c.name("frac").is_none();
            Some(words(
                (!one).then_some(c),
                matches!(&c["sign"], "-" | "−"),
                &c["exp"],
            ))
        },
    )
}

/// Mot en capitales lu comme un mot : sa forme en minuscules est un mot
/// outil de la langue (listes de la détection de langue) ou un mot courant
/// de `extra`. Les vrais sigles (« API », « GPU ») restent épelés.
pub(crate) fn is_common_word(word: &str, language: crate::Language, extra: &[&str]) -> bool {
    let lower = word.to_lowercase();
    lower.chars().count() > 1
        && (crate::language::common_words(language).contains(&lower.as_str())
            || extra.contains(&lower.as_str()))
}

/// Sigles en capitales (`letters` : au moins deux lettres) : lus comme des
/// mots (minuscules) s'ils sont courants (`is_common_word`, sauf `spelled`)
/// ou s'ils ont 5 lettres ou plus dont une voyelle ; sinon épelés. Un trait
/// d'union qui les colle à un nombre ou à un autre sigle devient une espace
/// (« GPT-4 » → « G P T quatre », « USB-C » → « U S B C ») ; dans un mot
/// composé en minuscules (« e-MAIL »), rien ne change.
pub(crate) fn acronyms(
    t: &str,
    letters: &Regex,
    vowel: fn(char) -> bool,
    language: crate::Language,
    extra: &[&str],
    spelled: &[&str],
) -> String {
    sub_to(t, letters, |c, t| {
        let m = c.get(0).unwrap();
        let in_compound = t[..m.start()]
            .strip_suffix('-')
            .is_some_and(|before| before.ends_with(char::is_lowercase));
        if !not_after(t, m.start(), "-") && (in_compound || !t[..m.start()].ends_with('-')) {
            return None;
        }
        if !not_before(t, m.end(), "-'") && !t[m.end()..].starts_with(['-', '\'']) {
            return None;
        }
        let w = m.as_str();
        let as_word = (w.chars().count() >= 5 && w.chars().any(vowel))
            || (is_common_word(w, language, extra) && !spelled.contains(&w));
        let mut words = if as_word { w.to_lowercase() } else { spell(w) };
        let mut end = m.end();
        if let Some(after) = t[end..].strip_prefix('-')
            && after.starts_with(|c: char| c.is_alphanumeric())
        {
            words.push(' ');
            end += 1;
        }
        Some((words, end))
    })
}

/// Mot (en minuscules) qui précède l'octet `at`, séparé par des espaces.
pub(crate) fn previous_word(t: &str, at: usize) -> Option<String> {
    let before = t[..at].trim_end();
    if before.len() == at {
        return None;
    }
    let word: String = before
        .chars()
        .rev()
        .take_while(|c| c.is_alphabetic())
        .collect();
    (!word.is_empty()).then(|| word.chars().rev().collect::<String>().to_lowercase())
}

/// Vrai si « a - b » est une soustraction : suivi de « = » (ou d'un mot de
/// `operators` : « égal »), ou précédé d'un opérateur.
pub(crate) fn is_subtraction(t: &str, start: usize, end: usize, operators: &[&str]) -> bool {
    let after = t[end..].trim_start();
    after.starts_with('=')
        || operators.iter().any(|w| after.starts_with(w))
        || t[..start]
            .trim_end()
            .ends_with(['+', '-', '−', '*', '×', '/', '='])
        || previous_word(t, start).is_some_and(|w| operators.contains(&w.as_str()))
}

/// Fin d'une unité : ni lettre, ni « / » suivi d'un chiffre, ni apostrophe
/// (« les 2 l'utilisent »).
pub(crate) fn unit_ends(rest: &str) -> bool {
    match rest.chars().next() {
        None => true,
        Some('\'') => false,
        Some('/') => rest[1..].starts_with(char::is_alphabetic),
        Some(c) => !is_word(c),
    }
}

/// « /mois » après une unité ou un montant : (« par mois », longueur lue).
pub(crate) fn per_period(rest: &str, word: &str) -> (String, usize) {
    let Some(after) = rest.strip_prefix('/') else {
        return (String::new(), 0);
    };
    let period: String = after.chars().take_while(|c| c.is_alphabetic()).collect();
    if period.is_empty() {
        return (String::new(), 0);
    }
    (format!(" {word} {period}"), 1 + period.len())
}

/// Rapports « 16:9 », « écran 16:10 » : après un mot de `context`, ou quand
/// le second nombre n'a qu'un chiffre (pas une heure) ; `read(a, b)` les lit.
pub(crate) fn ratios(t: &str, context: &[&str], read: impl Fn(u64, u64) -> String) -> String {
    sub(
        t,
        re!(r"\b(?P<a>[0-9]{1,2}):(?P<b>[0-9]{1,2})\b"),
        |c, t| {
            let m = c.get(0).unwrap();
            if t[m.end()..].starts_with(':') || t[..m.start()].ends_with(':') {
                return None;
            }
            let in_context =
                previous_word(t, m.start()).is_some_and(|w| context.contains(&w.as_str()));
            (in_context || c["b"].len() == 1).then(|| {
                read(
                    parse_int(&c["a"]).unwrap_or(0),
                    parse_int(&c["b"]).unwrap_or(0),
                )
            })
        },
    )
}

/// Mot suivant (après des espaces de nombre) : `\s*` puis `\w+`.
pub(crate) fn next_word(text: &str, at: usize) -> Option<&str> {
    static NEXT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(concat!("^", sp!(), r"+(\w+)")).unwrap());
    NEXT.captures(&text[at..])
        .map(|c| c.get(1).unwrap().as_str())
}

/// Chiffres sans les espaces de groupement.
pub(crate) fn strip_spaces(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, ' ' | '\u{a0}' | '\u{202f}'))
        .collect()
}

/// Entier d'une suite de chiffres, `None` au-delà de u64.
pub(crate) fn parse_int(s: &str) -> Option<u64> {
    strip_spaces(s).parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roman_numerals() {
        assert_eq!(roman("XIV"), Some(14));
        assert_eq!(roman("XXI"), Some(21));
        assert_eq!(roman("MCMXC"), Some(1990));
        assert_eq!(roman("IIII"), None);
        assert_eq!(roman("IC"), None);
        assert_eq!(roman(""), None);
        assert_eq!(roman_in_context("C"), None);
        assert_eq!(roman_in_context("V"), Some(5));
        assert_eq!(roman_in_context("CD"), Some(400));
    }

    #[test]
    fn sub_retries_after_a_refused_candidate() {
        // Candidat « 12 » refusé (suivi d'une lettre), puis « 2 » refusé
        // (précédé d'un chiffre) : rien ne change, comme en Python.
        let out = sub("12a 3", re!(r"[0-9]+"), |c, t| {
            let m = c.get(0).unwrap();
            (not_after(t, m.start(), "") && not_before(t, m.end(), "")).then(|| "N".to_owned())
        });
        assert_eq!(out, "12a N");
    }

    #[test]
    fn symbols_and_spaces() {
        assert_eq!(finish("  Prêt 🚀 !  \u{2764}\u{FE0F}  "), "Prêt !");
        assert_eq!(finish("a\nb"), "a\nb");
        assert_eq!(
            finish("usa verbose 🚀. Le framework .NET , oui"),
            "usa verbose. Le framework .NET, oui"
        );
        assert_eq!(spell("api"), "A P I");
        assert_eq!(expand_lexicon("mac {OS}"), "mac O S");
    }
}
