//! Disque d'un projet de l'Atelier : dossier du projet (créé sous le dossier
//! des projets, nom court et unique), dossier de travail `.atelier/`, et
//! écriture d'un fichier par le projet avec ses garde-fous (conception § 7,
//! « écriture seulement dans le dossier du projet »).
//!
//! Garde-fous de [`write_in`], que rien ne lève :
//! - chemin RELATIF seulement, écrit avec `/` ou `\` : ni racine, ni lecteur
//!   (`C:`), ni `..`, ni segment vide, ni caractère interdit sous Windows, ni
//!   nom réservé de Windows (`CON`, `NUL`…), ni point ou espace final ;
//! - aucun lien symbolique suivi (ni dans les dossiers traversés, ni à la
//!   place du fichier), puis le dossier atteint, résolu, doit être sous le
//!   dossier du projet résolu (jonctions de Windows comprises) ;
//! - un fichier déjà là HORS de `.atelier/` (livrable, peut-être retouché par
//!   l'utilisateur) n'est jamais écrasé sans copie : l'ancien part dans
//!   `.atelier/versions/` ;
//! - écriture dans un fichier temporaire du même dossier, puis renommage.
//!
//! Logique pure sans `cfg` (Linux et Windows suivent les mêmes règles, les
//! plus strictes des deux).

use std::path::{Path, PathBuf};

/// Dossier de travail d'un projet (spécification, plan, sources, journal).
pub(crate) const WORK_DIR: &str = ".atelier";

/// Copies des livrables remplacés.
const VERSIONS_DIR: &str = "versions";

/// Longueur maximale du nom du dossier d'un projet (caractères).
pub(crate) const MAX_FOLDER_CHARS: usize = 60;

/// Longueur maximale d'un chemin relatif écrit par un projet (caractères).
const MAX_RELATIVE_CHARS: usize = 400;

/// Caractères refusés dans un nom (Windows), en plus des caractères de
/// contrôle.
const FORBIDDEN: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Refus d'une écriture : code stable (journal, événement).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WriteError {
    /// Chemin vide, absolu, avec lecteur, `..`, segment vide ou nom refusé.
    BadPath,
    /// Un lien symbolique (ou une jonction) sur le chemin.
    Link,
    /// Le dossier atteint n'est pas sous le dossier du projet.
    Outside,
    /// Erreur du système (message du système, pour le journal seulement).
    Io(String),
}

impl WriteError {
    /// Code stable (événement, page) : jamais le message du système, qui
    /// part au journal ([`Self::log`]).
    pub(crate) fn code(&self) -> String {
        match self {
            WriteError::BadPath => "badPath".into(),
            WriteError::Link => "link".into(),
            WriteError::Outside => "outside".into(),
            WriteError::Io(_) => "io".into(),
        }
    }

    /// Message du système au journal (erreur d'entrée-sortie seulement).
    pub(crate) fn log(&self, what: &str) {
        if let WriteError::Io(error) = self {
            // i18n-ignore : journal.
            crate::app_log!("chatonlm: workshop: {what} not written: {error}");
        }
    }
}

impl From<std::io::Error> for WriteError {
    fn from(error: std::io::Error) -> Self {
        WriteError::Io(error.to_string())
    }
}

/// Un segment de nom acceptable partout (Linux et Windows).
fn valid_segment(segment: &str) -> bool {
    if segment.is_empty() || segment == "." || segment == ".." {
        return false;
    }
    if segment
        .chars()
        .any(|c| c.is_control() || FORBIDDEN.contains(&c))
    {
        return false;
    }
    if segment.ends_with('.') || segment.ends_with(' ') {
        return false;
    }
    !crate::os_util::windows_reserved(segment)
}

/// Segments d'un chemin relatif écrit par un projet, séparés par `/` ou `\` ;
/// `None` s'il est refusé (voir le module).
pub(crate) fn relative_segments(relative: &str) -> Option<Vec<&str>> {
    if relative.is_empty() || relative.chars().count() > MAX_RELATIVE_CHARS {
        return None;
    }
    let segments: Vec<&str> = relative.split(['/', '\\']).collect();
    segments
        .iter()
        .all(|segment| valid_segment(segment))
        .then_some(segments)
}

/// Le chemin désigne un fichier de `.atelier/` (casse ignorée : Windows).
fn in_work_dir(segments: &[&str]) -> bool {
    segments.len() > 1 && segments[0].eq_ignore_ascii_case(WORK_DIR)
}

/// Lien symbolique, ou point de montage/jonction sous Windows (vu comme un
/// lien par `symlink_metadata` depuis Rust 1.x pour les liens, pas toujours
/// pour les jonctions : la résolution finale les attrape).
fn is_link(path: &Path) -> std::io::Result<Option<bool>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata.file_type().is_symlink())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Écrit `bytes` dans `root/relative` (voir les garde-fous du module).
/// `root` est le dossier du projet (existant).
pub(crate) fn write_in(root: &Path, relative: &str, bytes: &[u8]) -> Result<(), WriteError> {
    let segments = relative_segments(relative).ok_or(WriteError::BadPath)?;
    if is_link(root)? != Some(false) {
        return Err(WriteError::Link);
    }
    let real_root = root.canonicalize()?;
    let (name, parents) = segments.split_last().ok_or(WriteError::BadPath)?;
    // Dossiers traversés : jamais un lien, créés au besoin.
    let real_dir = dirs_inside(&real_root, parents)?;
    let target = real_dir.join(name);
    match is_link(&target)? {
        Some(true) => return Err(WriteError::Link),
        Some(false) if !in_work_dir(&segments) => {
            if target.is_dir() {
                return Err(WriteError::BadPath);
            }
            keep_version(&real_root, &segments, &target)?;
        }
        Some(false) | None => {}
    }
    if target.is_dir() {
        return Err(WriteError::BadPath);
    }
    write_atomic(&real_dir, name, &target, bytes)
}

/// `dir` résolu (jonctions de Windows, montages) : un dossier sous
/// `real_root` (résolu), sinon [`WriteError::Outside`]. Sous Linux, les liens
/// étant refusés avant, seul un montage ou une course y mène.
fn resolved_inside(real_root: &Path, dir: &Path) -> Result<PathBuf, WriteError> {
    let real_dir = dir.canonicalize()?;
    if !real_dir.starts_with(real_root) || !real_dir.is_dir() {
        return Err(WriteError::Outside);
    }
    Ok(real_dir)
}

/// Dossiers `real_root/<segments…>`, vérifiés UN PAR UN avant toute
/// création (jamais un lien, créés s'il manquent), puis le dernier résolu
/// sous `real_root`. Un `create_dir_all` suivrait un `.atelier` remplacé par un
/// lien et créerait ses dossiers ailleurs.
fn dirs_inside(real_root: &Path, segments: &[&str]) -> Result<PathBuf, WriteError> {
    let mut dir = real_root.to_path_buf();
    for segment in segments {
        dir.push(segment);
        match is_link(&dir)? {
            Some(true) => return Err(WriteError::Link),
            Some(false) => {}
            None => match std::fs::create_dir(&dir) {
                Ok(()) => {}
                // Créé entre-temps : revérifié.
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if is_link(&dir)? != Some(false) {
                        return Err(WriteError::Link);
                    }
                }
                Err(error) => return Err(error.into()),
            },
        }
    }
    resolved_inside(real_root, &dir)
}

/// Copie l'ancien `target` (livrable) dans `.atelier/versions/<chemin>.<n>`
/// (n = 1, 2… le premier libre).
fn keep_version(real_root: &Path, segments: &[&str], target: &Path) -> Result<(), WriteError> {
    let versions = dirs_inside(real_root, &[WORK_DIR, VERSIONS_DIR])?;
    let flat = segments.join("_");
    for n in 1..10_000u32 {
        let copy = versions.join(format!("{flat}.{n}"));
        if is_link(&copy)?.is_none() {
            std::fs::copy(target, &copy)?;
            return Ok(());
        }
    }
    Err(WriteError::Io("too many versions".into()))
}

/// Écrit dans un fichier temporaire de `dir`, puis le renomme en `target`
/// (remplacé s'il existe, Linux et Windows).
fn write_atomic(dir: &Path, name: &str, target: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    let temporary = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::now_v7().simple()));
    let written =
        std::fs::write(&temporary, bytes).and_then(|()| std::fs::rename(&temporary, target));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

/// Nom de dossier tiré de `wanted` (nom donné, ou titre) : caractères
/// refusés remplacés par `-`, espaces réduits, points et espaces des bords
/// retirés, au plus [`MAX_FOLDER_CHARS`] caractères ; nom réservé ou vide :
/// `fallback`.
pub(crate) fn folder_name(wanted: &str, fallback: &str) -> String {
    let replaced: String = wanted
        .chars()
        .map(|c| {
            if c.is_control() || FORBIDDEN.contains(&c) {
                '-'
            } else if c.is_whitespace() {
                ' '
            } else {
                c
            }
        })
        .collect();
    let joined = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let clipped: String = joined.chars().take(MAX_FOLDER_CHARS).collect();
    let trimmed = clipped.trim_matches(|c: char| c == '.' || c == ' ' || c == '-');
    if valid_segment(trimmed) {
        trimmed.to_owned()
    } else {
        fallback.to_owned()
    }
}

/// Crée `<root>/<name>/` (nom libre : `<name>`, puis `<name> (2)`,
/// `<name> (3)`…) et son `.atelier/` ; `root` est créé au besoin. Rend le
/// chemin du dossier.
pub(crate) fn create_project_dir(root: &Path, name: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    for n in 1..10_000u32 {
        let candidate = if n == 1 {
            root.join(name)
        } else {
            root.join(format!("{name} ({n})"))
        };
        match std::fs::create_dir(&candidate) {
            Ok(()) => {
                std::fs::create_dir(candidate.join(WORK_DIR))?;
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other("no free folder name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = create_project_dir(dir.path(), "Guide").unwrap();
        (dir, root)
    }

    #[test]
    fn relative_paths_are_split_on_both_separators() {
        assert_eq!(relative_segments("a/b\\c.md"), Some(vec!["a", "b", "c.md"]));
        assert_eq!(relative_segments(".atelier/plan.json").unwrap().len(), 2);
    }

    #[test]
    fn escaping_or_odd_paths_are_refused() {
        for bad in [
            "",
            "/etc/passwd",
            "\\Windows\\x",
            "C:\\x.md",
            "C:x.md",
            "../x.md",
            "a/../../x.md",
            "a/./b.md",
            "a//b.md",
            "a/",
            "CON",
            "nul.txt",
            "a/com1.md",
            "x.md.",
            "x.md ",
            "a\u{0}b",
            "what?.md",
        ] {
            assert_eq!(relative_segments(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_file_is_written_under_the_project_with_its_folders() {
        let (_dir, root) = project();
        write_in(&root, "chapitres/un.md", b"un").unwrap();
        write_in(&root, ".atelier/spec.json", b"{}").unwrap();
        assert_eq!(std::fs::read(root.join("chapitres/un.md")).unwrap(), b"un");
        assert_eq!(
            std::fs::read(root.join(".atelier/spec.json")).unwrap(),
            b"{}"
        );
        // Aucun fichier temporaire laissé.
        let names: Vec<String> = std::fs::read_dir(root.join("chapitres"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["un.md"]);
    }

    #[test]
    fn parent_and_absolute_paths_write_nothing() {
        let (dir, root) = project();
        let outside = dir.path().join("outside.md");
        for bad in ["../outside.md", outside.to_str().unwrap()] {
            assert_eq!(write_in(&root, bad, b"x"), Err(WriteError::BadPath));
        }
        assert!(!outside.exists());
    }

    #[test]
    fn a_deliverable_is_never_overwritten_without_a_copy() {
        let (_dir, root) = project();
        write_in(&root, "guide.md", b"v1").unwrap();
        write_in(&root, "guide.md", b"v2").unwrap();
        write_in(&root, "guide.md", b"v3").unwrap();
        assert_eq!(std::fs::read(root.join("guide.md")).unwrap(), b"v3");
        let versions = root.join(".atelier/versions");
        assert_eq!(std::fs::read(versions.join("guide.md.1")).unwrap(), b"v1");
        assert_eq!(std::fs::read(versions.join("guide.md.2")).unwrap(), b"v2");
        // Fichier de travail : remplacé sans copie.
        write_in(&root, ".atelier/plan.json", b"1").unwrap();
        write_in(&root, ".atelier/plan.json", b"2").unwrap();
        assert_eq!(std::fs::read_dir(&versions).unwrap().count(), 2);
    }

    #[test]
    fn a_folder_in_place_of_the_file_is_refused() {
        let (_dir, root) = project();
        std::fs::create_dir(root.join("guide.md")).unwrap();
        assert_eq!(write_in(&root, "guide.md", b"x"), Err(WriteError::BadPath));
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_never_followed() {
        let (dir, root) = project();
        let outside = dir.path().join("elsewhere");
        std::fs::create_dir(&outside).unwrap();
        // Dossier lié vers l'extérieur.
        std::os::unix::fs::symlink(&outside, root.join("sortie")).unwrap();
        assert_eq!(write_in(&root, "sortie/x.md", b"x"), Err(WriteError::Link));
        // Fichier lié vers l'extérieur.
        let target = outside.join("victim.md");
        std::fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, root.join("lien.md")).unwrap();
        assert_eq!(write_in(&root, "lien.md", b"x"), Err(WriteError::Link));
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
        // Dossier du projet lui-même remplacé par un lien.
        let moved = dir.path().join("moved");
        std::fs::rename(&root, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &root).unwrap();
        assert_eq!(write_in(&root, "x.md", b"x"), Err(WriteError::Link));
    }

    /// Relecture du lot 2 : `.atelier` (ou `.atelier/versions`) remplacé par
    /// un lien vers l'extérieur ne fait rien créer dehors quand un livrable
    /// est remplacé (avant : `create_dir_all` suivait le lien).
    #[cfg(unix)]
    #[test]
    fn a_linked_work_dir_creates_nothing_outside() {
        let (dir, root) = project();
        let outside = dir.path().join("elsewhere");
        std::fs::create_dir(&outside).unwrap();
        write_in(&root, "guide.md", b"v1").unwrap();
        std::fs::remove_dir(root.join(WORK_DIR)).unwrap();
        std::os::unix::fs::symlink(&outside, root.join(WORK_DIR)).unwrap();
        assert_eq!(write_in(&root, "guide.md", b"v2"), Err(WriteError::Link));
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        assert_eq!(std::fs::read(root.join("guide.md")).unwrap(), b"v1");
        // `.atelier` réel, `versions` lié vers l'extérieur.
        std::fs::remove_file(root.join(WORK_DIR)).unwrap();
        std::fs::create_dir(root.join(WORK_DIR)).unwrap();
        std::os::unix::fs::symlink(&outside, root.join(WORK_DIR).join(VERSIONS_DIR)).unwrap();
        assert_eq!(write_in(&root, "guide.md", b"v3"), Err(WriteError::Link));
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }

    #[test]
    fn a_system_error_gives_a_stable_code() {
        assert_eq!(
            WriteError::Io("Permission denied (os error 13)".into()).code(),
            "io"
        );
    }

    #[test]
    fn a_resolved_folder_outside_the_root_is_refused() {
        // Ce que donnerait une jonction (Windows) ou un montage : le dossier
        // atteint, une fois résolu, est hors du dossier du projet.
        let (dir, root) = project();
        let real_root = root.canonicalize().unwrap();
        let outside = dir.path().join("elsewhere");
        std::fs::create_dir(&outside).unwrap();
        assert_eq!(
            resolved_inside(&real_root, &outside),
            Err(WriteError::Outside)
        );
        // Un fichier à la place du dossier : refusé aussi.
        std::fs::write(root.join("f"), b"x").unwrap();
        assert_eq!(
            resolved_inside(&real_root, &root.join("f")),
            Err(WriteError::Outside)
        );
        std::fs::create_dir(root.join("in")).unwrap();
        assert_eq!(
            resolved_inside(&real_root, &root.join("in")),
            Ok(real_root.join("in"))
        );
    }

    #[test]
    fn a_missing_project_folder_writes_nothing() {
        // Chemin du projet qui n'existe pas (dossier déplacé) : erreur, rien
        // n'est écrit ailleurs.
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone");
        assert!(write_in(&gone, "x.md", b"x").is_err());
        assert!(!gone.exists());
    }

    #[test]
    fn folder_names_are_cleaned_and_bounded() {
        assert_eq!(
            folder_name("  Guide : la  permaculture ?  ", "p"),
            "Guide - la permaculture"
        );
        assert_eq!(folder_name("..", "projet"), "projet");
        assert_eq!(folder_name("CON", "projet"), "projet");
        assert_eq!(folder_name("a/b\\c", "p"), "a-b-c");
        let long = "é".repeat(100);
        assert_eq!(folder_name(&long, "p").chars().count(), MAX_FOLDER_CHARS);
    }

    #[test]
    fn project_folders_are_unique() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ChatonLM");
        let first = create_project_dir(&root, "Guide").unwrap();
        let second = create_project_dir(&root, "Guide").unwrap();
        let third = create_project_dir(&root, "Guide").unwrap();
        assert_eq!(first, root.join("Guide"));
        assert_eq!(second, root.join("Guide (2)"));
        assert_eq!(third, root.join("Guide (3)"));
        assert!(second.join(WORK_DIR).is_dir());
    }
}
