//! Banc réel du pipeline (hors check.sh : `#[ignore]`, nom sans
//! « contract ») : le vrai pipeline, le vrai `agent::Worker`, le vrai
//! `llama-server` et un vrai modèle (rôle `text` de
//! `$CHATONLM_LLAMA_TEST_MODELS/models.json`, voir `RealBench`), sur les deux
//! tâches de P3 du lot 0 (`.tools/proofs/atelier/p3-taches.mjs`, texte lu
//! tel quel ; pièce jointe de (b) : `data/doc-b.md`).
//!
//! Actif seulement si `CHATONLM_WORKSHOP_BENCH=<dossier de sortie>` est posé ;
//! `CHATONLM_WORKSHOP_BENCH_TASKS=a,b` (défaut) choisit les tâches,
//! `CHATONLM_WORKSHOP_BENCH_TIMEOUT_S` borne chaque tâche (défaut 4 h : le
//! projet est alors annulé, état sauvé), `CHATONLM_WORKSHOP_BENCH_FRESH=1`
//! repart de zéro (sinon l'état sauvé d'un passage précédent est repris).
//! Le verrou des vrais serveurs se prend dehors (`flock`).
//!
//! Sorties par tâche, sous `<sortie>/<tâche>/` : livrable et
//! `.atelier/rapport.md` (écrits par le pipeline), `evenements.jsonl`,
//! `etat.json` (état reprenable), `appels/NNN-<rôle>.json` (chaque requête
//! au modèle et sa réponse), `mesures.json`. Autonomie : Autonome. Un échec du
//! pipeline arrête le banc (tâches suivantes non lancées).

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::agent::{AgentError, Call, Json, Text, ThinkMode, Worker, WorkerOptions, Write};
use crate::inference::{Content, Message, Usage};
use crate::llm::testing::{RealBench, Role};
use crate::workshop::pipeline::host::{Host, HostFuture};
use crate::workshop::pipeline::model::{Model, OnDelta};
use crate::workshop::pipeline::prompts::{DIRECT_SYSTEM, SECTION_SYSTEM};
use crate::workshop::pipeline::types::{
    Answer, Attachment, Autonomy, EventKind, ProjectInfo, Question, StateKey,
};
use crate::workshop::pipeline::{RunEnd, run};

/// Contexte demandé au `Worker` (celui du choix du modèle de l'Atelier).
const CTX: u32 = 32_768;

fn now_ms() -> i64 {
    crate::store::now_ms()
}

/// Racine du projet (au-dessus de `src-tauri`).
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Texte exact de la tâche `key` dans `p3-taches.mjs` (chaîne JSON après
/// `texte: `).
fn task_text(source: &str, key: &str) -> String {
    let block = source
        .find(&format!("\n  {key}: {{"))
        .unwrap_or_else(|| panic!("tâche {key} absente"));
    let rest = &source[block..];
    let at = rest.find("texte: ").expect("texte") + "texte: ".len();
    serde_json::Deserializer::from_str(&rest[at..])
        .into_iter::<String>()
        .next()
        .expect("chaîne")
        .expect("chaîne JSON")
}

/// Écrit `value` en JSON lisible, d'un coup (fichier temporaire puis
/// renommage).
fn write_json(path: &Path, value: &Value) {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}

/// Ce que suivent ensemble le socle et le modèle du banc.
#[derive(Default)]
struct Tracker {
    phase: Option<String>,
    activity: Option<Value>,
    /// Phase → (début en cours, secondes cumulées).
    phases: BTreeMap<String, (Option<Instant>, f64)>,
    /// (phase, début s, fin s) dans l'ordre.
    timeline: Vec<(String, f64, Option<f64>)>,
    warnings: Vec<String>,
    errors: Vec<String>,
    questions: Vec<Value>,
    strategy: Option<Value>,
    plan_bricks: Option<usize>,
    calls: Vec<Value>,
}

/// Socle sur disque.
struct DiskHost {
    info: ProjectInfo,
    dir: PathBuf,
    start: Instant,
    state: Mutex<BTreeMap<String, Value>>,
    seq: Mutex<u64>,
    events: Mutex<std::fs::File>,
    tracker: Mutex<Tracker>,
    cancel: CancellationToken,
}

impl DiskHost {
    fn new(info: ProjectInfo, fresh: bool) -> Self {
        let dir = info.dir.clone();
        std::fs::create_dir_all(dir.join("appels")).unwrap();
        let state_file = dir.join("etat.json");
        let state: BTreeMap<String, Value> = if !fresh && state_file.is_file() {
            serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap()
        } else {
            BTreeMap::new()
        };
        if !state.is_empty() {
            eprintln!(
                "[banc] reprise : {} états sauvés relus de {}",
                state.len(),
                state_file.display()
            );
        }
        let events = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("evenements.jsonl"))
            .unwrap();
        DiskHost {
            info,
            dir,
            start: Instant::now(),
            state: Mutex::new(state),
            seq: Mutex::new(0),
            events: Mutex::new(events),
            tracker: Mutex::new(Tracker::default()),
            cancel: CancellationToken::new(),
        }
    }

    fn elapsed(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn persist(&self, state: &BTreeMap<String, Value>) {
        write_json(
            &self.dir.join("etat.json"),
            &serde_json::to_value(state).unwrap(),
        );
    }

    fn track(&self, kind: &EventKind) {
        let now = Instant::now();
        let t = self.elapsed();
        let mut tracker = self.tracker.lock().unwrap();
        let name = |phase: &crate::workshop::pipeline::types::Phase| {
            serde_json::to_value(phase)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default()
        };
        match kind {
            EventKind::PhaseStarted { phase } => {
                let phase = name(phase);
                tracker.phase = Some(phase.clone());
                tracker.phases.entry(phase.clone()).or_default().0 = Some(now);
                tracker.timeline.push((phase, t, None));
            }
            EventKind::PhaseDone { phase } => {
                let phase = name(phase);
                let entry = tracker.phases.entry(phase.clone()).or_default();
                if let Some(started) = entry.0.take() {
                    entry.1 += now.duration_since(started).as_secs_f64();
                }
                if let Some(open) = tracker
                    .timeline
                    .iter_mut()
                    .rev()
                    .find(|(p, _, end)| *p == phase && end.is_none())
                {
                    open.2 = Some(t);
                }
            }
            EventKind::Activity { activity } => {
                tracker.activity = serde_json::to_value(activity).ok();
            }
            EventKind::Warning { code } => tracker.warnings.push(code.clone()),
            EventKind::Error { code } => tracker.errors.push(code.clone()),
            EventKind::QuestionAsked { question } => {
                tracker
                    .questions
                    .push(serde_json::to_value(question).unwrap_or_default());
            }
            EventKind::StrategyChosen {
                strategy,
                unanimous,
            } => {
                tracker.strategy = Some(json!({"strategy": strategy, "unanimous": unanimous}));
            }
            EventKind::PlanReady { plan } => tracker.plan_bricks = Some(plan.bricks.len()),
            _ => {}
        }
    }

    /// Phases restées ouvertes (arrêt) : closes maintenant.
    fn close_phases(&self) {
        let now = Instant::now();
        let mut tracker = self.tracker.lock().unwrap();
        for entry in tracker.phases.values_mut() {
            if let Some(started) = entry.0.take() {
                entry.1 += now.duration_since(started).as_secs_f64();
            }
        }
    }

    fn get(&self, key: &str) -> Option<Value> {
        self.state.lock().unwrap().get(key).cloned()
    }
}

/// Chemin relatif sûr (pas d'absolu, pas de « .. »).
fn safe_rel(rel: &str) -> bool {
    let path = Path::new(rel);
    !rel.is_empty()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

impl Host for DiskHost {
    fn project(&self) -> &ProjectInfo {
        &self.info
    }

    fn emit(&self, kind: EventKind) -> HostFuture<'_, u64> {
        Box::pin(async move {
            self.track(&kind);
            let seq = {
                let mut seq = self.seq.lock().unwrap();
                *seq += 1;
                *seq
            };
            let line = json!({
                "seq": seq,
                "atMs": now_ms(),
                "t": (self.elapsed() * 10.0).round() / 10.0,
                "kind": kind,
            });
            let mut file = self.events.lock().unwrap();
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
            eprintln!("[banc] {:>7.1} s {}", self.elapsed(), line["kind"]);
            seq
        })
    }

    fn save(&self, key: StateKey, value: Value) -> HostFuture<'_, ()> {
        Box::pin(async move {
            let mut state = self.state.lock().unwrap();
            state.insert(key.name(), value);
            self.persist(&state);
        })
    }

    fn load(&self, key: StateKey) -> HostFuture<'_, Option<Value>> {
        Box::pin(async move { self.state.lock().unwrap().get(&key.name()).cloned() })
    }

    /// Autonome : ne devrait jamais être appelé ; noté (mesures) et répondu
    /// d'office comme le ferait un utilisateur qui accepte tout.
    fn ask(&self, question: Question) -> HostFuture<'_, Answer> {
        Box::pin(async move {
            eprintln!("[banc] QUESTION posée en Autonome : {question:?}");
            match question {
                Question::Strategy(options) => Answer::Strategy(options[0]),
                Question::Framing(questions) => {
                    Answer::Framing(questions.iter().map(|_| String::new()).collect())
                }
                Question::ApprovePlan(_) => Answer::Plan {
                    approved: true,
                    edited: None,
                    note: None,
                },
                Question::ApproveAction { .. } => Answer::Action(true),
            }
        })
    }

    fn yield_to_user(&self) -> HostFuture<'_, ()> {
        Box::pin(async {})
    }

    fn write_file<'a>(
        &'a self,
        rel: &'a str,
        bytes: &'a [u8],
    ) -> HostFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if !safe_rel(rel) {
                return Err("unsafePath".into());
            }
            let path = self.dir.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|_| "writeFailed".to_owned())?;
            }
            std::fs::write(&path, bytes).map_err(|_| "writeFailed".to_owned())?;
            eprintln!("[banc] fichier écrit : {}", path.display());
            Ok(())
        })
    }

    fn cancelled(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

fn text_of(message: &Message) -> String {
    match &message.content {
        Content::Text(text) => text.clone(),
        Content::Parts(_) => String::new(),
    }
}

fn think_json(think: ThinkMode) -> Value {
    match think {
        ThinkMode::Off => json!("off"),
        ThinkMode::On { budget } => json!({ "budget": budget }),
    }
}

fn usage_json(usage: &Usage) -> Value {
    json!({"prompt": usage.prompt, "completion": usage.completion, "cached": usage.cached})
}

/// Le vrai `Worker`, mesuré : chaque requête et sa réponse dans
/// `appels/`, et un résumé par appel dans les mesures.
struct Measured<'a> {
    worker: &'a Worker,
    host: &'a DiskHost,
    n: Mutex<usize>,
}

impl Measured<'_> {
    fn record(&self, role: &str, request: Value, started: Instant, outcome: Value) {
        let n = {
            let mut n = self.n.lock().unwrap();
            *n += 1;
            *n
        };
        let seconds = started.elapsed().as_secs_f64();
        let (phase, activity) = {
            let tracker = self.host.tracker.lock().unwrap();
            (tracker.phase.clone(), tracker.activity.clone())
        };
        let t_end = self.host.elapsed();
        let file = format!("{n:03}-{role}.json");
        write_json(
            &self.host.dir.join("appels").join(&file),
            &json!({
                "n": n, "role": role, "phase": phase, "activity": activity,
                "seconds": seconds, "endS": t_end,
                "request": request, "outcome": outcome,
            }),
        );
        let summary = json!({
            "n": n, "role": role, "phase": phase, "activity": activity,
            "seconds": (seconds * 10.0).round() / 10.0,
            "usage": outcome.get("usage").cloned().unwrap_or(Value::Null),
            "reasoningChars": outcome.get("reasoningChars").cloned().unwrap_or(Value::Null),
            "finish": outcome.get("finish").cloned().unwrap_or(Value::Null),
            "error": outcome.get("error").cloned().unwrap_or(Value::Null),
        });
        eprintln!("[banc] appel {summary}");
        self.host.tracker.lock().unwrap().calls.push(summary);
    }
}

fn error_json(error: &AgentError) -> Value {
    let partial = match error {
        AgentError::Incomplete { partial } => Some(partial.clone()),
        _ => None,
    };
    json!({"code": error.code(), "debug": format!("{error:?}").chars().take(2_000).collect::<String>(),
           "partial": partial})
}

impl Model for Measured<'_> {
    fn ask_json<'a>(&'a self, call: &'a Call) -> HostFuture<'a, Result<Json, AgentError>> {
        Box::pin(async move {
            let role = call.schema["title"].as_str().unwrap_or("json").to_owned();
            let request = json!({
                "kind": "json", "system": call.system, "user": call.user, "schema": call.schema,
                "temperature": call.temperature, "think": think_json(call.think),
                "maxTokens": call.max_tokens,
            });
            let started = Instant::now();
            let result = Worker::ask_json(self.worker, call).await;
            let outcome = match &result {
                Ok(json) => json!({
                    "value": json.value, "usage": usage_json(&json.usage),
                    "reasoningChars": json.reasoning_chars,
                    "elapsedS": json.elapsed.as_secs_f64(),
                }),
                Err(error) => json!({ "error": error_json(error) }),
            };
            self.record(&role, request, started, outcome);
            result
        })
    }

    fn write<'a>(
        &'a self,
        messages: Vec<Message>,
        options: &'a Write,
        on_delta: OnDelta<'a>,
    ) -> HostFuture<'a, Result<Text, AgentError>> {
        Box::pin(async move {
            let system = messages.first().map(text_of).unwrap_or_default();
            let user = messages.get(1).map(text_of).unwrap_or_default();
            let role = if system == DIRECT_SYSTEM {
                "direct"
            } else if system == SECTION_SYSTEM && user.contains("YOUR PREVIOUS VERSION") {
                "fix"
            } else if system == SECTION_SYSTEM {
                "section"
            } else {
                "write"
            };
            let request = json!({
                "kind": "write",
                "messages": messages.iter().map(text_of).collect::<Vec<_>>(),
                "temperature": options.temperature, "think": think_json(options.think),
                "maxTokens": options.max_tokens,
            });
            let started = Instant::now();
            let result = Worker::write(self.worker, messages, options, on_delta).await;
            let outcome = match &result {
                Ok(text) => json!({
                    "content": text.content, "usage": usage_json(&text.usage),
                    "reasoningChars": text.reasoning_chars,
                    "finish": format!("{:?}", text.finish),
                    "words": text.content.split_whitespace().count(),
                }),
                Err(error) => json!({ "error": error_json(error) }),
            };
            self.record(role, request, started, outcome);
            result
        })
    }

    fn count_tokens<'a>(&'a self, text: &'a str) -> HostFuture<'a, Result<usize, AgentError>> {
        Box::pin(Worker::count_tokens(self.worker, text))
    }

    fn context(&self) -> u32 {
        Worker::context(self.worker)
    }

    fn model(&self) -> &str {
        Worker::model(self.worker)
    }
}

/// Une tâche du banc.
struct Task {
    key: &'static str,
    title: &'static str,
    target: u32,
    attachment: Option<&'static str>,
}

const TASKS: [Task; 2] = [
    Task {
        key: "a",
        title: "Potager en carrés",
        target: 3_500,
        attachment: None,
    },
    Task {
        key: "b",
        title: "Analyse de la revue",
        target: 1_500,
        attachment: Some("doc-b.md"),
    },
];

/// Résumé des vérifications.
fn checks_summary(checks: &[Value]) -> Value {
    let failed: Vec<Value> = checks
        .iter()
        .filter(|c| c["passed"] == false)
        .map(|c| {
            json!({
                "criterion": c["criterion"], "method": c["method"], "blocking": c["blocking"],
                "evidence": c["evidence"].as_str().unwrap_or_default().chars().take(400).collect::<String>(),
                "where": c["where"],
            })
        })
        .collect();
    json!({
        "total": checks.len(),
        "passed": checks.iter().filter(|c| c["passed"] == true).count(),
        "failedBlocking": checks.iter().filter(|c| c["passed"] == false && c["blocking"] == true).count(),
        "failedNonBlocking": checks.iter().filter(|c| c["passed"] == false && c["blocking"] != true).count(),
        "failed": failed,
    })
}

/// Mesures d'une tâche.
fn measures(host: &DiskHost, task: &Task, worker: &Worker, end: &RunEnd, seconds: f64) -> Value {
    host.close_phases();
    let tracker = host.tracker.lock().unwrap();
    let mut tokens = (0u64, 0u64, 0u64);
    let mut by_role: BTreeMap<String, (usize, f64, u64, u64)> = BTreeMap::new();
    for call in &tracker.calls {
        let usage = &call["usage"];
        let p = usage["prompt"].as_u64().unwrap_or(0);
        let c = usage["completion"].as_u64().unwrap_or(0);
        tokens.0 += p;
        tokens.1 += c;
        tokens.2 += usage["cached"].as_u64().unwrap_or(0);
        let role = by_role
            .entry(call["role"].as_str().unwrap_or("?").to_owned())
            .or_default();
        role.0 += 1;
        role.1 += call["seconds"].as_f64().unwrap_or(0.0);
        role.2 += p;
        role.3 += c;
    }
    let report = host.get("report");
    let delivered = host
        .get("delivered")
        .and_then(|v| v.as_str().map(str::to_owned));
    let words = delivered
        .as_ref()
        .and_then(|file| std::fs::read_to_string(host.dir.join(file)).ok())
        .map(|doc| doc.split_whitespace().count());
    let plan_bricks = host
        .get("plan")
        .and_then(|plan| plan["bricks"].as_array().map(Vec::len))
        .or(tracker.plan_bricks);
    let whole_checks: Vec<Value> = report
        .as_ref()
        .and_then(|r| r["checks"].as_array().cloned())
        .unwrap_or_default();
    let bricks: Vec<Value> = report
        .as_ref()
        .and_then(|r| r["bricks"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|b| {
            json!({
                "id": b["id"], "title": b["title"], "state": b["state"], "fixed": b["fixed"],
                "words": b["words"],
                "checks": checks_summary(b["checks"].as_array().map_or(&[][..], Vec::as_slice)),
            })
        })
        .collect();
    let route = report
        .as_ref()
        .map(|r| r["route"].clone())
        .unwrap_or(Value::Null);
    json!({
        "task": task.key,
        "model": worker.model(),
        "context": worker.context(),
        "autonomy": "autonomous",
        "end": format!("{end:?}"),
        "seconds": (seconds * 10.0).round() / 10.0,
        "route": route,
        "strategy": tracker.strategy,
        "deliverable": delivered.map(|f| host.dir.join(f)),
        "report": host.dir.join(".atelier/rapport.md"),
        "words": words,
        "reportWords": report.as_ref().map(|r| r["words"].clone()),
        "targetWords": task.target,
        "planBricks": plan_bricks,
        "bricks": bricks,
        "judgedOnOutline": report.as_ref().map(|r| r["judgedOnOutline"].clone()),
        "notChecked": report.as_ref().map(|r| r["notChecked"].clone()),
        "wholeChecks": checks_summary(&whole_checks),
        "phases": tracker.phases.iter().map(|(p, (_, s))| (p.clone(), json!((s * 10.0).round() / 10.0))).collect::<serde_json::Map<_, _>>(),
        "timeline": tracker.timeline.iter().map(|(p, a, b)| json!({"phase": p, "startS": a, "endS": b})).collect::<Vec<_>>(),
        "calls": tracker.calls.len(),
        "byRole": by_role.iter().map(|(r, (n, s, p, c))| (r.clone(), json!({"n": n, "seconds": (s * 10.0).round() / 10.0, "prompt": p, "completion": c}))).collect::<serde_json::Map<_, _>>(),
        "tokens": {"prompt": tokens.0, "completion": tokens.1, "cached": tokens.2},
        "warnings": tracker.warnings,
        "errors": tracker.errors,
        "questionsAsked": tracker.questions,
    })
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "banc réel : CHATONLM_WORKSHOP_BENCH, vrai llama-server et vrai modèle, sous verrou"]
async fn real_bench() {
    let Some(out) = std::env::var_os("CHATONLM_WORKSHOP_BENCH").map(PathBuf::from) else {
        eprintln!("real_bench: CHATONLM_WORKSHOP_BENCH absent, banc sauté");
        return;
    };
    let wanted: Vec<String> = std::env::var("CHATONLM_WORKSHOP_BENCH_TASKS")
        .unwrap_or_else(|_| "a,b".into())
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    let limit = Duration::from_secs(
        std::env::var("CHATONLM_WORKSHOP_BENCH_TIMEOUT_S")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(4 * 3600),
    );
    let fresh = std::env::var("CHATONLM_WORKSHOP_BENCH_FRESH").as_deref() == Ok("1");
    let proofs = root().join(".tools/proofs/atelier");
    let source = std::fs::read_to_string(proofs.join("p3-taches.mjs")).expect("p3-taches.mjs");
    // Textes lus avant de charger le modèle (une erreur ici ne coûte rien).
    for task in TASKS.iter().filter(|t| wanted.iter().any(|w| w == t.key)) {
        eprintln!(
            "[banc] tâche {} : {}",
            task.key,
            task_text(&source, task.key)
        );
    }

    let bench = RealBench::new(Role::Text).expect("vrai llama-server et modèle (RealBench)");
    eprintln!("[banc] modèle {}", bench.id);
    let opened = Instant::now();
    let worker = Worker::open(
        &bench.access,
        &bench.id,
        WorkerOptions {
            ctx_min: CTX,
            cancel: CancellationToken::new(),
        },
    )
    .await
    .expect("Worker::open");
    eprintln!(
        "[banc] Worker ouvert en {:.1} s : contexte {}, réflexion {}",
        opened.elapsed().as_secs_f64(),
        worker.context(),
        worker.thinks()
    );

    for task in TASKS.iter().filter(|t| wanted.iter().any(|w| w == t.key)) {
        let dir = out.join(task.key);
        if fresh && dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        let attachments = task
            .attachment
            .map(|name| {
                vec![Attachment {
                    name: name.into(),
                    text: std::fs::read_to_string(proofs.join("data").join(name)).unwrap(),
                }]
            })
            .unwrap_or_default();
        let info = ProjectInfo {
            id: i64::from(task.key.as_bytes()[0]),
            title: task.title.into(),
            task: task_text(&source, task.key),
            attachments,
            strategy: None,
            autonomy: Autonomy::Autonomous,
            app_language: "fr".into(),
            dir: std::fs::canonicalize(&dir).unwrap(),
        };
        eprintln!("[banc] tâche {} : {}", task.key, info.task);
        let host = DiskHost::new(info, fresh);
        let model = Measured {
            worker: &worker,
            host: &host,
            n: Mutex::new(0),
        };
        let started = Instant::now();
        let token = host.cancel.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(limit).await;
            eprintln!("[banc] délai dépassé : projet annulé (état sauvé)");
            token.cancel();
        });
        let end = run(&host, &model).await;
        timer.abort();
        let seconds = started.elapsed().as_secs_f64();
        let measured = measures(&host, task, &worker, &end, seconds);
        write_json(&dir.join("mesures.json"), &measured);
        eprintln!(
            "[banc] tâche {} finie : {end:?} en {seconds:.0} s ; mesures {}",
            task.key,
            dir.join("mesures.json").display()
        );
        if !matches!(end, RunEnd::Delivered { .. }) {
            eprintln!("[banc] arrêt du banc (fin autre que livrée)");
            break;
        }
    }
    worker.release();
    bench.shutdown().await;
}
