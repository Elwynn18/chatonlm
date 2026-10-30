//! Génération d'images dans la conversation (propre à ChatonLM) : outil
//! `generate_image` appelé par le modèle, et bouton « Image » de la zone de
//! saisie (contrat : `.tools/research/84-image-chat-contract.md`).
//!
//! - Le peintre ([`Painter`]) fait l'image ; celui de l'app ([`AppPainter`])
//!   relit à chaque appel le moteur (`image_gen`), le modèle choisi dans le
//!   ModelHub (`image_models`) et la base. Sans peintre (essais), rien n'est
//!   prêt : l'outil n'est jamais offert.
//! - Prêt = moteur livré, processeur pris en charge, modèle choisi installé ;
//!   sinon un refus typé ([`PaintRefusal`]) et son texte. Demandé à chaque
//!   envoi vers un modèle « tools » : rien de coûteux. La carte graphique
//!   (détection qui peut lancer `nvidia-smi`) n'est regardée qu'au moment
//!   d'une image ([`Painter::cpu`], [`Painter::paint`]).
//! - Pas d'outil pour un envoi du mode vocal : chaque phrase prononcée
//!   arrêterait l'image en cours sans prévenir (choix fait dans `chat/mod.rs`).
//! - Taille : arguments arrondis au multiple de 16 le plus proche, bornés à
//!   [256, 1536] ; un côté absent prend le côté par défaut (celui de la
//!   famille avec une carte graphique, 512 sans carte).
//! - Arrêt : lâcher le futur de [`Painter::paint`] (le moteur arrête alors
//!   `sd-server` s'il calculait).
//! - Temps mesuré (contrat 85 § 2) : après chaque image réussie, la durée de
//!   la première étape d'échantillonnage à l'image (sans démarrage,
//!   chargement ni compilation des shaders), arrondie à la seconde (au moins
//!   1), est notée pour le modèle ; un échec ou un arrêt ne note rien.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};
use ts_rs::TS;

use crate::i18n::t;
use crate::image_gen::{self, ImageGen};
use crate::image_models::ImageModels;
use crate::store::Store;
use crate::store::image_models as stored;

/// Nom de l'outil.
pub const TOOL_NAME: &str = "generate_image";

/// Images au plus par envoi ; au-delà, erreur donnée au modèle.
pub const MAX_IMAGES_PER_SEND: usize = 4;

/// Côtés permis d'une image demandée par l'outil (pixels).
pub const MIN_TOOL_SIDE: u32 = 256;
pub const MAX_TOOL_SIDE: u32 = 1536;

/// Côté par défaut sans carte graphique (calcul sur le processeur).
pub const CPU_SIDE: u32 = 512;

/// Aperçu le plus lourd envoyé à la page (octets du PNG).
const MAX_PREVIEW: usize = 512 * 1024;

/// Étape d'une image en cours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ImageStage {
    Starting,
    Queued,
    Loading,
    Generating,
}

/// Avancement d'une image (événement `imageProgress`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageProgress {
    pub stage: ImageStage,
    /// Étape d'échantillonnage en cours et nombre total (étape `generating`
    /// seulement).
    pub step: Option<u32>,
    pub total: Option<u32>,
    /// Aperçu de l'étape : `data:image/png;base64,…` (aucun s'il dépasse
    /// 512 Kio).
    pub preview: Option<String>,
    /// Calcul par le processeur (aucune carte graphique) : plusieurs minutes.
    pub cpu: bool,
    /// Taille demandée (pixels), pour réserver la place de l'image.
    pub width: u32,
    pub height: u32,
}

impl ImageProgress {
    /// Avancement sans étape ni aperçu.
    pub fn at(stage: ImageStage, cpu: bool, width: u32, height: u32) -> Self {
        Self {
            stage,
            step: None,
            total: None,
            preview: None,
            cpu,
            width,
            height,
        }
    }
}

/// Avancement du moteur, tel que la page le reçoit.
pub fn progress_of(
    progress: image_gen::Progress,
    cpu: bool,
    width: u32,
    height: u32,
) -> ImageProgress {
    match progress {
        image_gen::Progress::Starting => {
            ImageProgress::at(ImageStage::Starting, cpu, width, height)
        }
        image_gen::Progress::Queued => ImageProgress::at(ImageStage::Queued, cpu, width, height),
        image_gen::Progress::Generating { step: None, .. } => {
            ImageProgress::at(ImageStage::Loading, cpu, width, height)
        }
        image_gen::Progress::Generating {
            step: Some((step, total)),
            preview,
        } => ImageProgress {
            step: Some(step),
            total: Some(total),
            preview: preview.as_deref().and_then(preview_url),
            ..ImageProgress::at(ImageStage::Generating, cpu, width, height)
        },
    }
}

/// Durée d'une image en secondes : arrondie, au moins 1.
pub fn measured_seconds(elapsed: Duration) -> u32 {
    let seconds = elapsed.as_secs_f64().round();
    // Borné avant la conversion.
    seconds.clamp(1.0, f64::from(u32::MAX)) as u32
}

/// Aperçu en adresse `data:` ; aucun au-delà de 512 Kio.
fn preview_url(png: &[u8]) -> Option<String> {
    (png.len() <= MAX_PREVIEW).then(|| format!("data:image/png;base64,{}", BASE64.encode(png)))
}

/// Peintre prêt (su sans détecter la carte graphique).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ready {
    /// Nom du modèle d'images choisi (« FLUX.2 klein 4B »).
    pub model_name: String,
    /// Côté d'une image dont la taille n'est pas demandée, avec une carte
    /// graphique (celui de la famille du modèle).
    pub default_side: u32,
}

impl Ready {
    /// Côté d'une image dont la taille n'est pas demandée : celui de la
    /// famille avec une carte graphique, [`CPU_SIDE`] sans carte.
    pub fn side(&self, cpu: bool) -> u32 {
        if cpu { CPU_SIDE } else { self.default_side }
    }
}

/// Pourquoi aucune image ne peut être faite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaintRefusal {
    /// Moteur absent de cette version.
    Unavailable,
    /// Processeur sans les instructions qu'exige le moteur (noms affichés).
    CpuUnsupported(String),
    /// Aucun modèle d'images choisi (ou plus installé).
    NoModel,
}

impl PaintRefusal {
    /// Texte montré, dans la langue du moment.
    pub fn message(&self) -> String {
        match self {
            PaintRefusal::Unavailable => t!("chat.imageUnavailable"),
            PaintRefusal::CpuUnsupported(features) => {
                t!("chat.imageCpuUnsupported", features = features)
            }
            PaintRefusal::NoModel => t!("chat.imageNoModel"),
        }
    }
}

/// Image demandée (taille déjà bornée).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaintRequest {
    pub prompt: String,
    pub width: u32,
    pub height: u32,
}

/// Image faite (PNG).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Painted {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub seed: u32,
    pub model_name: String,
}

/// Futur d'un peintre.
pub type PaintFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Fait les images de la conversation.
pub trait Painter: Send + Sync {
    /// Prêt à peindre, ou pourquoi non. Peu coûteux : demandé à chaque envoi
    /// vers un modèle qui accepte les outils, jamais la carte graphique.
    fn ready(&self) -> PaintFuture<'_, Result<Ready, PaintRefusal>>;

    /// Aucune carte graphique : calcul par le processeur. Demandé seulement
    /// au moment d'une image (la détection peut prendre quelques secondes).
    fn cpu(&self) -> PaintFuture<'_, bool>;

    /// Fait une image ; `progress` reçoit l'avancement. Erreur : texte montré
    /// (et donné au modèle). Lâcher le futur arrête l'image.
    fn paint<'a>(
        &'a self,
        request: PaintRequest,
        progress: &'a mut (dyn FnMut(ImageProgress) + Send),
    ) -> PaintFuture<'a, Result<Painted, String>>;
}

/// Peintre de l'app : moteur, modèles d'images et base relus par
/// `app.state()` à chaque appel.
pub struct AppPainter(pub AppHandle);

impl Painter for AppPainter {
    fn ready(&self) -> PaintFuture<'_, Result<Ready, PaintRefusal>> {
        Box::pin(async move {
            let app = &self.0;
            let (Some(engine), Some(models)) =
                (app.try_state::<ImageGen>(), app.try_state::<ImageModels>())
            else {
                return Err(PaintRefusal::Unavailable);
            };
            // Base illisible : aucun modèle choisi ne se lit.
            let Some(store) = app.try_state::<Store>() else {
                return Err(PaintRefusal::NoModel);
            };
            EnginePainter {
                engine: &engine,
                models: &models,
                store: &store,
                gpu: None,
            }
            .ready()
            .await
        })
    }

    fn cpu(&self) -> PaintFuture<'_, bool> {
        Box::pin(async move {
            let app = &self.0;
            match (
                app.try_state::<ImageGen>(),
                app.try_state::<ImageModels>(),
                app.try_state::<Store>(),
            ) {
                (Some(engine), Some(models), Some(store)) => {
                    EnginePainter {
                        engine: &engine,
                        models: &models,
                        store: &store,
                        gpu: None,
                    }
                    .cpu()
                    .await
                }
                // Aucune image ne se fera (`paint` le dira) : peu importe.
                _ => false,
            }
        })
    }

    fn paint<'a>(
        &'a self,
        request: PaintRequest,
        progress: &'a mut (dyn FnMut(ImageProgress) + Send),
    ) -> PaintFuture<'a, Result<Painted, String>> {
        Box::pin(async move {
            let app = &self.0;
            let (Some(engine), Some(models)) =
                (app.try_state::<ImageGen>(), app.try_state::<ImageModels>())
            else {
                return Err(PaintRefusal::Unavailable.message());
            };
            let Some(store) = app.try_state::<Store>() else {
                return Err(PaintRefusal::NoModel.message());
            };
            let gpu = app.try_state::<crate::gpu::GpuChief>();
            EnginePainter {
                engine: &engine,
                models: &models,
                store: &store,
                gpu: gpu.as_deref(),
            }
            .paint(request, progress)
            .await
        })
    }
}

/// Ce que fait le peintre de l'app, sur des états donnés (essayé contre le
/// faux `sd-server`).
pub(crate) struct EnginePainter<'a> {
    pub engine: &'a ImageGen,
    pub models: &'a ImageModels,
    pub store: &'a Store,
    /// Chef de la carte graphique : décharge l'autocomplétion avant une
    /// image (le serveur d'images ne prend que la VRAM libre).
    pub gpu: Option<&'a crate::gpu::GpuChief>,
}

impl EnginePainter<'_> {
    pub async fn ready(&self) -> Result<Ready, PaintRefusal> {
        if !self.engine.available() {
            return Err(PaintRefusal::Unavailable);
        }
        let missing = self.engine.missing_cpu_features();
        if !missing.is_empty() {
            return Err(PaintRefusal::CpuUnsupported(missing.join(", ")));
        }
        let selected = self
            .models
            .selected_spec(self.store)
            .await
            .ok_or(PaintRefusal::NoModel)?;
        Ok(Ready {
            model_name: selected.name,
            default_side: selected.default_side,
        })
    }

    /// Aucune carte graphique (détection gardée en cache).
    pub async fn cpu(&self) -> bool {
        !self.models.gpu().await
    }

    pub async fn paint(
        &self,
        request: PaintRequest,
        progress: &mut (dyn FnMut(ImageProgress) + Send),
    ) -> Result<Painted, String> {
        // Relu à chaque image : supprimé entre-temps, plus de modèle.
        let selected = self
            .models
            .selected_spec(self.store)
            .await
            .ok_or_else(|| PaintRefusal::NoModel.message())?;
        // L'autocomplétion de l'éditeur rend sa VRAM avant l'image.
        if let Some(gpu) = self.gpu {
            gpu.before_image().await;
        }
        let cpu = !self.models.gpu().await;
        let (width, height) = (request.width, request.height);
        let wanted = image_gen::Request {
            prompt: request.prompt,
            negative_prompt: String::new(),
            width,
            height,
            seed: None,
            sampling: selected.sampling,
            preview: true,
        };
        // Première étape d'échantillonnage vue : début du temps mesuré.
        let mut first_step: Option<Instant> = None;
        let mut relay = |step: image_gen::Progress| {
            if first_step.is_none()
                && matches!(step, image_gen::Progress::Generating { step: Some(_), .. })
            {
                first_step = Some(Instant::now());
            }
            progress(progress_of(step, cpu, width, height));
        };
        let image = self
            .engine
            .generate(&selected.spec, &wanted, &mut relay)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(started) = first_step {
            let seconds = measured_seconds(started.elapsed());
            let id = selected.id.clone();
            let noted = self
                .store
                .call(move |connection| stored::record_seconds(connection, &id, seconds))
                .await;
            if let Err(error) = noted {
                // i18n-ignore : journal.
                eprintln!("chatonlm: image time not recorded: {error}");
            }
        }
        Ok(Painted {
            png: image.png,
            width: image.width,
            height: image.height,
            seed: image.seed,
            model_name: selected.name,
        })
    }
}

/// Définition de l'outil, envoyée dans `tools` (consigne au modèle : en
/// anglais, sans traduction).
pub fn definition() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": TOOL_NAME,
            "description": "Create an image from a text description with the local image model and show it to the user. Use it only when the user asks for an image, a drawing, a picture or a photo. Write the prompt in English, as a detailed description of the picture (subject, style, composition, colors, lighting).",
            "parameters": {
                "type": "object",
                "required": ["prompt"],
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Detailed description of the image, in English",
                    },
                    "width": {
                        "type": "integer",
                        "description": "Width in pixels (256 to 1536, optional)",
                    },
                    "height": {
                        "type": "integer",
                        "description": "Height in pixels (256 to 1536, optional)",
                    },
                },
            },
        },
    })
}

/// Côté demandé (nombre, ou texte d'un nombre) ; `None` s'il est absent ou
/// illisible.
fn side_argument(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    let side = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))?;
    side.is_finite().then_some(side)
}

/// Côté arrondi au multiple de 16 le plus proche, borné à [256, 1536].
fn bounded(side: f64) -> u32 {
    let step = f64::from(image_gen::SIDE_STEP);
    let rounded = (side / step).round() * step;
    // Borné avant la conversion (valeurs négatives ou immenses).
    rounded.clamp(f64::from(MIN_TOOL_SIDE), f64::from(MAX_TOOL_SIDE)) as u32
}

/// Taille d'une image d'après les arguments de l'appel ; un côté absent prend
/// `default_side`.
pub fn image_size(arguments: &Value, default_side: u32) -> (u32, u32) {
    let side = |key: &str| side_argument(arguments.get(key)).map_or(default_side, bounded);
    (side("width"), side("height"))
}

/// Description demandée par l'appel ; `None` si elle manque ou est vide.
pub fn prompt_argument(arguments: &Value) -> Option<&str> {
    arguments
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|prompt| !prompt.trim().is_empty())
}

/// Résultat d'un appel réussi, envoyé à la page et au modèle (JSON compact,
/// champs dans cet ordre).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PaintResult<'a> {
    attachment_id: i64,
    width: u32,
    height: u32,
    seed: u32,
    model: &'a str,
}

/// Texte du résultat d'une image enregistrée.
pub fn result_text(attachment_id: i64, painted: &Painted) -> String {
    serde_json::to_string(&PaintResult {
        attachment_id,
        width: painted.width,
        height: painted.height,
        seed: painted.seed,
        model: &painted.model_name,
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_rounded_bounded_and_defaulted() {
        let size = |arguments: Value| image_size(&arguments, 1024);
        assert_eq!(size(json!({ "prompt": "a cat" })), (1024, 1024));
        assert_eq!(size(json!({ "width": 512, "height": 768 })), (512, 768));
        // Un seul côté : l'autre par défaut.
        assert_eq!(size(json!({ "width": 640 })), (640, 1024));
        assert_eq!(size(json!({ "height": 640 })), (1024, 640));
        // Multiple de 16 le plus proche (8 : vers le haut).
        assert_eq!(size(json!({ "width": 500, "height": 520 })), (496, 528));
        assert_eq!(size(json!({ "width": 504.1, "height": "777" })), (512, 784));
        // Bornes.
        assert_eq!(size(json!({ "width": 10, "height": -400 })), (256, 256));
        assert_eq!(
            size(json!({ "width": 5000, "height": 1e300 })),
            (1536, 1536)
        );
        // Illisible : par défaut.
        assert_eq!(
            size(json!({ "width": "big", "height": null })),
            (1024, 1024)
        );
        assert_eq!(image_size(&json!({}), CPU_SIDE), (512, 512));
    }

    #[test]
    fn the_prompt_must_be_a_text() {
        assert_eq!(
            prompt_argument(&json!({ "prompt": " a cat " })),
            Some(" a cat ")
        );
        for arguments in [json!({}), json!({ "prompt": "  " }), json!({ "prompt": 3 })] {
            assert_eq!(prompt_argument(&arguments), None, "{arguments}");
        }
    }

    #[test]
    fn engine_progress_is_told_to_the_page() {
        let of = |progress| progress_of(progress, true, 512, 768);
        assert_eq!(
            of(image_gen::Progress::Starting),
            ImageProgress::at(ImageStage::Starting, true, 512, 768)
        );
        assert_eq!(
            of(image_gen::Progress::Queued),
            ImageProgress::at(ImageStage::Queued, true, 512, 768)
        );
        // Pas encore d'étape : le modèle se charge.
        assert_eq!(
            of(image_gen::Progress::Generating {
                step: None,
                preview: None
            }),
            ImageProgress::at(ImageStage::Loading, true, 512, 768)
        );
        let generating = of(image_gen::Progress::Generating {
            step: Some((2, 4)),
            preview: Some(vec![1, 2, 3]),
        });
        assert_eq!(
            generating,
            ImageProgress {
                step: Some(2),
                total: Some(4),
                preview: Some("data:image/png;base64,AQID".into()),
                ..ImageProgress::at(ImageStage::Generating, true, 512, 768)
            }
        );
        // Aperçu trop lourd : aucun.
        let heavy = of(image_gen::Progress::Generating {
            step: Some((1, 4)),
            preview: Some(vec![0; MAX_PREVIEW + 1]),
        });
        assert_eq!(heavy.preview, None);
        assert_eq!(heavy.step, Some(1));
        let limit = of(image_gen::Progress::Generating {
            step: Some((1, 4)),
            preview: Some(vec![0; MAX_PREVIEW]),
        });
        assert!(limit.preview.is_some());
    }

    #[test]
    fn the_result_is_compact_json_in_order() {
        let painted = Painted {
            png: Vec::new(),
            width: 1024,
            height: 1024,
            seed: 7,
            model_name: "FLUX.2 klein 4B".into(),
        };
        assert_eq!(
            result_text(42, &painted),
            r#"{"attachmentId":42,"width":1024,"height":1024,"seed":7,"model":"FLUX.2 klein 4B"}"#
        );
    }

    #[test]
    fn the_measured_time_is_rounded_to_at_least_one_second() {
        assert_eq!(measured_seconds(Duration::ZERO), 1);
        assert_eq!(measured_seconds(Duration::from_millis(120)), 1);
        assert_eq!(measured_seconds(Duration::from_millis(1499)), 1);
        assert_eq!(measured_seconds(Duration::from_millis(1500)), 2);
        assert_eq!(measured_seconds(Duration::from_millis(9400)), 9);
        assert_eq!(measured_seconds(Duration::from_millis(9600)), 10);
    }

    #[test]
    fn refusals_have_their_text() {
        assert_eq!(
            PaintRefusal::NoModel.message(),
            "No image model chosen: pick one in the ModelHub, Images tab."
        );
        assert!(
            PaintRefusal::CpuUnsupported("AVX2, FMA".into())
                .message()
                .contains("AVX2, FMA")
        );
        assert_ne!(PaintRefusal::Unavailable.message(), "chat.imageUnavailable");
    }

    /// Peintre de l'app contre le faux `sd-server` (relancé par ce binaire
    /// d'essai, voir `image_gen::tests`) : prêt, refus, image et avancement.
    mod engine {
        use std::path::Path;
        use std::sync::{Arc, Mutex};

        use super::*;
        use crate::hub::hardware::Machine;
        use crate::hub::types::HubGpu;
        use crate::image_gen::tests::{fake_config, temp_dir};
        use crate::image_models::catalog::ImageCatalog;
        use crate::image_models::downloads::blob_path;
        use crate::image_models::families::Families;
        use crate::image_models::types::ImageComponentRole;
        use crate::store::image_models::{self as stored, StoredFile, StoredModel};

        const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        /// Deux familles d'essai : « Tiny » (côté par défaut 1024) et
        /// « Tiny SD » (512).
        const FAMILIES: &str = r#"{
            "pieces": {
                "vae": {"role": "vae", "component": "vae", "repo": "t/vae", "revision": "r",
                        "file": "vae.safetensors", "size": 3, "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdee",
                        "license": null}
            },
            "families": [
                {"id": "tiny", "name": "Tiny", "bases": ["t/base"], "singleFile": false,
                 "model": "diffusionModel", "pieces": ["vae"],
                 "sampling": {"steps": 4, "cfg": 1.0, "sampleMethod": null, "scheduler": null, "flowShift": null},
                 "options": []},
                {"id": "tiny-sd", "name": "Tiny SD", "bases": ["t/sd"], "singleFile": true,
                 "defaultSide": 512, "model": "model", "pieces": [],
                 "sampling": {"steps": 4, "cfg": 7.0, "sampleMethod": null, "scheduler": null, "flowShift": null},
                 "options": []}
            ],
            "unsupported": "^none$",
            "recommendations": []
        }"#;

        fn machine(gpu: bool) -> Machine {
            Machine {
                gpus: if gpu {
                    vec![HubGpu {
                        name: "GPU".into(),
                        vram_total: 8 << 30,
                        vram_free: None,
                    }]
                } else {
                    Vec::new()
                },
                ram_total: 16 << 30,
                ram_available: 8 << 30,
            }
        }

        /// Modèle de la famille `family` enregistré (fichiers écrits sous
        /// `root`, à leur taille) et choisi.
        fn install(store: &Store, root: &Path, family: &str, name: &str) -> StoredModel {
            let families = Families::parse(FAMILIES).unwrap();
            let mut files = vec![StoredFile {
                role: ImageComponentRole::Model,
                component: families.get(family).unwrap().model_component,
                repo: "t/model".into(),
                revision: "r".into(),
                file: format!("{name}.gguf"),
                size: 7,
                sha256: SHA.into(),
                license: None,
            }];
            for piece in &families.get(family).unwrap().pieces {
                files.push(StoredFile {
                    role: piece.role,
                    component: piece.component,
                    repo: piece.repo.clone(),
                    revision: piece.revision.clone(),
                    file: piece.file.clone(),
                    size: piece.size,
                    sha256: piece.sha256.clone(),
                    license: None,
                });
            }
            for file in &files {
                let path = blob_path(root, file);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, vec![b'x'; file.size as usize]).unwrap();
            }
            let model = StoredModel {
                id: format!("t/model/{name}.gguf"),
                repo: "t/model".into(),
                revision: "r".into(),
                file: format!("{name}.gguf"),
                family: family.into(),
                name: name.into(),
                license: None,
                uncensored: false,
                ever_installed: true,
                files,
                created_at: 0,
                last_seconds: None,
            };
            let saved = model.clone();
            store
                .blocking(move |c| {
                    stored::save(c, &saved)?;
                    stored::select(c, &saved.id)
                })
                .unwrap();
            model
        }

        fn models(root: &Path, cache: &Path, gpu: bool) -> ImageModels {
            ImageModels::for_tests(
                root.to_path_buf(),
                ImageCatalog::new(cache.to_path_buf()),
                Families::parse(FAMILIES).unwrap(),
                Some(machine(gpu)),
            )
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn the_app_painter_makes_an_image_with_the_chosen_model() {
            let (dir, root, cache) = (temp_dir(), temp_dir(), temp_dir());
            let store = Store::open_in_memory().unwrap();
            let engine = ImageGen::new(fake_config(dir.path()));
            let models = models(root.path(), cache.path(), true);
            let painter = EnginePainter {
                engine: &engine,
                models: &models,
                store: &store,
                gpu: None,
            };
            // Aucun modèle choisi.
            assert_eq!(painter.ready().await, Err(PaintRefusal::NoModel));
            let model = install(&store, root.path(), "tiny", "Tiny Q4");
            assert_eq!(
                painter.ready().await,
                Ok(Ready {
                    model_name: "Tiny Q4".into(),
                    default_side: 1024
                })
            );
            // Prêt sans jamais détecter la carte graphique (`nvidia-smi`
            // jusqu'à 3 s) : seulement au moment d'une image.
            assert_eq!(models.detections(), 0);
            assert!(!painter.cpu().await);
            assert_eq!(models.detections(), 1);

            let seen = Arc::new(Mutex::new(Vec::new()));
            let sink = seen.clone();
            let mut progress = move |progress: ImageProgress| sink.lock().unwrap().push(progress);
            let request = PaintRequest {
                prompt: "a ginger kitten".into(),
                width: 256,
                height: 320,
            };
            let painted = painter.paint(request, &mut progress).await.unwrap();
            assert_eq!(
                (painted.width, painted.height, painted.model_name.as_str()),
                (256, 320, "Tiny Q4")
            );
            assert_eq!(
                painted.png,
                fake_ollama::sdcpp::png(
                    256,
                    320,
                    fake_ollama::sdcpp::color(i64::from(painted.seed))
                )
            );
            let seen = seen.lock().unwrap().clone();
            assert_eq!(
                seen.first(),
                Some(&ImageProgress::at(ImageStage::Starting, false, 256, 320))
            );
            let generating: Vec<&ImageProgress> = seen
                .iter()
                .filter(|progress| progress.stage == ImageStage::Generating)
                .collect();
            assert!(!generating.is_empty(), "{seen:?}");
            assert!(generating.iter().all(|progress| {
                progress.total == Some(4)
                    && progress
                        .preview
                        .as_deref()
                        .is_some_and(|preview| preview.starts_with("data:image/png;base64,"))
                    && (progress.width, progress.height, progress.cpu) == (256, 320, false)
            }));

            // Fichier du modèle disparu : plus prêt, et l'image demandée
            // quand même échoue avec le même texte.
            std::fs::remove_file(blob_path(root.path(), &model.files[0])).unwrap();
            assert_eq!(painter.ready().await, Err(PaintRefusal::NoModel));
            let request = PaintRequest {
                prompt: "a cat".into(),
                width: 256,
                height: 256,
            };
            assert_eq!(
                painter.paint(request, &mut |_| {}).await,
                Err(PaintRefusal::NoModel.message())
            );
            engine.shutdown().await;
        }

        /// Temps de la dernière image noté pour le modèle.
        fn last_seconds(store: &Store, id: &str) -> Option<u32> {
            let id = id.to_owned();
            store
                .blocking(move |c| Ok(stored::get(c, &id)?.and_then(|model| model.last_seconds)))
                .unwrap()
        }

        /// Avant l'image, le modèle d'autocomplétion de l'éditeur quitte la
        /// mémoire (le serveur d'images ne prend que la VRAM libre).
        #[tokio::test(flavor = "multi_thread")]
        async fn the_completion_model_is_unloaded_before_an_image() {
            let ollama = fake_ollama::FakeOllama::start(fake_ollama::Mode::ollama())
                .await
                .unwrap();
            let url = ollama.url();
            let chief = crate::gpu::GpuChief::default();
            let inputs = crate::gpu::Inputs {
                server: crate::chat::Server {
                    url: url.clone(),
                    context_length: 0,
                    keep_alive: 0,
                    cloud: false,
                },
                chat_model: "qwen3:0.6b".into(),
                local: true,
                busy: Box::new(|| false),
                machine: None,
            };
            chief.prepare(&inputs, "chatonlm-fim").await.unwrap();
            let (dir, root, cache) = (temp_dir(), temp_dir(), temp_dir());
            let store = Store::open_in_memory().unwrap();
            let engine = ImageGen::new(fake_config(dir.path()));
            let models = models(root.path(), cache.path(), true);
            install(&store, root.path(), "tiny", "Tiny Q4");
            let painter = EnginePainter {
                engine: &engine,
                models: &models,
                store: &store,
                gpu: Some(&chief),
            };
            let request = PaintRequest {
                prompt: "a ginger kitten".into(),
                width: 256,
                height: 256,
            };
            painter.paint(request, &mut |_| {}).await.unwrap();
            assert!(
                crate::ollama::running::list(&url).await.unwrap().is_empty(),
                "the completion model left the memory"
            );
            assert_eq!(ollama.load_requests().last().unwrap()["kind"], "unload");
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn the_painter_notes_the_time_of_a_finished_image_only() {
            let (dir, root, cache) = (temp_dir(), temp_dir(), temp_dir());
            let store = Store::open_in_memory().unwrap();
            let engine = ImageGen::new(fake_config(dir.path()));
            let models = models(root.path(), cache.path(), true);
            let model = install(&store, root.path(), "tiny", "Tiny Q4");
            let painter = EnginePainter {
                engine: &engine,
                models: &models,
                store: &store,
                gpu: None,
            };
            let request = |prompt: &str| PaintRequest {
                prompt: prompt.into(),
                width: 256,
                height: 256,
            };
            // Échec après une première étape vue : rien.
            let mut steps = 0;
            let failed = painter
                .paint(request("a cat [slow] [late-fail]"), &mut |progress| {
                    if progress.stage == ImageStage::Generating {
                        steps += 1;
                    }
                })
                .await;
            assert!(failed.is_err(), "{failed:?}");
            assert!(steps > 0, "a step was seen before the failure");
            assert_eq!(last_seconds(&store, &model.id), None);
            // Arrêt (futur lâché) en pleine image : rien.
            let stopped = tokio::time::timeout(
                Duration::from_millis(900),
                painter.paint(request("a cat [slow]"), &mut |_| {}),
            )
            .await;
            assert!(stopped.is_err(), "{stopped:?}");
            assert_eq!(last_seconds(&store, &model.id), None);
            // Image faite : 400 ms de chargement puis quatre étapes de 400 ms.
            // De la première étape à l'image : ≈ 1,2 s, soit 1 s (2 s si le
            // chargement ou le démarrage comptaient).
            painter
                .paint(request("a cat [slow]"), &mut |_| {})
                .await
                .unwrap();
            assert_eq!(last_seconds(&store, &model.id), Some(1));
            // Relu par la liste du ModelHub.
            let downloads = crate::image_models::ImageDownloads::with_base_url(
                |_| {},
                root.path().to_path_buf(),
                None,
            );
            let listed = models.list(&downloads, &store).await.unwrap();
            assert_eq!(
                listed
                    .iter()
                    .find(|info| info.id == model.id)
                    .and_then(|info| info.measured_seconds),
                Some(1)
            );
            engine.shutdown().await;
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn the_app_painter_tells_why_it_cannot_paint() {
            let (dir, root, cache) = (temp_dir(), temp_dir(), temp_dir());
            let store = Store::open_in_memory().unwrap();
            // Sans carte graphique : calcul sur le processeur, 512 par défaut
            // (même pour une famille à 1024), aussi dans l'avancement.
            let models = models(root.path(), cache.path(), false);
            install(&store, root.path(), "tiny", "Tiny Q4");
            let engine = ImageGen::new(fake_config(dir.path()));
            let painter = EnginePainter {
                engine: &engine,
                models: &models,
                store: &store,
                gpu: None,
            };
            let ready = painter.ready().await.unwrap();
            assert_eq!(
                ready,
                Ready {
                    model_name: "Tiny Q4".into(),
                    default_side: 1024
                }
            );
            assert!(painter.cpu().await);
            assert_eq!(ready.side(true), CPU_SIDE);
            assert_eq!(ready.side(false), 1024);
            let cpu = Arc::new(Mutex::new(Vec::new()));
            let sink = cpu.clone();
            let request = PaintRequest {
                prompt: "a cat".into(),
                width: 256,
                height: 256,
            };
            painter
                .paint(request, &mut move |progress| {
                    sink.lock().unwrap().push(progress.cpu)
                })
                .await
                .unwrap();
            let cpu = cpu.lock().unwrap().clone();
            assert!(!cpu.is_empty() && cpu.iter().all(|cpu| *cpu));
            engine.shutdown().await;

            // Famille à 512 avec une carte graphique.
            let models = self::models(root.path(), cache.path(), true);
            install(&store, root.path(), "tiny-sd", "Tiny SD");
            let painter = EnginePainter {
                engine: &engine,
                models: &models,
                store: &store,
                gpu: None,
            };
            assert_eq!(painter.ready().await.unwrap().side(false), 512);

            // Moteur absent, processeur sans AVX2.
            let mut absent = fake_config(dir.path());
            absent.bin = None;
            let absent = ImageGen::new(absent);
            let painter = EnginePainter {
                engine: &absent,
                models: &models,
                store: &store,
                gpu: None,
            };
            assert_eq!(painter.ready().await, Err(PaintRefusal::Unavailable));
            let mut old = fake_config(dir.path());
            old.cpu_missing = || &["AVX2", "FMA"];
            let old = ImageGen::new(old);
            let painter = EnginePainter {
                engine: &old,
                models: &models,
                store: &store,
                gpu: None,
            };
            assert_eq!(
                painter.ready().await,
                Err(PaintRefusal::CpuUnsupported("AVX2, FMA".into()))
            );
        }
    }
}
