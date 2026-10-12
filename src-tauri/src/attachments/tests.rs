//! Tests de la zone de préparation, de l'extraction et du protocole
//! `chatonlm://`. L'extracteur de PDF est simulé en mémoire (`fake_extractor`,
//! sur tout système) ; le dialogue avec un processus enfant (entrée, sortie,
//! délai, plafond, signaux) est essayé sous Linux avec `sh`, et sur tout
//! système avec le vrai binaire (`tests/it/pdf_child.rs`).

use std::ffi::OsString;
use std::io::Read;
#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::pdf::{FakeExtraction, PdfExtractor, join_pages, pdf_text};
use super::protocol::{self, Resource};
use super::*;
use crate::store::Store;
use crate::store::chats;

/// Extracteur de PDF remplacé par un script `sh -c` (`$0` : `arg0`).
#[cfg(target_os = "linux")]
pub fn shell(script: &str, arg0: &str) -> PdfExtractor {
    PdfExtractor {
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".into(), script.into(), arg0.into()],
        timeout: Duration::from_secs(10),
        max_output: pdf::MAX_OUTPUT,
        fake: None,
    }
}

/// Extracteur de PDF simulé en mémoire, sans processus : `run` reçoit le PDF
/// et rend le texte ou la raison d'un échec, après `delay`.
pub fn fake_extractor(
    run: impl Fn(&[u8]) -> Result<String, String> + Send + Sync + 'static,
    delay: Duration,
) -> PdfExtractor {
    PdfExtractor {
        fake: Some(FakeExtraction {
            run: Arc::new(run),
            delay,
        }),
        ..PdfExtractor::app()
    }
}

/// Extracteur simulé qui écrit un fichier dans `traces` à chaque PDF reçu.
fn tracing_extractor(traces: &Path, delay: Duration) -> PdfExtractor {
    let traces = traces.to_owned();
    let count = std::sync::atomic::AtomicUsize::new(0);
    fake_extractor(
        move |_| {
            let number = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::fs::write(traces.join(format!("pdf.{number}")), b"").unwrap();
            Ok(String::new())
        },
        delay,
    )
}

/// Zone dont l'extracteur de PDF renvoie toujours « Text of the PDF » (sans
/// relancer l'app : le binaire des tests n'est pas l'app).
pub fn staging() -> Staging {
    Staging::with_extractor(fake_extractor(
        |_| Ok("Text of the PDF".into()),
        Duration::ZERO,
    ))
}

pub fn staged(result: &StageResult) -> &StagedFile {
    match result {
        StageResult::Staged { file } => file,
        StageResult::Rejected { filename, error } => panic!("{filename} rejected: {error}"),
    }
}

pub fn rejection(result: &StageResult) -> (&str, &str) {
    match result {
        StageResult::Rejected { filename, error } => (filename, error),
        StageResult::Staged { file } => panic!("{} staged", file.filename),
    }
}

/// Refus officiels, en anglais (langue des essais) : textes de l'officiel
/// inchangés.
pub const FILE_TYPE_NOT_SUPPORTED: &str = "File type not supported";
pub const FILE_TOO_LARGE: &str = "File too large";
pub const ERROR_READING_FILE: &str = "Error reading file";

/// Liste officielle (fileValidation.ts), recopiée : `docx` y figure.
const OFFICIAL_TEXT_EXTENSIONS: [&str; 37] = [
    "pdf", "docx", "txt", "md", "csv", "json", "xml", "html", "htm", "js", "jsx", "ts", "tsx",
    "py", "java", "cpp", "c", "cc", "h", "cs", "php", "rb", "go", "rs", "swift", "kt", "scala",
    "sh", "bat", "yaml", "yml", "toml", "ini", "cfg", "conf", "log", "rtf",
];

#[tokio::test]
async fn the_official_extensions_are_accepted_except_docx() {
    let staging = staging();
    for extension in OFFICIAL_TEXT_EXTENSIONS.iter().chain(&IMAGE_EXTENSIONS) {
        for name in [
            format!("file.{extension}"),
            format!("FILE.{}", extension.to_uppercase()),
        ] {
            let result = staging.stage_bytes(name.clone(), b"x".to_vec()).await;
            if *extension == "docx" {
                assert_eq!(rejection(&result), (name.as_str(), FILE_TYPE_NOT_SUPPORTED));
            } else {
                assert_eq!(staged(&result).filename, name);
            }
        }
    }
    // Extension = dernier morceau après un point, en minuscules ; un nom sans
    // point sert entier d'extension, donc refusé, comme l'officiel (sauf un
    // fichier nommé « txt » ou « md »...).
    for name in [
        "Makefile",
        "Dockerfile",
        "",
        "archive.tar.gz",
        "photo.gif",
        "notes.txt.exe",
        "README.",
    ] {
        let result = staging.stage_bytes(name.into(), b"x".to_vec()).await;
        assert_eq!(rejection(&result), (name, FILE_TYPE_NOT_SUPPORTED));
    }
    for name in [".txt", "notes.final.md", "a.b.PNG", "txt"] {
        staged(&staging.stage_bytes(name.into(), b"x".to_vec()).await);
    }
}

/// Le sélecteur natif filtre sur la même liste, dans l'ordre de l'officiel.
#[test]
fn the_picker_offers_the_accepted_extensions() {
    let expected: Vec<&str> = OFFICIAL_TEXT_EXTENSIONS
        .into_iter()
        .filter(|extension| *extension != "docx")
        .chain(["png", "jpg", "jpeg", "webp"])
        .collect();
    assert_eq!(accepted_extensions().collect::<Vec<_>>(), expected);
}

#[tokio::test]
async fn kinds_and_types_come_from_the_extension() {
    let staging = staging();
    for (name, kind, mime) in [
        ("photo.png", AttachmentKind::Image, "image/png"),
        ("photo.JPG", AttachmentKind::Image, "image/jpeg"),
        ("photo.jpeg", AttachmentKind::Image, "image/jpeg"),
        ("photo.webp", AttachmentKind::Image, "image/webp"),
        ("report.pdf", AttachmentKind::Text, "application/pdf"),
        ("data.json", AttachmentKind::Text, "application/json"),
        ("data.csv", AttachmentKind::Text, "text/csv"),
        ("page.htm", AttachmentKind::Text, "text/html"),
        ("notes.md", AttachmentKind::Text, "text/markdown"),
        ("main.rs", AttachmentKind::Text, "text/plain"),
        ("letter.rtf", AttachmentKind::Text, "text/plain"),
    ] {
        let result = staging.stage_bytes(name.into(), b"12345".to_vec()).await;
        let file = staged(&result);
        assert_eq!(
            (file.kind, file.mime.as_str(), file.size),
            (kind, mime, 5),
            "{name}"
        );
        assert!(uuid::Uuid::parse_str(&file.id).is_ok(), "{name}");
    }
}

#[tokio::test]
async fn text_is_extracted_once_like_the_official_app() {
    let staging = staging();
    let text = staging
        .stage_bytes("notes.txt".into(), "Élève, déjà\n".into())
        .await;
    let latin1 = staging
        .stage_bytes("legacy.txt".into(), b"caf\xe9".to_vec())
        .await;
    let image = staging
        .stage_bytes("red.png".into(), samples::png(2, 2, [255, 0, 0]))
        .await;
    let pdf = staging
        .stage_bytes("report.pdf".into(), b"%PDF-1.4".to_vec())
        .await;
    let text_of = |result: &StageResult| staging.get(&staged(result).id).unwrap().text.clone();
    assert_eq!(text_of(&text).as_deref(), Some("Élève, déjà\n"));
    assert_eq!(
        text_of(&latin1).as_deref(),
        Some("[Binary file - 4 bytes - not valid UTF-8]")
    );
    assert_eq!(text_of(&image), None);
    assert_eq!(text_of(&pdf).as_deref(), Some("Text of the PDF"));
    // Le contenu d'origine est gardé tel quel.
    let png = staging.get(&staged(&image).id).unwrap();
    assert_eq!(png.data, samples::png(2, 2, [255, 0, 0]));
}

#[tokio::test]
async fn sizes_are_checked_after_the_type() {
    let staging = staging();
    let limit = usize::try_from(MAX_FILE_SIZE).unwrap();
    let largest = staging
        .stage_bytes("largest.txt".into(), vec![b'a'; limit])
        .await;
    assert_eq!(staged(&largest).size, MAX_FILE_SIZE);
    for (name, error) in [
        ("big.txt", FILE_TOO_LARGE),
        // Le type d'abord, comme l'officiel.
        ("big.exe", FILE_TYPE_NOT_SUPPORTED),
    ] {
        let result = staging
            .stage_bytes(name.into(), vec![b'a'; limit + 1])
            .await;
        assert_eq!(rejection(&result), (name, error));
    }
    // Un PDF trop grand n'est pas extrait : l'extracteur laisserait une trace.
    let dir = temp_dir();
    let tracing = Staging::with_extractor(tracing_extractor(dir.path(), Duration::ZERO));
    let result = tracing
        .stage_bytes("big.pdf".into(), vec![b'a'; limit + 1])
        .await;
    assert_eq!(rejection(&result), ("big.pdf", FILE_TOO_LARGE));
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "un PDF trop grand a été extrait"
    );
}

/// Dossier temporaire dans `.tools/tmp` du projet.
pub fn temp_dir() -> tempfile::TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tools/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::tempdir_in(base).unwrap()
}

#[cfg(target_os = "linux")]
fn make_fifo(path: &Path) {
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY : chemin terminé par un octet nul, valide pendant l'appel.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
}

fn path_string(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

#[tokio::test]
async fn paths_are_read_with_the_official_refusals() {
    let dir = temp_dir();
    let at = |name: &str| dir.path().join(name);
    std::fs::write(at("notes.txt"), "hello").unwrap();
    std::fs::write(at("tool.exe"), "MZ").unwrap();
    // Taille lue avant le contenu : un fichier creux de 10 Mio + 1 octet.
    std::fs::File::create(at("big.log"))
        .unwrap()
        .set_len(MAX_FILE_SIZE + 1)
        .unwrap();
    std::fs::File::create(at("largest.log"))
        .unwrap()
        .set_len(MAX_FILE_SIZE)
        .unwrap();
    #[cfg(target_os = "linux")]
    {
        std::fs::write(at("secret.md"), "no").unwrap();
        std::fs::set_permissions(at("secret.md"), std::fs::Permissions::from_mode(0o000)).unwrap();
        // Un tube nommé n'est pas un fichier ordinaire (et ne bloque pas).
        make_fifo(&at("pipe.txt"));
    }

    let staging = staging();
    let mut paths = vec![
        "notes.txt",
        "tool.exe",
        "big.log",
        "largest.log",
        "gone.txt",
        "gone.exe",
    ];
    // Tube nommé et fichier sans droit de lecture : Linux seulement.
    if cfg!(target_os = "linux") {
        paths.extend(["pipe.txt", "secret.md"]);
    }
    let paths: Vec<String> = paths.iter().map(|name| path_string(&at(name))).collect();
    staging.grant(paths.clone());
    let started = Instant::now();
    let results = staging.stage_paths(paths.clone()).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(results.len(), paths.len());
    let notes = staged(&results[0]);
    assert_eq!((notes.filename.as_str(), notes.size), ("notes.txt", 5));
    assert_eq!(
        staging.get(&notes.id).unwrap().text.as_deref(),
        Some("hello")
    );
    assert_eq!(
        rejection(&results[1]),
        ("tool.exe", FILE_TYPE_NOT_SUPPORTED)
    );
    assert_eq!(rejection(&results[2]), ("big.log", FILE_TOO_LARGE));
    assert_eq!(staged(&results[3]).size, MAX_FILE_SIZE);
    assert_eq!(rejection(&results[4]), ("gone.txt", ERROR_READING_FILE));
    assert_eq!(
        rejection(&results[5]),
        ("gone.exe", FILE_TYPE_NOT_SUPPORTED)
    );
    #[cfg(target_os = "linux")]
    {
        assert_eq!(rejection(&results[6]), ("pipe.txt", ERROR_READING_FILE));
        // Sans droit de lecture (sauf pour root, qui lit tout).
        // SAFETY : geteuid ne lit ni n'écrit de mémoire.
        if unsafe { libc::geteuid() } != 0 {
            assert_eq!(rejection(&results[7]), ("secret.md", ERROR_READING_FILE));
        }
    }
}

/// Sous Windows, un périphérique passe pour un fichier (`is_file()`) : il est
/// refusé d'après le type de la poignée ouverte, sans lecture sans fin.
#[cfg(windows)]
#[test]
fn a_device_is_not_read_as_a_file() {
    for device in ["NUL", r"\\.\NUL"] {
        assert_eq!(
            read_limited(Path::new(device)),
            Err(Refusal::ErrorReadingFile),
            "{device}"
        );
    }
}

#[test]
fn reading_stops_after_the_limit() {
    /// Source qui compte les octets qu'on lui prend. Finie (20 Mio) : même
    /// si la limite disparaissait, le test resterait borné en mémoire.
    struct Counted<'a>(std::io::Take<std::io::Repeat>, &'a mut u64);
    impl Read for Counted<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let read = self.0.read(buffer)?;
            *self.1 += read as u64;
            Ok(read)
        }
    }
    let mut taken = 0;
    let large = Counted(std::io::repeat(b'a').take(2 * MAX_FILE_SIZE), &mut taken);
    assert_eq!(read_at_most(large), Err(Refusal::FileTooLarge));
    // Jamais plus de 10 Mio + 1 octet lus.
    assert_eq!(taken, MAX_FILE_SIZE + 1);
    let limit = usize::try_from(MAX_FILE_SIZE).unwrap();
    let exact = std::io::repeat(b'a').take(MAX_FILE_SIZE);
    assert_eq!(read_at_most(exact).map(|data| data.len()), Ok(limit));
}

#[tokio::test]
async fn a_folder_gives_its_own_files_sorted_by_name() {
    let dir = temp_dir();
    let folder = dir.path().join("project");
    std::fs::create_dir_all(folder.join("sub")).unwrap();
    for (name, text) in [
        ("b.txt", "b"),
        ("a.md", "a"),
        ("C.json", "{}"),
        ("run.exe", "MZ"),
        ("sub/deep.txt", "deep"),
    ] {
        std::fs::write(folder.join(name), text).unwrap();
    }
    // Lien symbolique et tube nommé : Linux seulement (créer un lien sous
    // Windows exige un droit particulier).
    #[cfg(target_os = "linux")]
    {
        std::os::unix::fs::symlink(folder.join("b.txt"), folder.join("link.txt")).unwrap();
        make_fifo(&folder.join("pipe.txt"));
    }
    let notes = dir.path().join("notes.txt");
    std::fs::write(&notes, "notes").unwrap();

    let staging = staging();
    let paths = vec![path_string(&notes), path_string(&folder)];
    staging.grant(paths.clone());
    let results = staging.stage_paths(paths).await;
    let names: Vec<(&str, bool)> = results
        .iter()
        .map(|result| match result {
            StageResult::Staged { file } => (file.filename.as_str(), true),
            StageResult::Rejected { filename, .. } => (filename.as_str(), false),
        })
        .collect();
    // Premier niveau seulement (ni sous-dossier ni tube), liens suivis, par
    // ordre des octets du nom ; les types refusés le sont un par un.
    let mut expected = vec![
        ("notes.txt", true),
        ("C.json", true),
        ("a.md", true),
        ("b.txt", true),
        ("run.exe", false),
    ];
    if cfg!(target_os = "linux") {
        expected.insert(4, ("link.txt", true));
    }
    assert_eq!(names, expected);
    #[cfg(target_os = "linux")]
    {
        let link = staged(&results[4]);
        assert_eq!(staging.get(&link.id).unwrap().text.as_deref(), Some("b"));
    }
}

#[tokio::test]
async fn unstaging_and_taking_files() {
    let staging = staging();
    let mut ids = Vec::new();
    for name in ["a.txt", "b.txt", "c.txt"] {
        let result = staging.stage_bytes(name.into(), name.into()).await;
        ids.push(staged(&result).id.clone());
    }
    // Retrait : sans effet pour un identifiant inconnu.
    staging.unstage(&["unknown".into(), ids[2].clone()]);
    assert!(staging.get(&ids[2]).is_none());
    assert!(staging.get(&ids[0]).is_some());

    // Tous ou aucun : un identifiant inconnu, retiré ou répété refuse tout.
    for wanted in [
        vec![ids[0].clone(), "unknown".into()],
        vec![ids[0].clone(), ids[2].clone()],
        vec![ids[1].clone(), ids[1].clone()],
    ] {
        assert!(matches!(staging.take(&wanted), Err(AppError::Invalid(_))));
        assert!(staging.get(&ids[0]).is_some() && staging.get(&ids[1]).is_some());
    }

    // Dans l'ordre demandé ; remis en place s'ils ne sont pas gardés.
    let order = [ids[1].clone(), ids[0].clone()];
    let taken = staging.take(&order).unwrap();
    let names: Vec<_> = taken
        .files()
        .iter()
        .map(|f| f.file.filename.as_str())
        .collect();
    assert_eq!(names, ["b.txt", "a.txt"]);
    assert!(staging.get(&ids[0]).is_none());
    drop(taken);
    assert!(staging.get(&ids[0]).is_some() && staging.get(&ids[1]).is_some());

    // Gardés : ils ne reviennent pas.
    staging.take(&order).unwrap().keep();
    assert!(staging.get(&ids[0]).is_none() && staging.get(&ids[1]).is_none());
    // Aucun identifiant : rien à prendre.
    assert!(staging.take(&[]).unwrap().files().is_empty());
}

#[tokio::test]
async fn a_reload_empties_the_zone_for_good() {
    let staging = staging();
    let first = staging.stage_bytes("a.txt".into(), b"a".to_vec()).await;
    let second = staging.stage_bytes("b.txt".into(), b"b".to_vec()).await;
    let (first, second) = (staged(&first).id.clone(), staged(&second).id.clone());
    let taken = staging.take(std::slice::from_ref(&second)).unwrap();

    staging.clear();
    assert!(staging.get(&first).is_none());
    // Un envoi refusé après le rechargement ne ramène pas ses fichiers.
    drop(taken);
    assert!(staging.get(&second).is_none());
    // Un fichier lu pour la page d'avant n'est pas gardé.
    let generation = staging.inner.lock().generation - 1;
    let late = staging
        .keep(generation, "late.txt".into(), b"late".to_vec())
        .await;
    assert!(staging.get(&staged(&late).id).is_none());
    let fresh = staging.stage_bytes("c.txt".into(), b"c".to_vec()).await;
    assert!(staging.get(&staged(&fresh).id).is_some());
}

#[tokio::test]
async fn a_reload_stops_staging_the_remaining_files() {
    // Trois PDF d'un dossier, puis un autre chemin, chacun extrait en 300 ms ;
    // l'extracteur laisse une trace par fichier. Rechargement pendant le
    // premier : les suivants ne sont ni lus ni extraits.
    let dir = temp_dir();
    let folder = dir.path().join("pdfs");
    let traces = dir.path().join("traces");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(&traces).unwrap();
    for name in ["a.pdf", "b.pdf", "c.pdf"] {
        std::fs::write(folder.join(name), b"%PDF-1.4 fake").unwrap();
    }
    let other = dir.path().join("other.pdf");
    std::fs::write(&other, b"%PDF-1.4 fake").unwrap();
    let staging = Arc::new(Staging::with_extractor(tracing_extractor(
        &traces,
        Duration::from_millis(300),
    )));
    let task = {
        let staging = staging.clone();
        let paths = vec![path_string(&folder), path_string(&other)];
        staging.grant(paths.clone());
        tokio::spawn(async move { staging.stage_paths(paths).await })
    };
    let count = || std::fs::read_dir(&traces).unwrap().count();
    for _ in 0..100 {
        if count() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(count(), 1);
    staging.clear();
    let results = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
    assert!(results.len() <= 1, "{results:?}");
    assert_eq!(count(), 1, "fichiers extraits après le rechargement");
}

/// Refus d'un chemin non remis, ou dont la remise a déjà servi.
fn refused(result: &StageResult, filename: &str) {
    assert_eq!(rejection(result), (filename, ERROR_READING_FILE));
}

#[tokio::test]
async fn only_handed_over_paths_are_read_and_once() {
    let dir = temp_dir();
    let notes = dir.path().join("notes.txt");
    let other = dir.path().join("other.md");
    std::fs::write(&notes, "hello").unwrap();
    std::fs::write(&other, "other").unwrap();
    let folder = dir.path().join("folder");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("a.txt"), "a").unwrap();
    // Extracteur qui laisse une trace : un PDF non remis n'est jamais lu.
    let traces = dir.path().join("traces");
    std::fs::create_dir_all(&traces).unwrap();
    let pdf = dir.path().join("report.pdf");
    std::fs::write(&pdf, b"%PDF-1.4 fake").unwrap();
    let staging = Staging::with_extractor(tracing_extractor(&traces, Duration::ZERO));
    let (notes, other, folder, pdf) = (
        path_string(&notes),
        path_string(&other),
        path_string(&folder),
        path_string(&pdf),
    );

    // Rien de remis : fichiers lisibles, dossier et PDF refusés comme
    // illisibles, sans lecture ni listage ; rien gardé.
    let results = staging
        .stage_paths(vec![notes.clone(), folder.clone(), pdf.clone()])
        .await;
    assert_eq!(results.len(), 3, "{results:?}");
    refused(&results[0], "notes.txt");
    refused(&results[1], "folder");
    refused(&results[2], "report.pdf");
    assert_eq!(std::fs::read_dir(&traces).unwrap().count(), 0);
    assert!(staging.inner.lock().files.is_empty());

    // Remis (dossier compris) : préparés, chacun une fois ; un chemin remis
    // deux fois sert deux fois ; comparaison exacte (pas de « ./ », de « // »
    // ni de lien résolu).
    staging.grant([notes.clone(), folder.clone(), other.clone(), other.clone()]);
    let separator = std::path::MAIN_SEPARATOR;
    let variant = notes.replacen(
        &format!("{separator}notes.txt"),
        &format!("{separator}.{separator}notes.txt"),
        1,
    );
    assert_ne!(variant, notes);
    let results = staging
        .stage_paths(vec![
            variant,
            notes.clone(),
            notes.clone(),
            folder.clone(),
            other.clone(),
            other.clone(),
            other.clone(),
        ])
        .await;
    let summary: Vec<(&str, bool)> = results
        .iter()
        .map(|result| match result {
            StageResult::Staged { file } => (file.filename.as_str(), true),
            StageResult::Rejected { filename, .. } => (filename.as_str(), false),
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("notes.txt", false),
            ("notes.txt", true),
            ("notes.txt", false),
            // Le contenu d'un dossier remis est préparé.
            ("a.txt", true),
            ("other.md", true),
            ("other.md", true),
            ("other.md", false),
        ]
    );
    refused(&results[0], "notes.txt");
    // Remises consommées : un second appel est refusé ; un fichier d'un
    // dossier remis ne l'est pas lui-même.
    let child = path_string(&Path::new(&folder).join("a.txt"));
    let results = staging
        .stage_paths(vec![notes.clone(), folder.clone(), child])
        .await;
    refused(&results[0], "notes.txt");
    refused(&results[1], "folder");
    refused(&results[2], "a.txt");
    assert_eq!(staging.inner.lock().granted.len(), 0);
}

#[tokio::test]
async fn a_reload_forgets_the_handed_over_paths() {
    let dir = temp_dir();
    let notes = path_string(&dir.path().join("notes.txt"));
    std::fs::write(&notes, "hello").unwrap();
    let staging = staging();
    staging.grant([notes.clone()]);
    staging.clear();
    refused(
        &staging.stage_paths(vec![notes.clone()]).await[0],
        "notes.txt",
    );

    // Sélecteur ouvert par la page d'avant : ses chemins ne sont pas remis.
    let before = staging.generation();
    staging.clear();
    staging.grant_if_current(before, std::slice::from_ref(&notes));
    refused(
        &staging.stage_paths(vec![notes.clone()]).await[0],
        "notes.txt",
    );
    staging.grant_if_current(staging.generation(), std::slice::from_ref(&notes));
    staged(&staging.stage_paths(vec![notes]).await[0]);
}

#[tokio::test]
async fn the_handed_over_paths_are_bounded() {
    let dir = temp_dir();
    let first = path_string(&dir.path().join("first.txt"));
    let second = path_string(&dir.path().join("second.txt"));
    std::fs::write(&first, "1").unwrap();
    std::fs::write(&second, "2").unwrap();
    let staging = staging();
    // Au-delà de la borne, les plus anciennes remises sortent.
    staging.grant([first.clone(), second.clone()]);
    staging.grant((0..MAX_GRANTED - 1).map(|n| format!("/never/staged/{n}.txt")));
    assert_eq!(staging.inner.lock().granted.len(), MAX_GRANTED);
    let results = staging.stage_paths(vec![first, second]).await;
    refused(&results[0], "first.txt");
    staged(&results[1]);
    // Un chemin remis plusieurs fois sort remise par remise.
    let staging = self::staging();
    let notes = path_string(&dir.path().join("first.txt"));
    staging.grant([notes.clone(), notes.clone()]);
    staging.grant((0..MAX_GRANTED - 1).map(|n| format!("/never/staged/{n}.txt")));
    let granted = staging.inner.lock().granted.len();
    assert_eq!(granted, MAX_GRANTED);
    let results = staging.stage_paths(vec![notes.clone(), notes]).await;
    staged(&results[0]);
    refused(&results[1], "first.txt");
    // Ensemble resté cohérent : la remise consommée en est sortie.
    assert_eq!(staging.inner.lock().granted.len(), MAX_GRANTED - 1);
}

#[test]
fn pages_are_joined_like_the_official_app() {
    let pages = |texts: &[&str]| texts.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
    assert_eq!(join_pages(&pages(&["One"])), "One");
    // Pages vides ou blanches sautées ; numéro réel ; aucun marqueur avant
    // la première page écrite, même si ce n'est pas la première.
    assert_eq!(
        join_pages(&pages(&[" \n", "Two", "", "Four\n", "\t"])),
        "Two\n\n--- Page 4 ---\nFour\n"
    );
    assert_eq!(join_pages(&pages(&["", " "])), "");
    assert_eq!(join_pages(&[]), "");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn pdf_text_uses_the_official_placeholders() {
    let data = b"%PDF-1.7 fake";
    for (script, expected) in [
        ("cat >/dev/null; printf 'Page one'", "Page one".to_owned()),
        (
            "cat >/dev/null; printf ' \\n '",
            "[PDF file - 13 bytes - no text content found]".to_owned(),
        ),
        (
            "cat >/dev/null; printf 'the PDF is encrypted'; exit 1",
            "[PDF file - 13 bytes - failed to extract text: the PDF is encrypted]".to_owned(),
        ),
    ] {
        assert_eq!(pdf_text(data, &shell(script, "sh")).await, expected);
    }
}

/// Les mêmes textes officiels, sans processus : sur tout système.
#[tokio::test]
async fn pdf_text_uses_the_official_placeholders_without_a_process() {
    let data = b"%PDF-1.7 fake";
    let fake = |result: Result<&'static str, &'static str>| {
        fake_extractor(
            move |_| result.map(str::to_owned).map_err(str::to_owned),
            Duration::ZERO,
        )
    };
    for (result, expected) in [
        (Ok("Page one"), "Page one"),
        (Ok(" \n "), "[PDF file - 13 bytes - no text content found]"),
        (
            Err("the PDF is encrypted"),
            "[PDF file - 13 bytes - failed to extract text: the PDF is encrypted]",
        ),
    ] {
        assert_eq!(pdf_text(data, &fake(result)).await, expected);
    }
}

/// Sous Windows, la fin anormale de l'enfant se lit dans son code de sortie.
#[test]
fn windows_exit_codes_tell_crashes_from_failures() {
    use super::pdf::windows_end;
    assert_eq!(
        windows_end(1816).as_deref(),
        Some("the PDF reader used too much processor time")
    );
    // Codes NTSTATUS d'erreur : `abort` (échec rapide), violation d'accès.
    assert_eq!(
        windows_end(0xC000_0044_u32 as i32).as_deref(),
        Some("the PDF reader used too much processor time")
    );
    assert_eq!(
        windows_end(0xC000_0409_u32 as i32).as_deref(),
        Some("the PDF reader crashed (code 0xc0000409)")
    );
    assert_eq!(
        windows_end(0xC000_0005_u32 as i32).as_deref(),
        Some("the PDF reader crashed (code 0xc0000005)")
    );
    // Codes de sortie ordinaires : dits par l'appelant.
    for code in [0, 1, 3, 255] {
        assert_eq!(windows_end(code), None, "{code}");
    }
}

/// Le processus est-il encore là (même zombie) ?
#[cfg(target_os = "linux")]
fn alive(pid: &str) -> bool {
    Path::new("/proc").join(pid.trim()).exists()
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_reader_is_fed_and_its_failures_are_told_apart() {
    // Entrée lue en entier (bien plus qu'un tube n'en contient), puis fermée.
    let input = vec![b'x'; 3 * 1024 * 1024];
    let counted = shell("wc -c", "sh").extract(&input).await;
    assert_eq!(counted.unwrap().trim(), "3145728");
    // Sortie d'erreur non lue : abondante, elle ne bloque pas l'enfant.
    let noisy = shell(
        "cat >/dev/null; yes warning | head -c 1000000 >&2; printf done",
        "sh",
    );
    assert_eq!(noisy.extract(b"%PDF").await.unwrap(), "done");

    for (script, expected) in [
        ("echo 'bad xref'; exit 1", "bad xref"),
        // Sortie d'erreur ignorée.
        (
            "echo 'not this' >&2; exit 1",
            "the PDF reader failed (exit code 1)",
        ),
        ("echo 'odd'; exit 3", "the PDF reader failed (exit code 3)"),
        ("kill -TERM $$", "the PDF reader crashed (signal 15)"),
    ] {
        let failure = shell(script, "sh").extract(b"%PDF").await.unwrap_err();
        assert_eq!(failure, expected, "{script}");
    }
    // Raison donnée par l'enfant : écourtée.
    let long = shell("printf '%0600d' 0; exit 1", "sh")
        .extract(b"%PDF")
        .await;
    assert_eq!(long.unwrap_err().len(), 500);
    let missing = PdfExtractor {
        program: PathBuf::from("/nonexistent/chatonlm"),
        ..shell("", "")
    };
    let failure = missing.extract(b"%PDF").await.unwrap_err();
    assert!(
        failure.starts_with("could not start the PDF reader: "),
        "{failure}"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_reader_that_hangs_is_killed_and_reaped() {
    let dir = temp_dir();
    let pid_file = dir.path().join("pid");
    let mut hanging = shell(
        "echo $$ > \"$0\"; exec sleep 30",
        pid_file.to_str().unwrap(),
    );
    hanging.timeout = Duration::from_millis(500);
    let started = Instant::now();
    let failure = hanging.extract(b"%PDF").await.unwrap_err();
    assert_eq!(failure, "the PDF reader took more than 0.5 s");
    assert!(started.elapsed() < Duration::from_secs(3));
    // Tué et attendu : ni vivant ni zombie.
    let pid = std::fs::read_to_string(&pid_file).unwrap();
    assert!(!alive(&pid), "process {pid} is still there");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn too_much_text_stops_the_reader() {
    // 1 Mo de texte puis une longue attente : arrêté dès le plafond (100 ko)
    // passé, sans attendre la fin. Fini, le texte garde le test borné en
    // mémoire même sans plafond (le délai l'arrêterait alors).
    let dir = temp_dir();
    let pid_file = dir.path().join("pid");
    let mut flooding = shell(
        "echo $$ > \"$0\"; head -c 1000000 /dev/zero; exec sleep 30",
        pid_file.to_str().unwrap(),
    );
    flooding.max_output = 100_000;
    flooding.timeout = Duration::from_secs(5);
    let started = Instant::now();
    let failure = flooding.extract(b"%PDF").await.unwrap_err();
    assert_eq!(failure, "the extracted text is too large");
    assert!(started.elapsed() < Duration::from_secs(3));
    let pid = std::fs::read_to_string(&pid_file).unwrap();
    assert!(!alive(&pid), "process {pid} is still there");
    // Juste au plafond : accepté.
    let mut exact = shell("cat >/dev/null; head -c 100000 /dev/zero", "sh");
    exact.max_output = 100_000;
    assert_eq!(exact.extract(b"%PDF").await.unwrap().len(), 100_000);
}

#[test]
fn the_app_relaunches_itself_with_the_reserved_argument() {
    let app = PdfExtractor::app();
    let program = if cfg!(windows) {
        std::env::current_exe().unwrap()
    } else {
        PathBuf::from("/proc/self/exe")
    };
    assert_eq!(app.program, program);
    assert_eq!(app.args, [OsString::from(pdf_child::CHILD_ARG)]);
    assert_eq!(app.timeout, Duration::from_secs(30));
    assert_eq!(app.max_output, 64 * 1024 * 1024);
}

#[test]
fn protocol_addresses() {
    let uuid = "0199b0c4-9a43-7c1e-8b5d-1f2e3d4c5b6a";
    for (uri, expected) in [
        (
            "chatonlm://localhost/attachment/12",
            Some(Resource::Attachment(12)),
        ),
        (
            "chatonlm://localhost/attachment/12?v=2",
            Some(Resource::Attachment(12)),
        ),
        (
            &format!("chatonlm://localhost/staged/{uuid}") as &str,
            Some(Resource::Staged(uuid.into())),
        ),
        ("chatonlm://localhost/attachment/", None),
        ("chatonlm://localhost/attachment/-1", None),
        ("chatonlm://localhost/attachment/1x", None),
        ("chatonlm://localhost/attachment/12/more", None),
        ("chatonlm://localhost/staged/not-a-uuid", None),
        ("chatonlm://localhost/other/12", None),
        ("chatonlm://localhost", None),
        ("http://localhost/attachment/12", None),
    ] {
        assert_eq!(protocol::resource(uri), expected, "{uri}");
    }
}

#[tokio::test]
async fn the_protocol_serves_images_only() {
    let store = Store::open_in_memory().unwrap();
    let staging = staging();
    let red = samples::png(2, 2, [255, 0, 0]);
    let image = staging.stage_bytes("red.png".into(), red.clone()).await;
    let text = staging
        .stage_bytes("notes.txt".into(), b"hi".to_vec())
        .await;
    let other = staging
        .stage_bytes("other.txt".into(), b"hi".to_vec())
        .await;
    let (image, text) = (staged(&image).id.clone(), staged(&text).id.clone());
    let other = staged(&other).id.clone();

    // Fichier préparé : jamais mis en cache (il peut disparaître).
    let response = protocol::handle(
        Some(&store),
        &staging,
        &format!("chatonlm://localhost/staged/{image}"),
    )
    .await;
    assert_eq!(response.status(), 200);
    let headers = response.headers();
    assert_eq!(headers["content-type"], "image/png");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(response.body(), &red);

    // Pièce jointe enregistrée.
    let saved = staging.take(&[image.clone(), text.clone()]).unwrap();
    let message = store
        .blocking(|c| {
            let chat = chats::create(c, 1)?;
            let files: Vec<_> = saved.files().iter().map(|f| f.as_new()).collect();
            chats::add_user_message(c, &chat, "Look", &files, 2)
        })
        .unwrap();
    saved.keep();
    let (image_id, text_id) = (message.attachments[0].id, message.attachments[1].id);
    let response = protocol::handle(
        Some(&store),
        &staging,
        &format!("chatonlm://localhost/attachment/{image_id}"),
    )
    .await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "image/png");
    assert!(response.headers().get("cache-control").is_none());
    assert_eq!(response.body(), &red);

    // 404 : texte, inconnu, plus préparé, base absente, adresse invalide.
    for (uri, store) in [
        (
            format!("chatonlm://localhost/attachment/{text_id}"),
            Some(&store),
        ),
        (
            "chatonlm://localhost/attachment/999".to_owned(),
            Some(&store),
        ),
        (format!("chatonlm://localhost/staged/{image}"), Some(&store)),
        (format!("chatonlm://localhost/staged/{other}"), Some(&store)),
        (format!("chatonlm://localhost/attachment/{image_id}"), None),
        ("chatonlm://localhost/nothing".to_owned(), Some(&store)),
    ] {
        let response = protocol::handle(store, &staging, &uri).await;
        assert_eq!(response.status(), 404, "{uri}");
        assert!(response.body().is_empty(), "{uri}");
    }
}
