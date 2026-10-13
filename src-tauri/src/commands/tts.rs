//! Commandes de la lecture à voix haute et des voix (fonction propre à
//! ChatonLM ; contrat `.tools/research/59-tts-contract.md`).

use tauri::State;

use crate::dictation::downloads::SpeechDownload;
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::Store;
use crate::store::chats::{self, Role};
use crate::tts::models::{self, VoiceModelInfo};
use crate::tts::{
    ReadAloud, ReadAloudOutcome, ReadAloudSource, ReadAloudState, VoiceDownloads, VoiceLanguage,
};

/// Catalogue des voix, avec celles qui sont installées (et ONNX Runtime
/// chargé la première fois, bloquant).
#[tauri::command]
pub async fn voice_models(downloads: State<'_, VoiceDownloads>) -> AppResult<Vec<VoiceModelInfo>> {
    let root = downloads.root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = crate::dictation::onnx::ensure().err();
        models::CATALOG
            .iter()
            .map(|model| model.info(models::installed(&root, model), runtime.clone()))
            .collect()
    })
    .await
    .map_err(|error| AppError::Internal(error.to_string()))
}

#[tauri::command]
pub async fn voice_downloads(
    downloads: State<'_, VoiceDownloads>,
) -> AppResult<Vec<SpeechDownload>> {
    Ok(downloads.list())
}

/// Refusé si ONNX Runtime est indisponible sur cette machine.
#[tauri::command]
pub async fn voice_download(downloads: State<'_, VoiceDownloads>, id: String) -> AppResult<()> {
    if models::find(&id).is_some() {
        let runtime = tauri::async_runtime::spawn_blocking(crate::dictation::onnx::ensure)
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
        if let Err(problem) = runtime {
            return Err(AppError::Invalid(t!(
                "readAloud.runtimeUnavailable",
                detail = problem.detail
            )));
        }
    }
    downloads.download(&id)
}

/// Arrête un téléchargement (les morceaux reçus sont gardés pour la reprise).
#[tauri::command]
pub async fn voice_cancel_download(
    downloads: State<'_, VoiceDownloads>,
    id: String,
) -> AppResult<()> {
    downloads.cancel(&id);
    Ok(())
}

/// Retire de la liste un téléchargement fini, échoué ou annulé.
#[tauri::command]
pub async fn voice_dismiss_download(
    downloads: State<'_, VoiceDownloads>,
    id: String,
) -> AppResult<()> {
    downloads.dismiss(&id);
    Ok(())
}

/// Supprime une voix (dossier entier, morceaux d'un téléchargement compris).
/// Refusé pendant son téléchargement ; la lecture qui s'en sert s'arrête et le
/// moteur est libéré d'abord.
#[tauri::command]
pub async fn voice_delete_model(
    downloads: State<'_, VoiceDownloads>,
    read_aloud: State<'_, ReadAloud>,
    id: String,
) -> AppResult<()> {
    let model = models::find(&id)
        // i18n-ignore : identifiant du catalogue, toujours connu de la page.
        .ok_or_else(|| AppError::Invalid(format!("unknown voice: {id}")))?;
    if downloads.is_active(model.id) {
        return Err(AppError::Invalid(t!("dictation.models.downloading")));
    }
    // Annulé à l'instant : sa tâche peut encore écrire un `.part`.
    if !downloads
        .wait_idle(model.id, std::time::Duration::from_secs(5))
        .await
    {
        return Err(AppError::Invalid(t!("dictation.models.stillDownloading")));
    }
    read_aloud.release_voice(model.language);
    let dir = downloads.root().join(model.id);
    match tokio::fs::remove_dir_all(&dir).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::Io(error)),
    }
}

/// Lit la réponse `message_id` de `chat_id` (texte relu en base). Voix
/// absente : `NeedsVoice`, sauf si `download` (accord donné) : la voix est
/// téléchargée puis la lecture démarre.
#[tauri::command]
pub async fn read_aloud(
    store: State<'_, Store>,
    read_aloud: State<'_, ReadAloud>,
    voices: State<'_, VoiceDownloads>,
    chat_id: String,
    message_id: i64,
    download: bool,
) -> AppResult<ReadAloudOutcome> {
    let content = store
        .call(move |connection| {
            let chat = chats::get(connection, &chat_id)?;
            Ok(chat
                .messages
                .into_iter()
                .find(|message| message.id == message_id && message.role == Role::Assistant)
                .map(|message| message.content))
        })
        .await?;
    let Some(markdown) = content else {
        return Ok(ReadAloudOutcome::NothingToRead);
    };
    let source = ReadAloudSource::Message { message_id };
    start_reading(&read_aloud, &voices, source, &markdown, None, download).await
}

/// Lit le passage `passage` du cours `course_id` (onglet Study ; texte relu
/// en base, clés dans `study::passage`), comme `read_aloud` : une seule
/// lecture dans l'app, voix absente → `NeedsVoice` sauf avec `download`. La
/// langue du cours sert de repli quand le texte ne trahit pas la sienne.
#[tauri::command]
pub async fn read_aloud_study(
    store: State<'_, Store>,
    read_aloud: State<'_, ReadAloud>,
    voices: State<'_, VoiceDownloads>,
    course_id: String,
    passage: String,
    download: bool,
) -> AppResult<ReadAloudOutcome> {
    let (course, key) = (course_id.clone(), passage.clone());
    let found = store
        .call(move |connection| crate::study::passage::load(connection, &course, &key))
        .await?;
    let Some((markdown, language)) = found else {
        return Ok(ReadAloudOutcome::NothingToRead);
    };
    let fallback = match language {
        crate::i18n::Lang::En => VoiceLanguage::En,
        crate::i18n::Lang::Es => VoiceLanguage::Es,
        crate::i18n::Lang::Fr => VoiceLanguage::Fr,
    };
    let source = ReadAloudSource::Study { course_id, passage };
    start_reading(
        &read_aloud,
        &voices,
        source,
        &markdown,
        Some(fallback),
        download,
    )
    .await
}

/// Lance la lecture de `markdown` (ONNX Runtime chargé la première fois,
/// bloquant, avant la lecture).
async fn start_reading(
    read_aloud: &ReadAloud,
    voices: &VoiceDownloads,
    source: ReadAloudSource,
    markdown: &str,
    fallback: Option<VoiceLanguage>,
    download: bool,
) -> AppResult<ReadAloudOutcome> {
    tauri::async_runtime::spawn_blocking(crate::dictation::onnx::ensure)
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?
        .ok();
    read_aloud.read(
        source,
        markdown,
        fallback,
        download,
        &|id| voices.is_active(id),
        &|id| voices.download(id),
    )
}

/// Arrête la lecture ; le téléchargement de la voix qu'elle attendait est
/// annulé (les morceaux reçus sont gardés pour la reprise).
#[tauri::command]
pub async fn read_aloud_stop(
    read_aloud: State<'_, ReadAloud>,
    voices: State<'_, VoiceDownloads>,
) -> AppResult<()> {
    if let Some(voice) = read_aloud.stop() {
        voices.cancel(voice);
    }
    Ok(())
}

#[tauri::command]
pub async fn read_aloud_state(read_aloud: State<'_, ReadAloud>) -> AppResult<ReadAloudState> {
    Ok(read_aloud.state())
}
