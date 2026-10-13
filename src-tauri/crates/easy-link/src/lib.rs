//! Easy Link (fonction propre à ChatonLM) : un bot Telegram ou Discord relie
//! une messagerie au modèle de l'app. Ce crate ne tient que les protocoles :
//! recevoir les messages privés d'un bot, y répondre, lire ses fichiers,
//! mettre en forme et découper le texte, décoder un message vocal.
//!
//! Crate à part, sans Tauri ni base de données : ses essais se compilent vite
//! et tournent contre ses propres faux serveurs (fonctionnalité `fake`). Il ne
//! produit aucun texte montré à l'utilisateur : l'app traduit ([`Problem`] est
//! typé pour cela). Qui a le droit de parler au bot, quelle conversation et
//! quel modèle : c'est l'app qui décide (`src-tauri/src/easy_link/`).
//!
//! Règles communes aux deux messageries :
//! - messages privés seulement : un message d'un groupe, d'un serveur ou d'un
//!   autre bot n'est jamais remis à l'app ;
//! - le jeton n'apparaît dans aucune erreur ni aucun journal (l'adresse d'une
//!   requête Telegram le contient : une erreur de `reqwest` passe par
//!   `without_url`) ;
//! - le texte reçu par [`Messenger::send`] et [`Messenger::edit`] est du
//!   Markdown tel que l'écrit un modèle ; chaque messagerie le rend à sa
//!   façon, et un texte en cours d'écriture (bloc de code non fermé) reste
//!   valable.

use std::time::Duration;

use tokio::sync::{mpsc, watch};

pub mod discord;
#[cfg(feature = "fake")]
pub mod fake;
pub mod telegram;
pub mod text;
pub mod voice;

/// Messagerie reliée.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    Telegram,
    Discord,
}

impl Platform {
    pub const ALL: [Platform; 2] = [Platform::Telegram, Platform::Discord];

    /// Nom gardé en base et dans les types de la page.
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Telegram => "telegram",
            Platform::Discord => "discord",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|platform| platform.as_str() == name)
    }
}

/// Le bot lui-même, tel que la messagerie le décrit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Identifiant du bot (Telegram : son identifiant d'utilisateur ; Discord :
    /// celui de l'application, qui sert à son lien d'invitation).
    pub id: String,
    /// Nom d'utilisateur, sans « @ ».
    pub name: String,
}

/// Auteur d'un message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sender {
    /// Identifiant stable du compte dans la messagerie.
    pub id: String,
    /// Nom à montrer (prénom et nom, ou nom d'utilisateur) ; peut être vide.
    pub name: String,
}

/// Message privé reçu par le bot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incoming {
    pub sender: Sender,
    /// Où répondre : à redonner tel quel à [`Messenger::send`].
    pub channel: String,
    /// Texte du message, ou légende de ses fichiers ; vide s'il n'y en a pas.
    pub text: String,
    pub files: Vec<IncomingFile>,
    /// Date d'envoi (secondes Unix), si la messagerie la donne : l'app écarte
    /// un message trop ancien, remis après une longue absence.
    pub sent_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// Photo ou image (PNG, JPEG, WebP).
    Image,
    /// Message vocal (OGG, Opus).
    Voice,
    /// Tout le reste : l'app le refuse sans le lire.
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncomingFile {
    pub kind: FileKind,
    /// Nom du fichier, avec une extension qui dit sa sorte (`photo.jpg`,
    /// `voice.ogg` quand la messagerie n'en donne pas).
    pub name: String,
    /// Taille annoncée en octets, si connue.
    pub size: Option<u64>,
    pub handle: FileHandle,
}

/// De quoi lire un fichier reçu, à redonner à [`Messenger::download`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileHandle {
    Telegram { file_id: String },
    Discord { url: String },
}

/// Échec d'un appel à une messagerie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Jeton refusé (Telegram : 401 ou 404 ; Discord : 401, ou fermeture 4004
    /// de la passerelle) : inutile de réessayer.
    Unauthorized,
    /// Telegram : une autre instance lit déjà les messages de ce bot (409).
    Conflict,
    /// Le destinataire a bloqué le bot ou fermé ses messages privés (403).
    Forbidden,
    /// Trop de requêtes : attendre cette durée avant la suivante.
    RateLimited(Duration),
    /// Fichier au-delà de la taille permise.
    TooLarge,
    /// Autre refus de la messagerie ; `message` est le sien, sans jeton.
    Refused { status: u16, message: String },
    /// Réseau, délai dépassé, réponse illisible ; texte sans adresse.
    Network(String),
}

/// Ce que [`Messenger::listen`] remet à l'app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Le bot reçoit les messages (aussi après une reconnexion).
    Connected(Identity),
    /// Il ne les reçoit plus ; une fois par coupure. `Unauthorized` est
    /// définitif (l'écoute se termine), le reste est réessayé.
    Disconnected(Problem),
    Message(Incoming),
}

/// Bot d'une messagerie.
#[derive(Clone)]
pub enum Messenger {
    Telegram(telegram::Client),
    Discord(discord::Client),
}

impl Messenger {
    pub fn platform(&self) -> Platform {
        match self {
            Messenger::Telegram(_) => Platform::Telegram,
            Messenger::Discord(_) => Platform::Discord,
        }
    }

    /// Longueur sûre d'un message (en unités UTF-16 du texte rendu), sous la
    /// limite de la messagerie : au-delà, [`text::split`].
    pub fn text_limit(&self) -> usize {
        match self {
            Messenger::Telegram(_) => telegram::TEXT_LIMIT,
            Messenger::Discord(_) => discord::TEXT_LIMIT,
        }
    }

    /// Vérifie le jeton et rend le bot (15 s au plus).
    pub async fn identity(&self) -> Result<Identity, Problem> {
        match self {
            Messenger::Telegram(client) => client.identity().await,
            Messenger::Discord(client) => client.identity().await,
        }
    }

    /// Reçoit les messages privés jusqu'à ce que `stop` passe à vrai (ou que
    /// `events` soit fermé). Se reconnecte seul, avec une attente croissante
    /// (1 s, puis le double, 60 s au plus) ; ne revient avant `stop` que sur
    /// [`Problem::Unauthorized`], après l'avoir annoncé.
    pub async fn listen(&self, events: mpsc::Sender<Event>, stop: watch::Receiver<bool>) {
        match self {
            Messenger::Telegram(client) => client.listen(events, stop).await,
            Messenger::Discord(client) => client.listen(events, stop).await,
        }
    }

    /// Envoie un message (Markdown, [`Messenger::text_limit`] au plus) et rend
    /// son identifiant, à redonner à [`Messenger::edit`].
    pub async fn send(&self, channel: &str, markdown: &str) -> Result<String, Problem> {
        match self {
            Messenger::Telegram(client) => client.send(channel, markdown).await,
            Messenger::Discord(client) => client.send(channel, markdown).await,
        }
    }

    /// Remplace le texte d'un message du bot. Un texte identique à celui
    /// déjà affiché n'est pas une erreur.
    pub async fn edit(&self, channel: &str, message: &str, markdown: &str) -> Result<(), Problem> {
        match self {
            Messenger::Telegram(client) => client.edit(channel, message, markdown).await,
            Messenger::Discord(client) => client.edit(channel, message, markdown).await,
        }
    }

    /// Montre « écrit… » dans la conversation (quelques secondes : l'app le
    /// redemande tant qu'elle prépare une réponse).
    pub async fn typing(&self, channel: &str) -> Result<(), Problem> {
        match self {
            Messenger::Telegram(client) => client.typing(channel).await,
            Messenger::Discord(client) => client.typing(channel).await,
        }
    }

    /// Lit un fichier reçu, `max` octets au plus ([`Problem::TooLarge`]
    /// au-delà, sans tout lire). Une poignée de l'autre messagerie est
    /// refusée.
    pub async fn download(&self, file: &FileHandle, max: u64) -> Result<Vec<u8>, Problem> {
        match self {
            Messenger::Telegram(client) => client.download(file, max).await,
            Messenger::Discord(client) => client.download(file, max).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_names_round_trip() {
        for platform in Platform::ALL {
            assert_eq!(Platform::parse(platform.as_str()), Some(platform));
        }
        assert_eq!(Platform::parse("signal"), None);
    }
}
