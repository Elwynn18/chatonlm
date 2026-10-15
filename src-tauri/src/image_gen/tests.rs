//! Essais du moteur d'images contre le faux `sd-server`
//! (`fake_ollama::sdcpp`) : ce binaire d'essai lui-même, relancé sur le seul
//! essai [`fake_sd_server`] (arguments du serveur dans
//! `fake_ollama::sdcpp::ARGS_ENV`). Délais raccourcis.
//!
//! Essais contre le vrai `sd-server` (processeur seul, petites images) :
//! module `real`, ignorés sauf demande explicite (voir son en-tête).

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;

/// Faux `sd-server` : ignoré, sauf relancé par un essai.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "faux serveur lancé par les autres essais"]
async fn fake_sd_server() {
    if let Some(args) = fake_ollama::sdcpp::args_from_env() {
        std::process::exit(fake_ollama::sdcpp::run(args).await);
    }
}

fn temp_dir() -> tempfile::TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tools/tmp");
    std::fs::create_dir_all(&base).unwrap();
    // Chemin canonique : comparé à ceux de `/proc` (sans `..`).
    tempfile::tempdir_in(std::fs::canonicalize(base).unwrap()).unwrap()
}

/// Moteur joué par le faux, journal sous `dir`.
fn fake_config(dir: &Path) -> Config {
    let mut config = Config::new(Some(std::env::current_exe().unwrap()), dir);
    config.prefix_args = [
        "image_gen::tests::fake_sd_server",
        "--exact",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]
    .map(OsString::from)
    .to_vec();
    config.args_in_env = Some(fake_ollama::sdcpp::ARGS_ENV.into());
    config.timing = Timing {
        ready_timeout: Duration::from_secs(20),
        ready_poll: Duration::from_millis(50),
        poll: Duration::from_millis(20),
        lost_after: Duration::from_secs(2),
        idle_stop: Duration::from_secs(600),
        stop_grace: Duration::from_millis(1000),
        kill_wait: Duration::from_millis(1000),
    };
    config
}

/// Modèle à deux fichiers (créés) sous `dir`, de nom `name`.
fn spec(dir: &Path, name: &str) -> ModelSpec {
    let model = dir.join(format!("{name}.gguf"));
    let vae = dir.join("vae.safetensors");
    std::fs::write(&model, b"weights").unwrap();
    std::fs::write(&vae, b"vae").unwrap();
    ModelSpec {
        files: vec![(Component::DiffusionModel, model), (Component::Vae, vae)],
        options: vec!["--vae-tiling".into()],
    }
}

fn request(prompt: &str) -> Request {
    Request {
        prompt: prompt.into(),
        negative_prompt: String::new(),
        width: 64,
        height: 96,
        seed: Some(7),
        sampling: Sampling {
            steps: 4,
            cfg_scale: 1.0,
            sample_method: None,
            scheduler: None,
        },
        preview: true,
    }
}

/// Avancement noté.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<Progress>>>);

impl Seen {
    fn sink(&self) -> impl FnMut(Progress) + Send + use<> {
        let seen = self.0.clone();
        move |progress| seen.lock().unwrap().push(progress)
    }

    fn all(&self) -> Vec<Progress> {
        self.0.lock().unwrap().clone()
    }
}

async fn generate(
    engine: &ImageGen,
    spec: &ModelSpec,
    prompt: &str,
) -> Result<Image, ImageGenError> {
    engine.generate(spec, &request(prompt), &mut |_| {}).await
}

/// Le processus `pid` vit encore (ni absent ni zombie).
fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            !stat
                .rsplit(')')
                .next()
                .unwrap_or("")
                .trim_start()
                .starts_with('Z')
        })
    }
    #[cfg(windows)]
    {
        crate::ollama::builtin::process::alive(pid)
    }
}

/// Attend la fin du processus `pid`, `limit` au plus.
async fn gone_within(pid: u32, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if !alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    !alive(pid)
}

async fn get_json(url: &str) -> serde_json::Value {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(url)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// Adresse du serveur lancé (lue dans ses arguments : port après
/// `--listen-port`).
async fn server_url(engine: &ImageGen) -> String {
    let slot = engine.inner.server.lock().await;
    slot.as_ref().unwrap().url().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_image_is_made_with_every_setting_and_its_previews() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "klein");
    let seen = Seen::default();
    let image = engine
        .generate(&spec, &request("a ginger kitten"), &mut seen.sink())
        .await
        .unwrap();
    assert_eq!((image.width, image.height, image.seed), (64, 96, 7));
    assert_eq!(api::png_size(&image.png), Some((64, 96)));
    assert_eq!(
        image.png,
        fake_ollama::sdcpp::png(64, 96, fake_ollama::sdcpp::color(7))
    );

    let progress = seen.all();
    assert_eq!(progress.first(), Some(&Progress::Starting));
    let steps: Vec<(u32, u32)> = progress
        .iter()
        .filter_map(|progress| match progress {
            Progress::Generating {
                step: Some(step),
                preview: Some(png),
            } => {
                assert_eq!(api::png_size(png), Some((8, 8)));
                Some(*step)
            }
            _ => None,
        })
        .collect();
    assert!(!steps.is_empty(), "{progress:?}");
    assert!(
        steps
            .iter()
            .all(|&(step, total)| total == 4 && (1..=4).contains(&step))
    );
    // Un aperçu n'est annoncé qu'une fois.
    let mut unique = steps.clone();
    unique.dedup();
    assert_eq!(unique, steps);

    let url = server_url(&engine).await;
    let args = get_json(&format!("{url}/fake/args")).await;
    let args: Vec<&str> = args
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    assert_eq!(&args[..3], ["--listen-ip", "127.0.0.1", "--listen-port"]);
    assert_eq!(url, format!("http://127.0.0.1:{}", args[3]));
    let model = spec.files[0].1.to_string_lossy().into_owned();
    let vae = spec.files[1].1.to_string_lossy().into_owned();
    assert_eq!(
        &args[4..],
        ["--diffusion-model", &model, "--vae", &vae, "--vae-tiling"]
    );
    let requests = get_json(&format!("{url}/fake/requests")).await;
    let body = &requests[0];
    assert_eq!(body["prompt"], "a ginger kitten");
    assert_eq!(body["seed"], 7);
    assert_eq!(body["preview"], "proj");
    assert_eq!(body["sample_params"]["sample_steps"], 4);
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_random_seed_is_drawn_and_given_back() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "klein");
    let mut request = request("a ginger kitten");
    request.seed = None;
    request.preview = false;
    let seen = Seen::default();
    let image = engine
        .generate(&spec, &request, &mut seen.sink())
        .await
        .unwrap();
    assert!(image.seed <= i32::MAX as u32);
    assert_eq!(
        image.png,
        fake_ollama::sdcpp::png(64, 96, fake_ollama::sdcpp::color(i64::from(image.seed)))
    );
    // Sans aperçu demandé, aucun n'est annoncé.
    assert!(seen.all().iter().all(|progress| !matches!(
        progress,
        Progress::Generating {
            preview: Some(_),
            ..
        }
    )));
    let url = server_url(&engine).await;
    let requests = get_json(&format!("{url}/fake/requests")).await;
    assert_eq!(requests[0]["preview"], "none");
    assert_eq!(requests[0]["seed"], image.seed);
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_is_kept_for_the_same_model_and_restarted_for_another() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let klein = spec(dir.path(), "klein");
    generate(&engine, &klein, "one").await.unwrap();
    let first = engine.server_pid().unwrap();
    let seen = Seen::default();
    engine
        .generate(&klein, &request("two"), &mut seen.sink())
        .await
        .unwrap();
    assert_eq!(engine.server_pid(), Some(first));
    assert!(!seen.all().contains(&Progress::Starting));

    let zimage = spec(dir.path(), "zimage");
    generate(&engine, &zimage, "three").await.unwrap();
    let second = engine.server_pid().unwrap();
    assert_ne!(second, first);
    assert!(gone_within(first, Duration::from_secs(5)).await);
    engine.shutdown().await;
    assert!(gone_within(second, Duration::from_secs(5)).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_requests_are_refused_before_anything_starts() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "klein");
    let check = |change: fn(&mut Request)| {
        let mut request = request("a ginger kitten");
        change(&mut request);
        validate(&request)
    };
    assert!(matches!(
        check(|r| r.prompt = "  \n".into()),
        Err(ImageGenError::Invalid(_))
    ));
    assert!(matches!(
        check(|r| r.prompt = "x".repeat(MAX_PROMPT + 1)),
        Err(ImageGenError::Invalid(_))
    ));
    assert!(matches!(
        check(|r| r.width = 48),
        Err(ImageGenError::Invalid(_))
    ));
    assert!(matches!(
        check(|r| r.height = 2064),
        Err(ImageGenError::Invalid(_))
    ));
    assert!(matches!(
        check(|r| r.width = 520),
        Err(ImageGenError::Invalid(_))
    ));
    assert!(matches!(
        check(|r| r.sampling.steps = 0),
        Err(ImageGenError::Invalid(_))
    ));
    assert!(matches!(
        check(|r| r.sampling.steps = MAX_STEPS + 1),
        Err(ImageGenError::Invalid(_))
    ));
    assert!(matches!(
        check(|r| r.sampling.cfg_scale = f32::NAN),
        Err(ImageGenError::Invalid(_))
    ));
    assert!(matches!(
        check(|r| r.sampling.cfg_scale = 31.0),
        Err(ImageGenError::Invalid(_))
    ));
    assert_eq!(
        check(|r| {
            r.width = 2048;
            r.height = 64;
            r.sampling.cfg_scale = 0.0;
        }),
        Ok(())
    );

    let mut empty = request("");
    empty.prompt = " ".into();
    assert!(matches!(
        engine.generate(&spec, &empty, &mut |_| {}).await,
        Err(ImageGenError::Invalid(_))
    ));
    assert_eq!(engine.server_pid(), None);

    let mut missing = spec.clone();
    missing
        .files
        .push((Component::Llm, dir.path().join("absent.gguf")));
    assert_eq!(
        generate(&engine, &missing, "a ginger kitten").await,
        Err(ImageGenError::MissingFile(dir.path().join("absent.gguf")))
    );
    assert_eq!(engine.server_pid(), None);

    let none = ImageGen::new(Config::new(None, dir.path()));
    assert_eq!(
        generate(&none, &spec, "x").await,
        Err(ImageGenError::Unavailable)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_the_engine_cannot_load_gives_its_last_error() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "fail-load");
    match generate(&engine, &spec, "a ginger kitten").await {
        Err(ImageGenError::StartFailed(reason)) => assert_eq!(reason, "new_sd_ctx_t failed"),
        other => panic!("{other:?}"),
    }
    assert_eq!(engine.server_pid(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_never_answers_is_stopped() {
    let dir = temp_dir();
    let mut config = fake_config(dir.path());
    config.timing.ready_timeout = Duration::from_millis(1500);
    let engine = ImageGen::new(config);
    let spec = spec(dir.path(), "never-ready");
    let (started, mut launched) = tokio::sync::watch::channel(None);
    let watcher = {
        let engine = engine.clone();
        tokio::spawn(async move {
            loop {
                if let Some(pid) = engine.server_pid() {
                    let _ = started.send(Some(pid));
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
    };
    assert_eq!(
        generate(&engine, &spec, "a ginger kitten").await,
        Err(ImageGenError::NotReady(1))
    );
    watcher.await.unwrap();
    let pid = launched.borrow_and_update().unwrap();
    assert!(gone_within(pid, Duration::from_secs(5)).await);
    assert_eq!(engine.server_pid(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_image_keeps_the_server_and_a_crash_restarts_it() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "klein");
    assert_eq!(
        generate(&engine, &spec, "[fail] a kitten").await,
        Err(ImageGenError::Failed("fake failure".into()))
    );
    let first = engine.server_pid().unwrap();
    generate(&engine, &spec, "a kitten").await.unwrap();
    assert_eq!(engine.server_pid(), Some(first));

    match generate(&engine, &spec, "[crash] a kitten").await {
        Err(ImageGenError::Crashed(status)) => assert!(status.contains("134"), "{status}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(engine.server_pid(), None);
    generate(&engine, &spec, "a kitten").await.unwrap();
    let second = engine.server_pid().unwrap();
    assert_ne!(second, first);
    engine.shutdown().await;
}

/// Lance une image qui ne finit pas (`prompt`) dans une tâche ; rend la
/// tâche une fois le calcul commencé.
async fn hanging(
    engine: &ImageGen,
    spec: &ModelSpec,
    prompt: &'static str,
) -> tokio::task::JoinHandle<Result<Image, ImageGenError>> {
    let (started, mut generating) = tokio::sync::watch::channel(false);
    let worker = {
        let engine = engine.clone();
        let spec = spec.clone();
        tokio::spawn(async move {
            let mut seen = move |progress: Progress| {
                if matches!(progress, Progress::Generating { .. }) {
                    let _ = started.send(true);
                }
            };
            engine.generate(&spec, &request(prompt), &mut seen).await
        })
    };
    let _ = generating.wait_for(|started| *started).await;
    worker
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_an_image_being_made_stops_the_server() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "klein");
    let worker = hanging(&engine, &spec, "[hang] a kitten").await;
    // Lisible pendant l'image (le verrou des images est tenu).
    let first = engine.server_pid().unwrap();
    worker.abort();
    let _ = worker.await;
    // Lâché : le serveur, qui ne sait pas interrompre l'image, est arrêté.
    assert!(gone_within(first, Duration::from_secs(5)).await);
    assert_eq!(engine.server_pid(), None);
    generate(&engine, &spec, "again").await.unwrap();
    assert_ne!(engine.server_pid(), Some(first));
    engine.shutdown().await;
}

/// Un serveur lent à mourir (SIGTERM ignoré) est attendu avant d'en lancer
/// un autre : les deux ne se partagent jamais la mémoire.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_new_server_waits_for_the_previous_one_to_die() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "klein");
    let worker = hanging(&engine, &spec, "[deaf] a kitten").await;
    let first = engine.server_pid().unwrap();
    worker.abort();
    let _ = worker.await;
    let overlapped = Arc::new(Mutex::new(None));
    let mut seen = {
        let overlapped = overlapped.clone();
        move |progress: Progress| {
            if matches!(progress, Progress::Generating { .. }) {
                overlapped.lock().unwrap().get_or_insert(alive(first));
            }
        }
    };
    engine
        .generate(&spec, &request("again"), &mut seen)
        .await
        .unwrap();
    assert_eq!(*overlapped.lock().unwrap(), Some(false));
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_start_leaves_no_server() {
    let dir = temp_dir();
    let mut config = fake_config(dir.path());
    config.timing.ready_timeout = Duration::from_secs(30);
    let engine = ImageGen::new(config);
    let spec = spec(dir.path(), "never-ready");
    let work = {
        let engine = engine.clone();
        tokio::spawn(async move { generate(&engine, &spec, "a kitten").await })
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let pid = loop {
        if let Some(pid) = engine.server_pid() {
            break pid;
        }
        assert!(Instant::now() < deadline, "never started");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    work.abort();
    let _ = work.await;
    // Le processus lancé (qui n'écoutait pas encore) est arrêté.
    assert!(gone_within(pid, Duration::from_secs(5)).await);
    assert_eq!(engine.server_pid(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_wait_their_turn() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "klein");
    let seen = Seen::default();
    let (first, second) = tokio::join!(generate(&engine, &spec, "[slow] first"), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        engine
            .generate(&spec, &request("second"), &mut seen.sink())
            .await
    });
    first.unwrap();
    second.unwrap();
    // La seconde attend son tour sans rien envoyer : le serveur ne l'a
    // reçue qu'une fois la première finie.
    assert_eq!(seen.all().first(), Some(&Progress::Queued));
    let url = server_url(&engine).await;
    let received = get_json(&format!("{url}/fake/received")).await;
    assert_eq!(
        received,
        serde_json::json!([
            {"prompt": "[slow] first", "busy": false},
            {"prompt": "second", "busy": false},
        ])
    );
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn progress_is_announced_once_per_change() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "klein");
    // Sans aperçu, aucune étape n'est connue : un seul « en cours ».
    let mut quiet = request("[slow] a kitten");
    quiet.preview = false;
    let seen = Seen::default();
    engine
        .generate(&spec, &quiet, &mut seen.sink())
        .await
        .unwrap();
    let generating = seen
        .all()
        .iter()
        .filter(|progress| matches!(progress, Progress::Generating { .. }))
        .count();
    assert_eq!(generating, 1, "{:?}", seen.all());
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_stops_after_a_while_without_images() {
    let dir = temp_dir();
    let mut config = fake_config(dir.path());
    config.timing.idle_stop = Duration::from_millis(1500);
    let engine = ImageGen::new(config);
    let spec = spec(dir.path(), "klein");
    generate(&engine, &spec, "one").await.unwrap();
    let first = engine.server_pid().unwrap();
    // Une nouvelle image avant le délai le repousse.
    tokio::time::sleep(Duration::from_millis(900)).await;
    generate(&engine, &spec, "two").await.unwrap();
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(engine.server_pid(), Some(first));
    assert!(alive(first));
    assert!(gone_within(first, Duration::from_secs(5)).await);
    assert_eq!(engine.server_pid(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn shutting_down_stops_the_image_being_made_and_any_later_one() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "klein");
    let worker = hanging(&engine, &spec, "[hang] a kitten").await;
    let pid = engine.server_pid().unwrap();
    let began = Instant::now();
    engine.shutdown().await;
    assert!(began.elapsed() < Duration::from_secs(4));
    // Arrêté quand `shutdown` rend la main.
    assert!(!alive(pid));
    assert_eq!(worker.await.unwrap(), Err(ImageGenError::Closed));
    assert_eq!(
        generate(&engine, &spec, "later").await,
        Err(ImageGenError::Closed)
    );
}

#[test]
fn the_bundled_engine_is_found_under_the_resources() {
    // La variable des builds d'essai passerait avant les ressources.
    if std::env::var_os("CHATONLM_SD_SERVER_BIN").is_some() {
        return;
    }
    let dir = temp_dir();
    assert_eq!(locate(Some(dir.path())), None);
    assert_eq!(locate(None), None);
    let bin_dir = dir.path().join("sd-server");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let bin = bin_dir.join(format!("sd-server{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&bin, b"").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(locate(Some(dir.path())), None, "not executable yet");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert_eq!(locate(Some(dir.path())), Some(bin));
}

/// Un serveur mort entre deux images (OOM killer, plantage hors calcul) est
/// relancé à la demande suivante, pas réutilisé.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_died_between_images_is_restarted() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "klein");
    generate(&engine, &spec, "one").await.unwrap();
    let first = engine.server_pid().unwrap();
    // SAFETY : signal envoyé au faux serveur lancé par cet essai.
    unsafe { libc::kill(first as i32, libc::SIGKILL) };
    // Récolté (pas seulement zombie : un processus à plusieurs fils l'est
    // avant que sa fin soit signalée), puis un instant pour la remontée.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Path::new(&format!("/proc/{first}")).exists() {
        assert!(Instant::now() < deadline, "never reaped");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let seen = Seen::default();
    engine
        .generate(&spec, &request("two"), &mut seen.sink())
        .await
        .unwrap();
    assert_eq!(seen.all().first(), Some(&Progress::Starting));
    assert_ne!(engine.server_pid(), Some(first));
    engine.shutdown().await;
}

/// Serveur tué juste avant une demande (sa fin n'est pas encore signalée :
/// il semble vivant) : la demande le relance une fois au lieu d'échouer.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_server_killed_just_before_a_request_is_restarted() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "klein");
    generate(&engine, &spec, "one").await.unwrap();
    let first = engine.server_pid().unwrap();
    // SAFETY : signal envoyé au faux serveur lancé par cet essai.
    unsafe { libc::kill(first as i32, libc::SIGKILL) };
    generate(&engine, &spec, "two").await.unwrap();
    assert_ne!(engine.server_pid(), Some(first));
    engine.shutdown().await;
}

/// Sur un processeur sans les instructions de ggml, rien n'est lancé : le
/// serveur s'arrêterait sur une instruction illégale.
#[tokio::test(flavor = "multi_thread")]
async fn a_processor_without_the_needed_instructions_is_refused() {
    let dir = temp_dir();
    let mut config = fake_config(dir.path());
    config.cpu_missing = || &["AVX2", "FMA"];
    let engine = ImageGen::new(config);
    let spec = spec(dir.path(), "klein");
    assert_eq!(
        generate(&engine, &spec, "a kitten").await,
        Err(ImageGenError::CpuUnsupported("AVX2, FMA".into()))
    );
    assert_eq!(engine.server_pid(), None);
}

#[test]
fn a_search_dir_is_added_first_or_last_whatever_the_case_of_the_variable() {
    let dir = Path::new("/opt/engine/fallback");
    let mut environment = BTreeMap::new();
    add_search_dir(&mut environment, "LD_LIBRARY_PATH", dir, true, ':');
    assert_eq!(
        environment.get(OsStr::new("LD_LIBRARY_PATH")),
        Some(&OsString::from("/opt/engine/fallback"))
    );
    add_search_dir(
        &mut environment,
        "LD_LIBRARY_PATH",
        Path::new("/x"),
        true,
        ':',
    );
    assert_eq!(
        environment.get(OsStr::new("LD_LIBRARY_PATH")),
        Some(&OsString::from("/x:/opt/engine/fallback"))
    );
    // Windows : `Path` existant, complété en queue, sans second `PATH`.
    let mut environment = BTreeMap::from([(
        OsString::from("Path"),
        OsString::from(r"C:\Windows\system32;C:\Windows"),
    )]);
    add_search_dir(
        &mut environment,
        "PATH",
        Path::new(r"C:\App\sd-server\fallback"),
        false,
        ';',
    );
    assert_eq!(environment.len(), 1);
    assert_eq!(
        environment.get(OsStr::new("Path")),
        Some(&OsString::from(
            r"C:\Windows\system32;C:\Windows;C:\App\sd-server\fallback"
        ))
    );
}

/// Chargeur Vulkan de secours : donné au serveur seulement si le système n'en
/// a pas (Linux), toujours en dernier recours (Windows).
#[test]
fn the_fallback_vulkan_loader_is_offered_only_as_a_last_resort() {
    let dir = temp_dir();
    let fallback = dir.path().join("fallback");
    let mut environment = BTreeMap::new();
    // Pas de chargeur livré : rien.
    os::vulkan_loader(&mut environment, &fallback, false);
    assert!(environment.is_empty());
    std::fs::create_dir_all(&fallback).unwrap();
    std::fs::write(fallback.join(os::LOADER), b"").unwrap();
    os::vulkan_loader(&mut environment, &fallback, true);
    #[cfg(target_os = "linux")]
    {
        assert!(environment.is_empty(), "the system's loader is used");
        os::vulkan_loader(&mut environment, &fallback, false);
        assert_eq!(
            environment.get(OsStr::new("LD_LIBRARY_PATH")),
            Some(&fallback.clone().into_os_string())
        );
    }
    #[cfg(windows)]
    assert_eq!(
        environment.get(OsStr::new("PATH")),
        Some(&fallback.clone().into_os_string())
    );
}

/// Sans graine donnée, chaque image en tire une nouvelle.
#[tokio::test(flavor = "multi_thread")]
async fn each_image_without_a_seed_draws_its_own() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "klein");
    let mut request = request("a ginger kitten");
    request.seed = None;
    request.preview = false;
    let mut seeds = Vec::new();
    for _ in 0..3 {
        let image = engine.generate(&spec, &request, &mut |_| {}).await.unwrap();
        seeds.push(image.seed);
    }
    seeds.dedup();
    // Trois tirages égaux : une chance sur 2^62.
    assert!(seeds.len() > 1, "{seeds:?}");
    engine.shutdown().await;
}

/// Une tâche que le serveur ne connaît pas (404) est un échec de l'image,
/// donné aussitôt, pas une panne du réseau.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_job_is_a_failure_not_a_lost_server() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "klein");
    generate(&engine, &spec, "one").await.unwrap();
    let slot = engine.inner.server.lock().await;
    let running = slot.as_ref().unwrap();
    match api::job(&running.client(), running.url(), "job_unknown").await {
        Ok(api::Job {
            state: api::JobState::Failed { message },
        }) => assert_eq!(message, "job not found"),
        other => panic!("{other:?}"),
    }
    drop(slot);
    engine.shutdown().await;
}

/// La description négative est bornée comme la description.
#[test]
fn a_too_long_negative_prompt_is_refused() {
    let mut long = request("a ginger kitten");
    long.negative_prompt = "x".repeat(MAX_PROMPT + 1);
    assert!(matches!(validate(&long), Err(ImageGenError::Invalid(_))));
    long.negative_prompt = "x".repeat(MAX_PROMPT);
    assert_eq!(validate(&long), Ok(()));
}

/// Fermeture juste après une image lâchée : `shutdown` attend aussi l'arrêt
/// lancé en tâche de fond par le serveur lâché (SIGTERM ignoré).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn shutting_down_waits_for_a_dropped_server_to_die() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "klein");
    let worker = hanging(&engine, &spec, "[deaf] a kitten").await;
    let pid = engine.server_pid().unwrap();
    worker.abort();
    let _ = worker.await;
    engine.shutdown().await;
    assert!(!alive(pid));
}

/// Un suivi qui échoue par moments (serveur débordé) ne perd pas l'image :
/// seule une panne continue de `lost_after` la perd.
#[tokio::test(flavor = "multi_thread")]
async fn passing_poll_errors_do_not_lose_the_image() {
    let dir = temp_dir();
    let mut config = fake_config(dir.path());
    config.timing.lost_after = Duration::from_millis(500);
    let engine = ImageGen::new(config);
    let spec = spec(dir.path(), "klein");
    // 4 étapes de 400 ms : bien plus long que `lost_after`.
    generate(&engine, &spec, "[slow] [flaky] a kitten")
        .await
        .unwrap();
    engine.shutdown().await;
}

/// Une demande en file du serveur n'est annoncée qu'une fois.
#[tokio::test(flavor = "multi_thread")]
async fn a_queued_job_is_announced_once() {
    let dir = temp_dir();
    let engine = ImageGen::new(fake_config(dir.path()));
    let spec = spec(dir.path(), "klein");
    let mut queued = request("[queued] a kitten");
    queued.preview = false;
    let seen = Seen::default();
    engine
        .generate(&spec, &queued, &mut seen.sink())
        .await
        .unwrap();
    let announced = seen
        .all()
        .iter()
        .filter(|progress| **progress == Progress::Queued)
        .count();
    assert_eq!(announced, 1, "{:?}", seen.all());
    engine.shutdown().await;
}

/// Lancement abandonné pendant la création du processus : l'enfant que le
/// fil de lancement crée quand même est arrêté aussitôt.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_spawn_given_up_leaves_no_child() {
    let dir = temp_dir();
    let slot = parking_lot::Mutex::new(None);
    let mut command = std::process::Command::new("sleep");
    command.arg("30").env("CHATONLM_TEST_GIVEN_UP", dir.path());
    // Demande envoyée au fil (un seul poll), futur lâché avant sa réponse.
    let mut spawning = Box::pin(crate::ollama::builtin::process::spawn_with(
        &slot,
        "chatonlm-test",
        command,
    ));
    let first = std::future::poll_fn(|cx| std::task::Poll::Ready(spawning.as_mut().poll(cx))).await;
    assert!(first.is_pending());
    drop(spawning);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let needle = dir.path().to_string_lossy().into_owned();
    let left: Vec<u32> = std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&pid| {
            alive(pid)
                && std::fs::read(format!("/proc/{pid}/environ"))
                    .is_ok_and(|env| String::from_utf8_lossy(&env).contains(&needle))
        })
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

/// Essais contre le vrai `sd-server`, sur le processeur seul (pilote Vulkan
/// masqué), en images minuscules. Ignorés ; à lancer à la main :
///
/// ```sh
/// CHATONLM_SD_REAL_BIN=…/.tools/sdcpp/build-vulkan/bin/sd-server \
/// CHATONLM_SD_REAL_MODELS=…/.tools/sdcpp/models \
/// cargo test --manifest-path src-tauri/Cargo.toml --lib image_gen::tests::real -- --ignored --test-threads=1
/// ```
///
/// Modèles attendus (lot 0) : `flux-2-klein-4b-Q4_0.gguf`,
/// `full_encoder_small_decoder.safetensors`, `Qwen3-4B-Q4_K_M.gguf`.
mod real {
    use super::*;

    fn real_engine(dir: &Path) -> Option<(ImageGen, ModelSpec)> {
        let bin = PathBuf::from(std::env::var_os("CHATONLM_SD_REAL_BIN")?);
        let models = PathBuf::from(std::env::var_os("CHATONLM_SD_REAL_MODELS")?);
        let mut config = Config::new(Some(bin), dir);
        config.extra_env = [
            ("VK_ICD_FILENAMES", "/nonexistent.json"),
            ("VK_DRIVER_FILES", "/nonexistent.json"),
        ]
        .map(|(key, value)| (key.into(), value.into()))
        .to_vec();
        config.timing.poll = Duration::from_millis(100);
        let spec = ModelSpec {
            files: vec![
                (
                    Component::DiffusionModel,
                    models.join("flux-2-klein-4b-Q4_0.gguf"),
                ),
                (
                    Component::Vae,
                    models.join("full_encoder_small_decoder.safetensors"),
                ),
                (Component::Llm, models.join("Qwen3-4B-Q4_K_M.gguf")),
            ],
            options: vec!["-t".into(), "4".into()],
        };
        Some((ImageGen::new(config), spec))
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "vrai sd-server et modèles requis (voir l'en-tête du module)"]
    async fn real_engine_makes_a_small_image_with_previews() {
        let dir = temp_dir();
        let Some((engine, spec)) = real_engine(dir.path()) else {
            panic!("CHATONLM_SD_REAL_BIN et CHATONLM_SD_REAL_MODELS requis");
        };
        let mut request = request("a ginger kitten sleeping on old books");
        request.width = 128;
        request.height = 128;
        let seen = Seen::default();
        let image = engine
            .generate(&spec, &request, &mut seen.sink())
            .await
            .unwrap();
        assert_eq!((image.width, image.height, image.seed), (128, 128, 7));
        let progress = seen.all();
        assert_eq!(progress.first(), Some(&Progress::Starting));
        assert!(
            progress.iter().any(|progress| matches!(
                progress,
                Progress::Generating {
                    step: Some((_, 4)),
                    preview: Some(_)
                }
            )),
            "{progress:?}"
        );
        engine.shutdown().await;
    }

    /// Correctif de ChatonLM (`packaging/sdcpp/0001-…`) : seule l'app parle
    /// au serveur ; une page web (en-tête `Origin`) ou un autre nom d'hôte
    /// (rebinding DNS) sont refusés.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "vrai sd-server et modèles requis (voir l'en-tête du module)"]
    async fn real_engine_refuses_browser_requests() {
        let dir = temp_dir();
        let Some((engine, spec)) = real_engine(dir.path()) else {
            panic!("CHATONLM_SD_REAL_BIN et CHATONLM_SD_REAL_MODELS requis");
        };
        let inner = &engine.inner;
        let running = server::start(
            &inner.config,
            &inner.spawner,
            &spec,
            &inner.pid,
            &inner.stops,
        )
        .await
        .unwrap();
        let url = format!("{}/sdcpp/v1/capabilities", running.url());
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let status = |request: reqwest::RequestBuilder| async move {
            request.send().await.unwrap().status().as_u16()
        };
        assert_eq!(status(client.get(&url)).await, 200);
        assert_eq!(
            status(client.get(&url).header("Origin", "https://example.com")).await,
            403
        );
        assert_eq!(status(client.get(&url).header("Origin", "null")).await, 403);
        assert_eq!(
            status(client.get(&url).header("Host", "attacker.example:80")).await,
            403
        );
        assert_eq!(
            status(
                client
                    .request(reqwest::Method::OPTIONS, &url)
                    .header("Origin", "https://example.com")
            )
            .await,
            403
        );
        running.stop(&inner.config.timing).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "vrai sd-server et modèles requis (voir l'en-tête du module)"]
    async fn real_engine_reports_a_model_it_cannot_load() {
        let dir = temp_dir();
        let Some((engine, mut spec)) = real_engine(dir.path()) else {
            panic!("CHATONLM_SD_REAL_BIN et CHATONLM_SD_REAL_MODELS requis");
        };
        let bad = dir.path().join("bad.gguf");
        std::fs::write(&bad, [7u8; 100]).unwrap();
        spec.files[0].1 = bad;
        match generate(&engine, &spec, "a kitten").await {
            Err(ImageGenError::StartFailed(reason)) => {
                assert!(reason.contains("new_sd_ctx_t"), "{reason}");
                assert!(reason.contains("get sd version"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "vrai sd-server et modèles requis (voir l'en-tête du module)"]
    async fn real_engine_is_stopped_when_an_image_is_dropped() {
        let dir = temp_dir();
        let Some((engine, spec)) = real_engine(dir.path()) else {
            panic!("CHATONLM_SD_REAL_BIN et CHATONLM_SD_REAL_MODELS requis");
        };
        let (started, mut generating) = tokio::sync::watch::channel(false);
        let mut seen = move |progress: Progress| {
            if matches!(progress, Progress::Generating { .. }) {
                let _ = started.send(true);
            }
        };
        let mut request = request("a kitten");
        request.width = 256;
        request.height = 256;
        // Gardé vivant le temps de relever le serveur (select! lâche les
        // branches qui ne finissent pas).
        let mut work = Box::pin(engine.generate(&spec, &request, &mut seen));
        tokio::select! {
            outcome = &mut work => panic!("finished: {outcome:?}"),
            _ = generating.wait_for(|started| *started) => {}
        }
        let pid = engine.server_pid().unwrap();
        drop(work);
        assert!(gone_within(pid, Duration::from_secs(6)).await);
    }
}
