//! Premières phases : stratégie (3 tirages), cadrage, spécification, et la
//! version directe (conception § 0 bis 4, 2 et 9).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::AgentError;

use super::checks::MAX_LENGTH_WORDS;
use super::checks::{self, Expect, Question as JudgeQuestion, WHOLE_LENGTH_TOLERANCE};
use super::measure;
use super::prompts::{self, THINK_DIRECT};
use super::text::{as_document, document_wide, looks_like_refusal, normal_case};
use super::types::{
    Activity, Answer, Check, Criterion, CriterionScope, EventKind, Phase, Plan, Question, Register,
    Spec, StateKey, Strategy,
};
use super::{Run, Stop};

/// Plus longue rédaction tentée d'un jet (conception § 0 bis 9).
pub const DIRECT_MAX_WORDS: u32 = 4_000;

/// Températures des trois tirages de la stratégie.
const DRAWS: [f32; 3] = [0.2, 0.6, 1.0];

/// Stratégie retenue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Chosen {
    pub strategy: Strategy,
    pub unanimous: bool,
    /// Questions de cadrage proposées par les tirages.
    pub questions: Vec<String>,
}

/// Cadrage : questions et réponses (vides : sans réponse).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Framing {
    pub questions: Vec<String>,
    pub answers: Vec<String>,
}

/// Version directe vérifiée.
pub(super) struct Direct {
    pub doc: String,
    pub checks: Vec<Check>,
}

/// Échec « doux » d'une requête : réponse illisible, coupée ou qui boucle
/// → `None` (l'appelant décide) ; arrêt → [`Stop::Cancelled`] ; erreur du
/// moteur → échec du projet (reprenable).
pub(super) fn soft<T>(result: Result<T, AgentError>) -> Result<Option<T>, Stop> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(AgentError::Unreadable | AgentError::Incomplete { .. } | AgentError::Looping) => {
            Ok(None)
        }
        Err(other) => Err(other.into()),
    }
}

fn str_field(value: &Value, name: &str) -> Option<String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_owned())
}

fn strings_field(value: &Value, name: &str) -> Vec<String> {
    value
        .get(name)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Un tirage lu : stratégie et questions.
fn read_draw(value: &Value) -> Option<(Strategy, Vec<String>)> {
    let strategy = prompts::strategy_of(&str_field(value, "strategy")?)?;
    let mut questions = strings_field(value, "framing_questions");
    questions.truncate(4);
    Some((strategy, questions))
}

/// Critères lus : objets `{text, scope}` (ou simples textes), numérotés
/// C1…, en casse normale ([`normal_case`]) ; portée « document » donnée par
/// la spécification, ou reconnue par le programme ([`document_wide`]).
fn read_criteria(value: &Value) -> Vec<Criterion> {
    let items = value
        .get("criteria")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    items
        .iter()
        .filter_map(|item| {
            let (text, scope) = match item {
                Value::String(text) => (text.trim().to_owned(), None),
                other => (str_field(other, "text")?, str_field(other, "scope")),
            };
            (!text.is_empty()).then_some((text, scope))
        })
        .enumerate()
        .map(|(i, (text, scope))| {
            let mut criterion = Criterion {
                id: format!("C{}", i + 1),
                text,
                scope: if scope.as_deref() == Some("document") {
                    CriterionScope::Document
                } else {
                    CriterionScope::Section
                },
            };
            settle_criterion(&mut criterion);
            criterion
        })
        .collect()
}

/// Casse normale ([`normal_case`]) et portée « document » reconnue par le
/// programme ([`document_wide`]) ; aussi à la relecture d'une spécification
/// sauvée avant cette règle (reprise d'un projet commencé).
pub(super) fn settle_criterion(criterion: &mut Criterion) {
    criterion.text = normal_case(&criterion.text);
    if document_wide(&criterion.text) {
        criterion.scope = CriterionScope::Document;
    }
}

/// Spécification lue de la réponse du modèle (critères numérotés C1…).
pub(super) fn read_spec(value: &Value) -> Option<Spec> {
    let register = match str_field(value, "register")?.as_str() {
        "informal" => Register::Informal,
        "formal" => Register::Formal,
        "impersonal" => Register::Impersonal,
        _ => return None,
    };
    let glossary = value
        .get("glossary")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| Some((str_field(item, "term")?, str_field(item, "definition")?)))
                .filter(|(term, _)| !term.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let length = value
        .get("length_words")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Some(Spec {
        goal: str_field(value, "goal")?,
        deliverables: strings_field(value, "deliverables"),
        criteria: read_criteria(value),
        audience: str_field(value, "audience").unwrap_or_default(),
        language: str_field(value, "language")?.to_lowercase(),
        register,
        units: str_field(value, "units").unwrap_or_default(),
        length_words: u32::try_from(length).ok().filter(|n| *n > 0),
        glossary,
        assumptions: strings_field(value, "assumptions"),
        constraints: strings_field(value, "constraints"),
    })
}

/// Questions du juge de forme sur le livrable entier : chaque critère, la
/// langue, le registre. Les critères mesurables (nombre de mots, de titres,
/// tableau…) n'en sont pas : le programme les mesure ([`measure`]).
// i18n-ignore : questions au modèle.
pub(super) fn whole_questions(spec: &Spec) -> Vec<JudgeQuestion> {
    let mut out: Vec<JudgeQuestion> = spec
        .criteria
        .iter()
        .filter(|c| measure::of_criterion(c, &spec.language).is_none())
        .map(|c| JudgeQuestion {
            criterion: c.id.clone(),
            text: format!("Criterion {} (« {} »): is it met?", c.id, c.text),
            blocking: true,
        })
        .collect();
    out.extend(common_questions(spec));
    out
}

/// Langue et registre (bloquants : le lot 0 a vu le tutoiement des
/// parties et le vouvoiement de la conclusion).
// i18n-ignore : questions au modèle.
pub(super) fn common_questions(spec: &Spec) -> Vec<JudgeQuestion> {
    vec![
        JudgeQuestion {
            criterion: "language".into(),
            text: format!(
                "Is the whole text written in {}?",
                prompts::language_name(&spec.language)
            ),
            blocking: true,
        },
        JudgeQuestion {
            criterion: "register".into(),
            text: format!(
                "Does the text {} (no other way of addressing the reader anywhere)?",
                prompts::register_rule(spec.register)
            ),
            blocking: true,
        },
    ]
}

impl Run<'_> {
    /// Pose une question (événements autour) ; annulation pendant l'attente :
    /// [`Answer::Cancelled`].
    pub(super) async fn ask(&self, question: Question) -> Answer {
        self.emit(EventKind::QuestionAsked {
            question: question.clone(),
        })
        .await;
        let answer = tokio::select! {
            biased;
            () = self.cancel.cancelled() => Answer::Cancelled,
            answer = self.host.ask(question) => answer,
        };
        self.emit(EventKind::AnswerReceived {
            answer: answer.clone(),
        })
        .await;
        answer
    }

    async fn draw(&self, n: u8, temperature: f32) -> Result<Option<(Strategy, Vec<String>)>, Stop> {
        let project = self.project();
        let call = prompts::strategy_call(&project.task, &project.attachments, temperature);
        let value = soft(
            self.json(
                StateKey::StrategyDraw(n),
                Activity::ChoosingStrategy { draw: u32::from(n) },
                call,
            )
            .await,
        )?;
        Ok(value.as_ref().and_then(read_draw))
    }

    /// Stratégie : imposée (un seul tirage, pour les questions de cadrage,
    /// sauf en Autonome), sinon 3 tirages réflexion coupée ; unanimes →
    /// retenue ; sinon générique (Autonome) ou question à l'utilisateur.
    pub(super) async fn strategy(&self) -> Result<Chosen, Stop> {
        self.phase(Phase::Strategy, true).await;
        if let Some(chosen) = self.load::<Chosen>(StateKey::Strategy).await {
            self.phase(Phase::Strategy, false).await;
            return Ok(chosen);
        }
        let mut chosen = if let Some(forced) = self.project().strategy {
            if !forced.available() {
                return Err(Stop::Failed("strategyNotYet".into()));
            }
            let questions = if self.autonomous() {
                Vec::new()
            } else {
                self.draw(1, DRAWS[1])
                    .await?
                    .map(|(_, q)| q)
                    .unwrap_or_default()
            };
            Chosen {
                strategy: forced,
                unanimous: true,
                questions,
            }
        } else {
            let mut draws = Vec::new();
            for (i, temperature) in DRAWS.iter().enumerate() {
                draws.push(self.draw(i as u8 + 1, *temperature).await?);
            }
            let valid: Vec<&(Strategy, Vec<String>)> = draws.iter().flatten().collect();
            let unanimous =
                valid.len() == DRAWS.len() && valid.iter().all(|(s, _)| *s == valid[0].0);
            let strategy = if unanimous {
                valid[0].0
            } else if self.autonomous() {
                Strategy::Generic
            } else {
                // Les plus votées d'abord.
                let mut options: Vec<(Strategy, usize)> = Vec::new();
                for (strategy, _) in &valid {
                    match options.iter_mut().find(|(s, _)| s == strategy) {
                        Some((_, count)) => *count += 1,
                        None => options.push((*strategy, 1)),
                    }
                }
                options.sort_by_key(|a| std::cmp::Reverse(a.1));
                // Seules les stratégies que ce lot sait mener sont proposées.
                let mut options: Vec<Strategy> = options
                    .into_iter()
                    .map(|(s, _)| s)
                    .filter(|s| s.available())
                    .collect();
                if !options.contains(&Strategy::Generic) {
                    options.push(Strategy::Generic);
                }
                if options.len() == 1 {
                    // Plus de choix (votes pour des stratégies à venir) :
                    // générique, sans question.
                    self.warn("strategyNotYet").await;
                    options[0]
                } else {
                    match self.ask(Question::Strategy(options.clone())).await {
                        Answer::Strategy(strategy) => strategy,
                        Answer::Cancelled => return Err(Stop::Cancelled),
                        _ => options[0],
                    }
                }
            };
            // Questions du tirage du milieu s'il a voté pour elle, sinon du
            // premier qui l'a fait, sinon de n'importe lequel.
            let questions = [1, 0, 2]
                .iter()
                .filter_map(|i| draws[*i].as_ref())
                .find(|(s, _)| *s == strategy)
                .or_else(|| valid.first().copied())
                .map(|(_, q)| q.clone())
                .unwrap_or_default();
            Chosen {
                strategy,
                unanimous,
                questions,
            }
        };
        if !chosen.strategy.available() {
            self.warn("strategyNotYet").await;
            chosen.strategy = Strategy::Generic;
        }
        self.save(StateKey::Strategy, &chosen).await;
        self.emit(EventKind::StrategyChosen {
            strategy: chosen.strategy,
            unanimous: chosen.unanimous,
        })
        .await;
        self.phase(Phase::Strategy, false).await;
        Ok(chosen)
    }

    /// Cadrage : questions à l'utilisateur, sauf en Autonome (la
    /// spécification écrit alors des hypothèses).
    pub(super) async fn framing(&self, chosen: &Chosen) -> Result<Framing, Stop> {
        self.phase(Phase::Framing, true).await;
        if let Some(framing) = self.load::<Framing>(StateKey::Framing).await {
            self.phase(Phase::Framing, false).await;
            return Ok(framing);
        }
        let questions = chosen.questions.clone();
        let answers = if questions.is_empty() || self.autonomous() {
            Vec::new()
        } else {
            match self.ask(Question::Framing(questions.clone())).await {
                Answer::Framing(answers) => answers,
                Answer::Cancelled => return Err(Stop::Cancelled),
                _ => Vec::new(),
            }
        };
        let framing = Framing { questions, answers };
        self.save(StateKey::Framing, &framing).await;
        self.phase(Phase::Framing, false).await;
        Ok(framing)
    }

    /// Spécification : 3 essais au plus ; chaque défaut (indécision, mot
    /// interne, longueur absente) est rendu au modèle pour l'essai suivant.
    /// Longueur au-delà de [`MAX_LENGTH_WORDS`] : bornée (`lengthCapped`).
    /// Trois refus : la dernière spécification lisible est gardée
    /// (`specBestOfThree`, défauts dits, langue invalide remplacée par celle
    /// de l'app) ; aucune lisible : échec `specUndecided`.
    pub(super) async fn spec(&self, strategy: Strategy, framing: &Framing) -> Result<Spec, Stop> {
        self.phase(Phase::Spec, true).await;
        if let Some(mut spec) = self.load::<Spec>(StateKey::Spec).await {
            spec.criteria.iter_mut().for_each(settle_criterion);
            self.phase(Phase::Spec, false).await;
            return Ok(spec);
        }
        let project = self.project();
        let framing_text =
            prompts::framing_block(&framing.questions, &framing.answers, self.autonomous());
        let mut feedback: Vec<String> = Vec::new();
        let mut last: Option<(Spec, Vec<&'static str>, bool)> = None;
        for attempt in 1..=3u8 {
            let call = prompts::spec_call(
                &project.task,
                &project.attachments,
                self.attachment_room(),
                &project.app_language,
                &framing_text,
                &feedback,
            );
            let value = soft(
                self.json(
                    StateKey::SpecAttempt(attempt),
                    Activity::WritingSpec {
                        attempt: u32::from(attempt),
                    },
                    call,
                )
                .await,
            )?;
            // i18n-ignore : retours au modèle.
            let Some(mut spec) = value.as_ref().and_then(read_spec) else {
                feedback = vec!["the answer was not complete, readable JSON".into()];
                continue;
            };
            // Plus long que 20 parties de 1 500 mots : borné, dit au rapport.
            let capped = spec.length_words.is_some_and(|n| n > MAX_LENGTH_WORDS);
            if capped {
                spec.length_words = Some(MAX_LENGTH_WORDS);
            }
            let defects = checks::spec_defects(&spec, strategy, &self.allowed);
            if defects.is_empty() {
                return self.keep_spec(spec, capped, &[]).await;
            }
            let mut codes: Vec<&'static str> = Vec::new();
            for defect in &defects {
                if !codes.contains(&defect.code) {
                    codes.push(defect.code);
                }
            }
            feedback = defects.into_iter().map(|d| d.detail).collect();
            last = Some((spec, codes, capped));
        }
        match last {
            Some((mut spec, codes, capped)) => {
                let language_ok = spec.language.len() == 2
                    && spec.language.chars().all(|c| c.is_ascii_lowercase());
                if !language_ok {
                    spec.language.clone_from(&project.app_language);
                }
                self.keep_spec(spec, capped, &codes).await
            }
            None => Err(Stop::Failed("specUndecided".into())),
        }
    }

    /// Spécification retenue, avec ses avertissements pour le rapport.
    async fn keep_spec(
        &self,
        spec: Spec,
        capped: bool,
        defects: &[&'static str],
    ) -> Result<Spec, Stop> {
        if capped {
            self.note("lengthCapped").await;
        }
        if !defects.is_empty() {
            self.note("specBestOfThree").await;
            for code in defects {
                self.note(&format!("specDefect.{code}")).await;
            }
        }
        self.save(StateKey::Spec, &spec).await;
        self.phase(Phase::Spec, false).await;
        Ok(spec)
    }

    /// La version directe se tente : longueur ≤ 4 000 mots (ou pas un texte
    /// mesuré) et documents qui tiennent dans la place que le contexte du
    /// modèle leur laisse ([`prompts::attachment_room`]).
    pub(super) fn direct_fits(&self, spec: &Spec) -> bool {
        spec.length_words.is_none_or(|n| n <= DIRECT_MAX_WORDS)
            && !prompts::attachments_cut(&self.project().attachments, self.attachment_room())
    }

    /// Version directe (une requête, réflexion 8 000), puis les MÊMES
    /// vérifications que le livrable assemblé (longueur à ±10 %, critères
    /// mesurables comptés par programme) ; des « non » : UNE correction
    /// ciblée (comme une partie ; longueur : allonger ou raccourcir, mots
    /// mesurés et visés dits), la version qui a le moins de « non » gardée.
    /// Tenue (pas de « non » de fond, longueur à ±35 %) → `Some`, l'écart
    /// restant dit au rapport ; sinon (ou refus, boucle) → `None` : plan et
    /// parties.
    pub(super) async fn direct(
        &self,
        spec: &Spec,
        outline: Option<&Plan>,
    ) -> Result<Option<Direct>, Stop> {
        self.phase(Phase::Build, true).await;
        let project = self.project();
        let messages = prompts::direct_messages(
            spec,
            &project.task,
            &project.attachments,
            self.attachment_room(),
            outline,
        );
        let words = spec.length_words.unwrap_or(2_000);
        let Some(text) = self
            .write_version(
                StateKey::Text { id: 0, version: 0 },
                Activity::WritingDirect,
                messages.clone(),
                words,
                THINK_DIRECT,
            )
            .await?
        else {
            self.warn("directFailed").await;
            return Ok(None);
        };
        let doc = as_document(&text);
        if looks_like_refusal(&doc, words) && self.refused(0, &project.task, &doc).await? {
            self.warn("directFailed").await;
            return Ok(None);
        }
        self.phase(Phase::Build, false).await;
        self.phase(Phase::Verify, true).await;
        let questions = whole_questions(spec);
        let measures = measure::measured(spec);
        let expect = Expect {
            words: spec.length_words,
            tolerance: WHOLE_LENGTH_TOLERANCE,
            language: &spec.language,
            allowed: &self.allowed,
            lists: &[],
            measures: &measures,
        };
        let checks = self
            .verify(
                0,
                0,
                Activity::CheckingDirect,
                spec,
                "DOCUMENT",
                &doc,
                &expect,
                &questions,
            )
            .await?;
        self.emit(EventKind::BrickChecks {
            id: 0,
            attempt: 0,
            checks: checks.clone(),
        })
        .await;
        let (mut doc, mut checks) = (doc, checks);
        if checks::noes(&checks) > 0 {
            // UNE correction ciblée, comme pour une partie : critères non
            // tenus et affirmations fausses, avec l'endroit.
            let problems = checks::problems(&checks, &questions, spec.length_words);
            let fixed = self
                .write_version(
                    StateKey::Text { id: 0, version: 1 },
                    Activity::WritingDirect,
                    prompts::direct_fix_messages(&messages, &doc, &problems),
                    words,
                    THINK_DIRECT,
                )
                .await?;
            if let Some(fixed) = fixed.map(|text| as_document(&text)) {
                let mut second = self
                    .verify(
                        0,
                        1,
                        Activity::CheckingDirect,
                        spec,
                        "DOCUMENT",
                        &fixed,
                        &expect,
                        &questions,
                    )
                    .await?;
                // Remarque du juge recopiée : signalée (non bloquante).
                second.extend(checks::copied_remarks(&checks, &doc, &fixed));
                self.emit(EventKind::BrickChecks {
                    id: 0,
                    attempt: 1,
                    checks: second.clone(),
                })
                .await;
                // Moins de « non », ou autant mais une longueur plus proche.
                if checks::better(&second, &checks) {
                    doc = fixed;
                    checks = second;
                }
            }
        }
        // Seuls des « non » de fond (ou une longueur très à côté) valent la
        // route par parties ; le reste est dit au rapport. Quelques
        // affirmations fausses seulement (≤ 5) : passe chirurgicale sur ces
        // phrases, puis la version est livrée ; ce qui reste faux est « à
        // revoir » (banc du 25 nov. : une bonne version de 3 464 mots
        // envoyée au plan ×13 pour « une tomate trop mûre attire les
        // pucerons »).
        if checks::direct_fails(&checks) {
            if !checks::surgery_fits(&checks) {
                self.warn("directFailed").await;
                return Ok(None);
            }
            let (fixed, after) = self.surgery(0, spec, &doc, checks).await?;
            self.emit(EventKind::BrickChecks {
                id: 0,
                attempt: 2,
                checks: after.clone(),
            })
            .await;
            doc = fixed;
            checks = after;
        }
        self.phase(Phase::Verify, false).await;
        Ok(Some(Direct { doc, checks }))
    }
}
