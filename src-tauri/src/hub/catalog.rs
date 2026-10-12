//! Catalogue du ModelHub (contrat § 4-6, 35 § 5-7) : dépôts gardés,
//! dédoublonnage, rubriques, familles, filtres, et cartes et fiches montrées
//! par l'interface.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use super::curation::{CURATION, Family};
use super::fit;
use super::gguf::GgufFacts;
use super::quant;
use super::repo::Repo;
use super::types::{
    HubFamily, HubFilters, HubFit, HubHardware, HubModel, HubQuant, HubRepo, HubSectionId, HubSize,
    HubTrust, HubUse,
};

/// Rubriques de l'accueil, dans l'ordre.
pub const SECTIONS: [HubSectionId; 7] = [
    HubSectionId::ForYou,
    HubSectionId::Trending,
    HubSectionId::Popular,
    HubSectionId::Coding,
    HubSectionId::Vision,
    HubSectionId::Reasoning,
    HubSectionId::Small,
];

/// Moins de 4 milliards : petit ; plus de 15 : grand.
const SMALL_BELOW: u64 = 4_000_000_000;
const LARGE_ABOVE: u64 = 15_000_000_000;
/// Audience minimale d'un dépôt de l'accueil.
const MIN_DOWNLOADS: u64 = 1000;
const MIN_LIKES: u64 = 50;

/// « 8B », « 0.6B », « 135M » dans un nom, détaché des lettres et chiffres
/// voisins (« A3B », « E4B » : non).
static SIZE_IN_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9.])(\d+(?:\.\d+)?)([bm])(?:[^a-z0-9]|$)").expect("valid pattern")
});

/// Suffixes retirés pour la clé d'un modèle (35 § 5.2).
static KEY_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(-qat-q4_0-unquantized|[-_.](i1-)?gguf)$").expect("valid pattern")
});
static QAT_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)-qat-q4_0$").expect("valid pattern"));

/// Nombre de paramètres écrit dans un nom.
fn parameters_in_name(name: &str) -> Option<u64> {
    let captures = SIZE_IN_NAME.captures(name)?;
    let number: f64 = captures.get(1)?.as_str().parse().ok()?;
    let unit = if captures.get(2)?.as_str().eq_ignore_ascii_case("b") {
        1e9
    } else {
        1e6
    };
    Some((number * unit).round() as u64).filter(|&n| n > 0)
}

/// Paramètres : `gguf.total`, contrôlé par le nombre écrit dans le nom
/// (écart de plus d'un facteur 2 : le nom l'emporte ; 35 R7.1 : `total` peut
/// venir d'un fichier auxiliaire).
pub fn parameters(repo: &Repo) -> Option<u64> {
    let named = parameters_in_name(repo.repo_name());
    match (repo.total, named) {
        (Some(total), Some(named)) => {
            let (low, high) = (total.min(named) as f64, total.max(named) as f64);
            Some(if high > 2.0 * low { named } else { total })
        }
        (total, named) => total.or(named),
    }
}

/// Nom affiché : le dépôt sans suffixe `-GGUF` / `_GGUF` (casse ignorée).
pub fn display_name(repo_name: &str) -> String {
    let lower = repo_name.to_ascii_lowercase();
    if repo_name.len() > 5 && (lower.ends_with("-gguf") || lower.ends_with("_gguf")) {
        repo_name[..repo_name.len() - 5].to_owned()
    } else {
        repo_name.to_owned()
    }
}

/// Au moins une version téléchargeable.
pub fn downloadable(repo: &Repo) -> bool {
    quant::inventory(&repo.id, &repo.file_entries())
        .iter()
        .any(|version| version.model.is_some())
}

/// Règles de toutes les vues, recherche comprise : architecture acceptée,
/// conversation, ni restreint ni privé, une version téléchargeable.
pub fn usable(repo: &Repo) -> bool {
    CURATION.accepted_arch(repo.architecture.as_deref())
        && CURATION.chat_pipeline(repo.pipeline_tag.as_deref())
        && !repo.gated
        && !repo.private
        && downloadable(repo)
}

/// Règle de l'accueil, des rubriques et des familles (35 § 7, dans l'ordre) :
/// origine de confiance, règles communes, pas de mot exclu, audience.
pub fn curated(repo: &Repo) -> bool {
    CURATION.trusted_origin(
        &repo.author,
        repo.base_model.as_deref(),
        repo.base_relation.as_deref(),
    ) && CURATION.accepted_arch(repo.architecture.as_deref())
        && CURATION.chat_pipeline(repo.pipeline_tag.as_deref())
        && !CURATION.has_excluded_word(&repo.id)
        && (repo.downloads >= MIN_DOWNLOADS || repo.likes >= MIN_LIKES)
        && !repo.gated
        && !repo.private
        && downloadable(repo)
}

/// Clé d'un modèle : modèle de base d'une quantification, sinon le dépôt,
/// sans suffixe GGUF ni QAT, en minuscules.
pub fn canonical_key(repo: &Repo) -> String {
    let source = match (&repo.base_model, repo.base_relation.as_deref()) {
        (Some(base), Some("quantized")) => base.as_str(),
        _ => repo.id.as_str(),
    };
    let stripped = KEY_SUFFIX.replace(source, "");
    QAT_SUFFIX.replace(&stripped, "").to_ascii_lowercase()
}

/// Organisation d'origine : celle du modèle de base, sinon le laboratoire
/// publieur.
fn origin(repo: &Repo) -> Option<&str> {
    match &repo.base_model {
        Some(base) => base.split_once('/').map(|(org, _)| org),
        None => (CURATION.trust(&repo.author) == HubTrust::Lab).then_some(repo.author.as_str()),
    }
}

pub fn family_of(repo: &Repo) -> Option<&'static Family> {
    CURATION.family(
        &repo.id,
        repo.base_model.as_deref(),
        origin(repo),
        repo.architecture.as_deref(),
    )
}

/// Place d'un modèle de taille `size` dans la machine, cache estimé sans
/// en-tête.
fn list_fit(repo: &Repo, parameters: Option<u64>, size: u64, hardware: &HubHardware) -> HubFit {
    let context = fit::model_context(
        hardware.context,
        repo.context_length,
        repo.capabilities.vision,
    );
    let kv = fit::kv_cache(None, parameters, repo.architecture.as_deref(), context);
    fit::level(fit::memory(size, kv, context), hardware)
}

/// Carte d'un dépôt (listes : taille estimée d'après les paramètres).
pub fn model(repo: &Repo, hardware: Option<&HubHardware>) -> HubModel {
    let parameters = parameters(repo);
    let estimated_size = fit::list_size(parameters, repo.capabilities.vision);
    let fit = hardware
        .zip(estimated_size)
        .map(|(hardware, size)| list_fit(repo, parameters, size, hardware));
    HubModel {
        id: repo.id.clone(),
        author: repo.author.clone(),
        name: display_name(repo.repo_name()),
        family: family_of(repo).map(|family| family.id.clone()),
        base_model: repo.base_model.clone(),
        trust: CURATION.trust(&repo.author),
        parameters,
        architecture: repo.architecture.clone(),
        context_length: repo.context_length,
        capabilities: repo.capabilities,
        license: repo.license.clone(),
        downloads: repo.downloads,
        likes: repo.likes,
        last_modified: repo.last_modified.clone(),
        estimated_size,
        fit,
    }
}

/// Filtres de l'interface. `Fits` est sans effet si la machine est inconnue.
pub fn matches(model: &HubModel, filters: &HubFilters, machine_known: bool) -> bool {
    let caps = model.capabilities;
    let uses = filters.uses.iter().all(|wanted| match wanted {
        HubUse::Code => caps.code,
        HubUse::Vision => caps.vision,
        HubUse::Reasoning => caps.thinking,
        HubUse::Tools => caps.tools,
    });
    let size = match filters.size {
        None => true,
        Some(HubSize::Fits) => !machine_known || model.fit == Some(HubFit::Fits),
        Some(HubSize::Small) => model.parameters.is_some_and(|p| p < SMALL_BELOW),
        Some(HubSize::Medium) => model
            .parameters
            .is_some_and(|p| (SMALL_BELOW..=LARGE_ABOVE).contains(&p)),
        Some(HubSize::Large) => model.parameters.is_some_and(|p| p > LARGE_ABOVE),
    };
    uses && size
        && (!filters.open_license || CURATION.open_license(model.license.as_deref()))
        && (!filters.verified_only || model.trust != HubTrust::Unverified)
}

/// Modèle du catalogue de l'accueil.
#[derive(Clone, Debug)]
pub struct Entry {
    pub model: HubModel,
    /// Clé du modèle de base (dédoublonnage).
    pub key: String,
    /// Score de tendance (le plus haut du groupe) si le modèle figure dans la
    /// liste des tendances.
    pub trending: Option<f64>,
}

/// Catalogue de l'accueil : populaires et tendances réunis, dépôts gardés
/// (règle 35 § 7), un représentant par modèle de base (laboratoire, sinon le
/// plus téléchargé des quantificateurs de rang 1, puis 2). Ordre : celui des
/// listes.
pub fn entries(popular: &[Repo], trending: &[Repo], hardware: Option<&HubHardware>) -> Vec<Entry> {
    let trending_ids: HashSet<&str> = trending.iter().map(|repo| repo.id.as_str()).collect();
    let mut seen = HashSet::new();
    let mut groups: Vec<(String, Vec<&Repo>)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for repo in popular.iter().chain(trending) {
        if !seen.insert(repo.id.to_ascii_lowercase()) || !curated(repo) {
            continue;
        }
        let key = canonical_key(repo);
        match index.get(&key) {
            Some(&at) => groups[at].1.push(repo),
            None => {
                index.insert(key.clone(), groups.len());
                groups.push((key, vec![repo]));
            }
        }
    }
    groups
        .into_iter()
        .map(|(key, members)| {
            let best = members
                .iter()
                .copied()
                .min_by(|a, b| {
                    CURATION
                        .rank(&a.author)
                        .cmp(&CURATION.rank(&b.author))
                        .then(b.downloads.cmp(&a.downloads))
                })
                .expect("a group has members");
            let trending = members
                .iter()
                .filter(|repo| trending_ids.contains(repo.id.as_str()))
                .map(|repo| repo.trending)
                .max_by(f64::total_cmp);
            Entry {
                model: model(best, hardware),
                key,
                trending,
            }
        })
        .collect()
}

fn by_downloads(a: &HubModel, b: &HubModel) -> Ordering {
    b.downloads.cmp(&a.downloads).then_with(|| a.id.cmp(&b.id))
}

/// Modèles d'une rubrique, dans son ordre (liste complète).
pub fn section(entries: &[Entry], id: HubSectionId, machine_known: bool) -> Vec<HubModel> {
    let pick = |keep: &dyn Fn(&HubModel) -> bool| {
        let mut models: Vec<HubModel> = entries
            .iter()
            .map(|entry| &entry.model)
            .filter(|model| keep(model))
            .cloned()
            .collect();
        models.sort_by(by_downloads);
        models
    };
    match id {
        HubSectionId::ForYou => {
            if !machine_known {
                return Vec::new();
            }
            // Par famille (ou par modèle hors famille), le plus gros qui tient.
            let mut best: HashMap<&str, &HubModel> = HashMap::new();
            for entry in entries {
                let model = &entry.model;
                if model.fit != Some(HubFit::Fits) {
                    continue;
                }
                let group = model.family.as_deref().unwrap_or(&entry.key);
                let bigger = |current: &&HubModel| {
                    (model.parameters, model.downloads) > (current.parameters, current.downloads)
                };
                if best.get(group).is_none_or(bigger) {
                    best.insert(group, model);
                }
            }
            let mut models: Vec<HubModel> = best.into_values().cloned().collect();
            models.sort_by(by_downloads);
            models
        }
        HubSectionId::Trending => {
            let mut scored: Vec<(f64, &HubModel)> = entries
                .iter()
                .filter_map(|entry| Some((entry.trending?, &entry.model)))
                .collect();
            scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| by_downloads(a.1, b.1)));
            scored.into_iter().map(|(_, model)| model.clone()).collect()
        }
        HubSectionId::Popular => pick(&|_| true),
        HubSectionId::Coding => pick(&|m| m.capabilities.code),
        HubSectionId::Vision => pick(&|m| m.capabilities.vision),
        HubSectionId::Reasoning => pick(&|m| m.capabilities.thinking),
        HubSectionId::Small => pick(&|m| m.parameters.is_some_and(|p| p < SMALL_BELOW)),
    }
}

/// Modèles d'une famille, par téléchargements.
pub fn family_models(entries: &[Entry], family: &str) -> Vec<HubModel> {
    let mut models: Vec<HubModel> = entries
        .iter()
        .map(|entry| &entry.model)
        .filter(|model| model.family.as_deref() == Some(family))
        .cloned()
        .collect();
    models.sort_by(by_downloads);
    models
}

/// Familles présentes, par téléchargements cumulés.
pub fn families(entries: &[Entry]) -> Vec<HubFamily> {
    let mut found: HashMap<&str, (u32, u64)> = HashMap::new();
    for entry in entries {
        if let Some(family) = &entry.model.family {
            let total = found.entry(family).or_default();
            total.0 += 1;
            total.1 += entry.model.downloads;
        }
    }
    let mut families: Vec<(HubFamily, u64)> = found
        .into_iter()
        .filter_map(|(id, (count, downloads))| {
            Some((
                HubFamily {
                    id: id.to_owned(),
                    label: CURATION.family_label(id)?.to_owned(),
                    count,
                },
                downloads,
            ))
        })
        .collect();
    families.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.id.cmp(&b.0.id)));
    families.into_iter().map(|(family, _)| family).collect()
}

/// Autres dépôts de confiance du même modèle de base, parmi `known`.
pub fn other_sources(repo: &Repo, known: &[&Repo]) -> Vec<String> {
    let key = canonical_key(repo);
    let mut seen = HashSet::new();
    let mut others: Vec<&Repo> = known
        .iter()
        .copied()
        .filter(|other| {
            !other.id.eq_ignore_ascii_case(&repo.id)
                && CURATION.trust(&other.author) != HubTrust::Unverified
                && canonical_key(other) == key
                && seen.insert(other.id.to_ascii_lowercase())
                && usable(other)
        })
        .collect();
    others.sort_by(|a, b| b.downloads.cmp(&a.downloads).then_with(|| a.id.cmp(&b.id)));
    others.into_iter().map(|other| other.id.clone()).collect()
}

/// Fiche d'un dépôt : versions avec tailles exactes, estimation de mémoire
/// (en-tête GGUF s'il a été lu), version conseillée.
pub fn repo_view(
    repo: &Repo,
    header: Option<&GgufFacts>,
    hardware: Option<&HubHardware>,
    other_sources: Vec<String>,
) -> HubRepo {
    let parameters = parameters(repo);
    let files = repo.file_entries();
    let versions = quant::inventory(&repo.id, &files);
    let vision = repo.capabilities.vision;
    let projector = if vision {
        quant::projector_size(&files).unwrap_or(0)
    } else {
        0
    };
    let context_used =
        hardware.map(|hardware| fit::model_context(hardware.context, repo.context_length, vision));
    let mut quants: Vec<HubQuant> = versions
        .iter()
        .map(|version| {
            let download_size = version.size.saturating_add(projector);
            let memory = context_used.map(|context| {
                let kv = fit::kv_cache(header, parameters, repo.architecture.as_deref(), context);
                fit::memory(download_size, kv, context)
            });
            HubQuant {
                label: version.label.clone(),
                file: version.file.clone(),
                model: if repo.gated {
                    None
                } else {
                    version.model.clone()
                },
                issue: version.issue,
                size: version.size,
                download_size,
                tier: quant::tier(version.quant.as_deref(), version.size, parameters),
                fit: hardware
                    .zip(memory)
                    .map(|(hardware, need)| fit::level(need, hardware)),
                memory,
                recommended: false,
            }
        })
        .collect();
    let chosen = if repo.gated {
        None
    } else {
        quant::recommended(&versions, |index| quants[index].fit == Some(HubFit::Fits))
    };
    let mut card = model(repo, hardware);
    if let Some(index) = chosen {
        quants[index].recommended = true;
        card.estimated_size = Some(quants[index].download_size);
        card.fit = quants[index].fit;
    }
    HubRepo {
        model: card,
        sha: repo.sha.clone(),
        gated: repo.gated,
        created_at: repo.created_at.clone(),
        languages: repo.languages.clone(),
        quants,
        context_used,
        other_sources,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::repo::RepoFile;
    use crate::hub::types::{HubCapabilities, HubGpu};

    pub fn repo(id: &str) -> Repo {
        let name = id.split_once('/').unwrap().1;
        Repo {
            id: id.to_owned(),
            author: id.split_once('/').unwrap().0.to_owned(),
            downloads: 10_000,
            likes: 10,
            trending: 0.0,
            created_at: None,
            last_modified: None,
            pipeline_tag: Some("text-generation".into()),
            gated: false,
            private: false,
            architecture: Some("qwen3".into()),
            context_length: Some(40960),
            total: Some(8_000_000_000),
            base_model: None,
            base_relation: None,
            license: Some("apache-2.0".into()),
            languages: vec![],
            capabilities: HubCapabilities::default(),
            files: vec![RepoFile {
                name: format!("{}-Q4_K_M.gguf", display_name(name)),
                size: None,
            }],
            sha: None,
        }
    }

    fn quantized(id: &str, base: &str) -> Repo {
        Repo {
            base_model: Some(base.to_owned()),
            base_relation: Some("quantized".into()),
            ..repo(id)
        }
    }

    fn machine(vram_gib: u64) -> HubHardware {
        HubHardware {
            gpus: vec![HubGpu {
                name: "GPU".into(),
                vram_total: vram_gib << 30,
                vram_free: None,
            }],
            ram_total: 32 << 30,
            ram_available: 16 << 30,
            context: 4096,
        }
    }

    #[test]
    fn parameters_are_checked_against_the_name() {
        for (name, value) in [
            ("Qwen3-8B-GGUF", Some(8_000_000_000)),
            ("Qwen3.5-0.8B-GGUF", Some(800_000_000)),
            ("SmolLM2-135M-Instruct-GGUF", Some(135_000_000)),
            ("Qwen3-Coder-30B-A3B-Instruct-GGUF", Some(30_000_000_000)),
            ("gpt-oss-20b-GGUF", Some(20_000_000_000)),
            ("gemma-4-E4B-it-GGUF", None),
            ("Mixtral-8x22B-GGUF", None),
            ("Phi-4-mini-GGUF", None),
        ] {
            assert_eq!(parameters_in_name(name), value, "{name}");
        }
        // Tête MTP lue à la place du modèle (1,86 B pour un 27B) : le nom.
        let mtp = Repo {
            total: Some(1_860_000_000),
            ..repo("HauhauCS/Qwen3.8-27B-GGUF")
        };
        assert_eq!(parameters(&mtp), Some(27_000_000_000));
        // Écart modéré (gemma « E2B » = 4,65 B réels) : `total`.
        let close = Repo {
            total: Some(8_190_000_000),
            ..repo("Qwen/Qwen3-8B-GGUF")
        };
        assert_eq!(parameters(&close), Some(8_190_000_000));
        let unnamed = Repo {
            total: None,
            ..repo("x/Model-7B-GGUF")
        };
        assert_eq!(parameters(&unnamed), Some(7_000_000_000));
    }

    #[test]
    fn names_and_keys() {
        assert_eq!(display_name("Qwen3-8B-GGUF"), "Qwen3-8B");
        assert_eq!(
            display_name("gemma-4-12B-it-qat-q4_0-gguf"),
            "gemma-4-12B-it-qat-q4_0"
        );
        assert_eq!(display_name("Qwen_Qwen3-8B_GGUF"), "Qwen_Qwen3-8B");
        assert_eq!(display_name("GGUF"), "GGUF");
        assert_eq!(display_name("Model"), "Model");
        assert_eq!(canonical_key(&repo("Qwen/Qwen3-8B-GGUF")), "qwen/qwen3-8b");
        assert_eq!(
            canonical_key(&quantized("unsloth/Qwen3-8B-GGUF", "Qwen/Qwen3-8B")),
            "qwen/qwen3-8b"
        );
        assert_eq!(
            canonical_key(&repo("google/gemma-4-12B-it-qat-q4_0-gguf")),
            "google/gemma-4-12b-it"
        );
        assert_eq!(
            canonical_key(&quantized(
                "unsloth/gemma-4-12b-it-GGUF",
                "google/gemma-4-12b-it-qat-q4_0-unquantized"
            )),
            "google/gemma-4-12b-it"
        );
        assert_eq!(
            canonical_key(&repo("mradermacher/Qwen3-8B-i1-GGUF")),
            "mradermacher/qwen3-8b"
        );
    }

    /// Règle de l'accueil, point par point (35 § 7).
    #[test]
    fn the_home_rule() {
        assert!(curated(&repo("Qwen/Qwen3-8B-GGUF")));
        assert!(curated(&quantized(
            "unsloth/Qwen3-8B-GGUF",
            "Qwen/Qwen3-8B"
        )));
        // 1) Origine : quantificateur sans base de confiance, inconnu, exclu.
        assert!(!curated(&repo("unsloth/Qwen3-8B-GGUF")));
        assert!(!curated(&quantized(
            "unsloth/Ornith-9B-GGUF",
            "ornith-ai/Ornith-9B"
        )));
        assert!(!curated(&repo("ornith-ai/Ornith-1.5-9B-GGUF")));
        assert!(!curated(&quantized(
            "TheBloke/Qwen3-8B-GGUF",
            "Qwen/Qwen3-8B"
        )));
        // 2) Architecture, conversation.
        for architecture in [None, Some("xing4_0"), Some("bert"), Some("clip")] {
            let other = Repo {
                architecture: architecture.map(str::to_owned),
                ..repo("Qwen/Qwen3-8B-GGUF")
            };
            assert!(!curated(&other), "{architecture:?}");
            assert!(!usable(&other), "{architecture:?}");
        }
        let speech = Repo {
            pipeline_tag: Some("automatic-speech-recognition".into()),
            ..repo("microsoft/VibeVoice-GGUF")
        };
        assert!(!curated(&speech));
        // 3) Mot exclu (la recherche les garde).
        let abliterated = quantized("unsloth/Qwen3-8B-abliterated-GGUF", "Qwen/Qwen3-8B");
        assert!(!curated(&abliterated));
        assert!(usable(&abliterated));
        // 4) Audience.
        let unknown = Repo {
            downloads: 999,
            likes: 49,
            ..repo("Qwen/Qwen3-8B-GGUF")
        };
        assert!(!curated(&unknown));
        assert!(usable(&unknown));
        assert!(curated(&Repo {
            likes: 50,
            ..unknown.clone()
        }));
        // 5) Version téléchargeable : que des morceaux.
        let sharded = Repo {
            files: vec![
                RepoFile {
                    name: "Q4_K_M/x-Q4_K_M-00001-of-00002.gguf".into(),
                    size: None,
                },
                RepoFile {
                    name: "Q4_K_M/x-Q4_K_M-00002-of-00002.gguf".into(),
                    size: None,
                },
            ],
            ..repo("Qwen/Qwen3-235B-GGUF")
        };
        assert!(!curated(&sharded));
        assert!(!usable(&sharded));
        // 6) Restreint, privé.
        for other in [
            Repo {
                gated: true,
                ..repo("google/gemma-2b-GGUF")
            },
            Repo {
                private: true,
                ..repo("google/gemma-2b-GGUF")
            },
        ] {
            assert!(!curated(&other));
            assert!(!usable(&other));
        }
    }

    #[test]
    fn duplicates_keep_the_lab_then_the_most_downloaded_quantizer() {
        let lab = Repo {
            downloads: 5_000,
            ..quantized("Qwen/Qwen3-8B-GGUF", "Qwen/Qwen3-8B")
        };
        let unsloth = Repo {
            downloads: 900_000,
            trending: 40.0,
            ..quantized("unsloth/Qwen3-8B-GGUF", "Qwen/Qwen3-8B")
        };
        let lmstudio = Repo {
            downloads: 1_000_000,
            ..quantized("lmstudio-community/Qwen3-8B-GGUF", "Qwen/Qwen3-8B")
        };
        let mradermacher = Repo {
            downloads: 9_000_000,
            ..quantized("mradermacher/Qwen3-8B-i1-GGUF", "Qwen/Qwen3-8B")
        };
        let all = [
            unsloth.clone(),
            lab.clone(),
            lmstudio.clone(),
            mradermacher.clone(),
        ];
        let found = entries(&all, std::slice::from_ref(&unsloth), None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].model.id, "Qwen/Qwen3-8B-GGUF");
        assert_eq!(found[0].trending, Some(40.0));
        // Sans le laboratoire : le plus téléchargé du rang 1, pas le rang 2.
        let found = entries(&all[2..], &[], None);
        assert_eq!(found[0].model.id, "lmstudio-community/Qwen3-8B-GGUF");
        let found = entries(&[unsloth.clone(), mradermacher.clone()], &[], None);
        assert_eq!(found[0].model.id, "unsloth/Qwen3-8B-GGUF");
        assert_eq!(found[0].trending, None);
        // Même dépôt dans les deux listes : une fois.
        let both = std::slice::from_ref(&lab);
        assert_eq!(entries(both, both, None).len(), 1);
    }

    #[test]
    fn sections_filters_and_families() {
        let coder = Repo {
            capabilities: HubCapabilities {
                code: true,
                tools: true,
                ..Default::default()
            },
            downloads: 500_000,
            total: Some(30_000_000_000),
            ..quantized("unsloth/Qwen3-Coder-30B-GGUF", "Qwen/Qwen3-Coder-30B")
        };
        let small = Repo {
            capabilities: HubCapabilities {
                thinking: true,
                ..Default::default()
            },
            downloads: 800_000,
            total: Some(600_000_000),
            ..repo("Qwen/Qwen3-0.6B-GGUF")
        };
        let gemma = Repo {
            capabilities: HubCapabilities {
                vision: true,
                ..Default::default()
            },
            architecture: Some("gemma4".into()),
            downloads: 100_000,
            total: Some(12_000_000_000),
            license: Some("gemma".into()),
            ..repo("google/gemma-4-12B-it-GGUF")
        };
        let popular = [coder, small, gemma.clone()];
        let gpu = machine(16);
        let all = entries(&popular, &[gemma], Some(&gpu));
        let ids = |models: Vec<HubModel>| models.into_iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(
            ids(section(&all, HubSectionId::Popular, true)),
            [
                "Qwen/Qwen3-0.6B-GGUF",
                "unsloth/Qwen3-Coder-30B-GGUF",
                "google/gemma-4-12B-it-GGUF"
            ]
        );
        assert_eq!(
            ids(section(&all, HubSectionId::Trending, true)),
            ["google/gemma-4-12B-it-GGUF"]
        );
        assert_eq!(
            ids(section(&all, HubSectionId::Coding, true)),
            ["unsloth/Qwen3-Coder-30B-GGUF"]
        );
        assert_eq!(
            ids(section(&all, HubSectionId::Vision, true)),
            ["google/gemma-4-12B-it-GGUF"]
        );
        assert_eq!(
            ids(section(&all, HubSectionId::Reasoning, true)),
            ["Qwen/Qwen3-0.6B-GGUF"]
        );
        assert_eq!(
            ids(section(&all, HubSectionId::Small, true)),
            ["Qwen/Qwen3-0.6B-GGUF"]
        );
        // Pour la machine : par famille, le plus gros qui tient (le 30B ne
        // tient pas dans 16 Gio ; le 0,6B est alors le seul Qwen).
        let coder_fit = all
            .iter()
            .find(|e| e.model.id.contains("Coder"))
            .unwrap()
            .model
            .fit;
        assert_ne!(coder_fit, Some(HubFit::Fits));
        assert_eq!(
            ids(section(&all, HubSectionId::ForYou, true)),
            ["Qwen/Qwen3-0.6B-GGUF", "google/gemma-4-12B-it-GGUF"]
        );
        assert!(section(&all, HubSectionId::ForYou, false).is_empty());
        // Familles par téléchargements cumulés.
        assert_eq!(
            families(&all),
            [
                HubFamily {
                    id: "qwen".into(),
                    label: "Qwen".into(),
                    count: 2
                },
                HubFamily {
                    id: "gemma".into(),
                    label: "Gemma".into(),
                    count: 1
                },
            ]
        );
        assert_eq!(family_models(&all, "gemma").len(), 1);

        // Filtres.
        let model = |id: &str| all.iter().find(|e| e.model.id == id).unwrap().model.clone();
        let filters = |uses: Vec<HubUse>, size: Option<HubSize>, open: bool| HubFilters {
            uses,
            size,
            open_license: open,
            verified_only: false,
        };
        let coder = model("unsloth/Qwen3-Coder-30B-GGUF");
        assert!(matches(
            &coder,
            &filters(vec![HubUse::Code, HubUse::Tools], None, false),
            true
        ));
        assert!(!matches(
            &coder,
            &filters(vec![HubUse::Code, HubUse::Vision], None, false),
            true
        ));
        assert!(matches(
            &coder,
            &filters(vec![], Some(HubSize::Large), true),
            true
        ));
        assert!(!matches(
            &coder,
            &filters(vec![], Some(HubSize::Medium), false),
            true
        ));
        assert!(!matches(
            &coder,
            &filters(vec![], Some(HubSize::Fits), false),
            true
        ));
        // Machine inconnue : « Fits » sans effet.
        assert!(matches(
            &coder,
            &filters(vec![], Some(HubSize::Fits), false),
            false
        ));
        let gemma = model("google/gemma-4-12B-it-GGUF");
        assert!(matches(
            &gemma,
            &filters(vec![], Some(HubSize::Medium), false),
            true
        ));
        assert!(!matches(&gemma, &filters(vec![], None, true), true));
        let small = model("Qwen/Qwen3-0.6B-GGUF");
        assert!(matches(
            &small,
            &filters(vec![HubUse::Reasoning], Some(HubSize::Small), true),
            true
        ));
        let unverified = HubModel {
            trust: HubTrust::Unverified,
            ..small.clone()
        };
        let verified = HubFilters {
            verified_only: true,
            ..HubFilters::default()
        };
        assert!(!matches(&unverified, &verified, true));
        assert!(matches(&small, &verified, true));
    }

    #[test]
    fn the_repository_view_estimates_each_version() {
        let mut detail = quantized("unsloth/Qwen3.5-9B-GGUF", "Qwen/Qwen3.5-9B");
        detail.capabilities.vision = true;
        detail.total = Some(8_953_803_264);
        detail.context_length = Some(262144);
        detail.files = [
            ("Qwen3.5-9B-BF16.gguf", 17_920_697_312u64),
            ("Qwen3.5-9B-Q8_0.gguf", 9_527_502_048),
            ("Qwen3.5-9B-Q4_K_M.gguf", 5_680_522_464),
            ("BF16/Qwen3.5-9B-BF16-00001-of-00002.gguf", 9_000_000_000),
            ("BF16/Qwen3.5-9B-BF16-00002-of-00002.gguf", 8_920_697_312),
            ("mmproj-F16.gguf", 918_166_080),
            ("mmproj-F32.gguf", 1_824_062_016),
        ]
        .into_iter()
        .map(|(name, size)| RepoFile {
            name: name.into(),
            size: Some(size),
        })
        .collect();
        let facts = GgufFacts {
            architecture: Some("qwen35".into()),
            block_count: Some(32),
            // Une couche d'attention sur quatre (qwen35) : 8 couches × 4 têtes.
            head_count_kv: Some(4),
            key_length: Some(256),
            ..Default::default()
        };
        let gpu = HubHardware {
            context: 32768,
            ..machine(16)
        };
        let view = repo_view(
            &detail,
            Some(&facts),
            Some(&gpu),
            vec!["lmstudio-community/x".into()],
        );
        assert_eq!(view.context_used, Some(32768));
        assert_eq!(view.quants.len(), 4);
        let q8 = view.quants.iter().find(|q| q.label == "Q8_0").unwrap();
        assert_eq!(q8.download_size, 9_527_502_048 + 918_166_080);
        // Poids + KV (8 × 4 têtes × 512 × 2 octets × 32768) + 0,5 Gio + 768 Mio.
        let kv = 32 * 512 * 2 * 32768u64;
        assert_eq!(
            q8.memory,
            Some(q8.download_size + kv + (512 << 20) + (768 << 20))
        );
        assert_eq!(q8.fit, Some(HubFit::Fits));
        assert!(q8.recommended);
        assert_eq!(view.quants.iter().filter(|q| q.recommended).count(), 1);
        let bf16 = view
            .quants
            .iter()
            .find(|q| q.label == "BF16" && q.issue.is_none())
            .unwrap();
        assert_eq!(bf16.fit, Some(HubFit::Partial));
        let sharded = view
            .quants
            .iter()
            .find(|q| q.issue == Some(crate::hub::types::HubQuantIssue::Sharded))
            .unwrap();
        assert_eq!(sharded.model, None);
        assert_eq!(view.model.estimated_size, Some(q8.download_size));
        assert_eq!(view.model.fit, Some(HubFit::Fits));
        assert_eq!(view.other_sources, ["lmstudio-community/x"]);

        // Machine inconnue : Q4_K_M conseillée, ni mémoire ni place.
        let remote = repo_view(&detail, Some(&facts), None, vec![]);
        let recommended = remote.quants.iter().find(|q| q.recommended).unwrap();
        assert_eq!(recommended.label, "Q4_K_M");
        assert!(
            remote
                .quants
                .iter()
                .all(|q| q.memory.is_none() && q.fit.is_none())
        );
        assert_eq!(remote.context_used, None);

        // Restreint : rien de téléchargeable ni de conseillé.
        let gated = repo_view(
            &Repo {
                gated: true,
                ..detail.clone()
            },
            None,
            Some(&gpu),
            vec![],
        );
        assert!(gated.gated);
        assert!(
            gated
                .quants
                .iter()
                .all(|q| q.model.is_none() && !q.recommended)
        );
    }

    #[test]
    fn other_sources_are_trusted_copies_of_the_same_model() {
        let this = quantized("unsloth/Qwen3-8B-GGUF", "Qwen/Qwen3-8B");
        let lab = Repo {
            downloads: 5,
            ..quantized("Qwen/Qwen3-8B-GGUF", "Qwen/Qwen3-8B")
        };
        let bartowski = Repo {
            downloads: 50,
            ..quantized("bartowski/Qwen_Qwen3-8B-GGUF", "Qwen/Qwen3-8B")
        };
        let stranger = quantized("someone/Qwen3-8B-GGUF", "Qwen/Qwen3-8B");
        let other_model = quantized("bartowski/Qwen3-4B-GGUF", "Qwen/Qwen3-4B");
        let known = [&this, &lab, &bartowski, &stranger, &other_model, &lab];
        assert_eq!(
            other_sources(&this, &known),
            ["bartowski/Qwen_Qwen3-8B-GGUF", "Qwen/Qwen3-8B-GGUF"]
        );
    }
}
