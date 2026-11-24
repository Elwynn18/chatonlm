//! Outils offerts au modèle par [`super::Worker::run_tools`] : définition au
//! format d'OpenAI (comme `chat::tools::definitions`), appel asynchrone
//! annulable. Les outils réels (recherche, lecture de page, image…) sont
//! branchés par l'Atelier au lot 2.

use futures_util::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// Longueur maximale d'un résultat d'outil rendu au modèle (caractères) :
/// celle du texte d'une page lue par `web_fetch` (`chat/tools/page.rs`).
pub(crate) const MAX_TOOL_RESULT_CHARS: usize = 40_000;

/// Appels exécutés au plus par tour : au-delà, « trop d'appels » est rendu
/// au modèle (sans exécuter l'outil), qui peut les refaire au tour suivant.
pub(crate) const MAX_TOOL_CALLS: usize = 8;

/// Marque d'un résultat coupé.
const CUT: &str = "\n[…]";

// i18n-ignore : textes au modèle (anglais, comme ceux du chat).
/// Marque d'un résultat coupé faute de place dans le contexte du modèle.
const CUT_FOR_CONTEXT: &str = "\n[… cut: the context of the model is nearly full]";
// i18n-ignore : texte au modèle.
/// Résultat écarté faute de place (moins de [`MIN_ROOM`] caractères).
pub(super) const LEFT_OUT: &str = "[result left out: the context of the model is full]";
/// Place en dessous de laquelle un résultat n'est plus coupé mais écarté.
const MIN_ROOM: usize = 200;

/// Erreur rendue pour un appel au-delà de [`MAX_TOOL_CALLS`].
pub(super) fn too_many_calls() -> String {
    // i18n-ignore : texte au modèle.
    format!(
        "too many tool calls in one turn (at most {MAX_TOOL_CALLS}): this call was not run; \
         make it again in the next turn if it is still needed"
    )
}

/// Outil appelable par le modèle.
pub(crate) trait Tool: Send + Sync {
    /// Nom (celui de la définition).
    fn name(&self) -> &str;

    /// Définition au format d'OpenAI : `{"type": "function", "function":
    /// {name, description, parameters}}`.
    fn definition(&self) -> Value;

    /// Exécute l'appel ; `Err` : texte d'erreur rendu au modèle (le travail
    /// continue). `cancel` : arrêt du travail (l'outil lâche sa requête).
    fn call(&self, args: Value, cancel: CancellationToken)
    -> BoxFuture<'_, Result<String, String>>;
}

/// Résultat rendu au modèle : texte de l'outil, ou « Error: … » (forme du
/// chat), borné à [`MAX_TOOL_RESULT_CHARS`] caractères, et à `room`
/// caractères (place laissée dans le contexte du modèle par
/// `Worker::run_tools`) : coupé avec une mention qui le dit au modèle, ou
/// écarté ([`LEFT_OUT`]) s'il ne reste presque plus de place ; un texte de
/// moins de [`MIN_ROOM`] caractères passe toujours.
pub(super) fn tool_message(result: Result<String, String>, room: usize) -> String {
    let text = match result {
        Ok(text) => text,
        Err(error) => format!("Error: {error}"),
    };
    // Un texte court (erreur, « trop d'appels ») passe toujours en entier.
    if room >= MAX_TOOL_RESULT_CHARS || text.chars().count() <= room.max(MIN_ROOM) {
        return bounded(text, MAX_TOOL_RESULT_CHARS, CUT);
    }
    if room < MIN_ROOM {
        return LEFT_OUT.to_owned();
    }
    bounded(text, room, CUT_FOR_CONTEXT)
}

/// `text` coupé à `max` caractères au plus, marque `cut` comprise.
fn bounded(text: String, max: usize, cut: &str) -> String {
    if text.chars().count() <= max {
        return text;
    }
    let keep = max.saturating_sub(cut.chars().count());
    let end = text
        .char_indices()
        .nth(keep)
        .map_or(text.len(), |(index, _)| index);
    let mut out = text[..end].to_owned();
    out.push_str(cut);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_results_are_bounded_and_errors_say_so() {
        let all = usize::MAX;
        assert_eq!(tool_message(Ok("14°C".into()), all), "14°C");
        assert_eq!(
            tool_message(Err("no network".into()), all),
            "Error: no network"
        );
        let long = "é".repeat(MAX_TOOL_RESULT_CHARS + 10);
        let cut = tool_message(Ok(long), all);
        assert_eq!(cut.chars().count(), MAX_TOOL_RESULT_CHARS);
        assert!(cut.ends_with(CUT) && cut.starts_with("éé"));
        let exact = "a".repeat(MAX_TOOL_RESULT_CHARS);
        assert_eq!(tool_message(Ok(exact.clone()), all), exact);
    }

    /// Place du contexte : coupé à elle, avec une mention au modèle ;
    /// presque plus de place : écarté ; assez de place : tel quel.
    #[test]
    fn tool_results_fit_the_room_left_in_the_context() {
        let long = "x".repeat(5_000);
        let cut = tool_message(Ok(long.clone()), 1_000);
        assert_eq!(cut.chars().count(), 1_000);
        assert!(cut.starts_with("xxx") && cut.ends_with(CUT_FOR_CONTEXT));
        assert_eq!(tool_message(Ok(long.clone()), 5_000), long);
        assert_eq!(tool_message(Ok(long), MIN_ROOM - 1), LEFT_OUT);
        assert_eq!(tool_message(Ok("short".into()), 0), "short");
        assert!(too_many_calls().contains("at most 8"));
    }
}
