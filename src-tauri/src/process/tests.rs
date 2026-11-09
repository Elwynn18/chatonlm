//! Essais des outils du sous-processus (port, environnement, arrêt borné).
//! Les essais de bout en bout (lancement, relance, PDEATHSIG, Job Object)
//! sont ceux de l'Ollama intégré (`ollama/builtin/tests.rs`) et du moteur
//! d'images (`image_gen/tests.rs`), qui s'en servent.

use std::ffi::OsString;
use std::net::{Ipv4Addr, TcpListener};
#[cfg(unix)]
use std::os::unix::process::{CommandExt, ExitStatusExt};
#[cfg(unix)]
use std::process::Command;
#[cfg(unix)]
use std::time::{Duration, Instant};

use super::*;

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

/// Lot L3 : un moteur de texte qui plante ne laisse pas de fichier `core`
/// (sa mémoire contient les conversations).
#[cfg(unix)]
#[test]
fn engines_leave_no_core_dump() {
    let mut command = Command::new("sh");
    command.args(["-c", "ulimit -c; ulimit -H -c"]);
    no_core_dump(&mut command);
    let output = command.output().unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout), "0\n0\n");
}
