//! Accès aux modèles de langage pour toute l'app (contrat 102 § 1.2) :
//! modèles locaux du moteur llama.cpp ([`crate::engine`]), modèles distants
//! (crochet [`remote::RemoteModels`], lot L7), réglages lus à chaque appel,
//! et point d'appel prêt ([`Session`]) pour le client d'inférence
//! ([`crate::inference`]).
//!
//! - [`ids`] : identifiants (local, `@remote/<fournisseur>/<modèle>`) ;
//!   [`models`] : façade du sélecteur et de l'onglet Models ; [`errors`] :
//!   erreurs et leur code pour la page ; [`defaults`] : réglages repris
//!   d'Ollama appliqués aux requêtes.
//! - [`Access`] est un état Tauri (cloné à volonté). Les appelants qui n'en
//!   ont pas encore (Étude, jusqu'au lot L5a) passent par [`access_for`].

// Façade posée par le socle, branchée par les lots L5a, L7 et L9 : une
// partie n'a pas encore d'appelant.
#![allow(dead_code, reason = "branché par les lots L5a, L7 et L9")]

pub mod defaults;
pub mod errors;
pub mod ids;
pub mod models;
pub mod remote;
#[cfg(test)]
pub(crate) mod testing;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use tokio_util::sync::CancellationToken;

use crate::engine::{Engine, KeepAlive, Lease, LoadOptions};
use crate::error::AppResult;
use crate::hub::downloads::HubDownloads;
use crate::inference::{self, ChatRequest, Endpoint, InferenceError, Message};
use crate::local_models::ModelCapabilities;
use crate::relay::RelayClient;
use crate::store::Store;
use crate::store::model_import::ModelDefaults;
pub use errors::LlmError;
use ids::Target;
use remote::RemoteModels;

/// Accès aux modèles (état Tauri ; `Arc` dedans).
#[derive(Clone)]
pub struct Access {
    inner: Arc<Inner>,
}

struct Inner {
    engine: Engine,
    store: Store,
    data_dir: PathBuf,
    remote: Arc<dyn RemoteModels>,
    relay: Arc<RelayClient>,
    downloads: HubDownloads,
}

/// Réglages de l'app qui comptent pour une requête, lus à chaque appel
/// (remplace `ollama::target::request_settings`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Prefs {
    /// Contexte réglé (0 : défaut du modèle, puis du moteur).
    pub context_length: u32,
    /// Durée en mémoire (secondes ; 0 : défaut, négatif : toujours).
    pub keep_alive: i64,
    /// Modèles distants permis (réglage `remoteModels`).
    pub remote_models: bool,
    /// Recherche web activée (réglage `webSearchEnabled`).
    pub web_search: bool,
}

impl Prefs {
    pub fn of(settings: &crate::store::settings::Settings) -> Prefs {
        Prefs {
            context_length: settings.context_length,
            keep_alive: settings.keep_alive,
            remote_models: settings.remote_models,
            web_search: settings.web_search_enabled,
        }
    }
}

/// Options d'une ouverture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenOptions {
    /// Contexte imposé par l'appelant (FIM, visite) ; `None` : réglage, puis
    /// défaut repris du modèle, puis moteur.
    pub ctx: Option<u32>,
    /// `None` : réglage `keep_alive`.
    pub keep_alive: Option<KeepAlive>,
    pub cpu_only: bool,
}

/// Point d'appel prêt pour des requêtes : un modèle local tenu par le
/// moteur (son `Lease` vit autant que la session), ou un modèle distant.
pub struct Session {
    /// Identifiant canonique (base locale, ou `@remote/…`).
    pub id: String,
    /// Local : `127.0.0.1` et la clé du processus ; distant : le fournisseur.
    pub endpoint: Endpoint,
    /// Contexte du processus (`-c`) ; distant : contexte annoncé, sinon
    /// `None`.
    pub context: Option<u32>,
    pub capabilities: ModelCapabilities,
    pub remote: bool,
    /// Sur le processeur après l'échec de la carte.
    pub cpu_fallback: bool,
    /// Réglages repris d'Ollama (modèle local importé).
    defaults: Option<ModelDefaults>,
    /// Nom du modèle chez le fournisseur (distant), écrit dans les requêtes.
    model: Option<String>,
    lease: Option<Lease>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("endpoint", &self.endpoint)
            .field("context", &self.context)
            .field("remote", &self.remote)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// Session d'un modèle distant (crochet [`remote::RemoteModels`]) :
    /// `id` = `@remote/<fournisseur>/<modèle>`, `model` = nom du
    /// fournisseur, `context` = contexte annoncé.
    pub fn remote(
        id: String,
        model: String,
        endpoint: Endpoint,
        context: Option<u32>,
        capabilities: ModelCapabilities,
    ) -> Session {
        Session {
            id,
            endpoint,
            context,
            capabilities,
            remote: true,
            cpu_fallback: false,
            defaults: None,
            model: Some(model),
            lease: None,
        }
    }

    /// Requête avec `model` (distant : nom du fournisseur ; local : rien) et
    /// les réglages repris du modèle ([`Session::apply_defaults`]).
    pub fn request(&self, messages: Vec<Message>) -> ChatRequest {
        let mut request = ChatRequest {
            model: self.model.clone(),
            ..ChatRequest::new(messages)
        };
        self.apply_defaults(&mut request);
        request
    }

    /// Réglages repris d'Ollama ([`defaults::apply`]) : un champ déjà posé
    /// par l'appelant gagne.
    pub fn apply_defaults(&self, request: &mut ChatRequest) {
        if let Some(defaults) = &self.defaults {
            defaults::apply(defaults, request);
        }
    }

    /// Le moteur a déchargé CE processus à la demande (bouton, suppression,
    /// sortie) : une coupure est un arrêt, pas une erreur. Toujours faux pour
    /// un distant.
    pub fn unloaded(&self) -> bool {
        self.lease.as_ref().is_some_and(Lease::unloaded)
    }

    /// Classe une erreur d'une requête de cette session : coupure d'un
    /// modèle déchargé à la demande → [`LlmError::Cancelled`] ; distant :
    /// 401/403 → clé, 402/429 → limite, connexion ou délai → injoignable.
    pub fn error(&self, error: InferenceError) -> LlmError {
        if error == InferenceError::Cancelled {
            return LlmError::Cancelled;
        }
        if !self.remote {
            let cut = matches!(
                error,
                InferenceError::Interrupted(_) | InferenceError::Connect(_)
            );
            return if cut && self.unloaded() {
                LlmError::Cancelled
            } else {
                LlmError::Inference(error)
            };
        }
        let provider = match ids::parse(&self.id) {
            Some(Target::Remote { provider, .. }) => provider,
            _ => self.id.clone(),
        };
        match error {
            InferenceError::Unauthorized
            | InferenceError::Http {
                status: 401 | 403, ..
            } => LlmError::RemoteKey { provider },
            InferenceError::Http {
                status: 402 | 429, ..
            } => LlmError::RemoteLimit { provider },
            InferenceError::Connect(_) | InferenceError::Timeout => {
                LlmError::RemoteUnreachable { provider }
            }
            error => LlmError::Inference(error),
        }
    }
}

impl Access {
    pub fn new(
        engine: Engine,
        store: Store,
        data_dir: PathBuf,
        remote: Arc<dyn RemoteModels>,
        relay: Arc<RelayClient>,
        downloads: HubDownloads,
    ) -> Self {
        Access {
            inner: Arc::new(Inner {
                engine,
                store,
                data_dir,
                remote,
                relay,
                downloads,
            }),
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.inner.engine
    }

    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    /// Dossier des données de l'app.
    pub fn data_dir(&self) -> &Path {
        &self.inner.data_dir
    }

    pub fn relay(&self) -> &Arc<RelayClient> {
        &self.inner.relay
    }

    pub fn downloads(&self) -> &HubDownloads {
        &self.inner.downloads
    }

    pub fn remote(&self) -> &Arc<dyn RemoteModels> {
        &self.inner.remote
    }

    /// Réglages lus à chaque appel.
    pub async fn prefs(&self) -> AppResult<Prefs> {
        self.store()
            .call(|connection| {
                crate::store::settings::get(connection, None).map(|settings| Prefs::of(&settings))
            })
            .await
    }

    /// Point d'appel prêt pour `id`. Local : modèle chargé (ou relancé) par
    /// le moteur avec le contexte de `options`, sinon du réglage, sinon du
    /// modèle ; `on_loading` est appelé une fois, AVANT d'attendre, si le
    /// modèle n'est pas déjà prêt avec ces options (jamais pour un distant).
    /// Distant : refusé ([`LlmError::RemoteOff`]) sans rien appeler si le
    /// réglage est coupé. Annulation : [`LlmError::Cancelled`] (le
    /// chargement continue dans le moteur, comme `Engine::ensure`).
    pub async fn open(
        &self,
        id: &str,
        options: OpenOptions,
        cancel: &CancellationToken,
        on_loading: &(dyn Fn() + Send + Sync),
    ) -> Result<Session, LlmError> {
        let prefs = self.prefs().await.map_err(store_error)?;
        if let Some(Target::Remote { provider, model }) = ids::parse(id) {
            if !prefs.remote_models {
                return Err(LlmError::RemoteOff);
            }
            return tokio::select! {
                biased;
                () = cancel.cancelled() => Err(LlmError::Cancelled),
                session = self.remote().open(&provider, &model) => session,
            };
        }
        if ids::is_remote(id) {
            // `@remote/…` mal formé : jamais au moteur local.
            return Err(if prefs.remote_models {
                LlmError::RemoteUnavailable
            } else {
                LlmError::RemoteOff
            });
        }
        let model = crate::local_models::get(self.store(), self.data_dir(), id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| LlmError::NotInstalled(id.to_owned()))?;
        let key = model.id.clone();
        let defaults = self
            .store()
            .call(move |connection| crate::store::model_import::defaults(connection, &key))
            .await
            .ok()
            .flatten();
        let load = LoadOptions {
            ctx: options
                .ctx
                .filter(|ctx| *ctx > 0)
                .or_else(|| defaults::context(prefs.context_length, defaults.as_ref())),
            keep_alive: options
                .keep_alive
                .unwrap_or_else(|| KeepAlive::from_setting(prefs.keep_alive)),
            cpu_only: options.cpu_only,
        };
        if !self.engine().is_ready(&model.id, load).await {
            on_loading();
        }
        let lease = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(LlmError::Cancelled),
            lease = self.engine().ensure(&model.id, load) => lease?,
        };
        Ok(Session {
            id: lease.model.clone(),
            endpoint: lease.endpoint.clone(),
            context: Some(lease.ctx),
            capabilities: model.capabilities,
            remote: false,
            cpu_fallback: lease.cpu_fallback,
            defaults,
            model: None,
            lease: Some(lease),
        })
    }

    /// Accès de secours sans moteur ni fournisseur, sur `store` (transition :
    /// appelants qui n'ont pas encore d'`Access`, essais sur l'ancien chemin
    /// d'Ollama). Rien n'est écrit sous `data_dir` tant qu'aucun modèle n'est
    /// lancé (impossible : pas de binaire).
    pub(crate) fn detached(store: Store, data_dir: PathBuf) -> Access {
        let engine = Engine::new(
            crate::engine::Config::new(None, &data_dir),
            Arc::new(crate::engine::AppModels::new(
                Some(store.clone()),
                data_dir.clone(),
            )),
        );
        let downloads = HubDownloads::with_options(|_| {}, Default::default());
        downloads.attach(store.clone(), data_dir.clone());
        let relay = Arc::new(RelayClient::with_url(
            CLOSED_RELAY.to_owned(),
            data_dir.clone(),
        ));
        Access::new(
            engine,
            store,
            data_dir,
            Arc::new(remote::Unavailable),
            relay,
            downloads,
        )
    }
}

/// Base illisible pendant une ouverture : le moteur ne peut pas servir.
fn store_error(error: crate::error::AppError) -> LlmError {
    LlmError::Engine(crate::engine::EngineError::StartFailed(error.to_string()))
}

/// Relais fermé (port 9, `discard`, jamais servi en local) des accès sans
/// réseau.
const CLOSED_RELAY: &str = "http://127.0.0.1:9";

/// Pose l'accès de l'app une fois la base ouverte (démarrage, ou base
/// récupérée) : état Tauri et accès global ([`access_for`]). Le moteur et
/// la file du ModelHub sont déjà des états de l'app.
pub fn setup(app: &tauri::AppHandle, store: Store) -> AppResult<()> {
    use tauri::Manager;
    if app.try_state::<Access>().is_some() {
        return Ok(());
    }
    let data_dir = crate::paths::data_dir(app)?;
    let engine = app.state::<Engine>().inner().clone();
    let downloads = app.state::<HubDownloads>().inner().clone();
    let relay = Arc::new(RelayClient::new(data_dir.clone()));
    let remote = remote::for_app(&store, &data_dir);
    let access = Access::new(engine, store, data_dir, remote, relay, downloads);
    install(access.clone());
    app.manage(access);
    Ok(())
}

/// Accès de l'app, posé une fois au démarrage (lib.rs).
static INSTALLED: OnceLock<Access> = OnceLock::new();

/// Pose l'accès de l'app (une fois, au démarrage).
pub fn install(access: Access) {
    let _ = INSTALLED.set(access);
}

/// Transition (jusqu'au lot L5a) : accès de l'app pour un appelant qui n'a
/// que la base (Étude) ; hors de l'app (essais), un accès de secours sur
/// cette base ([`Access::detached`]).
pub fn access_for(store: &Store) -> Access {
    INSTALLED
        .get()
        .cloned()
        .unwrap_or_else(|| Access::detached(store.clone(), detached_dir()))
}

/// Dossier de l'accès de secours (rien n'y est écrit) : sous `.tools` du
/// projet pour les essais.
fn detached_dir() -> PathBuf {
    #[cfg(test)]
    {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tools/tmp/detached")
    }
    #[cfg(not(test))]
    {
        std::env::temp_dir().join("chatonlm-detached")
    }
}

/// Appel d'une session (raccourci des lots : flux classé par
/// [`Session::error`]).
pub async fn chat_stream(
    session: &Session,
    request: &ChatRequest,
    cancel: CancellationToken,
) -> Result<inference::ChatStream, LlmError> {
    inference::chat_stream(&session.endpoint, request, cancel)
        .await
        .map_err(|error| session.error(error))
}

#[cfg(test)]
mod tests;
