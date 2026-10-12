// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Translated to Rust from Ollama (https://github.com/ollama/ollama),
// file app/ui/extract.go, commit 42e911bc3d05 (PDF text, page markers and
// messages).
// Modified for ChatonLM: another PDF library (oxidize-pdf), run in a child
// process with time, memory and output limits (kernel limits on Linux, a Job
// Object on Windows).

//! Texte d'un PDF, extrait dans un processus enfant : le binaire de l'app
//! relancé avec un argument réservé (voir `pdf_child`), le PDF sur son entrée
//! standard, le texte (ou la raison d'un échec) sur sa sortie. Un PDF piégé
//! (boucle, mémoire, plantage de la bibliothèque) ne peut ainsi ni figer ni
//! faire tomber l'app : délai mural, limites du système dans l'enfant, sortie
//! plafonnée ; tout échec donne le texte officiel « failed to extract text ».

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::pdf_child;
use crate::os_util::NoWindow;

/// Délai mural de l'extraction d'un PDF.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// Plafond du texte renvoyé par l'enfant.
pub const MAX_OUTPUT: usize = 64 * 1024 * 1024;

/// Longueur maximale de la raison d'un échec donnée par l'enfant.
const MAX_REASON: usize = 500;

/// Programme d'extraction : l'app elle-même en temps normal ; un autre
/// programme (ou d'autres limites) dans les tests.
#[derive(Clone, Debug)]
pub struct PdfExtractor {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub timeout: Duration,
    pub max_output: usize,
    /// Essais : extraction faite en mémoire, sans processus enfant.
    #[cfg(test)]
    pub fake: Option<FakeExtraction>,
}

/// Extraction simulée des essais (sur tout système, sans `sh`) : `run` reçoit
/// le PDF et rend le texte ou la raison d'un échec, comme l'enfant ; le
/// résultat n'arrive qu'après `delay`.
#[cfg(test)]
#[derive(Clone)]
pub struct FakeExtraction {
    pub run: std::sync::Arc<FakeRun>,
    pub delay: Duration,
}

/// Ce que fait une extraction simulée d'un PDF reçu.
#[cfg(test)]
pub type FakeRun = dyn Fn(&[u8]) -> Result<String, String> + Send + Sync;

#[cfg(test)]
impl std::fmt::Debug for FakeExtraction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "FakeExtraction({:?})", self.delay)
    }
}

impl PdfExtractor {
    /// Le binaire en cours d'exécution, par `/proc/self/exe` : la même version
    /// que l'app, même si le paquet a été mis à jour entre-temps. Sous
    /// Windows, le chemin de l'exécutable (qui ne peut pas être remplacé tant
    /// que l'app tourne).
    pub fn app() -> Self {
        Self {
            program: app_program(),
            args: vec![pdf_child::CHILD_ARG.into()],
            timeout: TIMEOUT,
            max_output: MAX_OUTPUT,
            #[cfg(test)]
            fake: None,
        }
    }

    /// Texte du PDF (pages assemblées par l'enfant), ou la raison de l'échec.
    pub async fn extract(&self, data: &[u8]) -> Result<String, String> {
        #[cfg(test)]
        {
            if let Some(fake) = &self.fake {
                let result = (fake.run)(data);
                tokio::time::sleep(fake.delay).await;
                return result;
            }
        }
        // Sortie d'erreur jetée : seuls les avertissements d'une
        // bibliothèque y passeraient.
        let mut child = Command::new(&self.program)
            .no_window()
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("could not start the PDF reader: {error}"))?;
        let (Some(mut stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill().await;
            return Err("could not talk to the PDF reader".into());
        };
        let max_output = self.max_output;
        let work = async {
            let write = async {
                // L'enfant peut finir sans tout lire : erreur ignorée.
                let _ = stdin.write_all(data).await;
                drop(stdin);
                Ok(())
            };
            let output = async {
                let (bytes, complete) = read_capped(&mut stdout, max_output).await;
                if complete {
                    Ok(bytes)
                } else {
                    Err("the extracted text is too large".to_owned())
                }
            };
            let ((), output) = tokio::try_join!(write, output)?;
            let status = child.wait().await.map_err(|error| error.to_string())?;
            Ok::<_, String>((status, output))
        };
        let outcome = tokio::time::timeout(self.timeout, work).await;
        // Délai dépassé ou sortie trop longue : l'enfant est tué puis attendu
        // (aucun processus zombie). Sans effet s'il est déjà fini.
        let _ = child.kill().await;
        let (status, output) = match outcome {
            Ok(Ok(finished)) => finished,
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Err(format!(
                    "the PDF reader took more than {} s",
                    self.timeout.as_secs_f32()
                ));
            }
        };
        let output = String::from_utf8_lossy(&output).into_owned();
        if status.success() {
            return Ok(output);
        }
        Err(failure(status, &output))
    }
}

/// Lit au plus `max` octets ; `false` si le flux allait au-delà (la lecture
/// s'arrête alors). Une erreur de lecture termine la lecture comme une fin de
/// flux.
async fn read_capped(reader: &mut (impl AsyncRead + Unpin), max: usize) -> (Vec<u8>, bool) {
    let mut bytes = Vec::new();
    if reader
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .is_ok()
        && bytes.len() > max
    {
        bytes.truncate(max);
        return (bytes, false);
    }
    (bytes, true)
}

/// Raison d'une fin en échec : celle que donne l'enfant (code 1), sinon le
/// signal (Linux), le code d'un arrêt par le système (Windows) ou le code de
/// sortie.
fn failure(status: ExitStatus, output: &str) -> String {
    let reason = output.trim();
    if status.code() == Some(pdf_child::FAILED) && !reason.is_empty() {
        return reason.chars().take(MAX_REASON).collect();
    }
    abnormal_end(status).unwrap_or_else(|| {
        format!(
            "the PDF reader failed (exit code {})",
            status.code().unwrap_or(-1)
        )
    })
}

/// Programme relancé en extracteur : voir [`PdfExtractor::app`].
fn app_program() -> PathBuf {
    if cfg!(windows) {
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from("chatonlm.exe"))
    } else {
        PathBuf::from("/proc/self/exe")
    }
}

/// Enfant arrêté par un signal : temps de processeur dépassé (SIGXCPU) ou
/// plantage.
#[cfg(target_os = "linux")]
fn abnormal_end(status: ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;
    match status.signal() {
        Some(libc::SIGXCPU) => Some("the PDF reader used too much processor time".into()),
        Some(signal) => Some(format!("the PDF reader crashed (signal {signal})")),
        None => None,
    }
}

/// Enfant arrêté par le système : voir [`windows_end`].
#[cfg(windows)]
fn abnormal_end(status: ExitStatus) -> Option<String> {
    status.code().and_then(windows_end)
}

/// Sous Windows, il n'y a pas de signaux : un arrêt par le système se lit
/// dans le code de sortie. `STATUS_QUOTA_EXCEEDED` (`0xC0000044`, mesuré sur
/// un runner Windows) ou `ERROR_NOT_ENOUGH_QUOTA` (1816, son équivalent
/// Win32) : limite de temps de processeur du Job Object de l'enfant ; un
/// autre code NTSTATUS d'erreur (`0xC…`) : plantage (violation d'accès,
/// `abort`, débordement de pile).
/// `None` : code de sortie ordinaire.
#[cfg(any(windows, test))]
pub(super) fn windows_end(code: i32) -> Option<String> {
    const ERROR_NOT_ENOUGH_QUOTA: u32 = 1816;
    const STATUS_QUOTA_EXCEEDED: u32 = 0xC000_0044;
    let code = code as u32;
    if code == ERROR_NOT_ENOUGH_QUOTA || code == STATUS_QUOTA_EXCEEDED {
        Some("the PDF reader used too much processor time".into())
    } else if code >= 0xC000_0000 {
        Some(format!("the PDF reader crashed (code {code:#010x})"))
    } else {
        None
    }
}

/// Texte d'un PDF à partir de celui de ses pages, comme `extractPDFText`
/// (extract.go) : les pages sans texte sont sautées ; avant chaque page
/// écrite sauf la première, `\n\n--- Page N ---\n` (N : numéro réel).
pub fn join_pages(pages: &[String]) -> String {
    let mut text = String::new();
    for (index, page) in pages.iter().enumerate() {
        if page.trim().is_empty() {
            continue;
        }
        if !text.is_empty() {
            let _ = write!(text, "\n\n--- Page {} ---\n", index + 1);
        }
        text.push_str(page);
    }
    text
}

/// Texte enregistré pour un PDF, comme `convertBytesToText` : le texte
/// extrait, ou l'un des deux textes officiels d'échec.
pub async fn pdf_text(data: &[u8], extractor: &PdfExtractor) -> String {
    match extractor.extract(data).await {
        Ok(text) if text.trim().is_empty() => {
            format!("[PDF file - {} bytes - no text content found]", data.len())
        }
        Ok(text) => text,
        Err(error) => format!(
            "[PDF file - {} bytes - failed to extract text: {error}]",
            data.len()
        ),
    }
}
