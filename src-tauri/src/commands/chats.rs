use tauri::State;
use tauri::ipc::Channel;

use crate::attachments::Staging;
use crate::chat::{self, ChatEngine, ChatEvent, SendRequest, Server};
use crate::error::AppResult;
use crate::store::Store;
use crate::store::chats::{self, Chat, ChatInfo, Compaction};

#[tauri::command]
pub async fn list_chats(store: State<'_, Store>) -> AppResult<Vec<ChatInfo>> {
    store.call(|connection| chats::list(connection)).await
}

/// Conversation complète ; une réponse en cours y figure dans son dernier état.
#[tauri::command]
pub async fn get_chat(
    store: State<'_, Store>,
    engine: State<'_, ChatEngine>,
    id: String,
) -> AppResult<Chat> {
    let mut chat = store
        .call(move |connection| chats::get(connection, &id))
        .await?;
    engine.overlay(&mut chat);
    Ok(chat)
}

#[tauri::command]
pub async fn rename_chat(store: State<'_, Store>, id: String, title: String) -> AppResult<()> {
    store
        .call(move |connection| chats::rename(connection, &id, &title))
        .await
}

/// Range une conversation dans les favoris ou l'en retire.
#[tauri::command]
pub async fn set_chat_favorite(
    store: State<'_, Store>,
    id: String,
    favorite: bool,
) -> AppResult<()> {
    store
        .call(move |connection| chats::set_favorite(connection, &id, favorite))
        .await
}

/// Supprime une conversation, après avoir arrêté sa réponse en cours (et la
/// séance du mode vocal qui s'y tient).
#[tauri::command]
pub async fn delete_chat(
    store: State<'_, Store>,
    engine: State<'_, ChatEngine>,
    read_aloud: State<'_, crate::tts::ReadAloud>,
    voice_mode: State<'_, crate::voice_mode::VoiceMode>,
    id: String,
) -> AppResult<()> {
    stop_work(&store, &engine, &read_aloud, &voice_mode, &id).await?;
    store
        .call(move |connection| chats::delete(connection, &id))
        .await
}

/// Conversation éphémère quittée (propre à ChatonLM) : effacée comme par
/// `delete_chat`, seulement si elle est toujours éphémère (une conversation
/// gardée entre-temps, ou ordinaire, reste) ; rien pour une conversation
/// inconnue.
#[tauri::command]
pub async fn forget_chat(
    store: State<'_, Store>,
    engine: State<'_, ChatEngine>,
    read_aloud: State<'_, crate::tts::ReadAloud>,
    voice_mode: State<'_, crate::voice_mode::VoiceMode>,
    id: String,
) -> AppResult<()> {
    let chat = id.clone();
    let ephemeral = store
        .call(
            move |connection| match chats::is_ephemeral(connection, &chat) {
                Err(crate::error::AppError::NotFound(_)) => Ok(false),
                result => result,
            },
        )
        .await?;
    if !ephemeral {
        return Ok(());
    }
    stop_work(&store, &engine, &read_aloud, &voice_mode, &id).await?;
    store
        .call(move |connection| chats::forget(connection, &id).map(|_| ()))
        .await
}

/// « Keep » d'une conversation éphémère (propre à ChatonLM) : elle passe dans
/// l'historique.
#[tauri::command]
pub async fn keep_chat(store: State<'_, Store>, id: String) -> AppResult<()> {
    store
        .call(move |connection| chats::keep(connection, &id))
        .await
}

/// Arrête ce qui touche une conversation avant de l'effacer : la séance du
/// mode vocal qui s'y tient, sa réponse en cours, la lecture à voix haute
/// d'une de ses réponses.
async fn stop_work(
    store: &Store,
    engine: &ChatEngine,
    read_aloud: &crate::tts::ReadAloud,
    voice_mode: &crate::voice_mode::VoiceMode,
    id: &str,
) -> AppResult<()> {
    voice_mode.chat_deleted(id);
    engine.stop_chat(id).await;
    let chat = id.to_owned();
    let messages: Vec<i64> = store
        .call(move |connection| {
            Ok(chats::get(connection, &chat)
                .map(|chat| chat.messages.iter().map(|message| message.id).collect())
                .unwrap_or_default())
        })
        .await?;
    read_aloud.stop_if_reading(&messages);
    Ok(())
}

/// Envoie un message ; la réponse arrive par `on_event`, et la commande se
/// termine après l'événement final.
#[tauri::command]
pub async fn send_message(
    store: State<'_, Store>,
    engine: State<'_, ChatEngine>,
    staging: State<'_, Staging>,
    request: SendRequest,
    on_event: Channel<ChatEvent>,
) -> AppResult<()> {
    let settings = crate::ollama::target::request_settings(&store).await?;
    let server = Server::from_settings(&settings);
    chat::send(&store, &engine, &staging, server, request, &on_event).await
}

/// Compacte une conversation avec le modèle donné (propre à ChatonLM) : le
/// modèle résume ce qui précède, les requêtes suivantes n'envoient plus que
/// ce résumé et les messages qui le suivent.
#[tauri::command]
pub async fn compact_chat(
    store: State<'_, Store>,
    engine: State<'_, ChatEngine>,
    chat_id: String,
    model: String,
) -> AppResult<Compaction> {
    let settings = crate::ollama::target::request_settings(&store).await?;
    let server = Server::from_settings(&settings);
    chat::compact_chat(&store, &engine, server, &chat_id, &model).await
}

/// Arrête une réponse en cours ; sans effet si elle est déjà finie.
#[tauri::command]
pub async fn cancel_message(engine: State<'_, ChatEngine>, stream_id: String) -> AppResult<()> {
    engine.cancel(&stream_id);
    Ok(())
}
