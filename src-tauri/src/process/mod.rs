// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Inspired by Ollama (https://github.com/ollama/ollama), file
// app/server/server.go (openRotatingLog), commit 42e911bc3d05: one previous
// log kept. Moved out of ollama/builtin.rs for ChatonLM, unchanged: the
// process tools shared by every engine the app runs (built-in Ollama, image
// engine, llama-server).

//! Outils des sous-processus de l'app (Ollama intégré, moteur d'images,
//! llama-server), communs aux deux systèmes : rien ne survit à ChatonLM,
//! arrêt borné.
//!
//! - [`Spawner`] / [`spawn_with`] : création depuis un fil dédié qui vit
//!   autant que son propriétaire (`PR_SET_PDEATHSIG` suit le FIL qui a créé
//!   l'enfant, pas seulement le processus).
//! - [`pick_port`] : port libre de 127.0.0.1, jamais 11434 ni 11435.
//! - [`rotate_log`] : le journal précédent devient `.1`.
//! - [`describe_exit`] : fin d'un processus, en clair (langue du moment).
//! - Ce qui dépend du système, monté sous le nom `os` : `unix.rs` (groupe de
//!   processus, `PR_SET_PDEATHSIG`, fin attendue sans récolte, SIGTERM puis
//!   SIGKILL au groupe, environnement nettoyé de l'AppImage) et `windows.rs`
//!   (Job Object fermé avec ChatonLM, `CREATE_NO_WINDOW`,
//!   `TerminateJobObject`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::net::{Ipv4Addr, TcpListener};
use std::path::Path;
use std::process::{Child, Command, ExitStatus};

use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::i18n::t;

#[cfg(unix)]
#[path = "unix.rs"]
mod os;
#[cfg(windows)]
#[path = "windows.rs"]
mod os;

#[cfg(all(test, windows))]
pub(crate) use os::alive;
pub(crate) use os::{
    desktop_changes, exit_signal, is_executable, isolate, no_core_dump, scrub, terminate,
    watch_exit,
};

/// Ports jamais pris : celui d'Ollama et celui de l'instance de test.
pub(crate) const RESERVED_PORTS: [u16; 2] = [11434, 11435];

/// Lance une tâche sur le moteur tokio en cours, sinon sur celui de Tauri.
pub(crate) fn spawn_task(future: impl std::future::Future<Output = ()> + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(future);
        }
        Err(_) => {
            tauri::async_runtime::spawn(future);
        }
    }
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

/// Fin d'un processus en clair (code ou signal).
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

/// Fil qui crée les processus d'un moteur (Ollama intégré, moteur d'images,
/// llama-server) et vit autant que l'app (tant que son propriétaire existe) :
/// `PR_SET_PDEATHSIG` envoie SIGTERM à l'enfant quand le FIL qui l'a créé se
/// termine, pas seulement le processus.
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

#[cfg(test)]
mod tests;
