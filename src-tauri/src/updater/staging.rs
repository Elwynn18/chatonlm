//! Mise à jour téléchargée, gardée sur disque (`<données>/updates/`) :
//! le paquet et sa description (`update.json` : version, nouveautés,
//! signature, type de paquet qui l'a téléchargée). Une relance de ChatonLM la
//! retrouve prête sans nouveau téléchargement, après avoir revérifié sa
//! signature, le nom de fichier signé et son format ; un seul paquet à la
//! fois (ceux des autres versions sont supprimés). Le .deb et l'AppImage
//! partagent ce dossier (même identifiant) : une mise à jour n'est reprise
//! que par le type de paquet qui l'a téléchargée.
//!
//! Un échec de l'installation une fois les services arrêtés (ChatonLM est
//! alors relancé) est noté dans `install-failed.json`, lu au démarrage.

use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};

use super::UpdateInstaller;
use super::install;
use crate::i18n::t;

/// Description de la mise à jour gardée.
const MANIFEST: &str = "update.json";
/// Échec de la dernière installation (lu puis supprimé au démarrage).
const FAILURE: &str = "install-failed.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Staged {
    pub version: String,
    pub notes: Option<String>,
    /// Contenu du `.sig` publié (base64), comme dans `latest.json`.
    pub signature: String,
    /// Nom du paquet dans le dossier.
    pub file: String,
    /// Type de paquet qui l'a téléchargée (seul à pouvoir l'installer).
    pub installer: UpdateInstaller,
}

impl Staged {
    pub fn new(
        version: &str,
        notes: Option<String>,
        signature: &str,
        installer: UpdateInstaller,
    ) -> Self {
        // Version déjà validée (semver) par le greffon : pas de « / ».
        let safe: String = version
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || ".-+".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Self {
            version: version.to_owned(),
            notes,
            signature: signature.to_owned(),
            file: format!("chatonlm-{safe}.update"),
            installer,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Failure {
    pub version: String,
    pub detail: String,
}

/// Échec de la relecture de la mise à jour gardée.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReadError {
    /// Fichier absent ou illisible (supprimé, disque) : rien à voir avec la
    /// signature.
    Io(String),
    /// Signature fausse, ou paquet qui n'est pas celui attendu.
    Invalid(String),
}

/// Écrit le paquet puis sa description (fichiers temporaires renommés : une
/// coupure ne laisse jamais une description sans son paquet entier), puis
/// supprime tout le reste du dossier.
pub(crate) fn save(dir: &Path, staged: &Staged, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    write_atomically(&dir.join(&staged.file), bytes)?;
    let manifest = serde_json::to_vec_pretty(staged).map_err(std::io::Error::other)?;
    write_atomically(&dir.join(MANIFEST), &manifest)?;
    remove_others(dir, &[staged.file.as_str(), MANIFEST]);
    Ok(())
}

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let part = path.with_extension("part");
    let mut file = std::fs::File::create(&part)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&part, path)
}

/// Description de la mise à jour gardée, si elle est lisible.
pub(crate) fn load(dir: &Path) -> Option<Staged> {
    let text = std::fs::read(dir.join(MANIFEST)).ok()?;
    let staged: Staged = serde_json::from_slice(&text).ok()?;
    // Jamais un chemin hors du dossier.
    let plain = Path::new(&staged.file)
        .file_name()
        .is_some_and(|name| name == staged.file.as_str());
    plain.then_some(staged)
}

/// Paquet gardé, relu puis vérifié (signature de la clé publique, version
/// signée égale à celle attendue, nom de fichier signé et format du type de
/// paquet qui l'a téléchargé).
pub(crate) fn read_verified(
    dir: &Path,
    staged: &Staged,
    pubkey: &str,
    require_signed_version: bool,
) -> Result<Vec<u8>, ReadError> {
    let bytes =
        std::fs::read(dir.join(&staged.file)).map_err(|error| ReadError::Io(error.to_string()))?;
    verify(
        &bytes,
        &staged.signature,
        pubkey,
        &staged.version,
        require_signed_version,
        staged.installer,
    )
    .map_err(ReadError::Invalid)?;
    Ok(bytes)
}

/// Supprime tout le contenu du dossier (mise à jour installée, dépassée ou
/// refusée).
pub(crate) fn clear(dir: &Path) {
    remove_others(dir, &[]);
}

/// Supprime la mise à jour `staged` (paquet et description), seulement si
/// le dossier la décrit encore : une autre, téléchargée entre-temps, reste.
pub(crate) fn discard(dir: &Path, staged: &Staged) {
    if load(dir).as_ref() == Some(staged) {
        let _ = std::fs::remove_file(dir.join(MANIFEST));
        let _ = std::fs::remove_file(dir.join(&staged.file));
    }
}

fn remove_others(dir: &Path, keep: &[&str]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if keep.iter().any(|kept| name == *kept) {
            continue;
        }
        let path = entry.path();
        let removed = if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(error) = removed {
            eprintln!("chatonlm: cannot remove {}: {error}", path.display());
        }
    }
}

/// Note l'échec d'une installation, pour le montrer après la relance.
pub(crate) fn save_failure(dir: &Path, failure: &Failure) {
    let written = serde_json::to_vec(failure)
        .map_err(std::io::Error::other)
        .and_then(|bytes| {
            std::fs::create_dir_all(dir)?;
            write_atomically(&dir.join(FAILURE), &bytes)
        });
    if let Err(error) = written {
        eprintln!("chatonlm: cannot record the failed update: {error}");
    }
}

/// Échec noté par la session précédente, lu une fois (fichier supprimé).
pub(crate) fn take_failure(dir: &Path) -> Option<Failure> {
    let path: PathBuf = dir.join(FAILURE);
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    serde_json::from_slice(&bytes).ok()
}

/// Même vérification que tauri-plugin-updater après un téléchargement
/// (`verify_signature`) : clé et signature en base64 (fichiers minisign),
/// signature globale (commentaire de confiance compris), puis version liée
/// à la signature (`version:` du commentaire de confiance), exigée si
/// `require_signed_version`. En plus : nom de fichier signé (`file:`) du
/// type de paquet attendu (`.AppImage`, `.deb`, `-setup.exe` de
/// l'installateur de Windows), et format du paquet
/// ([`install::has_format`]) : l'annonce (`latest.json`) n'est pas signée,
/// et une signature valide d'un .deb ne doit jamais remplacer l'AppImage.
pub(crate) fn verify(
    bytes: &[u8],
    signature: &str,
    pubkey: &str,
    version: &str,
    require_signed_version: bool,
    installer: UpdateInstaller,
) -> Result<(), String> {
    let decode = |text: &str| -> Result<String, String> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .map_err(|error| error.to_string())?;
        String::from_utf8(raw).map_err(|error| error.to_string())
    };
    let key = PublicKey::decode(&decode(pubkey)?).map_err(|error| error.to_string())?;
    let signature = Signature::decode(&decode(signature)?).map_err(|error| error.to_string())?;
    key.verify(bytes, &signature, true)
        .map_err(|error| error.to_string())?;
    let field = |name: &str| {
        signature
            .trusted_comment()
            .split('\t')
            .find_map(|field| field.strip_prefix(name))
            .map(str::to_owned)
    };
    match field("version:") {
        Some(signed) if same_version(&signed, version) => {}
        Some(signed) => {
            return Err(t!(
                "updates.signedForOtherVersion",
                signed = signed,
                version = version
            ));
        }
        None if require_signed_version => return Err(t!("updates.unsignedVersion")),
        None => {}
    }
    let extension = match installer {
        UpdateInstaller::AppImage => ".AppImage",
        UpdateInstaller::Deb => ".deb",
        UpdateInstaller::Nsis => "-setup.exe",
    };
    let file = field("file:").unwrap_or_default();
    if !file.ends_with(extension) {
        return Err(t!("updates.signedForOtherFile", file = file));
    }
    if !install::has_format(bytes, installer) {
        return Err(match installer {
            UpdateInstaller::AppImage => t!("updates.notAppImage"),
            UpdateInstaller::Deb => t!("updates.notDeb"),
            UpdateInstaller::Nsis => t!("updates.windows.notInstaller"),
        });
    }
    Ok(())
}

/// Comparaison du greffon : en semver (« v1.2.3 » = « 1.2.3 »), sinon
/// littérale.
fn same_version(a: &str, b: &str) -> bool {
    match (
        semver::Version::parse(a.trim_start_matches('v')),
        semver::Version::parse(b.trim_start_matches('v')),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Clé publique d'essai (`scripts/updater-keys.sh --test`, clé privée
    /// hors du dépôt) et signatures, pour la version 9.9.9
    /// (`tauri signer sign --app-version 9.9.9`) : de [`test_payload`]
    /// (`payload.bin`), de [`test_appimage`] (`chatonlm_9.9.9_amd64.AppImage`,
    /// deux fois : horodatages différents), de [`test_deb`]
    /// (`chatonlm_9.9.9_amd64.deb`), et de [`test_appimage`] signée sous le
    /// nom du .deb.
    pub const TEST_PUBKEY: &str = include_str!("fixtures/test.key.pub");
    pub const TEST_SIGNATURE: &str = include_str!("fixtures/payload-9.9.9.sig");
    pub const APPIMAGE_SIGNATURE: &str = include_str!("fixtures/appimage-9.9.9.sig");
    pub const APPIMAGE_RESIGNED: &str = include_str!("fixtures/appimage-9.9.9-resigned.sig");
    pub const DEB_SIGNATURE: &str = include_str!("fixtures/deb-9.9.9.sig");
    pub const APPIMAGE_AS_DEB_SIGNATURE: &str = include_str!("fixtures/appimage-as-deb-9.9.9.sig");
    /// De `install::test_nsis` (`chatonlm_9.9.9_x64-setup.exe`).
    pub const NSIS_SIGNATURE: &str = include_str!("fixtures/nsis-9.9.9.sig");

    fn pattern(len: u32, seed: u32) -> Vec<u8> {
        (0..len).map(|i| ((i * 31 + seed) % 256) as u8).collect()
    }

    /// Contenu quelconque (200 000 octets).
    pub fn test_payload() -> Vec<u8> {
        pattern(200_000, 7)
    }

    /// AppImage d'essai (64 Kio) : en-tête ELF 64 bits avec la marque
    /// « AI\x02 », une seule entrée dans la table des sections, juste après
    /// l'en-tête, puis l'image squashfs (« hsqs ») à l'octet 128. Mêmes
    /// octets que `testAppImage()` de scripts/release-manifest.test.mjs.
    pub fn test_appimage() -> Vec<u8> {
        let mut bytes = pattern(65_536, 7);
        bytes[..11].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1, 0, b'A', b'I', 2]);
        bytes[0x28..0x30].copy_from_slice(&64u64.to_le_bytes());
        bytes[0x3a..0x3c].copy_from_slice(&64u16.to_le_bytes());
        bytes[0x3c..0x3e].copy_from_slice(&1u16.to_le_bytes());
        bytes[64 + 0x18..64 + 0x28].fill(0);
        bytes[128..132].copy_from_slice(b"hsqs");
        bytes
    }

    /// Paquet .deb d'essai (64 Kio) : archive ar dont le premier membre est
    /// `debian-binary`. Mêmes octets que `testDeb()` du même fichier.
    pub fn test_deb() -> Vec<u8> {
        let mut bytes = pattern(65_536, 11);
        let head = b"!<arch>\ndebian-binary   ";
        bytes[..head.len()].copy_from_slice(head);
        bytes
    }

    const APPIMAGE: UpdateInstaller = UpdateInstaller::AppImage;
    const DEB: UpdateInstaller = UpdateInstaller::Deb;

    #[test]
    fn the_signed_package_is_accepted_and_any_change_is_refused() {
        let image = test_appimage();
        let check = |bytes: &[u8], signature, key, version, require| {
            verify(bytes, signature, key, version, require, APPIMAGE)
        };
        assert_eq!(
            check(&image, APPIMAGE_SIGNATURE, TEST_PUBKEY, "9.9.9", true),
            Ok(())
        );
        assert_eq!(
            check(&image, APPIMAGE_SIGNATURE, TEST_PUBKEY, "v9.9.9", true),
            Ok(())
        );
        assert_eq!(
            check(&image, APPIMAGE_RESIGNED, TEST_PUBKEY, "9.9.9", true),
            Ok(())
        );
        let mut tampered = image.clone();
        tampered[1000] ^= 1;
        assert!(check(&tampered, APPIMAGE_SIGNATURE, TEST_PUBKEY, "9.9.9", false).is_err());
        // Autre version annoncée que celle signée : refusée même sans
        // `require_signed_version` (comme le greffon), texte de ChatonLM.
        let error = check(&image, APPIMAGE_SIGNATURE, TEST_PUBKEY, "9.9.10", false).unwrap_err();
        assert_eq!(
            error,
            t!(
                "updates.signedForOtherVersion",
                signed = "9.9.9",
                version = "9.9.10"
            )
        );
        assert!(
            error.contains("9.9.9") && error.contains("9.9.10"),
            "{error}"
        );
        // Clé de production : signature d'une autre clé.
        let production = include_str!("fixtures/production.key.pub");
        assert!(check(&image, APPIMAGE_SIGNATURE, production, "9.9.9", false).is_err());
        assert!(check(&image, "not base64!", TEST_PUBKEY, "9.9.9", false).is_err());
    }

    #[test]
    fn the_signed_file_name_and_the_format_must_be_those_of_the_installer() {
        // Le .deb signé, pour un .deb ; jamais pour l'AppImage (nom signé).
        assert_eq!(
            verify(&test_deb(), DEB_SIGNATURE, TEST_PUBKEY, "9.9.9", true, DEB),
            Ok(())
        );
        assert_eq!(
            verify(
                &test_deb(),
                DEB_SIGNATURE,
                TEST_PUBKEY,
                "9.9.9",
                true,
                APPIMAGE
            ),
            Err(t!(
                "updates.signedForOtherFile",
                file = "chatonlm_9.9.9_amd64.deb"
            ))
        );
        // Des octets d'AppImage signés sous le nom d'un .deb : refusés pour
        // l'AppImage (nom signé) comme pour le .deb (format).
        let image = test_appimage();
        assert_eq!(
            verify(
                &image,
                APPIMAGE_AS_DEB_SIGNATURE,
                TEST_PUBKEY,
                "9.9.9",
                true,
                APPIMAGE
            ),
            Err(t!(
                "updates.signedForOtherFile",
                file = "chatonlm_9.9.9_amd64.deb"
            ))
        );
        assert_eq!(
            verify(
                &image,
                APPIMAGE_AS_DEB_SIGNATURE,
                TEST_PUBKEY,
                "9.9.9",
                true,
                DEB
            ),
            Err(t!("updates.notDeb"))
        );
        // Signature d'un fichier quelconque (`payload.bin`).
        assert!(
            verify(
                &test_payload(),
                TEST_SIGNATURE,
                TEST_PUBKEY,
                "9.9.9",
                true,
                APPIMAGE
            )
            .is_err()
        );
    }

    #[test]
    fn a_windows_installer_is_only_accepted_as_a_windows_installer() {
        let nsis = install::test_nsis();
        let check = |bytes: &[u8], signature, installer| {
            verify(bytes, signature, TEST_PUBKEY, "9.9.9", true, installer)
        };
        assert_eq!(check(&nsis, NSIS_SIGNATURE, UpdateInstaller::Nsis), Ok(()));
        // Jamais pour l'AppImage ni le .deb (nom de fichier signé).
        for installer in [APPIMAGE, DEB] {
            assert_eq!(
                check(&nsis, NSIS_SIGNATURE, installer),
                Err(t!(
                    "updates.signedForOtherFile",
                    file = "chatonlm_9.9.9_x64-setup.exe"
                ))
            );
        }
        // Paquets de Linux annoncés à Windows : refusés (nom signé).
        assert_eq!(
            check(&test_appimage(), APPIMAGE_SIGNATURE, UpdateInstaller::Nsis),
            Err(t!(
                "updates.signedForOtherFile",
                file = "chatonlm_9.9.9_amd64.AppImage"
            ))
        );
        assert!(check(&test_deb(), DEB_SIGNATURE, UpdateInstaller::Nsis).is_err());
        // Installateur modifié : signature fausse.
        let mut tampered = nsis.clone();
        tampered[2000] ^= 1;
        assert!(check(&tampered, NSIS_SIGNATURE, UpdateInstaller::Nsis).is_err());
    }

    #[test]
    fn appimage_and_deb_formats_are_recognised() {
        assert!(install::has_format(&test_appimage(), APPIMAGE));
        assert!(!install::has_format(&test_appimage(), DEB));
        assert!(install::has_format(&test_deb(), DEB));
        assert!(!install::has_format(&test_deb(), APPIMAGE));
        // Sans image squashfs, sans la marque AppImage, ELF 32 bits,
        // tronquée : pas une AppImage.
        for (at, value) in [(128, b'x'), (9, b'X'), (4, 1)] {
            let mut image = test_appimage();
            image[at] = value;
            assert!(!install::has_format(&image, APPIMAGE), "octet {at}");
        }
        assert!(!install::has_format(&test_appimage()[..130], APPIMAGE));
        assert!(!install::has_format(b"", APPIMAGE));
        assert!(!install::has_format(b"!<arch>\n", DEB));
        assert!(!install::has_format(b"!<arch>\ncontrol.tar.gz", DEB));
    }

    #[test]
    fn the_production_key_is_the_configured_one() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let updater = &config["plugins"]["updater"];
        assert_eq!(
            updater["pubkey"].as_str().unwrap(),
            include_str!("fixtures/production.key.pub").trim()
        );
        assert_eq!(updater["requireSignedVersion"], true);
        assert_eq!(
            updater["endpoints"],
            serde_json::json!([
                "https://github.com/Elwynn18/chatonlm/releases/latest/download/latest.json"
            ])
        );
        assert_eq!(config["bundle"]["createUpdaterArtifacts"], true);
    }

    #[test]
    fn a_saved_update_is_found_again_and_others_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let updates = dir.path().join("updates");
        std::fs::create_dir_all(updates.join("old")).unwrap();
        std::fs::write(updates.join("chatonlm-9.9.8.update"), b"old").unwrap();
        std::fs::write(updates.join("chatonlm-9.9.9.part"), b"partial").unwrap();

        let staged = Staged::new("9.9.9", Some("Notes".into()), APPIMAGE_SIGNATURE, APPIMAGE);
        assert_eq!(staged.file, "chatonlm-9.9.9.update");
        save(&updates, &staged, &test_appimage()).unwrap();
        let mut names: Vec<_> = std::fs::read_dir(&updates)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["chatonlm-9.9.9.update", "update.json"]);

        let loaded = load(&updates).unwrap();
        assert_eq!(loaded, staged);
        assert_eq!(loaded.installer, APPIMAGE);
        assert_eq!(
            read_verified(&updates, &loaded, TEST_PUBKEY, true).unwrap(),
            test_appimage()
        );
        // Paquet abîmé sur le disque : refusé (signature) ; absent : erreur
        // de lecture, distincte.
        std::fs::write(updates.join(&staged.file), b"tampered").unwrap();
        assert!(matches!(
            read_verified(&updates, &loaded, TEST_PUBKEY, true),
            Err(ReadError::Invalid(_))
        ));
        std::fs::remove_file(updates.join(&staged.file)).unwrap();
        assert!(matches!(
            read_verified(&updates, &loaded, TEST_PUBKEY, true),
            Err(ReadError::Io(_))
        ));

        clear(&updates);
        assert_eq!(std::fs::read_dir(&updates).unwrap().count(), 0);
        assert_eq!(load(&updates), None);
    }

    #[test]
    fn discard_only_removes_the_update_still_described() {
        let dir = tempfile::tempdir().unwrap();
        let first = Staged::new("9.9.9", None, APPIMAGE_SIGNATURE, APPIMAGE);
        let second = Staged::new("9.9.9", None, APPIMAGE_RESIGNED, APPIMAGE);
        save(dir.path(), &second, &test_appimage()).unwrap();
        discard(dir.path(), &first);
        assert_eq!(load(dir.path()), Some(second.clone()));
        discard(dir.path(), &second);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_description_never_points_outside_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let staged = Staged {
            file: "../escape.update".into(),
            ..Staged::new("9.9.9", None, TEST_SIGNATURE, APPIMAGE)
        };
        std::fs::write(
            dir.path().join(MANIFEST),
            serde_json::to_vec(&staged).unwrap(),
        )
        .unwrap();
        assert_eq!(load(dir.path()), None);
        assert_eq!(
            Staged::new("1.0.0/../../x", None, "", APPIMAGE).file,
            "chatonlm-1.0.0_.._.._x.update"
        );
        // Description sans type de paquet (ancienne) : ignorée.
        std::fs::write(
            dir.path().join(MANIFEST),
            r#"{"version":"9.9.9","notes":null,"signature":"x","file":"chatonlm-9.9.9.update"}"#,
        )
        .unwrap();
        assert_eq!(load(dir.path()), None);
    }

    #[test]
    fn a_failed_installation_is_read_once() {
        let dir = tempfile::tempdir().unwrap();
        let failure = Failure {
            version: "9.9.9".into(),
            detail: "Authentication failed or was cancelled".into(),
        };
        save_failure(dir.path(), &failure);
        assert_eq!(take_failure(dir.path()), Some(failure));
        assert_eq!(take_failure(dir.path()), None);
    }
}
