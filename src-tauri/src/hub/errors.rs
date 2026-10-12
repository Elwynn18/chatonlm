//! Textes lisibles des erreurs d'un téléchargement (`POST /api/pull`), pour le
//! ModelHub comme pour le pull du chat. Contrat : `37-step15a-contract.md` § 9 ;
//! erreurs d'Ollama et du registre de Hugging Face relevées dans
//! `34-modelhub-ollama-hf.md` § 2-3 et `35-modelhub-hf-api.md` § 2.4.
//!
//! Ollama relaie les refus du registre de Hugging Face sous la forme
//! `pull model manifest: 400: {"error":"…"}` : le JSON imbriqué est extrait.
//!
//! Textes dans la langue de l'interface au moment de l'échec ; les messages
//! d'Ollama et de Hugging Face restent tels quels.

use std::fmt::Display;

use crate::chat::pull::{NETWORK_ERRORS, offline};
use crate::i18n::t;

/// Version découpée en plusieurs fichiers (« This tag is a sharded GGUF… »,
/// « This repository only contains sharded GGUF files… »).
pub fn sharded() -> String {
    t!("downloads.sharded")
}

/// Étiquette inconnue du dépôt, ou qui n'est pas une quantification.
pub fn unavailable_version() -> String {
    t!("downloads.unavailableVersion")
}

/// Dépôt sans fichier GGUF utilisable.
pub fn not_gguf() -> String {
    t!("downloads.notGguf")
}

/// Dépôt inexistant, privé ou à accès restreint : Hugging Face répond 401
/// dans les trois cas, et Ollama les confond (« realm host … »).
pub fn not_found_or_private() -> String {
    t!("downloads.notFoundOrPrivate")
}

/// Ollama injoignable (ModelHub).
pub fn unreachable() -> String {
    t!("downloads.unreachable")
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

/// Téléchargement arrêté : ChatonLM parle maintenant à un autre serveur
/// (Ollama intégré ou de la machine, adresse changée).
pub fn server_changed() -> String {
    t!("downloads.serverChanged")
}

/// Flux coupé (téléchargement ou réponse du chat), avec le détail technique
/// de la connexion s'il y en a un.
pub fn connection_lost(detail: Option<&dyn Display>) -> String {
    match detail {
        Some(detail) => t!("errors.connectionLostDetail", error = detail),
        None => t!("errors.connectionLost"),
    }
}

/// Texte montré pour l'erreur `error` d'un téléchargement : cas connus de
/// Hugging Face, puis manifeste injoignable faute de réseau (texte officiel
/// [`offline`], comme `getError` de l'app officielle), puis message d'un JSON
/// imbriqué, sinon le texte tel quel.
pub fn pull_error(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    let has = |pattern: &str| lower.contains(pattern);
    if has("sharded gguf") {
        return sharded();
    }
    if has("not a valid quantization scheme") || has("not available in the repository") {
        return unavailable_version();
    }
    if has("repository is not gguf") {
        return not_gguf();
    }
    if (has("realm host") && has("does not match original host"))
        || has("invalid username or password")
        || (has("access to model") && has("is restricted"))
    {
        return not_found_or_private();
    }
    // Comme l'officiel : seulement pour le manifeste (une erreur réseau pendant
    // une couche reste brute).
    if error.starts_with("pull model manifest")
        && NETWORK_ERRORS.iter().any(|pattern| error.contains(pattern))
    {
        return offline();
    }
    nested_message(error).unwrap_or_else(|| error.to_owned())
}

/// Message `error` du JSON qui termine le texte (`…: 400: {"error":"…"}`), s'il
/// y en a un, non vide.
fn nested_message(error: &str) -> Option<String> {
    let start = error.find('{')?;
    let value: serde_json::Value = serde_json::from_str(&error[start..]).ok()?;
    let message = value.get("error")?.as_str()?.trim();
    (!message.is_empty()).then(|| message.to_owned())
}

/// Textes anglais des échecs, pour les essais (qui tournent en anglais) :
/// ceux de l'officiel et de ChatonLM, inchangés.
#[cfg(test)]
pub mod english {
    pub const OFFLINE: &str = "Unable to download model. Please check your internet connection to download the model for offline use.";
    pub const SHARDED: &str = "This version is split into several files, which Ollama can't download from Hugging Face yet. Choose another version.";
    pub const UNAVAILABLE_VERSION: &str =
        "This version isn't available on Hugging Face. Choose another version.";
    pub const NOT_GGUF: &str = "This repository has no model file Ollama can use.";
    pub const NOT_FOUND_OR_PRIVATE: &str =
        "This model doesn't exist on Hugging Face, is private, or needs an account to download.";
    pub const UNREACHABLE: &str = "Can't reach Ollama. Make sure it's running, then try again.";
    pub const TIMED_OUT: &str = "Hugging Face didn't answer in time. Try again.";
    pub const CONNECTION_LOST: &str = "Connection lost";
    pub const LOCATION_CHANGING: &str =
        "the model location is being changed: try again once it's done";
    pub const SERVER_CHANGED: &str =
        "Stopped: ChatonLM now uses another Ollama server. Download it again to use it there.";
}

#[cfg(test)]
mod tests {
    use super::english::*;
    use super::*;

    /// Textes exacts reçus d'Ollama 0.35.1 et du registre de Hugging Face
    /// (34 § 2-3, 35 § 2.4, 35-samples/manifest-*.json).
    #[test]
    fn hugging_face_errors_read_well() {
        let sharded_tag = r#"pull model manifest: 400: {"error":"This tag is a sharded GGUF. Ollama does not yet support pulling sharded GGUF via the registry; please use a single-file quantization tag or \"latest\", or download the shards and merge them locally with `ollama create` (workaround detailed at https://github.com/ollama/ollama/issues/5245)."}"#;
        let sharded_repo = r#"pull model manifest: 400: {"error":"This repository only contains sharded GGUF files. Ollama does not yet support pulling sharded GGUF via the registry; please download the shards and merge them locally with `ollama create` (workaround detailed at https://github.com/ollama/ollama/issues/5245), or use a repository with single-file quantizations."}"#;
        let invalid = r#"pull model manifest: 400: {"error":"The specified tag is not a valid quantization scheme. Please use another tag or \"latest\""}"#;
        let absent = r#"pull model manifest: 400: {"error":"The specified tag is not available in the repository. Please use another tag or \"latest\""}"#;
        let not_gguf = r#"pull model manifest: 400: {"error":"Repository is not GGUF or is not compatible with llama.cpp"}"#;
        let realm = r#"pull model manifest: realm host "huggingface.co" does not match original host "hf.co""#;
        let unknown = r#"pull model manifest: 401: {"error":"Invalid username or password."}"#;
        let gated = "Access to model google/gemma-2b is restricted. You must have access to it and be authenticated to access it. Please log in.";
        for (error, shown) in [
            (sharded_tag, SHARDED),
            (sharded_repo, SHARDED),
            (invalid, UNAVAILABLE_VERSION),
            (absent, UNAVAILABLE_VERSION),
            (not_gguf, NOT_GGUF),
            (realm, NOT_FOUND_OR_PRIVATE),
            (unknown, NOT_FOUND_OR_PRIVATE),
            (gated, NOT_FOUND_OR_PRIVATE),
            // Messages seuls (corps d'un manifeste refusé).
            (
                "The specified tag is not available in the repository. Please use another tag or \"latest\"",
                UNAVAILABLE_VERSION,
            ),
            ("Invalid username or password.", NOT_FOUND_OR_PRIVATE),
        ] {
            assert_eq!(pull_error(error), shown, "{error}");
        }
    }

    #[test]
    fn the_offline_case_is_kept() {
        let offline = "pull model manifest: Get \"https://hf.co/v2/a/b/manifests/Q4_K_M\": dial tcp: lookup hf.co: no such host";
        assert_eq!(pull_error(offline), OFFLINE);
        // Réseau pendant une couche : brut, comme l'officiel.
        let layer = "max retries exceeded: dial tcp: lookup example.com: no such host";
        assert_eq!(pull_error(layer), layer);
    }

    #[test]
    fn other_errors_are_shown_with_the_nested_message_extracted() {
        // JSON imbriqué : son message seul.
        let nested = r#"pull model manifest: 412: {"error":"this model requires a newer version of Ollama"}"#;
        assert_eq!(
            pull_error(nested),
            "this model requires a newer version of Ollama"
        );
        // Sans JSON (ou JSON sans message) : tel quel.
        for raw in [
            "pull model manifest: file does not exist",
            "pull model manifest: 412: this model requires a newer version of Ollama",
            r#"pull model manifest: 500: {"error":""}"#,
            r#"pull model manifest: 500: {"message":"x"}"#,
            "pull model manifest: 500: {not json",
        ] {
            assert_eq!(pull_error(raw), raw);
        }
    }

    #[test]
    fn texts_follow_the_language() {
        use crate::i18n::{Lang, text};
        assert_eq!(
            text(Lang::Fr, "downloads.unreachable", None, &[]),
            "Impossible de joindre Ollama. Vérifiez qu'il est lancé, puis réessayez."
        );
        assert_eq!(
            connection_lost(Some(&"reset by peer")),
            "Connection lost: reset by peer"
        );
        assert_eq!(connection_lost(None), "Connection lost");
    }
}
