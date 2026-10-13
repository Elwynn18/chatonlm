//! Estimation de la mémoire d'un modèle et place dans la machine (34 § 9,
//! contrat § 6) : poids + cache de contexte + marge, comparés à la VRAM et à
//! la RAM.

use super::gguf::GgufFacts;
use super::types::{HubFit, HubHardware};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const GIB: u64 = 1024 * MIB;

/// Mémoire laissée au système (RAM).
const SYSTEM_RAM: u64 = 6 * GIB;
/// Part de la VRAM utilisable.
const VRAM_SHARE: f64 = 0.9;

/// Contexte minimal d'un modèle de vision chez Ollama.
const VISION_MIN_CONTEXT: u32 = 2048;

/// Contexte qu'Ollama choisit d'après la VRAM totale de tous les GPU, sans
/// réglage (routes.go:2211-2227).
pub fn default_context(vram_total: u64) -> u32 {
    if vram_total >= 47 * GIB {
        262_144
    } else if vram_total >= 23 * GIB {
        32_768
    } else {
        4_096
    }
}

/// Contexte d'un modèle : celui de la machine, plafonné au maximum du modèle,
/// au moins 2048 pour un modèle de vision.
pub fn model_context(machine: u32, model: Option<u32>, vision: bool) -> u32 {
    let mut context = match model.filter(|&m| m > 0) {
        Some(model) => machine.min(model),
        None => machine,
    };
    if vision {
        context = context.max(VISION_MIN_CONTEXT);
    }
    context
}

/// Taille estimée d'un dépôt dont on ne connaît aucune version
/// téléchargeable (les autres : `catalog::model`, d'après le libellé de la
/// version conseillée) : 0,64 octet par paramètre (Q4_K_M, 0,60 à 0,67
/// mesurés, 35 § 2.2), +30 % pour un modèle de vision (projecteur).
pub fn list_size(parameters: Option<u64>, vision: bool) -> Option<u64> {
    let parameters = u128::from(parameters.filter(|&p| p > 0)?);
    let size = if vision {
        parameters * 64 * 13 / 1000
    } else {
        parameters * 64 / 100
    };
    u64::try_from(size).ok()
}

/// Cache de contexte (octets) : lu dans l'en-tête GGUF couche par couche
/// ([`GgufFacts::kv_bytes`]), sinon estimé : `ctx × (100 Kio + 3,3 Kio ×
/// milliards de paramètres)`, réduit pour les architectures dont la plupart
/// des couches ne gardent pas tout le contexte ([`attention_share`]).
pub fn kv_cache(
    facts: Option<&GgufFacts>,
    parameters: Option<u64>,
    architecture: Option<&str>,
    context: u32,
) -> u64 {
    if let Some(bytes) = facts.and_then(|facts| facts.kv_bytes(context)) {
        return bytes;
    }
    // 3,3 Kio par milliard = 33 792 octets pour 10^10 paramètres.
    let per_token =
        u128::from(100 * KIB) + u128::from(parameters.unwrap_or(0)) * 33_792 / 10_000_000_000;
    let (part, whole) = attention_share(architecture.unwrap_or(""));
    u64::try_from(per_token * u128::from(context) * part / whole).unwrap_or(u64::MAX)
}

/// Part du cache d'un modèle classique, sans en-tête GGUF : une couche sur
/// quatre a de l'attention dans qwen35 et voisins (un tiers, avec l'état des
/// couches récurrentes), une sur huit dans minimax-01 ; cinq couches sur six
/// de Gemma 3 et 4 et une sur deux de Gemma 2 et gpt-oss ne gardent qu'une
/// fenêtre (llama.cpp b11232, voir `gguf.rs`).
fn attention_share(architecture: &str) -> (u128, u128) {
    match architecture {
        "qwen3next" | "qwen35" | "qwen35moe" | "qwen4exp" => (1, 3),
        "minimax-01" => (1, 8),
        "gemma3" | "gemma3n" | "gemma4" => (1, 5),
        "gemma2" | "gpt-oss" => (1, 2),
        _ => (1, 1),
    }
}

/// Marge : 0,5 Gio, plus le lot d'Ollama (768 Mio au-delà de 4096 jetons,
/// 2 Gio au-delà de 32768).
pub fn margin(context: u32) -> u64 {
    let batch = if context > 32_768 {
        2 * GIB
    } else if context > 4_096 {
        768 * MIB
    } else {
        0
    };
    GIB / 2 + batch
}

/// Mémoire nécessaire au chargement.
pub fn memory(weights: u64, kv: u64, context: u32) -> u64 {
    weights.saturating_add(kv).saturating_add(margin(context))
}

/// Place d'un besoin de mémoire dans la machine : avec GPU, `Fits` sous 90 %
/// de la VRAM totale, `Partial` sous 90 % de la VRAM plus la RAM moins 6 Gio,
/// sinon `TooLarge` ; sans GPU, `Fits` sous la RAM moins 6 Gio.
pub fn level(need: u64, hardware: &HubHardware) -> HubFit {
    let ram = hardware.ram_total.saturating_sub(SYSTEM_RAM);
    let vram: u64 = hardware.gpus.iter().map(|gpu| gpu.vram_total).sum();
    if hardware.gpus.is_empty() || vram == 0 {
        return if need <= ram {
            HubFit::Fits
        } else {
            HubFit::TooLarge
        };
    }
    let usable = (vram as f64 * VRAM_SHARE) as u64;
    if need <= usable {
        HubFit::Fits
    } else if need <= usable.saturating_add(ram) {
        HubFit::Partial
    } else {
        HubFit::TooLarge
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::HubGpu;

    fn machine(vram_gib: &[u64], ram_gib: u64) -> HubHardware {
        HubHardware {
            gpus: vram_gib
                .iter()
                .map(|&vram| HubGpu {
                    name: "GPU".into(),
                    vram_total: vram * GIB,
                    vram_free: None,
                })
                .collect(),
            ram_total: ram_gib * GIB,
            ram_available: ram_gib * GIB / 2,
            context: 4096,
        }
    }

    #[test]
    fn the_default_context_follows_the_vram() {
        assert_eq!(default_context(0), 4096);
        assert_eq!(default_context(16 * GIB), 4096);
        // RTX 3090 : 24576 Mio annoncés.
        assert_eq!(default_context(24 * GIB), 32768);
        assert_eq!(default_context(23 * GIB), 32768);
        assert_eq!(default_context(48 * GIB), 262144);
        assert_eq!(model_context(32768, Some(8192), false), 8192);
        assert_eq!(model_context(32768, None, false), 32768);
        assert_eq!(model_context(1024, Some(8192), true), 2048);
        assert_eq!(model_context(4096, Some(0), false), 4096);
    }

    #[test]
    fn margins_and_estimates() {
        assert_eq!(margin(4096), GIB / 2);
        assert_eq!(margin(8192), GIB / 2 + 768 * MIB);
        assert_eq!(margin(32768), GIB / 2 + 768 * MIB);
        assert_eq!(margin(32769), GIB / 2 + 2 * GIB);
        assert_eq!(list_size(Some(8_000_000_000), false), Some(5_120_000_000));
        assert_eq!(list_size(Some(10_000_000_000), true), Some(8_320_000_000));
        assert_eq!(list_size(None, true), None);
        // Sans en-tête : 100 Kio + 3,3 Kio par milliard, par jeton.
        let kv = kv_cache(None, Some(10_000_000_000), None, 1000);
        assert_eq!(kv, ((100.0 + 33.0) * 1024.0 * 1000.0) as u64);
        // Architectures hybrides ou à fenêtre glissante : une part seulement.
        assert_eq!(
            kv_cache(None, Some(10_000_000_000), Some("qwen35"), 1000),
            kv / 3
        );
        assert_eq!(
            kv_cache(None, Some(10_000_000_000), Some("gemma4"), 1000),
            kv / 5
        );
        assert_eq!(
            kv_cache(None, Some(10_000_000_000), Some("llama"), 1000),
            kv
        );
        let facts = GgufFacts {
            architecture: Some("qwen3".into()),
            block_count: Some(28),
            head_count_kv: Some(8),
            key_length: Some(128),
            ..Default::default()
        };
        assert_eq!(kv_cache(Some(&facts), None, None, 32768), 3584 * MIB);
    }

    #[test]
    fn levels_with_and_without_gpu() {
        let rtx = machine(&[24], 32);
        assert_eq!(level(20 * GIB, &rtx), HubFit::Fits);
        assert_eq!(level(22 * GIB, &rtx), HubFit::Partial);
        assert_eq!(level(21 * GIB + 26 * GIB, &rtx), HubFit::Partial);
        assert_eq!(level(50 * GIB, &rtx), HubFit::TooLarge);
        // Deux GPU : VRAM additionnée.
        assert_eq!(level(40 * GIB, &machine(&[24, 24], 32)), HubFit::Fits);
        // Sans GPU : la RAM moins 6 Gio.
        let cpu = machine(&[], 16);
        assert_eq!(level(10 * GIB, &cpu), HubFit::Fits);
        assert_eq!(level(11 * GIB, &cpu), HubFit::TooLarge);
        // Petite machine.
        assert_eq!(level(GIB, &machine(&[], 4)), HubFit::TooLarge);
    }
}
