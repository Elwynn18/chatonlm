//! Essais du pack CUDA : environnement d'un lancement, journal, choix de CUDA,
//! installation depuis des archives fabriquées (Linux et Windows), et
//! parcours complet du gestionnaire contre un petit serveur HTTP local
//! (manifeste signé par une clé d'essai, `fixtures/`).

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::archive::tests::{tar_bytes, zip_bytes};
use super::detect::NvidiaGpu;
use super::manager::{CudaPhase, CudaStatus, Sources, install_from, retry_busy};
use super::pack::{self, Manifest};
use super::pins::{Pins, Redist};
use super::*;

/// Épingles du dépôt, build et version de llama.cpp fixées à celles des
/// fichiers d'essai (un llama.env plus récent ne les change pas).
pub fn fixture_pins(platform: Platform) -> Pins {
    let mut pins = pins::parse(
        include_str!("../../../../packaging/cuda.env"),
        include_str!("../../../../packaging/llama.env"),
        platform,
    )
    .unwrap();
    pins.llama_build = "b12110".into();
    pins.llama_version = "v0.9.0".into();
    pins.pack_version = FIXTURE_VERSION.into();
    pins
}

/// `VERSION` des packs d'essai (`fixtures/`, [`windows_pack`]).
pub const FIXTURE_VERSION: &str = "v0.9.0+chatonlm.cuda.0123456789ab";

const TEST_PUBKEY: &str = include_str!("fixtures/test.key.pub");
const PACK: &[u8] = include_bytes!("fixtures/chatonlm-cuda-b12110-linux-x64.tar.gz");
const MANIFEST: &[u8] = include_bytes!("fixtures/chatonlm-cuda-b12110-linux-x64.json");
const MANIFEST_SIG: &[u8] = include_bytes!("fixtures/chatonlm-cuda-b12110-linux-x64.json.sig");

fn rtx3090() -> NvidiaGpu {
    NvidiaGpu {
        name: "NVIDIA GeForce RTX 3090".into(),
        uuid: "GPU-3090".into(),
        compute: (8, 6),
        driver: "615.78.08".into(),
    }
}

fn runtime_for(platform: Platform, dir: &std::path::Path) -> CudaRuntime {
    CudaRuntime {
        platform,
        dir: dir.to_path_buf(),
        uuids: vec!["GPU-a".into(), "GPU-b".into()],
        jit_cache: dir.join("jit"),
    }
}

fn env_map(env: Vec<(OsString, OsString)>) -> HashMap<String, String> {
    env.into_iter()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect()
}

#[test]
fn a_cuda_launch_loads_the_pack_engine_and_hides_vulkan() {
    let dir = std::path::Path::new("/data/engine/cuda/b12110");
    let linux = env_map(runtime_env(
        &runtime_for(Platform::Linux, dir),
        Some("/usr/bin".as_ref()),
    ));
    // Chemins joints par le système de la machine (« \\ » sous Windows).
    let text = |path: std::path::PathBuf| path.to_string_lossy().into_owned();
    assert_eq!(linux[BACKEND_PATH_ENV], text(dir.join("libggml-cuda.so")));
    assert_eq!(linux["CUDA_VISIBLE_DEVICES"], "GPU-a,GPU-b");
    assert_eq!(linux["CUDA_DEVICE_ORDER"], "PCI_BUS_ID");
    assert_eq!(linux["CUDA_CACHE_PATH"], text(dir.join("jit")));
    assert_eq!(linux["GGML_VK_VISIBLE_DEVICES"], "none");
    // Linux : RUNPATH du moteur, le PATH n'est pas touché.
    assert!(!linux.contains_key("PATH"));
    let windows = env_map(runtime_env(
        &runtime_for(Platform::Windows, dir),
        Some(r"C:\Windows\system32".as_ref()),
    ));
    assert!(windows[BACKEND_PATH_ENV].ends_with("ggml-cuda.dll"));
    // Windows : dossier du pack en tête du PATH (DLL de NVIDIA).
    assert_eq!(
        windows["PATH"],
        r"/data/engine/cuda/b12110;C:\Windows\system32"
    );
    assert_eq!(
        env_map(runtime_env(&runtime_for(Platform::Windows, dir), None))["PATH"],
        "/data/engine/cuda/b12110"
    );
    let args: Vec<String> = device_args(&runtime_for(Platform::Linux, dir))
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args, ["--device", "CUDA0,CUDA1"]);
}

#[test]
fn the_journal_accuses_cuda() {
    for log in [
        "load_backend: failed to load /d/engine/cuda/b1/libggml-cuda.so: libcuda.so.1: cannot open shared object file",
        "error: invalid device: CUDA0",
        "CUDA error: the provided PTX was compiled with an unsupported toolchain.",
        "ggml_cuda_init: failed to initialize CUDA: no CUDA-capable device is detected",
        "cuBLAS error CUBLAS_STATUS_NOT_INITIALIZED",
        "CUDA error: no kernel image is available for execution on the device",
        "CUDA error: CUDA driver version is insufficient for CUDA runtime version",
    ] {
        assert!(journal_accuses_cuda(log), "{log}");
        assert!(!journal_cuda_out_of_memory(log), "{log}");
    }
    // Mémoire et plantage du modèle : pas la faute du pack.
    for log in [
        "CUDA error: out of memory",
        "ggml_backend_cuda_buffer_type_alloc_buffer: allocating 3584.00 MiB on device 0: cudaMalloc failed: out of memory",
        "cuBLAS error CUBLAS_STATUS_ALLOC_FAILED",
    ] {
        assert!(!journal_accuses_cuda(log), "{log}");
        assert!(journal_cuda_out_of_memory(log), "{log}");
    }
    for log in [
        "ggml_cuda_host_malloc: failed to allocate 512.00 MiB of pinned memory: out of memory",
        "/src/ggml/src/ggml-cuda/ggml-cuda.cu:100: GGML_ASSERT(ne00 % 2 == 0) failed",
        "warning: ggml_cuda: failed to enable peer access",
    ] {
        assert!(!journal_accuses_cuda(log), "{log}");
        assert!(!journal_cuda_out_of_memory(log), "{log}");
    }
    for log in [
        "ggml_cuda_init: found 1 CUDA devices:",
        "load_tensors: offloaded 29/29 layers to GPU",
        "llama_model_load: error loading model: tensor 'x' has wrong shape",
        "ggml_vulkan: device lost",
    ] {
        assert!(!journal_accuses_cuda(log), "{log}");
    }
}

/// Pack installé factice (fichiers complets) sous `root`.
fn install_fake(root: &std::path::Path, platform: Platform) {
    let pins = fixture_pins(platform);
    let dir = pack::build_dir(root, &pins.llama_build);
    std::fs::create_dir_all(&dir).unwrap();
    for file in pack::installed_files(platform, pins.cuda_major) {
        std::fs::write(dir.join(file), b"x").unwrap();
    }
    std::fs::write(dir.join("VERSION"), FIXTURE_VERSION).unwrap();
}

#[tokio::test]
async fn cuda_is_chosen_only_with_a_pack_a_card_the_setting_and_no_failure() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("cuda");
    // Sans pack : jamais (et sans détection).
    let runtime = Runtime::fixed(root.clone(), Platform::Linux, vec![rtx3090()]);
    assert_eq!(runtime.select().await, None);
    install_fake(&root, Platform::Linux);
    let chosen = runtime.select().await.unwrap();
    assert_eq!(chosen.dir, root.join("b12110"));
    assert_eq!(chosen.uuids, ["GPU-3090"]);
    assert_eq!(chosen.jit_cache, root.join("jit-cache"));
    // Réglage coupé, puis remis.
    runtime.set_enabled(false).unwrap();
    assert!(!runtime.enabled());
    assert_eq!(runtime.select().await, None);
    runtime.set_enabled(true).unwrap();
    runtime.set_enabled(true).unwrap();
    assert!(runtime.select().await.is_some());
    // Échec de la session.
    runtime.fail("CUDA error".into());
    assert_eq!(runtime.select().await, None);
    runtime.clear_failure();
    assert!(runtime.select().await.is_some());
    // Carte trop ancienne, ou aucune : jamais.
    let pascal = NvidiaGpu {
        compute: (6, 1),
        ..rtx3090()
    };
    assert_eq!(
        Runtime::fixed(root.clone(), Platform::Linux, vec![pascal])
            .select()
            .await,
        None
    );
    assert_eq!(
        Runtime::fixed(root.clone(), Platform::Linux, Vec::new())
            .select()
            .await,
        None
    );
    // Pack incomplet (bibliothèque de NVIDIA manquante) : jamais.
    std::fs::remove_file(root.join("b12110").join("libcublas.so.13")).unwrap();
    assert_eq!(runtime.select().await, None);
}

/// Pack de la même build mais d'une autre `VERSION` (refait : correctif,
/// épingles) : pas choisi, montré périmé (à retélécharger).
#[tokio::test]
async fn a_pack_of_another_version_is_stale_and_not_chosen() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("cuda");
    install_fake(&root, Platform::Linux);
    std::fs::write(
        root.join("b12110").join("VERSION"),
        "v0.9.0+chatonlm.cuda.ffffffffffff\n",
    )
    .unwrap();
    let runtime = Runtime::fixed(root.clone(), Platform::Linux, vec![rtx3090()]);
    assert_eq!(runtime.select().await, None);
    let cuda = Cuda::with_sources(runtime, "9.9.9", |_| {}, Sources::default());
    let status = cuda.status(false).await;
    assert_eq!(status.phase, CudaPhase::Absent);
    assert!(status.stale);
    assert!(!status.active);
}

/// Archive de NVIDIA fabriquée pour `platform` et ses épingles.
fn nvidia_archive(platform: Platform, files: &[(&str, &[u8])]) -> (Vec<u8>, &'static str) {
    match platform {
        Platform::Linux => {
            let tar = tar_bytes(
                &files
                    .iter()
                    .map(|(path, content)| (*path, Some(*content)))
                    .collect::<Vec<_>>(),
            );
            let mut xz = Vec::new();
            {
                let mut writer =
                    lzma_rust2::XzWriter::new(&mut xz, lzma_rust2::XzOptions::with_preset(1))
                        .unwrap();
                writer.write_all(&tar).unwrap();
                writer.finish().unwrap();
            }
            (xz, ".tar.xz")
        }
        Platform::Windows => (zip_bytes(files), ".zip"),
    }
}

fn redist_for(name: &str, bytes: &[u8]) -> Redist {
    Redist {
        path: format!("sub/{name}"),
        size: bytes.len() as u64,
        sha256: sha256(bytes),
    }
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Archives de NVIDIA fabriquées (noms de fichier, contenus) et épingles
/// qui les désignent.
fn fake_nvidia(platform: Platform) -> (Pins, Vec<(String, Vec<u8>)>) {
    let (cudart, cublas) = match platform {
        Platform::Linux => (
            nvidia_archive(
                platform,
                &[
                    ("cuda_cudart-x/lib/libcudart.so.13.4.49", b"cudart"),
                    ("cuda_cudart-x/lib/libcudart_static.a", b"static"),
                    ("cuda_cudart-x/LICENSE", b"NVIDIA EULA"),
                ],
            ),
            nvidia_archive(
                platform,
                &[
                    ("libcublas-x/lib/libcublas.so.13.7.0.27", b"cublas"),
                    ("libcublas-x/lib/libcublasLt.so.13.7.0.27", b"cublasLt"),
                    ("libcublas-x/LICENSE", b"other"),
                ],
            ),
        ),
        Platform::Windows => (
            nvidia_archive(
                platform,
                &[
                    ("cuda_cudart-x/bin/x64/cudart64_13.dll", b"cudart"),
                    ("cuda_cudart-x/LICENSE", b"NVIDIA EULA"),
                ],
            ),
            nvidia_archive(
                platform,
                &[
                    ("libcublas-x/bin/x64/cublas64_13.dll", b"cublas"),
                    ("libcublas-x/bin/x64/cublasLt64_13.dll", b"cublasLt"),
                ],
            ),
        ),
    };
    let names = (
        format!("cuda_cudart-archive{}", cudart.1),
        format!("libcublas-archive{}", cublas.1),
    );
    let mut pins = fixture_pins(platform);
    pins.cudart = redist_for(&names.0, &cudart.0);
    pins.cublas = redist_for(&names.1, &cublas.0);
    (pins, vec![(names.0, cudart.0), (names.1, cublas.0)])
}

/// Pack de Windows fabriqué (le pack de Linux est celui de `fixtures/`) ;
/// `SHA256SUMS` en mode binaire (« * »), comme le `sha256sum` de Git pour
/// Windows l'écrivait avant `--text`.
fn windows_pack() -> (Vec<u8>, Manifest) {
    let engine: &[u8] = b"fake ggml-cuda.dll";
    let version: &[u8] = b"v0.9.0+chatonlm.cuda.0123456789ab\n";
    let sums = format!(
        "{} *ggml-cuda.dll\n{} *VERSION\n{} *LICENSE-llama.cpp.txt\n{} *LICENSE-CCCL.txt\n",
        sha256(engine),
        sha256(version),
        sha256(b"mit"),
        sha256(b"apache")
    );
    let bytes = zip_bytes(&[
        ("ggml-cuda.dll", engine),
        ("VERSION", version),
        ("LICENSE-llama.cpp.txt", b"mit"),
        ("LICENSE-CCCL.txt", b"apache"),
        ("SHA256SUMS", sums.as_bytes()),
    ]);
    let manifest = Manifest {
        build: "b12110".into(),
        version: "v0.9.0+chatonlm.cuda.0123456789ab".into(),
        file: pack::pack_name(Platform::Windows, "b12110"),
        size: bytes.len() as u64,
        sha256: sha256(&bytes),
    };
    (bytes, manifest)
}

#[test]
fn archives_are_assembled_into_the_build_folder() {
    for platform in [Platform::Linux, Platform::Windows] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let downloads = root.join("downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        let (pins, nvidia) = fake_nvidia(platform);
        for (name, bytes) in &nvidia {
            std::fs::write(downloads.join(name), bytes).unwrap();
        }
        let (pack_bytes, manifest) = match platform {
            Platform::Linux => (
                PACK.to_vec(),
                pack::parse_manifest(MANIFEST, platform, &pins).unwrap(),
            ),
            Platform::Windows => windows_pack(),
        };
        std::fs::write(downloads.join(&manifest.file), &pack_bytes).unwrap();
        // Ancienne build : retirée après l'installation.
        std::fs::create_dir_all(root.join("b11000")).unwrap();
        let cancel = AtomicBool::new(false);
        install_from(root, platform, &pins, &manifest, &cancel).unwrap();
        let installed = pack::installed(root, &pins, platform).unwrap();
        assert_eq!(installed.version, manifest.version);
        let read = |name: &str| std::fs::read(installed.dir.join(name)).unwrap();
        let (cudart, cublas_lt) = match platform {
            Platform::Linux => ("libcudart.so.13", "libcublasLt.so.13"),
            Platform::Windows => ("cudart64_13.dll", "cublasLt64_13.dll"),
        };
        assert_eq!(read(cudart), b"cudart");
        assert_eq!(read(cublas_lt), b"cublasLt");
        // La licence de NVIDIA est celle de cudart, rangée à côté.
        assert_eq!(read(archive::NVIDIA_LICENSE), b"NVIDIA EULA");
        // Rien d'autre que les fichiers attendus (pas de statique).
        let mut names: Vec<String> = std::fs::read_dir(&installed.dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let mut expected = pack::installed_files(platform, 13);
        expected.sort();
        assert_eq!(names, expected);
        assert!(!downloads.exists());
        assert!(!root.join("b11000").exists());
        assert!(!root.join("b12110.part").exists());
    }
}

#[test]
fn a_pack_that_does_not_match_its_manifest_is_not_installed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let downloads = root.join("downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    let (pins, nvidia) = fake_nvidia(Platform::Linux);
    for (name, bytes) in &nvidia {
        std::fs::write(downloads.join(name), bytes).unwrap();
    }
    let mut manifest = pack::parse_manifest(MANIFEST, Platform::Linux, &pins).unwrap();
    std::fs::write(downloads.join(&manifest.file), PACK).unwrap();
    // VERSION du pack différente de celle du manifeste.
    manifest.version = "v0.9.0+chatonlm.cuda.ffffffffffff".into();
    let error = install_from(
        root,
        Platform::Linux,
        &pins,
        &manifest,
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(error.contains("VERSION"), "{error}");
    assert!(pack::installed(root, &pins, Platform::Linux).is_none());
    assert!(!root.join("b12110.part").exists());
    // Archive de NVIDIA sans cuBLAS.
    let manifest = pack::parse_manifest(MANIFEST, Platform::Linux, &pins).unwrap();
    let (empty, _) = nvidia_archive(Platform::Linux, &[("x/README", b"none")]);
    std::fs::write(downloads.join(pins.cublas.file_name()), empty).unwrap();
    let error = install_from(
        root,
        Platform::Linux,
        &pins,
        &manifest,
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(error.contains("missing"), "{error}");
    assert!(pack::installed(root, &pins, Platform::Linux).is_none());
}

/// Serveur HTTP de fichiers (chemin → contenu), `Range` compris ; compte
/// les requêtes.
async fn serve(files: HashMap<String, Vec<u8>>) -> (String, Arc<parking_lot::Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let files = Arc::new(files);
    let requests = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let seen = requests.clone();
    tauri::async_runtime::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let files = files.clone();
            let seen = seen.clone();
            tauri::async_runtime::spawn(async move {
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
                seen.lock().push(path.clone());
                let start: usize = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("range")
                            .then(|| value.trim().to_owned())
                    })
                    .and_then(|range| {
                        range
                            .strip_prefix("bytes=")?
                            .trim_end_matches('-')
                            .parse()
                            .ok()
                    })
                    .unwrap_or(0);
                let Some(body) = files.get(&path) else {
                    let _ = stream
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                    return;
                };
                let head = if start > 0 {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{}\r\nConnection: close\r\n\r\n",
                        body.len() - start,
                        body.len() - 1,
                        body.len()
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                };
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(&body[start..]).await;
            });
        }
    });
    (url, requests)
}

/// Dernier état émis, dès qu'il convient (borne de 60 s). L'installation
/// extrait et synchronise (`sync_all`) chaque fichier du paquet dans TMPDIR ;
/// sous `check.sh`, TMPDIR est sur le disque du projet (ntfs3, ~200 ms par
/// fichier créé et synchronisé sur cette machine, bien plus avec seize
/// essais à la fois) : l'ancienne borne de 10 s y a été dépassée (phase
/// `Installing` encore affichée).
async fn wait_for(
    statuses: &Arc<parking_lot::Mutex<Vec<CudaStatus>>>,
    ready: impl Fn(&CudaStatus) -> bool,
) -> CudaStatus {
    for _ in 0..3000 {
        if let Some(status) = statuses.lock().last().filter(|status| ready(status)) {
            return status.clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("état attendu jamais vu : {:?}", statuses.lock().last());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_manager_downloads_verifies_and_installs_the_pack() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("cuda");
    let (pins, nvidia) = fake_nvidia(Platform::Linux);
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    files.insert(
        "/release/chatonlm-cuda-b12110-linux-x64.json".into(),
        MANIFEST.to_vec(),
    );
    files.insert(
        "/release/chatonlm-cuda-b12110-linux-x64.json.sig".into(),
        MANIFEST_SIG.to_vec(),
    );
    files.insert(
        "/release/chatonlm-cuda-b12110-linux-x64.tar.gz".into(),
        PACK.to_vec(),
    );
    for (name, bytes) in &nvidia {
        files.insert(format!("/nvidia/sub/{name}"), bytes.clone());
    }
    let (url, requests) = serve(files).await;
    let statuses = Arc::new(parking_lot::Mutex::new(Vec::<CudaStatus>::new()));
    let seen = statuses.clone();
    let runtime = Runtime::fixed_with(root.clone(), Platform::Linux, vec![rtx3090()], pins);
    let sources = Sources {
        release: Some(format!("{url}/release")),
        nvidia: Some(format!("{url}/nvidia/")),
        pubkey: Some(TEST_PUBKEY.into()),
    };
    let cuda = Cuda::with_sources(
        runtime.clone(),
        "9.9.9",
        move |status| seen.lock().push(status.clone()),
        sources.clone(),
    );
    let first = cuda.status(false).await;
    assert_eq!(first.phase, CudaPhase::Absent);
    assert_eq!(first.compat, detect::Compat::Compatible);
    assert_eq!(first.gpu_name.as_deref(), Some("NVIDIA GeForce RTX 3090"));
    assert!(!first.active);
    // Licence non acceptée : refus, rien de téléchargé.
    assert!(cuda.install(false).await.is_err());
    assert!(requests.lock().is_empty());
    cuda.install(true).await.unwrap();
    let done = wait_for(&statuses, |status| status.phase == CudaPhase::Installed).await;
    assert!(done.active && done.enabled);
    assert_eq!(done.error, None);
    assert!(done.installed_bytes.unwrap() > 0);
    assert!(runtime.select().await.is_some());
    // Manifeste et signature, puis les trois archives.
    let asked = requests.lock().clone();
    assert!(asked.iter().any(|path| path.ends_with(".json.sig")));
    assert!(asked.iter().any(|path| path.ends_with("linux-x64.tar.gz")));
    assert_eq!(
        asked
            .iter()
            .filter(|path| path.starts_with("/nvidia/sub/"))
            .count(),
        2
    );
    // Coupé, puis retiré.
    cuda.set_enabled(false).unwrap();
    assert!(!wait_for(&statuses, |status| !status.enabled).await.active);
    cuda.remove().await.unwrap();
    let removed = wait_for(&statuses, |status| status.phase == CudaPhase::Absent).await;
    assert!(removed.enabled, "le réglage repart à zéro");
    assert!(!root.exists());

    // Une signature d'une autre version de l'app : refusée, rien installé.
    let statuses = Arc::new(parking_lot::Mutex::new(Vec::<CudaStatus>::new()));
    let seen = statuses.clone();
    let other = Cuda::with_sources(
        runtime.clone(),
        "9.9.10",
        move |status| seen.lock().push(status.clone()),
        sources,
    );
    other.status(false).await;
    other.install(true).await.unwrap();
    let failed = wait_for(&statuses, |status| status.error.is_some()).await;
    assert_eq!(failed.phase, CudaPhase::Absent);
    assert!(failed.error.unwrap().contains("another version"));
    assert!(!root.join("downloads").exists() || runtime.installed().is_none());
}

#[tokio::test]
async fn an_incompatible_machine_cannot_install() {
    let dir = tempfile::tempdir().unwrap();
    let old = NvidiaGpu {
        driver: "575.1".into(),
        ..rtx3090()
    };
    let cuda = Cuda::with_sources(
        Runtime::fixed(dir.path().join("cuda"), Platform::Linux, vec![old]),
        "9.9.9",
        |_| {},
        Sources::default(),
    );
    let status = cuda.status(true).await;
    assert_eq!(status.compat, detect::Compat::DriverTooOld);
    assert_eq!(status.driver_min, 580);
    assert_eq!(status.compute_min, "7.5");
    assert!(cuda.install(true).await.is_err());
}

/// Débit d'une génération de `id` (jetons par seconde de llama-server,
/// `timings.predicted_per_second`, meilleur de trois après un tour de
/// chauffe) et journal de son chargement.
async fn throughput(engine: &crate::engine::Engine, id: &str) -> (f64, String) {
    let lease = engine
        .ensure(id, crate::engine::LoadOptions::default())
        .await
        .expect("modèle chargé");
    let client = reqwest::Client::new();
    let mut best: f64 = 0.0;
    for round in 0..4 {
        let reply: serde_json::Value = client
            .post(format!("{}/completion", lease.endpoint.base_url))
            .bearer_auth(&lease.endpoint.api_key)
            .json(&serde_json::json!({
                "prompt": "Write a long story about a ginger kitten who learns to code.",
                "n_predict": 256,
                "temperature": 0,
                "ignore_eos": true,
                "cache_prompt": false,
            }))
            .send()
            .await
            .expect("requête")
            .json()
            .await
            .expect("réponse JSON");
        let rate = reply["timings"]["predicted_per_second"]
            .as_f64()
            .expect("timings");
        if round > 0 {
            best = best.max(rate);
        }
    }
    drop(lease);
    let log = crate::engine::journal::read(
        &engine
            .logs_dir()
            .join(crate::engine::journal::file_name(id)),
    );
    (best, log)
}

/// Preuve réelle (§ 1.7 du contrat 103) : pack de la CI servi localement
/// (manifeste signé par une clé d'essai), bibliothèques prises sur le VRAI
/// serveur de NVIDIA par le code de l'app (taille et SHA-256 épinglés),
/// vrai `llama-server`, vrai modèle sur la carte NVIDIA par CUDA, puis le
/// même par Vulkan (réglage coupé) ; débits écrits sur la sortie.
/// Variables : `CHATONLM_LLAMA_TEST_BIN`, `CHATONLM_LLAMA_TEST_MODELS` (banc
/// réel) ; `CHATONLM_CUDA_PROOF_DIR` (archive du pack, son manifeste, sa
/// signature, `test.key.pub`) ; `CHATONLM_CUDA_PROOF_VERSION` (version
/// signée). Jamais dans `check.sh` (pas de « contract » dans le nom).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "vraie carte NVIDIA, vrai llama-server, ~0,9 Go chez NVIDIA : à la main, sous verrou"]
async fn real_cuda_pack_runs_a_model_on_the_nvidia_card() {
    let Some(proof) = std::env::var_os("CHATONLM_CUDA_PROOF_DIR").map(std::path::PathBuf::from)
    else {
        eprintln!("CHATONLM_CUDA_PROOF_DIR missing, test skipped");
        return;
    };
    let version = std::env::var("CHATONLM_CUDA_PROOF_VERSION").unwrap_or_else(|_| "9.9.9".into());
    let Some(bench) = crate::llm::testing::RealBench::new(crate::llm::testing::Role::Text) else {
        return;
    };
    let engine = bench.access.engine().clone();
    let runtime = engine.cuda().clone();
    let mut files = HashMap::new();
    for entry in std::fs::read_dir(&proof).unwrap() {
        let entry = entry.unwrap();
        files.insert(
            format!("/release/{}", entry.file_name().to_string_lossy()),
            std::fs::read(entry.path()).unwrap(),
        );
    }
    let (url, _) = serve(files).await;
    let cuda = Cuda::with_sources(
        runtime.clone(),
        &version,
        |_| {},
        Sources {
            release: Some(format!("{url}/release")),
            nvidia: None,
            pubkey: Some(std::fs::read_to_string(proof.join("test.key.pub")).unwrap()),
        },
    );
    let status = cuda.status(true).await;
    eprintln!(
        "card: {:?}, driver {:?}, compute {:?}: {:?}",
        status.gpu_name, status.driver, status.compute, status.compat
    );
    assert_eq!(status.compat, detect::Compat::Compatible);
    let started = std::time::Instant::now();
    cuda.install(true).await.unwrap();
    loop {
        let status = cuda.status(false).await;
        if let Some(error) = status.error {
            panic!("installation : {error}");
        }
        if status.phase == CudaPhase::Installed {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(1800),
            "installation trop longue"
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    eprintln!(
        "pack installed in {:.0} s ({} bytes)",
        started.elapsed().as_secs_f64(),
        runtime.installed().unwrap().bytes
    );
    assert!(runtime.select().await.is_some());

    let (on_cuda, log) = throughput(&engine, &bench.id).await;
    assert!(log.contains("CUDA0"), "CUDA absent du journal :\n{log}");
    assert_eq!(runtime.failure(), None);
    engine.unload_all().await;
    runtime.set_enabled(false).unwrap();
    let (on_vulkan, log) = throughput(&engine, &bench.id).await;
    assert!(log.contains("Vulkan0"), "Vulkan absent du journal :\n{log}");
    assert!(!log.contains("CUDA0"));
    eprintln!("throughput: CUDA {on_cuda:.1} tok/s, Vulkan {on_vulkan:.1} tok/s");
    bench.shutdown().await;
}

/// Windows : un renommage ou un effacement refusé un instant (antivirus sur
/// une DLL tout juste écrite) est refait ; sous Linux, jamais ; une autre
/// erreur n'est pas refaite ; au bout des essais, l'erreur reste.
#[test]
fn a_busy_file_is_tried_again_on_windows_only() {
    // `busy` refus de `error`, puis la réussite : (réussi ?, appels).
    let failing = |busy: u32, error: fn() -> std::io::Error| {
        move |windows: bool| {
            let calls = std::cell::Cell::new(0);
            let outcome = retry_busy(windows, 10, Duration::ZERO, || {
                calls.set(calls.get() + 1);
                if calls.get() <= busy {
                    Err(error())
                } else {
                    Ok(())
                }
            });
            (outcome.is_ok(), calls.get())
        }
    };
    let denied = || std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    let sharing = || std::io::Error::from_raw_os_error(32);
    let missing = || std::io::Error::from(std::io::ErrorKind::NotFound);
    // Deux refus puis la réussite : trois appels.
    assert_eq!(failing(2, denied)(true), (true, 3));
    assert_eq!(failing(2, sharing)(true), (true, 3));
    // Linux : pas de nouvel essai.
    assert_eq!(failing(2, denied)(false), (false, 1));
    // Autre erreur : pas de nouvel essai.
    assert_eq!(failing(2, missing)(true), (false, 1));
    // Toujours refusé : dix appels, puis l'erreur.
    assert_eq!(failing(99, denied)(true), (false, 10));
}

/// Retrait pendant une installation : il attend la fin de l'assemblage, et
/// aucun lancement ne prend CUDA pendant ce temps ; ensuite tout est parti
/// et un nouveau pack est de nouveau choisi.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removal_waits_for_the_installation_and_blocks_cuda() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("cuda");
    install_fake(&root, Platform::Linux);
    let runtime = Runtime::fixed(root.clone(), Platform::Linux, vec![rtx3090()]);
    let cuda = Cuda::with_sources(runtime.clone(), "9.9.9", |_| {}, Sources::default());
    assert!(runtime.select().await.is_some());
    cuda.hold_install();
    let removing = {
        let cuda = cuda.clone();
        tokio::spawn(async move { cuda.remove().await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!removing.is_finished(), "removed during the installation");
    assert!(root.join("b12110").exists());
    assert_eq!(runtime.select().await, None);
    cuda.release_install();
    removing.await.unwrap().unwrap();
    assert!(!root.exists());
    install_fake(&root, Platform::Linux);
    assert!(runtime.select().await.is_some());
}
