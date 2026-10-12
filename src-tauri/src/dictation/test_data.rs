//! Données des contrats de la dictée, hors du dépôt (`.tools/`) : modèles de
//! `.tools/speech-models` (ou `CHATONLM_SPEECH_TEST_MODELS`), échantillons de
//! `.tools/speech-samples` (ou `CHATONLM_SPEECH_TEST_SAMPLES`). Ces tests ne
//! tournent qu'avec `--ignored` : une donnée absente les fait alors échouer
//! (jamais sautés en silence).

use std::path::{Path, PathBuf};

use super::audio::read_wav;

fn tools() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tools")
}

fn from_env_or(variable: &str, default: &str) -> PathBuf {
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| tools().join(default))
}

/// Fichier ou dossier d'un modèle de test (`whisper/ggml-tiny.bin`...).
pub fn model(relative: &str) -> PathBuf {
    let path = from_env_or("CHATONLM_SPEECH_TEST_MODELS", "speech-models").join(relative);
    assert!(
        path.exists(),
        "speech test model missing: {} (see .tools/research/42-step17-contract.md § 0)",
        path.display()
    );
    path
}

fn sample_path(name: &str) -> PathBuf {
    from_env_or("CHATONLM_SPEECH_TEST_SAMPLES", "speech-samples").join(name)
}

/// Échantillon à 16 kHz.
pub fn sample(name: &str) -> Vec<f32> {
    let path = sample_path(name);
    read_wav(&path)
        .unwrap_or_else(|error| panic!("speech sample {}: {error}", path.display()))
        .to_16k()
}

/// Texte de référence d'un échantillon.
pub fn reference(name: &str) -> String {
    let path = sample_path(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reference {}: {error}", path.display()))
}
