//! Faux `llama-server`, lancé comme le vrai (par l'app avec
//! `CHATONLM_LLAMA_SERVER_BIN`, ou par un essai) :
//!   fake-llama --host 127.0.0.1 --port <n> -m <modèle.gguf> [options de llama-server…]
//! (clé par `LLAMA_API_KEY`, `--api-key` ou `--api-key-file`, comme le vrai)
//! Le comportement dépend du nom du modèle (voir `scenario` dans la
//! bibliothèque) ; mode réseau par `FAKE_LLAMA_MODE` (`ok`, `silent`,
//! `close`, `huge`, `status:<code>`, `loading:<ms>`) ; chargement par défaut
//! de 150 ms (`/health` répond 503 pendant ce temps), ou
//! `FAKE_LLAMA_LOAD_MS`. Journal au format du vrai sur la sortie d'erreur
//! (« listening on http://… », « model loaded ») ; options inconnues
//! acceptées et journalisées. Comme le vrai : `--version`, `--list-devices`,
//! modèle absent (code 1), port pris (code 1). `--port 0` : port libre,
//! écrit dans la ligne « listening on ». Fin : SIGINT ou SIGTERM (Windows :
//! Ctrl+C ou Ctrl+Pause) → arrêt propre, code 0 ; `chatonlm-loadfail` → code 1
//! ; `chatonlm-crash` → `abort` au milieu d'une réponse.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use fake_llama::args;
use fake_llama::{Config, Exit, FakeLlama, Mode};

/// Chargement par défaut du binaire : la sonde de l'app voit d'abord 503.
const DEFAULT_LOAD: Duration = Duration::from_millis(150);

/// Demande d'arrêt du système : SIGTERM sous Unix, Ctrl+Pause sous Windows.
struct Terminate {
    #[cfg(unix)]
    signal: tokio::signal::unix::Signal,
    #[cfg(windows)]
    signal: tokio::signal::windows::CtrlBreak,
}

impl Terminate {
    fn listen() -> Terminate {
        #[cfg(unix)]
        let signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("cannot listen for SIGTERM");
        #[cfg(windows)]
        let signal = tokio::signal::windows::ctrl_break().expect("cannot listen for Ctrl+Break");
        Terminate { signal }
    }

    async fn recv(&mut self) {
        let _ = self.signal.recv().await;
    }
}

fn number(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

#[tokio::main]
async fn main() {
    // Clés aussi par l'environnement, comme le vrai (`LLAMA_API_KEY`,
    // `LLAMA_ARG_API_KEY_FILE`).
    let args = match args::parse_with_env(std::env::args().skip(1), |name| std::env::var(name).ok())
    {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    if args.version {
        println!("version: 0.9.0-dev (build 12110, commit cd959bae9)");
        println!("built with fake-llama for tests");
        return;
    }
    if args.list_devices {
        println!("Available devices:");
        println!("  Fake0: fake-llama device (8192 MiB, 8192 MiB free)");
        return;
    }
    if args.help {
        println!("fake-llama: faux llama-server des essais de ChatonLM (voir main.rs)");
        return;
    }
    for option in &args.unknown {
        eprintln!("fake-llama: option inconnue acceptée : {option}");
    }
    let Some(model) = &args.model else {
        eprintln!("fake-llama: -m est exigé (le mode routeur n'est pas simulé)");
        std::process::exit(1);
    };
    if !model.is_file() {
        let model = model.display();
        eprintln!(
            "E llama_model_load: error loading model: llama_model_loader: failed to load model from {model}"
        );
        eprintln!("E srv    load_model: failed to load model, '{model}'");
        eprintln!("E srv  server_run_i: exiting due to model loading error");
        std::process::exit(1);
    }
    let mode = match std::env::var("FAKE_LLAMA_MODE").map(|mode| Mode::parse(&mode)) {
        Err(_) => Mode::Ok,
        Ok(Some(mode)) => mode,
        Ok(None) => {
            eprintln!(
                "fake-llama: FAKE_LLAMA_MODE inconnu (ok, silent, close, huge, status:<code>, loading:<ms>)"
            );
            std::process::exit(2);
        }
    };
    let Ok(host) = args.host.parse::<IpAddr>() else {
        eprintln!("E srv  init: invalid host {}", args.host);
        std::process::exit(1);
    };
    let mut config = Config::from_args(&args, mode);
    config.load_delay = number("FAKE_LLAMA_LOAD_MS").map_or(DEFAULT_LOAD, Duration::from_millis);
    let server = match FakeLlama::start_on(SocketAddr::new(host, args.port), config).await {
        Ok(server) => server,
        Err(error) => {
            eprintln!(
                "E srv  init: couldn't bind HTTP server socket, hostname: {host}, port: {}: {error}",
                args.port
            );
            std::process::exit(1);
        }
    };
    let mut terminate = Terminate::listen();
    let exit = tokio::select! {
        _ = tokio::signal::ctrl_c() => None,
        _ = terminate.recv() => None,
        exit = server.exit_requested() => Some(exit),
    };
    server.stop().await;
    match exit {
        None => {}
        Some(Exit::Code(code)) => std::process::exit(code),
        Some(Exit::Abort) => std::process::abort(),
    }
}
