//! Catalogue du ModelHub (état Tauri) : requêtes à Hugging Face avec cache
//! disque, une seule requête en vol par ressource, et machine du serveur.
//! Contrat : `.tools/research/37-step15a-contract.md` § 2-7.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use url::Url;

use super::cache::{Cache, Entry, Kind, Limit};
use super::catalog;
use super::curation::CURATION;
use super::flight::Flights;
use super::gguf::{self, GgufFacts};
use super::hardware::{Detector, Machine};
use super::hf::{Expect, Failure, Hf, Reply, Request};
use super::names;
use super::quant;
use super::readme;
use super::repo::{self, Repo};
use super::types::{
    HubFilters, HubHardware, HubHome, HubList, HubModel, HubProblem, HubReadme, HubRepoResult,
    HubScope, HubSearch, HubSection, HubSectionId, HubStatus,
};
use crate::ollama::client::is_local;
use crate::store::now_ms;

/// Âge maximal de la mémoire montrée par la jauge de VRAM (relue toutes
/// les 2 s tant que l'onglet est affiché).
const MEMORY_KEEP: Duration = Duration::from_secs(1);
const HOUR: i64 = 60 * 60 * 1000;
const DAY: i64 = 24 * HOUR;

/// Délais (35 R1, contrat § 3).
const LIST_TIMEOUT: Duration = Duration::from_secs(15);
const DETAIL_TIMEOUT: Duration = Duration::from_secs(20);

/// Taille lue au plus d'une liste (500 dépôts : environ 5 Mo) et d'une fiche.
/// Une liste est lue élément par élément, 1000 dépôts au plus
/// (`repo::list_from_json`) : la mémoire reste bornée même pour une réponse
/// hostile de cette taille.
const LIST_MAX: usize = 16 * 1024 * 1024;
const DETAIL_MAX: usize = 8 * 1024 * 1024;

/// Champs demandés dans les listes : pas de `cardData` (35 R1).
const E_LIST: &str = "expand[]=author&expand[]=downloads&expand[]=likes&expand[]=createdAt&expand[]=lastModified&expand[]=tags&expand[]=pipeline_tag&expand[]=gated&expand[]=gguf&expand[]=baseModels&expand[]=trendingScore&expand[]=siblings";
/// Champs de la fiche (35 R1), avec les tailles des fichiers.
const E_DETAIL: &str = "blobs=true&expand[]=siblings&expand[]=gguf&expand[]=cardData&expand[]=tags&expand[]=downloads&expand[]=likes&expand[]=lastModified&expand[]=createdAt&expand[]=gated&expand[]=pipeline_tag&expand[]=baseModels&expand[]=author&expand[]=sha";

/// Dépôts affichés par rubrique de l'accueil.
const SECTION_SIZE: usize = 6;
/// Résultats par page de recherche.
const SEARCH_PAGE: usize = 30;

const LISTS: Kind = Kind {
    dir: "lists",
    limit: Limit::None,
    memory: true,
};
const SEARCHES: Kind = Kind {
    dir: "search",
    limit: Limit::Count(50),
    memory: false,
};
const REPOS: Kind = Kind {
    dir: "repos",
    limit: Limit::Count(300),
    memory: false,
};
const READMES: Kind = Kind {
    dir: "readme",
    limit: Limit::Bytes(30 * 1024 * 1024),
    memory: false,
};

/// Listes de l'accueil.
#[derive(Clone, Copy)]
enum List {
    /// 500 plus téléchargés, frais 24 h.
    Popular,
    /// 200 tendances, frais 6 h.
    Trending,
    /// Rubrique « Uncensored » : 100 plus téléchargés d'une recherche
    /// (`uncensored.queries` de la curation), frais 24 h ; demandée seulement
    /// si la rubrique est montrée.
    Uncensored(&'static str),
}

/// Dépôts d'une recherche de la rubrique « Uncensored ».
const UNCENSORED_LIMIT: usize = 100;

impl List {
    fn key(self) -> String {
        match self {
            List::Popular => "list:popular".into(),
            List::Trending => "list:trending".into(),
            List::Uncensored(query) => format!("list:uncensored:{query}"),
        }
    }

    fn max_age(self) -> i64 {
        match self {
            List::Popular | List::Uncensored(_) => DAY,
            List::Trending => 6 * HOUR,
        }
    }

    fn path(self) -> String {
        let (sort, limit) = match self {
            List::Popular => ("downloads", 500),
            List::Trending => ("trendingScore", 200),
            List::Uncensored(query) => {
                return format!(
                    "/api/models?apps=ollama&search={}&sort=downloads&limit={UNCENSORED_LIMIT}&{E_LIST}",
                    utf8_percent_encode(query, NON_ALPHANUMERIC)
                );
            }
        };
        format!("/api/models?apps=ollama&sort={sort}&limit={limit}&{E_LIST}")
    }
}

/// État de l'accueil : celui des populaires et des tendances, et celui de la
/// rubrique « Uncensored » si elle est montrée (date la plus ancienne,
/// premier problème).
fn merge_status(main: HubStatus, other: Option<HubStatus>) -> HubStatus {
    let Some(other) = other else {
        return main;
    };
    HubStatus {
        fetched_at: match (main.fetched_at, other.fetched_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        },
        problem: main.problem.or(other.problem),
    }
}

/// Page de liste ou de recherche, normalisée.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPage {
    pub repos: Vec<Repo>,
    pub next_cursor: Option<String>,
}

/// En-tête GGUF lu pour une fiche.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderRecord {
    /// Version du contenu de `facts` ([`gguf::FORMAT`]) ; 0 : avant les
    /// couches récurrentes et à fenêtre glissante, à relire.
    #[serde(default)]
    pub format: u32,
    pub sha: Option<String>,
    pub file: String,
    /// `None` : en-tête illisible (pas de nouvel essai pour ce commit).
    pub facts: Option<GgufFacts>,
}

/// Fiche normalisée, avec l'en-tête GGUF une fois lu.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoData {
    pub repo: Repo,
    pub header: Option<HeaderRecord>,
}

/// README préparé ; `None` : le dépôt n'en a pas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadmeData {
    pub text: Option<String>,
}

/// Réponse d'un chargement réseau.
enum Fetched<T> {
    /// Réponse 200 analysée : mise en cache.
    Fresh {
        data: T,
        etag: Option<String>,
    },
    /// Réponse montrée sans être mise en cache (404 d'un README absent).
    Uncached(T),
    NotModified,
}

/// Données montrées (fraîches ou anciennes) et problème éventuel.
struct Loaded<T> {
    entry: Option<Arc<Entry<T>>>,
    problem: Option<HubProblem>,
}

impl<T> Clone for Loaded<T> {
    fn clone(&self) -> Self {
        Self {
            entry: self.entry.clone(),
            problem: self.problem.clone(),
        }
    }
}

impl<T> Loaded<T> {
    fn status(&self) -> HubStatus {
        HubStatus {
            fetched_at: self.entry.as_ref().map(|entry| entry.fetched_at),
            problem: self.problem.clone(),
        }
    }
}

pub struct Catalog {
    hf: Hf,
    cache: Cache,
    flights: Flights,
    detector: Detector,
    /// Détection à part pour la jauge de VRAM : ses relectures fréquentes
    /// (et un échec passager de `nvidia-smi`) ne changent pas celle du
    /// ModelHub, gardée une minute.
    memory_detector: Detector,
    /// Délais des listes et des fiches (README, en-tête GGUF compris).
    list_timeout: Duration,
    detail_timeout: Duration,
}

/// Caractères laissés tels quels dans un segment de chemin.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.');

/// Identifiant « auteur/dépôt » qu'Ollama accepterait.
fn valid_id(id: &str) -> bool {
    names::parse_hf_name(&format!("{}/{id}", names::HF_HOST))
        .is_some_and(|name| name.tag.is_none() && name.repo_id() == id)
}

/// Révision : commit ou branche simple.
fn valid_revision(revision: &str) -> bool {
    (1..=64).contains(&revision.len())
        && revision
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        && !revision.starts_with('.')
}

fn is_sha(revision: &str) -> bool {
    revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Catalog {
    /// Catalogue dont le cache est dans `dir` (`app_data_dir()/modelhub`),
    /// adresses de Hugging Face de l'environnement.
    pub fn new(dir: PathBuf) -> Self {
        Self::with_urls(dir, names::hf_url(), names::hf_status_url())
    }

    pub fn with_urls(dir: PathBuf, base: String, status: String) -> Self {
        Self {
            hf: Hf::new(base, status),
            cache: Cache::new(dir),
            flights: Flights::default(),
            detector: Detector::default(),
            memory_detector: Detector::default(),
            list_timeout: LIST_TIMEOUT,
            detail_timeout: DETAIL_TIMEOUT,
        }
    }

    /// Délais raccourcis (essais d'un serveur muet).
    #[cfg(test)]
    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.list_timeout = timeout;
        self.detail_timeout = timeout;
        self
    }

    /// Ressource du cache si elle est fraîche (`max_age` en ms, `None` : sans
    /// limite), sinon chargée (une seule requête en vol par clé) ; échec :
    /// l'ancienne copie et le problème.
    async fn load<T, F, Fut>(
        &self,
        kind: Kind,
        key: &str,
        max_age: Option<i64>,
        fetch: F,
    ) -> Loaded<T>
    where
        T: Serialize + DeserializeOwned + Clone + Send + Sync + 'static,
        F: FnOnce(Option<String>) -> Fut,
        Fut: Future<Output = Result<Fetched<T>, Failure>>,
    {
        // Une entrée datée dans le futur (horloge recalée en arrière, cache
        // d'une autre machine) est périmée : sinon elle resterait fraîche
        // jusqu'à ce que l'horloge la rattrape.
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
                match fetch(etag).await {
                    Ok(Fetched::Fresh { data, etag }) => {
                        let entry = Arc::new(Entry {
                            fetched_at: now_ms(),
                            etag,
                            data,
                        });
                        self.cache.write(kind, key, entry.clone()).await;
                        Loaded {
                            entry: Some(entry),
                            problem: None,
                        }
                    }
                    Ok(Fetched::Uncached(data)) => Loaded {
                        entry: Some(Arc::new(Entry {
                            fetched_at: now_ms(),
                            etag: None,
                            data,
                        })),
                        problem: None,
                    },
                    Ok(Fetched::NotModified) => match stored {
                        // 304 : la copie est à jour, sa date est rafraîchie.
                        Some(old) => {
                            let entry = Arc::new(Entry {
                                fetched_at: now_ms(),
                                etag: old.etag.clone(),
                                data: old.data.clone(),
                            });
                            self.cache.write(kind, key, entry.clone()).await;
                            Loaded {
                                entry: Some(entry),
                                problem: None,
                            }
                        }
                        None => Loaded {
                            entry: None,
                            problem: Some(HubProblem::Unavailable { incident: false }),
                        },
                    },
                    Err(failure) => Loaded {
                        entry: stored,
                        problem: Some(self.hf.problem(failure).await),
                    },
                }
            })
            .await
    }

    /// Liste ou page de recherche de `/api/models`.
    async fn fetch_page(
        &self,
        path: &str,
        etag: Option<String>,
    ) -> Result<Fetched<ListPage>, Failure> {
        let reply = self
            .hf
            .get(Request {
                path,
                timeout: self.list_timeout,
                etag: etag.as_deref(),
                max_bytes: LIST_MAX,
                truncate: false,
                expect: Expect::Json,
                range: None,
            })
            .await?;
        match reply {
            Reply::NotModified => Ok(Fetched::NotModified),
            Reply::Body {
                bytes,
                etag,
                next_cursor,
            } => {
                let repos =
                    tauri::async_runtime::spawn_blocking(move || repo::list_from_json(&bytes))
                        .await
                        .ok()
                        .flatten()
                        .ok_or(Failure::Unavailable)?;
                Ok(Fetched::Fresh {
                    data: ListPage { repos, next_cursor },
                    etag,
                })
            }
        }
    }

    async fn list(&self, list: List) -> Loaded<ListPage> {
        let path = list.path();
        self.load(LISTS, &list.key(), Some(list.max_age()), |etag| {
            self.fetch_page(&path, etag)
        })
        .await
    }

    /// Populaires et tendances (en parallèle), avec l'état combiné : premier
    /// problème, date la plus ancienne montrée.
    async fn lists(&self) -> (Loaded<ListPage>, Loaded<ListPage>, HubStatus) {
        let (popular, trending) = tokio::join!(self.list(List::Popular), self.list(List::Trending));
        let status = HubStatus {
            fetched_at: [&popular, &trending]
                .iter()
                .filter_map(|loaded| loaded.entry.as_ref().map(|entry| entry.fetched_at))
                .min(),
            problem: popular.problem.clone().or_else(|| trending.problem.clone()),
        };
        (popular, trending, status)
    }

    /// Catalogue de l'accueil, filtré.
    async fn entries(
        &self,
        filters: &HubFilters,
        hardware: Option<&HubHardware>,
    ) -> (Vec<catalog::Entry>, HubStatus) {
        let (popular, trending, status) = self.lists().await;
        let (popular, trending) = (popular.entry, trending.entry);
        let hardware = hardware.cloned();
        let filters = filters.clone();
        // Analyse de quelque 700 dépôts : hors du fil de l'appel.
        let entries = tauri::async_runtime::spawn_blocking(move || {
            fn repos(entry: &Option<Arc<Entry<ListPage>>>) -> &[Repo] {
                entry.as_ref().map_or(&[], |entry| &entry.data.repos)
            }
            catalog::entries(repos(&popular), repos(&trending), hardware.as_ref())
                .into_iter()
                .filter(|entry| catalog::matches(&entry.model, &filters, hardware.is_some()))
                .collect()
        })
        .await
        .unwrap_or_default();
        (entries, status)
    }

    /// Rubrique « Uncensored », filtrée, et l'état de ses listes ; rien,
    /// sans requête, si elle n'est pas montrée (`shown`).
    async fn uncensored(
        &self,
        filters: &HubFilters,
        hardware: Option<&HubHardware>,
        shown: bool,
    ) -> (Vec<HubModel>, Option<HubStatus>) {
        if !shown {
            return (Vec::new(), None);
        }
        let loaded = futures_util::future::join_all(
            CURATION
                .uncensored_queries
                .iter()
                .map(|query| self.list(List::Uncensored(query))),
        )
        .await;
        let status = HubStatus {
            fetched_at: loaded
                .iter()
                .filter_map(|loaded| loaded.entry.as_ref().map(|entry| entry.fetched_at))
                .min(),
            problem: loaded.iter().find_map(|loaded| loaded.problem.clone()),
        };
        let entries: Vec<_> = loaded
            .into_iter()
            .filter_map(|loaded| loaded.entry)
            .collect();
        let hardware = hardware.cloned();
        let filters = filters.clone();
        let models = tauri::async_runtime::spawn_blocking(move || {
            let lists: Vec<&[Repo]> = entries
                .iter()
                .map(|entry| entry.data.repos.as_slice())
                .collect();
            catalog::uncensored_models(&lists, hardware.as_ref())
                .into_iter()
                .filter(|model| catalog::matches(model, &filters, hardware.is_some()))
                .collect()
        })
        .await
        .unwrap_or_default();
        (models, Some(status))
    }

    /// Accueil : rubriques (six dépôts au plus) et familles ; « Uncensored »
    /// en dernier si elle est montrée (`uncensored`).
    pub async fn home(
        &self,
        filters: &HubFilters,
        hardware: Option<&HubHardware>,
        uncensored: bool,
    ) -> HubHome {
        let ((entries, status), (mut hidden, hidden_status)) = tokio::join!(
            self.entries(filters, hardware),
            self.uncensored(filters, hardware, uncensored)
        );
        let mut sections: Vec<HubSection> = catalog::SECTIONS
            .iter()
            .filter_map(|&id| {
                let mut models = catalog::section(&entries, id, hardware.is_some());
                models.truncate(SECTION_SIZE);
                (!models.is_empty()).then_some(HubSection { id, models })
            })
            .collect();
        hidden.truncate(SECTION_SIZE);
        if !hidden.is_empty() {
            sections.push(HubSection {
                id: HubSectionId::Uncensored,
                models: hidden,
            });
        }
        HubHome {
            sections,
            families: catalog::families(&entries),
            status: merge_status(status, hidden_status),
        }
    }

    /// Liste complète d'une rubrique ou d'une famille (mêmes listes) ;
    /// « Uncensored » : vide, sans requête, si elle n'est pas montrée.
    pub async fn browse(
        &self,
        scope: &HubScope,
        filters: &HubFilters,
        hardware: Option<&HubHardware>,
        uncensored: bool,
    ) -> HubList {
        if *scope
            == (HubScope::Section {
                id: HubSectionId::Uncensored,
            })
        {
            let (models, status) = self.uncensored(filters, hardware, uncensored).await;
            return HubList {
                models,
                next_cursor: None,
                status: status.unwrap_or_default(),
            };
        }
        let (entries, status) = self.entries(filters, hardware).await;
        let models = match scope {
            HubScope::Section { id } => catalog::section(&entries, *id, hardware.is_some()),
            HubScope::Family { id } => catalog::family_models(&entries, id),
        };
        HubList {
            models,
            next_cursor: None,
            status,
        }
    }

    /// Recherche libre (30 résultats par page, curseur de Hugging Face).
    pub async fn search(&self, search: &HubSearch, hardware: Option<&HubHardware>) -> HubList {
        let text = search.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            return HubList {
                models: Vec::new(),
                next_cursor: None,
                status: HubStatus::default(),
            };
        }
        let cursor = search.cursor.as_deref().filter(|cursor| !cursor.is_empty());
        let mut path = format!(
            "/api/models?apps=ollama&search={}&sort=downloads&limit={SEARCH_PAGE}&{E_LIST}",
            utf8_percent_encode(&text, NON_ALPHANUMERIC)
        );
        if let Some(cursor) = cursor {
            path.push_str(&format!(
                "&cursor={}",
                utf8_percent_encode(cursor, NON_ALPHANUMERIC)
            ));
        }
        let key = format!(
            "search:{}|{}",
            text.to_lowercase(),
            cursor.unwrap_or_default()
        );
        let loaded = self
            .load(SEARCHES, &key, Some(HOUR), |etag| {
                self.fetch_page(&path, etag)
            })
            .await;
        let machine_known = hardware.is_some();
        let (models, next_cursor) = match &loaded.entry {
            Some(entry) => (
                entry
                    .data
                    .repos
                    .iter()
                    .filter(|repo| catalog::usable(repo))
                    .map(|repo| catalog::model(repo, hardware))
                    .filter(|model| catalog::matches(model, &search.filters, machine_known))
                    .collect(),
                entry.data.next_cursor.clone(),
            ),
            None => (Vec::new(), None),
        };
        HubList {
            models,
            next_cursor,
            status: loaded.status(),
        }
    }

    /// Dernières listes de l'accueil en cache, sans requête.
    async fn cached_repos(&self) -> Vec<Arc<Entry<ListPage>>> {
        let mut found = Vec::new();
        for list in [List::Popular, List::Trending] {
            if let Some(entry) = self.cache.read::<ListPage>(LISTS, &list.key()).await {
                found.push(entry);
            }
        }
        found
    }

    /// Fiche d'un dépôt (« auteur/dépôt »).
    pub async fn repo(&self, id: &str, hardware: Option<&HubHardware>) -> HubRepoResult {
        if !valid_id(id) {
            return HubRepoResult {
                repo: None,
                status: HubStatus {
                    fetched_at: None,
                    problem: Some(HubProblem::NotFound),
                },
            };
        }
        let key = format!("repo:{}", id.to_ascii_lowercase());
        let lists = self.cached_repos().await;
        // Une liste plus récente qui donne une autre date de modification
        // rend la fiche périmée.
        let stored = self.cache.read::<RepoData>(REPOS, &key).await;
        let changed = stored.as_ref().is_some_and(|stored| {
            lists.iter().any(|list| {
                list.fetched_at > stored.fetched_at
                    && list.data.repos.iter().any(|listed| {
                        listed.id.eq_ignore_ascii_case(id)
                            && listed.last_modified != stored.data.repo.last_modified
                    })
            })
        });
        let max_age = if changed { 0 } else { DAY };
        let path = format!("/api/models/{id}?{E_DETAIL}");
        let loaded = self
            .load(REPOS, &key, Some(max_age), |etag| {
                self.fetch_detail(&path, etag)
            })
            .await;
        let status = loaded.status();
        let Some(entry) = loaded.entry else {
            return HubRepoResult { repo: None, status };
        };
        let header = match hardware {
            Some(_) => self.header(&key, &entry).await,
            None => None,
        };
        let known: Vec<&Repo> = lists.iter().flat_map(|list| &list.data.repos).collect();
        let others = catalog::other_sources(&entry.data.repo, &known);
        HubRepoResult {
            repo: Some(catalog::repo_view(
                &entry.data.repo,
                header.as_ref(),
                hardware,
                others,
            )),
            status,
        }
    }

    async fn fetch_detail(
        &self,
        path: &str,
        etag: Option<String>,
    ) -> Result<Fetched<RepoData>, Failure> {
        let reply = self
            .hf
            .get(Request {
                path,
                timeout: self.detail_timeout,
                etag: etag.as_deref(),
                max_bytes: DETAIL_MAX,
                truncate: false,
                expect: Expect::Json,
                range: None,
            })
            .await?;
        match reply {
            Reply::NotModified => Ok(Fetched::NotModified),
            Reply::Body { bytes, etag, .. } => {
                let repo = serde_json::from_slice::<serde_json::Value>(&bytes)
                    .ok()
                    .as_ref()
                    .and_then(repo::from_json)
                    .ok_or(Failure::Unavailable)?;
                Ok(Fetched::Fresh {
                    data: RepoData { repo, header: None },
                    etag,
                })
            }
        }
    }

    /// En-tête GGUF du plus petit fichier téléchargeable (requête Range de
    /// 256 Kio), lu une fois par commit et gardé avec la fiche. `None` : pas
    /// de version téléchargeable, dépôt restreint, échec réseau (estimation
    /// sans en-tête).
    async fn header(&self, key: &str, entry: &Arc<Entry<RepoData>>) -> Option<GgufFacts> {
        let repo = &entry.data.repo;
        if repo.gated {
            return None;
        }
        let file = quant::inventory(&repo.id, &repo.file_entries())
            .into_iter()
            .filter(|version| version.model.is_some())
            .min_by_key(|version| version.size)?
            .file;
        if let Some(record) = &entry.data.header
            && record.format == gguf::FORMAT
            && record.sha == repo.sha
            && record.file == file
        {
            return record.facts.clone();
        }
        let revision = repo.sha.clone().unwrap_or_else(|| "main".to_owned());
        let path = format!(
            "/{}/resolve/{}/{}",
            repo.id,
            utf8_percent_encode(&revision, SEGMENT),
            utf8_percent_encode(&file, SEGMENT)
        );
        let flight = format!("header:{key}@{revision}");
        self.flights
            .run(&flight, || async {
                let reply = self
                    .hf
                    .get(Request {
                        path: &path,
                        timeout: self.detail_timeout,
                        etag: None,
                        max_bytes: gguf::HEADER_BYTES,
                        truncate: true,
                        expect: Expect::Bytes,
                        range: Some(&format!("bytes=0-{}", gguf::HEADER_BYTES - 1)),
                    })
                    .await;
                let Ok(Reply::Body { bytes, .. }) = reply else {
                    return None;
                };
                let facts = gguf::parse(&bytes);
                let mut data = entry.data.clone();
                data.header = Some(HeaderRecord {
                    format: gguf::FORMAT,
                    sha: repo.sha.clone(),
                    file: file.clone(),
                    facts: facts.clone(),
                });
                let updated = Arc::new(Entry {
                    fetched_at: entry.fetched_at,
                    etag: entry.etag.clone(),
                    data,
                });
                self.cache.write(REPOS, key, updated).await;
                facts
            })
            .await
    }

    /// README d'un dépôt à une révision (commit : gardé sans limite de durée).
    pub async fn readme(&self, id: &str, revision: &str) -> HubReadme {
        if !valid_id(id) || !valid_revision(revision) {
            return HubReadme {
                text: None,
                problem: Some(HubProblem::NotFound),
            };
        }
        let key = format!("readme:{}@{revision}", id.to_ascii_lowercase());
        let max_age = if is_sha(revision) { None } else { Some(DAY) };
        let path = format!("/{id}/raw/{revision}/README.md");
        let loaded = self
            .load(READMES, &key, max_age, |etag| async move {
                let reply = self
                    .hf
                    .get(Request {
                        path: &path,
                        timeout: self.detail_timeout,
                        etag: etag.as_deref(),
                        max_bytes: readme::MAX_BODY,
                        truncate: true,
                        expect: Expect::Text,
                        range: None,
                    })
                    .await;
                match reply {
                    Ok(Reply::NotModified) => Ok(Fetched::NotModified),
                    Ok(Reply::Body { bytes, etag, .. }) => Ok(Fetched::Fresh {
                        data: ReadmeData {
                            text: Some(readme::prepare(&String::from_utf8_lossy(&bytes))),
                        },
                        etag,
                    }),
                    // Pas de README : rien à montrer, sans problème (seules
                    // les réponses 200 vont dans le cache).
                    Err(Failure::Missing) => Ok(Fetched::Uncached(ReadmeData { text: None })),
                    Err(failure) => Err(failure),
                }
            })
            .await;
        HubReadme {
            text: loaded.entry.and_then(|entry| entry.data.text.clone()),
            problem: loaded.problem,
        }
    }

    /// Machine du serveur : `None` pour un serveur distant. `context` : réglage
    /// de ChatonLM (0 : celui d'Ollama d'après la VRAM).
    pub async fn hardware(&self, server_url: &str, context: u32) -> Option<HubHardware> {
        let machine = self
            .machine(&self.detector, server_url, super::hardware::KEEP)
            .await?;
        Some(machine.with_context(context))
    }

    /// Mémoire de la machine du serveur pour la jauge de VRAM : détection
    /// d'au plus une seconde ; `None` pour un serveur distant.
    pub async fn memory(&self, server_url: &str) -> Option<Machine> {
        self.machine(&self.memory_detector, server_url, MEMORY_KEEP)
            .await
    }

    /// Machine du serveur local détectée par `detector` depuis au plus `keep`
    /// (celle injectée par `CHATONLM_E2E_HARDWARE` dans les essais).
    async fn machine(
        &self,
        detector: &Detector,
        server_url: &str,
        keep: Duration,
    ) -> Option<Machine> {
        #[cfg(any(debug_assertions, feature = "e2e"))]
        if let Some(injected) = super::hardware::injected() {
            return injected;
        }
        if !Url::parse(server_url).is_ok_and(|url| is_local(&url)) {
            return None;
        }
        detector.machine_within(keep).await
    }
}

#[cfg(test)]
mod tests;
