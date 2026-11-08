use super::*;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn facts() -> Facts {
    Facts {
        version: "1.0.0".into(),
        platform: "linux",
        os: "Arch Linux, kernel 7.2.11".into(),
        arch: "x86_64",
        desktop: Some("KDE, wayland".into()),
        install: "AppImage".into(),
        language: "fr".into(),
        ollama: "this computer, 0.40.13".into(),
        builtin_ollama: Some("0.40.13, stopped".into()),
        log: "2026-11-08T10:00:00Z chatonlm: started\n".into(),
    }
}

fn report(include_diagnostic: bool) -> BugReport {
    BugReport {
        message: "  The window stays blank.  ".into(),
        email: " me@example.org ".into(),
        include_diagnostic,
    }
}

#[test]
fn the_diagnostic_lists_the_facts_then_the_end_of_the_log() {
    let text = diagnostic(&facts());
    assert!(text.starts_with("ChatonLM: 1.0.0\nSystem: Arch Linux, kernel 7.2.11\n"));
    assert!(text.contains("Desktop: KDE, wayland\n"));
    assert!(text.contains("Installed as: AppImage\n"));
    assert!(text.contains("Ollama: this computer, 0.40.13\n"));
    assert!(text.contains("Built-in Ollama: 0.40.13, stopped\n"));
    assert!(text.ends_with("--- chatonlm.log (end) ---\n2026-11-08T10:00:00Z chatonlm: started\n"));
    // Sans journal, pas de section vide.
    let empty = Facts {
        log: " \n".into(),
        ..facts()
    };
    assert!(!diagnostic(&empty).contains("chatonlm.log"));
}

#[test]
fn the_home_folder_and_the_account_name_are_masked() {
    let text =
        "/home/theo/.local/share/x\nC:\\Users\\theo\\AppData\nC:/Users/theo/x\nuser theo, theory\n";
    let masked = mask_personal(text, Some("/home/theo/"), Some("theo"));
    assert_eq!(
        masked,
        "~/.local/share/x\nC:\\Users\\<user>\\AppData\nC:/Users/<user>/x\nuser <user>, theory\n"
    );
    let windows = mask_personal(
        "C:\\Users\\theo\\AppData and C:/Users/theo/x",
        Some("C:\\Users\\theo"),
        None,
    );
    assert_eq!(windows, "~\\AppData and ~/x");
    // Nom trop court : gardé hors du dossier personnel.
    assert_eq!(mask_personal("al is here", None, Some("al")), "al is here");
}

#[test]
fn a_long_diagnostic_keeps_its_facts_and_the_latest_lines() {
    let log: String = (0..3000).map(|i| format!("line {i:05}\n")).collect();
    let text = diagnostic(&Facts { log, ..facts() });
    assert!(text.chars().count() <= DIAGNOSTIC_MAX);
    assert!(text.starts_with("ChatonLM: 1.0.0\n"));
    assert!(text.ends_with("line 02999\n"));
    // Coupé au début d'une ligne.
    let after = text
        .split_once("Built-in Ollama: 0.40.13, stopped\n\n")
        .unwrap()
        .1;
    assert!(after.starts_with("line "), "{}", &after[..20]);
}

#[test]
fn os_release_and_windows_names() {
    let release = "NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\nID=arch\n";
    assert_eq!(os_release_name(release).as_deref(), Some("Arch Linux"));
    assert_eq!(os_release_name("NAME=Fedora\n").as_deref(), Some("Fedora"));
    assert_eq!(os_release_name("ID=x\n"), None);
    assert_eq!(
        windows_name(
            Some("Windows 10 Pro"),
            Some("24H2"),
            Some("26100"),
            Some(4061)
        ),
        "Windows 11 Pro 24H2, build 26100.4061"
    );
    assert_eq!(
        windows_name(Some("Windows 10 Home"), Some("22H2"), Some("19045"), None),
        "Windows 10 Home 22H2, build 19045"
    );
    assert_eq!(windows_name(None, None, None, None), "Windows");
}

#[test]
fn the_payload_follows_the_service_contract() {
    let body = payload(&report(false), &facts(), "fr");
    assert_eq!(
        body,
        serde_json::json!({
            "message": "The window stays blank.",
            "email": "me@example.org",
            "lang": "fr",
            "version": "1.0.0",
            "platform": "linux",
        })
    );
    let with = payload(&report(true), &facts(), "fr");
    assert_eq!(with["diagnostic"], serde_json::json!(diagnostic(&facts())));
}

#[test]
fn answers_of_the_service_become_outcomes() {
    assert_eq!(outcome_of(200, None), ReportOutcome::Sent);
    assert_eq!(outcome_of(429, Some("rate")), ReportOutcome::TooMany);
    assert_eq!(outcome_of(422, Some("email")), ReportOutcome::InvalidEmail);
    assert_eq!(
        outcome_of(422, Some("message")),
        ReportOutcome::InvalidMessage
    );
    assert_eq!(outcome_of(422, Some("version")), ReportOutcome::Unavailable);
    assert_eq!(outcome_of(503, Some("send")), ReportOutcome::Unavailable);
    assert_eq!(outcome_of(403, Some("origin")), ReportOutcome::Unavailable);
}

/// Serveur d'une seule requête : rend la requête reçue.
async fn serve_once(
    status: &'static str,
    body: &'static str,
) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/report", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let read = socket.read(&mut buffer).await.unwrap();
            request.extend_from_slice(&buffer[..read]);
            let text = String::from_utf8_lossy(&request);
            if let Some((head, rest)) = text.split_once("\r\n\r\n") {
                let length = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if rest.len() >= length {
                    break;
                }
            }
            if read == 0 {
                break;
            }
        }
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8_lossy(&request).into_owned()
    });
    (url, task)
}

#[tokio::test]
async fn a_report_is_posted_as_json_by_chatonlm() {
    let (url, request) = serve_once("200 OK", r#"{"ok":true}"#).await;
    let body = payload(&report(false), &facts(), "en");
    assert_eq!(send_to(&url, &body).await, ReportOutcome::Sent);
    let request = request.await.unwrap();
    assert!(
        request.starts_with("POST /api/report HTTP/1.1\r\n"),
        "{request}"
    );
    let head = request.to_ascii_lowercase();
    assert!(head.contains("content-type: application/json"));
    assert!(head.contains(&format!(
        "user-agent: chatonlm/{}",
        env!("CARGO_PKG_VERSION")
    )));
    // Jamais d'en-tête Origin : le service refuse une origine étrangère.
    assert!(!head.contains("\r\norigin:"));
    let json = request.split_once("\r\n\r\n").unwrap().1;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(json).unwrap(),
        body
    );
}

#[tokio::test]
async fn a_refusal_of_the_service_is_reported() {
    let (url, _request) =
        serve_once("429 Too Many Requests", r#"{"ok":false,"error":"rate"}"#).await;
    let body = payload(&report(false), &facts(), "en");
    assert_eq!(send_to(&url, &body).await, ReportOutcome::TooMany);
    let (url, _request) = serve_once(
        "422 Unprocessable Entity",
        r#"{"ok":false,"error":"email"}"#,
    )
    .await;
    assert_eq!(send_to(&url, &body).await, ReportOutcome::InvalidEmail);
}

#[tokio::test]
async fn an_unreachable_service_is_unavailable() {
    // Port fermé : rien n'écoute.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/report", listener.local_addr().unwrap());
    drop(listener);
    let body = payload(&report(false), &facts(), "en");
    assert_eq!(send_to(&url, &body).await, ReportOutcome::Unavailable);
}
