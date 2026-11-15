//! Consignes de la synthèse d'une transcription (« Synthétiser ») : notes sur
//! une partie, notes regroupées, document de lecture final. Envoyées au
//! modèle en anglais (consignes, pas des textes de l'interface) ; la langue
//! du document y est nommée.
//!
//! Le faux moteur (`fake-llama`, scénarios `chatonlm-synthesis*`) reconnaît
//! la demande à ses marques : [`fake_llama::scenarios::synthesis::PART_MARK`]
//! (balise d'une partie), `DOCUMENT_MARK` (« reading document ») et
//! `RETRY_MARK` (rappel de la langue) ; un essai vérifie qu'elles y sont.

use crate::inference::Message;

/// Consigne des notes sur une partie.
const NOTES_PROMPT: &str = "You turn one part of the transcript of a recording into notes. \
The transcript comes from speech recognition: it can contain recognition mistakes, \
repetitions and filler words. Write the notes {lang}. List, as short bullet points that \
start with \"- \", the ideas, facts, names, numbers, examples and conclusions of this part, \
in their order. Use only what the transcript says: add nothing of your own and do not \
comment on the transcript. No timestamps, no introduction: reply with the notes only.";

/// Consigne du regroupement de notes trop longues pour le document.
const MERGE_PROMPT: &str = "You merge notes taken on consecutive parts of the transcript of a \
recording into shorter notes. Write {lang}. Keep every idea, fact, name, number and \
conclusion, in their order, and remove repetitions. Use only what the notes say. Reply with \
the notes only, as short bullet points that start with \"- \".";

/// Consigne du document (« reading document » : marque du faux moteur).
const DOCUMENT_PROMPT: &str = "You write a reading document in Markdown from {source}. \
Write everything {lang}. Structure: a title (#), a short introduction, then sections with \
headings (##) that follow the order of the content, written as clear prose (with lists where \
they help), then a section of key points (a list of the main points, its heading written \
{lang}), and a short conclusion. Be faithful: use only what the {short} says, add no facts, \
opinions or examples of your own, and do not mention the transcript, the recording quality \
or speech recognition. No timestamps. It is a document to read, not slides. Reply with the \
document only, without code fences.";

/// Langue demandée, pour les consignes (« in French ») ; inconnue : celle
/// de la transcription.
fn language_clause(language: Option<&str>) -> String {
    match language {
        Some(name) => format!("in {name}"),
        None => "in the language of the transcript".to_owned(),
    }
}

fn titled(title: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        String::new()
    } else {
        format!("Title of the recording: {title}\n\n")
    }
}

/// Notes sur la partie `index` (à partir de 1) de `count`.
pub fn notes(
    language: Option<&str>,
    title: &str,
    index: usize,
    count: usize,
    text: &str,
) -> Vec<Message> {
    vec![
        Message::system(NOTES_PROMPT.replace("{lang}", &language_clause(language))),
        Message::user(format!(
            "{}Part {index} of {count} of the transcript:\n\
             <transcript part=\"{index}/{count}\">\n{text}\n</transcript>",
            titled(title)
        )),
    ]
}

/// Notes regroupées.
pub fn merge(language: Option<&str>, notes: &str) -> Vec<Message> {
    vec![
        Message::system(MERGE_PROMPT.replace("{lang}", &language_clause(language))),
        Message::user(format!("<notes>\n{notes}\n</notes>")),
    ]
}

/// D'où vient le document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum From<'a> {
    /// Transcription entière (assez courte).
    Transcript(&'a str),
    /// Notes des parties d'une transcription longue.
    Notes(&'a str),
}

/// Document final ; `retry` : rappel de la langue après une réponse écrite
/// dans une autre.
pub fn document(language: Option<&str>, title: &str, from: From<'_>, retry: bool) -> Vec<Message> {
    let lang = language_clause(language);
    let (source, short, body) = match from {
        From::Transcript(text) => (
            "the transcript of a recording",
            "transcript",
            format!("<transcript>\n{text}\n</transcript>"),
        ),
        From::Notes(notes) => (
            "notes taken on the transcript of a recording",
            "notes",
            format!("<notes>\n{notes}\n</notes>"),
        ),
    };
    let reminder = match (retry, language) {
        (true, Some(name)) => format!(
            "Your previous answer was not written in {name}. Write the whole document in \
             {name}.\n\n"
        ),
        _ => String::new(),
    };
    vec![
        Message::system(
            DOCUMENT_PROMPT
                .replace("{source}", source)
                .replace("{short}", short)
                .replace("{lang}", &lang),
        ),
        Message::user(format!("{reminder}{}{body}", titled(title))),
    ]
}

/// Taille de la consigne la plus longue avec son titre (caractères), pour
/// le budget du contexte.
pub fn longest_prompt_chars(title: &str) -> usize {
    [NOTES_PROMPT, MERGE_PROMPT, DOCUMENT_PROMPT]
        .iter()
        .map(|prompt| prompt.chars().count())
        .max()
        .unwrap_or(0)
        + titled(title).chars().count()
        + 200
}

#[cfg(test)]
mod tests {
    use fake_llama::scenarios::synthesis::{DOCUMENT_MARK, PART_MARK, RETRY_MARK};

    use super::*;
    use crate::inference::{Content, Role};

    fn texts(messages: &[Message]) -> Vec<(Role, String)> {
        messages
            .iter()
            .map(|message| {
                let text = match &message.content {
                    Content::Text(text) => text.clone(),
                    other => panic!("{other:?}"),
                };
                (message.role, text)
            })
            .collect()
    }

    /// Les marques du faux moteur sont dans les consignes ; la langue est
    /// nommée partout, sinon celle de la transcription est demandée.
    #[test]
    fn prompts_carry_the_marks_and_the_language() {
        let part = texts(&notes(Some("French"), "Cours", 2, 3, "Bonjour."));
        assert!(part[0].1.contains("in French"));
        assert!(part[1].1.contains(PART_MARK));
        assert!(part[1].1.starts_with("Title of the recording: Cours\n\n"));
        assert!(
            part[1]
                .1
                .contains("<transcript part=\"2/3\">\nBonjour.\n</transcript>")
        );
        assert!(!part[0].1.contains(DOCUMENT_MARK));
        let merged = texts(&merge(None, "- a"));
        assert!(merged[0].1.contains("in the language of the transcript"));
        assert!(!merged[0].1.contains(DOCUMENT_MARK));
        let direct = texts(&document(
            Some("Spanish"),
            "",
            From::Transcript("Hola."),
            false,
        ));
        assert!(direct[0].1.contains(DOCUMENT_MARK));
        assert!(direct[0].1.contains("Write everything in Spanish"));
        assert!(!direct[0].1.contains("{"), "{}", direct[0].1);
        assert_eq!(direct[1].1, "<transcript>\nHola.\n</transcript>");
        let notes = texts(&document(Some("English"), "", From::Notes("- a"), true));
        assert!(notes[1].1.contains(RETRY_MARK));
        assert!(notes[1].1.ends_with("<notes>\n- a\n</notes>"));
        // Sans langue connue, pas de rappel possible.
        let unknown = texts(&document(None, "", From::Notes("- a"), true));
        assert!(!unknown[1].1.contains(RETRY_MARK));
    }
}
