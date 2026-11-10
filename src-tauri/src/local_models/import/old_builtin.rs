//! Ollama intégré de ChatonLM 0.9.x resté en vie (ChatonLM tué avant son
//! arrêt, puis mis à jour) : arrêté au lancement, avant que la reprise ne
//! déplace ses fichiers (sous Windows, un fichier ouvert ne se renomme pas).
//! Il n'est plus jamais démarré ; ses données restent pour la reprise.
//!
//! Seul indice : son fichier pid, `<données>/ollama/ollama.pid` (lu puis
//! effacé). Le processus n'est arrêté que s'il est bien notre ancien binaire,
//! reconnu par son chemin (`…/ollama/bin/ollama` sous Linux, avec le `HOME`
//! privé que ChatonLM lui donnait ; `<dossier de l'app>\ollama\ollama.exe`
//! sous Windows) : jamais un programme qui aurait repris le numéro, jamais
//! ChatonLM lui-même. Parties propres au système : `old_builtin/linux.rs`,
//! `old_builtin/windows.rs`.

#[cfg(target_os = "linux")]
#[path = "old_builtin/linux.rs"]
mod os;
#[cfg(windows)]
#[path = "old_builtin/windows.rs"]
mod os;

use std::fs;
use std::path::{Path, PathBuf};

/// Dossier de l'ancien intégré sous les données de l'app.
pub fn root(data_dir: &Path) -> PathBuf {
    data_dir.join("ollama")
}

/// Fichier pid écrit par l'ancien intégré à chaque lancement.
pub fn pid_file(data_dir: &Path) -> PathBuf {
    root(data_dir).join("ollama.pid")
}

/// Numéro lu dans le fichier pid ; `None` s'il est vide, illisible, ou
/// désigne ChatonLM, init ou 0.
pub fn parse_pid(text: &str) -> Option<u32> {
    let pid = text.trim().parse::<u32>().ok()?;
    (pid > 1 && pid != std::process::id()).then_some(pid)
}

/// Arrête l'ancien intégré noté dans le fichier pid s'il vit encore ; le
/// fichier est effacé dans tous les cas. Bloquant (attente bornée).
pub fn stop_leftover(data_dir: &Path) {
    let file = pid_file(data_dir);
    let Ok(text) = fs::read_to_string(&file) else {
        return;
    };
    let _ = fs::remove_file(&file);
    if let Some(pid) = parse_pid(&text) {
        os::stop(pid, data_dir);
    }
}

/// Windows : `image` est l'ancien intégré livré à côté de ChatonLM
/// (`<install_dir>\ollama\ollama.exe`, casse et préfixe `\\?\` ignorés).
/// Logique pure, essayée sous Linux.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn is_builtin_image(image: &str, install_dir: &str) -> bool {
    let expected = format!(r"{}\ollama\ollama.exe", super::winpath::plain(install_dir));
    super::winpath::volume(image).is_some() && super::winpath::same(image, &expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pid_file_never_designates_chatonlm_or_init() {
        assert_eq!(parse_pid(" 4242\n"), Some(4242));
        for text in ["", "abc", "0", "1", "-5", &std::process::id().to_string()] {
            assert_eq!(parse_pid(text), None, "{text:?}");
        }
    }

    #[test]
    fn the_pid_file_is_removed_even_without_a_process() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(root(dir.path())).unwrap();
        fs::write(pid_file(dir.path()), "not a pid").unwrap();
        stop_leftover(dir.path());
        assert!(!pid_file(dir.path()).exists());
        // Sans fichier : rien.
        stop_leftover(dir.path());
    }

    #[test]
    fn only_the_bundled_binary_is_the_old_builtin_on_windows() {
        let install = r"C:\Users\me\AppData\Local\ChatonLM";
        assert!(is_builtin_image(
            r"\\?\c:\users\me\appdata\local\chatonlm\ollama\ollama.exe",
            install
        ));
        for other in [
            r"C:\Users\me\AppData\Local\Programs\Ollama\ollama.exe",
            r"C:\Users\me\AppData\Local\ChatonLM\chatonlm.exe",
            r"C:\Users\me\AppData\Local\ChatonLM\ollama\other.exe",
            r"D:\Users\me\AppData\Local\ChatonLM\ollama\ollama.exe",
            "ollama.exe",
        ] {
            assert!(!is_builtin_image(other, install), "{other}");
        }
    }

    #[cfg(target_os = "linux")]
    mod linux {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::process::CommandExt;
        use std::path::Path;
        use std::process::{Child, Command};
        use std::time::{Duration, Instant};

        use super::super::os::{OldBuiltin, is_builtin_binary, leftover_processes, stop_with};
        use super::*;

        fn alive(pid: u32) -> bool {
            let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            stat.rfind(')')
                .is_some_and(|end| !stat[end + 1..].trim_start().starts_with('Z'))
        }

        fn eventually(limit: Duration, done: impl Fn() -> bool) -> bool {
            let deadline = Instant::now() + limit;
            while Instant::now() < deadline {
                if done() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            done()
        }

        /// Copie de `sleep` sous `<dir>/<relative>` (même exécutable que
        /// l'ancien intégré pour `/proc/<pid>/exe`).
        fn binary(dir: &Path, relative: &str) -> std::path::PathBuf {
            let source = ["/usr/bin/sleep", "/bin/sleep"]
                .into_iter()
                .find(|path| Path::new(path).exists())
                .unwrap();
            let target = dir.join(relative);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::copy(source, &target).unwrap();
            target
        }

        /// Processus `bin 30` avec ce `HOME`, dans le groupe `group` (0 :
        /// le sien) ; SIGTERM ignoré si `deaf` (hérité à travers `exec`).
        fn spawn(bin: &Path, home: &Path, group: i32, deaf: bool) -> Child {
            let mut command = Command::new(bin);
            command.arg("120").env("HOME", home).process_group(group);
            if deaf {
                // SAFETY : seul appel async-signal-safe entre fork et exec.
                unsafe {
                    command.pre_exec(|| {
                        libc::signal(libc::SIGTERM, libc::SIG_IGN);
                        Ok(())
                    });
                }
            }
            // Binaire tout juste copié : un fork d'un autre essai peut tenir
            // un instant le fichier ouvert (ETXTBSY).
            for _ in 0..50 {
                match command.spawn() {
                    Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    result => return result.unwrap(),
                }
            }
            command.spawn().unwrap()
        }

        #[test]
        fn the_binary_is_recognized_even_once_deleted() {
            assert!(is_builtin_binary(Path::new(
                "/usr/lib/chatonlm/ollama/bin/ollama"
            )));
            assert!(is_builtin_binary(Path::new(
                "/tmp/.mount_ChatoX/usr/lib/chatonlm/ollama/bin/ollama (deleted)"
            )));
            assert!(!is_builtin_binary(Path::new("/usr/bin/ollama")));
            assert!(!is_builtin_binary(Path::new(
                "/usr/lib/chatonlm/llama-server"
            )));
        }

        /// Contrat 103 § 2.3 : l'ancien intégré resté en vie (son serveur
        /// et un runner de son groupe, SIGTERM ignoré) est arrêté par son
        /// fichier pid seul, sans binaire livré ; un autre programme (numéro
        /// repris, autre `HOME`, autre exécutable) n'est jamais touché.
        #[test]
        fn an_old_builtin_left_running_is_stopped_by_its_pid_file() {
            let dir = tempfile::tempdir().unwrap();
            let data = dir.path().join("data");
            let home = root(&data).join("home");
            fs::create_dir_all(&home).unwrap();
            let bin = binary(dir.path(), "res/ollama/bin/ollama");
            let proc = Path::new("/proc");
            let ours = OldBuiltin {
                uid: fs::metadata("/proc/self").unwrap().uid(),
                home: home.clone(),
            };

            // Numéro repris par un autre programme : jamais touché.
            let other = binary(dir.path(), "res/other/sleep");
            for (bin, home) in [(&other, home.as_path()), (&bin, dir.path())] {
                let mut foreign = spawn(bin, home, 0, false);
                fs::write(pid_file(&data), foreign.id().to_string()).unwrap();
                stop_leftover(&data);
                assert!(!pid_file(&data).exists());
                // Laissé en vie (le délai de SIGKILL est passé).
                std::thread::sleep(Duration::from_millis(300));
                assert!(alive(foreign.id()));
                foreign.kill().unwrap();
                foreign.wait().unwrap();
            }

            // L'ancien intégré et son runner.
            let mut server = spawn(&bin, &home, 0, true);
            let group = i32::try_from(server.id()).unwrap();
            let mut runner = spawn(&bin, &home, group, true);
            assert!(eventually(Duration::from_secs(5), || {
                leftover_processes(proc, server.id(), &ours).len() == 2
            }));
            fs::write(pid_file(&data), server.id().to_string()).unwrap();
            stop_with(proc, server.id(), &ours, Duration::from_millis(200));
            // Arrêtés (SIGKILL après le délai), pas finis d'eux-mêmes.
            assert!(eventually(Duration::from_secs(10), || {
                !alive(server.id()) && !alive(runner.id())
            }));
            server.wait().unwrap();
            runner.wait().unwrap();

            // Au lancement de l'app : par le fichier pid (délai normal).
            let mut server = spawn(&bin, &home, 0, false);
            assert!(eventually(Duration::from_secs(5), || ours.owns(proc, server.id())));
            fs::write(pid_file(&data), server.id().to_string()).unwrap();
            stop_leftover(&data);
            assert!(!pid_file(&data).exists());
            assert!(eventually(Duration::from_secs(10), || !alive(server.id())));
            server.wait().unwrap();
        }
    }
}
