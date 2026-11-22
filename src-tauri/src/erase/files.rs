//! « Erase my data » : fichiers de l'utilisateur sous le dossier des données
//! (fonctions pures sur un dossier, essayées dans un dossier temporaire).
//!
//! Effacés : les clés des fournisseurs distants (`remote/keys.json`), les
//! copies de la base (avant une migration, base illisible ou plus récente
//! mise de côté : elles gardent toutes les conversations), les journaux
//! (`logs/`, sauf celui de l'app, vidé par son propre fichier : voir
//! `logging::clear`), l'audio de travail de la Transcription et, de l'ancien
//! Ollama intégré (0.9.x), son dossier personnel (`ollama/home` : la clé du
//! compte ollama.com, `.ollama/id_ed25519`) et son `ollama.pid`. Gardés : les
//! modèles de l'ancien Ollama intégré (`ollama/models`, repris par
//! `local_models::import`) et les
//! modèles (texte, images, transcription, voix), les outils et le pack CUDA
//! téléchargés, le cache du ModelHub, l'icône des notifications,
//! `launcher.json` (chemin de l'app pour l'extension VS Code, sans
//! appairage), l'autorisation de coller du portail du bureau.
//!
//! Un fichier qui ne se supprime pas (Windows : ouvert par un autre programme
//! sans partage) est vidé à la place ; s'il ne se vide pas non plus, il est
//! rendu dans la liste des restes, sans arrêter les autres.

use std::path::{Path, PathBuf};

/// Nom de la base dans `<données>/db` (voir `lib.rs`).
pub const DATABASE: &str = "chatonlm.sqlite";

/// Nom du verrou des ouvertures de la base (`store::lock_openings`) : vide,
/// gardé.
const OPENING: &str = "chatonlm.sqlite.opening";

/// Copie de la base dans `db/` : `<base>.v<N>.bak` (avant une migration),
/// `<base>.broken-<date>` et `<base>.newer-<date>` (mises de côté par
/// l'écran du problème de la base), avec leurs `-wal` et `-shm` ; pas la base
/// elle-même, son journal, ni le verrou des ouvertures.
pub fn is_database_copy(name: &str) -> bool {
    if name == OPENING {
        return false;
    }
    [DATABASE, "chatonlm.sqlite-wal", "chatonlm.sqlite-shm"]
        .iter()
        .any(|base| {
            name.strip_prefix(base)
                .is_some_and(|rest| rest.starts_with('.') && rest.len() > 1)
        })
}

/// Efface les fichiers de l'utilisateur sous `data_dir` ; rend ceux qui
/// restent (chemins relatifs, `/` comme séparateur). `log_file` : nom du
/// journal de l'app dans `logs/`, vidé ailleurs.
pub fn erase(data_dir: &Path, log_file: &str) -> Vec<String> {
    let mut leftovers = Vec::new();
    let mut wipe = |path: PathBuf| {
        if !erase_file(&path) {
            leftovers.push(relative(data_dir, &path));
        }
    };
    wipe(data_dir.join("remote").join("keys.json"));
    for path in files_in(&data_dir.join("db"), is_database_copy) {
        wipe(path);
    }
    for path in files_in(&data_dir.join("logs"), |name| name != log_file) {
        wipe(path);
    }
    let ollama = data_dir.join("ollama");
    wipe(ollama.join("ollama.pid"));
    for folder in [
        ollama.join("home"),
        data_dir.join("transcription").join("work"),
    ] {
        for path in erase_folder(&folder) {
            leftovers.push(relative(data_dir, &path));
        }
    }
    leftovers
}

/// Supprime le dossier `dir` entier ; s'il résiste, chacun de ses fichiers
/// est supprimé ou vidé ([`erase_file`]). Rend les fichiers dont le contenu
/// reste (dossier absent : aucun).
fn erase_folder(dir: &Path) -> Vec<PathBuf> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => return Vec::new(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            crate::app_log!("chatonlm: erase: folder kept, emptied file by file: {error}")
        }
    }
    let mut kept = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(folder) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                _ => {
                    if !erase_file(&path) {
                        kept.push(path);
                    }
                }
            }
        }
    }
    kept.sort();
    kept
}

/// Fichiers ordinaires de `dir` dont le nom convient (dossier absent ou
/// illisible : aucun).
fn files_in(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| entry.file_name().to_str().is_some_and(&keep))
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

/// Supprime `path`, sinon le vide ; vrai si son contenu n'y est plus (ou
/// s'il n'existait pas).
fn erase_file(path: &Path) -> bool {
    let missing = |error: &std::io::Error| error.kind() == std::io::ErrorKind::NotFound;
    let removal = match std::fs::remove_file(path) {
        Ok(()) => return true,
        Err(error) if missing(&error) => return true,
        Err(error) => error,
    };
    let emptied = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.set_len(0));
    match emptied {
        Ok(()) => true,
        Err(error) => {
            crate::app_log!(
                "chatonlm: erase: {} kept ({removal}; {error})",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            false
        }
    }
}

/// `path` relatif à `root`, séparé par `/` (montré à l'utilisateur).
fn relative(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "secret").unwrap();
    }

    #[test]
    fn database_copies_are_told_apart_from_the_database() {
        for copy in [
            "chatonlm.sqlite.v38.bak",
            "chatonlm.sqlite.broken-1731000000000",
            "chatonlm.sqlite.newer-1731000000000",
            "chatonlm.sqlite-wal.broken-1731000000000",
            "chatonlm.sqlite-shm.newer-1731000000000",
        ] {
            assert!(is_database_copy(copy), "{copy}");
        }
        for kept in [
            "chatonlm.sqlite",
            "chatonlm.sqlite-wal",
            "chatonlm.sqlite-shm",
            "chatonlm.sqlite.opening",
            "chatonlm.sqlite.",
            "other.sqlite.v38.bak",
            "chatonlm.sqlitex.bak",
        ] {
            assert!(!is_database_copy(kept), "{kept}");
        }
    }

    /// Clés, copies de la base, journaux (sauf celui de l'app) et audio de
    /// travail effacés ; la base, son journal, les modèles, les outils, le
    /// cache du ModelHub et `launcher.json` restent.
    #[test]
    fn user_files_are_erased_and_models_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let erased = [
            "remote/keys.json",
            "db/chatonlm.sqlite.v38.bak",
            "db/chatonlm.sqlite.broken-1",
            "db/chatonlm.sqlite-wal.broken-1",
            "logs/chatonlm.log.1",
            "logs/llama-a_b_Q4_K_M.log",
            "logs/sd-server.log.1",
            "logs/whisper-server.log",
            "transcription/work/0193/audio.f32",
            "ollama/home/.ollama/id_ed25519",
            "ollama/home/.ollama/id_ed25519.pub",
            "ollama/ollama.pid",
        ];
        let kept = [
            "db/chatonlm.sqlite",
            "db/chatonlm.sqlite-wal",
            "db/chatonlm.sqlite.opening",
            "logs/chatonlm.log",
            "models/a/b/b.gguf",
            "image-models/blobs/abc",
            "speech-models/whisper-base/model.bin",
            "voice-models/fr/voice.onnx",
            "tools/yt-dlp",
            "engine/cuda/pack/lib.so",
            "modelhub/models/x.json",
            "icons/chatonlm.png",
            "launcher.json",
            "ollama/models/manifests/registry.ollama.ai/library/qwen3/0.6b",
            "ollama/models/blobs/sha256-abc",
        ];
        for path in erased.iter().chain(&kept) {
            write(&root.join(path));
        }
        assert_eq!(erase(root, "chatonlm.log"), Vec::<String>::new());
        for path in erased {
            assert!(!root.join(path).exists(), "{path}");
        }
        assert!(!root.join("ollama/home").exists());
        for path in kept {
            assert_eq!(
                std::fs::read_to_string(root.join(path)).unwrap(),
                "secret",
                "{path}"
            );
        }
        // Rien à effacer : rien ne reste.
        assert_eq!(erase(root, "chatonlm.log"), Vec::<String>::new());
    }

    /// Un fichier qui ne se supprime pas est vidé.
    #[cfg(unix)]
    #[test]
    fn a_file_that_cannot_be_removed_is_emptied() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let keys = dir.path().join("remote").join("keys.json");
        write(&keys);
        let folder = keys.parent().unwrap();
        // Dossier sans droit d'écriture : la suppression est refusée.
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o500)).unwrap();
        let leftovers = erase(dir.path(), "chatonlm.log");
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(leftovers, Vec::<String>::new());
        // Sous root, les droits ne refusent rien : supprimé.
        if !keys.exists() {
            return;
        }
        assert_eq!(std::fs::read_to_string(&keys).unwrap(), "");
    }

    /// Dossier qui ne se supprime pas d'un coup : ses fichiers sont vidés un
    /// à un ; aucun contenu ne reste.
    #[cfg(unix)]
    #[test]
    fn a_folder_that_cannot_be_removed_is_emptied_file_by_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("ollama/home/.ollama/id_ed25519");
        write(&key);
        let folder = key.parent().unwrap();
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o500)).unwrap();
        let leftovers = erase(dir.path(), "chatonlm.log");
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(leftovers, Vec::<String>::new());
        // Sous root, les droits ne refusent rien : supprimé.
        if key.exists() {
            assert_eq!(std::fs::read_to_string(&key).unwrap(), "");
        }
    }

    /// Ni supprimé ni vidé : rendu dans les restes, les autres effacés.
    #[cfg(unix)]
    #[test]
    fn a_file_that_cannot_be_erased_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let keys = dir.path().join("remote").join("keys.json");
        let log = dir.path().join("logs").join("sd-server.log");
        write(&keys);
        write(&log);
        let folder = keys.parent().unwrap();
        std::fs::set_permissions(&keys, std::fs::Permissions::from_mode(0o400)).unwrap();
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o500)).unwrap();
        let leftovers = erase(dir.path(), "chatonlm.log");
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Sous root, les droits ne refusent rien : l'essai ne dit rien.
        if std::fs::read_to_string(&keys).unwrap().is_empty() {
            return;
        }
        assert_eq!(leftovers, ["remote/keys.json"]);
        assert!(!log.exists());
    }
}
