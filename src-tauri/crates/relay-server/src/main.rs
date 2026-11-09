//! `chatonlm-relay` : point d'entrée.
//!
//! ```text
//! chatonlm-relay serve --config <config.toml> [--secrets <secrets.toml>]
//! chatonlm-relay check --config <config.toml> [--secrets <secrets.toml>]
//! chatonlm-relay stats --config <config.toml>
//! ```
//!
//! Sans `--secrets`, le fichier `secrets` du dossier des identifiants de
//! systemd (`$CREDENTIALS_DIRECTORY`, `LoadCredential=`) est lu.

use relay_server::config::{Config, Secrets};
use relay_server::log::{Clock, Log};
use std::path::PathBuf;
use std::process::ExitCode;

struct Args {
    command: String,
    config: PathBuf,
    secrets: Option<PathBuf>,
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: chatonlm-relay serve|check --config <config.toml> [--secrets <secrets.toml>]\n       \
         chatonlm-relay stats --config <config.toml>"
    );
    ExitCode::from(2)
}

fn parse_args() -> Option<Args> {
    let mut args = std::env::args().skip(1);
    let command = args.next()?;
    let mut config = None;
    let mut secrets = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config = Some(PathBuf::from(args.next()?)),
            "--secrets" => secrets = Some(PathBuf::from(args.next()?)),
            _ => return None,
        }
    }
    Some(Args {
        command,
        config: config?,
        secrets,
    })
}

fn secrets_path(args: &Args) -> Option<PathBuf> {
    args.secrets.clone().or_else(|| {
        std::env::var_os("CREDENTIALS_DIRECTORY").map(|dir| PathBuf::from(dir).join("secrets"))
    })
}

fn load(args: &Args) -> Result<(Config, Secrets), String> {
    let config = Config::load(&args.config)?;
    let path =
        secrets_path(args).ok_or("no secrets file (--secrets or LoadCredential=secrets:…)")?;
    let secrets = Secrets::load(&path)?;
    Ok((config, secrets))
}

fn main() -> ExitCode {
    // Avant tout : une panique n'écrit que son emplacement, jamais son message.
    relay_server::log::install_panic_hook();
    let Some(args) = parse_args() else {
        return usage();
    };
    let log = Log::default();
    match args.command.as_str() {
        "check" => match load(&args) {
            Ok(_) => {
                println!("configuration OK");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        },
        "stats" => stats(&args),
        "serve" => {
            let (config, secrets) = match load(&args) {
                Ok(loaded) => loaded,
                Err(error) => {
                    log.error(&error);
                    return ExitCode::FAILURE;
                }
            };
            relay_server::install_crypto_provider();
            let cores = std::thread::available_parallelism().map_or(1, usize::from);
            let (workers, blocking) = runtime_threads(cores);
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(workers)
                .max_blocking_threads(blocking)
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    log.error(&format!("runtime: {error}"));
                    return ExitCode::FAILURE;
                }
            };
            runtime.block_on(async move {
                let listen = config.listen;
                if !listen.ip().is_loopback() {
                    log.warn("listening on a non-loopback address: put the relay behind nginx");
                }
                let relay = match relay_server::Relay::new(
                    config,
                    secrets,
                    Clock::default(),
                    log.clone(),
                ) {
                    Ok(relay) => relay,
                    Err(error) => {
                        log.error(&error);
                        return ExitCode::FAILURE;
                    }
                };
                let listener = match tokio::net::TcpListener::bind(listen).await {
                    Ok(listener) => listener,
                    Err(error) => {
                        log.error(&format!("cannot listen on {listen}: {error}"));
                        return ExitCode::FAILURE;
                    }
                };
                log.info(&format!("listening on {listen}"));
                match relay_server::serve(relay, listener, shutdown_signal()).await {
                    Ok(()) => {
                        log.info("stopped");
                        ExitCode::SUCCESS
                    }
                    Err(error) => {
                        log.error(&format!("server: {error}"));
                        ExitCode::FAILURE
                    }
                }
            })
        }
        _ => usage(),
    }
}

/// Fils du runtime (travail, puis tâches bloquantes : SQLite, preuves de travail, résolution des
/// noms). Bornés pour tenir dans le `TasksMax=` de l'unité systemd quelle que soit la machine : par
/// défaut, tokio prend un fil par cœur et jusqu'à 512 fils bloquants. `CPUQuota=100%` ne laisse
/// de toute façon qu'un cœur au relais.
fn runtime_threads(cores: usize) -> (usize, usize) {
    (cores.clamp(1, 4), 16)
}

/// Compteurs agrégés du budget, mois par mois (aucune donnée personnelle).
fn stats(args: &Args) -> ExitCode {
    let config = match Config::load(&args.config) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    // Lancé par root à côté du service : lecture seule, aucun fichier créé (voir `open_read_only`).
    let store = match relay_server::store::Store::open_read_only(&config.database) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("database: {error}");
            return ExitCode::FAILURE;
        }
    };
    let (Ok(months), Ok(keys)) = (store.months(), store.key_count()) else {
        eprintln!("database: unreadable");
        return ExitCode::FAILURE;
    };
    println!("registered keys: {keys}");
    println!("month    spent (EUR)  brave   mojeek  failures");
    for (month, record) in months {
        println!(
            "{month}  {:>11.2}  {:>6}  {:>6}  {:>8}",
            record.spent_micro_eur as f64 / 1_000_000.0,
            record.brave,
            record.mojeek,
            record.failures
        );
    }
    ExitCode::SUCCESS
}

/// SIGTERM (systemd) ou SIGINT.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::runtime_threads;

    /// B1 : le binaire pose le crochet de panique sans contenu dès son entrée
    /// (le crochet lui-même est essayé dans `tests/panic.rs`).
    #[test]
    fn main_installs_the_quiet_panic_hook_first() {
        let source = include_str!("main.rs");
        let main = source
            .split("fn main() -> ExitCode {")
            .nth(1)
            .and_then(|body| body.split("parse_args()").next())
            .expect("fn main");
        let call = ["relay_server::log::", "install_panic_hook();"].concat();
        assert!(main.contains(&call), "{main}");
    }

    /// Les fils du relais tiennent dans le `TasksMax=` de l'unité livrée, même sur une grosse
    /// machine (sinon tokio ne pourrait plus créer de fil, ou le runtime ne démarrerait pas).
    #[test]
    fn runtime_threads_fit_in_the_systemd_unit() {
        let unit = include_str!("../../../../packaging/relay/chatonlm-relay.service");
        let tasks_max: usize = unit
            .lines()
            .find_map(|line| line.strip_prefix("TasksMax="))
            .expect("TasksMax= dans l'unité")
            .trim()
            .parse()
            .unwrap();
        for cores in [1, 2, 4, 16, 64, 256] {
            let (workers, blocking) = runtime_threads(cores);
            assert!(workers >= 1);
            // Fil principal, signaux et marge.
            assert!(workers + blocking + 8 <= tasks_max, "{cores} cœurs");
        }
    }
}
