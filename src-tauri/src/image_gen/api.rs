//! API native de `sd-server` (`/sdcpp/v1`, master-964) : tâche envoyée
//! (202 + identifiant), suivie jusqu'à sa fin (état, aperçu, image en
//! base64). Les erreurs rendues sont la raison d'un message traduit : texte
//! du serveur ou du système tel quel, sinon un texte de ChatonLM (`t!`).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};

use super::Request;
use super::server::REQUEST_TIMEOUT;
use crate::i18n::t;

/// Plafond d'une réponse (une image de 2048² en PNG encodée en base64 tient
/// largement dedans).
const MAX_BODY: usize = 96 * 1024 * 1024;

/// Client du serveur local (sans proxy).
pub(super) fn client(base: &str) -> Result<reqwest::Client, String> {
    let url = reqwest::Url::parse(base).map_err(|error| error.to_string())?;
    crate::ollama::client::builder(&url, Some(REQUEST_TIMEOUT))
        .build()
        .map_err(|error| error.to_string())
}

/// Le serveur répond par son API (`GET /sdcpp/v1/capabilities`).
pub(super) async fn ready(client: &reqwest::Client, base: &str) -> bool {
    let response = client
        .get(format!("{base}/sdcpp/v1/capabilities"))
        .send()
        .await;
    let Ok(response) = response else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    read_json(response)
        .await
        .is_ok_and(|body| body.get("model").is_some())
}

/// Corps de la demande : tous les réglages, jamais ceux du serveur par
/// défaut.
pub(super) fn body(request: &Request, seed: u32) -> Value {
    let sampling = &request.sampling;
    let mut sample_params = json!({
        "sample_steps": sampling.steps,
        "guidance": { "txt_cfg": sampling.cfg_scale },
    });
    if let Some(method) = &sampling.sample_method {
        sample_params["sample_method"] = json!(method);
    }
    if let Some(scheduler) = &sampling.scheduler {
        sample_params["scheduler"] = json!(scheduler);
    }
    if let Some(shift) = sampling.flow_shift {
        sample_params["flow_shift"] = json!(shift);
    }
    json!({
        "prompt": request.prompt,
        "negative_prompt": request.negative_prompt,
        "width": request.width,
        "height": request.height,
        "seed": seed,
        "batch_count": 1,
        "output_format": "png",
        "preview": if request.preview { "proj" } else { "none" },
        "sample_params": sample_params,
    })
}

/// Envoie la demande ; rend l'identifiant de la tâche.
pub(super) async fn submit(
    client: &reqwest::Client,
    base: &str,
    body: &Value,
) -> Result<String, String> {
    let response = client
        .post(format!("{base}/sdcpp/v1/img_gen"))
        .json(body)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = read_json(response).await?;
    if !status.is_success() {
        return Err(engine_error(&body).unwrap_or_else(|| http_error(status)));
    }
    body["id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| t!("imageGen.detail.noJobId"))
}

/// Aperçu d'une étape.
#[derive(Debug)]
pub(super) struct Frame {
    pub pass: u32,
    pub step: u32,
    pub total: u32,
    pub png: Vec<u8>,
}

/// État d'une tâche.
#[derive(Debug)]
pub(super) enum JobState {
    Queued,
    Generating {
        preview: Option<Frame>,
    },
    Completed {
        png: Vec<u8>,
    },
    /// Échec, annulation par un autre, ou tâche inconnue du serveur.
    Failed {
        message: String,
    },
}

/// Suivi d'une tâche.
#[derive(Debug)]
pub(super) struct Job {
    pub state: JobState,
}

/// Lit l'état de la tâche `id`.
pub(super) async fn job(client: &reqwest::Client, base: &str, id: &str) -> Result<Job, String> {
    let response = client
        .get(format!("{base}/sdcpp/v1/jobs/{id}"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = read_json(response).await?;
    if status.as_u16() == 404 || status.as_u16() == 410 {
        return Ok(Job {
            state: JobState::Failed {
                message: engine_error(&body).unwrap_or_else(|| t!("imageGen.detail.jobNotFound")),
            },
        });
    }
    if !status.is_success() {
        return Err(engine_error(&body).unwrap_or_else(|| http_error(status)));
    }
    Ok(Job {
        state: parse_state(&body)?,
    })
}

/// État lu dans la réponse. Un état inconnu (version plus récente du
/// serveur) compte comme un calcul en cours, sans aperçu.
fn parse_state(body: &Value) -> Result<JobState, String> {
    match body["status"].as_str() {
        Some("queued") => Ok(JobState::Queued),
        Some("completed") => {
            let encoded = body["result"]["images"][0]["b64_json"]
                .as_str()
                .ok_or_else(|| t!("imageGen.detail.noImage"))?;
            let png = BASE64
                .decode(encoded)
                .map_err(|_| t!("imageGen.detail.badImage"))?;
            Ok(JobState::Completed { png })
        }
        Some("failed" | "cancelled") => Ok(JobState::Failed {
            message: body["error"]["message"]
                .as_str()
                .or(body["error"]["code"].as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| t!("imageGen.detail.unknownError")),
        }),
        Some(_) => Ok(JobState::Generating {
            preview: frame(&body["preview"]),
        }),
        None => Err(t!("imageGen.detail.noStatus")),
    }
}

fn frame(preview: &Value) -> Option<Frame> {
    let number = |key: &str| preview[key].as_u64().and_then(|n| u32::try_from(n).ok());
    let png = BASE64.decode(preview["b64_json"].as_str()?).ok()?;
    Some(Frame {
        pass: number("pass").unwrap_or(1),
        step: number("step")?,
        total: number("total_steps")?,
        png,
    })
}

/// Message d'erreur du serveur (`{"error": "..."}` ou
/// `{"error": {"message": "..."}}`).
fn engine_error(body: &Value) -> Option<String> {
    body["error"]
        .as_str()
        .or(body["error"]["message"].as_str())
        .map(str::to_owned)
}

/// Code HTTP d'une réponse sans message du serveur.
fn http_error(status: reqwest::StatusCode) -> String {
    t!("imageGen.detail.http", status = status.as_u16())
}

/// Corps JSON, plafonné.
async fn read_json(response: reqwest::Response) -> Result<Value, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY as u64)
    {
        return Err(t!("imageGen.detail.tooLarge"));
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if bytes.len() + chunk.len() > MAX_BODY {
            return Err(t!("imageGen.detail.tooLarge"));
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&bytes).map_err(|_| t!("imageGen.detail.notJson"))
}

/// Largeur et hauteur d'un PNG (en-tête IHDR) ; `None` si ce n'en est pas un.
pub(super) fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if png.len() < 24 || png[..8] != SIGNATURE || &png[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(png[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_gen::Sampling;

    fn request() -> Request {
        Request {
            prompt: "a ginger kitten".into(),
            negative_prompt: String::new(),
            width: 512,
            height: 768,
            seed: None,
            sampling: Sampling {
                steps: 4,
                cfg_scale: 1.0,
                sample_method: None,
                scheduler: None,
                flow_shift: None,
            },
            preview: true,
        }
    }

    #[test]
    fn the_body_gives_every_setting() {
        let body = body(&request(), 7);
        assert_eq!(body["width"], 512);
        assert_eq!(body["height"], 768);
        assert_eq!(body["seed"], 7);
        assert_eq!(body["batch_count"], 1);
        assert_eq!(body["output_format"], "png");
        assert_eq!(body["preview"], "proj");
        assert_eq!(body["sample_params"]["sample_steps"], 4);
        assert_eq!(body["sample_params"]["guidance"]["txt_cfg"], 1.0);
        assert!(body["sample_params"].get("sample_method").is_none());
        assert!(body["sample_params"].get("flow_shift").is_none());
        let mut chosen = request();
        chosen.preview = false;
        chosen.sampling.sample_method = Some("euler".into());
        chosen.sampling.scheduler = Some("simple".into());
        chosen.sampling.flow_shift = Some(3.0);
        let body = super::body(&chosen, 7);
        assert_eq!(body["preview"], "none");
        assert_eq!(body["sample_params"]["sample_method"], "euler");
        assert_eq!(body["sample_params"]["scheduler"], "simple");
        assert_eq!(body["sample_params"]["flow_shift"], 3.0);
    }

    #[test]
    fn job_states_are_read_like_the_engine_writes_them() {
        let png = fake_ollama::sdcpp::png(4, 2, [1, 2, 3]);
        let encoded = BASE64.encode(&png);
        assert!(matches!(
            parse_state(&json!({"status": "queued"})),
            Ok(JobState::Queued)
        ));
        let generating = parse_state(&json!({
            "status": "generating",
            "preview": {"pass": 1, "step": 2, "total_steps": 4, "b64_json": encoded},
        }));
        match generating {
            Ok(JobState::Generating {
                preview: Some(frame),
            }) => {
                assert_eq!((frame.pass, frame.step, frame.total), (1, 2, 4));
                assert_eq!(frame.png, png);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse_state(&json!({"status": "generating", "preview": null})),
            Ok(JobState::Generating { preview: None })
        ));
        match parse_state(&json!({
            "status": "completed",
            "result": {"output_format": "png", "images": [{"index": 0, "b64_json": encoded}]},
        })) {
            Ok(JobState::Completed { png: image }) => assert_eq!(image, png),
            other => panic!("{other:?}"),
        }
        match parse_state(&json!({
            "status": "failed",
            "error": {"code": "generation_failed", "message": "out of memory"},
        })) {
            Ok(JobState::Failed { message }) => assert_eq!(message, "out of memory"),
            other => panic!("{other:?}"),
        }
        assert!(parse_state(&json!({"status": "completed", "result": null})).is_err());
        assert!(parse_state(&json!({"preview": null})).is_err());
        // État inconnu d'une version plus récente : calcul en cours.
        assert!(matches!(
            parse_state(&json!({"status": "upscaling"})),
            Ok(JobState::Generating { preview: None })
        ));
    }

    #[test]
    fn png_size_reads_the_header_only_of_a_png() {
        let png = fake_ollama::sdcpp::png(48, 16, [0, 0, 0]);
        assert_eq!(png_size(&png), Some((48, 16)));
        assert_eq!(png_size(b"\xff\xd8\xff\xe0 not a png at all, a JPEG"), None);
        assert_eq!(png_size(&png[..20]), None);
    }

    #[test]
    fn png_size_needs_the_png_signature() {
        // En-tête IHDR intact, signature abîmée : ce n'est pas un PNG.
        let mut png = fake_ollama::sdcpp::png(48, 16, [0, 0, 0]);
        png[1] = b'J';
        assert_eq!(png_size(&png), None);
    }
}
