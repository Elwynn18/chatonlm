use tauri::State;

use crate::chat::stream::Think;
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::ollama::models::{self, ModelDetails, ModelInfo, RecommendedModel};
use crate::ollama::outdated;
use crate::ollama::registry;
use crate::ollama::running::{self, RunningModel};
use crate::store::settings::Settings;
use crate::store::{Store, model_prefs};

/// Modèles utilisables en conversation, à l'adresse effective du serveur.
#[tauri::command]
pub async fn list_models(store: State<'_, Store>) -> AppResult<Vec<ModelInfo>> {
    models::list(&server_url(&store).await?).await
}

/// Capacités et réglages du raisonnement d'un modèle (`/api/show`).
#[tauri::command]
pub async fn model_details(store: State<'_, Store>, model: String) -> AppResult<ModelDetails> {
    details(&store, &model).await
}

/// Modèles recommandés par le serveur, cloud compris (avec le forfait
/// exigé), sans `:latest`.
#[tauri::command]
pub async fn recommended_models(store: State<'_, Store>) -> AppResult<Vec<RecommendedModel>> {
    recommended(&store).await
}

/// Vrai si le modèle existe au registre d'Ollama (nom tapé dans le
/// sélecteur) ; une erreur réseau vaut « non » pour l'interface.
#[tauri::command]
pub async fn model_exists_upstream(model: String) -> AppResult<bool> {
    registry::model_exists(&registry::registry_url(), &model).await
}

/// Parmi `models`, les modèles installés dont le registre (ollama.com,
/// Hugging Face) a une version plus récente. Rejette si Ollama ne répond
/// pas ; un registre injoignable ne signale rien.
#[tauri::command]
pub async fn outdated_models(
    store: State<'_, Store>,
    models: Vec<String>,
) -> AppResult<Vec<String>> {
    let server = server_url(&store).await?;
    outdated::outdated(&server, &registry::registry_url(), &models).await
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

/// Modèles en mémoire (`/api/ps`), à l'adresse effective du serveur.
#[tauri::command]
pub async fn running_models(store: State<'_, Store>) -> AppResult<Vec<RunningModel>> {
    running::list(&server_url(&store).await?).await
}

/// Modèle que prend « Auto » (pages Proofread et Study, rien de choisi) en ce
/// moment : le meilleur modèle en mémoire, sinon celui du chat ; `None` si
/// aucun. Réglages à montrer : la page ne patiente pas pour l'intégré.
#[tauri::command]
pub async fn auto_model(store: State<'_, Store>) -> AppResult<Option<String>> {
    let settings = crate::ollama::target::display_settings(&store).await?;
    Ok(
        crate::ollama::auto::model_of(&settings.effective_server_url, "", &settings.selected_model)
            .await,
    )
}

/// Charge un modèle avec le contexte et la durée des réglages ; rend la main
/// une fois chargé (bouton Load, préchargement du modèle choisi).
/// Le chef de la carte graphique replanifie aussitôt (lien VS Code).
#[tauri::command]
pub async fn load_model(
    store: State<'_, Store>,
    gpu: State<'_, crate::gpu::GpuChief>,
    model: String,
) -> AppResult<()> {
    let loaded = load(&store, &model).await;
    gpu.poke();
    loaded
}

/// Décharge un modèle.
#[tauri::command]
pub async fn unload_model(
    store: State<'_, Store>,
    gpu: State<'_, crate::gpu::GpuChief>,
    model: String,
) -> AppResult<()> {
    let working = crate::gpu::activity::hold();
    let unloaded = running::unload(&server_url(&store).await?, &model).await;
    drop(working);
    gpu.poke();
    unloaded
}

/// Décharge tous les modèles en mémoire (la première erreur est rendue).
#[tauri::command]
pub async fn unload_all_models(
    store: State<'_, Store>,
    gpu: State<'_, crate::gpu::GpuChief>,
) -> AppResult<()> {
    let working = crate::gpu::activity::hold();
    let unloaded = running::unload_all(&server_url(&store).await?).await;
    drop(working);
    gpu.poke();
    unloaded
}

/// Adresse du serveur auquel parler : celle de la cible (`ollama::target` :
/// Ollama intégré, sinon adresse réglée, `OLLAMA_HOST` ou défaut).
pub(crate) async fn server_url(store: &Store) -> AppResult<String> {
    Ok(app_settings(store).await?.effective_server_url)
}

/// Réglages de l'app, avec l'adresse de la cible (point unique de toutes les
/// requêtes à Ollama).
pub(crate) async fn app_settings(store: &Store) -> AppResult<Settings> {
    crate::ollama::target::request_settings(store).await
}

/// Détails d'Ollama, plus ce que ChatonLM a observé : le modèle raisonne même
/// avec `think: false`.
pub(crate) async fn details(store: &Store, model: &str) -> AppResult<ModelDetails> {
    let mut details = models::details(&server_url(store).await?, model).await?;
    let name = model.to_owned();
    details.always_thinks = store
        .call(move |connection| model_prefs::always_thinks(connection, &name))
        .await?;
    Ok(details)
}

async fn recommended(store: &Store) -> AppResult<Vec<RecommendedModel>> {
    models::recommended(&server_url(store).await?).await
}

/// Chargement avec les options d'un envoi du chat : adresse effective,
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

    /// Les recommandations aussi.
    #[tokio::test]
    async fn recommendations_are_asked_to_the_configured_server() {
        let server = FakeOllama::start(Mode::no_models()).await.unwrap();
        let store = configured(server.url());
        let models = recommended(&store).await.unwrap();
        let names: Vec<_> = models.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(names[4..], ["gemma4", "qwen3.5"]);
    }
}
