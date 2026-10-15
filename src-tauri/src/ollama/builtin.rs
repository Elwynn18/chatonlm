// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Inspired by Ollama (https://github.com/ollama/ollama), file
// app/server/server.go (cmd, cleanup, stop, openRotatingLog), commit
// 42e911bc3d05: `ollama serve` run by the app with OLLAMA_* added to a copy of
// its environment, a pid file to clean up a server left over by a previous
// session, SIGTERM then SIGKILL after 5 s, one previous log kept.
// Modified for ChatonLM: a server ChatonLM did not start is never stopped (no
// reapServers); a free port of 127.0.0.1 (never 11434 or 11435) and HOME and
// OLLAMA_MODELS under the app's data; the server gets its own process group
// (its runners inherit it) and PR_SET_PDEATHSIG, from a thread that lives as
// long as the app, so that no process outlives ChatonLM; a leftover is stopped
// only when it is the built-in Ollama (its private HOME); the environment is
// cleaned like the one of the desktop's programs (AppImage); restarts with a
// growing delay, failure after 5 crashes in 2 minutes or no answer within
// 30 s; a chosen models folder that doesn't exist is never created (the
// caller falls back to the default one, like cmd()); the wait after SIGKILL
// is bounded (a process stuck in the kernel never freezes ChatonLM); after
// shutdown() nothing restarts it until resume(). On Windows (builtin/windows.rs)
// a Job Object closed with ChatonLM replaces the process group and
// PR_SET_PDEATHSIG, and the stop ends the whole job at once.

//! Ollama intégré : `ollama serve` lancé et surveillé par ChatonLM (mode
//! hybride, étape 18b ; contrat `.tools/research/51-step18b-contract.md`).
//!
//! - Binaire : `<ressources>/ollama/bin/ollama` (paquets :
//!   `/usr/lib/chatonlm/ollama/bin/ollama`), version dans
//!   `<ressources>/ollama/VERSION` ; builds de débogage et d'essai :
//!   `CHATONLM_OLLAMA_BIN` le remplace (faux binaire `fake-ollama serve`).
//! - Données : `<données>/ollama/home` (son `HOME` : clé
//!   `.ollama/id_ed25519`, rien dans le vrai `$HOME`), `<données>/ollama/models`
//!   (dossier des modèles par défaut), `<données>/ollama/ollama.pid` ;
//!   journal `<données>/logs/ollama.log` (le précédent en `.1`).
//! - Sous-processus : groupe à lui (`setpgid`), `PR_SET_PDEATHSIG(SIGTERM)`
//!   posé avant `exec` depuis un fil dédié (le signal suit le FIL qui a créé
//!   l'enfant) ; un fil par processus attend sa fin sans la récolter
//!   (`waitid(WNOWAIT)`), tue ce qui reste du groupe (runners), puis la
//!   récolte : le numéro du groupe reste réservé jusque-là.
//! - Prêt quand `GET /api/version` répond (30 s au plus, sinon échec) ;
//!   plantage : relance après 1, 2, 4, 8, 16 s, échec au 5e en 2 min.
//! - Arrêt : SIGTERM au groupe, 5 s, puis SIGKILL ; récolte attendue 2 s au
//!   plus ensuite (au-delà : abandon journalisé, le fil de surveillance
//!   récoltera plus tard, le fichier pid reste pour le ménage suivant).
//! - [`shutdown`] (fermeture, installation d'une mise à jour) : arrêt borné,
//!   puis plus aucun démarrage jusqu'à [`resume`].
//!
//! Ce module est le superviseur, commun aux deux systèmes (états, port,
//! environnement, journal, relances, pause, fermeture). Ce qui dépend du
//! système est monté sous le nom `os` : `builtin/unix.rs` (ce qui est décrit
//! ci-dessus : groupe, signaux, `/proc`) et `builtin/windows.rs`
//! (`<ressources>\ollama\ollama.exe`, Job Object fermé avec ChatonLM,
//! `CREATE_NO_WINDOW`, `USERPROFILE` privé, arrêt par `TerminateJobObject`).

use std::collections::{BTreeMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::{oneshot, watch};
use ts_rs::TS;

use super::client::{self, Probe};
use crate::i18n::t;

#[cfg(unix)]
#[path = "builtin/unix.rs"]
mod os;
#[cfg(windows)]
#[path = "builtin/windows.rs"]
mod os;

use os::{cleanup_leftovers, is_executable, watch_exit};
// Aussi pour les essais (`use super::*`).
pub(crate) use os::terminate;

/// Outils du sous-processus, partagés avec le moteur d'images (`image_gen`) :
/// mêmes garanties (rien ne survit à ChatonLM, arrêt borné), propres à chaque
/// système (`builtin/unix.rs`, `builtin/windows.rs`).
pub(crate) mod process {
    #[cfg(all(test, windows))]
    pub(crate) use super::os::alive;
    pub(crate) use super::os::{
        desktop_changes, is_executable, isolate, scrub, terminate, watch_exit,
    };
    pub(crate) use super::{Spawner, describe_exit, pick_port, rotate_log, spawn_with};
}

/// Ports jamais pris : celui d'Ollama et celui de l'instance de test.
const RESERVED_PORTS: [u16; 2] = [11434, 11435];

/// État de l'Ollama intégré.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BuiltinState {
    /// Pas lancé (ou arrêté).
    Stopped,
    /// Lancé, `/api/version` ne répond pas encore (ou relance après un
    /// plantage).
    Starting,
    Running,
    /// Abandonné : plantages répétés, pas de réponse dans le délai, ou
    /// lancement impossible ; relancé seulement à la demande.
    Failed,
}

/// Ce que l'on sait de l'intégré.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub state: BuiltinState,
    /// `http://127.0.0.1:<port>` depuis le premier lancement.
    pub url: Option<String>,
    /// Version servie (`/api/version`), une fois prêt.
    pub version: Option<String>,
    /// Raison d'un échec (langue de l'interface au moment de l'échec).
    pub failure: Option<String>,
    /// Processus `ollama serve` en cours.
    pub pid: Option<u32>,
    /// Dossier des modèles du lancement en cours.
    pub models_dir: Option<PathBuf>,
}

impl Snapshot {
    fn stopped() -> Snapshot {
        Snapshot {
            state: BuiltinState::Stopped,
            url: None,
            version: None,
            failure: None,
            pid: None,
            models_dir: None,
        }
    }
}

/// Délais (raccourcis par les essais).
#[derive(Clone, Debug)]
pub struct Timing {
    pub ready_timeout: Duration,
    pub ready_poll: Duration,
    /// Attente avant la relance qui suit le n-ième plantage récent.
    pub backoff: Vec<Duration>,
    pub crash_window: Duration,
    pub max_crashes: usize,
    pub stop_grace: Duration,
    /// Attente de la fin du processus après SIGKILL (au-delà : abandon).
    pub kill_wait: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            ready_timeout: Duration::from_secs(30),
            ready_poll: Duration::from_millis(200),
            backoff: [1, 2, 4, 8, 16].map(Duration::from_secs).to_vec(),
            crash_window: Duration::from_secs(120),
            max_crashes: 5,
            stop_grace: Duration::from_secs(5),
            kill_wait: Duration::from_secs(2),
        }
    }
}

/// Réglages du sous-processus.
#[derive(Clone, Debug)]
pub struct Config {
    /// Binaire ; `None` : intégré indisponible.
    pub bin: Option<PathBuf>,
    /// Arguments (`serve`).
    pub args: Vec<OsString>,
    /// `<données>/ollama`.
    pub root: PathBuf,
    /// `<données>/logs/ollama.log`.
    pub log: PathBuf,
    /// Variables ajoutées en dernier (essais).
    pub extra_env: Vec<(OsString, OsString)>,
    pub timing: Timing,
}

impl Config {
    /// Réglages de l'app : `data_dir` = dossier des données.
    pub fn new(bin: Option<PathBuf>, data_dir: &Path) -> Config {
        Config {
            bin,
            args: vec!["serve".into()],
            root: data_dir.join("ollama"),
            log: data_dir.join("logs").join("ollama.log"),
            extra_env: Vec::new(),
            timing: Timing::default(),
        }
    }

    /// `HOME` du serveur.
    pub fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    pub fn pid_file(&self) -> PathBuf {
        self.root.join("ollama.pid")
    }

    /// Dossier des modèles quand aucun n'est choisi.
    pub fn default_models_dir(&self) -> PathBuf {
        self.root.join("models")
    }

    /// Dossier d'installation (`bin/`, `lib/`, `VERSION`), canonique.
    pub fn install_root(&self) -> Option<PathBuf> {
        let bin = fs::canonicalize(self.bin.as_ref()?).ok()?;
        os::install_root(&bin)
    }
}

/// Binaire de l'intégré sous `resource_dir` ; builds de débogage et d'essai :
/// `CHATONLM_OLLAMA_BIN` (chemin absolu ; vide : aucun) le remplace.
pub fn locate(resource_dir: Option<&Path>) -> Option<PathBuf> {
    #[cfg(any(debug_assertions, feature = "e2e"))]
    if let Some(path) = std::env::var_os("CHATONLM_OLLAMA_BIN") {
        let path = PathBuf::from(path);
        return (path.is_absolute() && is_executable(&path)).then_some(path);
    }
    let bin = os::bundled_bin(&resource_dir?.join("ollama"));
    is_executable(&bin).then_some(bin)
}

/// Ollama intégré, partagé par l'app.
#[derive(Clone)]
pub struct Builtin {
    inner: Arc<Inner>,
}

struct Inner {
    config: Config,
    snapshot: watch::Sender<Snapshot>,
    run: Mutex<Option<Run>>,
    /// Fil qui crée les processus, lancé au premier démarrage.
    spawner: Mutex<Option<Spawner>>,
    /// Ménage d'un reste d'une session précédente fait (verrou tenu
    /// pendant : le premier lancement l'attend).
    cleaned: Mutex<bool>,
    /// Dernier port, repris à la relance s'il est libre.
    last_port: Mutex<Option<u16>>,
    /// Numéro du dernier lancement.
    generation: AtomicU64,
    /// Nombre de suspensions des démarrages en cours (changement du dossier
    /// des modèles) ; observable : les requêtes attendent la fin.
    paused: watch::Sender<usize>,
    /// Fermé par [`shutdown`] : plus aucun démarrage jusqu'à [`resume`].
    closed: AtomicBool,
}

/// Surveillance en cours (ou finie, jusqu'au prochain lancement).
struct Run {
    generation: u64,
    stop: watch::Sender<bool>,
    done: watch::Receiver<bool>,
}

/// Tant qu'il existe, l'intégré ne démarre pas (voir [`Builtin::pause`]).
pub struct Paused(Arc<Inner>);

impl Drop for Paused {
    fn drop(&mut self) {
        self.0
            .paused
            .send_modify(|count| *count = count.saturating_sub(1));
    }
}

impl Builtin {
    pub fn new(config: Config) -> Builtin {
        Builtin {
            inner: Arc::new(Inner {
                config,
                snapshot: watch::channel(Snapshot::stopped()).0,
                run: Mutex::new(None),
                spawner: Mutex::new(None),
                cleaned: Mutex::new(false),
                last_port: Mutex::new(None),
                generation: AtomicU64::new(0),
                paused: watch::channel(0).0,
                closed: AtomicBool::new(false),
            }),
        }
    }

    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    /// Un binaire est là.
    pub fn available(&self) -> bool {
        self.inner.config.bin.as_deref().is_some_and(is_executable)
    }

    /// Version livrée (`VERSION` du dossier d'installation), sinon celle
    /// servie par l'intégré une fois prêt.
    pub fn version(&self) -> Option<String> {
        self.inner
            .config
            .install_root()
            .and_then(|root| fs::read_to_string(root.join("VERSION")).ok())
            .map(|text| text.trim().trim_start_matches('v').to_owned())
            .filter(|version| !version.is_empty())
            .or_else(|| self.snapshot().version)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.inner.snapshot.borrow().clone()
    }

    /// Changements d'état. La référence rendue par `wait_for` ou `borrow`
    /// bloque tout changement tant qu'elle vit : la lâcher aussitôt.
    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.inner.snapshot.subscribe()
    }

    /// Lance l'intégré s'il est arrêté, sur `models_dir` ; renvoie son
    /// adresse (`None` : indisponible ; arrêté, la dernière, pour
    /// l'affichage). En échec, rien n'est relancé (voir
    /// [`Builtin::restart`]) ; déjà lancé (ou en cours d'arrêt), le dossier
    /// ne change pas ; suspendu ([`Builtin::pause`]) ou fermé
    /// ([`Builtin::close`]), rien ne démarre.
    pub fn ensure_started(&self, models_dir: &Path) -> Option<String> {
        if !self.available() {
            return None;
        }
        let mut run = self.inner.run.lock();
        let active = run.as_ref().is_some_and(|run| !*run.done.borrow());
        let snapshot = self.snapshot();
        if active || snapshot.state == BuiltinState::Failed || self.paused() || self.is_closed() {
            return snapshot.url;
        }
        let port = match pick_port(*self.inner.last_port.lock()) {
            Ok(port) => port,
            Err(error) => {
                self.inner.snapshot.send_modify(|snapshot| {
                    snapshot.state = BuiltinState::Failed;
                    snapshot.failure =
                        Some(t!("builtinOllama.failed.spawn", error = error.to_string()));
                });
                return snapshot.url;
            }
        };
        let url = url_for(port);
        self.inner.snapshot.send_modify(|snapshot| {
            *snapshot = Snapshot {
                state: BuiltinState::Starting,
                url: Some(url.clone()),
                models_dir: Some(models_dir.to_path_buf()),
                ..Snapshot::stopped()
            };
        });
        let (stop, stop_rx) = watch::channel(false);
        let (done_tx, done) = watch::channel(false);
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *run = Some(Run {
            generation,
            stop,
            done,
        });
        spawn(supervise(
            self.inner.clone(),
            models_dir.to_path_buf(),
            port,
            stop_rx,
            Done(done_tx),
        ));
        Some(url)
    }

    /// Arrête l'intégré (SIGTERM au groupe, puis SIGKILL après le délai) et
    /// attend la fin ; l'état devient `Stopped`, même après un échec. Pendant
    /// l'arrêt, rien ne le relance ; un lancement demandé juste après est
    /// gardé.
    pub async fn stop(&self) {
        let stopping = {
            let run = self.inner.run.lock();
            run.as_ref().map(|run| {
                let _ = run.stop.send(true);
                (run.generation, run.done.clone())
            })
        };
        let generation = match stopping {
            Some((generation, mut done)) => {
                let _ = done.wait_for(|done| *done).await;
                Some(generation)
            }
            None => None,
        };
        let mut run = self.inner.run.lock();
        if run.as_ref().map(|run| run.generation) != generation {
            return;
        }
        *run = None;
        self.inner.snapshot.send_modify(|snapshot| {
            snapshot.state = BuiltinState::Stopped;
            snapshot.failure = None;
            snapshot.pid = None;
            snapshot.version = None;
        });
    }

    /// Ménage d'un intégré laissé par une session précédente (fichier pid),
    /// au démarrage de l'app, même si l'intégré ne sert pas ; fait une fois
    /// (le premier lancement l'attend). Bloquant.
    pub fn clean_leftovers(&self) {
        if self.available() {
            self.inner.clean_leftovers();
        }
    }

    /// Suspend les démarrages jusqu'à la fin du garde (changement du dossier
    /// des modèles : aucune requête ne relance l'intégré sur l'ancien).
    pub fn pause(&self) -> Paused {
        self.inner.paused.send_modify(|count| *count += 1);
        Paused(self.inner.clone())
    }

    /// Démarrages suspendus en ce moment.
    pub fn paused(&self) -> bool {
        *self.inner.paused.borrow() > 0
    }

    /// Attend la fin des suspensions en cours (aussitôt s'il n'y en a pas).
    pub async fn wait_unpaused(&self) {
        let mut paused = self.inner.paused.subscribe();
        let _ = paused.wait_for(|count| *count == 0).await;
    }

    /// Plus aucun démarrage jusqu'à [`Builtin::reopen`] (voir [`shutdown`]).
    pub fn close(&self) {
        self.inner.closed.store(true, Ordering::SeqCst);
    }

    /// Annule [`Builtin::close`] : l'intégré peut de nouveau démarrer.
    pub fn reopen(&self) {
        self.inner.closed.store(false, Ordering::SeqCst);
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    /// Fermeture : plus aucun démarrage, puis arrêt borné (SIGTERM, délai,
    /// SIGKILL, attente bornée, plus une marge) ; au-delà, abandonné (la
    /// sortie de ChatonLM envoie SIGTERM par PDEATHSIG, le fichier pid sert au
    /// ménage du lancement suivant).
    pub async fn shutdown(&self) {
        self.close();
        let timing = &self.inner.config.timing;
        let limit = timing.stop_grace + timing.kill_wait + Duration::from_secs(1);
        if tokio::time::timeout(limit, self.stop()).await.is_err() {
            // i18n-ignore : journal.
            eprintln!("chatonlm: the built-in Ollama didn't stop in time; leaving it");
        }
    }

    /// Arrête puis relance (Retry après un échec, nouveau dossier des
    /// modèles).
    pub async fn restart(&self, models_dir: &Path) -> Option<String> {
        self.stop().await;
        self.ensure_started(models_dir)
    }

    /// Attend la fin du démarrage en cours, `limit` au plus.
    pub async fn wait_ready(&self, limit: Duration) -> Snapshot {
        let mut changes = self.subscribe();
        let _ = tokio::time::timeout(
            limit,
            changes.wait_for(|snapshot| snapshot.state != BuiltinState::Starting),
        )
        .await;
        self.snapshot()
    }
}

/// Envoie `true` à la fin de la surveillance, même si sa tâche est
/// abandonnée.
struct Done(watch::Sender<bool>);

impl Drop for Done {
    fn drop(&mut self) {
        let _ = self.0.send(true);
    }
}

pub(crate) fn spawn(future: impl std::future::Future<Output = ()> + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(future);
        }
        Err(_) => {
            tauri::async_runtime::spawn(future);
        }
    }
}

fn url_for(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// Port libre de 127.0.0.1 : le précédent s'il l'est encore, sinon un port
/// choisi par le système, jamais ceux d'Ollama ou de l'instance de test.
pub(crate) fn pick_port(previous: Option<u16>) -> io::Result<u16> {
    if let Some(port) = previous.filter(|port| !RESERVED_PORTS.contains(port))
        && TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
    {
        return Ok(port);
    }
    for _ in 0..16 {
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
            .local_addr()?
            .port();
        if !RESERVED_PORTS.contains(&port) {
            return Ok(port);
        }
    }
    Err(io::Error::other("no free port on 127.0.0.1"))
}

/// Fin du démarrage.
enum Phase {
    Ready(String),
    End(End),
}

/// Ce qui a mis fin à un lancement.
enum End {
    NotReady,
    Exited(Option<ExitStatus>),
    Stop,
}

async fn supervise(
    inner: Arc<Inner>,
    models_dir: PathBuf,
    mut port: u16,
    mut stop: watch::Receiver<bool>,
    _done: Done,
) {
    let config = inner.config.clone();
    let timing = &config.timing;
    let cleaning = inner.clone();
    let _ = tokio::task::spawn_blocking(move || cleaning.clean_leftovers()).await;
    rotate_log(&config.log);
    let mut crashes: VecDeque<Instant> = VecDeque::new();
    let fail = |failure: String| {
        inner.snapshot.send_modify(|snapshot| {
            snapshot.state = BuiltinState::Failed;
            snapshot.failure = Some(failure);
            snapshot.pid = None;
        });
    };
    loop {
        *inner.last_port.lock() = Some(port);
        let url = url_for(port);
        let child = match prepare(&config, port, &models_dir) {
            Ok(command) => inner.spawn(command).await,
            Err(error) => Err(error),
        };
        let child = match child {
            Ok(child) => child,
            Err(error) => {
                fail(t!("builtinOllama.failed.spawn", error = error.to_string()));
                return;
            }
        };
        let pid = child.id();
        let _ = fs::write(config.pid_file(), pid.to_string());
        let mut exited = watch_exit(child);
        inner.snapshot.send_modify(|snapshot| {
            snapshot.state = BuiltinState::Starting;
            snapshot.url = Some(url.clone());
            snapshot.pid = Some(pid);
            snapshot.version = None;
            snapshot.failure = None;
        });

        let phase = tokio::select! {
            biased;
            () = stopped(&mut stop) => Phase::End(End::Stop),
            status = &mut exited => Phase::End(End::Exited(status.ok().flatten())),
            ready = answer(&url, timing.ready_timeout, timing.ready_poll) => match ready {
                Some(version) => Phase::Ready(version),
                None => Phase::End(End::NotReady),
            },
        };
        let end = match phase {
            Phase::Ready(version) => {
                inner.snapshot.send_modify(|snapshot| {
                    snapshot.state = BuiltinState::Running;
                    snapshot.version = Some(version);
                });
                tokio::select! {
                    biased;
                    () = stopped(&mut stop) => End::Stop,
                    status = &mut exited => End::Exited(status.ok().flatten()),
                }
            }
            Phase::End(end) => end,
        };
        match end {
            End::Stop => {
                // Pas récolté : fichier pid gardé pour le ménage suivant.
                if terminate(pid, &mut exited, timing.stop_grace, timing.kill_wait).await {
                    let _ = fs::remove_file(config.pid_file());
                }
                return;
            }
            End::NotReady => {
                if terminate(pid, &mut exited, timing.stop_grace, timing.kill_wait).await {
                    let _ = fs::remove_file(config.pid_file());
                }
                fail(t!(
                    "builtinOllama.failed.notReady",
                    seconds = timing.ready_timeout.as_secs()
                ));
                return;
            }
            End::Exited(status) => {
                let _ = fs::remove_file(config.pid_file());
                let now = Instant::now();
                crashes.push_back(now);
                while crashes
                    .front()
                    .is_some_and(|at| now.duration_since(*at) > timing.crash_window)
                {
                    crashes.pop_front();
                }
                if crashes.len() >= timing.max_crashes {
                    fail(t!(
                        "builtinOllama.failed.crashed",
                        status = describe_exit(status)
                    ));
                    return;
                }
                let delay = timing
                    .backoff
                    .get(crashes.len() - 1)
                    .or(timing.backoff.last())
                    .copied()
                    .unwrap_or_default();
                inner.snapshot.send_modify(|snapshot| {
                    snapshot.state = BuiltinState::Starting;
                    snapshot.pid = None;
                    snapshot.version = None;
                });
                tokio::select! {
                    biased;
                    () = stopped(&mut stop) => return,
                    () = tokio::time::sleep(delay) => {}
                }
                match pick_port(Some(port)) {
                    Ok(next) => port = next,
                    Err(error) => {
                        fail(t!("builtinOllama.failed.spawn", error = error.to_string()));
                        return;
                    }
                }
            }
        }
    }
}

/// Se termine quand l'arrêt est demandé (ou que la demande ne peut plus
/// venir).
async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

/// Version d'Ollama dès que `/api/version` répond ; `None` après `limit`.
async fn answer(url: &str, limit: Duration, poll: Duration) -> Option<String> {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        let probe = tokio::time::timeout_at(deadline, client::probe(url)).await;
        match probe {
            Ok(Probe::Ollama { version }) => return Some(version),
            Ok(_) => {}
            Err(_) => return None,
        }
        if tokio::time::Instant::now() + poll >= deadline {
            return None;
        }
        tokio::time::sleep(poll).await;
    }
}

pub(crate) fn describe_exit(status: Option<ExitStatus>) -> String {
    match status {
        Some(status) => match (status.code(), os::exit_signal(&status)) {
            (Some(code), _) => t!("builtinOllama.exitCode", code = code),
            (None, Some(signal)) => t!("builtinOllama.exitSignal", signal = signal),
            _ => status.to_string(),
        },
        None => t!("builtinOllama.exitUnknown"),
    }
}

impl Inner {
    /// Ménage d'un reste d'une session précédente, une fois ; bloquant.
    fn clean_leftovers(&self) {
        let mut cleaned = self.cleaned.lock();
        if !*cleaned {
            cleanup_leftovers(&self.config);
            *cleaned = true;
        }
    }

    /// Crée le processus depuis le fil dédié (créé au premier appel).
    async fn spawn(&self, command: Command) -> io::Result<Child> {
        spawn_with(&self.spawner, "chatonlm-ollama", command).await
    }
}

/// Crée le processus depuis le fil dédié de `slot`, nommé `name` (créé au
/// premier appel, puis gardé autant que `slot`).
pub(crate) async fn spawn_with(
    slot: &Mutex<Option<Spawner>>,
    name: &str,
    command: Command,
) -> io::Result<Child> {
    let (reply, answer) = oneshot::channel();
    {
        let mut spawner = slot.lock();
        if spawner.is_none() {
            *spawner = Some(Spawner::start(name)?);
        }
        let Some(spawner) = spawner.as_ref() else {
            return Err(io::Error::other("no spawner thread"));
        };
        spawner
            .requests
            .send((command, reply))
            .map_err(|_| io::Error::other("spawner thread stopped"))?;
    }
    answer
        .await
        .map_err(|_| io::Error::other("spawner thread stopped"))?
}

type SpawnRequest = (Command, oneshot::Sender<io::Result<Child>>);

/// Fil qui crée les processus de l'intégré (ou du moteur d'images) et vit
/// autant que l'app (tant que son propriétaire existe) : `PR_SET_PDEATHSIG`
/// envoie SIGTERM à l'enfant quand le FIL qui l'a créé se termine, pas
/// seulement le processus.
pub(crate) struct Spawner {
    requests: std::sync::mpsc::Sender<SpawnRequest>,
}

impl Spawner {
    fn start(name: &str) -> io::Result<Spawner> {
        let (requests, received) = std::sync::mpsc::channel::<SpawnRequest>();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                for (mut command, reply) in received {
                    // Demande abandonnée pendant la création (lancement
                    // annulé) : l'enfant n'est suivi par personne, il est
                    // arrêté aussitôt (sous Windows, son Job reste noté
                    // jusqu'à la fin de ChatonLM).
                    if let Err(Ok(mut child)) = reply.send(os::spawn(&mut command)) {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                }
            })?;
        Ok(Spawner { requests })
    }
}

/// Dossiers, journal et commande d'un lancement.
fn prepare(config: &Config, port: u16, models_dir: &Path) -> io::Result<Command> {
    let bin = config
        .bin
        .clone()
        .ok_or_else(|| io::Error::other("no built-in Ollama"))?;
    fs::create_dir_all(config.home())?;
    // Seul le dossier par défaut est créé : un dossier choisi absent (disque
    // démonté, dossier supprimé) n'est jamais recréé à vide sur le disque
    // système (l'appelant prend alors le dossier par défaut, comme cmd()).
    if models_dir == config.default_models_dir() {
        fs::create_dir_all(models_dir)?;
    } else if !models_dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            t!(
                "builtinOllama.modelsDirMissing",
                path = models_dir.display().to_string()
            ),
        ));
    }
    if let Some(dir) = config.log.parent() {
        fs::create_dir_all(dir)?;
    }
    let log = File::options()
        .create(true)
        .append(true)
        .open(&config.log)?;
    let (appdir, nvidia_workaround) = os::desktop_changes();
    let environment = child_environment(
        std::env::vars_os(),
        appdir.as_deref(),
        nvidia_workaround,
        port,
        models_dir,
        &config.home(),
        &config.extra_env,
    );
    let mut command = Command::new(bin);
    command
        .args(&config.args)
        .env_clear()
        .envs(environment)
        .current_dir(config.home())
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // Rien ne lui survit : groupe et signal de fin du parent (Unix), ou
    // drapeaux de création du Job Object (Windows).
    os::isolate(&mut command);
    Ok(command)
}

/// Environnement du serveur : celui de ChatonLM, nettoyé comme celui des
/// programmes du bureau (variables de l'AppImage, contournement NVIDIA),
/// sans `LD_LIBRARY_PATH` ni `LD_PRELOAD` (l'intégré trouve ses
/// bibliothèques à côté de lui), avec son adresse, son dossier des modèles et
/// son `HOME` ; `extra` en dernier (essais).
pub(crate) fn child_environment(
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
    appdir: Option<&OsStr>,
    nvidia_workaround: bool,
    port: u16,
    models_dir: &Path,
    home: &Path,
    extra: &[(OsString, OsString)],
) -> BTreeMap<OsString, OsString> {
    let mut environment: BTreeMap<OsString, OsString> = inherited.into_iter().collect();
    os::scrub(&mut environment, appdir, nvidia_workaround);
    environment.insert("OLLAMA_HOST".into(), format!("127.0.0.1:{port}").into());
    environment.insert("OLLAMA_MODELS".into(), models_dir.as_os_str().to_owned());
    for name in os::HOME_VARS {
        environment.insert(name.into(), home.as_os_str().to_owned());
    }
    environment.extend(extra.iter().cloned());
    environment
}

/// Retire les variables de ces noms, quelle que soit leur casse (sous
/// Windows, `Ollama_Host` et `OLLAMA_HOST` sont la même variable : en
/// laisser deux donnerait l'une ou l'autre au serveur).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn remove_ignoring_case(environment: &mut BTreeMap<OsString, OsString>, names: &[&str]) {
    environment.retain(|key, _| {
        let key = key.to_string_lossy();
        !names.iter().any(|name| key.eq_ignore_ascii_case(name))
    });
}

/// Journal du lancement : le précédent devient `.1`.
pub(crate) fn rotate_log(log: &Path) {
    if let Some(dir) = log.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if log.exists() {
        let mut previous = log.file_name().unwrap_or_default().to_os_string();
        previous.push(".1");
        let _ = fs::rename(log, log.with_file_name(previous));
    }
}

/// Arrête l'intégré pour de bon (fermeture de l'app, installation d'une
/// mise à jour, étape 18c) : arrêt borné (voir [`Builtin::shutdown`]), puis
/// plus rien ne le relance (requêtes, vérification périodique de l'écran,
/// `ollama_runtime`) jusqu'à [`resume`]. Les travaux qui lui parlent
/// (chat, cours, correction) sont à arrêter avant. Sans effet sur un
/// serveur qui ne tourne pas, sinon de bloquer ses démarrages.
pub async fn shutdown(_app: &tauri::AppHandle) {
    if let Some(targeting) = super::target::current() {
        targeting.builtin().shutdown().await;
    }
}

/// Annule [`shutdown`] (installation d'une mise à jour abandonnée ou en
/// échec, l'app continue) : l'intégré peut de nouveau démarrer, à la
/// prochaine requête ou vérification s'il est la cible.
#[allow(dead_code, reason = "pour la mise à jour (étape 18c)")]
pub fn resume(_app: &tauri::AppHandle) {
    if let Some(targeting) = super::target::current() {
        targeting.builtin().reopen();
    }
}

#[cfg(test)]
pub(crate) mod tests;
