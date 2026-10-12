//! Prétraitement du texte avant la synthèse (agent T) : Pocket TTS lit mal les
//! chiffres, les unités et les sigles ; on les écrit en toutes lettres.
//!
//! Spécification : `.tools/tts-bench/bin/normalize_fr.py` (français, ses
//! `CASES` sont repris tels quels dans les essais) et le contrat 59 § 3
//! (espagnol, anglais). Sigles épelés en capitales espacées (« A P I »),
//! choix de l'utilisateur. Règles seules, sans modèle.

mod common;
mod en;
mod es;
mod fr;

use crate::Language;

/// `text` (texte parlé) prêt pour la synthèse dans `language`.
pub fn normalize(text: &str, language: Language) -> String {
    match language {
        Language::En => en::normalize(text),
        Language::Es => es::normalize(text),
        Language::Fr => fr::normalize(text),
    }
}
