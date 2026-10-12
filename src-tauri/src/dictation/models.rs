//! Catalogue des modèles de transcription (section Transcription du
//! ModelHub). Téléchargés par ChatonLM lui-même depuis Hugging Face (Ollama
//! ne sait pas transcrire), à une révision épinglée, chaque fichier vérifié
//! par sa taille et son SHA-256.

use serde::Serialize;
use ts_rs::TS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum SpeechEngine {
    /// NVIDIA Parakeet (ONNX Runtime, transcribe-rs).
    Parakeet,
    /// OpenAI Whisper (whisper.cpp, transcribe-rs).
    Whisper,
}

/// Un fichier d'un modèle : chemin dans le dépôt (aussi son nom sur le
/// disque), taille et empreinte attendues.
#[derive(Clone, Copy, Debug)]
pub struct ModelFile {
    pub path: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// Modèle du catalogue.
#[derive(Clone, Copy, Debug)]
pub struct CatalogModel {
    /// Identifiant stable (réglage `speechModel`, dossier sur le disque).
    pub id: &'static str,
    pub name: &'static str,
    pub engine: SpeechEngine,
    pub description: &'static str,
    /// Langues, en clair (« 25 European languages »).
    pub languages: &'static str,
    pub license: &'static str,
    pub license_url: &'static str,
    /// Dépôt Hugging Face et révision épinglée.
    pub repo: &'static str,
    pub revision: &'static str,
    pub files: &'static [ModelFile],
    pub recommended: bool,
}

impl CatalogModel {
    pub fn size(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }

    /// Page du dépôt sur Hugging Face.
    pub fn source_url(&self) -> String {
        format!("https://huggingface.co/{}", self.repo)
    }
}

const PARAKEET_REPO: &str = "istupakov/parakeet-tdt-0.6b-v3-onnx";
const PARAKEET_REVISION: &str = "8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce";
const WHISPER_REPO: &str = "ggerganov/whisper.cpp";
const WHISPER_REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";
const WHISPER_LICENSE: &str = "MIT";
const WHISPER_LICENSE_URL: &str = "https://github.com/openai/whisper/blob/main/LICENSE";
const WHISPER_LANGUAGES: &str = "99 languages";

/// Le catalogue, dans l'ordre d'affichage. Tailles et empreintes relevées le
/// 6 octobre 2026 (API Hugging Face, fichiers LFS ; vocab.txt calculé).
pub const CATALOG: &[CatalogModel] = &[
    CatalogModel {
        id: "parakeet-tdt-0.6b-v3",
        name: "Parakeet TDT 0.6B v3",
        engine: SpeechEngine::Parakeet,
        description: "Fast and accurate, with punctuation. Detects the language automatically. Runs well without a GPU.",
        languages: "25 European languages",
        license: "CC BY 4.0",
        license_url: "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3",
        repo: PARAKEET_REPO,
        revision: PARAKEET_REVISION,
        files: &[
            ModelFile {
                path: "encoder-model.int8.onnx",
                size: 652_183_999,
                sha256: "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09",
            },
            ModelFile {
                path: "decoder_joint-model.int8.onnx",
                size: 18_202_004,
                sha256: "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70",
            },
            ModelFile {
                path: "nemo128.onnx",
                size: 139_764,
                sha256: "a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f",
            },
            ModelFile {
                path: "vocab.txt",
                size: 93_939,
                sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
            },
        ],
        recommended: true,
    },
    CatalogModel {
        id: "whisper-tiny",
        name: "Whisper Tiny",
        engine: SpeechEngine::Whisper,
        description: "Smallest and fastest Whisper model. Rough accuracy.",
        languages: WHISPER_LANGUAGES,
        license: WHISPER_LICENSE,
        license_url: WHISPER_LICENSE_URL,
        repo: WHISPER_REPO,
        revision: WHISPER_REVISION,
        files: &[ModelFile {
            path: "ggml-tiny.bin",
            size: 77_691_713,
            sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        }],
        recommended: false,
    },
    CatalogModel {
        id: "whisper-base",
        name: "Whisper Base",
        engine: SpeechEngine::Whisper,
        description: "Fast, for clear speech.",
        languages: WHISPER_LANGUAGES,
        license: WHISPER_LICENSE,
        license_url: WHISPER_LICENSE_URL,
        repo: WHISPER_REPO,
        revision: WHISPER_REVISION,
        files: &[ModelFile {
            path: "ggml-base.bin",
            size: 147_951_465,
            sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        }],
        recommended: false,
    },
    CatalogModel {
        id: "whisper-small",
        name: "Whisper Small",
        engine: SpeechEngine::Whisper,
        description: "Good balance of speed and accuracy.",
        languages: WHISPER_LANGUAGES,
        license: WHISPER_LICENSE,
        license_url: WHISPER_LICENSE_URL,
        repo: WHISPER_REPO,
        revision: WHISPER_REVISION,
        files: &[ModelFile {
            path: "ggml-small.bin",
            size: 487_601_967,
            sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        }],
        recommended: false,
    },
    CatalogModel {
        id: "whisper-large-v3-turbo",
        name: "Whisper Large v3 Turbo",
        engine: SpeechEngine::Whisper,
        description: "Most accurate Whisper model here. Slow without a GPU.",
        languages: WHISPER_LANGUAGES,
        license: WHISPER_LICENSE,
        license_url: WHISPER_LICENSE_URL,
        repo: WHISPER_REPO,
        revision: WHISPER_REVISION,
        files: &[ModelFile {
            path: "ggml-large-v3-turbo-q5_0.bin",
            size: 574_041_195,
            sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        }],
        recommended: false,
    },
];

/// Modèle du catalogue par identifiant.
pub fn find(id: &str) -> Option<&'static CatalogModel> {
    CATALOG.iter().find(|model| model.id == id)
}

/// Tous les fichiers de `model` présents sous `root` (dossier des modèles de
/// transcription), à la bonne taille ; empreintes non relues (vérifiées au
/// téléchargement).
pub fn installed(root: &std::path::Path, model: &CatalogModel) -> bool {
    let dir = root.join(model.id);
    model.files.iter().all(|file| {
        std::fs::metadata(dir.join(file.path))
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() == file.size)
    })
}

/// Modèle tel que la page le voit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeechModelInfo {
    pub id: String,
    pub name: String,
    pub engine: SpeechEngine,
    pub description: String,
    pub size_bytes: u64,
    pub languages: String,
    pub license: String,
    pub license_url: String,
    pub source_url: String,
    pub recommended: bool,
    /// Tous les fichiers présents, à la bonne taille.
    pub installed: bool,
    /// Instructions du processeur qui manquent à ce modèle (Whisper sans
    /// AVX2...), noms affichés ; vide : utilisable sur cette machine.
    pub missing_cpu_features: Vec<String>,
    /// ONNX Runtime indisponible (Parakeet seulement) : pourquoi ; `None` :
    /// chargé, ou inutile.
    pub runtime_problem: Option<super::onnx::RuntimeProblem>,
}

impl CatalogModel {
    /// Modèle tel que la page le voit ; charge ONNX Runtime pour Parakeet
    /// (bloquant la première fois).
    pub fn info(&self, installed: bool) -> SpeechModelInfo {
        let runtime = match self.engine {
            SpeechEngine::Parakeet => super::onnx::ensure().err(),
            SpeechEngine::Whisper => None,
        };
        self.info_on(installed, super::cpu::missing_for(self.engine), runtime)
    }

    /// [`Self::info`] sur un processeur auquel manquent `missing`, avec
    /// `runtime` pour ONNX Runtime.
    pub fn info_on(
        &self,
        installed: bool,
        missing: &[&str],
        runtime: Option<super::onnx::RuntimeProblem>,
    ) -> SpeechModelInfo {
        SpeechModelInfo {
            id: self.id.into(),
            name: self.name.into(),
            engine: self.engine,
            description: self.description.into(),
            size_bytes: self.size(),
            languages: self.languages.into(),
            license: self.license.into(),
            license_url: self.license_url.into(),
            source_url: self.source_url(),
            recommended: self.recommended,
            installed,
            missing_cpu_features: missing.iter().map(|name| (*name).to_owned()).collect(),
            runtime_problem: runtime,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        let mut ids: Vec<_> = CATALOG.iter().map(|model| model.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CATALOG.len());
        assert_eq!(CATALOG.iter().filter(|model| model.recommended).count(), 1);
        for model in CATALOG {
            // Identifiant utilisé comme nom de dossier : sans séparateur.
            assert!(
                model
                    .id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-.".contains(c))
            );
            assert!(!model.files.is_empty());
            for file in model.files {
                assert_eq!(file.sha256.len(), 64);
                assert!(!file.path.contains('/') && !file.path.starts_with('.'));
            }
        }
    }

    /// La page traduit description et langues par identifiant
    /// (`hub.speech.catalog` de hub.json, SpeechModels.tsx) : l'anglais du
    /// catalogue de la page est celui d'ici, pour chaque modèle.
    #[test]
    fn the_page_catalog_has_these_english_texts() {
        let hub: serde_json::Value =
            serde_json::from_str(include_str!("../../../src/locales/en/hub.json")).unwrap();
        let catalog = &hub["speech"]["catalog"];
        for model in CATALOG {
            let key: String = model
                .id
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            assert_eq!(
                catalog[&key]["description"].as_str(),
                Some(model.description),
                "{key}"
            );
            assert_eq!(
                catalog[&key]["languages"].as_str(),
                Some(model.languages),
                "{key}"
            );
        }
    }

    #[test]
    fn the_page_learns_which_models_this_processor_cannot_run() {
        let model = find("whisper-tiny").unwrap();
        assert!(
            model
                .info_on(true, &[], None)
                .missing_cpu_features
                .is_empty()
        );
        let info = model.info_on(false, &["AVX2", "FMA"], None);
        assert_eq!(info.missing_cpu_features, ["AVX2", "FMA"]);
        assert!(!info.installed);
        // Whisper n'utilise pas ONNX Runtime (rien n'est chargé ici).
        assert_eq!(model.info(false).runtime_problem, None);
        // Parakeet : jamais concerné par le processeur ; ONNX Runtime absent
        // rapporté tel quel.
        let parakeet = find("parakeet-tdt-0.6b-v3").unwrap();
        assert!(super::super::cpu::missing_for(parakeet.engine).is_empty());
        let problem = super::super::onnx::RuntimeProblem {
            kind: super::super::onnx::RuntimeProblemKind::Missing,
            detail: "/x/libonnxruntime.so.1".into(),
        };
        let info = parakeet.info_on(true, &[], Some(problem.clone()));
        assert_eq!(info.runtime_problem, Some(problem));
    }

    #[test]
    fn installed_means_every_file_at_its_size() {
        let root = tempfile::tempdir().unwrap();
        let model = find("whisper-tiny").unwrap();
        assert!(!installed(root.path(), model));
        let dir = root.path().join(model.id);
        std::fs::create_dir_all(&dir).unwrap();
        let file = std::fs::File::create(dir.join(model.files[0].path)).unwrap();
        file.set_len(model.files[0].size - 1).unwrap();
        assert!(!installed(root.path(), model));
        file.set_len(model.files[0].size).unwrap();
        assert!(installed(root.path(), model));
    }
}
