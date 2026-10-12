//! Serveur Ollama auquel parle ChatonLM (la « cible ») : point unique de
//! toutes les requêtes (chat, modèles, chargements, ModelHub, compte,
//! recherche web, dictée, correction, cours) et de l'écran d'accueil
//! (contrat `.tools/research/51-step18b-contract.md`, choix 1a et 2a de
//! l'utilisateur).
//!
//! - Source `builtin` (réglage) : l'Ollama intégré, démarré au besoin.
//! - Source `auto`, adresse explicite (Settings ou `OLLAMA_HOST`) : cette
//!   adresse ; injoignable, l'écran le dit et propose « Use the built-in
//!   Ollama » (source `builtin`), jamais de bascule d'office.
//! - Source `auto`, adresse par défaut : l'Ollama de la machine qui répond ;
//!   installé mais arrêté, l'écran propose « Start Ollama » et « Use the
//!   built-in Ollama instead » ; rien d'installé (ni binaire ni unité) :
//!   l'intégré démarre seul et le reste jusqu'à la fin de la session (pas
//!   d'aller-retour si l'Ollama de la machine apparaît), choix refait au
//!   lancement suivant.
//! - Intégré indisponible (aucun binaire livré) : comportement d'avant,
//!   quelle que soit la source.
//!
//! Un changement de cible ferme les téléchargements du ModelHub lancés vers
//! l'ancienne et émet `ollama-target` vers la page ; quand l'intégré cesse
//! d'être la cible, les travaux qui lui parlent (chat, cours, correction)
//! s'arrêtent, puis lui.
//!
//! Requêtes ([`request_settings`]) : jamais vers une adresse morte. Source
//! `auto` à l'adresse par défaut, la première attend la première décision ;
//! vers l'intégré, elles attendent la fin d'un démarrage ou d'une suspension
//! (changement du dossier des modèles), et échouent aussitôt, avec un
//! message clair, s'il est en échec ou fermé ([`super::builtin::shutdown`]).

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use ts_rs::TS;
use url::Url;

use super::address::{self, Configured};
use super::builtin::{Builtin, BuiltinState};
use super::client::{self, Probe};
use super::detect::{self, ServerState, ServerStatus};
use super::models_dir::ModelsSummary;
use super::models_dir::builtin::folder_summary;
use crate::error::{AppError, AppResult};
use crate::hub::downloads::HubDownloads;
use crate::store::Store;
use crate::store::builtin_ollama::{self, BuiltinSettings, OllamaSource};
use crate::store::settings::{self, Settings};

/// Événement émis vers la page quand la cible change ([`OllamaTarget`]).
pub const TARGET_EVENT: &str = "ollama-target";

/// Serveur de la cible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TargetKind {
    /// L'Ollama de la machine ou de l'adresse réglée.
    Machine,
    /// L'Ollama intégré à ChatonLM.
    Builtin,
}

/// Cible des requêtes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OllamaTarget {
    pub url: String,
    pub kind: TargetKind,
}

/// Settings › General, ligne « Ollama » et Model location en mode intégré.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OllamaRuntime {
    /// Réglage : `auto` ou `builtin`.
    pub source: OllamaSource,
    /// Serveur qui sert en ce moment.
    pub kind: TargetKind,
    /// Son adresse (celle de l'intégré : `http://127.0.0.1:<port>`).
    pub url: String,
    /// Adresse de cette machine (« on this computer »).
    pub local: bool,
    /// Adresse que vise la source `auto` hors intégré : réglée dans
    /// Settings, sinon `OLLAMA_HOST`, sinon l'adresse par défaut (bouton qui
    /// quitte l'intégré).
    pub machine_url: String,
    /// `machine_url` est une adresse de cette machine.
    pub machine_local: bool,
    /// Adresse réglée dans Settings ou par `OLLAMA_HOST`.
    pub explicit_address: bool,
    /// Version du serveur qui sert, s'il répond.
    pub served_version: Option<String>,
    /// Un Ollama intégré est livré.
    pub builtin_available: bool,
    pub builtin_state: BuiltinState,
    /// Version de l'intégré (livrée, sinon servie).
    pub builtin_version: Option<String>,
    /// Dossier des modèles de l'intégré (celui qui sert).
    pub builtin_models_dir: String,
    pub builtin_models_dir_is_default: bool,
    /// Le dossier choisi n'existe pas (disque démonté, dossier supprimé) :
    /// l'intégré prend le dossier par défaut à sa place, comme l'officiel.
    pub builtin_models_dir_missing: bool,
    /// Modèles présents dans ce dossier (lus sur le disque ; `count` 0 s'il
    /// n'y en a pas). Toujours renseigné.
    pub builtin_models: Option<ModelsSummary>,
    /// Journal de l'intégré (`<données>/logs/ollama.log`).
    pub log_path: String,
    /// Raison du dernier échec de l'intégré.
    pub builtin_failure: Option<String>,
}

/// Fin de « Start Ollama » (service systemd).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum ServiceStart {
    /// Démarrage demandé à systemd : l'écran suit l'état du service.
    Started,
    /// Invite de polkit fermée ou refusée.
    NotAuthorized,
    /// Aucun service `ollama.service` : la commande `ollama serve` reste à
    /// lancer à la main.
    NoService,
    Failed {
        detail: String,
    },
}

type Check = dyn Fn(String) -> Pin<Box<dyn Future<Output = ServerStatus> + Send>> + Send + Sync;
type OnChange = dyn Fn(&OllamaTarget) + Send + Sync;

/// État partagé de la cible (un par app : voir [`install`]).
pub struct Targeting {
    builtin: Builtin,
    /// Source `auto` : l'intégré a été choisi pour cette session (rien
    /// d'installé sur la machine).
    session_builtin: AtomicBool,
    last: Mutex<Option<OllamaTarget>>,
    on_change: Box<OnChange>,
    /// Arrête les travaux qui parlent à l'intégré (chat, cours, correction),
    /// juste avant de l'arrêter.
    before_stop: Box<dyn Fn() + Send + Sync>,
    /// Changé à chaque changement de réglage qui touche la cible (source,
    /// adresse) : une décision prise avec des réglages lus avant est
    /// périmée.
    decisions: AtomicU64,
    /// Première décision de la cible prise (source `auto`, adresse par
    /// défaut : une requête précoce l'attend).
    first: tokio::sync::OnceCell<()>,
    /// Vérification d'une adresse de la machine (`detect::check` ; simulée
    /// par les essais).
    check: Box<Check>,
    /// Valeur de `OLLAMA_HOST` (absente dans les essais).
    host_env: fn() -> Option<String>,
}

static CURRENT: OnceLock<Arc<Targeting>> = OnceLock::new();

#[cfg(test)]
tokio::task_local! {
    /// Cible imposée par un essai, dans sa tâche seulement (les autres
    /// essais du même processus n'en voient rien).
    pub(crate) static TEST_TARGETING: Arc<Targeting>;
}

/// Installe la cible de l'app (au démarrage, une fois).
pub fn install(targeting: Arc<Targeting>) {
    let _ = CURRENT.set(targeting);
}

/// Cible de l'app ; `None` hors de l'app (essais unitaires) : l'adresse
/// réglée sert alors telle quelle.
pub fn current() -> Option<Arc<Targeting>> {
    #[cfg(test)]
    if let Ok(targeting) = TEST_TARGETING.try_with(Arc::clone) {
        return Some(targeting);
    }
    CURRENT.get().cloned()
}

/// Changement de cible, dans l'app : les téléchargements du ModelHub vers
/// l'ancienne s'arrêtent, puis `emit` prévient la page (`ollama-target`).
pub fn announce(
    target: &OllamaTarget,
    downloads: Option<&HubDownloads>,
    emit: impl FnOnce(&OllamaTarget),
) {
    if let Some(downloads) = downloads {
        downloads.retarget(&target.url);
    }
    emit(target);
}

/// Réglages de l'app pour une requête : `effective_server_url` est l'adresse
/// de la cible (voir [`Targeting::request_settings`] : première décision,
/// démarrage ou suspension de l'intégré attendus ; intégré en échec ou
/// fermé : erreur).
pub async fn request_settings(store: &Store) -> AppResult<Settings> {
    match current() {
        Some(targeting) => targeting.request_settings(store).await,
        None => plain_settings(store).await,
    }
}

/// Réglages de l'app à montrer (sans attendre l'intégré).
pub async fn display_settings(store: &Store) -> AppResult<Settings> {
    match current() {
        Some(targeting) => targeting
            .settings(store)
            .await
            .map(|(settings, _)| settings),
        None => plain_settings(store).await,
    }
}

/// Réglages enregistrés (adresse peut-être changée) : une vérification en
/// cours ne note plus sa décision ; la cible est recalculée (téléchargements
/// vers l'ancienne fermés, intégré arrêté s'il ne sert plus).
pub async fn settings_updated(store: &Store) -> AppResult<Settings> {
    if let Some(targeting) = current() {
        targeting.settings_changed();
    }
    display_settings(store).await
}

/// Erreur d'une requête vers un intégré qui ne tourne pas (échec, arrêt).
fn not_running(failure: Option<String>) -> AppError {
    AppError::Ollama(match failure {
        Some(reason) => crate::i18n::t!("builtinOllama.notRunningBecause", reason = reason),
        None => crate::i18n::t!("builtinOllama.notRunning"),
    })
}

async fn plain_settings(store: &Store) -> AppResult<Settings> {
    let host = crate::commands::ollama_host_env();
    store
        .call(move |connection| settings::get(connection, host.as_deref()))
        .await
}

/// Ce que veulent les réglages.
struct Wanted {
    settings: Settings,
    builtin: BuiltinSettings,
    configured: Configured,
}

/// Choix de la cible, sans entrée-sortie.
pub fn choose(
    source: OllamaSource,
    explicit_address: bool,
    session_builtin: bool,
    builtin_available: bool,
) -> TargetKind {
    if !builtin_available {
        return TargetKind::Machine;
    }
    match source {
        OllamaSource::Builtin => TargetKind::Builtin,
        OllamaSource::Auto if session_builtin && !explicit_address => TargetKind::Builtin,
        OllamaSource::Auto => TargetKind::Machine,
    }
}

impl Targeting {
    pub fn new(
        builtin: Builtin,
        on_change: impl Fn(&OllamaTarget) + Send + Sync + 'static,
    ) -> Self {
        Self::with_check(builtin, on_change, |url| {
            Box::pin(async move { detect::check(&url).await })
        })
    }

    /// Avec une vérification de la machine imposée (essais).
    pub fn with_check(
        builtin: Builtin,
        on_change: impl Fn(&OllamaTarget) + Send + Sync + 'static,
        check: impl Fn(String) -> Pin<Box<dyn Future<Output = ServerStatus> + Send>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            builtin,
            session_builtin: AtomicBool::new(false),
            last: Mutex::new(None),
            on_change: Box::new(on_change),
            before_stop: Box::new(|| {}),
            decisions: AtomicU64::new(0),
            first: tokio::sync::OnceCell::new(),
            check: Box::new(check),
            host_env: crate::commands::ollama_host_env,
        }
    }

    /// Travaux à arrêter juste avant tout arrêt de l'intégré par la cible
    /// (retour à l'Ollama de la machine, adresse réglée, changement de
    /// dossier : voir [`Targeting::stop_work`]).
    pub fn before_builtin_stop(mut self, hook: impl Fn() + Send + Sync + 'static) -> Self {
        self.before_stop = Box::new(hook);
        self
    }

    /// Sans `OLLAMA_HOST` (essais).
    #[cfg(test)]
    pub fn without_ollama_host(mut self) -> Self {
        self.host_env = || None;
        self
    }

    /// Avec cette valeur de `OLLAMA_HOST` (essais).
    #[cfg(test)]
    pub fn with_ollama_host(mut self, host_env: fn() -> Option<String>) -> Self {
        self.host_env = host_env;
        self
    }

    pub fn builtin(&self) -> &Builtin {
        &self.builtin
    }

    /// Arrête les travaux qui parlent à l'intégré (avant de l'arrêter).
    pub fn stop_work(&self) {
        (self.before_stop)();
    }

    /// Un réglage qui touche la cible vient d'être enregistré.
    pub fn settings_changed(&self) {
        self.decisions.fetch_add(1, Ordering::SeqCst);
    }

    /// Dossier des modèles de l'intégré, et vrai si c'est celui par défaut.
    /// Un dossier choisi qui n'existe pas est remplacé par celui par défaut
    /// (comme `cmd()` de l'officiel : jamais recréé vide sur le disque
    /// système).
    pub fn models_dir(&self, saved: &BuiltinSettings) -> (PathBuf, bool) {
        match &saved.models_dir {
            Some(dir) if Path::new(dir).is_dir() => (PathBuf::from(dir), false),
            _ => (self.builtin.config().default_models_dir(), true),
        }
    }

    /// Le dossier choisi n'existe pas (celui par défaut le remplace).
    fn chosen_dir_missing(saved: &BuiltinSettings) -> bool {
        saved
            .models_dir
            .as_deref()
            .is_some_and(|dir| !Path::new(dir).is_dir())
    }

    /// Dossier des modèles réglé.
    pub async fn saved_models_dir(&self, store: &Store) -> AppResult<(PathBuf, bool)> {
        let saved = store
            .call(|connection| builtin_ollama::get(connection))
            .await?;
        Ok(self.models_dir(&saved))
    }

    async fn wanted(&self, store: &Store) -> AppResult<Wanted> {
        let host = (self.host_env)();
        store
            .call(move |connection| {
                let settings = settings::get(connection, host.as_deref())?;
                let builtin = builtin_ollama::get(connection)?;
                let configured = address::configured(&settings.server_url, host.as_deref());
                Ok(Wanted {
                    settings,
                    builtin,
                    configured,
                })
            })
            .await
    }

    fn kind(&self, wanted: &Wanted) -> TargetKind {
        choose(
            wanted.builtin.source,
            wanted.configured.explicit,
            self.session_builtin.load(Ordering::SeqCst),
            self.builtin.available(),
        )
    }

    /// Cible voulue ; l'intégré est démarré s'il le faut (pas relancé après
    /// un échec).
    fn target(&self, wanted: &Wanted, kind: TargetKind) -> OllamaTarget {
        let target = match kind {
            TargetKind::Builtin => {
                let (dir, _) = self.models_dir(&wanted.builtin);
                OllamaTarget {
                    url: self.builtin.ensure_started(&dir).unwrap_or_default(),
                    kind,
                }
            }
            TargetKind::Machine => OllamaTarget {
                url: wanted.configured.url.clone(),
                kind,
            },
        };
        self.note(&target);
        target
    }

    /// Retient la cible ; un changement est signalé (téléchargements,
    /// page). L'intégré qui cesse d'être la cible s'arrête, ses travaux
    /// d'abord.
    fn note(&self, target: &OllamaTarget) {
        let previous = self.last.lock().replace(target.clone());
        let Some(previous) = previous else {
            return;
        };
        if previous == *target {
            return;
        }
        if previous.kind == TargetKind::Builtin && target.kind == TargetKind::Machine {
            self.stop_work();
            let builtin = self.builtin.clone();
            super::builtin::spawn(async move { builtin.stop().await });
        }
        (self.on_change)(target);
    }

    /// Réglages avec l'adresse de la cible, et la cible.
    pub async fn settings(&self, store: &Store) -> AppResult<(Settings, OllamaTarget)> {
        let wanted = self.wanted(store).await?;
        let target = self.target(&wanted, self.kind(&wanted));
        let mut settings = wanted.settings;
        settings.effective_server_url.clone_from(&target.url);
        Ok((settings, target))
    }

    /// Comme [`Targeting::settings`], pour une requête : jamais vers une
    /// adresse morte.
    /// - Source `auto` à l'adresse par défaut, avant la première décision :
    ///   elle est prise (ou attendue) d'abord.
    /// - Intégré suspendu (changement du dossier des modèles) : attendu
    ///   jusqu'à la fin, puis la cible est recalculée.
    /// - Intégré en démarrage : attendu (délai de démarrage, plus 1 s).
    /// - Intégré en échec ou fermé : erreur aussitôt.
    pub async fn request_settings(&self, store: &Store) -> AppResult<Settings> {
        self.first_decision_for_request(store).await?;
        let mut waited = false;
        let mut retried = false;
        loop {
            let (mut settings, target) = self.settings(store).await?;
            if target.kind == TargetKind::Machine {
                return Ok(settings);
            }
            if self.builtin.paused() {
                self.builtin.wait_unpaused().await;
                continue;
            }
            let snapshot = self.builtin.snapshot();
            match snapshot.state {
                BuiltinState::Running => {
                    if let Some(url) = snapshot.url {
                        settings.effective_server_url = url;
                    }
                    return Ok(settings);
                }
                BuiltinState::Starting if !waited => {
                    waited = true;
                    let limit = self.builtin.config().timing.ready_timeout + Duration::from_secs(1);
                    self.builtin.wait_ready(limit).await;
                }
                // Arrêt qui vient de finir : le tour suivant le relance.
                BuiltinState::Stopped if !retried && !self.builtin.is_closed() => {
                    retried = true;
                    tokio::task::yield_now().await;
                }
                _ => return Err(not_running(snapshot.failure)),
            }
        }
    }

    /// Source `auto`, adresse par défaut, intégré livré, pas encore de
    /// décision : la prend (ou attend celle en cours, celle du démarrage de
    /// l'app compris), pour qu'une requête précoce n'aille pas à l'adresse
    /// par défaut quand rien n'y est installé.
    async fn first_decision_for_request(&self, store: &Store) -> AppResult<()> {
        if self.first.initialized() || self.session_builtin.load(Ordering::SeqCst) {
            return Ok(());
        }
        let wanted = self.wanted(store).await?;
        if wanted.builtin.source == OllamaSource::Auto
            && !wanted.configured.explicit
            && self.builtin.available()
        {
            self.first_decision(store).await;
        }
        Ok(())
    }

    /// Première décision de la cible (démarrage de l'app), une fois ; une
    /// requête qui arrive pendant l'attend.
    pub async fn first_decision(&self, store: &Store) {
        self.first
            .get_or_init(|| async {
                let _ = self.status(store).await;
            })
            .await;
    }

    /// État du serveur pour l'écran d'accueil. Source `auto`, adresse par
    /// défaut, rien d'installé : l'intégré démarre et sert jusqu'à la fin de
    /// la session. Une décision prise avec des réglages changés pendant la
    /// vérification de la machine n'est pas notée (refaite, deux fois au
    /// plus).
    pub async fn status(&self, store: &Store) -> AppResult<ServerStatus> {
        let mut attempts = 0;
        loop {
            attempts += 1;
            let generation = self.decisions.load(Ordering::SeqCst);
            let wanted = self.wanted(store).await?;
            let available = self.builtin.available();
            if self.kind(&wanted) == TargetKind::Builtin {
                let _ = self.first.set(());
                return Ok(self.builtin_status(&wanted));
            }
            let mut status = (self.check)(wanted.configured.url.clone()).await;
            status.builtin_available = available;
            status.explicit_address = wanted.configured.explicit;
            if self.decisions.load(Ordering::SeqCst) != generation {
                if attempts < 3 {
                    continue;
                }
                return Ok(status);
            }
            if available
                && wanted.builtin.source == OllamaSource::Auto
                && !wanted.configured.explicit
                && status.state == ServerState::NotInstalled
            {
                self.session_builtin.store(true, Ordering::SeqCst);
                let _ = self.first.set(());
                return Ok(self.builtin_status(&wanted));
            }
            self.note(&OllamaTarget {
                url: wanted.configured.url.clone(),
                kind: TargetKind::Machine,
            });
            let _ = self.first.set(());
            return Ok(status);
        }
    }

    fn builtin_status(&self, wanted: &Wanted) -> ServerStatus {
        let target = self.target(wanted, TargetKind::Builtin);
        let snapshot = self.builtin.snapshot();
        let state = match snapshot.state {
            BuiltinState::Running => ServerState::Running,
            BuiltinState::Failed => ServerState::BuiltinFailed,
            BuiltinState::Starting | BuiltinState::Stopped => ServerState::BuiltinStarting,
        };
        ServerStatus {
            url: target.url,
            state,
            version: snapshot.version,
            alternative_url: None,
            host_env: None,
            http_status: None,
            detail: snapshot.failure,
            service: None,
            builtin: true,
            builtin_available: true,
            explicit_address: wanted.configured.explicit,
            log_path: Some(self.log_path()),
        }
    }

    pub fn log_path(&self) -> String {
        self.builtin.config().log.to_string_lossy().into_owned()
    }

    /// Settings › General : quel Ollama sert, l'intégré, ses modèles.
    pub async fn runtime(&self, store: &Store) -> AppResult<OllamaRuntime> {
        let wanted = self.wanted(store).await?;
        let target = self.target(&wanted, self.kind(&wanted));
        let snapshot = self.builtin.snapshot();
        let served_version = match target.kind {
            TargetKind::Builtin => snapshot.version.clone(),
            TargetKind::Machine => match client::probe(&target.url).await {
                Probe::Ollama { version } => Some(version),
                _ => None,
            },
        };
        let (dir, is_default) = self.models_dir(&wanted.builtin);
        let summary_dir = dir.clone();
        let builtin_models = tokio::task::spawn_blocking(move || folder_summary(&summary_dir))
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
        let is_local = |url: &str| Url::parse(url).is_ok_and(|url| client::is_local(&url));
        Ok(OllamaRuntime {
            source: wanted.builtin.source,
            kind: target.kind,
            local: is_local(&target.url),
            url: target.url,
            machine_local: is_local(&wanted.configured.url),
            machine_url: wanted.configured.url,
            explicit_address: wanted.configured.explicit,
            served_version,
            builtin_available: self.builtin.available(),
            builtin_state: snapshot.state,
            builtin_version: self.builtin.version(),
            builtin_models_dir: dir.to_string_lossy().into_owned(),
            builtin_models_dir_is_default: is_default,
            builtin_models_dir_missing: Self::chosen_dir_missing(&wanted.builtin),
            builtin_models: Some(builtin_models),
            log_path: self.log_path(),
            builtin_failure: snapshot.failure,
        })
    }

    /// Change la source. `builtin` : l'intégré démarre (relancé après un
    /// échec). `auto` : l'Ollama de la machine (ou de l'adresse réglée)
    /// reprend, et l'intégré s'arrête (ses travaux d'abord : voir
    /// [`Targeting::note`]), sauf si rien n'est installé (il reste alors la
    /// cible de la session). Cette décision est prise avant d'écrire le
    /// réglage : pendant la vérification de la machine, les requêtes vont
    /// encore à la cible actuelle.
    pub async fn set_source(
        &self,
        store: &Store,
        source: OllamaSource,
    ) -> AppResult<OllamaRuntime> {
        if source == OllamaSource::Builtin && !self.builtin.available() {
            return Err(AppError::Invalid(crate::i18n::t!(
                "builtinOllama.unavailable"
            )));
        }
        let keep = match source {
            OllamaSource::Builtin => true,
            OllamaSource::Auto => {
                let wanted = self.wanted(store).await?;
                let keep = self.builtin.available()
                    && !wanted.configured.explicit
                    && (self.check)(wanted.configured.url.clone()).await.state
                        == ServerState::NotInstalled;
                self.session_builtin.store(keep, Ordering::SeqCst);
                keep
            }
        };
        store
            .call(move |connection| builtin_ollama::set_source(connection, source))
            .await?;
        // Après l'écriture : une vérification lancée avant ne note rien.
        self.settings_changed();
        let _ = self.first.set(());
        if source == OllamaSource::Builtin && self.builtin.snapshot().state == BuiltinState::Failed
        {
            self.builtin.stop().await;
        }
        self.settings(store).await?;
        if !keep {
            self.builtin.stop().await;
        }
        self.runtime(store).await
    }

    /// Retry de l'écran d'accueil après un échec de l'intégré : arrêt puis
    /// relance, sur le dossier réglé.
    pub async fn restart_builtin(&self, store: &Store) -> AppResult<()> {
        let (dir, _) = self.saved_models_dir(store).await?;
        self.builtin.restart(&dir).await;
        Ok(())
    }

    /// L'intégré est la cible en ce moment.
    pub async fn builtin_is_target(&self, store: &Store) -> AppResult<bool> {
        let wanted = self.wanted(store).await?;
        Ok(self.kind(&wanted) == TargetKind::Builtin)
    }
}

/// « Start Ollama » : démarre `ollama.service` (unité du système par polkit,
/// ou de l'utilisateur). Build d'essai : jamais le vrai service, réponse
/// imposée par `CHATONLM_E2E_SERVICE_START` (`started`, `notAuthorized`,
/// `noService`, sinon échec).
pub async fn start_service() -> ServiceStart {
    if cfg!(feature = "e2e") {
        return injected_service_start();
    }
    use super::host::{self, StartResult, Unit};
    let started = tokio::task::spawn_blocking(|| match host::ollama_unit() {
        Unit::Loaded(service) => host::start_unit(service.user),
        Unit::NotFound => StartResult::NoUnit,
        Unit::Unknown => StartResult::Failed(crate::i18n::t!("builtinOllama.noSystemd")),
    })
    .await;
    match started {
        Ok(StartResult::Started) => ServiceStart::Started,
        Ok(StartResult::NotAuthorized) => ServiceStart::NotAuthorized,
        Ok(StartResult::NoUnit) => ServiceStart::NoService,
        Ok(StartResult::Failed(detail)) => ServiceStart::Failed { detail },
        Err(error) => ServiceStart::Failed {
            detail: error.to_string(),
        },
    }
}

/// Réponse imposée de « Start Ollama » (build d'essai seulement : jamais
/// appelée ailleurs, `cfg!` est constant).
fn injected_service_start() -> ServiceStart {
    match std::env::var("CHATONLM_E2E_SERVICE_START").as_deref() {
        Ok("started") => ServiceStart::Started,
        Ok("notAuthorized") => ServiceStart::NotAuthorized,
        Ok("noService") => ServiceStart::NoService,
        other => ServiceStart::Failed {
            // i18n-ignore : build d'essai seulement.
            detail: format!("test build: {other:?}"),
        },
    }
}

// Essais à côté du module : un dossier `target/` serait ignoré par git
// (dossiers de compilation).
#[cfg(test)]
#[path = "target_tests.rs"]
mod tests;
