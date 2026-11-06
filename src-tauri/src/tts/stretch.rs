//! Débit de la voix ralenti sans changer sa hauteur (propre à ChatonLM :
//! Pocket TTS n'a pas de réglage de vitesse). Algorithme WSOLA (Verhelst et
//! Roelands, 1993) : le son est découpé en fenêtres de Hann de 32 ms qui se
//! recouvrent à moitié ; elles sont reposées plus espacées qu'elles n'ont
//! été prises, chacune décalée d'au plus 10 ms pour suivre au mieux la
//! précédente (corrélation sur la partie recouverte), ce qui garde les
//! périodes de la voix intactes.
//!
//! En flux : le son arrive par morceaux (une trame de Mimi, 80 ms), sort au
//! fur et à mesure (~40 ms de retard), et [`Stretcher::finish`] rend la fin ;
//! la durée totale est exactement celle de l'entrée divisée par la vitesse.
//! Le résultat ne dépend pas du découpage en morceaux.

use super::VoiceLanguage;

/// Vitesse de lecture d'une voix (1 : telle que synthétisée). La voix
/// française de Pocket TTS parle un peu vite : 10 % plus lente.
pub fn speed_of(language: VoiceLanguage) -> f32 {
    match language {
        VoiceLanguage::Fr => 0.9,
        VoiceLanguage::En | VoiceLanguage::Es => 1.0,
    }
}

/// Pas de sortie (moitié d'une fenêtre) : 16 ms à 24 kHz.
const HOP: usize = 384;
/// Fenêtre : 32 ms.
const FRAME: usize = 2 * HOP;
/// Décalage cherché autour de la place nominale : ±10 ms (période d'une
/// voix grave, 100 Hz).
const TOLERANCE: usize = 240;

/// Ralentisseur d'un passage (une phrase) ; un neuf par passage.
pub struct Stretcher {
    /// Pas d'entrée entre deux fenêtres (`HOP × vitesse`).
    hop_in: f64,
    speed: f64,
    window: Vec<f32>,
    /// Entrée gardée ; `input[0]` est l'échantillon `base`.
    input: Vec<f32>,
    base: usize,
    received: usize,
    /// Fenêtres déjà posées.
    frames: usize,
    /// Place (dans l'entrée) de la dernière fenêtre posée.
    previous: Option<usize>,
    /// Somme des fenêtres en cours de recouvrement (`FRAME` échantillons).
    pending: Vec<f32>,
    emitted: usize,
}

impl Stretcher {
    /// `speed` < 1 ralentit (0,9 : 10 % plus lent).
    pub fn new(speed: f32) -> Self {
        let window = (0..FRAME)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FRAME as f32).cos())
            .collect();
        Self {
            hop_in: HOP as f64 * f64::from(speed),
            speed: f64::from(speed),
            window,
            input: Vec::new(),
            base: 0,
            received: 0,
            frames: 0,
            previous: None,
            pending: vec![0.0; FRAME],
            emitted: 0,
        }
    }

    /// Ajoute `samples` ; ce qui est prêt est ajouté à `out`.
    pub fn push(&mut self, samples: &[f32], out: &mut Vec<f32>) {
        self.input.extend_from_slice(samples);
        self.received += samples.len();
        self.run(false, out);
    }

    /// Fin du passage : le reste est ajouté à `out`, pour une durée totale
    /// de `reçu / vitesse`.
    pub fn finish(mut self, out: &mut Vec<f32>) {
        let start = out.len();
        self.run(true, out);
        out.extend_from_slice(&self.pending[..HOP]);
        // Tout ce qui est sorti du ralentisseur, cette fin comprise.
        let produced = self.emitted + HOP;
        let target = (self.received as f64 / self.speed).round() as usize;
        if produced > target {
            let extra = (produced - target).min(out.len() - start);
            out.truncate(out.len() - extra);
        } else {
            out.resize(out.len() + (target - produced), 0.0);
        }
    }

    /// Échantillon `index` de l'entrée (0 au-delà de la fin).
    fn at(&self, index: usize) -> f32 {
        index
            .checked_sub(self.base)
            .and_then(|i| self.input.get(i))
            .copied()
            .unwrap_or(0.0)
    }

    /// Place de la fenêtre suivante, d'après la précédente.
    fn best(&self, nominal: usize, natural: usize) -> usize {
        let low = nominal.saturating_sub(TOLERANCE);
        let mut best = (nominal, f32::MIN);
        for candidate in low..=nominal + TOLERANCE {
            let (mut dot, mut energy) = (0.0f32, 1e-9f32);
            for i in 0..HOP {
                let x = self.at(candidate + i);
                dot += x * self.at(natural + i);
                energy += x * x;
            }
            let score = dot / energy.sqrt();
            if score > best.1 {
                best = (candidate, score);
            }
        }
        best.0
    }

    fn run(&mut self, flushing: bool, out: &mut Vec<f32>) {
        loop {
            let nominal = (self.frames as f64 * self.hop_in).round() as usize;
            let position = match self.previous {
                None => {
                    if !flushing && self.received < FRAME {
                        return;
                    }
                    if self.received == 0 {
                        return;
                    }
                    0
                }
                Some(previous) => {
                    if nominal >= self.received {
                        return;
                    }
                    let natural = previous + HOP;
                    let needed = (nominal + TOLERANCE).max(natural) + FRAME;
                    if !flushing && needed > self.received {
                        return;
                    }
                    self.best(nominal, natural)
                }
            };
            for i in 0..FRAME {
                // Première fenêtre sans montée : le passage ne commence pas
                // par un fondu.
                let weight = if self.previous.is_none() && i < HOP {
                    1.0
                } else {
                    self.window[i]
                };
                self.pending[i] += weight * self.at(position + i);
            }
            out.extend_from_slice(&self.pending[..HOP]);
            self.emitted += HOP;
            self.pending.copy_within(HOP.., 0);
            self.pending[HOP..].fill(0.0);
            self.previous = Some(position);
            self.frames += 1;
            // L'entrée qui ne servira plus est oubliée.
            let next = (self.frames as f64 * self.hop_in).round() as usize;
            let keep = next.saturating_sub(TOLERANCE).min(position + HOP);
            if keep > self.base {
                let drop = (keep - self.base).min(self.input.len());
                self.input.drain(..drop);
                self.base += drop;
            }
        }
    }
}

/// Fait passer le son d'une synthèse par un ralentisseur quand la voix de
/// `language` en a un (sinon, tel quel) : `speak` reçoit le récepteur du son
/// ralenti. La fin n'est rendue que si la synthèse a fini sans arrêt.
pub fn speak_at_speed<E>(
    language: VoiceLanguage,
    stopped: impl Fn() -> bool,
    sink: &mut dyn FnMut(&[f32]),
    speak: impl FnOnce(&mut dyn FnMut(&[f32])) -> Result<(), E>,
) -> Result<(), E> {
    let speed = speed_of(language);
    if speed >= 1.0 {
        return speak(sink);
    }
    let mut stretcher = Stretcher::new(speed);
    let mut out = Vec::new();
    let result = speak(&mut |samples: &[f32]| {
        stretcher.push(samples, &mut out);
        if !out.is_empty() {
            sink(&out);
            out.clear();
        }
    });
    if result.is_ok() && !stopped() {
        stretcher.finish(&mut out);
        if !out.is_empty() {
            sink(&out);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 24_000.0;

    fn sine(frequency: f32, seconds: f32) -> Vec<f32> {
        (0..(RATE * seconds) as usize)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * frequency * i as f32 / RATE).sin())
            .collect()
    }

    fn stretch(input: &[f32], speed: f32, chunk: usize) -> Vec<f32> {
        let mut stretcher = Stretcher::new(speed);
        let mut out = Vec::new();
        for part in input.chunks(chunk) {
            stretcher.push(part, &mut out);
        }
        stretcher.finish(&mut out);
        out
    }

    /// Fréquence estimée par les passages à zéro montants.
    fn frequency(samples: &[f32]) -> f32 {
        let rising = samples
            .windows(2)
            .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
            .count();
        rising as f32 * RATE / samples.len() as f32
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn the_french_voice_is_slowed_and_the_others_are_not() {
        assert_eq!(speed_of(VoiceLanguage::Fr), 0.9);
        assert_eq!(speed_of(VoiceLanguage::En), 1.0);
        assert_eq!(speed_of(VoiceLanguage::Es), 1.0);
    }

    #[test]
    fn slower_but_at_the_same_pitch_and_loudness() {
        let input = sine(180.0, 2.0);
        let out = stretch(&input, 0.9, 1920);
        // Durée : exactement l'entrée divisée par la vitesse.
        assert_eq!(out.len(), (input.len() as f64 / 0.9).round() as usize);
        // Hauteur et volume gardés (milieu du passage).
        let middle = &out[out.len() / 4..out.len() * 3 / 4];
        let pitch = frequency(middle);
        assert!((pitch - 180.0).abs() < 4.0, "fréquence {pitch}");
        let loudness = rms(middle) / rms(&input);
        assert!((loudness - 1.0).abs() < 0.08, "volume {loudness}");
        // Pas de fondu au début.
        assert!(rms(&out[..HOP]) > 0.3, "début {}", rms(&out[..HOP]));
    }

    #[test]
    fn the_result_does_not_depend_on_the_chunks() {
        let input: Vec<f32> = sine(140.0, 0.7)
            .into_iter()
            .zip(sine(330.0, 0.7))
            .map(|(a, b)| a + 0.3 * b)
            .collect();
        let whole = stretch(&input, 0.9, input.len());
        assert_eq!(stretch(&input, 0.9, 1920), whole);
        assert_eq!(stretch(&input, 0.9, 7), whole);
    }

    #[test]
    fn short_or_empty_passages_keep_their_length() {
        for length in [0, 10, HOP, FRAME + 3, 3000] {
            let input = sine(200.0, 1.0)[..length].to_vec();
            let out = stretch(&input, 0.9, 1920);
            assert_eq!(
                out.len(),
                (length as f64 / 0.9).round() as usize,
                "{length}"
            );
        }
    }

    #[test]
    fn sound_comes_out_while_it_is_synthesized() {
        let mut stretcher = Stretcher::new(0.9);
        let mut out = Vec::new();
        // Une trame de Mimi (80 ms) : déjà du son, sans attendre la fin.
        stretcher.push(&sine(200.0, 0.08), &mut out);
        assert!(!out.is_empty(), "rien après une trame");
        assert!(out.len() <= (1920.0 / 0.9) as usize);
    }

    #[test]
    fn speaking_at_speed_passes_other_voices_through_and_stops_cleanly() {
        let input = sine(200.0, 0.5);
        let run = |language, stopped: bool| {
            let mut heard = Vec::new();
            speak_at_speed::<()>(
                language,
                || stopped,
                &mut |samples| heard.extend_from_slice(samples),
                |sink| {
                    for part in input.chunks(1920) {
                        sink(part);
                    }
                    Ok(())
                },
            )
            .unwrap();
            heard
        };
        assert_eq!(run(VoiceLanguage::En, false), input);
        let french = run(VoiceLanguage::Fr, false);
        assert_eq!(french.len(), (input.len() as f64 / 0.9).round() as usize);
        // Arrêtée : pas de fin rendue.
        assert!(run(VoiceLanguage::Fr, true).len() < french.len());
    }
}
