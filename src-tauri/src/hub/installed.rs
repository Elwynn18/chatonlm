//! Modèles installés sur le serveur (onglet Installed du ModelHub) et
//! suppression (`DELETE /api/delete`).
//!
//! Contrairement au sélecteur (`ollama::models::list`), la liste garde tous
//! les modèles locaux, embeddings compris ; seuls les modèles distants
//! (`remote_host`, cloud) sont écartés. Ordre : du plus récent au plus ancien
//! (date `modified_at`), celui du serveur à date égale ou illisible.

use std::time::Duration;

use serde::Deserialize;

use super::names::parse_hf_name;
use super::types::HubInstalled;
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::ollama::client;
use crate::ollama::models::{answered, error_message, unreachable};

/// Délai de la liste : quelques fichiers lus par le serveur.
const LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// Délai d'une suppression (manifeste et couches non partagées effacés).
const DELETE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<Tag>,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
    #[serde(default)]
    remote_host: Option<String>,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    modified_at: Option<String>,
    #[serde(default)]
    details: Details,
    #[serde(default)]
    capabilities: Option<Vec<String>>,
}

#[derive(Default, Deserialize)]
struct Details {
    #[serde(default)]
    parameter_size: String,
    #[serde(default)]
    quantization_level: String,
    #[serde(default)]
    family: String,
}

/// Modèles locaux du serveur `base`, du plus récent au plus ancien.
pub async fn list(base: &str) -> AppResult<Vec<HubInstalled>> {
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

fn parse(body: &str) -> AppResult<Vec<HubInstalled>> {
    let tags: TagsResponse = serde_json::from_str(body)
        .map_err(|error| AppError::Ollama(t!("ollama.unexpectedModels", error = error)))?;
    let mut models: Vec<(Option<i128>, HubInstalled)> = tags
        .models
        .into_iter()
        .filter(|tag| tag.remote_host.as_deref().is_none_or(str::is_empty))
        .map(|tag| {
            let repo = parse_hf_name(&tag.name).map(|name| name.repo_id());
            let date = tag.modified_at.as_deref().and_then(timestamp);
            (
                date,
                HubInstalled {
                    model: tag.name,
                    size: tag.size,
                    modified_at: tag.modified_at,
                    parameter_size: tag.details.parameter_size,
                    quantization_level: tag.details.quantization_level,
                    family: tag.details.family,
                    capabilities: tag.capabilities.unwrap_or_default(),
                    repo,
                },
            )
        })
        .collect();
    // Tri stable : dates décroissantes, illisibles à la fin.
    models.sort_by(|(a, _), (b, _)| b.cmp(a));
    Ok(models.into_iter().map(|(_, model)| model).collect())
}

/// Supprime `model` du serveur `base` : 200 → fait ; 404 → introuvable.
/// Déchargé d'abord, erreurs ignorées, comme `ollama rm` (`cmd/cmd.go`,
/// `DeleteHandler`) : un modèle supprimé resterait en mémoire, et Ollama
/// refuserait ensuite de le décharger (404, vu sur 0.35.1).
pub async fn delete(base: &str, model: &str) -> AppResult<()> {
    if model.trim().is_empty() {
        // i18n-ignore : la page ne demande jamais un nom vide.
        return Err(AppError::Invalid("model name is empty".into()));
    }
    let _ = crate::ollama::running::unload(base, model).await;
    let url = client::server_url(base)?;
    let client = client::client(&url, Some(DELETE_TIMEOUT)).map_err(unreachable)?;
    let response = client
        .delete(format!("{base}/api/delete"))
        .json(&serde_json::json!({ "model": model }))
        .send()
        .await
        .map_err(unreachable)?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(AppError::NotFound(t!("ollama.installedNotFound")));
    }
    let body = response.text().await.unwrap_or_default();
    Err(answered(status.as_u16(), &error_message(&body)))
}

/// Date RFC 3339 d'Ollama (« 2026-10-04T12:00:00.123456789+02:00 », « …Z »)
/// en nanosecondes depuis 1970 ; `None` si illisible. Aussi pour les
/// échéances de `/api/ps` (`ollama::running`).
pub(crate) fn timestamp(text: &str) -> Option<i128> {
    let number = |start: usize, len: usize| -> Option<i64> {
        let part = text.get(start..start + len)?;
        part.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let separators = [(4, b'-'), (7, b'-'), (13, b':'), (16, b':')];
    let bytes = text.as_bytes();
    if bytes.len() < 20
        || separators.iter().any(|&(at, byte)| bytes[at] != byte)
        || !matches!(bytes[10], b'T' | b't' | b' ')
    {
        return None;
    }
    let (year, month, day) = (number(0, 4)?, number(5, 2)?, number(8, 2)?);
    let (hour, minute, second) = (number(11, 2)?, number(14, 2)?, number(17, 2)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    if second > 60 {
        return None;
    }
    let mut rest = &text[19..];
    let mut nanos: i128 = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        for (index, byte) in fraction.bytes().take(digits).enumerate() {
            if index < 9 {
                nanos = nanos * 10 + i128::from(byte - b'0');
            }
        }
        for _ in digits..9 {
            nanos *= 10;
        }
        rest = &fraction[digits..];
    }
    let offset: i64 = match rest.as_bytes() {
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let digits = [*h1, *h2, *m1, *m2];
            if !digits.iter().all(u8::is_ascii_digit) {
                return None;
            }
            let value = |a: u8, b: u8| i64::from(a - b'0') * 10 + i64::from(b - b'0');
            let minutes = value(*h1, *h2) * 60 + value(*m1, *m2);
            if *sign == b'-' { -minutes } else { minutes }
        }
        _ => return None,
    };
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
        - offset * 60;
    Some(i128::from(seconds) * 1_000_000_000 + nanos)
}

/// Jours depuis le 1er janvier 1970 (calendrier grégorien proleptique ;
/// algorithme de Howard Hinnant, `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use fake_ollama::{FakeOllama, Mode};

    use super::*;

    #[test]
    fn dates_are_read_like_rfc_3339() {
        assert_eq!(timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            timestamp("1970-01-02T00:00:00Z"),
            Some(86_400 * 1_000_000_000)
        );
        // 2000-03-01 : après un 29 février.
        assert_eq!(
            timestamp("2000-03-01T00:00:00Z"),
            Some(951_868_800 * 1_000_000_000)
        );
        // Fuseaux : la même date.
        assert_eq!(
            timestamp("2026-10-04T12:00:00+02:00"),
            timestamp("2026-10-04T10:00:00Z")
        );
        assert_eq!(
            timestamp("2026-10-04T05:30:00-04:30"),
            timestamp("2026-10-04T10:00:00Z")
        );
        // Fractions : nanosecondes, au-delà de 9 chiffres ignorées.
        assert_eq!(timestamp("1970-01-01T00:00:00.5Z"), Some(500_000_000));
        assert_eq!(
            timestamp("1970-01-01T00:00:00.123456789123+00:00"),
            Some(123_456_789)
        );
        for bad in [
            "",
            "yesterday",
            "2026-10-04",
            "2026-10-04T12:00:00",
            "2026-13-04T12:00:00Z",
            "2026-10-04T12:00:00.Z",
            "2026-10-04T12:00:00+2:00",
            "2026/10/04T12:00:00Z",
            "2026-10-04T12:00:00Zx",
            "+026-10-04T12:00:00Z",
        ] {
            assert_eq!(timestamp(bad), None, "{bad}");
        }
    }

    #[test]
    fn local_models_newest_first_with_their_repository() {
        let body = serde_json::json!({ "models": [
            {
                "name": "qwen3:0.6b", "size": 522_653_767_u64,
                "modified_at": "2026-10-01T10:00:00.000000000+02:00",
                "details": { "family": "qwen3", "parameter_size": "751.63M",
                    "quantization_level": "Q4_K_M" },
                "capabilities": ["completion", "thinking"],
            },
            // Distant : écarté.
            {
                "name": "gpt-oss:120b-cloud", "remote_host": "https://ollama.com:443",
                "modified_at": "2026-10-05T10:00:00Z",
            },
            // Hôte distant vide : local.
            {
                "name": "nomic-embed-text:latest", "remote_host": "", "size": 274_302_450_u64,
                "modified_at": "2026-09-01T00:00:00Z",
                "details": { "family": "nomic-bert" }, "capabilities": ["embedding"],
            },
            // Plus récent malgré son fuseau (08:30 UTC contre 08:00 UTC).
            {
                "name": "HF.CO/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M",
                "size": 105_455_694_u64,
                "modified_at": "2026-10-01T04:30:00-04:00",
                "details": { "family": "llama", "parameter_size": "135M",
                    "quantization_level": "Q4_K_M" },
                "capabilities": ["completion"],
            },
            // Date illisible : à la fin.
            { "name": "mystery:latest", "modified_at": "someday" },
            { "name": "huggingface.co/unsloth/Qwen3-8B-GGUF:UD-Q4_K_XL",
              "modified_at": "2026-10-02T00:00:00Z" },
        ]});
        let models = parse(&body.to_string()).unwrap();
        let names: Vec<_> = models.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(
            names,
            [
                "huggingface.co/unsloth/Qwen3-8B-GGUF:UD-Q4_K_XL",
                "HF.CO/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M",
                "qwen3:0.6b",
                "nomic-embed-text:latest",
                "mystery:latest",
            ]
        );
        let repos: Vec<_> = models.iter().map(|m| m.repo.as_deref()).collect();
        assert_eq!(
            repos,
            [
                Some("unsloth/Qwen3-8B-GGUF"),
                Some("bartowski/SmolLM2-135M-Instruct-GGUF"),
                None,
                None,
                None
            ]
        );
        let smol = &models[1];
        assert_eq!(smol.size, 105_455_694);
        assert_eq!(smol.parameter_size, "135M");
        assert_eq!(smol.quantization_level, "Q4_K_M");
        assert_eq!(smol.family, "llama");
        assert_eq!(smol.capabilities, ["completion"]);
        assert_eq!(
            smol.modified_at.as_deref(),
            Some("2026-10-01T04:30:00-04:00")
        );
        assert_eq!(models[3].capabilities, ["embedding"]);
        assert_eq!(models[4].capabilities, Vec::<String>::new());
        assert!(matches!(parse("{}"), Err(AppError::Ollama(_))));
    }

    #[tokio::test]
    async fn installed_then_deleted_on_the_server() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let names = |models: Vec<HubInstalled>| -> Vec<String> {
            models.into_iter().map(|m| m.model).collect()
        };
        assert_eq!(
            names(list(&fake.url()).await.unwrap()),
            ["qwen3:0.6b", "qwen3.5:0.8b"]
        );
        // En mémoire : déchargé avant la suppression (après, Ollama ne le
        // déchargerait plus).
        crate::ollama::running::load(&fake.url(), "qwen3:0.6b", 0, -1)
            .await
            .unwrap();
        delete(&fake.url(), "qwen3:0.6b").await.unwrap();
        assert_eq!(names(list(&fake.url()).await.unwrap()), ["qwen3.5:0.8b"]);
        assert!(
            crate::ollama::running::list(&fake.url())
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            fake.load_requests()[1],
            serde_json::json!({ "model": "qwen3:0.6b", "kind": "unload", "keep_alive": 0 })
        );
        let Err(AppError::NotFound(message)) = delete(&fake.url(), "qwen3:0.6b").await else {
            panic!("expected not found");
        };
        assert_eq!(message, "model not found");
        assert_eq!(fake.delete_requests(), ["qwen3:0.6b", "qwen3:0.6b"]);
        assert!(matches!(
            delete(&fake.url(), " ").await,
            Err(AppError::Invalid(_))
        ));
        // Serveur injoignable : texte de models.rs.
        let url = fake.url();
        fake.stop().await;
        let Err(AppError::Ollama(message)) = delete(&url, "qwen3.5:0.8b").await else {
            panic!("expected an Ollama error");
        };
        assert_eq!(message, "can't connect to Ollama");
        assert!(matches!(list(&url).await, Err(AppError::Ollama(_))));
        // Autre statut.
        let proxy = FakeOllama::start(Mode::Status(502)).await.unwrap();
        let Err(AppError::Ollama(message)) = delete(&proxy.url(), "qwen3.5:0.8b").await else {
            panic!("expected an Ollama error");
        };
        assert!(message.starts_with("Ollama answered HTTP 502"), "{message}");
    }
}
