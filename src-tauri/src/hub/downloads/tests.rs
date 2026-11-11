//! Essais des téléchargements du ModelHub, contre le faux Hugging Face
//! (dépôts synthétisés `chatonlm-dl/…` au vrai contenu, voir
//! `fake_services::hf_models`) : fichiers, empreintes, base locale, file.

use std::sync::Arc;
use std::time::Duration;

use fake_services::hf::{FakeHf, HfMode};
use fake_services::hf_models::Synth;
use parking_lot::Mutex;

use super::*;
use crate::hub::errors::english::{
    CONNECTION_LOST, LOCATION_CHANGING, NOT_FOUND_OR_PRIVATE, NOT_GGUF, TIMED_OUT,
    UNAVAILABLE_VERSION,
};

use HubDownloadState::{Cancelled, Done, Downloading, Failed, Finishing, Paused, Queued, Starting};

/// Attente maximale d'un état, sur le faux serveur. Seulement une borne :
/// `wait_until` rend la main dès que l'état est vu. Les états attendus
/// suivent des écritures et des suppressions de fichiers dans TMPDIR ; sous
/// `check.sh`, TMPDIR est sur le disque du projet (ntfs3, métadonnées
/// écrites de façon synchrone, ~200 ms par fichier créé et synchronisé sur
/// cette machine, bien plus avec seize essais à la fois) : 10 s n'y
/// suffisaient pas toujours (nettoyage d'une annulation, fin d'un modèle
/// découpé).
const WAIT: Duration = Duration::from_secs(60);

/// Liste émise, datée.
type Emitted = (Instant, Vec<HubDownload>);

/// Listes émises.
#[derive(Clone, Default)]
struct Recorder {
    events: Arc<Mutex<Vec<Emitted>>>,
}

impl Recorder {
    fn events(&self) -> Vec<Emitted> {
        self.events.lock().clone()
    }

    /// États successifs (distincts) de `model` dans les listes émises.
    fn states(&self, model: &str) -> Vec<HubDownloadState> {
        let mut states: Vec<HubDownloadState> = Vec::new();
        for (_, list) in self.events() {
            if let Some(download) = list.iter().find(|download| download.model == model)
                && states.last() != Some(&download.state)
            {
                states.push(download.state);
            }
        }
        states
    }

    /// Émissions où `model` figure, dans l'ordre.
    fn of(&self, model: &str) -> Vec<HubDownload> {
        self.events()
            .into_iter()
            .filter_map(|(_, list)| list.into_iter().find(|download| download.model == model))
            .collect()
    }
}

/// Faux Hugging Face, base en mémoire, dossier des données, file.
struct Env {
    fake: FakeHf,
    store: Store,
    dir: tempfile::TempDir,
    downloads: HubDownloads,
    recorder: Recorder,
}

impl Env {
    async fn new(options: impl FnOnce(Options) -> Options) -> Self {
        Self::with_mode(HfMode::Normal, options).await
    }

    async fn with_mode(mode: HfMode, options: impl FnOnce(Options) -> Options) -> Self {
        let fake = FakeHf::start(mode).await.unwrap();
        let recorder = Recorder::default();
        let sink = recorder.events.clone();
        let downloads = HubDownloads::with_options(
            move |list| sink.lock().push((Instant::now(), list.to_vec())),
            options(Options {
                hf_url: Some(fake.url()),
                ..Options::default()
            }),
        );
        let store = Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        downloads.attach(store.clone(), dir.path().to_owned());
        Self {
            fake,
            store,
            dir,
            downloads,
            recorder,
        }
    }

    /// Dossier des modèles par défaut.
    fn models(&self) -> PathBuf {
        self.dir.path().join("models")
    }

    fn ask(&self, model: &str) {
        self.downloads.download(ask(model)).unwrap();
    }

    fn entry(&self, model: &str) -> HubDownload {
        entry(&self.downloads, model)
    }

    async fn finished(&self, model: &str) -> HubDownload {
        let what = format!("the end of {model}");
        wait_until(&what, WAIT, || !active(self.entry(model).state)).await;
        self.entry(model)
    }

    fn installed(&self) -> Vec<StoredLocalModel> {
        self.store.blocking(|c| table::list(c)).unwrap()
    }

    /// Fichiers demandés à `resolve/`, dans l'ordre.
    fn fetched(&self) -> Vec<String> {
        self.fake
            .requests()
            .into_iter()
            .filter(|request| request.path.contains("/resolve/"))
            .map(|request| request.path)
            .collect()
    }

    /// Fichiers (relatifs) sous le dossier des modèles, triés.
    fn files(&self) -> Vec<String> {
        let mut found = Vec::new();
        let root = self.models();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let relative = path.strip_prefix(&root).unwrap();
                    found.push(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        found.sort();
        found
    }
}

fn ask(model: &str) -> HubDownloadRequest {
    HubDownloadRequest {
        model: model.to_owned(),
        repo: None,
        label: None,
    }
}

fn entry(downloads: &HubDownloads, model: &str) -> HubDownload {
    downloads
        .list()
        .into_iter()
        .find(|download| download.model == model)
        .unwrap_or_else(|| panic!("{model} is not listed"))
}

async fn wait_until(what: &str, limit: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Contenu servi par le faux pour `path` du dépôt `repo` (révision 0).
fn content(repo: &str, path: &str) -> Vec<u8> {
    Synth::parse(repo, 0).unwrap().content(path).unwrap()
}

fn size(repo: &str, path: &str) -> u64 {
    content(repo, path).len() as u64
}

fn sha_of(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

const EYE: &str = "chatonlm-dl/Eye-vision-tools-think-GGUF";

/// Tout un téléchargement : fiche, poids puis projecteur (même précision,
/// sinon Q8_0), à la révision de la fiche ; fichiers rangés et vérifiés,
/// ligne de la base écrite avec ce que dit l'en-tête GGUF.
#[tokio::test]
async fn a_download_fetches_every_file_then_installs_the_model() {
    let env = Env::new(|options| options).await;
    let model = format!("{EYE}:Q4_K_M");
    env.ask(&model);
    let done = env.finished(&model).await;
    assert_eq!(done.state, Done, "{done:?}");
    assert_eq!(done.repo.as_deref(), Some(EYE));
    assert_eq!(done.label.as_deref(), Some("Q4_K_M"));
    let weights = size(EYE, "Eye-Q4_K_M.gguf");
    let projector = size(EYE, "mmproj-Q8_0.gguf");
    assert_eq!(
        (done.completed, done.total),
        (weights + projector, weights + projector)
    );
    assert_eq!(
        env.recorder.states(&model),
        [Starting, Downloading, Finishing, Done]
    );
    for download in env.recorder.of(&model) {
        assert!(download.completed <= download.total, "{download:?}");
    }
    let sha = Synth::parse(EYE, 0).unwrap().sha();
    assert_eq!(
        env.fetched(),
        [
            format!("/{EYE}/resolve/{sha}/Eye-Q4_K_M.gguf"),
            format!("/{EYE}/resolve/{sha}/mmproj-Q8_0.gguf"),
        ]
    );
    // Rangés sous <données>/models/<auteur>/<dépôt>/, sans .part.
    assert_eq!(
        env.files(),
        [
            format!("{EYE}/Eye-Q4_K_M.gguf"),
            format!("{EYE}/mmproj-Q8_0.gguf"),
        ]
    );
    assert_eq!(
        std::fs::read(env.models().join(EYE).join("Eye-Q4_K_M.gguf")).unwrap(),
        content(EYE, "Eye-Q4_K_M.gguf")
    );
    let [row] = env.installed().try_into().unwrap();
    assert_eq!(row.id, model);
    assert_eq!(row.source, ModelSource::Hf);
    assert_eq!(row.repo, EYE);
    assert_eq!(row.revision.as_deref(), Some(sha.as_str()));
    assert_eq!(row.quant, "Q4_K_M");
    assert_eq!(row.files, [format!("{EYE}/Eye-Q4_K_M.gguf")]);
    assert_eq!(
        row.mmproj.as_deref(),
        Some(format!("{EYE}/mmproj-Q8_0.gguf").as_str())
    );
    assert_eq!(row.size_bytes, weights + projector);
    assert_eq!(
        row.sha256[&format!("{EYE}/Eye-Q4_K_M.gguf")],
        sha_of(&content(EYE, "Eye-Q4_K_M.gguf"))
    );
    assert_eq!(row.arch.as_deref(), Some("qwen3"));
    assert_eq!(row.context_max, Some(40960));
    assert_eq!(row.params_label.as_deref(), Some("0.6B"));
    assert_eq!(row.file_type, Some(15));
    assert_eq!(
        row.capabilities,
        ModelCapabilities {
            vision: true,
            tools: true,
            thinking: true,
            ..ModelCapabilities::default()
        }
    );
}

/// Morceaux dans l'ordre, sous-dossiers gardés ; la forme `hf.co/…` d'une
/// demande est ramenée à l'identifiant.
#[tokio::test]
async fn split_and_nested_versions_are_downloaded_too() {
    let env = Env::new(|options| options).await;
    let sharded = "chatonlm-dl/Big-sharded-fim-GGUF";
    env.ask(&format!("hf.co/{sharded}:q8_0"));
    // L'entrée garde la casse demandée ; la base, celle de la fiche.
    assert_eq!(env.finished(&format!("{sharded}:q8_0")).await.state, Done);
    let row = env.installed().remove(0);
    assert_eq!(row.id, format!("{sharded}:Q8_0"));
    assert_eq!(
        row.files,
        [
            format!("{sharded}/Big-Q8_0-00001-of-00002.gguf"),
            format!("{sharded}/Big-Q8_0-00002-of-00002.gguf"),
        ]
    );
    assert_eq!(row.mmproj, None);
    assert!(row.capabilities.fim);
    assert_eq!(row.file_type, Some(7));

    let nested = "chatonlm-dl/Deep-sub-GGUF";
    let model = format!("{nested}:Q8_0");
    env.ask(&model);
    assert_eq!(env.finished(&model).await.state, Done);
    assert!(
        env.files()
            .contains(&format!("{nested}/Q8_0/Deep-Q8_0.gguf"))
    );
}

/// Un projecteur partagé par deux versions n'est reçu qu'une fois ; la
/// suppression d'une version le garde pour l'autre, celle de la dernière
/// l'efface avec le dossier du dépôt.
#[tokio::test]
async fn a_shared_projector_is_fetched_once_and_kept_while_used() {
    let env = Env::new(|options| options).await;
    let q4 = format!("{EYE}:Q4_K_M");
    let q8 = format!("{EYE}:Q8_0");
    env.ask(&q4);
    assert_eq!(env.finished(&q4).await.state, Done);
    env.ask(&q8);
    assert_eq!(env.finished(&q8).await.state, Done);
    let projectors = env
        .fetched()
        .iter()
        .filter(|path| path.ends_with("mmproj-Q8_0.gguf"))
        .count();
    assert_eq!(projectors, 1);
    // Q8_0 : déjà là, à la bonne empreinte, donc toute la taille d'un coup.
    assert_eq!(env.installed().len(), 2);

    let data = env.dir.path();
    local_models::delete(&env.store, data, &q4, &env.downloads.reserved())
        .await
        .unwrap();
    assert_eq!(
        env.files(),
        [
            format!("{EYE}/Eye-Q8_0.gguf"),
            format!("{EYE}/mmproj-Q8_0.gguf")
        ]
    );
    local_models::delete(&env.store, data, &q8, &env.downloads.reserved())
        .await
        .unwrap();
    assert_eq!(env.files(), Vec::<String>::new());
    assert!(!env.models().join("chatonlm-dl").exists());
    assert!(env.models().exists());
    assert!(env.installed().is_empty());
    let Err(AppError::NotFound(message)) =
        local_models::delete(&env.store, data, &q8, &env.downloads.reserved()).await
    else {
        panic!("expected not found");
    };
    assert_eq!(message, "This model isn't installed.");
}

/// Les fichiers vont dans le dossier des modèles réglé.
#[tokio::test]
async fn the_chosen_models_folder_is_used() {
    let env = Env::new(|options| options).await;
    let elsewhere = tempfile::tempdir().unwrap();
    let chosen = elsewhere.path().join("mine").to_string_lossy().into_owned();
    env.store
        .blocking(|c| table::set_models_dir(c, Some(&chosen)))
        .unwrap();
    let model = "chatonlm-dl/Tiny-GGUF:Q4_K_M";
    env.ask(model);
    assert_eq!(env.finished(model).await.state, Done);
    assert!(
        elsewhere
            .path()
            .join("mine/chatonlm-dl/Tiny-GGUF/Tiny-Q4_K_M.gguf")
            .is_file()
    );
    assert!(!env.models().exists());
    let listed = local_models::list(&env.store, env.dir.path())
        .await
        .unwrap();
    assert_eq!(
        listed[0].path,
        local_models::absolute(Path::new(&chosen), "chatonlm-dl/Tiny-GGUF/Tiny-Q4_K_M.gguf")
            .unwrap()
            .to_string_lossy()
    );
    assert_eq!(listed[0].display_name, "Tiny");
}

/// Un `.part` laissé par une session précédente reprend par `Range` ; un
/// serveur qui ignore `Range` renvoie tout, et le fichier repart de zéro.
#[tokio::test]
async fn a_partial_file_is_resumed() {
    let env = Env::new(|options| options).await;
    for repo in [
        "chatonlm-dl/Half-GGUF",
        "chatonlm-dl/Whole-norange-big-GGUF",
    ] {
        let name = repo.split('/').nth(1).unwrap().split('-').next().unwrap();
        let file = format!("{name}-Q4_K_M.gguf");
        let bytes = content(repo, &file);
        let target = env.models().join(repo).join(&file);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let half = bytes.len() / 2;
        std::fs::write(part_path(&target), &bytes[..half]).unwrap();
        let model = format!("{repo}:Q4_K_M");
        env.ask(&model);
        assert_eq!(env.finished(&model).await.state, Done, "{repo}");
        assert_eq!(std::fs::read(&target).unwrap(), bytes, "{repo}");
        let ranges: Vec<_> = env
            .fake
            .requests()
            .into_iter()
            .filter(|request| request.path.ends_with(&file))
            .map(|request| request.range)
            .collect();
        assert_eq!(ranges, [Some(format!("bytes={half}-"))], "{repo}");
        // Sans `Range` respecté (200), le fichier repart de zéro : un ajout
        // au `.part` l'aurait rendu trop long ou faux.
    }
}

/// Deux à la fois, dans l'ordre des demandes ; une place libérée fait
/// démarrer le suivant.
#[tokio::test]
async fn two_downloads_at_a_time_in_request_order() {
    let env = Env::new(|options| options).await;
    let [a, b, c, d] =
        ["A", "B", "C", "D"].map(|name| format!("chatonlm-dl/{name}-stall-GGUF:Q4_K_M"));
    for model in [&a, &b, &c, &d] {
        env.ask(model);
    }
    // Octets reçus : les requêtes sont ouvertes (`Downloading` s'affiche
    // avant l'envoi de la requête).
    wait_until("two downloads", WAIT, || {
        env.entry(&a).completed > 0 && env.entry(&b).completed > 0
    })
    .await;
    assert_eq!(env.entry(&c).state, Queued);
    assert_eq!(env.entry(&d).state, Queued);
    env.downloads.cancel(&a);
    assert_eq!(env.entry(&a).state, Cancelled);
    wait_until("the third download", WAIT, || {
        env.entry(&c).state == Downloading
    })
    .await;
    assert_eq!(env.entry(&d).state, Queued);
    for model in [&b, &c, &d] {
        env.downloads.cancel(model);
    }
}

/// Pause : requête fermée, `.part` et progression gardés, le suivant
/// démarre ; la reprise repart du `.part` (`Range`) et va au bout.
#[tokio::test]
async fn a_download_can_be_paused_and_resumed() {
    let env = Env::new(|options| Options {
        max_running: 1,
        ..options
    })
    .await;
    let slow = "chatonlm-dl/Turtle-slow-big-GGUF:Q4_K_M";
    let next = "chatonlm-dl/Next-stall-GGUF:Q4_K_M";
    // Retenu passé sa moitié jusqu'à la reprise : la pause tombe toujours en
    // plein téléchargement, même réveillé en retard (relecture L8, M7).
    env.fake.hold("Turtle-Q4_K_M.gguf");
    env.ask(slow);
    env.ask(next);
    wait_until("some progress", WAIT, || {
        let download = env.entry(slow);
        download.state == Downloading && download.completed > 20_000
    })
    .await;
    assert_eq!(env.entry(next).state, Queued);
    env.downloads.pause(slow);
    let paused = env.entry(slow);
    assert_eq!(paused.state, Paused);
    assert!(paused.completed > 0 && paused.total > 0, "{paused:?}");
    assert_eq!((paused.bytes_per_second, paused.finished_at), (None, None));
    wait_until("the closed request", WAIT, || env.fake.disconnects() == 1).await;
    wait_until("the next download", WAIT, || {
        env.entry(next).state == Downloading
    })
    .await;
    let part = part_path(
        &env.models()
            .join("chatonlm-dl/Turtle-slow-big-GGUF/Turtle-Q4_K_M.gguf"),
    );
    let kept = std::fs::metadata(&part).unwrap().len();
    assert!(kept > 0);
    // Sans effet : déjà en pause, inconnu.
    env.downloads.pause(slow);
    env.downloads.pause("chatonlm-dl/Nobody-GGUF:Q4_K_M");

    env.fake.release();
    env.downloads.resume(slow).unwrap();
    assert_eq!(env.entry(slow).state, Queued);
    env.downloads.cancel(next);
    let done = env.finished(slow).await;
    assert_eq!(done.state, Done, "{done:?}");
    let ranges: Vec<_> = env
        .fake
        .requests()
        .into_iter()
        .filter(|request| request.path.ends_with("Turtle-Q4_K_M.gguf"))
        .map(|request| request.range)
        .collect();
    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[0], None);
    let resumed_at: u64 = ranges[1]
        .as_deref()
        .and_then(|range| range.strip_prefix("bytes="))
        .and_then(|range| range.strip_suffix('-'))
        .and_then(|start| start.parse().ok())
        .unwrap();
    assert!(resumed_at >= kept, "{resumed_at} < {kept}");
    assert!(env.installed().iter().any(|row| row.id == slow));
}

/// « Download first » : en tête et démarré ; le dernier en cours retourne en
/// file (requête fermée) ; en pause : repris.
#[tokio::test]
async fn a_download_can_be_put_first() {
    let env = Env::new(|options| options).await;
    let [a, b, c, d] =
        ["A", "B", "C", "D"].map(|name| format!("chatonlm-dl/{name}-stall-GGUF:Q4_K_M"));
    for model in [&a, &b, &c, &d] {
        env.ask(model);
    }
    // Octets reçus : les requêtes sont ouvertes (`Downloading` s'affiche
    // avant l'envoi de la requête).
    wait_until("two downloads", WAIT, || {
        env.entry(&a).completed > 0 && env.entry(&b).completed > 0
    })
    .await;
    let order = || -> Vec<(String, HubDownloadState)> {
        env.downloads
            .list()
            .into_iter()
            .map(|download| (download.model, download.state))
            .collect()
    };
    env.downloads.prioritize(&d).unwrap();
    wait_until("the prioritized download", WAIT, || {
        env.entry(&d).state == Downloading
    })
    .await;
    assert_eq!(
        order(),
        [
            (d.clone(), Downloading),
            (a.clone(), Downloading),
            (b.clone(), Queued),
            (c.clone(), Queued),
        ]
    );
    wait_until("the closed request", WAIT, || env.fake.disconnects() == 1).await;
    assert_eq!(env.entry(&b).bytes_per_second, None);
    // Déjà en cours : en tête seulement.
    env.downloads.prioritize(&a).unwrap();
    assert_eq!(order()[0], (a.clone(), Downloading));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(env.fake.disconnects(), 1);
    // En pause : repris et premier ; D, le dernier en cours, retourne en file.
    env.downloads.pause(&c);
    env.downloads.prioritize(&c).unwrap();
    wait_until("the resumed download", WAIT, || {
        env.entry(&c).state == Downloading
    })
    .await;
    assert_eq!(
        order(),
        [
            (c.clone(), Downloading),
            (a.clone(), Downloading),
            (d.clone(), Queued),
            (b.clone(), Queued),
        ]
    );
    // Inconnu ou fini : rien.
    env.downloads.cancel(&a);
    env.downloads
        .prioritize("chatonlm-dl/Nobody-GGUF:Q4_K_M")
        .unwrap();
    env.downloads.prioritize(&a).unwrap();
    assert_eq!(env.entry(&a).state, Cancelled);
    for model in [&b, &c, &d] {
        env.downloads.cancel(model);
    }
}

/// Annulation en cours : requête fermée, puis ce qui a été reçu pour ce
/// modèle est effacé (`.part`, fichiers déjà complets, dossiers vides) ; un
/// fichier d'un modèle installé reste. En pause : effacé aussi.
///
/// Dépôts `stall` (la moitié du fichier, puis un octet toutes les 200 ms) :
/// l'annulation tombe toujours en plein téléchargement. Avec `slow` (le
/// fichier entier en 1,5 s environ), un essai réveillé en retard sous la
/// charge d'une suite complète annulait parfois pendant l'écriture de la
/// ligne, que l'annulation laisse faire (voir `cancel`) : le modèle
/// s'installait et le ménage attendu ne venait jamais (1 échec sur 4 suites
/// complètes, 9 nov. 2026).
#[tokio::test]
async fn a_cancelled_download_leaves_nothing_behind() {
    let env = Env::new(|options| options).await;
    // Q8_0 installée d'avance (fichiers et ligne, comme après un
    // téléchargement) : un dépôt `stall` ne finirait jamais de la servir.
    let repo = "chatonlm-dl/Shy-vision-stall-big-GGUF";
    let installed = format!("{repo}:Q8_0");
    let mut sha256 = std::collections::BTreeMap::new();
    for file in ["Shy-Q8_0.gguf", "mmproj-Q8_0.gguf"] {
        let bytes = content(repo, file);
        let path = env.models().join(repo).join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        sha256.insert(format!("{repo}/{file}"), sha_of(&bytes));
    }
    let row = StoredLocalModel {
        id: installed.clone(),
        source: crate::local_models::ModelSource::Hf,
        repo: repo.to_owned(),
        revision: None,
        quant: "Q8_0".to_owned(),
        files: vec![format!("{repo}/Shy-Q8_0.gguf")],
        mmproj: Some(format!("{repo}/mmproj-Q8_0.gguf")),
        size_bytes: size(repo, "Shy-Q8_0.gguf") + size(repo, "mmproj-Q8_0.gguf"),
        sha256,
        arch: None,
        context_max: None,
        params_label: None,
        file_type: None,
        capabilities: crate::local_models::ModelCapabilities::default(),
        installed_at: 1,
    };
    env.store.blocking(|c| table::save(c, &row)).unwrap();
    let before = env.files();

    // Q4_K_M annulée pendant ses poids : le projecteur de la Q8_0 installée
    // (le même, mmproj-Q8_0) reste.
    let sibling = format!("{repo}:Q4_K_M");
    env.ask(&sibling);
    wait_until("some progress", WAIT, || {
        env.entry(&sibling).completed > 10_000
    })
    .await;
    env.downloads.cancel(&sibling);
    assert_eq!(env.entry(&sibling).state, Cancelled);
    wait_until("the cleanup", WAIT, || env.files() == before).await;

    let gone = "chatonlm-dl/Gone-stall-big-GGUF:Q4_K_M";
    env.ask(gone);
    wait_until("some progress", WAIT, || env.entry(gone).completed > 10_000).await;
    env.downloads.cancel(gone);
    assert_eq!(env.entry(gone).state, Cancelled);
    // Les fichiers partent avant leur dossier (même nettoyage, en tâche de
    // fond) : attendre les deux, pas seulement les fichiers.
    let gone_dir = env.models().join("chatonlm-dl/Gone-stall-big-GGUF");
    wait_until("the cleanup", WAIT, || {
        env.files() == before && !gone_dir.exists()
    })
    .await;

    // En pause, puis annulé : les `.part` partent aussi (fiche relue).
    let paused = "chatonlm-dl/Nap-stall-big-GGUF:Q4_K_M";
    env.ask(paused);
    wait_until("some progress", WAIT, || {
        env.entry(paused).completed > 10_000
    })
    .await;
    env.downloads.pause(paused);
    assert_eq!(env.entry(paused).state, Paused);
    wait_until("the kept part", WAIT, || env.files().len() > before.len()).await;
    env.downloads.cancel(paused);
    assert_eq!(env.entry(paused).state, Cancelled);
    wait_until("the cleanup", WAIT, || env.files() == before).await;
    assert!(env.files().contains(&format!("{repo}/mmproj-Q8_0.gguf")));
    let ids: Vec<String> = env.installed().into_iter().map(|model| model.id).collect();
    assert_eq!(ids, [installed]);
}

/// Annulé en pause pendant que la tâche arrêtée par la pause tient encore
/// ses fichiers (fin lente : machine chargée) : ses `.part` partent quand
/// même, à sa fin. Avant, la suppression les sautait (réservés), puis la
/// tâche les gardait (arrêt « pause »).
#[tokio::test]
async fn a_cancel_during_the_end_of_a_paused_task_leaves_nothing() {
    let env = Env::new(|options| options).await;
    let (open, gate) = watch::channel(false);
    *env.downloads.inner.stop_gate.lock() = Some(gate);
    let model = "chatonlm-dl/Nap-stall-big-GGUF:Q4_K_M";
    env.ask(model);
    wait_until("some progress", WAIT, || {
        env.entry(model).completed > 10_000
    })
    .await;
    env.downloads.pause(model);
    env.downloads.cancel(model);
    assert_eq!(env.entry(model).state, Cancelled);
    // Une suppression lancée tout de suite (compteur à 2) a eu le temps de
    // finir avant que la tâche arrêtée rende ses fichiers.
    wait_until("the immediate discard", WAIT, || {
        *env.downloads.inner.live.borrow() == 1
    })
    .await;
    assert!(!env.files().is_empty());
    open.send_replace(true);
    wait_until("the cleanup", WAIT, || {
        env.files().is_empty() && *env.downloads.inner.live.borrow() == 0
    })
    .await;
}

/// Échecs lisibles, sans rien installer.
#[tokio::test]
async fn failures_are_readable() {
    let env = Env::new(|options| options).await;
    for (model, error) in [
        ("chatonlm-dl/Gone-missing-GGUF:Q4_K_M", NOT_FOUND_OR_PRIVATE.to_owned()),
        ("chatonlm-dl/Lab-gated-GGUF:Q4_K_M", NOT_FOUND_OR_PRIVATE.to_owned()),
        ("chatonlm-dl/Tiny-GGUF:Q6_K", UNAVAILABLE_VERSION.to_owned()),
        (
            "chatonlm-dl/Weights-notgguf-GGUF:Q4_K_M",
            "Weights-Q4_K_M.gguf isn't a GGUF model file: ChatonLM can't use it.".to_owned(),
        ),
        (
            "chatonlm-dl/Bad-badsha-GGUF:Q4_K_M",
            "Bad-Q4_K_M.gguf is damaged (checksum mismatch): download it again".to_owned(),
        ),
        (
            "chatonlm-dl/Raw-nolfs-GGUF:Q4_K_M",
            "Hugging Face gives no checksum for Raw-Q4_K_M.gguf: it can't be checked, so it isn't downloaded.".to_owned(),
        ),
        ("chatonlm-dl/Short-cut-GGUF:Q4_K_M", CONNECTION_LOST.to_owned()),
    ] {
        env.ask(model);
        let failed = env.finished(model).await;
        assert_eq!(failed.state, Failed, "{model}");
        let message = failed.error.unwrap_or_default();
        assert!(message.starts_with(&error), "{model}: {message}");
    }
    assert!(env.installed().is_empty());
    // Ni le fichier qui n'est pas un GGUF, ni celui à l'empreinte fausse ne
    // restent ; le morceau coupé reste pour une reprise, avec son empreinte
    // attendue.
    wait_until("the cleanup", WAIT, || {
        env.files()
            == [
                "chatonlm-dl/Short-cut-GGUF/Short-Q4_K_M.gguf.part",
                "chatonlm-dl/Short-cut-GGUF/Short-Q4_K_M.gguf.part.sha256",
            ]
    })
    .await;
    // Aucun GGUF du tout.
    let plan = files::plan(
        &ModelId::parse("a/b:Q4_K_M").unwrap(),
        &files::Listing {
            id: None,
            sha: "x".into(),
            gated: false,
            files: vec![],
        },
    );
    assert_eq!(plan.unwrap_err(), NOT_GGUF);
}

/// Fiche illisible (page HTML d'un CDN), Hugging Face injoignable.
#[tokio::test]
async fn an_unusable_listing_is_told() {
    let html = Env::with_mode(HfMode::Html, |options| options).await;
    let model = "chatonlm-dl/Tiny-GGUF:Q4_K_M";
    html.ask(model);
    assert_eq!(
        html.finished(model).await.error.as_deref(),
        Some("Hugging Face sent an unreadable list of files. Try again later.")
    );
    let closed = FakeHf::start(HfMode::Normal).await.unwrap();
    let url = closed.url();
    closed.stop().await;
    let env = Env::new(|options| Options {
        hf_url: Some(url.clone()),
        ..options
    })
    .await;
    env.ask(model);
    let failed = env.finished(model).await;
    assert!(
        failed
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("can't reach Hugging Face")),
        "{failed:?}"
    );
}

/// Plus aucun octet pendant le délai d'inactivité : échec, `.part` gardé.
#[tokio::test]
async fn silence_ends_a_download() {
    let env = Env::new(|options| Options {
        idle_timeout: Duration::from_millis(300),
        ..options
    })
    .await;
    let stall = "chatonlm-dl/Silent-mute-GGUF:Q4_K_M";
    env.ask(stall);
    let failed = env.finished(stall).await;
    assert_eq!(failed.state, Failed);
    assert_eq!(failed.error.as_deref(), Some(TIMED_OUT));
    assert!(failed.completed > 0);
    assert_eq!(
        env.files(),
        [
            "chatonlm-dl/Silent-mute-GGUF/Silent-Q4_K_M.gguf.part",
            "chatonlm-dl/Silent-mute-GGUF/Silent-Q4_K_M.gguf.part.sha256",
        ]
    );
}

fn no_space(_: &Path) -> Option<u64> {
    Some(1_000)
}

/// Place vérifiée avant le premier octet.
#[tokio::test]
async fn space_is_checked_before_downloading() {
    let env = Env::new(|options| Options {
        free_space: no_space,
        ..options
    })
    .await;
    let model = "chatonlm-dl/Tiny-GGUF:Q4_K_M";
    env.ask(model);
    let failed = env.finished(model).await;
    assert!(
        failed
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("Not enough disk space")),
        "{failed:?}"
    );
    assert!(env.fetched().is_empty());
}

/// Sans base (illisible au démarrage), un téléchargement le dit.
#[tokio::test]
async fn without_a_database_downloads_fail_clearly() {
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let downloads = HubDownloads::with_options(
        |_| {},
        Options {
            hf_url: Some(fake.url()),
            ..Options::default()
        },
    );
    let model = "chatonlm-dl/Tiny-GGUF:Q4_K_M";
    downloads.download(ask(model)).unwrap();
    let what = "the failure";
    wait_until(what, WAIT, || entry(&downloads, model).state == Failed).await;
    assert_eq!(
        entry(&downloads, model).error.as_deref(),
        Some("Models can't be installed: ChatonLM's database couldn't be opened.")
    );
    assert!(fake.requests().is_empty());
}

/// Fini, échoué ou arrêté : remplacé par une nouvelle demande, retiré à la
/// demande ; casse de la quantification ignorée ; identifiants invalides
/// refusés.
#[tokio::test]
async fn finished_downloads_are_replaced_or_dismissed() {
    let env = Env::new(|options| options).await;
    let cut = "chatonlm-dl/Short-cut-GGUF:Q4_K_M";
    env.ask(cut);
    assert_eq!(env.finished(cut).await.state, Failed);
    env.downloads
        .download(HubDownloadRequest {
            model: cut.into(),
            repo: Some("chatonlm-dl/Short-cut-GGUF".into()),
            label: Some("Q4_K_M".into()),
        })
        .unwrap();
    assert_eq!(env.downloads.list().len(), 1);
    assert_eq!(env.finished(cut).await.state, Failed);
    // Même modèle, autre casse de quantification : le même.
    env.downloads.dismiss("chatonlm-dl/Short-cut-GGUF:q4_k_m");
    assert!(env.downloads.list().is_empty());
    for bad in [
        "chatonlm-dl/Short-cut-GGUF",
        "qwen3:0.6b",
        "@ollama/qwen3:0.6b",
        "chatonlm-dl/Short cut:Q4",
        "",
    ] {
        assert!(
            matches!(env.downloads.download(ask(bad)), Err(AppError::Invalid(_))),
            "{bad}"
        );
    }
    // Message lisible (montré par la proposition de VS Code).
    let Err(AppError::Invalid(message)) = env.downloads.download(ask(" qwen2.5-coder:1.5b-base "))
    else {
        panic!("expected a refusal");
    };
    assert_eq!(
        message,
        "qwen2.5-coder:1.5b-base isn't a Hugging Face model ID (author/repository:QUANT)."
    );
}

/// Changement de dossier propre à cet essai.
static CHANGING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn changing() -> bool {
    CHANGING.load(std::sync::atomic::Ordering::SeqCst)
}

/// Pendant un changement du dossier des modèles, rien ne commence ni ne
/// reprend.
#[tokio::test]
async fn nothing_starts_while_the_model_location_changes() {
    let env = Env::new(|options| Options {
        location_changing: changing,
        ..options
    })
    .await;
    let model = "chatonlm-dl/Held-stall-GGUF:Q4_K_M";
    env.ask(model);
    env.downloads.pause(model);
    CHANGING.store(true, std::sync::atomic::Ordering::SeqCst);
    for refused in [
        env.downloads.download(ask("chatonlm-dl/Other-GGUF:Q4_K_M")),
        env.downloads.resume(model),
        env.downloads.prioritize(model),
    ] {
        let Err(AppError::Invalid(message)) = refused else {
            panic!("expected a refusal");
        };
        assert_eq!(message, LOCATION_CHANGING);
    }
    assert_eq!(env.entry(model).state, Paused);
    CHANGING.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(env.entry(model).state, Paused);
    env.downloads.cancel(model);
}

/// Progression limitée à une émission par intervalle ; la dernière part à
/// l'échéance même si plus rien n'arrive.
#[tokio::test]
async fn progress_events_are_limited() {
    let env = Env::new(|options| options).await;
    let interval = Duration::from_millis(250);
    let slow = "chatonlm-dl/Steady-slow-big-GGUF:Q4_K_M";
    env.ask(slow);
    assert_eq!(env.finished(slow).await.state, Done);
    let events = env.recorder.events();
    let mut progress = 0;
    for pair in events.windows(2) {
        let ((before, previous), (at, list)) = (&pair[0], &pair[1]);
        let states = |list: &[HubDownload]| list.iter().map(|d| d.state).collect::<Vec<_>>();
        if states(previous) == states(list) {
            progress += 1;
            let gap = at.duration_since(*before);
            assert!(gap >= interval - Duration::from_millis(20), "{gap:?}");
        }
    }
    assert!(progress >= 1, "{events:?}");

    let stall = "chatonlm-dl/Still-mute-GGUF:Q4_K_M";
    env.ask(stall);
    let half = size("chatonlm-dl/Still-mute-GGUF", "Still-Q4_K_M.gguf") / 2;
    wait_until("the held progress", WAIT, || {
        env.recorder
            .of(stall)
            .last()
            .is_some_and(|d| d.completed == half)
    })
    .await;
    env.downloads.cancel(stall);
}

/// Sortie : requêtes fermées, `.part` gardés, plus rien ne démarre.
#[tokio::test]
async fn leaving_closes_running_downloads() {
    let env = Env::new(|options| options).await;
    let [a, b, c] = ["A", "B", "C"].map(|name| format!("chatonlm-dl/{name}-stall-GGUF:Q4_K_M"));
    for model in [&a, &b, &c] {
        env.ask(model);
    }
    // Octets reçus : les requêtes sont ouvertes (`Downloading` s'affiche
    // avant l'envoi de la requête).
    wait_until("two downloads", WAIT, || {
        env.entry(&a).completed > 0 && env.entry(&b).completed > 0
    })
    .await;
    // `stop_all` rend la main parce que les tâches sont finies (compteur à
    // zéro), pas parce que sa limite est passée. Pas de mesure de durée :
    // une tâche arrêtée ferme son `.part` sur le disque, dont la lenteur
    // (TMPDIR sur ntfs3 sous check.sh) dépassait la seconde exigée.
    env.downloads.stop_all(WAIT).await;
    assert_eq!(*env.downloads.inner.live.borrow(), 0);
    wait_until("the closed requests", WAIT, || env.fake.disconnects() == 2).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(env.entry(&c).state, Queued);
    // Un `.part` et son empreinte attendue par téléchargement.
    assert_eq!(env.files().len(), 4);
    // Après la sortie, une demande ne fait rien.
    env.ask("chatonlm-dl/Late-GGUF:Q4_K_M");
    assert_eq!(env.downloads.list().len(), 3);
}

#[test]
fn the_rate_is_smoothed_over_three_seconds() {
    let start = Instant::now();
    let second = Duration::from_secs(1);
    let mut rate = Rate::default();
    // Reprise : le premier relevé compte déjà des octets.
    assert_eq!(rate.add(start, 5_000), None);
    assert_eq!(rate.add(start + Duration::from_millis(250), 6_000), None);
    assert_eq!(rate.add(start + second, 9_000), Some(4_000));
    // 1 Mo/s régulier : 1 Mo/s.
    let at = |seconds: u32| start + second * seconds;
    let completed = |seconds: u64| 9_000 + (seconds - 1) * 1_000_000;
    for seconds in 2..=10 {
        rate.add(at(seconds), completed(u64::from(seconds)));
    }
    assert_eq!(rate.samples.front().unwrap().0, at(7));
    assert_eq!(rate.add(at(11), completed(11)), Some(1_000_000));
    // Arrêt net : le débit baisse sur la fenêtre, sans tomber à zéro d'un coup.
    assert_eq!(rate.add(at(12), completed(11)), Some(666_667));
    assert_eq!(rate.add(at(15), completed(11)), Some(0));
}

/// Nombre de demandes de la fiche du dépôt `repo`.
fn listings(env: &Env, repo: &str) -> usize {
    env.fake
        .requests()
        .iter()
        .filter(|request| request.path.starts_with(&format!("/api/models/{repo}")))
        .count()
}

/// Pause, nouvelle révision publiée sur `main`, reprise : la suite vient du
/// commit du début (`resolve/<ancien commit>`, `Range`), sans relire la
/// fiche ; le fichier installé est celui de l'ancienne révision.
#[tokio::test]
async fn a_resumed_download_keeps_its_revision() {
    let env = Env::new(|options| options).await;
    let repo = "chatonlm-dl/Moving-slow-big-GGUF";
    let file = "Moving-Q4_K_M.gguf";
    let model = format!("{repo}:Q4_K_M");
    let old = Synth::parse(repo, 0).unwrap().sha();
    // Retenu jusqu'à la reprise : la pause tombe en plein téléchargement.
    env.fake.hold(file);
    env.ask(&model);
    wait_until("some progress", WAIT, || {
        env.entry(&model).completed > 20_000
    })
    .await;
    env.downloads.pause(&model);
    assert_eq!(env.entry(&model).state, Paused);
    wait_until("the closed request", WAIT, || env.fake.disconnects() == 1).await;
    env.fake.publish(repo);
    env.fake.release();
    env.downloads.resume(&model).unwrap();
    let done = env.finished(&model).await;
    assert_eq!(done.state, Done, "{done:?}");
    let requests: Vec<_> = env
        .fake
        .requests()
        .into_iter()
        .filter(|request| request.path.ends_with(file))
        .collect();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert!(
            request.path.contains(&format!("/resolve/{old}/")),
            "{}",
            request.path
        );
    }
    assert!(requests[1].range.is_some());
    assert_eq!(listings(&env, repo), 1);
    let row = env.installed().remove(0);
    assert_eq!(row.revision.as_deref(), Some(old.as_str()));
    assert_eq!(
        std::fs::read(env.models().join(repo).join(file)).unwrap(),
        content(repo, file)
    );
}

/// Un `.part` d'une session précédente, d'une révision remplacée depuis
/// (empreinte attendue écrite à côté) : il repart de zéro au lieu d'être
/// complété avec les octets de la nouvelle révision ; l'empreinte attendue
/// part avec le renommage.
#[tokio::test]
async fn a_part_of_another_revision_starts_over() {
    let env = Env::new(|options| options).await;
    let repo = "chatonlm-dl/Stale-big-GGUF";
    let file = "Stale-Q4_K_M.gguf";
    let old = content(repo, file);
    let target = env.models().join(repo).join(file);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(part_path(&target), &old[..old.len() / 2]).unwrap();
    std::fs::write(local_models::expected_path(&target), sha_of(&old)).unwrap();
    env.fake.publish(repo);
    let model = format!("{repo}:Q4_K_M");
    env.ask(&model);
    let done = env.finished(&model).await;
    assert_eq!(done.state, Done, "{done:?}");
    let ranges: Vec<_> = env
        .fake
        .requests()
        .into_iter()
        .filter(|request| request.path.ends_with(file))
        .map(|request| request.range)
        .collect();
    assert_eq!(ranges, [None]);
    let new = Synth::parse(repo, 1).unwrap().content(file).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), new);
    assert_eq!(env.files(), [format!("{repo}/{file}")]);
}

/// Un morceau fini puis une pause : à la reprise, il n'est pas haché de
/// nouveau (vérifié pendant cette session, à taille et date égales).
#[tokio::test]
async fn finished_parts_are_not_hashed_again() {
    let env = Env::new(|options| options).await;
    let repo = "chatonlm-dl/Pair-sharded-slow-big-GGUF";
    let first = "Pair-Q8_0-00001-of-00002.gguf";
    let model = format!("{repo}:Q8_0");
    // Second morceau retenu jusqu'à la reprise : la pause tombe en plein
    // dedans.
    env.fake.hold("Pair-Q8_0-00002-of-00002.gguf");
    env.ask(&model);
    let past = size(repo, first) + 20_000;
    wait_until("the second part", WAIT, || {
        env.entry(&model).completed > past
    })
    .await;
    env.downloads.pause(&model);
    assert_eq!(env.entry(&model).state, Paused);
    wait_until("the closed request", WAIT, || env.fake.disconnects() == 1).await;
    env.fake.release();
    env.downloads.resume(&model).unwrap();
    assert_eq!(env.finished(&model).await.state, Done);
    let hashed = env.downloads.inner.hashed.lock().clone();
    let times = hashed
        .iter()
        .filter(|path| path.to_string_lossy().contains(first))
        .count();
    assert_eq!(times, 1, "{hashed:?}");
}

fn plan_file(path: &str) -> PlanFile {
    PlanFile {
        path: path.into(),
        size: 1,
        sha256: "0".repeat(64),
    }
}

/// Seul un fichier qui n'est pas un GGUF fait effacer ce qui a été reçu ;
/// une erreur de lecture (projecteur supprimé ou verrouillé) garde tout.
#[test]
fn only_a_file_that_is_not_gguf_is_thrown_away() {
    let plan = Plan {
        id: "u/V-GGUF:Q4_K_M".into(),
        repo: "u/V-GGUF".into(),
        revision: "c0ffee".into(),
        quant: "Q4_K_M".into(),
        files: vec![plan_file("V-Q4_K_M.gguf")],
        mmproj: Some(plan_file("mmproj-F16.gguf")),
    };
    let io = || MetaError::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    let ok = || Ok(meta::ModelMeta::default());
    let unusable = |outcome: Result<_, (End, bool)>| outcome.err().map(|(_, unusable)| unusable);
    assert_eq!(
        unusable(read_outcome(&plan, ok(), Some(Err(io())))),
        Some(false)
    );
    assert_eq!(
        unusable(read_outcome(&plan, ok(), Some(Err(MetaError::NotGguf)))),
        Some(true)
    );
    assert_eq!(
        unusable(read_outcome(&plan, Err(io()), Some(ok()))),
        Some(false)
    );
    assert_eq!(
        unusable(read_outcome(&plan, Err(MetaError::NotGguf), Some(ok()))),
        Some(true)
    );
    let Err((End::Failed(message), _)) = read_outcome(&plan, ok(), Some(Err(io()))) else {
        panic!("expected a failure");
    };
    assert!(
        message.starts_with("Could not check mmproj-F16.gguf"),
        "{message}"
    );
    // Supprimé avant la lecture des en-têtes : « manquant », comme s'il
    // disparaissait juste après (rien d'effacé non plus).
    let gone = || MetaError::Io(std::io::Error::from(std::io::ErrorKind::NotFound));
    let Err((End::Failed(message), false)) = read_outcome(&plan, ok(), Some(Err(gone()))) else {
        panic!("expected a failure that keeps the files");
    };
    assert_eq!(
        message,
        "mmproj-F16.gguf is missing: download the model again."
    );
    assert!(read_outcome(&plan, ok(), Some(ok())).is_ok());
}

/// Une version en `Finishing` garde réservé le projecteur qu'elle partage :
/// supprimer pendant ce temps l'autre version installée ne l'efface pas, et
/// elle s'installe.
#[tokio::test]
async fn a_deletion_spares_the_files_of_an_install_in_progress() {
    let env = Env::new(|options| Options {
        finish_delay: Duration::from_millis(600),
        ..options
    })
    .await;
    let q4 = format!("{EYE}:Q4_K_M");
    let q8 = format!("{EYE}:Q8_0");
    env.ask(&q4);
    assert_eq!(env.finished(&q4).await.state, Done);
    env.ask(&q8);
    wait_until("the installation", WAIT, || {
        env.entry(&q8).state == Finishing
    })
    .await;
    local_models::delete(&env.store, env.dir.path(), &q4, &env.downloads.reserved())
        .await
        .unwrap();
    assert!(env.models().join(EYE).join("mmproj-Q8_0.gguf").exists());
    let done = env.finished(&q8).await;
    assert_eq!(done.state, Done, "{done:?}");
    assert_eq!(
        env.files(),
        [
            format!("{EYE}/Eye-Q8_0.gguf"),
            format!("{EYE}/mmproj-Q8_0.gguf")
        ]
    );
}

/// Annuler une version qui attend le projecteur qu'une autre installe ne
/// l'efface pas : il est réservé.
#[tokio::test]
async fn a_cancellation_spares_the_files_another_download_holds() {
    let env = Env::new(|options| Options {
        finish_delay: Duration::from_millis(1500),
        ..options
    })
    .await;
    let repo = "chatonlm-dl/Both-vision-GGUF";
    let q8 = format!("{repo}:Q8_0");
    let q4 = format!("{repo}:Q4_K_M");
    env.ask(&q8);
    wait_until("the installation", WAIT, || {
        env.entry(&q8).state == Finishing
    })
    .await;
    env.ask(&q4);
    let weights = size(repo, "Both-Q4_K_M.gguf");
    wait_until("the wait for the projector", WAIT, || {
        env.entry(&q4).completed >= weights
    })
    .await;
    env.downloads.cancel(&q4);
    let done = env.finished(&q8).await;
    assert_eq!(done.state, Done, "{done:?}");
    wait_until("the cleanup", WAIT, || {
        env.files()
            == [
                format!("{repo}/Both-Q8_0.gguf"),
                format!("{repo}/mmproj-Q8_0.gguf"),
            ]
    })
    .await;
}

/// Annulé pendant `Finishing`, avant l'écriture de la ligne : rien
/// d'installé, fichiers effacés.
#[tokio::test]
async fn a_cancellation_while_finishing_installs_nothing() {
    let env = Env::new(|options| Options {
        finish_delay: Duration::from_millis(400),
        ..options
    })
    .await;
    let model = "chatonlm-dl/Late-GGUF:Q4_K_M";
    env.ask(model);
    wait_until("the installation", WAIT, || {
        env.entry(model).state == Finishing
    })
    .await;
    env.downloads.cancel(model);
    assert_eq!(env.finished(model).await.state, Cancelled);
    wait_until("the cleanup", WAIT, || env.files().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(env.installed().is_empty());
}

/// Un fichier disparu pendant `Finishing` (autre programme) : pas de ligne
/// sans ses fichiers.
#[tokio::test]
async fn a_file_gone_before_the_end_installs_nothing() {
    let env = Env::new(|options| Options {
        finish_delay: Duration::from_millis(400),
        ..options
    })
    .await;
    let model = format!("{EYE}:Q4_K_M");
    env.ask(&model);
    // `Finishing` paraît au dernier octet reçu, parfois avant que le `.part`
    // ait pris son nom : attendre aussi le fichier (machine chargée).
    let projector = env.models().join(EYE).join("mmproj-Q8_0.gguf");
    wait_until("the installation", WAIT, || {
        env.entry(&model).state == Finishing && projector.is_file()
    })
    .await;
    std::fs::remove_file(&projector).unwrap();
    let failed = env.finished(&model).await;
    assert_eq!(failed.state, Failed);
    assert_eq!(
        failed.error.as_deref(),
        Some("mmproj-Q8_0.gguf is missing: download the model again.")
    );
    assert!(env.installed().is_empty());
}

/// Mise à jour d'un modèle installé, annulée : le fichier installé reste,
/// son `.part` (de la nouvelle révision) part avec son empreinte attendue.
#[tokio::test]
async fn a_cancelled_update_leaves_no_part_behind() {
    let env = Env::new(|options| options).await;
    let repo = "chatonlm-dl/Upd-slow-big-GGUF";
    let model = format!("{repo}:Q4_K_M");
    env.ask(&model);
    assert_eq!(env.finished(&model).await.state, Done);
    let before = env.files();
    env.fake.publish(repo);
    env.ask(&model);
    wait_until("the kept part", WAIT, || env.files().len() > before.len()).await;
    env.downloads.cancel(&model);
    wait_until("the cleanup", WAIT, || env.files() == before).await;
    assert_eq!(env.installed().len(), 1);
}

/// Annulation d'un téléchargement en pause : son plan suffit pour effacer
/// ses `.part` (la fiche n'est pas relue : marche aussi hors ligne).
#[tokio::test]
async fn a_paused_download_is_cleaned_from_its_plan() {
    let env = Env::new(|options| options).await;
    let repo = "chatonlm-dl/Offline-slow-big-GGUF";
    let model = format!("{repo}:Q4_K_M");
    // Retenu : la pause tombe en plein téléchargement.
    env.fake.hold("Offline-Q4_K_M.gguf");
    env.ask(&model);
    wait_until("some progress", WAIT, || {
        env.entry(&model).completed > 20_000
    })
    .await;
    env.downloads.pause(&model);
    assert_eq!(env.entry(&model).state, Paused);
    wait_until("the closed request", WAIT, || env.fake.disconnects() == 1).await;
    let target = env.models().join(repo).join("Offline-Q4_K_M.gguf");
    assert!(part_path(&target).exists());
    assert!(local_models::expected_path(&target).exists());
    env.downloads.cancel(&model);
    wait_until("the cleanup", WAIT, || env.files().is_empty()).await;
    assert_eq!(listings(&env, repo), 1);
}

/// Place libre lue au plus par l'essai de la place partagée.
static FREE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn shared_space(_: &Path) -> Option<u64> {
    Some(FREE.load(Ordering::Relaxed))
}

/// Deux téléchargements qui tiennent chacun seul, mais pas ensemble : le
/// second refusé (la place qui reste aux autres est comptée).
#[tokio::test]
async fn space_is_shared_between_downloads() {
    let a = "chatonlm-dl/Ape-stall-big-GGUF";
    let b = "chatonlm-dl/Bee-stall-big-GGUF";
    let need = size(a, "Ape-Q4_K_M.gguf");
    assert_eq!(need, size(b, "Bee-Q4_K_M.gguf"));
    FREE.store(
        crate::image_models::downloads::SPACE_MARGIN + need + need / 5,
        Ordering::Relaxed,
    );
    let env = Env::new(|options| Options {
        free_space: shared_space,
        ..options
    })
    .await;
    let models = [format!("{a}:Q4_K_M"), format!("{b}:Q4_K_M")];
    for model in &models {
        env.ask(model);
    }
    wait_until("one failure", WAIT, || {
        models.iter().any(|model| env.entry(model).state == Failed)
    })
    .await;
    let failed: Vec<HubDownload> = models
        .iter()
        .map(|model| env.entry(model))
        .filter(|download| download.state == Failed)
        .collect();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(
        failed[0]
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("Not enough disk space")),
        "{failed:?}"
    );
    for model in &models {
        env.downloads.cancel(model);
    }
}

/// Projecteur audio seulement : audio, pas vision ; modèle de base dit.
#[tokio::test]
async fn capabilities_come_from_the_projector_and_the_weights() {
    let env = Env::new(|options| options).await;
    let model = "chatonlm-dl/Ear-audio-base-fim-GGUF:Q4_K_M";
    env.ask(model);
    assert_eq!(env.finished(model).await.state, Done);
    let row = env.installed().remove(0);
    assert!(row.mmproj.is_some());
    assert_eq!(
        row.capabilities,
        ModelCapabilities {
            audio: true,
            fim: true,
            base: true,
            ..ModelCapabilities::default()
        }
    );
}
