//! Téléchargements du ModelHub (état Tauri `HubDownloads`). Contrat :
//! `37-step15a-contract.md` § 2 et § 8 ; comportement d'Ollama :
//! `34-modelhub-ollama-hf.md` § 1, 5-7.
//!
//! - File FIFO, deux pulls à la fois au plus (`Queued` pour les autres).
//! - Démarrage (`Starting`) d'un nom `hf.co/...` : manifeste lu au registre de
//!   Hugging Face (taille exacte ; 400/401 → `Failed` lisible sans pull ; pas
//!   de réponse ou réponse inattendue → pull quand même).
//! - Pull `POST /api/pull` en flux, sans délai global ; progression
//!   (`chat::pull::Tracker`) = somme des couches sur la taille du manifeste,
//!   sinon (autre nom, manifeste illisible) la plus grosse couche, comme le
//!   pull de l'app officielle : une somme des seules couches vues reculerait
//!   à l'arrivée du projecteur d'un modèle de vision. `Downloading` à la
//!   première couche, `Finishing` à `verifying sha256 digest`, `Done` à
//!   `success` (completed == total). Délai d'inactivité (120 s sans ligne) → `Failed` ; fin de flux
//!   sans `success` → `Failed` « Connection lost ». Écart voulu au contrat :
//!   pas de délai d'inactivité pendant `Finishing` (Ollama vérifie alors
//!   l'empreinte de fichiers de plusieurs Go sans envoyer de ligne).
//! - Annulation : en file → `Cancelled` ; en cours → `Cancelled` tout de
//!   suite, requête fermée (Ollama garde les morceaux `-partial`, un nouveau
//!   pull reprend).
//! - Pause (propre à ChatonLM, hors contrat) : en file ou en cours → `Paused`,
//!   requête fermée, progression gardée à l'écran ; le suivant de la file
//!   démarre. Reprise → `Queued` à sa place : Ollama repart des morceaux
//!   reçus. Rien pendant `Finishing` (tout est reçu). Un téléchargement en
//!   pause ne compte pas comme une activité (`any_active`) et n'est pas
//!   gardé d'une session à l'autre (comme toute la file).
//! - Priorité (« Download first », comme « Download now » de Steam) : le
//!   téléchargement passe en tête de la file et démarre tout de suite ; si
//!   les places sont prises, le dernier en cours (hors `Finishing`) retourne
//!   en file, requête fermée, et reprendra de ses morceaux.
//! - Serveur figé à la demande : si la cible change (`ollama::target`),
//!   les téléchargements vers l'ancienne s'arrêtent (`Failed`, message
//!   explicite ; un nouveau clic tire vers la nouvelle cible).
//! - Événement : liste complète à chaque changement d'état, progression
//!   limitée à 4 émissions par seconde (la dernière n'est jamais perdue).
//! - Débit lissé sur environ 3 s, seulement pendant `Downloading`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::json;
use tokio::sync::watch;
use tokio::time::Instant;
use url::Url;

use super::errors::{self, pull_error};
use super::manifest::{self, Manifest};
use super::names::{self, parse_hf_name};
use super::types::{HubDownload, HubDownloadRequest, HubDownloadState};
use crate::chat::ChatErrorKind;
use crate::chat::pull::{self, Tracker};
use crate::chat::stream::LineBuffer;
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::ollama::{client, models, registry};
use crate::store::now_ms;

/// Réglages de la file (modifiables pour les tests).
#[derive(Clone, Debug)]
pub struct Options {
    /// Pulls simultanés au plus.
    pub max_running: usize,
    /// Plus aucune ligne pendant ce délai (avant `Finishing`) : échec.
    pub idle_timeout: Duration,
    /// Intervalle minimal entre deux émissions de progression.
    pub progress_interval: Duration,
    /// Délai de la lecture du manifeste.
    pub manifest_timeout: Duration,
    /// Adresse de Hugging Face ; `None` : `names::hf_url()` au démarrage de
    /// chaque téléchargement.
    pub hf_url: Option<String>,
    /// Vrai pendant un changement de Settings > Model location
    /// (`commands::models_dir::location_changing`) : un nouveau
    /// téléchargement est refusé (le script arrêterait Ollama sous lui).
    pub location_changing: fn() -> bool,
}

fn never() -> bool {
    false
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_running: 2,
            idle_timeout: Duration::from_secs(120),
            progress_interval: Duration::from_millis(250),
            manifest_timeout: Duration::from_secs(15),
            hf_url: None,
            location_changing: never,
        }
    }
}

/// Fenêtre du débit lissé.
const RATE_WINDOW: Duration = Duration::from_secs(3);

/// Durée minimale observée avant d'annoncer un débit.
const RATE_MIN_SPAN: Duration = Duration::from_millis(500);

/// Taille lue au plus pour le corps d'une réponse d'erreur d'Ollama.
const MAX_ERROR_BODY: usize = 64 * 1024;

type Emit = Box<dyn Fn(&[HubDownload]) + Send + Sync>;

/// Téléchargements en cours, en file et terminés, partagés par les commandes.
pub struct HubDownloads {
    inner: Arc<Inner>,
}

struct Inner {
    options: Options,
    emit: Emit,
    state: Mutex<State>,
    /// Tâches de téléchargement encore en vie (attendues à la sortie).
    live: watch::Sender<usize>,
    next_id: AtomicU64,
}

#[derive(Default)]
struct State {
    /// Dans l'ordre des demandes.
    entries: Vec<Entry>,
    /// Sortie de l'app : plus rien ne démarre.
    closed: bool,
    last_emit: Option<Instant>,
    /// Progression pas encore émise.
    dirty: bool,
    flush_scheduled: bool,
}

struct Entry {
    /// Distingue une entrée de celle qui l'a remplacée (même nom).
    id: u64,
    /// Adresse d'Ollama au moment de la demande.
    server: String,
    download: HubDownload,
    /// Arrêt de la tâche (en cours seulement).
    cancel: Option<watch::Sender<bool>>,
    rate: Rate,
}

/// Fin d'un téléchargement.
#[derive(Debug, PartialEq, Eq)]
enum End {
    Done,
    Failed(String),
    Cancelled,
}

/// Effet d'une mise à jour d'une entrée.
enum Change {
    Nothing,
    Progress,
    State,
}

fn active(state: HubDownloadState) -> bool {
    matches!(
        state,
        HubDownloadState::Queued
            | HubDownloadState::Starting
            | HubDownloadState::Downloading
            | HubDownloadState::Finishing
    )
}

fn running(state: HubDownloadState) -> bool {
    active(state) && state != HubDownloadState::Queued
}

/// Pas encore fini : en file, en cours ou en pause.
fn pending(state: HubDownloadState) -> bool {
    active(state) || state == HubDownloadState::Paused
}

/// Même modèle pour Ollama, qui résout les noms sans la casse.
fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Nom qu'Ollama accepterait : `hf.co/<auteur>/<dépôt>:<étiquette>` valide
/// (toujours avec une étiquette : `latest` tire chez Hugging Face une version
/// imprévisible), ou un autre nom de registre bien formé.
fn valid_name(model: &str) -> bool {
    if manifest::is_hugging_face(model) {
        parse_hf_name(model).is_some_and(|name| name.tag.is_some())
    } else {
        registry::manifest_path(model).is_some()
    }
}

impl HubDownloads {
    /// `emit` reçoit la liste complète à chaque changement.
    pub fn with_options(
        emit: impl Fn(&[HubDownload]) + Send + Sync + 'static,
        options: Options,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                options,
                emit: Box::new(emit),
                state: Mutex::new(State::default()),
                live: watch::channel(0).0,
                next_id: AtomicU64::new(1),
            }),
        }
    }

    /// Téléchargements, dans l'ordre des demandes.
    pub fn list(&self) -> Vec<HubDownload> {
        snapshot(&self.inner.state.lock())
    }

    /// Vrai si un téléchargement est en file ou en cours (un redémarrage
    /// d'Ollama le couperait : Settings > Model location attend).
    pub fn any_active(&self) -> bool {
        self.list().iter().any(|download| active(download.state))
    }

    /// Demande un téléchargement depuis le serveur `server`. Nom invalide, ou
    /// changement de dossier des modèles en cours : refus ; déjà en cours ou
    /// en file : rien ; en pause : repris ; terminé, échoué ou arrêté :
    /// remplacé (en fin de file).
    pub fn download(&self, server: &str, request: HubDownloadRequest) -> AppResult<()> {
        let model = request.model.trim().to_owned();
        if !valid_name(&model) {
            // i18n-ignore : nom choisi dans le catalogue par la page.
            return Err(AppError::Invalid(format!("invalid model name: {model}")));
        }
        let parsed = parse_hf_name(&model);
        let inner = &self.inner;
        let mut state = inner.state.lock();
        if state.closed {
            return Ok(());
        }
        // Sous le verrou de la file : un changement qui commence voit ce
        // téléchargement dans `any_active`, ou ce téléchargement voit le
        // changement.
        if (inner.options.location_changing)() {
            return Err(AppError::Invalid(errors::location_changing()));
        }
        if let Some(index) = state
            .entries
            .iter()
            .position(|entry| same(&entry.download.model, &model))
        {
            if active(state.entries[index].download.state) {
                return Ok(());
            }
            if state.entries[index].download.state == HubDownloadState::Paused {
                state.entries[index].download.state = HubDownloadState::Queued;
                inner.pump(&mut state);
                inner.emit_now(&mut state);
                return Ok(());
            }
            state.entries.remove(index);
        }
        state.entries.push(Entry {
            id: inner.next_id.fetch_add(1, Ordering::Relaxed),
            server: server.to_owned(),
            download: HubDownload {
                repo: request
                    .repo
                    .or_else(|| parsed.as_ref().map(names::HfName::repo_id)),
                label: request
                    .label
                    .or_else(|| parsed.and_then(|parsed| parsed.tag)),
                model,
                state: HubDownloadState::Queued,
                completed: 0,
                total: 0,
                bytes_per_second: None,
                error: None,
                started_at: now_ms(),
                finished_at: None,
            },
            cancel: None,
            rate: Rate::default(),
        });
        inner.pump(&mut state);
        inner.emit_now(&mut state);
        Ok(())
    }

    /// Arrête un téléchargement en file, en cours (requête fermée) ou en
    /// pause ; inconnu ou déjà fini : rien.
    pub fn cancel(&self, model: &str) {
        let inner = &self.inner;
        let mut state = inner.state.lock();
        let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| same(&entry.download.model, model.trim()))
            .filter(|entry| pending(entry.download.state))
        else {
            return;
        };
        finish_entry(entry, End::Cancelled);
        inner.pump(&mut state);
        inner.emit_now(&mut state);
    }

    /// Met en pause un téléchargement en file ou en cours (requête fermée :
    /// Ollama garde les morceaux reçus) ; sa progression reste affichée et le
    /// suivant de la file démarre. Sans effet pendant `Finishing` (tout est
    /// reçu), sur un inconnu ou sur un téléchargement fini.
    pub fn pause(&self, model: &str) {
        let inner = &self.inner;
        let mut state = inner.state.lock();
        let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| same(&entry.download.model, model.trim()))
            .filter(|entry| {
                matches!(
                    entry.download.state,
                    HubDownloadState::Queued
                        | HubDownloadState::Starting
                        | HubDownloadState::Downloading
                )
            })
        else {
            return;
        };
        inner.halt(entry, HubDownloadState::Paused);
        inner.pump(&mut state);
        inner.emit_now(&mut state);
    }

    /// Reprend un téléchargement en pause : de nouveau en file, à sa place
    /// (il démarre s'il y a de la place, et repart des morceaux reçus).
    /// Changement de dossier des modèles en cours : refus. Pas en pause :
    /// rien.
    pub fn resume(&self, model: &str) -> AppResult<()> {
        let inner = &self.inner;
        let mut state = inner.state.lock();
        if state.closed {
            return Ok(());
        }
        let Some(index) = state.entries.iter().position(|entry| {
            same(&entry.download.model, model.trim())
                && entry.download.state == HubDownloadState::Paused
        }) else {
            return Ok(());
        };
        if (inner.options.location_changing)() {
            return Err(AppError::Invalid(errors::location_changing()));
        }
        state.entries[index].download.state = HubDownloadState::Queued;
        inner.pump(&mut state);
        inner.emit_now(&mut state);
        Ok(())
    }

    /// Fait passer un téléchargement en premier : il prend la tête de la
    /// file et démarre tout de suite (en pause : repris). Si toutes les
    /// places sont prises, le dernier téléchargement en cours (hors
    /// `Finishing`) retourne en file, requête fermée : il reprendra de ses
    /// morceaux à la première place libre. Déjà en cours : il passe seulement
    /// en tête. Inconnu ou fini : rien ; reprise refusée pendant un
    /// changement de dossier des modèles.
    pub fn prioritize(&self, model: &str) -> AppResult<()> {
        let inner = &self.inner;
        let mut state = inner.state.lock();
        if state.closed {
            return Ok(());
        }
        let Some(index) = state.entries.iter().position(|entry| {
            same(&entry.download.model, model.trim()) && pending(entry.download.state)
        }) else {
            return Ok(());
        };
        let paused = state.entries[index].download.state == HubDownloadState::Paused;
        if paused && (inner.options.location_changing)() {
            return Err(AppError::Invalid(errors::location_changing()));
        }
        let mut entry = state.entries.remove(index);
        if paused {
            entry.download.state = HubDownloadState::Queued;
        }
        let waiting = entry.download.state == HubDownloadState::Queued;
        state.entries.insert(0, entry);
        let busy = state
            .entries
            .iter()
            .filter(|entry| running(entry.download.state))
            .count();
        if waiting
            && busy >= inner.options.max_running
            && let Some(last) = state.entries.iter_mut().rev().find(|entry| {
                matches!(
                    entry.download.state,
                    HubDownloadState::Starting | HubDownloadState::Downloading
                )
            })
        {
            inner.halt(last, HubDownloadState::Queued);
        }
        inner.pump(&mut state);
        inner.emit_now(&mut state);
        Ok(())
    }

    /// Retire un téléchargement terminé, échoué ou arrêté (ni un actif, ni
    /// un téléchargement en pause : il s'arrête d'abord).
    pub fn dismiss(&self, model: &str) {
        let inner = &self.inner;
        let mut state = inner.state.lock();
        let before = state.entries.len();
        state.entries.retain(|entry| {
            !(same(&entry.download.model, model.trim()) && !pending(entry.download.state))
        });
        if state.entries.len() != before {
            inner.emit_now(&mut state);
        }
    }

    /// La cible des requêtes est maintenant `server` : les téléchargements en
    /// file, en cours ou en pause vers un autre serveur s'arrêtent (requête
    /// fermée), en échec avec un message qui le dit.
    pub fn retarget(&self, server: &str) {
        let inner = &self.inner;
        let mut state = inner.state.lock();
        let mut changed = false;
        for entry in &mut state.entries {
            if pending(entry.download.state) && entry.server != server {
                finish_entry(entry, End::Failed(errors::server_changed()));
                changed = true;
            }
        }
        if changed {
            inner.pump(&mut state);
            inner.emit_now(&mut state);
        }
    }

    /// Sortie de l'app : plus rien ne démarre, les pulls en cours sont fermés
    /// (Ollama garde les morceaux), attente de leur fin, `limit` au plus.
    pub async fn stop_all(&self, limit: Duration) {
        {
            let mut state = self.inner.state.lock();
            state.closed = true;
            for entry in &state.entries {
                if let Some(cancel) = &entry.cancel {
                    let _ = cancel.send(true);
                }
            }
        }
        let mut live = self.inner.live.subscribe();
        let _ = tokio::time::timeout(limit, live.wait_for(|count| *count == 0)).await;
    }
}

/// Copie de la liste à émettre.
fn snapshot(state: &State) -> Vec<HubDownload> {
    state
        .entries
        .iter()
        .map(|entry| entry.download.clone())
        .collect()
}

/// Fin d'une entrée active (l'arrêt de sa tâche est demandé).
fn finish_entry(entry: &mut Entry, end: End) {
    let download = &mut entry.download;
    match end {
        End::Done => {
            download.state = HubDownloadState::Done;
            download.completed = download.total;
        }
        End::Failed(message) => {
            download.state = HubDownloadState::Failed;
            download.error = Some(message);
        }
        End::Cancelled => download.state = HubDownloadState::Cancelled,
    }
    download.bytes_per_second = None;
    download.finished_at = Some(now_ms());
    if let Some(cancel) = entry.cancel.take() {
        let _ = cancel.send(true);
    }
}

impl Inner {
    /// Arrête la tâche d'une entrée sans la finir : l'entrée passe à `state`
    /// (en pause, ou de nouveau en file), garde sa progression, et change de
    /// numéro : la fin de l'ancienne tâche ne la concerne plus.
    fn halt(&self, entry: &mut Entry, state: HubDownloadState) {
        if let Some(cancel) = entry.cancel.take() {
            let _ = cancel.send(true);
        }
        entry.id = self.next_id.fetch_add(1, Ordering::Relaxed);
        entry.download.state = state;
        entry.download.bytes_per_second = None;
        entry.rate = Rate::default();
    }

    /// Démarre les téléchargements en file, dans l'ordre, tant qu'il y a de la
    /// place.
    fn pump(self: &Arc<Self>, state: &mut State) {
        if state.closed {
            return;
        }
        let mut busy = state
            .entries
            .iter()
            .filter(|entry| running(entry.download.state))
            .count();
        for entry in &mut state.entries {
            if busy >= self.options.max_running {
                break;
            }
            if entry.download.state != HubDownloadState::Queued {
                continue;
            }
            busy += 1;
            entry.download.state = HubDownloadState::Starting;
            let (cancel, cancelled) = watch::channel(false);
            entry.cancel = Some(cancel);
            self.live.send_modify(|count| *count += 1);
            let task = Task {
                inner: self.clone(),
                id: entry.id,
                model: entry.download.model.clone(),
                server: entry.server.clone(),
            };
            tauri::async_runtime::spawn(task.run(cancelled));
        }
    }

    /// Émet la liste tout de suite (changement d'état).
    fn emit_now(&self, state: &mut State) {
        state.dirty = false;
        state.last_emit = Some(Instant::now());
        (self.emit)(&snapshot(state));
    }

    /// Progression : émise si la dernière émission date d'au moins
    /// `progress_interval`, sinon à l'échéance.
    fn emit_progress(self: &Arc<Self>, state: &mut State) {
        let now = Instant::now();
        let due = state
            .last_emit
            .map(|last| last + self.options.progress_interval);
        match due {
            Some(due) if due > now => {
                state.dirty = true;
                if !state.flush_scheduled {
                    state.flush_scheduled = true;
                    let inner = self.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep_until(due).await;
                        inner.flush();
                    });
                }
            }
            _ => self.emit_now(state),
        }
    }

    /// Échéance : progression retenue émise (ou de nouveau retenue si une
    /// émission a eu lieu entre-temps).
    fn flush(self: &Arc<Self>) {
        let mut state = self.state.lock();
        state.flush_scheduled = false;
        if state.dirty {
            self.emit_progress(&mut state);
        }
    }

    /// Applique `change` à l'entrée `id` si elle est encore active.
    fn change(self: &Arc<Self>, id: u64, change: impl FnOnce(&mut Entry) -> Change) {
        let mut state = self.state.lock();
        let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && active(entry.download.state))
        else {
            return;
        };
        match change(entry) {
            Change::Nothing => {}
            Change::Progress => self.emit_progress(&mut state),
            Change::State => self.emit_now(&mut state),
        }
    }

    /// Fin de la tâche `id` : l'entrée (si elle est encore active : pas
    /// annulée ni remplacée) prend son état final, la suivante démarre.
    fn finish(self: &Arc<Self>, id: u64, end: End) {
        let mut state = self.state.lock();
        if let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && active(entry.download.state))
        {
            finish_entry(entry, end);
            self.pump(&mut state);
            self.emit_now(&mut state);
        }
    }
}

/// Un téléchargement en cours.
struct Task {
    inner: Arc<Inner>,
    id: u64,
    model: String,
    server: String,
}

impl Task {
    async fn run(self, mut cancel: watch::Receiver<bool>) {
        let end = self.download(&mut cancel).await;
        self.inner.finish(self.id, end);
        self.inner.live.send_modify(|count| *count -= 1);
    }

    async fn download(&self, cancel: &mut watch::Receiver<bool>) -> End {
        if let Some(name) = parse_hf_name(&self.model) {
            let hf_url = self
                .inner
                .options
                .hf_url
                .clone()
                .unwrap_or_else(names::hf_url);
            let timeout = self.inner.options.manifest_timeout;
            let checked = tokio::select! {
                biased;
                () = cancelled(cancel) => return End::Cancelled,
                checked = manifest::fetch(&hf_url, &name, timeout) => checked,
            };
            match checked {
                Manifest::Found { total, .. } => {
                    self.inner.change(self.id, |entry| {
                        entry.download.total = total;
                        Change::Progress
                    });
                    return self.pull(cancel, Some(total)).await;
                }
                Manifest::Refused(message) => return End::Failed(message),
                // Pas de réponse sûre : Ollama dira lui-même ce qui ne va pas.
                Manifest::Unexpected | Manifest::Unreachable(_) => {}
            }
        }
        self.pull(cancel, None).await
    }

    /// `known` : taille du manifeste ; `None` : plus grosse couche.
    async fn pull(&self, cancel: &mut watch::Receiver<bool>, known: Option<u64>) -> End {
        let idle = self.inner.options.idle_timeout;
        let Ok(url) = Url::parse(&self.server) else {
            // i18n-ignore : adresse déjà validée par les réglages.
            return End::Failed(format!("invalid address: {}", self.server));
        };
        let client = match client::client(&url, None) {
            Ok(client) => client,
            Err(error) => return End::Failed(error.to_string()),
        };
        let request = client
            .post(format!("{}/api/pull", self.server))
            .json(&json!({ "model": self.model, "stream": true }))
            .send();
        let response = tokio::select! {
            biased;
            () = cancelled(cancel) => return End::Cancelled,
            response = tokio::time::timeout(idle, request) => response,
        };
        let mut response = match response {
            Err(_) => return End::Failed(errors::timed_out()),
            Ok(Err(error)) if error.is_connect() => {
                return End::Failed(errors::unreachable());
            }
            Ok(Err(error)) => {
                return End::Failed(errors::connection_lost(Some(&error.without_url())));
            }
            Ok(Ok(response)) => response,
        };
        let status = response.status();
        if !status.is_success() {
            let body = tokio::select! {
                biased;
                () = cancelled(cancel) => return End::Cancelled,
                body = tokio::time::timeout(idle, read_limited(&mut response)) => body.unwrap_or_default(),
            };
            let message = models::error_message(&body);
            return End::Failed(if message.trim().is_empty() {
                t!("ollama.httpStatus", status = status.as_u16())
            } else {
                pull_error(&message)
            });
        }
        self.read(response, cancel, Tracker::new(known)).await
    }

    async fn read(
        &self,
        mut response: reqwest::Response,
        cancel: &mut watch::Receiver<bool>,
        mut tracker: Tracker,
    ) -> End {
        let idle = self.inner.options.idle_timeout;
        let mut buffer = LineBuffer::default();
        let mut understood = false;
        let mut finishing = false;
        // Repoussé à chaque ligne non vide.
        let mut deadline = Instant::now() + idle;
        loop {
            let next = async {
                if finishing {
                    Ok(response.chunk().await)
                } else {
                    tokio::time::timeout_at(deadline, response.chunk()).await
                }
            };
            let chunk = tokio::select! {
                biased;
                () = cancelled(cancel) => return End::Cancelled,
                chunk = next => chunk,
            };
            let Ok(chunk) = chunk else {
                return End::Failed(errors::timed_out());
            };
            let ended = matches!(chunk, Ok(None));
            let lines = match chunk {
                Ok(Some(bytes)) => buffer.push(&bytes),
                Ok(None) => buffer.finish().into_iter().collect(),
                Err(error) => {
                    return End::Failed(errors::connection_lost(Some(&error.without_url())));
                }
            };
            for line in lines {
                let progress = match pull::parse(&line, ended, understood) {
                    Ok(progress) => progress,
                    Err((ChatErrorKind::ConnectionLost, _)) => {
                        return End::Failed(errors::connection_lost(None));
                    }
                    Err((_, message)) => return End::Failed(message),
                };
                understood = true;
                deadline = Instant::now() + idle;
                if let Some(error) = progress.error.filter(|error| !error.is_empty()) {
                    return End::Failed(pull_error(&error));
                }
                match progress.status.as_str() {
                    "success" => {
                        let total = tracker.finish();
                        self.inner.change(self.id, |entry| {
                            entry.download.total = entry.download.total.max(total);
                            Change::Nothing
                        });
                        return End::Done;
                    }
                    "verifying sha256 digest" => {
                        // Octets reçus : Ollama vérifie et écrit le manifeste.
                        finishing = true;
                        let total = tracker.finish();
                        self.inner.change(self.id, |entry| {
                            let download = &mut entry.download;
                            download.total = download.total.max(total);
                            download.completed = download.total;
                            download.state = HubDownloadState::Finishing;
                            download.bytes_per_second = None;
                            Change::State
                        });
                    }
                    // Ligne d'une couche, avant la vérification.
                    _ if !progress.digest.is_empty() && !finishing => {
                        if let Some((completed, total)) =
                            tracker.update(&progress.digest, progress.total, progress.completed)
                        {
                            self.inner
                                .change(self.id, |entry| entry.progress(completed, total));
                        }
                    }
                    _ => {}
                }
            }
            if ended {
                // Fin du flux sans `success` : serveur arrêté ou proxy coupé.
                return End::Failed(errors::connection_lost(None));
            }
        }
    }
}

impl Entry {
    /// Nouvelle progression montrée : `Downloading` à la première.
    fn progress(&mut self, completed: u64, total: u64) -> Change {
        let download = &mut self.download;
        download.total = download.total.max(total);
        download.completed = completed.min(download.total);
        download.bytes_per_second = self.rate.add(Instant::now(), download.completed);
        if download.state == HubDownloadState::Starting {
            download.state = HubDownloadState::Downloading;
            Change::State
        } else {
            Change::Progress
        }
    }
}

/// Se termine quand l'arrêt est demandé.
async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    if cancel.wait_for(|cancelled| *cancelled).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Corps d'une réponse d'erreur, borné.
async fn read_limited(response: &mut reqwest::Response) -> String {
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_ERROR_BODY {
            break;
        }
    }
    String::from_utf8_lossy(&body).into_owned()
}

/// Débit moyen sur les ~3 dernières secondes.
#[derive(Default)]
struct Rate {
    samples: VecDeque<(Instant, u64)>,
}

impl Rate {
    /// Ajoute un relevé ; débit en octets par seconde, une fois au moins
    /// 0,5 s observée.
    fn add(&mut self, now: Instant, completed: u64) -> Option<u64> {
        self.samples.push_back((now, completed));
        // Garde le plus récent relevé d'au moins 3 s : la fenêtre reste pleine.
        while self.samples.len() > 2 && now.duration_since(self.samples[1].0) >= RATE_WINDOW {
            self.samples.pop_front();
        }
        let &(start, first) = self.samples.front()?;
        let span = now.duration_since(start);
        if span < RATE_MIN_SPAN {
            return None;
        }
        let bytes = completed.saturating_sub(first) as f64;
        Some((bytes / span.as_secs_f64()).round() as u64)
    }
}

#[cfg(test)]
mod tests;
