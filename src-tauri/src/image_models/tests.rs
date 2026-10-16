//! Essais des modèles d'images contre le faux Hugging Face
//! (`fake_ollama::hf` : dépôts d'images au vrai contenu, familles d'essai à
//! petites pièces `IMAGE_FAMILIES`) : recherche, fiche, téléchargement réel
//! par la file, pièces partagées, suppression avec références, refus.

use std::time::Duration;

use fake_ollama::hf::{FakeHf, HfMode, IMAGE_FAMILIES, image_blob};
use tokio::time::Instant;

use super::*;
use crate::dictation::downloads::SpeechDownloadState;
use crate::hub::types::HubGpu;
use crate::image_gen::{Component, Config};
use crate::image_models::types::ImageModelInfo;

const WAIT: Duration = Duration::from_secs(10);
const KLEIN: &str = "chatonlm/klein-test-GGUF";
const ZIMAGE: &str = "chatonlm/uncensored-image-GGUF";

struct Setup {
    models: ImageModels,
    downloads: ImageDownloads,
    store: Store,
    fake: FakeHf,
    root: tempfile::TempDir,
    _cache: tempfile::TempDir,
}

/// Machine à une carte de `vram` octets.
fn machine(vram: u64) -> Machine {
    Machine {
        gpus: vec![HubGpu {
            name: "GPU".into(),
            vram_total: vram,
            vram_free: None,
        }],
        ram_total: 16 << 30,
        ram_available: 8 << 30,
    }
}

async fn setup_with(fake: FakeHf, vram: u64) -> Setup {
    setup_families(fake, vram, IMAGE_FAMILIES).await
}

async fn setup_families(fake: FakeHf, vram: u64, families: &str) -> Setup {
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let catalog = ImageCatalog::with_urls(cache.path().to_path_buf(), fake.url(), fake.url());
    let models = ImageModels::for_tests(
        root.path().to_path_buf(),
        catalog,
        Families::parse(families).unwrap(),
        Some(machine(vram)),
    );
    let downloads =
        ImageDownloads::with_base_url(|_| {}, root.path().to_path_buf(), Some(fake.url()));
    let store = Store::open_in_memory().unwrap();
    downloads.record_installs(store.clone());
    Setup {
        models,
        downloads,
        store,
        fake,
        root,
        _cache: cache,
    }
}

async fn setup() -> Setup {
    setup_with(FakeHf::start(HfMode::Normal).await.unwrap(), 1 << 30).await
}

impl Setup {
    fn show_uncensored(&self, shown: bool) {
        self.store
            .blocking(|connection| {
                crate::store::settings::update(
                    connection,
                    crate::store::settings::SettingsPatch {
                        show_uncensored: Some(shown),
                        ..Default::default()
                    },
                    None,
                )
            })
            .unwrap();
    }

    async fn download(&self, repo: &str, file: &str) -> AppResult<()> {
        self.models
            .download(&self.downloads, &self.store, repo, file, "")
            .await
    }

    async fn wait_done(&self, id: &str) {
        let deadline = Instant::now() + WAIT;
        loop {
            let state = self
                .downloads
                .list()
                .into_iter()
                .find(|download| download.id == id)
                .map(|download| (download.state, download.error));
            match state {
                Some((SpeechDownloadState::Done, _)) => {
                    assert!(self.downloads.wait_idle(id, WAIT).await);
                    return;
                }
                Some((SpeechDownloadState::Failed, error)) => panic!("{id}: {error:?}"),
                _ => {}
            }
            assert!(Instant::now() < deadline, "{id}: {state:?}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn list(&self) -> Vec<ImageModelInfo> {
        self.models
            .list(&self.downloads, &self.store)
            .await
            .unwrap()
    }

    async fn info(&self, id: &str) -> ImageModelInfo {
        self.list()
            .await
            .into_iter()
            .find(|model| model.id == id)
            .unwrap_or_else(|| panic!("{id}"))
    }

    /// Requêtes de fichiers reçues par le faux (`resolve/`).
    fn fetched(&self) -> Vec<String> {
        self.fake
            .requests()
            .into_iter()
            .filter(|request| request.path.contains("/resolve/"))
            .map(|request| request.path)
            .collect()
    }

    /// Fichiers rangés sous `blobs/`.
    fn blobs(&self) -> Vec<String> {
        let mut found = Vec::new();
        let Ok(dirs) = std::fs::read_dir(self.root.path().join("blobs")) else {
            return found;
        };
        for dir in dirs.flatten() {
            for file in std::fs::read_dir(dir.path()).unwrap().flatten() {
                found.push(file.file_name().to_string_lossy().into_owned());
            }
        }
        found.sort();
        found
    }
}

fn invalid(result: AppResult<()>) -> String {
    match result {
        Err(AppError::Invalid(message)) => message,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_search_recognizes_families_and_hides_uncensored_models() {
    let setup = setup().await;
    let result = setup
        .models
        .search(&setup.store, "  chatonlm ")
        .await
        .unwrap();
    assert_eq!(result.problem, None);
    assert!(!result.stale);
    let family = |id: &str| {
        result
            .repos
            .iter()
            .find(|repo| repo.id == id)
            .unwrap_or_else(|| panic!("{id}"))
            .family
            .as_ref()
            .map(|family| family.id.clone())
    };
    assert_eq!(family(KLEIN).as_deref(), Some("flux2-klein-4b"));
    // Deuxième cran : la base de sa base.
    assert_eq!(
        family("chatonlm/Tiny-Klein-Finetune-GGUF").as_deref(),
        Some("flux2-klein-4b")
    );
    // Fichier unique, trouvé par la seconde liste.
    assert_eq!(family("chatonlm/Tiny-SDXL").as_deref(), Some("sdxl"));
    assert_eq!(family("chatonlm/unknown-diffusion-GGUF"), None);
    assert_eq!(family("chatonlm/Tiny-LTX-GGUF"), None);
    assert!(
        result
            .repos
            .iter()
            .find(|repo| repo.id == "chatonlm/gated-image-GGUF")
            .unwrap()
            .gated
    );
    // Plus téléchargés d'abord ; non censuré retiré.
    assert_eq!(result.repos[0].id, KLEIN);
    assert!(result.repos.iter().all(|repo| repo.id != ZIMAGE));
    assert!(
        result
            .repos
            .windows(2)
            .all(|pair| pair[0].downloads >= pair[1].downloads)
    );

    setup.show_uncensored(true);
    let result = setup.models.search(&setup.store, "chatonlm").await.unwrap();
    let zimage = result.repos.iter().find(|repo| repo.id == ZIMAGE).unwrap();
    assert!(zimage.uncensored);
    assert_eq!(
        zimage.family.as_ref().map(|family| family.id.as_str()),
        Some("z-image-turbo")
    );
    assert!(
        !result
            .repos
            .iter()
            .find(|repo| repo.id == KLEIN)
            .unwrap()
            .uncensored
    );
}

#[tokio::test]
async fn an_unreachable_hugging_face_or_an_unknown_repo_is_told() {
    let offline = setup_with(FakeHf::start(HfMode::Status(503)).await.unwrap(), 1 << 30).await;
    let empty = offline
        .models
        .search(&offline.store, "klein")
        .await
        .unwrap();
    assert!(empty.repos.is_empty());
    assert!(matches!(
        empty.problem,
        Some(HubProblem::Unavailable { .. })
    ));
    assert!(!empty.stale);
    let detail = offline
        .models
        .repo(&offline.downloads, &offline.store, KLEIN)
        .await
        .unwrap();
    assert!(matches!(
        detail.problem,
        Some(HubProblem::Unavailable { .. })
    ));
    assert!(!detail.stale);
    assert!(detail.files.is_empty());
    assert_eq!(
        invalid(offline.download(KLEIN, "tiny-klein-Q8_0.gguf").await),
        "Hugging Face isn't responding right now: try again later"
    );
    // Une recommandation, elle, n'a pas besoin du réseau pour être choisie.
    offline
        .download(KLEIN, "tiny-klein-Q4_0.gguf")
        .await
        .unwrap();
    assert_eq!(offline.downloads.list().len(), 1);

    let setup = setup().await;
    let missing = setup
        .models
        .repo(&setup.downloads, &setup.store, "nobody/missing")
        .await
        .unwrap();
    assert_eq!(missing.problem, Some(HubProblem::NotFound));
    assert!(!missing.stale);
    assert_eq!(missing.summary.id, "nobody/missing");
}

#[test]
fn the_advised_file_is_the_largest_that_fits() {
    let files = |list: &[(u64, &str)]| -> Vec<(u64, Option<String>)> {
        list.iter()
            .map(|(size, quant)| (*size, Some((*quant).to_owned()).filter(|q| !q.is_empty())))
            .collect()
    };
    let quants = files(&[(30, "Q4_0"), (50, "Q8_0"), (90, "F16"), (40, "Q5_K_M")]);
    // Tout tient : la plus grande conseillée (pas la F16).
    assert_eq!(recommend(&quants, 10, 1000), Some(1));
    // Q8_0 ne tient plus avec les pièces.
    assert_eq!(recommend(&quants, 10, 55), Some(3));
    // Rien ne tient : la plus petite conseillée.
    assert_eq!(recommend(&quants, 10, 5), Some(0));
    // Aucune conseillée : mêmes règles sur tous les fichiers.
    let others = files(&[(90, "F16"), (60, "BF16"), (80, "")]);
    assert_eq!(recommend(&others, 0, 85), Some(2));
    assert_eq!(recommend(&others, 0, 10), Some(1));
    assert_eq!(recommend(&[], 0, 10), None);
    // Sans carte : la RAM ; machine inconnue : 8 Gio.
    let mut cpu = machine(0);
    cpu.gpus.clear();
    assert_eq!(memory_of(Some(&cpu)), 16 << 30);
    assert_eq!(memory_of(Some(&machine(6 << 30))), 6 << 30);
    assert_eq!(memory_of(None), UNKNOWN_MEMORY);
}

#[tokio::test]
async fn a_repo_shows_its_files_pieces_and_why_it_cannot_be_downloaded() {
    let setup = setup().await;
    let detail = setup
        .models
        .repo(&setup.downloads, &setup.store, KLEIN)
        .await
        .unwrap();
    assert_eq!(detail.unsupported, None);
    assert_eq!(
        detail.revision,
        fake_revision(KLEIN),
        "the commit of the repository"
    );
    let files: Vec<(&str, Option<&str>, bool)> = detail
        .files
        .iter()
        .map(|file| (file.file.as_str(), file.quant.as_deref(), file.recommended))
        .collect();
    // Sans `lfs` : pas proposé. Tout tient dans 1 Gio : Q8_0.
    assert_eq!(
        files,
        [
            ("tiny-klein-Q4_0.gguf", Some("Q4_0"), false),
            ("tiny-klein-Q8_0.gguf", Some("Q8_0"), true),
            ("tiny-klein-F16.gguf", Some("F16"), false)
        ]
    );
    assert_eq!(
        detail
            .components
            .iter()
            .map(|piece| (piece.role, piece.size_bytes, piece.present))
            .collect::<Vec<_>>(),
        [
            (ImageComponentRole::TextEncoder, 2500, false),
            (ImageComponentRole::Vae, 1500, false)
        ]
    );
    assert_eq!(detail.summary.license.as_deref(), Some("apache-2.0"));
    assert!(detail.summary.open_license);

    // Petite carte : rien ne tient avec les pièces, la plus petite.
    let small = setup_with(FakeHf::start(HfMode::Normal).await.unwrap(), 6000).await;
    let detail = small
        .models
        .repo(&small.downloads, &small.store, KLEIN)
        .await
        .unwrap();
    assert!(detail.files[0].recommended);
    assert_eq!(
        detail.files.iter().filter(|file| file.recommended).count(),
        1
    );

    let why = |repo: &'static str| {
        let setup = &setup;
        async move {
            setup
                .models
                .repo(&setup.downloads, &setup.store, repo)
                .await
                .unwrap()
                .unsupported
        }
    };
    assert_eq!(
        why("chatonlm/unknown-diffusion-GGUF").await,
        Some(ImageRepoUnsupported::UnknownFamily)
    );
    assert_eq!(
        why("chatonlm/Tiny-LTX-GGUF").await,
        Some(ImageRepoUnsupported::UnsupportedFamily)
    );
    assert_eq!(
        why("chatonlm/gated-image-GGUF").await,
        Some(ImageRepoUnsupported::Gated)
    );
    // Bases sans fichier GGUF : rien d'utilisable.
    assert_eq!(
        why("chatonlm/Klein-Finetune").await,
        Some(ImageRepoUnsupported::NoUsableFile)
    );
    let sdxl = setup
        .models
        .repo(&setup.downloads, &setup.store, "chatonlm/Tiny-SDXL")
        .await
        .unwrap();
    assert_eq!(
        sdxl.files
            .iter()
            .map(|file| file.file.as_str())
            .collect::<Vec<_>>(),
        ["tiny_sdxl.safetensors"]
    );
    assert!(sdxl.files[0].recommended);
    // Une fiche non censurée reste lisible, marquée.
    let zimage = setup
        .models
        .repo(&setup.downloads, &setup.store, ZIMAGE)
        .await
        .unwrap();
    assert!(zimage.summary.uncensored);
}

/// Commit du dépôt dans le faux.
fn fake_revision(repo: &str) -> String {
    let families: serde_json::Value = serde_json::from_str(IMAGE_FAMILIES).unwrap();
    families["recommendations"]
        .as_array()
        .unwrap()
        .iter()
        .chain(families["pieces"].as_object().unwrap().values())
        .find(|pinned| pinned["repo"] == repo)
        .and_then(|pinned| pinned["revision"].as_str())
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn models_share_their_pieces_and_deleting_keeps_what_others_use() {
    let setup = setup().await;
    let klein = format!("{KLEIN}/tiny-klein-Q4_0.gguf");
    setup.download(KLEIN, "tiny-klein-Q4_0.gguf").await.unwrap();
    setup.wait_done(&klein).await;
    let info = setup.info(&klein).await;
    assert!(info.installed);
    assert_eq!(info.missing_bytes, 0);
    assert_eq!(info.size_bytes, 3000 + 2500 + 1500);
    assert_eq!(info.family.id, "flux2-klein-4b");
    assert_eq!(info.revision, fake_revision(KLEIN));
    assert!(info.recommended, "the test families recommend it");
    assert!(info.components.iter().all(|piece| piece.present));
    assert_eq!(
        info.components
            .iter()
            .map(|piece| piece.role)
            .collect::<Vec<_>>(),
        [
            ImageComponentRole::Model,
            ImageComponentRole::TextEncoder,
            ImageComponentRole::Vae
        ]
    );
    // Rangés par empreinte, contenu vérifié.
    let model = image_blob(KLEIN, "tiny-klein-Q4_0.gguf").unwrap();
    let stored = setup
        .store
        .blocking(|c| stored::get(c, &klein))
        .unwrap()
        .unwrap();
    let path = blob_path(setup.root.path(), &stored.files[0]);
    assert_eq!(std::fs::read(&path).unwrap(), model);
    assert!(
        path.starts_with(
            setup
                .root
                .path()
                .join("blobs")
                .join(&stored.files[0].sha256)
        )
    );
    assert_eq!(
        setup.blobs(),
        [
            "tiny-klein-Q4_0.gguf",
            "tiny-qwen3-Q4_K_M.gguf",
            "tiny_ae.safetensors"
        ]
    );

    // Même famille de pièces : seul le modèle est téléchargé.
    setup.show_uncensored(true);
    let zimage = format!("{ZIMAGE}/tiny-zimage-Q4_K.gguf");
    let before = setup.fetched().len();
    setup
        .download(ZIMAGE, "tiny-zimage-Q4_K.gguf")
        .await
        .unwrap();
    assert_eq!(
        setup
            .downloads
            .list()
            .iter()
            .find(|download| download.id == zimage)
            .unwrap()
            .total_bytes,
        4000,
        "only what is missing"
    );
    setup.wait_done(&zimage).await;
    let fetched = setup.fetched();
    assert_eq!(fetched.len(), before + 1, "{fetched:?}");
    assert!(fetched.last().unwrap().ends_with("/tiny-zimage-Q4_K.gguf"));
    assert!(setup.info(&zimage).await.uncensored);

    // Le premier supprimé : ses pièces restent (employées par l'autre).
    setup
        .models
        .delete(&setup.downloads, &setup.store, None, &klein)
        .await
        .unwrap();
    assert_eq!(
        setup.blobs(),
        [
            "tiny-qwen3-Q4_K_M.gguf",
            "tiny-zimage-Q4_K.gguf",
            "tiny_ae.safetensors"
        ]
    );
    assert!(
        setup
            .list()
            .await
            .iter()
            .all(|model| model.id != klein || !model.installed)
    );
    assert!(setup.info(&zimage).await.installed);
    // Le second : plus rien.
    let engine = ImageGen::new(Config::new(None, setup.root.path()));
    setup
        .models
        .delete(&setup.downloads, &setup.store, Some(&engine), &zimage)
        .await
        .unwrap();
    assert!(setup.blobs().is_empty());
    assert!(
        setup
            .store
            .blocking(|c| stored::list(c))
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn refusals_are_explained() {
    let setup = setup().await;
    assert_eq!(
        invalid(
            setup
                .download("chatonlm/gated-image-GGUF", "restricted-Q4_0.gguf")
                .await
        ),
        "This repository is restricted (a Hugging Face account is needed): ChatonLM can't download it"
    );
    assert_eq!(
        invalid(
            setup
                .download("chatonlm/unknown-diffusion-GGUF", "mystery-Q4_0.gguf")
                .await
        ),
        "ChatonLM doesn't know this model's family, so it can't run it"
    );
    assert_eq!(
        invalid(
            setup
                .download("chatonlm/Tiny-LTX-GGUF", "ltx-Q4_0.gguf")
                .await
        ),
        "This model's family isn't supported by ChatonLM's image engine"
    );
    // Fichier sans `lfs` : pas d'empreinte, refusé.
    assert_eq!(
        invalid(setup.download(KLEIN, "tiny-klein-Q5_K_M.gguf").await),
        "tiny-klein-Q5_K_M.gguf has no checksum on Hugging Face: ChatonLM won't download it"
    );
    assert_eq!(
        invalid(
            setup
                .models
                .download(
                    &setup.downloads,
                    &setup.store,
                    KLEIN,
                    "tiny-klein-Q8_0.gguf",
                    "0123"
                )
                .await
        ),
        "This repository has changed since its page was opened: open it again"
    );
    assert_eq!(
        invalid(setup.download(KLEIN, "absent.gguf").await),
        "absent.gguf isn't in this repository"
    );
    assert_eq!(
        invalid(
            setup
                .download(
                    "chatonlm/Tiny-SDXL",
                    "unet/diffusion_pytorch_model.safetensors"
                )
                .await
        ),
        "unet/diffusion_pytorch_model.safetensors can't be used as an image model"
    );
    // Non censuré, réglage coupé : la fiche se lit, le téléchargement non.
    assert_eq!(
        invalid(setup.download(ZIMAGE, "tiny-zimage-Q4_K.gguf").await),
        "This model is uncensored: turn on “Show uncensored models” in Settings to download it"
    );
    // Place insuffisante : refusé avant la file, avec les deux tailles.
    let huge = invalid(
        setup
            .download("chatonlm/Tiny-Huge-GGUF", "huge-Q4_0.gguf")
            .await,
    );
    assert!(
        huge.starts_with("Not enough disk space: 1000000000.1 GB needed, "),
        "{huge}"
    );
    assert!(
        huge.ends_with(" GB free") || huge.ends_with(" MB free"),
        "{huge}"
    );
    assert!(setup.downloads.list().is_empty());
    assert!(
        setup
            .store
            .blocking(|c| stored::list(c))
            .unwrap()
            .is_empty()
    );
    assert!(setup.fetched().is_empty());
}

#[tokio::test]
async fn a_recommendation_downloads_without_reading_the_repository() {
    let setup = setup().await;
    let list = setup.list().await;
    let recommended: Vec<&ImageModelInfo> = list.iter().filter(|model| model.recommended).collect();
    assert_eq!(recommended.len(), 1);
    let tiny = recommended[0];
    assert_eq!(tiny.name, "Tiny klein");
    assert!(!tiny.installed && !tiny.selected);
    assert_eq!(tiny.missing_bytes, 3000 + 2500 + 1500);
    assert_eq!(tiny.revision, fake_revision(KLEIN));
    setup
        .models
        .download(
            &setup.downloads,
            &setup.store,
            &tiny.repo,
            &tiny.file,
            &tiny.revision,
        )
        .await
        .unwrap();
    setup.wait_done(&tiny.id).await;
    assert!(
        setup
            .fake
            .requests()
            .iter()
            .all(|request| !request.path.starts_with("/api/")),
        "{:?}",
        setup.fake.requests()
    );
    let installed = setup.info(&tiny.id).await;
    assert!(installed.installed);
    assert_eq!(
        setup.list().await.len(),
        1,
        "no second card for the recommendation"
    );
}

#[tokio::test]
async fn the_chosen_model_gives_the_engine_its_files_and_settings() {
    let setup = setup().await;
    let klein = format!("{KLEIN}/tiny-klein-Q8_0.gguf");
    assert_eq!(
        invalid(
            setup
                .models
                .use_model(&setup.downloads, &setup.store, &klein)
                .await
        ),
        "This model isn't installed"
    );
    assert!(setup.models.selected_spec(&setup.store).await.is_none());
    setup.download(KLEIN, "tiny-klein-Q8_0.gguf").await.unwrap();
    setup.wait_done(&klein).await;
    setup
        .models
        .use_model(&setup.downloads, &setup.store, &klein)
        .await
        .unwrap();
    assert!(setup.info(&klein).await.selected);
    let (spec, sampling) = setup.models.selected_spec(&setup.store).await.unwrap();
    assert_eq!(
        spec.files
            .iter()
            .map(|(component, path)| (*component, path.file_name().unwrap().to_owned()))
            .collect::<Vec<_>>(),
        [
            (Component::DiffusionModel, "tiny-klein-Q8_0.gguf".into()),
            (Component::Llm, "tiny-qwen3-Q4_K_M.gguf".into()),
            (Component::Vae, "tiny_ae.safetensors".into())
        ]
    );
    assert!(spec.files.iter().all(|(_, path)| path.is_file()));
    assert_eq!((sampling.steps, sampling.cfg_scale), (4, 1.0));
    // Un fichier disparu : plus prêt.
    let (_, model) = &spec.files[0];
    std::fs::remove_file(model).unwrap();
    assert!(setup.models.selected_spec(&setup.store).await.is_none());
    assert!(!setup.info(&klein).await.installed);
    // Supprimé : plus choisi.
    setup
        .models
        .delete(&setup.downloads, &setup.store, None, &klein)
        .await
        .unwrap();
    assert_eq!(setup.store.blocking(|c| stored::selected(c)).unwrap(), "");
    setup
        .models
        .use_model(&setup.downloads, &setup.store, "")
        .await
        .unwrap();
}

#[tokio::test]
async fn a_cancelled_download_is_forgotten_when_dismissed_and_refuses_deletion_meanwhile() {
    // Réponses retardées : le téléchargement est annulé en route.
    let fake = FakeHf::start_with(
        std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
        HfMode::Normal,
        Duration::from_millis(400),
    )
    .await
    .unwrap();
    let setup = setup_with(fake, 1 << 30).await;
    let klein = format!("{KLEIN}/tiny-klein-F16.gguf");
    setup.download(KLEIN, "tiny-klein-F16.gguf").await.unwrap();
    assert!(setup.downloads.is_active(&klein));
    // En cours : ni suppression, ni second téléchargement.
    assert_eq!(
        invalid(
            setup
                .models
                .delete(&setup.downloads, &setup.store, None, &klein)
                .await
        ),
        "This model is downloading: cancel the download first"
    );
    setup.download(KLEIN, "tiny-klein-F16.gguf").await.unwrap();
    assert_eq!(setup.downloads.list().len(), 1);
    let info = setup.info(&klein).await;
    assert!(!info.installed);
    assert_eq!(info.missing_bytes, 9000 + 2500 + 1500);

    let installed = |detail: &ImageRepoDetail| {
        detail
            .files
            .iter()
            .find(|file| file.file == "tiny-klein-F16.gguf")
            .unwrap()
            .installed
    };
    let detail = setup
        .models
        .repo(&setup.downloads, &setup.store, KLEIN)
        .await
        .unwrap();
    assert!(installed(&detail), "while downloading");

    setup.downloads.cancel(&klein);
    assert!(setup.downloads.wait_idle(&klein, WAIT).await);
    let detail = setup
        .models
        .repo(&setup.downloads, &setup.store, KLEIN)
        .await
        .unwrap();
    assert!(!installed(&detail), "cancelled, its row is not an install");
    setup
        .models
        .dismiss(&setup.downloads, &setup.store, &klein)
        .await
        .unwrap();
    assert!(setup.downloads.list().is_empty());
    assert!(
        setup
            .store
            .blocking(|c| stored::list(c))
            .unwrap()
            .is_empty()
    );
    assert!(setup.blobs().is_empty());

    // Recommandation : sa ligne reste (« Try again » relit sa révision) ;
    // elle repasse avec les recommandations, après les autres.
    let recommended = format!("{KLEIN}/tiny-klein-Q4_0.gguf");
    setup.download(KLEIN, "tiny-klein-Q4_0.gguf").await.unwrap();
    setup.downloads.cancel(&recommended);
    let listed = setup.list().await;
    assert_eq!(listed[0].id, recommended, "with its download first");
    assert!(setup.downloads.wait_idle(&recommended, WAIT).await);
    setup
        .models
        .dismiss(&setup.downloads, &setup.store, &recommended)
        .await
        .unwrap();
    assert_eq!(setup.store.blocking(|c| stored::list(c)).unwrap().len(), 1);
    let info = setup.info(&recommended).await;
    assert!(info.recommended && !info.installed);
    setup
        .models
        .delete(&setup.downloads, &setup.store, None, &recommended)
        .await
        .unwrap();
    assert!(
        setup
            .store
            .blocking(|c| stored::list(c))
            .unwrap()
            .is_empty()
    );
}

/// Adaptateurs (LoRA) et fichiers trop petits : jamais pris pour des modèles
/// complets de SD 1.5 ou SDXL.
#[tokio::test]
async fn adapters_are_not_whole_models() {
    let setup = setup().await;
    let result = setup.models.search(&setup.store, "sdxl").await.unwrap();
    let ids: Vec<&str> = result.repos.iter().map(|repo| repo.id.as_str()).collect();
    assert_eq!(ids, ["chatonlm/Tiny-SDXL"]);
    let lora = setup
        .models
        .repo(&setup.downloads, &setup.store, "chatonlm/Tiny-SDXL-LoRA")
        .await
        .unwrap();
    assert_eq!(lora.unsupported, Some(ImageRepoUnsupported::NoUsableFile));
    assert!(lora.files.is_empty());
    assert_eq!(
        invalid(
            setup
                .download("chatonlm/Tiny-SDXL-LoRA", "tiny-lora-xl.safetensors")
                .await
        ),
        "tiny-lora-xl.safetensors can't be used as an image model"
    );
    for file in [
        "tiny_sdxl_offset-lora.safetensors",
        "tiny_small.safetensors",
    ] {
        assert_eq!(
            invalid(setup.download("chatonlm/Tiny-SDXL", file).await),
            format!("{file} can't be used as an image model")
        );
    }
}

/// Même contenu sous deux noms : deux fichiers ; supprimer l'un des modèles
/// garde le fichier de l'autre.
#[tokio::test]
async fn the_same_content_under_two_names_is_kept_for_the_other_model() {
    let setup = setup().await;
    let model = |name: &str| {
        let mut model = recommended_model(
            setup.models.families.get("flux2-klein-4b").unwrap(),
            &setup.models.families.recommendations[0],
        );
        model.id = format!("chatonlm/same/{name}");
        model.file = name.to_owned();
        model.files[0].file = name.to_owned();
        model
    };
    let (first, second) = (model("first.gguf"), model("second.gguf"));
    for model in [&first, &second] {
        for file in &model.files {
            let path = blob_path(setup.root.path(), file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, vec![0; file.size as usize]).unwrap();
        }
        let model = model.clone();
        setup.store.blocking(|c| stored::save(c, &model)).unwrap();
    }
    let first_path = blob_path(setup.root.path(), &first.files[0]);
    let second_path = blob_path(setup.root.path(), &second.files[0]);
    assert_eq!(first_path.parent(), second_path.parent());
    setup
        .models
        .delete(&setup.downloads, &setup.store, None, &first.id)
        .await
        .unwrap();
    assert!(!first_path.exists());
    assert!(second_path.is_file());
    assert!(setup.models.installed(&second));
}

/// Un fichier qui ne s'efface pas laisse le modèle en place (pas de fichier
/// orphelin).
#[cfg(unix)]
#[tokio::test]
async fn a_file_that_cannot_be_deleted_keeps_the_model() {
    use std::os::unix::fs::PermissionsExt;
    let setup = setup().await;
    let klein = format!("{KLEIN}/tiny-klein-Q8_0.gguf");
    setup.download(KLEIN, "tiny-klein-Q8_0.gguf").await.unwrap();
    setup.wait_done(&klein).await;
    let stored = setup
        .store
        .blocking(|c| stored::get(c, &klein))
        .unwrap()
        .unwrap();
    let dir = blob_path(setup.root.path(), &stored.files[0])
        .parent()
        .unwrap()
        .to_path_buf();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let refused = setup
        .models
        .delete(&setup.downloads, &setup.store, None, &klein)
        .await;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(refused.is_err());
    assert!(
        setup
            .store
            .blocking(|c| stored::get(c, &klein))
            .unwrap()
            .is_some()
    );
    setup
        .models
        .delete(&setup.downloads, &setup.store, None, &klein)
        .await
        .unwrap();
    assert!(setup.blobs().is_empty());
}

/// Révisions : une recommandation dont le dépôt a un commit plus récent est
/// relue sur sa fiche ; un modèle enregistré non installé aussi ; un modèle
/// installé à une autre révision est refusé.
#[tokio::test]
async fn a_newer_revision_is_read_from_the_repository() {
    let mut families: serde_json::Value = serde_json::from_str(IMAGE_FAMILIES).unwrap();
    families["recommendations"][0]["revision"] = "0".repeat(40).into();
    let setup = setup_families(
        FakeHf::start(HfMode::Normal).await.unwrap(),
        1 << 30,
        &families.to_string(),
    )
    .await;
    let klein = format!("{KLEIN}/tiny-klein-Q4_0.gguf");
    let current = fake_revision(KLEIN);
    let api = |setup: &Setup| {
        setup
            .fake
            .requests()
            .iter()
            .filter(|request| request.path.starts_with("/api/models/"))
            .count()
    };
    // Commit plus récent que l'épinglé : relu sur la fiche.
    setup
        .models
        .download(
            &setup.downloads,
            &setup.store,
            KLEIN,
            "tiny-klein-Q4_0.gguf",
            &current,
        )
        .await
        .unwrap();
    assert!(api(&setup) > 0);
    setup.wait_done(&klein).await;
    let info = setup.info(&klein).await;
    assert_eq!(info.revision, current);
    assert_eq!(info.name, "Tiny klein", "the name of the recommendation");
    // Installé à cette révision : une autre est refusée.
    assert_eq!(
        invalid(
            setup
                .models
                .download(
                    &setup.downloads,
                    &setup.store,
                    KLEIN,
                    "tiny-klein-Q4_0.gguf",
                    &"1".repeat(40)
                )
                .await
        ),
        "This repository has changed since its page was opened: open it again"
    );

    // Enregistré à une ancienne révision, jamais installé : relu.
    let q8 = format!("{KLEIN}/tiny-klein-Q8_0.gguf");
    let mut old = setup
        .store
        .blocking(|c| stored::get(c, &klein))
        .unwrap()
        .unwrap();
    old.id = q8.clone();
    old.file = "tiny-klein-Q8_0.gguf".into();
    old.revision = "0".repeat(40);
    old.files[0].file = old.file.clone();
    old.files[0].sha256 = "c".repeat(64);
    old.files[0].revision = old.revision.clone();
    old.ever_installed = false;
    let partial = blob_path(setup.root.path(), &old.files[0]);
    std::fs::create_dir_all(partial.parent().unwrap()).unwrap();
    std::fs::write(part_path(&partial), [0; 10]).unwrap();
    setup.store.blocking(|c| stored::save(c, &old)).unwrap();
    setup
        .models
        .download(
            &setup.downloads,
            &setup.store,
            KLEIN,
            "tiny-klein-Q8_0.gguf",
            &current,
        )
        .await
        .unwrap();
    setup.wait_done(&q8).await;
    let info = setup.info(&q8).await;
    assert_eq!(info.revision, current);
    assert!(info.installed);
    // Le morceau de l'ancienne révision est parti.
    assert!(!part_path(&partial).exists());
}

/// Bases lues au plus 12 par reconnaissance (fiche, téléchargement).
#[tokio::test]
async fn at_most_twelve_bases_are_read() {
    let setup = setup().await;
    let repo = ImageRepo {
        id: "someone/many-bases".into(),
        author: "someone".into(),
        downloads: 0,
        likes: 0,
        last_modified: None,
        license: None,
        gated: false,
        bases: (0..20)
            .map(|index| format!("nobody/base-{index}"))
            .collect(),
        sha: None,
        files: Vec::new(),
        adapter: false,
        adult: false,
    };
    let recognized = setup
        .models
        .catalog
        .recognize(&setup.models.families, &repo)
        .await;
    assert_eq!(recognized.known, Known::Unknown);
    assert!(recognized.unverified);
    assert_eq!(setup.fake.requests().len(), 12);
}

async fn search_ids(setup: &Setup) -> Vec<String> {
    setup
        .models
        .search(&setup.store, "chatonlm")
        .await
        .unwrap()
        .repos
        .into_iter()
        .map(|repo| repo.id)
        .collect()
}

/// Non censurés : tag `not-for-all-audiences` ; base illisible : refusé
/// sans le réglage, par prudence.
#[tokio::test]
async fn adult_tags_and_unreadable_bases_need_the_setting() {
    let setup = setup().await;
    let hidden = search_ids(&setup).await;
    assert!(!hidden.contains(&"chatonlm/Tiny-Klein-NFAA-GGUF".to_owned()));
    assert_eq!(
        invalid(
            setup
                .download("chatonlm/Tiny-Klein-NFAA-GGUF", "nfaa-Q4_0.gguf")
                .await
        ),
        "This model is uncensored: turn on “Show uncensored models” in Settings to download it"
    );
    assert_eq!(
        invalid(
            setup
                .download("chatonlm/Tiny-Klein-Mixed-GGUF", "mixed-Q4_0.gguf")
                .await
        ),
        "ChatonLM couldn't check where this model comes from (one of its base models can't be read): turn on “Show uncensored models” in Settings to download it anyway"
    );
    setup.show_uncensored(true);
    assert!(
        search_ids(&setup)
            .await
            .contains(&"chatonlm/Tiny-Klein-NFAA-GGUF".to_owned())
    );
    let mixed = "chatonlm/Tiny-Klein-Mixed-GGUF/mixed-Q4_0.gguf";
    setup
        .download("chatonlm/Tiny-Klein-Mixed-GGUF", "mixed-Q4_0.gguf")
        .await
        .unwrap();
    setup.wait_done(mixed).await;
    let info = setup.info(mixed).await;
    assert_eq!(info.family.id, "flux2-klein-4b");
    assert!(info.uncensored, "shown as uncensored, out of caution");
}

/// Retirer de la liste un modèle installé une fois, dont un fichier a
/// disparu depuis, ne l'oublie pas.
#[tokio::test]
async fn dismissing_never_forgets_a_model_installed_once() {
    let setup = setup().await;
    let klein = format!("{KLEIN}/tiny-klein-F16.gguf");
    setup.download(KLEIN, "tiny-klein-F16.gguf").await.unwrap();
    setup.wait_done(&klein).await;
    let stored = setup
        .store
        .blocking(|c| stored::get(c, &klein))
        .unwrap()
        .unwrap();
    assert!(stored.ever_installed);
    std::fs::remove_file(blob_path(setup.root.path(), &stored.files[0])).unwrap();
    setup
        .models
        .dismiss(&setup.downloads, &setup.store, &klein)
        .await
        .unwrap();
    assert!(
        setup
            .store
            .blocking(|c| stored::get(c, &klein))
            .unwrap()
            .is_some()
    );
    let info = setup.info(&klein).await;
    assert!(!info.installed);
    assert_eq!(info.missing_bytes, 9000);
}
