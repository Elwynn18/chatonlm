//! Dépôts d'images de Hugging Face (contrat § 5) : recherche, fiche et
//! reconnaissance des familles. Même client que le ModelHub (`hub::hf` :
//! délais, 429 partagé, page d'état, `HubProblem`), même cache disque
//! (`hub::cache`, sous `modelhub/image-*`), une seule requête en vol par
//! ressource (`hub::flight`) ; sans `apps=ollama`.

use std::sync::Arc;
use std::time::Duration;

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::families::{Families, Recognition};
use crate::hub::cache::{Cache, Entry, Kind, Limit};
use crate::hub::flight::Flights;
use crate::hub::hf::{Expect, Failure, Hf, Reply, Request};
use crate::hub::names;
use crate::hub::types::HubProblem;
use crate::store::now_ms;

const HOUR: i64 = 60 * 60 * 1000;
const DAY: i64 = 24 * HOUR;
const LIST_TIMEOUT: Duration = Duration::from_secs(15);
const DETAIL_TIMEOUT: Duration = Duration::from_secs(20);
const LIST_MAX: usize = 16 * 1024 * 1024;
const DETAIL_MAX: usize = 8 * 1024 * 1024;
/// Dépôts demandés par liste.
const LIST_LIMIT: usize = 50;
/// Fiches de bases lues au plus pour une recherche (reconnaissance au
/// deuxième cran).
const SECOND_STEP_MAX: usize = 12;

/// Champs des listes (sans `cardData`).
const E_LIST: &str = "expand[]=author&expand[]=downloads&expand[]=likes&expand[]=lastModified&expand[]=tags&expand[]=gated&expand[]=baseModels&expand[]=siblings&expand[]=pipeline_tag";
/// Champs de la fiche, tailles et empreintes des fichiers comprises.
const E_DETAIL: &str = "blobs=true&expand[]=siblings&expand[]=cardData&expand[]=tags&expand[]=downloads&expand[]=likes&expand[]=lastModified&expand[]=gated&expand[]=pipeline_tag&expand[]=baseModels&expand[]=author&expand[]=sha";

const SEARCHES: Kind = Kind {
    dir: "image-search",
    limit: Limit::Count(50),
    memory: false,
};
const REPOS: Kind = Kind {
    dir: "image-repos",
    limit: Limit::Count(500),
    memory: false,
};

/// Fichier d'un dépôt ; taille et empreinte connues dans la fiche seulement
/// (`lfs.sha256` : fichier sans `lfs` jamais téléchargé).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoBlob {
    pub path: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

/// Dépôt normalisé, écrit tel quel dans le cache.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRepo {
    pub id: String,
    pub author: String,
    pub downloads: u64,
    pub likes: u64,
    pub last_modified: Option<String>,
    pub license: Option<String>,
    pub gated: bool,
    /// Modèles de base déclarés (`cardData.base_model`, `baseModels`, tags
    /// `base_model:`), sans doublon.
    pub bases: Vec<String>,
    /// Commit de la fiche.
    pub sha: Option<String>,
    pub files: Vec<RepoBlob>,
    /// Adaptateur d'un modèle (relation de base `adapter`, tag `lora`) :
    /// jamais un modèle complet.
    #[serde(default)]
    pub adapter: bool,
    /// Tag `not-for-all-audiences` : non censuré.
    #[serde(default)]
    pub adult: bool,
}

impl ImageRepo {
    /// Nom du dépôt, sans l'auteur.
    pub fn name(&self) -> &str {
        self.id.split_once('/').map_or(&self.id, |(_, name)| name)
    }

    pub fn blob(&self, path: &str) -> Option<&RepoBlob> {
        self.files.iter().find(|blob| blob.path == path)
    }
}

fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn count(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_f64().filter(|n| *n >= 0.0).map(|n| n as u64))
}

/// Identifiant « auteur/dépôt » plausible.
pub fn valid_repo_id(id: &str) -> bool {
    let part = |part: &str| {
        (1..=96).contains(&part.len())
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
            && !part.starts_with('.')
            && !part.contains("..")
    };
    id.split_once('/')
        .is_some_and(|(author, name)| part(author) && part(name))
}

/// Dépôt d'une réponse de `/api/models` (liste ou fiche) ; `None` sans
/// identifiant valable.
pub fn repo_from_json(value: &Value) -> Option<ImageRepo> {
    let id = text(&value["id"])?;
    if !valid_repo_id(&id) {
        return None;
    }
    let tags: Vec<String> = value["tags"]
        .as_array()
        .map(|tags| tags.iter().filter_map(text).collect())
        .unwrap_or_default();
    let card = &value["cardData"];
    let mut bases: Vec<String> = Vec::new();
    let mut add = |base: String| {
        if base.contains('/') && !bases.iter().any(|known| known.eq_ignore_ascii_case(&base)) {
            bases.push(base);
        }
    };
    match &card["base_model"] {
        Value::String(base) => add(base.trim().to_owned()),
        Value::Array(list) => list.iter().filter_map(text).for_each(&mut add),
        _ => {}
    }
    if let Some(models) = value["baseModels"]["models"].as_array() {
        models
            .iter()
            .filter_map(|model| text(&model["id"]))
            .for_each(&mut add);
    }
    let mut adapter = text(&value["baseModels"]["relation"]).as_deref() == Some("adapter");
    for tag in &tags {
        if let Some(rest) = tag.strip_prefix("base_model:") {
            let base = rest.split_once(':').map_or(rest, |(relation, id)| {
                adapter |= relation == "adapter";
                id
            });
            add(base.to_owned());
        }
    }
    adapter |= tags.iter().any(|tag| tag.eq_ignore_ascii_case("lora"));
    let adult = tags
        .iter()
        .any(|tag| tag.eq_ignore_ascii_case("not-for-all-audiences"));
    // Comme le ModelHub (`hub/repo.rs`) : tag `license:`, sinon la carte ;
    // « other » : son nom.
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
    let files = value["siblings"]
        .as_array()
        .map(|siblings| {
            siblings
                .iter()
                .filter_map(|sibling| {
                    Some(RepoBlob {
                        path: text(&sibling["rfilename"])?,
                        size: count(&sibling["size"]).or_else(|| count(&sibling["lfs"]["size"])),
                        sha256: text(&sibling["lfs"]["sha256"])
                            .filter(|sha| super::families::is_sha256(sha))
                            .map(|sha| sha.to_ascii_lowercase()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(ImageRepo {
        author: text(&value["author"])
            .unwrap_or_else(|| id.split('/').next().unwrap_or_default().to_owned()),
        downloads: count(&value["downloads"]).unwrap_or(0),
        likes: count(&value["likes"]).unwrap_or(0),
        last_modified: text(&value["lastModified"]),
        license,
        gated: match &value["gated"] {
            Value::Bool(gated) => *gated,
            Value::String(mode) => !mode.is_empty() && mode != "false",
            _ => false,
        },
        bases,
        sha: text(&value["sha"]),
        files,
        adapter,
        adult,
        id,
    })
}

/// Données montrées (fraîches ou du cache) et problème éventuel.
pub struct Loaded<T> {
    pub entry: Option<Arc<Entry<T>>>,
    pub problem: Option<HubProblem>,
}

impl<T> Clone for Loaded<T> {
    fn clone(&self) -> Self {
        Self {
            entry: self.entry.clone(),
            problem: self.problem.clone(),
        }
    }
}

/// Ce que l'on sait de la famille d'un dépôt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Known {
    Family(String),
    Unsupported,
    Unknown,
}

/// Dépôt reconnu : famille et non censuré (tag `not-for-all-audiences`, ou un
/// mot dans son identifiant ou celui d'une base).
#[derive(Clone, Debug)]
pub struct Recognized {
    pub known: Known,
    pub uncensored: bool,
    /// Une base qu'il fallait lire ne l'a pas été (fiche illisible, plafond) :
    /// non censuré possible, par prudence.
    pub unverified: bool,
}

pub struct ImageCatalog {
    hf: Hf,
    cache: Cache,
    flights: Flights,
    list_timeout: Duration,
    detail_timeout: Duration,
}

impl ImageCatalog {
    /// Cache dans `dir` (`<données>/modelhub`), adresses de Hugging Face de
    /// l'environnement (`CHATONLM_HF_URL` dans les essais).
    pub fn new(dir: std::path::PathBuf) -> Self {
        Self::with_urls(dir, names::hf_url(), names::hf_status_url())
    }

    pub fn with_urls(dir: std::path::PathBuf, base: String, status: String) -> Self {
        Self {
            hf: Hf::new(base, status),
            cache: Cache::new(dir),
            flights: Flights::default(),
            list_timeout: LIST_TIMEOUT,
            detail_timeout: DETAIL_TIMEOUT,
        }
    }

    /// Ressource du cache si elle est fraîche (`max_age` en ms, `None` :
    /// sans limite), sinon chargée (une requête en vol par clé) ; échec :
    /// l'ancienne copie et le problème. Délai et taille selon le genre (liste
    /// ou fiche).
    async fn load<T>(
        &self,
        kind: Kind,
        key: &str,
        max_age: Option<i64>,
        path: &str,
        parse: fn(&[u8]) -> Option<T>,
    ) -> Loaded<T>
    where
        T: Serialize + serde::de::DeserializeOwned + Clone + Send + Sync + 'static,
    {
        let (timeout, max_bytes) = if kind.dir == SEARCHES.dir {
            (self.list_timeout, LIST_MAX)
        } else {
            (self.detail_timeout, DETAIL_MAX)
        };
        let fresh = |entry: &Entry<T>| {
            max_age.is_none_or(|age| (0..age).contains(&(now_ms() - entry.fetched_at)))
        };
        if let Some(entry) = self.cache.read::<T>(kind, key).await
            && fresh(&entry)
        {
            return Loaded {
                entry: Some(entry),
                problem: None,
            };
        }
        let flight = format!("{}:{key}", kind.dir);
        self.flights
            .run(&flight, || async {
                let stored = self.cache.read::<T>(kind, key).await;
                if let Some(entry) = &stored
                    && fresh(entry)
                {
                    return Loaded {
                        entry: stored,
                        problem: None,
                    };
                }
                let etag = stored.as_ref().and_then(|entry| entry.etag.clone());
                let reply = self
                    .hf
                    .get(Request {
                        path,
                        timeout,
                        etag: etag.as_deref(),
                        max_bytes,
                        truncate: false,
                        expect: Expect::Json,
                        range: None,
                    })
                    .await;
                let failure = match reply {
                    Ok(Reply::Body { bytes, etag, .. }) => {
                        let parsed = tauri::async_runtime::spawn_blocking(move || parse(&bytes))
                            .await
                            .ok()
                            .flatten();
                        match parsed {
                            Some(data) => {
                                let entry = Arc::new(Entry {
                                    fetched_at: now_ms(),
                                    etag,
                                    data,
                                });
                                self.cache.write(kind, key, entry.clone()).await;
                                return Loaded {
                                    entry: Some(entry),
                                    problem: None,
                                };
                            }
                            None => Failure::Unavailable,
                        }
                    }
                    Ok(Reply::NotModified) => match stored {
                        Some(old) => {
                            let entry = Arc::new(Entry {
                                fetched_at: now_ms(),
                                etag: old.etag.clone(),
                                data: old.data.clone(),
                            });
                            self.cache.write(kind, key, entry.clone()).await;
                            return Loaded {
                                entry: Some(entry),
                                problem: None,
                            };
                        }
                        None => Failure::Unavailable,
                    },
                    Err(failure) => failure,
                };
                Loaded {
                    entry: stored,
                    problem: Some(self.hf.problem(failure).await),
                }
            })
            .await
    }

    /// Une liste de `/api/models` (une heure de fraîcheur).
    async fn list(&self, key: &str, path: &str) -> Loaded<Vec<ImageRepo>> {
        self.load(SEARCHES, key, Some(HOUR), path, |bytes| {
            let values: Vec<Value> = serde_json::from_slice(bytes).ok()?;
            Some(
                values
                    .iter()
                    .take(1000)
                    .filter_map(repo_from_json)
                    .collect(),
            )
        })
        .await
    }

    /// Dépôts GGUF de génération d'images, puis dépôts à fichier unique
    /// (SD 1.5 et SDXL sont filtrés par l'appelant) ; `query` vide : les plus
    /// téléchargés.
    pub async fn search_lists(
        &self,
        query: &str,
    ) -> (Loaded<Vec<ImageRepo>>, Loaded<Vec<ImageRepo>>) {
        let search = if query.is_empty() {
            String::new()
        } else {
            format!("&search={}", utf8_percent_encode(query, NON_ALPHANUMERIC))
        };
        let gguf = format!(
            "/api/models?pipeline_tag=text-to-image&filter=gguf{search}&sort=downloads&limit={LIST_LIMIT}&{E_LIST}"
        );
        let single = format!(
            "/api/models?pipeline_tag=text-to-image{search}&sort=downloads&limit={LIST_LIMIT}&{E_LIST}"
        );
        let lower = query.to_lowercase();
        let (gguf_key, single_key) = (
            format!("images:gguf|{lower}"),
            format!("images:single|{lower}"),
        );
        tokio::join!(self.list(&gguf_key, &gguf), self.list(&single_key, &single))
    }

    /// Fiche d'un dépôt (sa dernière révision, gardée un jour).
    pub async fn repo(&self, id: &str) -> Loaded<ImageRepo> {
        if !valid_repo_id(id) {
            return Loaded {
                entry: None,
                problem: Some(HubProblem::NotFound),
            };
        }
        let key = format!("image-repo:{}", id.to_ascii_lowercase());
        let path = format!("/api/models/{id}?{E_DETAIL}");
        self.load(REPOS, &key, Some(DAY), &path, |bytes| {
            serde_json::from_slice::<Value>(bytes)
                .ok()
                .as_ref()
                .and_then(repo_from_json)
        })
        .await
    }

    /// Bases déclarées par la fiche de `base` ; `None` si elle ne se lit pas.
    async fn bases_of(&self, base: &str) -> Option<Vec<String>> {
        self.repo(base)
            .await
            .entry
            .map(|entry| entry.data.bases.clone())
    }

    /// Reconnaissance d'un dépôt : son identifiant, puis ses bases, puis les
    /// bases de ses bases (fiches lues, en cache, 12 au plus).
    pub async fn recognize(&self, families: &Families, repo: &ImageRepo) -> Recognized {
        self.recognize_all(families, std::slice::from_ref(repo), SECOND_STEP_MAX)
            .await
            .pop()
            .unwrap_or(Recognized {
                known: Known::Unknown,
                uncensored: false,
                unverified: true,
            })
    }

    /// Reconnaissance de plusieurs dépôts ; au plus `second_step` fiches de
    /// bases lues (en parallèle) pour le deuxième cran.
    pub async fn recognize_all(
        &self,
        families: &Families,
        repos: &[ImageRepo],
        second_step: usize,
    ) -> Vec<Recognized> {
        let first = |repo: &ImageRepo| -> Option<Known> {
            match families.recognize(std::slice::from_ref(&repo.id), false) {
                Some(found) => Some(known(found)),
                None => families.recognize(&repo.bases, true).map(known),
            }
        };
        // Bases à lire pour le deuxième cran.
        let mut wanted: Vec<String> = Vec::new();
        for repo in repos {
            if first(repo).is_none() {
                for base in &repo.bases {
                    if wanted.len() < second_step
                        && !wanted.iter().any(|known| known.eq_ignore_ascii_case(base))
                    {
                        wanted.push(base.clone());
                    }
                }
            }
        }
        let fetched =
            futures_util::future::join_all(wanted.iter().map(|base| self.bases_of(base))).await;
        // `None` : fiche illisible, ou non lue (plafond atteint).
        let second = |base: &str| -> Option<&[String]> {
            wanted
                .iter()
                .position(|known| known.eq_ignore_ascii_case(base))
                .and_then(|index| fetched[index].as_deref())
        };
        let curation = &*crate::hub::curation::CURATION;
        repos
            .iter()
            .map(|repo| {
                let direct = first(repo);
                let grand: Vec<String> = repo
                    .bases
                    .iter()
                    .flat_map(|base| second(base).unwrap_or_default().iter().cloned())
                    .collect();
                // Deuxième cran nécessaire mais une base n'a pas pu être lue.
                let unverified =
                    direct.is_none() && repo.bases.iter().any(|base| second(base).is_none());
                let known = direct.unwrap_or_else(|| {
                    families
                        .recognize(&grand, true)
                        .map_or(Known::Unknown, known)
                });
                let uncensored = repo.adult
                    || std::iter::once(&repo.id)
                        .chain(&repo.bases)
                        .chain(&grand)
                        .any(|id| curation.uncensored_image(id));
                Recognized {
                    known,
                    uncensored,
                    unverified,
                }
            })
            .collect()
    }

    /// Nombre de fiches de bases lues au plus pour une recherche.
    pub fn second_step_max() -> usize {
        SECOND_STEP_MAX
    }
}

fn known(recognition: Recognition<'_>) -> Known {
    match recognition {
        Recognition::Family(family) => Known::Family(family.id.clone()),
        Recognition::Unsupported => Known::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repos_are_read_with_their_bases_licenses_and_checksums() {
        let value = serde_json::json!({
            "id": "someone/Klein-GGUF",
            "tags": ["gguf", "base_model:quantized:black-forest-labs/FLUX.2-klein-4B", "base_model:other/thing", "license:other"],
            "cardData": { "base_model": ["Black-Forest-Labs/FLUX.2-klein-4B"], "license_name": "flux-non-commercial" },
            "baseModels": { "relation": "quantized", "models": [{ "id": "third/base" }] },
            "gated": "auto",
            "sha": "0123456789abcdef0123456789abcdef01234567",
            "siblings": [
                { "rfilename": "a.gguf", "size": 10, "lfs": { "sha256": "AB".repeat(32), "size": 10 } },
                { "rfilename": "README.md", "size": 3 },
                { "rfilename": "bad.gguf", "lfs": { "sha256": "zz", "size": 4 } }
            ]
        });
        let repo = repo_from_json(&value).unwrap();
        assert!(!repo.adapter && !repo.adult);
        assert_eq!(repo.author, "someone");
        assert_eq!(repo.name(), "Klein-GGUF");
        assert_eq!(
            repo.bases,
            [
                "Black-Forest-Labs/FLUX.2-klein-4B",
                "third/base",
                "other/thing"
            ]
        );
        assert_eq!(repo.license.as_deref(), Some("flux-non-commercial"));
        assert!(repo.gated);
        assert_eq!(repo.blob("a.gguf").unwrap().sha256, Some("ab".repeat(32)));
        assert_eq!(repo.blob("README.md").unwrap().sha256, None);
        assert_eq!(repo.blob("bad.gguf").unwrap().size, Some(4));
        assert_eq!(repo.blob("bad.gguf").unwrap().sha256, None);
        assert!(repo_from_json(&serde_json::json!({ "id": "../x" })).is_none());
        assert!(repo_from_json(&serde_json::json!({ "id": "noslash" })).is_none());
    }

    #[test]
    fn adapters_and_adult_repos_are_told() {
        let read = |value: serde_json::Value| repo_from_json(&value).unwrap();
        assert!(read(serde_json::json!({
            "id": "latent-consistency/lcm-lora-sdxl",
            "baseModels": { "relation": "adapter", "models": [{ "id": "stabilityai/stable-diffusion-xl-base-1.0" }] }
        }))
        .adapter);
        assert!(
            read(serde_json::json!({
                "id": "a/b",
                "tags": ["base_model:adapter:stabilityai/stable-diffusion-xl-base-1.0"]
            }))
            .adapter
        );
        assert!(read(serde_json::json!({ "id": "a/b", "tags": ["LoRA"] })).adapter);
        let adult = read(serde_json::json!({ "id": "a/b", "tags": ["not-for-all-audiences"] }));
        assert!(adult.adult && !adult.adapter);
    }

    #[test]
    fn repository_ids_are_checked() {
        assert!(valid_repo_id("leejet/FLUX.2-klein-4B-GGUF"));
        assert!(!valid_repo_id("a/b/c"));
        assert!(!valid_repo_id("a/.."));
        assert!(!valid_repo_id("a/b c"));
    }
}
