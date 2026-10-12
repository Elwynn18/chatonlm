//! Tests purs du dossier des modèles : classement, validation (comparée aux
//! fonctions du script), rendu du drop-in et des commandes à copier, codes de
//! sortie, progression, verrou. Le banc du script est dans `bench.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;

const OWN: u32 = 1000;

/// Dossier temporaire dans `.tools/tmp` du projet (chemin avec une espace).
pub fn temp_dir() -> tempfile::TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tools/tmp");
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("models dir ")
        .tempdir_in(base)
        .unwrap()
}

/// `dash` du système, ou celui extrait dans `.tools/dash` (dépôt principal
/// ou copie de travail placée dessous).
pub fn find_dash() -> Option<PathBuf> {
    let in_path = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("dash"))
            .find(|candidate| candidate.is_file())
    });
    in_path.or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .map(|dir| dir.join(".tools/dash/usr/bin/dash"))
            .find(|candidate| candidate.is_file())
    })
}

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

fn env(items: &[&str]) -> Env {
    Env::from_assignments(items.iter().copied())
}

fn process(cgroup: &str, exe: &str, items: &[&str]) -> ProcessFacts {
    ProcessFacts {
        cgroup: cgroup.into(),
        exe: exe.into(),
        env: env(items),
    }
}

fn unit(active: &str, user: &str, items: &[&str]) -> UnitFacts {
    UnitFacts {
        service: ServiceDetails {
            active_state: active.into(),
            main_pid: if active == "active" { 4000 } else { 0 },
            environment: items.iter().map(|item| (*item).to_owned()).collect(),
            user: user.into(),
            group: user.into(),
            protect_home: "yes".into(),
            protect_system: "full".into(),
            ..ServiceDetails::default()
        },
        env: env(items),
        user_home: Some(format!("/home/{user}")),
        ..UnitFacts::default()
    }
}

fn facts(
    listener: Option<Listener>,
    process: Option<ProcessFacts>,
    unit: Option<UnitFacts>,
) -> Facts {
    Facts {
        own_uid: OWN,
        listener,
        process,
        unit,
        answers: false,
        can_elevate: true,
    }
}

fn listening(uid: u32) -> Option<Listener> {
    Some(Listener { uid, inode: 42 })
}

fn location(address: &str, facts: &Facts) -> ModelsLocation {
    classify(&url(address), facts).location
}

const LOCAL: &str = "http://127.0.0.1:11434";
const SYSTEM_CGROUP: &str = "/system.slice/ollama.service";

#[test]
fn a_remote_server_is_changed_on_that_computer() {
    let none = Facts::default();
    assert_eq!(
        location("http://192.168.1.20:11434", &none),
        ModelsLocation::Remote {
            host: "192.168.1.20".into()
        }
    );
    assert_eq!(
        location("https://ollama.example.com/ollama", &none),
        ModelsLocation::Remote {
            host: "ollama.example.com".into()
        }
    );
}

#[test]
fn the_system_service_of_the_same_user_is_read_from_its_process() {
    // Cette machine : User=alice, override.conf, dossier sur NTFS.
    let facts = facts(
        listening(OWN),
        Some(process(
            SYSTEM_CGROUP,
            "ollama",
            &[
                "HOME=/home/alice",
                "OLLAMA_MODELS=/data/Logiciel/Ollama/models",
            ],
        )),
        Some(unit(
            "active",
            "alice",
            &["HOME=/home/alice", "OLLAMA_MODELS=/data/elsewhere"],
        )),
    );
    let detection = classify(&url(LOCAL), &facts);
    assert_eq!(
        detection.location,
        ModelsLocation::SystemService {
            dir: Some("/data/Logiciel/Ollama/models".into()),
            user: "alice".into(),
            group: "alice".into(),
            running: true,
            managed: false,
            can_elevate: true,
            env_file: None,
        }
    );
    assert_eq!(detection.protect_home, "yes");
    assert_eq!(detection.protect_system, "full");
}

#[test]
fn the_service_of_the_official_script_is_recognised_by_its_main_process() {
    // User=ollama, pas d'OLLAMA_MODELS : ~ollama/.ollama/models.
    let mut service = unit("active", "ollama", &["PATH=/usr/bin"]);
    service.main_uid = Some(961);
    service.user_home = Some("/usr/share/ollama".into());
    let detected = location(LOCAL, &facts(listening(961), None, Some(service.clone())));
    assert_eq!(
        detected,
        ModelsLocation::SystemService {
            dir: Some("/usr/share/ollama/.ollama/models".into()),
            user: "ollama".into(),
            group: "ollama".into(),
            running: true,
            managed: false,
            can_elevate: true,
            env_file: None,
        }
    );
    // Le service écoute ailleurs : ce n'est pas lui.
    service.service.environment = vec!["OLLAMA_HOST=0.0.0.0:11435".into()];
    service.env = env(&["OLLAMA_HOST=0.0.0.0:11435"]);
    assert_eq!(
        location(LOCAL, &facts(listening(961), None, Some(service.clone()))),
        ModelsLocation::Unknown { running: true }
    );
    // Autre uid que le processus principal : pas lui non plus.
    service.env = Env::default();
    service.main_uid = Some(962);
    assert_eq!(
        location(LOCAL, &facts(listening(961), None, Some(service))),
        ModelsLocation::Unknown { running: true }
    );
}

#[test]
fn a_root_socket_without_service_is_a_container() {
    assert_eq!(
        location(LOCAL, &facts(listening(0), None, None)),
        ModelsLocation::Container
    );
}

#[test]
fn processes_of_the_same_user_are_told_apart_by_cgroup_and_executable() {
    let cases = [
        (
            process(
                "/user.slice/user-1000.slice/user@1000.service/app.slice/ollama.service",
                "ollama",
                &["OLLAMA_MODELS=/data/user models"],
            ),
            ModelsLocation::UserService {
                dir: Some("/data/user models".into()),
            },
        ),
        (
            process(
                "/user.slice/user-1000.slice/user@1000.service/user.slice/libpod-0123.scope/container",
                "ollama",
                &[],
            ),
            ModelsLocation::Container,
        ),
        (
            process("/system.slice/docker-0123.scope", "ollama", &["HOME=/root"]),
            ModelsLocation::Container,
        ),
        (
            process(
                "/user.slice/user-1000.slice/user@1000.service/app.slice/app-x.scope",
                "rootlessport",
                &[],
            ),
            ModelsLocation::Container,
        ),
        (
            process(
                "/user.slice/user-1000.slice/user@1000.service/app.slice/app-com.microsoft.VSCode-2396.scope",
                "ollama",
                &["HOME=/home/alice/"],
            ),
            ModelsLocation::Manual {
                dir: Some("/home/alice/.ollama/models".into()),
                host_env: None,
            },
        ),
        (
            process("/user.slice/x.scope", "python3", &[]),
            ModelsLocation::Unknown { running: true },
        ),
    ];
    for (process, expected) in cases {
        let facts = facts(listening(OWN), Some(process.clone()), None);
        assert_eq!(location(LOCAL, &facts), expected, "{process:?}");
    }
}

#[test]
fn a_manual_launch_on_another_port_keeps_its_address() {
    let facts = facts(
        listening(OWN),
        Some(process(
            "/user.slice/app.slice/app-x.scope",
            "ollama",
            &["OLLAMA_HOST=127.0.0.1:11435", "OLLAMA_MODELS=/x/models"],
        )),
        None,
    );
    let detection = classify(&url("http://127.0.0.1:11435"), &facts);
    assert_eq!(
        detection.location,
        ModelsLocation::Manual {
            dir: Some("/x/models".into()),
            host_env: Some("127.0.0.1:11435".into()),
        }
    );
    assert_eq!(detection.host_env.as_deref(), Some("127.0.0.1:11435"));
}

#[test]
fn a_system_cgroup_without_systemd_answers_is_unknown() {
    let facts = facts(
        listening(OWN),
        Some(process(SYSTEM_CGROUP, "ollama", &[])),
        None,
    );
    assert_eq!(
        location(LOCAL, &facts),
        ModelsLocation::Unknown { running: true }
    );
}

#[test]
fn nothing_listening_uses_the_configured_service() {
    let stopped = unit("inactive", "ollama", &["OLLAMA_MODELS=/srv/models"]);
    assert_eq!(
        location(LOCAL, &facts(None, None, Some(stopped.clone()))),
        ModelsLocation::SystemService {
            dir: Some("/srv/models".into()),
            user: "ollama".into(),
            group: "ollama".into(),
            running: false,
            managed: false,
            can_elevate: true,
            env_file: None,
        }
    );
    // Démarrage en cours : il sera redémarré.
    let starting = unit("activating", "ollama", &[]);
    assert!(matches!(
        location(LOCAL, &facts(None, None, Some(starting))),
        ModelsLocation::SystemService { running: true, .. }
    ));
    // Service d'un autre port : rien à dire.
    assert_eq!(
        location(
            "http://127.0.0.1:11435",
            &facts(None, None, Some(stopped.clone()))
        ),
        ModelsLocation::Unknown { running: false }
    );
    // Rien du tout ; ou une réponse sans socket sur l'hôte (conteneur en
    // DNAT).
    let mut nothing = facts(None, None, None);
    assert_eq!(
        location(LOCAL, &nothing),
        ModelsLocation::Unknown { running: false }
    );
    nothing.answers = true;
    assert_eq!(
        location(LOCAL, &nothing),
        ModelsLocation::Unknown { running: true }
    );
}

/// Service qui tourne en root (pas de `User=`, ou un nom d'uid 0) :
/// toujours « root », l'app ne le change pas elle-même.
#[test]
fn a_service_that_runs_as_root_is_named_root() {
    let user = |name: &str, uid: Option<u32>| {
        let mut service = unit("inactive", name, &[]);
        service.user_uid = uid;
        match location(LOCAL, &facts(None, None, Some(service))) {
            ModelsLocation::SystemService { user, .. } => user,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(user("", Some(0)), "root");
    assert_eq!(user("", None), "root");
    assert_eq!(user("toor", Some(0)), "root");
    assert_eq!(user("ollama", Some(961)), "ollama");
    assert_eq!(user("ollama", None), "ollama");
    assert_eq!(uid_of(""), Some(0));
    assert_eq!(uid_of("root"), Some(0));
    assert_eq!(uid_of("961"), Some(961));
    assert_eq!(uid_of("chatonlm-no-such-user"), None);
}

#[test]
fn the_service_folder_comes_from_the_most_reliable_source() {
    let mut service = unit(
        "inactive",
        "",
        &["HOME=/var/lib/ollama", "OLLAMA_MODELS=/from/environment"],
    );
    service.user_home = Some("/root".into());
    let dir = |service: &UnitFacts| match location(LOCAL, &facts(None, None, Some(service.clone())))
    {
        ModelsLocation::SystemService { dir, user, .. } => {
            assert_eq!(user, "root");
            dir
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(dir(&service).as_deref(), Some("/from/environment"));
    // EnvironmentFile= l'emporte sur Environment=.
    service.env_file = Some(("/etc/ollama.env".into(), "/from/file".into()));
    assert_eq!(dir(&service).as_deref(), Some("/from/file"));
    // L'environnement du processus principal (même uid) l'emporte sur tout.
    service.main_env = Some(env(&["OLLAMA_MODELS=/from/process"]));
    assert_eq!(dir(&service).as_deref(), Some("/from/process"));
    // Sans OLLAMA_MODELS : HOME, puis le répertoire de User=.
    service.main_env = None;
    service.env_file = None;
    service.env = env(&["HOME=/var/lib/ollama"]);
    assert_eq!(
        dir(&service).as_deref(),
        Some("/var/lib/ollama/.ollama/models")
    );
    service.env = Env::default();
    assert_eq!(dir(&service).as_deref(), Some("/root/.ollama/models"));
}

#[test]
fn managed_folder_environment_file_and_pkexec_are_reported() {
    let mut service = unit("active", "alice", &[]);
    service.service.drop_in_paths = vec![
        "/etc/systemd/system/ollama.service.d/override.conf".into(),
        "/etc/systemd/system/ollama.service.d/zz-chatonlm-models.conf".into(),
    ];
    service.env_file = Some(("/etc/ollama.env".into(), "/x".into()));
    let mut facts = facts(
        listening(OWN),
        Some(process(SYSTEM_CGROUP, "ollama", &[])),
        Some(service),
    );
    facts.can_elevate = false;
    let ModelsLocation::SystemService {
        managed,
        env_file,
        can_elevate,
        ..
    } = location(LOCAL, &facts)
    else {
        panic!("service système attendu");
    };
    assert!(managed);
    assert_eq!(env_file.as_deref(), Some("/etc/ollama.env"));
    assert!(!can_elevate);
}

#[test]
fn ollama_host_gives_the_port_like_envconfig() {
    let cases = [
        (None, 11434),
        (Some("0.0.0.0"), 11434),
        (Some("0.0.0.0:11435"), 11435),
        (Some("\"0.0.0.0:11435\""), 11435),
        (Some(":8080"), 8080),
        (Some("http://example.com"), 80),
        (Some("https://example.com"), 443),
        (Some("http://127.0.0.1:11434/path"), 11434),
        (Some("[::1]:9000"), 9000),
        (Some("::1"), 11434),
        (Some("host:abc"), 11434),
        (Some("host:70000"), 11434),
        (Some("ollama.com"), 443),
    ];
    for (value, port) in cases {
        assert_eq!(ollama_host_port(value), Some(port), "{value:?}");
    }
}

#[test]
#[cfg(target_endian = "little")]
fn listening_sockets_are_read_from_proc_net() {
    let tcp = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:2CAA 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1953260 1 0000000000000000 100 0 0 10 0
   1: 0100007F:2CAB 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1979014 1 0000000000000000 100 0 0 10 0
   2: 0100007F:2CAA 0100007F:D2F0 01 00000000:00000000 00:00000000 00000000  1000        0 5555 1 0000000000000000 20 4 30 10 -1
   3: 0501A8C0:2CAA 00000000:0000 0A 00000000:00000000 00:00000000 00000000   961        0 7777 1 0000000000000000 100 0 0 10 0
   4: 00000000:2CAA 00000000:0000 0A 00000000:00000000 00:00000000 00000000   961        0 8888 1 0000000000000000 100 0 0 10 0
";
    assert_eq!(
        parse_proc_net(tcp, 11434),
        [
            Listener {
                uid: 1000,
                inode: 1953260
            },
            Listener {
                uid: 961,
                inode: 8888
            }
        ]
    );
    assert_eq!(
        parse_proc_net(tcp, 11435),
        [Listener {
            uid: 1000,
            inode: 1979014
        }]
    );
    let tcp6 = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000001000000:2CAA 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 11 1 0000000000000000 100 0 0 10 0
   1: 0000000000000000FFFF00000100007F:2CAB 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 12 1 0000000000000000 100 0 0 10 0
   2: 000080FE00000000FF00000000000001:2CAA 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 13 1 0000000000000000 100 0 0 10 0
   3: 00000000000000000000000000000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 14 1 0000000000000000 100 0 0 10 0
";
    assert_eq!(
        parse_proc_net(tcp6, 11434),
        [Listener {
            uid: 1000,
            inode: 11
        }]
    );
    assert_eq!(
        parse_proc_net(tcp6, 11435),
        [Listener {
            uid: 1000,
            inode: 12
        }]
    );
    assert_eq!(parse_proc_net(tcp6, 8080), [Listener { uid: 0, inode: 14 }]);
    assert!(parse_proc_net("garbage\nnot a line", 11434).is_empty());
}

#[test]
fn environments_keep_the_last_value_like_ollama_reads_it() {
    let env = env(&[
        "OLLAMA_MODELS=/first",
        "OLLAMA_MODELS= \"/last one\" ",
        "HOME=",
        "NOT_AN_ASSIGNMENT",
    ]);
    assert_eq!(env.models.as_deref(), Some("/last one"));
    assert_eq!(env.home, None);
    assert_eq!(env.models_dir().as_deref(), Some("/last one"));
    let text = "# OLLAMA_MODELS=/commented\nOLLAMA_MODELS=/a\n  export  OLLAMA_MODELS=\"/b c\"\nexportOLLAMA_MODELS=/no\nOLLAMA_HOST=x\n";
    assert_eq!(env_file_models(text).as_deref(), Some("/b c"));
    assert_eq!(env_file_models("OLLAMA_HOST=1\n"), None);
}

#[test]
fn facts_for_the_real_app_tests() {
    let detection = parse_injected(
        "launch=system;dir=/var/lib/ollama;user=ollama;group=ollama;running=yes;protect_home=yes",
        true,
    )
    .unwrap();
    assert_eq!(
        detection.location,
        ModelsLocation::SystemService {
            dir: Some("/var/lib/ollama".into()),
            user: "ollama".into(),
            group: "ollama".into(),
            running: true,
            managed: false,
            can_elevate: true,
            env_file: None,
        }
    );
    assert_eq!(detection.protect_home, "yes");
    assert_eq!(
        parse_injected("launch=manual;dir=/m;host_env=127.0.0.1:11435", false)
            .unwrap()
            .location,
        ModelsLocation::Manual {
            dir: Some("/m".into()),
            host_env: Some("127.0.0.1:11435".into())
        }
    );
    assert_eq!(
        parse_injected("launch=remote;host=box", false)
            .unwrap()
            .location,
        ModelsLocation::Remote { host: "box".into() }
    );
    assert_eq!(
        parse_injected("launch=unknown;running=no", false)
            .unwrap()
            .location,
        ModelsLocation::Unknown { running: false }
    );
    for bad in [
        "launch=moon",
        "dir=/x",
        "launch=system;running=maybe",
        "launch=remote",
        "launch=system;colour=red",
    ] {
        assert!(parse_injected(bad, true).is_none(), "{bad}");
    }
}

/// `dir=@<fichier>` : dossier relu dans le fichier à chaque détection (e2e :
/// le champ suit l'état du faux service après un déplacement).
#[test]
fn injected_folder_read_from_a_file() {
    let tmp = temp_dir();
    let file = tmp.path().join("loaded models");
    let facts = format!("launch=system;dir=@{};running=yes", file.display());
    let dir = |facts: &str| match parse_injected(facts, true).unwrap().location {
        ModelsLocation::SystemService { dir, .. }
        | ModelsLocation::Manual { dir, .. }
        | ModelsLocation::UserService { dir } => dir,
        other => panic!("{other:?}"),
    };
    // Fichier absent, puis vide : dossier inconnu.
    assert_eq!(dir(&facts), None);
    fs::write(&file, "").unwrap();
    assert_eq!(dir(&facts), None);
    fs::write(&file, "/data/Old models\n").unwrap();
    assert_eq!(dir(&facts).as_deref(), Some("/data/Old models"));
    fs::write(&file, "/data/New models é\nignored\n").unwrap();
    assert_eq!(dir(&facts).as_deref(), Some("/data/New models é"));
    let manual = format!("launch=manual;dir=@{}", file.display());
    assert_eq!(dir(&manual).as_deref(), Some("/data/New models é"));
    let user = format!("launch=user;dir=@{}", file.display());
    assert_eq!(dir(&user).as_deref(), Some("/data/New models é"));
    // Sans « @ », la valeur est le dossier.
    assert_eq!(
        dir("launch=system;dir=/var/lib/ollama;running=no").as_deref(),
        Some("/var/lib/ollama")
    );
}

// --- Validation ------------------------------------------------------------------

/// Texte d'une fonction du script (`nom() {` … `}` en début de ligne).
fn script_function(name: &str) -> &'static str {
    let start = SCRIPT
        .find(&format!("\n{name}() {{\n"))
        .unwrap_or_else(|| panic!("fonction {name} absente du script"))
        + 1;
    let end = start + SCRIPT[start..].find("\n}\n").unwrap() + 3;
    &SCRIPT[start..end]
}

/// Réponse d'une fonction du script pour `path`, sous `shell` (avec
/// `plain_path`, qu'appelle `clean_path`).
fn script_says(shell: &Path, function: &str, path: &str) -> bool {
    let program = format!(
        "TEST_HOME=''\n{}\n{}\n{function} \"$1\"",
        script_function("plain_path"),
        script_function(function)
    );
    let status = Command::new(shell)
        .args(["-c", &program, "sh", path])
        .env("LC_ALL", "C")
        .status()
        .unwrap();
    status.success()
}

fn shells() -> Vec<PathBuf> {
    let mut shells = vec![PathBuf::from(SHELL)];
    shells.extend(find_dash());
    shells
}

/// Chemins refusés et admis par les deux validations.
const PATHS: [&str; 40] = [
    "/",
    "/home",
    "/media",
    "/mnt",
    "/opt",
    "/srv",
    "/var",
    "/var/lib",
    "/etc/ollama",
    "/usr/share/ollama",
    "/usr",
    "/boot/x",
    "/dev/shm",
    "/efi",
    "/nix/store/x",
    "/proc/1",
    "/root/models",
    "/run",
    "/run/user/1000/m",
    "/run/media/alice",
    "/snap/x",
    "/sys/x",
    "/tmp",
    "/tmp/models",
    "/var/cache/x",
    "/var/log/ollama",
    "/var/tmp/m",
    "/var/lock/m",
    "/var/run/m",
    // Admis.
    "/var/lib/ollama",
    "/home/alice/LLM models",
    "/mnt/disk/models",
    "/media/disk",
    "/opt/ollama",
    "/srv/models",
    "/run/media/alice/disk",
    "/run/media/alice/disk/Ollama models",
    "/data/Logiciel/Ollama/models",
    "/usrx/models",
    "/etcetera",
];

#[test]
fn system_folders_are_refused_like_the_script_does() {
    for shell in shells() {
        for path in PATHS {
            assert_eq!(
                forbidden(path),
                script_says(&shell, "forbidden", path),
                "{path} sous {}",
                shell.display()
            );
        }
    }
    assert!(forbidden("/etc"));
    assert!(forbidden("/run/media/alice"));
    assert!(!forbidden("/run/media/alice/disk"));
    assert!(!forbidden("/home/alice"));
}

#[test]
fn characters_are_refused_like_the_script_does() {
    // Chemins inexistants hors de tout lien symbolique : realpath -m les
    // laisse tels quels.
    let base = "/chatonlm-nonexistent";
    let cases: Vec<(String, bool)> = vec![
        (format!("{base}/My Models/100% sure"), true),
        (format!("{base}/it's"), true),
        (format!("{base}/été — modèles"), true),
        (format!("{base}/a:b"), false),
        (format!("{base}/quote\"d"), false),
        (format!("{base}/back\\slash"), false),
        (format!("{base}/dollar$x"), false),
        (format!("{base}/tick`x"), false),
        (format!("{base}/new\nline"), false),
        (format!("{base}/tab\tx"), false),
        (format!("{base}/del\u{7f}x"), false),
        (format!("{base}/x/../y"), false),
        (format!("{base}/./y"), false),
        (format!("{base}//y"), false),
        (format!("{base}/y/"), false),
        ("relative/path".to_owned(), false),
        ("/".to_owned(), true),
    ];
    for shell in shells() {
        for (path, clean) in &cases {
            assert_eq!(clean_path(path), *clean, "{path:?}");
            assert_eq!(
                script_says(&shell, "clean_path", path),
                *clean,
                "{path:?} sous {}",
                shell.display()
            );
        }
    }
    // Côté Rust seulement : caractères de contrôle C1, longueur.
    assert!(!clean_path(&format!("{base}/c1\u{85}x")));
    assert!(!clean_path(&format!("/{}", "a".repeat(MAX_PATH))));
    assert!(clean_path(&format!("/{}", "a".repeat(MAX_PATH - 2))));
}

fn mkmodels(dir: &Path) {
    fs::create_dir_all(dir.join("blobs")).unwrap();
    fs::create_dir_all(dir.join("manifests/registry.ollama.ai/library/qwen3")).unwrap();
    fs::write(dir.join("blobs/sha256-aaaa"), "weights-A").unwrap();
}

fn system_detection(dir: Option<&Path>, running: bool) -> Detection {
    Detection {
        location: ModelsLocation::SystemService {
            dir: dir.map(|dir| dir.to_str().unwrap().to_owned()),
            user: "ollama".into(),
            group: "ollama".into(),
            running,
            managed: false,
            can_elevate: true,
            env_file: None,
        },
        protect_home: "yes".into(),
        protect_system: "full".into(),
        host_env: None,
        ollama_dir: None,
        hidden_home: None,
    }
}

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn the_chosen_folder_is_checked_before_any_prompt() {
    let tmp = temp_dir();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let old = base.join("old models");
    mkmodels(&old);
    let detection = system_detection(Some(&old), true);
    let check = |path: &Path| check_folder(path_str(path), &detection);

    let empty = base.join("empty");
    fs::create_dir(&empty).unwrap();
    let found = check(&empty);
    assert_eq!(found.problem, None);
    assert_eq!(found.path, path_str(&empty));
    assert_eq!(found.old.as_deref(), Some(path_str(&old)));
    assert!(!found.has_models);
    assert!(found.same_drive);
    assert!(found.free_bytes.is_some_and(|free| free > 0));
    assert!(
        found
            .commands
            .as_deref()
            .unwrap()
            .contains("sudo chown ollama:ollama")
    );

    let models = base.join("models");
    mkmodels(&models);
    fs::create_dir(models.join("metadata")).unwrap();
    assert_eq!(check(&models).problem, None);
    assert!(check(&models).has_models);

    let documents = base.join("documents");
    fs::create_dir(&documents).unwrap();
    fs::write(documents.join("cv.odt"), "cv").unwrap();
    assert_eq!(check(&documents).problem, Some(FolderProblem::NotEmpty));
    let hidden = base.join("hidden");
    fs::create_dir(&hidden).unwrap();
    fs::write(hidden.join(".directory"), "").unwrap();
    assert_eq!(check(&hidden).problem, Some(FolderProblem::NotEmpty));

    // Absent : créé par le script sous un parent existant.
    assert_eq!(check(&base.join("new folder")).problem, None);
    assert_eq!(
        check(&base.join("missing/new folder")).problem,
        Some(FolderProblem::Missing)
    );
    let file = base.join("file");
    fs::write(&file, "x").unwrap();
    assert_eq!(check(&file).problem, Some(FolderProblem::Missing));

    assert_eq!(check(&old).problem, Some(FolderProblem::Same));
    assert_eq!(
        check(&old.join("blobs")).problem,
        Some(FolderProblem::Inside)
    );
    assert_eq!(check(&base).problem, Some(FolderProblem::Inside));

    // Absent (NTFS monté avec windows_names refuse « : ») : même refus.
    let colon = base.join("a:b");
    assert_eq!(check(&colon).problem, Some(FolderProblem::BadCharacters));
    assert_eq!(
        check_folder("/etc/ollama", &detection).problem,
        Some(FolderProblem::System)
    );
    assert_eq!(
        check_folder("relative", &detection).problem,
        Some(FolderProblem::Missing)
    );

    // Lien symbolique : c'est le dossier pointé qui part au script.
    let link = base.join("link");
    std::os::unix::fs::symlink(&empty, &link).unwrap();
    let linked = check(&link);
    assert_eq!(linked.problem, None);
    assert_eq!(linked.path, path_str(&empty));
}

/// Dossier existant d'un autre compte (`/var/empty`, `/srv/x` de root) :
/// refusé avant l'invite, comme le script le ferait ; un dossier neuf, ou à
/// l'utilisateur, ou au service (jamais root), est admis.
#[test]
fn a_folder_of_another_account_is_refused() {
    let tmp = temp_dir();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let old = base.join("old models");
    mkmodels(&old);
    let mine = base.join("mine");
    fs::create_dir(&mine).unwrap();
    let me = fs::metadata(&mine).unwrap().uid();
    let other = if me == 4242 { 4243 } else { 4242 };
    let service = |user: &str| {
        let mut detection = system_detection(Some(&old), true);
        if let ModelsLocation::SystemService { user: name, .. } = &mut detection.location {
            *name = user.to_owned();
        }
        detection
    };
    let problem = |path: &Path, user: &str, uid: u32| {
        check_folder_as(path_str(path), &service(user), uid).problem
    };
    // Vu par un autre compte, service `nobody` : ni l'un ni l'autre.
    assert_eq!(
        problem(&mine, "nobody", other),
        Some(FolderProblem::NotYours)
    );
    // L'utilisateur lui-même, ou le service (uid en nombre) : admis.
    assert_eq!(problem(&mine, "nobody", me), None);
    assert_eq!(problem(&mine, &me.to_string(), other), None);
    // Dossier neuf, créé par le script : admis.
    assert_eq!(problem(&base.join("new folder"), "nobody", other), None);
    // Un service root ne fait jamais admettre un dossier de root.
    assert!(foreign_folder(&mine, other, None));
    assert!(!foreign_folder(&mine, other, Some(me)));
    assert_eq!(foreign_folder(&mine, other, Some(0)), me != 0);
    if me != 0 {
        assert_eq!(problem(&mine, "root", other), Some(FolderProblem::NotYours));
    }
    // Ailleurs (lancement à la main) : rien n'est donné à un service.
    let manual = Detection::new(
        ModelsLocation::Manual {
            dir: Some(path_str(&old).to_owned()),
            host_env: None,
        },
        None,
    );
    assert_eq!(
        check_folder_as(path_str(&mine), &manual, other).problem,
        None
    );
}

/// Ancien dossier illisible pour l'app (home 0700 de l'utilisateur `ollama`
/// du script officiel) : même disque d'après son plus proche ancêtre lisible,
/// pas de copie annoncée ni de refus pour la place.
#[test]
fn an_unreadable_old_folder_is_on_the_drive_of_its_parent() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = temp_dir();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let home = base.join("usr-share-ollama");
    let old = home.join(".ollama/models");
    mkmodels(&old);
    fs::set_permissions(&home, fs::Permissions::from_mode(0o000)).unwrap();
    // Root lit tout : l'essai n'a de sens que pour un utilisateur ordinaire.
    let readable = fs::metadata(old.join("blobs")).is_ok();
    let target = base.join("target");
    fs::create_dir(&target).unwrap();
    let check = check_folder(path_str(&target), &system_detection(Some(&old), true));
    fs::set_permissions(&home, fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        return;
    }
    assert_eq!(check.problem, None);
    assert_eq!(check.old.as_deref(), Some(path_str(&old)));
    assert!(check.same_drive, "{check:?}");
    let check = FolderCheck {
        models: Some(ModelsSummary {
            count: 1,
            bytes: u64::MAX / 2,
        }),
        ..check
    };
    assert_eq!(space_shortage(&check, true), None);
}

#[test]
fn an_unknown_or_unclean_old_folder_cannot_be_moved() {
    let tmp = temp_dir();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let target = base.join("target");
    fs::create_dir(&target).unwrap();
    let check = check_folder(path_str(&target), &system_detection(None, true));
    assert_eq!(check.problem, None);
    assert_eq!(check.old, None);
    assert!(!check.same_drive);
    // Ancien dossier refusé par le script (« : ») : pas de déplacement.
    let odd = base.join("odd:old");
    assert_eq!(old_for_script(path_str(&odd)), None);
    // Lien symbolique : sa cible.
    let real = base.join("real");
    mkmodels(&real);
    let link = base.join("models link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(
        old_for_script(path_str(&link)).as_deref(),
        Some(path_str(&real))
    );
    // Illisible ou absent : forme lexicale.
    assert_eq!(
        old_for_script("/chatonlm-nonexistent/models/").as_deref(),
        Some("/chatonlm-nonexistent/models")
    );
}

#[test]
fn copies_to_another_drive_need_free_space() {
    let mut check = FolderCheck {
        path: "/new".into(),
        problem: None,
        old: Some("/old".into()),
        has_models: false,
        same_drive: false,
        models: Some(ModelsSummary {
            count: 2,
            bytes: 1_000_000_000,
        }),
        free_bytes: Some(1_000_000_000 + SPACE_MARGIN),
        commands: None,
        replies_running: false,
        downloads_running: false,
    };
    assert_eq!(space_shortage(&check, true), None);
    check.free_bytes = Some(1_000_000_000 + SPACE_MARGIN - 1);
    assert_eq!(
        space_shortage(&check, true),
        Some(MoveFailure::NoSpace {
            need: Some(1_000_000_000),
            free: Some(1_000_000_000 + SPACE_MARGIN - 1)
        })
    );
    assert_eq!(space_shortage(&check, false), None);
    check.same_drive = true;
    assert_eq!(space_shortage(&check, true), None);
    check.same_drive = false;
    check.models = None;
    assert_eq!(space_shortage(&check, true), None);
}

// --- Drop-in et commandes ---------------------------------------------------------

#[test]
fn the_dropin_doubles_percent_signs_and_adds_the_sandbox_lines() {
    assert_eq!(
        dropin_text("/data/100% sure/it's", Sandbox::None, None),
        "# Written by ChatonLM (Settings > Model location).\n[Unit]\nRequiresMountsFor=\"/data/100%% sure/it's\"\n\n[Service]\nEnvironment=\"OLLAMA_MODELS=/data/100%% sure/it's\"\n"
    );
    assert!(
        dropin_text("/home/alice/LLM models", Sandbox::Bind, None)
            .ends_with("Environment=\"OLLAMA_MODELS=/home/alice/LLM models\"\nProtectHome=tmpfs\nBindPaths=\"/home/alice/LLM models\"\n")
    );
    assert!(
        dropin_text("/home/alice/m", Sandbox::ReadWrite, None).ends_with(
            "Environment=\"OLLAMA_MODELS=/home/alice/m\"\nReadWritePaths=\"/home/alice/m\"\n"
        )
    );
}

/// Dossier des modèles monté par BindPaths= : le .ollama du service (sous
/// /home, hors du dossier des modèles) est monté aussi, comme le script.
#[test]
fn the_service_ollama_folder_is_bound_with_the_models() {
    let ollama = Some("/home/alice/.ollama");
    let cases = [
        ("/home/alice/LLM models", Sandbox::Bind, ollama, ollama),
        ("/home/alice/.ollama/models", Sandbox::Bind, ollama, ollama),
        // Dans le dossier des modèles, ou sans BindPaths= : rien de plus.
        ("/home/alice/.ollama", Sandbox::Bind, ollama, None),
        ("/home/alice", Sandbox::Bind, ollama, None),
        ("/home/alice/m", Sandbox::ReadWrite, ollama, None),
        ("/data/m", Sandbox::None, ollama, None),
        // Hors de /home (paquet Arch), inconnu, ou que systemd interpréterait.
        (
            "/home/alice/m",
            Sandbox::Bind,
            Some("/var/lib/ollama/.ollama"),
            None,
        ),
        ("/home/alice/m", Sandbox::Bind, None, None),
        (
            "/home/alice/m",
            Sandbox::Bind,
            Some("/home/a:b/.ollama"),
            None,
        ),
        (
            "/root/m",
            Sandbox::Bind,
            Some("/root/.ollama"),
            Some("/root/.ollama"),
        ),
    ];
    for (path, sandbox, dir, expected) in cases {
        assert_eq!(
            home_bind(path, sandbox, dir),
            expected,
            "{path} {sandbox:?} {dir:?}"
        );
    }
    assert!(
        dropin_text(
            "/home/alice/LLM models",
            Sandbox::Bind,
            Some("/home/alice/100% .ollama")
        )
        .ends_with(
            "BindPaths=\"/home/alice/LLM models\"\nBindPaths=\"/home/alice/100%% .ollama\"\n"
        )
    );
    // Ailleurs qu'avec BindPaths=, jamais de second montage.
    assert_eq!(
        dropin_text("/data/m", Sandbox::None, ollama),
        dropin_text("/data/m", Sandbox::None, None)
    );
    // Commandes : dossier créé par le service, même drop-in.
    let mut detection = system_detection(Some(Path::new("/var/lib/ollama")), true);
    detection.ollama_dir = Some("/home/ollama/.ollama".into());
    let commands = commands_for(&detection, "/home/alice/models").unwrap();
    assert!(
        commands.contains("\nsudo -u ollama mkdir -p '/home/ollama/.ollama'\n"),
        "{commands}"
    );
    assert!(
        commands
            .contains("'BindPaths=\"/home/alice/models\"' 'BindPaths=\"/home/ollama/.ollama\"'"),
        "{commands}"
    );
    let outside = commands_for(&detection, "/data/models").unwrap();
    assert!(!outside.contains(".ollama"), "{outside}");
}

#[test]
fn the_sandbox_follows_protect_home_and_protect_system() {
    let cases = [
        ("/home/alice/m", "yes", "full", Sandbox::Bind),
        ("/home/alice/m", "tmpfs", "no", Sandbox::Bind),
        ("/root/m", "true", "no", Sandbox::Bind),
        ("/run/user/1000/m", "yes", "no", Sandbox::Bind),
        ("/home/alice/m", "read-only", "full", Sandbox::ReadWrite),
        ("/home/alice/m", "no", "full", Sandbox::None),
        ("/home/alice/m", "no", "strict", Sandbox::ReadWrite),
        ("/data/m", "yes", "full", Sandbox::None),
        ("/data/m", "yes", "strict", Sandbox::ReadWrite),
        ("/homework/m", "yes", "full", Sandbox::None),
    ];
    for (path, home, system, expected) in cases {
        assert_eq!(
            sandbox_for(path, home, system),
            expected,
            "{path} {home} {system}"
        );
    }
}

/// Exécute `program` sous sh dans `dir`, avec `HOME=dir` (les faux
/// programmes sont des fonctions du shell : rien n'est écrit puis exécuté).
fn sh_in(dir: &Path, program: &str) -> std::process::Output {
    Command::new(SHELL)
        .args(["-c", program])
        .current_dir(dir)
        .env("HOME", dir)
        .env("LC_ALL", "C")
        .output()
        .unwrap()
}

const TRICKY: &str = "/data/It's 100% «sûr» & (vrai); *ok*";

#[test]
fn shell_quoting_survives_the_shell() {
    let tmp = temp_dir();
    for word in [TRICKY, "''", "a'b'c", " lead", "", "$HOME `id`"] {
        let output = sh_in(tmp.path(), &format!("printf '%s' {}", shell_quote(word)));
        assert_eq!(String::from_utf8(output.stdout).unwrap(), word);
    }
}

#[test]
fn the_manual_command_keeps_the_address_and_quotes_the_folder() {
    assert_eq!(
        manual_command("/data/Models", None),
        "OLLAMA_MODELS='/data/Models' ollama serve"
    );
    assert_eq!(
        manual_command("/data/it's 100%", Some("127.0.0.1:11435")),
        "OLLAMA_HOST=127.0.0.1:11435 OLLAMA_MODELS='/data/it'\\''s 100%' ollama serve"
    );
    assert_eq!(
        manual_command("/m", Some("[::1]:8080")),
        "OLLAMA_HOST=[::1]:8080 OLLAMA_MODELS='/m' ollama serve"
    );
    // La commande donne bien ce dossier à ollama serve.
    let tmp = temp_dir();
    let fake = "ollama() { printf '%s|%s|%s' \"$OLLAMA_HOST\" \"$OLLAMA_MODELS\" \"$*\"; }\n";
    let output = sh_in(
        tmp.path(),
        &format!("{fake}{}", manual_command(TRICKY, Some("127.0.0.1:11435"))),
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("127.0.0.1:11435|{TRICKY}|serve")
    );
}

#[test]
fn user_service_commands_write_the_dropin() {
    let commands = user_service_commands("/data/100% Models");
    assert_eq!(
        commands,
        "mkdir -p ~/.config/systemd/user/ollama.service.d\n\
         printf '%s\\n' '[Service]' 'Environment=\"OLLAMA_MODELS=/data/100%% Models\"' > ~/.config/systemd/user/ollama.service.d/zz-chatonlm-models.conf\n\
         systemctl --user daemon-reload && systemctl --user restart ollama.service"
    );
    // Exécutées avec un faux systemctl : le drop-in est bien écrit.
    let tmp = temp_dir();
    let fake = "systemctl() { echo \"$*\" >> \"$HOME/systemctl.log\"; }\n";
    let output = sh_in(
        tmp.path(),
        &format!("{fake}{}", user_service_commands(TRICKY)),
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(
            tmp.path()
                .join(".config/systemd/user/ollama.service.d/zz-chatonlm-models.conf")
        )
        .unwrap(),
        format!(
            "[Service]\nEnvironment=\"OLLAMA_MODELS={}\"\n",
            TRICKY.replace('%', "%%")
        )
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("systemctl.log")).unwrap(),
        "--user daemon-reload\n--user restart ollama.service\n"
    );
}

#[test]
fn sudo_commands_write_the_same_dropin_as_the_script() {
    let commands = sudo_commands("/data/Models", "alice", "alice", Sandbox::None, true, None);
    assert_eq!(
        commands,
        "sudo mkdir -p '/data/Models' && sudo chown alice:alice '/data/Models'\n\
         sudo mkdir -p /etc/systemd/system/ollama.service.d\n\
         printf '%s\\n' '# Written by ChatonLM (Settings > Model location).' '[Unit]' 'RequiresMountsFor=\"/data/Models\"' '' '[Service]' 'Environment=\"OLLAMA_MODELS=/data/Models\"' | sudo tee /etc/systemd/system/ollama.service.d/zz-chatonlm-models.conf > /dev/null\n\
         sudo systemctl daemon-reload && sudo systemctl restart ollama.service"
    );
    let stopped = sudo_commands("/m", "ollama", "", Sandbox::None, false, None);
    assert!(stopped.contains("sudo chown ollama: '/m'"), "{stopped}");
    assert!(
        stopped.ends_with("\nsudo systemctl daemon-reload"),
        "{stopped}"
    );
    // La ligne printf, envoyée à un fichier au lieu de sudo tee, donne le
    // drop-in octet pour octet (apostrophes, %, espaces).
    let tmp = temp_dir();
    for sandbox in [Sandbox::None, Sandbox::Bind, Sandbox::ReadWrite] {
        let commands = sudo_commands(TRICKY, "alice", "alice", sandbox, true, None);
        let printf = commands
            .lines()
            .find(|line| line.starts_with("printf"))
            .unwrap()
            .replace(
                "| sudo tee /etc/systemd/system/ollama.service.d/zz-chatonlm-models.conf > /dev/null",
                "> dropin.conf",
            );
        let output = sh_in(tmp.path(), &printf);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            fs::read_to_string(tmp.path().join("dropin.conf")).unwrap(),
            dropin_text(TRICKY, sandbox, None)
        );
    }
}

#[test]
fn commands_depend_on_how_ollama_runs() {
    let mut detection = system_detection(Some(Path::new("/var/lib/ollama")), true);
    let commands = commands_for(&detection, "/home/alice/models").unwrap();
    assert!(
        commands.contains("'ProtectHome=tmpfs' 'BindPaths=\"/home/alice/models\"'"),
        "{commands}"
    );
    detection.location = ModelsLocation::UserService { dir: None };
    assert!(
        commands_for(&detection, "/m")
            .unwrap()
            .starts_with("mkdir -p ~/.config")
    );
    detection.location = ModelsLocation::Manual {
        dir: None,
        host_env: Some("127.0.0.1:11435".into()),
    };
    assert_eq!(
        commands_for(&detection, "/m").as_deref(),
        Some("OLLAMA_HOST=127.0.0.1:11435 OLLAMA_MODELS='/m' ollama serve")
    );
    detection.location = ModelsLocation::Unknown { running: true };
    detection.host_env = Some("localhost:8080".into());
    assert_eq!(
        commands_for(&detection, "/m").as_deref(),
        Some("OLLAMA_HOST=localhost:8080 OLLAMA_MODELS='/m' ollama serve")
    );
    for location in [
        ModelsLocation::Unknown { running: false },
        ModelsLocation::Container,
        ModelsLocation::Remote { host: "x".into() },
    ] {
        detection.location = location;
        assert_eq!(commands_for(&detection, "/m"), None);
    }
}

// --- Script, codes de sortie, progression -------------------------------------

#[test]
fn the_script_is_passed_whole_in_argv() {
    // 1re ligne : début du message de la boîte polkit ; bien sous la limite
    // d'un argument (MAX_ARG_STRLEN, 128 Kio).
    assert!(SCRIPT.starts_with("# ChatonLM: Ollama model dir\n"));
    assert!(SCRIPT.len() < 128 * 1024);
    for shell in shells() {
        let status = Command::new(&shell)
            .args(["-n", "-c", SCRIPT])
            .status()
            .unwrap();
        assert!(status.success(), "syntaxe refusée par {}", shell.display());
    }
}

#[test]
fn each_exit_code_has_its_outcome() {
    let failure = |code: i32, detail: &str| match outcome(Some(code), detail, None) {
        MoveOutcome::Failed {
            reason, restored, ..
        } => (reason, restored),
        other => panic!("{code}: {other:?}"),
    };
    let result = ScriptResult {
        restarted: true,
        renamed: 3,
        copied: 1,
        skipped: 2,
        cleanup_failed: true,
    };
    assert_eq!(
        outcome(Some(0), "", Some(result)),
        MoveOutcome::Done {
            restarted: true,
            renamed: 3,
            copied: 1,
            skipped: 2,
            cleanup_failed: true
        }
    );
    assert_eq!(failure(0, ""), (MoveFailure::Unexpected, false));
    assert_eq!(
        outcome(Some(126), "Request dismissed", None),
        MoveOutcome::Cancelled
    );
    assert_eq!(
        failure(
            127,
            "Error executing command as another user: No authentication agent found."
        ),
        (MoveFailure::NoAgent, false)
    );
    assert_eq!(
        failure(
            127,
            "Error executing command as another user: Not authorized"
        ),
        (MoveFailure::NotAuthorized, false)
    );
    assert_eq!(failure(10, ""), (MoveFailure::Refused, false));
    assert_eq!(failure(11, ""), (MoveFailure::UnitChanged, false));
    assert_eq!(failure(12, ""), (MoveFailure::Refused, false));
    assert_eq!(failure(13, ""), (MoveFailure::NotEmpty, false));
    assert_eq!(
        failure(
            14,
            "chatonlm: not enough space: 2048 KiB needed, 1024 KiB free"
        ),
        (
            MoveFailure::NoSpace {
                need: Some(2048 * 1024),
                free: Some(1024 * 1024)
            },
            false
        )
    );
    assert_eq!(
        failure(14, "?"),
        (
            MoveFailure::NoSpace {
                need: None,
                free: None
            },
            false
        )
    );
    assert_eq!(failure(15, ""), (MoveFailure::CannotWrite, false));
    assert_eq!(
        failure(16, "chatonlm: OLLAMA_MODELS is set in /etc/ollama env"),
        (
            MoveFailure::EnvironmentFile {
                file: Some("/etc/ollama env".into())
            },
            false
        )
    );
    assert_eq!(failure(17, ""), (MoveFailure::Unexpected, false));
    assert_eq!(failure(20, ""), (MoveFailure::StopFailed, true));
    assert_eq!(failure(21, ""), (MoveFailure::MoveFailed, true));
    assert_eq!(failure(22, ""), (MoveFailure::ConfigFailed, true));
    assert_eq!(failure(23, ""), (MoveFailure::Overridden, true));
    assert_eq!(failure(24, ""), (MoveFailure::StartFailed, true));
    assert_eq!(failure(30, ""), (MoveFailure::RestoreFailed, false));
    assert_eq!(failure(1, ""), (MoveFailure::Unexpected, false));
    // Signal.
    assert!(matches!(
        outcome(None, "", None),
        MoveOutcome::Failed {
            reason: MoveFailure::Unexpected,
            ..
        }
    ));
    // Détail transmis tel quel.
    assert_eq!(
        outcome(Some(21), "chatonlm: cannot copy", None),
        MoveOutcome::Failed {
            reason: MoveFailure::MoveFailed,
            restored: true,
            detail: "chatonlm: cannot copy".into()
        }
    );
}

#[test]
fn script_lines_are_parsed() {
    assert_eq!(
        parse_line("step prepare"),
        Some(Line::Step(MoveStep::Prepare))
    );
    assert_eq!(
        parse_line("step cleanup"),
        Some(Line::Step(MoveStep::Cleanup))
    );
    assert_eq!(parse_line("step dance"), None);
    assert_eq!(parse_line("total 43"), Some(Line::Total(43)));
    assert_eq!(
        parse_line("copy 9 manifests/hf.co/user/Some GGUF/Q4_K_M"),
        Some(Line::Copy {
            bytes: 9,
            path: "manifests/hf.co/user/Some GGUF/Q4_K_M".into()
        })
    );
    assert_eq!(parse_line("copy x blobs/a"), None);
    assert_eq!(parse_line("restored"), Some(Line::Restored));
    assert_eq!(
        parse_line("result restarted=no renamed=0 copied=5 skipped=1 cleanup=ok"),
        Some(Line::Result(ScriptResult {
            restarted: false,
            renamed: 0,
            copied: 5,
            skipped: 1,
            cleanup_failed: false
        }))
    );
    assert_eq!(parse_line("result renamed=x"), None);
    assert_eq!(parse_line(""), None);
}

#[test]
fn progress_counts_finished_files_and_the_one_being_copied() {
    let tmp = temp_dir();
    let new = tmp.path().join("new");
    fs::create_dir_all(new.join("blobs")).unwrap();
    let mut reader = Reader {
        new: new.clone(),
        completed: 0,
        current: None,
        last_done: None,
        result: None,
    };
    let mut events = Vec::new();
    let mut stops = 0;
    let mut feed = |reader: &mut Reader, line: &str, events: &mut Vec<MoveEvent>| {
        reader.handle(line, &mut |event| events.push(event), &mut || stops += 1);
    };
    feed(&mut reader, "step stop", &mut events);
    feed(&mut reader, "step move", &mut events);
    feed(&mut reader, "total 1000", &mut events);
    feed(&mut reader, "copy 600 blobs/sha256-big", &mut events);
    // Copie en cours : 250 octets déjà écrits.
    fs::write(new.join("blobs/sha256-big"), vec![0u8; 250]).unwrap();
    reader.poll(&mut |event| events.push(event));
    // Relevé identique : rien de plus.
    reader.poll(&mut |event| events.push(event));
    feed(&mut reader, "copy 400 blobs/sha256-small", &mut events);
    reader.poll(&mut |event| events.push(event));
    feed(&mut reader, "step config", &mut events);
    feed(
        &mut reader,
        "result restarted=yes renamed=0 copied=2 skipped=0 cleanup=ok",
        &mut events,
    );
    drop(feed);
    assert_eq!(stops, 1);
    assert_eq!(
        events,
        [
            MoveEvent::Step {
                step: MoveStep::Stop
            },
            MoveEvent::Step {
                step: MoveStep::Move
            },
            MoveEvent::Total { bytes: 1000 },
            MoveEvent::Progress { done: 0 },
            MoveEvent::Progress { done: 250 },
            MoveEvent::Progress { done: 600 },
            MoveEvent::Progress { done: 1000 },
            MoveEvent::Step {
                step: MoveStep::Config
            },
        ]
    );
    assert!(reader.result.is_some());
}

/// Ligne non UTF-8 sur la sortie du script (nom de fichier en Latin-1) : la
/// lecture continue, la ligne `result` arrive, le déplacement réussi n'est
/// pas rapporté comme un échec.
#[tokio::test]
async fn a_line_that_is_not_utf8_does_not_stop_the_reading() {
    let tmp = temp_dir();
    let request = Request {
        pkexec: Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/ollama/models_dir/fake-bin/latin1-output"),
        shell: PathBuf::from(SHELL),
        move_models: true,
        old: Some("/var/lib/ollama".into()),
        new: path_str(&tmp.path().join("new")).to_owned(),
        envs: Vec::new(),
    };
    let mut lines = Vec::new();
    let mut events = Vec::new();
    let outcome = run_with(
        request,
        |event| events.push(event),
        || {},
        |line| {
            lines.push(line.to_owned());
        },
    )
    .await;
    assert_eq!(
        outcome,
        MoveOutcome::Done {
            restarted: true,
            renamed: 0,
            copied: 1,
            skipped: 0,
            cleanup_failed: false,
        }
    );
    assert!(
        lines.contains(&"copy 5 blobs/caf\u{fffd}".to_owned()),
        "{lines:?}"
    );
    assert_eq!(lines.len(), 9, "{lines:?}");
    assert!(events.contains(&MoveEvent::Step {
        step: MoveStep::Cleanup
    }));
    assert!(events.contains(&MoveEvent::Progress { done: 5 }));
}

#[test]
fn one_operation_at_a_time() {
    static LOCK: OperationLock = OperationLock::new();
    assert!(!LOCK.is_held());
    let first = LOCK.try_acquire();
    assert!(first.is_some());
    assert!(LOCK.is_held());
    assert!(LOCK.try_acquire().is_none(), "seconde opération acceptée");
    drop(first);
    assert!(!LOCK.is_held());
    assert!(LOCK.try_acquire().is_some());
}

#[test]
fn only_a_setuid_root_pkexec_is_used() {
    let tmp = temp_dir();
    let fake = tmp.path().join("pkexec");
    fs::write(&fake, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o4755)).unwrap();
    // Setuid, mais pas à root.
    assert!(!is_setuid_root(&fake));
    assert!(!is_setuid_root(&tmp.path().join("absent")));
    if let Ok(meta) = fs::metadata("/usr/bin/pkexec")
        && meta.uid() == 0
        && meta.mode() & 0o4000 != 0
    {
        assert!(is_setuid_root(Path::new("/usr/bin/pkexec")));
    }
}

/// Contrat : l'instance de test (`scripts/test-ollama.sh start`, port 11435,
/// lancée à la main) est classée « Manual », avec le dossier lu dans
/// l'environnement de son processus.
#[tokio::test]
#[ignore = "instance Ollama de test sur 127.0.0.1:11435"]
async fn contract_manual_launch_of_the_test_instance() {
    let detection = detect("http://127.0.0.1:11435").await;
    let ModelsLocation::Manual {
        dir: Some(dir),
        host_env,
    } = &detection.location
    else {
        panic!("lancement à la main attendu : {detection:?}");
    };
    assert!(dir.ends_with("/.tools/ollama-test/models"), "{dir}");
    assert!(Path::new(dir).is_dir(), "{dir}");
    assert_eq!(host_env.as_deref(), Some("127.0.0.1:11435"));
    // Le résumé des modèles vient de la même instance.
    assert!(models_summary("http://127.0.0.1:11435").await.is_some());
}

/// Service système de cette machine (lecture seule : D-Bus et /proc).
#[tokio::test]
#[ignore = "dépend de la machine : ollama.service sur 127.0.0.1:11434"]
async fn reads_the_location_of_this_machine() {
    let detection = detect("http://127.0.0.1:11434").await;
    eprintln!("{detection:#?}");
    assert!(
        matches!(
            detection.location,
            ModelsLocation::SystemService { dir: Some(_), .. }
        ),
        "{detection:?}"
    );
}
