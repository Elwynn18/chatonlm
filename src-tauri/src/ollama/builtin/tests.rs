//! Essais du sous-processus contre un faux `ollama serve` : ce binaire
//! d'essai lui-même, relancé sur le seul essai [`fake_serve`] (qui sert le
//! faux serveur de `fake_ollama::serve` quand `CHATONLM_FAKE_SERVE` est
//! posée) ; ses « runners » sont le même binaire. Délais raccourcis.

use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[cfg(unix)]
use super::os::{Ours, group_of, leftover_processes};
use super::*;

/// Faux `ollama serve` (et ses runners) : ignoré, sauf relancé par un essai.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "faux serveur lancé par les autres essais"]
async fn fake_serve() {
    if std::env::var_os("CHATONLM_FAKE_SERVE").is_some() {
        std::process::exit(fake_ollama::serve::run().await);
    }
}

/// Arguments qui relancent ce binaire sur [`fake_serve`].
pub(crate) fn fake_args() -> Vec<OsString> {
    [
        "ollama::builtin::tests::fake_serve",
        "--exact",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]
    .map(OsString::from)
    .to_vec()
}

/// Réglages d'un intégré joué par le faux serveur, données sous `dir`.
pub(crate) fn fake_config(dir: &Path, env: &[(&str, &str)]) -> Config {
    let mut config = Config::new(Some(std::env::current_exe().unwrap()), dir);
    config.args = fake_args();
    config.extra_env = std::iter::once(("CHATONLM_FAKE_SERVE", "1"))
        .chain(env.iter().copied())
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
    config.timing = Timing {
        ready_timeout: Duration::from_secs(20),
        ready_poll: Duration::from_millis(50),
        backoff: vec![Duration::from_millis(50); 5],
        crash_window: Duration::from_secs(60),
        max_crashes: 3,
        stop_grace: Duration::from_millis(1500),
        kill_wait: Duration::from_millis(1000),
    };
    config
}

/// Dossier des modèles choisi (hors dossier par défaut), créé : un dossier
/// choisi absent n'est jamais créé par l'intégré.
pub(crate) fn chosen_models(dir: &Path) -> PathBuf {
    let models = dir.join("models");
    std::fs::create_dir_all(&models).unwrap();
    models
}

pub(crate) fn temp_dir() -> tempfile::TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tools/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::tempdir_in(base).unwrap()
}

/// Vivant (ni absent ni zombie).
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    group_of(Path::new("/proc"), pid).is_some()
}

/// Vivant (pas encore sorti).
#[cfg(windows)]
fn alive(pid: u32) -> bool {
    super::os::alive(pid)
}

/// Attend que `condition` soit vraie, `limit` au plus.
async fn eventually(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    condition()
}

/// Attend un état qui vérifie `condition`, `limit` au plus (la référence de
/// `wait_for` est rendue aussitôt : la garder bloquerait les changements).
pub(crate) async fn reaches(
    builtin: &Builtin,
    limit: Duration,
    condition: impl FnMut(&Snapshot) -> bool,
) -> bool {
    let mut changes = builtin.subscribe();
    tokio::time::timeout(limit, changes.wait_for(condition))
        .await
        .is_ok_and(|result| result.is_ok())
}

fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

async fn runner_pid(config: &Config) -> u32 {
    let file = config.home().join("runner.pid");
    assert!(
        eventually(Duration::from_secs(10), || read_pid(&file).is_some()).await,
        "no runner"
    );
    read_pid(&file).unwrap()
}

fn environment_of(config: &Config) -> String {
    std::fs::read_to_string(config.home().join("env")).unwrap_or_default()
}

#[test]
fn ports_avoid_ollama_and_the_test_instance() {
    for previous in [Some(11434), Some(11435), None] {
        let port = pick_port(previous).unwrap();
        assert!(!RESERVED_PORTS.contains(&port), "{port}");
    }
    // Le port précédent est repris tant qu'il est libre.
    let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    assert_eq!(pick_port(Some(free)).unwrap(), free);
    let held = TcpListener::bind((Ipv4Addr::LOCALHOST, free)).unwrap();
    assert_ne!(pick_port(Some(free)).unwrap(), free);
    drop(held);
}

#[cfg(unix)]
#[test]
fn the_environment_is_cleaned_then_completed() {
    let inherited = [
        ("PATH", "/tmp/.mount_x/usr/bin:/usr/bin"),
        ("LD_LIBRARY_PATH", "/tmp/.mount_x/usr/lib:/opt/cuda/lib"),
        ("LD_PRELOAD", "/usr/lib/libgamemode.so"),
        ("XDG_DATA_DIRS", "/tmp/.mount_x/usr/share:/usr/share"),
        ("GTK_THEME", "Adwaita"),
        ("__NV_DISABLE_EXPLICIT_SYNC", "1"),
        ("OLLAMA_HOST", "0.0.0.0"),
        ("OLLAMA_MODELS", "/var/lib/ollama"),
        ("OLLAMA_KEEP_ALIVE", "10m"),
        ("HOME", "/home/user"),
        ("LANG", "fr_FR.UTF-8"),
    ]
    .map(|(key, value)| (OsString::from(key), OsString::from(value)));
    let environment = child_environment(
        inherited,
        Some(OsStr::new("/tmp/.mount_x")),
        true,
        40123,
        Path::new("/data/models"),
        Path::new("/data/ollama/home"),
        &[("EXTRA".into(), "1".into())],
    );
    let get = |key: &str| {
        environment
            .get(OsStr::new(key))
            .map(|value| value.to_string_lossy().into_owned())
    };
    assert_eq!(get("PATH").as_deref(), Some("/usr/bin"));
    assert_eq!(get("XDG_DATA_DIRS").as_deref(), Some("/usr/share"));
    for removed in [
        "LD_LIBRARY_PATH",
        "LD_PRELOAD",
        "GTK_THEME",
        "__NV_DISABLE_EXPLICIT_SYNC",
    ] {
        assert_eq!(get(removed), None, "{removed}");
    }
    assert_eq!(get("OLLAMA_HOST").as_deref(), Some("127.0.0.1:40123"));
    assert_eq!(get("OLLAMA_MODELS").as_deref(), Some("/data/models"));
    assert_eq!(get("HOME").as_deref(), Some("/data/ollama/home"));
    // Le reste suit, réglages d'Ollama de l'utilisateur compris.
    assert_eq!(get("OLLAMA_KEEP_ALIVE").as_deref(), Some("10m"));
    assert_eq!(get("LANG").as_deref(), Some("fr_FR.UTF-8"));
    assert_eq!(get("EXTRA").as_deref(), Some("1"));

    // Hors AppImage, sans contournement : seules LD_LIBRARY_PATH et
    // LD_PRELOAD partent.
    let plain = child_environment(
        [
            ("PATH", "/usr/bin"),
            ("LD_LIBRARY_PATH", "/opt/cuda/lib"),
            ("LD_PRELOAD", "/usr/lib/libgamemode.so"),
            ("__NV_DISABLE_EXPLICIT_SYNC", "1"),
        ]
        .map(|(key, value)| (OsString::from(key), OsString::from(value))),
        None,
        false,
        1,
        Path::new("/m"),
        Path::new("/h"),
        &[],
    );
    let mut keys: Vec<_> = plain.keys().map(|key| key.to_string_lossy()).collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "HOME",
            "OLLAMA_HOST",
            "OLLAMA_MODELS",
            "PATH",
            "__NV_DISABLE_EXPLICIT_SYNC"
        ]
    );
}

#[test]
fn variables_are_removed_whatever_their_case() {
    let mut environment: BTreeMap<OsString, OsString> = [
        ("Ollama_Host", "0.0.0.0"),
        ("OLLAMA_MODELS", "D:\\Models"),
        ("UserProfile", "C:\\Users\\me"),
        ("OLLAMA_KEEP_ALIVE", "10m"),
        ("Path", "C:\\Windows"),
    ]
    .map(|(key, value)| (OsString::from(key), OsString::from(value)))
    .into_iter()
    .collect();
    remove_ignoring_case(
        &mut environment,
        &["OLLAMA_HOST", "OLLAMA_MODELS", "USERPROFILE", "HOME"],
    );
    let keys: Vec<_> = environment
        .keys()
        .map(|key| key.to_string_lossy().into_owned())
        .collect();
    assert_eq!(keys, ["OLLAMA_KEEP_ALIVE", "Path"]);
}

#[cfg(unix)]
#[test]
fn the_binary_comes_with_the_package_and_its_version() {
    let dir = temp_dir();
    assert_eq!(locate(Some(dir.path())), None);
    let bin = dir.path().join("ollama/bin/ollama");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, "#!/bin/sh\n").unwrap();
    // Pas exécutable : indisponible.
    assert_eq!(locate(Some(dir.path())), None);
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(locate(Some(dir.path())), Some(bin.clone()));
    std::fs::write(dir.path().join("ollama/VERSION"), "v0.40.0\n").unwrap();
    let builtin = Builtin::new(Config::new(Some(bin), dir.path()));
    assert!(builtin.available());
    assert_eq!(builtin.version().as_deref(), Some("0.40.0"));

    let none = Builtin::new(Config::new(None, dir.path()));
    assert!(!none.available());
    assert_eq!(none.ensure_started(dir.path()), None);
    assert_eq!(none.snapshot().state, BuiltinState::Stopped);
}

#[tokio::test(flavor = "multi_thread")]
async fn starts_answers_and_stops_with_its_runners() {
    let dir = temp_dir();
    let config = fake_config(dir.path(), &[("FAKE_OLLAMA_RUNNER", "1")]);
    std::fs::create_dir_all(config.log.parent().unwrap()).unwrap();
    std::fs::write(&config.log, "previous session\n").unwrap();
    let builtin = Builtin::new(config.clone());
    let models = dir.path().join("models here");
    std::fs::create_dir_all(&models).unwrap();
    let url = builtin.ensure_started(&models).unwrap();
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    let port: u16 = url.rsplit(':').next().unwrap().parse().unwrap();
    assert!(!RESERVED_PORTS.contains(&port));
    // Lancé une seule fois.
    assert_eq!(builtin.ensure_started(&models), Some(url.clone()));

    let snapshot = builtin.wait_ready(Duration::from_secs(20)).await;
    assert_eq!(snapshot.state, BuiltinState::Running, "{snapshot:?}");
    assert_eq!(snapshot.version.as_deref(), Some("0.40.0"));
    let pid = snapshot.pid.unwrap();
    assert_eq!(read_pid(&config.pid_file()), Some(pid));
    let runner = runner_pid(&config).await;
    assert!(alive(pid) && alive(runner));
    assert!(matches!(client::probe(&url).await, Probe::Ollama { .. }));

    let environment = environment_of(&config);
    for line in [
        format!("OLLAMA_HOST=127.0.0.1:{port}"),
        format!("OLLAMA_MODELS={}", models.display()),
        format!("HOME={}", config.home().display()),
    ] {
        assert!(
            environment.lines().any(|l| l == line),
            "{line}\n{environment}"
        );
    }
    // Journal du lancement ; le précédent gardé en .1.
    let mut previous = config.log.clone().into_os_string();
    previous.push(".1");
    assert_eq!(
        std::fs::read_to_string(previous).unwrap(),
        "previous session\n"
    );
    assert!(
        eventually(Duration::from_secs(5), || std::fs::read_to_string(
            &config.log
        )
        .unwrap_or_default()
        .contains("Listening on"))
        .await
    );

    builtin.stop().await;
    assert_eq!(builtin.snapshot().state, BuiltinState::Stopped);
    assert!(!config.pid_file().exists());
    assert!(eventually(Duration::from_secs(5), || !alive(pid) && !alive(runner)).await);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_ignores_sigterm_is_killed_with_its_group() {
    let dir = temp_dir();
    let config = fake_config(
        dir.path(),
        &[
            ("FAKE_OLLAMA_RUNNER", "1"),
            ("FAKE_OLLAMA_IGNORE_TERM", "1"),
        ],
    );
    let builtin = Builtin::new(config.clone());
    builtin.ensure_started(&chosen_models(dir.path())).unwrap();
    let snapshot = builtin.wait_ready(Duration::from_secs(20)).await;
    assert_eq!(snapshot.state, BuiltinState::Running, "{snapshot:?}");
    let (pid, runner) = (snapshot.pid.unwrap(), runner_pid(&config).await);
    let started = Instant::now();
    builtin.stop().await;
    // SIGTERM ignoré par les deux : SIGKILL au groupe après le délai.
    assert!(started.elapsed() >= config.timing.stop_grace);
    assert!(eventually(Duration::from_secs(5), || !alive(pid) && !alive(runner)).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_leaves_no_runner_behind_and_restarts() {
    let dir = temp_dir();
    let crashes = dir.path().join("crashes");
    std::fs::write(&crashes, "0").unwrap();
    let config = fake_config(
        dir.path(),
        &[
            ("FAKE_OLLAMA_RUNNER", "1"),
            ("FAKE_OLLAMA_IGNORE_TERM", "1"),
            ("FAKE_OLLAMA_EXIT_AFTER_MS", "1500"),
        ],
    );
    let builtin = Builtin::new(config.clone());
    builtin.ensure_started(&chosen_models(dir.path())).unwrap();
    let first = builtin.wait_ready(Duration::from_secs(20)).await;
    assert_eq!(first.state, BuiltinState::Running, "{first:?}");
    let (pid, runner) = (first.pid.unwrap(), runner_pid(&config).await);
    std::fs::remove_file(config.home().join("runner.pid")).unwrap();
    // Le serveur sort seul (code 3) en laissant son runner : le groupe est
    // nettoyé, puis l'intégré relancé.
    assert!(eventually(Duration::from_secs(10), || !alive(pid)).await);
    assert!(eventually(Duration::from_secs(5), || !alive(runner)).await);
    let relaunched = reaches(&builtin, Duration::from_secs(20), |snapshot| {
        snapshot.state == BuiltinState::Running && snapshot.pid.is_some_and(|new| new != pid)
    })
    .await;
    assert!(relaunched, "{:?}", builtin.snapshot());
    builtin.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn crashes_at_start_are_retried_then_give_up() {
    let dir = temp_dir();
    let crashes = dir.path().join("crashes");
    std::fs::write(&crashes, "2").unwrap();
    let crashes_env = crashes.to_string_lossy().into_owned();
    let config = fake_config(dir.path(), &[("FAKE_OLLAMA_CRASHES", &crashes_env)]);
    let builtin = Builtin::new(config.clone());
    let models = chosen_models(dir.path());
    builtin.ensure_started(&models).unwrap();
    // Deux plantages (sous le maximum de 3) : il finit par servir.
    let running = reaches(&builtin, Duration::from_secs(20), |snapshot| {
        snapshot.state == BuiltinState::Running
    })
    .await;
    assert!(running, "{:?}", builtin.snapshot());
    assert_eq!(std::fs::read_to_string(&crashes).unwrap(), "0");
    let log = std::fs::read_to_string(&config.log).unwrap();
    assert_eq!(log.matches("plantage simulé").count(), 2, "{log}");
    builtin.stop().await;

    // Trois de suite : échec, avec le code de sortie ; plus de relance.
    std::fs::write(&crashes, "10").unwrap();
    builtin.ensure_started(&models).unwrap();
    let failed = reaches(&builtin, Duration::from_secs(20), |snapshot| {
        snapshot.state == BuiltinState::Failed
    })
    .await;
    assert!(failed, "{:?}", builtin.snapshot());
    let snapshot = builtin.snapshot();
    let failure = snapshot.failure.clone().unwrap();
    assert!(failure.contains("exit code 1"), "{failure}");
    assert_eq!(std::fs::read_to_string(&crashes).unwrap(), "7");
    // Demandé de nouveau sans relance explicite : rien ne repart.
    builtin.ensure_started(&models);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(builtin.snapshot().state, BuiltinState::Failed);
    assert_eq!(std::fs::read_to_string(&crashes).unwrap(), "7");

    // Retry : relancé.
    std::fs::write(&crashes, "0").unwrap();
    builtin.restart(&models).await.unwrap();
    let snapshot = builtin.wait_ready(Duration::from_secs(20)).await;
    assert_eq!(snapshot.state, BuiltinState::Running, "{snapshot:?}");
    builtin.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_answer_in_time_is_a_failure_and_the_server_is_stopped() {
    let dir = temp_dir();
    let mut config = fake_config(dir.path(), &[("FAKE_OLLAMA_DELAY_MS", "30000")]);
    config.timing.ready_timeout = Duration::from_millis(1500);
    let builtin = Builtin::new(config.clone());
    builtin.ensure_started(&chosen_models(dir.path())).unwrap();
    let mut pid = None;
    let failed = reaches(&builtin, Duration::from_secs(20), |snapshot| {
        pid = pid.or(snapshot.pid);
        snapshot.state == BuiltinState::Failed
    })
    .await;
    assert!(failed, "{:?}", builtin.snapshot());
    let failure = builtin.snapshot().failure.unwrap();
    assert!(failure.contains("1 seconds"), "{failure}");
    let pid = pid.unwrap();
    assert!(eventually(Duration::from_secs(5), || !alive(pid)).await);
    assert!(!config.pid_file().exists());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn leftovers_are_stopped_only_when_they_are_the_builtin_ollama() {
    let dir = temp_dir();
    let mut config = fake_config(dir.path(), &[]);
    config.timing.stop_grace = Duration::from_millis(500);
    std::fs::create_dir_all(config.home()).unwrap();
    std::fs::create_dir_all(&config.root).unwrap();
    let uid = std::fs::metadata("/proc/self").unwrap().uid();
    let ours = Ours {
        uid,
        home: config.home(),
    };
    let proc = Path::new("/proc");

    // Un autre programme a repris le numéro : jamais touché.
    let mut foreign = Command::new("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    std::fs::write(config.pid_file(), foreign.id().to_string()).unwrap();
    assert!(leftover_processes(proc, foreign.id(), &ours).is_empty());
    cleanup_leftovers(&config);
    assert!(!config.pid_file().exists());
    assert!(alive(foreign.id()));
    foreign.kill().unwrap();
    foreign.wait().unwrap();

    // ChatonLM lui-même, son groupe, init : jamais.
    for pid in [std::process::id(), 1, 0] {
        assert!(leftover_processes(proc, pid, &ours).is_empty());
    }

    // Un intégré d'une session précédente (SIGTERM ignoré) et son runner.
    let port = pick_port(None).unwrap();
    let mut leftover = Command::new(config.bin.as_ref().unwrap())
        .args(fake_args())
        .env("CHATONLM_FAKE_SERVE", "1")
        .env("OLLAMA_HOST", format!("127.0.0.1:{port}"))
        .env("HOME", config.home())
        .env("FAKE_OLLAMA_RUNNER", "1")
        .env("FAKE_OLLAMA_IGNORE_TERM", "1")
        .stdout(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    let runner = runner_pid(&config).await;
    std::fs::write(config.pid_file(), leftover.id().to_string()).unwrap();
    let mut both = vec![leftover.id(), runner];
    both.sort_unstable();
    assert_eq!(leftover_processes(proc, leftover.id(), &ours), both);
    // Notre binaire, mais un autre HOME : pas l'intégré (avec
    // CHATONLM_OLLAMA_BIN=/usr/bin/ollama, l'exécutable ne dirait rien).
    let stranger = Ours {
        uid,
        home: dir.path().join("elsewhere"),
    };
    assert!(leftover_processes(proc, leftover.id(), &stranger).is_empty());
    assert!(!stranger.owns(proc, leftover.id()) && !stranger.owns(proc, runner));
    // Au démarrage de l'app, une fois.
    let builtin = Builtin::new(config.clone());
    let cleaning = builtin.clone();
    tokio::task::spawn_blocking(move || cleaning.clean_leftovers())
        .await
        .unwrap();
    assert!(!config.pid_file().exists());
    leftover.wait().unwrap();
    assert!(eventually(Duration::from_secs(5), || !alive(runner)).await);
}

/// Faux ChatonLM pour l'essai de PDEATHSIG : démarre l'intégré sous
/// `CHATONLM_FAKE_PARENT`, puis attend d'être tué.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "faux ChatonLM lancé par pdeathsig_stops_the_server_when_chatonlm_dies"]
async fn fake_parent() {
    let Some(dir) = std::env::var_os("CHATONLM_FAKE_PARENT").map(PathBuf::from) else {
        return;
    };
    let builtin = Builtin::new(fake_config(&dir, &[("FAKE_OLLAMA_RUNNER", "1")]));
    builtin.ensure_started(&chosen_models(&dir)).unwrap();
    let snapshot = builtin.wait_ready(Duration::from_secs(20)).await;
    std::fs::write(dir.join("ready"), format!("{:?}", snapshot.state)).unwrap();
    std::future::pending::<()>().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pdeathsig_stops_the_server_when_chatonlm_dies() {
    let dir = temp_dir();
    let mut parent = Command::new(std::env::current_exe().unwrap())
        .args([
            "ollama::builtin::tests::fake_parent",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .env("CHATONLM_FAKE_PARENT", dir.path())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let ready = dir.path().join("ready");
    assert!(eventually(Duration::from_secs(30), || ready.exists()).await);
    assert_eq!(std::fs::read_to_string(&ready).unwrap(), "Running");
    let config = fake_config(dir.path(), &[]);
    let server = read_pid(&config.pid_file()).unwrap();
    let runner = runner_pid(&config).await;
    assert!(alive(server) && alive(runner));
    // ChatonLM tué sans rien pouvoir arrêter : le serveur reçoit SIGTERM et
    // arrête son runner, comme Ollama.
    parent.kill().unwrap();
    parent.wait().unwrap();
    assert!(eventually(Duration::from_secs(10), || !alive(server) && !alive(runner)).await);
}

// Unix seulement : sous Windows l'arrêt (TerminateJobObject) est immédiat,
// il n'y a pas d'arrêt long pendant lequel demander un démarrage.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn nothing_starts_while_stopping_or_paused() {
    let dir = temp_dir();
    let config = fake_config(dir.path(), &[("FAKE_OLLAMA_IGNORE_TERM", "1")]);
    let builtin = Builtin::new(config.clone());
    let models = chosen_models(dir.path());
    builtin.ensure_started(&models).unwrap();
    let pid = builtin
        .wait_ready(Duration::from_secs(20))
        .await
        .pid
        .unwrap();
    // Arrêt long (SIGTERM ignoré) : une demande de démarrage pendant
    // l'arrêt ne lance rien.
    let stopping = tokio::spawn({
        let builtin = builtin.clone();
        async move { builtin.stop().await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    builtin.ensure_started(&models);
    stopping.await.unwrap();
    let snapshot = builtin.snapshot();
    assert_eq!(snapshot.state, BuiltinState::Stopped, "{snapshot:?}");
    assert!(!alive(pid));
    assert!(!config.pid_file().exists());

    // Suspendu : rien ne démarre jusqu'à la fin du garde.
    let paused = builtin.pause();
    builtin.ensure_started(&models);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(builtin.snapshot().state, BuiltinState::Stopped);
    assert!(!config.pid_file().exists());
    drop(paused);
    builtin.ensure_started(&models).unwrap();
    assert_eq!(builtin.snapshot().state, BuiltinState::Starting);
    builtin.stop().await;
}

/// Un dossier des modèles choisi qui n'existe pas (disque démonté) n'est
/// jamais recréé vide : échec qui le dit ; le dossier par défaut, lui, est
/// créé.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_chosen_folder_is_never_created() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let unplugged = dir.path().join("unplugged/models");
    builtin.ensure_started(&unplugged).unwrap();
    let failed = reaches(&builtin, Duration::from_secs(20), |snapshot| {
        snapshot.state == BuiltinState::Failed
    })
    .await;
    assert!(failed, "{:?}", builtin.snapshot());
    let failure = builtin.snapshot().failure.unwrap();
    assert!(failure.contains("doesn't exist"), "{failure}");
    assert!(!dir.path().join("unplugged").exists());

    let default = builtin.config().default_models_dir();
    builtin.restart(&default).await.unwrap();
    let snapshot = builtin.wait_ready(Duration::from_secs(20)).await;
    assert_eq!(snapshot.state, BuiltinState::Running, "{snapshot:?}");
    assert!(default.is_dir());
    builtin.stop().await;
}

/// Après `shutdown` (fermeture, installation d'une mise à jour), plus rien
/// ne relance l'intégré jusqu'à la reprise.
#[tokio::test(flavor = "multi_thread")]
async fn after_shutdown_nothing_starts_until_resumed() {
    let dir = temp_dir();
    let config = fake_config(dir.path(), &[]);
    let builtin = Builtin::new(config.clone());
    let models = chosen_models(dir.path());
    builtin.ensure_started(&models).unwrap();
    let pid = builtin
        .wait_ready(Duration::from_secs(20))
        .await
        .pid
        .unwrap();
    builtin.shutdown().await;
    assert_eq!(builtin.snapshot().state, BuiltinState::Stopped);
    assert!(eventually(Duration::from_secs(5), || !alive(pid)).await);
    builtin.ensure_started(&models);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(builtin.snapshot().state, BuiltinState::Stopped);
    assert!(!config.pid_file().exists());

    builtin.reopen();
    builtin.ensure_started(&models).unwrap();
    assert_eq!(builtin.snapshot().state, BuiltinState::Starting);
    builtin.stop().await;
}

/// Après SIGKILL, la récolte n'est attendue qu'un temps : un processus
/// bloqué dans le noyau ne fige pas ChatonLM (simulé par une fin jamais
/// signalée).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn the_wait_after_sigkill_is_bounded() {
    // SIGTERM ignoré (par le shell et son `sleep`) : seul SIGKILL l'arrête.
    let mut child = Command::new("sh")
        .args(["-c", "trap '' TERM; sleep 30"])
        .process_group(0)
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let (_never, mut exited) = oneshot::channel::<Option<ExitStatus>>();
    let started = Instant::now();
    let reaped = tokio::time::timeout(
        Duration::from_secs(10),
        terminate(
            child.id(),
            &mut exited,
            Duration::from_millis(100),
            Duration::from_millis(300),
        ),
    )
    .await
    .expect("terminate must give up");
    assert!(!reaped);
    assert!(started.elapsed() < Duration::from_secs(3));
    // SIGKILL a bien été envoyé au groupe.
    assert!(child.wait().unwrap().signal() == Some(libc::SIGKILL));
}

/// La fermeture rend la main même si l'arrêt ne finit jamais.
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_gives_up_on_a_stop_that_never_ends() {
    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[]));
    let (stop, _stop_seen) = watch::channel(false);
    let (_never_done, done) = watch::channel(false);
    *builtin.inner.run.lock() = Some(Run {
        generation: 1,
        stop,
        done,
    });
    let timing = builtin.config().timing.clone();
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(15), builtin.shutdown())
        .await
        .expect("shutdown must give up");
    assert!(
        started.elapsed() < timing.stop_grace + timing.kill_wait + Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert!(builtin.is_closed());
}

/// Version d'Ollama épinglée pour les paquets (`OLLAMA_VERSION` de
/// `packaging/ollama.env`) : celle que `scripts/fetch-ollama.sh` extrait.
fn pinned_version() -> &'static str {
    include_str!("../../../../packaging/ollama.env")
        .lines()
        .find_map(|line| line.strip_prefix("OLLAMA_VERSION="))
        .expect("OLLAMA_VERSION in packaging/ollama.env")
        .trim()
}

/// Le vrai Ollama intégré (`.tools/runtimes/ollama`, `scripts/fetch-ollama.sh`),
/// sur un port libre, données sous `.tools/tmp` : prêt, version livrée (celle
/// qui est épinglée), clé dans son HOME privé, puis arrêté sans reste. Passé
/// sans lui.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "contrat : Ollama intégré réel (scripts/fetch-ollama.sh)"]
async fn contract_the_real_builtin_ollama_starts_and_stops() {
    let bin =
        os::bundled_bin(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tools/runtimes/ollama"));
    let Ok(bin) = std::fs::canonicalize(bin) else {
        eprintln!("no built-in Ollama in .tools/runtimes: skipped");
        return;
    };
    let dir = temp_dir();
    let mut config = Config::new(Some(bin), dir.path());
    config.timing.ready_timeout = Duration::from_secs(60);
    let builtin = Builtin::new(config.clone());
    let pinned = pinned_version();
    assert_eq!(builtin.version().as_deref(), Some(pinned));
    builtin
        .ensure_started(&config.default_models_dir())
        .unwrap();
    let snapshot = builtin.wait_ready(Duration::from_secs(70)).await;
    let log = std::fs::read_to_string(&config.log).unwrap_or_default();
    assert_eq!(snapshot.state, BuiltinState::Running, "{snapshot:?}\n{log}");
    assert_eq!(snapshot.version.as_deref(), Some(pinned));
    let url = snapshot.url.unwrap();
    assert!(matches!(client::probe(&url).await, Probe::Ollama { version } if version == pinned));
    // Sa clé est dans son HOME privé.
    assert!(config.home().join(".ollama/id_ed25519").exists());
    let pid = snapshot.pid.unwrap();
    builtin.stop().await;
    assert!(eventually(Duration::from_secs(5), || !alive(pid)).await);
    // Plus aucun processus du groupe.
    #[cfg(unix)]
    {
        let ours = Ours {
            uid: std::fs::metadata("/proc/self").unwrap().uid(),
            home: config.home(),
        };
        assert!(leftover_processes(Path::new("/proc"), pid, &ours).is_empty());
    }
    eprintln!("{log}");
}
