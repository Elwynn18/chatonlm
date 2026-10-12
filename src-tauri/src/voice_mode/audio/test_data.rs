//! Échantillons de parole des contrats, hors du dépôt : `.tools/speech-
//! samples` (ou `CHATONLM_SPEECH_TEST_SAMPLES`). Contrats lancés avec
//! `--ignored` seulement ; un échantillon absent les fait échouer.

use std::path::{Path, PathBuf};

use crate::dictation::audio::read_wav;

fn sample_path(name: &str) -> PathBuf {
    std::env::var_os("CHATONLM_SPEECH_TEST_SAMPLES")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tools/speech-samples"))
        .join(name)
}

/// Échantillon à 16 kHz mono.
pub fn sample(name: &str) -> Vec<f32> {
    let path = sample_path(name);
    read_wav(&path)
        .unwrap_or_else(|error| panic!("speech sample {}: {error}", path.display()))
        .to_16k()
}
