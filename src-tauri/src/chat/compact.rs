//! Contexte occupé et compactage d'une conversation (propre à ChatonLM ;
//! contrat `.tools/research/81-context-contract.md`).
//!
//! - contexte occupé : ce que la conversation prendra dans la requête
//!   suivante. Compté après une réponse finie (`prompt_eval_count`, le prompt
//!   entier même en cache, plus ce que le modèle relira de la réponse : son
//!   texte et ses appels, pas son raisonnement, que les gabarits retirent ;
//!   vérifié sur Ollama 0.40), puis estimé pour les messages venus après
//!   (réponse arrêtée, nouveau message, conversation d'avant cette version) ;
//!   sur la taille du contexte du modèle de l'envoi (réglage borné par le
//!   modèle, sinon `/api/ps` ; inconnue pour un modèle du cloud) ;
//! - compacter : le modèle résume le début de ce qu'une requête enverrait
//!   (résumé précédent compris), autant qu'il en tient dans le contexte avec
//!   la place du résumé ; une requête suivante n'envoie que ce résumé, en
//!   message système, et les messages qui le suivent. Les messages restent
//!   en base et affichés ;
//! - automatiquement avant un envoi, quand la conversation a passé 80 % du
//!   contexte ; la réponse d'un envoi est limitée (`num_predict`) à ce qui
//!   reste du contexte, pour ne pas laisser Ollama décaler son contexte et
//!   perdre le début sans rien dire.

use serde_json::json;
use tokio::sync::watch;

use super::stream::{self, Line, LineBuffer, Think};
use super::{ChatErrorKind, Opened, Outcome, Server, cancelled, open, with_system_message};
use crate::hub::errors::connection_lost;
use crate::i18n::t;
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
/// d'Ollama dans la conversation, pour limiter une réponse : 4 caractères par
/// jeton, le cas courant (une limite trop basse couperait à tort une
/// réponse ; trop haute, Ollama décale son contexte comme sans limite).
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
/// (s'il est encore valable : pas avant le dernier compactage), sinon le
/// résumé ; plus l'estimation, corrigée par `scale`, des messages suivants
/// de `sendable`, sauf les `skip_last` derniers.
pub(super) fn occupied(
    counted: Option<Counted>,
    sendable: &Sendable,
    skip_last: usize,
    scale: f64,
) -> i64 {
    let (base, after) = match counted {
        Some(counted) if counted.through >= sendable.through => {
            (counted.usage.used, counted.through)
        }
        _ => (
            scaled(sendable.summary.as_deref().map_or(0, summary_tokens), scale),
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
/// `sendable` : le résumé et les messages non résumés (estimations corrigées
/// par `scale`), compté jusqu'au dernier message de `sendable`.
pub(super) fn after_compaction(
    sendable: &Sendable,
    summary: &str,
    count: usize,
    scale: f64,
    size: i64,
) -> Counted {
    let rest: i64 = sendable.turns[count..].iter().map(turn_tokens).sum();
    Counted {
        usage: ContextUsage {
            used: scaled(summary_tokens(summary) + rest, scale),
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

/// Limite de la réponse (`num_predict`) : ce qui reste du contexte après ce
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

/// Rapport entre les jetons comptés par Ollama et l'estimation des mêmes
/// messages (le tokenizer du modèle, la langue, le code), borné entre 0,3 et
/// 3 ; `None` sans compte valable.
pub(super) fn calibration(counted: Option<Counted>, sendable: &Sendable) -> Option<f64> {
    let counted = counted.filter(|counted| counted.through >= sendable.through)?;
    let estimated: i64 = sendable.summary.as_deref().map_or(0, summary_tokens)
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

/// Demande au modèle le résumé du début de `sendable` qui tient dans le
/// contexte (raisonnement `think`, coupé si possible ; retiré si le modèle
/// le refuse) : le résumé et le nombre de messages qu'il couvre (au moins
/// un ; `sendable` n'est pas vide). Erreur : fin décidée.
#[allow(clippy::too_many_arguments)]
pub(super) async fn summarize(
    server: &Server,
    model: &str,
    sendable: &Sendable,
    mut think: Option<Think>,
    images: bool,
    size: Option<i64>,
    scale: f64,
    cancel: &mut watch::Receiver<bool>,
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
    loop {
        let mut body = stream::request_body(
            model,
            &turns,
            think.as_ref(),
            server.context_length,
            server.keep_alive,
            images,
            None,
        );
        if let Some(summary) = &sendable.summary {
            with_system_message(&mut body, &summary_message(summary));
        }
        body["options"]["num_predict"] = json!(summary_budget(size));
        match open(&server.url, &body, cancel).await {
            Opened::Stream(response) => {
                return read_summary(response, cancel)
                    .await
                    .map(|summary| (summary, count));
            }
            Opened::ThinkingRefused(_) if think.is_some() => think = None,
            Opened::ThinkingRefused(message) => {
                return Err(Outcome::Failed(ChatErrorKind::Model, message));
            }
            Opened::Ended(outcome) => return Err(outcome),
        }
    }
}

/// Lit le résumé (texte seulement, le raisonnement est ignoré) jusqu'à
/// `done`.
async fn read_summary(
    mut response: reqwest::Response,
    cancel: &mut watch::Receiver<bool>,
) -> Result<String, Outcome> {
    let mut buffer = LineBuffer::default();
    let mut summary = String::new();
    let lost = || Outcome::Failed(ChatErrorKind::ConnectionLost, connection_lost(None));
    loop {
        let chunk = tokio::select! {
            biased;
            () = cancelled(cancel) => return Err(Outcome::Cancelled),
            chunk = response.chunk() => chunk,
        };
        let lines = match chunk {
            Ok(Some(bytes)) => buffer.push(&bytes),
            Ok(None) => return Err(lost()),
            Err(error) => {
                return Err(Outcome::Failed(
                    ChatErrorKind::ConnectionLost,
                    connection_lost(Some(&error.without_url())),
                ));
            }
        };
        for line in lines {
            match stream::parse_line(&line) {
                Ok(Line::Fragment { content, done, .. }) => {
                    summary.push_str(&content);
                    if done {
                        let summary = summary.trim();
                        if summary.is_empty() {
                            return Err(Outcome::Failed(
                                ChatErrorKind::Model,
                                t!("chat.compactEmpty"),
                            ));
                        }
                        return Ok(summary.to_owned());
                    }
                }
                Ok(Line::Error(message)) | Err(message) => {
                    return Err(Outcome::Failed(ChatErrorKind::Model, message));
                }
            }
        }
    }
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
        assert_eq!(occupied(counted(500, 2), &chat, 1, 1.0), 500 + 116);
        assert_eq!(occupied(counted(500, 2), &chat, 1, 0.5), 500 + 58);
        // Rien de compté (modification, ancienne conversation) : tout estimé.
        assert_eq!(occupied(None, &chat, 1, 1.0), 3 * 116);
        assert_eq!(occupied(None, &chat, 0, 1.0), 4 * 116);
        // Compte d'avant le dernier compactage : le résumé, puis la suite.
        let compacted = Sendable {
            summary: Some("s".repeat(90)),
            through: 2,
            turns: chat.turns[2..].to_vec(),
            ids: vec![3, 4],
        };
        let summary = summary_tokens(&"s".repeat(90));
        assert_eq!(
            occupied(counted(3000, 1), &compacted, 1, 1.0),
            summary + 116
        );
        assert_eq!(occupied(counted(300, 3), &compacted, 1, 1.0), 300);
    }

    #[test]
    fn after_a_partial_compaction_the_rest_is_counted() {
        let chat = sendable(4);
        let summary = "s".repeat(90);
        let all = after_compaction(&chat, &summary, 4, 1.0, 4096);
        assert_eq!(all.usage.used, summary_tokens(&summary));
        assert_eq!(all.through, 4);
        // Deux messages non résumés (116 jetons chacun), corrigés de moitié.
        let part = after_compaction(&chat, &summary, 2, 0.5, 4096);
        assert_eq!(part.usage.used, scaled(summary_tokens(&summary) + 232, 0.5));
        assert_eq!(part.through, 4);
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
        // Messages 1 et 2 estimés à 232 jetons, comptés 116 par Ollama.
        assert_eq!(calibration(counted(116, 2), &chat), Some(0.5));
        assert_eq!(calibration(counted(10_000, 2), &chat), Some(3.0));
        assert_eq!(calibration(None, &chat), None);
        assert_eq!(scaled(116, 0.75), 87);
    }
}
