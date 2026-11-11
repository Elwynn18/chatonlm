//! Essais de l'ordonnanceur contre le faux `llama-server` (`fake_llama::cli`)
//! lancé comme un vrai processus : ce binaire d'essai lui-même, relancé sur
//! le seul essai [`fake_llama_server`] (arguments du serveur dans
//! `fake_llama::cli::ARGS_ENV`, la ligne de commande étant à libtest).
//! Délais raccourcis ; mémoire de la machine imposée par [`FakeModels`].
//!
//! Vérification contre le vrai llama-server b12110 : `real_engine_by_hand`
//! (ignoré, voir son en-tête).

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::Value;

use super::*;
use crate::hub::types::HubGpu;
use crate::local_models::{ModelCapabilities, ModelSource};

const GIB: u64 = 1024 * 1024 * 1024;

/// Faux `llama-server` : ignoré, sauf relancé par un essai.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "faux serveur lancé par les autres essais"]
async fn fake_llama_server() {
    // Pack CUDA des essais : comme le vrai serveur dont le moteur CUDA ne se
    // charge pas (pilote absent…), un moteur marqué `FAIL` fait échouer le
    // lancement (`--device CUDA0` refusé), avant tout le reste.
    if let Some(path) = std::env::var_os(super::cuda::BACKEND_PATH_ENV)
        && std::fs::read(&path).is_ok_and(|bytes| bytes.starts_with(b"FAIL"))
    {
        eprintln!(
            "load_backend: failed to load {}: libcuda.so.1: cannot open shared object file",
            Path::new(&path).display()
        );
        eprintln!("error: invalid device: CUDA0");
        std::process::exit(1);
    }
    if let Some(args) = fake_llama::cli::args_from_env() {
        std::process::exit(fake_llama::cli::run(args).await);
    }
}

/// Modèles installés et machine imposés.
#[derive(Default)]
struct FakeModels {
    models: Mutex<HashMap<String, LocalModel>>,
    machine: Mutex<Option<Machine>>,
    /// Lecture gardée rendue par `machine()` (le cache de la détection),
    /// quand elle est posée ; `machine_now()` rend toujours la vraie.
    kept: Mutex<Option<Machine>>,
}

impl FakeModels {
    fn set_free_vram(&self, free: u64) {
        *self.machine.lock() = Some(Machine {
            gpus: vec![HubGpu {
                name: "Fake GPU".into(),
                vram_total: 24 * GIB,
                vram_free: Some(free),
            }],
            ram_total: 64 * GIB,
            ram_available: 48 * GIB,
        });
    }

    /// Carte non vue (aucune, ou `nvidia-smi` en échec).
    fn set_no_card(&self) {
        *self.machine.lock() = Some(Machine {
            gpus: Vec::new(),
            ram_total: 64 * GIB,
            ram_available: 48 * GIB,
        });
    }

    /// `machine()` rend désormais la lecture du moment, figée.
    fn freeze(&self) {
        *self.kept.lock() = self.machine.lock().clone();
    }
}

impl Models for FakeModels {
    fn local<'a>(&'a self, id: &'a str) -> BoxFuture<'a, AppResult<Option<LocalModel>>> {
        let found = self
            .models
            .lock()
            .values()
            .find(|model| same_model(&model.id, id))
            .cloned();
        Box::pin(async move { Ok(found) })
    }

    fn machine(&self) -> BoxFuture<'_, Option<Machine>> {
        let kept = self.kept.lock().clone();
        let machine = kept.or_else(|| self.machine.lock().clone());
        Box::pin(async move { machine })
    }

    fn machine_now(&self) -> BoxFuture<'_, Option<Machine>> {
        let machine = self.machine.lock().clone();
        Box::pin(async move { machine })
    }
}

/// Un moteur d'essai, son dossier et ses modèles (aussi pour les essais des
/// commandes et du chef de la carte graphique).
pub(crate) struct Bench {
    pub engine: Engine,
    models: Arc<FakeModels>,
    dir: tempfile::TempDir,
    changes: Arc<Mutex<usize>>,
}

pub(crate) fn temp_dir() -> tempfile::TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tools/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::tempdir_in(std::fs::canonicalize(base).unwrap()).unwrap()
}

pub(crate) fn fake_config(data: &Path) -> Config {
    let mut config = Config::new(Some(std::env::current_exe().unwrap()), data);
    config.prefix_args = [
        "engine::tests::fake_llama_server",
        "--exact",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]
    .map(OsString::from)
    .to_vec();
    config.args_in_env = Some(fake_llama::cli::ARGS_ENV.into());
    config.extra_env = vec![("FAKE_LLAMA_LOAD_MS".into(), "100".into())];
    // Jamais la variable des e2e : les essais choisissent eux-mêmes.
    config.force_cpu = false;
    config.timing = Timing {
        ready_timeout: Duration::from_secs(30),
        ready_poll: Duration::from_millis(50),
        stop_grace: Duration::from_secs(2),
        kill_wait: Duration::from_secs(2),
    };
    config
}

fn bench_with(adjust: impl FnOnce(&mut Config)) -> Bench {
    let dir = temp_dir();
    let mut config = fake_config(&dir.path().join("data"));
    adjust(&mut config);
    let models = Arc::new(FakeModels::default());
    models.set_free_vram(20 * GIB);
    let engine = Engine::new(config, models.clone());
    let changes = Arc::new(Mutex::new(0));
    let counted = changes.clone();
    engine.on_change(move |_| *counted.lock() += 1);
    Bench {
        engine,
        models,
        dir,
        changes,
    }
}

pub(crate) fn bench() -> Bench {
    bench_with(|_| {})
}

impl Bench {
    /// Installe un modèle `test/<name>-GGUF:Q4_K_M` (fichier factice : le
    /// faux ne le lit pas ; le nom porte le scénario).
    pub fn install(&self, name: &str, size: u64) -> String {
        let id = format!("test/{name}-GGUF:Q4_K_M");
        let dir = self.dir.path().join("models").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}-Q4_K_M.gguf"));
        std::fs::write(&path, b"GGUF").unwrap();
        self.models.models.lock().insert(
            id.clone(),
            LocalModel {
                id: id.clone(),
                display_name: name.into(),
                repo: format!("test/{name}-GGUF"),
                quant: "Q4_K_M".into(),
                path: path.to_string_lossy().into_owned(),
                mmproj: None,
                size_bytes: size,
                context_max: Some(40_960),
                capabilities: ModelCapabilities::default(),
                arch: Some("qwen3".into()),
                params_label: Some("0.6B".into()),
                installed_at: 0,
                source: ModelSource::Hf,
            },
        );
        id
    }

    pub fn logs(&self) -> PathBuf {
        self.dir.path().join("data").join("logs")
    }

    /// Journal du modèle `id` (`llama-<modèle>.log`).
    pub fn log_of(&self, id: &str) -> PathBuf {
        self.logs().join(journal::file_name(id))
    }

    /// Journal du lancement précédent de `id` (`.1`).
    pub fn old_log_of(&self, id: &str) -> PathBuf {
        self.log_of(id).with_extension("log.1")
    }

    /// Fichier du modèle `id` installé.
    pub fn model_path(&self, id: &str) -> PathBuf {
        PathBuf::from(&self.models.models.lock()[id].path)
    }

    fn ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self.engine.running().into_iter().map(|m| m.id).collect();
        ids.sort();
        ids
    }

    fn model(&self, id: &str) -> Option<RunningModel> {
        self.engine.running().into_iter().find(|m| m.id == id)
    }
}

fn kept() -> LoadOptions {
    LoadOptions {
        keep_alive: KeepAlive::Forever,
        ..LoadOptions::default()
    }
}

fn with_ctx(ctx: u32) -> LoadOptions {
    LoadOptions {
        ctx: Some(ctx),
        ..kept()
    }
}

/// Le processus `pid` vit encore (ni absent ni zombie).
pub(crate) fn alive(pid: u32) -> bool {
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
        crate::process::alive(pid)
    }
}

/// Attend que `pid` soit fini (3 s au plus).
async fn dead(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while alive(pid) {
        if Instant::now() > deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    true
}

/// Attend que `check` soit vrai (`limit` au plus).
async fn until(limit: Duration, check: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while !check() {
        if Instant::now() > deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    true
}

/// GET sans clé d'une route de pilotage du faux.
async fn pilot(lease: &Lease, route: &str) -> Value {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}/fake/{route}", lease.endpoint.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// Lancé, prêt, clé exigée (jamais en ligne de commande), options du
/// contrat, journal et environnement nettoyé ; un second `ensure` reprend le
/// même processus.
#[tokio::test(flavor = "multi_thread")]
async fn a_model_is_launched_ready_and_guarded_by_its_key() {
    let bench = bench();
    let id = bench.install("simple", GIB);
    let lease = bench.engine.ensure(&id, with_ctx(8192)).await.unwrap();
    assert_eq!(lease.model, id);
    assert_eq!(lease.ctx, 8192);
    assert!(!lease.cpu_fallback);
    assert!(lease.endpoint.base_url.starts_with("http://127.0.0.1:"));
    // Notre clé passe, une autre non.
    let props = crate::inference::props(&lease.endpoint).await.unwrap();
    assert_eq!(props.n_ctx, 8192);
    assert_eq!(props.model_alias, id);
    let stranger = Endpoint::new(lease.endpoint.base_url.clone(), "other").unwrap();
    assert!(matches!(
        crate::inference::props(&stranger).await,
        Err(crate::inference::InferenceError::Unauthorized)
    ));
    let args: Vec<String> = serde_json::from_value(pilot(&lease, "args").await).unwrap();
    let line = args.join(" ");
    assert!(line.contains("--host 127.0.0.1"), "{line}");
    assert!(
        line.contains("-c 8192 -np 1 --no-webui --offline -lv 4"),
        "{line}"
    );
    // Cache des invites borné d'après la RAM du banc (64 Gio).
    assert!(line.contains("--cache-ram 2048"), "{line}");
    assert!(!line.contains(&lease.endpoint.api_key), "{line}");
    assert!(!line.contains("-ngl"), "{line}");
    let env = pilot(&lease, "env").await;
    assert_eq!(env["LLAMA_API_KEY"], lease.endpoint.api_key.as_str());
    assert_eq!(
        env["LLAMA_CACHE"],
        bench
            .dir
            .path()
            .join("data/engine/cache")
            .to_string_lossy()
            .as_ref()
    );
    assert!(env.get("GGML_VK_VISIBLE_DEVICES").is_none());
    // Journal, mémoire lue dans le journal.
    let log = std::fs::read_to_string(bench.log_of(&id)).unwrap();
    assert!(log.contains("model loaded"), "{log}");
    let shown = bench.model(&id).unwrap();
    assert_eq!(shown.state, ModelState::Ready);
    assert!(shown.in_use);
    assert_eq!(shown.context_length, 8192);
    assert_eq!(shown.gpu_layers, Some(29));
    let kv = (8192.0 * 448.0 / 4096.0 * 1024.0 * 1024.0) as u64;
    assert!(shown.vram_bytes > kv, "{}", shown.vram_bytes);
    assert!(shown.size_bytes > shown.vram_bytes);
    assert!(shown.kept && shown.expires_at.is_none());
    // Même modèle, mêmes options : même processus.
    let pid = bench.engine.pid_of(&id).unwrap();
    let again = bench.engine.ensure(&id, with_ctx(8192)).await.unwrap();
    assert_eq!(again.endpoint.base_url, lease.endpoint.base_url);
    assert_eq!(bench.engine.pid_of(&id), Some(pid));
    assert!(*bench.changes.lock() >= 2);
    drop((lease, again));
    bench.engine.shutdown().await;
    assert!(dead(pid).await);
}

/// Un autre contexte relance le modèle, mais seulement une fois ses requêtes
/// finies (comme `needsReload` d'Ollama).
#[tokio::test(flavor = "multi_thread")]
async fn another_context_relaunches_once_the_model_is_free() {
    let bench = bench();
    let id = bench.install("simple", GIB);
    let first = bench.engine.ensure(&id, with_ctx(4096)).await.unwrap();
    let pid = bench.engine.pid_of(&id).unwrap();
    let engine = bench.engine.clone();
    let wanted = id.clone();
    let mut second = tokio::spawn(async move {
        engine
            .ensure(&wanted, with_ctx(2048))
            .await
            .map(|l| (l.ctx, l))
    });
    // Tenu par `first` : toujours en attente.
    assert!(
        tokio::time::timeout(Duration::from_millis(500), &mut second)
            .await
            .is_err()
    );
    assert_eq!(bench.engine.pid_of(&id), Some(pid));
    drop(first);
    let (ctx, lease) = second.await.unwrap().unwrap();
    assert_eq!(ctx, 2048);
    assert!(dead(pid).await, "the old process is stopped");
    assert_ne!(bench.engine.pid_of(&id), Some(pid));
    assert_eq!(
        crate::inference::props(&lease.endpoint)
            .await
            .unwrap()
            .n_ctx,
        2048
    );
    // Le journal du premier lancement est gardé en `.1`.
    assert!(bench.old_log_of(&id).is_file());
    drop(lease);
    bench.engine.shutdown().await;
}

/// Contexte non imposé (`ensure_at_least`) : une instance TENUE avec un
/// contexte plus grand est reprise aussitôt (même processus, son contexte) ;
/// un plus petit attend comme avant ; libre, elle est relancée au contexte
/// demandé (un réglage abaissé reprend effet) (relecture L5a, B1).
#[tokio::test(flavor = "multi_thread")]
async fn a_larger_context_in_use_is_taken_as_is() {
    let bench = bench();
    let id = bench.install("simple", GIB);
    let held = bench.engine.ensure(&id, with_ctx(8192)).await.unwrap();
    let pid = bench.engine.pid_of(&id).unwrap();
    assert!(bench.engine.is_ready_at_least(&id, with_ctx(4096)).await);
    assert!(!bench.engine.is_ready(&id, with_ctx(4096)).await);
    let taken = tokio::time::timeout(
        Duration::from_secs(5),
        bench.engine.ensure_at_least(&id, with_ctx(4096)),
    )
    .await
    .expect("taken without waiting")
    .unwrap();
    assert_eq!(taken.ctx, 8192);
    assert_eq!(bench.engine.pid_of(&id), Some(pid));
    drop(taken);
    // Plus grand que celui qui tourne : attend qu'il soit libre.
    assert!(!bench.engine.is_ready_at_least(&id, with_ctx(16_384)).await);
    let engine = bench.engine.clone();
    let wanted = id.clone();
    let mut larger =
        tokio::spawn(async move { engine.ensure_at_least(&wanted, with_ctx(16_384)).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(500), &mut larger)
            .await
            .is_err()
    );
    drop(held);
    let larger = larger.await.unwrap().unwrap();
    assert_eq!(larger.ctx, 16_384);
    assert!(dead(pid).await, "the smaller process is stopped");
    let pid = bench.engine.pid_of(&id).unwrap();
    drop(larger);
    // Libre : relancé au contexte demandé.
    assert!(!bench.engine.is_ready_at_least(&id, with_ctx(4096)).await);
    let smaller = bench
        .engine
        .ensure_at_least(&id, with_ctx(4096))
        .await
        .unwrap();
    assert_eq!(smaller.ctx, 4096);
    assert!(dead(pid).await, "the idle larger process is replaced");
    drop(smaller);
    bench.engine.shutdown().await;
}

/// Durée en mémoire : déchargé après son `keep_alive` une fois libre, jamais
/// pendant une requête ; `Unload` aussitôt ; `Forever` jamais ; `Default` :
/// 5 minutes annoncées.
#[tokio::test(flavor = "multi_thread")]
async fn models_leave_after_their_keep_alive() {
    let bench = bench();
    let short = bench.install("simple", GIB);
    let options = LoadOptions {
        keep_alive: KeepAlive::For(Duration::from_millis(400)),
        ..LoadOptions::default()
    };
    let lease = bench.engine.ensure(&short, options).await.unwrap();
    let pid = bench.engine.pid_of(&short).unwrap();
    // Tenu plus longtemps que son `keep_alive` : gardé.
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(bench.ids(), std::slice::from_ref(&short));
    assert!(bench.model(&short).unwrap().expires_at.is_some());
    drop(lease);
    let shown = bench.model(&short).unwrap();
    assert!(!shown.in_use);
    let left = shown.expires_at.unwrap() - crate::store::now_ms();
    assert!((0..=400).contains(&left), "{left}");
    assert!(until(Duration::from_secs(3), || bench.ids().is_empty()).await);
    assert!(dead(pid).await);

    // Unload : arrêté dès la fin de la requête.
    let lease = bench
        .engine
        .ensure(
            &short,
            LoadOptions {
                keep_alive: KeepAlive::Unload,
                ..LoadOptions::default()
            },
        )
        .await
        .unwrap();
    let pid = bench.engine.pid_of(&short).unwrap();
    drop(lease);
    // Retiré de la liste dès la fin du `Lease` (pas par un minuteur à zéro :
    // la page ne le voit jamais « expiré maintenant »).
    assert!(bench.ids().is_empty(), "{:?}", bench.ids());
    assert!(dead(pid).await);

    // Forever : toujours là ; Default : 5 minutes.
    bench.engine.preload(&short, kept()).await.unwrap();
    let defaulted = bench.install("other-simple", GIB);
    bench
        .engine
        .preload(&defaulted, LoadOptions::default())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(bench.ids().len(), 2);
    assert!(bench.model(&short).unwrap().kept);
    let left = bench.model(&defaulted).unwrap().expires_at.unwrap() - crate::store::now_ms();
    assert!((290_000..=300_000).contains(&left), "{left}");
    bench.engine.shutdown().await;
}

/// Plus de [`Config::max_loaded`] modèles : le moins récemment servi des
/// inactifs part, jamais un modèle tenu.
#[tokio::test(flavor = "multi_thread")]
async fn the_least_recently_used_idle_model_makes_room() {
    let bench = bench_with(|config| config.max_loaded = 2);
    let a = bench.install("a-simple", GIB);
    let b = bench.install("b-simple", GIB);
    let c = bench.install("c-simple", GIB);
    let held = bench.engine.ensure(&a, kept()).await.unwrap();
    bench.engine.preload(&b, kept()).await.unwrap();
    // `a` est le plus ancien, mais tenu : `b` part.
    bench.engine.preload(&c, kept()).await.unwrap();
    assert_eq!(bench.ids(), [a.clone(), c.clone()]);
    // Fin de la requête de `a` : servi après `c`, c'est `c` qui part pour
    // `b`.
    drop(held);
    bench.engine.preload(&b, kept()).await.unwrap();
    assert_eq!(bench.ids(), [a.clone(), b.clone()]);
    bench.engine.shutdown().await;
}

/// Pas assez de VRAM libre pour le nouveau modèle : les modèles inactifs
/// partent (leur mémoire, lue dans leur journal, revient) ; un modèle tenu
/// reste ; assez de place : rien ne part.
#[tokio::test(flavor = "multi_thread")]
async fn models_are_evicted_when_the_card_is_full() {
    let bench = bench();
    let small = bench.install("small-simple", GIB / 2);
    let big = bench.install("big-simple", 6 * GIB);
    bench.engine.preload(&small, kept()).await.unwrap();
    // Assez de place : les deux.
    bench.engine.preload(&big, kept()).await.unwrap();
    assert_eq!(bench.ids(), [big.clone(), small.clone()]);
    bench.engine.unload(&big).await;
    // Carte presque pleine (le petit l'occupe) : il part pour le grand.
    bench.models.set_free_vram(GIB);
    bench.engine.preload(&big, kept()).await.unwrap();
    assert_eq!(bench.ids(), std::slice::from_ref(&big));
    // Tenu : jamais évincé (le grand passe quand même, `--fit` répartit).
    bench.engine.unload(&big).await;
    let held = bench.engine.ensure(&small, kept()).await.unwrap();
    bench.engine.preload(&big, kept()).await.unwrap();
    assert_eq!(bench.ids(), [big.clone(), small.clone()]);
    drop(held);
    bench.engine.shutdown().await;
}

/// Déchargement : même en cours de chargement (l'attente finit en
/// `Unloaded`), même tenu ; tout d'un coup ; sans effet sur un modèle absent.
#[tokio::test(flavor = "multi_thread")]
async fn models_are_unloaded_even_while_loading() {
    let bench = bench();
    let slow = bench.install("chatonlm-slowload", GIB);
    let quick = bench.install("simple", GIB);
    let engine = bench.engine.clone();
    let wanted = slow.clone();
    let loading = tokio::spawn(async move { engine.ensure(&wanted, kept()).await });
    assert!(
        until(Duration::from_secs(5), || bench
            .engine
            .pid_of(&slow)
            .is_some())
        .await
    );
    let pid = bench.engine.pid_of(&slow).unwrap();
    assert_eq!(bench.model(&slow).unwrap().state, ModelState::Loading);
    bench.engine.unload(&slow).await;
    assert_eq!(loading.await.unwrap().unwrap_err(), EngineError::Unloaded);
    assert!(dead(pid).await);
    assert!(bench.ids().is_empty());

    let held = bench.engine.ensure(&quick, kept()).await.unwrap();
    let pid = bench.engine.pid_of(&quick).unwrap();
    bench.engine.unload("test/absent-GGUF:Q4_K_M").await;
    // Tenu : arrêté quand même (le bouton, la suppression du modèle).
    bench
        .engine
        .unload(&quick.to_lowercase().replace("q4_k_m", "Q4_K_M"))
        .await;
    assert!(bench.ids().contains(&quick), "the repo is compared exactly");
    bench
        .engine
        .unload(&quick.replace("Q4_K_M", "q4_k_m"))
        .await;
    assert!(bench.ids().is_empty());
    assert!(dead(pid).await);
    assert!(
        crate::inference::props(&held.endpoint).await.is_err(),
        "the lease of an unloaded model fails"
    );
    drop(held);

    bench.engine.preload(&quick, kept()).await.unwrap();
    bench.engine.preload(&slow, kept()).await.unwrap();
    let pids = [
        bench.engine.pid_of(&quick).unwrap(),
        bench.engine.pid_of(&slow).unwrap(),
    ];
    bench.engine.unload_all().await;
    assert!(bench.ids().is_empty());
    for pid in pids {
        assert!(dead(pid).await);
    }
    bench.engine.shutdown().await;
}

/// La carte échoue au chargement (Vulkan, plantage) : un nouvel essai sur le
/// processeur, gardé pour la session et annoncé ; un modèle refusé (code 1,
/// sans Vulkan) n'est pas réessayé.
#[tokio::test(flavor = "multi_thread")]
async fn a_card_failure_falls_back_to_the_processor() {
    let bench = bench();
    let id = bench.install("chatonlm-vulkanfail", GIB);
    assert!(!bench.engine.gpu_failed());
    // Borné : sans la note de la session (`gpu_failed`), `ensure` relancerait
    // sans fin (essai sur la carte, repli, autre `Spec`, …).
    let lease = tokio::time::timeout(Duration::from_secs(20), bench.engine.ensure(&id, kept()))
        .await
        .expect("no endless relaunch")
        .unwrap();
    assert!(lease.cpu_fallback);
    // Écran du moteur : repli de la session.
    assert!(bench.engine.gpu_failed());
    let args: Vec<String> = serde_json::from_value(pilot(&lease, "args").await).unwrap();
    assert!(args.join(" ").ends_with("-ngl 0"), "{args:?}");
    let env = pilot(&lease, "env").await;
    assert_eq!(env["GGML_VK_VISIBLE_DEVICES"], "none");
    let shown = bench.model(&id).unwrap();
    assert!(shown.cpu_fallback && shown.cpu_only);
    assert!(
        shown
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("processor")),
        "{shown:?}"
    );
    assert_eq!(shown.vram_bytes, 0);
    assert_eq!(shown.gpu_layers, None);
    // Le journal de l'essai sur la carte est gardé.
    let first = std::fs::read_to_string(bench.old_log_of(&id)).unwrap();
    assert!(first.contains("ErrorDeviceLost"), "{first}");
    // Repris sans relance, même sans demander le processeur.
    let pid = bench.engine.pid_of(&id).unwrap();
    drop(lease);
    // Borné de même.
    tokio::time::timeout(Duration::from_secs(20), bench.engine.preload(&id, kept()))
        .await
        .expect("no endless relaunch")
        .unwrap();
    assert_eq!(bench.engine.pid_of(&id), Some(pid));
    // Prêt avec ces options (le repli compte comme le processeur), pas avec
    // un autre contexte ; rien n'est lancé par la question.
    assert!(bench.engine.is_ready(&id, kept()).await);
    assert!(!bench.engine.is_ready(&id, with_ctx(2048)).await);
    assert!(
        !bench
            .engine
            .is_ready("test/absent-GGUF:Q4_K_M", kept())
            .await
    );
    assert_eq!(bench.engine.pid_of(&id), Some(pid));
    // Et après un déchargement : directement sur le processeur.
    bench.engine.unload(&id).await;
    let lease = bench.engine.ensure(&id, kept()).await.unwrap();
    assert!(lease.cpu_fallback);
    drop(lease);

    // Refusé sans mettre la carte en cause : une erreur, un seul essai.
    let refused = bench.install("chatonlm-loadfail", GIB);
    let error = bench.engine.ensure(&refused, kept()).await.unwrap_err();
    let EngineError::StartFailed(reason) = &error else {
        panic!("{error:?}");
    };
    assert!(reason.contains("failed to load model"), "{reason}");
    assert_eq!(error.code(), "startFailed");
    assert!(!bench.ids().contains(&refused));
    let slot = std::fs::read_dir(bench.logs())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            std::fs::read_to_string(entry.path()).is_ok_and(|text| text.contains("loadfail"))
        })
        .count();
    assert_eq!(slot, 1, "a single launch for a refused model");
    bench.engine.shutdown().await;
}

/// Demandé sur le processeur : jamais sur la carte ; fichier absent :
/// erreur sans lancement.
#[tokio::test(flavor = "multi_thread")]
async fn the_processor_can_be_forced_and_files_are_checked() {
    let bench = bench();
    let id = bench.install("simple", GIB);
    let lease = bench
        .engine
        .ensure(
            &id,
            LoadOptions {
                cpu_only: true,
                ..kept()
            },
        )
        .await
        .unwrap();
    assert!(!lease.cpu_fallback);
    let shown = bench.model(&id).unwrap();
    assert!(shown.cpu_only && !shown.cpu_fallback);
    assert_eq!(shown.vram_bytes, 0);
    // Sur la carte ensuite : relancé.
    let pid = bench.engine.pid_of(&id).unwrap();
    drop(lease);
    bench.engine.preload(&id, kept()).await.unwrap();
    assert_ne!(bench.engine.pid_of(&id), Some(pid));
    assert!(!bench.model(&id).unwrap().cpu_only);

    let missing = bench.install("missing-simple", GIB);
    let path = bench.models.models.lock()[&missing].path.clone();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        bench.engine.ensure(&missing, kept()).await.unwrap_err(),
        EngineError::MissingFile(PathBuf::from(&path))
    );
    assert_eq!(
        bench
            .engine
            .ensure("test/never-GGUF:Q4_K_M", kept())
            .await
            .unwrap_err(),
        EngineError::NotInstalled("test/never-GGUF:Q4_K_M".into())
    );
    bench.engine.shutdown().await;
}

/// Gel (déplacement du dossier des modèles) : tout est déchargé, une demande
/// attend la fin du gel sans rien lancer, puis relit le modèle (nouveau
/// chemin) ; l'autocomplétion n'obtient rien (relecture L5b, M4).
#[tokio::test(flavor = "multi_thread")]
async fn a_frozen_engine_launches_nothing_until_thawed() {
    let bench = bench();
    let id = bench.install("simple", GIB);
    bench.engine.preload(&id, kept()).await.unwrap();
    let pid = bench.engine.pid_of(&id).unwrap();
    let frozen = bench.engine.freeze().await;
    assert!(bench.engine.running().is_empty());
    assert!(dead(pid).await);
    assert!(bench.engine.lease_if_ready(&id, kept()).await.is_none());
    let engine = bench.engine.clone();
    let wanted = id.clone();
    let mut waiting = tokio::spawn(async move { engine.ensure(&wanted, kept()).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(500), &mut waiting)
            .await
            .is_err()
    );
    assert!(bench.engine.running().is_empty(), "nothing launched");
    // Le fichier déplacé pendant le gel : relu à la fin.
    let old = bench.model_path(&id);
    let new = old.with_file_name("moved-simple.gguf");
    std::fs::rename(&old, &new).unwrap();
    bench.models.models.lock().get_mut(&id).unwrap().path = new.to_string_lossy().into_owned();
    drop(frozen);
    let lease = tokio::time::timeout(Duration::from_secs(20), waiting)
        .await
        .expect("launched once thawed")
        .unwrap()
        .unwrap();
    let args = pilot(&lease, "args").await;
    assert!(args.to_string().contains("moved-simple.gguf"), "{args}");
    drop(lease);
    bench.engine.shutdown().await;
}

/// `CHATONLM_LLAMA_CPU_ONLY=1` des e2e (configuration `force_cpu`) : tout
/// lancement sur le processeur, annoncé comme demandé (pas un repli) ; les
/// arguments le disent (relecture L8, M2). Seul `1` compte.
#[tokio::test(flavor = "multi_thread")]
async fn the_test_variable_forces_the_processor() {
    use std::ffi::OsStr;
    assert!(super::options::cpu_only_value(Some(OsStr::new("1"))));
    assert!(!super::options::cpu_only_value(Some(OsStr::new("0"))));
    assert!(!super::options::cpu_only_value(Some(OsStr::new(""))));
    assert!(!super::options::cpu_only_value(None));
    let bench = bench_with(|config| config.force_cpu = true);
    let id = bench.install("simple", GIB);
    let lease = bench.engine.ensure(&id, kept()).await.unwrap();
    assert!(!lease.cpu_fallback);
    let shown = bench.model(&id).unwrap();
    assert!(shown.cpu_only && !shown.cpu_fallback);
    let args = pilot(&lease, "args").await;
    let args: Vec<&str> = args
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        args.windows(2).any(|pair| pair == ["-ngl", "0"]),
        "{args:?}"
    );
    assert!(bench.engine.is_ready(&id, kept()).await);
    drop(lease);
    bench.engine.shutdown().await;
}

/// Mort en cours (plantage pendant une réponse) : le modèle quitte la liste,
/// la requête suivante le relance.
#[tokio::test(flavor = "multi_thread")]
async fn a_process_that_dies_leaves_the_list() {
    let bench = bench();
    let id = bench.install("chatonlm-crash", GIB);
    let lease = bench.engine.ensure(&id, kept()).await.unwrap();
    let pid = bench.engine.pid_of(&id).unwrap();
    let request = crate::inference::ChatRequest {
        messages: vec![crate::inference::Message::user("hi")],
        ..Default::default()
    };
    let answered = crate::inference::chat_once(
        &lease.endpoint,
        &request,
        tokio_util::sync::CancellationToken::new(),
    )
    .await;
    assert!(answered.is_err());
    assert!(dead(pid).await);
    assert!(until(Duration::from_secs(3), || bench.ids().is_empty()).await);
    drop(lease);
    // Relancé par la demande suivante.
    let lease = bench.engine.ensure(&id, kept()).await.unwrap();
    assert_ne!(bench.engine.pid_of(&id), Some(pid));
    drop(lease);
    bench.engine.shutdown().await;
}

/// Un serveur prêt qui refuse notre clé (le port repris entre-temps par un
/// autre llama-server) : échec « port pris » aussitôt, sans attendre le délai
/// ni garder le modèle.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_refuses_our_key_is_not_ours() {
    let bench = bench_with(|config| {
        // Appliquée après la nôtre : le faux attend une autre clé.
        config
            .extra_env
            .push(("LLAMA_API_KEY".into(), "someone-else".into()));
        config.timing.ready_timeout = Duration::from_secs(20);
    });
    let id = bench.install("simple", GIB);
    let started = Instant::now();
    let error = bench.engine.ensure(&id, kept()).await.unwrap_err();
    assert_eq!(
        error,
        EngineError::StartFailed(t!("engine.detail.portTaken"))
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert!(bench.ids().is_empty());
    bench.engine.shutdown().await;
}

/// Pas prêt dans le délai : arrêté, erreur `notReady`.
#[tokio::test(flavor = "multi_thread")]
async fn a_model_that_never_gets_ready_is_stopped() {
    let bench = bench_with(|config| config.timing.ready_timeout = Duration::from_millis(800));
    let id = bench.install("chatonlm-longload", GIB);
    let engine = bench.engine.clone();
    let wanted = id.clone();
    let loading = tokio::spawn(async move { engine.ensure(&wanted, kept()).await });
    assert!(
        until(Duration::from_secs(5), || bench
            .engine
            .pid_of(&id)
            .is_some())
        .await
    );
    let pid = bench.engine.pid_of(&id).unwrap();
    let error = loading.await.unwrap().unwrap_err();
    // 800 ms : « 1 seconde », jamais « 0 ».
    assert_eq!(error, EngineError::NotReady(1));
    assert!(bench.ids().is_empty());
    // Arrêté avant que l'erreur soit rendue (pas laissé à un `Drop`).
    assert!(!alive(pid), "the process is stopped before the error");
    let log = std::fs::read_to_string(bench.log_of(&id)).unwrap();
    assert!(!log.contains("model loaded"), "{log}");
    bench.engine.shutdown().await;
}

/// Deux demandes ensemble : un seul lancement ; une demande abandonnée
/// n'arrête pas le chargement (le modèle reste ensuite).
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_and_abandoned_requests_share_one_launch() {
    let bench = bench();
    let id = bench.install("chatonlm-slowload", GIB);
    let (a, b) = tokio::join!(
        bench.engine.ensure(&id, kept()),
        bench.engine.ensure(&id, kept())
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.endpoint.base_url, b.endpoint.base_url);
    assert!(!bench.old_log_of(&id).exists());
    drop((a, b));
    bench.engine.unload(&id).await;

    let abandoned =
        tokio::time::timeout(Duration::from_millis(300), bench.engine.ensure(&id, kept())).await;
    assert!(abandoned.is_err());
    assert!(
        until(Duration::from_secs(10), || bench
            .model(&id)
            .is_some_and(|m| m.state == ModelState::Ready))
        .await
    );
    bench.engine.shutdown().await;
}

/// Fin de l'app : tous les processus arrêtés (aucun reste), plus aucun
/// lancement, même attendu.
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_stops_everything_and_leaves_no_process() {
    let bench = bench();
    let a = bench.install("a-simple", GIB);
    let slow = bench.install("chatonlm-longload", GIB);
    let held = bench.engine.ensure(&a, kept()).await.unwrap();
    let pid = bench.engine.pid_of(&a).unwrap();
    let engine = bench.engine.clone();
    let wanted = slow.clone();
    let loading = tokio::spawn(async move { engine.ensure(&wanted, kept()).await });
    assert!(
        until(Duration::from_secs(5), || bench
            .engine
            .pid_of(&slow)
            .is_some())
        .await
    );
    let slow_pid = bench.engine.pid_of(&slow).unwrap();
    bench.engine.shutdown().await;
    assert_eq!(loading.await.unwrap().unwrap_err(), EngineError::Closed);
    assert!(dead(pid).await);
    assert!(dead(slow_pid).await);
    assert!(bench.ids().is_empty());
    assert_eq!(
        bench.engine.ensure(&a, kept()).await.unwrap_err(),
        EngineError::Closed
    );
    drop(held);
}

/// Sans binaire : indisponible, rien de lancé.
#[tokio::test]
async fn without_a_binary_the_engine_is_unavailable() {
    let bench = bench_with(|config| config.bin = None);
    let id = bench.install("simple", GIB);
    assert!(!bench.engine.available());
    assert_eq!(
        bench.engine.ensure(&id, kept()).await.unwrap_err(),
        EngineError::Unavailable
    );
}

/// Le contexte par défaut suit la VRAM totale (palier d'Ollama), ramené au
/// modèle.
#[tokio::test(flavor = "multi_thread")]
async fn the_default_context_follows_the_card() {
    let bench = bench();
    let id = bench.install("simple", GIB);
    let lease = bench.engine.ensure(&id, kept()).await.unwrap();
    // 24 Gio : 32 768.
    assert_eq!(lease.ctx, 32_768);
    drop(lease);
    bench.engine.shutdown().await;
}

/// Tue `pid` net (SIGKILL) et attend qu'il soit fini, puis laisse à la
/// surveillance le temps de publier sa fin (essais de courses, Unix).
#[cfg(unix)]
fn kill_and_settle(pid: u32) {
    // SAFETY : simple envoi de signal à un processus d'essai.
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(300));
}

/// Relecture I1 : un lancement juste après un autre lit la carte telle
/// qu'elle est (pas la lecture gardée d'avant le premier), et fait de la
/// place.
#[tokio::test(flavor = "multi_thread")]
async fn a_launch_right_after_another_sees_the_card_as_it_is() {
    let bench = bench();
    let first = bench.install("simple", GIB);
    let second = bench.install("other-simple", GIB);
    // La lecture gardée : 20 Gio libres.
    bench.models.freeze();
    bench.engine.preload(&first, kept()).await.unwrap();
    // La carte maintenant : pleine (le premier, et d'autres programmes).
    bench.models.set_free_vram(GIB / 2);
    bench.engine.preload(&second, kept()).await.unwrap();
    assert_eq!(bench.ids(), std::slice::from_ref(&second));
    bench.engine.shutdown().await;
}

/// Relecture M1 : une demande qui attendait le chargement d'un AUTRE
/// réglage du même modèle, et le voit échouer, lance le sien au lieu de
/// recevoir l'erreur d'autrui.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_does_not_take_the_failure_of_another_setting() {
    let bench = bench();
    let id = bench.install("chatonlm-bigctx", GIB);
    let engine = bench.engine.clone();
    let wanted = id.clone();
    // 40 960 (contexte du modèle) : refusé au bout de 1,5 s.
    let big = tokio::spawn(async move { engine.ensure(&wanted, with_ctx(65_536)).await });
    assert!(
        until(Duration::from_secs(5), || bench
            .engine
            .pid_of(&id)
            .is_some())
        .await
    );
    let small = bench.engine.ensure(&id, with_ctx(4096)).await;
    assert!(
        matches!(big.await.unwrap(), Err(EngineError::StartFailed(_))),
        "the big context is refused"
    );
    let small = small.expect("its own launch, not the other failure");
    assert_eq!(small.ctx, 4096);
    drop(small);
    bench.engine.shutdown().await;
}

/// Relecture M2 : un processus mort entre « prêt » et la fin du lancement
/// n'est ni rendu ni gardé dans la liste.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_process_dead_on_its_way_to_ready_is_not_kept() {
    let bench = bench();
    let id = bench.install("simple", GIB);
    // Le premier lancement seulement (une relance éventuelle vit).
    let once = std::sync::atomic::AtomicBool::new(false);
    bench.engine.hooks().after_ready = Some(Arc::new(move |pid| {
        if !once.swap(true, std::sync::atomic::Ordering::SeqCst) {
            kill_and_settle(pid);
        }
    }));
    let result = bench.engine.ensure(&id, kept()).await;
    bench.engine.hooks().after_ready = None;
    let error = result.expect_err("no lease on a dead process");
    assert!(
        matches!(error, EngineError::StartFailed(_) | EngineError::Crashed(_)),
        "{error:?}"
    );
    assert!(bench.ids().is_empty());
    bench.engine.shutdown().await;
}

/// Relecture M2 : quand le moteur apprend la fin d'un processus
/// (`on_exit`), `Process::exited` la montre déjà (sinon `loaded()` pourrait
/// rendre prêt un processus dont la fin a été ignorée pendant le
/// chargement).
#[tokio::test(flavor = "multi_thread")]
async fn the_end_is_published_before_the_engine_hears_of_it() {
    let bench = bench();
    let id = bench.install("chatonlm-longload", GIB);
    let config = fake_config(&bench.dir.path().join("direct"));
    let spawner = Mutex::new(None);
    let stopping = Stopping::default();
    let slot: Arc<Mutex<Option<Arc<launch::Process>>>> = Arc::default();
    let seen: Arc<Mutex<Option<bool>>> = Arc::default();
    let (watched, heard) = (slot.clone(), seen.clone());
    let process = launch::spawn(
        &launch::Launch {
            config: &config,
            spawner: &spawner,
            stopping: &stopping,
            cache_ram: 1024,
            cuda: None,
        },
        &Spec {
            model: bench.model_path(&id),
            mmproj: None,
            ctx: 4096,
            cpu: false,
        },
        &id,
        0,
        move |_| {
            let published = watched
                .lock()
                .as_ref()
                .is_some_and(|process| process.exited().is_some());
            *heard.lock() = Some(published);
        },
    )
    .await
    .unwrap();
    *slot.lock() = Some(process.clone());
    process.stop().await;
    assert!(until(Duration::from_secs(3), || seen.lock().is_some()).await);
    assert_eq!(*seen.lock(), Some(true));
    slot.lock().take();
}

/// Relecture M2 : un modèle prêt dont le processus est mort (sa fin pas
/// encore traitée) n'est jamais prêté : relancé.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_process_is_never_lent() {
    let bench = bench();
    let id = bench.install("simple", GIB);
    bench.engine.preload(&id, kept()).await.unwrap();
    let pid = bench.engine.pid_of(&id).unwrap();
    bench.engine.hooks().ignore_exits = true;
    tokio::task::spawn_blocking(move || kill_and_settle(pid))
        .await
        .unwrap();
    let lease = bench.engine.ensure(&id, kept()).await.unwrap();
    bench.engine.hooks().ignore_exits = false;
    assert_ne!(bench.engine.pid_of(&id), Some(pid), "relaunched");
    crate::inference::props(&lease.endpoint)
        .await
        .expect("a living server");
    drop(lease);
    bench.engine.shutdown().await;
}

/// Relecture M3 : tout juste prêt, pas encore pris par son demandeur, un
/// modèle lui est réservé (ni libéré pour une image, ni évincé) ; une
/// demande abandonnée rend cette réserve.
#[tokio::test(flavor = "multi_thread")]
async fn a_model_just_ready_is_kept_for_its_requester() {
    let bench = bench();
    let id = bench.install("chatonlm-slowload", GIB);
    let ready = || {
        bench
            .model(&id)
            .is_some_and(|model| model.state == ModelState::Ready)
    };
    let engine = bench.engine.clone();
    let mut waiting = Box::pin(engine.ensure(&id, kept()));
    // Demande notée, puis plus réveillée tant que le modèle n'est pas prêt.
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut waiting)
            .await
            .is_err()
    );
    assert!(until(Duration::from_secs(10), ready).await);
    assert_eq!(
        bench.engine.release_idle().await,
        0,
        "kept for its requester"
    );
    let lease = waiting.await.unwrap();
    drop(lease);

    bench.engine.unload(&id).await;
    let mut abandoned = Box::pin(engine.ensure(&id, kept()));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut abandoned)
            .await
            .is_err()
    );
    assert!(until(Duration::from_secs(10), ready).await);
    drop(abandoned);
    assert_eq!(
        bench.engine.release_idle().await,
        1,
        "no requester any more"
    );
    bench.engine.shutdown().await;
}

/// Relecture M4 : une détection sans carte (`nvidia-smi` en échec) n'est
/// pas gardée pour la session : le contexte par défaut suit la carte dès
/// qu'elle est vue.
#[tokio::test(flavor = "multi_thread")]
async fn a_missed_card_is_looked_for_again() {
    let bench = bench();
    let id = bench.install("simple", GIB);
    bench.models.set_no_card();
    let lease = bench.engine.ensure(&id, kept()).await.unwrap();
    assert_eq!(lease.ctx, 4096);
    drop(lease);
    bench.engine.unload(&id).await;
    bench.models.set_free_vram(20 * GIB);
    let lease = bench.engine.ensure(&id, kept()).await.unwrap();
    // 24 Gio : 32 768.
    assert_eq!(lease.ctx, 32_768);
    drop(lease);
    bench.engine.shutdown().await;
}

/// Relecture M5 : un serveur qui accepte la requête mais annonce un autre
/// modèle ou un autre contexte n'est pas le nôtre (port pris par un autre
/// llama-server, même sans clé).
#[tokio::test(flavor = "multi_thread")]
async fn a_server_with_another_model_or_context_is_not_ours() {
    let bench = bench();
    for name in ["chatonlm-otheralias", "chatonlm-otherctx"] {
        let id = bench.install(name, GIB);
        let error = bench.engine.ensure(&id, kept()).await.unwrap_err();
        assert_eq!(
            error,
            EngineError::StartFailed(t!("engine.detail.portTaken")),
            "{name}"
        );
        assert!(bench.ids().is_empty(), "{name}");
        // Deux autres ports essayés (relecture M8), pas plus.
        let path = format!("{}.attempts", bench.model_path(&id).display());
        let attempts = std::fs::read_to_string(path).unwrap().lines().count();
        assert_eq!(attempts, 3, "{name}");
    }
    bench.engine.shutdown().await;
}

/// Relecture M7 : décharger un modèle dont le processus est en création
/// (pas encore noté) attend sa fin.
#[tokio::test(flavor = "multi_thread")]
async fn unload_waits_for_a_process_being_created() {
    let bench = bench();
    let id = bench.install("simple", GIB);
    let (created, pids) = std::sync::mpsc::channel();
    let created = Mutex::new(created);
    bench.engine.hooks().before_attach = Some(Arc::new(move |pid| {
        let _ = created.lock().send(pid);
        std::thread::sleep(Duration::from_millis(500));
    }));
    let engine = bench.engine.clone();
    let wanted = id.clone();
    let loading = tokio::spawn(async move { engine.ensure(&wanted, kept()).await });
    let pid = tokio::task::spawn_blocking(move || pids.recv_timeout(Duration::from_secs(10)))
        .await
        .unwrap()
        .unwrap();
    bench.engine.unload(&id).await;
    assert!(!alive(pid), "unload returns once that process is gone");
    assert_eq!(loading.await.unwrap().unwrap_err(), EngineError::Unloaded);
    bench.engine.hooks().before_attach = None;
    bench.engine.shutdown().await;
}

/// Relecture M8 : port pris entre son choix et la liaison : un autre port,
/// au plus deux fois.
#[tokio::test(flavor = "multi_thread")]
async fn a_taken_port_is_tried_again_but_not_forever() {
    let bench = bench();
    let attempts = |id: &str| {
        let path = format!("{}.attempts", bench.model_path(id).display());
        std::fs::read_to_string(path).unwrap().lines().count()
    };
    let id = bench.install("chatonlm-bindfail", GIB);
    let lease = bench.engine.ensure(&id, kept()).await.unwrap();
    assert_eq!(attempts(&id), 2);
    drop(lease);
    let always = bench.install("chatonlm-bindfail-always", GIB);
    let error = bench.engine.ensure(&always, kept()).await.unwrap_err();
    assert!(
        matches!(&error, EngineError::StartFailed(reason) if reason.contains("couldn't bind")),
        "{error:?}"
    );
    assert_eq!(attempts(&always), 3);
    bench.engine.shutdown().await;
}

/// Relecture M9 : un arrêt envoie UN SEUL SIGTERM (un second fait planter
/// b12110 pendant son arrêt ; le faux fait de même, arrêt de 500 ms).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_model_is_stopped_with_a_single_sigterm() {
    let bench = bench_with(|config| {
        config
            .extra_env
            .push(("FAKE_LLAMA_STOP_MS".into(), "500".into()));
    });
    let id = bench.install("simple", GIB);
    bench.engine.preload(&id, kept()).await.unwrap();
    let pid = bench.engine.pid_of(&id).unwrap();
    bench.engine.unload(&id).await;
    assert!(dead(pid).await);
    let log = std::fs::read_to_string(bench.log_of(&id)).unwrap();
    assert_eq!(
        log.matches("stopping on a termination signal").count(),
        1,
        "{log}"
    );
    assert!(!log.contains("Received second interrupt"), "{log}");
    bench.engine.shutdown().await;
}

/// Vérification à la main contre le vrai llama-server b12110 (hors
/// `cargo test` ordinaire) : `CHATONLM_LLAMA_REAL_BIN` = binaire construit
/// (`.tools/runtimes/llama-server/llama-server`), `CHATONLM_LLAMA_REAL_MODEL`
/// = un petit GGUF. Lancement, prêt, clé, mémoire lue, relance pour un autre
/// contexte, repli forcé sur le processeur, arrêt sans reste.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "vrai llama-server : voir l'en-tête"]
async fn real_engine_by_hand() {
    let bin = PathBuf::from(std::env::var("CHATONLM_LLAMA_REAL_BIN").unwrap());
    let model_path = std::env::var("CHATONLM_LLAMA_REAL_MODEL").unwrap();
    let dir = temp_dir();
    let mut config = Config::new(Some(bin), &dir.path().join("data"));
    config.timing.ready_timeout = Duration::from_secs(300);
    let models = Arc::new(FakeModels::default());
    // Machine réelle.
    *models.machine.lock() = Detector::default().machine_within(Duration::ZERO).await;
    let id = "test/real-GGUF:Q4_K_M".to_owned();
    models.models.lock().insert(
        id.clone(),
        LocalModel {
            id: id.clone(),
            display_name: "real".into(),
            repo: "test/real-GGUF".into(),
            quant: "Q4_K_M".into(),
            path: model_path,
            mmproj: None,
            size_bytes: 400 * 1024 * 1024,
            context_max: Some(40_960),
            capabilities: ModelCapabilities::default(),
            arch: Some("qwen3".into()),
            params_label: Some("0.6B".into()),
            installed_at: 0,
            source: ModelSource::Hf,
        },
    );
    let engine = Engine::new(config, models);
    let started = Instant::now();
    let lease = engine.ensure(&id, with_ctx(4096)).await.unwrap();
    println!("ready in {:?}", started.elapsed());
    let shown = engine.running();
    println!("{shown:#?}");
    let props = crate::inference::props(&lease.endpoint).await.unwrap();
    assert_eq!(props.n_ctx, 4096);
    let stranger = Endpoint::new(lease.endpoint.base_url.clone(), "nope").unwrap();
    assert!(crate::inference::props(&stranger).await.is_err());
    let answer = crate::inference::chat_once(
        &lease.endpoint,
        &crate::inference::ChatRequest {
            messages: vec![crate::inference::Message::user("Say hello.")],
            think: crate::inference::Think::Off,
            max_tokens: Some(16),
            ..Default::default()
        },
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    println!("answer: {:?}", answer.content);
    let pid = engine.pid_of(&id).unwrap();
    drop(lease);
    let lease = engine.ensure(&id, with_ctx(2048)).await.unwrap();
    assert!(dead(pid).await);
    println!("{:#?}", engine.running());
    let pid = engine.pid_of(&id).unwrap();
    drop(lease);
    let lease = engine
        .ensure(
            &id,
            LoadOptions {
                ctx: Some(2048),
                cpu_only: true,
                ..kept()
            },
        )
        .await
        .unwrap();
    assert!(dead(pid).await);
    let cpu = engine.running();
    println!("{cpu:#?}");
    assert_eq!(cpu[0].vram_bytes, 0);
    let pid = engine.pid_of(&id).unwrap();
    drop(lease);
    engine.shutdown().await;
    assert!(dead(pid).await);
}

/// Banc dont le pack CUDA est installé (moteur factice de contenu `engine` :
/// `FAIL` le fait refuser par le faux serveur) et la carte compatible.
fn cuda_bench(engine: &[u8]) -> (Bench, super::cuda::Runtime) {
    let gpu = super::cuda::detect::NvidiaGpu {
        name: "NVIDIA GeForce RTX 3090".into(),
        uuid: "GPU-3090".into(),
        compute: (8, 6),
        driver: "615.78.08".into(),
    };
    let platform = super::cuda::Platform::current().unwrap_or(super::cuda::Platform::Linux);
    let mut runtime = None;
    let bench = bench_with(|config| {
        let root = config.logs.parent().unwrap().join("engine").join("cuda");
        let dir = root.join("b12110");
        std::fs::create_dir_all(&dir).unwrap();
        for file in super::cuda::pack::installed_files(platform, 13) {
            std::fs::write(dir.join(&file), b"x").unwrap();
        }
        std::fs::write(dir.join(platform.engine_file()), engine).unwrap();
        let fixed = super::cuda::Runtime::fixed(root, platform, vec![gpu]);
        config.cuda = fixed.clone();
        runtime = Some(fixed);
    });
    (bench, runtime.unwrap())
}

/// Pack CUDA installé : le modèle est lancé avec le moteur du pack, la carte
/// de CUDA seule et Vulkan caché ; un CUDA qui ne se charge pas coupe CUDA
/// pour la session et le modèle est relancé sur Vulkan (pas le processeur).
#[tokio::test(flavor = "multi_thread")]
async fn cuda_runs_the_model_and_falls_back_to_vulkan() {
    let (bench, runtime) = cuda_bench(b"engine");
    let id = bench.install("chatonlm-simple", GIB);
    let lease = bench.engine.ensure(&id, kept()).await.unwrap();
    let args: Vec<String> = serde_json::from_value(pilot(&lease, "args").await).unwrap();
    assert!(
        args.windows(2).any(|w| w == ["--device", "CUDA0"]),
        "{args:?}"
    );
    let env = pilot(&lease, "env").await;
    assert!(
        env[super::cuda::BACKEND_PATH_ENV]
            .as_str()
            .is_some_and(|path| path.ends_with(runtime.platform().unwrap().engine_file())),
        "{env}"
    );
    assert_eq!(env["CUDA_VISIBLE_DEVICES"], "GPU-3090");
    assert_eq!(env["GGML_VK_VISIBLE_DEVICES"], "none");
    assert!(!lease.cpu_fallback);
    drop(lease);
    bench.engine.shutdown().await;

    let (bench, runtime) = cuda_bench(b"FAIL");
    let id = bench.install("chatonlm-simple", GIB);
    let lease = tokio::time::timeout(Duration::from_secs(20), bench.engine.ensure(&id, kept()))
        .await
        .expect("no endless relaunch")
        .unwrap();
    // Relancé sur Vulkan : ni le moteur du pack, ni `--device`, ni le
    // processeur.
    let args: Vec<String> = serde_json::from_value(pilot(&lease, "args").await).unwrap();
    assert!(!args.iter().any(|arg| arg == "--device"), "{args:?}");
    assert!(!args.join(" ").ends_with("-ngl 0"), "{args:?}");
    let env = pilot(&lease, "env").await;
    assert!(
        env.get(super::cuda::BACKEND_PATH_ENV)
            .is_none_or(Value::is_null),
        "{env}"
    );
    assert!(!lease.cpu_fallback);
    assert!(!bench.engine.gpu_failed());
    let failure = runtime.failure().unwrap();
    assert!(failure.contains("invalid device: CUDA0"), "{failure}");
    // Le journal du premier essai (CUDA) est gardé.
    let first = std::fs::read_to_string(bench.old_log_of(&id)).unwrap();
    assert!(first.contains("ggml-cuda"), "{first}");
    drop(lease);
    bench.engine.shutdown().await;
}
