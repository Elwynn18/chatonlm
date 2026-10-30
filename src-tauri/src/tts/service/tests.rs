//! Essais du service de lecture, avec un faux moteur et une fausse sortie.

use super::*;
use crate::chat::ChatErrorKind;
use crate::store::chats::{Message, Role};

#[derive(Default)]
struct Log {
    spoken: Mutex<Vec<String>>,
    /// Drapeau « premier son » de chaque appel de `speak`.
    firsts: Mutex<Vec<bool>>,
    loads: AtomicUsize,
    output_stops: AtomicUsize,
    pushed: AtomicUsize,
    states: Mutex<Vec<ReadAloudState>>,
    /// Langue de chaque moteur chargé.
    languages: Mutex<Vec<VoiceLanguage>>,
    /// Événements des lectures vocales reçus par l'écouteur.
    readings: Mutex<Vec<(String, VoiceReading)>>,
    /// Échantillons « joués » par la sortie, réglés par l'essai ; `None` :
    /// tout ce qui est poussé est aussitôt joué.
    clock: Mutex<Option<u64>>,
}

struct FakeSynth {
    log: Arc<Log>,
    /// Vrai : `speak` attend l'arrêt (lecture longue).
    hold: Arc<AtomicBool>,
    /// Vrai : `speak` échoue.
    broken: Arc<AtomicBool>,
}

impl Synth for FakeSynth {
    fn speak(
        &mut self,
        text: &str,
        first: bool,
        stop: &AtomicBool,
        sink: &mut dyn FnMut(&[f32]),
    ) -> Result<(), String> {
        if self.broken.load(Ordering::Relaxed) {
            return Err("broken".into());
        }
        if text == "Panic." {
            panic!("panic in the engine");
        }
        self.log.spoken.lock().push(text.to_owned());
        self.log.firsts.lock().push(first);
        sink(&[0.0; 10]);
        while self.hold.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
}

struct FakeOutput {
    log: Arc<Log>,
    stopped: AtomicBool,
}

impl Output for FakeOutput {
    fn push(&self, samples: &[f32]) {
        self.log.pushed.fetch_add(samples.len(), Ordering::Relaxed);
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.log.output_stops.fetch_add(1, Ordering::Relaxed);
    }

    /// Horloge réglée par l'essai : attend que tout soit « joué » (ou
    /// l'arrêt).
    fn drain(&self) -> bool {
        while self.played() < self.queued_end() {
            if self.stopped.load(Ordering::Relaxed) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        !self.stopped.load(Ordering::Relaxed)
    }

    fn played(&self) -> u64 {
        (*self.log.clock.lock()).unwrap_or_else(|| self.queued_end())
    }

    fn queued_end(&self) -> u64 {
        self.log.pushed.load(Ordering::Relaxed) as u64
    }
}

struct Rig {
    read_aloud: ReadAloud,
    log: Arc<Log>,
    installed: Arc<AtomicBool>,
    runtime: Arc<AtomicBool>,
    hold: Arc<AtomicBool>,
    broken: Arc<AtomicBool>,
    /// Durée d'un chargement de moteur.
    load_delay: Arc<Mutex<Duration>>,
    load_error: Arc<Mutex<Option<ReadAloudError>>>,
    open_error: Arc<Mutex<Option<ReadAloudError>>>,
    queued: Arc<Mutex<Vec<String>>>,
    /// Voix déjà en téléchargement (vue par `read`).
    downloading: Arc<AtomicBool>,
}

fn voice_id(language: VoiceLanguage) -> &'static str {
    match language {
        VoiceLanguage::En => "pocket-tts-en",
        VoiceLanguage::Es => "pocket-tts-es",
        VoiceLanguage::Fr => "pocket-tts-fr",
    }
}

fn rig_with_idle(idle: Duration) -> Rig {
    let log = Arc::new(Log::default());
    let installed = Arc::new(AtomicBool::new(true));
    let runtime = Arc::new(AtomicBool::new(true));
    let hold = Arc::new(AtomicBool::new(false));
    let broken = Arc::new(AtomicBool::new(false));
    let load_delay = Arc::new(Mutex::new(Duration::ZERO));
    let load_error = Arc::new(Mutex::new(None));
    let open_error = Arc::new(Mutex::new(None));
    let read_aloud = ReadAloud::with_idle(idle);
    let backend = {
        let (log, installed, hold, broken) =
            (log.clone(), installed.clone(), hold.clone(), broken.clone());
        let (load_error, open_error, load_delay) =
            (load_error.clone(), open_error.clone(), load_delay.clone());
        let output_log = log.clone();
        let runtime = runtime.clone();
        Backend {
            voice: Box::new(move |language| {
                Some(Voice {
                    id: voice_id(language),
                    bytes: 1_000,
                    dir: PathBuf::new(),
                    installed: installed.load(Ordering::Relaxed),
                })
            }),
            load: Box::new(move |_, language| {
                std::thread::sleep(*load_delay.lock());
                log.languages.lock().push(language);
                log.loads.fetch_add(1, Ordering::Relaxed);
                match *load_error.lock() {
                    Some(error) => Err(error),
                    None => Ok(Box::new(FakeSynth {
                        log: log.clone(),
                        hold: hold.clone(),
                        broken: broken.clone(),
                    }) as Box<dyn Synth>),
                }
            }),
            open: Box::new(move || match *open_error.lock() {
                Some(error) => Err(error),
                None => Ok(Arc::new(FakeOutput {
                    log: output_log.clone(),
                    stopped: AtomicBool::new(false),
                }) as Arc<dyn Output>),
            }),
            interface: Box::new(|| VoiceLanguage::En),
            runtime: Box::new(move || runtime.load(Ordering::Relaxed)),
        }
    };
    let states = log.clone();
    read_aloud.configure(backend, move |state| {
        states.states.lock().push(state.clone())
    });
    let readings = log.clone();
    read_aloud.set_voice_listener(move |chat, reading| {
        readings.readings.lock().push((chat.to_owned(), reading));
    });
    Rig {
        read_aloud,
        log,
        installed,
        runtime,
        hold,
        broken,
        load_delay,
        load_error,
        open_error,
        queued: Arc::new(Mutex::new(Vec::new())),
        downloading: Arc::new(AtomicBool::new(false)),
    }
}

fn rig() -> Rig {
    rig_with_idle(IDLE)
}

const CHAT: &str = "chat-a";

fn message(message_id: i64) -> ReadAloudSource {
    ReadAloudSource::Message { message_id }
}

/// Texte d'un cours.
fn passage(course: &str, passage: &str) -> ReadAloudSource {
    ReadAloudSource::Study {
        course_id: course.into(),
        passage: passage.into(),
    }
}

/// Réponse que nomme l'état (`None` : aucune, ou un texte d'un cours).
fn message_of(state: &ReadAloudState) -> Option<i64> {
    match state.source {
        Some(ReadAloudSource::Message { message_id }) => Some(message_id),
        _ => None,
    }
}

impl Rig {
    fn read(&self, message_id: i64, markdown: &str, download: bool) -> ReadAloudOutcome {
        self.read_from(message(message_id), markdown, None, download)
    }

    /// Lit `source` ; `fallback` : langue de repli du texte.
    fn read_from(
        &self,
        source: ReadAloudSource,
        markdown: &str,
        fallback: Option<VoiceLanguage>,
        download: bool,
    ) -> ReadAloudOutcome {
        let queued = self.queued.clone();
        let downloading = self.downloading.clone();
        self.read_aloud
            .read(
                source,
                markdown,
                fallback,
                download,
                &move |_| downloading.load(Ordering::Relaxed),
                &move |id| {
                    queued.lock().push(id.to_owned());
                    Ok(())
                },
            )
            .unwrap()
    }

    fn observe(&self, event: ChatEvent) {
        self.read_aloud.observe(CHAT, &event, None);
    }

    fn spoken(&self) -> Vec<String> {
        self.log.spoken.lock().clone()
    }

    fn phases(&self) -> Vec<(Option<i64>, ReadAloudPhase)> {
        self.log
            .states
            .lock()
            .iter()
            .map(|state| (message_of(state), state.phase))
            .collect()
    }

    fn loads(&self) -> usize {
        self.log.loads.load(Ordering::Relaxed)
    }
}

/// Attend `condition` (5 s au plus).
fn wait(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn idle(rig: &Rig) -> bool {
    rig.read_aloud.state().phase == ReadAloudPhase::Idle
}

fn playing(rig: &Rig) -> bool {
    rig.read_aloud.state().phase == ReadAloudPhase::Playing
}

fn user_message() -> ChatEvent {
    ChatEvent::UserMessage {
        message: Message {
            id: 1,
            role: Role::User,
            content: "question".into(),
            thinking: String::new(),
            model: None,
            created_at: 0,
            updated_at: 0,
            thinking_started_at: None,
            thinking_ended_at: None,
            attachments: Vec::new(),
            tool_calls: Vec::new(),
        },
    }
}

fn reply_started(message_id: i64) -> ChatEvent {
    ChatEvent::ReplyStarted {
        message_id,
        created_at: 0,
    }
}

fn content(text: &str) -> ChatEvent {
    ChatEvent::Content { text: text.into() }
}

fn done(message_id: i64) -> ChatEvent {
    ChatEvent::Done {
        message_id: Some(message_id),
    }
}

fn download(state: SpeechDownloadState, completed: u64) -> SpeechDownload {
    SpeechDownload {
        id: "pocket-tts-en".into(),
        name: "Pocket TTS".into(),
        state,
        completed_bytes: completed,
        total_bytes: 1_000,
        error: None,
    }
}

#[test]
fn an_empty_answer_has_nothing_to_read() {
    let rig = rig();
    assert_eq!(rig.read(7, "   ", false), ReadAloudOutcome::NothingToRead);
    assert!(rig.spoken().is_empty());
    assert!(rig.log.states.lock().is_empty());
}

#[test]
fn a_missing_voice_asks_before_downloading() {
    let rig = rig();
    rig.installed.store(false, Ordering::Relaxed);
    assert_eq!(
        rig.read(7, "Hello.", false),
        ReadAloudOutcome::NeedsVoice {
            model_id: "pocket-tts-en".into(),
            language: VoiceLanguage::En,
            bytes: 1_000,
        }
    );
    assert!(rig.queued.lock().is_empty());
    assert!(rig.log.states.lock().is_empty());
    assert_eq!(rig.loads(), 0);
}

#[test]
fn a_voice_already_downloading_is_waited_for_without_asking() {
    let rig = rig();
    rig.installed.store(false, Ordering::Relaxed);
    rig.downloading.store(true, Ordering::Relaxed);
    assert_eq!(rig.read(7, "Hello.", false), ReadAloudOutcome::Started);
    // Pas remise en file : la lecture attend le téléchargement en cours.
    assert!(rig.queued.lock().is_empty());
    assert_eq!(rig.read_aloud.state().phase, ReadAloudPhase::Downloading);
    rig.installed.store(true, Ordering::Relaxed);
    rig.read_aloud
        .on_downloads(&[download(SpeechDownloadState::Done, 1_000)]);
    wait("the reading", || rig.spoken().len() == 1 && idle(&rig));
}

#[test]
fn without_onnx_runtime_nothing_is_downloaded() {
    let rig = rig();
    rig.installed.store(false, Ordering::Relaxed);
    rig.runtime.store(false, Ordering::Relaxed);
    assert_eq!(rig.read(7, "Hello.", true), ReadAloudOutcome::Started);
    assert!(rig.queued.lock().is_empty());
    let state = rig.read_aloud.state();
    assert_eq!(
        (message_of(&state), state.phase, state.error),
        (
            Some(7),
            ReadAloudPhase::Idle,
            Some(ReadAloudError::RuntimeUnavailable)
        )
    );
}

#[test]
fn an_answer_is_read_then_the_state_goes_back_to_idle() {
    let rig = rig();
    assert_eq!(
        rig.read(7, "Hello there.", false),
        ReadAloudOutcome::Started
    );
    wait("the end of the reading", || {
        idle(&rig) && rig.phases().len() >= 3
    });
    assert_eq!(rig.spoken(), ["Hello there."]);
    assert_eq!(
        rig.phases(),
        [
            (Some(7), ReadAloudPhase::Preparing),
            (Some(7), ReadAloudPhase::Playing),
            (None, ReadAloudPhase::Idle),
        ]
    );
    assert_eq!(rig.log.pushed.load(Ordering::Relaxed), 10);
    assert_eq!(rig.read_aloud.state().error, None);
}

#[test]
fn the_engine_is_kept_between_two_readings() {
    let rig = rig();
    rig.read(1, "One.", false);
    wait("the first reading", || {
        rig.spoken().len() == 1 && idle(&rig)
    });
    rig.read(2, "Two.", false);
    wait("the second reading", || {
        rig.spoken().len() == 2 && idle(&rig)
    });
    assert_eq!(rig.loads(), 1);
}

#[test]
fn a_new_reading_waits_for_the_engine_of_the_stopping_one() {
    let rig = rig();
    rig.hold.store(true, Ordering::Relaxed);
    rig.read(1, "First.", false);
    wait("the first reading", || playing(&rig));
    rig.read(2, "Second.", false);
    wait("the second reading", || {
        message_of(&rig.read_aloud.state()) == Some(2) && playing(&rig)
    });
    // Le moteur de la première lecture est repris, pas rechargé.
    assert_eq!(rig.loads(), 1);
    rig.read_aloud.stop();
}

#[test]
fn an_idle_engine_is_released() {
    let rig = rig_with_idle(Duration::from_millis(30));
    rig.read(1, "One.", false);
    wait("the first reading", || {
        rig.spoken().len() == 1 && idle(&rig)
    });
    wait("the release of the engine", || {
        rig.read_aloud.inner.engine.lock().loaded.is_none()
    });
    rig.read(2, "Two.", false);
    wait("the second reading", || {
        rig.spoken().len() == 2 && idle(&rig)
    });
    assert_eq!(rig.loads(), 2);
}

#[test]
fn a_broken_engine_is_not_kept() {
    let rig = rig();
    rig.broken.store(true, Ordering::Relaxed);
    rig.read(1, "One.", false);
    wait("the failure", || {
        idle(&rig) && rig.read_aloud.state().error.is_some()
    });
    assert_eq!(
        rig.read_aloud.state().error,
        Some(ReadAloudError::EngineFailed)
    );
    rig.broken.store(false, Ordering::Relaxed);
    rig.read(2, "Two.", false);
    wait("the second reading", || {
        rig.spoken().len() == 1 && idle(&rig)
    });
    assert_eq!(rig.loads(), 2);
}

#[test]
fn an_engine_taken_before_a_voice_is_deleted_is_not_kept() {
    let rig = rig();
    rig.hold.store(true, Ordering::Relaxed);
    rig.read(1, "One.", false);
    wait("the reading", || playing(&rig));
    rig.read_aloud.release_voice(VoiceLanguage::En);
    assert!(idle(&rig));
    wait("the engine given back", || {
        !rig.read_aloud.inner.engine.lock().busy
    });
    assert!(rig.read_aloud.inner.engine.lock().loaded.is_none());
}

#[test]
fn nothing_is_kept_or_loaded_after_shutdown() {
    let rig = rig();
    rig.hold.store(true, Ordering::Relaxed);
    rig.read(1, "One.", false);
    wait("the reading", || playing(&rig));
    rig.read_aloud.shutdown();
    assert_eq!(rig.read_aloud.inner.workers.load(Ordering::Acquire), 0);
    assert!(rig.read_aloud.inner.engine.lock().loaded.is_none());
    rig.hold.store(false, Ordering::Relaxed);
    rig.read(2, "Two.", false);
    wait("the refusal", || idle(&rig));
    assert_eq!(rig.loads(), 1);
}

#[test]
fn stop_silences_at_once() {
    let rig = rig();
    rig.hold.store(true, Ordering::Relaxed);
    rig.read(7, "A long answer.", false);
    wait("the reading", || playing(&rig));
    assert_eq!(rig.read_aloud.stop(), None);
    assert!(idle(&rig));
    assert_eq!(message_of(&rig.read_aloud.state()), None);
    assert!(rig.log.output_stops.load(Ordering::Relaxed) >= 1);
}

#[test]
fn a_new_reading_replaces_the_current_one() {
    let rig = rig();
    rig.hold.store(true, Ordering::Relaxed);
    rig.read(1, "First.", false);
    wait("the first reading", || playing(&rig));
    rig.read(2, "Second.", false);
    wait("the second reading", || {
        message_of(&rig.read_aloud.state()) == Some(2) && playing(&rig)
    });
    // La fin de la première lecture ne remet pas l'état au repos.
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(message_of(&rig.read_aloud.state()), Some(2));
    rig.read_aloud.stop();
}

/// Texte d'un cours : même moteur, même lecture unique ; l'état nomme le
/// texte lu (seul son bouton montre « Stop »), et la lecture s'arrête si son
/// cours est supprimé ou refait, pas celle d'un autre cours ni d'une réponse.
#[test]
fn a_course_text_is_read_and_named_by_the_state() {
    let rig = rig();
    rig.hold.store(true, Ordering::Relaxed);
    let lesson = passage("course-a", "L0:s1");
    assert_eq!(
        rig.read_from(lesson.clone(), "A lesson.", None, false),
        ReadAloudOutcome::Started
    );
    wait("the reading", || playing(&rig));
    assert_eq!(rig.read_aloud.state().source, Some(lesson.clone()));
    assert_eq!(rig.spoken(), ["A lesson."]);
    // Ni une réponse supprimée, ni un autre cours ne l'arrêtent.
    rig.read_aloud.stop_if_reading(&[0, 7]);
    rig.read_aloud.stop_if_reading_course("course-b");
    assert_eq!(rig.read_aloud.state().source, Some(lesson.clone()));
    rig.read_aloud.stop_if_reading_course("course-a");
    assert!(idle(&rig));
    assert_eq!(rig.read_aloud.state().source, None);

    // Une seule lecture : une réponse remplace le texte du cours, et
    // l'inverse ; la suppression d'un cours n'arrête pas une réponse.
    rig.read_from(lesson.clone(), "A lesson.", None, false);
    wait("the lesson again", || playing(&rig));
    rig.read(7, "An answer.", false);
    wait("the answer", || {
        message_of(&rig.read_aloud.state()) == Some(7) && playing(&rig)
    });
    rig.read_aloud.stop_if_reading_course("course-a");
    assert_eq!(message_of(&rig.read_aloud.state()), Some(7));
    let other = passage("course-a", "L0:recap");
    rig.read_from(other.clone(), "Key points.", None, false);
    wait("the other text", || {
        rig.read_aloud.state().source == Some(other.clone()) && playing(&rig)
    });
    assert_eq!(rig.loads(), 1);
    rig.read_aloud.stop();

    // En erreur, l'état garde le texte concerné (texte montré à côté de son
    // bouton).
    rig.hold.store(false, Ordering::Relaxed);
    rig.broken.store(true, Ordering::Relaxed);
    rig.read_from(lesson.clone(), "A lesson.", None, false);
    wait("the failure", || {
        idle(&rig) && rig.read_aloud.state().error.is_some()
    });
    assert_eq!(rig.read_aloud.state().source, Some(lesson));
}

/// Texte trop court pour trahir sa langue : celle du cours (`fallback`),
/// pas celle de l'interface ; une réponse garde celle de l'interface.
#[test]
fn a_short_course_text_is_read_in_the_language_of_its_course() {
    let rig = rig();
    rig.read_from(
        passage("course-a", "L0:s0"),
        "OK.",
        Some(VoiceLanguage::Fr),
        false,
    );
    wait("the reading", || rig.spoken().len() == 1 && idle(&rig));
    rig.read(7, "OK.", false);
    wait("the answer", || rig.spoken().len() == 2 && idle(&rig));
    assert_eq!(
        *rig.log.languages.lock(),
        [VoiceLanguage::Fr, VoiceLanguage::En]
    );
    // Un texte dont la langue se reconnaît garde la sienne.
    let french = "Pour corriger l'erreur, ouvre le fichier de configuration et relance le serveur, puis vérifie que la page se recharge.";
    rig.read_from(
        passage("course-a", "L0:s1"),
        french,
        Some(VoiceLanguage::Es),
        false,
    );
    wait("the french text", || rig.spoken().len() >= 3 && idle(&rig));
    assert_eq!(rig.log.languages.lock().last(), Some(&VoiceLanguage::Fr));
}

#[test]
fn a_voice_is_downloaded_then_the_answer_is_read() {
    let rig = rig();
    rig.installed.store(false, Ordering::Relaxed);
    assert_eq!(rig.read(7, "Bonjour.", true), ReadAloudOutcome::Started);
    assert_eq!(*rig.queued.lock(), ["pocket-tts-en"]);
    let state = rig.read_aloud.state();
    assert_eq!(
        (message_of(&state), state.phase, state.percent),
        (Some(7), ReadAloudPhase::Downloading, Some(0))
    );
    // Un autre téléchargement ne compte pas.
    rig.read_aloud.on_downloads(&[SpeechDownload {
        id: "pocket-tts-fr".into(),
        ..download(SpeechDownloadState::Downloading, 900)
    }]);
    assert_eq!(rig.read_aloud.state().percent, Some(0));
    rig.read_aloud
        .on_downloads(&[download(SpeechDownloadState::Downloading, 500)]);
    assert_eq!(rig.read_aloud.state().percent, Some(50));
    // Entrée terminée mais voix toujours absente (périmée) : on attend.
    rig.read_aloud
        .on_downloads(&[download(SpeechDownloadState::Done, 1_000)]);
    assert_eq!(rig.read_aloud.state().phase, ReadAloudPhase::Downloading);
    assert!(rig.spoken().is_empty());
    rig.installed.store(true, Ordering::Relaxed);
    rig.read_aloud
        .on_downloads(&[download(SpeechDownloadState::Done, 1_000)]);
    wait("the reading", || rig.spoken().len() == 1 && idle(&rig));
    assert_eq!(rig.spoken(), ["Bonjour."]);
    assert_eq!(rig.read_aloud.state().error, None);
}

#[test]
fn stopping_while_downloading_gives_the_voice_to_cancel() {
    let rig = rig();
    rig.installed.store(false, Ordering::Relaxed);
    rig.read(7, "Bonjour.", true);
    assert_eq!(rig.read_aloud.stop(), Some("pocket-tts-en"));
    assert!(idle(&rig));
}

#[test]
fn a_failed_download_ends_the_reading_with_an_error() {
    let rig = rig();
    rig.installed.store(false, Ordering::Relaxed);
    rig.read(7, "Bonjour.", true);
    rig.read_aloud
        .on_downloads(&[download(SpeechDownloadState::Failed, 10)]);
    wait("the end of the reading", || idle(&rig));
    let state = rig.read_aloud.state();
    assert_eq!(
        (message_of(&state), state.error),
        (Some(7), Some(ReadAloudError::DownloadFailed))
    );
    assert!(rig.spoken().is_empty());
}

#[test]
fn a_cancelled_download_ends_the_reading_silently() {
    let rig = rig();
    rig.installed.store(false, Ordering::Relaxed);
    rig.read(7, "Bonjour.", true);
    rig.read_aloud
        .on_downloads(&[download(SpeechDownloadState::Cancelled, 10)]);
    wait("the end of the reading", || idle(&rig));
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(rig.read_aloud.state(), ReadAloudState::idle());
}

#[test]
fn engine_and_output_failures_are_reported() {
    for (load, open, expected) in [
        (
            Some(ReadAloudError::RuntimeUnavailable),
            None,
            ReadAloudError::RuntimeUnavailable,
        ),
        (
            Some(ReadAloudError::EngineFailed),
            None,
            ReadAloudError::EngineFailed,
        ),
        (
            None,
            Some(ReadAloudError::NoOutputDevice),
            ReadAloudError::NoOutputDevice,
        ),
    ] {
        let rig = rig();
        *rig.load_error.lock() = load;
        *rig.open_error.lock() = open;
        rig.read(7, "Hello.", false);
        wait("the end of the reading", || {
            idle(&rig) && rig.read_aloud.state().error.is_some()
        });
        assert_eq!(rig.read_aloud.state().error, Some(expected));
        // L'erreur reste attachée à sa réponse.
        assert_eq!(message_of(&rig.read_aloud.state()), Some(7));
        assert!(rig.spoken().is_empty());
    }
}

#[test]
fn deleting_the_chat_of_the_answer_being_read_stops_it() {
    let rig = rig();
    rig.hold.store(true, Ordering::Relaxed);
    rig.read(7, "A long answer.", false);
    wait("the reading", || playing(&rig));
    rig.read_aloud.stop_if_reading(&[1, 2, 3]);
    assert!(playing(&rig));
    rig.read_aloud.stop_if_reading(&[6, 7]);
    assert!(idle(&rig));
}

#[test]
fn automatic_reading_follows_a_reply_sentence_by_sentence() {
    let rig = rig();
    rig.read_aloud.set_auto_read(true);
    rig.observe(user_message());
    rig.observe(reply_started(42));
    rig.observe(content("Hello there. How"));
    rig.observe(content(" are you?"));
    rig.observe(done(42));
    wait("the end of the reading", || {
        rig.spoken().len() == 2 && idle(&rig)
    });
    assert_eq!(rig.spoken(), ["Hello there.", "How are you?"]);
    assert!(rig.phases().contains(&(Some(42), ReadAloudPhase::Playing)));
    // Seule la première phrase est un « premier son ».
    assert_eq!(*rig.log.firsts.lock(), [true, false]);
}

#[test]
fn automatic_reading_is_off_by_default() {
    let rig = rig();
    rig.observe(user_message());
    rig.observe(reply_started(42));
    rig.observe(content("Hello there. "));
    rig.observe(done(42));
    std::thread::sleep(Duration::from_millis(30));
    assert!(rig.spoken().is_empty());
    assert_eq!(rig.loads(), 0);
}

#[test]
fn automatic_reading_preloads_the_engine_while_the_model_thinks() {
    let rig = rig();
    rig.read_aloud.set_auto_read(true);
    rig.observe(user_message());
    wait("the preload", || {
        rig.read_aloud.inner.engine.lock().loaded.is_some()
    });
    rig.observe(reply_started(42));
    rig.observe(content("Hello there. "));
    rig.observe(done(42));
    wait("the reading", || rig.spoken().len() == 1 && idle(&rig));
    assert_eq!(rig.loads(), 1);
}

#[test]
fn automatic_reading_ignores_the_other_conversations() {
    let rig = rig();
    rig.read_aloud.set_auto_read(true);
    rig.observe(user_message());
    rig.observe(reply_started(42));
    rig.observe(content("Hello there. "));
    // Une autre conversation génère en même temps.
    rig.read_aloud.observe("chat-b", &reply_started(77), None);
    rig.read_aloud
        .observe("chat-b", &content("Other words. "), None);
    rig.read_aloud.observe("chat-b", &done(77), None);
    rig.observe(content("How are you?"));
    rig.observe(done(42));
    wait("the end of the reading", || {
        rig.spoken().len() == 2 && idle(&rig)
    });
    assert_eq!(rig.spoken(), ["Hello there.", "How are you?"]);
    assert!(rig.phases().iter().all(|(message, _)| *message != Some(77)));
}

#[test]
fn a_reading_started_by_hand_gets_no_text_from_the_reply() {
    let rig = rig();
    rig.hold.store(true, Ordering::Relaxed);
    rig.read_aloud.set_auto_read(true);
    rig.observe(user_message());
    rig.observe(reply_started(42));
    // Assez de texte pour choisir la langue (DETECT_CHARS).
    rig.observe(content(&"Hello there. ".repeat(12)));
    wait("the automatic reading", || playing(&rig));
    // L'utilisateur lit une ancienne réponse : la réponse en cours n'y
    // envoie plus rien.
    rig.read(7, "An old answer.", false);
    wait("the old answer", || {
        message_of(&rig.read_aloud.state()) == Some(7) && playing(&rig)
    });
    rig.observe(content("More words. "));
    rig.observe(done(42));
    std::thread::sleep(Duration::from_millis(30));
    rig.read_aloud.stop();
    assert_eq!(rig.spoken(), ["Hello there.", "An old answer."]);
}

#[test]
fn automatic_reading_skips_a_missing_voice_silently() {
    let rig = rig();
    rig.installed.store(false, Ordering::Relaxed);
    rig.read_aloud.set_auto_read(true);
    rig.observe(user_message());
    rig.observe(reply_started(42));
    rig.observe(content("Hello there. "));
    rig.observe(done(42));
    std::thread::sleep(Duration::from_millis(30));
    assert!(rig.spoken().is_empty());
    assert!(
        rig.log
            .states
            .lock()
            .iter()
            .all(|state| state.phase == ReadAloudPhase::Idle && state.error.is_none())
    );
    assert!(rig.queued.lock().is_empty());
}

#[test]
fn a_new_message_stops_the_current_reading() {
    let rig = rig();
    rig.hold.store(true, Ordering::Relaxed);
    rig.read(7, "A long answer.", false);
    wait("the reading", || playing(&rig));
    rig.observe(user_message());
    assert!(idle(&rig));
    assert!(rig.log.output_stops.load(Ordering::Relaxed) >= 1);
}

#[test]
fn stopping_the_reply_stops_its_automatic_reading() {
    let rig = rig();
    rig.hold.store(true, Ordering::Relaxed);
    rig.read_aloud.set_auto_read(true);
    rig.observe(user_message());
    rig.observe(reply_started(42));
    rig.observe(content(&"Hello there. ".repeat(40)));
    wait("the reading", || playing(&rig));
    rig.observe(ChatEvent::Cancelled);
    assert!(idle(&rig));
}

#[test]
fn an_error_of_the_reply_still_reads_what_was_written() {
    let rig = rig();
    rig.read_aloud.set_auto_read(true);
    rig.observe(user_message());
    rig.observe(reply_started(42));
    rig.observe(content("Half an answer"));
    rig.observe(ChatEvent::Error {
        kind: ChatErrorKind::ConnectionLost,
        message: "lost".into(),
    });
    wait("the reading", || rig.spoken().len() == 1 && idle(&rig));
    // Ponctuation finale ajoutée par le texte parlé.
    assert_eq!(rig.spoken(), ["Half an answer."]);
}

#[test]
fn the_next_tool_turn_moves_the_reading_to_its_message() {
    let rig = rig();
    rig.hold.store(true, Ordering::Relaxed);
    rig.read_aloud.set_auto_read(true);
    rig.observe(user_message());
    rig.observe(reply_started(41));
    rig.observe(content(&"Searching the web. ".repeat(30)));
    wait("the reading", || playing(&rig));
    assert_eq!(message_of(&rig.read_aloud.state()), Some(41));
    rig.observe(reply_started(42));
    assert_eq!(message_of(&rig.read_aloud.state()), Some(42));
    rig.read_aloud.stop();
}

#[test]
fn the_end_of_a_tool_turn_is_read_before_the_next_one() {
    let rig = rig();
    rig.read_aloud.set_auto_read(true);
    rig.observe(user_message());
    rig.observe(reply_started(41));
    // Pas de point final : la phrase n'est rendue qu'à la fin du message.
    rig.observe(content(
        &"Let me look this up for you in a moment ".repeat(4),
    ));
    rig.observe(reply_started(42));
    rig.observe(content("Here is the answer."));
    rig.observe(done(42));
    wait("the reading", || rig.spoken().len() == 2 && idle(&rig));
    assert!(rig.spoken()[0].starts_with("Let me look this up"));
    assert_eq!(rig.spoken()[1], "Here is the answer.");
}

#[test]
fn a_slow_engine_load_does_not_lose_the_first_sentences() {
    let rig = rig();
    *rig.load_delay.lock() = Duration::from_millis(50);
    rig.read_aloud.set_auto_read(true);
    rig.observe(user_message());
    rig.observe(reply_started(42));
    rig.observe(content(&"A sentence about many things. ".repeat(6)));
    rig.observe(content("The last one."));
    rig.observe(done(42));
    wait("the reading", || rig.spoken().len() == 7 && idle(&rig));
    assert_eq!(rig.spoken().last().unwrap(), "The last one.");
}

#[test]
fn a_panic_ends_the_reading_with_an_error_and_frees_the_engine() {
    let rig = rig();
    rig.read(1, "Panic.", false);
    wait("the end of the reading", || {
        idle(&rig) && rig.read_aloud.state().error.is_some()
    });
    assert_eq!(
        rig.read_aloud.state().error,
        Some(ReadAloudError::EngineFailed)
    );
    assert_eq!(rig.read_aloud.inner.workers.load(Ordering::Acquire), 0);
    assert!(!rig.read_aloud.inner.engine.lock().busy);
    // La lecture suivante ne l'attend pas.
    let started = Instant::now();
    rig.read(2, "Two.", false);
    wait("the next reading", || {
        rig.spoken() == ["Two."] && idle(&rig)
    });
    assert!(started.elapsed() < ENGINE_WAIT);
}

/// Envoi vocal de la séance du mode vocal, `hint` : langue de la phrase.
fn voice(hint: Option<VoiceLanguage>) -> VoiceSend {
    VoiceSend {
        language: hint,
        ..VoiceSend::default()
    }
}

impl Rig {
    fn observe_voice(&self, event: ChatEvent, hint: Option<VoiceLanguage>) {
        self.read_aloud.observe(CHAT, &event, Some(&voice(hint)));
    }

    /// Événements des lectures vocales (conversation `CHAT`), sans ceux des
    /// autres lectures.
    fn readings(&self) -> Vec<VoiceReading> {
        self.log
            .readings
            .lock()
            .iter()
            .filter(|(_, reading)| !matches!(reading, VoiceReading::Aside(_)))
            .map(|(chat, reading)| {
                assert_eq!(chat, CHAT);
                reading.clone()
            })
            .collect()
    }

    /// Événements des autres lectures (premier son, fin).
    fn asides(&self) -> Vec<bool> {
        self.log
            .readings
            .lock()
            .iter()
            .filter_map(|(chat, reading)| match reading {
                VoiceReading::Aside(playing) => {
                    assert!(chat.is_empty());
                    Some(*playing)
                }
                _ => None,
            })
            .collect()
    }

    fn languages(&self) -> Vec<VoiceLanguage> {
        self.log.languages.lock().clone()
    }

    /// Envoi vocal suivi jusqu'au premier fragment de sa réponse.
    fn voice_reply(&self, hint: Option<VoiceLanguage>, text: &str) {
        self.observe_voice(user_message(), hint);
        self.observe_voice(reply_started(42), hint);
        self.observe_voice(content(text), hint);
    }
}

fn ended(end: ReadingEnd) -> VoiceReading {
    VoiceReading::Ended(end)
}

#[test]
fn a_voice_send_is_read_without_automatic_reading() {
    let rig = rig();
    rig.read_aloud.set_voice_active(true);
    rig.voice_reply(Some(VoiceLanguage::En), "Hello there. How");
    rig.observe_voice(content(" are you?"), Some(VoiceLanguage::En));
    rig.observe_voice(done(42), Some(VoiceLanguage::En));
    wait("the end of the reading", || {
        rig.readings().len() == 2 && idle(&rig)
    });
    assert_eq!(rig.spoken(), ["Hello there.", "How are you?"]);
    assert_eq!(
        rig.readings(),
        [VoiceReading::Playing, ended(ReadingEnd::Finished)]
    );
}

#[test]
fn a_voice_send_outside_a_session_is_an_ordinary_send() {
    let rig = rig();
    rig.voice_reply(Some(VoiceLanguage::En), "Hello there. ");
    rig.observe_voice(done(42), Some(VoiceLanguage::En));
    std::thread::sleep(Duration::from_millis(30));
    assert!(rig.spoken().is_empty());
    assert!(rig.readings().is_empty());
    // Lecture automatique : lue, sans rien dire à l'écouteur.
    rig.read_aloud.set_auto_read(true);
    rig.voice_reply(Some(VoiceLanguage::En), "Hello there. ");
    rig.observe_voice(done(42), Some(VoiceLanguage::En));
    wait("the reading", || rig.spoken().len() == 1 && idle(&rig));
    assert!(rig.readings().is_empty());
}

#[test]
fn a_voice_reading_starts_with_the_first_sentence_in_the_hinted_language() {
    let rig = rig();
    rig.hold.store(true, Ordering::Relaxed);
    rig.read_aloud.set_voice_active(true);
    // Première phrase trop courte pour la détection : l'indice, et non la
    // langue de l'interface (anglais) ; la lecture part dès qu'elle est
    // complète (début de la suivante), sans attendre `DETECT_CHARS`
    // caractères ni la fin.
    rig.voice_reply(Some(VoiceLanguage::Fr), "Oui. Il");
    wait("the first sentence", || playing(&rig));
    assert_eq!(rig.spoken(), ["Oui."]);
    assert_eq!(rig.languages(), [VoiceLanguage::Fr]);
    assert_eq!(rig.readings(), [VoiceReading::Playing]);
    rig.read_aloud.stop_voice();
    assert_eq!(
        rig.readings(),
        [VoiceReading::Playing, ended(ReadingEnd::Stopped)]
    );
}

#[test]
fn a_clear_first_sentence_beats_the_hint() {
    let rig = rig();
    rig.read_aloud.set_voice_active(true);
    let english = "I think that the weather is going to be nice for the rest of the week. ";
    rig.voice_reply(Some(VoiceLanguage::Fr), english);
    rig.observe_voice(done(42), Some(VoiceLanguage::Fr));
    wait("the reading", || rig.readings().len() == 2);
    // La voix de l'indice a pu être préparée pendant que le modèle écrivait.
    assert_eq!(rig.languages().last(), Some(&VoiceLanguage::En));
    assert_eq!(rig.spoken().len(), 1);
    // Sans indice : la langue de l'interface.
    let rig = self::rig();
    rig.read_aloud.set_voice_active(true);
    rig.voice_reply(None, "Oui. ");
    rig.observe_voice(done(42), None);
    wait("the reading", || rig.readings().len() == 2);
    assert_eq!(rig.languages(), [VoiceLanguage::En]);
}

#[test]
fn a_voice_reading_tells_why_it_ended() {
    // Autre langue.
    let rig = rig();
    rig.read_aloud.set_voice_active(true);
    let german = "Das ist eine kurze Antwort auf deine Frage, und sie ist ganz sicher nicht auf Englisch. Und";
    rig.voice_reply(None, german);
    wait("the end", || !rig.readings().is_empty());
    assert_eq!(rig.readings(), [ended(ReadingEnd::Unsupported)]);
    assert!(rig.spoken().is_empty());

    // Voix absente : rien n'est téléchargé, rien n'est montré sous la
    // réponse.
    let rig = self::rig();
    rig.read_aloud.set_voice_active(true);
    rig.installed.store(false, Ordering::Relaxed);
    rig.voice_reply(Some(VoiceLanguage::Es), "Hola. Que");
    wait("the end", || !rig.readings().is_empty());
    assert_eq!(
        rig.readings(),
        [ended(ReadingEnd::MissingVoice(VoiceLanguage::Es))]
    );
    assert!(rig.queued.lock().is_empty());
    assert_eq!(rig.read_aloud.state(), ReadAloudState::idle());

    // Réponse vide.
    let rig = self::rig();
    rig.read_aloud.set_voice_active(true);
    rig.observe_voice(user_message(), None);
    rig.observe_voice(reply_started(42), None);
    rig.observe_voice(done(42), None);
    wait("the end", || !rig.readings().is_empty());
    assert_eq!(rig.readings(), [ended(ReadingEnd::NothingToRead)]);

    // Moteur en échec.
    let rig = self::rig();
    rig.read_aloud.set_voice_active(true);
    rig.broken.store(true, Ordering::Relaxed);
    rig.voice_reply(None, "Hello there. How");
    wait("the end", || !rig.readings().is_empty());
    assert_eq!(
        rig.readings(),
        [ended(ReadingEnd::Failed(ReadAloudError::EngineFailed))]
    );
}

#[test]
fn a_voice_send_without_a_reply_ends_at_once() {
    // Erreur avant la réponse (serveur injoignable) : rien à lire, l'erreur
    // dite d'abord.
    let rig = rig();
    rig.read_aloud.set_voice_active(true);
    rig.observe_voice(user_message(), None);
    rig.observe_voice(
        ChatEvent::Error {
            kind: ChatErrorKind::Unreachable,
            message: "unreachable".into(),
        },
        None,
    );
    assert_eq!(
        rig.readings(),
        [
            VoiceReading::ReplyFailed("unreachable".into()),
            ended(ReadingEnd::NothingToRead)
        ]
    );
    // Arrêtée avant la réponse.
    let rig = self::rig();
    rig.read_aloud.set_voice_active(true);
    rig.observe_voice(user_message(), None);
    rig.observe_voice(ChatEvent::Cancelled, None);
    assert_eq!(rig.readings(), [ended(ReadingEnd::Stopped)]);
    // Arrêtée pendant la lecture.
    let rig = self::rig();
    rig.hold.store(true, Ordering::Relaxed);
    rig.read_aloud.set_voice_active(true);
    rig.voice_reply(None, "Hello there. How");
    wait("the reading", || playing(&rig));
    rig.observe_voice(ChatEvent::Cancelled, None);
    assert!(idle(&rig));
    assert_eq!(
        rig.readings(),
        [VoiceReading::Playing, ended(ReadingEnd::Stopped)]
    );
}

#[test]
fn a_new_send_ends_the_previous_voice_reading_first() {
    let rig = rig();
    rig.hold.store(true, Ordering::Relaxed);
    rig.read_aloud.set_voice_active(true);
    rig.voice_reply(None, "Hello there. How");
    wait("the reading", || playing(&rig));
    rig.observe_voice(user_message(), None);
    assert_eq!(
        rig.readings(),
        [VoiceReading::Playing, ended(ReadingEnd::Stopped)]
    );
    // Envoi vocal suivi pas encore commencé : arrêté de même.
    rig.observe_voice(user_message(), None);
    assert_eq!(rig.readings().len(), 3);
    assert_eq!(rig.readings()[2], ended(ReadingEnd::Stopped));
    rig.read_aloud.stop();
}

#[test]
fn stop_voice_leaves_a_manual_reading_alone() {
    let rig = rig();
    rig.hold.store(true, Ordering::Relaxed);
    rig.read_aloud.set_voice_active(true);
    rig.read(7, "An old answer.", false);
    wait("the reading", || playing(&rig));
    rig.read_aloud.stop_voice();
    assert!(playing(&rig));
    assert!(rig.readings().is_empty());
    // La séance apprend qu'une autre lecture joue, puis qu'elle s'arrête.
    assert_eq!(rig.asides(), [true]);
    rig.read_aloud.stop();
    assert!(rig.readings().is_empty());
    assert_eq!(rig.asides(), [true, false]);
}

#[test]
fn another_reading_tells_its_first_sound_and_its_end() {
    let rig = rig();
    rig.read(7, "An old answer.", false);
    wait("the end of the reading", || rig.asides().len() == 2);
    assert_eq!(rig.asides(), [true, false]);
    // Lecture qui finit avant de jouer : rien.
    let rig = self::rig();
    rig.broken.store(true, Ordering::Relaxed);
    rig.read(7, "An old answer.", false);
    wait("the failure", || {
        idle(&rig) && rig.read_aloud.state().error.is_some()
    });
    assert!(rig.asides().is_empty());
    // Lecture vocale : pas une autre lecture.
    let rig = self::rig();
    rig.read_aloud.set_voice_active(true);
    rig.voice_reply(None, "Hello there. How");
    rig.observe_voice(done(42), None);
    wait("the reading", || rig.readings().len() == 2);
    assert!(rig.asides().is_empty());
}

#[test]
fn an_error_of_a_voice_reply_is_told_before_its_end() {
    let rig = rig();
    rig.read_aloud.set_voice_active(true);
    let error = ChatEvent::Error {
        kind: ChatErrorKind::ConnectionLost,
        message: "Connection lost".into(),
    };
    // Lecture commencée : ce qui a été écrit est lu, l'erreur dite avant.
    rig.voice_reply(None, "Half an answer");
    rig.observe_voice(error.clone(), None);
    wait("the reading", || rig.readings().len() == 3);
    assert_eq!(
        rig.readings(),
        [
            VoiceReading::ReplyFailed("Connection lost".into()),
            VoiceReading::Playing,
            ended(ReadingEnd::Finished)
        ]
    );
    // Pas de réponse : l'erreur, puis la fin.
    let rig = self::rig();
    rig.read_aloud.set_voice_active(true);
    rig.observe_voice(user_message(), None);
    rig.observe_voice(error, None);
    assert_eq!(
        rig.readings(),
        [
            VoiceReading::ReplyFailed("Connection lost".into()),
            ended(ReadingEnd::NothingToRead)
        ]
    );
}

#[test]
fn a_short_first_sentence_is_checked_again_on_more_text() {
    // Réponse en allemand, première phrase trop courte pour la détection :
    // lue avec l'indice, puis arrêtée quand la suite montre la langue.
    let rig = rig();
    rig.read_aloud.set_voice_active(true);
    rig.voice_reply(Some(VoiceLanguage::Fr), "Gut, danke. Das");
    rig.observe_voice(
        content(
            " ist eine kurze Antwort auf deine Frage, und sie ist ganz sicher nicht auf Englisch geschrieben, sondern auf Deutsch, wie du siehst. Und",
        ),
        Some(VoiceLanguage::Fr),
    );
    rig.observe_voice(done(42), Some(VoiceLanguage::Fr));
    wait("the end", || rig.readings().len() == 2);
    assert_eq!(
        rig.readings(),
        [VoiceReading::Playing, ended(ReadingEnd::Unsupported)]
    );
    // Seule la première phrase a été lue ; la sortie est coupée.
    assert_eq!(rig.spoken().len(), 1);
    assert!(rig.log.output_stops.load(Ordering::Relaxed) >= 1);

    // Réponse en anglais avec l'indice français : la suite passe à la voix
    // anglaise.
    let rig = self::rig();
    rig.read_aloud.set_voice_active(true);
    rig.voice_reply(Some(VoiceLanguage::Fr), "OK. I");
    rig.observe_voice(
        content(
            " think that the weather is going to be nice for the rest of the week, so you can go outside and enjoy the sun with your friends. Then",
        ),
        Some(VoiceLanguage::Fr),
    );
    rig.observe_voice(done(42), Some(VoiceLanguage::Fr));
    wait("the end", || rig.readings().len() == 2);
    assert_eq!(
        rig.readings(),
        [VoiceReading::Playing, ended(ReadingEnd::Finished)]
    );
    assert_eq!(rig.spoken().len(), 3);
    // Voix française pour la première phrase, puis anglaise.
    let languages = rig.languages();
    let french = languages.iter().position(|l| *l == VoiceLanguage::Fr);
    let english = languages.iter().position(|l| *l == VoiceLanguage::En);
    assert!(french.is_some() && french < english, "{languages:?}");
}

#[test]
fn the_voice_of_a_language_is_preloaded() {
    let rig = rig();
    rig.read_aloud.preload_language(VoiceLanguage::Fr);
    wait("the preload", || {
        rig.read_aloud.inner.engine.lock().loaded.is_some()
    });
    rig.read_aloud.preload_language(VoiceLanguage::Fr);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(rig.languages(), [VoiceLanguage::Fr]);
    // Autre langue : remplacé.
    rig.read_aloud.preload_language(VoiceLanguage::Es);
    wait("the second preload", || rig.loads() == 2);
    wait("the engine kept", || {
        rig.read_aloud
            .inner
            .engine
            .lock()
            .loaded
            .as_ref()
            .is_some_and(|loaded| loaded.language == VoiceLanguage::Es)
    });
    // Voix absente : rien.
    rig.installed.store(false, Ordering::Relaxed);
    rig.read_aloud.preload_language(VoiceLanguage::En);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(rig.loads(), 2);
}

#[test]
fn a_voice_send_preloads_the_hinted_voice() {
    let rig = rig();
    rig.read_aloud.set_voice_active(true);
    rig.observe_voice(user_message(), Some(VoiceLanguage::Es));
    wait("the preload", || rig.loads() == 1);
    assert_eq!(rig.languages(), [VoiceLanguage::Es]);
}

/// Lecture par segments du pont : évènements reçus.
fn segments_rig() -> (Rig, Arc<Mutex<Vec<SegmentEvent>>>, SegmentSink) {
    let rig = rig();
    let events = Arc::new(Mutex::new(Vec::new()));
    let received = events.clone();
    let sink: SegmentSink = Arc::new(move |event| received.lock().push(event));
    (rig, events, sink)
}

fn segment(id: &str, text: &str) -> Segment {
    Segment {
        id: id.into(),
        text: text.into(),
    }
}

fn started(id: &str) -> SegmentEvent {
    SegmentEvent::Started(id.into())
}

fn finished(id: &str) -> SegmentEvent {
    SegmentEvent::Finished(id.into())
}

/// Chaque segment est annoncé quand son son est joué (horloge de la
/// sortie), pas quand il est synthétisé : la synthèse finie, rien n'est
/// annoncé tant que la sortie n'a rien joué ; puis début et fin de chaque
/// segment, dans l'ordre, au fil du compteur.
#[test]
fn segments_follow_the_sound_actually_played() {
    let (rig, events, sink) = segments_rig();
    *rig.log.clock.lock() = Some(0);
    rig.read_aloud
        .read_segments(
            "1-1".into(),
            vec![segment("a", "First part."), segment("b", "Second part.")],
            Some(VoiceLanguage::En),
            sink,
        )
        .unwrap();
    assert_eq!(
        rig.read_aloud.state().source,
        Some(ReadAloudSource::Bridge {
            read_id: "1-1".into()
        })
    );
    // Deux segments synthétisés (10 échantillons chacun), rien de joué.
    wait("both segments synthesized", || rig.spoken().len() == 2);
    std::thread::sleep(Duration::from_millis(60));
    assert!(events.lock().is_empty(), "{:?}", events.lock());
    // Début du premier joué.
    *rig.log.clock.lock() = Some(3);
    wait("first segment started", || events.lock().len() == 1);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(*events.lock(), [started("a")]);
    // Tout le premier joué, un échantillon du second.
    *rig.log.clock.lock() = Some(11);
    wait("second segment started", || events.lock().len() == 3);
    assert_eq!(*events.lock(), [started("a"), finished("a"), started("b")]);
    // Tout joué : fin du second, puis plus rien (ni arrêt ni échec).
    *rig.log.clock.lock() = Some(20);
    wait("reading over", || idle(&rig));
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(
        *events.lock(),
        [started("a"), finished("a"), started("b"), finished("b")]
    );
}

/// Une nouvelle lecture (ou un arrêt) arrête la lecture par segments, qui
/// le dit ; plus rien n'est annoncé pour elle ensuite.
#[test]
fn a_new_reading_stops_the_segments() {
    let (rig, events, sink) = segments_rig();
    *rig.log.clock.lock() = Some(0);
    rig.read_aloud
        .read_segments(
            "1-1".into(),
            vec![segment("a", "First part."), segment("b", "Second part.")],
            None,
            sink,
        )
        .unwrap();
    wait("both segments synthesized", || rig.spoken().len() == 2);
    *rig.log.clock.lock() = Some(3);
    wait("first segment started", || events.lock().len() == 1);
    rig.read(7, "Another text.", false);
    wait("stopped", || events.lock().len() == 2);
    *rig.log.clock.lock() = None;
    wait("other reading over", || idle(&rig));
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(*events.lock(), [started("a"), SegmentEvent::Stopped]);

    // Arrêt par son identifiant : seulement le sien.
    let (rig, events, sink) = segments_rig();
    rig.hold.store(true, Ordering::Relaxed);
    rig.read_aloud
        .read_segments("2-5".into(), vec![segment("a", "Long part.")], None, sink)
        .unwrap();
    wait("speaking", || rig.spoken().len() == 1);
    rig.read_aloud.stop_segments(|id| id == "2-4");
    assert!(!idle(&rig));
    rig.read_aloud.stop_segments(|id| id == "2-5");
    assert!(idle(&rig));
    assert_eq!(*events.lock(), [SegmentEvent::Stopped]);
}

/// Refus d'emblée : rien à lire, autre langue, voix absente (jamais
/// téléchargée), machine incapable ; rien ne démarre.
#[test]
fn segments_are_refused_before_anything_starts() {
    let (rig, events, sink) = segments_rig();
    let read = |segments: Vec<Segment>, language| {
        rig.read_aloud
            .read_segments("1-1".into(), segments, language, sink.clone())
    };
    assert_eq!(
        read(vec![segment("a", "```\ncode\n```")], None),
        Err(SegmentsRefusal::NothingToRead)
    );
    assert_eq!(
        read(
            vec![segment(
                "a",
                "Das ist eine kurze Antwort auf deine Frage, und sie ist ganz sicher nicht auf Englisch. Und"
            )],
            None
        ),
        Err(SegmentsRefusal::Unsupported)
    );
    rig.installed.store(false, Ordering::Relaxed);
    assert_eq!(
        read(vec![segment("a", "Hello.")], Some(VoiceLanguage::Fr)),
        Err(SegmentsRefusal::MissingVoice(VoiceLanguage::Fr))
    );
    assert!(rig.queued.lock().is_empty());
    rig.installed.store(true, Ordering::Relaxed);
    rig.runtime.store(false, Ordering::Relaxed);
    assert_eq!(
        read(vec![segment("a", "Hello.")], None),
        Err(SegmentsRefusal::RuntimeUnavailable)
    );
    assert!(rig.log.states.lock().is_empty());
    assert!(events.lock().is_empty());
}

/// Moteur en échec : la lecture par segments finit par `Failed`.
#[test]
fn a_failed_engine_ends_the_segments_with_an_error() {
    let (rig, events, sink) = segments_rig();
    rig.broken.store(true, Ordering::Relaxed);
    rig.read_aloud
        .read_segments("1-1".into(), vec![segment("a", "Hello.")], None, sink)
        .unwrap();
    wait("failed", || !events.lock().is_empty());
    assert_eq!(
        *events.lock(),
        [SegmentEvent::Failed(ReadingEnd::Failed(
            ReadAloudError::EngineFailed
        ))]
    );
}
