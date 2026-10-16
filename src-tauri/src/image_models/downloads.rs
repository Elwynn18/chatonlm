//! File des modèles d'images : la file des modèles de transcription
//! (`dictation::downloads`), troisième instance. Un modèle à la fois
//! (`max_running = 1`, tâche annulée comprise) : deux modèles d'une famille ne
//! téléchargent jamais la même pièce en même temps. Fichiers rangés par
//! empreinte sous `<données>/image-models/blobs/<sha256>/<nom>` (contenu
//! partagé entre modèles, nom d'origine gardé pour l'extension), adresses
//! `{HF}/{dépôt}/resolve/{révision}/{chemin}` (`CHATONLM_HF_URL` dans les
//! essais). Place recontrôlée au démarrage de chaque tâche.

use std::collections::HashSet;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use crate::dictation::downloads::{
    DownloadItem, ItemFile, Options, SpeechDownload, SpeechDownloads,
};
use crate::hub::names;
use crate::i18n::{Lang, t};
use crate::store::Store;
use crate::store::image_models::{StoredFile, mark_installed};

/// Événement envoyé à la page à chaque changement de la liste.
pub const DOWNLOADS_EVENT: &str = "image-downloads";

/// Marge gardée sur le disque en plus des fichiers à télécharger.
pub const SPACE_MARGIN: u64 = 100 * 1024 * 1024;

/// Caractères laissés tels quels dans un segment d'adresse.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.');

/// Téléchargements des modèles d'images (état Tauri).
pub struct ImageDownloads(pub SpeechDownloads);

impl Deref for ImageDownloads {
    type Target = SpeechDownloads;

    fn deref(&self) -> &SpeechDownloads {
        &self.0
    }
}

impl ImageDownloads {
    /// `emit` reçoit la liste complète à chaque changement ; `root` : dossier
    /// des modèles d'images.
    pub fn new(emit: impl Fn(&[SpeechDownload]) + Send + Sync + 'static, root: PathBuf) -> Self {
        Self::with_base_url(emit, root, None)
    }

    /// `base_url` : adresse imposée à tous les fichiers (essais).
    pub fn with_base_url(
        emit: impl Fn(&[SpeechDownload]) + Send + Sync + 'static,
        root: PathBuf,
        base_url: Option<String>,
    ) -> Self {
        Self(SpeechDownloads::with_options(
            emit,
            root,
            Options {
                max_running: 1,
                base_url,
                test_env: "CHATONLM_HF_URL",
                // Pas de catalogue : chaque modèle est mis en file tel quel
                // (« Try again » reprend celui de son entrée).
                find: Arc::new(|_| None),
                count_ending_tasks: true,
                before_start: Some(Arc::new(check_space)),
                ..Options::default()
            },
        ))
    }
}

impl ImageDownloads {
    /// Chaque modèle dont tous les fichiers viennent d'arriver est noté
    /// « installé au moins une fois » dans `store` (un retrait de la liste ne
    /// l'oublie plus).
    pub fn record_installs(&self, store: Store) {
        self.on_installed(move |id| {
            if let Err(error) = store.blocking(|connection| mark_installed(connection, id)) {
                // i18n-ignore : journal.
                eprintln!("chatonlm: image model {id} not marked installed: {error}");
            }
        });
    }
}

/// Un modèle à télécharger : ceux de ses fichiers qui manquaient au moment
/// du choix (taille du téléchargement = octets manquants).
pub struct ImageItem {
    pub id: String,
    pub name: String,
    pub files: Vec<StoredFile>,
}

impl DownloadItem for ImageItem {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn files(&self, base: Option<&str>) -> Vec<ItemFile> {
        let base = base.unwrap_or(names::DEFAULT_HF_URL);
        unique(&self.files)
            .into_iter()
            .map(|file| ItemFile {
                path: file.file.clone(),
                size: file.size,
                sha256: file.sha256.clone(),
                local: blob_relative(file),
                url: resolve_url(base, file),
            })
            .collect()
    }
}

/// Fichiers sans doublon (même empreinte et même nom : un seul fichier sur
/// le disque ; même contenu sous deux noms : deux fichiers).
pub fn unique(files: &[StoredFile]) -> Vec<&StoredFile> {
    let mut seen = HashSet::new();
    files
        .iter()
        .filter(|file| seen.insert(blob_relative(file)))
        .collect()
}

/// Somme de tailles, bornée (tailles annoncées par Hugging Face).
pub fn total(sizes: impl IntoIterator<Item = u64>) -> u64 {
    sizes.into_iter().fold(0, u64::saturating_add)
}

/// Nom du fichier sans le chemin du dépôt.
pub fn basename(file: &str) -> &str {
    file.rsplit('/').next().unwrap_or(file)
}

/// Longueur maximale d'un nom de fichier rangé (octets).
const NAME_MAX: usize = 150;

/// Nom de fichier rangeable sur le disque, sous Linux comme sous Windows :
/// ni vide, ni `.`/`..`, ni caractère interdit par Windows ou de contrôle, ni
/// point ou espace final, ni nom réservé de Windows (`CON`, `PRN`, `AUX`,
/// `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`, avec ou sans extension), 150 octets
/// au plus.
pub fn safe_name(file: &str) -> bool {
    let name = basename(file);
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let reserved = matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON" | "PRN" | "AUX" | "NUL"
    ) || (stem.len() == 4
        && ["COM", "LPT"].iter().any(|prefix| {
            stem.get(..3)
                .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
        })
        && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= NAME_MAX
        && !reserved
        && !name.chars().any(|c| {
            c.is_control() || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        && !name.ends_with(['.', ' '])
}

/// `blobs/<sha256>/<nom>`, relatif au dossier des modèles d'images.
pub fn blob_relative(file: &StoredFile) -> PathBuf {
    Path::new("blobs")
        .join(file.sha256.to_ascii_lowercase())
        .join(basename(&file.file))
}

pub fn blob_path(root: &Path, file: &StoredFile) -> PathBuf {
    root.join(blob_relative(file))
}

/// Adresse du fichier à sa révision.
pub fn resolve_url(base: &str, file: &StoredFile) -> String {
    let path: Vec<String> = file
        .file
        .split('/')
        .map(|segment| utf8_percent_encode(segment, SEGMENT).to_string())
        .collect();
    format!(
        "{base}/{}/resolve/{}/{}",
        file.repo,
        utf8_percent_encode(&file.revision, SEGMENT),
        path.join("/")
    )
}

/// Fichier présent à la bonne taille (donc vérifié : la file ne le renomme
/// qu'après sa vérification).
pub fn present(root: &Path, file: &StoredFile) -> bool {
    std::fs::metadata(blob_path(root, file))
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() == file.size)
}

/// Octets à télécharger pour `files` (chemins sans doublon) : tailles des
/// fichiers absents (les morceaux `.part` déjà reçus ne sont pas déduits :
/// même calcul que la page).
pub fn missing_bytes(files: impl IntoIterator<Item = (PathBuf, u64)>) -> u64 {
    let mut seen = HashSet::new();
    total(
        files
            .into_iter()
            .filter(|(path, _)| seen.insert(path.clone()))
            .filter(|(path, size)| !complete(path, *size))
            .map(|(_, size)| size),
    )
}

/// Place encore nécessaire pour `files` : tailles des fichiers absents,
/// moins les morceaux `.part` déjà reçus (une reprise sur un disque presque
/// plein n'est pas refusée).
pub fn needed_bytes(files: impl IntoIterator<Item = (PathBuf, u64)>) -> u64 {
    let mut seen = HashSet::new();
    total(
        files
            .into_iter()
            .filter(|(path, _)| seen.insert(path.clone()))
            .filter(|(path, size)| !complete(path, *size))
            .map(|(path, size)| size.saturating_sub(part_len(&path))),
    )
}

fn complete(path: &Path, size: u64) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() == size)
}

/// Morceau `<fichier>.part`.
pub fn part_path(path: &Path) -> PathBuf {
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    PathBuf::from(part)
}

fn part_len(path: &Path) -> u64 {
    std::fs::metadata(part_path(path)).map_or(0, |meta| meta.len())
}

/// Refus si la place manque : `needed + 100 Mio > libre` ; place inconnue :
/// acceptée.
pub fn space_problem(needed: u64, free: Option<u64>) -> Option<String> {
    let free = free?;
    (needed > 0 && needed.saturating_add(SPACE_MARGIN) > free).then(|| {
        t!(
            "imageModels.notEnoughSpace",
            needed = size_text(needed.saturating_add(SPACE_MARGIN)),
            free = size_text(free)
        )
    })
}

/// Contrôle de place pour `files` (chemins complets et tailles), avec la
/// place libre `free`.
pub fn space_check(
    files: impl IntoIterator<Item = (PathBuf, u64)>,
    free: Option<u64>,
) -> Result<(), String> {
    match space_problem(needed_bytes(files), free) {
        Some(problem) => Err(problem),
        None => Ok(()),
    }
}

/// Contrôle de la file au démarrage d'une tâche.
fn check_space(item: &dyn DownloadItem, root: &Path) -> Result<(), String> {
    space_check(
        item.files(None)
            .into_iter()
            .map(|file| (root.join(file.local), file.size)),
        crate::disk::free_space_for(root),
    )
}

/// Taille lisible (Go au-delà d'un milliard d'octets, sinon Mo), dans la
/// langue du moment.
pub fn size_text(bytes: u64) -> String {
    let decimal = |text: String| match crate::i18n::current() {
        Lang::En => text,
        Lang::Es | Lang::Fr => text.replace('.', ","),
    };
    if bytes >= 1_000_000_000 {
        let value = decimal(format!("{:.1}", bytes as f64 / 1e9));
        t!("imageModels.sizeGb", value = value)
    } else {
        let value = format!("{}", (bytes as f64 / 1e6).ceil() as u64);
        t!("imageModels.sizeMb", value = value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_gen::Component;
    use crate::image_models::types::ImageComponentRole;

    fn file(repo: &str, path: &str, sha: char, size: u64) -> StoredFile {
        StoredFile {
            role: ImageComponentRole::Model,
            component: Component::DiffusionModel,
            repo: repo.into(),
            revision: "0123abcd".into(),
            file: path.into(),
            size,
            sha256: sha.to_string().repeat(64),
            license: None,
        }
    }

    #[test]
    fn files_are_stored_by_checksum_and_fetched_at_their_revision() {
        let item = ImageItem {
            id: "a/b/m.gguf".into(),
            name: "m".into(),
            files: vec![
                file("a/b", "m.gguf", 'a', 10),
                file("c/vae", "split files/vae/ae.safetensors", 'b', 5),
                // Même contenu sous un autre nom : un second fichier ; sous
                // le même nom (pièce citée deux fois) : un seul.
                file("d/e", "copy.gguf", 'a', 10),
                file("f/g", "m.gguf", 'a', 10),
            ],
        };
        let files = item.files(Some("http://127.0.0.1:9"));
        assert_eq!(files.len(), 3);
        assert_eq!(
            files[2].local,
            Path::new("blobs").join("a".repeat(64)).join("copy.gguf")
        );
        assert_eq!(
            files[1].local,
            Path::new("blobs")
                .join("b".repeat(64))
                .join("ae.safetensors")
        );
        assert_eq!(
            files[1].url,
            "http://127.0.0.1:9/c/vae/resolve/0123abcd/split%20files/vae/ae.safetensors"
        );
        assert_eq!(
            item.files(None)[0].url,
            "https://huggingface.co/a/b/resolve/0123abcd/m.gguf"
        );
        assert_eq!(item.size(), 25);
    }

    #[test]
    fn unsafe_names_are_told() {
        assert!(safe_name("dir/z_image_turbo-Q4_K.gguf"));
        assert!(safe_name("console.gguf"));
        assert!(safe_name("COM0.gguf"));
        assert!(safe_name("LPT10.gguf"));
        for bad in [
            "..",
            "a/..",
            "x:y.gguf",
            "a\\b.gguf",
            "con.",
            "",
            "CON",
            "nul.gguf",
            "dir/Aux.safetensors",
            "com1.gguf",
            "LPT9",
            "prn.tar.gz",
            "a?.gguf",
            "a\u{1}.gguf",
        ] {
            assert!(!safe_name(bad), "{bad}");
        }
        // Pas de coupure au milieu d'un caractère.
        for odd in ["a€", "a€.gguf"] {
            assert!(safe_name(odd), "{odd}");
        }
        assert!(safe_name(&format!("{}.gguf", "a".repeat(145))));
        assert!(!safe_name(&format!("{}.gguf", "a".repeat(146))));
    }

    #[test]
    fn sizes_add_up_without_overflowing() {
        assert_eq!(total([u64::MAX, 10]), u64::MAX);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            missing_bytes([(dir.path().join("a"), u64::MAX), (dir.path().join("b"), 10)]),
            u64::MAX
        );
    }

    #[test]
    fn a_resume_needs_only_what_is_left() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("model.gguf");
        std::fs::write(part_path(&file), [0; 90]).unwrap();
        assert_eq!(missing_bytes([(file.clone(), 100)]), 100);
        assert_eq!(needed_bytes([(file.clone(), 100)]), 10);
        // Place pour la fin seulement : la reprise passe, pas un nouveau.
        let free = Some(SPACE_MARGIN + 50);
        assert!(space_check([(file, 100)], free).is_ok());
        assert!(space_check([(dir.path().join("other.gguf"), 100)], free).is_err());
    }

    #[test]
    fn missing_bytes_count_absent_files_once() {
        let dir = tempfile::tempdir().unwrap();
        let whole = dir.path().join("whole");
        std::fs::write(&whole, [0; 10]).unwrap();
        let partial = dir.path().join("partial");
        std::fs::write(dir.path().join("partial.part"), [0; 4]).unwrap();
        let absent = dir.path().join("absent");
        assert_eq!(
            missing_bytes([
                (whole, 10),
                (partial.clone(), 10),
                (partial, 10),
                (absent, 7)
            ]),
            10 + 7
        );
    }

    #[test]
    fn space_is_refused_with_both_sizes() {
        assert_eq!(space_problem(0, Some(0)), None);
        assert_eq!(space_problem(5, None), None);
        assert_eq!(
            space_problem(1_000_000, Some(SPACE_MARGIN + 1_000_000)),
            None
        );
        assert_eq!(
            space_problem(3_000_000_000, Some(2_500_000_000)).as_deref(),
            Some("Not enough disk space: 3.1 GB needed, 2.5 GB free")
        );
        assert_eq!(size_text(1), "1 MB");
    }
}
