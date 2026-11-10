//! Essais de la reprise : faux dossiers d'Ollama (manifestes à la forme des
//! vrais, relevés dans l'Ollama de test, blobs synthétisés), bases 0.9.5,
//! faux Hugging Face pour retrouver un modèle `hf.co/…` par son empreinte.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fake_services::hf::{FakeHf, HfMode};
use fake_services::hf_models::Synth;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use super::compat::tests::{projector, weights};
use super::*;
use crate::store::Store;

const MODEL_TYPE: &str = "application/vnd.ollama.image.model";
const PROJECTOR_TYPE: &str = "application/vnd.ollama.image.projector";
const PARAMS_TYPE: &str = "application/vnd.ollama.image.params";
const SYSTEM_TYPE: &str = "application/vnd.ollama.image.system";
const TEMPLATE_TYPE: &str = "application/vnd.ollama.image.template";
const DOCKER: &str = "application/vnd.docker.distribution.manifest.v2+json";

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Faux dossier d'Ollama.
struct Ollama {
    dir: PathBuf,
}

impl Ollama {
    fn new(dir: PathBuf) -> Self {
        fs::create_dir_all(dir.join("blobs")).unwrap();
        Self { dir }
    }

    /// Écrit un blob ; rend sa couche JSON.
    fn blob(&self, media_type: &str, bytes: &[u8]) -> serde_json::Value {
        let hex = sha(bytes);
        fs::write(self.dir.join("blobs").join(format!("sha256-{hex}")), bytes).unwrap();
        serde_json::json!({"mediaType": media_type, "digest": format!("sha256:{hex}"), "size": bytes.len()})
    }

    /// Manifeste Docker v2 de ces couches (rangé à l'ancienne sous
    /// `manifests/<chemin>`), comme Ollama les écrit.
    fn manifest(
        &self,
        path: &str,
        layers: Vec<serde_json::Value>,
        extra: serde_json::Value,
    ) -> Vec<u8> {
        let mut manifest = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": DOCKER,
            "config": {"mediaType": "application/vnd.docker.container.image.v1+json",
                       "digest": format!("sha256:{}", "0".repeat(64)), "size": 2},
            "layers": layers,
        });
        if let serde_json::Value::Object(extra) = extra {
            manifest.as_object_mut().unwrap().extend(extra);
        }
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let target = self.dir.join("manifests").join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, &bytes).unwrap();
        bytes
    }

    fn blob_path(&self, layer: &serde_json::Value) -> PathBuf {
        let digest = layer["digest"].as_str().unwrap().replace(':', "-");
        self.dir.join("blobs").join(digest)
    }
}

/// Contenu de chaque fichier sous `dir` (chemin relatif → empreinte), pour
/// prouver qu'un dossier n'a pas changé.
fn snapshot(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(&folder).unwrap().flatten() {
            let kind = entry.file_type().unwrap();
            let relative = entry
                .path()
                .strip_prefix(dir)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_symlink() {
                out.push((
                    relative,
                    format!("-> {:?}", fs::read_link(entry.path()).unwrap()),
                ));
            } else {
                // Liens physiques comptés (Unix) : un lien posé vers un
                // fichier le change aussi.
                #[cfg(unix)]
                let links = {
                    use std::os::unix::fs::MetadataExt;
                    fs::metadata(entry.path()).unwrap().nlink()
                };
                #[cfg(not(unix))]
                let links = 1;
                out.push((
                    relative,
                    format!("{} x{links}", sha(&fs::read(entry.path()).unwrap())),
                ));
            }
        }
    }
    out.sort();
    out
}

/// Base 0.9.5 (schéma 38) avec des réglages qui nomment des modèles
/// d'Ollama, ouverte (migrations 0039 et 0040 appliquées).
fn database_0_9_5(dir: &Path) -> Store {
    let path = dir.join("chatonlm.sqlite");
    let connection = crate::store::database_at(&path, 38);
    connection
        .execute_batch(
            "UPDATE settings SET selected_model = 'qwen3',
                 study_model = 'hf.co/chatonlm-dl/Tiny-vision-GGUF:Q4_K_M',
                 proofread_model = 'gemma3:4b';",
        )
        .unwrap();
    drop(connection);
    Store::open(&path).unwrap()
}

fn context(store: &Store, data_dir: &Path, hf_url: &str) -> Context {
    Context {
        store: store.clone(),
        data_dir: data_dir.to_owned(),
        hf_url: hf_url.to_owned(),
        cancel: Arc::new(AtomicBool::new(false)),
        downloads_active: Arc::new(|| false),
    }
}

/// Dossier d'un Ollama 0.9.x : qwen3 (deux noms, réglages), gemma3 4b au
/// format d'Ollama, qwen3.5 en liste avec sa variante llamacpp, un modèle de
/// Hugging Face, un modèle du cloud, un modèle au blob manquant.
struct Fixture {
    ollama: Ollama,
    qwen3: serde_json::Value,
    qwen35_weights: serde_json::Value,
    qwen35_projector: serde_json::Value,
    qwen35_ggml: serde_json::Value,
    gemma: serde_json::Value,
    hf_weights: serde_json::Value,
    hf_projector: serde_json::Value,
    synth: Synth,
}

fn fixture(dir: PathBuf) -> Fixture {
    let ollama = Ollama::new(dir);
    // qwen3 : même manifeste sous deux étiquettes ; rangement 0.40
    // (`manifests-v2`, lien vers le blob du manifeste) pour l'une.
    let qwen3 = ollama.blob(MODEL_TYPE, &weights("qwen3", &[], 1));
    let params = ollama.blob(
        PARAMS_TYPE,
        br#"{"repeat_penalty":1,"stop":["<|im_start|>","<|im_end|>"],"temperature":0.6,"top_k":20,"top_p":0.95}"#,
    );
    let system = ollama.blob(SYSTEM_TYPE, b"You are a cat.");
    let template = ollama.blob(TEMPLATE_TYPE, b"{{ .Prompt }}");
    let layers = vec![qwen3.clone(), template, params, system];
    let bytes = ollama.manifest(
        "registry.ollama.ai/library/qwen3/0.6b",
        layers.clone(),
        serde_json::json!({}),
    );
    ollama.manifest(
        "registry.ollama.ai/library/qwen3/latest",
        layers,
        serde_json::json!({}),
    );
    let manifest_blob = ollama.blob(DOCKER, &bytes);
    let v2 = ollama.dir.join("manifests-v2/ollama.com/library/qwen3");
    fs::create_dir_all(&v2).unwrap();
    // Lien de `manifests-v2/` vers ce blob : Unix seulement (le blob reste
    // écrit partout, comme chez Ollama).
    #[cfg(not(unix))]
    let _ = manifest_blob;
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        format!(
            "../../../../blobs/{}",
            manifest_blob["digest"].as_str().unwrap().replace(':', "-")
        ),
        v2.join("0.6b"),
    )
    .unwrap();
    // gemma3:4b : vision fondue dans les poids.
    let gemma = ollama.blob(
        MODEL_TYPE,
        &weights("gemma3", &["v.blk.0.attn_q.weight", "mm.0.weight"], 2),
    );
    ollama.manifest(
        "registry.ollama.ai/library/gemma3/4b",
        vec![gemma.clone()],
        serde_json::json!({}),
    );
    // qwen3.5:0.8b : liste ; variante ggml (fondue) et llamacpp (séparée).
    let qwen35_ggml = ollama.blob(MODEL_TYPE, &weights("qwen35", &["v.blk.0.weight"], 3));
    let qwen35_weights = ollama.blob(MODEL_TYPE, &weights("qwen35", &[], 4));
    let qwen35_projector = ollama.blob(PROJECTOR_TYPE, &projector(5));
    let ggml_manifest = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 2, "mediaType": DOCKER, "layers": [qwen35_ggml], "runner": "ggml", "format": "gguf"
    }))
    .unwrap();
    let llamacpp_manifest = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 2, "mediaType": DOCKER,
        "layers": [qwen35_projector, qwen35_weights], "runner": "llamacpp", "format": "gguf"
    }))
    .unwrap();
    let ggml_blob = ollama.blob(DOCKER, &ggml_manifest);
    let llamacpp_blob = ollama.blob(DOCKER, &llamacpp_manifest);
    let list = ollama.blob(
        "application/vnd.ollama.manifest.list.v2+json",
        &serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.ollama.manifest.list.v2+json",
            "manifests": [
                {"mediaType": DOCKER, "digest": ggml_blob["digest"], "runner": "ggml"},
                {"mediaType": DOCKER, "digest": llamacpp_blob["digest"], "runner": "llamacpp"},
            ]
        }))
        .unwrap(),
    );
    ollama.manifest(
        "registry.ollama.ai/library/qwen3.5/0.8b",
        vec![qwen35_ggml.clone(), list, ggml_blob, llamacpp_blob.clone()],
        serde_json::json!({"runner": "ggml", "format": "gguf"}),
    );
    // Variante rangée par Ollama 0.40 : pas un modèle à montrer.
    ollama.manifest(
        &format!(
            "registry.ollama.ai/library/llamacpp/{}",
            llamacpp_blob["digest"]
                .as_str()
                .unwrap()
                .trim_start_matches("sha256:")
        ),
        vec![qwen35_weights.clone()],
        serde_json::json!({}),
    );
    // Modèle de Hugging Face (fichiers du faux Hugging Face).
    let synth = Synth::parse("chatonlm-dl/Tiny-vision-GGUF", 0).unwrap();
    let hf_weights = ollama.blob(MODEL_TYPE, &synth.content("Tiny-Q4_K_M.gguf").unwrap());
    let hf_projector = ollama.blob(PROJECTOR_TYPE, &synth.content("mmproj-Q8_0.gguf").unwrap());
    ollama.manifest(
        "hf.co/chatonlm-dl/Tiny-vision-GGUF/Q4_K_M",
        vec![hf_weights.clone(), hf_projector.clone()],
        serde_json::json!({}),
    );
    // Cloud et blob manquant.
    ollama.manifest(
        "registry.ollama.ai/library/gpt-oss/120b-cloud",
        vec![],
        serde_json::json!({"config": {"remote_host": "https://ollama.com:443", "remote_model": "gpt-oss:120b"}}),
    );
    ollama.manifest(
        "registry.ollama.ai/library/lost/1b",
        vec![serde_json::json!({"mediaType": MODEL_TYPE, "digest": format!("sha256:{}", "e".repeat(64)), "size": 10})],
        serde_json::json!({}),
    );
    Fixture {
        ollama,
        qwen3,
        qwen35_weights,
        qwen35_projector,
        qwen35_ggml,
        gemma,
        hf_weights,
        hf_projector,
        synth,
    }
}

/// Le même fichier (lien physique) ; sous Windows, même contenu.
fn same_file(a: &Path, b: &Path) -> bool {
    if cfg!(unix) {
        super::os::same_file(a, b)
    } else {
        fs::read(a).unwrap() == fs::read(b).unwrap()
    }
}

fn by_name<'a>(source: &'a ImportSource, name: &str) -> &'a ImportCandidate {
    source
        .models
        .iter()
        .find(|model| model.name == name)
        .unwrap_or_else(|| panic!("{name} in {:?}", source.models))
}

#[tokio::test]
async fn a_builtin_ollama_is_listed_with_what_will_happen() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let fixture = fixture(data.path().join("ollama").join("models"));
    let source = source(
        &store,
        data.path(),
        ImportOrigin::Builtin,
        &fixture.ollama.dir,
        false,
    )
    .await
    .unwrap();
    assert!(source.readable);
    assert!(source.same_drive);
    let names: Vec<&str> = source
        .models
        .iter()
        .map(|model| model.name.as_str())
        .collect();
    // La variante rangée sous `library/llamacpp` n'est pas un modèle ; les
    // deux noms de qwen3 n'en font qu'un.
    assert_eq!(
        names,
        [
            "gemma3:4b",
            "gpt-oss:120b-cloud",
            "hf.co/chatonlm-dl/Tiny-vision-GGUF:Q4_K_M",
            "lost:1b",
            "qwen3.5:0.8b",
            "qwen3:0.6b",
        ]
    );
    let qwen3 = by_name(&source, "qwen3:0.6b");
    assert_eq!(qwen3.aliases, ["qwen3:latest"]);
    assert_eq!(qwen3.status, CandidateStatus::Ready);
    assert_eq!(qwen3.id.as_deref(), Some("@ollama/qwen3:0.6b"));
    let gemma = by_name(&source, "gemma3:4b");
    assert_eq!(gemma.status, CandidateStatus::Redownload);
    assert_eq!(gemma.problem, Some(ImportProblem::OllamaFormat));
    assert_eq!(
        gemma.suggestion.as_deref(),
        Some("ggml-org/gemma-3-4b-it-GGUF:Q4_K_M")
    );
    let qwen35 = by_name(&source, "qwen3.5:0.8b");
    assert_eq!(qwen35.status, CandidateStatus::Ready);
    assert!(qwen35.vision);
    assert_eq!(
        qwen35.size_bytes,
        fixture.qwen35_weights["size"].as_u64().unwrap()
            + fixture.qwen35_projector["size"].as_u64().unwrap()
    );
    assert_eq!(
        by_name(&source, "gpt-oss:120b-cloud").problem,
        Some(ImportProblem::Cloud)
    );
    assert_eq!(
        by_name(&source, "lost:1b").problem,
        Some(ImportProblem::BlobMissing)
    );
    assert_eq!(
        by_name(&source, "hf.co/chatonlm-dl/Tiny-vision-GGUF:Q4_K_M")
            .id
            .as_deref(),
        Some("chatonlm-dl/Tiny-vision-GGUF:Q4_K_M")
    );
}

/// Reprise de l'intégré sur le même disque : blobs renommés dans le
/// rangement, lignes de la base, réglages d'Ollama gardés, réglages de
/// l'app réécrits, puis ménage après confirmation.
#[tokio::test]
async fn the_builtin_ollama_is_taken_over() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let fixture = fixture(data.path().join("ollama").join("models"));
    // Clé du compte ollama.com et fichier de processus de l'intégré.
    fs::create_dir_all(data.path().join("ollama/home/.ollama")).unwrap();
    fs::write(data.path().join("ollama/home/.ollama/id_ed25519"), b"key").unwrap();
    let fake = FakeHf::start(HfMode::Normal).await.unwrap();
    let job = Job {
        origin: ImportOrigin::Builtin,
        dir: fixture.ollama.dir.clone(),
        names: None,
        shared: false,
        retry_failed: false,
    };
    let seen = Arc::new(Mutex::new(Vec::new()));
    let report = {
        let seen = seen.clone();
        run(
            job.clone(),
            context(&store, data.path(), &fake.url()),
            move |progress| seen.lock().push(progress),
        )
        .await
    };
    assert_eq!(report.error, None);
    assert!(!report.cancelled);
    assert_eq!(report.failed, []);
    let mut imported: Vec<(String, String)> = report
        .imported
        .iter()
        .map(|model| (model.name.clone(), model.id.clone()))
        .collect();
    imported.sort();
    assert_eq!(
        imported,
        [
            (
                "hf.co/chatonlm-dl/Tiny-vision-GGUF:Q4_K_M".into(),
                "chatonlm-dl/Tiny-vision-GGUF:Q4_K_M".into()
            ),
            ("qwen3.5:0.8b".into(), "@ollama/qwen3.5:0.8b".into()),
            ("qwen3:0.6b".into(), "@ollama/qwen3:0.6b".into()),
            ("qwen3:latest".into(), "@ollama/qwen3:0.6b".into()),
        ]
    );
    assert_eq!(
        report.redownload,
        [RedownloadModel {
            name: "gemma3:4b".into(),
            suggestion: Some("ggml-org/gemma-3-4b-it-GGUF:Q4_K_M".into()),
        }]
    );
    // Renommages : aucun octet copié.
    let last = seen.lock().last().cloned().unwrap();
    assert_eq!((last.total_bytes, last.done_bytes), (0, 0));
    assert_eq!((last.done_models, last.total_models), (3, 3));

    let models_dir = crate::local_models::default_models_dir(data.path());
    // Blobs liés, pas copiés : la source reste jusqu'au ménage (une coupure
    // ne perd rien, l'intégré marche encore).
    assert!(same_file(
        &fixture.ollama.blob_path(&fixture.qwen3),
        &models_dir.join("@ollama/qwen3/0.6b/model.gguf")
    ));
    assert!(models_dir.join("@ollama/qwen3/0.6b/model.gguf").is_file());
    assert!(models_dir.join("@ollama/qwen3.5/0.8b/model.gguf").is_file());
    assert!(
        models_dir
            .join("@ollama/qwen3.5/0.8b/mmproj.gguf")
            .is_file()
    );
    // Le format d'Ollama reste où il est.
    assert!(fixture.ollama.blob_path(&fixture.gemma).exists());
    assert!(fixture.ollama.blob_path(&fixture.qwen35_ggml).exists());

    let local = crate::local_models::list(&store, data.path())
        .await
        .unwrap();
    let get = |id: &str| local.iter().find(|model| model.id == id).unwrap().clone();
    let qwen3 = get("@ollama/qwen3:0.6b");
    assert_eq!(qwen3.source, ModelSource::Ollama);
    assert_eq!(qwen3.repo, "qwen3:0.6b");
    assert_eq!(qwen3.display_name, "qwen3:0.6b");
    assert_eq!(qwen3.arch.as_deref(), Some("qwen3"));
    assert_eq!(qwen3.context_max, Some(40960));
    assert!(qwen3.capabilities.tools);
    assert!(!qwen3.capabilities.vision);
    let qwen35 = get("@ollama/qwen3.5:0.8b");
    assert!(qwen35.capabilities.vision);
    assert!(qwen35.mmproj.is_some());
    // Hugging Face retrouvé par l'empreinte : fichiers d'origine, révision
    // suivie.
    let hf = get("chatonlm-dl/Tiny-vision-GGUF:Q4_K_M");
    assert_eq!(hf.source, ModelSource::Hf);
    assert_eq!(hf.repo, "chatonlm-dl/Tiny-vision-GGUF");
    assert!(
        hf.path
            .ends_with("chatonlm-dl/Tiny-vision-GGUF/Tiny-Q4_K_M.gguf"),
        "{}",
        hf.path
    );
    assert!(
        hf.mmproj
            .as_deref()
            .unwrap()
            .ends_with("chatonlm-dl/Tiny-vision-GGUF/mmproj-Q8_0.gguf")
    );
    let stored = store
        .call(|c| crate::store::local_models::get(c, "chatonlm-dl/Tiny-vision-GGUF:Q4_K_M"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.revision, Some(fixture.synth.sha()));
    assert_eq!(
        stored.sha256["chatonlm-dl/Tiny-vision-GGUF/Tiny-Q4_K_M.gguf"],
        fixture.hf_weights["digest"]
            .as_str()
            .unwrap()
            .trim_start_matches("sha256:")
    );
    // À jour d'après la fiche : rien à mettre à jour.
    assert_eq!(
        crate::local_models::check_outdated(&fake.url(), &stored).await,
        Some(false)
    );
    assert!(same_file(
        &fixture.ollama.blob_path(&fixture.hf_projector),
        Path::new(hf.mmproj.as_deref().unwrap())
    ));

    // Réglages d'Ollama gardés par modèle.
    let defaults = store
        .call(|c| records::defaults(c, "@ollama/qwen3:0.6b"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(defaults.params.temperature, Some(0.6));
    assert_eq!(defaults.params.top_k, Some(20));
    assert_eq!(defaults.params.stop, ["<|im_start|>", "<|im_end|>"]);
    assert_eq!(defaults.system, "You are a cat.");
    // Réglages de l'app réécrits ; le modèle à retélécharger garde son nom.
    let settings: (String, String, String) = store
        .call(|c| {
            Ok(c.query_row(
                "SELECT selected_model, study_model, proofread_model FROM settings",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(
        settings,
        (
            "@ollama/qwen3:0.6b".into(),
            "chatonlm-dl/Tiny-vision-GGUF:Q4_K_M".into(),
            "gemma3:4b".into()
        )
    );
    assert!(
        store
            .call(|c| records::builtin_import_done(c))
            .await
            .unwrap()
    );

    // Deuxième passage : tout est déjà là, rien ne bouge.
    let before = snapshot(data.path());
    let again = run(job, context(&store, data.path(), &fake.url()), |_| {}).await;
    assert_eq!(again.imported, []);
    assert_eq!(again.failed, []);
    assert_eq!(snapshot(data.path()).len(), before.len());
    let source = source(
        &store,
        data.path(),
        ImportOrigin::Builtin,
        &fixture.ollama.dir,
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        by_name(&source, "qwen3:0.6b").status,
        CandidateStatus::Installed
    );
    assert_eq!(
        by_name(&source, "hf.co/chatonlm-dl/Tiny-vision-GGUF:Q4_K_M").status,
        CandidateStatus::Installed
    );

    // Ménage : manifestes, blobs restés, HOME de l'intégré ; jamais la base
    // locale.
    let keep: HashSet<PathBuf> = local
        .iter()
        .flat_map(|model| {
            std::iter::once(PathBuf::from(&model.path))
                .chain(model.mmproj.clone().map(PathBuf::from))
        })
        .collect();
    let root = data.path().join("ollama");
    let left = cleanup::leftovers(&fixture.ollama.dir, &root, &keep);
    assert!(left.files > 0);
    let report = cleanup::cleanup(&fixture.ollama.dir, &root, &keep);
    assert_eq!(report.failed, 0);
    assert!(!root.exists());
    for model in &local {
        assert!(Path::new(&model.path).is_file(), "{}", model.path);
    }
    fake.stop().await;
}

/// Ollama de la machine : copie seulement, son dossier n'est jamais touché ;
/// Hugging Face injoignable : modèle `hf.co/…` repris sans suivi.
#[tokio::test]
async fn a_machine_ollama_is_copied_and_never_touched() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let machine = tempfile::tempdir().unwrap();
    let fixture = fixture(machine.path().join("models"));
    let before = snapshot(machine.path());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let report = {
        let seen = seen.clone();
        run(
            Job {
                origin: ImportOrigin::Machine,
                dir: fixture.ollama.dir.clone(),
                names: Some(vec![
                    "qwen3".into(),
                    "HF.CO/chatonlm-dl/Tiny-vision-GGUF:q4_k_m".into(),
                ]),
                shared: false,
                retry_failed: false,
            },
            // Adresse fermée : la fiche ne se lit pas.
            context(&store, data.path(), "http://127.0.0.1:9"),
            move |progress| seen.lock().push(progress),
        )
        .await
    };
    assert_eq!(report.error, None);
    assert_eq!(report.failed, []);
    let ids: HashSet<&str> = report
        .imported
        .iter()
        .map(|model| model.id.as_str())
        .collect();
    assert_eq!(
        ids,
        ["@ollama/qwen3:0.6b", "chatonlm-dl/Tiny-vision-GGUF:Q4_K_M"].into()
    );
    assert_eq!(
        snapshot(machine.path()),
        before,
        "the machine's Ollama was changed"
    );
    let last = seen.lock().last().cloned().unwrap();
    let expected: u64 = [&fixture.qwen3, &fixture.hf_weights, &fixture.hf_projector]
        .iter()
        .map(|layer| layer["size"].as_u64().unwrap())
        .sum();
    assert_eq!((last.done_bytes, last.total_bytes), (expected, expected));
    let hf = store
        .call(|c| crate::store::local_models::get(c, "chatonlm-dl/Tiny-vision-GGUF:Q4_K_M"))
        .await
        .unwrap()
        .unwrap();
    // Sans la fiche : sans suivi, noms construits.
    assert_eq!(hf.source, ModelSource::Ollama);
    assert_eq!(hf.revision, None);
    assert_eq!(
        hf.files,
        ["chatonlm-dl/Tiny-vision-GGUF/Tiny-vision-Q4_K_M.gguf"]
    );
    assert_eq!(
        hf.mmproj.as_deref(),
        Some("chatonlm-dl/Tiny-vision-GGUF/mmproj-Q4_K_M.gguf")
    );
    // La reprise de la machine ne marque pas celle de l'intégré.
    assert!(
        !store
            .call(|c| records::builtin_import_done(c))
            .await
            .unwrap()
    );
    // Ni le format d'Ollama ni ce qui n'est pas demandé.
    assert_eq!(report.redownload, []);
}

/// Annulation pendant le deuxième modèle : le premier reste repris, le
/// deuxième ne laisse rien, la reprise de l'intégré reste à faire.
#[tokio::test]
async fn a_cancelled_import_keeps_finished_models_only() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let fixture = fixture(data.path().join("ollama").join("models"));
    let context = context(&store, data.path(), "http://127.0.0.1:9");
    let cancel = context.cancel.clone();
    let report = run(
        Job {
            origin: ImportOrigin::Machine,
            dir: fixture.ollama.dir.clone(),
            names: Some(vec!["qwen3.5:0.8b".into(), "qwen3:0.6b".into()]),
            shared: false,
            retry_failed: false,
        },
        context,
        move |progress| {
            if progress.model.as_deref() == Some("qwen3:0.6b") {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    )
    .await;
    assert!(report.cancelled);
    let ids: Vec<&str> = report
        .imported
        .iter()
        .map(|model| model.id.as_str())
        .collect();
    assert_eq!(ids, ["@ollama/qwen3.5:0.8b"]);
    let models_dir = crate::local_models::default_models_dir(data.path());
    assert!(models_dir.join("@ollama/qwen3.5/0.8b/model.gguf").is_file());
    assert!(!models_dir.join("@ollama/qwen3").exists());
    let local = crate::local_models::list(&store, data.path())
        .await
        .unwrap();
    assert_eq!(local.len(), 1);
    // Aucun fichier partiel.
    assert!(
        snapshot(&models_dir)
            .iter()
            .all(|(path, _)| !path.ends_with(transfer::PARTIAL))
    );
}

/// Un blob abîmé (empreinte fausse) échoue à la copie : rien n'est laissé,
/// l'échec est noté, l'intégré reste à reprendre.
#[tokio::test]
async fn a_damaged_blob_fails_cleanly() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let machine = tempfile::tempdir().unwrap();
    let fixture = fixture(machine.path().join("models"));
    // Même taille, autre contenu.
    let path = fixture.ollama.blob_path(&fixture.qwen3);
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&path, bytes).unwrap();
    let report = run(
        Job {
            origin: ImportOrigin::Machine,
            dir: fixture.ollama.dir.clone(),
            names: Some(vec!["qwen3:0.6b".into()]),
            shared: false,
            retry_failed: false,
        },
        context(&store, data.path(), "http://127.0.0.1:9"),
        |_| {},
    )
    .await;
    assert_eq!(report.imported, []);
    let names: Vec<&str> = report
        .failed
        .iter()
        .map(|model| model.name.as_str())
        .collect();
    assert_eq!(names, ["qwen3:0.6b", "qwen3:latest"]);
    let models_dir = crate::local_models::default_models_dir(data.path());
    assert!(!models_dir.join("@ollama").exists());
    let records = store.call(|c| records::records(c)).await.unwrap();
    assert!(
        records
            .iter()
            .all(|record| record.status == RecordStatus::Failed)
    );
}

/// Démarrage : un intégré sans modèle marque la reprise faite ; avec des
/// modèles, tout est repris par le service, l'état final envoyé à la page.
#[tokio::test]
async fn the_service_takes_over_at_startup() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let import = service::ModelImport::new(|_| {});
    service::at_startup(&import, store.clone(), data.path().to_owned()).await;
    assert!(
        store
            .call(|c| records::builtin_import_done(c))
            .await
            .unwrap()
    );

    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let _fixture = fixture(data.path().join("ollama").join("models"));
    let states = Arc::new(Mutex::new(Vec::new()));
    let import = {
        let states = states.clone();
        service::ModelImport::new(move |state| states.lock().push(state.clone()))
    };
    service::at_startup(&import, store.clone(), data.path().to_owned()).await;
    let last = import.state().last.unwrap();
    assert_eq!(last.origin, ImportOrigin::Builtin);
    assert!(!last.imported.is_empty());
    assert!(import.state().running.is_none());
    assert!(!import.is_running());
    assert!(states.lock().iter().any(|state| state.running.is_some()));
    assert!(
        store
            .call(|c| records::builtin_import_done(c))
            .await
            .unwrap()
    );
    // Une deuxième fois : rien (déjà faite).
    let count = states.lock().len();
    service::at_startup(&import, store.clone(), data.path().to_owned()).await;
    assert_eq!(states.lock().len(), count);
    // Ménage proposé seulement après la reprise, puis fait.
    let left = service::leftovers(&store, data.path())
        .await
        .unwrap()
        .unwrap();
    assert!(left.files > 0);
    let report = service::clean(&import, &store, data.path()).await.unwrap();
    assert_eq!(report.removed_files, left.files);
    assert_eq!(service::leftovers(&store, data.path()).await.unwrap(), None);
}

#[tokio::test]
async fn one_import_at_a_time() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let import = service::ModelImport::new(|_| {});
    let job = Job {
        origin: ImportOrigin::Machine,
        dir: data.path().join("nothing"),
        names: None,
        shared: false,
        retry_failed: false,
    };
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let first = import
        .start(
            job.clone(),
            store.clone(),
            data.path().to_owned(),
            async move {
                let _ = wait.await;
            },
        )
        .unwrap();
    assert!(import.is_running());
    assert!(
        import
            .start(job, store.clone(), data.path().to_owned(), async {})
            .is_err()
    );
    release.send(()).unwrap();
    let report = first.await.unwrap();
    // Dossier absent : échec d'ensemble lisible.
    assert!(report.error.is_some());
    assert!(!import.is_running());
}

/// Changement du dossier des modèles : fichiers déplacés, réglage écrit,
/// chemins de la base relus dans le nouveau dossier ; retour au défaut.
#[tokio::test]
async fn the_models_folder_moves_with_its_models() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let fixture = fixture(data.path().join("ollama").join("models"));
    run(
        Job {
            origin: ImportOrigin::Builtin,
            dir: fixture.ollama.dir.clone(),
            names: None,
            shared: false,
            retry_failed: false,
        },
        context(&store, data.path(), "http://127.0.0.1:9"),
        |_| {},
    )
    .await;
    let before = crate::local_models::list(&store, data.path())
        .await
        .unwrap();
    assert_eq!(before.len(), 3);
    let current = folder::current(&store, data.path()).await.unwrap();
    assert!(current.is_default);
    assert_eq!(current.models, 3);
    let cancel = Arc::new(AtomicBool::new(false));
    // Refus : chemin relatif, dossier dans l'actuel.
    assert!(
        folder::change(
            &store,
            data.path(),
            "rel".into(),
            true,
            cancel.clone(),
            || Ok(()),
            |_| {}
        )
        .await
        .is_err()
    );
    let inside = PathBuf::from(&current.path).join("sub");
    assert!(
        folder::change(
            &store,
            data.path(),
            inside,
            true,
            cancel.clone(),
            || Ok(()),
            |_| {}
        )
        .await
        .is_err()
    );
    let new = data.path().join("elsewhere").join("models");
    let moved = folder::change(
        &store,
        data.path(),
        new.clone(),
        true,
        cancel.clone(),
        || Ok(()),
        |_| {},
    )
    .await
    .unwrap();
    // Même disque : renommages ; le projecteur partagé compte une fois.
    assert_eq!(moved.copied, 0);
    assert_eq!(moved.renamed, 5);
    let after = crate::local_models::list(&store, data.path())
        .await
        .unwrap();
    for model in &after {
        assert!(Path::new(&model.path).starts_with(&new), "{}", model.path);
        assert!(Path::new(&model.path).is_file());
    }
    assert!(!PathBuf::from(&current.path).join("@ollama").exists());
    let now = folder::current(&store, data.path()).await.unwrap();
    assert!(!now.is_default);
    // Retour au dossier par défaut : réglage effacé.
    folder::change(
        &store,
        data.path(),
        PathBuf::from(&current.path),
        true,
        cancel,
        || Ok(()),
        |_| {},
    )
    .await
    .unwrap();
    assert!(
        folder::current(&store, data.path())
            .await
            .unwrap()
            .is_default
    );
    assert_eq!(
        store
            .call(|c| crate::store::local_models::models_dir(c))
            .await
            .unwrap(),
        None
    );
    for model in crate::local_models::list(&store, data.path())
        .await
        .unwrap()
    {
        assert!(Path::new(&model.path).is_file());
    }
}

fn builtin_job(dir: &Path, names: Option<Vec<&str>>) -> Job {
    Job {
        origin: ImportOrigin::Builtin,
        dir: dir.to_owned(),
        names: names.map(|names| names.into_iter().map(str::to_owned).collect()),
        shared: false,
        retry_failed: false,
    }
}

async fn import_done(store: &Store) -> bool {
    store
        .call(|c| records::builtin_import_done(c))
        .await
        .unwrap()
}

async fn set_builtin_folder(store: &Store, dir: &Path) {
    let dir = dir.to_string_lossy().into_owned();
    store
        .call(move |c| {
            c.execute(
                "UPDATE builtin_ollama SET models_dir = ?1 WHERE id = 1",
                [&dir],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

/// Relecture L6, B1 : dossier de l'intégré réglé en 0.9.x sur
/// `~/.ollama/models`, celui de l'Ollama de la machine. Copies seulement :
/// rien n'y change (contenu, liens physiques), il ne devient pas le dossier
/// des modèles, et aucun ménage n'est proposé ni permis.
#[tokio::test]
async fn a_builtin_folder_shared_with_the_machine_is_only_copied() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let home = tempfile::tempdir().unwrap();
    let machine = sources::home_dir(home.path());
    let fixture = fixture(machine.clone());
    set_builtin_folder(&store, &machine).await;
    service::TEST_MACHINE_DIRS.with(|dirs| *dirs.borrow_mut() = vec![machine.clone()]);
    let before = snapshot(home.path());
    let import = service::ModelImport::new(|_| {});
    service::at_startup(&import, store.clone(), data.path().to_owned()).await;
    let last = import.state().last.unwrap();
    assert_eq!(last.error, None);
    assert_eq!(last.failed, []);
    assert!(!last.imported.is_empty());
    assert_eq!(
        snapshot(home.path()),
        before,
        "the machine's Ollama was changed"
    );
    // Pas repris comme dossier des modèles : copies dans le dossier par
    // défaut, sans lien vers les blobs de la machine.
    assert_eq!(
        store
            .call(|c| crate::store::local_models::models_dir(c))
            .await
            .unwrap(),
        None
    );
    let copied =
        crate::local_models::default_models_dir(data.path()).join("@ollama/qwen3/0.6b/model.gguf");
    assert!(copied.is_file());
    assert!(!super::os::same_file(
        &fixture.ollama.blob_path(&fixture.qwen3),
        &copied
    ));
    assert!(store.call(|c| records::builtin_shared(c)).await.unwrap());
    let listed = source(&store, data.path(), ImportOrigin::Builtin, &machine, false)
        .await
        .unwrap();
    assert!(listed.shared);
    assert!(!listed.same_drive);
    // Ménage : jamais, même la reprise faite.
    assert!(import_done(&store).await);
    assert_eq!(service::leftovers(&store, data.path()).await.unwrap(), None);
    assert!(service::clean(&import, &store, data.path()).await.is_err());
    assert_eq!(snapshot(home.path()), before);
    service::TEST_MACHINE_DIRS.with(|dirs| dirs.borrow_mut().clear());
}

/// Relecture L6, B2 : dossier réglé absent au lancement (disque pas
/// monté) : la reprise reste à faire, rien n'est proposé au ménage ; disque
/// monté, elle se fait au lancement suivant.
#[tokio::test]
async fn a_missing_builtin_folder_stays_pending() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let dir = data.path().join("unmounted").join("ollama");
    set_builtin_folder(&store, &dir).await;
    let import = service::ModelImport::new(|_| {});
    service::at_startup(&import, store.clone(), data.path().to_owned()).await;
    assert!(!import_done(&store).await);
    assert!(import.state().last.is_none());
    assert_eq!(service::leftovers(&store, data.path()).await.unwrap(), None);
    assert!(service::clean(&import, &store, data.path()).await.is_err());
    let _fixture = fixture(dir.clone());
    service::at_startup(&import, store.clone(), data.path().to_owned()).await;
    let last = import.state().last.unwrap();
    assert!(!last.imported.is_empty());
    assert!(import_done(&store).await);
    // Dossier propre à l'intégré : il devient celui des modèles.
    assert_eq!(
        store
            .call(|c| crate::store::local_models::models_dir(c))
            .await
            .unwrap(),
        Some(dir.to_string_lossy().into_owned())
    );
}

/// Relecture de l'intégration, M1 : un téléchargement du ModelHub en cours
/// pendant la reprise de l'intégré déplacé : son dossier n'est pas adopté
/// (le téléchargement finit dans le dossier actuel) ; sans téléchargement,
/// il l'est.
#[tokio::test]
async fn a_moved_builtin_folder_is_not_adopted_during_a_download() {
    for downloading in [true, false] {
        let data = tempfile::tempdir().unwrap();
        let store = database_0_9_5(data.path());
        let dir = data.path().join("big").join("ollama");
        let _fixture = fixture(dir.clone());
        set_builtin_folder(&store, &dir).await;
        let report = run(
            builtin_job(&dir, None),
            Context {
                downloads_active: Arc::new(move || downloading),
                ..context(&store, data.path(), "http://127.0.0.1:9")
            },
            |_| {},
        )
        .await;
        assert_eq!(report.error, None);
        let adopted = store
            .call(|c| crate::store::local_models::models_dir(c))
            .await
            .unwrap();
        let expected = (!downloading).then(|| dir.to_string_lossy().into_owned());
        assert_eq!(adopted, expected, "downloading: {downloading}");
    }
}

/// Relecture L6, I1 : une reprise de l'intégré limitée à un nom ne le
/// marque pas repris ; le ménage n'efface pas les autres modèles prêts. Le
/// ménage nomme ensuite les modèles qui disparaîtront (M1).
#[tokio::test]
async fn a_partial_builtin_import_is_not_done() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let fixture = fixture(data.path().join("ollama").join("models"));
    let import = service::ModelImport::new(|_| {});
    let report = run(
        builtin_job(&fixture.ollama.dir, Some(vec!["qwen3:0.6b"])),
        context(&store, data.path(), "http://127.0.0.1:9"),
        |_| {},
    )
    .await;
    assert_eq!(report.failed, []);
    assert_eq!(report.imported.len(), 2);
    assert!(!import_done(&store).await);
    assert_eq!(service::leftovers(&store, data.path()).await.unwrap(), None);
    assert!(service::clean(&import, &store, data.path()).await.is_err());
    let report = run(
        builtin_job(&fixture.ollama.dir, None),
        context(&store, data.path(), "http://127.0.0.1:9"),
        |_| {},
    )
    .await;
    assert_eq!(report.failed, []);
    assert!(import_done(&store).await);
    let left = service::leftovers(&store, data.path())
        .await
        .unwrap()
        .unwrap();
    let mut gone: Vec<(&str, CandidateStatus)> = left
        .models
        .iter()
        .map(|model| (model.name.as_str(), model.status))
        .collect();
    gone.sort_by_key(|(name, _)| *name);
    assert_eq!(
        gone,
        [
            ("gemma3:4b", CandidateStatus::Redownload),
            ("gpt-oss:120b-cloud", CandidateStatus::Skipped),
            ("lost:1b", CandidateStatus::Skipped),
        ]
    );
}

/// Relecture L6, I2 : `mybot` créé par `ollama create` sur les poids de
/// qwen3 avec sa consigne et ses réglages reste un modèle à part.
#[tokio::test]
async fn a_custom_model_on_the_same_weights_stays_its_own() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let machine = tempfile::tempdir().unwrap();
    let ollama = Ollama::new(machine.path().join("models"));
    let weights_layer = ollama.blob(MODEL_TYPE, &weights("qwen3", &[], 7));
    let system = ollama.blob(SYSTEM_TYPE, b"You are a pirate.");
    let params = ollama.blob(PARAMS_TYPE, br#"{"temperature":1.2}"#);
    ollama.manifest(
        "registry.ollama.ai/library/qwen3/0.6b",
        vec![weights_layer.clone()],
        serde_json::json!({}),
    );
    ollama.manifest(
        "registry.ollama.ai/library/mybot/v1",
        vec![weights_layer, system, params],
        serde_json::json!({}),
    );
    let listed = source(
        &store,
        data.path(),
        ImportOrigin::Machine,
        &ollama.dir,
        false,
    )
    .await
    .unwrap();
    assert_eq!(listed.models.len(), 2);
    assert!(listed.models.iter().all(|model| model.aliases.is_empty()));
    let report = run(
        Job {
            origin: ImportOrigin::Machine,
            dir: ollama.dir.clone(),
            names: None,
            shared: false,
            retry_failed: false,
        },
        context(&store, data.path(), "http://127.0.0.1:9"),
        |_| {},
    )
    .await;
    assert_eq!(report.failed, []);
    let ids: HashSet<&str> = report.imported.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["@ollama/qwen3:0.6b", "@ollama/mybot:v1"].into());
    let mybot = store
        .call(|c| records::defaults(c, "@ollama/mybot:v1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mybot.system, "You are a pirate.");
    assert_eq!(mybot.params.temperature, Some(1.2));
    assert_eq!(
        store
            .call(|c| records::defaults(c, "@ollama/qwen3:0.6b"))
            .await
            .unwrap(),
        None
    );
}

/// Relecture L6, I3 : `hf.co/u/R-GGUF` (étiquette `latest`, en Q4_K_M) et
/// `hf.co/u/R-GGUF:Q4_K_M` aux poids différents prétendent au même
/// identifiant et au même fichier : aucun n'écrase l'autre (identifiant de
/// repli pour le second) ; un troisième contenu venu de la machine non plus.
#[tokio::test]
async fn two_names_for_one_id_never_overwrite_each_other() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let ollama = Ollama::new(data.path().join("ollama").join("models"));
    let current = weights("llama", &[], 8);
    let old = weights("llama", &[], 9);
    let current_layer = ollama.blob(MODEL_TYPE, &current);
    let old_layer = ollama.blob(MODEL_TYPE, &old);
    ollama.manifest(
        "hf.co/u/R-GGUF/latest",
        vec![current_layer.clone()],
        serde_json::json!({}),
    );
    ollama.manifest(
        "hf.co/u/R-GGUF/Q4_K_M",
        vec![old_layer.clone()],
        serde_json::json!({}),
    );
    let report = run(
        builtin_job(&ollama.dir, None),
        context(&store, data.path(), "http://127.0.0.1:9"),
        |_| {},
    )
    .await;
    assert_eq!(report.failed, []);
    let mut ids: Vec<(String, String)> = report
        .imported
        .iter()
        .map(|m| (m.name.clone(), m.id.clone()))
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            ("hf.co/u/R-GGUF:Q4_K_M".into(), "u/R-GGUF:Q4_K_M".into()),
            (
                "hf.co/u/R-GGUF:latest".into(),
                "@ollama/hf.co/u/R-GGUF:latest".into()
            ),
        ]
    );
    let content = |id: &'static str| {
        let store = store.clone();
        let data = data.path().to_owned();
        async move {
            let model = crate::local_models::list(&store, &data)
                .await
                .unwrap()
                .into_iter()
                .find(|model| model.id == id)
                .unwrap();
            fs::read(&model.path).unwrap()
        }
    };
    assert_eq!(content("u/R-GGUF:Q4_K_M").await, old);
    assert_eq!(content("@ollama/hf.co/u/R-GGUF:latest").await, current);
    assert!(ollama.blob_path(&current_layer).is_file());
    assert!(ollama.blob_path(&old_layer).is_file());
    // Un autre contenu sous le même nom, depuis la machine : à part aussi.
    let machine = tempfile::tempdir().unwrap();
    let other = Ollama::new(machine.path().join("models"));
    let third = weights("llama", &[], 10);
    other.manifest(
        "hf.co/u/R-GGUF/Q4_K_M",
        vec![other.blob(MODEL_TYPE, &third)],
        serde_json::json!({}),
    );
    let report = run(
        Job {
            origin: ImportOrigin::Machine,
            dir: other.dir.clone(),
            names: None,
            shared: false,
            retry_failed: false,
        },
        context(&store, data.path(), "http://127.0.0.1:9"),
        |_| {},
    )
    .await;
    assert_eq!(report.failed, []);
    assert_eq!(report.imported[0].id, "@ollama/hf.co/u/R-GGUF:Q4_K_M");
    assert_eq!(content("u/R-GGUF:Q4_K_M").await, old);
    assert_eq!(content("@ollama/hf.co/u/R-GGUF:Q4_K_M").await, third);
}

/// Relecture L6, I4 : coupure (l'app tuée) entre deux modèles : le premier
/// est en base avec la correspondance de ses noms ; au relancement tout est
/// repris et les réglages réécrits.
#[tokio::test]
async fn a_crash_between_two_models_keeps_the_first_one_named() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let fixture = fixture(data.path().join("ollama").join("models"));
    let crashed = tokio::spawn(run(
        builtin_job(
            &fixture.ollama.dir,
            Some(vec!["qwen3.5:0.8b", "qwen3:0.6b"]),
        ),
        context(&store, data.path(), "http://127.0.0.1:9"),
        |progress| {
            if progress.model.as_deref() == Some("qwen3:0.6b") {
                panic!("coupure simulée");
            }
        },
    ))
    .await;
    assert!(crashed.is_err());
    let named = store
        .call(|c| records::renamed(c, "qwen3.5:0.8b"))
        .await
        .unwrap();
    assert_eq!(named.as_deref(), Some("@ollama/qwen3.5:0.8b"));
    let report = run(
        builtin_job(&fixture.ollama.dir, None),
        context(&store, data.path(), "http://127.0.0.1:9"),
        |_| {},
    )
    .await;
    assert_eq!(report.failed, []);
    let selected: String = store
        .call(|c| Ok(c.query_row("SELECT selected_model FROM settings", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(selected, "@ollama/qwen3:0.6b");
    assert!(import_done(&store).await);
}

/// Relecture L6, I5 : deux modèles au même projecteur ; le premier repris
/// seul ne prive pas le second de son projecteur.
#[tokio::test]
async fn a_shared_projector_stays_for_the_model_not_taken() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let ollama = Ollama::new(data.path().join("ollama").join("models"));
    let projector_layer = ollama.blob(PROJECTOR_TYPE, &projector(11));
    for (name, salt) in [("visa", 12), ("visb", 13)] {
        let weights_layer = ollama.blob(MODEL_TYPE, &weights("qwen3", &[], salt));
        ollama.manifest(
            &format!("registry.ollama.ai/library/{name}/1b"),
            vec![weights_layer, projector_layer.clone()],
            serde_json::json!({}),
        );
    }
    let first = run(
        builtin_job(&ollama.dir, Some(vec!["visa:1b"])),
        context(&store, data.path(), "http://127.0.0.1:9"),
        |_| {},
    )
    .await;
    assert_eq!(first.failed, []);
    assert!(ollama.blob_path(&projector_layer).is_file());
    let listed = source(
        &store,
        data.path(),
        ImportOrigin::Builtin,
        &ollama.dir,
        false,
    )
    .await
    .unwrap();
    assert_eq!(by_name(&listed, "visb:1b").status, CandidateStatus::Ready);
    let second = run(
        builtin_job(&ollama.dir, Some(vec!["visb:1b"])),
        context(&store, data.path(), "http://127.0.0.1:9"),
        |_| {},
    )
    .await;
    assert_eq!(second.failed, []);
    assert_eq!(second.imported[0].id, "@ollama/visb:1b");
}

/// Relecture L6, I6 et M5-M7 : architecture que le llama-server livré ne
/// charge pas (llama3.2-vision « mllama »), modèles d'embeddings (par
/// l'architecture ou `pooling_type`), cloud d'après le blob de
/// configuration (forme réelle d'Ollama), gabarit absent signalé, FIFO à la
/// place d'un blob de réglages (jamais ouverte).
#[tokio::test]
async fn odd_models_are_judged_without_loading_them() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let machine = tempfile::tempdir().unwrap();
    let ollama = Ollama::new(machine.path().join("models"));
    let header = |arch: &str, extra: &[(&str, u32, Vec<u8>)], salt: u8| {
        let arch_text = super::compat::tests::text(arch);
        let mut pairs: Vec<(&str, u32, Vec<u8>)> = vec![("general.architecture", 8, arch_text)];
        pairs.extend(extra.iter().cloned());
        let mut bytes = super::compat::tests::gguf(&pairs, &[("blk.0.attn_q.weight", 2)]);
        bytes.extend(std::iter::repeat_n(salt, 1024));
        bytes
    };
    for (path, bytes) in [
        (
            "registry.ollama.ai/library/llama3.2-vision/11b",
            header("mllama", &[], 1),
        ),
        (
            "registry.ollama.ai/library/nomic-embed-text/latest",
            header("nomic-bert", &[], 2),
        ),
        (
            "registry.ollama.ai/library/qwen3-embedding/0.6b",
            header(
                "qwen3",
                &[("qwen3.pooling_type", 4, 3u32.to_le_bytes().to_vec())],
                3,
            ),
        ),
        (
            "registry.ollama.ai/library/bare/1b",
            header(
                "qwen3",
                &[("general.file_type", 4, 15u32.to_le_bytes().to_vec())],
                4,
            ),
        ),
    ] {
        let layer = ollama.blob(MODEL_TYPE, &bytes);
        ollama.manifest(path, vec![layer], serde_json::json!({}));
    }
    // Cloud : `remote_host` dans le blob de configuration.
    let config = ollama.blob(
        "application/vnd.docker.container.image.v1+json",
        br#"{"model_format":"","remote_host":"https://ollama.com:443","remote_model":"gpt-oss:20b"}"#,
    );
    ollama.manifest(
        "registry.ollama.ai/library/gpt-oss/20b-cloud",
        vec![],
        serde_json::json!({ "config": config }),
    );
    #[cfg(unix)]
    {
        let fifo = ollama
            .dir
            .join("blobs")
            .join(format!("sha256-{}", "f".repeat(64)));
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        let layer = ollama.blob(MODEL_TYPE, &weights("qwen3", &[], 5));
        ollama.manifest(
            "registry.ollama.ai/library/fifo/1b",
            vec![
                layer,
                serde_json::json!({"mediaType": PARAMS_TYPE, "digest": format!("sha256:{}", "f".repeat(64)), "size": 10}),
            ],
            serde_json::json!({}),
        );
    }
    let listed = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        source(
            &store,
            data.path(),
            ImportOrigin::Machine,
            &ollama.dir,
            false,
        ),
    )
    .await
    .expect("the scan must not block on a FIFO")
    .unwrap();
    let judged = |name: &str| {
        let model = by_name(&listed, name);
        (model.status, model.problem)
    };
    assert_eq!(
        judged("llama3.2-vision:11b"),
        (CandidateStatus::Skipped, Some(ImportProblem::Unsupported))
    );
    assert_eq!(
        judged("nomic-embed-text:latest"),
        (CandidateStatus::Skipped, Some(ImportProblem::Embedding))
    );
    assert_eq!(
        judged("qwen3-embedding:0.6b"),
        (CandidateStatus::Skipped, Some(ImportProblem::Embedding))
    );
    assert_eq!(
        judged("gpt-oss:20b-cloud"),
        (CandidateStatus::Skipped, Some(ImportProblem::Cloud))
    );
    let bare = by_name(&listed, "bare:1b");
    assert_eq!(bare.status, CandidateStatus::Ready);
    assert!(bare.no_template);
    #[cfg(unix)]
    {
        let fifo = by_name(&listed, "fifo:1b");
        assert_eq!(fifo.status, CandidateStatus::Ready);
        assert!(!fifo.no_template);
    }
}

/// Relecture L6, I7 : pendant un changement du dossier des modèles, aucun
/// téléchargement du ModelHub ni aucune reprise ne commence ; un changement
/// refusé par l'appelant (téléchargement vu verrou pris) rend le verrou.
#[tokio::test]
async fn nothing_starts_while_the_models_folder_moves() {
    use crate::hub::downloads::{HubDownloads, Options};
    use crate::hub::types::HubDownloadRequest;
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let downloads = HubDownloads::with_options(
        |_| {},
        Options {
            hf_url: Some("http://127.0.0.1:9".into()),
            ..Options::default()
        },
    );
    downloads.attach(store.clone(), data.path().to_owned());
    let import = service::ModelImport::new(|_| {});
    let busy = folder::try_busy().unwrap();
    let refused = downloads.download(HubDownloadRequest {
        model: "u/R-GGUF:Q4_K_M".into(),
        repo: None,
        label: None,
    });
    assert!(matches!(refused, Err(AppError::Invalid(_))), "{refused:?}");
    assert!(downloads.list().is_empty());
    assert!(
        import
            .start(
                builtin_job(data.path(), None),
                store.clone(),
                data.path().to_owned(),
                async {}
            )
            .is_err()
    );
    drop(busy);
    let refused = folder::change(
        &store,
        data.path(),
        data.path().join("elsewhere"),
        true,
        Arc::new(AtomicBool::new(false)),
        || Err(AppError::Invalid("download running".into())),
        |_| {},
    )
    .await;
    assert!(refused.is_err());
    assert!(!folder::is_busy());
    assert!(!data.path().join("elsewhere").exists());
}

/// Relecture L6, I8 et M8 : un modèle qui échoue deux fois de suite n'est
/// plus repris au lancement et ne bloque plus le ménage, qui le nomme ;
/// l'écran peut le retenter. Texte de l'échec sans enrobage vide.
#[tokio::test]
async fn a_model_failing_twice_is_given_up_and_listed() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let fixture = fixture(data.path().join("ollama").join("models"));
    // Fichier étranger à la place de qwen3 : jamais effacé, échec à chaque
    // fois.
    let target =
        crate::local_models::default_models_dir(data.path()).join("@ollama/qwen3/0.6b/model.gguf");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, b"someone else's file").unwrap();
    let import = service::ModelImport::new(|_| {});
    service::at_startup(&import, store.clone(), data.path().to_owned()).await;
    let last = import.state().last.unwrap();
    let failed: Vec<&str> = last.failed.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(failed, ["qwen3:0.6b", "qwen3:latest"]);
    assert_eq!(
        last.failed[0].error,
        crate::i18n::t!("modelImport.fileExists", file = target.to_string_lossy())
    );
    assert!(!import_done(&store).await);
    assert_eq!(service::leftovers(&store, data.path()).await.unwrap(), None);
    // Deuxième échec : abandonné, la reprise est faite.
    service::at_startup(&import, store.clone(), data.path().to_owned()).await;
    assert_eq!(import.state().last.unwrap().failed.len(), 2);
    assert!(import_done(&store).await);
    let left = service::leftovers(&store, data.path())
        .await
        .unwrap()
        .unwrap();
    assert!(
        left.models
            .iter()
            .any(|m| m.name == "qwen3:0.6b" && m.status == CandidateStatus::Failed)
    );
    let listed = source(
        &store,
        data.path(),
        ImportOrigin::Builtin,
        &fixture.ollama.dir,
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        by_name(&listed, "qwen3:0.6b").status,
        CandidateStatus::Failed
    );
    // Plus retenté au lancement.
    let report = run(
        builtin_job(&fixture.ollama.dir, None),
        context(&store, data.path(), "http://127.0.0.1:9"),
        |_| {},
    )
    .await;
    assert_eq!(report.failed, []);
    assert_eq!(fs::read(&target).unwrap(), b"someone else's file");
    // Retenté à la demande, une fois la place libre.
    fs::remove_file(&target).unwrap();
    let mut job = builtin_job(&fixture.ollama.dir, None);
    job.retry_failed = true;
    let report = run(
        job,
        context(&store, data.path(), "http://127.0.0.1:9"),
        |_| {},
    )
    .await;
    assert_eq!(report.failed, []);
    assert!(report.imported.iter().any(|m| m.id == "@ollama/qwen3:0.6b"));
}

/// Relecture L6, M2 et M3 : l'ancien dossier dans le nouveau est refusé ;
/// un déplacement coupé est défait ; un fichier étranger du dossier visé
/// arrête le déplacement sans être effacé, tout reste à sa place.
#[tokio::test]
async fn a_cut_or_refused_folder_move_puts_everything_back() {
    let data = tempfile::tempdir().unwrap();
    let store = database_0_9_5(data.path());
    let fixture = fixture(data.path().join("ollama").join("models"));
    run(
        builtin_job(&fixture.ollama.dir, None),
        context(&store, data.path(), "http://127.0.0.1:9"),
        |_| {},
    )
    .await;
    let cancel = Arc::new(AtomicBool::new(false));
    let contains = folder::change(
        &store,
        data.path(),
        data.path().to_owned(),
        true,
        cancel.clone(),
        || Ok(()),
        |_| {},
    )
    .await;
    assert!(contains.is_err());
    let old = crate::local_models::default_models_dir(data.path());
    let new = data.path().join("elsewhere");
    let stored = store
        .call(|c| crate::store::local_models::list(c))
        .await
        .unwrap();
    let relative = stored[0].files[0].clone();
    let all: Vec<String> = stored
        .iter()
        .flat_map(|model| model.all_files().map(str::to_owned).collect::<Vec<_>>())
        .collect();
    let present = |dir: &Path| all.iter().all(|file| dir.join(file).is_file());
    assert!(present(&old));
    // Coupure simulée : journal écrit, un fichier déjà passé dans le nouveau
    // dossier ; un autre déjà lié (les deux noms existent).
    fs::create_dir_all(new.join(&relative).parent().unwrap()).unwrap();
    fs::rename(old.join(&relative), new.join(&relative)).unwrap();
    let linked = stored[1].files[0].clone();
    fs::create_dir_all(new.join(&linked).parent().unwrap()).unwrap();
    fs::hard_link(old.join(&linked), new.join(&linked)).unwrap();
    let target = new.to_string_lossy().into_owned();
    store
        .call(move |c| records::set_models_dir_moving(c, Some(&target)))
        .await
        .unwrap();
    assert!(folder::recover(&store, data.path()).await.unwrap() >= 2);
    assert!(present(&old));
    assert!(!new.join(&relative).exists());
    assert!(!new.join(&linked).exists());
    assert_eq!(
        store.call(|c| records::models_dir_moving(c)).await.unwrap(),
        None
    );
    // Fichier étranger à un chemin du dossier visé : refus, rien d'effacé,
    // tout dans l'ancien dossier, journal effacé.
    fs::create_dir_all(new.join(&relative).parent().unwrap()).unwrap();
    fs::write(new.join(&relative), b"not yours").unwrap();
    let refused = folder::change(
        &store,
        data.path(),
        new.clone(),
        true,
        cancel,
        || Ok(()),
        |_| {},
    )
    .await;
    let Err(AppError::System(text)) = refused else {
        panic!("expected a refusal: {refused:?}");
    };
    assert!(
        text.contains(&*new.join(&relative).to_string_lossy()),
        "{text}"
    );
    assert_eq!(fs::read(new.join(&relative)).unwrap(), b"not yours");
    assert!(present(&old));
    assert_eq!(
        store
            .call(|c| crate::store::local_models::models_dir(c))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        store.call(|c| records::models_dir_moving(c)).await.unwrap(),
        None
    );
}

#[test]
fn ollama_params_are_read_leniently() {
    // Couche réelle de qwen3:0.6b.
    let params = OllamaParams::from_json(
        br#"{"repeat_penalty":1,"stop":["<|im_start|>","<|im_end|>"],"temperature":0.6,"top_k":20,"top_p":0.95}"#,
    )
    .unwrap();
    assert_eq!(params.repeat_penalty, Some(1.0));
    assert_eq!(params.top_p, Some(0.95));
    assert_eq!(params.stop.len(), 2);
    // Une valeur d'un autre type n'empêche pas les autres ; arrêt seul.
    let odd =
        OllamaParams::from_json(br#"{"temperature":"hot","top_k":40,"stop":"END","num_ctx":8192}"#)
            .unwrap();
    assert_eq!(odd.temperature, None);
    assert_eq!(odd.top_k, Some(40));
    assert_eq!(odd.stop, ["END"]);
    assert_eq!(odd.num_ctx, Some(8192));
    assert!(OllamaParams::from_json(b"[1]").is_none());
    assert!(OllamaParams::from_json(b"{}").unwrap().is_empty());
}

/// Fiche réelle de bartowski/SmolLM2-135M-Instruct-GGUF (relevée le 9 nov.) :
/// Q4_K_L et Q4_K_M y sont le même fichier ; `hf.co/…:Q4_K_M` d'Ollama
/// reste Q4_K_M (vérification réelle : il était repris en Q4_K_L).
#[test]
fn identical_hf_files_keep_the_version_ollama_named() {
    use crate::hub::files::{Listing, RemoteFile};
    let same = "2e8040ceae7815abe0dcb3540b9995eaa1fa0d2ca9e797d0a635ae4433c68c2d";
    let file = |path: &str, sha: &str| RemoteFile {
        path: path.into(),
        size: 105_454_432,
        sha256: Some(sha.into()),
    };
    let listing = Listing {
        id: None,
        sha: "09816acd5d99df7be770d85ea30822623dab342c".into(),
        gated: false,
        files: vec![
            file("SmolLM2-135M-Instruct-Q4_K_L.gguf", same),
            file("SmolLM2-135M-Instruct-Q4_K_M.gguf", same),
            file("SmolLM2-135M-Instruct-Q4_K_S.gguf", &"a".repeat(64)),
        ],
    };
    let repo = "bartowski/SmolLM2-135M-Instruct-GGUF";
    let wanted = ModelId::parse(&format!("{repo}:Q4_K_M")).unwrap();
    let pick = |quant: &str| hf_weights(&listing, repo, same, quant, &wanted);
    assert_eq!(
        pick("Q4_K_M"),
        Some(("SmolLM2-135M-Instruct-Q4_K_M.gguf".into(), wanted.clone()))
    );
    assert_eq!(
        pick("q4_k_l").map(|(path, _)| path),
        Some("SmolLM2-135M-Instruct-Q4_K_L.gguf".into())
    );
    // Aucune version de ce nom : la première aux mêmes octets.
    assert_eq!(
        pick("Q8_0"),
        Some((
            "SmolLM2-135M-Instruct-Q4_K_L.gguf".into(),
            ModelId::parse(&format!("{repo}:Q4_K_L")).unwrap()
        ))
    );
    assert_eq!(
        hf_weights(&listing, repo, &"b".repeat(64), "Q4_K_M", &wanted),
        None
    );
}

#[test]
fn windows_volumes_are_compared_as_text() {
    assert!(same_volume(r"C:\Users\a\AppData\Local\x", r"c:\models"));
    assert!(same_volume(r"\\?\D:\ollama", r"D:\chatonlm\models"));
    assert!(!same_volume(r"C:\a", r"D:\a"));
    assert!(same_volume(r"\\nas\share\a", r"\\NAS\Share\b"));
    assert!(!same_volume(r"\\nas\share\a", r"\\nas\other\b"));
    assert!(!same_volume("relative", "relative"));
}

/// Vérification réelle (hors `cargo test`, lecture seule) : dossier de
/// l'Ollama de test, `CHATONLM_L6_REAL_OLLAMA=<dossier>`. Rien n'y est
/// écrit ; affiche ce que l'import ferait de chaque modèle.
#[test]
#[ignore = "lecture d'un vrai dossier d'Ollama (CHATONLM_L6_REAL_OLLAMA)"]
fn real_ollama_folder_scan() {
    let Some(dir) = std::env::var_os("CHATONLM_L6_REAL_OLLAMA") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let before = snapshot(&dir.join("manifests"));
    for found in scan::scan(&dir).unwrap() {
        let aliases: Vec<String> = found
            .aliases
            .iter()
            .map(names::ManifestName::ollama_name)
            .collect();
        println!(
            "{:<48} {:?} list={} aliases={aliases:?} size={}",
            found.name.ollama_name(),
            found.verdict,
            found.from_list,
            found.size()
        );
    }
    assert_eq!(snapshot(&dir.join("manifests")), before);
}

/// Vérification réelle (hors `cargo test`) : copie d'un vrai modèle de
/// l'Ollama de test (`CHATONLM_L6_REAL_OLLAMA`, noms dans
/// `CHATONLM_L6_REAL_NAMES`, séparés par des virgules) vers un dossier
/// temporaire, en lecture seule chez lui.
#[tokio::test]
#[ignore = "copie d'un vrai modèle d'Ollama (CHATONLM_L6_REAL_OLLAMA)"]
async fn real_ollama_copy() {
    let (Some(dir), Some(names)) = (
        std::env::var_os("CHATONLM_L6_REAL_OLLAMA"),
        std::env::var("CHATONLM_L6_REAL_NAMES").ok(),
    ) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let data =
        tempfile::tempdir_in(std::env::var_os("CHATONLM_L6_TMP").unwrap_or_else(|| ".".into()))
            .unwrap();
    let store = Store::open(&data.path().join("chatonlm.sqlite")).unwrap();
    let report = run(
        Job {
            origin: ImportOrigin::Machine,
            dir,
            names: Some(names.split(',').map(str::to_owned).collect()),
            shared: false,
            retry_failed: false,
        },
        context(&store, data.path(), crate::hub::names::DEFAULT_HF_URL),
        |progress| println!("{progress:?}"),
    )
    .await;
    println!("{report:#?}");
    for model in crate::local_models::list(&store, data.path())
        .await
        .unwrap()
    {
        println!("{model:#?}");
    }
    assert!(report.failed.is_empty());
}
