//! Détection de parole : Silero VAD v6.2 (MIT, © Silero Team), graphe ONNX
//! 16 kHz embarqué, sur l'ONNX Runtime de la dictée.
//!
//! Modèle : `src/silero_vad/data/silero_vad_16k_op15.onnx` du dépôt
//! snakers4/silero-vad, tag v6.2.3 (commit 5cd7945676eb), copié sous
//! `src-tauri/models/silero_vad.onnx` ; SHA-256
//! 7ed98ddbad84ccac4cd0aeb3099049280713df825c610a8ed34543318f1b2c49. Mêmes
//! poids que `silero_vad.onnx` (opset 16, 8 et 16 kHz), deux fois plus
//! petit ; sorties identiques vérifiées sur `jfk.wav` et `fr0.wav`.
//!
//! Graphe (lu dans le fichier) : entrées `input` f32 [1, N], `state` f32
//! [2, 1, 128], `sr` i64 scalaire ; sorties `output` f32 [1, 1] et `stateN`
//! f32 [2, 1, 128]. Comme le fait `OnnxWrapper` de silero-vad, chaque trame
//! de 512 échantillons est précédée des 64 derniers échantillons de la
//! précédente (N = 576) ; l'état récurrent passe d'une trame à l'autre et
//! repart de zéro à chaque ouverture de l'écoute.

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::TensorRef;

use super::RATE;
use super::endpoint::FRAME;

/// Le graphe (1,3 Mo).
const MODEL: &[u8] = include_bytes!("../../../models/silero_vad.onnx");

/// Échantillons de la trame précédente ajoutés en tête.
const CONTEXT: usize = 64;
/// État récurrent : [2, 1, 128].
const STATE: usize = 2 * 128;

/// Probabilité de parole d'une trame (Silero, ou un faux dans les essais).
pub trait SpeechDetector {
    /// `frame` : `FRAME` échantillons à 16 kHz ; erreur en anglais (détail
    /// technique).
    fn probability(&mut self, frame: &[f32]) -> Result<f32, String>;
}

pub struct Silero {
    session: Session,
    state: Vec<f32>,
    /// Contexte puis trame courante.
    input: Vec<f32>,
}

fn ort_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

impl Silero {
    /// Session neuve (état à zéro), un seul fil. Charge ONNX Runtime au
    /// besoin (`dictation::onnx::ensure`, jamais un second `init_from`).
    pub fn load() -> Result<Self, String> {
        crate::dictation::onnx::ensure().map_err(|problem| problem.detail)?;
        let session = Session::builder()
            .map_err(ort_error)?
            .with_optimization_level(GraphOptimizationLevel::All)
            .map_err(ort_error)?
            .with_intra_threads(1)
            .map_err(ort_error)?
            .with_inter_threads(1)
            .map_err(ort_error)?
            .commit_from_memory(MODEL)
            .map_err(ort_error)?;
        Ok(Self {
            session,
            state: vec![0.0; STATE],
            input: vec![0.0; CONTEXT + FRAME],
        })
    }
}

impl SpeechDetector for Silero {
    fn probability(&mut self, frame: &[f32]) -> Result<f32, String> {
        if frame.len() != FRAME {
            return Err(format!("speech detection: {} samples", frame.len()));
        }
        self.input[CONTEXT..].copy_from_slice(frame);
        let rate = [i64::from(RATE)];
        let input = TensorRef::from_array_view(([1, CONTEXT + FRAME], &self.input[..]))
            .map_err(ort_error)?;
        let state =
            TensorRef::from_array_view(([2, 1, 128], &self.state[..])).map_err(ort_error)?;
        let rate = TensorRef::from_array_view(((), &rate[..])).map_err(ort_error)?;
        let outputs = self
            .session
            .run(ort::inputs![
                "input" => input,
                "state" => state,
                "sr" => rate,
            ])
            .map_err(ort_error)?;
        let (_, probability) = outputs["output"]
            .try_extract_tensor::<f32>()
            .map_err(ort_error)?;
        let (_, next) = outputs["stateN"]
            .try_extract_tensor::<f32>()
            .map_err(ort_error)?;
        let probability = *probability.first().ok_or("speech detection: no output")?;
        if next.len() != STATE {
            return Err("speech detection: unexpected state size".into());
        }
        self.state.copy_from_slice(next);
        drop(outputs);
        // Contexte de la trame suivante : la fin de celle-ci.
        self.input.copy_within(FRAME.., 0);
        Ok(probability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice_mode::audio::noise::Noise;

    /// Probabilités de toutes les trames de `samples`.
    pub fn probabilities(samples: &[f32]) -> Vec<f32> {
        let mut silero = Silero::load().expect("Silero VAD");
        samples
            .as_chunks::<FRAME>()
            .0
            .iter()
            .map(|frame| silero.probability(frame).unwrap())
            .collect()
    }

    /// Le fichier embarqué est celui du tag épinglé.
    #[test]
    fn the_model_is_the_pinned_one() {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(MODEL);
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(
            hex,
            "7ed98ddbad84ccac4cd0aeb3099049280713df825c610a8ed34543318f1b2c49"
        );
    }

    /// Contrat : silence et bruit faible (-60 dBFS et moins) ne sont pas de
    /// la parole (p < 0,1).
    #[test]
    #[ignore = "ONNX Runtime (ORT_DYLIB_PATH)"]
    fn voice_contract_silence_and_faint_noise_are_not_speech() {
        let silence = probabilities(&vec![0.0; 2 * RATE as usize]);
        assert!(silence.iter().all(|p| *p < 0.1), "{silence:?}");
        for (seed, rms) in [(1, 0.001), (2, 0.000_316), (3, 0.001)] {
            let mut noise = Noise::new(seed);
            // Uniforme : RMS = amplitude / √3.
            let faint: Vec<f32> = (0..3 * RATE as usize)
                .map(|_| noise.next() * rms * 3f32.sqrt())
                .collect();
            let worst = probabilities(&faint).into_iter().fold(0.0f32, f32::max);
            assert!(worst < 0.1, "{rms}: {worst}");
        }
    }

    /// Contrat : mêmes probabilités que la référence Python (silero-vad
    /// v6.2.3, `OnnxWrapper`, onnxruntime 1.2x) sur `jfk.wav`, contexte de
    /// 64 échantillons compris ; le début de parole tombe à la trame 11.
    #[test]
    #[ignore = "needs the speech samples of .tools and ONNX Runtime"]
    fn voice_contract_silero_matches_the_python_reference() {
        let samples = crate::voice_mode::audio::test_data::sample("jfk.wav");
        let found = probabilities(&samples);
        assert_eq!(found.len(), 343);
        let reference = [
            0.002, 0.085, 0.302, 0.133, 0.083, 0.043, 0.055, 0.053, 0.031, 0.038, 0.343, 0.946,
            0.933, 0.857, 0.973, 0.988,
        ];
        for (index, (found, expected)) in found.iter().zip(reference).enumerate() {
            assert!(
                (found - expected).abs() < 0.002,
                "frame {index}: {found} instead of {expected}"
            );
        }
        let speech = found.iter().filter(|p| **p >= 0.5).count();
        assert!((228..=238).contains(&speech), "{speech}");
    }
}
