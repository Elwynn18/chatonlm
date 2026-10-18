//! Raccourci global « Correct selected text » (propre à ChatonLM) : sélection
//! primaire lue sans focus (data-control), corrigée par [`super::run`],
//! copiée dans le presse-papiers, puis collée à la place de la sélection
//! (réglage `proofread_paste`, activé par défaut : Ctrl+V envoyé par
//! [`Clipboard::paste`]) ; notification avec l'action « Show » (page
//! Proofread, [`super::Proofreader::last_selection`]). Réglage coupé, ou
//! collage impossible (bureau sans portail RemoteDesktop, autorisation
//! refusée) : la notification dit de coller par Ctrl+V.
//!
//! Le résultat est toujours copié, même sans faute trouvée : Ctrl+V ne colle
//! jamais un vieux contenu du presse-papiers. Rien n'est collé quand le
//! texte n'a pas changé. Rien n'est copié ni collé si la correction échoue
//! ou si elle est arrêtée : un appui pendant une correction en cours
//! l'arrête.
//!
//! Modèle repris du chat (aucun choisi sur la page Proofread) : un modèle
//! cloud ou distant n'est jamais utilisé, la sélection (de n'importe quelle
//! application) ne quitte pas la machine sans ce choix explicite.
//!
//! Modèle introuvable (supprimé depuis qu'il a été choisi) : la correction se
//! refait avec un modèle chargé dans Ollama (`/api/ps`, le plus récemment
//! servi d'abord) qui tourne sur la machine et produit du texte ; la
//! notification le dit. Le réglage ne change pas. Aucun : l'erreur reste.
//!
//! Notifications (D-Bus, comme la dictée, mais une suite à part : une
//! correction et une dictée simultanées ne se remplacent pas) : progression
//! « Correcting the selected text… », affichée tant que la correction dure,
//! puis le résultat, qui la remplace.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use tauri::{AppHandle, Manager};

use super::{
    Cancel, MAX_CHARS, Outcome, ProofreadErrorCode, ProofreadEvent, ProofreadMode, Proofreader,
    SelectionResult, model_of,
};
use crate::chat::Server;
use crate::dictation::clipboard::{
    BoxFuture, Clipboard, PasteError, SelectionError, SystemClipboard,
};
use crate::dictation::notify::{self, Action, DesktopNotifier, Notice, Notifier, Offer};
use crate::i18n::t;
use crate::ollama::{models, running};
use crate::store::Store;

/// Ce que la correction lit de l'app au moment de l'appui.
pub(crate) struct Prepared {
    /// Adresse effective d'Ollama, contexte et durée en mémoire de l'app.
    pub server: Server,
    /// Modèle de la correction (réglé, sinon celui du chat) ; `None` : aucun.
    pub model: Option<String>,
    /// Modèle repris du chat : aucun n'est choisi sur la page Proofread.
    pub from_chat: bool,
    /// Coller le texte corrigé à la place de la sélection (réglage).
    pub paste: bool,
}

/// L'app vue par la correction de la sélection (remplacée dans les tests).
pub(crate) trait Host: Send + Sync + 'static {
    fn prepare(&self) -> BoxFuture<Result<Prepared, String>>;

    /// Noms des modèles chargés dans Ollama à l'adresse `base`, dans l'ordre
    /// de `/api/ps` (le plus récemment servi d'abord) ; vide si la liste est
    /// illisible.
    fn loaded(&self, base: String) -> BoxFuture<Vec<String>>;
}

/// Moteur de correction : [`super::run`] en mode Correct, ou un faux.
pub(crate) trait Corrector: Send + Sync + 'static {
    fn correct(
        &self,
        server: Server,
        model: String,
        text: String,
        cancel: Cancel,
    ) -> BoxFuture<Outcome>;
}

/// Le vrai moteur ; ses évènements ne vont nulle part (seul le bilan sert).
struct Engine;

impl Corrector for Engine {
    fn correct(
        &self,
        server: Server,
        model: String,
        text: String,
        cancel: Cancel,
    ) -> BoxFuture<Outcome> {
        Box::pin(async move {
            let ignore = |_: ProofreadEvent| {};
            super::run(
                &server,
                &model,
                ProofreadMode::Correct,
                &text,
                &cancel,
                &ignore,
            )
            .await
        })
    }
}

/// Presse-papiers, notifications et moteur (faux dans les tests).
pub(crate) struct Services {
    pub clipboard: Arc<dyn Clipboard>,
    pub notifier: Arc<dyn Notifier>,
    pub corrector: Arc<dyn Corrector>,
}

/// Correction de la sélection (état géré par Tauri).
pub struct Selection {
    services: Services,
    /// Notificateur du bureau, à configurer au démarrage (identité, clic sur
    /// « Show ») ; absent dans les tests.
    desktop: Option<Arc<DesktopNotifier>>,
    /// Presse-papiers du système, à qui donner au démarrage le fichier de
    /// l'autorisation de coller ; absent dans les tests.
    system_clipboard: Option<Arc<SystemClipboard>>,
    /// Numéro de la dernière correction.
    jobs: AtomicU64,
    /// Arrêt de la correction en cours (`None` : aucune). Changé sous ce
    /// verrou en même temps que `Proofreader::selection_running` : un appui
    /// pendant une correction trouve toujours de quoi l'arrêter.
    current: Mutex<Option<Cancel>>,
}

impl Default for Selection {
    fn default() -> Self {
        Self::new()
    }
}

impl Selection {
    pub fn new() -> Self {
        let notifier = Arc::new(DesktopNotifier::new(Action::Show));
        let clipboard = Arc::new(SystemClipboard::default());
        let services = Services {
            clipboard: clipboard.clone(),
            notifier: notifier.clone(),
            corrector: Arc::new(Engine),
        };
        Self {
            desktop: Some(notifier),
            system_clipboard: Some(clipboard),
            ..Self::with_services(services)
        }
    }

    pub(crate) fn with_services(services: Services) -> Self {
        Self {
            services,
            desktop: None,
            system_clipboard: None,
            jobs: AtomicU64::new(0),
            current: Mutex::new(None),
        }
    }

    /// Arrête la correction en cours, s'il y en a une (Ollama va s'arrêter) ;
    /// la notification le dit, comme pour un second appui.
    pub fn cancel(&self) {
        if let Some(cancel) = self.current.lock().as_ref() {
            cancel.cancel();
        }
    }

    /// Appui : réserve une nouvelle correction (son arrêt est renvoyé), ou
    /// arrête celle en cours (`None`).
    fn press(&self, proofreader: &Proofreader) -> Option<Cancel> {
        let mut current = self.current.lock();
        if proofreader.selection_running.swap(true, Ordering::SeqCst) {
            if let Some(cancel) = current.as_ref() {
                cancel.cancel();
            }
            return None;
        }
        let cancel = Cancel::default();
        *current = Some(cancel.clone());
        Some(cancel)
    }

    /// Corrige la sélection (réservée par [`Self::press`], arrêtée par
    /// `cancel`) puis libère la place, même si la tâche est abandonnée.
    pub(crate) async fn correct(&self, proofreader: &Proofreader, host: &dyn Host, cancel: Cancel) {
        let _running = Running {
            selection: self,
            proofreader,
        };
        let job = self.jobs.fetch_add(1, Ordering::Relaxed) + 1;
        let notice = self.work(proofreader, host, job, &cancel).await;
        self.services.notifier.show(notice);
    }

    /// Lecture, contrôles, correction, copie ; notification finale.
    async fn work(
        &self,
        proofreader: &Proofreader,
        host: &dyn Host,
        job: u64,
        cancel: &Cancel,
    ) -> Notice {
        let read = self.services.clipboard.selection();
        let text = tokio::select! {
            biased;
            () = cancel.cancelled() => return stopped(),
            read = read => match read {
                Ok(text) => text,
                Err(error) => {
                    return failure(t!("proofread.selection.unreadable"), &unreadable(error));
                }
            },
        };
        if text.trim().is_empty() {
            return hint(
                t!("proofread.selection.empty"),
                &t!("proofread.selection.emptyHint"),
            );
        }
        if text.chars().count() > MAX_CHARS {
            return hint(
                t!("proofread.selection.tooLong"),
                &t!("proofread.selection.tooLongHint"),
            );
        }
        let prepared = match host.prepare().await {
            Ok(prepared) => prepared,
            Err(reason) => return failure(t!("proofread.selection.failed"), &reason),
        };
        let Some(model) = prepared.model else {
            return hint(
                t!("proofread.selection.noModel"),
                &t!("proofread.selection.noModelHint"),
            );
        };
        // Modèle repris du chat : un modèle cloud ou distant n'est utilisé
        // que s'il est choisi sur la page Proofread.
        if prepared.from_chat {
            let elsewhere = tokio::select! {
                biased;
                () = cancel.cancelled() => return stopped(),
                elsewhere = runs_elsewhere(&prepared.server.url, &model) => elsewhere,
            };
            if elsewhere {
                return hint(
                    t!("proofread.selection.cloud"),
                    &t!("proofread.selection.cloudHint", model = model),
                );
            }
        }
        if cancel.is_cancelled() {
            return stopped();
        }
        let paste = prepared.paste;
        let server = prepared.server;

        let mut outcome = self
            .run_with(server.clone(), model.clone(), &text, cancel)
            .await;
        // Arrêt demandé : rien de copié, même si le moteur a fini entre-temps.
        if outcome.stopped || cancel.is_cancelled() {
            return stopped();
        }
        // Modèle introuvable : refaite avec un modèle chargé, s'il y en a un.
        let mut replaced = None;
        if matches!(outcome.error, Some((ProofreadErrorCode::ModelNotFound, _))) {
            let found = tokio::select! {
                biased;
                () = cancel.cancelled() => return stopped(),
                found = loaded_instead(host, &server.url, &model) => found,
            };
            if let Some(other) = found {
                outcome = self.run_with(server, other, &text, cancel).await;
                if outcome.stopped || cancel.is_cancelled() {
                    return stopped();
                }
                replaced = Some(model);
            }
        }
        if let Some((_, message)) = &outcome.error {
            return failure(t!("proofread.selection.failed"), message);
        }
        if outcome.parts.is_empty() {
            // Texte non vide sans passage : le moteur n'a rien rendu ; ne
            // jamais vider le presse-papiers.
            return failure(t!("proofread.selection.failed"), "");
        }

        let corrected = outcome.text();
        let kept = outcome.results.iter().flatten().any(|result| result.kept);
        let changed = corrected != text;
        let summary = if changed {
            t!("proofread.selection.copied")
        } else if kept {
            t!("proofread.selection.unchanged")
        } else {
            t!("proofread.selection.noMistakes")
        };
        let replaced = replaced.map(|missing| {
            t!(
                "proofread.selection.replaced",
                missing = missing,
                model = outcome.model
            )
        });
        *proofreader.last_selection.lock() = Some(SelectionResult {
            model: outcome.model,
            parts: outcome.parts,
            results: outcome.results,
        });
        let offer = Offer {
            session: job,
            text: corrected.clone(),
        };
        let mut body = notify::preview(&corrected);
        if kept {
            body.push_str("\n\n");
            body.push_str(&notify::escape(&t!("proofread.selection.kept")));
        }
        if let Some(note) = replaced {
            body.push_str("\n\n");
            body.push_str(&notify::escape(&note));
        }
        // Copie échouée : le résultat reste visible par « Show ».
        let notice = match self.services.clipboard.copy(corrected).await {
            // Texte changé : collé à la place de la sélection (Ctrl+V envoyé
            // à l'application au premier plan). Sinon, le presse-papiers et
            // la raison : Ctrl+V reste à faire.
            Ok(()) if paste && changed => match self.services.clipboard.paste().await {
                Ok(()) => Notice::lasting(t!("proofread.selection.pasted"), body),
                Err(error) => {
                    body.push_str("\n\n");
                    body.push_str(&notify::escape(&not_pasted(error)));
                    Notice::lasting(summary, body)
                }
            },
            Ok(()) => Notice::lasting(summary, body),
            Err(reason) => Notice::lasting(
                t!("notifications.notCopied"),
                notify::escape(&t!("dictation.copyFailed", reason = reason)),
            ),
        };
        notice.offering(offer)
    }
}

impl Selection {
    /// Notification de progression (affichée jusqu'au résultat : la
    /// correction d'un long texte dure), puis correction par `model`.
    async fn run_with(
        &self,
        server: Server,
        model: String,
        text: &str,
        cancel: &Cancel,
    ) -> Outcome {
        let progress = format!(
            "{}\n{}",
            t!("proofread.selection.model", model = model),
            t!("notifications.runAgain")
        );
        self.services.notifier.show(Notice::working(
            t!("proofread.selection.correcting"),
            &progress,
        ));
        self.services
            .corrector
            .correct(server, model, text.to_owned(), cancel.clone())
            .await
    }
}

/// Modèle chargé dans Ollama qui peut remplacer `missing`, introuvable : le
/// premier de la liste qui n'est pas `missing` et qui corrige sur la machine
/// (voir [`corrects_here`]) ; nom sans `:latest`, comme dans les réglages.
async fn loaded_instead(host: &dyn Host, base: &str, missing: &str) -> Option<String> {
    for name in host.loaded(base.to_owned()).await {
        let name = models::display_name(&name);
        if !running::same_model(&name, missing) && corrects_here(base, &name).await {
            return Some(name);
        }
    }
    None
}

/// Modèle qui tourne sur la machine (jamais un modèle cloud ou distant : la
/// sélection ne la quitte pas sans choix explicite) et produit du texte
/// (pas un modèle d'embedding). Détails illisibles : la requête dira ce qui
/// ne va pas.
async fn corrects_here(base: &str, model: &str) -> bool {
    if models::is_cloud_name(model) {
        return false;
    }
    match models::show(base, model).await {
        Ok(details) => {
            !details.remote
                && (details.capabilities.is_empty()
                    || details.capabilities.iter().any(|c| c == "completion"))
        }
        Err(_) => true,
    }
}

/// Modèle qui tourne hors de la machine : nom de modèle cloud, ou
/// `/api/show` qui le dit distant (comme le polish de la dictée ; détails
/// illisibles : la requête dira ce qui ne va pas).
async fn runs_elsewhere(base: &str, model: &str) -> bool {
    models::is_cloud_name(model)
        || models::show(base, model)
            .await
            .is_ok_and(|details| details.remote)
}

/// Pourquoi le texte corrigé n'a pas été collé, et quoi faire, dans la
/// langue de l'interface.
fn not_pasted(error: PasteError) -> String {
    match error {
        PasteError::Unsupported => t!("proofread.selection.pasteUnsupported"),
        PasteError::Refused => t!("proofread.selection.pasteRefused"),
        PasteError::Other(reason) => t!("proofread.selection.pasteFailed", reason = reason),
    }
}

/// Raison d'une sélection illisible, dans la langue de l'interface ; seul le
/// détail d'une bibliothèque reste tel quel.
fn unreadable(error: SelectionError) -> String {
    match error {
        SelectionError::Timeout => t!("proofread.selection.unreadableTimeout"),
        SelectionError::ThreadStopped => t!("proofread.selection.unreadableStopped"),
        SelectionError::Unsupported => t!("proofread.selection.unreadableUnsupported"),
        SelectionError::Other(detail) => detail,
    }
}

/// Dernière notification d'un échec : `detail` (message du moteur, raison
/// technique) en corps.
fn failure(summary: String, detail: &str) -> Notice {
    Notice::lasting(summary, notify::escape(detail))
}

/// Dernière notification d'un appui sans correction possible, avec ce qu'il
/// faut faire.
fn hint(summary: String, advice: &str) -> Notice {
    Notice::lasting(summary, notify::escape(advice))
}

/// Dernière notification d'une correction arrêtée par un second appui.
fn stopped() -> Notice {
    hint(
        t!("proofread.selection.stopped"),
        &t!("proofread.selection.stoppedHint"),
    )
}

/// Correction en cours ; la place se libère à la fin (ou à l'abandon de la
/// tâche), sous le verrou de [`Selection::current`].
struct Running<'a> {
    selection: &'a Selection,
    proofreader: &'a Proofreader,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        let mut current = self.selection.current.lock();
        *current = None;
        self.proofreader
            .selection_running
            .store(false, Ordering::SeqCst);
    }
}

/// Réglages lus dans la base à chaque appui.
struct AppHost(AppHandle);

impl Host for AppHost {
    fn prepare(&self) -> BoxFuture<Result<Prepared, String>> {
        let store = self
            .0
            .try_state::<Store>()
            .map(|store| store.inner().clone());
        Box::pin(async move {
            let store = store.ok_or_else(|| t!("dictation.noDatabase"))?;
            let settings = crate::commands::models::app_settings(&store)
                .await
                .map_err(|error| error.to_string())?;
            Ok(Prepared {
                server: Server::from_settings(&settings),
                model: model_of(&settings),
                from_chat: settings.proofread_model.is_empty(),
                paste: settings.proofread_paste,
            })
        })
    }

    fn loaded(&self, base: String) -> BoxFuture<Vec<String>> {
        Box::pin(async move {
            running::list(&base)
                .await
                .map(|models| models.into_iter().map(|model| model.name).collect())
                .unwrap_or_default()
        })
    }
}

/// Fichier, dans les données de l'app, de l'autorisation de coller donnée
/// par le bureau (jeton du portail RemoteDesktop, Linux).
const PASTE_TOKEN_FILE: &str = "paste.token";

/// Démarrage de l'app : identité des notifications (icône écrite par la
/// dictée) et clic sur « Show » (fenêtre montrée, page Proofread qui charge
/// la dernière correction ; nouvelle demande à chaque clic) ; fichier de
/// l'autorisation de coller.
pub fn setup(app: &AppHandle) {
    let Some(selection) = app.try_state::<Selection>() else {
        return;
    };
    if let (Some(clipboard), Ok(data)) = (&selection.system_clipboard, crate::paths::data_dir(app))
    {
        clipboard.set_paste_token_file(data.join(PASTE_TOKEN_FILE));
    }
    let Some(notifier) = &selection.desktop else {
        return;
    };
    notifier.configure(
        app.config().identifier.clone(),
        crate::paths::notification_icon(app).ok(),
    );
    let handle = app.clone();
    notifier.on_action(move |_| {
        crate::desktop::open_route(&handle, crate::desktop::PROOFREAD_SELECTION_ROUTE);
    });
}

/// Corrige la sélection courante (appui du raccourci, `chatonlm --proofread`).
/// Un appui pendant une correction en cours l'arrête.
pub fn correct_selection(app: &AppHandle) {
    let (Some(proofreader), Some(selection)) =
        (app.try_state::<Proofreader>(), app.try_state::<Selection>())
    else {
        return;
    };
    let Some(cancel) = selection.press(&proofreader) else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let selection = app.state::<Selection>();
        let proofreader = app.state::<Proofreader>();
        let host = AppHost(app.clone());
        selection.correct(&proofreader, &host, cancel).await;
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use fake_ollama::{FakeOllama, Mode};
    use tokio::sync::Notify;

    use super::*;
    use crate::proofread::{PartResult, ProofreadErrorCode, ProofreadPart};

    /// Faux presse-papiers : sélection donnée (ou jamais rendue), copies
    /// notées.
    struct FakeClipboard {
        selection: Mutex<Result<String, SelectionError>>,
        /// La lecture de la sélection ne finit jamais (application figée).
        hangs: AtomicBool,
        copies: Mutex<Vec<String>>,
        copy_error: Mutex<Option<String>>,
        /// Ce que le presse-papiers contenait à chaque collage demandé.
        pastes: Mutex<Vec<Option<String>>>,
        paste_error: Mutex<Option<PasteError>>,
    }

    impl FakeClipboard {
        fn new(selected: &str) -> Self {
            Self {
                selection: Mutex::new(Ok(selected.to_owned())),
                hangs: AtomicBool::new(false),
                copies: Mutex::new(Vec::new()),
                copy_error: Mutex::new(None),
                pastes: Mutex::new(Vec::new()),
                paste_error: Mutex::new(None),
            }
        }
    }

    impl Clipboard for FakeClipboard {
        fn copy(&self, text: String) -> BoxFuture<Result<(), String>> {
            let result = match self.copy_error.lock().clone() {
                Some(error) => Err(error),
                None => {
                    self.copies.lock().push(text);
                    Ok(())
                }
            };
            Box::pin(async move { result })
        }

        fn selection(&self) -> BoxFuture<Result<String, SelectionError>> {
            let selection = self.selection.lock().clone();
            let hangs = self.hangs.load(Ordering::SeqCst);
            Box::pin(async move {
                if hangs {
                    std::future::pending::<()>().await;
                }
                selection
            })
        }

        fn paste(&self) -> BoxFuture<Result<(), PasteError>> {
            self.pastes.lock().push(self.copies.lock().last().cloned());
            let result = match self.paste_error.lock().clone() {
                Some(error) => Err(error),
                None => Ok(()),
            };
            Box::pin(async move { result })
        }
    }

    #[derive(Default)]
    struct FakeNotifier {
        shown: Mutex<Vec<Notice>>,
    }

    impl Notifier for FakeNotifier {
        fn show(&self, notice: Notice) {
            self.shown.lock().push(notice);
        }

        fn withdraw(&self) {}
    }

    /// Réglages : modèle (choisi pour la correction, ou repris du chat) et
    /// adresse d'Ollama (rien n'écoute par défaut).
    struct FakeHost {
        model: Option<&'static str>,
        from_chat: bool,
        url: Option<String>,
        /// Réglage « coller à la place de la sélection ».
        paste: bool,
        /// Modèles chargés dans Ollama (`/api/ps`).
        loaded: &'static [&'static str],
    }

    impl FakeHost {
        /// Modèle repris du chat (rien de choisi sur la page Proofread).
        fn chat(model: &'static str, url: Option<String>) -> Self {
            Self {
                model: Some(model),
                from_chat: true,
                url,
                paste: false,
                loaded: &[],
            }
        }
    }

    impl Host for FakeHost {
        fn prepare(&self) -> BoxFuture<Result<Prepared, String>> {
            let model = self.model.map(str::to_owned);
            let from_chat = self.from_chat;
            let paste = self.paste;
            let url = self
                .url
                .clone()
                .unwrap_or_else(|| "http://127.0.0.1:9".into());
            Box::pin(async move {
                Ok(Prepared {
                    server: Server {
                        url,
                        context_length: 0,
                        keep_alive: 0,
                    },
                    model,
                    from_chat,
                    paste,
                })
            })
        }

        fn loaded(&self, _base: String) -> BoxFuture<Vec<String>> {
            let loaded = self.loaded.iter().map(|&name| name.to_owned()).collect();
            Box::pin(async move { loaded })
        }
    }

    struct BrokenHost;

    impl Host for BrokenHost {
        fn prepare(&self) -> BoxFuture<Result<Prepared, String>> {
            Box::pin(async { Err("The database could not be opened".to_owned()) })
        }

        fn loaded(&self, _base: String) -> BoxFuture<Vec<String>> {
            Box::pin(async { Vec::new() })
        }
    }

    /// Faux moteur : un passage par paragraphe, « teh » → « the » ; un
    /// passage « KEEP » est gardé tel quel (garde-fou) ; erreur au choix ;
    /// peut attendre un signal avant de répondre, et s'arrêter pendant
    /// l'attente (`stoppable`) ou finir quand même ; modèle `missing`
    /// introuvable.
    #[derive(Default)]
    struct FakeCorrector {
        calls: Mutex<Vec<(String, String)>>,
        error: Mutex<Option<(ProofreadErrorCode, String)>>,
        missing: Option<&'static str>,
        nothing: AtomicBool,
        gate: Option<Arc<Notify>>,
        stoppable: bool,
    }

    impl Corrector for FakeCorrector {
        fn correct(
            &self,
            _server: Server,
            model: String,
            text: String,
            cancel: Cancel,
        ) -> BoxFuture<Outcome> {
            self.calls.lock().push((model.clone(), text.clone()));
            let error = if self.missing == Some(model.as_str()) {
                Some((
                    ProofreadErrorCode::ModelNotFound,
                    format!("The model {model} is not installed"),
                ))
            } else {
                self.error.lock().clone()
            };
            let nothing = self.nothing.load(Ordering::SeqCst);
            let gate = self.gate.clone();
            let stoppable = self.stoppable;
            Box::pin(async move {
                if let Some(gate) = gate {
                    tokio::select! {
                        () = gate.notified() => {}
                        () = cancel.cancelled(), if stoppable => {
                            return Outcome {
                                model,
                                parts: Vec::new(),
                                results: Vec::new(),
                                error: None,
                                stopped: true,
                            };
                        }
                    }
                }
                let paragraphs: Vec<&str> = if nothing {
                    Vec::new()
                } else {
                    text.split("\n\n").collect()
                };
                let count = paragraphs.len();
                let parts: Vec<ProofreadPart> = paragraphs
                    .iter()
                    .enumerate()
                    .map(|(index, paragraph)| ProofreadPart {
                        before: String::new(),
                        text: (*paragraph).to_owned(),
                        after: if index + 1 < count {
                            "\n\n".into()
                        } else {
                            String::new()
                        },
                    })
                    .collect();
                let results = parts
                    .iter()
                    .map(|part| {
                        let kept = part.text.contains("KEEP");
                        Some(PartResult {
                            text: if kept {
                                part.text.clone()
                            } else {
                                part.text.replace("teh", "the")
                            },
                            kept,
                        })
                    })
                    .collect();
                Outcome {
                    model,
                    parts,
                    results: if error.is_some() { Vec::new() } else { results },
                    error,
                    stopped: false,
                }
            })
        }
    }

    struct Rig {
        clipboard: Arc<FakeClipboard>,
        notifier: Arc<FakeNotifier>,
        corrector: Arc<FakeCorrector>,
        selection: Selection,
        proofreader: Proofreader,
    }

    impl Rig {
        fn new(selected: &str) -> Self {
            Self::with_corrector(selected, FakeCorrector::default())
        }

        fn with_corrector(selected: &str, corrector: FakeCorrector) -> Self {
            let clipboard = Arc::new(FakeClipboard::new(selected));
            let notifier = Arc::new(FakeNotifier::default());
            let corrector = Arc::new(corrector);
            let selection = Selection::with_services(Services {
                clipboard: clipboard.clone(),
                notifier: notifier.clone(),
                corrector: corrector.clone(),
            });
            Self {
                clipboard,
                notifier,
                corrector,
                selection,
                proofreader: Proofreader::default(),
            }
        }

        async fn press(&self, host: &dyn Host) {
            let cancel = self
                .selection
                .press(&self.proofreader)
                .expect("press accepted");
            self.selection
                .correct(&self.proofreader, host, cancel)
                .await;
        }

        /// La correction lancée par `press` est finie : un nouvel appui en
        /// lance une autre (réservation rendue aussitôt).
        fn idle(&self) -> bool {
            let accepted = self.selection.press(&self.proofreader).is_some();
            if accepted {
                *self.selection.current.lock() = None;
                self.proofreader
                    .selection_running
                    .store(false, Ordering::SeqCst);
            }
            accepted
        }

        fn notices(&self) -> Vec<Notice> {
            self.notifier.shown.lock().clone()
        }

        fn last(&self) -> Notice {
            self.notices().last().cloned().expect("a notification")
        }

        fn copies(&self) -> Vec<String> {
            self.clipboard.copies.lock().clone()
        }
    }

    /// Réglage « coller » coupé : le presse-papiers seul.
    const MODEL: FakeHost = FakeHost {
        model: Some("gemma3:4b"),
        from_chat: false,
        url: None,
        paste: false,
        loaded: &[],
    };

    /// Réglage par défaut : le texte corrigé est collé.
    const PASTING: FakeHost = FakeHost {
        paste: true,
        ..MODEL
    };

    #[tokio::test]
    async fn the_corrected_selection_is_copied_and_shown() {
        let rig = Rig::new("I recieve teh letter.\n\nSee <you> & teh team.");
        rig.press(&MODEL).await;
        assert_eq!(
            *rig.corrector.calls.lock(),
            [(
                "gemma3:4b".to_owned(),
                "I recieve teh letter.\n\nSee <you> & teh team.".to_owned()
            )]
        );
        let corrected = "I recieve the letter.\n\nSee <you> & the team.";
        assert_eq!(rig.copies(), [corrected]);
        let notices = rig.notices();
        assert_eq!(notices.len(), 2);
        assert_eq!(
            notices[0],
            Notice::working(
                "Correcting the selected text…".into(),
                "Model: gemma3:4b\nRun the shortcut again to stop"
            )
        );
        // Affichée tant que la correction dure, remplacée par le résultat.
        assert!(notices[0].resident && !notices[0].last);
        assert_eq!(notices[1].summary, "Corrected text copied");
        assert!(notices[1].last);
        // Extrait échappé (balisage des serveurs de notifications).
        assert_eq!(
            notices[1].body,
            "I recieve the letter.\n\nSee &lt;you&gt; &amp; the team."
        );
        // Action « Show » : la page charge la dernière correction.
        assert_eq!(
            notices[1].copy,
            Some(Offer {
                session: 1,
                text: corrected.into()
            })
        );
        let last = rig.proofreader.last_selection().expect("result kept");
        assert_eq!(last.model, "gemma3:4b");
        assert_eq!(last.parts.len(), 2);
        assert_eq!(
            super::super::assemble(&last.parts, &last.results),
            corrected
        );
        // Libéré : un nouvel appui est accepté.
        assert!(rig.idle());
    }

    #[tokio::test]
    async fn a_correct_text_is_copied_anyway() {
        // Ctrl+V ne doit jamais coller un vieux contenu du presse-papiers.
        let rig = Rig::new("Nothing to fix here.");
        rig.press(&MODEL).await;
        assert_eq!(rig.copies(), ["Nothing to fix here."]);
        let last = rig.last();
        assert_eq!(last.summary, "No mistakes found");
        assert_eq!(last.body, "Nothing to fix here.");
        assert!(last.copy.is_some());
    }

    #[tokio::test]
    async fn passages_kept_by_the_guard_are_mentioned() {
        let note =
            "Some parts were left unchanged: the model's answer did not look like a correction.";
        let rig = Rig::new("teh first\n\nKEEP teh second");
        rig.press(&MODEL).await;
        assert_eq!(rig.copies(), ["the first\n\nKEEP teh second"]);
        let last = rig.last();
        assert_eq!(last.summary, "Corrected text copied");
        assert_eq!(last.body, format!("the first\n\nKEEP teh second\n\n{note}"));

        // Rien de changé parce que tout a été gardé : pas « No mistakes
        // found ».
        let rig = Rig::new("KEEP teh text");
        rig.press(&MODEL).await;
        assert_eq!(rig.copies(), ["KEEP teh text"]);
        let last = rig.last();
        assert_eq!(last.summary, "Text copied unchanged");
        assert!(last.body.ends_with(note), "{}", last.body);
    }

    #[tokio::test]
    async fn an_empty_selection_copies_nothing() {
        for selected in ["", " \n\t "] {
            let rig = Rig::new(selected);
            rig.press(&MODEL).await;
            assert!(rig.copies().is_empty());
            assert!(rig.corrector.calls.lock().is_empty());
            assert_eq!(
                rig.notices(),
                [Notice::lasting(
                    "No text selected".into(),
                    "Select some text in any app, then use the shortcut again.".into()
                )]
            );
            assert_eq!(rig.proofreader.last_selection(), None);
            assert!(rig.idle());
        }
    }

    #[tokio::test]
    async fn a_selection_beyond_the_limit_is_refused() {
        // Limite en caractères, pas en octets.
        let rig = Rig::new(&"é".repeat(MAX_CHARS));
        rig.press(&MODEL).await;
        assert_eq!(rig.corrector.calls.lock().len(), 1);

        let rig = Rig::new(&"a".repeat(MAX_CHARS + 1));
        rig.press(&MODEL).await;
        assert!(rig.corrector.calls.lock().is_empty());
        assert!(rig.copies().is_empty());
        let last = rig.last();
        assert_eq!(last.summary, "The selected text is too long");
        assert_eq!(last.body, "Select at most 100,000 characters.");
        assert_eq!(rig.notices().len(), 1);
    }

    #[test]
    fn the_limit_in_the_messages_is_the_real_one() {
        use crate::i18n::Lang;
        assert_eq!(MAX_CHARS, 100_000);
        assert_eq!(
            t!("proofread.selection.tooLongHint" in Lang::Fr),
            "Sélectionnez au plus 100\u{a0}000 caractères."
        );
        assert_eq!(
            t!("proofread.selection.tooLongHint" in Lang::Es),
            "Selecciona como máximo 100.000 caracteres."
        );
    }

    #[tokio::test]
    async fn without_a_model_nothing_is_sent() {
        let rig = Rig::new("teh text");
        rig.press(&FakeHost {
            model: None,
            ..MODEL
        })
        .await;
        assert!(rig.corrector.calls.lock().is_empty());
        assert!(rig.copies().is_empty());
        assert_eq!(
            rig.notices(),
            [Notice::lasting(
                "No model selected".into(),
                "Choose a model on the Proofread page of ChatonLM.".into()
            )]
        );
    }

    #[tokio::test]
    async fn an_engine_error_copies_nothing() {
        let corrector = FakeCorrector::default();
        *corrector.error.lock() = Some((
            ProofreadErrorCode::ModelNotFound,
            "Model \"gemma3:4b\" not found <404>".into(),
        ));
        let rig = Rig::with_corrector("teh text", corrector);
        rig.press(&MODEL).await;
        assert!(rig.copies().is_empty());
        let notices = rig.notices();
        assert_eq!(notices[0].summary, "Correcting the selected text…");
        assert_eq!(
            notices[1],
            Notice::lasting(
                "Proofreading failed".into(),
                "Model \"gemma3:4b\" not found &lt;404&gt;".into()
            )
        );
        assert_eq!(rig.proofreader.last_selection(), None);
        assert!(rig.idle());

        // Moteur qui ne rend aucun passage : presse-papiers intact.
        let corrector = FakeCorrector::default();
        corrector.nothing.store(true, Ordering::SeqCst);
        let rig = Rig::with_corrector("teh text", corrector);
        rig.press(&MODEL).await;
        assert!(rig.copies().is_empty());
        assert_eq!(rig.last().summary, "Proofreading failed");
    }

    #[tokio::test]
    async fn unreadable_selection_and_settings_are_reported() {
        // Raison traduite ; seul le détail d'une bibliothèque reste tel quel.
        for (error, reason) in [
            (
                SelectionError::Timeout,
                "The app that holds the selected text did not answer.",
            ),
            (
                SelectionError::ThreadStopped,
                "Reading the selected text stopped unexpectedly.",
            ),
            (
                SelectionError::Unsupported,
                "This desktop does not let ChatonLM read the selected text.",
            ),
            (
                SelectionError::Other("Unknown error while <reading>".into()),
                "Unknown error while &lt;reading&gt;",
            ),
        ] {
            let rig = Rig::new("");
            *rig.clipboard.selection.lock() = Err(error);
            rig.press(&MODEL).await;
            assert_eq!(
                rig.notices(),
                [Notice::lasting(
                    "Cannot read the selected text".into(),
                    reason.into()
                )]
            );
        }
        {
            use crate::i18n::Lang;
            assert_eq!(
                t!("proofread.selection.unreadableTimeout" in Lang::Fr),
                "L'application qui détient le texte sélectionné n'a pas répondu."
            );
            assert_eq!(
                t!("proofread.selection.unreadableUnsupported" in Lang::Es),
                "Este escritorio no permite que ChatonLM lea el texto seleccionado."
            );
        }

        let rig = Rig::new("teh text");
        rig.press(&BrokenHost).await;
        assert!(rig.corrector.calls.lock().is_empty());
        assert_eq!(
            rig.notices(),
            [Notice::lasting(
                "Proofreading failed".into(),
                "The database could not be opened".into()
            )]
        );
    }

    #[tokio::test]
    async fn a_failed_copy_is_reported_but_the_result_can_be_shown() {
        let rig = Rig::new("teh text");
        *rig.clipboard.copy_error.lock() = Some("no data-control".into());
        rig.press(&MODEL).await;
        let last = rig.last();
        assert_eq!(last.summary, "Could not copy to the clipboard");
        assert_eq!(
            last.body,
            "Could not copy the text to the clipboard: no data-control"
        );
        assert!(last.copy.is_some());
        assert!(rig.proofreader.last_selection().is_some());
    }

    /// Correction lancée en tâche de fond (premier appui) ; attend sa
    /// progression.
    async fn start_slow(rig: &Arc<Rig>) -> tokio::task::JoinHandle<()> {
        let cancel = rig.selection.press(&rig.proofreader).expect("accepted");
        let running = rig.clone();
        let task = tokio::spawn(async move {
            running
                .selection
                .correct(&running.proofreader, &MODEL, cancel)
                .await;
        });
        for _ in 0..200 {
            if !rig.notices().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(rig.notices().len(), 1, "progress shown");
        task
    }

    /// La correction se termine (arrêtée) dans un délai raisonnable.
    async fn finished(task: tokio::task::JoinHandle<()>) {
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("correction stopped")
            .unwrap();
    }

    fn stopped_notice() -> Notice {
        Notice::lasting("Proofreading stopped".into(), "Nothing was copied.".into())
    }

    #[tokio::test]
    async fn a_second_press_stops_the_correction() {
        let gate = Arc::new(Notify::new());
        let corrector = FakeCorrector {
            gate: Some(gate.clone()),
            stoppable: true,
            ..FakeCorrector::default()
        };
        let rig = Arc::new(Rig::with_corrector("teh text", corrector));
        let task = start_slow(&rig).await;
        // Second appui : pas de nouvelle correction, celle en cours s'arrête.
        assert!(rig.selection.press(&rig.proofreader).is_none());
        finished(task).await;
        assert_eq!(rig.corrector.calls.lock().len(), 1);
        assert!(rig.copies().is_empty());
        assert_eq!(rig.last(), stopped_notice());
        assert_eq!(rig.notices().len(), 2);
        assert_eq!(rig.proofreader.last_selection(), None);
        // Fini : un nouvel appui lance une nouvelle correction, numéro
        // suivant (permis donné d'avance au faux moteur).
        gate.notify_one();
        rig.press(&MODEL).await;
        assert_eq!(rig.copies(), ["the text"]);
        assert_eq!(rig.last().copy.map(|offer| offer.session), Some(2));
    }

    #[tokio::test]
    async fn nothing_is_copied_after_a_stop_even_if_the_engine_finished() {
        // Moteur qui finit son passage malgré l'arrêt demandé.
        let gate = Arc::new(Notify::new());
        let corrector = FakeCorrector {
            gate: Some(gate.clone()),
            ..FakeCorrector::default()
        };
        let rig = Arc::new(Rig::with_corrector("teh text", corrector));
        let task = start_slow(&rig).await;
        assert!(rig.selection.press(&rig.proofreader).is_none());
        gate.notify_one();
        task.await.unwrap();
        assert!(rig.copies().is_empty());
        assert_eq!(rig.last(), stopped_notice());
        assert_eq!(rig.proofreader.last_selection(), None);
    }

    #[tokio::test]
    async fn a_selection_that_never_comes_can_be_stopped() {
        let rig = Arc::new(Rig::new("teh text"));
        rig.clipboard.hangs.store(true, Ordering::SeqCst);
        let cancel = rig.selection.press(&rig.proofreader).expect("accepted");
        let running = rig.clone();
        let task = tokio::spawn(async move {
            running
                .selection
                .correct(&running.proofreader, &MODEL, cancel)
                .await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(rig.selection.press(&rig.proofreader).is_none());
        finished(task).await;
        assert!(rig.corrector.calls.lock().is_empty());
        assert_eq!(rig.notices(), [stopped_notice()]);
        assert!(rig.idle());
    }

    #[test]
    fn the_stop_texts_follow_the_language() {
        use crate::i18n::Lang;
        assert_eq!(
            t!("proofread.selection.stopped" in Lang::Fr),
            "Correction arrêtée"
        );
        assert_eq!(
            t!("proofread.selection.stoppedHint" in Lang::Es),
            "No se copió nada."
        );
    }

    #[tokio::test]
    async fn a_cloud_or_remote_chat_model_is_never_sent_the_selection() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let url = Some(fake.url());
        // Repris du chat : modèle cloud (sans requête), ou distant d'après
        // `/api/show`.
        for model in ["gpt-oss:20b-cloud", "chatonlm-polish-remote"] {
            let rig = Rig::new("teh text");
            rig.press(&FakeHost::chat(model, url.clone())).await;
            assert!(rig.corrector.calls.lock().is_empty(), "{model}");
            assert!(rig.copies().is_empty());
            assert_eq!(
                rig.notices(),
                [Notice::lasting(
                    "Selected text not sent".into(),
                    format!(
                        "The chat model ({model}) runs outside this computer. To proofread \
                         with it, choose it on the Proofread page of ChatonLM."
                    )
                )],
                "{model}"
            );
            assert!(rig.idle());
        }
        // Modèle local repris du chat : corrigé.
        let rig = Rig::new("teh text");
        rig.press(&FakeHost::chat("chatonlm-proofread", url.clone()))
            .await;
        assert_eq!(rig.copies(), ["the text"]);
        // Modèle cloud ou distant choisi sur la page Proofread : utilisé.
        for model in ["gpt-oss:20b-cloud", "chatonlm-polish-remote"] {
            let rig = Rig::new("teh text");
            rig.press(&FakeHost {
                model: Some(model),
                from_chat: false,
                url: url.clone(),
                paste: false,
                loaded: &[],
            })
            .await;
            assert_eq!(rig.corrector.calls.lock().len(), 1, "{model}");
            assert_eq!(rig.copies(), ["the text"]);
        }
        {
            use crate::i18n::Lang;
            assert_eq!(
                t!("proofread.selection.cloudHint" in Lang::Fr, model = "m"),
                "Le modèle de la conversation (m) fonctionne hors de cet ordinateur. Pour \
                 corriger avec ce modèle, choisissez-le sur la page «\u{a0}Correction\u{a0}» de \
                 ChatonLM."
            );
        }
    }

    #[tokio::test]
    async fn an_abandoned_correction_frees_its_place() {
        let gate = Arc::new(Notify::new());
        let corrector = FakeCorrector {
            gate: Some(gate),
            ..FakeCorrector::default()
        };
        let rig = Arc::new(Rig::with_corrector("teh text", corrector));
        let task = start_slow(&rig).await;
        task.abort();
        let _ = task.await;
        assert!(rig.idle());
    }

    /// Réglage par défaut : le texte corrigé, une fois dans le
    /// presse-papiers, est collé à la place de la sélection.
    #[tokio::test]
    async fn the_corrected_selection_is_pasted_over_the_selection() {
        let rig = Rig::new("I recieve teh letter.");
        rig.press(&PASTING).await;
        let corrected = "I recieve the letter.";
        assert_eq!(rig.copies(), [corrected]);
        // Collé après la copie : c'est le texte corrigé qui est collé.
        assert_eq!(*rig.clipboard.pastes.lock(), [Some(corrected.to_owned())]);
        let last = rig.last();
        assert_eq!(last.summary, "Corrected text pasted");
        assert_eq!(last.body, corrected);
        assert!(last.last);
        // « Show » reste offert.
        assert_eq!(
            last.copy,
            Some(Offer {
                session: 1,
                text: corrected.into()
            })
        );

        // Réglage coupé : copié, jamais collé.
        let rig = Rig::new("I recieve teh letter.");
        rig.press(&MODEL).await;
        assert!(rig.clipboard.pastes.lock().is_empty());
        assert_eq!(rig.last().summary, "Corrected text copied");
    }

    /// Rien à remplacer : pas de collage (le texte reste sélectionné tel
    /// quel), le presse-papiers comme avant.
    #[tokio::test]
    async fn an_unchanged_selection_is_not_pasted() {
        for (selected, summary) in [
            ("All good here.", "No mistakes found"),
            ("KEEP teh text", "Text copied unchanged"),
        ] {
            let rig = Rig::new(selected);
            rig.press(&PASTING).await;
            assert_eq!(rig.copies(), [selected], "{selected}");
            assert!(rig.clipboard.pastes.lock().is_empty(), "{selected}");
            assert_eq!(rig.last().summary, summary, "{selected}");
        }
    }

    /// Collage impossible : le texte corrigé reste dans le presse-papiers et
    /// la notification dit pourquoi, et de coller par Ctrl+V.
    #[tokio::test]
    async fn a_failed_paste_says_to_paste_by_hand() {
        for (error, reason) in [
            (
                PasteError::Unsupported,
                "This desktop does not let ChatonLM paste for you: press Ctrl+V.",
            ),
            (
                PasteError::Refused,
                "Pasting was not allowed: press Ctrl+V.",
            ),
            (
                PasteError::Other("no <reply>".into()),
                "Pasting failed (no &lt;reply&gt;): press Ctrl+V.",
            ),
        ] {
            let rig = Rig::new("teh text");
            *rig.clipboard.paste_error.lock() = Some(error.clone());
            rig.press(&PASTING).await;
            assert_eq!(rig.copies(), ["the text"], "{error:?}");
            assert_eq!(rig.clipboard.pastes.lock().len(), 1, "{error:?}");
            let last = rig.last();
            assert_eq!(last.summary, "Corrected text copied", "{error:?}");
            assert_eq!(last.body, format!("the text\n\n{reason}"), "{error:?}");
        }
    }

    /// Copie échouée, correction en échec ou arrêtée : rien n'est collé (ce
    /// serait un vieux contenu du presse-papiers).
    #[tokio::test]
    async fn nothing_is_pasted_without_a_corrected_text_in_the_clipboard() {
        let rig = Rig::new("teh text");
        *rig.clipboard.copy_error.lock() = Some("clipboard busy".into());
        rig.press(&PASTING).await;
        assert!(rig.clipboard.pastes.lock().is_empty());
        assert_eq!(rig.last().summary, "Could not copy to the clipboard");

        let rig = Rig::new("teh text");
        *rig.corrector.error.lock() =
            Some((ProofreadErrorCode::ModelNotFound, "model crashed".into()));
        rig.press(&PASTING).await;
        assert!(rig.clipboard.pastes.lock().is_empty());
        assert!(rig.copies().is_empty());

        let gate = Arc::new(Notify::new());
        let corrector = FakeCorrector {
            gate: Some(gate),
            stoppable: true,
            ..FakeCorrector::default()
        };
        let rig = Arc::new(Rig::with_corrector("teh text", corrector));
        let task = {
            let rig = rig.clone();
            tokio::spawn(async move { rig.press(&PASTING).await })
        };
        while rig.corrector.calls.lock().is_empty() {
            tokio::task::yield_now().await;
        }
        rig.selection.cancel();
        task.await.unwrap();
        assert!(rig.clipboard.pastes.lock().is_empty());
        assert!(rig.copies().is_empty());
    }

    /// Modèle de la correction supprimé depuis qu'il a été choisi (`/api/ps`
    /// le nomme avec `:latest`).
    const MISSING: &str = "gone";

    fn missing_corrector() -> FakeCorrector {
        FakeCorrector {
            missing: Some(MISSING),
            ..FakeCorrector::default()
        }
    }

    #[tokio::test]
    async fn a_missing_model_is_replaced_by_a_loaded_one() {
        let rig = Rig::with_corrector("teh text", missing_corrector());
        rig.press(&FakeHost {
            model: Some(MISSING),
            loaded: &["gone:latest", "llama3.2:latest"],
            ..PASTING
        })
        .await;
        // Refaite avec le premier modèle chargé qui n'est pas l'absent, nommé
        // comme dans les réglages.
        assert_eq!(
            *rig.corrector.calls.lock(),
            [
                ("gone".to_owned(), "teh text".to_owned()),
                ("llama3.2".to_owned(), "teh text".to_owned())
            ]
        );
        assert_eq!(rig.copies(), ["the text"]);
        assert_eq!(rig.clipboard.pastes.lock().len(), 1);
        let notices = rig.notices();
        assert_eq!(notices.len(), 3);
        assert_eq!(
            notices[1],
            Notice::working(
                "Correcting the selected text…".into(),
                "Model: llama3.2\nRun the shortcut again to stop"
            )
        );
        assert_eq!(notices[2].summary, "Corrected text pasted");
        assert_eq!(
            notices[2].body,
            "the text\n\nThe model gone is no longer installed: corrected with llama3.2, \
             loaded in Ollama."
        );
        let last = rig.proofreader.last_selection().expect("result kept");
        assert_eq!(last.model, "llama3.2");
        assert!(rig.idle());
        {
            use crate::i18n::Lang;
            assert_eq!(
                t!("proofread.selection.replaced" in Lang::Fr, missing = "a", model = "b"),
                "Le modèle a n'est plus installé\u{a0}: correction faite avec b, chargé dans \
                 Ollama."
            );
        }
    }

    #[tokio::test]
    async fn the_replacement_runs_here_and_writes_text() {
        // Modèle cloud, distant d'après `/api/show`, ou d'embedding : jamais
        // pris ; le suivant l'est.
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let rig = Rig::with_corrector("teh text", missing_corrector());
        rig.press(&FakeHost {
            model: Some(MISSING),
            url: Some(fake.url()),
            loaded: &[
                "gpt-oss:20b-cloud",
                "chatonlm-polish-remote:latest",
                "chatonlm-embed:latest",
                "chatonlm-proofread:latest",
            ],
            ..MODEL
        })
        .await;
        let calls = rig.corrector.calls.lock().clone();
        assert_eq!(
            calls
                .iter()
                .map(|(model, _)| model.as_str())
                .collect::<Vec<_>>(),
            ["gone", "chatonlm-proofread"]
        );
        assert_eq!(rig.copies(), ["the text"]);

        // Aucun modèle chargé utilisable : l'erreur reste, rien de copié.
        let none: [&'static [&'static str]; 3] = [
            &[],
            &["gone:latest"],
            &[
                "gpt-oss:20b-cloud",
                "chatonlm-polish-remote:latest",
                "chatonlm-embed:latest",
            ],
        ];
        for loaded in none {
            let rig = Rig::with_corrector("teh text", missing_corrector());
            rig.press(&FakeHost {
                model: Some(MISSING),
                url: Some(fake.url()),
                loaded,
                ..PASTING
            })
            .await;
            assert_eq!(rig.corrector.calls.lock().len(), 1, "{loaded:?}");
            assert!(rig.copies().is_empty());
            assert!(rig.clipboard.pastes.lock().is_empty());
            assert_eq!(
                rig.last(),
                Notice::lasting(
                    "Proofreading failed".into(),
                    "The model gone is not installed".into()
                ),
                "{loaded:?}"
            );
        }
    }

    #[tokio::test]
    async fn only_a_missing_model_is_replaced() {
        let rig = Rig::new("teh text");
        *rig.corrector.error.lock() = Some((
            ProofreadErrorCode::Failed,
            "Proofreading failed: boom".into(),
        ));
        rig.press(&FakeHost {
            loaded: &["llama3.2:latest"],
            ..MODEL
        })
        .await;
        assert_eq!(rig.corrector.calls.lock().len(), 1);
        assert!(rig.copies().is_empty());
    }

    #[tokio::test]
    async fn long_results_are_cut_in_the_notification() {
        let selected = format!("{} teh end", "word ".repeat(100));
        let rig = Rig::new(&selected);
        rig.press(&MODEL).await;
        let last = rig.last();
        assert_eq!(last.body.chars().count(), 300);
        assert!(last.body.ends_with('…'));
        // Le texte entier est copié.
        assert!(rig.copies()[0].ends_with("the end"));
    }
}
