//! Tokenizer de Pocket TTS : modèle Unigram (converti de SentencePiece) lu
//! dans `tokenizer.json`, au comportement de la bibliothèque `tokenizers`
//! qu'emploie pocket_tts (normaliseur « ▁ » en tête, pré-découpage Metaspace,
//! Viterbi, repli sur les octets ; décodage Metaspace + ByteFallback + Fuse).
//! Écrit ici plutôt que par la crate `tokenizers` (dépendances lourdes).
//!
//! Écart voulu : les pièces spéciales (`<unk>`, `<s>`…) et d'octets
//! (`<0x41>`) ne sont jamais reconnues dans le texte (comme SentencePiece) :
//! `tokenizers` changerait un « <s> » écrit dans une réponse en début de
//! séquence.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

/// Marque d'espace de SentencePiece.
const SPACE: char = '\u{2581}';

/// Pénalité d'un caractère inconnu (`K_UNK_PENALTY` de `tokenizers`).
const UNK_PENALTY: f64 = 10.0;

pub struct Tokenizer {
    /// Pièce de chaque identifiant (vide : absente, jamais reconnue).
    pieces: Vec<String>,
    /// Pièces reconnues dans le texte -> (identifiant, score).
    lookup: HashMap<String, (u32, f64)>,
    /// Longueur maximale (octets) d'une pièce reconnue.
    longest: usize,
    /// Identifiant de chaque octet (`<0xNN>`).
    bytes: [u32; 256],
    min_score: f64,
    /// Identifiants sautés au décodage (jetons spéciaux).
    special: Vec<bool>,
}

#[derive(Deserialize)]
struct TokenizerJson {
    model: ModelJson,
    #[serde(default)]
    added_tokens: Vec<AddedToken>,
}

#[derive(Deserialize)]
struct ModelJson {
    #[serde(rename = "type")]
    kind: String,
    vocab: Vec<(String, f64)>,
    #[serde(default)]
    byte_fallback: bool,
}

#[derive(Deserialize)]
struct AddedToken {
    id: u32,
    #[serde(default)]
    special: bool,
}

/// Pièce d'octet `<0xNN>`.
fn byte_of(piece: &str) -> Option<u8> {
    let hex = piece.strip_prefix("<0x")?.strip_suffix('>')?;
    (hex.len() == 2)
        .then(|| u8::from_str_radix(hex, 16).ok())
        .flatten()
}

impl Tokenizer {
    /// Lit `tokenizer.json`.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        Self::from_json(&text)
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let parsed: TokenizerJson =
            serde_json::from_str(text).map_err(|error| format!("tokenizer.json: {error}"))?;
        let model = parsed.model;
        if model.kind != "Unigram" || !model.byte_fallback {
            return Err(format!(
                "tokenizer.json: unsupported model {} (byte fallback {})",
                model.kind, model.byte_fallback
            ));
        }
        let min_score = model
            .vocab
            .iter()
            .map(|(_, score)| *score)
            .fold(f64::INFINITY, f64::min);
        let size = model.vocab.len();
        let pieces = model
            .vocab
            .into_iter()
            .enumerate()
            .map(|(id, (piece, score))| (id as u32, piece, score));
        let special: Vec<u32> = parsed
            .added_tokens
            .iter()
            .filter(|token| token.special)
            .map(|token| token.id)
            .collect();
        Self::from_pieces(size, min_score, pieces, &special)
    }

    /// Construit le tokenizer à partir de `(identifiant, pièce, score)` (tout
    /// le vocabulaire ou une partie : les essais n'en gardent que les pièces
    /// présentes dans leurs textes, `min_score` venant du vocabulaire
    /// complet).
    pub(crate) fn from_pieces(
        size: usize,
        min_score: f64,
        pieces: impl IntoIterator<Item = (u32, String, f64)>,
        special: &[u32],
    ) -> Result<Self, String> {
        let mut table = vec![String::new(); size];
        let mut lookup = HashMap::new();
        let mut bytes = [u32::MAX; 256];
        let mut flags = vec![false; size];
        for id in special {
            if let Some(flag) = flags.get_mut(*id as usize) {
                *flag = true;
            }
        }
        for (id, piece, score) in pieces {
            let index = id as usize;
            if index >= size {
                return Err(format!("tokenizer: id {id} out of range"));
            }
            if let Some(byte) = byte_of(&piece) {
                bytes[byte as usize] = id;
            } else if !flags[index] && !piece.is_empty() {
                lookup.insert(piece.clone(), (id, score));
            }
            table[index] = piece;
        }
        if bytes.contains(&u32::MAX) {
            return Err("tokenizer: missing byte pieces".into());
        }
        let longest = lookup.keys().map(String::len).max().unwrap_or(1);
        Ok(Self {
            pieces: table,
            lookup,
            longest,
            bytes,
            min_score,
            special: flags,
        })
    }

    /// Jetons de `text` (comme `Tokenizer.encode(text).ids`).
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut ids = Vec::new();
        if text.is_empty() {
            return ids;
        }
        // Normaliseur « Prepend ▁ », puis Metaspace : espaces -> ▁, découpe
        // devant chaque ▁ (la marque reste au début du mot).
        let normalized: String = std::iter::once(SPACE)
            .chain(text.chars().map(|c| if c == ' ' { SPACE } else { c }))
            .collect();
        let mut start = 0;
        for (index, c) in normalized.char_indices().skip(1) {
            if c == SPACE {
                self.encode_word(&normalized[start..index], &mut ids);
                start = index;
            }
        }
        self.encode_word(&normalized[start..], &mut ids);
        ids
    }

    /// Viterbi sur un mot (`encode_optimized` de `tokenizers`), puis repli
    /// sur les octets pour les caractères inconnus.
    fn encode_word(&self, word: &str, ids: &mut Vec<u32>) {
        #[derive(Clone, Copy)]
        struct Node {
            score: f64,
            start: Option<usize>,
            /// `None` : caractère inconnu.
            id: Option<u32>,
        }
        let size = word.len();
        let unk_score = self.min_score - UNK_PENALTY;
        let mut best = vec![
            Node {
                score: 0.0,
                start: None,
                id: None,
            };
            size + 1
        ];
        let boundaries: Vec<usize> = word
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(size))
            .collect();
        for (position, &start) in boundaries[..boundaries.len() - 1].iter().enumerate() {
            let here = best[start].score;
            let char_end = boundaries[position + 1];
            let mut single = false;
            // Préfixes du reste du mot, du plus court au plus long (ordre du
            // trie : départage identique).
            for &end in &boundaries[position + 1..] {
                if end - start > self.longest {
                    break;
                }
                if let Some(&(id, score)) = self.lookup.get(&word[start..end]) {
                    let candidate = here + score;
                    let target = &mut best[end];
                    if target.start.is_none() || candidate > target.score {
                        *target = Node {
                            score: candidate,
                            start: Some(start),
                            id: Some(id),
                        };
                    }
                    single |= end == char_end;
                }
            }
            if !single {
                let candidate = here + unk_score;
                let target = &mut best[char_end];
                if target.start.is_none() || candidate > target.score {
                    *target = Node {
                        score: candidate,
                        start: Some(start),
                        id: None,
                    };
                }
            }
        }
        // Chemin, de la fin vers le début.
        let mut path = Vec::new();
        let mut end = size;
        while end > 0 {
            let node = best[end];
            let start = node.start.expect("every character is reachable");
            path.push((start, end, node.id));
            end = start;
        }
        for (start, end, id) in path.into_iter().rev() {
            match id {
                Some(id) => ids.push(id),
                None => ids.extend(word[start..end].bytes().map(|b| self.bytes[b as usize])),
            }
        }
    }

    /// Texte de `ids` (comme `Tokenizer.decode(ids)` : jetons spéciaux
    /// sautés, ▁ -> espace sauf au premier jeton, octets regroupés en UTF-8,
    /// U+FFFD par octet d'une suite invalide).
    pub fn decode(&self, ids: &[u32]) -> String {
        let mut text = String::new();
        let mut pending: Vec<u8> = Vec::new();
        let flush = |pending: &mut Vec<u8>, text: &mut String| {
            if pending.is_empty() {
                return;
            }
            match std::str::from_utf8(pending) {
                Ok(valid) => text.push_str(valid),
                Err(_) => text.extend(std::iter::repeat_n('\u{FFFD}', pending.len())),
            }
            pending.clear();
        };
        let mut first = true;
        for &id in ids {
            let index = id as usize;
            if self.special.get(index).copied().unwrap_or(false) {
                continue;
            }
            let piece = self.pieces.get(index).map_or("", String::as_str);
            if let Some(byte) = byte_of(piece) {
                pending.push(byte);
            } else {
                flush(&mut pending, &mut text);
                for c in piece.chars() {
                    if c == SPACE {
                        if !first {
                            text.push(' ');
                        }
                    } else {
                        text.push(c);
                    }
                }
            }
            first = false;
        }
        flush(&mut pending, &mut text);
        text
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[derive(Deserialize)]
    pub(crate) struct MiniVocab {
        size: usize,
        min_score: f64,
        vocab: Vec<(u32, String, f64)>,
    }

    /// Tokenizer d'une fixture `prompt-<langue>.json` (sous-vocabulaire).
    pub(crate) fn from_fixture(fixture: &serde_json::Value) -> Tokenizer {
        let mini: MiniVocab = serde_json::from_value(fixture["tokenizer"].clone()).unwrap();
        Tokenizer::from_pieces(mini.size, mini.min_score, mini.vocab, &[0, 1, 2, 3]).unwrap()
    }

    pub(crate) fn fixture(language: &str) -> serde_json::Value {
        let text = match language {
            "fr" => include_str!("fixtures/prompt-fr.json"),
            "en" => include_str!("fixtures/prompt-en.json"),
            "es" => include_str!("fixtures/prompt-es.json"),
            _ => unreachable!(),
        };
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn encoding_matches_the_python_tokenizer() {
        for language in ["fr", "en", "es"] {
            let fixture = fixture(language);
            let tokenizer = from_fixture(&fixture);
            let cases = fixture["encode"].as_array().unwrap();
            assert!(cases.len() >= 10);
            for case in cases {
                let text = case[0].as_str().unwrap();
                let expected: Vec<u32> = serde_json::from_value(case[1].clone()).unwrap();
                assert_eq!(tokenizer.encode(text), expected, "{language}: {text:?}");
            }
        }
    }

    #[test]
    fn decoding_matches_the_python_tokenizer() {
        for language in ["fr", "en", "es"] {
            let fixture = fixture(language);
            let tokenizer = from_fixture(&fixture);
            for case in fixture["decode"].as_array().unwrap() {
                let ids: Vec<u32> = serde_json::from_value(case[0].clone()).unwrap();
                let expected = case[1].as_str().unwrap();
                assert_eq!(tokenizer.decode(&ids), expected, "{language}: {ids:?}");
            }
        }
    }

    /// Caractères absents du vocabulaire : leurs octets UTF-8, un jeton
    /// chacun ; un découpage au milieu d'un caractère donne U+FFFD.
    #[test]
    fn unknown_characters_fall_back_to_bytes() {
        let tokenizer = from_fixture(&fixture("fr"));
        let ids = tokenizer.encode("東");
        let bytes: Vec<u32> = "東".bytes().map(|b| tokenizer.bytes[b as usize]).collect();
        assert!(ids.ends_with(&bytes), "{ids:?}");
        assert!(!ids.contains(&0), "<unk>");
        assert_eq!(tokenizer.decode(&ids), "東");
        assert_eq!(tokenizer.decode(&bytes[..2]), "\u{FFFD}\u{FFFD}");
    }

    /// Une pièce spéciale écrite dans le texte n'est pas reconnue.
    #[test]
    fn special_pieces_are_never_matched_in_text() {
        let tokenizer = from_fixture(&fixture("en"));
        let ids = tokenizer.encode("<s>");
        assert!(!ids.contains(&1), "{ids:?}");
        assert_eq!(tokenizer.decode(&ids), "<s>");
    }

    #[test]
    fn the_json_file_is_read() {
        let json = serde_json::json!({
            "added_tokens": [{"id": 0, "special": true}],
            "model": {
                "type": "Unigram",
                "unk_id": 0,
                "byte_fallback": true,
                "vocab": std::iter::once(serde_json::json!(["<unk>", 0.0]))
                    .chain((0..=255).map(|b| serde_json::json!([format!("<0x{b:02X}>"), 0.0])))
                    .chain([
                        serde_json::json!(["\u{2581}a", -1.0]),
                        serde_json::json!(["\u{2581}ab", -1.5]),
                        serde_json::json!(["b", -1.0]),
                        serde_json::json!(["\u{2581}", -2.0]),
                    ])
                    .collect::<Vec<_>>(),
            }
        });
        let tokenizer = Tokenizer::from_json(&json.to_string()).unwrap();
        // « ▁ab » (−1,5) bat « ▁a » + « b » (−2).
        assert_eq!(tokenizer.encode("ab"), [258]);
        assert_eq!(tokenizer.encode("a b"), [257, 260, 259]);
        assert_eq!(tokenizer.decode(&[257, 260, 259]), "a b");
        // Jeton spécial sauté au décodage.
        assert_eq!(tokenizer.decode(&[0, 257]), "a");
        let refused = json.to_string().replace("Unigram", "BPE");
        assert!(Tokenizer::from_json(&refused).is_err());
    }
}
