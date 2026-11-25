//! Faux modèle scénarisé et faux socle en mémoire, pour les essais du
//! pipeline. Le faux modèle reconnaît chaque requête par son rôle (`title`
//! du schéma JSON ; pour un texte : version directe, section, correction),
//! répond par défaut comme un bon modèle, et chaque essai remplace ce qu'il
//! veut. Le faux socle garde l'état, les événements, les questions et les
//! fichiers ; une trace commune aux deux ordonne cessions et requêtes.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::agent::{AgentError, Call, Delta, Json, Text, Write};
use crate::inference::{Content, FinishReason, Message, Usage};
use crate::workshop::pipeline::host::{Host, HostFuture};
use crate::workshop::pipeline::model::{Model, OnDelta};
use crate::workshop::pipeline::prompts::{DIRECT_SYSTEM, SECTION_SYSTEM};
use crate::workshop::pipeline::types::{
    Answer, Autonomy, EventKind, ProjectInfo, Question, StateKey, Strategy,
};

/// Trace commune : « yield », « call:<rôle> ».
pub type Trace = Arc<Mutex<Vec<String>>>;

/// Une requête vue par le faux modèle.
#[derive(Clone, Debug)]
pub struct Req {
    /// `strategy`, `spec`, `plan`, `planJudge`, `formJudge`, `factJudge`,
    /// `refusal`, `split`, `card`, `contradictions`, `surgery`, `direct`,
    /// `directFix`, `section`, `fix`, `proofread` (son `user` : le passage,
    /// sans balises).
    pub role: String,
    pub user: String,
    pub temperature: f32,
    /// Rang de cette requête parmi celles du même rôle (0 : la première).
    pub n: usize,
}

impl Req {
    /// Section concernée : titre de « YOUR SECTION », sinon premier titre
    /// « ## » du texte examiné.
    pub fn section(&self) -> Option<String> {
        if let Some(at) = self.user.find("YOUR SECTION: « ") {
            let rest = &self.user[at + "YOUR SECTION: « ".len()..];
            return rest.split(" »").next().map(str::to_owned);
        }
        let text = self.user.split("<text>\n").nth(1)?;
        text.lines()
            .find_map(|l| l.strip_prefix("## "))
            .map(|t| t.trim().to_owned())
    }

    pub fn about(&self, title: &str) -> bool {
        self.section().as_deref() == Some(title)
    }
}

/// Réponse scénarisée.
pub enum Reply {
    Json(Value),
    Text(String),
    Err(AgentError),
}

type Script = dyn Fn(&Req) -> Option<Reply> + Send + Sync;

/// Phrase de remplissage : 6 mots, aucun mot interne.
const FILLER: &str = "Le jardin pousse bien au soleil. ";

/// `n` mots de remplissage.
pub fn filler(n: u32) -> String {
    let sentences = n.div_ceil(6);
    FILLER.repeat(sentences as usize).trim().to_owned()
}

/// Mots demandés à une section (« (about N words) » du rappel).
fn asked_words(user: &str) -> u32 {
    user.rfind("(about ")
        .and_then(|at| {
            user[at + 7..]
                .split(' ')
                .next()
                .and_then(|n| n.replace(',', "").parse().ok())
        })
        .unwrap_or(300)
}

/// Nombre de questions numérotées après « QUESTIONS ».
pub fn questions(user: &str) -> usize {
    user.split("QUESTIONS\n")
        .nth(1)
        .map(|rest| {
            rest.lines()
                .take_while(|l| !l.trim().is_empty())
                .filter(|l| l.chars().next().is_some_and(|c| c.is_ascii_digit()))
                .count()
        })
        .unwrap_or(0)
}

/// Le bon plan par défaut : introduction, deux sections, conclusion (1 000
/// mots, critères C1 à C4 couverts).
pub fn good_plan() -> Value {
    json!({
        "analysis": "a",
        "title": "Guide du balcon",
        "parts": [
            {"id": 1, "title": "Introduction", "goal": "Présenter le guide", "kind": "intro",
             "words": 100, "depends_on": [], "criteria": [], "forbidden": []},
            {"id": 2, "title": "Les cultures", "goal": "Choisir les plantes", "kind": "text",
             "words": 400, "depends_on": [], "criteria": ["C1", "C2"], "forbidden": []},
            {"id": 3, "title": "Le calendrier", "goal": "Dire quand semer et récolter", "kind": "text",
             "words": 400, "depends_on": [2], "criteria": ["C3", "C4"], "forbidden": []},
            {"id": 4, "title": "Conclusion", "goal": "Conclure", "kind": "conclusion",
             "words": 100, "depends_on": [], "criteria": [], "forbidden": []},
        ]
    })
}

/// Évaluations du juge de forme : toutes « oui » sauf `no` (numéros).
pub fn form(n: usize, no: &[usize]) -> Value {
    let evaluations: Vec<Value> = (1..=n)
        .map(|i| {
            let failed = no.contains(&i);
            json!({"question": i, "evidence": format!("preuve {i}"),
                   "verdict": if failed { "no" } else { "yes" },
                   "where": if failed { "là" } else { "" }})
        })
        .collect();
    json!({ "evaluations": evaluations })
}

/// Un verdict d'exactitude.
pub fn facts(verdict: &str) -> Value {
    json!({"claims": [{"quote": "200 g de fumier par m²", "reasoning": "r",
                       "verdict": verdict, "correction": "3 kg par m²"}]})
}

/// Faux modèle.
pub struct FakeModel {
    script: Box<Script>,
    pub log: Mutex<Vec<Req>>,
    counts: Mutex<HashMap<String, usize>>,
    trace: Trace,
    /// À la k-ième requête (0 : la première) : jeton annulé, requête en
    /// échec (interruption).
    interrupt: Option<(usize, CancellationToken)>,
    calls: AtomicUsize,
    /// Longueur de la spécification par défaut.
    pub length: u32,
    pub context: u32,
}

impl FakeModel {
    pub fn new(
        trace: &Trace,
        script: impl Fn(&Req) -> Option<Reply> + Send + Sync + 'static,
    ) -> Self {
        FakeModel {
            script: Box::new(script),
            log: Mutex::new(Vec::new()),
            counts: Mutex::new(HashMap::new()),
            trace: trace.clone(),
            interrupt: None,
            calls: AtomicUsize::new(0),
            length: 1_000,
            context: 32_768,
        }
    }

    pub fn interrupted_at(mut self, k: usize, token: CancellationToken) -> Self {
        self.interrupt = Some((k, token));
        self
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Rôles appelés, dans l'ordre.
    pub fn roles(&self) -> Vec<String> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.role.clone())
            .collect()
    }

    /// Requêtes d'un rôle.
    pub fn of(&self, role: &str) -> Vec<Req> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.role == role)
            .cloned()
            .collect()
    }

    fn request(
        &self,
        role: String,
        system: &str,
        user: &str,
        temperature: f32,
    ) -> Result<Reply, AgentError> {
        let k = self.calls.fetch_add(1, Ordering::SeqCst);
        self.trace.lock().unwrap().push(format!("call:{role}"));
        if let Some((at, token)) = &self.interrupt
            && k == *at
        {
            token.cancel();
            return Err(AgentError::Cancelled);
        }
        let n = {
            let mut counts = self.counts.lock().unwrap();
            let count = counts.entry(role.clone()).or_insert(0);
            *count += 1;
            *count - 1
        };
        let _ = system;
        let req = Req {
            role,
            user: user.to_owned(),
            temperature,
            n,
        };
        self.log.lock().unwrap().push(req.clone());
        Ok((self.script)(&req).unwrap_or_else(|| self.default(&req)))
    }

    fn default(&self, req: &Req) -> Reply {
        match req.role.as_str() {
            "strategy" => Reply::Json(json!({
                "analysis": "a", "strategy": "long_form", "deliverables": ["guide"],
                "framing_questions": ["Quelle exposition ?", "Quelle surface ?"]})),
            "spec" => Reply::Json(json!({
                "analysis": "a",
                "goal": "Un guide du potager sur un balcon.",
                "deliverables": ["Guide du balcon"],
                "criteria": [
                    "QUAND on lit, LE LIVRABLE DOIT lister les cultures",
                    "QUAND on lit, LE LIVRABLE DOIT dire l'exposition",
                    "QUAND on lit, LE LIVRABLE DOIT donner un calendrier",
                    "QUAND on lit, LE LIVRABLE DOIT donner les récoltes"],
                "audience": "débutants",
                "language": "fr",
                "register": "informal",
                "units": "métrique : g, kg, m, L",
                "length_words": self.length,
                "glossary": [{"term": "paillage", "definition": "couverture du sol"}],
                "assumptions": [],
                "constraints": []})),
            "plan" => Reply::Json(good_plan()),
            "planJudge" => Reply::Json(json!({
                "evaluations": (1..=8).map(|i| json!({"question": i, "evidence": "e", "verdict": "yes"})).collect::<Vec<_>>(),
                "defects": [], "score": 8})),
            "formJudge" => Reply::Json(form(questions(&req.user), &[])),
            "factJudge" => Reply::Json(facts("right")),
            "refusal" => Reply::Json(json!({"reasoning": "r", "refusal": true})),
            "split" => Reply::Json(json!({"analysis": "a", "parts": [
                {"title": "Été", "goal": "Les mois chauds", "words": 200},
                {"title": "Hiver", "goal": "Les mois froids", "words": 200}]})),
            "card" => Reply::Json(
                json!({"summary": format!("Résumé de {}", req.section().unwrap_or_default()),
                "terms": [], "decisions": [], "lists": [], "exposes": []}),
            ),
            "contradictions" => Reply::Json(json!({"verdicts": []})),
            // Chaque phrase corrigée par une phrase juste.
            "surgery" => {
                let n = req.user.matches("SENTENCE: «").count();
                let edits: Vec<Value> = (1..=n)
                    .map(|i| json!({"n": i, "action": "fix", "sentence": "Une phrase juste."}))
                    .collect();
                Reply::Json(json!({ "edits": edits }))
            }
            // Le passage rendu tel quel.
            "proofread" => Reply::Text(req.user.clone()),
            "direct" | "directFix" => {
                Reply::Text(format!("# Guide du balcon\n\n{}", filler(self.length)))
            }
            "section" | "fix" => Reply::Text(format!(
                "## {}\n\n{}",
                req.section().unwrap_or_default(),
                filler(asked_words(&req.user))
            )),
            other => panic!("rôle inconnu : {other}"),
        }
    }
}

fn text_of(message: &Message) -> String {
    match &message.content {
        Content::Text(text) => text.clone(),
        Content::Parts(_) => String::new(),
    }
}

impl Model for FakeModel {
    fn ask_json<'a>(&'a self, call: &'a Call) -> HostFuture<'a, Result<Json, AgentError>> {
        Box::pin(async move {
            let role = call.schema["title"].as_str().unwrap_or("?").to_owned();
            match self.request(role, &call.system, &call.user, call.temperature)? {
                Reply::Json(value) => Ok(Json {
                    value,
                    usage: Usage::default(),
                    reasoning_chars: 0,
                    elapsed: Duration::ZERO,
                }),
                Reply::Text(_) => Err(AgentError::Unreadable),
                Reply::Err(error) => Err(error),
            }
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
            let mut user = messages.get(1).map(text_of).unwrap_or_default();
            let proofreading = system.starts_with("You are a proofreading tool");
            if proofreading {
                // Le passage : dernier message, sans ses balises.
                let last = messages.last().map(text_of).unwrap_or_default();
                user = last
                    .strip_prefix("<text>\n")
                    .and_then(|t| t.strip_suffix("\n</text>"))
                    .unwrap_or(&last)
                    .to_owned();
            }
            let role = if proofreading {
                "proofread"
            } else if system == DIRECT_SYSTEM && user.contains("YOUR PREVIOUS VERSION") {
                "directFix"
            } else if system == DIRECT_SYSTEM {
                "direct"
            } else if system == SECTION_SYSTEM && user.contains("YOUR PREVIOUS VERSION") {
                "fix"
            } else if system == SECTION_SYSTEM {
                "section"
            } else {
                "?"
            };
            match self.request(role.into(), &system, &user, options.temperature)? {
                Reply::Text(content) => {
                    on_delta(Delta::Text(content.clone()));
                    Ok(Text {
                        content,
                        reasoning_chars: 0,
                        usage: Usage::default(),
                        finish: FinishReason::Stop,
                    })
                }
                Reply::Json(_) => Err(AgentError::Unreadable),
                Reply::Err(error) => Err(error),
            }
        })
    }

    fn count_tokens<'a>(&'a self, text: &'a str) -> HostFuture<'a, Result<usize, AgentError>> {
        Box::pin(async move { Ok(text.chars().count().div_ceil(4)) })
    }

    fn context(&self) -> u32 {
        self.context
    }

    fn model(&self) -> &str {
        "fake/model-GGUF:Q4_K_M"
    }
}

/// Faux socle en mémoire.
pub struct FakeHost {
    pub info: ProjectInfo,
    pub state: Mutex<BTreeMap<String, Value>>,
    pub events: Mutex<Vec<EventKind>>,
    pub asked: Mutex<Vec<Question>>,
    pub answers: Mutex<VecDeque<Answer>>,
    pub files: Mutex<BTreeMap<String, String>>,
    /// Écritures refusées avec ce code (disque plein, droits…).
    pub refuse_writes: Mutex<Option<String>>,
    pub trace: Trace,
    pub yields: AtomicUsize,
    cancel: Mutex<CancellationToken>,
    /// Faux : l'utilisateur occupe l'app ; `yield_to_user` attend `true`.
    pub free: watch::Sender<bool>,
}

impl FakeHost {
    pub fn new(autonomy: Autonomy) -> Self {
        FakeHost {
            info: ProjectInfo {
                id: 1,
                title: "Potager".into(),
                task: "Écris un guide d'environ 1 000 mots pour cultiver un potager sur un balcon."
                    .into(),
                attachments: Vec::new(),
                strategy: None,
                autonomy,
                app_language: "fr".into(),
                dir: std::path::PathBuf::from("/nowhere"),
            },
            state: Mutex::new(BTreeMap::new()),
            events: Mutex::new(Vec::new()),
            asked: Mutex::new(Vec::new()),
            answers: Mutex::new(VecDeque::new()),
            files: Mutex::new(BTreeMap::new()),
            refuse_writes: Mutex::new(None),
            trace: Arc::new(Mutex::new(Vec::new())),
            yields: AtomicUsize::new(0),
            cancel: Mutex::new(CancellationToken::new()),
            free: watch::Sender::new(true),
        }
    }

    pub fn with_strategy(mut self, strategy: Strategy) -> Self {
        self.info.strategy = Some(strategy);
        self
    }

    /// Réponses données dans l'ordre (ensuite : réponses par défaut).
    pub fn answering(self, answers: Vec<Answer>) -> Self {
        *self.answers.lock().unwrap() = answers.into();
        self
    }

    pub fn token(&self) -> CancellationToken {
        self.cancel.lock().unwrap().clone()
    }

    /// Nouveau jeton (reprise après une pause).
    pub fn resume(&self) {
        *self.cancel.lock().unwrap() = CancellationToken::new();
    }

    pub fn events(&self) -> Vec<EventKind> {
        self.events.lock().unwrap().clone()
    }

    pub fn has(&self, key: StateKey) -> bool {
        self.state.lock().unwrap().contains_key(&key.name())
    }

    pub fn get(&self, key: StateKey) -> Option<Value> {
        self.state.lock().unwrap().get(&key.name()).cloned()
    }

    pub fn file(&self, name: &str) -> Option<String> {
        self.files.lock().unwrap().get(name).cloned()
    }

    pub fn questions(&self) -> Vec<Question> {
        self.asked.lock().unwrap().clone()
    }
}

impl Host for FakeHost {
    fn project(&self) -> &ProjectInfo {
        &self.info
    }

    fn emit(&self, kind: EventKind) -> HostFuture<'_, u64> {
        Box::pin(async move {
            let mut events = self.events.lock().unwrap();
            events.push(kind);
            events.len() as u64
        })
    }

    fn save(&self, key: StateKey, value: Value) -> HostFuture<'_, ()> {
        Box::pin(async move {
            self.state.lock().unwrap().insert(key.name(), value);
        })
    }

    fn load(&self, key: StateKey) -> HostFuture<'_, Option<Value>> {
        Box::pin(async move { self.state.lock().unwrap().get(&key.name()).cloned() })
    }

    fn ask(&self, question: Question) -> HostFuture<'_, Answer> {
        Box::pin(async move {
            self.asked.lock().unwrap().push(question.clone());
            if let Some(answer) = self.answers.lock().unwrap().pop_front() {
                return answer;
            }
            match question {
                Question::Strategy(options) => Answer::Strategy(options[0]),
                Question::Framing(questions) => Answer::Framing(
                    questions
                        .iter()
                        .map(|_| "Plein sud, 2 m².".into())
                        .collect(),
                ),
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
        Box::pin(async move {
            self.yields.fetch_add(1, Ordering::SeqCst);
            self.trace.lock().unwrap().push("yield".into());
            let mut free = self.free.subscribe();
            let _ = free.wait_for(|free| *free).await;
        })
    }

    fn write_file<'a>(
        &'a self,
        rel: &'a str,
        bytes: &'a [u8],
    ) -> HostFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if let Some(code) = self.refuse_writes.lock().unwrap().clone() {
                return Err(code);
            }
            self.files
                .lock()
                .unwrap()
                .insert(rel.to_owned(), String::from_utf8_lossy(bytes).into_owned());
            Ok(())
        })
    }

    fn cancelled(&self) -> CancellationToken {
        self.token()
    }
}
