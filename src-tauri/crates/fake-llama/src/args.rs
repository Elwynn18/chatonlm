//! Options de la ligne de commande, lues comme `llama-server` (b12110) les
//! lit pour celles que l'app passe ; les autres sont acceptées et gardées
//! (le vrai serveur refuse une option inconnue : le faux la journalise, pour
//! qu'un essai voie ce que l'app a passé). Variables d'environnement lues
//! comme le vrai (avant la ligne de commande, les clés s'ajoutent) :
//! `LLAMA_API_KEY` et `LLAMA_ARG_API_KEY_FILE`.

use std::path::PathBuf;

/// Options reçues.
#[derive(Clone, Debug, PartialEq)]
pub struct Args {
    pub host: String,
    pub port: u16,
    pub model: Option<PathBuf>,
    pub alias: Option<String>,
    /// Clés acceptées : `LLAMA_API_KEY`, `--api-key` (coupées aux virgules,
    /// vides ignorées), lignes de `--api-key-file` (sauf vides et `#…`).
    pub api_keys: Vec<String>,
    /// `-c` ; 0 = contexte d'entraînement du modèle (comme le vrai).
    pub ctx_size: u32,
    /// `-np` ; `None` = automatique (4 emplacements chez le vrai).
    pub parallel: Option<u32>,
    pub mmproj: Option<PathBuf>,
    pub n_gpu_layers: Option<String>,
    pub no_webui: bool,
    pub offline: bool,
    pub metrics: bool,
    pub jinja: bool,
    pub sleep_idle_seconds: Option<u64>,
    pub version: bool,
    pub list_devices: bool,
    pub help: bool,
    /// Options inconnues, avec leur valeur s'il y en a une (`--foo bar` ou
    /// `--foo=bar`), dans l'ordre.
    pub unknown: Vec<String>,
    /// Arguments reçus, tels quels.
    pub argv: Vec<String>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8080,
            model: None,
            alias: None,
            api_keys: Vec::new(),
            ctx_size: 4096,
            parallel: None,
            mmproj: None,
            n_gpu_layers: None,
            no_webui: false,
            offline: false,
            metrics: false,
            jinja: true,
            sleep_idle_seconds: None,
            version: false,
            list_devices: false,
            help: false,
            unknown: Vec::new(),
            argv: Vec::new(),
        }
    }
}

/// Erreur de lecture : message du vrai serveur.
#[derive(Debug, PartialEq, Eq)]
pub struct ArgError(pub String);

impl ArgError {
    /// `error: invalid argument: …`
    fn invalid(what: impl std::fmt::Display) -> ArgError {
        ArgError(format!("error: invalid argument: {what}"))
    }
}

impl std::fmt::Display for ArgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Clés d'une valeur de `--api-key` : coupée aux virgules, comme le vrai
/// (une clé ne contient donc jamais de virgule).
fn push_keys(keys: &mut Vec<String>, value: &str) {
    keys.extend(
        value
            .split(',')
            .filter(|key| !key.is_empty())
            .map(str::to_owned),
    );
}

/// Clés d'un fichier de `--api-key-file` : une par ligne, sans les lignes
/// vides ni celles qui commencent par `#` (rien d'autre n'est retiré).
fn push_key_file(keys: &mut Vec<String>, path: &str) -> Result<(), ArgError> {
    let text = std::fs::read_to_string(path).map_err(|_| {
        ArgError(format!(
            "error while handling argument \"--api-key-file\": error: failed to open file '{path}'"
        ))
    })?;
    keys.extend(
        text.lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_owned),
    );
    Ok(())
}

/// Une option inconnue est suivie de sa valeur si le mot suivant n'est pas
/// une option (un nombre négatif reste une valeur).
fn is_value(word: &str) -> bool {
    !word.starts_with('-') || word.parse::<f64>().is_ok()
}

/// Lit les arguments (sans le nom du programme), sans environnement.
pub fn parse<I, S>(arguments: I) -> Result<Args, ArgError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    parse_with_env(arguments, |_| None)
}

/// Lit les arguments (sans le nom du programme) et les variables
/// d'environnement que le vrai lit pour les clés (`env` : valeur d'une
/// variable).
pub fn parse_with_env<I, S>(
    arguments: I,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Args, ArgError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let argv: Vec<String> = arguments.into_iter().map(Into::into).collect();
    let mut args = Args {
        argv: argv.clone(),
        ..Args::default()
    };
    if let Some(value) = env("LLAMA_API_KEY") {
        push_keys(&mut args.api_keys, &value);
    }
    if let Some(path) = env("LLAMA_ARG_API_KEY_FILE") {
        push_key_file(&mut args.api_keys, &path)?;
    }
    let mut words = argv.into_iter().peekable();
    while let Some(word) = words.next() {
        // `--opt=valeur` : même sens que `--opt valeur`.
        let (name, inline) = match word.split_once('=') {
            Some((name, value)) if name.starts_with("--") => {
                (name.to_owned(), Some(value.to_owned()))
            }
            _ => (word.clone(), None),
        };
        let value = |words: &mut std::iter::Peekable<std::vec::IntoIter<String>>| {
            inline
                .clone()
                .or_else(|| words.next())
                .ok_or_else(|| ArgError::invalid(&name))
        };
        match name.as_str() {
            "--host" => args.host = value(&mut words)?,
            "--port" => {
                let text = value(&mut words)?;
                args.port = text
                    .parse()
                    .map_err(|_| ArgError::invalid(format!("--port {text}")))?;
            }
            "-m" | "--model" => args.model = Some(PathBuf::from(value(&mut words)?)),
            "-a" | "--alias" => args.alias = Some(value(&mut words)?),
            "--api-key" => push_keys(&mut args.api_keys, &value(&mut words)?),
            "--api-key-file" => push_key_file(&mut args.api_keys, &value(&mut words)?)?,
            "-c" | "--ctx-size" => {
                let text = value(&mut words)?;
                args.ctx_size = text
                    .parse()
                    .map_err(|_| ArgError::invalid(format!("{name} {text}")))?;
            }
            "-np" | "--parallel" => {
                let text = value(&mut words)?;
                let number: i64 = text
                    .parse()
                    .map_err(|_| ArgError::invalid(format!("{name} {text}")))?;
                args.parallel = u32::try_from(number).ok().filter(|n| *n > 0);
            }
            "-mm" | "--mmproj" => args.mmproj = Some(PathBuf::from(value(&mut words)?)),
            "-ngl" | "--gpu-layers" | "--n-gpu-layers" => {
                args.n_gpu_layers = Some(value(&mut words)?)
            }
            "--sleep-idle-seconds" => {
                let text = value(&mut words)?;
                args.sleep_idle_seconds = Some(
                    text.parse()
                        .map_err(|_| ArgError::invalid(format!("{name} {text}")))?,
                );
            }
            "--no-webui" | "--no-ui" => args.no_webui = true,
            "--offline" => args.offline = true,
            "--metrics" => args.metrics = true,
            "--jinja" => args.jinja = true,
            "--no-jinja" => args.jinja = false,
            "--version" => args.version = true,
            "--list-devices" => args.list_devices = true,
            "-h" | "--help" => args.help = true,
            _ => {
                let mut entry = word.clone();
                if inline.is_none() && words.peek().is_some_and(|next| is_value(next)) {
                    entry.push(' ');
                    entry.push_str(&words.next().unwrap_or_default());
                }
                args.unknown.push(entry);
            }
        }
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_options_of_the_app_are_read() {
        let args = parse([
            "--host",
            "127.0.0.1",
            "--port",
            "4321",
            "--api-key",
            "k",
            "-m",
            "/m/a.gguf",
            "--mmproj",
            "/m/p.gguf",
            "-c",
            "8192",
            "-np",
            "1",
            "--no-webui",
            "--offline",
            "--metrics",
            "--alias",
            "a/b:Q4_K_M",
            "-ngl",
            "99",
            "--sleep-idle-seconds",
            "5",
        ])
        .unwrap();
        assert_eq!(args.port, 4321);
        assert_eq!(args.api_keys, ["k"]);
        assert_eq!(args.model, Some(PathBuf::from("/m/a.gguf")));
        assert_eq!(args.mmproj, Some(PathBuf::from("/m/p.gguf")));
        assert_eq!(args.ctx_size, 8192);
        assert_eq!(args.parallel, Some(1));
        assert!(args.no_webui && args.offline && args.metrics);
        assert_eq!(args.alias.as_deref(), Some("a/b:Q4_K_M"));
        assert_eq!(args.n_gpu_layers.as_deref(), Some("99"));
        assert_eq!(args.sleep_idle_seconds, Some(5));
        assert!(args.unknown.is_empty());
        assert_eq!(args.argv.len(), 23);
    }

    #[test]
    fn unknown_options_are_kept_with_their_value() {
        let args = parse([
            "--flash-attn",
            "on",
            "--fit",
            "--threads=4",
            "-t",
            "-1",
            "--port=9",
            "-m",
            "x.gguf",
        ])
        .unwrap();
        assert_eq!(
            args.unknown,
            ["--flash-attn on", "--fit", "--threads=4", "-t -1"]
        );
        assert_eq!(args.port, 9);
        assert_eq!(args.model, Some(PathBuf::from("x.gguf")));
    }

    #[test]
    fn a_bad_or_missing_value_is_refused() {
        assert_eq!(
            parse(["--port", "abc"]).unwrap_err().to_string(),
            "error: invalid argument: --port abc"
        );
        assert!(parse(["-m"]).is_err());
        assert!(parse(["-c", "big"]).is_err());
        // -np -1 : automatique.
        assert_eq!(parse(["-np", "-1"]).unwrap().parallel, None);
    }

    #[test]
    fn keys_come_like_the_real_server() {
        // Relevé sur b12110 : LLAMA_API_KEY et --api-key s'ajoutent, coupés aux
        // virgules (fichier de clés : essai du binaire).
        let env = |name: &str| (name == "LLAMA_API_KEY").then(|| "env1,env2".to_owned());
        let args = parse_with_env(["--api-key", "cli,,cli2", "-m", "x.gguf"], env).unwrap();
        assert_eq!(args.api_keys, ["env1", "env2", "cli", "cli2"]);
        assert!(args.unknown.is_empty());
        // Sans environnement (parse) : aucune clé.
        assert!(parse(["-m", "x.gguf"]).unwrap().api_keys.is_empty());
        // Fichier absent : refus, message du vrai (par l'option ou la variable).
        let missing = "/nonexistent/fake-llama/keys.txt";
        let expected = format!(
            "error while handling argument \"--api-key-file\": error: failed to open file '{missing}'"
        );
        assert_eq!(
            parse(["--api-key-file", missing]).unwrap_err().to_string(),
            expected
        );
        let by_env = |name: &str| (name == "LLAMA_ARG_API_KEY_FILE").then(|| missing.to_owned());
        assert_eq!(
            parse_with_env(Vec::<String>::new(), by_env)
                .unwrap_err()
                .to_string(),
            expected
        );
    }
}
