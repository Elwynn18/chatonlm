//! Règles pures du lancement d'un `llama-server` (sans processus ni état) :
//! durée en mémoire (`keep_alive` d'Ollama), contexte par défaut, arguments,
//! environnement et chemins longs de Windows. Essayées sous Linux, y compris
//! ce qui ne sert que sous Windows (paramètre `windows`).

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::hub::fit;

/// Durée en mémoire par défaut d'Ollama (`OLLAMA_KEEP_ALIVE` absent) : 5 min.
pub const DEFAULT_KEEP_ALIVE: Duration = Duration::from_secs(300);

/// Variable de ggml qui choisit les cartes Vulkan visibles. Une valeur qui
/// n'est pas une liste de numéros (`none`) n'en laisse aucune : le moteur ne
/// crée alors aucun périphérique Vulkan (vérifié sur b12110, rapport L3). Ni
/// `GGML_DISABLE_VULKAN` (sans effet depuis le chargement dynamique des
/// moteurs) ni une valeur vide (que le CRT de Windows pourrait prendre pour
/// une variable absente).
pub const VK_DEVICES_ENV: &str = "GGML_VK_VISIBLE_DEVICES";
pub const VK_DEVICES_NONE: &str = "none";

/// Variable de la clé d'API (jamais en ligne de commande : lisible dans la
/// liste des processus).
pub const API_KEY_ENV: &str = "LLAMA_API_KEY";
/// Cache de llama.cpp (téléchargements `-hf`, que l'app n'emploie pas) :
/// exigé sous Windows (exception sans lui ni `LOCALAPPDATA`).
pub const CACHE_ENV: &str = "LLAMA_CACHE";

/// Durée en mémoire d'un modèle après sa dernière requête (le `keep_alive`
/// d'Ollama).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeepAlive {
    /// Défaut d'Ollama : 5 minutes.
    Default,
    For(Duration),
    /// Jusqu'au déchargement demandé (ou la fin de l'app).
    Forever,
    /// Déchargé dès sa dernière requête finie.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "aucun appelant ne le demande encore")
    )]
    Unload,
}

impl KeepAlive {
    /// Réglage `keep_alive` de l'app (secondes, comme le champ d'Ollama) :
    /// 0 = défaut du serveur (5 min), négatif = toujours.
    pub fn from_setting(seconds: i64) -> KeepAlive {
        match seconds {
            0 => KeepAlive::Default,
            seconds if seconds < 0 => KeepAlive::Forever,
            seconds => KeepAlive::For(Duration::from_secs(seconds.unsigned_abs())),
        }
    }

    /// Durée avant le déchargement ; `None` : jamais.
    pub fn duration(self) -> Option<Duration> {
        match self {
            KeepAlive::Default => Some(DEFAULT_KEEP_ALIVE),
            KeepAlive::For(duration) => Some(duration),
            KeepAlive::Forever => None,
            KeepAlive::Unload => Some(Duration::ZERO),
        }
    }
}

/// Options d'un chargement ([`super::Engine::ensure`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadOptions {
    /// Contexte demandé ; `None` ou 0 : celui qu'Ollama choisirait
    /// ([`context_for`]).
    pub ctx: Option<u32>,
    pub keep_alive: KeepAlive,
    /// Calcul sur le processeur seulement (aucune carte Vulkan).
    pub cpu_only: bool,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            ctx: None,
            keep_alive: KeepAlive::Default,
            cpu_only: false,
        }
    }
}

impl LoadOptions {
    /// Options d'un envoi d'après les réglages de l'app (`context_length`,
    /// `keep_alive`).
    pub fn from_settings(context_length: u32, keep_alive: i64) -> LoadOptions {
        LoadOptions {
            ctx: Some(context_length).filter(|ctx| *ctx > 0),
            keep_alive: KeepAlive::from_setting(keep_alive),
            cpu_only: false,
        }
    }
}

/// Contexte de lancement, comme Ollama 0.40 (`server/routes.go`, palier de
/// VRAM, et `server/sched.go`) : le demandé, sinon 4 096 / 32 768 / 262 144
/// selon la VRAM totale ([`fit::default_context`]), ramené au contexte
/// d'entraînement du modèle, au moins 2 048 pour un modèle de vision et au
/// moins 4 jetons. Jamais laissé à llama-server : sans `-c`, il prend le
/// contexte d'entraînement (des Gio de cache pour un petit modèle).
pub fn context_for(requested: Option<u32>, vram_total: u64, max: Option<u32>, vision: bool) -> u32 {
    let wanted = requested
        .filter(|ctx| *ctx > 0)
        .unwrap_or_else(|| fit::default_context(vram_total));
    fit::model_context(wanted, max, vision).max(4)
}

/// Variable des essais de l'app qui impose le processeur à tout lancement
/// (e2e réels sans carte, ou voulus sur processeur).
pub const CPU_ONLY_ENV: &str = "CHATONLM_LLAMA_CPU_ONLY";

/// Processeur imposé par `CHATONLM_LLAMA_CPU_ONLY=1` (builds de débogage ou
/// `e2e` seulement, comme `CHATONLM_E2E_HARDWARE`) : les `GGML_*` hérités
/// étant retirés de l'environnement du processus, c'est le seul moyen pour un
/// e2e réel de rester sur le processeur (relecture L8, M2).
pub fn forced_cpu() -> bool {
    #[cfg(any(debug_assertions, feature = "e2e"))]
    {
        cpu_only_value(std::env::var_os(CPU_ONLY_ENV).as_deref())
    }
    #[cfg(not(any(debug_assertions, feature = "e2e")))]
    {
        false
    }
}

/// Lecture de la variable : `1` seulement.
#[cfg_attr(not(any(test, debug_assertions, feature = "e2e")), allow(dead_code))]
pub(crate) fn cpu_only_value(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| value == "1")
}

/// Ce que lance un processus : deux demandes de même `Spec` partagent le
/// processus ; une autre (contexte, processeur) le relance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub model: PathBuf,
    pub mmproj: Option<PathBuf>,
    pub ctx: u32,
    pub cpu: bool,
}

/// Plafond du cache des invites gardé en RAM par chaque processus
/// (`--cache-ram`, en Mio) : un seizième de la RAM totale, entre 256 Mio et
/// 2 Gio ; RAM inconnue : 1 Gio. Le défaut de b12110 (8 Gio par processus,
/// jusqu'à trois processus) prendrait la RAM d'une petite machine.
pub fn cache_ram_mib(total_ram_bytes: Option<u64>) -> u32 {
    const MIB: u64 = 1024 * 1024;
    match total_ram_bytes.filter(|total| *total > 0) {
        None => 1024,
        Some(total) => (total / 16 / MIB).clamp(256, 2048) as u32,
    }
}

/// Arguments de `llama-server` (le binaire et ses options), sans la clé.
/// `-lv 4` : les lignes de chargement (mémoire par périphérique, couches sur
/// la carte) ne sont écrites qu'à ce niveau (b12110) ; une requête y ajoute
/// une vingtaine de lignes au journal. `cache_ram` : [`cache_ram_mib`].
pub fn server_args(
    spec: &Spec,
    alias: &str,
    port: u16,
    cache_ram: u32,
    windows: bool,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--alias",
        alias,
        "-c",
        &spec.ctx.to_string(),
        "-np",
        "1",
        "--no-webui",
        "--offline",
        "-lv",
        "4",
        "--cache-ram",
        &cache_ram.to_string(),
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.push("-m".into());
    args.push(long_path(&spec.model, windows).into_os_string());
    if let Some(mmproj) = &spec.mmproj {
        args.push("--mmproj".into());
        args.push(long_path(mmproj, windows).into_os_string());
    }
    if spec.cpu {
        args.push("-ngl".into());
        args.push("0".into());
    }
    args
}

/// Chemin donné à llama-server : sous Windows, un chemin absolu de 260
/// caractères ou plus prend le préfixe `\\?\` (`\\?\UNC\` pour un partage),
/// sans quoi l'ouverture échoue (limite `MAX_PATH`) ; ailleurs, inchangé.
pub fn long_path(path: &Path, windows: bool) -> PathBuf {
    const MAX_PATH: usize = 260;
    let text = path.as_os_str().to_string_lossy();
    if !windows || text.encode_utf16().count() < MAX_PATH || text.starts_with(r"\\?\") {
        return path.to_path_buf();
    }
    let unix_slashes = text.replace('/', "\\");
    if let Some(share) = unix_slashes.strip_prefix(r"\\") {
        return PathBuf::from(format!(r"\\?\UNC\{share}"));
    }
    let bytes = unix_slashes.as_bytes();
    let drive = bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && &bytes[1..3] == b":\\";
    if drive {
        PathBuf::from(format!(r"\\?\{unix_slashes}"))
    } else {
        path.to_path_buf()
    }
}

/// Préfixes des variables lues par llama.cpp et ggml (`LLAMA_ARG_*` donne
/// n'importe quelle option, `LLAMA_API_KEY` ou `LLAMA_ARG_API_KEY_FILE` des
/// clés de plus) : celles héritées du bureau sont retirées, seules celles
/// de l'app passent.
const ENGINE_PREFIXES: [&str; 2] = ["LLAMA_", "GGML_"];

/// Environnement de llama-server : `inherited` (déjà nettoyé par
/// `process::scrub`) sans les variables de llama.cpp ni de ggml (casse
/// ignorée sous Windows), plus la clé, le cache et, pour le processeur
/// seul, aucune carte Vulkan. Sous Windows, `USERPROFILE` (retiré par
/// `scrub` pour tout moteur) est rendu tel que l'app l'a reçu : les pilotes
/// graphiques et le CRT s'en servent.
pub fn server_env(
    mut inherited: BTreeMap<OsString, OsString>,
    api_key: &str,
    cache: &Path,
    cpu: bool,
    user_profile: Option<&OsStr>,
    windows: bool,
) -> BTreeMap<OsString, OsString> {
    inherited.retain(|key, _| {
        let key = key.to_string_lossy();
        let key = if windows {
            key.to_ascii_uppercase()
        } else {
            key.into_owned()
        };
        !ENGINE_PREFIXES.iter().any(|prefix| key.starts_with(prefix))
    });
    inherited.insert(API_KEY_ENV.into(), api_key.into());
    inherited.insert(CACHE_ENV.into(), cache.as_os_str().to_owned());
    if cpu {
        inherited.insert(VK_DEVICES_ENV.into(), VK_DEVICES_NONE.into());
    }
    if windows && let Some(profile) = user_profile.filter(|profile| !profile.is_empty()) {
        inherited.insert("USERPROFILE".into(), profile.to_owned());
    }
    inherited
}

/// Clé d'API d'un lancement : 32 octets aléatoires en hexadécimal (ni
/// virgule ni espace : llama-server coupe les clés aux virgules).
pub fn api_key() -> String {
    use rand::RngExt;
    let bytes: [u8; 32] = rand::rng().random();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn keep_alive_follows_the_setting_of_ollama() {
        assert_eq!(KeepAlive::from_setting(0), KeepAlive::Default);
        assert_eq!(KeepAlive::from_setting(-1), KeepAlive::Forever);
        assert_eq!(KeepAlive::from_setting(-300), KeepAlive::Forever);
        assert_eq!(
            KeepAlive::from_setting(900),
            KeepAlive::For(Duration::from_secs(900))
        );
        assert_eq!(KeepAlive::Default.duration(), Some(DEFAULT_KEEP_ALIVE));
        assert_eq!(DEFAULT_KEEP_ALIVE, Duration::from_secs(300));
        assert_eq!(KeepAlive::Forever.duration(), None);
        assert_eq!(KeepAlive::Unload.duration(), Some(Duration::ZERO));
        let options = LoadOptions::from_settings(0, 60);
        assert_eq!(options.ctx, None);
        assert_eq!(options.keep_alive, KeepAlive::For(Duration::from_secs(60)));
        assert_eq!(LoadOptions::from_settings(8192, 0).ctx, Some(8192));
    }

    /// Paliers d'Ollama 0.40 (47 et 23 Gio), contexte d'entraînement,
    /// vision.
    #[test]
    fn the_default_context_is_the_one_of_ollama() {
        assert_eq!(context_for(None, 0, None, false), 4096);
        assert_eq!(context_for(Some(0), 8 * GIB, None, false), 4096);
        assert_eq!(context_for(None, 24 * GIB, None, false), 32_768);
        assert_eq!(context_for(None, 23 * GIB, Some(40_960), false), 32_768);
        assert_eq!(context_for(None, 48 * GIB, Some(40_960), false), 40_960);
        assert_eq!(context_for(None, 48 * GIB, None, false), 262_144);
        // Demandé : gardé, sauf au-delà du modèle.
        assert_eq!(context_for(Some(8192), 0, Some(40_960), false), 8192);
        assert_eq!(context_for(Some(65_536), 0, Some(40_960), false), 40_960);
        // Vision : au moins 2 048, même si le modèle annonce moins.
        assert_eq!(context_for(Some(1024), 0, Some(1024), true), 2048);
        assert_eq!(context_for(Some(1024), 0, Some(1024), false), 1024);
        assert_eq!(context_for(Some(1), 0, None, false), 4);
    }

    #[test]
    fn the_arguments_never_hold_the_key_and_always_a_context() {
        let spec = Spec {
            model: PathBuf::from("/m/a.gguf"),
            mmproj: Some(PathBuf::from("/m/mmproj.gguf")),
            ctx: 8192,
            cpu: false,
        };
        let args: Vec<String> = server_args(&spec, "a/b:Q4_K_M", 4321, 512, false)
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect();
        assert_eq!(
            args.join(" "),
            "--host 127.0.0.1 --port 4321 --alias a/b:Q4_K_M -c 8192 -np 1 --no-webui \
             --offline -lv 4 --cache-ram 512 -m /m/a.gguf --mmproj /m/mmproj.gguf"
        );
        assert!(!args.iter().any(|arg| arg.contains("api-key")));
        let cpu = Spec {
            mmproj: None,
            cpu: true,
            ..spec
        };
        let args: Vec<String> = server_args(&cpu, "x", 1, 256, false)
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect();
        assert!(args.ends_with(&["-m".into(), "/m/a.gguf".into(), "-ngl".into(), "0".into()]));
        assert!(!args.contains(&"--mmproj".to_owned()));
    }

    #[test]
    fn the_prompt_cache_takes_a_sixteenth_of_the_memory() {
        const GIB: u64 = 1024 * 1024 * 1024;
        assert_eq!(cache_ram_mib(Some(8 * GIB)), 512);
        assert_eq!(cache_ram_mib(Some(16 * GIB)), 1024);
        assert_eq!(cache_ram_mib(Some(64 * GIB)), 2048);
        assert_eq!(cache_ram_mib(Some(2 * GIB)), 256);
        assert_eq!(cache_ram_mib(Some(0)), 1024);
        assert_eq!(cache_ram_mib(None), 1024);
    }

    #[test]
    fn long_windows_paths_get_their_prefix() {
        let short = Path::new(r"C:\Users\a\model.gguf");
        assert_eq!(long_path(short, true), short);
        let long = format!(r"C:\Users\a\{}\model.gguf", "d".repeat(260));
        assert_eq!(
            long_path(Path::new(&long), true),
            PathBuf::from(format!(r"\\?\{long}"))
        );
        // Barres obliques ramenées, partage réseau, déjà préfixé, relatif.
        let slashes = format!("C:/Users/{}/m.gguf", "d".repeat(260));
        assert_eq!(
            long_path(Path::new(&slashes), true),
            PathBuf::from(format!(r"\\?\C:\Users\{}\m.gguf", "d".repeat(260)))
        );
        let share = format!(r"\\server\share\{}\m.gguf", "d".repeat(260));
        assert_eq!(
            long_path(Path::new(&share), true),
            PathBuf::from(format!(r"\\?\UNC\server\share\{}\m.gguf", "d".repeat(260)))
        );
        let prefixed = format!(r"\\?\C:\{}", "d".repeat(300));
        assert_eq!(long_path(Path::new(&prefixed), true), Path::new(&prefixed));
        let relative = "d".repeat(300);
        assert_eq!(long_path(Path::new(&relative), true), Path::new(&relative));
        // Sous Linux : jamais touché.
        assert_eq!(long_path(Path::new(&long), false), Path::new(&long));
        // 259 caractères : rien ; 260 : le préfixe.
        let at = |n: usize| format!(r"C:\{}", "d".repeat(n - 3));
        assert_eq!(long_path(Path::new(&at(259)), true), Path::new(&at(259)));
        assert!(
            long_path(Path::new(&at(260)), true)
                .to_string_lossy()
                .starts_with(r"\\?\")
        );
    }

    #[test]
    fn the_environment_keeps_only_the_variables_of_the_app() {
        let inherited: BTreeMap<OsString, OsString> = [
            ("PATH", "/usr/bin"),
            ("LLAMA_ARG_HOST", "0.0.0.0"),
            ("LLAMA_API_KEY", "known"),
            ("LLAMA_ARG_API_KEY_FILE", "/tmp/keys"),
            ("GGML_VK_VISIBLE_DEVICES", "0"),
            ("llama_arg_port", "80"),
            ("XDG_CACHE_HOME", "/home/u/.cache"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        let env = server_env(
            inherited.clone(),
            "k3y",
            Path::new("/data/engine/cache"),
            false,
            Some(OsStr::new(r"C:\Users\u")),
            false,
        );
        let keys: Vec<_> = env
            .keys()
            .map(|k| k.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            keys,
            [
                "LLAMA_API_KEY",
                "LLAMA_CACHE",
                "PATH",
                "XDG_CACHE_HOME",
                "llama_arg_port"
            ]
        );
        assert_eq!(env[OsStr::new("LLAMA_API_KEY")], "k3y");
        // Sous Windows, la casse ne compte pas ; USERPROFILE rendu.
        let env = server_env(
            inherited,
            "k3y",
            Path::new(r"C:\data\cache"),
            true,
            Some(OsStr::new(r"C:\Users\u")),
            true,
        );
        assert!(!env.contains_key(OsStr::new("llama_arg_port")));
        assert_eq!(env[OsStr::new("USERPROFILE")], r"C:\Users\u");
        assert_eq!(env[OsStr::new(VK_DEVICES_ENV)], VK_DEVICES_NONE);
    }

    #[test]
    fn keys_are_long_hexadecimal_and_new_each_time() {
        let (a, b) = (api_key(), api_key());
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
