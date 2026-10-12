//! Machine d'états des mises à jour, sans l'app autour : vérification et
//! téléchargement (greffon tauri-plugin-updater), mise à jour gardée sur
//! disque, installation (par ChatonLM : `install`). L'état est publié sur un
//! canal `watch` que suit [`super::setup`] (page, zone de notification,
//! notification).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use semver::Version;
use tauri_plugin_updater::{Update, Updater};
use tokio::sync::watch;

use super::install::{Platform, Support};
use super::staging::{self, Failure, ReadError, Staged};
use super::{UpdateErrorCode, UpdateErrorInfo, UpdateInstaller, UpdateState, UpdateStatus};
use crate::i18n::t;

/// Intervalle minimal entre deux publications de la progression.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// Ce que le moteur reçoit au démarrage.
pub(crate) struct Config {
    /// Greffon prêt (`None` : non pris en charge, ou greffon indisponible).
    pub updater: Option<Updater>,
    pub support: Support,
    /// Cibles de l'installation selon le système (AppImage à remplacer et
    /// pkexec ; dossier d'installation sous Windows).
    pub platform: Platform,
    /// Dossier de la mise à jour gardée.
    pub staging: PathBuf,
    /// Clé publique (`plugins.updater.pubkey`) et exigence d'une version
    /// signée, pour revérifier la mise à jour gardée.
    pub pubkey: String,
    pub require_signed_version: bool,
    /// Version de l'app en cours.
    pub current: Version,
    pub auto_download: bool,
}

/// Origine d'une vérification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// Au démarrage puis toutes les heures : ne remplace pas une mise à jour
    /// annoncée ni un échec d'installation ou de signature affiché.
    Scheduled,
    /// Bouton « Check for updates », Retry.
    Requested,
}

/// Pourquoi « Restart to update » ne peut pas installer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    NotReady,
    /// Installation déjà en cours (second clic).
    Busy,
    ReadOnlyFolder,
    /// Mise à jour gardée illisible, de signature fausse ou d'un autre
    /// paquet (supprimée).
    Failed(String),
}

/// Mise à jour relue et vérifiée, prête à être installée.
pub(crate) struct Prepared {
    version: String,
    installer: UpdateInstaller,
    bytes: Vec<u8>,
}

struct Inner {
    state: UpdateState,
    auto_download: bool,
    checked_at: Option<i64>,
    last_error: Option<UpdateErrorInfo>,
    /// Dernière mise à jour annoncée par le serveur (objet du greffon qui
    /// sait la télécharger).
    found: Option<Update>,
    /// Mise à jour gardée sur disque, vérifiée.
    staged: Option<Staged>,
    downloading: bool,
    /// Annonce (version, signature) dont le paquet a été refusé à la
    /// vérification : les vérifications planifiées qui la retrouvent ne la
    /// retéléchargent pas (une vérification demandée, si).
    refused: Option<(String, String)>,
}

pub(crate) struct Engine {
    updater: Option<Updater>,
    support: Support,
    platform: Platform,
    staging: PathBuf,
    pubkey: String,
    require_signed_version: bool,
    current: Version,
    inner: Mutex<Inner>,
    /// Une seule vérification à la fois.
    checking: tokio::sync::Mutex<()>,
    /// « Restart to update » en cours : un second clic ne fait rien.
    installing: std::sync::atomic::AtomicBool,
    status: watch::Sender<UpdateStatus>,
}

impl Engine {
    pub fn new(config: Config) -> Self {
        let state = match config.support {
            Support::Unsupported(reason) => UpdateState::Unsupported {
                reason,
                version: None,
            },
            // Greffon mal configuré : dit, jamais muet.
            Support::Installable(_) if config.updater.is_none() => UpdateState::Error {
                code: UpdateErrorCode::Check,
                detail: t!("updates.notConfigured"),
            },
            Support::Installable(_) => UpdateState::Idle,
        };
        let inner = Inner {
            state,
            auto_download: config.auto_download,
            checked_at: None,
            last_error: None,
            found: None,
            staged: None,
            downloading: false,
            refused: None,
        };
        let status = watch::Sender::new(snapshot(&inner, config.support));
        Self {
            updater: config.updater,
            support: config.support,
            platform: config.platform,
            staging: config.staging,
            pubkey: config.pubkey,
            require_signed_version: config.require_signed_version,
            current: config.current,
            inner: Mutex::new(inner),
            checking: tokio::sync::Mutex::new(()),
            installing: std::sync::atomic::AtomicBool::new(false),
            status,
        }
    }

    pub fn status(&self) -> UpdateStatus {
        snapshot(&self.inner.lock(), self.support)
    }

    pub fn subscribe(&self) -> watch::Receiver<UpdateStatus> {
        self.status.subscribe()
    }

    /// Manière d'installer de ce programme (`None` : non pris en charge).
    fn installer(&self) -> Option<UpdateInstaller> {
        match self.support {
            Support::Installable(installer) => Some(installer),
            Support::Unsupported(_) => None,
        }
    }

    /// Publie l'état (sans doublon).
    fn publish(&self, inner: &Inner) {
        let status = snapshot(inner, self.support);
        self.status.send_if_modified(|current| {
            if *current == status {
                false
            } else {
                *current = status;
                true
            }
        });
    }

    /// Change l'état ; un état d'une autre nature (ou une erreur, qui se
    /// montre elle-même) efface `last_error`.
    fn set_state(&self, inner: &mut Inner, state: UpdateState) {
        if std::mem::discriminant(&inner.state) != std::mem::discriminant(&state)
            || matches!(state, UpdateState::Error { .. })
        {
            inner.last_error = None;
        }
        inner.state = state;
        self.publish(inner);
    }

    /// Note un échec qui ne change pas l'état (jamais sous une erreur).
    fn note_error(&self, inner: &mut Inner, code: UpdateErrorCode, detail: String) {
        if !matches!(inner.state, UpdateState::Error { .. }) {
            inner.last_error = Some(UpdateErrorInfo { code, detail });
            self.publish(inner);
        }
    }

    /// Démarrage : mise à jour gardée d'une session précédente relue. Prête
    /// si elle a été téléchargée par ce type de paquet, que sa signature, son
    /// nom signé et son format sont bons et qu'elle est plus récente que
    /// l'app ; sinon (autre type de paquet, installée, dépassée, abîmée)
    /// supprimée. Un échec d'installation noté avant la relance devient
    /// l'état affiché.
    pub async fn restore(&self) {
        let Some(installer) = self.installer() else {
            return;
        };
        if self.updater.is_none() {
            return;
        }
        let dir = self.staging.clone();
        let pubkey = self.pubkey.clone();
        let require = self.require_signed_version;
        let current = self.current.clone();
        let restored = tokio::task::spawn_blocking(move || {
            let failure = staging::take_failure(&dir);
            let Some(staged) = staging::load(&dir) else {
                staging::clear(&dir);
                return (None, failure);
            };
            let newer = Version::parse(staged.version.trim_start_matches('v'))
                .is_ok_and(|version| version > current);
            if !newer || staged.installer != installer {
                staging::clear(&dir);
                return (None, None);
            }
            match staging::read_verified(&dir, &staged, &pubkey, require) {
                Ok(_) => (Some(staged), failure),
                Err(error) => {
                    eprintln!("chatonlm: kept update refused: {error:?}");
                    staging::clear(&dir);
                    (None, None)
                }
            }
        })
        .await
        .unwrap_or((None, None));

        let mut inner = self.inner.lock();
        if !matches!(inner.state, UpdateState::Idle) {
            return;
        }
        let (staged, failure) = restored;
        let Some(staged) = staged else {
            return;
        };
        let state = match failure {
            Some(failure) if failure.version == staged.version => UpdateState::Error {
                code: UpdateErrorCode::Install,
                detail: failure.detail,
            },
            _ => UpdateState::Ready {
                version: staged.version.clone(),
                notes: staged.notes.clone(),
            },
        };
        inner.staged = Some(staged);
        self.set_state(&mut inner, state);
    }

    /// Vérifie le serveur. Jamais deux à la fois : une demande pendant une
    /// vérification en attend la fin et en rend le résultat. Une mise à jour
    /// annoncée se télécharge aussitôt si l'auto-download est activé.
    pub async fn check(self: &Arc<Self>, trigger: Trigger) -> UpdateStatus {
        let Some(updater) = &self.updater else {
            return self.status();
        };
        let _guard = match self.checking.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                drop(self.checking.lock().await);
                return self.status();
            }
        };
        {
            let mut inner = self.inner.lock();
            if trigger == Trigger::Requested {
                // Retry : l'annonce refusée est retentée.
                inner.refused = None;
            }
            if !busy(&inner) && !keeps_state(&inner.state, trigger) {
                self.set_state(&mut inner, UpdateState::Checking);
            }
        }

        let result = updater.check().await;

        let mut obsolete = false;
        let mut download = false;
        {
            let mut inner = self.inner.lock();
            match result {
                Err(error) => {
                    let detail = plugin_error_text(&error);
                    if matches!(inner.state, UpdateState::Checking) {
                        self.set_state(
                            &mut inner,
                            UpdateState::Error {
                                code: UpdateErrorCode::Check,
                                detail,
                            },
                        );
                    } else {
                        // Mise à jour annoncée, en téléchargement ou prête :
                        // elle reste, l'échec est noté à côté.
                        self.note_error(&mut inner, UpdateErrorCode::Check, detail);
                    }
                }
                Ok(None) => {
                    inner.checked_at = Some(crate::store::now_ms());
                    inner.last_error = None;
                    inner.found = None;
                    inner.refused = None;
                    if inner.downloading || installing(&inner) {
                        self.publish(&inner);
                    } else {
                        obsolete = inner.staged.take().is_some();
                        self.set_state(&mut inner, UpdateState::UpToDate);
                    }
                }
                Ok(Some(update)) => {
                    inner.checked_at = Some(crate::store::now_ms());
                    inner.last_error = None;
                    let version = update.version.clone();
                    let signature = update.signature.clone();
                    let notes = update.body.clone();
                    let date = update
                        .raw_json
                        .get("pub_date")
                        .and_then(|date| date.as_str())
                        .map(str::to_owned);
                    inner.found = Some(update);
                    // Même version ET même signature : une Release republiée
                    // sous le même numéro (nouvelle signature) se
                    // retélécharge.
                    let kept = inner.staged.as_ref().is_some_and(|staged| {
                        staged.version == version && staged.signature == signature
                    });
                    let announce = (version.clone(), signature);
                    let refused_again = inner.refused.as_ref() == Some(&announce);
                    if !refused_again {
                        inner.refused = None;
                    }
                    let install_error_kept = trigger == Trigger::Scheduled
                        && matches!(
                            inner.state,
                            UpdateState::Error {
                                code: UpdateErrorCode::Install,
                                ..
                            }
                        );
                    if inner.downloading || installing(&inner) || install_error_kept {
                        // Téléchargement en cours (même annonce), installation
                        // en cours, ou échec d'installation affiché : rien ne
                        // change.
                        self.publish(&inner);
                    } else if refused_again {
                        // Paquet déjà refusé pour cette même annonce : pas de
                        // nouveau téléchargement (l'erreur reste affichée).
                        self.publish(&inner);
                    } else if kept {
                        self.set_state(&mut inner, UpdateState::Ready { version, notes });
                    } else {
                        obsolete = inner.staged.take().is_some();
                        if let Some(reason) = self.platform.location_problem() {
                            self.set_state(
                                &mut inner,
                                UpdateState::Unsupported {
                                    reason,
                                    version: Some(version),
                                },
                            );
                        } else {
                            download = inner.auto_download;
                            self.set_state(
                                &mut inner,
                                UpdateState::Available {
                                    version,
                                    notes,
                                    date,
                                },
                            );
                        }
                    }
                }
            }
        }
        if obsolete {
            let dir = self.staging.clone();
            let _ = tokio::task::spawn_blocking(move || staging::clear(&dir)).await;
        }
        if download {
            let _ = self.download();
        }
        self.status()
    }

    /// Commence le téléchargement de la mise à jour annoncée (en fond) ;
    /// `Err` s'il n'y en a pas. Dossier de l'AppImage vérifié avant.
    pub fn download(self: &Arc<Self>) -> Result<UpdateStatus, ()> {
        let update = {
            let mut inner = self.inner.lock();
            if inner.downloading
                || installing(&inner)
                || matches!(inner.state, UpdateState::Ready { .. })
            {
                return Ok(snapshot(&inner, self.support));
            }
            let Some(update) = inner.found.clone() else {
                return Err(());
            };
            if let Some(reason) = self.platform.location_problem() {
                self.set_state(
                    &mut inner,
                    UpdateState::Unsupported {
                        reason,
                        version: Some(update.version.clone()),
                    },
                );
                return Ok(snapshot(&inner, self.support));
            }
            inner.downloading = true;
            self.set_state(
                &mut inner,
                UpdateState::Downloading {
                    version: update.version.clone(),
                    received: 0,
                    total: None,
                },
            );
            update
        };
        let engine = Arc::clone(self);
        tokio::spawn(async move { engine.run_download(update).await });
        Ok(self.status())
    }

    /// Téléchargement (signature vérifiée par le greffon), contrôle du nom
    /// signé et du format pour ce type de paquet, puis mise à jour gardée
    /// sur disque.
    async fn run_download(self: Arc<Self>, update: Update) {
        let version = update.version.clone();
        let mut received = 0u64;
        let mut last = Instant::now();
        let result = update
            .download(
                |chunk, total| {
                    received += chunk as u64;
                    if last.elapsed() >= PROGRESS_EVERY {
                        last = Instant::now();
                        let mut inner = self.inner.lock();
                        if inner.downloading {
                            self.set_state(
                                &mut inner,
                                UpdateState::Downloading {
                                    version: version.clone(),
                                    received,
                                    total,
                                },
                            );
                        }
                    }
                },
                || {},
            )
            .await;
        let outcome = match (result, self.installer()) {
            (Ok(bytes), Some(installer)) => {
                let staged =
                    Staged::new(&version, update.body.clone(), &update.signature, installer);
                let dir = self.staging.clone();
                let kept = staged.clone();
                let pubkey = self.pubkey.clone();
                let require = self.require_signed_version;
                let saved = tokio::task::spawn_blocking(move || {
                    staging::verify(
                        &bytes,
                        &kept.signature,
                        &pubkey,
                        &kept.version,
                        require,
                        kept.installer,
                    )
                    .map_err(|detail| (UpdateErrorCode::Signature, detail))?;
                    staging::save(&dir, &kept, &bytes)
                        .map_err(|error| (UpdateErrorCode::Download, error.to_string()))
                })
                .await;
                match saved {
                    Ok(Ok(())) => Ok(staged),
                    Ok(Err(failure)) => Err(failure),
                    Err(error) => Err((UpdateErrorCode::Download, error.to_string())),
                }
            }
            (Ok(_), None) => Err((UpdateErrorCode::Download, t!("updates.notReady"))),
            (Err(error), _) => Err((download_code(&error), plugin_error_text(&error))),
        };
        let mut inner = self.inner.lock();
        inner.downloading = false;
        match outcome {
            Ok(staged) => {
                let notes = staged.notes.clone();
                inner.staged = Some(staged);
                self.set_state(&mut inner, UpdateState::Ready { version, notes });
            }
            Err((code, detail)) => {
                if code == UpdateErrorCode::Signature {
                    inner.refused = Some((version, update.signature.clone()));
                }
                self.set_state(&mut inner, UpdateState::Error { code, detail });
            }
        }
    }

    /// Réglage « Auto-download updates » ; activé avec une mise à jour
    /// signalée : elle se télécharge.
    pub fn set_auto_download(self: &Arc<Self>, enabled: bool) -> UpdateStatus {
        let start = {
            let mut inner = self.inner.lock();
            inner.auto_download = enabled;
            self.publish(&inner);
            enabled && matches!(inner.state, UpdateState::Available { .. })
        };
        if start {
            let _ = self.download();
        }
        self.status()
    }

    /// Avant « Restart to update » (services encore actifs) : mise à jour
    /// prête, dossier inscriptible, paquet relu sur le disque (sans réseau)
    /// et revérifié (signature, nom signé, format). Accepté : état
    /// `Installing`. Refusé sans perdre la mise à jour (pas prête, dossier
    /// non inscriptible) : l'état reste, le refus va dans `lastError`.
    pub async fn prepare_install(self: &Arc<Self>) -> Result<Prepared, Refusal> {
        use std::sync::atomic::Ordering;
        if self.installing.swap(true, Ordering::SeqCst) {
            return Err(Refusal::Busy);
        }
        let prepared = self.prepare().await;
        let mut inner = self.inner.lock();
        match &prepared {
            Ok(prepared) => self.set_state(
                &mut inner,
                UpdateState::Installing {
                    version: prepared.version.clone(),
                },
            ),
            Err(refusal) => {
                self.installing.store(false, Ordering::SeqCst);
                if matches!(refusal, Refusal::NotReady | Refusal::ReadOnlyFolder) {
                    self.note_error(
                        &mut inner,
                        UpdateErrorCode::Install,
                        super::refusal_text(refusal),
                    );
                }
            }
        }
        prepared
    }

    async fn prepare(self: &Arc<Self>) -> Result<Prepared, Refusal> {
        let installer = self.installer().ok_or(Refusal::NotReady)?;
        let staged = {
            let inner = self.inner.lock();
            let UpdateState::Ready { version, .. } = &inner.state else {
                return Err(Refusal::NotReady);
            };
            inner
                .staged
                .clone()
                .filter(|staged| staged.version == *version && staged.installer == installer)
                .ok_or(Refusal::NotReady)?
        };
        if !self.platform.ready(installer) {
            return Err(Refusal::NotReady);
        }
        if self.platform.location_problem().is_some() {
            return Err(Refusal::ReadOnlyFolder);
        }
        let dir = self.staging.clone();
        let pubkey = self.pubkey.clone();
        let require = self.require_signed_version;
        let relu = staged.clone();
        let read = tokio::task::spawn_blocking(move || {
            staging::read_verified(&dir, &relu, &pubkey, require)
        })
        .await
        .unwrap_or_else(|error| Err(ReadError::Io(error.to_string())));
        let (code, detail) = match read {
            Ok(bytes) => {
                return Ok(Prepared {
                    version: staged.version,
                    installer,
                    bytes,
                });
            }
            Err(ReadError::Io(detail)) => (UpdateErrorCode::Io, detail),
            Err(ReadError::Invalid(detail)) => (UpdateErrorCode::Signature, detail),
        };
        // Perdue ou refusée : supprimée, sauf si une vérification l'a déjà
        // remplacée entre-temps (rien ne change alors).
        let current = {
            let mut inner = self.inner.lock();
            let current = inner.staged.as_ref() == Some(&staged);
            if current {
                inner.staged = None;
                self.set_state(
                    &mut inner,
                    UpdateState::Error {
                        code,
                        detail: detail.clone(),
                    },
                );
            }
            current
        };
        if !current {
            return Err(Refusal::NotReady);
        }
        let dir = self.staging.clone();
        let _ = tokio::task::spawn_blocking(move || staging::discard(&dir, &staged)).await;
        Err(Refusal::Failed(detail))
    }

    /// Installe (services déjà arrêtés) : AppImage remplacée sur place, .deb
    /// par `pkexec dpkg -i` ; sous Windows, installateur lancé (ChatonLM se
    /// ferme ensuite sans en connaître l'issue : l'échec possible est noté
    /// d'avance). Un échec est noté sur le disque pour la relance qui suit,
    /// et montré tout de suite.
    pub async fn install(&self, prepared: Prepared) -> Result<(), String> {
        let Prepared {
            version,
            installer,
            bytes,
        } = prepared;
        let platform = self.platform.clone();
        let target = version.clone();
        let pending = platform.pending_failure().map(|detail| Failure {
            version: version.clone(),
            detail,
        });
        let dir = self.staging.clone();
        let result = tokio::task::spawn_blocking(move || {
            if let Some(failure) = &pending {
                staging::save_failure(&dir, failure);
            }
            platform.install(installer, &target, &bytes)
        })
        .await
        .unwrap_or_else(|error| Err(error.to_string()));
        if let Err(detail) = &result {
            self.installing
                .store(false, std::sync::atomic::Ordering::SeqCst);
            let failure = Failure {
                version,
                detail: detail.clone(),
            };
            let dir = self.staging.clone();
            let _ =
                tokio::task::spawn_blocking(move || staging::save_failure(&dir, &failure)).await;
            let mut inner = self.inner.lock();
            self.set_state(
                &mut inner,
                UpdateState::Error {
                    code: UpdateErrorCode::Install,
                    detail: detail.clone(),
                },
            );
        }
        result
    }
}

fn snapshot(inner: &Inner, support: Support) -> UpdateStatus {
    UpdateStatus {
        state: inner.state.clone(),
        auto_download: inner.auto_download,
        checked_at: inner.checked_at,
        installer: match support {
            Support::Installable(installer) => Some(installer),
            Support::Unsupported(_) => None,
        },
        last_error: inner.last_error.clone(),
    }
}

fn installing(inner: &Inner) -> bool {
    matches!(inner.state, UpdateState::Installing { .. })
}

/// Téléchargement en cours, mise à jour prête ou en cours d'installation :
/// une vérification ne les remplace pas par « Checking » ni par une erreur.
fn busy(inner: &Inner) -> bool {
    inner.downloading || installing(inner) || matches!(inner.state, UpdateState::Ready { .. })
}

/// Vérification planifiée qui ne passe pas par « Checking » : une mise à
/// jour annoncée reste affichée (avec Download ou la raison qui l'empêche)
/// si le serveur ne répond pas ; un échec d'installation ou de signature
/// affiché reste jusqu'à une vérification demandée (ou, pour la
/// signature, une autre annonce).
fn keeps_state(state: &UpdateState, trigger: Trigger) -> bool {
    trigger == Trigger::Scheduled
        && matches!(
            state,
            UpdateState::Available { .. }
                | UpdateState::Unsupported {
                    version: Some(_),
                    ..
                }
                | UpdateState::Error {
                    code: UpdateErrorCode::Install | UpdateErrorCode::Signature,
                    ..
                }
        )
}

/// Code d'un échec du téléchargement : signature (rien n'est gardé) ou
/// réseau.
fn download_code(error: &tauri_plugin_updater::Error) -> UpdateErrorCode {
    use tauri_plugin_updater::Error;
    match error {
        Error::Minisign(_)
        | Error::Base64(_)
        | Error::SignatureUtf8(_)
        | Error::SignedVersionMismatch { .. }
        | Error::MissingSignedVersion => UpdateErrorCode::Signature,
        _ => UpdateErrorCode::Download,
    }
}

/// Texte montré pour un échec du greffon, dans la langue de l'interface (ses
/// propres messages sont en anglais). Serveur sans annonce lisible : réponse
/// autre que 2xx (dépôt privé, Release absente), JSON mal formé ou sans
/// cette plate-forme ; réseau : seules les causes de reqwest (sans
/// l'adresse) restent en anglais, après un titre traduit.
fn plugin_error_text(error: &tauri_plugin_updater::Error) -> String {
    use tauri_plugin_updater::Error;
    match error {
        Error::ReleaseNotFound => t!("updates.errors.noRelease"),
        Error::Reqwest(error) if error.is_decode() => t!("updates.errors.invalidRelease"),
        Error::Serialization(_)
        | Error::Semver(_)
        | Error::TargetNotFound(_)
        | Error::TargetsNotFound(_) => t!("updates.errors.invalidRelease"),
        Error::Reqwest(error) if error.is_timeout() => t!("updates.errors.timeout"),
        Error::Reqwest(error) => t!(
            "updates.errors.unreachable",
            error = crate::hub::manifest::error_text(error)
        ),
        // « Download request failed with status: 404 Not Found ».
        Error::Network(message) => match message.rsplit_once("status: ") {
            Some((_, status)) => t!("updates.errors.downloadStatus", status = status),
            None => t!("updates.errors.unreachable", error = message),
        },
        Error::SignedVersionMismatch { signed, announced } => t!(
            "updates.signedForOtherVersion",
            signed = signed,
            version = announced
        ),
        Error::MissingSignedVersion => t!("updates.unsignedVersion"),
        Error::Minisign(_) | Error::Base64(_) | Error::SignatureUtf8(_) => {
            t!("updates.errors.badSignature")
        }
        Error::EmptyEndpoints | Error::UrlParse(_) | Error::InsecureTransportProtocol => {
            t!("updates.notConfigured")
        }
        _ => t!("updates.errors.unexpected", error = error),
    }
}

#[cfg(test)]
mod testkit;
// AppImage factice, droits Unix, faux pkexec : essais propres à Linux.
#[cfg(all(test, target_os = "linux"))]
mod tests;
// Mêmes parcours pour l'installateur de Windows, sur tout système.
#[cfg(test)]
mod tests_nsis;
