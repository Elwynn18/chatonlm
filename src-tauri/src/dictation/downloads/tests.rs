//! Tests des téléchargements des modèles de transcription, contre un petit
//! serveur HTTP local (fichiers en mémoire, `Range`, coupures, lenteur).

use std::collections::HashMap;
use std::sync::OnceLock;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::dictation::models::SpeechEngine;

use SpeechDownloadState::{Cancelled, Done, Downloading, Failed, Queued, Verifying};

const WAIT: Duration = Duration::from_secs(10);
const REPO: &str = "chatonlm/speech-test";
const REVISION: &str = "0123abcd";

/// Contenu déterministe d'un fichier de test.
fn content(name: &str, length: usize) -> Vec<u8> {
    let seed = name
        .bytes()
        .fold(7u8, |a, b| a.wrapping_mul(31).wrapping_add(b));
    (0..length)
        .map(|i| seed.wrapping_add((i % 251) as u8).wrapping_mul(3))
        .collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Fichiers servis : nom → (taille, empreinte déclarée au catalogue).
const FILES: &[(&str, &str, usize)] = &[
    ("two", "two-a.bin", 100_000),
    ("two", "two-b.bin", 50_000),
    ("damaged", "damaged.bin", 20_000),
    ("missing", "missing.bin", 1_000),
    ("slow", "slow.bin", 400_000),
];

fn catalog() -> &'static [CatalogModel] {
    static CATALOG: OnceLock<Vec<CatalogModel>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        ["two", "damaged", "missing", "slow"]
            .into_iter()
            .map(|id| {
                let files: Vec<ModelFile> = FILES
                    .iter()
                    .filter(|(model, _, _)| *model == id)
                    .map(|&(_, path, size)| {
                        let declared = if id == "damaged" {
                            sha256_hex(b"something else")
                        } else {
                            sha256_hex(&content(path, size))
                        };
                        ModelFile {
                            path,
                            size: size as u64,
                            sha256: Box::leak(declared.into_boxed_str()),
                        }
                    })
                    .collect();
                CatalogModel {
                    id,
                    name: id,
                    engine: SpeechEngine::Whisper,
                    description: "",
                    languages: "",
                    license: "MIT",
                    license_url: "",
                    repo: REPO,
                    revision: REVISION,
                    files: Box::leak(files.into_boxed_slice()),
                    recommended: false,
                }
            })
            .collect()
    })
}

fn find(id: &str) -> Option<&'static CatalogModel> {
    catalog().iter().find(|model| model.id == id)
}

/// Le même catalogue, vu par la file.
fn find_item(id: &str) -> Option<&'static dyn DownloadItem> {
    find(id).map(|model| model as &'static dyn DownloadItem)
}

/// Comportement du serveur pour un fichier.
#[derive(Clone, Copy, Debug)]
enum Serve {
    /// Tout le fichier (ou la suite demandée).
    Normal,
    /// En-têtes du fichier complet, puis ce nombre d'octets, puis la connexion
    /// est fermée.
    CutAfter(usize),
    /// 4 Kio toutes les 10 ms.
    Slow,
    /// Tout le fichier (200), même quand une suite est demandée (`Range`
    /// ignoré, comme certains proxys).
    IgnoreRange,
}

#[derive(Default)]
struct Shared {
    serve: Mutex<HashMap<String, Serve>>,
    /// (fichier, en-tête Range) de chaque requête.
    requests: Mutex<Vec<(String, Option<String>)>>,
}

struct Server {
    url: String,
    shared: Arc<Shared>,
}

impl Server {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let shared = Arc::new(Shared::default());
        let state = shared.clone();
        tauri::async_runtime::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tauri::async_runtime::spawn(answer(stream, state.clone()));
            }
        });
        Self { url, shared }
    }

    fn serve(&self, name: &str, serve: Serve) {
        self.shared.serve.lock().insert(name.into(), serve);
    }

    fn requests(&self) -> Vec<(String, Option<String>)> {
        self.shared.requests.lock().clone()
    }
}

async fn answer(mut stream: tokio::net::TcpStream, shared: Arc<Shared>) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).await.unwrap_or(0) == 0 {
            return;
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let path = head
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    let range = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("range")
            .then(|| value.trim().to_owned())
    });
    let prefix = format!("/{REPO}/resolve/{REVISION}/");
    let name = path.strip_prefix(&prefix).unwrap_or_default().to_owned();
    shared.requests.lock().push((name.clone(), range.clone()));
    let Some(&(_, _, size)) = FILES
        .iter()
        .find(|(_, file, _)| *file == name)
        .filter(|(model, _, _)| *model != "missing")
    else {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return;
    };
    let body = content(&name, size);
    let serve = shared
        .serve
        .lock()
        .get(&name)
        .copied()
        .unwrap_or(Serve::Normal);
    let range = range.filter(|_| !matches!(serve, Serve::IgnoreRange));
    let start: usize = range
        .as_deref()
        .and_then(|range| range.strip_prefix("bytes="))
        .and_then(|range| range.trim_end_matches('-').parse().ok())
        .unwrap_or(0);
    let head = if start > 0 {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{size}\r\nConnection: close\r\n\r\n",
            size - start,
            size - 1
        )
    } else {
        format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n")
    };
    if stream.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    let rest = &body[start..];
    match serve {
        Serve::Normal | Serve::IgnoreRange => {
            let _ = stream.write_all(rest).await;
        }
        Serve::CutAfter(count) => {
            let _ = stream.write_all(&rest[..count.min(rest.len())]).await;
            let _ = stream.flush().await;
        }
        Serve::Slow => {
            for chunk in rest.chunks(4096) {
                if stream.write_all(chunk).await.is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
    let _ = stream.shutdown().await;
}

/// Liste émise, datée.
type Emitted = (Instant, Vec<SpeechDownload>);

/// Listes émises.
#[derive(Clone, Default)]
struct Recorder {
    events: Arc<Mutex<Vec<Emitted>>>,
}

impl Recorder {
    fn states(&self, id: &str) -> Vec<SpeechDownloadState> {
        let mut states: Vec<SpeechDownloadState> = Vec::new();
        for (_, list) in self.events.lock().iter() {
            if let Some(download) = list.iter().find(|download| download.id == id)
                && states.last() != Some(&download.state)
            {
                states.push(download.state);
            }
        }
        states
    }

    fn count(&self) -> usize {
        self.events.lock().len()
    }
}

struct Setup {
    downloads: SpeechDownloads,
    recorder: Recorder,
    server: Server,
    root: tempfile::TempDir,
}

async fn setup(max_running: usize) -> Setup {
    setup_with(max_running, verify).await
}

/// Vérification lente (0,6 s), pour annuler pendant `Verifying`.
fn slow_verify(path: &Path, size: u64, sha256: &str) -> std::io::Result<bool> {
    std::thread::sleep(Duration::from_millis(600));
    verify(path, size, sha256)
}

async fn setup_with(
    max_running: usize,
    verify: fn(&Path, u64, &str) -> std::io::Result<bool>,
) -> Setup {
    let server = Server::start().await;
    let root = tempfile::tempdir().unwrap();
    let recorder = Recorder::default();
    let sink = recorder.events.clone();
    let downloads = SpeechDownloads::with_options(
        move |list| sink.lock().push((Instant::now(), list.to_vec())),
        root.path().to_path_buf(),
        Options {
            max_running,
            progress_interval: Duration::from_millis(100),
            base_url: Some(server.url.clone()),
            read_timeout: Duration::from_secs(5),
            test_env: "CHATONLM_SPEECH_MODELS_URL",
            find: find_item,
            verify,
        },
    );
    Setup {
        downloads,
        recorder,
        server,
        root,
    }
}

impl Setup {
    fn state(&self, id: &str) -> Option<SpeechDownload> {
        self.downloads
            .list()
            .into_iter()
            .find(|download| download.id == id)
    }

    async fn wait(&self, id: &str, condition: impl Fn(&SpeechDownload) -> bool) -> SpeechDownload {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(download) = self.state(id)
                && condition(&download)
            {
                return download;
            }
            assert!(Instant::now() < deadline, "{id}: {:?}", self.state(id));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn wait_state(&self, id: &str, state: SpeechDownloadState) -> SpeechDownload {
        self.wait(id, |download| download.state == state).await
    }

    fn file(&self, id: &str, name: &str) -> PathBuf {
        self.root.path().join(id).join(name)
    }
}

#[tokio::test]
async fn a_model_is_downloaded_file_by_file_and_verified() {
    let setup = setup(2).await;
    let installed = Arc::new(Mutex::new(Vec::new()));
    let sink = installed.clone();
    setup
        .downloads
        .on_installed(move |id| sink.lock().push(id.to_owned()));
    setup.downloads.download("two").unwrap();
    let done = setup.wait_state("two", Done).await;
    // Annoncé une fois, après la liste qui le dit fini.
    let deadline = Instant::now() + WAIT;
    while installed.lock().is_empty() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(*installed.lock(), ["two"]);
    assert_eq!(done.total_bytes, 150_000);
    assert_eq!(done.completed_bytes, 150_000);
    assert_eq!(done.error, None);
    for (name, size) in [("two-a.bin", 100_000), ("two-b.bin", 50_000)] {
        assert_eq!(
            std::fs::read(setup.file("two", name)).unwrap(),
            content(name, size)
        );
        assert!(!part_path(&setup.file("two", name)).exists());
    }
    assert!(models::installed(setup.root.path(), find("two").unwrap()));
    assert_eq!(
        setup.recorder.states("two"),
        [Downloading, Verifying, Downloading, Verifying, Done]
    );
    // Déjà là : rien n'est redemandé.
    let asked = setup.server.requests().len();
    setup.downloads.download("two").unwrap();
    setup.wait_state("two", Done).await;
    assert_eq!(setup.server.requests().len(), asked);
}

#[tokio::test]
async fn an_interrupted_download_resumes_where_it_stopped() {
    let setup = setup(2).await;
    setup.server.serve("two-a.bin", Serve::CutAfter(30_000));
    setup.downloads.download("two").unwrap();
    let failed = setup.wait_state("two", Failed).await;
    assert!(
        failed
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("Download interrupted")),
        "{failed:?}"
    );
    let part = part_path(&setup.file("two", "two-a.bin"));
    assert_eq!(std::fs::metadata(&part).unwrap().len(), 30_000);

    setup.server.serve("two-a.bin", Serve::Normal);
    setup.downloads.download("two").unwrap();
    setup.wait_state("two", Done).await;
    assert_eq!(
        std::fs::read(setup.file("two", "two-a.bin")).unwrap(),
        content("two-a.bin", 100_000)
    );
    let requests = setup.server.requests();
    assert!(
        requests.contains(&("two-a.bin".into(), Some("bytes=30000-".into()))),
        "{requests:?}"
    );
}

#[tokio::test]
async fn a_server_that_ignores_range_restarts_from_zero() {
    let setup = setup(2).await;
    setup.server.serve("two-a.bin", Serve::CutAfter(30_000));
    setup.downloads.download("two").unwrap();
    setup.wait_state("two", Failed).await;
    setup.server.serve("two-a.bin", Serve::IgnoreRange);
    setup.downloads.download("two").unwrap();
    setup.wait_state("two", Done).await;
    assert_eq!(
        std::fs::read(setup.file("two", "two-a.bin")).unwrap(),
        content("two-a.bin", 100_000)
    );
    // La suite a bien été demandée ; le fichier entier est arrivé.
    assert!(
        setup
            .server
            .requests()
            .contains(&("two-a.bin".into(), Some("bytes=30000-".into())))
    );
}

#[tokio::test]
async fn cancel_during_verification_keeps_the_part_unrenamed() {
    let setup = setup_with(2, slow_verify).await;
    setup.downloads.download("damaged").unwrap();
    setup.wait_state("damaged", Verifying).await;
    setup.downloads.cancel("damaged");
    assert_eq!(setup.state("damaged").unwrap().state, Cancelled);
    // La vérification finit dans son fil, sans suite.
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(setup.state("damaged").unwrap().state, Cancelled);
    let target = setup.file("damaged", "damaged.bin");
    assert!(!target.exists());
    assert!(part_path(&target).exists(), "kept for a later resume");
}

#[tokio::test]
async fn a_new_download_waits_for_the_cancelled_task_of_the_same_model() {
    let setup = setup(2).await;
    let model = find("two").unwrap();
    // Tâche annulée qui n'a pas encore fini (simulée).
    setup.downloads.inner.state.lock().busy.insert(model.id);
    setup.downloads.download("two").unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(setup.state("two").unwrap().state, Queued);
    assert!(setup.server.requests().is_empty());
    assert!(
        !setup
            .downloads
            .wait_idle("two", Duration::from_millis(50))
            .await
    );
    setup.downloads.inner.task_ended(model);
    setup.wait_state("two", Done).await;

    // Pour de vrai : annulé puis relancé aussitôt, le fichier est intact.
    setup.server.serve("slow.bin", Serve::Slow);
    setup.downloads.download("slow").unwrap();
    setup
        .wait("slow", |download| download.completed_bytes > 20_000)
        .await;
    setup.downloads.cancel("slow");
    setup.downloads.download("slow").unwrap();
    setup.wait_state("slow", Done).await;
    assert_eq!(
        std::fs::read(setup.file("slow", "slow.bin")).unwrap(),
        content("slow.bin", 400_000)
    );
}

#[tokio::test]
async fn dismissing_a_failed_or_cancelled_download_deletes_its_parts() {
    let setup = setup(2).await;
    setup.server.serve("two-a.bin", Serve::CutAfter(30_000));
    setup.downloads.download("two").unwrap();
    setup.wait_state("two", Failed).await;
    assert!(setup.downloads.wait_idle("two", WAIT).await);
    let part = part_path(&setup.file("two", "two-a.bin"));
    assert!(part.exists());
    setup.downloads.dismiss("two");
    assert!(!part.exists());
    // Dossier vide : supprimé aussi.
    assert!(!setup.root.path().join("two").exists());

    // Tâche encore en vie (simulée) : supprimés à sa fin seulement.
    let model = find("slow").unwrap();
    setup.server.serve("slow.bin", Serve::Slow);
    setup.downloads.download("slow").unwrap();
    setup
        .wait("slow", |download| download.completed_bytes > 20_000)
        .await;
    setup.downloads.cancel("slow");
    assert!(setup.downloads.wait_idle("slow", WAIT).await);
    let part = part_path(&setup.file("slow", "slow.bin"));
    assert!(part.exists());
    setup.downloads.inner.state.lock().busy.insert(model.id);
    setup.downloads.dismiss("slow");
    assert!(part.exists());
    setup.downloads.inner.task_ended(model);
    assert!(!part.exists());
}

#[tokio::test]
async fn a_damaged_file_is_deleted() {
    let setup = setup(2).await;
    let installed = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = installed.clone();
    setup
        .downloads
        .on_installed(move |id| sink.lock().push(id.to_owned()));
    setup.downloads.download("damaged").unwrap();
    let failed = setup.wait_state("damaged", Failed).await;
    // Échec : rien d'installé à annoncer.
    assert!(installed.lock().is_empty());
    assert_eq!(
        failed.error.as_deref(),
        Some("damaged.bin is damaged (checksum mismatch): download it again")
    );
    let target = setup.file("damaged", "damaged.bin");
    assert!(!target.exists() && !part_path(&target).exists());
}

#[tokio::test]
async fn http_errors_fail_the_download() {
    let setup = setup(2).await;
    setup.downloads.download("missing").unwrap();
    let failed = setup.wait_state("missing", Failed).await;
    assert_eq!(failed.error.as_deref(), Some("Download failed: HTTP 404"));
    assert!(matches!(
        setup.downloads.download("nope"),
        Err(AppError::Invalid(_))
    ));
}

#[tokio::test]
async fn cancel_keeps_the_part_file_and_dismiss_forgets() {
    let setup = setup(2).await;
    setup.server.serve("slow.bin", Serve::Slow);
    setup.downloads.download("slow").unwrap();
    setup
        .wait("slow", |download| download.completed_bytes > 20_000)
        .await;
    assert!(setup.downloads.is_active("slow"));
    setup.downloads.cancel("slow");
    assert_eq!(setup.state("slow").unwrap().state, Cancelled);
    assert!(!setup.downloads.is_active("slow"));
    let part = part_path(&setup.file("slow", "slow.bin"));
    let deadline = Instant::now() + WAIT;
    while std::fs::metadata(&part).map_or(0, |metadata| metadata.len()) == 0 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    setup.downloads.dismiss("slow");
    assert!(setup.state("slow").is_none());
}

#[tokio::test]
async fn the_queue_runs_in_order_within_the_limit() {
    let setup = setup(1).await;
    setup.server.serve("slow.bin", Serve::Slow);
    setup.downloads.download("slow").unwrap();
    setup.downloads.download("two").unwrap();
    setup.wait_state("slow", Downloading).await;
    assert_eq!(setup.state("two").unwrap().state, Queued);
    // Déjà en file : rien ne change.
    setup.downloads.download("two").unwrap();
    assert_eq!(setup.downloads.list().len(), 2);
    setup.downloads.cancel("slow");
    setup.wait_state("two", Done).await;
}

#[tokio::test]
async fn progress_is_emitted_at_most_ten_times_per_second_here() {
    let setup = setup(2).await;
    setup.server.serve("slow.bin", Serve::Slow);
    let started = Instant::now();
    setup.downloads.download("slow").unwrap();
    setup.wait_state("slow", Done).await;
    let elapsed = started.elapsed();
    // Environ 100 morceaux de 4 Kio en 1 s : une émission par 100 ms au plus,
    // plus les changements d'état.
    let limit = (elapsed.as_millis() / 100) as usize + 6;
    assert!(
        setup.recorder.count() <= limit,
        "{} emissions in {elapsed:?}",
        setup.recorder.count()
    );
    // La dernière progression n'est pas perdue.
    let last = setup.state("slow").unwrap();
    assert_eq!(last.completed_bytes, last.total_bytes);
}

#[tokio::test]
async fn stop_all_closes_the_queue() {
    let setup = setup(2).await;
    setup.server.serve("slow.bin", Serve::Slow);
    setup.downloads.download("slow").unwrap();
    setup.wait_state("slow", Downloading).await;
    setup.downloads.stop_all(Duration::from_secs(2)).await;
    assert_eq!(setup.state("slow").unwrap().state, Cancelled);
    setup.downloads.download("two").unwrap();
    assert!(setup.state("two").is_none());
}

#[test]
fn urls_point_at_the_pinned_revision() {
    let model = crate::dictation::models::find("whisper-tiny").unwrap();
    assert_eq!(
        file_url(DEFAULT_BASE_URL, model, &model.files[0]),
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-tiny.bin"
    );
}
