//! Machine d'états de la dictée, avec faux micro, moteur, presse-papiers et
//! notifications ; nettoyage par le faux serveur Ollama.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use fake_ollama::{FakeOllama, Mode};

use super::audio::{Captured, Prerecorded, Recording};
use super::clipboard::PasteError;
use super::engine::Recognizer;
use super::*;

const WAIT: Duration = Duration::from_secs(10);
/// Titres des notifications (les essais tournent en anglais, langue par
/// défaut).
const LISTENING: &str = "Listening…";
const TRANSCRIBING: &str = "Transcribing…";
const SPOKEN: &str = "hello world this is a dictation test";

/// « Parole » (syllabes : 250 ms de sinus à -13 dBFS, 150 ms de silence) à
/// 48 kHz.
fn speech(seconds: f32) -> Captured {
    speech_at(seconds, 48_000)
}

fn speech_at(seconds: f32, rate: u32) -> Captured {
    let count = (rate as f32 * seconds) as usize;
    let syllable = rate as usize * 400 / 1000;
    let sound = rate as usize * 250 / 1000;
    Captured {
        samples: (0..count)
            .map(|i| {
                if i % syllable < sound {
                    0.3 * (i as f32 * 0.05).sin()
                } else {
                    0.0
                }
            })
            .collect(),
        rate,
    }
}

/// Faux micro : rend `captured` à l'arrêt, ou l'erreur réglée ; compte les
/// ouvertures et les fermetures (enregistrement fini ou lâché).
struct FakeMicrophone {
    captured: Mutex<Result<Captured, String>>,
    opens: AtomicUsize,
    closes: Arc<AtomicUsize>,
    /// Ouverture retenue jusqu'au feu vert du test ; ouvertures commencées.
    gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    opening: AtomicUsize,
}

impl Microphone for FakeMicrophone {
    fn open(&self) -> Result<Box<dyn Recording>, String> {
        self.opening.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.lock().take();
        if let Some(gate) = gate {
            let _ = gate.recv_timeout(WAIT);
        }
        self.opens.fetch_add(1, Ordering::SeqCst);
        let captured = self.captured.lock().clone()?;
        Ok(Box::new(Tracked {
            captured: Some(Prerecorded(captured)),
            closes: self.closes.clone(),
        }))
    }
}

/// Enregistrement dont la fermeture est comptée.
struct Tracked {
    captured: Option<Prerecorded>,
    closes: Arc<AtomicUsize>,
}

impl Recording for Tracked {
    fn finish(mut self: Box<Self>) -> Captured {
        Box::new(self.captured.take().unwrap()).finish()
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct FakeClipboard {
    copied: Mutex<Vec<String>>,
    fail: Mutex<Option<String>>,
    /// Copie bloquée (sans rendre la main) jusqu'au feu vert du test.
    gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    entered: std::sync::atomic::AtomicBool,
    /// Collages : dernier texte copié au moment du Ctrl+V.
    pastes: Mutex<Vec<Option<String>>>,
    paste_error: Mutex<Option<PasteError>>,
    /// Collage retenu jusqu'au feu vert du test (boîte du bureau ouverte) ;
    /// collages menés à leur terme.
    paste_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    pastes_finished: Arc<AtomicUsize>,
}

impl Clipboard for FakeClipboard {
    fn copy(&self, text: String) -> BoxFuture<Result<(), String>> {
        self.entered.store(true, Ordering::SeqCst);
        let gate = self.gate.lock().take();
        if let Some(gate) = gate {
            let _ = gate.recv_timeout(WAIT);
        }
        let result = match self.fail.lock().clone() {
            Some(error) => Err(error),
            None => {
                self.copied.lock().push(text);
                Ok(())
            }
        };
        Box::pin(async move { result })
    }

    fn paste(&self) -> BoxFuture<Result<(), PasteError>> {
        self.pastes.lock().push(self.copied.lock().last().cloned());
        let result = match self.paste_error.lock().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        };
        let gate = self.paste_gate.lock().clone();
        let finished = self.pastes_finished.clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.notified().await;
            }
            finished.fetch_add(1, Ordering::SeqCst);
            result
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Shown {
    Notice(Notice),
    Withdrawn,
}

#[derive(Default)]
struct FakeNotifier {
    shown: Mutex<Vec<Shown>>,
}

impl Notifier for FakeNotifier {
    fn show(&self, notice: Notice) {
        self.shown.lock().push(Shown::Notice(notice));
    }

    fn withdraw(&self) {
        self.shown.lock().push(Shown::Withdrawn);
    }
}

/// Modèle de transcription simulé : rend `SPOKEN`, après le feu vert du test
/// si `gated`.
#[derive(Default)]
struct Model {
    transcriptions: AtomicUsize,
    gated: std::sync::atomic::AtomicBool,
    gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    text: Mutex<String>,
    /// Durée d'un chargement ; chargements commencés et finis.
    load_delay: Mutex<Duration>,
    loads_started: AtomicUsize,
    loads_finished: AtomicUsize,
    /// Requêtes reçues (langue, glossaire).
    requests: Mutex<Vec<Request>>,
}

struct FakeRecognizer(Arc<Model>);

impl Recognizer for FakeRecognizer {
    fn transcribe(&mut self, _samples: &[f32], request: &Request) -> Result<String, String> {
        self.0.transcriptions.fetch_add(1, Ordering::SeqCst);
        self.0.requests.lock().push(request.clone());
        if self.0.gated.load(Ordering::SeqCst) {
            let gate = self.0.gate.lock().take();
            if let Some(gate) = gate {
                let _ = gate.recv_timeout(WAIT);
            }
        }
        let text = self.0.text.lock().clone();
        Ok(match &request.language {
            Some(language) => format!("{text} ({language})"),
            None => text,
        })
    }
}

struct FakeHost {
    settings: Mutex<DictationSettings>,
    models_dir: tempfile::TempDir,
    server: String,
    /// Contexte et `keep_alive` des réglages de l'app.
    context_length: Mutex<u32>,
    keep_alive: Mutex<i64>,
    trigger: Mutex<Option<String>>,
    phases: Mutex<Vec<DictationPhase>>,
    /// Lectures du serveur (adresse d'Ollama) ; retenues jusqu'au feu vert
    /// si une barrière est posée (Ollama intégré qui démarre).
    server_reads: AtomicUsize,
    server_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
}

impl Host for FakeHost {
    fn prepare(&self) -> BoxFuture<Result<Prepared, String>> {
        let prepared = Prepared {
            settings: self.settings.lock().clone(),
            models_dir: self.models_dir.path().to_path_buf(),
            always_thinks: false,
            trigger: self.trigger.lock().clone(),
        };
        Box::pin(async move { Ok(prepared) })
    }

    fn server(&self) -> BoxFuture<Result<Server, String>> {
        self.server_reads.fetch_add(1, Ordering::SeqCst);
        let server = Server {
            url: self.server.clone(),
            context_length: *self.context_length.lock(),
            keep_alive: *self.keep_alive.lock(),
            cloud: false,
        };
        let gate = self.server_gate.lock().clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.notified().await;
            }
            Ok(server)
        })
    }

    fn changed(&self, phase: DictationPhase) {
        self.phases.lock().push(phase);
    }
}

struct Rig {
    core: Arc<Core>,
    host: Arc<FakeHost>,
    microphone: Arc<FakeMicrophone>,
    clipboard: Arc<FakeClipboard>,
    notifier: Arc<FakeNotifier>,
    model: Arc<Model>,
    server: FakeOllama,
}

/// Dictée activée, whisper-tiny installé (fichier creux à la bonne taille),
/// nettoyage par `polish_model` sur le faux serveur.
async fn rig(polish_model: &str) -> Rig {
    rig_on(polish_model, |_| Ok(())).await
}

/// [`rig`] sur une machine dont `support` dit quels moteurs y tournent.
async fn rig_on(polish_model: &str, support: fn(SpeechEngine) -> Result<(), String>) -> Rig {
    let server = FakeOllama::start(Mode::ollama()).await.unwrap();
    let models_dir = tempfile::tempdir().unwrap();
    let tiny = models::find("whisper-tiny").unwrap();
    let dir = models_dir.path().join(tiny.id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::File::create(dir.join(tiny.files[0].path))
        .unwrap()
        .set_len(tiny.files[0].size)
        .unwrap();
    let host = Arc::new(FakeHost {
        settings: Mutex::new(DictationSettings {
            enabled: true,
            mode: DictationMode::Toggle,
            speech_model: tiny.id.into(),
            language: "auto".into(),
            polish_model: polish_model.into(),
            polish_prompt: String::new(),
            dictionary: Vec::new(),
            notify_text: true,
            auto_copy: true,
            // Collage essayé à part (`paste_*`).
            auto_paste: false,
        }),
        models_dir,
        server: server.url(),
        context_length: Mutex::new(0),
        keep_alive: Mutex::new(0),
        trigger: Mutex::new(Some("Ctrl+Alt+D".into())),
        phases: Mutex::new(Vec::new()),
        server_reads: AtomicUsize::new(0),
        server_gate: Mutex::new(None),
    });
    let microphone = Arc::new(FakeMicrophone {
        captured: Mutex::new(Ok(speech(1.0))),
        opens: AtomicUsize::new(0),
        closes: Arc::default(),
        gate: Mutex::new(None),
        opening: AtomicUsize::new(0),
    });
    let clipboard = Arc::new(FakeClipboard::default());
    let notifier = Arc::new(FakeNotifier::default());
    let model = Arc::new(Model::default());
    *model.text.lock() = SPOKEN.into();
    let loaded = model.clone();
    let engine = Engine::with_loader(
        Box::new(move |_spec: &ModelSpec| {
            loaded.loads_started.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(*loaded.load_delay.lock());
            loaded.loads_finished.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeRecognizer(loaded.clone())) as Box<dyn Recognizer>)
        }),
        engine::IDLE,
    );
    let core = Arc::new(Core::new(Services {
        microphone: microphone.clone(),
        engine,
        clipboard: clipboard.clone(),
        notifier: notifier.clone(),
        max_recording: audio::MAX_DURATION,
        support,
    }));
    Rig {
        core,
        host,
        microphone,
        clipboard,
        notifier,
        model,
        server,
    }
}

impl Rig {
    fn host(&self) -> Arc<dyn Host> {
        self.host.clone()
    }

    fn toggle(&self) {
        self.core.toggle(self.host());
    }

    fn phase(&self) -> DictationPhase {
        *self.core.phase.borrow()
    }

    fn last_result(&self) -> Option<DictationResult> {
        self.core.inner.lock().last_result.clone()
    }

    fn shown(&self) -> Vec<Shown> {
        self.notifier.shown.lock().clone()
    }

    fn summaries(&self) -> Vec<String> {
        self.shown()
            .into_iter()
            .map(|shown| match shown {
                Shown::Notice(notice) => notice.summary,
                Shown::Withdrawn => "-".into(),
            })
            .collect()
    }

    async fn wait(&self, what: &str, condition: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !condition(self) {
            assert!(
                Instant::now() < deadline,
                "{what}: phase {:?}, shown {:?}",
                self.phase(),
                self.shown()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Enregistrement en cours, micro ouvert (« Listening… » montré).
    async fn wait_listening(&self) {
        self.wait("listening", |rig| {
            rig.summaries().last().map(String::as_str) == Some(LISTENING)
        })
        .await;
    }

    async fn wait_idle(&self) {
        self.wait("idle", |rig| {
            rig.phase() == DictationPhase::Idle && rig.core.inner.lock().stage == Stage::Idle
        })
        .await;
    }

    /// Une dictée complète : début, arrêt, fin.
    async fn dictate(&self) {
        self.toggle();
        self.wait_listening().await;
        self.toggle();
        self.wait_idle().await;
    }

    /// Dictée qui échoue avant d'écouter.
    async fn dictate_failing(&self) {
        self.toggle();
        self.wait_idle().await;
    }

    fn last_notice(&self) -> Notice {
        match self.shown().last() {
            Some(Shown::Notice(notice)) => notice.clone(),
            other => panic!("no notice: {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_dictation_is_transcribed_polished_copied_and_notified() {
    let rig = rig("chatonlm-polish").await;
    rig.dictate().await;
    let polished = format!("Polished: {SPOKEN}");
    assert_eq!(
        *rig.clipboard.copied.lock(),
        std::slice::from_ref(&polished)
    );
    let result = rig.last_result().unwrap();
    assert_eq!(result.text, polished);
    assert_eq!(result.raw, SPOKEN);
    assert!(result.polished && result.copied);
    assert_eq!(result.error, None);
    assert_eq!(
        *rig.host.phases.lock(),
        [
            DictationPhase::Recording,
            DictationPhase::Transcribing,
            DictationPhase::Polishing,
            DictationPhase::Idle,
        ]
    );
    // Une seule notification, remplacée d'état en état.
    let shown = rig.shown();
    assert_eq!(
        shown,
        [
            Shown::Notice(Notice::progress(
                Title::Listening,
                "Press Ctrl+Alt+D again to stop"
            )),
            Shown::Notice(Notice::progress(Title::Transcribing, "")),
            Shown::Notice(Notice::last(Title::Copied, polished)),
        ]
    );
    assert_eq!(rig.model.transcriptions.load(Ordering::SeqCst), 1);
}

/// rust-2 : le micro s'ouvre sans attendre Ollama (l'intégré peut démarrer
/// pendant que l'on parle) ; le serveur n'est lu qu'au moment du nettoyage,
/// et jamais sans modèle de nettoyage.
#[tokio::test]
async fn the_microphone_opens_without_waiting_for_ollama() {
    let rig = rig("chatonlm-polish").await;
    let gate = Arc::new(tokio::sync::Notify::new());
    *rig.host.server_gate.lock() = Some(gate.clone());
    rig.toggle();
    rig.wait_listening().await;
    assert_eq!(rig.microphone.opens.load(Ordering::SeqCst), 1);
    assert_eq!(rig.host.server_reads.load(Ordering::SeqCst), 0);
    rig.toggle();
    rig.wait("polishing", |rig| {
        rig.host.server_reads.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(rig.phase(), DictationPhase::Polishing);
    gate.notify_one();
    rig.wait_idle().await;
    assert!(rig.last_result().unwrap().polished);

    let plain = self::rig("").await;
    plain.dictate().await;
    assert_eq!(plain.last_result().unwrap().raw, SPOKEN);
    assert_eq!(plain.host.server_reads.load(Ordering::SeqCst), 0);
}

/// Le nettoyage charge le modèle comme le chat : contexte et durée en
/// mémoire réglés dans l'app (onglet Models), rien d'envoyé par défaut.
#[tokio::test]
async fn the_polish_loads_the_model_as_set_in_the_app() {
    for (context_length, keep_alive) in [(0, 0), (8192, 300), (0, -1)] {
        let case = format!("{context_length} {keep_alive}");
        let rig = rig("chatonlm-polish").await;
        *rig.host.context_length.lock() = context_length;
        *rig.host.keep_alive.lock() = keep_alive;
        rig.dictate().await;
        assert!(rig.last_result().unwrap().polished, "{case}");
        let request = rig.server.chat_requests().pop().unwrap();
        let sent = (keep_alive != 0).then_some(keep_alive);
        assert_eq!(
            request
                .get("keep_alive")
                .and_then(serde_json::Value::as_i64),
            sent,
            "{case}"
        );
        let context = (context_length > 0).then_some(u64::from(context_length));
        assert_eq!(request["options"]["num_ctx"].as_u64(), context, "{case}");
    }
}

#[tokio::test]
async fn without_polish_the_transcript_is_copied_and_can_stay_hidden() {
    let rig = rig("").await;
    {
        let mut settings = rig.host.settings.lock();
        settings.notify_text = false;
        settings.language = "fr".into();
        settings.mode = DictationMode::Hold;
    }
    rig.core.start(rig.host());
    rig.wait_listening().await;
    match &rig.shown()[0] {
        Shown::Notice(notice) => assert_eq!(notice.body, "Release Ctrl+Alt+D to stop"),
        other => panic!("{other:?}"),
    }
    rig.core.stop();
    rig.wait_idle().await;
    let expected = format!("{SPOKEN} (fr)");
    assert_eq!(
        *rig.clipboard.copied.lock(),
        std::slice::from_ref(&expected)
    );
    assert_eq!(
        rig.last_notice(),
        Notice::last(Title::Copied, notify::text_hidden())
    );
    let result = rig.last_result().unwrap();
    assert_eq!((result.polished, result.error), (false, None));
    assert!(!rig.host.phases.lock().contains(&DictationPhase::Polishing));
}

#[tokio::test]
async fn a_rejected_polish_copies_the_raw_transcript() {
    let rig = rig("chatonlm-polish-poem").await;
    *rig.host.trigger.lock() = None;
    *rig.model.text.lock() = "the <b>bold</b> & brave".into();
    rig.dictate().await;
    match &rig.shown()[0] {
        Shown::Notice(notice) => assert_eq!(notice.body, "Run the shortcut again to stop"),
        other => panic!("{other:?}"),
    }
    assert_eq!(*rig.clipboard.copied.lock(), ["the <b>bold</b> & brave"]);
    let result = rig.last_result().unwrap();
    assert!(!result.polished && result.copied);
    assert_eq!(result.error.as_deref(), Some(polish::skipped().as_str()));
    // Texte échappé dans la notification.
    assert_eq!(
        rig.last_notice(),
        Notice::last(
            Title::CopiedRaw,
            "the &lt;b&gt;bold&lt;/b&gt; &amp; brave".into()
        )
    );
}

/// Copie automatique coupée : rien dans le presse-papiers, pas d'erreur ; la
/// notification « Transcription ready » propose « Copy », qui copie le texte
/// de ce résultat.
#[tokio::test]
async fn without_auto_copy_the_notification_offers_copy() {
    let rig = rig("").await;
    rig.host.settings.lock().auto_copy = false;
    rig.dictate().await;
    assert!(rig.clipboard.copied.lock().is_empty());
    let result = rig.last_result().unwrap();
    assert_eq!(result.text, SPOKEN);
    assert!(!result.copied);
    assert_eq!(result.error, None);
    let offer = Offer {
        session: 1,
        text: SPOKEN.into(),
    };
    assert_eq!(
        rig.last_notice(),
        Notice::last(Title::Ready, SPOKEN.into()).offering(offer.clone())
    );
    // Clic sur « Copy ».
    assert_eq!(rig.core.copy_offer(offer).await, Ok(true));
    assert_eq!(*rig.clipboard.copied.lock(), [SPOKEN]);
    assert!(rig.last_result().unwrap().copied);
}

#[tokio::test]
async fn without_auto_copy_a_skipped_polish_is_still_said() {
    let rig = rig("chatonlm-polish-poem").await;
    {
        let mut settings = rig.host.settings.lock();
        settings.auto_copy = false;
        settings.notify_text = false;
    }
    rig.dictate().await;
    let result = rig.last_result().unwrap();
    assert!(!result.copied && !result.polished);
    // Le nettoyage sauté reste la seule « erreur ».
    assert_eq!(result.error.as_deref(), Some(polish::skipped().as_str()));
    let notice = rig.last_notice();
    assert_eq!(notice.summary, Title::ReadyRaw.text());
    assert_eq!(notice.body, notify::text_hidden());
    assert!(notice.copy.is_some());
    assert!(rig.clipboard.copied.lock().is_empty());
}

/// Collage activé : le texte est copié, puis collé là où se trouve le
/// curseur (Ctrl+V après la copie) ; la notification le dit.
#[tokio::test]
async fn auto_paste_pastes_the_copied_text_where_the_cursor_is() {
    let rig = rig("chatonlm-polish").await;
    rig.host.settings.lock().auto_paste = true;
    rig.dictate().await;
    let polished = format!("Polished: {SPOKEN}");
    assert_eq!(*rig.clipboard.pastes.lock(), [Some(polished.clone())]);
    let result = rig.last_result().unwrap();
    assert!(result.copied && result.pasted);
    assert_eq!((result.error, result.paste_error), (None, None));
    assert_eq!(rig.last_notice(), Notice::last(Title::Pasted, polished));
    assert_eq!(Title::Pasted.text(), "Text pasted");

    // Nettoyage écarté : collé quand même, le titre le dit.
    let raw = self::rig("chatonlm-polish-poem").await;
    raw.host.settings.lock().auto_paste = true;
    raw.dictate().await;
    assert_eq!(*raw.clipboard.pastes.lock(), [Some(SPOKEN.into())]);
    let result = raw.last_result().unwrap();
    assert!(result.pasted && !result.polished);
    assert_eq!(result.error.as_deref(), Some(polish::skipped().as_str()));
    assert_eq!(raw.last_notice().summary, "Text pasted (not polished)");
}

/// Collage impossible : le texte reste copié, la notification et le
/// résultat disent pourquoi et de coller par Ctrl+V.
#[tokio::test]
async fn a_failed_paste_leaves_the_text_copied_and_says_why() {
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
            "Pasting failed (no <reply>): press Ctrl+V.",
        ),
    ] {
        let rig = rig("").await;
        rig.host.settings.lock().auto_paste = true;
        *rig.clipboard.paste_error.lock() = Some(error.clone());
        rig.dictate().await;
        assert_eq!(*rig.clipboard.copied.lock(), [SPOKEN], "{error:?}");
        assert_eq!(rig.clipboard.pastes.lock().len(), 1, "{error:?}");
        let result = rig.last_result().unwrap();
        assert!(result.copied && !result.pasted, "{error:?}");
        assert_eq!(result.error, None, "{error:?}");
        assert_eq!(result.paste_error.as_deref(), Some(reason), "{error:?}");
        let notice = rig.last_notice();
        assert_eq!(notice.summary, Title::Copied.text(), "{error:?}");
        assert_eq!(
            notice.body,
            format!("{SPOKEN}\n\n{}", notify::escape(reason)),
            "{error:?}"
        );
    }
    // Texte masqué : la raison reste dite.
    let rig = rig("").await;
    {
        let mut settings = rig.host.settings.lock();
        settings.auto_paste = true;
        settings.notify_text = false;
    }
    *rig.clipboard.paste_error.lock() = Some(PasteError::Refused);
    rig.dictate().await;
    assert_eq!(
        rig.last_notice().body,
        format!(
            "{}\n\nPasting was not allowed: press Ctrl+V.",
            notify::text_hidden()
        )
    );
}

/// Dictée abandonnée pendant le collage (boîte d'autorisation du bureau
/// ouverte) : le collage va à son terme (session du portail fermée), sans
/// résultat ni notification de fin.
#[tokio::test]
async fn a_dictation_cancelled_while_pasting_lets_the_paste_finish() {
    let rig = rig("").await;
    rig.host.settings.lock().auto_paste = true;
    let gate = Arc::new(tokio::sync::Notify::new());
    *rig.clipboard.paste_gate.lock() = Some(gate.clone());
    rig.toggle();
    rig.wait_listening().await;
    rig.toggle();
    rig.wait("collage commencé", |rig| {
        rig.clipboard.pastes.lock().len() == 1
    })
    .await;
    assert!(rig.core.cancel());
    gate.notify_one();
    rig.wait("collage fini", |rig| {
        rig.clipboard.pastes_finished.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(rig.last_result(), None);
    assert_eq!(rig.shown().last(), Some(&Shown::Withdrawn));
}

/// Essai lancé depuis la page : copié, jamais collé (le curseur est dans
/// ChatonLM).
#[tokio::test]
async fn a_dictation_started_from_the_page_is_not_pasted() {
    let rig = rig("").await;
    rig.host.settings.lock().auto_paste = true;
    rig.core.toggle_from(rig.host(), Origin::Page);
    rig.wait_listening().await;
    rig.core.toggle_from(rig.host(), Origin::Page);
    rig.wait_idle().await;
    assert_eq!(*rig.clipboard.copied.lock(), [SPOKEN]);
    assert!(rig.clipboard.pastes.lock().is_empty());
    let result = rig.last_result().unwrap();
    assert!(result.copied && !result.pasted && result.paste_error.is_none());
    assert_eq!(rig.last_notice().summary, Title::Copied.text());
    // Commencée au raccourci, arrêtée depuis la page : collée.
    rig.toggle();
    rig.wait_listening().await;
    rig.core.toggle_from(rig.host(), Origin::Page);
    rig.wait_idle().await;
    assert_eq!(rig.clipboard.pastes.lock().len(), 1);
}

/// Rien n'est collé sans le réglage, sans copie automatique (le
/// presse-papiers n'a pas le texte), si la copie échoue, ni par « Copy ».
#[tokio::test]
async fn nothing_is_pasted_without_the_text_in_the_clipboard() {
    let off = rig("").await;
    off.dictate().await;
    assert!(off.clipboard.pastes.lock().is_empty());
    assert!(!off.last_result().unwrap().pasted);
    assert_eq!(off.last_notice().summary, Title::Copied.text());

    let no_copy = rig("").await;
    {
        let mut settings = no_copy.host.settings.lock();
        settings.auto_paste = true;
        settings.auto_copy = false;
    }
    no_copy.dictate().await;
    let offer = no_copy.last_notice().copy.unwrap();
    assert_eq!(no_copy.core.copy_offer(offer).await, Ok(true));
    assert_eq!(no_copy.core.copy_last().await, Ok(()));
    assert_eq!(no_copy.clipboard.copied.lock().len(), 2);
    assert!(no_copy.clipboard.pastes.lock().is_empty());
    let result = no_copy.last_result().unwrap();
    assert!(!result.pasted && result.paste_error.is_none());

    let failed = rig("").await;
    failed.host.settings.lock().auto_paste = true;
    *failed.clipboard.fail.lock() = Some("no data-control protocol".into());
    failed.dictate().await;
    assert!(failed.clipboard.pastes.lock().is_empty());
    let result = failed.last_result().unwrap();
    assert!(result.copy_failed && !result.pasted && result.paste_error.is_none());
}

/// L'action d'une notification plus ancienne copie son propre texte, sans
/// marquer le résultat suivant comme copié.
#[tokio::test]
async fn an_older_copy_action_copies_its_own_text_only() {
    let rig = rig("").await;
    rig.host.settings.lock().auto_copy = false;
    rig.dictate().await;
    let first = rig.last_notice().copy.unwrap();
    *rig.model.text.lock() = "second text".into();
    rig.dictate().await;
    assert_eq!(rig.core.copy_offer(first).await, Ok(false));
    assert_eq!(*rig.clipboard.copied.lock(), [SPOKEN]);
    let last = rig.last_result().unwrap();
    assert_eq!(last.text, "second text");
    assert!(!last.copied);
}

#[tokio::test]
async fn copy_last_copies_the_last_text() {
    let rig = rig("").await;
    assert_eq!(rig.core.copy_last().await, Err(CopyError::Nothing));
    // Dictée échouée : rien à copier.
    *rig.microphone.captured.lock() = Err(audio::no_microphone());
    rig.dictate_failing().await;
    assert_eq!(rig.core.copy_last().await, Err(CopyError::Nothing));

    *rig.microphone.captured.lock() = Ok(speech(1.0));
    rig.host.settings.lock().auto_copy = false;
    rig.dictate().await;
    *rig.clipboard.fail.lock() = Some("no data-control protocol".into());
    assert_eq!(
        rig.core.copy_last().await,
        Err(CopyError::Clipboard("no data-control protocol".into()))
    );
    assert!(!rig.last_result().unwrap().copied);
    *rig.clipboard.fail.lock() = None;
    assert_eq!(rig.core.copy_last().await, Ok(()));
    assert_eq!(*rig.clipboard.copied.lock(), [SPOKEN]);
    assert!(rig.last_result().unwrap().copied);
}

#[tokio::test]
async fn a_clipboard_failure_keeps_the_text_in_the_result() {
    let rig = rig("").await;
    *rig.clipboard.fail.lock() = Some("no data-control protocol".into());
    rig.dictate().await;
    let result = rig.last_result().unwrap();
    assert_eq!(result.text, SPOKEN);
    assert!(!result.copied && result.copy_failed);
    let error = "Could not copy the text to the clipboard: no data-control protocol";
    assert_eq!(result.error.as_deref(), Some(error));
    assert_eq!(
        rig.last_notice(),
        Notice::last(Title::NotCopied, error.into())
    );
}

#[tokio::test]
async fn start_is_refused_without_a_usable_model_or_when_turned_off() {
    let rig = rig("").await;
    let refused = |rig: &Rig| {
        let result = rig.last_result().unwrap();
        assert!(result.text.is_empty() && !result.copied && !result.copy_failed);
        assert_eq!(rig.last_notice().summary, Title::Failed.text());
        result.error.unwrap()
    };
    rig.host.settings.lock().enabled = false;
    rig.toggle();
    rig.wait_idle().await;
    assert_eq!(refused(&rig), "Dictation is turned off in Settings");

    rig.host.settings.lock().enabled = true;
    rig.host.settings.lock().speech_model = String::new();
    rig.toggle();
    rig.wait_idle().await;
    assert_eq!(refused(&rig), "Choose a speech model in Settings");

    // Réglé mais pas téléchargé.
    rig.host.settings.lock().speech_model = "whisper-base".into();
    rig.toggle();
    rig.wait_idle().await;
    assert_eq!(refused(&rig), "Speech model not installed");

    assert_eq!(rig.microphone.opens.load(Ordering::SeqCst), 0);
    assert!(rig.clipboard.copied.lock().is_empty());
}

/// Moteur que la machine ne peut pas faire tourner (Whisper sans AVX2,
/// Parakeet sans ONNX Runtime) : refusé avant d'ouvrir le micro, avec le
/// refus de `support`, et le modèle jamais chargé.
#[tokio::test]
async fn an_engine_this_machine_cannot_run_is_refused_before_listening() {
    let rig = rig_on("", |engine| {
        assert_eq!(engine, SpeechEngine::Whisper);
        Err("This processor cannot run Whisper models".into())
    })
    .await;
    rig.toggle();
    rig.wait_idle().await;
    assert_eq!(
        rig.last_result().unwrap().error.as_deref(),
        Some("This processor cannot run Whisper models")
    );
    assert_eq!(rig.microphone.opens.load(Ordering::SeqCst), 0);
    assert_eq!(rig.model.loads_started.load(Ordering::SeqCst), 0);
    assert!(rig.clipboard.copied.lock().is_empty());
}

#[tokio::test]
async fn microphone_errors_are_reported() {
    let rig = rig("").await;
    *rig.microphone.captured.lock() = Err(audio::no_microphone());
    rig.toggle();
    rig.wait_idle().await;
    assert_eq!(
        rig.last_result().unwrap().error.as_deref(),
        Some(audio::no_microphone().as_str())
    );
    assert_eq!(
        rig.last_notice(),
        Notice::last(Title::Failed, audio::no_microphone())
    );
}

#[tokio::test]
async fn a_short_press_is_dropped_silently() {
    let rig = rig("").await;
    *rig.microphone.captured.lock() = Ok(speech(0.2));
    rig.dictate().await;
    assert_eq!(rig.summaries(), [LISTENING, "-"]);
    assert_eq!(rig.last_result(), None);
    assert!(rig.clipboard.copied.lock().is_empty());
    assert_eq!(rig.model.transcriptions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn silence_is_not_transcribed() {
    let rig = rig("").await;
    *rig.microphone.captured.lock() = Ok(Captured {
        samples: vec![0.0; 32_000],
        rate: 16_000,
    });
    rig.dictate().await;
    assert_eq!(
        rig.last_result().unwrap().error.as_deref(),
        Some("No speech detected")
    );
    assert_eq!(rig.model.transcriptions.load(Ordering::SeqCst), 0);
    // Whisper n'a rien entendu : même message.
    *rig.microphone.captured.lock() = Ok(speech(1.0));
    *rig.model.text.lock() = "  ".into();
    rig.dictate().await;
    assert_eq!(
        rig.last_result().unwrap().error.as_deref(),
        Some("No speech detected")
    );
}

#[tokio::test]
async fn cancel_drops_the_recording() {
    let rig = rig("").await;
    rig.toggle();
    rig.wait_listening().await;
    rig.core.cancel();
    assert_eq!(rig.phase(), DictationPhase::Idle);
    assert_eq!(rig.summaries(), [LISTENING, "-"]);
    // Rien ne suit.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.last_result(), None);
    assert!(rig.clipboard.copied.lock().is_empty());
    assert!(!rig.core.cancel(), "nothing left to cancel");
}

#[tokio::test]
async fn cancel_during_transcription_discards_the_result() {
    let rig = rig("").await;
    let (go, gate) = std::sync::mpsc::channel();
    *rig.model.gate.lock() = Some(gate);
    rig.model.gated.store(true, Ordering::SeqCst);
    rig.toggle();
    rig.wait_listening().await;
    rig.toggle();
    rig.wait("transcribing", |rig| {
        rig.model.transcriptions.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(rig.phase(), DictationPhase::Transcribing);
    // Pendant le traitement, toggle et start sont ignorés.
    rig.toggle();
    rig.core.start(rig.host());
    rig.core.stop();
    assert_eq!(rig.phase(), DictationPhase::Transcribing);
    assert_eq!(rig.microphone.opens.load(Ordering::SeqCst), 1);

    rig.core.cancel();
    assert_eq!(rig.phase(), DictationPhase::Idle);
    go.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(rig.clipboard.copied.lock().is_empty());
    assert_eq!(rig.last_result(), None);
    assert_eq!(rig.summaries(), [LISTENING, TRANSCRIBING, "-"]);

    // La dictée suivante se passe normalement.
    rig.model.gated.store(false, Ordering::SeqCst);
    rig.dictate().await;
    assert_eq!(*rig.clipboard.copied.lock(), [SPOKEN]);
}

/// Abandon pendant que la tâche ne peut pas être interrompue (copie qui
/// bloque sans rendre la main) : sa fin ne touche plus à rien.
#[tokio::test]
async fn a_task_cancelled_too_late_to_stop_changes_nothing() {
    let rig = rig("").await;
    let (go, gate) = std::sync::mpsc::channel();
    *rig.clipboard.gate.lock() = Some(gate);
    rig.toggle();
    rig.wait_listening().await;
    rig.toggle();
    rig.wait("copying", |rig| {
        rig.clipboard.entered.load(Ordering::SeqCst)
    })
    .await;
    rig.core.cancel();
    // Une nouvelle dictée commence aussitôt.
    rig.toggle();
    rig.wait_listening().await;
    go.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.last_result(), None);
    assert_eq!(rig.phase(), DictationPhase::Recording);
    assert_eq!(rig.core.inner.lock().stage, Stage::Recording);
    assert_eq!(rig.summaries(), [LISTENING, TRANSCRIBING, "-", LISTENING]);
    // La nouvelle dictée finit normalement.
    rig.toggle();
    rig.wait_idle().await;
    assert_eq!(rig.last_result().unwrap().text, SPOKEN);
}

/// Fermeture de l'app pendant le chargement anticipé du modèle : elle
/// attend sa fin (sinon whisper.cpp plante à la sortie), et plus aucune
/// dictée ne commence ensuite.
#[tokio::test]
async fn shutdown_waits_for_the_model_and_nothing_starts_after() {
    let rig = rig("").await;
    *rig.model.load_delay.lock() = Duration::from_millis(400);
    rig.toggle();
    rig.wait_listening().await;
    rig.wait("loading", |rig| {
        rig.model.loads_started.load(Ordering::SeqCst) == 1
    })
    .await;
    rig.core.shutdown(WAIT).await;
    assert_eq!(rig.model.loads_finished.load(Ordering::SeqCst), 1);
    assert_eq!(rig.phase(), DictationPhase::Idle);
    assert_eq!(rig.summaries(), [LISTENING, "-"]);

    rig.toggle();
    rig.core.start(rig.host());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.phase(), DictationPhase::Idle);
    assert_eq!(rig.microphone.opens.load(Ordering::SeqCst), 1);
    assert_eq!(rig.model.loads_started.load(Ordering::SeqCst), 1);
}

/// Abandon pendant la transcription d'une longue dictée : elle s'arrête au
/// morceau suivant au lieu d'aller au bout (le moteur se libère).
#[tokio::test]
async fn cancel_stops_a_long_transcription_between_chunks() {
    let rig = rig("").await;
    // 100 s à 16 kHz (pas de rééchantillonnage) : 3 morceaux.
    *rig.microphone.captured.lock() = Ok(speech_at(100.0, 16_000));
    let (go, gate) = std::sync::mpsc::channel();
    *rig.model.gate.lock() = Some(gate);
    rig.model.gated.store(true, Ordering::SeqCst);
    rig.toggle();
    rig.wait_listening().await;
    rig.toggle();
    rig.wait("first chunk", |rig| {
        rig.model.transcriptions.load(Ordering::SeqCst) == 1
    })
    .await;
    rig.core.cancel();
    go.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(rig.model.transcriptions.load(Ordering::SeqCst), 1);
    assert!(rig.clipboard.copied.lock().is_empty());

    // Sans abandon, les trois morceaux sont transcrits et joints.
    rig.model.gated.store(false, Ordering::SeqCst);
    rig.dictate().await;
    assert_eq!(rig.model.transcriptions.load(Ordering::SeqCst), 4);
    assert_eq!(
        *rig.clipboard.copied.lock(),
        [format!("{SPOKEN} {SPOKEN} {SPOKEN}")]
    );
}

#[tokio::test]
async fn a_stop_before_the_microphone_opens_is_kept() {
    let rig = rig("").await;
    // Bascule deux fois de suite : l'arrêt attend l'ouverture du micro.
    rig.toggle();
    rig.toggle();
    rig.wait_idle().await;
    assert_eq!(*rig.clipboard.copied.lock(), [SPOKEN]);
    assert_eq!(rig.microphone.opens.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_recording_stops_by_itself_at_the_limit() {
    let mut rig = rig("").await;
    let core = Arc::get_mut(&mut rig.core).unwrap();
    core.services.max_recording = Duration::from_millis(200);
    rig.toggle();
    rig.wait_idle().await;
    assert_eq!(*rig.clipboard.copied.lock(), [SPOKEN]);
}

#[test]
fn listening_tells_how_to_stop() {
    assert_eq!(
        listening_body(DictationMode::Toggle, Some("Ctrl+Alt+D")),
        "Press Ctrl+Alt+D again to stop"
    );
    assert_eq!(
        listening_body(DictationMode::Hold, Some("Ctrl+Alt+D")),
        "Release Ctrl+Alt+D to stop"
    );
    assert_eq!(
        listening_body(DictationMode::Hold, None),
        "Run the shortcut again to stop"
    );
}

/// Séance du mode vocal : la dictée refuse de démarrer (avis d'échec), ne
/// libère pas le modèle qu'elle partage, puis redémarre à la fin de la
/// séance ; une dictée en cours empêche la réservation.
#[tokio::test]
async fn the_voice_mode_holds_the_dictation_back() {
    let rig = rig("").await;
    assert!(rig.core.reserve());
    rig.toggle();
    rig.wait("the refusal", |rig| rig.last_result().is_some())
        .await;
    assert_eq!(rig.phase(), DictationPhase::Idle);
    assert_eq!(rig.microphone.opens.load(Ordering::SeqCst), 0);
    let result = rig.last_result().unwrap();
    assert_eq!(
        result.error.as_deref(),
        Some("Voice mode is on: end it to dictate")
    );
    assert!(result.text.is_empty() && !result.copied);
    assert_eq!(
        rig.last_notice(),
        Notice::last(Title::Failed, "Voice mode is on: end it to dictate".into())
    );
    assert_eq!(*rig.host.phases.lock(), [DictationPhase::Idle]);

    rig.core.release();
    rig.dictate().await;
    assert_eq!(rig.last_result().unwrap().raw, SPOKEN);
    rig.toggle();
    rig.wait_listening().await;
    assert!(!rig.core.reserve());
    rig.toggle();
    rig.wait_idle().await;
}

/// Connexion du pont simulée : ce que la dictée lui dit.
struct FakeLink {
    connection: u64,
    glossary: Vec<String>,
    alive: std::sync::atomic::AtomicBool,
    phases: Mutex<Vec<DictationPhase>>,
    ends: Mutex<Vec<EditorEnd>>,
}

impl FakeLink {
    fn new(connection: u64, glossary: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            connection,
            glossary: glossary.iter().map(|word| (*word).to_owned()).collect(),
            alive: std::sync::atomic::AtomicBool::new(true),
            phases: Mutex::new(Vec::new()),
            ends: Mutex::new(Vec::new()),
        })
    }
}

impl DictationLink for FakeLink {
    fn connection(&self) -> u64 {
        self.connection
    }
    fn glossary(&self) -> Vec<String> {
        self.glossary.clone()
    }
    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
    fn phase(&self, phase: DictationPhase) {
        self.phases.lock().push(phase);
    }
    fn ended(&self, end: EditorEnd) {
        self.ends.lock().push(end);
    }
}

impl Rig {
    fn start_editor(&self, link: &Arc<FakeLink>) -> Result<Opened, EditorStartError> {
        self.core.start_editor(self.host(), link.clone())
    }

    /// Dictée de l'éditeur, micro ouvert.
    async fn editor_listening(&self, link: &Arc<FakeLink>) {
        let opened = self.start_editor(link).expect("started");
        let answer = tokio::time::timeout(WAIT, opened).await.expect("in time");
        assert_eq!(answer, Ok(Ok(())));
        assert_eq!(*link.phases.lock(), [DictationPhase::Recording]);
    }

    fn closes(&self) -> usize {
        self.microphone.closes.load(Ordering::SeqCst)
    }
}

/// Lot 4b : dictée de l'éditeur, texte rendu à lui seul (ni presse-papiers,
/// ni collage, ni notification, ni « Last dictation ») ; glossaire de la
/// séance ajouté au dictionnaire pour Whisper et le nettoyage, sans toucher
/// aux réglages ; l'interrupteur de la dictée (raccourci) ne compte pas.
#[tokio::test]
async fn an_editor_dictation_gives_its_text_to_the_editor_only() {
    let rig = rig("chatonlm-polish").await;
    {
        let mut settings = rig.host.settings.lock();
        settings.dictionary = vec!["ChatonLM".into()];
        settings.auto_paste = true;
        settings.enabled = false;
    }
    let before = rig.host.settings.lock().clone();
    let link = FakeLink::new(7, &["GpuChief", "chatonlm", "use_dictation"]);
    rig.editor_listening(&link).await;
    assert!(rig.core.inner.lock().editor.is_some());
    // Le raccourci, la zone de notification et la page ne l'arrêtent pas.
    rig.toggle();
    rig.core.stop();
    rig.core.toggle_from(rig.host(), Origin::Page);
    assert!(!rig.core.cancel());
    assert!(!rig.core.stop_editor(8), "another connection");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.phase(), DictationPhase::Recording);
    assert!(!rig.core.stop_editor(7));
    rig.wait_idle().await;

    let polished = format!("Polished: {SPOKEN}");
    assert_eq!(*link.ends.lock(), [EditorEnd::Text(polished)]);
    assert_eq!(
        *link.phases.lock(),
        [
            DictationPhase::Recording,
            DictationPhase::Transcribing,
            DictationPhase::Polishing,
            DictationPhase::Idle,
        ]
    );
    assert!(rig.clipboard.copied.lock().is_empty(), "never copied");
    assert!(rig.clipboard.pastes.lock().is_empty(), "never pasted");
    assert!(rig.shown().is_empty(), "no desktop notification");
    assert_eq!(rig.last_result(), None);
    assert!(rig.core.inner.lock().editor.is_none());
    assert_eq!(rig.closes(), 1, "microphone closed");
    // Glossaire : dictionnaire d'abord, puis, à part (Whisper leur garde
    // une part de sa consigne), les mots de la séance qu'il n'a pas.
    let request = rig.model.requests.lock()[0].clone();
    assert_eq!(request.glossary, vec!["ChatonLM".to_owned()]);
    assert_eq!(
        request.project,
        vec!["GpuChief".to_owned(), "use_dictation".to_owned()]
    );
    let request = rig.server.chat_requests().pop().unwrap();
    let system = request["messages"][0]["content"].as_str().unwrap();
    assert!(
        system.contains("Spell these words exactly as written: ChatonLM, GpuChief, use_dictation."),
        "{system}"
    );
    assert_eq!(*rig.host.settings.lock(), before, "settings untouched");

    // La dictée suivante (raccourci) a le seul dictionnaire, et colle.
    rig.host.settings.lock().enabled = true;
    rig.dictate().await;
    assert_eq!(
        rig.model.requests.lock()[1].glossary,
        vec!["ChatonLM".to_owned()]
    );
    assert!(rig.model.requests.lock()[1].project.is_empty());
    assert_eq!(rig.clipboard.pastes.lock().len(), 1);
}

/// Une seule dictée dans l'app : l'éditeur est refusé si l'app (raccourci,
/// page), le mode vocal ou une autre connexion l'occupe ; une dictée
/// d'éditeur empêche le mode vocal.
#[tokio::test]
async fn an_editor_dictation_is_refused_while_the_dictation_is_busy() {
    let rig = rig("").await;
    let link = FakeLink::new(1, &[]);
    rig.toggle();
    rig.wait_listening().await;
    assert_eq!(
        rig.start_editor(&link).err(),
        Some(EditorStartError::Busy(DictationOwner::App))
    );
    rig.toggle();
    rig.wait_idle().await;
    assert!(rig.core.reserve());
    assert_eq!(
        rig.start_editor(&link).err(),
        Some(EditorStartError::Busy(DictationOwner::VoiceMode))
    );
    rig.core.release();
    rig.editor_listening(&link).await;
    let other = FakeLink::new(2, &[]);
    assert_eq!(
        rig.start_editor(&other).err(),
        Some(EditorStartError::Busy(DictationOwner::Bridge))
    );
    assert_eq!(
        rig.start_editor(&link).err(),
        Some(EditorStartError::Busy(DictationOwner::Bridge))
    );
    assert!(!rig.core.reserve(), "no voice mode meanwhile");
    assert_eq!(rig.microphone.opens.load(Ordering::SeqCst), 2);
    assert!(other.phases.lock().is_empty());
    // Une connexion déjà fermée ne démarre rien.
    rig.core.cancel_editor(Some(1));
    let closed = FakeLink::new(3, &[]);
    closed.alive.store(false, Ordering::SeqCst);
    assert_eq!(
        rig.start_editor(&closed).err(),
        Some(EditorStartError::Cancelled)
    );
    assert_eq!(rig.phase(), DictationPhase::Idle);
}

/// Le micro de l'éditeur se ferme quand sa connexion s'en va (abandon de sa
/// dictée, pas d'une autre), au bouton du bandeau, à la fermeture de l'app ;
/// rien ne lui est rendu, il apprend `idle`.
#[tokio::test]
async fn the_microphone_of_an_editor_closes_when_it_goes() {
    let rig = rig("").await;
    let link = FakeLink::new(4, &[]);
    rig.editor_listening(&link).await;
    assert!(!rig.core.cancel_editor(Some(5)), "another connection");
    assert_eq!(rig.closes(), 0);
    assert!(rig.core.cancel_editor(Some(4)));
    rig.wait("microphone closed", |rig| rig.closes() == 1).await;
    assert_eq!(
        *link.phases.lock(),
        [DictationPhase::Recording, DictationPhase::Idle]
    );
    assert!(link.ends.lock().is_empty());
    assert_eq!(rig.phase(), DictationPhase::Idle);

    // Bandeau de la page.
    let link = FakeLink::new(6, &[]);
    rig.editor_listening(&link).await;
    assert!(rig.core.cancel_editor(None));
    rig.wait("microphone closed", |rig| rig.closes() == 2).await;
    assert!(link.ends.lock().is_empty());

    // Fermeture de l'app.
    let link = FakeLink::new(7, &[]);
    rig.editor_listening(&link).await;
    rig.core.shutdown(WAIT).await;
    rig.wait("microphone closed", |rig| rig.closes() == 3).await;
    assert_eq!(link.phases.lock().last(), Some(&DictationPhase::Idle));
    assert!(rig.clipboard.copied.lock().is_empty());
}

/// Arrêt pendant le démarrage (micro qui s'ouvre) : le démarrage répond
/// « arrêtée », le micro ouvert ensuite se referme aussitôt, l'éditeur
/// n'apprend aucune phase. Même chose si sa connexion se ferme avant.
#[tokio::test]
async fn a_stop_while_the_editor_microphone_opens_closes_it() {
    let rig = rig("").await;
    for stop_by_request in [true, false] {
        let (go, gate) = std::sync::mpsc::channel();
        *rig.microphone.gate.lock() = Some(gate);
        let link = FakeLink::new(9, &[]);
        let opening = rig.microphone.opening.load(Ordering::SeqCst);
        let opens = rig.microphone.opens.load(Ordering::SeqCst);
        let closes = rig.closes();
        let opened = rig.start_editor(&link).unwrap();
        rig.wait("the microphone opening", |rig| {
            rig.microphone.opening.load(Ordering::SeqCst) == opening + 1
        })
        .await;
        if stop_by_request {
            assert!(rig.core.stop_editor(9), "abandoned before the microphone");
        } else {
            link.alive.store(false, Ordering::SeqCst);
        }
        go.send(()).unwrap();
        let answer = tokio::time::timeout(WAIT, opened).await.expect("in time");
        assert!(
            matches!(answer, Ok(Err(EditorStartError::Cancelled)) | Err(_)),
            "{answer:?}"
        );
        rig.wait("microphone opened then closed", |rig| {
            rig.microphone.opens.load(Ordering::SeqCst) == opens + 1 && rig.closes() == closes + 1
        })
        .await;
        rig.wait_idle().await;
        assert!(link.phases.lock().is_empty(), "{:?}", link.phases.lock());
        assert!(link.ends.lock().is_empty());
    }
}

/// Refus avant d'écouter, rendus au démarrage : modèle absent (même s'il
/// est réglé), micro ; un échec après l'ouverture va à l'éditeur.
#[tokio::test]
async fn an_editor_start_says_why_it_cannot_listen() {
    let rig = rig("").await;
    rig.host.settings.lock().speech_model = "whisper-base".into();
    let link = FakeLink::new(1, &[]);
    let opened = rig.start_editor(&link).unwrap();
    assert_eq!(opened.await, Ok(Err(EditorStartError::SpeechModelMissing)));
    rig.wait_idle().await;
    // Rien de réglé : le premier installé (whisper-tiny).
    rig.host.settings.lock().speech_model = String::new();
    *rig.microphone.captured.lock() = Err("No microphone".into());
    let opened = rig.start_editor(&link).unwrap();
    assert_eq!(
        opened.await,
        Ok(Err(EditorStartError::Failed("No microphone".into())))
    );
    rig.wait_idle().await;
    assert!(link.phases.lock().is_empty());
    assert!(rig.shown().is_empty());
    // Silence : dit à l'éditeur, après l'ouverture du micro.
    *rig.microphone.captured.lock() = Ok(Captured {
        samples: vec![0.0; 48_000],
        rate: 48_000,
    });
    rig.editor_listening(&link).await;
    rig.core.stop_editor(1);
    rig.wait_idle().await;
    assert_eq!(
        *link.ends.lock(),
        [EditorEnd::Failed("No speech detected".into())]
    );
    assert_eq!(link.phases.lock().last(), Some(&DictationPhase::Idle));
    assert_eq!(rig.last_result(), None);
}
