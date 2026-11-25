//! Vérifications : contrôles par programme (spécification, plan, texte),
//! lecture des verdicts des juges, décompte des « non ». Logique pure.

use serde_json::Value;

use super::measure::{self, Measure, Unit};
use super::text::{
    Stray, body_of, copied_runs, folded, internal_phrases, internal_words, jaccard,
    out_of_registry, undecided, words,
};
use super::types::{BrickKind, Check, CheckMethod, CriterionScope, Plan, Spec, Strategy};

/// Défaut trouvé par programme : code stable (événements, page) et détail
/// en anglais (rendu au modèle pour l'essai suivant).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Defect {
    pub code: &'static str,
    pub detail: String,
}

impl Defect {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// Écart de longueur toléré pour une partie (±25 %).
pub const LENGTH_TOLERANCE: f64 = 0.25;
/// Écart de longueur toléré pour le livrable entier, version directe ou
/// document assemblé (±10 %) : au-delà, la version directe est corrigée
/// (allonger, raccourcir) ; jusqu'à [`DIRECT_LENGTH_TOLERANCE`], elle est
/// livrée et l'écart dit au rapport.
pub const WHOLE_LENGTH_TOLERANCE: f64 = 0.10;
/// Écart toléré entre la somme des parties du plan et la longueur (±20 %).
pub const PLAN_LENGTH_TOLERANCE: f64 = 0.20;
/// Recouvrement (Jaccard titre + but) au-delà duquel deux parties ont le
/// même objectif (lot 0).
pub const OVERLAP: f64 = 0.45;
/// Longueur d'une partie écrite d'un jet, au plus (1 500 demandés au plan,
/// tolérance comprise).
pub const MAX_PART_WORDS: u32 = 1_800;
/// Parties d'un plan, au plus (schéma du plan).
pub const MAX_PARTS: u32 = 20;
/// Longueur demandée à une partie du plan, au plus.
pub const PART_WORDS: u32 = 1_500;
/// Longueur d'un livrable, au plus : 20 parties de 1 500 mots. Au-delà, la
/// spécification est bornée (avertissement `lengthCapped`).
pub const MAX_LENGTH_WORDS: u32 = MAX_PARTS * PART_WORDS;
/// Écart de longueur au-delà duquel une version directe ne vaut pas d'être
/// gardée (±35 %) : à l'intérieur, l'écart est dit au rapport.
pub const DIRECT_LENGTH_TOLERANCE: f64 = 0.35;
/// Affirmations fausses qu'une passe chirurgicale corrige, au plus : au-delà,
/// le texte est trop fautif pour être repris phrase à phrase.
pub const MAX_SURGERY: usize = 5;
/// Suite de mots de la remarque du juge recopiée dans une correction, au
/// moins (signalée, non bloquante).
pub const COPIED_WORDS: usize = 8;

// i18n-ignore (détails des défauts) : rendus au modèle, en anglais.

/// Défauts de la spécification : rien d'indécis, rien d'interne au moteur
/// dans ce que verra le rédacteur.
pub fn spec_defects(spec: &Spec, strategy: Strategy, allowed: &str) -> Vec<Defect> {
    let mut out = Vec::new();
    let code_ok = spec.language.len() == 2 && spec.language.chars().all(|c| c.is_ascii_lowercase());
    if !code_ok {
        out.push(Defect::new(
            "language",
            format!(
                "\"language\" must be a two-letter ISO 639-1 code, not \"{}\"",
                spec.language
            ),
        ));
    }
    if let Some(word) = undecided(&spec.units) {
        out.push(Defect::new(
            "undecided",
            format!("\"units\" is not decided (it contains \"{word}\"): choose one system"),
        ));
    }
    if strategy == Strategy::LongForm
        && !spec
            .length_words
            .is_some_and(|n| (100..=MAX_LENGTH_WORDS).contains(&n))
    {
        out.push(Defect::new(
            "length",
            "\"length_words\" must give the length of the text in words",
        ));
    }
    if spec.criteria.is_empty() {
        out.push(Defect::new(
            "criteria",
            "give at least 4 acceptance criteria",
        ));
    }
    let mut seen = vec![spec.goal.clone(), spec.audience.clone(), spec.units.clone()];
    seen.extend(spec.deliverables.iter().cloned());
    seen.extend(spec.criteria.iter().map(|c| c.text.clone()));
    seen.extend(spec.glossary.iter().map(|(t, d)| format!("{t} {d}")));
    seen.extend(spec.assumptions.iter().cloned());
    seen.extend(spec.constraints.iter().cloned());
    let found = internal_phrases(&seen.join("\n"), &spec.language, allowed);
    if !found.is_empty() {
        out.push(Defect::new(
            "internalWord",
            format!(
                "do not use these words, meaningless to the reader: {}",
                found.join(", ")
            ),
        ));
    }
    out
}

/// Défauts d'un plan (lot 0 : les contrôles par programme voient 8 défauts
/// injectés sur 8).
pub fn plan_defects(plan: &Plan, spec: &Spec, strategy: Strategy, allowed: &str) -> Vec<Defect> {
    let mut out = Vec::new();
    let ids: Vec<u32> = plan.bricks.iter().map(|b| b.id).collect();
    for (i, id) in ids.iter().enumerate() {
        if !(1..=99).contains(id) {
            out.push(Defect::new(
                "badId",
                format!("part id {id} must be an integer from 1 to 99"),
            ));
        }
        if ids[..i].contains(id) {
            out.push(Defect::new(
                "duplicateId",
                format!("two parts have the id {id}"),
            ));
        }
    }
    let body: Vec<_> = plan
        .bricks
        .iter()
        .filter(|b| b.kind == BrickKind::Text)
        .collect();
    if body.is_empty() {
        out.push(Defect::new(
            "noParts",
            "the plan has no part of kind \"text\"",
        ));
    }
    let frame = |id: u32| {
        plan.bricks
            .iter()
            .any(|b| b.id == id && b.kind != BrickKind::Text)
    };
    for brick in &plan.bricks {
        for dep in &brick.depends_on {
            if *dep == brick.id {
                out.push(Defect::new(
                    "selfDependency",
                    format!("part {} depends on itself", brick.id),
                ));
            } else if !ids.contains(dep) {
                out.push(Defect::new(
                    "unknownDependency",
                    format!(
                        "part {} depends on part {dep}, which does not exist",
                        brick.id
                    ),
                ));
            } else if brick.kind == BrickKind::Text && frame(*dep) {
                out.push(Defect::new(
                    "dependsOnFrame",
                    format!(
                        "part {} depends on part {dep}, an intro or conclusion written last",
                        brick.id
                    ),
                ));
            }
        }
        if brick.words == 0 {
            out.push(Defect::new(
                "zeroWords",
                format!("part {} has no length", brick.id),
            ));
        } else if brick.words > MAX_PART_WORDS {
            out.push(Defect::new(
                "tooLong",
                format!(
                    "part {} has {} words: at most {MAX_PART_WORDS} can be written in one go",
                    brick.id, brick.words
                ),
            ));
        }
    }
    if has_cycle(plan) {
        out.push(Defect::new("cycle", "the dependencies form a cycle"));
    }
    // Un critère du document entier n'est jamais attribué à une partie (il
    // est retiré du plan avant ces contrôles, `strip_document_criteria`).
    for criterion in spec
        .criteria
        .iter()
        .filter(|c| c.scope == CriterionScope::Section)
    {
        if !plan
            .bricks
            .iter()
            .any(|b| b.criteria.contains(&criterion.id))
        {
            out.push(Defect::new(
                "uncoveredCriterion",
                format!("criterion {} is cited by no part", criterion.id),
            ));
        }
    }
    for brick in &plan.bricks {
        for criterion in &brick.criteria {
            if !spec.criteria.iter().any(|c| &c.id == criterion) {
                out.push(Defect::new(
                    "unknownCriterion",
                    format!(
                        "part {} cites criterion {criterion}, which does not exist",
                        brick.id
                    ),
                ));
            }
        }
    }
    for (i, a) in body.iter().enumerate() {
        for b in body.iter().skip(i + 1) {
            let overlap = jaccard(
                &format!("{} {}", a.title, a.goal),
                &format!("{} {}", b.title, b.goal),
            );
            if overlap >= OVERLAP {
                out.push(Defect::new(
                    "overlap",
                    format!("parts {} and {} have the same goal", a.id, b.id),
                ));
            }
        }
    }
    if let Some(total) = spec.length_words {
        let sum: u32 = plan.bricks.iter().map(|b| b.words).sum();
        let ratio = f64::from(sum) / f64::from(total.max(1));
        if (ratio - 1.0).abs() > PLAN_LENGTH_TOLERANCE {
            out.push(Defect::new(
                "lengthMismatch",
                format!("the parts add up to {sum} words for a total of about {total}"),
            ));
        }
    }
    if strategy == Strategy::LongForm {
        for (kind, code, name) in [
            (BrickKind::Intro, "missingIntro", "intro"),
            (BrickKind::Conclusion, "missingConclusion", "conclusion"),
        ] {
            let count = plan.bricks.iter().filter(|b| b.kind == kind).count();
            if count != 1 {
                out.push(Defect::new(
                    code,
                    format!("the plan must have exactly one part of kind \"{name}\""),
                ));
            }
        }
    }
    let seen: Vec<String> = plan
        .bricks
        .iter()
        .map(|b| format!("{} {} {}", b.title, b.goal, b.forbidden.join(" ")))
        .chain(std::iter::once(plan.title.clone()))
        .collect();
    let found = internal_phrases(&seen.join("\n"), &spec.language, allowed);
    if !found.is_empty() {
        out.push(Defect::new(
            "internalWord",
            format!(
                "do not use these words, meaningless to the reader: {}",
                found.join(", ")
            ),
        ));
    }
    out
}

/// Cycle dans les dépendances (algorithme de Kahn ; dépendances inconnues
/// ignorées, signalées ailleurs).
fn has_cycle(plan: &Plan) -> bool {
    let ids: Vec<u32> = plan.bricks.iter().map(|b| b.id).collect();
    let mut indegree: Vec<usize> = plan
        .bricks
        .iter()
        .map(|b| {
            b.depends_on
                .iter()
                .filter(|d| **d != b.id && ids.contains(d))
                .count()
        })
        .collect();
    let mut ready: Vec<usize> = (0..ids.len()).filter(|i| indegree[*i] == 0).collect();
    let mut pushed: Vec<bool> = indegree.iter().map(|d| *d == 0).collect();
    let mut seen = 0;
    while let Some(i) = ready.pop() {
        seen += 1;
        for (j, brick) in plan.bricks.iter().enumerate() {
            if j != i && brick.id != ids[i] && brick.depends_on.contains(&ids[i]) {
                indegree[j] = indegree[j].saturating_sub(1);
                if indegree[j] == 0 && !pushed[j] {
                    pushed[j] = true;
                    ready.push(j);
                }
            }
        }
    }
    seen < ids.len()
}

/// Retire du plan les critères du document entier attribués à des parties
/// (ils ne se jugent que sur le livrable entier) ; vrai s'il y en avait.
pub fn strip_document_criteria(plan: &mut Plan, spec: &Spec) -> bool {
    let whole: Vec<&str> = spec
        .criteria
        .iter()
        .filter(|c| c.scope == CriterionScope::Document)
        .map(|c| c.id.as_str())
        .collect();
    let mut removed = false;
    for brick in &mut plan.bricks {
        let before = brick.criteria.len();
        brick.criteria.retain(|c| !whole.contains(&c.as_str()));
        removed |= brick.criteria.len() != before;
    }
    removed
}

/// Ce qu'un texte doit tenir (contrôles par programme).
pub struct Expect<'a> {
    /// Longueur visée (corps sans titre), `None` : pas mesurée.
    pub words: Option<u32>,
    /// Écart de longueur toléré : [`LENGTH_TOLERANCE`] (partie) ou
    /// [`WHOLE_LENGTH_TOLERANCE`] (livrable entier).
    pub tolerance: f64,
    /// Critères mesurés par programme ([`measure::measured`]) : (identifiant,
    /// mesure) ; jamais posés aux juges.
    pub measures: &'a [(String, Measure)],
    pub language: &'a str,
    /// Tâche et documents de l'utilisateur (ses mots ne sont pas internes).
    pub allowed: &'a str,
    /// Listes retenues du registre.
    pub lists: &'a [(String, Vec<String>)],
}

/// Contrôles par programme d'un texte : longueur à la tolérance de
/// `expect` (bloquant), critères mesurables (bloquants, preuve = la mesure,
/// [`Measure::evidence`]), texte vide (bloquant), tournures du moteur
/// (bloquant), mots du moteur seuls
/// (`engineWord`, avertissement : souvent des mots du métier), éléments hors
/// registre (signalés, non bloquants : le juge d'exactitude les examine).
pub fn program_checks(text: &str, expect: &Expect<'_>) -> (Vec<Check>, Vec<Stray>) {
    let mut checks = Vec::new();
    let body = body_of(text);
    let n = words(body);
    if n == 0 {
        checks.push(Check {
            criterion: "empty".into(),
            method: CheckMethod::Program,
            passed: false,
            evidence: "0".into(),
            where_: None,
            blocking: true,
        });
    }
    if let Some(target) = expect.words.filter(|t| *t > 0) {
        let ratio = f64::from(n) / f64::from(target);
        checks.push(Check {
            criterion: "length".into(),
            method: CheckMethod::Program,
            passed: (ratio - 1.0).abs() <= expect.tolerance,
            evidence: format!("{n}/{target}"),
            where_: None,
            blocking: true,
        });
    }
    for (id, wanted) in expect.measures {
        let (have, passed) = measure::apply(text, wanted);
        checks.push(Check {
            criterion: id.clone(),
            method: CheckMethod::Program,
            passed,
            evidence: wanted.evidence(have),
            where_: None,
            blocking: true,
        });
    }
    for phrase in internal_phrases(text, expect.language, expect.allowed) {
        checks.push(Check {
            criterion: "internalWord".into(),
            method: CheckMethod::Program,
            passed: false,
            evidence: phrase.clone(),
            where_: Some(phrase),
            blocking: true,
        });
    }
    for word in internal_words(text, expect.language, expect.allowed) {
        checks.push(Check {
            criterion: "engineWord".into(),
            method: CheckMethod::Program,
            passed: false,
            evidence: word.clone(),
            where_: Some(word),
            blocking: false,
        });
    }
    let strays = out_of_registry(text, expect.lists);
    for stray in &strays {
        checks.push(Check {
            criterion: "outsideRegistry".into(),
            method: CheckMethod::Program,
            passed: false,
            evidence: format!("{}: {}", stray.list, stray.item),
            where_: Some(stray.item.clone()),
            blocking: false,
        });
    }
    (checks, strays)
}

/// Question au juge de forme, bloquante ou non.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    /// Ce qui est vérifié (« C2 », `goal`, `language`…).
    pub criterion: String,
    pub text: String,
    pub blocking: bool,
}

/// Verdicts du juge de forme (`None` : juge indisponible) → vérifications.
/// Une question sans verdict est comptée indisponible.
pub fn form_checks(questions: &[Question], value: Option<&Value>) -> Vec<Check> {
    let evaluations = value
        .and_then(|v| v.get("evaluations"))
        .and_then(Value::as_array);
    let Some(evaluations) = evaluations else {
        return vec![unavailable("formJudge")];
    };
    let mut out = Vec::new();
    for (i, question) in questions.iter().enumerate() {
        let number = i as u64 + 1;
        let found = evaluations
            .iter()
            .find(|e| e.get("question").and_then(Value::as_u64) == Some(number))
            .or_else(|| evaluations.get(i));
        let Some(evaluation) = found else {
            out.push(unavailable("formJudge"));
            continue;
        };
        let field = |name: &str| {
            evaluation
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_owned()
        };
        let passed = field("verdict") == "yes";
        let place = field("where");
        out.push(Check {
            criterion: question.criterion.clone(),
            method: CheckMethod::FormJudge,
            passed,
            evidence: field("evidence"),
            where_: (!passed && !place.is_empty()).then_some(place),
            blocking: question.blocking,
        });
    }
    out
}

/// Marques d'un raisonnement qui ne dit pas « faux » mais « pas universel,
/// dépend du contexte, trop général » (texte replié). Le banc du 25 nov. a
/// vu « 0,5 L par plant » ou « à moins de 5 m d'un point d'eau » jugés
/// ainsi. i18n-ignore : mots cherchés.
const HEDGED: &[&str] = &[
    "universel",
    "universal",
    "trop general",
    "too general",
    "too generic",
    "demasiado general",
    "depend du contexte",
    "depend du climat",
    "depends on the context",
    "depends on the climate",
    "depende del contexto",
    "depende del clima",
    "pas une verite absolue",
    "not an absolute",
];

/// Le raisonnement d'un « faux » ne parle que d'une règle pas universelle
/// ou trop générale : c'est un « douteux ».
pub fn hedged(reasoning: &str) -> bool {
    let text = folded(reasoning).replace(['’', '\''], " ");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    HEDGED.iter().any(|m| text.contains(m))
}

/// Verdicts du juge d'exactitude → vérifications : « wrong » bloque (et
/// compte double), « doubtful » est signalé (à revoir), « right » tient. Un
/// « wrong » dont le raisonnement ne dit que « pas universel, trop
/// général, dépend du contexte » est compté « doubtful » ([`hedged`]).
pub fn fact_checks(value: Option<&Value>) -> Vec<Check> {
    let claims = value
        .and_then(|v| v.get("claims"))
        .and_then(Value::as_array);
    let Some(claims) = claims else {
        return vec![unavailable("factJudge")];
    };
    claims
        .iter()
        .map(|claim| {
            let field = |name: &str| {
                claim
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_owned()
            };
            let reasoning = field("reasoning");
            let wrong = field("verdict") == "wrong" && !hedged(&reasoning);
            let right = field("verdict") == "right";
            let quote = field("quote");
            let correction = field("correction");
            Check {
                criterion: quote.clone(),
                method: CheckMethod::FactJudge,
                passed: right,
                evidence: if wrong && !correction.is_empty() {
                    correction
                } else {
                    reasoning
                },
                where_: (!right).then_some(quote),
                blocking: wrong,
            }
        })
        .collect()
}

/// Juge qui n'a pas pu répondre : rien de vérifié par lui (dit au rapport).
pub fn unavailable(judge: &str) -> Check {
    Check {
        criterion: judge.into(),
        method: if judge == "factJudge" {
            CheckMethod::FactJudge
        } else {
            CheckMethod::FormJudge
        },
        passed: false,
        evidence: "unavailable".into(),
        where_: None,
        blocking: false,
    }
}

/// Le juge n'a pas pu répondre.
pub fn is_unavailable(check: &Check) -> bool {
    !check.passed && check.evidence == "unavailable" && check.where_.is_none()
}

/// Nombre de « non » d'une version : un par échec bloquant, deux pour une
/// affirmation fausse.
pub fn noes(checks: &[Check]) -> u32 {
    checks
        .iter()
        .filter(|c| c.blocking && !c.passed)
        .map(|c| {
            if c.method == CheckMethod::FactJudge {
                2
            } else {
                1
            }
        })
        .sum()
}

/// Reste un défaut de fond (critère ou affirmation fausse jugés) : vaut un
/// redécoupage.
pub fn substance_left(checks: &[Check]) -> bool {
    checks
        .iter()
        .any(|c| c.blocking && !c.passed && c.method != CheckMethod::Program)
}

/// Une affirmation jugée fausse, restée (pas une contradiction entre
/// parties : elle se juge sur le livrable entier).
pub fn is_wrong_claim(check: &Check) -> bool {
    check.method == CheckMethod::FactJudge
        && check.blocking
        && !check.passed
        && check.criterion != "contradiction"
}

/// Affirmations fausses restées.
pub fn wrong_claims(checks: &[Check]) -> usize {
    checks.iter().filter(|c| is_wrong_claim(c)).count()
}

/// Reste un critère de fond non tenu, jugé par le juge de forme (but,
/// interdits, critère, langue, registre).
pub fn form_left(checks: &[Check]) -> bool {
    checks
        .iter()
        .any(|c| c.blocking && !c.passed && c.method == CheckMethod::FormJudge)
}

/// La version directe ne tient pas pour autre chose que des affirmations
/// fausses : critère de fond, longueur hors ±35 %, texte vide, tournure du
/// moteur.
pub fn direct_fails_beyond_facts(checks: &[Check]) -> bool {
    let rest: Vec<Check> = checks
        .iter()
        .filter(|c| !is_wrong_claim(c))
        .cloned()
        .collect();
    direct_fails(&rest)
}

/// Une passe chirurgicale suffit : de 1 à [`MAX_SURGERY`] affirmations
/// fausses, rien d'autre de fond.
pub fn surgery_fits(checks: &[Check]) -> bool {
    (1..=MAX_SURGERY).contains(&wrong_claims(checks)) && !direct_fails_beyond_facts(checks)
}

/// Remarques des juges recopiées dans une correction (suites d'au moins
/// [`COPIED_WORDS`] mots absentes de la version d'avant) : signalées, non
/// bloquantes.
pub fn copied_remarks(before: &[Check], previous: &str, fixed: &str) -> Vec<Check> {
    let mut out: Vec<Check> = Vec::new();
    for check in before.iter().filter(|c| c.blocking && !c.passed) {
        for run in copied_runs(&check.evidence, fixed, previous, COPIED_WORDS) {
            if !out.iter().any(|c| c.evidence == run) {
                out.push(Check {
                    criterion: "copiedRemark".into(),
                    method: CheckMethod::Program,
                    passed: false,
                    evidence: run.clone(),
                    where_: Some(run),
                    blocking: false,
                });
            }
        }
    }
    out
}

/// Affirmation douteuse (jamais bloquante).
pub fn is_doubtful(check: &Check) -> bool {
    check.method == CheckMethod::FactJudge
        && !check.passed
        && !check.blocking
        && !is_unavailable(check)
        && check.criterion != "contradictions"
}

/// Point « à revoir » : un échec qui compte, resté.
pub fn is_to_review(check: &Check) -> bool {
    check.blocking && !check.passed
}

/// Écart relatif de longueur mesuré par un contrôle `length` (« 950/1000 »).
fn length_gap(check: &Check) -> Option<f64> {
    let (have, want) = check.evidence.split_once('/')?;
    let have: f64 = have.trim().parse().ok()?;
    let want: f64 = want.trim().parse().ok()?;
    (want > 0.0).then(|| (have / want - 1.0).abs())
}

/// Écart relatif d'un critère de nombre de mots mesuré par programme
/// (« 3239/3300-3700 words ») ; `None` : pas un tel critère.
fn words_gap(check: &Check) -> Option<f64> {
    let (have, wanted) = measure::read_evidence(&check.evidence)?;
    (check.method == CheckMethod::Program && wanted.unit == Unit::Words).then(|| wanted.gap(have))
}

/// La version directe (gardée après sa correction) ne tient pas : un « non »
/// de fond reste (critère ou affirmation fausse jugés, critère mesuré autre
/// qu'un nombre de mots, texte vide, tournure du moteur), ou sa longueur
/// (longueur visée, critère de nombre de mots) s'écarte de plus de ±35 %.
/// Le style et une longueur un peu à côté ne valent pas la route par
/// parties (×13 : lot 0).
pub fn direct_fails(checks: &[Check]) -> bool {
    substance_left(checks)
        || checks
            .iter()
            .filter(|c| c.blocking && !c.passed && c.method == CheckMethod::Program)
            .any(|c| {
                let gap = if c.criterion == "length" {
                    length_gap(c)
                } else {
                    match words_gap(c) {
                        Some(gap) => Some(gap),
                        None => return true,
                    }
                };
                gap.is_none_or(|gap| gap > DIRECT_LENGTH_TOLERANCE)
            })
}

/// Écarts de longueur restés (longueur visée, critères de nombre de mots),
/// additionnés.
fn length_gaps(checks: &[Check]) -> f64 {
    checks
        .iter()
        .filter(|c| c.blocking && !c.passed && c.method == CheckMethod::Program)
        .filter_map(|c| {
            if c.criterion == "length" {
                length_gap(c)
            } else {
                words_gap(c)
            }
        })
        .sum()
}

/// La correction de la version directe vaut mieux que la version d'avant :
/// moins de « non », ou autant mais une longueur plus proche de la cible
/// (une correction qui allonge sans tout à fait atteindre la cible est
/// gardée).
pub fn better(fixed: &[Check], before: &[Check]) -> bool {
    let (after, first) = (noes(fixed), noes(before));
    after < first || (after == first && length_gaps(fixed) < length_gaps(before))
}

/// Consigne pour une longueur à côté : mesurée, visée, allonger ou
/// raccourcir de combien. i18n-ignore : consigne au modèle.
fn resize(have: u32, aim: u32) -> String {
    if have < aim {
        format!(
            "lengthen it by about {} words, to about {aim} words: develop the existing points with \
             useful details, examples and explanations (no padding, no repetition)",
            aim - have
        )
    } else {
        format!(
            "shorten it by about {} words, to about {aim} words: tighten sentences and remove \
             repetitions, without losing required content",
            have - aim
        )
    }
}

/// Point à corriger pour un critère mesuré par programme. i18n-ignore :
/// consigne au modèle.
fn measure_problem(id: &str, have: u32, wanted: &Measure) -> String {
    let bounds = |unit: &str| match (wanted.min, wanted.max) {
        (Some(min), Some(max)) if min == max => format!("exactly {min} {unit}"),
        (Some(min), Some(max)) => format!("between {min} and {max} {unit}"),
        (Some(min), None) => format!("at least {min} {unit}"),
        (None, Some(max)) => format!("at most {max} {unit}"),
        (None, None) => unit.to_owned(),
    };
    let too_few = wanted.min.is_some_and(|min| have < min);
    match wanted.unit {
        Unit::Words => format!(
            "criterion {id} (counted by program): the text has {have} words; it must have {}: {}",
            bounds("words"),
            resize(have, wanted.target())
        ),
        Unit::Headings(level) => {
            let unit = match level {
                Some(n) => format!("level-{n} Markdown headings"),
                None => "Markdown headings".to_owned(),
            };
            format!(
                "criterion {id} (counted by program): the text has {have} {unit}; it must have \
                 {}: {}",
                bounds(&unit),
                if too_few {
                    "add headings where the content calls for them (split long sections)"
                } else {
                    "merge or remove headings"
                }
            )
        }
        Unit::Sections => format!(
            "criterion {id} (counted by program): the text has {have} sections; it must have {}: \
             {}",
            bounds("sections"),
            if too_few {
                "add sections, each with its own Markdown heading"
            } else {
                "merge sections"
            }
        ),
        Unit::Tables => {
            if too_few {
                format!(
                    "criterion {id} (counted by program): the text has {have} Markdown tables; it \
                     must have {}: add a Markdown table (header line, separator line |---|, rows)",
                    bounds("tables")
                )
            } else {
                format!(
                    "criterion {id} (counted by program): the text has {have} Markdown tables; it \
                     must have {}: present that content as text or lists instead",
                    bounds("tables")
                )
            }
        }
        Unit::Items => format!(
            "criterion {id} (counted by program): the best-matching Markdown list has {have} \
             items; it must have {}: {}",
            bounds("items"),
            if too_few {
                "add items to the list"
            } else {
                "shorten the list"
            }
        ),
    }
}

/// Points à corriger, en anglais, avec l'endroit (correction ciblée).
pub fn problems(checks: &[Check], questions: &[Question], target: Option<u32>) -> Vec<String> {
    let mut out = Vec::new();
    for check in checks.iter().filter(|c| c.blocking && !c.passed) {
        let text = match (check.method, check.criterion.as_str()) {
            (CheckMethod::Program, "length") => {
                let target = target.unwrap_or(0);
                let have: u32 = check
                    .evidence
                    .split('/')
                    .next()
                    .and_then(|n| n.trim().parse().ok())
                    .unwrap_or(0);
                format!(
                    "length: the text has {have} words; it must have about {target} words: {}",
                    resize(have, target)
                )
            }
            (CheckMethod::Program, "empty") => "the section is empty".into(),
            (CheckMethod::Program, "internalWord") => format!(
                "remove the words \"{}\": they mean nothing to the reader",
                check.evidence
            ),
            (CheckMethod::Program, other) => match measure::read_evidence(&check.evidence) {
                Some((have, wanted)) => measure_problem(other, have, &wanted),
                None => other.to_owned(),
            },
            (CheckMethod::FormJudge, criterion) => {
                let question = questions
                    .iter()
                    .find(|q| q.criterion == criterion)
                    .map_or(criterion, |q| q.text.as_str());
                match &check.where_ {
                    Some(place) => format!(
                        "{question}\n  Finding: {}\n  Where: «{place}»",
                        check.evidence
                    ),
                    None => format!("{question}\n  Finding: {}", check.evidence),
                }
            }
            (CheckMethod::FactJudge, _) => format!(
                "wrong claim: «{}»\n  Correction: {}",
                check.criterion, check.evidence
            ),
        };
        out.push(text);
    }
    out
}

/// Points relevés par programme pour le juge d'exactitude.
pub fn flagged(strays: &[Stray]) -> Vec<String> {
    strays
        .iter()
        .map(|s| {
            format!(
                "the text cites «{}» together with items of the list «{}» retained for the whole \
                 document, but «{}» is not in that list",
                s.item, s.list, s.item
            )
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::workshop::pipeline::types::{BrickPlan, Criterion, Register};

    pub(crate) fn spec() -> Spec {
        Spec {
            goal: "Un guide du potager sur un balcon.".into(),
            title: "Potager sur un balcon".into(),
            deliverables: vec!["guide".into()],
            criteria: vec![
                Criterion {
                    id: "C1".into(),
                    text: "QUAND on lit, LE LIVRABLE DOIT lister les cultures".into(),
                    scope: CriterionScope::Section,
                },
                Criterion {
                    id: "C2".into(),
                    text: "QUAND on lit, LE LIVRABLE DOIT donner un calendrier".into(),
                    scope: CriterionScope::Section,
                },
            ],
            audience: "débutants".into(),
            language: "fr".into(),
            register: Register::Informal,
            units: "métrique : g, kg, m, L".into(),
            length_words: Some(1_000),
            glossary: Vec::new(),
            assumptions: Vec::new(),
            constraints: Vec::new(),
        }
    }

    fn part(
        id: u32,
        kind: BrickKind,
        title: &str,
        words: u32,
        deps: &[u32],
        criteria: &[&str],
    ) -> BrickPlan {
        BrickPlan {
            id,
            title: title.into(),
            goal: format!("Traiter {title}"),
            kind,
            words,
            depends_on: deps.to_vec(),
            criteria: criteria.iter().map(|c| (*c).to_owned()).collect(),
            forbidden: Vec::new(),
        }
    }

    pub(crate) fn good_plan() -> Plan {
        Plan {
            title: "Guide".into(),
            bricks: vec![
                part(1, BrickKind::Intro, "Introduction", 100, &[], &[]),
                part(2, BrickKind::Text, "Les cultures", 400, &[], &["C1"]),
                part(
                    3,
                    BrickKind::Text,
                    "Le calendrier des récoltes",
                    400,
                    &[2],
                    &["C2"],
                ),
                part(4, BrickKind::Conclusion, "Conclusion", 100, &[], &[]),
            ],
        }
    }

    fn codes(defects: &[Defect]) -> Vec<&'static str> {
        defects.iter().map(|d| d.code).collect()
    }

    #[test]
    fn a_good_plan_passes() {
        assert!(plan_defects(&good_plan(), &spec(), Strategy::LongForm, "").is_empty());
    }

    #[test]
    fn plan_defects_are_each_found() {
        let spec = spec();
        let mut plan = good_plan();
        plan.bricks[2].depends_on = vec![3];
        assert_eq!(
            codes(&plan_defects(&plan, &spec, Strategy::LongForm, "")),
            vec!["selfDependency"]
        );
        let mut plan = good_plan();
        plan.bricks[1].depends_on = vec![3];
        assert_eq!(
            codes(&plan_defects(&plan, &spec, Strategy::LongForm, "")),
            vec!["cycle"]
        );
        let mut plan = good_plan();
        plan.bricks[1].depends_on = vec![9];
        assert_eq!(
            codes(&plan_defects(&plan, &spec, Strategy::LongForm, "")),
            vec!["unknownDependency"]
        );
        let mut plan = good_plan();
        plan.bricks[1].depends_on = vec![1];
        assert_eq!(
            codes(&plan_defects(&plan, &spec, Strategy::LongForm, "")),
            vec!["dependsOnFrame"]
        );
        let mut plan = good_plan();
        plan.bricks[2].criteria.clear();
        assert_eq!(
            codes(&plan_defects(&plan, &spec, Strategy::LongForm, "")),
            vec!["uncoveredCriterion"]
        );
        let mut plan = good_plan();
        plan.bricks[2].criteria.push("C7".into());
        assert_eq!(
            codes(&plan_defects(&plan, &spec, Strategy::LongForm, "")),
            vec!["unknownCriterion"]
        );
        let mut plan = good_plan();
        plan.bricks[2].title = "Les cultures".into();
        plan.bricks[2].goal = "Traiter Les cultures".into();
        assert_eq!(
            codes(&plan_defects(&plan, &spec, Strategy::LongForm, "")),
            vec!["overlap"]
        );
        let mut plan = good_plan();
        plan.bricks[2].words = 900;
        assert_eq!(
            codes(&plan_defects(&plan, &spec, Strategy::LongForm, "")),
            vec!["lengthMismatch"]
        );
        let mut plan = good_plan();
        plan.bricks.remove(0);
        plan.bricks[0].words = 450;
        plan.bricks[1].words = 450;
        assert_eq!(
            codes(&plan_defects(&plan, &spec, Strategy::LongForm, "")),
            vec!["missingIntro"]
        );
        // Générique : introduction facultative.
        assert!(plan_defects(&plan, &spec, Strategy::Generic, "").is_empty());
        let mut plan = good_plan();
        plan.bricks[3].id = 2;
        assert!(
            codes(&plan_defects(&plan, &spec, Strategy::LongForm, "")).contains(&"duplicateId")
        );
        let mut plan = good_plan();
        plan.bricks[2].title = "Astuce atelier".into();
        // Un mot du métier seul n'est pas un défaut du plan.
        let mut fine = good_plan();
        fine.bricks[2].title = "Les briques du four".into();
        assert!(plan_defects(&fine, &spec, Strategy::LongForm, "").is_empty());
        assert_eq!(
            codes(&plan_defects(&plan, &spec, Strategy::LongForm, "")),
            vec!["internalWord"]
        );
    }

    #[test]
    fn a_spec_must_be_decided() {
        let good = spec();
        assert!(spec_defects(&good, Strategy::LongForm, "").is_empty());
        let mut bad = good.clone();
        bad.units = "métrique ou impérial".into();
        assert_eq!(
            codes(&spec_defects(&bad, Strategy::LongForm, "")),
            vec!["undecided"]
        );
        let mut bad = good.clone();
        bad.length_words = None;
        assert_eq!(
            codes(&spec_defects(&bad, Strategy::LongForm, "")),
            vec!["length"]
        );
        assert!(spec_defects(&bad, Strategy::Generic, "").is_empty());
        let mut bad = good.clone();
        bad.goal = "Un guide du potager. Astuce atelier : pailler.".into();
        assert_eq!(
            codes(&spec_defects(&bad, Strategy::LongForm, "")),
            vec!["internalWord"]
        );
        assert!(spec_defects(&bad, Strategy::LongForm, "une astuce atelier par partie").is_empty());
        // Relecture du lot 2 : unités métriques avec sigle et « / ».
        let mut si = good.clone();
        si.units = "métrique (SI) : g, kg, m², L/m²".into();
        assert!(spec_defects(&si, Strategy::LongForm, "").is_empty());
        // Longueur au-delà de 20 parties de 1 500 mots : refusée telle quelle
        // (la phase de spécification la borne avant).
        let mut long = good.clone();
        long.length_words = Some(MAX_LENGTH_WORDS + 1);
        assert_eq!(
            codes(&spec_defects(&long, Strategy::LongForm, "")),
            vec!["length"]
        );
        let mut bad = good;
        bad.language = "French".into();
        assert_eq!(
            codes(&spec_defects(&bad, Strategy::LongForm, "")),
            vec!["language"]
        );
    }

    #[test]
    fn program_checks_measure_length_and_internal_words() {
        let lists = vec![(
            "cultures".to_owned(),
            vec!["tomate".to_owned(), "radis".to_owned()],
        )];
        let expect = Expect {
            words: Some(30),
            tolerance: LENGTH_TOLERANCE,
            language: "fr",
            allowed: "",
            lists: &lists,
            measures: &[],
        };
        let text = "## Récoltes\n\nAstuce atelier : un deux trois quatre cinq six sept.\n\n- Tomates\n- Poireaux";
        let (checks, strays) = program_checks(text, &expect);
        let failed: Vec<&str> = checks
            .iter()
            .filter(|c| !c.passed)
            .map(|c| c.criterion.as_str())
            .collect();
        assert_eq!(
            failed,
            vec!["length", "internalWord", "engineWord", "outsideRegistry"]
        );
        let blocking: Vec<&str> = checks
            .iter()
            .filter(|c| c.blocking && !c.passed)
            .map(|c| c.criterion.as_str())
            .collect();
        assert_eq!(blocking, vec!["length", "internalWord"]);
        assert_eq!(strays.len(), 1);
        // Relecture du lot 2 : un mot du métier seul n'est qu'un
        // avertissement.
        let text = format!("## Four\n\nLes briques réfractaires. {}", "mot ".repeat(27));
        let (checks, _) = program_checks(&text, &expect);
        assert_eq!(noes(&checks), 0);
        assert!(
            checks
                .iter()
                .any(|c| c.criterion == "engineWord" && !c.blocking)
        );
        // Longueur dans la tolérance (le titre ne compte pas).
        let text = format!("## Un titre très long ici\n\n{}", "mot ".repeat(26));
        let (checks, _) = program_checks(&text, &expect);
        assert!(checks.iter().all(|c| c.passed));
    }

    #[test]
    fn a_wrong_claim_counts_double() {
        let facts = serde_json::json!({"claims": [
            {"quote": "200 g de fumier", "reasoning": "r", "verdict": "wrong", "correction": "3 kg"},
            {"quote": "le haricot", "reasoning": "?", "verdict": "doubtful", "correction": ""},
            {"quote": "la tomate", "reasoning": "ok", "verdict": "right", "correction": ""},
        ]});
        let checks = fact_checks(Some(&facts));
        assert_eq!(noes(&checks), 2);
        assert!(substance_left(&checks));
        assert_eq!(checks[0].evidence, "3 kg");
        assert!(!checks[1].blocking && !checks[1].passed);
        // Juge indisponible : rien de bloquant, mais dit.
        let none = fact_checks(None);
        assert_eq!(noes(&none), 0);
        assert!(is_unavailable(&none[0]));
    }

    /// Banc du 25 nov. (cas réels) : un « faux » qui ne dit que « trop
    /// général » ou « pas universel » est douteux ; une vraie erreur reste
    /// fausse (« selon le pH » ne suffit pas à l'excuser).
    #[test]
    fn hedged_wrong_claims_are_only_doubtful() {
        let facts = serde_json::json!({"claims": [
            {"quote": "Les jeunes plants ont besoin de 0,5 L par plant par jour.",
             "reasoning": "C'est une valeur trop générale. Le besoin en eau dépend de la taille du plant.",
             "verdict": "wrong", "correction": "x"},
            {"quote": "à moins de 5 m d'un point d'eau",
             "reasoning": "Être proche d'un point d'eau facilite l'arrosage, mais 5 m n'est pas une limite technique universelle.",
             "verdict": "wrong", "correction": "x"},
            {"quote": "Une tomate trop mûre peut attirer les pucerons.",
             "reasoning": "Les pucerons se développent surtout sur les jeunes pousses, pas spécifiquement sur un fruit trop mûr.",
             "verdict": "wrong", "correction": "Les pucerons visent les pousses tendres."},
            {"quote": "Tu peux aussi ajouter 1 kg de chaux par m².",
             "reasoning": "Dose très élevée ; les doses courantes sont de 100 à 200 g/m², selon le pH et la texture du sol.",
             "verdict": "wrong", "correction": "100 à 200 g par m²"},
        ]});
        let checks = fact_checks(Some(&facts));
        let blocking: Vec<bool> = checks.iter().map(|c| c.blocking).collect();
        assert_eq!(blocking, vec![false, false, true, true]);
        assert!(checks.iter().take(2).all(is_doubtful));
        assert_eq!(wrong_claims(&checks), 2);
        assert_eq!(
            checks[2].evidence,
            "Les pucerons visent les pousses tendres."
        );
        assert!(
            checks[0]
                .evidence
                .starts_with("C'est une valeur trop générale")
        );
    }

    #[test]
    fn a_surgery_fits_only_a_few_wrong_claims_and_nothing_else() {
        let wrong = |n: usize| -> Vec<Check> {
            (0..n)
                .map(|i| Check {
                    criterion: format!("q{i}"),
                    method: CheckMethod::FactJudge,
                    passed: false,
                    evidence: "e".into(),
                    where_: None,
                    blocking: true,
                })
                .collect()
        };
        assert!(!surgery_fits(&wrong(0)));
        assert!(surgery_fits(&wrong(1)));
        assert!(surgery_fits(&wrong(MAX_SURGERY)));
        assert!(!surgery_fits(&wrong(MAX_SURGERY + 1)));
        let mut with_form = wrong(1);
        with_form.push(Check {
            criterion: "C1".into(),
            method: CheckMethod::FormJudge,
            passed: false,
            evidence: "e".into(),
            where_: None,
            blocking: true,
        });
        assert!(!surgery_fits(&with_form));
        let mut far = wrong(1);
        far.push(Check {
            criterion: "length".into(),
            method: CheckMethod::Program,
            passed: false,
            evidence: "500/1000".into(),
            where_: None,
            blocking: true,
        });
        assert!(!surgery_fits(&far));
        far.last_mut().unwrap().evidence = "800/1000".into();
        assert!(surgery_fits(&far));
    }

    /// Point A : un critère du document entier n'a pas à être couvert par
    /// une partie ; s'il y est attribué, il est retiré.
    #[test]
    fn document_criteria_are_stripped_from_the_plan() {
        let mut spec = spec();
        spec.criteria[0].scope = CriterionScope::Document;
        let mut plan = good_plan();
        assert!(strip_document_criteria(&mut plan, &spec));
        assert!(plan.bricks[1].criteria.is_empty());
        assert!(!strip_document_criteria(&mut plan, &spec));
        assert!(plan_defects(&plan, &spec, Strategy::LongForm, "").is_empty());
    }

    #[test]
    fn form_verdicts_follow_their_question() {
        let questions = vec![
            Question {
                criterion: "goal".into(),
                text: "Goal?".into(),
                blocking: true,
            },
            Question {
                criterion: "C1".into(),
                text: "C1?".into(),
                blocking: true,
            },
        ];
        let value = serde_json::json!({"evaluations": [
            {"question": 2, "evidence": "e2", "verdict": "no", "where": "là"},
            {"question": 1, "evidence": "e1", "verdict": "yes", "where": ""},
        ]});
        let checks = form_checks(&questions, Some(&value));
        assert!(checks[0].passed);
        assert_eq!(checks[1].criterion, "C1");
        assert_eq!(checks[1].where_.as_deref(), Some("là"));
        let problems = problems(&checks, &questions, None);
        assert_eq!(
            problems,
            vec!["C1?\n  Finding: e2\n  Where: «là»".to_owned()]
        );
    }
    fn program(criterion: &str, evidence: &str) -> Check {
        Check {
            criterion: criterion.into(),
            method: CheckMethod::Program,
            passed: false,
            evidence: evidence.into(),
            where_: None,
            blocking: true,
        }
    }

    /// Un critère de nombre de mots suit la règle de la longueur (±35 %) ;
    /// un autre critère mesuré (tableau, titres) fait échouer la version.
    #[test]
    fn a_word_criterion_follows_the_length_rule() {
        assert!(!direct_fails(&[program("C1", "700/900-1100 words")]));
        assert!(direct_fails(&[program("C1", "600/900-1100 words")]));
        assert!(!direct_fails(&[program("C1", "1300/-1000 words")]));
        assert!(direct_fails(&[program("C1", "1400/-1000 words")]));
        assert!(direct_fails(&[program("C2", "0/1- tables")]));
        assert!(direct_fails(&[program("C2", "7/8- headings")]));
        // Une correction plus proche de la cible, autant de « non » : gardée.
        let before = [
            program("length", "798/1000"),
            program("C1", "798/900-1100 words"),
        ];
        let closer = [
            program("length", "858/1000"),
            program("C1", "858/900-1100 words"),
        ];
        assert!(better(&closer, &before));
        assert!(!better(&before, &closer));
        assert!(!better(&before, &before));
        assert!(better(&[], &before));
    }

    #[test]
    fn measured_criteria_are_checked_by_program_with_their_measure() {
        let measures = vec![
            (
                "C1".to_owned(),
                measure::parse("Le guide compte entre 10 et 20 mots.", "fr").unwrap(),
            ),
            (
                "C2".to_owned(),
                measure::parse("Le guide contient un tableau.", "fr").unwrap(),
            ),
        ];
        let expect = Expect {
            words: Some(30),
            tolerance: WHOLE_LENGTH_TOLERANCE,
            language: "fr",
            allowed: "",
            lists: &[],
            measures: &measures,
        };
        // 28 mots : −7 % (tenu à ±10 %), au-dessus des 20 mots de C1.
        let text = format!("# Guide\n\n{}", "mot ".repeat(28));
        let (checks, _) = program_checks(&text, &expect);
        let found: Vec<(&str, bool, &str)> = checks
            .iter()
            .map(|c| (c.criterion.as_str(), c.passed, c.evidence.as_str()))
            .collect();
        assert_eq!(
            found,
            vec![
                ("length", true, "28/30"),
                ("C1", false, "28/10-20 words"),
                ("C2", false, "0/1- tables")
            ]
        );
        assert!(
            checks
                .iter()
                .all(|c| c.method == CheckMethod::Program && c.blocking)
        );
        // −13 % : hors de ±10 %.
        let text = format!("# Guide\n\n{}", "mot ".repeat(26));
        assert!(!program_checks(&text, &expect).0[0].passed);
        let problems = problems(&checks, &[], Some(30));
        assert_eq!(
            problems[0],
            "criterion C1 (counted by program): the text has 28 words; it must have between 10 and \
             20 words: shorten it by about 13 words, to about 15 words: tighten sentences and \
             remove repetitions, without losing required content"
        );
        assert!(problems[1].starts_with(
            "criterion C2 (counted by program): the text has 0 Markdown tables; it must have at \
             least 1 tables: add a Markdown table"
        ));
    }
}
