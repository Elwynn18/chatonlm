//! Langues proposées pour la dictée (réglage « Language »).

use serde::Serialize;
use ts_rs::TS;

/// Une langue du réglage : code ISO 639-1, nom anglais (interface en
/// anglais), et prise en charge par Parakeet v3 (25 langues européennes ;
/// Whisper les prend toutes).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DictationLanguage {
    pub code: &'static str,
    pub name: &'static str,
    pub parakeet: bool,
}

const fn lang(code: &'static str, name: &'static str, parakeet: bool) -> DictationLanguage {
    DictationLanguage {
        code,
        name,
        parakeet,
    }
}

/// Par nom anglais. « auto » (détection) n'en fait pas partie.
pub const LANGUAGES: &[DictationLanguage] = &[
    lang("ar", "Arabic", false),
    lang("bg", "Bulgarian", true),
    lang("ca", "Catalan", false),
    lang("zh", "Chinese", false),
    lang("hr", "Croatian", true),
    lang("cs", "Czech", true),
    lang("da", "Danish", true),
    lang("nl", "Dutch", true),
    lang("en", "English", true),
    lang("et", "Estonian", true),
    lang("fi", "Finnish", true),
    lang("fr", "French", true),
    lang("de", "German", true),
    lang("el", "Greek", true),
    lang("he", "Hebrew", false),
    lang("hi", "Hindi", false),
    lang("hu", "Hungarian", true),
    lang("id", "Indonesian", false),
    lang("it", "Italian", true),
    lang("ja", "Japanese", false),
    lang("ko", "Korean", false),
    lang("lv", "Latvian", true),
    lang("lt", "Lithuanian", true),
    lang("ms", "Malay", false),
    lang("mt", "Maltese", true),
    lang("no", "Norwegian", false),
    lang("fa", "Persian", false),
    lang("pl", "Polish", true),
    lang("pt", "Portuguese", true),
    lang("ro", "Romanian", true),
    lang("ru", "Russian", true),
    lang("sr", "Serbian", false),
    lang("sk", "Slovak", true),
    lang("sl", "Slovenian", true),
    lang("es", "Spanish", true),
    lang("sv", "Swedish", true),
    lang("th", "Thai", false),
    lang("tr", "Turkish", false),
    lang("uk", "Ukrainian", true),
    lang("vi", "Vietnamese", false),
];

/// Valeur du réglage pour la détection automatique.
pub const AUTO: &str = "auto";

/// Code accepté par le réglage : « auto » ou une langue de [`LANGUAGES`].
pub fn is_supported(code: &str) -> bool {
    code == AUTO || LANGUAGES.iter().any(|language| language.code == code)
}

/// Nom anglais d'un code (pour les consignes du nettoyage) ; `None` pour
/// « auto » ou un code inconnu.
pub fn name_of(code: &str) -> Option<&'static str> {
    LANGUAGES
        .iter()
        .find(|language| language.code == code)
        .map(|language| language.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_parakeet_has_25_languages() {
        let mut codes: Vec<_> = LANGUAGES.iter().map(|language| language.code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), LANGUAGES.len());
        assert_eq!(
            LANGUAGES
                .iter()
                .filter(|language| language.parakeet)
                .count(),
            25
        );
        assert!(is_supported("auto") && is_supported("fr") && !is_supported("xx"));
        assert_eq!(name_of("fr"), Some("French"));
    }
}
