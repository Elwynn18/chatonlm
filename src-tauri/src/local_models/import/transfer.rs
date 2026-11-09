//! Mise en place des fichiers d'un modèle repris (patron
//! `ollama/models_dir/builtin.rs`) : sur le même système de fichiers, lien
//! physique (la source reste jusqu'au ménage : une coupure ne perd rien, un
//! blob partagé avec un modèle non repris reste à sa place), renommage
//! seulement si le système refuse les liens et que rien d'autre ne cite le
//! blob ; sinon copie (fichier partiel renommé une fois complet, écrit sur
//! le disque et vérifié par son SHA-256), avec progression, annulation et
//! place vérifiée ; un blob déjà mis en place pour un autre modèle du même
//! lot est lié (lien physique, sinon copié). Un fichier déjà à la place
//! cible n'est jamais effacé : gardé s'il est identique, refus sinon. Un
//! échec ou une annulation remet tout en place ([`undo`]).

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use sha2::{Digest, Sha256};

/// Suffixe des copies en cours.
pub const PARTIAL: &str = ".chatonlm-partial";

/// Façon de mettre un fichier en place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Même système de fichiers : lien physique (la source reste) ; si le
    /// système refuse les liens (FAT, exFAT), renommage quand `rename_ok`
    /// (personne d'autre ne cite le fichier), sinon copie ; copie aussi
    /// pour un autre volume.
    Move { rename_ok: bool },
    /// Copie ; la source reste en place.
    Copy,
    /// Lien physique vers un fichier déjà mis en place, sinon copie de
    /// celui-ci.
    Link(PathBuf),
}

/// Fichier à mettre en place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub source: PathBuf,
    pub target: PathBuf,
    pub size: u64,
    /// SHA-256 attendu (hexadécimal, minuscules) : vérifié pendant une copie ;
    /// vide : inconnu, rien de vérifié (et un fichier déjà à la place cible
    /// est remplacé).
    pub sha256: String,
    pub mode: Mode,
}

/// Ce qui a été fait pour un fichier (pour le défaire).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placed {
    /// Renommé de `source` en `target`.
    Renamed { source: PathBuf, target: PathBuf },
    /// Lien physique de la source à `target` (même contenu, la source
    /// reste).
    Linked(PathBuf),
    /// Écrit à `target` (copie, ou lien vers un fichier déjà mis en place).
    Written(PathBuf),
    /// Déjà là, identique : rien à défaire.
    Present(PathBuf),
}

/// Échec d'un fichier.
#[derive(Debug)]
pub enum TransferError {
    Cancelled,
    /// Contenu copié différent de l'empreinte attendue.
    Checksum(PathBuf),
    /// Un autre fichier (contenu différent ou inconnu) est déjà à la place
    /// cible : jamais effacé.
    Exists(PathBuf),
    /// Pas assez de place pour une copie (octets nécessaires, libres).
    NoSpace {
        needed: u64,
        free: u64,
    },
    Io(PathBuf, io::Error),
    /// Échec déjà rédigé (dans la langue du moment).
    Text(String),
}

impl TransferError {
    /// Le même échec, avec en plus « des fichiers n'ont pas pu être remis en
    /// place » quand [`undo`] n'a pas tout défait.
    pub fn with_undo(self, restored: bool) -> Self {
        if restored {
            return self;
        }
        TransferError::Text(crate::i18n::t!(
            "modelImport.undoFailed",
            error = super::transfer_text(&self)
        ))
    }
}

/// Octets copiés au plus entre deux regards sur l'annulation.
const BUFFER: usize = 1 << 20;

/// Place libre suffisante pour copier `size` octets vers `target` (marge
/// comprise) ; place inconnue : on essaie.
fn room_for(target: &Path, size: u64) -> Result<(), TransferError> {
    let needed = size.saturating_add(super::SPACE_MARGIN);
    match crate::disk::free_space_for(target) {
        Some(free) if free < needed => Err(TransferError::NoSpace { needed, free }),
        _ => Ok(()),
    }
}

/// Met `step` en place ; `done` compte les octets copiés (pas ceux d'un
/// renommage ou d'un lien vers la source), `cancel` arrête une copie entre
/// deux tampons. Un fichier déjà à sa place avec le bon contenu (ou le même
/// fichier) est gardé ; un autre n'est jamais effacé : refus.
pub fn place(step: &Step, cancel: &AtomicBool, done: &AtomicU64) -> Result<Placed, TransferError> {
    let io_error = |error: io::Error| TransferError::Io(step.target.clone(), error);
    if cancel.load(Ordering::Relaxed) {
        return Err(TransferError::Cancelled);
    }
    if let Some(parent) = step.target.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    if let Ok(meta) = fs::symlink_metadata(&step.target) {
        let same = meta.is_file()
            && meta.len() == step.size
            && (super::os::same_file(&step.source, &step.target)
                || matches!(&step.mode, Mode::Link(from) if super::os::same_file(from, &step.target))
                || (!step.sha256.is_empty() && hash_file(&step.target, cancel)? == step.sha256));
        if same {
            return Ok(Placed::Present(step.target.clone()));
        }
        return Err(TransferError::Exists(step.target.clone()));
    }
    match &step.mode {
        Mode::Move { rename_ok } => {
            match fs::hard_link(&step.source, &step.target) {
                Ok(()) => return Ok(Placed::Linked(step.target.clone())),
                // Autre système de fichiers (EXDEV, ou autre volume sous
                // Windows) : copie.
                Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {}
                Err(_) if *rename_ok => match fs::rename(&step.source, &step.target) {
                    Ok(()) => {
                        return Ok(Placed::Renamed {
                            source: step.source.clone(),
                            target: step.target.clone(),
                        });
                    }
                    Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {}
                    Err(error) => return Err(TransferError::Io(step.source.clone(), error)),
                },
                // Liens refusés et fichier cité ailleurs : copie.
                Err(_) => {}
            }
            room_for(&step.target, step.size)?;
        }
        Mode::Link(from) => {
            if fs::hard_link(from, &step.target).is_ok() {
                done.fetch_add(step.size, Ordering::Relaxed);
                return Ok(Placed::Written(step.target.clone()));
            }
            room_for(&step.target, step.size)?;
            copy_verified(from, &step.target, &step.sha256, cancel, done)?;
            return Ok(Placed::Written(step.target.clone()));
        }
        Mode::Copy => {}
    }
    copy_verified(&step.source, &step.target, &step.sha256, cancel, done)?;
    Ok(Placed::Written(step.target.clone()))
}

/// Chemin du fichier partiel d'une copie vers `target`.
pub fn partial_path(target: &Path) -> PathBuf {
    let mut partial = target.as_os_str().to_os_string();
    partial.push(PARTIAL);
    PathBuf::from(partial)
}

/// Copie `source` vers `target` en calculant son SHA-256 ; le fichier
/// partiel n'est renommé que complet, écrit sur le disque et conforme à
/// `sha256`. Échec ou annulation : fichier partiel effacé.
fn copy_verified(
    source: &Path,
    target: &Path,
    sha256: &str,
    cancel: &AtomicBool,
    done: &AtomicU64,
) -> Result<(), TransferError> {
    let partial = partial_path(target);
    let result = (|| {
        let mut reader = File::open(source).map_err(|e| TransferError::Io(source.into(), e))?;
        let mut writer = File::create(&partial).map_err(|e| TransferError::Io(target.into(), e))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; BUFFER];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(TransferError::Cancelled);
            }
            let read = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(TransferError::Io(source.into(), error)),
            };
            hasher.update(&buffer[..read]);
            writer
                .write_all(&buffer[..read])
                .map_err(|e| TransferError::Io(target.into(), e))?;
            done.fetch_add(read as u64, Ordering::Relaxed);
        }
        if !sha256.is_empty() && hex(&hasher.finalize()) != sha256 {
            return Err(TransferError::Checksum(source.into()));
        }
        writer
            .sync_all()
            .map_err(|e| TransferError::Io(target.into(), e))?;
        drop(writer);
        fs::rename(&partial, target).map_err(|e| TransferError::Io(target.into(), e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

/// SHA-256 du fichier `path` (hexadécimal, minuscules).
pub fn hash_file(path: &Path, cancel: &AtomicBool) -> Result<String, TransferError> {
    let mut reader = File::open(path).map_err(|e| TransferError::Io(path.into(), e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; BUFFER];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(TransferError::Cancelled);
        }
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(TransferError::Io(path.into(), error)),
        }
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Défait `placed` (dans l'ordre inverse) : renommages remis à leur place,
/// fichiers écrits effacés, puis dossiers devenus vides sous `root`. Vrai si
/// tout a pu l'être.
pub fn undo(placed: &[Placed], root: &Path) -> bool {
    let mut restored = true;
    for item in placed.iter().rev() {
        match item {
            Placed::Renamed { source, target } => {
                let result = source
                    .parent()
                    .map_or(Ok(()), fs::create_dir_all)
                    .and_then(|()| fs::rename(target, source));
                restored &= result.is_ok();
                remove_empty_parents(target, root);
            }
            Placed::Written(target) | Placed::Linked(target) => {
                restored &= fs::remove_file(target).is_ok();
                remove_empty_parents(target, root);
            }
            Placed::Present(_) => {}
        }
    }
    restored
}

/// Retire les dossiers vides au-dessus de `path`, jusqu'à `root` exclu.
pub fn remove_empty_parents(path: &Path, root: &Path) {
    for dir in path.ancestors().skip(1) {
        if dir == root || !dir.starts_with(root) || fs::remove_dir(dir).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    fn step(dir: &Path, name: &str, content: &[u8], mode: Mode) -> Step {
        let source = dir.join("src").join(name);
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, content).unwrap();
        Step {
            source,
            target: dir.join("dst").join("a").join(name),
            size: content.len() as u64,
            sha256: sha(content),
            mode,
        }
    }

    const MOVE: Mode = Mode::Move { rename_ok: true };

    /// Même système de fichiers : lien physique, la source reste (une
    /// coupure ne perd rien) ; défaire n'efface que la cible.
    #[test]
    fn a_move_links_and_undo_removes_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let step = step(dir.path(), "w", b"weights", MOVE);
        let done = AtomicU64::new(0);
        let placed = place(&step, &AtomicBool::new(false), &done).unwrap();
        assert_eq!(placed, Placed::Linked(step.target.clone()));
        assert!(step.source.exists());
        assert!(super::super::os::same_file(&step.source, &step.target) || cfg!(windows));
        assert_eq!(fs::read(&step.target).unwrap(), b"weights");
        // Ni un lien ni un renommage ne comptent d'octets copiés.
        assert_eq!(done.load(Ordering::Relaxed), 0);
        // Déjà là (même fichier) : gardé.
        assert_eq!(
            place(&step, &AtomicBool::new(false), &done).unwrap(),
            Placed::Present(step.target.clone())
        );
        assert!(undo(&[placed], &dir.path().join("dst")));
        assert_eq!(fs::read(&step.source).unwrap(), b"weights");
        assert!(!dir.path().join("dst").join("a").exists());
    }

    #[test]
    fn a_renamed_file_is_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let step = step(dir.path(), "w", b"weights", MOVE);
        let target = step.target.clone();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::rename(&step.source, &target).unwrap();
        let placed = Placed::Renamed {
            source: step.source.clone(),
            target,
        };
        assert!(undo(&[placed], &dir.path().join("dst")));
        assert_eq!(fs::read(&step.source).unwrap(), b"weights");
    }

    #[test]
    fn a_copy_is_verified_and_keeps_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let step = step(dir.path(), "w", &vec![7u8; 3 * BUFFER + 5], Mode::Copy);
        let done = AtomicU64::new(0);
        let placed = place(&step, &AtomicBool::new(false), &done).unwrap();
        assert_eq!(placed, Placed::Written(step.target.clone()));
        assert!(step.source.exists());
        assert_eq!(done.load(Ordering::Relaxed), step.size);
        assert!(!partial_path(&step.target).exists());
        // Empreinte fausse : rien n'est laissé.
        let mut wrong = step.clone();
        wrong.target = dir.path().join("dst").join("b");
        wrong.sha256 = "0".repeat(64);
        assert!(matches!(
            place(&wrong, &AtomicBool::new(false), &done),
            Err(TransferError::Checksum(_))
        ));
        assert!(!wrong.target.exists());
        assert!(!partial_path(&wrong.target).exists());
    }

    #[test]
    fn a_cancelled_copy_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let step = step(dir.path(), "w", &vec![1u8; 2 * BUFFER], Mode::Copy);
        let result = place(&step, &AtomicBool::new(true), &AtomicU64::new(0));
        assert!(matches!(result, Err(TransferError::Cancelled)));
        assert!(!step.target.exists());
        assert!(!partial_path(&step.target).exists());
        assert!(step.source.exists());
    }

    /// Un fichier identique déjà en place est gardé ; un autre (contenu
    /// différent, ou inconnu) n'est jamais effacé (relecture L6, I3 et M2).
    #[test]
    fn a_present_file_is_kept_and_another_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let step = step(dir.path(), "w", b"same", MOVE);
        fs::create_dir_all(step.target.parent().unwrap()).unwrap();
        fs::write(&step.target, b"same").unwrap();
        let placed = place(&step, &AtomicBool::new(false), &AtomicU64::new(0)).unwrap();
        assert_eq!(placed, Placed::Present(step.target.clone()));
        // La source n'a pas bougé ; défaire ne touche à rien.
        assert!(step.source.exists());
        assert!(undo(&[placed], dir.path()));
        assert!(step.target.exists());
        fs::write(&step.target, b"diff").unwrap();
        for mode in [MOVE, Mode::Copy, Mode::Link(step.source.clone())] {
            let other = Step {
                mode,
                ..step.clone()
            };
            assert!(matches!(
                place(&other, &AtomicBool::new(false), &AtomicU64::new(0)),
                Err(TransferError::Exists(_))
            ));
            assert_eq!(fs::read(&step.target).unwrap(), b"diff");
            assert!(step.source.exists());
        }
        // Empreinte inconnue : rien ne prouve que c'est le même, refus.
        let unknown = Step {
            sha256: String::new(),
            ..step.clone()
        };
        fs::write(&step.target, b"same").unwrap();
        assert!(matches!(
            place(&unknown, &AtomicBool::new(false), &AtomicU64::new(0)),
            Err(TransferError::Exists(_))
        ));
    }

    #[test]
    fn a_shared_blob_is_linked() {
        let dir = tempfile::tempdir().unwrap();
        let first = step(dir.path(), "p", b"projector", MOVE);
        place(&first, &AtomicBool::new(false), &AtomicU64::new(0)).unwrap();
        let second = Step {
            source: first.source.clone(),
            target: dir.path().join("dst").join("other").join("p"),
            mode: Mode::Link(first.target.clone()),
            ..first.clone()
        };
        let placed = place(&second, &AtomicBool::new(false), &AtomicU64::new(0)).unwrap();
        assert_eq!(placed, Placed::Written(second.target.clone()));
        assert_eq!(fs::read(&second.target).unwrap(), b"projector");
        // Effacer l'un laisse l'autre.
        assert!(undo(&[placed], &dir.path().join("dst")));
        assert!(first.target.exists());
    }
}
