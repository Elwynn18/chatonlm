//! Commandes du mode conversation vocale (fonction propre à ChatonLM ;
//! contrat `.tools/research/63-voice-mode-contract.md`).

use tauri::{AppHandle, State};

use crate::error::{AppError, AppResult};
use crate::store::Store;
use crate::store::voice_mode::{self, VoiceModeSettings, VoiceModeSettingsPatch};
use crate::voice_mode::{VoiceMode, VoiceModeInfo, VoiceModeRequirements, VoiceModeState};

/// Ce qui manque avant de démarrer (modèle de transcription, voix, machine).
#[tauri::command]
pub async fn voice_mode_requirements(app: AppHandle) -> AppResult<VoiceModeRequirements> {
    crate::voice_mode::session::prepare(&app)
        .await
        .map(|prepared| prepared.requirements)
        .map_err(AppError::Invalid)
}

/// Démarre (sans effet si une séance est en cours) ; erreur traduite si une
/// dictée est en cours, si un prérequis manque, si le micro ne s'ouvre pas.
#[tauri::command]
pub async fn voice_mode_start(voice_mode: State<'_, VoiceMode>) -> AppResult<()> {
    voice_mode.start().await.map_err(AppError::Invalid)
}

/// Termine : micro fermé, lecture vocale arrêtée (la génération continue).
#[tauri::command]
pub async fn voice_mode_stop(voice_mode: State<'_, VoiceMode>) -> AppResult<()> {
    voice_mode.stop();
    Ok(())
}

/// Bouton Stop du bandeau « VS Code utilise le micro » (propre à ChatonLM) :
/// fin de la séance tenue par l'extension VS Code, à la demande de
/// l'utilisateur (une séance de la page continue). Sous Linux comme sous
/// Windows.
#[tauri::command]
pub async fn voice_mode_stop_editor(voice_mode: State<'_, VoiceMode>) -> AppResult<()> {
    voice_mode.stop_editor_session();
    Ok(())
}

/// Ferme ou rouvre le micro.
#[tauri::command]
pub async fn voice_mode_mute(voice_mode: State<'_, VoiceMode>, muted: bool) -> AppResult<()> {
    voice_mode.mute(muted).await.map_err(AppError::Invalid)
}

/// Responding ou Speaking : coupe la voix et la génération, puis écoute.
#[tauri::command]
pub async fn voice_mode_interrupt(voice_mode: State<'_, VoiceMode>) -> AppResult<()> {
    voice_mode.interrupt();
    Ok(())
}

/// « J'ai fini » : fin du tour tout de suite (Hearing).
#[tauri::command]
pub async fn voice_mode_end_turn(voice_mode: State<'_, VoiceMode>) -> AppResult<()> {
    voice_mode.end_turn();
    Ok(())
}

/// Push-to-talk : appui (coupe d'abord la réponse) ou relâche.
#[tauri::command]
pub async fn voice_mode_hold(voice_mode: State<'_, VoiceMode>, pressed: bool) -> AppResult<()> {
    voice_mode.hold(pressed);
    Ok(())
}

/// Paused : micro rouvert, écoute.
#[tauri::command]
pub async fn voice_mode_resume(voice_mode: State<'_, VoiceMode>) -> AppResult<()> {
    voice_mode.resume().await.map_err(AppError::Invalid)
}

/// L'envoi du tour `turn` a été refusé d'emblée par la page (`message`
/// traduit) : avis, puis écoute.
#[tauri::command]
pub async fn voice_mode_reply_failed(
    voice_mode: State<'_, VoiceMode>,
    turn: u32,
    message: String,
) -> AppResult<()> {
    voice_mode.reply_failed(turn, message);
    Ok(())
}

#[tauri::command]
pub async fn voice_mode_state(voice_mode: State<'_, VoiceMode>) -> AppResult<VoiceModeState> {
    Ok(voice_mode.state())
}

#[tauri::command]
pub async fn voice_mode_info() -> AppResult<VoiceModeInfo> {
    Ok(VoiceModeInfo::new())
}

#[tauri::command]
pub async fn get_voice_mode_settings(store: State<'_, Store>) -> AppResult<VoiceModeSettings> {
    store.call(|connection| voice_mode::get(connection)).await
}

/// Enregistre, puis applique aussitôt à une séance en cours (mode, pause,
/// interruption par la parole).
#[tauri::command]
pub async fn update_voice_mode_settings(
    store: State<'_, Store>,
    voice_mode: State<'_, VoiceMode>,
    patch: VoiceModeSettingsPatch,
) -> AppResult<VoiceModeSettings> {
    let settings = store
        .call(move |connection| voice_mode::update(connection, patch))
        .await?;
    voice_mode.settings_changed(&settings);
    Ok(settings)
}
