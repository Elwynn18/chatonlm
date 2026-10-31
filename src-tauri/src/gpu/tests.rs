//! Essais du chef contre le faux Ollama (modèles `chatonlm-fim*`, carte
//! simulée par `set_vram`), et contrats contre l'instance de test.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fake_ollama::{FIM_SIZE, FakeOllama, LOADED_SIZE, Mode};
use serde_json::{Value, json};

use super::*;
use crate::hub::types::HubGpu;
use crate::ollama::running::tests::{CONTRACT_MODELS, TEST_INSTANCE, Unloading, unloaded};

const GIB: u64 = 1024 * 1024 * 1024;

fn server(url: &str) -> Server {
    Server {
        url: url.to_owned(),
        context_length: 0,
        keep_alive: 0,
        cloud: false,
    }
}

fn inputs(url: &str, chat: &str) -> Inputs {
    Inputs {
        server: server(url),
        chat_model: chat.to_owned(),
        local: true,
        busy: Box::new(|| false),
        machine: None,
    }
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

/// Requêtes de génération (prompts) reçues.
fn generated(fake: &FakeOllama) -> Vec<Value> {
    fake.generate_requests()
}

/// Modèles (re)chargés par une proposition : jamais (principe n° 1).
fn loaded_by_prompt(fake: &FakeOllama) -> Vec<Value> {
    fake.load_requests()
        .into_iter()
        .filter(|load| load["kind"] == "byPrompt")
        .collect()
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

async fn loaded_names(url: &str) -> Vec<String> {
    running::list(url)
        .await
        .unwrap()
        .into_iter()
        .map(|model| model.name)
        .collect()
}

/// Principe n° 1 : une proposition ne charge jamais le modèle ; seul
/// « Préparer » le fait, avec les options figées de l'autocomplétion.
#[tokio::test]
async fn a_completion_never_loads_its_model() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let url = fake.url();
    let chief = GpuChief::default();
    let inputs = inputs(&url, "qwen3:0.6b");
    let params = fim_params("chatonlm-fim", "def f(", ")\n");
    assert_eq!(
        chief.complete(&inputs, &params).await,
        Err(Refusal::Paused(Reason::NotLoaded))
    );
    assert!(generated(&fake).is_empty(), "nothing sent to the model");
    assert!(loaded_names(&url).await.is_empty());
    assert!(fake.load_requests().is_empty());

    let plan = chief.prepare(&inputs, "chatonlm-fim").await.unwrap();
    assert!(plan.completion.as_ref().unwrap().loaded);
    assert_eq!(plan.reason, None);
    // Conversation au défaut du serveur (5 min) : 80 %, 4 min.
    let keep = GpuChief::fixed(&inputs, "chatonlm-fim").keep_alive;
    if std::env::var_os("OLLAMA_KEEP_ALIVE").is_none() {
        assert_eq!(keep, 240);
    }
    assert_eq!(
        fake.load_requests(),
        [json!({ "model": "chatonlm-fim", "kind": "load", "keep_alive": keep, "num_ctx": 4096 })]
    );
    let done = chief.complete(&inputs, &params).await.unwrap();
    // Préfixe de 6 caractères, suffixe de 2 ; arrêt à la ligne vide.
    assert_eq!(done.text, "value(6|2)\n    second line");
    assert!(!done.timed_out);
    let sent = generated(&fake);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["suffix"], ")\n");
    assert_eq!(sent[0]["think"], false);
    assert_eq!(sent[0]["options"]["num_ctx"], 4096);
    assert_eq!(sent[0]["keep_alive"], keep);
    assert_eq!(sent[0]["options"]["num_predict"], fim::MULTILINE_TOKENS);
    // Une seule ligne : 16 jetons, arrêt au saut de ligne.
    let line = fim::FimParams {
        multiline: Some(false),
        ..fim_params("chatonlm-fim", "def f(", ")\n")
    };
    assert_eq!(
        chief.complete(&inputs, &line).await.unwrap().text,
        "value(6|2)"
    );
    // Options figées : même contexte et même durée à chaque requête.
    let sent = generated(&fake);
    assert_eq!(sent[1]["options"]["num_ctx"], sent[0]["options"]["num_ctx"]);
    assert_eq!(sent[1]["keep_alive"], sent[0]["keep_alive"]);
    assert_eq!(sent[1]["options"]["num_predict"], fim::LINE_TOKENS);
    // Limite de jetons atteinte en pleine ligne (`done_reason: "length"`) :
    // la ligne inachevée part. 4 jetons = 16 caractères :
    // « value(6|2)\n    s ».
    let short = fim::FimParams {
        max_tokens: Some(4),
        ..fim_params("chatonlm-fim", "def f(", ")\n")
    };
    assert_eq!(
        chief.complete(&inputs, &short).await.unwrap().text,
        "value(6|2)"
    );
    // Cache de préfixe : le même début est repris.
    assert!(
        chief
            .complete(&inputs, &params)
            .await
            .unwrap()
            .cached_tokens
            > 0
    );
    assert!(loaded_by_prompt(&fake).is_empty());
}

/// Rapport 92, M3 : l'autocomplétion quitte la mémoire avant la
/// conversation, quel que soit son réglage (0 : défaut du serveur).
#[test]
fn completion_leaves_memory_before_the_chat() {
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
    // `OLLAMA_KEEP_ALIVE` (forme d'Ollama : secondes ou durée de Go).
    for (env, expected) in [
        (None, 300),
        (Some("10m"), 600),
        (Some("90"), 90),
        (Some("1h30m"), 5400),
        (Some("-1"), -1),
        (Some("-1s"), -1),
        (Some("1m30s"), 90),
        (Some("bad"), 300),
        (Some(""), 300),
    ] {
        assert_eq!(server_keep_alive(env), expected, "{env:?}");
    }
    assert_eq!(keep_alive_beside(0, 60), 48);
    assert_eq!(keep_alive_beside(0, -1), 600);
    assert_eq!(keep_alive_beside(1, 300), 1, "never 0 (nothing sent)");
    // Toujours plus court que la conversation, sauf gardée sans fin.
    for chat in [2, 30, 60, 300, 900, 1800, 3600] {
        assert!(keep_alive_beside(chat, 300) < chat, "{chat}");
    }
}

/// Rapport 92, M2 : chargé avec un autre contexte (bouton Load, autre
/// client), le modèle ne sert pas : une proposition le rechargerait.
/// « Préparer » le recharge avec les options de l'autocomplétion.
#[tokio::test]
async fn a_model_loaded_with_another_context_is_not_used() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let url = fake.url();
    let chief = GpuChief::default();
    let inputs = inputs(&url, "qwen3:0.6b");
    running::load(&url, "chatonlm-fim", 8192, 0).await.unwrap();
    let params = fim_params("chatonlm-fim", "def f(", ")\n");
    assert_eq!(
        chief.complete(&inputs, &params).await,
        Err(Refusal::Paused(Reason::NotLoaded))
    );
    assert_eq!(
        chief.measure(&inputs, "chatonlm-fim").await,
        Err(Refusal::Paused(Reason::NotLoaded))
    );
    assert!(generated(&fake).is_empty(), "nothing sent");
    let plan = chief.plan(&inputs).await;
    assert!(plan.completion.as_ref().unwrap().loaded);
    assert_eq!(plan.reason, Some(Reason::NotLoaded));
    let loads = fake.load_requests().len();
    chief.prepare(&inputs, "chatonlm-fim").await.unwrap();
    let reload = &fake.load_requests()[loads];
    assert_eq!(reload["num_ctx"], 4096, "{reload}");
    assert!(chief.complete(&inputs, &params).await.is_ok());
    assert!(loaded_by_prompt(&fake).is_empty());
}

/// Rapport 92, M9 : une réponse (ou un autre travail) qui commence arrête
/// « Préparer » : chargement lâché, rien en mémoire.
#[tokio::test(flavor = "multi_thread")]
async fn a_reply_stops_prepare() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let url = fake.url();
    let chief = Arc::new(GpuChief::default());
    let busy = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&busy);
    let mut inputs = inputs(&url, "qwen3:0.6b");
    inputs.busy = Box::new(move || flag.load(Ordering::SeqCst));
    let preparing = {
        let chief = Arc::clone(&chief);
        tokio::spawn(async move { chief.prepare(&inputs, "chatonlm-slowload").await })
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while fake.load_requests().is_empty() {
        assert!(Instant::now() < deadline, "no load asked");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // La réponse commence pendant le chargement.
    let started = Instant::now();
    busy.store(true, Ordering::SeqCst);
    assert_eq!(
        preparing.await.unwrap(),
        Err(Refusal::Paused(Reason::ChatBusy))
    );
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while fake.disconnects() == 0 {
        assert!(Instant::now() < deadline, "the load was not dropped");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(fake_ollama::SLOW_LOAD).await;
    assert!(loaded_names(&url).await.is_empty());
}

/// Rapport 92, M7 : sans moyen de prévoir (machine illisible, taille
/// inconnue), « Préparer » ne charge pas à côté de la conversation.
#[tokio::test]
async fn an_unknown_plan_never_loads_beside_the_chat() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let url = fake.url();
    let chief = GpuChief::default();
    let inputs = inputs(&url, "qwen3:0.6b");
    running::load(&url, "qwen3:0.6b", 0, 0).await.unwrap();
    let loads = fake.load_requests().len();
    assert_eq!(
        chief.prepare(&inputs, "chatonlm-fim").await,
        Err(Refusal::Paused(Reason::Exclusive))
    );
    assert_eq!(fake.load_requests().len(), loads);
    let plan = chief.plan(&inputs).await;
    assert_eq!(plan.mode, plan::Mode::Unknown);
    assert_eq!(plan.reason, Some(Reason::Exclusive));
}

/// Rapport 92, m2 : suffixe vide, famille connue : `raw` avec ses jetons
/// (Ollama n'appliquerait pas le gabarit FIM) ; sinon `suffix`.
#[tokio::test]
async fn an_empty_suffix_is_asked_raw() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let url = fake.url();
    let chief = GpuChief::default();
    let inputs = inputs(&url, "qwen3:0.6b");
    let model = "chatonlm-qwen-coder-fim";
    chief.prepare(&inputs, model).await.unwrap();
    let done = chief
        .complete(&inputs, &fim_params(model, "ab", ""))
        .await
        .unwrap();
    assert_eq!(done.text, "value(raw 2|0)\n    second line");
    let done = chief
        .complete(&inputs, &fim_params(model, "ab", "c"))
        .await
        .unwrap();
    assert_eq!(done.text, "value(2|1)\n    second line");
    let sent = generated(&fake);
    assert_eq!(sent[0]["raw"], true);
    assert!(sent[0].get("suffix").is_none());
    assert_eq!(sent[1]["suffix"], "c");
    assert!(sent[1].get("raw").is_none());
}

/// Rapport 92, m3 : préfixe ramené au contexte du modèle chargé.
#[tokio::test]
async fn the_prefix_is_bounded_by_the_context() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let url = fake.url();
    let chief = GpuChief::default();
    let inputs = inputs(&url, "qwen3:0.6b");
    chief.prepare(&inputs, "chatonlm-fim").await.unwrap();
    let prefix: String = (0..3000).map(|line| format!("x = {line:05}\n")).collect();
    chief
        .complete(&inputs, &fim_params("chatonlm-fim", &prefix, ""))
        .await
        .unwrap();
    let sent = generated(&fake);
    let prompt = sent[0]["prompt"].as_str().unwrap();
    let budget = fim::budget(COMPLETION_CONTEXT, fim::MULTILINE_TOKENS);
    assert!(
        prompt.chars().count() <= budget,
        "{}",
        prompt.chars().count()
    );
    assert!(prompt.ends_with("x = 02999\n"));
    assert!(prompt.starts_with("x = "));
}

/// Même modèle pour la conversation et l'autocomplétion : les options de
/// la conversation (sinon chaque changement le rechargerait).
#[tokio::test]
async fn the_chat_model_keeps_the_chat_options() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let url = fake.url();
    let chief = GpuChief::default();
    let mut inputs = inputs(&url, "chatonlm-fim");
    inputs.server.context_length = 8192;
    inputs.server.keep_alive = -1;
    chief.prepare(&inputs, "chatonlm-fim").await.unwrap();
    assert_eq!(
        fake.load_requests(),
        [json!({ "model": "chatonlm-fim", "kind": "load", "keep_alive": -1, "num_ctx": 8192 })]
    );
    chief
        .complete(&inputs, &fim_params("chatonlm-fim", "x", ""))
        .await
        .unwrap();
    let sent = generated(&fake);
    assert_eq!(sent[0]["options"]["num_ctx"], 8192);
    assert_eq!(sent[0]["keep_alive"], -1);
    // Contexte du serveur (0) : celui du modèle chargé, demandé tel quel
    // (rien d'envoyé, Ollama rechargerait un modèle chargé avec un contexte
    // explicite).
    let mut default = self::inputs(&url, "chatonlm-fim");
    default.server.context_length = 0;
    chief
        .complete(&default, &fim_params("chatonlm-fim", "x", ""))
        .await
        .unwrap();
    assert_eq!(generated(&fake)[1]["options"]["num_ctx"], 8192);
    assert!(loaded_by_prompt(&fake).is_empty());
}

/// Principe n° 5 : pendant une réponse, rien ne part.
#[tokio::test]
async fn a_reply_pauses_completion() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let url = fake.url();
    let chief = GpuChief::default();
    chief
        .prepare(&inputs(&url, "qwen3:0.6b"), "chatonlm-fim")
        .await
        .unwrap();
    let busy = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&busy);
    let mut replying = inputs(&url, "qwen3:0.6b");
    replying.busy = Box::new(move || flag.load(Ordering::SeqCst));
    let params = fim_params("chatonlm-fim", "a", "");
    assert_eq!(
        chief.complete(&replying, &params).await,
        Err(Refusal::Paused(Reason::ChatBusy))
    );
    assert_eq!(
        chief.measure(&replying, "chatonlm-fim").await,
        Err(Refusal::Paused(Reason::ChatBusy))
    );
    // Préparer non plus.
    let loads = fake.load_requests().len();
    assert_eq!(
        chief.prepare(&replying, "chatonlm-fim").await,
        Err(Refusal::Paused(Reason::ChatBusy))
    );
    assert_eq!(fake.load_requests().len(), loads);
    assert!(generated(&fake).is_empty());
    let plan = chief.plan(&replying).await;
    assert!(plan.chat_busy);
    assert_eq!(plan.reason, Some(Reason::ChatBusy));
    busy.store(false, Ordering::SeqCst);
    assert!(chief.complete(&replying, &params).await.is_ok());
}

#[tokio::test]
async fn raw_tokens_slow_models_and_unknown_families() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let url = fake.url();
    let chief = GpuChief::default();
    let inputs = inputs(&url, "qwen3:0.6b");
    for model in [
        "chatonlm-qwen-coder-raw",
        "chatonlm-nofim",
        "chatonlm-fim-slow",
    ] {
        chief.prepare(&inputs, model).await.unwrap();
    }
    // Sans `insert` : jetons FIM de Qwen-Coder écrits par l'app.
    let done = chief
        .complete(&inputs, &fim_params("chatonlm-qwen-coder-raw", "ab", "c"))
        .await
        .unwrap();
    assert_eq!(done.text, "value(raw 2|1)\n    second line");
    let sent = generated(&fake);
    assert_eq!(sent[0]["raw"], true);
    assert_eq!(
        sent[0]["prompt"],
        "<|fim_prefix|>ab<|fim_suffix|>c<|fim_middle|>"
    );
    assert!(sent[0].get("suffix").is_none());
    // Ni `insert`, ni famille connue : refus traduit, rien d'envoyé.
    let Err(Refusal::Refused(message)) = chief
        .complete(&inputs, &fim_params("chatonlm-nofim", "ab", "c"))
        .await
    else {
        panic!("expected a refusal");
    };
    assert!(message.contains("chatonlm-nofim"), "{message}");
    assert_eq!(generated(&fake).len(), 1);
    // Premier jeton trop tard (rapport 92, M1 : le faux modèle retient ses
    // en-têtes, comme Ollama) : abandon pendant l'envoi, texte vide.
    let started = Instant::now();
    let done = chief
        .complete(&inputs, &fim_params("chatonlm-fim-slow", "ab", "c"))
        .await
        .unwrap();
    assert!(done.timed_out);
    assert_eq!(done.text, "");
    assert!(done.first_token_ms >= fim::FIRST_TOKEN.as_millis() as u64);
    assert!(
        started.elapsed() < fake_ollama::FIM_SLOW_FIRST,
        "{:?}",
        started.elapsed()
    );
    // Requête lâchée : connexion fermée (Ollama saute une requête en file).
    let deadline = Instant::now() + Duration::from_secs(5);
    while fake.disconnects() == 0 {
        assert!(Instant::now() < deadline, "the request was not dropped");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Principe n° 6 : serveur ailleurs ou modèle du cloud : jamais ; absent :
/// jamais téléchargé.
#[tokio::test]
async fn only_local_installed_models_serve() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let url = fake.url();
    let chief = GpuChief::default();
    let mut remote = inputs(&url, "qwen3:0.6b");
    remote.local = false;
    assert_eq!(
        chief.prepare(&remote, "chatonlm-fim").await,
        Err(Refusal::Paused(Reason::NotLocal))
    );
    assert_eq!(
        chief
            .complete(&remote, &fim_params("chatonlm-fim", "a", ""))
            .await,
        Err(Refusal::Paused(Reason::NotLocal))
    );
    let local = inputs(&url, "qwen3:0.6b");
    assert_eq!(
        chief.prepare(&local, "gpt-oss:20b-cloud").await,
        Err(Refusal::Paused(Reason::NotLocal))
    );
    assert_eq!(
        chief.prepare(&local, "chatonlm-polish-remote").await,
        Err(Refusal::Paused(Reason::NotLocal))
    );
    assert_eq!(
        chief.prepare(&local, "qwen2.5-coder:1.5b-base").await,
        Err(Refusal::Paused(Reason::NotInstalled))
    );
    assert!(fake.load_requests().is_empty());
    assert!(fake.pull_requests().is_empty());
}

/// Constat sur une carte simulée : les deux tiennent, ou le modèle
/// d'autocomplétion chasse la conversation (déchargé, la conversation
/// rechargée, exclusif retenu), ou il ne tient pas seul.
#[tokio::test(flavor = "multi_thread")]
async fn a_load_is_checked_against_what_ollama_kept() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let url = fake.url();
    // Carte large : la prévision dit « ensemble » ; le constat de la carte
    // simulée (plus petite) corrige.
    let mut inputs = inputs(&url, "qwen3:0.6b");
    inputs.machine = large_card();
    // Taille apprise d'un premier chargement seul (le faux modèle n'est pas
    // dans `/api/tags`).
    let learned = || {
        let (inputs, url) = (&inputs, &url);
        async move {
            let chief = GpuChief::default();
            chief.prepare(inputs, "chatonlm-fim").await.unwrap();
            running::unload_all(url).await.unwrap();
            chief
        }
    };

    // Les deux tiennent : ensemble.
    let chief = learned().await;
    fake.set_vram(Some(LOADED_SIZE + FIM_SIZE));
    running::load(&url, "qwen3:0.6b", 0, 0).await.unwrap();
    let plan = chief.prepare(&inputs, "chatonlm-fim").await.unwrap();
    assert_eq!(plan.mode, plan::Mode::Together);
    assert!(plan.chat.unwrap().loaded);
    running::unload_all(&url).await.unwrap();

    // Trop juste : la conversation (échéance la plus proche) est chassée.
    let chief = learned().await;
    fake.set_vram(Some(LOADED_SIZE + FIM_SIZE / 2));
    running::load(&url, "qwen3:0.6b", 0, 0).await.unwrap();
    assert_eq!(
        chief.prepare(&inputs, "chatonlm-fim").await,
        Err(Refusal::Paused(Reason::Exclusive))
    );
    // Autocomplétion déchargée, conversation rechargée (en tâche de fond).
    let deadline = Instant::now() + Duration::from_secs(5);
    while loaded_names(&url).await != ["qwen3:0.6b"] {
        assert!(Instant::now() < deadline, "{:?}", loaded_names(&url).await);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let plan = chief.plan(&inputs).await;
    assert_eq!(plan.mode, plan::Mode::Exclusive);
    assert_eq!(plan.reason, Some(Reason::Exclusive));
    // Préparer de nouveau : refus sans rien charger.
    let loads = fake.load_requests().len();
    assert_eq!(
        chief.prepare(&inputs, "chatonlm-fim").await,
        Err(Refusal::Paused(Reason::Exclusive))
    );
    assert_eq!(fake.load_requests().len(), loads);
    // Conversation partie : l'autocomplétion se charge seule.
    running::unload(&url, "qwen3:0.6b").await.unwrap();
    assert!(chief.prepare(&inputs, "chatonlm-fim").await.is_ok());
    // Rapport 92, M8 : chargé seul ne prouve rien, le constat reste : la
    // conversation revenue (elle chasse l'autocomplétion), « Préparer »
    // refuse sans rien charger.
    running::load(&url, "qwen3:0.6b", 0, 0).await.unwrap();
    assert_eq!(loaded_names(&url).await, ["qwen3:0.6b"]);
    let loads = fake.load_requests().len();
    assert_eq!(
        chief.prepare(&inputs, "chatonlm-fim").await,
        Err(Refusal::Paused(Reason::Exclusive))
    );
    assert_eq!(fake.load_requests().len(), loads);
    running::unload_all(&url).await.unwrap();

    // Même seul, il déborde : ne tient pas, déchargé.
    let chief = GpuChief::default();
    let mut inputs = self::inputs(&url, "qwen3:0.6b");
    inputs.machine = None;
    fake.set_vram(Some(FIM_SIZE / 2));
    assert_eq!(
        chief.prepare(&inputs, "chatonlm-fim").await,
        Err(Refusal::Paused(Reason::DoesNotFit))
    );
    assert!(loaded_names(&url).await.is_empty());
}

#[tokio::test]
async fn a_measure_reads_ollama_durations() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let url = fake.url();
    let chief = GpuChief::default();
    let inputs = inputs(&url, "qwen3:0.6b");
    assert_eq!(
        chief.measure(&inputs, "chatonlm-fim").await,
        Err(Refusal::Paused(Reason::NotLoaded))
    );
    assert!(generated(&fake).is_empty());
    chief.prepare(&inputs, "chatonlm-fim").await.unwrap();
    let measure = chief.measure(&inputs, "chatonlm-fim").await.unwrap();
    // Chauffe + 3 mesures, sans flux, 32 jetons, sans arrêt aux lignes.
    let sent = generated(&fake);
    assert_eq!(sent.len(), 4);
    assert!(sent.iter().all(|body| body["stream"] == false));
    assert_eq!(sent[1]["options"]["num_predict"], 32);
    assert!(sent[1]["options"]["stop"].as_array().unwrap().is_empty());
    // Faux serveur : 5 ms par jeton (200/s), 1 ms par jeton de prompt hors
    // cache.
    assert_eq!(measure.tokens_per_second, 200.0);
    assert!(!measure.partial_gpu);
    assert_eq!(measure.rating, fim::rate(measure.first_token_ms, 200.0));
}

#[tokio::test]
async fn an_image_unloads_the_completion_model_first() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let url = fake.url();
    let chief = GpuChief::default();
    // Rien de connu : rien à faire.
    chief.before_image().await;
    assert!(fake.load_requests().is_empty());
    chief
        .prepare(&inputs(&url, "qwen3:0.6b"), "chatonlm-fim")
        .await
        .unwrap();
    running::load(&url, "qwen3:0.6b", 0, 0).await.unwrap();
    chief.before_image().await;
    assert_eq!(loaded_names(&url).await, ["qwen3:0.6b"]);
    // Le même modèle sert la conversation : gardé.
    chief
        .prepare(&inputs(&url, "chatonlm-fim"), "chatonlm-fim")
        .await
        .unwrap();
    chief.remember(&url, "chatonlm-fim");
    chief.before_image().await;
    assert!(
        loaded_names(&url)
            .await
            .contains(&"chatonlm-fim:latest".to_owned())
    );
}

#[tokio::test]
async fn suggestions_follow_the_machine() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let chief = GpuChief::default();
    let mut inputs = inputs(&fake.url(), "qwen3:0.6b");
    inputs.machine = Some(Machine {
        gpus: vec![HubGpu {
            name: "GPU".into(),
            vram_total: 12 * GIB,
            vram_free: Some(10 * GIB),
        }],
        ram_total: 32 * GIB,
        ram_available: 20 * GIB,
    });
    let suggested = chief.suggest(&inputs).await;
    assert_eq!(suggested.recommended, "qwen2.5-coder:7b-base");
    assert_eq!(suggested.options.len(), 4);
    assert!(suggested.options.iter().all(|option| !option.installed));
    let plan = chief.plan(&inputs).await;
    let memory = plan.memory.unwrap();
    assert!(memory.gpu);
    assert_eq!(memory.total, 12 * GIB);
    assert_eq!(memory.free, Some(10 * GIB));
}

/// Contrat : sur l'instance de test, `insert` annoncé par Qwen2.5-Coder
/// base, FIM réel par `suffix` (réponse non vide, cache de préfixe relu),
/// mesure, et `/api/ps` après le chargement des deux modèles ; l'autocomplétion
/// quitte la mémoire avant la conversation ; chargée avec un autre contexte,
/// elle ne sert pas (rien ne la recharge) jusqu'à « Préparer ».
#[tokio::test]
#[ignore = "instance Ollama de test : scripts/test-ollama.sh start"]
async fn contract_fim_and_measure_on_the_test_instance() {
    let _alone = CONTRACT_MODELS.lock().await;
    let coder = "qwen2.5-coder:0.5b-base";
    let chat = "qwen3:0.6b";
    let _unloading = Unloading(coder);
    unloaded(coder).await;
    let chief = GpuChief::default();
    let mut inputs = inputs(TEST_INSTANCE, chat);
    // Carte supposée large : la prévision dit « ensemble », le constat
    // d'`/api/ps` décide (le modèle de code est dans `/api/tags`).
    inputs.machine = large_card();
    inputs.server.keep_alive = 120;
    let params = fim_params(coder, "def fib(n):\n    ", "\n\nprint(fib(10))\n");
    // Jamais chargé par une proposition.
    assert_eq!(
        chief.complete(&inputs, &params).await,
        Err(Refusal::Paused(Reason::NotLoaded))
    );
    assert!(crate::ollama::running::tests::listed(coder).await.is_none());

    // Chargé avec un autre contexte : ne sert pas, n'est pas rechargé.
    running::load(TEST_INSTANCE, coder, 8192, 60).await.unwrap();
    assert_eq!(
        chief.complete(&inputs, &params).await,
        Err(Refusal::Paused(Reason::NotLoaded))
    );
    let listed = crate::ollama::running::tests::listed(coder).await.unwrap();
    assert_eq!(listed.context_length, Some(8192), "{listed:?}");

    running::load(TEST_INSTANCE, chat, 0, 120).await.unwrap();
    let plan = chief.prepare(&inputs, coder).await.unwrap();
    // Constat : les deux en mémoire, l'autocomplétion avec 4 096 de contexte,
    // et sortie de la mémoire avant la conversation.
    let both = running::list(TEST_INSTANCE).await.unwrap();
    let completion = both.iter().find(|m| same_model(&m.name, coder)).unwrap();
    let chatting = both.iter().find(|m| same_model(&m.name, chat));
    assert_eq!(completion.context_length, Some(COMPLETION_CONTEXT));
    let chatting = chatting.unwrap_or_else(|| panic!("{both:?}"));
    assert!(
        completion.expires_at.unwrap() < chatting.expires_at.unwrap(),
        "{both:?}"
    );
    assert!(plan.completion.as_ref().unwrap().loaded);
    eprintln!("plan with both models: {}", json!(plan));

    let done = chief.complete(&inputs, &params).await.unwrap();
    assert!(!done.timed_out, "{done:?}");
    assert!(!done.text.trim().is_empty(), "{done:?}");
    eprintln!("fim: {done:?}");
    // Même début : le cache de préfixe est repris.
    let again = chief.complete(&inputs, &params).await.unwrap();
    assert!(again.cached_tokens > 0, "{again:?}");
    // Suffixe vide : `raw` avec les jetons de Qwen-Coder, réponse non vide.
    let end = fim_params(coder, "def fib(n):\n    ", "");
    let raw = chief.complete(&inputs, &end).await.unwrap();
    assert!(!raw.timed_out && !raw.text.trim().is_empty(), "{raw:?}");
    eprintln!("fim at the end of the file: {raw:?}");
    // Toujours le même contexte : pas de rechargement.
    let after = running::list(TEST_INSTANCE).await.unwrap();
    let completion = after.iter().find(|m| same_model(&m.name, coder)).unwrap();
    assert_eq!(completion.context_length, Some(COMPLETION_CONTEXT));

    let measure = chief.measure(&inputs, coder).await.unwrap();
    assert!(measure.tokens_per_second > 0.0, "{measure:?}");
    eprintln!("measure: {}", json!(measure));
}
