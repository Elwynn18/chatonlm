//! Modèles inscrits au démarrage par les e2e (`CHATONLM_E2E_MODELS`, builds
//! de débogage ou `e2e` seulement, comme `CHATONLM_E2E_HARDWARE`) : un JSON
//! `[{id, files, mmproj?, capabilities?, contextMax?, paramsLabel?}]` dont
//! les fichiers sont déjà rangés sous le dossier des modèles (chemins
//! relatifs, comme la table `local_models`). Le moteur lance alors le faux
//! `llama-server` (`CHATONLM_LLAMA_SERVER_BIN`) sur ces fichiers, sans
//! téléchargement. Ce que l'essai donne gagne sur l'en-tête GGUF du
//! fichier ; à défaut, l'en-tête (écrit par `fake-llama gguf`) est lu comme
//! à la fin d'un téléchargement.

use std::path::Path;

use serde::Deserialize;

use super::id::ModelId;
use super::{ModelCapabilities, ModelSource};
use crate::error::AppResult;
use crate::hub::gguf::meta;
use crate::store::Store;
use crate::store::local_models::{self as table, StoredLocalModel};

/// Variable lue au démarrage.
pub const ENV: &str = "CHATONLM_E2E_MODELS";

/// Un modèle de la variable.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Seed {
    pub id: String,
    pub files: Vec<String>,
    #[serde(default)]
    pub mmproj: Option<String>,
    #[serde(default)]
    pub capabilities: Option<ModelCapabilities>,
    #[serde(default)]
    pub context_max: Option<u32>,
    #[serde(default)]
    pub params_label: Option<String>,
}

/// Lit la variable ; une erreur dit quel modèle est refusé et pourquoi
/// (identifiant mal formé, aucun fichier, chemin hors du dossier).
pub fn parse(value: &str) -> Result<Vec<Seed>, String> {
    let seeds: Vec<Seed> = serde_json::from_str(value).map_err(|error| error.to_string())?;
    for seed in &seeds {
        if ModelId::parse(&seed.id).is_none() {
            return Err(format!("{}: malformed id", seed.id));
        }
        if seed.files.is_empty() {
            return Err(format!("{}: no files", seed.id));
        }
        if let Some(bad) = seed
            .files
            .iter()
            .chain(&seed.mmproj)
            .find(|file| !crate::hub::files::safe_path(file))
        {
            return Err(format!("{}: unsafe path {bad}", seed.id));
        }
    }
    Ok(seeds)
}

/// Lignes de la table : le premier modèle de la liste est le plus récent
/// (`installed_at` décroissant à partir de `now`).
pub fn rows(seeds: &[Seed], models_dir: &Path, now: i64) -> Vec<StoredLocalModel> {
    let read =
        |file: &str| super::absolute(models_dir, file).and_then(|path| meta::read(&path).ok());
    seeds
        .iter()
        .zip(0i64..)
        .map(|(seed, rank)| {
            let id = ModelId::parse(&seed.id).expect("checked by parse");
            let header = read(&seed.files[0]).unwrap_or_default();
            let projector = seed.mmproj.as_deref().and_then(read);
            let size_bytes = seed
                .files
                .iter()
                .chain(&seed.mmproj)
                .filter_map(|file| super::absolute(models_dir, file))
                .filter_map(|path| std::fs::metadata(path).ok())
                .map(|metadata| metadata.len())
                .sum();
            let read_capabilities = ModelCapabilities {
                vision: projector.as_ref().is_some_and(meta::ModelMeta::sees),
                audio: projector.as_ref().is_some_and(meta::ModelMeta::hears),
                tools: header.tools,
                thinking: header.thinking,
                fim: header.fim,
                base: header.base,
            };
            let (source, repo) = match &id {
                ModelId::Hf { .. } => (ModelSource::Hf, id.repo_id().unwrap_or_default()),
                ModelId::Ollama { name, .. } => (ModelSource::Ollama, name.clone()),
            };
            StoredLocalModel {
                id: seed.id.clone(),
                source,
                repo,
                revision: None,
                quant: id.quant().to_owned(),
                files: seed.files.clone(),
                mmproj: seed.mmproj.clone(),
                size_bytes,
                sha256: Default::default(),
                context_max: seed
                    .context_max
                    .or_else(|| header.context_length.and_then(|n| u32::try_from(n).ok())),
                params_label: seed.params_label.clone().or(header.size_label.clone()),
                arch: header.architecture,
                file_type: header.file_type,
                capabilities: seed.capabilities.unwrap_or(read_capabilities),
                installed_at: now - rank,
            }
        })
        .collect()
}

/// Inscrit les modèles de la variable dans la base (rien sans variable) ;
/// une variable invalide est notée au journal et ignorée, comme
/// `CHATONLM_E2E_HARDWARE`. Appelé une fois au démarrage, avant que la page
/// ou le moteur ne lisent la base. Rend le nombre de modèles inscrits.
pub fn apply(store: &Store, data_dir: &Path) -> AppResult<usize> {
    let Ok(value) = std::env::var(ENV) else {
        return Ok(0);
    };
    let seeds = match parse(&value) {
        Ok(seeds) => seeds,
        Err(error) => {
            crate::app_log!("chatonlm: {ENV} invalide, ignoré : {error}");
            return Ok(0);
        }
    };
    let now = crate::store::now_ms();
    store.blocking(|connection| {
        let models_dir = super::models_dir_in(connection, data_dir)?;
        let rows = rows(&seeds, &models_dir, now);
        for row in &rows {
            table::save(connection, row)?;
        }
        Ok(rows.len())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variable_is_checked_before_anything_is_written() {
        let good = r#"[{"id": "a/B-GGUF:Q4_K_M", "files": ["a/B-GGUF/b.gguf"],
                        "mmproj": "a/B-GGUF/mmproj.gguf", "contextMax": 8192,
                        "capabilities": {"vision": true, "tools": true}}]"#;
        let seeds = parse(good).unwrap();
        assert_eq!(seeds[0].context_max, Some(8192));
        assert_eq!(
            seeds[0].capabilities,
            Some(ModelCapabilities {
                vision: true,
                tools: true,
                ..ModelCapabilities::default()
            })
        );
        assert!(
            parse(
                r#"[{"id": "@ollama/qwen3:latest", "files": ["@ollama/qwen3/latest/model.gguf"]}]"#
            )
            .is_ok()
        );
        for bad in [
            r#"[{"id": "no-quant", "files": ["x.gguf"]}]"#,
            r#"[{"id": "a/B:Q4", "files": []}]"#,
            r#"[{"id": "a/B:Q4", "files": ["../outside.gguf"]}]"#,
            r#"[{"id": "a/B:Q4", "files": ["/etc/passwd"]}]"#,
            r#"[{"id": "a/B:Q4", "files": ["a/b.gguf"], "mmproj": "../m.gguf"}]"#,
            r#"[{"id": "a/B:Q4", "files": ["a/b.gguf"], "unknown": 1}]"#,
            "not json",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn rows_read_the_header_unless_the_test_says_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a/B-GGUF/b.gguf");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        // En-tête lisible : architecture, contexte et taille annoncée.
        std::fs::write(
            &file,
            crate::llm::testing::gguf_header("qwen3", 40_960, "0.6B"),
        )
        .unwrap();
        let seeds = parse(
            r#"[{"id": "a/B-GGUF:Q4_K_M", "files": ["a/B-GGUF/b.gguf"]},
                {"id": "a/B-GGUF:Q8_0", "files": ["a/B-GGUF/b.gguf"], "contextMax": 2048,
                 "paramsLabel": "7B", "capabilities": {"fim": true}},
                {"id": "@ollama/old:latest", "files": ["@ollama/old/latest/model.gguf"]}]"#,
        )
        .unwrap();
        let rows = rows(&seeds, dir.path(), 1_000);
        let read = &rows[0];
        assert_eq!(read.source, ModelSource::Hf);
        assert_eq!(read.repo, "a/B-GGUF");
        assert_eq!(read.quant, "Q4_K_M");
        assert_eq!(read.arch.as_deref(), Some("qwen3"));
        assert_eq!(read.context_max, Some(40_960));
        assert_eq!(read.params_label.as_deref(), Some("0.6B"));
        assert_eq!(read.capabilities, ModelCapabilities::default());
        assert_eq!(read.size_bytes, std::fs::metadata(&file).unwrap().len());
        // Ce que l'essai donne gagne sur l'en-tête.
        let forced = &rows[1];
        assert_eq!(forced.context_max, Some(2048));
        assert_eq!(forced.params_label.as_deref(), Some("7B"));
        assert_eq!(
            forced.capabilities,
            ModelCapabilities {
                fim: true,
                ..ModelCapabilities::default()
            }
        );
        // Importé d'Ollama, fichier absent : ligne quand même, sans en-tête.
        let imported = &rows[2];
        assert_eq!(imported.source, ModelSource::Ollama);
        assert_eq!(imported.repo, "old");
        assert_eq!(imported.quant, "latest");
        assert_eq!(imported.size_bytes, 0);
        // Le premier de la liste est le plus récent.
        assert!(rows[0].installed_at > rows[1].installed_at);
        assert!(rows[1].installed_at > rows[2].installed_at);
    }

    /// Les fichiers de `fake-llama gguf` (e2e) se lisent comme un vrai
    /// téléchargement : capacités du gabarit, jetons FIM, modèle de base,
    /// projecteur.
    #[test]
    fn fake_llama_files_give_their_capabilities() {
        use fake_llama::cli::gguf;
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, flags: &[&str]| {
            let out = dir.path().join("a/M-GGUF").join(name);
            let mut argv = vec![out.to_string_lossy().into_owned()];
            argv.extend(flags.iter().map(|flag| (*flag).to_owned()));
            assert_eq!(gguf::main(&argv), 0);
            format!("a/M-GGUF/{name}")
        };
        let chat = write("chat.gguf", &["--tools", "--think", "--ctx", "8192"]);
        let coder = write("coder.gguf", &["--fim", "--base", "--name", "Coder"]);
        let eyes = write("mmproj.gguf", &["--vision"]);
        let ears = write("mmproj-audio.gguf", &["--audio"]);
        let seeds = vec![
            Seed {
                id: "a/M-GGUF:CHAT".into(),
                files: vec![chat],
                mmproj: Some(eyes),
                capabilities: None,
                context_max: None,
                params_label: None,
            },
            Seed {
                id: "a/M-GGUF:CODER".into(),
                files: vec![coder],
                mmproj: Some(ears),
                capabilities: None,
                context_max: None,
                params_label: None,
            },
        ];
        let rows = rows(&seeds, dir.path(), 1_000);
        assert_eq!(
            rows[0].capabilities,
            ModelCapabilities {
                vision: true,
                tools: true,
                thinking: true,
                ..ModelCapabilities::default()
            }
        );
        assert_eq!(rows[0].context_max, Some(8192));
        assert_eq!(rows[0].arch.as_deref(), Some("qwen3"));
        assert_eq!(
            rows[1].capabilities,
            ModelCapabilities {
                audio: true,
                fim: true,
                base: true,
                ..ModelCapabilities::default()
            }
        );
    }

    #[test]
    fn the_variable_fills_the_table_at_startup() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("chatonlm.sqlite")).unwrap();
        // SAFETY: essai seul à lire cette variable (nom propre aux e2e).
        unsafe {
            std::env::set_var(
                ENV,
                r#"[{"id": "a/B-GGUF:Q4_K_M", "files": ["a/B-GGUF/b.gguf"]},
                    {"id": "c/D-GGUF:Q8_0", "files": ["c/D-GGUF/d.gguf"]}]"#,
            );
        }
        let count = apply(&store, dir.path()).unwrap();
        unsafe { std::env::set_var(ENV, "[{\"id\": \"broken\"}]") };
        let ignored = apply(&store, dir.path()).unwrap();
        unsafe { std::env::remove_var(ENV) };
        assert_eq!((count, ignored), (2, 0));
        let ids: Vec<String> = store
            .blocking(|connection| table::list(connection))
            .unwrap()
            .into_iter()
            .map(|model| model.id)
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"a/B-GGUF:Q4_K_M".to_owned()));
        assert!(ids.contains(&"c/D-GGUF:Q8_0".to_owned()));
    }
}
