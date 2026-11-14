//! Outils de la Transcription téléchargés à la demande (yt-dlp et Deno,
//! module `tools`) : état, installation après accord des licences, mise à
//! jour, annulation et retrait. Chaque changement part aussi vers la page
//! par l'événement `tools-status`.

use tauri::State;

use crate::error::AppResult;
use crate::tools::{Tools, ToolsStatus};

/// État des deux outils ; `check` : relève la dernière version si le
/// relevé est dû (au plus une fois par jour pour yt-dlp, par semaine pour
/// Deno), jamais pendant une transcription. Le résultat du relevé arrive
/// par l'événement.
#[tauri::command]
pub async fn tools_status(tools: State<'_, Tools>, check: bool) -> AppResult<ToolsStatus> {
    Ok(tools.status(check))
}

/// Télécharge et installe les outils absents ; `accept_licenses` : licences
/// de yt-dlp (et de ses composants) et de Deno acceptées sur la page.
#[tauri::command]
pub async fn tools_install(tools: State<'_, Tools>, accept_licenses: bool) -> AppResult<()> {
    tools.install(accept_licenses)
}

/// Relève tout de suite la dernière version et installe les plus récentes
/// (refusé pendant une transcription).
#[tauri::command]
pub async fn tools_update(tools: State<'_, Tools>) -> AppResult<()> {
    tools.update()
}

/// Arrête les téléchargements et installations en cours.
#[tauri::command]
pub async fn tools_cancel(tools: State<'_, Tools>) -> AppResult<()> {
    tools.cancel();
    Ok(())
}

/// Retire les deux outils, leurs archives et leurs caches (refusé pendant
/// une transcription).
#[tauri::command]
pub async fn tools_remove(tools: State<'_, Tools>) -> AppResult<()> {
    tools.remove().await
}
