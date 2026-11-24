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
//!
//! Atelier (moteur de tâches, lot 1) : [`largest_fitting`] choisit parmi les
//! modèles INSTALLÉS le plus gros qui tient sur la carte (estimation du
//! moteur, comme `make_room`), et [`size_warning`] dit ce qui risque de
//! manquer au modèle retenu ([`ModelWarning`], montré par la page).

use std::cmp::Ordering;

use serde::Serialize;
use ts_rs::TS;

use super::{ModelState, RunningModel};
use crate::hub::hardware::Machine;
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

/// Nombre TOTAL de paramètres d'après une taille annoncée, mélanges
/// d'experts compris : « 30B-A3B » (30 milliards, dont 3 actifs) → 30 B,
/// « 8x7B » (huit experts) → 56 B ; sinon comme [`parameters`].
pub fn total_parameters(size: &str) -> Option<f64> {
    let size = size.trim();
    // « 30B-A3B » : la partie avant les paramètres actifs.
    if let Some((total, active)) = size.split_once('-')
        && active.trim_start().starts_with(['A', 'a'])
    {
        return parameters(total);
    }
    // « 8x7B » : nombre d'experts × taille d'un expert.
    if let Some((count, expert)) = size.split_once(['x', 'X'])
        && let Ok(count) = count.trim().parse::<u32>()
        && count > 0
    {
        return parameters(expert).map(|expert| expert * f64::from(count));
    }
    parameters(size)
}

/// Architectures qui ne conversent pas (`hub/curation.json`,
/// `rejectedArchs` : encodeurs d'embeddings, modèles non conversationnels,
/// diffusion de texte, T5) : jamais retenues par [`largest_fitting`].
const NOT_TEXT_ARCHS: &[&str] = &[
    // nonChat
    "clip",
    "eagle3",
    "dflash",
    "gemma4-assistant",
    "wavtokenizer-dec",
    "t5encoder",
    "qwen3tts",
    "pockettts",
    // encoders (embeddings)
    "bert",
    "modern-bert",
    "nomic-bert",
    "nomic-bert-moe",
    "neo-bert",
    "jina-bert-v2",
    "jina-bert-v3",
    "eurobert",
    "gemma-embedding",
    "llama-embed",
    // diffusion
    "dream",
    "llada",
    "llada-moe",
    "rnd1",
    // other
    "t5",
    "gptj",
];

/// Mots d'un identifiant de modèle d'embeddings ou de reclassement, dont
/// l'architecture peut être celle d'un modèle de conversation
/// (Qwen3-Embedding, Qwen3-Reranker : architecture `qwen3`).
const NOT_TEXT_WORDS: &[&str] = &["embed", "rerank"];

/// Modèle qui écrit du texte sur demande : ni modèle de base (dont les
/// modèles d'autocomplétion FIM), ni architecture d'embeddings ou autre
/// architecture qui ne converse pas, ni modèle d'embeddings ou de
/// reclassement d'après son identifiant (casse ignorée).
fn writes_text(model: &LocalModel) -> bool {
    let id = model.id.to_ascii_lowercase();
    !model.capabilities.base
        && !model
            .arch
            .as_deref()
            .is_some_and(|arch| NOT_TEXT_ARCHS.contains(&arch))
        && !NOT_TEXT_WORDS.iter().any(|word| id.contains(word))
}

/// Paramètres estimés d'après la taille du fichier et la densité de sa
/// quantification (`hub::quant::density`, octets par paramètre ;
/// quantification inconnue : celle de Q4_K_M) ; `None` sans taille.
fn estimated_parameters(model: &LocalModel) -> Option<f64> {
    let density = crate::hub::quant::density(Some(&model.quant.to_ascii_uppercase()));
    (model.size_bytes > 0 && density > 0).then(|| model.size_bytes as f64 * 1000.0 / density as f64)
}

/// Modèle retenu par [`largest_fitting`].
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub id: String,
    /// Paramètres au total (mélange d'experts : tous les experts) ; `None` :
    /// taille annoncée absente ou illisible.
    pub params: Option<f64>,
    /// Contexte estimé : `min_ctx`, borné au contexte d'entraînement.
    pub ctx: u32,
    /// Mémoire estimée sur la carte (poids, cache de contexte, marges).
    pub estimated_bytes: u64,
}

/// Le plus gros modèle de texte installé qui tient sur la carte avec un
/// contexte de `min_ctx` jetons (borné à son contexte d'entraînement).
/// « Tient » : estimation du moteur (poids + cache de contexte + marge de
/// `hub::fit`, plus la marge laissée par `--fit`, comme `make_room`) au plus
/// la VRAM TOTALE de la plus grande carte : les modèles inactifs seront
/// évincés au chargement. Ordre : paramètres (taille annoncée ; sans elle,
/// estimés d'après la taille du fichier et sa quantification), puis
/// quantification la plus fine, puis taille sur le disque. Aucune carte, ou
/// VRAM inconnue : `None` (l'appelant prend le modèle réglé).
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "Atelier (lot 2) : appelant à venir")
)]
pub async fn largest_fitting(
    installed: &[LocalModel],
    machine: &Machine,
    min_ctx: u32,
) -> Option<Pick> {
    let card = machine
        .gpus
        .iter()
        .map(|gpu| gpu.vram_total)
        .max()
        .filter(|vram| *vram > 0)?;
    // (retenu, paramètres pour le tri, densité, taille)
    let mut fitting: Vec<(Pick, Option<f64>, u64, u64)> = Vec::new();
    for model in installed.iter().filter(|model| writes_text(model)) {
        let ctx = match model.context_max.filter(|max| *max > 0) {
            Some(max) => min_ctx.min(max),
            None => min_ctx,
        };
        let need = super::estimate(model, ctx)
            .await
            .saturating_add(super::FIT_TARGET);
        if need > card {
            continue;
        }
        let pick = Pick {
            id: model.id.clone(),
            params: model.params_label.as_deref().and_then(total_parameters),
            ctx,
            estimated_bytes: need,
        };
        let rank = pick.params.or_else(|| estimated_parameters(model));
        let density = crate::hub::quant::density(Some(&model.quant.to_ascii_uppercase()));
        fitting.push((pick, rank, density, model.size_bytes));
    }
    fitting.sort_by(|(_, a, a_density, a_size), (_, b, b_density, b_size)| {
        let by_parameters = match (a, b) {
            (Some(a), Some(b)) => b.partial_cmp(a).unwrap_or(Ordering::Equal),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        by_parameters
            .then(b_density.cmp(a_density))
            .then(b_size.cmp(a_size))
    });
    fitting.into_iter().next().map(|(pick, ..)| pick)
}

/// Ce qui risque de manquer au modèle d'un projet (montré par la page,
/// textes côté page).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ModelWarning {
    /// Moins de 4 milliards de paramètres.
    TooSmall,
    /// De 4 à 13,9 milliards de paramètres.
    Small,
    /// Contexte possible sous 16 000 jetons.
    SmallContext,
    /// Le modèle ne sait pas réfléchir.
    NoThinking,
    /// Taille inconnue (modèle distant, taille annoncée illisible).
    UnknownSize,
}

/// Paramètres sous lesquels un modèle est « trop petit », puis « petit ».
const TOO_SMALL: f64 = 4e9;
const SMALL: f64 = 14e9;
/// Contexte sous lequel il est « court ».
const SMALL_CONTEXT: u32 = 16_000;

/// Avertissements pour un modèle de `params` paramètres AU TOTAL
/// ([`total_parameters`] ; `None` : inconnus), de contexte possible `ctx_max`
/// (0 : inconnu, rien n'est dit) et qui sait réfléchir ou non.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "Atelier (lot 2) : appelant à venir")
)]
pub fn size_warning(params: Option<f64>, ctx_max: u32, thinking: bool) -> Vec<ModelWarning> {
    let mut warnings = Vec::new();
    match params {
        None => warnings.push(ModelWarning::UnknownSize),
        Some(params) if params < TOO_SMALL => warnings.push(ModelWarning::TooSmall),
        Some(params) if params < SMALL => warnings.push(ModelWarning::Small),
        Some(_) => {}
    }
    if ctx_max > 0 && ctx_max < SMALL_CONTEXT {
        warnings.push(ModelWarning::SmallContext);
    }
    if !thinking {
        warnings.push(ModelWarning::NoThinking);
    }
    warnings
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

    const GIB: u64 = 1024 * 1024 * 1024;

    /// Modèle installé factice (fichier absent : estimation d'après la
    /// taille annoncée, comme pour un GGUF illisible).
    fn model(
        id: &str,
        params: &str,
        size: u64,
        context_max: Option<u32>,
        arch: &str,
        caps: ModelCapabilities,
    ) -> LocalModel {
        LocalModel {
            size_bytes: size,
            context_max,
            arch: Some(arch.to_owned()),
            ..local(id, params, caps)
        }
    }

    /// Machine factice : cartes de ces tailles (Gio), 64 Gio de RAM.
    fn machine(cards: &[u64]) -> Machine {
        Machine {
            gpus: cards
                .iter()
                .map(|gib| crate::hub::types::HubGpu {
                    name: "Fake GPU".into(),
                    vram_total: gib * GIB,
                    vram_free: Some(gib * GIB / 2),
                })
                .collect(),
            ram_total: 64 * GIB,
            ram_available: 48 * GIB,
        }
    }

    /// Table installée : tailles réalistes (Q4_K_M ≈ 0,62 octet par
    /// paramètre), plus des modèles qui gagneraient s'ils n'étaient pas
    /// écartés (base FIM, embeddings).
    fn shelf() -> Vec<LocalModel> {
        let plain = ModelCapabilities::default();
        let base_fim = ModelCapabilities {
            fim: true,
            base: true,
            ..plain
        };
        vec![
            model(
                "a/Tiny-4B-GGUF:Q4_K_M",
                "4B",
                2_500_000_000,
                Some(40_960),
                "qwen3",
                plain,
            ),
            model(
                "a/Mid-8B-GGUF:Q4_K_M",
                "8B",
                5_000_000_000,
                Some(40_960),
                "qwen3",
                plain,
            ),
            model(
                "g/gemma-3-12b-it-GGUF:Q4_K_M",
                "12B",
                7_300_000_000,
                Some(131_072),
                "gemma3",
                plain,
            ),
            model(
                "a/Big-27B-GGUF:Q4_K_M",
                "27B",
                16_500_000_000,
                Some(131_072),
                "qwen35",
                plain,
            ),
            model(
                "a/Big-27B-GGUF:Q5_K_M",
                "27B",
                19_200_000_000,
                Some(131_072),
                "qwen35",
                plain,
            ),
            model(
                "a/Big-27B-GGUF:Q8_0",
                "27B",
                28_700_000_000,
                Some(131_072),
                "qwen35",
                plain,
            ),
            model(
                "a/Moe-30B-A3B-GGUF:Q4_K_M",
                "30B-A3B",
                18_600_000_000,
                Some(40_960),
                "qwen3moe",
                plain,
            ),
            model(
                "a/Huge-70B-GGUF:Q4_K_M",
                "70B",
                42_000_000_000,
                Some(131_072),
                "llama",
                plain,
            ),
            // Écartés, quoique « plus gros » et petits sur le disque.
            model(
                "a/Coder-Base-40B-GGUF:Q4_K_M",
                "40B",
                3_000_000_000,
                Some(32_768),
                "qwen2",
                base_fim,
            ),
            model(
                "n/embed-50B-GGUF:F16",
                "50B",
                1_000_000_000,
                Some(8_192),
                "nomic-bert",
                plain,
            ),
            // Taille annoncée inconnue : estimée d'après le fichier (1,6 B).
            model(
                "a/mystery-GGUF:Q4_K_M",
                "",
                1_000_000_000,
                None,
                "llama",
                plain,
            ),
        ]
    }

    #[test]
    fn moe_sizes_count_every_expert() {
        let near = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() < b * 1e-9);
        assert!(near(total_parameters("30B-A3B"), 30e9));
        assert!(near(total_parameters("235B-A22B"), 235e9));
        assert!(near(total_parameters("8x7B"), 56e9));
        assert!(near(total_parameters("8.2B"), 8.2e9));
        assert_eq!(total_parameters("x7B"), None);
        assert_eq!(total_parameters(""), None);
    }

    /// Le plus gros qui tient sur la plus grande carte : 8, 12 et 24 Gio ;
    /// rien sans carte.
    #[tokio::test]
    async fn the_largest_model_that_fits_the_card_is_picked() {
        let shelf = shelf();
        let id = |pick: Option<Pick>| pick.map(|pick| pick.id);
        assert_eq!(
            id(largest_fitting(&shelf, &machine(&[8]), 16_384).await).as_deref(),
            Some("a/Tiny-4B-GGUF:Q4_K_M")
        );
        assert_eq!(
            id(largest_fitting(&shelf, &machine(&[12]), 16_384).await).as_deref(),
            Some("g/gemma-3-12b-it-GGUF:Q4_K_M")
        );
        // Mélange d'experts : compté à 30 B (au total), devant le 27B.
        assert_eq!(
            id(largest_fitting(&shelf, &machine(&[24]), 16_384).await).as_deref(),
            Some("a/Moe-30B-A3B-GGUF:Q4_K_M")
        );
        // Deux cartes : la plus grande compte, pas la somme.
        assert_eq!(
            id(largest_fitting(&shelf, &machine(&[8, 12]), 16_384).await).as_deref(),
            Some("g/gemma-3-12b-it-GGUF:Q4_K_M")
        );
        // Sans carte, ou VRAM inconnue : rien (l'appelant prend le modèle réglé).
        assert_eq!(largest_fitting(&shelf, &machine(&[]), 16_384).await, None);
        assert_eq!(largest_fitting(&shelf, &machine(&[0]), 16_384).await, None);
        // Rien qui tienne.
        assert_eq!(
            largest_fitting(&shelf[7..8], &machine(&[24]), 16_384).await,
            None
        );
    }

    /// Même taille : la quantification la plus fine qui tient ; puis la
    /// taille ; taille annoncée inconnue : estimée d'après le fichier ;
    /// contexte borné au modèle.
    #[tokio::test]
    async fn the_finest_quantization_that_fits_wins_among_equals() {
        let shelf: Vec<LocalModel> = shelf()
            .into_iter()
            .filter(|model| !model.id.contains("Moe"))
            .collect();
        let pick = largest_fitting(&shelf, &machine(&[24]), 16_384)
            .await
            .unwrap();
        assert_eq!(pick.id, "a/Big-27B-GGUF:Q5_K_M");
        assert!(pick.params.is_some_and(|p| (p - 27e9).abs() < 1.0));
        assert_eq!(pick.ctx, 16_384);
        assert_eq!(
            pick.estimated_bytes,
            crate::engine::estimate(&shelf[4], 16_384).await + crate::engine::FIT_TARGET
        );
        // Sur une carte où seul Q4_K_M tient.
        let pick = largest_fitting(&shelf, &machine(&[20]), 16_384)
            .await
            .unwrap();
        assert_eq!(pick.id, "a/Big-27B-GGUF:Q4_K_M");
        // Même quantification : la plus lourde sur le disque.
        let plain = ModelCapabilities::default();
        let twins = [
            model(
                "a/One-8B-GGUF:Q4_K_M",
                "8B",
                4_900_000_000,
                None,
                "llama",
                plain,
            ),
            model(
                "a/Two-8B-GGUF:Q4_K_M",
                "8B",
                5_100_000_000,
                None,
                "llama",
                plain,
            ),
            model(
                "a/Low-8B-GGUF:q3_k_m",
                "8B",
                9_000_000_000,
                None,
                "llama",
                plain,
            ),
        ];
        assert_eq!(
            largest_fitting(&twins, &machine(&[24]), 8192)
                .await
                .unwrap()
                .id,
            "a/Two-8B-GGUF:Q4_K_M"
        );
        // Taille annoncée inconnue : prise seule ; estimée (1,6 B) d'après
        // son fichier, elle perd contre un 4B annoncé.
        let mystery = shelf.last().unwrap().clone();
        let unknown = largest_fitting(std::slice::from_ref(&mystery), &machine(&[8]), 4096)
            .await
            .unwrap();
        assert_eq!(
            (unknown.id.as_str(), unknown.params),
            ("a/mystery-GGUF:Q4_K_M", None)
        );
        let small = largest_fitting(&[mystery, shelf[0].clone()], &machine(&[8]), 4096)
            .await
            .unwrap();
        assert_eq!(small.id, "a/Tiny-4B-GGUF:Q4_K_M");
        // Contexte demandé au-delà de celui du modèle : borné.
        let short = model(
            "a/Short-8B-GGUF:Q4_K_M",
            "8B",
            5_000_000_000,
            Some(8_192),
            "llama",
            plain,
        );
        assert_eq!(
            largest_fitting(&[short], &machine(&[24]), 32_768)
                .await
                .unwrap()
                .ctx,
            8_192
        );
    }

    /// Sans taille annoncée, les paramètres sont estimés d'après le fichier
    /// et sa quantification : un 32B sans libellé passe devant un 4B annoncé
    /// (et devant un 14B), mais pas devant un 70B annoncé.
    #[tokio::test]
    async fn an_unlabeled_model_is_ranked_by_its_file() {
        let plain = ModelCapabilities::default();
        let unlabeled = model(
            "a/Nameless-GGUF:Q4_K_M",
            "",
            19_800_000_000,
            Some(40_960),
            "qwen2",
            plain,
        );
        let tiny = shelf()[0].clone();
        let mid = model(
            "a/Mid-14B-GGUF:Q4_K_M",
            "14B",
            9_000_000_000,
            Some(40_960),
            "qwen3",
            plain,
        );
        let pick = largest_fitting(&[tiny, mid, unlabeled.clone()], &machine(&[48]), 8192)
            .await
            .unwrap();
        assert_eq!(pick.id, "a/Nameless-GGUF:Q4_K_M");
        // Le libellé manque toujours (avertissement « taille inconnue »).
        assert_eq!(pick.params, None);
        let estimate = estimated_parameters(&unlabeled).unwrap();
        assert!((30e9..33e9).contains(&estimate), "{estimate}");
        // Quantification plus dense : moins de paramètres pour la même taille.
        let q8 = LocalModel {
            quant: "Q8_0".into(),
            ..unlabeled.clone()
        };
        assert!(estimated_parameters(&q8).unwrap() < 20e9);
        let huge = shelf()[7].clone();
        assert_eq!(
            largest_fitting(&[unlabeled, huge], &machine(&[96]), 8192)
                .await
                .unwrap()
                .id,
            "a/Huge-70B-GGUF:Q4_K_M"
        );
    }

    /// Modèle de base (autocomplétion FIM), embeddings : jamais retenus,
    /// même plus gros ; la liste suit celle de la curation. Un modèle
    /// d'embeddings ou de reclassement d'architecture conversationnelle est
    /// reconnu à son identifiant.
    #[tokio::test]
    async fn base_and_embedding_models_are_never_picked() {
        let shelf = shelf();
        let excluded = &shelf[8..10];
        assert_eq!(largest_fitting(excluded, &machine(&[24]), 4096).await, None);
        let plain = ModelCapabilities::default();
        let by_name = [
            model(
                "Qwen/Qwen3-Embedding-8B-GGUF:Q4_K_M",
                "8B",
                5_000_000_000,
                Some(40_960),
                "qwen3",
                plain,
            ),
            model(
                "a/Qwen3-RERANKER-4B-GGUF:Q4_K_M",
                "4B",
                2_500_000_000,
                Some(40_960),
                "qwen3",
                plain,
            ),
            model(
                "a/multilingual-e5-EMBED-GGUF:F16",
                "",
                1_000_000_000,
                None,
                "qwen2",
                plain,
            ),
        ];
        assert_eq!(largest_fitting(&by_name, &machine(&[24]), 4096).await, None);
        let mut with_chat = by_name.to_vec();
        with_chat.push(shelf[0].clone());
        assert_eq!(
            largest_fitting(&with_chat, &machine(&[24]), 4096)
                .await
                .unwrap()
                .id,
            "a/Tiny-4B-GGUF:Q4_K_M"
        );
        // La marge de `--fit` compte : un modèle qui tient juste sans elle
        // ne tient pas.
        let plain = ModelCapabilities::default();
        let bare = model(
            "a/Edge-8B-GGUF:Q4_K_M",
            "8B",
            0,
            Some(40_960),
            "llama",
            plain,
        );
        let without_weights = crate::engine::estimate(&bare, 8192).await;
        let card = 12 * GIB;
        let edge = |weights: u64| LocalModel {
            size_bytes: weights,
            ..bare.clone()
        };
        let exact = card - crate::engine::FIT_TARGET - without_weights;
        assert!(
            largest_fitting(&[edge(exact)], &machine(&[12]), 8192)
                .await
                .is_some()
        );
        assert_eq!(
            largest_fitting(&[edge(exact + 1)], &machine(&[12]), 8192).await,
            None
        );
        for arch in NOT_TEXT_ARCHS {
            assert!(
                !crate::hub::curation::CURATION.accepted_arch(Some(arch)),
                "{arch}"
            );
        }
    }

    #[test]
    fn size_warnings_follow_the_thresholds() {
        use ModelWarning::*;
        assert_eq!(size_warning(Some(27e9), 131_072, true), vec![]);
        assert_eq!(size_warning(Some(14e9), 16_000, true), vec![]);
        assert_eq!(size_warning(Some(13.9e9), 16_000, true), vec![Small]);
        assert_eq!(size_warning(Some(4e9), 40_960, true), vec![Small]);
        assert_eq!(size_warning(Some(3.9e9), 40_960, true), vec![TooSmall]);
        assert_eq!(
            size_warning(Some(0.6e9), 8192, false),
            vec![TooSmall, SmallContext, NoThinking]
        );
        assert_eq!(size_warning(Some(30e9), 15_999, true), vec![SmallContext]);
        // Distant ou illisible : taille inconnue ; contexte inconnu : rien.
        assert_eq!(size_warning(None, 0, true), vec![UnknownSize]);
        assert_eq!(size_warning(Some(70e9), 131_072, false), vec![NoThinking]);
        // Mélange d'experts : nombre total.
        assert_eq!(
            size_warning(total_parameters("30B-A3B"), 40_960, true),
            vec![]
        );
        assert_eq!(
            serde_json::to_value([TooSmall, SmallContext, NoThinking, UnknownSize, Small]).unwrap(),
            serde_json::json!([
                "tooSmall",
                "smallContext",
                "noThinking",
                "unknownSize",
                "small"
            ])
        );
    }
}
