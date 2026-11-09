//! Façade des modèles (contrat 102 § 1.2) : sélecteur, détails, onglet
//! Models. Remplace pour tous les modules `ollama::models::{list, show,
//! details}` et `ollama::running::*`.

use serde::Serialize;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use super::ids;
use super::{Access, LlmError, OpenOptions};
use crate::chat::stream::Think;
use crate::engine::RunningModel as EngineRunningModel;
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::local_models::{LocalModel, ModelCapabilities, ModelSource};
use crate::ollama::models::ThinkingOptions;
use crate::store::model_prefs;

/// Origine d'un modèle du sélecteur.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ModelOrigin {
    /// Téléchargé de Hugging Face.
    Hf,
    /// Repris d'un Ollama (ChatonLM 0.9.x).
    Ollama,
    /// Chez un fournisseur distant (clé de l'utilisateur).
    Remote,
}

/// Modèle du sélecteur (`list_models`, pont `models.list`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatModel {
    /// Identifiant : base locale, ou `@remote/<fournisseur>/<modèle>`.
    pub id: String,
    pub display_name: String,
    pub origin: ModelOrigin,
    /// Fournisseur d'un modèle distant.
    pub provider: Option<String>,
    /// Octets sur le disque (projecteur compris) ; distant : `None`.
    pub size_bytes: Option<u64>,
    /// Taille annoncée (« 0.6B »).
    pub params_label: Option<String>,
    pub quant: Option<String>,
    /// Contexte maximal du modèle.
    pub context_max: Option<u32>,
    /// `completion`, `vision`, `audio`, `tools`, `thinking`, `insert` (mêmes
    /// mots que `HubInstalled`).
    pub capabilities: Vec<String>,
    /// ms depuis 1970 ; distant : `None`.
    pub installed_at: Option<i64>,
}

/// Détails d'un modèle utiles à l'interface (capacités, réglages de la
/// réflexion) ; même forme que ceux d'Ollama en 0.9.x.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelDetails {
    /// Identifiant demandé, tel que reçu.
    pub model: String,
    pub capabilities: Vec<String>,
    /// Réglages de la réflexion acceptés : `[true, false]` pour un modèle
    /// qui réfléchit, niveaux pour gpt-oss ; `null` sinon.
    pub thinking: Option<ThinkingOptions>,
    /// Le modèle a déjà réfléchi alors que la réflexion était coupée (vu par
    /// ChatonLM) : la réflexion ne peut pas être coupée.
    pub always_thinks: bool,
}

/// Modèle recommandé (tête du sélecteur, premier modèle) ; corps de la
/// commande : lot L5b (`hub/curation.json` + `hub::fit`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecommendedModel {
    /// Identifiant Hugging Face (`<auteur>/<dépôt>:<QUANT>`).
    pub model: String,
    pub display_name: String,
    pub size_bytes: Option<u64>,
    /// Mémoire qu'il faut au modèle pour tourner.
    pub vram_bytes: Option<u64>,
    /// Place de ce besoin dans la machine (pastille du ModelHub).
    pub fit: Option<crate::hub::types::HubFit>,
}

/// Capacités sous les mots de la page.
pub fn capability_words(capabilities: &ModelCapabilities) -> Vec<String> {
    std::iter::once("completion")
        .chain(capabilities.vision.then_some("vision"))
        .chain(capabilities.audio.then_some("audio"))
        .chain(capabilities.tools.then_some("tools"))
        .chain(capabilities.thinking.then_some("thinking"))
        .chain(capabilities.fim.then_some("insert"))
        .map(str::to_owned)
        .collect()
}

/// Modèle installé pour le sélecteur.
pub fn local_entry(model: &LocalModel) -> ChatModel {
    ChatModel {
        id: model.id.clone(),
        display_name: model.display_name.clone(),
        origin: match model.source {
            ModelSource::Hf => ModelOrigin::Hf,
            ModelSource::Ollama => ModelOrigin::Ollama,
        },
        provider: None,
        size_bytes: Some(model.size_bytes),
        params_label: model.params_label.clone(),
        quant: Some(model.quant.clone()),
        context_max: model.context_max,
        capabilities: capability_words(&model.capabilities),
        installed_at: Some(model.installed_at),
    }
}

/// Réglages de la réflexion d'après les capacités et l'architecture (règle
/// pure) : niveaux `low`/`medium`/`high` pour gpt-oss (comme Ollama, défaut
/// `medium`), marche/arrêt pour un modèle qui réfléchit (défaut : marche),
/// rien sinon.
pub fn thinking_options(thinking: bool, arch: Option<&str>) -> Option<ThinkingOptions> {
    if arch.is_some_and(|arch| arch.eq_ignore_ascii_case("gpt-oss")) {
        let level = |name: &str| Think::Level(name.to_owned());
        return Some(ThinkingOptions {
            values: vec![level("low"), level("medium"), level("high")],
            default: Some(level("medium")),
        });
    }
    thinking.then(|| ThinkingOptions {
        values: vec![Think::Enabled(true), Think::Enabled(false)],
        default: Some(Think::Enabled(true)),
    })
}

/// Modèles du sélecteur : installés du plus récent au plus ancien, puis
/// distants si le réglage les permet.
pub async fn list(access: &Access) -> AppResult<Vec<ChatModel>> {
    let mut models: Vec<ChatModel> = crate::local_models::list(access.store(), access.data_dir())
        .await?
        .iter()
        .map(local_entry)
        .collect();
    if access.prefs().await?.remote_models {
        models.extend(access.remote().list().await);
    }
    Ok(models)
}

/// Modèle `id` du sélecteur, s'il est là (distant : seulement si permis).
pub async fn get(access: &Access, id: &str) -> AppResult<Option<ChatModel>> {
    if ids::is_remote(id) {
        if !access.prefs().await?.remote_models {
            return Ok(None);
        }
        return Ok(access
            .remote()
            .list()
            .await
            .into_iter()
            .find(|model| ids::same(&model.id, id)));
    }
    Ok(
        crate::local_models::get(access.store(), access.data_dir(), id)
            .await?
            .as_ref()
            .map(local_entry),
    )
}

/// Détails du modèle `id` ; [`AppError::NotFound`] s'il est absent.
pub async fn details(access: &Access, id: &str) -> AppResult<ModelDetails> {
    let (capabilities, thinking) = if ids::is_remote(id) {
        let model = get(access, id).await?.ok_or_else(|| not_found(id))?;
        let thinking = model.capabilities.iter().any(|word| word == "thinking");
        (model.capabilities, thinking_options(thinking, None))
    } else {
        let model = crate::local_models::get(access.store(), access.data_dir(), id)
            .await?
            .ok_or_else(|| not_found(id))?;
        (
            capability_words(&model.capabilities),
            thinking_options(model.capabilities.thinking, model.arch.as_deref()),
        )
    };
    let key = id.to_owned();
    let always_thinks = access
        .store()
        .call(move |connection| model_prefs::always_thinks(connection, &key))
        .await?;
    Ok(ModelDetails {
        model: id.to_owned(),
        capabilities,
        thinking,
        always_thinks,
    })
}

fn not_found(id: &str) -> AppError {
    AppError::NotFound(t!("engine.notInstalled", model = id))
}

/// Modèles du moteur, du plus récemment servi au plus ancien.
pub fn running(access: &Access) -> Vec<EngineRunningModel> {
    access.engine().running()
}

/// Charge `id` avec les options d'un envoi (réglages) et rend la main une
/// fois prêt ; le minuteur `keep_alive` part aussitôt (le `Lease` est
/// lâché). Un modèle distant n'a rien à charger : rien n'est fait.
pub async fn load(access: &Access, id: &str) -> Result<(), LlmError> {
    if ids::is_remote(id) {
        return Ok(());
    }
    let session = access
        .open(
            id,
            OpenOptions::default(),
            &CancellationToken::new(),
            &|| {},
        )
        .await?;
    drop(session);
    Ok(())
}

/// Décharge `id` (attend la fin de son processus) ; un distant : rien.
pub async fn unload(access: &Access, id: &str) {
    if !ids::is_remote(id) {
        access.engine().unload(id).await;
    }
}

/// Décharge tous les modèles du moteur.
pub async fn unload_all(access: &Access) {
    access.engine().unload_all().await;
}
