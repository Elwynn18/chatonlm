//! Conversations et messages.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::easy_link::Platform;

/// Longueur maximale de l'extrait du premier message, affiché dans
/// l'historique tant que la conversation n'a pas de titre.
const EXCERPT_CHARS: usize = 100;

/// Longueur maximale d'un titre.
const TITLE_CHARS: usize = 200;

/// Entrée de l'historique.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatInfo {
    pub id: String,
    /// Vide tant que la conversation n'a pas été renommée.
    pub title: String,
    /// Début du premier message de l'utilisateur.
    pub user_excerpt: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Rangée dans « Favorites », en tête de l'historique (propre à ChatonLM).
    pub favorite: bool,
    /// Messagerie d'où vient la conversation (Easy Link, propre à ChatonLM) ;
    /// absente : écrite dans l'app.
    pub source: Option<Platform>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    fn parse(value: &str) -> AppResult<Self> {
        match value {
            "user" => Ok(Role::User),
            "assistant" => Ok(Role::Assistant),
            other => Err(AppError::Internal(format!(
                "unknown role in database: {other}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum AttachmentKind {
    Image,
    Text,
}

/// Pièce jointe, sans son contenu.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AttachmentInfo {
    pub id: i64,
    pub filename: String,
    pub kind: AttachmentKind,
    pub mime: String,
    pub size: i64,
}

/// Pièce jointe à enregistrer avec un message.
pub struct NewAttachment<'a> {
    pub filename: &'a str,
    pub kind: AttachmentKind,
    pub mime: &'a str,
    pub data: &'a [u8],
    /// Texte extrait (aucun pour une image).
    pub text: Option<&'a str>,
}

impl AttachmentKind {
    fn as_str(self) -> &'static str {
        match self {
            AttachmentKind::Image => "image",
            AttachmentKind::Text => "text",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Message {
    pub id: i64,
    pub role: Role,
    pub content: String,
    pub thinking: String,
    pub model: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub thinking_started_at: Option<i64>,
    pub thinking_ended_at: Option<i64>,
    pub attachments: Vec<AttachmentInfo>,
    /// Appels d'outils d'une réponse de l'assistant, dans l'ordre (vide le
    /// plus souvent).
    pub tool_calls: Vec<ToolCall>,
}

/// Appel d'outil (recherche web) d'une réponse de l'assistant, avec son
/// résultat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ToolCall {
    pub name: String,
    /// Arguments donnés par le modèle : objet JSON (texte compact).
    pub arguments: String,
    /// JSON renvoyé par l'outil ; absent tant qu'il n'a pas répondu, et en
    /// cas d'erreur.
    pub result: Option<String>,
    /// Erreur de l'outil (sans « Error: »).
    pub error: Option<String>,
}

impl ToolCall {
    /// Appel reçu du modèle, pas encore exécuté.
    pub fn new(name: impl Into<String>, arguments: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            arguments: arguments.into(),
            result: None,
            error: None,
        }
    }

    /// Résultat ou erreur reçus ; sinon l'appel a été interrompu (arrêt, app
    /// fermée) et n'est jamais renvoyé au modèle.
    pub fn is_finished(&self) -> bool {
        self.result.is_some() || self.error.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub messages: Vec<Message>,
}

/// Historique, de la plus récemment active à la plus ancienne.
pub fn list(connection: &Connection) -> AppResult<Vec<ChatInfo>> {
    let mut statement = connection.prepare(
        "SELECT c.id, c.title, c.created_at, c.updated_at,
                (SELECT m.content FROM messages m
                 WHERE m.chat_id = c.id AND m.role = 'user'
                 ORDER BY m.id LIMIT 1),
                c.favorite, c.source
         FROM chats c
         ORDER BY c.updated_at DESC, c.id DESC",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(ChatInfo {
            id: row.get(0)?,
            title: row.get(1)?,
            created_at: row.get(2)?,
            updated_at: row.get(3)?,
            user_excerpt: excerpt(&row.get::<_, Option<String>>(4)?.unwrap_or_default()),
            favorite: row.get(5)?,
            // Valeur inconnue (base d'une version plus récente) : sans marque.
            source: row
                .get::<_, Option<String>>(6)?
                .and_then(|source| easy_link::Platform::parse(&source))
                .map(Platform::from),
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Conversation complète (pièces jointes sans leur contenu).
pub fn get(connection: &Connection, id: &str) -> AppResult<Chat> {
    let chat = connection
        .query_row(
            "SELECT id, title, created_at, updated_at FROM chats WHERE id = ?1",
            params![id],
            |row| {
                Ok(Chat {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                    messages: Vec::new(),
                })
            },
        )
        .optional()?
        .ok_or_else(|| not_found(id))?;

    let mut statement = connection.prepare(
        "SELECT id, role, content, thinking, model, created_at, updated_at,
                thinking_started_at, thinking_ended_at
         FROM messages WHERE chat_id = ?1 ORDER BY id",
    )?;
    let mut messages = statement
        .query_map(params![id], |row| {
            Ok((
                Message {
                    id: row.get(0)?,
                    role: Role::User,
                    content: row.get(2)?,
                    thinking: row.get(3)?,
                    model: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                    thinking_started_at: row.get(7)?,
                    thinking_ended_at: row.get(8)?,
                    attachments: Vec::new(),
                    tool_calls: Vec::new(),
                },
                row.get::<_, String>(1)?,
            ))
        })?
        .map(|row| {
            let (mut message, role) = row?;
            message.role = Role::parse(&role)?;
            Ok(message)
        })
        .collect::<AppResult<Vec<_>>>()?;

    let mut attachments = connection.prepare(
        "SELECT a.message_id, a.id, a.filename, a.kind, a.mime, a.size
         FROM attachments a JOIN messages m ON m.id = a.message_id
         WHERE m.chat_id = ?1 ORDER BY a.id",
    )?;
    let rows = attachments.query_map(params![id], |row| {
        Ok((row.get::<_, i64>(0)?, attachment_info(row, 1)?))
    })?;
    for row in rows {
        let (message_id, attachment) = row?;
        if let Some(message) = messages.iter_mut().find(|m| m.id == message_id) {
            message.attachments.push(attachment);
        }
    }

    for (message_id, call) in saved_tool_calls(connection, id, false)? {
        if let Some(message) = messages.iter_mut().find(|m| m.id == message_id) {
            message.tool_calls.push(call);
        }
    }

    Ok(Chat { messages, ..chat })
}

/// Appels d'outils des messages d'une conversation, par message puis dans
/// l'ordre ; `finished` : seulement ceux qui ont un résultat ou une erreur.
fn saved_tool_calls(
    connection: &Connection,
    chat_id: &str,
    finished: bool,
) -> AppResult<Vec<(i64, ToolCall)>> {
    let mut statement = connection.prepare(
        "SELECT t.message_id, t.name, t.arguments, t.result, t.error
         FROM tool_calls t JOIN messages m ON m.id = t.message_id
         WHERE m.chat_id = ?1 AND (?2 = 0 OR t.result IS NOT NULL OR t.error IS NOT NULL)
         ORDER BY t.message_id, t.position",
    )?;
    let rows = statement.query_map(params![chat_id, finished], |row| {
        Ok((
            row.get(0)?,
            ToolCall {
                name: row.get(1)?,
                arguments: row.get(2)?,
                result: row.get(3)?,
                error: row.get(4)?,
            },
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Pièce jointe lue à partir de la colonne `first` : id, filename, kind, mime,
/// size.
fn attachment_info(row: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<AttachmentInfo> {
    Ok(AttachmentInfo {
        id: row.get(first)?,
        filename: row.get(first + 1)?,
        kind: if row.get::<_, String>(first + 2)? == "image" {
            AttachmentKind::Image
        } else {
            AttachmentKind::Text
        },
        mime: row.get(first + 3)?,
        size: row.get(first + 4)?,
    })
}

/// Conversation introuvable (supprimée entre-temps).
fn not_found(id: &str) -> AppError {
    AppError::NotFound(t!("chat.notFound", id = id))
}

/// Renomme sans toucher à la date d'activité : la conversation garde sa place
/// dans l'historique, comme dans l'app officielle.
pub fn rename(connection: &Connection, id: &str, title: &str) -> AppResult<()> {
    let title = title.trim();
    if title.is_empty() {
        return Err(AppError::Invalid(t!("chat.emptyTitle")));
    }
    let title: String = title.chars().take(TITLE_CHARS).collect();
    let changed = connection.execute(
        "UPDATE chats SET title = ?1 WHERE id = ?2",
        params![title, id],
    )?;
    if changed == 0 {
        return Err(not_found(id));
    }
    Ok(())
}

/// Range une conversation dans les favoris ou l'en retire, sans toucher à
/// la date d'activité (comme `rename`).
pub fn set_favorite(connection: &Connection, id: &str, favorite: bool) -> AppResult<()> {
    let changed = connection.execute(
        "UPDATE chats SET favorite = ?1 WHERE id = ?2",
        params![favorite, id],
    )?;
    if changed == 0 {
        return Err(not_found(id));
    }
    Ok(())
}

/// Supprime une conversation, ses messages et ses pièces jointes.
pub fn delete(connection: &Connection, id: &str) -> AppResult<()> {
    connection.execute("DELETE FROM chats WHERE id = ?1", params![id])?;
    Ok(())
}

/// Crée une conversation vide ; renvoie son identifiant.
pub fn create(connection: &Connection, now: i64) -> AppResult<String> {
    let id = uuid::Uuid::now_v7().to_string();
    connection.execute(
        "INSERT INTO chats (id, title, created_at, updated_at) VALUES (?1, '', ?2, ?2)",
        params![id, now],
    )?;
    Ok(id)
}

/// Marque une conversation ouverte par un bot d'Easy Link.
pub fn set_source(connection: &Connection, id: &str, source: Platform) -> AppResult<()> {
    connection.execute(
        "UPDATE chats SET source = ?2 WHERE id = ?1",
        params![id, source.as_str()],
    )?;
    Ok(())
}

/// Ajoute un message de l'utilisateur et ses pièces jointes (dans l'ordre),
/// en une transaction ; la conversation remonte en tête de l'historique.
pub fn add_user_message(
    connection: &mut Connection,
    chat_id: &str,
    content: &str,
    attachments: &[NewAttachment<'_>],
    now: i64,
) -> AppResult<Message> {
    let transaction = connection.transaction()?;
    let changed = transaction.execute(
        "UPDATE chats SET updated_at = ?2 WHERE id = ?1",
        params![chat_id, now],
    )?;
    if changed == 0 {
        return Err(not_found(chat_id));
    }
    transaction.execute(
        "INSERT INTO messages (chat_id, role, content, created_at, updated_at)
         VALUES (?1, 'user', ?2, ?3, ?3)",
        params![chat_id, content, now],
    )?;
    let id = transaction.last_insert_rowid();
    let infos = insert_attachments(&transaction, id, attachments)?;
    transaction.commit()?;
    Ok(Message {
        id,
        role: Role::User,
        content: content.to_owned(),
        thinking: String::new(),
        model: None,
        created_at: now,
        updated_at: now,
        thinking_started_at: None,
        thinking_ended_at: None,
        attachments: infos,
        tool_calls: Vec::new(),
    })
}

/// Enregistre des pièces jointes d'un message, dans l'ordre.
fn insert_attachments(
    connection: &Connection,
    message_id: i64,
    attachments: &[NewAttachment<'_>],
) -> AppResult<Vec<AttachmentInfo>> {
    let mut infos = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let size = i64::try_from(attachment.data.len())
            .map_err(|error| AppError::Internal(error.to_string()))?;
        connection.execute(
            "INSERT INTO attachments (message_id, filename, kind, mime, size, data, text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                message_id,
                attachment.filename,
                attachment.kind.as_str(),
                attachment.mime,
                size,
                attachment.data,
                attachment.text
            ],
        )?;
        infos.push(AttachmentInfo {
            id: connection.last_insert_rowid(),
            filename: attachment.filename.to_owned(),
            kind: attachment.kind,
            mime: attachment.mime.to_owned(),
            size,
        });
    }
    Ok(infos)
}

/// Vérifie une modification avant de l'écrire : `message_id` est un message de
/// l'utilisateur de cette conversation, et `kept` des pièces jointes de ce
/// message, chacune une seule fois. Refus : `AppError::Invalid`.
pub fn check_edit(
    connection: &Connection,
    chat_id: &str,
    message_id: i64,
    kept: &[i64],
) -> AppResult<()> {
    let found: Option<(String, String)> = connection
        .query_row(
            "SELECT chat_id, role FROM messages WHERE id = ?1",
            params![message_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    // Erreurs en anglais : la page ne modifie que ses propres messages, avec
    // leurs pièces jointes.
    let Some((owner, role)) = found else {
        // i18n-ignore
        return Err(AppError::Invalid(format!("unknown message: {message_id}")));
    };
    if owner != chat_id {
        // i18n-ignore : identifiants donnés par la page, jamais incohérents.
        return Err(AppError::Invalid(format!(
            "message {message_id} is in another chat"
        )));
    }
    if role != "user" {
        // i18n-ignore : identifiants donnés par la page, jamais incohérents.
        return Err(AppError::Invalid(format!(
            "message {message_id} is not a user message"
        )));
    }
    let own = attachment_ids(connection, message_id)?;
    for (index, id) in kept.iter().enumerate() {
        if !own.contains(id) {
            // i18n-ignore : identifiants donnés par la page, jamais incohérents.
            return Err(AppError::Invalid(format!(
                "attachment {id} is not in message {message_id}"
            )));
        }
        if kept[..index].contains(id) {
            // i18n-ignore
            return Err(AppError::Invalid(format!("attachment {id} is kept twice")));
        }
    }
    Ok(())
}

/// Modifie un message de l'utilisateur, en une transaction (comme l'officiel,
/// ui.go : l'historique est coupé au message modifié, qui est remplacé) : son
/// texte change (date de création gardée), les messages suivants de la
/// conversation sont supprimés avec leurs pièces jointes, ses pièces jointes
/// absentes de `kept` aussi, puis les nouvelles sont ajoutées ; la
/// conversation remonte en tête de l'historique. Renvoie le message, ses
/// pièces jointes dans l'ordre : gardées dans leur ordre d'origine (l'ordre de
/// `kept` n'y change rien), puis nouvelles (identifiants jamais réutilisés,
/// donc plus grands). Refus de [`check_edit`] : rien n'est écrit.
pub fn edit_user_message(
    connection: &mut Connection,
    chat_id: &str,
    message_id: i64,
    content: &str,
    kept: &[i64],
    attachments: &[NewAttachment<'_>],
    now: i64,
) -> AppResult<Message> {
    let transaction = connection.transaction()?;
    // Revérifié dans la transaction : la conversation a pu changer depuis la
    // vérification faite avant l'envoi.
    check_edit(&transaction, chat_id, message_id, kept)?;
    transaction.execute(
        "UPDATE chats SET updated_at = ?2 WHERE id = ?1",
        params![chat_id, now],
    )?;
    transaction.execute(
        "UPDATE messages SET content = ?2, updated_at = ?3 WHERE id = ?1",
        params![message_id, content, now],
    )?;
    // Pièces jointes des messages supprimés : en cascade (clés étrangères
    // actives, voir store::configure).
    transaction.execute(
        "DELETE FROM messages WHERE chat_id = ?1 AND id > ?2",
        params![chat_id, message_id],
    )?;
    for id in attachment_ids(&transaction, message_id)? {
        if kept.contains(&id) {
            continue;
        }
        transaction.execute("DELETE FROM attachments WHERE id = ?1", params![id])?;
    }
    insert_attachments(&transaction, message_id, attachments)?;
    let message = user_message(&transaction, message_id)?;
    transaction.commit()?;
    Ok(message)
}

/// Identifiants des pièces jointes d'un message, dans l'ordre.
fn attachment_ids(connection: &Connection, message_id: i64) -> AppResult<Vec<i64>> {
    let mut statement =
        connection.prepare("SELECT id FROM attachments WHERE message_id = ?1 ORDER BY id")?;
    let ids = statement
        .query_map(params![message_id], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

/// Message de l'utilisateur relu en base, avec ses pièces jointes dans
/// l'ordre.
fn user_message(connection: &Connection, id: i64) -> AppResult<Message> {
    let (content, created_at, updated_at) = connection.query_row(
        "SELECT content, created_at, updated_at FROM messages WHERE id = ?1",
        params![id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let mut statement = connection.prepare(
        "SELECT id, filename, kind, mime, size FROM attachments
         WHERE message_id = ?1 ORDER BY id",
    )?;
    let attachments = statement
        .query_map(params![id], |row| attachment_info(row, 0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Message {
        id,
        role: Role::User,
        content,
        thinking: String::new(),
        model: None,
        created_at,
        updated_at,
        thinking_started_at: None,
        thinking_ended_at: None,
        attachments,
        tool_calls: Vec::new(),
    })
}

/// Message de l'historique envoyé au modèle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub role: Role,
    pub content: String,
    pub thinking: String,
    /// Pièces jointes, dans l'ordre.
    pub files: Vec<TurnFile>,
    /// Appels d'outils terminés d'une réponse, dans l'ordre : envoyés avec
    /// elle, chacun suivi de son message `tool`.
    pub tool_calls: Vec<ToolCall>,
}

/// Pièce jointe d'un message de l'historique, telle qu'envoyée au modèle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnFile {
    /// Contenu d'origine de l'image.
    Image(Vec<u8>),
    /// Texte extrait à la préparation.
    Text { filename: String, text: String },
}

/// Historique pour le modèle, sans les messages vides (comme l'officiel :
/// ni texte, ni raisonnement, ni pièce jointe, ni appel d'outil). Seuls les
/// appels terminés (résultat ou erreur) en font partie : un appel interrompu
/// n'a pas de réponse à donner au modèle.
pub fn history(connection: &Connection, chat_id: &str) -> AppResult<Vec<Turn>> {
    let mut statement = connection.prepare(
        "SELECT id, role, content, thinking FROM messages WHERE chat_id = ?1 ORDER BY id",
    )?;
    let rows = statement.query_map(params![chat_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut turns = Vec::new();
    let mut ids = Vec::new();
    for row in rows {
        let (id, role, content, thinking) = row?;
        ids.push(id);
        turns.push(Turn {
            role: Role::parse(&role)?,
            content,
            thinking,
            files: Vec::new(),
            tool_calls: Vec::new(),
        });
    }

    // Images : leur contenu ; autres fichiers : leur texte extrait.
    let mut attachments = connection.prepare(
        "SELECT a.message_id, a.filename, a.kind,
                CASE WHEN a.kind = 'image' THEN a.data END, a.text
         FROM attachments a JOIN messages m ON m.id = a.message_id
         WHERE m.chat_id = ?1 ORDER BY a.id",
    )?;
    let rows = attachments.query_map(params![chat_id], |row| {
        let file = if row.get::<_, String>(2)? == "image" {
            TurnFile::Image(row.get(3)?)
        } else {
            TurnFile::Text {
                filename: row.get(1)?,
                text: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            }
        };
        Ok((row.get::<_, i64>(0)?, file))
    })?;
    for row in rows {
        let (message_id, file) = row?;
        if let Some(index) = ids.iter().position(|id| *id == message_id) {
            turns[index].files.push(file);
        }
    }

    for (message_id, call) in saved_tool_calls(connection, chat_id, true)? {
        if let Some(index) = ids.iter().position(|id| *id == message_id) {
            turns[index].tool_calls.push(call);
        }
    }
    turns.retain(|turn| {
        !(turn.content.is_empty()
            && turn.thinking.is_empty()
            && turn.files.is_empty()
            && turn.tool_calls.is_empty())
    });
    Ok(turns)
}

/// Image enregistrée (type MIME et contenu), pour le protocole `chatonlm://` ;
/// `None` pour une pièce jointe inconnue ou qui n'est pas une image.
pub fn image_attachment(connection: &Connection, id: i64) -> AppResult<Option<(String, Vec<u8>)>> {
    Ok(connection
        .query_row(
            "SELECT mime, data FROM attachments WHERE id = ?1 AND kind = 'image'",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

/// État d'une réponse de l'assistant à enregistrer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub content: String,
    pub thinking: String,
    pub thinking_started_at: Option<i64>,
    pub thinking_ended_at: Option<i64>,
    /// Appels d'outils et résultats reçus, pour l'état courant superposé à la
    /// conversation ; enregistrés à part ([`add_tool_calls`],
    /// [`set_tool_result`]), jamais par [`save_reply`].
    pub tool_calls: Vec<ToolCall>,
}

/// Crée la réponse de l'assistant (au premier fragment reçu) ; renvoie son
/// identifiant.
pub fn create_reply(
    connection: &Connection,
    chat_id: &str,
    model: &str,
    now: i64,
) -> AppResult<i64> {
    connection.execute(
        "INSERT INTO messages (chat_id, role, content, model, created_at, updated_at)
         VALUES (?1, 'assistant', '', ?2, ?3, ?3)",
        params![chat_id, model, now],
    )?;
    Ok(connection.last_insert_rowid())
}

/// Enregistre l'état courant de la réponse ; `finished` remonte aussi la
/// conversation en tête de l'historique.
pub fn save_reply(
    connection: &mut Connection,
    id: i64,
    reply: &Reply,
    finished: bool,
    now: i64,
) -> AppResult<()> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "UPDATE messages SET content = ?2, thinking = ?3, thinking_started_at = ?4,
                thinking_ended_at = ?5, updated_at = ?6
         WHERE id = ?1",
        params![
            id,
            reply.content,
            reply.thinking,
            reply.thinking_started_at,
            reply.thinking_ended_at,
            now
        ],
    )?;
    if finished {
        transaction.execute(
            "UPDATE chats SET updated_at = ?2
             WHERE id = (SELECT chat_id FROM messages WHERE id = ?1)",
            params![id, now],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

/// Enregistre des appels d'outils d'une réponse, à la suite de ses appels
/// déjà enregistrés (positions consécutives à partir de 0), en une
/// transaction.
pub fn add_tool_calls(
    connection: &mut Connection,
    message_id: i64,
    calls: &[ToolCall],
) -> AppResult<()> {
    let transaction = connection.transaction()?;
    let first: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM tool_calls WHERE message_id = ?1",
        params![message_id],
        |row| row.get(0),
    )?;
    for (position, call) in (first..).zip(calls) {
        transaction.execute(
            "INSERT INTO tool_calls (message_id, position, name, arguments, result, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                message_id,
                position,
                call.name,
                call.arguments,
                call.result,
                call.error
            ],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

/// Enregistre le résultat (ou l'erreur) de l'appel `index` d'une réponse.
pub fn set_tool_result(
    connection: &Connection,
    message_id: i64,
    index: u32,
    result: Option<&str>,
    error: Option<&str>,
) -> AppResult<()> {
    let changed = connection.execute(
        "UPDATE tool_calls SET result = ?3, error = ?4 WHERE message_id = ?1 AND position = ?2",
        params![message_id, index, result, error],
    )?;
    if changed == 0 {
        return Err(AppError::Internal(format!(
            "tool call {index} of message {message_id} not found"
        )));
    }
    Ok(())
}

/// Au démarrage : clôt les réflexions restées ouvertes (app arrêtée pendant
/// une réponse) et retire les réponses restées vides (une réponse faite
/// seulement d'appels d'outils, même interrompus, reste).
pub fn repair(connection: &Connection) -> AppResult<()> {
    connection.execute_batch(
        "UPDATE messages SET thinking_ended_at = updated_at
         WHERE thinking_started_at IS NOT NULL AND thinking_ended_at IS NULL;
         DELETE FROM messages
         WHERE role = 'assistant' AND content = '' AND thinking = ''
           AND NOT EXISTS (SELECT 1 FROM attachments a WHERE a.message_id = messages.id)
           AND NOT EXISTS (SELECT 1 FROM tool_calls t WHERE t.message_id = messages.id);",
    )?;
    Ok(())
}

/// Premier message ramené à une ligne et coupé.
fn excerpt(content: &str) -> String {
    let line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    line.chars().take(EXCERPT_CHARS).collect()
}

/// Conversations de démonstration, pour les essais de l'interface (builds de
/// débogage et d'essai seulement).
#[cfg(any(debug_assertions, feature = "e2e"))]
pub fn seed_demo(connection: &mut Connection, now: i64) -> AppResult<Vec<String>> {
    const DAY: i64 = 24 * 60 * 60 * 1000;
    let demo: [(&str, i64, &str, &str); 6] = [
        (
            "",
            0,
            "How do I reverse a list in Python?",
            "Use `reversed()` or slicing: `items[::-1]`.",
        ),
        (
            "Trip ideas",
            2 * 60 * 60 * 1000,
            "Plan a weekend in Lyon",
            "Here is a two-day plan for Lyon.",
        ),
        (
            "",
            2 * DAY,
            "Explain the difference between TCP and UDP",
            "TCP is connection-oriented; UDP is not.",
        ),
        (
            "",
            4 * DAY,
            "Write a haiku about autumn",
            "Leaves drift on cold wind…",
        ),
        (
            "",
            10 * DAY,
            "Summarize the plot of Hamlet",
            "A prince seeks revenge for his father.",
        ),
        (
            "Rust notes",
            40 * DAY,
            "What is the borrow checker?",
            "It enforces Rust's ownership rules.",
        ),
    ];
    let transaction = connection.transaction()?;
    let mut ids = Vec::new();
    for (title, age, question, answer) in demo {
        let id = uuid::Uuid::now_v7().to_string();
        let at = now - age;
        transaction.execute(
            "INSERT INTO chats (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![id, title, at],
        )?;
        transaction.execute(
            "INSERT INTO messages (chat_id, role, content, created_at, updated_at)
             VALUES (?1, 'user', ?2, ?3, ?3)",
            params![id, question, at],
        )?;
        transaction.execute(
            "INSERT INTO messages (chat_id, role, content, model, created_at, updated_at)
             VALUES (?1, 'assistant', ?2, 'qwen3:0.6b', ?3, ?3)",
            params![id, answer, at],
        )?;
        ids.push(id);
    }
    transaction.commit()?;
    Ok(ids)
}

/// `CHATONLM_SEED_DEMO` : au démarrage d'un build de débogage ou d'essai, un
/// historique vide reçoit les conversations de démonstration. Un historique
/// déjà rempli reste tel quel (`tauri dev` relance l'app à chaque compilation).
#[cfg(any(debug_assertions, feature = "e2e"))]
pub fn seed_demo_if_empty(connection: &mut Connection, now: i64) -> AppResult<()> {
    let empty: bool =
        connection.query_row("SELECT NOT EXISTS (SELECT 1 FROM chats)", [], |row| {
            row.get(0)
        })?;
    if empty {
        seed_demo(connection, now)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    const NOW: i64 = 1_790_000_000_000;

    fn seeded() -> (Store, Vec<String>) {
        let store = Store::open_in_memory().unwrap();
        let ids = store.blocking(|c| seed_demo(c, NOW)).unwrap();
        (store, ids)
    }

    #[test]
    fn a_conversation_from_user_message_to_finished_reply() {
        let store = Store::open_in_memory().unwrap();
        let id = store.blocking(|c| create(c, NOW)).unwrap();
        let user = store
            .blocking(|c| add_user_message(c, &id, "Say hello", &[], NOW + 1))
            .unwrap();
        assert_eq!(user.role, Role::User);
        assert_eq!(user.model, None);

        let reply_id = store
            .blocking(|c| create_reply(c, &id, "qwen3:0.6b", NOW + 2))
            .unwrap();
        // Réponse vide : absente de l'historique envoyé au modèle.
        let turns = store.blocking(|c| history(c, &id)).unwrap();
        assert_eq!(turns.len(), 1);

        let reply = Reply {
            content: "Hello".into(),
            thinking: "Short.".into(),
            thinking_started_at: Some(NOW + 2),
            thinking_ended_at: Some(NOW + 3),
            ..Reply::default()
        };
        store
            .blocking(|c| save_reply(c, reply_id, &reply, true, NOW + 4))
            .unwrap();
        let chat = store.blocking(|c| get(c, &id)).unwrap();
        assert_eq!(chat.updated_at, NOW + 4);
        let saved = &chat.messages[1];
        assert_eq!(saved.content, "Hello");
        assert_eq!(saved.model.as_deref(), Some("qwen3:0.6b"));
        assert_eq!(saved.thinking_ended_at, Some(NOW + 3));
        let turns = store.blocking(|c| history(c, &id)).unwrap();
        assert_eq!(
            turns[1],
            Turn {
                role: Role::Assistant,
                content: "Hello".into(),
                thinking: "Short.".into(),
                files: Vec::new(),
                tool_calls: Vec::new(),
            }
        );
        assert!(matches!(
            store.blocking(|c| add_user_message(c, "missing", "x", &[], NOW)),
            Err(AppError::NotFound(_))
        ));
    }

    #[test]
    fn attachments_are_saved_in_order_and_sent_with_the_history() {
        let store = Store::open_in_memory().unwrap();
        let id = store.blocking(|c| create(c, NOW)).unwrap();
        let files = [
            NewAttachment {
                filename: "notes.txt",
                kind: AttachmentKind::Text,
                mime: "text/plain",
                data: b"Line 1",
                text: Some("Line 1"),
            },
            NewAttachment {
                filename: "red.png",
                kind: AttachmentKind::Image,
                mime: "image/png",
                data: b"\x89PNG",
                text: None,
            },
        ];
        let user = store
            .blocking(|c| add_user_message(c, &id, "Look", &files, NOW))
            .unwrap();
        let infos: Vec<_> = user
            .attachments
            .iter()
            .map(|a| (a.filename.as_str(), a.kind, a.mime.as_str(), a.size))
            .collect();
        assert_eq!(
            infos,
            [
                ("notes.txt", AttachmentKind::Text, "text/plain", 6),
                ("red.png", AttachmentKind::Image, "image/png", 4)
            ]
        );
        assert_eq!(store.blocking(|c| get(c, &id)).unwrap().messages[0], user);
        assert_eq!(
            store
                .blocking(|c| image_attachment(c, user.attachments[1].id))
                .unwrap(),
            Some(("image/png".to_owned(), b"\x89PNG".to_vec()))
        );
        assert_eq!(
            store
                .blocking(|c| image_attachment(c, user.attachments[0].id))
                .unwrap(),
            None
        );

        // Comme l'officiel, un message sans texte mais avec une pièce jointe
        // reste dans l'historique (aucun envoi n'en crée aujourd'hui).
        store
            .blocking(|c| {
                c.execute(
                    "INSERT INTO messages (chat_id, role, content, created_at, updated_at)
                     VALUES (?1, 'user', '', ?2, ?2)",
                    params![id, NOW + 1],
                )?;
                c.execute(
                    "INSERT INTO attachments (message_id, filename, kind, size, data)
                     VALUES (?1, 'old.md', 'text', 2, x'6869')",
                    params![c.last_insert_rowid()],
                )?;
                Ok(())
            })
            .unwrap();
        let turns = store.blocking(|c| history(c, &id)).unwrap();
        assert_eq!(
            turns,
            [
                Turn {
                    role: Role::User,
                    content: "Look".into(),
                    thinking: String::new(),
                    files: vec![
                        TurnFile::Text {
                            filename: "notes.txt".into(),
                            text: "Line 1".into()
                        },
                        TurnFile::Image(b"\x89PNG".to_vec()),
                    ],
                    tool_calls: Vec::new(),
                },
                // Texte absent (jamais extrait) : vide.
                Turn {
                    role: Role::User,
                    content: String::new(),
                    thinking: String::new(),
                    files: vec![TurnFile::Text {
                        filename: "old.md".into(),
                        text: String::new()
                    }],
                    tool_calls: Vec::new(),
                },
            ]
        );
    }

    /// Fichier texte dont le contenu et le texte sont le nom.
    fn text_file(name: &str) -> NewAttachment<'_> {
        NewAttachment {
            filename: name,
            kind: AttachmentKind::Text,
            mime: "text/plain",
            data: name.as_bytes(),
            text: Some(name),
        }
    }

    /// Conversation `chat` : message 1 (a.txt, b.txt, c.txt : pièces jointes
    /// 1 à 3), réponse 2, message 3 (d.txt : 4), réponse 4 ; conversation
    /// `other` : message 5 (e.txt : 5), aux identifiants plus grands.
    fn two_chats(store: &Store) -> (String, String) {
        store
            .blocking(|c| {
                let chat = create(c, NOW)?;
                let other = create(c, NOW)?;
                let files = [text_file("a.txt"), text_file("b.txt"), text_file("c.txt")];
                add_user_message(c, &chat, "First", &files, NOW + 1)?;
                let reply = create_reply(c, &chat, "qwen3:0.6b", NOW + 2)?;
                let answer = Reply {
                    content: "Answer".into(),
                    ..Reply::default()
                };
                save_reply(c, reply, &answer, true, NOW + 2)?;
                add_user_message(c, &chat, "Second", &[text_file("d.txt")], NOW + 3)?;
                let reply = create_reply(c, &chat, "qwen3:0.6b", NOW + 4)?;
                save_reply(c, reply, &answer, true, NOW + 4)?;
                add_user_message(c, &other, "Elsewhere", &[text_file("e.txt")], NOW + 5)?;
                Ok((chat, other))
            })
            .unwrap()
    }

    /// Toutes les pièces jointes en base : (message, nom), dans l'ordre.
    fn all_attachments(store: &Store) -> Vec<(i64, String)> {
        store
            .blocking(|c| {
                let mut statement =
                    c.prepare("SELECT message_id, filename FROM attachments ORDER BY id")?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .unwrap()
    }

    #[test]
    fn editing_a_message_cuts_the_chat_and_orders_its_attachments() {
        let store = Store::open_in_memory().unwrap();
        let (chat, other) = two_chats(&store);
        let other_before = store.blocking(|c| get(c, &other)).unwrap();
        // c.txt et a.txt gardés (dans le désordre), b.txt retiré, new.txt
        // ajouté.
        let edited = store
            .blocking(|c| {
                let files = [text_file("new.txt")];
                edit_user_message(c, &chat, 1, "First, edited", &[3, 1], &files, NOW + 10)
            })
            .unwrap();
        assert_eq!(edited.id, 1);
        assert_eq!(edited.role, Role::User);
        assert_eq!(edited.content, "First, edited");
        assert_eq!(edited.created_at, NOW + 1);
        assert_eq!(edited.updated_at, NOW + 10);
        // Gardées dans leur ordre d'origine, puis la nouvelle, sous un
        // identifiant jamais utilisé.
        let attachments: Vec<_> = edited
            .attachments
            .iter()
            .map(|a| (a.id, a.filename.as_str(), a.size))
            .collect();
        assert_eq!(
            attachments,
            [(1, "a.txt", 5), (3, "c.txt", 5), (6, "new.txt", 7)]
        );

        // Messages suivants supprimés, avec leurs pièces jointes (d.txt) ;
        // l'autre conversation, aux identifiants plus grands, intacte.
        let saved = store.blocking(|c| get(c, &chat)).unwrap();
        assert_eq!(saved.messages, [edited]);
        assert_eq!(saved.updated_at, NOW + 10);
        assert_eq!(store.blocking(|c| get(c, &other)).unwrap(), other_before);
        assert_eq!(
            all_attachments(&store),
            [
                (1, "a.txt".to_owned()),
                (1, "c.txt".to_owned()),
                (5, "e.txt".to_owned()),
                (1, "new.txt".to_owned())
            ]
        );
        let text = |name: &str| TurnFile::Text {
            filename: name.into(),
            text: name.into(),
        };
        assert_eq!(
            store.blocking(|c| history(c, &chat)).unwrap(),
            [Turn {
                role: Role::User,
                content: "First, edited".into(),
                thinking: String::new(),
                files: vec![text("a.txt"), text("c.txt"), text("new.txt")],
                tool_calls: Vec::new(),
            }]
        );

        // Rien de gardé, rien d'ajouté : plus aucune pièce jointe.
        let bare = store
            .blocking(|c| edit_user_message(c, &chat, 1, "Bare", &[], &[], NOW + 20))
            .unwrap();
        assert!(bare.attachments.is_empty());
        assert_eq!(all_attachments(&store), [(5, "e.txt".to_owned())]);
    }

    #[test]
    fn edits_that_do_not_fit_write_nothing() {
        let store = Store::open_in_memory().unwrap();
        let (chat, other) = two_chats(&store);
        let snapshot = |store: &Store| {
            let chats = store.blocking(|c| Ok((get(c, &chat)?, get(c, &other)?)));
            (chats.unwrap(), all_attachments(store))
        };
        let before = snapshot(&store);
        // Message, pièces jointes gardées, refus attendu.
        for (message, kept, expected) in [
            (99, &[][..], "unknown message: 99"),
            (5, &[][..], "message 5 is in another chat"),
            (2, &[][..], "message 2 is not a user message"),
            // d.txt (message 3) et e.txt (autre conversation).
            (1, &[1, 4][..], "attachment 4 is not in message 1"),
            (1, &[5][..], "attachment 5 is not in message 1"),
            (1, &[2, 1, 2][..], "attachment 2 is kept twice"),
        ] {
            let refused = store.blocking(|c| {
                let files = [text_file("new.txt")];
                edit_user_message(c, &chat, message, "Edited", kept, &files, NOW + 10)
            });
            let Err(AppError::Invalid(text)) = refused else {
                panic!("{expected}: {refused:?}");
            };
            assert_eq!(text, expected);
            assert_eq!(snapshot(&store), before, "{expected}");
            // La même vérification, faite avant l'envoi.
            let checked = store.blocking(|c| check_edit(c, &chat, message, kept));
            assert!(
                matches!(&checked, Err(AppError::Invalid(text)) if text == expected),
                "{checked:?}"
            );
        }
        assert!(store.blocking(|c| check_edit(c, &chat, 1, &[3, 1])).is_ok());
    }

    fn finished(
        name: &str,
        arguments: &str,
        result: Option<&str>,
        error: Option<&str>,
    ) -> ToolCall {
        ToolCall {
            result: result.map(str::to_owned),
            error: error.map(str::to_owned),
            ..ToolCall::new(name, arguments)
        }
    }

    fn count_tool_calls(store: &Store) -> i64 {
        store
            .blocking(|c| Ok(c.query_row("SELECT COUNT(*) FROM tool_calls", [], |r| r.get(0))?))
            .unwrap()
    }

    #[test]
    fn tool_calls_are_saved_in_order_and_only_finished_ones_are_sent() {
        let store = Store::open_in_memory().unwrap();
        let id = store.blocking(|c| create(c, NOW)).unwrap();
        store
            .blocking(|c| add_user_message(c, &id, "Search", &[], NOW))
            .unwrap();
        // Tour d'outils : une réponse faite seulement d'appels.
        let calls_only = store
            .blocking(|c| create_reply(c, &id, "qwen3:0.6b", NOW + 1))
            .unwrap();
        let calls = [
            ToolCall::new("web_search", r#"{"query":"a"}"#),
            ToolCall::new("web_fetch", r#"{"url":"https://example.com"}"#),
            ToolCall::new("web_search", r#"{"query":"b"}"#),
        ];
        store
            .blocking(|c| add_tool_calls(c, calls_only, &calls))
            .unwrap();
        let found = r#"{"results":[]}"#;
        store
            .blocking(|c| set_tool_result(c, calls_only, 0, Some(found), None))
            .unwrap();
        store
            .blocking(|c| set_tool_result(c, calls_only, 1, None, Some("boom")))
            .unwrap();
        assert!(matches!(
            store.blocking(|c| set_tool_result(c, calls_only, 3, Some("{}"), None)),
            Err(AppError::Internal(_))
        ));
        // Tour suivant : la réponse ; puis un tour arrêté pendant son outil.
        let answer = store
            .blocking(|c| create_reply(c, &id, "qwen3:0.6b", NOW + 2))
            .unwrap();
        let text = Reply {
            content: "Answer".into(),
            ..Reply::default()
        };
        store
            .blocking(|c| save_reply(c, answer, &text, true, NOW + 3))
            .unwrap();
        let stopped = store
            .blocking(|c| create_reply(c, &id, "qwen3:0.6b", NOW + 4))
            .unwrap();
        store
            .blocking(|c| add_tool_calls(c, stopped, &[ToolCall::new("web_search", "{}")]))
            .unwrap();

        let chat = store.blocking(|c| get(c, &id)).unwrap();
        let expected = [
            finished("web_search", r#"{"query":"a"}"#, Some(found), None),
            finished(
                "web_fetch",
                r#"{"url":"https://example.com"}"#,
                None,
                Some("boom"),
            ),
            ToolCall::new("web_search", r#"{"query":"b"}"#),
        ];
        assert_eq!(chat.messages[1].tool_calls, expected);
        assert!(chat.messages[0].tool_calls.is_empty());
        assert!(chat.messages[2].tool_calls.is_empty());
        assert_eq!(chat.messages[3].tool_calls.len(), 1);

        // Envoyés : les appels terminés seulement ; le message fait d'un seul
        // appel interrompu est vide, donc absent.
        let turns = store.blocking(|c| history(c, &id)).unwrap();
        assert_eq!(turns.len(), 3, "{turns:?}");
        assert_eq!(
            turns[1],
            Turn {
                role: Role::Assistant,
                content: String::new(),
                thinking: String::new(),
                files: Vec::new(),
                tool_calls: expected[..2].to_vec(),
            }
        );
        assert_eq!(turns[2].content, "Answer");
        assert!(turns[2].tool_calls.is_empty());

        // D'autres appels sur la même réponse : à la suite.
        store
            .blocking(|c| add_tool_calls(c, calls_only, &[ToolCall::new("web_fetch", "{}")]))
            .unwrap();
        store
            .blocking(|c| set_tool_result(c, calls_only, 3, Some("{}"), None))
            .unwrap();
        let chat = store.blocking(|c| get(c, &id)).unwrap();
        assert_eq!(
            chat.messages[1].tool_calls[3],
            finished("web_fetch", "{}", Some("{}"), None)
        );
    }

    #[test]
    fn editing_a_message_deletes_the_tool_calls_that_followed_it() {
        let store = Store::open_in_memory().unwrap();
        let (chat, other) = two_chats(&store);
        // Réponses 2 et 4 de `chat`, et une réponse de `other`.
        let elsewhere = store
            .blocking(|c| create_reply(c, &other, "qwen3:0.6b", NOW + 6))
            .unwrap();
        for message in [2, 4, elsewhere] {
            store
                .blocking(|c| add_tool_calls(c, message, &[ToolCall::new("web_search", "{}")]))
                .unwrap();
        }
        assert_eq!(count_tool_calls(&store), 3);
        store
            .blocking(|c| edit_user_message(c, &chat, 3, "Second, edited", &[4], &[], NOW + 10))
            .unwrap();
        assert_eq!(count_tool_calls(&store), 2);
        let saved = store.blocking(|c| get(c, &chat)).unwrap();
        assert_eq!(saved.messages[1].tool_calls.len(), 1);
        store
            .blocking(|c| edit_user_message(c, &chat, 1, "First, edited", &[], &[], NOW + 11))
            .unwrap();
        assert_eq!(count_tool_calls(&store), 1);
        let kept = store.blocking(|c| get(c, &other)).unwrap();
        assert_eq!(kept.messages[1].tool_calls.len(), 1);
    }

    #[test]
    fn repair_closes_thinking_and_drops_empty_replies() {
        let store = Store::open_in_memory().unwrap();
        let id = store.blocking(|c| create(c, NOW)).unwrap();
        store
            .blocking(|c| add_user_message(c, &id, "Hi", &[], NOW))
            .unwrap();
        let empty = store
            .blocking(|c| create_reply(c, &id, "qwen3:0.6b", NOW))
            .unwrap();
        let open = store
            .blocking(|c| create_reply(c, &id, "qwen3:0.6b", NOW))
            .unwrap();
        let thinking = Reply {
            thinking: "Hmm".into(),
            thinking_started_at: Some(NOW),
            ..Reply::default()
        };
        store
            .blocking(|c| save_reply(c, open, &thinking, false, NOW + 9))
            .unwrap();
        // Faite seulement d'un appel d'outil interrompu : gardée.
        let calls_only = store
            .blocking(|c| create_reply(c, &id, "qwen3:0.6b", NOW))
            .unwrap();
        store
            .blocking(|c| add_tool_calls(c, calls_only, &[ToolCall::new("web_search", "{}")]))
            .unwrap();
        store.blocking(|c| repair(c)).unwrap();
        let messages = store.blocking(|c| get(c, &id)).unwrap().messages;
        assert!(messages.iter().all(|m| m.id != empty));
        let open = messages.iter().find(|m| m.id == open).unwrap();
        assert_eq!(open.thinking_ended_at, Some(NOW + 9));
        let calls_only = messages.iter().find(|m| m.id == calls_only).unwrap();
        assert_eq!(calls_only.tool_calls.len(), 1);
    }

    #[test]
    fn demo_seeding_only_fills_an_empty_history() {
        let store = Store::open_in_memory().unwrap();
        store.blocking(|c| seed_demo_if_empty(c, NOW)).unwrap();
        store.blocking(|c| seed_demo_if_empty(c, NOW)).unwrap();
        assert_eq!(store.blocking(|c| list(c)).unwrap().len(), 6);
    }

    #[test]
    fn history_is_sorted_by_activity_with_excerpts() {
        let (store, ids) = seeded();
        let chats = store.blocking(|c| list(c)).unwrap();
        assert_eq!(chats.len(), 6);
        assert_eq!(chats.iter().map(|c| c.id.clone()).collect::<Vec<_>>(), ids);
        assert_eq!(chats[0].user_excerpt, "How do I reverse a list in Python?");
        assert_eq!(chats[1].title, "Trip ideas");
        assert!(chats.windows(2).all(|w| w[0].updated_at >= w[1].updated_at));
    }

    #[test]
    fn excerpt_is_one_line_and_short() {
        assert_eq!(excerpt("  Hello\n\n  world  "), "Hello world");
        assert_eq!(excerpt(&"x".repeat(500)).chars().count(), EXCERPT_CHARS);
    }

    #[test]
    fn get_returns_messages_in_order() {
        let (store, ids) = seeded();
        let chat = store.blocking(|c| get(c, &ids[0])).unwrap();
        assert_eq!(chat.messages.len(), 2);
        assert_eq!(chat.messages[0].role, Role::User);
        assert_eq!(chat.messages[1].role, Role::Assistant);
        assert_eq!(chat.messages[1].model.as_deref(), Some("qwen3:0.6b"));
        assert!(matches!(
            store.blocking(|c| get(c, "missing")),
            Err(AppError::NotFound(_))
        ));
    }

    #[test]
    fn rename_keeps_the_activity_date_and_the_attachments() {
        let (store, ids) = seeded();
        let id = ids[3].clone();
        store
            .blocking(|c| {
                let message_id: i64 = c.query_row(
                    "SELECT id FROM messages WHERE chat_id = ?1 AND role = 'user'",
                    params![id],
                    |r| r.get(0),
                )?;
                c.execute(
                    "INSERT INTO attachments (message_id, filename, kind, size, data)
                     VALUES (?1, 'notes.txt', 'text', 5, x'68656c6c6f')",
                    params![message_id],
                )?;
                Ok(())
            })
            .unwrap();
        let before = store.blocking(|c| get(c, &id)).unwrap();
        store
            .blocking(|c| rename(c, &id, "  Autumn poem  "))
            .unwrap();
        let after = store.blocking(|c| get(c, &id)).unwrap();
        assert_eq!(after.title, "Autumn poem");
        assert_eq!(after.updated_at, before.updated_at);
        assert_eq!(after.messages[0].attachments.len(), 1);
        assert_eq!(after.messages[0].attachments[0].filename, "notes.txt");
    }

    #[test]
    fn rename_refuses_empty_titles_and_unknown_chats() {
        let (store, ids) = seeded();
        assert!(matches!(
            store.blocking(|c| rename(c, &ids[0], "   ")),
            Err(AppError::Invalid(_))
        ));
        assert!(matches!(
            store.blocking(|c| rename(c, "missing", "Title")),
            Err(AppError::NotFound(_))
        ));
    }

    #[test]
    fn favorites_are_listed_without_moving_in_the_history() {
        let (store, ids) = seeded();
        let order = |store: &Store| -> Vec<(String, bool)> {
            store
                .blocking(|c| list(c))
                .unwrap()
                .into_iter()
                .map(|chat| (chat.id, chat.favorite))
                .collect()
        };
        let before = order(&store);
        assert!(before.iter().all(|(_, favorite)| !favorite));
        let id = before[2].0.clone();
        store.blocking(|c| set_favorite(c, &id, true)).unwrap();
        let after = order(&store);
        assert_eq!(
            after.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            before.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            "même ordre : la date d'activité ne change pas"
        );
        assert_eq!(
            after
                .iter()
                .filter(|(_, f)| *f)
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            [&id]
        );
        store.blocking(|c| set_favorite(c, &id, false)).unwrap();
        assert!(order(&store).iter().all(|(_, favorite)| !favorite));
        assert!(matches!(
            store.blocking(|c| set_favorite(c, "missing", true)),
            Err(AppError::NotFound(_))
        ));
        assert_eq!(ids.len(), before.len());
    }

    #[test]
    fn delete_removes_messages_and_attachments() {
        let (store, ids) = seeded();
        store.blocking(|c| delete(c, &ids[0])).unwrap();
        let remaining: (i64, i64) = store
            .blocking(|c| {
                Ok(c.query_row(
                    "SELECT (SELECT COUNT(*) FROM chats), (SELECT COUNT(*) FROM messages)",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })
            .unwrap();
        assert_eq!(remaining, (5, 10));
    }
}
