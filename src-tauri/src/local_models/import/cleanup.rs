//! Ménage différé de l'Ollama intégré, après confirmation (écran du lot
//! L5) : blobs restés dans son dossier (non repris, à retélécharger, ou
//! copiés vers un autre disque), manifestes (`manifests/`, `manifests-v2/`),
//! son `HOME` (`<données>/ollama/home`, clé `id_ed25519` du compte
//! ollama.com) et son fichier de processus. Seulement des fichiers à la
//! forme de ceux d'Ollama, jamais un fichier de la base locale (le dossier
//! des modèles peut être celui de l'intégré : lot L6, point 3), jamais un
//! lien suivi ; puis les dossiers devenus vides. L'Ollama de la machine
//! n'est jamais concerné.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use ts_rs::TS;

/// Ce qui reste de l'Ollama intégré.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Leftovers {
    /// Dossier des modèles de l'intégré.
    pub dir: String,
    pub files: u32,
    pub bytes: u64,
    /// Modèles de l'intégré qui disparaîtront : à retélécharger, en échec,
    /// non repris (cloud, embeddings…), ou tirés après la reprise (à dire à
    /// l'écran avant la confirmation ; rempli par `service::leftovers`).
    pub models: Vec<super::ImportCandidate>,
}

/// Bilan du ménage.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CleanupReport {
    pub removed_files: u32,
    pub removed_bytes: u64,
    /// Fichiers qui n'ont pas pu être effacés (sous Windows : ouverts).
    pub failed: u32,
}

/// Fichiers à effacer, avec leur taille.
fn plan(models: &Path, ollama_root: &Path, keep: &HashSet<PathBuf>) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let kept = |path: &Path| keep.contains(path);
    // Blobs : `sha256-<hex>` (et téléchargements inachevés `-partial…`).
    if let Ok(entries) = fs::read_dir(models.join("blobs")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if name.starts_with("sha256-")
                && entry
                    .file_type()
                    .is_ok_and(|kind| kind.is_file() || kind.is_symlink())
                && !kept(&path)
            {
                let size = entry.metadata().map_or(0, |meta| meta.len());
                out.push((path, size));
            }
        }
    }
    // Manifestes : fichiers et liens à quatre niveaux au plus, jamais un
    // GGUF (un auteur « manifests » de Hugging Face dans un dossier partagé).
    for root in ["manifests", "manifests-v2"] {
        let mut stack = vec![(models.join(root), 0usize)];
        while let Some((folder, depth)) = stack.pop() {
            let Ok(entries) = fs::read_dir(&folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                if kind.is_dir() && depth < 3 {
                    stack.push((path, depth + 1));
                } else if (kind.is_file() || kind.is_symlink())
                    && depth == 3
                    && !kept(&path)
                    && !path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
                {
                    let size = fs::symlink_metadata(&path).map_or(0, |meta| meta.len());
                    out.push((path, size));
                }
            }
        }
    }
    // HOME de l'intégré et fichier de processus (sous `<données>/ollama`).
    let mut stack = vec![ollama_root.join("home")];
    while let Some(folder) = stack.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(entry.path());
            } else {
                let size = fs::symlink_metadata(entry.path()).map_or(0, |meta| meta.len());
                out.push((entry.path(), size));
            }
        }
    }
    let pid = ollama_root.join("ollama.pid");
    if let Ok(meta) = fs::symlink_metadata(&pid) {
        out.push((pid, meta.len()));
    }
    out.sort();
    out
}

/// Ce qui reste dans `models` (dossier des modèles de l'intégré) et sous
/// `ollama_root` (`<données>/ollama`) ; `keep` : chemins absolus des
/// fichiers de la base locale.
pub fn leftovers(models: &Path, ollama_root: &Path, keep: &HashSet<PathBuf>) -> Leftovers {
    let files = plan(models, ollama_root, keep);
    Leftovers {
        dir: models.to_string_lossy().into_owned(),
        files: u32::try_from(files.len()).unwrap_or(u32::MAX),
        bytes: files.iter().map(|(_, size)| size).sum(),
        models: Vec::new(),
    }
}

/// Efface ce que [`leftovers`] compte, puis les dossiers devenus vides
/// (`blobs/`, manifestes, `home/`, le dossier des modèles s'il est vide,
/// `<données>/ollama` s'il l'est).
pub fn cleanup(models: &Path, ollama_root: &Path, keep: &HashSet<PathBuf>) -> CleanupReport {
    let mut report = CleanupReport::default();
    for (path, size) in plan(models, ollama_root, keep) {
        match fs::remove_file(&path) {
            Ok(()) => {
                report.removed_files += 1;
                report.removed_bytes += size;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => report.failed += 1,
        }
    }
    for root in [
        models.join("blobs"),
        models.join("manifests"),
        models.join("manifests-v2"),
        ollama_root.join("home"),
    ] {
        remove_empty_tree(&root);
    }
    // Échoue sans dommage si le dossier n'est pas vide.
    let _ = fs::remove_dir(models);
    let _ = fs::remove_dir(ollama_root);
    report
}

/// Retire les dossiers vides sous `root` (et `root` s'il l'est), sans
/// suivre les liens.
fn remove_empty_tree(root: &Path) {
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                remove_empty_tree(&entry.path());
            }
        }
    }
    let _ = fs::remove_dir(root);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// Dossier des modèles partagé avec l'intégré (dossier déplacé en
    /// 0.9.x) : seuls ses fichiers partent, jamais ceux de la base locale ni
    /// ceux de l'utilisateur.
    #[test]
    fn only_ollama_files_are_removed() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("ollama");
        let models = data.path().join("shared");
        let blob = models
            .join("blobs")
            .join(format!("sha256-{}", "a".repeat(64)));
        write(&blob, b"12345");
        write(
            &models
                .join("blobs")
                .join(format!("sha256-{}-partial", "b".repeat(64))),
            b"1",
        );
        write(
            &models.join("manifests/registry.ollama.ai/library/qwen3/0.6b"),
            b"{}",
        );
        write(
            &models.join("manifests-v2/ollama.com/library/qwen3/0.6b"),
            b"{}",
        );
        write(&root.join("home/.ollama/id_ed25519"), b"key");
        write(&root.join("ollama.pid"), b"1");
        // Base locale (auteur « manifests » de Hugging Face), fichier à
        // l'utilisateur, blob gardé.
        let ours = models.join("manifests/Repo-GGUF/sub/x.gguf");
        write(&ours, b"gguf");
        // Version découpée dans un sous-dossier : à la profondeur des
        // manifestes, seule son extension la protège.
        let ours_deep = models.join("manifests/Repo-GGUF/Q4_K_M/part/x-00001-of-00002.gguf");
        write(&ours_deep, b"gguf");
        let kept_blob = models
            .join("blobs")
            .join(format!("sha256-{}", "c".repeat(64)));
        write(&kept_blob, b"kept");
        let user = models.join("blobs/notes.txt");
        write(&user, b"mine");
        let installed = models.join("u/R-GGUF/R-Q4_K_M.gguf");
        write(&installed, b"model");
        let keep: HashSet<PathBuf> = [kept_blob.clone(), installed.clone()].into();
        let left = leftovers(&models, &root, &keep);
        assert_eq!(left.files, 6);
        assert_eq!(left.bytes, 5 + 1 + 2 + 2 + 3 + 1);
        let report = cleanup(&models, &root, &keep);
        assert_eq!(report.removed_files, 6);
        assert_eq!(report.failed, 0);
        for path in [&ours, &ours_deep, &kept_blob, &user, &installed] {
            assert!(path.exists(), "{path:?}");
        }
        assert!(!blob.exists());
        assert!(!models.join("manifests-v2").exists());
        assert!(!models.join("manifests/registry.ollama.ai").exists());
        assert!(!root.exists());
        assert_eq!(leftovers(&models, &root, &keep).files, 0);
    }

    #[test]
    fn the_default_folder_goes_away_when_empty() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("ollama");
        let models = root.join("models");
        write(
            &models
                .join("blobs")
                .join(format!("sha256-{}", "a".repeat(64))),
            b"1",
        );
        write(
            &models.join("manifests/registry.ollama.ai/library/a/b"),
            b"{}",
        );
        cleanup(&models, &root, &HashSet::new());
        assert!(!root.exists());
        assert!(data.path().exists());
    }

    #[cfg(unix)]
    #[test]
    fn links_are_never_followed() {
        let data = tempfile::tempdir().unwrap();
        let outside = data.path().join("outside");
        write(&outside.join("precious"), b"keep me");
        let models = data.path().join("models");
        fs::create_dir_all(models.join("manifests/registry.ollama.ai/library")).unwrap();
        std::os::unix::fs::symlink(
            &outside,
            models.join("manifests/registry.ollama.ai/library/linked"),
        )
        .unwrap();
        fs::create_dir_all(models.join("blobs")).unwrap();
        std::os::unix::fs::symlink(
            outside.join("precious"),
            models
                .join("blobs")
                .join(format!("sha256-{}", "d".repeat(64))),
        )
        .unwrap();
        cleanup(&models, &data.path().join("ollama"), &HashSet::new());
        assert_eq!(fs::read(outside.join("precious")).unwrap(), b"keep me");
    }
}
