use std::sync::LazyLock;

use futures_util::{StreamExt, stream};
use serde::Deserialize;
use tauri::State;

use crate::chat::stream::Think;
use crate::engine::{Engine, RunningModel as EngineRunningModel};
use crate::error::{AppError, AppResult};
use crate::hub::state::Catalog;
use crate::hub::types::HubHardware;
use crate::llm::Access;
use crate::llm::models::{self as facade, ChatModel, ModelDetails, RecommendedModel};
use crate::ollama::registry;
use crate::store::settings::{self, Settings};
use crate::store::{Store, model_prefs};

/// Modèles du sélecteur : installés (du plus récent), puis distants si le
/// réglage les permet.
#[tauri::command]
pub async fn list_models(access: State<'_, Access>) -> AppResult<Vec<ChatModel>> {
    facade::list(&access).await
}

/// Capacités et réglages de la réflexion d'un modèle.
#[tauri::command]
pub async fn model_details(access: State<'_, Access>, model: String) -> AppResult<ModelDetails> {
    facade::details(&access, &model).await
}

/// Modèle recommandé de `hub/curation.json` (`recommended`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Recommendation {
    /// Identifiant Hugging Face (`<auteur>/<dépôt>:<QUANT>`).
    id: String,
    display_name: String,
    /// Poids et projecteur, s'ils sont connus.
    size_bytes: Option<u64>,
    /// Paramètres (estimation du cache de contexte et, sans `size_bytes`,
    /// de la taille).
    parameters: u64,
    /// Architecture GGUF (part du cache de contexte, `hub::fit`).
    arch: String,
    vision: bool,
}

#[derive(Deserialize)]
struct RecommendedList {
    recommended: Vec<Recommendation>,
}

/// Liste embarquée, lue une fois.
static RECOMMENDED: LazyLock<Vec<Recommendation>> = LazyLock::new(|| {
    serde_json::from_str::<RecommendedList>(include_str!("../hub/curation.json"))
        .expect("valid recommended models in curation.json")
        .recommended
});

/// Contexte d'une estimation sans machine lue (celui du moteur sans carte).
const DEFAULT_CONTEXT: u32 = 4096;

/// Recommandés avec la mémoire qu'il leur faut au chargement (poids, cache
/// de contexte au contexte de la machine ou réglé, marge : `hub::fit`) et
/// leur place dans la machine ; sans machine lue, mémoire au contexte réglé
/// (sinon 4 096) et aucune pastille.
fn recommendations(hardware: Option<&HubHardware>, context_setting: u32) -> Vec<RecommendedModel> {
    let machine = hardware.map_or(
        if context_setting > 0 {
            context_setting
        } else {
            DEFAULT_CONTEXT
        },
        |hardware| hardware.context,
    );
    let models = RECOMMENDED
        .iter()
        .map(|entry| {
            let weights = entry
                .size_bytes
                .or_else(|| crate::hub::fit::list_size(Some(entry.parameters), entry.vision));
            let context = crate::hub::fit::model_context(machine, None, entry.vision);
            let vram_bytes = weights.map(|weights| {
                let kv = crate::hub::fit::kv_cache(
                    None,
                    Some(entry.parameters),
                    Some(&entry.arch),
                    context,
                );
                crate::hub::fit::memory(weights, kv, context)
            });
            RecommendedModel {
                model: entry.id.clone(),
                display_name: entry.display_name.clone(),
                size_bytes: entry.size_bytes,
                vram_bytes,
                fit: None,
            }
        })
        .collect();
    with_fit(models, hardware)
}

/// Modèles recommandés (tête du sélecteur, premier modèle), avec la mémoire
/// qu'il leur faut et leur place dans cette machine.
#[tauri::command]
pub async fn recommended_models(
    catalog: State<'_, Catalog>,
    store: State<'_, Store>,
) -> AppResult<Vec<RecommendedModel>> {
    let hardware = crate::commands::hub::machine(&catalog, &store).await?;
    let context = store.call(|c| settings::get(c, None)).await?.context_length;
    Ok(recommendations(hardware.as_ref(), context))
}

/// Pastille de chaque modèle recommandé dont la mémoire est connue.
fn with_fit(
    mut models: Vec<RecommendedModel>,
    hardware: Option<&crate::hub::types::HubHardware>,
) -> Vec<RecommendedModel> {
    for model in &mut models {
        model.fit = match (model.vram_bytes, hardware) {
            (Some(need), Some(hardware)) => Some(crate::hub::fit::level(need, hardware)),
            _ => None,
        };
    }
    models
}

/// Vrai si le modèle existe au registre d'Ollama (retirée de la page ; code
/// gardé jusqu'au lot L10).
#[tauri::command]
pub async fn model_exists_upstream(model: String) -> AppResult<bool> {
    registry::model_exists(&registry::registry_url(), &model).await
}

/// Parmi `models`, les modèles installés dont Hugging Face a une version plus
/// récente, dans l'ordre reçu ; un modèle inconnu, repris d'Ollama ou dont
/// la fiche ne se lit pas (hors ligne) n'en est pas.
#[tauri::command]
pub async fn outdated_models(
    store: State<'_, Store>,
    models: Vec<String>,
) -> AppResult<Vec<String>> {
    outdated_in(&store, &crate::hub::names::hf_url(), models).await
}

/// Fiches lues en même temps, au plus (onglet Installed : tous les modèles).
const OUTDATED_AT_ONCE: usize = 4;

/// Corps de [`outdated_models`] contre l'adresse de Hugging Face `hf_url`.
pub(crate) async fn outdated_in(
    store: &Store,
    hf_url: &str,
    models: Vec<String>,
) -> AppResult<Vec<String>> {
    let rows = store
        .call(move |connection| {
            models
                .into_iter()
                .map(|id| {
                    let row = crate::store::local_models::get(connection, &id)?;
                    Ok((id, row))
                })
                .collect::<AppResult<Vec<_>>>()
        })
        .await?;
    let checked: Vec<Option<String>> = stream::iter(rows)
        .map(|(id, row)| async move {
            let row = row?;
            (crate::local_models::check_outdated(hf_url, &row).await == Some(true)).then_some(id)
        })
        .buffered(OUTDATED_AT_ONCE)
        .collect()
        .await;
    Ok(checked.into_iter().flatten().collect())
}

/// Réglage du raisonnement mémorisé pour ce modèle (`null` : aucun).
#[tauri::command]
pub async fn get_think_pref(store: State<'_, Store>, model: String) -> AppResult<Option<Think>> {
    store
        .call(move |connection| model_prefs::think(connection, &model))
        .await
}

/// Mémorise le réglage du raisonnement choisi pour ce modèle.
#[tauri::command]
pub async fn set_think_pref(store: State<'_, Store>, model: String, think: Think) -> AppResult<()> {
    store
        .call(move |connection| model_prefs::set_think(connection, &model, &think))
        .await
}

/// Modèles du moteur de l'app (processus `llama-server`), du plus récemment
/// servi au plus ancien.
#[tauri::command]
pub async fn running_models(engine: State<'_, Engine>) -> AppResult<Vec<EngineRunningModel>> {
    Ok(engine.running())
}

/// Modèle que prend « Auto » (pages Proofread et Study, rien de choisi) en ce
/// moment : le meilleur modèle prêt dans le moteur, sinon celui du chat ;
/// `None` si aucun.
#[tauri::command]
pub async fn auto_model(
    app: tauri::AppHandle,
    store: State<'_, Store>,
    engine: State<'_, Engine>,
) -> AppResult<Option<String>> {
    let data_dir = crate::paths::data_dir(&app)?;
    let installed = crate::local_models::list(&store, &data_dir).await?;
    let settings = store.call(|c| settings::get(c, None)).await?;
    Ok(crate::engine::auto::model_of(
        &engine.running(),
        &installed,
        "",
        &settings.selected_model,
    ))
}

/// Charge un modèle avec le contexte et la durée des réglages ; rend la main
/// une fois chargé (bouton Load, préchargement du modèle choisi).
/// Le chef de la carte graphique replanifie aussitôt (lien VS Code).
#[tauri::command]
pub async fn load_model(
    access: State<'_, Access>,
    gpu: State<'_, crate::gpu::GpuChief>,
    model: String,
) -> AppResult<()> {
    let loaded = load_in_engine(&access, &model).await;
    gpu.poke();
    loaded
}

/// Décharge un modèle (attend la fin de son processus).
#[tauri::command]
pub async fn unload_model(
    access: State<'_, Access>,
    gpu: State<'_, crate::gpu::GpuChief>,
    model: String,
) -> AppResult<()> {
    let working = crate::gpu::activity::hold();
    facade::unload(&access, &model).await;
    drop(working);
    gpu.poke();
    Ok(())
}

/// Décharge tous les modèles du moteur.
#[tauri::command]
pub async fn unload_all_models(
    access: State<'_, Access>,
    gpu: State<'_, crate::gpu::GpuChief>,
) -> AppResult<()> {
    let working = crate::gpu::activity::hold();
    facade::unload_all(&access).await;
    drop(working);
    gpu.poke();
    Ok(())
}

/// Adresse du serveur auquel parler : celle de la cible (`ollama::target` :
/// Ollama intégré, sinon adresse réglée, `OLLAMA_HOST` ou défaut). Transition
/// (lots L5a, L9).
pub(crate) async fn server_url(store: &Store) -> AppResult<String> {
    Ok(app_settings(store).await?.effective_server_url)
}

/// Réglages de l'app, avec l'adresse de la cible (point unique de toutes les
/// requêtes à Ollama). Transition (lots L5a, L9).
pub(crate) async fn app_settings(store: &Store) -> AppResult<Settings> {
    crate::ollama::target::request_settings(store).await
}

/// Chargement avec les options d'un envoi du chat (contexte et durée
/// réglés : le message suivant ne relance pas le modèle), dans le moteur de
/// l'app (bouton Load, préchargement). Un chargement arrêté par un
/// déchargement demandé entre-temps (Load puis Unload), ou par la fin de
/// l'app, est une annulation : rien à montrer.
pub(crate) async fn load_in_engine(access: &Access, model: &str) -> AppResult<()> {
    // L'autocomplétion de l'éditeur attend pendant ce temps.
    let _working = crate::gpu::activity::hold();
    match facade::load(access, model).await {
        Ok(()) => Ok(()),
        Err(error) if error.chat_kind().is_none() => Ok(()),
        Err(error) => Err(AppError::from(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::settings::SettingsPatch;

    #[test]
    fn a_recommended_model_gets_the_fit_of_the_machine() {
        use crate::hub::types::{HubFit, HubGpu, HubHardware};
        const GIB: u64 = 1024 * 1024 * 1024;
        let model = |name: &str, vram_bytes: Option<u64>| RecommendedModel {
            model: name.into(),
            display_name: name.into(),
            size_bytes: None,
            vram_bytes,
            fit: None,
        };
        let models = || {
            vec![
                model("small", Some(4 * GIB)),
                model("large", Some(200 * GIB)),
                model("unknown", None),
            ]
        };
        let hardware = HubHardware {
            gpus: vec![HubGpu {
                name: "GPU".into(),
                vram_total: 24 * GIB,
                vram_free: None,
            }],
            ram_total: 32 * GIB,
            ram_available: 16 * GIB,
            context: 4096,
        };
        let fits: Vec<_> = with_fit(models(), Some(&hardware))
            .into_iter()
            .map(|m| m.fit)
            .collect();
        assert_eq!(fits, [Some(HubFit::Fits), Some(HubFit::TooLarge), None]);
        // Serveur distant : aucune pastille.
        assert!(with_fit(models(), None).iter().all(|m| m.fit.is_none()));
    }

    /// Liste embarquée : identifiants Hugging Face valides et uniques, la
    /// mémoire de chacun (taille connue ou estimée) au contexte de la
    /// machine, plus grande que ses poids, et sa place dans la machine.
    #[test]
    fn the_recommended_models_have_valid_ids_and_their_need() {
        use crate::hub::types::{HubFit, HubGpu};
        use crate::local_models::id::ModelId;
        const GIB: u64 = 1024 * 1024 * 1024;
        let small = HubHardware {
            gpus: vec![HubGpu {
                name: "GPU".into(),
                vram_total: 8 * GIB,
                vram_free: None,
            }],
            ram_total: 16 * GIB,
            ram_available: 8 * GIB,
            context: 4096,
        };
        let models = recommendations(Some(&small), 0);
        assert!(models.len() >= 3);
        for (index, model) in models.iter().enumerate() {
            let id = ModelId::parse(&model.model).expect("a valid id");
            assert!(id.repo_id().is_some(), "{}", model.model);
            assert!(
                models[..index]
                    .iter()
                    .all(|other| other.model != model.model),
                "{} twice",
                model.model
            );
            let need = model.vram_bytes.expect("a need");
            assert!(need > model.size_bytes.unwrap_or(0), "{}", model.model);
            assert!(model.fit.is_some());
        }
        // Le premier : Qwen3.5 9B, comme la recommandation d'Ollama.
        assert_eq!(models[0].model, "unsloth/Qwen3.5-9B-GGUF:Q4_K_M");
        assert_eq!(models[0].display_name, "Qwen3.5-9B");
        // gpt-oss-20b ne tient pas dans 8 Gio de VRAM, il déborde.
        let gpt = models.iter().find(|m| m.model.contains("gpt-oss")).unwrap();
        assert_eq!(gpt.fit, Some(HubFit::Partial));
        // Contexte plus grand (machine plus grosse) : plus de mémoire.
        let big = HubHardware {
            context: 32_768,
            ..small.clone()
        };
        let larger = recommendations(Some(&big), 0);
        assert!(larger[0].vram_bytes > models[0].vram_bytes);
        // Machine illisible : mémoire au contexte réglé, aucune pastille.
        let unread = recommendations(None, 32_768);
        assert_eq!(unread[0].vram_bytes, larger[0].vram_bytes);
        assert!(unread.iter().all(|m| m.fit.is_none()));
        assert_eq!(recommendations(None, 0)[0].vram_bytes, models[0].vram_bytes);
    }

    /// Modèle installé depuis Hugging Face : périmé une fois une nouvelle
    /// version publiée ; un identifiant inconnu ou une fiche injoignable
    /// n'en est pas ; l'ordre reçu est gardé.
    #[tokio::test]
    async fn outdated_models_follow_hugging_face() {
        use crate::hub::downloads::{HubDownloads, Options};
        use crate::hub::types::{HubDownloadRequest, HubDownloadState};
        use fake_ollama::hf::{FakeHf, HfMode};
        let fake = FakeHf::start(HfMode::Normal).await.unwrap();
        let store = Store::open_in_memory().unwrap();
        let data = tempfile::tempdir().unwrap();
        let downloads = HubDownloads::with_options(
            |_| {},
            Options {
                hf_url: Some(fake.url()),
                ..Options::default()
            },
        );
        downloads.attach(store.clone(), data.path().to_owned());
        let id = "chatonlm-dl/Fresh-vision-GGUF:Q4_K_M";
        downloads
            .download(HubDownloadRequest {
                model: id.into(),
                repo: None,
                label: None,
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while downloads.list()[0].state != HubDownloadState::Done {
            assert!(
                std::time::Instant::now() < deadline,
                "{:?}",
                downloads.list()
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let asked = || vec!["a/Unknown-GGUF:Q4_K_M".to_owned(), id.to_owned()];
        assert!(
            outdated_in(&store, &fake.url(), asked())
                .await
                .unwrap()
                .is_empty()
        );
        fake.publish("chatonlm-dl/Fresh-vision-GGUF");
        assert_eq!(
            outdated_in(&store, &fake.url(), asked()).await.unwrap(),
            [id]
        );
        let url = fake.url();
        fake.stop().await;
        assert!(outdated_in(&store, &url, asked()).await.unwrap().is_empty());
    }

    /// Load puis Unload pendant le chargement : le Load finit sans erreur
    /// (une annulation, rien à montrer), et rien ne reste dans le moteur.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_load_stopped_by_an_unload_is_a_cancellation() {
        let id = "test/Slow-GGUF:Q4_K_M";
        let bench = crate::llm::testing::FakeBench::new(&[(id, "slowload")]);
        let access = bench.access.clone();
        let load = tokio::spawn(async move { load_in_engine(&access, id).await });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !bench.engine().running().iter().any(|model| model.id == id) {
            assert!(std::time::Instant::now() < deadline, "never loading");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        facade::unload(&bench.access, id).await;
        let loaded = tokio::time::timeout(std::time::Duration::from_secs(10), load)
            .await
            .expect("the load ends")
            .unwrap();
        assert!(loaded.is_ok(), "{loaded:?}");
        assert!(bench.engine().running().is_empty());
        bench.shutdown().await;
    }

    /// Chargement (bouton Load, préchargement) dans le moteur, avec le
    /// contexte et la durée en mémoire réglés, comme le chat ; un modèle
    /// absent de la base locale est introuvable.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_model_is_loaded_in_the_engine_as_the_chat_would() {
        let id = "test/Simple-GGUF:Q4_K_M";
        let bench = crate::llm::testing::FakeBench::new(&[(id, "simple")]);
        bench.set(SettingsPatch {
            context_length: Some(8192),
            keep_alive: Some(-1),
            ..Default::default()
        });
        load_in_engine(&bench.access, id).await.unwrap();
        let running = bench.engine().running();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].context_length, 8192);
        assert!(running[0].kept && !running[0].in_use);
        bench.set(SettingsPatch {
            keep_alive: Some(60),
            ..Default::default()
        });
        load_in_engine(&bench.access, id).await.unwrap();
        let left = bench.engine().running()[0].expires_at.unwrap() - crate::store::now_ms();
        assert!((55_000..=60_000).contains(&left), "{left}");
        let Err(AppError::NotFound(message)) =
            load_in_engine(&bench.access, "a/nope-GGUF:Q4_K_M").await
        else {
            panic!("expected a missing model");
        };
        assert!(message.contains("a/nope-GGUF:Q4_K_M"), "{message}");
        bench.shutdown().await;
    }
}
