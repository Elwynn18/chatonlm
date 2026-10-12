use tauri::State;
use tauri::ipc::Channel;

use crate::attachments::Staging;
use crate::chat::{self, ChatEngine, ChatEvent, SendRequest, Server};
use crate::error::AppResult;
use crate::store::Store;
use crate::store::chats::{self, Chat, ChatInfo};

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
    voice_mode.chat_deleted(&id);
    engine.stop_chat(&id).await;
    // Lecture à voix haute d'une réponse de cette conversation : arrêtée.
    let chat = id.clone();
    let messages: Vec<i64> = store
        .call(move |connection| {
            Ok(chats::get(connection, &chat)
                .map(|chat| chat.messages.iter().map(|message| message.id).collect())
                .unwrap_or_default())
        })
        .await?;
    read_aloud.stop_if_reading(&messages);
    store
        .call(move |connection| chats::delete(connection, &id))
        .await
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

/// Arrête une réponse en cours ; sans effet si elle est déjà finie.
#[tauri::command]
pub async fn cancel_message(engine: State<'_, ChatEngine>, stream_id: String) -> AppResult<()> {
    engine.cancel(&stream_id);
    Ok(())
}
