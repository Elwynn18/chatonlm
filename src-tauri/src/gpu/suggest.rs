//! Modèles d'autocomplétion proposés (`completion.suggest`) : Qwen2.5-Coder
//! « base » (capacité `insert`, entraîné au FIM). Licence libre par défaut :
//! 0.5B, 1.5B et 7B sous Apache-2.0 ; le 3B est sous « qwen-research »
//! (usage non commercial), proposé avec cette mention et jamais recommandé
//! (choix de l'utilisateur, rapport 87-c § 3.2).

use serde::Serialize;

use crate::hub::fit;
use crate::hub::hardware::Machine;
use crate::hub::types::HubFit;
use crate::ollama::running::same_model;

const MB: u64 = 1_000_000;
const GIB: u64 = 1024 * 1024 * 1024;

/// Un modèle proposable.
pub struct Known {
    pub model: &'static str,
    pub license: &'static str,
    pub commercial_use: bool,
    /// Taille du téléchargement (registre d'Ollama, oct. 2026).
    pub weights: u64,
    /// Paramètres, pour le cache de contexte.
    pub parameters: u64,
}

/// Dans l'ordre des tailles.
pub const KNOWN: [Known; 4] = [
    Known {
        model: "qwen2.5-coder:0.5b-base",
        license: "Apache-2.0",
        commercial_use: true,
        // Mesuré sur l'instance de test (Q8_0).
        weights: 531 * MB,
        parameters: 494_000_000,
    },
    Known {
        model: "qwen2.5-coder:1.5b-base",
        license: "Apache-2.0",
        commercial_use: true,
        weights: 986 * MB,
        parameters: 1_540_000_000,
    },
    Known {
        model: "qwen2.5-coder:3b-base",
        license: "qwen-research",
        commercial_use: false,
        weights: 1_930 * MB,
        parameters: 3_090_000_000,
    },
    Known {
        model: "qwen2.5-coder:7b-base",
        license: "Apache-2.0",
        commercial_use: true,
        weights: 4_680 * MB,
        parameters: 7_620_000_000,
    },
];

/// Petit modèle (processeur seul, moins de 4 Gio de VRAM).
const SMALL: &str = "qwen2.5-coder:0.5b-base";
const DEFAULT: &str = "qwen2.5-coder:1.5b-base";
/// Plus de 8 Gio de VRAM.
const LARGE: &str = "qwen2.5-coder:7b-base";

/// Un nom proposé par [`suggest`] (seuls ceux-là peuvent être offerts au
/// téléchargement par le pont).
pub fn known(model: &str) -> Option<&'static Known> {
    KNOWN.iter().find(|known| same_model(known.model, model))
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
    } else if vram > 8 * GIB {
        LARGE
    } else {
        DEFAULT
    }
}

/// Propositions ; `installed` : noms installés sur le serveur.
pub fn suggest(machine: Option<&Machine>, installed: &[String], context: u32) -> Suggestions {
    let hardware = machine.map(|machine| machine.clone().with_context(context));
    let options = KNOWN
        .iter()
        .map(|known| {
            let need = estimate(known.weights, Some(known.parameters), context);
            Suggested {
                model: known.model.to_owned(),
                installed: installed.iter().any(|name| same_model(name, known.model)),
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
        assert_eq!(recommended(Some(&machine(&[12], 16))), LARGE);
        assert_eq!(recommended(Some(&machine(&[6, 6], 16))), LARGE);
        assert_eq!(recommended(None), DEFAULT);
    }

    #[test]
    fn the_three_b_is_listed_but_never_recommended() {
        for vram in [0, 2, 4, 6, 8, 12, 24, 48] {
            let gpus: Vec<u64> = if vram == 0 { vec![] } else { vec![vram] };
            let suggested = suggest(Some(&machine(&gpus, 32)), &[], 4096);
            assert_ne!(suggested.recommended, "qwen2.5-coder:3b-base");
        }
        let suggested = suggest(None, &[], 4096);
        let three = suggested
            .options
            .iter()
            .find(|option| option.model == "qwen2.5-coder:3b-base")
            .unwrap();
        assert!(!three.commercial_use);
        assert_eq!(three.license, "qwen-research");
        assert!(
            suggested
                .options
                .iter()
                .filter(|option| option.model != "qwen2.5-coder:3b-base")
                .all(|option| option.commercial_use && option.license == "Apache-2.0")
        );
        assert_eq!(three.fits, None, "unknown machine");
    }

    #[test]
    fn installed_and_fitting_models_are_told() {
        let installed = vec!["qwen2.5-coder:1.5b-base".to_owned(), "qwen3:0.6b".into()];
        let suggested = suggest(Some(&machine(&[4], 16)), &installed, 4096);
        let by_name = |name: &str| {
            suggested
                .options
                .iter()
                .find(|option| option.model == name)
                .unwrap()
                .clone()
        };
        assert!(by_name("qwen2.5-coder:1.5b-base").installed);
        assert!(!by_name("qwen2.5-coder:0.5b-base").installed);
        assert_eq!(by_name("qwen2.5-coder:0.5b-base").fits, Some(true));
        // 7B : plus que 90 % de 4 Gio.
        assert_eq!(by_name("qwen2.5-coder:7b-base").fits, Some(false));
        assert!(known("qwen2.5-coder:7b-base").is_some());
        assert!(known("QWEN2.5-CODER:0.5B-BASE").is_some());
        assert!(known("qwen3:0.6b").is_none());
    }
}
