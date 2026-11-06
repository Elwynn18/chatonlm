//! Raccourci global « Read selected text aloud » (propre à ChatonLM) : le
//! texte sélectionné dans n'importe quelle application est lu à voix haute
//! (sélection primaire sous Linux, copie simulée puis presse-papiers remis en
//! place sous Windows : `dictation::clipboard`, comme la correction de la
//! sélection). Un nouvel appui pendant la lecture l'arrête ; pendant la
//! lecture de la sélection (avant le premier son), il l'annule.
//!
//! La lecture passe par le service de lecture ([`super::ReadAloud`]), comme
//! celle du pont VS Code : une lecture à la fois (celle d'une réponse
//! s'arrête), langue détectée sur le texte (repli : celle de l'interface),
//! voix jamais téléchargée par ce chemin (la notification mène à l'onglet
//! Read aloud des réglages, action « Show »).
//!
//! Rien n'est notifié quand tout va bien (le son suffit) ; une notification
//! (suite à part, comme la correction) dit pourquoi rien n'est lu.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use tauri::{AppHandle, Manager};

use super::service::{ReadingEnd, SegmentEvent, SegmentSink, SegmentsRefusal};
use super::{ReadAloud, ReadAloudError, VoiceLanguage};
use crate::dictation::clipboard::{Clipboard, SelectionError, SystemClipboard};
use crate::dictation::notify::{self, Action, DesktopNotifier, Notice, Notifier, Offer};
use crate::i18n::t;

/// Au-delà, la sélection n'est pas lue (comme la correction de la sélection).
pub const MAX_CHARS: usize = 100_000;

/// Route des réglages de la lecture à voix haute (action « Show » d'une
/// notification de voix absente).
pub const READ_ALOUD_SETTINGS_ROUTE: &str = "/settings?section=readAloud";

/// Le service de lecture vu par le raccourci (remplacé dans les essais).
pub(crate) trait Speaker: Send + Sync + 'static {
    /// Arrête la lecture de la sélection si elle est en cours ; vrai alors.
    fn stop(&self) -> bool;

    /// Lit `text` ; la lecture en cours s'arrête.
    fn read(&self, text: &str, sink: SegmentSink) -> Result<(), SegmentsRefusal>;
}

struct AppSpeaker(AppHandle);

impl Speaker for AppSpeaker {
    fn stop(&self) -> bool {
        self.0
            .try_state::<ReadAloud>()
            .is_some_and(|read_aloud| read_aloud.stop_selection())
    }

    fn read(&self, text: &str, sink: SegmentSink) -> Result<(), SegmentsRefusal> {
        match self.0.try_state::<ReadAloud>() {
            Some(read_aloud) => read_aloud.read_selection(text, sink),
            None => Err(SegmentsRefusal::RuntimeUnavailable),
        }
    }
}

/// Presse-papiers et notifications (faux dans les essais).
pub(crate) struct Services {
    pub clipboard: Arc<dyn Clipboard>,
    pub notifier: Arc<dyn Notifier>,
}

/// Lecture de la sélection (état géré par Tauri).
pub struct ReadSelection {
    services: Services,
    /// Notificateur du bureau, à configurer au démarrage ; absent dans les
    /// essais.
    desktop: Option<Arc<DesktopNotifier>>,
    /// Appui dont la sélection est en train d'être lue (avant le début de la
    /// lecture à voix haute) ; un nouvel appui le retire, ce qui l'annule.
    grabbing: Mutex<Option<u64>>,
    presses: AtomicU64,
}

impl Default for ReadSelection {
    fn default() -> Self {
        Self::new()
    }
}

impl ReadSelection {
    pub fn new() -> Self {
        let notifier = Arc::new(DesktopNotifier::new(Action::Show));
        let services = Services {
            clipboard: Arc::new(SystemClipboard::default()),
            notifier: notifier.clone(),
        };
        Self {
            desktop: Some(notifier),
            ..Self::with_services(services)
        }
    }

    pub(crate) fn with_services(services: Services) -> Self {
        Self {
            services,
            desktop: None,
            grabbing: Mutex::new(None),
            presses: AtomicU64::new(0),
        }
    }

    /// Appui : arrête la lecture de la sélection en cours, ou annule la
    /// lecture de la sélection pas encore commencée (`None` dans les deux
    /// cas) ; sinon, numéro du nouvel appui, dont la sélection est à lire.
    pub(crate) fn press(&self, speaker: &dyn Speaker) -> Option<u64> {
        let mut grabbing = self.grabbing.lock();
        if grabbing.take().is_some() || speaker.stop() {
            return None;
        }
        let press = self.presses.fetch_add(1, Ordering::Relaxed) + 1;
        *grabbing = Some(press);
        Some(press)
    }

    /// Lit la sélection de l'appui `press` et la fait lire à voix haute,
    /// sauf si un autre appui l'a annulé entre-temps ; notification si rien
    /// n'est lu.
    pub(crate) async fn read(&self, speaker: &dyn Speaker, press: u64) {
        let selected = self.services.clipboard.selection().await;
        let refused = {
            let mut grabbing = self.grabbing.lock();
            if *grabbing != Some(press) {
                return;
            }
            *grabbing = None;
            // Sous le verrou : un appui suivant trouve la lecture commencée,
            // et l'arrête.
            match checked(selected) {
                Ok(text) => speaker
                    .read(&readable(&text), self.sink())
                    .err()
                    .map(|refusal| refusal_notice(refusal, press)),
                Err(notice) => Some(notice),
            }
        };
        if let Some(notice) = refused {
            self.services.notifier.show(notice);
        }
    }

    /// Fin de la lecture : notification d'un échec seulement.
    fn sink(&self) -> SegmentSink {
        let notifier = self.services.notifier.clone();
        Arc::new(move |event| {
            if let SegmentEvent::Failed(end) = event {
                notifier.show(failure_notice(end));
            }
        })
    }
}

/// Sélection lue et contrôlée ; sinon, la notification qui dit pourquoi
/// rien n'est lu.
fn checked(selected: Result<String, SelectionError>) -> Result<String, Notice> {
    let text = selected.map_err(|error| {
        notice(
            t!("readSelection.unreadable"),
            &crate::proofread::selection::unreadable(error),
        )
    })?;
    if text.trim().is_empty() {
        return Err(notice(
            t!("readSelection.empty"),
            &t!("readSelection.emptyHint"),
        ));
    }
    if text.chars().count() > MAX_CHARS {
        return Err(notice(
            t!("readSelection.tooLong"),
            &t!("readSelection.tooLongHint"),
        ));
    }
    Ok(text)
}

/// Texte sélectionné tel que le service le lit (comme du Markdown) : le
/// retrait en tête des lignes est ôté, sinon une ligne en retrait de quatre
/// espaces (courriel, document) serait prise pour du code, qui ne se lit pas.
fn readable(text: &str) -> String {
    text.lines()
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n")
}

fn notice(summary: String, body: &str) -> Notice {
    Notice::lasting(summary, notify::escape(body))
}

/// Lecture refusée avant de commencer ; une voix absente propose « Show »
/// (onglet Read aloud des réglages).
fn refusal_notice(refusal: SegmentsRefusal, press: u64) -> Notice {
    match refusal {
        SegmentsRefusal::NothingToRead => notice(
            t!("readSelection.nothingToRead"),
            &t!("readSelection.nothingToReadHint"),
        ),
        SegmentsRefusal::Unsupported => notice(
            t!("readSelection.unsupported"),
            &t!("readSelection.unsupportedHint"),
        ),
        SegmentsRefusal::MissingVoice(language) => notice(
            missing_voice(language),
            &t!("readSelection.missingVoiceHint"),
        )
        .offering(Offer {
            session: press,
            text: String::new(),
        }),
        SegmentsRefusal::RuntimeUnavailable => notice(
            t!("readSelection.unavailable"),
            &t!("readSelection.unavailableHint"),
        ),
    }
}

fn missing_voice(language: VoiceLanguage) -> String {
    match language {
        VoiceLanguage::En => t!("readSelection.missingVoice.en"),
        VoiceLanguage::Es => t!("readSelection.missingVoice.es"),
        VoiceLanguage::Fr => t!("readSelection.missingVoice.fr"),
    }
}

/// Lecture commencée puis échouée.
fn failure_notice(end: ReadingEnd) -> Notice {
    match end {
        ReadingEnd::Failed(ReadAloudError::NoOutputDevice) => notice(
            t!("readSelection.failed"),
            &t!("readSelection.noOutputDevice"),
        ),
        ReadingEnd::Failed(ReadAloudError::RuntimeUnavailable) => notice(
            t!("readSelection.unavailable"),
            &t!("readSelection.unavailableHint"),
        ),
        ReadingEnd::NothingToRead => notice(
            t!("readSelection.nothingToRead"),
            &t!("readSelection.nothingToReadHint"),
        ),
        ReadingEnd::Unsupported => notice(
            t!("readSelection.unsupported"),
            &t!("readSelection.unsupportedHint"),
        ),
        ReadingEnd::MissingVoice(language) => notice(
            missing_voice(language),
            &t!("readSelection.missingVoiceHint"),
        ),
        ReadingEnd::Failed(_) | ReadingEnd::Finished | ReadingEnd::Stopped => {
            notice(t!("readSelection.failed"), "")
        }
    }
}

/// Démarrage : identité des notifications et clic sur « Show » (fenêtre
/// montrée, onglet Read aloud des réglages).
pub fn setup(app: &AppHandle) {
    let Some(selection) = app.try_state::<ReadSelection>() else {
        return;
    };
    let Some(notifier) = &selection.desktop else {
        return;
    };
    notifier.configure(
        app.config().identifier.clone(),
        crate::paths::notification_icon(app).ok(),
    );
    let handle = app.clone();
    notifier.on_action(move |_| {
        crate::desktop::open_route(&handle, READ_ALOUD_SETTINGS_ROUTE);
    });
}

/// Lit à voix haute la sélection courante (appui du raccourci, `chatonlm
/// --read-aloud`) ; un appui pendant la lecture l'arrête.
pub fn read_selection(app: &AppHandle) {
    let Some(selection) = app.try_state::<ReadSelection>() else {
        return;
    };
    let speaker = AppSpeaker(app.clone());
    let Some(press) = selection.press(&speaker) else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let selection = app.state::<ReadSelection>();
        selection.read(&AppSpeaker(app.clone()), press).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictation::clipboard::BoxFuture;
    use std::sync::atomic::AtomicBool;
    use tokio::sync::Notify;

    #[derive(Default)]
    struct FakeClipboard {
        selected: Mutex<Option<Result<String, SelectionError>>>,
        /// La lecture de la sélection attend `release`.
        gate: Option<Arc<Notify>>,
    }

    impl FakeClipboard {
        fn with(selected: Result<String, SelectionError>) -> Self {
            Self {
                selected: Mutex::new(Some(selected)),
                gate: None,
            }
        }
    }

    impl Clipboard for FakeClipboard {
        fn copy(&self, _text: String) -> BoxFuture<Result<(), String>> {
            Box::pin(async { Err("not used".into()) })
        }

        fn selection(&self) -> BoxFuture<Result<String, SelectionError>> {
            let selected = self
                .selected
                .lock()
                .clone()
                .unwrap_or(Err(SelectionError::Unsupported));
            let gate = self.gate.clone();
            Box::pin(async move {
                if let Some(gate) = gate {
                    gate.notified().await;
                }
                selected
            })
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

    /// Service de lecture : textes lus, lecture en cours, refus voulu.
    #[derive(Default)]
    struct FakeSpeaker {
        reads: Mutex<Vec<String>>,
        reading: AtomicBool,
        refusal: Mutex<Option<SegmentsRefusal>>,
        sink: Mutex<Option<SegmentSink>>,
    }

    impl Speaker for FakeSpeaker {
        fn stop(&self) -> bool {
            self.reading.swap(false, Ordering::SeqCst)
        }

        fn read(&self, text: &str, sink: SegmentSink) -> Result<(), SegmentsRefusal> {
            if let Some(refusal) = *self.refusal.lock() {
                return Err(refusal);
            }
            self.reads.lock().push(text.to_owned());
            self.reading.store(true, Ordering::SeqCst);
            *self.sink.lock() = Some(sink);
            Ok(())
        }
    }

    fn setup(clipboard: FakeClipboard) -> (ReadSelection, Arc<FakeNotifier>) {
        let notifier = Arc::new(FakeNotifier::default());
        let selection = ReadSelection::with_services(Services {
            clipboard: Arc::new(clipboard),
            notifier: notifier.clone(),
        });
        (selection, notifier)
    }

    async fn press_and_read(selection: &ReadSelection, speaker: &FakeSpeaker) -> bool {
        match selection.press(speaker) {
            Some(press) => {
                selection.read(speaker, press).await;
                true
            }
            None => false,
        }
    }

    fn summaries(notifier: &FakeNotifier) -> Vec<String> {
        notifier
            .shown
            .lock()
            .iter()
            .map(|notice| notice.summary.clone())
            .collect()
    }

    #[tokio::test]
    async fn a_press_reads_the_selection_and_the_next_one_stops_it() {
        let (selection, notifier) = setup(FakeClipboard::with(Ok("Hello there.".into())));
        let speaker = FakeSpeaker::default();
        assert!(press_and_read(&selection, &speaker).await);
        assert_eq!(*speaker.reads.lock(), ["Hello there."]);
        assert!(speaker.reading.load(Ordering::SeqCst));
        // Second appui : la lecture s'arrête, rien n'est relu.
        assert!(!press_and_read(&selection, &speaker).await);
        assert!(!speaker.reading.load(Ordering::SeqCst));
        assert_eq!(speaker.reads.lock().len(), 1);
        // Lecture finie (ou arrêtée) : l'appui suivant relit.
        assert!(press_and_read(&selection, &speaker).await);
        assert_eq!(speaker.reads.lock().len(), 2);
        assert!(notifier.shown.lock().is_empty());
    }

    #[tokio::test]
    async fn a_press_while_the_selection_is_read_cancels_the_reading() {
        let gate = Arc::new(Notify::new());
        let clipboard = FakeClipboard {
            gate: Some(gate.clone()),
            ..FakeClipboard::with(Ok("Hello there.".into()))
        };
        let (selection, notifier) = setup(clipboard);
        let selection = Arc::new(selection);
        let speaker = Arc::new(FakeSpeaker::default());
        let first = selection.press(speaker.as_ref()).expect("first press");
        let task = {
            let (selection, speaker) = (selection.clone(), speaker.clone());
            tokio::spawn(async move { selection.read(speaker.as_ref(), first).await })
        };
        // Appui pendant la lecture de la sélection : annulée.
        assert_eq!(selection.press(speaker.as_ref()), None);
        gate.notify_one();
        task.await.unwrap();
        assert!(speaker.reads.lock().is_empty());
        assert!(notifier.shown.lock().is_empty());
        // L'appui suivant lit de nouveau.
        let third = selection.press(speaker.as_ref()).expect("third press");
        let task = {
            let (selection, speaker) = (selection.clone(), speaker.clone());
            tokio::spawn(async move { selection.read(speaker.as_ref(), third).await })
        };
        gate.notify_one();
        task.await.unwrap();
        assert_eq!(*speaker.reads.lock(), ["Hello there."]);
    }

    #[tokio::test]
    async fn nothing_selected_or_unreadable_is_notified() {
        let speaker = FakeSpeaker::default();
        let (selection, notifier) = setup(FakeClipboard::with(Ok(" \n ".into())));
        assert!(press_and_read(&selection, &speaker).await);
        let (unreadable, unreadable_notifier) =
            setup(FakeClipboard::with(Err(SelectionError::Timeout)));
        assert!(press_and_read(&unreadable, &speaker).await);
        let (long, long_notifier) = setup(FakeClipboard::with(Ok("a".repeat(MAX_CHARS + 1))));
        assert!(press_and_read(&long, &speaker).await);
        assert!(speaker.reads.lock().is_empty());
        assert_eq!(summaries(&notifier), ["No text selected"]);
        assert_eq!(
            summaries(&unreadable_notifier),
            ["Cannot read the selected text"]
        );
        assert_eq!(
            unreadable_notifier.shown.lock()[0].body,
            "The app that holds the selected text did not answer."
        );
        assert_eq!(summaries(&long_notifier), ["The selected text is too long"]);
        // Rien n'est en cours : un appui suivant relit (pas d'arrêt).
        assert!(selection.press(&speaker).is_some());
    }

    #[tokio::test]
    async fn a_missing_voice_offers_the_settings_and_refusals_are_notified() {
        let (selection, notifier) = setup(FakeClipboard::with(Ok("Bonjour à tous.".into())));
        let speaker = FakeSpeaker::default();
        *speaker.refusal.lock() = Some(SegmentsRefusal::MissingVoice(VoiceLanguage::Fr));
        assert!(press_and_read(&selection, &speaker).await);
        *speaker.refusal.lock() = Some(SegmentsRefusal::Unsupported);
        assert!(press_and_read(&selection, &speaker).await);
        let shown = notifier.shown.lock().clone();
        assert_eq!(shown[0].summary, "The French voice is not downloaded");
        assert!(shown[0].copy.is_some(), "« Show » ouvre les réglages");
        assert_eq!(shown[1].summary, "Language not supported");
        assert!(shown[1].copy.is_none());
    }

    #[tokio::test]
    async fn a_failed_reading_is_notified_but_not_its_end() {
        let (selection, notifier) = setup(FakeClipboard::with(Ok("Hello.".into())));
        let speaker = FakeSpeaker::default();
        assert!(press_and_read(&selection, &speaker).await);
        let sink = speaker.sink.lock().clone().expect("sink");
        sink(SegmentEvent::Started(
            super::super::service::SELECTION_SEGMENT.into(),
        ));
        sink(SegmentEvent::Stopped);
        assert!(notifier.shown.lock().is_empty());
        sink(SegmentEvent::Failed(ReadingEnd::Failed(
            ReadAloudError::NoOutputDevice,
        )));
        let shown = notifier.shown.lock().clone();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].summary, "Could not read the text aloud");
        assert_eq!(shown[0].body, "No audio output was found.");
    }

    #[test]
    fn indented_lines_are_read_rather_than_taken_for_code() {
        let text = "Dear all,\n\n    the meeting moves to Friday.\n\tThanks.";
        assert_eq!(
            readable(text),
            "Dear all,\n\nthe meeting moves to Friday.\nThanks."
        );
        assert!(
            super::super::text::spoken_text(&readable(text)).contains("meeting moves to Friday")
        );
        assert!(!super::super::text::spoken_text(text).contains("meeting moves to Friday"));
    }
}
