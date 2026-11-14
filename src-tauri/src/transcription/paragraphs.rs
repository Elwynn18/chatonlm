//! Passages horodatés d'une transcription, regroupés en paragraphes, et
//! textes exportés (`.txt`, `.md`, `.srt`) : fonctions pures, enregistrées
//! par la page (boîte « Enregistrer sous » de `save_document`).
//!
//! Un paragraphe s'arrête après une pause de plus de 1,5 s entre deux
//! passages, ou dès qu'il couvre 60 s : la coupure tombe alors entre deux
//! passages (des phrases avec Parakeet), jamais au milieu.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Passage horodaté ; secondes depuis le début du son.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "TranscriptionSegment")]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Paragraphe : passages joints par une espace.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "TranscriptionParagraph")]
pub struct Paragraph {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Pause qui ouvre un nouveau paragraphe (secondes).
const PARAGRAPH_PAUSE: f64 = 1.5;

/// Durée au-delà de laquelle un paragraphe s'arrête au passage suivant.
const PARAGRAPH_SPAN: f64 = 60.0;

pub fn paragraphs(segments: &[Segment]) -> Vec<Paragraph> {
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    for segment in segments {
        let text = segment.text.trim();
        if text.is_empty() {
            continue;
        }
        match paragraphs.last_mut() {
            Some(last)
                if segment.start - last.end <= PARAGRAPH_PAUSE
                    && segment.start - last.start < PARAGRAPH_SPAN =>
            {
                last.text.push(' ');
                last.text.push_str(text);
                last.end = last.end.max(segment.end);
            }
            _ => paragraphs.push(Paragraph {
                start: segment.start,
                end: segment.end,
                text: text.to_owned(),
            }),
        }
    }
    paragraphs
}

/// Texte enregistré avec la transcription : les paragraphes, séparés par une
/// ligne vide.
pub fn plain_text(segments: &[Segment]) -> String {
    to_text(segments, false)
}

/// Horodatage d'un paragraphe : `[mm:ss]`, ou `[h:mm:ss]` pour tout le
/// texte s'il dure une heure ou plus.
fn stamp(seconds: f64, long: bool) -> String {
    let total = seconds.max(0.0).floor() as u64;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if long {
        format!("[{hours}:{minutes:02}:{seconds:02}]")
    } else {
        format!("[{:02}:{seconds:02}]", total / 60)
    }
}

fn is_long(paragraphs: &[Paragraph]) -> bool {
    paragraphs.last().is_some_and(|last| last.start >= 3600.0)
}

/// `.txt` : paragraphes séparés par une ligne vide, horodatés en tête si
/// demandé.
pub fn to_text(segments: &[Segment], timestamps: bool) -> String {
    let paragraphs = paragraphs(segments);
    let long = is_long(&paragraphs);
    let mut out = String::new();
    for paragraph in &paragraphs {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        if timestamps {
            out.push_str(&stamp(paragraph.start, long));
            out.push(' ');
        }
        out.push_str(&paragraph.text);
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Caractères qui commencent une construction Markdown en tête de ligne.
fn escape_line_start(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let trimmed = text.trim_start();
    let starts_block = trimmed.starts_with(['#', '>', '-', '+', '*', '=', '|', '`', '~'])
        || trimmed.split_once(['.', ')']).is_some_and(|(number, _)| {
            !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
        });
    if starts_block {
        out.push('\\');
    }
    out.push_str(trimmed);
    out
}

/// `.md` : titre (s'il y en a un), puis les paragraphes ; horodatage en gras
/// en tête si demandé. Le texte transcrit est échappé en tête de paragraphe
/// (un « # » ou un « 1. » dicté ne devient pas un titre ni une liste).
pub fn to_markdown(title: &str, segments: &[Segment], timestamps: bool) -> String {
    let paragraphs = paragraphs(segments);
    let long = is_long(&paragraphs);
    let mut out = String::new();
    let title = title.trim();
    if !title.is_empty() {
        out.push_str("# ");
        out.push_str(&title.replace(['\n', '\r'], " "));
        out.push_str("\n\n");
    }
    for (index, paragraph) in paragraphs.iter().enumerate() {
        if index > 0 {
            out.push_str("\n\n");
        }
        if timestamps {
            out.push_str("**");
            out.push_str(&stamp(paragraph.start, long));
            out.push_str("** ");
            out.push_str(paragraph.text.trim());
        } else {
            out.push_str(&escape_line_start(&paragraph.text));
        }
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Horodatage SubRip : `hh:mm:ss,mmm`.
fn srt_time(seconds: f64) -> String {
    let millis = (seconds.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        millis / 3_600_000,
        millis / 60_000 % 60,
        millis / 1000 % 60,
        millis % 1000
    )
}

/// Durée minimale d'un sous-titre (un passage sans durée).
const MIN_CUE: f64 = 0.5;

/// `.srt` : un sous-titre par passage, numérotés à partir de 1, chacun fini
/// avant le début du suivant ; lignes vides retirées du texte (elles
/// fermeraient le sous-titre).
pub fn to_srt(segments: &[Segment]) -> String {
    let kept: Vec<&Segment> = segments
        .iter()
        .filter(|segment| !segment.text.trim().is_empty())
        .collect();
    let mut out = String::new();
    for (index, segment) in kept.iter().enumerate() {
        let start = segment.start.max(0.0);
        let mut end = segment.end.max(start + MIN_CUE);
        if let Some(next) = kept.get(index + 1) {
            end = end.min(next.start.max(start));
        }
        let text: Vec<&str> = segment
            .text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            index + 1,
            srt_time(start),
            srt_time(end),
            text.join("\n")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(start: f64, end: f64, text: &str) -> Segment {
        Segment {
            start,
            end,
            text: text.into(),
        }
    }

    #[test]
    fn pauses_and_length_make_paragraphs() {
        let segments = [
            segment(0.0, 2.0, "Bonjour."),
            segment(2.5, 4.0, " Ça va ? "),
            // Pause de 2 s : nouveau paragraphe.
            segment(6.0, 8.0, "Oui."),
            segment(8.0, 8.0, "   "),
        ];
        let found = paragraphs(&segments);
        assert_eq!(
            found,
            [
                Paragraph {
                    start: 0.0,
                    end: 4.0,
                    text: "Bonjour. Ça va ?".into()
                },
                Paragraph {
                    start: 6.0,
                    end: 8.0,
                    text: "Oui.".into()
                }
            ]
        );
        // Parole continue : un paragraphe au plus toutes les 60 s, coupé
        // entre deux passages.
        let long: Vec<Segment> = (0..30)
            .map(|i| segment(f64::from(i) * 5.0, f64::from(i) * 5.0 + 5.0, "Phrase."))
            .collect();
        let found = paragraphs(&long);
        assert_eq!(
            found.iter().map(|p| p.start).collect::<Vec<_>>(),
            [0.0, 60.0, 120.0]
        );
        assert_eq!(found[0].text.matches("Phrase.").count(), 12);
    }

    #[test]
    fn text_and_markdown_exports() {
        let segments = [
            segment(0.0, 2.0, "Bonjour."),
            segment(65.0, 70.0, "# pas un titre"),
        ];
        assert_eq!(to_text(&segments, false), "Bonjour.\n\n# pas un titre\n");
        assert_eq!(
            to_text(&segments, true),
            "[00:00] Bonjour.\n\n[01:05] # pas un titre\n"
        );
        assert_eq!(plain_text(&segments), to_text(&segments, false));
        assert_eq!(
            to_markdown("Cours 1", &segments, false),
            "# Cours 1\n\nBonjour.\n\n\\# pas un titre\n"
        );
        assert_eq!(
            to_markdown("", &segments, true),
            "**[00:00]** Bonjour.\n\n**[01:05]** # pas un titre\n"
        );
        assert_eq!(
            to_markdown("", &[segment(0.0, 1.0, "1. un")], false),
            "\\1. un\n"
        );
        assert_eq!(to_text(&[], true), "");
        // Plus d'une heure : heures partout.
        let long = [segment(5.0, 6.0, "a"), segment(3725.0, 3726.0, "b")];
        assert_eq!(to_text(&long, true), "[0:00:05] a\n\n[1:02:05] b\n");
    }

    #[test]
    fn subrip_cues_are_numbered_and_never_overlap() {
        let segments = [
            segment(0.0, 2.5, "Premier."),
            segment(2.0, 2.0, "Deuxième\n\nligne."),
            segment(3.0, 3.0, " "),
            segment(3725.25, 3726.5, "Fin."),
        ];
        assert_eq!(
            to_srt(&segments),
            "1\n00:00:00,000 --> 00:00:02,000\nPremier.\n\n\
             2\n00:00:02,000 --> 00:00:02,500\nDeuxième\nligne.\n\n\
             3\n01:02:05,250 --> 01:02:06,500\nFin.\n\n"
        );
        assert_eq!(to_srt(&[]), "");
    }
}
