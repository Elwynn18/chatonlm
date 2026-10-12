//! Annulation d'écho : AEC3 de WebRTC (crate `sonora`, Rust pur), 16 kHz
//! mono, trames de 10 ms. Référence (« render ») : ce que la sortie joue,
//! lu sur le moniteur de `tts::playback` et ramené à 16 kHz ; silence quand
//! rien ne joue. La trame de référence part toujours avant la trame du micro
//! qui lui correspond.
//!
//! Réglages : AEC3 seul, filtre passe-haut ; ni suppression de bruit (elle
//! abîmerait la parole donnée à la détection et à la transcription), ni gain
//! automatique (le niveau du micro reste celui du système). Le retard entre
//! la sortie et le micro est estimé par AEC3 (filtres adaptés : jusqu'à
//! ~500 ms) ; essais de 40 à 250 ms. Tant qu'AEC3 n'a pas convergé, la
//! détection prend le micro brut sous la garde de l'écho ([`EchoGuard`]).

use std::collections::VecDeque;

use sonora::config::{EchoCanceller, HighPassFilter};
use sonora::{AudioProcessing, Config, StreamConfig};

use super::RATE;

/// Trame de l'annulation d'écho : 10 ms à 16 kHz.
pub const AEC_FRAME: usize = RATE as usize / 100;

/// Annuleur d'écho d'une écoute.
pub struct Canceller {
    processing: AudioProcessing,
    /// Sortie de `process_render_f32` (inutilisée : la référence n'est pas
    /// modifiée).
    scratch: [f32; AEC_FRAME],
}

impl Canceller {
    pub fn new() -> Self {
        let config = Config {
            echo_canceller: Some(EchoCanceller {
                enforce_high_pass_filtering: true,
                ..EchoCanceller::default()
            }),
            high_pass_filter: Some(HighPassFilter::default()),
            ..Config::default()
        };
        let stream = StreamConfig::new(RATE, 1);
        Self {
            processing: AudioProcessing::builder()
                .config(config)
                .capture_config(stream)
                .render_config(stream)
                .build(),
            scratch: [0.0; AEC_FRAME],
        }
    }

    /// Trame jouée (référence), avant la trame du micro correspondante.
    pub fn render(&mut self, frame: &[f32; AEC_FRAME]) {
        // Formats fixes et valides : jamais d'erreur.
        let _ = self
            .processing
            .process_render_f32(&[&frame[..]], &mut [&mut self.scratch[..]]);
    }

    /// Trame du micro ; `out` : la même, sans l'écho.
    pub fn capture(&mut self, frame: &[f32; AEC_FRAME], out: &mut [f32; AEC_FRAME]) {
        let _ = self
            .processing
            .process_capture_f32(&[&frame[..]], &mut [&mut out[..]]);
    }

    /// Écho retiré par le filtre d'AEC3 selon sa propre estimation (dB,
    /// 0 avant convergence).
    pub fn erle_db(&self) -> f64 {
        self.processing
            .statistics()
            .echo_return_loss_enhancement
            .unwrap_or(0.0)
    }
}

/// Retard que la file peut prendre au-delà de son niveau de base avant
/// d'être raccourcie (dérive des horloges de la sortie et du micro).
const MAX_DRIFT: usize = 20 * RATE as usize / 1000;
/// Échantillons retirés par trame pour rattraper une dérive : assez peu
/// pour qu'AEC3 suive sans voir un nouveau retard (et tout remettre à zéro).
const DRIFT_STEP: usize = 2;
/// Trames sur lesquelles le plus bas niveau de la file est observé.
const SLACK_WINDOW: usize = 100;
/// Plus rien reçu depuis ce nombre de trames : la lecture est finie.
const IDLE_FRAMES: usize = 30;
/// Au-delà (fil de l'écoute bloqué, sortie relancée…), les plus vieux
/// échantillons sont jetés : de quoi couvrir un retard du fil de 2 s.
const MAX_QUEUED: usize = 2 * RATE as usize;

/// File de la référence à 16 kHz : une trame en sort pour chaque trame du
/// micro. Elle reçoit le son par rafales (rappels de la sortie). Le niveau
/// le plus bas atteint pendant la première seconde d'une lecture sert de
/// base : le retard qu'il représente est celui qu'AEC3 estime. Au-delà de
/// la base (horloges qui dérivent), l'excédent est retiré peu à peu ; une
/// trame qui manque est remplacée par une trame de silence entière, le son
/// en attente restant en file. Ainsi le retard vu par AEC3 ne saute pas, ce
/// qui le remettrait à zéro (filtres, ERLE).
#[derive(Default)]
pub struct RenderQueue {
    samples: VecDeque<f32>,
    /// Lecture en cours (du son est arrivé récemment).
    playing: bool,
    /// Trames sans son reçu.
    idle: usize,
    /// Son reçu depuis la trame précédente.
    received: bool,
    /// Plus bas niveau de la file depuis le début de la fenêtre.
    lowest: Option<usize>,
    frames: usize,
    /// Niveau de base de la lecture (première fenêtre).
    baseline: Option<usize>,
    /// Échantillons à retirer peu à peu.
    excess: usize,
}

impl RenderQueue {
    pub fn push(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        self.samples.extend(samples);
        self.received = true;
        if self.samples.len() > MAX_QUEUED {
            let extra = self.samples.len() - MAX_QUEUED;
            self.samples.drain(..extra);
        }
    }

    /// Trame suivante de la référence (silence à défaut).
    pub fn pop(&mut self, frame: &mut [f32; AEC_FRAME]) {
        if std::mem::take(&mut self.received) {
            self.idle = 0;
            if !self.playing {
                self.playing = true;
                (self.lowest, self.frames, self.baseline, self.excess) = (None, 0, None, 0);
            }
        } else if self.playing {
            self.idle += 1;
            if self.idle > IDLE_FRAMES && self.samples.len() < AEC_FRAME {
                // Fin de lecture : la suivante repart de zéro.
                self.playing = false;
                self.samples.clear();
            }
        }
        if self.samples.len() < AEC_FRAME {
            frame.fill(0.0);
            return;
        }
        for (slot, sample) in frame.iter_mut().zip(self.samples.drain(..AEC_FRAME)) {
            *slot = sample;
        }
        if self.excess > 0 && self.samples.len() > AEC_FRAME + DRIFT_STEP {
            let step = self.excess.min(DRIFT_STEP);
            self.samples.drain(..step);
            self.excess -= step;
        }
        let level = self.samples.len();
        self.lowest = Some(self.lowest.map_or(level, |lowest| lowest.min(level)));
        self.frames += 1;
        if self.frames >= SLACK_WINDOW {
            if let Some(lowest) = self.lowest {
                match self.baseline {
                    None => self.baseline = Some(lowest),
                    Some(baseline) if lowest > baseline + MAX_DRIFT && self.excess == 0 => {
                        self.excess = lowest - baseline;
                    }
                    Some(_) => {}
                }
            }
            self.lowest = None;
            self.frames = 0;
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.samples.len()
    }
}

/// Référence observée pour l'écho attendu : 500 ms (plage de retard
/// d'AEC3), en trames de 10 ms.
const GUARD_WINDOW: usize = 50;
/// La sortie joue (énergie de la référence, -50 dBFS).
const ACTIVE: f32 = 1e-5;
/// Retards essayés pour le couplage : 0 à 480 ms, par pas de 20 ms.
const DELAY_STEPS: usize = 25;
const DELAY_STEP: usize = 2;
/// Son joué assez fort pour que son écho domine le bruit du micro : à
/// 10 dB au plus du plus fort des dernières 500 ms.
const LOUD: f32 = 0.1;
/// Rapports gardés par retard : 3 s de son joué fort.
const COUPLING_HISTORY: usize = 300;
/// Rapports nécessaires avant de se fier au couplage (0,5 s de son joué
/// fort) ; avant, l'écho est supposé aussi fort que le son joué.
const COUPLING_MIN: usize = 50;
/// Trames entre deux calculs du couplage.
const COUPLING_EVERY: usize = 10;
/// Rang retenu : 20e centile, l'enveloppe basse.
const COUPLING_RANK: f32 = 0.2;
/// Marge ajoutée (dB) : l'enveloppe basse sous-estime l'écho des syllabes
/// qui suivent un son fort (réverbération).
const COUPLING_MARGIN_DB: f32 = 6.0;
/// AEC3 tenu pour convergé au-delà de cette ERLE estimée (dB), et de
/// nouveau pour non convergé en dessous de `DIVERGED_ERLE_DB` (elle oscille
/// autour de 10 dB pendant une double parole ; AEC3 la remet à 0 quand le
/// chemin de l'écho change).
const CONVERGED_ERLE_DB: f64 = 10.0;
const DIVERGED_ERLE_DB: f64 = 6.0;

/// Garde de l'écho, pour le démarrage d'AEC3. Tant qu'il n'a pas convergé
/// (ERLE estimée de 0 dB pendant ~3 s sur `jfk.wav`, à jamais sans écho :
/// casque, sortie factice), AEC3 suppose un écho aussi fort que le son
/// joué et efface aussi la voix proche (mesuré : plus de 7 s sans écho).
/// Pendant que la sortie joue, l'écoute prend alors le micro brut, et une
/// trame n'y est de la parole proche que si elle dépasse l'écho attendu :
/// le plus fort son joué des dernières 500 ms × le couplage haut-parleur →
/// micro. Convergé, AEC3 suffit (sa sortie, sans garde).
///
/// Le couplage est l'enveloppe basse du rapport micro brut / son joué au
/// bon retard : l'écho est toujours là, la voix proche et le bruit ne font
/// qu'ajouter de l'énergie. Pour chaque retard de 0 à 480 ms, le 20e
/// centile des rapports (trames où le son joué à ce retard est fort) ; le
/// plus grand d'entre eux, au vrai retard, plus 6 dB. Ni la détection ni
/// la parole n'entrent dans l'apprentissage : une voix proche ne le gonfle
/// pas (il faudrait qu'elle couvre 80 % des 3 dernières secondes de son
/// joué fort), un écho qui apparaît (casque débranché) l'élève en ~2,5 s. Casque : couplage
/// infime, rien n'est masqué ; rien ne joue : rien n'est masqué.
pub struct EchoGuard {
    /// Énergie des dernières trames de la référence (la plus récente en
    /// dernier).
    render: VecDeque<f32>,
    /// Rapports micro brut / référence (dB), par retard essayé.
    ratios: Vec<VecDeque<f32>>,
    /// Couplage (rapport d'énergies).
    coupling: f32,
    /// Trames depuis le dernier calcul du couplage.
    stale: usize,
    /// AEC3 a convergé (hystérésis sur son ERLE estimée).
    converged: bool,
}

impl Default for EchoGuard {
    fn default() -> Self {
        Self {
            render: VecDeque::with_capacity(GUARD_WINDOW + 1),
            ratios: (0..DELAY_STEPS)
                .map(|_| VecDeque::with_capacity(COUPLING_HISTORY + 1))
                .collect(),
            coupling: 1.0,
            stale: 0,
            converged: false,
        }
    }
}

/// Énergie moyenne (carré moyen).
pub fn energy(samples: &[f32]) -> f32 {
    samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len().max(1) as f32
}

fn decibels(energy: f32) -> f32 {
    10.0 * energy.max(1e-12).log10()
}

/// Valeur de rang `rank` (0 à 1) de `history`.
fn percentile(history: &VecDeque<f32>, rank: f32) -> f32 {
    let mut sorted: Vec<f32> = history.iter().copied().collect();
    let index = ((sorted.len() - 1) as f32 * rank) as usize;
    *sorted.select_nth_unstable_by(index, f32::total_cmp).1
}

impl EchoGuard {
    /// Trame de 10 ms : énergies de la référence et du micro brut, ERLE
    /// estimée par AEC3 (dB).
    pub fn add(&mut self, render: f32, raw: f32, erle_db: f64) {
        self.converged = if self.converged {
            erle_db >= DIVERGED_ERLE_DB
        } else {
            erle_db >= CONVERGED_ERLE_DB
        };
        self.render.push_back(render);
        if self.render.len() > GUARD_WINDOW {
            self.render.pop_front();
        }
        let reference = self.reference();
        if reference > ACTIVE {
            let newest = self.render.len() - 1;
            for (step, ratios) in self.ratios.iter_mut().enumerate() {
                let Some(index) = newest.checked_sub(step * DELAY_STEP) else {
                    break;
                };
                let played = self.render[index];
                if played > ACTIVE && played >= LOUD * reference {
                    ratios.push_back(decibels(raw) - decibels(played));
                    if ratios.len() > COUPLING_HISTORY {
                        ratios.pop_front();
                    }
                }
            }
        }
        self.stale += 1;
        if self.stale >= COUPLING_EVERY {
            self.stale = 0;
            self.coupling = self
                .ratios
                .iter()
                .filter(|ratios| ratios.len() >= COUPLING_MIN)
                .map(|ratios| percentile(ratios, COUPLING_RANK))
                .reduce(f32::max)
                .map_or(1.0, |ratio| 10f32.powf((ratio + COUPLING_MARGIN_DB) / 10.0));
        }
    }

    /// Plus forte énergie jouée des dernières 500 ms.
    fn reference(&self) -> f32 {
        self.render.iter().copied().fold(0.0, f32::max)
    }

    /// AEC3 n'a pas convergé et la sortie joue : l'écoute prend le micro
    /// brut, sous la garde.
    pub fn bypasses(&self) -> bool {
        !self.converged && self.reference() > ACTIVE
    }

    /// Écart (dB) entre le son entendu et l'écho attendu ; `None` quand rien
    /// ne joue.
    pub fn excess(&self, heard: f32) -> Option<f32> {
        let reference = self.reference();
        (reference > ACTIVE)
            .then(|| 10.0 * (heard.max(1e-12) / (self.coupling * reference)).log10())
    }

    /// Trame entendue d'énergie `heard` à tenir pour de l'écho (AEC3 non
    /// convergé, pas plus forte que l'écho attendu).
    pub fn is_echo(&self, heard: f32) -> bool {
        !self.converged && self.excess(heard).is_some_and(|excess| excess < 0.0)
    }

    /// Couplage appris (dB, marge comprise) : essais.
    #[cfg(test)]
    pub fn coupling_db(&self) -> f32 {
        decibels(self.coupling)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::voice_mode::audio::noise::Noise;

    /// Voix de synthèse : bruit coloré en syllabes de ~180 ms, 1,6 s de
    /// parole puis 0,4 s de pause, ~-18 dBFS pendant la parole.
    pub fn voice_like(seconds: f32, seed: u64) -> Vec<f32> {
        let mut noise = Noise::new(seed);
        let count = (seconds * RATE as f32) as usize;
        let (mut low, mut slow) = (0.0f32, 0.0f32);
        (0..count)
            .map(|index| {
                let t = index as f32 / RATE as f32;
                // Passe-bas (spectre de la parole), sans le très grave.
                low += 0.4 * (noise.next() - low);
                slow += 0.02 * (low - slow);
                let syllable = (std::f32::consts::PI * t / 0.18 + seed as f32).sin().abs();
                let phrase = if (t + seed as f32 * 0.37) % 2.0 < 1.6 {
                    1.0
                } else {
                    0.0
                };
                0.6 * (low - slow) * syllable * phrase
            })
            .collect()
    }

    /// Écho : référence retardée de `delay_ms`, atténuée de `gain`, étalée
    /// par une réponse de salle (~20 ms), plus un bruit de fond à -60 dBFS.
    pub fn echo_of(render: &[f32], delay_ms: usize, gain: f32) -> Vec<f32> {
        let delay = delay_ms * RATE as usize / 1000;
        let room: Vec<f32> = (0..320)
            .map(|tap| match tap {
                0 => 1.0,
                _ if tap % 3 == 0 => 0.2 * (-(tap as f32) / 80.0).exp(),
                _ => -0.1 * (-(tap as f32) / 80.0).exp(),
            })
            .collect();
        let mut noise = Noise::new(99);
        (0..render.len())
            .map(|index| {
                let mut sum = 0.0;
                for (tap, weight) in room.iter().enumerate() {
                    if let Some(source) = index.checked_sub(delay + tap) {
                        sum += weight * render[source];
                    }
                }
                // Uniforme d'amplitude 1,73 × 10⁻³ : RMS 10⁻³.
                gain * sum + 0.001_73 * noise.next()
            })
            .collect()
    }

    fn power(samples: &[f32]) -> f64 {
        samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len().max(1) as f64
    }

    fn db(ratio: f64) -> f64 {
        10.0 * ratio.max(1e-20).log10()
    }

    /// Passe `render` et `capture` trame par trame ; rend la sortie.
    pub fn cancel(render: &[f32], capture: &[f32], feed_render: bool) -> Vec<f32> {
        let mut canceller = Canceller::new();
        let mut out = Vec::with_capacity(capture.len());
        let silence = [0.0; AEC_FRAME];
        for (render, capture) in render
            .as_chunks::<AEC_FRAME>()
            .0
            .iter()
            .zip(capture.as_chunks::<AEC_FRAME>().0)
        {
            canceller.render(if feed_render { render } else { &silence });
            let mut cleaned = [0.0; AEC_FRAME];
            canceller.capture(capture, &mut cleaned);
            out.extend_from_slice(&cleaned);
        }
        out
    }

    /// ERLE (dB) de l'écho seul, après `from` secondes.
    fn erle(capture: &[f32], out: &[f32], from: f32) -> f64 {
        let start = (from * RATE as f32) as usize;
        db(power(&capture[start..]) / power(&out[start..]))
    }

    /// Écho seul, retards de 40 à 250 ms : au moins 15 dB retirés après
    /// 2 s.
    #[test]
    fn echo_is_cancelled_for_usual_delays() {
        let render = voice_like(6.0, 1);
        for delay in [40, 80, 120, 250] {
            let capture = echo_of(&render, delay, 0.5);
            let out = cancel(&render, &capture, true);
            let erle = erle(&capture, &out, 2.0);
            eprintln!("echo delay {delay} ms: ERLE {erle:.1} dB after 2 s");
            assert!(erle >= 15.0, "{delay} ms: ERLE {erle:.1} dB");
        }
    }

    /// Sans la référence, rien n'est retiré (essai de l'essai).
    #[test]
    fn without_the_render_nothing_is_cancelled() {
        let render = voice_like(4.0, 1);
        let capture = echo_of(&render, 80, 0.5);
        let out = cancel(&render, &capture, false);
        let erle = erle(&capture, &out, 2.0);
        assert!(erle < 3.0, "ERLE {erle:.1} dB");
    }

    /// Double parole : la voix proche, ajoutée à l'écho, reste (atténuée de
    /// 6 dB au plus), et l'écho reste retiré autour.
    #[test]
    fn near_speech_survives_double_talk() {
        let render = voice_like(8.0, 1);
        let echo = echo_of(&render, 80, 0.5);
        let near = voice_like(8.0, 7);
        let (from, to) = (4 * RATE as usize, 6 * RATE as usize);
        let capture: Vec<f32> = echo
            .iter()
            .zip(&near)
            .enumerate()
            .map(|(index, (echo, near))| {
                echo + if (from..to).contains(&index) {
                    *near
                } else {
                    0.0
                }
            })
            .collect();
        let out = cancel(&render, &capture, true);
        let kept = db(power(&out[from..to]) / power(&near[from..to]));
        eprintln!("double talk: near speech kept at {kept:.1} dB");
        assert!(kept >= -6.0, "near speech at {kept:.1} dB");
        let dot = |a: &[f32], b: &[f32]| -> f64 {
            a.iter()
                .zip(b)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum()
        };
        // Faite de la voix proche, pas de l'écho (corrélation au meilleur
        // décalage : AEC3 retarde la sortie de 8 ms). Le suppresseur d'AEC3
        // module la voix proche par bandes : corrélation de ~0,66 mesurée.
        let best = |signal: &[f32]| {
            let reference = &signal[from..to - 1_000];
            (0..1_000)
                .map(|lag| {
                    let out = &out[from + lag..to - 1_000 + lag];
                    dot(out, reference) / (dot(out, out) * dot(reference, reference)).sqrt()
                })
                .fold(f64::MIN, f64::max)
        };
        let (with_near, with_echo) = (best(&near), best(&echo));
        eprintln!(
            "double talk: correlation {with_near:.2} with the near speech, {with_echo:.2} with the echo"
        );
        assert!(with_near > 0.5, "{with_near:.2}");
        assert!(with_echo < 0.2, "{with_echo:.2}");
        let after = erle(&echo[7 * RATE as usize..], &out[7 * RATE as usize..], 0.0);
        assert!(after >= 10.0, "ERLE after double talk {after:.1} dB");
    }

    /// Rien ne joue : la voix passe (filtre passe-haut seulement).
    #[test]
    fn speech_passes_when_nothing_plays() {
        let near = voice_like(3.0, 3);
        let out = cancel(&vec![0.0; near.len()], &near, true);
        let start = RATE as usize;
        let kept = db(power(&out[start..]) / power(&near[start..]));
        assert!(kept.abs() < 1.5, "{kept:.1} dB");
    }

    /// Énergie par trame : son joué en blocs de 70 ms (fort, puis presque
    /// rien).
    fn blocks(frame: usize) -> f32 {
        if (frame / 7).is_multiple_of(2) {
            0.01
        } else {
            1e-6
        }
    }

    /// Couplage au bon retard (80 ms, -10 dB : 20e centile plus 6 dB) après
    /// 0,5 s de son joué fort ; 0 dB avant.
    #[test]
    fn the_guard_learns_the_coupling_at_the_right_delay() {
        let mut guard = EchoGuard::default();
        assert_eq!(guard.excess(0.1), None);
        let feed = |guard: &mut EchoGuard, frames: std::ops::Range<usize>| {
            for frame in frames {
                let echo = frame.checked_sub(8).map_or(0.0, |past| 0.1 * blocks(past));
                guard.add(blocks(frame), echo + 1e-8, 0.0);
            }
        };
        feed(&mut guard, 0..30);
        assert_eq!(guard.coupling_db(), 0.0);
        feed(&mut guard, 30..400);
        let coupling = guard.coupling_db();
        assert!((coupling + 4.0).abs() < 0.5, "{coupling}");
        // Écho seul (-30 dBFS) : masqué ; voix plus forte : gardée.
        assert!(guard.is_echo(0.001));
        assert!(!guard.is_echo(0.01));
        assert!(guard.bypasses());
        // AEC3 convergé (ERLE ≥ 10 dB) : sa sortie suffit, et le reste tant
        // que l'ERLE ne tombe pas sous 6 dB.
        guard.add(0.01, 0.001, 12.0);
        assert!(!guard.is_echo(0.001));
        assert!(!guard.bypasses());
        guard.add(0.01, 0.001, 7.0);
        assert!(!guard.bypasses());
        guard.add(0.01, 0.001, 5.0);
        assert!(guard.bypasses());
        // Plus rien ne joue depuis 500 ms : rien n'est masqué.
        for _ in 0..50 {
            guard.add(0.0, 1e-8, 0.0);
        }
        assert_eq!(guard.excess(0.001), None);
        assert!(!guard.bypasses());
    }

    /// Casque : couplage infime (bruit du micro), la voix n'est pas
    /// masquée, même 2 s de voix ininterrompue sur le son joué. Puis des
    /// haut-parleurs : l'écho qui apparaît est appris en moins de 3 s.
    #[test]
    fn near_speech_is_not_learned_but_a_new_echo_is() {
        let mut guard = EchoGuard::default();
        for _ in 0..120 {
            guard.add(0.01, 1e-7, 0.0);
        }
        assert!(guard.coupling_db() < -40.0, "{}", guard.coupling_db());
        // Voix proche à -25 dBFS pendant 2 s.
        for frame in 0..200 {
            guard.add(0.01, 0.003, 0.0);
            assert!(!guard.is_echo(0.003), "frame {frame}");
        }
        // Haut-parleurs : écho à -30 dBFS (couplage -10 dB).
        for _ in 0..50 {
            guard.add(0.01, 1e-7, 0.0);
        }
        for _ in 0..280 {
            guard.add(0.01, 0.001, 0.0);
        }
        let coupling = guard.coupling_db();
        assert!((coupling + 4.0).abs() < 0.5, "{coupling}");
        assert!(guard.is_echo(0.001));
    }

    /// Voix jouée doucement (-40 dBFS) sur un micro bruyant (-50 dBFS) :
    /// l'écho attendu est le bruit plus 6 dB, une voix normale passe.
    #[test]
    fn a_noisy_microphone_only_raises_the_threshold() {
        let mut guard = EchoGuard::default();
        for _ in 0..100 {
            guard.add(1e-4, 1e-5, 0.0);
        }
        assert!(!guard.is_echo(1e-3));
        assert!(guard.is_echo(2e-5));
    }

    /// Trame manquante : une trame de silence entière, le son en attente
    /// reste en file (pas de trou au milieu du signal).
    #[test]
    fn a_missing_frame_is_whole_silence() {
        let mut queue = RenderQueue::default();
        let mut frame = [9.0; AEC_FRAME];
        queue.pop(&mut frame);
        assert!(frame.iter().all(|s| *s == 0.0));
        let ramp: Vec<f32> = (0..250).map(|i| i as f32).collect();
        queue.push(&ramp);
        queue.pop(&mut frame);
        assert_eq!((frame[0], frame[159]), (0.0, 159.0));
        queue.pop(&mut frame);
        assert!(frame.iter().all(|s| *s == 0.0));
        queue.push(&(250..330).map(|i| i as f32).collect::<Vec<_>>());
        queue.pop(&mut frame);
        assert_eq!((frame[0], frame[159]), (160.0, 319.0));
    }

    /// Simule une sortie qui rend `period` échantillons à 48 kHz par rappel
    /// (deux périodes d'un coup au départ, comme cpal), à `speed` fois le
    /// rythme du micro, pendant `seconds`. Rend (trames de silence après la
    /// première seconde, plus grand saut entre deux trames, plus haut
    /// niveau).
    fn simulate(period: usize, speed: f64, seconds: usize) -> (usize, f32, usize) {
        let mut queue = RenderQueue::default();
        let mut frame = [0.0; AEC_FRAME];
        let per_burst = period as f64 / 3.0;
        let interval = 1000.0 * period as f64 / 48_000.0 / speed;
        let (mut next_burst, mut owed, mut pushed) = (0.0f64, 0.0f64, 0usize);
        let (mut silent, mut jump, mut highest) = (0, 0.0f32, 0);
        let mut last = None;
        let mut first = true;
        for millisecond in 0..seconds * 1000 {
            while next_burst <= millisecond as f64 {
                let bursts = if first { 2 } else { 1 };
                first = false;
                next_burst += interval;
                owed += per_burst * bursts as f64;
                let count = owed as usize - pushed;
                // Une rampe : le saut entre deux trames se mesure.
                let ramp: Vec<f32> = (pushed..pushed + count).map(|i| i as f32).collect();
                queue.push(&ramp);
                pushed += count;
            }
            if millisecond % 10 == 0 {
                queue.pop(&mut frame);
                highest = highest.max(queue.len());
                if frame.iter().all(|s| *s == 0.0) {
                    if millisecond > 1_000 {
                        silent += 1;
                    }
                } else {
                    if let Some(previous) = last {
                        jump = jump.max(frame[0] - previous - 1.0);
                    }
                    last = Some(frame[AEC_FRAME - 1]);
                }
            }
        }
        (silent, jump, highest)
    }

    /// Rafales de 1 024 ou 2 048 trames à 48 kHz : ni silence inséré ni
    /// rognage passé la première seconde, donc pas de saut de retard pour
    /// AEC3.
    #[test]
    fn output_bursts_cause_no_delay_jump() {
        for period in [1_024, 2_048] {
            let (silent, jump, highest) = simulate(period, 1.0, 60);
            assert_eq!(silent, 0, "{period}");
            assert_eq!(jump, 0.0, "{period}");
            assert!(highest <= 3 * period / 3 + MAX_DRIFT, "{period}: {highest}");
        }
    }

    /// Sortie 0,125 % plus rapide : l'excédent est retiré par pas de 2
    /// échantillons au plus, la file reste bornée. Plus lente : des trames
    /// de silence entières de temps en temps.
    #[test]
    fn a_drifting_clock_is_followed_in_small_steps() {
        let (silent, jump, highest) = simulate(1_024, 1.001_25, 120);
        assert_eq!(silent, 0);
        assert!(jump <= DRIFT_STEP as f32, "{jump}");
        assert!(highest <= 683 + MAX_DRIFT + 200, "{highest}");
        let (silent, jump, _) = simulate(1_024, 0.998_75, 120);
        assert!(silent > 0);
        assert_eq!(jump, 0.0);
    }

    /// Fil de l'écoute en retard : 1 s de référence d'un coup, gardée et
    /// rendue trame par trame (pas de rognage).
    #[test]
    fn a_late_thread_keeps_its_reference() {
        let mut queue = RenderQueue::default();
        let mut frame = [0.0; AEC_FRAME];
        let ramp: Vec<f32> = (0..RATE as usize).map(|i| i as f32).collect();
        queue.push(&ramp);
        for index in 0..100 {
            queue.pop(&mut frame);
            assert_eq!(frame[0], (index * AEC_FRAME) as f32);
        }
    }

    /// Fin de lecture (plus rien reçu) : la suivante repart de zéro.
    #[test]
    fn a_new_reading_starts_afresh() {
        let mut queue = RenderQueue::default();
        let mut frame = [0.0; AEC_FRAME];
        queue.push(&[0.5; 250]);
        for _ in 0..40 {
            queue.pop(&mut frame);
        }
        assert_eq!(queue.len(), 0);
        queue.push(&[0.25; 160]);
        queue.pop(&mut frame);
        assert!(frame.iter().all(|s| *s == 0.25));
    }
}
