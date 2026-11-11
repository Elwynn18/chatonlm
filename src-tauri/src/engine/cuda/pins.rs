//! Épingles du pack CUDA, lues à la compilation dans `packaging/cuda.env`
//! (toolkit, pilote, archives de NVIDIA) et `packaging/llama.env` (version et
//! build de llama.cpp du serveur livré, auxquels le pack doit correspondre).

use std::collections::HashMap;
use std::sync::OnceLock;

use super::Platform;

const CUDA_ENV: &str = include_str!("../../../../packaging/cuda.env");
const LLAMA_ENV: &str = include_str!("../../../../packaging/llama.env");

/// Empreinte du pack CUDA de cette version de l'app (src-tauri/build.rs,
/// même formule que scripts/build-llama-cuda.sh).
pub const PACK_FINGERPRINT: &str = env!("CHATONLM_CUDA_PACK_FINGERPRINT");

/// Archive de NVIDIA : chemin sous [`Pins::redist_url`], taille, SHA-256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redist {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

impl Redist {
    /// Nom du fichier (dernier segment du chemin).
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// Épingles pour une plateforme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pins {
    /// Version majeure de CUDA (13) : noms des bibliothèques.
    pub cuda_major: u32,
    /// Pilote minimal (version majeure : 580).
    pub driver_min: u32,
    /// Capacité de calcul minimale (7.5).
    pub compute_min: (u32, u32),
    /// Adresse des archives de NVIDIA (avec `/` final).
    pub redist_url: String,
    pub cudart: Redist,
    pub cublas: Redist,
    /// Licence de NVIDIA (EULA du CUDA Toolkit), montrée avant.
    pub eula_url: String,
    /// Version de llama.cpp du serveur livré (`v0.9.0`).
    pub llama_version: String,
    /// Build de llama.cpp (`b12110`) : nom du pack et de son dossier.
    pub llama_build: String,
    /// `VERSION` du pack de cette version de l'app
    /// (`<llama_version>+chatonlm.cuda.<empreinte>`) : un pack installé
    /// d'une autre `VERSION` (même build, correctifs ou épingles changés)
    /// est périmé.
    pub pack_version: String,
}

/// Épingles de cette plateforme (`None` hors Linux et Windows).
pub fn current() -> Option<&'static Pins> {
    static PINS: OnceLock<Option<Pins>> = OnceLock::new();
    PINS.get_or_init(|| {
        Platform::current().and_then(|platform| parse(CUDA_ENV, LLAMA_ENV, platform).ok())
    })
    .as_ref()
}

fn variables(text: &str) -> HashMap<&str, &str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .collect()
}

/// Épingles de `platform` d'après les textes de `cuda.env` et `llama.env`.
pub fn parse(cuda_env: &str, llama_env: &str, platform: Platform) -> Result<Pins, String> {
    let cuda = variables(cuda_env);
    let llama = variables(llama_env);
    let get = |vars: &HashMap<&str, &str>, key: &str| -> Result<String, String> {
        vars.get(key)
            .filter(|value| !value.is_empty())
            .map(|value| (*value).to_owned())
            // i18n-ignore : épingles de la construction.
            .ok_or_else(|| format!("{key} missing"))
    };
    let number = |key: &str| -> Result<u64, String> {
        get(&cuda, key)?
            .parse()
            // i18n-ignore : épingles de la construction.
            .map_err(|_| format!("{key} is not a number"))
    };
    let system = match platform {
        Platform::Linux => "LINUX",
        Platform::Windows => "WINDOWS",
    };
    let redist = |name: &str| -> Result<Redist, String> {
        let sha256 = get(&cuda, &format!("CUDA_{name}_{system}_SHA256"))?;
        if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            // i18n-ignore : épingles de la construction.
            return Err(format!("CUDA_{name}_{system}_SHA256 is not a SHA-256"));
        }
        let path = get(&cuda, &format!("CUDA_{name}_{system}_PATH"))?;
        if path.starts_with('/') || path.split('/').any(|part| part == "..") {
            // i18n-ignore : épingles de la construction.
            return Err(format!("CUDA_{name}_{system}_PATH is not relative"));
        }
        Ok(Redist {
            path,
            size: number(&format!("CUDA_{name}_{system}_SIZE"))?,
            sha256: sha256.to_ascii_lowercase(),
        })
    };
    let compute = get(&cuda, "CUDA_COMPUTE_MIN")?;
    let compute_min = super::detect::parse_compute(&compute)
        // i18n-ignore : épingles de la construction.
        .ok_or_else(|| format!("CUDA_COMPUTE_MIN {compute}"))?;
    let mut redist_url = get(&cuda, "CUDA_REDIST_URL")?;
    if !redist_url.ends_with('/') {
        redist_url.push('/');
    }
    Ok(Pins {
        cuda_major: u32::try_from(number("CUDA_MAJOR")?).map_err(|error| error.to_string())?,
        driver_min: u32::try_from(number("CUDA_DRIVER_MIN")?).map_err(|error| error.to_string())?,
        compute_min,
        redist_url,
        cudart: redist("CUDART")?,
        cublas: redist("CUBLAS")?,
        eula_url: get(&cuda, "CUDA_EULA_URL")?,
        pack_version: format!(
            "{}+chatonlm.cuda.{PACK_FINGERPRINT}",
            get(&llama, "LLAMA_VERSION")?
        ),
        llama_version: get(&llama, "LLAMA_VERSION")?,
        llama_build: get(&llama, "LLAMA_BUILD")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_files_are_read_for_both_systems() {
        for platform in [Platform::Linux, Platform::Windows] {
            let pins = parse(CUDA_ENV, LLAMA_ENV, platform).unwrap();
            assert_eq!(pins.cuda_major, 13);
            assert_eq!(pins.driver_min, 580);
            assert_eq!(pins.compute_min, (7, 5));
            assert!(
                pins.redist_url
                    .starts_with("https://developer.download.nvidia.com/")
            );
            assert!(pins.redist_url.ends_with('/'));
            assert!(pins.llama_build.starts_with('b'));
            assert!(pins.llama_version.starts_with('v'));
            let extension = match platform {
                Platform::Linux => ".tar.xz",
                Platform::Windows => ".zip",
            };
            for redist in [&pins.cudart, &pins.cublas] {
                assert!(redist.path.ends_with(extension), "{}", redist.path);
                assert!(redist.size > 0);
                assert_eq!(redist.sha256.len(), 64);
            }
            assert!(pins.cudart.file_name().starts_with("cuda_cudart-"));
            assert!(pins.cublas.file_name().starts_with("libcublas-"));
        }
    }

    /// La `VERSION` attendue est celle que le script du pack écrit (même
    /// empreinte : build.rs et scripts/build-llama-cuda.sh ne divergent pas).
    #[cfg(unix)]
    #[test]
    fn the_expected_pack_version_is_the_one_the_script_writes() {
        let pins = parse(CUDA_ENV, LLAMA_ENV, Platform::Linux).unwrap();
        assert_eq!(PACK_FINGERPRINT.len(), 12);
        assert!(
            PACK_FINGERPRINT
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
        let script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/build-llama-cuda.sh");
        let output = std::process::Command::new("bash")
            .arg(script)
            .arg("--version")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            pins.pack_version
        );
    }

    #[test]
    fn a_broken_pin_is_refused() {
        let broken = CUDA_ENV.replace("CUDA_CUBLAS_LINUX_SHA256=", "CUDA_CUBLAS_LINUX_SHA256=zz");
        assert!(parse(&broken, LLAMA_ENV, Platform::Linux).is_err());
        // L'autre système n'en dépend pas.
        assert!(parse(&broken, LLAMA_ENV, Platform::Windows).is_ok());
        let escaping = CUDA_ENV.replace(
            "CUDA_CUDART_WINDOWS_PATH=cuda_cudart",
            "CUDA_CUDART_WINDOWS_PATH=../cuda_cudart",
        );
        assert!(parse(&escaping, LLAMA_ENV, Platform::Windows).is_err());
        let no_build = LLAMA_ENV.replace("LLAMA_BUILD=", "LLAMA_OTHER=");
        assert!(parse(CUDA_ENV, &no_build, Platform::Linux).is_err());
    }
}
