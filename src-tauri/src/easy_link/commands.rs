//! Commandes du bot, lues dans le texte d'un message : `/nom` (usage de
//! Telegram) ou `!nom` (usage de Discord), acceptées toutes deux partout.

/// Commande d'un compte lié.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    /// Nouvelle conversation.
    New,
    /// Arrête la réponse en cours.
    Stop,
    /// Liste des modèles installés.
    Models,
    /// Choisit le modèle (nom ou numéro de la liste) ; sans argument : le dit.
    Model(Option<String>),
    /// Charge le modèle nommé, sinon le modèle choisi.
    Load(Option<String>),
    /// Décharge le modèle nommé (`all` : tous), sinon le modèle choisi.
    Unload(Option<String>),
    Status,
    /// Recherche web ; sans argument : la dit. `Err` : argument incompris.
    Web(Result<Option<bool>, String>),
    /// Mode éphémère ; sans argument : le dit. `Err` : argument incompris.
    Ephemeral(Result<Option<bool>, String>),
    /// Garde la conversation éphémère en cours dans l'historique.
    Keep,
    /// Délie ce compte.
    Unlink,
    /// `/nom` inconnu.
    Unknown(String),
}

/// Commande d'un message, s'il en est une. Un « / » suivi d'un mot inconnu est
/// une commande inconnue (l'auteur voulait une commande) ; un « ! » suivi d'un
/// mot inconnu est du texte pour le modèle (« !important », « !! »).
pub fn parse(text: &str) -> Option<Command> {
    let text = text.trim();
    let (bang, rest) = match text.strip_prefix('/') {
        Some(rest) => (false, rest),
        None => (true, text.strip_prefix('!')?),
    };
    let (word, argument) = match rest.split_once(char::is_whitespace) {
        Some((word, argument)) => (word, argument.trim()),
        None => (rest, ""),
    };
    // Telegram : `/models@nom_du_bot`.
    let word = word.split_once('@').map_or(word, |(word, _)| word);
    if word.is_empty() || !word.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let argument = (!argument.is_empty()).then(|| argument.to_owned());
    let command = match word.to_ascii_lowercase().as_str() {
        "help" | "start" | "commands" => Command::Help,
        "new" | "reset" => Command::New,
        "stop" | "cancel" => Command::Stop,
        "models" => Command::Models,
        "model" => Command::Model(argument),
        "load" => Command::Load(argument),
        "unload" => Command::Unload(argument),
        "status" => Command::Status,
        "web" | "search" => Command::Web(switch(argument.as_deref())),
        "ephemeral" => Command::Ephemeral(switch(argument.as_deref())),
        "keep" => Command::Keep,
        "unlink" => Command::Unlink,
        _ if bang => return None,
        other => Command::Unknown(other.to_owned()),
    };
    Some(command)
}

/// Argument d'un réglage à deux états : absent, on, off ; `Err` : incompris.
fn switch(argument: Option<&str>) -> Result<Option<bool>, String> {
    match argument {
        None => Ok(None),
        Some(value) => match value.to_ascii_lowercase().as_str() {
            "on" | "1" | "yes" | "true" => Ok(Some(true)),
            "off" | "0" | "no" | "false" => Ok(Some(false)),
            _ => Err(value.to_owned()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_prefixes_give_the_same_commands() {
        for prefix in ["/", "!"] {
            let parse = |text: &str| parse(&format!("{prefix}{text}"));
            assert_eq!(parse("help"), Some(Command::Help));
            assert_eq!(parse("start"), Some(Command::Help));
            assert_eq!(parse("new"), Some(Command::New));
            assert_eq!(parse("STOP"), Some(Command::Stop));
            assert_eq!(parse("models"), Some(Command::Models));
            assert_eq!(parse("model"), Some(Command::Model(None)));
            assert_eq!(
                parse("model  gemma3:4b "),
                Some(Command::Model(Some("gemma3:4b".into())))
            );
            assert_eq!(parse("load"), Some(Command::Load(None)));
            assert_eq!(
                parse("unload all"),
                Some(Command::Unload(Some("all".into())))
            );
            assert_eq!(parse("status"), Some(Command::Status));
            assert_eq!(parse("unlink"), Some(Command::Unlink));
            assert_eq!(parse("keep"), Some(Command::Keep));
            assert_eq!(parse("ephemeral"), Some(Command::Ephemeral(Ok(None))));
        }
    }

    #[test]
    fn ephemeral_mode_reads_on_and_off() {
        assert_eq!(
            parse("/ephemeral on"),
            Some(Command::Ephemeral(Ok(Some(true))))
        );
        assert_eq!(
            parse("!ephemeral OFF"),
            Some(Command::Ephemeral(Ok(Some(false))))
        );
        assert_eq!(
            parse("/ephemeral@chaton_bot yes"),
            Some(Command::Ephemeral(Ok(Some(true))))
        );
        assert_eq!(
            parse("/ephemeral xyz"),
            Some(Command::Ephemeral(Err("xyz".into())))
        );
        assert_eq!(parse("!keep"), Some(Command::Keep));
        // Mot inconnu après « ! » : du texte pour le modèle.
        assert_eq!(parse("!ephemerals"), None);
    }

    #[test]
    fn web_search_reads_on_and_off() {
        assert_eq!(parse("/web"), Some(Command::Web(Ok(None))));
        assert_eq!(parse("/web on"), Some(Command::Web(Ok(Some(true)))));
        assert_eq!(parse("!web OFF"), Some(Command::Web(Ok(Some(false)))));
        assert_eq!(parse("/web maybe"), Some(Command::Web(Err("maybe".into()))));
    }

    #[test]
    fn telegram_bot_suffix_is_dropped() {
        assert_eq!(parse("/models@chaton_bot"), Some(Command::Models));
        assert_eq!(
            parse("/model@chaton_bot 2"),
            Some(Command::Model(Some("2".into())))
        );
    }

    #[test]
    fn an_unknown_slash_word_is_an_unknown_command() {
        assert_eq!(
            parse("/frobnicate now"),
            Some(Command::Unknown("frobnicate".into()))
        );
    }

    #[test]
    fn ordinary_text_is_not_a_command() {
        for text in [
            "hello",
            "what does /etc/passwd hold?",
            "!important: read this",
            "!!",
            "!",
            "/",
            "/ models",
            "/usr/bin/env python",
            "/2",
            "",
        ] {
            assert_eq!(parse(text), None, "{text:?}");
        }
    }
}
