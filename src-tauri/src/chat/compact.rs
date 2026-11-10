//! Contexte occupé et compactage d'une conversation (propre à ChatonLM ;
//! contrat `.tools/research/81-context-contract.md`).
//!
//! - contexte occupé : ce que la conversation prendra dans la requête
//!   suivante. Compté après une réponse finie (`cache_n + prompt_n` des
//!   temps de `llama-server`, le prompt entier, cache compris, plus ce que le
//!   modèle relira de la réponse : son texte et ses appels, pas son
//!   raisonnement, que les gabarits retirent), puis estimé pour les messages
//!   venus après (réponse arrêtée, nouveau message, conversation d'avant
//!   cette version), estimation corrigée d'après le dernier compte ; sur la
//!   taille du contexte du processus de l'envoi (`-c`, [`Session::context`] ;
//!   inconnue pour un modèle distant qui ne l'annonce pas) ;
//! - compacter : le modèle résume le début de ce qu'une requête enverrait
//!   (résumé précédent compris), autant qu'il en tient dans le contexte avec
//!   la place du résumé ; une requête suivante n'envoie que ce résumé, en
//!   message système, et les messages qui le suivent. Les messages restent
//!   en base et affichés ;
//! - automatiquement avant un envoi, quand la conversation a passé 80 % du
//!   contexte ; la réponse d'un envoi est limitée (`max_tokens`) à ce qui
//!   reste du contexte, pour finir en `contextFull` plutôt que sur un refus
//!   du serveur au tour suivant (`llama-server` ne décale pas le contexte) ;
//! - le message système fixe d'un envoi (pré-prompt, consigne vocale), que
//!   le compte du moteur comprend, entre dans chaque estimation ; le compactage
//!   ne le réduit pas : pas de compactage automatique quand, avec la place du
//!   résumé, il tient déjà 80 % du contexte (il recommencerait à chaque envoi).

use tokio_util::sync::CancellationToken;

use super::stream::{self, Think};
use super::{ChatErrorKind, Outcome, call};
use crate::i18n::t;
use crate::inference;
use crate::llm::Session;
use crate::store::chats::{ContextUsage, Counted, Reply, Role, Sendable, Turn, TurnFile};

/// Consigne du résumé, pour le modèle (pas un texte de l'interface).
const INSTRUCTION: &str = "Summarize our conversation so far so that it can \
continue without the earlier messages. Keep the user's goals and preferences, \
the facts, names, numbers, decisions, code and commands that still matter, \
and any open question. Write it in the language of the conversation, as \
plain notes, without any introduction. Reply with the summary only.";

/// Jetons au plus pour le résumé (un huitième du contexte au plus).
const SUMMARY_TOKENS: i64 = 4_096;

/// Réponse jamais limitée sous ce nombre de jetons (estimation du prompt
/// trop haute, ou conversation déjà pleine) ; aussi le plus petit résumé.
const MIN_REPLY: i64 = 256;

/// Jetons estimés pour une image jointe.
const IMAGE_TOKENS: i64 = 1_000;

/// Jetons estimés pour le gabarit d'un message (rôle, balises).
const MESSAGE_TOKENS: i64 = 16;

/// Correction de l'estimation (3 caractères par jeton) avant tout compte
/// du moteur dans la conversation, pour limiter une réponse : 4 caractères
/// par jeton, le cas courant (une limite trop basse couperait à tort une
/// réponse ; trop haute, le contexte se remplirait avant la fin).
pub(super) const UNCALIBRATED: f64 = 0.75;

/// `tokens` corrigés par `scale` (arrondis par excès).
pub(super) fn scaled(tokens: i64, scale: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let scaled = (tokens as f64 * scale).ceil() as i64;
    scaled
}

/// La conversation a passé 80 % de son contexte : à compacter avant l'envoi.
pub(super) fn needs_compaction(usage: Option<ContextUsage>) -> bool {
    usage.is_some_and(|usage| usage.size > 0 && usage.used * 5 >= usage.size * 4)
}

/// Message système qui porte le résumé du dernier compactage.
pub(super) fn summary_message(summary: &str) -> String {
    format!(
        "Summary of the earlier part of this conversation (older messages \
         were compacted to save space):\n\n{summary}"
    )
}

/// Jetons estimés d'un texte (environ 3 caractères par jeton, par excès).
fn tokens(text: &str) -> i64 {
    i64::try_from(text.chars().count() / 3).unwrap_or(i64::MAX)
}

/// Jetons estimés du résumé envoyé, message système compris.
pub(super) fn summary_tokens(summary: &str) -> i64 {
    tokens(&summary_message(summary)) + MESSAGE_TOKENS
}

/// Jetons estimés du message système fixe d'un envoi (pré-prompt, consigne
/// vocale), gabarit compris ; 0 sans message.
pub(super) fn system_tokens(system: Option<&str>) -> i64 {
    system.map_or(0, |system| tokens(system) + MESSAGE_TOKENS)
}

/// Un compactage automatique peut-il ramener la conversation sous 80 % de
/// `size` ? Non si le message système fixe (`system`, estimé et corrigé) et
/// la place du résumé y suffisent déjà.
pub(super) fn compaction_relieves(size: i64, system: i64) -> bool {
    (system + summary_budget(Some(size))) * 5 < size * 4
}

/// Jetons estimés de ce qu'un message ajoute à la requête suivante : son
/// texte, ses fichiers, ses appels d'outils et leurs résultats (pas son
/// raisonnement).
pub(super) fn turn_tokens(turn: &Turn) -> i64 {
    let files: i64 = turn
        .files
        .iter()
        .map(|file| match file {
            TurnFile::Image(_) => IMAGE_TOKENS,
            TurnFile::Text { text, .. } => tokens(text),
        })
        .sum();
    let calls: i64 = turn
        .tool_calls
        .iter()
        .map(|call| {
            let answer = call.result.as_deref().or(call.error.as_deref());
            tokens(&call.arguments) + tokens(answer.unwrap_or("")) + MESSAGE_TOKENS
        })
        .sum();
    tokens(&turn.content) + files + calls + MESSAGE_TOKENS
}

/// Contexte occupé avant une requête : compté jusqu'à `counted.through`
/// (s'il est encore valable : pas avant le dernier compactage ; message
/// système compris), sinon le résumé et le message système fixe (`system`,
/// estimé : [`system_tokens`]) ; plus l'estimation, corrigée par `scale`,
/// des messages suivants de `sendable`, sauf les `skip_last` derniers.
pub(super) fn occupied(
    counted: Option<Counted>,
    sendable: &Sendable,
    skip_last: usize,
    scale: f64,
    system: i64,
) -> i64 {
    let (base, after) = match counted {
        Some(counted) if counted.through >= sendable.through => {
            (counted.usage.used, counted.through)
        }
        _ => (
            scaled(
                sendable.summary.as_deref().map_or(0, summary_tokens) + system,
                scale,
            ),
            sendable.through,
        ),
    };
    let kept = sendable.turns.len().saturating_sub(skip_last);
    let added: i64 = sendable.turns[..kept]
        .iter()
        .zip(&sendable.ids)
        .filter(|(_, id)| **id > after)
        .map(|(turn, _)| turn_tokens(turn))
        .sum();
    base + scaled(added, scale)
}

/// Contexte estimé après un compactage des `count` premiers messages de
/// `sendable` : le message système fixe (`system`, estimé), le résumé et les
/// messages non résumés (estimations corrigées par `scale`), compté jusqu'au
/// dernier message de `sendable`.
pub(super) fn after_compaction(
    sendable: &Sendable,
    summary: &str,
    count: usize,
    scale: f64,
    size: i64,
    system: i64,
) -> Counted {
    let rest: i64 = sendable.turns[count..].iter().map(turn_tokens).sum();
    Counted {
        usage: ContextUsage {
            used: scaled(system + summary_tokens(summary) + rest, scale),
            size,
        },
        through: sendable.ids.last().copied().unwrap_or(sendable.through),
    }
}

/// Contexte occupé après une réponse finie : son prompt, et ce que le modèle
/// relira d'elle (tout, sans raisonnement ; sinon son texte et ses appels,
/// estimés, au plus ses jetons générés).
pub(super) fn occupied_after(prompt: u64, eval: u64, reply: &Reply) -> i64 {
    let prompt = i64::try_from(prompt).unwrap_or(i64::MAX);
    let eval = i64::try_from(eval).unwrap_or(i64::MAX);
    if reply.thinking.is_empty() {
        return prompt.saturating_add(eval);
    }
    let calls: i64 = reply
        .tool_calls
        .iter()
        .map(|call| tokens(&call.arguments))
        .sum();
    prompt.saturating_add(eval.min(tokens(&reply.content) + calls + MESSAGE_TOKENS))
}

/// Limite de la réponse (`max_tokens`) : ce qui reste du contexte après ce
/// qu'occupe la requête (`occupied`, estimé).
pub(super) fn reply_limit(size: i64, occupied: i64) -> i64 {
    (size - occupied).max(MIN_REPLY)
}

/// Jetons du résumé : un huitième du contexte, entre [`MIN_REPLY`] et
/// [`SUMMARY_TOKENS`].
fn summary_budget(size: Option<i64>) -> i64 {
    size.map_or(SUMMARY_TOKENS, |size| {
        (size / 8).clamp(MIN_REPLY, SUMMARY_TOKENS)
    })
}

/// Rapport entre les jetons comptés par le moteur et l'estimation des mêmes
/// messages, message système fixe (`system`, estimé) compris (le tokenizer
/// du modèle, la langue, le code), borné entre 0,3 et 3 ; `None` sans compte
/// valable.
pub(super) fn calibration(
    counted: Option<Counted>,
    sendable: &Sendable,
    system: i64,
) -> Option<f64> {
    let counted = counted.filter(|counted| counted.through >= sendable.through)?;
    let estimated: i64 = system
        + sendable.summary.as_deref().map_or(0, summary_tokens)
        + sendable
            .turns
            .iter()
            .zip(&sendable.ids)
            .filter(|(_, id)| **id <= counted.through)
            .map(|(turn, _)| turn_tokens(turn))
            .sum::<i64>();
    if estimated <= 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let ratio = counted.usage.used as f64 / estimated as f64;
    Some(ratio.clamp(0.3, 3.0))
}

/// Messages de `sendable` résumés : le plus long début qui tient dans le
/// contexte avec le résumé précédent, la consigne et la place du nouveau
/// résumé, estimations corrigées par `scale` ([`calibration`]) ; au moins
/// un message ; tous si la taille est inconnue.
fn covered(sendable: &Sendable, size: Option<i64>, scale: f64) -> usize {
    let Some(size) = size else {
        return sendable.turns.len();
    };
    let scaled = |tokens: i64| scaled(tokens, scale);
    let mut room = size
        - summary_budget(Some(size))
        - scaled(tokens(INSTRUCTION) + MESSAGE_TOKENS)
        - scaled(sendable.summary.as_deref().map_or(0, summary_tokens));
    let mut count = 0;
    for turn in &sendable.turns {
        room -= scaled(turn_tokens(turn));
        if room < 0 && count > 0 {
            break;
        }
        count += 1;
    }
    count
}

/// Demande au modèle de `session` le résumé du début de `sendable` qui
/// tient dans le contexte (raisonnement `think`, coupé si le modèle le
/// permet ; réflexion reçue à part, jamais dans le résumé) : le résumé et le
/// nombre de messages qu'il couvre (au moins un ; `sendable` n'est pas vide).
/// Réponse entière (sans flux) bornée à la place du résumé. Erreur : fin
/// décidée (arrêt : `Cancelled`).
pub(super) async fn summarize(
    session: &Session,
    sendable: &Sendable,
    think: Option<Think>,
    images: bool,
    size: Option<i64>,
    scale: f64,
    cancel: &CancellationToken,
) -> Result<(String, usize), Outcome> {
    let count = covered(sendable, size, scale);
    let mut turns = sendable.turns[..count].to_vec();
    turns.push(Turn {
        role: Role::User,
        content: INSTRUCTION.to_owned(),
        thinking: String::new(),
        files: Vec::new(),
        tool_calls: Vec::new(),
    });
    let summary = sendable.summary.as_deref().map(summary_message);
    let messages = call::with_system(stream::messages(&turns, images), summary.as_deref());
    let mut request = session.request(messages);
    request.think = call::think_request(think.as_ref());
    request.max_tokens = u32::try_from(summary_budget(size)).ok();
    let answered = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(Outcome::Cancelled),
        answered = inference::chat_once(&session.endpoint, &request, cancel.clone()) => answered,
    };
    let answer = answered.map_err(|error| super::failed(&session.error(error)))?;
    let summary = answer.content.trim();
    if summary.is_empty() {
        return Err(Outcome::Failed(
            ChatErrorKind::Model,
            t!("chat.compactEmpty"),
        ));
    }
    Ok((summary.to_owned(), count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::chats::ToolCall;

    fn turn(role: Role, content: &str) -> Turn {
        Turn {
            role,
            content: content.into(),
            thinking: String::new(),
            files: Vec::new(),
            tool_calls: Vec::new(),
        }
    }

    /// Conversation de `count` messages de 300 caractères (116 jetons
    /// chacun), d'identifiants 1 à `count`.
    fn sendable(count: i64) -> Sendable {
        Sendable {
            summary: None,
            through: 0,
            turns: (1..=count)
                .map(|_| turn(Role::User, &"a".repeat(300)))
                .collect(),
            ids: (1..=count).collect(),
        }
    }

    #[test]
    fn compaction_starts_at_eighty_percent() {
        let usage = |used, size| Some(ContextUsage { used, size });
        assert!(!needs_compaction(None));
        assert!(!needs_compaction(usage(799, 1000)));
        assert!(needs_compaction(usage(800, 1000)));
        assert!(needs_compaction(usage(1200, 1000)));
        assert!(!needs_compaction(usage(10, 0)));
    }

    #[test]
    fn the_reply_gets_what_is_left_of_the_context() {
        assert_eq!(reply_limit(32_768, 20_768), 12_000);
        // Déjà plein : jamais moins que le minimum.
        assert_eq!(reply_limit(32_768, 40_000), MIN_REPLY);
    }

    #[test]
    fn what_a_message_adds_is_estimated_by_excess() {
        let mut user = turn(Role::User, &"a".repeat(300));
        user.thinking = "not sent again by the templates".into();
        user.files = vec![
            TurnFile::Image(vec![0; 10]),
            TurnFile::Text {
                filename: "notes.txt".into(),
                text: "b".repeat(30),
            },
        ];
        assert_eq!(turn_tokens(&user), 100 + IMAGE_TOKENS + 10 + 16);
        let mut searched = turn(Role::Assistant, "");
        searched.tool_calls = vec![ToolCall {
            result: Some("r".repeat(60)),
            ..ToolCall::new("web_search", "q".repeat(30))
        }];
        assert_eq!(turn_tokens(&searched), 10 + 20 + 16 + 16);
    }

    #[test]
    fn what_follows_the_last_count_is_estimated() {
        let chat = sendable(4);
        let counted = |used, through| {
            Some(Counted {
                usage: ContextUsage { used, size: 4096 },
                through,
            })
        };
        // Compté jusqu'au message 2 ; le 3 (réponse arrêtée) estimé, le 4
        // (le nouveau message) laissé de côté.
        assert_eq!(occupied(counted(500, 2), &chat, 1, 1.0, 0), 500 + 116);
        assert_eq!(occupied(counted(500, 2), &chat, 1, 0.5, 0), 500 + 58);
        // Rien de compté (modification, ancienne conversation) : tout estimé.
        assert_eq!(occupied(None, &chat, 1, 1.0, 0), 3 * 116);
        assert_eq!(occupied(None, &chat, 0, 1.0, 0), 4 * 116);
        // Compte d'avant le dernier compactage : le résumé, puis la suite.
        let compacted = Sendable {
            summary: Some("s".repeat(90)),
            through: 2,
            turns: chat.turns[2..].to_vec(),
            ids: vec![3, 4],
        };
        let summary = summary_tokens(&"s".repeat(90));
        assert_eq!(
            occupied(counted(3000, 1), &compacted, 1, 1.0, 0),
            summary + 116
        );
        assert_eq!(occupied(counted(300, 3), &compacted, 1, 1.0, 0), 300);
        // Message système fixe : compris dans un compte, ajouté à une
        // estimation.
        assert_eq!(occupied(counted(500, 2), &chat, 1, 1.0, 400), 500 + 116);
        assert_eq!(
            occupied(None, &chat, 1, 0.5, 400),
            scaled(400 + 3 * 116, 0.5)
        );
        assert_eq!(
            occupied(counted(3000, 1), &compacted, 1, 1.0, 400),
            400 + summary + 116
        );
    }

    #[test]
    fn the_fixed_system_message_is_estimated_and_never_compacted() {
        assert_eq!(system_tokens(None), 0);
        assert_eq!(system_tokens(Some(&"p".repeat(300))), 100 + MESSAGE_TOKENS);
        // 4096 : 512 jetons pour le résumé ; sous 80 % (3276,8), il reste
        // 2764 jetons au plus pour le message système.
        assert!(compaction_relieves(4096, 0));
        assert!(compaction_relieves(4096, 2764));
        assert!(!compaction_relieves(4096, 2765));
        assert!(!compaction_relieves(4096, 4000));
    }

    #[test]
    fn after_a_partial_compaction_the_rest_is_counted() {
        let chat = sendable(4);
        let summary = "s".repeat(90);
        let all = after_compaction(&chat, &summary, 4, 1.0, 4096, 0);
        assert_eq!(all.usage.used, summary_tokens(&summary));
        assert_eq!(all.through, 4);
        // Deux messages non résumés (116 jetons chacun), corrigés de moitié.
        let part = after_compaction(&chat, &summary, 2, 0.5, 4096, 0);
        assert_eq!(part.usage.used, scaled(summary_tokens(&summary) + 232, 0.5));
        assert_eq!(part.through, 4);
        let with_system = after_compaction(&chat, &summary, 4, 0.5, 4096, 400);
        assert_eq!(
            with_system.usage.used,
            scaled(400 + summary_tokens(&summary), 0.5)
        );
    }

    #[test]
    fn a_thinking_the_model_does_not_read_again_is_not_counted() {
        let mut reply = Reply {
            content: "c".repeat(30),
            ..Reply::default()
        };
        assert_eq!(occupied_after(24, 651, &reply), 24 + 651);
        reply.thinking = "t".repeat(1746);
        assert_eq!(occupied_after(24, 651, &reply), 24 + 10 + 16);
        // Jamais plus que les jetons générés.
        assert_eq!(occupied_after(24, 5, &reply), 24 + 5);
    }

    #[test]
    fn the_summary_covers_what_fits_in_the_context() {
        let chat = sendable(10);
        assert_eq!(covered(&chat, None, 1.0), 10);
        assert_eq!(covered(&chat, Some(32_768), 1.0), 10);
        // 1024 : 256 pour le résumé, 129 pour la consigne : 5 messages ;
        // 10 si le modèle compte deux fois moins de jetons que l'estimation.
        assert_eq!(covered(&chat, Some(1024), 1.0), 5);
        assert_eq!(covered(&chat, Some(1024), 0.5), 10);
        // Même un seul message trop long : au moins celui-là.
        assert_eq!(covered(&chat, Some(100), 1.0), 1);
        assert_eq!(summary_budget(Some(1024)), 256);
        assert_eq!(summary_budget(Some(8192)), 1024);
        assert_eq!(summary_budget(Some(262_144)), SUMMARY_TOKENS);
    }

    #[test]
    fn the_estimate_is_calibrated_on_the_counted_tokens() {
        let chat = sendable(4);
        let counted = |used, through| {
            Some(Counted {
                usage: ContextUsage { used, size: 4096 },
                through,
            })
        };
        // Messages 1 et 2 estimés à 232 jetons, comptés 116 par le moteur.
        assert_eq!(calibration(counted(116, 2), &chat, 0), Some(0.5));
        assert_eq!(calibration(counted(10_000, 2), &chat, 0), Some(3.0));
        assert_eq!(calibration(None, &chat, 0), None);
        // Le compte du moteur comprend le message système : l'estimation aussi.
        assert_eq!(calibration(counted(232, 2), &chat, 232), Some(0.5));
        assert_eq!(scaled(116, 0.75), 87);
    }
}
