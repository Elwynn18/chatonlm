//! Jetons du faux : un mot avec l'espace qui le précède (« Hello world » →
//! `Hello`, ` world`, comme le découpage du vrai sur ce texte), identifiant
//! tiré du texte du morceau. Le même découpage compte le prompt (contexte
//! dépassé, cache) et les morceaux d'une réponse (un morceau = un jeton).

use std::hash::{DefaultHasher, Hash, Hasher};

/// Coût d'un message dans le prompt, en plus de son texte (balises du
/// gabarit) ; coût fixe de la fin du prompt (début de la réponse) ; coût
/// d'une image jointe.
pub const PER_MESSAGE: usize = 4;
pub const GENERATION_PROMPT: usize = 3;
pub const PER_IMAGE: usize = 64;

/// Morceaux du texte : chaque morceau commence au début d'un mot et garde
/// les blancs qui le précèdent.
pub fn pieces(text: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut previous_blank = false;
    for character in text.chars() {
        let blank = character.is_whitespace();
        if blank && !previous_blank && !current.is_empty() {
            pieces.push(std::mem::take(&mut current));
        }
        current.push(character);
        previous_blank = blank;
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

/// Identifiant d'un morceau (stable d'une exécution à l'autre).
pub fn id(piece: &str) -> u32 {
    let mut hasher = DefaultHasher::new();
    piece.hash(&mut hasher);
    (hasher.finish() % 150_000) as u32
}

/// Identifiants du texte.
pub fn ids(text: &str) -> Vec<u32> {
    pieces(text).iter().map(|piece| id(piece)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_keeps_the_blank_before_it() {
        assert_eq!(pieces("Hello world"), ["Hello", " world"]);
        assert_eq!(pieces("  a\n\nb "), ["  a", "\n\nb", " "]);
        assert!(pieces("").is_empty());
        assert_eq!(ids("Hello world"), ids("Hello world"));
        assert_ne!(id("Hello"), id(" Hello"));
    }
}
