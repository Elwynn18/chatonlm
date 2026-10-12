//! Commandes du ModelHub (`src/lib/ipc.ts`). Contrat :
//! `.tools/research/37-step15a-contract.md`.

use tauri::State;

use super::models::server_url;
use crate::error::AppResult;
use crate::hub::downloads::HubDownloads;
use crate::hub::installed;
use crate::hub::state::Catalog;
use crate::hub::types::{
    HubDownload, HubDownloadRequest, HubFilters, HubHardware, HubHome, HubInstalled, HubList,
    HubReadme, HubRepoResult, HubScope, HubSearch,
};
use crate::store::Store;

/// Machine du serveur qui sert (cible, contexte réglé) ; `None` pour un
/// serveur distant. Sans attendre l'intégré : seul compte que l'adresse soit
/// locale (le catalogue de Hugging Face ne dépend pas d'Ollama).
async fn machine(catalog: &Catalog, store: &Store) -> AppResult<Option<HubHardware>> {
    let settings = crate::ollama::target::display_settings(store).await?;
    Ok(catalog
        .hardware(&settings.effective_server_url, settings.context_length)
        .await)
}

/// Accueil : rubriques et familles, filtrées.
#[tauri::command]
pub async fn hub_home(
    catalog: State<'_, Catalog>,
    store: State<'_, Store>,
    filters: HubFilters,
) -> AppResult<HubHome> {
    let hardware = machine(&catalog, &store).await?;
    Ok(catalog.home(&filters, hardware.as_ref()).await)
}

/// Liste complète d'une rubrique ou d'une famille, filtrée.
#[tauri::command]
pub async fn hub_browse(
    catalog: State<'_, Catalog>,
    store: State<'_, Store>,
    scope: HubScope,
    filters: HubFilters,
) -> AppResult<HubList> {
    let hardware = machine(&catalog, &store).await?;
    Ok(catalog.browse(&scope, &filters, hardware.as_ref()).await)
}

/// Recherche libre sur Hugging Face, page par page.
#[tauri::command]
pub async fn hub_search(
    catalog: State<'_, Catalog>,
    store: State<'_, Store>,
    search: HubSearch,
) -> AppResult<HubList> {
    let hardware = machine(&catalog, &store).await?;
    Ok(catalog.search(&search, hardware.as_ref()).await)
}

/// Fiche d'un dépôt (`id` = « auteur/dépôt »).
#[tauri::command]
pub async fn hub_repo(
    catalog: State<'_, Catalog>,
    store: State<'_, Store>,
    id: String,
) -> AppResult<HubRepoResult> {
    let hardware = machine(&catalog, &store).await?;
    Ok(catalog.repo(&id, hardware.as_ref()).await)
}

/// README d'un dépôt à une révision (`sha` de la fiche, sinon « main »).
#[tauri::command]
pub async fn hub_readme(
    catalog: State<'_, Catalog>,
    id: String,
    revision: String,
) -> AppResult<HubReadme> {
    Ok(catalog.readme(&id, &revision).await)
}

/// Mémoire de la machine du serveur ; `None` pour un serveur distant.
#[tauri::command]
pub async fn hub_hardware(
    catalog: State<'_, Catalog>,
    store: State<'_, Store>,
) -> AppResult<Option<HubHardware>> {
    machine(&catalog, &store).await
}

/// Lance (ou met en file) un téléchargement depuis le serveur réglé ; sans
/// effet s'il est déjà en cours ou en file ; nom invalide : refus.
#[tauri::command]
pub async fn hub_download(
    store: State<'_, Store>,
    downloads: State<'_, HubDownloads>,
    request: HubDownloadRequest,
) -> AppResult<()> {
    let server = server_url(&store).await?;
    downloads.download(&server, request)
}

/// Arrête un téléchargement en cours ou en file.
#[tauri::command]
pub async fn hub_cancel_download(
    downloads: State<'_, HubDownloads>,
    model: String,
) -> AppResult<()> {
    downloads.cancel(&model);
    Ok(())
}

/// Téléchargements en cours, en file et terminés (jusqu'à `hub_dismiss_download`).
#[tauri::command]
pub async fn hub_downloads(downloads: State<'_, HubDownloads>) -> AppResult<Vec<HubDownload>> {
    Ok(downloads.list())
}

/// Retire de la liste un téléchargement terminé, échoué ou arrêté.
#[tauri::command]
pub async fn hub_dismiss_download(
    downloads: State<'_, HubDownloads>,
    model: String,
) -> AppResult<()> {
    downloads.dismiss(&model);
    Ok(())
}

/// Tous les modèles installés sur le serveur (embeddings compris), du plus
/// récent au plus ancien.
#[tauri::command]
pub async fn hub_installed(store: State<'_, Store>) -> AppResult<Vec<HubInstalled>> {
    installed::list(&server_url(&store).await?).await
}

/// Supprime un modèle du serveur (`DELETE /api/delete`).
#[tauri::command]
pub async fn hub_delete_model(store: State<'_, Store>, model: String) -> AppResult<()> {
    installed::delete(&server_url(&store).await?, &model).await
}
