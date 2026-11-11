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

/// Le binaire, sans vidage mémoire sous Unix (`ulimit -c 0` puis `exec`,
/// même processus) : les plantages voulus (`crash`, `vulkanfail`)
/// n'encombrent pas systemd-coredump.
fn command() -> Command {
    if cfg!(unix) {
        let mut command = Command::new("sh");
        command.args(["-c", "ulimit -c 0 && exec \"$0\" \"$@\"", BIN]);
        command
    } else {
        Command::new(BIN)
    }
}

/// Serveur lancé, journal lu ligne à ligne.
struct Server {
    child: Child,
    /// Lignes du journal lues jusqu'ici.
    lines: Arc<Mutex<Vec<String>>>,
    port: u16,
}

fn launch(model: &PathBuf, extra: &[&str], env: &[(&str, &str)]) -> Server {
    let mut command = command();
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

/// Lot L3 : sur la carte, `chatonlm-vulkanfail` meurt au chargement en
/// accusant Vulkan ; sur le processeur (`-ngl 0`, ou aucune carte visible),
/// il sert. Le journal dit la mémoire réservée comme le vrai à `-lv 4`.
#[test]
fn the_vulkan_failure_and_the_memory_lines_follow_the_real_server() {
    let path = model("x-chatonlm-vulkanfail.gguf");
    let output = command()
        .args(["--host", "127.0.0.1", "--port", "0", "-m"])
        .arg(&path)
        .env("FAKE_LLAMA_LOAD_MS", "10")
        .env_remove("GGML_VK_VISIBLE_DEVICES")
        .output()
        .unwrap();
    assert!(!output.status.success());
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(output.status.signal(), Some(6));
    }
    let log = String::from_utf8_lossy(&output.stderr);
    assert!(log.contains("vk::DeviceLostError"), "{log}");
    assert!(!log.contains("listening on"), "{log}");

    let cpu = launch(&path, &["-ngl", "0", "-c", "4096"], &[]);
    cpu.wait_line("model loaded");
    cpu.wait_line("CPU KV buffer size =   448.00 MiB");
    assert!(
        !cpu.lines
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("Vulkan0"))
    );
    let hidden = launch(&path, &[], &[("GGML_VK_VISIBLE_DEVICES", "none")]);
    hidden.wait_line("model loaded");

    let gpu = launch(&model("plain.gguf"), &["-c", "8192"], &[]);
    gpu.wait_line("offloaded 29/29 layers to GPU");
    gpu.wait_line("Vulkan0 KV buffer size =   896.00 MiB");
}

/// `FAKE_LLAMA_REGISTRY` : l'instance note son adresse et son alias (port
/// 0 : une fois à l'écoute ; les e2e y trouvent le port choisi par l'app),
/// et retire sa fiche à l'arrêt propre.
#[test]
fn the_registry_gives_the_address_of_each_instance() {
    let model = model("chatonlm-simple-registry.gguf");
    let registry = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fake-llama-registry");
    let _ = std::fs::remove_dir_all(&registry);
    let registry_text = registry.to_string_lossy().into_owned();
    let server = launch(
        &model,
        &["--alias", "a/chatonlm-simple-GGUF:Q4_K_M"],
        &[("FAKE_LLAMA_REGISTRY", &registry_text)],
    );
    let file = registry.join(format!("{}.json", server.child.id()));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !file.exists() {
        assert!(Instant::now() < deadline, "pas de fiche dans le registre");
        std::thread::sleep(Duration::from_millis(10));
    }
    let entry: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(entry["url"], format!("http://127.0.0.1:{}", server.port));
    assert_eq!(entry["alias"], "a/chatonlm-simple-GGUF:Q4_K_M");
    assert_eq!(entry["pid"], server.child.id());
    // Aucun fichier à moitié écrit ne reste.
    let names: Vec<String> = std::fs::read_dir(&registry)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [format!("{}.json", server.child.id())]);
    #[cfg(unix)]
    {
        let mut server = server;
        let term = Command::new("kill")
            .args(["-TERM", &server.child.id().to_string()])
            .status()
            .unwrap();
        assert!(term.success());
        assert_eq!(server.wait_exit().code(), Some(0));
        assert!(!file.exists(), "fiche restée après l'arrêt");
    }
}

/// Port donné (comme le donne l'app) : la fiche est écrite AVANT que le
/// port n'accepte une connexion, donc avant que l'app voie le modèle prêt
/// (sous un disque très chargé, une fiche écrite après manquait aux e2e).
#[test]
fn with_a_given_port_the_entry_exists_before_the_port_opens() {
    let model = model("chatonlm-simple-early.gguf");
    let registry = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fake-llama-registry-early");
    let _ = std::fs::remove_dir_all(&registry);
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = command()
        .args(["--host", "127.0.0.1", "--port", &port.to_string(), "-m"])
        .arg(&model)
        .env_remove("FAKE_LLAMA_MODE")
        .env("FAKE_LLAMA_REGISTRY", &registry)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let file = registry.join(format!("{}.json", child.id()));
    let deadline = Instant::now() + Duration::from_secs(5);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "le port ne s'ouvre pas");
        std::thread::sleep(Duration::from_millis(1));
    }
    let present = file.exists();
    let _ = child.kill();
    let _ = child.wait();
    assert!(present, "port ouvert sans fiche dans le registre");
    let entry: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(entry["url"], format!("http://127.0.0.1:{port}"));
}

/// `fake-llama gguf` écrit un fichier GGUF (dossiers créés) ; une option
/// inconnue est refusée (code 2) sans rien écrire.
#[test]
fn the_gguf_command_writes_a_test_model() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fake-llama-gguf");
    let _ = std::fs::remove_dir_all(&dir);
    let out = dir.join("a/B-GGUF/B-Q4_K_M.gguf");
    let status = Command::new(BIN)
        .arg("gguf")
        .arg(&out)
        .args(["--fim", "--size", "5000"])
        .status()
        .unwrap();
    assert!(status.success());
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(&bytes[..4], b"GGUF");
    assert_eq!(bytes.len(), 5000);
    let bad = dir.join("bad.gguf");
    let refused = Command::new(BIN)
        .arg("gguf")
        .arg(&bad)
        .arg("--nope")
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(refused.code(), Some(2));
    assert!(!bad.exists());
}

/// `chatonlm-split` (alias ou fichier) : sur la carte, 22 couches sur 29
/// seulement (modèle à cheval, escargot de l'app) ; un autre modèle : 29.
#[test]
fn a_split_model_keeps_layers_on_the_processor() {
    let split = launch(
        &model("plain.gguf"),
        &["--alias", "a/chatonlm-split-GGUF:Q4_K_M"],
        &[],
    );
    split.wait_line("offloaded 22/29 layers to GPU");
    let whole = launch(&model("chatonlm-simple-whole.gguf"), &[], &[]);
    whole.wait_line("offloaded 29/29 layers to GPU");
}
