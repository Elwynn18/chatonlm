//! Client d'inférence de `llama-server` (llama.cpp b12110) : API OpenAI
//! (`/v1/chat/completions`, en flux SSE ou entière), autocomplétion
//! (`/infill`), santé, propriétés et jetons. Sans Tauri ni état de l'app :
//! une adresse ([`Endpoint`]) et des requêtes ; le lancement du serveur et le
//! choix du modèle sont ailleurs (moteur, lot L3). Formats relevés sur le vrai
//! serveur (`.tools/research/99-llamacpp-preuve.md` § 3 et rapport du lot L4) ;
//! faux serveur des essais : crate `fake-llama`.
//!
//! Bornes (le serveur est local, mais ses réponses ne sont pas lues sans
//! limite) : un bloc SSE fait au plus [`MAX_EVENT`] octets, une réponse JSON
//! [`MAX_BODY`], les noms, `id` et arguments recollés des appels d'outils
//! aussi, et un flux porte au plus [`MAX_TOOL_CALLS`] appels.
//!
//! Délais : le serveur ne répond (en-têtes compris) qu'une fois la requête
//! prise par un emplacement (`-np`) ; avant, elle attend en file, ou le
//! rechargement d'un modèle endormi (`--sleep-idle-seconds`), parfois des
//! minutes. Aucun délai n'y est donc mis par défaut (annulation seule ;
//! [`Endpoint::with_header_timeout`] pour en poser un). Une fois le flux
//! commencé, le silence est borné ([`Endpoint::with_idle_timeout`]) : le
//! serveur envoie un ping SSE (`:`) toutes les 30 s pendant un long prompt,
//! qui remet ce délai à zéro.
//!
//! Clé d'API : passée au serveur par la variable d'environnement
//! `LLAMA_API_KEY` (pas par `--api-key`, lisible par tout utilisateur dans la
//! liste des processus). Le serveur coupe la clé aux virgules (plusieurs
//! clés) : elle n'en contient jamais. La générer en hexadécimal (octets
//! aléatoires), ce qui exclut aussi espaces et caractères spéciaux.

// Branché sur le chat, l'Étude, la correction… par les lots L3 et L5 :
// d'ici là, seuls les essais s'en servent. À retirer alors.
#![allow(dead_code)]

use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

mod api;
mod error;
mod sse;
mod stream;
mod types;

#[cfg(test)]
mod tests;

// Réexportations : l'API du module (employée par les lots L3 et L5).
#[allow(unused_imports)]
pub use api::{chat_once, health, infill, props, tokenize};
pub use error::InferenceError;
#[allow(unused_imports)]
pub use stream::{ChatStream, Event, chat_stream};
#[allow(unused_imports)]
pub use types::{
    ChatRequest, ChatResponse, Content, Effort, FinishReason, Health, InfillChunk, InfillRequest,
    InfillResponse, InfillStop, Message, Modalities, Part, Props, ResponseFormat, Role,
    TemplateCaps, Think, Timings, ToolCall, Usage,
};

/// Taille maximale d'un bloc SSE (un morceau de réponse ou un fragment
/// d'appel d'outil en fait quelques centaines d'octets).
pub const MAX_EVENT: usize = 2 * 1024 * 1024;
/// Taille maximale d'une réponse JSON (réponse entière, `/props` avec son
/// gabarit…), et des arguments recollés des appels d'outils.
pub const MAX_BODY: usize = 16 * 1024 * 1024;

/// Appels d'outils au plus dans une réponse (un modèle en fait un ou
/// quelques-uns).
pub const MAX_TOOL_CALLS: usize = 128;

/// Silence toléré par défaut entre deux blocs d'un flux commencé (pings
/// compris) : le serveur envoie un ping toutes les 30 s pendant un long
/// prompt.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Connexion inactive gardée par le client : moins que les 5 s de
/// cpp-httplib (`keep_alive_timeout`), sinon une requête partie au moment où
/// le serveur ferme finit en erreur de connexion.
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(4);

/// Connexion : sous Windows, le refus d'un port local fermé n'arrive
/// qu'après environ 2 s (voir `ollama::client`).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(if cfg!(windows) { 3 } else { 2 });

/// Connexion à un fournisseur distant (modèles distants, contrat 102
/// § 1.2) : Internet, proxy du système compris.
const REMOTE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Langue d'un serveur : `llama-server` (champs propres à llama.cpp permis)
/// ou API d'OpenAI stricte (fournisseurs distants : le corps n'a ni
/// `cache_prompt`, ni `chat_template_kwargs`, ni les réglages propres à
/// llama.cpp ; filtrage écrit par le lot L7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    #[default]
    LlamaServer,
    OpenAi,
}

/// Adresse d'un `llama-server` (ou d'un fournisseur distant) et sa clé
/// d'API.
#[derive(Clone)]
pub struct Endpoint {
    /// `http://127.0.0.1:<port>`, sans barre finale.
    pub base_url: String,
    /// Clé d'API (voir l'en-tête du module : sans virgule, en hexadécimal).
    pub api_key: String,
    /// Silence toléré dans un flux commencé (après ses en-têtes).
    pub idle_timeout: Duration,
    /// Attente des en-têtes d'un flux ; `None` (par défaut) : sans limite,
    /// l'appelant annule (requête en file, modèle qui se réveille).
    pub header_timeout: Option<Duration>,
    /// Langue du serveur ([`Dialect::LlamaServer`] pour [`Endpoint::new`]).
    pub dialect: Dialect,
    client: reqwest::Client,
}

impl std::fmt::Debug for Endpoint {
    /// Sans la clé (journaux).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("base_url", &self.base_url)
            .field("dialect", &self.dialect)
            .finish_non_exhaustive()
    }
}

/// Installe une fois le moteur cryptographique de rustls (ring), exigé par
/// reqwest avant de créer un client (même s'il ne sert pas en HTTP local).
fn ensure_crypto_provider() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

impl Endpoint {
    /// Adresse locale : jamais de proxy (un `HTTP_PROXY` d'entreprise ne doit
    /// pas intercepter 127.0.0.1).
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, InferenceError> {
        ensure_crypto_provider();
        let client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(CONNECT_TIMEOUT)
            .pool_idle_timeout(POOL_IDLE_TIMEOUT)
            .user_agent(concat!("ChatonLM/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| InferenceError::Connect(error.to_string()))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            idle_timeout: IDLE_TIMEOUT,
            header_timeout: None,
            dialect: Dialect::LlamaServer,
            client,
        })
    }

    /// Fournisseur distant (API d'OpenAI) : proxy du système permis,
    /// connexion bornée à 10 s, [`Dialect::OpenAi`].
    pub fn remote(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, InferenceError> {
        ensure_crypto_provider();
        let client = reqwest::Client::builder()
            .connect_timeout(REMOTE_CONNECT_TIMEOUT)
            .pool_idle_timeout(POOL_IDLE_TIMEOUT)
            .user_agent(concat!("ChatonLM/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| InferenceError::Connect(error.to_string()))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            idle_timeout: IDLE_TIMEOUT,
            header_timeout: None,
            dialect: Dialect::OpenAi,
            client,
        })
    }

    pub fn with_idle_timeout(mut self, idle: Duration) -> Self {
        self.idle_timeout = idle;
        self
    }

    pub fn with_header_timeout(mut self, timeout: Duration) -> Self {
        self.header_timeout = Some(timeout);
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.get(self.url(path)).bearer_auth(&self.api_key)
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.post(self.url(path)).bearer_auth(&self.api_key)
    }
}

/// Corps JSON d'une réponse, au plus [`MAX_BODY`] octets ; `Null` s'il est
/// vide ou n'est pas du JSON (une erreur sans corps lisible garde son code).
async fn read_json(
    response: reqwest::Response,
    cancel: &CancellationToken,
) -> Result<Value, InferenceError> {
    let bytes = read_bytes(response, MAX_BODY, cancel).await?;
    Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn read_bytes(
    mut response: reqwest::Response,
    max: usize,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, InferenceError> {
    let too_large = || InferenceError::Malformed(format!("reply larger than {max} bytes"));
    if response
        .content_length()
        .is_some_and(|length| length > max as u64)
    {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(InferenceError::Cancelled),
            chunk = response.chunk() => chunk.map_err(|error| InferenceError::from_body(&error))?,
        };
        let Some(chunk) = chunk else {
            return Ok(bytes);
        };
        if bytes.len() + chunk.len() > max {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
}
