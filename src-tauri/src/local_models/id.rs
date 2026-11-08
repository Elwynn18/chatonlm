//! Identifiant d'un modèle de la base locale (`ModelId`, contrat 100 § 1) :
//! `<auteur>/<dépôt>:<QUANT>` pour Hugging Face (la forme `hf.co/…` d'Ollama
//! sans son hôte), `@ollama/<nom>:<étiquette>` pour un modèle importé d'un
//! Ollama sans équivalent sur Hugging Face (un dépôt de Hugging Face ne
//! commence jamais par `@`). Comparaison exacte, sauf la quantification,
//! comparée sans la casse.

/// Préfixe des modèles importés d'un Ollama.
pub const OLLAMA_PREFIX: &str = "@ollama/";

/// Longueur maximale d'un auteur ou d'un dépôt de Hugging Face.
const MAX_PART: usize = 96;

/// Longueur maximale d'une quantification (un nom de fichier sans `.gguf`
/// quand le libellé seul serait ambigu).
const MAX_QUANT: usize = 128;

/// Identifiant décomposé.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelId {
    /// Version d'un dépôt de Hugging Face.
    Hf {
        author: String,
        repo: String,
        quant: String,
    },
    /// Modèle importé d'un Ollama : nom (« qwen3 », « library/qwen3 ») et
    /// étiquette.
    Ollama { name: String, tag: String },
}

impl ModelId {
    /// Décompose un identifiant ; `None` s'il est mal formé.
    pub fn parse(id: &str) -> Option<Self> {
        let (path, quant) = id.rsplit_once(':')?;
        if let Some(name) = path.strip_prefix(OLLAMA_PREFIX) {
            let valid_name = !name.is_empty()
                && name.len() <= 2 * MAX_PART
                && name.split('/').all(|part| valid_part(part, MAX_PART));
            return (valid_name && valid_part(quant, MAX_QUANT)).then(|| ModelId::Ollama {
                name: name.to_owned(),
                tag: quant.to_owned(),
            });
        }
        let (author, repo) = path.split_once('/')?;
        (valid_part(author, MAX_PART) && valid_part(repo, MAX_PART) && valid_part(quant, MAX_QUANT))
            .then(|| ModelId::Hf {
                author: author.to_owned(),
                repo: repo.to_owned(),
                quant: quant.to_owned(),
            })
    }

    /// Identifiant d'une demande : la forme d'Ollama `hf.co/<a>/<d>:<q>` (ou
    /// `huggingface.co/…`, hôte sans la casse) est acceptée et ramenée à
    /// `<a>/<d>:<q>`.
    pub fn from_request(text: &str) -> Option<Self> {
        let text = text.trim();
        let bare = text
            .split_once('/')
            .filter(|(host, _)| {
                host.eq_ignore_ascii_case("hf.co") || host.eq_ignore_ascii_case("huggingface.co")
            })
            .map_or(text, |(_, rest)| rest);
        Self::parse(bare)
    }

    /// Identifiant d'une version d'un dépôt de Hugging Face (« auteur/dépôt »),
    /// `None` s'il serait mal formé.
    pub fn hf(repo_id: &str, quant: &str) -> Option<Self> {
        Self::parse(&format!("{repo_id}:{quant}")).filter(|id| matches!(id, ModelId::Hf { .. }))
    }

    /// « auteur/dépôt » d'un modèle de Hugging Face.
    pub fn repo_id(&self) -> Option<String> {
        match self {
            ModelId::Hf { author, repo, .. } => Some(format!("{author}/{repo}")),
            ModelId::Ollama { .. } => None,
        }
    }

    /// Quantification (ou étiquette d'un importé).
    pub fn quant(&self) -> &str {
        match self {
            ModelId::Hf { quant, .. } => quant,
            ModelId::Ollama { tag, .. } => tag,
        }
    }
}

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelId::Hf {
                author,
                repo,
                quant,
            } => write!(f, "{author}/{repo}:{quant}"),
            ModelId::Ollama { name, tag } => write!(f, "{OLLAMA_PREFIX}{name}:{tag}"),
        }
    }
}

/// Même modèle : tout pareil sauf la casse de la quantification. Deux textes
/// mal formés ne sont égaux que s'ils sont identiques.
pub fn same_model(a: &str, b: &str) -> bool {
    match (a.rsplit_once(':'), b.rsplit_once(':')) {
        (Some((path_a, quant_a)), Some((path_b, quant_b))) => {
            path_a == path_b && quant_a.eq_ignore_ascii_case(quant_b)
        }
        _ => a == b,
    }
}

/// Partie d'un identifiant : 1 à `max` caractères ASCII, lettres, chiffres,
/// `-`, `_` ou `.`, ni `.` ni `-` au début, ni `..` (jamais un chemin qui
/// remonte : chaque partie devient un dossier du rangement), et un nom que
/// Windows accepte pour un dossier (ni `CON`, `AUX`, `NUL`, `COM1`…, ni `.`
/// final), sous Linux aussi : un identifiant vaut sur les deux systèmes.
fn valid_part(part: &str, max: usize) -> bool {
    let bytes = part.as_bytes();
    (1..=max).contains(&bytes.len())
        && bytes
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        && !matches!(bytes[0], b'.' | b'-')
        && !part.contains("..")
        && crate::image_models::downloads::safe_name(part)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hugging_face_ids_are_split_and_rewritten() {
        let id = ModelId::parse("unsloth/Qwen3-0.6B-GGUF:Q4_K_M").unwrap();
        assert_eq!(
            id,
            ModelId::Hf {
                author: "unsloth".into(),
                repo: "Qwen3-0.6B-GGUF".into(),
                quant: "Q4_K_M".into(),
            }
        );
        assert_eq!(id.to_string(), "unsloth/Qwen3-0.6B-GGUF:Q4_K_M");
        assert_eq!(id.repo_id().as_deref(), Some("unsloth/Qwen3-0.6B-GGUF"));
        assert_eq!(id.quant(), "Q4_K_M");
        // Forme d'Ollama acceptée dans une demande, sans son hôte.
        for request in [
            "hf.co/unsloth/Qwen3-0.6B-GGUF:Q4_K_M",
            " HF.CO/unsloth/Qwen3-0.6B-GGUF:Q4_K_M ",
            "huggingface.co/unsloth/Qwen3-0.6B-GGUF:Q4_K_M",
        ] {
            assert_eq!(
                ModelId::from_request(request).as_ref(),
                Some(&id),
                "{request}"
            );
        }
        // Quantification écrite comme le nom du fichier.
        assert!(ModelId::parse("bartowski/Qwen3.8-27B-GGUF:Qwen3.8-27B-Q4_K_L").is_some());
        assert!(ModelId::hf("unsloth/Qwen3-8B-GGUF", "UD-Q4_K_XL").is_some());
    }

    #[test]
    fn imported_models_start_with_an_at_sign() {
        let id = ModelId::parse("@ollama/library/qwen3:0.6b").unwrap();
        assert_eq!(
            id,
            ModelId::Ollama {
                name: "library/qwen3".into(),
                tag: "0.6b".into()
            }
        );
        assert_eq!(id.to_string(), "@ollama/library/qwen3:0.6b");
        assert_eq!(id.repo_id(), None);
        // Jamais un modèle de Hugging Face.
        assert_eq!(ModelId::hf("@ollama/qwen3", "0.6b"), None);
    }

    #[test]
    fn malformed_ids_are_refused() {
        let long = "x".repeat(97);
        for bad in [
            "",
            "qwen3:0.6b",
            "a/b",
            "a/b:",
            ":Q4_K_M",
            "a/:Q4_K_M",
            "/b:Q4_K_M",
            "a/b/c:Q4_K_M",
            "../b:Q4_K_M",
            "a/..:Q4_K_M",
            "a/b:..",
            "a/b:.hidden",
            "a/-b:Q4",
            "a/b c:Q4",
            "a/b:Q4/K",
            "a/b:Q4\\K",
            "a/b€:Q4",
            "@ollama/:latest",
            "@ollama/a//b:latest",
            "@ollama/../x:latest",
            "@other/x:latest",
            // Noms réservés de Windows, même sous Linux.
            "con/b:Q4",
            "a/AUX:Q4",
            "a/nul.x:Q4",
            "a/com1:Q4",
            "@ollama/lpt9:latest",
            "a/b.:Q4",
            &format!("{long}/b:Q4"),
            &format!("a/b:{}", "Q".repeat(129)),
        ] {
            assert_eq!(ModelId::parse(bad), None, "{bad}");
        }
        assert!(ModelId::parse(&format!("{}/b:Q4", "x".repeat(96))).is_some());
        // Un autre hôte n'est pas retiré.
        assert_eq!(ModelId::from_request("ollama.com/a/b:Q4"), None);
    }

    #[test]
    fn only_the_quantization_ignores_case() {
        assert!(same_model("a/B-GGUF:q4_k_m", "a/B-GGUF:Q4_K_M"));
        assert!(!same_model("a/b-gguf:Q4_K_M", "a/B-GGUF:Q4_K_M"));
        assert!(!same_model("A/b:Q4", "a/b:Q4"));
        assert!(!same_model("a/b:Q4_K_M", "a/b:Q4_K_S"));
        assert!(same_model("odd", "odd"));
        assert!(!same_model("odd", "ODD"));
    }
}
