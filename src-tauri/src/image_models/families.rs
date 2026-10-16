//! Familles de modèles d'images connues de ChatonLM (contrat § 4), lues dans
//! `families.json` (embarqué) : modèles de base de Hugging Face qui les
//! désignent, fichiers acceptés, pièces annexes épinglées et réglages
//! d'échantillonnage ; motif des familles écartées ; recommandations.
//!
//! Builds de débogage et d'essai : `CHATONLM_IMAGE_FAMILIES` (chemin d'un
//! JSON de même forme) remplace `families.json` (familles d'essai à petites
//! pièces servies par le faux Hugging Face).
//!
//! Reconnaissance d'un dépôt (sans devinette sur son nom) : son identifiant
//! est une base d'une famille (cran 0), sinon l'une de ses bases déclarées
//! (cran 1), sinon une base de ses bases (cran 2, fiches lues par
//! l'appelant). Une base qui désigne une famille écartée donne
//! `Unsupported`.

use std::collections::HashMap;

use regex::Regex;
use serde::Deserialize;

use super::types::{ImageComponentRole, ImageFamily};
use crate::image_gen::{Component, Sampling};

/// Variable qui remplace `families.json` (débogage, essais, e2e).
pub const FAMILIES_ENV: &str = "CHATONLM_IMAGE_FAMILIES";

/// Pièce annexe épinglée (encodeur de texte, VAE).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Piece {
    pub role: ImageComponentRole,
    pub component: Component,
    pub repo: String,
    pub revision: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
    pub license: Option<String>,
}

/// Famille reconnue.
#[derive(Clone, Debug)]
pub struct Family {
    pub id: String,
    pub name: String,
    /// Modèles de base qui la désignent, en minuscules.
    bases: Vec<String>,
    /// Modèle complet en un `.safetensors` à la racine accepté (SD 1.5,
    /// SDXL), en plus des GGUF.
    pub single_file: bool,
    /// Taille minimale d'un modèle complet en un `.safetensors` (≈ 1,5 Go
    /// pour SD 1.5, ≈ 4 Go pour SDXL) : un plus petit est un adaptateur.
    pub min_size: u64,
    /// Option du moteur qui reçoit le fichier choisi.
    pub model_component: Component,
    pub pieces: Vec<Piece>,
    pub sampling: Sampling,
    /// Options de lancement du moteur (`--model-args ...`).
    pub options: Vec<String>,
    /// Côté d'une image dont la taille n'est pas demandée, avec une carte
    /// graphique (512 pour SD 1.5, entraîné à cette taille ; 1024 sinon).
    pub default_side: u32,
}

impl Family {
    pub fn info(&self) -> ImageFamily {
        ImageFamily {
            id: self.id.clone(),
            name: self.name.clone(),
        }
    }

    /// Fichier d'un dépôt que cette famille sait charger comme modèle : un
    /// GGUF (ni auxiliaire ni morceau), ou, pour SD 1.5 et SDXL, un
    /// `.safetensors` à la racine d'au moins `min_size` octets (`size` :
    /// taille connue, sinon jugé sur le nom seul). Pour ces familles, les
    /// noms d'adaptateurs et de morceaux (LoRA, LCM, UNet, ControlNet, VAE,
    /// embedding, inpainting) sont écartés.
    pub fn accepts(&self, path: &str, size: Option<u64>) -> bool {
        if self.single_file && not_a_whole_model(path) {
            return false;
        }
        accepted_gguf(path)
            || (self.single_file
                && single_file(path)
                && size.is_none_or(|size| size >= self.min_size))
    }
}

/// Nom d'un adaptateur ou d'un morceau de modèle (SD 1.5, SDXL).
fn not_a_whole_model(path: &str) -> bool {
    static PARTS: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)lora|lcm|unet|controlnet|vae|embedding|inpaint")
            .expect("valid parts pattern")
    });
    PARTS.is_match(path)
}

/// GGUF proposé comme modèle : ni projecteur, ni brouillon, ni morceau.
pub fn accepted_gguf(path: &str) -> bool {
    use crate::hub::quant;
    quant::is_gguf(path) && !quant::is_auxiliary(path) && !is_shard(path)
}

/// Modèle complet en un fichier : `.safetensors` à la racine du dépôt.
pub fn single_file(path: &str) -> bool {
    !path.contains('/') && path.to_ascii_lowercase().ends_with(".safetensors")
}

fn is_shard(path: &str) -> bool {
    static SHARD: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)-\d{5}-of-\d{5}\.gguf$").expect("valid shard pattern")
    });
    SHARD.is_match(path)
}

/// Modèle proposé sans réseau (révision, taille et empreinte épinglées).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    pub repo: String,
    pub revision: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
    pub license: Option<String>,
    pub family: String,
    pub name: String,
}

/// Ce que disent les bases d'un dépôt.
#[derive(Clone, Copy, Debug)]
pub enum Recognition<'a> {
    Family(&'a Family),
    /// Famille écartée par ChatonLM.
    Unsupported,
}

pub struct Families {
    families: Vec<Family>,
    unsupported: Regex,
    pub recommendations: Vec<Recommendation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    pieces: HashMap<String, Piece>,
    families: Vec<RawFamily>,
    unsupported: String,
    recommendations: Vec<Recommendation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFamily {
    id: String,
    name: String,
    bases: Vec<String>,
    single_file: bool,
    #[serde(default)]
    min_size: u64,
    #[serde(default = "default_side")]
    default_side: u32,
    model: Component,
    pieces: Vec<String>,
    sampling: RawSampling,
    options: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSampling {
    steps: u32,
    cfg: f32,
    sample_method: Option<String>,
    scheduler: Option<String>,
    flow_shift: Option<f32>,
}

/// Côté par défaut d'une famille qui n'en dit rien.
fn default_side() -> u32 {
    1024
}

/// Empreinte SHA-256 écrite en hexadécimal (64 caractères).
pub fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Families {
    /// Familles de `json` ; refusées si une pièce, une empreinte ou une
    /// famille citée manque.
    pub fn parse(json: &str) -> Result<Self, String> {
        let raw: Raw = serde_json::from_str(json).map_err(|error| error.to_string())?;
        for (key, piece) in &raw.pieces {
            if !is_sha256(&piece.sha256) || piece.revision.is_empty() || piece.size == 0 {
                return Err(format!("piece {key}: bad pin"));
            }
        }
        let mut families = Vec::new();
        for family in raw.families {
            if families.iter().any(|known: &Family| known.id == family.id) {
                return Err(format!("family {} twice", family.id));
            }
            let pieces = family
                .pieces
                .iter()
                .map(|key| {
                    raw.pieces
                        .get(key)
                        .cloned()
                        .ok_or_else(|| format!("family {}: unknown piece {key}", family.id))
                })
                .collect::<Result<_, _>>()?;
            families.push(Family {
                bases: family
                    .bases
                    .iter()
                    .map(|base| base.to_ascii_lowercase())
                    .collect(),
                id: family.id,
                name: family.name,
                single_file: family.single_file,
                min_size: family.min_size,
                model_component: family.model,
                pieces,
                sampling: Sampling {
                    steps: family.sampling.steps,
                    cfg_scale: family.sampling.cfg,
                    sample_method: family.sampling.sample_method,
                    scheduler: family.sampling.scheduler,
                    flow_shift: family.sampling.flow_shift,
                },
                options: family.options,
                default_side: family.default_side,
            });
        }
        for recommendation in &raw.recommendations {
            if !families
                .iter()
                .any(|family| family.id == recommendation.family)
                || !is_sha256(&recommendation.sha256)
            {
                return Err(format!("recommendation {}: bad", recommendation.repo));
            }
        }
        Ok(Self {
            families,
            unsupported: Regex::new(&format!("(?i){}", raw.unsupported))
                .map_err(|error| error.to_string())?,
            recommendations: raw.recommendations,
        })
    }

    /// Familles embarquées.
    pub fn embedded() -> Self {
        Self::parse(include_str!("families.json")).expect("valid families.json")
    }

    /// Familles de l'app : celles de `CHATONLM_IMAGE_FAMILIES` (débogage,
    /// essais, e2e) si la variable est définie et le fichier valide, sinon
    /// celles embarquées.
    pub fn load() -> Self {
        #[cfg(any(test, debug_assertions, feature = "e2e"))]
        if let Some(path) = std::env::var_os(FAMILIES_ENV).filter(|path| !path.is_empty()) {
            match std::fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|json| Self::parse(&json))
            {
                Ok(families) => return families,
                // i18n-ignore : journal.
                Err(error) => eprintln!("chatonlm: {FAMILIES_ENV} ignored: {error}"),
            }
        }
        Self::embedded()
    }

    pub fn get(&self, id: &str) -> Option<&Family> {
        self.families.iter().find(|family| family.id == id)
    }

    /// Famille dont `id` est un modèle de base.
    pub fn by_base(&self, id: &str) -> Option<&Family> {
        let id = id.to_ascii_lowercase();
        self.families
            .iter()
            .find(|family| family.bases.contains(&id))
    }

    /// Base d'une famille écartée.
    pub fn unsupported_base(&self, id: &str) -> bool {
        self.unsupported.is_match(id)
    }

    /// Ce que disent les identifiants d'un cran : la première famille
    /// reconnue, sinon `Unsupported` si l'un désigne une famille écartée
    /// (`check_unsupported` : faux pour le dépôt lui-même, jamais jugé sur
    /// son nom).
    pub fn recognize<'a, S: AsRef<str>>(
        &'a self,
        ids: &[S],
        check_unsupported: bool,
    ) -> Option<Recognition<'a>> {
        if let Some(family) = ids.iter().find_map(|id| self.by_base(id.as_ref())) {
            return Some(Recognition::Family(family));
        }
        (check_unsupported && ids.iter().any(|id| self.unsupported_base(id.as_ref())))
            .then_some(Recognition::Unsupported)
    }

    /// Recommandation de ce dépôt et de ce fichier.
    pub fn recommendation(&self, repo: &str, file: &str) -> Option<&Recommendation> {
        self.recommendations
            .iter()
            .find(|found| found.repo.eq_ignore_ascii_case(repo) && found.file == file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_families_follow_the_contract() {
        let families = Families::embedded();
        let ids: Vec<&str> = families
            .families
            .iter()
            .map(|family| family.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "flux2-klein-4b",
                "flux2-klein-base-4b",
                "flux2-klein-9b",
                "z-image-turbo",
                "z-image",
                "flux1-schnell",
                "flux1-dev",
                "chroma",
                "qwen-image",
                "sdxl",
                "sd15"
            ]
        );
        let klein = families
            .by_base("Black-Forest-Labs/FLUX.2-klein-4B")
            .unwrap();
        assert_eq!(klein.id, "flux2-klein-4b");
        assert_eq!(klein.model_component, Component::DiffusionModel);
        assert_eq!(
            klein
                .pieces
                .iter()
                .map(|piece| (piece.component, piece.repo.as_str()))
                .collect::<Vec<_>>(),
            [
                (Component::Llm, "unsloth/Qwen3-4B-GGUF"),
                (Component::Vae, "black-forest-labs/FLUX.2-small-decoder")
            ]
        );
        assert_eq!((klein.sampling.steps, klein.sampling.cfg_scale), (4, 1.0));
        let schnell = families.get("flux1-schnell").unwrap();
        assert_eq!(
            schnell
                .pieces
                .iter()
                .map(|piece| piece.component)
                .collect::<Vec<_>>(),
            [Component::T5xxl, Component::ClipL, Component::Vae]
        );
        assert_eq!(schnell.sampling.sample_method.as_deref(), Some("euler"));
        let qwen = families.get("qwen-image").unwrap();
        assert_eq!(qwen.sampling.flow_shift, Some(3.0));
        assert_eq!(qwen.sampling.cfg_scale, 2.5);
        let chroma = families.get("chroma").unwrap();
        assert_eq!(
            chroma.options,
            ["--model-args", "chroma_use_dit_mask=false"]
        );
        let sdxl = families.get("sdxl").unwrap();
        assert!(sdxl.single_file);
        assert_eq!(sdxl.model_component, Component::Model);
        assert!(families.get("sd15").unwrap().pieces.is_empty());
        // Côté par défaut : 512 pour SD 1.5 seulement (champ absent : 1024).
        let sides: Vec<(&str, u32)> = families
            .families
            .iter()
            .filter(|family| family.default_side != 1024)
            .map(|family| (family.id.as_str(), family.default_side))
            .collect();
        assert_eq!(sides, [("sd15", 512)]);
        // Recommandations épinglées.
        let klein_q4 = families
            .recommendation("leejet/FLUX.2-klein-4B-GGUF", "flux-2-klein-4b-Q4_0.gguf")
            .unwrap();
        assert_eq!(klein_q4.size, 2_460_378_560);
        assert!(klein_q4.sha256.starts_with("d1023499"));
        assert!(
            families
                .recommendation("leejet/Z-Image-Turbo-GGUF", "z_image_turbo-Q4_K.gguf")
                .is_some_and(|found| found.family == "z-image-turbo")
        );
    }

    #[test]
    fn bases_are_recognized_and_excluded_families_told_apart() {
        let families = Families::embedded();
        let found = |ids: &[&str]| match families.recognize(ids, true) {
            Some(Recognition::Family(family)) => Some(family.id.clone()),
            Some(Recognition::Unsupported) => Some("unsupported".into()),
            None => None,
        };
        assert_eq!(
            found(&["someone/other", "Tongyi-MAI/Z-Image-Turbo"]).as_deref(),
            Some("z-image-turbo")
        );
        assert_eq!(
            found(&["Qwen/Qwen-Image-2512"]).as_deref(),
            Some("qwen-image")
        );
        for excluded in [
            "Lightricks/LTX-2",
            "google/gemma-3-4b-it",
            "Qwen/Qwen-Image-2.1",
            "Wan-AI/Wan2.2-T2V-A14B",
            "tencent/HunyuanVideo",
        ] {
            assert_eq!(
                found(&[excluded]).as_deref(),
                Some("unsupported"),
                "{excluded}"
            );
        }
        assert_eq!(found(&["someone/mystery-diffusion"]), None);
        // Le nom du dépôt lui-même ne fait rien deviner.
        assert!(families.recognize(&["x/LTX-2-GGUF"], false).is_none());
    }

    #[test]
    fn accepted_files_depend_on_the_family() {
        let families = Families::embedded();
        let klein = families.get("flux2-klein-4b").unwrap();
        let sdxl = families.get("sdxl").unwrap();
        let sd15 = families.get("sd15").unwrap();
        assert!(klein.accepts("flux-2-klein-4b-Q4_0.gguf", Some(2_460_378_560)));
        assert!(!klein.accepts("mmproj-F16.gguf", None));
        assert!(!klein.accepts("big-00001-of-00002.gguf", None));
        assert!(!klein.accepts("sd_xl_base_1.0.safetensors", None));
        assert!(sdxl.accepts("sd_xl_base_1.0.safetensors", Some(6_938_078_334)));
        assert!(sdxl.accepts("sdxl-Q8_0.gguf", None));
        assert!(!sdxl.accepts("unet/diffusion_pytorch_model.safetensors", None));
        // LoRA à la racine du dépôt officiel (49 Mo) : pas un modèle.
        assert!(!sdxl.accepts(
            "sd_xl_offset_example-lora_1.0.safetensors",
            Some(49_553_604)
        ));
        for name in [
            "lcm-sdxl.safetensors",
            "sdxl_vae.safetensors",
            "controlnet-canny.safetensors",
            "sd-xl-inpainting.safetensors",
            "embedding-pack.safetensors",
            "unet-Q8_0.gguf",
        ] {
            assert!(!sdxl.accepts(name, Some(7_000_000_000)), "{name}");
        }
        // Trop petit pour un modèle complet.
        assert!(!sdxl.accepts("tiny.safetensors", Some(2_000_000_000)));
        assert!(sd15.accepts("v1-5-pruned-emaonly.safetensors", Some(4_265_380_512)));
        assert!(!sd15.accepts("small.safetensors", Some(1_000_000_000)));
    }

    #[test]
    fn broken_families_are_refused() {
        let json = include_str!("families.json");
        let mut value: serde_json::Value = serde_json::from_str(json).unwrap();
        value["families"][0]["pieces"][0] = "missing".into();
        assert!(Families::parse(&value.to_string()).is_err());
        let mut value: serde_json::Value = serde_json::from_str(json).unwrap();
        value["pieces"]["qwen3-4b"]["sha256"] = "abc".into();
        assert!(Families::parse(&value.to_string()).is_err());
        let mut value: serde_json::Value = serde_json::from_str(json).unwrap();
        value["recommendations"][0]["family"] = "nope".into();
        assert!(Families::parse(&value.to_string()).is_err());
    }
}
