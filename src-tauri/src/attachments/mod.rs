// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Translated to Rust from Ollama (https://github.com/ollama/ollama),
// files app/ui/app/src/utils/fileValidation.ts and app/ui/extract.go,
// commit 42e911bc3d05
// (accepted extensions, validation order and messages, convertBytesToText).
// Modified for ChatonLM: .docx is refused; files are validated, read and
// extracted once in Rust when they are added, then kept in memory until sent.

//! Pièces jointes : zone de préparation, validation et extraction du texte.
//!
//! Un fichier ajouté au composer est validé dans l'ordre de l'officiel
//! (`validateFile`, fileValidation.ts : extension, taille, lecture), lu, son
//! texte extrait une seule fois, puis gardé en mémoire sous un identifiant
//! jusqu'à l'envoi (il part en base avec le message) ou jusqu'à son retrait.
//! Rien n'est écrit sur disque ; la zone est vidée au rechargement de la page.
//!
//! Défense en profondeur : `stage_paths` ne lit que des chemins remis par le
//! moteur (sélecteur natif, dépôt natif), chacun une seule fois ; un chemin
//! inventé par la page (script injecté) est refusé sans toucher au disque.
//!
//! Écart à l'officiel (plan § D) : `.docx` est refusé (l'officiel l'accepte
//! mais n'en extrait rien : archive ZIP, envoyée comme « binaire »).

pub mod clipboard;
pub mod pdf;
pub mod pdf_child;
pub mod protocol;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::chats::{AttachmentKind, NewAttachment};
use pdf::PdfExtractor;

/// Taille maximale d'un fichier : 10 Mio, comme l'officiel.
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// `TEXT_FILE_EXTENSIONS` de l'officiel, sans `docx`.
const TEXT_EXTENSIONS: [&str; 36] = [
    "pdf", "txt", "md", "csv", "json", "xml", "html", "htm", "js", "jsx", "ts", "tsx", "py",
    "java", "cpp", "c", "cc", "h", "cs", "php", "rb", "go", "rs", "swift", "kt", "scala", "sh",
    "bat", "yaml", "yml", "toml", "ini", "cfg", "conf", "log", "rtf",
];

/// `IMAGE_EXTENSIONS` de l'officiel.
const IMAGE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

/// Refus officiels (fileValidation.ts), montrés par l'interface : textes de
/// l'officiel en anglais, traduits dans la langue de l'interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    FileTypeNotSupported,
    FileTooLarge,
    ErrorReadingFile,
}

impl Refusal {
    /// Texte dans la langue de l'interface du moment.
    pub fn text(self) -> String {
        match self {
            Refusal::FileTypeNotSupported => t!("attachments.fileTypeNotSupported"),
            Refusal::FileTooLarge => t!("attachments.fileTooLarge"),
            Refusal::ErrorReadingFile => t!("attachments.errorReadingFile"),
        }
    }
}

/// Chemins remis gardés au plus (les plus anciens sortent au-delà) : un dépôt
/// jamais préparé (page Settings, type refusé par la page) ne fait pas
/// grossir l'ensemble sans fin.
pub const MAX_GRANTED: usize = 10_000;

/// Fichier prêt à partir avec le prochain message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StagedFile {
    pub id: String,
    pub filename: String,
    pub kind: AttachmentKind,
    pub mime: String,
    pub size: u64,
}

/// Résultat de l'ajout d'un fichier : préparé, ou refusé avec le texte officiel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "status", rename_all = "camelCase")]
#[ts(export)]
pub enum StageResult {
    Staged { file: StagedFile },
    Rejected { filename: String, error: String },
}

/// Fichier préparé : sa description, son contenu et son texte (aucun pour une
/// image).
#[derive(Debug)]
pub struct Staged {
    pub file: StagedFile,
    pub data: Vec<u8>,
    pub text: Option<String>,
}

impl Staged {
    /// Pièce jointe à enregistrer avec le message.
    pub fn as_new(&self) -> NewAttachment<'_> {
        NewAttachment {
            filename: &self.file.filename,
            kind: self.file.kind,
            mime: &self.file.mime,
            data: &self.data,
            text: self.text.as_deref(),
        }
    }
}

/// Extensions acceptées, dans l'ordre de l'officiel (filtre du sélecteur
/// natif).
pub fn accepted_extensions() -> impl Iterator<Item = &'static str> {
    TEXT_EXTENSIONS.into_iter().chain(IMAGE_EXTENSIONS)
}

/// Refus officiel d'un fichier d'après son nom, puis sa taille (`None` :
/// inconnue), dans l'ordre de l'officiel ; `None` s'il passe.
pub fn refusal(filename: &str, size: Option<u64>) -> Option<Refusal> {
    if kind_of(&extension(filename)).is_none() {
        Some(Refusal::FileTypeNotSupported)
    } else if size.is_some_and(|size| size > MAX_FILE_SIZE) {
        Some(Refusal::FileTooLarge)
    } else {
        None
    }
}

/// Extension au sens de l'officiel : `nom.toLowerCase().split(".").pop()` ;
/// un nom sans point donne le nom entier.
fn extension(filename: &str) -> String {
    let lower = filename.to_lowercase();
    lower.rsplit('.').next().unwrap_or_default().to_owned()
}

/// Sorte d'un fichier d'après son extension ; `None` : type refusé.
fn kind_of(extension: &str) -> Option<AttachmentKind> {
    if IMAGE_EXTENSIONS.contains(&extension) {
        Some(AttachmentKind::Image)
    } else if TEXT_EXTENSIONS.contains(&extension) {
        Some(AttachmentKind::Text)
    } else {
        None
    }
}

/// Type MIME d'après l'extension (jamais vide).
fn mime_of(extension: &str) -> &'static str {
    match extension {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "xml" => "application/xml",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "md" => "text/markdown",
        _ => "text/plain",
    }
}

/// Texte d'un fichier, calculé une fois à la préparation, comme
/// `convertBytesToText` (extract.go) ; `None` pour une image. (Le cas
/// « Binary file of type <ext> » de l'officiel ne concerne aucune extension
/// acceptée.)
async fn text_of(data: &[u8], extension: &str, extractor: &PdfExtractor) -> Option<String> {
    if IMAGE_EXTENSIONS.contains(&extension) {
        return None;
    }
    if extension == "pdf" {
        return Some(pdf::pdf_text(data, extractor).await);
    }
    Some(match std::str::from_utf8(data) {
        Ok(text) => text.to_owned(),
        Err(_) => format!("[Binary file - {} bytes - not valid UTF-8]", data.len()),
    })
}

/// Zone de préparation (état géré de l'app).
pub struct Staging {
    inner: Mutex<Inner>,
    extractor: PdfExtractor,
}

#[derive(Default)]
struct Inner {
    files: HashMap<String, Arc<Staged>>,
    /// Chemins que `stage_paths` peut lire.
    granted: Granted,
    /// Augmente à chaque vidage : un fichier lu pour une page disparue
    /// n'est pas gardé.
    generation: u64,
}

/// Chemins remis par le moteur (sélecteur natif, dépôt natif), comparés tels
/// quels (chaîne exacte, sans canonicalisation : la page les renvoie tels
/// qu'elle les a reçus). Un chemin remis deux fois sert deux fois.
#[derive(Default)]
struct Granted {
    /// Numéros d'arrivée de chaque chemin, du plus ancien au plus récent.
    by_path: HashMap<String, VecDeque<u64>>,
    /// Chemins par numéro d'arrivée : les plus anciens sortent d'abord.
    order: BTreeMap<u64, String>,
    next: u64,
}

impl Granted {
    fn grant(&mut self, path: String) {
        let number = self.next;
        self.next += 1;
        self.by_path
            .entry(path.clone())
            .or_default()
            .push_back(number);
        self.order.insert(number, path);
        while self.order.len() > MAX_GRANTED {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            // Le plus ancien de tous est aussi le plus ancien de son chemin.
            self.pop_oldest(&oldest);
        }
    }

    /// Consomme une remise de ce chemin ; faux s'il n'a pas été remis.
    fn consume(&mut self, path: &str) -> bool {
        match self.pop_oldest(path) {
            Some(number) => {
                self.order.remove(&number);
                true
            }
            None => false,
        }
    }

    /// Retire la plus ancienne remise de ce chemin de `by_path` seulement.
    fn pop_oldest(&mut self, path: &str) -> Option<u64> {
        let numbers = self.by_path.get_mut(path)?;
        let number = numbers.pop_front();
        if numbers.is_empty() {
            self.by_path.remove(path);
        }
        number
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.order.len()
    }
}

impl Default for Staging {
    fn default() -> Self {
        Self::with_extractor(PdfExtractor::app())
    }
}

/// Fichiers retirés de la zone pour un envoi : remis en place si l'envoi est
/// refusé d'emblée, sauf si la zone a été vidée entre-temps.
pub struct Taken<'a> {
    staging: &'a Staging,
    generation: u64,
    files: Vec<Arc<Staged>>,
    kept: bool,
}

impl Taken<'_> {
    pub fn files(&self) -> &[Arc<Staged>] {
        &self.files
    }

    /// Les fichiers ne reviennent plus dans la zone (enregistrés, ou envoi
    /// accepté puis en échec).
    pub fn keep(mut self) {
        self.kept = true;
    }
}

impl Drop for Taken<'_> {
    fn drop(&mut self) {
        if self.kept || self.files.is_empty() {
            return;
        }
        let mut inner = self.staging.inner.lock();
        if inner.generation == self.generation {
            for file in self.files.drain(..) {
                inner.files.insert(file.file.id.clone(), file);
            }
        }
    }
}

impl Staging {
    /// Zone qui extrait les PDF avec ce programme (tests).
    pub fn with_extractor(extractor: PdfExtractor) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            extractor,
        }
    }

    /// Vide la zone (page rechargée : ses fichiers n'ont plus de composer,
    /// ses chemins remis plus de destinataire).
    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.files.clear();
        inner.granted = Granted::default();
        inner.generation += 1;
    }

    /// Génération courante (voir `grant_if_current`).
    pub fn generation(&self) -> u64 {
        self.inner.lock().generation
    }

    /// Remet des chemins venus du moteur (dépôt natif) : `stage_paths` pourra
    /// les lire, une fois chacun.
    pub fn grant(&self, paths: impl IntoIterator<Item = String>) {
        let mut inner = self.inner.lock();
        for path in paths {
            inner.granted.grant(path);
        }
    }

    /// Comme `grant`, si la page n'a pas été rechargée depuis `generation`
    /// (sélecteur ouvert par une page disparue : personne ne les attend).
    pub fn grant_if_current(&self, generation: u64, paths: &[String]) {
        let mut inner = self.inner.lock();
        if inner.generation == generation {
            for path in paths {
                inner.granted.grant(path.clone());
            }
        }
    }

    /// Retire des fichiers ; sans effet pour un identifiant inconnu.
    pub fn unstage(&self, ids: &[String]) {
        let mut inner = self.inner.lock();
        for id in ids {
            inner.files.remove(id);
        }
    }

    /// Fichier préparé, s'il existe encore.
    pub fn get(&self, id: &str) -> Option<Arc<Staged>> {
        self.inner.lock().files.get(id).cloned()
    }

    /// Retire ces fichiers pour un envoi, dans l'ordre demandé : tous, ou
    /// aucun si un identifiant est inconnu (ou répété).
    pub fn take(&self, ids: &[String]) -> AppResult<Taken<'_>> {
        let mut inner = self.inner.lock();
        for (index, id) in ids.iter().enumerate() {
            if !inner.files.contains_key(id) || ids[..index].contains(id) {
                // i18n-ignore : identifiant donné par le moteur à la page.
                return Err(AppError::Invalid(format!("unknown attachment: {id}")));
            }
        }
        let files = ids.iter().filter_map(|id| inner.files.remove(id)).collect();
        Ok(Taken {
            staging: self,
            generation: inner.generation,
            files,
            kept: false,
        })
    }

    /// Prépare des fichiers choisis par leur chemin (sélecteur natif, dépôt),
    /// dans l'ordre ; un dossier donne ses fichiers ordinaires de premier
    /// niveau, triés par nom, comme l'officiel. Seuls les chemins remis
    /// (`grant`) sont lus, et leur remise est consommée ; les autres sont
    /// refusés « Error reading file » sans aucun accès au disque.
    pub async fn stage_paths(&self, paths: Vec<String>) -> Vec<StageResult> {
        let (generation, paths) = {
            let mut inner = self.inner.lock();
            let paths: Vec<(String, bool)> = paths
                .into_iter()
                .map(|path| {
                    let granted = inner.granted.consume(&path);
                    (path, granted)
                })
                .collect();
            (inner.generation, paths)
        };
        let mut results = Vec::new();
        for (path, granted) in paths {
            // Page rechargée : plus personne n'attend la suite.
            if !self.is_current(generation) {
                break;
            }
            let path = PathBuf::from(path);
            if !granted {
                results.push(rejected(base_name(&path), Refusal::ErrorReadingFile));
                continue;
            }
            let listed = {
                let path = path.clone();
                tauri::async_runtime::spawn_blocking(move || list_directory(&path)).await
            };
            match listed {
                Ok(Listing::Files(files)) => {
                    for file in files {
                        if !self.is_current(generation) {
                            break;
                        }
                        results.push(self.stage_path(generation, &file).await);
                    }
                }
                Ok(Listing::NotADirectory) => {
                    results.push(self.stage_path(generation, &path).await);
                }
                Ok(Listing::Unreadable) | Err(_) => {
                    results.push(rejected(base_name(&path), Refusal::ErrorReadingFile));
                }
            }
        }
        results
    }

    /// Prépare un fichier collé (contenu reçu de la page).
    pub async fn stage_bytes(&self, filename: String, data: Vec<u8>) -> StageResult {
        let generation = self.inner.lock().generation;
        if let Some(error) = refusal(&filename, Some(data.len() as u64)) {
            return rejected(filename, error);
        }
        self.keep(generation, filename, data).await
    }

    /// La zone n'a pas été vidée (page rechargée) depuis `generation`.
    fn is_current(&self, generation: u64) -> bool {
        self.inner.lock().generation == generation
    }

    async fn stage_path(&self, generation: u64, path: &Path) -> StageResult {
        let filename = base_name(path);
        if kind_of(&extension(&filename)).is_none() {
            return rejected(filename, Refusal::FileTypeNotSupported);
        }
        let read = {
            let path = path.to_owned();
            tauri::async_runtime::spawn_blocking(move || read_limited(&path)).await
        };
        match read {
            Ok(Ok(data)) => self.keep(generation, filename, data).await,
            Ok(Err(error)) => rejected(filename, error),
            Err(_) => rejected(filename, Refusal::ErrorReadingFile),
        }
    }

    /// Extrait le texte et garde le fichier (déjà validé).
    async fn keep(&self, generation: u64, filename: String, data: Vec<u8>) -> StageResult {
        let extension = extension(&filename);
        let Some(kind) = kind_of(&extension) else {
            return rejected(filename, Refusal::FileTypeNotSupported);
        };
        let text = text_of(&data, &extension, &self.extractor).await;
        let file = StagedFile {
            id: uuid::Uuid::now_v7().to_string(),
            filename,
            kind,
            mime: mime_of(&extension).to_owned(),
            size: data.len() as u64,
        };
        let mut inner = self.inner.lock();
        if inner.generation == generation {
            let staged = Staged {
                file: file.clone(),
                data,
                text,
            };
            inner.files.insert(file.id.clone(), Arc::new(staged));
        }
        StageResult::Staged { file }
    }
}

fn rejected(filename: String, refusal: Refusal) -> StageResult {
    StageResult::Rejected {
        filename,
        error: refusal.text(),
    }
}

/// Nom de base d'un chemin (le chemin entier s'il n'en a pas, ex. « / »).
fn base_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// Contenu d'un chemin déposé ou choisi.
enum Listing {
    NotADirectory,
    /// Fichiers ordinaires de premier niveau, triés par nom.
    Files(Vec<PathBuf>),
    Unreadable,
}

/// Fichiers ordinaires de premier niveau d'un dossier, triés par nom. Les
/// liens sont suivis ; les sous-dossiers et fichiers spéciaux, ignorés.
fn list_directory(path: &Path) -> Listing {
    if !std::fs::metadata(path).is_ok_and(|m| m.is_dir()) {
        return Listing::NotADirectory;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return Listing::Unreadable;
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| std::fs::metadata(path).is_ok_and(|m| m.is_file()))
        .collect();
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Listing::Files(files)
}

/// Lit un fichier ordinaire de 10 Mio au plus : taille lue d'abord, jamais
/// plus de 10 Mio + 1 octet lus. Refus officiels : « File too large »,
/// « Error reading file » (droits, disparu, pas un fichier ordinaire).
fn read_limited(path: &Path) -> Result<Vec<u8>, Refusal> {
    let metadata = std::fs::metadata(path).map_err(|_| Refusal::ErrorReadingFile)?;
    if !metadata.is_file() {
        return Err(Refusal::ErrorReadingFile);
    }
    if metadata.len() > MAX_FILE_SIZE {
        return Err(Refusal::FileTooLarge);
    }
    // Le type est revérifié sur le fichier ouvert (voir `open_for_reading`).
    let file = open_for_reading(path).map_err(|_| Refusal::ErrorReadingFile)?;
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Err(Refusal::ErrorReadingFile);
    }
    read_at_most(file)
}

/// Ouvre un fichier en lecture. O_NONBLOCK : un tube nommé mis à la place du
/// fichier entre-temps ne bloque pas l'ouverture (sans effet sur la lecture
/// d'un fichier ordinaire).
#[cfg(target_os = "linux")]
fn open_for_reading(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

/// Ouvre un fichier en lecture. Sous Windows, `is_file()` est vrai pour tout
/// ce qui n'est ni un dossier ni un lien, donc aussi pour un périphérique ou
/// un tube (`NUL`, `COM1`, `\\.\pipe\…`), dont la lecture ne finirait pas :
/// seul un fichier sur disque est accepté.
#[cfg(windows)]
fn open_for_reading(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{FILE_TYPE_DISK, GetFileType};

    let file = std::fs::File::open(path)?;
    // SAFETY : poignée ouverte, valide pendant l'appel.
    if unsafe { GetFileType(HANDLE(file.as_raw_handle())) } != FILE_TYPE_DISK {
        return Err(std::io::Error::other("not a file on disk"));
    }
    Ok(file)
}

/// Lit au plus 10 Mio + 1 octet : au-delà, « File too large ».
fn read_at_most(reader: impl Read) -> Result<Vec<u8>, Refusal> {
    let mut data = Vec::new();
    reader
        .take(MAX_FILE_SIZE + 1)
        .read_to_end(&mut data)
        .map_err(|_| Refusal::ErrorReadingFile)?;
    if data.len() as u64 > MAX_FILE_SIZE {
        return Err(Refusal::FileTooLarge);
    }
    Ok(data)
}

#[cfg(test)]
pub mod tests;

/// Fichiers d'essai générés (PDF, PNG), partagés avec les tests
/// d'intégration.
#[cfg(test)]
#[path = "../../tests/it/samples.rs"]
pub mod samples;
