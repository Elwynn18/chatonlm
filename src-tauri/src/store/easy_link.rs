//! Easy Link (propre à ChatonLM) : bots reliés et comptes liés.
//!
//! Le jeton d'un bot ne sort de ce module que vers le moteur
//! (`crate::easy_link`) : aucun type exporté vers la page ne le porte.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppError, AppResult};

/// Messagerie reliée.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "EasyLinkPlatform")]
pub enum Platform {
    Telegram,
    Discord,
}

impl Platform {
    pub const ALL: [Platform; 2] = [Platform::Telegram, Platform::Discord];

    pub fn as_str(self) -> &'static str {
        easy_link::Platform::from(self).as_str()
    }

    fn parse(value: &str) -> AppResult<Self> {
        easy_link::Platform::parse(value)
            .map(Self::from)
            .ok_or_else(|| AppError::Internal(format!("unknown platform in database: {value}")))
    }
}

impl From<Platform> for easy_link::Platform {
    fn from(platform: Platform) -> Self {
        match platform {
            Platform::Telegram => easy_link::Platform::Telegram,
            Platform::Discord => easy_link::Platform::Discord,
        }
    }
}

impl From<easy_link::Platform> for Platform {
    fn from(platform: easy_link::Platform) -> Self {
        match platform {
            easy_link::Platform::Telegram => Platform::Telegram,
            easy_link::Platform::Discord => Platform::Discord,
        }
    }
}

/// Bot d'une messagerie, jeton compris.
#[derive(Clone, PartialEq, Eq)]
pub struct Bot {
    pub platform: Platform,
    /// Vide : aucun bot relié.
    pub token: String,
    pub enabled: bool,
    pub bot_id: String,
    pub bot_name: String,
}

// Jamais de jeton dans un journal ou un message d'essai.
impl std::fmt::Debug for Bot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bot")
            .field("platform", &self.platform)
            .field("configured", &!self.token.is_empty())
            .field("enabled", &self.enabled)
            .field("bot_id", &self.bot_id)
            .field("bot_name", &self.bot_name)
            .finish()
    }
}

/// Compte lié, tel que la page le montre.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EasyLinkAccount {
    pub id: i64,
    pub platform: Platform,
    /// Nom donné par la messagerie ; peut être vide.
    pub name: String,
    pub linked_at: i64,
}

/// Compte lié, pour le moteur.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub id: i64,
    pub platform: Platform,
    pub user_id: String,
    pub name: String,
    /// Conversation en cours ; absente : la prochaine en ouvre une.
    pub chat_id: Option<String>,
    /// Vide : modèle de l'app.
    pub model: String,
    pub web_search: bool,
    /// Mode éphémère (`/ephemeral on`) : ses nouvelles conversations sont
    /// éphémères. Jamais montré à la page.
    pub ephemeral: bool,
}

/// Les deux bots, Telegram puis Discord.
pub fn bots(connection: &Connection) -> AppResult<Vec<Bot>> {
    Platform::ALL
        .into_iter()
        .map(|platform| bot(connection, platform))
        .collect()
}

pub fn bot(connection: &Connection, platform: Platform) -> AppResult<Bot> {
    Ok(connection.query_row(
        "SELECT token, enabled, bot_id, bot_name FROM easy_link_bots WHERE platform = ?1",
        params![platform.as_str()],
        |row| {
            Ok(Bot {
                platform,
                token: row.get(0)?,
                enabled: row.get(1)?,
                bot_id: row.get(2)?,
                bot_name: row.get(3)?,
            })
        },
    )?)
}

/// Enregistre un bot dont le jeton vient d'être vérifié, et l'active.
pub fn save_bot(
    connection: &Connection,
    platform: Platform,
    token: &str,
    bot_id: &str,
    bot_name: &str,
) -> AppResult<()> {
    connection.execute(
        "UPDATE easy_link_bots SET token = ?2, enabled = 1, bot_id = ?3, bot_name = ?4
         WHERE platform = ?1",
        params![platform.as_str(), token, bot_id, bot_name],
    )?;
    Ok(())
}

/// Identité relue à une connexion (un bot peut être renommé).
pub fn set_identity(
    connection: &Connection,
    platform: Platform,
    bot_id: &str,
    bot_name: &str,
) -> AppResult<()> {
    connection.execute(
        "UPDATE easy_link_bots SET bot_id = ?2, bot_name = ?3 WHERE platform = ?1",
        params![platform.as_str(), bot_id, bot_name],
    )?;
    Ok(())
}

pub fn set_enabled(connection: &Connection, platform: Platform, enabled: bool) -> AppResult<()> {
    connection.execute(
        "UPDATE easy_link_bots SET enabled = ?2 WHERE platform = ?1",
        params![platform.as_str(), enabled],
    )?;
    Ok(())
}

/// Oublie le bot d'une messagerie et délie ses comptes (un autre bot serait
/// un autre interlocuteur : chacun refait l'appairage). Les conversations
/// restent dans l'historique.
pub fn forget_bot(connection: &mut Connection, platform: Platform) -> AppResult<()> {
    // Le jeton effacé ne doit pas rester lisible dans le fichier : ses
    // octets sont écrasés, et le journal vidé une fois l'écriture faite.
    connection.pragma_update(None, "secure_delete", true)?;
    let transaction = connection.transaction()?;
    transaction.execute(
        "UPDATE easy_link_bots SET token = '', enabled = 0, bot_id = '', bot_name = ''
         WHERE platform = ?1",
        params![platform.as_str()],
    )?;
    transaction.execute(
        "DELETE FROM easy_link_accounts WHERE platform = ?1",
        params![platform.as_str()],
    )?;
    transaction.commit()?;
    // Sans effet (et sans erreur) pour une base en mémoire.
    let _ = connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    Ok(())
}

/// Comptes liés, du plus ancien au plus récent.
pub fn accounts(connection: &Connection) -> AppResult<Vec<EasyLinkAccount>> {
    let mut statement = connection
        .prepare("SELECT id, platform, name, linked_at FROM easy_link_accounts ORDER BY id")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    rows.map(|row| {
        let (id, platform, name, linked_at) = row?;
        Ok(EasyLinkAccount {
            id,
            platform: Platform::parse(&platform)?,
            name,
            linked_at,
        })
    })
    .collect()
}

const ACCOUNT_COLUMNS: &str = "id, platform, user_id, name, chat_id, model, web_search, ephemeral";

fn read_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<(Account, String)> {
    let platform: String = row.get(1)?;
    Ok((
        Account {
            id: row.get(0)?,
            // Remplacée par l'appelant (valeur lue juste à côté).
            platform: Platform::Telegram,
            user_id: row.get(2)?,
            name: row.get(3)?,
            chat_id: row.get(4)?,
            model: row.get(5)?,
            web_search: row.get(6)?,
            ephemeral: row.get(7)?,
        },
        platform,
    ))
}

fn with_platform(read: (Account, String)) -> AppResult<Account> {
    let (mut account, platform) = read;
    account.platform = Platform::parse(&platform)?;
    Ok(account)
}

/// Compte lié d'un auteur, s'il l'est.
pub fn account(
    connection: &Connection,
    platform: Platform,
    user_id: &str,
) -> AppResult<Option<Account>> {
    connection
        .query_row(
            &format!(
                "SELECT {ACCOUNT_COLUMNS} FROM easy_link_accounts
                 WHERE platform = ?1 AND user_id = ?2"
            ),
            params![platform.as_str(), user_id],
            read_account,
        )
        .optional()?
        .map(with_platform)
        .transpose()
}

#[cfg(test)]
pub fn account_by_id(connection: &Connection, id: i64) -> AppResult<Option<Account>> {
    connection
        .query_row(
            &format!("SELECT {ACCOUNT_COLUMNS} FROM easy_link_accounts WHERE id = ?1"),
            params![id],
            read_account,
        )
        .optional()?
        .map(with_platform)
        .transpose()
}

/// Comptes liés, pour le moteur (envoi spontané), du plus ancien au plus
/// récent.
pub fn linked(connection: &Connection) -> AppResult<Vec<Account>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {ACCOUNT_COLUMNS} FROM easy_link_accounts ORDER BY id"
    ))?;
    let rows = statement.query_map([], read_account)?;
    rows.map(|row| with_platform(row?)).collect()
}

/// « Let VS Code notify me through Easy Link » (coupé par défaut).
pub fn vscode_notify(connection: &Connection) -> AppResult<bool> {
    Ok(connection.query_row(
        "SELECT vscode_notify FROM easy_link_settings WHERE id = 1",
        [],
        |row| row.get(0),
    )?)
}

pub fn set_vscode_notify(connection: &Connection, enabled: bool) -> AppResult<()> {
    connection.execute(
        "UPDATE easy_link_settings SET vscode_notify = ?1 WHERE id = 1",
        params![enabled],
    )?;
    Ok(())
}

/// Lie un compte (ou rafraîchit le nom d'un compte déjà lié).
pub fn link(
    connection: &Connection,
    platform: Platform,
    user_id: &str,
    name: &str,
    now: i64,
) -> AppResult<Account> {
    connection.execute(
        "INSERT INTO easy_link_accounts (platform, user_id, name, linked_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (platform, user_id) DO UPDATE SET name = excluded.name",
        params![platform.as_str(), user_id, name, now],
    )?;
    account(connection, platform, user_id)?
        .ok_or_else(|| AppError::Internal("linked account not found".into()))
}

/// Délie un compte ; faux s'il ne l'était plus.
pub fn unlink(connection: &Connection, id: i64) -> AppResult<bool> {
    let removed =
        connection.execute("DELETE FROM easy_link_accounts WHERE id = ?1", params![id])?;
    Ok(removed > 0)
}

pub fn set_name(connection: &Connection, id: i64, name: &str) -> AppResult<()> {
    connection.execute(
        "UPDATE easy_link_accounts SET name = ?2 WHERE id = ?1",
        params![id, name],
    )?;
    Ok(())
}

/// Conversation en cours d'un compte ; `None` : la prochaine en ouvre une.
pub fn set_chat(connection: &Connection, id: i64, chat_id: Option<&str>) -> AppResult<()> {
    connection.execute(
        "UPDATE easy_link_accounts SET chat_id = ?2 WHERE id = ?1",
        params![id, chat_id],
    )?;
    Ok(())
}

pub fn set_model(connection: &Connection, id: i64, model: &str) -> AppResult<()> {
    connection.execute(
        "UPDATE easy_link_accounts SET model = ?2 WHERE id = ?1",
        params![id, model],
    )?;
    Ok(())
}

pub fn set_web_search(connection: &Connection, id: i64, enabled: bool) -> AppResult<()> {
    connection.execute(
        "UPDATE easy_link_accounts SET web_search = ?2 WHERE id = ?1",
        params![id, enabled],
    )?;
    Ok(())
}

pub fn set_ephemeral(connection: &Connection, id: i64, enabled: bool) -> AppResult<()> {
    connection.execute(
        "UPDATE easy_link_accounts SET ephemeral = ?2 WHERE id = ?1",
        params![id, enabled],
    )?;
    Ok(())
}

/// Conversation en cours d'un compte si elle est éphémère.
pub fn ephemeral_chat(connection: &Connection, id: i64) -> AppResult<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT chats.id FROM easy_link_accounts
             JOIN chats ON chats.id = easy_link_accounts.chat_id
             WHERE easy_link_accounts.id = ?1 AND chats.ephemeral = 1",
            params![id],
            |row| row.get(0),
        )
        .optional()?)
}

/// Détache la conversation en cours d'un compte si elle est éphémère, et la
/// rend, à effacer par l'appelant : un message arrivé ensuite en ouvre une
/// autre au lieu de partir dans celle qui va être effacée.
pub fn detach_ephemeral(connection: &Connection, id: i64) -> AppResult<Option<String>> {
    let chat = ephemeral_chat(connection, id)?;
    if chat.is_some() {
        set_chat(connection, id, None)?;
    }
    Ok(chat)
}

/// Rattache au compte la conversation que sa réponse vient d'ouvrir, s'il
/// n'en a pas d'autre et que son mode éphémère est encore celui de la
/// conversation : `/ephemeral`, passé entre la lecture du compte et
/// l'ouverture, a pu le changer. Vrai si elle l'est.
pub fn attach_chat(connection: &Connection, id: i64, chat_id: &str) -> AppResult<bool> {
    let attached = connection.execute(
        "UPDATE easy_link_accounts SET chat_id = ?2
         WHERE id = ?1 AND chat_id IS NULL
           AND ephemeral = (SELECT ephemeral FROM chats WHERE id = ?2)",
        params![id, chat_id],
    )?;
    Ok(attached > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Store, chats};

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn two_bots_exist_without_token() {
        let bots = store().blocking(|c| bots(c)).unwrap();
        assert_eq!(
            bots.iter().map(|bot| bot.platform).collect::<Vec<_>>(),
            Platform::ALL
        );
        assert!(bots.iter().all(|bot| bot.token.is_empty() && !bot.enabled));
    }

    #[test]
    fn a_saved_bot_is_enabled_and_keeps_its_identity() {
        let store = store();
        store
            .blocking(|c| {
                save_bot(c, Platform::Discord, "secret", "42", "chaton")?;
                let saved = bot(c, Platform::Discord)?;
                assert_eq!(
                    (saved.token.as_str(), saved.enabled, saved.bot_name.as_str()),
                    ("secret", true, "chaton")
                );
                // L'autre messagerie ne bouge pas.
                assert!(bot(c, Platform::Telegram)?.token.is_empty());
                set_enabled(c, Platform::Discord, false)?;
                set_identity(c, Platform::Discord, "42", "chaton2")?;
                let saved = bot(c, Platform::Discord)?;
                assert_eq!((saved.enabled, saved.bot_name.as_str()), (false, "chaton2"));
                assert_eq!(saved.token, "secret");
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn the_token_never_shows_in_debug_output() {
        let bot = Bot {
            platform: Platform::Telegram,
            token: "123456:very-secret".into(),
            enabled: true,
            bot_id: "1".into(),
            bot_name: "chaton".into(),
        };
        let shown = format!("{bot:?}");
        assert!(!shown.contains("very-secret"), "{shown}");
        assert!(shown.contains("configured: true"), "{shown}");
    }

    #[test]
    fn linking_twice_keeps_one_account_and_its_choices() {
        let store = store();
        store
            .blocking(|c| {
                let first = link(c, Platform::Telegram, "7", "Ana", 10)?;
                set_model(c, first.id, "gemma3:4b")?;
                set_web_search(c, first.id, true)?;
                let again = link(c, Platform::Telegram, "7", "Ana B.", 99)?;
                assert_eq!(again.id, first.id);
                assert_eq!(again.name, "Ana B.");
                assert_eq!(again.model, "gemma3:4b");
                assert!(again.web_search);
                // Même identifiant sur l'autre messagerie : un autre compte.
                let other = link(c, Platform::Discord, "7", "Ana", 11)?;
                assert_ne!(other.id, first.id);
                let listed = accounts(c)?;
                assert_eq!(listed.len(), 2);
                // La date de la première liaison reste.
                assert_eq!(listed[0].linked_at, 10);
                assert_eq!(account(c, Platform::Telegram, "8")?, None);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_deleted_chat_leaves_the_account_without_conversation() {
        let store = store();
        store
            .blocking(|c| {
                let account = link(c, Platform::Telegram, "7", "Ana", 10)?;
                let chat = chats::create(c, 20)?;
                set_chat(c, account.id, Some(&chat))?;
                assert_eq!(
                    account_by_id(c, account.id)?.unwrap().chat_id.as_deref(),
                    Some(chat.as_str())
                );
                chats::delete(c, &chat)?;
                assert_eq!(account_by_id(c, account.id)?.unwrap().chat_id, None);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn the_ephemeral_mode_is_kept_and_only_an_ephemeral_chat_is_found() {
        let store = store();
        store
            .blocking(|c| {
                let account = link(c, Platform::Telegram, "7", "Ana", 10)?;
                assert!(!account.ephemeral);
                set_ephemeral(c, account.id, true)?;
                // Gardé d'une liaison à l'autre, comme les autres choix.
                assert!(link(c, Platform::Telegram, "7", "Ana", 11)?.ephemeral);
                assert_eq!(ephemeral_chat(c, account.id)?, None);
                let ordinary = chats::create(c, 20)?;
                set_chat(c, account.id, Some(&ordinary))?;
                assert_eq!(ephemeral_chat(c, account.id)?, None);
                let ephemeral = chats::create_with(c, 21, true)?;
                set_chat(c, account.id, Some(&ephemeral))?;
                assert_eq!(ephemeral_chat(c, account.id)?, Some(ephemeral.clone()));
                // Effacée : le compte n'a plus de conversation.
                assert!(chats::forget(c, &ephemeral)?);
                assert_eq!(account_by_id(c, account.id)?.unwrap().chat_id, None);
                set_ephemeral(c, account.id, false)?;
                assert!(!account_by_id(c, account.id)?.unwrap().ephemeral);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn an_ephemeral_chat_is_detached_before_it_is_erased() {
        let store = store();
        store
            .blocking(|c| {
                let account = link(c, Platform::Telegram, "7", "Ana", 10)?;
                let ordinary = chats::create(c, 20)?;
                set_chat(c, account.id, Some(&ordinary))?;
                // Une conversation ordinaire reste rattachée.
                assert_eq!(detach_ephemeral(c, account.id)?, None);
                assert_eq!(
                    account_by_id(c, account.id)?.unwrap().chat_id.as_deref(),
                    Some(ordinary.as_str())
                );
                let ephemeral = chats::create_with(c, 21, true)?;
                set_chat(c, account.id, Some(&ephemeral))?;
                assert_eq!(detach_ephemeral(c, account.id)?, Some(ephemeral.clone()));
                // Détachée, pas encore effacée.
                assert_eq!(account_by_id(c, account.id)?.unwrap().chat_id, None);
                assert!(chats::is_ephemeral(c, &ephemeral)?);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_new_chat_is_attached_only_if_it_matches_the_mode() {
        let store = store();
        store
            .blocking(|c| {
                let account = link(c, Platform::Telegram, "7", "Ana", 10)?;
                let ordinary = chats::create(c, 20)?;
                let ephemeral = chats::create_with(c, 21, true)?;
                // Mode coupé : une conversation éphémère n'est pas rattachée.
                assert!(!attach_chat(c, account.id, &ephemeral)?);
                assert!(attach_chat(c, account.id, &ordinary)?);
                // Déjà rattaché : rien ne change.
                let other = chats::create(c, 22)?;
                assert!(!attach_chat(c, account.id, &other)?);
                set_chat(c, account.id, None)?;
                set_ephemeral(c, account.id, true)?;
                // Mode actif : seulement une conversation éphémère.
                assert!(!attach_chat(c, account.id, &other)?);
                assert!(attach_chat(c, account.id, &ephemeral)?);
                assert_eq!(
                    account_by_id(c, account.id)?.unwrap().chat_id.as_deref(),
                    Some(ephemeral.as_str())
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn forgetting_a_bot_unlinks_only_its_accounts() {
        let store = store();
        store
            .blocking(|c| {
                save_bot(c, Platform::Telegram, "t", "1", "a")?;
                save_bot(c, Platform::Discord, "d", "2", "b")?;
                link(c, Platform::Telegram, "7", "Ana", 10)?;
                let kept = link(c, Platform::Discord, "8", "Luis", 11)?;
                forget_bot(c, Platform::Telegram)?;
                let forgotten = bot(c, Platform::Telegram)?;
                assert!(forgotten.token.is_empty() && !forgotten.enabled);
                assert!(forgotten.bot_name.is_empty());
                assert_eq!(bot(c, Platform::Discord)?.token, "d");
                let left = accounts(c)?;
                assert_eq!(left.iter().map(|a| a.id).collect::<Vec<_>>(), [kept.id]);
                assert!(unlink(c, kept.id)?);
                assert!(!unlink(c, kept.id)?);
                Ok(())
            })
            .unwrap();
    }
}
