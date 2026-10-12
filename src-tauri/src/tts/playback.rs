//! Sortie audio en flux par cpal (ALSA sous Linux, WASAPI sous Windows) : fil
//! dédié (le flux cpal n'est pas Send partout, comme l'entrée de
//! `dictation::audio`), file d'échantillons au débit du
//! périphérique, rééchantillonnage en flux de `engine::SAMPLE_RATE` vers ce
//! débit, démarrage après `PREBUFFER` en tampon (et de nouveau après une
//! sous-alimentation), ou dès que rien n'arrive plus depuis `IDLE_START`
//! (phrase courte suivie d'une attente), arrêt immédiat (file vidée). Au-delà
//! de `MAX_QUEUED` en file, `push` attend que la sortie consomme (la
//! synthèse va ~6× plus vite que la lecture).
//!
//! `dictation::audio::resample` traite un enregistrement entier : appliqué
//! morceau par morceau, il couperait le signal à chaque bord. Le
//! rééchantillonneur d'ici garde l'historique du filtre d'un envoi à
//! l'autre (même noyau : sinus cardinal fenêtré par Blackman).
//!
//! Build e2e : `CHATONLM_E2E_AUDIO_OUT=null` remplace le périphérique (absent
//! de la session de test virtuelle) par une sortie factice qui consomme les
//! échantillons au rythme réel et les compte.
//!
//! Moniteur de la sortie ([`monitor`]) : ce que la sortie joue réellement
//! (silence compris), au débit du périphérique, pour l'annulation d'écho du
//! mode vocal. Le rappel audio n'y attend jamais (file sans verrou, bornée,
//! plus vieux jetés) ; sans abonné, il n'y coûte qu'une lecture atomique.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering, fence};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    FromSample, SampleFormat, SizedSample, SupportedStreamConfig, SupportedStreamConfigRange,
};
use parking_lot::{Condvar, Mutex};

use super::engine::SAMPLE_RATE;
use crate::dictation::audio::{NoAnswer, os as host, wait_opened};
use crate::error::{AppError, AppResult};

/// Son en tampon avant de commencer à jouer.
const PREBUFFER: Duration = Duration::from_millis(300);
/// Sortie qui ne consomme plus rien pendant ce délai : `drain` abandonne,
/// `push` n'attend plus.
const STALL: Duration = Duration::from_secs(2);
/// Rien d'envoyé depuis ce délai : jouer ce qui est en file sans attendre le
/// tampon de départ.
const IDLE_START: Duration = Duration::from_millis(150);
/// Son en file au-delà duquel `push` attend.
const MAX_QUEUED: Duration = Duration::from_secs(10);

/// Variable du build e2e : `null` = sortie factice.
#[cfg(feature = "e2e")]
pub const E2E_AUDIO_OUT_ENV: &str = "CHATONLM_E2E_AUDIO_OUT";

/// Demi-largeur du filtre, en passages par zéro du sinus cardinal.
const ZEROS: f64 = 16.0;
/// Points du noyau tabulé par échantillon d'entrée.
const TABLE_STEP: f64 = 512.0;

/// Rééchantillonneur en flux : sinus cardinal fenêtré (Blackman), coupé à
/// 90 % de la plus basse des deux fréquences de Nyquist.
pub struct Resampler {
    /// Pas, en échantillons d'entrée, entre deux sorties ; `None` : même
    /// débit.
    step: Option<f64>,
    /// Demi-largeur du noyau, en échantillons d'entrée.
    width: f64,
    table: Vec<f64>,
    /// Historique et échantillons en attente.
    input: Vec<f32>,
    /// Position de la prochaine sortie dans `input`.
    position: f64,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        if from == to || from == 0 || to == 0 {
            return Self {
                step: None,
                width: 0.0,
                table: Vec::new(),
                input: Vec::new(),
                position: 0.0,
            };
        }
        let step = f64::from(from) / f64::from(to);
        let cutoff = (1.0 / step).min(1.0) * 0.9;
        let width = ZEROS / cutoff;
        let points = (2.0 * width * TABLE_STEP).ceil() as usize + 2;
        let table = (0..points)
            .map(|point| {
                use std::f64::consts::PI;
                let t = point as f64 / TABLE_STEP - width;
                let x = t / width;
                if x.abs() >= 1.0 {
                    return 0.0;
                }
                let window = 0.42 + 0.5 * (PI * x).cos() + 0.08 * (2.0 * PI * x).cos();
                let phase = PI * cutoff * t;
                let sinc = if phase.abs() < 1e-12 {
                    1.0
                } else {
                    phase.sin() / phase
                };
                sinc * window
            })
            .collect();
        let mut resampler = Self {
            step: Some(step),
            width,
            table,
            input: Vec::new(),
            position: 0.0,
        };
        resampler.reset();
        resampler
    }

    /// Oublie l'historique (nouveau signal).
    pub fn reset(&mut self) {
        self.input.clear();
        // Historique de zéros : la première sortie tombe sur le premier
        // échantillon, avec tout son contexte gauche.
        let history = self.width.ceil() as usize;
        self.input.resize(history, 0.0);
        self.position = history as f64;
    }

    fn kernel(&self, offset: f64) -> f64 {
        let scaled = (offset + self.width) * TABLE_STEP;
        let index = scaled.floor();
        let fraction = scaled - index;
        let index = index as usize;
        match (self.table.get(index), self.table.get(index + 1)) {
            (Some(a), Some(b)) => a + (b - a) * fraction,
            (Some(a), None) => *a,
            _ => 0.0,
        }
    }

    /// Ajoute `samples` et écrit dans `out` les sorties dont tout le
    /// contexte est connu.
    pub fn process(&mut self, samples: &[f32], out: &mut Vec<f32>) {
        let Some(step) = self.step else {
            out.extend_from_slice(samples);
            return;
        };
        self.input.extend_from_slice(samples);
        let last = self.input.len() as f64 - 1.0;
        while self.position + self.width <= last {
            let center = self.position;
            let first = (center - self.width).ceil().max(0.0) as usize;
            let end = (center + self.width).floor() as usize;
            let (mut sum, mut weights) = (0.0f64, 0.0f64);
            for (index, &sample) in self.input[first..=end].iter().enumerate() {
                let weight = self.kernel(center - (first + index) as f64);
                sum += weight * f64::from(sample);
                weights += weight;
            }
            out.push(if weights.abs() > 1e-9 {
                (sum / weights) as f32
            } else {
                0.0
            });
            self.position += step;
        }
        // Garder seulement le contexte gauche de la prochaine sortie.
        let keep_from = (self.position - self.width).floor().max(0.0) as usize;
        if keep_from > 0 {
            self.input.drain(..keep_from);
            self.position -= keep_from as f64;
        }
    }

    /// Fin du signal : sort la queue (contexte droit complété de zéros).
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        if self.step.is_some() {
            let padding = vec![0.0; self.width.ceil() as usize + 1];
            self.process(&padding, out);
        }
    }
}

/// File d'échantillons sans verrou, à un seul écrivain et un seul lecteur :
/// l'écrivain (rappel audio) n'attend jamais ; pleine, il écrase les plus
/// vieux échantillons, que le lecteur saute (et compte).
pub struct SampleRing {
    /// Échantillons (bits des f32), en boucle.
    slots: Box<[AtomicU32]>,
    /// Échantillons écrits depuis la création.
    written: AtomicU64,
    /// Fin de l'écriture en cours, publiée avant ses cases : le lecteur voit
    /// ainsi les cases écrasées pendant sa copie (verrou séquentiel).
    reserved: AtomicU64,
    /// Débit des derniers échantillons écrits.
    rate: AtomicU32,
}

impl SampleRing {
    pub fn new(capacity: usize) -> Self {
        Self {
            slots: (0..capacity.max(1)).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
            reserved: AtomicU64::new(0),
            rate: AtomicU32::new(0),
        }
    }

    fn capacity(&self) -> u64 {
        self.slots.len() as u64
    }

    /// Écrivain (un seul à la fois) : ajoute `samples`, au débit `rate`.
    pub fn push(&self, samples: &[f32], rate: u32) {
        let start = self.written.load(Ordering::Relaxed);
        let capacity = self.capacity();
        self.reserved
            .store(start + samples.len() as u64, Ordering::Relaxed);
        fence(Ordering::Release);
        // Au-delà de la capacité, seuls les derniers comptent.
        let skip = samples.len().saturating_sub(self.slots.len());
        for (offset, sample) in samples.iter().enumerate().skip(skip) {
            let slot = ((start + offset as u64) % capacity) as usize;
            self.slots[slot].store(sample.to_bits(), Ordering::Relaxed);
        }
        self.rate.store(rate, Ordering::Relaxed);
        self.written
            .store(start + samples.len() as u64, Ordering::Release);
    }

    /// Lecteur (un seul) : ajoute à `out` ce qui a été écrit depuis
    /// `*position` (avancée) ; rend le nombre d'échantillons perdus (écrasés
    /// avant d'être lus).
    pub fn read(&self, position: &mut u64, out: &mut Vec<f32>) -> u64 {
        let end = self.written.load(Ordering::Acquire);
        let capacity = self.capacity();
        let start = (*position).max(end.saturating_sub(capacity));
        let mut lost = start - *position;
        let base = out.len();
        out.extend((start..end).map(|index| {
            f32::from_bits(self.slots[(index % capacity) as usize].load(Ordering::Relaxed))
        }));
        // Écrasés pendant la copie (écriture en cours comprise) : jetés.
        fence(Ordering::Acquire);
        let now = self.reserved.load(Ordering::Relaxed);
        let overwritten = (now.saturating_sub(capacity)).saturating_sub(start);
        if overwritten > 0 {
            let overwritten = overwritten.min(end - start);
            out.drain(base..base + overwritten as usize);
            lost += overwritten;
        }
        *position = end;
        lost
    }

    /// Débit des derniers échantillons écrits (0 avant le premier).
    pub fn rate(&self) -> u32 {
        self.rate.load(Ordering::Relaxed)
    }

    /// Position de l'écrivain : un lecteur qui part de là ne lit que la
    /// suite.
    pub fn end(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }
}

/// Son gardé pour un abonné du moniteur qui ne lit pas assez vite.
const TAP_CAPACITY: Duration = Duration::from_secs(2);
/// Débit maximal prévu pour dimensionner la file d'un abonné.
const TAP_MAX_RATE: u32 = 96_000;

/// Moniteur de la sortie : chaque sortie lui passe ce qu'elle joue.
pub struct Monitor {
    /// Abonnés, lu sans verrou à chaque rappel audio.
    count: AtomicUsize,
    /// Files des abonnés ; prise par la sortie avec `try_lock` seulement
    /// (une seule sortie écrit à la fois dans une file).
    taps: Mutex<Vec<Arc<SampleRing>>>,
}

impl Monitor {
    pub const fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
            taps: parking_lot::const_mutex(Vec::new()),
        }
    }

    /// Abonnement : l'abonné lit ce qui est joué à partir de maintenant.
    pub fn subscribe(&'static self) -> OutputTap {
        let ring = Arc::new(SampleRing::new(
            (f64::from(TAP_MAX_RATE) * TAP_CAPACITY.as_secs_f64()) as usize,
        ));
        let mut taps = self.taps.lock();
        taps.push(ring.clone());
        self.count.store(taps.len(), Ordering::Release);
        OutputTap {
            monitor: self,
            ring,
            position: 0,
        }
    }

    /// Appelé par la sortie avec ce qu'elle vient de jouer (mono, à
    /// `rate`). N'attend jamais : si la liste des abonnés est prise
    /// (abonnement en cours, autre sortie), le bloc est perdu.
    pub fn feed(&self, samples: &[f32], rate: u32) {
        if self.count.load(Ordering::Acquire) == 0 {
            return;
        }
        if let Some(taps) = self.taps.try_lock() {
            for ring in taps.iter() {
                ring.push(samples, rate);
            }
        }
    }
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}

/// Moniteur de toutes les sorties ouvertes par [`Player::open`].
static MONITOR: Monitor = Monitor::new();

/// Le moniteur des sorties réelles (et de la sortie factice des essais e2e).
pub fn monitor() -> &'static Monitor {
    &MONITOR
}

/// Abonnement au moniteur ; lâché, il se désabonne.
pub struct OutputTap {
    monitor: &'static Monitor,
    ring: Arc<SampleRing>,
    position: u64,
}

impl OutputTap {
    /// Ajoute à `out` ce qui a été joué depuis la lecture précédente (mono) ;
    /// rend son débit (0 si rien n'a encore été joué).
    pub fn read(&mut self, out: &mut Vec<f32>) -> u32 {
        self.ring.read(&mut self.position, out);
        self.ring.rate()
    }
}

impl Drop for OutputTap {
    fn drop(&mut self) {
        let mut taps = self.monitor.taps.lock();
        taps.retain(|ring| !Arc::ptr_eq(ring, &self.ring));
        self.monitor.count.store(taps.len(), Ordering::Release);
    }
}

/// File des échantillons au débit du périphérique, partagée avec la
/// sortie.
struct Queue {
    samples: VecDeque<f32>,
    /// Tampon de départ atteint : la sortie joue.
    playing: bool,
    /// `drain` en cours : jouer jusqu'au bout sans attendre le tampon.
    draining: bool,
    /// Change à chaque `stop` (un `drain` en cours rend faux).
    epoch: u64,
    prebuffer: usize,
    /// Échantillons en file au-delà desquels `push` attend.
    capacity: usize,
    /// Dernier `push` (début après `IDLE_START` sans envoi).
    last_push: Option<Instant>,
    /// Échantillons joués depuis l'ouverture.
    played: u64,
}

impl Queue {
    /// Remplit `out` (mono) ; silence tant que le tampon de départ n'est pas
    /// atteint. Rend le nombre d'échantillons réels.
    fn fill(&mut self, out: &mut [f32]) -> usize {
        let idle = self
            .last_push
            .is_some_and(|last| last.elapsed() >= IDLE_START);
        if !self.playing
            && (self.samples.len() >= self.prebuffer
                || (!self.samples.is_empty() && (self.draining || idle)))
        {
            self.playing = true;
        }
        let mut count = 0;
        if self.playing {
            count = out.len().min(self.samples.len());
            for (slot, sample) in out.iter_mut().zip(self.samples.drain(..count)) {
                *slot = sample;
            }
            // Sous-alimentation : attendre de nouveau un tampon.
            if self.samples.is_empty() && !self.draining {
                self.playing = false;
            }
        }
        out[count..].fill(0.0);
        self.played += count as u64;
        count
    }
}

struct Shared {
    queue: Mutex<Queue>,
    /// Signalé quand la sortie consomme, et à l'arrêt.
    changed: Condvar,
    /// Délai sans consommation après lequel la sortie est tenue pour
    /// bloquée (`STALL`, plus court dans les essais).
    stall: Duration,
    /// Débit de la sortie.
    rate: u32,
    /// Reçoit ce qui est joué.
    monitor: &'static Monitor,
}

impl Shared {
    fn new(rate: u32) -> Arc<Self> {
        Self::with_limits(rate, MAX_QUEUED, STALL)
    }

    fn with_limits(rate: u32, capacity: Duration, stall: Duration) -> Arc<Self> {
        Self::with_monitor(rate, capacity, stall, monitor())
    }

    fn with_monitor(
        rate: u32,
        capacity: Duration,
        stall: Duration,
        monitor: &'static Monitor,
    ) -> Arc<Self> {
        let samples = |duration: Duration| (f64::from(rate) * duration.as_secs_f64()) as usize;
        Arc::new(Self {
            queue: Mutex::new(Queue {
                samples: VecDeque::new(),
                playing: false,
                draining: false,
                epoch: 0,
                prebuffer: samples(PREBUFFER),
                capacity: samples(capacity),
                last_push: None,
                played: 0,
            }),
            changed: Condvar::new(),
            stall,
            rate,
            monitor,
        })
    }

    /// Appelé par la sortie : remplit `out`, passe ce qui est joué au
    /// moniteur et réveille `drain` (qui trouve donc tout le son joué sur
    /// le moniteur).
    fn consume(&self, out: &mut [f32]) -> usize {
        let count = self.queue.lock().fill(out);
        self.monitor.feed(out, self.rate);
        self.changed.notify_all();
        count
    }
}

/// Fil de la sortie ; arrêté (flux fermé) quand le lecteur est lâché.
struct OutputThread {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for OutputThread {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Sortie ouverte sur le périphérique par défaut.
pub struct Player {
    shared: Arc<Shared>,
    resampler: Mutex<Resampler>,
    #[cfg(any(test, feature = "e2e"))]
    null: bool,
    _output: OutputThread,
}

impl Player {
    /// Ouvre la sortie. Erreurs : `AppError::NotFound` s'il n'y a aucun
    /// périphérique de sortie, `AppError::Internal` (détail) s'il ne s'ouvre
    /// pas.
    pub fn open() -> AppResult<Self> {
        #[cfg(feature = "e2e")]
        if std::env::var(E2E_AUDIO_OUT_ENV).is_ok_and(|value| value == "null") {
            return Ok(Self::null(SAMPLE_RATE, 1.0));
        }
        let (ready, opened) = mpsc::channel::<Result<(u32, Arc<Shared>), OpenError>>();
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("chatonlm-speaker".into())
            .spawn(move || match open_stream() {
                Ok((stream, rate, shared)) => {
                    let _ = ready.send(Ok((rate, shared)));
                    // Jusqu'à ce que le lecteur soit lâché.
                    let _ = stopped.recv();
                    drop(stream);
                }
                Err(error) => {
                    let _ = ready.send(Err(error));
                }
            })
            .map_err(|error| AppError::Internal(error.to_string()))?;
        let mut output = OutputThread {
            stop: Some(stop),
            thread: Some(thread),
        };
        // Attente sans limite, ou bornée par le système (Windows).
        let answer = wait_opened(&opened, host::OPEN_WAIT).map_err(|reason| match reason {
            NoAnswer::Stopped => AppError::Internal("the audio output thread stopped".into()),
            NoAnswer::Late => {
                // Fil détaché (l'attendre bloquerait) : il fermera le flux
                // dès qu'il l'aura, son canal d'arrêt étant lâché.
                output.thread.take();
                AppError::Internal("the audio output did not answer".into())
            }
        })?;
        let (rate, shared) = answer.map_err(|error| match error {
            // i18n-ignore : journal ; la page montre son texte
            // (ReadAloudError::NoOutputDevice).
            OpenError::NoDevice => AppError::NotFound(NO_OUTPUT_DEVICE.to_owned()),
            OpenError::Failed(detail) => AppError::Internal(format!("audio output: {detail}")),
        })?;
        Ok(Self {
            shared,
            resampler: Mutex::new(Resampler::new(SAMPLE_RATE, rate)),
            #[cfg(any(test, feature = "e2e"))]
            null: false,
            _output: output,
        })
    }

    /// Sortie factice à `rate` Hz qui consomme au rythme réel multiplié par
    /// `speed` (essais : plus vite).
    #[cfg(any(test, feature = "e2e"))]
    fn null(rate: u32, speed: f64) -> Self {
        Self::null_with(rate, speed, Shared::new(rate))
    }

    #[cfg(any(test, feature = "e2e"))]
    fn null_with(rate: u32, speed: f64, shared: Arc<Shared>) -> Self {
        let consumer = shared.clone();
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("chatonlm-speaker-null".into())
            .spawn(move || {
                let tick = Duration::from_millis(10);
                let mut last = Instant::now();
                let mut owed = 0.0f64;
                let mut scratch = Vec::new();
                while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(tick) {
                    let now = Instant::now();
                    owed += now.duration_since(last).as_secs_f64() * f64::from(rate) * speed;
                    last = now;
                    let count = owed as usize;
                    owed -= count as f64;
                    scratch.resize(count, 0.0);
                    consumer.consume(&mut scratch);
                }
            })
            .expect("null audio output thread");
        Self {
            shared,
            resampler: Mutex::new(Resampler::new(SAMPLE_RATE, rate)),
            null: true,
            _output: OutputThread {
                stop: Some(stop),
                thread: Some(thread),
            },
        }
    }

    /// Ajoute des échantillons mono à `engine::SAMPLE_RATE`. Attend tant
    /// que `MAX_QUEUED` de son sont en file (réveillé par la sortie, ou par
    /// `stop` : le son est alors jeté ; plus d'attente si la sortie ne
    /// consomme plus depuis `STALL`).
    pub fn push(&self, samples: &[f32]) {
        let epoch = {
            let mut queue = self.shared.queue.lock();
            let epoch = queue.epoch;
            let mut progress = (queue.played, Instant::now());
            // `stop` vide la file : l'attente s'arrête aussi.
            while queue.samples.len() >= queue.capacity {
                self.shared
                    .changed
                    .wait_for(&mut queue, Duration::from_millis(50));
                if queue.played != progress.0 {
                    progress = (queue.played, Instant::now());
                } else if progress.1.elapsed() > self.shared.stall {
                    break;
                }
            }
            epoch
        };
        // Ordre des verrous : rééchantillonneur puis file (comme `stop`) :
        // un envoi n'arrive jamais après un arrêt qui l'a suivi.
        let mut resampler = self.resampler.lock();
        if self.shared.queue.lock().epoch != epoch {
            return;
        }
        let mut converted = Vec::with_capacity(samples.len() * 2 + 16);
        resampler.process(samples, &mut converted);
        let mut queue = self.shared.queue.lock();
        queue.samples.extend(converted);
        queue.last_push = Some(Instant::now());
    }

    /// Vide le tampon et coupe le son tout de suite.
    pub fn stop(&self) {
        let mut resampler = self.resampler.lock();
        resampler.reset();
        let mut queue = self.shared.queue.lock();
        queue.samples.clear();
        queue.playing = false;
        queue.draining = false;
        queue.epoch += 1;
        drop(queue);
        self.shared.changed.notify_all();
    }

    /// Attend que tout le tampon soit joué (ou `stop`) ; faux si arrêté.
    pub fn drain(&self) -> bool {
        let mut queue = {
            let mut resampler = self.resampler.lock();
            let mut tail = Vec::new();
            resampler.flush(&mut tail);
            let mut queue = self.shared.queue.lock();
            queue.samples.extend(tail);
            queue
        };
        let epoch = queue.epoch;
        queue.draining = true;
        let mut progress = (queue.played, Instant::now());
        loop {
            if queue.epoch != epoch {
                return false;
            }
            if queue.samples.is_empty() {
                queue.draining = false;
                queue.playing = false;
                #[cfg(any(test, feature = "e2e"))]
                if self.null {
                    eprintln!("chatonlm: e2e audio: {} samples played", queue.played);
                }
                return true;
            }
            self.shared
                .changed
                .wait_for(&mut queue, Duration::from_millis(50));
            if queue.played != progress.0 {
                progress = (queue.played, Instant::now());
            } else if progress.1.elapsed() > self.shared.stall {
                // Sortie bloquée (périphérique débranché...) : abandon.
                eprintln!("chatonlm: audio output stalled, dropping the rest");
                queue.samples.clear();
                queue.draining = false;
                queue.playing = false;
                return true;
            }
        }
    }

    /// Échantillons joués depuis l'ouverture (au débit de la sortie).
    #[cfg(test)]
    pub fn played(&self) -> u64 {
        self.shared.queue.lock().played
    }
}

/// Message de l'erreur « aucun périphérique » (journal seulement).
const NO_OUTPUT_DEVICE: &str = "no audio output device";

/// Échec de l'ouverture de la sortie.
#[derive(Debug, PartialEq)]
enum OpenError {
    NoDevice,
    Failed(String),
}

/// Formats d'échantillons joués.
fn handled(format: SampleFormat) -> bool {
    matches!(
        format,
        SampleFormat::F32
            | SampleFormat::F64
            | SampleFormat::I8
            | SampleFormat::I16
            | SampleFormat::I32
            | SampleFormat::U8
            | SampleFormat::U16
    )
}

/// Configuration de sortie : celle par défaut si son format est joué, sinon
/// celle du périphérique au débit le plus proche du débit par défaut (ou de
/// 48 kHz), en f32, puis i16, puis les autres formats joués.
fn choose_config(
    default: Option<SupportedStreamConfig>,
    ranges: &[SupportedStreamConfigRange],
) -> Option<SupportedStreamConfig> {
    if let Some(config) = &default
        && handled(config.sample_format())
    {
        return default;
    }
    let target = default
        .as_ref()
        .map_or(48_000, |config| config.sample_rate());
    let rank = |format: SampleFormat| match format {
        SampleFormat::F32 => 0,
        SampleFormat::I16 => 1,
        _ => 2,
    };
    ranges
        .iter()
        .filter(|range| handled(range.sample_format()))
        .min_by_key(|range| {
            let rate = target.clamp(range.min_sample_rate(), range.max_sample_rate());
            (rate.abs_diff(target), rank(range.sample_format()))
        })
        .map(|range| {
            range.with_sample_rate(target.clamp(range.min_sample_rate(), range.max_sample_rate()))
        })
}

/// Ouvre et lance le flux de sortie (sur le fil de la sortie).
fn open_stream() -> Result<(cpal::Stream, u32, Arc<Shared>), OpenError> {
    let failed = |error: &dyn std::fmt::Display| OpenError::Failed(error.to_string());
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or(OpenError::NoDevice)?;
    let default = device.default_output_config().ok();
    let ranges: Vec<_> = device
        .supported_output_configs()
        .map(Iterator::collect)
        .unwrap_or_default();
    let supported = choose_config(default, &ranges).ok_or_else(|| match &default {
        Some(config) => OpenError::Failed(format!(
            "unsupported sample format {}",
            config.sample_format()
        )),
        None => OpenError::NoDevice,
    })?;
    let format = supported.sample_format();
    let config = supported.config();
    let channels = usize::from(config.channels.max(1));
    let rate = config.sample_rate;
    let shared = Shared::new(rate);
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, config, channels, shared.clone()),
        SampleFormat::F64 => build::<f64>(&device, config, channels, shared.clone()),
        SampleFormat::I8 => build::<i8>(&device, config, channels, shared.clone()),
        SampleFormat::I16 => build::<i16>(&device, config, channels, shared.clone()),
        SampleFormat::I32 => build::<i32>(&device, config, channels, shared.clone()),
        SampleFormat::U8 => build::<u8>(&device, config, channels, shared.clone()),
        SampleFormat::U16 => build::<u16>(&device, config, channels, shared.clone()),
        other => Err(format!("unsupported sample format {other}")),
    }
    .map_err(|error| failed(&error))?;
    stream.play().map_err(|error| failed(&error))?;
    Ok((stream, rate, shared))
}

/// Canaux qui reçoivent la voix. Linux : tous (comportement d'origine).
/// Windows : les deux premiers (avant gauche et droit) ; sur une sortie 5.1
/// ou 7.1 de WASAPI, le centre, le caisson et les arrières restent muets.
const FRONT: usize = if cfg!(windows) { 2 } else { usize::MAX };

/// Écrit `value` sur les `front` premiers canaux de la trame, `silence` sur
/// les autres.
fn fill_frame<T: Copy>(frame: &mut [T], value: T, silence: T, front: usize) {
    for (channel, slot) in frame.iter_mut().enumerate() {
        *slot = if channel < front { value } else { silence };
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let mut mono: Vec<f32> = Vec::new();
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                mono.resize(data.len() / channels, 0.0);
                shared.consume(&mut mono);
                for (frame, sample) in data.chunks_mut(channels).zip(&mono) {
                    fill_frame(frame, T::from_sample(*sample), T::EQUILIBRIUM, FRONT);
                }
            },
            |error| eprintln!("chatonlm: audio output error: {error}"),
            None,
        )
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sortie à plus de deux canaux (Windows) : la voix sur l'avant, le
    /// reste muet ; sans limite (Linux), tous les canaux.
    #[test]
    fn the_voice_goes_to_the_front_channels() {
        let mut surround = [9i16; 6];
        fill_frame(&mut surround, 5, 0, 2);
        assert_eq!(surround, [5, 5, 0, 0, 0, 0]);
        let mut mono = [9i16; 1];
        fill_frame(&mut mono, 5, 0, 2);
        assert_eq!(mono, [5]);
        let mut all = [9i16; 6];
        fill_frame(&mut all, 5, 0, usize::MAX);
        assert_eq!(all, [5; 6]);
    }

    fn sine(frequency: f64, rate: u32, seconds: f64) -> Vec<f32> {
        let count = (f64::from(rate) * seconds) as usize;
        (0..count)
            .map(|i| {
                (0.5 * (std::f64::consts::TAU * frequency * i as f64 / f64::from(rate)).sin())
                    as f32
            })
            .collect()
    }

    /// Écart RMS relatif au sinus idéal, bords exclus.
    fn error_against(output: &[f32], frequency: f64, rate: u32) -> f64 {
        let ideal = sine(frequency, rate, output.len() as f64 / f64::from(rate));
        let margin = rate as usize / 20;
        let range = margin..output.len().min(ideal.len()) - margin;
        let (mut error, mut power) = (0.0, 0.0);
        for i in range {
            error += (f64::from(output[i]) - f64::from(ideal[i])).powi(2);
            power += f64::from(ideal[i]).powi(2);
        }
        (error / power).sqrt()
    }

    #[test]
    fn speech_frequencies_survive_resampling() {
        for to in [48_000, 44_100, 22_050, 16_000] {
            for frequency in [220.0, 1_000.0, 3_500.0, 6_000.0] {
                if frequency > 0.4 * f64::from(to) {
                    continue;
                }
                let mut resampler = Resampler::new(SAMPLE_RATE, to);
                let mut out = Vec::new();
                resampler.process(&sine(frequency, SAMPLE_RATE, 0.5), &mut out);
                resampler.flush(&mut out);
                let expected = (0.5 * f64::from(to)) as usize;
                assert!(out.len().abs_diff(expected) <= 40, "{to}: {}", out.len());
                let error = error_against(&out, frequency, to);
                assert!(error < 2e-3, "{to} Hz, {frequency} Hz: {error}");
            }
        }
    }

    /// En morceaux de tailles quelconques : exactement le résultat d'un
    /// seul envoi (pas de raccord audible au bord des trames).
    #[test]
    fn streaming_equals_a_single_call() {
        let signal = sine(440.0, SAMPLE_RATE, 0.4);
        for to in [48_000, 44_100] {
            let mut once = Resampler::new(SAMPLE_RATE, to);
            let mut expected = Vec::new();
            once.process(&signal, &mut expected);
            once.flush(&mut expected);
            let mut pieces = Resampler::new(SAMPLE_RATE, to);
            let mut actual = Vec::new();
            let mut start = 0;
            for size in [1, 7, 1920, 333, 1, 4000].iter().cycle() {
                if start >= signal.len() {
                    break;
                }
                let end = (start + size).min(signal.len());
                pieces.process(&signal[start..end], &mut actual);
                start = end;
            }
            pieces.flush(&mut actual);
            assert_eq!(actual.len(), expected.len());
            let worst = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(worst < 1e-6, "{to}: {worst}");
        }
    }

    #[test]
    fn same_rate_is_untouched() {
        let mut resampler = Resampler::new(SAMPLE_RATE, SAMPLE_RATE);
        let mut out = Vec::new();
        resampler.process(&[0.1, 0.2], &mut out);
        resampler.flush(&mut out);
        assert_eq!(out, [0.1, 0.2]);
    }

    fn queue(prebuffer: usize) -> Queue {
        Queue {
            samples: VecDeque::new(),
            playing: false,
            draining: false,
            epoch: 0,
            prebuffer,
            capacity: usize::MAX,
            last_push: None,
            played: 0,
        }
    }

    /// Phrase courte puis plus rien (le modèle cherche sur le web...) : elle
    /// joue dès `IDLE_START` sans envoi, sans attendre le tampon de départ.
    #[test]
    fn a_short_sentence_plays_once_nothing_more_comes() {
        let mut queue = queue(100);
        let mut out = vec![1.0; 40];
        queue.samples.extend(vec![0.5; 30]);
        queue.last_push = Some(Instant::now());
        assert_eq!(queue.fill(&mut out), 0);
        queue.last_push = Instant::now().checked_sub(IDLE_START + Duration::from_millis(50));
        assert_eq!(queue.fill(&mut out), 30);
        assert!(out[..30].iter().all(|s| *s == 0.5));
    }

    fn limited(speed: f64, capacity: Duration, stall: Duration) -> Player {
        Player::null_with(
            SAMPLE_RATE,
            speed,
            Shared::with_limits(SAMPLE_RATE, capacity, stall),
        )
    }

    /// Au-delà de la limite, `push` attend que la sortie consomme.
    #[test]
    fn push_waits_while_the_queue_is_full() {
        let capacity = Duration::from_millis(500);
        let player = limited(10.0, capacity, STALL);
        let piece = vec![0.25; SAMPLE_RATE as usize / 10];
        let limit = (f64::from(SAMPLE_RATE) * capacity.as_secs_f64()) as usize + piece.len();
        let started = Instant::now();
        for _ in 0..30 {
            player.push(&piece);
            let queued = player.shared.queue.lock().samples.len();
            assert!(queued <= limit, "{queued} > {limit}");
        }
        // 3 s de son, 0,5 s en file au plus, sortie à 10× : ~0,25 s.
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert!(player.drain());
    }

    /// `stop` réveille un `push` en attente, dont le son est jeté.
    #[test]
    fn stop_wakes_a_waiting_push() {
        let player = Arc::new(limited(0.0, Duration::from_millis(200), STALL));
        player.push(&vec![0.25; SAMPLE_RATE as usize / 2]);
        let pushing = {
            let player = player.clone();
            std::thread::spawn(move || {
                player.push(&vec![0.5; 2_400]);
                Instant::now()
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        let stopped = Instant::now();
        player.stop();
        let returned = pushing.join().unwrap();
        assert!(returned.duration_since(stopped) < Duration::from_millis(100));
        assert!(player.shared.queue.lock().samples.is_empty());
    }

    /// Sortie qui ne consomme plus (fil mort) : `push` n'attend pas
    /// indéfiniment.
    #[test]
    fn a_dead_output_does_not_block_push() {
        let stall = Duration::from_millis(100);
        let player = Arc::new(limited(0.0, Duration::from_millis(100), stall));
        player.push(&vec![0.25; 4_800]);
        let (done, finished) = mpsc::channel();
        {
            let player = player.clone();
            std::thread::spawn(move || {
                player.push(&vec![0.25; 4_800]);
                let _ = done.send(());
            });
        }
        assert!(
            finished.recv_timeout(Duration::from_secs(1)).is_ok(),
            "push still waiting"
        );
        assert!(player.shared.queue.lock().samples.len() >= 9_600);
    }

    fn range(format: SampleFormat, min: u32, max: u32) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(2, min, max, cpal::SupportedBufferSize::Unknown, format)
    }

    #[test]
    fn an_unplayable_default_format_falls_back_to_a_supported_one() {
        let default = |format| {
            Some(SupportedStreamConfig::new(
                2,
                44_100,
                cpal::SupportedBufferSize::Unknown,
                format,
            ))
        };
        // Format par défaut joué : gardé.
        let kept = choose_config(default(SampleFormat::I16), &[]).unwrap();
        assert_eq!(kept.sample_format(), SampleFormat::I16);
        // I24 par défaut : le débit le plus proche, f32 avant i16 à débit
        // égal.
        let ranges = [
            range(SampleFormat::I24, 8_000, 192_000),
            range(SampleFormat::F32, 8_000, 16_000),
            range(SampleFormat::I16, 8_000, 96_000),
            range(SampleFormat::F32, 48_000, 48_000),
        ];
        let chosen = choose_config(default(SampleFormat::I24), &ranges).unwrap();
        assert_eq!(
            (chosen.sample_format(), chosen.sample_rate()),
            (SampleFormat::I16, 44_100)
        );
        let ranges = [
            range(SampleFormat::I16, 44_100, 44_100),
            range(SampleFormat::F32, 44_100, 44_100),
        ];
        let chosen = choose_config(default(SampleFormat::I24), &ranges).unwrap();
        assert_eq!(chosen.sample_format(), SampleFormat::F32);
        // Sans configuration par défaut : 48 kHz visés.
        let chosen = choose_config(None, &[range(SampleFormat::F32, 8_000, 96_000)]).unwrap();
        assert_eq!(chosen.sample_rate(), 48_000);
        // Rien de jouable.
        assert!(
            choose_config(
                default(SampleFormat::I24),
                &[range(SampleFormat::I24, 1, 9)]
            )
            .is_none()
        );
    }

    /// Silence jusqu'au tampon de départ, puis le son ; après une
    /// sous-alimentation, de nouveau un tampon.
    #[test]
    fn playback_starts_after_the_prebuffer() {
        let mut queue = queue(100);
        let mut out = vec![1.0; 40];
        queue.samples.extend(vec![0.5; 60]);
        assert_eq!(queue.fill(&mut out), 0);
        assert!(out.iter().all(|s| *s == 0.0));
        queue.samples.extend(vec![0.5; 40]);
        assert_eq!(queue.fill(&mut out), 40);
        assert!(out.iter().all(|s| *s == 0.5));
        assert_eq!(queue.fill(&mut out), 40);
        assert_eq!(queue.fill(&mut out), 20);
        assert!(out[20..].iter().all(|s| *s == 0.0));
        // Vide : attente d'un nouveau tampon.
        queue.samples.extend(vec![0.5; 50]);
        assert_eq!(queue.fill(&mut out), 0);
        // Sauf pendant `drain` : tout part.
        queue.draining = true;
        assert_eq!(queue.fill(&mut out), 40);
        assert_eq!(queue.played, 140);
    }

    #[test]
    fn drain_waits_until_everything_is_played() {
        let player = Player::null(48_000, 10.0);
        player.push(&vec![0.25; 12_000]);
        assert!(player.drain());
        // 0,5 s à 24 kHz = 24 000 échantillons à 48 kHz (plus la queue du
        // filtre).
        let played = player.played();
        assert!((24_000..24_100).contains(&played), "{played}");
        // Le lecteur sert encore après.
        player.push(&vec![0.25; 2_400]);
        assert!(player.drain());
    }

    #[test]
    fn stop_silences_at_once_and_ends_the_drain() {
        let player = Arc::new(Player::null(SAMPLE_RATE, 1.0));
        player.push(&vec![0.25; 10 * SAMPLE_RATE as usize]);
        let draining = {
            let player = player.clone();
            std::thread::spawn(move || player.drain())
        };
        std::thread::sleep(Duration::from_millis(400));
        let started = Instant::now();
        player.stop();
        assert!(!draining.join().unwrap());
        assert!(started.elapsed() < Duration::from_millis(200));
        assert!(player.shared.queue.lock().samples.is_empty());
        // Joué : moins d'une seconde sur dix.
        assert!(player.played() < SAMPLE_RATE as u64, "{}", player.played());
    }

    #[test]
    fn the_player_can_be_shared_between_threads() {
        fn shared<T: Send + Sync>() {}
        shared::<Player>();
    }

    #[test]
    fn the_ring_keeps_order_and_the_newest_samples() {
        let ring = SampleRing::new(10);
        let mut position = ring.end();
        let mut out = Vec::new();
        ring.push(&[1.0, 2.0, 3.0], 48_000);
        ring.push(&[4.0], 48_000);
        assert_eq!(ring.read(&mut position, &mut out), 0);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(ring.rate(), 48_000);
        // Plein : les plus vieux sont perdus, et comptés.
        let ramp: Vec<f32> = (5..=19).map(|i| i as f32).collect();
        ring.push(&ramp, 16_000);
        out.clear();
        assert_eq!(ring.read(&mut position, &mut out), 5);
        assert_eq!(out, (10..=19).map(|i| i as f32).collect::<Vec<_>>());
        out.clear();
        assert_eq!(ring.read(&mut position, &mut out), 0);
        assert!(out.is_empty());
        // Plus long que la file d'un coup.
        ring.push(&(0..25).map(|i| i as f32).collect::<Vec<_>>(), 16_000);
        assert_eq!(ring.read(&mut position, &mut out), 15);
        assert_eq!(out, (15..25).map(|i| i as f32).collect::<Vec<_>>());
    }

    fn private_monitor() -> &'static Monitor {
        Box::leak(Box::new(Monitor::new()))
    }

    fn monitored(rate: u32, speed: f64, monitor: &'static Monitor) -> Player {
        Player::null_with(
            rate,
            speed,
            Shared::with_monitor(rate, MAX_QUEUED, STALL, monitor),
        )
    }

    /// L'abonné reçoit ce que la sortie joue réellement, silence du départ
    /// compris, au débit de la sortie.
    #[test]
    fn the_tap_gets_what_the_output_plays() {
        let monitor = private_monitor();
        let mut tap = monitor.subscribe();
        let player = monitored(SAMPLE_RATE, 10.0, monitor);
        player.push(&vec![0.25; 12_000]);
        assert!(player.drain());
        let mut heard = Vec::new();
        assert_eq!(tap.read(&mut heard), SAMPLE_RATE);
        let played = heard.iter().filter(|sample| **sample == 0.25).count();
        assert_eq!(played, 12_000);
        assert!(heard.len() as u64 >= player.played());
        // La suite seulement.
        heard.clear();
        player.push(&vec![0.5; 2_400]);
        assert!(player.drain());
        tap.read(&mut heard);
        assert_eq!(heard.iter().filter(|sample| **sample == 0.5).count(), 2_400);
        assert!(!heard.contains(&0.25));
    }

    /// Sans abonné, rien n'est gardé ; un abonné ne reçoit que ce qui est
    /// joué après son abonnement ; lâché, il se désabonne.
    #[test]
    fn nothing_is_kept_without_a_subscriber() {
        let monitor = private_monitor();
        let player = monitored(SAMPLE_RATE, 10.0, monitor);
        player.push(&vec![0.25; 4_800]);
        assert!(player.drain());
        let mut tap = monitor.subscribe();
        assert_eq!(monitor.count.load(Ordering::Acquire), 1);
        let mut heard = Vec::new();
        tap.read(&mut heard);
        assert!(!heard.contains(&0.25));
        drop(tap);
        assert_eq!(monitor.count.load(Ordering::Acquire), 0);
        assert!(monitor.taps.lock().is_empty());
    }

    /// La sortie n'attend jamais le moniteur : liste des abonnés prise, le
    /// bloc est perdu mais le rappel rend la main.
    #[test]
    fn the_output_never_waits_for_the_monitor() {
        let monitor = private_monitor();
        let mut tap = monitor.subscribe();
        let held = monitor.taps.lock();
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            monitor.feed(&[0.5; 480], 48_000);
            let _ = done.send(());
        });
        assert!(finished.recv_timeout(Duration::from_secs(1)).is_ok());
        drop(held);
        let mut heard = Vec::new();
        tap.read(&mut heard);
        assert!(heard.is_empty());
        monitor.feed(&[0.5; 480], 48_000);
        tap.read(&mut heard);
        assert_eq!(heard.len(), 480);
    }

    /// Un abonné qui ne lit pas : sa file reste bornée (2 s au plus haut
    /// débit), les plus vieux échantillons jetés.
    #[test]
    fn a_slow_subscriber_keeps_a_bounded_queue() {
        let monitor = private_monitor();
        let mut tap = monitor.subscribe();
        for second in 0..5 {
            monitor.feed(&vec![second as f32; TAP_MAX_RATE as usize], TAP_MAX_RATE);
        }
        let mut heard = Vec::new();
        tap.read(&mut heard);
        assert_eq!(heard.len(), 2 * TAP_MAX_RATE as usize);
        assert_eq!(heard[0], 3.0);
        assert_eq!(*heard.last().unwrap(), 4.0);
    }
}
