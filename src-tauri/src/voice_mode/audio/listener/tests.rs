use std::sync::atomic::AtomicBool;

use super::*;
use crate::dictation::audio::resample;
use crate::voice_mode::audio::TurnMode;
use crate::voice_mode::audio::echo::tests::{echo_of, voice_like};

/// Détection par le niveau : parole au-dessus de -45 dBFS (sans ONNX).
struct Loudness;

impl SpeechDetector for Loudness {
    fn probability(&mut self, frame: &[f32]) -> Result<f32, String> {
        let power = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
        Ok(if 10.0 * power.max(1e-12).log10() > -45.0 {
            1.0
        } else {
            0.0
        })
    }
}

fn loudness() -> Result<Box<dyn SpeechDetector>, String> {
    Ok(Box::new(Loudness))
}

fn hands_free() -> ListenConfig {
    ListenConfig {
        mode: TurnMode::HandsFree,
        silence: Duration::from_millis(600),
        barge_in: true,
    }
}

/// Moniteur à soi (celui des sorties réelles est partagé par les essais).
fn private_monitor() -> &'static Monitor {
    Box::leak(Box::new(Monitor::new()))
}

/// Sinus de 440 Hz.
fn tone(seconds: f32, amplitude: f32, rate: u32) -> Vec<f32> {
    tone_at(440.0, seconds, amplitude, rate)
}

fn tone_at(frequency: f32, seconds: f32, amplitude: f32, rate: u32) -> Vec<f32> {
    (0..(seconds * rate as f32) as usize)
        .map(|i| amplitude * (std::f32::consts::TAU * frequency * i as f32 / rate as f32).sin())
        .collect()
}

/// Laisse au fil le temps d'appliquer une commande.
fn settle() {
    std::thread::sleep(Duration::from_millis(40));
}

/// Passe `render` et `capture` (16 kHz) par morceaux de 20 ms ; la gate
/// passe à `gate` après `after` secondes.
fn run_pipeline(
    render: Option<&[f32]>,
    capture: &[f32],
    gate: Gate,
    after: f32,
) -> Vec<ListenEvent> {
    let mut pipeline = Pipeline::new(Box::new(Loudness), Box::new(Loudness), hands_free());
    let mut events = Vec::new();
    pipeline.command(Command::Gate(Gate::Ignore), &mut |e| events.push(e));
    let switch = (after * RATE as f32) as usize / 320;
    for (index, chunk) in capture.chunks(320).enumerate() {
        if index == switch {
            pipeline.command(Command::Gate(gate), &mut |e| events.push(e));
        }
        if let Some(render) = render {
            pipeline.push_render(&render[index * 320..(index * 320 + 320).min(render.len())]);
        }
        pipeline
            .push_capture(chunk, &mut |e| events.push(e))
            .unwrap();
    }
    events.retain(|event| !matches!(event, ListenEvent::Levels { .. }));
    events
}

/// L'écho de la voix jouée, retiré grâce à la référence, n'interrompt pas
/// la réponse ni ne commence un tour ; sans la référence, il le ferait.
#[test]
fn the_echo_of_what_plays_is_not_speech() {
    let render = voice_like(8.0, 1);
    let capture = echo_of(&render, 80, 0.5);
    for gate in [Gate::Respond, Gate::Listen] {
        assert_eq!(
            run_pipeline(Some(&render), &capture, gate, 2.0),
            [],
            "{gate:?}"
        );
        let unfed = run_pipeline(None, &capture, gate, 2.0);
        let expected = match gate {
            Gate::Respond => ListenEvent::BargeIn,
            _ => ListenEvent::SpeechStarted,
        };
        assert_eq!(unfed.first(), Some(&expected), "{gate:?}");
    }
}

/// La référence va bien à l'annuleur : le niveau du micro (après
/// annulation) tombe de plus de 15 dB sur l'écho seul.
#[test]
fn the_canceller_gets_the_render() {
    let render = voice_like(6.0, 1);
    let capture = echo_of(&render, 80, 0.5);
    let level = |fed: bool| {
        let mut pipeline = Pipeline::new(Box::new(Loudness), Box::new(Loudness), hands_free());
        let mut levels = Vec::new();
        for (index, chunk) in capture.chunks(320).enumerate() {
            if fed {
                pipeline.push_render(&render[index * 320..index * 320 + chunk.len()]);
            }
            pipeline
                .push_capture(chunk, &mut |event| {
                    if let ListenEvent::Levels { input, .. } = event {
                        levels.push(input);
                    }
                })
                .unwrap();
        }
        // Moyenne des 2 dernières secondes.
        let tail = &levels[levels.len() - 30..];
        tail.iter().sum::<f32>() / tail.len() as f32
    };
    let (fed, unfed) = (level(true), level(false));
    assert!(unfed - fed > 0.25, "fed {fed}, unfed {unfed}");
}

/// La voix proche, pendant que la sortie joue, interrompt la réponse et
/// devient le tour suivant.
#[test]
fn near_speech_over_the_echo_barges_in() {
    let render = voice_like(8.0, 1);
    let near = voice_like(8.0, 7);
    let capture: Vec<f32> = echo_of(&render, 80, 0.5)
        .iter()
        .zip(&near)
        .enumerate()
        .map(|(index, (echo, near))| {
            echo + if (4 * RATE as usize..6 * RATE as usize).contains(&index) {
                *near
            } else {
                0.0
            }
        })
        .collect();
    let events = run_pipeline(Some(&render), &capture, Gate::Respond, 2.0);
    assert_eq!(events.first(), Some(&ListenEvent::BargeIn), "{events:?}");
    assert!(
        matches!(events.get(1), Some(ListenEvent::Utterance(_))),
        "{events:?}"
    );
}

/// Casque (aucun écho), lecture commencée par 300 ms de silence (tampon de
/// départ de la sortie) : la voix jouée, et un souffle à -70 dBFS au micro.
fn headphones(seconds: f32) -> (Vec<f32>, Vec<f32>) {
    use crate::voice_mode::audio::noise::Noise;
    let mut render = vec![0.0; 3 * RATE as usize / 10];
    render.extend(voice_like(seconds - 0.3, 1));
    let mut noise = Noise::new(11);
    let capture = (0..render.len()).map(|_| noise.next() * 0.000_55).collect();
    (render, capture)
}

/// Ajoute `voice` au micro à partir de `at` secondes.
fn speak_at(capture: &mut [f32], at: f32, voice: &[f32]) {
    let start = (at * RATE as f32) as usize;
    for (slot, sample) in capture[start..].iter_mut().zip(voice) {
        *slot += sample;
    }
}

/// Voix proche à ~-28 dBFS (autre voix que celle jouée).
fn near_voice(seconds: f32) -> Vec<f32> {
    voice_like(seconds, 7).iter().map(|s| s * 0.3).collect()
}

/// Casque : la voix proche, 1 s ou 1,5 s après le début de la lecture,
/// interrompt la réponse (le couplage appris pendant le silence du micro
/// est infime) et devient le tour suivant, pré-roll compris.
#[test]
fn with_headphones_early_speech_barges_in() {
    for at in [1.0, 1.5] {
        let (render, mut capture) = headphones(6.0);
        let voice = near_voice(2.0);
        speak_at(&mut capture, at, &voice);
        let events = run_pipeline(Some(&render), &capture, Gate::Respond, 0.0);
        assert_eq!(
            events.first(),
            Some(&ListenEvent::BargeIn),
            "{at} s: {events:?}"
        );
        let Some(ListenEvent::Utterance(samples)) = events.get(1) else {
            panic!("{at} s: {events:?}")
        };
        // Les 2 s de voix et 300 ms avant le début de l'interruption.
        assert!(
            samples.len() >= (2.2 * RATE as f32) as usize,
            "{at} s: {}",
            samples.len()
        );
    }
}

/// Voix proche de 3 s, 1,2 s après le début de la lecture : interruption ;
/// le couplage n'apprend rien de cette parole, et une nouvelle réponse peut
/// encore être interrompue.
#[test]
fn long_near_speech_does_not_raise_the_coupling() {
    let (render, mut capture) = headphones(9.0);
    speak_at(&mut capture, 1.2, &near_voice(3.0));
    speak_at(&mut capture, 7.0, &near_voice(1.0));
    let mut pipeline = Pipeline::new(Box::new(Loudness), Box::new(Loudness), hands_free());
    let mut events = Vec::new();
    pipeline.command(Command::Gate(Gate::Respond), &mut |_| {});
    for (index, chunk) in capture.chunks(320).enumerate() {
        // Nouvelle réponse à 6 s.
        if index * 320 == 6 * RATE as usize {
            pipeline.command(Command::Gate(Gate::Respond), &mut |_| {});
        }
        pipeline.push_render(&render[index * 320..index * 320 + chunk.len()]);
        pipeline
            .push_capture(chunk, &mut |event| {
                if !matches!(event, ListenEvent::Levels { .. }) {
                    events.push(event);
                }
            })
            .unwrap();
    }
    let barge_ins = events
        .iter()
        .filter(|event| **event == ListenEvent::BargeIn)
        .count();
    assert_eq!(barge_ins, 2, "{events:?}");
}

/// Casque puis haut-parleurs (casque débranché pendant la lecture) : l'écho
/// qui apparaît, sans voix proche, est appris par la garde au cours de la
/// lecture ; passé ce délai, il n'interrompt plus la réponse (la détection
/// le prend pour de la parole, mais l'apprentissage ne dépend pas d'elle).
#[test]
fn an_echo_that_appears_is_learned() {
    let (render, mut capture) = headphones(14.0);
    let switch = 3 * RATE as usize;
    for (slot, echo) in capture[switch..]
        .iter_mut()
        .zip(&echo_of(&render, 80, 0.5)[switch..])
    {
        *slot += echo;
    }
    let mut pipeline = Pipeline::new(Box::new(Loudness), Box::new(Loudness), hands_free());
    let mut late = Vec::new();
    pipeline.command(Command::Gate(Gate::Respond), &mut |_| {});
    let rearm = switch + 4 * RATE as usize;
    for (index, chunk) in capture.chunks(320).enumerate() {
        let at = index * 320;
        if at == rearm {
            pipeline.command(Command::Gate(Gate::Respond), &mut |_| {});
        }
        pipeline.push_render(&render[at..at + chunk.len()]);
        pipeline
            .push_capture(chunk, &mut |event| {
                if at >= rearm && !matches!(event, ListenEvent::Levels { .. }) {
                    late.push(event);
                }
            })
            .unwrap();
    }
    assert!(
        pipeline.guard_coupling_db() > -10.0,
        "{}",
        pipeline.guard_coupling_db()
    );
    assert_eq!(late, [], "late events");
}

/// Relâché du push-to-talk : le son entendu après la dernière trame de
/// détection (moins de 32 ms) termine le tour au lieu d'être perdu.
#[test]
fn a_release_keeps_the_last_samples() {
    let mut pipeline = Pipeline::new(
        Box::new(Loudness),
        Box::new(Loudness),
        ListenConfig {
            mode: TurnMode::PushToTalk,
            ..hands_free()
        },
    );
    let mut events = Vec::new();
    pipeline
        .push_capture(&vec![0.0; RATE as usize], &mut |_| {})
        .unwrap();
    pipeline.command(Command::Hold(true), &mut |e| events.push(e));
    // 1 s de voix, plus 320 échantillons : 448 attendent une trame.
    let voice = tone(1.02, 0.1, RATE);
    pipeline
        .push_capture(&voice, &mut |e| events.push(e))
        .unwrap();
    pipeline.command(Command::Hold(false), &mut |e| events.push(e));
    events.retain(|event| !matches!(event, ListenEvent::Levels { .. }));
    let Some(ListenEvent::Utterance(samples)) = events.get(1) else {
        panic!("{events:?}")
    };
    // 300 ms avant l'appui, les 128 échantillons qui attendaient alors une
    // trame, puis tout ce qui a été dit.
    assert_eq!(samples.len(), 4_800 + 128 + voice.len());
}

/// Morceau envoyé au micro des essais.
enum Chunk {
    /// Son capté.
    Sound(Vec<f32>),
    /// Son capté, et la sortie à donner au moniteur dès qu'il est rendu :
    /// du fil de l'écoute, donc avant sa prochaine lecture de la sortie.
    /// L'avance de la référence sur le micro est ainsi exacte, quelle que
    /// soit la charge (envoyée du fil de l'essai, elle pouvait arriver après
    /// le micro). Un seul de ces morceaux par lecture.
    WithOutput {
        sound: Vec<f32>,
        output: Vec<f32>,
        output_rate: u32,
    },
    /// Micro perdu.
    Failed(String),
}

/// Micro des essais : rend ce que l'essai lui envoie ; lâché, il le
/// signale.
struct TestMic {
    chunks: mpsc::Receiver<Chunk>,
    rate: u32,
    dropped: Arc<AtomicBool>,
    monitor: &'static Monitor,
}

impl Microphone for TestMic {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn read(&mut self, out: &mut Vec<f32>, _cues: &Cues) -> Result<(), String> {
        while let Ok(chunk) = self.chunks.try_recv() {
            match chunk {
                Chunk::Sound(sound) => out.extend(sound),
                Chunk::WithOutput {
                    sound,
                    output,
                    output_rate,
                } => {
                    out.extend(sound);
                    self.monitor.feed(&output, output_rate);
                    break;
                }
                Chunk::Failed(message) => return Err(message),
            }
        }
        Ok(())
    }
}

impl Drop for TestMic {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}

/// Écoute ouverte sur un micro d'essai.
struct Opened {
    listening: Box<dyn Listening>,
    mic: mpsc::Sender<Chunk>,
    events: mpsc::Receiver<ListenEvent>,
    dropped: Arc<AtomicBool>,
    monitor: &'static Monitor,
}

impl Opened {
    fn new(config: ListenConfig, rate: u32) -> Self {
        let (mic, chunks) = mpsc::channel();
        let chunks = Mutex::new(Some(chunks));
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = dropped.clone();
        let monitor = private_monitor();
        let listener = Listener {
            microphone: Arc::new(move |_: Thread| -> Result<Box<dyn Microphone>, String> {
                Ok(Box::new(TestMic {
                    chunks: chunks.lock().take().expect("opened once"),
                    rate,
                    dropped: flag.clone(),
                    monitor,
                }))
            }),
            detector: Arc::new(loudness),
            monitor,
        };
        let (sender, events) = mpsc::channel();
        let listening = listener
            .open(
                config,
                Box::new(move |event| {
                    let _ = sender.send(event);
                }),
            )
            .unwrap();
        Self {
            listening,
            mic,
            events,
            dropped,
            monitor,
        }
    }

    fn say(&self, samples: Vec<f32>) {
        self.mic.send(Chunk::Sound(samples)).unwrap();
    }

    /// `sound` au micro, puis `output` (à `output_rate`) au moniteur, du fil
    /// de l'écoute (voir [`Chunk::WithOutput`]).
    fn say_with_output(&self, sound: Vec<f32>, output: Vec<f32>, output_rate: u32) {
        self.mic
            .send(Chunk::WithOutput {
                sound,
                output,
                output_rate,
            })
            .unwrap();
    }

    /// Prochain événement autre qu'un niveau.
    fn next(&self) -> ListenEvent {
        loop {
            match self.events.recv_timeout(Duration::from_secs(2)) {
                Ok(ListenEvent::Levels { .. }) => {}
                Ok(event) => return event,
                Err(error) => panic!("no event: {error}"),
            }
        }
    }

    /// Plus aucun événement autre qu'un niveau pendant `wait`.
    fn quiet(&self, wait: Duration) {
        let until = Instant::now() + wait;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match self.events.recv_timeout(left) {
                Ok(ListenEvent::Levels { .. }) | Err(_) => {}
                Ok(event) => panic!("unexpected {event:?}"),
            }
        }
    }
}

/// Le micro (48 kHz) est ramené à 16 kHz ; parole puis silence : début,
/// puis fin du tour, pré-roll compris.
#[test]
fn speech_from_the_microphone_makes_a_turn() {
    let opened = Opened::new(hands_free(), 48_000);
    opened.say(vec![0.0; 24_000]);
    opened.say(tone(0.5, 0.1, 48_000));
    assert_eq!(opened.next(), ListenEvent::SpeechStarted);
    opened.say(vec![0.0; 48_000]);
    let ListenEvent::Utterance(samples) = opened.next() else {
        panic!("no utterance")
    };
    // 300 ms avant, 0,5 s de son, 300 ms après (à une trame près de
    // chaque côté).
    let expected = (0.3 + 0.5 + 0.3) * RATE as f32;
    assert!(
        (samples.len() as f32 - expected).abs() <= 1_024.0,
        "{}",
        samples.len()
    );
}

/// Les commandes passent au fil : gate, fin de tour, push-to-talk,
/// réglages.
#[test]
fn commands_reach_the_listening_thread() {
    let opened = Opened::new(hands_free(), RATE);
    opened.listening.set_gate(Gate::Ignore);
    settle();
    opened.say(vec![0.0; 1_600]);
    opened.say(tone(1.0, 0.1, RATE));
    opened.quiet(Duration::from_millis(200));
    opened.listening.set_gate(Gate::Listen);
    settle();
    opened.say(tone(0.3, 0.1, RATE));
    assert_eq!(opened.next(), ListenEvent::SpeechStarted);
    opened.listening.end_turn();
    assert!(matches!(opened.next(), ListenEvent::Utterance(_)));
    // Push-to-talk : l'appui commence, le relâché finit.
    opened.listening.configure(ListenConfig {
        mode: TurnMode::PushToTalk,
        ..hands_free()
    });
    settle();
    opened.say(tone(0.5, 0.1, RATE));
    opened.quiet(Duration::from_millis(200));
    opened.listening.hold(true);
    assert_eq!(opened.next(), ListenEvent::SpeechStarted);
    opened.say(tone(0.5, 0.1, RATE));
    opened.say(vec![0.0; 2 * RATE as usize]);
    opened.quiet(Duration::from_millis(200));
    opened.listening.hold(false);
    let ListenEvent::Utterance(samples) = opened.next() else {
        panic!("no utterance")
    };
    // Tout le son tenu, silence compris.
    assert!(samples.len() >= 5 * RATE as usize / 2, "{}", samples.len());
}

/// Lâchée, l'écoute ferme le micro sans faire attendre.
#[test]
fn dropping_closes_the_microphone_quickly() {
    let opened = Opened::new(hands_free(), RATE);
    opened.say(vec![0.0; 1_600]);
    std::thread::sleep(Duration::from_millis(50));
    let Opened {
        listening, dropped, ..
    } = opened;
    let started = Instant::now();
    drop(listening);
    assert!(started.elapsed() < Duration::from_millis(150));
    assert!(dropped.load(Ordering::Acquire));
}

#[test]
fn a_lost_microphone_fails_and_ends_the_listening() {
    let opened = Opened::new(hands_free(), RATE);
    opened.mic.send(Chunk::Failed("unplugged".into())).unwrap();
    assert_eq!(opened.next(), ListenEvent::Failed("unplugged".into()));
    std::thread::sleep(Duration::from_millis(50));
    assert!(opened.dropped.load(Ordering::Acquire));
}

/// Un micro qui ne rend plus rien pendant 3 s est tenu pour perdu.
#[test]
fn a_silent_microphone_is_lost() {
    // Avant l'ouverture : le fil de l'écoute compte dès son départ, et
    // l'ouverture peut rendre la main bien plus tard sous la charge.
    let started = Instant::now();
    let opened = Opened::new(hands_free(), RATE);
    let event = loop {
        match opened.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            ListenEvent::Levels { .. } => {}
            event => break event,
        }
    };
    assert!(matches!(event, ListenEvent::Failed(_)), "{event:?}");
    assert!(started.elapsed() >= MIC_STALL);
}

#[test]
fn opening_failures_are_reported() {
    let monitor = private_monitor();
    let refused = Listener {
        microphone: Arc::new(|_: Thread| -> Result<Box<dyn Microphone>, String> {
            Err("no microphone".into())
        }),
        detector: Arc::new(loudness),
        monitor,
    };
    let error = refused.open(hands_free(), Box::new(|_| {})).err().unwrap();
    assert_eq!(error, "no microphone");
    let opened = Arc::new(AtomicBool::new(false));
    let flag = opened.clone();
    let broken = Listener {
        microphone: Arc::new(move |_: Thread| -> Result<Box<dyn Microphone>, String> {
            flag.store(true, Ordering::Release);
            Err("unused".into())
        }),
        detector: Arc::new(|| -> Result<Box<dyn SpeechDetector>, String> {
            Err("bad model".into())
        }),
        monitor,
    };
    let error = broken.open(hands_free(), Box::new(|_| {})).err().unwrap();
    assert!(error.contains("bad model"), "{error}");
    // Détection d'abord : le micro n'a pas été ouvert.
    assert!(!opened.load(Ordering::Acquire));
}

/// ~15 niveaux par seconde : micro (après annulation d'écho) et sortie,
/// en dBFS ramenés de -60..0 à 0..1.
#[test]
fn levels_follow_the_microphone_and_the_output() {
    let opened = Opened::new(hands_free(), RATE);
    // Sortie à -12 dBFS (RMS d'un sinus d'amplitude 0,355), micro à
    // -23 dBFS (amplitude 0,1), à d'autres fréquences : pas un écho.
    let output = tone_at(1_000.0, 1.0, 0.355, 48_000);
    // Tout le son d'un coup, la sortie donnée par le micro d'essai : un fil
    // d'essai retardé par la charge laissait l'écoute sans son plus de 3 s,
    // et le micro était tenu pour perdu.
    for _ in 0..2 {
        for (output, input) in output.chunks(4_800).zip(tone(1.0, 0.1, RATE).chunks(1_600)) {
            opened.say_with_output(input.to_vec(), output.to_vec(), 48_000);
        }
    }
    // Tous les niveaux des 2 s de son, quel que soit le retard de l'écoute
    // (charge), puis plus aucun.
    let mut levels = Vec::new();
    let mut others = Vec::new();
    let mut note = |event| match event {
        ListenEvent::Levels { input, output } => levels.push((input, output)),
        event => others.push(event),
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut received = 0;
    while received < 30 && Instant::now() < deadline {
        if let Ok(event) = opened.events.recv_timeout(Duration::from_millis(100)) {
            received += usize::from(matches!(event, ListenEvent::Levels { .. }));
            note(event);
        }
    }
    let until = Instant::now() + Duration::from_millis(300);
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        if let Ok(event) = opened.events.recv_timeout(left) {
            note(event);
        }
    }
    let count = levels.len();
    let last = levels.last().copied();
    assert!((29..=31).contains(&count), "{count} levels, {others:?}");
    let (input, output) = last.unwrap();
    assert!((input - 37.0 / 60.0).abs() < 0.03, "{input}");
    assert!((output - 48.0 / 60.0).abs() < 0.03, "{output}");
}

/// Chaîne complète : la sortie (48 kHz, lue sur le moniteur) sert de
/// référence ; son écho dans le micro n'interrompt pas la réponse. Sans
/// elle, il l'interrompt.
#[test]
fn the_output_monitor_feeds_the_echo_canceller() {
    let render = voice_like(8.0, 1);
    let echo = resample(&echo_of(&render, 60, 0.5), RATE, 48_000);
    let render = resample(&render, RATE, 48_000);
    for fed in [true, false] {
        let opened = Opened::new(hands_free(), 48_000);
        opened.listening.set_gate(Gate::Ignore);
        settle();
        // Morceaux de 20 ms, ~4 fois plus vite que le temps réel ; la sortie
        // a un morceau d'avance sur le micro : la première d'abord, puis
        // chacune donnée par le micro d'essai avec le morceau qui la précède
        // de deux (`say_with_output`).
        let chunks: Vec<_> = render.chunks(960).zip(echo.chunks(960)).collect();
        if fed {
            opened.monitor.feed(chunks[1].0, 48_000);
        }
        let mut events = Vec::new();
        // Son traité par l'écoute : un niveau toutes les 1/15 s de son.
        let levels = |events: &[ListenEvent]| {
            events
                .iter()
                .filter(|event| matches!(event, ListenEvent::Levels { .. }))
                .count()
        };
        for (index, (_, echo)) in chunks.iter().enumerate() {
            // Pas plus de 300 ms d'avance sur l'écoute : sous la charge (suite
            // complète), une avance plus grande faisait déborder la file de la
            // référence et l'écho n'était plus annulé.
            let due = (index as u64 * 20).saturating_sub(300);
            let deadline = Instant::now() + Duration::from_secs(10);
            while levels(&events) as u64 * 1000 / 15 < due && Instant::now() < deadline {
                events.extend(opened.events.recv_timeout(Duration::from_millis(20)));
            }
            if index == 100 {
                opened.listening.set_gate(Gate::Respond);
            }
            match chunks.get(index + 2) {
                Some((ahead, _)) if fed => {
                    opened.say_with_output(echo.to_vec(), ahead.to_vec(), 48_000)
                }
                _ => opened.say(echo.to_vec()),
            }
            std::thread::sleep(Duration::from_millis(5));
            events.extend(opened.events.try_iter());
        }
        // Jusqu'à ce que tout le son soit traité.
        let deadline = Instant::now() + Duration::from_secs(30);
        while levels(&events) < 8 * 15 - 1 && Instant::now() < deadline {
            events.extend(opened.events.recv_timeout(Duration::from_millis(100)));
        }
        assert!(levels(&events) >= 8 * 15 - 1, "{}", levels(&events));
        let barged = events.contains(&ListenEvent::BargeIn);
        assert_eq!(barged, !fed, "fed: {fed}");
    }
}

/// Contrats : vraie détection (Silero sur ONNX Runtime) et vrais
/// échantillons de parole (`.tools/speech-samples`).
mod contract {
    use super::*;
    use crate::voice_mode::audio::noise::Noise;
    use crate::voice_mode::audio::test_data::sample;
    use crate::voice_mode::audio::vad::Silero;

    fn silero() -> Box<dyn SpeechDetector> {
        Box::new(Silero::load().expect("Silero VAD (ONNX Runtime)"))
    }

    /// `seconds` de souffle à -60 dBFS.
    fn hiss(seconds: f32, seed: u64) -> Vec<f32> {
        let mut noise = Noise::new(seed);
        (0..(seconds * RATE as f32) as usize)
            .map(|_| noise.next() * 0.001_73)
            .collect()
    }

    /// Événements (hors niveaux) et l'échantillon où chacun est sorti.
    fn listen(
        render: &[f32],
        capture: &[f32],
        gate: Gate,
        config: ListenConfig,
    ) -> Vec<(usize, ListenEvent)> {
        let mut pipeline = Pipeline::new(silero(), silero(), config);
        let mut events = Vec::new();
        pipeline.command(Command::Gate(gate), &mut |_| {});
        for (index, chunk) in capture.chunks(AEC_FRAME).enumerate() {
            let start = index * AEC_FRAME;
            pipeline.push_render(
                &render[start.min(render.len())..(start + AEC_FRAME).min(render.len())],
            );
            pipeline
                .push_capture(chunk, &mut |event| {
                    if !matches!(event, ListenEvent::Levels { .. }) {
                        events.push((start + chunk.len(), event));
                    }
                })
                .unwrap();
        }
        events
    }

    fn seconds(samples: usize) -> f32 {
        samples as f32 / RATE as f32
    }

    /// Parole trouvée, et fin de tour trouvée après le silence réglé, sur
    /// une voix anglaise et une voix française, sans rien qui joue.
    #[test]
    #[ignore = "needs the speech samples of .tools and ONNX Runtime"]
    fn voice_contract_speech_and_turn_end_are_found() {
        for name in ["jfk.wav", "fr0.wav"] {
            let speech = sample(name);
            let lead = RATE as usize;
            let mut capture = hiss(1.0, 1);
            capture.extend(speech.iter().zip(hiss(30.0, 2)).map(|(s, n)| s + n));
            capture.extend(hiss(3.0, 3));
            let render = vec![0.0; capture.len()];
            let events = listen(&render, &capture, Gate::Listen, hands_free_normal());
            let (started, first) = &events[0];
            assert_eq!(*first, ListenEvent::SpeechStarted, "{name}: {events:?}");
            assert!(*started > lead, "{name}: started at {}", seconds(*started));
            let utterances: Vec<_> = events
                .iter()
                .filter_map(|(at, event)| match event {
                    ListenEvent::Utterance(samples) => Some((*at, samples.len())),
                    _ => None,
                })
                .collect();
            assert!(!utterances.is_empty(), "{name}: {events:?}");
            assert!(
                !events
                    .iter()
                    .any(|(_, event)| *event == ListenEvent::Discarded),
                "{name}"
            );
            let heard: usize = utterances.iter().map(|(_, length)| length).sum();
            let (ended, _) = utterances.last().unwrap();
            let speech_end = lead + speech.len();
            eprintln!(
                "{name}: {:.2} s of speech, started at {:.2} s, {} turn(s), {:.2} s kept, last turn ended {:.2} s after the end of the file",
                seconds(speech.len()),
                seconds(*started),
                utterances.len(),
                seconds(heard),
                seconds(ended - speech_end),
            );
            assert!(heard as f32 >= 0.8 * speech.len() as f32, "{name}");
            // Fin du fichier (déjà calme) → fin du tour : le silence réglé
            // (1 s) au plus, à une trame près.
            assert!(*ended <= speech_end + 17_000, "{name}: {}", seconds(*ended));
        }
    }

    fn hands_free_normal() -> ListenConfig {
        ListenConfig {
            silence: Duration::from_secs(1),
            ..hands_free()
        }
    }

    /// La voix jouée (jfk.wav, fr0.wav) revient dans le micro (80 ms de
    /// retard, atténuée, salle) : pas d'interruption pendant la réponse,
    /// même avant qu'AEC3 ait convergé ; en écoute, aucun tour ne part
    /// (un début de parole abandonné peut passer pendant la convergence).
    #[test]
    #[ignore = "needs the speech samples of .tools and ONNX Runtime"]
    fn voice_contract_echo_alone_does_not_barge_in() {
        for name in ["jfk.wav", "fr0.wav"] {
            let mut render = vec![0.0; RATE as usize / 2];
            render.extend(sample(name));
            render.extend(vec![0.0; RATE as usize]);
            let capture: Vec<f32> = echo_of(&render, 80, 0.5)
                .iter()
                .zip(hiss(30.0, 4))
                .map(|(echo, noise)| echo + noise)
                .collect();
            let unfed = listen(
                &vec![0.0; render.len()],
                &capture,
                Gate::Respond,
                hands_free_normal(),
            );
            assert_eq!(
                unfed.first().map(|(_, event)| event),
                Some(&ListenEvent::BargeIn),
                "{name}: the echo is speech"
            );
            let events = listen(&render, &capture, Gate::Respond, hands_free_normal());
            assert!(events.is_empty(), "{name}: {events:?}");
            let events = listen(&render, &capture, Gate::Listen, hands_free_normal());
            assert!(
                !events
                    .iter()
                    .any(|(_, event)| matches!(event, ListenEvent::Utterance(_))),
                "{name}: {events:?}"
            );
        }
    }

    /// Double parole : l'utilisateur (fr0.wav) parle pendant que la voix
    /// (jfk.wav) est jouée : interruption, puis son tour.
    #[test]
    #[ignore = "needs the speech samples of .tools and ONNX Runtime"]
    fn voice_contract_near_speech_barges_in_over_the_echo() {
        let mut render = vec![0.0; RATE as usize / 2];
        render.extend(sample("jfk.wav"));
        render.extend(sample("jfk.wav"));
        let near = sample("fr0.wav");
        let from = 4 * RATE as usize;
        let mut capture: Vec<f32> = echo_of(&render, 80, 0.5)
            .iter()
            .zip(hiss(60.0, 5))
            .map(|(echo, noise)| echo + noise)
            .collect();
        for (slot, voice) in capture[from..].iter_mut().zip(&near) {
            *slot += voice;
        }
        let events = listen(&render, &capture, Gate::Respond, hands_free_normal());
        let (at, first) = &events[0];
        assert_eq!(*first, ListenEvent::BargeIn, "{events:?}");
        eprintln!(
            "barge-in {:.2} s after the near speech began",
            seconds(at - from)
        );
        assert!(
            *at > from && *at < from + 2 * RATE as usize,
            "{}",
            seconds(*at)
        );
        let heard: usize = events
            .iter()
            .filter_map(|(_, event)| match event {
                ListenEvent::Utterance(samples) => Some(samples.len()),
                _ => None,
            })
            .sum();
        assert!(heard as f32 >= 0.8 * near.len() as f32, "{events:?}");
    }

    /// Comme l'e2e : sortie sans écho (casque, sortie factice), souffle à
    /// -70 dBFS, puis le début de fr1.wav (2,9 s) 1 s après le premier son
    /// joué : interruption, puis cette parole comme tour suivant.
    #[test]
    #[ignore = "needs the speech samples of .tools and ONNX Runtime"]
    fn voice_contract_headphones_barge_in() {
        let mut render = vec![0.0; 3 * RATE as usize / 10];
        render.extend(sample("fr0.wav"));
        let mut noise = Noise::new(12);
        let mut capture: Vec<f32> = (0..render.len()).map(|_| noise.next() * 0.000_55).collect();
        let near = &sample("fr1.wav")[..29 * RATE as usize / 10];
        // Premier son joué : même seuil que le faux micro de l'e2e.
        let first_sound = render.iter().position(|s| s.abs() > 1e-3).unwrap();
        let from = first_sound + RATE as usize;
        for (slot, voice) in capture[from..].iter_mut().zip(near) {
            *slot += voice;
        }
        let events = listen(&render, &capture, Gate::Respond, hands_free_normal());
        let (at, first) = &events[0];
        assert_eq!(*first, ListenEvent::BargeIn, "{events:?}");
        eprintln!(
            "headphones: barge-in {:.2} s after the WAV began",
            seconds(at - from)
        );
        assert!(*at < from + 2 * RATE as usize, "{}", seconds(*at));
        let Some((_, ListenEvent::Utterance(samples))) = events.get(1) else {
            panic!("{events:?}")
        };
        // Parole de 0,48 s à 2,57 s du WAV, plus 300 ms de chaque côté.
        assert!(
            samples.len() >= 26 * RATE as usize / 10,
            "{}",
            samples.len()
        );
    }

    /// Coût d'une trame de 10 ms (annulation d'écho, et détection une
    /// trame sur 3,2) sur un fil, et ERLE d'AEC3 sur une vraie voix.
    #[test]
    #[ignore = "needs the speech samples of .tools and ONNX Runtime"]
    fn voice_contract_cost_of_a_frame() {
        let mut render = vec![0.0; RATE as usize / 2];
        render.extend(sample("fr0.wav"));
        let capture: Vec<f32> = echo_of(&render, 80, 0.5)
            .iter()
            .zip(hiss(30.0, 6))
            .map(|(echo, noise)| echo + noise)
            .collect();
        let mut pipeline = Pipeline::new(silero(), silero(), hands_free());
        pipeline.command(Command::Gate(Gate::Respond), &mut |_| {});
        let frames = capture.len() / AEC_FRAME;
        let started = Instant::now();
        for (render, capture) in render
            .as_chunks::<AEC_FRAME>()
            .0
            .iter()
            .zip(capture.as_chunks::<AEC_FRAME>().0)
        {
            pipeline.push_render(render);
            pipeline.push_capture(capture, &mut |_| {}).unwrap();
        }
        let per_frame = started.elapsed() / frames as u32;
        let echo_only = crate::voice_mode::audio::echo::tests::cancel(&render, &capture, true);
        let start = 2 * RATE as usize;
        let power = |s: &[f32]| s.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>();
        let erle = 10.0 * (power(&capture[start..]) / power(&echo_only[start..])).log10();
        let mut vad = Silero::load().unwrap();
        let vad_started = Instant::now();
        for frame in capture.as_chunks::<FRAME>().0 {
            vad.probability(frame).unwrap();
        }
        let per_vad = vad_started.elapsed() / (capture.len() / FRAME) as u32;
        eprintln!(
            "10 ms frame: {per_frame:?} (AEC + VAD), Silero alone: {per_vad:?} per 32 ms, ERLE on fr0.wav after 2 s: {erle:.1} dB"
        );
        assert!(per_frame < Duration::from_millis(5), "{per_frame:?}");
        assert!(erle >= 15.0, "{erle:.1} dB");
    }
}

/// Micro rouvert à un autre débit (Windows, périphérique par défaut changé) :
/// le son continue au débit annoncé à l'ouverture.
#[test]
fn a_reopened_microphone_keeps_the_announced_rate() {
    let ring = Arc::new(SampleRing::new(96_000));
    let mut feed = Feed::new(ring.clone(), 48_000);
    let tone = |rate: u32, seconds: f64| -> Vec<f32> {
        (0..(f64::from(rate) * seconds) as usize)
            .map(|i| (std::f64::consts::TAU * 440.0 * i as f64 / f64::from(rate)).sin() as f32)
            .collect()
    };
    // Même débit : tel quel.
    let first = tone(48_000, 0.5);
    ring.push(&first, 48_000);
    let mut out = Vec::new();
    feed.read(&mut out);
    assert_eq!(out, first);
    // Micro rouvert à 16 kHz (la file a été vidée avant) : 0,5 s de son
    // deviennent ~0,5 s à 48 kHz, au même niveau.
    ring.push(&tone(16_000, 0.5), 16_000);
    out.clear();
    feed.read(&mut out);
    assert!((22_000..=24_000).contains(&out.len()), "{}", out.len());
    let rms = |samples: &[f32]| {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    };
    assert!((rms(&out[2_000..]) - rms(&first)).abs() < 0.02);
    // Revenu au débit annoncé : de nouveau tel quel.
    ring.push(&first, 48_000);
    out.clear();
    feed.read(&mut out);
    assert_eq!(out, first);
}
