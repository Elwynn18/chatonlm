//! Rééchantillonnage : interpolation par sinus cardinal fenêtré (Blackman),
//! coupé à 90 % de la plus basse des deux fréquences de Nyquist (pas de
//! repliement quand la fréquence baisse).
//!
//! [`resample`] traite un son entier (dictée, mode vocal, Easy Link : c'est
//! la fonction qu'avait `dictation::audio`, déplacée ici sans changement de
//! calcul). [`Resampler`] rend exactement les mêmes échantillons, mais par
//! blocs, au fil du décodage : la mémoire reste bornée pour un fichier d'une
//! heure ou plus, et chaque bloc est réparti sur plusieurs fils. Chaque
//! échantillon de sortie ne dépend que des entrées à `width` près (le noyau
//! est local) : un bloc garde cette marge de part et d'autre, et le calcul de
//! chaque échantillon, dans le même ordre, est celui de [`resample`] (égalité
//! au bit près, vérifiée par les essais).

use std::ops::Range;
use std::thread;

/// Demi-largeur du filtre, en passages par zéro du sinus cardinal.
const ZEROS: f64 = 16.0;

/// Pas de la table du noyau : 512 points par échantillon d'entrée.
const TABLE_STEP: f64 = 512.0;

/// En dessous, un bloc n'est pas réparti sur plusieurs fils (le lancement
/// d'un fil coûte plus que le calcul).
const MIN_PER_THREAD: usize = 8_192;

/// Rééchantillonne `samples` de `from` Hz vers `to` Hz. Même fréquence, une
/// fréquence nulle ou un son vide : rendu tel quel.
pub fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 || to == 0 || samples.is_empty() {
        return samples.to_vec();
    }
    let kernel = Kernel::new(from, to);
    let length = kernel.length(samples.len());
    let last_input = samples.len() - 1;
    (0..length)
        .map(|index| kernel.sample(samples, 0, index, last_input))
        .collect()
}

/// Noyau du rééchantillonnage d'une fréquence vers une autre.
struct Kernel {
    /// Pas, en échantillons d'entrée, entre deux échantillons de sortie.
    step: f64,
    /// Demi-largeur du noyau, en échantillons d'entrée.
    width: f64,
    table: Vec<f64>,
}

impl Kernel {
    fn new(from: u32, to: u32) -> Self {
        let step = f64::from(from) / f64::from(to);
        // Fréquence de coupure, en fraction de la fréquence de Nyquist d'entrée.
        let cutoff = (1.0 / step).min(1.0) * 0.9;
        let width = ZEROS / cutoff;
        Self {
            step,
            width,
            table: kernel_table(cutoff, width),
        }
    }

    /// Nombre d'échantillons de sortie pour `inputs` échantillons d'entrée.
    fn length(&self, inputs: usize) -> usize {
        (inputs as f64 / self.step).floor() as usize
    }

    /// Première entrée lue par la sortie `index`.
    fn first_input(&self, index: usize) -> usize {
        (index as f64 * self.step - self.width).ceil().max(0.0) as usize
    }

    /// Dernière entrée lue par la sortie `index`, avant la borne du son.
    fn last_input(&self, index: usize) -> usize {
        (index as f64 * self.step + self.width).floor() as usize
    }

    /// Sortie `index`, calculée sur `input` qui commence à l'entrée `base`
    /// (jamais après [`Kernel::first_input`]) ; `last_input` : dernière entrée
    /// du son. Normalisée : gain 1 pour un signal constant, bords compris.
    fn sample(&self, input: &[f32], base: usize, index: usize, last_input: usize) -> f32 {
        let center = index as f64 * self.step;
        let first = (center - self.width).ceil().max(0.0) as usize;
        let last = ((center + self.width).floor() as usize).min(last_input);
        let (mut sum, mut weights) = (0.0f64, 0.0f64);
        for (offset, &sample) in input[first - base..=last - base].iter().enumerate() {
            let weight = lookup(&self.table, center - (first + offset) as f64 + self.width);
            sum += weight * f64::from(sample);
            weights += weight;
        }
        if weights.abs() > 1e-9 {
            (sum / weights) as f32
        } else {
            0.0
        }
    }
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

/// Rééchantillonneur en flux : les entrées arrivent par [`Resampler::push`],
/// les sorties prêtes sont ajoutées au tampon donné, le reste par
/// [`Resampler::finish`]. Le résultat, mis bout à bout, est exactement celui
/// de [`resample`] sur le son entier.
pub struct Resampler {
    /// `None` : même fréquence, les entrées passent telles quelles.
    kernel: Option<Kernel>,
    /// Entrées gardées, depuis l'entrée `base`.
    input: Vec<f32>,
    base: usize,
    /// Entrées reçues en tout.
    received: usize,
    /// Prochaine sortie à calculer.
    next: usize,
    threads: usize,
    /// Entrées attendues avant un calcul (une seconde par fil).
    batch: usize,
}

impl Resampler {
    /// De `from` Hz vers `to` Hz, sur `threads` fils au plus (1 : sur le fil
    /// de l'appelant seulement).
    pub fn new(from: u32, to: u32, threads: usize) -> Self {
        let kernel = (from != to && from != 0 && to != 0).then(|| Kernel::new(from, to));
        let threads = threads.max(1);
        Self {
            kernel,
            input: Vec::new(),
            base: 0,
            received: 0,
            next: 0,
            threads,
            batch: threads * from.max(1) as usize,
        }
    }

    /// Ajoute des entrées ; les sorties déjà calculables vont dans `output`.
    pub fn push(&mut self, samples: &[f32], output: &mut Vec<f32>) {
        if self.kernel.is_none() {
            output.extend_from_slice(samples);
            return;
        }
        self.input.extend_from_slice(samples);
        self.received += samples.len();
        if self.received - self.consumed() >= self.batch {
            self.flush(output);
        }
    }

    /// Calcule tout ce qui est prêt maintenant, sans attendre un bloc entier
    /// (fin d'une étape, rythme de la progression).
    pub fn flush(&mut self, output: &mut Vec<f32>) {
        let Some(kernel) = &self.kernel else {
            return;
        };
        if self.received == 0 {
            return;
        }
        // Sorties dont toutes les entrées sont arrivées : la borne de fin du
        // son ne les touche pas, leur calcul est déjà définitif.
        let limit = self.received - 1;
        let estimate = (limit as f64 - kernel.width) / kernel.step;
        let mut end = if estimate < 0.0 {
            0
        } else {
            estimate.floor() as usize + 1
        }
        .max(self.next);
        while end > self.next && kernel.last_input(end - 1) > limit {
            end -= 1;
        }
        while kernel.last_input(end) <= limit {
            end += 1;
        }
        self.compute(self.next..end, limit, output);
    }

    /// Fin du son : calcule les dernières sorties.
    pub fn finish(mut self, output: &mut Vec<f32>) {
        let Some(kernel) = &self.kernel else {
            return;
        };
        if self.received == 0 {
            return;
        }
        let length = kernel.length(self.received);
        let range = self.next..length.max(self.next);
        self.compute(range, self.received - 1, output);
    }

    /// Entrées déjà couvertes par les sorties calculées.
    fn consumed(&self) -> usize {
        match &self.kernel {
            Some(kernel) => kernel.last_input(self.next).min(self.received),
            None => self.received,
        }
    }

    /// Calcule les sorties `range` (réparties sur les fils), puis oublie les
    /// entrées dont les suivantes n'ont plus besoin.
    fn compute(&mut self, range: Range<usize>, last_input: usize, output: &mut Vec<f32>) {
        let Some(kernel) = &self.kernel else {
            return;
        };
        let count = range.len();
        if count > 0 {
            let start = output.len();
            output.resize(start + count, 0.0);
            let slots = &mut output[start..];
            let (input, base) = (&self.input[..], self.base);
            let parts = self.threads.min(count / MIN_PER_THREAD).max(1);
            if parts == 1 {
                for (slot, index) in slots.iter_mut().zip(range.clone()) {
                    *slot = kernel.sample(input, base, index, last_input);
                }
            } else {
                let size = count.div_ceil(parts);
                thread::scope(|scope| {
                    for (part, chunk) in slots.chunks_mut(size).enumerate() {
                        let first = range.start + part * size;
                        scope.spawn(move || {
                            for (offset, slot) in chunk.iter_mut().enumerate() {
                                *slot = kernel.sample(input, base, first + offset, last_input);
                            }
                        });
                    }
                });
            }
        }
        self.next = range.end;
        let keep = kernel.first_input(self.next).min(self.received);
        if keep > self.base {
            self.input.drain(..keep - self.base);
            self.base = keep;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L'ancienne fonction de `dictation::audio`, recopiée telle quelle :
    /// la fonction déplacée et le flux doivent la reproduire au bit près.
    fn original(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
        if from == to || from == 0 || to == 0 || samples.is_empty() {
            return samples.to_vec();
        }
        let step = f64::from(from) / f64::from(to);
        let cutoff = (1.0 / step).min(1.0) * 0.9;
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
            output.push(if weights.abs() > 1e-9 {
                (sum / weights) as f32
            } else {
                0.0
            });
        }
        output
    }

    /// Bruit déterministe (xorshift) : tous les échantillons comptent.
    fn noise(count: usize, seed: u64) -> Vec<f32> {
        let mut state = seed | 1;
        (0..count)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 40) as f32 / (1u64 << 23) as f32 - 1.0
            })
            .collect()
    }

    /// Passe `input` dans un [`Resampler`] par morceaux de tailles variées.
    fn streamed(input: &[f32], from: u32, to: u32, threads: usize, pieces: &[usize]) -> Vec<f32> {
        let mut resampler = Resampler::new(from, to, threads);
        let mut output = Vec::new();
        let mut at = 0;
        let mut piece = 0;
        while at < input.len() {
            let size = pieces[piece % pieces.len()].min(input.len() - at);
            resampler.push(&input[at..at + size], &mut output);
            at += size;
            piece += 1;
        }
        resampler.finish(&mut output);
        output
    }

    #[test]
    fn the_moved_function_is_the_original() {
        for (from, to) in [
            (48_000, 16_000),
            (44_100, 16_000),
            (8_000, 16_000),
            (16_000, 48_000),
        ] {
            let input = noise(from as usize / 3 + 17, u64::from(from));
            assert_eq!(resample(&input, from, to), original(&input, from, to));
        }
    }

    #[test]
    fn streamed_blocks_equal_the_whole_at_every_rate() {
        // Morceaux de toutes tailles, dont un seul échantillon, et des blocs
        // assez grands pour être répartis sur plusieurs fils.
        let pieces = [1, 7, 960, 4_096, 1_024, 33_333, 2];
        for (from, to) in [
            (48_000, 16_000),
            (44_100, 16_000),
            (22_050, 16_000),
            (32_000, 16_000),
            (8_000, 16_000),
            (11_025, 16_000),
            (16_000, 16_000),
        ] {
            let input = noise(from as usize * 5 / 2 + 101, u64::from(from) * 7);
            let whole = original(&input, from, to);
            for threads in [1, 3, 16] {
                let output = streamed(&input, from, to, threads, &pieces);
                assert_eq!(output.len(), whole.len(), "{from} Hz, {threads} fils");
                assert!(
                    output
                        .iter()
                        .zip(&whole)
                        .all(|(a, b)| a.to_bits() == b.to_bits()),
                    "{from} Hz, {threads} fils : écart"
                );
            }
        }
    }

    #[test]
    fn short_and_empty_sounds_are_like_the_original() {
        for length in [0, 1, 2, 5, 40, 120] {
            let input = noise(length, 3 + length as u64);
            assert_eq!(
                streamed(&input, 48_000, 16_000, 4, &[1, 3]),
                original(&input, 48_000, 16_000),
                "{length} échantillons"
            );
        }
    }

    #[test]
    fn memory_stays_bounded_while_streaming() {
        // Une minute à 48 kHz par morceaux de 20 ms : l'entrée gardée ne
        // dépasse jamais un bloc et sa marge.
        let mut resampler = Resampler::new(48_000, 16_000, 2);
        let piece = noise(960, 9);
        let mut output = Vec::new();
        let mut largest = 0;
        for _ in 0..3_000 {
            resampler.push(&piece, &mut output);
            largest = largest.max(resampler.input.len());
        }
        assert!(largest <= 2 * 48_000 + 2_000, "{largest}");
        resampler.finish(&mut output);
        assert_eq!(output.len(), 60 * 16_000);
    }

    #[test]
    fn flush_gives_what_is_ready_without_changing_the_result() {
        let input = noise(48_000, 5);
        let whole = original(&input, 48_000, 16_000);
        let mut resampler = Resampler::new(48_000, 16_000, 8);
        let mut output = Vec::new();
        for piece in input.chunks(4_800) {
            resampler.push(piece, &mut output);
            resampler.flush(&mut output);
            // Tout ce qui est rendu est déjà définitif.
            assert_eq!(output[..], whole[..output.len()]);
        }
        assert!(output.len() > 15_000);
        resampler.finish(&mut output);
        assert_eq!(output, whole);
    }
}
