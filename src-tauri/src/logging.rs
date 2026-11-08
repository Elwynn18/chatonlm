//! Journal de l'app (propre à ChatonLM) : `<données>/logs/chatonlm.log`, le
//! précédent en `.1` dès que le journal dépasse [`MAX_SIZE`]. Il garde ce que
//! le moteur écrivait seulement sur la sortie d'erreur (`app_log!` y écrit
//! toujours), que personne ne voit sous Windows (`windows_subsystem`) ni
//! depuis le menu du bureau, et les plantages (crochet de panique).
//!
//! Vie privée : jamais le texte d'une conversation, d'une dictée ou d'une
//! correction, seulement des états et des erreurs. Rien ne quitte la machine,
//! sauf le diagnostic que l'utilisateur envoie lui-même (`bug_report.rs`), où
//! le dossier personnel est masqué.
//!
//! Le journal s'allonge d'un lancement à l'autre : le diagnostic montre ainsi
//! la fin du lancement qui a planté.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;

/// Au-delà, le journal devient `chatonlm.log.1` (l'ancien `.1` est perdu) :
/// deux Mio au plus sur le disque.
pub const MAX_SIZE: u64 = 1024 * 1024;

/// Nom du journal dans `<données>/logs`.
pub const FILE_NAME: &str = "chatonlm.log";

/// Trace d'appels d'un plantage gardée au plus (en caractères).
const BACKTRACE_MAX: usize = 6000;

static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();

struct Sink {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    max: u64,
}

impl Sink {
    fn open(path: PathBuf, max: u64) -> Self {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        let size = file
            .as_ref()
            .and_then(|file| file.metadata().ok())
            .map_or(0, |metadata| metadata.len());
        Sink {
            path,
            file,
            size,
            max,
        }
    }

    fn write(&mut self, line: &str) {
        if self.size > 0 && self.size + line.len() as u64 > self.max {
            self.rotate();
        }
        if let Some(file) = &mut self.file
            && file.write_all(line.as_bytes()).is_ok()
        {
            self.size += line.len() as u64;
        }
    }

    /// Le journal devient `.1` : fermé avant d'être renommé (Windows).
    fn rotate(&mut self) {
        self.file = None;
        let _ = std::fs::rename(&self.path, rotated(&self.path));
        *self = Sink::open(self.path.clone(), self.max);
    }
}

/// `chatonlm.log` → `chatonlm.log.1`.
fn rotated(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

/// Ouvre le journal dans `logs_dir` (créé au besoin), y note le lancement et
/// pose le crochet de panique. Appelé une fois, dès que le dossier des
/// données est connu ; un second appel ne change rien. Un journal impossible
/// à ouvrir n'empêche pas l'app de démarrer.
pub fn init(logs_dir: &Path) {
    let _ = std::fs::create_dir_all(logs_dir);
    let fresh = SINK
        .set(Mutex::new(Sink::open(logs_dir.join(FILE_NAME), MAX_SIZE)))
        .is_ok();
    if !fresh {
        return;
    }
    write(&format!(
        "=== ChatonLM {} started ({} {}) ===",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    install_panic_hook();
}

/// Écrit une ligne sur la sortie d'erreur et, une fois [`init`] fait, dans le
/// journal, précédée de l'heure (UTC).
pub fn write(message: &str) {
    eprintln!("{message}");
    if let Some(sink) = SINK.get() {
        let line = format!("{} {message}\n", timestamp(SystemTime::now()));
        sink.lock().write(&line);
    }
}

/// Ligne du journal : `app_log!("chatonlm: …", …)` remplace `eprintln!`.
#[macro_export]
macro_rules! app_log {
    ($($arg:tt)*) => {
        $crate::logging::write(&format!($($arg)*))
    };
}

/// Plantage : message, endroit et trace d'appels dans le journal, puis le
/// crochet d'origine (sortie d'erreur).
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed");
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        let backtrace: String = backtrace.chars().take(BACKTRACE_MAX).collect();
        if let Some(sink) = SINK.get() {
            let line = format!(
                "{} PANIC in thread '{name}': {info}\n{backtrace}\n",
                timestamp(SystemTime::now())
            );
            sink.lock().write(&line);
        }
        previous(info);
    }));
}

/// Chemin du journal, une fois [`init`] fait.
pub fn path() -> Option<PathBuf> {
    SINK.get().map(|sink| sink.lock().path.clone())
}

/// Les `max` derniers octets du journal (et de `.1` s'il le faut), coupés au
/// début d'une ligne. Vide si rien n'est écrit.
pub fn tail(path: &Path, max: u64) -> String {
    let (current, whole) = read_end(path, max);
    let missing = max.saturating_sub(current.len() as u64);
    let (mut text, cut) = if !whole {
        (Vec::new(), true)
    } else if missing > 0 {
        let (previous, whole) = read_end(&rotated(path), missing);
        (previous, !whole)
    } else {
        // Le journal courant, lu en entier, remplit tout : il commence par une
        // ligne entière, le précédent n'est pas lu.
        (Vec::new(), false)
    };
    text.extend_from_slice(&current);
    let text = String::from_utf8_lossy(&text).into_owned();
    if !cut {
        return text;
    }
    // Première ligne entamée par la coupe : retirée.
    match text.find('\n') {
        Some(end) => text[end + 1..].to_owned(),
        None => text,
    }
}

/// Les `max` derniers octets de `path`, et s'ils sont tout le fichier (vrai
/// pour un fichier absent).
fn read_end(path: &Path, max: u64) -> (Vec<u8>, bool) {
    let Ok(mut file) = File::open(path) else {
        return (Vec::new(), true);
    };
    let len = file.metadata().map_or(0, |metadata| metadata.len());
    let start = len.saturating_sub(max);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_ok() {
        let _ = file.take(max).read_to_end(&mut bytes);
    }
    (bytes, start == 0)
}

/// `2026-11-08T14:03:07Z`.
fn timestamp(at: SystemTime) -> String {
    let secs = at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Jours depuis le 1er janvier 1970 → (année, mois, jour), algorithme de
/// Howard Hinnant (calendrier grégorien proleptique).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn timestamps_are_utc_dates() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        // 29 février d'une année bissextile, puis fin d'année.
        let leap = UNIX_EPOCH + Duration::from_secs(1_709_210_096);
        assert_eq!(timestamp(leap), "2024-02-29T12:34:56Z");
        let end = UNIX_EPOCH + Duration::from_secs(1_798_761_599);
        assert_eq!(timestamp(end), "2026-12-31T23:59:59Z");
    }

    #[test]
    fn a_full_log_becomes_the_previous_one() {
        let dir = dir();
        let path = dir.path().join(FILE_NAME);
        let mut sink = Sink::open(path.clone(), 30);
        sink.write("first line, 20 chars\n");
        sink.write("second line\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second line\n");
        assert_eq!(
            std::fs::read_to_string(rotated(&path)).unwrap(),
            "first line, 20 chars\n"
        );
        // Encore plein : l'ancien `.1` est remplacé.
        sink.write("third line, long enough\n");
        assert_eq!(
            std::fs::read_to_string(rotated(&path)).unwrap(),
            "second line\n"
        );
    }

    #[test]
    fn the_log_grows_across_launches() {
        let dir = dir();
        let path = dir.path().join(FILE_NAME);
        Sink::open(path.clone(), MAX_SIZE).write("launch 1\n");
        Sink::open(path.clone(), MAX_SIZE).write("launch 2\n");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "launch 1\nlaunch 2\n"
        );
    }

    #[test]
    fn the_tail_starts_at_a_line_and_reaches_into_the_previous_log() {
        let dir = dir();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(rotated(&path), "old a\nold b\n").unwrap();
        std::fs::write(&path, "new c\n").unwrap();
        // Tout tient.
        assert_eq!(tail(&path, 100), "old a\nold b\nnew c\n");
        // Coupé dans « old a » : la ligne entamée est retirée.
        assert_eq!(tail(&path, 15), "old b\nnew c\n");
        // Seulement le journal courant.
        assert_eq!(tail(&path, 6), "new c\n");
        assert_eq!(tail(&dir.path().join("absent.log"), 100), "");
    }
}
