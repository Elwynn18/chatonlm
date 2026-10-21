//! Essais du moteur des cours contre le faux serveur (`fake-ollama`,
//! modèles `chatonlm-study*`) et, en contrat, contre l'Ollama de test.

use std::sync::Arc;
use std::time::Duration;

use fake_ollama::{FakeOllama, Mode};
use parking_lot::Mutex;
use serde_json::Value;
use tauri::ipc::{Channel, InvokeResponseBody};

use super::data::{self as stored};
use super::*;
use crate::attachments::{StageResult, Staging};
use crate::error::AppError;
use crate::store::settings::{self, SettingsPatch};
use crate::store::{Store, study as db};

/// Attente maximale d'un état du cours.
const WAIT: Duration = Duration::from_secs(20);

/// Base dont le serveur réglé est `url`, avec ces modèles du chat et des
/// cours.
fn configured(url: &str, chat: &str, study: &str) -> Store {
    let store = Store::open_in_memory().unwrap();
    set_models(&store, Some(url), chat, study);
    store
}

fn set_models(store: &Store, url: Option<&str>, chat: &str, study: &str) {
    let patch = SettingsPatch {
        server_url: url.map(str::to_owned),
        selected_model: Some(chat.into()),
        study_model: Some(study.into()),
        ..Default::default()
    };
    store
        .blocking(|c| settings::update(c, patch, None).map(drop))
        .unwrap();
}

/// Évènements `study-updated` reçus.
#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<StudyUpdated>>>);

impl Events {
    fn ctx(&self, store: &Store) -> Ctx {
        let events = self.0.clone();
        Ctx::new(store.clone(), move |updated| events.lock().push(updated))
    }

    fn count(&self, course: &str) -> usize {
        self.0
            .lock()
            .iter()
            .filter(|updated| updated.course_id == course && !updated.deleted)
            .count()
    }

    fn deleted(&self, course: &str) -> bool {
        self.0
            .lock()
            .iter()
            .any(|updated| updated.course_id == course && updated.deleted)
    }
}

/// Banc : faux serveur, base réglée, moteur, évènements.
struct Bench {
    fake: FakeOllama,
    store: Store,
    engine: StudyEngine,
    events: Events,
    ctx: Ctx,
}

impl Bench {
    async fn new(study_model: &str) -> Self {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = configured(&fake.url(), "", study_model);
        let events = Events::default();
        let ctx = events.ctx(&store);
        Self {
            fake,
            store,
            engine: StudyEngine::default(),
            events,
            ctx,
        }
    }

    async fn create(&self, topic: &str, minutes: u32) -> String {
        self.engine
            .create(&self.ctx, &Staging::default(), new_course(topic, minutes))
            .await
            .unwrap()
    }

    /// Cours tel qu'en base (sans relancer la génération).
    async fn course(&self, id: &str) -> Course {
        let id = id.to_owned();
        let snapshot = self
            .store
            .call(move |c| db::load(c, &id))
            .await
            .unwrap()
            .unwrap();
        stored::assemble(snapshot)
    }

    /// Attend que le cours remplisse `ready`.
    async fn until(&self, id: &str, ready: impl Fn(&Course) -> bool) -> Course {
        let started = std::time::Instant::now();
        loop {
            let course = self.course(id).await;
            if ready(&course) {
                return course;
            }
            assert!(
                started.elapsed() < WAIT,
                "still waiting: {:#?}",
                (
                    course.status,
                    &course.error,
                    course
                        .levels
                        .iter()
                        .map(|l| (l.state, l.remedy, &l.error))
                        .collect::<Vec<_>>(),
                    course.final_quiz.as_ref().map(|f| f.state)
                )
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Attend que la file soit vide.
    async fn idle(&self) {
        let started = std::time::Instant::now();
        while !self.engine.idle() {
            assert!(started.elapsed() < WAIT, "queue still busy");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn answer(
        &self,
        id: &str,
        question: &Question,
        right: bool,
    ) -> Result<Answered, AppError> {
        let value = if right {
            right_answer(&question.body)
        } else {
            wrong_answer(&question.body)
        };
        self.engine.answer(&self.ctx, id, &question.id, value).await
    }

    /// Requêtes de chat reçues dont les consignes contiennent `marker`.
    fn requests_with(&self, marker: &str) -> Vec<Value> {
        self.fake
            .chat_requests()
            .into_iter()
            .filter(|request| {
                request["messages"][0]["content"]
                    .as_str()
                    .is_some_and(|system| system.contains(marker))
            })
            .collect()
    }
}

fn new_course(topic: &str, minutes: u32) -> NewCourse {
    NewCourse {
        topic: topic.into(),
        level: StudyLevel::Beginner,
        minutes,
        web: false,
        document_ids: Vec::new(),
    }
}

/// Bonne réponse aux questions du faux serveur.
fn right_answer(body: &QuestionBody) -> AnswerValue {
    let rank = |item: &String| {
        ["First", "Second", "Third"]
            .iter()
            .position(|word| item.contains(word))
            .unwrap()
    };
    match body {
        QuestionBody::Choice { options, .. } => AnswerValue::Choice {
            index: options.iter().position(|o| o.contains("Right")).unwrap() as u32,
        },
        QuestionBody::TrueFalse { .. } => AnswerValue::TrueFalse { value: true },
        QuestionBody::Blank { .. } => AnswerValue::Blank {
            text: "Right".into(),
        },
        QuestionBody::Order { items, .. } => {
            let mut order: Vec<u32> = (0..items.len() as u32).collect();
            order.sort_by_key(|index| rank(&items[*index as usize]));
            AnswerValue::Order { order }
        }
        QuestionBody::Match { left, right, .. } => AnswerValue::Match {
            pairs: left
                .iter()
                .map(|word| {
                    let digit = match word.as_str() {
                        "One" => "1",
                        "Two" => "2",
                        _ => "3",
                    };
                    right.iter().position(|r| r == digit).unwrap() as u32
                })
                .collect(),
        },
    }
}

fn wrong_answer(body: &QuestionBody) -> AnswerValue {
    match body {
        QuestionBody::Choice { options, .. } => AnswerValue::Choice {
            index: options.iter().position(|o| o.contains("Wrong")).unwrap() as u32,
        },
        QuestionBody::TrueFalse { .. } => AnswerValue::TrueFalse { value: false },
        QuestionBody::Blank { .. } => AnswerValue::Blank {
            text: "wrong".into(),
        },
        // Jamais montrés dans l'ordre : l'identité est fausse.
        QuestionBody::Order { items, .. } => AnswerValue::Order {
            order: (0..items.len() as u32).collect(),
        },
        QuestionBody::Match { left, .. } => AnswerValue::Match {
            pairs: (0..left.len() as u32).collect(),
        },
    }
}

fn level_ready(index: usize) -> impl Fn(&Course) -> bool {
    move |course| {
        course
            .levels
            .get(index)
            .is_some_and(|level| level.state == GenState::Ready)
    }
}

fn final_ready(course: &Course) -> bool {
    course
        .final_quiz
        .as_ref()
        .is_some_and(|final_quiz| final_quiz.state == GenState::Ready)
}

fn invalid(result: Result<impl std::fmt::Debug, AppError>) {
    assert!(matches!(result, Err(AppError::Invalid(_))), "{result:?}");
}

#[tokio::test]
async fn a_course_goes_from_its_plan_to_the_end() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Photosynthesis", 10).await;
    let course = bench
        .until(&id, |course| {
            course.status == CourseStatus::Planned && level_ready(0)(course)
        })
        .await;
    assert_eq!(course.topic, "Photosynthesis");
    assert_eq!(course.language, crate::i18n::Lang::En);
    assert_eq!(course.model, "chatonlm-study");
    let plan = course.plan.as_ref().unwrap();
    // Titres sans Markdown (le faux serveur écrit `**sujet**`).
    assert_eq!(plan.title, "Photosynthesis");
    assert_eq!(plan.further, ["Further 1", "Further 2", "Further 3"]);
    assert_eq!(plan.levels.len(), 2);
    assert_eq!(plan.kind, StudyKind::Concept);
    // Seul le palier 0 est préparé avant « Start ».
    assert_eq!(course.levels[1].state, GenState::Pending);
    assert_eq!(bench.requests_with("Write level 2").len(), 0);
    let lesson = course.levels[0].lesson.as_ref().unwrap();
    assert_eq!(lesson.sections.len(), 3);
    assert_eq!(lesson.sections[0].heading, "Part one");
    assert_eq!(lesson.sections[1].heading, "Part two");
    // Formule écrite par le modèle avec une seule barre : réparée.
    assert!(
        lesson.sections[0].body.contains("\\(\\frac{1}{2}\\) = 0,5"),
        "{}",
        lesson.sections[0].body
    );
    assert!(!serde_json::to_string(&course).unwrap().contains('\u{c}'));
    assert_eq!(course.levels[0].check.as_ref().unwrap().id, "L0-check");
    let attempt = &course.levels[0].attempts[0];
    let ids: Vec<&str> = attempt.questions.iter().map(|q| q.id.as_str()).collect();
    assert_eq!(ids, ["L0-A0-Q0", "L0-A0-Q1", "L0-A0-Q2"]);
    assert_eq!(attempt.pass_mark, 2);
    // Sorte « concept », palier 0 : choix, choix, vrai/faux, association.
    assert!(matches!(
        course.levels[0].check.as_ref().unwrap().body,
        QuestionBody::Choice { .. }
    ));
    assert!(matches!(
        attempt.questions[2].body,
        QuestionBody::Match { .. }
    ));
    assert!(course.levels[0].estimated_seconds > 20);
    assert_eq!(
        course.levels[1].estimated_seconds,
        stored::UNKNOWN_LEVEL_SECONDS
    );
    assert_eq!(course.levels[0].status, LevelStatus::Current);
    assert_eq!(course.levels[1].status, LevelStatus::Locked);

    // Requêtes : JSON contraint, contexte des cours, raisonnement non
    // demandé (modèle sans `thinking`).
    let plan_request = &bench.requests_with("Design the plan")[0];
    assert_eq!(plan_request["stream"], false);
    assert_eq!(
        plan_request["format"]["properties"]["levels"]["minItems"],
        2
    );
    assert_eq!(
        plan_request["options"],
        serde_json::json!({"temperature": 0.3, "num_ctx": 8192, "num_predict": 4096})
    );
    assert!(plan_request.get("think").is_none());
    assert_eq!(
        plan_request["messages"][1]["content"],
        "Topic: Photosynthesis"
    );
    // Auto-vérification des quatre questions ; l'association, paire par
    // paire dans l'ordre de la solution (le faux serveur ne juge vraies que
    // ses paires : mélangées, elles seraient écartées).
    let checks = bench.requests_with("You are a careful student");
    assert_eq!(checks.len(), 4);
    let pairs = checks.last().unwrap()["messages"][1]["content"]
        .as_str()
        .unwrap();
    assert!(
        pairs.ends_with(
            "\n0. One → 1\n1. Two → 2\n2. Three → 3\nAnswer with one true or false per statement, in order."
        ),
        "{pairs}"
    );
    assert!(
        checks
            .iter()
            .all(|check| check["options"]["temperature"] == 0.0)
    );

    // Réponse à un palier verrouillé, au bilan, ou avant « Start » : refusées.
    invalid(bench.answer(&id, &attempt.questions[0], true).await);
    bench.engine.start(&bench.ctx, &id).await.unwrap();
    invalid(bench.engine.start(&bench.ctx, &id).await);
    let course = bench.until(&id, final_ready).await;
    assert_eq!(course.status, CourseStatus::Active);
    // La leçon du palier 2 connaît l'accroche du palier 1 ; pas celle du 1.
    let first = &bench.requests_with("Write level 1")[0];
    assert!(
        !first["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("hooks of the earlier levels")
    );
    let second = &bench.requests_with("Write level 2")[0];
    assert!(
        second["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("It must be different from the hooks of the earlier levels: \"Level 1?\".")
    );
    let final_quiz = course.final_quiz.clone().unwrap();
    let final_ids: Vec<&str> = final_quiz.questions.iter().map(|q| q.id.as_str()).collect();
    assert_eq!(final_ids, ["F-Q0", "F-Q1", "F-Q2"]);
    invalid(bench.answer(&id, &final_quiz.questions[0], true).await);
    invalid(
        bench
            .answer(&id, &course.levels[1].attempts[0].questions[0], true)
            .await,
    );
    // Inconnue, ou de mauvaise forme.
    invalid(
        bench
            .engine
            .answer(
                &bench.ctx,
                &id,
                "L9-check",
                AnswerValue::TrueFalse { value: true },
            )
            .await,
    );
    invalid(
        bench
            .engine
            .answer(
                &bench.ctx,
                &id,
                "L0-A0-Q2",
                AnswerValue::TrueFalse { value: true },
            )
            .await,
    );

    // Entraînement (permis, non compté) puis palier 0 réussi.
    let check = course.levels[0].check.clone().unwrap();
    let before = bench.events.count(&id);
    let answered = bench.answer(&id, &check, false).await.unwrap();
    assert!(!answered.correct);
    assert_eq!(answered.expected, right_answer(&check.body));
    assert_eq!(
        answered.explanation,
        "This is the right answer because the lesson says so."
    );
    assert!(bench.events.count(&id) > before);
    // La première réponse est définitive : renvoyée telle quelle, sans
    // évènement.
    let after_first = bench.events.count(&id);
    assert_eq!(bench.answer(&id, &check, true).await.unwrap(), answered);
    assert_eq!(bench.events.count(&id), after_first);
    let questions = course.levels[0].attempts[0].questions.clone();
    for (n, question) in questions.iter().enumerate() {
        let answered = bench.answer(&id, question, n != 1).await.unwrap();
        if let QuestionBody::Match { .. } = question.body {
            assert_eq!(answered.detail.len(), 3);
        }
    }
    let course = bench.course(&id).await;
    assert_eq!(course.levels[0].status, LevelStatus::Passed);
    assert_eq!(course.levels[1].status, LevelStatus::Current);
    assert_eq!(course.levels[0].failed_attempts, 0);

    // Palier 1, puis bilan : le cours est fini à la dernière réponse.
    for question in course.levels[1].attempts[0].questions.clone() {
        bench.answer(&id, &question, true).await.unwrap();
    }
    for (n, question) in final_quiz.questions.iter().enumerate() {
        let course = bench.course(&id).await;
        assert_eq!(course.status, CourseStatus::Active);
        assert!(course.finished_at.is_none());
        bench.answer(&id, question, n > 0).await.unwrap();
    }
    let course = bench.course(&id).await;
    assert_eq!(course.status, CourseStatus::Finished);
    assert!(course.finished_at.is_some());
    let list = bench.engine.list(&bench.store).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].title, "Photosynthesis");
    assert_eq!((list[0].levels_done, list[0].levels_total), (2, 2));
    assert_eq!(
        (list[0].final_correct, list[0].final_total),
        (Some(2), Some(3))
    );
    let notes = bench.engine.notes(&bench.store, &id).await.unwrap();
    assert!(notes.starts_with("# Photosynthesis\n"));
    assert!(notes.contains("## Level 2: Level 2\n\n### Key points\n\n- Key point of Level 2."));
    bench.idle().await;
}

#[tokio::test]
async fn failed_attempts_lead_to_a_new_explanation_then_to_continue_anyway() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Photosynthesis", 10).await;
    bench.until(&id, level_ready(0)).await;
    bench.engine.start(&bench.ctx, &id).await.unwrap();
    let course = bench.until(&id, level_ready(1)).await;
    // Rien de manqué : ni nouvel essai ni saut.
    invalid(bench.engine.remedy(&bench.ctx, &id, 0).await);
    invalid(bench.engine.skip(&bench.ctx, &id, 0).await);
    for question in &course.levels[0].attempts[0].questions {
        bench.answer(&id, question, false).await.unwrap();
    }
    let course = bench.course(&id).await;
    assert_eq!(course.levels[0].failed_attempts, 1);
    assert_eq!(course.levels[0].status, LevelStatus::Current);
    // Un seul essai manqué : pas encore « Continue anyway ».
    invalid(bench.engine.skip(&bench.ctx, &id, 0).await);
    bench.engine.remedy(&bench.ctx, &id, 0).await.unwrap();
    // Demande déjà en cours.
    let pending = bench.course(&id).await;
    if pending.levels[0].remedy.is_some() {
        invalid(bench.engine.remedy(&bench.ctx, &id, 0).await);
    }
    let course = bench
        .until(&id, |course| {
            course.levels[0].attempts.len() == 2 && course.levels[0].remedy.is_none()
        })
        .await;
    let second = &course.levels[0].attempts[1];
    assert!(
        second
            .explanation
            .as_ref()
            .unwrap()
            .contains("The leaf uses the light")
    );
    // Formule écrite pour le rendu de l'app.
    assert!(second.explanation.as_ref().unwrap().ends_with(" \\(x^2\\)"));
    let ids: Vec<&str> = second.questions.iter().map(|q| q.id.as_str()).collect();
    assert_eq!(ids, ["L0-A1-Q0", "L0-A1-Q1", "L0-A1-Q2"]);
    let remedy = &bench.requests_with("has just failed the questions")[0];
    let system = remedy["messages"][0]["content"].as_str().unwrap();
    assert!(
        system.contains("The learner's mistakes:\n- Question: "),
        "{system}"
    );
    assert!(
        system.contains("Learner's answer: Wrong answer"),
        "{system}"
    );
    assert!(system.contains("Correct answer: Right answer"), "{system}");
    assert_eq!(remedy["format"]["required"][0], "explanation");
    for question in &second.questions {
        bench.answer(&id, question, false).await.unwrap();
    }
    let course = bench.course(&id).await;
    assert_eq!(course.levels[0].failed_attempts, 2);
    bench.engine.skip(&bench.ctx, &id, 0).await.unwrap();
    invalid(bench.engine.skip(&bench.ctx, &id, 0).await);
    let course = bench.course(&id).await;
    assert_eq!(course.levels[0].status, LevelStatus::Review);
    assert_eq!(course.levels[1].status, LevelStatus::Current);
    // Le palier suivant n'est plus verrouillé.
    bench
        .answer(&id, &course.levels[1].attempts[0].questions[0], true)
        .await
        .unwrap();
    bench.idle().await;
}

#[tokio::test]
async fn an_ambiguous_topic_is_asked_once() {
    let bench = Bench::new("chatonlm-study-unclear").await;
    let id = bench.create("Java", 20).await;
    let course = bench
        .until(&id, |c| c.status == CourseStatus::Unclear)
        .await;
    assert_eq!(
        course.clarify,
        Some(Clarify {
            question: "Which Java do you mean?".into(),
            options: vec!["The island".into(), "The programming language".into()],
        })
    );
    assert!(course.plan.is_none());
    invalid(bench.engine.start(&bench.ctx, &id).await);
    bench
        .engine
        .clarify(&bench.ctx, &id, Some(" The programming language ".into()))
        .await
        .unwrap();
    let course = bench
        .until(&id, |c| c.status == CourseStatus::Planned)
        .await;
    assert_eq!(
        course.plan.unwrap().title,
        "Java (The programming language)"
    );
    assert_eq!(course.clarify, None);
    let last = bench.requests_with("Design the plan").pop().unwrap();
    assert_eq!(
        last["messages"][1]["content"],
        "Topic: Java\nMeaning: The programming language"
    );
    invalid(bench.engine.clarify(&bench.ctx, &id, None).await);

    // Sujet gardé tel quel : plus de question, même si le modèle en pose.
    let other = bench.create("Java", 10).await;
    bench
        .until(&other, |c| c.status == CourseStatus::Unclear)
        .await;
    bench
        .engine
        .clarify(&bench.ctx, &other, None)
        .await
        .unwrap();
    let course = bench
        .until(&other, |c| c.status == CourseStatus::Planned)
        .await;
    assert_eq!(course.plan.unwrap().levels.len(), 2);
    bench.idle().await;
}

#[tokio::test]
async fn web_search_needs_an_account_and_feeds_the_plan() {
    let bench = Bench::new("chatonlm-study").await;
    let mut request = new_course("Photosynthesis", 10);
    request.web = true;
    let id = bench
        .engine
        .create(&bench.ctx, &Staging::default(), request.clone())
        .await
        .unwrap();
    let course = bench.until(&id, |c| c.status == CourseStatus::Failed).await;
    assert_eq!(
        course.error,
        Some(StudyError {
            code: StudyErrorCode::SignedOut,
            message: "Web search requires an Ollama account".into(),
        })
    );
    assert!(bench.requests_with("Design the plan").is_empty());
    // Sans la recherche web.
    bench.engine.retry(&bench.ctx, &id, true).await.unwrap();
    let course = bench
        .until(&id, |c| c.status == CourseStatus::Planned)
        .await;
    assert!(!course.web);
    assert!(course.sources.is_empty());
    assert_eq!(bench.fake.web_requests().len(), 1);

    // Compte relié : les résultats deviennent des sources du plan.
    bench.fake.set_signed_in(true);
    let id = bench
        .engine
        .create(&bench.ctx, &Staging::default(), request)
        .await
        .unwrap();
    let course = bench
        .until(&id, |c| c.status == CourseStatus::Planned)
        .await;
    assert_eq!(course.sources.len(), 2);
    assert_eq!(course.sources[0].kind, SourceKind::Web);
    assert_eq!(course.sources[0].title, "Result 1 for Photosynthesis");
    assert_eq!(
        course.sources[0].url,
        "https://example.com/1?q=Photosynthesis"
    );
    let web = bench.fake.web_requests();
    assert_eq!(web.last().unwrap()["body"]["query"], "Photosynthesis");
    let system = bench.requests_with("Design the plan").pop().unwrap()["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        system.contains(
            "<source title=\"Result 1 for Photosynthesis\">\nContent 1 about Photosynthesis."
        ),
        "{system}"
    );
    assert!(system.ends_with(prompts::SOURCE_RULE));
    bench.idle().await;
}

async fn stage(staging: &Staging, name: &str, data: &[u8]) -> String {
    match staging.stage_bytes(name.into(), data.to_vec()).await {
        StageResult::Staged { file } => file.id,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn documents_become_sources_and_requests_are_checked() {
    let bench = Bench::new("chatonlm-study").await;
    let staging = Staging::default();
    let notes = stage(
        &staging,
        "notes.md",
        "The chloroplast holds the chlorophyll.\n\nIt absorbs light.".as_bytes(),
    )
    .await;
    let mut request = new_course("  ", 10);
    request.document_ids = vec![notes.clone()];
    let id = bench
        .engine
        .create(&bench.ctx, &staging, request)
        .await
        .unwrap();
    // Repris de la zone de préparation.
    assert!(staging.get(&notes).is_none());
    let course = bench
        .until(&id, |c| c.status == CourseStatus::Planned)
        .await;
    assert_eq!(course.topic, "");
    assert_eq!(
        course.sources,
        [StudySource {
            kind: SourceKind::Document,
            title: "notes.md".into(),
            url: String::new()
        }]
    );
    let plan = bench.requests_with("Design the plan").pop().unwrap();
    let system = plan["messages"][0]["content"].as_str().unwrap();
    assert!(
        system.contains("<source title=\"notes.md\">\nThe chloroplast holds the chlorophyll."),
        "{system}"
    );
    assert_eq!(
        plan["messages"][1]["content"],
        "Topic: the main subject of the source material"
    );
    let list = bench.engine.list(&bench.store).await.unwrap();
    assert_eq!(list[0].title, "the main subject of the source material");

    // Refus : image (remise dans la zone), document inconnu, limites.
    let image = stage(&staging, "pic.png", &fake_ollama::AVATAR_PNG).await;
    let mut request = new_course("Plants", 10);
    request.document_ids = vec![image.clone()];
    let refused = bench
        .engine
        .create(&bench.ctx, &staging, request.clone())
        .await;
    assert!(
        matches!(&refused, Err(AppError::Invalid(message)) if message == "pic.png: images can't be used as course documents"),
        "{refused:?}"
    );
    assert!(staging.get(&image).is_some());
    request.document_ids = vec!["nope".into()];
    let refused = bench
        .engine
        .create(&bench.ctx, &staging, request.clone())
        .await;
    assert!(
        matches!(&refused, Err(AppError::Invalid(m)) if m == "This document is no longer available: add it again")
    );
    request.document_ids = vec!["a".into(), "b".into(), "c".into(), "d".into()];
    invalid(bench.engine.create(&bench.ctx, &staging, request).await);
    let long = "x".repeat(MAX_TOPIC_CHARS + 1);
    let refused = bench
        .engine
        .create(&bench.ctx, &staging, new_course(&long, 10))
        .await;
    assert!(
        matches!(&refused, Err(AppError::Invalid(m)) if m == "The topic is too long: 300 characters at most")
    );
    // Juste à la limite (espaces autour retirés) : accepté.
    let limit = format!(" {} ", "x".repeat(MAX_TOPIC_CHARS));
    bench
        .engine
        .create(&bench.ctx, &staging, new_course(&limit, 10))
        .await
        .unwrap();
    invalid(
        bench
            .engine
            .create(&bench.ctx, &staging, new_course("Plants", 15))
            .await,
    );
    let refused = bench
        .engine
        .create(&bench.ctx, &staging, new_course(" ", 10))
        .await;
    assert!(
        matches!(&refused, Err(AppError::Invalid(m)) if m == "Enter a topic or add a document")
    );
    // Aucun modèle, ni des cours ni du chat.
    set_models(&bench.store, None, "", "");
    let refused = bench
        .engine
        .create(&bench.ctx, &staging, new_course("Plants", 10))
        .await;
    assert!(
        matches!(&refused, Err(AppError::Invalid(m)) if m == "Choose a model for your courses")
    );
    // Modèle du chat, à défaut de celui des cours.
    set_models(&bench.store, None, "chatonlm-study", "");
    bench
        .engine
        .create(&bench.ctx, &staging, new_course("Plants", 10))
        .await
        .unwrap();
    bench.idle().await;
}

#[tokio::test]
async fn unusable_questions_fail_the_level_until_retried() {
    let bench = Bench::new("chatonlm-study-invalid").await;
    let id = bench.create("Photosynthesis", 10).await;
    let course = bench
        .until(&id, |c| {
            c.levels
                .first()
                .is_some_and(|l| l.state == GenState::Failed)
        })
        .await;
    assert_eq!(course.status, CourseStatus::Planned);
    let level = &course.levels[0];
    assert_eq!(level.error.as_ref().unwrap().code, StudyErrorCode::Invalid);
    assert_eq!(
        level.error.as_ref().unwrap().message,
        "The model's answers could not be used. Try again, or choose a larger model."
    );
    // Leçon gardée ; une demande de questions, puis une pour compléter ; pas
    // de vérification (tout écarté par le garde-fou).
    assert!(level.lesson.is_some());
    // Deux demandes, puis la dernière chance (choix seulement).
    assert_eq!(bench.requests_with("You write the questions").len(), 3);
    assert!(bench.requests_with("careful student").is_empty());
    // Nouvel essai avec un modèle qui écrit de bonnes questions : la leçon
    // n'est pas redemandée.
    set_models(&bench.store, None, "", "chatonlm-study");
    bench.engine.retry(&bench.ctx, &id, false).await.unwrap();
    let course = bench.until(&id, level_ready(0)).await;
    assert_eq!(bench.requests_with("Write level 1").len(), 1);
    assert_eq!(course.levels[0].attempts[0].questions.len(), 3);
    bench.idle().await;
}

#[tokio::test]
async fn a_missing_model_fails_a_part_until_retried() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Photosynthesis", 10).await;
    bench.until(&id, level_ready(0)).await;
    bench.idle().await;
    set_models(&bench.store, None, "", "");
    bench.engine.start(&bench.ctx, &id).await.unwrap();
    let course = bench
        .until(&id, |c| c.levels[1].state == GenState::Failed)
        .await;
    assert_eq!(
        course.levels[1].error,
        Some(StudyError {
            code: StudyErrorCode::NoModel,
            message: "Choose a model for your courses".into(),
        })
    );
    set_models(&bench.store, None, "", "nope:1b");
    bench.engine.retry(&bench.ctx, &id, false).await.unwrap();
    let course = bench
        .until(&id, |c| {
            c.levels[1].state == GenState::Failed
                && c.levels[1].error.as_ref().unwrap().code == StudyErrorCode::ModelNotFound
        })
        .await;
    assert_eq!(
        course.levels[1].error.as_ref().unwrap().message,
        "The model nope:1b is not installed"
    );
    // Le modèle réglé est lu à chaque génération ; celui du plan reste.
    set_models(&bench.store, None, "chatonlm-study", "");
    bench.engine.retry(&bench.ctx, &id, false).await.unwrap();
    let course = bench.until(&id, final_ready).await;
    assert_eq!(course.model, "chatonlm-study");
    bench.idle().await;
    // Serveur arrêté : injoignable.
    let url = bench.fake.url();
    let Bench {
        fake,
        engine,
        ctx,
        store,
        ..
    } = bench;
    fake.stop().await;
    let id = engine
        .create(&ctx, &Staging::default(), new_course("Plants", 10))
        .await
        .unwrap();
    let started = std::time::Instant::now();
    loop {
        let snapshot = db_load(&store, &id).await;
        if snapshot.row.status == CourseStatus::Failed {
            let error = snapshot.row.error.unwrap();
            assert_eq!(error.code, StudyErrorCode::Unreachable);
            assert_eq!(error.message, format!("can't connect to Ollama at {url}"));
            break;
        }
        assert!(started.elapsed() < WAIT);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn db_load(store: &Store, id: &str) -> stored::Snapshot {
    let id = id.to_owned();
    store
        .call(move |c| db::load(c, &id))
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn deleting_a_course_stops_its_generation() {
    let bench = Bench::new("chatonlm-study-slow").await;
    let id = bench.create("Photosynthesis", 10).await;
    let started = std::time::Instant::now();
    while bench.engine.running().is_none() {
        assert!(started.elapsed() < WAIT);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Requête du plan partie.
    while bench.fake.chat_requests().is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    bench.engine.delete(&bench.ctx, &id).await.unwrap();
    assert!(bench.events.deleted(&id));
    bench.idle().await;
    assert!(started.elapsed() < fake_ollama::STUDY_SLOW * 3);
    let abandoned = async {
        while bench.fake.disconnects() == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), abandoned)
        .await
        .unwrap();
    assert!(
        bench
            .store
            .call({
                let id = id.clone();
                move |c| db::load(c, &id)
            })
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(bench.fake.chat_requests().len(), 1);
    assert!(matches!(
        bench.engine.get(&bench.ctx, &id).await,
        Err(AppError::NotFound(_))
    ));
    // Supprimer un cours absent : sans effet.
    bench.engine.delete(&bench.ctx, &id).await.unwrap();
}

/// rust-1 : Ollama s'arrête pendant la génération d'un palier (fermeture,
/// retour à l'Ollama de la machine, changement du dossier de l'intégré) :
/// les travaux sont arrêtés d'abord ; le palier n'est pas noté en échec et
/// reprend à la lecture suivante du cours, auprès du serveur qui sert alors.
#[tokio::test]
async fn a_part_stopped_with_ollama_is_not_failed_and_resumes() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let store = configured(&fake.url(), "", "chatonlm-study");
    let events = Events::default();
    let ctx = events.ctx(&store);
    let engine = StudyEngine::with_timeouts(super::request::Timeouts {
        retry_delay: Duration::from_millis(100),
        ..super::request::TIMEOUTS
    });
    let id = engine
        .create(&ctx, &Staging::default(), new_course("Photosynthesis", 10))
        .await
        .unwrap();
    let bench_until = |ready: fn(&Course) -> bool| {
        let (store, id) = (store.clone(), id.clone());
        async move {
            let started = std::time::Instant::now();
            loop {
                let course = stored::assemble(db_load(&store, &id).await);
                if ready(&course) {
                    return course;
                }
                assert!(started.elapsed() < WAIT, "{:?}", course.levels);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    };
    bench_until(|c| c.levels.first().is_some_and(|l| l.state == GenState::Ready)).await;
    set_models(&store, None, "", "chatonlm-study-slow");
    engine.start(&ctx, &id).await.unwrap();
    let started = std::time::Instant::now();
    while engine.running() != Some((id.clone(), generate::Part::Level(1))) {
        assert!(started.elapsed() < WAIT);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let asked = fake.chat_requests().len();
    while fake.chat_requests().len() == asked {
        assert!(started.elapsed() < WAIT);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Travaux arrêtés, puis Ollama s'arrête.
    engine.cancel_all();
    assert!(engine.queued().is_empty());
    fake.stop().await;
    let started = std::time::Instant::now();
    while !engine.idle() {
        assert!(started.elapsed() < WAIT);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let course = stored::assemble(db_load(&store, &id).await);
    for level in &course.levels[1..] {
        assert_ne!(level.state, GenState::Failed, "{:?}", level.error);
        assert_eq!(level.error, None);
    }
    assert_eq!(course.levels[1].state, GenState::Generating);

    // Un autre Ollama sert : le cours relu reprend où il en était.
    let back = FakeOllama::start(Mode::ollama()).await.unwrap();
    set_models(&store, Some(&back.url()), "", "chatonlm-study");
    engine.get(&ctx, &id).await.unwrap();
    bench_until(|c| c.levels.iter().all(|l| l.state == GenState::Ready)).await;
}

/// rust-8 : créer un cours ne lit que les réglages (modèle des cours), sans
/// attendre l'Ollama intégré qui démarre.
#[tokio::test(flavor = "multi_thread")]
async fn creating_a_course_does_not_wait_for_the_builtin_ollama() {
    use crate::ollama::builtin::Builtin;
    use crate::ollama::builtin::tests::{fake_config, temp_dir};
    use crate::ollama::target::{TEST_TARGETING, Targeting};
    use crate::store::builtin_ollama::{self, OllamaSource};

    let dir = temp_dir();
    let builtin = Builtin::new(fake_config(dir.path(), &[("FAKE_OLLAMA_DELAY_MS", "4000")]));
    let targeting = Arc::new(Targeting::new(builtin.clone(), |_| {}).without_ollama_host());
    // Serveur réglé (adresse explicite) : le plan, généré hors de cette
    // tâche, va au faux serveur.
    let bench = Bench::new("chatonlm-study").await;
    bench
        .store
        .blocking(|c| builtin_ollama::set_source(c, OllamaSource::Builtin))
        .unwrap();
    let created = TEST_TARGETING.scope(targeting.clone(), async {
        let started = std::time::Instant::now();
        let id = bench.create("Photosynthesis", 10).await;
        (id, started.elapsed())
    });
    let (id, elapsed) = created.await;
    assert!(elapsed < Duration::from_millis(1500), "{elapsed:?}");
    assert_eq!(
        builtin.snapshot().state,
        crate::ollama::builtin::BuiltinState::Starting
    );
    bench.engine.delete(&bench.ctx, &id).await.unwrap();
    builtin.stop().await;
}

#[tokio::test]
async fn a_level_reported_as_wrong_is_written_again() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Photosynthesis", 10).await;
    let course = bench.until(&id, level_ready(0)).await;
    bench.engine.start(&bench.ctx, &id).await.unwrap();
    bench.until(&id, final_ready).await;
    bench.idle().await;
    let check = course.levels[0].check.clone().unwrap();
    bench.answer(&id, &check, true).await.unwrap();
    bench
        .answer(&id, &course.levels[0].attempts[0].questions[0], true)
        .await
        .unwrap();
    let lessons_before = bench.requests_with("Write level 1").len();
    bench.engine.regenerate(&bench.ctx, &id, 0).await.unwrap();
    let course = bench
        .until(&id, |c| level_ready(0)(c) && bench_answers_cleared(c))
        .await;
    assert_eq!(
        bench.requests_with("Write level 1").len(),
        lessons_before + 1
    );
    // Mêmes identifiants, réponses effacées.
    assert_eq!(course.levels[0].check.as_ref().unwrap().id, "L0-check");
    assert!(course.answers.keys().all(|key| !key.starts_with("L0-")));
    invalid(bench.engine.regenerate(&bench.ctx, &id, 9).await);

    // Palier refait pendant sa génération (modèle lent) : requête abandonnée,
    // palier refait par le modèle réglé au moment de la génération.
    set_models(&bench.store, None, "", "chatonlm-study-slow");
    let lessons = bench.requests_with("Write level 2").len();
    bench.engine.regenerate(&bench.ctx, &id, 1).await.unwrap();
    let started = std::time::Instant::now();
    // Leçon demandée (réponse lente en cours).
    while bench.requests_with("Write level 2").len() == lessons {
        assert!(started.elapsed() < WAIT);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        bench.engine.running(),
        Some((id.clone(), generate::Part::Level(1)))
    );
    let disconnects = bench.fake.disconnects();
    bench.engine.regenerate(&bench.ctx, &id, 1).await.unwrap();
    let course = bench.until(&id, level_ready(1)).await;
    assert!(bench.fake.disconnects() > disconnects);
    // La leçon abandonnée n'a rien écrit : une seconde leçon demandée.
    assert_eq!(bench.requests_with("Write level 2").len(), lessons + 2);
    assert_eq!(course.model, "chatonlm-study");
    let last = bench.fake.chat_requests().pop().unwrap();
    assert_eq!(last["model"], "chatonlm-study-slow");
    bench.idle().await;
}

/// Pause d'un cours : sa partie en cours d'écriture est arrêtée aussitôt
/// (requête abandonnée), ses parties quittent la file et n'y reviennent ni
/// à la lecture du cours ni dans une nouvelle session ; l'autre cours est
/// écrit jusqu'au bout ; la reprise refait la partie arrêtée.
#[tokio::test]
async fn a_paused_course_is_not_written_until_resumed() {
    let bench = Bench::new("chatonlm-study").await;
    let paused = bench.create("Photosynthesis", 10).await;
    let other = bench.create("Fractions", 10).await;
    bench.until(&paused, level_ready(0)).await;
    bench.until(&other, level_ready(0)).await;
    bench.idle().await;
    // Modèle lent, le temps de lancer le palier 1 du premier cours.
    set_models(&bench.store, None, "", "chatonlm-study-slow");
    let lessons = bench.requests_with("Write level 2").len();
    bench.engine.start(&bench.ctx, &paused).await.unwrap();
    let started = std::time::Instant::now();
    while bench.engine.running() != Some((paused.clone(), generate::Part::Level(1)))
        || bench.requests_with("Write level 2").len() == lessons
    {
        assert!(started.elapsed() < WAIT);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    set_models(&bench.store, None, "", "chatonlm-study");
    bench.engine.start(&bench.ctx, &other).await.unwrap();
    assert_eq!(
        bench.course(&paused).await.levels[1].state,
        GenState::Generating
    );
    // L'autre cours attend son tour, et sa page le sait.
    assert!(
        bench
            .engine
            .get(&bench.ctx, &other)
            .await
            .unwrap()
            .queue_busy
    );
    assert!(
        !bench
            .engine
            .get(&bench.ctx, &paused)
            .await
            .unwrap()
            .queue_busy
    );
    let (disconnects, events) = (bench.fake.disconnects(), bench.events.count(&paused));

    bench
        .engine
        .set_paused(&bench.ctx, &paused, true)
        .await
        .unwrap();
    // Aussitôt : plus rien de ce cours n'est en cours ni en file, et la page
    // l'apprend.
    let course = bench.course(&paused).await;
    assert!(course.paused);
    assert_eq!(course.levels[1].state, GenState::Pending);
    assert!(bench.events.count(&paused) > events);
    assert!(bench.engine.queued().iter().all(|(id, _)| *id != paused));
    assert_ne!(
        bench.engine.running().map(|(id, _)| id),
        Some(paused.clone())
    );
    let abandoned = async {
        while bench.fake.disconnects() == disconnects {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), abandoned)
        .await
        .unwrap();

    // L'autre cours continue jusqu'au bilan ; celui en pause n'avance pas.
    bench.until(&other, final_ready).await;
    bench.idle().await;
    let course = bench.course(&paused).await;
    assert_eq!(course.levels[1].state, GenState::Pending);
    assert_eq!(course.levels[1].lesson, None);
    assert_eq!(
        course.final_quiz.as_ref().map(|quiz| quiz.state),
        Some(GenState::Pending)
    );
    // Le lire ne relance rien, pas plus qu'une nouvelle session (la pause
    // est enregistrée).
    assert!(bench.engine.get(&bench.ctx, &paused).await.unwrap().paused);
    assert!(bench.engine.idle());
    let engine = StudyEngine::default();
    assert!(engine.get(&bench.ctx, &paused).await.unwrap().paused);
    assert!(engine.idle() && engine.queued().is_empty());
    let listed = engine.list(&bench.store).await.unwrap();
    let row = listed.iter().find(|row| row.id == paused).unwrap();
    assert_eq!((row.paused, row.pending, row.levels_ready), (true, true, 1));

    // Reprise : le palier arrêté est refait, puis le bilan.
    let lessons = bench.requests_with("Write level 2").len();
    engine.set_paused(&bench.ctx, &paused, false).await.unwrap();
    let course = bench.until(&paused, final_ready).await;
    assert!(!course.paused);
    assert!(level_ready(1)(&course));
    assert_eq!(bench.requests_with("Write level 2").len(), lessons + 1);
    let started = std::time::Instant::now();
    while !engine.idle() {
        assert!(started.elapsed() < WAIT);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let listed = engine.list(&bench.store).await.unwrap();
    let row = listed.iter().find(|row| row.id == paused).unwrap();
    assert_eq!(
        (row.paused, row.pending, row.levels_ready),
        (false, false, 2)
    );
    // Cours inconnu.
    assert!(matches!(
        engine.set_paused(&bench.ctx, "nope", true).await,
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        engine.set_paused(&bench.ctx, "nope", false).await,
        Err(AppError::NotFound(_))
    ));
}

/// Partie mise en file au moment même de la pause (demande lue juste avant,
/// arrivée juste après) : le travail qui démarre voit la pause et n'écrit
/// rien, ni ne demande rien au modèle.
#[tokio::test]
async fn a_part_queued_as_the_course_is_paused_is_not_written() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Photosynthesis", 10).await;
    bench.until(&id, level_ready(0)).await;
    bench.idle().await;
    bench
        .engine
        .set_paused(&bench.ctx, &id, true)
        .await
        .unwrap();
    bench.engine.start(&bench.ctx, &id).await.unwrap();
    let requests = bench.fake.chat_requests().len();
    for part in [generate::Part::Level(1), generate::Part::Final] {
        bench.engine.enqueue_unchecked(&bench.ctx, &id, part);
    }
    bench.idle().await;
    let course = bench.course(&id).await;
    assert_eq!(course.levels[1].state, GenState::Pending);
    assert_eq!(bench.fake.chat_requests().len(), requests);
    // Plan d'un cours en pause (modèle lent : arrêté avant d'être écrit) :
    // pas écrit non plus.
    set_models(&bench.store, None, "", "chatonlm-study-slow");
    let planning = bench.create("Fractions", 10).await;
    bench
        .engine
        .set_paused(&bench.ctx, &planning, true)
        .await
        .unwrap();
    bench.idle().await;
    set_models(&bench.store, None, "", "chatonlm-study");
    let requests = bench.fake.chat_requests().len();
    bench
        .engine
        .enqueue_unchecked(&bench.ctx, &planning, generate::Part::Plan);
    bench.idle().await;
    assert_eq!(bench.course(&planning).await.status, CourseStatus::Planning);
    assert_eq!(bench.fake.chat_requests().len(), requests);
    bench
        .engine
        .set_paused(&bench.ctx, &planning, false)
        .await
        .unwrap();
    bench.until(&planning, level_ready(0)).await;
    bench.idle().await;
}

/// Une demande de l'apprenant qui exige une génération reprend un cours en
/// pause (il ne reste jamais bloqué en silence) : nouveau plan, palier
/// refait, nouvel essai, « Retry », précision du sujet. « Start » ne demande
/// rien à écrire tout de suite : la pause tient.
#[tokio::test]
async fn a_request_that_needs_writing_resumes_a_paused_course() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Photosynthesis", 10).await;
    bench.until(&id, level_ready(0)).await;
    bench.idle().await;
    let pause = || async {
        bench
            .engine
            .set_paused(&bench.ctx, &id, true)
            .await
            .unwrap();
        assert!(bench.course(&id).await.paused);
    };

    // Nouveau plan.
    pause().await;
    let plans = bench.requests_with("Design the plan").len();
    bench.engine.replan(&bench.ctx, &id).await.unwrap();
    let course = bench
        .until(&id, |c| {
            c.status == CourseStatus::Planned && level_ready(0)(c)
        })
        .await;
    assert!(!course.paused);
    assert_eq!(bench.requests_with("Design the plan").len(), plans + 1);
    bench.idle().await;

    // « Start » : la pause tient, rien n'est écrit.
    pause().await;
    bench.engine.start(&bench.ctx, &id).await.unwrap();
    bench.idle().await;
    let course = bench.course(&id).await;
    assert!(course.paused);
    assert_eq!(course.levels[1].state, GenState::Pending);

    // Palier refait (« Report a mistake ») : tout ce qui manque est écrit.
    bench.engine.regenerate(&bench.ctx, &id, 0).await.unwrap();
    let course = bench
        .until(&id, |c| level_ready(0)(c) && final_ready(c))
        .await;
    assert!(!course.paused);
    bench.idle().await;

    // Nouvel essai (« Explain differently »).
    for question in &course.levels[0].attempts[0].questions {
        bench.answer(&id, question, false).await.unwrap();
    }
    pause().await;
    bench.engine.remedy(&bench.ctx, &id, 0).await.unwrap();
    let course = bench
        .until(&id, |c| {
            c.levels[0].attempts.len() == 2 && c.levels[0].remedy.is_none()
        })
        .await;
    assert!(!course.paused);
    bench.idle().await;

    // « Retry » d'un palier en échec.
    let course_id = id.clone();
    bench
        .store
        .call(move |c| {
            db::update_level(c, &course_id, 1, |level| {
                *level = stored::LevelData {
                    state: GenState::Failed,
                    ..stored::LevelData::default()
                };
                Ok(())
            })
        })
        .await
        .unwrap();
    pause().await;
    bench.engine.retry(&bench.ctx, &id, false).await.unwrap();
    let course = bench.until(&id, level_ready(1)).await;
    assert!(!course.paused);
    bench.idle().await;

    // Précision d'un sujet ambigu.
    set_models(&bench.store, None, "", "chatonlm-study-unclear");
    let unclear = bench.create("Java", 10).await;
    bench
        .until(&unclear, |c| c.status == CourseStatus::Unclear)
        .await;
    bench
        .engine
        .set_paused(&bench.ctx, &unclear, true)
        .await
        .unwrap();
    bench
        .engine
        .clarify(&bench.ctx, &unclear, None)
        .await
        .unwrap();
    let course = bench
        .until(&unclear, |c| c.status == CourseStatus::Planned)
        .await;
    assert!(!course.paused);
    bench.idle().await;
}

fn bench_answers_cleared(course: &Course) -> bool {
    course.answers.keys().all(|key| !key.starts_with("L0-"))
}

#[tokio::test]
async fn parts_interrupted_by_a_previous_session_are_generated_again() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Photosynthesis", 10).await;
    bench.until(&id, level_ready(0)).await;
    bench.idle().await;
    // Fermeture pendant la génération du palier 0.
    let course = id.clone();
    bench
        .store
        .call(move |c| {
            db::update_level(c, &course, 0, |level| {
                *level = stored::LevelData {
                    state: GenState::Generating,
                    ..stored::LevelData::default()
                };
                Ok(())
            })
        })
        .await
        .unwrap();
    // Nouvelle session : nouveau moteur, même base.
    let engine = StudyEngine::default();
    let course = engine.get(&bench.ctx, &id).await.unwrap();
    assert_eq!(course.levels[0].state, GenState::Pending);
    let course = bench.until(&id, level_ready(0)).await;
    assert!(course.levels[0].lesson.is_some());
    let started = std::time::Instant::now();
    while !engine.idle() {
        assert!(started.elapsed() < WAIT);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_new_plan_throws_the_levels_away() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Photosynthesis", 10).await;
    bench.until(&id, level_ready(0)).await;
    bench.idle().await;
    let plans = bench.requests_with("Design the plan").len();
    bench.engine.replan(&bench.ctx, &id).await.unwrap();
    let course = bench.course(&id).await;
    assert!(matches!(
        course.status,
        CourseStatus::Planning | CourseStatus::Planned
    ));
    let course = bench.until(&id, level_ready(0)).await;
    assert_eq!(bench.requests_with("Design the plan").len(), plans + 1);
    assert_eq!(course.levels.len(), 2);
    bench.engine.start(&bench.ctx, &id).await.unwrap();
    invalid(bench.engine.replan(&bench.ctx, &id).await);
    bench.idle().await;
}

#[tokio::test]
async fn progress_is_saved_without_an_event() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Photosynthesis", 10).await;
    bench.until(&id, level_ready(0)).await;
    bench.idle().await;
    let events = bench.events.count(&id);
    bench
        .engine
        .progress(&bench.store, &id, "L0:s1".into(), 4_200)
        .await
        .unwrap();
    let course = bench.course(&id).await;
    assert_eq!(
        (course.position.as_str(), course.elapsed_ms),
        ("L0:s1", 4_200)
    );
    assert_eq!(bench.events.count(&id), events);
    assert!(matches!(
        bench
            .engine
            .progress(&bench.store, "nope", "x".into(), 1)
            .await,
        Err(AppError::NotFound(_))
    ));
}

/// Canal qui garde les évènements (en JSON) ; refuse les envois à partir du
/// `fail_from`-ième.
fn channel(fail_from: Option<usize>) -> (Channel<StudyStreamEvent>, Arc<Mutex<Vec<Value>>>) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    let channel = Channel::new(move |body| {
        let mut events = sink.lock();
        if fail_from.is_some_and(|from| events.len() + 1 >= from) {
            return Err(tauri::Error::FailedToReceiveMessage);
        }
        if let InvokeResponseBody::Json(text) = body {
            events.push(serde_json::from_str(&text).unwrap());
        }
        Ok(())
    });
    (channel, received)
}

#[tokio::test]
async fn explanations_and_answers_stream_then_are_kept() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Photosynthesis", 10).await;
    bench.until(&id, level_ready(0)).await;
    bench.idle().await;
    let (events, received) = channel(None);
    bench
        .engine
        .explain(&bench.ctx, &id, 0, 1, "s1".into(), &events)
        .await
        .unwrap();
    let received = received.lock().clone();
    let deltas: String = received
        .iter()
        .filter(|event| event["event"] == "delta")
        .map(|event| event["text"].as_str().unwrap())
        .collect();
    assert!(received.iter().filter(|e| e["event"] == "delta").count() >= 2);
    // Formule `$…$` du modèle écrite `\(…\)` au fil du flux (coupée entre
    // deux fragments), montant gardé.
    let expected = "**Simpler:** The leaf uses the light of the sun and the water from the soil to make the sugar that it needs every day. Half is \\(\\frac{1}{2}\\), it costs $5.";
    assert_eq!(deltas, expected);
    assert_eq!(
        received.last().unwrap(),
        &serde_json::json!({"event": "done", "text": expected, "stopped": false})
    );
    let course = bench.course(&id).await;
    assert_eq!(
        course.levels[0].explains,
        [Explain {
            section: 1,
            text: expected.into()
        }]
    );
    let request = bench
        .requests_with("Explain this section again")
        .pop()
        .unwrap();
    assert_eq!(request["stream"], true);
    assert!(request.get("format").is_none());
    assert!(
        request["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("section \"Part two\"")
    );
    invalid(
        bench
            .engine
            .explain(&bench.ctx, &id, 0, 3, "s".into(), &events)
            .await,
    );
    invalid(
        bench
            .engine
            .explain(&bench.ctx, &id, 1, 0, "s".into(), &events)
            .await,
    );

    // Questions : les trois précédentes en contexte.
    for n in 0..4 {
        let (events, _) = channel(None);
        bench
            .engine
            .ask(
                &bench.ctx,
                &id,
                0,
                format!("Question {n}?"),
                format!("a{n}"),
                &events,
            )
            .await
            .unwrap();
    }
    let course = bench.course(&id).await;
    assert_eq!(course.levels[0].asks.len(), 4);
    assert_eq!(course.levels[0].asks[3].question, "Question 3?");
    assert!(course.levels[0].asks[3].answer.starts_with("**Answer:**"));
    let last = bench
        .requests_with("Answer the learner's question")
        .pop()
        .unwrap();
    let messages = last["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1 + 2 * 3 + 1);
    assert_eq!(messages[1]["content"], "Question 0?");
    assert_eq!(messages[7]["content"], "Question 3?");
    invalid(
        bench
            .engine
            .ask(&bench.ctx, &id, 0, "  ".into(), "x".into(), &events)
            .await,
    );
    let long = "x".repeat(MAX_QUESTION_CHARS + 1);
    invalid(
        bench
            .engine
            .ask(&bench.ctx, &id, 0, long, "x".into(), &events)
            .await,
    );
    assert_eq!(bench.engine.streams(), 0);
}

#[tokio::test]
async fn a_stopped_stream_keeps_nothing() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Photosynthesis", 10).await;
    bench.until(&id, level_ready(0)).await;
    bench.idle().await;
    set_models(&bench.store, None, "", "chatonlm-study-slow");
    let engine = Arc::new(bench.engine);
    let (events, received) = channel(None);
    let task = {
        let (engine, ctx, id) = (engine.clone(), bench.ctx.clone(), id.clone());
        tokio::spawn(async move {
            engine
                .ask(&ctx, &id, 0, "Why?".into(), "stream-1".into(), &events)
                .await
        })
    };
    let first = async {
        while !received.lock().iter().any(|e| e["event"] == "delta") {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(WAIT, first).await.unwrap();
    assert_eq!(engine.streams(), 1);
    engine.cancel_stream("other");
    engine.cancel_stream("stream-1");
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let last = received.lock().last().cloned().unwrap();
    assert_eq!(last["event"], "done");
    assert_eq!(last["stopped"], true);
    assert!(!last["text"].as_str().unwrap().is_empty());
    let course = db_load(&bench.store, &id).await;
    assert!(course.levels[0].asks.is_empty());
    assert_eq!(engine.streams(), 0);

    // Canal fermé (page rechargée) : le flux s'arrête.
    let (events, received) = channel(Some(2));
    let started = std::time::Instant::now();
    engine
        .explain(&bench.ctx, &id, 0, 0, "s".into(), &events)
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(received.lock().len(), 1);
    assert!(
        db_load(&bench.store, &id).await.levels[0]
            .explains
            .is_empty()
    );

    // Modèle absent : erreur du flux.
    set_models(&bench.store, None, "", "nope:1b");
    let (events, received) = channel(None);
    engine
        .explain(&bench.ctx, &id, 0, 0, "s".into(), &events)
        .await
        .unwrap();
    assert_eq!(
        received.lock().clone(),
        [
            serde_json::json!({"event": "error", "code": "modelNotFound", "message": "The model nope:1b is not installed"})
        ]
    );
}

/// Cours créé dans une autre langue de l'interface (ligne écrite
/// directement : la langue du moteur est commune à tous les essais).
#[tokio::test]
async fn a_course_in_french_is_written_and_checked_in_french() {
    let bench = Bench::new("chatonlm-study").await;
    let mut row = stored::tests::row(10);
    row.id = "fr-course".into();
    row.topic = "La photosynthèse".into();
    row.language = crate::i18n::Lang::Fr;
    bench
        .store
        .call(move |c| db::insert(c, &row, &[]))
        .await
        .unwrap();
    // Lecture du cours : plan relancé.
    bench.engine.get(&bench.ctx, "fr-course").await.unwrap();
    let course = bench.until("fr-course", level_ready(0)).await;
    assert_eq!(course.status, CourseStatus::Planned);
    assert!(
        course.levels[0].lesson.as_ref().unwrap().sections[0]
            .body
            .starts_with("La feuille")
    );
    let plan = &bench.requests_with("Design the plan")[0];
    assert!(
        plan["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Write everything in French.")
    );
    let lesson = &bench.requests_with("Write level 1")[0];
    assert!(
        lesson["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("« vous »")
    );
    let notes = bench.engine.notes(&bench.store, "fr-course").await.unwrap();
    assert!(notes.contains("## Objectifs"));
    assert!(notes.contains("### À retenir"));
    bench.idle().await;
}

const TEST_INSTANCE: &str = "http://127.0.0.1:11435";
const TEST_MODEL: &str = "qwen3:0.6b";

/// Contrat : vrai cours par qwen3:0.6b sur l'Ollama de test (port 11435) :
/// plan JSON lisible au bon nombre de paliers, premier palier écrit (leçon
/// réparée et validée ; questions gardées, ou rejetées en bloc par les
/// garde-fous pour un si petit modèle), raisonnement coupé, explication en
/// flux. La qualité n'est pas exigée.
#[tokio::test]
#[ignore = "needs the test Ollama instance (scripts/test-ollama.sh) with qwen3:0.6b"]
async fn contract_study_course_with_a_small_model() {
    let store = configured(TEST_INSTANCE, "", TEST_MODEL);
    let events = Events::default();
    let ctx = events.ctx(&store);
    let engine = StudyEngine::default();
    let id = engine
        .create(&ctx, &Staging::default(), new_course("Photosynthesis", 10))
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let course = loop {
        let course = stored::assemble(db_load(&store, &id).await);
        let level_done = course
            .levels
            .first()
            .is_some_and(|level| matches!(level.state, GenState::Ready | GenState::Failed));
        if course.status == CourseStatus::Failed || level_done {
            break course;
        }
        assert!(started.elapsed() < Duration::from_secs(600), "{course:#?}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    eprintln!("{}", serde_json::to_string_pretty(&course).unwrap());
    assert_eq!(course.status, CourseStatus::Planned, "{:?}", course.error);
    assert_eq!(course.plan.as_ref().unwrap().levels.len(), 2);
    let level = &course.levels[0];
    let lesson = level.lesson.as_ref().expect("lesson written");
    assert!((2..=3).contains(&lesson.sections.len()));
    assert!(!serde_json::to_string(lesson).unwrap().contains('\u{c}'));
    match level.state {
        GenState::Ready => assert!(level.attempts[0].questions.len() >= 2),
        _ => assert_eq!(level.error.as_ref().unwrap().code, StudyErrorCode::Invalid),
    }
    // Explication en flux, sans raisonnement.
    let (channel, received) = channel(None);
    engine
        .explain(&ctx, &id, 0, 0, "contract".into(), &channel)
        .await
        .unwrap();
    let last = received.lock().last().cloned().unwrap();
    assert_eq!(last["event"], "done", "{last}");
    let text = last["text"].as_str().unwrap();
    assert!(
        !text.trim().is_empty() && !text.contains("<think"),
        "{text}"
    );
    // qwen3 sait raisonner : `think: false` lui est envoyé.
    let details = crate::ollama::models::show(TEST_INSTANCE, TEST_MODEL)
        .await
        .unwrap();
    assert!(crate::dictation::polish::think_off(Some(&details), false));
}

/// Chaque chaîne d'une réponse ne dépasse pas son `maxLength`.
fn within_limits(value: &Value, schema: &Value) -> bool {
    if let (Some(text), Some(max)) = (value.as_str(), schema["maxLength"].as_u64()) {
        return text.chars().count() as u64 <= max;
    }
    match schema["type"].as_str() {
        Some("object") => schema["properties"].as_object().is_some_and(|properties| {
            properties
                .iter()
                .all(|(key, property)| within_limits(&value[key.as_str()], property))
        }),
        Some("array") => value.as_array().is_none_or(|items| {
            items
                .iter()
                .all(|item| within_limits(item, &schema["items"]))
        }),
        _ => true,
    }
}

/// Contrat : ce que garantit le schéma `format`, même à un très petit
/// modèle, à température 0 (reproductible) : pour 2, 4 et 6 paliers, la
/// réponse brute est un JSON complet (done_reason « stop »), avec exactement
/// le bon nombre de paliers et chaque texte dans sa limite `maxLength`. Pas
/// davantage : mesuré le 7 oct., qwen3:0.6b boucle dans `scope` (« …des
/// images, des photographies,等 » : il prend la photosynthèse pour la
/// photographie), à 4 et 6 paliers, et parfois à 2 sous charge (Ollama n'est
/// pas tout à fait reproductible à température 0 quand d'autres requêtes
/// tournent) ; `maxLength` coupe le texte, la détection d'emballement
/// (`checks::runaway`, testée par ailleurs) refuse la réponse : c'est la
/// capacité du modèle, pas le schéma. Un plan utilisable de bout en bout :
/// moteur contre le faux serveur, et e2e réel (refus propre accepté).
#[tokio::test]
#[ignore = "needs the test Ollama instance (scripts/test-ollama.sh) with qwen3:0.6b"]
async fn contract_study_plan_schema_is_followed() {
    let store = configured(TEST_INSTANCE, "", TEST_MODEL);
    let cancel = crate::proofread::Cancel::default();
    let ollama = super::request::Gen::connect(&store, &cancel, super::request::TIMEOUTS)
        .await
        .unwrap();
    assert!(ollama.think_off);
    for count in [2, 4, 6] {
        let system = prompts::plan_prompt(crate::i18n::Lang::Fr, StudyLevel::Beginner, 20, count);
        let schema = prompts::plan_schema(count);
        let answer = ollama
            .ask_raw(
                &system,
                "Topic: La photosynthèse",
                &schema,
                prompts::CHECK_TEMPERATURE,
            )
            .await
            .unwrap()
            .expect("an answer");
        assert_eq!(answer["done_reason"], "stop", "{count}: {answer}");
        let content = answer["message"]["content"].as_str().unwrap();
        let value: Value = serde_json::from_str(content).expect("complete JSON");
        eprintln!("{count}: {value:#}");
        assert_eq!(value["levels"].as_array().unwrap().len(), count);
        assert!(within_limits(&value, &schema.value()), "{count}: {value:#}");
    }
}

#[tokio::test]
async fn a_lesson_of_the_wrong_length_is_asked_again_then_kept() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Short lessons", 10).await;
    let course = bench.until(&id, level_ready(0)).await;
    assert_eq!(bench.requests_with("Write level 1").len(), 2);
    let lesson = course.levels[0].lesson.as_ref().unwrap();
    assert!(!checks::length_ok(lesson));
    assert_eq!(lesson.sections.len(), 3);
    // Une leçon de bonne longueur n'est demandée qu'une fois.
    let other = bench.create("Photosynthesis", 10).await;
    bench.until(&other, level_ready(0)).await;
    assert_eq!(bench.requests_with("Write level 1").len(), 3);
    bench.idle().await;
}

#[tokio::test]
async fn questions_the_checker_disputes_are_dropped() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Disputed facts", 10).await;
    let course = bench.until(&id, level_ready(0)).await;
    let level = &course.levels[0];
    // Choix contestés (entraînement compris) écartés, même après la
    // demande qui complète ; vrai/faux et association gardés.
    assert!(level.check.is_none());
    let attempt = &level.attempts[0];
    assert_eq!(attempt.questions.len(), 2);
    assert!(matches!(
        attempt.questions[0].body,
        QuestionBody::TrueFalse { .. }
    ));
    assert!(matches!(
        attempt.questions[1].body,
        QuestionBody::Match { .. }
    ));
    assert_eq!(
        (attempt.questions[1].id.as_str(), attempt.pass_mark),
        ("L0-A0-Q1", 2)
    );
    assert_eq!(bench.requests_with("You write the questions").len(), 2);
    // Premier tour : les quatre questions ; second : les deux choix.
    assert_eq!(bench.requests_with("careful student").len(), 6);
    let second = &bench.requests_with("You write the questions")[1];
    assert_eq!(
        second["format"]["required"],
        serde_json::json!(["q1", "q2"])
    );
    bench.idle().await;
}

#[tokio::test]
async fn slow_answers_and_model_errors_fail_with_their_reason() {
    let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
    let store = configured(&fake.url(), "", "chatonlm-wait");
    let ctx = Events::default().ctx(&store);
    let engine = StudyEngine::with_timeouts(super::request::Timeouts {
        json: Duration::from_millis(300),
        stream_idle: Duration::from_millis(300),
        retry_delay: Duration::from_millis(100),
        stream_bytes: super::request::TIMEOUTS.stream_bytes,
    });
    let requests_of = |model: &str| {
        fake.chat_requests()
            .iter()
            .filter(|request| request["model"] == model)
            .count()
    };
    let failure = |model: &'static str| {
        let (engine, ctx, store) = (&engine, ctx.clone(), store.clone());
        async move {
            set_models(&store, None, "", model);
            let id = engine
                .create(&ctx, &Staging::default(), new_course("Plants", 10))
                .await
                .unwrap();
            let started = std::time::Instant::now();
            loop {
                let snapshot = db_load(&store, &id).await;
                if snapshot.row.status == CourseStatus::Failed {
                    return snapshot.row.error.unwrap();
                }
                assert!(started.elapsed() < WAIT, "{model}");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    };
    assert_eq!(
        failure("chatonlm-wait").await,
        StudyError {
            code: StudyErrorCode::Failed,
            message: "Generation failed: the model took too long to answer".into(),
        }
    );
    // Délai dépassé : passager, la requête est retentée une fois, puis
    // l'erreur arrête le plan.
    assert_eq!(requests_of("chatonlm-wait"), 2);
    // Modèle supprimé entre `/api/show` et `/api/chat`.
    assert_eq!(
        failure("chatonlm-vanished").await,
        StudyError {
            code: StudyErrorCode::ModelNotFound,
            message: "The model chatonlm-vanished is not installed".into(),
        }
    );
    assert_eq!(
        failure("chatonlm-error").await,
        StudyError {
            code: StudyErrorCode::Failed,
            message: "Generation failed: model runner has unexpectedly stopped".into(),
        }
    );
    // Modèle absent, erreur du modèle : jamais retentés.
    assert_eq!(requests_of("chatonlm-vanished"), 1);
    assert_eq!(requests_of("chatonlm-error"), 1);
    // 404 d'un proxy : un échec, pas un modèle absent.
    let proxy = FakeOllama::start(Mode::Status(404)).await.unwrap();
    let store = configured(&proxy.url(), "", "gemma3:4b");
    let ctx = Events::default().ctx(&store);
    let id = engine
        .create(&ctx, &Staging::default(), new_course("Plants", 10))
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let error = loop {
        let snapshot = db_load(&store, &id).await;
        if let Some(error) = snapshot.row.error {
            break error;
        }
        assert!(started.elapsed() < WAIT);
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(
        error,
        StudyError {
            code: StudyErrorCode::Failed,
            message: "Generation failed: Ollama answered HTTP 404".into(),
        }
    );

    // Flux qui ne répond pas.
    let store = configured(&fake.url(), "", "chatonlm-study");
    let ctx = Events::default().ctx(&store);
    let id = engine
        .create(&ctx, &Staging::default(), new_course("Plants", 10))
        .await
        .unwrap();
    let started = std::time::Instant::now();
    while db_load(&store, &id)
        .await
        .levels
        .first()
        .is_none_or(|l| l.state != GenState::Ready)
    {
        assert!(started.elapsed() < WAIT);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    set_models(&store, None, "", "chatonlm-wait");
    let (events, received) = channel(None);
    engine
        .explain(&ctx, &id, 0, 0, "s".into(), &events)
        .await
        .unwrap();
    assert_eq!(
        received.lock().clone(),
        [
            serde_json::json!({"event": "error", "code": "failed", "message": "Generation failed: the model took too long to answer"})
        ]
    );
}

#[tokio::test]
async fn a_clarified_topic_is_searched_again_with_its_meaning() {
    let bench = Bench::new("chatonlm-study-unclear").await;
    bench.fake.set_signed_in(true);
    let mut request = new_course("Java", 10);
    request.web = true;
    let id = bench
        .engine
        .create(&bench.ctx, &Staging::default(), request)
        .await
        .unwrap();
    let course = bench
        .until(&id, |c| c.status == CourseStatus::Unclear)
        .await;
    assert_eq!(course.sources[0].title, "Result 1 for Java");
    bench
        .engine
        .clarify(&bench.ctx, &id, Some("The island".into()))
        .await
        .unwrap();
    let course = bench
        .until(&id, |c| c.status == CourseStatus::Planned)
        .await;
    let titles: Vec<&str> = course.sources.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Result 1 for Java The island",
            "Result 2 for Java The island"
        ]
    );
    let queries: Vec<Value> = bench
        .fake
        .web_requests()
        .iter()
        .map(|request| request["body"]["query"].clone())
        .collect();
    assert_eq!(queries, ["Java", "Java The island"]);
    bench.idle().await;
}

#[tokio::test]
async fn a_lesson_with_real_errors_is_written_again_once() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Wrong facts", 10).await;
    let course = bench.until(&id, level_ready(0)).await;
    let lesson = course.levels[0].lesson.as_ref().unwrap();
    assert!(
        lesson.pitfall.ends_with("(corrected)"),
        "{}",
        lesson.pitfall
    );
    // Une relecture (température 0, sans l'accroche), une réécriture, pas de
    // seconde relecture.
    let reviews = bench.requests_with("You are a careful fact-checker");
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0]["options"]["temperature"], 0.0);
    let reviewed = reviews[0]["messages"][1]["content"].as_str().unwrap();
    assert!(
        reviewed.starts_with("<lesson>\n## Part one\n") && !reviewed.contains("Wrong level 1?")
    );
    let lessons = bench.requests_with("Write level 1");
    assert_eq!(lessons.len(), 2);
    let system = lessons[1]["messages"][0]["content"].as_str().unwrap();
    // Seule la vraie erreur, en fin de consigne.
    assert!(
        system.ends_with(
            "\nA reviewer found these errors in a previous version; do not repeat them:\n\
             - \"$\\frac{1}{2}$ = 0,5\" → $\\frac{1}{2}$ = 0.5 with a decimal point"
        ),
        "{system}"
    );
    assert!(
        !lessons[0]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("A reviewer")
    );

    // Rien de réel trouvé : leçon gardée, pas de réécriture.
    let other = bench.create("Photosynthesis", 10).await;
    let course = bench.until(&other, level_ready(0)).await;
    assert!(
        !course.levels[0]
            .lesson
            .as_ref()
            .unwrap()
            .pitfall
            .contains("corrected")
    );
    assert_eq!(
        bench.requests_with("You are a careful fact-checker").len(),
        2
    );
    assert_eq!(bench.requests_with("Write level 1").len(), 3);
    bench.idle().await;
}

#[tokio::test]
async fn a_failed_review_keeps_the_lesson() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Unreviewed facts", 10).await;
    let course = bench.until(&id, level_ready(0)).await;
    assert!(course.levels[0].error.is_none());
    assert_eq!(course.levels[0].attempts[0].questions.len(), 3);
    // Erreur 500 : passagère, retentée une fois, puis relecture ignorée.
    assert_eq!(
        bench.requests_with("You are a careful fact-checker").len(),
        2
    );
    assert_eq!(bench.requests_with("Write level 1").len(), 1);
    bench.idle().await;
}

/// rust-7 : une réponse en flux trop longue s'arrête là (requête lâchée),
/// et sa requête borne la longueur écrite.
#[tokio::test]
async fn a_streamed_answer_is_bounded() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Photosynthesis", 10).await;
    bench.until(&id, level_ready(0)).await;
    bench.idle().await;
    set_models(&bench.store, None, "", "chatonlm-study-slow");
    let engine = StudyEngine::with_timeouts(super::request::Timeouts {
        stream_bytes: 30,
        ..super::request::TIMEOUTS
    });
    let (events, received) = channel(None);
    let started = std::time::Instant::now();
    engine
        .explain(&bench.ctx, &id, 0, 0, "s".into(), &events)
        .await
        .unwrap();
    // Le flux lent (4 caractères toutes les 150 ms) en aurait pour 6 s.
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    let last = received.lock().last().cloned().unwrap();
    assert_eq!(last["event"], "done");
    assert_eq!(last["stopped"], false);
    let text = last["text"].as_str().unwrap();
    assert!((30..40).contains(&text.len()), "{text:?}");
    let saved = db_load(&bench.store, &id).await.levels[0].explains[0]
        .text
        .clone();
    assert_eq!(saved, text);
    let request = bench
        .requests_with("Explain this section again")
        .pop()
        .unwrap();
    assert_eq!(request["options"]["num_predict"], 1024);
}

/// Flux lent lancé en tâche ; renvoie la tâche et ses évènements dès le
/// premier fragment reçu.
async fn slow_explain(
    engine: &Arc<StudyEngine>,
    ctx: &Ctx,
    id: &str,
) -> (
    tokio::task::JoinHandle<Result<(), AppError>>,
    Arc<Mutex<Vec<Value>>>,
) {
    let (events, received) = channel(None);
    let task = {
        let (engine, ctx, id) = (engine.clone(), ctx.clone(), id.to_owned());
        tokio::spawn(async move {
            engine
                .explain(&ctx, &id, 0, 0, "slow".into(), &events)
                .await
        })
    };
    let first = async {
        while !received.lock().iter().any(|e| e["event"] == "delta") {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(WAIT, first).await.unwrap();
    (task, received)
}

/// rust-10 : « Report a mistake » arrête les flux du palier ; une
/// explication de l'ancienne leçon n'est jamais enregistrée dans la
/// nouvelle.
#[tokio::test]
async fn a_regenerated_level_drops_the_streams_of_its_old_lesson() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Photosynthesis", 10).await;
    bench.until(&id, level_ready(0)).await;
    bench.idle().await;
    set_models(&bench.store, None, "", "chatonlm-study-slow");
    let engine = Arc::new(StudyEngine::default());
    let (task, received) = slow_explain(&engine, &bench.ctx, &id).await;
    let started = std::time::Instant::now();
    engine.regenerate(&bench.ctx, &id, 0).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    let last = received.lock().last().cloned().unwrap();
    assert_eq!(
        (last["event"].clone(), last["stopped"].clone()),
        ("done".into(), true.into())
    );
    set_models(&bench.store, None, "", "chatonlm-study");
    let course = bench.until(&id, level_ready(0)).await;
    assert!(course.levels[0].explains.is_empty());

    // Leçon changée sans arrêt du flux : réponse jetée quand même.
    set_models(&bench.store, None, "", "chatonlm-study-slow");
    let (task, received) = slow_explain(&engine, &bench.ctx, &id).await;
    let course = id.clone();
    bench
        .store
        .call(move |c| {
            db::update_level(c, &course, 0, |level| {
                level.lesson.as_mut().unwrap().hook = "Another hook?".into();
                Ok(())
            })
        })
        .await
        .unwrap();
    tokio::time::timeout(WAIT, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let last = received.lock().last().cloned().unwrap();
    assert_eq!(
        (last["event"].clone(), last["stopped"].clone()),
        ("done".into(), true.into())
    );
    assert!(
        db_load(&bench.store, &id).await.levels[0]
            .explains
            .is_empty()
    );
}

/// rust-14 : une partie restée « en cours » sans travail (travail fini sur
/// une erreur) est relancée à la lecture du cours, sans redémarrage.
#[tokio::test]
async fn a_part_left_generating_is_generated_again_when_read() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Photosynthesis", 10).await;
    bench.until(&id, level_ready(0)).await;
    bench.engine.start(&bench.ctx, &id).await.unwrap();
    bench.until(&id, final_ready).await;
    bench.idle().await;
    let course = id.clone();
    bench
        .store
        .call(move |c| {
            db::update_level(c, &course, 1, |level| {
                *level = stored::LevelData {
                    state: GenState::Generating,
                    ..stored::LevelData::default()
                };
                Ok(())
            })?;
            db::update_level(c, &course, 0, |level| {
                level.remedy = Some(GenState::Generating);
                Ok(())
            })?;
            db::update_row(c, &course, |row| {
                row.final_quiz.as_mut().unwrap().state = GenState::Generating;
                Ok(())
            })
        })
        .await
        .unwrap();
    // Même moteur, même session (pas de reprise au démarrage).
    let course = bench.engine.get(&bench.ctx, &id).await.unwrap();
    assert_eq!(course.levels[1].state, GenState::Pending);
    let course = bench
        .until(&id, |c| level_ready(1)(c) && c.levels[0].remedy.is_none())
        .await;
    assert!(course.levels[1].lesson.is_some());
    assert_eq!(course.levels[0].attempts.len(), 2);
    bench.idle().await;
}

/// Base qui refuse une écriture (déclencheur), pour vérifier qu'une
/// opération est toute ou rien.
async fn refuse(store: &Store, trigger: &'static str) {
    store
        .call(move |c| Ok(c.execute_batch(trigger)?))
        .await
        .unwrap();
}

/// rust-13 : réponse et fin du cours, palier remis à zéro et réponses
/// effacées, nouveau plan : chacun en une transaction.
#[tokio::test]
async fn related_writes_are_all_or_nothing() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Photosynthesis", 10).await;
    bench.until(&id, level_ready(0)).await;
    bench.engine.start(&bench.ctx, &id).await.unwrap();
    let course = bench.until(&id, final_ready).await;
    bench.idle().await;

    // Réponse : la mise à jour du cours échoue → réponse non gardée.
    refuse(&bench.store, "CREATE TRIGGER no_course BEFORE UPDATE ON courses BEGIN SELECT RAISE(ABORT, 'refused'); END;").await;
    let check = course.levels[0].check.clone().unwrap();
    assert!(bench.answer(&id, &check, true).await.is_err());
    refuse(&bench.store, "DROP TRIGGER no_course;").await;
    assert!(db_load(&bench.store, &id).await.answers.is_empty());

    // Palier refait : effacement des réponses refusé → palier gardé.
    bench.answer(&id, &check, true).await.unwrap();
    refuse(&bench.store, "CREATE TRIGGER no_delete BEFORE DELETE ON course_answers BEGIN SELECT RAISE(ABORT, 'refused'); END;").await;
    assert!(bench.engine.regenerate(&bench.ctx, &id, 0).await.is_err());
    refuse(&bench.store, "DROP TRIGGER no_delete;").await;
    let snapshot = db_load(&bench.store, &id).await;
    assert_eq!(snapshot.levels[0].state, GenState::Ready);
    assert!(snapshot.levels[0].lesson.is_some());
    assert_eq!(snapshot.answers.len(), 1);

    // Nouveau plan : paliers non effacés → plan gardé.
    let other = bench.create("Plants", 10).await;
    bench.until(&other, level_ready(0)).await;
    bench.idle().await;
    refuse(&bench.store, "CREATE TRIGGER no_levels BEFORE DELETE ON course_levels BEGIN SELECT RAISE(ABORT, 'refused'); END;").await;
    assert!(bench.engine.replan(&bench.ctx, &other).await.is_err());
    refuse(&bench.store, "DROP TRIGGER no_levels;").await;
    let snapshot = db_load(&bench.store, &other).await;
    assert_eq!(snapshot.row.status, CourseStatus::Planned);
    assert!(snapshot.row.plan.is_some());
    assert_eq!(snapshot.levels.len(), 2);

    // Bilan répondu mais cours resté « active » : la réponse redonnée le
    // finit.
    for level in &course.levels {
        for question in &level.attempts[0].questions {
            bench.answer(&id, question, true).await.unwrap();
        }
    }
    let final_questions = course.final_quiz.clone().unwrap().questions;
    for question in &final_questions {
        bench.answer(&id, question, true).await.unwrap();
    }
    let course_id = id.clone();
    bench
        .store
        .call(move |c| {
            db::update_row(c, &course_id, |row| {
                row.status = CourseStatus::Active;
                row.finished_at = None;
                Ok(())
            })
        })
        .await
        .unwrap();
    bench.answer(&id, &final_questions[0], false).await.unwrap();
    assert_eq!(bench.course(&id).await.status, CourseStatus::Finished);
}

/// rust-3 : dans un cours de langue (sorte du plan), une faute de frappe
/// dans un trou est fausse (mais proche) ; ailleurs, la variante proche est
/// acceptée.
#[tokio::test]
async fn a_language_course_grades_blanks_strictly() {
    let bench = Bench::new("chatonlm-study").await;
    for (topic, kind, expected) in [
        ("language basics", StudyKind::Language, (false, true)),
        ("math basics", StudyKind::Math, (true, false)),
    ] {
        let id = bench.create(topic, 10).await;
        let course = bench.until(&id, level_ready(0)).await;
        bench.engine.start(&bench.ctx, &id).await.unwrap();
        let blank = course.levels[0]
            .check
            .iter()
            .chain(&course.levels[0].attempts[0].questions)
            .find(|q| matches!(q.body, QuestionBody::Blank { .. }))
            .cloned()
            .unwrap();
        assert_eq!(course.plan.as_ref().unwrap().kind, kind);
        let answered = bench
            .engine
            .answer(
                &bench.ctx,
                &id,
                &blank.id,
                AnswerValue::Blank {
                    text: "rigth".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!((answered.correct, answered.close), expected, "{topic}");
        assert_eq!(
            answered.expected,
            AnswerValue::Blank {
                text: "right".into()
            }
        );
    }
    bench.idle().await;
}

/// Emballement (qwen3:0.6b au contrat) : un texte qui atteint sa limite
/// rend la partie inutilisable, comme un JSON illisible : un nouvel essai,
/// puis `invalid` ; chaque chaîne du schéma envoyé est bornée.
#[tokio::test]
async fn a_runaway_answer_is_unusable() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Runaway topic", 10).await;
    let course = bench.until(&id, |c| c.status == CourseStatus::Failed).await;
    assert_eq!(course.error.unwrap().code, StudyErrorCode::Invalid);
    let plans = bench.requests_with("Design the plan");
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0]["format"]["properties"]["scope"]["maxLength"], 500);
    bench.idle().await;
}

/// Erreur passagère d'Ollama (500 au milieu d'une génération) : la même
/// requête est retentée une fois, après une courte pause ; deux 500 de
/// suite : l'erreur, comme avant.
#[tokio::test]
async fn a_transient_server_error_is_retried_once() {
    let bench = Bench::new("chatonlm-study").await;
    let engine = StudyEngine::with_timeouts(super::request::Timeouts {
        retry_delay: Duration::from_millis(300),
        ..super::request::TIMEOUTS
    });
    let started = std::time::Instant::now();
    let id = engine
        .create(
            &bench.ctx,
            &Staging::default(),
            new_course("Hiccup topic", 10),
        )
        .await
        .unwrap();
    let course = bench
        .until(&id, |c| c.status == CourseStatus::Planned)
        .await;
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert_eq!(course.plan.unwrap().title, "Hiccup topic");
    let plans = bench.requests_with("Design the plan");
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0], plans[1]);

    // Toujours 500 : deux tentatives, puis l'erreur (un moteur sert une
    // seule base : file vidée avant d'en changer).
    while !engine.idle() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let broken = FakeOllama::start(Mode::Status(500)).await.unwrap();
    let store = configured(&broken.url(), "", "gemma3:4b");
    let ctx = Events::default().ctx(&store);
    let started = std::time::Instant::now();
    let id = engine
        .create(&ctx, &Staging::default(), new_course("Plants", 10))
        .await
        .unwrap();
    let error = loop {
        let snapshot = db_load(&store, &id).await;
        if let Some(error) = snapshot.row.error {
            break error;
        }
        assert!(started.elapsed() < WAIT);
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(error.message, "Generation failed: Ollama answered HTTP 500");
    assert!(started.elapsed() >= Duration::from_millis(300));
    while !engine.idle() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Questions écartées partout : une dernière demande de questions à choix
/// seulement sauve le palier, le nouvel essai et le bilan ; l'entraînement,
/// facultatif, n'a pas de demande de plus.
#[tokio::test]
async fn questions_get_a_last_chance_with_multiple_choice() {
    let bench = Bench::new("chatonlm-study").await;
    let id = bench.create("Fragile facts", 10).await;
    let course = bench.until(&id, level_ready(0)).await;
    let level = &course.levels[0];
    assert!(level.check.is_none());
    let questions = &level.attempts[0].questions;
    assert_eq!(questions.len(), 3);
    assert!(
        questions
            .iter()
            .all(|q| matches!(q.body, QuestionBody::Choice { .. }))
    );
    let quizzes = bench.requests_with("You write the questions");
    assert_eq!(quizzes.len(), 3);
    let last = &quizzes[2]["format"]["properties"];
    assert_eq!(last.as_object().unwrap().len(), 3);
    assert!(last["q1"]["properties"]["options"].is_object());
    // Bilan et nouvel essai : même dernière chance.
    bench.engine.start(&bench.ctx, &id).await.unwrap();
    let course = bench.until(&id, final_ready).await;
    let final_quiz = course.final_quiz.unwrap();
    assert_eq!(final_quiz.questions.len(), 3);
    for question in &course.levels[0].attempts[0].questions {
        bench.answer(&id, question, false).await.unwrap();
    }
    bench.engine.remedy(&bench.ctx, &id, 0).await.unwrap();
    let course = bench
        .until(&id, |c| {
            c.levels[0].attempts.len() == 2 && c.levels[0].remedy.is_none()
        })
        .await;
    assert_eq!(course.levels[0].attempts[1].questions.len(), 3);
    assert!(course.levels[0].attempts[1].explanation.is_some());
    bench.idle().await;
}

/// Modèle cloud pour les cours : réglage coupé, refusé sans rien envoyer ;
/// activé sans compte, le 401 du serveur est dit comme un compte manquant.
#[tokio::test]
async fn a_cloud_model_writes_courses_only_once_turned_on() {
    let bench = Bench::new("kimi-k2.6:cloud").await;
    let id = bench.create("Photosynthesis", 10).await;
    let failed = |store: Store, id: String| async move {
        let started = std::time::Instant::now();
        loop {
            let snapshot = db_load(&store, &id).await;
            if snapshot.row.status == CourseStatus::Failed {
                return snapshot.row.error.unwrap();
            }
            assert!(started.elapsed() < WAIT);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let error = failed(bench.store.clone(), id.clone()).await;
    assert_eq!(
        error,
        StudyError {
            code: StudyErrorCode::Failed,
            message: "Cloud models are turned off: kimi-k2.6:cloud runs on ollama.com. Turn them on in Settings › Models to use it.".into(),
        }
    );
    assert!(bench.fake.chat_requests().is_empty());
    assert!(bench.fake.show_requests().is_empty());
    bench.idle().await;
    let on = SettingsPatch {
        cloud_models: Some(true),
        ..Default::default()
    };
    bench
        .store
        .blocking(|c| settings::update(c, on, None).map(drop))
        .unwrap();
    bench.engine.retry(&bench.ctx, &id, false).await.unwrap();
    let started = std::time::Instant::now();
    let error = loop {
        let snapshot = db_load(&bench.store, &id).await;
        if snapshot.row.status == CourseStatus::Failed
            && let Some(error) = snapshot.row.error
            && error.message.contains("account")
        {
            break error;
        }
        assert!(started.elapsed() < WAIT);
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(error.code, StudyErrorCode::Failed);
    assert_eq!(error.message, "Cloud models require an Ollama account");
    assert!(!bench.fake.chat_requests().is_empty());
}
