//! Découpage d'un long son (16 kHz) en morceaux pour les moteurs de la
//! dictée (rapport 105, § 4.2).
//!
//! Pendant le décodage, [`Analyzer`] relève pour chaque trame de 32 ms
//! (512 échantillons, celle de Silero) son énergie et, si la détection de
//! parole est là, sa probabilité de parole. [`plan`] en tire des morceaux
//! d'environ 40 s (45 s d'un seul tenant au plus, comme la dictée), sans
//! chevauchement : chaque coupure se place dans une pause de la détection
//! (p < 0,3 pendant 224 ms au moins) la plus proche de sa place idéale, à
//! 4 s au plus ; à défaut, au milieu de la trame la plus calme de cette
//! fenêtre (comme `dictation::engine::chunks`). Un morceau où la détection
//! ne voit aucune parole (p < 0,5 partout) est marqué muet : il n'est pas
//! transcrit, Whisper y inventerait du texte (« Sous-titres réalisés par la
//! communauté d'Amara.org »).
//!
//! Chaque morceau est envoyé sans ses silences de bord ([`Chunk::voiced`],
//! 0,3 s de marge) : mesuré sur sept minutes de français coupées au milieu de
//! pauses de silence numérique, Parakeet perdait la fin des morceaux qui
//! finissaient par 0,75 s de zéros (1 037 mots au lieu d'environ 1 200,
//! rapport 106-lot2).

use std::ops::Range;

use crate::voice_mode::audio::vad::{FRAME, SpeechDetector};

/// Fréquence du son analysé.
pub const RATE: usize = 16_000;

/// Son transcrit d'un seul morceau jusqu'à cette durée.
pub const MAX_CHUNK: usize = 45 * RATE;

/// Longueur visée d'un morceau.
pub const CHUNK_TARGET: usize = 40 * RATE;

/// Une coupure se cherche à 4 s au plus de sa place idéale.
pub const CUT_SEARCH: usize = 4 * RATE;

/// Pause : probabilité de parole sous ce seuil...
const PAUSE_PROBABILITY: f32 = 0.3;

/// ... pendant au moins ce nombre de trames (224 ms).
const PAUSE_FRAMES: usize = 7;

/// Parole : une trame au moins atteint ce seuil.
const SPEECH_PROBABILITY: f32 = 0.5;

/// Son gardé de part et d'autre de la parole d'un morceau (0,3 s).
const MARGIN: usize = RATE * 3 / 10;

/// Sans détection : trame silencieuse sous ce carré moyen (-60 dBFS).
const QUIET_ENERGY: f32 = 1e-6;

/// Relevé d'un son entier, trame par trame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    /// Échantillons du son.
    pub samples: usize,
    /// Énergie moyenne (carré moyen) de chaque trame ; la dernière,
    /// incomplète, sur ce qu'elle contient.
    pub energies: Vec<f32>,
    /// Probabilité de parole de chaque trame (la dernière complétée de
    /// silence) ; `None` sans détection (ONNX Runtime absent, échec).
    pub probabilities: Option<Vec<f32>>,
}

/// Relève l'énergie et la parole d'un son reçu par blocs quelconques.
pub struct Analyzer {
    detector: Option<Box<dyn SpeechDetector + Send>>,
    frame: Vec<f32>,
    analysis: Analysis,
}

impl Analyzer {
    /// `detector` : remis à zéro ici (un autre son commence).
    pub fn new(mut detector: Option<Box<dyn SpeechDetector + Send>>) -> Self {
        if let Some(detector) = detector.as_mut() {
            detector.reset();
        }
        let probabilities = detector.is_some().then(Vec::new);
        Self {
            detector,
            frame: Vec::with_capacity(FRAME),
            analysis: Analysis {
                probabilities,
                ..Analysis::default()
            },
        }
    }

    pub fn push(&mut self, mut block: &[f32]) {
        self.analysis.samples += block.len();
        while !block.is_empty() {
            let taken = (FRAME - self.frame.len()).min(block.len());
            self.frame.extend_from_slice(&block[..taken]);
            block = &block[taken..];
            if self.frame.len() == FRAME {
                self.close_frame();
            }
        }
    }

    /// Fin du son : la trame incomplète est relevée ; le détecteur est rendu
    /// pour servir à un autre son.
    pub fn finish(mut self) -> (Analysis, Option<Box<dyn SpeechDetector + Send>>) {
        if !self.frame.is_empty() {
            self.close_frame();
        }
        (self.analysis, self.detector)
    }

    fn close_frame(&mut self) {
        let length = self.frame.len();
        let energy = self.frame.iter().map(|s| s * s).sum::<f32>() / length as f32;
        self.analysis.energies.push(energy);
        if let Some(detector) = self.detector.as_mut() {
            self.frame.resize(FRAME, 0.0);
            match detector.probability(&self.frame) {
                Ok(probability) => {
                    if let Some(probabilities) = self.analysis.probabilities.as_mut() {
                        probabilities.push(probability);
                    }
                }
                Err(error) => {
                    // Détection en échec : coupures et morceaux muets au
                    // niveau sonore, pour tout le son.
                    crate::app_log!("chatonlm: transcription: speech detection: {error}");
                    self.detector = None;
                    self.analysis.probabilities = None;
                }
            }
        }
        self.frame.clear();
    }
}

/// Morceau à transcrire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// Échantillons du son (morceaux bout à bout).
    pub range: Range<usize>,
    /// Partie envoyée au moteur : `range` sans ses silences de bord (marge
    /// de 0,3 s) ; `range` entier si rien n'y sonne.
    pub voiced: Range<usize>,
    /// Parole vue par la détection ; `None` sans détection (à juger sur le
    /// son, par `dictation::audio::has_speech`).
    pub speech: Option<bool>,
}

/// Morceaux de tout le son, bout à bout, du début à la fin.
pub fn plan(analysis: &Analysis) -> Vec<Chunk> {
    let total = analysis.samples;
    let probabilities = analysis.probabilities.as_deref();
    let mut ranges = Vec::new();
    let mut start = 0;
    while total - start > MAX_CHUNK {
        let ideal = start + CHUNK_TARGET;
        let low = ideal - CUT_SEARCH;
        let high = (ideal + CUT_SEARCH).min(total);
        let cut = probabilities
            .and_then(|probabilities| pause_cut(probabilities, low, high, ideal))
            .unwrap_or_else(|| quietest(&analysis.energies, low, high, ideal));
        ranges.push(start..cut);
        start = cut;
    }
    if total > start || ranges.is_empty() {
        ranges.push(start..total);
    }
    ranges
        .into_iter()
        .map(|range| Chunk {
            speech: probabilities.map(|probabilities| has_speech(probabilities, &range)),
            voiced: voiced(analysis, &range),
            range,
        })
        .collect()
}

/// Coupure dans une pause qui touche `low..high`, la plus proche d'`ideal` :
/// au milieu de la pause, ramené dans la fenêtre.
fn pause_cut(probabilities: &[f32], low: usize, high: usize, ideal: usize) -> Option<usize> {
    let quiet = |frame: usize| probabilities[frame] < PAUSE_PROBABILITY;
    let first = low / FRAME;
    let last = high.div_ceil(FRAME).min(probabilities.len());
    let mut best: Option<usize> = None;
    let mut frame = first;
    while frame < last {
        if !quiet(frame) {
            frame += 1;
            continue;
        }
        // Toute la pause, même commencée avant la fenêtre.
        let mut begin = frame;
        while begin > 0 && quiet(begin - 1) {
            begin -= 1;
        }
        let mut end = frame;
        while end < probabilities.len() && quiet(end) {
            end += 1;
        }
        if end - begin >= PAUSE_FRAMES {
            let middle = (begin + end) * FRAME / 2;
            let cut = middle.clamp(low.max(begin * FRAME), high.min(end * FRAME));
            if best.is_none_or(|best| cut.abs_diff(ideal) < best.abs_diff(ideal)) {
                best = Some(cut);
            }
        }
        frame = end;
    }
    best
}

/// Milieu de la trame la plus calme de `low..high` (la plus proche
/// d'`ideal` à égalité) ; `ideal` si aucune trame n'y est relevée.
fn quietest(energies: &[f32], low: usize, high: usize, ideal: usize) -> usize {
    let first = low.div_ceil(FRAME);
    let mut best: Option<(f32, usize)> = None;
    let mut frame = first;
    while (frame + 1) * FRAME <= high && frame < energies.len() {
        let middle = frame * FRAME + FRAME / 2;
        let better = best.is_none_or(|(energy, cut)| {
            energies[frame] < energy
                || (energies[frame] == energy && middle.abs_diff(ideal) < cut.abs_diff(ideal))
        });
        if better {
            best = Some((energies[frame], middle));
        }
        frame += 1;
    }
    best.map_or(ideal, |(_, cut)| cut)
}

/// `range` sans ses silences de bord : de la première à la dernière trame
/// qui sonne (détection : p ≥ 0,3 ; sinon au-dessus de -60 dBFS), marge
/// comprise.
fn voiced(analysis: &Analysis, range: &Range<usize>) -> Range<usize> {
    let first = range.start / FRAME;
    let last = range.end.div_ceil(FRAME).min(analysis.energies.len());
    let sounds = |frame: usize| match analysis.probabilities.as_deref() {
        Some(probabilities) => probabilities
            .get(frame)
            .is_some_and(|p| *p >= PAUSE_PROBABILITY),
        None => analysis.energies[frame] > QUIET_ENERGY,
    };
    let Some(begin) = (first..last).find(|frame| sounds(*frame)) else {
        return range.clone();
    };
    let end = (first..last)
        .rev()
        .find(|frame| sounds(*frame))
        .unwrap_or(begin);
    let start = (begin * FRAME).saturating_sub(MARGIN).max(range.start);
    let stop = ((end + 1) * FRAME + MARGIN).min(range.end);
    start..stop.max(start)
}

/// Une trame touchée par `range` atteint le seuil de parole.
fn has_speech(probabilities: &[f32], range: &Range<usize>) -> bool {
    let first = range.start / FRAME;
    let last = range.end.div_ceil(FRAME).min(probabilities.len());
    probabilities
        .get(first..last)
        .is_some_and(|frames| frames.iter().any(|p| *p >= SPEECH_PROBABILITY))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Détecteur qui juge au niveau : parole si une valeur dépasse 0,01 ;
    /// compte ses remises à zéro.
    pub struct Loudness {
        pub resets: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        pub fail_after: Option<usize>,
        seen: usize,
    }

    impl Loudness {
        pub fn new() -> Self {
            Self {
                resets: Default::default(),
                fail_after: None,
                seen: 0,
            }
        }
    }

    impl SpeechDetector for Loudness {
        fn probability(&mut self, frame: &[f32]) -> Result<f32, String> {
            assert_eq!(frame.len(), FRAME);
            self.seen += 1;
            if self.fail_after.is_some_and(|limit| self.seen > limit) {
                return Err("broken".into());
            }
            Ok(if frame.iter().any(|s| s.abs() > 0.01) {
                0.9
            } else {
                0.05
            })
        }

        fn reset(&mut self) {
            self.resets
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Bruit de « parole » (amplitude 0,2) de `seconds`.
    fn speech(seconds: f32) -> Vec<f32> {
        let mut state = 7u32;
        (0..(seconds * RATE as f32) as usize)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                ((state >> 16) as f32 / 32_768.0 - 1.0) * 0.2
            })
            .collect()
    }

    fn silence(seconds: f32) -> Vec<f32> {
        vec![0.0; (seconds * RATE as f32) as usize]
    }

    fn analyse(samples: &[f32], detector: bool) -> Analysis {
        let detector: Option<Box<dyn SpeechDetector + Send>> =
            detector.then(|| Box::new(Loudness::new()) as _);
        let mut analyzer = Analyzer::new(detector);
        // Blocs de tailles quelconques.
        for block in samples.chunks(1_237) {
            analyzer.push(block);
        }
        analyzer.finish().0
    }

    fn concat(parts: &[Vec<f32>]) -> Vec<f32> {
        parts.concat()
    }

    #[test]
    fn the_analysis_covers_every_frame_whatever_the_blocks() {
        let samples = concat(&[speech(1.0), silence(0.5), speech(0.01)]);
        let analysis = analyse(&samples, true);
        assert_eq!(analysis.samples, samples.len());
        let frames = samples.len().div_ceil(FRAME);
        assert_eq!(analysis.energies.len(), frames);
        let probabilities = analysis.probabilities.unwrap();
        assert_eq!(probabilities.len(), frames);
        assert_eq!(probabilities[0], 0.9);
        assert_eq!(probabilities[40], 0.05);
        // Trame finale incomplète : complétée de silence pour la détection.
        assert_eq!(probabilities[frames - 1], 0.9);
        let without = analyse(&samples, false);
        assert_eq!(without.probabilities, None);
        assert_eq!(without.energies, analysis.energies);
    }

    /// Le détecteur est remis à zéro pour chaque son ; en échec, le son entier
    /// passe au niveau sonore (aucune probabilité gardée).
    #[test]
    fn the_detector_is_reset_and_a_failure_falls_back_to_levels() {
        let detector = Loudness::new();
        let resets = detector.resets.clone();
        let analyzer = Analyzer::new(Some(Box::new(detector)));
        let (_, detector) = analyzer.finish();
        let mut analyzer = Analyzer::new(detector);
        analyzer.push(&speech(0.1));
        let (analysis, detector) = analyzer.finish();
        assert!(detector.is_some());
        assert!(analysis.probabilities.is_some());
        assert_eq!(resets.load(std::sync::atomic::Ordering::SeqCst), 2);

        let mut failing = Loudness::new();
        failing.fail_after = Some(3);
        let mut analyzer = Analyzer::new(Some(Box::new(failing)));
        analyzer.push(&speech(1.0));
        let (analysis, detector) = analyzer.finish();
        assert!(detector.is_none());
        assert_eq!(analysis.probabilities, None);
        assert_eq!(analysis.energies.len(), RATE.div_ceil(FRAME));
    }

    #[test]
    fn short_sounds_make_a_single_chunk() {
        for samples in [Vec::new(), speech(3.0), speech(45.0)] {
            let parts = plan(&analyse(&samples, true));
            assert_eq!(parts.len(), 1);
            assert_eq!(parts[0].range, 0..samples.len());
        }
        let parts = plan(&analyse(&silence(10.0), true));
        assert_eq!(parts[0].speech, Some(false));
        let parts = plan(&analyse(&silence(10.0), false));
        assert_eq!(parts[0].speech, None);
    }

    /// Parole continue avec des pauses de 0,5 s : chaque coupure tombe dans
    /// une pause, la plus proche des 40 s ; aucun échantillon perdu ni
    /// répété.
    #[test]
    fn cuts_fall_in_the_pauses_nearest_to_forty_seconds() {
        // Pauses à 36,5 s et 42,5 s (idéal : 40 s), puis toutes les 9 s.
        let mut samples = concat(&[speech(36.5), silence(0.5), speech(5.5), silence(0.5)]);
        let mut pauses = vec![36.5..37.0, 42.5..43.0];
        while samples.len() < 200 * RATE {
            let at = samples.len() as f32 / RATE as f32 + 8.5;
            samples.extend(speech(8.5));
            samples.extend(silence(0.5));
            pauses.push(at..at + 0.5);
        }
        let analysis = analyse(&samples, true);
        let parts = plan(&analysis);
        assert!(parts.len() >= 4, "{parts:?}");
        assert_eq!(parts[0].range.start, 0);
        assert_eq!(parts.last().unwrap().range.end, samples.len());
        for pair in parts.windows(2) {
            assert_eq!(pair[0].range.end, pair[1].range.start);
            let cut = pair[0].range.end as f32 / RATE as f32;
            assert!(
                pauses.iter().any(|pause| pause.contains(&cut)),
                "cut at {cut} s outside the pauses"
            );
        }
        // Milieux à 3,25 s et 2,75 s de l'idéal : la plus proche l'emporte.
        let first = parts[0].range.end as f32 / RATE as f32;
        assert!((42.5..43.0).contains(&first), "{first}");
        for part in &parts {
            assert!(part.range.len() <= MAX_CHUNK, "{part:?}");
            assert_eq!(part.speech, Some(true));
        }
    }

    /// Une pause trop courte (100 ms) ne compte pas ; sans pause, la trame la
    /// plus calme de la fenêtre.
    #[test]
    fn without_a_real_pause_the_quietest_frame_wins() {
        let mut samples = concat(&[speech(38.0), silence(0.1), speech(30.0)]);
        // Passage plus doux à 41 s (sans silence).
        for sample in &mut samples[41 * RATE..41 * RATE + 2 * FRAME] {
            *sample *= 0.05;
        }
        let analysis = analyse(&samples, true);
        let parts = plan(&analysis);
        assert_eq!(parts.len(), 2);
        let cut = parts[0].range.end;
        assert!(
            (38 * RATE..38 * RATE + RATE / 10).contains(&cut),
            "{} s",
            cut as f32 / RATE as f32
        );
        // Sans détection : même choix, au niveau sonore.
        let parts = plan(&analyse(&samples, false));
        assert!((38 * RATE..38 * RATE + RATE / 10).contains(&parts[0].range.end));
        assert_eq!(parts[0].speech, None);
    }

    /// Les silences de bord ne sont pas envoyés (marge de 0,3 s) ; un morceau
    /// sans rien qui sonne reste entier.
    #[test]
    fn chunks_are_sent_without_their_silent_edges() {
        let samples = concat(&[silence(2.0), speech(5.0), silence(3.0)]);
        for detector in [true, false] {
            let parts = plan(&analyse(&samples, detector));
            assert_eq!(parts.len(), 1);
            let voiced = &parts[0].voiced;
            // Bords à la trame près.
            assert!(
                voiced.start.abs_diff(2 * RATE - MARGIN) < FRAME,
                "{voiced:?} ({detector})"
            );
            assert!(
                voiced.end.abs_diff(7 * RATE + MARGIN) < FRAME,
                "{voiced:?} ({detector})"
            );
            assert_eq!(parts[0].range, 0..samples.len());
        }
        let quiet = plan(&analyse(&silence(5.0), false));
        assert_eq!(quiet[0].voiced, quiet[0].range);
        // Parole jusqu'aux bords : rien de retiré.
        let full = plan(&analyse(&speech(5.0), true));
        assert_eq!(full[0].voiced, full[0].range);
    }

    /// Une longue plage de musique ou de silence donne des morceaux muets ;
    /// un morceau qui a un peu de parole n'en est pas un.
    #[test]
    fn chunks_without_speech_are_marked_silent() {
        let samples = concat(&[speech(20.0), silence(100.0), speech(0.5), silence(30.0)]);
        let parts = plan(&analyse(&samples, true));
        let flags: Vec<bool> = parts.iter().map(|part| part.speech.unwrap()).collect();
        assert_eq!(flags.first(), Some(&true));
        assert!(flags.contains(&false), "{parts:?}");
        let with_burst = parts
            .iter()
            .find(|part| part.range.contains(&(120 * RATE + RATE / 4)))
            .unwrap();
        assert_eq!(with_burst.speech, Some(true));
    }
}
