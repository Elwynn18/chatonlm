//! Textes lisibles des erreurs d'un téléchargement depuis Hugging Face, pour
//! le ModelHub comme pour le téléchargement du chat (contrat
//! `37-step15a-contract.md` § 9). Textes dans la langue de l'interface au
//! moment de l'échec.

use std::fmt::Display;

use crate::i18n::t;

/// Étiquette inconnue du dépôt, ou qui n'est pas une quantification.
pub fn unavailable_version() -> String {
    t!("downloads.unavailableVersion")
}

/// Dépôt sans fichier GGUF utilisable.
pub fn not_gguf() -> String {
    t!("downloads.notGguf")
}

/// Dépôt inexistant, privé ou à accès restreint : Hugging Face répond 401
/// dans les trois cas.
pub fn not_found_or_private() -> String {
    t!("downloads.notFoundOrPrivate")
}

/// Plus aucune ligne du téléchargement pendant le délai d'inactivité
/// (ModelHub).
pub fn timed_out() -> String {
    t!("downloads.timedOut")
}

/// Refus d'un téléchargement pendant un changement de dossier des modèles.
pub fn location_changing() -> String {
    t!("downloads.locationChanging")
}

/// Flux coupé (téléchargement ou réponse du chat), avec le détail technique
/// de la connexion s'il y en a un.
pub fn connection_lost(detail: Option<&dyn Display>) -> String {
    match detail {
        Some(detail) => t!("errors.connectionLostDetail", error = detail),
        None => t!("errors.connectionLost"),
    }
}

/// Textes anglais des échecs, pour les essais (qui tournent en anglais) :
/// ceux de l'officiel et de ChatonLM, inchangés.
#[cfg(test)]
pub mod english {
    pub const UNAVAILABLE_VERSION: &str =
        "This version isn't available on Hugging Face. Choose another version.";
    pub const NOT_GGUF: &str = "This repository has no model file ChatonLM can use.";
    pub const NOT_FOUND_OR_PRIVATE: &str =
        "This model doesn't exist on Hugging Face, is private, or needs an account to download.";
    pub const TIMED_OUT: &str = "Hugging Face didn't answer in time. Try again.";
    pub const CONNECTION_LOST: &str = "Connection lost";
    pub const LOCATION_CHANGING: &str =
        "the model location is being changed: try again once it's done";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_follow_the_language() {
        use crate::i18n::{Lang, text};
        assert_eq!(
            text(Lang::Fr, "downloads.timedOut", None, &[]),
            "Hugging Face n'a pas répondu à temps. Réessayez."
        );
        assert_eq!(
            connection_lost(Some(&"reset by peer")),
            "Connection lost: reset by peer"
        );
        assert_eq!(connection_lost(None), "Connection lost");
    }
}
