//! Pack de ChatonLM et son installation : manifeste signé de la Release
//! (taille et SHA-256 de l'archive, signé par la clé des mises à jour comme
//! les paquets), puis dossier installé `<racine>/<build>/` (moteur, licences
//! et bibliothèques de NVIDIA à plat), assemblé dans `<build>.part/` puis
//! renommé : un dossier `<build>/` présent est complet.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use minisign_verify::{PublicKey, Signature};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::Platform;
use super::archive::{NVIDIA_LICENSE, nvidia_files, pack_files};
use super::pins::Pins;

/// Taille maximale d'un manifeste ou d'une signature lus.
pub const MAX_SMALL: usize = 64 * 1024;
/// Taille maximale annoncée pour l'archive du pack.
const MAX_PACK: u64 = 2 * 1024 * 1024 * 1024;

/// Nom de l'archive du pack de cette build (`chatonlm-cuda-b12110-linux-x64.tar.gz`).
pub fn pack_name(platform: Platform, build: &str) -> String {
    format!("chatonlm-cuda-{build}-{}", platform.pack_suffix())
}

/// Nom du manifeste de cette archive (`….json`).
pub fn manifest_name(platform: Platform, build: &str) -> String {
    format!("chatonlm-cuda-{build}-{}.json", platform.short())
}

/// Manifeste d'un pack (écrit par `scripts/cuda-pack-manifest.sh`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    pub build: String,
    pub version: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
}

/// Clé publique des mises à jour (`plugins.updater.pubkey` de
/// `tauri.conf.json`), en base64 comme dans ce fichier.
pub fn production_pubkey() -> Option<String> {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../../tauri.conf.json")).ok()?;
    config["plugins"]["updater"]["pubkey"]
        .as_str()
        .map(str::to_owned)
}

fn decode_base64(text: &str) -> Result<String, String> {
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|error| error.to_string())?;
    String::from_utf8(raw).map_err(|error| error.to_string())
}

/// Version sans « v » initial.
fn bare(version: &str) -> &str {
    version.trim().trim_start_matches('v')
}

/// Vérifie la signature minisign de `bytes` (fichier `.sig` de `tauri signer
/// sign`, en base64), puis son commentaire de confiance : fichier signé
/// `file` et version de l'app `version` (une signature d'une autre Release
/// ou d'un autre fichier est refusée).
pub fn verify_signature(
    bytes: &[u8],
    signature: &str,
    pubkey: &str,
    file: &str,
    version: &str,
) -> Result<(), String> {
    let key = PublicKey::decode(&decode_base64(pubkey)?).map_err(|error| error.to_string())?;
    let signature =
        Signature::decode(&decode_base64(signature)?).map_err(|error| error.to_string())?;
    key.verify(bytes, &signature, true)
        .map_err(|error| error.to_string())?;
    let field = |name: &str| {
        signature
            .trusted_comment()
            .split('\t')
            .find_map(|field| field.strip_prefix(name))
            .map(str::to_owned)
    };
    if field("file:").as_deref() != Some(file) {
        // i18n-ignore : détail technique, dans un message traduit.
        return Err(format!(
            "signed for another file ({})",
            field("file:").unwrap_or_default()
        ));
    }
    match field("version:") {
        Some(signed) if bare(&signed) == bare(version) => Ok(()),
        // i18n-ignore : détail technique, dans un message traduit.
        signed => Err(format!(
            "signed for another version ({})",
            signed.unwrap_or_default()
        )),
    }
}

/// Lit et contrôle le manifeste : même build et même version de llama.cpp
/// que le serveur livré (épingles), nom d'archive attendu, empreinte et
/// taille plausibles.
pub fn parse_manifest(bytes: &[u8], platform: Platform, pins: &Pins) -> Result<Manifest, String> {
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let expected = pack_name(platform, &pins.llama_build);
    let problems = [
        (manifest.build != pins.llama_build).then(|| format!("build {}", manifest.build)),
        // Celle du pack de cette version de l'app : un autre serait périmé
        // aussitôt installé.
        (manifest.version != pins.pack_version).then(|| format!("version {}", manifest.version)),
        (manifest.file != expected).then(|| format!("file {}", manifest.file)),
        (manifest.sha256.len() != 64 || !manifest.sha256.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| "sha256".to_owned()),
        (manifest.size == 0 || manifest.size > MAX_PACK).then(|| format!("size {}", manifest.size)),
    ];
    match problems.into_iter().flatten().next() {
        // i18n-ignore : détail technique, dans un message traduit.
        Some(problem) => Err(format!("unexpected manifest: {problem}")),
        None => Ok(Manifest {
            sha256: manifest.sha256.to_ascii_lowercase(),
            ..manifest
        }),
    }
}

/// Fichiers d'un dossier installé complet.
pub fn installed_files(platform: Platform, major: u32) -> Vec<String> {
    let mut files: Vec<String> = pack_files(platform)
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    files.extend(nvidia_files(platform, major));
    files.push(NVIDIA_LICENSE.to_owned());
    files
}

/// Pack installé.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub dir: PathBuf,
    pub build: String,
    /// `VERSION` du pack.
    pub version: String,
    /// Taille sur le disque.
    pub bytes: u64,
}

/// Dossier d'une build sous `root`.
pub fn build_dir(root: &Path, build: &str) -> PathBuf {
    root.join(build)
}

/// Build plausible (`b` + chiffres) : seuls ces dossiers sont lus et retirés.
pub fn is_build(name: &str) -> bool {
    name.len() > 1 && name.starts_with('b') && name[1..].bytes().all(|byte| byte.is_ascii_digit())
}

/// `VERSION` du pack de la build épinglée sous `root`, s'il y en a un.
fn version_in(root: &Path, pins: &Pins) -> Option<String> {
    std::fs::read_to_string(build_dir(root, &pins.llama_build).join("VERSION"))
        .ok()
        .map(|text| text.trim().to_owned())
}

/// Pack de la build épinglée sous `root`, complet et de la `VERSION`
/// attendue ([`Pins::pack_version`]), sinon `None`.
pub fn installed(root: &Path, pins: &Pins, platform: Platform) -> Option<Installed> {
    let build = pins.llama_build.as_str();
    let dir = build_dir(root, build);
    let version = version_in(root, pins).filter(|version| *version == pins.pack_version)?;
    let mut bytes = 0u64;
    for file in installed_files(platform, pins.cuda_major) {
        let meta = std::fs::metadata(dir.join(&file)).ok()?;
        if !meta.is_file() {
            return None;
        }
        bytes = bytes.saturating_add(meta.len());
    }
    Some(Installed {
        dir,
        build: build.to_owned(),
        version,
        bytes,
    })
}

/// Pack de la build épinglée d'une autre `VERSION` (pack refait : correctif,
/// épingles de NVIDIA) : périmé, à retélécharger.
pub fn outdated(root: &Path, pins: &Pins) -> bool {
    version_in(root, pins).is_some_and(|version| version != pins.pack_version)
}

/// Autres builds présentes sous `root` (packs périmés), triées.
pub fn stale_builds(root: &Path, current: &str) -> Vec<String> {
    let mut builds: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|name| is_build(name) && name != current)
        .collect();
    builds.sort();
    builds
}

/// SHA-256 d'un fichier, en hexadécimal.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Ligne de `sha256sum` : empreinte, une espace, puis « ␠ » (mode texte)
/// ou « * » (mode binaire, celui du `sha256sum` de Git pour Windows) et le
/// nom ; même règle que `parseSums` (scripts/inspect-packages.mjs).
fn sum_line(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_end_matches('\r');
    let (sum, rest) = line.trim_start().split_once(' ')?;
    let name = rest.strip_prefix([' ', '*'])?.trim();
    (sum.len() == 64 && !name.is_empty()).then_some((sum, name))
}

/// Contrôle `SHA256SUMS` du pack extrait dans `dir` : il couvre exactement
/// les fichiers du pack (hors lui-même), chacun intact.
pub fn check_sums(dir: &Path, platform: Platform) -> Result<(), String> {
    let text =
        std::fs::read_to_string(dir.join("SHA256SUMS")).map_err(|error| error.to_string())?;
    let mut listed = HashSet::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let (sum, name) = sum_line(line)
            // i18n-ignore : détail technique, dans un message traduit.
            .ok_or_else(|| format!("SHA256SUMS: {line}"))?;
        if !pack_files(platform).contains(&name) || name == "SHA256SUMS" {
            // i18n-ignore : détail technique, dans un message traduit.
            return Err(format!("SHA256SUMS: unexpected {name}"));
        }
        if sha256_file(&dir.join(name)).map_err(|error| error.to_string())?
            != sum.trim().to_ascii_lowercase()
        {
            // i18n-ignore : détail technique, dans un message traduit.
            return Err(format!("SHA256SUMS: {name} differs"));
        }
        listed.insert(name.to_owned());
    }
    let missing: Vec<&str> = pack_files(platform)
        .into_iter()
        .filter(|name| *name != "SHA256SUMS" && !listed.contains(*name))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        // i18n-ignore : détail technique, dans un message traduit.
        Err(format!("SHA256SUMS: missing {}", missing.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PUBKEY: &str = include_str!("fixtures/test.key.pub");
    const MANIFEST: &[u8] = include_bytes!("fixtures/chatonlm-cuda-b12110-linux-x64.json");
    const MANIFEST_SIG: &str = include_str!("fixtures/chatonlm-cuda-b12110-linux-x64.json.sig");

    fn pins() -> Pins {
        super::super::tests::fixture_pins(Platform::Linux)
    }

    #[test]
    fn names_of_the_pack() {
        assert_eq!(
            pack_name(Platform::Linux, "b12110"),
            "chatonlm-cuda-b12110-linux-x64.tar.gz"
        );
        assert_eq!(
            pack_name(Platform::Windows, "b12110"),
            "chatonlm-cuda-b12110-windows-x64.zip"
        );
        assert_eq!(
            manifest_name(Platform::Windows, "b1"),
            "chatonlm-cuda-b1-windows-x64.json"
        );
        assert!(
            is_build("b12110")
                && !is_build("b")
                && !is_build("b12110.part")
                && !is_build("downloads")
        );
    }

    #[test]
    fn the_signed_manifest_is_checked() {
        let file = "chatonlm-cuda-b12110-linux-x64.json";
        assert_eq!(
            verify_signature(MANIFEST, MANIFEST_SIG, TEST_PUBKEY, file, "9.9.9"),
            Ok(())
        );
        assert_eq!(
            verify_signature(MANIFEST, MANIFEST_SIG, TEST_PUBKEY, file, "v9.9.9"),
            Ok(())
        );
        // Autre version de l'app, autre fichier, contenu changé, autre clé.
        assert!(verify_signature(MANIFEST, MANIFEST_SIG, TEST_PUBKEY, file, "9.9.10").is_err());
        assert!(
            verify_signature(MANIFEST, MANIFEST_SIG, TEST_PUBKEY, "other.json", "9.9.9").is_err()
        );
        let mut tampered = MANIFEST.to_vec();
        tampered[10] ^= 1;
        assert!(verify_signature(&tampered, MANIFEST_SIG, TEST_PUBKEY, file, "9.9.9").is_err());
        let production = production_pubkey().unwrap();
        assert!(verify_signature(MANIFEST, MANIFEST_SIG, &production, file, "9.9.9").is_err());
        assert!(verify_signature(MANIFEST, "not base64!", TEST_PUBKEY, file, "9.9.9").is_err());
        // Le manifeste signé est celui de la build épinglée.
        let manifest = parse_manifest(MANIFEST, Platform::Linux, &pins()).unwrap();
        assert_eq!(manifest.file, "chatonlm-cuda-b12110-linux-x64.tar.gz");
    }

    #[test]
    fn a_manifest_of_another_build_is_refused() {
        let pins = pins();
        let good = serde_json::json!({
            "build": pins.llama_build,
            "version": format!("{}+chatonlm.cuda.0123456789ab", pins.llama_version),
            "file": pack_name(Platform::Linux, &pins.llama_build),
            "size": 123,
            "sha256": "AB".repeat(32),
        });
        let parsed = parse_manifest(good.to_string().as_bytes(), Platform::Linux, &pins).unwrap();
        assert_eq!(parsed.sha256, "ab".repeat(32));
        for (key, value) in [
            ("build", serde_json::json!("b1")),
            ("version", serde_json::json!("v0.0.1+chatonlm.cuda.x")),
            ("file", serde_json::json!("../evil.tar.gz")),
            ("size", serde_json::json!(0)),
            ("sha256", serde_json::json!("zz")),
        ] {
            let mut bad = good.clone();
            bad[key] = value;
            assert!(
                parse_manifest(bad.to_string().as_bytes(), Platform::Linux, &pins).is_err(),
                "{key}"
            );
        }
        // Le pack de Linux n'est pas celui de Windows.
        assert!(parse_manifest(good.to_string().as_bytes(), Platform::Windows, &pins).is_err());
    }

    /// Les deux formes de `sha256sum` : texte (« ␠␠ ») et binaire
    /// (« ␠* », Git pour Windows), fin de ligne CRLF comprise.
    #[test]
    fn sums_are_read_in_text_and_binary_modes() {
        let sum = "ab".repeat(32);
        assert_eq!(
            sum_line(&format!("{sum}  ggml-cuda.dll")),
            Some((sum.as_str(), "ggml-cuda.dll"))
        );
        assert_eq!(
            sum_line(&format!("{sum} *ggml-cuda.dll\r")),
            Some((sum.as_str(), "ggml-cuda.dll"))
        );
        assert_eq!(sum_line(&format!("{sum} ggml-cuda.dll")), None);
        assert_eq!(sum_line("abc  ggml-cuda.dll"), None);
        assert_eq!(sum_line(&format!("{sum}  ")), None);
    }

    /// Pack Windows dont le `SHA256SUMS` vient du `sha256sum` de Git pour
    /// Windows (mode binaire, « * ») : accepté.
    #[test]
    fn a_windows_pack_summed_in_binary_mode_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        for name in pack_files(Platform::Windows) {
            std::fs::write(dir.path().join(name), name.as_bytes()).unwrap();
        }
        let sums: String = pack_files(Platform::Windows)
            .into_iter()
            .filter(|name| *name != "SHA256SUMS")
            .map(|name| {
                format!(
                    "{} *{name}\r\n",
                    sha256_file(&dir.path().join(name)).unwrap()
                )
            })
            .collect();
        std::fs::write(dir.path().join("SHA256SUMS"), sums).unwrap();
        assert_eq!(check_sums(dir.path(), Platform::Windows), Ok(()));
    }

    #[test]
    fn an_installed_pack_is_complete_and_its_sums_match() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let build = build_dir(root, "b12110");
        std::fs::create_dir_all(&build).unwrap();
        let files = installed_files(Platform::Linux, 13);
        for file in &files {
            std::fs::write(build.join(file), file.as_bytes()).unwrap();
        }
        std::fs::write(build.join("VERSION"), format!("{}\n", pins().pack_version)).unwrap();
        let sums: String = pack_files(Platform::Linux)
            .into_iter()
            .filter(|name| *name != "SHA256SUMS")
            .map(|name| format!("{}  {name}\n", sha256_file(&build.join(name)).unwrap()))
            .collect();
        std::fs::write(build.join("SHA256SUMS"), &sums).unwrap();
        assert_eq!(check_sums(&build, Platform::Linux), Ok(()));
        let found = installed(root, &pins(), Platform::Linux).unwrap();
        assert!(!outdated(root, &pins()));
        assert_eq!(found.version, pins().pack_version);
        // Même build, pack refait (autre empreinte) : périmé, pas installé.
        let mut newer = pins();
        newer.pack_version = "v0.9.0+chatonlm.cuda.ffffffffffff".into();
        assert!(installed(root, &newer, Platform::Linux).is_none());
        assert!(outdated(root, &newer));
        assert!(found.bytes > 0);
        // Autre build : périmée ; dossiers étrangers ignorés.
        std::fs::create_dir_all(root.join("b11000")).unwrap();
        std::fs::create_dir_all(root.join("downloads")).unwrap();
        std::fs::create_dir_all(root.join("b12110.part")).unwrap();
        assert_eq!(stale_builds(root, "b12110"), vec!["b11000".to_owned()]);
        let mut other = pins();
        other.llama_build = "b11000".into();
        assert!(installed(root, &other, Platform::Linux).is_none());
        // Fichier changé, ou manquant.
        std::fs::write(build.join("LICENSE-CCCL.txt"), "changed").unwrap();
        assert!(
            check_sums(&build, Platform::Linux)
                .unwrap_err()
                .contains("differs")
        );
        std::fs::write(
            build.join("SHA256SUMS"),
            sums.lines()
                .filter(|line| !line.contains("CCCL"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        assert!(
            check_sums(&build, Platform::Linux)
                .unwrap_err()
                .contains("missing")
        );
        std::fs::remove_file(build.join("libcublasLt.so.13")).unwrap();
        assert!(installed(root, &pins(), Platform::Linux).is_none());
    }
}
