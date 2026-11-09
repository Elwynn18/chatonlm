use tauri::State;

use crate::chat::stream::Think;
use crate::engine::{Engine, RunningModel as EngineRunningModel};
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::llm::Access;
use crate::llm::models::{self as facade, ChatModel, ModelDetails, RecommendedModel};
use crate::ollama::models;
use crate::ollama::registry;
use crate::ollama::running;
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

/// Modèles recommandés (tête du sélecteur, premier modèle), avec leur place
/// dans la machine. Corps : lot L5b (`hub/curation.json` + `hub::fit`) ;
/// provisoire : aucun.
#[tauri::command]
pub async fn recommended_models() -> AppResult<Vec<RecommendedModel>> {
    Ok(Vec::new())
}

/// Pastille de chaque modèle recommandé dont la mémoire est connue (pour le
/// corps de `recommended_models`, lot L5b).
#[allow(dead_code, reason = "corps de recommended_models (lot L5b)")]
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
/// récente. Corps : lot L5b (`local_models::check_outdated`) ; provisoire :
/// aucun.
#[tauri::command]
pub async fn outdated_models(models: Vec<String>) -> AppResult<Vec<String>> {
    let _ = models;
    Ok(Vec::new())
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

/// Détails d'Ollama, plus ce que ChatonLM a observé : le modèle raisonne même
/// avec `think: false`. Transition (pont, lot L9) : la page passe par
/// [`facade::details`].
pub(crate) async fn details(store: &Store, model: &str) -> AppResult<models::ModelDetails> {
    let mut details = models::details(&server_url(store).await?, model).await?;
    let name = model.to_owned();
    details.always_thinks = store
        .call(move |connection| model_prefs::always_thinks(connection, &name))
        .await?;
    Ok(details)
}

/// Chargement avec les options d'un envoi du chat (contexte et durée
/// réglés : le message suivant ne relance pas le modèle), dans le moteur de
/// l'app (bouton Load, préchargement).
pub(crate) async fn load_in_engine(access: &Access, model: &str) -> AppResult<()> {
    // L'autocomplétion de l'éditeur attend pendant ce temps.
    let _working = crate::gpu::activity::hold();
    facade::load(access, model).await.map_err(AppError::from)
}

/// Chargement dans Ollama avec les options d'un envoi du chat (Easy Link,
/// jusqu'au lot L9) : adresse effective,
/// contexte et durée réglés (le message suivant ne recharge pas le modèle).
/// Refusé pour un modèle distant (nom cloud, ou `/api/show` qui le dit) :
/// rien à charger ici.
pub(crate) async fn load(store: &Store, model: &str) -> AppResult<()> {
    // L'autocomplétion de l'éditeur attend pendant ce temps (une
    // proposition mise en file pendant le chargement le rechargerait).
    let _working = crate::gpu::activity::hold();
    let settings = app_settings(store).await?;
    let base = &settings.effective_server_url;
    if models::is_cloud_name(model)
        || models::show(base, model)
            .await
            .is_ok_and(|details| details.remote)
    {
        return Err(AppError::Invalid(t!(
            "models.cloudNotLoaded",
            model = model
        )));
    }
    running::load(
        &settings.effective_server_url,
        model,
        settings.context_length,
        settings.keep_alive,
    )
    .await
}

#[cfg(test)]
mod tests {
    use fake_ollama::{FakeOllama, Mode};

    use super::*;
    use crate::store::settings::{self, SettingsPatch};

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

    /// Base dont le serveur réglé est `url`.
    fn configured(url: String) -> Store {
        let store = Store::open_in_memory().unwrap();
        let patch = SettingsPatch {
            server_url: Some(url),
            ..Default::default()
        };
        store
            .blocking(|c| settings::update(c, patch, None).map(drop))
            .unwrap();
        store
    }

    /// Les détails viennent du serveur réglé, pas de l'adresse par défaut.
    #[tokio::test]
    async fn details_are_asked_to_the_configured_server() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = configured(server.url());
        let found = details(&store, "chatonlm-never").await.unwrap();
        assert_eq!(found.model, "chatonlm-never");
        assert_eq!(
            found.thinking.map(|thinking| thinking.values),
            Some(vec![Think::Enabled(false)])
        );
    }

    /// Un modèle vu raisonner malgré `think: false` est signalé dans ses
    /// détails ; les autres non.
    #[tokio::test]
    async fn details_tell_a_model_that_always_thinks() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = configured(server.url());
        assert!(
            !details(&store, "chatonlm-stubborn")
                .await
                .unwrap()
                .always_thinks
        );
        store
            .blocking(|c| model_prefs::set_always_thinks(c, "chatonlm-stubborn"))
            .unwrap();
        assert!(
            details(&store, "chatonlm-stubborn")
                .await
                .unwrap()
                .always_thinks
        );
        assert!(
            !details(&store, "chatonlm-nometa")
                .await
                .unwrap()
                .always_thinks
        );
    }

    /// Chargement (bouton Load, préchargement) au serveur réglé, avec le
    /// contexte et la durée en mémoire réglés, comme le chat.
    #[tokio::test]
    async fn a_model_is_loaded_as_the_chat_would() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = configured(server.url());
        load(&store, "qwen3:0.6b").await.unwrap();
        let patch = SettingsPatch {
            context_length: Some(8192),
            keep_alive: Some(-1),
            ..Default::default()
        };
        store
            .blocking(|c| settings::update(c, patch, None).map(drop))
            .unwrap();
        load(&store, "qwen3.5:0.8b").await.unwrap();
        assert_eq!(
            server.load_requests(),
            [
                serde_json::json!({ "model": "qwen3:0.6b", "kind": "load" }),
                serde_json::json!({ "model": "qwen3.5:0.8b", "kind": "load", "keep_alive": -1, "num_ctx": 8192 }),
            ]
        );
        let Err(AppError::NotFound(message)) = load(&store, "nope:1b").await else {
            panic!("expected a missing model");
        };
        assert_eq!(message, "model 'nope:1b' not found");
        // Modèles distants : refusés sans rien demander à charger.
        for model in ["gpt-oss:20b-cloud", "chatonlm-polish-remote"] {
            let Err(AppError::Invalid(message)) = load(&store, model).await else {
                panic!("{model} loaded");
            };
            assert!(
                message.starts_with(&format!("{model} runs on ollama.com")),
                "{message}"
            );
        }
        assert_eq!(server.load_requests().len(), 3);
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
