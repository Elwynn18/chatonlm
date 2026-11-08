//! Le binaire `fake-llama` lancé comme `llama-server` : options, journal,
//! chargement, arrêt et fins demandées par les scénarios.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_fake-llama");

/// Fichier de modèle (vide : le faux ne le lit pas, il vérifie qu'il existe).
fn model(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fake-llama-models");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, b"GGUF").unwrap();
    path
}

/// Serveur lancé, journal lu ligne à ligne.
struct Server {
    child: Child,
    /// Lignes du journal lues jusqu'ici.
    lines: Arc<Mutex<Vec<String>>>,
    port: u16,
}

fn launch(model: &PathBuf, extra: &[&str], env: &[(&str, &str)]) -> Server {
    let mut command = Command::new(BIN);
    command
        .args(["--host", "127.0.0.1", "--port", "0", "--api-key", "k", "-m"])
        .arg(model)
        .args(extra)
        .env_remove("FAKE_LLAMA_MODE")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command.spawn().unwrap();
    let stderr = child.stderr.take().unwrap();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let reader = lines.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            reader.lock().unwrap().push(line);
        }
    });
    let mut server = Server {
        child,
        lines,
        port: 0,
    };
    let line = server.wait_line("listening on http://127.0.0.1:");
    server.port = line.rsplit(':').next().unwrap().trim().parse().unwrap();
    server
}

impl Server {
    /// Attend une ligne du journal qui contient `text` (5 s au plus).
    fn wait_line(&self, text: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(line) = self
                .lines
                .lock()
                .unwrap()
                .iter()
                .find(|line| line.contains(text))
            {
                return line.clone();
            }
            assert!(Instant::now() < deadline, "pas de ligne « {text} »");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_exit(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "le serveur ne s'arrête pas");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Requête HTTP/1.1 minimale ; rend le code et le corps.
fn http(port: u16, method: &str, path: &str, key: Option<&str>, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let auth = key
        .map(|key| format!("Authorization: Bearer {key}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    let status = response
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default();
    (status, body)
}

#[test]
fn version_and_devices_answer_like_the_real_binary() {
    let output = Command::new(BIN).arg("--version").output().unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .starts_with("version: 0.9.0-dev (build 12110, commit cd959bae9)")
    );
    let output = Command::new(BIN).arg("--list-devices").output().unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("Available devices:"));
}

#[test]
fn a_missing_model_or_a_bad_option_value_ends_with_code_1() {
    let missing = Command::new(BIN)
        .args(["--port", "0", "-m", "/nonexistent/model.gguf"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("failed to load model"));
    let bad = Command::new(BIN).args(["--port", "x"]).output().unwrap();
    assert_eq!(bad.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("error: invalid argument: --port x"));
}

#[test]
fn it_loads_serves_and_stops_on_request() {
    let model = model("chatonlm-simple-Q4_K_M.gguf");
    let server = launch(
        &model,
        &[
            "-c",
            "2048",
            "-np",
            "1",
            "--no-webui",
            "--offline",
            "--metrics",
            "--flash-attn",
            "on",
        ],
        &[],
    );
    server.wait_line("option inconnue acceptée : --flash-attn on");
    // Chargement de 150 ms : 503 d'abord.
    let (status, body) = http(server.port, "GET", "/health", None, "");
    assert_eq!(status, 503, "{body}");
    server.wait_line("model loaded");
    assert_eq!(http(server.port, "GET", "/health", None, "").0, 200);
    let (status, props) = http(server.port, "GET", "/props", Some("k"), "");
    assert_eq!(status, 200);
    assert!(props.contains("\"n_ctx\":2048"), "{props}");
    assert_eq!(http(server.port, "GET", "/props", Some("bad"), "").0, 401);
    let (_, args) = http(server.port, "GET", "/fake/args", None, "");
    assert!(args.contains("\"--flash-attn\",\"on\""), "{args}");
    let (status, reply) = http(
        server.port,
        "POST",
        "/v1/chat/completions",
        Some("k"),
        r#"{"messages":[{"role":"user","content":"hi"}]}"#,
    );
    assert_eq!(status, 200);
    assert!(reply.contains("Hello! You said: hi"), "{reply}");
    // Sous Windows, pas de SIGTERM : la fin passe par `Drop`.
    #[cfg(unix)]
    {
        let mut server = server;
        let term = Command::new("kill")
            .args(["-TERM", &server.child.id().to_string()])
            .status()
            .unwrap();
        assert!(term.success());
        let status = server.wait_exit();
        assert_eq!(status.code(), Some(0));
    }
}

#[test]
fn a_failed_load_ends_with_code_1() {
    let model = model("chatonlm-loadfail.gguf");
    let mut server = launch(&model, &[], &[("FAKE_LLAMA_LOAD_MS", "50")]);
    assert_eq!(http(server.port, "GET", "/health", None, "").0, 503);
    server.wait_line("exiting due to model loading error");
    assert_eq!(server.wait_exit().code(), Some(1));
}

#[test]
fn a_crash_aborts_the_process() {
    let model = model("chatonlm-crash.gguf");
    let mut server = launch(&model, &[], &[("FAKE_LLAMA_LOAD_MS", "0")]);
    server.wait_line("model loaded");
    let body = r#"{"messages":[{"role":"user","content":"hi"}],"stream":true}"#;
    let (status, _) = http(server.port, "POST", "/v1/chat/completions", Some("k"), body);
    assert_eq!(status, 200);
    let status = server.wait_exit();
    assert!(!status.success());
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(6), "SIGABRT");
    }
}

#[test]
fn the_mode_comes_from_the_environment() {
    let model = model("m.gguf");
    let server = launch(&model, &[], &[("FAKE_LLAMA_MODE", "status:502")]);
    assert_eq!(http(server.port, "GET", "/health", None, "").0, 502);
    let bad = Command::new(BIN)
        .args(["--port", "0", "-m"])
        .arg(&model)
        .env("FAKE_LLAMA_MODE", "bogus")
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
}

#[test]
fn keys_also_come_from_the_environment_and_a_file() {
    // Comme le vrai (relevé sur b12110) : `LLAMA_API_KEY`, `--api-key` et
    // `--api-key-file` s'ajoutent ; fichier : une clé par ligne, sans `#…`.
    let model = model("m.gguf");
    let file = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fake-llama-keys.txt");
    std::fs::write(&file, "# commentaire\nfile1\n\nfile2\n").unwrap();
    let file = file.to_string_lossy().into_owned();
    let server = launch(
        &model,
        &["--api-key-file", &file],
        &[("LLAMA_API_KEY", "env1,env2"), ("FAKE_LLAMA_LOAD_MS", "0")],
    );
    server.wait_line("model loaded");
    for key in ["k", "env1", "env2", "file1", "file2"] {
        assert_eq!(
            http(server.port, "GET", "/props", Some(key), "").0,
            200,
            "{key}"
        );
    }
    for key in ["env1,env2", "# commentaire", "bad"] {
        assert_eq!(
            http(server.port, "GET", "/props", Some(key), "").0,
            401,
            "{key}"
        );
    }
    assert!(
        !server
            .lines
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("option inconnue")),
        "--api-key-file est une option connue"
    );
    // Fichier absent : code 1, comme le vrai.
    let missing = Command::new(BIN)
        .args([
            "--port",
            "0",
            "--api-key-file",
            "/nonexistent/keys.txt",
            "-m",
        ])
        .arg(&model)
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("failed to open file"));
}
