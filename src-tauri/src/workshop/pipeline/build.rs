//! Construction par parties : ordre des dépendances (introduction et
//! conclusion à la fin), consignes avec le registre GLOBAL, rédaction,
//! vérifications (programme, juge de forme, juge d'exactitude), UNE
//! correction ciblée en gardant la version qui a le moins de « non », passe
//! chirurgicale pour quelques affirmations fausses restées, redécoupage
//! ADaPT (critère de fond non tenu, partie d'au moins 600 mots, profondeur
//! ≤ 1, 3 sous-parties au plus), carte de sortie (conception § 4.2, 4.6,
//! § 0 bis 1, 7, 8 ; banc du 25 nov.).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::AgentError;
use crate::inference::Message;

use super::brief::{Brief, Neighbour};
use super::checks::{self, Expect, LENGTH_TOLERANCE, MAX_SURGERY, Question as JudgeQuestion};
use super::flow::{common_questions, soft};
use super::measure::{self, Measure};
use super::prompts::{self, ANSWER, THINK_FRAME, THINK_PART};
use super::registry::{MAX_LEVEL, Registry};
use super::text::{as_intro, as_section, body_of, clean_title, demoted, looks_like_refusal, words};
use super::types::{
    Activity, BrickKind, BrickPlan, BrickState, Card, Check, CheckMethod, CriterionScope,
    EventKind, Phase, Plan, Spec, StateKey,
};
use super::{Run, Stop, writing};

/// Consignes d'une partie au plus (jetons ; conception § 4.2 : 4 000 à
/// 8 000 visés, atteints sans condensation dans le cas courant). Plafond
/// effectif : le plus petit de celui-ci et du contexte moins la réflexion et
/// la réponse ([`Run::brief_cap`]).
pub const BRIEF_MAX: usize = 16_000;
/// Registre au-delà duquel il est condensé (jetons).
pub const REGISTRY_MAX: usize = 2_000;
/// Profondeur de redécoupage, au plus (banc du 25 nov. : une partie de 300
/// mots redécoupée deux fois, pendant plus d'une heure).
pub const MAX_DEPTH: u8 = 1;
/// Longueur d'une partie en dessous de laquelle elle n'est jamais
/// redécoupée (mots) : ce qui reste est « à revoir ».
pub const MIN_SPLIT_WORDS: u32 = 600;

/// Une partie finie (sauvée sous [`StateKey::Brick`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrickRecord {
    pub id: u32,
    pub title: String,
    pub kind: BrickKind,
    pub state: BrickState,
    /// Version gardée (section Markdown ; vide : refusée ou sans texte).
    pub text: String,
    /// Une correction a été tentée.
    pub fixed: bool,
    pub checks: Vec<Check>,
}

/// Ce que la construction a produit jusqu'ici.
#[derive(Default)]
pub(super) struct Progress {
    pub registry: Registry,
    /// Cartes par partie (une partie redécoupée : fusion de ses sous-parties).
    pub cards: HashMap<u32, Card>,
    /// Cartes des parties écrites (pas des redécoupées), dans l'ordre.
    pub written: Vec<(u32, Card)>,
    /// Parties finies, dans l'ordre où elles l'ont été.
    pub records: Vec<BrickRecord>,
}

/// Ordre d'exécution : sections dans l'ordre des dépendances (ordre du plan
/// entre parties libres), puis introductions, puis conclusions.
pub(super) fn execution_order(plan: &Plan) -> Vec<u32> {
    let body: Vec<&BrickPlan> = plan
        .bricks
        .iter()
        .filter(|b| b.kind == BrickKind::Text)
        .collect();
    let body_ids: Vec<u32> = body.iter().map(|b| b.id).collect();
    let mut done: Vec<u32> = Vec::new();
    while done.len() < body.len() {
        let next = body.iter().find(|b| {
            !done.contains(&b.id)
                && b.depends_on
                    .iter()
                    .all(|d| *d == b.id || !body_ids.contains(d) || done.contains(d))
        });
        match next {
            Some(b) => done.push(b.id),
            // Cycle (exclu par les contrôles) : ordre du plan.
            None => {
                let rest: Vec<u32> = body_ids
                    .iter()
                    .filter(|id| !done.contains(id))
                    .copied()
                    .collect();
                done.extend(rest);
            }
        }
    }
    for kind in [BrickKind::Intro, BrickKind::Conclusion] {
        done.extend(plan.bricks.iter().filter(|b| b.kind == kind).map(|b| b.id));
    }
    done
}

/// Ordre de lecture : introductions, sections (ordre du plan), conclusions.
pub(super) fn reading_order(bricks: &[BrickPlan]) -> Vec<&BrickPlan> {
    let mut out: Vec<&BrickPlan> = Vec::new();
    for kind in [BrickKind::Intro, BrickKind::Text, BrickKind::Conclusion] {
        out.extend(bricks.iter().filter(|b| b.kind == kind));
    }
    out
}

/// Titre du livrable.
pub(super) fn doc_title(plan: &Plan, spec: &Spec, fallback: &str) -> String {
    [
        plan.title.as_str(),
        spec.deliverables.first().map_or("", String::as_str),
        fallback,
    ]
    .into_iter()
    .map(str::trim)
    .find(|t| !t.is_empty())
    .unwrap_or("Document")
    .to_owned()
}

/// Questions du juge de forme pour une partie.
// i18n-ignore : questions au modèle.
fn part_questions(spec: &Spec, brick: &BrickPlan) -> Vec<JudgeQuestion> {
    let mut forbidden = brick.forbidden.join("; ");
    if brick.kind == BrickKind::Text {
        if !forbidden.is_empty() {
            forbidden.push_str("; ");
        }
        forbidden.push_str("a general introduction or conclusion of the document");
    }
    let mut out = vec![
        JudgeQuestion {
            criterion: "goal".into(),
            text: format!("Does the section fully cover its goal: {}?", brick.goal),
            blocking: true,
        },
        JudgeQuestion {
            criterion: "forbidden".into(),
            text: format!("Does the section stay out of what it must not cover ({forbidden})?"),
            blocking: true,
        },
    ];
    out.extend(common_questions(spec));
    // Jamais un critère du document entier (longueur totale, nombre de
    // titres) : une partie ne peut pas le tenir seule ; jamais un critère
    // mesurable (compté par programme, [`part_measures`]).
    out.extend(
        spec.criteria
            .iter()
            .filter(|c| c.scope == CriterionScope::Section && brick.criteria.contains(&c.id))
            .filter(|c| measure::of_criterion(c, &spec.language).is_none())
            .map(|c| JudgeQuestion {
                criterion: c.id.clone(),
                text: format!(
                    "Criterion {} (« {} »): is it met, as far as this section is concerned?",
                    c.id, c.text
                ),
                blocking: true,
            }),
    );
    out
}

/// Critères mesurables d'une partie (portée « section », attribués à la
/// partie) : comptés par programme sur elle.
fn part_measures(spec: &Spec, brick: &BrickPlan) -> Vec<(String, Measure)> {
    spec.criteria
        .iter()
        .filter(|c| c.scope == CriterionScope::Section && brick.criteria.contains(&c.id))
        .filter_map(|c| Some((c.id.clone(), measure::of_criterion(c, &spec.language)?)))
        .collect()
}

/// Carte lue de la réponse du modèle.
fn read_card(value: &Value) -> Card {
    let strings = |name: &str| -> Vec<String> {
        value
            .get(name)
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
    let lists = value
        .get("lists")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|l| {
                    let name = l.get("name")?.as_str()?.trim().to_owned();
                    let items: Vec<String> = l
                        .get("items")?
                        .as_array()?
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .collect();
                    (!name.is_empty() && !items.is_empty()).then_some((name, items))
                })
                .collect()
        })
        .unwrap_or_default();
    Card {
        summary: value
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned(),
        terms: strings("terms"),
        decisions: strings("decisions"),
        lists,
        exposes: strings("exposes"),
    }
}

/// Sous-parties lues d'un redécoupage (au plus 3) : identifiants
/// `parent * 100 + k`, mêmes critères, chacune après la précédente.
pub(crate) fn read_split(value: &Value, parent: &BrickPlan) -> Vec<BrickPlan> {
    let Some(parts) = value.get("parts").and_then(Value::as_array) else {
        return Vec::new();
    };
    let parts: Vec<(String, String, u32)> = parts
        .iter()
        .filter_map(|p| {
            // Titre sans marques « ### » (le modèle les donne parfois).
            let title = clean_title(p.get("title")?.as_str()?);
            let goal = p.get("goal")?.as_str()?.trim().to_owned();
            let words = p.get("words").and_then(Value::as_u64).unwrap_or(0);
            (!title.is_empty()).then_some((title, goal, u32::try_from(words).unwrap_or(0)))
        })
        .take(3)
        .collect();
    if parts.len() < 2 {
        return Vec::new();
    }
    let share = parent.words / parts.len() as u32;
    let mut out: Vec<BrickPlan> = Vec::new();
    for (k, (title, goal, words)) in parts.iter().enumerate() {
        let id = parent.id * 100 + k as u32 + 1;
        let mut depends_on = parent.depends_on.clone();
        if let Some(previous) = out.last() {
            depends_on.push(previous.id);
        }
        let mut forbidden = parent.forbidden.clone();
        for (j, (other, _, _)) in parts.iter().enumerate() {
            if j != k {
                // i18n-ignore : consigne au modèle.
                forbidden.push(format!(
                    "the content of « {other} »: another section covers it"
                ));
            }
        }
        out.push(BrickPlan {
            id,
            title: title.clone(),
            goal: goal.clone(),
            kind: BrickKind::Text,
            words: if *words > 0 { *words } else { share.max(100) },
            depends_on,
            criteria: parent.criteria.clone(),
            forbidden,
        });
    }
    out
}

/// Carte d'une partie redécoupée : fusion de celles de ses sous-parties.
fn merged(cards: &[&Card]) -> Card {
    let mut out = Card::default();
    for card in cards {
        if !card.summary.is_empty() {
            if !out.summary.is_empty() {
                out.summary.push(' ');
            }
            out.summary.push_str(&card.summary);
        }
        out.terms.extend(card.terms.iter().cloned());
        out.decisions.extend(card.decisions.iter().cloned());
        out.lists.extend(card.lists.iter().cloned());
        out.exposes.extend(card.exposes.iter().cloned());
    }
    out
}

/// Voisine d'une partie.
fn neighbour(brick: Option<&BrickPlan>) -> Option<Neighbour> {
    brick.map(|b| Neighbour {
        title: b.title.clone(),
        goal: b.goal.clone(),
    })
}

/// Où se trouve une partie : ses sœurs (le plan, ou les sous-parties d'un
/// redécoupage) et, aux deux bords du groupe, les voisines de son parent.
#[derive(Clone, Debug)]
pub(super) struct Scope<'b> {
    pub siblings: &'b [BrickPlan],
    pub before: Option<Neighbour>,
    pub after: Option<Neighbour>,
}

impl<'b> Scope<'b> {
    /// Le plan entier.
    pub fn top(plan: &'b Plan) -> Self {
        Scope {
            siblings: &plan.bricks,
            before: None,
            after: None,
        }
    }

    /// Voisines de `brick` dans l'ordre de lecture ; au bord du groupe,
    /// celles du parent.
    fn neighbours(&self, brick: &BrickPlan) -> (Option<Neighbour>, Option<Neighbour>) {
        let reading = reading_order(self.siblings);
        let at = reading.iter().position(|b| b.id == brick.id).unwrap_or(0);
        let before = at
            .checked_sub(1)
            .and_then(|i| reading.get(i))
            .copied()
            .map_or_else(|| self.before.clone(), |b| neighbour(Some(b)));
        let after = reading
            .get(at + 1)
            .copied()
            .map_or_else(|| self.after.clone(), |b| neighbour(Some(b)));
        (before, after)
    }

    /// Groupe des sous-parties de `parent`.
    fn inside(&self, parent: &BrickPlan, subs: &'b [BrickPlan]) -> Scope<'b> {
        let (before, after) = self.neighbours(parent);
        Scope {
            siblings: subs,
            before,
            after,
        }
    }
}

/// Ce que les contrôles d'une version ont mis en doute : éléments hors
/// registre, affirmations jugées fausses (jamais ajoutés à une liste fixée).
fn doubted(checks: &[Check]) -> Vec<String> {
    checks
        .iter()
        .filter(|c| {
            !c.passed
                && (c.criterion == "outsideRegistry"
                    || (c.method == CheckMethod::FactJudge && c.blocking))
        })
        .filter_map(|c| c.where_.clone())
        .collect()
}

type Boxed<'b, T> = Pin<Box<dyn Future<Output = T> + Send + 'b>>;

impl Run<'_> {
    /// Une version écrite (`key`) ; boucle → un nouvel essai plus chaud,
    /// encore une boucle → `None`.
    pub(super) async fn write_version(
        &self,
        key: StateKey,
        activity: Activity,
        messages: Vec<Message>,
        words: u32,
        budget: u32,
    ) -> Result<Option<String>, Stop> {
        let first = self
            .text(
                key,
                activity.clone(),
                messages.clone(),
                writing(0.7, budget, words),
            )
            .await;
        match first {
            Ok(text) => Ok(Some(text)),
            Err(AgentError::Looping) => {
                self.warn("looping").await;
                match self
                    .text(key, activity, messages, writing(1.0, budget, words))
                    .await
                {
                    Ok(text) => Ok(Some(text)),
                    Err(AgentError::Looping) => Ok(None),
                    Err(other) => Err(other.into()),
                }
            }
            Err(other) => Err(other.into()),
        }
    }

    /// Vérifications d'une version : programme, juge de forme, juge
    /// d'exactitude (avec les éléments hors registre à examiner d'abord).
    #[expect(clippy::too_many_arguments, reason = "une vérification, ses entrées")]
    pub(super) async fn verify(
        &self,
        id: u32,
        version: u8,
        activity: Activity,
        spec: &Spec,
        label: &str,
        text: &str,
        expect: &Expect<'_>,
        questions: &[JudgeQuestion],
    ) -> Result<Vec<Check>, Stop> {
        let (mut found, strays) = checks::program_checks(text, expect);
        let asked: Vec<String> = questions.iter().map(|q| q.text.clone()).collect();
        let form = soft(
            self.json(
                StateKey::FormJudge { id, version },
                activity.clone(),
                prompts::form_judge_call(spec, label, text, &asked),
            )
            .await,
        )?;
        if form.is_none() {
            self.warn("judgeUnavailable").await;
        }
        found.extend(checks::form_checks(questions, form.as_ref()));
        let fact = soft(
            self.json(
                StateKey::FactJudge { id, version },
                activity,
                prompts::fact_judge_call(
                    spec,
                    text,
                    &checks::flagged(&strays),
                    &self.project().attachments,
                    self.attachment_room(),
                ),
            )
            .await,
        )?;
        if fact.is_none() {
            self.warn("judgeUnavailable").await;
        }
        found.extend(checks::fact_checks(fact.as_ref()));
        Ok(found)
    }

    /// Le texte de la partie `id` est un refus (motif, puis juge).
    pub(super) async fn refused(&self, id: u32, request: &str, text: &str) -> Result<bool, Stop> {
        let value = soft(
            self.json(
                StateKey::RefusalJudge { id, version: 0 },
                Activity::CheckingBrick { id },
                prompts::refusal_call(request, text),
            )
            .await,
        )?;
        Ok(value
            .as_ref()
            .and_then(|v| v.get("refusal"))
            .and_then(Value::as_bool)
            // Juge muet : le motif suffit.
            .unwrap_or(true))
    }

    /// Carte de sortie (réflexion coupée) ; illisible : résumé vide.
    async fn card(&self, id: u32, text: &str) -> Result<Card, Stop> {
        if body_of(text).is_empty() {
            return Ok(Card::default());
        }
        let value = soft(
            self.json(
                StateKey::Card(id),
                Activity::SummarizingBrick { id },
                prompts::card_call(text),
            )
            .await,
        )?;
        Ok(value.as_ref().map(read_card).unwrap_or_default())
    }

    /// Plafond des consignes d'une rédaction : [`BRIEF_MAX`], ou moins si le
    /// contexte ne laisse pas la place de la réflexion (`budget`) et de la
    /// réponse (`words`).
    pub(super) fn brief_cap(&self, budget: u32, words: u32) -> usize {
        let answer = ANSWER.max(words.saturating_mul(3));
        let room = self
            .model
            .context()
            .saturating_sub(budget.saturating_add(answer));
        BRIEF_MAX.min(room as usize)
    }

    /// Consignes d'une partie, mesurées avec `extra` jetons (version à
    /// corriger et points relevés : 0 pour la première rédaction) sous
    /// [`Run::brief_cap`] : registre condensé au-delà de [`REGISTRY_MAX`],
    /// puis davantage (cartes réduites à leur résumé, listes bornées) tant
    /// qu'elles dépassent, documents retirés en dernier. Entrées cherchées
    /// dans le plan ENTIER (une sous-partie hérite des dépendances de son
    /// parent).
    #[expect(clippy::too_many_arguments, reason = "une consigne, ses entrées")]
    async fn brief(
        &self,
        spec: &Spec,
        doc_title: &str,
        plan: &Plan,
        scope: &Scope<'_>,
        brick: &BrickPlan,
        progress: &Progress,
        extra: usize,
    ) -> Result<String, Stop> {
        let (before, after) = scope.neighbours(brick);
        let title_of = |id: u32| {
            scope
                .siblings
                .iter()
                .chain(plan.bricks.iter())
                .find(|b| b.id == id)
                .map(|b| b.title.as_str())
        };
        let inputs: Vec<(&str, &Card)> = match brick.kind {
            BrickKind::Text => brick
                .depends_on
                .iter()
                .filter_map(|d| Some((title_of(*d)?, progress.cards.get(d)?)))
                .collect(),
            BrickKind::Intro | BrickKind::Conclusion => plan
                .bricks
                .iter()
                .filter(|b| b.kind == BrickKind::Text)
                .filter_map(|b| Some((b.title.as_str(), progress.cards.get(&b.id)?)))
                .collect(),
        };
        let budget = if brick.kind == BrickKind::Text {
            THINK_PART
        } else {
            THINK_FRAME
        };
        let cap = self.brief_cap(budget, brick.words);
        let focus = format!("{} {}", brick.title, brick.goal);
        let registry_tokens = if progress.registry.is_empty() {
            0
        } else {
            self.tokens(&progress.registry.render(0, &focus)).await?
        };
        let mut level = u8::from(registry_tokens > REGISTRY_MAX);
        if level > 0 {
            self.warn("registryCondensed").await;
        }
        let room = self.attachment_room();
        loop {
            let text = Brief {
                spec,
                doc_title,
                brick,
                registry: &progress.registry,
                level: level.min(MAX_LEVEL),
                inputs: inputs.clone(),
                before: before.clone(),
                after: after.clone(),
                attachments: &self.project().attachments,
                attachment_chars: if level > MAX_LEVEL { 0 } else { room },
            }
            .render();
            let n = self.tokens(&text).await?.saturating_add(extra);
            if n <= cap || level > MAX_LEVEL {
                if n > cap {
                    self.warn("briefTooLong").await;
                }
                return Ok(text);
            }
            level += 1;
        }
    }

    /// Toutes les parties du plan.
    pub(super) async fn build(&self, spec: &Spec, plan: &Plan) -> Result<Progress, Stop> {
        self.phase(Phase::Build, true).await;
        let title = doc_title(plan, spec, &self.project().title);
        let mut progress = Progress::default();
        let scope = Scope::top(plan);
        for id in execution_order(plan) {
            let Some(brick) = plan.bricks.iter().find(|b| b.id == id) else {
                continue;
            };
            self.part(spec, &title, plan, &scope, brick, 0, &mut progress)
                .await?;
        }
        self.phase(Phase::Build, false).await;
        Ok(progress)
    }

    /// Une partie (récursive : redécoupage) ; sa carte entre au registre.
    #[expect(clippy::too_many_arguments, reason = "une partie, son contexte")]
    fn part<'b>(
        &'b self,
        spec: &'b Spec,
        doc_title: &'b str,
        plan: &'b Plan,
        scope: &'b Scope<'b>,
        brick: &'b BrickPlan,
        depth: u8,
        progress: &'b mut Progress,
    ) -> Boxed<'b, Result<(), Stop>> {
        Box::pin(async move {
            let id = brick.id;
            let mut record = match self.load::<BrickRecord>(StateKey::Brick(id)).await {
                Some(record) if record.state == BrickState::Split => {
                    // Reprise : sous-parties relues (toutes sauvées).
                    let subs = self.sub_parts(spec, brick, &record.text, &[]).await?;
                    let inner = scope.inside(brick, &subs);
                    for sub in &subs {
                        self.part(spec, doc_title, plan, &inner, sub, depth + 1, progress)
                            .await?;
                    }
                    record
                }
                Some(record) => record,
                None => {
                    self.make(spec, doc_title, plan, scope, brick, depth, progress)
                        .await?
                }
            };
            let card = if record.state == BrickState::Split {
                let subs: Vec<&Card> = progress
                    .records
                    .iter()
                    .filter(|r| r.id / 100 == id && r.id >= 100)
                    .filter_map(|r| progress.cards.get(&r.id))
                    .collect();
                merged(&subs)
            } else {
                let card = self.card(id, &record.text).await?;
                let conflicts = progress.registry.add(&card, &doubted(&record.checks));
                // Décision contraire au registre (même sujet, autre valeur) :
                // signalée dès l'ajout, et dite au rapport de la partie.
                let mut noted = false;
                for conflict in conflicts {
                    let check = Check {
                        criterion: "registryConflict".into(),
                        method: CheckMethod::Program,
                        passed: false,
                        evidence: format!("{} / {}", conflict.kept, conflict.other),
                        where_: Some(conflict.other),
                        blocking: false,
                    };
                    self.warn("registryConflict").await;
                    if !record.checks.contains(&check) {
                        record.checks.push(check);
                        noted = true;
                    }
                }
                if noted {
                    self.save(StateKey::Brick(id), &record).await;
                }
                progress.written.push((id, card.clone()));
                card
            };
            progress.cards.insert(id, card);
            progress.records.push(record);
            Ok(())
        })
    }

    /// Sous-parties d'un redécoupage (sauvé sous [`StateKey::Split`]).
    async fn sub_parts(
        &self,
        spec: &Spec,
        brick: &BrickPlan,
        text: &str,
        problems: &[String],
    ) -> Result<Vec<BrickPlan>, Stop> {
        let value = soft(
            self.json(
                StateKey::Split(brick.id),
                Activity::SplittingBrick { id: brick.id },
                prompts::split_call(spec, brick, text, problems),
            )
            .await,
        )?;
        Ok(value
            .as_ref()
            .map(|v| read_split(v, brick))
            .unwrap_or_default())
    }

    /// Écrit, vérifie, corrige une fois, redécoupe au besoin.
    #[expect(clippy::too_many_arguments, reason = "une partie, son contexte")]
    async fn make(
        &self,
        spec: &Spec,
        doc_title: &str,
        plan: &Plan,
        scope: &Scope<'_>,
        brick: &BrickPlan,
        depth: u8,
        progress: &mut Progress,
    ) -> Result<BrickRecord, Stop> {
        let id = brick.id;
        let state = |state, attempt| EventKind::Brick { id, state, attempt };
        self.emit(state(BrickState::Running, 0)).await;
        let brief = self
            .brief(spec, doc_title, plan, scope, brick, progress, 0)
            .await?;
        let budget = if brick.kind == BrickKind::Text {
            THINK_PART
        } else {
            THINK_FRAME
        };
        let shaped = |text: &str| match brick.kind {
            BrickKind::Intro => as_intro(text),
            _ => as_section(text, &brick.title),
        };
        let mut record = BrickRecord {
            id,
            title: brick.title.clone(),
            kind: brick.kind,
            state: BrickState::ToReview,
            text: String::new(),
            fixed: false,
            checks: Vec::new(),
        };
        let Some(draft) = self
            .write_version(
                StateKey::Text { id, version: 0 },
                Activity::WritingBrick { id },
                prompts::section_messages(&brief),
                brick.words,
                budget,
            )
            .await?
        else {
            return self.close(record).await;
        };
        let draft = shaped(&draft);
        if looks_like_refusal(body_of(&draft), brick.words)
            && self.refused(id, &brief, &draft).await?
        {
            record.state = BrickState::Refused;
            return self.close(record).await;
        }

        self.emit(state(BrickState::Checking, 0)).await;
        let questions = part_questions(spec, brick);
        let lists = progress.registry.lists.clone();
        let measures = part_measures(spec, brick);
        let expect = Expect {
            words: Some(brick.words),
            tolerance: LENGTH_TOLERANCE,
            language: &spec.language,
            allowed: &self.allowed,
            lists: &lists,
            measures: &measures,
        };
        let activity = Activity::CheckingBrick { id };
        let first = self
            .verify(
                id,
                0,
                activity.clone(),
                spec,
                "SECTION",
                &draft,
                &expect,
                &questions,
            )
            .await?;
        self.emit(EventKind::BrickChecks {
            id,
            attempt: 0,
            checks: first.clone(),
        })
        .await;
        record.text.clone_from(&draft);
        record.checks.clone_from(&first);

        if checks::noes(&first) > 0 {
            self.emit(state(BrickState::Fixing, 1)).await;
            let problems = checks::problems(&first, &questions, Some(brick.words));
            record.fixed = true;
            // La correction est mesurée aussi : consignes, version et points
            // relevés sous le même plafond (consignes condensées au besoin).
            let extra = self.tokens(&draft).await? + self.tokens(&problems.join("\n")).await?;
            let fix_brief = self
                .brief(spec, doc_title, plan, scope, brick, progress, extra)
                .await?;
            let fixed = self
                .write_version(
                    StateKey::Text { id, version: 1 },
                    Activity::FixingBrick { id },
                    prompts::fix_messages(&fix_brief, &draft, &problems),
                    brick.words,
                    budget,
                )
                .await?;
            if let Some(fixed) = fixed {
                let fixed = shaped(&fixed);
                self.emit(state(BrickState::Checking, 1)).await;
                let mut second = self
                    .verify(
                        id, 1, activity, spec, "SECTION", &fixed, &expect, &questions,
                    )
                    .await?;
                // Remarque du juge recopiée : signalée (non bloquante).
                second.extend(checks::copied_remarks(&first, &draft, &fixed));
                self.emit(EventKind::BrickChecks {
                    id,
                    attempt: 1,
                    checks: second.clone(),
                })
                .await;
                // La correction n'est gardée que si elle a MOINS de « non ».
                if checks::noes(&second) < checks::noes(&first) {
                    record.text = fixed;
                    record.checks = second;
                }
            }
        }

        // Redécoupage : seulement pour un critère de fond non tenu (ou trop
        // d'affirmations fausses pour une passe chirurgicale), dans une
        // partie assez longue pour être découpée, au premier niveau.
        let wrong = checks::wrong_claims(&record.checks);
        if (checks::form_left(&record.checks) || wrong > MAX_SURGERY)
            && brick.words >= MIN_SPLIT_WORDS
            && depth < MAX_DEPTH
            && brick.kind == BrickKind::Text
        {
            let problems = checks::problems(&record.checks, &questions, Some(brick.words));
            let subs = self.sub_parts(spec, brick, &record.text, &problems).await?;
            if !subs.is_empty() {
                self.emit(state(BrickState::Split, 0)).await;
                let inner = scope.inside(brick, &subs);
                for sub in &subs {
                    self.part(spec, doc_title, plan, &inner, sub, depth + 1, progress)
                        .await?;
                }
                let mut text = format!("## {}", brick.title);
                for sub in &subs {
                    let child = progress.records.iter().find(|r| r.id == sub.id);
                    if let Some(child) = child.filter(|c| !body_of(&c.text).is_empty()) {
                        text.push_str("\n\n");
                        text.push_str(&demoted(&child.text));
                    }
                }
                record.text = text;
                record.state = BrickState::Split;
                record.checks = Vec::new();
                return self.close(record).await;
            }
        }
        if (1..=MAX_SURGERY).contains(&wrong) {
            // Quelques affirmations fausses : seules ces phrases reprises.
            self.emit(state(BrickState::Fixing, 2)).await;
            let found = std::mem::take(&mut record.checks);
            let (text, after) = self.surgery(id, spec, &record.text, found).await?;
            record.text = text;
            record.checks = after;
            self.emit(EventKind::BrickChecks {
                id,
                attempt: 2,
                checks: record.checks.clone(),
            })
            .await;
        }
        let failing = record.checks.iter().any(|c| c.blocking && !c.passed);
        let unverified = record.checks.iter().any(checks::is_unavailable);
        record.state = if failing || unverified {
            BrickState::ToReview
        } else {
            BrickState::Done
        };
        self.close(record).await
    }

    /// Sauve une partie finie et l'annonce.
    async fn close(&self, record: BrickRecord) -> Result<BrickRecord, Stop> {
        self.save(StateKey::Brick(record.id), &record).await;
        self.emit(EventKind::Brick {
            id: record.id,
            state: record.state,
            attempt: u32::from(record.fixed),
        })
        .await;
        Ok(record)
    }
}

/// Mots du corps d'une section.
pub(crate) fn body_words(section: &str) -> u32 {
    words(body_of(section))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workshop::pipeline::checks::tests::good_plan;

    #[test]
    fn frames_come_last_and_dependencies_first() {
        let mut plan = good_plan();
        // La section 2 dépend de la 3, placée après elle dans le plan.
        plan.bricks[1].depends_on = vec![3];
        plan.bricks[2].depends_on = vec![];
        assert_eq!(execution_order(&plan), vec![3, 2, 1, 4]);
        let reading: Vec<u32> = reading_order(&plan.bricks).iter().map(|b| b.id).collect();
        assert_eq!(reading, vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_split_gives_numbered_sub_parts() {
        let parent = &good_plan().bricks[2];
        let value = serde_json::json!({"analysis": "", "parts": [
            {"title": "Été", "goal": "a", "words": 200},
            {"title": "Hiver", "goal": "b", "words": 0},
        ]});
        let subs = read_split(&value, parent);
        assert_eq!(
            subs.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![301, 302]
        );
        assert_eq!(subs[1].depends_on, vec![2, 301]);
        assert_eq!(subs[1].words, 200);
        assert!(subs[0].forbidden[0].contains("Hiver"));
        assert!(
            read_split(
                &serde_json::json!({"parts": [{"title": "x", "goal": "y", "words": 1}]}),
                parent
            )
            .is_empty()
        );
        assert_eq!(demoted("## A\n\ntexte\n### B"), "### A\n\ntexte\n#### B");
        // Banc du 25 nov. : titres « ### … » rendus par le redécoupage.
        let value = serde_json::json!({"analysis": "", "parts": [
            {"title": "### Choisis un emplacement", "goal": "a", "words": 100},
            {"title": "## #### Évite l'ombre", "goal": "b", "words": 100},
        ]});
        let subs = read_split(&value, parent);
        assert_eq!(subs[0].title, "Choisis un emplacement");
        assert_eq!(subs[1].title, "Évite l'ombre");
    }
}
