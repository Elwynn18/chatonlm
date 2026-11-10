//! Essais du chef contre le faux moteur (modèles du banc `FakeBench`,
//! scénarios `chatonlm-gpu-*` de `fake-llama` et `chatonlm-fim` intégré ;
//! mémoire de la machine réglable pour le moteur), et contrat contre le
//! vrai `llama-server` (`RealBench`, rôle `Fim`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::future::BoxFuture;
use serde_json::{Value, json};

use super::*;
use crate::engine::{Engine, Models};
use crate::error::AppResult;
use crate::hub::types::HubGpu;
use crate::llm::testing::{FakeBench, RealBench, Role};
use crate::store::settings::SettingsPatch;

const GIB: u64 = 1024 * 1024 * 1024;

/// Modèle de code (jetons FIM) qui dit ce qu'il a reçu.
const CODER: &str = "test/Coder-GGUF:Q4_K_M";
/// Modèle de la conversation.
const CHAT: &str = "test/Chat-GGUF:Q4_K_M";
/// Sans jetons FIM.
const PLAIN: &str = "test/Plain-GGUF:Q4_K_M";
/// Réponse au bout de 5 s.
const LATE: &str = "test/Late-GGUF:Q4_K_M";
/// Chargé en 1,5 s.
const SLOW: &str = "test/Slow-GGUF:Q4_K_M";
/// `/infill` intégré (cache du prompt simulé).
const CACHED: &str = "test/Cached-GGUF:Q4_K_M";

/// Modèles de la base de l'essai, machine réglable (mémoire libre vue par
/// le moteur avant un lancement).
struct Machines {
    store: crate::store::Store,
    data_dir: std::path::PathBuf,
    machine: parking_lot::Mutex<Option<Machine>>,
}

impl Machines {
    fn set_free(&self, free: u64) {
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
}

impl Models for Machines {
    fn local<'a>(&'a self, id: &'a str) -> BoxFuture<'a, AppResult<Option<LocalModel>>> {
        Box::pin(crate::local_models::get(&self.store, &self.data_dir, id))
    }

    fn machine(&self) -> BoxFuture<'_, Option<Machine>> {
        self.machine_now()
    }

    fn machine_now(&self) -> BoxFuture<'_, Option<Machine>> {
        let machine = self.machine.lock().clone();
        Box::pin(async move { machine })
    }
}

/// Banc : base et modèles de `FakeBench`, moteur sur une machine réglable
/// (20 Gio libres au départ).
struct Gpu {
    bench: FakeBench,
    access: Access,
    machines: Arc<Machines>,
}

impl Gpu {
    fn new() -> Gpu {
        let bench = FakeBench::new(&[
            (CHAT, "simple"),
            (CODER, "gpu-fimlines"),
            (PLAIN, "simple"),
            (LATE, "gpu-fimwait"),
            (SLOW, "gpu-fimslowload"),
            (CACHED, "fim"),
            ("ggml-org/Qwen2.5-Coder-1.5B-Q8_0-GGUF:Q8_0", "gpu-fimlines"),
        ]);
        let data_dir = bench.dir().join("data");
        let machines = Arc::new(Machines {
            store: bench.store().clone(),
            data_dir: data_dir.clone(),
            machine: parking_lot::Mutex::new(None),
        });
        machines.set_free(20 * GIB);
        let engine = Engine::new(
            crate::engine::tests::fake_config(&data_dir),
            machines.clone(),
        );
        let access = Access::new(
            engine,
            bench.store().clone(),
            data_dir.clone(),
            Arc::new(crate::llm::remote::Unavailable),
            Arc::new(crate::relay::RelayClient::with_url(
                "http://127.0.0.1:9".into(),
                data_dir,
            )),
            bench.access.downloads().clone(),
        );
        Gpu {
            bench,
            access,
            machines,
        }
    }

    fn engine(&self) -> &Engine {
        self.access.engine()
    }

    async fn inputs(&self, chat: &str) -> Inputs {
        Inputs {
            server: Server::current(&self.access).await.unwrap(),
            chat_model: chat.to_owned(),
            local: true,
            busy: Box::new(|| false),
            machine: None,
        }
    }

    /// Modèles prêts dans le moteur, triés.
    fn loaded(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .engine()
            .running()
            .into_iter()
            .filter(|model| model.state == crate::engine::ModelState::Ready)
            .map(|model| model.id)
            .collect();
        ids.sort();
        ids
    }

    fn context_of(&self, id: &str) -> Option<u32> {
        self.engine()
            .running()
            .into_iter()
            .find(|model| model.id == id)
            .map(|model| model.context_length)
    }

    /// Corps reçus par `path` du processus de `id`, prêt avec les options
    /// figées de `inputs` (lues par un `Lease` pris aussitôt : rien n'est
    /// relancé).
    async fn received(&self, inputs: &Inputs, id: &str, path: &str) -> Vec<Value> {
        let (local, options) = resolved(&self.access, id, GpuChief::fixed(inputs, id))
            .await
            .unwrap();
        assert!(self.engine().is_ready(&local.id, options).await, "{id}");
        let lease = self.engine().ensure(&local.id, options).await.unwrap();
        let base = lease.endpoint.base_url.clone();
        drop(lease);
        requests(&base, path).await
    }

    async fn shutdown(&self) {
        self.engine().shutdown().await;
        self.bench.shutdown().await;
    }
}

/// Corps reçus par le faux (`GET /fake/requests`) sur `path`.
async fn requests(base: &str, path: &str) -> Vec<Value> {
    let all: Value = reqwest::get(format!("{base}/fake/requests"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    all.as_array()
        .unwrap()
        .iter()
        .filter(|request| request["path"] == path)
        .map(|request| request["body"].clone())
        .collect()
}

fn fim_params(model: &str, prefix: &str, suffix: &str) -> fim::FimParams {
    serde_json::from_value(json!({
        "model": model,
        "prefix": prefix,
        "suffix": suffix,
        "language": "python",
        "path": "/tmp/a.py",
    }))
    .unwrap()
}

/// Carte de 24 Gio, libre inconnu : la prévision dit « ensemble » dès que
/// les tailles sont connues.
fn large_card() -> Option<Machine> {
    Some(Machine {
        gpus: vec![HubGpu {
            name: "GPU".into(),
            vram_total: 24 * GIB,
            vram_free: None,
        }],
        ram_total: 32 * GIB,
        ram_available: 20 * GIB,
    })
}

/// Attend que `check` soit vrai (5 s au plus).
async fn until(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Principe n° 1 : une proposition ne charge jamais le modèle ; seul
/// « Préparer » le fait, avec les options figées de l'autocomplétion ;
/// `/infill` reçoit le code, les arrêts et la limite de jetons.
#[tokio::test(flavor = "multi_thread")]
async fn a_completion_never_loads_its_model() {
    let gpu = Gpu::new();
    let chief = GpuChief::default();
    let inputs = gpu.inputs(CHAT).await;
    let params = fim_params(CODER, "def f(", ")\n");
    assert_eq!(
        chief.complete(&inputs, &params).await,
        Err(Refusal::Paused(Reason::NotLoaded))
    );
    assert!(gpu.engine().running().is_empty(), "nothing loaded");

    let plan = chief.prepare(&inputs, CODER).await.unwrap();
    assert!(plan.completion.as_ref().unwrap().loaded);
    assert_eq!(plan.reason, None);
    assert_eq!(gpu.context_of(CODER), Some(COMPLETION_CONTEXT));
    // Conversation au défaut du moteur (5 min) : 80 %, 4 min.
    assert_eq!(GpuChief::fixed(&inputs, CODER).keep_alive, 240);
    let expires = gpu
        .engine()
        .running()
        .into_iter()
        .find(|model| model.id == CODER)
        .and_then(|model| model.expires_at)
        .unwrap();
    let in_four_minutes = crate::store::now_ms() + 240_000;
    assert!((expires - in_four_minutes).abs() < 10_000, "{expires}");
    let pid = gpu.engine().pid_of(CODER).unwrap();

    let done = chief.complete(&inputs, &params).await.unwrap();
    // Préfixe de 6 caractères, suffixe de 2 ; arrêt à la ligne vide.
    assert_eq!(done.text, "value(6|2)\n    second line");
    assert!(!done.timed_out);
    // Une seule ligne : 16 jetons, arrêt au saut de ligne.
    let line = fim::FimParams {
        multiline: Some(false),
        ..fim_params(CODER, "def f(", ")\n")
    };
    assert_eq!(
        chief.complete(&inputs, &line).await.unwrap().text,
        "value(6|2)"
    );
    // Limite de jetons atteinte en pleine ligne (fin `limit`) : la ligne
    // inachevée part. 4 morceaux = 16 caractères : « value(6|2)\n    s ».
    let short = fim::FimParams {
        max_tokens: Some(4),
        ..fim_params(CODER, "def f(", ")\n")
    };
    assert_eq!(
        chief.complete(&inputs, &short).await.unwrap().text,
        "value(6|2)"
    );
    let sent = gpu.received(&inputs, CODER, "/infill").await;
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0]["input_prefix"], "def f(");
    assert_eq!(sent[0]["input_suffix"], ")\n");
    assert_eq!(sent[0]["n_predict"], fim::MULTILINE_TOKENS);
    assert_eq!(sent[0]["temperature"], 0.2f32);
    assert_eq!(sent[0]["stop"], json!(["\n\n"]));
    assert_eq!(sent[1]["n_predict"], fim::LINE_TOKENS);
    assert_eq!(sent[1]["stop"], json!(["\n"]));
    assert_eq!(sent[2]["n_predict"], 4);
    // Toujours le même processus : rien n'a été relancé.
    assert_eq!(gpu.engine().pid_of(CODER), Some(pid));
    assert_eq!(gpu.loaded(), [CODER]);
    gpu.shutdown().await;
}

/// Modèle déchargé entre la vérification « prêt » et la prise du `Lease`
/// (crochet du moteur) : refus `NotLoaded`, rien n'est relancé ni attendu
/// (relecture L5a, M1 : `ensure` le relançait).
#[tokio::test(flavor = "multi_thread")]
async fn a_model_unloaded_just_before_the_request_is_not_reloaded() {
    let gpu = Gpu::new();
    let chief = GpuChief::default();
    let inputs = gpu.inputs(CHAT).await;
    chief.prepare(&inputs, CODER).await.unwrap();
    assert_eq!(gpu.loaded(), [CODER]);
    let engine = gpu.engine().clone();
    gpu.engine().hooks().before_lend = Some(Arc::new(move || {
        let engine = engine.clone();
        Box::pin(async move { engine.unload(CODER).await })
    }));
    let refused = tokio::time::timeout(
        Duration::from_secs(5),
        chief.complete(&inputs, &fim_params(CODER, "def f(", ")\n")),
    )
    .await
    .expect("never waits for a load");
    assert_eq!(refused, Err(Refusal::Paused(Reason::NotLoaded)));
    gpu.engine().hooks().before_lend = None;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(gpu.engine().running().is_empty(), "nothing launched again");
    gpu.shutdown().await;
}

/// Cache du prompt du serveur : le même début est repris (`cache_n`).
#[tokio::test(flavor = "multi_thread")]
async fn the_cached_prompt_is_reported() {
    let gpu = Gpu::new();
    let chief = GpuChief::default();
    let inputs = gpu.inputs(CHAT).await;
    chief.prepare(&inputs, CACHED).await.unwrap();
    let params = fim_params(CACHED, "def add(a, b):\n    ", "\n");
    let first = chief.complete(&inputs, &params).await.unwrap();
    assert_eq!(first.text, fake_llama::scenario::FIM_COMPLETION);
    let again = chief.complete(&inputs, &params).await.unwrap();
    assert!(again.cached_tokens > 0, "{again:?}");
    gpu.shutdown().await;
}

/// Rapport 92, M3 : l'autocomplétion quitte la mémoire avant la
/// conversation, quel que soit son réglage (0 : défaut du moteur).
#[test]
fn completion_leaves_memory_before_the_chat() {
    assert_eq!(engine_keep_alive(), 300);
    for (chat, expected) in [
        (0, 240),
        (60, 48),
        (300, 240),
        (900, 600),
        (14_400, 600),
        (-1, 600),
    ] {
        assert_eq!(keep_alive_beside(chat, 300), expected, "{chat}");
    }
    assert_eq!(keep_alive_beside(0, 60), 48);
    assert_eq!(keep_alive_beside(0, -1), 600);
    assert_eq!(keep_alive_beside(1, 300), 1, "never 0 (engine default)");
    // Toujours plus court que la conversation, sauf gardée sans fin.
    for chat in [2, 30, 60, 300, 900, 1800, 3600] {
        assert!(keep_alive_beside(chat, 300) < chat, "{chat}");
    }
}

/// Rapport 92, M2 : chargé avec un autre contexte (bouton Load), le modèle
/// ne sert pas : une proposition le relancerait. « Préparer » le relance
/// avec les options de l'autocomplétion.
#[tokio::test(flavor = "multi_thread")]
async fn a_model_loaded_with_another_context_is_not_used() {
    let gpu = Gpu::new();
    let chief = GpuChief::default();
    let inputs = gpu.inputs(CHAT).await;
    let other = LoadOptions {
        ctx: Some(8192),
        ..LoadOptions::default()
    };
    gpu.engine().preload(CODER, other).await.unwrap();
    let pid = gpu.engine().pid_of(CODER).unwrap();
    let params = fim_params(CODER, "def f(", ")\n");
    assert_eq!(
        chief.complete(&inputs, &params).await,
        Err(Refusal::Paused(Reason::NotLoaded))
    );
    assert_eq!(
        chief.measure(&inputs, CODER).await,
        Err(Refusal::Paused(Reason::NotLoaded))
    );
    assert_eq!(gpu.engine().pid_of(CODER), Some(pid), "not relaunched");
    let lease = gpu.engine().ensure(CODER, other).await.unwrap();
    assert!(
        requests(&lease.endpoint.base_url, "/infill")
            .await
            .is_empty()
    );
    drop(lease);
    let plan = chief.plan(&inputs).await;
    assert!(plan.completion.as_ref().unwrap().loaded);
    assert_eq!(plan.reason, Some(Reason::NotLoaded));
    chief.prepare(&inputs, CODER).await.unwrap();
    assert_eq!(gpu.context_of(CODER), Some(COMPLETION_CONTEXT));
    assert!(chief.complete(&inputs, &params).await.is_ok());
    gpu.shutdown().await;
}

/// Rapport 92, M9 : une réponse (ou un autre travail) qui commence arrête
/// « Préparer » : chargement arrêté, rien en mémoire.
#[tokio::test(flavor = "multi_thread")]
async fn a_reply_stops_prepare() {
    let gpu = Gpu::new();
    let chief = Arc::new(GpuChief::default());
    let busy = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&busy);
    let mut inputs = gpu.inputs(CHAT).await;
    inputs.busy = Box::new(move || flag.load(Ordering::SeqCst));
    let preparing = {
        let chief = Arc::clone(&chief);
        tokio::spawn(async move { chief.prepare(&inputs, SLOW).await })
    };
    until("the load started", || {
        gpu.engine().running().iter().any(|model| model.id == SLOW)
    })
    .await;
    // La réponse commence pendant le chargement.
    let started = Instant::now();
    busy.store(true, Ordering::SeqCst);
    assert_eq!(
        preparing.await.unwrap(),
        Err(Refusal::Paused(Reason::ChatBusy))
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    // Le chargement est arrêté : rien ne reste, même après sa durée.
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert!(gpu.engine().running().is_empty(), "{:?}", gpu.loaded());
    gpu.shutdown().await;
}

/// Rapport 92, M7 : sans moyen de prévoir (machine illisible), « Préparer »
/// ne charge pas à côté de la conversation.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_plan_never_loads_beside_the_chat() {
    let gpu = Gpu::new();
    let chief = GpuChief::default();
    let inputs = gpu.inputs(CHAT).await;
    crate::llm::models::load(&gpu.access, CHAT).await.unwrap();
    assert_eq!(
        chief.prepare(&inputs, CODER).await,
        Err(Refusal::Paused(Reason::Exclusive))
    );
    assert_eq!(gpu.loaded(), [CHAT]);
    let plan = chief.plan(&inputs).await;
    assert_eq!(plan.mode, plan::Mode::Unknown);
    assert_eq!(plan.reason, Some(Reason::Exclusive));
    gpu.shutdown().await;
}

/// Rapport 92, m3 : préfixe ramené au contexte du modèle chargé.
#[tokio::test(flavor = "multi_thread")]
async fn the_prefix_is_bounded_by_the_context() {
    let gpu = Gpu::new();
    let chief = GpuChief::default();
    let inputs = gpu.inputs(CHAT).await;
    chief.prepare(&inputs, CODER).await.unwrap();
    let prefix: String = (0..3000).map(|line| format!("x = {line:05}\n")).collect();
    chief
        .complete(&inputs, &fim_params(CODER, &prefix, ""))
        .await
        .unwrap();
    let sent = gpu.received(&inputs, CODER, "/infill").await;
    let prompt = sent[0]["input_prefix"].as_str().unwrap();
    let budget = fim::budget(COMPLETION_CONTEXT, fim::MULTILINE_TOKENS);
    assert!(
        prompt.chars().count() <= budget,
        "{}",
        prompt.chars().count()
    );
    assert!(prompt.ends_with("x = 02999\n"));
    assert!(prompt.starts_with("x = "));
    gpu.shutdown().await;
}

/// Même modèle pour la conversation et l'autocomplétion : les réglages de
/// la conversation (sinon chaque changement le relancerait) ; réglage
/// changé : il ne sert plus jusqu'à « Préparer » (ou au prochain message).
#[tokio::test(flavor = "multi_thread")]
async fn the_chat_model_keeps_the_chat_options() {
    let gpu = Gpu::new();
    let chief = GpuChief::default();
    gpu.bench.set(SettingsPatch {
        context_length: Some(8192),
        keep_alive: Some(-1),
        ..Default::default()
    });
    let inputs = gpu.inputs(CODER).await;
    chief.prepare(&inputs, CODER).await.unwrap();
    assert_eq!(gpu.context_of(CODER), Some(8192));
    assert!(
        gpu.engine()
            .running()
            .iter()
            .any(|model| model.id == CODER && model.kept)
    );
    // Ouvert comme la conversation l'ouvrirait : déjà prêt, aucun
    // `on_loading`.
    let session = gpu
        .access
        .open(
            CODER,
            crate::llm::OpenOptions::default(),
            &tokio_util::sync::CancellationToken::new(),
            &|| panic!("the chat would not reload it"),
        )
        .await
        .unwrap();
    drop(session);
    chief
        .complete(&inputs, &fim_params(CODER, "x", ""))
        .await
        .unwrap();
    let pid = gpu.engine().pid_of(CODER).unwrap();
    // Réglage « Default » : la conversation le relancerait avec un autre
    // contexte ; l'autocomplétion attend, sans rien relancer.
    gpu.bench.set(SettingsPatch {
        context_length: Some(0),
        ..Default::default()
    });
    let changed = gpu.inputs(CODER).await;
    assert_eq!(
        chief.complete(&changed, &fim_params(CODER, "x", "")).await,
        Err(Refusal::Paused(Reason::NotLoaded))
    );
    assert_eq!(gpu.engine().pid_of(CODER), Some(pid));
    gpu.shutdown().await;
}

/// Principe n° 5 : pendant une réponse, rien ne part.
#[tokio::test(flavor = "multi_thread")]
async fn a_reply_pauses_completion() {
    let gpu = Gpu::new();
    let chief = GpuChief::default();
    chief.prepare(&gpu.inputs(CHAT).await, CODER).await.unwrap();
    let busy = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&busy);
    let mut replying = gpu.inputs(CHAT).await;
    replying.busy = Box::new(move || flag.load(Ordering::SeqCst));
    let params = fim_params(CODER, "a", "");
    assert_eq!(
        chief.complete(&replying, &params).await,
        Err(Refusal::Paused(Reason::ChatBusy))
    );
    assert_eq!(
        chief.measure(&replying, CODER).await,
        Err(Refusal::Paused(Reason::ChatBusy))
    );
    // Préparer non plus.
    assert_eq!(
        chief.prepare(&replying, CODER).await,
        Err(Refusal::Paused(Reason::ChatBusy))
    );
    let plan = chief.plan(&replying).await;
    assert!(plan.chat_busy);
    assert_eq!(plan.reason, Some(Reason::ChatBusy));
    let quiet = gpu.inputs(CHAT).await;
    assert!(gpu.received(&quiet, CODER, "/infill").await.is_empty());
    busy.store(false, Ordering::SeqCst);
    assert!(chief.complete(&replying, &params).await.is_ok());
    gpu.shutdown().await;
}

/// Sans jetons FIM : refus traduit, ni chargé ni interrogé ; réponse trop
/// lente : abandonnée, texte vide.
#[tokio::test(flavor = "multi_thread")]
async fn models_without_fim_are_refused_and_late_answers_dropped() {
    let gpu = Gpu::new();
    let chief = GpuChief::default();
    let inputs = gpu.inputs(CHAT).await;
    let Err(Refusal::Refused(message)) = chief.prepare(&inputs, PLAIN).await else {
        panic!("expected a refusal");
    };
    assert!(message.contains(PLAIN), "{message}");
    assert!(gpu.engine().running().is_empty());
    // Chargé par ailleurs : refusé de même, rien d'envoyé.
    gpu.engine()
        .preload(
            PLAIN,
            LoadOptions {
                ctx: Some(COMPLETION_CONTEXT),
                ..LoadOptions::default()
            },
        )
        .await
        .unwrap();
    let Err(Refusal::Refused(message)) =
        chief.complete(&inputs, &fim_params(PLAIN, "ab", "c")).await
    else {
        panic!("expected a refusal");
    };
    assert!(message.contains(PLAIN), "{message}");

    chief.prepare(&inputs, LATE).await.unwrap();
    let started = Instant::now();
    let done = chief
        .complete(&inputs, &fim_params(LATE, "ab", "c"))
        .await
        .unwrap();
    assert!(done.timed_out);
    assert_eq!(done.text, "");
    let waited = fim::deadline(fim::MULTILINE_TOKENS);
    assert!(done.first_token_ms >= waited.as_millis() as u64, "{done:?}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    gpu.shutdown().await;
}

/// Principe n° 6 : moteur ailleurs ou modèle distant : jamais ; absent :
/// jamais téléchargé.
#[tokio::test(flavor = "multi_thread")]
async fn only_local_installed_models_serve() {
    let gpu = Gpu::new();
    let chief = GpuChief::default();
    let mut remote = gpu.inputs(CHAT).await;
    remote.local = false;
    assert_eq!(
        chief.prepare(&remote, CODER).await,
        Err(Refusal::Paused(Reason::NotLocal))
    );
    assert_eq!(
        chief.complete(&remote, &fim_params(CODER, "a", "")).await,
        Err(Refusal::Paused(Reason::NotLocal))
    );
    let local = gpu.inputs(CHAT).await;
    assert_eq!(
        chief
            .prepare(&local, "@remote/mistral/codestral-latest")
            .await,
        Err(Refusal::Paused(Reason::NotLocal))
    );
    assert_eq!(
        chief.prepare(&local, "test/Absent-GGUF:Q4_K_M").await,
        Err(Refusal::Paused(Reason::NotInstalled))
    );
    assert!(gpu.engine().running().is_empty());
    assert!(gpu.access.downloads().list().is_empty());
    gpu.shutdown().await;
}

/// Constat sur la mémoire vue par le moteur : les deux tiennent, ou le
/// modèle d'autocomplétion chasse la conversation (déchargé, la
/// conversation rechargée, exclusif retenu).
#[tokio::test(flavor = "multi_thread")]
async fn a_load_is_checked_against_what_the_engine_kept() {
    let gpu = Gpu::new();
    let mut inputs = gpu.inputs(CHAT).await;
    // Carte large : la prévision dit « ensemble » ; le constat du moteur
    // corrige.
    inputs.machine = large_card();

    // Les deux tiennent : ensemble.
    let chief = GpuChief::default();
    crate::llm::models::load(&gpu.access, CHAT).await.unwrap();
    let plan = chief.prepare(&inputs, CODER).await.unwrap();
    assert_eq!(plan.mode, plan::Mode::Together);
    assert!(plan.chat.unwrap().loaded);
    assert_eq!(gpu.loaded(), [CHAT, CODER]);
    gpu.engine().unload_all().await;

    // Plus de place : la conversation (inactive) est chassée.
    let chief = GpuChief::default();
    gpu.machines.set_free(0);
    crate::llm::models::load(&gpu.access, CHAT).await.unwrap();
    assert_eq!(
        chief.prepare(&inputs, CODER).await,
        Err(Refusal::Paused(Reason::Exclusive))
    );
    // Autocomplétion déchargée, conversation rechargée (en tâche de fond).
    until("the chat model back alone", || gpu.loaded() == [CHAT]).await;
    let plan = chief.plan(&inputs).await;
    assert_eq!(plan.mode, plan::Mode::Exclusive);
    assert_eq!(plan.reason, Some(Reason::Exclusive));
    // Préparer de nouveau : refus sans rien charger.
    let pid = gpu.engine().pid_of(CHAT);
    assert_eq!(
        chief.prepare(&inputs, CODER).await,
        Err(Refusal::Paused(Reason::Exclusive))
    );
    assert_eq!(gpu.loaded(), [CHAT]);
    assert_eq!(gpu.engine().pid_of(CHAT), pid);
    // Conversation partie : l'autocomplétion se charge seule.
    gpu.engine().unload(CHAT).await;
    assert!(chief.prepare(&inputs, CODER).await.is_ok());
    assert_eq!(gpu.loaded(), [CODER]);
    gpu.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_measure_reads_the_server_timings() {
    let gpu = Gpu::new();
    let chief = GpuChief::default();
    let inputs = gpu.inputs(CHAT).await;
    assert_eq!(
        chief.measure(&inputs, CODER).await,
        Err(Refusal::Paused(Reason::NotLoaded))
    );
    chief.prepare(&inputs, CODER).await.unwrap();
    let measure = chief.measure(&inputs, CODER).await.unwrap();
    // Chauffe + 3 mesures, 32 jetons, sans arrêt aux lignes.
    let sent = gpu.received(&inputs, CODER, "/infill").await;
    assert_eq!(sent.len(), 4);
    assert_eq!(sent[1]["n_predict"], 32);
    assert!(sent[1].get("stop").is_none(), "{}", sent[1]);
    assert_ne!(sent[1]["input_prefix"], sent[2]["input_prefix"]);
    // Faux serveur : 2 ms par jeton produit (500/s).
    assert_eq!(measure.tokens_per_second, 500.0);
    assert!(!measure.partial_gpu);
    assert_eq!(measure.rating, fim::rate(measure.first_token_ms, 500.0));
    gpu.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_image_unloads_the_completion_model_first() {
    let gpu = Gpu::new();
    let chief = GpuChief::default();
    // Rien de connu : rien à faire.
    chief.before_image().await;
    chief.prepare(&gpu.inputs(CHAT).await, CODER).await.unwrap();
    crate::llm::models::load(&gpu.access, CHAT).await.unwrap();
    chief.before_image().await;
    assert_eq!(gpu.loaded(), [CHAT]);
    // Le même modèle sert la conversation : gardé.
    chief
        .prepare(&gpu.inputs(CODER).await, CODER)
        .await
        .unwrap();
    chief.remember(&gpu.access, CODER);
    chief.before_image().await;
    assert!(gpu.loaded().contains(&CODER.to_owned()));
    gpu.engine().unload_all().await;
    // Sans plan encore fait : sur le moteur attaché.
    let fresh = GpuChief::default();
    fresh.attach_engine(gpu.engine().clone());
    fresh.set_completion(CODER);
    gpu.engine()
        .preload(
            CODER,
            LoadOptions {
                ctx: Some(COMPLETION_CONTEXT),
                ..LoadOptions::default()
            },
        )
        .await
        .unwrap();
    fresh.before_image().await;
    assert!(gpu.loaded().is_empty());
    gpu.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn suggestions_follow_the_machine() {
    let gpu = Gpu::new();
    let chief = GpuChief::default();
    let mut inputs = gpu.inputs(CHAT).await;
    inputs.machine = Some(Machine {
        gpus: vec![HubGpu {
            name: "GPU".into(),
            vram_total: 12 * GIB,
            vram_free: Some(10 * GIB),
        }],
        ram_total: 32 * GIB,
        ram_available: 20 * GIB,
    });
    let suggested = chief.suggest(&inputs, &[]).await;
    assert_eq!(
        suggested.recommended,
        "ggml-org/Qwen2.5-Coder-1.5B-Q8_0-GGUF:Q8_0"
    );
    assert_eq!(suggested.options.len(), 4);
    // Le modèle de la base locale compte installé.
    let installed: Vec<&str> = suggested
        .options
        .iter()
        .filter(|option| option.installed)
        .map(|option| option.model.as_str())
        .collect();
    assert_eq!(installed, ["ggml-org/Qwen2.5-Coder-1.5B-Q8_0-GGUF:Q8_0"]);
    // Et ceux que donne le pont.
    let more = ["ggml-org/Qwen2.5-Coder-0.5B-Q8_0-GGUF:Q8_0".to_owned()];
    let suggested = chief.suggest(&inputs, &more).await;
    let mut both: Vec<&str> = suggested
        .options
        .iter()
        .filter(|option| option.installed)
        .map(|option| option.model.as_str())
        .collect();
    both.sort_unstable();
    assert_eq!(
        both,
        [
            "ggml-org/Qwen2.5-Coder-0.5B-Q8_0-GGUF:Q8_0",
            "ggml-org/Qwen2.5-Coder-1.5B-Q8_0-GGUF:Q8_0"
        ]
    );
    let plan = chief.plan(&inputs).await;
    let memory = plan.memory.unwrap();
    assert!(memory.gpu);
    assert_eq!(memory.total, 12 * GIB);
    assert_eq!(memory.free, Some(10 * GIB));
    gpu.shutdown().await;
}

/// Lot L3 : les modèles prêts du moteur deviennent les constats du plan.
#[test]
fn engine_models_become_the_facts_of_the_plan() {
    use crate::engine::{ModelState, RunningModel as EngineModel};
    let model = |id: &str, state| EngineModel {
        id: id.into(),
        state,
        size_bytes: 3 * GIB,
        vram_bytes: 2 * GIB,
        context_length: 4096,
        expires_at: Some(1_000),
        kept: false,
        in_use: true,
        gpu_layers: Some(20),
        total_layers: Some(29),
        cpu_fallback: false,
        notice: None,
        cpu_only: false,
        params_label: Some("1.5B".into()),
    };
    let facts = engine_facts(&[
        model("Qwen/Qwen2.5-Coder-1.5B-GGUF:Q8_0", ModelState::Ready),
        model("a/loading-GGUF:Q4_K_M", ModelState::Loading),
    ]);
    assert_eq!(
        facts,
        [RunningModel {
            name: "Qwen/Qwen2.5-Coder-1.5B-GGUF:Q8_0".into(),
            size: 3 * GIB,
            size_vram: 2 * GIB,
            context_length: Some(4096),
            expires_at: Some(1_000),
            kept: false,
            parameter_size: "1.5B".into(),
            quantization_level: "Q8_0".into(),
        }]
    );
    // Partiellement sur la carte : le plan le voit.
    assert!(facts[0].size_vram < facts[0].size);
    // Toutes les couches sur la carte (tampons en mémoire vive comptés dans
    // la taille) : entièrement sur la carte.
    let full = EngineModel {
        gpu_layers: Some(29),
        ..model("a/full-GGUF:Q4_K_M", ModelState::Ready)
    };
    assert_eq!(engine_facts(&[full])[0].size_vram, 3 * GIB);
    // Sur le processeur (couches inconnues, rien sur la carte) : rien.
    let cpu = EngineModel {
        vram_bytes: 0,
        gpu_layers: None,
        cpu_only: true,
        ..model("a/cpu-GGUF:Q4_K_M", ModelState::Ready)
    };
    assert_eq!(engine_facts(&[cpu])[0].size_vram, 0);
}

/// Contrat : sur le vrai `llama-server` et Qwen2.5-Coder 0.5B (rôle
/// `Fim`) : jamais chargé par une proposition ; chargé avec un autre
/// contexte, il ne sert pas ; « Préparer » le charge à 4 096 ; FIM réel
/// (réponse non vide, cache du prompt relu), suffixe vide, mesure.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "vrai llama-server : CHATONLM_LLAMA_TEST_BIN et un modèle Fim"]
async fn contract_fim_and_measure_on_the_real_engine() {
    let Some(bench) = RealBench::new(Role::Fim) else {
        return;
    };
    let access = &bench.access;
    let coder = bench.id.clone();
    let chief = GpuChief::default();
    let inputs = Inputs {
        server: Server::current(access).await.unwrap(),
        chat_model: String::new(),
        local: true,
        busy: Box::new(|| false),
        machine: None,
    };
    let params = fim_params(&coder, "def fib(n):\n    ", "\n\nprint(fib(10))\n");
    assert_eq!(
        chief.complete(&inputs, &params).await,
        Err(Refusal::Paused(Reason::NotLoaded))
    );
    assert!(access.engine().running().is_empty());
    // Autre contexte : ne sert pas, n'est pas relancé.
    let other = LoadOptions {
        ctx: Some(2048),
        ..LoadOptions::default()
    };
    access.engine().preload(&coder, other).await.unwrap();
    assert_eq!(
        chief.complete(&inputs, &params).await,
        Err(Refusal::Paused(Reason::NotLoaded))
    );
    let running = access.engine().running();
    assert_eq!(running[0].context_length, 2048, "{running:?}");

    let plan = chief.prepare(&inputs, &coder).await.unwrap();
    assert!(plan.completion.as_ref().unwrap().loaded);
    let running = access.engine().running();
    assert_eq!(running[0].context_length, COMPLETION_CONTEXT, "{running:?}");
    eprintln!("plan: {}", json!(plan));

    let done = chief.complete(&inputs, &params).await.unwrap();
    assert!(!done.timed_out, "{done:?}");
    assert!(!done.text.trim().is_empty(), "{done:?}");
    eprintln!("fim: {done:?}");
    let again = chief.complete(&inputs, &params).await.unwrap();
    assert!(again.cached_tokens > 0, "{again:?}");
    let end = fim_params(&coder, "def fib(n):\n    ", "");
    let at_end = chief.complete(&inputs, &end).await.unwrap();
    assert!(
        !at_end.timed_out && !at_end.text.trim().is_empty(),
        "{at_end:?}"
    );
    eprintln!("fim at the end of the file: {at_end:?}");

    let measure = chief.measure(&inputs, &coder).await.unwrap();
    assert!(measure.tokens_per_second > 0.0, "{measure:?}");
    eprintln!("measure: {}", json!(measure));
    let running = access.engine().running();
    assert_eq!(
        running[0].context_length, COMPLETION_CONTEXT,
        "not relaunched"
    );
    bench.shutdown().await;
}
