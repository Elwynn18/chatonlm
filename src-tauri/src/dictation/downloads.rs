//! Téléchargements des modèles de transcription (dossier
//! `app_data_dir()/speech-models/<id>/`), depuis Hugging Face à une révision
//! épinglée (voir `models::CATALOG`). La même file sert aux voix de la lecture
//! à voix haute (`tts::models`, autre dossier, autre instance) : un catalogue
//! décrit ses éléments par [`DownloadItem`].
//!
//! - File FIFO, deux modèles à la fois au plus, un fichier à la fois par
//!   modèle ; un fichier déjà là à la bonne taille n'est pas retéléchargé.
//! - Chaque fichier arrive dans `<fichier>.part`, repris par `Range` après un
//!   échec ou une annulation ; taille et SHA-256 vérifiés (`Verifying`) avant
//!   le renommage. Empreinte fausse : `.part` supprimé.
//! - Événement : liste complète à chaque changement d'état, progression
//!   limitée à 4 émissions par seconde (la dernière n'est jamais perdue).
//! - Un modèle n'a jamais deux tâches : après une annulation, un nouveau
//!   téléchargement attend en file la vraie fin de la précédente (même
//!   `.part`). Retirer un téléchargement échoué ou annulé supprime ses
//!   `.part` (et le dossier s'il est vide), à la fin de sa tâche ; jamais
//!   ceux d'un fichier qu'un autre téléchargement en cours emploie.
//! - Éléments dynamiques (`Arc<dyn DownloadItem>`) : un catalogue fixe (dictée,
//!   voix) les trouve par `Options::find` ; les modèles d'images, choisis
//!   librement, sont mis en file tels quels ([`SpeechDownloads::enqueue`]).
//!   Chaque fichier dit où il se range sous le dossier de la file.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use tokio::time::Instant;
use ts_rs::TS;
use url::Url;

use super::models::{self, CatalogModel, ModelFile};
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::ollama::client;

/// Événement envoyé à la page à chaque changement de la liste.
pub const DOWNLOADS_EVENT: &str = "speech-downloads";

/// Adresse de Hugging Face.
const DEFAULT_BASE_URL: &str = "https://huggingface.co";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SpeechDownloadState {
    Queued,
    Downloading,
    /// Empreintes en cours de vérification.
    Verifying,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeechDownload {
    /// Identifiant du catalogue.
    pub id: String,
    /// Nom affiché du modèle.
    pub name: String,
    pub state: SpeechDownloadState,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    /// Message d'erreur (en anglais) si `Failed`.
    pub error: Option<String>,
}

/// Fichier d'un élément : ce qu'on attend, où le chercher, où le ranger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemFile {
    /// Nom montré dans les messages (chemin dans le dépôt).
    pub path: String,
    pub size: u64,
    /// Empreinte SHA-256 attendue, en hexadécimal.
    pub sha256: String,
    /// Emplacement, relatif au dossier de la file.
    pub local: PathBuf,
    /// Adresse du fichier.
    pub url: String,
}

/// Élément téléchargeable : un modèle et ses fichiers, téléchargés dans
/// l'ordre.
pub trait DownloadItem: Send + Sync {
    /// Identifiant stable (celui de la liste et des commandes).
    fn id(&self) -> &str;
    /// Nom affiché.
    fn name(&self) -> &str;
    /// Fichiers ; `base` : adresse imposée par les essais, pour tous les
    /// fichiers (sinon l'hôte réel de chacun).
    fn files(&self, base: Option<&str>) -> Vec<ItemFile>;
    fn size(&self) -> u64 {
        self.files(None)
            .iter()
            .fold(0, |sum, file| sum.saturating_add(file.size))
    }
}

/// Fichiers d'un modèle d'un catalogue fixe : `<dossier>/<id>/<chemin>`,
/// adresse donnée par `url`.
pub fn catalog_files(
    id: &str,
    files: &[ModelFile],
    url: impl Fn(&ModelFile) -> String,
) -> Vec<ItemFile> {
    files
        .iter()
        .map(|file| ItemFile {
            path: file.path.to_owned(),
            size: file.size,
            sha256: file.sha256.to_owned(),
            local: Path::new(id).join(file.path),
            url: url(file),
        })
        .collect()
}

impl DownloadItem for CatalogModel {
    fn id(&self) -> &str {
        self.id
    }

    fn name(&self) -> &str {
        self.name
    }

    fn files(&self, base: Option<&str>) -> Vec<ItemFile> {
        catalog_files(self.id, self.files, |file| {
            file_url(base.unwrap_or(DEFAULT_BASE_URL), self, file)
        })
    }
}

/// Recherche d'un élément par identifiant.
pub type Find = Arc<dyn Fn(&str) -> Option<Arc<dyn DownloadItem>> + Send + Sync>;

/// Contrôle fait au démarrage de la tâche d'un élément (place libre...) :
/// `Err(message)` la fait échouer avec ce message. Reçoit le dossier de la
/// file.
pub type BeforeStart = Arc<dyn Fn(&dyn DownloadItem, &Path) -> Result<(), String> + Send + Sync>;

/// Catalogue de la dictée.
fn find_speech_model(id: &str) -> Option<Arc<dyn DownloadItem>> {
    models::find(id).map(|model| Arc::new(*model) as Arc<dyn DownloadItem>)
}

/// Réglages de la file (modifiables pour les tests).
#[derive(Clone)]
pub struct Options {
    /// Modèles téléchargés en même temps au plus.
    pub max_running: usize,
    /// Intervalle minimal entre deux émissions de progression.
    pub progress_interval: Duration,
    /// Adresse imposée à tous les fichiers ; `None` : la variable `test_env`
    /// (essais seulement) ou l'hôte réel de chaque fichier, lue à chaque
    /// téléchargement.
    pub base_url: Option<String>,
    /// Variable d'environnement qui remplace les hôtes réels (essais, debug et
    /// e2e seulement).
    pub test_env: &'static str,
    /// Plus aucun octet reçu pendant ce délai : échec (reprise possible).
    pub read_timeout: Duration,
    /// Catalogue.
    pub find: Find,
    /// Vérification d'un fichier reçu : taille et SHA-256.
    pub verify: fn(&Path, u64, &str) -> std::io::Result<bool>,
    /// Une tâche annulée qui n'a pas encore fini compte dans `max_running`
    /// (modèles d'images : deux modèles d'une famille ne doivent jamais
    /// écrire la même pièce en même temps).
    pub count_ending_tasks: bool,
    /// Contrôle au démarrage de chaque tâche.
    pub before_start: Option<BeforeStart>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_running: 2,
            progress_interval: Duration::from_millis(250),
            base_url: None,
            read_timeout: Duration::from_secs(60),
            test_env: "CHATONLM_SPEECH_MODELS_URL",
            find: Arc::new(find_speech_model),
            verify,
            count_ending_tasks: false,
            before_start: None,
        }
    }
}

/// Adresse imposée par la variable `name` si elle est définie et non vide
/// (faux serveur des essais ; jamais lue par le build de production).
pub fn test_base_url(name: &str) -> Option<String> {
    test_override(name)
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(any(test, debug_assertions, feature = "e2e"))]
fn test_override(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(not(any(test, debug_assertions, feature = "e2e")))]
fn test_override(_name: &str) -> Option<String> {
    None
}

/// Adresse d'un fichier du catalogue.
pub fn file_url(base: &str, model: &CatalogModel, file: &ModelFile) -> String {
    format!(
        "{base}/{}/resolve/{}/{}",
        model.repo, model.revision, file.path
    )
}

type Emit = Box<dyn Fn(&[SpeechDownload]) + Send + Sync>;
type Installed = Box<dyn Fn(&str) + Send + Sync>;

/// Téléchargements en cours, en file et terminés (géré par Tauri).
pub struct SpeechDownloads {
    inner: Arc<Inner>,
}

struct Inner {
    options: Options,
    emit: Emit,
    /// Dossier des modèles de transcription.
    root: PathBuf,
    state: Mutex<State>,
    /// Tâches encore en vie (attendues à la sortie).
    live: watch::Sender<usize>,
    next_id: AtomicU64,
    /// Appelé quand un modèle vient d'être installé (voir
    /// [`SpeechDownloads::on_installed`]).
    installed: OnceLock<Installed>,
}

#[derive(Default)]
struct State {
    entries: Vec<Entry>,
    /// Modèles dont une tâche vit encore (même annulée).
    busy: HashSet<String>,
    /// Modèles dont les `.part` sont à supprimer à la fin de leur tâche.
    discard: HashMap<String, Arc<dyn DownloadItem>>,
    /// Sortie de l'app : plus rien ne démarre.
    closed: bool,
    last_emit: Option<Instant>,
    dirty: bool,
    flush_scheduled: bool,
}

struct Entry {
    /// Distingue une entrée de celle qui l'a remplacée (même modèle).
    key: u64,
    model: Arc<dyn DownloadItem>,
    download: SpeechDownload,
    cancel: Option<watch::Sender<bool>>,
}

#[derive(Debug, PartialEq, Eq)]
enum End {
    Done,
    Failed(String),
    Cancelled,
}

fn active(state: SpeechDownloadState) -> bool {
    matches!(
        state,
        SpeechDownloadState::Queued
            | SpeechDownloadState::Downloading
            | SpeechDownloadState::Verifying
    )
}

impl SpeechDownloads {
    /// `emit` reçoit la liste complète à chaque changement ; `root` : dossier
    /// des modèles de transcription.
    pub fn new(emit: impl Fn(&[SpeechDownload]) + Send + Sync + 'static, root: PathBuf) -> Self {
        Self::with_options(emit, root, Options::default())
    }

    pub fn with_options(
        emit: impl Fn(&[SpeechDownload]) + Send + Sync + 'static,
        root: PathBuf,
        options: Options,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                options,
                emit: Box::new(emit),
                root,
                state: Mutex::new(State::default()),
                live: watch::channel(0).0,
                next_id: AtomicU64::new(1),
                installed: OnceLock::new(),
            }),
        }
    }

    /// `installed` reçoit l'identifiant de chaque modèle dont le
    /// téléchargement vient de finir (`Done`), une fois la liste émise : la
    /// dictée renvoie alors son statut (zone de notification « prête »).
    /// Réglé une fois, au démarrage.
    pub fn on_installed(&self, installed: impl Fn(&str) + Send + Sync + 'static) {
        let _ = self.inner.installed.set(Box::new(installed));
    }

    /// Dossier des modèles de transcription.
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    pub fn list(&self) -> Vec<SpeechDownload> {
        snapshot(&self.inner.state.lock())
    }

    /// Vrai si le modèle `id` est en file ou en cours de téléchargement.
    pub fn is_active(&self, id: &str) -> bool {
        self.inner
            .state
            .lock()
            .entries
            .iter()
            .any(|entry| entry.model.id() == id && active(entry.download.state))
    }

    /// Fichiers (chemins complets) des téléchargements en file ou en cours,
    /// et de ceux dont la tâche vit encore : à ne pas supprimer.
    pub fn files_in_use(&self) -> HashSet<PathBuf> {
        let state = self.inner.state.lock();
        in_use(&self.inner.root, &state, None)
    }

    /// Tâche du modèle `id` encore en vie, jusqu'à `ended` (tests).
    #[cfg(test)]
    pub(crate) fn simulate_task(&self, id: &str) {
        self.inner.state.lock().busy.insert(id.to_owned());
    }

    /// Fin d'une tâche simulée par `simulate_task` (tests).
    #[cfg(test)]
    pub(crate) fn end_task(&self, id: &str) {
        self.inner.task_ended(id);
    }

    /// Attend la vraie fin de la tâche du modèle `id` (annulée, elle peut
    /// encore écrire son `.part` un instant), `limit` au plus. Faux si elle
    /// vit encore.
    pub async fn wait_idle(&self, id: &str, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if !self.inner.state.lock().busy.contains(id) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Demande le téléchargement du modèle `id`. Inconnu : refus ; déjà en
    /// cours ou en file : rien ; terminé, échoué ou annulé : remplacé (en fin
    /// de file).
    /// Un identifiant que le catalogue ne connaît pas reprend l'élément de son
    /// entrée finie, échouée ou annulée (« Try again ») s'il y en a une.
    pub fn download(&self, id: &str) -> AppResult<()> {
        let model = (self.inner.options.find)(id)
            .or_else(|| {
                let state = self.inner.state.lock();
                state
                    .entries
                    .iter()
                    .find(|entry| entry.model.id() == id)
                    .map(|entry| entry.model.clone())
            })
            // i18n-ignore : identifiant du catalogue, toujours connu de la page.
            .ok_or_else(|| AppError::Invalid(format!("unknown speech model: {id}")))?;
        self.enqueue(model);
        Ok(())
    }

    /// Met `model` en file ; déjà en cours ou en file (même identifiant) :
    /// rien ; terminé, échoué ou annulé : remplacé (en fin de file).
    pub fn enqueue(&self, model: Arc<dyn DownloadItem>) {
        let inner = &self.inner;
        let mut state = inner.state.lock();
        if state.closed {
            return;
        }
        if let Some(index) = state
            .entries
            .iter()
            .position(|entry| entry.model.id() == model.id())
        {
            if active(state.entries[index].download.state) {
                return;
            }
            state.entries.remove(index);
        }
        let download = SpeechDownload {
            id: model.id().into(),
            name: model.name().into(),
            state: SpeechDownloadState::Queued,
            completed_bytes: 0,
            total_bytes: model.size(),
            error: None,
        };
        state.entries.push(Entry {
            key: inner.next_id.fetch_add(1, Ordering::Relaxed),
            model,
            download,
            cancel: None,
        });
        inner.pump(&mut state);
        inner.emit_now(&mut state);
    }

    /// Arrête un téléchargement en file ou en cours (`.part` gardé pour la
    /// reprise) ; inconnu ou fini : rien.
    pub fn cancel(&self, id: &str) {
        let inner = &self.inner;
        let mut state = inner.state.lock();
        let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.model.id() == id && active(entry.download.state))
        else {
            return;
        };
        finish_entry(entry, End::Cancelled);
        inner.pump(&mut state);
        inner.emit_now(&mut state);
    }

    /// Retire un téléchargement fini, échoué ou annulé (pas un actif).
    /// Échoué ou annulé : ses `.part` sont supprimés (tout de suite, ou à la
    /// fin de sa tâche si elle vit encore).
    pub fn dismiss(&self, id: &str) {
        let inner = &self.inner;
        let mut state = inner.state.lock();
        let Some(index) = state
            .entries
            .iter()
            .position(|entry| entry.model.id() == id && !active(entry.download.state))
        else {
            return;
        };
        let entry = state.entries.remove(index);
        if matches!(
            entry.download.state,
            SpeechDownloadState::Failed | SpeechDownloadState::Cancelled
        ) {
            if state.busy.contains(entry.model.id()) {
                state
                    .discard
                    .insert(entry.model.id().to_owned(), entry.model.clone());
            } else {
                let keep = in_use(&inner.root, &state, Some(entry.model.id()));
                discard_parts(&inner.root, entry.model.as_ref(), &keep);
            }
        }
        inner.emit_now(&mut state);
    }

    /// Sortie de l'app : plus rien ne démarre, les téléchargements en cours
    /// s'arrêtent (`.part` gardés), attente de leur fin, `limit` au plus.
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

/// Fichiers employés par les entrées actives et les tâches en vie, sauf
/// celles de `except`.
fn in_use(root: &Path, state: &State, except: Option<&str>) -> HashSet<PathBuf> {
    state
        .entries
        .iter()
        .filter(|entry| Some(entry.model.id()) != except)
        .filter(|entry| active(entry.download.state) || state.busy.contains(entry.model.id()))
        .flat_map(|entry| entry.model.files(None))
        .map(|file| root.join(file.local))
        .collect()
}

fn snapshot(state: &State) -> Vec<SpeechDownload> {
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
            download.state = SpeechDownloadState::Done;
            download.completed_bytes = download.total_bytes;
        }
        End::Failed(message) => {
            download.state = SpeechDownloadState::Failed;
            download.error = Some(message);
        }
        End::Cancelled => download.state = SpeechDownloadState::Cancelled,
    }
    if let Some(cancel) = entry.cancel.take() {
        let _ = cancel.send(true);
    }
}

impl Inner {
    /// Démarre les téléchargements en file, dans l'ordre, tant qu'il y a de la
    /// place.
    /// Vrai si un téléchargement a démarré.
    fn pump(self: &Arc<Self>, state: &mut State) -> bool {
        if state.closed {
            return false;
        }
        let mut started = false;
        let mut busy = if self.options.count_ending_tasks {
            state.busy.len()
        } else {
            state
                .entries
                .iter()
                .filter(|entry| {
                    active(entry.download.state)
                        && entry.download.state != SpeechDownloadState::Queued
                })
                .count()
        };
        let State {
            entries,
            busy: tasks,
            ..
        } = state;
        for entry in entries.iter_mut() {
            if busy >= self.options.max_running {
                break;
            }
            // La tâche d'avant (annulée) n'a pas fini : même `.part`.
            if entry.download.state != SpeechDownloadState::Queued
                || tasks.contains(entry.model.id())
            {
                continue;
            }
            busy += 1;
            started = true;
            tasks.insert(entry.model.id().to_owned());
            entry.download.state = SpeechDownloadState::Downloading;
            let (cancel, cancelled) = watch::channel(false);
            entry.cancel = Some(cancel);
            self.live.send_modify(|count| *count += 1);
            let task = Task {
                inner: self.clone(),
                key: entry.key,
                model: entry.model.clone(),
            };
            tauri::async_runtime::spawn(task.run(cancelled));
        }
        started
    }

    /// Fin de la tâche du modèle : `.part` supprimés si son téléchargement a
    /// été retiré entre-temps, puis le suivant (du même modèle compris)
    /// peut démarrer.
    fn task_ended(self: &Arc<Self>, id: &str) {
        let mut state = self.state.lock();
        state.busy.remove(id);
        if let Some(model) = state.discard.remove(id) {
            let keep = in_use(&self.root, &state, Some(id));
            discard_parts(&self.root, model.as_ref(), &keep);
        }
        if self.pump(&mut state) {
            self.emit_now(&mut state);
        }
    }

    fn emit_now(&self, state: &mut State) {
        state.dirty = false;
        state.last_emit = Some(Instant::now());
        (self.emit)(&snapshot(state));
    }

    /// Progression : émise si la dernière émission date d'au moins
    /// `progress_interval`, sinon à l'échéance.
    fn emit_progress(self: &Arc<Self>, state: &mut State) {
        let now = Instant::now();
        match state
            .last_emit
            .map(|last| last + self.options.progress_interval)
        {
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

    fn flush(self: &Arc<Self>) {
        let mut state = self.state.lock();
        state.flush_scheduled = false;
        if state.dirty {
            self.emit_progress(&mut state);
        }
    }

    /// Met à jour l'entrée `key` si elle est encore active ; `progress` :
    /// émission limitée, sinon immédiate.
    fn change(
        self: &Arc<Self>,
        key: u64,
        progress: bool,
        change: impl FnOnce(&mut SpeechDownload),
    ) {
        let mut state = self.state.lock();
        let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.key == key && active(entry.download.state))
        else {
            return;
        };
        change(&mut entry.download);
        if progress {
            self.emit_progress(&mut state);
        } else {
            self.emit_now(&mut state);
        }
    }

    fn finish(self: &Arc<Self>, key: u64, end: End) {
        let mut state = self.state.lock();
        let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.key == key && active(entry.download.state))
        else {
            return;
        };
        let installed = (end == End::Done).then(|| entry.model.id().to_owned());
        finish_entry(entry, end);
        self.pump(&mut state);
        self.emit_now(&mut state);
        drop(state);
        if let (Some(id), Some(installed)) = (installed, self.installed.get()) {
            installed(&id);
        }
    }
}

/// Un téléchargement en cours.
struct Task {
    inner: Arc<Inner>,
    key: u64,
    model: Arc<dyn DownloadItem>,
}

impl Task {
    async fn run(self, mut cancel: watch::Receiver<bool>) {
        let end = self.download(&mut cancel).await;
        self.inner.finish(self.key, end);
        self.inner.task_ended(self.model.id());
        self.inner.live.send_modify(|count| *count -= 1);
    }

    async fn download(&self, cancel: &mut watch::Receiver<bool>) -> End {
        let options = &self.inner.options;
        let base = options
            .base_url
            .clone()
            .or_else(|| test_base_url(options.test_env));
        let files = self.model.files(base.as_deref());
        let root = &self.inner.root;
        let mut dirs: Vec<PathBuf> = files
            .iter()
            .filter_map(|file| root.join(&file.local).parent().map(Path::to_path_buf))
            .collect();
        dirs.dedup();
        for dir in &dirs {
            if let Err(error) = tokio::fs::create_dir_all(dir).await {
                return End::Failed(t!("speechDownloads.createFolder", error = error));
            }
        }
        if let Some(check) = &options.before_start
            && let Err(message) = check(self.model.as_ref(), root)
        {
            return End::Failed(message);
        }
        let client = match Url::parse(base.as_deref().unwrap_or(DEFAULT_BASE_URL))
            .map_err(|error| error.to_string())
            .and_then(|url| {
                client::builder(&url, None)
                    .connect_timeout(Duration::from_secs(15))
                    .read_timeout(self.inner.options.read_timeout)
                    .build()
                    .map_err(|error| error.to_string())
            }) {
            Ok(client) => client,
            Err(error) => return End::Failed(t!("speechDownloads.failed", error = error)),
        };
        let mut before = 0;
        for file in &files {
            let target = root.join(&file.local);
            let present = tokio::fs::metadata(&target)
                .await
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() == file.size);
            if !present
                && let Err(end) = self
                    .fetch(&client, &file.url, file, &target, before, cancel)
                    .await
            {
                return end;
            }
            before = before.saturating_add(file.size);
            self.inner.change(self.key, true, |download| {
                download.completed_bytes = before;
            });
        }
        End::Done
    }

    /// Télécharge `file` dans `<target>.part` (repris s'il existe), vérifie
    /// puis renomme. `before` : octets des fichiers précédents.
    async fn fetch(
        &self,
        client: &reqwest::Client,
        url: &str,
        file: &ItemFile,
        target: &Path,
        before: u64,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<(), End> {
        let part = part_path(target);
        let mut have = tokio::fs::metadata(&part)
            .await
            .map_or(0, |metadata| metadata.len());
        if have > file.size {
            let _ = tokio::fs::remove_file(&part).await;
            have = 0;
        }
        self.inner.change(self.key, false, |download| {
            download.state = SpeechDownloadState::Downloading;
            download.completed_bytes = before.saturating_add(have);
        });
        if have < file.size {
            self.receive(client, url, file, &part, have, before, cancel)
                .await?;
        }

        self.inner.change(self.key, false, |download| {
            download.state = SpeechDownloadState::Verifying;
        });
        let checked = part.clone();
        let expected = file.sha256.clone();
        let size = file.size;
        let check = self.inner.options.verify;
        let verify = tauri::async_runtime::spawn_blocking(move || check(&checked, size, &expected));
        let verified = tokio::select! {
            biased;
            () = cancelled(cancel) => return Err(End::Cancelled),
            verified = verify => verified,
        };
        match verified {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => {
                let _ = tokio::fs::remove_file(&part).await;
                return Err(End::Failed(t!("speechDownloads.damaged", file = file.path)));
            }
            Ok(Err(error)) => {
                return Err(End::Failed(t!(
                    "speechDownloads.checkFailed",
                    file = file.path,
                    error = error
                )));
            }
            Err(error) => {
                return Err(End::Failed(t!(
                    "speechDownloads.checkFailed",
                    file = file.path,
                    error = error
                )));
            }
        }
        tokio::fs::rename(&part, target)
            .await
            .map_err(|error| save_failed(file, &error))
    }

    /// Reçoit la suite de `part` (`have` octets déjà là).
    #[allow(clippy::too_many_arguments)]
    async fn receive(
        &self,
        client: &reqwest::Client,
        url: &str,
        file: &ItemFile,
        part: &Path,
        mut have: u64,
        before: u64,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<(), End> {
        let interrupted = |error: &dyn std::fmt::Display| {
            End::Failed(t!("speechDownloads.interrupted", error = error))
        };
        let mut request = client.get(url);
        if have > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let response = tokio::select! {
            biased;
            () = cancelled(cancel) => return Err(End::Cancelled),
            response = request.send() => response,
        };
        let mut response = match response {
            Ok(response) => response,
            Err(error) if error.is_connect() => {
                return Err(End::Failed(t!("speechDownloads.unreachable")));
            }
            Err(error) => return Err(interrupted(&error.without_url())),
        };
        let status = response.status();
        let append = match status.as_u16() {
            // 200 : le serveur renvoie tout le fichier (reprise ignorée).
            200 => false,
            206 => have > 0,
            // Morceau déjà complet ou plus valable : on repart de zéro.
            416 => {
                let _ = tokio::fs::remove_file(part).await;
                return Err(End::Failed(t!("speechDownloads.resumeRefused")));
            }
            code => return Err(End::Failed(t!("speechDownloads.http", status = code))),
        };
        if !append {
            have = 0;
        }
        let mut output = match tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(part)
            .await
        {
            Ok(output) => output,
            Err(error) => {
                return Err(save_failed(file, &error));
            }
        };
        loop {
            let chunk = tokio::select! {
                biased;
                () = cancelled(cancel) => {
                    let _ = output.flush().await;
                    return Err(End::Cancelled);
                }
                chunk = response.chunk() => chunk,
            };
            let bytes = match chunk {
                Ok(Some(bytes)) => bytes,
                Ok(None) => break,
                Err(error) => {
                    let _ = output.flush().await;
                    return Err(interrupted(&error.without_url()));
                }
            };
            if have + bytes.len() as u64 > file.size {
                drop(output);
                let _ = tokio::fs::remove_file(part).await;
                return Err(End::Failed(t!(
                    "speechDownloads.tooLarge",
                    file = file.path
                )));
            }
            if let Err(error) = output.write_all(&bytes).await {
                return Err(save_failed(file, &error));
            }
            have += bytes.len() as u64;
            let completed = before.saturating_add(have);
            self.inner.change(self.key, true, |download| {
                download.completed_bytes = completed;
            });
        }
        if let Err(error) = output.flush().await {
            return Err(save_failed(file, &error));
        }
        if have < file.size {
            return Err(interrupted(&t!("speechDownloads.connectionClosed")));
        }
        Ok(())
    }
}

/// Fichier `file` impossible à écrire ; `error` : détail technique.
fn save_failed(file: &ItemFile, error: &dyn std::fmt::Display) -> End {
    End::Failed(t!(
        "speechDownloads.saveFailed",
        file = file.path,
        error = error
    ))
}

/// Supprime les `.part` de `model` (sauf ceux des fichiers de `keep`), puis
/// leurs dossiers devenus vides, jusqu'au dossier de la file (exclu).
fn discard_parts(root: &Path, model: &dyn DownloadItem, keep: &HashSet<PathBuf>) {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for file in model.files(None) {
        let target = root.join(&file.local);
        if keep.contains(&target) {
            continue;
        }
        let _ = std::fs::remove_file(part_path(&target));
        for dir in target.ancestors().skip(1) {
            if dir == root || !dir.starts_with(root) {
                break;
            }
            if !dirs.iter().any(|known| known == dir) {
                dirs.push(dir.to_path_buf());
            }
        }
    }
    // Les plus profonds d'abord.
    dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    for dir in dirs {
        let _ = std::fs::remove_dir(&dir);
    }
}

fn part_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    target.with_file_name(name)
}

/// Taille et empreinte SHA-256 (en hexadécimal) de `path`.
fn verify(path: &Path, size: u64, sha256: &str) -> std::io::Result<bool> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    if file.metadata()?.len() != size {
        return Ok(false);
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(hex.eq_ignore_ascii_case(sha256))
}

/// Se termine quand l'arrêt est demandé.
async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    if cancel.wait_for(|cancelled| *cancelled).await.is_err() {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests;
