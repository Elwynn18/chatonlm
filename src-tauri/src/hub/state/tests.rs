//! Essais du catalogue contre le faux Hugging Face (crate `fake-ollama`,
//! module `hf`) : requêtes exactes, cache, données anciennes servies,
//! classement des échecs, quota, une seule requête en vol.

use std::time::Duration;

use fake_ollama::hf::{FakeHf, HfMode, HfRequest};
use tempfile::TempDir;

use super::*;
use crate::hub::fit;
use crate::hub::types::{HubFit, HubGpu, HubQuantIssue, HubSectionId, HubTrust, HubUse};

const GIB: u64 = 1 << 30;

fn machine() -> HubHardware {
    HubHardware {
        gpus: vec![HubGpu {
            name: "NVIDIA GeForce RTX 3090".into(),
            vram_total: 24 * GIB,
            vram_free: Some(20 * GIB),
        }],
        ram_total: 32 * GIB,
        ram_available: 16 * GIB,
        context: 32768,
    }
}

/// Catalogue neuf dans un dossier temporaire, branché sur `url`.
fn open_catalog(url: &str) -> (Catalog, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Catalog::with_urls(dir.path().join("modelhub"), url.into(), url.into());
    (catalog, dir)
}

/// Autre catalogue sur le même cache (redémarrage de l'app, autre serveur).
fn reopen(dir: &TempDir, url: &str) -> Catalog {
    Catalog::with_urls(dir.path().join("modelhub"), url.into(), url.into())
}

/// Adresse d'un serveur arrêté (port fermé : hors ligne).
async fn closed_url() -> String {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let url = fake.url();
    fake.stop().await;
    url
}

fn paths(requests: &[HfRequest]) -> Vec<&str> {
    requests.iter().map(|r| r.path.as_str()).collect()
}

fn ids(models: &[crate::hub::types::HubModel]) -> Vec<&str> {
    models.iter().map(|m| m.id.as_str()).collect()
}

fn section(home: &HubHome, id: HubSectionId) -> Vec<&str> {
    home.sections
        .iter()
        .find(|s| s.id == id)
        .map(|s| ids(&s.models))
        .unwrap_or_default()
}

/// Vieillit les deux listes de l'accueil.
async fn age_lists(catalog: &Catalog, ms: i64) {
    for list in [List::Popular, List::Trending] {
        catalog.cache.age::<ListPage>(LISTS, &list.key(), ms).await;
    }
}

#[tokio::test]
async fn the_home_comes_from_two_exact_requests_then_from_the_cache() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let (catalog, dir) = open_catalog(&fake.url());
    let home = catalog
        .home(&HubFilters::default(), Some(&machine()), false)
        .await;
    assert_eq!(home.status.problem, None);
    assert!(home.status.fetched_at.is_some());
    let mut requested = paths(&fake.requests())
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    requested.sort();
    assert_eq!(
        requested,
        [
            format!("/api/models?apps=ollama&sort=downloads&limit=500&{E_LIST}"),
            format!("/api/models?apps=ollama&sort=trendingScore&limit=200&{E_LIST}"),
        ]
    );
    assert!(E_LIST.contains("expand[]=siblings") && !E_LIST.contains("cardData"));

    // Règles et dédoublonnage : un représentant par modèle de base.
    let popular = section(&home, HubSectionId::Popular);
    assert_eq!(popular.len(), 6);
    let browse = catalog
        .browse(
            &HubScope::Section {
                id: HubSectionId::Popular,
            },
            &HubFilters::default(),
            Some(&machine()),
            false,
        )
        .await;
    let all = ids(&browse.models);
    for kept in [
        "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF",
        "unsloth/Qwen3.5-9B-GGUF",
        "Qwen/Qwen3-8B-GGUF",
        "bartowski/Qwen3.8-27B-GGUF",
        "ggml-org/gpt-oss-20b-GGUF",
        "google/gemma-4-12B-it-qat-q4_0-gguf",
        "LiquidAI/LFM2.5-2.6B-GGUF",
        "bartowski/Meta-Llama-3.1-8B-Instruct-GGUF",
        "unsloth/DeepSeek-R1-0528-Qwen3-8B-GGUF",
    ] {
        assert!(all.contains(&kept), "{kept} missing from {all:?}");
    }
    assert_eq!(all.len(), 9, "{all:?}");
    // Doublons (laboratoire puis le plus téléchargé), non vérifié, que des
    // morceaux, architecture inconnue, mot exclu, restreint, audience,
    // reconnaissance vocale : absents.
    assert_eq!(all[0], "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF");
    assert_eq!(browse.next_cursor, None);

    assert_eq!(
        section(&home, HubSectionId::Trending)[..3],
        [
            "unsloth/Qwen3.5-9B-GGUF",
            "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF",
            "google/gemma-4-12B-it-qat-q4_0-gguf"
        ]
    );
    assert_eq!(
        section(&home, HubSectionId::Coding),
        ["unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF"]
    );
    assert_eq!(
        section(&home, HubSectionId::Vision),
        [
            "unsloth/Qwen3.5-9B-GGUF",
            "google/gemma-4-12B-it-qat-q4_0-gguf",
            "bartowski/Qwen3.8-27B-GGUF"
        ]
    );
    // Distillation R1 : septième des modèles qui raisonnent.
    assert_eq!(section(&home, HubSectionId::Reasoning).len(), 6);
    let reasoning = catalog
        .browse(
            &HubScope::Section {
                id: HubSectionId::Reasoning,
            },
            &HubFilters::default(),
            None,
            false,
        )
        .await;
    assert_eq!(
        ids(&reasoning.models).last(),
        Some(&"unsloth/DeepSeek-R1-0528-Qwen3-8B-GGUF")
    );
    assert_eq!(
        section(&home, HubSectionId::Small),
        ["LiquidAI/LFM2.5-2.6B-GGUF"]
    );
    assert!(!section(&home, HubSectionId::ForYou).is_empty());
    assert_eq!(home.sections[0].id, HubSectionId::ForYou);
    let families: Vec<(&str, u32)> = home
        .families
        .iter()
        .map(|f| (f.id.as_str(), f.count))
        .collect();
    assert_eq!(families[0], ("qwen", 4));
    for family in ["gpt-oss", "gemma", "lfm", "llama", "deepseek"] {
        assert!(families.contains(&(family, 1)), "{family}: {families:?}");
    }
    let qwen = catalog
        .browse(
            &HubScope::Family { id: "qwen".into() },
            &HubFilters::default(),
            None,
            false,
        )
        .await;
    assert_eq!(qwen.models.len(), 4);

    // Rien de plus sur le réseau : listes fraîches (mémoire puis disque).
    catalog.home(&HubFilters::default(), None, false).await;
    let again = reopen(&dir, &fake.url());
    let cached = again
        .home(&HubFilters::default(), Some(&machine()), false)
        .await;
    assert_eq!(cached.sections, home.sections);
    assert_eq!(fake.requests().len(), 2);

    // Cache normalisé : jamais le gabarit de conversation.
    for file in catalog.cache.files(LISTS) {
        let text = std::fs::read_to_string(file).unwrap();
        assert!(!text.contains("chat_template") && !text.contains("add_generation_prompt"));
    }
}

/// Rubrique « Uncensored » : aucune requête tant qu'elle n'est pas montrée ;
/// montrée, trois recherches (puis le cache), en dernier, un dépôt par
/// dérivé de lignée officielle ; les autres rubriques n'en contiennent pas.
#[tokio::test]
async fn the_uncensored_section_comes_only_when_shown() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let (catalog, _dir) = open_catalog(&fake.url());
    let filters = HubFilters::default();
    let scope = HubScope::Section {
        id: HubSectionId::Uncensored,
    };
    let hidden = catalog.home(&filters, Some(&machine()), false).await;
    assert!(
        hidden
            .sections
            .iter()
            .all(|s| s.id != HubSectionId::Uncensored)
    );
    let none = catalog
        .browse(&scope, &filters, Some(&machine()), false)
        .await;
    assert!(none.models.is_empty() && none.status == HubStatus::default());
    assert_eq!(fake.requests().len(), 2);

    let shown = catalog.home(&filters, Some(&machine()), true).await;
    assert_eq!(shown.status.problem, None);
    let mut searches: Vec<String> = paths(&fake.requests()[2..])
        .into_iter()
        .map(str::to_owned)
        .collect();
    searches.sort();
    assert_eq!(
        searches,
        ["abliterated", "heretic", "uncensored"].map(|query| format!(
            "/api/models?apps=ollama&search={query}&sort=downloads&limit=100&{E_LIST}"
        ))
    );
    let last = shown.sections.last().unwrap();
    assert_eq!(last.id, HubSectionId::Uncensored);
    // Base officielle (tout publieur), puis un dérivé de huihui-ai : une
    // carte, bartowski devant mradermacher. Refusés : fusion communautaire,
    // pour adultes, audience, encodeur d'images.
    let expected = [
        "huihui-ai/Huihui-Qwen3.5-9B-abliterated-GGUF",
        "unsloth/Qwen3-4B-Uncensored-GGUF",
        "bartowski/huihui-ai_Huihui-Qwen3-14B-abliterated-v2-GGUF",
    ];
    assert_eq!(ids(&last.models), expected);
    assert!(
        last.models
            .iter()
            .all(|m| m.family.as_deref() == Some("qwen"))
    );
    assert_eq!(last.models[0].trust, HubTrust::Unverified);
    assert_eq!(shown.families, hidden.families);
    assert_eq!(
        &shown.sections[..shown.sections.len() - 1],
        &hidden.sections[..]
    );
    for section in &hidden.sections {
        for model in &section.models {
            assert!(!expected.contains(&model.id.as_str()), "{}", model.id);
        }
    }

    // Liste complète, filtres, puis le cache seulement.
    let all = catalog
        .browse(&scope, &filters, Some(&machine()), true)
        .await;
    assert_eq!(ids(&all.models), expected);
    let small = HubFilters {
        size: Some(crate::hub::types::HubSize::Small),
        ..Default::default()
    };
    assert!(
        catalog
            .browse(&scope, &small, Some(&machine()), true)
            .await
            .models
            .is_empty()
    );
    catalog.home(&filters, None, true).await;
    assert_eq!(fake.requests().len(), 5);
}

#[tokio::test]
async fn filters_apply_to_sections_and_families() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let (catalog, _dir) = open_catalog(&fake.url());
    let filters = HubFilters {
        uses: vec![HubUse::Vision, HubUse::Reasoning],
        ..Default::default()
    };
    let home = catalog.home(&filters, Some(&machine()), false).await;
    let popular = section(&home, HubSectionId::Popular);
    assert_eq!(
        popular,
        [
            "unsloth/Qwen3.5-9B-GGUF",
            "google/gemma-4-12B-it-qat-q4_0-gguf",
            "bartowski/Qwen3.8-27B-GGUF"
        ]
    );
    assert!(section(&home, HubSectionId::Coding).is_empty());
    assert!(home.sections.iter().all(|s| !s.models.is_empty()));
    let families: Vec<&str> = home.families.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(families, ["qwen", "gemma"]);
    // « Fits my machine » : sans machine, sans effet.
    let fits = HubFilters {
        size: Some(crate::hub::types::HubSize::Fits),
        ..Default::default()
    };
    let remote = catalog.home(&fits, None, false).await;
    assert_eq!(section(&remote, HubSectionId::Popular).len(), 6);
    assert!(section(&remote, HubSectionId::ForYou).is_empty());
    let local = catalog.home(&fits, Some(&machine()), false).await;
    let fitting = catalog
        .browse(
            &HubScope::Section {
                id: HubSectionId::Popular,
            },
            &fits,
            Some(&machine()),
            false,
        )
        .await;
    assert!(fitting.models.iter().all(|m| m.fit == Some(HubFit::Fits)));
    assert!(fitting.models.len() < 9);
    assert!(!section(&local, HubSectionId::ForYou).is_empty());
}

#[tokio::test]
async fn simultaneous_homes_share_their_requests() {
    let fake = FakeHf::start_with(
        "127.0.0.1:0".parse().unwrap(),
        HfMode::Normal,
        Duration::from_millis(300),
    )
    .await
    .unwrap();
    let (catalog, _dir) = open_catalog(&fake.url());
    let filters = HubFilters::default();
    let qwen = HubScope::Family { id: "qwen".into() };
    let (a, b, c) = tokio::join!(
        catalog.home(&filters, None, false),
        catalog.home(&filters, None, false),
        catalog.browse(&qwen, &filters, None, false)
    );
    assert_eq!(a.sections, b.sections);
    assert_eq!(c.models.len(), 4);
    assert_eq!(fake.requests().len(), 2);
}

#[tokio::test]
async fn offline_without_cache_gives_empty_lists_and_the_problem() {
    let (catalog, _dir) = open_catalog(&closed_url().await);
    let home = catalog.home(&HubFilters::default(), None, false).await;
    assert!(home.sections.is_empty() && home.families.is_empty());
    assert_eq!(
        home.status,
        HubStatus {
            fetched_at: None,
            problem: Some(HubProblem::Offline)
        }
    );
    let search = catalog
        .search(
            &HubSearch {
                text: "qwen".into(),
                filters: HubFilters::default(),
                cursor: None,
            },
            None,
        )
        .await;
    assert!(search.models.is_empty());
    assert_eq!(search.status.problem, Some(HubProblem::Offline));
    let repo = catalog.repo("Qwen/Qwen3-8B-GGUF", None).await;
    assert_eq!(repo.repo, None);
    assert_eq!(repo.status.problem, Some(HubProblem::Offline));
}

/// Données anciennes + échec : les données, leur date et le problème ; le
/// cache n'est jamais vidé.
#[tokio::test]
async fn stale_data_is_served_with_the_problem() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let (catalog, dir) = open_catalog(&fake.url());
    let fresh = catalog.home(&HubFilters::default(), None, false).await;
    age_lists(&catalog, 25 * HOUR).await;
    let saved_at = fresh.status.fetched_at.unwrap() - 25 * HOUR;

    let offline = reopen(&dir, &closed_url().await);
    let home = offline.home(&HubFilters::default(), None, false).await;
    assert_eq!(home.sections, fresh.sections);
    assert_eq!(
        home.status,
        HubStatus {
            fetched_at: Some(saved_at),
            problem: Some(HubProblem::Offline)
        }
    );

    let down = FakeHf::start(HfMode::Status(503)).await.unwrap();
    let home = reopen(&dir, &down.url())
        .home(&HubFilters::default(), None, false)
        .await;
    assert_eq!(home.sections, fresh.sections);
    assert_eq!(
        home.status.problem,
        Some(HubProblem::Unavailable { incident: true })
    );
    // La page d'état n'est lue qu'après l'échec, une fois.
    let status_reads = down
        .requests()
        .iter()
        .filter(|r| r.path == "/index.json")
        .count();
    assert_eq!(status_reads, 1);

    let html = FakeHf::start(HfMode::Html).await.unwrap();
    let home = reopen(&dir, &html.url())
        .home(&HubFilters::default(), None, false)
        .await;
    assert_eq!(home.sections, fresh.sections);
    assert_eq!(
        home.status.problem,
        Some(HubProblem::Unavailable { incident: false })
    );

    let refused = FakeHf::start(HfMode::Status(403)).await.unwrap();
    let home = reopen(&dir, &refused.url())
        .home(&HubFilters::default(), None, false)
        .await;
    assert_eq!(
        home.status.problem,
        Some(HubProblem::Failed {
            message: "Something went wrong.".into()
        })
    );
    // Toujours là après tous ces échecs.
    let back = reopen(&dir, &closed_url().await);
    assert_eq!(
        back.home(&HubFilters::default(), None, false)
            .await
            .sections,
        fresh.sections
    );
}

/// Copie périmée : revalidation par ETag ; un 304 rafraîchit la date.
#[tokio::test]
async fn stale_lists_are_revalidated_with_their_etag() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let (catalog, _dir) = open_catalog(&fake.url());
    let first = catalog.home(&HubFilters::default(), None, false).await;
    assert!(fake.requests().iter().all(|r| r.if_none_match.is_none()));
    age_lists(&catalog, 7 * HOUR).await;
    // Tendances périmées (6 h), populaires encore fraîches (24 h).
    let second = catalog.home(&HubFilters::default(), None, false).await;
    let requests = fake.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].path.contains("sort=trendingScore"));
    assert!(requests[2].if_none_match.is_some());
    assert_eq!(second.sections, first.sections);
    assert_eq!(second.status.problem, None);
    let trending = catalog
        .cache
        .read::<ListPage>(LISTS, &List::Trending.key())
        .await
        .unwrap();
    assert!(trending.fetched_at >= first.status.fetched_at.unwrap());
}

/// Délai dépassé, connexion établie (Hugging Face muet) : `Unavailable`, pas
/// `Offline`, après lecture de la page d'état (une fois pour les deux
/// listes), qui signale ici un incident.
#[tokio::test]
async fn a_silent_hugging_face_is_unavailable() {
    let silent = FakeHf::start(HfMode::Silent).await.unwrap();
    let (catalog, _dir) = open_catalog(&silent.url());
    let catalog = catalog.with_timeout(Duration::from_millis(300));
    let home = catalog.home(&HubFilters::default(), None, false).await;
    assert!(home.sections.is_empty());
    assert_eq!(
        home.status.problem,
        Some(HubProblem::Unavailable { incident: true })
    );
    let requests = silent.requests();
    assert_eq!(
        requests.iter().filter(|r| r.path == "/index.json").count(),
        1,
        "{:?}",
        paths(&requests)
    );
    let repo = catalog.repo("Qwen/Qwen3-8B-GGUF", None).await;
    assert_eq!(repo.repo, None);
    assert_eq!(
        repo.status.problem,
        Some(HubProblem::Unavailable { incident: true })
    );
}

/// Copie datée dans le futur (horloge recalée en arrière, cache d'une autre
/// machine) : périmée, relue tout de suite.
#[tokio::test]
async fn a_copy_dated_in_the_future_is_stale() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let (catalog, _dir) = open_catalog(&fake.url());
    catalog.home(&HubFilters::default(), None, false).await;
    assert_eq!(fake.requests().len(), 2);
    age_lists(&catalog, -2 * HOUR).await;
    let home = catalog.home(&HubFilters::default(), None, false).await;
    assert_eq!(home.status.problem, None);
    let requests = fake.requests();
    assert_eq!(requests.len(), 4, "{:?}", paths(&requests));
    assert!(requests[2..].iter().all(|r| r.if_none_match.is_some()));
    assert!(home.status.fetched_at.unwrap() <= now_ms());
}

/// 429 : plus aucune requête avant l'échéance, puis reprise.
#[tokio::test]
async fn the_rate_limit_stops_every_request_until_it_ends() {
    let limited = FakeHf::start(HfMode::RateLimited { reset: 1 })
        .await
        .unwrap();
    let (catalog, _dir) = open_catalog(&limited.url());
    let home = catalog.home(&HubFilters::default(), None, false).await;
    assert_eq!(
        home.status.problem,
        Some(HubProblem::RateLimited { retry_after: 1 })
    );
    let sent = limited.requests().len();
    assert!((1..=2).contains(&sent), "{sent}");
    // Ni l'accueil, ni la recherche, ni une fiche, ni un README.
    catalog.home(&HubFilters::default(), None, false).await;
    let search = catalog
        .search(
            &HubSearch {
                text: "qwen".into(),
                filters: HubFilters::default(),
                cursor: None,
            },
            None,
        )
        .await;
    assert!(matches!(
        search.status.problem,
        Some(HubProblem::RateLimited { retry_after: 1 })
    ));
    catalog.repo("Qwen/Qwen3-8B-GGUF", None).await;
    let readme = catalog.readme("Qwen/Qwen3-8B-GGUF", "main").await;
    assert!(matches!(
        readme.problem,
        Some(HubProblem::RateLimited { .. })
    ));
    assert_eq!(limited.requests().len(), sent);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    catalog.home(&HubFilters::default(), None, false).await;
    assert!(limited.requests().len() > sent);

    // Échéance lue dans `RateLimit` (mode de la ligne de commande : 42 s).
    let long = FakeHf::start(HfMode::parse("hf-ratelimit").unwrap())
        .await
        .unwrap();
    let (catalog, _dir) = open_catalog(&long.url());
    let home = catalog.home(&HubFilters::default(), None, false).await;
    assert_eq!(
        home.status.problem,
        Some(HubProblem::RateLimited { retry_after: 42 })
    );
    let again = catalog.home(&HubFilters::default(), None, false).await;
    let Some(HubProblem::RateLimited { retry_after }) = again.status.problem else {
        panic!("still limited");
    };
    assert!((41..=42).contains(&retry_after));
}

#[tokio::test]
async fn search_keeps_unverified_publishers_and_excluded_words() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let (catalog, _dir) = open_catalog(&fake.url());
    let search = |text: &str, filters: HubFilters, cursor: Option<&str>| HubSearch {
        text: text.into(),
        filters,
        cursor: cursor.map(str::to_owned),
    };
    let qwen = catalog
        .search(&search("  Qwen  ", HubFilters::default(), None), None)
        .await;
    assert_eq!(qwen.status.problem, None);
    let found = ids(&qwen.models);
    // Ordre de Hugging Face (téléchargements), sans dédoublonnage.
    assert_eq!(found[0], "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF");
    assert!(found.contains(&"unsloth/Qwen3-8B-GGUF") && found.contains(&"Qwen/Qwen3-8B-GGUF"));
    assert!(found.contains(&"unsloth/Qwen3-4B-Uncensored-GGUF"));
    // Que des morceaux : écarté partout.
    assert!(!found.contains(&"unsloth/Qwen3.8-Flash-Next-GGUF"));
    let request = &fake.requests()[0].path;
    assert_eq!(
        request,
        &format!("/api/models?apps=ollama&search=Qwen&sort=downloads&limit=30&{E_LIST}")
    );

    let ornith = catalog
        .search(&search("ornith", HubFilters::default(), None), None)
        .await;
    assert_eq!(ornith.models.len(), 1);
    assert_eq!(ornith.models[0].trust, HubTrust::Unverified);
    let verified = HubFilters {
        verified_only: true,
        ..Default::default()
    };
    assert!(
        catalog
            .search(&search("ornith", verified.clone(), None), None)
            .await
            .models
            .is_empty()
    );
    let granite = catalog
        .search(&search("granite tiny", HubFilters::default(), None), None)
        .await;
    assert_eq!(ids(&granite.models), ["ibm-granite/granite-4.2-tiny-GGUF"]);
    assert_eq!(granite.models[0].trust, HubTrust::Lab);
    for excluded in ["xing", "gemma-2-2b", "vibevoice", "tiny-sharded"] {
        let result = catalog
            .search(&search(excluded, HubFilters::default(), None), None)
            .await;
        assert!(result.models.is_empty(), "{excluded}");
        assert_eq!(result.status.problem, None);
    }
    let bloke = catalog
        .search(&search("mistral", HubFilters::default(), None), None)
        .await;
    assert_eq!(bloke.models[0].trust, HubTrust::Unverified);
    assert!(
        catalog
            .search(&search("mistral", verified, None), None)
            .await
            .models
            .is_empty()
    );

    // Page suivante : le curseur de Hugging Face, renvoyé tel quel.
    let before = fake.requests().len();
    let rest = catalog
        .search(
            &search("gguf", HubFilters::default(), Some("page-20")),
            None,
        )
        .await;
    // Curseur encodé (ceux de Hugging Face sont en base64).
    assert!(fake.requests()[before].path.ends_with("&cursor=page%2D20"));
    assert_eq!(rest.next_cursor, None);
    assert!(!rest.models.is_empty());

    // Même recherche (autre casse, autres espaces) : le cache.
    let count = fake.requests().len();
    catalog
        .search(&search("qwen", HubFilters::default(), None), None)
        .await;
    assert_eq!(fake.requests().len(), count);
    // Texte vide : rien, sans requête.
    let empty = catalog
        .search(&search("   ", HubFilters::default(), None), None)
        .await;
    assert!(empty.models.is_empty() && empty.status == HubStatus::default());
    assert_eq!(fake.requests().len(), count);
}

/// Un fichier au nom non ASCII (« 说明.md » dans chatonlm/Tiny-Chat-GGUF)
/// ne fait tomber ni la liste, ni la recherche, ni la fiche.
#[tokio::test]
async fn a_non_ascii_file_name_breaks_nothing() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let (catalog, _dir) = open_catalog(&fake.url());
    let home = catalog.home(&HubFilters::default(), None, false).await;
    assert_eq!(home.status.problem, None);
    assert!(!home.sections.is_empty());
    let found = catalog
        .search(
            &HubSearch {
                text: "tiny-chat".into(),
                filters: HubFilters::default(),
                cursor: None,
            },
            None,
        )
        .await;
    assert_eq!(found.status.problem, None);
    assert_eq!(ids(&found.models), ["chatonlm/Tiny-Chat-GGUF"]);
    let result = catalog
        .repo("chatonlm/Tiny-Chat-GGUF", Some(&machine()))
        .await;
    assert_eq!(result.status.problem, None);
    let repo = result.repo.unwrap();
    assert_eq!(repo.quants.len(), 3);
    assert!(repo.quants.iter().all(|q| q.file.ends_with(".gguf")));
}

#[tokio::test]
async fn a_repository_view_reads_the_gguf_header_once() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let (catalog, _dir) = open_catalog(&fake.url());
    catalog.home(&HubFilters::default(), None, false).await;
    let before = fake.requests().len();
    let result = catalog
        .repo("unsloth/Qwen3.5-9B-GGUF", Some(&machine()))
        .await;
    assert_eq!(result.status.problem, None);
    let repo = result.repo.unwrap();
    let requests = fake.requests()[before..].to_vec();
    assert_eq!(
        paths(&requests),
        [
            format!("/api/models/unsloth/Qwen3.5-9B-GGUF?{E_DETAIL}").as_str(),
            // Plus petit fichier téléchargeable, au commit de la fiche.
            "/unsloth/Qwen3.5-9B-GGUF/resolve/3333333333333333333333333333333333333333/Qwen3.5-9B-IQ4_XS.gguf",
        ]
    );
    assert_eq!(requests[1].range.as_deref(), Some("bytes=0-262143"));
    assert_eq!(
        repo.sha.as_deref(),
        Some("3333333333333333333333333333333333333333")
    );
    assert_eq!(repo.context_used, Some(32768));
    assert!(repo.model.capabilities.vision);
    assert_eq!(repo.other_sources, ["lmstudio-community/Qwen3.5-9B-GGUF"]);
    // Versions du plus gros au plus petit, projecteur F16 compté.
    let labels: Vec<&str> = repo.quants.iter().map(|q| q.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "BF16",
            "Q8_0",
            "UD-Q6_K_XL",
            "Q6_K",
            "UD-Q4_K_XL",
            "Q4_K_M",
            "IQ4_XS"
        ]
    );
    let q8 = &repo.quants[1];
    assert_eq!(q8.download_size, 9527502048 + 918166080);
    // En-tête : une couche d'attention sur quatre (qwen35, intervalle par
    // défaut de llama.cpp), soit 8 couches × 4 têtes kv × (256 + 256) × 2
    // octets par jeton.
    let kv = 8 * 4 * 512 * 2 * 32768u64;
    assert_eq!(
        q8.memory,
        Some(q8.download_size + kv + GIB / 2 + 768 * 1024 * 1024)
    );
    assert_eq!(q8.fit, Some(HubFit::Fits));
    assert!(q8.recommended);
    assert_eq!(repo.model.estimated_size, Some(q8.download_size));
    assert_eq!(
        q8.model.as_deref(),
        Some("hf.co/unsloth/Qwen3.5-9B-GGUF:Q8_0")
    );
    assert_eq!(
        repo.quants[3].model.as_deref(),
        Some("hf.co/unsloth/Qwen3.5-9B-GGUF:Qwen3.5-9B-Q6_K.gguf")
    );

    // Fiche et en-tête en cache : aucune requête.
    let again = catalog
        .repo("unsloth/Qwen3.5-9B-GGUF", Some(&machine()))
        .await;
    assert_eq!(again.repo.unwrap(), repo);
    assert_eq!(fake.requests().len(), before + 2);

    // Sans machine (serveur distant) : pas d'en-tête, Q4_K_M conseillée.
    let (remote_catalog, _other) = open_catalog(&fake.url());
    let count = fake.requests().len();
    let remote = remote_catalog
        .repo("unsloth/Qwen3.5-9B-GGUF", None)
        .await
        .repo
        .unwrap();
    assert_eq!(fake.requests().len(), count + 1);
    assert!(
        remote
            .quants
            .iter()
            .all(|q| q.fit.is_none() && q.memory.is_none())
    );
    assert_eq!(
        remote.quants.iter().find(|q| q.recommended).unwrap().label,
        "Q4_K_M"
    );
}

#[tokio::test]
async fn unreadable_headers_fall_back_to_the_estimate_without_retrying() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let (catalog, _dir) = open_catalog(&fake.url());
    let first = catalog
        .repo("LiquidAI/LFM2.5-2.6B-GGUF", Some(&machine()))
        .await
        .repo
        .unwrap();
    let requests = fake.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].path.contains("/resolve/"));
    // Vocabulaire avant l'architecture : estimation d'après les paramètres.
    let q4 = first.quants.iter().find(|q| q.label == "Q4_K_M").unwrap();
    let context = 32768u64;
    let kv = context * (100 * 1024 + 2_600_000_000 * 33_792 / 10_000_000_000);
    assert_eq!(
        q4.memory,
        Some(q4.download_size + kv + GIB / 2 + 768 * 1024 * 1024)
    );
    // Licence « other » précisée par la fiche, langues.
    assert_eq!(first.model.license.as_deref(), Some("lfm1.0"));
    assert_eq!(first.languages[..2], ["en", "fr"]);
    catalog
        .repo("LiquidAI/LFM2.5-2.6B-GGUF", Some(&machine()))
        .await;
    assert_eq!(fake.requests().len(), 2);
}

#[tokio::test]
async fn restricted_missing_and_invalid_repositories() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let (catalog, _dir) = open_catalog(&fake.url());
    let gated = catalog
        .repo("google/gemma-2-2b-it-GGUF", Some(&machine()))
        .await;
    let repo = gated.repo.unwrap();
    assert!(repo.gated);
    assert!(
        repo.quants
            .iter()
            .all(|q| q.model.is_none() && !q.recommended)
    );
    // Pas d'en-tête pour un dépôt restreint.
    assert_eq!(fake.requests().len(), 1);

    let missing = catalog.repo("nobody/missing-GGUF", None).await;
    assert_eq!(missing.repo, None);
    assert_eq!(missing.status.problem, Some(HubProblem::NotFound));

    // Sharded seulement : fiche montrée, rien de téléchargeable.
    let sharded = catalog
        .repo("chatonlm/Tiny-Sharded-GGUF", None)
        .await
        .repo
        .unwrap();
    assert_eq!(sharded.quants.len(), 1);
    assert_eq!(sharded.quants[0].issue, Some(HubQuantIssue::Sharded));
    assert!(sharded.quants.iter().all(|q| !q.recommended));

    let count = fake.requests().len();
    for id in ["bart.owski/x", "a/b/c", "noslash", "a/-b", ""] {
        let result = catalog.repo(id, None).await;
        assert_eq!(result.status.problem, Some(HubProblem::NotFound), "{id}");
    }
    assert_eq!(fake.requests().len(), count);
}

/// Un en-tête gardé sous un format antérieur (sans les couches récurrentes ni
/// les fenêtres glissantes) est relu une fois, puis gardé.
#[tokio::test]
async fn a_header_of_an_older_format_is_read_again() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let (catalog, _dir) = open_catalog(&fake.url());
    let id = "unsloth/Qwen3.5-9B-GGUF";
    catalog.repo(id, Some(&machine())).await;
    let key = "repo:unsloth/qwen3.5-9b-gguf";
    let stored = catalog.cache.read::<RepoData>(REPOS, key).await.unwrap();
    let mut old = (*stored).clone();
    old.data.header.as_mut().unwrap().format = 0;
    catalog.cache.write(REPOS, key, Arc::new(old)).await;
    let ranges = |fake: &FakeHf| {
        fake.requests()
            .iter()
            .filter(|request| request.range.is_some())
            .count()
    };
    let before = ranges(&fake);
    catalog.repo(id, Some(&machine())).await;
    assert_eq!(ranges(&fake), before + 1);
    let stored = catalog.cache.read::<RepoData>(REPOS, key).await.unwrap();
    assert_eq!(stored.data.header.as_ref().unwrap().format, gguf::FORMAT);
    catalog.repo(id, Some(&machine())).await;
    assert_eq!(ranges(&fake), before + 1);
}

/// Une liste plus récente qui annonce une autre modification périme la fiche.
#[tokio::test]
async fn a_newer_list_with_another_date_refreshes_the_repository() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let (catalog, _dir) = open_catalog(&fake.url());
    catalog.repo("Qwen/Qwen3-8B-GGUF", None).await;
    let key = "repo:qwen/qwen3-8b-gguf";
    let stored = catalog.cache.read::<RepoData>(REPOS, key).await.unwrap();
    let mut old = (*stored).clone();
    // Copie d'une version précédente du dépôt (autre date, autre ETag).
    old.data.repo.last_modified = Some("2020-01-01T00:00:00.000Z".into());
    old.etag = Some("W/\"older\"".into());
    old.fetched_at -= HOUR;
    catalog.cache.write(REPOS, key, Arc::new(old)).await;
    // Même date que la liste : gardée ; autre date : relue.
    catalog.home(&HubFilters::default(), None, false).await;
    let before = fake.requests().len();
    let fresh = catalog.repo("Qwen/Qwen3-8B-GGUF", None).await.repo.unwrap();
    assert_eq!(fake.requests().len(), before + 1);
    assert_eq!(
        fresh.model.last_modified.as_deref(),
        Some("2025-05-14T10:00:00.000Z")
    );
    catalog.repo("Qwen/Qwen3-8B-GGUF", None).await;
    assert_eq!(fake.requests().len(), before + 1);
}

#[tokio::test]
async fn readmes() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let (catalog, _dir) = open_catalog(&fake.url());
    let sha = "7c41481f57cb95916b40956ab2f0b9a0e2b9a0d1";
    let readme = catalog.readme("Qwen/Qwen3-8B-GGUF", sha).await;
    assert_eq!(readme.problem, None);
    let text = readme.text.unwrap();
    assert!(text.starts_with("# Qwen3-8B-GGUF"));
    assert_eq!(
        paths(&fake.requests()),
        [format!("/Qwen/Qwen3-8B-GGUF/raw/{sha}/README.md").as_str()]
    );
    // Révision fixe : gardé sans limite de durée.
    catalog
        .cache
        .age::<ReadmeData>(
            READMES,
            &format!("readme:qwen/qwen3-8b-gguf@{sha}"),
            400 * DAY,
        )
        .await;
    assert_eq!(
        catalog
            .readme("Qwen/Qwen3-8B-GGUF", sha)
            .await
            .text
            .unwrap(),
        text
    );
    assert_eq!(fake.requests().len(), 1);

    // Pas de README : rien, sans problème, et rien dans le cache.
    let none = catalog.readme("unsloth/Qwen3.5-9B-GGUF", "main").await;
    assert_eq!(
        none,
        HubReadme {
            text: None,
            problem: None
        }
    );
    assert_eq!(catalog.cache.files(READMES).len(), 1);
    let gated = catalog.readme("google/gemma-2-2b-it-GGUF", "main").await;
    assert_eq!(gated.problem, Some(HubProblem::Restricted));
    let missing = catalog.readme("nobody/missing-GGUF", "main").await;
    assert_eq!(missing.problem, Some(HubProblem::NotFound));
    let count = fake.requests().len();
    for (id, revision) in [
        ("Qwen/Qwen3-8B-GGUF", "../x"),
        ("Qwen/Qwen3-8B-GGUF", ""),
        ("a b/c", "main"),
    ] {
        let result = catalog.readme(id, revision).await;
        assert_eq!(
            result.problem,
            Some(HubProblem::NotFound),
            "{id} {revision}"
        );
    }
    assert_eq!(fake.requests().len(), count);

    // Copie périmée et hors ligne : la copie et le problème.
    let tiny = catalog.readme("chatonlm/Tiny-Chat-GGUF", "main").await;
    let tiny_text = tiny.text.unwrap();
    assert!(tiny_text.starts_with("# Tiny Chat"));
    assert!(
        tiny_text.contains("<script>"),
        "rendu et assainissement : l'interface"
    );
    catalog
        .cache
        .age::<ReadmeData>(READMES, "readme:chatonlm/tiny-chat-gguf@main", 2 * DAY)
        .await;
    let offline = reopen(&_dir, &closed_url().await)
        .readme("chatonlm/Tiny-Chat-GGUF", "main")
        .await;
    assert_eq!(offline.text.as_deref(), Some(tiny_text.as_str()));
    assert_eq!(offline.problem, Some(HubProblem::Offline));
}

#[tokio::test]
async fn a_remote_server_has_no_machine() {
    let (catalog, _dir) = open_catalog("http://127.0.0.1:1");
    assert_eq!(catalog.hardware("http://192.168.1.20:11434", 0).await, None);
    assert_eq!(catalog.hardware("not an address", 0).await, None);
    assert_eq!(catalog.memory("http://192.168.1.20:11434").await, None);
    assert_eq!(catalog.memory("not an address").await, None);
}

/// Manifeste du registre de Hugging Face, comme le demande Ollama :
/// (statut, JSON).
async fn registry_manifest(id: &str, tag: &str) -> (u16, serde_json::Value) {
    let client = reqwest::Client::builder()
        .user_agent(concat!("ChatonLM/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let response = client
        .get(format!("{}/v2/{id}/manifests/{tag}", names::DEFAULT_HF_URL))
        .header(
            reqwest::header::ACCEPT,
            "application/vnd.docker.distribution.manifest.v2+json",
        )
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = response.bytes().await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

fn real_catalog() -> (Catalog, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Catalog::with_urls(
        dir.path().join("modelhub"),
        names::DEFAULT_HF_URL.into(),
        names::DEFAULT_HF_STATUS_URL.into(),
    );
    (catalog, dir)
}

/// Contrat : les vraies listes, une vraie fiche et son README se lisent ;
/// l'en-tête GGUF du plus petit fichier donne l'estimation.
#[tokio::test]
#[ignore = "huggingface.co : accès au réseau"]
async fn contract_real_lists_repository_and_readme_are_readable() {
    let (catalog, _dir) = real_catalog();
    let home = catalog
        .home(&HubFilters::default(), Some(&machine()), false)
        .await;
    assert_eq!(home.status.problem, None);
    for id in [
        HubSectionId::Trending,
        HubSectionId::Popular,
        HubSectionId::Vision,
    ] {
        assert_eq!(section(&home, id).len(), 6, "{id:?}");
    }
    assert!(home.families.iter().any(|f| f.id == "qwen" && f.count > 1));
    let search = catalog
        .search(
            &HubSearch {
                text: "qwen".into(),
                filters: HubFilters::default(),
                cursor: None,
            },
            None,
        )
        .await;
    assert_eq!(search.status.problem, None);
    assert!(!search.models.is_empty());
    assert!(search.next_cursor.is_some());

    let result = catalog
        .repo("unsloth/Qwen3.5-9B-GGUF", Some(&machine()))
        .await;
    assert_eq!(result.status.problem, None);
    let repo = result.repo.unwrap();
    assert!(repo.model.capabilities.vision && repo.model.capabilities.tools);
    assert_eq!(repo.quants.iter().filter(|q| q.recommended).count(), 1);
    // En-tête lu : cache de contexte d'après le fichier, pas l'estimation.
    let fallback = fit::kv_cache(
        None,
        repo.model.parameters,
        repo.model.architecture.as_deref(),
        32768,
    );
    let q4 = repo.quants.iter().find(|q| q.label == "Q4_K_M").unwrap();
    let header_kv = q4.memory.unwrap() - q4.download_size - fit::margin(32768);
    assert_ne!(header_kv, fallback);
    let readme = catalog
        .readme("unsloth/Qwen3.5-9B-GGUF", repo.sha.as_deref().unwrap())
        .await;
    assert_eq!(readme.problem, None);
    let text = readme.text.unwrap();
    assert!(!text.starts_with("---") && text.len() <= readme::MAX_TEXT);
}

/// Contrat : l'étiquette choisie pour chaque version désigne bien ce fichier
/// au registre (manifeste 200, même taille), pour des publieurs différents.
#[tokio::test]
#[ignore = "huggingface.co : accès au réseau"]
async fn contract_real_headers_follow_hybrid_and_sliding_window_layers() {
    let (catalog, _dir) = real_catalog();
    // Fiches réelles lues le 5 octobre 2026 (34 § 9 ; recette de l'étape 15) :
    // qwen35 (une couche d'attention sur quatre) et Gemma 4 (cinq couches à
    // fenêtre glissante sur six) tiennent dans 24 Gio à 32768 jetons, comme le
    // montre llama-server ; avant la correction, ils « débordaient ».
    for (id, label) in [
        ("unsloth/Qwen3.8-27B-GGUF", "UD-Q4_K_XL"),
        ("google/gemma-4-12B-it-qat-q4_0-gguf", "Q4_0"),
    ] {
        let repo = catalog.repo(id, Some(&machine())).await.repo.unwrap();
        let quant = repo
            .quants
            .iter()
            .find(|q| q.label.eq_ignore_ascii_case(label))
            .unwrap_or_else(|| panic!("{id}: no {label}"));
        let kv = quant.memory.unwrap() - quant.download_size - fit::margin(32768);
        assert!(kv < 3 * GIB, "{id}: {kv} bytes of context cache");
        assert_eq!(quant.fit, Some(HubFit::Fits), "{id} {label}");
    }
}

#[tokio::test]
#[ignore = "huggingface.co : accès au réseau"]
async fn contract_chosen_tags_have_a_manifest() {
    let (catalog, _dir) = real_catalog();
    for (id, labels) in [
        // Libellé, nom de fichier (Q6_K est contenu dans UD-Q6_K_XL).
        ("unsloth/Qwen3.5-9B-GGUF", &["UD-Q4_K_XL", "Q6_K"][..]),
        // Q4_K_L : inconnu de la regex de Hugging Face, nom de fichier.
        ("bartowski/Qwen3.8-27B-GGUF", &["Q4_K_L", "Q3_K_XL"][..]),
        // fp16 : nom de fichier.
        ("MaziyarPanahi/Qwen3-8B-GGUF", &["Q4_K_M", "F16"][..]),
        ("ggml-org/gpt-oss-20b-GGUF", &["MXFP4"][..]),
    ] {
        let repo = catalog.repo(id, None).await.repo.unwrap();
        for label in labels {
            let quant = repo
                .quants
                .iter()
                .find(|q| q.label == *label)
                .unwrap_or_else(|| panic!("{id}: {label}"));
            let model = quant.model.as_deref().unwrap();
            let tag = model.rsplit_once(':').unwrap().1;
            let (status, manifest) = registry_manifest(id, tag).await;
            assert_eq!(status, 200, "{model}: {manifest}");
            let layer = manifest["layers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|l| l["mediaType"] == "application/vnd.ollama.image.model")
                .unwrap();
            assert_eq!(layer["size"].as_u64(), Some(quant.size), "{model}");
        }
    }
}

/// Contrat : une version découpée est refusée par le registre (400
/// « sharded »), d'où `issue: Sharded` et aucun nom.
#[tokio::test]
#[ignore = "huggingface.co : accès au réseau"]
async fn contract_a_sharded_version_is_refused() {
    let (catalog, _dir) = real_catalog();
    let id = "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF";
    let repo = catalog.repo(id, None).await.repo.unwrap();
    let sharded = repo
        .quants
        .iter()
        .find(|q| q.issue == Some(HubQuantIssue::Sharded))
        .unwrap();
    assert_eq!(sharded.model, None);
    let (status, body) = registry_manifest(id, &sharded.label).await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("sharded GGUF"),
        "{body}"
    );
}
