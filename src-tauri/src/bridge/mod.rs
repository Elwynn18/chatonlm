//! Lien VS Code (propre à ChatonLM) : pont local entre l'app et l'extension
//! ChatonLM pour VS Code. L'app garde le cerveau (modèles, historique, voix) ;
//! l'extension converse par ce pont.
//!
//! - Coupé par défaut ; ouvert, un point d'accès local, sans port réseau :
//!   socket Unix réservé à l'utilisateur sous Linux ([`linux`]), tube nommé
//!   protégé sous Windows ([`windows`]). Fichier de découverte `bridge.json`
//!   à côté (ou dans le dossier des données sous Windows), et `launcher.json`
//!   (programme à lancer) dans le dossier des données.
//! - JSON-RPC 2.0 sur la trame de LSP ([`frame`]), liste blanche de méthodes
//!   ([`connection`]).
//! - Seuls les éditeurs appairés par un code montré dans l'app sont servis ;
//!   chaque connexion se prouve dans les deux sens ([`proof`]). Retirer un
//!   éditeur ferme aussi ses connexions.
//! - Par le pont, on converse et on lit l'état ; ni réglage, ni suppression,
//!   ni téléchargement.
//!
//! Modèle de menace : un programme du même compte peut de toute façon lire
//! les données de l'app ; l'appairage sert au consentement et à la
//! révocation. Contrat : `.tools/research/87-lot1a-contract.md`.

mod connection;
mod frame;
pub mod proof;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as os;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use self::windows as os;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;
use ts_rs::TS;

use crate::attachments::Staging;
use crate::chat::ChatEngine;
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::bridge::{self as saved};
use crate::store::{Store, now_ms};
use proof::Pairings;

/// Événement : état du pont ([`BridgeState`]), à chaque changement.
pub const STATE_EVENT: &str = "bridge";

/// Versions du protocole servies (plus ancienne, plus récente).
pub const PROTOCOL: (u32, u32) = (1, 1);

/// Connexions ouvertes au plus (une par fenêtre de VS Code).
const MAX_CONNECTIONS: usize = 32;

/// Début de l'identifiant des flux du pont dans le moteur de chat.
pub const STREAM_PREFIX: &str = "bridge:";

/// Flux d'une connexion.
pub trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}

/// Échec de l'ouverture du point d'accès (sous Windows, seulement `Io`).
#[derive(Debug)]
#[cfg_attr(windows, allow(dead_code))]
pub enum OpenError {
    /// Dossier qui n'est pas à nous (lien, autre propriétaire, fichier).
    Unsafe,
    /// Une autre instance sert déjà ce point d'accès.
    InUse,
    /// Chemin du socket trop long pour `sun_path`.
    PathTooLong,
    Io(std::io::Error),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Unsafe => f.write_str("the folder is not ours"),
            OpenError::InUse => f.write_str("another instance serves it"),
            OpenError::PathTooLong => f.write_str("the socket path is too long"),
            OpenError::Io(error) => write!(f, "{error}"),
        }
    }
}

/// Nom d'un tube nommé Windows (logique pure, essayée sous Linux).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn pipe_name(random: &[u8]) -> String {
    format!(r"\\.\pipe\chatonlm-{}", proof::to_hex(random))
}

/// Liste d'accès d'un tube : protégée (sans héritage), tous les droits à
/// l'utilisateur, à personne d'autre.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn pipe_sddl(sid: &str) -> String {
    format!("D:P(A;;GA;;;{sid})")
}

/// État du point d'accès.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BridgeStatus {
    /// Pont coupé.
    Off,
    /// Point d'accès ouvert.
    Listening,
    /// Pas de dossier d'exécution (`$XDG_RUNTIME_DIR` sous Linux).
    Unavailable,
    /// Ouverture refusée (autre instance, droits).
    Failed,
}

/// Éditeur appairé, tel que la page le montre (jamais son secret).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BridgeClient {
    pub id: String,
    /// Nom donné par l'éditeur (« VS Code », « VSCodium »…).
    pub name: String,
    pub paired_at: i64,
    pub last_seen_at: Option<i64>,
    /// Au moins une connexion authentifiée ouverte.
    pub connected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BridgePairing {
    /// Six chiffres.
    pub code: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BridgeState {
    pub enabled: bool,
    pub status: BridgeStatus,
    pub clients: Vec<BridgeClient>,
    pub pairing: Option<BridgePairing>,
}

/// Ce que le service demande à l'app (remplacé dans les essais).
pub trait Host: Send + Sync + 'static {
    fn engine(&self) -> &ChatEngine;
    /// L'état a changé : à montrer.
    fn changed(&self, state: &BridgeState);
    /// Une conversation a été créée ou modifiée par l'éditeur.
    fn chat_updated(&self, chat_id: &str);
}

/// Où le pont s'installe.
#[derive(Clone, Debug)]
pub struct Config {
    /// Dossier du point d'accès et de `bridge.json` ; `None` : indisponible.
    pub dir: Option<PathBuf>,
    /// Dossier des données de l'app (`launcher.json`).
    pub data_dir: PathBuf,
    /// Programme à relancer (AppImage ou binaire).
    pub launcher: Option<PathBuf>,
    pub app_version: String,
    /// Délai pour s'authentifier après l'ouverture d'une connexion.
    pub auth_timeout: std::time::Duration,
}

impl Config {
    /// Dossiers du système ; `CHATONLM_BRIDGE_DIR` les remplace dans les
    /// builds de débogage et d'essai seulement.
    pub fn for_app(app: &AppHandle) -> AppResult<Self> {
        let data_dir = crate::paths::data_dir(app)?;
        let identifier = app.config().identifier.clone();
        let dir = test_override("CHATONLM_BRIDGE_DIR")
            .map(PathBuf::from)
            .or_else(|| os::discovery_dir(&identifier, &data_dir));
        let launcher = std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::current_exe().ok());
        Ok(Self {
            dir,
            data_dir,
            launcher,
            app_version: app.package_info().version.to_string(),
            auth_timeout: connection::AUTH_TIMEOUT,
        })
    }
}

/// Jamais lue par le build de production : un environnement modifié ne doit
/// pas déplacer le point d'accès.
#[cfg(any(test, debug_assertions, feature = "e2e"))]
fn test_override(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(not(any(test, debug_assertions, feature = "e2e")))]
fn test_override(_name: &str) -> Option<String> {
    None
}

/// Point d'accès ouvert.
struct Listening {
    stop: watch::Sender<bool>,
    /// Distingue cette ouverture (fichier de découverte).
    instance: String,
}

/// Connexion ouverte.
struct Open {
    /// Éditeur authentifié ; `None` avant `auth`.
    client: Option<String>,
    close: watch::Sender<bool>,
}

pub(crate) struct Shared<H: Host> {
    pub(crate) host: H,
    pub(crate) store: Store,
    /// Pièces jointes du pont : à part de celles de la page.
    pub(crate) staging: Staging,
    pub(crate) config: Config,
    pairings: Mutex<Pairings>,
    listening: Mutex<Option<Listening>>,
    status: Mutex<BridgeStatus>,
    connections: Mutex<HashMap<u64, Open>>,
    next_connection: AtomicU64,
    /// Numéro des flux de conversation du pont.
    pub(crate) next_stream: AtomicU64,
    /// Une commande de cycle de vie à la fois.
    changing: tokio::sync::Mutex<()>,
    closed: AtomicBool,
}

/// Service du pont (état géré par Tauri).
pub struct Bridge<H: Host = AppHost>(Arc<Shared<H>>);

impl<H: Host> Clone for Bridge<H> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<H: Host> Bridge<H> {
    pub fn new(host: H, store: Store, config: Config) -> Self {
        Self(Arc::new(Shared {
            host,
            store,
            staging: Staging::default(),
            config,
            pairings: Mutex::new(Pairings::default()),
            listening: Mutex::new(None),
            status: Mutex::new(BridgeStatus::Off),
            connections: Mutex::new(HashMap::new()),
            next_connection: AtomicU64::new(1),
            next_stream: AtomicU64::new(1),
            changing: tokio::sync::Mutex::new(()),
            closed: AtomicBool::new(false),
        }))
    }

    /// Ouvre le point d'accès s'il est activé (démarrage de l'app).
    pub async fn start_saved(&self) {
        let _changing = self.0.changing.lock().await;
        let enabled = self
            .0
            .store
            .call(|c| saved::enabled(c))
            .await
            .unwrap_or(false);
        if enabled && !self.0.closed.load(Ordering::SeqCst) {
            Shared::open(&self.0).await;
            self.0.emit().await;
        }
    }

    pub async fn state(&self) -> AppResult<BridgeState> {
        self.0.state().await
    }

    /// Active ou coupe le pont ; coupé, toutes les connexions sont fermées.
    pub async fn set_enabled(&self, enabled: bool) -> AppResult<BridgeState> {
        let _changing = self.0.changing.lock().await;
        self.0
            .store
            .call(move |c| saved::set_enabled(c, enabled))
            .await?;
        if enabled {
            if self.0.listening.lock().is_none() {
                Shared::open(&self.0).await;
            }
        } else {
            self.0.close();
            self.0.pairings.lock().cancel();
        }
        self.0.emit_now().await
    }

    /// Nouveau code d'appairage (remplace le précédent).
    pub async fn pair(&self) -> AppResult<BridgeState> {
        if *self.0.status.lock() != BridgeStatus::Listening {
            return Err(AppError::Invalid(t!("bridge.off")));
        }
        let pairing = self.0.pairings.lock().start(now_ms());
        // Code expiré : l'état montré change sans autre événement.
        let shared = Arc::clone(&self.0);
        let delay = u64::try_from(pairing.expires_at - now_ms()).unwrap_or(0);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay + 50)).await;
            let expired = {
                let mut pairings = shared.pairings.lock();
                pairings
                    .current(now_ms())
                    .is_none_or(|current| current.code != pairing.code)
            };
            if expired {
                shared.emit().await;
            }
        });
        self.0.emit_now().await
    }

    pub async fn cancel_pairing(&self) -> AppResult<BridgeState> {
        self.0.pairings.lock().cancel();
        self.0.emit_now().await
    }

    /// Oublie un éditeur et ferme ses connexions.
    pub async fn unlink(&self, client_id: String) -> AppResult<BridgeState> {
        let _changing = self.0.changing.lock().await;
        let id = client_id.clone();
        self.0
            .store
            .call(move |c| saved::remove_client(c, &id))
            .await?;
        self.0.close_client(&client_id);
        self.0.emit_now().await
    }

    /// Fin de l'app : point d'accès fermé, fichier de découverte retiré.
    pub fn shutdown(&self) {
        self.0.closed.store(true, Ordering::SeqCst);
        self.0.close();
    }

    #[cfg(test)]
    pub(crate) fn shared(&self) -> &Arc<Shared<H>> {
        &self.0
    }
}

impl<H: Host> Shared<H> {
    async fn state(&self) -> AppResult<BridgeState> {
        let (enabled, clients) = self
            .store
            .call(|c| Ok((saved::enabled(c)?, saved::clients(c)?)))
            .await?;
        let connected: Vec<String> = self
            .connections
            .lock()
            .values()
            .filter_map(|open| open.client.clone())
            .collect();
        let pairing = self
            .pairings
            .lock()
            .current(now_ms())
            .map(|pairing| BridgePairing {
                code: pairing.code.clone(),
                expires_at: pairing.expires_at,
            });
        Ok(BridgeState {
            enabled,
            status: *self.status.lock(),
            clients: clients
                .into_iter()
                .map(|client| BridgeClient {
                    connected: connected.contains(&client.id),
                    id: client.id,
                    name: client.name,
                    paired_at: client.paired_at,
                    last_seen_at: client.last_seen_at,
                })
                .collect(),
            pairing,
        })
    }

    async fn emit_now(&self) -> AppResult<BridgeState> {
        let state = self.state().await?;
        self.host.changed(&state);
        Ok(state)
    }

    pub(crate) async fn emit(&self) {
        let _ = self.emit_now().await;
    }

    /// Ouvre le point d'accès et écrit les fichiers de découverte.
    async fn open(this: &Arc<Self>) {
        let Some(dir) = this.config.dir.clone() else {
            *this.status.lock() = BridgeStatus::Unavailable;
            return;
        };
        let mut listener = match os::open(&dir).await {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("ChatonLM: VS Code bridge not opened: {error}");
                *this.status.lock() = BridgeStatus::Failed;
                return;
            }
        };
        let instance = uuid::Uuid::now_v7().to_string();
        let discovery = serde_json::json!({
            "protocol": [PROTOCOL.0, PROTOCOL.1],
            "endpoint": listener.endpoint(),
            "pid": std::process::id(),
            "app": this.config.app_version,
            "instance": instance,
            "startedAt": now_ms(),
        });
        if let Err(error) = write_private(&dir.join(DISCOVERY), &discovery) {
            eprintln!("ChatonLM: VS Code bridge discovery file not written: {error}");
            listener.close();
            *this.status.lock() = BridgeStatus::Failed;
            return;
        }
        if let Some(launcher) = &this.config.launcher {
            let launcher = serde_json::json!({ "path": launcher });
            let _ = write_private(&this.config.data_dir.join(LAUNCHER), &launcher);
        }
        let (stop, mut stopped) = watch::channel(false);
        *this.listening.lock() = Some(Listening { stop, instance });
        *this.status.lock() = BridgeStatus::Listening;
        let shared = Arc::clone(this);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    // Seul `true` est jamais envoyé ; émetteur perdu : fermé aussi.
                    _ = stopped.changed() => break,
                    accepted = listener.accept() => match accepted {
                        Ok(stream) => Shared::serve(&shared, stream),
                        Err(error) => {
                            eprintln!("ChatonLM: VS Code bridge accept failed: {error}");
                            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        }
                    },
                }
            }
            listener.close();
        });
    }

    /// Ferme le point d'accès et toutes les connexions.
    fn close(&self) {
        if let Some(listening) = self.listening.lock().take() {
            let _ = listening.stop.send(true);
            if let Some(dir) = &self.config.dir {
                remove_discovery(&dir.join(DISCOVERY), &listening.instance);
            }
        }
        *self.status.lock() = BridgeStatus::Off;
        for open in self.connections.lock().values() {
            let _ = open.close.send(true);
        }
    }

    fn close_client(&self, client_id: &str) {
        for open in self.connections.lock().values() {
            if open.client.as_deref() == Some(client_id) {
                let _ = open.close.send(true);
            }
        }
    }

    /// Sert une connexion acceptée, dans sa tâche.
    fn serve(this: &Arc<Self>, stream: Box<dyn Io>) {
        let (close, closed) = watch::channel(false);
        {
            let mut connections = this.connections.lock();
            if connections.len() >= MAX_CONNECTIONS {
                return;
            }
            let id = this.next_connection.fetch_add(1, Ordering::SeqCst);
            connections.insert(
                id,
                Open {
                    client: None,
                    close,
                },
            );
            let shared = Arc::clone(this);
            tauri::async_runtime::spawn(async move {
                connection::serve(&shared, id, stream, closed).await;
                let was_client = shared
                    .connections
                    .lock()
                    .remove(&id)
                    .is_some_and(|open| open.client.is_some());
                if was_client {
                    shared.emit().await;
                }
            });
        }
    }

    /// Connexion authentifiée pour `client` : montrée « connectée ».
    pub(crate) async fn authenticated(&self, connection: u64, client: &str) {
        if let Some(open) = self.connections.lock().get_mut(&connection) {
            open.client = Some(client.to_owned());
        }
        let id = client.to_owned();
        let _ = self
            .store
            .call(move |c| saved::touch_client(c, &id, now_ms()))
            .await;
        self.emit().await;
    }

    /// Code tapé dans l'éditeur.
    pub(crate) fn attempt_pairing(&self, code: &str) -> Result<(), proof::PairError> {
        self.pairings.lock().attempt(code, now_ms())
    }
}

/// Fichier de découverte, à côté du point d'accès.
pub const DISCOVERY: &str = "bridge.json";

/// Programme à lancer, dans le dossier des données.
pub const LAUNCHER: &str = "launcher.json";

/// Écrit un fichier JSON lisible du seul utilisateur, d'un coup (fichier
/// temporaire renommé).
fn write_private(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent"))?;
    std::fs::create_dir_all(dir)?;
    let temporary = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&temporary);
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary)?;
        std::io::Write::write_all(&mut file, value.to_string().as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)
}

/// Retire le fichier de découverte s'il est encore celui de cette ouverture.
fn remove_discovery(path: &Path, instance: &str) {
    let ours = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|value| value["instance"] == instance);
    if ours {
        let _ = std::fs::remove_file(path);
    }
}

/// L'app, pour le service.
pub struct AppHost {
    app: AppHandle,
}

impl Host for AppHost {
    fn engine(&self) -> &ChatEngine {
        self.app.state::<ChatEngine>().inner()
    }

    fn changed(&self, state: &BridgeState) {
        let _ = self.app.emit(STATE_EVENT, state);
    }

    fn chat_updated(&self, chat_id: &str) {
        let _ = self.app.emit(
            crate::easy_link::CHATS_EVENT,
            crate::easy_link::ChatsUpdated {
                chat_id: chat_id.to_owned(),
            },
        );
    }
}

/// Démarre le service : point d'accès ouvert dès le lancement s'il est
/// activé. Sans base lisible, pas de pont tant qu'elle n'est pas récupérée
/// (`startup::recover_database` rappelle cette fonction).
pub fn setup(app: &AppHandle) {
    if app.try_state::<Bridge>().is_some() {
        return;
    }
    let Some(store) = app.try_state::<Store>().map(|store| store.inner().clone()) else {
        return;
    };
    let config = match Config::for_app(app) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("ChatonLM: VS Code bridge unavailable: {error}");
            return;
        }
    };
    let service = Bridge::new(AppHost { app: app.clone() }, store, config);
    app.manage(service.clone());
    tauri::async_runtime::spawn(async move { service.start_saved().await });
}
