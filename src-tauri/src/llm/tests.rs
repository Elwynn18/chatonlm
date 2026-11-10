//! Essais de l'accès aux modèles contre le faux moteur ([`FakeBench`]), et
//! contrat contre le vrai (`contract_llm_open_answers`, [`RealBench`]).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::models::{self, ChatModel, ModelOrigin};
use super::testing::{FakeBench, RealBench, Role};
use super::*;
use crate::chat::stream::Think;
use crate::engine::{EngineError, KeepAlive};
use crate::error::AppError;
use crate::inference::{Event, ResponseFormat};
use crate::local_models::import::OllamaParams;
use crate::store::settings::SettingsPatch;

const THINK: &str = "test/Think-GGUF:Q4_K_M";
const SIMPLE: &str = "test/Simple-GGUF:Q4_K_M";

fn never() -> CancellationToken {
    CancellationToken::new()
}

/// Compte les appels de `on_loading`.
#[derive(Default)]
struct Counter(AtomicUsize);

impl Counter {
    fn get(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

async fn open(bench: &FakeBench, id: &str, counter: &Counter) -> Result<Session, LlmError> {
    bench
        .access
        .open(id, OpenOptions::default(), &never(), &|| {
            counter.0.fetch_add(1, Ordering::SeqCst);
        })
        .await
}

/// Fournisseur distant d'essai : compte les listes et les ouvertures.
#[derive(Default)]
struct FakeRemote {
    lists: AtomicUsize,
    opened: Mutex<Vec<(String, String)>>,
}

impl remote::RemoteModels for FakeRemote {
    fn list(&self) -> BoxFuture<'_, Vec<ChatModel>> {
        self.lists.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            vec![ChatModel {
                id: "@remote/mistral/mistral-small-latest".into(),
                display_name: "Mistral Small".into(),
                origin: ModelOrigin::Remote,
                provider: Some("mistral".into()),
                size_bytes: None,
                params_label: None,
                quant: None,
                context_max: Some(32_768),
                capabilities: vec!["completion".into(), "tools".into()],
                installed_at: None,
            }]
        })
    }

    fn open<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
    ) -> BoxFuture<'a, Result<Session, LlmError>> {
        self.opened
            .lock()
            .unwrap()
            .push((provider.to_owned(), model.to_owned()));
        Box::pin(async move {
            Ok(Session::remote(
                ids::remote_id(provider, model),
                model.to_owned(),
                provider.to_owned(),
                Endpoint::remote("http://127.0.0.1:9", "key").unwrap(),
                Some(32_768),
                Default::default(),
            ))
        })
    }
}

/// Même base et même moteur que le banc, avec ce fournisseur distant.
fn with_remote(bench: &FakeBench, remote: Arc<FakeRemote>) -> Access {
    let data_dir = bench.dir().join("data");
    Access::new(
        bench.engine().clone(),
        bench.store().clone(),
        data_dir.clone(),
        remote,
        Arc::new(RelayClient::with_url(CLOSED_RELAY.into(), data_dir)),
        HubDownloads::with_options(|_| {}, Default::default()),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn the_picker_lists_local_models_newest_first_then_remote_ones_if_allowed() {
    let bench = FakeBench::new(&[(SIMPLE, "simple"), (THINK, "think")]);
    let remote = Arc::new(FakeRemote::default());
    let access = with_remote(&bench, remote.clone());
    let listed = models::list(&access).await.unwrap();
    let ids: Vec<_> = listed.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(ids, [THINK, SIMPLE]);
    assert_eq!(listed[0].origin, ModelOrigin::Hf);
    assert_eq!(listed[0].capabilities, ["completion", "thinking"]);
    assert_eq!(listed[0].display_name, "Think");
    assert_eq!(listed[0].quant.as_deref(), Some("Q4_K_M"));
    // Distants coupés (défaut) : le fournisseur n'est même pas consulté.
    assert_eq!(remote.lists.load(Ordering::SeqCst), 0);
    let off = "@remote/mistral/mistral-small-latest";
    assert_eq!(models::get(&access, off).await.unwrap(), None);
    assert!(models::details(&access, off).await.is_err());
    assert_eq!(remote.lists.load(Ordering::SeqCst), 0);
    let refused = access
        .open(off, OpenOptions::default(), &never(), &|| {
            panic!("no loading for a remote model")
        })
        .await
        .unwrap_err();
    assert_eq!(refused, LlmError::RemoteOff);
    assert!(remote.opened.lock().unwrap().is_empty());
    // Permis : après les locaux.
    bench.set(SettingsPatch {
        remote_models: Some(true),
        ..Default::default()
    });
    let ids: Vec<_> = models::list(&access)
        .await
        .unwrap()
        .into_iter()
        .map(|model| model.id)
        .collect();
    assert_eq!(ids, [THINK, SIMPLE, off]);
    let session = access
        .open(off, OpenOptions::default(), &never(), &|| {
            panic!("no loading for a remote model")
        })
        .await
        .unwrap();
    assert!(session.remote && !session.unloaded());
    assert_eq!(
        *remote.opened.lock().unwrap(),
        [("mistral".to_owned(), "mistral-small-latest".to_owned())]
    );
    // Le nom du fournisseur part dans la requête.
    let request = session.request(vec![Message::user("hi")]);
    assert_eq!(request.body(false)["model"], "mistral-small-latest");
    bench.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn details_come_from_the_file_and_a_missing_model_is_not_found() {
    let bench = FakeBench::new(&[(SIMPLE, "simple"), (THINK, "think")]);
    let access = &bench.access;
    let details = models::details(access, THINK).await.unwrap();
    assert_eq!(details.model, THINK);
    assert_eq!(details.capabilities, ["completion", "thinking"]);
    assert_eq!(
        details.thinking.map(|thinking| thinking.values),
        Some(vec![Think::Enabled(true), Think::Enabled(false)])
    );
    assert!(!details.always_thinks);
    bench
        .store()
        .blocking(|c| crate::store::model_prefs::set_always_thinks(c, THINK))
        .unwrap();
    assert!(models::details(access, THINK).await.unwrap().always_thinks);
    // Sans réflexion : rien.
    assert_eq!(
        models::details(access, SIMPLE).await.unwrap().thinking,
        None
    );
    let missing = models::details(access, "test/Nope-GGUF:Q4_K_M").await;
    assert!(matches!(missing, Err(AppError::NotFound(_))), "{missing:?}");
    assert_eq!(
        models::get(access, "test/Nope-GGUF:Q4_K_M").await.unwrap(),
        None
    );
    assert_eq!(
        models::get(access, "test/simple-GGUF:q4_k_m")
            .await
            .unwrap(),
        None,
        "le dépôt garde sa casse"
    );
    assert_eq!(
        models::get(access, "test/Simple-GGUF:q4_k_m")
            .await
            .unwrap()
            .map(|model| model.id),
        Some(SIMPLE.to_owned())
    );
}

#[test]
fn gpt_oss_thinks_in_levels() {
    let levels = models::thinking_options(false, Some("gpt-oss")).unwrap();
    assert_eq!(
        levels.values,
        ["low", "medium", "high"].map(|level| Think::Level(level.into()))
    );
    assert_eq!(levels.default, Some(Think::Level("medium".into())));
    assert_eq!(
        models::thinking_options(true, Some("GPT-OSS"))
            .unwrap()
            .values
            .len(),
        3
    );
    let plain = models::thinking_options(true, Some("qwen3")).unwrap();
    assert_eq!(plain.values, [Think::Enabled(true), Think::Enabled(false)]);
    assert_eq!(models::thinking_options(false, Some("qwen3")), None);
    assert_eq!(models::thinking_options(false, None), None);
}

/// Sans moteur installé : aucun chargement annoncé, et l'ouverture dit que le
/// moteur manque.
#[tokio::test]
async fn nothing_is_announced_without_an_engine() {
    let bench = FakeBench::with_config(&[(SIMPLE, "simple")], |config| config.bin = None);
    let counter = Counter::default();
    let error = open(&bench, SIMPLE, &counter).await.expect_err("no engine");
    assert_eq!(counter.get(), 0, "rien d'annoncé");
    assert_eq!(
        error.chat_kind(),
        Some(crate::chat::ChatErrorKind::EngineUnavailable),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn loading_is_announced_once_and_only_when_needed() {
    let bench = FakeBench::new(&[(SIMPLE, "simple")]);
    let counter = Counter::default();
    let session = open(&bench, SIMPLE, &counter).await.unwrap();
    assert_eq!(counter.get(), 1, "chargement annoncé une fois");
    assert_eq!(session.id, SIMPLE);
    assert!(!session.remote && !session.cpu_fallback);
    // Contexte du moteur : 32 768 sur une carte de 24 Gio, borné par le
    // modèle (40 960).
    assert_eq!(session.context, Some(32_768));
    drop(session);
    // Déjà prêt avec les mêmes options : rien d'annoncé.
    let again = open(&bench, SIMPLE, &counter).await.unwrap();
    assert_eq!(counter.get(), 1);
    drop(again);
    // Autre contexte : relancé, donc annoncé.
    let other = bench
        .access
        .open(
            SIMPLE,
            OpenOptions {
                ctx: Some(2048),
                ..Default::default()
            },
            &never(),
            &|| {
                counter.0.fetch_add(1, Ordering::SeqCst);
            },
        )
        .await
        .unwrap();
    assert_eq!(counter.get(), 2);
    assert_eq!(other.context, Some(2048));
    // La session répond.
    let request = other.request(vec![Message::user("Hi")]);
    let mut stream = chat_stream(&other, &request, never()).await.unwrap();
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        if let Event::Content(piece) = event.unwrap() {
            text.push_str(&piece);
        }
    }
    assert_eq!(text, "Hello! You said: Hi");
    drop(other);
    // Absent : modèle manquant, sans annonce.
    let missing = open(&bench, "test/Nope-GGUF:Q4_K_M", &counter).await;
    assert!(matches!(missing, Err(LlmError::NotInstalled(_))));
    assert_eq!(counter.get(), 2);
    bench.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_context_setting_and_the_imported_num_ctx_open_the_model() {
    let bench = FakeBench::new(&[(SIMPLE, "simple")]);
    let defaults = crate::store::model_import::ModelDefaults {
        params: OllamaParams {
            num_ctx: Some(6144),
            temperature: Some(0.25),
            ..Default::default()
        },
        system: "Model prompt".into(),
    };
    bench
        .store()
        .blocking(move |c| crate::store::model_import::set_defaults(c, SIMPLE, &defaults))
        .unwrap();
    let counter = Counter::default();
    let session = open(&bench, SIMPLE, &counter).await.unwrap();
    assert_eq!(session.context, Some(6144), "num_ctx du modèle repris");
    let request = session.request(vec![Message::user("Hi")]);
    assert_eq!(request.temperature, Some(0.25));
    assert_eq!(request.messages[0], Message::system("Model prompt"));
    drop(session);
    // Le réglage passe avant le modèle.
    bench.set(SettingsPatch {
        context_length: Some(4096),
        ..Default::default()
    });
    let session = open(&bench, SIMPLE, &counter).await.unwrap();
    assert_eq!(session.context, Some(4096));
    drop(session);
    bench.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_open_returns_and_the_loading_goes_on() {
    let bench = FakeBench::new(&[(SIMPLE, "longload")]);
    let cancel = CancellationToken::new();
    let counter = Counter::default();
    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        stopper.cancel();
    });
    let started = std::time::Instant::now();
    let result = bench
        .access
        .open(SIMPLE, OpenOptions::default(), &cancel, &|| {
            counter.0.fetch_add(1, Ordering::SeqCst);
        })
        .await;
    assert_eq!(result.unwrap_err(), LlmError::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(counter.get(), 1);
    // Le chargement continue dans le moteur.
    assert_eq!(bench.engine().running().len(), 1);
    bench.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lease_lives_with_the_session_then_the_timer_runs() {
    let bench = FakeBench::new(&[(SIMPLE, "simple")]);
    bench.set(SettingsPatch {
        keep_alive: Some(60),
        ..Default::default()
    });
    let counter = Counter::default();
    let session = open(&bench, SIMPLE, &counter).await.unwrap();
    let running = &bench.engine().running()[0];
    assert!(running.in_use, "tenu tant que la session vit");
    // Une image ne prend pas un modèle tenu.
    assert_eq!(bench.engine().release_idle().await, 0);
    drop(session);
    let running = &bench.engine().running()[0];
    assert!(!running.in_use);
    let left = running.expires_at.unwrap() - crate::store::now_ms();
    assert!((55_000..=60_000).contains(&left), "{left}");
    // `keep_alive` imposé par l'appelant : déchargé dès la fin.
    let session = bench
        .access
        .open(
            SIMPLE,
            OpenOptions {
                keep_alive: Some(KeepAlive::Unload),
                ..Default::default()
            },
            &never(),
            &|| {},
        )
        .await
        .unwrap();
    drop(session);
    assert!(bench.engine().running().is_empty());
    bench.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unload_marks_the_session_and_cuts_are_stops() {
    let bench = FakeBench::new(&[(SIMPLE, "hang")]);
    let counter = Counter::default();
    let session = open(&bench, SIMPLE, &counter).await.unwrap();
    assert!(!session.unloaded());
    let request = session.request(vec![Message::user("Hi")]);
    let mut stream = chat_stream(&session, &request, never()).await.unwrap();
    let unloading = {
        let engine = bench.engine().clone();
        tokio::spawn(async move { engine.unload(SIMPLE).await })
    };
    let mut cut = None;
    while let Some(event) = stream.next().await {
        if let Err(error) = event {
            cut = Some(error);
            break;
        }
    }
    unloading.await.unwrap();
    assert!(session.unloaded());
    let cut = cut.expect("the stream is cut");
    // Coupure d'un modèle déchargé à la demande : un arrêt.
    assert_eq!(session.error(cut.clone()), LlmError::Cancelled, "{cut:?}");
    assert_eq!(session.error(cut).chat_kind(), None);
    bench.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_is_not_an_unload() {
    let bench = FakeBench::new(&[(SIMPLE, "crash")]);
    let counter = Counter::default();
    let session = open(&bench, SIMPLE, &counter).await.unwrap();
    let request = session.request(vec![Message::user("Hi")]);
    let mut stream = chat_stream(&session, &request, never()).await.unwrap();
    let mut cut = None;
    while let Some(event) = stream.next().await {
        if let Err(error) = event {
            cut = Some(error);
            break;
        }
    }
    let error = session.error(cut.expect("the stream is cut"));
    assert!(!session.unloaded());
    assert_eq!(
        error.chat_kind(),
        Some(crate::chat::ChatErrorKind::ConnectionLost)
    );
    bench.shutdown().await;
}

#[test]
fn remote_errors_name_their_provider() {
    let session = Session::remote(
        "@remote/openrouter/x/y".into(),
        "x/y".into(),
        "My OpenRouter".into(),
        Endpoint::remote("http://127.0.0.1:9", "k").unwrap(),
        None,
        Default::default(),
    );
    // Nom choisi par l'utilisateur, pas l'identifiant (`openrouter`).
    let provider = "My OpenRouter".to_owned();
    let http = |status| InferenceError::Http {
        status,
        message: String::new(),
    };
    assert_eq!(
        session.error(InferenceError::Unauthorized),
        LlmError::RemoteKey {
            provider: provider.clone()
        }
    );
    assert_eq!(
        session.error(http(403)),
        LlmError::RemoteKey {
            provider: provider.clone()
        }
    );
    for status in [402, 429] {
        assert_eq!(
            session.error(http(status)),
            LlmError::RemoteLimit {
                provider: provider.clone()
            }
        );
    }
    for error in [InferenceError::Connect("x".into()), InferenceError::Timeout] {
        assert_eq!(
            session.error(error),
            LlmError::RemoteUnreachable {
                provider: provider.clone()
            }
        );
    }
    assert_eq!(
        session
            .error(InferenceError::Interrupted("x".into()))
            .chat_kind(),
        Some(crate::chat::ChatErrorKind::ConnectionLost)
    );
    // Panne du fournisseur : injoignable pour le moment (jamais « le moteur
    // local a échoué ») ; requête refusée : erreur du modèle.
    assert_eq!(
        session.error(http(500)).chat_kind(),
        Some(crate::chat::ChatErrorKind::RemoteUnreachable)
    );
    assert_eq!(
        session.error(http(400)).chat_kind(),
        Some(crate::chat::ChatErrorKind::Model)
    );
    assert_eq!(
        session.error(InferenceError::Cancelled),
        LlmError::Cancelled
    );
    assert!(session.endpoint.dialect == inference::Dialect::OpenAi);
}

#[tokio::test(flavor = "multi_thread")]
async fn load_and_unload_go_through_the_engine() {
    let bench = FakeBench::new(&[(SIMPLE, "simple")]);
    let access = &bench.access;
    models::load(access, SIMPLE).await.unwrap();
    let running = models::running(access);
    assert_eq!(running.len(), 1);
    assert!(!running[0].in_use, "Lease lâché");
    // Rien à charger pour un distant.
    models::load(access, "@remote/mistral/m").await.unwrap();
    assert_eq!(models::running(access).len(), 1);
    assert!(matches!(
        models::load(access, "test/Nope-GGUF:Q4_K_M").await,
        Err(LlmError::NotInstalled(_))
    ));
    models::unload(access, SIMPLE).await;
    assert!(models::running(access).is_empty());
    models::load(access, SIMPLE).await.unwrap();
    models::unload_all(access).await;
    assert!(models::running(access).is_empty());
    // Pendant un effacement, le modèle ne se recharge pas.
    let deleting = access.engine().deleting(SIMPLE);
    assert!(matches!(
        models::load(access, SIMPLE).await,
        Err(LlmError::Engine(EngineError::NotInstalled(_)))
    ));
    drop(deleting);
    bench.shutdown().await;
}

/// Ordre des propriétés d'un schéma gardé jusque dans le corps reçu par le
/// serveur (`serde_json/preserve_order`) : la grammaire de llama-server
/// l'impose.
#[tokio::test]
async fn a_schema_keeps_its_property_order_on_the_wire() {
    let schema = json!({ "type": "object", "properties": {
        "name": { "type": "string" }, "age": { "type": "integer" } },
        "required": ["name", "age"] });
    let mut request = ChatRequest::new(vec![Message::user("Hi")]);
    request.response_format = Some(ResponseFormat::JsonSchema(schema));
    let text = serde_json::to_string(&request.body(false)).unwrap();
    assert!(
        text.find("\"name\"").unwrap() < text.find("\"age\"").unwrap(),
        "{text}"
    );
    let fake = fake_llama::FakeLlama::start(fake_llama::Config::new("m.gguf").api_key("k"))
        .await
        .unwrap();
    let endpoint = Endpoint::new(fake.url(), "k").unwrap();
    let mut ready = false;
    for _ in 0..100 {
        if inference::health(&endpoint).await == inference::Health::Ready {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ready);
    let reply = inference::chat_once(&endpoint, &request, never())
        .await
        .unwrap();
    // La valeur factice suit le schéma dans son ordre.
    assert!(reply.content.find("name").unwrap() < reply.content.find("age").unwrap());
    let received = fake.chat_requests();
    let schema = &received[0]["response_format"]["json_schema"]["schema"]["properties"];
    let keys: Vec<_> = schema.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["name", "age"]);
    fake.stop().await;
}

/// Contrat contre le vrai llama-server (`RealBench(Text)`) : la session
/// répond, et son contexte est celui du processus (`-c`, `/props`).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "vrai llama-server : CHATONLM_LLAMA_TEST_BIN, sous verrou"]
async fn contract_llm_open_answers() {
    let Some(bench) = RealBench::new(Role::Text) else {
        return;
    };
    let counter = Counter::default();
    let session = bench
        .access
        .open(
            &bench.id,
            OpenOptions {
                ctx: Some(2048),
                ..bench.options()
            },
            &never(),
            &|| {
                counter.0.fetch_add(1, Ordering::SeqCst);
            },
        )
        .await
        .unwrap();
    assert_eq!(counter.get(), 1);
    assert_eq!(session.context, Some(2048));
    let props = inference::props(&session.endpoint).await.unwrap();
    assert_eq!(props.n_ctx, 2048);
    let mut request = session.request(vec![Message::user("Say hello in one word.")]);
    request.think = inference::Think::Off;
    request.max_tokens = Some(32);
    let reply = inference::chat_once(&session.endpoint, &request, never())
        .await
        .unwrap();
    assert!(!reply.content.trim().is_empty(), "{reply:?}");
    drop(session);
    bench.shutdown().await;
    assert!(bench.access.engine().running().is_empty());
}
