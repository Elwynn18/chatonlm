//! Modèles en mémoire (`/api/ps`), chargement et déchargement par
//! `/api/generate` sans prompt (propre à ChatonLM : onglet Models des
//! réglages, préchargement, ligne « Loading model... » du chat).
//!
//! Comportement d'Ollama 0.35.1 (instance de test) : `/api/ps` ne liste un
//! modèle qu'une fois chargé ; un modèle chargé avec un autre `num_ctx` est
//! rechargé, sauf quand le contexte demandé dépasse celui du modèle (Ollama
//! le ramène au maximum du modèle, `/api/ps` montre ce maximum, rien n'est
//! rechargé) ; sans `keep_alive`, un modèle déjà chargé garde sa durée.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use ts_rs::TS;

use super::client;
use super::models::{answered, unreachable};
use crate::error::{AppError, AppResult};
use crate::hub::installed::timestamp;

/// Délai de la liste (état gardé en mémoire par Ollama).
const LIST_TIMEOUT: Duration = Duration::from_secs(5);

/// Délai d'un déchargement (le moteur du modèle est arrêté).
const UNLOAD_TIMEOUT: Duration = Duration::from_secs(10);

/// Échéances à partir du 1er janvier 2200 (en ns) : `keep_alive` négatif,
/// qu'Ollama traduit par une échéance dans 292 ans (en 2319).
const KEPT_FROM: i128 = 7_258_118_400 * 1_000_000_000;

/// Un modèle en mémoire, d'après `/api/ps`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunningModel {
    /// Nom tel que donné par Ollama (`name`).
    pub name: String,
    /// Mémoire occupée en octets (RAM + VRAM).
    pub size: u64,
    /// Part en VRAM, en octets (`size_vram`).
    pub size_vram: u64,
    /// Contexte du modèle chargé ; `None` pour un serveur qui ne le dit pas.
    pub context_length: Option<u32>,
    /// Échéance du déchargement, en millisecondes depuis l'époque Unix ;
    /// `None` : inconnue (absente, illisible) ou gardé chargé (`kept`).
    pub expires_at: Option<i64>,
    /// Gardé chargé jusqu'au déchargement (`keep_alive` négatif : Ollama
    /// annonce une échéance au-delà de l'an 2200).
    pub kept: bool,
    /// « 8.2B », vide si inconnu.
    pub parameter_size: String,
    /// « Q4_K_M », vide si inconnu.
    pub quantization_level: String,
}

/// `api.ProcessResponse` ; liste absente ou nulle : vide.
#[derive(Deserialize)]
struct PsResponse {
    #[serde(default)]
    models: Option<Vec<Ps>>,
}

/// `api.ProcessModelResponse`, champs utiles ; tous facultatifs.
#[derive(Deserialize)]
struct Ps {
    #[serde(default)]
    name: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    size_vram: u64,
    #[serde(default)]
    context_length: Option<u32>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    details: PsDetails,
}

#[derive(Default, Deserialize)]
struct PsDetails {
    #[serde(default)]
    parameter_size: String,
    #[serde(default)]
    quantization_level: String,
}

/// Modèles en mémoire, dans l'ordre d'Ollama.
pub async fn list(base: &str) -> AppResult<Vec<RunningModel>> {
    list_within(base, LIST_TIMEOUT).await
}

/// Comme [`list`], avec ce délai (court pour le moteur de chat, qui s'en
/// passe s'il ne répond pas).
pub(crate) async fn list_within(base: &str, timeout: Duration) -> AppResult<Vec<RunningModel>> {
    let url = client::server_url(base)?;
    let client = client::client(&url, Some(timeout)).map_err(unreachable)?;
    let response = client
        .get(format!("{base}/api/ps"))
        .send()
        .await
        .map_err(unreachable)?;
    let status = response.status();
    let body = response.text().await.map_err(unreachable)?;
    if !status.is_success() {
        return Err(answered(
            status.as_u16(),
            &super::models::error_message(&body),
        ));
    }
    parse(&body)
}

fn parse(body: &str) -> AppResult<Vec<RunningModel>> {
    let response: PsResponse = serde_json::from_str(body).map_err(|error| {
        AppError::Ollama(crate::i18n::t!("ollama.unexpectedLoaded", error = error))
    })?;
    Ok(response
        .models
        .unwrap_or_default()
        .into_iter()
        .map(|ps| {
            let expires = ps.expires_at.as_deref().and_then(timestamp);
            let kept = expires.is_some_and(|at| at >= KEPT_FROM);
            RunningModel {
                name: if ps.name.is_empty() {
                    ps.model
                } else {
                    ps.name
                },
                size: ps.size,
                size_vram: ps.size_vram,
                context_length: ps.context_length.filter(|&length| length > 0),
                // Avant 1970 (date zéro de Go, « 0001-01-01T00:00:00Z ») :
                // inconnue.
                expires_at: expires
                    .filter(|&at| at > 0 && !kept)
                    .and_then(|at| i64::try_from(at / 1_000_000).ok()),
                kept,
                parameter_size: ps.details.parameter_size,
                quantization_level: ps.details.quantization_level,
            }
        })
        .collect())
}

/// Nom complet d'un modèle : `:latest` ajouté à un nom sans étiquette
/// (étiquette : après le dernier `:` du dernier segment `/`, le port d'un
/// registre n'en est pas une).
fn full_name(model: &str) -> String {
    let last = model.rsplit('/').next().unwrap_or(model);
    if last.contains(':') {
        model.to_owned()
    } else {
        format!("{model}:latest")
    }
}

/// Même modèle pour Ollama : à `:latest` et à la casse près.
pub fn same_model(a: &str, b: &str) -> bool {
    full_name(a).eq_ignore_ascii_case(&full_name(b))
}

/// Charge `model` avec les options d'un envoi du chat (`num_ctx` si
/// `context_length` > 0, `keep_alive` si `keep_alive` != 0), pour que le
/// premier message ne le recharge pas ; rend la main une fois chargé. Pas de
/// délai sur la réponse : un gros modèle met des minutes à se charger.
pub async fn load(base: &str, model: &str, context_length: u32, keep_alive: i64) -> AppResult<()> {
    try_load(base, model, context_length, keep_alive)
        .await
        .map_err(AppError::from)
}

/// Comme [`load`], avec le refus d'Ollama à part (le moteur de chat le
/// classe comme une erreur de `/api/chat`).
pub(crate) async fn try_load(
    base: &str,
    model: &str,
    context_length: u32,
    keep_alive: i64,
) -> Result<(), LoadFailure> {
    let mut body = json!({ "model": model });
    if keep_alive != 0 {
        body["keep_alive"] = json!(keep_alive);
    }
    if context_length > 0 {
        body["options"] = json!({ "num_ctx": context_length });
    }
    generate(base, &body, None).await
}

/// Décharge `model` (`keep_alive: 0`) ; sans effet s'il n'est pas chargé.
pub async fn unload(base: &str, model: &str) -> AppResult<()> {
    let body = json!({ "model": model, "keep_alive": 0 });
    generate(base, &body, Some(UNLOAD_TIMEOUT))
        .await
        .map_err(AppError::from)
}

/// Décharge tous les modèles en mémoire, l'un après l'autre ; un échec
/// n'arrête pas les suivants, la première erreur est rendue.
pub async fn unload_all(base: &str) -> AppResult<()> {
    let mut first = None;
    for running in list(base).await? {
        if let Err(error) = unload(base, &running.name).await {
            first.get_or_insert(error);
        }
    }
    first.map_or(Ok(()), Err)
}

/// Échec de `/api/generate`.
#[derive(Debug)]
pub(crate) enum LoadFailure {
    /// Réponse d'erreur d'Ollama : statut et message (première ligne du
    /// corps, comme `chat::error_line`).
    Refused {
        status: reqwest::StatusCode,
        message: String,
    },
    /// Requête impossible (adresse, connexion, lecture...).
    Failed(AppError),
}

/// Pour l'interface : le message d'Ollama (`model 'x' not found` :
/// [`AppError::NotFound`]), sinon le statut seul.
impl From<LoadFailure> for AppError {
    fn from(failure: LoadFailure) -> Self {
        match failure {
            LoadFailure::Failed(error) => error,
            LoadFailure::Refused { status, message } if message.trim().is_empty() => {
                AppError::Ollama(crate::i18n::t!(
                    "ollama.httpStatus",
                    status = status.as_u16()
                ))
            }
            LoadFailure::Refused { status, message }
                if status == reqwest::StatusCode::NOT_FOUND =>
            {
                AppError::NotFound(message)
            }
            LoadFailure::Refused { message, .. } => AppError::Ollama(message),
        }
    }
}

/// `POST /api/generate` sans prompt (chargement ou déchargement).
async fn generate(
    base: &str,
    body: &serde_json::Value,
    timeout: Option<Duration>,
) -> Result<(), LoadFailure> {
    let failed = |error: reqwest::Error| LoadFailure::Failed(unreachable(error));
    if body["model"]
        .as_str()
        .is_none_or(|model| model.trim().is_empty())
    {
        // i18n-ignore : la page ne demande jamais un nom vide.
        return Err(LoadFailure::Failed(AppError::Invalid(
            "model name is empty".into(),
        )));
    }
    let url = client::server_url(base).map_err(LoadFailure::Failed)?;
    let client = client::client(&url, timeout).map_err(failed)?;
    let response = client
        .post(format!("{base}/api/generate"))
        .json(body)
        .send()
        .await
        .map_err(failed)?;
    let status = response.status();
    // Lu jusqu'au bout : la réponse arrive une fois le modèle chargé.
    let text = response.text().await.map_err(failed)?;
    if status.is_success() {
        return Ok(());
    }
    Err(LoadFailure::Refused {
        status,
        message: crate::chat::error_line(&text),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use fake_ollama::{FakeOllama, LOAD_FAILURE, LOADED_SIZE, Mode};
    use parking_lot::Mutex;
    use serde_json::Value;

    use super::*;
    use crate::store::now_ms;

    /// Réponse de l'instance de test (Ollama 0.35.1) : gemma3:270m chargé
    /// pour 120 s, puis gardé (`keep_alive: -1`, échéance en 2319, à l'heure
    /// d'hiver).
    const RECORDED: &str = r#"{"models":[{"name":"gemma3:270m","model":"gemma3:270m","size":323359866,"digest":"e7d36fb2c3b3293cfe56d55889867a064b3a2b22e98335f2e6e8a387e081d6be","details":{"parent_model":"","format":"gguf","family":"gemma3","families":["gemma3"],"parameter_size":"268.10M","quantization_level":"Q8_0"},"expires_at":"2026-10-06T15:06:02.890490792+02:00","size_vram":323359866,"context_length":4096},{"name":"qwen3:0.6b","model":"qwen3:0.6b","size":1073741824,"digest":"7df6b6e09427a769808717c0a93cadc4ae99ed4eb8bf5ca557c90846becea435","details":{"parent_model":"","format":"gguf","family":"qwen3","families":["qwen3"],"parameter_size":"751.63M","quantization_level":"Q4_K_M"},"expires_at":"2319-01-16T13:59:19.035362148+01:00","size_vram":805306368,"context_length":40960}]}"#;

    #[test]
    fn the_list_is_read_as_ollama_writes_it() {
        let models = parse(RECORDED).unwrap();
        assert_eq!(
            models,
            [
                RunningModel {
                    name: "gemma3:270m".into(),
                    size: 323_359_866,
                    size_vram: 323_359_866,
                    context_length: Some(4096),
                    // 13:06:02.890 UTC.
                    expires_at: Some(1_791_291_962_890),
                    kept: false,
                    parameter_size: "268.10M".into(),
                    quantization_level: "Q8_0".into(),
                },
                RunningModel {
                    name: "qwen3:0.6b".into(),
                    size: 1_073_741_824,
                    size_vram: 805_306_368,
                    context_length: Some(40960),
                    expires_at: None,
                    kept: true,
                    parameter_size: "751.63M".into(),
                    quantization_level: "Q4_K_M".into(),
                },
            ]
        );
        // Liste vide, nulle ou absente.
        for empty in [r#"{"models":[]}"#, r#"{"models":null}"#, "{}"] {
            assert!(parse(empty).unwrap().is_empty(), "{empty}");
        }
        for unreadable in [
            "<html>",
            r#"{"models":{}}"#,
            r#"{"models":[{"size":"big"}]}"#,
        ] {
            let Err(AppError::Ollama(message)) = parse(unreadable) else {
                panic!("{unreadable}");
            };
            assert!(
                message.starts_with("unexpected list of loaded models from Ollama"),
                "{message}"
            );
        }
    }

    /// Champs absents tolérés ; échéances : fraction de 0 à 9 chiffres,
    /// décalage ou « Z », au-delà de l'an 2200 → gardé, illisible ou avant
    /// 1970 (date zéro de Go) → inconnue.
    #[test]
    fn missing_fields_and_dates_are_tolerated() {
        let one = |fields: &str| {
            let models = parse(&format!(r#"{{"models":[{{{fields}}}]}}"#)).unwrap();
            assert_eq!(models.len(), 1, "{fields}");
            models.into_iter().next().unwrap()
        };
        assert_eq!(
            one(r#""model":"llama3:latest""#),
            RunningModel {
                name: "llama3:latest".into(),
                size: 0,
                size_vram: 0,
                context_length: None,
                expires_at: None,
                kept: false,
                parameter_size: String::new(),
                quantization_level: String::new(),
            }
        );
        assert_eq!(one(r#""name":"a","context_length":0"#).context_length, None);
        let expires = |date: &str| {
            let model = one(&format!(r#""name":"a","expires_at":"{date}""#));
            (model.expires_at, model.kept)
        };
        let at = Some(1_791_291_962_000);
        assert_eq!(expires("2026-10-06T13:06:02Z"), (at, false));
        assert_eq!(expires("2026-10-06T15:06:02+02:00"), (at, false));
        assert_eq!(expires("2026-10-06T09:06:02-04:00"), (at, false));
        assert_eq!(
            expires("2026-10-06T15:06:02.123+02:00"),
            (Some(1_791_291_962_123), false)
        );
        assert_eq!(
            expires("2026-10-06T15:06:02.123456789+02:00"),
            (Some(1_791_291_962_123), false)
        );
        assert_eq!(expires("2200-01-01T00:00:00Z"), (None, true));
        assert!(!expires("2199-12-31T23:59:59Z").1);
        for unknown in ["soon", "2026-10-06", "", "0001-01-01T00:00:00Z"] {
            assert_eq!(expires(unknown), (None, false), "{unknown}");
        }
    }

    #[test]
    fn names_are_compared_with_their_tag() {
        for (a, b) in [
            ("qwen3", "qwen3:latest"),
            ("QWEN3:0.6B", "qwen3:0.6b"),
            (
                "hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M",
                "hf.co/bartowski/smollm2-135m-instruct-gguf:q4_k_m",
            ),
            (
                "localhost:5000/library/llama3",
                "localhost:5000/library/llama3:latest",
            ),
        ] {
            assert!(same_model(a, b), "{a} = {b}");
        }
        for (a, b) in [
            ("qwen3", "qwen3:0.6b"),
            ("qwen3:0.6b", "qwen3:8b"),
            ("localhost:5000/llama3", "localhost:latest"),
        ] {
            assert!(!same_model(a, b), "{a} != {b}");
        }
    }

    /// Chargement avec les options d'un envoi, relu dans la liste ;
    /// déchargement.
    #[tokio::test]
    async fn models_are_loaded_listed_and_unloaded() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let base = fake.url();
        assert!(list(&base).await.unwrap().is_empty());
        load(&base, "qwen3:0.6b", 8192, 300).await.unwrap();
        load(&base, "chatonlm-split", 0, 0).await.unwrap();
        load(&base, "qwen3.5:0.8b", 0, -1).await.unwrap();
        assert_eq!(
            fake.load_requests(),
            [
                json!({ "model": "qwen3:0.6b", "kind": "load", "keep_alive": 300, "num_ctx": 8192 }),
                // 0 : rien d'envoyé (défaut du serveur).
                json!({ "model": "chatonlm-split", "kind": "load" }),
                json!({ "model": "qwen3.5:0.8b", "kind": "load", "keep_alive": -1 }),
            ]
        );
        let listed = list(&base).await.unwrap();
        let names: Vec<_> = listed.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            ["qwen3.5:0.8b", "chatonlm-split:latest", "qwen3:0.6b"]
        );
        let (kept, split, qwen3) = (&listed[0], &listed[1], &listed[2]);
        assert!(kept.kept && kept.expires_at.is_none());
        assert_eq!(kept.quantization_level, "Q8_0");
        assert_eq!(split.size_vram, LOADED_SIZE / 4 * 3);
        assert_eq!(qwen3.context_length, Some(8192));
        assert_eq!(qwen3.size, LOADED_SIZE);
        assert_eq!(qwen3.parameter_size, "751.63M");
        let left = qwen3.expires_at.unwrap() - now_ms();
        assert!((298_000..=300_000).contains(&left), "{left}");

        unload(&base, "qwen3:0.6b").await.unwrap();
        // Sans effet sur un modèle installé qui n'est pas chargé.
        unload(&base, "qwen3:0.6b").await.unwrap();
        let names: Vec<_> = list(&base)
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert_eq!(names, ["qwen3.5:0.8b", "chatonlm-split:latest"]);
        assert_eq!(
            fake.load_requests()[3..],
            vec![json!({ "model": "qwen3:0.6b", "kind": "unload", "keep_alive": 0 }); 2]
        );
        unload_all(&base).await.unwrap();
        assert!(list(&base).await.unwrap().is_empty());
        // Rien dans le journal du chat.
        assert!(fake.chat_requests().is_empty());
    }

    #[tokio::test]
    async fn errors_carry_the_message_of_ollama() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let base = fake.url();
        let Err(AppError::NotFound(message)) = load(&base, "nope:1b", 0, 0).await else {
            panic!("expected a missing model");
        };
        assert_eq!(message, "model 'nope:1b' not found");
        let Err(AppError::NotFound(message)) = unload(&base, "nope:1b").await else {
            panic!("expected a missing model");
        };
        assert_eq!(message, "model 'nope:1b' not found");
        let Err(AppError::Ollama(message)) = load(&base, "chatonlm-loadfail", 4096, 0).await else {
            panic!("expected a failed load");
        };
        assert_eq!(message, LOAD_FAILURE);
        assert!(matches!(
            load(&base, " ", 0, 0).await,
            Err(AppError::Invalid(_))
        ));

        // Proxy : statut seul, ou première ligne de sa page.
        let proxy = FakeOllama::start(Mode::Status(502)).await.unwrap();
        let Err(AppError::Ollama(message)) = load(&proxy.url(), "qwen3:0.6b", 0, 0).await else {
            panic!("expected an HTTP error");
        };
        assert_eq!(message, "Ollama answered HTTP 502");
        let Err(AppError::Ollama(message)) = list(&proxy.url()).await else {
            panic!("expected an HTTP error");
        };
        assert!(message.starts_with("Ollama answered HTTP 502"), "{message}");
        let other = FakeOllama::start(Mode::NotOllama).await.unwrap();
        let Err(AppError::Ollama(message)) = list(&other.url()).await else {
            panic!("expected an unreadable list");
        };
        assert!(
            message.starts_with("unexpected list of loaded models"),
            "{message}"
        );

        let url = fake.url();
        fake.stop().await;
        for result in [
            list(&url).await.map(drop),
            load(&url, "qwen3:0.6b", 0, 0).await,
            unload(&url, "qwen3:0.6b").await,
            unload_all(&url).await,
        ] {
            let Err(AppError::Ollama(message)) = result else {
                panic!("expected an unreachable server: {result:?}");
            };
            assert_eq!(message, "can't connect to Ollama");
        }
    }

    /// Serveur minimal : `/api/ps` liste `names`, `/api/generate` répond 500
    /// pour `failing` et 200 sinon ; corps reçus par `/api/generate`, dans
    /// l'ordre.
    async fn scripted(names: &[&str], failing: &'static str) -> (String, Arc<Mutex<Vec<Value>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let models: Vec<Value> = names.iter().map(|name| json!({ "name": name })).collect();
        let ps = json!({ "models": models }).to_string();
        let received = Arc::new(Mutex::new(Vec::new()));
        let journal = received.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                // En-têtes, puis le corps annoncé.
                let (head, length) = loop {
                    let read = socket.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        break (String::new(), 0);
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&request).into_owned();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        break (text[..end + 4].to_owned(), length);
                    }
                };
                while request.len() < head.len() + length {
                    let read = socket.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let (status, body) = if head.starts_with("GET /api/ps ") {
                    ("200 OK", ps.clone())
                } else {
                    let body: Value =
                        serde_json::from_slice(&request[head.len()..]).unwrap_or_default();
                    let fails = body["model"] == failing;
                    journal.lock().push(body);
                    if fails {
                        (
                            "500 Internal Server Error",
                            r#"{"error":"stuck"}"#.to_owned(),
                        )
                    } else {
                        (
                            "200 OK",
                            r#"{"done":true,"done_reason":"unload"}"#.to_owned(),
                        )
                    }
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        (format!("http://{address}"), received)
    }

    /// Un échec n'arrête pas les déchargements suivants ; la première erreur
    /// est rendue.
    #[tokio::test]
    async fn unloading_everything_goes_on_after_a_failure() {
        let (base, received) = scripted(&["a:1b", "b:latest", "c:2b"], "b:latest").await;
        let Err(AppError::Ollama(message)) = unload_all(&base).await else {
            panic!("expected the failure of b");
        };
        assert_eq!(message, "stuck");
        assert_eq!(
            *received.lock(),
            [
                json!({ "model": "a:1b", "keep_alive": 0 }),
                json!({ "model": "b:latest", "keep_alive": 0 }),
                json!({ "model": "c:2b", "keep_alive": 0 }),
            ]
        );
    }

    /// Instance Ollama de test (`scripts/test-ollama.sh`).
    pub(crate) const TEST_INSTANCE: &str = "http://127.0.0.1:11435";

    /// Pris par les contrats qui chargent un modèle de plus que ceux des
    /// autres contrats (qwen3:0.6b, qwen3.5:0.8b) : un seul à la fois, pour
    /// rester sous les 3 modèles en mémoire d'Ollama (au-delà, il en
    /// déchargerait un).
    pub(crate) static CONTRACT_MODELS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Décharge le modèle de l'instance de test à la fin du contrat, même en
    /// échec.
    pub(crate) struct Unloading(pub &'static str);

    impl Drop for Unloading {
        fn drop(&mut self) {
            let model = self.0;
            let unloaded = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(unload(TEST_INSTANCE, model))
            })
            .join();
            if !matches!(unloaded, Ok(Ok(()))) {
                eprintln!("{model} not unloaded: {unloaded:?}");
            }
        }
    }

    /// Ce modèle dans la liste, s'il y est.
    pub(crate) async fn listed(model: &str) -> Option<RunningModel> {
        let models = list(TEST_INSTANCE).await.unwrap();
        models.into_iter().find(|m| same_model(&m.name, model))
    }

    /// Décharge ce modèle et attend qu'il quitte la liste : Ollama répond
    /// aussitôt, mais décharge plus tard quand il charge un autre modèle
    /// (contrats lancés ensemble).
    pub(crate) async fn unloaded(model: &str) {
        unload(TEST_INSTANCE, model).await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while listed(model).await.is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "{model} still loaded 30 s after its unload"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Contrat : `/api/ps` de l'instance de test au format lu ; chargement
    /// avec contexte et durée, déchargement, modèle inconnu.
    #[tokio::test]
    #[ignore = "instance Ollama de test : scripts/test-ollama.sh start"]
    async fn contract_running_models_of_the_test_instance() {
        let _alone = CONTRACT_MODELS.lock().await;
        let model = "gemma3:270m";
        let _unloading = Unloading(model);
        unloaded(model).await;

        load(TEST_INSTANCE, model, 4096, 120).await.unwrap();
        // Corps brut : échéance à 9 chiffres de fraction et décalage.
        let url = url::Url::parse(TEST_INSTANCE).unwrap();
        let raw: Value = client::client(&url, Some(LIST_TIMEOUT))
            .unwrap()
            .get(format!("{TEST_INSTANCE}/api/ps"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let entry = raw["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == model)
            .unwrap_or_else(|| panic!("{model} not in {raw}"));
        let expires = entry["expires_at"].as_str().unwrap();
        assert!(timestamp(expires).is_some(), "{expires}");
        assert_eq!(entry["context_length"], 4096);
        assert_eq!(entry["details"]["parameter_size"], "268.10M");

        let running = listed(model).await.unwrap();
        assert_eq!(running.name, model);
        assert_eq!(running.context_length, Some(4096));
        assert!(running.size > 100_000_000, "{running:?}");
        assert!(running.size_vram <= running.size, "{running:?}");
        assert_eq!(running.parameter_size, "268.10M");
        assert_eq!(running.quantization_level, "Q8_0");
        assert!(!running.kept);
        let left = running.expires_at.unwrap() - now_ms();
        assert!((100_000..=120_000).contains(&left), "{left}");

        // Gardé : échéance au-delà de 2200.
        load(TEST_INSTANCE, model, 4096, -1).await.unwrap();
        let kept = listed(model).await.unwrap();
        assert!(kept.kept && kept.expires_at.is_none(), "{kept:?}");

        unloaded(model).await;
        // Sans effet sur un modèle installé qui n'est pas chargé.
        unload(TEST_INSTANCE, model).await.unwrap();
        for result in [
            load(TEST_INSTANCE, "nope:1b", 0, 0).await,
            unload(TEST_INSTANCE, "nope:1b").await,
        ] {
            let Err(AppError::NotFound(message)) = result else {
                panic!("expected a missing model: {result:?}");
            };
            assert_eq!(message, "model 'nope:1b' not found");
        }
    }
}
