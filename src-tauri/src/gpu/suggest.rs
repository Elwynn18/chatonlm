//! Modèles d'autocomplétion proposés (`completion.suggest`) : Qwen2.5-Coder
//! « base » (entraîné au FIM, jetons FIM dans son vocabulaire). Licence libre
//! par défaut : 0.5B, 1.5B et 7B sous Apache-2.0 ; le 3B est sous
//! « qwen-research » (usage non commercial), proposé avec cette mention et
//! jamais recommandé (rapport 87-c § 3.2).
//!
//! Depuis le passage à llama-server (lot L2), ce sont des identifiants de
//! Hugging Face (`<auteur>/<dépôt>:<QUANT>`) que le ModelHub télécharge
//! lui-même : les GGUF de ggml-org (l'organisation de llama.cpp, ceux que
//! llama.vscode propose), en Q8_0, seule précision publiée. Écart : le 7B
//! en Q8_0 pèse 8,1 Go (4,7 Go en Q4_K_M chez Ollama) : il n'est recommandé
//! qu'à partir de 16 Gio de mémoire graphique (plus de 8 Gio avant).
//! L'autocomplétion passe encore par Ollama jusqu'au lot L5 : un modèle
//! installé seulement dans la base locale n'y répond pas encore.

use serde::Serialize;

use crate::hub::fit;
use crate::hub::hardware::Machine;
use crate::hub::types::HubFit;
use crate::local_models::id::ModelId;

const MB: u64 = 1_000_000;
const GIB: u64 = 1024 * 1024 * 1024;

/// Un modèle proposable.
pub struct Known {
    pub model: &'static str,
    pub license: &'static str,
    pub commercial_use: bool,
    /// Taille du téléchargement (fiche de Hugging Face, nov. 2026).
    pub weights: u64,
    /// Paramètres, pour le cache de contexte.
    pub parameters: u64,
}

/// Dans l'ordre des tailles.
pub const KNOWN: [Known; 4] = [
    Known {
        model: SMALL,
        license: "Apache-2.0",
        commercial_use: true,
        weights: 531 * MB,
        parameters: 494_000_000,
    },
    Known {
        model: DEFAULT,
        license: "Apache-2.0",
        commercial_use: true,
        weights: 1_647 * MB,
        parameters: 1_540_000_000,
    },
    Known {
        model: "ggml-org/Qwen2.5-Coder-3B-Q8_0-GGUF:Q8_0",
        license: "qwen-research",
        commercial_use: false,
        weights: 3_285 * MB,
        parameters: 3_090_000_000,
    },
    Known {
        model: LARGE,
        license: "Apache-2.0",
        commercial_use: true,
        weights: 8_099 * MB,
        parameters: 7_620_000_000,
    },
];

/// Petit modèle (processeur seul, moins de 4 Gio de VRAM).
const SMALL: &str = "ggml-org/Qwen2.5-Coder-0.5B-Q8_0-GGUF:Q8_0";
const DEFAULT: &str = "ggml-org/Qwen2.5-Coder-1.5B-Q8_0-GGUF:Q8_0";
/// 16 Gio de VRAM et plus.
const LARGE: &str = "ggml-org/Qwen2.5-Coder-7B-Q8_0-GGUF:Q8_0";

/// Même modèle : identifiants de Hugging Face comparés comme la base locale
/// (forme `hf.co/…` d'Ollama acceptée, quantification sans la casse) ;
/// autres noms : sans la casse.
pub fn same_id(a: &str, b: &str) -> bool {
    match (ModelId::from_request(a), ModelId::from_request(b)) {
        (Some(a), Some(b)) => crate::local_models::id::same_model(&a.to_string(), &b.to_string()),
        _ => a.trim().eq_ignore_ascii_case(b.trim()),
    }
}

/// Un nom proposé par [`suggest`] (seuls ceux-là peuvent être offerts au
/// téléchargement par le pont).
pub fn known(model: &str) -> Option<&'static Known> {
    KNOWN.iter().find(|known| same_id(known.model, model))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggested {
    pub model: String,
    pub installed: bool,
    pub license: String,
    pub commercial_use: bool,
    /// Tient dans la mémoire de la machine ; `None` : machine inconnue
    /// (serveur distant).
    pub fits: Option<bool>,
    /// Mémoire estimée une fois chargé (contexte de l'autocomplétion).
    pub estimated_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestions {
    pub recommended: String,
    pub options: Vec<Suggested>,
}

/// Mémoire d'un modèle chargé avec ce contexte (poids, cache, marge).
pub fn estimate(weights: u64, parameters: Option<u64>, context: u32) -> u64 {
    fit::memory(
        weights,
        fit::kv_cache(None, parameters, None, context),
        context,
    )
}

/// Modèle recommandé pour cette machine.
pub fn recommended(machine: Option<&Machine>) -> &'static str {
    let Some(machine) = machine else {
        return DEFAULT;
    };
    let vram: u64 = machine.gpus.iter().map(|gpu| gpu.vram_total).sum();
    if vram < 4 * GIB {
        SMALL
    } else if vram >= 16 * GIB {
        LARGE
    } else {
        DEFAULT
    }
}

/// Propositions ; `installed` : modèles installés (base locale, et noms du
/// serveur Ollama pendant la transition : un `hf.co/…` tiré par Ollama
/// compte).
pub fn suggest(machine: Option<&Machine>, installed: &[String], context: u32) -> Suggestions {
    let hardware = machine.map(|machine| machine.clone().with_context(context));
    let options = KNOWN
        .iter()
        .map(|known| {
            let need = estimate(known.weights, Some(known.parameters), context);
            Suggested {
                model: known.model.to_owned(),
                installed: installed.iter().any(|name| same_id(name, known.model)),
                license: known.license.to_owned(),
                commercial_use: known.commercial_use,
                fits: hardware
                    .as_ref()
                    .map(|hardware| fit::level(need, hardware) == HubFit::Fits),
                estimated_bytes: need,
            }
        })
        .collect();
    Suggestions {
        recommended: recommended(machine).to_owned(),
        options,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::HubGpu;

    const THREE: &str = "ggml-org/Qwen2.5-Coder-3B-Q8_0-GGUF:Q8_0";

    fn machine(vram_gib: &[u64], ram_gib: u64) -> Machine {
        Machine {
            gpus: vram_gib
                .iter()
                .map(|&gib| HubGpu {
                    name: "GPU".into(),
                    vram_total: gib * GIB,
                    vram_free: Some(gib * GIB),
                })
                .collect(),
            ram_total: ram_gib * GIB,
            ram_available: ram_gib * GIB,
        }
    }

    #[test]
    fn the_recommendation_follows_the_graphics_card() {
        assert_eq!(recommended(Some(&machine(&[], 32))), SMALL);
        assert_eq!(recommended(Some(&machine(&[2], 16))), SMALL);
        assert_eq!(recommended(Some(&machine(&[4], 16))), DEFAULT);
        assert_eq!(recommended(Some(&machine(&[8], 16))), DEFAULT);
        assert_eq!(recommended(Some(&machine(&[12], 16))), DEFAULT);
        assert_eq!(recommended(Some(&machine(&[16], 16))), LARGE);
        assert_eq!(recommended(Some(&machine(&[8, 8], 16))), LARGE);
        assert_eq!(recommended(None), DEFAULT);
    }

    /// Des identifiants que le ModelHub télécharge : `auteur/dépôt:QUANT`,
    /// modèles de base (pas « Instruct »).
    #[test]
    fn suggestions_are_hugging_face_ids() {
        for known in &KNOWN {
            let id = ModelId::parse(known.model).unwrap_or_else(|| panic!("{}", known.model));
            assert!(id.repo_id().is_some(), "{}", known.model);
            assert!(!known.model.to_ascii_lowercase().contains("instruct"));
        }
    }

    #[test]
    fn the_three_b_is_listed_but_never_recommended() {
        for vram in [0, 2, 4, 6, 8, 12, 24, 48] {
            let gpus: Vec<u64> = if vram == 0 { vec![] } else { vec![vram] };
            let suggested = suggest(Some(&machine(&gpus, 32)), &[], 4096);
            assert_ne!(suggested.recommended, THREE);
        }
        let suggested = suggest(None, &[], 4096);
        let three = suggested
            .options
            .iter()
            .find(|option| option.model == THREE)
            .unwrap();
        assert!(!three.commercial_use);
        assert_eq!(three.license, "qwen-research");
        assert!(
            suggested
                .options
                .iter()
                .filter(|option| option.model != THREE)
                .all(|option| option.commercial_use && option.license == "Apache-2.0")
        );
        assert_eq!(three.fits, None, "unknown machine");
    }

    #[test]
    fn installed_and_fitting_models_are_told() {
        // Base locale (quantification d'une autre casse), et un nom tiré
        // par Ollama (`hf.co/…`).
        let installed = vec![
            "ggml-org/Qwen2.5-Coder-1.5B-Q8_0-GGUF:q8_0".to_owned(),
            "hf.co/ggml-org/Qwen2.5-Coder-0.5B-Q8_0-GGUF:Q8_0".into(),
            "qwen3:0.6b".into(),
        ];
        let suggested = suggest(Some(&machine(&[4], 16)), &installed, 4096);
        let by_name = |name: &str| {
            suggested
                .options
                .iter()
                .find(|option| option.model == name)
                .unwrap()
                .clone()
        };
        assert!(by_name(DEFAULT).installed);
        assert!(by_name(SMALL).installed);
        assert!(!by_name(LARGE).installed);
        assert_eq!(by_name(SMALL).fits, Some(true));
        // 7B : plus que 90 % de 4 Gio.
        assert_eq!(by_name(LARGE).fits, Some(false));
        assert!(known(LARGE).is_some());
        assert!(known("hf.co/ggml-org/Qwen2.5-Coder-0.5B-Q8_0-GGUF:q8_0").is_some());
        // Le dépôt est sensible à la casse ; les anciens noms d'Ollama ne
        // sont plus proposés.
        assert!(known("GGML-ORG/Qwen2.5-Coder-0.5B-Q8_0-GGUF:Q8_0").is_none());
        assert!(known("qwen2.5-coder:1.5b-base").is_none());
        assert!(known("qwen3:0.6b").is_none());
    }
}
