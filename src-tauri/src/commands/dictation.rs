//! Commandes de la dictée et des modèles de transcription (étape 17).

use tauri::{AppHandle, State};

use crate::dictation::downloads::{SpeechDownload, SpeechDownloads};
use crate::dictation::models::{self, SpeechModelInfo};
use crate::dictation::{Dictation, DictationInfo, DictationStatus};
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::Store;
use crate::store::dictation::{self, DictationPatch, DictationSettings};

#[tauri::command]
pub async fn get_dictation_settings(store: State<'_, Store>) -> AppResult<DictationSettings> {
    store.call(|connection| dictation::get(connection)).await
}

/// Enregistre puis applique (raccourci lié ou délié, modèle chargé libéré
/// s'il n'est plus réglé, statut envoyé).
#[tauri::command]
pub async fn update_dictation_settings(
    app: AppHandle,
    store: State<'_, Store>,
    dictation: State<'_, Dictation>,
    patch: DictationPatch,
) -> AppResult<DictationSettings> {
    let settings = store
        .call(move |connection| dictation::update(connection, patch))
        .await?;
    // Réglages enregistrés : un échec du raccourci se lit dans le statut.
    if let Err(error) = crate::desktop::apply(&app, &settings).await {
        eprintln!("chatonlm: dictation shortcut not applied: {error}");
    }
    dictation.settings_changed(&settings);
    dictation.emit_status(&app);
    Ok(settings)
}

#[tauri::command]
pub async fn dictation_info() -> AppResult<DictationInfo> {
    Ok(DictationInfo::new())
}

#[tauri::command]
pub async fn dictation_status(
    app: AppHandle,
    dictation: State<'_, Dictation>,
) -> AppResult<DictationStatus> {
    Ok(dictation.status(&app))
}

/// Commence ou arrête (bouton « Try dictation » des réglages).
#[tauri::command]
pub async fn dictation_toggle(app: AppHandle, dictation: State<'_, Dictation>) -> AppResult<()> {
    dictation.toggle_from_page(&app);
    Ok(())
}

#[tauri::command]
pub async fn dictation_cancel(app: AppHandle, dictation: State<'_, Dictation>) -> AppResult<()> {
    dictation.cancel(&app);
    Ok(())
}

/// Copie le texte du dernier résultat (copie automatique coupée).
#[tauri::command]
pub async fn dictation_copy_last(app: AppHandle, dictation: State<'_, Dictation>) -> AppResult<()> {
    dictation.copy_last(&app).await
}

/// Nouvel essai de liaison du raccourci (après un refus ou une erreur).
#[tauri::command]
pub async fn dictation_bind_shortcut(app: AppHandle) -> AppResult<()> {
    crate::desktop::bind_shortcut(&app).await
}

/// Ouvre la boîte du bureau qui choisit la touche (portail version 2).
#[tauri::command]
pub async fn dictation_configure_shortcut(app: AppHandle) -> AppResult<()> {
    crate::desktop::configure_shortcut(&app).await
}

/// Catalogue, avec les modèles installés (tous leurs fichiers à la bonne
/// taille).
#[tauri::command]
pub async fn speech_models(
    downloads: State<'_, SpeechDownloads>,
) -> AppResult<Vec<SpeechModelInfo>> {
    let root = downloads.root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        models::CATALOG
            .iter()
            .map(|model| model.info(models::installed(&root, model)))
            .collect()
    })
    .await
    .map_err(|error| AppError::Internal(error.to_string()))
}

#[tauri::command]
pub async fn speech_downloads(
    downloads: State<'_, SpeechDownloads>,
) -> AppResult<Vec<SpeechDownload>> {
    Ok(downloads.list())
}

/// Refusé pour un modèle que cette machine ne peut pas faire tourner
/// (Whisper sans AVX2..., Parakeet sans ONNX Runtime).
#[tauri::command]
pub async fn speech_download(downloads: State<'_, SpeechDownloads>, id: String) -> AppResult<()> {
    if let Some(model) = models::find(&id) {
        let engine = model.engine;
        let support =
            tauri::async_runtime::spawn_blocking(move || crate::dictation::engine_support(engine))
                .await
                .map_err(|error| AppError::Internal(error.to_string()))?;
        refuse_unsupported(support)?;
    }
    downloads.download(&id)
}

fn refuse_unsupported(support: Result<(), String>) -> AppResult<()> {
    support.map_err(AppError::Invalid)
}

/// Arrête un téléchargement (les morceaux reçus sont gardés pour la reprise).
#[tauri::command]
pub async fn speech_cancel_download(
    downloads: State<'_, SpeechDownloads>,
    id: String,
) -> AppResult<()> {
    downloads.cancel(&id);
    Ok(())
}

/// Retire de la liste un téléchargement fini, échoué ou annulé.
#[tauri::command]
pub async fn speech_dismiss_download(
    downloads: State<'_, SpeechDownloads>,
    id: String,
) -> AppResult<()> {
    downloads.dismiss(&id);
    Ok(())
}

/// Supprime un modèle (dossier entier, morceaux d'un téléchargement compris).
/// Refusé pendant son téléchargement ; libéré d'abord s'il est chargé.
#[tauri::command]
pub async fn speech_delete_model(
    app: AppHandle,
    downloads: State<'_, SpeechDownloads>,
    dictation: State<'_, Dictation>,
    id: String,
) -> AppResult<()> {
    delete_model(&downloads, &dictation, &id).await?;
    dictation.emit_status(&app);
    Ok(())
}

async fn delete_model(
    downloads: &SpeechDownloads,
    dictation: &Dictation,
    id: &str,
) -> AppResult<()> {
    let model = models::find(id)
        // i18n-ignore : identifiant du catalogue, toujours connu de la page.
        .ok_or_else(|| AppError::Invalid(format!("unknown speech model: {id}")))?;
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
    dictation.release_model(model.id).await;
    let dir = downloads.root().join(model.id);
    match tokio::fs::remove_dir_all(&dir).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::Io(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_models_are_not_downloaded() {
        assert!(refuse_unsupported(Ok(())).is_ok());
        let refusal = crate::dictation::cpu::unsupported(&["AVX2"]);
        let Err(AppError::Invalid(message)) = refuse_unsupported(Err(refusal.clone())) else {
            panic!("accepted");
        };
        assert_eq!(message, refusal);
    }

    #[tokio::test]
    async fn deleting_a_model_removes_its_folder_but_not_during_its_download() {
        // Serveur qui accepte sans jamais répondre : le téléchargement reste
        // en cours.
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let downloads = SpeechDownloads::with_options(
            |_| {},
            root.path().to_path_buf(),
            crate::dictation::downloads::Options {
                base_url: Some(format!("http://{}", silent.local_addr().unwrap())),
                ..Default::default()
            },
        );
        let dictation = Dictation::new();
        let dir = root.path().join("whisper-tiny");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ggml-tiny.bin.part"), b"partial").unwrap();

        downloads.download("whisper-tiny").unwrap();
        let refused = delete_model(&downloads, &dictation, "whisper-tiny").await;
        assert!(matches!(refused, Err(AppError::Invalid(_))));
        assert!(dir.exists());

        downloads.cancel("whisper-tiny");
        delete_model(&downloads, &dictation, "whisper-tiny")
            .await
            .unwrap();
        assert!(!dir.exists());
        // Annulé à l'instant : la suppression attend la vraie fin de sa
        // tâche (qui pourrait encore écrire un `.part`).
        std::fs::create_dir_all(&dir).unwrap();
        downloads.simulate_task("whisper-tiny");
        let ending = std::sync::Arc::new(downloads);
        let ender = ending.clone();
        let started = std::time::Instant::now();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            ender.end_task("whisper-tiny");
        });
        delete_model(&ending, &dictation, "whisper-tiny")
            .await
            .unwrap();
        assert!(started.elapsed() >= std::time::Duration::from_millis(300));
        assert!(!dir.exists());
        let downloads = ending;
        // Déjà absent : rien à faire ; inconnu : refusé.
        delete_model(&downloads, &dictation, "whisper-tiny")
            .await
            .unwrap();
        assert!(matches!(
            delete_model(&downloads, &dictation, "nope").await,
            Err(AppError::Invalid(_))
        ));
    }
}
