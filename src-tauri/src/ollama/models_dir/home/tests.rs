//! Dossier `.ollama` caché : détection pure, rendu du drop-in et des
//! commandes, fonctions communes aux deux scripts (identiques, et d'accord
//! avec Rust), codes de sortie, et banc du script `home.sh` lancé par
//! [`fix_with`] à travers le faux pkexec (sous `/bin/sh` et `dash` s'il
//! existe ; racine simulée, rien d'exécuté en root, rien d'écrit hors du
//! dossier temporaire du projet ; chemins avec une espace et un accent).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::super::tests::{find_dash, temp_dir};
use super::super::{Detection, Env, Facts, Listener, ProcessFacts, SHELL, UnitFacts, classify};
use super::*;
use crate::ollama::host::ServiceDetails;

// --- Détection -----------------------------------------------------------------

#[test]
fn protect_home_hides_home_root_and_run_user() {
    for (path, root) in [
        ("/home/alice/.ollama", Some("/home")),
        ("/home/.ollama", Some("/home")),
        ("/root/.ollama", Some("/root")),
        ("/run/user/1000/.ollama", Some("/run/user")),
        ("/homework/.ollama", None),
        ("/home", None),
        ("/var/lib/ollama/.ollama", None),
        ("/usr/share/ollama/.ollama", None),
        ("/run/.ollama", None),
    ] {
        assert_eq!(hidden_root(path), root, "{path}");
    }
}

/// Essais de l'app réelle : le /home simulé (CHATONLM_TEST_HOME) compte comme
/// /home, comme dans les scripts ; jamais en dehors de la détection imposée.
#[test]
fn the_simulated_home_counts_as_home_for_injected_facts() {
    let test_home = Some("/data/run é/home/");
    let dir = "/data/run é/home/théo/.ollama";
    let hidden = hidden_home_in(
        &system("alice", true, true),
        Some(dir),
        "yes",
        &[],
        test_home,
    )
    .unwrap();
    assert_eq!(hidden.hidden_root, "/home");
    assert_eq!(hidden.dir, dir);
    assert_eq!(
        hidden_home(&system("alice", true, true), Some(dir), "yes", &[]),
        None
    );
    for other in ["/data/run é/homework/.ollama", "/data/run é/home"] {
        assert_eq!(
            hidden_home_in(
                &system("alice", true, true),
                Some(other),
                "yes",
                &[],
                test_home
            ),
            None,
            "{other}"
        );
    }
    assert_eq!(
        hidden_home_in(
            &system("alice", true, true),
            Some(dir),
            "tmpfs",
            &[dir.into()],
            test_home
        ),
        None
    );
}

#[test]
fn the_service_home_is_its_environment_then_its_user() {
    // Cette machine : HOME= de l'override.conf.
    assert_eq!(
        service_ollama_dir(Some("/home/alice"), "alice", Some("/home/other")).as_deref(),
        Some("/home/alice/.ollama")
    );
    // Un seul / final retiré, comme ${h%/} des scripts.
    assert_eq!(
        service_ollama_dir(Some("/home/alice/"), "alice", None).as_deref(),
        Some("/home/alice/.ollama")
    );
    // Sans HOME= : répertoire de User= (paquet Arch : /var/lib/ollama).
    assert_eq!(
        service_ollama_dir(None, "ollama", Some("/var/lib/ollama")).as_deref(),
        Some("/var/lib/ollama/.ollama")
    );
    assert_eq!(
        service_ollama_dir(Some(""), "root", Some("/root")).as_deref(),
        Some("/root/.ollama")
    );
    // Service root sans User= : systemd ne pose pas HOME.
    assert_eq!(service_ollama_dir(None, "", Some("/root")), None);
    assert_eq!(service_ollama_dir(Some("/"), "alice", None), None);
}

#[test]
fn hidden_only_by_yes_or_tmpfs_without_a_bind() {
    let dir = "/home/alice/.ollama";
    let binds = |items: &[&str]| {
        items
            .iter()
            .map(|item| (*item).to_owned())
            .collect::<Vec<_>>()
    };
    let cases: [(&str, Vec<String>, bool); 12] = [
        ("yes", binds(&[]), true),
        ("true", binds(&[]), true),
        ("tmpfs", binds(&[]), true),
        // Lisible (read-only : on considère que c'est bon), ou pas masqué.
        ("read-only", binds(&[]), false),
        ("no", binds(&[]), false),
        ("", binds(&[]), false),
        // tmpfs : un montage du dossier ou d'un parent le rend.
        ("tmpfs", binds(&["/home/alice/.ollama"]), false),
        ("tmpfs", binds(&["/home/alice/"]), false),
        ("tmpfs", binds(&["/"]), false),
        // Un sous-dossier (modèles) ne rend pas la clé ; un voisin non plus.
        (
            "tmpfs",
            binds(&["/home/alice/.ollama/models", "/home/alice/.ollama2"]),
            true,
        ),
        ("tmpfs", binds(&["/home/alice/LLM models"]), true),
        // yes : systemd ne peut rien monter sous /home.
        ("yes", binds(&["/home/alice/.ollama"]), true),
    ];
    for (protect_home, binds, hidden) in cases {
        assert_eq!(
            is_hidden_in(dir, protect_home, &binds, None),
            hidden,
            "{protect_home} {binds:?}"
        );
    }
    // Hors de /home, /root et /run/user : jamais caché.
    assert!(!is_hidden_in("/var/lib/ollama/.ollama", "yes", &[], None));
    assert!(is_hidden_in("/root/.ollama", "yes", &[], None));
    assert!(is_hidden_in("/run/user/1000/.ollama", "tmpfs", &[], None));
}

fn system(user: &str, running: bool, can_elevate: bool) -> ModelsLocation {
    ModelsLocation::SystemService {
        dir: Some("/data/models".into()),
        user: user.into(),
        group: user.into(),
        running,
        managed: false,
        can_elevate,
        env_file: None,
    }
}

#[test]
fn only_the_system_service_gets_an_alert() {
    let hidden = hidden_home(
        &system("alice", true, true),
        Some("/home/alice/.ollama"),
        "yes",
        &[],
    )
    .unwrap();
    assert_eq!(
        hidden,
        HiddenHome {
            dir: "/home/alice/.ollama".into(),
            hidden_root: "/home".into(),
            user: "alice".into(),
            running: true,
            can_elevate: true,
            commands: Some(sudo_commands("/home/alice/.ollama", "alice", true)),
        }
    );
    let stopped = hidden_home(
        &system("ollama", false, false),
        Some("/run/user/961/.ollama"),
        "tmpfs",
        &[],
    )
    .unwrap();
    assert_eq!(stopped.hidden_root, "/run/user");
    assert!(!stopped.running && !stopped.can_elevate);
    assert!(
        stopped
            .commands
            .unwrap()
            .ends_with("\nsudo systemctl daemon-reload")
    );
    // Service root : l'alerte, mais ni Fix (le script refuse) ni commandes
    // (sudo -u root mkdir, BindPaths= qu'un lien détournerait).
    let root = hidden_home(
        &system("root", true, true),
        Some("/root/.ollama"),
        "yes",
        &[],
    )
    .unwrap();
    assert_eq!(root.hidden_root, "/root");
    assert_eq!(root.user, "root");
    assert_eq!(root.commands, None);
    let root_home = hidden_home(
        &system("root", true, true),
        Some("/home/alice/.ollama"),
        "yes",
        &[],
    )
    .unwrap();
    assert_eq!(root_home.commands, None);
    for location in [
        ModelsLocation::UserService { dir: None },
        ModelsLocation::Manual {
            dir: None,
            host_env: None,
        },
        ModelsLocation::Container,
        ModelsLocation::Remote { host: "box".into() },
        ModelsLocation::Unknown { running: true },
    ] {
        assert_eq!(
            hidden_home(&location, Some("/home/alice/.ollama"), "yes", &[]),
            None
        );
    }
    let service = system("alice", true, true);
    // Visible, sans HOME, ou HOME que systemd interpréterait.
    assert_eq!(
        hidden_home(&service, Some("/home/alice/.ollama"), "read-only", &[]),
        None
    );
    assert_eq!(hidden_home(&service, None, "yes", &[]), None);
    assert_eq!(
        hidden_home(&service, Some("/home/a:b/.ollama"), "yes", &[]),
        None
    );
}

fn unit_facts(protect_home: &str, binds: &[&str], environment: &[&str]) -> UnitFacts {
    let service = ServiceDetails {
        active_state: "active".into(),
        main_pid: 4000,
        environment: environment.iter().map(|item| (*item).to_owned()).collect(),
        user: "alice".into(),
        group: "alice".into(),
        protect_home: protect_home.into(),
        protect_system: "full".into(),
        binds: binds
            .iter()
            .map(|bind| ((*bind).to_owned(), (*bind).to_owned()))
            .collect(),
        ..ServiceDetails::default()
    };
    UnitFacts {
        // Sources présentes.
        visible_binds: binds.iter().map(|bind| (*bind).to_owned()).collect(),
        env: Env::from_assignments(environment.iter().copied()),
        user_home: Some("/home/alice".into()),
        user_uid: Some(1000),
        main_uid: Some(1000),
        service,
        ..UnitFacts::default()
    }
}

/// Détection complète : le service de cette machine (User=alice,
/// HOME=/home/alice, ProtectHome=yes de l'unité d'Arch).
fn detect_with(unit: UnitFacts) -> Detection {
    let facts = Facts {
        own_uid: 1000,
        listener: Some(Listener {
            uid: 1000,
            inode: 42,
        }),
        process: Some(ProcessFacts {
            cgroup: "/system.slice/ollama.service".into(),
            exe: "ollama".into(),
            env: Env::default(),
        }),
        unit: Some(unit),
        answers: false,
        can_elevate: true,
    };
    classify(&url::Url::parse("http://127.0.0.1:11434").unwrap(), &facts)
}

#[test]
fn the_service_of_this_machine_is_detected() {
    let environment = [
        "HOME=/home/alice",
        "OLLAMA_MODELS=/data/Logiciel/Ollama/models",
    ];
    let detection = detect_with(unit_facts("yes", &[], &environment));
    assert_eq!(detection.ollama_dir.as_deref(), Some("/home/alice/.ollama"));
    let hidden = detection.hidden_home.unwrap();
    assert_eq!(hidden.dir, "/home/alice/.ollama");
    assert_eq!(hidden.user, "alice");
    // Son zz-home.conf (read-only), ou la correction de l'app : rien.
    assert_eq!(
        detect_with(unit_facts("read-only", &[], &environment)).hidden_home,
        None
    );
    assert_eq!(
        detect_with(unit_facts("tmpfs", &["/home/alice/.ollama"], &environment)).hidden_home,
        None
    );
    // Dossier des modèles sous /home monté seul : la clé reste cachée.
    assert!(
        detect_with(unit_facts(
            "tmpfs",
            &["/home/alice/LLM models"],
            &environment
        ))
        .hidden_home
        .is_some()
    );
    // Paquet Arch sans override : HOME=/var/lib/ollama, rien à signaler.
    let arch = ["HOME=/var/lib/ollama", "OLLAMA_MODELS=/var/lib/ollama"];
    let detection = detect_with(unit_facts("yes", &[], &arch));
    assert_eq!(
        detection.ollama_dir.as_deref(),
        Some("/var/lib/ollama/.ollama")
    );
    assert_eq!(detection.hidden_home, None);
}

/// `.ollama` effacé après la correction : le montage reste, sa source a
/// disparu (systemd refuse alors de démarrer le service) ; même alerte, et
/// Fix le recrée.
#[test]
fn a_bind_whose_source_is_gone_hides_the_folder_again() {
    let dir = "/home/alice/.ollama";
    let binds = vec![
        (dir.to_owned(), dir.to_owned()),
        (
            "/data/models".to_owned(),
            "/home/alice/LLM models".to_owned(),
        ),
    ];
    assert_eq!(
        visible_binds(&binds, |source| source != dir),
        vec!["/home/alice/LLM models".to_owned()]
    );
    let environment = ["HOME=/home/alice"];
    let mut unit = unit_facts("tmpfs", &[dir], &environment);
    assert_eq!(detect_with(unit.clone()).hidden_home, None);
    unit.visible_binds = visible_binds(&unit.service.binds, |_| false);
    let hidden = detect_with(unit).hidden_home.unwrap();
    assert_eq!(hidden.dir, dir);
    assert!(
        hidden
            .commands
            .unwrap()
            .contains("mkdir -p '/home/alice/.ollama'")
    );
    // Sur le disque : présent, absent, lien cassé (systemd suit les liens).
    let tmp = temp_dir();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let present = base.join("present");
    fs::create_dir_all(&present).unwrap();
    let broken = base.join("broken");
    std::os::unix::fs::symlink(base.join("gone"), &broken).unwrap();
    assert!(source_exists(present.to_str().unwrap()));
    assert!(!source_exists(base.join("missing").to_str().unwrap()));
    assert!(!source_exists(broken.to_str().unwrap()));
}

#[test]
fn injected_facts_follow_the_fake_service() {
    let tmp = temp_dir();
    let protect = tmp.path().join("loaded protect home");
    let binds = tmp.path().join("loaded binds");
    let facts = format!(
        "launch=system;user=alice;running=yes;ollama_dir=/home/alice/.ollama;protect_home=@{};binds=@{}",
        protect.display(),
        binds.display()
    );
    let hidden = || parse_injected(&facts).unwrap().hidden_home;
    // Fichier absent : ProtectHome= inconnu, rien à signaler.
    assert_eq!(hidden(), None);
    fs::write(&protect, "yes\n").unwrap();
    assert_eq!(hidden().unwrap().dir, "/home/alice/.ollama");
    fs::write(&protect, "tmpfs\n").unwrap();
    fs::write(&binds, "/home/alice/LLM models\n").unwrap();
    assert!(hidden().is_some());
    // Montage d'un parent dont la source existe (/ : toujours là).
    fs::write(&binds, "/home/alice/LLM models\n/\n").unwrap();
    assert_eq!(hidden(), None);
    // Montage dont la source n'existe pas : rien de rendu.
    let gone = tmp.path().join("gone");
    fs::write(&binds, format!("{}\n", gone.display())).unwrap();
    assert!(hidden().is_some());
    // Valeurs données telles quelles.
    let literal = parse_injected(
        "launch=system;user=alice;running=no;ollama_dir=/home/alice/.ollama;protect_home=tmpfs;binds=/",
    )
    .unwrap();
    assert_eq!(literal.hidden_home, None);
    assert_eq!(literal.ollama_dir.as_deref(), Some("/home/alice/.ollama"));
}

fn parse_injected(value: &str) -> Option<Detection> {
    super::super::parse_injected(value, true)
}

// --- Drop-in et commandes ---------------------------------------------------------

#[test]
fn the_dropin_binds_only_the_ollama_folder() {
    assert_eq!(
        dropin_text("/home/théo/100% sure/.ollama"),
        "# Written by ChatonLM (Settings > General): Ollama's own folder.\n[Service]\nProtectHome=tmpfs\nBindPaths=\"/home/théo/100%% sure/.ollama\"\n"
    );
}

#[test]
fn sudo_commands_do_what_the_script_does() {
    assert_eq!(
        sudo_commands("/home/alice/.ollama", "alice", true),
        "sudo -u alice mkdir -p '/home/alice/.ollama'\n\
         sudo mkdir -p /etc/systemd/system/ollama.service.d\n\
         printf '%s\\n' '# Written by ChatonLM (Settings > General): Ollama'\\''s own folder.' '[Service]' 'ProtectHome=tmpfs' 'BindPaths=\"/home/alice/.ollama\"' | sudo tee /etc/systemd/system/ollama.service.d/zz-chatonlm-home.conf > /dev/null\n\
         sudo systemctl daemon-reload && sudo systemctl restart ollama.service"
    );
    // Exécutées dans un dossier temporaire (faux sudo et systemctl, /etc
    // remplacé) : dossier créé, drop-in octet pour octet, service relancé.
    let tmp = temp_dir();
    let dir = fs::canonicalize(tmp.path())
        .unwrap()
        .join("home/It's 100% «sûr»/.ollama");
    fs::create_dir_all(dir.parent().unwrap()).unwrap();
    let dir = dir.to_str().unwrap();
    let commands = sudo_commands(dir, "alice", true).replace(
        "/etc/systemd/system/ollama.service.d",
        "etc/ollama.service.d",
    );
    let fakes = "sudo() { if [ \"$1\" = -u ]; then shift 2; fi; \"$@\"; }\n\
                 systemctl() { echo \"$*\" >> systemctl.log; }\n";
    let output = Command::new(SHELL)
        .args(["-c", &format!("{fakes}{commands}")])
        .current_dir(tmp.path())
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(Path::new(dir).is_dir());
    assert_eq!(
        fs::read_to_string(
            tmp.path()
                .join("etc/ollama.service.d/zz-chatonlm-home.conf")
        )
        .unwrap(),
        dropin_text(dir)
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("systemctl.log")).unwrap(),
        "daemon-reload\nrestart ollama.service\n"
    );
}

// --- Fonctions communes aux deux scripts -----------------------------------------

/// Texte d'une fonction de `script` (`nom() {` … `}` en début de ligne, ou
/// sur une ligne).
fn function_in(script: &'static str, name: &str) -> &'static str {
    if let Some(start) = script.find(&format!("\n{name}() {{\n")) {
        let start = start + 1;
        let end = start + script[start..].find("\n}\n").unwrap() + 3;
        return &script[start..end];
    }
    let start = script
        .find(&format!("\n{name}() {{ "))
        .unwrap_or_else(|| panic!("fonction {name} absente"))
        + 1;
    let end = start + script[start..].find('\n').unwrap() + 1;
    &script[start..end]
}

#[test]
fn shared_functions_are_identical_in_both_scripts() {
    for name in [
        "plain_path",
        "in_home",
        "prop",
        "env_items",
        "service_ollama_dir",
        "as_svc",
        "ollama_dir_ready",
    ] {
        assert_eq!(
            function_in(super::super::SCRIPT, name),
            function_in(SCRIPT, name),
            "{name} diffère entre models_dir.sh et home.sh"
        );
    }
}

fn shells() -> Vec<PathBuf> {
    let mut shells = vec![PathBuf::from(SHELL)];
    shells.extend(find_dash());
    shells
}

/// Sortie de `program` (fonctions du script, faux `prop` et `getent`) sous
/// `shell`.
fn run_functions(shell: &Path, program: &str, args: &[&str]) -> String {
    let output = Command::new(shell)
        .arg("-c")
        .arg(program)
        .arg("sh")
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn plain_paths_and_home_folders_match_rust() {
    let cases = [
        "/home/alice/.ollama",
        "/home/théo/My files 100%/.ollama",
        "/root/.ollama",
        "/run/user/1000/.ollama",
        "/homework/.ollama",
        "/var/lib/ollama/.ollama",
        "/home/a:b/.ollama",
        "/home/quote\"d",
        "/home/dollar$x",
        "/home/tick`x",
        "/home/new\nline",
        "relative/.ollama",
    ];
    let program = format!(
        "TEST_HOME=''\n{}{}\nif plain_path \"$1\"; then printf p; fi\nif in_home \"$1\"; then printf h; fi",
        function_in(SCRIPT, "plain_path"),
        function_in(SCRIPT, "in_home")
    );
    for shell in shells() {
        for path in cases {
            let expected = format!(
                "{}{}",
                if super::super::plain_path(path) {
                    "p"
                } else {
                    ""
                },
                if hidden_root(path).is_some() { "h" } else { "" }
            );
            assert_eq!(
                run_functions(&shell, &program, &[path]),
                expected,
                "{path:?} sous {}",
                shell.display()
            );
        }
    }
}

#[test]
fn the_scripts_read_home_like_rust() {
    // Ligne de `systemctl show -p Environment --value` (shell_maybe_quote :
    // guillemets autour d'un élément avec espace, " \ ` $ échappés) ; `User=`
    // et `getent passwd` simulés.
    let cases: [(&str, &str, Option<&str>); 7] = [
        (
            "HOME=/home/alice OLLAMA_MODELS=/data/Logiciel/Ollama/models",
            "alice",
            Some("/home/alice/.ollama"),
        ),
        (
            "\"HOME=/home/My Name\" \"OLLAMA_MODELS=/data/a b\" PATH=/usr/bin",
            "alice",
            Some("/home/My Name/.ollama"),
        ),
        (
            "\"FOO=x HOME=/home/evil y\" HOME=/home/théo/",
            "alice",
            Some("/home/théo/.ollama"),
        ),
        (
            "\"X=say \\\"hi\\\" \\$HOME\" HOME=/srv/ollama",
            "ollama",
            Some("/srv/ollama/.ollama"),
        ),
        // Sans HOME= : répertoire de User= (getent).
        (
            "OLLAMA_MODELS=/var/lib/ollama",
            "ollama",
            Some("/home/from-passwd/.ollama"),
        ),
        ("", "ollama", Some("/home/from-passwd/.ollama")),
        // Ni HOME= ni User= : rien.
        ("OLLAMA_MODELS=/m", "", None),
    ];
    let program = format!(
        "prop() {{ case $1 in Environment) printf '%s\\n' \"$ENV_LINE\" ;; User) printf '%s\\n' \"$USER_\" ;; esac; }}\n\
         getent() {{ printf '%s:x:961:961::/home/from-passwd/:/bin/false\\n' \"$2\"; }}\n\
         {}{}SUSER=${{USER_:-root}}\nservice_ollama_dir",
        function_in(SCRIPT, "env_items"),
        function_in(SCRIPT, "service_ollama_dir"),
    );
    for shell in shells() {
        for (line, user, expected) in cases {
            let output = Command::new(&shell)
                .args(["-c", &program])
                .env("LC_ALL", "C")
                .env("ENV_LINE", line)
                .env("USER_", user)
                .output()
                .unwrap();
            let said = String::from_utf8(output.stdout).unwrap();
            assert_eq!(
                said.strip_suffix('\n'),
                expected,
                "{line:?} sous {}",
                shell.display()
            );
            // Rust, d'après les éléments que donne D-Bus (déjà séparés).
            if !line.contains('"') {
                let env = Env::from_assignments(line.split(' '));
                let rust =
                    service_ollama_dir(env.home.as_deref(), user, Some("/home/from-passwd/"));
                assert_eq!(rust.as_deref(), expected, "{line:?} (Rust)");
            }
        }
    }
}

/// Vérification du montage après daemon-reload : l'entrée entière, sous
/// toutes les formes plausibles de `systemctl show -p BindPaths --value`
/// (une par ligne ou séparées par des espaces, destination répétée ou non,
/// options, « - »), jamais un sous-dossier, un voisin ou une autre source.
#[test]
fn the_bind_is_checked_whole() {
    let dir = "/home/théo é/.ollama";
    let program = format!(
        "prop() {{ printf '%s\\n' \"$OUT\"; }}\n{}bound \"$1\"",
        function_in(SCRIPT, "bound")
    );
    let cases = [
        (format!("{dir}:{dir}:rbind"), true),
        (format!("{dir}:{dir}:norbind"), true),
        (format!("{dir}:{dir}"), true),
        (format!("{dir}:rbind"), true),
        (dir.to_owned(), true),
        (format!("-{dir}:{dir}:rbind"), true),
        (format!("/data/m:/data/m:rbind\n{dir}:{dir}:rbind"), true),
        (
            format!("/data/m:/data/m:rbind {dir}:{dir}:rbind /x:/x"),
            true,
        ),
        (format!("{dir}/models:{dir}/models:rbind"), false),
        (format!("{dir}2:{dir}2:rbind"), false),
        (format!("/srv/other:{dir}:rbind"), false),
        (format!("/home/x{dir}:/home/x{dir}:rbind"), false),
        (String::new(), false),
    ];
    for shell in shells() {
        for (out, bound) in &cases {
            let status = Command::new(&shell)
                .args(["-c", &program, "sh", dir])
                .env("LC_ALL", "C")
                .env("OUT", out)
                .status()
                .unwrap();
            assert_eq!(status.success(), *bound, "{out:?} sous {}", shell.display());
        }
    }
}

// --- Codes de sortie ----------------------------------------------------------------

#[test]
fn each_exit_code_has_its_outcome() {
    use HomeFixFailure::*;
    let failed = |reason, restored| HomeFixOutcome::Failed {
        reason,
        restored,
        detail: "d".into(),
    };
    assert_eq!(
        outcome(Some(0), "d", Some((true, true))),
        HomeFixOutcome::Done {
            changed: true,
            restarted: true
        }
    );
    assert_eq!(outcome(Some(0), "d", None), failed(Unexpected, false));
    assert_eq!(outcome(Some(126), "d", None), HomeFixOutcome::Cancelled);
    assert_eq!(
        outcome(Some(127), "No authentication agent found.", None),
        HomeFixOutcome::Failed {
            reason: NoAgent,
            restored: false,
            detail: "No authentication agent found.".into()
        }
    );
    for (code, expected) in [
        (127, failed(NotAuthorized, false)),
        (10, failed(Unexpected, false)),
        (11, failed(UnitChanged, false)),
        (12, failed(NotYours, false)),
        (15, failed(CannotCreate, false)),
        (17, failed(Unexpected, false)),
        (22, failed(ConfigFailed, true)),
        (23, failed(Overridden, true)),
        (24, failed(StartFailed, true)),
        (30, failed(RestoreFailed, false)),
    ] {
        assert_eq!(outcome(Some(code), "d", None), expected, "{code}");
    }
    assert_eq!(outcome(None, "d", None), failed(Unexpected, false));
    assert_eq!(
        parse_result("result changed=yes restarted=no"),
        Some((true, false))
    );
    assert_eq!(parse_result("result changed=no"), None);
    assert_eq!(parse_result("step restart"), None);
}

#[test]
fn the_script_is_passed_whole_in_argv() {
    assert!(SCRIPT.starts_with("# ChatonLM: Ollama home access\n"));
    assert!(SCRIPT.len() < 128 * 1024);
    for shell in shells() {
        let status = Command::new(&shell)
            .args(["-n", "-c", SCRIPT])
            .status()
            .unwrap();
        assert!(status.success(), "syntaxe refusée par {}", shell.display());
    }
}

// --- Banc du script -------------------------------------------------------------

fn fake_bin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ollama/models_dir/fake-bin")
}

/// Un cas : racine simulée, faux service actif (PID 4000) dont le HOME est
/// `<cas>/home/alice` (sous le /home simulé), `ProtectHome=yes`.
struct Case {
    name: &'static str,
    shell: PathBuf,
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    root: PathBuf,
    home: PathBuf,
    ollama: PathBuf,
    envs: Vec<(OsString, OsString)>,
}

struct Ran {
    outcome: HomeFixOutcome,
    lines: Vec<String>,
    restarts: usize,
}

impl Ran {
    fn failure(&self) -> Option<(HomeFixFailure, bool)> {
        match &self.outcome {
            HomeFixOutcome::Failed {
                reason, restored, ..
            } => Some((*reason, *restored)),
            _ => None,
        }
    }

    fn detail_has(&self, text: &str) -> bool {
        matches!(&self.outcome, HomeFixOutcome::Failed { detail, .. } if detail.contains(text))
    }
}

impl Case {
    fn new(name: &'static str, shell: &Path) -> Case {
        let tmp = temp_dir();
        let dir = fs::canonicalize(tmp.path()).unwrap().join("cas é");
        let root = dir.join("root");
        for sub in ["run", "log", "state", "proc/4000", "etc/systemd/system"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        let home = dir.join("home/alice é");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(dir.join("models")).unwrap();
        let case = Case {
            name,
            shell: shell.to_path_buf(),
            _tmp: tmp,
            envs: vec![
                ("CHATONLM_TEST_ROOT".into(), root.clone().into_os_string()),
                ("CHATONLM_TEST_BIN".into(), fake_bin().into_os_string()),
                (
                    "CHATONLM_TEST_HOME".into(),
                    dir.join("home").into_os_string(),
                ),
            ],
            ollama: home.join(".ollama"),
            dir,
            root,
            home,
        };
        case.state("base_models", case.dir.join("models").to_str().unwrap());
        case.state("active", "active");
        case.state("pid", "4000");
        case.state("home", case.home.to_str().unwrap());
        case.state("protect_home", "yes");
        case
    }

    fn set(&mut self, name: &str, value: &str) {
        self.envs.push((name.into(), value.into()));
    }

    fn state(&self, name: &str, value: &str) {
        fs::write(self.root.join("state").join(name), format!("{value}\n")).unwrap();
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.root.join("log").join(format!("{name}.log"))).unwrap_or_default()
    }

    /// Appels à systemctl qui changent quelque chose.
    fn actions(&self) -> Vec<String> {
        self.log("systemctl")
            .lines()
            .filter(|line| !line.starts_with("show") && !line.starts_with("is-active"))
            .map(str::to_owned)
            .collect()
    }

    fn dropins(&self) -> PathBuf {
        self.root.join("etc/systemd/system/ollama.service.d")
    }

    fn dropin(&self) -> PathBuf {
        self.dropins().join(DROPIN_NAME)
    }

    fn ollama_str(&self) -> &str {
        self.ollama.to_str().unwrap()
    }

    async fn run(&self, dir: &str) -> Ran {
        let request = FixRequest {
            pkexec: fake_bin().join("pkexec"),
            shell: self.shell.clone(),
            dir: dir.to_owned(),
            envs: self.envs.clone(),
        };
        let mut lines = Vec::new();
        let mut restarts = 0;
        let outcome = fix_with(
            request,
            || restarts += 1,
            |line| lines.push(line.to_owned()),
        )
        .await;
        Ran {
            outcome,
            lines,
            restarts,
        }
    }
}

struct Checks {
    failures: Vec<String>,
    passed: usize,
}

impl Checks {
    fn check(&mut self, case: &Case, what: &str, ok: bool, ran: Option<&Ran>) {
        if ok {
            self.passed += 1;
            return;
        }
        let detail = ran.map_or(String::new(), |ran| format!(" ({:?})", ran.outcome));
        self.failures.push(format!(
            "[{}, {}] {what}{detail}",
            case.name,
            case.shell.display()
        ));
    }
}

const FIXED: HomeFixOutcome = HomeFixOutcome::Done {
    changed: true,
    restarted: true,
};

async fn bench(shell: &Path) {
    let mut c = Checks {
        failures: Vec::new(),
        passed: 0,
    };

    // Cas de l'utilisateur : ProtectHome=yes, HOME sous /home, .ollama absent.
    let case = Case::new("hidden-by-yes", shell);
    let ran = case.run(case.ollama_str()).await;
    let r = Some(&ran);
    c.check(&case, "succès", ran.outcome == FIXED, r);
    c.check(
        &case,
        "drop-in rendu comme dropin_text",
        fs::read_to_string(case.dropin()).unwrap_or_default() == dropin_text(case.ollama_str()),
        r,
    );
    c.check(&case, ".ollama créé", case.ollama.is_dir(), r);
    c.check(
        &case,
        ".ollama créé avec les droits du service",
        case.log("setpriv")
            .lines()
            .any(|line| line.ends_with(&format!("-- mkdir -m 0755 -- {}", case.ollama.display()))),
        r,
    );
    c.check(
        &case,
        "rechargé puis redémarré",
        case.actions() == ["daemon-reload", "restart -- ollama.service"],
        r,
    );
    c.check(&case, "réponses arrêtées une fois", ran.restarts == 1, r);
    c.check(
        &case,
        "étapes annoncées",
        ran.lines
            == [
                "step prepare",
                "step config",
                "step restart",
                "result changed=yes restarted=yes",
            ],
        r,
    );
    c.check(
        &case,
        "dossier de travail effacé",
        fs::read_dir(case.root.join("run"))
            .unwrap()
            .next()
            .is_none(),
        r,
    );
    c.check(
        &case,
        "arguments après le script, dossier en dernier (boîte polkit)",
        case.log("pkexec").trim_end().ends_with(&format!(
            " chatonlm-home ollama.service {}",
            case.ollama.display()
        )),
        r,
    );

    // Dossier déjà là (clé d'Ollama), ProtectHome=tmpfs (dossier des modèles
    // sous /home monté seul).
    let case = Case::new("existing-folder-tmpfs", shell);
    case.state("protect_home", "tmpfs");
    fs::create_dir_all(&case.ollama).unwrap();
    fs::write(case.ollama.join("id_ed25519"), "key").unwrap();
    let ran = case.run(case.ollama_str()).await;
    let r = Some(&ran);
    c.check(&case, "succès", ran.outcome == FIXED, r);
    c.check(
        &case,
        "rien recréé",
        !case.log("setpriv").contains("mkdir"),
        r,
    );
    c.check(
        &case,
        "clé intacte",
        fs::read_to_string(case.ollama.join("id_ed25519")).unwrap_or_default() == "key",
        r,
    );

    // Service arrêté : configuration seule.
    let case = Case::new("service-stopped", shell);
    case.state("active", "inactive");
    let ran = case.run(case.ollama_str()).await;
    let r = Some(&ran);
    c.check(
        &case,
        "succès sans redémarrage",
        ran.outcome
            == HomeFixOutcome::Done {
                changed: true,
                restarted: false,
            },
        r,
    );
    c.check(
        &case,
        "ni arrêt ni démarrage",
        case.actions() == ["daemon-reload"],
        r,
    );
    c.check(&case, "réponses laissées", ran.restarts == 0, r);

    // Rien à faire : read-only (lisible), ou dossier hors de /home.
    let case = Case::new("read-only", shell);
    case.state("protect_home", "read-only");
    let ran = case.run(case.ollama_str()).await;
    let r = Some(&ran);
    c.check(
        &case,
        "rien à faire",
        ran.outcome
            == HomeFixOutcome::Done {
                changed: false,
                restarted: false,
            },
        r,
    );
    c.check(&case, "pas de drop-in", !case.dropin().exists(), r);
    c.check(&case, "rien créé", !case.ollama.exists(), r);
    c.check(
        &case,
        "systemctl seulement lu",
        case.actions().is_empty(),
        r,
    );
    let case = Case::new("outside-home", shell);
    let var_lib = case.dir.join("var lib ollama");
    fs::create_dir_all(&var_lib).unwrap();
    case.state("home", var_lib.to_str().unwrap());
    let outside = var_lib.join(".ollama");
    let ran = case.run(outside.to_str().unwrap()).await;
    c.check(
        &case,
        "rien à faire hors de /home",
        ran.outcome
            == HomeFixOutcome::Done {
                changed: false,
                restarted: false,
            }
            && case.actions().is_empty(),
        Some(&ran),
    );

    // Le dossier passé n'est pas celui du service : refus (11).
    let case = Case::new("other-folder", shell);
    let other = case.dir.join("home/other/.ollama");
    let ran = case.run(other.to_str().unwrap()).await;
    let r = Some(&ran);
    c.check(
        &case,
        "code 11",
        ran.failure() == Some((HomeFixFailure::UnitChanged, false)),
        r,
    );
    c.check(
        &case,
        "raison dans le détail",
        ran.detail_has("is not the folder of"),
        r,
    );
    c.check(
        &case,
        "rien créé",
        !other.exists() && !case.dropin().exists(),
        r,
    );
    // Service root sans User= ni HOME= : pas de HOME, refus.
    let case = Case::new("root-without-home", shell);
    case.state("user", "");
    case.state("home", "");
    let ran = case.run(case.ollama_str()).await;
    c.check(
        &case,
        "code 11",
        ran.failure() == Some((HomeFixFailure::UnitChanged, false)) && case.actions().is_empty(),
        Some(&ran),
    );
    // Chemin que systemd interpréterait : refusé (10).
    let case = Case::new("bad-characters", shell);
    let ran = case
        .run(&format!("{}/a:b/.ollama", case.dir.display()))
        .await;
    c.check(
        &case,
        "code 10",
        ran.failure() == Some((HomeFixFailure::Unexpected, false))
            && ran.detail_has("invalid folder"),
        Some(&ran),
    );

    // .ollama n'est pas un dossier (12), ou appartient à un autre compte (12),
    // ou ne peut pas être créé (15) : rien de changé.
    let case = Case::new("not-a-folder", shell);
    fs::write(&case.ollama, "file").unwrap();
    let ran = case.run(case.ollama_str()).await;
    c.check(
        &case,
        "fichier : code 12",
        ran.failure() == Some((HomeFixFailure::NotYours, false))
            && ran.detail_has("not a folder")
            && !case.dropin().exists()
            && case.actions().is_empty(),
        Some(&ran),
    );
    let case = Case::new("folder-of-another-user", shell);
    case.state("user", "nobody");
    case.state("group", &id_of("-gn", "nobody"));
    fs::create_dir_all(&case.ollama).unwrap();
    let ran = case.run(case.ollama_str()).await;
    c.check(
        &case,
        "autre compte : code 12",
        ran.failure() == Some((HomeFixFailure::NotYours, false))
            && ran.detail_has("not a folder of nobody")
            && case.actions().is_empty(),
        Some(&ran),
    );
    // Lien symbolique (même vers un dossier du service) : systemd le
    // suivrait à chaque démarrage, refus.
    let case = Case::new("symbolic-link", shell);
    let target = case.dir.join("elsewhere");
    fs::create_dir_all(&target).unwrap();
    std::os::unix::fs::symlink(&target, &case.ollama).unwrap();
    let ran = case.run(case.ollama_str()).await;
    c.check(
        &case,
        "lien : code 12",
        ran.failure() == Some((HomeFixFailure::NotYours, false))
            && ran.detail_has("not a folder of")
            && !case.dropin().exists()
            && case.actions().is_empty(),
        Some(&ran),
    );
    // Service root : refus (11), rien créé ni écrit.
    for (name, user) in [("root-service", "root"), ("root-service-no-user", "")] {
        let case = Case::new(name, shell);
        case.state("user", user);
        let ran = case.run(case.ollama_str()).await;
        c.check(
            &case,
            "root : code 11",
            ran.failure() == Some((HomeFixFailure::UnitChanged, false))
                && ran.detail_has("runs as root")
                && !case.ollama.exists()
                && !case.dropin().exists()
                && case.actions().is_empty(),
            Some(&ran),
        );
    }
    // Montage déjà là mais .ollama effacé : recréé, service redémarré.
    let case = Case::new("bind-present-source-gone", shell);
    case.state("protect_home", "tmpfs");
    fs::create_dir_all(case.dropins()).unwrap();
    fs::write(case.dropin(), dropin_text(case.ollama_str())).unwrap();
    let ran = case.run(case.ollama_str()).await;
    c.check(
        &case,
        "recréé",
        ran.outcome == FIXED
            && case.ollama.is_dir()
            && fs::read_to_string(case.dropin()).unwrap_or_default()
                == dropin_text(case.ollama_str()),
        Some(&ran),
    );
    let case = Case::new("cannot-create", shell);
    fs::remove_dir(&case.home).unwrap();
    let ran = case.run(case.ollama_str()).await;
    c.check(
        &case,
        "HOME absent : code 15",
        ran.failure() == Some((HomeFixFailure::CannotCreate, false))
            && ran.detail_has("cannot create")
            && !case.dropin().exists()
            && case.actions().is_empty(),
        Some(&ran),
    );

    // Un drop-in plus tardif l'emporte : tout est remis (23).
    for (name, later) in [
        (
            "protect-home-overridden",
            "[Service]\nProtectHome=yes\n".to_owned(),
        ),
        ("binds-reset", "[Service]\nBindPaths=\n".to_owned()),
        // Seul un sous-dossier reste monté : pas le dossier lui-même.
        (
            "only-a-subfolder-bound",
            "[Service]\nBindPaths=\nBindPaths=\"@/models\"\n".to_owned(),
        ),
    ] {
        let case = Case::new(name, shell);
        let later = later.replace('@', case.ollama_str());
        fs::create_dir_all(case.dropins()).unwrap();
        fs::write(case.dropins().join("zzz-admin.conf"), later).unwrap();
        let ran = case.run(case.ollama_str()).await;
        let r = Some(&ran);
        c.check(
            &case,
            "code 23",
            ran.failure() == Some((HomeFixFailure::Overridden, true)),
            r,
        );
        c.check(
            &case,
            "restauré",
            ran.lines.iter().any(|line| line == "restored"),
            r,
        );
        c.check(&case, "drop-in retiré", !case.dropin().exists(), r);
        c.check(&case, "dossier créé retiré", !case.ollama.exists(), r);
        c.check(
            &case,
            "configuration rechargée, service jamais redémarré",
            case.actions() == ["daemon-reload", "daemon-reload"] && ran.restarts == 0,
            r,
        );
    }

    // Le service redémarré ne voit toujours pas le dossier (24) : ancien
    // drop-in remis, service relancé avec.
    let mut case = Case::new("still-hidden-after-restart", shell);
    case.set("FAKE_FAIL", "hide-home");
    fs::create_dir_all(case.dropins()).unwrap();
    fs::write(case.dropin(), "# previous\n[Service]\n").unwrap();
    let ran = case.run(case.ollama_str()).await;
    let r = Some(&ran);
    c.check(
        &case,
        "code 24",
        ran.failure() == Some((HomeFixFailure::StartFailed, true)),
        r,
    );
    c.check(
        &case,
        "restauré",
        ran.lines.iter().any(|line| line == "restored"),
        r,
    );
    c.check(
        &case,
        "ancien drop-in remis",
        fs::read_to_string(case.dropin()).unwrap_or_default() == "# previous\n[Service]\n",
        r,
    );
    c.check(
        &case,
        "rechargé et relancé avec l'ancienne configuration",
        case.actions()
            == [
                "daemon-reload",
                "restart -- ollama.service",
                "daemon-reload",
                "restart -- ollama.service",
            ],
        r,
    );
    c.check(&case, "détail du script", ran.detail_has("does not see"), r);
    // Redémarrage refusé par systemd.
    let mut case = Case::new("restart-fails", shell);
    case.set("FAKE_FAIL", "restart");
    let ran = case.run(case.ollama_str()).await;
    c.check(
        &case,
        "code 30 : la relance de restauration échoue aussi",
        ran.failure() == Some((HomeFixFailure::RestoreFailed, false))
            && ran.detail_has("could not restore everything")
            && !case.dropin().exists(),
        Some(&ran),
    );

    // Invite fermée, pas d'agent : rien d'exécuté.
    let mut case = Case::new("cancel", shell);
    case.set("FAKE_PKEXEC", "cancel");
    let ran = case.run(case.ollama_str()).await;
    c.check(
        &case,
        "annulé, rien fait",
        ran.outcome == HomeFixOutcome::Cancelled
            && case.log("systemctl").is_empty()
            && !case.ollama.exists(),
        Some(&ran),
    );
    let mut case = Case::new("no-agent", shell);
    case.set("FAKE_PKEXEC", "noagent");
    let ran = case.run(case.ollama_str()).await;
    c.check(
        &case,
        "sans agent",
        ran.failure() == Some((HomeFixFailure::NoAgent, false))
            && ran.detail_has("No authentication agent"),
        Some(&ran),
    );

    eprintln!(
        "banc home.sh sous {} : réussis {}, échecs {}",
        shell.display(),
        c.passed,
        c.failures.len()
    );
    assert!(c.failures.is_empty(), "{}", c.failures.join("\n"));
}

fn id_of(flag: &str, user: &str) -> String {
    let output = Command::new("id")
        .args([flag, "--", user])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[tokio::test]
async fn home_script_bench_under_sh() {
    bench(Path::new(SHELL)).await;
}

#[tokio::test]
async fn home_script_bench_under_dash() {
    match find_dash() {
        Some(dash) => bench(&dash).await,
        None => eprintln!("dash introuvable : banc sous dash sauté"),
    }
}
