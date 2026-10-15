//! Lancement de `sd-server` : port libre de 127.0.0.1, fichiers du modèle en
//! arguments, journal, isolation du processus (celle de l'Ollama intégré),
//! attente de la première réponse. Un [`Running`] lâché sans
//! [`Running::stop`] arrête son processus en tâche de fond : un lancement ou
//! une image abandonnés ne laissent rien derrière eux.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use tokio::sync::{oneshot, watch};

use super::{Config, ImageGenError, ModelSpec, Timing, api};
use crate::ollama::builtin::process::{
    Spawner, describe_exit, desktop_changes, isolate, pick_port, rotate_log, scrub, spawn_with,
    terminate, watch_exit,
};

/// Fin du journal relue pour expliquer un échec au démarrage.
const LOG_TAIL: u64 = 64 * 1024;

/// Lignes d'erreur du serveur gardées pour expliquer un échec.
const MAX_ERROR_LINES: usize = 3;

/// Arrêts de serveurs en cours (y compris ceux faits en tâche de fond par
/// un [`Running`] lâché) : un nouveau serveur attend leur fin, pour ne pas
/// partager la mémoire (RAM, VRAM) avec un serveur qui meurt.
#[derive(Clone)]
pub(super) struct Stops(Arc<watch::Sender<usize>>);

impl Default for Stops {
    fn default() -> Self {
        Stops(Arc::new(watch::channel(0).0))
    }
}

impl Stops {
    fn begin(&self) -> StopToken {
        self.0.send_modify(|count| *count += 1);
        StopToken(self.clone())
    }

    /// Attend la fin des arrêts en cours, `limit` au plus.
    pub(super) async fn settled(&self, limit: Duration) {
        let mut count = self.0.subscribe();
        let _ = tokio::time::timeout(limit, count.wait_for(|count| *count == 0)).await;
    }
}

/// Un arrêt en cours, jusqu'à sa fin.
struct StopToken(Stops);

impl Drop for StopToken {
    fn drop(&mut self) {
        self.0
            .0
            .send_modify(|count| *count = count.saturating_sub(1));
    }
}

/// Serveur lancé.
pub(super) struct Running {
    pid: u32,
    url: String,
    spec: ModelSpec,
    client: reqwest::Client,
    /// Fin du processus ; `None` une fois arrêté par [`Running::stop`].
    exited: Option<oneshot::Receiver<Option<ExitStatus>>>,
    /// Fin constatée (le récepteur ne la donne qu'une fois).
    status: Option<Option<ExitStatus>>,
    timing: Timing,
    /// Numéro publié hors du verrou des images (0 : aucun serveur).
    published: Arc<AtomicU32>,
    stops: Stops,
}

impl Running {
    pub(super) fn url(&self) -> &str {
        &self.url
    }

    pub(super) fn spec(&self) -> &ModelSpec {
        &self.spec
    }

    pub(super) fn client(&self) -> reqwest::Client {
        self.client.clone()
    }

    /// Le processus tourne encore.
    pub(super) fn alive(&mut self) -> bool {
        if self.status.is_some() {
            return false;
        }
        let Some(exited) = self.exited.as_mut() else {
            return false;
        };
        match exited.try_recv() {
            Err(oneshot::error::TryRecvError::Empty) => true,
            Ok(status) => {
                self.status = Some(status);
                false
            }
            Err(oneshot::error::TryRecvError::Closed) => {
                self.status = Some(None);
                false
            }
        }
    }

    /// Fin du processus, en mots (« code de sortie 134 »...).
    pub(super) fn exit_description(&mut self) -> String {
        self.alive();
        describe_exit(self.status.flatten())
    }

    /// Plus publié : le numéro ne désigne plus ce serveur.
    fn unpublish(&self) {
        let _ = self
            .published
            .compare_exchange(self.pid, 0, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Arrête le processus (SIGTERM, délai, SIGKILL ; Job Object sous
    /// Windows) et attend sa fin, bornée. Rien n'est envoyé à un processus
    /// déjà fini (son numéro a pu être repris).
    pub(super) async fn stop(mut self, timing: &Timing) {
        self.unpublish();
        let alive = self.alive();
        if let Some(mut exited) = self.exited.take()
            && alive
        {
            let _token = self.stops.begin();
            terminate(self.pid, &mut exited, timing.stop_grace, timing.kill_wait).await;
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.unpublish();
        let alive = self.alive();
        if let Some(mut exited) = self.exited.take()
            && alive
        {
            let pid = self.pid;
            let timing = self.timing.clone();
            let token = self.stops.begin();
            crate::ollama::builtin::spawn(async move {
                terminate(pid, &mut exited, timing.stop_grace, timing.kill_wait).await;
                drop(token);
            });
        }
    }
}

/// Lance `sd-server` sur `spec` et attend qu'il réponde.
pub(super) async fn start(
    config: &Config,
    spawner: &parking_lot::Mutex<Option<Spawner>>,
    spec: &ModelSpec,
    published: &Arc<AtomicU32>,
    stops: &Stops,
) -> Result<Running, ImageGenError> {
    let bin = config.bin.clone().ok_or(ImageGenError::Unavailable)?;
    for (_, path) in &spec.files {
        if !path.is_file() {
            return Err(ImageGenError::MissingFile(path.clone()));
        }
    }
    let port = pick_port(None).map_err(|error| ImageGenError::StartFailed(error.to_string()))?;
    let url = format!("http://127.0.0.1:{port}");
    // Tout ce qui peut échouer avant le lancement : un enfant lancé est
    // toujours suivi (et arrêté) par un `Running`.
    let client = api::client(&url).map_err(ImageGenError::StartFailed)?;
    let command = prepare(config, &bin, port, spec)
        .map_err(|error| ImageGenError::StartFailed(error.to_string()))?;
    let child = spawn_with(spawner, "chatonlm-images", command)
        .await
        .map_err(|error| ImageGenError::StartFailed(error.to_string()))?;
    let pid = child.id();
    published.store(pid, Ordering::SeqCst);
    let mut running = Running {
        pid,
        client,
        url,
        spec: spec.clone(),
        exited: Some(watch_exit(child)),
        status: None,
        timing: config.timing.clone(),
        published: published.clone(),
        stops: stops.clone(),
    };
    let timing = &config.timing;
    let deadline = tokio::time::Instant::now() + timing.ready_timeout;
    loop {
        if !running.alive() {
            let reason = last_error(&config.log).unwrap_or_else(|| running.exit_description());
            return Err(ImageGenError::StartFailed(reason));
        }
        if api::ready(&running.client, &running.url).await {
            return Ok(running);
        }
        if tokio::time::Instant::now() + timing.ready_poll >= deadline {
            running.stop(timing).await;
            return Err(ImageGenError::NotReady(timing.ready_timeout.as_secs()));
        }
        tokio::time::sleep(timing.ready_poll).await;
    }
}

/// Journal et commande d'un lancement.
fn prepare(config: &Config, bin: &Path, port: u16, spec: &ModelSpec) -> std::io::Result<Command> {
    rotate_log(&config.log);
    let log = File::options()
        .create(true)
        .append(true)
        .open(&config.log)?;
    let mut server_args: Vec<OsString> = vec![
        "--listen-ip".into(),
        "127.0.0.1".into(),
        "--listen-port".into(),
        port.to_string().into(),
    ];
    server_args.extend(spec.args());
    let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    // Linux : environnement nettoyé comme celui des programmes du bureau
    // (AppImage, LD_*, contournement NVIDIA). Rien à retirer sous Windows
    // (le `scrub` de Windows ôte les variables propres à Ollama).
    if cfg!(unix) {
        let (appdir, nvidia_workaround) = desktop_changes();
        scrub(&mut environment, appdir.as_deref(), nvidia_workaround);
    }
    // Chargeur Vulkan livré, seulement si le système n'en a pas.
    if let Some(dir) = bin.parent() {
        super::os::vulkan_loader(
            &mut environment,
            &dir.join("fallback"),
            super::os::system_has_loader(),
        );
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
        .stdout(log.try_clone()?)
        .stderr(log);
    if let Some(dir) = config.log.parent() {
        command.current_dir(dir);
    }
    isolate(&mut command);
    Ok(command)
}

/// Dernières erreurs écrites par le serveur pendant ce lancement (`[E] ...`,
/// sans la source C++), de la plus ancienne à la plus récente : la cause
/// vient souvent avant la conséquence (« get sd version from file failed »,
/// puis « new_sd_ctx_t failed »).
fn last_error(log: &Path) -> Option<String> {
    let mut file = File::open(log).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(LOG_TAIL)))
        .ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    let text = String::from_utf8_lossy(&tail);
    let errors: Vec<&str> = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("[E]"))
        .map(|line| line.split(" --- ").next().unwrap_or(line).trim())
        .filter(|line| !line.is_empty())
        .collect();
    let kept = &errors[errors.len().saturating_sub(MAX_ERROR_LINES)..];
    (!kept.is_empty()).then(|| kept.join("; "))
}

/// Délai d'une requête au serveur local.
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_engine_error_is_read_without_its_source_location() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("sd-server.log");
        std::fs::write(
            &log,
            "[I] start --- main.cpp:1\n[E] get sd version from file failed: '' --- diffusion_engine.cpp:1011\n[E] new_sd_ctx_t failed --- main.cpp:95\n[I] bye\n",
        )
        .unwrap();
        assert_eq!(
            last_error(&log).as_deref(),
            Some("get sd version from file failed: ''; new_sd_ctx_t failed")
        );
        std::fs::write(&log, "[I] nothing wrong\n").unwrap();
        assert_eq!(last_error(&log), None);
    }
}
