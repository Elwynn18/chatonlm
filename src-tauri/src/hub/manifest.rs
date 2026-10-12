//! Manifeste d'une version au registre de Hugging Face, lu comme Ollama le
//! fait avant un pull `hf.co/...` : `GET <HF>/v2/<auteur>/<dépôt>/manifests/<étiquette>`
//! avec l'en-tête `Accept` du manifeste Docker v2, redirections suivies (`hf.co`
//! répond 307 vers `huggingface.co`). Le registre n'envoie pas d'en-tête
//! d'empreinte : seuls le statut 200 et un JSON avec `layers` comptent
//! (`34-modelhub-ollama-hf.md` § 2 et § 10, `35-modelhub-hf-api.md` § 2.4).
//!
//! Sert à deux choses : connaître la taille exacte d'un téléchargement (couches
//! et configuration, projecteur de vision compris) et refuser tout de suite une
//! version que le registre ne servira pas (400, 401), avec un texte lisible.
//!
//! Limite assumée : ces requêtes ne passent pas par `hub::hf::Hf` et ignorent
//! le quota partagé après une réponse 429 du catalogue (contrat § 3) ; un 429
//! du registre ne bloque pas non plus le catalogue. Elles sont rares (un clic
//! Download, un pull `hf.co` du chat, une vérification du sélecteur) et
//! qu'elles partent pendant une limite ne coûte qu'une réponse `Unexpected`.

use std::time::Duration;

use serde::Deserialize;
use url::Url;

use super::errors;
use super::names::{HF_HOST, HfName};
use crate::i18n::t;
use crate::ollama::client;

/// En-tête `Accept` envoyé par Ollama (`server/images.go`).
pub const ACCEPT: &str = "application/vnd.docker.distribution.manifest.v2+json";

/// Taille lue au plus pour la réponse (un manifeste fait moins de 2 Kio).
const MAX_BODY: usize = 256 * 1024;

/// Réponse du registre.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Manifest {
    /// 200 et un manifeste lisible : octets à télécharger (couches et
    /// configuration, comme la taille sur le disque d'Ollama).
    Found { total: u64 },
    /// 400 ou 401 : cette version ne se téléchargera pas ; texte lisible.
    Refused(String),
    /// Autre statut (404, 429, 5xx) ou réponse illisible (page HTML d'un
    /// CDN) : rien de sûr.
    Unexpected,
    /// Aucune réponse (réseau, délai) ; message lisible.
    Unreachable(String),
}

#[derive(Deserialize)]
struct Body {
    #[serde(default)]
    config: Option<Blob>,
    layers: Vec<Blob>,
}

#[derive(Deserialize)]
struct Blob {
    #[serde(default)]
    size: u64,
}

/// Vrai pour un nom d'hôte Hugging Face (`hf.co/…`, `huggingface.co/…`,
/// sans la casse), valide ou non : son manifeste est au registre de Hugging
/// Face, jamais à celui d'Ollama.
pub fn is_hugging_face(model: &str) -> bool {
    model.trim().split_once('/').is_some_and(|(host, _)| {
        host.eq_ignore_ascii_case(HF_HOST) || host.eq_ignore_ascii_case("huggingface.co")
    })
}

/// Adresse du manifeste de `name` (étiquette `latest` à défaut, comme Ollama).
pub fn url(hf_url: &str, name: &HfName) -> String {
    let tag = name.tag.as_deref().unwrap_or("latest");
    format!("{hf_url}/v2/{}/{}/manifests/{tag}", name.author, name.repo)
}

/// Lit le manifeste de `name` au registre `hf_url` (sans `/` final), en
/// `timeout` au plus.
pub async fn fetch(hf_url: &str, name: &HfName, timeout: Duration) -> Manifest {
    let Ok(url) = Url::parse(&url(hf_url, name)) else {
        return Manifest::Unreachable(format!("invalid Hugging Face address: {hf_url}"));
    };
    // Proxy du système permis (sauf vers la machine locale), même agent.
    let client = match client::client(&url, Some(timeout)) {
        Ok(client) => client,
        Err(error) => return Manifest::Unreachable(unreachable(&error)),
    };
    let mut response = match client
        .get(url)
        .header(reqwest::header::ACCEPT, ACCEPT)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => return Manifest::Unreachable(unreachable(&error)),
    };
    let status = response.status();
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk);
                if body.len() > MAX_BODY {
                    return Manifest::Unexpected;
                }
            }
            Ok(None) => break,
            Err(error) => return Manifest::Unreachable(unreachable(&error)),
        }
    }
    read(status, &body)
}

/// Sens d'une réponse du registre.
fn read(status: reqwest::StatusCode, body: &[u8]) -> Manifest {
    match status.as_u16() {
        200 => match serde_json::from_slice::<Body>(body) {
            Ok(manifest) => Manifest::Found {
                total: manifest
                    .layers
                    .iter()
                    .chain(manifest.config.as_ref())
                    .map(|blob| blob.size)
                    .sum(),
            },
            Err(_) => Manifest::Unexpected,
        },
        // Dépôt inexistant, privé ou restreint : le même 401 en anonyme.
        401 => Manifest::Refused(errors::not_found_or_private()),
        400 => {
            let message = serde_json::from_slice::<serde_json::Value>(body)
                .ok()
                .and_then(|value| value.get("error")?.as_str().map(str::to_owned))
                .unwrap_or_else(|| String::from_utf8_lossy(body).chars().take(200).collect());
            Manifest::Refused(if message.trim().is_empty() {
                errors::unavailable_version()
            } else {
                errors::pull_error(&message)
            })
        }
        _ => Manifest::Unexpected,
    }
}

fn unreachable(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        t!("downloads.hfTimeout")
    } else {
        t!("downloads.hfUnreachable", error = error_text(error))
    }
}

/// Message d'une erreur de reqwest sans l'adresse, avec ses causes.
pub(crate) fn error_text(error: &reqwest::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = std::error::Error::source(error);
    while let Some(current) = source {
        let text = current.to_string();
        if !text.is_empty() && !parts.iter().any(|part| part.contains(&text)) {
            parts.push(text);
        }
        source = current.source();
    }
    let text = parts.join(": ");
    match error.url() {
        Some(url) => text.replace(&format!(" ({url})"), ""),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use fake_ollama::{FakeOllama, Mode};

    use super::*;
    use crate::hub::errors::english::{
        NOT_FOUND_OR_PRIVATE, NOT_GGUF, SHARDED, UNAVAILABLE_VERSION,
    };
    use crate::hub::names::parse_hf_name;

    const WAIT: Duration = Duration::from_secs(5);

    fn name(model: &str) -> HfName {
        parse_hf_name(model).unwrap()
    }

    #[test]
    fn hugging_face_hosts() {
        for model in [
            "hf.co/a/b:c",
            " HF.CO/a",
            "huggingface.co/a/b",
            "hf.co/bart.owski/x:y",
        ] {
            assert!(is_hugging_face(model), "{model}");
        }
        for model in [
            "qwen3:0.6b",
            "library/qwen3",
            "hf.com/a/b",
            "hf.co",
            "x/hf.co/a",
        ] {
            assert!(!is_hugging_face(model), "{model}");
        }
    }

    #[test]
    fn the_address_is_the_one_ollama_asks() {
        assert_eq!(
            url(
                "https://huggingface.co",
                &name("hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M")
            ),
            "https://huggingface.co/v2/bartowski/SmolLM2-135M-Instruct-GGUF/manifests/Q4_K_M"
        );
        assert_eq!(
            url("http://127.0.0.1:9", &name("HF.CO/a/b")),
            "http://127.0.0.1:9/v2/a/b/manifests/latest"
        );
    }

    /// Échantillons réels (`.tools/research/35-samples` et `34-samples`,
    /// recopiés) : total = couches + configuration.
    #[test]
    fn real_manifests_are_read() {
        let ok = reqwest::StatusCode::OK;
        // unsloth/Qwen3.5-9B-GGUF:UD-Q4_K_XL (empreintes abrégées).
        let ud = r#"{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json",
            "config":{"digest":"sha256:8250","mediaType":"application/vnd.docker.container.image.v1+json","size":481},
            "layers":[{"digest":"sha256:6f5d","mediaType":"application/vnd.ollama.image.model","size":5966095584},
            {"digest":"sha256:8536","mediaType":"application/vnd.ollama.image.projector","size":921705024}]}"#;
        assert_eq!(
            read(ok, ud.as_bytes()),
            Manifest::Found {
                total: 5_966_095_584 + 921_705_024 + 481
            }
        );
        // unsloth/gemma-3-4b-it-GGUF:Q4_K_M : modèle, gabarit, projecteur,
        // paramètres.
        let vision = r#"{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json",
            "config":{"digest":"sha256:ec64","mediaType":"application/vnd.docker.container.image.v1+json","size":629},
            "layers":[{"digest":"sha256:04a4","mediaType":"application/vnd.ollama.image.model","size":2489894016},
            {"digest":"sha256:4b19","mediaType":"application/vnd.ollama.image.template","size":476},
            {"digest":"sha256:7311","mediaType":"application/vnd.ollama.image.projector","size":851251328},
            {"digest":"sha256:39bf","mediaType":"application/vnd.ollama.image.params","size":201}]}"#;
        assert_eq!(
            read(ok, vision.as_bytes()),
            Manifest::Found {
                total: 2_489_894_016 + 476 + 851_251_328 + 201 + 629
            }
        );
        let bad = reqwest::StatusCode::BAD_REQUEST;
        for (sample, shown) in [
            (
                r#"{"error":"This tag is a sharded GGUF. Ollama does not yet support pulling sharded GGUF via the registry; please use a single-file quantization tag or \"latest\", or download the shards and merge them locally with `ollama create` (workaround detailed at https://github.com/ollama/ollama/issues/5245)."}"#,
                SHARDED,
            ),
            (
                r#"{"error":"This repository only contains sharded GGUF files. Ollama does not yet support pulling sharded GGUF via the registry; please download the shards and merge them locally with `ollama create` (workaround detailed at https://github.com/ollama/ollama/issues/5245), or use a repository with single-file quantizations."}"#,
                SHARDED,
            ),
            (
                r#"{"error":"The specified tag is not a valid quantization scheme. Please use another tag or \"latest\""}"#,
                UNAVAILABLE_VERSION,
            ),
            (
                r#"{"error":"The specified tag is not available in the repository. Please use another tag or \"latest\""}"#,
                UNAVAILABLE_VERSION,
            ),
        ] {
            assert_eq!(
                read(bad, sample.as_bytes()),
                Manifest::Refused(shown.into())
            );
        }
        let gated = r#"{"error":"Access to model google/gemma-2b is restricted. You must have access to it and be authenticated to access it. Please log in."}"#;
        assert_eq!(
            read(reqwest::StatusCode::UNAUTHORIZED, gated.as_bytes()),
            Manifest::Refused(NOT_FOUND_OR_PRIVATE.into())
        );
        // 400 sans JSON : texte brut ; vide : version indisponible.
        assert_eq!(read(bad, b"Bad tag"), Manifest::Refused("Bad tag".into()));
        assert_eq!(
            read(bad, b""),
            Manifest::Refused(UNAVAILABLE_VERSION.into())
        );
        // Rien de sûr : page HTML, JSON sans couches, autres statuts.
        assert_eq!(read(ok, b"<html></html>"), Manifest::Unexpected);
        assert_eq!(read(ok, br#"{"schemaVersion":2}"#), Manifest::Unexpected);
        for status in [404, 429, 500, 503] {
            let status = reqwest::StatusCode::from_u16(status).unwrap();
            assert_eq!(read(status, b"{}"), Manifest::Unexpected, "{status}");
        }
    }

    #[tokio::test]
    async fn manifests_are_asked_with_the_docker_accept_header() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let fetch = |model: &'static str| {
            let url = fake.url();
            async move { fetch(&url, &name(model), WAIT).await }
        };
        // Le faux registre refuse une requête sans l'en-tête Accept.
        let vision = "hf.co/chatonlm/Eye-vision-GGUF:Q4_K_M";
        assert_eq!(
            fetch(vision).await,
            Manifest::Found {
                total: fake_ollama::hf_total(vision)
            }
        );
        for (model, shown) in [
            ("hf.co/chatonlm/Big-sharded-GGUF:Q8_0", SHARDED),
            (
                "hf.co/chatonlm/Gone-missing-GGUF:Q4_K_M",
                NOT_FOUND_OR_PRIVATE,
            ),
            (
                "hf.co/chatonlm/Lab-restricted-GGUF:Q4_K_M",
                NOT_FOUND_OR_PRIVATE,
            ),
            ("hf.co/chatonlm/Tiny-GGUF:badtag", UNAVAILABLE_VERSION),
            ("hf.co/chatonlm/Tiny-GGUF:badquant", UNAVAILABLE_VERSION),
            ("hf.co/chatonlm/Weights-notgguf:Q4_K_M", NOT_GGUF),
        ] {
            assert_eq!(
                fetch(model).await,
                Manifest::Refused(shown.into()),
                "{model}"
            );
        }
        assert_eq!(
            fetch("hf.co/chatonlm/Down-html-GGUF:Q4_K_M").await,
            Manifest::Unexpected
        );
        assert_eq!(
            fake.manifest_requests()[0],
            "GET /v2/chatonlm/Eye-vision-GGUF/manifests/Q4_K_M"
        );
        // Autre statut d'un serveur qui n'est pas le registre.
        let proxy = FakeOllama::start(Mode::Status(503)).await.unwrap();
        assert_eq!(
            super::fetch(&proxy.url(), &name(vision), WAIT).await,
            Manifest::Unexpected
        );
    }

    #[tokio::test]
    async fn no_answer_is_unreachable() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let url = fake.url();
        fake.stop().await;
        let model = name("hf.co/chatonlm/Tiny-GGUF:Q4_K_M");
        let Manifest::Unreachable(message) = fetch(&url, &model, WAIT).await else {
            panic!("expected no answer");
        };
        assert!(
            message.starts_with("can't reach Hugging Face: "),
            "{message}"
        );
        assert!(!message.contains("127.0.0.1"), "{message}");
        let silent = FakeOllama::start(Mode::Silent).await.unwrap();
        assert_eq!(
            fetch(&silent.url(), &model, Duration::from_millis(200)).await,
            Manifest::Unreachable("Hugging Face didn't answer in time".into())
        );
    }
}
