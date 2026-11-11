//! Le faux `llama-server` en ligne de commande (binaire `fake-llama`, et
//! relance d'un binaire d'essai de l'app : [`args_from_env`]) :
//!   fake-llama --host 127.0.0.1 --port <n> -m <modèle.gguf> [options de llama-server…]
//!   fake-llama gguf <sortie> [--arch --ctx --template --fim --vision …] (voir [`gguf`])
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
//! journal dit si le fichier du modèle avait déjà disparu. Avec
//! `FAKE_LLAMA_REGISTRY`, chaque instance note son adresse (voir
//! [`REGISTRY_ENV`]).

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

/// Nom (alias ou fichier) d'un modèle à cheval sur la carte et le
/// processeur : 22 couches sur 29 seulement sur la carte (escargot de l'app).
pub const SPLIT_NAME: &str = "chatonlm-split";

/// Lignes de chargement du vrai à `-lv 4` (Qwen3-0.6B Q4_K_M) : sur la
/// carte, 29 couches (22 pour [`SPLIT_NAME`]), poids, cache (selon le
/// contexte) et calcul sur `Vulkan0` ; sur le processeur, tout en mémoire
/// vive.
fn memory_lines(gpu: bool, ctx: u32, split: bool) {
    let kv = f64::from(ctx) * 448.0 / 4096.0;
    if gpu {
        log(
            'I',
            if split {
                "load_tensors: offloaded 22/29 layers to GPU"
            } else {
                "load_tensors: offloaded 29/29 layers to GPU"
            },
        );
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

/// Variable des e2e : dossier où chaque instance écrit `<pid>.json`
/// (`{"url", "alias", "model", "pid"}`) une fois à l'écoute, retiré à l'arrêt
/// propre. L'app choisit le port : l'essai y trouve l'adresse d'un modèle
/// pour lire ses requêtes (`/fake/requests`).
pub const REGISTRY_ENV: &str = "FAKE_LLAMA_REGISTRY";

/// Inscrit l'instance d'adresse `url` dans [`REGISTRY_ENV`] (écriture puis
/// renommage : un lecteur ne voit jamais un fichier à moitié écrit) ; rend
/// le fichier.
fn register(url: &str, alias: Option<&str>, model: &str) -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(std::env::var_os(REGISTRY_ENV)?);
    let pid = std::process::id();
    let entry = serde_json::json!({
        "url": url,
        "alias": alias,
        "model": model,
        "pid": pid,
    });
    std::fs::create_dir_all(&dir).ok()?;
    let file = dir.join(format!("{pid}.json"));
    let partial = dir.join(format!("{pid}.json.tmp"));
    std::fs::write(&partial, entry.to_string()).ok()?;
    std::fs::rename(&partial, &file).ok()?;
    Some(file)
}

/// Le faux en entier ; rend le code de sortie (ou s'arrête net, `abort`).
/// `fake-llama gguf …` : écrit un fichier GGUF d'essai (voir [`gguf`]).
pub async fn run(argv: Vec<String>) -> i32 {
    if argv.first().map(String::as_str) == Some("gguf") {
        return gguf::main(&argv[1..]);
    }
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
    let split = config
        .alias
        .iter()
        .chain([&config.model])
        .any(|name| name.to_ascii_lowercase().contains(SPLIT_NAME));
    memory_lines(gpu, config.ctx, split);
    let model = config.model.clone();
    // Port donné (l'app en donne toujours un) : inscrit AVANT d'écouter, donc
    // avant d'être prêt. Inscrit après, un disque très chargé (écritures
    // freinées) laissait l'app voir le modèle prêt sans que les essais
    // trouvent son instance.
    let address = SocketAddr::new(host, args.port);
    let early = (args.port != 0)
        .then(|| register(&format!("http://{address}"), args.alias.as_deref(), &model))
        .flatten();
    let server = match FakeLlama::start_on(address, config).await {
        Ok(server) => server,
        Err(error) => {
            if let Some(file) = early {
                let _ = std::fs::remove_file(file);
            }
            eprintln!(
                "E srv  init: couldn't bind HTTP server socket, hostname: {host}, port: {}: {error}",
                args.port
            );
            return 1;
        }
    };
    let registered = early.or_else(|| register(&server.url(), args.alias.as_deref(), &model));
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
    if let Some(file) = registered {
        let _ = std::fs::remove_file(file);
    }
    server.stop().await;
    match exit {
        None => 0,
        Some(Exit::Code(code)) => code,
        Some(Exit::Abort) => std::process::abort(),
    }
}

/// Formes du faux comparées aux réponses du vrai (`fixtures/`).
#[cfg(test)]
#[path = "fixtures.rs"]
mod fixtures;

/// `fake-llama gguf <sortie> [options]` : fichier GGUF d'essai à l'en-tête
/// lisible par l'app (`hub::gguf::meta`), pour les e2e qui inscrivent des
/// modèles (`CHATONLM_E2E_MODELS`) ou les rangent dans un dossier. Reprise
/// du synthétiseur de `fake-services` (`hf_models.rs`).
///
/// Options : `--arch <a>` (défaut `qwen3`), `--ctx <n>` (contexte
/// d'entraînement, défaut 40 960), `--size-label <t>` (défaut « 0.6B »),
/// `--name <t>` (`general.name`), `--template <gabarit>` (gabarit Jinja tel
/// quel), `--tools` et `--think` (ajoutés au gabarit : outils, réflexion),
/// `--fim` (jetons FIM dans le vocabulaire), `--base` (modèle de base :
/// `general.basename` sans `general.finetune`), `--vision` ou `--audio`
/// (projecteur `clip` qui lit les images, ou l'audio seulement), `--size <n>`
/// (taille totale, octets déterministes après l'en-tête).
pub mod gguf {
    use std::path::PathBuf;

    /// Options lues.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Options {
        pub out: PathBuf,
        pub arch: String,
        pub ctx: u32,
        pub size_label: String,
        pub name: Option<String>,
        pub template: Option<String>,
        pub tools: bool,
        pub think: bool,
        pub fim: bool,
        pub base: bool,
        pub vision: bool,
        pub audio: bool,
        pub size: Option<u64>,
    }

    /// Lit les arguments (sans le mot `gguf`).
    pub fn parse(argv: &[String]) -> Result<Options, String> {
        let mut options = Options {
            out: PathBuf::new(),
            arch: "qwen3".into(),
            ctx: 40_960,
            size_label: "0.6B".into(),
            name: None,
            template: None,
            tools: false,
            think: false,
            fim: false,
            base: false,
            vision: false,
            audio: false,
            size: None,
        };
        let mut out = None;
        let mut args = argv.iter();
        while let Some(arg) = args.next() {
            let mut value = |name: &str| {
                args.next()
                    .cloned()
                    .ok_or_else(|| format!("fake-llama gguf: {name} attend une valeur"))
            };
            match arg.as_str() {
                "--arch" => options.arch = value("--arch")?,
                "--ctx" => {
                    options.ctx = value("--ctx")?
                        .parse()
                        .map_err(|_| "fake-llama gguf: --ctx attend un nombre".to_owned())?;
                }
                "--size-label" => options.size_label = value("--size-label")?,
                "--name" => options.name = Some(value("--name")?),
                "--template" => options.template = Some(value("--template")?),
                "--size" => {
                    options.size = Some(
                        value("--size")?
                            .parse()
                            .map_err(|_| "fake-llama gguf: --size attend un nombre".to_owned())?,
                    );
                }
                "--tools" => options.tools = true,
                "--think" => options.think = true,
                "--fim" => options.fim = true,
                "--base" => options.base = true,
                "--vision" => options.vision = true,
                "--audio" => options.audio = true,
                other if other.starts_with("--") => {
                    return Err(format!("fake-llama gguf: option inconnue {other}"));
                }
                path if out.is_none() => out = Some(PathBuf::from(path)),
                extra => return Err(format!("fake-llama gguf: argument en trop {extra}")),
            }
        }
        if options.vision && options.audio {
            return Err("fake-llama gguf: --vision ou --audio, pas les deux".into());
        }
        options.out = out.ok_or("Usage : fake-llama gguf <sortie> [options]")?;
        Ok(options)
    }

    /// Valeur d'une paire GGUF.
    enum Kv {
        Str(String),
        U32(u32),
        Bool(bool),
        Strs(Vec<String>),
    }

    fn header(pairs: &[(&str, Kv)]) -> Vec<u8> {
        let mut out = b"GGUF".to_vec();
        out.extend(3u32.to_le_bytes());
        out.extend(0u64.to_le_bytes());
        out.extend((pairs.len() as u64).to_le_bytes());
        let string = |out: &mut Vec<u8>, text: &str| {
            out.extend((text.len() as u64).to_le_bytes());
            out.extend(text.as_bytes());
        };
        for (key, value) in pairs {
            string(&mut out, key);
            match value {
                Kv::Str(text) => {
                    out.extend(8u32.to_le_bytes());
                    string(&mut out, text);
                }
                Kv::U32(number) => {
                    out.extend(4u32.to_le_bytes());
                    out.extend(number.to_le_bytes());
                }
                Kv::Bool(value) => {
                    out.extend(7u32.to_le_bytes());
                    out.push(u8::from(*value));
                }
                Kv::Strs(list) => {
                    out.extend(9u32.to_le_bytes());
                    out.extend(8u32.to_le_bytes());
                    out.extend((list.len() as u64).to_le_bytes());
                    for text in list {
                        string(&mut out, text);
                    }
                }
            }
        }
        out
    }

    /// Contenu du fichier.
    pub fn content(options: &Options) -> Vec<u8> {
        let mut bytes = if options.vision || options.audio {
            header(&[
                ("general.architecture", Kv::Str("clip".into())),
                ("clip.has_vision_encoder", Kv::Bool(options.vision)),
                ("clip.has_audio_encoder", Kv::Bool(options.audio)),
            ])
        } else {
            let name = options.name.clone().unwrap_or_else(|| {
                options
                    .out
                    .file_stem()
                    .map_or("model".into(), |stem| stem.to_string_lossy().into_owned())
            });
            let mut pairs = vec![
                ("general.architecture", Kv::Str(options.arch.clone())),
                ("general.name", Kv::Str(name.clone())),
                ("general.size_label", Kv::Str(options.size_label.clone())),
                ("general.file_type", Kv::U32(15)),
            ];
            let context_key = format!("{}.context_length", options.arch);
            if options.base {
                pairs.push(("general.basename", Kv::Str(name)));
            }
            let mut tokens = vec!["<|im_start|>".to_owned(), "<|im_end|>".to_owned()];
            if options.fim {
                for token in ["<|fim_prefix|>", "<|fim_suffix|>", "<|fim_middle|>"] {
                    tokens.push(token.to_owned());
                }
            }
            pairs.push(("tokenizer.ggml.tokens", Kv::Strs(tokens)));
            let mut template = options.template.clone().unwrap_or_else(|| {
                "{%- for message in messages %}{{ message.content }}{%- endfor %}".to_owned()
            });
            if options.tools {
                template.push_str("{%- if tools %}<tool_call>{%- endif %}");
            }
            if options.think {
                template.push_str("{%- if enable_thinking %}<think>{%- endif %}");
            }
            pairs.push(("tokenizer.chat_template", Kv::Str(template)));
            let mut bytes = header(&pairs);
            // Clé du contexte à part : son nom dépend de l'architecture.
            let count = u64::from_le_bytes(bytes[16..24].try_into().unwrap()) + 1;
            bytes[16..24].copy_from_slice(&count.to_le_bytes());
            bytes.extend((context_key.len() as u64).to_le_bytes());
            bytes.extend(context_key.as_bytes());
            bytes.extend(4u32.to_le_bytes());
            bytes.extend(options.ctx.to_le_bytes());
            bytes
        };
        let size = options.size.unwrap_or(4096).max(bytes.len() as u64) as usize;
        while bytes.len() < size {
            let index = bytes.len();
            bytes.push((index as u8).wrapping_mul(31) ^ 0x5a);
        }
        bytes
    }

    /// Écrit le fichier (dossiers créés) ; code de sortie.
    pub fn main(argv: &[String]) -> i32 {
        let options = match parse(argv) {
            Ok(options) => options,
            Err(error) => {
                eprintln!("{error}");
                return 2;
            }
        };
        if let Some(parent) = options.out.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            eprintln!("fake-llama gguf: {}: {error}", parent.display());
            return 1;
        }
        if let Err(error) = std::fs::write(&options.out, content(&options)) {
            eprintln!("fake-llama gguf: {}: {error}", options.out.display());
            return 1;
        }
        0
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn args(list: &[&str]) -> Vec<String> {
            list.iter().map(|arg| (*arg).to_owned()).collect()
        }

        #[test]
        fn options_are_read_and_checked() {
            let options = parse(&args(&[
                "out/m.gguf",
                "--arch",
                "gemma3",
                "--ctx",
                "8192",
                "--fim",
                "--tools",
                "--size",
                "9000",
            ]))
            .unwrap();
            assert_eq!(options.out, PathBuf::from("out/m.gguf"));
            assert_eq!((options.arch.as_str(), options.ctx), ("gemma3", 8192));
            assert!(options.fim && options.tools && !options.think);
            assert_eq!(content(&options).len(), 9000);
            for bad in [
                &["--ctx", "x", "m.gguf"][..],
                &["m.gguf", "--nope"],
                &["m.gguf", "other.gguf"],
                &["--fim"],
                &["m.gguf", "--vision", "--audio"],
                &["m.gguf", "--arch"],
            ] {
                assert!(parse(&args(bad)).is_err(), "{bad:?}");
            }
        }

        /// En-tête lu comme l'app le lit : clés dans l'ordre, nombre de
        /// paires juste (la clé du contexte ajoutée à la fin compte).
        #[test]
        fn the_header_counts_every_pair() {
            let options = parse(&args(&["m.gguf", "--arch", "llama", "--ctx", "2048"])).unwrap();
            let bytes = content(&options);
            assert_eq!(&bytes[..4], b"GGUF");
            let count = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
            assert_eq!(count, 7);
            let key = b"llama.context_length";
            let at = bytes.windows(key.len()).position(|w| w == key).unwrap();
            let value = &bytes[at + key.len() + 4..at + key.len() + 8];
            assert_eq!(u32::from_le_bytes(value.try_into().unwrap()), 2048);
            let projector = content(&parse(&args(&["p.gguf", "--audio"])).unwrap());
            assert!(projector.windows(4).any(|w| w == b"clip"));
        }
    }
}
