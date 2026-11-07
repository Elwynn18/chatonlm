//! Majuscule au début et point à la fin d'un passage corrigé (option « mise
//! en forme automatique » de la correction, propre à ChatonLM), faites ici
//! plutôt que laissées au modèle : activée, la première lettre du passage
//! est mise en capitale et un point termine une phrase qui n'a pas de
//! ponctuation finale ; coupée, une capitale ou une ponctuation finale que
//! le modèle a ajoutées (ses exemples le font) sont retirées.
//!
//! Jamais sur une structure Markdown (liste, titre, tableau, citation,
//! code), ni sur un mot qui ressemble à du code, une adresse ou une marque
//! (« iPhone », « config.toml », « https://… »), ni de point après moins
//! de trois mots (titre, signature) ou après une adresse.

use super::guard::{self, word_char};
use super::reflow::starts_block;

/// Avant le premier mot : espaces, guillemets, parenthèses, ponctuation
/// d'ouverture espagnole, emphase Markdown.
const OPENERS: &[char] = &[
    ' ', '\u{a0}', '\u{202f}', '\t', '«', '"', '“', '‘', '\'', '(', '[', '¿', '¡', '*', '_',
];
/// Après la ponctuation finale : guillemets, parenthèses, emphase.
const CLOSERS: &[char] = &[
    '»', '"', '”', '’', '\'', ')', ']', '*', '_', ' ', '\u{a0}', '\u{202f}',
];
/// Ponctuation qui finit une phrase.
const TERMINAL: &[char] = &['.', '!', '?', '…', ':', ';'];

/// Passage rendu selon l'option (`formatting`) ; `original` : le passage
/// envoyé.
pub fn finish(original: &str, answer: &str, formatting: bool) -> String {
    if formatting {
        with_period(&capitalized(answer))
    } else {
        without_period(original, &uncapitalized(original, answer))
    }
}

fn first_line(text: &str) -> &str {
    text.split(guard::is_break).next().unwrap_or_default()
}

fn last_line(text: &str) -> &str {
    text.rsplit(guard::is_break).next().unwrap_or_default()
}

/// Premier mot du texte (positions en octets), s'il n'est précédé que
/// d'ouvrants et que la première ligne n'est pas une structure.
fn first_word(text: &str) -> Option<(usize, usize)> {
    if starts_block(first_line(text)) {
        return None;
    }
    let start = text.find(|c: char| !OPENERS.contains(&c))?;
    let mut chars = text[start..].char_indices();
    let (_, first) = chars.next()?;
    if !word_char(first) {
        return None;
    }
    let end = text[start..]
        .find(|c: char| !word_char(c))
        .map_or(text.len(), |at| start + at);
    Some((start, end))
}

/// Mot ordinaire en tête de phrase : que des lettres, aucune capitale après
/// la première, pas suivi de « . », « _ », « / », « @ », « : » collés à
/// une lettre (code, fichier, adresse).
fn plain_word(text: &str, (start, end): (usize, usize)) -> bool {
    let word = &text[start..end];
    let mut chars = word.chars();
    let tail_lower = chars.all(|c| !c.is_uppercase());
    let letters = word.chars().all(char::is_alphabetic);
    let mut after = text[end..].chars();
    let code = matches!(after.next(), Some('.' | '_' | '/' | '@' | ':' | '\\'))
        && after.next().is_some_and(word_char);
    tail_lower && letters && !code
}

fn capitalized(answer: &str) -> String {
    let Some(word) = first_word(answer) else {
        return answer.to_owned();
    };
    let first = answer[word.0..].chars().next().unwrap_or(' ');
    let mut upper = first.to_uppercase();
    let (Some(capital), None) = (upper.next(), upper.next()) else {
        return answer.to_owned();
    };
    if !first.is_lowercase() || !plain_word(answer, word) {
        return answer.to_owned();
    }
    format!(
        "{}{capital}{}",
        &answer[..word.0],
        &answer[word.0 + first.len_utf8()..]
    )
}

fn uncapitalized(original: &str, answer: &str) -> String {
    let (Some(ours), Some(theirs)) = (first_word(original), first_word(answer)) else {
        return answer.to_owned();
    };
    let ours = original[ours.0..].chars().next().unwrap_or(' ');
    let theirs_at = theirs.0;
    let theirs = answer[theirs_at..].chars().next().unwrap_or(' ');
    let mut lower = theirs.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(small), None)
            if ours.is_lowercase()
                && theirs.is_uppercase()
                && guard::fold(small) == guard::fold(ours) =>
        {
            format!(
                "{}{small}{}",
                &answer[..theirs_at],
                &answer[theirs_at + theirs.len_utf8()..]
            )
        }
        _ => answer.to_owned(),
    }
}

/// Le texte se termine-t-il (fermants ôtés) par une lettre ou un chiffre ?
fn open_end(text: &str) -> bool {
    text.trim_end()
        .trim_end_matches(CLOSERS)
        .chars()
        .next_back()
        .is_some_and(char::is_alphanumeric)
}

fn with_period(answer: &str) -> String {
    let open = open_end(answer);
    let last = last_line(answer);
    let last_token = answer
        .trim_end()
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or_default();
    let address = last_token.contains("://")
        || last_token.contains(['@', '/', '\\'])
        || last_token.trim_end_matches(CLOSERS).contains('.');
    if !open || starts_block(last) || guard::words(answer).len() < 3 || address {
        return answer.to_owned();
    }
    // Après les fermants : « (voir annexe). »
    let trimmed = answer.trim_end().len();
    format!("{}.{}", &answer[..trimmed], &answer[trimmed..])
}

fn without_period(original: &str, answer: &str) -> String {
    let open = open_end(original);
    if !open || starts_block(last_line(answer)) {
        return answer.to_owned();
    }
    let end = answer.trim_end().trim_end_matches(CLOSERS).len();
    let core = &answer[..end];
    let stripped = core.trim_end_matches(TERMINAL);
    if stripped.len() == core.len() {
        return answer.to_owned();
    }
    let stripped = stripped.trim_end_matches([' ', '\u{a0}', '\u{202f}']);
    if !stripped
        .chars()
        .next_back()
        .is_some_and(char::is_alphanumeric)
    {
        return answer.to_owned();
    }
    format!("{stripped}{}", &answer[end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_capitalizes_the_start_and_ends_with_a_period() {
        let on = |answer: &str| finish("", answer, true);
        assert_eq!(
            on("hier, je suis allé au cinéma"),
            "Hier, je suis allé au cinéma."
        );
        assert_eq!(on("« bonjour à tous »"), "« Bonjour à tous ».");
        assert_eq!(on("est-ce que tu viens ?"), "Est-ce que tu viens ?");
        assert_eq!(on("Il a fini (enfin)"), "Il a fini (enfin).");
        assert_eq!(on("Il a dit « oui. »"), "Il a dit « oui. »");
        assert_eq!(on("¿qué hora es?"), "¿Qué hora es?");
        // Fins de ligne gardées après le point.
        assert_eq!(on("une phrase sans point\n"), "Une phrase sans point.\n");
    }

    #[test]
    fn formatting_leaves_code_addresses_titles_and_structures_alone() {
        let on = |answer: &str| finish("", answer, true);
        for (answer, expected) in [
            // Pas de capitale sur une marque, un fichier, un nombre ; le
            // point final, lui, vient.
            ("iPhone et iPad sont là", "iPhone et iPad sont là."),
            (
                "config.toml contient la clé",
                "config.toml contient la clé.",
            ),
            ("2024 était une bonne année", "2024 était une bonne année."),
            // Structures, titres, signatures, adresses : rien.
            ("- un élément de liste", "- un élément de liste"),
            ("# un titre en tête", "# un titre en tête"),
            ("Merci", "Merci"),
            ("Théo Bailly", "Théo Bailly"),
            (
                "Voir la page https://chatonlm.com",
                "Voir la page https://chatonlm.com",
            ),
            (
                "Écrire à contact@chatonlm.com",
                "Écrire à contact@chatonlm.com",
            ),
        ] {
            assert_eq!(on(answer), expected, "{answer}");
        }
    }

    #[test]
    fn without_formatting_the_added_capital_and_period_are_removed() {
        let off = |original: &str, answer: &str| finish(original, answer, false);
        assert_eq!(
            off(
                "hier je suis allez au cinema",
                "Hier, je suis allé au cinéma."
            ),
            "hier, je suis allé au cinéma"
        );
        assert_eq!(
            off(
                "est-ce que tu sais a quelle heure",
                "Est-ce que tu sais à quelle heure ?"
            ),
            "est-ce que tu sais à quelle heure"
        );
        // Déjà là dans le texte : gardés.
        assert_eq!(off("Bonjour a tous.", "Bonjour à tous."), "Bonjour à tous.");
        // Initiale corrigée (accent) : la minuscule revient quand même.
        assert_eq!(off("ecrit moi", "Écris-moi."), "écris-moi");
        assert_eq!(off("- un\n- deux", "- Un\n- Deux."), "- Un\n- Deux.");
    }
}
