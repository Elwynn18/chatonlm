//! Processus enfant d'extraction de PDF (voir `pdf`) : le binaire de l'app
//! lancé avec [`CHILD_ARG`], traité au tout début de `main`, avant toute
//! initialisation de Tauri ou de GTK. Il lit le PDF sur son entrée standard,
//! écrit le texte (pages assemblées comme l'officiel) sur sa sortie standard
//! et finit par 0 ; en cas d'échec, il y écrit à la place un message court et
//! finit par [`FAILED`]. Sa sortie d'erreur n'est pas lue. Avant de lire le
//! PDF, il se limite : pas de vidage mémoire, mémoire et temps de processeur
//! bornés (limites du noyau sous Linux, Job Object sous Windows : voir
//! `pdf_child/linux.rs` et `pdf_child/windows.rs`).
//!
//! Chaque système fournit, sous le nom `os` :
//! - `confine(cpu_seconds) -> io::Result<()>` : pose les limites ;
//! - `private_stdout() -> io::Result<File>` : la sortie du résultat, la sortie
//!   standard étant détournée vers la sortie d'erreur ;
//! - `limits_report() -> String` (builds de débogage) : les limites telles
//!   que le système les voit, pour les essais.

#[cfg(target_os = "linux")]
#[path = "pdf_child/linux.rs"]
mod os;
#[cfg(windows)]
#[path = "pdf_child/windows.rs"]
mod os;

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};

use super::MAX_FILE_SIZE;
use super::pdf::join_pages;

/// Argument réservé qui fait de l'app un extracteur de PDF.
pub const CHILD_ARG: &str = "--chatonlm-extract-pdf";

/// Code de sortie d'une extraction en échec (raison sur la sortie standard).
pub const FAILED: i32 = 1;

/// Mémoire maximale de l'enfant : espace d'adressage sous Linux (RLIMIT_AS),
/// mémoire engagée sous Windows (Job Object).
const MEMORY_LIMIT: u64 = 1024 * 1024 * 1024;

/// Temps de processeur maximal de l'enfant, en secondes. Linux (RLIMIT_CPU) :
/// signal SIGXCPU à la limite, SIGKILL une seconde plus tard. Windows (Job
/// Object) : temps passé en mode utilisateur, l'enfant est arrêté à la limite.
const CPU_LIMIT: u64 = 30;

/// Durée en unités de 100 ns, celles des limites de temps d'un Job Object.
#[cfg(any(windows, test))]
fn job_time_ticks(seconds: u64) -> i64 {
    i64::try_from(seconds.saturating_mul(10_000_000)).unwrap_or(i64::MAX)
}

/// Si l'app est lancée en extracteur de PDF, fait l'extraction et renvoie le
/// code de sortie ; sinon `None`.
pub fn run_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new(CHILD_ARG)) {
        return None;
    }
    Some(run(&args.collect::<Vec<_>>()))
}

fn run(args: &[OsString]) -> i32 {
    let test = TestMode::parse(args);
    if let Err(error) = os::confine(test.cpu_seconds().unwrap_or(CPU_LIMIT)) {
        return fail(
            &mut io::stdout(),
            &format!("could not limit the PDF reader: {error}"),
        );
    }
    let mut output = match os::private_stdout() {
        Ok(output) => output,
        Err(error) => {
            return fail(
                &mut io::stdout(),
                &format!("could not set up the PDF reader: {error}"),
            );
        }
    };
    let text = match extract(&test, &mut output) {
        Ok(text) => text,
        Err(message) => return fail(&mut output, &message),
    };
    match output
        .write_all(text.as_bytes())
        .and_then(|()| output.flush())
    {
        Ok(()) => 0,
        Err(_) => FAILED,
    }
}

/// Écrit la raison de l'échec à la place du texte.
fn fail(output: &mut impl Write, message: &str) -> i32 {
    let _ = output.write_all(message.as_bytes());
    let _ = output.flush();
    FAILED
}

fn extract(test: &TestMode, output: &mut File) -> Result<String, String> {
    test.before_reading(output)?;

    let mut data = Vec::new();
    io::stdin()
        .lock()
        .take(MAX_FILE_SIZE + 1)
        .read_to_end(&mut data)
        .map_err(|error| format!("could not read the PDF: {error}"))?;
    if data.len() as u64 > MAX_FILE_SIZE {
        return Err("the PDF is too large".into());
    }

    // Une panique de la bibliothèque devient un échec ordinaire.
    std::panic::set_hook(Box::new(|_| {}));
    let pages = std::panic::catch_unwind(|| {
        test.while_reading();
        pdf_pages(&data)
    })
    .map_err(|panic| {
        let message = panic
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        format!("the PDF library crashed: {message}")
    })??;
    Ok(join_pages(&pages))
}

/// Texte de chaque page, dans l'ordre (vide pour une page sans texte, en
/// erreur ou qui fait paniquer la bibliothèque : l'officiel saute une page en
/// erreur) ; erreur si le document ne s'ouvre pas (message préfixé comme
/// celui de l'officiel).
fn pdf_pages(data: &[u8]) -> Result<Vec<String>, String> {
    use oxidize_pdf::parser::{PdfDocument, PdfReader};

    let opened = |error: &dyn std::fmt::Display| format!("failed to create PDF reader: {error}");
    let reader =
        PdfReader::new(std::io::Cursor::new(data.to_vec())).map_err(|error| opened(&error))?;
    let document = PdfDocument::new(reader);
    let count = document.page_count().map_err(|error| opened(&error))?;
    Ok((0..count)
        .map(|index| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                document.extract_text_from_page(index)
            }))
            .ok()
            .and_then(Result::ok)
            .map(|page| page.text)
            .unwrap_or_default()
        })
        .collect())
}

/// Comportements de l'enfant réservés aux tests (builds de débogage
/// seulement) : `--chatonlm-test=<mode>` et `--chatonlm-test-cpu=<secondes>`.
#[cfg(debug_assertions)]
#[derive(Default)]
struct TestMode {
    mode: Option<String>,
    cpu: Option<u64>,
}

#[cfg(debug_assertions)]
impl TestMode {
    fn parse(args: &[OsString]) -> Self {
        let mut test = Self::default();
        for arg in args.iter().filter_map(|arg| arg.to_str()) {
            if let Some(mode) = arg.strip_prefix("--chatonlm-test=") {
                test.mode = Some(mode.to_owned());
            } else if let Some(cpu) = arg.strip_prefix("--chatonlm-test-cpu=") {
                test.cpu = cpu.parse().ok();
            }
        }
        test
    }

    fn cpu_seconds(&self) -> Option<u64> {
        self.cpu
    }

    /// Avant la lecture du PDF : boucles, mémoire, plantage, flot de sortie.
    fn before_reading(&self, output: &mut File) -> Result<(), String> {
        match self.mode.as_deref() {
            // Ni processeur ni sortie : seul le délai mural l'arrête.
            Some("sleep") => std::thread::sleep(std::time::Duration::from_secs(3600)),
            // Boucle de calcul : arrêtée par la limite de temps de processeur.
            Some("spin") => loop {
                std::hint::black_box(0u64);
            },
            // 2 Gio réservés SANS y toucher (aucune page occupée, même si la
            // limite manquait) : refusés par la limite de mémoire.
            Some("alloc") => {
                let mut memory: Vec<u8> = Vec::new();
                memory
                    .try_reserve_exact(2 * 1024 * 1024 * 1024)
                    .map_err(|_| "out of memory".to_owned())?;
                let _ = output.write_all(b"2 GiB reserved");
                std::process::exit(0);
            }
            Some("abort") => std::process::abort(),
            // Limites posées, telles que le système les voit.
            Some("limits") => {
                let _ = output.write_all(os::limits_report().as_bytes());
                std::process::exit(0);
            }
            // 8 Mio de texte : au-delà du plafond choisi par le test (fini,
            // pour que le test reste borné en mémoire même sans plafond).
            Some("flood") => {
                for _ in 0..128 {
                    if output.write_all(&[b'x'; 65536]).is_err() {
                        std::process::exit(FAILED);
                    }
                }
                std::process::exit(0);
            }
            // Avertissements sur la sortie standard, plus qu'un tube n'en
            // contient : ils ne doivent ni bloquer l'enfant ni se mêler au
            // texte ou au message d'erreur.
            Some("noisy") => {
                for line in 0..20_000 {
                    println!("warning {line}: this goes to the error output");
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Pendant l'extraction : panique de la bibliothèque.
    fn while_reading(&self) {
        if self.mode.as_deref() == Some("panic") {
            panic!("simulated failure");
        }
    }
}

#[cfg(not(debug_assertions))]
struct TestMode;

#[cfg(not(debug_assertions))]
impl TestMode {
    fn parse(_args: &[OsString]) -> Self {
        Self
    }

    fn cpu_seconds(&self) -> Option<u64> {
        None
    }

    fn before_reading(&self, _output: &mut File) -> Result<(), String> {
        Ok(())
    }

    fn while_reading(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_time_limits_are_in_units_of_100_ns() {
        assert_eq!(job_time_ticks(0), 0);
        assert_eq!(job_time_ticks(1), 10_000_000);
        assert_eq!(job_time_ticks(CPU_LIMIT), 300_000_000);
        // Jamais de débordement : une durée absurde donne la plus grande.
        assert_eq!(job_time_ticks(u64::MAX), i64::MAX);
    }
}
