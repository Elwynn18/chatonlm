//! Modèles installés dont le registre a une version plus récente (bannière
//! de la conversation, onglet Installed du ModelHub).
//!
//! Comme l'app officielle (`app/ui/ui.go:1724-1765`, `modelUpstream`) : le
//! modèle est périmé si son empreinte locale n'est pas celle que le registre
//! annonce, sauf si son manifeste a été écrit après la publication (date
//! `ollama-push-time` : un Ollama récent garde un manifeste dont l'empreinte
//! n'est pas celle de l'en-tête, vu sur 0.40.0 avec `qwen3.5:0.8b`). Un modèle
//! absent du registre (créé sur place), un registre injoignable : pas périmé.
//!
//! Écarts voulus : l'empreinte et la date locales viennent de `/api/tags`
//! (l'officiel lit le manifeste sur le disque, à côté du serveur ; ici le
//! serveur peut être distant) ; un modèle `hf.co/...` est comparé au manifeste
//! du registre de Hugging Face (`registry::upstream`, sans date : l'officiel
//! le demande à ollama.com, qui répond 404) ; un nom qui porte un autre hôte
//! (`registre.exemple/espace/nom`) n'est demandé à personne (l'officiel
//! l'envoie à ollama.com) ; plusieurs noms à la fois (liste des installés),
//! quatre requêtes en même temps au plus.

use std::time::Duration;

use futures_util::{StreamExt, stream};
use serde::Deserialize;

use super::client;
use super::models::{answered, error_message, unreachable};
use super::registry::{self, Upstream, normalize_digest};
use crate::error::{AppError, AppResult};
use crate::hub::installed::timestamp;
use crate::hub::{manifest, names};
use crate::i18n::t;

/// Délai de la liste : quelques fichiers lus par le serveur.
const LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// Vérifications au registre en même temps, au plus.
const CONCURRENCY: usize = 4;

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<Tag>,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
    #[serde(default)]
    digest: String,
    #[serde(default)]
    modified_at: Option<String>,
    #[serde(default)]
    remote_host: Option<String>,
}

/// Version installée d'un modèle.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Local {
    /// Empreinte du manifeste, à la forme de [`normalize_digest`].
    digest: String,
    /// Date du manifeste, en secondes depuis 1970, si elle est lisible.
    modified: Option<i64>,
}

/// Parmi `models` (noms tels que l'interface les montre, avec ou sans
/// `:latest`), ceux qui sont installés sur le serveur `base` et dont le
/// registre a une version plus récente, dans l'ordre reçu. Serveur
/// injoignable : erreur ; registre injoignable ou modèle inconnu de lui : pas
/// périmé.
pub async fn outdated(base: &str, registry: &str, models: &[String]) -> AppResult<Vec<String>> {
    outdated_in(base, registry, &names::hf_url(), models).await
}

/// [`outdated`], avec l'adresse de Hugging Face `hf_url`.
async fn outdated_in(
    base: &str,
    registry: &str,
    hf_url: &str,
    models: &[String],
) -> AppResult<Vec<String>> {
    let installed = installed(base).await?;
    let mut asked: Vec<(String, Vec<Local>)> = Vec::new();
    for model in models {
        if !checkable(model) || asked.iter().any(|(seen, _)| same_name(seen, model)) {
            continue;
        }
        let locals: Vec<Local> = installed
            .iter()
            .filter(|(name, _)| same_name(name, model))
            .map(|(_, local)| local.clone())
            .collect();
        if !locals.is_empty() {
            asked.push((model.clone(), locals));
        }
    }
    // Futures sans emprunt : la commande Tauri doit rester `Send`.
    let checks: Vec<_> = asked
        .into_iter()
        .map(|(model, locals)| check(registry.to_owned(), hf_url.to_owned(), model, locals))
        .collect();
    let checked: Vec<Option<String>> = stream::iter(checks).buffered(CONCURRENCY).collect().await;
    Ok(checked.into_iter().flatten().collect())
}

/// `model` s'il est périmé : toutes ses versions locales (`/api/tags` donne
/// parfois deux entrées du même nom) le sont pour le registre. Erreur
/// (registre injoignable) ou modèle absent du registre : rien de sûr, comme
/// l'officiel.
async fn check(
    registry: String,
    hf_url: String,
    model: String,
    locals: Vec<Local>,
) -> Option<String> {
    let upstream = registry::upstream_in(&registry, &hf_url, &model)
        .await
        .ok()??;
    locals
        .iter()
        .all(|local| stale(local, &upstream))
        .then_some(model)
}

/// Règle de l'officiel : périmé, sauf si l'empreinte locale est celle du
/// registre, ou si le manifeste local a été écrit à la date de publication ou
/// après.
fn stale(local: &Local, upstream: &Upstream) -> bool {
    if local.digest == upstream.digest {
        return false;
    }
    !matches!(
        (upstream.push_time, local.modified),
        (Some(push), Some(modified)) if modified >= push
    )
}

/// Vrai si `model` peut être demandé à un registre : un nom de Hugging Face,
/// ou un nom d'ollama.com (`nom` ou `espace/nom`). Un nom qui porte un autre
/// hôte (deux `/`) reste sur la machine.
fn checkable(model: &str) -> bool {
    manifest::is_hugging_face(model) || model.matches('/').count() <= 1
}

/// Nom comme Ollama le résout : sans la casse, étiquette `latest` à défaut
/// (cherchée après le dernier `/` : un hôte peut porter un port).
fn canonical(model: &str) -> String {
    let model = model.trim().to_ascii_lowercase();
    let tagged = model
        .rsplit('/')
        .next()
        .is_some_and(|last| last.contains(':'));
    if tagged {
        model
    } else {
        format!("{model}:latest")
    }
}

fn same_name(a: &str, b: &str) -> bool {
    canonical(a) == canonical(b)
}

/// Modèles locaux du serveur `base` (`/api/tags`) qui ont une empreinte : nom
/// et version. Les modèles distants (`remote_host`, cloud) n'ont pas de
/// fichiers ici.
async fn installed(base: &str) -> AppResult<Vec<(String, Local)>> {
    let url = client::server_url(base)?;
    let client = client::client(&url, Some(LIST_TIMEOUT)).map_err(unreachable)?;
    let response = client
        .get(format!("{base}/api/tags"))
        .send()
        .await
        .map_err(unreachable)?;
    let status = response.status();
    let body = response.text().await.map_err(unreachable)?;
    if !status.is_success() {
        return Err(answered(status.as_u16(), &error_message(&body)));
    }
    parse(&body)
}

fn parse(body: &str) -> AppResult<Vec<(String, Local)>> {
    let tags: TagsResponse = serde_json::from_str(body)
        .map_err(|error| AppError::Ollama(t!("ollama.unexpectedModels", error = error)))?;
    Ok(tags
        .models
        .into_iter()
        .filter(|tag| tag.remote_host.as_deref().is_none_or(str::is_empty))
        .filter_map(|tag| {
            let digest = normalize_digest(&tag.digest);
            (!digest.is_empty()).then(|| {
                let modified = tag
                    .modified_at
                    .as_deref()
                    .and_then(timestamp)
                    .and_then(|nanos| i64::try_from(nanos.div_euclid(1_000_000_000)).ok());
                (tag.name, Local { digest, modified })
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use fake_ollama::{FakeOllama, Mode};

    use super::*;

    const HF_MODEL: &str = "hf.co/chatonlm/Tiny-GGUF:Q4_K_M";

    fn local(digest: &str, modified: Option<i64>) -> Local {
        Local {
            digest: digest.to_owned(),
            modified,
        }
    }

    fn upstream(digest: &str, push_time: Option<i64>) -> Upstream {
        Upstream {
            digest: digest.to_owned(),
            push_time,
        }
    }

    fn list(models: &[&str]) -> Vec<String> {
        models.iter().map(|model| (*model).to_owned()).collect()
    }

    /// Télécharge `model` sur le faux serveur, jusqu'à la fin du flux.
    async fn pull(fake: &FakeOllama, model: &str) {
        let url = client::server_url(&fake.url()).unwrap();
        let body = client::client(&url, Some(Duration::from_secs(10)))
            .unwrap()
            .post(format!("{}/api/pull", fake.url()))
            .json(&serde_json::json!({ "model": model }))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(body.contains(r#""status":"success""#), "{model}: {body}");
    }

    /// La règle de `modelUpstream` de l'officiel.
    #[test]
    fn a_model_is_stale_unless_it_is_the_registry_one_or_newer() {
        // Même empreinte : à jour, quelles que soient les dates.
        for (modified, push) in [(None, None), (Some(10), Some(20)), (Some(20), Some(10))] {
            assert!(!stale(&local("aa", modified), &upstream("aa", push)));
        }
        // Autre empreinte, manifeste local écrit à la publication ou après
        // (manifeste qu'un Ollama récent garde sous une autre empreinte).
        assert!(!stale(&local("aa", Some(20)), &upstream("bb", Some(20))));
        assert!(!stale(&local("aa", Some(21)), &upstream("bb", Some(20))));
        // Autre empreinte, publiée après : périmé.
        assert!(stale(&local("aa", Some(19)), &upstream("bb", Some(20))));
        // Sans l'une des deux dates (Hugging Face n'en donne pas) :
        // l'empreinte décide.
        assert!(stale(&local("aa", Some(20)), &upstream("bb", None)));
        assert!(stale(&local("aa", None), &upstream("bb", Some(20))));
        assert!(stale(&local("aa", None), &upstream("bb", None)));
    }

    #[test]
    fn names_are_compared_like_ollama_does() {
        for (a, b) in [
            ("gemma4", "gemma4:latest"),
            ("Gemma4:LATEST", " gemma4 "),
            ("qwen3:0.6b", "QWEN3:0.6B"),
            ("hf.co/a/b", "HF.co/a/b:latest"),
            ("localhost:5000/a/b", "localhost:5000/a/b:latest"),
        ] {
            assert!(same_name(a, b), "{a} / {b}");
        }
        for (a, b) in [
            ("gemma4", "gemma4:27b"),
            ("qwen3:0.6b", "qwen3:8b"),
            ("gemma4", "someone/gemma4"),
            ("localhost:5000/a/b", "localhost/a/b:5000"),
        ] {
            assert!(!same_name(a, b), "{a} / {b}");
        }
    }

    /// Un nom qui porte un autre hôte n'est demandé à aucun registre.
    #[test]
    fn only_ollama_and_hugging_face_names_are_asked() {
        for model in [
            "gemma4",
            "qwen3:0.6b",
            "someone/model:q4_K_M",
            "hf.co/a/b:Q4_K_M",
            "HuggingFace.co/a/b",
        ] {
            assert!(checkable(model), "{model}");
        }
        for model in [
            "registry.example.com/team/model",
            "localhost:5000/team/model:v2",
            "registry.ollama.ai/library/gemma4:latest",
        ] {
            assert!(!checkable(model), "{model}");
        }
    }

    #[test]
    fn local_versions_are_read_from_the_tags() {
        let body = r#"{"models":[
            {"name":"qwen3:0.6b","digest":"sha256:7DF6","modified_at":"2026-10-03T20:24:54.6278456+02:00"},
            {"name":"old:latest","digest":"ab12","modified_at":"never"},
            {"name":"dateless","digest":"cd34"},
            {"name":"no-digest:latest","digest":"","modified_at":"2026-10-03T20:24:54Z"},
            {"name":"absent-digest"},
            {"name":"gpt-oss:120b-cloud","digest":"ef56","remote_host":"https://ollama.com:443"},
            {"name":"here","digest":"0123","remote_host":""}]}"#;
        assert_eq!(
            parse(body).unwrap(),
            [
                // 2026-10-03T18:24:54Z.
                ("qwen3:0.6b".to_owned(), local("7df6", Some(1_791_051_894))),
                ("old:latest".to_owned(), local("ab12", None)),
                ("dateless".to_owned(), local("cd34", None)),
                ("here".to_owned(), local("0123", None)),
            ]
        );
        assert!(matches!(parse("<html>"), Err(AppError::Ollama(_))));
    }

    /// Faux serveur et faux registre : un modèle installé n'est périmé
    /// qu'après la publication d'une nouvelle version, et ne l'est plus après
    /// son téléchargement.
    #[tokio::test]
    async fn a_new_version_in_the_registry_makes_an_installed_model_stale() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let url = fake.url();
        let check = |models: &[&str]| {
            let (url, models) = (url.clone(), list(models));
            async move { outdated_in(&url, &url, &url, &models).await.unwrap() }
        };
        pull(&fake, "chatonlm-pull").await;
        let all = [
            // Absent du registre (404).
            "qwen3:0.6b",
            // Au registre sous une autre empreinte, publiée avant son
            // téléchargement.
            "qwen3.5:0.8b",
            // Même empreinte.
            "chatonlm-pull",
            // Au registre, pas installés : jamais demandés.
            "gemma4",
            "llama3.2:3b",
        ];
        assert_eq!(check(&all).await, [""; 0]);
        let mut seen = fake.manifest_requests();
        seen.sort();
        assert_eq!(
            seen,
            [
                "HEAD /v2/library/chatonlm-pull/manifests/latest",
                "HEAD /v2/library/qwen3.5/manifests/0.8b",
                "HEAD /v2/library/qwen3/manifests/0.6b",
            ]
        );

        // Nouvelles versions publiées : périmés, dans l'ordre demandé, sous
        // le nom reçu ; un nom répété n'est demandé qu'une fois.
        fake.publish("chatonlm-pull");
        fake.publish("qwen3.5:0.8b");
        assert_eq!(check(&all).await, ["qwen3.5:0.8b", "chatonlm-pull"]);
        assert_eq!(
            check(&["chatonlm-pull:latest", "chatonlm-pull", "qwen3:0.6b"]).await,
            ["chatonlm-pull:latest"]
        );
        assert_eq!(fake.manifest_requests().len(), 3 + 3 + 2);

        // Téléchargé de nouveau : à jour ; l'autre reste périmé.
        pull(&fake, "chatonlm-pull").await;
        assert_eq!(check(&all).await, ["qwen3.5:0.8b"]);
        // Rien à demander : aucune requête.
        let before = fake.manifest_requests().len();
        assert_eq!(check(&[]).await, [""; 0]);
        assert_eq!(
            check(&["gemma4", "registry.example.com/a/qwen3:0.6b"]).await,
            [""; 0]
        );
        assert_eq!(fake.manifest_requests().len(), before);
    }

    /// Modèle de Hugging Face : comparé au manifeste de son registre (GET),
    /// dont l'empreinte est celle des octets servis.
    #[tokio::test]
    async fn a_hugging_face_model_is_compared_with_its_own_registry() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let ollama_registry = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (url, registry) = (fake.url(), ollama_registry.url());
        let check = || {
            let (url, registry) = (url.clone(), registry.clone());
            async move {
                outdated_in(&url, &registry, &url, &list(&[HF_MODEL]))
                    .await
                    .unwrap()
            }
        };
        pull(&fake, HF_MODEL).await;
        assert_eq!(check().await, [""; 0]);
        assert_eq!(
            fake.manifest_requests(),
            ["GET /v2/chatonlm/Tiny-GGUF/manifests/Q4_K_M"]
        );
        fake.publish(HF_MODEL);
        assert_eq!(check().await, [HF_MODEL]);
        pull(&fake, HF_MODEL).await;
        assert_eq!(check().await, [""; 0]);
        // Jamais demandé au registre d'Ollama.
        assert!(ollama_registry.manifest_requests().is_empty());
    }

    /// Registre injoignable : rien n'est périmé (pas d'erreur) ; serveur
    /// Ollama injoignable : erreur.
    #[tokio::test]
    async fn an_unreachable_registry_says_nothing() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let registry = FakeOllama::start(Mode::ollama()).await.unwrap();
        let url = fake.url();
        pull(&fake, "chatonlm-pull").await;
        pull(&fake, HF_MODEL).await;
        registry.publish("chatonlm-pull");
        registry.publish(HF_MODEL);
        let models = list(&["chatonlm-pull", HF_MODEL]);
        let check = |registry: String| {
            let (url, models) = (url.clone(), models.clone());
            async move { outdated_in(&url, &registry, &registry, &models).await }
        };
        assert_eq!(check(registry.url()).await.unwrap(), models);
        // Réponses sans les en-têtes du registre, ou en erreur.
        for code in [200, 404, 500] {
            let other = FakeOllama::start(Mode::Status(code)).await.unwrap();
            assert_eq!(check(other.url()).await.unwrap(), [""; 0], "{code}");
        }
        let closed = registry.url();
        registry.stop().await;
        assert_eq!(check(closed).await.unwrap(), [""; 0]);

        fake.stop().await;
        assert!(matches!(
            outdated_in(&url, &url, &url, &models).await,
            Err(AppError::Ollama(_))
        ));
    }

    /// Contrat : l'instance de test et les vrais registres (HEAD et GET de
    /// manifestes, aucun téléchargement). L'empreinte de `/api/tags` est celle
    /// que le registre annonce pour un modèle inchangé : `outdated` ne signale
    /// un modèle que si le registre l'a republié depuis son téléchargement.
    #[tokio::test]
    #[ignore = "instance Ollama de test (scripts/test-ollama.sh start), ollama.com et Hugging Face : accès au réseau"]
    async fn contract_installed_models_against_the_registries() {
        const TEST_INSTANCE: &str = "http://127.0.0.1:11435";
        const HF_REAL: &str = "hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M";
        let installed = installed(TEST_INSTANCE).await.unwrap();
        let find = |model: &str| {
            installed
                .iter()
                .find(|(name, _)| same_name(name, model))
                .map(|(_, local)| local.clone())
        };
        let models = list(&["qwen3:0.6b", "gemma3:270m", HF_REAL]);
        let stale_now = outdated(TEST_INSTANCE, registry::DEFAULT_REGISTRY, &models)
            .await
            .unwrap();
        for model in ["qwen3:0.6b", "gemma3:270m"] {
            let local = find(model).unwrap_or_else(|| panic!("{model} is not installed"));
            assert_eq!(local.digest.len(), 64, "{model}");
            let modified = local.modified.expect("a date");
            let upstream = registry::upstream(registry::DEFAULT_REGISTRY, model)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{model} is not in the registry"));
            let push = upstream.push_time.expect("a push time");
            // Téléchargé après la publication : c'est le manifeste du registre.
            if modified >= push {
                assert_eq!(local.digest, upstream.digest, "{model}");
            }
            assert_eq!(
                stale_now.iter().any(|name| name == model),
                local.digest != upstream.digest && modified < push,
                "{model}"
            );
        }
        // Hugging Face : l'empreinte locale est celle des octets du manifeste.
        match find(HF_REAL) {
            Some(local) => {
                let upstream = registry::upstream_in(
                    registry::DEFAULT_REGISTRY,
                    names::DEFAULT_HF_URL,
                    HF_REAL,
                )
                .await
                .unwrap()
                .expect("a manifest");
                assert_eq!(upstream.push_time, None);
                assert_eq!(
                    stale_now.iter().any(|name| name == HF_REAL),
                    local.digest != upstream.digest
                );
                assert_eq!(local.digest, upstream.digest, "{HF_REAL} changed upstream?");
            }
            None => eprintln!("{HF_REAL} is not installed: not compared"),
        }
    }
}
