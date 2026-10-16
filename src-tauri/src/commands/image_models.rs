//! Commandes des modèles d'images (propre à ChatonLM ; contrat
//! `.tools/research/83-image-models-contract.md` § 2).

use tauri::State;

use crate::dictation::downloads::SpeechDownload;
use crate::error::AppResult;
use crate::image_gen::ImageGen;
use crate::image_models::types::{
    ImageMachine, ImageModelInfo, ImageRepoDetail, ImageSearchResult,
};
use crate::image_models::{ImageDownloads, ImageModels};
use crate::store::Store;

/// Installés et en cours, puis recommandations non installées.
#[tauri::command]
pub async fn image_models(
    models: State<'_, ImageModels>,
    downloads: State<'_, ImageDownloads>,
    store: State<'_, Store>,
) -> AppResult<Vec<ImageModelInfo>> {
    models.list(&downloads, &store).await
}

/// Ce que la machine permet (moteur, carte graphique, processeur, place).
#[tauri::command]
pub async fn image_machine(
    models: State<'_, ImageModels>,
    engine: State<'_, ImageGen>,
) -> AppResult<ImageMachine> {
    Ok(models.machine_info(&engine).await)
}

/// Recherche sur Hugging Face (vide : dépôts populaires).
#[tauri::command]
pub async fn image_search(
    models: State<'_, ImageModels>,
    store: State<'_, Store>,
    query: String,
) -> AppResult<ImageSearchResult> {
    models.search(&store, &query).await
}

/// Fiche d'un dépôt (« auteur/dépôt »).
#[tauri::command]
pub async fn image_repo(
    models: State<'_, ImageModels>,
    downloads: State<'_, ImageDownloads>,
    store: State<'_, Store>,
    repo: String,
) -> AppResult<ImageRepoDetail> {
    models.repo(&downloads, &store, &repo).await
}

/// Télécharge `file` de `repo` à `revision` (vide : celle du modèle
/// enregistré, sinon la dernière) avec les pièces de sa famille.
#[tauri::command]
pub async fn image_download(
    models: State<'_, ImageModels>,
    downloads: State<'_, ImageDownloads>,
    store: State<'_, Store>,
    repo: String,
    file: String,
    revision: String,
) -> AppResult<()> {
    models
        .download(&downloads, &store, &repo, &file, &revision)
        .await
}

#[tauri::command]
pub async fn image_downloads(
    downloads: State<'_, ImageDownloads>,
) -> AppResult<Vec<SpeechDownload>> {
    Ok(downloads.list())
}

/// Arrête un téléchargement (les morceaux reçus sont gardés pour la reprise).
#[tauri::command]
pub async fn image_cancel_download(
    downloads: State<'_, ImageDownloads>,
    id: String,
) -> AppResult<()> {
    downloads.cancel(&id);
    Ok(())
}

/// Retire de la liste un téléchargement fini, échoué ou annulé (un modèle
/// non installé est alors oublié).
#[tauri::command]
pub async fn image_dismiss_download(
    models: State<'_, ImageModels>,
    downloads: State<'_, ImageDownloads>,
    store: State<'_, Store>,
    id: String,
) -> AppResult<()> {
    models.dismiss(&downloads, &store, &id).await
}

/// Supprime un modèle (refusé pendant son téléchargement ; le moteur est
/// libéré s'il le charge).
#[tauri::command]
pub async fn image_delete_model(
    models: State<'_, ImageModels>,
    downloads: State<'_, ImageDownloads>,
    store: State<'_, Store>,
    engine: State<'_, ImageGen>,
    id: String,
) -> AppResult<()> {
    models.delete(&downloads, &store, Some(&engine), &id).await
}

/// Choisit le modèle de la génération (`""` : aucun ; refusé s'il n'est pas
/// installé).
#[tauri::command]
pub async fn image_use_model(
    models: State<'_, ImageModels>,
    downloads: State<'_, ImageDownloads>,
    store: State<'_, Store>,
    id: String,
) -> AppResult<()> {
    models.use_model(&downloads, &store, &id).await
}
