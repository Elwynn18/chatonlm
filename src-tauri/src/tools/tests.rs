//! Essais des outils de la Transcription : signature OpenPGP (vraie
//! signature de yt-dlp du 26 oct. 2026, clés d'essai du faux GitHub),
//! relevé des versions et des sommes, extraction assainie, installation,
//! et le gestionnaire complet contre le faux GitHub de `fake-services`.
//! `tools_contract_latest_releases` interroge les vraies Releases (réseau,
//! `check.sh --full`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Cursor, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use fake_services::github::{self as fake, FakeGithub, Fault, SignOptions};

use super::install::{self, Probe};
use super::openpgp::{self, PublicKey, SigError};
use super::release::{self, Release, Sources};
use super::{Platform, ToolErrorCode, ToolId, ToolPhase, Tools, ToolsStatus};

const SUMS: &[u8] = include_bytes!("fixtures/SHA2-256SUMS-2026.10.26");
const SUMS_SIG: &[u8] = include_bytes!("fixtures/SHA2-256SUMS-2026.10.26.sig");
const DENO_SUM: &str = include_str!("fixtures/deno-linux-v2.9.8.sha256sum");
/// 14 nov. 2026, en secondes.
const NOW: u64 = 1_794_614_400;

fn test_key() -> PublicKey {
    PublicKey::parse(fake::test_public_key().as_bytes()).unwrap()
}

// ---------------------------------------------------------------- OpenPGP

#[test]
fn the_real_ytdlp_signature_is_accepted_with_the_pinned_key() {
    let key = openpgp::pinned_ytdlp();
    assert_eq!(key.fingerprint(), openpgp::YTDLP_FINGERPRINT);
    assert_eq!(openpgp::verify_detached(&key, SUMS, SUMS_SIG, NOW), Ok(()));
}

#[test]
fn a_changed_byte_in_the_sums_or_the_signature_is_refused() {
    let key = openpgp::pinned_ytdlp();
    let mut sums = SUMS.to_vec();
    // Une somme de l'archive Linux changée d'un chiffre.
    let at = sums.iter().position(|byte| *byte == b'f').unwrap();
    sums[at] = b'e';
    assert_eq!(
        openpgp::verify_detached(&key, &sums, SUMS_SIG, NOW),
        Err(SigError::Bad)
    );
    let mut signature = SUMS_SIG.to_vec();
    let last = signature.len() - 1;
    signature[last] ^= 0x01;
    assert_eq!(
        openpgp::verify_detached(&key, SUMS, &signature, NOW),
        Err(SigError::Bad)
    );
    // Tronquée : illisible.
    assert!(matches!(
        openpgp::verify_detached(&key, SUMS, &SUMS_SIG[..300], NOW),
        Err(SigError::Malformed(_))
    ));
}

#[test]
fn a_signature_by_another_key_is_refused() {
    let data = b"0000  yt-dlp_linux.zip\n";
    let signed = fake::sign(data);
    // Clé d'essai : acceptée ; clé épinglée de yt-dlp : autre clé.
    assert_eq!(
        openpgp::verify_detached(&test_key(), data, &signed, NOW),
        Ok(())
    );
    assert_eq!(
        openpgp::verify_detached(&openpgp::pinned_ytdlp(), data, &signed, NOW),
        Err(SigError::OtherKey)
    );
    // La vraie signature de yt-dlp, vérifiée avec la clé d'essai.
    assert_eq!(
        openpgp::verify_detached(&test_key(), SUMS, SUMS_SIG, NOW),
        Err(SigError::OtherKey)
    );
    // Une autre clé d'essai qui se dit l'émettrice : refusée aussi.
    assert_eq!(
        openpgp::verify_detached(&test_key(), data, &fake::sign_with_other_key(data), NOW),
        Err(SigError::OtherKey)
    );
}

#[test]
fn armored_keys_and_signatures_are_read_and_their_checksum_checked() {
    let data = b"sums\n";
    let armored = fake::armor("PGP SIGNATURE", &fake::sign(data));
    assert_eq!(
        openpgp::verify_detached(&test_key(), data, armored.as_bytes(), NOW),
        Ok(())
    );
    assert_eq!(test_key().fingerprint(), fake::test_fingerprint());
    // Somme CRC-24 de l'armure changée.
    let broken = armored.replace("\n=", "\n=A");
    assert!(matches!(
        openpgp::verify_detached(&test_key(), data, broken.as_bytes(), NOW),
        Err(SigError::Malformed(_))
    ));
    assert!(PublicKey::parse(b"not a key").is_err());
}

#[test]
fn expired_text_or_critical_unknown_signatures_are_refused() {
    let data = b"sums\n";
    let key = test_key();
    let expired = fake::sign_with_options(
        data,
        SignOptions {
            expires_after: Some(3600),
            ..SignOptions::default()
        },
    );
    assert_eq!(
        openpgp::verify_detached(&key, data, &expired, NOW),
        Err(SigError::Expired)
    );
    // Avant son expiration (création + 30 min) : valide.
    assert_eq!(
        openpgp::verify_detached(&key, data, &expired, 1_700_001_800),
        Ok(())
    );
    for options in [
        SignOptions {
            critical_unknown: true,
            ..SignOptions::default()
        },
        SignOptions {
            text: true,
            ..SignOptions::default()
        },
    ] {
        let signed = fake::sign_with_options(data, options);
        assert!(matches!(
            openpgp::verify_detached(&key, data, &signed, NOW),
            Err(SigError::Unsupported(_))
        ));
    }
}

// ----------------------------------------------------- Versions et sommes

#[test]
fn the_tag_is_read_from_the_redirection_and_checked() {
    assert_eq!(
        release::tag_from_location("https://github.com/yt-dlp/yt-dlp/releases/tag/2026.10.26")
            .as_deref(),
        Some("2026.10.26")
    );
    assert_eq!(
        release::tag_from_location("/denoland/deno/releases/tag/v2.9.8?x=1").as_deref(),
        Some("v2.9.8")
    );
    for refused in [
        "https://github.com/yt-dlp/yt-dlp/releases",
        "https://github.com/yt-dlp/yt-dlp/releases/tag/",
        "https://github.com/yt-dlp/yt-dlp/releases/tag/..",
        "https://github.com/yt-dlp/yt-dlp/releases/tag/a%2F..%2Fb",
        "https://github.com/yt-dlp/yt-dlp/releases/tag/.hidden",
    ] {
        assert_eq!(release::tag_from_location(refused), None, "{refused}");
    }
    assert!(!release::valid_tag(&"1".repeat(65)));
    assert!(!release::valid_tag("a/b") && !release::valid_tag("a\\b"));
}

#[test]
fn versions_are_compared_by_their_numbers() {
    assert!(release::newer("2026.10.26", "2026.10.21"));
    assert!(release::newer("2026.10.26.1", "2026.10.26"));
    assert!(release::newer("2026.11.2", "2026.10.26"));
    assert!(!release::newer("2026.10.26", "2026.10.26"));
    assert!(!release::newer("2026.10.21", "2026.10.26"));
    assert!(release::newer("v2.10.0", "v2.9.8"));
    assert!(!release::newer("nightly", "2026.10.26"));
    assert!(release::usable_deno("2.9.8") && release::usable_deno("v2.3"));
    assert!(!release::usable_deno("2.2.9"));
    assert!(!release::usable_deno("v3.0.0"));
    assert!(!release::usable_deno("1.46.3"));
    assert_eq!(release::version_of(ToolId::Deno, "v2.9.8"), "2.9.8");
    assert_eq!(
        release::version_of(ToolId::YtDlp, "2026.10.26"),
        "2026.10.26"
    );
}

#[test]
fn sums_are_read_in_both_formats_by_exact_name() {
    let sums = String::from_utf8_lossy(SUMS);
    assert_eq!(
        release::sum_for(&sums, "yt-dlp_linux.zip").as_deref(),
        Some("f34053415136e55073dd64ba5326d161ecc23ce0e98c081d8752867b37809450")
    );
    assert_eq!(
        release::sum_for(&sums, "yt-dlp_win.zip").as_deref(),
        Some("fd51b212cf8380c7a52ee08c0d6be7f3c59b2f2c832eaf7b45acfae6eab57419")
    );
    // Le binaire seul n'est pas l'archive.
    assert_eq!(
        release::sum_for(&sums, "yt-dlp_linux").as_deref(),
        Some("767ba9dce7052aa196f8b12652f433d76de128ba168966fa84b6394d5e6c6795")
    );
    assert_eq!(release::sum_for(&sums, "yt-dlp_missing.zip"), None);
    assert_eq!(
        release::sum_for(DENO_SUM, "deno-x86_64-unknown-linux-gnu.zip").as_deref(),
        Some("e890975bc2549807bbcc0b584ffafda72cc7c93349dd99c80483dd784f6c4609")
    );
    let powershell = "\r\nAlgorithm : SHA256\r\nHash      : CBA84B26AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAEDD93BF\r\n\
                      Path      : D:\\a\\deno\\deno\\target\\release\\deno-x86_64-pc-windows-msvc.zip\r\n";
    assert_eq!(
        release::sum_for(powershell, "deno-x86_64-pc-windows-msvc.zip").as_deref(),
        Some("cba84b26aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaedd93bf")
    );
    // Autre fichier, autre algorithme : rien.
    assert_eq!(release::sum_for(powershell, "deno-other.zip"), None);
    assert_eq!(
        release::sum_for(
            &powershell.replace("SHA256", "SHA512"),
            "deno-x86_64-pc-windows-msvc.zip"
        ),
        None
    );
}

// --------------------------------------------------------------- Archives

/// Archive zip de `entries` (nom, contenu ; `None` : dossier ; mode Unix).
fn zip_of(entries: &[(&str, Option<&[u8]>, u32)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content, mode) in entries {
        let options = zip::write::SimpleFileOptions::default().unix_permissions(*mode);
        match content {
            Some(bytes) => {
                zip.start_file(*name, options).unwrap();
                zip.write_all(bytes).unwrap();
            }
            None => zip.add_directory(*name, options).unwrap(),
        }
    }
    zip.finish().unwrap().into_inner()
}

fn extract(bytes: &[u8]) -> (tempfile::TempDir, std::io::Result<u64>) {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("a.zip");
    std::fs::write(&archive, bytes).unwrap();
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).unwrap();
    let result = install::extract_tree(&archive, &dest, &AtomicBool::new(false));
    (dir, result)
}

#[test]
fn unsafe_paths_are_refused_the_same_way_on_both_systems() {
    for good in [
        "yt-dlp_linux",
        "_internal/a.so",
        "_internal/sub/",
        "a\\b.txt",
        "x.y.z",
    ] {
        assert!(install::safe_relative(good).is_some(), "{good}");
    }
    for bad in [
        "",
        "/etc/passwd",
        "\\evil",
        "../evil",
        "a/../../evil",
        "a/./b",
        "a//b",
        "C:/Windows/x",
        "c:evil",
        "file.txt:stream",
        "_internal/aux.txt",
        "NUL",
        "com1.dll",
        "LPT9",
        "trailing.",
        "trailing ",
        "a<b",
        "tab\tname",
    ] {
        assert_eq!(install::safe_relative(bad), None, "{bad:?}");
    }
    assert!(install::safe_relative(&"d/".repeat(40)).is_none());
}

#[test]
fn an_archive_keeps_its_tree_and_executable_bits() {
    let bytes = zip_of(&[
        ("_internal/", None, 0o755),
        ("_internal/lib.so", Some(b"\x7fELF"), 0o755),
        (
            "_internal/THIRD_PARTY_LICENSES.txt",
            Some(b"licenses"),
            0o644,
        ),
        ("yt-dlp_linux", Some(b"#!/bin/sh\n"), 0o755),
    ]);
    let (dir, result) = extract(&bytes);
    assert_eq!(result.unwrap(), 4 + 8 + 10);
    let out = dir.path().join("out");
    assert_eq!(
        std::fs::read(out.join("_internal/THIRD_PARTY_LICENSES.txt")).unwrap(),
        b"licenses"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &str| {
            std::fs::metadata(out.join(path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode("yt-dlp_linux"), 0o755);
        assert_eq!(mode("_internal/lib.so"), 0o755);
        assert_eq!(mode("_internal/THIRD_PARTY_LICENSES.txt"), 0o644);
    }
}

#[test]
fn hostile_archives_are_refused_without_writing_outside() {
    let binaries = fake::Binaries::default();
    for fault in [Fault::ZipSlip, Fault::Symlink, Fault::Truncated] {
        let bytes = fake::archive(
            fake::Tool::YtDlp,
            "1",
            "yt-dlp_linux.zip",
            &binaries,
            Some(fault),
        );
        let (dir, result) = extract(&bytes);
        assert!(result.is_err(), "{fault:?}");
        assert!(!dir.path().join("evil.txt").exists());
    }
    // Doublon à la casse près (NTFS), nom réservé de Windows.
    for entries in [
        vec![
            ("a.txt", Some(&b"1"[..]), 0o644),
            ("A.TXT", Some(&b"2"[..]), 0o644),
        ],
        vec![("_internal/con.txt", Some(&b"1"[..]), 0o644)],
    ] {
        let (_dir, result) = extract(&zip_of(&entries));
        assert!(result.is_err());
    }
}

// ------------------------------------------------------------ Installation

#[test]
fn version_outputs_are_parsed() {
    assert_eq!(
        install::parse_version(ToolId::YtDlp, "\n2026.10.26\n").as_deref(),
        Some("2026.10.26")
    );
    assert_eq!(
        install::parse_version(
            ToolId::Deno,
            "deno 2.9.8 (stable, release, x86_64-unknown-linux-gnu)\nv8 14.7\ntypescript 5.9.2\n"
        )
        .as_deref(),
        Some("2.9.8")
    );
    assert_eq!(install::parse_version(ToolId::Deno, "2.9.8\n"), None);
    assert_eq!(
        install::parse_version(ToolId::YtDlp, "Usage: yt-dlp\n"),
        None
    );
}

#[test]
fn the_environment_points_deno_to_the_app_data_and_drops_python_variables() {
    let environment = install::tool_environment(Path::new("/data/tools/deno-cache"));
    let get = |key: &str| environment.get(&OsString::from(key)).cloned();
    assert_eq!(get("DENO_DIR"), Some("/data/tools/deno-cache".into()));
    assert_eq!(get("DENO_NO_UPDATE_CHECK"), Some("1".into()));
    assert_eq!(get("PYTHONUTF8"), Some("1".into()));
    assert!(get("LD_PRELOAD").is_none() && get("LD_LIBRARY_PATH").is_none() || cfg!(windows));
    assert!(
        environment
            .keys()
            .filter(|key| key.to_string_lossy().starts_with("PYTHON"))
            .all(
                |key| ["PYTHONUTF8", "PYTHONIOENCODING", "PYTHONDONTWRITEBYTECODE"]
                    .contains(&key.to_string_lossy().as_ref())
            )
    );
}

/// `--version` imité : lit les lignes `echo …` du script du faux GitHub
/// (les deux systèmes ; le vrai lancement est essayé à part, sous Unix).
fn script_probe() -> Probe {
    Arc::new(
        |_tool, binary: &Path, _env: &BTreeMap<OsString, OsString>| {
            let text = std::fs::read_to_string(binary).map_err(|error| error.to_string())?;
            Ok(text
                .lines()
                .filter_map(|line| line.strip_prefix("echo "))
                .map(|line| line.trim_matches('\''))
                .collect::<Vec<_>>()
                .join("\n"))
        },
    )
}

fn fake_release(tool: ToolId, tag: &str, platform: Platform) -> Release {
    Release {
        tool,
        tag: tag.into(),
        asset: tool.asset(platform).into(),
        url: String::new(),
        sha256: String::new(),
        size: 0,
    }
}

fn fake_archive(dir: &Path, tool: ToolId, version: &str, platform: Platform) -> std::path::PathBuf {
    let fake_tool = match tool {
        ToolId::YtDlp => fake::Tool::YtDlp,
        ToolId::Deno => fake::Tool::Deno,
    };
    let bytes = fake::archive(
        fake_tool,
        version,
        tool.asset(platform),
        &fake::Binaries::default(),
        None,
    );
    let path = dir.join(format!("{}-{version}.zip", tool.key()));
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn an_installation_switches_the_pointer_only_after_a_good_version_check() {
    let dir = tempfile::tempdir().unwrap();
    let tool_dir = dir.path().join("yt-dlp");
    let deno_dir = dir.path().join("deno-cache");
    let cancel = AtomicBool::new(false);
    let platform = Platform::Linux;
    let first = fake_archive(dir.path(), ToolId::YtDlp, "2026.10.21", platform);
    let installed = install::install_from(
        &tool_dir,
        platform,
        &fake_release(ToolId::YtDlp, "2026.10.21", platform),
        &first,
        &script_probe(),
        &deno_dir,
        &cancel,
    )
    .unwrap();
    assert_eq!(installed.version, "2026.10.21");
    assert!(!first.exists(), "archive effacée");
    assert_eq!(
        install::read_current(&tool_dir).as_deref(),
        Some("2026.10.21")
    );
    assert_eq!(
        install::installed(&tool_dir, ToolId::YtDlp, platform),
        Some(installed.clone())
    );
    assert!(
        installed
            .dir
            .join("_internal/THIRD_PARTY_LICENSES.txt")
            .is_file()
    );

    // Une archive qui répond une autre version : refusée, l'ancienne reste.
    let wrong = fake_archive(dir.path(), ToolId::YtDlp, "2026.10.26", platform);
    let failure = install::install_from(
        &tool_dir,
        platform,
        &fake_release(ToolId::YtDlp, "2026.11.01", platform),
        &wrong,
        &script_probe(),
        &deno_dir,
        &cancel,
    )
    .unwrap_err();
    assert_eq!(failure.code, ToolErrorCode::Version);
    assert_eq!(
        install::read_current(&tool_dir).as_deref(),
        Some("2026.10.21")
    );
    assert!(!tool_dir.join("2026.11.01").exists() && !tool_dir.join("2026.11.01.part").exists());

    // Un programme qui ne se lance pas (antivirus) : refusé aussi.
    let refused = fake_archive(dir.path(), ToolId::YtDlp, "2026.10.26", platform);
    let failing: Probe = Arc::new(|_, _, _| Err("access denied".to_owned()));
    let failure = install::install_from(
        &tool_dir,
        platform,
        &fake_release(ToolId::YtDlp, "2026.10.26", platform),
        &refused,
        &failing,
        &deno_dir,
        &cancel,
    )
    .unwrap_err();
    assert_eq!(failure.code, ToolErrorCode::Launch);
    assert_eq!(
        install::read_current(&tool_dir).as_deref(),
        Some("2026.10.21")
    );

    // La bonne : bascule, l'ancienne version effacée.
    let good = fake_archive(dir.path(), ToolId::YtDlp, "2026.10.26", platform);
    install::install_from(
        &tool_dir,
        platform,
        &fake_release(ToolId::YtDlp, "2026.10.26", platform),
        &good,
        &script_probe(),
        &deno_dir,
        &cancel,
    )
    .unwrap();
    assert_eq!(
        install::read_current(&tool_dir).as_deref(),
        Some("2026.10.26")
    );
    assert!(!tool_dir.join("2026.10.21").exists());

    // Retrait : plus rien.
    install::remove(&tool_dir).unwrap();
    assert!(!tool_dir.exists());
    assert_eq!(install::installed(&tool_dir, ToolId::YtDlp, platform), None);
}

#[test]
fn deno_outside_the_supported_versions_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let archive = fake_archive(dir.path(), ToolId::Deno, "2.2.0", Platform::Windows);
    let failure = install::install_from(
        &dir.path().join("deno"),
        Platform::Windows,
        &fake_release(ToolId::Deno, "v2.2.0", Platform::Windows),
        &archive,
        &script_probe(),
        &dir.path().join("deno-cache"),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert_eq!(failure.code, ToolErrorCode::Version);
}

#[test]
fn leftovers_are_cleaned_but_the_version_in_service_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let tool_dir = dir.path();
    for name in ["2026.10.21", "2026.10.26", "2026.11.01.part"] {
        std::fs::create_dir_all(tool_dir.join(name).join("_internal")).unwrap();
    }
    std::fs::write(tool_dir.join(install::CURRENT), "2026.10.26").unwrap();
    std::fs::write(tool_dir.join(install::CHECKED), "{}").unwrap();
    std::fs::write(tool_dir.join("current.tmp"), "x").unwrap();
    install::cleanup(tool_dir);
    let mut left: Vec<String> = std::fs::read_dir(tool_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["2026.10.26", "checked.json", "current"]);
    // Pointeur abîmé : rien n'est « en service », tout part.
    std::fs::write(tool_dir.join(install::CURRENT), "../etc").unwrap();
    install::cleanup(tool_dir);
    assert!(!tool_dir.join("2026.10.26").exists());
}

#[cfg(unix)]
#[test]
fn the_real_probe_runs_the_program_with_the_given_environment() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("deno");
    std::fs::write(
        &script,
        "#!/bin/sh\n[ \"$1\" = --version ] || exit 2\necho \"deno 2.9.8 ($DENO_DIR)\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let environment = install::tool_environment(Path::new("/x/deno-cache"));
    let output = (install::real_probe())(ToolId::Deno, &script, &environment).unwrap();
    assert_eq!(output.trim(), "deno 2.9.8 (/x/deno-cache)");
    // yt-dlp reçoit ses options : le script refuse, l'erreur remonte.
    let failure = (install::real_probe())(ToolId::YtDlp, &script, &environment).unwrap_err();
    assert!(failure.contains('2'), "{failure}");
}

// -------------------------------------------- Gestionnaire et faux GitHub

type Statuses = Arc<parking_lot::Mutex<Vec<ToolsStatus>>>;

fn sources_for(github: &FakeGithub) -> Sources {
    Sources {
        yt_dlp: Some(github.repo_url(fake::Tool::YtDlp)),
        deno: Some(github.repo_url(fake::Tool::Deno)),
        pubkey: Some(fake::test_public_key()),
    }
}

fn tools_for(root: &Path, github: &FakeGithub, platform: Platform) -> (Tools, Statuses) {
    let statuses: Statuses = Arc::default();
    let seen = statuses.clone();
    let tools = Tools::with(
        root.to_path_buf(),
        Some(platform),
        sources_for(github),
        script_probe(),
        move |status| seen.lock().push(status.clone()),
    );
    (tools, statuses)
}

/// Dernier état émis, dès qu'il convient (60 s au plus : extraction sur
/// ntfs3 sous `check.sh`).
async fn wait_for(statuses: &Statuses, ready: impl Fn(&ToolsStatus) -> bool) -> ToolsStatus {
    for _ in 0..3000 {
        if let Some(status) = statuses.lock().last().filter(|status| ready(status)) {
            return status.clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("état attendu jamais vu : {:?}", statuses.lock().last());
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn phase(status: &ToolsStatus, tool: ToolId) -> ToolPhase {
    status
        .tools
        .iter()
        .find(|entry| entry.tool == tool)
        .unwrap()
        .phase
}

fn settled(status: &ToolsStatus) -> bool {
    status.tools.iter().all(|tool| {
        !matches!(
            tool.phase,
            ToolPhase::Fetching
                | ToolPhase::Downloading
                | ToolPhase::Verifying
                | ToolPhase::Installing
        )
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tools_are_installed_leased_updated_and_removed() {
    let github = FakeGithub::start().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tools");
    let (tools, statuses) = tools_for(&root, &github, Platform::Linux);
    let first = tools.status(false);
    assert!(first.supported && !first.ready && !first.in_use);
    assert!(
        first
            .tools
            .iter()
            .all(|tool| tool.phase == ToolPhase::Absent)
    );

    // Licences non acceptées : refus, aucune requête.
    assert!(tools.install(false).is_err());
    assert!(github.requests().is_empty());
    tools.install(true).unwrap();
    let done = wait_for(&statuses, |status| status.ready && settled(status)).await;
    let ytdlp = &done.tools[0];
    assert_eq!(ytdlp.version.as_deref(), Some(fake::YTDLP_VERSION));
    assert_eq!(ytdlp.latest.as_deref(), Some(fake::YTDLP_VERSION));
    assert_eq!(ytdlp.phase, ToolPhase::Installed);
    assert!(
        ytdlp
            .third_party_licenses
            .as_deref()
            .is_some_and(|path| Path::new(path).is_file())
    );
    assert_eq!(done.tools[1].version.as_deref(), Some(fake::DENO_VERSION));
    assert!(done.tools.iter().all(|tool| tool.error.is_none()));
    // Somme, signature, taille (bytes=0-0), archive ; jamais l'API.
    let asked: Vec<String> = github
        .requests()
        .into_iter()
        .map(|request| request.path)
        .collect();
    assert!(asked.iter().any(|path| path.ends_with("/SHA2-256SUMS.sig")));
    assert!(
        asked
            .iter()
            .any(|path| path.ends_with("deno-x86_64-unknown-linux-gnu.zip.sha256sum"))
    );
    assert!(
        github
            .requests()
            .iter()
            .any(|request| request.range.as_deref() == Some("bytes=0-0"))
    );
    assert!(!asked.iter().any(|path| path.contains("/api/")));
    assert!(!root.join("downloads/yt-dlp-2026.10.26.zip").exists());

    // Un bail : chemins vérifiés, ni mise à jour ni retrait.
    let lease = tools.ready().await.unwrap();
    assert!(
        lease
            .paths
            .yt_dlp
            .ends_with("yt-dlp/2026.10.26/yt-dlp_linux")
    );
    assert!(lease.paths.deno.ends_with("deno/v2.9.8/deno"));
    assert!(lease.paths.no_ffmpeg.is_dir() && lease.paths.deno_dir.is_dir());
    assert_eq!(
        lease.paths.environment().get(&OsString::from("DENO_DIR")),
        Some(&lease.paths.deno_dir.clone().into_os_string())
    );
    assert!(tools.status(false).in_use);
    github.publish(fake::Tool::YtDlp, "2026.11.02");
    assert!(tools.update().is_err());
    assert!(tools.remove().await.is_err());
    // Relevé de la page pendant le bail : rien.
    std::fs::write(
        root.join("yt-dlp/checked.json"),
        r#"{"checkedAt":0,"latest":"2026.10.26"}"#,
    )
    .unwrap();
    let before = github.requests().len();
    tools.status(true);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(github.requests().len(), before);
    drop(lease);
    assert!(!tools.status(false).in_use);

    // Mise à jour demandée : la nouvelle version en service, l'ancienne
    // effacée.
    tools.update().unwrap();
    let updated = wait_for(&statuses, |status| {
        status.tools[0].version.as_deref() == Some("2026.11.02") && settled(status)
    })
    .await;
    assert_eq!(updated.tools[0].phase, ToolPhase::Installed);
    assert!(!root.join("yt-dlp/2026.10.26").exists());
    assert_eq!(
        updated.tools[1].version.as_deref(),
        Some(fake::DENO_VERSION)
    );

    // Retrait : tout part.
    tools.remove().await.unwrap();
    let removed = tools.status(false);
    assert!(
        removed
            .tools
            .iter()
            .all(|tool| tool.phase == ToolPhase::Absent)
    );
    assert!(!root.join("yt-dlp").exists() && !root.join("deno").exists());
    assert!(!root.join("downloads").exists());
    assert!(matches!(
        tools.ready().await,
        Err(crate::error::AppError::NotFound(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_due_check_shows_the_update_and_ready_installs_it_first() {
    let github = FakeGithub::start().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tools");
    let (tools, statuses) = tools_for(&root, &github, Platform::Linux);
    tools.install(true).unwrap();
    wait_for(&statuses, |status| status.ready && settled(status)).await;

    // Relevé fait à l'instant : pas dû, aucune requête.
    github.publish(fake::Tool::YtDlp, "2026.11.02");
    let before = github.requests().len();
    tools.status(true);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(github.requests().len(), before);

    // Relevé d'hier : la page voit la mise à jour, sans l'installer.
    let yesterday = now_ms() - 25 * 3600 * 1000;
    std::fs::write(
        root.join("yt-dlp/checked.json"),
        format!(r#"{{"checkedAt":{yesterday},"latest":"2026.10.26"}}"#),
    )
    .unwrap();
    let (tools, statuses) = tools_for(&root, &github, Platform::Linux);
    tools.status(true);
    let seen = wait_for(&statuses, |status| {
        phase(status, ToolId::YtDlp) == ToolPhase::UpdateAvailable
    })
    .await;
    assert_eq!(seen.tools[0].latest.as_deref(), Some("2026.11.02"));
    assert_eq!(seen.tools[0].version.as_deref(), Some(fake::YTDLP_VERSION));

    // Au moment de servir, relevé dû : installée avant le bail.
    std::fs::write(
        root.join("yt-dlp/checked.json"),
        format!(r#"{{"checkedAt":{yesterday},"latest":"2026.10.26"}}"#),
    )
    .unwrap();
    let (tools, _statuses) = tools_for(&root, &github, Platform::Linux);
    let lease = tools.ready().await.unwrap();
    assert_eq!(lease.paths.yt_dlp_version, "2026.11.02");
    drop(lease);

    // Après une erreur « périmée » : relevé immédiat ; rien de plus récent.
    assert!(!tools.refresh().await.unwrap());
    github.publish(fake::Tool::YtDlp, "2026.11.03");
    assert!(tools.refresh().await.unwrap());
    assert_eq!(
        tools.installed_paths().unwrap().yt_dlp_version,
        "2026.11.03"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn windows_archives_and_the_powershell_sum_are_installed() {
    let github = FakeGithub::start().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tools");
    let (tools, statuses) = tools_for(&root, &github, Platform::Windows);
    tools.install(true).unwrap();
    wait_for(&statuses, |status| status.ready && settled(status)).await;
    let paths = tools.installed_paths().unwrap();
    assert!(paths.yt_dlp.ends_with("yt-dlp.exe") && paths.deno.ends_with("deno.exe"));
    assert!(github.requests().iter().any(|request| {
        request
            .path
            .ends_with("deno-x86_64-pc-windows-msvc.zip.sha256sum")
    }));
}

/// Installation de `tool` avec le défaut `fault` : état final.
async fn install_with_fault(tool: fake::Tool, fault: Fault) -> ToolsStatus {
    let github = FakeGithub::start().await.unwrap();
    github.set_fault(tool, Some(fault));
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tools");
    let (tools, statuses) = tools_for(&root, &github, Platform::Linux);
    tools.install(true).unwrap();
    let index = match tool {
        fake::Tool::YtDlp => 0,
        fake::Tool::Deno => 1,
    };
    let status = wait_for(&statuses, |status| {
        settled(status)
            && status.tools[index].error.is_some()
            && status.tools[1 - index].version.is_some()
    })
    .await;
    assert!(!dir.path().join("evil.txt").exists() && !root.join("evil.txt").exists());
    assert!(status.tools[index].version.is_none());
    assert_eq!(status.tools[index].phase, ToolPhase::Failed);
    status
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn faulty_releases_install_nothing_and_say_why() {
    let cases = [
        (
            fake::Tool::YtDlp,
            Fault::BadSignature,
            ToolErrorCode::Signature,
        ),
        (fake::Tool::YtDlp, Fault::OtherKey, ToolErrorCode::Signature),
        (fake::Tool::YtDlp, Fault::BadSum, ToolErrorCode::Download),
        (fake::Tool::YtDlp, Fault::ZipSlip, ToolErrorCode::Archive),
        (fake::Tool::YtDlp, Fault::Symlink, ToolErrorCode::Archive),
        (fake::Tool::Deno, Fault::Truncated, ToolErrorCode::Archive),
        (fake::Tool::Deno, Fault::BadSum, ToolErrorCode::Download),
        (
            fake::Tool::Deno,
            Fault::MissingAsset,
            ToolErrorCode::Release,
        ),
        (fake::Tool::YtDlp, Fault::NoLatest, ToolErrorCode::Network),
    ];
    for (tool, fault, code) in cases {
        let status = install_with_fault(tool, fault).await;
        let index = usize::from(tool == fake::Tool::Deno);
        let error = status.tools[index].error.clone().unwrap();
        assert_eq!(error.code, code, "{fault:?} : {}", error.message);
        if fault == Fault::OtherKey {
            assert!(error.message.contains("unknown key"), "{}", error.message);
        }
    }
}

#[tokio::test]
async fn an_unsupported_system_cannot_install() {
    let github = FakeGithub::start().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let tools = Tools::with(
        dir.path().to_path_buf(),
        None,
        sources_for(&github),
        script_probe(),
        |_| {},
    );
    assert!(!tools.status(false).supported);
    assert!(tools.install(true).is_err());
    assert!(github.requests().is_empty());
}

/// Contrat (réseau, `check.sh --full`) : les vraies Releases de yt-dlp et
/// de Deno suivent encore ce que l'app attend (redirection de
/// `releases/latest`, noms des archives, sommes aux deux formats, signature
/// de yt-dlp par la clé épinglée, taille par `bytes=0-0`), sans tout
/// télécharger : les quatre premiers octets de chaque archive (`PK\3\4`).
#[tokio::test]
#[ignore = "réseau : vraies Releases de GitHub"]
async fn tools_contract_latest_releases() {
    let sources = Sources::default();
    for tool in ToolId::ALL {
        let tag = release::latest_tag(tool, &sources).await.unwrap();
        assert!(release::valid_tag(&tag), "{tag}");
        if tool == ToolId::Deno {
            assert!(release::usable_deno(&tag), "Deno {tag}");
        }
        for platform in [Platform::Linux, Platform::Windows] {
            let found = release::release(tool, &tag, platform, &sources)
                .await
                .unwrap_or_else(|failure| {
                    panic!("{} {tag} {platform:?} : {failure:?}", tool.key())
                });
            assert_eq!(found.sha256.len(), 64);
            assert!(
                (5_000_000..=release::MAX_ARCHIVE).contains(&found.size),
                "{} : {} octets",
                found.asset,
                found.size
            );
            crate::http::ensure_crypto_provider();
            let head = reqwest::Client::new()
                .get(&found.url)
                .header(reqwest::header::RANGE, "bytes=0-3")
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
            assert_eq!(&head[..4], b"PK\x03\x04", "{}", found.asset);
        }
    }
}
