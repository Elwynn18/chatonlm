//! Génération des parties d'un cours (plan, palier, nouvel essai, bilan) :
//! requêtes, garde-fous, nouvel essai d'une réponse inutilisable,
//! auto-vérification des questions, enregistrement.
//!
//! Chaque écriture vérifie, sous le verrou de la base, que le travail n'a
//! pas été arrêté (cours supprimé, plan refait, palier refait) et que la
//! partie est toujours celle attendue : un travail arrêté n'écrit rien.

use super::checks::{self, Draft, LessonVerdict, PlanVerdict};
use super::data::{
    self, ClarifyState, FinalData, LevelData, QType, Snapshot, StoredAttempt, StoredQuestion,
};
use super::prompts::{self, CHECK_TEMPERATURE, TEMPERATURE};
use super::request::{Gen, Stop, Timeouts, error};
use super::sources::{self, WebOutcome};
use super::{CourseStatus, GenState, Lesson, StudyError, StudyErrorCode, StudyKind};
use crate::error::AppResult;
use crate::i18n::{Lang, t};
use crate::proofread::Cancel;
use crate::store::{Store, study as db};

/// Partie d'un cours à générer.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Plan,
    Level(usize),
    /// Nouvel essai (« Explain differently ») du palier.
    Remedy(usize),
    Final,
}

/// Ce que le travail demande ensuite : parties à mettre en file (la file
/// vérifie qu'elles sont encore attendues).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Next {
    pub parts: Vec<Part>,
}

/// Contexte d'un travail.
pub struct Job<'a> {
    pub store: &'a Store,
    pub course: &'a str,
    pub cancel: &'a Cancel,
    pub timeouts: Timeouts,
    /// Émet `study-updated` pour ce cours.
    pub changed: &'a (dyn Fn() + Send + Sync),
}

fn invalid() -> StudyError {
    error(StudyErrorCode::Invalid, t!("study.invalid"))
}

/// Recherche web d'un plan : requête = sujet (et sens précisé), sinon noms
/// des documents.
fn web_query(snapshot: &Snapshot) -> String {
    let topic = snapshot.row.topic.trim();
    let meaning = snapshot
        .row
        .clarify
        .as_ref()
        .and_then(|clarify| clarify.meaning.as_deref())
        .map(str::trim)
        .filter(|meaning| !meaning.is_empty());
    match meaning {
        _ if topic.is_empty() => data::document_names(snapshot),
        Some(meaning) => format!("{topic} {meaning}"),
        None => topic.to_owned(),
    }
}

/// Début du premier document donné au plan avant les morceaux proches.
const PLAN_LEAD: usize = sources::SOURCE_CHARS / 3;

impl Job<'_> {
    async fn load(&self) -> AppResult<Option<Snapshot>> {
        let course = self.course.to_owned();
        self.store.call(move |c| db::load_full(c, &course)).await
    }

    /// Change la ligne du cours si le travail n'est pas arrêté et que
    /// `change` l'accepte ; émet `study-updated` si c'est fait.
    async fn write_row(
        &self,
        change: impl FnOnce(&mut data::CourseRow) -> bool + Send + 'static,
    ) -> AppResult<bool> {
        let (course, cancel) = (self.course.to_owned(), self.cancel.clone());
        let written = self
            .store
            .call(move |c| {
                if cancel.is_cancelled() {
                    return Ok(false);
                }
                // Ligne relue sous le verrou ; rien d'écrit si refusé.
                let Some(mut row) = db::row(c, &course)? else {
                    return Ok(false);
                };
                if !change(&mut row) {
                    return Ok(false);
                }
                db::update_row(c, &course, move |current| {
                    *current = row;
                    Ok(())
                })?;
                Ok(true)
            })
            .await?;
        if written {
            (self.changed)();
        }
        Ok(written)
    }

    /// Change un palier si le travail n'est pas arrêté et que `change`
    /// l'accepte ; émet `study-updated` si c'est fait.
    async fn write_level(
        &self,
        index: usize,
        change: impl FnOnce(&mut LevelData) -> bool + Send + 'static,
    ) -> AppResult<bool> {
        let (course, cancel) = (self.course.to_owned(), self.cancel.clone());
        let written = self
            .store
            .call(move |c| {
                if cancel.is_cancelled() {
                    return Ok(false);
                }
                let Some(mut level) = db::level(c, &course, index)? else {
                    return Ok(false);
                };
                if !change(&mut level) {
                    return Ok(false);
                }
                db::update_level(c, &course, index, move |current| {
                    *current = level;
                    Ok(())
                })?;
                Ok(true)
            })
            .await?;
        if written {
            (self.changed)();
        }
        Ok(written)
    }

    // -------------------------------------------------------------- plan

    pub async fn plan(&self) -> AppResult<Next> {
        let Some(snapshot) = self.load().await? else {
            return Ok(Next::default());
        };
        if snapshot.row.status != CourseStatus::Planning {
            return Ok(Next::default());
        }
        let ollama = match Gen::connect(self.store, self.cancel, self.timeouts).await {
            Ok(ollama) => ollama,
            Err(stop) => return self.plan_stopped(stop).await,
        };
        let mut sources = snapshot.sources.clone();
        let has_web = sources
            .iter()
            .any(|source| source.source.kind == super::SourceKind::Web);
        if snapshot.row.web && !has_web {
            let query = web_query(&snapshot);
            let outcome = tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Ok(Next::default()),
                outcome = sources::web_search(&ollama.server.url, &query) => outcome,
            };
            match outcome {
                WebOutcome::Found(rows) => {
                    let (course, cancel, found) =
                        (self.course.to_owned(), self.cancel.clone(), rows.clone());
                    let added = self
                        .store
                        .call(move |c| {
                            if cancel.is_cancelled() || db::row(c, &course)?.is_none() {
                                return Ok(false);
                            }
                            db::add_sources(c, &course, &found)?;
                            Ok(true)
                        })
                        .await?;
                    if !added {
                        return Ok(Next::default());
                    }
                    (self.changed)();
                    sources.extend(rows);
                }
                WebOutcome::SignedOut => {
                    let error = error(StudyErrorCode::SignedOut, t!("study.signedOut"));
                    return self.plan_stopped(Stop::Failed(error)).await;
                }
                WebOutcome::Failed(reason) => {
                    // i18n-ignore : journal.
                    eprintln!("chatonlm: study: web search failed, planning without it: {reason}");
                }
            }
        }
        let row = &snapshot.row;
        let count = data::level_count(row.minutes);
        let extracts = sources::extracts(&sources, &web_query(&snapshot), PLAN_LEAD);
        let system = prompts::with_sources(
            prompts::plan_prompt(row.language, row.level, row.minutes, count),
            &extracts,
        );
        let clarify = row.clarify.clone().unwrap_or_default();
        let user = prompts::plan_user(&row.topic, clarify.meaning.as_deref());
        let which = t!("study.whichMeaning" in row.language);
        let mut verdict = PlanVerdict::Invalid("no answer");
        for _ in 0..2 {
            let answer = match ollama
                .ask_json(&system, &user, &prompts::plan_schema(count), TEMPERATURE)
                .await
            {
                Ok(answer) => answer,
                Err(stop) => return self.plan_stopped(stop).await,
            };
            verdict = match answer {
                Some(value) => {
                    checks::check_plan(&value, count, row.language, !clarify.answered, &which)
                }
                None => PlanVerdict::Invalid("unreadable answer"),
            };
            if !matches!(verdict, PlanVerdict::Invalid(_)) {
                break;
            }
        }
        let model = ollama.model.clone();
        match verdict {
            PlanVerdict::Unclear(question) => {
                self.write_row(move |row| {
                    if row.status != CourseStatus::Planning {
                        return false;
                    }
                    row.status = CourseStatus::Unclear;
                    row.clarify = Some(ClarifyState {
                        question: question.question,
                        options: question.options,
                        answered: false,
                        meaning: None,
                    });
                    row.error = None;
                    true
                })
                .await?;
                Ok(Next::default())
            }
            PlanVerdict::Ready(plan) => {
                let (course, cancel) = (self.course.to_owned(), self.cancel.clone());
                let levels = vec![LevelData::default(); plan.levels.len()];
                let written = self
                    .store
                    // Plan et paliers ensemble.
                    .call(move |c| {
                        db::atomic(c, |c| {
                            if cancel.is_cancelled() {
                                return Ok(false);
                            }
                            let ready = db::update_row(c, &course, |row| {
                                if row.status != CourseStatus::Planning {
                                    return Ok(false);
                                }
                                row.status = CourseStatus::Planned;
                                row.plan = Some(plan);
                                row.model = model;
                                row.error = None;
                                row.final_quiz = Some(FinalData::default());
                                Ok(true)
                            })?;
                            if ready != Some(true) {
                                return Ok(false);
                            }
                            db::put_levels(c, &course, &levels)?;
                            Ok(true)
                        })
                    })
                    .await?;
                if !written {
                    return Ok(Next::default());
                }
                (self.changed)();
                Ok(Next {
                    parts: vec![Part::Level(0)],
                })
            }
            PlanVerdict::Invalid(reason) => {
                // i18n-ignore : journal.
                eprintln!("chatonlm: study: unusable plan: {reason}");
                self.plan_stopped(Stop::Failed(invalid())).await
            }
        }
    }

    async fn plan_stopped(&self, stop: Stop) -> AppResult<Next> {
        if let Stop::Failed(error) = stop {
            self.write_row(move |row| {
                if row.status != CourseStatus::Planning {
                    return false;
                }
                row.status = CourseStatus::Failed;
                row.error = Some(error);
                true
            })
            .await?;
        }
        Ok(Next::default())
    }

    // -------------------------------------------------------------- palier

    pub async fn level(&self, index: usize) -> AppResult<Next> {
        let Some(snapshot) = self.load().await? else {
            return Ok(Next::default());
        };
        let Some(plan) = snapshot.row.plan.clone() else {
            return Ok(Next::default());
        };
        let wanted = match snapshot.row.status {
            CourseStatus::Planned => index == 0,
            CourseStatus::Active | CourseStatus::Finished => true,
            _ => false,
        };
        let Some(level) = snapshot.levels.get(index) else {
            return Ok(Next::default());
        };
        if !wanted || level.state != GenState::Pending || index >= plan.levels.len() {
            return Ok(Next::default());
        }
        let started = self
            .write_level(index, |level| {
                if level.state != GenState::Pending {
                    return false;
                }
                level.state = GenState::Generating;
                level.error = None;
                true
            })
            .await?;
        if !started {
            return Ok(Next::default());
        }
        let ollama = match Gen::connect(self.store, self.cancel, self.timeouts).await {
            Ok(ollama) => ollama,
            Err(stop) => return self.level_stopped(index, stop).await,
        };
        let lang = snapshot.row.language;
        let lesson = match level.lesson.clone() {
            Some(lesson) => lesson,
            None => {
                let query = {
                    let current = &plan.levels[index];
                    format!(
                        "{} {} {}",
                        current.title,
                        current.goal,
                        current.points.join(" ")
                    )
                };
                let extracts = sources::extracts(&snapshot.sources, &query, 0);
                let hooks: Vec<&str> = snapshot.levels[..index]
                    .iter()
                    .filter_map(|level| level.lesson.as_ref())
                    .map(|lesson| lesson.hook.as_str())
                    .collect();
                let system = prompts::with_sources(
                    prompts::lesson_prompt(&plan, snapshot.row.level, lang, index, &hooks),
                    &extracts,
                );
                let user = prompts::lesson_user(&plan, index);
                let lesson = match self.lesson(&ollama, &system, &user, lang, plan.kind).await {
                    Ok(Some(lesson)) => lesson,
                    Ok(None) => return self.level_stopped(index, Stop::Failed(invalid())).await,
                    Err(stop) => return self.level_stopped(index, stop).await,
                };
                let review = Review {
                    system: &system,
                    user: &user,
                    lang,
                    level: snapshot.row.level,
                    kind: plan.kind,
                };
                let lesson = match self.reviewed(&ollama, &review, lesson).await {
                    Ok(lesson) => lesson,
                    Err(stop) => return self.level_stopped(index, stop).await,
                };
                let saved = lesson.clone();
                let written = self
                    .write_level(index, move |level| {
                        if level.state != GenState::Generating {
                            return false;
                        }
                        level.lesson = Some(saved);
                        true
                    })
                    .await?;
                if !written {
                    return Ok(Next::default());
                }
                lesson
            }
        };
        let types = prompts::mix(plan.kind, index);
        let prefix = format!("L{index}");
        let generated = match self
            .questions(
                &ollama,
                lang,
                plan.kind,
                &types,
                1,
                &prefix,
                |types| prompts::quiz_prompt(&lesson, lang, types),
                prompts::QUIZ_USER,
                false,
            )
            .await
        {
            Ok(generated) => generated,
            Err(stop) => return self.level_stopped(index, stop).await,
        };
        let check = generated.slots[0].clone().map(|(draft, explanation)| {
            checks::finalize(draft, self.course, &format!("L{index}-check"), explanation)
        });
        let questions = numbered(&generated.slots[1..], self.course, &format!("L{index}-A0"));
        if questions.len() < 2 {
            return self.level_stopped(index, Stop::Failed(invalid())).await;
        }
        let attempt = StoredAttempt {
            explanation: None,
            pass_mark: data::pass_mark(questions.len()),
            questions,
        };
        let written = self
            .write_level(index, move |level| {
                if level.state != GenState::Generating {
                    return false;
                }
                level.state = GenState::Ready;
                level.error = None;
                level.check = check;
                level.attempts = vec![attempt];
                true
            })
            .await?;
        if !written {
            return Ok(Next::default());
        }
        Ok(Next {
            parts: vec![Part::Final],
        })
    }

    async fn level_stopped(&self, index: usize, stop: Stop) -> AppResult<Next> {
        if let Stop::Failed(error) = stop {
            self.write_level(index, move |level| {
                if level.state != GenState::Generating {
                    return false;
                }
                level.state = GenState::Failed;
                level.error = Some(error);
                true
            })
            .await?;
        }
        Ok(Next::default())
    }

    /// Leçon : un nouvel essai si la réponse est inutilisable ou de
    /// mauvaise longueur ; une leçon de mauvaise longueur mais valide par
    /// ailleurs est gardée au second essai. `None` : rien d'utilisable.
    async fn lesson(
        &self,
        ollama: &Gen<'_>,
        system: &str,
        user: &str,
        lang: Lang,
        kind: StudyKind,
    ) -> Result<Option<Lesson>, Stop> {
        let mut fallback = None;
        for _ in 0..2 {
            let answer = ollama
                .ask_json(system, user, &prompts::lesson_schema(), TEMPERATURE)
                .await?;
            match answer.map(|value| checks::check_lesson(&value, lang, kind)) {
                Some(LessonVerdict::Ready(lesson)) => return Ok(Some(lesson)),
                Some(LessonVerdict::Length(lesson)) => fallback = Some(lesson),
                Some(LessonVerdict::Invalid(reason)) => {
                    // i18n-ignore : journal.
                    eprintln!("chatonlm: study: unusable lesson: {reason}");
                }
                None => {}
            }
        }
        Ok(fallback)
    }

    /// Relecture de la leçon (température 0) : erreurs réelles relevées (voir
    /// `checks::review_errors`) → leçon réécrite une fois, ces erreurs en fin
    /// de consigne ; la seconde version est gardée si elle est valide, sans
    /// nouvelle relecture. Relecture ou réécriture en échec (réseau, réponse
    /// illisible) : la leçon est gardée ; seul l'arrêt est rendu.
    async fn reviewed(
        &self,
        ollama: &Gen<'_>,
        review: &Review<'_>,
        lesson: Lesson,
    ) -> Result<Lesson, Stop> {
        let answer = ollama
            .ask_json(
                &prompts::review_prompt(review.lang, review.level),
                &prompts::review_user(&lesson),
                &prompts::review_schema(),
                CHECK_TEMPERATURE,
            )
            .await;
        let errors = match answer {
            Ok(Some(value)) => checks::review_errors(&value, &prompts::lesson_text(&lesson)),
            Ok(None) | Err(Stop::Failed(_)) => return Ok(lesson),
            Err(Stop::Cancelled) => return Err(Stop::Cancelled),
        };
        if errors.is_empty() {
            return Ok(lesson);
        }
        let system = format!("{}\n{}", review.system, prompts::review_notes(&errors));
        let rewritten = ollama
            .ask_json(&system, review.user, &prompts::lesson_schema(), TEMPERATURE)
            .await;
        match rewritten {
            Ok(Some(value)) => match checks::check_lesson(&value, review.lang, review.kind) {
                LessonVerdict::Ready(lesson) | LessonVerdict::Length(lesson) => Ok(lesson),
                LessonVerdict::Invalid(_) => Ok(lesson),
            },
            Ok(None) | Err(Stop::Failed(_)) => Ok(lesson),
            Err(Stop::Cancelled) => Err(Stop::Cancelled),
        }
    }

    /// Questions des types `types` (une demande, puis une autre pour
    /// compléter celles écartées) ; chacune passe le garde-fou puis
    /// l'auto-vérification. S'il manque encore des questions de validation
    /// (moins de [`MIN_QUESTIONS`] après les `optional` premières, facultatives :
    /// l'entraînement) ou l'explication d'un nouvel essai, une dernière demande
    /// de questions à choix seulement (85 à 95 % d'accord au banc). Chaque
    /// question écartée est notée au journal. `prompt` : consignes pour une
    /// liste de types.
    #[allow(clippy::too_many_arguments)]
    async fn questions(
        &self,
        ollama: &Gen<'_>,
        lang: Lang,
        kind: StudyKind,
        types: &[QType],
        optional: usize,
        prefix: &str,
        prompt: impl Fn(&[QType]) -> String + Sync,
        user: &str,
        with_explanation: bool,
    ) -> Result<Generated, Stop> {
        let mut generated = Generated {
            explanation: None,
            slots: vec![None; types.len()],
        };
        let request = Request {
            ollama,
            lang,
            kind,
            prefix,
            prompt: &prompt,
            user,
            with_explanation,
        };
        for round in 0..2 {
            let missing: Vec<usize> = (0..types.len())
                .filter(|slot| generated.slots[*slot].is_none())
                .collect();
            if missing.is_empty() && !(with_explanation && generated.explanation.is_none()) {
                break;
            }
            let asked: Vec<QType> = missing.iter().map(|slot| types[*slot]).collect();
            self.ask_slots(&request, &mut generated, &missing, &asked, round)
                .await?;
        }
        let missing: Vec<usize> = (optional..types.len())
            .filter(|slot| generated.slots[*slot].is_none())
            .collect();
        let kept = types.len() - optional - missing.len();
        if kept < MIN_QUESTIONS || (with_explanation && generated.explanation.is_none()) {
            // i18n-ignore : journal.
            eprintln!(
                "chatonlm: study: {prefix}: {kept} question(s) kept, last chance with multiple choice"
            );
            let asked = vec![QType::Choice; missing.len()];
            self.ask_slots(&request, &mut generated, &missing, &asked, 2)
                .await?;
        }
        Ok(generated)
    }

    /// Une demande de questions pour les emplacements `missing` (types
    /// `asked`, dans l'ordre) : questions gardées, explication d'un nouvel
    /// essai si elle manquait.
    async fn ask_slots(
        &self,
        request: &Request<'_, '_>,
        generated: &mut Generated,
        missing: &[usize],
        asked: &[QType],
        round: usize,
    ) -> Result<(), Stop> {
        let Request {
            ollama,
            lang,
            kind,
            prefix,
            ..
        } = *request;
        let schema = prompts::quiz_schema(asked, request.with_explanation);
        let Some(value) = ollama
            .ask_json(&(request.prompt)(asked), request.user, &schema, TEMPERATURE)
            .await?
        else {
            return Ok(());
        };
        if request.with_explanation && generated.explanation.is_none() {
            let explanation = value["explanation"].as_str().unwrap_or_default().trim();
            if !explanation.is_empty() && checks::language_ok(explanation, lang, kind, true) {
                generated.explanation = Some(super::repair::inline_math(explanation));
            }
        }
        for (position, (slot, qtype)) in missing.iter().zip(asked).enumerate() {
            let item = &value[format!("q{}", position + 1)];
            let draft = match checks::question_verdict(*qtype, item, lang, kind) {
                Ok(draft) => draft,
                Err(reason) => {
                    dropped(prefix, *qtype, &format!("guard: {reason}"), item);
                    continue;
                }
            };
            let explanation = checks::explanation(item);
            let provisional = format!("{prefix}-r{round}-s{slot}");
            let candidate = checks::finalize(
                draft.clone(),
                self.course,
                &provisional,
                explanation.clone(),
            );
            match self.verified(ollama, lang, kind, &candidate).await? {
                Ok(()) => generated.slots[*slot] = Some((draft, explanation)),
                Err(detail) => dropped(prefix, *qtype, &format!("check: {detail}"), item),
            }
        }
        Ok(())
    }

    /// Auto-vérification (température 0) : le modèle répond à la question
    /// sans la solution, ou juge chaque paire d'une association ; un
    /// désaccord l'écarte (`Err` : ce qu'il a répondu, pour le journal).
    async fn verified(
        &self,
        ollama: &Gen<'_>,
        lang: Lang,
        kind: StudyKind,
        question: &StoredQuestion,
    ) -> Result<Result<(), String>, Stop> {
        let (text, schema) = prompts::check_request(question);
        let reply = ollama
            .ask_json(
                &prompts::check_prompt(lang),
                &text,
                &schema,
                CHECK_TEMPERATURE,
            )
            .await?;
        let strict = kind == StudyKind::Language;
        Ok(match reply {
            Some(reply) if checks::agrees(question, &reply, strict) => Ok(()),
            Some(reply) => Err(format!(
                "checker answered {}, solution {:?}",
                short(&reply.to_string()),
                question.expected
            )),
            None => Err("no usable answer from the checker".to_owned()),
        })
    }

    // -------------------------------------------------------------- nouvel essai

    pub async fn remedy(&self, index: usize) -> AppResult<Next> {
        let Some(snapshot) = self.load().await? else {
            return Ok(Next::default());
        };
        let (Some(plan), Some(level)) = (
            snapshot.row.plan.clone(),
            snapshot.levels.get(index).cloned(),
        ) else {
            return Ok(Next::default());
        };
        let Some(lesson) = level.lesson.clone() else {
            return Ok(Next::default());
        };
        if level.remedy != Some(GenState::Pending) {
            return Ok(Next::default());
        }
        let started = self
            .write_level(index, |level| {
                if level.remedy != Some(GenState::Pending) {
                    return false;
                }
                level.remedy = Some(GenState::Generating);
                level.remedy_error = None;
                true
            })
            .await?;
        if !started {
            return Ok(Next::default());
        }
        let ollama = match Gen::connect(self.store, self.cancel, self.timeouts).await {
            Ok(ollama) => ollama,
            Err(stop) => return self.remedy_stopped(index, stop).await,
        };
        let lang = snapshot.row.language;
        // Questions manquées du dernier essai : réponse donnée, attendue.
        let missed: Vec<(&StoredQuestion, &super::AnswerValue)> = level
            .attempts
            .last()
            .into_iter()
            .flat_map(|attempt| &attempt.questions)
            .filter_map(|question| {
                snapshot
                    .answers
                    .get(&question.id)
                    .filter(|answered| !answered.correct)
                    .map(|answered| (question, &answered.given))
            })
            .collect();
        let missed = prompts::missed_text(&missed);
        let types: Vec<QType> = prompts::mix(plan.kind, index)[1..].to_vec();
        let attempt_index = level.attempts.len();
        let prefix = format!("L{index}-A{attempt_index}");
        let generated = match self
            .questions(
                &ollama,
                lang,
                plan.kind,
                &types,
                0,
                &prefix,
                |types| prompts::remedy_prompt(&lesson, lang, &missed, types),
                prompts::REMEDY_USER,
                true,
            )
            .await
        {
            Ok(generated) => generated,
            Err(stop) => return self.remedy_stopped(index, stop).await,
        };
        let questions = numbered(&generated.slots, self.course, &prefix);
        let Some(explanation) = generated.explanation.filter(|_| questions.len() >= 2) else {
            return self.remedy_stopped(index, Stop::Failed(invalid())).await;
        };
        let attempt = StoredAttempt {
            explanation: Some(explanation),
            pass_mark: data::pass_mark(questions.len()),
            questions,
        };
        self.write_level(index, move |level| {
            if level.remedy != Some(GenState::Generating) || level.attempts.len() != attempt_index {
                return false;
            }
            level.attempts.push(attempt);
            level.remedy = None;
            level.remedy_error = None;
            true
        })
        .await?;
        Ok(Next::default())
    }

    async fn remedy_stopped(&self, index: usize, stop: Stop) -> AppResult<Next> {
        if let Stop::Failed(error) = stop {
            self.write_level(index, move |level| {
                if level.remedy != Some(GenState::Generating) {
                    return false;
                }
                level.remedy = Some(GenState::Failed);
                level.remedy_error = Some(error);
                true
            })
            .await?;
        }
        Ok(Next::default())
    }

    // -------------------------------------------------------------- bilan

    pub async fn final_quiz(&self) -> AppResult<Next> {
        let Some(snapshot) = self.load().await? else {
            return Ok(Next::default());
        };
        let Some(plan) = snapshot.row.plan.clone() else {
            return Ok(Next::default());
        };
        if !final_wanted(&snapshot) {
            return Ok(Next::default());
        }
        let started = self
            .write_row(|row| match &mut row.final_quiz {
                Some(final_quiz) if final_quiz.state == GenState::Pending => {
                    final_quiz.state = GenState::Generating;
                    final_quiz.error = None;
                    true
                }
                _ => false,
            })
            .await?;
        if !started {
            return Ok(Next::default());
        }
        let ollama = match Gen::connect(self.store, self.cancel, self.timeouts).await {
            Ok(ollama) => ollama,
            Err(stop) => return self.final_stopped(stop).await,
        };
        let lang = snapshot.row.language;
        let lessons: Vec<Option<&Lesson>> = snapshot
            .levels
            .iter()
            .map(|level| level.lesson.as_ref())
            .collect();
        let types = &prompts::FINAL_TYPES[..data::final_count(snapshot.row.minutes)];
        let generated = match self
            .questions(
                &ollama,
                lang,
                plan.kind,
                types,
                0,
                "F",
                |types| prompts::final_prompt(&plan, &lessons, lang, types),
                prompts::QUIZ_USER,
                false,
            )
            .await
        {
            Ok(generated) => generated,
            Err(stop) => return self.final_stopped(stop).await,
        };
        let questions = numbered(&generated.slots, self.course, "F");
        if questions.len() < 2 {
            return self.final_stopped(Stop::Failed(invalid())).await;
        }
        self.write_row(move |row| match &mut row.final_quiz {
            Some(final_quiz) if final_quiz.state == GenState::Generating => {
                final_quiz.state = GenState::Ready;
                final_quiz.error = None;
                final_quiz.questions = questions;
                true
            }
            _ => false,
        })
        .await?;
        Ok(Next::default())
    }

    async fn final_stopped(&self, stop: Stop) -> AppResult<Next> {
        if let Stop::Failed(error) = stop {
            self.write_row(move |row| match &mut row.final_quiz {
                Some(final_quiz) if final_quiz.state == GenState::Generating => {
                    final_quiz.state = GenState::Failed;
                    final_quiz.error = Some(error);
                    true
                }
                _ => false,
            })
            .await?;
        }
        Ok(Next::default())
    }
}

/// Le bilan peut être généré : cours commencé, tous les paliers prêts, bilan
/// en attente.
pub fn final_wanted(snapshot: &Snapshot) -> bool {
    matches!(
        snapshot.row.status,
        CourseStatus::Active | CourseStatus::Finished
    ) && !snapshot.levels.is_empty()
        && snapshot
            .levels
            .iter()
            .all(|level| level.state == GenState::Ready && level.lesson.is_some())
        && snapshot
            .row
            .final_quiz
            .as_ref()
            .is_some_and(|final_quiz| final_quiz.state == GenState::Pending)
}

/// Ce que la relecture d'une leçon doit savoir : consignes et message de la
/// leçon (pour la réécrire), langue, niveau, sorte de sujet.
struct Review<'a> {
    system: &'a str,
    user: &'a str,
    lang: Lang,
    level: super::StudyLevel,
    kind: StudyKind,
}

/// Questions de validation au moins (sinon dernière chance, puis
/// `invalid`).
const MIN_QUESTIONS: usize = 2;

/// Ce qu'une demande de questions envoie.
struct Request<'a, 'b> {
    ollama: &'a Gen<'b>,
    lang: Lang,
    kind: StudyKind,
    prefix: &'a str,
    prompt: &'a (dyn Fn(&[QType]) -> String + Sync),
    user: &'a str,
    with_explanation: bool,
}

/// 100 premiers caractères d'un texte, sur une ligne.
fn short(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    line.chars().take(100).collect()
}

/// Question écartée, au journal : partie, type, raison, début de l'énoncé.
fn dropped(prefix: &str, qtype: QType, reason: &str, item: &serde_json::Value) {
    // i18n-ignore : journal.
    eprintln!(
        "chatonlm: study: question dropped ({prefix}, {qtype:?}): {reason} — \"{}\"",
        checks::excerpt(item)
    );
}

/// Questions acceptées, une par emplacement (`None` : écartée).
pub struct Generated {
    pub explanation: Option<String>,
    pub slots: Vec<Option<(Draft, String)>>,
}

/// Questions gardées, numérotées `{prefix}-Q0`… dans l'ordre.
fn numbered(slots: &[Option<(Draft, String)>], course: &str, prefix: &str) -> Vec<StoredQuestion> {
    slots
        .iter()
        .flatten()
        .enumerate()
        .map(|(n, (draft, explanation))| {
            checks::finalize(
                draft.clone(),
                course,
                &format!("{prefix}-Q{n}"),
                explanation.clone(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_questions_are_numbered_without_gaps() {
        let draft = |statement: &str| {
            Some((
                Draft::TrueFalse {
                    statement: statement.into(),
                    answer: true,
                },
                "E".to_owned(),
            ))
        };
        let questions = numbered(&[draft("a"), None, draft("c")], "c", "L2-A1");
        let ids: Vec<&str> = questions.iter().map(|q| q.id.as_str()).collect();
        assert_eq!(ids, ["L2-A1-Q0", "L2-A1-Q1"]);
    }
}
