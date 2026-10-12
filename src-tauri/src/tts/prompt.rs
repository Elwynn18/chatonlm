// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Kyutai
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Translated to Rust from pocket-tts 3.3.0
// (https://github.com/kyutai-labs/pocket-tts), file
// pocket_tts/models/text_chunking.py, and the per-language settings of
// pocket_tts/config/{french,spanish,english}.yaml.
// Modified for ChatonLM: an empty text gives no chunk instead of an error;
// special tokens are never matched in the text (see tokenizer.rs).

//! Préparation du texte pour Pocket TTS, portée de pocket-tts 3.3.0
//! (`models/text_chunking.py` : `prepare_text_prompt`,
//! `split_into_best_sentences` ; MIT, Kyutai) : remplacements de caractères
//! de la langue, `;` -> `,`, majuscule initiale, ponctuation finale, puis
//! découpage en morceaux d'au plus 50 jetons sur les fins de phrase (et sur
//! `,;:` pour une phrase trop longue). Chaque morceau est généré à part.
//!
//! Le texte reçu est déjà prétraité (crate `speech-text`) : rien d'autre ici.

use super::VoiceLanguage;
use super::tokenizer::Tokenizer;

/// Jetons au plus par morceau (`MAX_TOKEN_PER_CHUNK`).
pub const MAX_TOKENS: usize = 50;
/// Estimation de la longueur maximale d'un morceau (`_estimate_max_gen_len`).
const TOKENS_PER_SECOND: f64 = 3.0;
const SECONDS_PADDING: f64 = 2.0;
/// Trames par seconde (12,5 Hz).
pub const FRAME_RATE: f64 = 12.5;

const TERMINAL: &str = ".!?\u{2026}";
const WEAK: &str = ",;:-\u{2013}\u{2014}";
const CLOSERS: &str = "\"'\u{201d}\u{2019})]\u{bb}";

/// Options de la configuration de la langue (`config/<langue>.yaml` de
/// pocket-tts 3.3.0) ; les autres valent leur défaut pour les trois langues
/// (pas d'espaces de remplissage, ponctuation finale, majuscule).
pub struct PromptConfig {
    pub remove_semicolons: bool,
    pub replace_characters: &'static [(char, &'static str)],
}

const QUOTES_AND_BRACKETS: [(char, &str); 12] = [
    ('"', ""),
    ('\u{201c}', ""),
    ('\u{201d}', ""),
    ('\u{201e}', ""),
    ('\u{ab}', ""),
    ('\u{bb}', ""),
    ('\u{2019}', "'"),
    ('\u{2018}', "'"),
    ('(', ""),
    (')', ""),
    ('[', ""),
    (']', ""),
];

const FRENCH: [(char, &str); 13] = {
    let mut table = [(':', ","); 13];
    let mut index = 0;
    while index < QUOTES_AND_BRACKETS.len() {
        table[index] = QUOTES_AND_BRACKETS[index];
        index += 1;
    }
    table
};

const SPANISH: [(char, &str); 14] = {
    let mut table = [('\u{a1}', ""); 14];
    let mut index = 0;
    while index < QUOTES_AND_BRACKETS.len() {
        table[index] = QUOTES_AND_BRACKETS[index];
        index += 1;
    }
    table[13] = ('\u{bf}', "");
    table
};

impl PromptConfig {
    pub fn for_language(language: VoiceLanguage) -> Self {
        match language {
            VoiceLanguage::Fr => Self {
                remove_semicolons: true,
                replace_characters: &FRENCH,
            },
            VoiceLanguage::Es => Self {
                remove_semicolons: false,
                replace_characters: &SPANISH,
            },
            VoiceLanguage::En => Self {
                remove_semicolons: false,
                replace_characters: &[],
            },
        }
    }
}

/// Morceau prêt pour la génération.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub text: String,
    pub tokens: Vec<u32>,
    /// Trames générées après la fin détectée.
    pub frames_after_eos: usize,
    /// Plafond de trames.
    pub max_frames: usize,
}

/// Espace au sens de Python (`str.isspace`, `str.split()`).
fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(is_space).filter(|word| !word.is_empty())
}

/// `prepare_text_prompt` : texte préparé et estimation des trames après la
/// fin (avant le +2 de l'appelant) ; `None` pour un texte vide.
pub fn prepare(text: &str, config: &PromptConfig) -> Option<(String, usize)> {
    let mut text = text.trim_matches(is_space).to_owned();
    if !config.replace_characters.is_empty() {
        let mut translated = String::with_capacity(text.len());
        for c in text.chars() {
            match config
                .replace_characters
                .iter()
                .find(|(from, _)| *from == c)
            {
                Some((_, to)) => translated.push_str(to),
                None => translated.push(c),
            }
        }
        text = words(&translated).collect::<Vec<_>>().join(" ");
        text = drop_marks_after_sentence_end(&text);
    }
    if text.is_empty() {
        return None;
    }
    text = text.replace(['\n', '\r'], " ").replace("  ", " ");
    if config.remove_semicolons {
        text = text.replace(';', ",");
    }
    let guess = if words(&text).count() <= 4 { 3 } else { 1 };
    let mut chars = text.chars();
    let first = chars.next().expect("not empty");
    text = first.to_uppercase().chain(chars).collect();
    Some((ensure_terminal_punctuation(&text), guess))
}

/// `re.sub(r"([.!?…])\s*[,;:]", r"\1", text)`.
fn drop_marks_after_sentence_end(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        out.push(c);
        index += 1;
        if TERMINAL.contains(c) {
            let mut next = index;
            while next < chars.len() && is_space(chars[next]) {
                next += 1;
            }
            if next < chars.len() && ",;:".contains(chars[next]) {
                index = next + 1;
            }
        }
    }
    out
}

/// `_ensure_terminal_punctuation`.
fn ensure_terminal_punctuation(text: &str) -> String {
    let core = text.trim_end_matches(|c: char| c == ' ' || CLOSERS.contains(c));
    let closers = text[core.len()..].trim_matches(is_space);
    match core.chars().last() {
        None => text.to_owned(),
        Some(last) if TERMINAL.contains(last) => text.to_owned(),
        Some(last) if WEAK.contains(last) => {
            let stripped = core.trim_end_matches(|c: char| c == ' ' || WEAK.contains(c));
            format!("{stripped}.{closers}")
        }
        Some(_) => format!("{text}."),
    }
}

/// `_find_boundary_indices` : début de chaque segment (après une suite de
/// jetons de fin), puis la longueur.
fn boundaries(tokens: &[u32], marks: &[u32], decimal: Option<&Tokenizer>) -> Vec<usize> {
    let mut indices = vec![0];
    let mut after_mark = false;
    for (index, token) in tokens.iter().enumerate() {
        if marks.contains(token) {
            after_mark = true;
            continue;
        }
        if after_mark {
            after_mark = false;
            if let Some(tokenizer) = decimal
                && after_decimal_point(tokens, index, tokenizer)
            {
                continue;
            }
            indices.push(index);
        }
    }
    indices.push(tokens.len());
    indices
}

/// `_is_decimal_period_boundary` : « 3.5 » ne coupe pas la phrase.
fn after_decimal_point(tokens: &[u32], index: usize, tokenizer: &Tokenizer) -> bool {
    let prefix: Vec<char> = tokenizer.decode(&tokens[..index]).chars().collect();
    let suffix = tokenizer.decode(&tokens[index..]);
    prefix.len() >= 2
        && prefix[prefix.len() - 1] == '.'
        && prefix[prefix.len() - 2].is_numeric()
        && suffix.chars().next().is_some_and(char::is_numeric)
}

/// `_segments_from_boundaries` : (nombre de jetons, texte décodé).
fn segments(tokens: &[u32], indices: &[usize], tokenizer: &Tokenizer) -> Vec<(usize, String)> {
    indices
        .windows(2)
        .map(|pair| {
            (
                pair[1] - pair[0],
                tokenizer.decode(&tokens[pair[0]..pair[1]]),
            )
        })
        .collect()
}

/// `split_into_best_sentences` : morceaux d'au plus `max_tokens` jetons
/// (sauf phrase sans virgule plus longue).
pub fn split(
    tokenizer: &Tokenizer,
    text: &str,
    max_tokens: usize,
    config: &PromptConfig,
) -> Vec<String> {
    let Some((text, _)) = prepare(text, config) else {
        return Vec::new();
    };
    let tokens = tokenizer.encode(text.trim_matches(is_space));
    let end_marks: Vec<u32> = tokenizer.encode(".!...?").into_iter().skip(1).collect();
    let indices = boundaries(&tokens, &end_marks, Some(tokenizer));
    let sentences = segments(&tokens, &indices, tokenizer);

    // Phrase trop longue : coupée sur les virgules, points-virgules et
    // deux-points.
    let fallback: Vec<u32> = tokenizer.encode(",;:").into_iter().skip(1).collect();
    let mut refined = Vec::new();
    for (count, sentence) in sentences {
        if count <= max_tokens {
            refined.push((count, sentence));
            continue;
        }
        let sub_tokens = tokenizer.encode(sentence.trim_matches(is_space));
        let sub_indices = boundaries(&sub_tokens, &fallback, None);
        let parts = segments(&sub_tokens, &sub_indices, tokenizer);
        if parts.len() > 1 {
            refined.extend(parts);
        } else {
            refined.push((count, sentence));
        }
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_count = 0;
    for (count, sentence) in refined {
        if current.is_empty() {
            current = sentence;
            current_count = count;
        } else if current_count + count > max_tokens {
            chunks.push(current.trim_matches(is_space).to_owned());
            current = sentence;
            current_count = count;
        } else {
            current.push(' ');
            current.push_str(&sentence);
            current_count += count;
        }
    }
    if !current.is_empty() {
        chunks.push(current.trim_matches(is_space).to_owned());
    }
    chunks
}

/// Plafond de trames d'un morceau de `tokens` jetons.
pub fn max_frames(tokens: usize) -> usize {
    ((tokens as f64 / TOKENS_PER_SECOND + SECONDS_PADDING) * FRAME_RATE).ceil() as usize
}

/// Morceaux à générer pour `text` (comme `generate_audio_stream` : chaque
/// morceau est préparé de nouveau, puis découpé en jetons).
pub fn chunks(tokenizer: &Tokenizer, text: &str, config: &PromptConfig) -> Vec<Chunk> {
    split(tokenizer, text, MAX_TOKENS, config)
        .into_iter()
        .filter_map(|chunk| {
            let (text, guess) = prepare(&chunk, config)?;
            let tokens = tokenizer.encode(&text);
            Some(Chunk {
                frames_after_eos: guess + 2,
                max_frames: max_frames(tokens.len()),
                tokens,
                text,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::tokenizer::tests::{fixture, from_fixture};
    use super::*;

    fn language(code: &str) -> VoiceLanguage {
        match code {
            "fr" => VoiceLanguage::Fr,
            "en" => VoiceLanguage::En,
            _ => VoiceLanguage::Es,
        }
    }

    #[test]
    fn preparation_matches_the_python_package() {
        for code in ["fr", "en", "es"] {
            let fixture = fixture(code);
            let config = PromptConfig::for_language(language(code));
            for case in fixture["prepare"].as_array().unwrap() {
                let text = case[0].as_str().unwrap();
                let expected = (
                    case[1].as_str().unwrap().to_owned(),
                    case[2].as_u64().unwrap() as usize,
                );
                assert_eq!(prepare(text, &config), Some(expected), "{code}: {text:?}");
            }
        }
    }

    #[test]
    fn chunks_match_the_python_package() {
        for code in ["fr", "en", "es"] {
            let fixture = fixture(code);
            let tokenizer = from_fixture(&fixture);
            let config = PromptConfig::for_language(language(code));
            let end: Vec<u32> = serde_json::from_value(fixture["eos_tokens"].clone()).unwrap();
            assert_eq!(
                tokenizer.encode(".!...?")[1..],
                end[..],
                "{code}: end of sentence marks"
            );
            let mut long = 0;
            for case in fixture["chunks"].as_array().unwrap() {
                let text = case[0].as_str().unwrap();
                let expected: Vec<Chunk> = case[1]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| Chunk {
                        text: item["text"].as_str().unwrap().to_owned(),
                        tokens: serde_json::from_value(item["tokens"].clone()).unwrap(),
                        frames_after_eos: item["frames_after_eos"].as_u64().unwrap() as usize,
                        max_frames: item["max_gen_len"].as_u64().unwrap() as usize,
                    })
                    .collect();
                long += usize::from(expected.len() > 1);
                assert_eq!(
                    chunks(&tokenizer, text, &config),
                    expected,
                    "{code}: {text:?}"
                );
            }
            // Le corpus coupe bien des textes en plusieurs morceaux.
            assert!(long >= 2, "{code}");
        }
    }

    #[test]
    fn terminal_punctuation_and_capital() {
        let config = PromptConfig::for_language(VoiceLanguage::En);
        let prepared = |text| prepare(text, &config).unwrap().0;
        assert_eq!(prepared("hello"), "Hello.");
        assert_eq!(prepared("hello,"), "Hello.");
        assert_eq!(prepared("hello —"), "Hello.");
        assert_eq!(prepared("hello!"), "Hello!");
        assert_eq!(prepared("\"hello\""), "\"hello\".");
        assert_eq!(prepared("(she said it.)"), "(she said it.)");
        assert_eq!(prepared("ßa"), "SSa.");
        assert_eq!(prepare("   ", &config), None);
        // Seulement des caractères retirés : rien à lire.
        let french = PromptConfig::for_language(VoiceLanguage::Fr);
        assert_eq!(prepare("« »", &french), None);
        assert!(chunks(&from_fixture(&fixture("fr")), "« »", &french).is_empty());
    }

    #[test]
    fn marks_after_a_sentence_end_are_dropped() {
        assert_eq!(
            drop_marks_after_sentence_end("Oui ?, dit-il"),
            "Oui ? dit-il"
        );
        assert_eq!(drop_marks_after_sentence_end("Non.  ; puis"), "Non. puis");
        assert_eq!(drop_marks_after_sentence_end("?,,"), "?,");
        assert_eq!(drop_marks_after_sentence_end(".?,"), ".?");
    }

    #[test]
    fn the_frame_cap_follows_the_package() {
        assert_eq!(max_frames(0), 25);
        assert_eq!(max_frames(10), 67);
        assert_eq!(max_frames(50), 234);
    }
}
