//! Mises à jour de ChatonLM (étape 18c), par tauri-plugin-updater.
//!
//! Comme l'app officielle d'Ollama (`app/updater/updater.go`) : vérification
//! 3 s après le démarrage puis toutes les heures, jamais deux à la fois ;
//! réglage « Auto-download updates » (activé par défaut : téléchargement en
//! fond, puis mise à jour « prête » ; coupé : seulement signalée) ; « Restart
//! to update » dans le menu de la zone de notification et notification
//! « Update available » une fois par version (`app/wintray`).
//!
//! Écarts voulus :
//! - point d'accès statique `latest.json` des Releases GitHub (et non
//!   l'API d'ollama.com) ; paquets signés (minisign), signature vérifiée
//!   après le téléchargement, à chaque démarrage et avant l'installation,
//!   avec le nom de fichier signé et le format du paquet (AppImage, .deb) ;
//! - auto-download coupé : la mise à jour est quand même signalée (Settings,
//!   zone de notification) et se télécharge d'un clic ;
//! - mise à jour prête gardée sur disque (`<données>/updates/`, liée au type
//!   de paquet qui l'a téléchargée) : après une relance sans installation,
//!   elle reste prête sans nouveau téléchargement et s'installe même hors
//!   ligne ; jamais installée seule au démarrage (l'officiel le fait quand
//!   l'app démarre masquée) ;
//! - installation par ChatonLM lui-même (pas par le greffon, qui ne sert qu'à
//!   vérifier et télécharger) : AppImage remplacée sur place (sans mot de
//!   passe), .deb par `pkexec dpkg -i` seulement (boîte de mot de passe du
//!   système, aucun autre repli) ; un type de paquet que ChatonLM ne sait pas
//!   mettre à jour (build sans paquet, paquet d'une distribution, Flatpak,
//!   Snap, .deb sans pkexec) ou un dossier de l'AppImage non inscriptible est
//!   dit (état `Unsupported`), avant tout téléchargement ;
//! - état `Installing` (services arrêtés, boîte de mot de passe ouverte) ;
//!   échec d'une vérification de fond ou refus de « Restart to update » qui
//!   ne change pas l'état : `lastError` ;
//! - pas d'icône « mise à jour » dans la zone de notification ; la
//!   notification n'a pas d'action (l'officiel installe au clic).
//!
//! Sous Windows (`install/nsis.rs`, `install/windows.rs`) : ChatonLM posé par
//! son installateur NSIS lance l'installateur vérifié de la nouvelle version
//! (fenêtre de progression, sans question), puis se ferme ; l'installateur
//! le relance. Une copie qui ne vient pas de l'installateur, ou un dossier
//! d'installation non inscriptible, est dit (`Unsupported`).

mod engine;
mod install;
mod staging;

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::Store;

pub(crate) use engine::{Engine, Trigger};
use install::Support;
pub use install::forget_foreign_appimage;

/// Événement envoyé à la page à chaque changement de [`UpdateStatus`].
pub const STATUS_EVENT: &str = "update-status";

/// Première vérification après le démarrage (`UpdateCheckInitialDelay`).
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(3);
/// Vérifications suivantes (`UpdateCheckInterval`).
const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Délai de la lecture de `latest.json`.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// Connexion, puis silence maximal entre deux morceaux d'un téléchargement
/// (le greffon ne borne pas sa durée totale : un paquet est gros).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Remplace le point d'accès de production (`plugins.updater.endpoints`)
/// dans les builds de debug et d'essai e2e (faux serveur de mises à jour) ;
/// jamais lue en production.
#[cfg(any(debug_assertions, feature = "e2e"))]
pub const UPDATE_URL_ENV: &str = "CHATONLM_UPDATE_URL";

/// Route des réglages qui montrent la carte « Updates » (onglet General ;
/// `card=updates` : la carte défile dans la vue et prend le focus).
pub const UPDATES_SETTINGS_ROUTE: &str = "/settings?section=general&card=updates";

/// État des mises à jour, envoyé à la page (`update_status`, événement
/// `update-status`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateStatus {
    pub state: UpdateState,
    /// Réglage « Auto-download updates ».
    pub auto_download: bool,
    /// Dernière vérification réussie (ms depuis l'époque Unix) ; `None` :
    /// aucune depuis le démarrage.
    pub checked_at: Option<i64>,
    /// Manière d'installer : AppImage remplacée sans mot de passe, .deb par
    /// la boîte de mot de passe du système (à dire avant), installateur de
    /// Windows ; `None` : ChatonLM ne sait pas se mettre à jour (voir
    /// `Unsupported`).
    pub installer: Option<UpdateInstaller>,
    /// Dernier échec qui n'a pas changé l'état : vérification qui échoue
    /// pendant qu'une mise à jour est annoncée, en téléchargement ou prête
    /// (code `check`), refus de « Restart to update » (code `install` ;
    /// l'état reste `ready`). Vidé par la vérification réussie suivante et
    /// quand l'état change de nature ; jamais en même temps qu'un état
    /// `error`.
    pub last_error: Option<UpdateErrorInfo>,
}

/// Échec à montrer sous l'état ; `detail` : texte traduit (refus) ou
/// message technique (greffon, système).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateErrorInfo {
    pub code: UpdateErrorCode,
    pub detail: String,
}

/// Machine d'états des mises à jour.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum UpdateState {
    /// Rien de vérifié depuis le démarrage.
    Idle,
    Checking,
    /// Aucune version plus récente (`checkedAt` : quand).
    UpToDate,
    /// Version plus récente annoncée, pas encore téléchargée (auto-download
    /// coupé, ou téléchargement sur le point de commencer). `notes` : les
    /// nouveautés (texte brut) ; `date` : RFC 3339.
    Available {
        version: String,
        notes: Option<String>,
        date: Option<String>,
    },
    /// Octets reçus ; `total` inconnu sans `Content-Length`.
    Downloading {
        version: String,
        received: u64,
        total: Option<u64>,
    },
    /// Téléchargée et vérifiée : « Restart to update ».
    Ready {
        version: String,
        notes: Option<String>,
    },
    /// « Restart to update » accepté : services arrêtés, installation en
    /// cours (pour un .deb, boîte de mot de passe du système ouverte), puis
    /// relance. Plus aucune action possible.
    Installing {
        version: String,
    },
    /// Échec ; `detail` : texte de ChatonLM (traduit) ou message technique
    /// (anglais, celui du greffon ou du système), à montrer sous le texte
    /// traduit du code. Un nouvel essai est une vérification
    /// (`check_for_updates`).
    Error {
        code: UpdateErrorCode,
        detail: String,
    },
    /// ChatonLM ne peut pas s'installer lui-même ici ; `version` : celle
    /// annoncée, si une vérification a eu lieu.
    Unsupported {
        reason: UnsupportedReason,
        version: Option<String>,
    },
}

/// Étape qui a échoué.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum UpdateErrorCode {
    /// Lecture de `latest.json` (réseau, serveur, réponse illisible).
    Check,
    /// Téléchargement, ou écriture de la mise à jour sur le disque.
    Download,
    /// Signature fausse, ou paquet qui n'est pas celui attendu (nom signé,
    /// format) : rien n'est gardé.
    Signature,
    /// Mise à jour gardée illisible (fichier supprimé, disque) : rien n'est
    /// gardé, une vérification la retélécharge.
    Io,
    /// Installation (mot de passe refusé, paquet refusé, disque...), ou
    /// refus de « Restart to update » (`lastError`).
    Install,
}

/// Pourquoi ChatonLM ne peut pas se mettre à jour lui-même.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum UnsupportedReason {
    /// Build sans paquet (développement, compilation à la main, AppImage
    /// extraite) : rien à remplacer.
    #[cfg_attr(windows, allow(dead_code))]
    NoPackage,
    /// Installé par un paquet de la distribution (ou un .deb que dpkg ne
    /// connaît pas, ou sans pkexec), un Flatpak ou un Snap : les mises à
    /// jour passent par le gestionnaire de paquets.
    #[cfg_attr(windows, allow(dead_code))]
    PackageManager,
    /// Dossier de l'AppImage non inscriptible : la déplacer dans un dossier à
    /// soi (ex. ~/Applications), puis réessayer.
    #[cfg_attr(windows, allow(dead_code))]
    ReadOnlyFolder,
    /// Windows : cette copie n'a pas été posée par l'installateur de ChatonLM
    /// (dossier copié, extraction, build sans paquet) : l'installateur de la
    /// nouvelle version écrirait ailleurs.
    #[cfg_attr(not(windows), allow(dead_code))]
    NotInstalled,
    /// Windows : dossier d'installation non inscriptible (installé en
    /// administrateur dans un dossier du système) : réinstaller dans un
    /// dossier à soi.
    #[cfg_attr(not(windows), allow(dead_code))]
    InstallFolderReadOnly,
}

/// Manière d'installer une mise à jour (aussi notée avec la mise à jour
/// gardée sur disque).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum UpdateInstaller {
    /// AppImage remplacée sur place, sans mot de passe.
    AppImage,
    /// Paquet .deb installé par `pkexec dpkg -i`, après la boîte de mot de
    /// passe du système.
    Deb,
    /// Windows : installateur NSIS de la nouvelle version, lancé avec une
    /// fenêtre de progression, sans question ni mot de passe ; il relance
    /// ChatonLM.
    Nsis,
}

/// Versions installées (Settings › General).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppVersions {
    pub chatonlm: String,
    /// Ollama intégré (fichier `ollama/VERSION` des ressources, livré avec
    /// les paquets) ; `None` : pas d'Ollama intégré.
    pub ollama: Option<String>,
}

/// Moteur des mises à jour, géré par Tauri une fois l'app prête.
pub struct Updates(pub(crate) Arc<Engine>);

/// Entrées de la zone de notification selon l'état.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrayUpdate {
    #[default]
    None,
    /// « An update is available » (ouvre les réglages).
    Available,
    /// « An update is available » (grisée) et « Restart to update ».
    Ready,
    /// Les deux grisées : installation en cours.
    Installing,
}

impl TrayUpdate {
    pub fn of(state: &UpdateState) -> Self {
        match state {
            UpdateState::Ready { .. } => TrayUpdate::Ready,
            UpdateState::Installing { .. } => TrayUpdate::Installing,
            UpdateState::Available { .. }
            | UpdateState::Downloading { .. }
            | UpdateState::Unsupported {
                version: Some(_), ..
            } => TrayUpdate::Available,
            _ => TrayUpdate::None,
        }
    }
}

/// Entrées de la zone de notification pour l'état actuel (`None` avant le
/// démarrage du moteur).
pub fn tray_update(app: &AppHandle) -> TrayUpdate {
    app.try_state::<Updates>()
        .map(|updates| TrayUpdate::of(&updates.0.status().state))
        .unwrap_or_default()
}

/// Démarrage (fin du `setup`) : moteur géré, mise à jour gardée sur disque
/// relue, puis vérifications 3 s après puis toutes les heures ; chaque
/// changement part vers la page, la zone de notification et, une mise à jour
/// prête, la notification (une fois par version).
pub fn setup(app: &AppHandle) {
    let store = app.try_state::<Store>().map(|store| store.inner().clone());
    let saved = store
        .as_ref()
        .and_then(|store| store.blocking(crate::store::updates::get).ok())
        .unwrap_or_default();
    let product = app
        .config()
        .product_name
        .clone()
        .unwrap_or_else(|| app.package_info().name.clone());
    let support = install::detect_current(&product);
    let updater = match &support {
        Support::Installable(_) => match build_updater(app) {
            Ok(updater) => Some(updater),
            Err(error) => {
                eprintln!("chatonlm: updater unavailable: {error}");
                None
            }
        },
        Support::Unsupported(_) => None,
    };
    let data_dir = crate::paths::data_dir(app).unwrap_or_default();
    let staging = data_dir.join("updates");
    let platform = install::Platform::current(support, &data_dir);
    // Windows : installateur de la mise à jour précédente supprimé.
    platform.startup_cleanup();
    let config = updater_config(app);
    let engine = Arc::new(Engine::new(engine::Config {
        updater,
        support,
        platform,
        staging,
        pubkey: config.pubkey,
        require_signed_version: config.require_signed_version,
        current: app.package_info().version.clone(),
        auto_download: saved.auto_download,
    }));
    app.manage(Updates(Arc::clone(&engine)));

    forward(app, &engine, store, saved.notified_version);

    tauri::async_runtime::spawn(async move {
        // Mise à jour gardée d'une session précédente : relue (signature)
        // avant la première vérification.
        engine.restore().await;
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            engine.check(Trigger::Scheduled).await;
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// Greffon configuré : délais du client, et faux serveur des essais.
fn build_updater(app: &AppHandle) -> tauri_plugin_updater::Result<tauri_plugin_updater::Updater> {
    #[allow(unused_mut)]
    let mut builder = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        .configure_client(|client| {
            client
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(READ_TIMEOUT)
        });
    #[cfg(any(debug_assertions, feature = "e2e"))]
    if let Some(url) = std::env::var(UPDATE_URL_ENV)
        .ok()
        .filter(|url| !url.is_empty())
    {
        let url = url::Url::parse(&url).map_err(tauri_plugin_updater::Error::UrlParse)?;
        builder = builder.endpoints(vec![url])?;
    }
    builder.build()
}

/// `plugins.updater` de la configuration (clé publique, signature liée à la
/// version).
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct PluginConfig {
    pubkey: String,
    require_signed_version: bool,
}

fn updater_config(app: &AppHandle) -> PluginConfig {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

/// Suit le moteur : page (événement), zone de notification, notification
/// d'une mise à jour prête (une fois par version, retenue dans la base).
fn forward(
    app: &AppHandle,
    engine: &Arc<Engine>,
    store: Option<Store>,
    mut notified: Option<String>,
) {
    let mut status = engine.subscribe();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut tray = TrayUpdate::None;
        loop {
            let current = status.borrow_and_update().clone();
            let _ = app.emit(STATUS_EVENT, &current);
            let wanted = TrayUpdate::of(&current.state);
            if wanted != tray {
                tray = wanted;
                crate::desktop::update_changed(&app);
            }
            if let UpdateState::Ready { version, .. } = &current.state
                && notified.as_deref() != Some(version.as_str())
            {
                notified = Some(version.clone());
                notify_ready(&app, version).await;
                if let Some(store) = &store {
                    let version = version.clone();
                    if let Err(error) = store
                        .call(move |connection| {
                            crate::store::updates::set_notified_version(connection, &version)
                        })
                        .await
                    {
                        eprintln!("chatonlm: cannot save the update notification: {error}");
                    }
                }
            }
            if status.changed().await.is_err() {
                break;
            }
        }
    });
}

/// Notification « Update available » (textes de `app/wintray/messages.go`).
async fn notify_ready(app: &AppHandle, version: &str) {
    let identifier = app.config().identifier.clone();
    let icon = crate::paths::notification_icon(app)
        .ok()
        .filter(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned());
    let mut notification = notify_rust::Notification::new();
    notification
        .appname("ChatonLM")
        .summary(&t!("updates.notification.summary"))
        .body(&t!("updates.notification.body", version = version))
        .icon(icon.as_deref().unwrap_or(&identifier));
    #[cfg(target_os = "linux")]
    {
        notification.hint(notify_rust::Hint::DesktopEntry(identifier.clone()));
        if let Err(error) = notification.show_async().await {
            eprintln!("chatonlm: cannot show the update notification: {error}");
        }
    }
    // Windows : notification au nom de l'app (identifiant du raccourci posé
    // par l'installateur), envoyée hors du fil asynchrone.
    #[cfg(windows)]
    {
        notification.app_id(&identifier);
        let shown = tauri::async_runtime::spawn_blocking(move || {
            notification
                .show()
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
        .await;
        if let Ok(Err(error)) = shown {
            eprintln!("chatonlm: cannot show the update notification: {error}");
        }
    }
}

/// Moteur géré (après le `setup`).
fn engine(app: &AppHandle) -> AppResult<Arc<Engine>> {
    app.try_state::<Updates>()
        .map(|updates| Arc::clone(&updates.0))
        .ok_or_else(|| AppError::Internal("updater not started".into()))
}

pub fn status(app: &AppHandle) -> AppResult<UpdateStatus> {
    Ok(engine(app)?.status())
}

/// Vérification demandée (bouton, Retry) : attend la fin de celle en cours
/// plutôt que d'en lancer une seconde.
pub async fn check(app: &AppHandle) -> AppResult<UpdateStatus> {
    Ok(engine(app)?.check(Trigger::Requested).await)
}

/// Téléchargement demandé (auto-download coupé) : commence en fond.
pub fn download(app: &AppHandle) -> AppResult<UpdateStatus> {
    engine(app)?
        .download()
        .map_err(|_| AppError::Invalid(t!("updates.nothingToDownload")))
}

/// Réglage « Auto-download updates » : enregistré, puis appliqué (activé
/// avec une mise à jour signalée : elle se télécharge).
pub async fn set_auto_download(app: &AppHandle, enabled: bool) -> AppResult<UpdateStatus> {
    if let Some(store) = app.try_state::<Store>() {
        store
            .call(move |connection| crate::store::updates::set_auto_download(connection, enabled))
            .await?;
    }
    Ok(engine(app)?.set_auto_download(enabled))
}

/// « Restart to update » : mise à jour prête relue (signature, nom signé,
/// format ; sans réseau), état `Installing`, services arrêtés proprement
/// (réponses, téléchargements, dictée, Ollama intégré), installation, puis
/// relance. Un refus avant l'arrêt des services laisse l'état et remplit
/// `lastError`. Si l'installation échoue une fois les services arrêtés
/// (boîte de mot de passe fermée...), ChatonLM est relancé quand même (la
/// même version), et l'erreur est montrée au démarrage. Sous Windows,
/// l'installateur lancé, ChatonLM se ferme : c'est l'installateur qui le
/// relance.
pub async fn restart_to_update(app: &AppHandle) -> AppResult<()> {
    let engine = engine(app)?;
    let prepared = engine
        .prepare_install()
        .await
        .map_err(|refusal| AppError::Invalid(refusal_text(&refusal)))?;
    crate::stop_services(app).await;
    let result = engine.install(prepared).await;
    if result.is_ok() && install::INSTALLER_RELAUNCHES {
        app.exit(0);
    } else {
        app.request_restart();
    }
    result.map_err(|detail| AppError::System(t!("updates.installFailed", error = detail)))
}

/// Texte d'un refus de « Restart to update » (réponse de la commande, et
/// `lastError`), dans la langue du moment.
fn refusal_text(refusal: &engine::Refusal) -> String {
    match refusal {
        engine::Refusal::NotReady => t!("updates.notReady"),
        engine::Refusal::Busy => t!("updates.installing"),
        engine::Refusal::ReadOnlyFolder => install::Platform::read_only_text(),
        engine::Refusal::Failed(detail) => t!("updates.installFailed", error = detail),
    }
}

/// Versions installées : ChatonLM, et l'Ollama intégré s'il est livré.
pub fn versions(app: &AppHandle) -> AppVersions {
    let ollama = app
        .path()
        .resource_dir()
        .ok()
        .and_then(|dir| std::fs::read_to_string(dir.join("ollama").join("VERSION")).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());
    AppVersions {
        chatonlm: app.package_info().version.to_string(),
        ollama,
    }
}
