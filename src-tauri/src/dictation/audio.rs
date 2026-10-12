//! Micro de la dictée : entrée par défaut de l'hôte par défaut de cpal (ALSA,
//! donc PipeWire par pipewire-alsa ; WASAPI sous Windows), dans sa
//! configuration par défaut, ramenée en mono f32 ; rééchantillonnée à 16 kHz à
//! l'arrêt.
//!
//! Le flux de cpal n'est pas `Send` : il vit sur un fil dédié, qui le garde
//! jusqu'à l'arrêt de l'enregistrement.
//!
//! Ce qui dépend du système (attente de l'ouverture, refus du système,
//! changement de périphérique) est dans `audio/linux.rs` et
//! `audio/windows.rs`.

use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use parking_lot::Mutex;

use crate::i18n::t;

#[cfg(target_os = "linux")]
#[path = "audio/linux.rs"]
pub(crate) mod os;
#[cfg(windows)]
#[path = "audio/windows.rs"]
pub(crate) mod os;

/// Fréquence d'échantillonnage des deux moteurs.
pub const RATE: u32 = 16_000;

/// Enregistrement plus court : appui accidentel, abandonné sans rien dire.
pub const MIN_DURATION: Duration = Duration::from_millis(300);

/// Durée maximale d'un enregistrement : arrêt automatique, puis transcription.
pub const MAX_DURATION: Duration = Duration::from_secs(10 * 60);

/// Parole : écart minimal entre les blocs de 30 ms forts et calmes (voir
/// [`has_speech`]).
const SPEECH_RANGE_DB: f32 = 15.0;

/// Parole : les blocs forts dépassent ce niveau (RMS).
const SPEECH_FLOOR_DBFS: f32 = -60.0;

/// Ignoré au début et à la fin pour juger de la parole (clic du raccourci).
const SPEECH_EDGE: usize = 150 * RATE as usize / 1000;

/// Échantillons d'un bloc de 30 ms à 16 kHz.
pub(crate) const BLOCK: usize = 480;

/// Échecs montrés, dans la langue de l'interface.
pub fn no_microphone() -> String {
    t!("dictation.noMicrophone")
}

pub fn no_speech() -> String {
    t!("dictation.noSpeech")
}

/// Micro refusé par les réglages de confidentialité de Windows.
pub fn microphone_blocked() -> String {
    t!("dictation.microphoneBlocked")
}

/// Vrai si `message` (erreur de cpal sous Windows) porte le code
/// `E_ACCESSDENIED` : écrit `0x80070005`, ou `-2147024891` dans une erreur du
/// système (« … (os error -2147024891) »).
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) fn access_denied(message: &str) -> bool {
    message.contains("-2147024891") || message.to_ascii_lowercase().contains("80070005")
}

/// Vrai si l'un des réglages de confidentialité du micro lus dans le registre
/// de Windows (absent : `None`) le refuse.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) fn consent_denied(values: &[Option<String>]) -> bool {
    values
        .iter()
        .flatten()
        .any(|value| value.eq_ignore_ascii_case("deny"))
}

/// Pourquoi le fil qui ouvre un périphérique n'a rien répondu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoAnswer {
    /// Le fil s'est arrêté sans répondre.
    Stopped,
    /// Rien après le délai : le système ne répond pas (le fil finira seul).
    Late,
}

/// Réponse du fil qui ouvre un périphérique, attendue `limit` au plus (sans
/// limite : `None`, voir `os::OPEN_WAIT`).
pub(crate) fn wait_opened<T>(
    answer: &mpsc::Receiver<T>,
    limit: Option<Duration>,
) -> Result<T, NoAnswer> {
    match limit {
        None => answer.recv().map_err(|_| NoAnswer::Stopped),
        Some(limit) => answer.recv_timeout(limit).map_err(|error| match error {
            mpsc::RecvTimeoutError::Disconnected => NoAnswer::Stopped,
            mpsc::RecvTimeoutError::Timeout => NoAnswer::Late,
        }),
    }
}

/// Son capturé, mono.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Captured {
    pub samples: Vec<f32>,
    pub rate: u32,
}

impl Captured {
    /// Son à 16 kHz, prêt pour le moteur.
    pub fn to_16k(&self) -> Vec<f32> {
        resample(&self.samples, self.rate, RATE)
    }
}

/// Source du son (le vrai micro, ou un faux dans les essais).
pub trait Microphone: Send + Sync {
    /// Ouvre l'entrée et commence à enregistrer. Bloquant (appelé dans
    /// `spawn_blocking`). Erreurs en anglais, montrées telles quelles.
    fn open(&self) -> Result<Box<dyn Recording>, String>;
}

/// Enregistrement en cours ; abandonné (micro fermé, son jeté) s'il est lâché
/// sans `finish`.
pub trait Recording: Send {
    /// Arrête et rend le son capturé (bloquant).
    fn finish(self: Box<Self>) -> Captured;
}

/// Le micro du système, par cpal.
#[derive(Default)]
pub struct SystemMicrophone;

impl Microphone for SystemMicrophone {
    fn open(&self) -> Result<Box<dyn Recording>, String> {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let shared = buffer.clone();
        let (ready, opened) = mpsc::channel();
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = thread::Builder::new()
            .name("chatonlm-microphone".into())
            .spawn(move || {
                let stream = match open_stream(shared) {
                    Ok((stream, rate)) => {
                        let _ = ready.send(Ok(rate));
                        stream
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                // Jusqu'à l'arrêt, ou l'abandon de l'enregistrement (canal
                // fermé).
                let _ = stopped.recv();
                drop(stream);
            })
            .map_err(|error| could_not_open(&error))?;
        // En retard : le fil, détaché, fermera le flux dès qu'il l'aura
        // (son canal d'arrêt est lâché ici).
        let rate = wait_opened(&opened, os::OPEN_WAIT).map_err(|reason| match reason {
            NoAnswer::Stopped => could_not_open(&"the audio thread stopped"),
            NoAnswer::Late => could_not_open(&"the audio device did not answer"),
        })??;
        Ok(Box::new(SystemRecording {
            buffer,
            rate,
            stop: Some(stop),
            thread: Some(thread),
        }))
    }
}

struct SystemRecording {
    buffer: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl SystemRecording {
    fn close(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Recording for SystemRecording {
    fn finish(mut self: Box<Self>) -> Captured {
        self.close();
        Captured {
            samples: std::mem::take(&mut *self.buffer.lock()),
            rate: self.rate,
        }
    }
}

impl Drop for SystemRecording {
    fn drop(&mut self) {
        // Abandon : le fil ferme le flux sans qu'on l'attende.
        self.stop.take();
    }
}

/// Micro impossible à ouvrir ; `error` : détail technique.
pub(crate) fn could_not_open(error: &dyn std::fmt::Display) -> String {
    t!("dictation.microphoneFailed", error = error)
}

/// Échec de cpal à l'ouverture : refus du système expliqué (Windows), sinon
/// le détail technique.
fn open_failed(error: &cpal::Error) -> String {
    if os::blocked(error) {
        microphone_blocked()
    } else {
        could_not_open(error)
    }
}

/// Ouvre et lance le flux (sur le fil du micro).
fn open_stream(buffer: Arc<Mutex<Vec<f32>>>) -> Result<(cpal::Stream, u32), String> {
    open_input(
        |rate| {
            // Au-delà, le son n'est plus gardé (l'arrêt automatique suit).
            let limit = (MAX_DURATION.as_secs() as usize + 5) * rate as usize;
            move |mono: &[f32]| {
                let mut buffer = buffer.lock();
                if buffer.len() < limit {
                    buffer.extend_from_slice(mono);
                }
            }
        },
        |error| eprintln!("chatonlm: microphone error: {error}"),
    )
}

/// Ouvre l'entrée par défaut dans sa configuration par défaut et la lance,
/// sur le fil qui gardera le flux (il n'est pas `Send`). `on_data`, fabriqué
/// par `make` d'après le débit, reçoit chaque bloc ramené en mono f32 :
/// appelé par le rappel audio, il ne doit ni attendre ni calculer longtemps.
/// Rend le flux et son débit. Partagée par la dictée et le mode vocal.
pub(crate) fn open_input<M, D, E>(make: M, on_error: E) -> Result<(cpal::Stream, u32), String>
where
    M: FnOnce(u32) -> D,
    D: FnMut(&[f32]) + Send + 'static,
    E: FnMut(cpal::Error) + Send + 'static,
{
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or_else(no_microphone)?;
    let supported = device
        .default_input_config()
        .map_err(|error| open_failed(&error))?;
    let format = supported.sample_format();
    let config = supported.config();
    let channels = usize::from(config.channels.max(1));
    let rate = config.sample_rate;
    let on_data = make(rate);
    let stream = match format {
        SampleFormat::F32 => build_input::<f32, _, _>(&device, config, channels, on_data, on_error),
        SampleFormat::I16 => build_input::<i16, _, _>(&device, config, channels, on_data, on_error),
        SampleFormat::I32 => build_input::<i32, _, _>(&device, config, channels, on_data, on_error),
        SampleFormat::U16 => build_input::<u16, _, _>(&device, config, channels, on_data, on_error),
        // WASAPI : formats de mixage possibles en plus (f32 en pratique).
        #[cfg(windows)]
        SampleFormat::I8 => build_input::<i8, _, _>(&device, config, channels, on_data, on_error),
        #[cfg(windows)]
        SampleFormat::U8 => build_input::<u8, _, _>(&device, config, channels, on_data, on_error),
        #[cfg(windows)]
        SampleFormat::I24 => {
            build_input::<cpal::I24, _, _>(&device, config, channels, on_data, on_error)
        }
        #[cfg(windows)]
        SampleFormat::F64 => build_input::<f64, _, _>(&device, config, channels, on_data, on_error),
        other => Err(could_not_open(&format!(
            "unsupported sample format {other}"
        ))),
    }?;
    stream.play().map_err(|error| open_failed(&error))?;
    Ok((stream, rate))
}

fn build_input<T, D, E>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    mut on_data: D,
    on_error: E,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
    D: FnMut(&[f32]) + Send + 'static,
    E: FnMut(cpal::Error) + Send + 'static,
{
    // Tampon du rappel, gardé d'un bloc à l'autre (pas d'allocation après
    // le premier).
    let mut mono = Vec::new();
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                mono.clear();
                push_mono(&mut mono, data, channels);
                on_data(&mono);
            },
            on_error,
            None,
        )
        .map_err(|error| open_failed(&error))
}

/// Ajoute des trames entrelacées, chaque trame ramenée à la moyenne de ses
/// canaux.
pub(crate) fn push_mono<T>(buffer: &mut Vec<f32>, data: &[T], channels: usize)
where
    T: Copy,
    f32: FromSample<T>,
{
    let channels = channels.max(1);
    buffer.reserve(data.len() / channels);
    for frame in data.chunks(channels) {
        let sum: f32 = frame.iter().map(|&sample| f32::from_sample_(sample)).sum();
        buffer.push(sum / frame.len() as f32);
    }
}

/// Demi-largeur du filtre, en passages par zéro du sinus cardinal.
const ZEROS: f64 = 16.0;

/// Pas de la table du noyau : 512 points par échantillon d'entrée.
const TABLE_STEP: f64 = 512.0;

/// Rééchantillonne `samples` de `from` Hz vers `to` Hz : interpolation par
/// sinus cardinal fenêtré (Blackman), coupé à 90 % de la plus basse des deux
/// fréquences de Nyquist (pas de repliement quand la fréquence baisse).
pub fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 || to == 0 || samples.is_empty() {
        return samples.to_vec();
    }
    // Pas, en échantillons d'entrée, entre deux échantillons de sortie.
    let step = f64::from(from) / f64::from(to);
    // Fréquence de coupure, en fraction de la fréquence de Nyquist d'entrée.
    let cutoff = (1.0 / step).min(1.0) * 0.9;
    // Demi-largeur du noyau, en échantillons d'entrée.
    let width = ZEROS / cutoff;
    let table = kernel_table(cutoff, width);
    let length = (samples.len() as f64 / step).floor() as usize;
    let last_input = samples.len() - 1;
    let mut output = Vec::with_capacity(length);
    for index in 0..length {
        let center = index as f64 * step;
        let first = (center - width).ceil().max(0.0) as usize;
        let last = ((center + width).floor() as usize).min(last_input);
        let (mut sum, mut weights) = (0.0f64, 0.0f64);
        for (offset, &sample) in samples[first..=last].iter().enumerate() {
            let weight = lookup(&table, center - (first + offset) as f64 + width);
            sum += weight * f64::from(sample);
            weights += weight;
        }
        // Normalisé : gain 1 pour un signal constant, bords compris.
        output.push(if weights.abs() > 1e-9 {
            (sum / weights) as f32
        } else {
            0.0
        });
    }
    output
}

/// Noyau échantillonné sur [-width, width], `TABLE_STEP` points par
/// échantillon d'entrée.
fn kernel_table(cutoff: f64, width: f64) -> Vec<f64> {
    use std::f64::consts::PI;
    let points = (2.0 * width * TABLE_STEP).ceil() as usize + 2;
    (0..points)
        .map(|point| {
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
        .collect()
}

/// Valeur du noyau en `position` (décalage + demi-largeur), interpolée.
fn lookup(table: &[f64], position: f64) -> f64 {
    let scaled = position * TABLE_STEP;
    let index = scaled.floor();
    let fraction = scaled - index;
    let index = index as usize;
    match (table.get(index), table.get(index + 1)) {
        (Some(a), Some(b)) => a + (b - a) * fraction,
        (Some(a), None) => *a,
        _ => 0.0,
    }
}

/// Durée de `samples` à 16 kHz.
pub fn duration(samples: &[f32]) -> Duration {
    Duration::from_secs_f64(samples.len() as f64 / f64::from(RATE))
}

/// Vrai s'il y a quelque chose à transcrire (Whisper invente du texte sur
/// le silence). Critère relatif, sur les niveaux (RMS) des blocs de 30 ms à
/// 16 kHz, 150 ms ignorées au début et à la fin : les blocs forts (95e
/// centile) dépassent les calmes (20e centile) d'au moins 15 dB et
/// -60 dBFS. Un seuil absolu laissait passer le souffle d'un micro poussé par
/// le contrôle de gain, et un clic de clavier.
pub fn has_speech(samples: &[f32]) -> bool {
    let trimmed = if samples.len() > 2 * SPEECH_EDGE + 10 * BLOCK {
        &samples[SPEECH_EDGE..samples.len() - SPEECH_EDGE]
    } else {
        samples
    };
    let mut levels: Vec<f32> = trimmed
        .chunks(BLOCK)
        .filter(|block| block.len() == BLOCK || trimmed.len() < BLOCK)
        .map(|block| {
            let energy: f32 = block.iter().map(|sample| sample * sample).sum();
            // -100 dBFS au plus bas (silence numérique).
            20.0 * (energy / block.len() as f32).sqrt().max(1e-5).log10()
        })
        .collect();
    if levels.is_empty() {
        return false;
    }
    levels.sort_by(f32::total_cmp);
    let percentile = |rank: f32| levels[((levels.len() - 1) as f32 * rank).round() as usize];
    let (loud, quiet) = (percentile(0.95), percentile(0.20));
    loud > SPEECH_FLOOR_DBFS && loud - quiet >= SPEECH_RANGE_DB
}

/// Lit un WAV (PCM entier ou flottant, toute fréquence), ramené en mono.
#[cfg(any(test, feature = "e2e"))]
pub fn read_wav(path: &std::path::Path) -> Result<Captured, String> {
    let mut reader = hound::WavReader::open(path).map_err(|error| could_not_open(&error))?;
    let spec = reader.spec();
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample.clamp(1, 32) - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|sample| sample.map(|value| value as f32 * scale))
                .collect()
        }
    }
    .map_err(|error| could_not_open(&error))?;
    let mut samples = Vec::new();
    push_mono(&mut samples, &interleaved, usize::from(spec.channels));
    Ok(Captured {
        samples,
        rate: spec.sample_rate,
    })
}

/// Son déjà enregistré, rendu tel quel à l'arrêt (micro simulé).
#[cfg(any(test, feature = "e2e"))]
pub struct Prerecorded(pub Captured);

#[cfg(any(test, feature = "e2e"))]
impl Recording for Prerecorded {
    fn finish(self: Box<Self>) -> Captured {
        self.0
    }
}

/// Micro simulé des essais e2e (`CHATONLM_E2E_MICROPHONE`) : chemin d'un WAV
/// rendu à l'arrêt, ou `none` (pas de micro).
#[cfg(feature = "e2e")]
pub struct WavMicrophone(pub std::ffi::OsString);

#[cfg(feature = "e2e")]
impl Microphone for WavMicrophone {
    fn open(&self) -> Result<Box<dyn Recording>, String> {
        if self.0 == "none" {
            return Err(no_microphone());
        }
        Ok(Box::new(Prerecorded(read_wav(std::path::Path::new(
            &self.0,
        ))?)))
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::PI;

    use super::*;

    fn sine(frequency: f32, rate: u32, seconds: f32, amplitude: f32) -> Vec<f32> {
        let count = (rate as f32 * seconds) as usize;
        (0..count)
            .map(|i| amplitude * (2.0 * PI * frequency * i as f32 / rate as f32).sin())
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    /// Refus de la confidentialité de Windows, reconnu dans le texte de cpal
    /// (message du système traduit, code compris).
    #[test]
    fn access_denied_is_recognised_by_its_code() {
        for denied in [
            "Failed to initialize audio client: Accès refusé. (os error -2147024891)",
            "Access is denied. (0x80070005)",
            "HRESULT(0x80070005)",
            "hresult 0X80070005",
        ] {
            assert!(access_denied(denied), "{denied}");
        }
        for other in [
            "Failed to initialize audio client: device in use (os error -2004287478)",
            "ALSA function 'snd_pcm_open' failed with error 'ENOENT: No such file or directory'",
            "",
        ] {
            assert!(!access_denied(other), "{other}");
        }
    }

    #[test]
    fn one_denied_setting_is_enough() {
        let (allow, deny) = (Some("Allow".to_owned()), Some("Deny".to_owned()));
        assert!(!consent_denied(&[]));
        assert!(!consent_denied(&[None, None, None]));
        assert!(!consent_denied(&[allow.clone(), None, allow.clone()]));
        assert!(consent_denied(&[allow.clone(), allow.clone(), deny]));
        assert!(consent_denied(&[Some("deny".to_owned()), None, allow]));
    }

    /// Fil d'ouverture : sa réponse, son arrêt, ou son retard quand
    /// l'attente est bornée.
    #[test]
    fn the_opening_thread_is_waited_within_the_limit() {
        let limit = Some(Duration::from_millis(50));
        let (sender, answer) = mpsc::channel();
        sender.send(7).unwrap();
        assert_eq!(wait_opened(&answer, limit), Ok(7));
        // Rien encore : en retard (borné), puis la réponse arrive.
        assert_eq!(wait_opened(&answer, limit), Err(NoAnswer::Late));
        sender.send(8).unwrap();
        assert_eq!(wait_opened(&answer, None), Ok(8));
        // Fil arrêté : dit tout de suite, avec ou sans limite.
        drop(sender);
        assert_eq!(wait_opened(&answer, limit), Err(NoAnswer::Stopped));
        assert_eq!(wait_opened(&answer, None), Err(NoAnswer::Stopped));
    }

    #[test]
    fn the_blocked_microphone_names_the_windows_setting_in_every_language() {
        for lang in [
            crate::i18n::Lang::En,
            crate::i18n::Lang::Es,
            crate::i18n::Lang::Fr,
        ] {
            let message = t!("dictation.microphoneBlocked" in lang);
            assert!(message.contains("Windows"), "{message}");
            assert!(message.to_lowercase().contains("micr"), "{message}");
        }
    }

    /// Écart RMS au sinus attendu, loin des bords.
    fn error_against(output: &[f32], frequency: f32, amplitude: f32) -> f32 {
        let expected = sine(
            frequency,
            RATE,
            output.len() as f32 / RATE as f32,
            amplitude,
        );
        let middle = 800..output.len() - 800;
        let diff: Vec<f32> = middle.clone().map(|i| output[i] - expected[i]).collect();
        rms(&diff)
    }

    #[test]
    fn speech_band_survives_resampling_from_usual_rates() {
        for from in [48_000, 44_100, 22_050, 8_000] {
            let input = sine(440.0, from, 1.0, 0.5);
            let output = resample(&input, from, RATE);
            assert!(
                output.len().abs_diff(RATE as usize) <= 1,
                "{from}: {}",
                output.len()
            );
            let error = error_against(&output, 440.0, 0.5);
            assert!(error < 0.01, "{from} Hz: error {error}");
        }
    }

    #[test]
    fn frequencies_above_8_khz_are_filtered_out() {
        // Sans filtre, 10 kHz à 48 kHz deviendrait un 6 kHz à pleine amplitude.
        let input = sine(10_000.0, 48_000, 1.0, 0.5);
        let output = resample(&input, 48_000, RATE);
        let level = rms(&output[800..output.len() - 800]);
        assert!(level < 0.005, "level {level}");
    }

    #[test]
    fn same_rate_is_untouched() {
        let input = sine(440.0, RATE, 0.1, 0.5);
        assert_eq!(resample(&input, RATE, RATE), input);
        assert!(resample(&[], 48_000, RATE).is_empty());
    }

    #[test]
    fn channels_are_averaged() {
        let mut mono = Vec::new();
        push_mono(&mut mono, &[1.0f32, 0.0, 0.5, 0.5, -1.0, 1.0], 2);
        assert_eq!(mono, [0.5, 0.5, 0.0]);
        let mut from_i16 = Vec::new();
        push_mono(&mut from_i16, &[i16::MIN, 0i16], 1);
        assert_eq!(from_i16, [-1.0, 0.0]);
    }

    /// Syllabes simulées : 250 ms de son, 150 ms de silence, en boucle.
    fn syllables(seconds: f32, amplitude: f32) -> Vec<f32> {
        sine(300.0, RATE, seconds, amplitude)
            .into_iter()
            .enumerate()
            .map(|(i, sample)| if i % 6_400 < 4_000 { sample } else { 0.0 })
            .collect()
    }

    /// Bruit pseudo-aléatoire (souffle), d'amplitude crête `amplitude`.
    fn hiss(seconds: f32, amplitude: f32) -> Vec<f32> {
        let mut state = 7u32;
        (0..(RATE as f32 * seconds) as usize)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                ((state >> 16) as f32 / 32_768.0 - 1.0) * amplitude
            })
            .collect()
    }

    #[test]
    fn silence_hiss_and_clicks_are_not_speech() {
        assert!(!has_speech(&vec![0.0; 32_000]));
        // Souffle d'un micro poussé par le contrôle de gain : -35 dBFS
        // environ, régulier (un seuil absolu de -50 dBFS le laissait passer).
        assert!(!has_speech(&hiss(2.0, 0.03)));
        // Silence et un clic de clavier de 30 ms à -10 dBFS.
        let mut click = vec![0.0; 32_000];
        click[16_000..16_480].copy_from_slice(&sine(300.0, RATE, 0.03, 0.4)[..480]);
        assert!(!has_speech(&click));
        // Clic du raccourci dans les 150 premières ms, parmi le souffle.
        let mut start = hiss(2.0, 0.01);
        start[..2_400].copy_from_slice(&sine(300.0, RATE, 0.15, 0.5)[..2_400]);
        assert!(!has_speech(&start));
    }

    #[test]
    fn speech_stands_out_of_its_pauses() {
        assert!(has_speech(&syllables(2.0, 0.1)));
        // Parole sur un souffle de fond, 20 dB plus bas.
        let mixed: Vec<f32> = syllables(2.0, 0.1)
            .iter()
            .zip(hiss(2.0, 0.01))
            .map(|(speech, noise)| speech + noise)
            .collect();
        assert!(has_speech(&mixed));
        // Micro très faible (-55 dBFS) : encore de la parole ; sous -60 dBFS,
        // non.
        assert!(has_speech(&syllables(2.0, 0.0025)));
        assert!(!has_speech(&syllables(2.0, 0.0008)));
        // Un mot bref, sans marge à couper.
        assert!(has_speech(&syllables(0.4, 0.1)));
    }

    #[test]
    fn durations_are_measured_at_16_khz() {
        assert_eq!(duration(&vec![0.0; 4_800]), Duration::from_millis(300));
        assert!(duration(&vec![0.0; 4_799]) < MIN_DURATION);
    }

    /// Contrat : de vraies voix passent le critère (échantillons de `.tools`).
    #[test]
    #[ignore = "needs the speech samples of .tools"]
    fn contract_real_speech_is_detected() {
        for name in ["jfk.wav", "fr0.wav", "fr1.wav", "pnc.wav"] {
            assert!(
                has_speech(&crate::dictation::test_data::sample(name)),
                "{name}"
            );
        }
    }

    #[test]
    fn wav_files_are_read_as_mono() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stereo.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..10 {
            writer.write_sample(16_384i16).unwrap();
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
        let captured = read_wav(&path).unwrap();
        assert_eq!(captured.rate, 48_000);
        assert_eq!(captured.samples, vec![0.25; 10]);
        assert!(
            read_wav(&dir.path().join("missing.wav"))
                .unwrap_err()
                .starts_with("Could not open the microphone: ")
        );
    }
}
