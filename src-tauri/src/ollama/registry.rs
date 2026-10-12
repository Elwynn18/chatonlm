//! Registre des modèles d'Ollama (ollama.com) : un nom tapé dans le sélecteur
//! existe-t-il en ligne ? Comme l'app officielle (`app/ui/ui.go:540-591`,
//! `checkModelUpstream`) : HEAD de son manifeste, vrai seulement avec
//! l'en-tête `ollama-content-digest`.
//!
//! Écart voulu : un nom `hf.co/<auteur>/<dépôt>[:<étiquette>]` est vérifié au
//! registre de Hugging Face (`hub::manifest`, GET avec l'en-tête `Accept`,
//! 200 et un manifeste avec `layers`), qui n'envoie pas
//! `ollama-content-digest` ; l'officiel le demande à ollama.com, qui répond
//! toujours 404 (`34-modelhub-ollama-hf.md` § 10).

use std::time::Duration;

use url::Url;

use super::client;
use crate::error::{AppError, AppResult};
use crate::hub::manifest::{self, Manifest};
use crate::hub::names::{self, parse_hf_name};

/// Registre par défaut, comme `OllamaDotCom` de l'officiel.
pub const DEFAULT_REGISTRY: &str = "https://ollama.com";

/// Délai de la vérification, comme l'officiel.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Adresse du registre : `CHATONLM_REGISTRY_URL` si elle est définie et non
/// vide (builds de débogage et d'essai seulement : tests de l'app réelle,
/// comme `OLLAMA_DOT_COM_URL` pour l'officiel), sinon ollama.com.
pub fn registry_url() -> String {
    registry_from(test_override("CHATONLM_REGISTRY_URL"))
}

/// Adresse imposée par les essais (faux registre), comme dans
/// `hub/names.rs` : jamais lue par le build de production, où un
/// environnement modifié enverrait sinon les noms de modèles cherchés vers
/// un hôte quelconque.
#[cfg(any(test, debug_assertions, feature = "e2e"))]
fn test_override(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(not(any(test, debug_assertions, feature = "e2e")))]
fn test_override(_name: &str) -> Option<String> {
    None
}

fn registry_from(value: Option<String>) -> String {
    value
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_owned())
}

/// Chemin du manifeste sous `/v2/`, comme l'officiel : étiquette `latest` par
/// défaut, préfixe `library/` sans `/`. Écart : l'étiquette est tout ce qui
/// suit le premier `:` (l'officiel garde seulement le deuxième morceau de
/// `split(":")`, et propose alors un nom qu'on ne peut pas télécharger). `None`
/// pour un nom rogné vide, un caractère hors de `[A-Za-z0-9._:/-]` ou un
/// segment vide, `.` ou `..` (le chemin resterait celui du manifeste).
pub(crate) fn manifest_path(model: &str) -> Option<String> {
    let model = model.trim();
    let allowed = |c: char| c.is_ascii_alphanumeric() || "._:/-".contains(c);
    if model.is_empty() || !model.chars().all(allowed) {
        return None;
    }
    let (name, tag) = model.split_once(':').unwrap_or((model, "latest"));
    let path = if name.contains('/') {
        format!("{name}/manifests/{tag}")
    } else {
        format!("library/{name}/manifests/{tag}")
    };
    path.split('/')
        .all(|segment| !matches!(segment, "" | "." | ".."))
        .then_some(path)
}

/// Vrai si `model` existe au registre `registry` : HEAD de son manifeste,
/// réponse 200 avec un en-tête `ollama-content-digest` non vide. Nom invalide :
/// faux, sans requête ; autre statut : faux ; échec réseau : erreur (que
/// l'interface prend pour un « non »). Un nom `hf.co/...` est demandé au
/// registre de Hugging Face (`names::hf_url`).
pub async fn model_exists(registry: &str, model: &str) -> AppResult<bool> {
    model_exists_in(registry, &names::hf_url(), model).await
}

/// [`model_exists`], avec l'adresse de Hugging Face `hf_url` : nom `hf.co/...`
/// invalide → faux sans requête ; manifeste 200 lisible → vrai ; refus
/// (400, 401), autre statut ou réponse illisible → faux ; pas de réponse →
/// erreur.
async fn model_exists_in(registry: &str, hf_url: &str, model: &str) -> AppResult<bool> {
    if manifest::is_hugging_face(model) {
        let Some(name) = parse_hf_name(model) else {
            return Ok(false);
        };
        return match manifest::fetch(hf_url, &name, TIMEOUT).await {
            Manifest::Found { .. } => Ok(true),
            Manifest::Refused(_) | Manifest::Unexpected => Ok(false),
            Manifest::Unreachable(message) => Err(AppError::Ollama(message)),
        };
    }
    let Some(path) = manifest_path(model) else {
        return Ok(false);
    };
    let url = Url::parse(&format!("{registry}/v2/{path}"))
        // i18n-ignore : adresse fixe (ou celle des essais).
        .map_err(|_| AppError::Invalid(format!("invalid registry address: {registry}")))?;
    // Proxy du système permis (sauf vers la machine locale), même agent.
    let client = client::client(&url, Some(TIMEOUT)).map_err(failed)?;
    let response = client.head(url).send().await.map_err(failed)?;
    Ok(found(response.status(), response.headers()))
}

/// Réponse d'un manifeste présent : 200 avec un `ollama-content-digest` non
/// vide.
fn found(status: reqwest::StatusCode, headers: &reqwest::header::HeaderMap) -> bool {
    status == reqwest::StatusCode::OK
        && headers
            .get("ollama-content-digest")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| !value.trim().is_empty())
}

fn failed(error: reqwest::Error) -> AppError {
    let error = error.without_url();
    AppError::Ollama(if error.is_timeout() {
        crate::i18n::t!("ollama.registryTimeout")
    } else {
        crate::i18n::t!("ollama.registryUnreachable", error = error)
    })
}

#[cfg(test)]
mod tests {
    use fake_ollama::{FakeOllama, Mode};

    use super::*;

    #[test]
    fn the_registry_can_be_replaced() {
        assert_eq!(registry_from(None), DEFAULT_REGISTRY);
        assert_eq!(registry_from(Some(String::new())), DEFAULT_REGISTRY);
        assert_eq!(registry_from(Some(" \n".into())), DEFAULT_REGISTRY);
        assert_eq!(
            registry_from(Some("http://127.0.0.1:1/".into())),
            "http://127.0.0.1:1"
        );
    }

    #[test]
    fn names_become_manifest_paths_like_the_official_app() {
        for (model, path) in [
            ("gemma4", "library/gemma4/manifests/latest"),
            ("qwen3:0.6b", "library/qwen3/manifests/0.6b"),
            ("llama3.2", "library/llama3.2/manifests/latest"),
            (" qwen3.5:9b \n", "library/qwen3.5/manifests/9b"),
            ("someone/model:q4_K_M", "someone/model/manifests/q4_K_M"),
            (
                "hf.co/org/model-GGUF",
                "hf.co/org/model-GGUF/manifests/latest",
            ),
            // Découpé au premier « : » ; le reste reste dans l'étiquette.
            ("qwen3:0.6b:extra", "library/qwen3/manifests/0.6b:extra"),
        ] {
            assert_eq!(manifest_path(model).as_deref(), Some(path), "{model}");
        }
    }

    #[test]
    fn invalid_names_are_never_asked_for() {
        for model in [
            "",
            "  ",
            "gemma 4",
            "gemmä",
            "gemma4?x=1",
            "gemma4#x",
            "gemma%2e",
            "a\\b",
            "../gemma4",
            "library/../../x",
            "a/./b",
            "a//b",
            "/gemma4",
            "gemma4/",
            ":latest",
            "gemma4:",
            "gemma4:..",
        ] {
            assert_eq!(manifest_path(model), None, "{model:?}");
        }
    }

    #[test]
    fn only_a_200_with_a_digest_is_a_model() {
        use reqwest::StatusCode;
        use reqwest::header::{HeaderMap, HeaderValue};
        let with = |digest: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert("ollama-content-digest", HeaderValue::from_static(digest));
            headers
        };
        let digest = "7df6b6e09427a769808717c0a93cadc4ae99ed4eb8bf5ca557c90846becea435";
        assert!(found(StatusCode::OK, &with(digest)));
        assert!(!found(StatusCode::OK, &HeaderMap::new()));
        assert!(!found(StatusCode::OK, &with("")));
        assert!(!found(StatusCode::OK, &with("  ")));
        for status in [
            StatusCode::NO_CONTENT,
            StatusCode::NOT_FOUND,
            StatusCode::UNAUTHORIZED,
        ] {
            assert!(!found(status, &with(digest)), "{status}");
        }
    }

    #[tokio::test]
    async fn a_model_exists_with_a_digest_only() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        for model in ["llama3.2", "gemma4:27b", "chatonlm-pull", "qwen3.5"] {
            assert!(model_exists(&fake.url(), model).await.unwrap(), "{model}");
        }
        // 404 du registre.
        for model in ["qwen3", "someone/gemma4", "chatonlm"] {
            assert!(!model_exists(&fake.url(), model).await.unwrap(), "{model}");
        }
        // HEAD seulement, au chemin de l'officiel.
        let seen = fake.manifest_requests();
        assert_eq!(seen.len(), 7);
        assert_eq!(seen[0], "HEAD /v2/library/llama3.2/manifests/latest");
        assert_eq!(seen[1], "HEAD /v2/library/gemma4/manifests/27b");
        assert_eq!(seen[5], "HEAD /v2/someone/gemma4/manifests/latest");
        assert!(seen.iter().all(|line| line.starts_with("HEAD ")));
        // Nom invalide : faux, sans requête.
        for model in ["", "gemma 4", "../gemma4"] {
            assert!(
                !model_exists(&fake.url(), model).await.unwrap(),
                "{model:?}"
            );
        }
        assert_eq!(fake.manifest_requests().len(), 7);

        // 200 sans l'en-tête (autre serveur, proxy), autre statut : faux.
        for code in [200, 404, 500] {
            let other = FakeOllama::start(Mode::Status(code)).await.unwrap();
            assert!(
                !model_exists(&other.url(), "gemma4").await.unwrap(),
                "{code}"
            );
        }
        let page = FakeOllama::start(Mode::NotOllama).await.unwrap();
        assert!(!model_exists(&page.url(), "gemma4").await.unwrap());
    }

    #[tokio::test]
    async fn network_failures_are_errors() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let url = fake.url();
        fake.stop().await;
        let Err(AppError::Ollama(message)) = model_exists(&url, "gemma4").await else {
            panic!("expected an error");
        };
        assert!(
            message.starts_with("can't reach the model registry"),
            "{message}"
        );
        assert!(!message.contains("127.0.0.1"), "{message}");
        assert!(matches!(
            model_exists("not a url", "gemma4").await,
            Err(AppError::Invalid(_))
        ));
    }

    /// Nom `hf.co/...` : demandé au registre de Hugging Face (espace
    /// `chatonlm` du faux serveur), jamais à celui d'Ollama.
    #[tokio::test]
    async fn hugging_face_names_are_asked_to_hugging_face() {
        let hf = FakeOllama::start(Mode::ollama()).await.unwrap();
        let ollama = FakeOllama::start(Mode::ollama()).await.unwrap();
        let exists = |model: &'static str| {
            let (registry, hf) = (ollama.url(), hf.url());
            async move { model_exists_in(&registry, &hf, model).await }
        };
        for model in [
            "hf.co/chatonlm/Tiny-GGUF:Q4_K_M",
            "HF.CO/chatonlm/Eye-vision-GGUF:q8_0",
            "huggingface.co/chatonlm/Tiny-GGUF:Q4_K_M",
            // Sans étiquette : `latest`, comme Ollama.
            "hf.co/chatonlm/Tiny-GGUF",
        ] {
            assert!(exists(model).await.unwrap(), "{model}");
        }
        // 400, 401 (inexistant, restreint), page HTML : non.
        for model in [
            "hf.co/chatonlm/Big-sharded-GGUF:Q8_0",
            "hf.co/chatonlm/Tiny-GGUF:badtag",
            "hf.co/chatonlm/Gone-missing-GGUF:Q4_K_M",
            "hf.co/chatonlm/Lab-restricted-GGUF:Q4_K_M",
            "hf.co/chatonlm/Down-html-GGUF:Q4_K_M",
        ] {
            assert!(!exists(model).await.unwrap(), "{model}");
        }
        let seen = hf.manifest_requests();
        assert_eq!(seen.len(), 9);
        assert_eq!(seen[0], "GET /v2/chatonlm/Tiny-GGUF/manifests/Q4_K_M");
        assert_eq!(seen[3], "GET /v2/chatonlm/Tiny-GGUF/manifests/latest");
        // Nom refusé par Ollama : non, sans requête.
        assert!(!exists("hf.co/bart.owski/x:y").await.unwrap());
        assert_eq!(hf.manifest_requests().len(), 9);
        assert!(ollama.manifest_requests().is_empty());
        // Pas de réponse : erreur.
        let url = hf.url();
        hf.stop().await;
        let Err(AppError::Ollama(message)) =
            model_exists_in(&ollama.url(), &url, "hf.co/chatonlm/Tiny-GGUF:Q4_K_M").await
        else {
            panic!("expected an error");
        };
        assert!(message.starts_with("can't reach Hugging Face"), "{message}");
    }

    /// Contrat : le vrai registre (simples HEAD, aucun téléchargement).
    #[tokio::test]
    #[ignore = "registre ollama.com : accès au réseau"]
    async fn contract_with_the_ollama_registry() {
        assert!(model_exists(DEFAULT_REGISTRY, "qwen3:0.6b").await.unwrap());
        assert!(
            !model_exists(DEFAULT_REGISTRY, "chatonlm-does-not-exist-xyz")
                .await
                .unwrap()
        );
    }

    /// Contrat : le registre de Hugging Face (deux GET de manifeste, aucun
    /// téléchargement) ; une version absente du dépôt n'existe pas.
    #[tokio::test]
    #[ignore = "registre de Hugging Face : accès au réseau"]
    async fn contract_with_the_hugging_face_registry() {
        let hf = names::DEFAULT_HF_URL;
        let model = "hf.co/bartowski/SmolLM2-135M-Instruct-GGUF";
        let at = |tag: &str| format!("{model}:{tag}");
        assert!(
            model_exists_in(DEFAULT_REGISTRY, hf, &at("Q4_K_M"))
                .await
                .unwrap()
        );
        assert!(
            !model_exists_in(DEFAULT_REGISTRY, hf, &at("bf16"))
                .await
                .unwrap()
        );
    }
}
