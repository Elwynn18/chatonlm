//! Commandes du ModelHub (`src/lib/ipc.ts`). Contrat :
//! `.tools/research/37-step15a-contract.md`.

use tauri::{AppHandle, State};

use crate::error::AppResult;
use crate::hub::downloads::HubDownloads;
use crate::hub::hardware::Machine;
use crate::hub::installed;
use crate::hub::state::Catalog;
use crate::hub::types::{
    HubDownload, HubDownloadRequest, HubFilters, HubHardware, HubHome, HubInstalled, HubList,
    HubReadme, HubRepoResult, HubScope, HubSearch,
};
use crate::store::{Store, settings};

/// Machine de l'app, avec le contexte réglé (le moteur tourne toujours sur
/// cet ordinateur) ; `None` si elle ne se lit pas.
pub(crate) async fn machine(catalog: &Catalog, store: &Store) -> AppResult<Option<HubHardware>> {
    Ok(machine_and_uncensored(catalog, store).await?.0)
}

/// Machine de l'app (voir [`machine`]) et réglage « Show uncensored
/// models », lus ensemble.
async fn machine_and_uncensored(
    catalog: &Catalog,
    store: &Store,
) -> AppResult<(Option<HubHardware>, bool)> {
    let settings = store.call(|c| settings::get(c)).await?;
    let hardware = catalog.hardware(settings.context_length).await;
    Ok((hardware, settings.show_uncensored))
}

/// Accueil : rubriques et familles, filtrées ; « Uncensored » selon le
/// réglage.
#[tauri::command]
pub async fn hub_home(
    catalog: State<'_, Catalog>,
    store: State<'_, Store>,
    filters: HubFilters,
) -> AppResult<HubHome> {
    let (hardware, uncensored) = machine_and_uncensored(&catalog, &store).await?;
    Ok(catalog.home(&filters, hardware.as_ref(), uncensored).await)
}

/// Liste complète d'une rubrique ou d'une famille, filtrée.
#[tauri::command]
pub async fn hub_browse(
    catalog: State<'_, Catalog>,
    store: State<'_, Store>,
    scope: HubScope,
    filters: HubFilters,
) -> AppResult<HubList> {
    let (hardware, uncensored) = machine_and_uncensored(&catalog, &store).await?;
    Ok(catalog
        .browse(&scope, &filters, hardware.as_ref(), uncensored)
        .await)
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

/// Mémoire de la machine ; `None` si elle ne se lit pas.
#[tauri::command]
pub async fn hub_hardware(
    catalog: State<'_, Catalog>,
    store: State<'_, Store>,
) -> AppResult<Option<HubHardware>> {
    machine(&catalog, &store).await
}

/// Mémoire de la machine pour la jauge de VRAM des réglages (propre à
/// ChatonLM), détectée depuis au plus une seconde ; `None` si elle ne se lit
/// pas.
#[tauri::command]
pub async fn machine_memory(catalog: State<'_, Catalog>) -> AppResult<Option<Machine>> {
    Ok(catalog.memory().await)
}

/// Lance (ou met en file) un téléchargement depuis Hugging Face, vers la
/// base locale des modèles ; sans effet s'il est déjà en cours ou en file ;
/// identifiant invalide : refus.
#[tauri::command]
pub async fn hub_download(
    downloads: State<'_, HubDownloads>,
    request: HubDownloadRequest,
) -> AppResult<()> {
    downloads.download(request)
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

/// Met en pause un téléchargement en file ou en cours.
#[tauri::command]
pub async fn hub_pause_download(
    downloads: State<'_, HubDownloads>,
    model: String,
) -> AppResult<()> {
    downloads.pause(&model);
    Ok(())
}

/// Reprend un téléchargement en pause.
#[tauri::command]
pub async fn hub_resume_download(
    downloads: State<'_, HubDownloads>,
    model: String,
) -> AppResult<()> {
    downloads.resume(&model)
}

/// Fait passer un téléchargement en premier : il démarre tout de suite.
#[tauri::command]
pub async fn hub_prioritize_download(
    downloads: State<'_, HubDownloads>,
    model: String,
) -> AppResult<()> {
    downloads.prioritize(&model)
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

/// Modèles de la base locale, du plus récent au plus ancien.
#[tauri::command]
pub async fn hub_installed(store: State<'_, Store>) -> AppResult<Vec<HubInstalled>> {
    installed::list(&store).await
}

/// Supprime un modèle de la base locale et ses fichiers (sauf ceux qu'un
/// autre modèle partage ou qu'un téléchargement en cours a réservés), après
/// l'arrêt de son processus dans le moteur (sous Windows, un fichier ouvert
/// ne s'efface pas).
#[tauri::command]
pub async fn hub_delete_model(
    app: AppHandle,
    store: State<'_, Store>,
    downloads: State<'_, HubDownloads>,
    engine: State<'_, crate::engine::Engine>,
    model: String,
) -> AppResult<()> {
    let data_dir = crate::paths::data_dir(&app)?;
    delete_model(&engine, &store, &data_dir, &model, &downloads.reserved()).await
}

/// Arrête le processus du modèle, puis le supprime avec ses journaux du
/// moteur (relecture L5a, M3).
async fn delete_model(
    engine: &crate::engine::Engine,
    store: &Store,
    data_dir: &std::path::Path,
    model: &str,
    reserved: &crate::local_models::Reserved,
) -> AppResult<()> {
    let model = model.trim();
    // Identifiant tel que le moteur a nommé ses journaux (celui de la base).
    let canonical = crate::local_models::get(store, data_dir, model)
        .await
        .ok()
        .flatten()
        .map_or_else(|| model.to_owned(), |local| local.id);
    unload_then(engine, model, async {
        installed::delete(store, data_dir, model, reserved).await?;
        engine.remove_logs(&canonical);
        Ok(())
    })
    .await
}

/// Arrête le processus de `id` (attendu fini), PUIS lance `delete` ; aucune
/// demande ne le relance entre les deux (`Engine::deleting`).
async fn unload_then<T>(
    engine: &crate::engine::Engine,
    id: &str,
    delete: impl std::future::Future<Output = T>,
) -> T {
    let _deleting = engine.deleting(id);
    engine.unload(id).await;
    delete.await
}

#[cfg(test)]
mod engine_tests {
    use super::*;

    /// Lot L3 : le processus du modèle est arrêté avant la suppression (sous
    /// Windows, un fichier ouvert ne s'efface pas), même si elle échoue
    /// ensuite (modèle absent de la base).
    #[tokio::test(flavor = "multi_thread")]
    async fn the_engine_lets_go_of_a_model_before_its_deletion() {
        let bench = crate::engine::tests::bench();
        let id = bench.install("simple", 1 << 30);
        bench
            .engine
            .preload(&id, crate::engine::LoadOptions::default())
            .await
            .unwrap();
        let store = Store::open_in_memory().unwrap();
        let data = tempfile::tempdir().unwrap();
        let reserved = crate::local_models::Reserved::default();
        let _ = delete_model(
            &bench.engine,
            &store,
            data.path(),
            &format!(" {id} "),
            &reserved,
        )
        .await;
        assert!(bench.engine.running().is_empty());
        bench.engine.shutdown().await;
    }

    /// Relecture L5a, M3 : les journaux du moteur d'un modèle supprimé
    /// partent avec lui (`.1` compris) ; ceux d'un autre restent.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_logs_of_a_deleted_model_go_with_it() {
        const GONE: &str = "test/Gone-GGUF:Q4_K_M";
        const KEPT: &str = "test/Kept-GGUF:Q4_K_M";
        let bench = crate::llm::testing::FakeBench::new(&[(GONE, "simple"), (KEPT, "simple")]);
        let engine = bench.engine();
        for id in [GONE, KEPT] {
            engine
                .preload(id, crate::engine::LoadOptions::default())
                .await
                .unwrap();
        }
        let log_of = |id: &str| {
            engine
                .logs_dir()
                .join(crate::engine::journal::file_name(id))
        };
        let old = log_of(GONE).with_extension("log.1");
        std::fs::write(&old, "previous run").unwrap();
        assert!(log_of(GONE).is_file() && log_of(KEPT).is_file());
        delete_model(
            engine,
            bench.store(),
            &bench.dir().join("data"),
            &format!(" {GONE} "),
            &bench.access.downloads().reserved(),
        )
        .await
        .unwrap();
        assert!(!log_of(GONE).exists());
        assert!(!old.exists());
        assert!(log_of(KEPT).is_file());
        bench.shutdown().await;
    }

    /// Le processus est fini AVANT que le fichier parte (le faux note, à son
    /// arrêt, un fichier déjà effacé), et rien ne relance le modèle pendant
    /// l'effacement.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_model_stops_before_its_file_goes_and_stays_stopped() {
        let bench = crate::engine::tests::bench();
        let id = bench.install("simple", 1 << 30);
        let path = bench.model_path(&id);
        bench
            .engine
            .preload(&id, crate::engine::LoadOptions::default())
            .await
            .unwrap();
        let engine = bench.engine.clone();
        let relaunch = unload_then(&bench.engine, &id, async {
            let relaunch = engine
                .ensure(&id, crate::engine::LoadOptions::default())
                .await
                .map(drop);
            std::fs::remove_file(&path).unwrap();
            relaunch
        })
        .await;
        assert_eq!(
            relaunch.unwrap_err().code(),
            "notInstalled",
            "no relaunch while deleting"
        );
        assert!(bench.engine.running().is_empty());
        let log = std::fs::read_to_string(bench.log_of(&id)).unwrap();
        assert!(log.contains("stopping on a termination signal"), "{log}");
        assert!(!log.contains(fake_llama::cli::DELETED_BEFORE_STOP), "{log}");
        bench.engine.shutdown().await;
    }
}
