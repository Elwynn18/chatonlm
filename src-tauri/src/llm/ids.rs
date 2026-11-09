//! Identifiants des modèles du sélecteur (contrat 102 § 1.1), fonctions
//! pures : local (`ModelId` de la base locale, `<auteur>/<dépôt>:<QUANT>` ou
//! `@ollama/<nom>:<étiquette>`) ou distant
//! (`@remote/<fournisseur>/<modèle du fournisseur>`).

use crate::local_models::id::{ModelId, same_model};

/// Préfixe des modèles distants.
pub const REMOTE_PREFIX: &str = "@remote/";

/// Longueur maximale de l'identifiant court d'un fournisseur.
const MAX_PROVIDER: usize = 32;
/// Longueur maximale du nom d'un modèle chez son fournisseur (caractères).
const MAX_REMOTE_MODEL: usize = 200;

/// Cible d'un identifiant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Local(ModelId),
    /// `provider` : identifiant court de la ligne de réglage
    /// (`[a-z0-9-]{1,32}`) ; `model` : nom du fournisseur, `/` permis.
    Remote {
        provider: String,
        model: String,
    },
}

/// Décompose un identifiant ; `None` s'il n'est ni local ni distant (ancien
/// nom d'Ollama, texte quelconque).
pub fn parse(id: &str) -> Option<Target> {
    if let Some(rest) = id.strip_prefix(REMOTE_PREFIX) {
        let (provider, model) = rest.split_once('/')?;
        let provider_ok = (1..=MAX_PROVIDER).contains(&provider.len())
            && provider
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
        let length = model.chars().count();
        let model_ok = (1..=MAX_REMOTE_MODEL).contains(&length)
            && model.trim() == model
            && !model.chars().any(char::is_control);
        return (provider_ok && model_ok).then(|| Target::Remote {
            provider: provider.to_owned(),
            model: model.to_owned(),
        });
    }
    ModelId::parse(id).map(Target::Local)
}

/// Identifiant d'un modèle distant.
pub fn remote_id(provider: &str, model: &str) -> String {
    format!("{REMOTE_PREFIX}{provider}/{model}")
}

/// Le modèle tourne chez un fournisseur (identifiant `@remote/…`, même mal
/// formé : il ne part jamais au moteur local).
pub fn is_remote(id: &str) -> bool {
    id.starts_with(REMOTE_PREFIX)
}

/// Même modèle : locaux comparés comme la base locale (quantification sans
/// la casse), distants exactement ; un texte qui n'est ni l'un ni l'autre
/// n'est égal qu'à lui-même.
pub fn same(a: &str, b: &str) -> bool {
    match (parse(a), parse(b)) {
        (Some(Target::Local(_)), Some(Target::Local(_))) => same_model(a, b),
        _ => a == b,
    }
}

/// Clé des préférences par modèle (`model_prefs`, `model_traits`) : un
/// identifiant (avec `@` ou `/`) tel quel ; un ancien nom d'Ollama (ni `@`
/// ni `/`) sans `:latest` (`qwen3:latest` et `qwen3` partagent leurs
/// préférences, comme en 0.9.x). Avant, tout perdait `:latest`, et
/// `@ollama/qwen3:latest` devenait `@ollama/qwen3`, qui ne se relisait plus.
pub fn legacy_key(id: &str) -> String {
    if id.contains('@') || id.contains('/') {
        id.to_owned()
    } else {
        id.strip_suffix(":latest").unwrap_or(id).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_remote_ids_are_told_apart() {
        assert_eq!(
            parse("Qwen/Qwen3-8B-GGUF:Q4_K_M"),
            Some(Target::Local(
                ModelId::parse("Qwen/Qwen3-8B-GGUF:Q4_K_M").unwrap()
            ))
        );
        assert!(matches!(
            parse("@ollama/qwen3:latest"),
            Some(Target::Local(ModelId::Ollama { .. }))
        ));
        assert_eq!(
            parse("@remote/openrouter/meta-llama/llama-3.3-70b:free"),
            Some(Target::Remote {
                provider: "openrouter".into(),
                model: "meta-llama/llama-3.3-70b:free".into(),
            })
        );
        assert_eq!(
            remote_id("mistral", "mistral-small-latest"),
            "@remote/mistral/mistral-small-latest"
        );
        // Fournisseur : minuscules, chiffres, tirets, 32 au plus.
        for bad in [
            "@remote/OpenRouter/x",
            "@remote/open_router/x",
            "@remote//x",
            "@remote/openrouter",
            "@remote/openrouter/",
            "@remote/openrouter/ x",
            "@remote/openrouter/a\nb",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        assert!(parse(&format!("@remote/{}/x", "a".repeat(32))).is_some());
        assert!(parse(&format!("@remote/{}/x", "a".repeat(33))).is_none());
        assert!(parse(&format!("@remote/p/{}", "é".repeat(200))).is_some());
        assert!(parse(&format!("@remote/p/{}", "é".repeat(201))).is_none());
        // Anciens noms d'Ollama : ni l'un ni l'autre.
        assert_eq!(parse("qwen3:0.6b"), None);
        assert_eq!(parse("qwen3"), None);
        assert!(is_remote("@remote/x/y") && !is_remote("Qwen/Q-GGUF:Q4_K_M"));
    }

    #[test]
    fn sameness_follows_the_kind_of_id() {
        assert!(same("Qwen/Q-GGUF:q4_k_m", "Qwen/Q-GGUF:Q4_K_M"));
        assert!(!same("Qwen/Q-GGUF:Q4_K_M", "qwen/Q-GGUF:Q4_K_M"));
        assert!(same("@remote/mistral/m-small", "@remote/mistral/m-small"));
        // Distant : exact, casse comprise.
        assert!(!same("@remote/mistral/M-small", "@remote/mistral/m-small"));
        assert!(!same("@remote/a/x:Q4", "@remote/a/x:q4"));
        assert!(!same("Qwen/Q-GGUF:Q4_K_M", "@remote/qwen/Q-GGUF:Q4_K_M"));
        assert!(same("qwen3:0.6b", "qwen3:0.6b"));
        assert!(!same("qwen3:0.6B", "qwen3:0.6b"));
    }

    #[test]
    fn the_preference_key_keeps_identifiers_whole() {
        // Défaut corrigé : `:latest` d'un identifiant gardé.
        assert_eq!(legacy_key("@ollama/qwen3:latest"), "@ollama/qwen3:latest");
        assert_eq!(legacy_key("hf.co/u/R-GGUF:latest"), "hf.co/u/R-GGUF:latest");
        assert_eq!(legacy_key("Qwen/Q-GGUF:Q4_K_M"), "Qwen/Q-GGUF:Q4_K_M");
        assert_eq!(legacy_key("@remote/p/m:latest"), "@remote/p/m:latest");
        // Ancien nom d'Ollama : sans `:latest`, comme en 0.9.x.
        assert_eq!(legacy_key("qwen3:latest"), "qwen3");
        assert_eq!(legacy_key("qwen3"), "qwen3");
        assert_eq!(legacy_key("qwen3:0.6b"), "qwen3:0.6b");
    }
}
