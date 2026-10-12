//! Extraction des PDF par le vrai binaire de l'app (argument réservé) :
//! texte, échecs, et limites du processus enfant (délai, processeur,
//! mémoire, plantage, sortie). Les comportements de test de l'enfant
//! (`--chatonlm-test=…`) n'existent que dans les builds de débogage.

use std::time::{Duration, Instant};

use chatonlm_lib::attachments::pdf::{MAX_OUTPUT, PdfExtractor, TIMEOUT, pdf_text};
use chatonlm_lib::attachments::pdf_child::CHILD_ARG;

use crate::samples;

/// Le binaire de l'app en extracteur, avec ces arguments de test.
fn app(test: &[&str]) -> PdfExtractor {
    let mut args = vec![CHILD_ARG.into()];
    args.extend(test.iter().map(Into::into));
    PdfExtractor {
        program: env!("CARGO_BIN_EXE_chatonlm").into(),
        args,
        timeout: TIMEOUT,
        max_output: MAX_OUTPUT,
    }
}

fn failed(data: &[u8], reason: &str) -> String {
    format!(
        "[PDF file - {} bytes - failed to extract text: {reason}]",
        data.len()
    )
}

#[tokio::test]
async fn the_app_extracts_the_text_of_each_page() {
    let pdf = samples::pdf(&[
        &["Hello world", "Second line"],
        &[],
        &["Élève, déjà, où, cœur."],
    ]);
    let text = pdf_text(&pdf, &app(&[])).await;
    assert_eq!(
        text,
        "Hello world\nSecond line\n\n--- Page 3 ---\nÉlève, déjà, où, cœur."
    );
}

#[tokio::test]
async fn a_pdf_without_text() {
    let pdf = samples::pdf_without_text();
    assert_eq!(
        pdf_text(&pdf, &app(&[])).await,
        format!("[PDF file - {} bytes - no text content found]", pdf.len())
    );
}

#[tokio::test]
async fn broken_pdfs_fail_cleanly() {
    let whole = samples::pdf(&[&["Hello world"]]);
    let truncated = whole[..whole.len() / 2].to_vec();
    let garbage: Vec<u8> = (0..4096u32).map(|i| (i * 7919 % 251) as u8).collect();
    let unreadable = |reason: &str| format!("failed to create PDF reader: {reason}");
    for (name, data, expected) in [
        (
            "empty",
            Vec::new(),
            failed(&[], &unreadable("File is empty (0 bytes)")),
        ),
        (
            "not a PDF",
            b"Hello, I am plain text.".to_vec(),
            failed(&[0; 23], &unreadable("Invalid PDF header")),
        ),
        (
            "garbage",
            garbage,
            failed(&[0; 4096], &unreadable("Invalid PDF header")),
        ),
        // Structure retrouvée, contenu coupé : aucun texte.
        (
            "truncated",
            truncated.clone(),
            format!(
                "[PDF file - {} bytes - no text content found]",
                truncated.len()
            ),
        ),
    ] {
        let started = Instant::now();
        assert_eq!(pdf_text(&data, &app(&[])).await, expected, "{name}");
        assert!(started.elapsed() < Duration::from_secs(10), "{name}");
    }
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn library_warnings_do_not_mix_with_the_text() {
    // 20 000 lignes sur la sortie standard de l'enfant avant l'extraction :
    // envoyées sur la sortie d'erreur, lues et jetées.
    let pdf = samples::pdf(&[&["Hello world"]]);
    assert_eq!(
        pdf_text(&pdf, &app(&["--chatonlm-test=noisy"])).await,
        "Hello world"
    );
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn the_child_limits_itself_before_reading() {
    // Linux : pas de vidage mémoire ; 1 Gio d'espace d'adressage ; 30 s de
    // processeur (SIGXCPU), puis SIGKILL une seconde plus tard. Windows :
    // Job Object de l'enfant, 1 Gio de mémoire engagée, 30 s de processeur
    // (unités de 100 ns), fin sans boîte d'erreur sur une exception (0x400).
    let limits = app(&["--chatonlm-test=limits"]).extract(b"").await;
    let expected = if cfg!(windows) {
        "memory=1073741824 cpu=300000000 flags=0x502"
    } else {
        "dumpable=0 core=0/0 as=1073741824/1073741824 cpu=30/31"
    };
    assert_eq!(limits.unwrap(), expected);
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn a_hanging_reader_is_stopped_by_the_wall_clock() {
    let pdf = samples::pdf(&[&["Hello world"]]);
    let mut sleeping = app(&["--chatonlm-test=sleep"]);
    sleeping.timeout = Duration::from_secs(1);
    let started = Instant::now();
    assert_eq!(
        pdf_text(&pdf, &sleeping).await,
        failed(&pdf, "the PDF reader took more than 1 s")
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn processor_time_is_limited() {
    // Limite abaissée à 1 s pour le test ; le délai mural (20 s) ne joue pas.
    let pdf = samples::pdf(&[&["Hello world"]]);
    let mut spinning = app(&["--chatonlm-test=spin", "--chatonlm-test-cpu=1"]);
    spinning.timeout = Duration::from_secs(20);
    let started = Instant::now();
    assert_eq!(
        pdf_text(&pdf, &spinning).await,
        failed(&pdf, "the PDF reader used too much processor time")
    );
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn memory_is_limited() {
    // 2 Gio réservés sans y toucher : accepté par le noyau sans la limite de
    // 1 Gio d'espace d'adressage (de mémoire engagée sous Windows).
    let pdf = samples::pdf(&[&["Hello world"]]);
    assert_eq!(
        pdf_text(&pdf, &app(&["--chatonlm-test=alloc"])).await,
        failed(&pdf, "out of memory")
    );
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn crashes_and_panics_are_failures() {
    let pdf = samples::pdf(&[&["Hello world"]]);
    // `abort` : SIGABRT sous Linux, échec rapide (0xC0000409) sous Windows.
    let aborted = if cfg!(windows) {
        "the PDF reader crashed (code 0xc0000409)"
    } else {
        "the PDF reader crashed (signal 6)"
    };
    for (mode, reason) in [
        ("abort", aborted),
        ("panic", "the PDF library crashed: simulated failure"),
    ] {
        let test = format!("--chatonlm-test={mode}");
        assert_eq!(
            pdf_text(&pdf, &app(&[&test])).await,
            failed(&pdf, reason),
            "{mode}"
        );
    }
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn the_text_is_capped() {
    // 8 Mio écrits par l'enfant, plafond à 1 Mio.
    let pdf = samples::pdf(&[&["Hello world"]]);
    let mut flooding = app(&["--chatonlm-test=flood"]);
    flooding.max_output = 1024 * 1024;
    let started = Instant::now();
    assert_eq!(
        pdf_text(&pdf, &flooding).await,
        failed(&pdf, "the extracted text is too large")
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}
