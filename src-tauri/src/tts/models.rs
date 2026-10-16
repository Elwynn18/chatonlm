//! Catalogue des voix de la lecture à voix haute : une entrée par langue
//! (modèle Pocket TTS 6 couches de Kyutai et une voix), rangée dans
//! `app_data_dir()/voice-models/<id>/`.
//!
//! Deux sources, à des versions épinglées :
//! - nos graphes ONNX (export du modèle de Kyutai, transformer quantifié en
//!   int8 ; scripts dans `scripts/tts/`), publiés comme fichiers d'une release
//!   GitHub dédiée de ChatonLM ;
//! - le tokenizer et l'état de la voix, pris tels quels dans le dépôt libre
//!   d'accès `kyutai/pocket-tts-without-voice-cloning` (une voix ne vaut que
//!   pour la révision des poids dont elle a été tirée : les deux sont épinglés
//!   ensemble).

use std::path::Path;

use serde::Serialize;
use ts_rs::TS;

use super::VoiceLanguage;
use crate::dictation::downloads::{DownloadItem, ItemFile, catalog_files};
use crate::dictation::models::ModelFile;
use crate::dictation::onnx::RuntimeProblem;

/// Dépôt et release GitHub de nos graphes ONNX.
const RELEASE_REPO: &str = "Elwynn18/chatonlm";
const RELEASE_TAG: &str = "tts-models-v1";
const GITHUB: &str = "https://github.com";
/// Dépôt Hugging Face de Kyutai (sans clonage de voix, libre d'accès).
const KYUTAI_REPO: &str = "kyutai/pocket-tts-without-voice-cloning";
const HUGGING_FACE: &str = "https://huggingface.co";

/// Origine d'un fichier.
#[derive(Clone, Copy, Debug)]
pub enum Source {
    /// Fichier `asset` de la release GitHub de nos graphes.
    Release { asset: &'static str },
    /// Fichier `path` du dépôt de Kyutai, à la révision `revision`.
    Kyutai {
        path: &'static str,
        revision: &'static str,
    },
}

/// Voix du catalogue.
#[derive(Clone, Copy, Debug)]
pub struct VoiceModel {
    /// Identifiant stable (dossier sur le disque) : `pocket-tts-<langue>`.
    pub id: &'static str,
    /// Nom affiché (anglais, comme les noms de modèles).
    pub name: &'static str,
    pub language: VoiceLanguage,
    /// Nom de la voix (estelle, alba, lola).
    pub voice: &'static str,
    /// Licence de la voix (le modèle est sous CC BY 4.0) et texte de la
    /// licence.
    pub voice_license: &'static str,
    pub voice_license_url: &'static str,
    /// Auteur de l'enregistrement d'où la voix est tirée (attribution) et
    /// page de cet enregistrement.
    pub voice_credit: &'static str,
    pub voice_source_url: &'static str,
    /// Fichiers, dans l'ordre de téléchargement ; `sources[i]` dit d'où vient
    /// `files[i]`.
    pub files: &'static [ModelFile],
    pub sources: &'static [Source],
}

/// Licence et page du modèle de Kyutai.
pub const MODEL_LICENSE: &str = "CC BY 4.0";
pub const MODEL_LICENSE_URL: &str =
    "https://huggingface.co/kyutai/pocket-tts-without-voice-cloning";

/// Révision des voix précalculées de Kyutai (pocket-tts 3.3.0,
/// `utils.get_predefined_voice`).
const VOICES_REVISION: &str = "4e1e0a3e611c51c0b4ed8174fc10f32a54644303";

/// Textes des licences des voix.
const CC0_URL: &str = "https://creativecommons.org/publicdomain/zero/1.0/legalcode";
const CC_BY_URL: &str = "https://creativecommons.org/licenses/by/4.0/legalcode";

/// Décodeur Mimi : mêmes poids pour toutes les langues, un seul fichier.
const MIMI: ModelFile = ModelFile {
    path: "mimi_decoder.onnx",
    size: 41_603_900,
    sha256: "9c9fea89889c26061179854abf1d28cb154ca481378c506775425ee749d0fb8f",
};

/// Fichiers d'une voix, dans l'ordre de téléchargement : nos graphes (Mimi,
/// FlowLM int8, conditionneur), puis le tokenizer et l'état de la voix de
/// Kyutai. Tailles et empreintes : `scripts/tts/assemble.py`.
const fn files(
    flow_sha256: &'static str,
    text_sha256: &'static str,
    tokenizer: (u64, &'static str),
    voice: (&'static str, u64, &'static str),
) -> [ModelFile; 5] {
    [
        MIMI,
        ModelFile {
            path: "flow_lm_step.int8.onnx",
            size: 115_693_467,
            sha256: flow_sha256,
        },
        ModelFile {
            path: "text_conditioner.onnx",
            size: 16_534_643,
            sha256: text_sha256,
        },
        ModelFile {
            path: "tokenizer.json",
            size: tokenizer.0,
            sha256: tokenizer.1,
        },
        ModelFile {
            path: voice.0,
            size: voice.1,
            sha256: voice.2,
        },
    ]
}

/// Sources des fichiers de `files`, dans le même ordre.
const fn sources(
    release_flow: &'static str,
    release_text: &'static str,
    tokenizer: (&'static str, &'static str),
    voice: &'static str,
) -> [Source; 5] {
    [
        Source::Release {
            asset: "mimi_decoder.onnx",
        },
        Source::Release {
            asset: release_flow,
        },
        Source::Release {
            asset: release_text,
        },
        Source::Kyutai {
            path: tokenizer.0,
            revision: tokenizer.1,
        },
        Source::Kyutai {
            path: voice,
            revision: VOICES_REVISION,
        },
    ]
}

const EN_FILES: [ModelFile; 5] = files(
    "89ae7a5ca0500a7abbf00332c22c5e266147bcabef281c2def0b2bdac4959f9f",
    "8c0d4f3feb7c97ab38ac6999a8eed216c31b661ace43997c11d1d11284880048",
    (
        245_020,
        "f498428e1eafee50492f7be13dc9bfafcfc12e508cd0eb1b01c92ecd5d8c6687",
    ),
    (
        "alba.safetensors",
        6_195_000,
        "d291428b416d6c36a1de7835e51dbe1e334b75e5af512bb18dd23a1047fe8f3b",
    ),
);
const EN_SOURCES: [Source; 5] = sources(
    "en-flow_lm_step.int8.onnx",
    "en-text_conditioner.onnx",
    (
        "languages/english/tokenizer.json",
        "00eac05ed3d16bdc3f6b5d598874019c34a89214",
    ),
    "languages/english/embeddings/alba.safetensors",
);

const ES_FILES: [ModelFile; 5] = files(
    "34712e6cbb6fee08eefb7ca44f05f836ce88f24b2c2079acbbad0f98c94f08e4",
    "95d1edfd7821b116a41c81d004c8e5a403e13a0c8cc020c485c17220c2809f01",
    (
        246_610,
        "9ade09bbdc64ab2b59952b90773fa0890b780ece4fd5aef0d36a00c6ad5f0531",
    ),
    (
        "lola.safetensors",
        5_949_240,
        "9f7bef177c7a80bb1f8d0a0db0a77ad3d23f75ee2d627ea6ef70e2d843352e26",
    ),
);
const ES_SOURCES: [Source; 5] = sources(
    "es-flow_lm_step.int8.onnx",
    "es-text_conditioner.onnx",
    (
        "languages/spanish/tokenizer.json",
        "4e1e0a3e611c51c0b4ed8174fc10f32a54644303",
    ),
    "languages/spanish/embeddings/lola.safetensors",
);

const FR_FILES: [ModelFile; 5] = files(
    "bfb1d5312cb1221c2b03424906dfe6b1437e1a170dd734f47f52bab73258739c",
    "5e0e363a0065e0d6baaed3bbd83c482ecae48c7e5b6625bf524ca466039775ae",
    (
        246_403,
        "6211acffe9cb95172f71f0051f18e26ea305b2b32ea00b331ee976551c70887f",
    ),
    (
        "estelle.safetensors",
        8_259_384,
        "f763800c6865a7c13e3368db7c24345d7ab22abd06c2ca40a3428d14c5357248",
    ),
);
const FR_SOURCES: [Source; 5] = sources(
    "fr-flow_lm_step.int8.onnx",
    "fr-text_conditioner.onnx",
    (
        "languages/french/tokenizer.json",
        "8843db76457a91db32077edf8dfcd1c0e3e755fd",
    ),
    "languages/french/embeddings/estelle.safetensors",
);

/// Le catalogue, dans l'ordre d'affichage (en, es, fr).
/// Graphes exportés de pocket-tts 3.3.0 (poids : fr `8843db76`, es
/// `4e1e0a3e`, en `e7205b6e`, config `english` = english_2026-09), FlowLM en int8 par canal
/// (transformer seul), Mimi et conditionneur en fp32 (rapport 57).
pub const CATALOG: &[VoiceModel] = &[
    VoiceModel {
        id: "pocket-tts-en",
        name: "Pocket TTS · English (Alba)",
        language: VoiceLanguage::En,
        voice: "alba",
        voice_license: "CC BY 4.0",
        voice_license_url: CC_BY_URL,
        voice_credit: "Alba MacKenna",
        voice_source_url: "https://huggingface.co/kyutai/tts-voices/tree/main/alba-mackenna",
        files: &EN_FILES,
        sources: &EN_SOURCES,
    },
    VoiceModel {
        id: "pocket-tts-es",
        name: "Pocket TTS · Spanish (Lola)",
        language: VoiceLanguage::Es,
        voice: "lola",
        // Voix tirée d'un enregistrement de Common Voice (CC0).
        voice_license: "CC0",
        voice_license_url: CC0_URL,
        voice_credit: "Mozilla Common Voice",
        voice_source_url: "https://commonvoice.mozilla.org/es/datasets",
        files: &ES_FILES,
        sources: &ES_SOURCES,
    },
    VoiceModel {
        id: "pocket-tts-fr",
        name: "Pocket TTS · French (Estelle)",
        language: VoiceLanguage::Fr,
        voice: "estelle",
        // Enregistrement de Kyutai pour Unmute
        // (`kyutai/tts-voices/unmute-prod-website`, CC0).
        voice_license: "CC0",
        voice_license_url: CC0_URL,
        voice_credit: "Kyutai",
        voice_source_url: "https://huggingface.co/kyutai/tts-voices/tree/main/unmute-prod-website",
        files: &FR_FILES,
        sources: &FR_SOURCES,
    },
];

pub fn find(id: &str) -> Option<&'static VoiceModel> {
    CATALOG.iter().find(|model| model.id == id)
}

/// La voix d'une langue.
pub fn for_language(language: VoiceLanguage) -> Option<&'static VoiceModel> {
    CATALOG.iter().find(|model| model.language == language)
}

/// Le catalogue, vu par la file de téléchargement.
pub fn find_item(id: &str) -> Option<std::sync::Arc<dyn DownloadItem>> {
    find(id).map(|model| std::sync::Arc::new(*model) as std::sync::Arc<dyn DownloadItem>)
}

/// Vrai si tous les fichiers de `model` sont présents dans `root`, à la bonne
/// taille.
pub fn installed(root: &Path, model: &VoiceModel) -> bool {
    let dir = root.join(model.id);
    model.files.iter().all(|file| {
        std::fs::metadata(dir.join(file.path))
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() == file.size)
    })
}

impl VoiceModel {
    /// Adresse de `file` ; `base` : adresse imposée par les essais, pour tous
    /// les fichiers (sinon l'hôte réel de chacun).
    pub fn url(&self, base: Option<&str>, file: &ModelFile) -> String {
        let index = self
            .files
            .iter()
            .position(|candidate| candidate.path == file.path)
            .expect("file of the catalog");
        match self.sources[index] {
            Source::Release { asset } => format!(
                "{}/{RELEASE_REPO}/releases/download/{RELEASE_TAG}/{asset}",
                base.unwrap_or(GITHUB)
            ),
            Source::Kyutai { path, revision } => format!(
                "{}/{KYUTAI_REPO}/resolve/{revision}/{path}",
                base.unwrap_or(HUGGING_FACE)
            ),
        }
    }
}

impl DownloadItem for VoiceModel {
    fn id(&self) -> &str {
        self.id
    }

    fn name(&self) -> &str {
        self.name
    }

    fn files(&self, base: Option<&str>) -> Vec<ItemFile> {
        catalog_files(self.id, self.files, |file| self.url(base, file))
    }
}

/// Voix telle que la page la voit (ModelHub, réglages).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceModelInfo {
    pub id: String,
    pub name: String,
    pub language: VoiceLanguage,
    pub voice: String,
    pub size_bytes: u64,
    pub license: String,
    pub license_url: String,
    pub voice_license: String,
    pub voice_license_url: String,
    pub voice_credit: String,
    pub voice_source_url: String,
    /// Tous les fichiers présents, à la bonne taille.
    pub installed: bool,
    /// ONNX Runtime indisponible : pourquoi ; `None` : chargé.
    pub runtime_problem: Option<RuntimeProblem>,
}

impl VoiceModel {
    pub fn size(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }

    pub fn info(&self, installed: bool, runtime: Option<RuntimeProblem>) -> VoiceModelInfo {
        VoiceModelInfo {
            id: self.id.into(),
            name: self.name.into(),
            language: self.language,
            voice: self.voice.into(),
            size_bytes: self.size(),
            license: MODEL_LICENSE.into(),
            license_url: MODEL_LICENSE_URL.into(),
            voice_license: self.voice_license.into(),
            voice_license_url: self.voice_license_url.into(),
            voice_credit: self.voice_credit.into(),
            voice_source_url: self.voice_source_url.into(),
            installed,
            runtime_problem: runtime,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_hex(text: &str, len: usize) -> bool {
        text.len() == len
            && text
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    }

    #[test]
    fn catalog_is_consistent() {
        // Une voix par langue, identifiant `pocket-tts-<langue>` (dossier).
        assert_eq!(CATALOG.len(), VoiceLanguage::ALL.len());
        for language in VoiceLanguage::ALL {
            let models: Vec<_> = CATALOG
                .iter()
                .filter(|model| model.language == language)
                .collect();
            assert_eq!(models.len(), 1, "{language:?}");
            assert_eq!(models[0].id, format!("pocket-tts-{}", language.code()));
            assert_eq!(for_language(language).unwrap().id, models[0].id);
            assert!(find(models[0].id).is_some());
        }
        // Ordre d'affichage : celui de `VoiceLanguage::ALL`.
        let order: Vec<_> = CATALOG.iter().map(|model| model.language).collect();
        assert_eq!(order, VoiceLanguage::ALL);
        for model in CATALOG {
            assert!(model.name.starts_with("Pocket TTS · "), "{}", model.name);
            assert!(
                model
                    .voice_license_url
                    .starts_with("https://creativecommons.org/")
            );
            assert!(model.voice_source_url.starts_with("https://"));
            assert!(!model.voice_credit.is_empty());
            assert_eq!(model.files.len(), model.sources.len(), "{}", model.id);
            for file in model.files {
                assert!(is_hex(file.sha256, 64), "{}: {}", model.id, file.path);
                assert!(file.size > 0);
                assert!(!file.path.contains('/') && !file.path.starts_with('.'));
            }
            let mut paths: Vec<_> = model.files.iter().map(|file| file.path).collect();
            paths.sort_unstable();
            paths.dedup();
            assert_eq!(paths.len(), model.files.len(), "{}", model.id);
        }
    }

    /// Chaque fichier vient de la bonne source : nos graphes de la release
    /// (préfixés par la langue, sauf Mimi commun), le reste chez Kyutai à une
    /// révision épinglée, au chemin de la langue.
    #[test]
    fn sources_match_the_files() {
        let folders = [
            (VoiceLanguage::En, "english"),
            (VoiceLanguage::Es, "spanish"),
            (VoiceLanguage::Fr, "french"),
        ];
        for model in CATALOG {
            let code = model.language.code();
            let folder = folders
                .iter()
                .find(|(language, _)| *language == model.language)
                .unwrap()
                .1;
            let voice = format!("{}.safetensors", model.voice);
            for (file, source) in model.files.iter().zip(model.sources) {
                match *source {
                    Source::Release { asset } => {
                        assert!(file.path.ends_with(".onnx"), "{}", file.path);
                        let expected = if file.path == "mimi_decoder.onnx" {
                            file.path.to_owned()
                        } else {
                            format!("{code}-{}", file.path)
                        };
                        assert_eq!(asset, expected);
                    }
                    Source::Kyutai { path, revision } => {
                        assert!(is_hex(revision, 40), "{revision}");
                        let expected = if file.path == "tokenizer.json" {
                            format!("languages/{folder}/tokenizer.json")
                        } else {
                            assert_eq!(file.path, voice);
                            // Voix : révision des voix de pocket-tts 3.3.0.
                            assert_eq!(revision, VOICES_REVISION);
                            format!("languages/{folder}/embeddings/{voice}")
                        };
                        assert_eq!(path, expected);
                    }
                }
            }
            // Fichiers attendus par le moteur.
            for name in super::super::engine::MODEL_FILES {
                assert!(
                    model.files.iter().any(|file| file.path == *name),
                    "{}: {name}",
                    model.id
                );
            }
            assert!(model.files.iter().any(|file| file.path == voice));
        }
        // Mimi : un seul fichier, identique pour toutes les langues.
        let mimi: Vec<_> = CATALOG
            .iter()
            .map(|model| {
                let file = model.files.iter().find(|f| f.path == "mimi_decoder.onnx");
                file.map(|f| (f.size, f.sha256)).unwrap()
            })
            .collect();
        assert!(mimi.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn urls_point_at_the_release_and_at_kyutai() {
        let model = for_language(VoiceLanguage::Fr).unwrap();
        let urls: Vec<_> = model
            .files
            .iter()
            .map(|file| model.url(None, file))
            .collect();
        assert_eq!(
            urls,
            [
                "https://github.com/Elwynn18/chatonlm/releases/download/tts-models-v1/mimi_decoder.onnx",
                "https://github.com/Elwynn18/chatonlm/releases/download/tts-models-v1/fr-flow_lm_step.int8.onnx",
                "https://github.com/Elwynn18/chatonlm/releases/download/tts-models-v1/fr-text_conditioner.onnx",
                "https://huggingface.co/kyutai/pocket-tts-without-voice-cloning/resolve/8843db76457a91db32077edf8dfcd1c0e3e755fd/languages/french/tokenizer.json",
                "https://huggingface.co/kyutai/pocket-tts-without-voice-cloning/resolve/4e1e0a3e611c51c0b4ed8174fc10f32a54644303/languages/french/embeddings/estelle.safetensors",
            ]
        );
        // Adresse imposée (essais, e2e) : même chemin sur l'autre hôte.
        let english = for_language(VoiceLanguage::En).unwrap();
        let tokenizer = english
            .files
            .iter()
            .find(|file| file.path == "tokenizer.json")
            .unwrap();
        assert_eq!(
            english.url(Some("http://127.0.0.1:9"), tokenizer),
            "http://127.0.0.1:9/kyutai/pocket-tts-without-voice-cloning/resolve/00eac05ed3d16bdc3f6b5d598874019c34a89214/languages/english/tokenizer.json"
        );
    }

    #[test]
    fn a_voice_is_installed_when_every_file_has_its_size() {
        let dir = tempfile::tempdir().unwrap();
        let model = for_language(VoiceLanguage::Es).unwrap();
        assert!(!installed(dir.path(), model));
        let folder = dir.path().join(model.id);
        std::fs::create_dir_all(&folder).unwrap();
        for file in model.files {
            let handle = std::fs::File::create(folder.join(file.path)).unwrap();
            handle.set_len(file.size).unwrap();
        }
        assert!(installed(dir.path(), model));
        let last = model.files.last().unwrap();
        let handle = std::fs::File::create(folder.join(last.path)).unwrap();
        handle.set_len(last.size - 1).unwrap();
        assert!(!installed(dir.path(), model));
        assert_eq!(
            model.size(),
            model.files.iter().map(|file| file.size).sum::<u64>()
        );
    }
}
