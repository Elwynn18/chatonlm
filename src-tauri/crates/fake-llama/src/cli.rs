//! Le faux `llama-server` en ligne de commande (binaire `fake-llama`, et
//! relance d'un binaire d'essai de l'app : [`args_from_env`]) :
//!   fake-llama --host 127.0.0.1 --port <n> -m <modèle.gguf> [options de llama-server…]
//! (clé par `LLAMA_API_KEY`, `--api-key` ou `--api-key-file`, comme le vrai)
//! Le comportement dépend du nom du modèle (voir `scenario`) ; mode réseau
//! par `FAKE_LLAMA_MODE` (`ok`, `silent`, `close`, `huge`, `status:<code>`,
//! `loading:<ms>`) ; chargement par défaut de 150 ms (`/health` répond 503
//! pendant ce temps), ou `FAKE_LLAMA_LOAD_MS`. Journal au format du vrai sur
//! la sortie d'erreur (« listening on http://… », « model loaded », et, comme
//! le vrai avec `-lv 4`, la mémoire réservée par périphérique et les couches
//! sur la carte) ; options inconnues acceptées et journalisées. Comme le
//! vrai : `--version`, `--list-devices`, modèle absent (code 1), port pris
//! (code 1). `--port 0` : port libre, écrit dans la ligne « listening on ».
//! Fin : SIGINT ou SIGTERM (Windows : Ctrl+C ou Ctrl+Pause) → arrêt propre,
//! code 0 ; `chatonlm-loadfail` → code 1 ; `chatonlm-crash` → `abort` au
//! milieu d'une réponse ; `chatonlm-vulkanfail` → sur la carte (sans `-ngl
//! 0` ni `GGML_VK_VISIBLE_DEVICES=none`), erreur de Vulkan puis `abort` au
//! chargement, sur le processeur comme `chatonlm-simple` ; `chatonlm-bigctx`,
//! `chatonlm-bindfail[-always]` : voir `scenario`. Avec
//! `FAKE_LLAMA_STOP_MS`, l'arrêt dure ce temps après le premier SIGTERM, et
//! un second SIGTERM le fait planter (`abort`), comme b12110 ; à l'arrêt, le
//! journal dit si le fichier du modèle avait déjà disparu.

use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use crate::{Config, Exit, FakeLlama, Mode, Scenario, args};

/// Chargement par défaut du binaire : la sonde de l'app voit d'abord 503.
const DEFAULT_LOAD: Duration = Duration::from_millis(150);

/// Contexte au-delà duquel `chatonlm-bigctx` refuse de charger.
pub const BIG_CTX_LIMIT: u32 = 8192;

/// Ligne du journal quand le fichier du modèle a disparu avant l'arrêt.
pub const DELETED_BEFORE_STOP: &str = "fake-llama: model file deleted before the stop";

/// Variable qui porte les arguments quand la ligne de commande appartient à
/// un autre programme (binaire d'essai de l'app relancé), séparés par U+001F.
pub const ARGS_ENV: &str = "FAKE_LLAMA_ARGS";

/// Arguments de [`ARGS_ENV`], s'il est posé.
pub fn args_from_env() -> Option<Vec<String>> {
    let joined = std::env::var(ARGS_ENV).ok()?;
    Some(
        joined
            .split('\u{1f}')
            .filter(|arg| !arg.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

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

/// Ligne du journal au format du vrai, horodatage factice.
fn log(level: char, text: &str) {
    eprintln!("0.00.100.000 {level} {text}");
}

/// Lignes de chargement du vrai à `-lv 4` (Qwen3-0.6B Q4_K_M) : sur la
/// carte, 29 couches, poids, cache (selon le contexte) et calcul sur
/// `Vulkan0` ; sur le processeur, tout en mémoire vive.
fn memory_lines(gpu: bool, ctx: u32) {
    let kv = f64::from(ctx) * 448.0 / 4096.0;
    if gpu {
        log('I', "load_tensors: offloaded 29/29 layers to GPU");
        log(
            'I',
            "load_tensors:   CPU_Mapped model buffer size =   121.71 MiB",
        );
        log(
            'I',
            "load_tensors:      Vulkan0 model buffer size =   372.65 MiB",
        );
        log(
            'I',
            &format!("llama_kv_cache:    Vulkan0 KV buffer size = {kv:8.2} MiB"),
        );
        log(
            'I',
            "sched_reserve:    Vulkan0 compute buffer size =    26.01 MiB",
        );
        log(
            'I',
            "sched_reserve: Vulkan_Host compute buffer size =    12.02 MiB",
        );
    } else {
        log(
            'I',
            "load_tensors:   CPU_Mapped model buffer size =   372.65 MiB",
        );
        log(
            'I',
            &format!("llama_kv_cache:        CPU KV buffer size = {kv:8.2} MiB"),
        );
        log(
            'I',
            "sched_reserve:        CPU compute buffer size =    30.00 MiB",
        );
    }
}

/// Le faux en entier ; rend le code de sortie (ou s'arrête net, `abort`).
pub async fn run(argv: Vec<String>) -> i32 {
    // Clés aussi par l'environnement, comme le vrai (`LLAMA_API_KEY`,
    // `LLAMA_ARG_API_KEY_FILE`).
    let args = match args::parse_with_env(argv, |name| std::env::var(name).ok()) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    if args.version {
        println!("version: 0.9.0-dev (build 12110, commit cd959bae9)");
        println!("built with fake-llama for tests");
        return 0;
    }
    if args.list_devices {
        println!("Available devices:");
        println!("  Fake0: fake-llama device (8192 MiB, 8192 MiB free)");
        return 0;
    }
    if args.help {
        println!("fake-llama: faux llama-server des essais de ChatonLM (voir cli.rs)");
        return 0;
    }
    for option in &args.unknown {
        eprintln!("fake-llama: option inconnue acceptée : {option}");
    }
    let Some(model) = &args.model else {
        eprintln!("fake-llama: -m est exigé (le mode routeur n'est pas simulé)");
        return 1;
    };
    if !model.is_file() {
        let model = model.display();
        eprintln!(
            "E llama_model_load: error loading model: llama_model_loader: failed to load model from {model}"
        );
        eprintln!("E srv    load_model: failed to load model, '{model}'");
        eprintln!("E srv  server_run_i: exiting due to model loading error");
        return 1;
    }
    let mode = match std::env::var("FAKE_LLAMA_MODE").map(|mode| Mode::parse(&mode)) {
        Err(_) => Mode::Ok,
        Ok(Some(mode)) => mode,
        Ok(None) => {
            eprintln!(
                "fake-llama: FAKE_LLAMA_MODE inconnu (ok, silent, close, huge, status:<code>, loading:<ms>)"
            );
            return 2;
        }
    };
    let Ok(host) = args.host.parse::<IpAddr>() else {
        eprintln!("E srv  init: invalid host {}", args.host);
        return 1;
    };
    let mut config = Config::from_args(&args, mode);
    config.load_delay = number("FAKE_LLAMA_LOAD_MS").map_or(DEFAULT_LOAD, Duration::from_millis);
    let gpu = args.n_gpu_layers.as_deref() != Some("0")
        && std::env::var("GGML_VK_VISIBLE_DEVICES").as_deref() != Ok("none");
    let scenario = Scenario::of(config.alias.as_deref(), &config.model);
    if gpu && scenario == Scenario::VulkanFail {
        log('I', "ggml_vulkan: Found 1 Vulkan devices:");
        tokio::time::sleep(config.load_delay).await;
        eprintln!("terminate called after throwing an instance of 'vk::DeviceLostError'");
        eprintln!("  what():  vk::Queue::submit: ErrorDeviceLost");
        std::process::abort();
    }
    if scenario == Scenario::BigCtx && config.ctx > BIG_CTX_LIMIT {
        tokio::time::sleep(scenario.load_delay().unwrap_or(config.load_delay)).await;
        log(
            'E',
            "llama_init_from_model: failed to initialize the context: failed to allocate buffer for kv cache",
        );
        log(
            'E',
            &format!(
                "srv    load_model: failed to load model, '{}'",
                config.model
            ),
        );
        log('E', "srv  server_run_i: exiting due to model loading error");
        return 1;
    }
    if matches!(
        scenario,
        Scenario::BindFail | Scenario::BindFailAlways | Scenario::OtherAlias | Scenario::OtherCtx
    ) && let Ok(mut attempts) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{}.attempts", config.model))
    {
        let _ = writeln!(attempts, "{}", args.port);
    }
    if matches!(scenario, Scenario::BindFail | Scenario::BindFailAlways) {
        let bound = format!("{}.bound", config.model);
        if scenario == Scenario::BindFailAlways || !Path::new(&bound).exists() {
            let _ = std::fs::write(&bound, b"");
            log(
                'E',
                &format!(
                    "srv  init: couldn't bind HTTP server socket, hostname: {host}, port: {}",
                    args.port
                ),
            );
            log('E', "srv  server_run_i: exiting due to HTTP server error");
            return 1;
        }
    }
    memory_lines(gpu, config.ctx);
    let model = config.model.clone();
    let server = match FakeLlama::start_on(SocketAddr::new(host, args.port), config).await {
        Ok(server) => server,
        Err(error) => {
            eprintln!(
                "E srv  init: couldn't bind HTTP server socket, hostname: {host}, port: {}: {error}",
                args.port
            );
            return 1;
        }
    };
    let mut terminate = Terminate::listen();
    let exit = tokio::select! {
        _ = tokio::signal::ctrl_c() => None,
        _ = terminate.recv() => None,
        exit = server.exit_requested() => Some(exit),
    };
    if exit.is_none() {
        if !Path::new(&model).exists() {
            log('E', DELETED_BEFORE_STOP);
        }
        log('I', "srv    operator(): stopping on a termination signal");
        if let Some(stop) = number("FAKE_LLAMA_STOP_MS") {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(stop)) => {}
                _ = terminate.recv() => {
                    // Comme b12110 : « exit(1) » dans le gestionnaire, qui
                    // plante pendant l'arrêt déjà commencé.
                    eprintln!("Received second interrupt, terminating immediately.");
                    std::process::abort();
                }
            }
        }
    }
    server.stop().await;
    match exit {
        None => 0,
        Some(Exit::Code(code)) => code,
        Some(Exit::Abort) => std::process::abort(),
    }
}
