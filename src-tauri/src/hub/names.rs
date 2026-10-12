//! Noms Ollama des modèles de Hugging Face (`hf.co/<auteur>/<dépôt>:<étiquette>`)
//! et adresses de Hugging Face.
//!
//! Validation reprise d'Ollama (`types/model/name.go`, `isValidPart`) : un
//! nom refusé là donne `invalid model name` avant tout téléchargement.

/// Hôte des noms Ollama de Hugging Face, toujours écrit ainsi par ChatonLM.
pub const HF_HOST: &str = "hf.co";

/// Adresse de Hugging Face par défaut (API, `raw/`, `resolve/`, registre `/v2`).
pub const DEFAULT_HF_URL: &str = "https://huggingface.co";

/// Page d'état de Hugging Face par défaut (`index.json`).
pub const DEFAULT_HF_STATUS_URL: &str = "https://status.huggingface.co";

/// Adresse de Hugging Face : `CHATONLM_HF_URL` si elle est définie et non vide
/// (builds de débogage et d'essai seulement), sinon huggingface.co ; sans `/`
/// final.
pub fn hf_url() -> String {
    url_from(test_override("CHATONLM_HF_URL"), DEFAULT_HF_URL)
}

/// Page d'état : `CHATONLM_HF_STATUS_URL` (débogage et essais), sinon
/// status.huggingface.co.
pub fn hf_status_url() -> String {
    url_from(
        test_override("CHATONLM_HF_STATUS_URL"),
        DEFAULT_HF_STATUS_URL,
    )
}

/// Adresse imposée par les essais (faux Hugging Face), comme `CHATONLM_PKEXEC` :
/// jamais lue par le build de production, où un environnement modifié
/// enverrait sinon tout le ModelHub vers un hôte quelconque.
#[cfg(any(test, debug_assertions, feature = "e2e"))]
fn test_override(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(not(any(test, debug_assertions, feature = "e2e")))]
fn test_override(_name: &str) -> Option<String> {
    None
}

fn url_from(value: Option<String>, default: &str) -> String {
    value
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

/// Nom Ollama d'un modèle de Hugging Face, décomposé.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HfName {
    /// « bartowski ».
    pub author: String,
    /// « SmolLM2-135M-Instruct-GGUF ».
    pub repo: String,
    /// « Q4_K_M » ; `None` : pas d'étiquette (Ollama demanderait `latest`).
    pub tag: Option<String>,
}

impl HfName {
    /// Identifiant du dépôt : « auteur/dépôt ».
    pub fn repo_id(&self) -> String {
        format!("{}/{}", self.author, self.repo)
    }
}

/// Décompose `hf.co/<auteur>/<dépôt>[:<étiquette>]` (`huggingface.co/` aussi,
/// hôte insensible à la casse). `None` pour un autre nom ou un composant
/// qu'Ollama refuserait.
pub fn parse_hf_name(model: &str) -> Option<HfName> {
    let (host, rest) = model.trim().split_once('/')?;
    if !host.eq_ignore_ascii_case(HF_HOST) && !host.eq_ignore_ascii_case("huggingface.co") {
        return None;
    }
    let (path, tag) = match rest.split_once(':') {
        Some((path, tag)) => (path, Some(tag)),
        None => (rest, None),
    };
    let (author, repo) = path.split_once('/')?;
    if !valid_part(Part::Namespace, author) || !valid_part(Part::Model, repo) {
        return None;
    }
    if let Some(tag) = tag
        && !valid_part(Part::Tag, tag)
    {
        return None;
    }
    Some(HfName {
        author: author.to_owned(),
        repo: repo.to_owned(),
        tag: tag.map(str::to_owned),
    })
}

/// Nom Ollama de la version `tag` du dépôt `repo_id` (« auteur/dépôt »),
/// `None` si Ollama le refuserait.
pub fn hf_model_name(repo_id: &str, tag: &str) -> Option<String> {
    let name = format!("{HF_HOST}/{repo_id}:{tag}");
    parse_hf_name(&name).filter(|parsed| parsed.tag.is_some())?;
    Some(name)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Namespace,
    Model,
    Tag,
}

/// `isValidPart` d'Ollama pour l'espace de noms, le modèle et l'étiquette :
/// 1 à 80 caractères, le premier alphanumérique ou `_`, les suivants aussi ou
/// `-`, `.` (pas dans l'espace de noms).
fn valid_part(part: Part, text: &str) -> bool {
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let bytes = text.as_bytes();
    (1..=80).contains(&bytes.len())
        && word(bytes[0])
        && bytes[1..]
            .iter()
            .all(|&c| word(c) || c == b'-' || (c == b'.' && part != Part::Namespace))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hugging_face_names_are_split() {
        assert_eq!(
            parse_hf_name("hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M"),
            Some(HfName {
                author: "bartowski".into(),
                repo: "SmolLM2-135M-Instruct-GGUF".into(),
                tag: Some("Q4_K_M".into()),
            })
        );
        let parsed = parse_hf_name(" HF.CO/Qwen/Qwen3-8B-GGUF ").unwrap();
        assert_eq!(parsed.repo_id(), "Qwen/Qwen3-8B-GGUF");
        assert_eq!(parsed.tag, None);
        assert!(parse_hf_name("huggingface.co/unsloth/Qwen3.5-9B-GGUF:UD-Q4_K_XL").is_some());
        // Nom complet de fichier comme étiquette (points permis).
        assert!(
            parse_hf_name("hf.co/bartowski/Qwen3.8-27B-GGUF:Qwen3.8-27B-Q4_K_L.gguf").is_some()
        );
    }

    #[test]
    fn other_names_and_names_ollama_refuses_are_rejected() {
        for model in [
            "qwen3:0.6b",
            "library/qwen3",
            "registry.ollama.ai/library/qwen3:0.6b",
            "hf.co/onlyauthor",
            "hf.co//repo:tag",
            "hf.co/bart.owski/x:y",
            "hf.co/a/-repo:tag",
            "hf.co/a/repo:-tag",
            "hf.co/a/repo:",
            "hf.co/a/re po:tag",
            "hf.co/a/repo:ta:g",
            "hf.co/a/b/c:tag",
        ] {
            assert_eq!(parse_hf_name(model), None, "{model}");
        }
        let long = "x".repeat(81);
        assert_eq!(parse_hf_name(&format!("hf.co/a/{long}:Q4_K_M")), None);
        assert_eq!(parse_hf_name(&format!("hf.co/a/repo:{long}")), None);
        assert!(parse_hf_name(&format!("hf.co/a/{}:Q4_K_M", "x".repeat(80))).is_some());
    }

    #[test]
    fn model_names_need_a_valid_tag() {
        assert_eq!(
            hf_model_name("unsloth/Qwen3-8B-GGUF", "Q4_K_M").as_deref(),
            Some("hf.co/unsloth/Qwen3-8B-GGUF:Q4_K_M")
        );
        assert_eq!(hf_model_name("unsloth/Qwen3-8B-GGUF", ""), None);
        assert_eq!(hf_model_name("un.sloth/Qwen3-8B-GGUF", "Q4_K_M"), None);
    }

    #[test]
    fn addresses_can_be_replaced() {
        assert_eq!(url_from(None, DEFAULT_HF_URL), DEFAULT_HF_URL);
        assert_eq!(url_from(Some(" ".into()), DEFAULT_HF_URL), DEFAULT_HF_URL);
        assert_eq!(
            url_from(Some("http://127.0.0.1:9/".into()), DEFAULT_HF_URL),
            "http://127.0.0.1:9"
        );
    }
}
