//! Choix « Auto » des pages Proofread et Study (propre à ChatonLM) : sans
//! modèle choisi sur la page, le meilleur modèle déjà en mémoire dans Ollama
//! (le plus grand en paramètres), sinon celui du chat (qu'Ollama charge alors
//! à la première requête).
//!
//! Un modèle en mémoire n'est retenu que s'il tourne sur la machine (ni
//! modèle cloud, ni distant) et produit du texte (pas un modèle
//! d'embedding).

use std::cmp::Ordering;
use std::time::Duration;

use super::models;
use super::running::{self, RunningModel};

/// Délai de la liste des modèles en mémoire : court, le modèle du chat sert
/// si Ollama ne répond pas (la requête dira alors ce qui ne va pas).
const LIST_TIMEOUT: Duration = Duration::from_secs(2);

/// Modèle d'une page : `own` (choisi sur la page) s'il y en a un, sinon le
/// meilleur modèle en mémoire sur `base`, sinon `chat` ; `None` si aucun.
pub async fn model_of(base: &str, own: &str, chat: &str) -> Option<String> {
    if !own.is_empty() {
        return Some(own.to_owned());
    }
    if let Some(best) = best_loaded(base).await {
        return Some(best);
    }
    (!chat.is_empty()).then(|| chat.to_owned())
}

/// Meilleur modèle en mémoire sur `base` qui sert ici (voir
/// [`serves_here`]) ; `None` : aucun, ou liste illisible.
pub async fn best_loaded(base: &str) -> Option<String> {
    let loaded = running::list_within(base, LIST_TIMEOUT).await.ok()?;
    first_serving(base, ranked(&loaded)).await
}

/// Premier de ces noms qui sert ici, dans l'ordre.
async fn first_serving(base: &str, names: Vec<String>) -> Option<String> {
    for name in names {
        if serves_here(base, &name).await {
            return Some(name);
        }
    }
    None
}

/// Modèle qui tourne sur la machine (jamais un modèle cloud ou distant) et
/// produit du texte (pas un modèle d'embedding). Détails illisibles : la
/// requête dira ce qui ne va pas.
pub async fn serves_here(base: &str, model: &str) -> bool {
    if models::is_cloud_name(model) {
        return false;
    }
    match models::show(base, model).await {
        Ok(details) => {
            !details.remote
                && (details.capabilities.is_empty()
                    || details.capabilities.iter().any(|c| c == "completion"))
        }
        Err(_) => true,
    }
}

/// Noms des modèles en mémoire (sans `:latest`), du meilleur au moins bon :
/// nombre de paramètres décroissant (inconnu en dernier), puis mémoire
/// occupée, puis ordre d'Ollama (le plus récemment servi d'abord). Sans les
/// noms de modèles cloud.
fn ranked(loaded: &[RunningModel]) -> Vec<String> {
    let mut candidates: Vec<&RunningModel> = loaded
        .iter()
        .filter(|model| !models::is_cloud_name(&model.name))
        .collect();
    // Tri stable : l'ordre d'Ollama départage.
    candidates.sort_by(|a, b| {
        let by_parameters = match (parameters(&a.parameter_size), parameters(&b.parameter_size)) {
            (Some(a), Some(b)) => b.partial_cmp(&a).unwrap_or(Ordering::Equal),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        by_parameters.then(b.size.cmp(&a.size))
    });
    candidates
        .into_iter()
        .map(|model| models::display_name(&model.name))
        .collect()
}

/// Nombre de paramètres d'après le texte d'Ollama (« 8.2B », « 751.63M »,
/// « 1.2T », « 270K ») ; `None` si illisible.
fn parameters(size: &str) -> Option<f64> {
    let size = size.trim();
    let unit = size.chars().last()?;
    let factor = match unit.to_ascii_uppercase() {
        'K' => 1e3,
        'M' => 1e6,
        'B' => 1e9,
        'T' => 1e12,
        _ => {
            return size
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite() && *n > 0.0);
        }
    };
    let number: f64 = size[..size.len() - unit.len_utf8()].trim().parse().ok()?;
    (number.is_finite() && number > 0.0).then_some(number * factor)
}

#[cfg(test)]
mod tests {
    use fake_ollama::{FakeOllama, Mode};

    use super::*;

    fn running(name: &str, parameter_size: &str, size: u64) -> RunningModel {
        RunningModel {
            name: name.into(),
            size,
            size_vram: size,
            context_length: None,
            expires_at: None,
            kept: false,
            parameter_size: parameter_size.into(),
            quantization_level: String::new(),
        }
    }

    #[test]
    fn parameters_read_ollama_sizes() {
        for (text, expected) in [
            ("8.2B", 8.2e9),
            ("751.63M", 751.63e6),
            ("1.2T", 1.2e12),
            ("270k", 270e3),
            (" 4B ", 4e9),
        ] {
            let read = parameters(text).unwrap();
            assert!((read - expected).abs() < expected * 1e-9, "{text}: {read}");
        }
        for unreadable in ["", "B", "abcB", "-1B", "0B", "NaNB", "?"] {
            assert_eq!(parameters(unreadable), None, "{unreadable}");
        }
    }

    #[test]
    fn the_largest_model_comes_first() {
        let loaded = [
            running("qwen3:0.6b", "751.63M", 1_000),
            running("gemma3:12b", "12.2B", 9_000),
            running("mystery:latest", "", 50_000),
            running("llama3.2:3b", "3.2B", 3_000),
            running("gpt-oss:120b-cloud", "116.8B", 0),
            // Même taille que llama3.2 : départagé par la mémoire.
            running("other:3b", "3.2B", 4_000),
        ];
        assert_eq!(
            ranked(&loaded),
            [
                "gemma3:12b",
                "other:3b",
                "llama3.2:3b",
                "qwen3:0.6b",
                "mystery"
            ]
        );
        assert!(ranked(&[]).is_empty());
    }

    #[test]
    fn equal_models_keep_the_order_of_ollama() {
        let loaded = [running("b:1b", "1B", 10), running("a:1b", "1B", 10)];
        assert_eq!(ranked(&loaded), ["b:1b", "a:1b"]);
    }

    #[tokio::test]
    async fn embedding_and_remote_models_are_skipped() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let names = |list: &[&str]| list.iter().map(|&n| n.to_owned()).collect::<Vec<_>>();
        assert_eq!(
            first_serving(
                &fake.url(),
                names(&["chatonlm-embed", "chatonlm-polish-remote", "qwen3:0.6b"])
            )
            .await
            .as_deref(),
            Some("qwen3:0.6b")
        );
        assert_eq!(
            first_serving(&fake.url(), names(&["chatonlm-embed"])).await,
            None
        );
    }

    #[tokio::test]
    async fn auto_takes_the_best_loaded_model_then_the_chat_model() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let base = fake.url();
        // Rien en mémoire : le modèle du chat, sinon aucun.
        assert_eq!(
            model_of(&base, "", "qwen3:0.6b").await.as_deref(),
            Some("qwen3:0.6b")
        );
        assert_eq!(model_of(&base, "", "").await, None);
        // Choisi sur la page : toujours lui, sans regarder la mémoire.
        assert_eq!(
            model_of(&base, "gemma3:4b", "qwen3:0.6b").await.as_deref(),
            Some("gemma3:4b")
        );

        running::load(&base, "qwen3:0.6b", 0, 0).await.unwrap();
        running::load(&base, "chatonlm-proofread", 0, 0)
            .await
            .unwrap();
        // chatonlm-proofread (8.0B) passe devant qwen3:0.6b (751.63M), même
        // si le chat en désigne un autre ou aucun.
        for chat in ["qwen3:0.6b", ""] {
            assert_eq!(
                model_of(&base, "", chat).await.as_deref(),
                Some("chatonlm-proofread"),
                "{chat}"
            );
        }
        assert_eq!(
            model_of(&base, "gemma3:4b", "").await.as_deref(),
            Some("gemma3:4b")
        );

        // Serveur injoignable : le modèle du chat (la requête dira l'erreur).
        assert_eq!(
            model_of("http://127.0.0.1:9", "", "qwen3:0.6b")
                .await
                .as_deref(),
            Some("qwen3:0.6b")
        );
    }

    /// Contre l'instance de test : tailles de `/api/ps` lisibles, « Auto »
    /// prend le plus grand des modèles en mémoire (d'autres contrats peuvent
    /// en charger en même temps : comparaison à la liste lue).
    #[tokio::test]
    #[ignore = "instance Ollama de test : scripts/test-ollama.sh start"]
    async fn contract_auto_takes_the_largest_loaded_model() {
        use running::tests::{CONTRACT_MODELS, TEST_INSTANCE, Unloading, unloaded};
        let _alone = CONTRACT_MODELS.lock().await;
        let model = "gemma3:270m";
        let _unloading = Unloading(model);
        unloaded(model).await;
        running::load(TEST_INSTANCE, model, 0, 120).await.unwrap();

        let loaded = running::list(TEST_INSTANCE).await.unwrap();
        let gemma = loaded
            .iter()
            .find(|m| running::same_model(&m.name, model))
            .unwrap();
        let size = parameters(&gemma.parameter_size).unwrap();
        assert!((1e8..1e9).contains(&size), "{}", gemma.parameter_size);
        let best = best_loaded(TEST_INSTANCE).await.unwrap();
        let largest = loaded
            .iter()
            .filter_map(|m| parameters(&m.parameter_size))
            .fold(0.0, f64::max);
        let chosen = loaded
            .iter()
            .find(|m| running::same_model(&m.name, &best))
            .unwrap();
        assert_eq!(parameters(&chosen.parameter_size), Some(largest));
    }
}
