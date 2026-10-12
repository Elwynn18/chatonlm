//! Banc du script, porté de `.tools/research/36-models-dir-bench/bench.sh` :
//! le texte embarqué est lancé par [`run_with`] (le code de l'app) à travers
//! le faux pkexec de `fake-bin/`, sous `/bin/sh` et `dash` s'il existe, avec
//! une racine simulée (`CHATONLM_TEST_ROOT` : etc/, run/, proc/, état du faux
//! service) ; faux `systemctl`, `chown`, `setpriv`, `cp`, `sleep` en tête du
//! PATH (`CHATONLM_TEST_BIN`). Rien n'est exécuté en root, rien n'est écrit
//! hors du dossier temporaire du projet. Tous les chemins contiennent une
//! espace et un accent.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::tests::{find_dash, temp_dir};
use super::*;

/// Faux programmes du banc (gardés exécutables dans le dépôt : rien n'est
/// écrit puis exécuté pendant les tests).
fn fake_bin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ollama/models_dir/fake-bin")
}

/// Résultat d'un passage du script.
struct Ran {
    outcome: MoveOutcome,
    events: Vec<MoveEvent>,
    lines: Vec<String>,
    stops: usize,
}

impl Ran {
    fn failure(&self) -> Option<(&MoveFailure, bool)> {
        match &self.outcome {
            MoveOutcome::Failed {
                reason, restored, ..
            } => Some((reason, *restored)),
            _ => None,
        }
    }

    fn printed(&self, line: &str) -> bool {
        self.lines.iter().any(|seen| seen == line)
    }
}

/// Un cas : racine simulée, anciens modèles, faux service actif (PID 4000).
struct Case {
    name: &'static str,
    shell: PathBuf,
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    root: PathBuf,
    old: PathBuf,
    envs: Vec<(OsString, OsString)>,
}

impl Case {
    fn new(name: &'static str, shell: &Path) -> Case {
        let tmp = temp_dir();
        let dir = fs::canonicalize(tmp.path()).unwrap().join("cas é");
        let root = dir.join("root");
        for sub in ["run", "log", "state", "proc/4000", "etc/systemd/system"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        let old = dir.join("var-lib-ollama");
        mkmodels(&old);
        let case = Case {
            name,
            shell: shell.to_path_buf(),
            _tmp: tmp,
            envs: vec![
                ("CHATONLM_TEST_ROOT".into(), root.clone().into_os_string()),
                ("CHATONLM_TEST_BIN".into(), fake_bin().into_os_string()),
            ],
            dir,
            root,
            old,
        };
        case.state("base_models", case.old.to_str().unwrap());
        case.state("active", "active");
        case.state("pid", "4000");
        fs::write(
            case.root.join("proc/4000/environ"),
            format!("OLLAMA_MODELS={}\0", case.old.display()),
        )
        .unwrap();
        case
    }

    fn set(&mut self, name: &str, value: &str) {
        self.envs.push((name.into(), value.into()));
    }

    fn state(&self, name: &str, value: &str) {
        fs::write(self.root.join("state").join(name), format!("{value}\n")).unwrap();
    }

    fn state_is(&self, name: &str) -> String {
        fs::read_to_string(self.root.join("state").join(name))
            .unwrap_or_default()
            .trim_end()
            .to_owned()
    }

    fn log(&self, name: &str) -> String {
        let bytes = fs::read(self.root.join("log").join(format!("{name}.log"))).unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Dernière ligne du journal du faux pkexec (arguments après le script,
    /// coupés à 200 octets).
    fn pkexec_arguments(&self) -> Vec<u8> {
        let bytes = fs::read(self.root.join("log/pkexec.log")).unwrap_or_default();
        let bytes = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
        let start = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |i| i + 1);
        bytes[start..].to_vec()
    }

    fn dropins(&self) -> PathBuf {
        self.root.join("etc/systemd/system/ollama.service.d")
    }

    fn dropin(&self) -> PathBuf {
        self.dropins().join(DROPIN_NAME)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.join(relative)
    }

    /// `move OLD NEW` (move_old) ou `keep - NEW`.
    async fn run(&self, move_old: bool, new: &str) -> Ran {
        let request = Request {
            pkexec: fake_bin().join("pkexec"),
            shell: self.shell.clone(),
            move_models: move_old,
            old: Some(self.old.to_str().unwrap().to_owned()),
            new: new.to_owned(),
            envs: self.envs.clone(),
        };
        let mut events = Vec::new();
        let mut lines = Vec::new();
        let mut stops = 0;
        let outcome = run_with(
            request,
            |event| events.push(event),
            || stops += 1,
            |line| lines.push(line.to_owned()),
        )
        .await;
        Ran {
            outcome,
            events,
            lines,
            stops,
        }
    }
}

/// Dossier de modèles comme Ollama le laisse (plus un téléchargement
/// interrompu).
fn mkmodels(dir: &Path) {
    for sub in [
        "blobs",
        "manifests/registry.ollama.ai/library/qwen3",
        "manifests/hf.co/bartowski/Some-GGUF",
        "metadata",
    ] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    let files = [
        ("blobs/sha256-aaaa", "weights-A"),
        ("blobs/sha256-bbbb", "weights-B"),
        ("blobs/sha256-cccc-partial", "half"),
        (
            "manifests/registry.ollama.ai/library/qwen3/0.6b",
            "{\"qwen3\":\"old\"}",
        ),
        ("manifests/hf.co/bartowski/Some-GGUF/Q4_K_M", "{\"hf\":1}"),
        ("metadata/sha256-aaaa.json", "{}"),
    ];
    for (file, content) in files {
        fs::write(dir.join(file), content).unwrap();
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn id(flag: &str, user: Option<&str>) -> String {
    let mut command = std::process::Command::new("id");
    command.arg(flag);
    if let Some(user) = user {
        command.args(["--", user]);
    }
    let output = command.output().unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// Vérifications d'un shell ; les échecs sont rassemblés, comme le banc.
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

const DONE_RENAMED: MoveOutcome = MoveOutcome::Done {
    restarted: true,
    renamed: 3,
    copied: 0,
    skipped: 0,
    cleanup_failed: false,
};

async fn bench(shell: &Path) {
    let mut c = Checks {
        failures: Vec::new(),
        passed: 0,
    };

    // Même disque : renommages, dans l'ordre stop → daemon-reload → start.
    let case = Case::new("rename-same-disk", shell);
    let new = case.path("data/Models");
    fs::create_dir_all(case.path("data")).unwrap();
    let ran = case.run(true, text(&new)).await;
    let r = Some(&ran);
    c.check(&case, "succès", ran.outcome == DONE_RENAMED, r);
    c.check(
        &case,
        "blobs renommés",
        new.join("blobs/sha256-aaaa").exists(),
        r,
    );
    c.check(
        &case,
        "partiel suit le renommage",
        new.join("blobs/sha256-cccc-partial").exists(),
        r,
    );
    c.check(
        &case,
        "manifeste hf.co",
        new.join("manifests/hf.co/bartowski/Some-GGUF/Q4_K_M")
            .exists(),
        r,
    );
    c.check(
        &case,
        "plus rien dans l'ancien",
        !case.old.join("blobs").exists(),
        r,
    );
    c.check(&case, "rien d'autre touché", case.old.exists(), r);
    c.check(
        &case,
        "drop-in rendu comme dropin_text",
        read(&case.dropin()) == dropin_text(text(&new), Sandbox::None, None),
        r,
    );
    c.check(
        &case,
        "arrêt, daemon-reload, démarrage",
        case.log("systemctl")
            .lines()
            .filter(|line| {
                ["stop", "daemon-reload", "start"]
                    .iter()
                    .any(|w| line.starts_with(w))
            })
            .collect::<Vec<_>>()
            == [
                "stop -- ollama.service",
                "daemon-reload",
                "start -- ollama.service",
            ],
        r,
    );
    c.check(
        &case,
        "service actif",
        case.state_is("active") == "active",
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
        "données en droits du service",
        case.log("setpriv").lines().any(|line| {
            line == format!(
                "--reuid={} --regid={} --init-groups -- mv -- {}/blobs {}/blobs",
                id("-u", None),
                id("-g", None),
                case.old.display(),
                new.display()
            )
        }),
        r,
    );
    c.check(
        &case,
        "étapes annoncées dans l'ordre",
        ran.events
            == [
                MoveEvent::Step {
                    step: MoveStep::Prepare,
                },
                MoveEvent::Step {
                    step: MoveStep::Stop,
                },
                MoveEvent::Step {
                    step: MoveStep::Move,
                },
                MoveEvent::Total { bytes: 0 },
                MoveEvent::Step {
                    step: MoveStep::Config,
                },
                MoveEvent::Step {
                    step: MoveStep::Start,
                },
                MoveEvent::Step {
                    step: MoveStep::Cleanup,
                },
            ],
        r,
    );
    c.check(&case, "réponses arrêtées une fois", ran.stops == 1, r);
    let arguments = format!(
        " chatonlm-models ollama.service move {} {}",
        case.old.display(),
        new.display()
    );
    c.check(
        &case,
        "arguments après le script, nouveau dossier en dernier (boîte polkit)",
        // Coupés à 200 octets ou caractères selon la version de cut.
        {
            let logged = case.pkexec_arguments();
            arguments.as_bytes().starts_with(&logged) && logged.len() >= arguments.len().min(200)
        },
        r,
    );

    // Autre disque (forcé), chemin avec %, espace et accent : copie.
    let mut case = Case::new("copy-other-disk-space-percent", shell);
    case.set("CHATONLM_TEST_FORCE_COPY", "1");
    let new = case.path("disk 2/100% modèles é");
    fs::create_dir_all(case.path("disk 2")).unwrap();
    let ran = case.run(true, text(&new)).await;
    let r = Some(&ran);
    c.check(
        &case,
        "succès",
        ran.outcome
            == MoveOutcome::Done {
                restarted: true,
                renamed: 0,
                copied: 5,
                skipped: 0,
                cleanup_failed: false,
            },
        r,
    );
    c.check(
        &case,
        "blob copié",
        read(&new.join("blobs/sha256-bbbb")) == "weights-B",
        r,
    );
    c.check(
        &case,
        "manifeste copié",
        read(&new.join("manifests/registry.ollama.ai/library/qwen3/0.6b")) == "{\"qwen3\":\"old\"}",
        r,
    );
    c.check(
        &case,
        "partiel laissé",
        !new.join("blobs/sha256-cccc-partial").exists(),
        r,
    );
    c.check(
        &case,
        "anciennes copies effacées",
        !case.old.join("manifests").exists(),
        r,
    );
    c.check(
        &case,
        "% doublé, drop-in rendu comme dropin_text",
        read(&case.dropin()) == dropin_text(text(&new), Sandbox::None, None)
            && read(&case.dropin()).contains("100%% modèles é"),
        r,
    );
    c.check(
        &case,
        "total annoncé",
        ran.events.contains(&MoveEvent::Total { bytes: 43 }),
        r,
    );
    c.check(
        &case,
        "ligne de progression",
        ran.printed("copy 9 blobs/sha256-aaaa"),
        r,
    );
    let progress: Vec<u64> = ran
        .events
        .iter()
        .filter_map(|event| match event {
            MoveEvent::Progress { done } => Some(*done),
            _ => None,
        })
        .collect();
    c.check(
        &case,
        "progression croissante jusqu'au total",
        progress.windows(2).all(|pair| pair[0] <= pair[1]) && progress.last() == Some(&43),
        r,
    );

    // Invite fermée : rien d'exécuté.
    let mut case = Case::new("cancel", shell);
    case.set("FAKE_PKEXEC", "cancel");
    let new = case.path("data-Models");
    let ran = case.run(true, text(&new)).await;
    let r = Some(&ran);
    c.check(&case, "annulé", ran.outcome == MoveOutcome::Cancelled, r);
    c.check(&case, "rien créé", !new.exists(), r);
    c.check(&case, "pas de drop-in", !case.dropin().exists(), r);
    c.check(
        &case,
        "modèles en place",
        case.old.join("blobs/sha256-aaaa").exists(),
        r,
    );
    c.check(
        &case,
        "systemctl jamais appelé",
        case.log("systemctl").is_empty(),
        r,
    );
    c.check(&case, "réponses laissées", ran.stops == 0, r);

    // Pas d'agent d'authentification.
    let mut case = Case::new("no-agent", shell);
    case.set("FAKE_PKEXEC", "noagent");
    let ran = case.run(true, text(&case.path("data-Models"))).await;
    let r = Some(&ran);
    c.check(
        &case,
        "sans agent",
        ran.failure() == Some((&MoveFailure::NoAgent, false)),
        r,
    );
    c.check(
        &case,
        "message de pkexec dans le détail",
        matches!(&ran.outcome, MoveOutcome::Failed { detail, .. } if detail.contains("No authentication agent found")),
        r,
    );

    // La copie échoue au 3e fichier : tout est remis.
    let mut case = Case::new("copy-fails-midway", shell);
    case.set("CHATONLM_TEST_FORCE_COPY", "1");
    case.set("FAKE_FAIL", "cp-after-3");
    let new = case.path("data-Models");
    let ran = case.run(true, text(&new)).await;
    let r = Some(&ran);
    c.check(
        &case,
        "code 21",
        ran.failure() == Some((&MoveFailure::MoveFailed, true)),
        r,
    );
    c.check(&case, "restauré", ran.printed("restored"), r);
    c.check(&case, "nouveau dossier retiré", !new.exists(), r);
    c.check(
        &case,
        "anciens modèles intacts",
        read(&case.old.join("blobs/sha256-bbbb")) == "weights-B",
        r,
    );
    c.check(&case, "pas de drop-in", !case.dropin().exists(), r);
    c.check(
        &case,
        "service relancé",
        case.state_is("active") == "active",
        r,
    );
    c.check(
        &case,
        "relancé une fois",
        case.log("systemctl")
            .lines()
            .filter(|line| line.starts_with("start"))
            .count()
            == 1,
        r,
    );
    c.check(
        &case,
        "détail du script",
        matches!(&ran.outcome, MoveOutcome::Failed { detail, .. } if detail.contains("chatonlm: cannot copy")),
        r,
    );

    // Ollama ne démarre pas sur le nouveau dossier : ancien drop-in remis.
    let mut case = Case::new("crash-on-new-folder", shell);
    case.set("FAKE_FAIL", "crash-new");
    fs::create_dir_all(case.dropins()).unwrap();
    fs::write(
        case.dropin(),
        format!(
            "# previous ChatonLM choice\n[Service]\nEnvironment=\"OLLAMA_MODELS={}\"\n",
            case.old.display()
        ),
    )
    .unwrap();
    let new = case.path("data-Models");
    let ran = case.run(true, text(&new)).await;
    let r = Some(&ran);
    c.check(
        &case,
        "code 24",
        ran.failure() == Some((&MoveFailure::StartFailed, true)),
        r,
    );
    c.check(&case, "restauré", ran.printed("restored"), r);
    c.check(
        &case,
        "modèles revenus",
        case.old.join("blobs/sha256-aaaa").exists(),
        r,
    );
    c.check(
        &case,
        "ancien drop-in remis",
        read(&case.dropin()).starts_with("# previous ChatonLM choice\n"),
        r,
    );
    c.check(
        &case,
        "configuration rechargée",
        case.state_is("loaded_models") == text(&case.old),
        r,
    );
    c.check(
        &case,
        "service relancé",
        case.state_is("active") == "active",
        r,
    );
    c.check(&case, "nouveau dossier retiré", !new.exists(), r);

    // Un drop-in plus tardif l'emporte : refus, tout est remis.
    let case = Case::new("overridden-by-later-drop-in", shell);
    fs::create_dir_all(case.dropins()).unwrap();
    fs::write(
        case.dropins().join("zzz-admin.conf"),
        "[Service]\nEnvironment=\"OLLAMA_MODELS=/srv/elsewhere\"\n",
    )
    .unwrap();
    let ran = case.run(true, text(&case.path("data-Models"))).await;
    let r = Some(&ran);
    c.check(
        &case,
        "code 23",
        ran.failure() == Some((&MoveFailure::Overridden, true)),
        r,
    );
    c.check(
        &case,
        "modèles revenus",
        case.old
            .join("manifests/hf.co/bartowski/Some-GGUF/Q4_K_M")
            .exists(),
        r,
    );
    c.check(&case, "drop-in retiré", !case.dropin().exists(), r);

    // override.conf de `systemctl edit` passe avant zz-.
    let case = Case::new("override-conf-sorts-first", shell);
    fs::create_dir_all(case.dropins()).unwrap();
    fs::write(
        case.dropins().join("override.conf"),
        "[Service]\nEnvironment=\"OLLAMA_MODELS=/srv/user-choice\"\n",
    )
    .unwrap();
    let ran = case.run(true, text(&case.path("data-Models"))).await;
    c.check(
        &case,
        "zz- l'emporte sur override.conf",
        ran.outcome == DONE_RENAMED,
        Some(&ran),
    );

    // Refus avant toute modification.
    let case = Case::new("refusals", shell);
    let bad = [
        "/etc/ollama".to_owned(),
        "/usr/share/ollama".to_owned(),
        "/var/lib".to_owned(),
        "/".to_owned(),
        "/home".to_owned(),
        format!("{}/a:b", case.dir.display()),
        "relative/path".to_owned(),
        format!("{}/x/../y", case.dir.display()),
        format!("{}/quote\"d", case.dir.display()),
        format!("{}/back\\slash", case.dir.display()),
        format!("{}/dollar$x", case.dir.display()),
        format!("{}/new\nline", case.dir.display()),
    ];
    for path in &bad {
        let ran = case.run(false, path).await;
        c.check(
            &case,
            &format!("refus de {path:?}"),
            ran.failure() == Some((&MoveFailure::Refused, false)),
            Some(&ran),
        );
        // Mêmes refus côté app, avant l'invite.
        c.check(
            &case,
            &format!("refus de {path:?} par l'app"),
            !clean_path(path) || forbidden(path),
            None,
        );
    }
    c.check(
        &case,
        "rien demandé à systemctl",
        case.log("systemctl").is_empty(),
        None,
    );
    let documents = case.path("documents");
    fs::create_dir_all(&documents).unwrap();
    fs::write(documents.join("cv.odt"), "cv").unwrap();
    let ran = case.run(false, text(&documents)).await;
    c.check(
        &case,
        "dossier non vide (13)",
        ran.failure() == Some((&MoveFailure::NotEmpty, false)),
        Some(&ran),
    );
    let ran = case.run(true, text(&case.old.join("sub"))).await;
    c.check(
        &case,
        "dans l'ancien dossier (12)",
        ran.failure() == Some((&MoveFailure::Refused, false)),
        Some(&ran),
    );
    let link = case.path("link");
    std::os::unix::fs::symlink(&documents, &link).unwrap();
    let ran = case.run(false, text(&link)).await;
    c.check(
        &case,
        "lien symbolique refusé (10)",
        ran.failure() == Some((&MoveFailure::Refused, false)),
        Some(&ran),
    );
    c.check(
        &case,
        "cv intact",
        read(&documents.join("cv.odt")) == "cv",
        None,
    );

    // Sous un /home masqué (ProtectHome=yes) : tmpfs + BindPaths.
    let mut case = Case::new("home-protected", shell);
    let home = case.path("home");
    case.set("CHATONLM_TEST_HOME", text(&home));
    fs::create_dir_all(home.join("alice")).unwrap();
    let new = home.join("alice/LLM models");
    let ran = case.run(true, text(&new)).await;
    let r = Some(&ran);
    c.check(&case, "succès", ran.outcome == DONE_RENAMED, r);
    c.check(
        &case,
        "drop-in avec BindPaths",
        read(&case.dropin()) == dropin_text(text(&new), Sandbox::Bind, None),
        r,
    );

    let mut case = Case::new("home-read-only", shell);
    let home = case.path("home");
    case.set("CHATONLM_TEST_HOME", text(&home));
    case.state("protect_home", "read-only");
    fs::create_dir_all(home.join("alice")).unwrap();
    let new = home.join("alice/models");
    let ran = case.run(true, text(&new)).await;
    c.check(
        &case,
        "ReadWritePaths sous ProtectHome=read-only",
        read(&case.dropin()) == dropin_text(text(&new), Sandbox::ReadWrite, None),
        Some(&ran),
    );

    // HOME du service sous /home (User=alice, HOME=/home/alice) : son .ollama
    // (clé id_ed25519, sans laquelle `ollama serve` ne démarre pas sur un
    // /home vide) est monté aussi, créé par le service s'il manque.
    let mut case = Case::new("home-protected-service-home", shell);
    let home = case.path("home");
    case.set("CHATONLM_TEST_HOME", text(&home));
    let service_home = home.join("alice é");
    fs::create_dir_all(&service_home).unwrap();
    case.state("home", text(&service_home));
    let ollama = service_home.join(".ollama");
    let new = service_home.join("LLM models");
    let ran = case.run(true, text(&new)).await;
    let r = Some(&ran);
    c.check(&case, "succès", ran.outcome == DONE_RENAMED, r);
    c.check(
        &case,
        "drop-in avec le .ollama du service",
        read(&case.dropin()) == dropin_text(text(&new), Sandbox::Bind, Some(text(&ollama)))
            && read(&case.dropin()).ends_with(&format!(
                "BindPaths=\"{}\"\nBindPaths=\"{}\"\n",
                new.display(),
                ollama.display()
            )),
        r,
    );
    c.check(&case, ".ollama créé", ollama.is_dir(), r);
    c.check(
        &case,
        ".ollama créé avec les droits du service",
        case.log("setpriv")
            .lines()
            .any(|line| line.ends_with(&format!("-- mkdir -m 0755 -- {}", ollama.display()))),
        r,
    );

    // Ollama ne démarre pas : le .ollama créé est retiré avec le reste.
    let mut case = Case::new("home-protected-service-home-crash", shell);
    let home = case.path("home");
    case.set("CHATONLM_TEST_HOME", text(&home));
    case.set("FAKE_FAIL", "crash-new");
    let service_home = home.join("alice");
    fs::create_dir_all(&service_home).unwrap();
    case.state("home", text(&service_home));
    let ran = case.run(true, text(&service_home.join("models"))).await;
    let r = Some(&ran);
    c.check(
        &case,
        "code 24",
        ran.failure() == Some((&MoveFailure::StartFailed, true)),
        r,
    );
    c.check(
        &case,
        ".ollama créé puis retiré",
        !service_home.join(".ollama").exists(),
        r,
    );

    // .ollama qui n'est pas un dossier : refus avant d'arrêter le service.
    let mut case = Case::new("home-protected-ollama-file", shell);
    let home = case.path("home");
    case.set("CHATONLM_TEST_HOME", text(&home));
    let service_home = home.join("alice");
    fs::create_dir_all(&service_home).unwrap();
    fs::write(service_home.join(".ollama"), "file").unwrap();
    case.state("home", text(&service_home));
    let new = service_home.join("models");
    let ran = case.run(true, text(&new)).await;
    let r = Some(&ran);
    c.check(
        &case,
        "code 12",
        ran.failure() == Some((&MoveFailure::Refused, false)),
        r,
    );
    c.check(&case, "nouveau dossier retiré", !new.exists(), r);
    c.check(
        &case,
        "service jamais arrêté",
        !case
            .log("systemctl")
            .lines()
            .any(|line| line.starts_with("stop")),
        r,
    );

    // .ollama lien symbolique (systemd suivrait sa source à chaque
    // démarrage), ou dossier d'un autre compte : refus avant l'arrêt.
    for name in [
        "home-protected-ollama-symlink",
        "home-protected-ollama-of-another-user",
    ] {
        let mut case = Case::new(name, shell);
        let home = case.path("home");
        case.set("CHATONLM_TEST_HOME", text(&home));
        let service_home = home.join("alice");
        let ollama = service_home.join(".ollama");
        fs::create_dir_all(&service_home).unwrap();
        if name.ends_with("symlink") {
            let target = case.path("elsewhere");
            fs::create_dir_all(&target).unwrap();
            std::os::unix::fs::symlink(&target, &ollama).unwrap();
        } else {
            fs::create_dir_all(&ollama).unwrap();
            case.state("user", "nobody");
            case.state("group", &id("-gn", Some("nobody")));
        }
        case.state("home", text(&service_home));
        let new = service_home.join("models");
        let ran = case.run(true, text(&new)).await;
        let r = Some(&ran);
        c.check(
            &case,
            "code 12",
            ran.failure() == Some((&MoveFailure::Refused, false))
                && matches!(&ran.outcome, MoveOutcome::Failed { detail, .. } if detail.contains("not a folder of")),
            r,
        );
        c.check(&case, "nouveau dossier retiré", !new.exists(), r);
        c.check(&case, "pas de drop-in", !case.dropin().exists(), r);
        c.check(
            &case,
            "service jamais arrêté",
            !case
                .log("systemctl")
                .lines()
                .any(|line| line.starts_with("stop")),
            r,
        );
    }

    // Modèles dans le .ollama lui-même : un seul BindPaths=.
    let mut case = Case::new("models-in-the-ollama-folder", shell);
    let home = case.path("home");
    case.set("CHATONLM_TEST_HOME", text(&home));
    let service_home = home.join("alice");
    let ollama = service_home.join(".ollama");
    fs::create_dir_all(&ollama).unwrap();
    case.state("home", text(&service_home));
    let ran = case.run(true, text(&ollama)).await;
    c.check(
        &case,
        "drop-in sans second montage",
        ran.outcome == DONE_RENAMED
            && read(&case.dropin()) == dropin_text(text(&ollama), Sandbox::Bind, None),
        Some(&ran),
    );

    // Cible qui contient déjà des modèles : fusion.
    let mut case = Case::new("merge", shell);
    case.set("CHATONLM_TEST_FORCE_COPY", "1");
    let new = case.path("existing");
    fs::create_dir_all(new.join("blobs")).unwrap();
    fs::create_dir_all(new.join("manifests/registry.ollama.ai/library/qwen3")).unwrap();
    fs::write(new.join("blobs/sha256-aaaa"), "weights-A").unwrap();
    fs::write(
        new.join("manifests/registry.ollama.ai/library/qwen3/0.6b"),
        "{\"qwen3\":\"new\"}",
    )
    .unwrap();
    let ran = case.run(true, text(&new)).await;
    let r = Some(&ran);
    c.check(
        &case,
        "succès",
        ran.outcome
            == MoveOutcome::Done {
                restarted: true,
                renamed: 0,
                copied: 3,
                skipped: 2,
                cleanup_failed: false,
            },
        r,
    );
    c.check(
        &case,
        "manifeste de la cible gardé",
        read(&new.join("manifests/registry.ollama.ai/library/qwen3/0.6b")) == "{\"qwen3\":\"new\"}",
        r,
    );
    c.check(
        &case,
        "blob manquant ajouté",
        read(&new.join("blobs/sha256-bbbb")) == "weights-B",
        r,
    );
    c.check(
        &case,
        "total sans les fichiers déjà là",
        ran.events.contains(&MoveEvent::Total { bytes: 19 }),
        r,
    );

    // Service arrêté : ni arrêt ni démarrage.
    let case = Case::new("service-stopped", shell);
    case.state("active", "inactive");
    let ran = case.run(true, text(&case.path("data-Models"))).await;
    let r = Some(&ran);
    c.check(
        &case,
        "succès sans redémarrage",
        ran.outcome
            == MoveOutcome::Done {
                restarted: false,
                renamed: 3,
                copied: 0,
                skipped: 0,
                cleanup_failed: false,
            },
        r,
    );
    c.check(
        &case,
        "ni arrêt ni démarrage",
        !case
            .log("systemctl")
            .lines()
            .any(|line| line.starts_with("stop") || line.starts_with("start")),
        r,
    );

    // Garder les modèles où ils sont.
    let case = Case::new("keep", shell);
    let new = case.path("data-Models");
    let ran = case.run(false, text(&new)).await;
    let r = Some(&ran);
    c.check(
        &case,
        "succès",
        ran.outcome
            == MoveOutcome::Done {
                restarted: true,
                renamed: 0,
                copied: 0,
                skipped: 0,
                cleanup_failed: false,
            },
        r,
    );
    c.check(
        &case,
        "modèles laissés",
        case.old.join("blobs/sha256-aaaa").exists(),
        r,
    );
    c.check(&case, "nouveau dossier pris", new.join("blobs").exists(), r);
    c.check(
        &case,
        "ancien dossier non passé",
        case.log("pkexec").contains(" keep - "),
        r,
    );

    // OLLAMA_MODELS dans un EnvironmentFile= : refus.
    let case = Case::new("environment-file", shell);
    let env_file = case.path("ollama.env");
    fs::write(&env_file, "OLLAMA_MODELS=/srv/x\n").unwrap();
    case.state(
        "envfiles",
        &format!("{} (ignore_errors=yes)", env_file.display()),
    );
    let new = case.path("data-Models");
    let ran = case.run(true, text(&new)).await;
    let r = Some(&ran);
    c.check(
        &case,
        "code 16 et fichier",
        ran.failure()
            == Some((
                &MoveFailure::EnvironmentFile {
                    file: Some(text(&env_file).to_owned()),
                },
                false,
            )),
        r,
    );
    c.check(&case, "rien créé", !new.exists(), r);

    // Le service ne peut pas écrire dans le dossier.
    let mut case = Case::new("service-user-cannot-write", shell);
    case.set("FAKE_FAIL", "setpriv");
    let new = case.path("data-Models");
    let ran = case.run(true, text(&new)).await;
    let r = Some(&ran);
    c.check(
        &case,
        "code 15",
        ran.failure() == Some((&MoveFailure::CannotWrite, false)),
        r,
    );
    c.check(&case, "dossier créé puis retiré", !new.exists(), r);
    c.check(
        &case,
        "service jamais arrêté",
        !case
            .log("systemctl")
            .lines()
            .any(|line| line.starts_with("stop")),
        r,
    );

    // Service d'un autre utilisateur (nobody ici, ollama chez l'officiel).
    let case = Case::new("other-user-gets-folder", shell);
    let group = id("-gn", Some("nobody"));
    case.state("user", "nobody");
    case.state("group", &group);
    let new = case.path("data-Models");
    fs::create_dir_all(&new).unwrap();
    let ran = case.run(false, text(&new)).await;
    let r = Some(&ran);
    let gid = String::from_utf8(
        std::process::Command::new("getent")
            .args(["group", &group])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .split(':')
    .nth(2)
    .unwrap_or_default()
    .to_owned();
    c.check(
        &case,
        "chown vers le service, épinglé",
        case.log("chown")
            .lines()
            .any(|line| line == format!("-h -- {}:{gid} .", id("-u", Some("nobody")))),
        r,
    );
    c.check(
        &case,
        "écriture testée en tant que service",
        case.log("setpriv")
            .contains(&format!("--reuid={}", id("-u", Some("nobody")))),
        r,
    );

    // Service en root (pas de `User=`) : refus avant toute modification (11),
    // l'app donne les commandes à la place.
    let case = Case::new("root-service", shell);
    case.state("user", "");
    let new = case.path("data-Models");
    let ran = case.run(true, text(&new)).await;
    let r = Some(&ran);
    c.check(
        &case,
        "code 11",
        ran.failure() == Some((&MoveFailure::UnitChanged, false)),
        r,
    );
    c.check(
        &case,
        "raison dans le détail",
        matches!(&ran.outcome, MoveOutcome::Failed { detail, .. } if detail.contains("runs as root")),
        r,
    );
    c.check(&case, "rien créé", !new.exists(), r);
    c.check(
        &case,
        "service jamais arrêté",
        !case
            .log("systemctl")
            .lines()
            .any(|line| line.starts_with("stop")),
        r,
    );
    let case = Case::new("root-user-service", shell);
    case.state("user", "root");
    let ran = case.run(false, text(&case.path("data-Models"))).await;
    c.check(
        &case,
        "User=root : code 11",
        ran.failure() == Some((&MoveFailure::UnitChanged, false)),
        Some(&ran),
    );

    // Dossier existant ni à l'appelant (uid 4242 ici) ni au service (nobody) :
    // refus (12), rien donné au service ; un dossier neuf reste admis.
    let mut case = Case::new("folder-of-another-user", shell);
    case.set("CHATONLM_TEST_CALLER", "4242");
    case.state("user", "nobody");
    case.state("group", &id("-gn", Some("nobody")));
    let foreign = case.path("var-empty");
    fs::create_dir_all(&foreign).unwrap();
    let ran = case.run(true, text(&foreign)).await;
    let r = Some(&ran);
    c.check(
        &case,
        "code 12",
        ran.failure() == Some((&MoveFailure::Refused, false)),
        r,
    );
    c.check(
        &case,
        "raison dans le détail",
        matches!(&ran.outcome, MoveOutcome::Failed { detail, .. } if detail.contains("folder of another user")),
        r,
    );
    c.check(
        &case,
        "propriétaire inchangé",
        case.log("chown").is_empty(),
        r,
    );
    c.check(
        &case,
        "service jamais arrêté",
        case.log("systemctl")
            .lines()
            .all(|line| !line.starts_with("stop")),
        r,
    );
    c.check(
        &case,
        "modèles en place",
        case.old.join("blobs/sha256-aaaa").exists(),
        r,
    );
    let new = case.path("new-Models");
    let ran = case.run(false, text(&new)).await;
    c.check(
        &case,
        "dossier neuf admis",
        matches!(ran.outcome, MoveOutcome::Done { .. }),
        Some(&ran),
    );
    // Sans PKEXEC_UID (ni appelant d'essai) : refus.
    let mut case = Case::new("no-caller", shell);
    case.set("CHATONLM_TEST_CALLER", "x");
    let ran = case.run(false, text(&case.path("data-Models"))).await;
    c.check(
        &case,
        "appelant inconnu : code 10",
        ran.failure() == Some((&MoveFailure::Refused, false)),
        Some(&ran),
    );

    // Dossier existant du service (appelant différent) : admis.
    let mut case = Case::new("folder-of-the-service", shell);
    case.set("CHATONLM_TEST_CALLER", "4242");
    let new = case.path("data-Models");
    fs::create_dir_all(&new).unwrap();
    let ran = case.run(true, text(&new)).await;
    c.check(&case, "succès", ran.outcome == DONE_RENAMED, Some(&ran));

    // Unité absente.
    let case = Case::new("unit-missing", shell);
    case.state("load", "not-found");
    let ran = case.run(false, text(&case.path("data-Models"))).await;
    c.check(
        &case,
        "code 11",
        ran.failure() == Some((&MoveFailure::UnitChanged, false)),
        Some(&ran),
    );

    eprintln!(
        "banc sous {} : réussis {}, échecs {}",
        shell.display(),
        c.passed,
        c.failures.len()
    );
    assert!(c.failures.is_empty(), "{}", c.failures.join("\n"));
}

#[tokio::test]
async fn script_bench_under_sh() {
    bench(Path::new(SHELL)).await;
}

#[tokio::test]
async fn script_bench_under_dash() {
    match find_dash() {
        Some(dash) => bench(&dash).await,
        None => eprintln!("dash introuvable : banc sous dash sauté"),
    }
}
