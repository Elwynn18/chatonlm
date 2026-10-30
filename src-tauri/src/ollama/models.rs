//! Modèles installés sur le serveur Ollama (`GET /api/tags`), détails d'un
//! modèle (`POST /api/show`) et modèles recommandés par le serveur
//! (`GET /api/experimental/model-recommendations`).
//!
//! Seuls les modèles installés utilisables en conversation sont gardés :
//! capacité `completion` (ce qui écarte embeddings, modèles d'images et de
//! décision), et pas de modèle distant (un modèle cloud n'est pas installé :
//! le sélecteur le propose par les recommandations, si le réglage
//! `cloud_models` le permet). L'ordre est celui d'Ollama : du plus récemment
//! installé au plus ancien.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use super::client;
use crate::chat::stream::Think;
use crate::error::{AppError, AppResult};
use crate::i18n::t;

/// Délai de la liste : quelques fichiers lus par le serveur local.
const LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// Délai des détails : Ollama lit le manifeste et l'en-tête du fichier GGUF.
const SHOW_TIMEOUT: Duration = Duration::from_secs(10);

/// Délai des recommandations (liste en cache chez Ollama).
const RECOMMENDATIONS_TIMEOUT: Duration = Duration::from_secs(10);

/// Modèle proposé dans le sélecteur.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelInfo {
    /// Nom affiché et mémorisé, sans le suffixe `:latest` (comme l'officiel).
    pub model: String,
    /// Empreinte du manifeste.
    pub digest: String,
    /// Taille sur le disque, en octets.
    pub size: u64,
    /// « 751.63M », « 8.0B »...
    pub parameter_size: String,
    /// « Q4_K_M »...
    pub quantization_level: String,
    /// Longueur de contexte maximale du modèle, si connue.
    pub context_length: Option<u32>,
    /// Capacités annoncées : completion, vision, thinking, tools...
    pub capabilities: Vec<String>,
}

/// Détails d'un modèle utiles à l'interface (`POST /api/show`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelDetails {
    /// Nom demandé, tel que reçu.
    pub model: String,
    /// Capacités annoncées ; vide pour un serveur trop ancien.
    pub capabilities: Vec<String>,
    /// Réglages du raisonnement acceptés ; `null` si Ollama n'en dit rien
    /// (serveur antérieur à 0.34.3, modèle sans métadonnées) : se fier alors
    /// aux capacités.
    pub thinking: Option<ThinkingOptions>,
    /// Le modèle a déjà raisonné alors que `think: false` était envoyé (vu par
    /// ChatonLM, pas par Ollama) : le raisonnement ne peut pas être coupé.
    pub always_thinks: bool,
    /// Modèle qui tourne ailleurs (voir [`is_remote`]) : rien à charger ici,
    /// rien de dicté ne lui est envoyé. Pour le moteur, et pour l'éditeur
    /// (`models.details` du pont, qui l'ajoute à part).
    #[serde(skip)]
    #[ts(skip)]
    pub remote: bool,
    /// Contexte maximal du modèle (`<architecture>.context_length` de
    /// `model_info`), si connu : Ollama y ramène un `num_ctx` plus grand. Pour
    /// le moteur de chat seulement.
    #[serde(skip)]
    #[ts(skip)]
    pub max_context: Option<u32>,
}

/// Objet `thinking` de `/api/show`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct ThinkingOptions {
    /// Valeurs acceptées pour `think`, dans l'ordre d'Ollama : booléens
    /// (marche, arrêt) et niveaux nommés (« low », « medium »...). `[false]` :
    /// le modèle ne sait pas raisonner.
    pub values: Vec<Think>,
    /// Valeur appliquée quand `think` est absent, si Ollama l'indique.
    pub default: Option<Think>,
}

#[derive(Deserialize)]
struct ShowResponse {
    #[serde(default)]
    capabilities: Option<Vec<String>>,
    /// Lu à la main : une valeur inattendue ne doit pas faire échouer le
    /// reste.
    #[serde(default)]
    thinking: Value,
    /// Lu à la main, comme `thinking`.
    #[serde(default)]
    model_info: Value,
    #[serde(default)]
    remote_host: Value,
    #[serde(default)]
    remote_model: Value,
}

/// `api.ModelRecommendationsResponse` : seuls les noms et le forfait exigé
/// servent.
#[derive(Deserialize)]
struct RecommendationsResponse {
    #[serde(default)]
    recommendations: Option<Vec<Recommendation>>,
}

#[derive(Deserialize)]
struct Recommendation {
    #[serde(default)]
    model: String,
    /// Lu à la main : une valeur inattendue ne doit pas faire échouer le
    /// reste.
    #[serde(default)]
    required_plan: Value,
}

/// Modèle recommandé par le serveur, en tête du sélecteur.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecommendedModel {
    /// Nom affiché, sans `:latest`.
    pub model: String,
    /// Forfait ollama.com exigé par un modèle cloud (« free », « pro »...),
    /// tel que le serveur le donne ; absent pour un modèle local ou si le
    /// serveur ne le dit pas.
    pub required_plan: Option<String>,
}

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<Tag>,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
    #[serde(default)]
    remote_host: Option<String>,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    digest: String,
    #[serde(default)]
    details: TagDetails,
    /// Absent pour un serveur ancien ou un modèle qui ne se charge pas.
    #[serde(default)]
    capabilities: Option<Vec<String>>,
}

#[derive(Default, Deserialize)]
struct TagDetails {
    #[serde(default)]
    families: Option<Vec<String>>,
    #[serde(default)]
    parameter_size: String,
    #[serde(default)]
    quantization_level: String,
    #[serde(default)]
    context_length: Option<u32>,
}

/// Modèles utilisables en conversation, du plus récent au plus ancien.
pub async fn list(base: &str) -> AppResult<Vec<ModelInfo>> {
    let url = client::server_url(base)?;
    let client = client::client(&url, Some(LIST_TIMEOUT)).map_err(unreachable)?;
    let response = client
        .get(format!("{base}/api/tags"))
        .send()
        .await
        .map_err(unreachable)?;
    let status = response.status();
    let body = response.text().await.map_err(unreachable)?;
    if !status.is_success() {
        return Err(answered(status.as_u16(), &error_message(&body)));
    }
    parse(&body)
}

fn parse(body: &str) -> AppResult<Vec<ModelInfo>> {
    let tags: TagsResponse = serde_json::from_str(body)
        .map_err(|error| AppError::Ollama(t!("ollama.unexpectedModels", error = error)))?;
    Ok(tags
        .models
        .into_iter()
        .filter(usable)
        .map(|tag| ModelInfo {
            model: display_name(&tag.name),
            digest: tag.digest,
            size: tag.size,
            parameter_size: tag.details.parameter_size,
            quantization_level: tag.details.quantization_level,
            context_length: tag.details.context_length,
            capabilities: tag.capabilities.unwrap_or_default(),
        })
        .collect())
}

fn usable(tag: &Tag) -> bool {
    if tag
        .remote_host
        .as_deref()
        .is_some_and(|host| !host.is_empty())
    {
        return false;
    }
    match &tag.capabilities {
        Some(capabilities) => capabilities.iter().any(|c| c == "completion"),
        // Serveur sans capacités dans la liste : comme l'officiel, les modèles
        // dont toutes les familles sont BERT sont des modèles d'embeddings.
        None => !tag.details.families.as_ref().is_some_and(|families| {
            !families.is_empty()
                && families
                    .iter()
                    .all(|family| family.to_ascii_lowercase().contains("bert"))
        }),
    }
}

/// Détails du modèle `model` (nom affiché ou complet : Ollama complète
/// `:latest`). Erreurs : modèle absent → [`AppError::NotFound`] (404
/// d'Ollama) ; serveur injoignable, autre statut d'erreur ou réponse illisible
/// → [`AppError::Ollama`].
pub async fn details(base: &str, model: &str) -> AppResult<ModelDetails> {
    show(base, model).await.map_err(|error| match error {
        ShowError::Unreachable(error) | ShowError::Other(error) => error,
        ShowError::NotFound(message) if message.trim().is_empty() => {
            AppError::NotFound(t!("ollama.modelNotFound", model = model))
        }
        ShowError::NotFound(message) => AppError::NotFound(message),
    })
}

/// Échec de `/api/show`, détaillé pour le moteur de chat : il ne télécharge
/// un modèle que s'il manque à un serveur joignable.
#[derive(Debug)]
pub enum ShowError {
    /// Connexion impossible (rien n'écoute, nom d'hôte inconnu...).
    Unreachable(AppError),
    /// HTTP 404, avec le message d'erreur d'Ollama (JSON) ; vide pour la
    /// page d'un proxy (vide, texte ou HTML).
    NotFound(String),
    /// Autre échec : statut d'erreur, délai dépassé, réponse illisible...
    Other(AppError),
}

/// `POST /api/show`, avec les erreurs de [`ShowError`].
pub async fn show(base: &str, model: &str) -> Result<ModelDetails, ShowError> {
    let other = |error: reqwest::Error| ShowError::Other(unreachable(error));
    if model.trim().is_empty() {
        // i18n-ignore : la page ne demande jamais un nom vide.
        return Err(ShowError::Other(AppError::Invalid(
            "model name is empty".into(),
        )));
    }
    let url = client::server_url(base).map_err(ShowError::Other)?;
    let client = client::client(&url, Some(SHOW_TIMEOUT)).map_err(other)?;
    let response = client
        .post(format!("{base}/api/show"))
        .json(&serde_json::json!({ "model": model }))
        .send()
        .await
        .map_err(|error| {
            if error.is_connect() {
                ShowError::Unreachable(unreachable(error))
            } else {
                other(error)
            }
        })?;
    let status = response.status();
    let body = response.text().await.map_err(other)?;
    if status == reqwest::StatusCode::NOT_FOUND {
        // Seule une erreur d'Ollama (JSON) peut dire « not found » : la page
        // 404 d'un proxy (« 404 page not found » de Go, gin, Traefik) n'est
        // pas un modèle absent.
        return Err(ShowError::NotFound(ollama_error(&body).unwrap_or_default()));
    }
    if !status.is_success() {
        return Err(ShowError::Other(answered(
            status.as_u16(),
            &error_message(&body),
        )));
    }
    parse_details(model, &body).map_err(ShowError::Other)
}

fn parse_details(model: &str, body: &str) -> AppResult<ModelDetails> {
    let show: ShowResponse = serde_json::from_str(body)
        .map_err(|error| AppError::Ollama(t!("ollama.unexpectedDetails", error = error)))?;
    Ok(ModelDetails {
        model: model.to_owned(),
        capabilities: show.capabilities.unwrap_or_default(),
        thinking: thinking_options(&show.thinking),
        // Lu dans la base par la commande model_details.
        always_thinks: false,
        remote: is_remote(model, &show.remote_host, &show.remote_model),
        max_context: max_context(&show.model_info),
    })
}

/// Nom d'un modèle d'ollama.com (`gpt-oss:20b-cloud`, `kimi-k2:cloud`),
/// comme le serveur le lit (`internal/modelref`, `parseSourceSuffix`) :
/// l'étiquette (après le dernier « : », hors hôte) rognée et sans casse vaut
/// « cloud » ou finit par « -cloud ». Plus strict que `isCloud` de
/// l'officiel (nom qui finit par « cloud ») : un modèle local `pointcloud`
/// reste local, `kimi-k2.6:Cloud` est bien du cloud.
pub fn is_cloud_name(model: &str) -> bool {
    let Some((_, tag)) = model.rsplit_once(':') else {
        return false;
    };
    if tag.contains('/') {
        return false;
    }
    let tag = tag.trim().to_ascii_lowercase();
    tag == "cloud" || tag.ends_with("-cloud")
}

/// Refus d'un modèle cloud, propre au cloud, comme l'officiel (`ui.go`,
/// `getError`) : 401 → aucun compte ollama.com relié au serveur (« cloud
/// unauthorized ») ; 402 → limite d'usage du forfait atteinte. Rien pour un
/// autre statut, ni pour un modèle local (un 401 vient alors d'un proxy).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudRefusal {
    SignedOut,
    UsageLimit,
}

impl CloudRefusal {
    pub fn of(status: u16, remote: bool) -> Option<Self> {
        match status {
            401 if remote => Some(Self::SignedOut),
            402 if remote => Some(Self::UsageLimit),
            _ => None,
        }
    }

    /// Texte montré (et dit par un bot d'Easy Link).
    pub fn message(self) -> String {
        match self {
            Self::SignedOut => t!("cloud.signedOut"),
            Self::UsageLimit => t!("cloud.usageLimit"),
        }
    }
}

/// Modèle cloud demandé alors que le réglage `cloud_models` est coupé :
/// rien n'est envoyé (texte de l'erreur).
pub fn cloud_off(model: &str) -> String {
    t!("cloud.off", model = model)
}

/// Modèle qui tourne ailleurs : nom de modèle cloud, ou `/api/show` qui
/// donne `remote_host` ou `remote_model` non vides (modèle d'ollama.com sous
/// un nom local).
fn is_remote(model: &str, remote_host: &Value, remote_model: &Value) -> bool {
    is_cloud_name(model)
        || [remote_host, remote_model]
            .iter()
            .any(|field| field.as_str().is_some_and(|value| !value.is_empty()))
}

/// Contexte maximal d'après `model_info` : la clé
/// `<general.architecture>.context_length` (« qwen3.context_length »).
fn max_context(model_info: &Value) -> Option<u32> {
    let architecture = model_info.get("general.architecture")?.as_str()?;
    let length = model_info
        .get(format!("{architecture}.context_length"))?
        .as_u64()?;
    u32::try_from(length).ok().filter(|&length| length > 0)
}

/// Lit l'objet `thinking` comme Ollama le valide (`types/model/thinking.go`,
/// `Valid`) : booléens et chaînes non vides, sans doublon ; une valeur d'un
/// autre type est ignorée. Sans aucune valeur exploitable, l'objet compte
/// comme absent. Le défaut n'est pas confronté aux valeurs : l'interface s'en
/// charge.
fn thinking_options(thinking: &Value) -> Option<ThinkingOptions> {
    let mut values = Vec::new();
    for value in thinking.get("values")?.as_array()? {
        if let Some(value) = think_value(value)
            && !values.contains(&value)
        {
            values.push(value);
        }
    }
    if values.is_empty() {
        return None;
    }
    Some(ThinkingOptions {
        values,
        default: thinking.get("default").and_then(think_value),
    })
}

fn think_value(value: &Value) -> Option<Think> {
    match value {
        Value::Bool(enabled) => Some(Think::Enabled(*enabled)),
        Value::String(level) if !level.is_empty() => Some(Think::Level(level.clone())),
        _ => None,
    }
}

/// Modèles recommandés par le serveur, dans son ordre, comme le sélecteur de
/// l'officiel (`useModels.ts`) : ceux du cloud compris (le sélecteur les
/// écarte tant que le réglage `cloud_models` est coupé), noms affichés sans
/// `:latest`, sans doublon. Serveur sans cette route (404) : liste vide.
pub async fn recommended(base: &str) -> AppResult<Vec<RecommendedModel>> {
    let url = client::server_url(base)?;
    let client = client::client(&url, Some(RECOMMENDATIONS_TIMEOUT)).map_err(unreachable)?;
    let response = client
        .get(format!("{base}/api/experimental/model-recommendations"))
        .send()
        .await
        .map_err(unreachable)?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(Vec::new());
    }
    let body = response.text().await.map_err(unreachable)?;
    if !status.is_success() {
        return Err(answered(status.as_u16(), &error_message(&body)));
    }
    parse_recommendations(&body)
}

fn parse_recommendations(body: &str) -> AppResult<Vec<RecommendedModel>> {
    let response: RecommendationsResponse = serde_json::from_str(body)
        .map_err(|error| AppError::Ollama(t!("ollama.unexpectedRecommendations", error = error)))?;
    let mut models: Vec<RecommendedModel> = Vec::new();
    for recommendation in response.recommendations.unwrap_or_default() {
        if recommendation.model.is_empty() {
            continue;
        }
        let model = display_name(&recommendation.model);
        if models.iter().any(|known| known.model == model) {
            continue;
        }
        // Forfait d'un modèle cloud seulement : rien à payer en local.
        let required_plan = recommendation
            .required_plan
            .as_str()
            .map(str::trim)
            .filter(|plan| !plan.is_empty() && is_cloud_name(&model))
            .map(str::to_ascii_lowercase);
        models.push(RecommendedModel {
            model,
            required_plan,
        });
    }
    Ok(models)
}

/// `llama3:latest` → `llama3` ; les autres étiquettes sont gardées.
pub fn display_name(name: &str) -> String {
    name.strip_suffix(":latest").unwrap_or(name).to_owned()
}

/// Message d'une réponse d'erreur d'Ollama (`{"error": "..."}`), sinon le corps.
pub fn error_message(body: &str) -> String {
    ollama_error(body).unwrap_or_else(|| body.chars().take(200).collect())
}

/// Message d'une réponse d'erreur d'Ollama (`{"error": "..."}`), s'il y en a.
fn ollama_error(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value["error"].as_str().map(str::to_owned))
}

pub(crate) fn unreachable(error: reqwest::Error) -> AppError {
    let error = error.without_url();
    AppError::Ollama(if error.is_timeout() {
        t!("ollama.timeout")
    } else if error.is_connect() {
        t!("ollama.unreachable")
    } else {
        t!("ollama.requestFailed", error = error)
    })
}

/// Réponse d'erreur d'Ollama : son statut, puis son message tel quel.
pub(crate) fn answered(status: u16, message: &str) -> AppError {
    AppError::Ollama(t!("ollama.http", status = status, message = message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fake_ollama::{FakeOllama, Mode, fixtures};

    #[test]
    fn recorded_list_is_read_in_server_order() {
        let models = parse(fixtures::TAGS).unwrap();
        let names: Vec<_> = models.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(names, ["qwen3:0.6b", "qwen3.5:0.8b"]);
        let qwen3 = &models[0];
        assert_eq!(qwen3.parameter_size, "751.63M");
        assert_eq!(qwen3.context_length, Some(40960));
        assert!(qwen3.capabilities.contains(&"thinking".to_owned()));
        assert!(models[1].capabilities.contains(&"vision".to_owned()));
    }

    #[test]
    fn only_local_chat_models_are_kept() {
        let body = r#"{"models": [
            {"name": "llama3:latest", "capabilities": ["completion"]},
            {"name": "nomic-embed-text:latest", "capabilities": ["embedding"]},
            {"name": "gpt-oss:120b-cloud", "remote_host": "https://ollama.com:443",
             "capabilities": ["completion"]},
            {"name": "old-embed", "details": {"families": ["nomic-bert"]}},
            {"name": "old-chat", "details": {"families": ["llama"]}},
            {"name": "unknown"}
        ]}"#;
        let names: Vec<_> = parse(body).unwrap().into_iter().map(|m| m.model).collect();
        assert_eq!(names, ["llama3", "old-chat", "unknown"]);
    }

    #[test]
    fn errors_name_the_cause() {
        assert_eq!(
            error_message(r#"{"error":"model 'x' not found"}"#),
            "model 'x' not found"
        );
        assert_eq!(error_message("<html>"), "<html>");
        assert!(matches!(parse("{}"), Err(AppError::Ollama(_))));
    }

    #[tokio::test]
    async fn lists_the_models_of_a_server() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        assert_eq!(list(&server.url()).await.unwrap().len(), 2);
        let empty = FakeOllama::start(Mode::no_models()).await.unwrap();
        assert!(list(&empty.url()).await.unwrap().is_empty());
        let url = empty.url();
        empty.stop().await;
        let Err(AppError::Ollama(message)) = list(&url).await else {
            panic!("expected an Ollama error");
        };
        assert_eq!(message, "can't connect to Ollama");
    }

    /// Contrat : l'instance de test répond toujours comme enregistré.
    #[tokio::test]
    #[ignore = "instance Ollama de test : scripts/test-ollama.sh start"]
    async fn contract_with_the_test_instance() {
        let models = list("http://127.0.0.1:11435").await.unwrap();
        let qwen3 = models.iter().find(|m| m.model == "qwen3:0.6b").unwrap();
        assert!(qwen3.capabilities.contains(&"completion".to_owned()));
        assert!(qwen3.context_length.is_some());
    }

    fn level(name: &str) -> Think {
        Think::Level(name.into())
    }

    fn options(values: Vec<Think>, default: Option<Think>) -> Option<ThinkingOptions> {
        Some(ThinkingOptions { values, default })
    }

    /// Réglages des deux modèles de l'instance de test (Ollama 0.35.1, puis 0.40.0).
    fn on_off_default_on() -> Option<ThinkingOptions> {
        options(
            vec![Think::Enabled(false), Think::Enabled(true)],
            Some(Think::Enabled(true)),
        )
    }

    fn has(details: &ModelDetails, capability: &str) -> bool {
        details.capabilities.iter().any(|c| c == capability)
    }

    #[test]
    fn recorded_details_are_read() {
        let qwen3 = parse_details("qwen3:0.6b", fixtures::SHOW_QWEN3).unwrap();
        assert_eq!(qwen3.model, "qwen3:0.6b");
        assert_eq!(qwen3.capabilities, ["completion", "tools", "thinking"]);
        assert_eq!(qwen3.thinking, on_off_default_on());
        let qwen3_5 = parse_details("qwen3.5:0.8b", fixtures::SHOW_QWEN3_5).unwrap();
        assert!(has(&qwen3_5, "vision") && has(&qwen3_5, "thinking"));
        assert_eq!(qwen3_5.thinking, on_off_default_on());
    }

    /// Pour le moteur de chat : modèle distant, contexte maximal du modèle.
    #[test]
    fn remote_models_and_the_largest_context_are_read() {
        let qwen3 = parse_details("qwen3:0.6b", fixtures::SHOW_QWEN3).unwrap();
        assert_eq!((qwen3.remote, qwen3.max_context), (false, Some(40960)));
        let qwen3_5 = parse_details("qwen3.5:0.8b", fixtures::SHOW_QWEN3_5).unwrap();
        assert_eq!(qwen3_5.max_context, Some(262_144));
        let cloud = parse_details(
            "gpt-oss:20b-cloud",
            r#"{"remote_host":"https://ollama.com:443","remote_model":"gpt-oss:20b"}"#,
        )
        .unwrap();
        assert_eq!((cloud.remote, cloud.max_context), (true, None));
        // Un seul des deux champs, ou seulement le nom, suffit.
        for (model, body) in [
            ("m", r#"{"remote_host":"https://ollama.com:443"}"#),
            ("m", r#"{"remote_model":"gpt-oss:20b"}"#),
            ("gpt-oss:20b-cloud", "{}"),
            ("kimi-k2:cloud", "{}"),
        ] {
            assert!(parse_details(model, body).unwrap().remote, "{model} {body}");
        }
        for body in [
            r#"{"remote_host":"","remote_model":""}"#,
            r#"{"remote_host":null}"#,
            r#"{"remote_host":1,"remote_model":true}"#,
            r#"{"model_info":{"llama.context_length":8192}}"#,
            r#"{"model_info":{"general.architecture":"llama","llama.context_length":"8192"}}"#,
            r#"{"model_info":{"general.architecture":"llama","llama.context_length":0}}"#,
            r#"{"model_info":{"general.architecture":"llama","llama.context_length":99999999999}}"#,
            r#"{"model_info":[]}"#,
        ] {
            let details = parse_details("m", body).unwrap();
            assert_eq!(
                (details.remote, details.max_context),
                (false, None),
                "{body}"
            );
        }
        // Champs propres au moteur : absents de ce que reçoit l'interface.
        let shown = serde_json::to_value(&cloud).unwrap();
        assert!(shown.get("remote").is_none() && shown.get("maxContext").is_none());
    }

    /// Objet `thinking` de cette réponse de `/api/show`.
    fn thinking_of(body: &str) -> Option<ThinkingOptions> {
        parse_details("m", body).unwrap().thinking
    }

    #[test]
    fn thinking_values_are_kept_in_order() {
        // gpt-oss : niveaux seulement.
        assert_eq!(
            thinking_of(r#"{"thinking":{"values":["low","medium","high"],"default":"medium"}}"#),
            options(
                vec![level("low"), level("medium"), level("high")],
                Some(level("medium"))
            )
        );
        // Arrêt et niveaux mêlés.
        assert_eq!(
            thinking_of(r#"{"thinking":{"values":[false,"low","high"],"default":"low"}}"#),
            options(
                vec![Think::Enabled(false), level("low"), level("high")],
                Some(level("low"))
            )
        );
        // Modèle qui ne sait pas raisonner.
        assert_eq!(
            thinking_of(r#"{"thinking":{"values":[false],"default":false}}"#),
            options(vec![Think::Enabled(false)], Some(Think::Enabled(false)))
        );
    }

    #[test]
    fn a_missing_default_is_none() {
        for body in [
            r#"{"thinking":{"values":[true]}}"#,
            r#"{"thinking":{"values":[true],"default":null}}"#,
            r#"{"thinking":{"values":[true],"default":1}}"#,
            r#"{"thinking":{"values":[true],"default":""}}"#,
            r#"{"thinking":{"values":[true],"default":{"level":"high"}}}"#,
        ] {
            assert_eq!(
                thinking_of(body),
                options(vec![Think::Enabled(true)], None),
                "{body}"
            );
        }
    }

    #[test]
    fn values_of_another_type_are_ignored() {
        assert_eq!(
            thinking_of(
                r#"{"thinking":{"values":[1,"high",null,"",{"a":1},["low"],"high",true,true],
                    "default":"high"}}"#
            ),
            options(
                vec![level("high"), Think::Enabled(true)],
                Some(level("high"))
            )
        );
    }

    /// Pas d'objet exploitable : pas de métadonnées, l'interface se fie aux
    /// capacités.
    #[test]
    fn unusable_thinking_objects_count_as_absent() {
        for body in [
            r#"{"capabilities":["completion","thinking"]}"#,
            r#"{"thinking":null}"#,
            r#"{"thinking":"high"}"#,
            r#"{"thinking":{}}"#,
            r#"{"thinking":{"default":true}}"#,
            r#"{"thinking":{"values":true,"default":true}}"#,
            r#"{"thinking":{"values":[],"default":true}}"#,
            r#"{"thinking":{"values":[1,null,""],"default":true}}"#,
        ] {
            assert_eq!(thinking_of(body), None, "{body}");
        }
    }

    #[test]
    fn capabilities_of_an_old_server_are_empty() {
        let details = parse_details("llama3", r#"{"details":{"family":"llama"}}"#).unwrap();
        assert_eq!(
            details,
            ModelDetails {
                model: "llama3".into(),
                capabilities: Vec::new(),
                thinking: None,
                always_thinks: false,
                remote: false,
                max_context: None,
            }
        );
    }

    #[tokio::test]
    async fn details_of_a_model_on_a_server() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        let qwen3 = details(&server.url(), "qwen3:0.6b").await.unwrap();
        assert_eq!(qwen3.model, "qwen3:0.6b");
        assert_eq!(qwen3.thinking, on_off_default_on());
        let levels = details(&server.url(), "chatonlm-levels").await.unwrap();
        assert!(has(&levels, "thinking"));
        assert_eq!(
            levels.thinking,
            options(
                vec![level("low"), level("medium"), level("high")],
                Some(level("medium"))
            )
        );
    }

    #[tokio::test]
    async fn details_errors_are_told_apart() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        let Err(AppError::NotFound(message)) = details(&server.url(), "chatonlm-missing").await
        else {
            panic!("expected a missing model");
        };
        assert_eq!(message, "model 'chatonlm-missing:latest' not found");
        assert!(matches!(
            details(&server.url(), " ").await,
            Err(AppError::Invalid(_))
        ));

        let failing = FakeOllama::start(Mode::Status(500)).await.unwrap();
        let Err(AppError::Ollama(message)) = details(&failing.url(), "qwen3:0.6b").await else {
            panic!("expected an Ollama error");
        };
        assert!(message.starts_with("Ollama answered HTTP 500"), "{message}");
        // Une page vide en 404 (proxy) : le modèle reste nommé.
        let proxy = FakeOllama::start(Mode::Status(404)).await.unwrap();
        let Err(AppError::NotFound(message)) = details(&proxy.url(), "qwen3:0.6b").await else {
            panic!("expected a missing model");
        };
        assert_eq!(message, "model 'qwen3:0.6b' not found");

        let other = FakeOllama::start(Mode::NotOllama).await.unwrap();
        let Err(AppError::Ollama(message)) = details(&other.url(), "qwen3:0.6b").await else {
            panic!("expected an Ollama error");
        };
        assert!(message.starts_with("unexpected model details"), "{message}");

        let url = server.url();
        server.stop().await;
        let Err(AppError::Ollama(message)) = details(&url, "qwen3:0.6b").await else {
            panic!("expected an Ollama error");
        };
        assert_eq!(message, "can't connect to Ollama");
    }

    /// Serveur qui répond à tout « 404 page not found » en texte brut.
    async fn plain_404() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = [0; 4096];
                let _ = socket.read(&mut request).await;
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\n\
                          Content-Length: 18\r\nConnection: close\r\n\r\n404 page not found",
                    )
                    .await;
            }
        });
        format!("http://{address}")
    }

    /// Pour le moteur de chat : serveur injoignable, 404 (message d'Ollama,
    /// vide pour un proxy) et autres échecs restent distincts.
    #[tokio::test]
    async fn show_errors_keep_the_connection_failure_apart() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        let Err(ShowError::NotFound(message)) = show(&server.url(), "chatonlm-missing").await
        else {
            panic!("expected a missing model");
        };
        assert_eq!(message, "model 'chatonlm-missing:latest' not found");
        let proxy = FakeOllama::start(Mode::Status(404)).await.unwrap();
        let Err(ShowError::NotFound(message)) = show(&proxy.url(), "qwen3:0.6b").await else {
            panic!("expected a 404");
        };
        assert_eq!(message, "");
        // Page 404 en texte d'un proxy (Go, gin, Traefik) : « not found »
        // n'y vient pas d'Ollama.
        let page = plain_404().await;
        let Err(ShowError::NotFound(message)) = show(&page, "qwen3:0.6b").await else {
            panic!("expected a 404");
        };
        assert_eq!(message, "");
        let failing = FakeOllama::start(Mode::Status(500)).await.unwrap();
        assert!(matches!(
            show(&failing.url(), "qwen3:0.6b").await,
            Err(ShowError::Other(AppError::Ollama(_)))
        ));
        // Connexion acceptée puis fermée : le serveur est là.
        let closing = FakeOllama::start(Mode::Close).await.unwrap();
        assert!(matches!(
            show(&closing.url(), "qwen3:0.6b").await,
            Err(ShowError::Other(_))
        ));
        let url = server.url();
        server.stop().await;
        let Err(ShowError::Unreachable(AppError::Ollama(message))) = show(&url, "qwen3:0.6b").await
        else {
            panic!("expected an unreachable server");
        };
        assert_eq!(message, "can't connect to Ollama");
    }

    /// Noms recommandés, dans l'ordre.
    fn names(models: &[RecommendedModel]) -> Vec<&str> {
        models.iter().map(|m| m.model.as_str()).collect()
    }

    #[test]
    fn recommendations_keep_cloud_models_with_their_plan() {
        let body = r#"{"recommendations": [
            {"model": "kimi-k2.6:cloud", "description": "x", "context_length": 262144, "required_plan": "pro"},
            {"model": "gemma4:31b-cloud", "required_plan": " Free "},
            {"model": "gpt-oss:120b-cloud"},
            {"model": "glm-5.3:cloud", "required_plan": 3},
            {"model": "gemma4", "vram_bytes": 12000000000, "required_plan": "pro"},
            {"model": "qwen3.5:latest"},
            {"model": "qwen3.5"},
            {"model": "kimi-k2.6:cloud", "required_plan": "free"},
            {"model": "llama3.2:3b", "thinking": {"values": [true]}},
            {"model": ""},
            {"description": "no name"}
        ], "mappings": {"codex": {"model": "gpt-oss:20b"}}}"#;
        let models = parse_recommendations(body).unwrap();
        assert_eq!(
            names(&models),
            [
                "kimi-k2.6:cloud",
                "gemma4:31b-cloud",
                "gpt-oss:120b-cloud",
                "glm-5.3:cloud",
                "gemma4",
                "qwen3.5",
                "llama3.2:3b"
            ]
        );
        let plans: Vec<_> = models.iter().map(|m| m.required_plan.as_deref()).collect();
        // Forfait rogné, en minuscules ; illisible ou d'un modèle local : aucun.
        assert_eq!(
            plans,
            [Some("pro"), Some("free"), None, None, None, None, None]
        );
        // Liste absente ou nulle, comme le décodage de Go : vide.
        for empty in [
            "{}",
            r#"{"recommendations": null}"#,
            r#"{"recommendations": []}"#,
        ] {
            assert!(parse_recommendations(empty).unwrap().is_empty(), "{empty}");
        }
        for unreadable in [
            "<html>",
            r#"{"recommendations": {}}"#,
            r#"{"recommendations": [{"model": 1}]}"#,
        ] {
            assert!(
                matches!(parse_recommendations(unreadable), Err(AppError::Ollama(_))),
                "{unreadable}"
            );
        }
    }

    #[tokio::test]
    async fn recommendations_of_a_server() {
        // Aussi sans modèle installé, comme Ollama.
        for mode in [Mode::ollama(), Mode::no_models()] {
            let server = FakeOllama::start(mode).await.unwrap();
            let models = recommended(&server.url()).await.unwrap();
            assert_eq!(
                names(&models),
                [
                    "kimi-k2.6:cloud",
                    "glm-5.1:cloud",
                    "qwen3.5:cloud",
                    "minimax-m2.7:cloud",
                    "gemma4",
                    "qwen3.5"
                ]
            );
            assert_eq!(models[0].required_plan.as_deref(), Some("pro"));
            assert_eq!(models[2].required_plan.as_deref(), Some("free"));
        }
        // Serveur antérieur à la route : rien à recommander.
        let old = FakeOllama::start(Mode::Status(404)).await.unwrap();
        assert!(recommended(&old.url()).await.unwrap().is_empty());
        let failing = FakeOllama::start(Mode::Status(500)).await.unwrap();
        let Err(AppError::Ollama(message)) = recommended(&failing.url()).await else {
            panic!("expected an Ollama error");
        };
        assert!(message.starts_with("Ollama answered HTTP 500"), "{message}");
        let other = FakeOllama::start(Mode::NotOllama).await.unwrap();
        let Err(AppError::Ollama(message)) = recommended(&other.url()).await else {
            panic!("expected an Ollama error");
        };
        assert!(
            message.starts_with("unexpected model recommendations"),
            "{message}"
        );
        let url = old.url();
        old.stop().await;
        let Err(AppError::Ollama(message)) = recommended(&url).await else {
            panic!("expected an Ollama error");
        };
        assert_eq!(message, "can't connect to Ollama");
    }

    /// Contrat : l'instance de test sert la route des recommandations.
    #[tokio::test]
    #[ignore = "instance Ollama de test : scripts/test-ollama.sh start"]
    async fn contract_recommendations_of_the_test_instance() {
        let base = "http://127.0.0.1:11435";
        let url = format!("{base}/api/experimental/model-recommendations");
        let client = client::client(&url::Url::parse(base).unwrap(), None).unwrap();
        let response = client.get(&url).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert!(body["recommendations"].is_array(), "{body}");
        let models = recommended(base).await.unwrap();
        assert!(!models.is_empty(), "{body}");
        for model in &models {
            assert!(!model.model.ends_with(":latest"), "{model:?}");
            if !is_cloud_name(&model.model) {
                assert_eq!(model.required_plan, None, "{model:?}");
            }
        }
        // Des modèles cloud y sont recommandés, avec le forfait exigé.
        assert!(
            models
                .iter()
                .any(|model| is_cloud_name(&model.model) && model.required_plan.is_some()),
            "{body}"
        );
    }

    #[test]
    fn cloud_names_are_read_as_the_server_reads_them() {
        for cloud in [
            "kimi-k2.6:cloud",
            "gpt-oss:120b-cloud",
            "kimi-k2.6:Cloud",
            "gpt-oss:120B-CLOUD ",
            "hf.co/someone/model:cloud",
        ] {
            assert!(is_cloud_name(cloud), "{cloud}");
        }
        for local in [
            "pointcloud",
            "pointcloud:latest",
            "qwen3:0.6b",
            "cloud",
            "registry.example.com:5000/cloud",
            "model:cloudy",
            "",
        ] {
            assert!(!is_cloud_name(local), "{local}");
        }
    }

    #[test]
    fn cloud_refusals_are_told_apart_for_remote_models_only() {
        assert_eq!(CloudRefusal::of(401, true), Some(CloudRefusal::SignedOut));
        assert_eq!(CloudRefusal::of(402, true), Some(CloudRefusal::UsageLimit));
        for status in [400, 403, 404, 429, 500] {
            assert_eq!(CloudRefusal::of(status, true), None, "{status}");
        }
        // 401 d'un proxy devant un serveur local : pas un compte ollama.com.
        assert_eq!(CloudRefusal::of(401, false), None);
        assert_eq!(CloudRefusal::of(402, false), None);
    }

    /// Contrat : `/api/show` de l'instance de test répond toujours comme
    /// enregistré, réglages du raisonnement compris.
    #[tokio::test]
    #[ignore = "instance Ollama de test : scripts/test-ollama.sh start"]
    async fn contract_details_with_the_test_instance() {
        let base = "http://127.0.0.1:11435";
        for model in ["qwen3:0.6b", "qwen3.5:0.8b"] {
            let found = details(base, model).await.unwrap();
            assert!(
                has(&found, "completion") && has(&found, "thinking"),
                "{model}"
            );
            assert_eq!(found.thinking, on_off_default_on(), "{model}");
        }
        assert!(matches!(
            details(base, "chatonlm-missing").await,
            Err(AppError::NotFound(_))
        ));
    }
}
