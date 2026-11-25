//! Relecture de la langue du livrable (banc du 25 nov. : « terre
//! végétaine », « tu tasse », « tu paillis », « laisse leur laisser » restés
//! dans le livrable). Avant la livraison, le texte est relu par passages de
//! paragraphes consécutifs (au plus [`BLOCK_WORDS`] mots, jamais un titre ni
//! un bloc de code), réflexion coupée, avec les consignes, les exemples et
//! le garde-fou de la Correction de l'app (`crate::proofread`, mode
//! « Correct », nombres gardés en chiffres), puis un garde-fou propre au
//! livrable ([`keeps_shape`]) : longueur à ±5 %, mêmes lignes, mêmes titres,
//! mêmes tableaux, mêmes chiffres. Une correction écartée laisse le passage
//! tel quel (compté au rapport).

use crate::agent::{ThinkMode, Write};
use crate::proofread::{ProofreadMode, ProofreadOptions, answer, guard, prompts as proof};

use super::flow::soft;
use super::text::{is_heading, words};
use super::types::{Activity, StateKey};
use super::{Run, Stop};

/// Mots d'un passage relu, au plus (un paragraphe plus long reste seul).
pub const BLOCK_WORDS: u32 = 300;

/// Correction seule ; mise en forme automatique coupée (le livrable a la
/// sienne) ; nombres gardés en chiffres.
const OPTIONS: ProofreadOptions = ProofreadOptions {
    keep_order: false,
    formatting: false,
    digits: true,
};

/// Écart de longueur toléré (mots) : ±5 %, au moins 2 mots (« laisse leur
/// laisser » → « leur laisser »).
fn length_kept(before: u32, after: u32) -> bool {
    let tolerance = (f64::from(before) * 0.05).max(2.0);
    (f64::from(after) - f64::from(before)).abs() <= tolerance
}

/// Une ligne de tableau Markdown.
fn table_row(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

/// La correction garde la forme du passage : longueur à ±5 %, même nombre
/// de lignes, mêmes titres, mêmes lignes de tableau (mêmes cellules), mêmes
/// chiffres dans le même ordre.
pub(super) fn keeps_shape(original: &str, corrected: &str) -> bool {
    let lines = |text: &str| text.lines().map(str::to_owned).collect::<Vec<_>>();
    let (before, after) = (lines(original), lines(corrected));
    let headings = |lines: &[String]| -> Vec<String> {
        lines.iter().filter(|l| is_heading(l)).cloned().collect()
    };
    let cells = |lines: &[String]| -> Vec<(usize, usize)> {
        lines
            .iter()
            .enumerate()
            .filter(|(_, l)| table_row(l))
            .map(|(i, l)| (i, l.matches('|').count()))
            .collect()
    };
    let digits = |text: &str| -> String { text.chars().filter(char::is_ascii_digit).collect() };
    !corrected.trim().is_empty()
        && length_kept(words(original), words(corrected))
        && before.len() == after.len()
        && headings(&before) == headings(&after)
        && cells(&before) == cells(&after)
        && digits(original) == digits(corrected)
}

/// Morceau du livrable : un paragraphe (plage d'octets), ou une barrière
/// (titre, bloc de code) qu'un passage ne franchit pas.
enum Piece {
    Paragraph(usize, usize),
    Barrier,
}

/// Passages à relire : plages d'octets de paragraphes consécutifs (au plus
/// [`BLOCK_WORDS`] mots), sans titre ni bloc de code.
pub(super) fn blocks(doc: &str) -> Vec<(usize, usize)> {
    let mut pieces: Vec<Piece> = Vec::new();
    let mut open: Option<(usize, usize)> = None;
    let mut code = false;
    let mut at = 0;
    for line in doc.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let content = line.trim_end_matches(['\n', '\r']);
        let end = start + content.len();
        let fence = content.trim_start().starts_with("```");
        let barrier = fence || code || is_heading(content);
        if barrier || content.trim().is_empty() {
            if let Some((s, e)) = open.take() {
                pieces.push(Piece::Paragraph(s, e));
            }
            if fence {
                code = !code;
            }
            if barrier {
                pieces.push(Piece::Barrier);
            }
            continue;
        }
        open = Some(open.map_or((start, end), |(s, _)| (s, end)));
    }
    if let Some((s, e)) = open {
        pieces.push(Piece::Paragraph(s, e));
    }
    let mut out = Vec::new();
    let mut current: Option<(usize, usize, u32)> = None;
    for piece in pieces {
        match piece {
            Piece::Barrier => {
                if let Some((s, e, _)) = current.take() {
                    out.push((s, e));
                }
            }
            Piece::Paragraph(s, e) => {
                let n = words(&doc[s..e]);
                current = match current {
                    Some((cs, _, cw)) if cw + n <= BLOCK_WORDS => Some((cs, e, cw + n)),
                    Some((cs, ce, _)) => {
                        out.push((cs, ce));
                        Some((s, e, n))
                    }
                    None => Some((s, e, n)),
                };
            }
        }
    }
    if let Some((s, e, _)) = current {
        out.push((s, e));
    }
    out
}

/// Bilan de la relecture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Proofread {
    pub blocks: u32,
    pub changed: u32,
    pub rejected: u32,
}

impl Run<'_> {
    /// Relit la langue de `doc`, passage par passage ; rend le livrable
    /// corrigé et le bilan. Chaque passage a sa clé d'état (reprise).
    pub(super) async fn proofread(&self, doc: &str) -> Result<(String, Proofread), Stop> {
        let ranges = blocks(doc);
        let mut summary = Proofread::default();
        let mut changes: Vec<(usize, usize, String)> = Vec::new();
        for (i, (start, end)) in ranges.iter().enumerate() {
            let block = u32::try_from(i).unwrap_or(u32::MAX);
            let original = &doc[*start..*end];
            summary.blocks += 1;
            let write = Write {
                temperature: proof::TEMPERATURE,
                think: ThinkMode::Off,
                max_tokens: Some(words(original).saturating_mul(4) + 1_000),
            };
            let reply = soft(
                self.text(
                    StateKey::Proofread(block),
                    Activity::Proofreading { block },
                    proof::messages(ProofreadMode::Correct, OPTIONS, original),
                    write,
                )
                .await,
            )?;
            let corrected = reply
                .map(|reply| answer::clean(original, &reply))
                .and_then(|reply| {
                    guard::settle_with(ProofreadMode::Correct, OPTIONS, original, reply)
                })
                .filter(|reply| keeps_shape(original, reply));
            match corrected {
                Some(text) if text != original => {
                    summary.changed += 1;
                    changes.push((*start, *end, text));
                }
                Some(_) => {}
                None => summary.rejected += 1,
            }
        }
        if summary.rejected > 0 {
            self.warn("proofreadRejected").await;
        }
        let mut out = doc.to_owned();
        for (start, end, text) in changes.into_iter().rev() {
            out.replace_range(start..end, &text);
        }
        Ok((out, summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_skip_titles_and_code() {
        let doc = "# Titre\n\nUn deux trois.\nQuatre.\n\nCinq six.\n\n## Partie\n\nSept.\n\n\
                   ```\ncode ici\n```\n\nHuit.";
        let texts: Vec<&str> = blocks(doc).iter().map(|(s, e)| &doc[*s..*e]).collect();
        assert_eq!(
            texts,
            vec!["Un deux trois.\nQuatre.\n\nCinq six.", "Sept.", "Huit."]
        );
        // Au-delà de 300 mots, un nouveau passage.
        let long = format!("{}\n\n{}", "mot ".repeat(200), "mot ".repeat(200));
        assert_eq!(blocks(&long).len(), 2);
    }

    #[test]
    fn a_correction_must_keep_the_shape() {
        let original = "Tu tasse la terre végétaine, 3 cm.\n| Radis | 3 cm |";
        assert!(keeps_shape(
            original,
            "Tu tasses la terre végétale, 3 cm.\n| Radis | 3 cm |"
        ));
        // Chiffre changé, cellule perdue, ligne en moins, texte allongé.
        assert!(!keeps_shape(
            original,
            "Tu tasses la terre végétale, 4 cm.\n| Radis | 3 cm |"
        ));
        assert!(!keeps_shape(
            original,
            "Tu tasses la terre végétale, 3 cm.\n| Radis 3 cm |"
        ));
        assert!(!keeps_shape(
            original,
            "Tu tasses la terre, 3 cm. Radis 3 cm"
        ));
        assert!(!keeps_shape(
            original,
            "Tu tasses bien fort la bonne terre végétale du jardin, 3 cm.\n| Radis | 3 cm |"
        ));
    }
}
