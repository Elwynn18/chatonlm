//! Le faux serveur lancé comme `ollama serve` : Ollama intégré de ChatonLM
//! (`CHATONLM_OLLAMA_BIN` des builds d'essai, essais du sous-processus).
//!
//! L'adresse vient de `OLLAMA_HOST` (`hôte:port`, port obligatoire : jamais
//! le 11434 d'un vrai Ollama par défaut) ; mode `ollama` (modèles de
//! `fixtures/`), version `FAKE_OLLAMA_VERSION` (0.40.0 par défaut).
//! L'environnement reçu est écrit dans `$HOME/env` (une variable par ligne,
//! triées). Comportements des essais, par variables :
//! - `FAKE_OLLAMA_CRASHES=<fichier>` : tant que le nombre écrit dans le
//!   fichier est positif, le décrémente et sort aussitôt avec le code 1
//!   (plantage au démarrage) ;
//! - `FAKE_OLLAMA_DELAY_MS=<n>` : attente avant de répondre (le port est
//!   pris aussitôt, comme Ollama qui se prépare : aucun autre serveur
//!   lancé pendant ce temps ne peut l'obtenir) ;
//! - `FAKE_OLLAMA_EXIT_AFTER_MS=<n>` : sortie (code 3) après n ms de service ;
//! - `FAKE_OLLAMA_RUNNER=1` : lance un « runner » (le même exécutable avec
//!   les mêmes arguments et `FAKE_OLLAMA_ROLE=runner`, qui attend sans fin),
//!   pid écrit dans `$HOME/runner.pid` ; arrêté à SIGTERM, comme le fait
//!   Ollama de ses runners ;
//! - `FAKE_OLLAMA_IGNORE_TERM=1` : SIGTERM ignoré par le serveur et par son
//!   runner, que le serveur n'arrête jamais (Ollama tué par SIGKILL) : seul
//!   un SIGKILL du groupe en vient à bout.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

use crate::{FakeOllama, Mode};

/// Variable qui donne le rôle du processus (`runner` : attendre sans fin).
pub const ROLE_ENV: &str = "FAKE_OLLAMA_ROLE";

/// Demande d'arrêt du système : SIGTERM sous Unix ; sous Windows, Ctrl+Pause
/// (`CTRL_BREAK_EVENT`, ce que l'app officielle envoie à son serveur). Tant
/// qu'elle est écoutée, son action par défaut (fin du processus) n'a pas
/// lieu.
pub struct Terminate {
    #[cfg(unix)]
    signal: tokio::signal::unix::Signal,
    #[cfg(windows)]
    signal: tokio::signal::windows::CtrlBreak,
}

impl Terminate {
    pub fn listen() -> Terminate {
        #[cfg(unix)]
        let signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("cannot listen for SIGTERM");
        #[cfg(windows)]
        let signal = tokio::signal::windows::ctrl_break().expect("cannot listen for Ctrl+Break");
        Terminate { signal }
    }

    /// Attend la prochaine demande.
    pub async fn recv(&mut self) {
        let _ = self.signal.recv().await;
    }
}

/// Vrai pour un « runner » lancé par [`run`].
pub fn is_runner() -> bool {
    std::env::var(ROLE_ENV).is_ok_and(|role| role == "runner")
}

/// Runner : attend d'être arrêté (action par défaut des signaux, SIGTERM
/// ignoré avec `FAKE_OLLAMA_IGNORE_TERM=1`).
pub async fn runner() {
    let _ignored = flag("FAKE_OLLAMA_IGNORE_TERM").then(Terminate::listen);
    loop {
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}

/// `hôte:port` de `OLLAMA_HOST` ; refusé sans port explicite ou sur 11434.
pub fn listen_address(value: Option<&str>) -> Option<SocketAddr> {
    let value = value?.trim();
    let value = value
        .strip_prefix("http://")
        .unwrap_or(value)
        .trim_end_matches('/');
    let address: SocketAddr = value.parse().ok()?;
    (address.port() != 0 && address.port() != 11434).then_some(address)
}

fn number(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| value == "1")
}

/// Décrémente le compteur de plantages ; vrai s'il en restait un.
fn crash_now(file: &PathBuf) -> bool {
    let left: u64 = std::fs::read_to_string(file)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0);
    if left == 0 {
        return false;
    }
    let _ = std::fs::write(file, (left - 1).to_string());
    true
}

/// Dossier personnel donné par l'app (`HOME` ; sous Windows, `USERPROFILE`
/// s'il manque : c'est celui que lit le vrai serveur).
fn home() -> Option<PathBuf> {
    let home = std::env::var_os("HOME");
    #[cfg(windows)]
    let home = home.or_else(|| std::env::var_os("USERPROFILE"));
    home.map(PathBuf::from)
}

fn write_environment() {
    let Some(home) = home() else {
        return;
    };
    let mut lines: Vec<String> = std::env::vars_os()
        .map(|(key, value)| format!("{}={}", key.to_string_lossy(), value.to_string_lossy()))
        .collect();
    lines.sort();
    let _ = std::fs::write(home.join("env"), lines.join("\n") + "\n");
}

fn start_runner() -> Option<Child> {
    let exe = std::env::current_exe().ok()?;
    let mut command = Command::new(exe);
    command
        .args(std::env::args_os().skip(1))
        .env(ROLE_ENV, "runner");
    // Comme le vrai serveur (`CREATE_NO_WINDOW` de llm_windows.go) : le
    // runner a sa propre console, sans fenêtre, et ne reçoit donc pas les
    // événements de celle du serveur.
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0800_0000);
    let child = command.spawn().ok()?;
    if let Some(home) = home() {
        let _ = std::fs::write(home.join("runner.pid"), child.id().to_string());
    }
    Some(child)
}

/// Sert jusqu'à SIGINT ou SIGTERM ; renvoie le code de sortie.
pub async fn run() -> i32 {
    if is_runner() {
        runner().await;
        return 0;
    }
    let host = std::env::var("OLLAMA_HOST").ok();
    let Some(address) = listen_address(host.as_deref()) else {
        eprintln!("fake-ollama serve: OLLAMA_HOST invalide ou absent : {host:?}");
        return 2;
    };
    if let Some(file) = std::env::var_os("FAKE_OLLAMA_CRASHES").map(PathBuf::from)
        && crash_now(&file)
    {
        eprintln!("fake-ollama serve: plantage simulé");
        return 1;
    }
    write_environment();
    // Avant de répondre : un SIGTERM reçu dès que l'adresse répond suit le
    // comportement demandé.
    let mut terminate = Terminate::listen();
    let ignore_term = flag("FAKE_OLLAMA_IGNORE_TERM");
    if let Some(ms) = number("FAKE_OLLAMA_DELAY_MS") {
        // Port pris, connexions fermées aussitôt (sans réponse).
        let held = tokio::net::TcpListener::bind(address)
            .await
            .ok()
            .map(|listener| tokio::spawn(async move { while listener.accept().await.is_ok() {} }));
        let delay = tokio::time::sleep(Duration::from_millis(ms));
        tokio::pin!(delay);
        loop {
            tokio::select! {
                () = &mut delay => break,
                _ = terminate.recv() => {
                    if !ignore_term {
                        return 0;
                    }
                }
            }
        }
        if let Some(held) = held {
            held.abort();
            let _ = held.await;
        }
    }
    let version = std::env::var("FAKE_OLLAMA_VERSION").unwrap_or_else(|_| "0.40.0".into());
    let mode = Mode::Ollama {
        version,
        models: true,
    };
    let server = match FakeOllama::start_on(address, mode).await {
        Ok(server) => server,
        Err(error) => {
            eprintln!("Error: listen tcp {address}: {error}");
            return 1;
        }
    };
    println!("Listening on {address} (fake-ollama serve)");
    let mut runner = flag("FAKE_OLLAMA_RUNNER").then(start_runner).flatten();
    let exit_after = number("FAKE_OLLAMA_EXIT_AFTER_MS").map(Duration::from_millis);

    let code = loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break 0,
            _ = terminate.recv() => {
                if ignore_term {
                    println!("SIGTERM ignored");
                    continue;
                }
                break 0;
            }
            () = async {
                match exit_after {
                    Some(delay) => tokio::time::sleep(delay).await,
                    None => std::future::pending().await,
                }
            } => break 3,
        }
    };
    if let Some(child) = runner.as_mut().filter(|_| !ignore_term) {
        let _ = child.kill();
        let _ = child.wait();
    }
    server.stop().await;
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_port_other_than_ollama_s_own() {
        assert_eq!(
            listen_address(Some("127.0.0.1:40123")),
            Some(SocketAddr::from(([127, 0, 0, 1], 40123)))
        );
        assert_eq!(
            listen_address(Some("http://127.0.0.1:40123/")),
            Some(SocketAddr::from(([127, 0, 0, 1], 40123)))
        );
        for value in ["127.0.0.1:11434", "127.0.0.1", "127.0.0.1:0", "localhost:1"] {
            assert_eq!(listen_address(Some(value)), None, "{value}");
        }
        assert_eq!(listen_address(None), None);
    }
}
