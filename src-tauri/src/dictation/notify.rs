//! Notifications de la dictée et de la correction de la sélection : une
//! seule par dictée (ou par correction), remplacée d'état en état
//! (« Listening… », « Transcribing… », puis le résultat). Sans serveur de
//! notifications, rien n'est montré (une ligne au journal).
//!
//! - Linux (`linux.rs`) : D-Bus `org.freedesktop.Notifications`, par
//!   notify-rust.
//! - Windows (`windows.rs`) : toasts WinRT ; leur XML est fait par `toast.rs`
//!   (logique pure, essayée partout).
//!
//! Chaque notificateur propose une action sur son résultat ([`Action`]) :
//! « Copy » pour la dictée sans copie automatique, « Show » pour la
//! correction de la sélection (page Proofread). Son clic est attendu hors du
//! runtime async, pour la dernière notification qui la propose seulement ;
//! celle d'une notification plus ancienne ne fait alors plus rien.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;
use tokio::sync::{mpsc, oneshot};

use super::clipboard::BoxFuture;
use crate::i18n::{self, Lang, t};

#[cfg(target_os = "linux")]
mod linux;
pub mod toast;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux::DbusBus as SystemBus;
#[cfg(windows)]
use windows::ToastBus as SystemBus;
#[cfg(windows)]
pub use windows::show_simple;

/// Titre d'une notification de la dictée.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Title {
    Listening,
    Transcribing,
    Copied,
    /// Copié sans le nettoyage (échoué ou écarté).
    CopiedRaw,
    /// Copié, puis collé là où se trouve le curseur.
    Pasted,
    PastedRaw,
    NotCopied,
    /// Copie automatique coupée : « Copy » proposé.
    Ready,
    ReadyRaw,
    Failed,
}

impl Title {
    /// Texte dans la langue `lang`.
    pub fn text_in(self, lang: Lang) -> String {
        match self {
            Title::Listening => t!("notifications.listening" in lang),
            Title::Transcribing => t!("notifications.transcribing" in lang),
            Title::Copied => t!("notifications.copied" in lang),
            Title::CopiedRaw => t!("notifications.copiedRaw" in lang),
            Title::Pasted => t!("notifications.pasted" in lang),
            Title::PastedRaw => t!("notifications.pastedRaw" in lang),
            Title::NotCopied => t!("notifications.notCopied" in lang),
            Title::Ready => t!("notifications.ready" in lang),
            Title::ReadyRaw => t!("notifications.readyRaw" in lang),
            Title::Failed => t!("notifications.failed" in lang),
        }
    }

    /// Texte dans la langue de l'interface (au moment de l'envoi).
    pub fn text(self) -> String {
        self.text_in(i18n::current())
    }
}

/// Corps du résultat quand le texte dicté n'est pas montré.
pub fn text_hidden() -> String {
    t!("notifications.textHidden")
}

/// Action de la notification de fin, sans copie automatique.
pub const COPY_ACTION: &str = "copy";

/// Action de la notification de fin d'une correction de la sélection.
pub const SHOW_ACTION: &str = "show";

/// Action proposée par la dernière notification d'un notificateur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Copier le texte du résultat (dictée).
    Copy,
    /// Montrer le résultat dans ChatonLM (correction de la sélection).
    Show,
}

impl Action {
    /// Clé envoyée au serveur de notifications (jamais traduite).
    pub fn key(self) -> &'static str {
        match self {
            Action::Copy => COPY_ACTION,
            Action::Show => SHOW_ACTION,
        }
    }

    /// Libellé du bouton, dans la langue `lang`.
    pub fn label_in(self, lang: Lang) -> String {
        match self {
            Action::Copy => t!("notifications.copy" in lang),
            Action::Show => t!("notifications.show" in lang),
        }
    }
}

/// Longueur maximale du texte montré (caractères), « … » ensuite.
const PREVIEW_CHARS: usize = 300;

/// Notification de la dictée (ou d'une correction de la sélection).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub summary: String,
    /// Corps, déjà échappé (balisage HTML des serveurs comme celui de KDE).
    pub body: String,
    /// Dernière de la dictée : gardée dans l'historique ; la dictée suivante
    /// en ouvrira une nouvelle. Les autres sont passagères (`transient`).
    pub last: bool,
    /// Action du notificateur proposée (dernière seulement) : « Copy » pour
    /// la dictée, « Show » pour la correction de la sélection.
    pub copy: Option<Offer>,
    /// Progression qui reste affichée jusqu'à son remplacement (ni
    /// passagère, ni effacée au bout du délai du serveur) : correction de la
    /// sélection, qui peut durer.
    pub resident: bool,
}

/// Résultat auquel s'applique l'action : texte copié au clic sur « Copy »
/// (dictée), correction montrée au clic sur « Show ».
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// Dictée (ou correction) qui l'a produit.
    pub session: u64,
    pub text: String,
}

impl Notice {
    /// Titre dans la langue de l'interface du moment.
    pub fn progress(title: Title, body: &str) -> Self {
        Self::passing(title.text(), body)
    }

    pub fn last(title: Title, body: String) -> Self {
        Self::lasting(title.text(), body)
    }

    /// Notification passagère de titre `summary` (déjà traduit) ; `body`
    /// échappé ici.
    pub fn passing(summary: String, body: &str) -> Self {
        Self {
            summary,
            body: escape(body),
            last: false,
            copy: None,
            resident: false,
        }
    }

    /// Progression de titre `summary` (déjà traduit) qui reste affichée
    /// jusqu'à ce que la suivante la remplace ; `body` échappé ici.
    pub fn working(summary: String, body: &str) -> Self {
        Self {
            resident: true,
            ..Self::passing(summary, body)
        }
    }

    /// Dernière notification, de titre `summary` (déjà traduit) ; `body`
    /// déjà échappé.
    pub fn lasting(summary: String, body: String) -> Self {
        Self {
            summary,
            body,
            last: true,
            copy: None,
            resident: false,
        }
    }

    /// Avec l'action du notificateur (« Copy » ou « Show »).
    pub fn offering(mut self, offer: Offer) -> Self {
        self.copy = Some(offer);
        self
    }
}

/// Affichage des notifications (le vrai serveur, ou un faux).
pub trait Notifier: Send + Sync {
    /// Montre `notice` à la place de la notification de la dictée en cours.
    fn show(&self, notice: Notice);
    /// Retire la notification de la dictée en cours (abandon).
    fn withdraw(&self);
}

/// `&`, `<` et `>` échappés : les serveurs lisent un sous-ensemble de HTML.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Texte dicté tel que montré : tronqué à 300 caractères (« … » ajouté),
/// échappé.
pub fn preview(text: &str) -> String {
    let mut shown: String = text.chars().take(PREVIEW_CHARS).collect();
    if text.chars().count() > PREVIEW_CHARS {
        shown = shown.trim_end().to_owned();
        shown.push('…');
    }
    escape(&shown)
}

enum Command {
    Show(Notice),
    Withdraw,
}

/// Identité de l'app dans ses notifications, fixée au démarrage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Identifiant de l'app (indice `desktop-entry`).
    pub desktop_entry: String,
    /// Icône de l'app écrite sur le disque.
    pub icon: Option<PathBuf>,
}

/// Notification telle qu'envoyée.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub summary: String,
    pub body: String,
    pub transient: bool,
    /// Identité lue au moment de l'envoi (`None` avant `configure`).
    pub identity: Option<Identity>,
    /// Notification remplacée (même identifiant).
    pub replaces: Option<u32>,
    /// Action proposée (« Copy » ou « Show »).
    pub action: Option<Action>,
    /// Affichée jusqu'à son remplacement (délai d'expiration nul).
    pub resident: bool,
}

/// Serveur de notifications (D-Bus, ou un faux dans les tests).
pub trait Bus: Send + Sync + 'static {
    /// Montre `message` ; identifiant attribué.
    fn show(&self, message: Message) -> BoxFuture<Result<u32, String>>;
    /// Ferme la notification `id`.
    fn close(&self, id: u32) -> BoxFuture<()>;
    /// Attend, sans bloquer l'appelant, le clic sur l'action `action` de la
    /// notification `id` (montrée avec elle) : `run` est appelé une fois au
    /// plus, jamais après `stop` (envoyé ou lâché).
    fn on_action(
        &self,
        id: u32,
        action: Action,
        stop: oneshot::Receiver<()>,
        run: Box<dyn FnOnce() + Send>,
    );
}

type ActionHandler = Arc<dyn Fn(Offer) + Send + Sync>;

/// Notifications de la dictée (ou de la correction de la sélection) ; une
/// tâche les envoie dans l'ordre.
pub struct DesktopNotifier {
    bus: Arc<dyn Bus>,
    /// Action proposée par la dernière notification.
    action: Action,
    /// Lue à chaque envoi : une notification partie avant `configure` ne
    /// fige pas l'identité des suivantes.
    identity: Arc<RwLock<Option<Identity>>>,
    /// Clic sur l'action (réglé au démarrage).
    handler: Arc<RwLock<Option<ActionHandler>>>,
    commands: OnceLock<mpsc::UnboundedSender<Command>>,
}

impl Default for DesktopNotifier {
    fn default() -> Self {
        Self::with_bus(Arc::new(SystemBus::default()))
    }
}

impl DesktopNotifier {
    /// Notificateur du bureau dont la dernière notification propose `action`.
    pub fn new(action: Action) -> Self {
        Self::with_bus_and_action(Arc::new(SystemBus::default()), action)
    }

    /// Notificateur de la dictée (« Copy ») sur `bus`.
    pub fn with_bus(bus: Arc<dyn Bus>) -> Self {
        Self::with_bus_and_action(bus, Action::Copy)
    }

    pub fn with_bus_and_action(bus: Arc<dyn Bus>, action: Action) -> Self {
        Self {
            bus,
            action,
            identity: Arc::new(RwLock::new(None)),
            handler: Arc::new(RwLock::new(None)),
            commands: OnceLock::new(),
        }
    }

    /// Appelé (hors du runtime async) au clic sur l'action, avec le
    /// résultat de cette notification.
    pub fn on_action(&self, handler: impl Fn(Offer) + Send + Sync + 'static) {
        *self.handler.write() = Some(Arc::new(handler));
    }

    /// Identifiant et icône de l'app (au démarrage).
    pub fn configure(&self, desktop_entry: String, icon: Option<PathBuf>) {
        // Windows : nom et icône de l'app dans ses toasts, même lancée sans
        // raccourci du menu Démarrer.
        #[cfg(windows)]
        windows::register_app(&desktop_entry, icon.as_deref());
        *self.identity.write() = Some(Identity {
            desktop_entry,
            icon,
        });
    }

    fn send(&self, command: Command) {
        let commands = self.commands.get_or_init(|| {
            let (sender, receiver) = mpsc::unbounded_channel();
            tauri::async_runtime::spawn(deliver(
                receiver,
                self.bus.clone(),
                self.action,
                self.identity.clone(),
                self.handler.clone(),
            ));
            sender
        });
        let _ = commands.send(command);
    }
}

impl Notifier for DesktopNotifier {
    fn show(&self, notice: Notice) {
        self.send(Command::Show(notice));
    }

    fn withdraw(&self) {
        self.send(Command::Withdraw);
    }
}

async fn deliver(
    mut commands: mpsc::UnboundedReceiver<Command>,
    bus: Arc<dyn Bus>,
    action: Action,
    identity: Arc<RwLock<Option<Identity>>>,
    handler: Arc<RwLock<Option<ActionHandler>>>,
) {
    // Notification de la dictée en cours.
    let mut current: Option<u32> = None;
    // Attente du clic sur l'action de la dernière notification qui la
    // propose.
    let mut waiting: Option<oneshot::Sender<()>> = None;
    let mut warned = false;
    while let Some(command) = commands.recv().await {
        match command {
            Command::Show(notice) => {
                let handler = handler.read().clone();
                let offer = notice.copy.filter(|_| notice.last && handler.is_some());
                let message = Message {
                    summary: notice.summary,
                    body: notice.body,
                    transient: !notice.last && !notice.resident,
                    identity: identity.read().clone(),
                    replaces: current,
                    action: offer.is_some().then_some(action),
                    resident: notice.resident,
                };
                match bus.show(message).await {
                    Ok(id) => {
                        current = (!notice.last).then_some(id);
                        if let (Some(offer), Some(handler)) = (offer, handler) {
                            // L'action de la notification d'avant ne fait
                            // plus rien.
                            if let Some(previous) = waiting.take() {
                                let _ = previous.send(());
                            }
                            let (stop, stopped) = oneshot::channel();
                            waiting = Some(stop);
                            bus.on_action(id, action, stopped, Box::new(move || handler(offer)));
                        }
                    }
                    Err(error) => {
                        current = None;
                        if !warned {
                            warned = true;
                            eprintln!("chatonlm: notification not shown: {error}");
                        }
                    }
                }
            }
            Command::Withdraw => {
                if let Some(id) = current.take() {
                    bus.close(id).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    #[test]
    fn markup_is_escaped() {
        assert_eq!(
            escape("a < b && c > <b>d</b>"),
            "a &lt; b &amp;&amp; c &gt; &lt;b&gt;d&lt;/b&gt;"
        );
        assert_eq!(
            Notice::progress(Title::Listening, "Press <Ctrl> again").body,
            "Press &lt;Ctrl&gt; again"
        );
    }

    #[test]
    fn titles_follow_the_language() {
        assert_eq!(Title::Listening.text_in(Lang::En), "Listening…");
        assert_eq!(
            Title::CopiedRaw.text_in(Lang::Es),
            "Copiado al portapapeles (sin pulir)"
        );
        assert_eq!(
            Title::NotCopied.text_in(Lang::Fr),
            "Impossible de copier dans le presse-papiers"
        );
        // Les essais tournent en anglais (langue par défaut).
        assert_eq!(
            Notice::last(Title::Failed, String::new()).summary,
            "Dictation failed"
        );
    }

    /// Faux serveur : messages reçus, notifications fermées.
    #[derive(Default)]
    pub(crate) struct FakeBus {
        pub(crate) shown: Mutex<Vec<Message>>,
        closed: Mutex<Vec<u32>>,
        /// Attentes d'un clic sur une action.
        waiting: Mutex<Vec<Waiting>>,
    }

    type Waiting = (
        u32,
        Action,
        oneshot::Receiver<()>,
        Option<Box<dyn FnOnce() + Send>>,
    );

    impl FakeBus {
        /// Clic sur l'action `action` de la notification `id` : vrai si
        /// l'action a servi.
        pub(crate) fn click(&self, id: u32, action: Action) -> bool {
            for (waited, offered, stop, run) in self.waiting.lock().iter_mut() {
                let stopped = !matches!(stop.try_recv(), Err(oneshot::error::TryRecvError::Empty));
                if *waited == id
                    && *offered == action
                    && !stopped
                    && let Some(run) = run.take()
                {
                    run();
                    return true;
                }
            }
            false
        }
    }

    impl Bus for FakeBus {
        fn show(&self, message: Message) -> BoxFuture<Result<u32, String>> {
            let mut shown = self.shown.lock();
            let id = message.replaces.unwrap_or(shown.len() as u32 + 1);
            shown.push(message);
            Box::pin(async move { Ok(id) })
        }

        fn close(&self, id: u32) -> BoxFuture<()> {
            self.closed.lock().push(id);
            Box::pin(async {})
        }

        fn on_action(
            &self,
            id: u32,
            action: Action,
            stop: oneshot::Receiver<()>,
            run: Box<dyn FnOnce() + Send>,
        ) {
            self.waiting.lock().push((id, action, stop, Some(run)));
        }
    }

    pub(crate) async fn settle(bus: &FakeBus, shown: usize, closed: usize) {
        for _ in 0..500 {
            if bus.shown.lock().len() >= shown && bus.closed.lock().len() >= closed {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("notifications not sent");
    }

    #[tokio::test]
    async fn one_notification_per_dictation_with_the_current_identity() {
        let bus = Arc::new(FakeBus::default());
        let notifier = DesktopNotifier::with_bus(bus.clone());
        // Avant `configure` (dictée lancée tôt) : pas encore d'identité...
        notifier.show(Notice::progress(Title::Listening, "Press X again to stop"));
        settle(&bus, 1, 0).await;
        // ... mais les suivantes la portent.
        notifier.configure("io.example.app".into(), Some(PathBuf::from("/icon.png")));
        notifier.show(Notice::progress(Title::Transcribing, ""));
        notifier.show(Notice::last(Title::Copied, "text".into()));
        // Dictée suivante : nouvelle notification, retirée à l'abandon.
        notifier.show(Notice::progress(Title::Listening, ""));
        notifier.withdraw();
        settle(&bus, 4, 1).await;
        let shown = bus.shown.lock().clone();
        let identity = Some(Identity {
            desktop_entry: "io.example.app".into(),
            icon: Some(PathBuf::from("/icon.png")),
        });
        assert_eq!(shown[0].identity, None);
        assert!(
            shown[1..]
                .iter()
                .all(|message| message.identity == identity)
        );
        assert_eq!(
            shown
                .iter()
                .map(|message| (message.replaces, message.transient))
                .collect::<Vec<_>>(),
            [
                (None, true),
                (Some(1), true),
                (Some(1), false),
                (None, true)
            ]
        );
        assert_eq!(*bus.closed.lock(), [4]);
    }

    #[tokio::test]
    async fn copy_is_offered_by_the_last_notification_only() {
        let bus = Arc::new(FakeBus::default());
        let notifier = DesktopNotifier::with_bus(bus.clone());
        let copied = Arc::new(Mutex::new(Vec::new()));
        let sink = copied.clone();
        notifier.on_action(move |offer| sink.lock().push(offer));
        let offer = |session: u64| Offer {
            session,
            text: format!("text {session}"),
        };
        notifier.show(Notice::progress(Title::Listening, ""));
        // Offre sur une notification intermédiaire : ignorée.
        notifier.show(Notice::progress(Title::Transcribing, "").offering(offer(0)));
        notifier.show(Notice::last(Title::Ready, "text 1".into()).offering(offer(1)));
        notifier.show(Notice::progress(Title::Listening, ""));
        notifier.show(Notice::last(Title::Ready, "text 2".into()).offering(offer(2)));
        notifier.show(Notice::last(Title::Copied, "text 3".into()));
        settle(&bus, 6, 0).await;
        let actions: Vec<bool> = bus
            .shown
            .lock()
            .iter()
            .map(|m| m.action == Some(Action::Copy))
            .collect();
        assert_eq!(actions, [false, false, true, false, true, false]);
        // La première attente est arrêtée par la seconde.
        assert!(!bus.click(1, Action::Copy));
        assert!(bus.click(4, Action::Copy));
        // Une seule fois.
        assert!(!bus.click(4, Action::Copy));
        assert_eq!(*copied.lock(), [offer(2)]);
    }

    #[tokio::test]
    async fn a_notifier_offers_its_own_action() {
        // Notificateur de la correction : « Show » à la place de « Copy ».
        let bus = Arc::new(FakeBus::default());
        let notifier = DesktopNotifier::with_bus_and_action(bus.clone(), Action::Show);
        let shown = Arc::new(Mutex::new(Vec::new()));
        let sink = shown.clone();
        notifier.on_action(move |offer| sink.lock().push(offer.session));
        notifier.show(Notice::working("Correcting…".into(), "<b>"));
        notifier.show(
            Notice::lasting("Corrected".into(), "text".into()).offering(Offer {
                session: 7,
                text: "text".into(),
            }),
        );
        settle(&bus, 2, 0).await;
        let messages = bus.shown.lock().clone();
        assert_eq!(messages[0].body, "&lt;b&gt;");
        // Progression qui reste affichée tant que la correction dure.
        assert!(messages[0].resident && !messages[0].transient);
        assert!(messages[0].action.is_none());
        assert_eq!(messages[1].action, Some(Action::Show));
        assert!(!messages[1].resident && !messages[1].transient);
        // Le résultat remplace la progression (même identifiant).
        assert_eq!(messages[1].replaces, Some(1));
        assert!(!bus.click(1, Action::Copy));
        assert!(bus.click(1, Action::Show));
        assert_eq!(*shown.lock(), [7]);
    }

    #[test]
    fn action_keys_stay_english_and_labels_follow_the_language() {
        assert_eq!(Action::Copy.key(), "copy");
        assert_eq!(Action::Show.key(), "show");
        assert_eq!(Action::Show.label_in(Lang::En), "Show");
        assert_eq!(Action::Show.label_in(Lang::Fr), "Afficher");
        assert_eq!(Action::Show.label_in(Lang::Es), "Mostrar");
        assert_eq!(Action::Copy.label_in(Lang::Fr), "Copier");
    }

    #[tokio::test]
    async fn without_a_handler_no_copy_is_offered() {
        let bus = Arc::new(FakeBus::default());
        let notifier = DesktopNotifier::with_bus(bus.clone());
        notifier.show(Notice::last(Title::Ready, String::new()).offering(Offer {
            session: 1,
            text: "x".into(),
        }));
        settle(&bus, 1, 0).await;
        assert!(bus.shown.lock()[0].action.is_none());
        assert!(bus.waiting.lock().is_empty());
    }

    #[test]
    fn long_texts_are_cut_at_300_characters() {
        let short = "é".repeat(300);
        assert_eq!(preview(&short), short);
        let long = format!("{} tail", "é".repeat(300));
        let shown = preview(&long);
        assert_eq!(shown.chars().count(), 301);
        assert!(shown.ends_with("é…"));
        // Coupé puis échappé : pas d'entité coupée en deux.
        let markup = "&".repeat(301);
        assert_eq!(preview(&markup), format!("{}…", "&amp;".repeat(300)));
    }
}
