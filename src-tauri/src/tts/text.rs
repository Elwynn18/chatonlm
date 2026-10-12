//! Texte parlé : façade du crate `speech-text` (Markdown, langue,
//! prétraitement), aux types de l'app.

use speech_text::Language;
use speech_text::language;
pub use speech_text::markdown::Segmenter;

use super::VoiceLanguage;

/// Texte parlé d'une réponse en Markdown.
pub fn spoken_text(markdown: &str) -> String {
    speech_text::markdown::spoken_text(markdown)
}

/// Langue de lecture de `spoken` : détectée, sinon `interface` ; `None` :
/// autre langue.
pub fn choose_language(spoken: &str, interface: VoiceLanguage) -> Option<VoiceLanguage> {
    language::choose(language::detect(spoken), to_text(interface)).map(from_text)
}

/// `text` prêt pour la synthèse dans `language`.
pub fn normalize(text: &str, language: VoiceLanguage) -> String {
    speech_text::normalize::normalize(text, to_text(language))
}

fn to_text(language: VoiceLanguage) -> Language {
    match language {
        VoiceLanguage::En => Language::En,
        VoiceLanguage::Es => Language::Es,
        VoiceLanguage::Fr => Language::Fr,
    }
}

fn from_text(language: Language) -> VoiceLanguage {
    match language {
        Language::En => VoiceLanguage::En,
        Language::Es => VoiceLanguage::Es,
        Language::Fr => VoiceLanguage::Fr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_of_an_answer_chooses_its_voice() {
        let french = "Pour corriger l'erreur, ouvre le fichier de configuration et relance le serveur avec npm run dev, puis vérifie que la page se recharge.";
        assert_eq!(
            choose_language(french, VoiceLanguage::En),
            Some(VoiceLanguage::Fr)
        );
        // Trop court : la langue de l'interface.
        assert_eq!(
            choose_language("OK.", VoiceLanguage::Es),
            Some(VoiceLanguage::Es)
        );
        let german = "Das ist eine kurze Antwort auf deine Frage, und sie ist ganz sicher nicht auf Englisch geschrieben, sondern auf Deutsch.";
        assert_eq!(choose_language(german, VoiceLanguage::Fr), None);
    }

    #[test]
    fn numbers_are_written_in_the_voice_language() {
        assert_eq!(
            normalize("21 minutes", VoiceLanguage::Fr),
            "vingt et une minutes"
        );
        assert_eq!(
            normalize("21 minutes", VoiceLanguage::En),
            "twenty-one minutes"
        );
    }
}
