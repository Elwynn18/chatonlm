//! Plan (conception § 4.1, § 0 bis 6) : contrôles par programme d'abord,
//! puis juge noté (≥ 6 accepté, ≤ 5 rejeté) ; jusqu'à 3 plans tentés UN PAR
//! UN ; le premier accepté, sinon la meilleure note, sinon (aucun plan sans
//! défaut de programme) celui qui a le moins de défauts, s'il peut être
//! mené ; aucun : échec `planUnusable`. Accord de
//! l'utilisateur hors Autonome (plan modifiable, re-contrôlé ; une note de
//! refus devient une consigne numérotée de la spécification et relance un
//! tour de plans).

use serde_json::Value;

use super::checks::{self, Defect};
use super::flow::soft;
use super::prompts;
use super::text::clean_title;
use super::types::{
    Activity, Answer, BrickKind, BrickPlan, EventKind, Phase, Plan, Question, Spec, StateKey,
    Strategy,
};
use super::{Run, Stop};

/// Note du juge à partir de laquelle un plan est accepté (lot 0, 27B).
pub const ACCEPTED: i64 = 6;

/// Températures des trois plans.
const TEMPERATURES: [f32; 3] = [0.6, 0.8, 1.0];

/// Défauts qui empêchent de mener un plan modifié par l'utilisateur (les
/// autres sont son choix : signalés seulement).
const STRUCTURAL: &[&str] = &[
    "badId",
    "duplicateId",
    "noParts",
    "selfDependency",
    "unknownDependency",
    "dependsOnFrame",
    "cycle",
];

/// Identifiant de critère normalisé : « c2 », « 2 » → « C2 ».
fn criterion_id(raw: &str) -> String {
    let raw = raw.trim().to_uppercase();
    if raw.chars().all(|c| c.is_ascii_digit()) && !raw.is_empty() {
        format!("C{raw}")
    } else {
        raw
    }
}

/// Codes des défauts, chacun une fois, dans l'ordre où ils sont vus.
fn defect_codes(defects: &[Defect]) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = Vec::new();
    for defect in defects {
        if !codes.contains(&defect.code) {
            codes.push(defect.code);
        }
    }
    codes
}

/// Plan lu de la réponse du modèle.
pub(super) fn read_plan(value: &Value) -> Option<Plan> {
    let text = |v: &Value, name: &str| {
        v.get(name)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned()
    };
    let numbers = |v: &Value, name: &str| -> Vec<u32> {
        v.get(name)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_u64)
                    .filter_map(|n| u32::try_from(n).ok())
                    .collect()
            })
            .unwrap_or_default()
    };
    let strings = |v: &Value, name: &str| -> Vec<String> {
        v.get(name)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    let parts = value.get("parts")?.as_array()?;
    let mut bricks = Vec::new();
    for part in parts {
        let kind = match text(part, "kind").as_str() {
            "intro" => BrickKind::Intro,
            "conclusion" => BrickKind::Conclusion,
            _ => BrickKind::Text,
        };
        bricks.push(BrickPlan {
            id: u32::try_from(part.get("id")?.as_u64()?).ok()?,
            title: clean_title(&text(part, "title")),
            goal: text(part, "goal"),
            kind,
            words: part
                .get("words")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
            depends_on: numbers(part, "depends_on"),
            criteria: strings(part, "criteria")
                .iter()
                .map(|c| criterion_id(c))
                .collect(),
            forbidden: strings(part, "forbidden"),
        });
    }
    Some(Plan {
        title: text(value, "title"),
        bricks,
    })
}

/// Note et défauts lus du jugement d'un plan.
fn read_judgement(value: &Value) -> Option<(i64, Vec<String>)> {
    let score = value.get("score")?.as_i64()?;
    let mut defects: Vec<String> = value
        .get("defects")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|d| {
                    let problem = d.get("problem")?.as_str()?;
                    let part = d.get("part").and_then(Value::as_u64).unwrap_or(0);
                    // i18n-ignore : retour au modèle.
                    Some(format!("part {part}: {problem}"))
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(evaluations) = value.get("evaluations").and_then(Value::as_array) {
        for e in evaluations {
            if e.get("verdict").and_then(Value::as_str) == Some("no") {
                let n = e.get("question").and_then(Value::as_u64).unwrap_or(0) as usize;
                if let Some(question) = n.checked_sub(1).and_then(|i| prompts::PLAN_GRID.get(i)) {
                    let evidence = e.get("evidence").and_then(Value::as_str).unwrap_or("");
                    defects.push(format!("{question} → {evidence}"));
                }
            }
        }
    }
    Some((score.clamp(0, 10), defects))
}

impl Run<'_> {
    /// Un tour de plans (au plus 3, un par un).
    async fn plan_round(&self, strategy: Strategy, spec: &Spec, round: u8) -> Result<Plan, Stop> {
        let task = &self.project().task;
        let mut feedback: Vec<String> = Vec::new();
        let mut best: Option<(i64, Plan)> = None;
        // Aucun plan sans défaut : celui qui a le moins de défauts, s'ils ne
        // l'empêchent pas d'être mené (sinon le projet échouerait, et une
        // reprise relirait les mêmes plans).
        let mut fallback: Option<(usize, Vec<&'static str>, Plan)> = None;
        for (i, temperature) in TEMPERATURES.iter().enumerate() {
            let attempt = i as u8 + 1;
            let call = prompts::plan_call(strategy, spec, task, &feedback, *temperature);
            let value = soft(
                self.json(
                    StateKey::PlanAttempt { round, attempt },
                    Activity::WritingPlan {
                        attempt: u32::from(attempt),
                    },
                    call,
                )
                .await,
            )?;
            // i18n-ignore : retours au modèle.
            let Some(mut plan) = value.as_ref().and_then(read_plan) else {
                feedback = vec!["the answer was not complete, readable JSON".into()];
                continue;
            };
            // Critères du document entier attribués à des parties : retirés
            // (jugés sur le livrable entier seulement), avertissement.
            if checks::strip_document_criteria(&mut plan, spec) {
                self.warn("documentCriteriaRemoved").await;
            }
            let defects = checks::plan_defects(&plan, spec, strategy, &self.allowed);
            if !defects.is_empty() {
                let codes = defect_codes(&defects);
                let usable = !codes.iter().any(|c| STRUCTURAL.contains(c));
                let count = defects.len();
                if usable && fallback.as_ref().is_none_or(|(n, _, _)| count < *n) {
                    fallback = Some((count, codes, plan));
                }
                feedback = defects.into_iter().map(|d| d.detail).collect();
                continue;
            }
            let judged = soft(
                self.json(
                    StateKey::PlanJudge { round, attempt },
                    Activity::JudgingPlan {
                        attempt: u32::from(attempt),
                    },
                    prompts::plan_judge_call(strategy, spec, &plan),
                )
                .await,
            )?;
            let Some((score, defects)) = judged.as_ref().and_then(read_judgement) else {
                // Juge indisponible : plan gardé en dernier recours.
                self.warn("judgeUnavailable").await;
                if best.is_none() {
                    best = Some((-1, plan));
                }
                continue;
            };
            if score >= ACCEPTED {
                return Ok(plan);
            }
            feedback = defects;
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                best = Some((score, plan));
            }
        }
        if let Some((_, plan)) = best {
            self.warn("planBestOfThree").await;
            return Ok(plan);
        }
        match fallback {
            Some((_, codes, plan)) => {
                self.warn("planBestOfThree").await;
                for code in codes {
                    self.warn(&format!("planDefect.{code}")).await;
                }
                Ok(plan)
            }
            None => Err(Stop::Failed("planUnusable".into())),
        }
    }

    /// Défauts d'un plan modifié par l'utilisateur : tous signalés ; vrai
    /// s'il ne peut pas être mené.
    async fn edited_defects(&self, plan: &Plan, spec: &Spec, strategy: Strategy) -> bool {
        let defects: Vec<Defect> = checks::plan_defects(plan, spec, strategy, &self.allowed);
        let codes = defect_codes(&defects);
        for code in &codes {
            self.warn(&format!("planDefect.{code}")).await;
        }
        codes.iter().any(|c| STRUCTURAL.contains(c))
    }

    /// Plan retenu (et spécification, enrichie des notes de l'utilisateur).
    pub(super) async fn plan(&self, strategy: Strategy, spec: &Spec) -> Result<(Plan, Spec), Stop> {
        self.phase(Phase::Plan, true).await;
        if let Some(mut plan) = self.load::<Plan>(StateKey::Plan).await {
            // Plan sauvé avant la règle des critères d'ensemble (reprise).
            if checks::strip_document_criteria(&mut plan, spec) {
                self.warn("documentCriteriaRemoved").await;
            }
            self.phase(Phase::Plan, false).await;
            return Ok((plan, spec.clone()));
        }
        let mut spec = spec.clone();
        let mut round = self.load::<u8>(StateKey::PlanRound).await.unwrap_or(0);
        let plan = 'rounds: loop {
            let mut plan = self.plan_round(strategy, &spec, round).await?;
            if self.autonomous() {
                break plan;
            }
            loop {
                match self.ask(Question::ApprovePlan(plan.clone())).await {
                    Answer::Plan {
                        approved: true,
                        edited,
                        ..
                    } => match edited {
                        None => break 'rounds plan,
                        Some(mut edited) => {
                            if checks::strip_document_criteria(&mut edited, &spec) {
                                self.warn("documentCriteriaRemoved").await;
                            }
                            let blocked = self.edited_defects(&edited, &spec, strategy).await;
                            if !blocked {
                                break 'rounds edited;
                            }
                            // Plan modifié impossible à mener : redemandé
                            // tel quel, défauts signalés.
                            plan = edited;
                        }
                    },
                    Answer::Plan {
                        approved: false,
                        note: Some(note),
                        ..
                    } if !note.trim().is_empty() => {
                        spec.constraints.push(note.trim().to_owned());
                        self.save(StateKey::Spec, &spec).await;
                        round = round.saturating_add(1);
                        self.save(StateKey::PlanRound, &round).await;
                        continue 'rounds;
                    }
                    Answer::Cancelled => return Err(Stop::Cancelled),
                    _ => return Err(Stop::Rejected),
                }
            }
        };
        self.save(StateKey::Plan, &plan).await;
        self.emit(EventKind::PlanReady { plan: plan.clone() }).await;
        self.phase(Phase::Plan, false).await;
        Ok((plan, spec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_are_read_with_normalised_criteria() {
        let value = serde_json::json!({
            "analysis": "a",
            "title": "Guide",
            "parts": [
                {"id": 1, "title": "Intro", "goal": "g", "kind": "intro", "words": 100,
                 "depends_on": [], "criteria": [], "forbidden": []},
                {"id": 2, "title": "Sol", "goal": "g", "kind": "text", "words": 400,
                 "depends_on": [1], "criteria": ["c1", "2"], "forbidden": ["l'eau"]},
            ]
        });
        let plan = read_plan(&value).unwrap();
        assert_eq!(plan.title, "Guide");
        assert_eq!(plan.bricks[0].kind, BrickKind::Intro);
        assert_eq!(
            plan.bricks[1].criteria,
            vec!["C1".to_owned(), "C2".to_owned()]
        );
        assert_eq!(plan.bricks[1].forbidden, vec!["l'eau".to_owned()]);
        assert!(read_plan(&serde_json::json!({"parts": [{"title": "x"}]})).is_none());
    }

    #[test]
    fn judgements_give_a_score_and_defects() {
        let value = serde_json::json!({
            "evaluations": [{"question": 2, "evidence": "3 et 4 pareils", "verdict": "no"}],
            "defects": [{"part": 3, "problem": "doublon"}],
            "score": 4
        });
        let (score, defects) = read_judgement(&value).unwrap();
        assert_eq!(score, 4);
        assert_eq!(defects[0], "part 3: doublon");
        assert!(defects[1].starts_with("Duplicates"));
    }
}
