//! Installation d'un outil depuis son archive vérifiée : extraction de
//! l'arborescence assainie dans `<outil>/<étiquette>.part/`, droits
//! d'exécution (Linux), `--version` lancé une fois (marque
//! [`VERIFIED`]), renommage en `<outil>/<étiquette>/`, puis bascule du
//! pointeur [`CURRENT`] ; anciennes versions effacées ensuite.
//!
//! Sous Windows, un fichier ouvert (antivirus qui inspecte un `.exe` tout
//! juste écrit, programme encore lancé) ne se renomme ni ne s'efface :
//! renommages refaits patiemment (comme le pack CUDA), et ce qui n'a pas pu
//! être effacé l'est au lancement suivant ([`cleanup`] : tout ce qui n'est
//! pas la version en service). Le pointeur est un petit fichier remplacé
//! par renommage (atomique sur les deux systèmes).

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::release::{self, Failure, Release};
use super::{Platform, ToolErrorCode, ToolId};
use crate::os_util::NoWindow;

/// Pointeur de la version en service (son étiquette).
pub const CURRENT: &str = "current";
/// Dernier relevé de la dernière version.
pub const CHECKED: &str = "checked.json";
/// Marque d'une version extraite et lancée avec succès.
pub const VERIFIED: &str = ".chatonlm-verified";

/// Entrées au plus dans une archive (yt-dlp : 175 sous Linux).
const MAX_ENTRIES: usize = 20_000;
/// Octets décompressés au plus (yt-dlp : 99 Mo, Deno : 96 Mo).
const MAX_TOTAL: u64 = 1024 * 1024 * 1024;
/// Profondeur et longueur d'un nom au plus.
const MAX_DEPTH: usize = 32;
const MAX_NAME: usize = 255;
/// Délai de `--version` (premier lancement sous Windows : l'antivirus
/// inspecte le programme et ses bibliothèques).
const VERSION_TIMEOUT: Duration = Duration::from_secs(120);

/// Nouveaux essais d'un renommage ou d'un effacement refusé un instant
/// (Windows : fichier ouvert par l'antivirus).
const BUSY_TRIES: u32 = 10;
const BUSY_PAUSE: Duration = Duration::from_millis(500);

fn patiently<T>(op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    crate::engine::cuda::manager::retry_busy(cfg!(windows), BUSY_TRIES, BUSY_PAUSE, op)
}

/// Une version installée et vérifiée.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub tag: String,
    /// Version rendue par `--version`.
    pub version: String,
    pub dir: PathBuf,
    pub binary: PathBuf,
    /// Taille extraite.
    pub bytes: u64,
}

/// Contenu de la marque [`VERIFIED`].
#[derive(Debug, Serialize, Deserialize)]
struct Marker {
    version: String,
    bytes: u64,
}

/// Dernier relevé de la dernière version ([`CHECKED`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checked {
    /// ms depuis l'époque Unix.
    pub checked_at: i64,
    pub latest: String,
}

pub fn read_checked(tool_dir: &Path) -> Option<Checked> {
    let text = std::fs::read_to_string(tool_dir.join(CHECKED)).ok()?;
    serde_json::from_str::<Checked>(&text)
        .ok()
        .filter(|checked| release::valid_tag(&checked.latest))
}

pub fn write_checked(tool_dir: &Path, checked: &Checked) -> io::Result<()> {
    std::fs::create_dir_all(tool_dir)?;
    std::fs::write(
        tool_dir.join(CHECKED),
        serde_json::to_vec(checked).map_err(io::Error::other)?,
    )
}

/// Étiquette de la version en service.
pub fn read_current(tool_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(tool_dir.join(CURRENT)).ok()?;
    let tag = text.trim();
    release::valid_tag(tag).then(|| tag.to_owned())
}

/// Pointe sur `tag` : fichier temporaire puis renommage.
fn write_current(tool_dir: &Path, tag: &str) -> io::Result<()> {
    let temporary = tool_dir.join(format!("{CURRENT}.tmp"));
    {
        let mut file = File::create(&temporary)?;
        file.write_all(tag.as_bytes())?;
        file.sync_all()?;
    }
    patiently(|| std::fs::rename(&temporary, tool_dir.join(CURRENT)))
}

/// Version en service, si elle est complète : pointeur, dossier, marque et
/// programme présents.
pub fn installed(tool_dir: &Path, tool: ToolId, platform: Platform) -> Option<Installed> {
    let tag = read_current(tool_dir)?;
    let dir = tool_dir.join(&tag);
    let marker: Marker = serde_json::from_slice(&std::fs::read(dir.join(VERIFIED)).ok()?).ok()?;
    let binary = dir.join(tool.binary(platform));
    if !crate::process::is_executable(&binary) {
        return None;
    }
    Some(Installed {
        tag,
        version: marker.version,
        dir,
        binary,
        bytes: marker.bytes,
    })
}

/// Nom réservé de Windows (`CON`, `NUL`, `COM1`…, extension comprise).
fn reserved(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).trim_end_matches(' ');
    let upper = stem.to_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    upper
        .strip_prefix("COM")
        .or_else(|| upper.strip_prefix("LPT"))
        .is_some_and(|rest| {
            let mut chars = rest.chars();
            matches!(
                (chars.next(), chars.next()),
                (Some(digit), None) if digit.is_ascii_digit() || matches!(digit, '¹' | '²' | '³')
            )
        })
}

/// Chemin relatif sûr d'une entrée d'archive (`\` lu comme `/`), sinon
/// `None` : chemin absolu ou de lecteur, `..`, `.`, composant vide, `:`
/// (flux de données de NTFS), caractères interdits par Windows ou de
/// contrôle, point ou espace final, nom réservé, trop long ou trop
/// profond. Logique pure : la même règle sous Linux et sous Windows.
pub fn safe_relative(name: &str) -> Option<PathBuf> {
    let name = name.replace('\\', "/");
    if name.starts_with('/') {
        return None;
    }
    let trimmed = name.strip_suffix('/').unwrap_or(&name);
    if trimmed.is_empty() {
        return None;
    }
    let mut path = PathBuf::new();
    let mut depth = 0;
    for part in trimmed.split('/') {
        depth += 1;
        let refused = part.is_empty()
            || part == "."
            || part == ".."
            || part.len() > MAX_NAME
            || part.ends_with(['.', ' '])
            || part
                .chars()
                .any(|c| c.is_control() || matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*'))
            || reserved(part);
        if refused || depth > MAX_DEPTH {
            return None;
        }
        path.push(part);
    }
    Some(path)
}

fn invalid(detail: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, detail.into())
}

/// Extrait toute l'archive zip `archive` dans `dest` (qui existe et est
/// vide) en gardant l'arborescence : tout chemin refusé par
/// [`safe_relative`], lien, entrée chiffrée, doublon (sans tenir compte de
/// la casse, comme NTFS), dépassement de [`MAX_ENTRIES`] ou de
/// [`MAX_TOTAL`] fait échouer l'extraction entière. Droits : exécutable si
/// l'archive le dit (Linux). Rend les octets écrits.
pub fn extract_tree(archive: &Path, dest: &Path, cancel: &AtomicBool) -> io::Result<u64> {
    let mut zip =
        zip::ZipArchive::new(BufReader::new(File::open(archive)?)).map_err(invalid_zip)?;
    if zip.len() > MAX_ENTRIES {
        // i18n-ignore : détail technique, dans un message traduit.
        return Err(invalid(format!("{} entries", zip.len())));
    }
    let mut files = HashSet::new();
    let mut total = 0u64;
    let mut buffer = vec![0u8; 1 << 20];
    for index in 0..zip.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::engine::cuda::archive::cancelled());
        }
        let mut entry = zip.by_index(index).map_err(invalid_zip)?;
        let name = entry.name().map_err(invalid_zip)?.into_owned();
        // i18n-ignore : détail technique, dans un message traduit.
        let relative =
            safe_relative(&name).ok_or_else(|| invalid(format!("unsafe path: {name}")))?;
        if entry.is_symlink() {
            // i18n-ignore : détail technique, dans un message traduit.
            return Err(invalid(format!("link: {name}")));
        }
        if entry.encrypted() {
            // i18n-ignore : détail technique, dans un message traduit.
            return Err(invalid(format!("encrypted: {name}")));
        }
        let target = dest.join(&relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        let key = relative.to_string_lossy().to_lowercase();
        if !files.insert(key) {
            // i18n-ignore : détail technique, dans un message traduit.
            return Err(invalid(format!("duplicate entry: {name}")));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let executable = entry.unix_mode().is_some_and(|mode| mode & 0o111 != 0);
        let mut file = File::options().write(true).create_new(true).open(&target)?;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(crate::engine::cuda::archive::cancelled());
            }
            let read = match entry.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(invalid(error.to_string())),
            };
            total += read as u64;
            if total > MAX_TOTAL {
                // i18n-ignore : détail technique, dans un message traduit.
                return Err(invalid("archive too large once extracted"));
            }
            file.write_all(&buffer[..read])?;
        }
        drop(file);
        super::os::set_executable(&target, executable)?;
    }
    Ok(total)
}

fn invalid_zip(error: zip::result::ZipError) -> io::Error {
    invalid(error.to_string())
}

/// Environnement de yt-dlp et de Deno : celui des programmes du bureau
/// (Linux : variables de l'AppImage, `LD_LIBRARY_PATH` et `LD_PRELOAD`
/// retirés, sinon les bibliothèques de `_internal/` ne se chargent pas),
/// sans les variables de Python ni celles de Deno qui changeraient leur
/// comportement, avec `DENO_DIR` sous les données de l'app (jamais
/// `~/.cache/deno`), sans vérification de mise à jour de Deno, sortie UTF-8
/// (Windows : sinon la page de code de la console) et sans couleurs.
pub fn tool_environment(deno_dir: &Path) -> BTreeMap<OsString, OsString> {
    let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    if cfg!(unix) {
        let (appdir, nvidia_workaround) = crate::process::desktop_changes();
        crate::process::scrub(&mut environment, appdir.as_deref(), nvidia_workaround);
    }
    environment.retain(|key, _| {
        let key = key.to_string_lossy().to_ascii_uppercase();
        !(key.starts_with("PYTHON") || key.starts_with("DENO_") || key.starts_with("NODE_"))
    });
    for (key, value) in [
        ("DENO_NO_UPDATE_CHECK", "1"),
        ("DENO_NO_PROMPT", "1"),
        ("NO_COLOR", "1"),
        ("PYTHONUTF8", "1"),
        ("PYTHONIOENCODING", "utf-8"),
        ("PYTHONDONTWRITEBYTECODE", "1"),
    ] {
        environment.insert(key.into(), value.into());
    }
    environment.insert("DENO_DIR".into(), deno_dir.as_os_str().to_owned());
    environment
}

/// Arguments de `--version`.
pub fn version_args(tool: ToolId) -> &'static [&'static str] {
    match tool {
        ToolId::YtDlp => &["--ignore-config", "--no-update", "--version"],
        ToolId::Deno => &["--version"],
    }
}

/// Version lue dans la sortie de `--version` : première ligne pour yt-dlp
/// (`2026.10.26`), `deno 2.9.8 (…)` pour Deno.
pub fn parse_version(tool: ToolId, output: &str) -> Option<String> {
    let first = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let version = match tool {
        ToolId::YtDlp => first,
        ToolId::Deno => first.strip_prefix("deno ")?.split_whitespace().next()?,
    };
    release::parts(version).map(|_| version.to_owned())
}

/// Lance `--version` (une fois) : programme, environnement ; rend la
/// sortie standard, ou un détail (sortie d'erreur, code).
pub type Probe = Arc<
    dyn Fn(ToolId, &Path, &BTreeMap<OsString, OsString>) -> Result<String, String> + Send + Sync,
>;

/// [`Probe`] réel : le programme, sans console (Windows), environnement
/// donné, délai borné.
pub fn real_probe() -> Probe {
    Arc::new(run_version)
}

fn run_version(
    tool: ToolId,
    binary: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<String, String> {
    let mut command = std::process::Command::new(binary);
    command
        .args(version_args(tool))
        .env_clear()
        .envs(environment)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .no_window();
    if let Some(dir) = binary.parent() {
        command.current_dir(dir);
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = Vec::new();
            if let Some(pipe) = pipe {
                let _ = pipe.take(1 << 20).read_to_end(&mut text);
            }
            String::from_utf8_lossy(&text).into_owned()
        })
    };
    let stdout = read(
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );
    let stderr = read(
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + VERSION_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                // i18n-ignore : détail technique, dans un message traduit.
                return Err("--version timed out".to_owned());
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if status.success() {
        Ok(stdout)
    } else {
        let tail: String = stderr
            .trim()
            .chars()
            .rev()
            .take(400)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        // i18n-ignore : détail technique, dans un message traduit.
        Err(format!("{status}: {tail}"))
    }
}

/// Efface un dossier, patiemment ; absent : fait.
pub fn remove_dir_patiently(dir: &Path) -> io::Result<()> {
    patiently(|| match std::fs::remove_dir_all(dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    })
}

/// Installe `archive` (somme déjà vérifiée par la file) de `release` dans
/// `tool_dir` ; `deno_dir` : `DENO_DIR` du lancement de `--version`.
/// L'archive est effacée dans tous les cas (elle a servi, ou elle est
/// mauvaise).
#[allow(clippy::too_many_arguments)]
pub fn install_from(
    tool_dir: &Path,
    platform: Platform,
    release: &Release,
    archive: &Path,
    probe: &Probe,
    deno_dir: &Path,
    cancel: &AtomicBool,
) -> Result<Installed, Failure> {
    let result = install_inner(
        tool_dir, platform, release, archive, probe, deno_dir, cancel,
    );
    let _ = patiently(|| match std::fs::remove_file(archive) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    });
    result
}

fn install_inner(
    tool_dir: &Path,
    platform: Platform,
    release: &Release,
    archive: &Path,
    probe: &Probe,
    deno_dir: &Path,
    cancel: &AtomicBool,
) -> Result<Installed, Failure> {
    let tool = release.tool;
    let io_failure = |error: io::Error| Failure::new(ToolErrorCode::Io, error.to_string());
    if !release::valid_tag(&release.tag) {
        // i18n-ignore : détail technique, dans un message traduit.
        return Err(Failure::new(ToolErrorCode::Release, "invalid tag"));
    }
    std::fs::create_dir_all(tool_dir).map_err(io_failure)?;
    let part = tool_dir.join(format!("{}.part", release.tag));
    remove_dir_patiently(&part).map_err(io_failure)?;
    std::fs::create_dir_all(&part).map_err(io_failure)?;
    let result = (|| {
        let bytes = extract_tree(archive, &part, cancel)
            .map_err(|error| Failure::new(ToolErrorCode::Archive, error.to_string()))?;
        let binary = part.join(tool.binary(platform));
        if !binary.is_file() {
            // i18n-ignore : détail technique, dans un message traduit.
            return Err(Failure::new(
                ToolErrorCode::Archive,
                format!("{} missing", tool.binary(platform)),
            ));
        }
        super::os::set_executable(&binary, true).map_err(io_failure)?;
        std::fs::create_dir_all(deno_dir).map_err(io_failure)?;
        let output = probe(tool, &binary, &tool_environment(deno_dir))
            .map_err(|detail| Failure::new(ToolErrorCode::Launch, detail))?;
        let expected = release::version_of(tool, &release.tag);
        let version = parse_version(tool, &output)
            // i18n-ignore : détail technique, dans un message traduit.
            .ok_or_else(|| Failure::new(ToolErrorCode::Version, "unreadable --version"))?;
        if version != expected {
            // i18n-ignore : détail technique, dans un message traduit.
            return Err(Failure::new(
                ToolErrorCode::Version,
                format!("{version} instead of {expected}"),
            ));
        }
        if tool == ToolId::Deno && !release::usable_deno(&version) {
            // i18n-ignore : détail technique, dans un message traduit.
            return Err(Failure::new(
                ToolErrorCode::Version,
                format!("Deno {version} (2.3 to 2.x needed)"),
            ));
        }
        if cancel.load(Ordering::Relaxed) {
            // i18n-ignore : jamais montré (annulé).
            return Err(Failure::new(ToolErrorCode::Io, "cancelled"));
        }
        let marker = serde_json::to_vec(&Marker {
            version: version.clone(),
            bytes,
        })
        .map_err(|error| Failure::new(ToolErrorCode::Io, error.to_string()))?;
        std::fs::write(part.join(VERIFIED), marker).map_err(io_failure)?;
        let target = tool_dir.join(&release.tag);
        remove_dir_patiently(&target).map_err(io_failure)?;
        patiently(|| std::fs::rename(&part, &target)).map_err(io_failure)?;
        write_current(tool_dir, &release.tag).map_err(io_failure)?;
        Ok(Installed {
            tag: release.tag.clone(),
            version,
            binary: target.join(tool.binary(platform)),
            dir: target,
            bytes,
        })
    })();
    if result.is_err() {
        let _ = remove_dir_patiently(&part);
    } else {
        cleanup(tool_dir);
    }
    result
}

/// Efface ce qui n'est pas la version en service (anciennes versions,
/// `.part` abandonnés) ; un reste (fichier ouvert sous Windows) sera
/// effacé au prochain appel. Sans pointeur, tout part.
pub fn cleanup(tool_dir: &Path) {
    let current = read_current(tool_dir);
    let Ok(entries) = std::fs::read_dir(tool_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == CURRENT || name == CHECKED || Some(&name) == current.as_ref() {
            continue;
        }
        let path = entry.path();
        let _ = if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
    }
}

/// Retire l'outil : le pointeur d'abord (l'outil n'est plus « installé »,
/// même si la suite échoue), puis tout le dossier. Un reste (Windows) est
/// effacé au lancement suivant par [`cleanup`].
pub fn remove(tool_dir: &Path) -> io::Result<()> {
    match patiently(|| std::fs::remove_file(tool_dir.join(CURRENT))) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    remove_dir_patiently(tool_dir)
}
