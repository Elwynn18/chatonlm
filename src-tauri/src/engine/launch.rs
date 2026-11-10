//! Lancement et surveillance d'UN `llama-server` (patron `image_gen/server.rs`
//! et `process/`) : port libre de 127.0.0.1, clé d'API aléatoire par la
//! variable `LLAMA_API_KEY`, journal `<données>/logs/llama-<modèle>.log` (le
//! précédent en `.1`), rempli par l'app depuis les sorties du processus et
//! reparti à zéro tous les 16 Mio (`journal::Capped`, l'ancien en `.1`), dossier de travail = dossier des journaux (jamais le
//! dossier courant de l'app : ggml y chargerait tout `libggml-*.so` /
//! `ggml-*.dll`), environnement nettoyé (`process::scrub`, puis les
//! variables de llama.cpp et de ggml), isolation (groupe et PDEATHSIG sous
//! Linux, Job Object sous Windows).
//!
//! Chaque processus a sa tâche de surveillance : elle attend sa fin, ou la
//! demande d'arrêt ([`Process::stop`], ou le dernier [`Process`] lâché),
//! qu'elle fait par UN SIGTERM puis SIGKILL après un délai (deux SIGTERM font
//! planter b12110 : « free(): chunks in smallbin corrupted ») ; sous Windows,
//! le Job entier d'un coup.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, watch};

use super::journal::{self, ExitFacts};
use super::options::{self, Spec};
use super::{Config, EngineError, Stopping};
use crate::inference::{self, Endpoint, Health, InferenceError};
use crate::process::{
    Spawner, describe_exit, desktop_changes, exit_signal, isolate, no_core_dump, pick_port,
    rotate_log, scrub, spawn_with, terminate, watch_exit,
};

/// Fin d'un processus.
#[derive(Clone, Debug)]
pub(super) struct Exited {
    pub facts: ExitFacts,
    /// « code de sortie 1 », « signal 11 »… (langue du moment).
    pub text: String,
    /// Arrêté par l'app (et non mort de lui-même).
    pub stopped: bool,
}

/// Un `llama-server` lancé. Le dernier lâché arrête le processus (en tâche de
/// fond) : un lancement abandonné ne laisse rien derrière lui.
pub(super) struct Process {
    #[cfg_attr(not(test), allow(dead_code, reason = "lu par les essais"))]
    pub pid: u32,
    pub endpoint: Endpoint,
    pub log: PathBuf,
    exit: watch::Receiver<Option<Exited>>,
    stop: parking_lot::Mutex<Option<oneshot::Sender<()>>>,
    /// Attente de la fin après la demande d'arrêt, au plus.
    stop_limit: Duration,
}

impl Process {
    /// Fin du processus, s'il est fini.
    pub fn exited(&self) -> Option<Exited> {
        self.exit.borrow().clone()
    }

    /// Attend la fin du processus.
    pub async fn wait_exit(&self) -> Exited {
        let mut exit = self.exit.clone();
        match exit.wait_for(Option::is_some).await {
            Ok(exited) => exited.clone().unwrap_or_else(Exited::lost),
            // Surveillance finie sans rien dire (impossible) : fini.
            Err(_) => Exited::lost(),
        }
    }

    /// Demande l'arrêt (une fois) et attend la fin, bornée.
    pub async fn stop(&self) {
        if let Some(stop) = self.stop.lock().take() {
            let _ = stop.send(());
        }
        let _ = tokio::time::timeout(self.stop_limit, self.wait_exit()).await;
    }
}

impl Exited {
    /// Fin sans état connu.
    pub fn lost() -> Exited {
        Exited {
            facts: ExitFacts::default(),
            text: describe_exit(None),
            stopped: false,
        }
    }
}

/// Ce qu'il faut au lancement, venu du moteur.
pub(super) struct Launch<'a> {
    pub config: &'a Config,
    pub spawner: &'a parking_lot::Mutex<Option<Spawner>>,
    pub stopping: &'a Stopping,
    /// Plafond du cache des invites (`--cache-ram`, Mio).
    pub cache_ram: u32,
}

/// Lance `llama-server` sur `spec` (journal du modèle `alias`) sans attendre
/// qu'il soit prêt ; `_slot` (numéro de l'instance) ne nomme plus le
/// journal. `on_exit` est appelé une fois, à la fin du processus
/// (morte ou arrêtée).
pub(super) async fn spawn(
    launch: &Launch<'_>,
    spec: &Spec,
    alias: &str,
    _slot: usize,
    on_exit: impl FnOnce(&Exited) + Send + 'static,
) -> Result<Arc<Process>, EngineError> {
    let config = launch.config;
    let bin = config.bin.clone().ok_or(EngineError::Unavailable)?;
    for path in std::iter::once(&spec.model).chain(spec.mmproj.as_ref()) {
        if !path.is_file() {
            return Err(EngineError::MissingFile(path.clone()));
        }
    }
    let failed = |error: std::io::Error| EngineError::StartFailed(error.to_string());
    let port = pick_port(None).map_err(failed)?;
    let key = options::api_key();
    let endpoint = Endpoint::new(format!("http://127.0.0.1:{port}"), key.clone())
        .map_err(|error| EngineError::StartFailed(error.to_string()))?;
    let log = config.logs.join(super::journal::file_name(alias));
    let args = options::server_args(spec, alias, port, launch.cache_ram, cfg!(windows));
    let (command, file) = prepare(config, &bin, spec, args, &key, &log).map_err(failed)?;
    let mut child = spawn_with(launch.spawner, "chatonlm-llama", command)
        .await
        .map_err(failed)?;
    let pid = child.id();
    let relays = relay_output(&mut child, file, &log);
    let mut exited = watch_exit(child);
    let (exit_tx, exit_rx) = watch::channel(None);
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let timing = config.timing.clone();
    let stopping = launch.stopping.clone();
    crate::process::spawn_task(async move {
        // Fin d'elle-même, ou arrêt demandé (ou plus personne pour le
        // processus : `Process` lâché).
        let natural = tokio::select! {
            status = &mut exited => Some(status.ok().flatten()),
            _ = stop_rx => None,
        };
        let end = match natural {
            Some(status) => Exited {
                facts: ExitFacts {
                    code: status.and_then(|status| status.code()),
                    signal: status.as_ref().and_then(exit_signal),
                },
                text: describe_exit(status),
                stopped: false,
            },
            None => {
                let _stopping = stopping.begin();
                terminate(pid, &mut exited, timing.stop_grace, timing.kill_wait).await;
                Exited {
                    facts: ExitFacts::default(),
                    text: describe_exit(None),
                    stopped: true,
                }
            }
        };
        // Dernières lignes recopiées dans le journal avant d'annoncer la fin
        // (l'erreur d'un échec y est lue) ; borné : un petit-enfant qui
        // tiendrait les tubes ne retient pas la fin.
        let mut relays = relays;
        let _ = tokio::time::timeout(RELAY_WAIT, relays.wait_for(|live| *live == 0)).await;
        // Fin publiée AVANT de prévenir le moteur : quand `on_exit` voit un
        // modèle encore en chargement (et l'ignore), le lancement verra la
        // fin dans `Process::exited`.
        exit_tx.send_replace(Some(end.clone()));
        on_exit(&end);
    });
    Ok(Arc::new(Process {
        pid,
        endpoint,
        log,
        exit: exit_rx,
        stop: parking_lot::Mutex::new(Some(stop_tx)),
        stop_limit: config.timing.stop_grace + config.timing.kill_wait + Duration::from_secs(1),
    }))
}

/// Attente des dernières lignes du journal après la fin du processus.
const RELAY_WAIT: Duration = Duration::from_secs(2);

/// Recopie la sortie et l'erreur de `child` dans `file` (le journal `log`),
/// qui repart à zéro tous les [`journal::MAX_LOG`] (deux fils, jusqu'à la
/// fin des tubes) ; rend le nombre de recopies encore en vie.
fn relay_output(child: &mut std::process::Child, file: File, log: &Path) -> watch::Receiver<usize> {
    let sink = Arc::new(parking_lot::Mutex::new(journal::Capped::rotating(
        file,
        journal::MAX_LOG,
        journal::reopen_file(log),
    )));
    let sources: Vec<Box<dyn std::io::Read + Send>> = [
        child
            .stdout
            .take()
            .map(|out| Box::new(out) as Box<dyn std::io::Read + Send>),
        child
            .stderr
            .take()
            .map(|err| Box::new(err) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    .collect();
    let (live, relays) = watch::channel(sources.len());
    let live = Arc::new(live);
    for source in sources {
        let (sink, done) = (Arc::clone(&sink), Arc::clone(&live));
        let started = std::thread::Builder::new()
            .name("chatonlm-llama-log".into())
            .spawn(move || {
                journal::relay(source, &sink);
                done.send_modify(|count| *count -= 1);
            });
        if started.is_err() {
            // Fil refusé : le tube se ferme (le processus voit une
            // écriture impossible, pas un blocage).
            live.send_modify(|count| *count -= 1);
        }
    }
    relays
}

/// Journal (ouvert, à remplir par [`relay_output`]) et commande d'un
/// lancement (`server_args` : [`options::server_args`]). L'ancien journal
/// du modèle est renommé en `.1` AVANT d'ouvrir le nouveau (sous Windows, un
/// fichier ouvert ne se renomme pas).
fn prepare(
    config: &Config,
    bin: &Path,
    spec: &Spec,
    server_args: Vec<OsString>,
    key: &str,
    log: &Path,
) -> std::io::Result<(Command, File)> {
    std::fs::create_dir_all(&config.logs)?;
    std::fs::create_dir_all(&config.cache)?;
    rotate_log(log);
    let log = File::options()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log)?;
    let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    let (appdir, nvidia_workaround) = desktop_changes();
    scrub(&mut environment, appdir.as_deref(), nvidia_workaround);
    let mut environment = options::server_env(
        environment,
        key,
        &config.cache,
        spec.cpu,
        std::env::var_os("USERPROFILE").as_deref(),
        cfg!(windows),
    );
    // Chargeur Vulkan livré, seulement si le système n'en a pas (le même
    // mécanisme que le moteur d'images).
    if let Some(fallback) = super::binary::fallback_dir(bin) {
        crate::image_gen::offer_vulkan_loader(&mut environment, &fallback);
    }
    let mut command = Command::new(bin);
    command.args(&config.prefix_args);
    match &config.args_in_env {
        Some(variable) => {
            let joined = server_args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("\u{1f}");
            environment.insert(variable.into(), joined.into());
        }
        None => {
            command.args(&server_args);
        }
    }
    environment.extend(config.extra_env.iter().cloned());
    command
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(&config.logs);
    isolate(&mut command);
    no_core_dump(&mut command);
    Ok((command, log))
}

/// Pourquoi un lancement n'est pas prêt.
#[derive(Debug)]
pub(super) enum NotReady {
    /// Mort pendant le chargement.
    Died(Exited),
    /// Pas prêt dans le délai.
    Timeout,
    /// Plus attendu (déchargé, fin de l'app).
    Cancelled,
    /// Un autre programme répond sur le port (clé refusée, autre modèle ou
    /// autre contexte).
    NotOurs,
}

/// Attend que le serveur soit prêt : `/health` 200 (503 pendant le
/// chargement), puis `/props` accepté avec NOTRE clé et annonçant notre
/// alias et notre contexte (sinon un autre programme, avec ou sans clé, a
/// pris le port entre-temps). `cancelled` est relu à chaque sondage.
pub(super) async fn wait_ready(
    process: &Process,
    config: &Config,
    alias: &str,
    ctx: u32,
    cancelled: impl Fn() -> bool,
) -> Result<(), NotReady> {
    let timing = &config.timing;
    let deadline = tokio::time::Instant::now() + timing.ready_timeout;
    loop {
        if let Some(exited) = process.exited() {
            return Err(NotReady::Died(exited));
        }
        if cancelled() {
            return Err(NotReady::Cancelled);
        }
        if let Health::Ready = inference::health(&process.endpoint).await {
            match inference::props(&process.endpoint).await {
                Ok(props) if props.model_alias == alias && props.n_ctx == ctx => {
                    caught_up(process).await;
                    return Ok(());
                }
                Ok(_) | Err(InferenceError::Unauthorized) => return Err(NotReady::NotOurs),
                // Encore en route (ou mort entre-temps : vu au tour suivant).
                Err(_) => {}
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(NotReady::Timeout);
        }
        // La mort du processus interrompt l'attente.
        let mut exit = process.exit.clone();
        let _ = tokio::time::timeout(timing.ready_poll, exit.wait_for(Option::is_some)).await;
    }
}

/// Attend (au plus [`RELAY_WAIT`]) que la recopie du journal ait passé la
/// fin du chargement : le serveur l'écrit avant d'écouter, mais la recopie
/// (un fil par sortie) peut être en retard sous charge, et la mémoire du
/// modèle est lue dans le journal dès qu'il est prêt.
async fn caught_up(process: &Process) {
    let deadline = tokio::time::Instant::now() + RELAY_WAIT;
    while !journal::load_finished(&journal::read(&process.log))
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La commande part du dossier des journaux (jamais du dossier courant de
    /// l'app, où ggml chargerait des moteurs étrangers), sans la clé en ligne
    /// de commande, avec la clé, le cache et le journal attendus.
    #[test]
    fn the_command_runs_in_our_folder_with_its_key_in_the_environment() {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("llama-server");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin = bin_dir.join("llama-server");
        std::fs::write(&bin, b"").unwrap();
        let config = Config::new(Some(bin.clone()), &dir.path().join("data"));
        let spec = Spec {
            model: dir.path().join("m.gguf"),
            mmproj: None,
            ctx: 4096,
            cpu: true,
        };
        let log = config.logs.join(journal::file_name("a/b:Q4_K_M"));
        std::fs::create_dir_all(&config.logs).unwrap();
        std::fs::write(&log, "old").unwrap();
        let args = options::server_args(&spec, "a/b:Q4_K_M", 4321, 512, false);
        let (command, _file) = prepare(&config, &bin, &spec, args, "s3cret", &log).unwrap();
        assert_eq!(command.get_current_dir(), Some(config.logs.as_path()));
        assert_ne!(
            command.get_current_dir(),
            Some(std::env::current_dir().unwrap().as_path())
        );
        assert!(config.logs.starts_with(dir.path().join("data")));
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(!args.iter().any(|arg| arg.contains("s3cret")));
        assert!(args.windows(2).any(|w| w == ["-c", "4096"]));
        assert!(args.windows(2).any(|w| w == ["-ngl", "0"]));
        let env: BTreeMap<_, _> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(env["LLAMA_API_KEY"].as_deref(), Some("s3cret"));
        assert_eq!(
            env["LLAMA_CACHE"].as_deref(),
            Some(config.cache.to_string_lossy().as_ref())
        );
        assert_eq!(env["GGML_VK_VISIBLE_DEVICES"].as_deref(), Some("none"));
        // Journal précédent gardé en `.1`, le nouveau vide.
        assert_eq!(
            std::fs::read_to_string(config.logs.join("llama-a_b_Q4_K_M.log.1")).unwrap(),
            "old"
        );
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "");
    }
}
