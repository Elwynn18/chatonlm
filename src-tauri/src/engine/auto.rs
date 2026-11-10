//! Choix « Auto » des pages Proofread et Study (propre à ChatonLM), sur le
//! moteur de l'app : sans modèle choisi sur la page, le meilleur modèle prêt
//! dans le moteur (le plus grand en paramètres, puis en mémoire, puis le plus
//! récemment servi), sinon celui du chat (que le moteur charge alors à la
//! première requête).
//!
//! Un modèle chargé n'est retenu que s'il est encore installé et n'est pas un
//! modèle de code d'autocomplétion (modèle de base à jetons FIM, ou famille
//! « fill in the middle » d'après son nom : celui que le lien VS Code met en
//! mémoire), sauf s'il est celui du chat.

use std::cmp::Ordering;

use super::{ModelState, RunningModel};
use crate::local_models::LocalModel;
use crate::local_models::id::same_model;

/// Modèle d'une page : `own` (choisi sur la page) s'il y en a un, sinon le
/// meilleur modèle prêt du moteur, sinon `chat` ; `None` si aucun.
pub fn model_of(
    running: &[RunningModel],
    installed: &[LocalModel],
    own: &str,
    chat: &str,
) -> Option<String> {
    if !own.trim().is_empty() {
        return Some(own.trim().to_owned());
    }
    if let Some(best) = best_loaded(running, installed, chat) {
        return Some(best);
    }
    let chat = chat.trim();
    (!chat.is_empty()).then(|| chat.to_owned())
}

/// [`model_of`] sur le moteur et la base de l'app : `own` s'il est choisi,
/// sinon le meilleur modèle prêt, sinon `chat` (pages Proofread et Study).
/// Base illisible : aucun modèle installé n'est vu (rien de prêt ne sert).
pub async fn model_for(access: &crate::llm::Access, own: &str, chat: &str) -> Option<String> {
    if !own.trim().is_empty() {
        return Some(own.trim().to_owned());
    }
    let installed = crate::local_models::list(access.store(), access.data_dir())
        .await
        .unwrap_or_default();
    model_of(&access.engine().running(), &installed, own, chat)
}

/// Meilleur modèle prêt qui sert ici ; `None` : aucun.
pub fn best_loaded(
    running: &[RunningModel],
    installed: &[LocalModel],
    chat: &str,
) -> Option<String> {
    let mut candidates: Vec<(&RunningModel, &LocalModel)> = running
        .iter()
        .filter(|model| model.state == ModelState::Ready)
        .filter_map(|model| {
            let local = installed
                .iter()
                .find(|local| same_model(&local.id, &model.id))?;
            let code_allowed = !chat.trim().is_empty() && same_model(&model.id, chat.trim());
            (code_allowed || !is_code_model(local)).then_some((model, local))
        })
        .collect();
    // Tri stable : l'ordre du moteur (le plus récemment servi d'abord)
    // départage.
    candidates.sort_by(|(a, la), (b, lb)| {
        let size = |model: &RunningModel, local: &LocalModel| {
            model
                .params_label
                .as_deref()
                .or(local.params_label.as_deref())
                .and_then(parameters)
        };
        let by_parameters = match (size(a, la), size(b, lb)) {
            (Some(a), Some(b)) => b.partial_cmp(&a).unwrap_or(Ordering::Equal),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        by_parameters.then(b.size_bytes.cmp(&a.size_bytes))
    });
    candidates.first().map(|(model, _)| model.id.clone())
}

/// Modèle de code d'autocomplétion : modèle de base à jetons FIM, ou famille
/// « fill in the middle » d'après le nom (Qwen2.5-Coder, StarCoder…).
pub fn is_code_model(model: &LocalModel) -> bool {
    (model.capabilities.fim && model.capabilities.base)
        || crate::gpu::fim::family(&model.repo).is_some()
}

/// Nombre de paramètres d'après une taille annoncée (« 8.2B », « 751.63M »,
/// « 1.2T », « 270K ») ; `None` si illisible.
pub fn parameters(size: &str) -> Option<f64> {
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
    use super::*;
    use crate::local_models::{ModelCapabilities, ModelSource};

    fn local(id: &str, params: &str, caps: ModelCapabilities) -> LocalModel {
        let (repo, quant) = id.rsplit_once(':').unwrap();
        LocalModel {
            id: id.into(),
            display_name: repo.into(),
            repo: repo.into(),
            quant: quant.into(),
            path: format!("/m/{repo}/{quant}.gguf"),
            mmproj: None,
            size_bytes: 1,
            context_max: None,
            capabilities: caps,
            arch: None,
            params_label: (!params.is_empty()).then(|| params.to_owned()),
            installed_at: 0,
            source: ModelSource::Hf,
        }
    }

    fn running(id: &str, params: Option<&str>, size: u64) -> RunningModel {
        RunningModel {
            id: id.into(),
            state: ModelState::Ready,
            size_bytes: size,
            vram_bytes: size,
            context_length: 4096,
            expires_at: None,
            kept: false,
            in_use: false,
            gpu_layers: None,
            total_layers: None,
            cpu_fallback: false,
            notice: None,
            cpu_only: false,
            params_label: params.map(str::to_owned),
        }
    }

    #[test]
    fn parameters_read_announced_sizes() {
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
    fn auto_takes_the_largest_ready_model_then_the_chat_model() {
        let plain = ModelCapabilities::default();
        let installed = [
            local("unsloth/Qwen3-0.6B-GGUF:Q4_K_M", "0.6B", plain),
            local("unsloth/gemma-3-12b-it-GGUF:Q4_K_M", "12B", plain),
            local("a/mystery-GGUF:Q8_0", "", plain),
            local("a/other-3b-GGUF:Q4_K_M", "3B", plain),
            local("a/llama-3b-GGUF:Q4_K_M", "3B", plain),
        ];
        let chat = "unsloth/Qwen3-0.6B-GGUF:Q4_K_M";
        // Rien en mémoire : celui du chat, sinon aucun.
        assert_eq!(model_of(&[], &installed, "", chat).as_deref(), Some(chat));
        assert_eq!(model_of(&[], &installed, "", ""), None);
        // Choisi sur la page : toujours lui.
        assert_eq!(
            model_of(&[], &installed, " a/x-GGUF:Q4_K_M ", chat).as_deref(),
            Some("a/x-GGUF:Q4_K_M")
        );
        let loaded = [
            running("unsloth/Qwen3-0.6B-GGUF:Q4_K_M", Some("0.6B"), 1_000),
            running("a/mystery-GGUF:Q8_0", None, 50_000),
            running("a/llama-3b-GGUF:Q4_K_M", Some("3B"), 3_000),
            // Même taille : départagé par la mémoire.
            running("a/other-3b-GGUF:Q4_K_M", Some("3B"), 4_000),
            running("unsloth/gemma-3-12b-it-GGUF:Q4_K_M", Some("12B"), 9_000),
        ];
        assert_eq!(
            model_of(&loaded, &installed, "", chat).as_deref(),
            Some("unsloth/gemma-3-12b-it-GGUF:Q4_K_M")
        );
        assert_eq!(
            best_loaded(&loaded[..4], &installed, "").as_deref(),
            Some("a/other-3b-GGUF:Q4_K_M")
        );
        // Taille lue dans la base si le moteur ne l'a pas.
        let unlabeled = [
            running("a/llama-3b-GGUF:Q4_K_M", None, 3_000),
            running("a/mystery-GGUF:Q8_0", None, 50_000),
        ];
        assert_eq!(
            best_loaded(&unlabeled, &installed, "").as_deref(),
            Some("a/llama-3b-GGUF:Q4_K_M")
        );
        // En chargement, ou plus installé : ignoré.
        let mut loading = running("unsloth/gemma-3-12b-it-GGUF:Q4_K_M", Some("12B"), 9_000);
        loading.state = ModelState::Loading;
        let gone = running("b/deleted-GGUF:Q4_K_M", Some("70B"), 90_000);
        assert_eq!(best_loaded(&[loading, gone], &installed, chat), None);
    }

    /// Un modèle d'autocomplétion en mémoire n'est jamais pris, même le plus
    /// grand, sauf s'il est celui du chat.
    #[test]
    fn auto_never_takes_an_autocomplete_model() {
        let base_fim = ModelCapabilities {
            fim: true,
            base: true,
            ..ModelCapabilities::default()
        };
        // Jetons FIM d'un modèle réglé pour la conversation : permis.
        let chat_fim = ModelCapabilities {
            fim: true,
            ..ModelCapabilities::default()
        };
        let installed = [
            local("a/tiny-base-GGUF:Q8_0", "14B", base_fim),
            local("Qwen/Qwen2.5-Coder-7B-Instruct-GGUF:Q4_K_M", "7B", chat_fim),
            local("unsloth/Qwen3-0.6B-GGUF:Q4_K_M", "0.6B", chat_fim),
        ];
        let loaded = [
            running("a/tiny-base-GGUF:Q8_0", Some("14B"), 9_000),
            running(
                "Qwen/Qwen2.5-Coder-7B-Instruct-GGUF:Q4_K_M",
                Some("7B"),
                5_000,
            ),
            running("unsloth/Qwen3-0.6B-GGUF:Q4_K_M", Some("0.6B"), 1_000),
        ];
        let chat = "unsloth/Qwen3-0.6B-GGUF:Q4_K_M";
        assert_eq!(
            best_loaded(&loaded, &installed, chat).as_deref(),
            Some(chat)
        );
        assert!(is_code_model(&installed[0]));
        assert!(is_code_model(&installed[1]));
        assert!(!is_code_model(&installed[2]));
        // Celui du chat : permis.
        assert_eq!(
            best_loaded(&loaded, &installed, "a/tiny-base-GGUF:q8_0").as_deref(),
            Some("a/tiny-base-GGUF:Q8_0")
        );
        // Seul en mémoire : le modèle du chat, pas lui.
        assert_eq!(
            model_of(&loaded[..1], &installed, "", chat).as_deref(),
            Some(chat)
        );
    }

    /// Sur le moteur de l'app : le modèle prêt, sinon celui du chat.
    #[tokio::test(flavor = "multi_thread")]
    async fn auto_reads_the_engine_and_the_database() {
        let small = "test/Small-GGUF:Q4_K_M";
        let chat = "test/Chat-GGUF:Q4_K_M";
        let bench = crate::llm::testing::FakeBench::new(&[(small, "simple"), (chat, "simple")]);
        let access = &bench.access;
        assert_eq!(
            model_for(access, " own ", chat).await.as_deref(),
            Some("own")
        );
        assert_eq!(model_for(access, "", chat).await.as_deref(), Some(chat));
        assert_eq!(model_for(access, "", "").await, None);
        let lease = bench
            .engine()
            .ensure(small, crate::engine::LoadOptions::default())
            .await
            .unwrap();
        assert_eq!(model_for(access, "", chat).await.as_deref(), Some(small));
        drop(lease);
        bench.shutdown().await;
    }
}
