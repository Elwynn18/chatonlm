//! Dictée (étape 17, propre à ChatonLM, inspirée d'OpenWhispr) : micro →
//! transcription locale (Parakeet ou Whisper) → nettoyage par un modèle
//! Ollama → presse-papiers + notification.
//!
//! [`Dictation`] tient l'état (phase, dernier résultat) ; elle est déclenchée
//! par le raccourci global et la zone de notification (`crate::desktop`),
//! par `chatonlm --dictate` et par la page (réglages).
//!
//! Machine d'états : Idle → Recording → Transcribing → (Polishing) → Idle.
//! Le texte est copié dans le presse-papiers à la fin, puis collé là où se
//! trouve le curseur (réglage `auto_paste`, activé par défaut : Ctrl+V envoyé
//! à l'application au premier plan par [`Clipboard::paste`] ; échec : la
//! notification dit de coller par Ctrl+V), sauf si la copie automatique est
//! coupée : rien n'est collé, la notification propose alors « Copy », et la
//! page `dictation_copy_last`.
//! `toggle`, `start`, `stop` et `cancel` sont idempotents ; pendant
//! Transcribing et Polishing, `start` et `toggle` sont ignorés. Les réglages
//! sont relus en base à chaque dictée.
//!
//! Le mode conversation vocale (`crate::voice_mode`) réserve la dictée le
//! temps d'une séance (seulement si aucune dictée n'est en cours) : elle
//! refuse alors de démarrer (avis « Dictation failed ») et garde son modèle
//! chargé, qu'elle partage avec la séance (`engine`).

pub mod audio;
pub mod clipboard;
pub mod cpu;
pub mod downloads;
pub mod engine;
pub mod languages;
pub mod models;
pub mod notify;
pub mod onnx;
pub mod polish;
#[cfg(test)]
mod test_data;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{oneshot, watch};
use ts_rs::TS;

use audio::Microphone;
use clipboard::{BoxFuture, Clipboard, PasteError};
use engine::{Engine, ModelSpec, Request};
use models::SpeechEngine;
use notify::{Notice, Notifier, Offer, Title};

use crate::chat::Server;
use crate::desktop::ShortcutStatus;
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::dictation::{DictationMode, DictationSettings};
use crate::store::{Store, now_ms};

/// Événement envoyé à la page à chaque changement de [`DictationStatus`].
pub const STATUS_EVENT: &str = "dictation-status";

/// Attente maximale, à la fermeture, d'un chargement ou d'une transcription
/// en cours.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(10);

/// Icône des notifications, écrite une fois sous le dossier des données.
const ICON: &[u8] = include_bytes!("../../icons/128x128.png");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DictationPhase {
    #[default]
    Idle,
    Recording,
    Transcribing,
    Polishing,
}

/// Résultat de la dernière dictée (en mémoire seulement).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DictationResult {
    /// Heure de fin, en millisecondes depuis l'époque Unix.
    pub at: i64,
    /// Texte copié dans le presse-papiers ; vide si la dictée a échoué.
    pub text: String,
    /// Transcription avant nettoyage.
    pub raw: String,
    pub polished: bool,
    /// `text` est dans le presse-papiers.
    pub copied: bool,
    /// Échec de la dictée (`text` vide), raison pour laquelle le nettoyage
    /// n'a pas été appliqué (texte brut copié), ou échec de la copie
    /// (`copied` faux, `text` gardé).
    pub error: Option<String>,
    /// `error` est l'échec de la copie automatique (pas un texte laissé à
    /// copier parce qu'elle est coupée).
    pub copy_failed: bool,
    /// `text` a été collé là où se trouvait le curseur.
    pub pasted: bool,
    /// Pourquoi le collage automatique n'a pas eu lieu (le texte reste dans
    /// le presse-papiers) ; `None` s'il a réussi ou n'était pas demandé.
    pub paste_error: Option<String>,
}

impl DictationResult {
    /// Dictée en échec, pour `reason`.
    fn failed(reason: String) -> Self {
        Self {
            at: now_ms(),
            text: String::new(),
            raw: String::new(),
            polished: false,
            copied: false,
            error: Some(reason),
            copy_failed: false,
            pasted: false,
            paste_error: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DictationStatus {
    pub phase: DictationPhase,
    pub shortcut: ShortcutStatus,
    /// Le portail sait ouvrir la boîte de réglage du raccourci (version 2).
    pub can_configure_shortcut: bool,
    /// Une zone de notification (StatusNotifierWatcher) affiche l'icône.
    pub tray_available: bool,
    /// Commande à lier soi-même dans les raccourcis du système (repli sans
    /// portail) : binaire, ou AppImage, suivi de « --dictate ».
    pub command: String,
    pub last_result: Option<DictationResult>,
}

/// Valeurs fixes utiles aux réglages.
#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DictationInfo {
    pub default_polish_prompt: String,
    pub languages: Vec<languages::DictationLanguage>,
}

impl DictationInfo {
    pub fn new() -> Self {
        Self {
            default_polish_prompt: polish::DEFAULT_PROMPT.into(),
            languages: languages::LANGUAGES.to_vec(),
        }
    }
}

/// État de la dictée (géré par Tauri).
pub struct Dictation {
    core: Arc<Core>,
    notifier: Arc<notify::DesktopNotifier>,
    /// Le vrai presse-papiers de `core` (fichier de l'autorisation de coller).
    clipboard: Arc<clipboard::SystemClipboard>,
}

impl Default for Dictation {
    fn default() -> Self {
        Self::new()
    }
}

impl Dictation {
    pub fn new() -> Self {
        let notifier = Arc::new(notify::DesktopNotifier::default());
        let clipboard = Arc::new(clipboard::SystemClipboard::default());
        let services = Services {
            microphone: microphone(),
            engine: Engine::new(),
            clipboard: clipboard.clone(),
            notifier: notifier.clone(),
            max_recording: audio::MAX_DURATION,
            support: engine_support,
        };
        Self {
            core: Arc::new(Core::new(services)),
            notifier,
            clipboard,
        }
    }

    /// Démarrage de l'app : identité des notifications et icône écrite sur le
    /// disque (une fois, ou si elle a changé) ; fichier de l'autorisation de
    /// coller (commun avec la correction).
    pub fn setup(&self, app: &AppHandle) {
        if let Ok(data) = crate::paths::data_dir(app) {
            self.clipboard
                .set_paste_token_file(data.join(clipboard::PASTE_TOKEN_FILE));
        }
        // ONNX Runtime de Parakeet, installé par les paquets avec les
        // ressources ; chargé seulement à la première demande.
        if let Ok(resources) = app.path().resource_dir() {
            onnx::configure(resources);
        }
        let icon = crate::paths::notification_icon(app).ok();
        if let Some(path) = icon.clone() {
            tauri::async_runtime::spawn_blocking(move || {
                if std::fs::read(&path).is_ok_and(|current| current == ICON) {
                    return;
                }
                let written = path
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|()| std::fs::write(&path, ICON));
                if let Err(error) = written {
                    eprintln!("chatonlm: notification icon not written: {error}");
                }
            });
        }
        self.notifier
            .configure(app.config().identifier.clone(), icon);
        // Clic sur « Copy » (fil de la notification) : copie dans le runtime.
        let handle = app.clone();
        self.notifier.on_action(move |offer| {
            let app = handle.clone();
            tauri::async_runtime::spawn(async move {
                app.state::<Dictation>().copy_offer(&app, offer).await;
            });
        });
    }

    /// Copie le texte du dernier résultat (bouton de la page) ; il compte
    /// ensuite comme copié.
    pub async fn copy_last(&self, app: &AppHandle) -> AppResult<()> {
        let copied = self.core.copy_last().await;
        if copied.is_ok() {
            self.emit_status(app);
        }
        copied.map_err(|error| match error {
            CopyError::Nothing => AppError::Invalid(t!("dictation.nothingToCopy")),
            CopyError::Clipboard(reason) => AppError::System(copy_error(&reason)),
        })
    }

    /// Clic sur « Copy » d'une notification : texte de son résultat copié ;
    /// échec montré par une notification.
    async fn copy_offer(&self, app: &AppHandle, offer: Offer) {
        match self.core.copy_offer(offer).await {
            Ok(true) => self.emit_status(app),
            Ok(false) => {}
            Err(reason) => self
                .core
                .services
                .notifier
                .show(Notice::last(Title::NotCopied, notify::escape(&reason))),
        }
    }

    /// Commence si rien n'est en cours, sinon arrête l'enregistrement (la
    /// transcription suit). Sans effet pendant la transcription ou le
    /// nettoyage.
    pub fn toggle(&self, app: &AppHandle) {
        self.core.toggle(host(app));
    }

    /// [`Self::toggle`] depuis la page (« Try dictation ») : une dictée
    /// commencée ainsi n'est jamais collée (le curseur est dans ChatonLM, sur
    /// le bouton) ; son texte se lit sous « Last dictation ».
    pub fn toggle_from_page(&self, app: &AppHandle) {
        self.core.toggle_from(host(app), Origin::Page);
    }

    /// Commence l'enregistrement ; sans effet si une dictée est en cours.
    pub fn start(&self, app: &AppHandle) {
        self.core.start(host(app));
    }

    /// Arrête l'enregistrement : transcription, nettoyage, copie.
    pub fn stop(&self, app: &AppHandle) {
        let _ = app;
        self.core.stop();
    }

    /// Abandonne l'enregistrement ou le traitement en cours, sans rien copier.
    pub fn cancel(&self, app: &AppHandle) {
        if self.core.cancel() {
            host(app).changed(DictationPhase::Idle);
        }
    }

    pub fn phase(&self) -> DictationPhase {
        *self.core.phase.borrow()
    }

    /// Moteur de transcription, partagé avec le mode conversation vocale.
    pub fn engine(&self) -> Engine {
        self.core.services.engine.clone()
    }

    /// Réserve la dictée pour une séance du mode vocal : faux si une dictée
    /// est en cours ; sinon, plus aucune ne démarre jusqu'à
    /// [`Dictation::release_voice_mode`].
    pub fn reserve_for_voice_mode(&self) -> bool {
        self.core.reserve()
    }

    pub fn release_voice_mode(&self) {
        self.core.release();
    }

    /// Phases successives (icône de la zone de notification).
    pub fn subscribe(&self) -> watch::Receiver<DictationPhase> {
        self.core.phase.subscribe()
    }

    pub fn status(&self, app: &AppHandle) -> DictationStatus {
        let desktop = crate::desktop::state(app);
        DictationStatus {
            phase: self.phase(),
            shortcut: desktop.shortcut_status(),
            can_configure_shortcut: desktop.can_configure_shortcut(),
            tray_available: desktop.tray_available(),
            command: crate::desktop::dictate_command(),
            last_result: self.core.inner.lock().last_result.clone(),
        }
    }

    /// Envoie [`DictationStatus`] à la page.
    pub fn emit_status(&self, app: &AppHandle) {
        let _ = app.emit(STATUS_EVENT, self.status(app));
    }

    /// Réglages enregistrés : le modèle chargé est libéré s'il n'est plus
    /// réglé, ou si la dictée est coupée (en arrière-plan).
    pub fn settings_changed(&self, settings: &DictationSettings) {
        // Séance du mode vocal : le modèle lui sert.
        if self.core.inner.lock().voice_mode {
            return;
        }
        let engine = self.core.services.engine.clone();
        let keep = settings.enabled.then(|| settings.speech_model.clone());
        tauri::async_runtime::spawn_blocking(move || engine.release_except(keep.as_deref()));
    }

    /// Modèle `id` sur le point d'être supprimé : libéré s'il est chargé
    /// (attend une transcription en cours).
    pub async fn release_model(&self, id: &str) {
        let engine = self.core.services.engine.clone();
        let id = id.to_owned();
        let _ = tauri::async_runtime::spawn_blocking(move || engine.release_model(&id)).await;
    }

    /// Fermeture de l'app : enregistrement abandonné, plus aucune dictée ;
    /// attend la fin d'un chargement ou d'une transcription en cours (10 s au
    /// plus) puis libère le moteur, avant les destructeurs de la sortie.
    pub async fn shutdown(&self) {
        self.core.shutdown(SHUTDOWN_WAIT).await;
    }
}

/// Micro du système, ou WAV des essais e2e (`CHATONLM_E2E_MICROPHONE`).
fn microphone() -> Arc<dyn Microphone> {
    #[cfg(feature = "e2e")]
    if let Some(path) = std::env::var_os("CHATONLM_E2E_MICROPHONE") {
        return Arc::new(audio::WavMicrophone(path));
    }
    Arc::new(audio::SystemMicrophone)
}

/// Ce qu'une dictée lit de l'app, relu à chaque dictée.
pub(crate) struct Prepared {
    pub settings: DictationSettings,
    /// Dossier des modèles de transcription.
    pub models_dir: PathBuf,
    /// Le modèle du nettoyage raisonne même avec `think: false`.
    pub always_thinks: bool,
    /// Touche du raccourci, telle que le bureau la décrit, si elle est liée.
    pub trigger: Option<String>,
}

/// L'app vue par la dictée (remplacée dans les tests).
pub(crate) trait Host: Send + Sync + 'static {
    /// Réglages de la dictée, sans rien attendre d'Ollama : le micro s'ouvre
    /// aussitôt.
    fn prepare(&self) -> BoxFuture<Result<Prepared, String>>;
    /// Adresse effective d'Ollama, contexte et durée en mémoire réglés dans
    /// l'app (envoyés avec le nettoyage, comme par le chat) : lus seulement
    /// au moment du nettoyage (l'Ollama intégré peut démarrer pendant que
    /// l'on parle).
    fn server(&self) -> BoxFuture<Result<Server, String>>;
    /// Phase ou dernier résultat changés.
    fn changed(&self, phase: DictationPhase);
}

struct AppHost(AppHandle);

fn host(app: &AppHandle) -> Arc<dyn Host> {
    Arc::new(AppHost(app.clone()))
}

impl Host for AppHost {
    fn prepare(&self) -> BoxFuture<Result<Prepared, String>> {
        let app = self.0.clone();
        Box::pin(async move {
            let failed = |error: crate::error::AppError| error.to_string();
            let store = app
                .try_state::<Store>()
                .map(|store| store.inner().clone())
                .ok_or_else(|| t!("dictation.noDatabase"))?;
            let settings = store
                .call(|connection| crate::store::dictation::get(connection))
                .await
                .map_err(failed)?;
            let model = settings.polish_model.clone();
            let always_thinks = !model.is_empty()
                && store
                    .call(move |connection| {
                        crate::store::model_prefs::always_thinks(connection, &model)
                    })
                    .await
                    .unwrap_or(false);
            let models_dir = crate::paths::speech_models_dir(&app).map_err(failed)?;
            let trigger = match crate::desktop::state(&app).shortcut_status() {
                ShortcutStatus::Bound { trigger } if !trigger.trim().is_empty() => Some(trigger),
                _ => None,
            };
            Ok(Prepared {
                settings,
                models_dir,
                always_thinks,
                trigger,
            })
        })
    }

    fn server(&self) -> BoxFuture<Result<Server, String>> {
        let store = self
            .0
            .try_state::<Store>()
            .map(|store| store.inner().clone());
        Box::pin(async move {
            let store = store.ok_or_else(|| t!("dictation.noDatabase"))?;
            let settings = crate::commands::models::app_settings(&store)
                .await
                .map_err(|error| error.to_string())?;
            Ok(Server::from_settings(&settings))
        })
    }

    fn changed(&self, _phase: DictationPhase) {
        self.0.state::<Dictation>().emit_status(&self.0);
    }
}

/// Micro, moteur, presse-papiers et notifications (faux dans les tests).
pub(crate) struct Services {
    pub microphone: Arc<dyn Microphone>,
    pub engine: Engine,
    pub clipboard: Arc<dyn Clipboard>,
    pub notifier: Arc<dyn Notifier>,
    /// Arrêt automatique de l'enregistrement.
    pub max_recording: Duration,
    /// Refus d'un moteur que cette machine ne peut pas faire tourner
    /// ([`engine_support`] ; les essais l'imposent).
    pub support: fn(SpeechEngine) -> Result<(), String>,
}

/// Le moteur peut-il tourner ici ? Whisper : instructions du processeur
/// exigées par whisper.cpp ; Parakeet : ONNX Runtime chargé (bloquant la
/// première fois). Sinon le refus, dans la langue de l'interface.
pub fn engine_support(engine: SpeechEngine) -> Result<(), String> {
    let missing = cpu::missing_for(engine);
    if !missing.is_empty() {
        return Err(cpu::unsupported(missing));
    }
    if engine == SpeechEngine::Parakeet {
        onnx::ensure().map_err(|problem| onnx::unavailable(&problem))?;
    }
    Ok(())
}

struct Core {
    services: Services,
    phase: watch::Sender<DictationPhase>,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Numéro de la dictée en cours : change à chaque début et abandon, pour
    /// qu'une tâche abandonnée ne touche plus à rien.
    session: u64,
    stage: Stage,
    /// Arrêt de l'enregistrement (Recording seulement).
    stop: Option<oneshot::Sender<()>>,
    task: Option<tauri::async_runtime::JoinHandle<()>>,
    /// Dictée qui a produit `last_result`.
    last_session: u64,
    /// Abandon de la dictée en cours, vu par la transcription entre deux
    /// morceaux (son fil ne peut pas être interrompu autrement).
    abort: Arc<AtomicBool>,
    last_result: Option<DictationResult>,
    /// Fermeture de l'app : plus rien ne commence.
    closed: bool,
    /// Séance du mode vocal en cours : aucune dictée ne démarre.
    voice_mode: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Idle,
    /// Enregistrement (micro en cours d'ouverture compris).
    Recording,
    /// Transcription ou nettoyage.
    Processing,
}

/// D'où vient une dictée : du bureau (raccourci, zone de notification,
/// `--dictate`) ou de la page de ChatonLM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    Desktop,
    Page,
}

/// Échec de `dictation_copy_last`.
#[derive(Debug, PartialEq, Eq)]
enum CopyError {
    /// Pas de dernier résultat, ou dictée échouée.
    Nothing,
    Clipboard(String),
}

/// Fin d'une dictée.
enum End {
    Done {
        result: DictationResult,
        notice: Notice,
    },
    Failed(String),
    /// Trop courte : rien à montrer.
    Abandoned,
}

impl Core {
    fn new(services: Services) -> Self {
        Self {
            services,
            phase: watch::Sender::new(DictationPhase::Idle),
            inner: Mutex::new(Inner::default()),
        }
    }

    fn toggle(self: &Arc<Self>, host: Arc<dyn Host>) {
        self.toggle_from(host, Origin::Desktop);
    }

    fn toggle_from(self: &Arc<Self>, host: Arc<dyn Host>, origin: Origin) {
        let stage = self.inner.lock().stage;
        match stage {
            Stage::Idle => self.start_from(host, origin),
            Stage::Recording => self.stop(),
            Stage::Processing => {}
        }
    }

    fn start(self: &Arc<Self>, host: Arc<dyn Host>) {
        self.start_from(host, Origin::Desktop);
    }

    fn start_from(self: &Arc<Self>, host: Arc<dyn Host>, origin: Origin) {
        let mut inner = self.inner.lock();
        if inner.closed || inner.stage != Stage::Idle {
            return;
        }
        if inner.voice_mode {
            // Refus montré comme un échec de la dictée.
            let reason = t!("dictation.voiceModeActive");
            inner.session += 1;
            inner.last_session = inner.session;
            self.services
                .notifier
                .show(Notice::last(Title::Failed, notify::escape(&reason)));
            inner.last_result = Some(DictationResult::failed(reason));
            drop(inner);
            host.changed(DictationPhase::Idle);
            return;
        }
        inner.session += 1;
        let session = inner.session;
        let (stop, stopped) = oneshot::channel();
        inner.stage = Stage::Recording;
        inner.stop = Some(stop);
        let abort = Arc::new(AtomicBool::new(false));
        inner.abort = abort.clone();
        // Sous le verrou : la tâche ne peut finir avant d'être rangée.
        let core = self.clone();
        let task_host = host.clone();
        inner.task = Some(tauri::async_runtime::spawn(async move {
            core.run(task_host, session, stopped, abort, origin).await;
        }));
        self.phase.send_replace(DictationPhase::Recording);
        drop(inner);
        host.changed(DictationPhase::Recording);
    }

    /// Réserve la dictée pour le mode vocal (voir
    /// [`Dictation::reserve_for_voice_mode`]).
    fn reserve(&self) -> bool {
        let mut inner = self.inner.lock();
        if inner.stage != Stage::Idle {
            return false;
        }
        inner.voice_mode = true;
        true
    }

    /// Fin de la séance du mode vocal : la dictée peut démarrer.
    fn release(&self) {
        self.inner.lock().voice_mode = false;
    }

    fn stop(&self) {
        let mut inner = self.inner.lock();
        if inner.stage == Stage::Recording
            && let Some(stop) = inner.stop.take()
        {
            let _ = stop.send(());
        }
    }

    /// Vrai si une dictée a été abandonnée.
    fn cancel(&self) -> bool {
        let mut inner = self.inner.lock();
        if inner.stage == Stage::Idle {
            return false;
        }
        inner.session += 1;
        inner.stage = Stage::Idle;
        inner.stop = None;
        inner.abort.store(true, Ordering::SeqCst);
        // Micro fermé et son jeté, requête du nettoyage fermée ; une
        // transcription déjà lancée s'arrête au morceau suivant, sans suite.
        if let Some(task) = inner.task.take() {
            task.abort();
        }
        self.services.notifier.withdraw();
        self.phase.send_replace(DictationPhase::Idle);
        true
    }

    /// Fermeture : plus aucune dictée, celle en cours abandonnée ; attend
    /// le moteur (`limit` au plus) puis le libère.
    async fn shutdown(&self, limit: Duration) {
        self.inner.lock().closed = true;
        self.cancel();
        let engine = self.services.engine.clone();
        let released = tauri::async_runtime::spawn_blocking(move || engine.shutdown(limit)).await;
        if !matches!(released, Ok(true)) {
            eprintln!("chatonlm: the speech model was still busy at exit");
        }
    }

    async fn run(
        self: Arc<Self>,
        host: Arc<dyn Host>,
        session: u64,
        stopped: oneshot::Receiver<()>,
        abort: Arc<AtomicBool>,
        origin: Origin,
    ) {
        let end = self.dictate(&host, session, stopped, abort, origin).await;
        if self.finish(session, end) {
            host.changed(DictationPhase::Idle);
        }
    }

    /// Passe à `stage` (et `phase`), montre `notice`, si la dictée `session`
    /// est toujours en cours.
    fn advance(
        &self,
        host: &Arc<dyn Host>,
        session: u64,
        stage: Stage,
        phase: Option<DictationPhase>,
        notice: Option<Notice>,
    ) -> bool {
        let mut inner = self.inner.lock();
        if inner.session != session {
            return false;
        }
        inner.stage = stage;
        if stage != Stage::Recording {
            inner.stop = None;
        }
        if let Some(notice) = notice {
            self.services.notifier.show(notice);
        }
        if let Some(phase) = phase {
            self.phase.send_replace(phase);
            drop(inner);
            host.changed(phase);
        }
        true
    }

    async fn dictate(
        &self,
        host: &Arc<dyn Host>,
        session: u64,
        mut stopped: oneshot::Receiver<()>,
        abort: Arc<AtomicBool>,
        origin: Origin,
    ) -> End {
        let prepared = match host.prepare().await {
            Ok(prepared) => prepared,
            Err(error) => return End::Failed(error),
        };
        let settings = &prepared.settings;
        if !settings.enabled {
            return End::Failed(t!("dictation.disabled"));
        }
        if settings.speech_model.is_empty() {
            return End::Failed(t!("dictation.noModel"));
        }
        let Some(model) = models::find(&settings.speech_model)
            .filter(|model| models::installed(&prepared.models_dir, model))
        else {
            return End::Failed(t!("dictation.notInstalled"));
        };
        // Moteur que cette machine ne peut pas faire tourner (Whisper sans
        // AVX2..., Parakeet sans ONNX Runtime) : refusé avant d'écouter (le
        // chargement le refuserait de toute façon).
        let support = self.services.support;
        let engine_kind = model.engine;
        match tauri::async_runtime::spawn_blocking(move || support(engine_kind)).await {
            Ok(Ok(())) => {}
            Ok(Err(refusal)) => return End::Failed(refusal),
            Err(error) => return End::Failed(engine::transcription_failed(&error)),
        }

        // Modèle chargé pendant que l'on parle.
        let spec = ModelSpec::new(model, &prepared.models_dir);
        let engine = self.services.engine.clone();
        let _hold = engine.hold();
        engine.preload(spec.clone());

        let microphone = self.services.microphone.clone();
        let recording = match tauri::async_runtime::spawn_blocking(move || microphone.open()).await
        {
            Ok(Ok(recording)) => recording,
            Ok(Err(error)) => return End::Failed(error),
            Err(error) => return End::Failed(audio::could_not_open(&error)),
        };
        let body = listening_body(settings.mode, prepared.trigger.as_deref());
        let listening = Notice::progress(Title::Listening, &body);
        if !self.advance(host, session, Stage::Recording, None, Some(listening)) {
            return End::Abandoned;
        }
        tokio::select! {
            // Arrêt demandé (ou canal fermé).
            _ = &mut stopped => {}
            () = tokio::time::sleep(self.services.max_recording) => {}
        }

        let samples =
            match tauri::async_runtime::spawn_blocking(move || recording.finish().to_16k()).await {
                Ok(samples) => samples,
                Err(error) => return End::Failed(t!("dictation.recordingFailed", error = error)),
            };
        if audio::duration(&samples) < audio::MIN_DURATION {
            return End::Abandoned;
        }
        if !audio::has_speech(&samples) {
            return End::Failed(audio::no_speech());
        }

        let transcribing = Notice::progress(Title::Transcribing, "");
        if !self.advance(
            host,
            session,
            Stage::Processing,
            Some(DictationPhase::Transcribing),
            Some(transcribing),
        ) {
            return End::Abandoned;
        }
        let request = Request {
            language: (settings.language != languages::AUTO).then(|| settings.language.clone()),
            glossary: settings.dictionary.clone(),
        };
        let transcribed = tauri::async_runtime::spawn_blocking(move || {
            engine.transcribe(&spec, &samples, &request, &abort)
        })
        .await;
        let raw = match transcribed {
            Ok(Ok(text)) => text.trim().to_owned(),
            Ok(Err(error)) => return End::Failed(error),
            Err(error) => return End::Failed(engine::transcription_failed(&error)),
        };
        if raw.is_empty() {
            return End::Failed(audio::no_speech());
        }

        let polishing = !settings.polish_model.is_empty();
        let (text, polished, mut error) = if polishing {
            self.advance(
                host,
                session,
                Stage::Processing,
                Some(DictationPhase::Polishing),
                None,
            );
            let polished = match host.server().await {
                Ok(server) => polish::polish(&server, settings, prepared.always_thinks, &raw).await,
                Err(reason) => Err(t!("dictation.polish.failed", detail = reason)),
            };
            match polished {
                Ok(text) => (text, true, None),
                Err(reason) => (raw.clone(), false, Some(reason)),
            }
        } else {
            (raw.clone(), false, None)
        };

        let raw_copy = polishing && !polished;
        let mut copy_failed = false;
        let mut pasted = false;
        let mut paste_error = None;
        let (title, copied) = if settings.auto_copy {
            match self.services.clipboard.copy(text.clone()).await {
                Ok(()) => {
                    // Collé là où se trouve le curseur ; sinon le texte reste
                    // dans le presse-papiers et la raison est dite.
                    if settings.auto_paste && origin == Origin::Desktop {
                        match self.paste().await {
                            Ok(()) => pasted = true,
                            Err(error) => paste_error = Some(error.explain()),
                        }
                    }
                    let title = match (pasted, raw_copy) {
                        (true, true) => Title::PastedRaw,
                        (true, false) => Title::Pasted,
                        (false, true) => Title::CopiedRaw,
                        (false, false) => Title::Copied,
                    };
                    (title, true)
                }
                Err(reason) => {
                    error = Some(copy_error(&reason));
                    copy_failed = true;
                    (Title::NotCopied, false)
                }
            }
        } else if raw_copy {
            // Copie automatique coupée : pas une erreur, « Copy » proposé.
            (Title::ReadyRaw, false)
        } else {
            (Title::Ready, false)
        };
        let mut body = match &error {
            Some(reason) if title == Title::NotCopied => notify::escape(reason),
            _ if !settings.notify_text => notify::text_hidden(),
            _ => notify::preview(&text),
        };
        if let Some(reason) = &paste_error {
            body.push_str("\n\n");
            body.push_str(&notify::escape(reason));
        }
        let mut notice = Notice::last(title, body);
        if !settings.auto_copy {
            notice = notice.offering(Offer {
                session,
                text: text.clone(),
            });
        }
        End::Done {
            result: DictationResult {
                at: now_ms(),
                text,
                raw,
                polished,
                copied,
                error,
                copy_failed,
                pasted,
                paste_error,
            },
            notice,
        }
    }

    /// Fin de la dictée `session` si elle est toujours en cours : résultat,
    /// notification, retour à Idle. Vrai si l'état a changé.
    fn finish(&self, session: u64, end: End) -> bool {
        let mut inner = self.inner.lock();
        if inner.session != session {
            return false;
        }
        inner.stage = Stage::Idle;
        inner.stop = None;
        inner.task = None;
        inner.last_session = session;
        let notifier = &self.services.notifier;
        match end {
            End::Done { result, notice } => {
                notifier.show(notice);
                inner.last_result = Some(result);
            }
            End::Failed(reason) => {
                notifier.show(Notice::last(Title::Failed, notify::escape(&reason)));
                inner.last_result = Some(DictationResult::failed(reason));
            }
            End::Abandoned => notifier.withdraw(),
        }
        self.phase.send_replace(DictationPhase::Idle);
        true
    }
}

impl Core {
    /// Colle le presse-papiers (Ctrl+V) dans une tâche à part : une dictée
    /// abandonnée pendant le collage (boîte d'autorisation du bureau
    /// ouverte) ne l'interrompt pas, et la session du portail est toujours
    /// fermée.
    async fn paste(&self) -> Result<(), PasteError> {
        let clipboard = self.services.clipboard.clone();
        tauri::async_runtime::spawn(async move { clipboard.paste().await })
            .await
            .map_err(|error| PasteError::Other(error.to_string()))?
    }

    /// Copie le texte d'un résultat (action « Copy ») ; vrai si c'est encore
    /// le dernier résultat, désormais copié.
    async fn copy_offer(&self, offer: Offer) -> Result<bool, String> {
        self.services.clipboard.copy(offer.text).await?;
        Ok(self.mark_copied(offer.session))
    }

    /// Copie le texte du dernier résultat (non vide).
    async fn copy_last(&self) -> Result<(), CopyError> {
        let (session, text) = {
            let inner = self.inner.lock();
            match &inner.last_result {
                Some(result) if !result.text.is_empty() => {
                    (inner.last_session, result.text.clone())
                }
                _ => return Err(CopyError::Nothing),
            }
        };
        self.services
            .clipboard
            .copy(text)
            .await
            .map_err(CopyError::Clipboard)?;
        self.mark_copied(session);
        Ok(())
    }

    /// Le dernier résultat, s'il vient de la dictée `session`, est copié.
    fn mark_copied(&self, session: u64) -> bool {
        let mut inner = self.inner.lock();
        if inner.last_session != session {
            return false;
        }
        match inner.last_result.as_mut() {
            Some(result) => {
                result.copied = true;
                true
            }
            None => false,
        }
    }
}

/// Corps de « Listening… » : comment arrêter.
fn listening_body(mode: DictationMode, trigger: Option<&str>) -> String {
    match (trigger, mode) {
        (Some(trigger), DictationMode::Toggle) => {
            t!("notifications.pressAgain", trigger = trigger)
        }
        (Some(trigger), DictationMode::Hold) => t!("notifications.release", trigger = trigger),
        (None, _) => t!("notifications.runAgain"),
    }
}

/// Échec de la copie dans le presse-papiers (`reason` : détail technique).
fn copy_error(reason: &str) -> String {
    t!("dictation.copyFailed", reason = reason)
}

#[cfg(test)]
mod tests;
