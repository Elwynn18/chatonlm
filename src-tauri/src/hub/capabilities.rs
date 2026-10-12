//! Capacités annoncées par un dépôt (35 § 6.1), déduites du gabarit de
//! conversation (`gguf.chat_template`), des fichiers et des noms. Les vraies
//! capacités se lisent dans `/api/show` après installation.

use std::sync::LazyLock;

use regex::Regex;

use super::types::HubCapabilities;

/// Outils, règle stricte : la version large (`\btools\b`) acceptait les
/// gabarits Phi-4-mini qui lisent les outils dans le message système.
static TOOLS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\bif\s+(?:not\s+)?tools\b|\btools\s+is\s+(?:not\s+)?(?:defined|none|iterable|sequence)|\bfor\s+\w+\s+in\s+tools\b|\btools\s*\|\s*(?:length|tojson|items|list)|\btools\s*\[|\btools\s*(?:and|or)\b",
    )
    .expect("valid tools pattern")
});

/// Réflexion (a) : un interrupteur dans le gabarit.
static THINK_SWITCH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"enable_thinking|reasoning_effort|thinking_budget|<\|channel\|>|\[THINK\]|<seed:think>",
    )
    .expect("valid thinking pattern")
});

/// Réflexion (c) : le gabarit nettoie un raisonnement...
static THINK_CLEANUP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"</think>|reasoning_content|message\.thinking").expect("valid cleanup pattern")
});

/// ... et le nom l'annonce (mot délimité).
static THINK_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:^|[^a-z0-9])(?:r1|qwq|thinking|reasoning|think|z1|magistral)(?:[^a-z0-9]|$)",
    )
    .expect("valid name pattern")
});

/// Code : dans l'identifiant, le modèle de base ou les tags.
static CODE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)coder|code|codestral|devstral|starcoder").expect("valid code pattern")
});

/// Caractères examinés après le dernier `add_generation_prompt`.
const AFTER_PROMPT: usize = 400;

/// Ce qu'il faut pour déduire les capacités d'un dépôt.
pub struct Facts<'a> {
    pub template: Option<&'a str>,
    /// Identifiant du dépôt.
    pub id: &'a str,
    pub base_model: Option<&'a str>,
    pub tags: &'a [String],
    /// Le dépôt contient un projecteur de vision (`mmproj`).
    pub projector: bool,
}

pub fn capabilities(facts: &Facts) -> HubCapabilities {
    let template = facts.template.unwrap_or_default();
    let names = format!("{} {}", facts.id, facts.base_model.unwrap_or_default());
    HubCapabilities {
        tools: TOOLS_RE.is_match(template),
        thinking: thinks(template, &names),
        vision: facts.projector,
        code: CODE_RE.is_match(&names) || facts.tags.iter().any(|tag| CODE_RE.is_match(tag)),
    }
}

/// Règles (a), (b) et (c) de la réflexion.
fn thinks(template: &str, names: &str) -> bool {
    if THINK_SWITCH_RE.is_match(template) {
        return true;
    }
    if let Some(index) = template.rfind("add_generation_prompt") {
        let after: String = template[index..].chars().take(AFTER_PROMPT).collect();
        if after.contains("<think>") {
            return true;
        }
    }
    THINK_CLEANUP_RE.is_match(template) && THINK_NAME_RE.is_match(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(template: &str, id: &str, base: Option<&str>) -> HubCapabilities {
        capabilities(&Facts {
            template: Some(template),
            id,
            base_model: base,
            tags: &[],
            projector: false,
        })
    }

    #[test]
    fn tools_need_a_real_use_of_the_list() {
        for template in [
            "{%- if tools %}…{%- endif %}",
            "{% if not tools %}x{% endif %}",
            "{%- if tools is defined and tools %}",
            "{% for tool in tools %}{{ tool | tojson }}{% endfor %}",
            "{{- tools | tojson }}",
            "{%- set n = tools|length %}",
            "{{ tools[0] }}",
            "{% if tools and messages %}",
        ] {
            assert!(caps(template, "x/y", None).tools, "{template}");
        }
        // Phi-4-mini : outils lus dans le message, pas dans la liste.
        for template in [
            "{% if 'tools' in message %}{{ message['tools'] }}{% endif %}",
            "",
        ] {
            assert!(!caps(template, "x/y", None).tools, "{template}");
        }
    }

    #[test]
    fn thinking_rules() {
        // (a) Interrupteurs.
        for template in [
            "{%- if enable_thinking is defined %}",
            "Reasoning: {{ reasoning_effort }}",
            "<|channel|>analysis",
            "[THINK]",
            "<seed:think>",
        ] {
            assert!(caps(template, "x/y", None).thinking, "{template}");
        }
        // (b) <think> juste après le début de la réponse.
        let qwen3 = "{%- if add_generation_prompt %}{{- '<|im_start|>assistant\\n<think>\\n' }}{%- endif %}";
        assert!(caps(qwen3, "Qwen/Qwen3-8B-GGUF", None).thinking);
        // <think> ailleurs (nettoyage de l'historique) : un Instruct-2507 ne
        // raisonne pas.
        let instruct = format!(
            "{{%- if '</think>' in content %}}{{%- set content = content.split('</think>')[-1] %}}{{%- endif %}}{}{{%- if add_generation_prompt %}}<|im_start|>assistant\\n{{%- endif %}}",
            " ".repeat(10)
        );
        assert!(!caps(&instruct, "unsloth/Qwen3-30B-A3B-Instruct-2507-GGUF", None).thinking);
        // <think> trop loin après add_generation_prompt.
        let far = format!(
            "{{%- if add_generation_prompt %}}{}<think>",
            "x".repeat(AFTER_PROMPT)
        );
        assert!(!caps(&far, "x/y", None).thinking);
        // (c) Nettoyage + nom qui annonce un raisonnement (distillation R1).
        let cleanup = "{% set content = content.split('</think>')[-1] %}";
        assert!(
            caps(
                cleanup,
                "unsloth/DeepSeek-R1-0528-Qwen3-8B-GGUF",
                Some("deepseek-ai/DeepSeek-R1-0528-Qwen3-8B")
            )
            .thinking
        );
        assert!(caps(cleanup, "x/y-GGUF", Some("Qwen/QwQ-32B")).thinking);
        assert!(!caps(cleanup, "x/Model-GGUF", None).thinking);
        // Mot non délimité : pas une annonce.
        assert!(!caps(cleanup, "x/Overthinker-GGUF", None).thinking);
        // Nom seul, sans nettoyage : non.
        assert!(!caps("{{ messages }}", "x/Thinking-GGUF", None).thinking);
    }

    #[test]
    fn vision_comes_from_the_projector_and_code_from_names() {
        let facts = Facts {
            template: None,
            id: "mradermacher/Qwen3-VL-8B-Instruct-abliterated-GGUF",
            base_model: None,
            tags: &[],
            projector: true,
        };
        let found = capabilities(&facts);
        assert!(found.vision && !found.tools && !found.thinking && !found.code);
        assert!(caps("", "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF", None).code);
        assert!(caps("", "x/y-GGUF", Some("mistralai/Devstral-Small-2")).code);
        let tags = vec!["code".to_owned()];
        let tagged = capabilities(&Facts {
            template: None,
            id: "x/y",
            base_model: None,
            tags: &tags,
            projector: false,
        });
        assert!(tagged.code);
        assert!(!caps("", "Qwen/Qwen3-8B-GGUF", None).code);
    }
}
