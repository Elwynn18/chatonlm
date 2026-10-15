//! Processeur et whisper.cpp (étape 18a). Pour un binaire portable, ggml est
//! compilé sans `-march=native` (`GGML_NATIVE=OFF`, `.cargo/config.toml`),
//! donc avec les instructions de x86-64-v3 : SSE4.2, AVX, AVX2, BMI2, FMA et
//! F16C. Sur un processeur qui en manque une, le moindre appel à whisper.cpp
//! (journaux compris) arrêterait l'app (instruction illégale) : les modèles
//! Whisper y sont indisponibles, avec un message clair, et Parakeet (ONNX
//! Runtime, qui choisit ses noyaux à l'exécution) reste utilisable. Le
//! moteur d'images (`sd-server`, ggml compilé de la même façon) exige les
//! mêmes instructions ([`missing_for_ggml`]).

use std::sync::OnceLock;

use super::models::SpeechEngine;
use crate::i18n::t;

/// Instructions exigées par whisper.cpp : nom de `is_x86_feature_detected!`,
/// nom affiché.
const WHISPER_FEATURES: [(&str, &str); 6] = [
    ("sse4.2", "SSE4.2"),
    ("avx", "AVX"),
    ("avx2", "AVX2"),
    ("bmi2", "BMI2"),
    ("fma", "FMA"),
    ("f16c", "F16C"),
];

/// Instructions exigées qui manquent (noms affichés), d'après `has`.
fn missing_with(has: impl Fn(&str) -> bool) -> Vec<&'static str> {
    WHISPER_FEATURES
        .iter()
        .filter(|(feature, _)| !has(feature))
        .map(|(_, name)| *name)
        .collect()
}

/// Instruction présente sur ce processeur.
#[cfg(target_arch = "x86_64")]
fn detected(feature: &str) -> bool {
    match feature {
        "sse4.2" => std::arch::is_x86_feature_detected!("sse4.2"),
        "avx" => std::arch::is_x86_feature_detected!("avx"),
        "avx2" => std::arch::is_x86_feature_detected!("avx2"),
        "bmi2" => std::arch::is_x86_feature_detected!("bmi2"),
        "fma" => std::arch::is_x86_feature_detected!("fma"),
        "f16c" => std::arch::is_x86_feature_detected!("f16c"),
        _ => false,
    }
}

/// Hors x86-64, ggml n'exige aucune de ces instructions.
#[cfg(not(target_arch = "x86_64"))]
fn detected(_feature: &str) -> bool {
    true
}

/// Instructions qui manquent à ce processeur pour ggml compilé sans
/// `-march=native` (Whisper, moteur d'images ; lues une fois) ; vide :
/// utilisable.
pub fn missing_for_ggml() -> &'static [&'static str] {
    static MISSING: OnceLock<Vec<&'static str>> = OnceLock::new();
    MISSING.get_or_init(|| {
        #[cfg(feature = "e2e")]
        if let Some(missing) = injected() {
            return missing;
        }
        missing_with(detected)
    })
}

/// Instructions qui manquent à ce processeur pour les modèles de `engine`
/// (noms affichés) ; vide : modèles utilisables.
pub fn missing_for(engine: SpeechEngine) -> &'static [&'static str] {
    match engine {
        SpeechEngine::Whisper => missing_for_ggml(),
        SpeechEngine::Parakeet => &[],
    }
}

/// Refus d'un modèle Whisper sur ce processeur (chargement, téléchargement).
pub fn unsupported(missing: &[&str]) -> String {
    t!("dictation.cpuUnsupported", features = missing.join(", "))
}

/// Processeur imposé par les essais de l'app réelle
/// (`CHATONLM_E2E_CPU_MISSING=avx2,fma` : instructions à faire manquer, noms
/// de `is_x86_feature_detected!` ou affichés, sans casse) ; `None` sans
/// variable.
#[cfg(feature = "e2e")]
fn injected() -> Option<Vec<&'static str>> {
    let value = std::env::var("CHATONLM_E2E_CPU_MISSING").ok()?;
    Some(parse_injected(&value))
}

#[cfg(any(test, feature = "e2e"))]
fn parse_injected(value: &str) -> Vec<&'static str> {
    let absent: Vec<String> = value
        .split(',')
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect();
    missing_with(|feature| !absent.iter().any(|name| name == feature))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_required_instruction_is_checked() {
        assert!(missing_with(|_| true).is_empty());
        assert_eq!(
            missing_with(|_| false),
            ["SSE4.2", "AVX", "AVX2", "BMI2", "FMA", "F16C"]
        );
        // Chacune manque seule : Whisper refusé.
        for (feature, name) in WHISPER_FEATURES {
            assert_eq!(missing_with(|has| has != feature), [name]);
        }
        // Processeur x86-64-v2 (sans AVX) : tout ce qui suit SSE4.2 manque.
        assert_eq!(
            missing_with(|feature| feature == "sse4.2"),
            ["AVX", "AVX2", "BMI2", "FMA", "F16C"]
        );
    }

    #[test]
    fn parakeet_never_needs_them() {
        assert!(missing_for(SpeechEngine::Parakeet).is_empty());
    }

    #[test]
    fn injected_processors_are_read_from_names() {
        assert!(parse_injected("").is_empty());
        assert_eq!(parse_injected("avx2, FMA"), ["AVX2", "FMA"]);
        assert_eq!(parse_injected("sse4.2,unknown"), ["SSE4.2"]);
    }

    #[test]
    fn the_refusal_names_the_missing_instructions() {
        for lang in [
            crate::i18n::Lang::En,
            crate::i18n::Lang::Es,
            crate::i18n::Lang::Fr,
        ] {
            let message = t!("dictation.cpuUnsupported" in lang, features = "AVX2, FMA");
            assert!(message.contains("AVX2, FMA"), "{message}");
            assert!(message.contains("Parakeet"), "{message}");
        }
    }
}
