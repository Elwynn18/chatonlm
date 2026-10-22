//! Machine d'états de la séance, avec une fausse écoute, un faux moteur de
//! transcription et une fausse app (dictée, chat, lecture à voix haute).

use std::sync::atomic::AtomicUsize;

use super::*;
use crate::dictation::engine::Recognizer;
use crate::store::chats::{Message, Role};
use crate::store::voice_mode::VoicePause;
use crate::tts::ReadAloudError;

const WAIT: Duration = Duration::from_secs(10);
const CHAT: &str = "chat-voice";
const SPOKEN: &str = "Bonjour, quelle heure est-il ?";

/// Écoute ouverte par la fausse oreille : ce que la séance lui demande, et
/// son rappel, par lequel l'essai joue l'écoute.
struct Probe {
    events: Events,
    config: ListenConfig,
    gates: Mutex<Vec<Gate>>,
    end_turns: AtomicUsize,
    holds: Mutex<Vec<bool>>,
    configs: Mutex<Vec<ListenConfig>>,
    closed: AtomicBool,
}

impl Probe {
    fn emit(&self, event: ListenEvent) {
        (self.events)(event);
    }

    /// Gate courante (`Listen` à l'ouverture).
    fn gate(&self) -> Gate {
        self.gates.lock().last().copied().unwrap_or(Gate::Listen)
    }

    fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

struct FakeListening(Arc<Probe>);

impl Listening for FakeListening {
    fn set_gate(&self, gate: Gate) {
        self.0.gates.lock().push(gate);
    }

    fn end_turn(&self) {
        self.0.end_turns.fetch_add(1, Ordering::SeqCst);
    }

    fn hold(&self, pressed: bool) {
        self.0.holds.lock().push(pressed);
    }

    fn configure(&self, config: ListenConfig) {
        self.0.configs.lock().push(config);
    }
}

impl Drop for FakeListening {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct FakeEar {
    probes: Mutex<Vec<Arc<Probe>>>,
    /// Micro qui refuse de s'ouvrir.
    refusal: Mutex<Option<String>>,
}

impl Ear for FakeEar {
    fn open(&self, config: ListenConfig, events: Events) -> Result<Box<dyn Listening>, String> {
        if let Some(refusal) = self.refusal.lock().clone() {
            return Err(refusal);
        }
        let probe = Arc::new(Probe {
            events,
            config,
            gates: Mutex::new(Vec::new()),
            end_turns: AtomicUsize::new(0),
            holds: Mutex::new(Vec::new()),
            configs: Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
        });
        self.probes.lock().push(probe.clone());
        Ok(Box::new(FakeListening(probe)))
    }
}

struct FakeHost {
    prepared: Mutex<Result<Prepared, String>>,
    /// Préparation retenue jusqu'au feu vert (arrêt pendant le démarrage).
    prepare_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    dictating: AtomicBool,
    reserved: AtomicBool,
    states: Mutex<Vec<VoiceModeState>>,
    levels: Mutex<Vec<VoiceModeLevel>>,
    utterances: Mutex<Vec<VoiceModeUtterance>>,
    reading: AtomicBool,
    stop_readings: AtomicUsize,
    stop_any_readings: AtomicUsize,
    preloaded: Mutex<Vec<VoiceLanguage>>,
    stopped_replies: Mutex<Vec<String>>,
    remembered: Mutex<Vec<String>>,
}

impl Host for FakeHost {
    fn prepare(&self) -> BoxFuture<Result<Prepared, String>> {
        let prepared = self.prepared.lock().clone();
        let gate = self.prepare_gate.lock().clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.notified().await;
            }
            prepared
        })
    }

    fn remember_speech_model(&self, id: String) -> BoxFuture<()> {
        self.remembered.lock().push(id);
        Box::pin(async {})
    }

    fn reserve(&self) -> bool {
        if self.dictating.load(Ordering::SeqCst) {
            return false;
        }
        self.reserved.store(true, Ordering::SeqCst);
        true
    }

    fn release(&self) {
        self.reserved.store(false, Ordering::SeqCst);
    }

    fn changed(&self, state: &VoiceModeState) {
        self.states.lock().push(state.clone());
    }

    fn level(&self, level: VoiceModeLevel) {
        self.levels.lock().push(level);
    }

    fn utterance(&self, utterance: &VoiceModeUtterance) {
        self.utterances.lock().push(utterance.clone());
    }

    fn reading_active(&self, active: bool) {
        self.reading.store(active, Ordering::SeqCst);
    }

    fn stop_reading(&self) {
        self.stop_readings.fetch_add(1, Ordering::SeqCst);
    }

    fn stop_any_reading(&self) {
        self.stop_any_readings.fetch_add(1, Ordering::SeqCst);
    }

    fn preload_voice(&self, language: VoiceLanguage) {
        self.preloaded.lock().push(language);
    }

    fn stop_reply(&self, chat: String) -> BoxFuture<()> {
        self.stopped_replies.lock().push(chat);
        Box::pin(async {})
    }
}

/// Modèle de transcription simulé : rend `text`, ou `error` ; après le feu
/// vert de l'essai si une barrière est posée.
#[derive(Default)]
struct Model {
    text: Mutex<String>,
    error: Mutex<Option<String>>,
    transcriptions: AtomicUsize,
    requests: Mutex<Vec<Request>>,
    gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

struct FakeRecognizer(Arc<Model>);

impl Recognizer for FakeRecognizer {
    fn transcribe(&mut self, _samples: &[f32], request: &Request) -> Result<String, String> {
        self.0.transcriptions.fetch_add(1, Ordering::SeqCst);
        self.0.requests.lock().push(request.clone());
        let gate = self.0.gate.lock().take();
        if let Some(gate) = gate {
            let _ = gate.recv_timeout(WAIT);
        }
        match self.0.error.lock().clone() {
            Some(error) => Err(error),
            None => Ok(self.0.text.lock().clone()),
        }
    }
}

fn requirements() -> VoiceModeRequirements {
    VoiceModeRequirements {
        speech_model: "whisper-tiny".into(),
        speech_model_installed: true,
        speech_model_bytes: 0,
        voice_model: "pocket-tts-fr".into(),
        voice_language: VoiceLanguage::Fr,
        voice_installed: true,
        voice_bytes: 0,
        unavailable: None,
    }
}

fn prepared() -> Prepared {
    let model = speech_models::find("whisper-tiny").unwrap();
    Prepared {
        requirements: requirements(),
        speech: ModelSpec::new(model, Path::new("/nowhere")),
        remember: false,
        request: Request {
            language: Some("fr".into()),
            glossary: vec!["ChatonLM".into()],
        },
        settings: VoiceModeSettings::default(),
    }
}

struct Rig {
    core: Arc<Core>,
    host: Arc<FakeHost>,
    ear: Arc<FakeEar>,
    model: Arc<Model>,
}

/// Refus d'un moteur de transcription par la machine.
type Support = fn(SpeechEngine) -> Result<(), String>;

fn rig() -> Rig {
    rig_with(prepared(), Duration::from_secs(60), |_| Ok(()))
}

/// Séance sur `prepared`, pause après `pause_after`, moteurs acceptés par
/// `support`.
fn rig_with(prepared: Prepared, pause_after: Duration, support: Support) -> Rig {
    let model = Arc::new(Model::default());
    *model.text.lock() = SPOKEN.into();
    let loaded = model.clone();
    let engine = Engine::with_loader(
        Box::new(move |_spec: &ModelSpec| {
            Ok(Box::new(FakeRecognizer(loaded.clone())) as Box<dyn Recognizer>)
        }),
        engine::IDLE,
    );
    let ear = Arc::new(FakeEar::default());
    let host = Arc::new(FakeHost {
        prepared: Mutex::new(Ok(prepared)),
        prepare_gate: Mutex::new(None),
        dictating: AtomicBool::new(false),
        reserved: AtomicBool::new(false),
        states: Mutex::new(Vec::new()),
        levels: Mutex::new(Vec::new()),
        utterances: Mutex::new(Vec::new()),
        reading: AtomicBool::new(false),
        stop_readings: AtomicUsize::new(0),
        stop_any_readings: AtomicUsize::new(0),
        preloaded: Mutex::new(Vec::new()),
        stopped_replies: Mutex::new(Vec::new()),
        remembered: Mutex::new(Vec::new()),
    });
    let services = Services {
        ear: ear.clone(),
        engine,
        support,
        pause_after,
        timings: false,
    };
    Rig {
        core: Core::new(services, host.clone()),
        host,
        ear,
        model,
    }
}

/// « Parole » (syllabes : 250 ms de sinus, 150 ms de silence) à 16 kHz.
fn speech() -> Vec<f32> {
    (0..16_000)
        .map(|i| {
            if i % 6_400 < 4_000 {
                0.3 * (i as f32 * 0.05).sin()
            } else {
                0.0
            }
        })
        .collect()
}

fn user_message() -> ChatEvent {
    ChatEvent::UserMessage {
        message: Message {
            id: 1,
            role: Role::User,
            content: SPOKEN.into(),
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

fn voice() -> VoiceSend {
    VoiceSend {
        language: Some(VoiceLanguage::Fr),
    }
}

impl Rig {
    fn state(&self) -> VoiceModeState {
        self.core.inner.lock().state.clone()
    }

    fn phase(&self) -> VoiceModePhase {
        self.state().phase
    }

    /// Dernière écoute ouverte.
    fn probe(&self) -> Arc<Probe> {
        self.ear
            .probes
            .lock()
            .last()
            .cloned()
            .expect("an open listening")
    }

    fn opens(&self) -> usize {
        self.ear.probes.lock().len()
    }

    /// Phases envoyées à la page, sans répétition.
    fn phases(&self) -> Vec<VoiceModePhase> {
        let mut phases: Vec<_> = self.host.states.lock().iter().map(|s| s.phase).collect();
        phases.dedup();
        phases
    }

    fn stop_readings(&self) -> usize {
        self.host.stop_readings.load(Ordering::SeqCst)
    }

    fn stopped_replies(&self) -> Vec<String> {
        self.host.stopped_replies.lock().clone()
    }

    async fn wait(&self, what: &str, condition: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !condition(self) {
            assert!(
                Instant::now() < deadline,
                "{what}: state {:?}",
                self.state()
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    async fn wait_phase(&self, phase: VoiceModePhase) {
        self.wait(&format!("{phase:?}"), |rig| rig.phase() == phase)
            .await;
    }

    /// Séance démarrée, à l'écoute.
    async fn started(&self) {
        self.core.start().await.unwrap();
        assert_eq!(self.phase(), VoiceModePhase::Listening);
    }

    /// Une phrase dite puis transcrite : Responding.
    async fn say(&self) {
        let probe = self.probe();
        probe.emit(ListenEvent::SpeechStarted);
        self.wait_phase(Hearing).await;
        probe.emit(ListenEvent::Utterance(speech()));
        self.wait_phase(Responding).await;
    }

    /// Phrase envoyée par la page dans `CHAT` (observateur du chat).
    fn sent(&self) {
        self.core.observe(CHAT, &user_message(), Some(&voice()));
    }

    /// Un tour jusqu'à la lecture de sa réponse : Speaking.
    async fn speaking(&self) {
        self.say().await;
        self.sent();
        self.core.on_reading(CHAT, VoiceReading::Playing);
        assert_eq!(self.phase(), Speaking);
    }

    /// Laisse passer les événements en vol (tâche de relais).
    async fn settle(&self) {
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

#[tokio::test]
async fn a_full_turn_ends_listening_again() {
    let rig = rig();
    rig.started().await;
    assert!(rig.host.reserved.load(Ordering::SeqCst));
    assert!(rig.host.reading.load(Ordering::SeqCst));
    assert_eq!(*rig.host.preloaded.lock(), [VoiceLanguage::Fr]);
    let probe = rig.probe();
    assert_eq!(
        probe.config,
        ListenConfig {
            mode: TurnMode::HandsFree,
            silence: Duration::from_secs(1),
            barge_in: true,
        }
    );
    probe.emit(ListenEvent::Levels {
        input: 0.5,
        output: 0.25,
    });
    rig.wait("the levels", |rig| !rig.host.levels.lock().is_empty())
        .await;
    assert_eq!(
        *rig.host.levels.lock(),
        [VoiceModeLevel {
            input: 0.5,
            output: 0.25
        }]
    );

    rig.say().await;
    // Transcription : rien n'est signalé ; réponse : seule une parole
    // franche l'est.
    assert_eq!(*probe.gates.lock(), [Gate::Ignore, Gate::Respond]);
    assert_eq!(
        *rig.host.utterances.lock(),
        [VoiceModeUtterance {
            turn: 1,
            text: SPOKEN.into(),
            language: Some(VoiceLanguage::Fr),
        }]
    );
    // Langue et dictionnaire de la dictée.
    assert_eq!(*rig.model.requests.lock(), [prepared().request]);
    assert_eq!(rig.state().turn, 1);

    rig.sent();
    rig.core.on_reading(CHAT, VoiceReading::Playing);
    assert_eq!(rig.phase(), Speaking);
    rig.core
        .on_reading(CHAT, VoiceReading::Ended(ReadingEnd::Finished));
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    assert_eq!(probe.gate(), Gate::Listen);
    assert_eq!(rig.state().notice, None);
    assert_eq!(
        rig.phases(),
        [
            Starting,
            VoiceModePhase::Listening,
            Hearing,
            Transcribing,
            Responding,
            Speaking,
            VoiceModePhase::Listening
        ]
    );
    assert!(!probe.closed());
}

#[tokio::test]
async fn a_phrase_without_speech_or_text_is_not_sent() {
    let rig = rig();
    rig.started().await;
    let probe = rig.probe();
    // Silence : pas de transcription.
    probe.emit(ListenEvent::SpeechStarted);
    rig.wait_phase(Hearing).await;
    probe.emit(ListenEvent::Utterance(vec![0.0; 16_000]));
    rig.wait_phase(VoiceModePhase::Listening).await;
    assert_eq!(rig.model.transcriptions.load(Ordering::SeqCst), 0);
    // Transcription vide.
    *rig.model.text.lock() = "  ".into();
    probe.emit(ListenEvent::SpeechStarted);
    rig.wait_phase(Hearing).await;
    probe.emit(ListenEvent::Utterance(speech()));
    rig.wait("the transcription", |rig| {
        rig.model.transcriptions.load(Ordering::SeqCst) == 1
            && rig.phase() == VoiceModePhase::Listening
    })
    .await;
    assert!(rig.host.utterances.lock().is_empty());
    assert_eq!(rig.state().turn, 0);
    assert_eq!(rig.state().notice, None);
    // Parole trop courte : écartée par l'écoute.
    probe.emit(ListenEvent::SpeechStarted);
    rig.wait_phase(Hearing).await;
    probe.emit(ListenEvent::Discarded);
    rig.wait_phase(VoiceModePhase::Listening).await;
}

#[tokio::test]
async fn a_failed_transcription_gives_a_notice() {
    let rig = rig();
    rig.started().await;
    *rig.model.error.lock() = Some("Transcription failed: boom".into());
    let probe = rig.probe();
    probe.emit(ListenEvent::Utterance(speech()));
    rig.wait("the failure", |rig| rig.state().notice.is_some())
        .await;
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    assert_eq!(
        rig.state().notice,
        Some(VoiceModeNotice::Failed {
            message: "Transcription failed: boom".into()
        })
    );
    // Effacé au tour suivant.
    *rig.model.error.lock() = None;
    rig.say().await;
    assert_eq!(rig.state().notice, None);
}

#[tokio::test]
async fn speaking_over_the_answer_cuts_it() {
    let rig = rig();
    rig.started().await;
    rig.speaking().await;
    let probe = rig.probe();
    probe.emit(ListenEvent::BargeIn);
    rig.wait_phase(Hearing).await;
    assert_eq!(rig.stop_readings(), 1);
    assert_eq!(rig.stopped_replies(), [CHAT]);
    // La fin de la lecture arrêtée ne change rien.
    rig.core
        .on_reading(CHAT, VoiceReading::Ended(ReadingEnd::Stopped));
    assert_eq!(rig.phase(), Hearing);
    // La parole devient le tour suivant.
    probe.emit(ListenEvent::Utterance(speech()));
    rig.wait_phase(Responding).await;
    assert_eq!(rig.state().turn, 2);
}

#[tokio::test]
async fn interrupt_stops_the_reading_and_the_reply() {
    let rig = rig();
    rig.started().await;
    // Sans effet hors d'une réponse.
    rig.core.interrupt();
    assert_eq!(rig.stop_readings(), 0);
    rig.say().await;
    rig.sent();
    rig.core.interrupt();
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    assert_eq!(rig.probe().gate(), Gate::Listen);
    assert_eq!(rig.stop_readings(), 1);
    assert_eq!(rig.stopped_replies(), [CHAT]);
    rig.speaking().await;
    rig.core.interrupt();
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    assert_eq!(rig.stop_readings(), 2);
    assert_eq!(rig.stopped_replies(), [CHAT, CHAT]);
}

#[tokio::test]
async fn a_send_that_arrives_after_its_turn_was_interrupted_is_stopped() {
    let rig = rig();
    rig.started().await;
    rig.say().await;
    // Interrompu avant que l'envoi n'arrive : rien à arrêter encore.
    rig.core.interrupt();
    assert!(rig.stopped_replies().is_empty());
    rig.core
        .observe("chat-late", &user_message(), Some(&voice()));
    assert_eq!(rig.stopped_replies(), ["chat-late"]);
    assert_eq!(rig.stop_readings(), 2);
    // Un envoi ordinaire (sans `voice`) n'est pas concerné.
    rig.core.observe("chat-typed", &user_message(), None);
    assert_eq!(rig.stopped_replies(), ["chat-late"]);
}

#[tokio::test]
async fn push_to_talk_records_while_held_and_cuts_the_answer() {
    let mut ready = prepared();
    ready.settings.turn_mode = VoiceTurnMode::PushToTalk;
    let rig = rig_with(ready, Duration::from_secs(60), |_| Ok(()));
    rig.started().await;
    let probe = rig.probe();
    assert_eq!(probe.config.mode, TurnMode::PushToTalk);
    rig.core.hold(true);
    assert_eq!(rig.phase(), Hearing);
    rig.core.hold(false);
    assert_eq!(*probe.holds.lock(), [true, false]);
    probe.emit(ListenEvent::Utterance(speech()));
    rig.wait_phase(Responding).await;
    rig.sent();
    rig.core.on_reading(CHAT, VoiceReading::Playing);
    // Appui pendant la réponse : coupée d'abord.
    rig.core.hold(true);
    assert_eq!(rig.phase(), Hearing);
    assert_eq!(rig.stop_readings(), 1);
    assert_eq!(rig.stopped_replies(), [CHAT]);
    assert_eq!(*probe.holds.lock(), [true, false, true]);
}

#[tokio::test]
async fn hold_is_ignored_hands_free() {
    let rig = rig();
    rig.started().await;
    rig.core.hold(true);
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    assert!(rig.probe().holds.lock().is_empty());
}

#[tokio::test]
async fn end_turn_ends_a_phrase_in_progress_only() {
    let rig = rig();
    rig.started().await;
    let probe = rig.probe();
    rig.core.end_turn();
    assert_eq!(probe.end_turns.load(Ordering::SeqCst), 0);
    probe.emit(ListenEvent::SpeechStarted);
    rig.wait_phase(Hearing).await;
    rig.core.end_turn();
    assert_eq!(probe.end_turns.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn muting_closes_the_microphone() {
    let rig = rig();
    rig.started().await;
    let first = rig.probe();
    first.emit(ListenEvent::SpeechStarted);
    rig.wait_phase(Hearing).await;
    rig.core.mute(true).await.unwrap();
    rig.wait("the microphone closed", |_| first.closed()).await;
    let state = rig.state();
    assert!(state.muted);
    // La parole en cours est perdue.
    assert_eq!(state.phase, VoiceModePhase::Listening);
    // Plus rien n'arrive du micro fermé.
    first.emit(ListenEvent::SpeechStarted);
    first.emit(ListenEvent::Levels {
        input: 1.0,
        output: 0.0,
    });
    rig.settle().await;
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    assert!(rig.host.levels.lock().is_empty());

    rig.core.mute(false).await.unwrap();
    assert_eq!(rig.opens(), 2);
    assert!(!rig.state().muted);
    // Micro rouvert : l'ancienne écoute reste ignorée.
    first.emit(ListenEvent::SpeechStarted);
    rig.settle().await;
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    let second = rig.probe();
    second.emit(ListenEvent::SpeechStarted);
    rig.wait_phase(Hearing).await;
}

#[tokio::test]
async fn a_muted_session_keeps_reading_the_answer() {
    let rig = rig();
    rig.started().await;
    rig.speaking().await;
    rig.core.mute(true).await.unwrap();
    assert_eq!(rig.phase(), Speaking);
    assert_eq!(rig.stop_readings(), 0);
    rig.core
        .on_reading(CHAT, VoiceReading::Ended(ReadingEnd::Finished));
    // Muet, pas en pause ; micro toujours fermé.
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    assert!(rig.state().muted);
    assert_eq!(rig.opens(), 1);
    // Rouvert pendant une réponse : gate de la réponse.
    rig.core.mute(false).await.unwrap();
    rig.speaking().await;
    rig.core.mute(true).await.unwrap();
    rig.core.mute(false).await.unwrap();
    assert_eq!(rig.opens(), 3);
    assert_eq!(rig.probe().gate(), Gate::Respond);
    rig.core
        .on_reading(CHAT, VoiceReading::Ended(ReadingEnd::Finished));
    assert_eq!(rig.probe().gate(), Gate::Listen);
}

impl Rig {
    /// Séance et étape courantes (délai de pause armé maintenant).
    fn step(&self) -> (u64, u64) {
        let inner = self.core.inner.lock();
        (inner.session, inner.step)
    }
}

#[tokio::test]
async fn two_minutes_without_speech_pause_the_session() {
    let rig = rig_with(prepared(), Duration::from_millis(150), |_| Ok(()));
    rig.started().await;
    let first = rig.probe();
    rig.wait_phase(Paused).await;
    rig.wait("the microphone closed", |_| first.closed()).await;
    // Modèle de transcription relâché.
    assert!(rig.core.inner.lock().hold.is_none());

    rig.core.resume().await.unwrap();
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    assert_eq!(rig.opens(), 2);
    assert!(rig.core.inner.lock().hold.is_some());
    assert!(!rig.probe().closed());
}

#[tokio::test]
async fn speech_puts_the_pause_off() {
    let rig = rig();
    rig.started().await;
    let (session, step) = rig.step();
    let probe = rig.probe();
    probe.emit(ListenEvent::SpeechStarted);
    rig.wait_phase(Hearing).await;
    probe.emit(ListenEvent::Discarded);
    rig.wait_phase(VoiceModePhase::Listening).await;
    // Délai armé avant la parole : périmé.
    rig.core.pause_if_idle(session, step);
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    let (session, step) = rig.step();
    rig.core.pause_if_idle(session, step);
    assert_eq!(rig.phase(), Paused);
}

#[tokio::test]
async fn a_muted_session_does_not_pause() {
    let rig = rig_with(prepared(), Duration::from_millis(300), |_| Ok(()));
    rig.started().await;
    rig.core.mute(true).await.unwrap();
    let (session, step) = rig.step();
    rig.core.pause_if_idle(session, step);
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    // Une fois rouvert, la pause revient.
    rig.core.mute(false).await.unwrap();
    rig.wait_phase(Paused).await;
}

#[tokio::test]
async fn a_lost_microphone_pauses_with_a_notice() {
    let rig = rig();
    rig.started().await;
    let first = rig.probe();
    first.emit(ListenEvent::Failed("Microphone lost".into()));
    rig.wait_phase(Paused).await;
    rig.wait("the microphone closed", |_| first.closed()).await;
    assert_eq!(
        rig.state().notice,
        Some(VoiceModeNotice::Failed {
            message: "Microphone lost".into()
        })
    );
    rig.core.resume().await.unwrap();
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    assert_eq!(rig.state().notice, None);

    // Perdu pendant la lecture : elle continue, puis pause.
    rig.speaking().await;
    rig.probe()
        .emit(ListenEvent::Failed("Microphone lost".into()));
    rig.wait("the notice", |rig| rig.state().notice.is_some())
        .await;
    assert_eq!(rig.phase(), Speaking);
    rig.core
        .on_reading(CHAT, VoiceReading::Ended(ReadingEnd::Finished));
    assert_eq!(rig.phase(), Paused);
    // L'avis du micro perdu reste jusqu'à la reprise.
    assert_eq!(rig.state().notice, lost());
}

fn lost() -> Option<VoiceModeNotice> {
    Some(VoiceModeNotice::Failed {
        message: "Microphone lost".into(),
    })
}

#[tokio::test]
async fn a_microphone_lost_while_transcribing_keeps_its_notice() {
    let rig = rig();
    rig.started().await;
    let (go, gate) = std::sync::mpsc::channel();
    *rig.model.gate.lock() = Some(gate);
    let probe = rig.probe();
    probe.emit(ListenEvent::Utterance(speech()));
    rig.wait_phase(Transcribing).await;
    probe.emit(ListenEvent::Failed("Microphone lost".into()));
    rig.wait("the notice", |rig| rig.state().notice.is_some())
        .await;
    go.send(()).unwrap();
    rig.wait_phase(Responding).await;
    rig.sent();
    rig.core.on_reading(CHAT, VoiceReading::Playing);
    rig.core
        .on_reading(CHAT, VoiceReading::Ended(ReadingEnd::Finished));
    assert_eq!(rig.phase(), Paused);
    assert_eq!(rig.state().notice, lost());
    rig.core.resume().await.unwrap();
    assert_eq!(rig.state().notice, None);
    // Pause ordinaire ensuite : sans avis.
    let (session, step) = rig.step();
    rig.core.pause_if_idle(session, step);
    assert_eq!(rig.phase(), Paused);
    assert_eq!(rig.state().notice, None);
}

#[tokio::test]
async fn a_microphone_that_does_not_reopen_keeps_the_pause() {
    let rig = rig_with(prepared(), Duration::from_millis(30), |_| Ok(()));
    rig.started().await;
    rig.wait_phase(Paused).await;
    *rig.ear.refusal.lock() = Some("No microphone found".into());
    assert_eq!(
        rig.core.resume().await,
        Err("No microphone found".to_owned())
    );
    assert_eq!(rig.phase(), Paused);
    assert_eq!(
        rig.state().notice,
        Some(VoiceModeNotice::Failed {
            message: "No microphone found".into()
        })
    );
}

/// Arrête la séance dans la phase où `reach` l'a menée.
async fn stops_from(reach: impl AsyncFnOnce(&Rig)) {
    let rig = rig_with(prepared(), Duration::from_millis(100), |_| Ok(()));
    rig.started().await;
    reach(&rig).await;
    let before = rig.stop_readings();
    let probe = rig.probe();
    rig.core.stop(false);
    let state = rig.state();
    assert_eq!(state, VoiceModeState::default());
    rig.wait("the microphone closed", |_| probe.closed()).await;
    assert!(!rig.host.reading.load(Ordering::SeqCst));
    assert!(!rig.host.reserved.load(Ordering::SeqCst));
    assert_eq!(rig.stop_readings(), before + 1);
    assert!(rig.core.inner.lock().hold.is_none());
    // Plus rien ne la ranime.
    probe.emit(ListenEvent::Utterance(speech()));
    rig.core.on_reading(CHAT, VoiceReading::Playing);
    rig.settle().await;
    assert_eq!(rig.phase(), Off);
    assert_eq!(rig.host.states.lock().last().unwrap().phase, Off);
}

#[tokio::test]
async fn stop_ends_the_session_while_listening_or_hearing() {
    stops_from(async |_| {}).await;
    stops_from(async |rig| {
        rig.probe().emit(ListenEvent::SpeechStarted);
        rig.wait_phase(Hearing).await;
    })
    .await;
}

#[tokio::test]
async fn stop_ends_the_session_while_responding_or_speaking() {
    stops_from(async |rig| rig.say().await).await;
    stops_from(async |rig| rig.speaking().await).await;
}

#[tokio::test]
async fn stop_ends_the_session_while_paused() {
    stops_from(async |rig| rig.wait_phase(Paused).await).await;
}

#[tokio::test]
async fn a_transcription_finished_after_stop_is_dropped() {
    let rig = rig();
    rig.started().await;
    let (go, gate) = std::sync::mpsc::channel();
    *rig.model.gate.lock() = Some(gate);
    rig.probe().emit(ListenEvent::Utterance(speech()));
    rig.wait("the transcription", |rig| {
        rig.model.transcriptions.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(rig.phase(), Transcribing);
    rig.core.stop(false);
    go.send(()).unwrap();
    rig.settle().await;
    assert_eq!(rig.phase(), Off);
    assert!(rig.host.utterances.lock().is_empty());
    // Une nouvelle séance repart de zéro.
    rig.started().await;
    rig.say().await;
    assert_eq!(rig.state().turn, 1);
}

#[tokio::test]
async fn stop_while_starting_leaves_nothing_open() {
    let rig = rig();
    let gate = Arc::new(tokio::sync::Notify::new());
    *rig.host.prepare_gate.lock() = Some(gate.clone());
    let core = rig.core.clone();
    let starting = tokio::spawn(async move { core.start().await });
    rig.wait_phase(Starting).await;
    rig.core.stop(false);
    assert_eq!(rig.phase(), Off);
    gate.notify_one();
    assert_eq!(starting.await.unwrap(), Ok(()));
    assert_eq!(rig.phase(), Off);
    rig.wait("the microphone closed", |rig| {
        rig.ear.probes.lock().iter().all(|probe| probe.closed())
    })
    .await;
    assert!(!rig.host.reading.load(Ordering::SeqCst));
    assert!(!rig.host.reserved.load(Ordering::SeqCst));
}

#[tokio::test]
async fn start_is_idempotent() {
    let rig = rig();
    rig.started().await;
    rig.core.start().await.unwrap();
    assert_eq!(rig.opens(), 1);
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
}

#[tokio::test]
async fn stale_events_are_ignored() {
    let rig = rig();
    rig.started().await;
    let probe = rig.probe();
    // Hors de leur phase.
    probe.emit(ListenEvent::Discarded);
    probe.emit(ListenEvent::BargeIn);
    rig.core.on_reading(CHAT, VoiceReading::Playing);
    rig.settle().await;
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    assert_eq!(rig.stop_readings(), 0);
    rig.say().await;
    // Une phrase pendant la réponse (gate Respond) ne fait rien.
    probe.emit(ListenEvent::Utterance(speech()));
    rig.settle().await;
    assert_eq!(rig.phase(), Responding);
    assert_eq!(rig.model.transcriptions.load(Ordering::SeqCst), 1);
    rig.sent();
    // Lecture d'une autre conversation.
    rig.core.on_reading("chat-other", VoiceReading::Playing);
    rig.core
        .on_reading("chat-other", VoiceReading::Ended(ReadingEnd::Finished));
    assert_eq!(rig.phase(), Responding);
    rig.core.on_reading(CHAT, VoiceReading::Playing);
    assert_eq!(rig.phase(), Speaking);
}

#[tokio::test]
async fn a_refused_send_gives_a_notice() {
    let rig = rig();
    rig.started().await;
    rig.say().await;
    rig.core.reply_failed(2, "Wrong turn".into());
    assert_eq!(rig.phase(), Responding);
    rig.core
        .reply_failed(1, "This chat is already waiting for an answer".into());
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    assert_eq!(
        rig.state().notice,
        Some(VoiceModeNotice::Failed {
            message: "This chat is already waiting for an answer".into()
        })
    );
    rig.say().await;
    assert_eq!(rig.state().notice, None);
}

#[tokio::test]
async fn the_end_of_a_reading_tells_why_nothing_was_read() {
    for (end, notice) in [
        (ReadingEnd::Finished, None),
        (ReadingEnd::Stopped, None),
        (ReadingEnd::NothingToRead, None),
        (
            ReadingEnd::Unsupported,
            Some(VoiceModeNotice::UnsupportedLanguage),
        ),
        (
            ReadingEnd::MissingVoice(VoiceLanguage::Es),
            Some(VoiceModeNotice::MissingVoice {
                language: VoiceLanguage::Es,
            }),
        ),
        (
            ReadingEnd::Failed(ReadAloudError::NoOutputDevice),
            Some(VoiceModeNotice::ReadFailed {
                error: ReadAloudError::NoOutputDevice,
            }),
        ),
    ] {
        let rig = rig();
        rig.started().await;
        rig.say().await;
        rig.sent();
        // Fin sans premier son (réponse non lue).
        rig.core.on_reading(CHAT, VoiceReading::Ended(end));
        assert_eq!(rig.phase(), VoiceModePhase::Listening, "{end:?}");
        assert_eq!(rig.state().notice, notice, "{end:?}");
    }
}

#[tokio::test]
async fn voice_mode_does_not_start_during_a_dictation() {
    let rig = rig();
    rig.host.dictating.store(true, Ordering::SeqCst);
    assert_eq!(
        rig.core.start().await,
        Err("A dictation is in progress: finish it first".to_owned())
    );
    assert_eq!(rig.phase(), Off);
    assert_eq!(rig.opens(), 0);
    assert!(rig.host.states.lock().is_empty());
}

#[tokio::test]
async fn missing_requirements_refuse_to_start() {
    let unsupported: Support = |_| Err("This processor cannot run Whisper models".into());
    let mut no_speech = prepared();
    no_speech.requirements.speech_model_installed = false;
    let mut no_voice = prepared();
    no_voice.requirements.voice_installed = false;
    let mut unavailable = prepared();
    unavailable.requirements.unavailable = Some("ONNX Runtime is missing".into());
    let cases: Vec<(Prepared, Support, Option<&str>, &str)> = vec![
        (no_speech, |_| Ok(()), None, "Download a speech model first"),
        (
            no_voice,
            |_| Ok(()),
            None,
            "Download a voice for reading aloud first",
        ),
        (unavailable, |_| Ok(()), None, "ONNX Runtime is missing"),
        (
            prepared(),
            unsupported,
            None,
            "This processor cannot run Whisper models",
        ),
        (
            prepared(),
            |_| Ok(()),
            Some("No microphone found"),
            "No microphone found",
        ),
    ];
    for (ready, support, microphone, refusal) in cases {
        let rig = rig_with(ready, Duration::from_secs(60), support);
        *rig.ear.refusal.lock() = microphone.map(str::to_owned);
        assert_eq!(rig.core.start().await, Err(refusal.to_owned()));
        assert_eq!(rig.phase(), Off, "{refusal}");
        assert_eq!(rig.phases(), [Starting, Off], "{refusal}");
        assert!(!rig.host.reserved.load(Ordering::SeqCst), "{refusal}");
        assert!(!rig.host.reading.load(Ordering::SeqCst), "{refusal}");
        assert!(rig.core.inner.lock().hold.is_none());
    }
    // Préparation impossible (base illisible).
    let rig = rig();
    *rig.host.prepared.lock() = Err("The database could not be opened".into());
    assert_eq!(
        rig.core.start().await,
        Err("The database could not be opened".to_owned())
    );
    assert_eq!(rig.phase(), Off);
}

#[tokio::test]
async fn the_speech_model_used_becomes_the_dictation_one() {
    let mut ready = prepared();
    ready.remember = true;
    let rig = rig_with(ready, Duration::from_secs(60), |_| Ok(()));
    rig.started().await;
    assert_eq!(*rig.host.remembered.lock(), ["whisper-tiny"]);
    let rig = self::rig();
    rig.started().await;
    assert!(rig.host.remembered.lock().is_empty());
}

#[tokio::test]
async fn deleting_the_chat_of_the_session_ends_it() {
    let rig = rig();
    rig.started().await;
    rig.speaking().await;
    rig.core
        .on_reading(CHAT, VoiceReading::Ended(ReadingEnd::Finished));
    rig.core.chat_deleted("chat-other");
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    rig.core.chat_deleted(CHAT);
    assert_eq!(rig.phase(), Off);
}

#[tokio::test]
async fn new_settings_apply_to_the_session() {
    let rig = rig();
    rig.started().await;
    let settings = VoiceModeSettings {
        turn_mode: VoiceTurnMode::PushToTalk,
        pause: VoicePause::Short,
        barge_in: false,
        ..VoiceModeSettings::default()
    };
    rig.core.settings_changed(&settings);
    let probe = rig.probe();
    assert_eq!(
        *probe.configs.lock(),
        [ListenConfig {
            mode: TurnMode::PushToTalk,
            silence: Duration::from_millis(600),
            barge_in: false,
        }]
    );
    // Push-to-talk désormais.
    rig.core.hold(true);
    assert_eq!(rig.phase(), Hearing);
}

#[tokio::test]
async fn shutdown_closes_the_microphone_before_returning() {
    let rig = rig();
    rig.started().await;
    let probe = rig.probe();
    rig.core.inner.lock().closed = true;
    rig.core.stop(true);
    assert!(probe.closed());
    // Plus rien ne démarre.
    rig.core.start().await.unwrap();
    assert_eq!(rig.phase(), Off);
    assert_eq!(rig.opens(), 1);
}

#[test]
fn the_phrase_language_falls_back_to_the_expected_voice() {
    // Repli : la langue attendue (indice de la voix), pas l'interface.
    assert_eq!(
        crate::tts::text::choose_language("OK.", VoiceLanguage::Fr),
        Some(VoiceLanguage::Fr)
    );
}

#[test]
fn the_timing_line_counts_from_the_end_of_speech() {
    let timing = Timing {
        turn: 3,
        ended: Instant::now(),
        ended_ms: 1_700_000_000_000,
        transcribed: Duration::from_millis(420),
        utterance: Duration::from_millis(425),
    };
    assert_eq!(
        timing.line(Some(Duration::from_millis(1_890))),
        "chatonlm-voice-timing turn=3 speech_end_ms=1700000000000 transcribed_ms=420 utterance_ms=425 first_sound_ms=1890"
    );
    assert!(timing.line(None).ends_with("first_sound_ms=none"));
}

fn dictation(speech_model: &str, language: &str) -> DictationSettings {
    DictationSettings {
        enabled: false,
        mode: crate::store::dictation::DictationMode::Toggle,
        speech_model: speech_model.into(),
        language: language.into(),
        polish_model: String::new(),
        polish_prompt: String::new(),
        dictionary: vec!["Ollama".into()],
        notify_text: true,
        auto_copy: true,
        auto_paste: false,
    }
}

/// Crée les fichiers de `files` (taille attendue) sous `root/id`.
fn install(root: &Path, id: &str, files: &[(&str, u64)]) {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    for (path, size) in files {
        std::fs::File::create(dir.join(path))
            .unwrap()
            .set_len(*size)
            .unwrap();
    }
}

fn install_speech(root: &Path, id: &str) {
    let model = speech_models::find(id).unwrap();
    let files: Vec<_> = model.files.iter().map(|f| (f.path, f.size)).collect();
    install(root, id, &files);
}

#[test]
fn the_speech_model_is_the_dictation_one_or_an_installed_one() {
    let installed = |id: &'static str| move |model: &CatalogModel| model.id == id;
    let none = |_: &CatalogModel| false;
    // Réglage de la dictée, installé ou non.
    assert_eq!(
        speech_model(&dictation("whisper-base", "auto"), &none).id,
        "whisper-base"
    );
    // Sinon le premier installé du catalogue.
    assert_eq!(
        speech_model(&dictation("", "auto"), &installed("whisper-small")).id,
        "whisper-small"
    );
    // Sinon le recommandé.
    assert_eq!(
        speech_model(&dictation("", "auto"), &none).id,
        "parakeet-tdt-0.6b-v3"
    );
    assert_eq!(
        speech_model(&dictation("gone", "auto"), &none).id,
        "parakeet-tdt-0.6b-v3"
    );
}

#[test]
fn the_expected_voice_follows_the_dictation_language() {
    let interface = VoiceLanguage::En;
    assert_eq!(
        expected_language(&dictation("", "es"), interface),
        VoiceLanguage::Es
    );
    assert_eq!(
        expected_language(&dictation("", "fr"), interface),
        VoiceLanguage::Fr
    );
    // Auto, ou une langue sans voix : celle de l'interface.
    assert_eq!(
        expected_language(&dictation("", "auto"), VoiceLanguage::Fr),
        VoiceLanguage::Fr
    );
    assert_eq!(
        expected_language(&dictation("", "de"), interface),
        VoiceLanguage::En
    );
}

#[test]
fn requirements_say_what_is_missing() {
    let speech = tempfile::tempdir().unwrap();
    let voices = tempfile::tempdir().unwrap();
    let settings = VoiceModeSettings::default();
    // Rien d'installé : le recommandé et la voix de la dictée, à télécharger.
    let ready = gather(
        &dictation("", "es"),
        settings.clone(),
        speech.path(),
        voices.path(),
        VoiceLanguage::En,
        |_| None,
    );
    let parakeet = speech_models::find("parakeet-tdt-0.6b-v3").unwrap();
    let voice = crate::tts::models::for_language(VoiceLanguage::Es).unwrap();
    assert_eq!(
        ready.requirements,
        VoiceModeRequirements {
            speech_model: parakeet.id.into(),
            speech_model_installed: false,
            speech_model_bytes: parakeet.size(),
            voice_model: voice.id.into(),
            voice_language: VoiceLanguage::Es,
            voice_installed: false,
            voice_bytes: voice.size(),
            unavailable: None,
        }
    );
    assert!(!ready.remember);
    assert_eq!(
        ready.request,
        Request {
            language: Some("es".into()),
            glossary: vec!["Ollama".into()],
        }
    );

    // Installés : rien à télécharger ; le modèle sera retenu par la dictée.
    install_speech(speech.path(), "whisper-tiny");
    let files: Vec<_> = voice.files.iter().map(|f| (f.path, f.size)).collect();
    install(voices.path(), voice.id, &files);
    let ready = gather(
        &dictation("", "auto"),
        settings.clone(),
        speech.path(),
        voices.path(),
        VoiceLanguage::Es,
        |engine| (engine == SpeechEngine::Whisper).then(|| "no AVX2".to_owned()),
    );
    let requirements = &ready.requirements;
    assert_eq!(requirements.speech_model, "whisper-tiny");
    assert!(requirements.speech_model_installed && requirements.voice_installed);
    assert_eq!(
        (requirements.speech_model_bytes, requirements.voice_bytes),
        (0, 0)
    );
    assert_eq!(requirements.unavailable.as_deref(), Some("no AVX2"));
    assert!(ready.remember);
    assert_eq!(ready.request.language, None);
    assert_eq!(ready.speech.dir, speech.path().join("whisper-tiny"));
    // Déjà réglé : rien à retenir.
    let ready = gather(
        &dictation("whisper-tiny", "auto"),
        settings,
        speech.path(),
        voices.path(),
        VoiceLanguage::Es,
        |_| None,
    );
    assert!(!ready.remember);
}

#[tokio::test]
async fn a_failed_reply_gives_its_error_at_the_end_of_the_turn() {
    let rig = rig();
    rig.started().await;
    rig.say().await;
    rig.sent();
    // Ce qui a été écrit est lu, puis l'avis.
    rig.core
        .on_reading(CHAT, VoiceReading::ReplyFailed("Connection lost".into()));
    assert_eq!(rig.phase(), Responding);
    rig.core.on_reading(CHAT, VoiceReading::Playing);
    rig.core
        .on_reading(CHAT, VoiceReading::Ended(ReadingEnd::Finished));
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    let failed = Some(VoiceModeNotice::Failed {
        message: "Connection lost".into(),
    });
    assert_eq!(rig.state().notice, failed);
    // Avant l'avis de la lecture.
    rig.say().await;
    assert_eq!(rig.state().notice, None);
    rig.sent();
    rig.core
        .on_reading(CHAT, VoiceReading::ReplyFailed("Connection lost".into()));
    rig.core
        .on_reading(CHAT, VoiceReading::Ended(ReadingEnd::NothingToRead));
    assert_eq!(rig.state().notice, failed);
    // Effacée au tour suivant : une fin ordinaire, sans avis.
    rig.say().await;
    rig.sent();
    rig.core
        .on_reading(CHAT, VoiceReading::Ended(ReadingEnd::Finished));
    assert_eq!(rig.state().notice, None);
}

#[tokio::test]
async fn a_send_that_fails_before_its_message_ends_the_turn() {
    let rig = rig();
    rig.started().await;
    rig.say().await;
    let error = ChatEvent::Error {
        kind: crate::chat::ChatErrorKind::Internal,
        message: "database error: disk full".into(),
    };
    // Envoi ordinaire : pas celui du tour.
    rig.core.observe(CHAT, &error, None);
    assert_eq!(rig.phase(), Responding);
    rig.core.observe(CHAT, &error, Some(&voice()));
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    assert_eq!(
        rig.state().notice,
        Some(VoiceModeNotice::Failed {
            message: "database error: disk full".into()
        })
    );
    // Arrêté avant son message : fin sans avis.
    rig.say().await;
    rig.core
        .observe(CHAT, &ChatEvent::Cancelled, Some(&voice()));
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    assert_eq!(rig.state().notice, None);
    // Message écrit : la fin vient de la lecture, pas de l'erreur.
    rig.say().await;
    rig.sent();
    rig.core.observe(CHAT, &error, Some(&voice()));
    assert_eq!(rig.phase(), Responding);
}

#[tokio::test]
async fn another_reading_raises_the_listening_threshold() {
    let rig = rig();
    rig.started().await;
    let probe = rig.probe();
    rig.core.on_reading("", VoiceReading::Aside(true));
    assert_eq!(probe.gate(), Gate::Respond);
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    rig.core.on_reading("", VoiceReading::Aside(false));
    assert_eq!(probe.gate(), Gate::Listen);
    // Parole franche pendant l'autre lecture : arrêtée, la parole est le
    // tour suivant.
    rig.core.on_reading("", VoiceReading::Aside(true));
    probe.emit(ListenEvent::BargeIn);
    rig.wait_phase(Hearing).await;
    assert_eq!(rig.host.stop_any_readings.load(Ordering::SeqCst), 1);
    assert_eq!(rig.stop_readings(), 0);
    rig.core.on_reading("", VoiceReading::Aside(false));
    probe.emit(ListenEvent::Utterance(speech()));
    rig.wait_phase(Responding).await;
    // Fin de la réponse pendant l'autre lecture : retour en Respond.
    rig.sent();
    rig.core.on_reading("", VoiceReading::Aside(true));
    rig.core
        .on_reading(CHAT, VoiceReading::Ended(ReadingEnd::Stopped));
    assert_eq!(rig.phase(), VoiceModePhase::Listening);
    assert_eq!(probe.gate(), Gate::Respond);
}

#[tokio::test]
async fn a_microphone_that_does_not_reopen_after_mute_gives_a_notice() {
    let rig = rig();
    rig.started().await;
    rig.core.mute(true).await.unwrap();
    *rig.ear.refusal.lock() = Some("No microphone found".into());
    assert_eq!(
        rig.core.mute(false).await,
        Err("No microphone found".to_owned())
    );
    let state = rig.state();
    assert!(state.muted);
    assert_eq!(
        state.notice,
        Some(VoiceModeNotice::Failed {
            message: "No microphone found".into()
        })
    );
    assert_eq!(rig.host.states.lock().last(), Some(&state));
}
