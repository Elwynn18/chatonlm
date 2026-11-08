//! Fin de l'app, quelle qu'en soit la cause : fermeture demandée (Ctrl+Q,
//! « Quit ChatonLM », dernière fenêtre fermée), signal du système (Linux :
//! SIGTERM, SIGINT, SIGHUP, voir `shutdown/linux.rs`) ou fin de la session
//! (Windows : `WM_ENDSESSION`, voir `shutdown/windows.rs`). Toutes passent par
//! [`on_exit`], une seule fois : la fenêtre disparaît d'abord, puis les
//! services s'arrêtent dans l'ordre de [`stop`], conversations éphémères
//! effacées avant tout ce qui peut prendre du temps (une fin de session peut
//! ne pas laisser finir le reste).

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager};

#[cfg(target_os = "linux")]
#[path = "shutdown/linux.rs"]
mod os;
#[cfg(windows)]
#[path = "shutdown/windows.rs"]
mod os;

/// Écoute ce qui, venu du système, doit fermer l'app comme Ctrl+Q.
pub fn listen(app: &AppHandle) {
    os::listen(app);
}

/// Étapes de la fin de l'app ; leur ordre est celui de [`stop`] et
/// [`exit_once`] (essai `steps_run_once_in_order`).
pub(crate) trait Steps {
    /// Fenêtres masquées, aussitôt (l'arrêt peut durer plusieurs secondes).
    fn hide_windows(&self);
    /// Plus rien de nouveau : mode vocal, Easy Link, lien VS Code.
    fn stop_intake(&self);
    /// Conversations éphémères effacées.
    async fn forget_ephemeral(&self);
    /// Réponses, cours, corrections, téléchargements, dictée, lecture et
    /// images arrêtés.
    async fn stop_work(&self);
    /// Modèles déchargés (réglage « Unload models when quitting »), puis
    /// l'Ollama intégré arrêté.
    async fn release_models(&self);
    /// Processus web de WebKit arrêtés.
    fn end_web(&self);
}

/// Arrête les services (voir `crate::stop_services`). Les conversations
/// éphémères sont effacées dès que plus rien de nouveau n'arrive, avant les
/// arrêts qui attendent (réponses, téléchargements, déchargement des
/// modèles), puis de nouveau une fois les réponses arrêtées : une réponse en
/// cours dans l'une d'elles a pu y écrire entre-temps.
pub(crate) async fn stop<S: Steps>(steps: &S) {
    steps.stop_intake();
    steps.forget_ephemeral().await;
    steps.stop_work().await;
    steps.forget_ephemeral().await;
    steps.release_models().await;
}

/// Fin de l'app, une seule fois même si plusieurs événements l'annoncent
/// (`ExitRequested` puis `Exit`) : fenêtres masquées d'abord, puis [`stop`],
/// puis les processus web.
pub(crate) fn exit_once<S: Steps>(done: &AtomicBool, steps: &S) {
    if done.swap(true, Ordering::SeqCst) {
        return;
    }
    steps.hide_windows();
    tauri::async_runtime::block_on(stop(steps));
    steps.end_web();
}

/// Fin de l'app commencée.
static EXITING: AtomicBool = AtomicBool::new(false);

/// L'app est en train de finir : un nouveau signal n'y change rien (Linux ;
/// Windows n'a pas de signal à écouter).
#[cfg_attr(windows, allow(dead_code))]
pub fn exiting() -> bool {
    EXITING.load(Ordering::SeqCst)
}

/// `RunEvent::ExitRequested` (fermeture demandée, `app.exit`) et
/// `RunEvent::Exit` (seul annoncé par une fin de session Windows), sur le fil
/// principal.
pub fn on_exit(app: &AppHandle) {
    exit_once(&EXITING, &AppSteps(app));
}

/// Les étapes, pour l'app.
pub(crate) struct AppSteps<'a>(pub &'a AppHandle);

impl Steps for AppSteps<'_> {
    fn hide_windows(&self) {
        os::hide_windows(self.0);
    }

    fn stop_intake(&self) {
        crate::stop_intake(self.0);
    }

    async fn forget_ephemeral(&self) {
        crate::forget_ephemeral_chats(self.0).await;
    }

    async fn stop_work(&self) {
        crate::stop_work(self.0).await;
    }

    async fn release_models(&self) {
        crate::release_models(self.0).await;
    }

    fn end_web(&self) {
        crate::desktop::terminate_web_processes(self.0);
    }
}

/// Fenêtres de l'app masquées (commun aux deux systèmes ; Linux y ajoute
/// l'envoi immédiat au serveur d'affichage).
fn hide_all(app: &AppHandle) {
    for window in app.webview_windows().values() {
        if let Err(error) = window.hide() {
            crate::app_log!("chatonlm: cannot hide the window at exit: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<&'static str>>);

    impl Recorder {
        fn log(&self, step: &'static str) {
            self.0.lock().push(step);
        }
    }

    impl Steps for Recorder {
        fn hide_windows(&self) {
            self.log("hide");
        }
        fn stop_intake(&self) {
            self.log("intake");
        }
        async fn forget_ephemeral(&self) {
            tokio::task::yield_now().await;
            self.log("forget");
        }
        async fn stop_work(&self) {
            tokio::task::yield_now().await;
            self.log("work");
        }
        async fn release_models(&self) {
            tokio::task::yield_now().await;
            self.log("release");
        }
        fn end_web(&self) {
            self.log("web");
        }
    }

    /// Fenêtre masquée d'abord, conversations éphémères effacées avant les
    /// arrêts qui attendent (et de nouveau après les réponses), modèles
    /// libérés ensuite ; une seconde annonce de la fin ne refait rien.
    #[test]
    fn steps_run_once_in_order() {
        let done = AtomicBool::new(false);
        let steps = Recorder::default();
        exit_once(&done, &steps);
        exit_once(&done, &steps);
        assert_eq!(
            *steps.0.lock(),
            [
                "hide", "intake", "forget", "work", "forget", "release", "web"
            ]
        );
    }

    /// Mise à jour (« Restart to update ») : les services s'arrêtent dans le
    /// même ordre, sans toucher aux fenêtres.
    #[test]
    fn stopping_the_services_alone_keeps_the_windows() {
        let steps = Recorder::default();
        tauri::async_runtime::block_on(stop(&steps));
        assert_eq!(
            *steps.0.lock(),
            ["intake", "forget", "work", "forget", "release"]
        );
    }
}
