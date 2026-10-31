//! Dictée tenue par une connexion du pont VS Code (lot 4b, propre à
//! ChatonLM) : le texte final va à l'éditeur, qui l'insère au curseur ;
//! jamais collé, jamais copié dans le presse-papiers.
//!
//! - Une seule dictée dans l'app : refus si la page, le raccourci, la zone
//!   de notification, le mode vocal ou une autre connexion l'occupe
//!   ([`EditorStartError::Busy`]).
//! - Glossaire de la séance (noms du projet) ajouté, pour elle seule, au
//!   dictionnaire de l'utilisateur, jamais enregistré ([`new_words`]) ; il a
//!   sa part de la consigne de Whisper, même après un long dictionnaire
//!   (`engine::Request::project`).
//! - Modèle de transcription comme le mode vocal : celui de la dictée, sinon
//!   le premier installé (l'interrupteur de la dictée, qui règle le raccourci,
//!   ne la concerne pas) ; nettoyage selon les réglages de la dictée.
//! - Ni notification du bureau, ni « Last dictation » : l'éditeur montre
//!   l'écoute et le texte ; la page montre le bandeau « VS Code is using your
//!   microphone » (`DictationStatus.editor`).

use parking_lot::Mutex;
use tokio::sync::oneshot;

use super::DictationPhase;
use crate::store::dictation::{MAX_DICTIONARY_WORDS, MAX_WORD_CHARS};

/// Connexion du pont qui tient la dictée (voir `bridge::dictation`).
/// Appelée sous le verrou de la dictée : rien n'y rappelle la dictée.
pub(crate) trait DictationLink: Send + Sync + 'static {
    fn connection(&self) -> u64;
    /// Glossaire de la séance, déjà nettoyé ([`clean_glossary`]).
    fn glossary(&self) -> Vec<String>;
    /// Connexion toujours ouverte : sinon rien ne s'ouvre pour elle.
    fn alive(&self) -> bool;
    /// Phase, à partir de l'ouverture du micro (`Recording`) ; `Idle` à la
    /// fin, quelle qu'elle soit (jamais si le micro ne s'est pas ouvert).
    fn phase(&self, phase: DictationPhase);
    /// Fin, avant `Idle` : texte final, ou échec après l'ouverture du micro.
    fn ended(&self, end: EditorEnd);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditorEnd {
    Text(String),
    /// Raison déjà traduite (rien entendu, transcription en échec…).
    Failed(String),
}

/// Qui occupe la dictée.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DictationOwner {
    /// Page, raccourci, zone de notification ou `--dictate`.
    App,
    /// Une connexion du pont (celle-ci ou une autre).
    Bridge,
    /// Séance du mode vocal (de la page ou d'un éditeur).
    VoiceMode,
}

/// Refus ou échec du démarrage (avant l'ouverture du micro).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorStartError {
    Busy(DictationOwner),
    SpeechModelMissing,
    /// Raison déjà traduite (machine incapable, micro…).
    Failed(String),
    /// Arrêtée avant l'ouverture du micro (arrêt demandé, connexion fermée,
    /// app qui se ferme).
    Cancelled,
}

/// Résultat du démarrage, rendu quand le micro est ouvert.
pub(crate) type Opened = oneshot::Receiver<Result<(), EditorStartError>>;

/// Dictée en cours d'un éditeur.
pub(crate) struct EditorSession {
    pub(crate) link: std::sync::Arc<dyn DictationLink>,
    /// Réponse du démarrage, tant que le micro n'est pas ouvert.
    opened: Mutex<Option<oneshot::Sender<Result<(), EditorStartError>>>>,
}

impl EditorSession {
    pub(crate) fn new(link: std::sync::Arc<dyn DictationLink>) -> (Self, Opened) {
        let (sender, receiver) = oneshot::channel();
        let session = Self {
            link,
            opened: Mutex::new(Some(sender)),
        };
        (session, receiver)
    }

    /// Le micro n'est pas encore ouvert (démarrage sans réponse).
    pub(crate) fn starting(&self) -> bool {
        self.opened.lock().is_some()
    }

    /// Répond au démarrage s'il attend encore ; vrai s'il attendait.
    pub(crate) fn answer(&self, result: Result<(), EditorStartError>) -> bool {
        match self.opened.lock().take() {
            Some(sender) => {
                let _ = sender.send(result);
                true
            }
            None => false,
        }
    }
}

/// Glossaire envoyé par l'éditeur, nettoyé : espaces retirés aux bords ;
/// vides, trop longs (plus de 64 caractères), caractères de contrôle et
/// doublons (sans tenir compte de la casse) écartés ; 200 mots au plus, les
/// premiers (l'éditeur met d'abord les plus proches du fichier ouvert).
pub fn clean_glossary(words: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for word in words {
        if kept.len() == MAX_DICTIONARY_WORDS {
            break;
        }
        let word = word.trim();
        if word.is_empty()
            || word.chars().count() > MAX_WORD_CHARS
            || word.chars().any(char::is_control)
        {
            continue;
        }
        if !contains(&kept, word) {
            kept.push(word.to_owned());
        }
    }
    kept
}

/// Mots de la séance que le dictionnaire de l'utilisateur n'a pas déjà
/// (sans tenir compte de la casse), dans leur ordre ; ajoutés après lui, en
/// mémoire seulement.
pub fn new_words(dictionary: &[String], glossary: &[String]) -> Vec<String> {
    let mut added: Vec<String> = Vec::new();
    for word in glossary {
        if !contains(dictionary, word) && !contains(&added, word) {
            added.push(word.clone());
        }
    }
    added
}

fn contains(words: &[String], word: &str) -> bool {
    let word = word.to_lowercase();
    words.iter().any(|seen| seen.to_lowercase() == word)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn the_glossary_is_cleaned_and_capped() {
        let long = "x".repeat(MAX_WORD_CHARS + 1);
        let sent = words(&[
            " useDictation ",
            "",
            "   ",
            &long,
            "bad\nword",
            "UseDictation",
            "snake_case_name",
        ]);
        assert_eq!(
            clean_glossary(&sent),
            words(&["useDictation", "snake_case_name"])
        );
        let many: Vec<String> = (0..300).map(|i| format!("name{i}")).collect();
        let kept = clean_glossary(&many);
        assert_eq!(kept.len(), MAX_DICTIONARY_WORDS);
        assert_eq!(kept[0], "name0", "the first ones are kept");
    }

    #[test]
    fn the_glossary_adds_what_the_user_dictionary_lacks() {
        assert_eq!(
            new_words(
                &words(&["ChatonLM", "Ollama"]),
                &words(&["ollama", "GpuChief", "gpuchief"])
            ),
            words(&["GpuChief"])
        );
        assert_eq!(new_words(&[], &words(&["a"])), words(&["a"]));
    }
}
