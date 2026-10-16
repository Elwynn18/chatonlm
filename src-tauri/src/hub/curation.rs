//! Liste de confiance, familles et architectures du ModelHub, lues dans
//! `curation.json` (embarqué ; 35 R4 et § 7, contrat § 4).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use super::types::HubTrust;

/// Curation embarquée, lue une fois.
pub static CURATION: LazyLock<Curation> =
    LazyLock::new(|| Curation::parse(include_str!("curation.json")).expect("valid curation.json"));

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    labs: Vec<String>,
    quantizers: Vec<RawQuantizer>,
    base_labs: Vec<String>,
    excluded: Vec<String>,
    families: Vec<RawFamily>,
    llama_cpp_archs: Vec<String>,
    rejected_archs: HashMap<String, Vec<String>>,
    chat_pipelines: Vec<String>,
    exclude_words: String,
    open_licenses: Vec<String>,
    uncensored: RawUncensored,
    images: RawImages,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawImages {
    uncensored_words: String,
}

#[derive(Deserialize)]
struct RawUncensored {
    queries: Vec<String>,
    words: String,
    exclude: String,
    makers: Vec<String>,
}

#[derive(Deserialize)]
struct RawQuantizer {
    id: String,
    tier: u8,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFamily {
    id: String,
    label: String,
    #[serde(rename = "match")]
    pattern: String,
    base_orgs: Vec<String>,
    archs: Vec<String>,
}

/// Famille de modèles.
pub struct Family {
    pub id: String,
    pub label: String,
    /// Motif cherché dans « identifiant + modèle de base », en minuscules.
    pattern: Regex,
    /// Organisations d'origine, en minuscules.
    base_orgs: Vec<String>,
    archs: Vec<String>,
}

pub struct Curation {
    /// Comptes en minuscules (Hugging Face ne distingue pas la casse).
    labs: HashSet<String>,
    quantizers: HashMap<String, u8>,
    base_labs: HashSet<String>,
    excluded: HashSet<String>,
    pub families: Vec<Family>,
    /// Architectures acceptées, à la casse près (llama.cpp les compare
    /// exactement).
    archs: HashSet<String>,
    chat_pipelines: HashSet<String>,
    exclude_words: Regex,
    open_licenses: HashSet<String>,
    /// Recherches de Hugging Face de la rubrique « Uncensored ».
    pub uncensored_queries: Vec<String>,
    uncensored_words: Regex,
    uncensored_exclude: Regex,
    /// Auteurs de dérivés sans refus faits d'après le modèle officiel, en
    /// minuscules.
    uncensored_makers: HashSet<String>,
    /// Mots d'un modèle d'images non censuré.
    image_uncensored_words: Regex,
}

fn lower(list: Vec<String>) -> HashSet<String> {
    list.into_iter().map(|s| s.to_ascii_lowercase()).collect()
}

impl Curation {
    fn parse(json: &str) -> Result<Self, String> {
        let raw: Raw = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let rejected: HashSet<String> = raw.rejected_archs.into_values().flatten().collect();
        let families = raw
            .families
            .into_iter()
            .map(|family| {
                Ok(Family {
                    pattern: Regex::new(&family.pattern).map_err(|e| e.to_string())?,
                    id: family.id,
                    label: family.label,
                    base_orgs: family
                        .base_orgs
                        .into_iter()
                        .map(|org| org.to_ascii_lowercase())
                        .collect(),
                    archs: family.archs,
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            labs: lower(raw.labs),
            quantizers: raw
                .quantizers
                .into_iter()
                .map(|q| (q.id.to_ascii_lowercase(), q.tier))
                .collect(),
            base_labs: lower(raw.base_labs),
            excluded: lower(raw.excluded),
            families,
            archs: raw
                .llama_cpp_archs
                .into_iter()
                .filter(|arch| !rejected.contains(arch))
                .collect(),
            chat_pipelines: raw.chat_pipelines.into_iter().collect(),
            exclude_words: Regex::new(&format!("(?i){}", raw.exclude_words))
                .map_err(|e| e.to_string())?,
            open_licenses: lower(raw.open_licenses),
            uncensored_queries: raw.uncensored.queries,
            uncensored_words: Regex::new(&format!("(?i){}", raw.uncensored.words))
                .map_err(|e| e.to_string())?,
            uncensored_exclude: Regex::new(&format!("(?i){}", raw.uncensored.exclude))
                .map_err(|e| e.to_string())?,
            uncensored_makers: lower(raw.uncensored.makers),
            image_uncensored_words: Regex::new(&format!("(?i){}", raw.images.uncensored_words))
                .map_err(|e| e.to_string())?,
        })
    }

    /// Confiance affichée, selon le publieur seul : laboratoire, quantificateur
    /// de confiance, sinon non vérifié.
    pub fn trust(&self, author: &str) -> HubTrust {
        let author = author.to_ascii_lowercase();
        if self.excluded.contains(&author) {
            HubTrust::Unverified
        } else if self.labs.contains(&author) {
            HubTrust::Lab
        } else if self.quantizers.contains_key(&author) {
            HubTrust::Quantizer
        } else {
            HubTrust::Unverified
        }
    }

    /// Rang d'un publieur pour choisir le représentant d'un modèle :
    /// laboratoire 0, quantificateur de rang 1 ou 2, autre 3.
    pub fn rank(&self, author: &str) -> u8 {
        let author = author.to_ascii_lowercase();
        if self.labs.contains(&author) {
            0
        } else {
            self.quantizers.get(&author).copied().unwrap_or(3)
        }
    }

    /// Règle 1 de l'accueil (35 § 7) : laboratoire de confiance, ou
    /// quantificateur de confiance dont le modèle de base (relation
    /// `quantized`) vient d'un laboratoire de `baseLabs`.
    pub fn trusted_origin(
        &self,
        author: &str,
        base_model: Option<&str>,
        relation: Option<&str>,
    ) -> bool {
        match self.trust(author) {
            HubTrust::Lab => true,
            HubTrust::Quantizer => {
                relation == Some("quantized")
                    && base_model
                        .and_then(|base| base.split_once('/'))
                        .is_some_and(|(org, _)| self.base_labs.contains(&org.to_ascii_lowercase()))
            }
            HubTrust::Unverified => false,
        }
    }

    /// Architecture que llama.cpp (donc Ollama) sait charger pour converser.
    pub fn accepted_arch(&self, arch: Option<&str>) -> bool {
        arch.is_some_and(|arch| self.archs.contains(arch))
    }

    /// `pipeline_tag` absent ou de conversation.
    pub fn chat_pipeline(&self, pipeline: Option<&str>) -> bool {
        pipeline.is_none_or(|pipeline| self.chat_pipelines.contains(pipeline))
    }

    /// Identifiant qui contient un mot exclu (dérivés « uncensored »...).
    pub fn has_excluded_word(&self, id: &str) -> bool {
        self.exclude_words.is_match(id)
    }

    /// Nom d'un dérivé sans refus (« abliterated », « uncensored »...), ni
    /// pour adultes ni encodeur d'un générateur d'images.
    pub fn uncensored_name(&self, id: &str) -> bool {
        self.uncensored_words.is_match(id) && !self.uncensored_exclude.is_match(id)
    }

    /// Modèle (« org/nom ») d'un laboratoire de `baseLabs`.
    pub fn official_base(&self, model: &str) -> bool {
        model
            .split_once('/')
            .is_some_and(|(org, _)| self.base_labs.contains(&org.to_ascii_lowercase()))
    }

    /// Lignée officielle d'un dérivé sans refus : modèle de base d'un
    /// laboratoire de `baseLabs` (quantifié ou affiné), ou dérivé d'un auteur
    /// de `makers` quantifié par lui-même ou par un quantificateur de
    /// confiance. Sans modèle de base : refusé.
    pub fn uncensored_origin(
        &self,
        author: &str,
        base_model: Option<&str>,
        relation: Option<&str>,
    ) -> bool {
        let Some(base) = base_model else {
            return false;
        };
        let Some((org, _)) = base.split_once('/') else {
            return false;
        };
        let (org, author) = (org.to_ascii_lowercase(), author.to_ascii_lowercase());
        match relation {
            Some("quantized" | "finetune") if self.official_base(base) => true,
            Some("quantized") if self.uncensored_makers.contains(&org) => {
                author == org || self.quantizers.contains_key(&author)
            }
            _ => false,
        }
    }

    /// Modèle d'images non censuré : un mot de `images.uncensoredWords` dans
    /// l'identifiant d'un dépôt (le sien ou celui d'un modèle de base).
    pub fn uncensored_image(&self, id: &str) -> bool {
        self.image_uncensored_words.is_match(id)
    }

    /// Licence libre (filtre « Open license »).
    pub fn open_license(&self, license: Option<&str>) -> bool {
        license.is_some_and(|license| self.open_licenses.contains(&license.to_ascii_lowercase()))
    }

    /// Famille d'un dépôt (`familyMatchOrder`) : 1) organisation d'origine
    /// (`origin` : celle du modèle de base, ou le laboratoire publieur) ;
    /// 2) motif sur « identifiant + modèle de base », familles dans l'ordre ;
    /// 3) architecture. Sinon `None` (« Autres »).
    pub fn family(
        &self,
        id: &str,
        base_model: Option<&str>,
        origin: Option<&str>,
        arch: Option<&str>,
    ) -> Option<&Family> {
        if let Some(origin) = origin.map(str::to_ascii_lowercase)
            && let Some(family) = self.families.iter().find(|f| f.base_orgs.contains(&origin))
        {
            return Some(family);
        }
        let text = format!("{id} {}", base_model.unwrap_or_default()).to_ascii_lowercase();
        if let Some(family) = self.families.iter().find(|f| f.pattern.is_match(&text)) {
            return Some(family);
        }
        let arch = arch?;
        self.families
            .iter()
            .find(|f| f.archs.iter().any(|a| a == arch))
    }

    /// Libellé d'une famille.
    pub fn family_label(&self, id: &str) -> Option<&str> {
        self.families
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.label.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_curation_is_complete() {
        let c = &*CURATION;
        assert!(c.families.iter().all(|f| f.id != "embeddings"));
        assert_eq!(c.families.len(), 14);
        // 152 architectures de llama.cpp b11232, moins les 24 refusées.
        assert_eq!(c.archs.len(), 152 - 24);
        for arch in [
            "qwen3",
            "qwen35",
            "gemma4",
            "llama",
            "gpt-oss",
            "pangu-embedded",
        ] {
            assert!(c.accepted_arch(Some(arch)), "{arch}");
        }
        for arch in [
            "clip",
            "bert",
            "nomic-bert",
            "llama-embed",
            "dream",
            "llada",
            "t5",
            "t5encoder",
            "gptj",
            "eagle3",
            "qwen3tts",
            "xing4_0",
            "Qwen3",
        ] {
            assert!(!c.accepted_arch(Some(arch)), "{arch}");
        }
        assert!(!c.accepted_arch(None));
    }

    #[test]
    fn uncensored_image_models_are_told_by_their_words() {
        let c = &*CURATION;
        for id in [
            "someone/Z-Image-Turbo-NSFW-GGUF",
            "a/flux-uncensored",
            "b/Lewd-Diffusion",
            "c/realistic-nude-xl",
        ] {
            assert!(c.uncensored_image(id), "{id}");
        }
        for id in ["leejet/Z-Image-Turbo-GGUF", "unsloth/FLUX.2-klein-4B-GGUF"] {
            assert!(!c.uncensored_image(id), "{id}");
        }
    }

    #[test]
    fn trust_depends_on_the_publisher_and_the_base() {
        let c = &*CURATION;
        assert_eq!(c.trust("Qwen"), HubTrust::Lab);
        assert_eq!(c.trust("qwen"), HubTrust::Lab);
        assert_eq!(c.trust("unsloth"), HubTrust::Quantizer);
        assert_eq!(c.trust("mradermacher"), HubTrust::Quantizer);
        assert_eq!(c.trust("TheBloke"), HubTrust::Unverified);
        assert_eq!(c.trust("ornith-ai"), HubTrust::Unverified);
        assert_eq!(c.rank("google"), 0);
        assert_eq!(c.rank("bartowski"), 1);
        assert_eq!(c.rank("MaziyarPanahi"), 2);
        assert_eq!(c.rank("someone"), 3);

        assert!(c.trusted_origin("google", None, None));
        assert!(c.trusted_origin("unsloth", Some("Qwen/Qwen3-8B"), Some("quantized")));
        assert!(c.trusted_origin(
            "mradermacher",
            Some("deepseek-ai/DeepSeek-R1"),
            Some("quantized")
        ));
        // Base communautaire, autre relation, pas de base : refusés.
        assert!(!c.trusted_origin(
            "mradermacher",
            Some("huihui-ai/Qwen3-VL-8B-abliterated"),
            Some("quantized")
        ));
        assert!(!c.trusted_origin("unsloth", Some("Qwen/Qwen3-8B"), Some("finetune")));
        assert!(!c.trusted_origin("unsloth", None, None));
        assert!(!c.trusted_origin("ornith-ai", Some("Qwen/Qwen3-8B"), Some("quantized")));
    }

    #[test]
    fn words_pipelines_and_licenses() {
        let c = &*CURATION;
        for id in [
            "huihui-ai/Qwen3-8B-abliterated-GGUF",
            "x/Model-Uncensored-GGUF",
            "x/model-rp-GGUF",
            "x/Model_RP",
            "x/HERETIC-GGUF",
        ] {
            assert!(c.has_excluded_word(id), "{id}");
        }
        for id in ["unsloth/Qwen3-8B-GGUF", "x/sharpener-GGUF", "x/grpo-GGUF"] {
            assert!(!c.has_excluded_word(id), "{id}");
        }
        assert!(c.chat_pipeline(None));
        assert!(c.chat_pipeline(Some("image-text-to-text")));
        assert!(!c.chat_pipeline(Some("automatic-speech-recognition")));
        assert!(!c.chat_pipeline(Some("feature-extraction")));
        assert!(c.open_license(Some("Apache-2.0")));
        assert!(c.open_license(Some("mit")));
        assert!(!c.open_license(Some("llama3.1")));
        assert!(!c.open_license(None));
    }

    #[test]
    fn uncensored_names_and_origins() {
        let c = &*CURATION;
        assert_eq!(
            c.uncensored_queries,
            ["uncensored", "abliterated", "heretic"]
        );
        for id in [
            "huihui-ai/Huihui-Qwen3.8-27B-abliterated-GGUF",
            "x/Qwen3.5-9B-Uncensored-HauhauCS-Aggressive",
            "llmfan46/gemma-4-31B-it-uncensored-heretic-GGUF",
            "x/Model-OBLITERATED-GGUF",
        ] {
            assert!(c.uncensored_name(id), "{id}");
        }
        for id in [
            "unsloth/Qwen3-8B-GGUF",
            "x/Qwen3.8-27B-Heretic-JP-Roleplay-NSFW-i1-GGUF",
            "x/Model-uncensored-RP-GGUF",
            "x/Model-abliterated-erotic-GGUF",
            "pottokao/MiniMax-H3-TextEncoder-Qwen3VL-32B-abliterated-GGUF",
            "pottokao/Qwen-Image-2.1-Text-Encoder-Heretic-GGUF",
            "BennyDaBall/Qwen3-4b-Z-Image-Turbo-AbliteratedV1",
        ] {
            assert!(!c.uncensored_name(id), "{id}");
        }

        // Base d'un laboratoire : tout publieur.
        assert!(c.uncensored_origin("huihui-ai", Some("Qwen/Qwen3.8-27B"), Some("quantized")));
        assert!(c.uncensored_origin("HauhauCS", Some("google/gemma-4-12B-it"), Some("finetune")));
        // Base d'un auteur de dérivés : lui-même ou un quantificateur de confiance.
        let base = Some("huihui-ai/Huihui-Qwen3.5-9B-abliterated");
        assert!(c.uncensored_origin("mradermacher", base, Some("quantized")));
        assert!(c.uncensored_origin("bartowski", base, Some("quantized")));
        assert!(c.uncensored_origin(
            "llmfan46",
            Some("llmfan46/gemma-4-31B-it-uncensored-heretic"),
            Some("quantized")
        ));
        assert!(!c.uncensored_origin("someone", base, Some("quantized")));
        assert!(!c.uncensored_origin("mradermacher", base, Some("finetune")));
        // Fusion communautaire, base inconnue, sans base, autre relation.
        assert!(!c.uncensored_origin(
            "DavidAU",
            Some("DavidAU/Qwen3.5-9B-The-Defiant-Fable-Uncensored-Heretic"),
            Some("quantized")
        ));
        assert!(!c.uncensored_origin(
            "huihui-ai",
            Some("oyildirim/CyberStrike-OffSec-35B"),
            Some("quantized")
        ));
        assert!(!c.uncensored_origin("HauhauCS", None, None));
        assert!(!c.uncensored_origin("x", Some("Qwen/Qwen3-8B"), Some("adapter")));
        assert!(!c.uncensored_origin("x", Some("Qwen/Qwen3-8B"), Some("merge")));
    }

    #[test]
    fn families_follow_the_match_order() {
        let c = &*CURATION;
        let family =
            |id, base, origin, arch| c.family(id, base, origin, arch).map(|f| f.id.as_str());
        // 1) Organisation d'origine : une distillation R1 de Qwen est DeepSeek.
        assert_eq!(
            family(
                "unsloth/DeepSeek-R1-0528-Qwen3-8B-GGUF",
                Some("deepseek-ai/DeepSeek-R1-0528-Qwen3-8B"),
                Some("deepseek-ai"),
                Some("qwen3")
            ),
            Some("deepseek")
        );
        assert_eq!(
            family("Qwen/Qwen3-8B-GGUF", None, Some("Qwen"), Some("qwen3")),
            Some("qwen")
        );
        // 2) Motif.
        assert_eq!(
            family(
                "bartowski/Mistral-Small-GGUF",
                Some("someone/x"),
                Some("someone"),
                Some("llama")
            ),
            Some("mistral")
        );
        assert_eq!(
            family("x/Phi-4-mini-GGUF", None, None, Some("phi3")),
            Some("phi")
        );
        assert_eq!(family("x/delphi-2-GGUF", None, None, None), None);
        // 3) Architecture.
        assert_eq!(
            family("x/Ornith-1.5-9B-GGUF", None, None, Some("qwen35")),
            Some("qwen")
        );
        assert_eq!(
            family("x/Model-GGUF", None, None, Some("llama")),
            Some("llama")
        );
        assert_eq!(family("x/Model-GGUF", None, None, Some("rwkv7")), None);
        assert_eq!(c.family_label("lfm"), Some("LFM (Liquid AI)"));
    }
}
