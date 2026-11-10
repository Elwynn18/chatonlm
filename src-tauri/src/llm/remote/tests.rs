//! Modèles distants : fournisseurs (base, clé à part), listes, sessions,
//! dialecte, erreurs, et la clé qui ne sort jamais (page, base, journal).

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fake_llama::{Config, FakeLlama, Mode};
use futures_util::StreamExt;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use super::catalog::{self, Problem};
use super::keys::{self, Keys};
use super::providers::{self, RemoteProblem, RemoteProviderInput, normalize_url};
use super::*;
use crate::inference::{self, ChatRequest, Dialect, Endpoint, Message, ToolCall};
use crate::llm::{Access, OpenOptions};
use crate::store::Store;
use crate::store::remote::RemoteProviderKind;

/// Lignes de [`note`] (essais : la clé n'y est jamais).
pub(super) static NOTES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Clé des essais : cherchée ensuite partout où elle ne doit pas être.
const SECRET: &str = "sk-remote-SECRET-7f3a9c2e";

fn input(kind: RemoteProviderKind, base: Option<&str>, key: Option<&str>) -> RemoteProviderInput {
    RemoteProviderInput {
        id: None,
        kind,
        name: String::new(),
        base_url: base.map(str::to_owned),
        api_key: key.map(str::to_owned),
        enabled: true,
    }
}

/// Base sur un fichier (relue octet par octet) et dossier des données.
fn store(dir: &Path) -> Store {
    Store::open(&dir.join("chatonlm.sqlite")).unwrap()
}

/// Accès complet avec les modèles distants de l'app.
fn access(store: &Store, dir: &Path) -> Access {
    access_with(store, dir, for_app(store, dir))
}

/// Accès complet avec ces modèles distants.
fn access_with(store: &Store, dir: &Path, remote: Arc<dyn RemoteModels>) -> Access {
    let engine = crate::engine::Engine::new(
        crate::engine::Config::new(None, dir),
        Arc::new(crate::engine::AppModels::new(
            Some(store.clone()),
            dir.to_path_buf(),
        )),
    );
    let downloads = crate::hub::downloads::HubDownloads::with_options(|_| {}, Default::default());
    let relay = Arc::new(crate::relay::RelayClient::with_url(
        "http://127.0.0.1:9".into(),
        dir.to_path_buf(),
    ));
    Access::new(
        engine,
        store.clone(),
        dir.to_path_buf(),
        remote,
        relay,
        downloads,
    )
}

async fn allow_remote(store: &Store, on: bool) {
    store
        .call(move |c| {
            crate::store::settings::update(
                c,
                crate::store::settings::SettingsPatch {
                    remote_models: Some(on),
                    ..Default::default()
                },
                None,
            )
        })
        .await
        .unwrap();
}

/// Requêtes reçues par le faux fournisseur : chemin et clé (`Bearer`).
type Seen = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// Faux fournisseur pour les listes : `/v1/models` et `/v1/key`, chemins et
/// clés reçus notés.
struct Provider {
    addr: SocketAddr,
    seen: Seen,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Provider {
    /// `answer(path, bearer)` : statut et corps.
    async fn start(
        answer: impl Fn(&str, Option<&str>) -> (u16, String) + Send + Sync + 'static,
    ) -> Provider {
        Self::start_late(Duration::ZERO, answer).await
    }

    /// Comme [`Provider::start`], chaque réponse après `delay`.
    async fn start_late(
        delay: Duration,
        answer: impl Fn(&str, Option<&str>) -> (u16, String) + Send + Sync + 'static,
    ) -> Provider {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let answer = Arc::new(answer);
        let task = tokio::spawn({
            let seen = seen.clone();
            async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    let (answer, seen) = (answer.clone(), seen.clone());
                    tokio::spawn(async move {
                        let mut request = Vec::new();
                        let mut buffer = [0u8; 4096];
                        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                            match socket.read(&mut buffer).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => request.extend_from_slice(&buffer[..n]),
                            }
                        }
                        let text = String::from_utf8_lossy(&request).into_owned();
                        let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
                        let bearer = text.lines().find_map(|line| {
                            line.to_ascii_lowercase()
                                .starts_with("authorization: bearer ")
                                .then(|| line["authorization: bearer ".len()..].to_owned())
                        });
                        seen.lock().push((path.clone(), bearer.clone()));
                        tokio::time::sleep(delay).await;
                        let (status, body) = answer(&path, bearer.as_deref());
                        let reply = format!(
                            "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                             content-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = socket.write_all(reply.as_bytes()).await;
                    });
                }
            }
        });
        Provider { addr, seen, task }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn paths(&self) -> Vec<String> {
        self.seen
            .lock()
            .iter()
            .map(|(path, _)| path.clone())
            .collect()
    }
}

/// Liste d'OpenRouter (forme réelle, réduite).
fn openrouter_models() -> Value {
    json!({ "data": [
        { "id": "meta-llama/llama-3.3-70b-instruct:free", "name": "Meta: Llama 3.3 70B (free)",
          "context_length": 131072,
          "architecture": { "input_modalities": ["text"], "output_modalities": ["text"] },
          "supported_parameters": ["tools", "temperature"] },
        { "id": "google/gemma-3-27b-it", "name": "Google: Gemma 3 27B",
          "context_length": 96000,
          "architecture": { "input_modalities": ["text", "image"], "output_modalities": ["text"] },
          "supported_parameters": ["temperature"] },
        { "id": "openai/gpt-image", "name": "Images only",
          "architecture": { "input_modalities": ["text"], "output_modalities": ["image"] } },
        { "id": " spaced", "name": "Refused id" },
        { "name": "No id" },
        { "id": "google/gemma-3-27b-it", "name": "Duplicate" },
    ] })
}

#[test]
fn model_lists_say_what_they_know() {
    let models = catalog::parse(RemoteProviderKind::OpenRouter, &openrouter_models()).unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].name, "Meta: Llama 3.3 70B (free)");
    assert_eq!(models[0].context, Some(131_072));
    assert!(models[0].capabilities.tools && !models[0].capabilities.vision);
    assert!(models[1].capabilities.vision && !models[1].capabilities.tools);
    // Mistral : capacités annoncées, modèles qui ne conversent pas écartés.
    let mistral = json!({ "data": [
        { "id": "mistral-small-latest", "max_context_length": 32768,
          "capabilities": { "completion_chat": true, "function_calling": true, "vision": true } },
        { "id": "mistral-embed", "capabilities": { "completion_chat": false } },
        { "id": "codestral-latest", "capabilities": { "completion_chat": true, "function_calling": false } },
    ] });
    let models = catalog::parse(RemoteProviderKind::Mistral, &mistral).unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["mistral-small-latest", "codestral-latest"]);
    assert_eq!(models[0].context, Some(32_768));
    assert!(models[0].capabilities.vision && models[0].capabilities.tools);
    assert!(!models[1].capabilities.tools);
    // OpenAI, Ollama : rien d'annoncé, conversation et outils supposés ; la
    // réflexion jamais.
    let ollama = json!({ "object": "list", "data": [{ "id": "gpt-oss:120b", "object": "model" }] });
    let models = catalog::parse(RemoteProviderKind::OllamaCloud, &ollama).unwrap();
    assert_eq!(models[0].name, "gpt-oss:120b");
    assert!(models[0].capabilities.tools);
    assert!(!models[0].capabilities.thinking && !models[0].capabilities.vision);
    assert!(catalog::parse(RemoteProviderKind::Custom, &json!({ "models": [] })).is_none());
}

#[test]
fn addresses_and_hints() {
    for (given, kept) in [
        (
            "https://api.example.com/v1",
            Some("https://api.example.com"),
        ),
        (
            "https://api.example.com/v1/",
            Some("https://api.example.com"),
        ),
        (" http://192.168.1.2:1234 ", Some("http://192.168.1.2:1234")),
        (
            "https://api.groq.com/openai/v1",
            Some("https://api.groq.com/openai"),
        ),
        ("ftp://example.com", None),
        ("https://user:pw@example.com", None),
        ("https://example.com/?key=1", None),
        ("example.com", None),
        ("", None),
    ] {
        assert_eq!(normalize_url(given).as_deref(), kept, "{given}");
    }
    assert_eq!(keys::hint(SECRET).as_deref(), Some("9c2e"));
    assert_eq!(keys::hint("short"), None);
    // Adresses des fournisseurs connus, langue de Mistral.
    assert_eq!(
        providers::known_base(RemoteProviderKind::OpenRouter).as_deref(),
        Some("https://openrouter.ai/api")
    );
    assert_eq!(
        providers::known_base(RemoteProviderKind::Mistral).as_deref(),
        Some("https://api.mistral.ai")
    );
    assert_eq!(
        providers::known_base(RemoteProviderKind::OllamaCloud).as_deref(),
        Some("https://ollama.com")
    );
    assert_eq!(providers::known_base(RemoteProviderKind::Custom), None);
    assert_eq!(
        providers::dialect_of(RemoteProviderKind::Mistral),
        Dialect::Mistral
    );
    assert_eq!(
        providers::dialect_of(RemoteProviderKind::OpenRouter),
        Dialect::OpenAi
    );
}

#[tokio::test]
async fn providers_are_saved_with_their_key_apart() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let saved = providers::save(
        &store,
        dir.path(),
        input(RemoteProviderKind::Mistral, None, Some(SECRET)),
    )
    .await
    .unwrap();
    assert_eq!(saved.id, "mistral");
    assert_eq!(saved.name, "Mistral");
    assert!(saved.has_key);
    assert_eq!(saved.key_hint.as_deref(), Some("9c2e"));
    let again = providers::save(
        &store,
        dir.path(),
        input(RemoteProviderKind::Mistral, None, None),
    )
    .await
    .unwrap();
    assert_eq!(again.id, "mistral-2");
    assert!(!again.has_key);
    // Clé dans son fichier, lisible du seul utilisateur.
    let keys = Keys::new(dir.path());
    assert_eq!(keys.get("mistral").as_deref(), Some(SECRET));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(keys.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // Changement sans clé : elle est gardée ; vide : retirée ; la sorte ne
    // change pas.
    let changed = providers::save(
        &store,
        dir.path(),
        RemoteProviderInput {
            id: Some("mistral".into()),
            name: "Mine".into(),
            enabled: false,
            ..input(RemoteProviderKind::Custom, None, None)
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (changed.kind, changed.name.as_str(), changed.enabled),
        (RemoteProviderKind::Mistral, "Mine", false)
    );
    assert_eq!(keys.get("mistral").as_deref(), Some(SECRET));
    providers::save(
        &store,
        dir.path(),
        RemoteProviderInput {
            id: Some("mistral".into()),
            ..input(RemoteProviderKind::Mistral, None, Some(""))
        },
    )
    .await
    .unwrap();
    assert_eq!(keys.get("mistral"), None);
    // Refus : clé avec une espace, nom trop long, adresse d'un « custom ».
    for wrong in [
        input(RemoteProviderKind::OpenRouter, None, Some("sk with space")),
        RemoteProviderInput {
            name: "n".repeat(61),
            ..input(RemoteProviderKind::OpenRouter, None, None)
        },
        input(RemoteProviderKind::Custom, Some("ftp://x"), None),
        input(RemoteProviderKind::Custom, None, None),
    ] {
        let refused = providers::save(&store, dir.path(), wrong.clone()).await;
        assert!(
            matches!(refused, Err(crate::error::AppError::Invalid(_))),
            "{wrong:?}"
        );
    }
    // Retrait : la ligne et la clé.
    providers::save(
        &store,
        dir.path(),
        RemoteProviderInput {
            id: Some("mistral".into()),
            ..input(RemoteProviderKind::Mistral, None, Some(SECRET))
        },
    )
    .await
    .unwrap();
    providers::delete(&store, dir.path(), "mistral")
        .await
        .unwrap();
    assert_eq!(keys.get("mistral"), None);
    let ids: Vec<String> = providers::list(&store, dir.path())
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids, ["mistral-2"]);
}

#[tokio::test]
async fn lists_are_kept_then_asked_again() {
    let provider = Provider::start(|path, bearer| match (path, bearer) {
        ("/v1/models", Some(SECRET)) => (200, openrouter_models().to_string()),
        ("/v1/models", _) => (401, "{}".into()),
        _ => (404, "{}".into()),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let remote = Remote::new(store.clone(), dir.path().to_path_buf());
    providers::save(
        &store,
        dir.path(),
        RemoteProviderInput {
            name: "Box".into(),
            ..input(
                RemoteProviderKind::Custom,
                Some(&provider.url()),
                Some(SECRET),
            )
        },
    )
    .await
    .unwrap();
    let models = remote.list().await;
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "@remote/custom/meta-llama/llama-3.3-70b-instruct:free",
            "@remote/custom/google/gemma-3-27b-it"
        ]
    );
    assert_eq!(models[0].provider.as_deref(), Some("Box"));
    assert_eq!(models[0].origin, crate::llm::models::ModelOrigin::Remote);
    assert_eq!(models[0].capabilities, ["completion", "tools"]);
    assert_eq!(models[1].capabilities, ["completion", "vision"]);
    assert_eq!(models[0].context_max, Some(131_072));
    // Gardée : pas de nouvelle requête ; oubliée : redemandée.
    remote.list().await;
    assert_eq!(provider.paths(), ["/v1/models"]);
    remote.invalidate();
    remote.list().await;
    assert_eq!(provider.paths().len(), 2);
    // Fournisseur coupé : ni listé, ni ouvert.
    store
        .call(|c| crate::store::remote::update(c, "custom", "Box", None, false, 1))
        .await
        .unwrap();
    remote.invalidate();
    assert!(remote.list().await.is_empty());
    assert!(matches!(
        remote.open("custom", "x").await,
        Err(LlmError::RemoteUnavailable)
    ));
    assert_eq!(provider.paths().len(), 2);
}

#[tokio::test]
async fn turned_off_nothing_is_asked() {
    let provider = Provider::start(|_, _| (200, openrouter_models().to_string())).await;
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    providers::save(
        &store,
        dir.path(),
        input(
            RemoteProviderKind::Custom,
            Some(&provider.url()),
            Some(SECRET),
        ),
    )
    .await
    .unwrap();
    let access = access(&store, dir.path());
    // Réglage coupé (défaut) : aucun modèle distant, aucune requête.
    let listed = crate::llm::models::list(&access).await.unwrap();
    assert!(listed.is_empty());
    let id = "@remote/custom/google/gemma-3-27b-it";
    let refused = access
        .open(
            id,
            OpenOptions::default(),
            &CancellationToken::new(),
            &|| {},
        )
        .await;
    assert!(matches!(refused, Err(LlmError::RemoteOff)), "{refused:?}");
    assert!(provider.paths().is_empty());
    // Permis : listés, ouverts sans `on_loading`.
    allow_remote(&store, true).await;
    let listed = crate::llm::models::list(&access).await.unwrap();
    assert_eq!(listed.len(), 2);
    let session = access
        .open(
            id,
            OpenOptions::default(),
            &CancellationToken::new(),
            &|| panic!("nothing loads for a remote model"),
        )
        .await
        .unwrap();
    assert!(session.remote);
    assert_eq!(session.id, id);
    assert_eq!(session.context, Some(96_000));
    assert!(session.capabilities.vision);
    assert_eq!(session.endpoint.dialect, Dialect::OpenAi);
    assert_eq!(session.endpoint.base_url, provider.url());
    let request = session.request(vec![Message::user("hi")]);
    assert_eq!(request.model.as_deref(), Some("google/gemma-3-27b-it"));
    // Détails : capacités de la liste, sans réflexion.
    let details = crate::llm::models::details(&access, id).await.unwrap();
    assert_eq!(details.capabilities, ["completion", "vision"]);
    assert_eq!(details.thinking, None);
}

#[tokio::test]
async fn a_disabled_provider_is_neither_listed_nor_opened() {
    let provider = Provider::start(|_, _| (200, openrouter_models().to_string())).await;
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    allow_remote(&store, true).await;
    providers::save(
        &store,
        dir.path(),
        RemoteProviderInput {
            enabled: false,
            ..input(
                RemoteProviderKind::Custom,
                Some(&provider.url()),
                Some(SECRET),
            )
        },
    )
    .await
    .unwrap();
    let access = access(&store, dir.path());
    // Modèles distants permis, mais ce fournisseur coupé : rien de lui, ni
    // dans la liste, ni à l'ouverture, et aucune requête vers lui.
    assert!(crate::llm::models::list(&access).await.unwrap().is_empty());
    let refused = access
        .open(
            "@remote/custom/google/gemma-3-27b-it",
            OpenOptions::default(),
            &CancellationToken::new(),
            &|| {},
        )
        .await;
    assert!(
        matches!(refused, Err(LlmError::RemoteUnavailable)),
        "{refused:?}"
    );
    assert!(provider.paths().is_empty());
}

/// Corps complet : tous les réglages de llama.cpp, réflexion, appels
/// d'outils d'un modèle local plus tôt dans la conversation.
fn loaded_request(model: &str) -> ChatRequest {
    let mut assistant = Message::assistant("");
    assistant.reasoning_content = Some("hmm".into());
    assistant.tool_calls = vec![ToolCall {
        id: "call_local_0".into(),
        name: "web_search".into(),
        arguments: "{\"query\":\"x\"}".into(),
    }];
    ChatRequest {
        model: Some(model.into()),
        messages: vec![
            Message::user("hi"),
            assistant,
            Message::tool("call_local_0", "web_search", "{}"),
        ],
        think: inference::Think::Off,
        top_k: Some(20),
        min_p: Some(0.05),
        repeat_penalty: Some(1.1),
        repeat_last_n: Some(64),
        cache_prompt: Some(true),
        seed: Some(3),
        temperature: Some(0.7),
        ..ChatRequest::default()
    }
}

async fn answer(
    endpoint: &Endpoint,
    request: &ChatRequest,
) -> Result<String, inference::InferenceError> {
    let mut stream = inference::chat_stream(endpoint, request, CancellationToken::new()).await?;
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        if let inference::Event::Content(chunk) = event? {
            text.push_str(&chunk);
        }
    }
    Ok(text)
}

#[tokio::test]
async fn no_llama_field_reaches_a_remote_provider() {
    for (alias, dialect, expected) in [
        ("chatonlm-remote-strict", Dialect::Mistral, "strict ok"),
        ("chatonlm-remote-openai", Dialect::OpenAi, "openai ok"),
    ] {
        let fake = FakeLlama::start(Config::new("m.gguf").alias(alias).api_key(SECRET))
            .await
            .unwrap();
        let mut endpoint = Endpoint::remote(fake.url(), SECRET).unwrap();
        endpoint.dialect = dialect;
        let request = loaded_request(alias);
        assert_eq!(
            answer(&endpoint, &request).await.unwrap(),
            expected,
            "{alias}"
        );
        // Le même corps tel que llama-server le reçoit : refusé.
        endpoint.dialect = Dialect::LlamaServer;
        let refused = answer(&endpoint, &request).await;
        assert!(
            matches!(
                refused,
                Err(inference::InferenceError::Http {
                    status: 400 | 422,
                    ..
                })
            ),
            "{alias}: {refused:?}"
        );
    }
    // Mistral : sans `stream_options` (inconnu chez lui).
    let fake = FakeLlama::start(Config::new("m.gguf").alias("chatonlm-remote-strict"))
        .await
        .unwrap();
    let mut endpoint = Endpoint::remote(fake.url(), SECRET).unwrap();
    endpoint.dialect = Dialect::OpenAi;
    assert!(matches!(
        answer(&endpoint, &loaded_request("m")).await,
        Err(inference::InferenceError::Http { status: 422, .. })
    ));
}

/// Ouvre `id` comme un envoi : détails (que la liste du fournisseur réponde
/// ou non), puis ouverture, puis requête ; l'erreur classée.
async fn first_error(access: &Access, id: &str) -> LlmError {
    let details = crate::llm::models::details(access, id).await;
    assert!(details.is_ok(), "details of {id}: {details:?}");
    let session = match access
        .open(
            id,
            OpenOptions::default(),
            &CancellationToken::new(),
            &|| {},
        )
        .await
    {
        Ok(session) => session,
        Err(error) => return error,
    };
    let request = session.request(vec![Message::user("hi")]);
    match crate::llm::chat_stream(&session, &request, CancellationToken::new()).await {
        Err(error) => error,
        Ok(mut stream) => match stream.next().await {
            Some(Err(error)) => session.error(error),
            other => panic!("an error was expected: {other:?}"),
        },
    }
}

/// Fournisseur « custom » ajouté directement en base (sans clé).
async fn custom_row(store: &Store, id: &str, name: &str, url: String) {
    let (id, name) = (id.to_owned(), name.to_owned());
    store
        .call(move |c| {
            crate::store::remote::insert(
                c,
                &crate::store::remote::ProviderRow {
                    id,
                    kind: RemoteProviderKind::Custom,
                    name,
                    base_url: Some(url),
                    enabled: true,
                    created_at: 9,
                    updated_at: 9,
                },
            )
        })
        .await
        .unwrap();
}

/// Chemin réel d'un envoi (détails, ouverture, requête) : la liste du
/// fournisseur échoue la première (clé, limite, hors ligne) sans cacher le
/// vrai code ; les textes donnent le nom choisi par l'utilisateur.
#[tokio::test]
async fn provider_errors_have_their_codes() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    allow_remote(&store, true).await;
    let access = access(&store, dir.path());
    // Clé refusée (liste comprise : `/v1/models` exige la clé).
    let fake = FakeLlama::start(
        Config::new("m.gguf")
            .alias("chatonlm-remote-openai")
            .api_key("other"),
    )
    .await
    .unwrap();
    let saved = providers::save(
        &store,
        dir.path(),
        RemoteProviderInput {
            name: "My box".into(),
            ..input(RemoteProviderKind::Custom, Some(&fake.url()), Some(SECRET))
        },
    )
    .await
    .unwrap();
    let id = format!("@remote/{}/m", saved.id);
    let error = first_error(&access, &id).await;
    assert_eq!(
        error,
        LlmError::RemoteKey {
            provider: "My box".into()
        }
    );
    assert!(error.message().contains("My box"), "{}", error.message());
    let checked = providers::check(&store, dir.path(), &saved.id)
        .await
        .unwrap();
    assert_eq!(checked.problem, Some(RemoteProblem::Key));
    assert!(!checked.ok);
    // Limites : 402 (crédit), 429 (cadence) ; 503 sans corps : indisponible
    // pour le moment, jamais « le modèle se charge ».
    for (status, expected) in [
        (402, "remoteLimit"),
        (429, "remoteLimit"),
        (503, "remoteUnreachable"),
        (500, "remoteUnreachable"),
    ] {
        let fake = FakeLlama::start(Config::new("m.gguf").mode(Mode::Status(status)))
            .await
            .unwrap();
        let provider = format!("status-{status}");
        custom_row(&store, &provider, "Busy", fake.url()).await;
        let error = first_error(&access, &format!("@remote/{provider}/m")).await;
        assert_eq!(error.code(), expected, "{status}: {error:?}");
        assert!(error.message().contains("Busy"), "{}", error.message());
        if status < 500 {
            let checked = providers::check(&store, dir.path(), &provider)
                .await
                .unwrap();
            assert_eq!(checked.problem, Some(RemoteProblem::Limit), "{status}");
        }
    }
    // Modèle retiré chez le fournisseur : indisponible, pas « non installé ».
    let gone = Provider::start(|path, _| match path {
        "/v1/models" => (200, json!({ "data": [] }).to_string()),
        _ => (
            404,
            json!({ "error": { "message": "model 'm' not found" } }).to_string(),
        ),
    })
    .await;
    custom_row(&store, "gone", "Gone", gone.url()).await;
    let error = first_error(&access, "@remote/gone/m").await;
    assert_eq!(error, LlmError::RemoteUnavailable);
    // Injoignable.
    let closed = FakeLlama::start(Config::new("m.gguf")).await.unwrap();
    let url = closed.url();
    closed.stop().await;
    let unreachable = providers::save(
        &store,
        dir.path(),
        RemoteProviderInput {
            name: "Laptop".into(),
            ..input(RemoteProviderKind::Custom, Some(&url), None)
        },
    )
    .await
    .unwrap();
    assert_eq!(
        first_error(&access, &format!("@remote/{}/m", unreachable.id)).await,
        LlmError::RemoteUnreachable {
            provider: "Laptop".into()
        }
    );
    let checked = providers::check(&store, dir.path(), &unreachable.id)
        .await
        .unwrap();
    assert_eq!(checked.problem, Some(RemoteProblem::Unreachable));
    // Fournisseur connu sans clé : refusé sans requête.
    let keyless = providers::save(
        &store,
        dir.path(),
        input(RemoteProviderKind::OpenRouter, None, None),
    )
    .await
    .unwrap();
    assert_eq!(
        first_error(&access, &format!("@remote/{}/m", keyless.id)).await,
        LlmError::RemoteKey {
            provider: "OpenRouter".into()
        }
    );
    assert_eq!(
        providers::check(&store, dir.path(), &keyless.id)
            .await
            .unwrap()
            .problem,
        Some(RemoteProblem::NoKey)
    );
    // Retiré : plus disponible.
    assert_eq!(
        first_error(&access, "@remote/nobody/m").await,
        LlmError::RemoteUnavailable
    );
}

/// Événement final d'un envoi par le chat (kind, texte).
async fn send_through_chat(access: &Access, model: &str) -> (String, String) {
    let events: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sink = {
        let events = events.clone();
        move |event: crate::chat::ChatEvent| {
            events.lock().push(serde_json::to_value(&event).unwrap());
        }
    };
    let server = crate::chat::Server::current(access).await.unwrap();
    let request: crate::chat::SendRequest = serde_json::from_value(json!({
        "streamId": format!("stream-{model}"),
        "content": "Say hello.",
        "model": model,
    }))
    .unwrap();
    let engine = crate::chat::ChatEngine::default();
    let staging = crate::attachments::Staging::default();
    let sending = crate::chat::send_to(
        access.store(),
        &engine,
        &staging,
        server,
        request,
        crate::chat::Origin::Page,
        &sink,
    );
    tokio::time::timeout(Duration::from_secs(20), sending)
        .await
        .expect("the send ends")
        .unwrap();
    let events = events.lock();
    let error = events
        .iter()
        .rev()
        .find(|event| event["type"] == "error")
        .unwrap_or_else(|| panic!("no error: {events:?}"));
    (
        error["kind"].as_str().unwrap().to_owned(),
        error["message"].as_str().unwrap().to_owned(),
    )
}

/// Par le chemin du chat (`send_to` : détails, ouverture, requête) : une
/// clé refusée, une limite, un fournisseur éteint ou un modèle retiré
/// donnent leur code, jamais « modèle non installé ».
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_send_shows_the_real_error_in_the_chat() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    allow_remote(&store, true).await;
    let access = access(&store, dir.path());
    // Clé B enregistrée, le serveur attend A : sa liste aussi répond 401.
    let fake = FakeLlama::start(
        Config::new("m.gguf")
            .alias("chatonlm-remote-openai")
            .api_key("key-A-123456789"),
    )
    .await
    .unwrap();
    let saved = providers::save(
        &store,
        dir.path(),
        RemoteProviderInput {
            name: "Home server".into(),
            ..input(
                RemoteProviderKind::Custom,
                Some(&fake.url()),
                Some("key-B-987654321"),
            )
        },
    )
    .await
    .unwrap();
    let (kind, message) = send_through_chat(&access, &format!("@remote/{}/m", saved.id)).await;
    assert_eq!(kind, "remoteKey", "{message}");
    assert!(message.contains("Home server"), "{message}");
    // Limite.
    let limited = FakeLlama::start(Config::new("m.gguf").mode(Mode::Status(429)))
        .await
        .unwrap();
    custom_row(&store, "limited", "Paid", limited.url()).await;
    let (kind, message) = send_through_chat(&access, "@remote/limited/m").await;
    assert_eq!(kind, "remoteLimit", "{message}");
    // Fournisseur éteint (portable hors ligne).
    let closed = FakeLlama::start(Config::new("m.gguf")).await.unwrap();
    let url = closed.url();
    closed.stop().await;
    custom_row(&store, "off", "Desktop", url).await;
    let (kind, message) = send_through_chat(&access, "@remote/off/m").await;
    assert_eq!(kind, "remoteUnreachable", "{message}");
    assert!(message.contains("Desktop"), "{message}");
    // Modèle retiré chez le fournisseur : modèle absent (la page relit la
    // liste), avec le texte des modèles distants.
    let gone = Provider::start(|path, _| match path {
        "/v1/models" => (200, json!({ "data": [] }).to_string()),
        _ => (
            404,
            json!({ "error": { "message": "model 'm' not found" } }).to_string(),
        ),
    })
    .await;
    custom_row(&store, "gone", "Gone", gone.url()).await;
    let (kind, message) = send_through_chat(&access, "@remote/gone/m").await;
    assert_eq!(kind, "modelMissing");
    assert_eq!(message, LlmError::RemoteUnavailable.message());
}

/// Un fournisseur lent ou en panne ne retient ni la liste des modèles (même
/// locale) ni un envoi : une seule requête de liste à la fois, attendue au
/// plus le délai depuis son départ ; un échec retenu n'est pas redemandé.
#[tokio::test(flavor = "multi_thread")]
async fn a_slow_or_failing_provider_does_not_hold_the_lists() {
    const BUDGET: Duration = Duration::from_millis(300);
    // Réponse après 3 s ; bornes larges (base sur ntfs3) mais loin de 3 s.
    const LATE: Duration = Duration::from_secs(3);
    let slow = Provider::start_late(LATE, |_, _| (200, openrouter_models().to_string())).await;
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    allow_remote(&store, true).await;
    let remote = Arc::new(
        Remote::new(store.clone(), dir.path().to_path_buf())
            .with_timeouts(BUDGET, Duration::from_secs(5)),
    );
    let access = access_with(&store, dir.path(), remote.clone());
    providers::save(
        &store,
        dir.path(),
        input(RemoteProviderKind::Custom, Some(&slow.url()), None),
    )
    .await
    .unwrap();
    // Première liste : le délai, pas la réponse.
    let started = Instant::now();
    assert!(crate::llm::models::list(&access).await.unwrap().is_empty());
    assert!(started.elapsed() < LATE / 2, "{:?}", started.elapsed());
    // Suivantes : la requête en cours n'est plus attendue.
    let started = Instant::now();
    assert!(remote.list().await.is_empty());
    let id = "@remote/custom/google/gemma-3-27b-it";
    crate::llm::models::details(&access, id).await.unwrap();
    // L'envoi n'attend pas une seconde fois la liste.
    access
        .open(
            id,
            OpenOptions::default(),
            &CancellationToken::new(),
            &|| {},
        )
        .await
        .unwrap();
    assert!(started.elapsed() < LATE / 3, "{:?}", started.elapsed());
    assert_eq!(slow.paths().len(), 1);
    // La requête finit en tâche de fond et remplit la liste.
    tokio::time::sleep(LATE).await;
    assert_eq!(remote.list().await.len(), 2);
    assert_eq!(slow.paths().len(), 1);

    // Fournisseur en panne : l'échec est retenu, rien n'est redemandé.
    let failing = Provider::start(|_, _| (500, "{}".into())).await;
    let dir = tempfile::tempdir().unwrap();
    let store = self::store(dir.path());
    let remote = Remote::new(store.clone(), dir.path().to_path_buf())
        .with_timeouts(BUDGET, Duration::from_secs(5));
    providers::save(
        &store,
        dir.path(),
        input(RemoteProviderKind::Custom, Some(&failing.url()), None),
    )
    .await
    .unwrap();
    assert!(remote.list().await.is_empty());
    assert!(remote.list().await.is_empty());
    assert_eq!(failing.paths().len(), 1);
    // Oublié (fournisseur changé, vérifié) : redemandé.
    remote.invalidate();
    remote.list().await;
    assert_eq!(failing.paths().len(), 2);
}

/// Un fournisseur qui accepte la connexion puis se tait n'arrête pas
/// l'envoi sans fin : délai des en-têtes, puis « injoignable ».
#[tokio::test]
async fn a_silent_provider_ends_the_send() {
    let silent = FakeLlama::start(Config::new("m.gguf").mode(Mode::Silent))
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let remote = Remote::new(store.clone(), dir.path().to_path_buf())
        .with_timeouts(Duration::from_millis(100), Duration::from_millis(300));
    custom_row(&store, "mute", "Mute", silent.url()).await;
    let session = remote.open("mute", "m").await.unwrap();
    let request = session.request(vec![Message::user("hi")]);
    let started = Instant::now();
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        crate::llm::chat_stream(&session, &request, CancellationToken::new()),
    )
    .await
    .expect("bounded")
    .err()
    .expect("an error");
    assert_eq!(
        error,
        LlmError::RemoteUnreachable {
            provider: "Mute".into()
        }
    );
    assert!(started.elapsed() < Duration::from_secs(3));
}

/// L'adresse d'un serveur « custom » change : sa clé est retirée (jamais
/// envoyée au nouveau serveur), sauf une nouvelle clé donnée avec elle ; la
/// page peut retirer une clé (`apiKey: ""`) ; un identifiant retiré n'est
/// jamais redonné.
#[tokio::test]
async fn a_key_never_follows_a_new_address() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let keys = Keys::new(dir.path());
    let first = providers::save(
        &store,
        dir.path(),
        input(
            RemoteProviderKind::Custom,
            Some("https://a.example.com/v1"),
            Some(SECRET),
        ),
    )
    .await
    .unwrap();
    assert_eq!(first.id, "custom");
    let change = |url: &str, key: Option<&str>| RemoteProviderInput {
        id: Some("custom".into()),
        ..input(RemoteProviderKind::Custom, Some(url), key)
    };
    // Même adresse (écrite autrement) : la clé reste.
    let same = providers::save(&store, dir.path(), change("https://a.example.com/", None))
        .await
        .unwrap();
    assert!(same.has_key);
    // Autre adresse sans clé : retirée.
    let moved = providers::save(&store, dir.path(), change("https://b.example.com", None))
        .await
        .unwrap();
    assert!(!moved.has_key);
    assert_eq!(keys.get("custom"), None);
    assert!(
        !std::fs::read_to_string(keys.path())
            .unwrap()
            .contains(SECRET)
    );
    // Autre adresse avec sa clé : gardée.
    let with_key = providers::save(
        &store,
        dir.path(),
        change("https://c.example.com", Some("sk-other-key-1234")),
    )
    .await
    .unwrap();
    assert!(with_key.has_key);
    // Retrait par la page.
    let removed = providers::save(
        &store,
        dir.path(),
        change("https://c.example.com", Some("")),
    )
    .await
    .unwrap();
    assert!(!removed.has_key);
    // Retiré puis un autre ajouté : jamais le même identifiant.
    providers::delete(&store, dir.path(), "custom")
        .await
        .unwrap();
    let next = providers::save(
        &store,
        dir.path(),
        input(
            RemoteProviderKind::Custom,
            Some("https://d.example.com"),
            None,
        ),
    )
    .await
    .unwrap();
    assert_ne!(next.id, "custom");
}

/// Fichier des clés illisible un instant : l'enregistrement échoue sans
/// rien écrire (les autres clés restent) ; le retrait d'un fournisseur
/// aussi (sa ligne reste, à retirer de nouveau).
#[cfg(unix)]
#[tokio::test]
async fn an_unreadable_key_file_is_never_overwritten() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let keys = Keys::new(dir.path());
    let saved = providers::save(
        &store,
        dir.path(),
        input(RemoteProviderKind::Mistral, None, Some(SECRET)),
    )
    .await
    .unwrap();
    let before = std::fs::read(keys.path()).unwrap();
    std::fs::set_permissions(keys.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
    // (Le super-utilisateur lit tout : l'essai n'a de sens qu'ailleurs.)
    let unreadable = std::fs::read(keys.path()).is_err();
    if unreadable {
        assert!(keys.set("openrouter", "sk-another-key-99").is_err());
        let refused = providers::save(
            &store,
            dir.path(),
            input(
                RemoteProviderKind::OpenRouter,
                None,
                Some("sk-another-key-99"),
            ),
        )
        .await;
        assert!(refused.is_err(), "{refused:?}");
        assert!(
            providers::delete(&store, dir.path(), &saved.id)
                .await
                .is_err()
        );
    }
    std::fs::set_permissions(keys.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(std::fs::read(keys.path()).unwrap(), before);
    assert_eq!(keys.get(&saved.id).as_deref(), Some(SECRET));
    if unreadable {
        let ids: Vec<String> = providers::list(&store, dir.path())
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert!(ids.contains(&saved.id), "{ids:?}");
    }
}

#[tokio::test]
async fn openrouter_keys_are_checked_apart_from_the_public_list() {
    let provider = Provider::start(|path, bearer| match (path, bearer) {
        ("/v1/models", _) => (200, openrouter_models().to_string()),
        ("/v1/key", Some(SECRET)) => (200, "{\"data\":{}}".into()),
        ("/v1/key", _) => (401, "{}".into()),
        _ => (404, "{}".into()),
    })
    .await;
    let models = catalog::check(RemoteProviderKind::OpenRouter, &provider.url(), "wrong").await;
    assert_eq!(models, Err(Problem::Key));
    let models = catalog::check(RemoteProviderKind::OpenRouter, &provider.url(), SECRET).await;
    assert_eq!(models.unwrap().len(), 2);
    assert_eq!(provider.paths(), ["/v1/key", "/v1/key", "/v1/models"]);
    // Ailleurs, la liste suffit ; une réponse illisible est dite.
    let garbage = Provider::start(|_, _| (200, "not json".into())).await;
    assert_eq!(
        catalog::check(RemoteProviderKind::Custom, &garbage.url(), "").await,
        Err(Problem::Malformed)
    );
    let server = Provider::start(|_, _| (500, "{}".into())).await;
    assert_eq!(
        catalog::check(RemoteProviderKind::Custom, &server.url(), "").await,
        Err(Problem::Server(500))
    );
}

/// La clé n'apparaît ni dans les réglages, ni dans ce que reçoit la page, ni
/// en base, ni au journal, ni dans les erreurs.
#[tokio::test]
async fn the_key_never_leaves_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    allow_remote(&store, true).await;
    let refusing =
        Provider::start(|_, _| (401, format!("{{\"error\":\"bad key {SECRET}\"}}"))).await;
    let access = access(&store, dir.path());
    let saved = providers::save(
        &store,
        dir.path(),
        input(
            RemoteProviderKind::Custom,
            Some(&refusing.url()),
            Some(SECRET),
        ),
    )
    .await
    .unwrap();
    let mut seen_by_page = vec![serde_json::to_string(&saved).unwrap()];
    seen_by_page
        .push(serde_json::to_string(&providers::list(&store, dir.path()).await.unwrap()).unwrap());
    seen_by_page.push(
        serde_json::to_string(
            &providers::check(&store, dir.path(), &saved.id)
                .await
                .unwrap(),
        )
        .unwrap(),
    );
    seen_by_page
        .push(serde_json::to_string(&crate::llm::models::list(&access).await.unwrap()).unwrap());
    let settings = store
        .call(|c| crate::store::settings::get(c, None))
        .await
        .unwrap();
    seen_by_page.push(serde_json::to_string(&settings).unwrap());
    let session = access
        .open(
            "@remote/custom/m",
            OpenOptions::default(),
            &CancellationToken::new(),
            &|| {},
        )
        .await
        .unwrap();
    seen_by_page.push(format!("{session:?}"));
    let error = crate::llm::chat_stream(
        &session,
        &session.request(vec![Message::user("hi")]),
        CancellationToken::new(),
    )
    .await
    .err();
    seen_by_page.push(format!("{error:?}"));
    if let Some(error) = &error {
        seen_by_page.push(error.message());
        seen_by_page.push(crate::error::AppError::from(error.clone()).to_string());
    }
    seen_by_page.push(format!(
        "{:?}",
        input(RemoteProviderKind::Custom, None, Some(SECRET))
    ));
    for text in &seen_by_page {
        assert!(!text.contains(SECRET), "{text}");
        assert!(!text.contains("SECRET-7f3a"), "{text}");
    }
    // Journal du module.
    for line in NOTES.lock().iter() {
        assert!(!line.contains(SECRET), "{line}");
    }
    // Base et fichiers de SQLite, octet par octet.
    drop(access);
    drop(store);
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = std::fs::read(&path).unwrap();
            assert!(
                !bytes.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()),
                "{}",
                path.display()
            );
        }
    }
    // La clé n'est que dans son fichier.
    let file = std::fs::read(Keys::new(dir.path()).path()).unwrap();
    assert!(file.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()));
    // Lisible du seul utilisateur (Linux ; sous Windows, le dossier des
    // données n'est ouvert qu'à son propriétaire).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(Keys::new(dir.path()).path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }
}
