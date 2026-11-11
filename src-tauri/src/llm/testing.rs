//! Bancs d'essai communs des lots (contrat 102 § 1.5) :
//!
//! - [`FakeBench`] : base temporaire, lignes `local_models` vers des
//!   fichiers `chatonlm-<scénario>.gguf` (en-tête GGUF lisible), moteur sur
//!   le faux `llama-server` du banc de L3 (`engine::tests`), [`Access`]
//!   complet (aucun modèle distant, moteurs de recherche fermés).
//! - [`RealBench`] : le vrai `llama-server` (`CHATONLM_LLAMA_TEST_BIN`) et
//!   un vrai modèle par rôle ([`Role`]), lus dans
//!   `$CHATONLM_LLAMA_TEST_MODELS/models.json` (écrit par `test-llama.sh
//!   fetch`, lot L8) ; à défaut, `Text`, `Vision` et `Fim` dans
//!   `.tools/runtimes/llama-proof/models/`. `None` (dit par l'essai) si
//!   le binaire ou le modèle manque. La base pointe le dossier des modèles
//!   sur ce dossier : AUCUN essai n'y écrit ni n'y supprime (copier d'abord).
//!   `CHATONLM_LLAMA_TEST_CPU=1` : processeur seulement (la carte reste
//!   libre, Vulkan par défaut).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;

use super::{Access, remote};
use crate::engine::{self, Engine, Models};
use crate::error::AppResult;
use crate::hub::downloads::HubDownloads;
use crate::hub::hardware::{Detector, Machine};
use crate::hub::types::HubGpu;
use crate::local_models::id::ModelId;
use crate::local_models::{LocalModel, ModelCapabilities, ModelSource};
use crate::store::Store;
use crate::store::local_models::StoredLocalModel;

const GIB: u64 = 1024 * 1024 * 1024;

/// Modèles de la base de l'essai, machine imposée (aucune détection réelle
/// pour le faux ; la vraie pour [`RealBench`]).
struct BenchModels {
    store: Store,
    data_dir: PathBuf,
    machine: Option<Machine>,
    real: bool,
    detector: Detector,
}

impl Models for BenchModels {
    fn local<'a>(&'a self, id: &'a str) -> BoxFuture<'a, AppResult<Option<LocalModel>>> {
        Box::pin(crate::local_models::get(&self.store, &self.data_dir, id))
    }

    fn machine(&self) -> BoxFuture<'_, Option<Machine>> {
        self.machine_now()
    }

    fn machine_now(&self) -> BoxFuture<'_, Option<Machine>> {
        Box::pin(async move {
            if self.real {
                self.detector.machine_within(Duration::from_secs(2)).await
            } else {
                self.machine.clone()
            }
        })
    }
}

/// Carte factice de 24 Gio, 20 libres ; 64 Gio de RAM.
fn fake_machine() -> Machine {
    Machine {
        gpus: vec![HubGpu {
            name: "Fake GPU".into(),
            vram_total: 24 * GIB,
            vram_free: Some(20 * GIB),
        }],
        ram_total: 64 * GIB,
        ram_available: 48 * GIB,
    }
}

/// Accès complet sur ce moteur et cette base : aucun modèle distant,
/// moteurs de recherche fermés (défaut des essais).
fn access(engine: Engine, store: Store, data_dir: PathBuf) -> Access {
    let downloads = HubDownloads::with_options(|_| {}, Default::default());
    downloads.attach(store.clone(), data_dir.clone());
    Access::new(
        engine,
        store,
        data_dir,
        Arc::new(remote::Unavailable),
        downloads,
    )
}

/// En-tête GGUF v3 minimal et lisible (`hub::gguf`, `hub::gguf::meta`) :
/// architecture, contexte d'entraînement, taille annoncée.
pub(crate) fn gguf_header(arch: &str, context: u32, size_label: &str) -> Vec<u8> {
    let mut out = b"GGUF".to_vec();
    out.extend(3u32.to_le_bytes());
    out.extend(0u64.to_le_bytes());
    out.extend(3u64.to_le_bytes());
    let string = |out: &mut Vec<u8>, text: &str| {
        out.extend((text.len() as u64).to_le_bytes());
        out.extend(text.as_bytes());
    };
    string(&mut out, "general.architecture");
    out.extend(8u32.to_le_bytes());
    string(&mut out, arch);
    string(&mut out, &format!("{arch}.context_length"));
    out.extend(4u32.to_le_bytes());
    out.extend(context.to_le_bytes());
    string(&mut out, "general.size_label");
    out.extend(8u32.to_le_bytes());
    string(&mut out, size_label);
    out
}

/// Capacités d'après le nom du scénario (`fake-llama`).
fn capabilities_of(scenario: &str) -> ModelCapabilities {
    let has = |words: &[&str]| words.iter().any(|word| scenario.contains(word));
    ModelCapabilities {
        vision: has(&["vision"]),
        audio: false,
        tools: has(&["tools", "parallel", "noid"]),
        thinking: has(&["think", "effort"]),
        fim: has(&["fim"]),
        base: has(&["fim"]),
    }
}

/// Banc du faux moteur.
pub(crate) struct FakeBench {
    pub access: Access,
    dir: tempfile::TempDir,
}

impl FakeBench {
    /// Base et modèles `(identifiant, scénario)` : le fichier s'appelle
    /// `chatonlm-<scénario>.gguf` (le faux choisit son comportement d'après
    /// l'alias, c.-à-d. l'identifiant, puis le nom du fichier : un
    /// identifiant ne doit donc pas contenir d'autre `chatonlm-<nom>`). Un
    /// scénario dont le nom contient `vision` reçoit un projecteur.
    /// Installés dans l'ordre donné (le dernier est le plus récent).
    pub fn new(models: &[(&str, &str)]) -> FakeBench {
        Self::with_config(models, |_| {})
    }

    /// Comme [`FakeBench::new`], configuration du moteur retouchée par
    /// `adjust` (binaire absent, délais…).
    pub fn with_config(
        models: &[(&str, &str)],
        adjust: impl FnOnce(&mut engine::Config),
    ) -> FakeBench {
        let dir = engine::tests::temp_dir();
        let data_dir = dir.path().join("data");
        let models_dir = crate::local_models::default_models_dir(&data_dir);
        let store = Store::open_in_memory().unwrap();
        for (index, (id, scenario)) in models.iter().enumerate() {
            let folder = match ModelId::parse(id).expect("model id") {
                ModelId::Hf { author, repo, .. } => format!("{author}/{repo}"),
                ModelId::Ollama { name, tag } => format!("@ollama/{name}/{tag}"),
            };
            let file = format!("{folder}/chatonlm-{scenario}.gguf");
            let path = crate::local_models::absolute(&models_dir, &file).unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, gguf_header("qwen3", 40_960, "0.6B")).unwrap();
            let capabilities = capabilities_of(scenario);
            let mmproj = capabilities.vision.then(|| {
                let file = format!("{folder}/mmproj-chatonlm.gguf");
                let path = crate::local_models::absolute(&models_dir, &file).unwrap();
                std::fs::write(&path, gguf_header("clip", 0, "")).unwrap();
                file
            });
            let row = StoredLocalModel {
                id: (*id).to_owned(),
                source: if id.starts_with('@') {
                    ModelSource::Ollama
                } else {
                    ModelSource::Hf
                },
                repo: folder.clone(),
                revision: None,
                quant: ModelId::parse(id).unwrap().quant().to_owned(),
                files: vec![file],
                mmproj,
                size_bytes: GIB / 2,
                sha256: Default::default(),
                arch: Some("qwen3".into()),
                context_max: Some(40_960),
                params_label: Some("0.6B".into()),
                file_type: None,
                capabilities,
                installed_at: 1_000 + index as i64,
            };
            store
                .blocking(move |c| crate::store::local_models::save(c, &row))
                .unwrap();
        }
        let mut config = engine::tests::fake_config(&data_dir);
        adjust(&mut config);
        let engine = Engine::new(
            config,
            Arc::new(BenchModels {
                store: store.clone(),
                data_dir: data_dir.clone(),
                machine: Some(fake_machine()),
                real: false,
                detector: Detector::default(),
            }),
        );
        FakeBench {
            access: access(engine, store, data_dir),
            dir,
        }
    }

    pub fn engine(&self) -> &Engine {
        self.access.engine()
    }

    pub fn store(&self) -> &Store {
        self.access.store()
    }

    /// Dossier temporaire du banc (base, données, modèles).
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    /// Réglages de l'app (contexte, durée, distants, recherche web).
    pub fn set(&self, patch: crate::store::settings::SettingsPatch) {
        self.store()
            .blocking(|c| crate::store::settings::update(c, patch).map(drop))
            .unwrap();
    }

    /// Arrête tous les processus du banc.
    pub async fn shutdown(&self) {
        self.engine().shutdown().await;
    }
}

/// Rôle d'un vrai modèle d'essai (contrat 102 § 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// Qwen3 0.6B : outils, réflexion.
    Text,
    Vision,
    /// Sans réflexion (gemma3 270m).
    #[allow(
        dead_code,
        reason = "rôle de models.json (contrat 102 § 1.5), sans essai qui le prenne encore"
    )]
    Tiny,
    /// Qwen2.5-Coder 0.5B de base (`/infill`).
    Fim,
    /// SmolLM2.
    Plain,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Role::Text => "text",
            Role::Vision => "vision",
            Role::Tiny => "tiny",
            Role::Fim => "fim",
            Role::Plain => "plain",
        }
    }
}

/// Modèle d'un rôle : identifiant, poids (relatifs au dossier des modèles,
/// morceaux dans l'ordre), projecteur.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RealModel {
    pub id: String,
    pub files: Vec<String>,
    pub mmproj: Option<String>,
}

/// Modèle du rôle dans `models.json` (`{version: 1, models: [{role, id,
/// files, mmproj, …}]}`), sinon dans le dossier de preuve.
pub(crate) fn real_model(models_json: Option<&str>, role: Role) -> Option<RealModel> {
    if let Some(text) = models_json {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let entry = value["models"]
            .as_array()?
            .iter()
            .find(|entry| entry["role"].as_str() == Some(role.name()))?;
        let files: Vec<String> = entry["files"]
            .as_array()?
            .iter()
            .filter_map(|file| file.as_str().map(str::to_owned))
            .collect();
        return (!files.is_empty()).then(|| RealModel {
            id: entry["id"].as_str().unwrap_or_default().to_owned(),
            files,
            mmproj: entry["mmproj"].as_str().map(str::to_owned),
        });
    }
    let (id, file, mmproj) = match role {
        Role::Text => (
            "Qwen/Qwen3-0.6B-GGUF:Q4_K_M",
            "Qwen3-0.6B-Q4_K_M.gguf",
            None,
        ),
        Role::Vision => (
            "ggml-org/SmolVLM-256M-Instruct-GGUF:Q8_0",
            "SmolVLM-256M-Instruct-Q8_0.gguf",
            Some("mmproj-SmolVLM-256M-Instruct-Q8_0.gguf"),
        ),
        Role::Fim => (
            "ggml-org/Qwen2.5-Coder-0.5B-Q8_0-GGUF:Q8_0",
            "qwen2.5-coder-0.5b-q8_0.gguf",
            None,
        ),
        Role::Tiny | Role::Plain => return None,
    };
    Some(RealModel {
        id: id.into(),
        files: vec![file.into()],
        mmproj: mmproj.map(str::to_owned),
    })
}

/// Banc du vrai moteur.
pub(crate) struct RealBench {
    pub access: Access,
    pub id: String,
    _dir: tempfile::TempDir,
}

impl RealBench {
    /// Vrai `llama-server` et vrai modèle du rôle ; `None` (et l'essai le
    /// dit) si l'un manque.
    pub fn new(role: Role) -> Option<RealBench> {
        let Some(bin) = std::env::var_os("CHATONLM_LLAMA_TEST_BIN")
            .map(PathBuf::from)
            .filter(|bin| bin.is_file())
        else {
            eprintln!("RealBench: CHATONLM_LLAMA_TEST_BIN missing, test skipped");
            return None;
        };
        let (models_dir, model) = Self::find(role)?;
        let dir = engine::tests::temp_dir();
        let data_dir = dir.path().join("data");
        let store = Store::open(&dir.path().join("chatonlm.sqlite")).unwrap();
        let read = |file: &str| {
            crate::hub::gguf::meta::read(&crate::local_models::absolute(&models_dir, file)?).ok()
        };
        let meta = read(&model.files[0]).unwrap_or_default();
        let projector = model.mmproj.as_deref().and_then(read);
        let size = model
            .files
            .iter()
            .chain(model.mmproj.as_ref())
            .filter_map(|file| crate::local_models::absolute(&models_dir, file))
            .filter_map(|path| std::fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .sum();
        let parsed = ModelId::parse(&model.id).expect("model id");
        let row = StoredLocalModel {
            id: model.id.clone(),
            source: ModelSource::Hf,
            repo: parsed.repo_id().unwrap_or_default(),
            revision: None,
            quant: parsed.quant().to_owned(),
            files: model.files.clone(),
            mmproj: model.mmproj.clone(),
            size_bytes: size,
            sha256: Default::default(),
            arch: meta.architecture.clone(),
            context_max: meta.context_length.and_then(|n| u32::try_from(n).ok()),
            params_label: meta.size_label.clone(),
            file_type: meta.file_type,
            capabilities: ModelCapabilities {
                vision: projector
                    .as_ref()
                    .is_some_and(crate::hub::gguf::meta::ModelMeta::sees),
                audio: projector
                    .as_ref()
                    .is_some_and(crate::hub::gguf::meta::ModelMeta::hears),
                tools: meta.tools,
                thinking: meta.thinking,
                fim: meta.fim,
                base: meta.base,
            },
            installed_at: 1_000,
        };
        let dir_text = models_dir.to_string_lossy().into_owned();
        store
            .blocking(move |c| {
                crate::store::local_models::set_models_dir(c, Some(&dir_text))?;
                crate::store::local_models::save(c, &row)
            })
            .unwrap();
        let mut config = engine::Config::new(Some(bin), &data_dir);
        config.timing.ready_timeout = Duration::from_secs(300);
        let engine = Engine::new(
            config,
            Arc::new(BenchModels {
                store: store.clone(),
                data_dir: data_dir.clone(),
                machine: None,
                real: true,
                detector: Detector::default(),
            }),
        );
        Some(RealBench {
            access: access(engine, store, data_dir),
            id: model.id,
            _dir: dir,
        })
    }

    /// Dossier des modèles et modèle du rôle.
    fn find(role: Role) -> Option<(PathBuf, RealModel)> {
        let test_models = std::env::var_os("CHATONLM_LLAMA_TEST_MODELS").map(PathBuf::from);
        if let Some(dir) = &test_models
            && let Ok(text) = std::fs::read_to_string(dir.join("models.json"))
        {
            let model = real_model(Some(&text), role);
            if model.is_none() {
                eprintln!("RealBench: no {role:?} model in models.json, test skipped");
            }
            return model.map(|model| (dir.clone(), model));
        }
        // Dossier de preuve : à côté du dossier des modèles d'essai (arbre
        // de travail), ou sous `.tools` de cet arbre.
        let proofs = test_models
            .as_deref()
            .and_then(|dir| dir.parent()?.parent())
            .map(|tools| tools.join("runtimes/llama-proof/models"))
            .into_iter()
            .chain([
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tools/runtimes/llama-proof/models")
            ]);
        let model = real_model(None, role);
        for dir in proofs {
            if let Some(model) = &model
                && model
                    .files
                    .iter()
                    .chain(model.mmproj.as_ref())
                    .all(|file| dir.join(file).is_file())
            {
                return Some((dir, model.clone()));
            }
        }
        eprintln!("RealBench: no {role:?} model found, test skipped");
        None
    }

    /// Options d'une ouverture du banc (processeur seulement si
    /// `CHATONLM_LLAMA_TEST_CPU=1`).
    pub fn options(&self) -> super::OpenOptions {
        super::OpenOptions {
            cpu_only: std::env::var("CHATONLM_LLAMA_TEST_CPU").as_deref() == Ok("1"),
            ..super::OpenOptions::default()
        }
    }

    pub async fn shutdown(&self) {
        self.access.engine().shutdown().await;
    }
}

#[test]
fn the_models_file_gives_each_role_its_model() {
    let json = r#"{"version": 1, "models": [
        {"role": "text", "id": "Qwen/Qwen3-0.6B-GGUF:Q4_K_M",
         "files": ["Qwen/Qwen3-0.6B-GGUF/Qwen3-0.6B-Q4_K_M.gguf"], "mmproj": null,
         "sha256": {}, "revision": "abc"},
        {"role": "vision", "id": "a/V-GGUF:Q8_0", "files": ["a/V-GGUF/v.gguf"],
         "mmproj": "a/V-GGUF/mmproj.gguf"}
    ]}"#;
    assert_eq!(
        real_model(Some(json), Role::Text),
        Some(RealModel {
            id: "Qwen/Qwen3-0.6B-GGUF:Q4_K_M".into(),
            files: vec!["Qwen/Qwen3-0.6B-GGUF/Qwen3-0.6B-Q4_K_M.gguf".into()],
            mmproj: None,
        })
    );
    assert_eq!(
        real_model(Some(json), Role::Vision)
            .unwrap()
            .mmproj
            .as_deref(),
        Some("a/V-GGUF/mmproj.gguf")
    );
    assert_eq!(real_model(Some(json), Role::Fim), None);
    // Sans fichier : le dossier de preuve, pour trois rôles seulement.
    assert!(real_model(None, Role::Text).is_some());
    assert!(real_model(None, Role::Fim).is_some());
    assert_eq!(real_model(None, Role::Plain), None);
}

#[test]
fn the_fake_header_is_read_like_a_real_one() {
    let dir = engine::tests::temp_dir();
    let path = dir.path().join("x.gguf");
    std::fs::write(&path, gguf_header("qwen3", 40_960, "0.6B")).unwrap();
    let meta = crate::hub::gguf::meta::read(&path).unwrap();
    assert_eq!(meta.architecture.as_deref(), Some("qwen3"));
    assert_eq!(meta.context_length, Some(40_960));
    assert_eq!(meta.size_label.as_deref(), Some("0.6B"));
}
