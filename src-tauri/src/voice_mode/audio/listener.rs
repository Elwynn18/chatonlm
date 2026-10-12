//! Fil de l'écoute : micro en flux → 16 kHz → annulation d'écho (référence :
//! moniteur de la sortie) → détection de parole → fin de tour → événements.
//!
//! Un fil par écoute : il ouvre le micro (le flux cpal n'est pas `Send`), le
//! garde et traite le son ; le rappel du micro ne fait que copier dans une
//! file sans verrou et réveiller le fil. Les commandes de la séance (gate,
//! fin de tour, push-to-talk, réglages) passent par une file prise le temps
//! d'un échange. L'écoute lâchée, le fil s'arrête et ferme le micro ; le
//! lâcher attend cette fermeture 100 ms au plus.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

use super::echo::{AEC_FRAME, Canceller, EchoGuard, RenderQueue, energy};
use super::endpoint::{Endpoint, FRAME};
use super::vad::SpeechDetector;
use super::{Ear, Events, Gate, ListenConfig, ListenEvent, Listening, RATE};
use crate::dictation::audio::{NoAnswer, os as host, wait_opened};
use crate::i18n::t;
use crate::tts::playback::{Monitor, Resampler, SampleRing};

/// Attente du fil quand il n'a rien à faire (le micro le réveille).
const TICK: Duration = Duration::from_millis(10);
/// Lâcher de l'écoute : fermeture du micro attendue au plus.
const CLOSE_WAIT: Duration = Duration::from_millis(100);
/// Micro muet (aucun échantillon) au-delà : tenu pour perdu.
const MIC_STALL: Duration = Duration::from_secs(3);
/// Niveaux par seconde.
const LEVELS_PER_SECOND: usize = 15;
/// Son de la sortie tenu pour non nul (faux micro des essais e2e).
const AUDIBLE: f32 = 1e-3;
/// File du micro : 2 s au plus haut débit prévu.
const MIC_RING: usize = 2 * 96_000;

/// Micro vu par le fil de l'écoute : le vrai, le faux des essais e2e, ou
/// celui des essais.
pub trait Microphone {
    /// Débit des échantillons rendus par `read`.
    fn rate(&self) -> u32;
    /// Ajoute à `out` les échantillons (mono) arrivés depuis l'appel
    /// précédent ; erreur (traduite) si le micro est perdu.
    fn read(&mut self, out: &mut Vec<f32>, cues: &Cues) -> Result<(), String>;
}

/// Ce que l'écoute sait de la séance (le faux micro des essais e2e s'y
/// règle).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cues {
    /// Passages en `Listen` depuis l'ouverture (1 à l'ouverture).
    pub listens: u64,
    /// La sortie a joué un son non nul depuis la lecture précédente.
    pub output_sound: bool,
}

/// Ouvre le micro sur le fil de l'écoute (qu'il peut réveiller) ; erreur
/// traduite.
pub type OpenMicrophone = dyn Fn(Thread) -> Result<Box<dyn Microphone>, String> + Send + Sync;
/// Charge la détection de parole ; erreur en anglais (détail).
pub type OpenDetector = dyn Fn() -> Result<Box<dyn SpeechDetector>, String> + Send + Sync;

/// Fabrique d'écoutes.
pub struct Listener {
    pub microphone: Arc<OpenMicrophone>,
    pub detector: Arc<OpenDetector>,
    pub monitor: &'static Monitor,
}

impl Ear for Listener {
    fn open(&self, config: ListenConfig, events: Events) -> Result<Box<dyn Listening>, String> {
        let control = Arc::new(Control::default());
        let (ready, opened) = mpsc::channel();
        let (microphone, detector, monitor) =
            (self.microphone.clone(), self.detector.clone(), self.monitor);
        let shared = control.clone();
        let thread = thread::Builder::new()
            .name("chatonlm-voice-ear".into())
            .spawn(move || {
                let setup = || -> Result<_, String> {
                    // Deux détections : son nettoyé et micro brut.
                    let detectors = detector()
                        .and_then(|first| Ok((first, detector()?)))
                        .map_err(|error| detection_failed(&error))?;
                    // Moniteur avant le micro : la référence est là dès le
                    // premier son capté.
                    let tap = monitor.subscribe();
                    let microphone = microphone(thread::current())?;
                    Ok((detectors, tap, microphone))
                };
                match setup() {
                    Ok(((detector, raw_detector), tap, microphone)) => {
                        let _ = ready.send(Ok(()));
                        let mut streams = Streams::new(microphone, tap);
                        let mut pipeline = Pipeline::new(detector, raw_detector, config);
                        streams.run(&mut pipeline, &shared, &*events);
                        // Micro fermé.
                        drop(streams);
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                    }
                }
                shared.close();
            })
            .map_err(|error| crate::dictation::audio::could_not_open(&error))?;
        // Ouverture attendue sans limite, ou bornée par le système (Windows :
        // détection chargée puis micro activé). En retard : le fil s'arrête
        // de lui-même dès que son ouverture aboutit.
        let limit = host::OPEN_WAIT.map(|limit| limit * 3);
        match wait_opened(&opened, limit) {
            Ok(answer) => answer?,
            Err(reason) => {
                control.stop.store(true, Ordering::Release);
                return Err(crate::dictation::audio::could_not_open(&match reason {
                    NoAnswer::Stopped => "the listening thread stopped",
                    NoAnswer::Late => "the audio device did not answer",
                }));
            }
        }
        Ok(Box::new(Handle {
            control,
            thread: thread.thread().clone(),
        }))
    }
}

/// Détection de parole en échec (texte montré).
fn detection_failed(error: &dyn std::fmt::Display) -> String {
    t!("voiceMode.listening.detectionFailed", error = error)
}

/// Micro perdu en cours d'écoute (texte montré).
pub fn microphone_lost(error: &dyn std::fmt::Display) -> String {
    t!("voiceMode.listening.microphoneLost", error = error)
}

/// Commande de la séance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    Gate(Gate),
    EndTurn,
    Hold(bool),
    Configure(ListenConfig),
}

/// Partagé entre l'écoute (côté séance) et son fil.
#[derive(Default)]
struct Control {
    stop: AtomicBool,
    commands: Mutex<Vec<Command>>,
    /// Le fil a fermé le micro.
    closed: Mutex<bool>,
    closed_changed: Condvar,
}

impl Control {
    fn take(&self) -> Vec<Command> {
        std::mem::take(&mut *self.commands.lock())
    }

    fn close(&self) {
        *self.closed.lock() = true;
        self.closed_changed.notify_all();
    }
}

/// Écoute ouverte (côté séance).
struct Handle {
    control: Arc<Control>,
    thread: Thread,
}

impl Handle {
    fn send(&self, command: Command) {
        self.control.commands.lock().push(command);
        self.thread.unpark();
    }
}

impl Listening for Handle {
    fn set_gate(&self, gate: Gate) {
        self.send(Command::Gate(gate));
    }

    fn end_turn(&self) {
        self.send(Command::EndTurn);
    }

    fn hold(&self, pressed: bool) {
        self.send(Command::Hold(pressed));
    }

    fn configure(&self, config: ListenConfig) {
        self.send(Command::Configure(config));
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.control.stop.store(true, Ordering::Release);
        self.thread.unpark();
        // Lâchée depuis le fil lui-même (rappel des événements) : il
        // s'arrêtera au retour.
        if thread::current().id() == self.thread.id() {
            return;
        }
        let mut closed = self.control.closed.lock();
        if !*closed {
            self.control
                .closed_changed
                .wait_for(&mut closed, CLOSE_WAIT);
        }
    }
}

/// Entrées du fil : micro et moniteur de la sortie, ramenés à 16 kHz.
struct Streams {
    microphone: Box<dyn Microphone>,
    tap: crate::tts::playback::OutputTap,
    from_microphone: Resampler,
    /// Rééchantillonneur de la sortie, et son débit d'entrée.
    from_output: Option<(u32, Resampler)>,
    raw: Vec<f32>,
    converted: Vec<f32>,
}

impl Streams {
    fn new(microphone: Box<dyn Microphone>, tap: crate::tts::playback::OutputTap) -> Self {
        Self {
            from_microphone: Resampler::new(microphone.rate(), RATE),
            microphone,
            tap,
            from_output: None,
            raw: Vec::new(),
            converted: Vec::new(),
        }
    }

    /// Jusqu'à l'arrêt ou l'échec du micro.
    fn run(&mut self, pipeline: &mut Pipeline, control: &Control, events: &dyn Fn(ListenEvent)) {
        let mut emit = |event: ListenEvent| events(event);
        let mut last_sound = Instant::now();
        while !control.stop.load(Ordering::Acquire) {
            // La sortie d'abord : sa référence précède le son du micro.
            self.raw.clear();
            let rate = self.tap.read(&mut self.raw);
            let output_sound = self.raw.iter().any(|sample| sample.abs() > AUDIBLE);
            if !self.raw.is_empty() && rate > 0 {
                if self.from_output.as_ref().map(|(from, _)| *from) != Some(rate) {
                    self.from_output = Some((rate, Resampler::new(rate, RATE)));
                }
                if let Some((_, resampler)) = &mut self.from_output {
                    self.converted.clear();
                    resampler.process(&self.raw, &mut self.converted);
                    pipeline.push_render(&self.converted);
                }
            }
            self.raw.clear();
            let cues = Cues {
                listens: pipeline.endpoint.listens(),
                output_sound,
            };
            if let Err(message) = self.microphone.read(&mut self.raw, &cues) {
                emit(ListenEvent::Failed(message));
                return;
            }
            if !self.raw.is_empty() {
                last_sound = Instant::now();
            } else if last_sound.elapsed() > MIC_STALL {
                emit(ListenEvent::Failed(microphone_lost(&"no audio")));
                return;
            }
            self.converted.clear();
            self.from_microphone.process(&self.raw, &mut self.converted);
            if let Err(error) = pipeline.push_capture(&self.converted, &mut emit) {
                emit(ListenEvent::Failed(detection_failed(&error)));
                return;
            }
            // Après le son déjà capté : un relâché garde tout ce qui le
            // précède.
            for command in control.take() {
                pipeline.command(command, &mut emit);
            }
            thread::park_timeout(TICK);
        }
    }
}

/// Traitement à 16 kHz : annulation d'écho, détection, fin de tour,
/// niveaux. Sans fil ni périphérique.
pub struct Pipeline {
    canceller: Canceller,
    render: RenderQueue,
    guard: EchoGuard,
    /// Détection sur le son nettoyé, et sur le micro brut : chacune suit
    /// son flux sans trou (l'état récurrent de Silero mêlerait les deux).
    detector: Box<dyn SpeechDetector>,
    raw_detector: Box<dyn SpeechDetector>,
    pub endpoint: Endpoint,
    /// Son du micro en attente d'une trame de 10 ms.
    capture: Vec<f32>,
    /// Son nettoyé et micro brut en attente d'une trame de détection
    /// (32 ms).
    cleaned: Vec<f32>,
    raw: Vec<f32>,
    meter: Meter,
}

impl Pipeline {
    pub fn new(
        detector: Box<dyn SpeechDetector>,
        raw_detector: Box<dyn SpeechDetector>,
        config: ListenConfig,
    ) -> Self {
        Self {
            canceller: Canceller::new(),
            render: RenderQueue::default(),
            guard: EchoGuard::default(),
            detector,
            raw_detector,
            endpoint: Endpoint::new(config),
            capture: Vec::with_capacity(4 * AEC_FRAME),
            cleaned: Vec::with_capacity(2 * FRAME),
            raw: Vec::with_capacity(2 * FRAME),
            meter: Meter::default(),
        }
    }

    /// Son joué par la sortie, à 16 kHz.
    pub fn push_render(&mut self, samples: &[f32]) {
        self.render.push(samples);
    }

    /// Son du micro à 16 kHz : chaque trame de 10 ms passe par l'annulation
    /// d'écho, précédée d'une trame de la référence. Tant qu'AEC3 n'a pas
    /// convergé et que la sortie joue, il efface aussi la voix proche :
    /// l'écoute prend alors le micro brut, sous la garde de l'écho. Erreur :
    /// détection en échec (détail en anglais).
    pub fn push_capture(
        &mut self,
        samples: &[f32],
        emit: &mut dyn FnMut(ListenEvent),
    ) -> Result<(), String> {
        self.capture.extend_from_slice(samples);
        let mut start = 0;
        while self.capture.len() - start >= AEC_FRAME {
            let mut render = [0.0; AEC_FRAME];
            self.render.pop(&mut render);
            self.canceller.render(&render);
            let mut frame = [0.0; AEC_FRAME];
            frame.copy_from_slice(&self.capture[start..start + AEC_FRAME]);
            start += AEC_FRAME;
            let mut cleaned = [0.0; AEC_FRAME];
            self.canceller.capture(&frame, &mut cleaned);
            self.guard
                .add(energy(&render), energy(&frame), self.canceller.erle_db());
            let heard = if self.guard.bypasses() {
                &frame
            } else {
                &cleaned
            };
            if let Some(levels) = self.meter.add(heard, &render) {
                emit(levels);
            }
            self.cleaned.extend_from_slice(&cleaned);
            self.raw.extend_from_slice(&frame);
            if self.cleaned.len() >= FRAME {
                let on_cleaned = self.detector.probability(&self.cleaned[..FRAME])?;
                let on_raw = self.raw_detector.probability(&self.raw[..FRAME])?;
                let (heard, mut probability) = if self.guard.bypasses() {
                    (&self.raw[..FRAME], on_raw)
                } else {
                    (&self.cleaned[..FRAME], on_cleaned)
                };
                if self.guard.is_echo(energy(heard)) {
                    probability = 0.0;
                }
                self.endpoint.push(heard, probability, emit);
                self.cleaned.drain(..FRAME);
                self.raw.drain(..FRAME);
            }
        }
        self.capture.drain(..start);
        Ok(())
    }

    /// Couplage appris par la garde de l'écho (dB) : essais.
    #[cfg(test)]
    pub fn guard_coupling_db(&self) -> f32 {
        self.guard.coupling_db()
    }

    pub fn command(&mut self, command: Command, emit: &mut dyn FnMut(ListenEvent)) {
        // Fin de tour demandée : le son entendu qui n'a pas encore rempli
        // une trame de détection (moins de 32 ms) termine le tour.
        if matches!(command, Command::EndTurn | Command::Hold(false)) {
            let pending = if self.guard.bypasses() {
                &self.raw
            } else {
                &self.cleaned
            };
            if self.endpoint.extend(pending) {
                self.cleaned.clear();
                self.raw.clear();
            }
        }
        match command {
            Command::Gate(gate) => self.endpoint.set_gate(gate),
            Command::EndTurn => self.endpoint.end_turn(emit),
            Command::Hold(pressed) => self.endpoint.hold(pressed, emit),
            Command::Configure(config) => self.endpoint.configure(config),
        }
    }
}

/// Niveaux du micro (son entendu : nettoyé, ou brut sous la garde) et de la
/// sortie, ~15 fois par seconde.
#[derive(Default)]
struct Meter {
    input: f64,
    output: f64,
    samples: usize,
    /// Échantillons × niveaux par seconde depuis le dernier envoi.
    clock: usize,
}

impl Meter {
    fn add(&mut self, input: &[f32], output: &[f32]) -> Option<ListenEvent> {
        let energy = |samples: &[f32]| samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
        self.input += energy(input);
        self.output += energy(output);
        self.samples += input.len();
        self.clock += input.len() * LEVELS_PER_SECOND;
        if self.clock < RATE as usize {
            return None;
        }
        self.clock -= RATE as usize;
        let count = self.samples.max(1) as f64;
        let levels = ListenEvent::Levels {
            input: level(self.input / count),
            output: level(self.output / count),
        };
        (self.input, self.output, self.samples) = (0.0, 0.0, 0);
        Some(levels)
    }
}

/// Niveau perceptif : RMS en dBFS, -60..0 ramené à 0..1.
fn level(mean_square: f64) -> f32 {
    let dbfs = 10.0 * mean_square.max(1e-12).log10();
    ((dbfs + 60.0) / 60.0).clamp(0.0, 1.0) as f32
}

/// Son du micro lu dans sa file, au débit annoncé à l'ouverture : un micro
/// rouvert à un autre débit (périphérique par défaut changé, Windows) y est
/// ramené.
struct Feed {
    ring: Arc<SampleRing>,
    position: u64,
    /// Débit annoncé à l'écoute (celui de la première ouverture).
    rate: u32,
    /// Conversion du débit du micro rouvert vers `rate`, et ce débit.
    adapter: Option<(u32, Resampler)>,
    scratch: Vec<f32>,
}

impl Feed {
    fn new(ring: Arc<SampleRing>, rate: u32) -> Self {
        let position = ring.end();
        Self {
            ring,
            position,
            rate,
            adapter: None,
            scratch: Vec::new(),
        }
    }

    /// Ajoute à `out` ce qui est arrivé depuis l'appel précédent. La file ne
    /// contient jamais deux débits à la fois : elle est vidée avant chaque
    /// réouverture.
    fn read(&mut self, out: &mut Vec<f32>) {
        let current = self.ring.rate();
        if current == 0 || current == self.rate {
            self.adapter = None;
            self.ring.read(&mut self.position, out);
            return;
        }
        if self.adapter.as_ref().map(|(from, _)| *from) != Some(current) {
            self.adapter = Some((current, Resampler::new(current, self.rate)));
        }
        self.scratch.clear();
        self.ring.read(&mut self.position, &mut self.scratch);
        if let Some((_, resampler)) = &mut self.adapter {
            resampler.process(&self.scratch, out);
        }
    }
}

/// Erreur grave signalée par le flux du micro.
struct Failure {
    message: String,
    /// Le périphérique par défaut a changé : le micro est à rouvrir (jamais
    /// sous Linux, voir `dictation::audio::os`).
    reopen: bool,
}

/// Deux réouvertures plus rapprochées : le micro est tenu pour perdu (pas de
/// boucle de réouvertures).
const REOPEN_SPACING: Duration = Duration::from_secs(1);

/// Le micro du système (entrée par défaut de cpal).
pub struct SystemMicrophone {
    /// `None` le temps d'une réouverture.
    stream: Option<cpal::Stream>,
    feed: Feed,
    /// Fil de l'écoute, réveillé par le micro.
    waker: Thread,
    failure: Arc<Mutex<Option<Failure>>>,
    reopened: Option<Instant>,
}

impl SystemMicrophone {
    pub fn open(thread: Thread) -> Result<Box<dyn Microphone>, String> {
        let ring = Arc::new(SampleRing::new(MIC_RING));
        let failure = Arc::new(Mutex::new(None));
        let (stream, rate) = Self::start(&ring, &failure, &thread)?;
        Ok(Box::new(Self {
            stream: Some(stream),
            feed: Feed::new(ring, rate),
            waker: thread,
            failure,
            reopened: None,
        }))
    }

    /// Ouvre l'entrée par défaut, qui écrit dans `ring` et réveille `thread`.
    fn start(
        ring: &Arc<SampleRing>,
        failure: &Arc<Mutex<Option<Failure>>>,
        thread: &Thread,
    ) -> Result<(cpal::Stream, u32), String> {
        let (writer, waker) = (ring.clone(), thread.clone());
        let (on_failure, failed_waker) = (failure.clone(), thread.clone());
        crate::dictation::audio::open_input(
            move |rate| {
                move |mono: &[f32]| {
                    writer.push(mono, rate);
                    waker.unpark();
                }
            },
            move |error: cpal::Error| {
                eprintln!("chatonlm: voice mode microphone error: {error}");
                if matches!(
                    error.kind(),
                    cpal::ErrorKind::DeviceNotAvailable
                        | cpal::ErrorKind::StreamInvalidated
                        | cpal::ErrorKind::HostUnavailable
                ) {
                    *on_failure.lock() = Some(Failure {
                        message: error.to_string(),
                        reopen: host::reopens(error.kind()),
                    });
                    failed_waker.unpark();
                }
            },
        )
    }

    /// Périphérique par défaut changé : ferme le flux, garde ce qu'il a déjà
    /// capté et rouvre l'entrée par défaut.
    fn reopen(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
        if self
            .reopened
            .is_some_and(|last| last.elapsed() < REOPEN_SPACING)
        {
            return Err("reopened twice in a row".into());
        }
        // Flux fermé d'abord : plus rien n'arrive à l'ancien débit.
        self.stream = None;
        self.feed.read(out);
        self.failure.lock().take();
        let (stream, rate) = Self::start(&self.feed.ring, &self.failure, &self.waker)?;
        eprintln!("chatonlm: voice mode microphone reopened at {rate} Hz");
        self.stream = Some(stream);
        self.reopened = Some(Instant::now());
        Ok(())
    }
}

impl Microphone for SystemMicrophone {
    fn rate(&self) -> u32 {
        self.feed.rate
    }

    fn read(&mut self, out: &mut Vec<f32>, _cues: &Cues) -> Result<(), String> {
        let failure = self.failure.lock().take();
        if let Some(failure) = failure {
            if !failure.reopen {
                return Err(microphone_lost(&failure.message));
            }
            if let Err(error) = self.reopen(out) {
                eprintln!("chatonlm: voice mode microphone not reopened: {error}");
                return Err(microphone_lost(&failure.message));
            }
        }
        self.feed.read(out);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
