// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Partly adapted from Ollama (https://github.com/ollama/ollama),
// file server/model_recommendations.go, commit 42e911bc3d05: the default
// model recommendations, served by this test server as they are.

//! Faux serveur Ollama pour les tests de ChatonLM : comportements déterministes,
//! sans modèle ni GPU. Il écoute sur 127.0.0.1, sur un port libre. Les réponses
//! d'Ollama viennent de `fixtures/`, enregistrées sur une vraie instance
//! (scripts/record-fixtures.sh) ; les réponses Markdown (`fixtures/*.md`) sont
//! écrites à la main et découpées en morceaux par le serveur ; les
//! téléchargements (`/api/pull`) sont synthétiques, aux lignes de la forme de
//! celles d'Ollama ; les recommandations sont la liste par défaut d'Ollama.
//! Le serveur sert aussi de registre (`HEAD /v2/.../manifests/...`, voir
//! [`manifest`]) : celui d'ollama.com pour l'espace `library`, celui de
//! Hugging Face pour l'espace `chatonlm` (modèles `hf.co/chatonlm/...`, voir
//! [`hf_pull`] ; une adresse `CHATONLM_HF_URL` qui pointe sur ce serveur fait
//! vérifier leurs manifestes ici). `DELETE /api/delete` supprime un modèle.
//! Les modèles en mémoire (aucun au départ) se lisent sur `GET /api/ps` ;
//! `/api/chat` et `POST /api/generate` sans prompt les chargent,
//! `/api/generate` avec `keep_alive: 0` les décharge (voir [`generate`]).
//!
//! Il joue aussi le compte du serveur et ollama.com pour la recherche web :
//! compte relié ou non (`POST /fake/account`, délié au départ), `/api/me`
//! (503 « account unavailable » après `POST /fake/account-unavailable`, comme
//! le vrai serveur quand ollama.com ne répond pas), `/api/signout`,
//! `/api/status` (cloud coupé par `POST /fake/cloud`),
//! `/api/experimental/web_search` et `web_fetch` (voir [`web_search`] et
//! [`web_fetch`]), avatar (`GET /fake/avatar.png`) ; aucun test ne touche le
//! vrai réseau.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Faux Hugging Face (serveur à part) : voir [`hf`].
pub mod hf;

/// Lancé comme `ollama serve` (Ollama intégré de l'app) : voir [`serve`].
pub mod serve;

/// Faux `sd-server` (moteur d'images de l'app) : voir [`sdcpp`].
pub mod sdcpp;

/// Cours de ChatonLM (modèles `chatonlm-study*`) : voir [`study`].
pub mod study;
pub use study::STUDY_SLOW;

/// Comportement du serveur.
#[derive(Clone, Debug)]
pub enum Mode {
    /// Répond comme Ollama, avec les modèles enregistrés (`fixtures/`), ou
    /// sans aucun modèle si `models` est faux.
    Ollama { version: String, models: bool },
    /// Un autre programme occupe le port : page HTML, pas l'API d'Ollama.
    NotOllama,
    /// Accepte les connexions mais ne répond jamais.
    Silent,
    /// Accepte les connexions et les ferme aussitôt (proxy dont la cible est
    /// arrêtée, conteneur qui démarre).
    Close,
    /// Répond à tout avec ce code d'état et un corps vide (proxy, contrôle
    /// d'accès).
    Status(u16),
    /// Répond 200 à `/api/version` avec un corps de 1 Mio.
    Huge,
}

impl Mode {
    pub fn ollama() -> Self {
        Mode::Ollama {
            version: "0.35.1".into(),
            models: true,
        }
    }

    /// Ollama sans aucun modèle installé.
    pub fn no_models() -> Self {
        Mode::Ollama {
            version: "0.35.1".into(),
            models: false,
        }
    }
}

pub struct FakeOllama {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
    journal: Arc<Journal>,
}

/// Ce que le serveur a reçu : corps des requêtes de chat et de téléchargement,
/// requêtes de manifeste au registre, noms des suppressions, requêtes de la
/// recherche web, chargements et déchargements, et nombre de flux (chat ou
/// téléchargement), de chargements lents ou de requêtes web lentes abandonnés
/// par le client avant leur fin (annulation). Aussi lisibles en HTTP :
/// `GET /fake/requests`, `GET /fake/pulls`, `GET /fake/deletes`,
/// `GET /fake/web`, `GET /fake/loads`, `GET /fake/disconnects`,
/// `GET /fake/manifests` (requêtes reçues par le registre). Garde aussi
/// les modèles de scénario téléchargés sur ce serveur (voir [`pull`]), les
/// modèles enregistrés supprimés, les modèles en mémoire (voir [`generate`]),
/// l'état du compte et celui du cloud.
#[derive(Default)]
pub struct Journal {
    chats: Mutex<Vec<serde_json::Value>>,
    pulls: Mutex<Vec<serde_json::Value>>,
    manifests: Mutex<Vec<String>>,
    deletes: Mutex<Vec<String>>,
    /// `{"path": "/api/experimental/web_search", "body": {…}}`, dans l'ordre.
    web: Mutex<Vec<serde_json::Value>>,
    /// Compte ollama.com relié au serveur (délié au départ).
    signed_in: AtomicBool,
    /// `/api/me` en échec (ollama.com injoignable pour le serveur).
    account_unavailable: AtomicBool,
    /// Cloud coupé (comme `OLLAMA_NO_CLOUD`).
    cloud_disabled: AtomicBool,
    disconnects: AtomicUsize,
    /// Du plus récent au plus ancien.
    pulled: Mutex<Vec<Pulled>>,
    /// Nouvelles versions publiées au registre (voir [`Journal::publish`]).
    published: Mutex<Vec<Published>>,
    /// Modèles enregistrés (`fixtures/tags.json`) supprimés.
    deleted: Mutex<Vec<String>>,
    /// Modèles en mémoire (`/api/ps`), aucun au départ.
    loaded: Mutex<Vec<Loaded>>,
    /// Chargements et déchargements demandés à `/api/generate`, dans l'ordre :
    /// `{"model", "kind": "load"|"unload", "keep_alive"?, "num_ctx"?}`.
    loads: Mutex<Vec<serde_json::Value>>,
}

/// Modèle téléchargé sur ce serveur.
#[derive(Clone)]
struct Pulled {
    /// Nom demandé, sans `:latest`.
    name: String,
    /// Taille : plus grosse couche des scénarios `chatonlm-pull*`, toutes les
    /// couches et la configuration pour `hf.co/chatonlm/...`.
    size: u64,
    capabilities: &'static [&'static str],
    /// Révision du registre au moment du téléchargement.
    revision: u64,
}

/// Nouvelle version d'un modèle publiée au registre.
#[derive(Clone)]
struct Published {
    /// Nom du modèle, sans `:latest`.
    name: String,
    /// 1 à la première publication, puis 2...
    revision: u64,
    /// Secondes depuis 1970.
    time: u64,
}

/// Date de publication (`ollama-push-time`) d'un modèle jamais republié :
/// avant la date des modèles installés (`modified_at`), comme un modèle
/// téléchargé après sa publication.
const REGISTRY_PUSH_TIME: u64 = 1_748_483_614;

/// `llama3:latest` et `llama3` désignent le même modèle, comme chez Ollama.
fn base_name(model: &str) -> &str {
    model.strip_suffix(":latest").unwrap_or(model)
}

/// Même modèle pour Ollama : à `:latest` et à la casse près (`getExistingName`).
fn same_model(a: &str, b: &str) -> bool {
    base_name(a).eq_ignore_ascii_case(base_name(b))
}

/// Vrai si le nom porte une étiquette (après le dernier `/`).
fn has_tag(model: &str) -> bool {
    model
        .rsplit('/')
        .next()
        .is_some_and(|last| last.contains(':'))
}

/// Noms des modèles enregistrés (`fixtures/tags.json`).
fn recorded_names() -> Vec<String> {
    let recorded: serde_json::Value = serde_json::from_str(fixtures::TAGS).expect("valid fixture");
    recorded["models"]
        .as_array()
        .expect("models")
        .iter()
        .filter_map(|model| model["name"].as_str().map(str::to_owned))
        .collect()
}

impl Journal {
    fn pulled(&self, model: &str) -> Option<Pulled> {
        let pulled = self.pulled.lock().unwrap();
        pulled.iter().find(|p| same_model(&p.name, model)).cloned()
    }

    fn is_pulled(&self, model: &str) -> bool {
        self.pulled(model).is_some()
    }

    /// Note un téléchargement réussi : le modèle passe en tête (un nom déjà
    /// installé à la casse près est remplacé, comme Ollama réutilise le sien).
    fn add_pulled(&self, model: &str, size: u64, capabilities: &'static [&'static str]) {
        let name = base_name(model).to_owned();
        let revision = self.revision(&name);
        let mut pulled = self.pulled.lock().unwrap();
        pulled.retain(|p| !same_model(&p.name, &name));
        pulled.insert(
            0,
            Pulled {
                name,
                size,
                capabilities,
                revision,
            },
        );
    }

    /// Révision de `model` au registre : 0 tant que rien n'a été publié.
    fn revision(&self, model: &str) -> u64 {
        let published = self.published.lock().unwrap();
        published
            .iter()
            .find(|p| same_model(&p.name, model))
            .map_or(0, |p| p.revision)
    }

    /// Date de publication de `model` au registre.
    fn push_time(&self, model: &str) -> u64 {
        let published = self.published.lock().unwrap();
        published
            .iter()
            .find(|p| same_model(&p.name, model))
            .map_or(REGISTRY_PUSH_TIME, |p| p.time)
    }

    /// Publie une nouvelle version de `model` au registre : son manifeste
    /// change d'empreinte et sa date de publication devient maintenant ; un
    /// modèle déjà installé garde l'ancienne jusqu'à son prochain
    /// téléchargement. Rend la nouvelle révision.
    fn publish(&self, model: &str) -> u64 {
        let name = base_name(model.trim()).to_owned();
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let mut published = self.published.lock().unwrap();
        match published.iter_mut().find(|p| same_model(&p.name, &name)) {
            Some(entry) => {
                entry.revision += 1;
                entry.time = time;
                entry.revision
            }
            None => {
                published.push(Published {
                    name,
                    revision: 1,
                    time,
                });
                1
            }
        }
    }

    fn is_deleted(&self, model: &str) -> bool {
        let deleted = self.deleted.lock().unwrap();
        deleted.iter().any(|name| same_model(name, model))
    }
}

impl FakeOllama {
    /// Démarre le serveur sur un port libre de 127.0.0.1.
    pub async fn start(mode: Mode) -> std::io::Result<Self> {
        Self::start_on(SocketAddr::from(([127, 0, 0, 1], 0)), mode).await
    }

    pub async fn start_on(addr: SocketAddr, mode: Mode) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let addr = listener.local_addr()?;
        let (shutdown, stop) = oneshot::channel::<()>();
        let journal = Arc::new(Journal::default());
        let task = match mode {
            Mode::Silent => tokio::spawn(silent(listener, stop)),
            Mode::Close => tokio::spawn(close(listener, stop)),
            mode => {
                let app = router(mode, journal.clone());
                tokio::spawn(async move {
                    let _ = axum::serve(listener, app)
                        .with_graceful_shutdown(async move {
                            let _ = stop.await;
                        })
                        .await;
                })
            }
        };
        Ok(Self {
            addr,
            shutdown: Some(shutdown),
            task,
            journal,
        })
    }

    /// Corps des requêtes `/api/chat` reçues, dans l'ordre.
    pub fn chat_requests(&self) -> Vec<serde_json::Value> {
        self.journal.chats.lock().unwrap().clone()
    }

    /// Corps des requêtes `/api/pull` reçues, dans l'ordre.
    pub fn pull_requests(&self) -> Vec<serde_json::Value> {
        self.journal.pulls.lock().unwrap().clone()
    }

    /// Noms des modèles téléchargés (`/api/pull`), dans l'ordre des requêtes.
    pub fn pulled_names(&self) -> Vec<String> {
        self.pull_requests()
            .iter()
            .map(|request| request["model"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    /// Noms reçus par `DELETE /api/delete`, dans l'ordre.
    pub fn delete_requests(&self) -> Vec<String> {
        self.journal.deletes.lock().unwrap().clone()
    }

    /// Requêtes de manifeste reçues par le registre, dans l'ordre :
    /// « HEAD /v2/library/gemma4/manifests/latest ».
    pub fn manifest_requests(&self) -> Vec<String> {
        self.journal.manifests.lock().unwrap().clone()
    }

    /// Requêtes de la recherche web reçues, dans l'ordre :
    /// `{"path": "/api/experimental/web_search", "body": {"query": …}}`.
    pub fn web_requests(&self) -> Vec<serde_json::Value> {
        self.journal.web.lock().unwrap().clone()
    }

    /// Chargements et déchargements reçus par `/api/generate`, dans l'ordre :
    /// `{"model", "kind": "load"|"unload", "keep_alive"?, "num_ctx"?}`.
    pub fn load_requests(&self) -> Vec<serde_json::Value> {
        self.journal.loads.lock().unwrap().clone()
    }

    /// Relie le compte ollama.com au serveur, ou le délie (comme
    /// `POST /fake/account`).
    pub fn set_signed_in(&self, signed_in: bool) {
        self.journal.signed_in.store(signed_in, Ordering::SeqCst);
    }

    /// `/api/me` en échec (503), ou rétabli (comme
    /// `POST /fake/account-unavailable`) ; les endpoints web suivent toujours
    /// le compte.
    pub fn set_account_unavailable(&self, unavailable: bool) {
        self.journal
            .account_unavailable
            .store(unavailable, Ordering::SeqCst);
    }

    /// Publie une nouvelle version de `model` au registre (comme
    /// `POST /fake/publish`) : le modèle installé devient périmé, jusqu'à son
    /// prochain téléchargement.
    pub fn publish(&self, model: &str) {
        self.journal.publish(model);
    }

    /// Coupe le cloud du serveur, ou le rétablit (comme `POST /fake/cloud`).
    pub fn set_cloud_disabled(&self, disabled: bool) {
        self.journal
            .cloud_disabled
            .store(disabled, Ordering::SeqCst);
    }

    /// Flux de chat ou de téléchargement, chargements lents ou requêtes web
    /// lentes, abandonnés par le client avant leur fin.
    pub fn disconnects(&self) -> usize {
        self.journal.disconnects.load(Ordering::SeqCst)
    }

    /// Adresse du serveur, sans barre finale : `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Arrête le serveur et attend la fin de sa tâche, 1 s au plus : un flux
    /// sans fin (`chatonlm-hang`, `chatonlm-stall`, `chatonlm-pull-stall`) ou une
    /// requête sans réponse (`chatonlm-wait`, `chatonlm-pull-wait`) encore
    /// ouverts empêcheraient l'arrêt en douceur.
    pub async fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if tokio::time::timeout(Duration::from_secs(1), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
        }
    }
}

impl Drop for FakeOllama {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.abort();
    }
}

/// Réponses enregistrées sur une vraie instance d'Ollama.
pub mod fixtures {
    pub const TAGS: &str = include_str!("../fixtures/tags.json");
    pub const SHOW_QWEN3: &str = include_str!("../fixtures/show-qwen3-0-6b.json");
    pub const SHOW_QWEN3_5: &str = include_str!("../fixtures/show-qwen3-5-0-8b.json");
    pub const SHOW_MISSING: &str = include_str!("../fixtures/show-missing.json");
    pub const CHAT_THINK: &str = include_str!("../fixtures/chat-think.ndjson");
    pub const CHAT_NO_THINK: &str = include_str!("../fixtures/chat-nothink.ndjson");
    pub const CHAT_MISSING: &str = include_str!("../fixtures/chat-missing.json");

    // Réponses écrites à la main (pas enregistrées), servies par `reply_lines`.
    /// Tout le Markdown que le rendu doit savoir afficher ou neutraliser, dont
    /// un bloc de code par langage coloré (les 19 de l'officiel).
    pub const MARKDOWN: &str = include_str!("../fixtures/markdown.md");
    /// Réponse courte à suivre pendant un flux lent (titre, gras, bloc de code,
    /// formule ouverte un moment). Le bloc bash contient ce que la réparation
    /// du Markdown incomplet de Streamdown abîmait dans un bloc ouvert (« $$ »,
    /// « [ », « ** »).
    pub const MARKDOWN_STREAM: &str = include_str!("../fixtures/markdown-stream.md");
    /// Un paragraphe, un bloc TypeScript d'environ 240 lignes, un paragraphe :
    /// coût de la coloration pendant un flux.
    pub const MARKDOWN_LONG: &str = include_str!("../fixtures/markdown-long.md");
}

fn json(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// Réponse d'erreur enregistrée : `{ "status": 404, "body": {...} }`.
fn recorded_error(fixture: &str) -> Response {
    let (status, body) = recorded(fixture);
    json(status, body)
}

/// Code d'état et corps d'une réponse d'erreur enregistrée.
fn recorded(fixture: &str) -> (StatusCode, String) {
    let value: serde_json::Value = serde_json::from_str(fixture).expect("valid fixture");
    let status = value["status"]
        .as_u64()
        .and_then(|code| StatusCode::from_u16(code as u16).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, value["body"].to_string())
}

/// Scénarios de `/api/chat` sans réglage du raisonnement (voir [`chat`]) :
/// leur `/api/show` répond 200 avec la seule capacité `completion`, pour
/// qu'aucun ne déclenche de téléchargement.
const CHAT_SCENARIOS: &[&str] = &[
    "chatonlm-slow",
    "chatonlm-slowthink",
    "chatonlm-longthink",
    "chatonlm-repeat",
    "chatonlm-ruminate",
    "chatonlm-context",
    "chatonlm-fast",
    "chatonlm-cut",
    "chatonlm-truncated",
    "chatonlm-reset",
    "chatonlm-error",
    "chatonlm-wait",
    "chatonlm-hang",
    "chatonlm-stall",
    "chatonlm-markdown",
    "chatonlm-markdown-slow",
    "chatonlm-markdown-long",
    "chatonlm-vanished",
    "chatonlm-polish",
    "chatonlm-polish-poem",
    "chatonlm-slowload",
    "chatonlm-longload",
    "chatonlm-loadfail",
    "chatonlm-split",
    "chatonlm-proofread",
    "chatonlm-proofread-slow",
    "chatonlm-proofread-off",
    "chatonlm-proofread-think",
    "chatonlm-proofread-opened",
    "chatonlm-study",
    "chatonlm-study-slow",
    "chatonlm-study-unclear",
    "chatonlm-study-invalid",
];

/// Scénarios de `/api/chat` qui appellent des outils (recherche web, et
/// `generate_image` pour `chatonlm-image`, voir [`chat`]) : leur `/api/show`
/// répond 200 avec les capacités `completion` et `tools`.
const TOOL_SCENARIOS: &[&str] = &[
    "chatonlm-search",
    "chatonlm-fetch",
    "chatonlm-loop",
    "chatonlm-image",
];

/// Scénarios de `/api/chat` du mode conversation vocale (voir
/// [`voice_reply`]) : leur `/api/show` répond 200 avec les capacités
/// `completion` et `thinking` et l'objet `thinking` de `qwen3:0.6b`
/// ([`voice_thinking_options`] : raisonnement par défaut, qui se coupe), pour
/// vérifier qu'un envoi vocal leur porte `think: false`.
const VOICE_SCENARIOS: &[&str] = &["chatonlm-voice", "chatonlm-voice-long"];

/// `/api/show` : réponses enregistrées pour `qwen3:0.6b` et `qwen3.5:0.8b`
/// (404 une fois supprimés) ; réponses synthétiques pour les modèles de
/// [`thinking_scenario`], ceux de [`CHAT_SCENARIOS`] et ceux de [`pull`] déjà
/// téléchargés sur ce serveur (capacité `completion` seule, plus `vision` pour
/// un `hf.co/chatonlm/...` dont le nom contient `vision`), ceux de
/// [`TOOL_SCENARIOS`] (capacités `completion` et `tools`), ceux de
/// [`VOICE_SCENARIOS`] (capacités `completion` et `thinking`), et pour
/// `chatonlm-vision` (capacités `completion` et `vision`) ; 500 pour
/// `chatonlm-nothink` (détails illisibles par le serveur) ; 404 enregistrée
/// pour tout autre nom.
async fn show(State(journal): State<Arc<Journal>>, body: String) -> Response {
    let request: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    let (status, body) = shown(&journal, request["model"].as_str().unwrap_or_default());
    json(status, body)
}

/// Réponse de [`show`] pour ce modèle : code d'état et corps.
fn shown(journal: &Journal, model: &str) -> (StatusCode, String) {
    if journal.is_deleted(model) {
        return recorded(fixtures::SHOW_MISSING);
    }
    if let Some(pulled) = journal.pulled(model) {
        return (StatusCode::OK, synthetic_show(pulled.capabilities, None));
    }
    match model {
        "qwen3:0.6b" => (StatusCode::OK, fixtures::SHOW_QWEN3.into()),
        "qwen3.5:0.8b" => (StatusCode::OK, fixtures::SHOW_QWEN3_5.into()),
        "chatonlm-vision" => (
            StatusCode::OK,
            synthetic_show(&["completion", "vision"], None),
        ),
        "chatonlm-nothink" => (
            StatusCode::INTERNAL_SERVER_ERROR,
            r#"{"error":"unable to read the model metadata"}"#.into(),
        ),
        // Modèle d'ollama.com (comme `gpt-oss:20b-cloud`), sous un nom local.
        "chatonlm-polish-remote" => {
            let mut show: serde_json::Value =
                serde_json::from_str(&synthetic_show(&["completion"], None)).expect("JSON");
            show["remote_host"] = "https://ollama.com:443".into();
            show["remote_model"] = "gpt-oss:20b".into();
            (StatusCode::OK, show.to_string())
        }
        _ => match thinking_scenario(model) {
            Some((capabilities, thinking)) => {
                (StatusCode::OK, synthetic_show(capabilities, thinking))
            }
            None if CHAT_SCENARIOS.contains(&model) => {
                (StatusCode::OK, synthetic_show(&["completion"], None))
            }
            None if TOOL_SCENARIOS.contains(&model) => (
                StatusCode::OK,
                synthetic_show(&["completion", "tools"], None),
            ),
            None if VOICE_SCENARIOS.contains(&model) => (
                StatusCode::OK,
                synthetic_show(&["completion", "thinking"], Some(voice_thinking_options())),
            ),
            None => recorded(fixtures::SHOW_MISSING),
        },
    }
}

/// Modèles de scénario du raisonnement : capacités et objet `thinking` de
/// leur `/api/show` (`None` : objet absent, comme pour un modèle sans
/// métadonnées).
/// - `chatonlm-levels` : niveaux seulement, défaut « medium » (comme gpt-oss) ;
/// - `chatonlm-levels-off` : arrêt ou niveaux, défaut « low » ;
/// - `chatonlm-always` : `[true]`, raisonnement impossible à couper ;
/// - `chatonlm-never` : capacité `thinking` mais `[false]` (ne sait pas
///   raisonner) ;
/// - `chatonlm-nometa` : capacité `thinking`, pas d'objet `thinking` ;
/// - `chatonlm-stubborn` : comme `chatonlm-nometa`, mais raisonne même avec
///   `think: false` (gabarit qui ouvre toujours le raisonnement, comme
///   LFM2.5 tiré de Hugging Face) ;
/// - `chatonlm-plain` : ni capacité `thinking` ni objet `thinking`.
fn thinking_scenario(model: &str) -> Option<(&'static [&'static str], Option<serde_json::Value>)> {
    use serde_json::json;
    const THINKING: &[&str] = &["completion", "thinking"];
    Some(match model {
        "chatonlm-levels" => (
            THINKING,
            Some(json!({ "values": ["low", "medium", "high"], "default": "medium" })),
        ),
        "chatonlm-levels-off" => (
            THINKING,
            Some(json!({ "values": [false, "low", "medium", "high"], "default": "low" })),
        ),
        "chatonlm-always" => (THINKING, Some(json!({ "values": [true], "default": true }))),
        "chatonlm-never" => (
            THINKING,
            Some(json!({ "values": [false], "default": false })),
        ),
        "chatonlm-nometa" | "chatonlm-stubborn" => (THINKING, None),
        "chatonlm-plain" => (&["completion"], None),
        _ => return None,
    })
}

/// Réponse de `/api/show` bâtie sur l'enregistrement de `qwen3:0.6b`, avec ces
/// capacités et cet objet `thinking`.
fn synthetic_show(capabilities: &[&str], thinking: Option<serde_json::Value>) -> String {
    let mut show: serde_json::Value =
        serde_json::from_str(fixtures::SHOW_QWEN3).expect("valid fixture");
    show["capabilities"] = serde_json::json!(capabilities);
    match thinking {
        Some(thinking) => show["thinking"] = thinking,
        None => {
            show.as_object_mut()
                .expect("JSON object")
                .remove("thinking");
        }
    }
    show.to_string()
}

/// `/api/chat` des modèles de [`thinking_scenario`] : flux court (morceaux de
/// 8 caractères toutes les 10 ms) avec un raisonnement si `think` ne vaut pas
/// false (jamais pour `chatonlm-never` et `chatonlm-plain`), puis une réponse qui
/// cite le `think` reçu : « Received think="high". », « Received think=false. »,
/// « Received think=absent. »... `chatonlm-plain` refuse `think` à true ou un
/// niveau comme Ollama (400 « does not support thinking ») ; ChatonLM ne le lui
/// envoie pas (capacité `thinking` absente de son `/api/show`).
fn thinking_reply(journal: Arc<Journal>, model: &str, request: &serde_json::Value) -> Response {
    let think = &request["think"];
    if model == "chatonlm-plain"
        && matches!(
            think,
            serde_json::Value::Bool(true) | serde_json::Value::String(_)
        )
    {
        return json(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": format!("\"{model}\" does not support thinking") })
                .to_string(),
        );
    }
    let received = match request.get("think") {
        Some(value) => value.to_string(),
        None => "absent".to_owned(),
    };
    let thinks = model == "chatonlm-stubborn"
        || (!matches!(model, "chatonlm-never" | "chatonlm-plain")
            && *think != serde_json::Value::Bool(false));
    let thinking = if thinks {
        format!("The user wants a short answer. Thinking with think={received}.")
    } else {
        String::new()
    };
    ndjson(
        journal,
        reasoned_reply_lines(model, &thinking, &format!("Received think={received}."), 8),
        Duration::from_millis(10),
        End::Close,
    )
}

/// Objet `thinking` du `/api/show` des [`VOICE_SCENARIOS`] : celui de
/// `qwen3:0.6b` (raisonnement par défaut, qui peut être coupé).
fn voice_thinking_options() -> serde_json::Value {
    serde_json::json!({ "values": [false, true], "default": true })
}

/// Réponses de `chatonlm-voice` : la n-ième pour le n-ième message `user` de
/// la requête (puis de nouveau la première...). Deux phrases françaises,
/// lues par la voix française : la première courte (un tour vite fini), la
/// seconde lue pendant plusieurs secondes (le temps de la couper par Espace).
pub const VOICE_REPLIES: [&str; 2] = [
    "Je vous entends très bien. Que puis-je faire pour vous ?",
    "C'est une belle histoire, tirée des Mille et Une Nuits. Voulez-vous que je vous \
     raconte ce que devient ensuite le pêcheur ?",
];

/// Réponse de `chatonlm-voice-long`, la même à chaque tour : assez longue pour
/// être lue pendant plusieurs dizaines de secondes, écrite pendant plusieurs
/// secondes ([`VOICE_LONG_DELAY`]) ; sa première phrase arrive en moins d'une
/// seconde, et sa dernière, [`VOICE_LONG_END`], montre que la réponse est
/// complète (un envoi coupé ne l'a pas).
pub const VOICE_LONG_REPLY: &str = "Voici une réponse plus longue, qui laisse le temps de me \
couper la parole. Un pêcheur jetait son filet quatre fois par jour, pas une de plus. Un matin, \
il remonta un vase de cuivre fermé par un sceau de plomb. Curieux, il ouvrit le vase avec son \
couteau, et une épaisse fumée s'en échappa. La fumée prit la forme d'un génie immense, qui \
menaça aussitôt de le tuer. Le pêcheur lui demanda comment un si grand génie pouvait tenir dans \
un si petit vase. Piqué au vif, le génie redevint fumée et rentra dans le vase pour le lui \
prouver. Le pêcheur referma le vase et le rejeta à la mer. Fin de l'histoire.";

/// Dernière phrase de [`VOICE_LONG_REPLY`].
pub const VOICE_LONG_END: &str = "Fin de l'histoire.";

/// Délai entre deux morceaux (8 caractères) de `chatonlm-voice-long`.
pub const VOICE_LONG_DELAY: Duration = Duration::from_millis(100);

/// Raisonnement des [`VOICE_SCENARIOS`] quand `think` ne vaut pas false.
pub const VOICE_THINKING: &str = "On me parle à voix haute : je réponds brièvement.";

/// `/api/chat` du mode conversation vocale ([`VOICE_SCENARIOS`]), en
/// morceaux de 8 caractères :
/// - `chatonlm-voice` : une phrase de [`VOICE_REPLIES`] selon le tour, toutes
///   les 20 ms ;
/// - `chatonlm-voice-long` : [`VOICE_LONG_REPLY`], toutes les 100 ms (environ
///   8 s : la voix peut être coupée pendant que le texte s'écrit encore).
///
/// Comme un modèle qui raisonne par défaut, [`VOICE_THINKING`] précède la
/// réponse si `think` ne vaut pas false (absent compris).
fn voice_reply(journal: Arc<Journal>, model: &str, request: &serde_json::Value) -> Response {
    let thinking = if request["think"] == serde_json::Value::Bool(false) {
        ""
    } else {
        VOICE_THINKING
    };
    let (text, delay) = match model {
        "chatonlm-voice-long" => (VOICE_LONG_REPLY, VOICE_LONG_DELAY),
        _ => {
            let users = request["messages"].as_array().map_or(0, |messages| {
                messages
                    .iter()
                    .filter(|message| message["role"] == "user")
                    .count()
            });
            let turn = users.saturating_sub(1) % VOICE_REPLIES.len();
            (VOICE_REPLIES[turn], Duration::from_millis(20))
        }
    };
    ndjson(
        journal,
        reasoned_reply_lines(model, thinking, text, 8),
        delay,
        End::Close,
    )
}

/// Scénarios de `/api/chat`, choisis par le nom du modèle :
/// - `qwen3:0.6b`, `qwen3.5:0.8b` : flux enregistré, avec raisonnement sauf si
///   `think` vaut false (comme Ollama, un modèle capable de raisonner raisonne
///   quand `think` est absent) ;
/// - `chatonlm-slow` : flux sans raisonnement, une ligne toutes les 200 ms ;
/// - `chatonlm-slowthink` : flux avec raisonnement, une ligne toutes les 50 ms ;
/// - `chatonlm-longthink` : long raisonnement en paragraphes numérotés (voir
///   [`long_thinking`]), 24 caractères toutes les 40 ms (environ 5 s), puis
///   « Done thinking. » : il déborde de l'aperçu replié de l'app ;
/// - `chatonlm-fast` : le même flux, une ligne toutes les 10 ms (un rendu par
///   image dans l'app, pendant environ 2 s) ;
/// - `chatonlm-cut` : trois lignes puis fin sans `done` ;
/// - `chatonlm-truncated` : deux lignes, la moitié de la troisième (sans saut de
///   ligne) puis fin normale du corps ;
/// - `chatonlm-reset` : deux lignes puis connexion coupée au milieu du corps ;
/// - `chatonlm-error` : deux lignes puis une ligne `error` ;
/// - `chatonlm-nothink` : 400 « does not support thinking » si `think` vaut
///   true ou un niveau, flux sans raisonnement sinon ; son `/api/show` échoue
///   (500) : ChatonLM envoie alors `think` sans filtre, et le refus le fait
///   retirer ;
/// - `chatonlm-wait` : aucune réponse, pas même les en-têtes (comme Ollama
///   pendant le chargement du modèle) ;
/// - `chatonlm-repeat` : raisonnement qui finit par répéter mot pour mot le
///   même paragraphe (voir [`repeating_thinking`]), puis seulement des lignes
///   vides, jamais de fin (ChatonLM doit l'arrêter de lui-même) ;
/// - `chatonlm-ruminate` : raisonnement qui ressasse sans se répéter mot pour
///   mot (voir [`ruminating_thinking`]), environ 8 s, puis seulement des
///   lignes vides, jamais de fin ;
/// - `chatonlm-context` : contexte consommé (voir [`context_reply`]) : jetons
///   comptés (4 caractères par jeton), `num_predict` respecté
///   (`done_reason: "length"`), résumé si on le lui demande ;
/// - `chatonlm-hang` : en-têtes envoyés, puis seulement des lignes vides (toutes
///   les 100 ms, pour remarquer un client parti), jamais de réponse ;
/// - `chatonlm-stall` : deux lignes de réponse, puis seulement des lignes vides
///   (toutes les 100 ms), jamais de fin ;
/// - `chatonlm-markdown` : `fixtures/markdown.md` sans raisonnement, en morceaux
///   de 16 caractères, sans délai ;
/// - `chatonlm-markdown-slow` : `fixtures/markdown-stream.md`, 6 caractères
///   toutes les 100 ms (environ 8 s) ;
/// - `chatonlm-markdown-long` : `fixtures/markdown-long.md`, 24 caractères toutes
///   les 15 ms (environ 5 s) ;
/// - `chatonlm-levels`, `chatonlm-levels-off`, `chatonlm-always`, `chatonlm-never`,
///   `chatonlm-nometa`, `chatonlm-stubborn`, `chatonlm-plain` : réglages du raisonnement décrits par
///   `/api/show` (voir [`thinking_scenario`]), flux court qui cite le `think`
///   reçu (voir [`thinking_reply`]) ; `chatonlm-plain` n'ayant pas la capacité
///   `thinking`, ChatonLM ne lui envoie jamais `think` ;
/// - `chatonlm-vanished` : `/api/show` répond 200 mais `/api/chat` 404
///   enregistrée (modèle supprimé entre les deux) ;
/// - `chatonlm-vision` : le flux enregistré sans raisonnement (celui des
///   modèles installés quand `think` vaut false) ; son `/api/show` donne les
///   capacités `completion` et `vision` : l'interface lui laisse les images
///   jointes (un autre scénario `chatonlm-*` n'a pas la vision) ;
/// - `chatonlm-search`, `chatonlm-fetch`, `chatonlm-loop` : recherche web (voir
///   [`tool_reply`]) ; leur `/api/show` donne les capacités `completion` et
///   `tools` ;
/// - `chatonlm-image` : génération d'images de ChatonLM, outil
///   `generate_image` (voir [`tool_reply`]) ; mêmes capacités ;
/// - `chatonlm-polish`, `chatonlm-polish-poem`, `chatonlm-polish-remote` :
///   nettoyage de la dictée (voir [`polish_reply`]) ; le `/api/show` du
///   dernier décrit un modèle d'ollama.com (`remote_host`, `remote_model`),
///   que ChatonLM ne doit jamais appeler pour la dictée et qui n'est jamais en
///   mémoire ici ;
/// - `chatonlm-slowload`, `chatonlm-longload` : chargement de [`SLOW_LOAD`]
///   ou [`LONG_LOAD`] avant la réponse s'il n'est pas en mémoire avec ce
///   contexte (aussi par `/api/generate`, voir [`generate`]), puis le flux de
///   `chatonlm-fast` ;
/// - `chatonlm-loadfail` : 500 [`LOAD_FAILURE`] (mémoire insuffisante), comme
///   son chargement par `/api/generate` ;
/// - `chatonlm-split` : le flux enregistré sans raisonnement ; en mémoire, 25 %
///   sur le CPU (voir [`ps`]) ;
/// - `chatonlm-proofread`, `chatonlm-proofread-slow`, `chatonlm-proofread-off`,
///   `chatonlm-proofread-think` : correction d'un texte (voir
///   [`proofread_reply`]) ;
/// - `chatonlm-study`, `chatonlm-study-slow`, `chatonlm-study-unclear`,
///   `chatonlm-study-invalid` : parties des cours de ChatonLM (plan, leçon,
///   questions, vérification, en JSON selon le schéma `format` ; flux
///   Markdown sans `format`), voir [`study`] ;
/// - `chatonlm-voice`, `chatonlm-voice-long` : mode conversation vocale, réponses
///   françaises (deux phrases courtes selon le tour, ou une longue réponse
///   écrite en 8 s environ), raisonnement si `think` ne vaut pas false (voir
///   [`voice_reply`]) ; leur `/api/show` donne les capacités `completion` et
///   `thinking` ;
/// - scénarios de téléchargement (`chatonlm-pull`... : voir [`pull`]) : 404
///   enregistrée tant que le modèle n'est pas téléchargé sur ce serveur, puis
///   le flux enregistré sans raisonnement ;
/// - tout autre nom : 404 enregistrée (modèle absent).
///
/// Un modèle qui répond est mis en mémoire comme par `/api/generate` (voir
/// [`load_request`]) : contexte `options.num_ctx`, `keep_alive`.
///
/// Aucun scénario `chatonlm-*` ne figure dans `/api/tags` (sauf ceux téléchargés
/// sur ce serveur) : un test les choisit par le réglage `selectedModel`. Leur `/api/show` répond 200 (capacité
/// `completion` seule, sauf ceux du raisonnement, ceux de la recherche web, ceux
/// du mode vocal et `chatonlm-vision` : aucun bouton Think dans l'interface pour
/// les autres),
/// sauf pour
/// `chatonlm-nothink` et les scénarios de téléchargement. Le champ `think` est
/// ignoré par les scénarios Markdown.
///
/// Chaque corps reçu est journalisé tel quel, quel que soit le modèle, et se
/// relit sur `GET /fake/requests` (tableau JSON, dans l'ordre) : historique
/// envoyé, texte des fichiers joints ajouté au `content` d'un message
/// utilisateur, images en base64 dans son champ `images`, `think`...
async fn chat(State(journal): State<Arc<Journal>>, body: String) -> Response {
    let request: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    journal.chats.lock().unwrap().push(request.clone());
    let model = request["model"].as_str().unwrap_or_default();
    if model == "chatonlm-loadfail" {
        return load_failure();
    }
    if loadable(&journal, model) {
        let load = load_request(&journal, model, &request);
        if let Some(delay) = load_delay(model).filter(|_| load.needed) {
            let mut guard = DisconnectGuard::new(journal.clone());
            tokio::time::sleep(delay).await;
            guard.finish();
        }
        apply_load(&journal, model, &load);
    }
    let thinks = request["think"] != serde_json::Value::Bool(false);
    let think_requested = matches!(
        request["think"],
        serde_json::Value::Bool(true) | serde_json::Value::String(_)
    );
    let quick = Duration::ZERO;
    let answer = || lines(fixtures::CHAT_NO_THINK);
    let first = |count: usize| {
        let mut lines = answer();
        lines.truncate(count);
        lines
    };
    match request["model"].as_str().unwrap_or_default() {
        "qwen3:0.6b" | "qwen3.5:0.8b" if thinks => {
            ndjson(journal, lines(fixtures::CHAT_THINK), quick, End::Close)
        }
        "qwen3:0.6b" | "qwen3.5:0.8b" => ndjson(journal, answer(), quick, End::Close),
        "chatonlm-slow" => ndjson(journal, answer(), Duration::from_millis(200), End::Close),
        "chatonlm-slowthink" => ndjson(
            journal,
            lines(fixtures::CHAT_THINK),
            Duration::from_millis(50),
            End::Close,
        ),
        model @ "chatonlm-longthink" => ndjson(
            journal,
            reasoned_reply_lines(model, &long_thinking(), "Done thinking.", 24),
            Duration::from_millis(40),
            End::Close,
        ),
        "chatonlm-fast" | "chatonlm-slowload" | "chatonlm-longload" => ndjson(
            journal,
            lines(fixtures::CHAT_THINK),
            Duration::from_millis(10),
            End::Close,
        ),
        "chatonlm-cut" => ndjson(journal, first(3), quick, End::Close),
        "chatonlm-truncated" => {
            let mut truncated = first(2);
            let third = answer().swap_remove(2);
            let half = third.chars().count() / 2;
            truncated.push(third.chars().take(half).collect());
            ndjson(journal, truncated, quick, End::Close)
        }
        "chatonlm-reset" => ndjson(journal, first(2), quick, End::Reset),
        "chatonlm-error" => {
            let mut failing = first(2);
            failing.push("{\"error\":\"model runner has unexpectedly stopped\"}\n".into());
            ndjson(journal, failing, quick, End::Close)
        }
        "chatonlm-nothink" if think_requested => json(
            StatusCode::BAD_REQUEST,
            r#"{"error":"\"chatonlm-nothink\" does not support thinking"}"#.into(),
        ),
        "chatonlm-nothink" | "chatonlm-vision" | "chatonlm-split" => {
            ndjson(journal, answer(), quick, End::Close)
        }
        "chatonlm-wait" => wait(journal).await,
        model @ "chatonlm-repeat" => {
            let mut thinking = reasoned_reply_lines(model, &repeating_thinking(), "", 24);
            // Sans la ligne `done` : le modèle ne s'arrête jamais.
            thinking.pop();
            ndjson(journal, thinking, Duration::from_millis(2), End::Stall)
        }
        model @ "chatonlm-ruminate" => {
            let mut thinking = reasoned_reply_lines(model, &ruminating_thinking(), "", 96);
            thinking.pop();
            ndjson(journal, thinking, Duration::from_millis(15), End::Stall)
        }
        model @ "chatonlm-context" => {
            ndjson(journal, context_reply(model, &request), quick, End::Close)
        }
        "chatonlm-hang" => ndjson(journal, Vec::new(), quick, End::Stall),
        "chatonlm-stall" => ndjson(journal, first(2), quick, End::Stall),
        model @ "chatonlm-markdown" => ndjson(
            journal,
            reply_lines(model, fixtures::MARKDOWN, 16),
            quick,
            End::Close,
        ),
        model @ "chatonlm-markdown-slow" => ndjson(
            journal,
            reply_lines(model, fixtures::MARKDOWN_STREAM, 6),
            Duration::from_millis(100),
            End::Close,
        ),
        model @ "chatonlm-markdown-long" => ndjson(
            journal,
            reply_lines(model, fixtures::MARKDOWN_LONG, 24),
            Duration::from_millis(15),
            End::Close,
        ),
        model @ ("chatonlm-polish" | "chatonlm-polish-poem" | "chatonlm-polish-remote") => ndjson(
            journal,
            reply_lines(model, &polish_reply(model, &request), 8),
            Duration::from_millis(10),
            End::Close,
        ),
        model @ ("chatonlm-proofread"
        | "chatonlm-proofread-slow"
        | "chatonlm-proofread-off"
        | "chatonlm-proofread-think"
        | "chatonlm-proofread-opened") => proofread_reply(journal, model, &request),
        model if study::STUDY_MODELS.contains(&model) => {
            study::reply(journal, model, &request).await
        }
        model if VOICE_SCENARIOS.contains(&model) => voice_reply(journal, model, &request),
        model if thinking_scenario(model).is_some() => thinking_reply(journal, model, &request),
        model if TOOL_SCENARIOS.contains(&model) => ndjson(
            journal,
            tool_reply(model, &request),
            Duration::from_millis(10),
            End::Close,
        ),
        model if journal.is_pulled(model) => ndjson(journal, answer(), quick, End::Close),
        _ => recorded_error(fixtures::CHAT_MISSING),
    }
}

/// Mémoire occupée par un modèle chargé (`/api/ps`, `size`) : 1 Gio.
pub const LOADED_SIZE: u64 = 1_073_741_824;

/// Contexte d'un chargement sans `options.num_ctx`.
pub const DEFAULT_CONTEXT: u64 = 4096;

/// Durée en mémoire sans `keep_alive` (défaut d'Ollama : 5 minutes).
const DEFAULT_KEEP_ALIVE: i64 = 300;

/// Durée du chargement de `chatonlm-slowload`.
pub const SLOW_LOAD: Duration = Duration::from_millis(1500);

/// Durée du chargement de `chatonlm-longload` (le temps d'arrêter un envoi
/// pendant le chargement, dans un e2e).
pub const LONG_LOAD: Duration = Duration::from_secs(6);

/// Durée du chargement de ce modèle, s'il est lent.
fn load_delay(model: &str) -> Option<Duration> {
    match base_name(model) {
        "chatonlm-slowload" => Some(SLOW_LOAD),
        "chatonlm-longload" => Some(LONG_LOAD),
        _ => None,
    }
}

/// Échec du chargement de `chatonlm-loadfail` (message d'Ollama, HTTP 500).
pub const LOAD_FAILURE: &str =
    "model requires more system memory (64.0 GiB) than is available (8.0 GiB)";

/// Modèle en mémoire.
#[derive(Clone)]
struct Loaded {
    /// Nom complet, comme `/api/ps` : `:latest` ajouté au nom sans étiquette.
    name: String,
    context: u64,
    /// Échéance du déchargement, en nanosecondes depuis l'époque.
    expires: i128,
}

/// Nom complet d'un modèle, comme `/api/ps` le donne.
fn full_name(model: &str) -> String {
    if has_tag(model) {
        model.to_owned()
    } else {
        format!("{model}:latest")
    }
}

/// Modèles que `/api/chat` sert (installés ou scénarios), à `:latest` près
/// (le nom que donne `/api/ps`) : chargeables. Pas `chatonlm-vanished`
/// (supprimé avant le chat), ni un modèle supprimé, ni le modèle distant
/// `chatonlm-polish-remote` (en mémoire ailleurs).
fn loadable(journal: &Journal, model: &str) -> bool {
    let model = base_name(model);
    if journal.is_deleted(model) {
        return false;
    }
    journal.is_pulled(model)
        || matches!(
            model,
            "qwen3:0.6b" | "qwen3.5:0.8b" | "chatonlm-nothink" | "chatonlm-vision"
        )
        || (CHAT_SCENARIOS.contains(&model) && model != "chatonlm-vanished")
        || TOOL_SCENARIOS.contains(&model)
        || thinking_scenario(model).is_some()
        || VOICE_SCENARIOS.contains(&model)
}

/// Contexte maximal du modèle d'après son `/api/show`
/// (`<general.architecture>.context_length` de `model_info`).
fn max_context(journal: &Journal, model: &str) -> Option<u64> {
    let (status, body) = shown(journal, base_name(model));
    if status != StatusCode::OK {
        return None;
    }
    let show: serde_json::Value = serde_json::from_str(&body).ok()?;
    let info = &show["model_info"];
    let architecture = info["general.architecture"].as_str()?;
    info[format!("{architecture}.context_length")].as_u64()
}

/// `keep_alive` d'une requête, en secondes ; `None` s'il est absent (une
/// durée écrite, « 5m », compte comme absente : ChatonLM envoie des nombres).
fn keep_alive(request: &serde_json::Value) -> Option<i64> {
    let value = request.get("keep_alive")?;
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|seconds| seconds as i64))
}

/// Chargement demandé par une requête (`/api/generate` ou `/api/chat`).
struct LoadRequest {
    /// Contexte retenu : `options.num_ctx` (sinon [`DEFAULT_CONTEXT`]), ramené
    /// au maximum du modèle s'il le dépasse, comme Ollama.
    context: u64,
    /// `keep_alive` reçu, en secondes.
    keep_alive: Option<i64>,
    /// Pas en mémoire avec ce contexte : (re)chargement.
    needed: bool,
}

fn load_request(journal: &Journal, model: &str, request: &serde_json::Value) -> LoadRequest {
    let asked = request["options"]["num_ctx"]
        .as_u64()
        .filter(|&context| context > 0)
        .unwrap_or(DEFAULT_CONTEXT);
    let context = max_context(journal, model).map_or(asked, |max| asked.min(max));
    let loaded = journal.loaded.lock().unwrap();
    let needed = !loaded
        .iter()
        .any(|l| same_model(&l.name, model) && l.context == context);
    LoadRequest {
        context,
        keep_alive: keep_alive(request),
        needed,
    }
}

/// Heure courante, en nanosecondes depuis l'époque.
fn now_nanos() -> i128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as i128)
}

/// Met en mémoire comme Ollama : `keep_alive` 0 décharge ; un modèle chargé
/// (ou rechargé) expire après `keep_alive` secondes (5 minutes sans, dans
/// 292 ans s'il est négatif) ; déjà chargé avec ce contexte, il garde son
/// échéance sans `keep_alive`.
fn apply_load(journal: &Journal, model: &str, load: &LoadRequest) {
    let mut loaded = journal.loaded.lock().unwrap();
    if load.keep_alive == Some(0) {
        loaded.retain(|l| !same_model(&l.name, model));
        return;
    }
    if !load.needed && load.keep_alive.is_none() {
        return;
    }
    let seconds = load.keep_alive.unwrap_or(DEFAULT_KEEP_ALIVE);
    let expires = if seconds < 0 {
        // `time.Duration(math.MaxInt64)` d'Ollama.
        now_nanos() + i128::from(i64::MAX)
    } else {
        now_nanos() + i128::from(seconds) * 1_000_000_000
    };
    loaded.retain(|l| !same_model(&l.name, model));
    loaded.push(Loaded {
        name: full_name(model),
        context: load.context,
        expires,
    });
}

/// 500 de `chatonlm-loadfail`, chargement comme chat.
fn load_failure() -> Response {
    json(
        StatusCode::INTERNAL_SERVER_ERROR,
        serde_json::json!({ "error": LOAD_FAILURE }).to_string(),
    )
}

/// `POST /api/generate` sans prompt (avec un prompt non vide : 400, le faux
/// serveur ne génère pas de texte), comme Ollama :
/// - `keep_alive: 0` : décharge → 200 `"done_reason": "unload"` ;
/// - sinon charge avec `options.num_ctx` (voir [`load_request`]) et
///   `keep_alive` → 200 `"done_reason": "load"` une fois chargé :
///   [`SLOW_LOAD`] pour `chatonlm-slowload`, [`LONG_LOAD`] pour
///   `chatonlm-longload`, s'il n'est pas en mémoire avec ce contexte (un client
///   parti avant compte comme un abandon), 500 [`LOAD_FAILURE`] pour
///   `chatonlm-loadfail` ;
/// - `chatonlm-polish-remote` (distant) : 200, jamais en mémoire ici ;
/// - modèle que `/api/chat` ne sert pas : 404 `model '<nom reçu>' not
///   found`, comme Ollama.
///
/// Chaque demande sans prompt est journalisée (`GET /fake/loads`), pas dans
/// `/fake/requests` (des e2e comptent ses entrées).
async fn generate(State(journal): State<Arc<Journal>>, body: String) -> Response {
    let request: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    let model = request["model"].as_str().unwrap_or_default().to_owned();
    if request["prompt"]
        .as_str()
        .is_some_and(|prompt| !prompt.is_empty())
    {
        return json(
            StatusCode::BAD_REQUEST,
            r#"{"error":"the fake server only loads and unloads models: no prompt"}"#.into(),
        );
    }
    let load = load_request(&journal, &model, &request);
    let unload = load.keep_alive == Some(0);
    let kind = if unload { "unload" } else { "load" };
    let mut entry = serde_json::json!({ "model": model, "kind": kind });
    if let Some(seconds) = load.keep_alive {
        entry["keep_alive"] = seconds.into();
    }
    if let Some(context) = request["options"]["num_ctx"].as_u64() {
        entry["num_ctx"] = context.into();
    }
    journal.loads.lock().unwrap().push(entry);
    let remote = base_name(&model) == "chatonlm-polish-remote";
    if !remote && !loadable(&journal, &model) {
        return json(
            StatusCode::NOT_FOUND,
            serde_json::json!({ "error": format!("model '{model}' not found") }).to_string(),
        );
    }
    if !unload && model == "chatonlm-loadfail" {
        return load_failure();
    }
    if let Some(delay) = load_delay(&model).filter(|_| !unload && load.needed) {
        let mut guard = DisconnectGuard::new(journal.clone());
        tokio::time::sleep(delay).await;
        guard.finish();
    }
    if !remote {
        apply_load(&journal, &model, &load);
    }
    let done = serde_json::json!({
        "model": model,
        "created_at": date_text(now_nanos(), 0),
        "response": "",
        "done": true,
        "done_reason": kind,
    });
    json(StatusCode::OK, done.to_string())
}

/// `GET /api/ps`, au format d'Ollama : modèles en mémoire, de l'échéance la
/// plus lointaine à la plus proche ; [`LOADED_SIZE`] chacun, tout en VRAM
/// sauf un modèle dont le nom contient `split` (75 %) ; détails de son
/// `/api/show` ; échéance avec 9 chiffres de fraction et le décalage `+02:00`.
fn ps(journal: &Journal) -> String {
    let mut loaded = journal.loaded.lock().unwrap().clone();
    loaded.sort_by_key(|l| std::cmp::Reverse(l.expires));
    let models: Vec<serde_json::Value> = loaded
        .iter()
        .map(|l| {
            let (status, body) = shown(journal, base_name(&l.name));
            let details = (status == StatusCode::OK)
                .then(|| serde_json::from_str::<serde_json::Value>(&body).ok())
                .flatten()
                .map_or_else(|| serde_json::json!({}), |show| show["details"].clone());
            let vram = if l.name.contains("split") {
                LOADED_SIZE / 4 * 3
            } else {
                LOADED_SIZE
            };
            serde_json::json!({
                "name": l.name,
                "model": l.name,
                "size": LOADED_SIZE,
                "digest": fake_digest(&l.name),
                "details": details,
                "expires_at": date_text(l.expires, 120),
                "size_vram": vram,
                "context_length": l.context,
            })
        })
        .collect();
    serde_json::json!({ "models": models }).to_string()
}

/// Date RFC 3339 avec 9 chiffres de fraction, au décalage donné (minutes ;
/// 0 : « Z »), comme Go l'écrit : « 2026-10-06T14:52:29.998419079+02:00 ».
fn date_text(nanos: i128, offset_minutes: i64) -> String {
    let local = nanos + i128::from(offset_minutes) * 60_000_000_000;
    let seconds = local.div_euclid(1_000_000_000);
    let fraction = local.rem_euclid(1_000_000_000);
    let days = i64::try_from(seconds.div_euclid(86_400)).unwrap_or_default();
    let time = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (time / 3_600, time % 3_600 / 60, time % 60);
    let zone = if offset_minutes == 0 {
        "Z".to_owned()
    } else {
        let sign = if offset_minutes < 0 { '-' } else { '+' };
        let minutes = offset_minutes.abs();
        format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60)
    };
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{fraction:09}{zone}")
}

/// Date d'un jour compté depuis le 1er janvier 1970 (algorithme de Howard
/// Hinnant, `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Taille de la grosse couche des scénarios de téléchargement (« 4.7 GB »).
pub const PULL_TOTAL: u64 = 5_046_586_573;

/// Taille de la petite couche de `chatonlm-pull` (gabarit, licence...).
pub const PULL_SMALL_LAYER: u64 = 487;

/// Couche unique de `chatonlm-pull-small` (≤ 1 024 octets : l'interface ne
/// montre pas de progression).
const PULL_TINY_LAYER: u64 = 1000;

/// Empreintes des couches des scénarios de téléchargement (inventées).
const BIG_LAYER: &str = "sha256:4f6c1a8e2b7d93c05e1f8a2b6c4d7e9f0a3b5c8d1e2f4a6b7c9d0e1f2a3b4c5d";
const SMALL_LAYER: &str = "sha256:9b2e4d6f8a0c1e3b5d7f9a1c3e5b7d9f0b2d4f6a8c0e1b3d5f7a9c1e3b5d7f9a";

/// Texte d'erreur d'Ollama pour un modèle absent du registre.
const PULL_MISSING: &str = "pull model manifest: file does not exist";

/// Ligne de progression d'une couche, comme Ollama (`completed` absent tant
/// qu'il vaut 0, statut « pulling » suivi des 12 premiers chiffres du digest).
fn layer_line(digest: &str, total: u64, completed: u64) -> String {
    let mut line = serde_json::json!({
        "status": format!("pulling {}", &digest[7..19]),
        "digest": digest,
        "total": total,
    });
    if completed > 0 {
        line["completed"] = completed.into();
    }
    format!("{line}\n")
}

fn status_line(status: &str) -> String {
    format!("{}\n", serde_json::json!({ "status": status }))
}

fn error_line(error: &str) -> String {
    format!("{}\n", serde_json::json!({ "error": error }))
}

/// Progression de la grosse couche en `steps` pas : `steps + 1` lignes, de 0
/// à `PULL_TOTAL`.
fn big_layer_lines(steps: u64) -> Vec<String> {
    (0..=steps)
        .map(|step| layer_line(BIG_LAYER, PULL_TOTAL, PULL_TOTAL * step / steps))
        .collect()
}

/// Flux complet de `chatonlm-pull` (grosse couche en `steps` pas).
fn full_pull_lines(steps: u64) -> Vec<String> {
    let mut lines = vec![status_line("pulling manifest")];
    lines.extend(big_layer_lines(steps));
    lines.push(layer_line(SMALL_LAYER, PULL_SMALL_LAYER, PULL_SMALL_LAYER));
    lines.extend(["verifying sha256 digest", "writing manifest", "success"].map(status_line));
    lines
}

/// `/api/pull`, choisi par le nom du modèle (corps journalisé) :
/// - `chatonlm-pull` : `pulling manifest`, grosse couche de [`PULL_TOTAL`]
///   octets de 0 à complète en 11 lignes, petite couche complète (une ligne),
///   `verifying sha256 digest`, `writing manifest`, `success` ; une ligne
///   toutes les 30 ms ;
/// - `chatonlm-pull-slow` : même forme, 50 lignes toutes les 100 ms (environ
///   5 s) ;
/// - `chatonlm-pull-small` : une seule couche de [`PULL_TINY_LAYER`] octets, puis
///   la fin normale ;
/// - `chatonlm-pull-stall` : manifeste, 3 lignes vers 25 %, puis seulement des
///   lignes vides (toutes les 100 ms), jamais de fin ;
/// - `chatonlm-pull-cut` : manifeste, 3 lignes de progression, fin du corps sans
///   `success` ;
/// - `chatonlm-pull-reset` : manifeste, 3 lignes, puis connexion coupée ;
/// - `chatonlm-pull-error` : manifeste, puis `{"error": "pull model manifest:
///   file does not exist"}` ;
/// - `chatonlm-pull-offline` : manifeste, puis l'erreur d'un registre
///   injoignable (« dial tcp: lookup registry.ollama.ai: no such host ») ;
/// - `chatonlm-pull-refused` : HTTP 400 « invalid model name » ;
/// - `chatonlm-pull-wait` : aucune réponse, pas même les en-têtes ;
/// - `hf.co/chatonlm/<dépôt>:<étiquette>` : modèle de Hugging Face, voir
///   [`hf_pull`] ;
/// - tout autre nom : manifeste, puis l'erreur « file does not exist », comme
///   Ollama pour un nom absent du registre.
///
/// `chatonlm-pull`, `chatonlm-pull-slow` et `chatonlm-pull-small` comptent comme
/// téléchargés sur ce serveur dès l'envoi de `success` : `/api/tags` les montre
/// en tête (`<nom>:latest`, taille de leur plus grosse couche, capacité
/// `completion`), `/api/show` répond 200 (capacité `completion`) et
/// `/api/chat` le flux enregistré sans raisonnement. Un téléchargement
/// interrompu ne compte pas.
async fn pull(State(journal): State<Arc<Journal>>, body: String) -> Response {
    let request: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    journal.pulls.lock().unwrap().push(request.clone());
    let model = request["model"].as_str().unwrap_or_default().to_owned();
    let delay = Duration::from_millis(30);
    let manifest = || vec![status_line("pulling manifest")];
    let failing = |error: &str| {
        let mut lines = manifest();
        lines.push(error_line(error));
        lines
    };
    let started = |end: End| {
        let mut lines = manifest();
        lines.extend(big_layer_lines(10).into_iter().take(3));
        ndjson(journal.clone(), lines, delay, end)
    };
    let pulled = |lines: Vec<String>, delay: Duration, size: u64| {
        let (state, name) = (journal.clone(), model.clone());
        ndjson_then(journal.clone(), lines, delay, End::Close, move || {
            state.add_pulled(&name, size, &["completion"]);
        })
    };
    if let Some((repo, tag)) = hf_chatonlm(&model) {
        return hf_pull(journal.clone(), &model, &repo, &tag);
    }
    // `<nom>:latest` est le même modèle, comme chez Ollama.
    match base_name(&model) {
        "chatonlm-pull" => pulled(full_pull_lines(10), delay, PULL_TOTAL),
        "chatonlm-pull-slow" => pulled(full_pull_lines(44), Duration::from_millis(100), PULL_TOTAL),
        "chatonlm-pull-small" => {
            let mut lines = manifest();
            lines.push(layer_line(SMALL_LAYER, PULL_TINY_LAYER, 0));
            lines.push(layer_line(SMALL_LAYER, PULL_TINY_LAYER, PULL_TINY_LAYER));
            lines.extend(
                ["verifying sha256 digest", "writing manifest", "success"].map(status_line),
            );
            pulled(lines, delay, PULL_TINY_LAYER)
        }
        "chatonlm-pull-stall" => {
            let mut lines = manifest();
            lines.extend(
                [23, 24, 25]
                    .map(|percent| layer_line(BIG_LAYER, PULL_TOTAL, PULL_TOTAL * percent / 100)),
            );
            ndjson(journal, lines, delay, End::Stall)
        }
        "chatonlm-pull-cut" => started(End::Close),
        "chatonlm-pull-reset" => started(End::Reset),
        "chatonlm-pull-offline" => ndjson(
            journal,
            failing(
                "pull model manifest: Get \"https://registry.ollama.ai/v2/library/chatonlm-pull-offline/manifests/latest\": dial tcp: lookup registry.ollama.ai: no such host",
            ),
            delay,
            End::Close,
        ),
        "chatonlm-pull-refused" => json(
            StatusCode::BAD_REQUEST,
            r#"{"error":"invalid model name"}"#.into(),
        ),
        "chatonlm-pull-wait" => wait(journal).await,
        _ => ndjson(journal, failing(PULL_MISSING), delay, End::Close),
    }
}

/// Couche modèle des `hf.co/chatonlm/...` (« 1.2 GB »).
pub const HF_MODEL_LAYER: u64 = 1_200_000_000;

/// Couche projecteur des `hf.co/chatonlm/...` de vision.
pub const HF_PROJECTOR_LAYER: u64 = 400_000_000;

/// Gabarit Go, paramètres et configuration générés par Hugging Face (tailles
/// de `hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M`).
const HF_TEMPLATE_LAYER: u64 = 675;
const HF_PARAMS_LAYER: u64 = 38;
const HF_CONFIG: u64 = 549;

/// Écart de la dernière ligne de la couche modèle à son total : Ollama émet
/// ses lignes toutes les 60 ms, la dernière reste souvent sous le total.
const HF_LAST_LINE_SHORT: u64 = 40_304;

/// Silence après `verifying sha256 digest` d'un nom qui contient `checksum`.
pub const HF_CHECKSUM_PAUSE: Duration = Duration::from_millis(800);

/// Morceau d'un flux NDJSON qui n'est pas envoyé : le flux attend ce nombre
/// de millisecondes (voir [`ndjson_then`]).
const PAUSE: &str = "\0pause:";

/// Dépôt et étiquette (`latest` par défaut) d'un nom
/// `hf.co/chatonlm/<dépôt>[:<étiquette>]` (hôte et auteur sans la casse).
fn hf_chatonlm(model: &str) -> Option<(String, String)> {
    let (host, rest) = model.trim().split_once('/')?;
    let (author, rest) = rest.split_once('/')?;
    if !host.eq_ignore_ascii_case("hf.co") || !author.eq_ignore_ascii_case("chatonlm") {
        return None;
    }
    let (repo, tag) = rest.split_once(':').unwrap_or((rest, "latest"));
    (!repo.is_empty() && !repo.contains('/') && !tag.is_empty())
        .then(|| (repo.to_owned(), tag.to_owned()))
}

/// Scénario d'un modèle `hf.co/chatonlm/...`, choisi par un mot du dépôt ou
/// de l'étiquette (sans la casse), dans cet ordre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HfScenario {
    /// `sharded` : version découpée (400 du registre).
    Sharded,
    /// `missing` : dépôt inexistant (401 du registre).
    Missing,
    /// `restricted` : dépôt à accès restreint (401 `GatedRepo` du registre).
    Restricted,
    /// `badtag` : étiquette absente du dépôt (400).
    BadTag,
    /// `badquant` : étiquette qui n'est pas une quantification (400).
    BadQuant,
    /// `notgguf` : dépôt sans GGUF (400).
    NotGguf,
    /// `stall` : trois lignes de la couche modèle, puis des lignes vides.
    Stall,
    /// `cut` : trois lignes de la couche modèle, puis fin sans `success`.
    Cut,
    /// Téléchargement complet ; `vision` : avec un projecteur ; `slow` : une
    /// ligne toutes les 100 ms (environ 5 s) au lieu de 30 ms ; `checksum` :
    /// [`HF_CHECKSUM_PAUSE`] sans rien envoyer après `verifying sha256 digest`
    /// (vérification d'un gros fichier).
    Pull {
        vision: bool,
        slow: bool,
        checksum: bool,
    },
}

fn hf_scenario(repo: &str, tag: &str) -> HfScenario {
    let name = format!("{repo}:{tag}").to_ascii_lowercase();
    let has = |word: &str| name.contains(word);
    if has("sharded") {
        HfScenario::Sharded
    } else if has("missing") {
        HfScenario::Missing
    } else if has("restricted") {
        HfScenario::Restricted
    } else if has("badtag") {
        HfScenario::BadTag
    } else if has("badquant") {
        HfScenario::BadQuant
    } else if has("notgguf") {
        HfScenario::NotGguf
    } else if has("stall") {
        HfScenario::Stall
    } else if has("cut") {
        HfScenario::Cut
    } else {
        HfScenario::Pull {
            vision: has("vision"),
            slow: has("slow"),
            checksum: has("checksum"),
        }
    }
}

/// Réponse du registre de Hugging Face à un manifeste refusé : statut, corps
/// JSON et `x-error-code` (textes exacts de 35-samples/manifest-*.json).
fn hf_refusal(
    scenario: HfScenario,
    repo: &str,
) -> Option<(StatusCode, String, Option<&'static str>)> {
    let error = |status, message: String, code| {
        Some((
            status,
            serde_json::json!({ "error": message }).to_string(),
            code,
        ))
    };
    match scenario {
        HfScenario::Sharded => error(
            StatusCode::BAD_REQUEST,
            "This tag is a sharded GGUF. Ollama does not yet support pulling sharded GGUF via the registry; please use a single-file quantization tag or \"latest\", or download the shards and merge them locally with `ollama create` (workaround detailed at https://github.com/ollama/ollama/issues/5245).".into(),
            None,
        ),
        HfScenario::Missing => error(
            StatusCode::UNAUTHORIZED,
            "Invalid username or password.".into(),
            None,
        ),
        HfScenario::Restricted => error(
            StatusCode::UNAUTHORIZED,
            format!("Access to model chatonlm/{repo} is restricted. You must have access to it and be authenticated to access it. Please log in."),
            Some("GatedRepo"),
        ),
        HfScenario::BadTag => error(
            StatusCode::BAD_REQUEST,
            "The specified tag is not available in the repository. Please use another tag or \"latest\"".into(),
            None,
        ),
        HfScenario::BadQuant => error(
            StatusCode::BAD_REQUEST,
            "The specified tag is not a valid quantization scheme. Please use another tag or \"latest\"".into(),
            None,
        ),
        HfScenario::NotGguf => error(
            StatusCode::BAD_REQUEST,
            "Repository is not GGUF or is not compatible with llama.cpp".into(),
            None,
        ),
        HfScenario::Stall | HfScenario::Cut | HfScenario::Pull { .. } => None,
    }
}

/// Couche d'un manifeste : empreinte, type, taille.
type HfLayer = (String, &'static str, u64);

/// Configuration d'un manifeste : empreinte, taille.
type HfConfig = (String, u64);

/// Couches (empreinte, type, taille) du manifeste d'un `hf.co/chatonlm/...`
/// téléchargeable, dans l'ordre du registre, puis la configuration.
fn hf_layers(repo: &str, tag: &str, vision: bool) -> (Vec<HfLayer>, HfConfig) {
    let digest = |part: &str| {
        let seed = format!("{repo}:{tag}/{part}").to_ascii_lowercase();
        format!("sha256:{}", fake_digest(&seed))
    };
    let mut layers = vec![(
        digest("model"),
        "application/vnd.ollama.image.model",
        HF_MODEL_LAYER,
    )];
    if vision {
        layers.push((
            digest("projector"),
            "application/vnd.ollama.image.projector",
            HF_PROJECTOR_LAYER,
        ));
    }
    layers.push((
        digest("template"),
        "application/vnd.ollama.image.template",
        HF_TEMPLATE_LAYER,
    ));
    layers.push((
        digest("params"),
        "application/vnd.ollama.image.params",
        HF_PARAMS_LAYER,
    ));
    (layers, (digest("config"), HF_CONFIG))
}

/// Taille sur le disque d'un `hf.co/chatonlm/...` téléchargeable : couches et
/// configuration (celle de `/api/tags`, et le total du manifeste).
pub fn hf_total(model: &str) -> u64 {
    let vision = model.to_ascii_lowercase().contains("vision");
    let (layers, (_, config)) = hf_layers("", "", vision);
    layers.iter().map(|(_, _, size)| size).sum::<u64>() + config
}

/// Lignes d'un téléchargement `hf.co/chatonlm/...` : manifeste, couche modèle
/// de 0 à presque complète en `model_steps + 1` lignes (la dernière sous son
/// total), projecteur de 0 à complet en 5 lignes, gabarit (jamais de
/// `completed`), paramètres complets, configuration (sans `completed`),
/// puis la fin, comme le vrai flux (34-samples/pull-hf-*.ndjson). `upto` :
/// seulement les `upto` premières lignes de la couche modèle, sans la suite ;
/// `checksum` : une pause après `verifying sha256 digest`.
fn hf_pull_lines(
    repo: &str,
    tag: &str,
    vision: bool,
    model_steps: u64,
    upto: Option<usize>,
    checksum: bool,
) -> Vec<String> {
    let (layers, (config, config_size)) = hf_layers(repo, tag, vision);
    let mut lines = vec![status_line("pulling manifest")];
    let (model, _, size) = &layers[0];
    let mut model_lines: Vec<String> = (0..model_steps)
        .map(|step| layer_line(model, *size, size * step / model_steps))
        .collect();
    model_lines.push(layer_line(model, *size, size - HF_LAST_LINE_SHORT));
    if let Some(upto) = upto {
        lines.extend(model_lines.into_iter().take(upto));
        return lines;
    }
    lines.extend(model_lines);
    for (digest, kind, size) in &layers[1..] {
        match *kind {
            "application/vnd.ollama.image.projector" => {
                lines.extend((0..=4).map(|step| layer_line(digest, *size, size * step / 4)));
            }
            "application/vnd.ollama.image.params" => lines.push(layer_line(digest, *size, *size)),
            _ => lines.push(layer_line(digest, *size, 0)),
        }
    }
    lines.push(layer_line(&config, config_size, 0));
    lines.push(status_line("verifying sha256 digest"));
    if checksum {
        lines.push(format!("{PAUSE}{}", HF_CHECKSUM_PAUSE.as_millis()));
    }
    lines.extend(["writing manifest", "success"].map(status_line));
    lines
}

/// `/api/pull` d'un `hf.co/chatonlm/<dépôt>:<étiquette>` (voir [`HfScenario`]) :
/// HTTP 200, `pulling manifest`, puis pour un refus du registre la ligne
/// d'erreur exacte d'Ollama 0.35.1 (34 § 2-3) : `pull model manifest: 400:
/// {"error":"…"}` (`sharded`, `badtag`, `badquant`, `notgguf`), ou `pull model
/// manifest: realm host "huggingface.co" does not match original host
/// "hf.co"` (`missing`, `restricted`) ; sinon le flux de [`hf_pull_lines`],
/// une ligne toutes les 30 ms (100 ms pour `slow` ; pour `checksum`, silence
/// de [`HF_CHECKSUM_PAUSE`] après `verifying sha256 digest`). Une fois `success` envoyé,
/// le modèle est installé : `/api/tags` le montre en tête sous le nom demandé
/// (taille [`hf_total`]), `/api/show` répond 200 (capacité `completion`, plus
/// `vision` pour un nom qui contient `vision`).
fn hf_pull(journal: Arc<Journal>, model: &str, repo: &str, tag: &str) -> Response {
    let delay = Duration::from_millis(30);
    let scenario = hf_scenario(repo, tag);
    let failing = |error: String| {
        ndjson(
            journal.clone(),
            vec![status_line("pulling manifest"), error_line(&error)],
            delay,
            End::Close,
        )
    };
    if let Some((status, body, _)) = hf_refusal(scenario, repo) {
        return failing(if status == StatusCode::UNAUTHORIZED {
            r#"pull model manifest: realm host "huggingface.co" does not match original host "hf.co""#.into()
        } else {
            format!("pull model manifest: {}: {body}", status.as_u16())
        });
    }
    match scenario {
        HfScenario::Stall => ndjson(
            journal,
            hf_pull_lines(repo, tag, false, 10, Some(3), false),
            delay,
            End::Stall,
        ),
        HfScenario::Cut => ndjson(
            journal,
            hf_pull_lines(repo, tag, false, 10, Some(3), false),
            delay,
            End::Close,
        ),
        HfScenario::Pull {
            vision,
            slow,
            checksum,
        } => {
            let (steps, delay) = if slow {
                (40, Duration::from_millis(100))
            } else {
                (10, delay)
            };
            let capabilities: &'static [&'static str] = if vision {
                &["completion", "vision"]
            } else {
                &["completion"]
            };
            let (state, name, size) = (journal.clone(), model.trim().to_owned(), hf_total(model));
            ndjson_then(
                journal,
                hf_pull_lines(repo, tag, vision, steps, None, checksum),
                delay,
                End::Close,
                move || state.add_pulled(&name, size, capabilities),
            )
        }
        _ => unreachable!("refusals are answered above"),
    }
}

/// Registre de Hugging Face (espace `chatonlm`) : `GET` ou `HEAD
/// /v2/chatonlm/<dépôt>/manifests/<étiquette>`. Exige l'en-tête `Accept` du
/// manifeste Docker v2 (comme Ollama l'envoie ; 400 sinon, pour que les tests
/// prouvent qu'il part). Refus exacts du registre (voir [`hf_refusal`]) ;
/// `html` : page HTML en 200 (CDN en panne) ; sinon 200 et le manifeste JSON
/// (couches de [`hf_layers`], sans en-tête d'empreinte, comme Hugging Face),
/// à la révision `revision` (voir [`Journal::publish`]).
fn hf_manifest(headers: &HeaderMap, repo: &str, tag: &str, revision: u64) -> Response {
    let accepted = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.contains("application/vnd.docker.distribution.manifest.v2+json")
        });
    if !accepted {
        return json(
            StatusCode::BAD_REQUEST,
            r#"{"error":"the fake registry wants the Docker manifest v2 Accept header"}"#.into(),
        );
    }
    let scenario = hf_scenario(repo, tag);
    if let Some((status, body, code)) = hf_refusal(scenario, repo) {
        let mut response = json(status, body);
        if let Some(code) = code {
            response
                .headers_mut()
                .insert("x-error-code", code.parse().expect("header value"));
        }
        return response;
    }
    if format!("{repo}:{tag}")
        .to_ascii_lowercase()
        .contains("html")
    {
        return not_ollama_page().into_response();
    }
    json(StatusCode::OK, hf_manifest_body(repo, tag, revision))
}

/// Manifeste JSON d'un `hf.co/chatonlm/<dépôt>:<étiquette>` téléchargeable,
/// à la révision `revision` du registre (une nouvelle version change la
/// configuration, donc les octets du manifeste).
fn hf_manifest_body(repo: &str, tag: &str, revision: u64) -> String {
    let vision = matches!(
        hf_scenario(repo, tag),
        HfScenario::Pull { vision: true, .. }
    );
    let (layers, (config, config_size)) = hf_layers(repo, tag, vision);
    let config = if revision == 0 {
        config
    } else {
        let seed = format!("{repo}:{tag}/config@{revision}").to_ascii_lowercase();
        format!("sha256:{}", fake_digest(&seed))
    };
    serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
        "config": {
            "digest": config,
            "mediaType": "application/vnd.docker.container.image.v1+json",
            "size": config_size,
        },
        "layers": layers
            .iter()
            .map(|(digest, kind, size)| serde_json::json!({
                "digest": digest,
                "mediaType": kind,
                "size": size,
            }))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

/// Empreinte du manifeste de `model` (nom sans `:latest`) à la révision
/// `revision` : celle que le registre annonce, et celle de `/api/tags` une
/// fois le modèle téléchargé. Pour un `hf.co/chatonlm/...`, le SHA-256 du
/// manifeste servi, comme Ollama le calcule sur les octets qu'il écrit.
fn manifest_digest(model: &str, revision: u64) -> String {
    if let Some((repo, tag)) = hf_chatonlm(model) {
        let body = hf_manifest_body(&repo, &tag, revision);
        return format!("{:x}", Sha256::digest(body.as_bytes()));
    }
    if revision == 0 {
        fake_digest(model)
    } else {
        fake_digest(&format!("{model}@{revision}"))
    }
}

/// `DELETE /api/delete {"model"}` : un modèle téléchargé sur ce serveur, ou
/// enregistré (si `installed`), à `:latest` et à la casse près → 200 et corps
/// vide, comme Ollama ; sinon 404 `{"error":"model '<nom reçu>' not found"}`.
/// Le nom reçu est journalisé.
fn delete_model(journal: &Journal, installed: bool, body: &str) -> Response {
    let request: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let model = request["model"]
        .as_str()
        .or_else(|| request["name"].as_str())
        .unwrap_or_default()
        .to_owned();
    journal.deletes.lock().unwrap().push(model.clone());
    let removed = {
        let mut pulled = journal.pulled.lock().unwrap();
        let before = pulled.len();
        pulled.retain(|p| !same_model(&p.name, &model));
        pulled.len() < before
    };
    if removed {
        return StatusCode::OK.into_response();
    }
    if installed
        && !journal.is_deleted(&model)
        && let Some(name) = recorded_names()
            .into_iter()
            .find(|name| same_model(name, &model))
    {
        journal.deleted.lock().unwrap().push(name);
        return StatusCode::OK.into_response();
    }
    json(
        StatusCode::NOT_FOUND,
        serde_json::json!({ "error": format!("model '{model}' not found") }).to_string(),
    )
}

/// Réponse des scénarios du nettoyage de la dictée :
/// - `chatonlm-polish` : « Polished: » suivi du dernier message de
///   l'utilisateur sans ses balises `<transcript>` (l'e2e vérifie ainsi tout
///   le chemin jusqu'au presse-papiers) ;
/// - `chatonlm-polish-poem` : un poème sans rapport, que le garde-fou de
///   ChatonLM doit écarter (texte brut copié).
fn polish_reply(model: &str, request: &serde_json::Value) -> String {
    if model == "chatonlm-polish-poem" {
        return "Roses are red,\nviolets are blue,\nthe ocean is deep\nand so is the view.".into();
    }
    let last = request["messages"]
        .as_array()
        .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "user"))
        .and_then(|message| message["content"].as_str())
        .unwrap_or_default();
    let transcript = last
        .trim()
        .trim_start_matches("<transcript>")
        .trim_end_matches("</transcript>")
        .trim();
    format!("Polished: {transcript}")
}

/// Corrections fixes de `chatonlm-proofread` (et de ses variantes), appliquées
/// au texte reçu dans l'ordre.
pub const PROOFREAD_FIXES: &[(&str, &str)] = &[
    ("teh", "the"),
    ("recieve", "receive"),
    ("cinema", "cinéma"),
    ("allez au", "allé au"),
];

/// Réécritures fixes de « Rephrase », après les corrections.
pub const PROOFREAD_REPHRASINGS: &[(&str, &str)] = &[
    ("a lot of", "many"),
    ("in order to", "to"),
    ("est-ce que tu peux", "peux-tu"),
];

/// Raisonnement écrit dans le contenu par `chatonlm-proofread-think`.
pub const PROOFREAD_THINKING: &str = "The user wants a correction. I keep the text as it is.";

/// Réponse de `chatonlm-proofread-off` : autre chose que le texte (le
/// garde-fou de ChatonLM la rejette).
pub const PROOFREAD_OFF: &str = "Sure! Here is a short poem about the sea:\n\
    Waves roll in under a silver sky,\nseagulls circle and the tide runs high.";

/// Texte corrigé (ou reformulé) par le faux serveur.
pub fn proofread(text: &str, rephrase: bool) -> String {
    let mut text = text.to_owned();
    for (from, to) in PROOFREAD_FIXES {
        text = text.replace(from, to);
    }
    if rephrase {
        for (from, to) in PROOFREAD_REPHRASINGS {
            text = text.replace(from, to);
        }
    }
    text
}

/// Correction d'un texte : le dernier message de l'utilisateur, sans ses
/// balises `<text>`, avec les corrections de [`PROOFREAD_FIXES`] ; « Rephrase »
/// (consignes qui demandent une réécriture, « rewrit ») ajoute
/// [`PROOFREAD_REPHRASINGS`]. Morceaux de 8 caractères toutes les 10 ms,
/// sauf :
/// - `chatonlm-proofread-slow` : morceaux de 4 caractères toutes les 150 ms
///   (le temps d'arrêter une correction) ;
/// - `chatonlm-proofread-off` : répond [`PROOFREAD_OFF`] quel que soit le
///   texte ;
/// - `chatonlm-proofread-think` : [`PROOFREAD_THINKING`] entre `<think>` et
///   `</think>` dans le contenu, avant la correction, en morceaux de 5
///   caractères (balises coupées entre deux morceaux) ;
/// - `chatonlm-proofread-opened` : raisonnement ouvert par le gabarit (seul
///   `</think>` écrit) : [`PROOFREAD_THINKING`] deux fois, `</think>`, puis la
///   correction, en morceaux de 5 caractères.
fn proofread_reply(journal: Arc<Journal>, model: &str, request: &serde_json::Value) -> Response {
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let rephrase = messages
        .first()
        .and_then(|message| message["content"].as_str())
        .is_some_and(|system| system.contains("rewrit"));
    let last = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .and_then(|message| message["content"].as_str())
        .unwrap_or_default();
    let text = last
        .strip_prefix("<text>\n")
        .and_then(|text| text.strip_suffix("\n</text>"))
        .unwrap_or(last);
    let corrected = proofread(text, rephrase);
    let (chunks, delay) = match model {
        "chatonlm-proofread-slow" => (reply_lines(model, &corrected, 4), 150),
        "chatonlm-proofread-off" => (reply_lines(model, PROOFREAD_OFF, 8), 10),
        "chatonlm-proofread-think" => (
            reply_lines(
                model,
                &format!("<think>\n{PROOFREAD_THINKING}\n</think>\n\n{corrected}"),
                5,
            ),
            10,
        ),
        "chatonlm-proofread-opened" => (
            reply_lines(
                model,
                &format!("{PROOFREAD_THINKING} {PROOFREAD_THINKING}\n</think>\n\n{corrected}"),
                5,
            ),
            10,
        ),
        _ => (reply_lines(model, &corrected, 8), 10),
    };
    ndjson(journal, chunks, Duration::from_millis(delay), End::Close)
}

/// Lignes des scénarios de la recherche web, selon la requête (`tools`
/// présent et non vide, messages reçus) ; les appels arrivent dans une ligne
/// `message.tool_calls` comme chez Ollama (`id`, `function.index`, `name`,
/// `arguments`), puis la ligne `done` :
/// - `chatonlm-search` : dernier message `tool` → « Found: » suivi de son
///   contenu ; sinon, avec `tools`, un court raisonnement puis un appel
///   `web_search` `{"query": <dernier message de l'utilisateur>}` ; sans
///   `tools`, « Answered without web search. » ;
/// - `chatonlm-fetch` : avec `tools`, appelle `web_fetch` sur la première
///   adresse (mot qui commence par `http://` ou `https://`, pris tel quel,
///   ponctuation comprise) du dernier message de l'utilisateur qui en contient
///   une (« No URL to fetch. » sans adresse), puis, après un seul résultat,
///   sur le premier de ses `links` ; ensuite (deux résultats, une erreur, pas
///   de lien, ou plus d'outils) « Found: » suivi du contenu du dernier message
///   `tool` ; sans `tools` ni message `tool`, « Answered without web
///   fetch. » ;
/// - `chatonlm-image` : si `tools` contient une fonction `generate_image` et
///   que le dernier message est de l'utilisateur, un court raisonnement puis
///   un appel `generate_image` `{"prompt": <son texte>, "width": 512,
///   "height": 512}` ; dernier message `tool` → « Here is your image. » ;
///   sans cet outil, « Image generation is not available. » ;
/// - `chatonlm-loop` : avec `tools`, appelle toujours `web_search`
///   `{"query": "loop N"}` ; sans `tools`, « Done », sauf si le dernier
///   message de l'utilisateur contient `stubborn` : l'appel part quand même
///   (Ollama ne le ferait pas ; ChatonLM doit l'ignorer).
///
/// Les messages `tool` comptés (N, résultats de `chatonlm-fetch`, identifiant
/// `call_N` des appels) sont ceux qui suivent le dernier message de
/// l'utilisateur, N étant leur nombre + 1.
fn tool_reply(model: &str, request: &serde_json::Value) -> Vec<String> {
    use serde_json::json;
    let tools = request["tools"].as_array().is_some_and(|t| !t.is_empty());
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let role =
        |message: &serde_json::Value| message["role"].as_str().unwrap_or_default().to_owned();
    let content =
        |message: &serde_json::Value| message["content"].as_str().unwrap_or_default().to_owned();
    let last = messages.last().cloned().unwrap_or_default();
    // Messages `tool` reçus depuis le dernier message de l'utilisateur (cet
    // envoi).
    let tool_messages = messages
        .iter()
        .rev()
        .take_while(|m| role(m) != "user")
        .filter(|m| role(m) == "tool")
        .count();
    let found = || reply_lines(model, &format!("Found: {}", content(&last)), 16);
    let call = |thinking: &str, name: &str, arguments: serde_json::Value| {
        tool_call_lines(model, thinking, name, arguments, tool_messages + 1)
    };
    // Fonction `generate_image` offerte.
    let paints = request["tools"].as_array().is_some_and(|tools| {
        tools
            .iter()
            .any(|tool| tool["function"]["name"] == "generate_image")
    });
    match model {
        "chatonlm-image" if role(&last) == "tool" => reply_lines(model, "Here is your image.", 16),
        "chatonlm-image" if paints && role(&last) == "user" => call(
            "The user wants a picture.",
            "generate_image",
            json!({ "prompt": content(&last), "width": 512, "height": 512 }),
        ),
        "chatonlm-image" => reply_lines(model, "Image generation is not available.", 16),
        "chatonlm-search" if role(&last) == "tool" => found(),
        "chatonlm-search" if tools => call(
            "The user wants a web search.",
            "web_search",
            json!({ "query": content(&last) }),
        ),
        "chatonlm-search" => reply_lines(model, "Answered without web search.", 16),
        "chatonlm-fetch" => {
            let link = serde_json::from_str::<serde_json::Value>(&content(&last))
                .ok()
                .and_then(|page| page["links"][0].as_str().map(str::to_owned));
            match (tools, role(&last) == "tool", link) {
                (true, true, Some(link)) if tool_messages == 1 => {
                    call("", "web_fetch", json!({ "url": link }))
                }
                (_, true, _) => found(),
                (true, false, _) => {
                    let url = messages
                        .iter()
                        .rev()
                        .filter(|m| role(m) == "user")
                        .find_map(|m| {
                            content(m)
                                .split_whitespace()
                                .find(|word| {
                                    word.starts_with("http://") || word.starts_with("https://")
                                })
                                .map(str::to_owned)
                        });
                    match url {
                        Some(url) => call("", "web_fetch", json!({ "url": url })),
                        None => reply_lines(model, "No URL to fetch.", 16),
                    }
                }
                (false, false, _) => reply_lines(model, "Answered without web fetch.", 16),
            }
        }
        _ if tools
            || messages
                .iter()
                .rev()
                .find(|m| role(m) == "user")
                .is_some_and(|m| content(m).contains("stubborn")) =>
        {
            call(
                "",
                "web_search",
                json!({ "query": format!("loop {}", tool_messages + 1) }),
            )
        }
        _ => reply_lines(model, "Done", 16),
    }
}

/// Flux d'un tour d'outils : raisonnement éventuel, une ligne avec l'appel
/// (identifiant `call_<number>`), puis `done`.
fn tool_call_lines(
    model: &str,
    thinking: &str,
    name: &str,
    arguments: serde_json::Value,
    number: usize,
) -> Vec<String> {
    let mut lines = reasoned_reply_lines(model, thinking, "", 16);
    let call = serde_json::json!({
        "model": model,
        "created_at": "2026-10-04T00:00:00Z",
        "message": {
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": format!("call_{number}"),
                "function": { "index": 0, "name": name, "arguments": arguments },
            }],
        },
        "done": false,
    });
    lines.insert(lines.len() - 1, format!("{call}\n"));
    lines
}

/// Avatar du compte : PNG de 1 × 1 pixel transparent.
pub const AVATAR_PNG: [u8; 68] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x60, 0x00, 0x02, 0x00,
    0x00, 0x05, 0x00, 0x01, 0xe9, 0xfa, 0xdc, 0xd8, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44,
    0xae, 0x42, 0x60, 0x82,
];

/// Attente d'une requête web dont la requête ou l'adresse contient `slow`
/// (plus que les délais de ChatonLM : de quoi l'arrêter).
pub const WEB_SLOW: Duration = Duration::from_secs(30);

/// Taille du contenu d'un résultat `huge` : plus que les 4 Mio lus par
/// ChatonLM, mais fini.
pub const WEB_HUGE: usize = 5 * 1024 * 1024;

/// Réponse 401 de `/api/me` d'un serveur sans compte relié, comme Ollama :
/// adresse de connexion chez « ollama.com », ici ce serveur (en-tête `Host`
/// reçu).
fn unauthorized(headers: &HeaderMap) -> Response {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("127.0.0.1");
    json(
        StatusCode::UNAUTHORIZED,
        serde_json::json!({
            "error": "unauthorized",
            "signin_url": format!("http://{host}/connect?name=fake-host&key=ZmFrZQ"),
        })
        .to_string(),
    )
}

/// `POST /api/me` : relié → 200 et l'utilisateur ; sinon 401 avec l'adresse
/// de connexion (voir [`unauthorized`]).
fn me(journal: &Journal, headers: &HeaderMap) -> Response {
    if journal.account_unavailable.load(Ordering::SeqCst) {
        return json(
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":"account unavailable"}"#.into(),
        );
    }
    if !journal.signed_in.load(Ordering::SeqCst) {
        return unauthorized(headers);
    }
    json(
        StatusCode::OK,
        serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "email": "ada@example.com",
            "name": "ada",
            "plan": "free",
            "avatarurl": "/fake/avatar.png",
        })
        .to_string(),
    )
}

/// `GET /api/status` : `{"cloud": {"disabled", "source"}}` ; source `env` une
/// fois le cloud coupé (comme `OLLAMA_NO_CLOUD`), `none` sinon.
fn cloud_status(journal: &Journal) -> Response {
    let disabled = journal.cloud_disabled.load(Ordering::SeqCst);
    let source = if disabled { "env" } else { "none" };
    json(
        StatusCode::OK,
        serde_json::json!({ "cloud": { "disabled": disabled, "source": source } }).to_string(),
    )
}

/// Encode une valeur de requête d'URL (lettres, chiffres et `-_.~` gardés).
fn query_escape(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            b' ' => "+".into(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// Requête de la recherche web, comme le proxy d'Ollama
/// (`webExperimentalProxyHandler`) : corps vide → 400 « missing request
/// body » ; cloud coupé → 403 « ollama cloud is disabled: web search is
/// unavailable » (ou « web fetch ») ; compte délié → 401
/// `{"error":"Unauthorized"}` (réponse d'ollama.com que le serveur transmet,
/// relevée sur Ollama 0.35.1 : voir le contrat
/// `contract_web_search_needs_a_linked_account` de ChatonLM) ; texte `key`
/// (`query` ou `url`) absent → 400. Sinon le
/// texte, après [`WEB_SLOW`] s'il contient `slow` (requête abandonnée
/// comptée). La requête reçue est journalisée.
async fn web_request(
    journal: &Arc<Journal>,
    path: &str,
    body: &str,
    key: &str,
    operation: &str,
) -> Result<(String, serde_json::Value), Box<Response>> {
    let request: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    journal
        .web
        .lock()
        .unwrap()
        .push(serde_json::json!({ "path": path, "body": request }));
    let error = |status: StatusCode, message: String| {
        let body = serde_json::json!({ "error": message }).to_string();
        Err(Box::new(json(status, body)))
    };
    if body.trim().is_empty() {
        return error(StatusCode::BAD_REQUEST, "missing request body".into());
    }
    if journal.cloud_disabled.load(Ordering::SeqCst) {
        return error(
            StatusCode::FORBIDDEN,
            format!("ollama cloud is disabled: {operation}"),
        );
    }
    if !journal.signed_in.load(Ordering::SeqCst) {
        let body = r#"{"error":"Unauthorized"}"#.to_owned();
        return Err(Box::new(json(StatusCode::UNAUTHORIZED, body)));
    }
    let Some(text) = request[key].as_str().map(str::to_owned) else {
        return error(StatusCode::BAD_REQUEST, format!("{key} is required"));
    };
    if text.contains("slow") {
        let mut guard = DisconnectGuard::new(journal.clone());
        tokio::time::sleep(WEB_SLOW).await;
        guard.finish();
    }
    Ok((text, request))
}

/// `POST /api/experimental/web_search` (voir [`web_request`]) : 200 avec deux
/// résultats déterministes (moins si `max_results` est plus petit) :
/// « Result N for QUERY », `https://example.com/N?q=QUERY`, « Content N about
/// QUERY. » ; une requête qui contient `huge` reçoit un seul résultat dont le
/// contenu fait [`WEB_HUGE`] octets.
async fn web_search(State(journal): State<Arc<Journal>>, body: String) -> Response {
    let path = "/api/experimental/web_search";
    let (query, request) =
        match web_request(&journal, path, &body, "query", "web search is unavailable").await {
            Ok(found) => found,
            Err(response) => return *response,
        };
    let result = |number: i64, content: String| {
        serde_json::json!({
            "title": format!("Result {number} for {query}"),
            "url": format!("https://example.com/{number}?q={}", query_escape(&query)),
            "content": content,
        })
    };
    if query.contains("huge") {
        let results = [result(1, "x".repeat(WEB_HUGE))];
        return json(
            StatusCode::OK,
            serde_json::json!({ "results": results }).to_string(),
        );
    }
    let count = request["max_results"]
        .as_i64()
        .filter(|count| *count > 0)
        .map_or(2, |count| count.min(2));
    let results: Vec<serde_json::Value> = (1..=count)
        .map(|number| result(number, format!("Content {number} about {query}.")))
        .collect();
    json(
        StatusCode::OK,
        serde_json::json!({ "results": results }).to_string(),
    )
}

/// `POST /api/experimental/web_fetch` (voir [`web_request`]) : 200
/// `{"title": "Page URL", "content": "Text of URL.", "links":
/// ["https://example.com/next"]}`.
async fn web_fetch(State(journal): State<Arc<Journal>>, body: String) -> Response {
    let path = "/api/experimental/web_fetch";
    let (url, _) = match web_request(&journal, path, &body, "url", "web fetch is unavailable").await
    {
        Ok(found) => found,
        Err(response) => return *response,
    };
    json(
        StatusCode::OK,
        serde_json::json!({
            "title": format!("Page {url}"),
            "content": format!("Text of {url}."),
            "links": ["https://example.com/next"],
        })
        .to_string(),
    )
}

/// Corps `{"<field>": bool}` d'un réglage du faux serveur ; 400 sinon.
fn fake_switch(body: &str, field: &str, target: &AtomicBool) -> Response {
    let request: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    match request[field].as_bool() {
        Some(value) => {
            target.store(value, Ordering::SeqCst);
            json(StatusCode::OK, "{}".into())
        }
        None => json(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": format!("expected {{\"{field}\": bool}}") }).to_string(),
        ),
    }
}

/// `POST /fake/publish {"model": "<nom>"}` : publie une nouvelle version du
/// modèle au registre (voir [`Journal::publish`]) ; 200 et
/// `{"revision": n}`, 400 sans nom.
fn fake_publish(journal: &Journal, body: &str) -> Response {
    let request: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    match request["model"].as_str().map(str::trim) {
        Some(model) if !model.is_empty() => {
            let revision = journal.publish(model);
            json(
                StatusCode::OK,
                serde_json::json!({ "revision": revision }).to_string(),
            )
        }
        _ => json(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": "expected {\"model\": string}" }).to_string(),
        ),
    }
}

/// Raisonnement de `chatonlm-longthink` : 60 paragraphes Markdown numérotés
/// (« Step 1: … » à « Step 60: … »), pour voir quelle partie l'aperçu montre.
fn long_thinking() -> String {
    (1..=60)
        .map(|step| format!("Step {step}: checking one more detail before answering.\n\n"))
        .collect()
}

/// Raisonnement de `chatonlm-repeat` : quelques paragraphes variés, puis le
/// même paragraphe 300 fois (une vraie boucle de modèle).
fn repeating_thinking() -> String {
    let mut thinking: String = (1..=5)
        .map(|step| format!("Step {step}: the question asks for detail number {step}.\n\n"))
        .collect();
    thinking.push_str(&"Wait, let me check the link count of the directory again.\n\n".repeat(300));
    thinking
}

/// Raisonnement de `chatonlm-ruminate` : 600 paragraphes qui reviennent sur
/// les mêmes vérifications, chacun différent (comme Qwen le 14 oct. 2026 :
/// « Need maybe if… »), environ 6 000 mots.
fn ruminating_thinking() -> String {
    const SUBJECTS: [&str; 5] = ["test0", "test1", "test2", "test3", "the link"];
    const CHECKS: [&str; 4] = [
        "the permissions are right",
        "the size matches",
        "the date can be set",
        "the link count is two",
    ];
    (0..600)
        .map(|i| {
            format!(
                "Need maybe if {} after step {i}: check that {} once more.\n\n",
                SUBJECTS[i % SUBJECTS.len()],
                CHECKS[i % CHECKS.len()],
            )
        })
        .collect()
}

/// Réponse de `chatonlm-context` : [`CONTEXT_ANSWER`] (600 jetons), ou,
/// si le dernier message demande un résumé (« Summarize our conversation »),
/// [`CONTEXT_SUMMARY`] ; coupée à `options.num_predict` jetons s'il est plus
/// petit (`done_reason: "length"`). Jetons comptés à 4 caractères par jeton :
/// `prompt_eval_count` pour tous les messages envoyés, `eval_count` pour la
/// réponse.
fn context_reply(model: &str, request: &serde_json::Value) -> Vec<String> {
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let summary = messages
        .last()
        .and_then(|message| message["content"].as_str())
        .is_some_and(|content| content.starts_with("Summarize our conversation"));
    let full = if summary {
        CONTEXT_SUMMARY.to_owned()
    } else {
        CONTEXT_ANSWER.repeat(40)
    };
    let prompt: usize = messages
        .iter()
        .map(|message| message["content"].as_str().unwrap_or_default().len() / 4)
        .sum();
    let limit = request["options"]["num_predict"]
        .as_u64()
        .and_then(|limit| usize::try_from(limit).ok());
    let (text, length) = match limit {
        Some(limit) if limit * 4 < full.len() => (full[..limit * 4].to_owned(), true),
        _ => (full, false),
    };
    let mut lines = reply_lines(model, &text, 64);
    let mut done: serde_json::Value = serde_json::from_str(lines.pop().unwrap().trim()).unwrap();
    done["prompt_eval_count"] = prompt.into();
    done["eval_count"] = (text.len() / 4).into();
    if length {
        done["done_reason"] = "length".into();
    }
    lines.push(format!("{done}\n"));
    lines
}

/// Morceau de la réponse de `chatonlm-context` (60 caractères, 15 jetons).
pub const CONTEXT_ANSWER: &str = "This answer fills the context of the model, fifteen tokens. ";

/// Résumé écrit par `chatonlm-context`.
pub const CONTEXT_SUMMARY: &str = "The user said hello; the model answered at length.";

/// Flux sans raisonnement dont le contenu est `text`, en morceaux de `size`
/// caractères (jamais au milieu d'un caractère), puis la ligne finale `done`,
/// comme Ollama.
fn reply_lines(model: &str, text: &str, size: usize) -> Vec<String> {
    reasoned_reply_lines(model, "", text, size)
}

/// Comme [`reply_lines`], précédé du raisonnement `thinking` (aucun s'il est
/// vide) dans des lignes au contenu vide, comme Ollama.
fn reasoned_reply_lines(model: &str, thinking: &str, text: &str, size: usize) -> Vec<String> {
    let line = |thinking: String, content: String, done: bool| {
        let mut line = serde_json::json!({
            "model": model,
            "created_at": "2026-10-04T00:00:00Z",
            "message": { "role": "assistant", "content": content },
            "done": done,
        });
        if !thinking.is_empty() {
            line["message"]["thinking"] = thinking.into();
        }
        if done {
            line["done_reason"] = "stop".into();
        }
        format!("{line}\n")
    };
    let pieces = |text: &str| -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        chars
            .chunks(size)
            .map(|chunk| chunk.iter().collect())
            .collect()
    };
    let mut lines: Vec<String> = pieces(thinking)
        .into_iter()
        .map(|piece| line(piece, String::new(), false))
        .collect();
    lines.extend(
        pieces(text)
            .into_iter()
            .map(|piece| line(String::new(), piece, false)),
    );
    lines.push(line(String::new(), String::new(), true));
    lines
}

/// Lignes d'un flux enregistré, chacune avec son saut de ligne.
fn lines(fixture: &str) -> Vec<String> {
    fixture.lines().map(|line| format!("{line}\n")).collect()
}

/// Compte un abandon si le flux est lâché avant sa fin.
struct DisconnectGuard {
    journal: Arc<Journal>,
    finished: bool,
}

impl DisconnectGuard {
    fn new(journal: Arc<Journal>) -> Self {
        Self {
            journal,
            finished: false,
        }
    }

    fn finish(&mut self) {
        self.finished = true;
    }
}

impl Drop for DisconnectGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.journal.disconnects.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Ne répond jamais : le gestionnaire ne rend pas sa réponse, donc aucun
/// en-tête ne part. Le départ du client abandonne ce futur, ce que le gardien
/// compte.
async fn wait(journal: Arc<Journal>) -> Response {
    let _guard = DisconnectGuard::new(journal);
    std::future::pending().await
}

/// Fin d'un flux NDJSON.
#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    /// Fin normale du corps.
    Close,
    /// Lignes vides toutes les 100 ms, sans fin (pour remarquer un client
    /// parti).
    Stall,
    /// Connexion coupée par le serveur au milieu du corps.
    Reset,
}

/// Réponse NDJSON : les morceaux donnés (lignes avec leur saut de ligne), avec
/// un délai entre deux morceaux, puis la fin choisie. Le flux compte comme
/// fini dès son dernier morceau : un client qui le lâche aussitôt n'a rien
/// abandonné.
fn ndjson(journal: Arc<Journal>, chunks: Vec<String>, delay: Duration, end: End) -> Response {
    ndjson_then(journal, chunks, delay, end, || {})
}

/// Comme [`ndjson`] ; `then` est appelé juste avant l'envoi du dernier
/// morceau d'un flux qui finit normalement (`End::Close`). Un morceau
/// `PAUSE<ms>` n'est pas envoyé : le flux se tait pendant ce temps.
fn ndjson_then(
    journal: Arc<Journal>,
    chunks: Vec<String>,
    delay: Duration,
    end: End,
    then: impl FnOnce() + Send + 'static,
) -> Response {
    let stream = futures_util::stream::unfold(
        (
            chunks.into_iter().peekable(),
            DisconnectGuard::new(journal),
            true,
            Some(then),
        ),
        move |(mut chunks, mut guard, first, mut then)| async move {
            while let Some(chunk) = chunks.next() {
                // Pause : rien d'envoyé pendant ce temps.
                if let Some(ms) = chunk.strip_prefix(PAUSE) {
                    tokio::time::sleep(Duration::from_millis(ms.parse().unwrap_or(0))).await;
                    continue;
                }
                if !first && !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                if end == End::Close && chunks.peek().is_none() {
                    guard.finish();
                    if let Some(then) = then.take() {
                        then();
                    }
                }
                return Some((Ok(chunk), (chunks, guard, false, then)));
            }
            match end {
                End::Close => {
                    guard.finish();
                    if let Some(then) = then.take() {
                        then();
                    }
                    None
                }
                End::Stall => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Some((Ok("\n".to_owned()), (chunks, guard, false, then)))
                }
                End::Reset => {
                    // Les lignes déjà écrites partent pendant cette attente ;
                    // la coupure vient du serveur, pas d'un abandon du client.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    guard.finish();
                    let reset = std::io::Error::new(
                        std::io::ErrorKind::ConnectionReset,
                        "connection reset by the fake server",
                    );
                    Some((Err(reset), (chunks, guard, false, then)))
                }
            }
        },
    );
    (
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        Body::from_stream(stream),
    )
        .into_response()
}

/// Empreinte de 64 chiffres hexadécimaux, toujours la même pour `seed`.
fn fake_digest(seed: &str) -> String {
    (0..4u8)
        .map(|part| {
            let mut hasher = DefaultHasher::new();
            (part, seed).hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        })
        .collect()
}

/// `/api/tags` : modèles téléchargés sur ce serveur (du plus récent au plus
/// ancien, `<nom>:latest` sans étiquette, sinon le nom demandé), puis les
/// modèles enregistrés si `installed` (sauf ceux supprimés).
fn tags(journal: &Journal, installed: bool) -> String {
    let mut models: Vec<serde_json::Value> = journal
        .pulled
        .lock()
        .unwrap()
        .iter()
        .map(|pulled| {
            let name = if has_tag(&pulled.name) {
                pulled.name.clone()
            } else {
                format!("{}:latest", pulled.name)
            };
            serde_json::json!({
                "name": name,
                "model": name,
                "modified_at": "2026-10-04T12:00:00.000000000+02:00",
                "size": pulled.size,
                "digest": manifest_digest(&pulled.name, pulled.revision),
                "details": {
                    "parent_model": "",
                    "format": "gguf",
                    "family": "llama",
                    "families": ["llama"],
                    "parameter_size": "8.0B",
                    "quantization_level": "Q4_K_M",
                    "context_length": 131072,
                },
                "capabilities": pulled.capabilities,
            })
        })
        .collect();
    if installed {
        let recorded: serde_json::Value =
            serde_json::from_str(fixtures::TAGS).expect("valid fixture");
        models.extend(
            recorded["models"]
                .as_array()
                .expect("models")
                .iter()
                .filter(|model| !journal.is_deleted(model["name"].as_str().unwrap_or_default()))
                .cloned(),
        );
    }
    serde_json::json!({ "models": models }).to_string()
}

/// `GET /api/experimental/model-recommendations` : liste par défaut d'Ollama
/// (`server/model_recommendations.go`, `defaultModelRecommendations`), au
/// format `api.ModelRecommendationsResponse`.
fn recommendations() -> String {
    const GB: u64 = 1_000_000_000;
    let cloud = |model: &str, description: &str, context: u64, output: u64| {
        serde_json::json!({
            "model": model,
            "description": description,
            "context_length": context,
            "max_output_tokens": output,
        })
    };
    let local = |model: &str, description: &str, vram: u64| serde_json::json!({ "model": model, "description": description, "vram_bytes": vram });
    serde_json::json!({
        "recommendations": [
            cloud(
                "kimi-k2.6:cloud",
                "State-of-the-art coding, long-horizon execution, and multimodal agent swarm capability",
                262_144,
                262_144,
            ),
            cloud("glm-5.1:cloud", "Reasoning and code generation", 202_752, 131_072),
            cloud(
                "qwen3.5:cloud",
                "Reasoning, coding, and agentic tool use with vision",
                262_144,
                32_768,
            ),
            cloud(
                "minimax-m2.7:cloud",
                "Fast, efficient coding and real-world productivity",
                204_800,
                128_000,
            ),
            local("gemma4", "Reasoning and code generation locally", 12 * GB),
            local("qwen3.5", "Reasoning, coding, and visual understanding locally", 14 * GB),
        ]
    })
    .to_string()
}

/// Modèles connus du faux registre (espace `library`, toute étiquette).
fn in_registry(namespace: &str, name: &str) -> bool {
    namespace == "library"
        && (name.starts_with("chatonlm-") || matches!(name, "gemma4" | "qwen3.5" | "llama3.2"))
}

/// Registre (`CHATONLM_REGISTRY_URL` de l'app) : `HEAD` ou `GET
/// /v2/<espace>/<nom>/manifests/<étiquette>` → 200 avec les en-têtes
/// `ollama-content-digest` (empreinte du manifeste, voir [`manifest_digest`])
/// et `ollama-push-time` (date de publication) pour les modèles de
/// [`in_registry`], 404 sinon, comme ollama.com ; espace `chatonlm` : registre
/// de Hugging Face (voir [`hf_manifest`], `CHATONLM_HF_URL` de l'app). Une
/// version publiée par [`Journal::publish`] change l'empreinte et la date.
/// Requête journalisée.
async fn manifest(
    State(journal): State<Arc<Journal>>,
    method: Method,
    headers: HeaderMap,
    Path((namespace, name, tag)): Path<(String, String, String)>,
) -> Response {
    journal
        .manifests
        .lock()
        .unwrap()
        .push(format!("{method} /v2/{namespace}/{name}/manifests/{tag}"));
    if namespace.eq_ignore_ascii_case("chatonlm") {
        let revision = journal.revision(&format!("hf.co/chatonlm/{name}:{tag}"));
        return hf_manifest(&headers, &name, &tag, revision);
    }
    if !in_registry(&namespace, &name) {
        return json(
            StatusCode::NOT_FOUND,
            r#"{"errors":[{"code":"MANIFEST_UNKNOWN","message":"manifest unknown"}]}"#.into(),
        );
    }
    let body = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
    });
    // Nom du modèle comme `/api/tags` le garde : sans `:latest`.
    let model = if tag == "latest" {
        name.clone()
    } else {
        format!("{name}:{tag}")
    };
    (
        [
            (
                header::CONTENT_TYPE.as_str(),
                "application/vnd.docker.distribution.manifest.v2+json".to_owned(),
            ),
            (
                "ollama-content-digest",
                manifest_digest(&model, journal.revision(&model)),
            ),
            ("ollama-push-time", journal.push_time(&model).to_string()),
        ],
        body.to_string(),
    )
        .into_response()
}

fn router(mode: Mode, journal: Arc<Journal>) -> Router {
    match mode {
        Mode::Ollama { version, models } => {
            let body = serde_json::json!({ "version": version }).to_string();
            Router::new()
                .route("/", get(|| async { "Ollama is running" }))
                .route(
                    "/api/version",
                    get(move || {
                        let body = body.clone();
                        async move { json(StatusCode::OK, body) }
                    }),
                )
                .route(
                    "/api/tags",
                    get(move |State(journal): State<Arc<Journal>>| async move {
                        json(StatusCode::OK, tags(&journal, models))
                    }),
                )
                .route(
                    "/api/delete",
                    delete(
                        move |State(journal): State<Arc<Journal>>, body: String| async move {
                            delete_model(&journal, models, &body)
                        },
                    ),
                )
                .route(
                    "/api/experimental/model-recommendations",
                    get(|| async { json(StatusCode::OK, recommendations()) }),
                )
                .route("/v2/{namespace}/{name}/manifests/{tag}", get(manifest))
                .route("/api/show", post(show))
                .route("/api/chat", post(chat))
                .route("/api/generate", post(generate))
                .route(
                    "/api/ps",
                    get(|State(journal): State<Arc<Journal>>| async move {
                        json(StatusCode::OK, ps(&journal))
                    }),
                )
                .route("/api/pull", post(pull))
                .route(
                    "/api/me",
                    post(
                        |State(journal): State<Arc<Journal>>, headers: HeaderMap| async move {
                            me(&journal, &headers)
                        },
                    ),
                )
                .route(
                    "/api/signout",
                    post(|State(journal): State<Arc<Journal>>| async move {
                        journal.signed_in.store(false, Ordering::SeqCst);
                        json(StatusCode::OK, "{}".into())
                    }),
                )
                .route(
                    "/api/status",
                    get(
                        |State(journal): State<Arc<Journal>>| async move { cloud_status(&journal) },
                    ),
                )
                .route("/api/experimental/web_search", post(web_search))
                .route("/api/experimental/web_fetch", post(web_fetch))
                .route(
                    "/fake/avatar.png",
                    get(|| async { ([(header::CONTENT_TYPE, "image/png")], AVATAR_PNG.to_vec()) }),
                )
                // Redirection en clair vers l'image : ChatonLM ne la suit pas.
                .route(
                    "/fake/avatar-redirect",
                    get(|headers: HeaderMap| async move {
                        let host = headers
                            .get(header::HOST)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("127.0.0.1")
                            .to_owned();
                        (
                            StatusCode::FOUND,
                            [(header::LOCATION, format!("http://{host}/fake/avatar.png"))],
                        )
                    }),
                )
                .route(
                    "/fake/account",
                    post(
                        |State(journal): State<Arc<Journal>>, body: String| async move {
                            fake_switch(&body, "signedIn", &journal.signed_in)
                        },
                    ),
                )
                .route(
                    "/fake/account-unavailable",
                    post(
                        |State(journal): State<Arc<Journal>>, body: String| async move {
                            fake_switch(&body, "unavailable", &journal.account_unavailable)
                        },
                    ),
                )
                .route(
                    "/fake/cloud",
                    post(
                        |State(journal): State<Arc<Journal>>, body: String| async move {
                            fake_switch(&body, "disabled", &journal.cloud_disabled)
                        },
                    ),
                )
                .route(
                    "/fake/manifests",
                    get(|State(journal): State<Arc<Journal>>| async move {
                        let seen = journal.manifests.lock().unwrap().clone();
                        json(StatusCode::OK, serde_json::json!(seen).to_string())
                    }),
                )
                .route(
                    "/fake/publish",
                    post(
                        |State(journal): State<Arc<Journal>>, body: String| async move {
                            fake_publish(&journal, &body)
                        },
                    ),
                )
                .route(
                    "/fake/web",
                    get(|State(journal): State<Arc<Journal>>| async move {
                        let web = journal.web.lock().unwrap().clone();
                        json(StatusCode::OK, serde_json::Value::Array(web).to_string())
                    }),
                )
                .route(
                    "/fake/requests",
                    get(|State(journal): State<Arc<Journal>>| async move {
                        let chats = journal.chats.lock().unwrap().clone();
                        json(StatusCode::OK, serde_json::Value::Array(chats).to_string())
                    }),
                )
                .route(
                    "/fake/pulls",
                    get(|State(journal): State<Arc<Journal>>| async move {
                        let pulls = journal.pulls.lock().unwrap().clone();
                        json(StatusCode::OK, serde_json::Value::Array(pulls).to_string())
                    }),
                )
                .route(
                    "/fake/deletes",
                    get(|State(journal): State<Arc<Journal>>| async move {
                        let deletes = journal.deletes.lock().unwrap().clone();
                        json(StatusCode::OK, serde_json::json!(deletes).to_string())
                    }),
                )
                .route(
                    "/fake/loads",
                    get(|State(journal): State<Arc<Journal>>| async move {
                        let loads = journal.loads.lock().unwrap().clone();
                        json(StatusCode::OK, serde_json::Value::Array(loads).to_string())
                    }),
                )
                .route(
                    "/fake/disconnects",
                    get(|State(journal): State<Arc<Journal>>| async move {
                        let count = journal.disconnects.load(Ordering::SeqCst);
                        let body = serde_json::json!({ "disconnects": count });
                        json(StatusCode::OK, body.to_string())
                    }),
                )
                .with_state(journal)
        }
        Mode::NotOllama => Router::new().fallback(|| async { not_ollama_page() }),
        Mode::Status(code) => {
            let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            Router::new().fallback(move || async move { status })
        }
        Mode::Huge => Router::new().route(
            "/api/version",
            get(|| async {
                let body = format!("{{\"version\":\"{}\"}}", "9".repeat(1 << 20));
                ([(header::CONTENT_TYPE, "application/json")], body)
            }),
        ),
        Mode::Silent | Mode::Close => unreachable!("this mode has no router"),
    }
}

fn not_ollama_page() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html")],
        "<!doctype html><title>Another server</title><p>Hello</p>",
    )
}

/// Ferme chaque connexion dès qu'elle est acceptée.
async fn close(listener: TcpListener, mut stop: oneshot::Receiver<()>) {
    loop {
        tokio::select! {
            accepted = listener.accept() => drop(accepted),
            _ = &mut stop => break,
        }
    }
}

/// Garde les connexions ouvertes sans jamais répondre.
async fn silent(listener: TcpListener, mut stop: oneshot::Receiver<()>) {
    let mut held = Vec::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((socket, _)) = accepted {
                    held.push(socket);
                }
            }
            _ = &mut stop => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use futures_util::StreamExt;

    use super::*;

    /// Morceaux du corps de `/api/chat` pour ce modèle, et durée du flux.
    async fn stream(model: &str) -> (Vec<String>, Duration) {
        let body = serde_json::json!({
            "model": model,
            "messages": [{ "role": "user", "content": "Markdown, please." }],
        });
        let response = chat(State(Arc::new(Journal::default())), body.to_string()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/x-ndjson"
        );
        let started = Instant::now();
        let mut data = response.into_body().into_data_stream();
        let mut chunks = Vec::new();
        while let Some(chunk) = data.next().await {
            chunks.push(String::from_utf8(chunk.expect("body chunk").to_vec()).expect("UTF-8"));
        }
        (chunks, started.elapsed())
    }

    /// Vérifie un scénario Markdown : une ligne NDJSON par morceau, morceaux de
    /// `size` caractères (le dernier peut être plus court), sans raisonnement,
    /// recollés égaux au fichier, puis `done` ; renvoie le nombre de morceaux
    /// et la durée du flux.
    async fn check_markdown_reply(model: &str, text: &str, size: usize) -> (usize, Duration) {
        let (chunks, elapsed) = stream(model).await;
        let lines: Vec<serde_json::Value> = chunks
            .iter()
            .map(|chunk| {
                let line = chunk.strip_suffix('\n').expect("one whole line per chunk");
                assert!(!line.contains('\n'), "one line per chunk: {chunk:?}");
                serde_json::from_str(line).expect("JSON line")
            })
            .collect();
        let (last, fragments) = lines.split_last().expect("at least the done line");
        assert_eq!(last["done"], true);
        assert_eq!(last["done_reason"], "stop");
        assert_eq!(last["message"]["content"], "");

        let mut joined = String::new();
        for (index, fragment) in fragments.iter().enumerate() {
            assert_eq!(fragment["done"], false);
            assert_eq!(fragment["model"], model);
            assert_eq!(fragment["message"]["role"], "assistant");
            assert!(fragment["message"].get("thinking").is_none());
            let content = fragment["message"]["content"].as_str().expect("content");
            let count = content.chars().count();
            if index + 1 < fragments.len() {
                assert_eq!(count, size, "fragment {index}: {content:?}");
            } else {
                assert!((1..=size).contains(&count), "last fragment: {content:?}");
            }
            joined.push_str(content);
        }
        assert_eq!(joined, text);
        (fragments.len(), elapsed)
    }

    #[tokio::test]
    async fn markdown_is_streamed_quickly_in_chunks_of_16_characters() {
        let (_, elapsed) = check_markdown_reply("chatonlm-markdown", fixtures::MARKDOWN, 16).await;
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    }

    #[tokio::test]
    async fn slow_markdown_sends_6_characters_every_100_ms() {
        let (fragments, elapsed) =
            check_markdown_reply("chatonlm-markdown-slow", fixtures::MARKDOWN_STREAM, 6).await;
        // Un délai avant chaque ligne sauf la première (`done` comprise) ; un
        // sommeil ne finit jamais en avance, d'où une borne basse seulement.
        assert!(elapsed >= Duration::from_millis(100) * fragments as u32);
    }

    #[tokio::test]
    async fn long_markdown_sends_24_characters_every_15_ms() {
        let (fragments, elapsed) =
            check_markdown_reply("chatonlm-markdown-long", fixtures::MARKDOWN_LONG, 24).await;
        assert!(elapsed >= Duration::from_millis(15) * fragments as u32);
    }

    #[test]
    fn reply_lines_never_split_a_character() {
        let lines = reply_lines("m", "aé€😀b", 2);
        let contents: Vec<String> = lines
            .iter()
            .map(|line| {
                let value: serde_json::Value = serde_json::from_str(line).unwrap();
                value["message"]["content"].as_str().unwrap().to_owned()
            })
            .collect();
        assert_eq!(contents, ["aé", "€😀", "b", ""]);
    }

    /// Correction déterministe des scénarios `chatonlm-proofread*` (les essais
    /// de l'app réelle comptent sur ces remplacements).
    #[test]
    fn proofread_replaces_fixed_mistakes() {
        assert_eq!(
            proofread("I recieve teh letter. Je suis allez au cinema.", false),
            "I receive the letter. Je suis allé au cinéma."
        );
        assert_eq!(
            proofread("A lot of work in order to recieve it.", false),
            "A lot of work in order to receive it."
        );
        assert_eq!(
            proofread("I have a lot of work in order to recieve it.", true),
            "I have many work to receive it."
        );
        assert_eq!(
            proofread("est-ce que tu peux venir ?", true),
            "peux-tu venir ?"
        );
    }

    /// Les tests de l'app réelle comptent sur ces constructions : elles ne
    /// doivent pas disparaître d'une retouche des fichiers.
    #[test]
    fn markdown_fixtures_cover_the_rendering_cases() {
        let markdown = fixtures::MARKDOWN;
        for needle in [
            "# Markdown check\n",
            "\n## ",
            "\n### ",
            "\n#### ",
            "\n##### ",
            "\n###### ",
            "**bold**",
            "*italic*",
            "~~strike~~",
            "\n  - Nested item",
            "\n1. One",
            "\n   1. Two point one",
            "\n- [x] ",
            "\n- [ ] ",
            "|:------|-----:|:-------:|",
            "\n> ",
            "`inline code`",
            "\n```typescript\n",
            "\n```python\n",
            "\n```bash\n",
            "\n```\nno   language\n",
            "\n```ruby\n",
            "\n```Python\n",
            "\n```mermaid\n",
            "\\(a^2 + b^2 = c^2\\)",
            "\n\\[\n",
            "\n$$\n",
            "$5 and $10",
            "<script>window.__pwned=1</script>",
            r#"<img src=x onerror="window.__pwned=1">"#,
            "<iframe ",
            "![Ollama logo](https://example.com/logo.png)",
            "(https://example.com/ollama-docs)",
            "(http://localhost:11434)",
            "[x](javascript:window.__pwned=1)",
            "[empty link]()",
            "<https://example.com/autolink>",
            r#"(https://example.com/titled "Docs title")"#,
            "[https://example.com/shown](https://example.com/real)",
        ] {
            assert!(markdown.contains(needle), "markdown.md lacks {needle:?}");
        }
        // Un bloc par langage coloré par l'app (les 19 de l'officiel), dont
        // les motifs réécrits contre le retour arrière (échappement CSS, aussi
        // dans un <style> HTML ; canaux Go ; pointeurs C++).
        for lang in [
            "javascript",
            "typescript",
            "python",
            "bash",
            "shell",
            "json",
            "html",
            "css",
            "tsx",
            "jsx",
            "go",
            "rust",
            "java",
            "c",
            "cpp",
            "sql",
            "swift",
            "yaml",
            "markdown",
        ] {
            assert!(!fenced_code(markdown, lang).is_empty(), "{lang}");
        }
        assert!(fenced_code(markdown, "css").contains(r"\:hover\:"));
        assert!(fenced_code(markdown, "html").contains("<style>"));
        assert!(fenced_code(markdown, "go").contains("chan<- bool"));
        assert!(fenced_code(markdown, "cpp").contains("int **"));

        let stream = fixtures::MARKDOWN_STREAM;
        assert!(stream.starts_with("## "));
        assert!(stream.contains("**deployment**"));
        assert!(stream.contains("\n$$\n"));
        let code = fenced_code(stream, "bash");
        assert!((8..=15).contains(&code.lines().count()), "{code}");
        // Ce que la réparation du Markdown incomplet de Streamdown abîmait
        // dans un bloc ouvert (« $$$$ », lien incomplet, « ** » ajouté).
        for trap in ["$$", "[ -d", "**"] {
            assert!(code.contains(trap), "{trap}");
        }

        let long = fixtures::MARKDOWN_LONG;
        let code = fenced_code(long, "typescript");
        assert!(code.lines().count() >= 180, "{}", code.lines().count());
        assert!(code.lines().all(|line| line.chars().count() <= 120));
        // Un paragraphe avant et après le bloc.
        assert!(!long.starts_with("```"));
        assert!(!long.trim_end().ends_with("```"));
    }

    /// Corps JSON de `/api/show` pour ce modèle, avec le code d'état.
    async fn show_of(model: &str) -> (StatusCode, serde_json::Value) {
        show_on(&Arc::new(Journal::default()), model).await
    }

    /// Comme [`show_of`], sur un serveur à l'état donné.
    async fn show_on(journal: &Arc<Journal>, model: &str) -> (StatusCode, serde_json::Value) {
        let request = serde_json::json!({ "model": model }).to_string();
        let response = show(State(journal.clone()), request).await;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (status, serde_json::from_slice(&body).expect("JSON body"))
    }

    #[tokio::test]
    async fn show_describes_the_thinking_scenarios() {
        let thinking = serde_json::json!(["completion", "thinking"]);
        for (model, expected) in [
            (
                "chatonlm-levels",
                serde_json::json!({ "values": ["low", "medium", "high"], "default": "medium" }),
            ),
            (
                "chatonlm-levels-off",
                serde_json::json!({ "values": [false, "low", "medium", "high"], "default": "low" }),
            ),
            (
                "chatonlm-always",
                serde_json::json!({ "values": [true], "default": true }),
            ),
            (
                "chatonlm-never",
                serde_json::json!({ "values": [false], "default": false }),
            ),
        ] {
            let (status, body) = show_of(model).await;
            assert_eq!(status, StatusCode::OK, "{model}");
            assert_eq!(body["capabilities"], thinking, "{model}");
            assert_eq!(body["thinking"], expected, "{model}");
            // Le reste suit une vraie réponse d'Ollama.
            assert_eq!(body["details"]["format"], "gguf", "{model}");
        }
        let (status, nometa) = show_of("chatonlm-nometa").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(nometa["capabilities"], thinking);
        assert!(nometa.get("thinking").is_none());
        let (status, plain) = show_of("chatonlm-plain").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(plain["capabilities"], serde_json::json!(["completion"]));
        assert!(plain.get("thinking").is_none());

        // Enregistrés pour les vrais modèles, 404 pour les noms inconnus.
        let (status, qwen3) = show_of("qwen3:0.6b").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            qwen3["thinking"],
            serde_json::json!({ "values": [false, true], "default": true })
        );
        for model in [
            "chatonlm-levels:latest",
            "chatonlm-missing",
            "chatonlm-pull",
        ] {
            assert_eq!(show_of(model).await.0, StatusCode::NOT_FOUND, "{model}");
        }
        // Aucun scénario dans la liste des modèles installés.
        assert!(!fixtures::TAGS.contains("chatonlm-"));
    }

    /// Aucun scénario de chat ne déclenche de téléchargement : `/api/show`
    /// répond 200, capacité `completion` seule, sans objet `thinking`.
    #[tokio::test]
    async fn chat_scenarios_are_installed() {
        for model in CHAT_SCENARIOS {
            let (status, body) = show_of(model).await;
            assert_eq!(status, StatusCode::OK, "{model}");
            assert_eq!(
                body["capabilities"],
                serde_json::json!(["completion"]),
                "{model}"
            );
            assert!(body.get("thinking").is_none(), "{model}");
        }
        // Chaque nom de la liste est bien un scénario de /api/chat (sauf
        // chatonlm-vanished, absent au moment du chat) : pas la 404 des noms
        // inconnus.
        for model in CHAT_SCENARIOS {
            // Ne répond jamais.
            if *model == "chatonlm-wait" {
                continue;
            }
            let request = serde_json::json!({ "model": model, "think": false });
            let response = chat(State(Arc::new(Journal::default())), request.to_string()).await;
            let expected = match *model {
                "chatonlm-vanished" => StatusCode::NOT_FOUND,
                // Ne se charge pas (mémoire insuffisante).
                "chatonlm-loadfail" => StatusCode::INTERNAL_SERVER_ERROR,
                _ => StatusCode::OK,
            };
            assert_eq!(response.status(), expected, "{model}");
        }
        // Détails illisibles : ChatonLM garde `think`, refusé ensuite par 400.
        let (status, nothink) = show_of("chatonlm-nothink").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(nothink["error"].is_string());
        let (status, lines) =
            chat_lines(serde_json::json!({ "model": "chatonlm-nothink", "think": "high" })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            lines,
            [serde_json::json!({ "error": "\"chatonlm-nothink\" does not support thinking" })]
        );
    }

    /// Requête HTTP/1.1 brute au serveur (le crate n'a pas de client) : corps
    /// de la réponse, tel qu'il arrive.
    async fn http(addr: SocketAddr, method: &str, path: &str, body: &str) -> String {
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        tokio::task::spawn_blocking(move || {
            use std::io::{Read, Write};
            let mut stream = std::net::TcpStream::connect(addr).expect("connect");
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("timeout");
            stream.write_all(request.as_bytes()).expect("write");
            let mut response = String::new();
            stream.read_to_string(&mut response).expect("read");
            let (_, body) = response.split_once("\r\n\r\n").expect("headers");
            body.to_owned()
        })
        .await
        .expect("request")
    }

    /// `chatonlm-vision` voit les images (un autre scénario non) et répond
    /// comme un modèle installé sans raisonnement ; le corps reçu (texte des
    /// fichiers, images en base64) se relit sur `GET /fake/requests`.
    #[tokio::test]
    async fn the_vision_scenario_and_what_it_received() {
        let (status, body) = show_of("chatonlm-vision").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["capabilities"],
            serde_json::json!(["completion", "vision"])
        );
        assert!(body.get("thinking").is_none());
        assert_eq!(body["details"]["format"], "gguf");
        let (_, slow) = show_of("chatonlm-slow").await;
        assert_eq!(slow["capabilities"], serde_json::json!(["completion"]));

        let request = serde_json::json!({
            "model": "chatonlm-vision",
            "messages": [{
                "role": "user",
                "content": "Describe.\n--- File: notes.txt ---\nhello\n--- End of notes.txt ---",
                "images": ["iVBORw0KGgo="],
            }],
            "stream": true,
        });
        let (status, received) = chat_lines(request.clone()).await;
        assert_eq!(status, StatusCode::OK);
        let recorded: Vec<serde_json::Value> = lines(fixtures::CHAT_NO_THINK)
            .iter()
            .map(|line| serde_json::from_str(line).expect("JSON line"))
            .collect();
        assert_eq!(received, recorded);

        let fake = FakeOllama::start(Mode::ollama()).await.expect("server");
        let reply = http(fake.addr(), "POST", "/api/chat", &request.to_string()).await;
        assert!(reply.contains(r#""done":true"#), "{reply}");
        let journal = http(fake.addr(), "GET", "/fake/requests", "").await;
        let journal: serde_json::Value = serde_json::from_str(&journal).expect("JSON body");
        assert_eq!(journal, serde_json::json!([request]));
        fake.stop().await;
    }

    /// Lignes JSON de `/api/pull` pour ce modèle, sur ce serveur, et durée.
    async fn pull_lines(
        journal: &Arc<Journal>,
        model: &str,
    ) -> (StatusCode, Vec<serde_json::Value>, Duration) {
        let request = serde_json::json!({ "model": model, "stream": true });
        let response = pull(State(journal.clone()), request.to_string()).await;
        let status = response.status();
        let started = Instant::now();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let elapsed = started.elapsed();
        let lines = String::from_utf8(body.to_vec())
            .expect("UTF-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON line"))
            .collect();
        (status, lines, elapsed)
    }

    fn statuses(lines: &[serde_json::Value]) -> Vec<&str> {
        lines
            .iter()
            .map(|line| line["status"].as_str().unwrap_or("-"))
            .collect()
    }

    #[tokio::test]
    async fn a_pull_installs_the_model_on_this_server_only() {
        let journal = Arc::new(Journal::default());
        let (status, lines, elapsed) = pull_lines(&journal, "chatonlm-pull").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(lines.len(), 16);
        assert_eq!(
            lines[0],
            serde_json::json!({ "status": "pulling manifest" })
        );
        // Grosse couche de 0 à complète en 11 lignes, `completed` absent à 0.
        let big = &lines[1..12];
        assert!(big[0].get("completed").is_none());
        assert_eq!(big[0]["total"], PULL_TOTAL);
        let mut previous = 0;
        for line in big {
            assert_eq!(line["digest"], BIG_LAYER);
            assert_eq!(line["status"], format!("pulling {}", &BIG_LAYER[7..19]));
            assert_eq!(line["total"], PULL_TOTAL);
            let completed = line["completed"].as_u64().unwrap_or(0);
            assert!(completed >= previous);
            previous = completed;
        }
        assert_eq!(previous, PULL_TOTAL);
        assert_eq!(lines[12]["digest"], SMALL_LAYER);
        assert_eq!(lines[12]["total"], PULL_SMALL_LAYER);
        assert_eq!(lines[12]["completed"], PULL_SMALL_LAYER);
        assert_eq!(
            statuses(&lines[13..]),
            ["verifying sha256 digest", "writing manifest", "success"]
        );
        // Une ligne toutes les 30 ms.
        assert!(elapsed >= Duration::from_millis(30) * 15, "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
        assert_eq!(
            journal.pulls.lock().unwrap().as_slice(),
            [serde_json::json!({ "model": "chatonlm-pull", "stream": true })]
        );

        // Téléchargé : installé sur ce serveur, avec une réponse sans
        // raisonnement.
        let (status, body) = show_on(&journal, "chatonlm-pull").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["capabilities"], serde_json::json!(["completion"]));
        let request = serde_json::json!({ "model": "chatonlm-pull", "think": true });
        let response = chat(State(journal.clone()), request.to_string()).await;
        assert_eq!(response.status(), StatusCode::OK);
        // Pas sur un autre serveur.
        assert_eq!(show_of("chatonlm-pull").await.0, StatusCode::NOT_FOUND);
        let (status, lines) = chat_lines(serde_json::json!({ "model": "chatonlm-pull" })).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            lines[0]["error"],
            "model 'chatonlm-missing:latest' not found"
        );
    }

    /// Noms de `/api/tags` sur ce serveur.
    fn tag_names(journal: &Journal, installed: bool) -> Vec<String> {
        let tags: serde_json::Value = serde_json::from_str(&tags(journal, installed)).unwrap();
        let models = tags["models"].as_array().unwrap();
        models
            .iter()
            .map(|m| m["name"].as_str().unwrap().to_owned())
            .collect()
    }

    #[tokio::test]
    async fn pulled_models_are_listed_first() {
        let journal = Arc::new(Journal::default());
        assert_eq!(tag_names(&journal, true), ["qwen3:0.6b", "qwen3.5:0.8b"]);
        assert!(tag_names(&journal, false).is_empty());
        pull_lines(&journal, "chatonlm-pull").await;
        // Téléchargement interrompu ou manqué : pas installé.
        pull_lines(&journal, "chatonlm-pull-cut").await;
        pull_lines(&journal, "chatonlm-pull-error").await;
        pull_lines(&journal, "chatonlm-pull-small").await;
        assert_eq!(
            tag_names(&journal, true),
            [
                "chatonlm-pull-small:latest",
                "chatonlm-pull:latest",
                "qwen3:0.6b",
                "qwen3.5:0.8b"
            ]
        );
        // Aussi sans les modèles enregistrés ; un nouveau téléchargement
        // remet le modèle en tête, sans doublon.
        pull_lines(&journal, "chatonlm-pull").await;
        assert_eq!(
            tag_names(&journal, false),
            ["chatonlm-pull:latest", "chatonlm-pull-small:latest"]
        );
        let tags: serde_json::Value = serde_json::from_str(&tags(&journal, false)).unwrap();
        let (big, small) = (&tags["models"][0], &tags["models"][1]);
        assert_eq!(big["model"], "chatonlm-pull:latest");
        assert_eq!(big["size"], PULL_TOTAL);
        assert_eq!(small["size"], PULL_TINY_LAYER);
        assert_eq!(big["capabilities"], serde_json::json!(["completion"]));
        assert_eq!(big["details"]["format"], "gguf");
        let digest = big["digest"].as_str().unwrap();
        assert_eq!(digest.len(), 64);
        assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(digest, small["digest"].as_str().unwrap());
        // `:latest` désigne le même modèle.
        assert_eq!(
            show_on(&journal, "chatonlm-pull:latest").await.0,
            StatusCode::OK
        );
    }

    #[test]
    fn recommendations_are_the_official_defaults() {
        let body: serde_json::Value = serde_json::from_str(&recommendations()).unwrap();
        let models: Vec<&str> = body["recommendations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["model"].as_str().unwrap())
            .collect();
        assert_eq!(
            models,
            [
                "kimi-k2.6:cloud",
                "glm-5.1:cloud",
                "qwen3.5:cloud",
                "minimax-m2.7:cloud",
                "gemma4",
                "qwen3.5"
            ]
        );
        let gemma = &body["recommendations"][4];
        assert_eq!(
            gemma["description"],
            "Reasoning and code generation locally"
        );
        assert_eq!(gemma["vram_bytes"], 12_000_000_000_u64);
        assert!(gemma.get("context_length").is_none());
        assert_eq!(body["recommendations"][0]["context_length"], 262_144);
    }

    /// Code d'état, en-tête `ollama-content-digest` et journal du registre.
    async fn manifest_of(
        journal: &Arc<Journal>,
        method: Method,
        path: (&str, &str, &str),
    ) -> (StatusCode, Option<String>) {
        let (namespace, name, tag) = path;
        let path = Path((namespace.to_owned(), name.to_owned(), tag.to_owned()));
        let response = manifest(State(journal.clone()), method, HeaderMap::new(), path).await;
        let digest = response
            .headers()
            .get("ollama-content-digest")
            .map(|value| value.to_str().unwrap().to_owned());
        (response.status(), digest)
    }

    #[tokio::test]
    async fn the_registry_knows_a_few_models() {
        let journal = Arc::new(Journal::default());
        for name in [
            "chatonlm-pull",
            "chatonlm-anything",
            "gemma4",
            "qwen3.5",
            "llama3.2",
        ] {
            let (status, digest) =
                manifest_of(&journal, Method::HEAD, ("library", name, "latest")).await;
            assert_eq!(status, StatusCode::OK, "{name}");
            let digest = digest.expect("digest header");
            assert_eq!(digest.len(), 64, "{name}");
            assert!(digest.chars().all(|c| c.is_ascii_hexdigit()), "{name}");
        }
        // Toute étiquette.
        let (status, _) = manifest_of(&journal, Method::GET, ("library", "gemma4", "27b")).await;
        assert_eq!(status, StatusCode::OK);
        for (namespace, name) in [
            ("library", "qwen3"),
            ("library", "gemma"),
            ("someone", "gemma4"),
        ] {
            let (status, digest) =
                manifest_of(&journal, Method::HEAD, (namespace, name, "latest")).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{namespace}/{name}");
            assert_eq!(digest, None, "{namespace}/{name}");
        }
        let seen = journal.manifests.lock().unwrap().clone();
        assert_eq!(seen.len(), 9);
        assert_eq!(seen[0], "HEAD /v2/library/chatonlm-pull/manifests/latest");
        assert_eq!(seen[5], "GET /v2/library/gemma4/manifests/27b");
    }

    #[tokio::test]
    async fn slow_and_small_pulls() {
        let journal = Arc::new(Journal::default());
        let (_, lines, elapsed) = pull_lines(&journal, "chatonlm-pull-slow").await;
        assert_eq!(lines.len(), 50);
        assert_eq!(statuses(&lines).last(), Some(&"success"));
        assert!(elapsed >= Duration::from_millis(100) * 49, "{elapsed:?}");
        let (_, lines, _) = pull_lines(&journal, "chatonlm-pull-small").await;
        let layers: Vec<_> = lines
            .iter()
            .filter(|line| line.get("digest").is_some())
            .collect();
        assert!(layers.iter().all(|line| line["total"] == PULL_TINY_LAYER));
        assert_eq!(layers.last().unwrap()["completed"], PULL_TINY_LAYER);
        assert_eq!(statuses(&lines).last(), Some(&"success"));
        for model in ["chatonlm-pull-slow", "chatonlm-pull-small"] {
            assert_eq!(show_on(&journal, model).await.0, StatusCode::OK, "{model}");
        }
    }

    #[tokio::test]
    async fn failing_pulls() {
        let journal = Arc::new(Journal::default());
        let offline = "pull model manifest: Get \"https://registry.ollama.ai/v2/library/chatonlm-pull-offline/manifests/latest\": dial tcp: lookup registry.ollama.ai: no such host";
        for (model, error) in [
            ("chatonlm-pull-error", PULL_MISSING),
            ("chatonlm-pull-offline", offline),
            ("qwen3:0.6b-unknown", PULL_MISSING),
            ("chatonlm-missing", PULL_MISSING),
        ] {
            let (status, lines, _) = pull_lines(&journal, model).await;
            assert_eq!(status, StatusCode::OK, "{model}");
            assert_eq!(
                lines,
                [
                    serde_json::json!({ "status": "pulling manifest" }),
                    serde_json::json!({ "error": error }),
                ],
                "{model}"
            );
        }
        let (status, lines, _) = pull_lines(&journal, "chatonlm-pull-refused").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            lines,
            [serde_json::json!({ "error": "invalid model name" })]
        );
        // Fin du corps sans `success` : pas installé.
        let (status, lines, _) = pull_lines(&journal, "chatonlm-pull-cut").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[3]["completed"], PULL_TOTAL / 5);
        for model in [
            "chatonlm-pull-error",
            "chatonlm-pull-offline",
            "chatonlm-pull-cut",
        ] {
            assert_eq!(
                show_on(&journal, model).await.0,
                StatusCode::NOT_FOUND,
                "{model}"
            );
        }
        assert_eq!(journal.pulls.lock().unwrap().len(), 6);
    }

    /// Début des flux sans fin normale : coupure, ou lignes vides sans fin.
    #[tokio::test]
    async fn stalled_and_reset_pulls() {
        let journal = Arc::new(Journal::default());
        let request = serde_json::json!({ "model": "chatonlm-pull-stall" }).to_string();
        let response = pull(State(journal.clone()), request).await;
        let mut data = response.into_body().into_data_stream();
        let mut lines = Vec::new();
        for _ in 0..6 {
            let chunk = data.next().await.expect("chunk").expect("body chunk");
            lines.push(String::from_utf8(chunk.to_vec()).expect("UTF-8"));
        }
        assert_eq!(lines[0], "{\"status\":\"pulling manifest\"}\n");
        for (line, percent) in lines[1..4].iter().zip([23, 24, 25]) {
            let line: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(line["completed"], PULL_TOTAL * percent / 100);
        }
        assert_eq!(&lines[4..], ["\n", "\n"]);
        // Flux lâché avant sa fin : abandon compté.
        drop(data);
        assert_eq!(journal.disconnects.load(Ordering::SeqCst), 1);

        let request = serde_json::json!({ "model": "chatonlm-pull-reset" }).to_string();
        let response = pull(State(journal.clone()), request).await;
        let mut data = response.into_body().into_data_stream();
        let mut received = 0;
        let error = loop {
            match data.next().await.expect("chunk or error") {
                Ok(_) => received += 1,
                Err(error) => break error,
            }
        };
        assert_eq!(received, 4);
        assert!(error.to_string().contains("reset"), "{error}");
        // Coupure venue du serveur : pas un abandon du client.
        drop(data);
        assert_eq!(journal.disconnects.load(Ordering::SeqCst), 1);
        assert_eq!(
            show_on(&journal, "chatonlm-pull-reset").await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            show_on(&journal, "chatonlm-pull-stall").await.0,
            StatusCode::NOT_FOUND
        );
    }

    /// Réponse du registre de Hugging Face (espace `chatonlm`) : statut,
    /// `x-error-code` et corps JSON.
    async fn hf_manifest_of(
        journal: &Arc<Journal>,
        repo: &str,
        tag: &str,
        accept: bool,
    ) -> (StatusCode, Option<String>, serde_json::Value) {
        let mut headers = HeaderMap::new();
        if accept {
            headers.insert(
                header::ACCEPT,
                "application/vnd.docker.distribution.manifest.v2+json"
                    .parse()
                    .unwrap(),
            );
        }
        let path = Path(("chatonlm".to_owned(), repo.to_owned(), tag.to_owned()));
        let response = manifest(State(journal.clone()), Method::GET, headers, path).await;
        let status = response.status();
        let code = response
            .headers()
            .get("x-error-code")
            .map(|value| value.to_str().unwrap().to_owned());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        (status, code, body)
    }

    #[tokio::test]
    async fn the_hugging_face_registry_serves_manifests() {
        let journal = Arc::new(Journal::default());
        // Manifeste d'un modèle de vision : modèle, projecteur, gabarit,
        // paramètres, et la configuration ; total = taille sur le disque.
        let (status, code, body) =
            hf_manifest_of(&journal, "Eye-vision-GGUF", "Q4_K_M", true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(code, None);
        let kinds: Vec<&str> = body["layers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|layer| layer["mediaType"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            [
                "application/vnd.ollama.image.model",
                "application/vnd.ollama.image.projector",
                "application/vnd.ollama.image.template",
                "application/vnd.ollama.image.params"
            ]
        );
        let sum: u64 = body["layers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|layer| layer["size"].as_u64().unwrap())
            .sum::<u64>()
            + body["config"]["size"].as_u64().unwrap();
        assert_eq!(sum, hf_total("hf.co/chatonlm/Eye-vision-GGUF:Q4_K_M"));
        let (_, _, plain) = hf_manifest_of(&journal, "Tiny-GGUF", "Q4_K_M", true).await;
        assert_eq!(plain["layers"].as_array().unwrap().len(), 3);
        assert_eq!(
            hf_total("hf.co/chatonlm/Tiny-GGUF:Q4_K_M"),
            HF_MODEL_LAYER + HF_TEMPLATE_LAYER + HF_PARAMS_LAYER + HF_CONFIG
        );
        // Refus exacts du registre.
        for (repo, tag, status, code, start) in [
            (
                "Big-sharded-GGUF",
                "Q8_0",
                400,
                None,
                "This tag is a sharded GGUF.",
            ),
            (
                "Gone-missing-GGUF",
                "Q4_K_M",
                401,
                None,
                "Invalid username or password.",
            ),
            (
                "Lab-restricted-GGUF",
                "Q4_K_M",
                401,
                Some("GatedRepo"),
                "Access to model chatonlm/Lab-restricted-GGUF is restricted.",
            ),
            (
                "Tiny-GGUF",
                "badtag",
                400,
                None,
                "The specified tag is not available",
            ),
            (
                "Tiny-GGUF",
                "BADQUANT",
                400,
                None,
                "The specified tag is not a valid quantization",
            ),
            (
                "Weights-notgguf",
                "latest",
                400,
                None,
                "Repository is not GGUF",
            ),
        ] {
            let (got, got_code, body) = hf_manifest_of(&journal, repo, tag, true).await;
            assert_eq!(got.as_u16(), status, "{repo}:{tag}");
            assert_eq!(got_code.as_deref(), code, "{repo}:{tag}");
            let message = body["error"].as_str().unwrap();
            assert!(message.starts_with(start), "{repo}:{tag}: {message}");
        }
        // Page HTML d'un CDN en panne.
        let (status, _, body) = hf_manifest_of(&journal, "Down-html-GGUF", "Q4_K_M", true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::Value::Null);
        // Sans l'en-tête Accept : refusé.
        let (status, _, _) = hf_manifest_of(&journal, "Tiny-GGUF", "Q4_K_M", false).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            journal.manifests.lock().unwrap()[0],
            "GET /v2/chatonlm/Eye-vision-GGUF/manifests/Q4_K_M"
        );
    }

    #[tokio::test]
    async fn hugging_face_pulls_install_under_the_requested_name() {
        let journal = Arc::new(Journal::default());
        let model = "hf.co/chatonlm/Eye-vision-GGUF:Q4_K_M";
        let (status, lines, _) = pull_lines(&journal, model).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(statuses(&lines)[0], "pulling manifest");
        assert_eq!(
            statuses(&lines[lines.len() - 3..]),
            ["verifying sha256 digest", "writing manifest", "success"]
        );
        // Couches dans l'ordre du manifeste, puis la configuration ; la
        // dernière ligne du modèle reste sous son total.
        let mut totals: Vec<u64> = Vec::new();
        let mut last_model = 0;
        for line in &lines {
            let Some(total) = line["total"].as_u64() else {
                continue;
            };
            if totals.last() != Some(&total) {
                totals.push(total);
            }
            if total == HF_MODEL_LAYER {
                last_model = line["completed"].as_u64().unwrap_or(0);
            }
        }
        assert_eq!(
            totals,
            [
                HF_MODEL_LAYER,
                HF_PROJECTOR_LAYER,
                HF_TEMPLATE_LAYER,
                HF_PARAMS_LAYER,
                HF_CONFIG
            ]
        );
        assert_eq!(last_model, HF_MODEL_LAYER - HF_LAST_LINE_SHORT);
        // Installé sous le nom demandé, étiquette comprise ; le même modèle à
        // la casse près.
        assert_eq!(tag_names(&journal, false), [model]);
        let tags: serde_json::Value = serde_json::from_str(&tags(&journal, false)).unwrap();
        assert_eq!(tags["models"][0]["size"], hf_total(model));
        assert_eq!(
            tags["models"][0]["capabilities"],
            serde_json::json!(["completion", "vision"])
        );
        let (status, body) = show_on(&journal, "HF.CO/chatonlm/eye-vision-gguf:q4_k_m").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["capabilities"],
            serde_json::json!(["completion", "vision"])
        );
        pull_lines(&journal, "hf.co/chatonlm/Tiny-GGUF:Q4_K_M").await;
        assert_eq!(
            tag_names(&journal, false),
            ["hf.co/chatonlm/Tiny-GGUF:Q4_K_M", model]
        );
        // `checksum` : même flux, avec un silence après la vérification (la
        // pause n'est pas envoyée).
        let (status, checksum, elapsed) =
            pull_lines(&journal, "hf.co/chatonlm/Huge-checksum-GGUF:Q4_K_M").await;
        assert_eq!(status, StatusCode::OK);
        // Sans projecteur : cinq lignes de moins que le modèle de vision.
        assert_eq!(checksum.len(), lines.len() - 5);
        assert_eq!(
            statuses(&checksum[checksum.len() - 3..]),
            ["verifying sha256 digest", "writing manifest", "success"]
        );
        assert!(elapsed >= HF_CHECKSUM_PAUSE, "{elapsed:?}");
    }

    #[tokio::test]
    async fn hugging_face_pull_errors_are_the_real_ones() {
        let journal = Arc::new(Journal::default());
        let realm = r#"pull model manifest: realm host "huggingface.co" does not match original host "hf.co""#;
        for (model, error) in [
            (
                "hf.co/chatonlm/Big-sharded-GGUF:Q8_0",
                r#"pull model manifest: 400: {"error":"This tag is a sharded GGUF. Ollama does not yet support pulling sharded GGUF via the registry; please use a single-file quantization tag or \"latest\", or download the shards and merge them locally with `ollama create` (workaround detailed at https://github.com/ollama/ollama/issues/5245)."}"#,
            ),
            ("hf.co/chatonlm/Gone-missing-GGUF:Q4_K_M", realm),
            ("hf.co/chatonlm/Lab-restricted-GGUF:Q4_K_M", realm),
            (
                "hf.co/chatonlm/Tiny-GGUF:badtag",
                r#"pull model manifest: 400: {"error":"The specified tag is not available in the repository. Please use another tag or \"latest\""}"#,
            ),
            (
                "hf.co/chatonlm/Tiny-GGUF:badquant",
                r#"pull model manifest: 400: {"error":"The specified tag is not a valid quantization scheme. Please use another tag or \"latest\""}"#,
            ),
            (
                "hf.co/chatonlm/Weights-notgguf",
                r#"pull model manifest: 400: {"error":"Repository is not GGUF or is not compatible with llama.cpp"}"#,
            ),
        ] {
            let (status, lines, _) = pull_lines(&journal, model).await;
            assert_eq!(status, StatusCode::OK, "{model}");
            assert_eq!(
                lines,
                [
                    serde_json::json!({ "status": "pulling manifest" }),
                    serde_json::json!({ "error": error }),
                ],
                "{model}"
            );
        }
        // Coupé sans `success` : pas installé.
        let (_, lines, _) = pull_lines(&journal, "hf.co/chatonlm/Tiny-cut-GGUF:Q4_K_M").await;
        assert_eq!(lines.len(), 4);
        assert!(tag_names(&journal, false).is_empty());
    }

    #[tokio::test]
    async fn models_are_deleted_like_ollama() {
        let journal = Arc::new(Journal::default());
        pull_lines(&journal, "hf.co/chatonlm/Tiny-GGUF:Q4_K_M").await;
        pull_lines(&journal, "chatonlm-pull-small").await;
        let delete = |model: &str, installed: bool| {
            let body = serde_json::json!({ "model": model }).to_string();
            delete_model(&journal, installed, &body)
        };
        let body_of = |response: Response| async move {
            let status = response.status();
            let headers = response.headers().clone();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (status, headers, String::from_utf8(body.to_vec()).unwrap())
        };
        // 200, corps vide, sans type ; à la casse et à `:latest` près.
        let (status, headers, body) =
            body_of(delete("hf.co/chatonlm/tiny-gguf:q4_k_m", true)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.is_empty());
        assert!(headers.get(header::CONTENT_TYPE).is_none());
        assert_eq!(
            body_of(delete("chatonlm-pull-small:latest", true)).await.0,
            StatusCode::OK
        );
        assert_eq!(body_of(delete("qwen3:0.6b", true)).await.0, StatusCode::OK);
        assert_eq!(tag_names(&journal, true), ["qwen3.5:0.8b"]);
        assert_eq!(
            show_on(&journal, "qwen3:0.6b").await.0,
            StatusCode::NOT_FOUND
        );
        // Une seconde fois, ou sans modèles enregistrés : 404 avec le nom reçu.
        for (model, installed) in [
            ("hf.co/chatonlm/Tiny-GGUF:Q4_K_M", true),
            ("qwen3:0.6b", true),
            ("qwen3.5:0.8b", false),
        ] {
            let (status, _, body) = body_of(delete(model, installed)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{model}");
            let body: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(body["error"], format!("model '{model}' not found"));
        }
        assert_eq!(journal.deletes.lock().unwrap().len(), 6);
    }

    /// Réponse de `/api/chat` à ce corps : code d'état et lignes JSON.
    async fn chat_lines(request: serde_json::Value) -> (StatusCode, Vec<serde_json::Value>) {
        let journal = Arc::new(Journal::default());
        let response = chat(State(journal.clone()), request.to_string()).await;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("UTF-8");
        let lines = body
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON line"))
            .collect();
        assert_eq!(journal.chats.lock().unwrap().as_slice(), [request]);
        (status, lines)
    }

    /// Raisonnement et réponse recollés d'un flux.
    fn joined(lines: &[serde_json::Value]) -> (String, String) {
        let field = |name: &str| {
            lines
                .iter()
                .filter_map(|line| line["message"][name].as_str())
                .collect::<String>()
        };
        (field("thinking"), field("content"))
    }

    #[tokio::test]
    async fn thinking_scenarios_quote_the_think_they_received() {
        let thinking_models = [
            "chatonlm-levels",
            "chatonlm-levels-off",
            "chatonlm-always",
            "chatonlm-nometa",
        ];
        let cases = [
            (None, "absent"),
            (Some(serde_json::json!(true)), "true"),
            (Some(serde_json::json!(false)), "false"),
            (Some(serde_json::json!("high")), "\"high\""),
            (Some(serde_json::Value::Null), "null"),
        ];
        for model in thinking_models
            .iter()
            .chain(&["chatonlm-never", "chatonlm-plain"])
        {
            for (think, label) in &cases {
                let mut request = serde_json::json!({
                    "model": model,
                    "messages": [{ "role": "user", "content": "Hi" }],
                    "stream": true,
                });
                if let Some(think) = think {
                    request["think"] = think.clone();
                }
                let case = format!("{model}, think={label}");
                let (status, lines) = chat_lines(request).await;
                let refused = *model == "chatonlm-plain"
                    && matches!(
                        think,
                        Some(serde_json::Value::Bool(true) | serde_json::Value::String(_))
                    );
                if refused {
                    assert_eq!(status, StatusCode::BAD_REQUEST, "{case}");
                    assert_eq!(
                        lines,
                        [serde_json::json!({
                            "error": "\"chatonlm-plain\" does not support thinking"
                        })],
                        "{case}"
                    );
                    continue;
                }
                assert_eq!(status, StatusCode::OK, "{case}");
                let (last, fragments) = lines.split_last().expect("lines");
                assert_eq!(last["done"], true, "{case}");
                assert!(fragments.iter().all(|line| line["done"] == false), "{case}");
                let (thinking, content) = joined(&lines);
                assert_eq!(content, format!("Received think={label}."), "{case}");
                let thinks = thinking_models.contains(model) && *label != "false";
                assert_eq!(!thinking.is_empty(), thinks, "{case}: {thinking:?}");
                if thinks {
                    assert!(thinking.ends_with(&format!("think={label}.")), "{case}");
                    // Le raisonnement vient d'abord, comme chez Ollama.
                    let first_content = fragments
                        .iter()
                        .position(|line| line["message"]["content"] != "")
                        .expect("content");
                    assert!(
                        fragments[..first_content]
                            .iter()
                            .all(|line| line["message"]["thinking"].is_string()),
                        "{case}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn thinking_scenarios_are_short_and_quick() {
        let request = serde_json::json!({ "model": "chatonlm-levels", "think": "medium" });
        let response = chat(State(Arc::new(Journal::default())), request.to_string()).await;
        let started = Instant::now();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let elapsed = started.elapsed();
        let lines = String::from_utf8(body.to_vec()).unwrap().lines().count();
        // Une ligne toutes les 10 ms (pas toutes d'un coup, pour que le
        // raisonnement ait une durée), bien moins d'une seconde en tout.
        assert!((10..=30).contains(&lines), "{lines}");
        assert!(elapsed >= Duration::from_millis(10) * (lines as u32 - 1));
        assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
    }

    #[tokio::test]
    async fn long_thinking_is_streamed_before_the_answer() {
        let thinking = long_thinking();
        assert!(thinking.starts_with("Step 1: checking"));
        assert!(thinking.ends_with("Step 60: checking one more detail before answering.\n\n"));
        assert_eq!(thinking.matches("Step ").count(), 60);

        // Début du flux seulement (il dure environ 5 s) : raisonnement par
        // morceaux de 24 caractères, sans contenu.
        let request = serde_json::json!({ "model": "chatonlm-longthink" });
        let response = chat(State(Arc::new(Journal::default())), request.to_string()).await;
        let mut data = response.into_body().into_data_stream();
        let mut start = String::new();
        for _ in 0..3 {
            let chunk = data.next().await.expect("chunk").expect("body chunk");
            let line: serde_json::Value = serde_json::from_slice(&chunk).expect("JSON line");
            assert_eq!(line["message"]["content"], "");
            let piece = line["message"]["thinking"].as_str().expect("thinking");
            assert_eq!(piece.chars().count(), 24);
            start.push_str(piece);
        }
        assert!(thinking.starts_with(&start));

        // Tout le flux : raisonnement complet, puis la réponse.
        let lines = reasoned_reply_lines("chatonlm-longthink", &thinking, "Done thinking.", 24);
        let values: Vec<serde_json::Value> = lines
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(joined(&values), (thinking, "Done thinking.".to_owned()));
    }

    #[test]
    fn reasoned_lines_put_the_thinking_first() {
        let contents: Vec<(String, String)> = reasoned_reply_lines("m", "abc", "def", 2)
            .iter()
            .map(|line| {
                let value: serde_json::Value = serde_json::from_str(line).unwrap();
                let message = &value["message"];
                (
                    message["thinking"].as_str().unwrap_or("-").to_owned(),
                    message["content"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        let pairs: Vec<(&str, &str)> = contents
            .iter()
            .map(|(thinking, content)| (thinking.as_str(), content.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [("ab", ""), ("c", ""), ("-", "de"), ("-", "f"), ("-", "")]
        );
    }

    /// Contenu du premier bloc de code ```lang.
    fn fenced_code<'a>(text: &'a str, lang: &str) -> &'a str {
        let fence = format!("```{lang}\n");
        let start = text.find(&fence).expect("code block") + fence.len();
        let end = start + text[start..].find("\n```").expect("closing fence");
        &text[start..end]
    }

    /// Requête HTTP/1.1 brute : code d'état et corps (en octets).
    async fn http_full(addr: SocketAddr, method: &str, path: &str, body: &str) -> (u16, Vec<u8>) {
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        tokio::task::spawn_blocking(move || {
            use std::io::{Read, Write};
            let mut stream = std::net::TcpStream::connect(addr).expect("connect");
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("timeout");
            stream.write_all(request.as_bytes()).expect("write");
            let mut response = Vec::new();
            stream.read_to_end(&mut response).expect("read");
            let end = response
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .expect("headers");
            let head = String::from_utf8_lossy(&response[..end]).into_owned();
            let status = head
                .split_whitespace()
                .nth(1)
                .and_then(|code| code.parse().ok())
                .expect("status");
            (status, response[end + 4..].to_vec())
        })
        .await
        .expect("request")
    }

    /// Code d'état et corps JSON.
    async fn http_json(
        addr: SocketAddr,
        method: &str,
        path: &str,
        body: &str,
    ) -> (u16, serde_json::Value) {
        let (status, body) = http_full(addr, method, path, body).await;
        (status, serde_json::from_slice(&body).expect("JSON body"))
    }

    /// Empreinte et date de publication annoncées par le registre pour un
    /// modèle de l'espace `library`.
    async fn registry_version(journal: &Arc<Journal>, name: &str, tag: &str) -> (String, u64) {
        let path = Path(("library".to_owned(), name.to_owned(), tag.to_owned()));
        let response = manifest(State(journal.clone()), Method::HEAD, HeaderMap::new(), path).await;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .unwrap_or_else(|| panic!("{name} header"))
                .to_str()
                .unwrap()
                .to_owned()
        };
        (
            header("ollama-content-digest"),
            header("ollama-push-time").parse().expect("seconds"),
        )
    }

    /// Empreinte d'un modèle installé (`/api/tags`).
    fn installed_digest(journal: &Journal, model: &str) -> String {
        let tags: serde_json::Value = serde_json::from_str(&tags(journal, true)).unwrap();
        tags["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == model)
            .unwrap_or_else(|| panic!("{model} is not installed"))["digest"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Une version publiée change l'empreinte et la date du registre ; le
    /// modèle installé garde la sienne jusqu'à son prochain téléchargement,
    /// qui prend celle du registre.
    #[tokio::test]
    async fn a_published_version_makes_the_installed_model_older_than_the_registry() {
        let journal = Arc::new(Journal::default());
        pull_lines(&journal, "chatonlm-pull").await;
        let first = installed_digest(&journal, "chatonlm-pull:latest");
        // Jamais republié : même empreinte, publié avant la date des modèles
        // installés (le 4 octobre 2026).
        assert_eq!(
            registry_version(&journal, "chatonlm-pull", "latest").await,
            (first.clone(), REGISTRY_PUSH_TIME)
        );
        // Une étiquette est un autre modèle.
        assert_ne!(
            registry_version(&journal, "chatonlm-pull", "v2").await.0,
            first
        );

        assert_eq!(journal.publish(" chatonlm-pull:latest "), 1);
        let (second, pushed) = registry_version(&journal, "chatonlm-pull", "latest").await;
        assert_ne!(second, first);
        assert_eq!(second.len(), 64);
        // Publié maintenant : après le 4 octobre 2026.
        assert!(pushed > 1_791_100_000, "{pushed}");
        assert_eq!(installed_digest(&journal, "chatonlm-pull:latest"), first);
        // Les autres modèles ne bougent pas.
        assert_eq!(
            registry_version(&journal, "chatonlm-pull-small", "latest")
                .await
                .1,
            REGISTRY_PUSH_TIME
        );

        // Sous son nom complet, comme l'app le demande pour une mise à jour.
        let (status, lines, _) = pull_lines(&journal, "chatonlm-pull:latest").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(lines.last().unwrap()["status"], "success");
        assert_eq!(installed_digest(&journal, "chatonlm-pull:latest"), second);
        // Toujours une seule entrée.
        assert_eq!(tag_names(&journal, false), ["chatonlm-pull:latest"]);
        assert_eq!(journal.publish("CHATONLM-PULL"), 2);
        let third = registry_version(&journal, "chatonlm-pull", "latest")
            .await
            .0;
        assert!(third != second && third != first);
        assert_eq!(installed_digest(&journal, "chatonlm-pull:latest"), second);
    }

    /// Modèle de Hugging Face : son empreinte dans `/api/tags` est le SHA-256
    /// du manifeste que le registre sert (Ollama écrit les octets reçus) ; une
    /// version publiée change ce manifeste.
    #[tokio::test]
    async fn a_hugging_face_model_has_the_digest_of_its_manifest() {
        let journal = Arc::new(Journal::default());
        let model = "hf.co/chatonlm/Tiny-GGUF:Q4_K_M";
        let served = |journal: &Arc<Journal>| {
            let journal = journal.clone();
            async move {
                let mut headers = HeaderMap::new();
                headers.insert(
                    header::ACCEPT,
                    "application/vnd.docker.distribution.manifest.v2+json"
                        .parse()
                        .unwrap(),
                );
                let path = Path((
                    "chatonlm".to_owned(),
                    "Tiny-GGUF".to_owned(),
                    "Q4_K_M".to_owned(),
                ));
                let response = manifest(State(journal), Method::GET, headers, path).await;
                assert_eq!(response.status(), StatusCode::OK);
                assert!(response.headers().get("ollama-content-digest").is_none());
                let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap();
                format!("{:x}", Sha256::digest(&body))
            }
        };
        pull_lines(&journal, model).await;
        let first = served(&journal).await;
        assert_eq!(installed_digest(&journal, model), first);

        journal.publish(model);
        let second = served(&journal).await;
        assert_ne!(second, first);
        assert_eq!(installed_digest(&journal, model), first);
        pull_lines(&journal, model).await;
        assert_eq!(installed_digest(&journal, model), second);
    }

    /// `POST /fake/publish` : la bascule des tests de l'app réelle.
    #[tokio::test]
    async fn a_version_can_be_published_over_http() {
        let fake = FakeOllama::start(Mode::ollama()).await.expect("server");
        let addr = fake.addr();
        let publish = |body: &'static str| http_json(addr, "POST", "/fake/publish", body);
        assert_eq!(
            publish(r#"{"model":"chatonlm-pull"}"#).await,
            (200, serde_json::json!({ "revision": 1 }))
        );
        assert_eq!(
            publish(r#"{"model":"chatonlm-pull:latest"}"#).await.1["revision"],
            2
        );
        for body in ["{}", r#"{"model":""}"#, r#"{"model":3}"#, "nope"] {
            assert_eq!(publish(body).await.0, 400, "{body}");
        }
        assert_eq!(fake.journal.revision("chatonlm-pull"), 2);
        fake.publish("chatonlm-pull");
        assert_eq!(fake.journal.revision("chatonlm-pull"), 3);
        // Journal du registre, lisible par les tests de l'app réelle.
        assert_eq!(
            http_json(addr, "GET", "/fake/manifests", "").await,
            (200, serde_json::json!([]))
        );
        http_full(
            addr,
            "HEAD",
            "/v2/library/chatonlm-pull/manifests/latest",
            "",
        )
        .await;
        assert_eq!(
            http_json(addr, "GET", "/fake/manifests", "").await.1,
            serde_json::json!(["HEAD /v2/library/chatonlm-pull/manifests/latest"])
        );
        fake.stop().await;
    }

    #[tokio::test]
    async fn the_account_can_be_linked_and_signed_out() {
        let fake = FakeOllama::start(Mode::ollama()).await.expect("server");
        let addr = fake.addr();
        let signin = format!("http://{addr}/connect?name=fake-host&key=ZmFrZQ");
        let (status, body) = http_json(addr, "POST", "/api/me", "{}").await;
        assert_eq!(status, 401);
        assert_eq!(
            body,
            serde_json::json!({ "error": "unauthorized", "signin_url": signin })
        );
        let (status, _) = http_json(addr, "POST", "/fake/account", r#"{"signedIn":true}"#).await;
        assert_eq!(status, 200);
        let (status, user) = http_json(addr, "POST", "/api/me", "{}").await;
        assert_eq!(status, 200);
        assert_eq!(user["name"], "ada");
        assert_eq!(user["email"], "ada@example.com");
        assert_eq!(user["plan"], "free");
        assert_eq!(user["avatarurl"], "/fake/avatar.png");
        assert_eq!(user["id"], "00000000-0000-0000-0000-000000000001");
        let (status, body) = http_json(addr, "POST", "/api/signout", "").await;
        assert_eq!((status, body), (200, serde_json::json!({})));
        assert_eq!(http_json(addr, "POST", "/api/me", "{}").await.0, 401);
        // Aussi depuis le test même ; un corps mal formé est refusé.
        fake.set_signed_in(true);
        assert_eq!(http_json(addr, "POST", "/api/me", "{}").await.0, 200);
        // ollama.com injoignable pour le serveur : 503, relié ou non.
        let (status, _) = http_json(
            addr,
            "POST",
            "/fake/account-unavailable",
            r#"{"unavailable":true}"#,
        )
        .await;
        assert_eq!(status, 200);
        let (status, body) = http_json(addr, "POST", "/api/me", "{}").await;
        assert_eq!(
            (status, body),
            (503, serde_json::json!({ "error": "account unavailable" }))
        );
        fake.set_account_unavailable(false);
        assert_eq!(http_json(addr, "POST", "/api/me", "{}").await.0, 200);
        assert_eq!(
            http_json(addr, "POST", "/fake/account", r#"{"signedIn":"yes"}"#)
                .await
                .0,
            400
        );

        let (status, avatar) = http_full(addr, "GET", "/fake/avatar.png", "").await;
        assert_eq!(status, 200);
        assert!(avatar.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(avatar, AVATAR_PNG);
        let (status, _) = http_full(addr, "GET", "/fake/avatar-redirect", "").await;
        assert_eq!(status, 302);
        fake.stop().await;
    }

    #[tokio::test]
    async fn the_cloud_status_can_be_switched_off() {
        let fake = FakeOllama::start(Mode::ollama()).await.expect("server");
        let addr = fake.addr();
        let (status, body) = http_json(addr, "GET", "/api/status", "").await;
        assert_eq!(status, 200);
        assert_eq!(
            body,
            serde_json::json!({ "cloud": { "disabled": false, "source": "none" } })
        );
        let (status, _) = http_json(addr, "POST", "/fake/cloud", r#"{"disabled":true}"#).await;
        assert_eq!(status, 200);
        let (_, body) = http_json(addr, "GET", "/api/status", "").await;
        assert_eq!(
            body,
            serde_json::json!({ "cloud": { "disabled": true, "source": "env" } })
        );
        fake.set_cloud_disabled(false);
        assert_eq!(
            http_json(addr, "GET", "/api/status", "").await.1["cloud"]["disabled"],
            false
        );
        fake.stop().await;
    }

    #[tokio::test]
    async fn web_requests_need_a_linked_account_and_the_cloud() {
        let fake = FakeOllama::start(Mode::ollama()).await.expect("server");
        let addr = fake.addr();
        let search = "/api/experimental/web_search";
        let fetch = "/api/experimental/web_fetch";
        let query = r#"{"query":"ollama release","max_results":5}"#;
        let page = r#"{"url":"https://example.com/page"}"#;
        for (path, body) in [(search, query), (fetch, page)] {
            let (status, answer) = http_json(addr, "POST", path, body).await;
            assert_eq!(status, 401, "{path}");
            assert_eq!(answer, serde_json::json!({ "error": "Unauthorized" }));
        }
        fake.set_signed_in(true);
        let (status, results) = http_json(addr, "POST", search, query).await;
        assert_eq!(status, 200);
        assert_eq!(
            results,
            serde_json::json!({ "results": [
                {
                    "title": "Result 1 for ollama release",
                    "url": "https://example.com/1?q=ollama+release",
                    "content": "Content 1 about ollama release.",
                },
                {
                    "title": "Result 2 for ollama release",
                    "url": "https://example.com/2?q=ollama+release",
                    "content": "Content 2 about ollama release.",
                },
            ] })
        );
        let (_, one) = http_json(addr, "POST", search, r#"{"query":"x","max_results":1}"#).await;
        assert_eq!(one["results"].as_array().unwrap().len(), 1);
        let (status, read) = http_json(addr, "POST", fetch, page).await;
        assert_eq!(status, 200);
        assert_eq!(
            read,
            serde_json::json!({
                "title": "Page https://example.com/page",
                "content": "Text of https://example.com/page.",
                "links": ["https://example.com/next"],
            })
        );
        let (status, huge) = http_full(addr, "POST", search, r#"{"query":"huge"}"#).await;
        assert_eq!(status, 200);
        assert!(huge.len() > WEB_HUGE, "{}", huge.len());
        // Comme le proxy d'Ollama : corps vide refusé, cloud coupé avant le
        // compte.
        let (status, empty) = http_json(addr, "POST", search, "").await;
        assert_eq!(status, 400);
        assert_eq!(empty["error"], "missing request body");
        assert_eq!(http_json(addr, "POST", fetch, "{}").await.0, 400);
        fake.set_cloud_disabled(true);
        fake.set_signed_in(false);
        for (path, operation) in [
            (search, "web search is unavailable"),
            (fetch, "web fetch is unavailable"),
        ] {
            let (status, answer) = http_json(addr, "POST", path, query).await;
            assert_eq!(status, 403, "{path}");
            assert_eq!(
                answer["error"],
                format!("ollama cloud is disabled: {operation}")
            );
        }

        // Journal : chaque requête reçue, dans l'ordre.
        let web = fake.web_requests();
        assert_eq!(web.len(), 10);
        assert_eq!(
            web[0],
            serde_json::json!({ "path": search, "body": { "query": "ollama release", "max_results": 5 } })
        );
        assert_eq!(web[1]["path"], fetch);
        let (_, journal) = http_json(addr, "GET", "/fake/web", "").await;
        assert_eq!(journal, serde_json::Value::Array(web));
        fake.stop().await;
    }

    #[tokio::test]
    async fn a_slow_web_request_can_be_abandoned() {
        let fake = FakeOllama::start(Mode::ollama()).await.expect("server");
        fake.set_signed_in(true);
        let body = r#"{"url":"https://example.com/slow"}"#;
        let request = format!(
            "POST /api/experimental/web_fetch HTTP/1.1\r\nHost: {}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            fake.addr(),
            body.len()
        );
        let stream = tokio::net::TcpStream::connect(fake.addr())
            .await
            .expect("connect");
        stream.writable().await.expect("writable");
        assert_eq!(
            stream.try_write(request.as_bytes()).expect("write"),
            request.len()
        );
        // Pas de réponse avant longtemps ; le client part.
        let answered = tokio::time::timeout(Duration::from_millis(300), stream.readable()).await;
        assert!(answered.is_err(), "the slow request answered");
        drop(stream);
        let polling = async {
            while fake.disconnects() == 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(2), polling)
            .await
            .expect("the abandoned request is counted");
        assert_eq!(fake.web_requests().len(), 1);
        fake.stop().await;
    }

    /// Lignes du scénario `model` pour ces messages, avec ou sans outils.
    async fn tool_lines(
        model: &str,
        messages: serde_json::Value,
        tools: bool,
    ) -> Vec<serde_json::Value> {
        let mut request = serde_json::json!({ "model": model, "messages": messages });
        if tools {
            request["tools"] = serde_json::json!([{ "type": "function" }]);
        }
        let (status, lines) = chat_lines(request).await;
        assert_eq!(status, StatusCode::OK, "{model}");
        assert_eq!(lines.last().unwrap()["done"], true, "{model}");
        lines
    }

    /// Flux de `model` pour ces messages, avec ce tableau `tools` (`null` :
    /// absent).
    async fn tool_lines_with(
        model: &str,
        messages: serde_json::Value,
        tools: serde_json::Value,
    ) -> Vec<serde_json::Value> {
        let mut request = serde_json::json!({ "model": model, "messages": messages });
        if !tools.is_null() {
            request["tools"] = tools;
        }
        let (status, lines) = chat_lines(request).await;
        assert_eq!(status, StatusCode::OK, "{model}");
        assert_eq!(lines.last().unwrap()["done"], true, "{model}");
        lines
    }

    /// Appels d'outils d'un flux : (nom, arguments).
    fn calls(lines: &[serde_json::Value]) -> Vec<(String, serde_json::Value)> {
        lines
            .iter()
            .filter_map(|line| line["message"]["tool_calls"].as_array())
            .flatten()
            .map(|call| {
                (
                    call["function"]["name"].as_str().unwrap().to_owned(),
                    call["function"]["arguments"].clone(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn tool_scenarios_call_the_web_tools() {
        for model in TOOL_SCENARIOS {
            let (status, body) = show_of(model).await;
            assert_eq!(status, StatusCode::OK, "{model}");
            assert_eq!(
                body["capabilities"],
                serde_json::json!(["completion", "tools"])
            );
        }
        let user = |text: &str| serde_json::json!({ "role": "user", "content": text });
        let tool = |text: &str| serde_json::json!({ "role": "tool", "content": text, "tool_name": "web_search" });
        let call = |name: &str, arguments: serde_json::Value| (name.to_owned(), arguments);

        // chatonlm-image : appel de generate_image seulement s'il est offert.
        let image_tool = serde_json::json!([{
            "type": "function",
            "function": { "name": "generate_image" },
        }]);
        let painting = tool_lines_with(
            "chatonlm-image",
            serde_json::json!([user("A red fox")]),
            image_tool.clone(),
        )
        .await;
        assert_eq!(
            calls(&painting),
            [call(
                "generate_image",
                serde_json::json!({ "prompt": "A red fox", "width": 512, "height": 512 })
            )]
        );
        assert_eq!(joined(&painting).0, "The user wants a picture.");
        let painted = tool_lines_with(
            "chatonlm-image",
            serde_json::json!([user("A red fox"), tool(r#"{"attachmentId":1}"#)]),
            image_tool,
        )
        .await;
        assert!(calls(&painted).is_empty());
        assert_eq!(joined(&painted).1, "Here is your image.");
        // Outils web seulement, ou aucun : pas d'image.
        for tools in [
            serde_json::json!([{ "type": "function", "function": { "name": "web_search" } }]),
            serde_json::Value::Null,
        ] {
            let refused = tool_lines_with(
                "chatonlm-image",
                serde_json::json!([user("A red fox")]),
                tools,
            )
            .await;
            assert!(calls(&refused).is_empty());
            assert_eq!(joined(&refused).1, "Image generation is not available.");
        }

        // chatonlm-search : appel, puis réponse qui cite le résultat.
        let asked = tool_lines(
            "chatonlm-search",
            serde_json::json!([user("Latest Ollama?")]),
            true,
        )
        .await;
        assert_eq!(
            calls(&asked),
            [call(
                "web_search",
                serde_json::json!({ "query": "Latest Ollama?" })
            )]
        );
        let line = asked
            .iter()
            .find(|l| l["message"]["tool_calls"].is_array())
            .unwrap();
        assert_eq!(line["message"]["tool_calls"][0]["id"], "call_1");
        assert_eq!(line["message"]["tool_calls"][0]["function"]["index"], 0);
        assert!(!joined(&asked).0.is_empty());
        let answered = tool_lines(
            "chatonlm-search",
            serde_json::json!([user("Latest Ollama?"), tool(r#"{"results":[]}"#)]),
            true,
        )
        .await;
        assert!(calls(&answered).is_empty());
        assert_eq!(joined(&answered).1, r#"Found: {"results":[]}"#);
        let plain = tool_lines("chatonlm-search", serde_json::json!([user("Hi")]), false).await;
        assert!(calls(&plain).is_empty());
        assert_eq!(joined(&plain).1, "Answered without web search.");

        // chatonlm-fetch : l'adresse telle qu'écrite, puis le premier lien,
        // puis la réponse.
        let fetch = "chatonlm-fetch";
        let first = tool_lines(
            fetch,
            serde_json::json!([user("Read https://example.com/a. now")]),
            true,
        )
        .await;
        assert_eq!(
            calls(&first),
            [call(
                "web_fetch",
                serde_json::json!({ "url": "https://example.com/a." })
            )]
        );
        let page = r#"{"title":"A","content":"B","links":["https://example.com/next"]}"#;
        let second = tool_lines(
            fetch,
            serde_json::json!([user("Read https://example.com/a"), tool(page)]),
            true,
        )
        .await;
        assert_eq!(
            calls(&second),
            [call(
                "web_fetch",
                serde_json::json!({ "url": "https://example.com/next" })
            )]
        );
        let third = tool_lines(
            fetch,
            serde_json::json!([user("Read https://example.com/a"), tool(page), tool(page)]),
            true,
        )
        .await;
        assert!(calls(&third).is_empty());
        assert_eq!(joined(&third).1, format!("Found: {page}"));
        let refused = tool_lines(
            fetch,
            serde_json::json!([user("Read https://example.com/a."), tool("Error: no")]),
            true,
        )
        .await;
        assert_eq!(joined(&refused).1, "Found: Error: no");
        // Adresse d'un message plus ancien, l'envoi suivant recompte.
        let older = tool_lines(
            fetch,
            serde_json::json!([
                user("See https://example.com/old"),
                tool(page),
                tool(page),
                user("Again")
            ]),
            true,
        )
        .await;
        assert_eq!(
            calls(&older),
            [call(
                "web_fetch",
                serde_json::json!({ "url": "https://example.com/old" })
            )]
        );
        let none = tool_lines(fetch, serde_json::json!([user("Nothing")]), true).await;
        assert_eq!(joined(&none).1, "No URL to fetch.");
        let plain = tool_lines(
            fetch,
            serde_json::json!([user("https://example.com")]),
            false,
        )
        .await;
        assert_eq!(joined(&plain).1, "Answered without web fetch.");

        // chatonlm-loop : toujours un appel tant qu'il y a des outils.
        let looped = tool_lines(
            "chatonlm-loop",
            serde_json::json!([user("Go"), tool("{}"), tool("{}")]),
            true,
        )
        .await;
        assert_eq!(
            calls(&looped),
            [call("web_search", serde_json::json!({ "query": "loop 3" }))]
        );
        let done = tool_lines(
            "chatonlm-loop",
            serde_json::json!([user("Go"), tool("{}")]),
            false,
        )
        .await;
        assert_eq!(joined(&done).1, "Done");
        let stubborn = tool_lines(
            "chatonlm-loop",
            serde_json::json!([user("Go, stubborn"), tool("{}")]),
            false,
        )
        .await;
        assert_eq!(
            calls(&stubborn),
            [call("web_search", serde_json::json!({ "query": "loop 2" }))]
        );
    }

    /// Corps de `/api/generate` sur ce serveur : code d'état, réponse JSON et
    /// durée.
    async fn generate_on(
        journal: &Arc<Journal>,
        request: serde_json::Value,
    ) -> (StatusCode, serde_json::Value, Duration) {
        let started = Instant::now();
        let response = generate(State(journal.clone()), request.to_string()).await;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let elapsed = started.elapsed();
        (
            status,
            serde_json::from_slice(&body).expect("JSON body"),
            elapsed,
        )
    }

    /// Modèles de `/api/ps` sur ce serveur.
    fn ps_models(journal: &Journal) -> Vec<serde_json::Value> {
        let body: serde_json::Value = serde_json::from_str(&ps(journal)).expect("JSON");
        body["models"].as_array().expect("models").clone()
    }

    /// Secondes d'ici l'échéance `expires_at` (« …+02:00 »), lue à la main.
    fn seconds_left(expires_at: &str) -> i128 {
        let year: i64 = expires_at[..4].parse().unwrap();
        let month: i64 = expires_at[5..7].parse().unwrap();
        let day: i64 = expires_at[8..10].parse().unwrap();
        let hour: i128 = expires_at[11..13].parse().unwrap();
        let minute: i128 = expires_at[14..16].parse().unwrap();
        let second: i128 = expires_at[17..19].parse().unwrap();
        assert!(expires_at.ends_with("+02:00"), "{expires_at}");
        // Jours depuis l'époque, par recherche (le test n'a que civil_from_days).
        let mut days = 0i64;
        while civil_from_days(days) < (year, month, day) {
            days += if civil_from_days(days + 1000) <= (year, month, day) {
                1000
            } else {
                1
            };
        }
        let at = i128::from(days) * 86_400 + hour * 3_600 + minute * 60 + second - 7_200;
        at - now_nanos() / 1_000_000_000
    }

    #[test]
    fn dates_are_written_like_go() {
        assert_eq!(date_text(0, 120), "1970-01-01T02:00:00.000000000+02:00");
        assert_eq!(
            date_text(1_791_291_149_998_419_079, 120),
            "2026-10-06T14:52:29.998419079+02:00"
        );
        // Lendemain d'un 29 février, et le même instant en UTC.
        let leap = 951_867_000 * 1_000_000_000;
        assert_eq!(date_text(leap, 120), "2000-03-01T01:30:00.000000000+02:00");
        assert_eq!(date_text(leap, 0), "2000-02-29T23:30:00.000000000Z");
        assert_eq!(date_text(0, -270), "1969-12-31T19:30:00.000000000-04:30");
        // `keep_alive` négatif, le 6 octobre 2026 : en 2319, comme Ollama.
        let kept = 1_791_291_149_998_419_079 + i128::from(i64::MAX);
        assert!(date_text(kept, 120).starts_with("2319-01-"));
    }

    /// Chargement et déchargement par `/api/generate`, état relu dans
    /// `/api/ps` au format d'Ollama, demandes journalisées.
    #[tokio::test]
    async fn generate_loads_and_unloads_models_listed_by_ps() {
        let journal = Arc::new(Journal::default());
        // Rien en mémoire au départ.
        assert_eq!(ps(&journal), r#"{"models":[]}"#);

        let (status, done, _) = generate_on(
            &journal,
            serde_json::json!({ "model": "qwen3:0.6b", "keep_alive": 90, "options": { "num_ctx": 8192 } }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(done["done_reason"], "load");
        assert_eq!(done["done"], true);
        assert_eq!(done["response"], "");
        let listed = ps_models(&journal);
        assert_eq!(listed.len(), 1);
        let qwen3 = &listed[0];
        assert_eq!(qwen3["name"], "qwen3:0.6b");
        assert_eq!(qwen3["model"], "qwen3:0.6b");
        assert_eq!(qwen3["size"], LOADED_SIZE);
        assert_eq!(qwen3["size_vram"], LOADED_SIZE);
        assert_eq!(qwen3["context_length"], 8192);
        assert_eq!(qwen3["details"]["parameter_size"], "751.63M");
        assert_eq!(qwen3["details"]["quantization_level"], "Q4_K_M");
        let expires = qwen3["expires_at"].as_str().unwrap();
        assert_eq!(expires.len(), "2026-10-06T14:52:29.998419079+02:00".len());
        assert!((88..=90).contains(&seconds_left(expires)), "{expires}");

        // Sans réglage : contexte de 4096, 5 minutes, nom complété ; un
        // contexte au-delà du modèle ramené à son maximum ; -1 : gardé.
        generate_on(&journal, serde_json::json!({ "model": "chatonlm-fast" })).await;
        generate_on(
            &journal,
            serde_json::json!({ "model": "chatonlm-split", "keep_alive": -1, "options": { "num_ctx": 65536 } }),
        )
        .await;
        let listed = ps_models(&journal);
        let names: Vec<_> = listed.iter().map(|m| m["name"].clone()).collect();
        // Échéance la plus lointaine d'abord.
        assert_eq!(
            names,
            [
                "chatonlm-split:latest",
                "chatonlm-fast:latest",
                "qwen3:0.6b"
            ]
        );
        assert_eq!(listed[1]["context_length"], DEFAULT_CONTEXT);
        let fast = listed[1]["expires_at"].as_str().unwrap();
        assert!((298..=300).contains(&seconds_left(fast)), "{fast}");
        assert_eq!(listed[0]["context_length"], 40960);
        let kept = listed[0]["expires_at"].as_str().unwrap();
        assert!(seconds_left(kept) > 290 * 365 * 86_400, "{kept}");
        // En partie sur le CPU.
        assert_eq!(listed[0]["size_vram"], LOADED_SIZE / 4 * 3);

        // Déjà chargé avec ce contexte : sans keep_alive, l'échéance reste.
        generate_on(
            &journal,
            serde_json::json!({ "model": "qwen3:0.6b", "options": { "num_ctx": 8192 } }),
        )
        .await;
        let qwen3 = ps_models(&journal).pop().unwrap();
        assert_eq!(qwen3["expires_at"], expires);

        // Déchargé (keep_alive 0), aussi un modèle installé non chargé.
        for model in ["qwen3:0.6b", "qwen3.5:0.8b"] {
            let (status, done, _) = generate_on(
                &journal,
                serde_json::json!({ "model": model, "keep_alive": 0 }),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{model}");
            assert_eq!(done["done_reason"], "unload", "{model}");
        }
        assert_eq!(ps_models(&journal).len(), 2);

        // Inconnu, ou supprimé avant le chat : 404 comme Ollama.
        for (model, message) in [
            ("nope:1b", "model 'nope:1b' not found"),
            ("chatonlm-vanished", "model 'chatonlm-vanished' not found"),
        ] {
            for keep_alive in [serde_json::json!(null), serde_json::json!(0)] {
                let mut request = serde_json::json!({ "model": model });
                if !keep_alive.is_null() {
                    request["keep_alive"] = keep_alive;
                }
                let (status, error, _) = generate_on(&journal, request).await;
                assert_eq!(status, StatusCode::NOT_FOUND, "{model}");
                assert_eq!(error, serde_json::json!({ "error": message }), "{model}");
            }
        }
        // Le faux serveur ne génère pas de texte.
        let (status, _, _) = generate_on(
            &journal,
            serde_json::json!({ "model": "qwen3:0.6b", "prompt": "Hi" }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Demandes journalisées telles que reçues (pas celle avec un prompt).
        let loads = journal.loads.lock().unwrap().clone();
        assert_eq!(loads.len(), 10);
        assert_eq!(
            loads[..5],
            [
                serde_json::json!({ "model": "qwen3:0.6b", "kind": "load", "keep_alive": 90, "num_ctx": 8192 }),
                serde_json::json!({ "model": "chatonlm-fast", "kind": "load" }),
                serde_json::json!({ "model": "chatonlm-split", "kind": "load", "keep_alive": -1, "num_ctx": 65536 }),
                serde_json::json!({ "model": "qwen3:0.6b", "kind": "load", "num_ctx": 8192 }),
                serde_json::json!({ "model": "qwen3:0.6b", "kind": "unload", "keep_alive": 0 }),
            ]
        );
        // Rien dans le journal du chat (des e2e comptent ses entrées).
        assert!(journal.chats.lock().unwrap().is_empty());
    }

    /// `/api/chat` met le modèle en mémoire comme `/api/generate`.
    #[tokio::test]
    async fn chat_loads_the_model_it_answers_with() {
        let journal = Arc::new(Journal::default());
        let ask = |model: &str, options: serde_json::Value| {
            let mut request = serde_json::json!({ "model": model, "think": false });
            if !options.is_null() {
                request["options"] = options;
            }
            request
        };
        for request in [
            ask("qwen3:0.6b", serde_json::json!({ "num_ctx": 8192 })),
            ask("chatonlm-plain", serde_json::Value::Null),
            // Absents : rien en mémoire.
            ask("chatonlm-vanished", serde_json::Value::Null),
            ask("nope:1b", serde_json::Value::Null),
        ] {
            let response = chat(State(journal.clone()), request.to_string()).await;
            let _ = axum::body::to_bytes(response.into_body(), usize::MAX).await;
        }
        let listed = ps_models(&journal);
        let loaded: Vec<_> = listed
            .iter()
            .map(|m| (m["name"].clone(), m["context_length"].clone()))
            .collect();
        assert_eq!(
            loaded,
            [
                (
                    serde_json::json!("chatonlm-plain:latest"),
                    serde_json::json!(4096)
                ),
                (serde_json::json!("qwen3:0.6b"), serde_json::json!(8192)),
            ]
        );
        // Un autre contexte le recharge ; `keep_alive` du chat suivi.
        let mut again = ask("qwen3:0.6b", serde_json::json!({ "num_ctx": 16384 }));
        again["keep_alive"] = serde_json::json!(-1);
        chat(State(journal.clone()), again.to_string()).await;
        let qwen3 = ps_models(&journal).remove(0);
        assert_eq!(qwen3["context_length"], 16384);
        let kept = qwen3["expires_at"].as_str().unwrap();
        assert!(seconds_left(kept) > 290 * 365 * 86_400, "{kept}");
        // Ni chargement ni déchargement journalisé pour un chat.
        assert!(journal.loads.lock().unwrap().is_empty());
    }

    /// `chatonlm-slowload` met 1,5 s à se charger, une seule fois ;
    /// `chatonlm-loadfail` ne se charge jamais.
    #[tokio::test]
    async fn slow_and_failing_loads() {
        let journal = Arc::new(Journal::default());
        let slow = serde_json::json!({ "model": "chatonlm-slowload" });
        let (status, _, elapsed) = generate_on(&journal, slow.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert!(elapsed >= SLOW_LOAD, "{elapsed:?}");
        assert_eq!(ps_models(&journal).len(), 1);
        let (_, _, elapsed) = generate_on(&journal, slow.clone()).await;
        assert!(elapsed < Duration::from_millis(500), "{elapsed:?}");
        // Le chat ne le recharge pas (même contexte) ; avec un autre, si. Durée
        // jusqu'aux en-têtes (le flux de chatonlm-fast dure environ 2 s).
        let answered = |request: serde_json::Value| {
            let journal = journal.clone();
            async move {
                let started = Instant::now();
                let response = chat(State(journal), request.to_string()).await;
                let elapsed = started.elapsed();
                assert_eq!(response.status(), StatusCode::OK);
                let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body");
                let body = String::from_utf8(body.to_vec()).expect("UTF-8");
                assert!(body.contains(r#""done":true"#), "{body}");
                elapsed
            }
        };
        let elapsed = answered(slow.clone()).await;
        assert!(elapsed < Duration::from_millis(500), "{elapsed:?}");
        let other =
            serde_json::json!({ "model": "chatonlm-slowload", "options": { "num_ctx": 8192 } });
        let elapsed = answered(other).await;
        assert!(elapsed >= SLOW_LOAD, "{elapsed:?}");

        let failing = serde_json::json!({ "model": "chatonlm-loadfail" });
        let (status, error, _) = generate_on(&journal, failing.clone()).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error, serde_json::json!({ "error": LOAD_FAILURE }));
        let (status, lines) = chat_lines_on(&journal, failing).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(lines, [serde_json::json!({ "error": LOAD_FAILURE })]);
        let names: Vec<_> = ps_models(&journal)
            .iter()
            .map(|m| m["name"].clone())
            .collect();
        assert_eq!(names, ["chatonlm-slowload:latest"]);
    }

    /// Lignes JSON de `/api/chat` sur ce serveur.
    async fn chat_lines_on(
        journal: &Arc<Journal>,
        request: serde_json::Value,
    ) -> (StatusCode, Vec<serde_json::Value>) {
        let response = chat(State(journal.clone()), request.to_string()).await;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("UTF-8");
        let lines = body
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON line"))
            .collect();
        (status, lines)
    }

    /// En HTTP : `/api/generate`, `/api/ps` et `/fake/loads` ; `/fake/requests`
    /// ne compte que les chats.
    #[tokio::test]
    async fn loads_over_http() {
        let fake = FakeOllama::start(Mode::ollama()).await.expect("server");
        let load = serde_json::json!({ "model": "qwen3.5:0.8b", "keep_alive": 300 });
        let done = http(fake.addr(), "POST", "/api/generate", &load.to_string()).await;
        assert!(done.contains(r#""done_reason":"load""#), "{done}");
        let listed = http(fake.addr(), "GET", "/api/ps", "").await;
        let listed: serde_json::Value = serde_json::from_str(&listed).expect("JSON");
        assert_eq!(listed["models"][0]["name"], "qwen3.5:0.8b");
        assert_eq!(listed["models"][0]["details"]["quantization_level"], "Q8_0");
        let loads = http(fake.addr(), "GET", "/fake/loads", "").await;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&loads).expect("JSON"),
            serde_json::json!([{ "model": "qwen3.5:0.8b", "kind": "load", "keep_alive": 300 }])
        );
        assert_eq!(fake.load_requests().len(), 1);
        let requests = http(fake.addr(), "GET", "/fake/requests", "").await;
        assert_eq!(requests, "[]");
        fake.stop().await;
    }

    /// Un chargement lent abandonné par le client compte comme un abandon, et
    /// le modèle reste hors de la mémoire.
    #[tokio::test]
    async fn an_abandoned_slow_load_is_counted() {
        let journal = Arc::new(Journal::default());
        let request = serde_json::json!({ "model": "chatonlm-slowload" }).to_string();
        let loading = generate(State(journal.clone()), request);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), loading)
                .await
                .is_err()
        );
        assert_eq!(journal.disconnects.load(Ordering::SeqCst), 1);
        assert!(ps_models(&journal).is_empty());
    }

    /// `chatonlm-longload` met 6 s à se charger ; le modèle distant n'est
    /// jamais en mémoire ici, ni par `/api/generate`, ni par `/api/chat`.
    #[tokio::test]
    async fn long_loads_and_remote_models() {
        let journal = Arc::new(Journal::default());
        let long = generate(
            State(journal.clone()),
            serde_json::json!({ "model": "chatonlm-longload" }).to_string(),
        );
        // Encore en cours après la durée de chatonlm-slowload.
        assert!(
            tokio::time::timeout(SLOW_LOAD + Duration::from_millis(500), long)
                .await
                .is_err()
        );
        let (status, _, elapsed) = generate_on(
            &journal,
            serde_json::json!({ "model": "chatonlm-longload" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(elapsed >= LONG_LOAD, "{elapsed:?}");

        let remote = serde_json::json!({ "model": "chatonlm-polish-remote" });
        let (status, done, _) = generate_on(&journal, remote.clone()).await;
        assert_eq!(
            (status, done["done_reason"].clone()),
            (StatusCode::OK, "load".into())
        );
        chat_lines_on(&journal, remote).await;
        let names: Vec<_> = ps_models(&journal)
            .iter()
            .map(|m| m["name"].clone())
            .collect();
        assert_eq!(names, ["chatonlm-longload:latest"]);
    }

    /// Un modèle supprimé reste en mémoire, et ne se décharge plus (404),
    /// comme Ollama 0.35.1 : ChatonLM le décharge avant de le supprimer.
    #[tokio::test]
    async fn a_deleted_model_stays_in_memory() {
        let journal = Arc::new(Journal::default());
        generate_on(&journal, serde_json::json!({ "model": "qwen3:0.6b" })).await;
        let deleted = delete_model(&journal, true, r#"{"model":"qwen3:0.6b"}"#);
        assert_eq!(deleted.status(), StatusCode::OK);
        let (status, error, _) = generate_on(
            &journal,
            serde_json::json!({ "model": "qwen3:0.6b", "keep_alive": 0 }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(error["error"], "model 'qwen3:0.6b' not found");
        assert_eq!(ps_models(&journal).len(), 1);
    }

    /// Mode conversation vocale : `chatonlm-voice` et `chatonlm-voice-long`
    /// raisonnent par défaut et savent couper le raisonnement, comme
    /// `qwen3:0.6b` (l'envoi vocal doit leur porter `think: false`).
    #[tokio::test]
    async fn voice_scenarios_can_turn_thinking_off() {
        for model in VOICE_SCENARIOS {
            let (status, body) = show_of(model).await;
            assert_eq!(status, StatusCode::OK, "{model}");
            assert_eq!(
                body["capabilities"],
                serde_json::json!(["completion", "thinking"]),
                "{model}"
            );
            assert_eq!(
                body["thinking"],
                serde_json::json!({ "values": [false, true], "default": true }),
                "{model}"
            );
            assert_eq!(body["details"]["format"], "gguf", "{model}");
            // Servis par /api/chat (pas la 404 des noms inconnus), puis en
            // mémoire comme un modèle installé.
            let journal = Arc::new(Journal::default());
            let request = serde_json::json!({ "model": model, "think": false });
            let response = chat(State(journal.clone()), request.to_string()).await;
            assert_eq!(response.status(), StatusCode::OK, "{model}");
            let names: Vec<_> = ps_models(&journal)
                .iter()
                .map(|m| m["name"].clone())
                .collect();
            assert_eq!(names, [format!("{model}:latest")], "{model}");
        }
        // Absents de la liste des modèles installés, comme les autres scénarios.
        let journal = Journal::default();
        assert!(
            !tag_names(&journal, true)
                .iter()
                .any(|n| n.contains("voice"))
        );
    }

    /// Requête d'un envoi vocal : consigne système en tête, puis l'historique.
    fn voice_request(
        model: &str,
        users: usize,
        think: Option<serde_json::Value>,
    ) -> serde_json::Value {
        let mut messages = vec![serde_json::json!({
            "role": "system",
            "content": "You are having a spoken conversation.",
        })];
        for turn in 0..users {
            if turn > 0 {
                messages.push(serde_json::json!({ "role": "assistant", "content": "Réponse." }));
            }
            messages
                .push(serde_json::json!({ "role": "user", "content": format!("Phrase {turn}.") }));
        }
        let mut request = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });
        if let Some(think) = think {
            request["think"] = think;
        }
        request
    }

    #[tokio::test]
    async fn voice_replies_follow_the_turn_without_thinking_when_it_is_off() {
        // Deux phrases françaises courtes par réponse, différentes d'un tour
        // à l'autre.
        assert_ne!(VOICE_REPLIES[0], VOICE_REPLIES[1]);
        for reply in VOICE_REPLIES {
            assert!(reply.chars().count() <= 130, "{reply}");
            assert_eq!(reply.matches(['.', '?', '!']).count(), 2, "{reply}");
            assert!(!reply.contains("  "), "{reply}");
        }
        assert!(VOICE_REPLIES[1].chars().count() >= 2 * VOICE_REPLIES[0].chars().count());
        for (users, expected) in [(1, 0), (2, 1), (3, 0)] {
            let request = voice_request("chatonlm-voice", users, Some(false.into()));
            let (status, lines) = chat_lines(request).await;
            assert_eq!(status, StatusCode::OK, "{users}");
            let (last, fragments) = lines.split_last().expect("lines");
            assert_eq!(last["done"], true);
            assert!(fragments.iter().all(|line| line["done"] == false));
            assert_eq!(
                joined(&lines),
                (String::new(), VOICE_REPLIES[expected].to_owned()),
                "{users} user message(s)"
            );
        }
        // Raisonnement par défaut (think absent, true ou un niveau), avant
        // la réponse, comme chez Ollama.
        for think in [None, Some(true.into()), Some("low".into())] {
            let request = voice_request("chatonlm-voice", 1, think.clone());
            let (status, lines) = chat_lines(request).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                joined(&lines),
                (VOICE_THINKING.to_owned(), VOICE_REPLIES[0].to_owned()),
                "think={think:?}"
            );
            let first_content = lines
                .iter()
                .position(|line| line["message"]["content"] != "")
                .expect("content");
            assert!(
                lines[..first_content]
                    .iter()
                    .all(|line| line["message"]["thinking"].is_string())
            );
        }
    }

    #[tokio::test]
    async fn the_long_voice_reply_is_written_for_seconds() {
        // Plusieurs phrases ; la première arrive vite (moins de 13 morceaux),
        // la dernière marque une réponse complète.
        let reply = VOICE_LONG_REPLY;
        assert!(reply.ends_with(VOICE_LONG_END));
        assert_eq!(reply.matches(VOICE_LONG_END).count(), 1);
        assert!(reply.matches(". ").count() >= 6, "{reply}");
        let first = reply.split_inclusive(". ").next().unwrap();
        assert!(first.chars().count() <= 100, "{first}");
        assert!(!reply.contains("  ") && !reply.contains('\n'));
        // Le flux entier dure au moins 6 s (un délai avant chaque ligne sauf
        // la première).
        let lines = reasoned_reply_lines("chatonlm-voice-long", "", reply, 8);
        assert!(VOICE_LONG_DELAY * (lines.len() as u32 - 1) >= Duration::from_secs(6));
        let values: Vec<serde_json::Value> = lines
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(joined(&values), (String::new(), reply.to_owned()));

        // Début du vrai flux seulement : la même réponse à chaque tour, sans
        // raisonnement quand think vaut false, au rythme annoncé.
        for users in [1, 2] {
            let request = voice_request("chatonlm-voice-long", users, Some(false.into()));
            let response = chat(State(Arc::new(Journal::default())), request.to_string()).await;
            assert_eq!(response.status(), StatusCode::OK);
            let started = Instant::now();
            let mut data = response.into_body().into_data_stream();
            let mut start = String::new();
            for _ in 0..4 {
                let chunk = data.next().await.expect("chunk").expect("body chunk");
                let line: serde_json::Value = serde_json::from_slice(&chunk).expect("JSON line");
                assert!(line["message"].get("thinking").is_none());
                start.push_str(line["message"]["content"].as_str().expect("content"));
            }
            assert!(started.elapsed() >= VOICE_LONG_DELAY * 3);
            assert_eq!(start.chars().count(), 32);
            assert!(reply.starts_with(&start), "{start}");
        }
    }

    /// En HTTP : la requête vocale se relit entière sur `/fake/requests`
    /// (consigne système, `think`), et la longue réponse lâchée en cours de
    /// route compte comme un abandon (l'e2e de l'interruption s'en sert).
    #[tokio::test]
    async fn voice_requests_over_http() {
        let fake = FakeOllama::start(Mode::ollama()).await.expect("server");
        let mut request = voice_request("chatonlm-voice", 1, Some(false.into()));
        request["options"] = serde_json::json!({ "num_ctx": 4096 });
        let reply = http(fake.addr(), "POST", "/api/chat", &request.to_string()).await;
        assert!(reply.contains(r#""done":true"#), "{reply}");
        assert!(!reply.contains("thinking"), "{reply}");
        let journal = http(fake.addr(), "GET", "/fake/requests", "").await;
        let journal: serde_json::Value = serde_json::from_str(&journal).expect("JSON body");
        assert_eq!(journal, serde_json::json!([request]));
        assert_eq!(journal[0]["messages"][0]["role"], "system");
        assert_eq!(journal[0]["think"], false);

        let long = voice_request("chatonlm-voice-long", 1, Some(false.into())).to_string();
        let head = format!(
            "POST /api/chat HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{long}",
            fake.addr(),
            long.len()
        );
        let stream = tokio::net::TcpStream::connect(fake.addr())
            .await
            .expect("connect");
        stream.writable().await.expect("writable");
        assert_eq!(
            stream.try_write(head.as_bytes()).expect("write"),
            head.len()
        );
        // Lit jusqu'au premier morceau de la réponse, puis part.
        let mut received = Vec::new();
        let reading = async {
            while !String::from_utf8_lossy(&received).contains(r#""content":"Voici un"#) {
                stream.readable().await.expect("readable");
                let mut buffer = [0u8; 4096];
                match stream.try_read(&mut buffer) {
                    Ok(0) => panic!("connection closed"),
                    Ok(read) => received.extend_from_slice(&buffer[..read]),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => panic!("read: {error}"),
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(2), reading)
            .await
            .expect("first chunk of the long reply");
        assert_eq!(fake.disconnects(), 0);
        drop(stream);
        let polling = async {
            while fake.disconnects() == 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(2), polling)
            .await
            .expect("the abandoned reply is counted");
        assert_eq!(fake.chat_requests().len(), 2);
        fake.stop().await;
    }
}
