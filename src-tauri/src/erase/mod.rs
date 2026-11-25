//! « Erase my data » (Settings › General, propre à ChatonLM) : efface d'un
//! seul coup les données de l'utilisateur, jamais les modèles ; l'app reste
//! ouverte et montre l'état vide.
//!
//! Déroulement ([`run`], dans l'ordre de [`Steps`]) :
//! 1. ce qui tourne et utiliserait les données s'arrête, sans fermer les
//!    services (un bot, un éditeur, une séance peuvent reprendre ensuite) :
//!    mode vocal, bots d'Easy Link, pont de VS Code (coupé), réponses (et
//!    leurs images), cours, corrections, transcription et synthèse, projet de
//!    l'Atelier (la base seulement : les dossiers livrés restent), dictée,
//!    lecture à voix haute ; les derniers textes gardés en mémoire (dictée,
//!    correction de la sélection) sont oubliés. Les modèles chargés restent
//!    chargés, les téléchargements continuent ;
//! 2. la base est effacée en une transaction (`store::erase`) ; un échec
//!    n'efface rien, remet en route les bots et le pont, et s'arrête là ;
//! 3. les réglages remis à zéro sont appliqués à ce qui tourne (thème de la
//!    fenêtre, raccourcis globaux, dictée, croix, mises à jour, modèles
//!    distants) et les vues prévenues (Easy Link, pont) ;
//! 4. les fichiers sont effacés ([`files`], journal de l'app vidé par son
//!    propre fichier) : un échec est rendu dans la liste des restes, sans
//!    rien défaire ;
//! 5. les données de navigation de la page (stockage, cache de WebKitGTK ou
//!    de WebView2) sont effacées par la webview.
//!
//! Linux et Windows : même déroulement ; sous Windows, un fichier ouvert par
//! un autre programme peut refuser d'être supprimé (il est alors vidé, sinon
//! rendu dans les restes), et les combinaisons choisies des raccourcis
//! globaux reviennent à celles par défaut.

pub mod files;

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::Store;

/// Résultat rendu à la page.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EraseReport {
    /// Fichiers qui n'ont pu être ni supprimés ni vidés (chemins relatifs au
    /// dossier des données) ; vide : tout est effacé.
    pub leftovers: Vec<String>,
    /// Données de navigation de la page (stockage, cache) effacées.
    pub browsing_data_cleared: bool,
}

/// Étapes de l'effacement ; leur ordre est celui de [`run`] (essais).
pub(crate) trait Steps {
    /// Travail en cours arrêté, textes gardés en mémoire oubliés.
    fn stop_work(&self) -> impl Future<Output = ()> + Send;
    /// Base effacée, tout ou rien.
    fn erase_database(&self) -> impl Future<Output = AppResult<()>> + Send;
    /// Base intacte après un échec : bots et pont remis comme avant.
    fn resume(&self) -> impl Future<Output = ()> + Send;
    /// Réglages par défaut appliqués à ce qui tourne, vues prévenues.
    fn apply_defaults(&self) -> impl Future<Output = ()> + Send;
    /// Fichiers effacés ; rend ceux qui restent.
    fn erase_files(&self) -> impl Future<Output = Vec<String>> + Send;
    /// Données de navigation effacées ; faux si la webview a refusé.
    fn clear_browsing_data(&self) -> bool;
}

/// Un effacement à la fois.
static ERASING: AtomicBool = AtomicBool::new(false);

/// Relâche le drapeau d'un effacement, même sur un échec.
struct Erasing<'a>(&'a AtomicBool);

impl Drop for Erasing<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Efface les données, dans l'ordre des étapes ; refusé si un effacement est
/// déjà en cours (`busy`).
pub(crate) async fn run<S: Steps>(busy: &AtomicBool, steps: &S) -> AppResult<EraseReport> {
    if busy.swap(true, Ordering::SeqCst) {
        return Err(AppError::Invalid(t!("erase.busy")));
    }
    let _erasing = Erasing(busy);
    steps.stop_work().await;
    if let Err(error) = steps.erase_database().await {
        crate::app_log!("chatonlm: erase: database kept: {error}");
        steps.resume().await;
        return Err(error);
    }
    steps.apply_defaults().await;
    let leftovers = steps.erase_files().await;
    let browsing_data_cleared = steps.clear_browsing_data();
    crate::app_log!(
        "chatonlm: user data erased ({} file(s) kept, browsing data {})",
        leftovers.len(),
        if browsing_data_cleared {
            "cleared"
        } else {
            "kept"
        }
    );
    Ok(EraseReport {
        leftovers,
        browsing_data_cleared,
    })
}

/// Effacement de l'app (commande `erase_user_data`). Sans base lisible,
/// refusé avant d'arrêter quoi que ce soit (comme un effacement déjà en
/// cours : erreurs `Invalid`, que la page distingue d'un échec de la base).
pub async fn erase(app: &AppHandle) -> AppResult<EraseReport> {
    if app.try_state::<Store>().is_none() {
        return Err(AppError::Invalid(t!("erase.noDatabase")));
    }
    let steps = AppSteps {
        app,
        services: AppServices(app),
        bridge_was_on: AtomicBool::new(false),
    };
    run(&ERASING, &steps).await
}

/// Attente d'une transcription ou d'une synthèse en cours, au plus.
const WORK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Réglages relus dans la base effacée, appliqués à ce qui tourne.
pub(crate) struct Defaults {
    pub settings: crate::store::settings::Settings,
    pub dictation: crate::store::dictation::DictationSettings,
    pub close_to_tray: bool,
    pub auto_download: bool,
}

/// Ce que l'effacement arrête, remet en route ou règle dans l'app, un
/// service à la fois (essais : un enregistreur vérifie qu'aucun n'est
/// oublié).
pub(crate) trait Services: Sync {
    // Avant la base : plus rien de nouveau, travail en cours arrêté.
    fn stop_voice_mode(&self);
    fn stop_bots(&self) -> impl Future<Output = ()> + Send;
    /// Pont coupé ; vrai s'il était activé.
    fn turn_off_bridge(&self) -> impl Future<Output = bool> + Send;
    /// Réponses, cours, corrections et correction de la sélection arrêtés.
    fn stop_model_work(&self);
    /// Réponses attendues (enregistrées avant l'effacement).
    fn stop_replies(&self) -> impl Future<Output = ()> + Send;
    fn forget_transcription(&self) -> impl Future<Output = ()> + Send;
    fn forget_synthesis(&self) -> impl Future<Output = ()> + Send;
    /// Projet de l'Atelier en cours arrêté sans rien écrire, réponses
    /// attendues oubliées (les dossiers des projets restent : ce sont des
    /// livrables de l'utilisateur, hors du dossier des données).
    fn forget_workshop(&self) -> impl Future<Output = ()> + Send;
    /// Dictée abandonnée, dernier texte dicté oublié.
    fn stop_dictation(&self);
    fn stop_reading(&self);
    /// Dernière correction de la sélection oubliée.
    fn forget_proofread(&self);
    // Base gardée (échec) : remis comme avant.
    fn restart_bots(&self) -> impl Future<Output = ()> + Send;
    fn turn_on_bridge(&self) -> impl Future<Output = ()> + Send;
    // Base effacée.
    /// Messages déjà servis par les bots oubliés (seulement une fois la base
    /// effacée : gardés, ils empêchent un message d'être servi deux fois).
    fn forget_served(&self);
    fn apply_theme(&self, theme: crate::store::settings::Theme);
    fn apply_auto_read(&self, enabled: bool);
    /// Raccourcis globaux de la correction et de la lecture de la sélection.
    fn apply_shortcuts(&self, proofread: bool, read_aloud: bool)
    -> impl Future<Output = ()> + Send;
    /// Raccourci et moteur de la dictée.
    fn apply_dictation(
        &self,
        settings: &crate::store::dictation::DictationSettings,
    ) -> impl Future<Output = ()> + Send;
    fn apply_close_to_tray(&self, enabled: bool);
    /// Combinaisons des raccourcis globaux revenues à celles par défaut
    /// (Windows ; rien sous Linux).
    fn reset_triggers(&self) -> impl Future<Output = ()> + Send;
    fn apply_auto_download(&self, enabled: bool) -> impl Future<Output = ()> + Send;
    /// Modèles distants relus, état d'Easy Link et du pont envoyé à la page.
    fn refresh_views(&self) -> impl Future<Output = ()> + Send;
}

/// Arrête tout ce qui utiliserait les données ; vrai si le pont était
/// activé.
pub(crate) async fn stop_services<S: Services>(services: &S) -> bool {
    // Plus rien de nouveau : séance vocale, bots, éditeurs.
    services.stop_voice_mode();
    services.stop_bots().await;
    let bridge_was_on = services.turn_off_bridge().await;
    // Travail en cours.
    services.stop_model_work();
    services.stop_replies().await;
    services.forget_transcription().await;
    services.forget_synthesis().await;
    services.forget_workshop().await;
    services.stop_dictation();
    services.stop_reading();
    services.forget_proofread();
    bridge_was_on
}

/// Base gardée : bots et pont remis comme avant.
pub(crate) async fn restart_services<S: Services>(services: &S, bridge_was_on: bool) {
    services.restart_bots().await;
    if bridge_was_on {
        services.turn_on_bridge().await;
    }
}

/// Base effacée : réglages par défaut appliqués à ce qui tourne.
pub(crate) async fn apply_defaults<S: Services>(services: &S, defaults: &Defaults) {
    services.forget_served();
    services.apply_theme(defaults.settings.theme);
    services.apply_auto_read(defaults.settings.auto_read);
    services
        .apply_shortcuts(
            defaults.settings.proofread_shortcut,
            defaults.settings.read_aloud_shortcut,
        )
        .await;
    services.apply_dictation(&defaults.dictation).await;
    services.apply_close_to_tray(defaults.close_to_tray);
    services.reset_triggers().await;
    services.apply_auto_download(defaults.auto_download).await;
    services.refresh_views().await;
}

/// Les étapes, pour l'app.
struct AppSteps<'a> {
    app: &'a AppHandle,
    services: AppServices<'a>,
    /// Pont activé avant l'effacement (remis ainsi si la base est gardée).
    bridge_was_on: AtomicBool,
}

impl Steps for AppSteps<'_> {
    async fn stop_work(&self) {
        let was_on = stop_services(&self.services).await;
        self.bridge_was_on.store(was_on, Ordering::SeqCst);
    }

    async fn erase_database(&self) -> AppResult<()> {
        let store = self
            .app
            .try_state::<Store>()
            .ok_or_else(|| AppError::Invalid(t!("erase.noDatabase")))?;
        store.call(crate::store::erase::erase_user_data).await
    }

    async fn resume(&self) {
        restart_services(&self.services, self.bridge_was_on.load(Ordering::SeqCst)).await;
    }

    async fn apply_defaults(&self) {
        let store = self.app.state::<Store>().inner().clone();
        let saved = store
            .call(|c| {
                Ok(Defaults {
                    settings: crate::store::settings::get(c)?,
                    dictation: crate::store::dictation::get(c)?,
                    close_to_tray: crate::store::desktop::close_to_tray(c)?,
                    auto_download: crate::store::updates::get(c)?.auto_download,
                })
            })
            .await;
        match saved {
            Ok(defaults) => apply_defaults(&self.services, &defaults).await,
            Err(error) => crate::app_log!("chatonlm: erase: settings not read again: {error}"),
        }
    }

    async fn erase_files(&self) -> Vec<String> {
        let mut leftovers = Vec::new();
        if let Err(error) = crate::logging::clear() {
            crate::app_log!("chatonlm: erase: app log not emptied: {error}");
            leftovers.push(format!("logs/{}", crate::logging::FILE_NAME));
        }
        let Ok(data_dir) = crate::paths::data_dir(self.app) else {
            return leftovers;
        };
        let found = tauri::async_runtime::spawn_blocking(move || {
            files::erase(&data_dir, crate::logging::FILE_NAME)
        })
        .await
        .unwrap_or_else(|error| {
            crate::app_log!("chatonlm: erase: files not erased: {error}");
            vec![String::from(".")]
        });
        leftovers.extend(found);
        leftovers
    }

    fn clear_browsing_data(&self) -> bool {
        let windows = self.app.webview_windows();
        if windows.is_empty() {
            return false;
        }
        let mut cleared = true;
        for window in windows.values() {
            if let Err(error) = window.clear_all_browsing_data() {
                crate::app_log!("chatonlm: erase: browsing data kept: {error}");
                cleared = false;
            }
        }
        cleared
    }
}

/// Les services de l'app (états gérés par Tauri ; absents quand la base est
/// illisible : Easy Link, pont, transcription).
struct AppServices<'a>(&'a AppHandle);

impl Services for AppServices<'_> {
    fn stop_voice_mode(&self) {
        if let Some(voice_mode) = self.0.try_state::<crate::voice_mode::VoiceMode>() {
            voice_mode.stop_all();
        }
    }

    async fn stop_bots(&self) {
        if let Some(easy_link) = self.0.try_state::<crate::easy_link::EasyLink>() {
            easy_link.erase().await;
        }
    }

    async fn turn_off_bridge(&self) -> bool {
        let Some(bridge) = self.0.try_state::<crate::bridge::Bridge>() else {
            return false;
        };
        let bridge = bridge.inner().clone();
        let was_on = bridge.state().await.is_ok_and(|state| state.enabled);
        if let Err(error) = bridge.set_enabled(false).await {
            crate::app_log!("chatonlm: erase: VS Code bridge not turned off: {error}");
        }
        was_on
    }

    fn stop_model_work(&self) {
        crate::stop_model_work(self.0);
    }

    async fn stop_replies(&self) {
        self.0
            .state::<crate::chat::ChatEngine>()
            .stop_all(crate::EXIT_GRACE)
            .await;
    }

    async fn forget_transcription(&self) {
        if let Some(transcription) = self.0.try_state::<crate::transcription::Transcription>() {
            transcription.forget(WORK_WAIT).await;
        }
    }

    async fn forget_synthesis(&self) {
        if let Some(synthesizer) = self
            .0
            .try_state::<crate::transcription::synthesis::Synthesizer>()
        {
            synthesizer.forget(WORK_WAIT).await;
        }
    }

    async fn forget_workshop(&self) {
        if let Some(workshop) = self.0.try_state::<crate::workshop::Workshop>() {
            workshop.forget(WORK_WAIT).await;
        }
    }

    fn stop_dictation(&self) {
        let dictation = self.0.state::<crate::dictation::Dictation>();
        dictation.cancel(self.0);
        dictation.forget_last(self.0);
    }

    fn stop_reading(&self) {
        self.0.state::<crate::tts::ReadAloud>().stop();
    }

    fn forget_proofread(&self) {
        self.0
            .state::<crate::proofread::Proofreader>()
            .forget_last_selection();
    }

    async fn restart_bots(&self) {
        if let Some(easy_link) = self.0.try_state::<crate::easy_link::EasyLink>() {
            easy_link.start_saved().await;
        }
    }

    async fn turn_on_bridge(&self) {
        if let Some(bridge) = self.0.try_state::<crate::bridge::Bridge>()
            && let Err(error) = bridge.inner().clone().set_enabled(true).await
        {
            crate::app_log!("chatonlm: erase: VS Code bridge not turned on again: {error}");
        }
    }

    fn forget_served(&self) {
        if let Some(easy_link) = self.0.try_state::<crate::easy_link::EasyLink>() {
            easy_link.forget_served();
        }
    }

    fn apply_theme(&self, theme: crate::store::settings::Theme) {
        crate::window::apply_theme(self.0, theme);
    }

    fn apply_auto_read(&self, enabled: bool) {
        self.0
            .state::<crate::tts::ReadAloud>()
            .set_auto_read(enabled);
    }

    async fn apply_shortcuts(&self, proofread: bool, read_aloud: bool) {
        if let Err(error) = crate::desktop::apply_proofread_shortcut(self.0, proofread).await {
            crate::app_log!("chatonlm: erase: proofread shortcut not applied: {error}");
        }
        crate::desktop::apply_read_aloud_shortcut(self.0, read_aloud);
    }

    async fn apply_dictation(&self, settings: &crate::store::dictation::DictationSettings) {
        if let Err(error) = crate::desktop::apply(self.0, settings).await {
            crate::app_log!("chatonlm: erase: dictation shortcut not applied: {error}");
        }
        let dictation = self.0.state::<crate::dictation::Dictation>();
        dictation.settings_changed(settings);
        dictation.emit_status(self.0);
    }

    fn apply_close_to_tray(&self, enabled: bool) {
        crate::desktop::state(self.0).set_close_to_tray(enabled);
    }

    async fn reset_triggers(&self) {
        crate::desktop::reset_all_triggers(self.0).await;
    }

    async fn apply_auto_download(&self, enabled: bool) {
        if let Err(error) = crate::updater::set_auto_download(self.0, enabled).await {
            crate::app_log!("chatonlm: erase: auto-download not applied: {error}");
        }
    }

    async fn refresh_views(&self) {
        if let Some(access) = self.0.try_state::<crate::llm::Access>() {
            access.remote().invalidate();
        }
        if let Some(easy_link) = self.0.try_state::<crate::easy_link::EasyLink>() {
            easy_link.publish().await;
        }
        if let Some(bridge) = self.0.try_state::<crate::bridge::Bridge>() {
            bridge.inner().clone().publish().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    #[derive(Default)]
    struct Recorder {
        steps: Mutex<Vec<&'static str>>,
        database_fails: bool,
        leftovers: Vec<String>,
        browsing_data: bool,
    }

    impl Recorder {
        fn log(&self, step: &'static str) {
            self.steps.lock().push(step);
        }
    }

    impl Steps for Recorder {
        async fn stop_work(&self) {
            tokio::task::yield_now().await;
            self.log("stop");
        }
        async fn erase_database(&self) -> AppResult<()> {
            self.log("database");
            if self.database_fails {
                return Err(AppError::Internal("disk full".into()));
            }
            Ok(())
        }
        async fn resume(&self) {
            self.log("resume");
        }
        async fn apply_defaults(&self) {
            self.log("defaults");
        }
        async fn erase_files(&self) -> Vec<String> {
            self.log("files");
            self.leftovers.clone()
        }
        fn clear_browsing_data(&self) -> bool {
            self.log("browsing");
            self.browsing_data
        }
    }

    fn erase(busy: &AtomicBool, steps: &Recorder) -> AppResult<EraseReport> {
        tauri::async_runtime::block_on(run(busy, steps))
    }

    /// Travail arrêté d'abord, base ensuite, puis réglages, fichiers et
    /// navigation ; les restes sont rendus à la page.
    #[test]
    fn steps_run_in_order() {
        let busy = AtomicBool::new(false);
        let steps = Recorder {
            leftovers: vec!["logs/sd-server.log".into()],
            browsing_data: true,
            ..Recorder::default()
        };
        let report = erase(&busy, &steps).unwrap();
        assert_eq!(
            *steps.steps.lock(),
            ["stop", "database", "defaults", "files", "browsing"]
        );
        assert_eq!(
            report,
            EraseReport {
                leftovers: vec!["logs/sd-server.log".into()],
                browsing_data_cleared: true,
            }
        );
        assert!(!busy.load(Ordering::SeqCst));
    }

    /// Base gardée (erreur) : rien d'autre n'est effacé, bots et pont
    /// remis, l'erreur rendue ; un nouvel essai est permis.
    #[test]
    fn a_database_failure_erases_no_file() {
        let busy = AtomicBool::new(false);
        let steps = Recorder {
            database_fails: true,
            ..Recorder::default()
        };
        let error = erase(&busy, &steps).unwrap_err();
        assert!(matches!(error, AppError::Internal(_)), "{error:?}");
        assert_eq!(*steps.steps.lock(), ["stop", "database", "resume"]);
        assert!(!busy.load(Ordering::SeqCst));
    }

    /// Un seul effacement à la fois : le second est refusé sans rien faire.
    #[test]
    fn a_second_erase_is_refused_while_one_runs() {
        let busy = AtomicBool::new(true);
        let steps = Recorder::default();
        let error = erase(&busy, &steps).unwrap_err();
        assert!(matches!(error, AppError::Invalid(_)), "{error:?}");
        assert!(steps.steps.lock().is_empty());
        assert!(busy.load(Ordering::SeqCst), "the running erase keeps it");
    }

    /// Enregistreur des services : chaque appel, avec ses valeurs.
    #[derive(Default)]
    struct Calls {
        log: Mutex<Vec<String>>,
    }

    impl Calls {
        fn note(&self, call: impl Into<String>) {
            self.log.lock().push(call.into());
        }
        fn taken(&self) -> Vec<String> {
            std::mem::take(&mut *self.log.lock())
        }
    }

    impl Services for Calls {
        fn stop_voice_mode(&self) {
            self.note("voice mode");
        }
        async fn stop_bots(&self) {
            self.note("bots");
        }
        async fn turn_off_bridge(&self) -> bool {
            self.note("bridge off");
            true
        }
        fn stop_model_work(&self) {
            self.note("model work");
        }
        async fn stop_replies(&self) {
            self.note("replies");
        }
        async fn forget_transcription(&self) {
            self.note("transcription");
        }
        async fn forget_synthesis(&self) {
            self.note("synthesis");
        }
        async fn forget_workshop(&self) {
            self.note("workshop");
        }
        fn stop_dictation(&self) {
            self.note("dictation");
        }
        fn stop_reading(&self) {
            self.note("reading");
        }
        fn forget_proofread(&self) {
            self.note("proofread");
        }
        async fn restart_bots(&self) {
            self.note("bots again");
        }
        async fn turn_on_bridge(&self) {
            self.note("bridge on");
        }
        fn forget_served(&self) {
            self.note("served");
        }
        fn apply_theme(&self, theme: crate::store::settings::Theme) {
            self.note(format!("theme {theme:?}"));
        }
        fn apply_auto_read(&self, enabled: bool) {
            self.note(format!("auto read {enabled}"));
        }
        async fn apply_shortcuts(&self, proofread: bool, read_aloud: bool) {
            self.note(format!("shortcuts {proofread} {read_aloud}"));
        }
        async fn apply_dictation(&self, settings: &crate::store::dictation::DictationSettings) {
            self.note(format!(
                "dictation {} {}",
                settings.enabled, settings.speech_model
            ));
        }
        fn apply_close_to_tray(&self, enabled: bool) {
            self.note(format!("close to tray {enabled}"));
        }
        async fn reset_triggers(&self) {
            self.note("triggers");
        }
        async fn apply_auto_download(&self, enabled: bool) {
            self.note(format!("auto download {enabled}"));
        }
        async fn refresh_views(&self) {
            self.note("views");
        }
    }

    /// Avant la base : séance vocale, bots, pont, réponses, cours,
    /// corrections, transcription, synthèse, Atelier, dictée, lecture et dernière
    /// correction de la sélection, rien de plus ; le pont activé est retenu.
    #[test]
    fn every_service_is_stopped_before_the_database() {
        let calls = Calls::default();
        let bridge_was_on = tauri::async_runtime::block_on(stop_services(&calls));
        assert!(bridge_was_on);
        assert_eq!(
            calls.taken(),
            [
                "voice mode",
                "bots",
                "bridge off",
                "model work",
                "replies",
                "transcription",
                "synthesis",
                "workshop",
                "dictation",
                "reading",
                "proofread"
            ]
        );
        // Base gardée : bots relancés, pont rouvert s'il l'était seulement.
        tauri::async_runtime::block_on(restart_services(&calls, true));
        assert_eq!(calls.taken(), ["bots again", "bridge on"]);
        tauri::async_runtime::block_on(restart_services(&calls, false));
        assert_eq!(calls.taken(), ["bots again"]);
    }

    /// Base effacée : les réglages relus (ceux par défaut, modèle de la
    /// dictée gardé) sont appliqués à tout ce qui tourne, messages servis
    /// oubliés compris, puis les vues prévenues.
    #[test]
    fn the_defaults_reach_every_running_service() {
        let store = Store::open_in_memory().unwrap();
        let defaults = store
            .blocking(|c| {
                c.execute_batch(
                    "UPDATE settings SET theme = 'dark', auto_read = 1, proofread_shortcut = 1;
                     UPDATE dictation_settings SET enabled = 1, speech_model = 'whisper-base';
                     UPDATE desktop_settings SET close_to_tray = 0;
                     UPDATE update_settings SET auto_download = 0;",
                )?;
                crate::store::erase::erase_user_data(c)?;
                Ok(Defaults {
                    settings: crate::store::settings::get(c)?,
                    dictation: crate::store::dictation::get(c)?,
                    close_to_tray: crate::store::desktop::close_to_tray(c)?,
                    auto_download: crate::store::updates::get(c)?.auto_download,
                })
            })
            .unwrap();
        let calls = Calls::default();
        tauri::async_runtime::block_on(apply_defaults(&calls, &defaults));
        assert_eq!(
            calls.taken(),
            [
                "served",
                "theme System",
                "auto read false",
                "shortcuts false false",
                "dictation false whisper-base",
                "close to tray true",
                "triggers",
                "auto download true",
                "views"
            ]
        );
    }
}
