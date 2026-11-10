//! Aides des appels au moteur (lot L5a) : arrêt d'un `watch` relié à un
//! [`CancellationToken`], réflexion demandée par la page traduite pour
//! `llama-server` et filtrée d'après les détails du modèle (aussi pour
//! l'Étude, le nettoyage de la dictée et la visite), message système en
//! tête, et fin d'un envoi d'après une [`LlmError`]. L'historique en
//! messages est dans `stream.rs`.

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::stream::Think;
use crate::inference::{self, Effort, Message};
use crate::llm::LlmError;
use crate::llm::models::ModelDetails;

/// Jeton annulé dès que l'arrêt est demandé sur un `watch` (relais tenu par
/// une petite tâche, arrêtée avec lui).
pub(crate) struct Linked {
    token: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Linked {
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }
}

impl Drop for Linked {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Jeton relié à `cancel` : annulé aussitôt si l'arrêt est déjà demandé,
/// jamais si l'émetteur disparaît sans l'avoir demandé.
pub(crate) fn linked(cancel: &watch::Receiver<bool>) -> Linked {
    let token = CancellationToken::new();
    let mut receiver = cancel.clone();
    let relay = token.clone();
    let task = tokio::spawn(async move {
        if receiver.wait_for(|cancelled| *cancelled).await.is_ok() {
            relay.cancel();
        }
    });
    if *cancel.borrow() {
        token.cancel();
    }
    Linked { token, task }
}

/// Réflexion demandée par la page (forme d'Ollama, `Think`) traduite pour
/// `llama-server` : rien de demandé → choix du gabarit ; marche/arrêt →
/// `enable_thinking` ; niveau `low`/`medium`/`high` → `reasoning_effort` ;
/// autre niveau (inconnu) : marche.
pub(crate) fn think_request(think: Option<&Think>) -> inference::Think {
    match think {
        None => inference::Think::Default,
        Some(Think::Enabled(true)) => inference::Think::On,
        Some(Think::Enabled(false)) => inference::Think::Off,
        Some(Think::Level(level)) => match level.to_ascii_lowercase().as_str() {
            "low" => inference::Think::Effort(Effort::Low),
            "medium" => inference::Think::Effort(Effort::Medium),
            "high" => inference::Think::Effort(Effort::High),
            _ => inference::Think::On,
        },
    }
}

/// `think` envoyé d'après les détails du moteur (façade
/// [`crate::llm::models::details`]) : absent s'il n'est pas demandé ;
/// retiré si le modèle ne réfléchit pas (ni « thinking » dans ses
/// capacités, ni réglages de réflexion), ou si ses valeurs acceptées sont
/// connues et ne le contiennent pas ; gardé sinon.
pub(crate) fn accepted_think(think: Option<Think>, details: &ModelDetails) -> Option<Think> {
    let think = think?;
    let thinks = details.capabilities.iter().any(|word| word == "thinking");
    match &details.thinking {
        Some(options) if options.values.contains(&think) => Some(think),
        Some(_) => None,
        None => thinks.then_some(think),
    }
}

/// Images acceptées d'après les détails du moteur (capacité « vision »).
pub(crate) fn sees(details: &ModelDetails) -> bool {
    details.capabilities.iter().any(|word| word == "vision")
}

/// Outils acceptés d'après les détails du moteur (capacité « tools »).
pub(crate) fn uses_tools(details: &ModelDetails) -> bool {
    details.capabilities.iter().any(|word| word == "tools")
}

/// Place `system` en premier message (pré-prompt, consigne vocale, résumé).
pub(crate) fn with_system(mut messages: Vec<Message>, system: Option<&str>) -> Vec<Message> {
    if let Some(system) = system {
        messages.insert(0, Message::system(system));
    }
    messages
}

/// Fin d'une erreur du moteur pour la page : type et texte (langue du
/// moment) ; `None` : un arrêt (annulation, modèle déchargé à la demande).
pub(crate) fn failure(error: &LlmError) -> Option<(super::ChatErrorKind, String)> {
    error.chat_kind().map(|kind| (kind, error.message()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn think_is_translated_for_llama_server() {
        assert_eq!(think_request(None), inference::Think::Default);
        assert_eq!(
            think_request(Some(&Think::Enabled(true))),
            inference::Think::On
        );
        assert_eq!(
            think_request(Some(&Think::Enabled(false))),
            inference::Think::Off
        );
        assert_eq!(
            think_request(Some(&Think::Level("High".into()))),
            inference::Think::Effort(Effort::High)
        );
        assert_eq!(
            think_request(Some(&Think::Level("low".into()))),
            inference::Think::Effort(Effort::Low)
        );
        assert_eq!(
            think_request(Some(&Think::Level("medium".into()))),
            inference::Think::Effort(Effort::Medium)
        );
        assert_eq!(
            think_request(Some(&Think::Level("max".into()))),
            inference::Think::On
        );
    }

    fn details(capabilities: &[&str], thinking: Option<Vec<Think>>) -> ModelDetails {
        ModelDetails {
            model: "a/b-GGUF:Q4_K_M".into(),
            capabilities: capabilities.iter().map(|word| (*word).to_owned()).collect(),
            thinking: thinking.map(|values| crate::llm::models::ThinkingOptions {
                values,
                default: None,
            }),
            always_thinks: false,
        }
    }

    #[test]
    fn think_is_kept_only_if_the_model_accepts_it() {
        let on = Some(Think::Enabled(true));
        let off = Some(Think::Enabled(false));
        let high = Some(Think::Level("high".into()));
        let plain = details(&["completion"], None);
        assert_eq!(accepted_think(on.clone(), &plain), None);
        assert_eq!(accepted_think(None, &plain), None);
        let thinks = details(
            &["completion", "thinking"],
            Some(vec![Think::Enabled(true), Think::Enabled(false)]),
        );
        assert_eq!(accepted_think(off.clone(), &thinks), off);
        assert_eq!(accepted_think(high.clone(), &thinks), None);
        // Capacité sans réglages connus : gardé.
        let bare = details(&["completion", "thinking"], None);
        assert_eq!(accepted_think(high.clone(), &bare), high);
        // gpt-oss : niveaux seulement.
        let levels = details(
            &["completion", "thinking"],
            Some(vec![
                Think::Level("low".into()),
                Think::Level("high".into()),
            ]),
        );
        assert_eq!(accepted_think(high.clone(), &levels), high);
        assert_eq!(accepted_think(on, &levels), None);
        assert!(sees(&details(&["completion", "vision"], None)));
        assert!(!sees(&plain));
        assert!(uses_tools(&details(&["completion", "tools"], None)));
        assert!(!uses_tools(&plain));
    }

    #[test]
    fn the_system_message_comes_first() {
        let turns = [Message::user("Hi")];
        let first = with_system(turns.to_vec(), Some("Be brief."));
        assert_eq!(
            first,
            vec![Message::system("Be brief."), Message::user("Hi")]
        );
        assert_eq!(with_system(turns.to_vec(), None), turns.to_vec());
    }

    #[tokio::test]
    async fn a_linked_token_follows_the_stop() {
        let (sender, receiver) = watch::channel(false);
        let linked = linked(&receiver);
        assert!(!linked.token().is_cancelled());
        sender.send(true).unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            linked.token().cancelled(),
        )
        .await
        .expect("cancelled");
        // Déjà demandé : annulé tout de suite.
        assert!(super::linked(&receiver).token().is_cancelled());
        // Émetteur disparu sans arrêt : jamais annulé.
        let (sender, receiver) = watch::channel(false);
        let linked = super::linked(&receiver);
        drop(sender);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!linked.token().is_cancelled());
    }
}
