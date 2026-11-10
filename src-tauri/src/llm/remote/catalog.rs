//! Modèles d'un fournisseur distant (`GET <base>/v1/models`, API d'OpenAI) :
//! liste relue sans faire confiance au serveur (champs bornés, entrées
//! illisibles écartées), et ce qu'elle dit des capacités.
//!
//! `/v1/models` ne dit rien des capacités chez OpenAI ou Ollama : un modèle
//! est supposé savoir converser et appeler des outils ; la vision seulement
//! si le fournisseur l'annonce (OpenRouter : `architecture.input_modalities`,
//! Mistral : `capabilities.vision`). La réflexion n'est jamais annoncée : ses
//! réglages ne partent pas vers un distant (`inference/dialect.rs`).

use std::time::Duration;

use serde_json::Value;

use crate::llm::ids;
use crate::local_models::ModelCapabilities;
use crate::store::remote::RemoteProviderKind;

/// Réponse lue au plus (la liste d'OpenRouter, plusieurs centaines de
/// modèles décrits, fait quelques Mio).
const MAX_REPLY: usize = 16 * 1024 * 1024;
/// Modèles gardés d'un fournisseur, au plus.
const MAX_MODELS: usize = 2_000;
/// Nom montré, au plus (caractères).
const MAX_NAME: usize = 120;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Modèle annoncé par un fournisseur.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteModel {
    /// Nom chez le fournisseur (`model` des requêtes).
    pub id: String,
    /// Nom montré (OpenRouter : `name`, sinon l'identifiant).
    pub name: String,
    /// Contexte annoncé.
    pub context: Option<u32>,
    pub capabilities: ModelCapabilities,
}

/// Échec d'une requête au fournisseur (sans texte du serveur : codes
/// traduits par la page).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// 401, 403 : clé refusée.
    Key,
    /// 402, 429 : limite atteinte (crédit, cadence).
    Limit,
    /// Connexion impossible ou délai dépassé.
    Unreachable,
    /// 404 : rien à cette adresse (adresse d'un serveur « custom » fausse).
    NotFound,
    /// Autre code d'erreur du serveur.
    Server(u16),
    /// Réponse illisible (pas du JSON, trop grande, pas une liste).
    Malformed,
}

/// Modèles annoncés dans `body`, réponse de `/v1/models`.
pub fn parse(kind: RemoteProviderKind, body: &Value) -> Option<Vec<RemoteModel>> {
    let entries = body["data"].as_array()?;
    let mut models: Vec<RemoteModel> = Vec::new();
    for entry in entries {
        if models.len() == MAX_MODELS {
            break;
        }
        let Some(id) = entry["id"].as_str() else {
            continue;
        };
        // Identifiant que l'app saura relire (`@remote/<p>/<id>`).
        if ids::parse(&ids::remote_id("p", id)).is_none() || models.iter().any(|m| m.id == id) {
            continue;
        }
        if !chats(kind, entry) {
            continue;
        }
        let name = entry["name"]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
            .map(|name| name.chars().take(MAX_NAME).collect())
            .unwrap_or_else(|| id.to_owned());
        let context = ["context_length", "max_context_length"]
            .iter()
            .find_map(|key| entry[key].as_u64())
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0);
        models.push(RemoteModel {
            id: id.to_owned(),
            name,
            context,
            capabilities: capabilities(entry),
        });
    }
    Some(models)
}

/// Le modèle sait converser (Mistral : `completion_chat` ; OpenRouter :
/// texte en sortie) ; inconnu : oui.
fn chats(kind: RemoteProviderKind, entry: &Value) -> bool {
    if kind == RemoteProviderKind::Mistral
        && entry["capabilities"]["completion_chat"].as_bool() == Some(false)
    {
        return false;
    }
    match entry["architecture"]["output_modalities"].as_array() {
        Some(outputs) => outputs.iter().any(|output| output == "text"),
        None => true,
    }
}

fn capabilities(entry: &Value) -> ModelCapabilities {
    let inputs = entry["architecture"]["input_modalities"].as_array();
    let vision = inputs.is_some_and(|inputs| inputs.iter().any(|input| input == "image"))
        || entry["capabilities"]["vision"].as_bool() == Some(true);
    // OpenRouter : paramètres acceptés ; Mistral : `function_calling`.
    let tools = match entry["supported_parameters"].as_array() {
        Some(parameters) => parameters.iter().any(|p| p == "tools"),
        None => entry["capabilities"]["function_calling"]
            .as_bool()
            .unwrap_or(true),
    };
    ModelCapabilities {
        vision,
        tools,
        ..ModelCapabilities::default()
    }
}

/// Client des requêtes aux fournisseurs : proxy du système permis (Internet),
/// connexion bornée, aucune redirection suivie (la clé ne part pas
/// ailleurs).
fn client() -> Result<reqwest::Client, Problem> {
    crate::hub::hf::ensure_crypto_provider();
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("ChatonLM/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| Problem::Unreachable)
}

/// `GET <base><path>` avec la clé, corps JSON lu au plus [`MAX_REPLY`].
async fn get(base: &str, path: &str, key: &str) -> Result<Value, Problem> {
    let mut request = client()?.get(format!("{base}{path}"));
    if !key.is_empty() {
        request = request.bearer_auth(key);
    }
    let mut response = request.send().await.map_err(|_| Problem::Unreachable)?;
    let status = response.status().as_u16();
    match status {
        200..=299 => {}
        401 | 403 => return Err(Problem::Key),
        402 | 429 => return Err(Problem::Limit),
        404 => return Err(Problem::NotFound),
        status => return Err(Problem::Server(status)),
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Problem::Unreachable)? {
        if body.len() + chunk.len() > MAX_REPLY {
            return Err(Problem::Malformed);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| Problem::Malformed)
}

/// Modèles du fournisseur à `base`.
pub async fn fetch(
    kind: RemoteProviderKind,
    base: &str,
    key: &str,
) -> Result<Vec<RemoteModel>, Problem> {
    let body = get(base, "/v1/models", key).await?;
    parse(kind, &body).ok_or(Problem::Malformed)
}

/// Vérifie la clé : OpenRouter publie sa liste sans clé, sa clé se vérifie
/// à part (`/v1/key`) ; ailleurs, la liste demande la clé.
pub async fn check(
    kind: RemoteProviderKind,
    base: &str,
    key: &str,
) -> Result<Vec<RemoteModel>, Problem> {
    if kind == RemoteProviderKind::OpenRouter {
        get(base, "/v1/key", key).await?;
    }
    fetch(kind, base, key).await
}
