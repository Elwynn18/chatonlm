//! Parties d'un projet de l'Atelier telles que la page les montre (lot 3,
//! commandes `workshop_parts` et `workshop_part`) : relues de l'état sauvé par
//! le pipeline (plan, parties finies, textes, redécoupages, rapport), le
//! journal ne servant qu'à ce que l'état ne dit pas encore (partie en cours,
//! essai, vérifications d'une version pas encore gardée). La page n'a donc
//! pas à tout reconstruire des événements.
//!
//! Version directe (d'un seul jet) : une seule partie, d'identifiant 0,
//! montrée tant qu'elle est tentée ou qu'elle a été livrée ; abandonnée
//! (`directFailed`), les parties du plan la remplacent. Sous-parties d'un
//! redécoupage : juste après leur parent, avec `parent`.
//!
//! Logique pure (lecteur d'état injecté), commune à Linux et Windows.

use std::collections::HashMap;
use std::path::{Component, Path};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use ts_rs::TS;

use super::pipeline::types::{
    Activity, BrickKind, BrickPlan, BrickState, Check, Event, EventKind, Plan, Report, Route, Spec,
    StateKey,
};
use super::pipeline::{BrickRecord, body_words, read_split, words};

/// Texte d'une partie rendu au plus (caractères).
pub const MAX_PART_CHARS: usize = 200_000;

/// Profondeur des redécoupages suivie (le pipeline n'en fait qu'un niveau).
const MAX_SPLIT_DEPTH: u8 = 2;

/// Une partie dans la vue d'un projet (commande `workshop_parts`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkshopPartSummary {
    /// Identifiant dans le plan (sous-partie : `parent * 100 + k`) ; 0 : la
    /// version directe (« Version d'un seul jet »).
    pub id: u32,
    /// Partie redécoupée dont c'est une sous-partie.
    pub parent: Option<u32>,
    /// Titre de la partie ; version directe : titre du projet.
    pub title: String,
    pub kind: BrickKind,
    /// Mots visés (`None` : version directe sans longueur demandée).
    pub target_words: Option<u32>,
    pub state: BrickState,
    /// Essai le plus avancé : 0 première version, 1 correction, 2 passe
    /// sur les phrases fausses.
    pub attempt: u32,
    /// Mots écrits (dernier texte sauvé ; 0 : rien encore).
    pub words: u32,
    pub depends_on: Vec<u32>,
    /// Critères de la spécification qu'elle sert (« C1 »…).
    pub criteria: Vec<String>,
}

/// Une partie en détail (commande `workshop_part`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkshopPart {
    pub id: u32,
    pub parent: Option<u32>,
    pub title: String,
    pub kind: BrickKind,
    pub target_words: Option<u32>,
    pub state: BrickState,
    pub attempt: u32,
    /// Mots écrits.
    pub words: u32,
    /// Dernier texte sauvé (Markdown), au plus [`MAX_PART_CHARS`]
    /// caractères ; version directe livrée : le livrable.
    pub text: String,
    /// Texte coupé à [`MAX_PART_CHARS`].
    pub truncated: bool,
    /// Vérifications de la version gardée (ou de la dernière vérifiée).
    pub checks: Vec<Check>,
}

/// Ce que le journal dit des parties.
#[derive(Default)]
struct History {
    /// Par partie : dernier état annoncé, essai le plus avancé, dernières
    /// vérifications.
    bricks: HashMap<u32, Seen>,
    /// Version directe tentée (et pas abandonnée depuis).
    direct: bool,
    direct_state: Option<BrickState>,
    direct_attempt: u32,
    /// Versions directes vérifiées (0, 1 : correction).
    direct_checked: u32,
    direct_checks: Option<Vec<Check>>,
}

#[derive(Default)]
struct Seen {
    state: Option<BrickState>,
    attempt: u32,
    checks: Option<Vec<Check>>,
}

impl History {
    fn read(events: &[Event]) -> Self {
        let mut history = History::default();
        for event in events {
            match &event.kind {
                EventKind::Brick { id, state, attempt } => {
                    let seen = history.bricks.entry(*id).or_default();
                    seen.state = Some(*state);
                    seen.attempt = seen.attempt.max(*attempt);
                }
                // Id 0 : version directe, ou vérification d'ensemble du
                // livrable assemblé (route par parties : jamais montrée,
                // `direct` restant faux).
                EventKind::BrickChecks {
                    id: 0,
                    attempt,
                    checks,
                } => {
                    history.direct_checked = history.direct_checked.max(attempt + 1);
                    history.direct_attempt = history.direct_attempt.max(*attempt);
                    history.direct_checks = Some(checks.clone());
                }
                EventKind::BrickChecks {
                    id,
                    attempt,
                    checks,
                } => {
                    let seen = history.bricks.entry(*id).or_default();
                    seen.attempt = seen.attempt.max(*attempt);
                    seen.checks = Some(checks.clone());
                }
                EventKind::Activity { activity } => history.activity(activity),
                EventKind::Warning { code } if code == "directFailed" => {
                    history.direct = false;
                    history.direct_state = None;
                    history.direct_checks = None;
                }
                _ => {}
            }
        }
        history
    }

    fn activity(&mut self, activity: &Activity) {
        match activity {
            Activity::WritingDirect => {
                self.direct = true;
                // Après une première vérification : c'est la correction.
                if self.direct_checked > 0 {
                    self.direct_state = Some(BrickState::Fixing);
                    self.direct_attempt = self.direct_attempt.max(1);
                } else {
                    self.direct_state = Some(BrickState::Running);
                }
            }
            Activity::CheckingDirect if self.direct => {
                self.direct_state = Some(BrickState::Checking);
            }
            Activity::FixingSentences { id: 0 } if self.direct => {
                self.direct_state = Some(BrickState::Fixing);
                self.direct_attempt = 2;
            }
            _ => {}
        }
    }
}

/// Lecteur de l'état sauvé : nom de clé (`StateKey::name`) → JSON.
pub(crate) type Load<'a> = dyn FnMut(&str) -> Option<Value> + 'a;

fn typed<T: DeserializeOwned>(load: &mut Load<'_>, key: StateKey) -> Option<T> {
    serde_json::from_value(load(&key.name())?).ok()
}

/// Dernier texte sauvé d'une partie (correction d'abord).
fn latest_text(load: &mut Load<'_>, id: u32) -> Option<String> {
    [1, 0]
        .into_iter()
        .find_map(|version| typed(load, StateKey::Text { id, version }))
}

fn count(kind: BrickKind, text: &str) -> u32 {
    match kind {
        BrickKind::Intro => words(text),
        _ => body_words(text),
    }
}

/// Une partie relue : résumé, texte, vérifications.
struct Found {
    summary: WorkshopPartSummary,
    text: String,
    checks: Vec<Check>,
}

/// Tout ce qui est lu pour une vue.
struct Reading {
    found: Vec<Found>,
    /// Livrable de la version directe livrée (relatif au dossier).
    delivered: Option<String>,
}

fn brick(
    load: &mut Load<'_>,
    history: &History,
    plan: &BrickPlan,
    parent: Option<u32>,
    depth: u8,
    out: &mut Vec<Found>,
) {
    let id = plan.id;
    let record: Option<BrickRecord> = typed(load, StateKey::Brick(id));
    let seen = history.bricks.get(&id);
    let state = record
        .as_ref()
        .map(|r| r.state)
        .or_else(|| seen.and_then(|s| s.state))
        .unwrap_or(BrickState::Pending);
    let attempt = seen
        .map_or(0, |s| s.attempt)
        .max(record.as_ref().map_or(0, |r| u32::from(r.fixed)));
    let (text, checks) = match &record {
        Some(record) => (record.text.clone(), record.checks.clone()),
        None => (
            latest_text(load, id).unwrap_or_default(),
            seen.and_then(|s| s.checks.clone()).unwrap_or_default(),
        ),
    };
    out.push(Found {
        summary: WorkshopPartSummary {
            id,
            parent,
            title: plan.title.clone(),
            kind: plan.kind,
            target_words: Some(plan.words),
            state,
            attempt,
            words: count(plan.kind, &text),
            depends_on: plan.depends_on.clone(),
            criteria: plan.criteria.clone(),
        },
        text,
        checks,
    });
    if state == BrickState::Split && depth < MAX_SPLIT_DEPTH {
        let Some(value) = load(&StateKey::Split(id).name()) else {
            return;
        };
        for sub in read_split(&value, plan) {
            brick(load, history, &sub, Some(id), depth + 1, out);
        }
    }
}

fn read(load: &mut Load<'_>, events: &[Event], title: &str) -> Reading {
    let history = History::read(events);
    let report: Option<Report> = typed(load, StateKey::Report);
    let direct = match &report {
        Some(report) => report.route == Route::Direct,
        None => history.direct,
    };
    if direct {
        let spec: Option<Spec> = typed(load, StateKey::Spec);
        let (state, checks, written, delivered) = match &report {
            Some(report) => {
                let failing = report.checks.iter().any(|c| c.blocking && !c.passed);
                let state = if failing || report.to_review > 0 {
                    BrickState::ToReview
                } else {
                    BrickState::Done
                };
                (
                    state,
                    report.checks.clone(),
                    Some(report.words),
                    Some(report.file.clone()),
                )
            }
            None => (
                history.direct_state.unwrap_or(BrickState::Running),
                history.direct_checks.clone().unwrap_or_default(),
                None,
                None,
            ),
        };
        let text = latest_text(load, 0).unwrap_or_default();
        let summary = WorkshopPartSummary {
            id: 0,
            parent: None,
            title: title.to_owned(),
            kind: BrickKind::Text,
            target_words: spec.as_ref().and_then(|s| s.length_words),
            state,
            attempt: history.direct_attempt,
            words: written.unwrap_or_else(|| words(&text)),
            depends_on: Vec::new(),
            criteria: spec
                .map(|s| s.criteria.into_iter().map(|c| c.id).collect())
                .unwrap_or_default(),
        };
        return Reading {
            found: vec![Found {
                summary,
                text,
                checks,
            }],
            delivered,
        };
    }
    let mut found = Vec::new();
    if let Some(plan) = typed::<Plan>(load, StateKey::Plan) {
        for part in &plan.bricks {
            brick(load, &history, part, None, 0, &mut found);
        }
    }
    Reading {
        found,
        delivered: None,
    }
}

/// Parties du projet, dans l'ordre du plan (sous-parties après leur parent).
pub(crate) fn summaries(
    load: &mut Load<'_>,
    events: &[Event],
    title: &str,
) -> Vec<WorkshopPartSummary> {
    read(load, events, title)
        .found
        .into_iter()
        .map(|f| f.summary)
        .collect()
}

/// Une partie en détail ; `None` : inconnue. Le second élément est le
/// livrable à relire à la place du texte (version directe livrée).
pub(crate) fn detail(
    load: &mut Load<'_>,
    events: &[Event],
    title: &str,
    id: u32,
) -> Option<(WorkshopPart, Option<String>)> {
    let Reading { found, delivered } = read(load, events, title);
    let found = found.into_iter().find(|f| f.summary.id == id)?;
    let s = found.summary;
    let (text, truncated) = clip(found.text);
    Some((
        WorkshopPart {
            id: s.id,
            parent: s.parent,
            title: s.title,
            kind: s.kind,
            target_words: s.target_words,
            state: s.state,
            attempt: s.attempt,
            words: s.words,
            text,
            truncated,
            checks: found.checks,
        },
        delivered.filter(|_| id == 0),
    ))
}

/// Texte borné à [`MAX_PART_CHARS`] ; vrai s'il a été coupé.
pub(crate) fn clip(text: String) -> (String, bool) {
    match text.char_indices().nth(MAX_PART_CHARS) {
        Some((cut, _)) => (text[..cut].to_owned(), true),
        None => (text, false),
    }
}

/// Livrable relatif au dossier du projet, sans sortir du dossier.
pub(crate) fn delivered_path(folder: &Path, file: &str) -> Option<std::path::PathBuf> {
    let rel = Path::new(file);
    let plain = !file.is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)));
    plain.then(|| folder.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workshop::pipeline::types::{CheckMethod, Criterion, CriterionScope, Register};
    use serde_json::json;

    fn plan() -> Plan {
        let part = |id: u32, kind, words, depends_on: Vec<u32>| BrickPlan {
            id,
            title: format!("Partie {id}"),
            goal: "but".into(),
            kind,
            words,
            depends_on,
            criteria: vec!["C1".into()],
            forbidden: Vec::new(),
        };
        Plan {
            title: "Guide".into(),
            bricks: vec![
                part(1, BrickKind::Intro, 150, vec![]),
                part(2, BrickKind::Text, 900, vec![]),
                part(3, BrickKind::Text, 600, vec![2]),
            ],
        }
    }

    fn check(passed: bool) -> Check {
        Check {
            criterion: "C1".into(),
            method: CheckMethod::FormJudge,
            passed,
            evidence: "« … »".into(),
            where_: None,
            blocking: true,
        }
    }

    fn event(seq: u64, kind: EventKind) -> Event {
        Event {
            seq,
            at_ms: 0,
            kind,
        }
    }

    fn states(pairs: Vec<(StateKey, Value)>) -> HashMap<String, Value> {
        pairs.into_iter().map(|(k, v)| (k.name(), v)).collect()
    }

    fn summaries_of(state: &HashMap<String, Value>, events: &[Event]) -> Vec<WorkshopPartSummary> {
        summaries(&mut |key: &str| state.get(key).cloned(), events, "Projet")
    }

    fn spec() -> Spec {
        Spec {
            goal: "g".into(),
            title: String::new(),
            deliverables: vec![],
            criteria: vec![Criterion {
                id: "C1".into(),
                text: "t".into(),
                scope: CriterionScope::Document,
            }],
            audience: "a".into(),
            language: "fr".into(),
            register: Register::Informal,
            units: "SI".into(),
            length_words: Some(1_200),
            glossary: vec![],
            assumptions: vec![],
            constraints: vec![],
        }
    }

    #[test]
    fn parts_come_from_the_saved_state_and_the_journal() {
        let record = json!({"id": 2, "title": "Partie 2", "kind": "text", "state": "toReview",
            "text": "## Partie 2\n\nun deux trois", "fixed": true, "checks": [check(false)]});
        let state = states(vec![
            (StateKey::Plan, serde_json::to_value(plan()).unwrap()),
            (StateKey::Brick(2), record),
            (
                StateKey::Text { id: 3, version: 0 },
                json!("## Partie 3\n\nquatre cinq"),
            ),
        ]);
        let events = vec![
            event(
                1,
                EventKind::Brick {
                    id: 2,
                    state: BrickState::Running,
                    attempt: 0,
                },
            ),
            event(
                2,
                EventKind::Brick {
                    id: 2,
                    state: BrickState::ToReview,
                    attempt: 1,
                },
            ),
            event(
                3,
                EventKind::Brick {
                    id: 3,
                    state: BrickState::Running,
                    attempt: 0,
                },
            ),
            event(
                4,
                EventKind::BrickChecks {
                    id: 3,
                    attempt: 0,
                    checks: vec![check(true)],
                },
            ),
            event(
                5,
                EventKind::Brick {
                    id: 3,
                    state: BrickState::Fixing,
                    attempt: 1,
                },
            ),
        ];
        let parts = summaries_of(&state, &events);
        let ids: Vec<u32> = parts.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
        assert_eq!(parts[0].state, BrickState::Pending);
        assert_eq!(parts[0].words, 0);
        assert_eq!(parts[1].state, BrickState::ToReview);
        assert_eq!((parts[1].attempt, parts[1].words), (1, 3));
        // Partie en cours : état du journal, texte sauvé, essai annoncé.
        assert_eq!(parts[2].state, BrickState::Fixing);
        assert_eq!((parts[2].attempt, parts[2].words), (1, 2));
        assert_eq!(parts[2].depends_on, vec![2]);
        assert_eq!(parts[2].target_words, Some(600));

        let mut load = |key: &str| state.get(key).cloned();
        let (part, file) = detail(&mut load, &events, "Projet", 3).unwrap();
        assert_eq!(part.checks, vec![check(true)]);
        assert!(part.text.contains("quatre"));
        assert_eq!(file, None);
        let (part, _) = detail(&mut load, &events, "Projet", 2).unwrap();
        assert_eq!(part.checks, vec![check(false)]);
        assert!(detail(&mut load, &events, "Projet", 9).is_none());
    }

    #[test]
    fn split_parts_follow_their_parent() {
        let split = json!({"analysis": "", "parts": [
            {"title": "Été", "goal": "a", "words": 300},
            {"title": "Hiver", "goal": "b", "words": 300},
        ]});
        let state = states(vec![
            (StateKey::Plan, serde_json::to_value(plan()).unwrap()),
            (StateKey::Split(2), split),
            (
                StateKey::Brick(201),
                json!({"id": 201, "title": "Été", "kind": "text", "state": "done",
                    "text": "## Été\n\nchaud", "fixed": false, "checks": []}),
            ),
        ]);
        // Le parent n'est sauvé qu'à la fin de ses sous-parties : son état
        // vient du journal pendant le redécoupage.
        let events = vec![event(
            1,
            EventKind::Brick {
                id: 2,
                state: BrickState::Split,
                attempt: 0,
            },
        )];
        let parts = summaries_of(&state, &events);
        let ids: Vec<(u32, Option<u32>)> = parts.iter().map(|p| (p.id, p.parent)).collect();
        assert_eq!(
            ids,
            vec![
                (1, None),
                (2, None),
                (201, Some(2)),
                (202, Some(2)),
                (3, None)
            ]
        );
        assert_eq!(parts[2].state, BrickState::Done);
        assert_eq!(parts[3].state, BrickState::Pending);
        assert_eq!(parts[3].title, "Hiver");
    }

    #[test]
    fn the_direct_version_is_part_zero_until_abandoned() {
        let state = states(vec![
            (StateKey::Spec, serde_json::to_value(spec()).unwrap()),
            (
                StateKey::Text { id: 0, version: 0 },
                json!("# Guide\n\nun deux trois"),
            ),
        ]);
        let mut events = vec![
            event(
                1,
                EventKind::Activity {
                    activity: Activity::WritingDirect,
                },
            ),
            event(
                2,
                EventKind::Activity {
                    activity: Activity::CheckingDirect,
                },
            ),
            event(
                3,
                EventKind::BrickChecks {
                    id: 0,
                    attempt: 0,
                    checks: vec![check(false)],
                },
            ),
            event(
                4,
                EventKind::Activity {
                    activity: Activity::WritingDirect,
                },
            ),
        ];
        let parts = summaries_of(&state, &events);
        assert_eq!(parts.len(), 1);
        let direct = &parts[0];
        assert_eq!(
            (direct.id, direct.state, direct.attempt),
            (0, BrickState::Fixing, 1)
        );
        assert_eq!(direct.title, "Projet");
        assert_eq!(direct.target_words, Some(1_200));
        assert_eq!(direct.criteria, vec!["C1".to_owned()]);
        assert_eq!(direct.words, 4);

        // Abandonnée : le plan prend la place, et la vérification
        // d'ensemble du livrable assemblé (id 0) n'est pas une partie.
        let mut state = state;
        state.insert(StateKey::Plan.name(), serde_json::to_value(plan()).unwrap());
        events.push(event(
            5,
            EventKind::Warning {
                code: "directFailed".into(),
            },
        ));
        events.push(event(
            6,
            EventKind::BrickChecks {
                id: 0,
                attempt: 0,
                checks: vec![],
            },
        ));
        let parts = summaries_of(&state, &events);
        assert_eq!(
            parts.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn a_delivered_direct_version_reads_the_report() {
        let report = Report {
            strategy: crate::workshop::pipeline::types::Strategy::Generic,
            route: Route::Direct,
            file: "guide.md".into(),
            words: 1_180,
            target_words: Some(1_200),
            checks: vec![check(true)],
            judged_on_outline: false,
            bricks: vec![],
            not_checked: vec![],
            warnings: vec![],
            doubtful: 0,
            to_review: 0,
            proofread_blocks: 0,
            proofread_changed: 0,
            proofread_rejected: 0,
        };
        let state = states(vec![(
            StateKey::Report,
            serde_json::to_value(&report).unwrap(),
        )]);
        let parts = summaries_of(&state, &[]);
        assert_eq!((parts[0].state, parts[0].words), (BrickState::Done, 1_180));
        let mut load = |key: &str| state.get(key).cloned();
        let (part, file) = detail(&mut load, &[], "Projet", 0).unwrap();
        assert_eq!(file.as_deref(), Some("guide.md"));
        assert_eq!(part.checks, vec![check(true)]);
        let mut failing = report;
        failing.checks = vec![check(false)];
        let state = states(vec![(
            StateKey::Report,
            serde_json::to_value(&failing).unwrap(),
        )]);
        assert_eq!(summaries_of(&state, &[])[0].state, BrickState::ToReview);
    }

    #[test]
    fn texts_are_clipped_and_paths_stay_inside() {
        let long = "é".repeat(MAX_PART_CHARS + 3);
        let (text, cut) = clip(long);
        assert!(cut);
        assert_eq!(text.chars().count(), MAX_PART_CHARS);
        assert_eq!(clip("court".into()), ("court".into(), false));
        let folder = Path::new("projets/guide");
        assert!(delivered_path(folder, "guide.md").is_some());
        assert!(delivered_path(folder, "../x.md").is_none());
        assert!(delivered_path(folder, "/etc/passwd").is_none());
        assert!(delivered_path(folder, "").is_none());
    }
}
