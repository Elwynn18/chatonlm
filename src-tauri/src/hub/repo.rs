//! Dépôt de Hugging Face normalisé (35 R2) : ce que le catalogue garde d'une
//! réponse de `/api/models` ou `/api/models/<id>`, écrit tel quel dans le
//! cache. Le gabarit de conversation n'est jamais gardé : seulement les
//! capacités qu'on en déduit. Lecture tolérante : un champ absent ou d'un
//! type inattendu est ignoré, une entrée sans identifiant est écartée.

use std::fmt;

use serde::de::{Deserializer as _, IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::capabilities::{self, Facts};
use super::quant::{self, FileEntry};
use super::types::HubCapabilities;

/// Fichier GGUF d'un dépôt (taille connue dans la fiche seulement).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoFile {
    pub name: String,
    pub size: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    /// « auteur/dépôt ».
    pub id: String,
    pub author: String,
    pub downloads: u64,
    pub likes: u64,
    pub trending: f64,
    pub created_at: Option<String>,
    pub last_modified: Option<String>,
    pub pipeline_tag: Option<String>,
    pub gated: bool,
    pub private: bool,
    pub architecture: Option<String>,
    pub context_length: Option<u32>,
    /// `gguf.total` : nombre de paramètres annoncé.
    pub total: Option<u64>,
    pub base_model: Option<String>,
    pub base_relation: Option<String>,
    pub license: Option<String>,
    pub languages: Vec<String>,
    pub capabilities: HubCapabilities,
    /// Fichiers `.gguf` seulement.
    pub files: Vec<RepoFile>,
    pub sha: Option<String>,
}

impl Repo {
    /// Nom du dépôt, sans l'auteur.
    pub fn repo_name(&self) -> &str {
        self.id.split_once('/').map_or(&self.id, |(_, name)| name)
    }

    /// Fichiers, pour l'inventaire des versions (taille inconnue : 0).
    pub fn file_entries(&self) -> Vec<FileEntry<'_>> {
        self.files
            .iter()
            .map(|file| FileEntry {
                path: &file.name,
                size: file.size.unwrap_or(0),
            })
            .collect()
    }
}

fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Nombre positif, entier ou non.
fn count(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_f64().filter(|n| *n >= 0.0).map(|n| n as u64))
}

/// Éléments lus au plus d'une liste (Hugging Face en renvoie 1000 au plus) ;
/// les suivants sont parcourus sans rien garder.
pub const LIST_ITEMS_MAX: usize = 1000;

/// Liste de `/api/models` : un tableau JSON, sinon `None`. Lu élément par
/// élément (jamais tout le tableau en `Value`) : une réponse hostile faite de
/// millions de petites valeurs ne coûte pas un gigaoctet de mémoire.
pub fn list_from_json(bytes: &[u8]) -> Option<Vec<Repo>> {
    struct Repos;

    impl<'de> Visitor<'de> for Repos {
        type Value = Vec<Repo>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a list of repositories")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<Repo>, A::Error> {
            let mut repos = Vec::new();
            for _ in 0..LIST_ITEMS_MAX {
                let Some(value) = seq.next_element::<Value>()? else {
                    return Ok(repos);
                };
                repos.extend(from_json(&value));
            }
            while seq.next_element::<IgnoredAny>()?.is_some() {}
            Ok(repos)
        }
    }

    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let repos = (&mut deserializer).deserialize_seq(Repos).ok()?;
    deserializer.end().ok()?;
    Some(repos)
}

/// Un dépôt de la liste ou de la fiche.
pub fn from_json(value: &Value) -> Option<Repo> {
    let id = text(&value["id"])?;
    let (id_author, name) = id.split_once('/')?;
    if id_author.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    let author = text(&value["author"]).unwrap_or_else(|| id_author.to_owned());
    let tags: Vec<String> = value["tags"]
        .as_array()
        .map(|tags| tags.iter().filter_map(text).collect())
        .unwrap_or_default();
    let gguf = &value["gguf"];
    let (base_model, base_relation) = base(value, &tags);
    let files: Vec<RepoFile> = value["siblings"]
        .as_array()
        .map(|siblings| {
            siblings
                .iter()
                .filter_map(|sibling| {
                    let name = text(&sibling["rfilename"])?;
                    quant::is_gguf(&name).then(|| RepoFile {
                        size: count(&sibling["size"]).or_else(|| count(&sibling["lfs"]["size"])),
                        name,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let projector = files.iter().any(|file| quant::is_projector(&file.name));
    let capabilities = capabilities::capabilities(&Facts {
        template: gguf["chat_template"].as_str(),
        id: &id,
        base_model: base_model.as_deref(),
        tags: &tags,
        projector,
    });
    let card = &value["cardData"];
    let license = tags
        .iter()
        .find_map(|tag| tag.strip_prefix("license:"))
        .map(str::to_owned)
        .or_else(|| text(&card["license"]))
        .map(
            |license| match (license.as_str(), text(&card["license_name"])) {
                ("other", Some(name)) => name,
                _ => license,
            },
        );
    Some(Repo {
        author,
        downloads: count(&value["downloads"]).unwrap_or(0),
        likes: count(&value["likes"]).unwrap_or(0),
        trending: value["trendingScore"].as_f64().unwrap_or(0.0),
        created_at: text(&value["createdAt"]),
        last_modified: text(&value["lastModified"]),
        pipeline_tag: text(&value["pipeline_tag"]),
        // `false`, ou « auto » / « manual ».
        gated: match &value["gated"] {
            Value::Bool(gated) => *gated,
            Value::String(mode) => !mode.is_empty() && mode != "false",
            _ => false,
        },
        private: value["private"].as_bool().unwrap_or(false),
        architecture: text(&gguf["architecture"]),
        context_length: count(&gguf["context_length"])
            .map(|length| u32::try_from(length).unwrap_or(u32::MAX)),
        total: count(&gguf["total"]).filter(|&total| total > 0),
        languages: languages(&tags, &card["language"]),
        base_model,
        base_relation,
        license,
        capabilities,
        files,
        sha: text(&value["sha"]),
        id,
    })
}

/// Modèle de base : `baseModels`, sinon les tags `base_model:[relation:]<id>`
/// (la quantification d'abord).
fn base(value: &Value, tags: &[String]) -> (Option<String>, Option<String>) {
    let declared = &value["baseModels"];
    if let Some(id) = declared["models"]
        .as_array()
        .and_then(|models| models.first())
        .and_then(|model| text(&model["id"]))
    {
        return (Some(id), text(&declared["relation"]));
    }
    let from_tags: Vec<(Option<&str>, &str)> = tags
        .iter()
        .filter_map(|tag| tag.strip_prefix("base_model:"))
        .map(|rest| match rest.split_once(':') {
            Some((relation, id)) => (Some(relation), id),
            None => (None, rest),
        })
        .filter(|(_, id)| id.contains('/'))
        .collect();
    let chosen = from_tags
        .iter()
        .find(|(relation, _)| *relation == Some("quantized"))
        .or_else(|| from_tags.first());
    match chosen {
        Some((relation, id)) => (Some((*id).to_owned()), relation.map(str::to_owned)),
        None => (None, None),
    }
}

/// Codes de langue : tags de deux lettres, et `cardData.language` (fiche).
fn languages(tags: &[String], declared: &Value) -> Vec<String> {
    let code = |s: &str| {
        let s = s.trim().to_ascii_lowercase();
        ((2..=3).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase())).then_some(s)
    };
    let mut found: Vec<String> = Vec::new();
    let mut push = |code: String| {
        if !found.contains(&code) {
            found.push(code);
        }
    };
    match declared {
        Value::String(language) => code(language).into_iter().for_each(&mut push),
        Value::Array(list) => list
            .iter()
            .filter_map(Value::as_str)
            .filter_map(code)
            .for_each(&mut push),
        _ => {}
    }
    tags.iter()
        .filter(|tag| tag.len() == 2)
        .filter_map(|tag| code(tag))
        .for_each(&mut push);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const DETAIL: &str =
        include_str!("../../crates/fake-ollama/fixtures/hf/detail-unsloth-qwen3.5-9b.json");
    const LIST: &str = include_str!("../../crates/fake-ollama/fixtures/hf/list-sample.json");

    #[test]
    fn a_real_detail_is_normalized() {
        let value: Value = serde_json::from_str(DETAIL).unwrap();
        let repo = from_json(&value).unwrap();
        assert_eq!(repo.id, "unsloth/Qwen3.5-9B-GGUF");
        assert_eq!(repo.author, "unsloth");
        assert_eq!(repo.base_model.as_deref(), Some("Qwen/Qwen3.5-9B"));
        assert_eq!(repo.base_relation.as_deref(), Some("quantized"));
        assert_eq!(repo.architecture.as_deref(), Some("qwen35"));
        assert_eq!(repo.context_length, Some(262144));
        assert_eq!(repo.total, Some(8953803264));
        assert_eq!(repo.license.as_deref(), Some("apache-2.0"));
        assert_eq!(repo.pipeline_tag.as_deref(), Some("image-text-to-text"));
        assert!(!repo.gated);
        assert!(repo.capabilities.vision);
        assert!(repo.capabilities.tools);
        assert!(repo.capabilities.thinking);
        assert!(!repo.capabilities.code);
        // Seulement les .gguf, avec leur taille.
        assert!(repo.files.iter().all(|f| f.name.ends_with(".gguf")));
        assert!(repo.files.iter().all(|f| f.size.is_some()));
        assert!(repo.files.iter().any(|f| f.name == "mmproj-F16.gguf"));
        // Le gabarit n'est jamais gardé.
        let stored = serde_json::to_string(&repo).unwrap();
        assert!(!stored.contains("im_start"), "{stored}");
    }

    #[test]
    fn a_real_list_is_normalized() {
        let repos = list_from_json(LIST.as_bytes()).unwrap();
        assert_eq!(repos.len(), 3);
        let coder = &repos[0];
        assert_eq!(coder.id, "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF");
        assert!(coder.capabilities.code && coder.capabilities.tools);
        assert_eq!(coder.downloads, 7361953);
        assert_eq!(coder.trending, 21.0);
        // Pas de baseModels dans cette liste : les tags.
        assert_eq!(
            coder.base_model.as_deref(),
            Some("Qwen/Qwen3-Coder-30B-A3B-Instruct")
        );
        assert_eq!(coder.base_relation.as_deref(), Some("quantized"));
        // Liste sans tailles.
        assert!(coder.files.iter().all(|f| f.size.is_none()));
        assert_eq!(repos[2].license.as_deref(), Some("mit"));
        assert_eq!(repos[2].base_model, None);
        assert_eq!(list_from_json(b"{\"error\":\"x\"}"), None);
        assert_eq!(list_from_json(b"<html>"), None);
        assert_eq!(list_from_json(b"[{\"id\":\"a/b\"}] x"), None);
    }

    /// Liste hostile : 1000 éléments gardés au plus, le reste parcouru sans
    /// rien garder ; des valeurs qui ne sont pas des dépôts sont écartées.
    #[test]
    fn a_huge_list_keeps_a_bounded_number_of_repositories() {
        let items: Vec<String> = (0..LIST_ITEMS_MAX + 500)
            .map(|index| format!("{{\"id\":\"a/r{index}\"}}"))
            .collect();
        let list = format!("[{}]", items.join(","));
        let repos = list_from_json(list.as_bytes()).unwrap();
        assert_eq!(repos.len(), LIST_ITEMS_MAX);
        assert_eq!(
            repos.last().unwrap().id,
            format!("a/r{}", LIST_ITEMS_MAX - 1)
        );
        let zeros = format!("[{}0]", "0,".repeat(100_000));
        assert_eq!(list_from_json(zeros.as_bytes()), Some(Vec::new()));
    }

    #[test]
    fn odd_values_are_tolerated() {
        let value = serde_json::json!({
            "id": "x/y-GGUF",
            "downloads": 12.7,
            "likes": -3,
            "gated": "manual",
            "gguf": { "total": "big", "context_length": 1e12, "architecture": 5 },
            "tags": ["license:other", "en", "fr", "region:us", 3, "base_model:finetune:a/b"],
            "cardData": { "license_name": "lfm1.0", "language": ["de", "en", "multilingual"] },
            "siblings": [{ "rfilename": "y-Q4_K_M.GGUF", "lfs": { "size": 5 } }, { "rfilename": "README.md" }]
        });
        let repo = from_json(&value).unwrap();
        assert_eq!(repo.downloads, 12);
        assert_eq!(repo.likes, 0);
        assert!(repo.gated);
        assert_eq!(repo.total, None);
        assert_eq!(repo.context_length, Some(u32::MAX));
        assert_eq!(repo.architecture, None);
        assert_eq!(repo.license.as_deref(), Some("lfm1.0"));
        assert_eq!(repo.languages, ["de", "en", "fr"]);
        assert_eq!(repo.base_model.as_deref(), Some("a/b"));
        assert_eq!(repo.base_relation.as_deref(), Some("finetune"));
        assert_eq!(
            repo.files,
            [RepoFile {
                name: "y-Q4_K_M.GGUF".into(),
                size: Some(5)
            }]
        );
        // Nom non ASCII parmi les fichiers (« 说明.md ») : ignoré, sans
        // panique, dans une liste comme dans une fiche.
        let siblings = serde_json::json!([
            { "rfilename": "说明.md" },
            { "rfilename": "读我😀.txt" },
            { "rfilename": "é-Q8_0.gguf" }
        ]);
        let list = serde_json::json!([{ "id": "a/b-GGUF", "siblings": siblings }]).to_string();
        let repos = list_from_json(list.as_bytes()).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(
            repos[0].files,
            [RepoFile {
                name: "é-Q8_0.gguf".into(),
                size: None
            }]
        );
        for id in [
            serde_json::json!({}),
            serde_json::json!({ "id": "noslash" }),
            serde_json::json!({ "id": "a/b/c" }),
            serde_json::json!({ "id": "/b" }),
        ] {
            assert_eq!(from_json(&id), None, "{id}");
        }
    }
}
