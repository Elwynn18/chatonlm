//! Outil Transcription (module `transcription`) : choix du fichier, fiche,
//! lancement, annulation et état de la transcription en cours (aussi par
//! l'événement `transcription-status`), historique et textes exportés
//! (`.txt`, `.md`, `.srt`, enregistrés par la page avec `save_document`),
//! synthèse (« Synthétiser », événement `transcription-synthesis`).

use tauri::{AppHandle, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::{Store, transcriptions};
use crate::transcription::paragraphs::{to_markdown, to_srt, to_text};
use crate::transcription::prepare;
use crate::transcription::synthesis::{SynthesisStatus, Synthesizer};
use crate::transcription::{
    ExportFormat, Transcription, TranscriptionDetail, TranscriptionItem, TranscriptionOptions,
    TranscriptionProbe, TranscriptionRequest, TranscriptionStatus,
};

/// Extensions proposées par le sélecteur (formats lus par `media_audio`).
const MEDIA_EXTENSIONS: &[&str] = &[
    "mp3", "m4a", "aac", "mp4", "m4v", "mov", "mkv", "webm", "mka", "ogg", "oga", "opus", "flac",
    "wav", "aif", "aiff", "caf",
];

/// Ouvre le sélecteur de fichiers natif (un fichier audio ou vidéo) ; le
/// chemin choisi est remis à la transcription. `None` si l'utilisateur
/// annule.
#[tauri::command]
pub async fn transcription_pick_file(
    window: WebviewWindow,
    transcription: State<'_, Transcription>,
) -> AppResult<Option<String>> {
    // Essais de l'app réelle : pas de dialogue à piloter, le chemin
    // « choisi » est la valeur de la variable (vide : annulé).
    #[cfg(feature = "e2e")]
    if let Some(path) = std::env::var_os("CHATONLM_E2E_PICK_MEDIA") {
        let path = path.to_string_lossy().into_owned();
        if path.is_empty() {
            return Ok(None);
        }
        transcription.grant([path.clone()]);
        return Ok(Some(path));
    }
    // Insensible à la casse comme le sélecteur des pièces jointes
    // (« COURS.MP3 » visible sous GTK).
    let patterns: Vec<String> = MEDIA_EXTENSIONS
        .iter()
        .map(|extension| super::attachments::picker_pattern(extension, cfg!(windows)))
        .collect();
    let patterns: Vec<&str> = patterns.iter().map(String::as_str).collect();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(t!("transcription.pickTitle"))
        .add_filter(t!("transcription.mediaFiles"), &patterns)
        .pick_file(move |path| {
            let _ = sender.send(path);
        });
    let Some(path) = receiver
        .await
        .ok()
        .flatten()
        .and_then(|path| path.into_path().ok())
    else {
        return Ok(None);
    };
    let path = path.to_string_lossy().into_owned();
    transcription.grant([path.clone()]);
    Ok(Some(path))
}

/// Modèles proposés, modèle et langue d'office.
#[tauri::command]
pub async fn transcription_options(
    app: AppHandle,
    store: State<'_, Store>,
) -> AppResult<TranscriptionOptions> {
    Ok(prepare(&app, &store).await?.options())
}

/// Durée annoncée d'un fichier remis, ou pourquoi il ne sera pas lu.
#[tauri::command]
pub async fn transcription_probe(
    transcription: State<'_, Transcription>,
    path: String,
) -> AppResult<TranscriptionProbe> {
    transcription.probe(path).await
}

/// Lance une transcription ; son identifiant.
#[tauri::command]
pub async fn transcription_start(
    app: AppHandle,
    store: State<'_, Store>,
    transcription: State<'_, Transcription>,
    request: TranscriptionRequest,
) -> AppResult<String> {
    let prepared = prepare(&app, &store).await?;
    transcription.start(store.inner().clone(), prepared, request)
}

/// Annule la transcription en cours.
#[tauri::command]
pub async fn transcription_cancel(transcription: State<'_, Transcription>) -> AppResult<()> {
    transcription.cancel();
    Ok(())
}

/// Transcription en cours (ou la dernière), avec tous ses passages.
#[tauri::command]
pub async fn transcription_status(
    transcription: State<'_, Transcription>,
) -> AppResult<TranscriptionStatus> {
    Ok(transcription.status())
}

#[tauri::command]
pub async fn transcription_list(store: State<'_, Store>) -> AppResult<Vec<TranscriptionItem>> {
    store
        .call(|connection| transcriptions::list(connection))
        .await
}

#[tauri::command]
pub async fn transcription_get(
    store: State<'_, Store>,
    id: String,
) -> AppResult<TranscriptionDetail> {
    store
        .call(move |connection| transcriptions::get(connection, &id))
        .await?
        .ok_or_else(|| AppError::NotFound(t!("transcription.notFound")))
}

/// Supprime une transcription de l'historique (refusé pour celle en cours ;
/// sa synthèse en cours est annulée).
#[tauri::command]
pub async fn transcription_delete(
    store: State<'_, Store>,
    transcription: State<'_, Transcription>,
    synthesizer: State<'_, Synthesizer>,
    id: String,
) -> AppResult<()> {
    if transcription.is_running(&id) {
        return Err(AppError::Invalid(t!("transcription.busy")));
    }
    synthesizer.cancel(&id);
    let deleted = store
        .call(move |connection| transcriptions::delete(connection, &id))
        .await?;
    if !deleted {
        return Err(AppError::NotFound(t!("transcription.notFound")));
    }
    Ok(())
}

#[tauri::command]
pub async fn transcription_rename(
    store: State<'_, Store>,
    id: String,
    title: String,
) -> AppResult<()> {
    if title.trim().is_empty() {
        return Err(AppError::Invalid(t!("transcription.emptyName")));
    }
    let renamed = store
        .call(move |connection| transcriptions::rename(connection, &id, &title))
        .await?;
    if !renamed {
        return Err(AppError::NotFound(t!("transcription.notFound")));
    }
    Ok(())
}

/// Texte d'une transcription au format demandé (`.md` : titre en tête ;
/// `timestamps` : horodatage des paragraphes de `.txt` et `.md`).
#[tauri::command]
pub async fn transcription_export(
    store: State<'_, Store>,
    id: String,
    format: ExportFormat,
    timestamps: bool,
) -> AppResult<String> {
    let detail = store
        .call(move |connection| transcriptions::get(connection, &id))
        .await?
        .ok_or_else(|| AppError::NotFound(t!("transcription.notFound")))?;
    Ok(match format {
        ExportFormat::Txt => to_text(&detail.segments, timestamps),
        ExportFormat::Md => to_markdown(&detail.item.title, &detail.segments, timestamps),
        ExportFormat::Srt => to_srt(&detail.segments),
    })
}

/// Lance la synthèse (« Synthétiser ») de la transcription `id` avec `model`
/// (absent : « Auto »). Refusée si une synthèse tourne déjà, si la
/// transcription n'existe pas, est en cours ou n'a pas de texte, ou si aucun
/// modèle n'est disponible ; la suite arrive par l'événement
/// `transcription-synthesis`.
#[tauri::command]
pub async fn transcription_synthesize(
    access: State<'_, crate::llm::Access>,
    transcription: State<'_, Transcription>,
    synthesizer: State<'_, Synthesizer>,
    id: String,
    model: Option<String>,
) -> AppResult<()> {
    if transcription.is_running(&id) {
        return Err(AppError::Invalid(t!("synthesis.transcribing")));
    }
    synthesizer.start(&access, id, model).await
}

/// Annule la synthèse de la transcription `id` (sans effet si elle ne
/// tourne pas).
#[tauri::command]
pub async fn transcription_synthesis_cancel(
    synthesizer: State<'_, Synthesizer>,
    id: String,
) -> AppResult<()> {
    synthesizer.cancel(&id);
    Ok(())
}

/// Synthèse en cours, ou la dernière depuis le lancement (document entier).
#[tauri::command]
pub async fn transcription_synthesis_status(
    synthesizer: State<'_, Synthesizer>,
) -> AppResult<Option<SynthesisStatus>> {
    Ok(synthesizer.status())
}
