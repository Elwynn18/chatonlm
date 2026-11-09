//! Noms des modèles d'un Ollama (logique pure) : nom d'après le chemin d'un
//! manifeste (`manifests/<hôte>/<espace>/<modèle>/<étiquette>`, ou
//! `manifests-v2/…` d'Ollama 0.40), clé de comparaison des noms écrits dans
//! la base de ChatonLM 0.9.x, identifiant de la base locale qui le remplace
//! (contrat 100 § 1) : `hf.co/<a>/<d>:<q>` → `<a>/<d>:<Q>`, `qwen3:0.6b` →
//! `@ollama/qwen3:0.6b`, `user/model:tag` → `@ollama/user/model:tag`.

use crate::local_models::id::ModelId;

/// Hôtes du registre d'Ollama (ancien et nouveau nom).
const OLLAMA_HOSTS: [&str; 2] = ["registry.ollama.ai", "ollama.com"];

/// Hôtes de Hugging Face.
const HF_HOSTS: [&str; 2] = ["hf.co", "huggingface.co"];

/// Nom d'un modèle d'après le chemin de son manifeste.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ManifestName {
    pub host: String,
    pub namespace: String,
    pub model: String,
    pub tag: String,
}

impl ManifestName {
    /// Nom d'après les quatre parties du chemin sous `manifests/` (ou
    /// `manifests-v2/`) ; `None` pour une autre profondeur ou une partie vide.
    pub fn from_parts(parts: &[&str]) -> Option<Self> {
        let [host, namespace, model, tag] = parts else {
            return None;
        };
        // L'hôte peut porter un port ; les autres parties, jamais de `:`.
        if parts.iter().any(|part| part.is_empty())
            || [namespace, model, tag]
                .iter()
                .any(|part| part.contains(':'))
        {
            return None;
        }
        Some(Self {
            host: (*host).to_owned(),
            namespace: (*namespace).to_owned(),
            model: (*model).to_owned(),
            tag: (*tag).to_owned(),
        })
    }

    /// Le registre d'Ollama (`registry.ollama.ai`, `ollama.com`).
    pub fn is_ollama_registry(&self) -> bool {
        OLLAMA_HOSTS
            .iter()
            .any(|host| self.host.eq_ignore_ascii_case(host))
    }

    /// Hugging Face (`hf.co`, `huggingface.co`).
    pub fn is_hf(&self) -> bool {
        HF_HOSTS
            .iter()
            .any(|host| self.host.eq_ignore_ascii_case(host))
    }

    /// Variante `runner: llamacpp` d'une liste de manifestes, rangée par
    /// Ollama 0.40 sous `library/llamacpp/<empreinte>` : pas un modèle à
    /// montrer (la liste qui la cite l'est).
    pub fn is_internal_variant(&self) -> bool {
        self.is_ollama_registry()
            && self.namespace == "library"
            && self.model == "llamacpp"
            && is_digest_hex(&self.tag)
    }

    /// Nom tel qu'Ollama l'écrit : `qwen3:0.6b` (bibliothèque),
    /// `user/model:tag` (autre espace du registre), `hf.co/user/repo:tag`.
    pub fn ollama_name(&self) -> String {
        let Self {
            host,
            namespace,
            model,
            tag,
        } = self;
        if self.is_ollama_registry() {
            if namespace == "library" {
                format!("{model}:{tag}")
            } else {
                format!("{namespace}/{model}:{tag}")
            }
        } else {
            format!("{host}/{namespace}/{model}:{tag}")
        }
    }

    /// Clé de comparaison (voir [`ollama_key`]).
    pub fn key(&self) -> String {
        ollama_key(&self.ollama_name())
    }

    /// Identifiant de la base locale ; `quant` : quantification d'un modèle
    /// de Hugging Face quand l'étiquette ne la dit pas (`latest` : lue dans
    /// le fichier). `None` si le nom ne se range pas
    /// (caractère refusé, nom réservé de Windows…).
    pub fn target(&self, quant: Option<&str>) -> Option<ModelId> {
        if self.is_hf() {
            let quant = hf_tag_quant(&self.tag).or_else(|| quant.map(str::to_owned))?;
            return ModelId::hf(&format!("{}/{}", self.namespace, self.model), &quant);
        }
        let name = if self.is_ollama_registry() {
            if self.namespace == "library" {
                self.model.clone()
            } else {
                format!("{}/{}", self.namespace, self.model)
            }
        } else {
            // Registre privé : son hôte en tête (un hôte avec un port ne se
            // range pas).
            format!("{}/{}/{}", self.host, self.namespace, self.model)
        };
        ModelId::parse(&format!(
            "{}{name}:{}",
            crate::local_models::id::OLLAMA_PREFIX,
            self.tag
        ))
        .filter(|id| matches!(id, ModelId::Ollama { .. }))
    }
}

/// Quantification d'une étiquette `hf.co/…:<tag>` : l'étiquette en
/// majuscules (comme les noms de fichiers), sauf `latest` (à lire ailleurs).
pub fn hf_tag_quant(tag: &str) -> Option<String> {
    (!tag.eq_ignore_ascii_case("latest")).then(|| tag.to_ascii_uppercase())
}

/// 64 chiffres hexadécimaux en minuscules.
pub fn is_digest_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Clé de comparaison d'un nom de modèle d'Ollama, tel qu'il est écrit dans
/// les réglages de ChatonLM 0.9.x : minuscules (Ollama ne distingue pas la
/// casse), sans hôte du registre d'Ollama ni espace `library/`,
/// `huggingface.co/` ramené à `hf.co/`, `:latest` ajouté sans étiquette.
pub fn ollama_key(name: &str) -> String {
    let mut name = name.trim().to_ascii_lowercase();
    for host in OLLAMA_HOSTS {
        if let Some(rest) = name.strip_prefix(&format!("{host}/")) {
            name = rest.to_owned();
            break;
        }
    }
    if let Some(rest) = name.strip_prefix("library/") {
        name = rest.to_owned();
    }
    if let Some(rest) = name.strip_prefix("huggingface.co/") {
        name = format!("hf.co/{rest}");
    }
    let last = name.rsplit('/').next().unwrap_or(&name);
    if !last.contains(':') {
        name.push_str(":latest");
    }
    name
}

/// Nom de la consigne d'un type de fichier GGUF (`general.file_type`,
/// `llama_ftype` de llama.cpp), pour un modèle `hf.co/…:latest` dont le nom
/// ne dit pas la quantification.
pub fn file_type_label(file_type: u32) -> Option<&'static str> {
    Some(match file_type {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        31 => "IQ1_M",
        32 => "BF16",
        36 => "TQ1_0",
        37 => "TQ2_0",
        38 => "MXFP4",
        _ => return None,
    })
}

/// Équivalent sur Hugging Face d'un modèle de la bibliothèque d'Ollama dont
/// le fichier est au format propre à Ollama (vision fondue dans les poids) :
/// dépôts GGUF publics vérifiés le 8 nov. 2026 (fiche lue, non restreints),
/// en Q4_K_M comme les étiquettes d'Ollama. `None` : pas d'équivalent connu.
pub fn hf_equivalent(name: &ManifestName) -> Option<&'static str> {
    if !name.is_ollama_registry() || name.namespace != "library" {
        return None;
    }
    // Taille en tête de l'étiquette (« 4b », « 4b-it-qat », « 4b-it-q4_K_M »).
    let tag = name.tag.to_ascii_lowercase();
    let size = tag.split('-').next().unwrap_or(&tag);
    Some(match (name.model.as_str(), size) {
        ("gemma3", "4b" | "latest") => "ggml-org/gemma-3-4b-it-GGUF:Q4_K_M",
        ("gemma3", "12b") => "ggml-org/gemma-3-12b-it-GGUF:Q4_K_M",
        ("gemma3", "27b") => "ggml-org/gemma-3-27b-it-GGUF:Q4_K_M",
        ("gemma3", "1b") => "ggml-org/gemma-3-1b-it-GGUF:Q4_K_M",
        ("ministral-3", "3b") => "mistralai/Ministral-3-3B-Instruct-2512-GGUF:Q4_K_M",
        ("ministral-3", "8b" | "latest") => "mistralai/Ministral-3-8B-Instruct-2512-GGUF:Q4_K_M",
        ("ministral-3", "14b") => "mistralai/Ministral-3-14B-Instruct-2512-GGUF:Q4_K_M",
        ("qwen3.5", "0.8b") => "unsloth/Qwen3.5-0.8B-GGUF:Q4_K_M",
        ("qwen3.5", "2b") => "unsloth/Qwen3.5-2B-GGUF:Q4_K_M",
        ("qwen3.5", "4b") => "unsloth/Qwen3.5-4B-GGUF:Q4_K_M",
        ("qwen3.5", "9b" | "latest") => "unsloth/Qwen3.5-9B-GGUF:Q4_K_M",
        ("qwen3-vl", "2b") => "Qwen/Qwen3-VL-2B-Instruct-GGUF:Q4_K_M",
        ("qwen3-vl", "4b") => "Qwen/Qwen3-VL-4B-Instruct-GGUF:Q4_K_M",
        ("qwen3-vl", "8b" | "latest") => "Qwen/Qwen3-VL-8B-Instruct-GGUF:Q4_K_M",
        ("qwen2.5vl", "3b") => "ggml-org/Qwen2.5-VL-3B-Instruct-GGUF:Q4_K_M",
        ("qwen2.5vl", "7b" | "latest") => "ggml-org/Qwen2.5-VL-7B-Instruct-GGUF:Q4_K_M",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(path: &str) -> ManifestName {
        let parts: Vec<&str> = path.split('/').collect();
        ManifestName::from_parts(&parts).unwrap()
    }

    #[test]
    fn manifest_paths_give_ollama_names_and_ids() {
        let library = name("registry.ollama.ai/library/qwen3/0.6b");
        assert_eq!(library.ollama_name(), "qwen3:0.6b");
        assert_eq!(
            library.target(None).unwrap().to_string(),
            "@ollama/qwen3:0.6b"
        );
        // Nouveau rangement d'Ollama 0.40 : même nom.
        let v2 = name("ollama.com/library/qwen3/0.6b");
        assert_eq!(v2.ollama_name(), "qwen3:0.6b");
        assert_eq!(v2.key(), library.key());
        let user = name("registry.ollama.ai/someone/tiny/q4");
        assert_eq!(user.ollama_name(), "someone/tiny:q4");
        assert_eq!(
            user.target(None).unwrap().to_string(),
            "@ollama/someone/tiny:q4"
        );
        let hf = name("hf.co/bartowski/SmolLM2-135M-Instruct-GGUF/Q4_K_M");
        assert_eq!(
            hf.ollama_name(),
            "hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M"
        );
        assert_eq!(
            hf.target(None).unwrap().to_string(),
            "bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M"
        );
        // Étiquette en minuscules : quantification en majuscules.
        let lower = name("hf.co/u/R-GGUF/q8_0");
        assert_eq!(lower.target(None).unwrap().to_string(), "u/R-GGUF:Q8_0");
        // `latest` : la quantification vient d'ailleurs.
        let latest = name("hf.co/u/R-GGUF/latest");
        assert_eq!(latest.target(None), None);
        assert_eq!(
            latest.target(Some("Q4_K_M")).unwrap().to_string(),
            "u/R-GGUF:Q4_K_M"
        );
        // L'étiquette passe avant ce que dit le fichier.
        assert_eq!(
            name("hf.co/u/R-GGUF/UD-Q4_K_XL")
                .target(Some("Q4_K_M"))
                .unwrap()
                .to_string(),
            "u/R-GGUF:UD-Q4_K_XL"
        );
        // Registre privé : son hôte en tête ; avec un port : refusé.
        let private = name("registry.example.com/team/model/v1");
        assert_eq!(
            private.target(None).unwrap().to_string(),
            "@ollama/registry.example.com/team/model:v1"
        );
        assert_eq!(name("localhost:5000/team/model/v1").target(None), None);
        // Nom impossible à ranger sous Windows.
        assert_eq!(name("registry.ollama.ai/library/con/1b").target(None), None);
    }

    #[test]
    fn internal_variants_and_bad_paths_are_told_apart() {
        let variant = name(&format!(
            "registry.ollama.ai/library/llamacpp/{}",
            "f1e6f0684a22457e8eb5b09d26041e8171c248c6b252c267a0b35ac5afc24ed6"
        ));
        assert!(variant.is_internal_variant());
        assert!(!name("registry.ollama.ai/library/llamacpp/latest").is_internal_variant());
        assert!(!name("registry.ollama.ai/library/qwen3/0.6b").is_internal_variant());
        assert_eq!(ManifestName::from_parts(&["a", "b", "c"]), None);
        assert_eq!(ManifestName::from_parts(&["a", "b", "c", "d", "e"]), None);
        assert_eq!(ManifestName::from_parts(&["a", "", "c", "d"]), None);
        assert_eq!(ManifestName::from_parts(&["a", "b", "c:x", "d"]), None);
    }

    #[test]
    fn saved_names_compare_like_ollama() {
        for (a, b) in [
            ("qwen3", "qwen3:latest"),
            ("Qwen3:0.6B", "qwen3:0.6b"),
            ("registry.ollama.ai/library/qwen3:0.6b", "qwen3:0.6b"),
            ("ollama.com/library/qwen3:0.6b", "qwen3:0.6b"),
            ("library/qwen3:0.6b", "qwen3:0.6b"),
            ("huggingface.co/u/R-GGUF:Q4_K_M", "hf.co/u/r-gguf:q4_k_m"),
            ("hf.co/u/R-GGUF", "hf.co/u/r-gguf:latest"),
            (" someone/tiny ", "someone/tiny:latest"),
        ] {
            assert_eq!(ollama_key(a), ollama_key(b), "{a} / {b}");
        }
        assert_ne!(ollama_key("qwen3:0.6b"), ollama_key("qwen3:1.7b"));
        assert_ne!(ollama_key("someone/qwen3"), ollama_key("qwen3"));
    }

    #[test]
    fn equivalents_and_file_types() {
        assert_eq!(
            hf_equivalent(&name("registry.ollama.ai/library/gemma3/4b-it-qat")),
            Some("ggml-org/gemma-3-4b-it-GGUF:Q4_K_M")
        );
        assert_eq!(
            hf_equivalent(&name("registry.ollama.ai/library/ministral-3/3b")),
            Some("mistralai/Ministral-3-3B-Instruct-2512-GGUF:Q4_K_M")
        );
        assert_eq!(
            hf_equivalent(&name("registry.ollama.ai/library/llama3.2/3b")),
            None
        );
        assert_eq!(
            hf_equivalent(&name("registry.ollama.ai/someone/gemma3/4b")),
            None
        );
        // Chaque équivalent est un identifiant valide.
        for model in ["gemma3", "ministral-3", "qwen3.5", "qwen3-vl", "qwen2.5vl"] {
            for size in [
                "0.8b", "1b", "2b", "3b", "4b", "7b", "8b", "9b", "12b", "14b", "27b",
            ] {
                let path = format!("registry.ollama.ai/library/{model}/{size}");
                if let Some(id) = hf_equivalent(&name(&path)) {
                    assert!(ModelId::parse(id).is_some(), "{id}");
                }
            }
        }
        assert_eq!(file_type_label(15), Some("Q4_K_M"));
        assert_eq!(file_type_label(7), Some("Q8_0"));
        assert_eq!(file_type_label(4), None);
    }
}
