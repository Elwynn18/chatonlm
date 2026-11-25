//! Essais du pipeline : faux modèle scénarisé, faux socle en mémoire
//! (`fake.rs`). Un essai par comportement du contrat 111 (partie P).

mod bench;
mod fake;

use std::time::Duration;

use serde_json::{Value, json};

use super::types::{
    Answer, Autonomy, BrickKind, BrickPlan, BrickState, CheckMethod, EventKind, Plan, ProjectState,
    Question, Report, Route, StateKey, Strategy,
};
use super::{RunEnd, run};
use crate::agent::AgentError;
use fake::{FakeHost, FakeModel, Reply, Req, filler, form, good_plan, questions};

/// Rien de scénarisé : un bon modèle.
fn nothing(_: &Req) -> Option<Reply> {
    None
}

fn report(host: &FakeHost) -> Report {
    serde_json::from_value(host.get(StateKey::Report).expect("rapport")).unwrap()
}

fn brick_state(host: &FakeHost, id: u32) -> BrickState {
    let record = host.get(StateKey::Brick(id)).expect("partie finie");
    serde_json::from_value(record["state"].clone()).unwrap()
}

/// Le juge de forme refuse la version directe (critère C1) : plan et parties.
fn direct_fails(req: &Req) -> Option<Reply> {
    (req.role == "formJudge" && req.user.contains("\nDOCUMENT TO ASSESS"))
        .then(|| Reply::Json(form(6, &[1])))
}

#[tokio::test]
async fn a_direct_version_that_holds_is_delivered() {
    let host = FakeHost::new(Autonomy::Ask);
    let model = FakeModel::new(&host.trace, nothing);
    let end = run(&host, &model).await;
    assert_eq!(
        end,
        RunEnd::Delivered {
            file: "guide-du-balcon.md".into()
        }
    );
    assert_eq!(
        model.roles(),
        vec![
            "strategy",
            "strategy",
            "strategy",
            "spec",
            "direct",
            "formJudge",
            "factJudge",
            "proofread"
        ]
    );
    let doc = host.file("guide-du-balcon.md").unwrap();
    assert!(doc.starts_with("# Guide du balcon"));
    let report = report(&host);
    assert_eq!(report.route, Route::Direct);
    assert_eq!(report.strategy, Strategy::LongForm);
    assert!(report.checks.iter().all(|c| c.passed));
    let readable = host.file(".atelier/rapport.md").unwrap();
    assert!(readable.starts_with("# Rapport de vérification"));
    assert!(host.has(StateKey::Delivered));
    assert_eq!(
        host.events().last(),
        Some(&EventKind::Finished {
            state: ProjectState::Done
        })
    );
    // Cadrage demandé, pas de plan à approuver (aucun plan).
    let asked = host.questions();
    assert_eq!(asked.len(), 1);
    assert!(matches!(asked[0], Question::Framing(_)));
    // Les réponses du cadrage arrivent à la spécification.
    assert!(model.of("spec")[0].user.contains("Plein sud, 2 m²."));
    // Le titre de la spécification nomme le projet (guillemets et point
    // retirés), une fois.
    assert_eq!(*host.names.lock().unwrap(), ["Potager sur un balcon"]);
    // Réflexion coupée pour les tirages, températures 0,2 / 0,6 / 1,0.
    let temperatures: Vec<f32> = model.of("strategy").iter().map(|r| r.temperature).collect();
    assert_eq!(temperatures, vec![0.2, 0.6, 1.0]);
}

#[tokio::test]
async fn a_direct_version_that_fails_goes_to_the_plan() {
    let host = FakeHost::new(Autonomy::Ask);
    let model = FakeModel::new(&host.trace, direct_fails);
    let end = run(&host, &model).await;
    assert!(matches!(end, RunEnd::Delivered { .. }));
    let roles = model.roles();
    assert!(roles.contains(&"plan".to_owned()));
    assert_eq!(model.of("section").len(), 4);
    let report = report(&host);
    assert_eq!(report.route, Route::Parts);
    assert_eq!(report.bricks.len(), 4);
    // Ordre : sections, puis introduction et conclusion (à la fin).
    let order: Vec<String> = model
        .of("section")
        .iter()
        .filter_map(Req::section)
        .collect();
    assert_eq!(
        order,
        vec![
            "Les cultures",
            "Le calendrier",
            "Introduction",
            "Conclusion"
        ]
    );
    // Assemblage mécanique dans l'ordre du plan.
    let doc = host.file("guide-du-balcon.md").unwrap();
    let at = |s: &str| doc.find(s).unwrap();
    assert!(at("# Guide du balcon") < at("## Les cultures"));
    assert!(at("## Les cultures") < at("## Le calendrier"));
    assert!(at("## Le calendrier") < at("## Conclusion"));
    // Plan approuvé par l'utilisateur (autonomie « Demander »).
    assert!(
        host.questions()
            .iter()
            .any(|q| matches!(q, Question::ApprovePlan(_)))
    );
}

#[tokio::test]
async fn a_long_text_skips_the_direct_version() {
    let host = FakeHost::new(Autonomy::AcceptActions);
    let mut model = FakeModel::new(&host.trace, |req| {
        (req.role == "plan").then(|| {
            let mut plan = good_plan();
            for (i, words) in [1_000, 1_500, 1_500, 1_000].iter().enumerate() {
                plan["parts"][i]["words"] = json!(words);
            }
            Reply::Json(plan)
        })
    });
    model.length = 5_000;
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert!(model.of("direct").is_empty());
}

#[tokio::test]
async fn a_rejected_plan_is_followed_by_another_one_at_a_time() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        match (req.role.as_str(), req.n) {
            // Premier plan : un cycle, vu par programme (pas de juge).
            ("plan", 0) => {
                let mut plan = good_plan();
                plan["parts"][1]["depends_on"] = json!([3]);
                Some(Reply::Json(plan))
            }
            // Deuxième plan : jugé 4 (rejeté) ; troisième : 7 (accepté).
            ("planJudge", 0) => Some(Reply::Json(json!({
                "evaluations": [{"question": 2, "evidence": "2 et 3 se recouvrent", "verdict": "no"}],
                "defects": [{"part": 3, "problem": "trop vague"}], "score": 4}))),
            ("planJudge", 1) => Some(Reply::Json(
                json!({"evaluations": [], "defects": [], "score": 7}),
            )),
            _ => None,
        }
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let plans = model.of("plan");
    assert_eq!(plans.len(), 3);
    assert_eq!(model.of("planJudge").len(), 2);
    assert!(plans[1].user.contains("the dependencies form a cycle"));
    assert!(plans[2].user.contains("part 3: trop vague"));
    assert!(plans[2].user.contains("Duplicates"));
    let temperatures: Vec<f32> = plans.iter().map(|r| r.temperature).collect();
    assert_eq!(temperatures, vec![0.6, 0.8, 1.0]);
    // Chaque plan jugé seul : un plan par requête du juge.
    for judge in model.of("planJudge") {
        assert_eq!(judge.user.matches("PLAN TO ASSESS").count(), 1);
    }
}

#[tokio::test]
async fn without_an_accepted_plan_the_best_score_is_kept() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        if req.role == "plan" {
            let mut plan = good_plan();
            plan["title"] = json!(["Plan un", "Plan deux", "Plan trois"][req.n]);
            return Some(Reply::Json(plan));
        }
        let score = match (req.role.as_str(), req.n) {
            ("planJudge", 0) => 3,
            ("planJudge", 1) => 5,
            ("planJudge", 2) => 2,
            _ => return None,
        };
        Some(Reply::Json(
            json!({"evaluations": [], "defects": [], "score": score}),
        ))
    });
    let end = run(&host, &model).await;
    assert_eq!(
        end,
        RunEnd::Delivered {
            file: "plan-deux.md".into()
        }
    );
    assert_eq!(model.of("planJudge").len(), 3);
    assert!(host.events().contains(&EventKind::Warning {
        code: "planBestOfThree".into()
    }));
}

#[tokio::test]
async fn without_a_flawless_plan_the_least_flawed_one_that_can_be_led_is_kept() {
    // Trois plans aux longueurs fausses (défaut non structurel), le
    // deuxième moins fautif (un défaut contre deux) : gardé, sans juge.
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        (req.role == "plan").then(|| {
            let mut plan = good_plan();
            plan["title"] = json!(["Plan un", "Plan deux", "Plan trois"][req.n]);
            plan["parts"][2]["words"] = json!(1_200);
            if req.n != 1 {
                plan["parts"][1]["title"] = json!("Le calendrier");
                plan["parts"][1]["goal"] = json!("Dire quand semer et récolter");
            }
            Reply::Json(plan)
        })
    });
    let end = run(&host, &model).await;
    assert_eq!(
        end,
        RunEnd::Delivered {
            file: "plan-deux.md".into()
        }
    );
    assert!(model.of("planJudge").is_empty());
    let events = host.events();
    assert!(events.contains(&EventKind::Warning {
        code: "planBestOfThree".into()
    }));
    assert!(events.contains(&EventKind::Warning {
        code: "planDefect.lengthMismatch".into()
    }));

    // Trois plans qu'on ne peut pas mener (cycle) : échec avec un code.
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        (req.role == "plan").then(|| {
            let mut plan = good_plan();
            plan["parts"][1]["depends_on"] = json!([3]);
            Reply::Json(plan)
        })
    });
    assert_eq!(
        run(&host, &model).await,
        RunEnd::Failed {
            code: "planUnusable".into()
        }
    );
    assert!(model.of("section").is_empty());
}

/// Le cas « cultures » du lot 0 : la partie « Récoltes » ne dépend que de
/// « Arrosage », mais reçoit la liste des cultures retenue par « Les
/// cultures » (registre global).
fn crops_plan(req: &Req) -> Option<Reply> {
    if let Some(reply) = direct_fails(req) {
        return Some(reply);
    }
    match req.role.as_str() {
        "plan" => Some(Reply::Json(
            json!({"analysis": "a", "title": "Guide du balcon", "parts": [
            {"id": 1, "title": "Introduction", "goal": "Présenter", "kind": "intro", "words": 100,
             "depends_on": [], "criteria": [], "forbidden": []},
            {"id": 2, "title": "Les cultures", "goal": "Choisir les plantes", "kind": "text", "words": 300,
             "depends_on": [], "criteria": ["C1", "C2"], "forbidden": []},
            {"id": 3, "title": "Arrosage", "goal": "Arroser au bon moment", "kind": "text", "words": 300,
             "depends_on": [], "criteria": ["C3"], "forbidden": []},
            {"id": 4, "title": "Récoltes", "goal": "Dire quand cueillir", "kind": "text", "words": 200,
             "depends_on": [3], "criteria": ["C4"], "forbidden": []},
            {"id": 5, "title": "Conclusion", "goal": "Conclure", "kind": "conclusion", "words": 100,
             "depends_on": [], "criteria": [], "forbidden": []}]}),
        )),
        "card" if req.about("Les cultures") => Some(Reply::Json(json!({
            "summary": "Trois cultures.", "terms": [], "decisions": ["Registre : tutoiement"],
            "lists": [{"name": "cultures", "items": ["tomate", "radis", "laitue"]}], "exposes": []}))),
        "section" if req.about("Récoltes") => Some(Reply::Text(format!(
            "## Récoltes\n\n{}\n\n- Tomates : en août\n- Poireaux : en novembre",
            filler(190)
        ))),
        _ => None,
    }
}

#[tokio::test]
async fn every_later_part_receives_the_global_registry() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, crops_plan);
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let harvest = model
        .of("section")
        .into_iter()
        .find(|r| r.about("Récoltes"))
        .unwrap();
    assert!(harvest.user.contains("cultures: tomate; radis; laitue"));
    assert!(harvest.user.contains("Registre : tutoiement"));
    // Carte complète de sa dépendance directe, en entrée.
    assert!(harvest.user.contains("Section « Arrosage »"));
    assert!(!harvest.user.contains("Section « Les cultures »"));
}

#[tokio::test]
async fn an_item_outside_a_retained_list_is_flagged_to_the_fact_judge() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, crops_plan);
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let judge = model
        .of("factJudge")
        .into_iter()
        .find(|r| r.about("Récoltes"))
        .unwrap();
    assert!(judge.user.contains("examine them first"));
    assert!(judge.user.contains("«Poireaux»"));
    // Signalé, pas bloquant seul : pas de correction pour ça.
    assert!(!model.of("fix").iter().any(|r| r.about("Récoltes")));
    assert_eq!(brick_state(&host, 4), BrickState::Done);
}

#[tokio::test]
async fn an_internal_word_is_caught_and_corrected() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        (req.role == "section" && req.about("Les cultures")).then(|| {
            Reply::Text(format!(
                "## Les cultures\n\nAstuce atelier : {}",
                filler(395)
            ))
        })
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let fixes = model.of("fix");
    assert_eq!(fixes.len(), 1);
    assert!(
        fixes[0]
            .user
            .contains("remove the words \"astuce atelier\"")
    );
    let doc = host.file("guide-du-balcon.md").unwrap();
    assert!(!doc.to_lowercase().contains("atelier"));
}

#[tokio::test]
async fn one_correction_is_kept_only_if_it_has_fewer_noes() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        let section = req.section().unwrap_or_default();
        let text = match (req.role.as_str(), section.as_str()) {
            // Cultures : trop courte (1 « non »), puis trop courte avec une
            // tournure du moteur (2 « non ») → la première gardée.
            ("section", "Les cultures") => format!("PREMIÈRE {}", filler(100)),
            ("fix", "Les cultures") => format!("CORRIGÉE Astuce atelier {}", filler(100)),
            // Calendrier : trop court, puis juste → la correction gardée.
            ("section", "Le calendrier") => format!("PREMIER {}", filler(100)),
            ("fix", "Le calendrier") => format!("CORRIGÉ {}", filler(398)),
            _ => return None,
        };
        Some(Reply::Text(format!("## {section}\n\n{text}")))
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(model.of("fix").len(), 2, "UNE correction par partie");
    let fix = model
        .of("fix")
        .into_iter()
        .find(|r| r.about("Les cultures"))
        .unwrap();
    assert!(
        fix.user
            .contains("length: the text has 103 words; it must have about 400 words: lengthen it by about 297 words")
    );
    let doc = host.file("guide-du-balcon.md").unwrap();
    assert!(doc.contains("PREMIÈRE") && !doc.contains("CORRIGÉE"));
    assert!(doc.contains("CORRIGÉ ") && !doc.contains("PREMIER "));
    // Défaut de forme restant (pas de fond) : à revoir, pas de redécoupage.
    assert_eq!(brick_state(&host, 2), BrickState::ToReview);
    assert_eq!(brick_state(&host, 3), BrickState::Done);
    assert!(model.of("split").is_empty());
}

/// Phrase fausse du calendrier (citée par le juge).
const MANURE: &str = "Mets 200 g de fumier par m² en mars.";

/// Le calendrier porte une affirmation fausse, restée après la correction
/// (la correction n'a pas moins de « non » : la première version gardée).
fn wrong_calendar(req: &Req) -> Option<Reply> {
    if let Some(reply) = direct_fails(req) {
        return Some(reply);
    }
    match req.role.as_str() {
        "section" if req.about("Le calendrier") => Some(Reply::Text(format!(
            "## Le calendrier\n\n{}\n\nAu printemps, tu prépares le sol. {MANURE} Tu arroses \
             ensuite.\n\n{}",
            filler(300),
            filler(80)
        ))),
        "factJudge" if req.about("Le calendrier") => Some(Reply::Json(json!({"claims": [
            {"quote": "200 g de fumier par m²", "reasoning": "dose trop faible",
             "verdict": "wrong", "correction": "3 kg par m²"}]}))),
        _ => None,
    }
}

/// Banc du 25 nov., point D : une affirmation fausse restée après la
/// correction n'est plus redécoupée : passe chirurgicale sur sa seule
/// phrase, le reste de la partie inchangé, puis re-jugement de la phrase
/// reprise.
#[tokio::test]
async fn a_wrong_claim_left_after_the_correction_gets_a_surgical_pass() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, wrong_calendar);
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let fix = model
        .of("fix")
        .into_iter()
        .find(|r| r.about("Le calendrier"))
        .unwrap();
    assert!(fix.user.contains("wrong claim: «200 g de fumier par m²»"));
    assert!(fix.user.contains("Correction: 3 kg par m²"));
    assert!(model.of("split").is_empty());
    let surgery = model.of("surgery");
    assert_eq!(surgery.len(), 1);
    assert!(surgery[0].user.contains(&format!("SENTENCE: «{MANURE}»")));
    assert!(surgery[0].user.contains("WHAT IS WRONG: 3 kg par m²"));
    // Re-jugement des seules phrases reprises.
    assert!(
        model
            .of("factJudge")
            .iter()
            .any(|r| r.user.contains("<text>\nUne phrase juste.\n</text>"))
    );
    assert_eq!(brick_state(&host, 3), BrickState::Done);
    let doc = host.file("guide-du-balcon.md").unwrap();
    assert!(
        doc.contains("Au printemps, tu prépares le sol. Une phrase juste. Tu arroses ensuite.")
    );
    assert!(!doc.contains("200 g de fumier"));
    assert!(doc.contains(&filler(80)));
    assert_eq!(report(&host).to_review, 0);
}

/// Plan au calendrier long (600 mots) : il peut être redécoupé.
fn long_calendar_plan() -> Value {
    let mut plan = good_plan();
    plan["parts"][1]["words"] = json!(300);
    plan["parts"][2]["words"] = json!(600);
    plan
}

/// Le calendrier (600 mots) ne tient pas son but, même corrigé :
/// redécoupé en « Été » et « Hiver » (titres rendus avec des « ### »).
fn split_calendar(req: &Req) -> Option<Reply> {
    if let Some(reply) = direct_fails(req) {
        return Some(reply);
    }
    match req.role.as_str() {
        "plan" => Some(Reply::Json(long_calendar_plan())),
        "formJudge" if req.about("Le calendrier") => {
            Some(Reply::Json(form(questions(&req.user), &[1])))
        }
        "split" => Some(Reply::Json(json!({"analysis": "a", "parts": [
            {"title": "### Été", "goal": "Les mois chauds", "words": 300},
            {"title": "## #### Hiver", "goal": "Les mois froids", "words": 300}]}))),
        _ => None,
    }
}

/// Points D et E : un critère de fond non tenu dans une partie d'au moins
/// 600 mots la redécoupe ; titres des sous-parties relatifs au parent,
/// jamais empilés.
#[tokio::test]
async fn a_long_part_that_misses_its_goal_is_split_with_clean_titles() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, split_calendar);
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(model.of("split").len(), 1);
    assert_eq!(brick_state(&host, 3), BrickState::Split);
    assert_eq!(brick_state(&host, 301), BrickState::Done);
    assert_eq!(brick_state(&host, 302), BrickState::Done);
    let doc = host.file("guide-du-balcon.md").unwrap();
    assert!(doc.contains("## Le calendrier\n\n### Été"), "{doc}");
    assert!(doc.contains("\n### Hiver\n"));
    for line in doc.lines() {
        let rest = line.trim_start_matches('#').trim_start();
        assert!(!rest.starts_with('#'), "titre empilé : {line}");
    }
    // Hiver reçoit l'ordre de ne pas traiter Été.
    let winter = model
        .of("section")
        .into_iter()
        .find(|r| r.about("Hiver"))
        .unwrap();
    assert!(winter.user.contains("the content of « Été »"));
}

/// Point D : une partie de moins de 600 mots n'est jamais redécoupée (banc :
/// « Emplacement », 300 mots, redécoupée deux fois) : à revoir.
#[tokio::test]
async fn a_short_part_is_never_split() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        (req.role == "formJudge" && req.about("Le calendrier"))
            .then(|| Reply::Json(form(questions(&req.user), &[1])))
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert!(model.of("split").is_empty());
    assert_eq!(brick_state(&host, 3), BrickState::ToReview);
    assert_eq!(report(&host).to_review, 1);
}

/// Point D : profondeur 1 : des sous-parties longues et toujours fautives
/// ne se redécoupent plus.
#[tokio::test]
async fn splitting_stops_at_depth_one() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        let calendar = ["Le calendrier", "Été", "Hiver"]
            .iter()
            .any(|t| req.about(t));
        match req.role.as_str() {
            "plan" => Some(Reply::Json(long_calendar_plan())),
            "formJudge" if calendar => Some(Reply::Json(form(questions(&req.user), &[1]))),
            "split" => Some(Reply::Json(json!({"analysis": "a", "parts": [
                {"title": "Été", "goal": "Les mois chauds", "words": 600},
                {"title": "Hiver", "goal": "Les mois froids", "words": 600}]}))),
            _ => None,
        }
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(model.of("split").len(), 1);
    assert_eq!(brick_state(&host, 301), BrickState::ToReview);
    assert_eq!(brick_state(&host, 302), BrickState::ToReview);
}

#[tokio::test]
async fn a_refusal_is_marked_without_retrying() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        (req.role == "section" && req.about("Les cultures")).then(|| {
            Reply::Text("Je suis désolé, mais je ne peux pas écrire cette section.".into())
        })
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(model.of("refusal").len(), 1);
    assert_eq!(brick_state(&host, 2), BrickState::Refused);
    assert!(model.of("fix").is_empty());
    assert!(
        !model
            .of("factJudge")
            .iter()
            .any(|r| r.about("Les cultures"))
    );
    let doc = host.file("guide-du-balcon.md").unwrap();
    assert!(!doc.contains("désolé"));
}

#[tokio::test]
async fn a_looping_part_is_tried_again_warmer_then_left_to_review() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        if req.role != "section" {
            return None;
        }
        if req.about("Les cultures") {
            // Boucle, puis écrit.
            return (req.temperature < 0.9).then_some(Reply::Err(AgentError::Looping));
        }
        req.about("Le calendrier")
            .then_some(Reply::Err(AgentError::Looping))
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let crops: Vec<f32> = model
        .of("section")
        .iter()
        .filter(|r| r.about("Les cultures"))
        .map(|r| r.temperature)
        .collect();
    assert_eq!(crops, vec![0.7, 1.0]);
    assert_eq!(brick_state(&host, 2), BrickState::Done);
    let calendar = model
        .of("section")
        .iter()
        .filter(|r| r.about("Le calendrier"))
        .count();
    assert_eq!(calendar, 2);
    assert_eq!(brick_state(&host, 3), BrickState::ToReview);
}

/// Scénario riche : direct refusé, plan, correction, passe chirurgicale
/// (« Les cultures »), redécoupage (« Le calendrier »), relecture.
fn rich(req: &Req) -> Option<Reply> {
    if let Some(reply) = split_calendar(req) {
        return Some(reply);
    }
    match req.role.as_str() {
        "section" if req.about("Les cultures") => Some(Reply::Text(format!(
            "## Les cultures\n\n{} {MANURE}",
            filler(290)
        ))),
        "factJudge" if req.about("Les cultures") => Some(Reply::Json(json!({"claims": [
            {"quote": MANURE, "reasoning": "dose trop faible", "verdict": "wrong",
             "correction": "3 kg par m²"}]}))),
        _ => None,
    }
}

#[tokio::test]
async fn an_interrupted_project_resumes_without_redoing_anything() {
    // Passage complet de référence.
    let host = FakeHost::new(Autonomy::Ask);
    let reference = FakeModel::new(&host.trace, rich);
    assert!(matches!(
        run(&host, &reference).await,
        RunEnd::Delivered { .. }
    ));
    let full: Vec<(String, String)> = reference
        .log
        .lock()
        .unwrap()
        .iter()
        .map(|r| (r.role.clone(), r.user.clone()))
        .collect();
    let doc = host.file("guide-du-balcon.md").unwrap();
    assert!(full.len() > 30);
    // Interrompu à chaque requête, puis repris.
    for k in 0..full.len() {
        let host = FakeHost::new(Autonomy::Ask);
        let first = FakeModel::new(&host.trace, rich).interrupted_at(k, host.token());
        assert_eq!(
            run(&host, &first).await,
            RunEnd::Cancelled,
            "interruption {k}"
        );
        assert!(
            !host
                .events()
                .iter()
                .any(|e| matches!(e, EventKind::Finished { .. })),
            "une pause n'émet pas de fin"
        );
        host.resume();
        let second = FakeModel::new(&host.trace, rich);
        assert!(matches!(
            run(&host, &second).await,
            RunEnd::Delivered { .. }
        ));
        let mut done: Vec<(String, String)> = first
            .log
            .lock()
            .unwrap()
            .iter()
            .map(|r| (r.role.clone(), r.user.clone()))
            .collect();
        done.extend(
            second
                .log
                .lock()
                .unwrap()
                .iter()
                .map(|r| (r.role.clone(), r.user.clone())),
        );
        assert_eq!(done.len(), full.len(), "interruption {k} : rien refait");
        // Repris après la spécification : le nom est redonné (l'app a pu
        // être coupée entre la spécification et le renommage).
        assert_eq!(
            host.names.lock().unwrap().last().map(String::as_str),
            Some("Potager sur un balcon"),
            "interruption {k}"
        );
        assert!(done == full, "interruption {k} : même suite de requêtes");
        assert_eq!(host.file("guide-du-balcon.md").unwrap(), doc);
    }
    // Un projet livré relancé ne refait rien.
    let again = FakeModel::new(&host.trace, rich);
    assert!(matches!(run(&host, &again).await, RunEnd::Delivered { .. }));
    assert_eq!(again.calls(), 0);
}

#[tokio::test]
async fn the_project_yields_before_every_request() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, rich);
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let trace = host.trace.lock().unwrap().clone();
    let calls = trace.iter().filter(|t| t.starts_with("call:")).count();
    assert!(calls > 20);
    for (i, entry) in trace.iter().enumerate() {
        if entry.starts_with("call:") {
            assert_eq!(trace[i - 1], "yield", "requête {i} sans cession");
        }
    }
}

#[tokio::test]
async fn nothing_is_asked_of_the_model_while_the_user_works() {
    let host = FakeHost::new(Autonomy::Autonomous);
    host.free.send_replace(false);
    let model = FakeModel::new(&host.trace, nothing);
    let user = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(model.calls(), 0);
        assert_eq!(host.yields.load(std::sync::atomic::Ordering::SeqCst), 1);
        host.free.send_replace(true);
    };
    let (end, ()) = tokio::join!(run(&host, &model), user);
    assert!(matches!(end, RunEnd::Delivered { .. }));
}

#[tokio::test]
async fn cancelling_while_waiting_for_the_user_stops_cleanly() {
    let host = FakeHost::new(Autonomy::Autonomous);
    host.free.send_replace(false);
    let model = FakeModel::new(&host.trace, nothing);
    let token = host.token();
    let stop = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        token.cancel();
    };
    let bounded = tokio::time::timeout(Duration::from_secs(5), run(&host, &model));
    let (end, ()) = tokio::join!(bounded, stop);
    assert_eq!(end.expect("arrêt pendant la cession"), RunEnd::Cancelled);
    assert_eq!(model.calls(), 0);
}

#[tokio::test]
async fn a_cancelled_project_keeps_what_was_done() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, nothing).interrupted_at(4, host.token());
    assert_eq!(run(&host, &model).await, RunEnd::Cancelled);
    assert!(host.has(StateKey::Spec));
    assert!(!host.has(StateKey::Text { id: 0, version: 0 }));
    assert!(host.file("guide-du-balcon.md").is_none());
}

#[tokio::test]
async fn plan_autonomy_waits_for_the_plan_before_writing() {
    let host = FakeHost::new(Autonomy::Plan).answering(vec![
        Answer::Framing(vec!["Sud".into(), "2 m²".into()]),
        Answer::Cancelled,
    ]);
    let model = FakeModel::new(&host.trace, nothing);
    assert_eq!(run(&host, &model).await, RunEnd::Cancelled);
    assert!(model.of("direct").is_empty());
    assert!(model.of("section").is_empty());
    assert!(matches!(host.questions()[1], Question::ApprovePlan(_)));

    // Plan approuvé : la version directe suit le plan.
    let host = FakeHost::new(Autonomy::Plan);
    let model = FakeModel::new(&host.trace, nothing);
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let roles = model.roles();
    let judged = roles.iter().position(|r| r == "planJudge").unwrap();
    let direct = roles.iter().position(|r| r == "direct").unwrap();
    assert!(judged < direct);
    assert!(
        model.of("direct")[0]
            .user
            .contains("OUTLINE APPROVED BY THE USER")
    );
}

#[tokio::test]
async fn autonomous_never_asks() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        // Tirages non unanimes.
        (req.role == "strategy" && req.n == 1).then(|| {
            Reply::Json(
                json!({"analysis": "a", "strategy": "research", "deliverables": ["x"],
                               "framing_questions": ["Q ?", "R ?"]}),
            )
        })
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert!(host.questions().is_empty());
    assert!(host.events().contains(&EventKind::StrategyChosen {
        strategy: Strategy::Generic,
        unanimous: false
    }));
    // Les questions de cadrage deviennent des hypothèses.
    assert!(
        model.of("spec")[0]
            .user
            .contains("the user cannot be asked")
    );
}

#[tokio::test]
async fn split_votes_are_put_to_the_user() {
    let host = FakeHost::new(Autonomy::Ask).answering(vec![Answer::Strategy(Strategy::LongForm)]);
    let model = FakeModel::new(&host.trace, |req| {
        (req.role == "strategy" && req.n == 2).then(|| {
            Reply::Json(
                json!({"analysis": "a", "strategy": "generic", "deliverables": ["x"],
                               "framing_questions": ["Q ?", "R ?"]}),
            )
        })
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(
        host.questions()[0],
        Question::Strategy(vec![Strategy::LongForm, Strategy::Generic])
    );
    assert_eq!(report(&host).strategy, Strategy::LongForm);
}

#[tokio::test]
async fn a_forced_strategy_skips_the_vote() {
    let host = FakeHost::new(Autonomy::Ask).with_strategy(Strategy::Generic);
    let model = FakeModel::new(&host.trace, nothing);
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    // Un seul tirage, pour les questions de cadrage.
    assert_eq!(model.of("strategy").len(), 1);
    assert_eq!(report(&host).strategy, Strategy::Generic);

    let host = FakeHost::new(Autonomy::Ask).with_strategy(Strategy::Code);
    let model = FakeModel::new(&host.trace, nothing);
    assert_eq!(
        run(&host, &model).await,
        RunEnd::Failed {
            code: "strategyNotYet".into()
        }
    );
    assert_eq!(model.calls(), 0);
}

#[tokio::test]
async fn an_undecided_spec_is_refused_and_asked_again() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        (req.role == "spec" && req.n == 0).then(|| {
            let mut spec = json!({
                "analysis": "a", "goal": "Un guide. Astuce atelier : pailler.", "deliverables": ["g"],
                "criteria": ["QUAND a, LE LIVRABLE DOIT b"], "audience": "x", "language": "fr",
                "register": "informal", "units": "métrique ou impérial", "length_words": 1000,
                "glossary": [], "assumptions": [], "constraints": []});
            spec["glossary"] = json!([]);
            Reply::Json(spec)
        })
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let specs = model.of("spec");
    assert_eq!(specs.len(), 2);
    assert!(
        specs[1]
            .user
            .contains("\"units\" is not decided (it contains \"ou\")")
    );
    assert!(specs[1].user.contains("do not use these words"));
}

#[tokio::test]
async fn a_note_on_a_refused_plan_becomes_a_constraint() {
    let host = FakeHost::new(Autonomy::Ask).answering(vec![
        Answer::Framing(vec![]),
        Answer::Plan {
            approved: false,
            edited: None,
            note: Some("Ajoute une partie sur les nuisibles.".into()),
        },
    ]);
    let model = FakeModel::new(&host.trace, direct_fails);
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let plans = model.of("plan");
    assert_eq!(plans.len(), 2);
    assert!(
        plans[1]
            .user
            .contains("CONSTRAINTS:\n  1. Ajoute une partie sur les nuisibles.")
    );
    assert_eq!(host.get(StateKey::PlanRound), Some(json!(1)));
    // Les parties voient la consigne.
    assert!(
        model.of("section")[0]
            .user
            .contains("Ajoute une partie sur les nuisibles.")
    );

    // Refus sans note : arrêt.
    let host = FakeHost::new(Autonomy::Ask).answering(vec![
        Answer::Framing(vec![]),
        Answer::Plan {
            approved: false,
            edited: None,
            note: None,
        },
    ]);
    let model = FakeModel::new(&host.trace, direct_fails);
    assert_eq!(run(&host, &model).await, RunEnd::Rejected);
    assert!(model.of("section").is_empty());
}

#[tokio::test]
async fn an_edited_plan_is_checked_again() {
    let mut broken: Plan = super::plan::read_plan(&good_plan()).unwrap();
    broken.bricks[1].depends_on = vec![3];
    let mut edited = super::plan::read_plan(&good_plan()).unwrap();
    edited.bricks.insert(
        3,
        BrickPlan {
            id: 5,
            title: "Les nuisibles".into(),
            goal: "Reconnaître les pucerons".into(),
            kind: BrickKind::Text,
            words: 300,
            depends_on: vec![],
            criteria: vec![],
            forbidden: vec![],
        },
    );
    let approve = |plan: Plan| Answer::Plan {
        approved: true,
        edited: Some(plan),
        note: None,
    };
    let host = FakeHost::new(Autonomy::Ask).answering(vec![
        Answer::Framing(vec![]),
        approve(broken),
        approve(edited),
    ]);
    let model = FakeModel::new(&host.trace, direct_fails);
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let events = host.events();
    // Cycle : redemandé ; longueur dépassée : signalée, acceptée.
    assert!(events.contains(&EventKind::Warning {
        code: "planDefect.cycle".into()
    }));
    assert!(events.contains(&EventKind::Warning {
        code: "planDefect.lengthMismatch".into()
    }));
    let approvals = host
        .questions()
        .iter()
        .filter(|q| matches!(q, Question::ApprovePlan(_)))
        .count();
    assert_eq!(approvals, 2);
    assert!(model.of("section").iter().any(|r| r.about("Les nuisibles")));
}

#[tokio::test]
async fn contradictions_between_parts_are_judged() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        let dose = if req.about("Les cultures") {
            "Dose de fumier : 3 kg par m²"
        } else if req.about("Le calendrier") {
            "Dose de fumier : 200 g par m²"
        } else {
            return None;
        };
        match req.role.as_str() {
            "card" => Some(Reply::Json(
                json!({"summary": "s", "terms": [], "decisions": [dose],
                                              "lists": [], "exposes": []}),
            )),
            _ => None,
        }
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let judged = model.of("contradictions");
    assert_eq!(judged.len(), 1);
    assert!(judged[0].user.contains("3 kg par m²"));
    assert!(judged[0].user.contains("200 g par m²"));
}

#[tokio::test]
async fn the_whole_is_judged_on_its_outline_when_too_long() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let mut model = FakeModel::new(&host.trace, direct_fails);
    // Contexte juste : le livrable ne tient pas dans un jugement.
    model.context = 16_000 + 4_000;
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let whole = model
        .of("formJudge")
        .into_iter()
        .find(|r| r.user.contains("OUTLINE OF THE DOCUMENT TO ASSESS"))
        .expect("jugé sur le squelette");
    assert!(whole.user.contains("Summary: Résumé de"));
    assert!(report(&host).judged_on_outline);
    assert!(
        host.file(".atelier/rapport.md")
            .unwrap()
            .contains("squelette")
    );
}

#[tokio::test]
async fn an_unreadable_judge_is_reported_not_passed() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        (req.role == "factJudge" && req.about("Les cultures"))
            .then_some(Reply::Err(AgentError::Unreadable))
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(brick_state(&host, 2), BrickState::ToReview);
    assert!(report(&host).not_checked.contains(&"factJudge".to_owned()));
}

#[tokio::test]
async fn an_engine_error_fails_the_project_with_a_code() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        (req.role == "spec").then_some(Reply::Err(AgentError::RemoteOff))
    });
    assert_eq!(
        run(&host, &model).await,
        RunEnd::Failed {
            code: "remoteOff".into()
        }
    );
    let events = host.events();
    assert!(events.contains(&EventKind::Error {
        code: "remoteOff".into()
    }));
    assert_eq!(
        events.last(),
        Some(&EventKind::Finished {
            state: ProjectState::Failed
        })
    );
}

/// Le pipeline tourne dans une tâche tokio : son futur est `Send`, avec le
/// vrai modèle comme avec le faux.
#[allow(dead_code, reason = "vérifié à la compilation")]
fn the_pipeline_future_is_send(host: &dyn super::Host, worker: &crate::agent::Worker) {
    fn send<T: Send>(_: T) {}
    send(run(host, worker));
}

/// Formes JSON partagées avec le socle et la page (contrat 111).
#[test]
fn shared_types_have_a_stable_json_shape() {
    use super::types::{Event, Phase};
    let event = Event {
        seq: 3,
        at_ms: 10,
        kind: EventKind::PhaseStarted {
            phase: Phase::Build,
        },
    };
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({"seq": 3, "atMs": 10, "kind": {"type": "phaseStarted", "phase": "build"}})
    );
    let question = Question::Framing(vec!["Q ?".into()]);
    assert_eq!(
        serde_json::to_value(&question).unwrap(),
        json!({"type": "framing", "value": ["Q ?"]})
    );
    let answer: Answer = serde_json::from_value(
        json!({"type": "plan", "value": {"approved": true, "edited": null, "note": null}}),
    )
    .unwrap();
    assert_eq!(
        answer,
        Answer::Plan {
            approved: true,
            edited: None,
            note: None
        }
    );
    let activity = EventKind::Activity {
        activity: super::types::Activity::WritingBrick { id: 2 },
    };
    assert_eq!(
        serde_json::to_value(&activity).unwrap(),
        json!({"type": "activity", "activity": {"code": "writingBrick", "id": 2}})
    );
    assert_eq!(StateKey::Text { id: 3, version: 1 }.name(), "text:3:1");
    assert_eq!(
        StateKey::PlanAttempt {
            round: 0,
            attempt: 2
        }
        .name(),
        "planAttempt:0:2"
    );
}

// ---------------------------------------------------------------------------
// Relecture du lot 2 : un essai par défaut corrigé.

/// Point 1 : un mot du métier (« briques réfractaires ») n'est qu'un
/// avertissement : pas de correction, partie faite, dit au rapport.
#[tokio::test]
async fn a_trade_word_is_only_a_warning() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        (req.role == "section" && req.about("Les cultures")).then(|| {
            Reply::Text(format!(
                "## Les cultures\n\nLes briques réfractaires gardent la chaleur. {}",
                filler(394)
            ))
        })
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert!(!model.of("fix").iter().any(|r| r.about("Les cultures")));
    assert_eq!(brick_state(&host, 2), BrickState::Done);
    let report = report(&host);
    let cultures = report.bricks.iter().find(|b| b.id == 2).unwrap();
    assert!(
        cultures
            .checks
            .iter()
            .any(|c| c.criterion == "engineWord" && !c.blocking && c.evidence == "briques")
    );
}

/// Point 2 : unités métriques avec sigle et « / » : tranchées du premier
/// coup.
#[tokio::test]
async fn metric_units_with_slashes_are_decided() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        (req.role == "spec").then(|| {
            Reply::Json(json!({
                "analysis": "a", "goal": "Un guide du potager sur un balcon.",
                "deliverables": ["Guide du balcon"],
                "criteria": ["QUAND on lit, LE LIVRABLE DOIT lister les cultures"],
                "audience": "débutants", "language": "fr", "register": "informal",
                "units": "métrique (SI) : g, kg, m², L/m²", "length_words": 1000,
                "glossary": [], "assumptions": [], "constraints": []}))
        })
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(model.of("spec").len(), 1);
}

/// Point 2 : trois spécifications refusées : la dernière est gardée, avec un
/// avertissement (au lieu d'un échec).
#[tokio::test]
async fn after_three_refused_specs_the_last_one_is_kept() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        (req.role == "spec").then(|| {
            Reply::Json(json!({
                "analysis": "a", "goal": format!("Guide n° {}", req.n),
                "deliverables": ["Guide du balcon"],
                "criteria": ["QUAND on lit, LE LIVRABLE DOIT lister les cultures"],
                "audience": "débutants", "language": "fr", "register": "informal",
                "units": "métrique ou impérial", "length_words": 1000,
                "glossary": [], "assumptions": [], "constraints": []}))
        })
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(model.of("spec").len(), 3);
    let spec = host.get(StateKey::Spec).unwrap();
    assert_eq!(spec["goal"], json!("Guide n° 2"));
    let events = host.events();
    assert!(events.contains(&EventKind::Warning {
        code: "specBestOfThree".into()
    }));
    assert!(events.contains(&EventKind::Warning {
        code: "specDefect.undecided".into()
    }));
    assert!(
        report(&host)
            .warnings
            .contains(&"specBestOfThree".to_owned())
    );
}

/// Point 3 : une version directe refusée est corrigée UNE fois ; tenue après
/// correction, elle est livrée (pas de plan).
#[tokio::test]
async fn a_direct_version_is_corrected_once_before_the_plan() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| match (req.role.as_str(), req.n) {
        ("formJudge", 0) => Some(Reply::Json(form(6, &[1]))),
        ("directFix", _) => Some(Reply::Text(format!(
            "# Guide du balcon\n\nCORRIGÉ {}",
            filler(998)
        ))),
        _ => None,
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(model.of("directFix").len(), 1);
    assert!(model.of("directFix")[0].user.contains("Criterion C1"));
    assert!(model.of("plan").is_empty());
    let report = report(&host);
    assert_eq!(report.route, Route::Direct);
    assert!(host.file("guide-du-balcon.md").unwrap().contains("CORRIGÉ"));
}

/// Point 3 : une longueur un peu à côté (−30 %) ne vaut pas la route par
/// parties : livrée, l'écart dit au rapport.
#[tokio::test]
async fn a_direct_version_slightly_short_is_kept() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        matches!(req.role.as_str(), "direct" | "directFix")
            .then(|| Reply::Text(format!("# Guide du balcon\n\n{}", filler(700))))
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(model.of("directFix").len(), 1);
    assert!(model.of("plan").is_empty());
    let report = report(&host);
    assert_eq!(report.route, Route::Direct);
    assert!(
        report
            .checks
            .iter()
            .any(|c| c.criterion == "length" && !c.passed)
    );
}

/// Plan de 20 parties : introduction, 18 sections aux titres sans mot commun,
/// conclusion.
fn twenty_parts(words: u32) -> Value {
    let topics = [
        "Arrosage",
        "Compost",
        "Semis",
        "Paillage",
        "Tuteurs",
        "Pucerons",
        "Récoltes",
        "Engrais",
        "Bacs",
        "Lumière",
        "Vent",
        "Hiver",
        "Graines",
        "Outils",
        "Rempotage",
        "Taille",
        "Boutures",
        "Insectes",
    ];
    let mut parts = vec![
        json!({"id": 1, "title": "Introduction", "goal": "Présenter",
        "kind": "intro", "words": 500, "depends_on": [], "criteria": [], "forbidden": []}),
    ];
    for (i, topic) in topics.iter().enumerate() {
        let criteria: Vec<String> = if i < 4 {
            vec![format!("C{}", i + 1)]
        } else {
            Vec::new()
        };
        parts.push(
            json!({"id": i + 2, "title": topic, "goal": format!("Tout sur {topic}"),
            "kind": "text", "words": words, "depends_on": [], "criteria": criteria,
            "forbidden": []}),
        );
    }
    parts.push(json!({"id": 20, "title": "Conclusion", "goal": "Conclure",
        "kind": "conclusion", "words": 500, "depends_on": [], "criteria": [], "forbidden": []}));
    json!({"analysis": "a", "title": "Guide du balcon", "parts": parts})
}

/// Point 4 : 20 parties aux cartes lourdes : les consignes de la conclusion
/// (cartes de toutes les sections) restent sous le plafond, sans
/// « briefTooLong » (cartes réduites à leur résumé, listes bornées).
#[tokio::test]
async fn twenty_heavy_parts_keep_every_brief_under_the_cap() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let mut model = FakeModel::new(&host.trace, |req| match req.role.as_str() {
        "plan" => Some(Reply::Json(twenty_parts(1_500))),
        "card" => {
            let title = req.section().unwrap_or_default();
            let line = |what: &str, i: usize| format!("{title} {what} {i} {}", "z".repeat(60));
            Some(Reply::Json(json!({
                "summary": "r".repeat(880),
                "terms": (0..10).map(|i| line("terme", i)).collect::<Vec<_>>(),
                "decisions": (0..10).map(|i| format!("{title} sujet{i} : {}", "v".repeat(170))).collect::<Vec<_>>(),
                "lists": (0..6).map(|l| json!({"name": format!("{title} liste{l}"),
                    "items": (0..40).map(|i| line("élément", i)).collect::<Vec<_>>()})).collect::<Vec<_>>(),
                "exposes": (0..10).map(|i| line("expose", i)).collect::<Vec<_>>(),
            })))
        }
        _ => None,
    });
    model.length = 28_000;
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let tokens = |text: &str| text.chars().count().div_ceil(4);
    let conclusion = model
        .of("section")
        .into_iter()
        .find(|r| r.about("Conclusion"))
        .unwrap();
    assert!(
        tokens(&conclusion.user) <= super::build::BRIEF_MAX,
        "{} jetons",
        tokens(&conclusion.user)
    );
    // Toutes les sections de la conclusion, par leur résumé.
    assert_eq!(conclusion.user.matches("Section « ").count(), 18);
    for req in model.of("section") {
        assert!(tokens(&req.user) <= super::build::BRIEF_MAX);
    }
    assert!(!host.events().contains(&EventKind::Warning {
        code: "briefTooLong".into()
    }));
}

/// Point 5 : une sous-partie reçoit la carte de la dépendance de son parent,
/// et les voisines de son parent aux bords.
#[tokio::test]
async fn a_sub_part_inherits_its_parent_inputs_and_neighbours() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, split_calendar);
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let section = |title: &str| {
        model
            .of("section")
            .into_iter()
            .find(|r| r.about(title))
            .unwrap()
    };
    let summer = section("Été");
    // « Le calendrier » dépend des « Cultures » : sa sous-partie aussi.
    assert!(summer.user.contains("Section « Les cultures »"));
    assert!(summer.user.contains("Before: « Les cultures »"));
    assert!(summer.user.contains("After: « Hiver »"));
    let winter = section("Hiver");
    assert!(winter.user.contains("Before: « Été »"));
    assert!(winter.user.contains("After: « Conclusion »"));
}

/// Point 6 : le cas « poireaux/fraises » du lot 0 : signalés hors registre
/// dans « Récoltes », ils n'entrent pas dans la liste des cultures que voient
/// les parties suivantes.
#[tokio::test]
async fn strays_never_join_a_retained_list() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        match req.role.as_str() {
            "section" if req.about("Récoltes") => {
                return Some(Reply::Text(format!(
                    "## Récoltes\n\n{}\n\n- Tomates : en août\n- Poireaux : en novembre\n\
                     - Fraises : en juin",
                    filler(188)
                )));
            }
            "card" if req.about("Récoltes") => {
                return Some(Reply::Json(json!({
                    "summary": "Les récoltes.", "terms": [], "decisions": [],
                    "lists": [{"name": "Cultures", "items": ["Tomates", "Poireaux", "Fraises"]}],
                    "exposes": []})));
            }
            _ => {}
        }
        crops_plan(req)
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let conclusion = model
        .of("section")
        .into_iter()
        .find(|r| r.about("Conclusion"))
        .unwrap();
    assert!(
        conclusion
            .user
            .contains("- cultures: tomate; radis; laitue\n"),
        "{}",
        conclusion.user
    );
    assert!(
        !conclusion
            .user
            .contains("cultures: tomate; radis; laitue; ")
    );
}

/// Point 7 : une décision contraire au registre est signalée dès l'ajout
/// (événement et vérification de la partie), la première gardée.
#[tokio::test]
async fn a_decision_against_the_registry_is_reported() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        let dose = if req.about("Les cultures") {
            "Dose de fumier : 3 kg par m²"
        } else if req.about("Le calendrier") {
            "Dose de fumier : 200 g par m²"
        } else {
            return None;
        };
        (req.role == "card").then(|| {
            Reply::Json(json!({"summary": "s", "terms": [], "decisions": [dose],
                               "lists": [], "exposes": []}))
        })
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert!(host.events().contains(&EventKind::Warning {
        code: "registryConflict".into()
    }));
    let calendar = report(&host)
        .bricks
        .into_iter()
        .find(|b| b.id == 3)
        .unwrap();
    let conflict = calendar
        .checks
        .iter()
        .find(|c| c.criterion == "registryConflict")
        .expect("vérification");
    assert!(conflict.evidence.contains("3 kg") && conflict.evidence.contains("200 g"));
    // Le registre de la conclusion ne garde que la première valeur.
    let conclusion = model
        .of("section")
        .into_iter()
        .find(|r| r.about("Conclusion"))
        .unwrap();
    let registry = conclusion
        .user
        .split("4. DECISIONS ALREADY TAKEN")
        .nth(1)
        .and_then(|rest| rest.split("5. INPUTS").next())
        .unwrap();
    assert!(registry.contains("Dose de fumier : 3 kg"));
    assert!(!registry.contains("200 g"));
}

fn with_document(host: FakeHost, chars: usize) -> FakeHost {
    let mut host = host;
    let mut text: String = "Le document dit : 5 kg de fumier par m². "
        .repeat(chars / 20 + 1)
        .chars()
        .take(chars - 3)
        .collect();
    text.push_str("FIN");
    host.info.attachments = vec![super::types::Attachment {
        name: "notes.md".into(),
        text,
    }];
    host
}

/// Point 8 : le juge d'exactitude voit les pièces jointes, qui font foi.
#[tokio::test]
async fn the_fact_judge_reads_the_attachments() {
    let host = with_document(FakeHost::new(Autonomy::Autonomous), 2_000);
    let model = FakeModel::new(&host.trace, nothing);
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let judge = &model.of("factJudge")[0];
    assert!(judge.user.contains("5 kg de fumier par m²"));
    assert!(
        judge
            .user
            .contains("supported by the documents supplied is right")
    );
}

/// Point 9 (et ajout) : 33 000 caractères tiennent dans un contexte de
/// 32 768 jetons (version directe tentée, rien de coupé) ; avec 16 384, ils
/// sont coupés : avertissement, `documents` non vérifiés au rapport, pas de
/// version directe.
#[tokio::test]
async fn attachments_follow_the_context_and_a_cut_is_reported() {
    let host = with_document(FakeHost::new(Autonomy::Autonomous), 33_000);
    let model = FakeModel::new(&host.trace, nothing);
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let direct = &model.of("direct")[0];
    assert!(direct.user.contains("FIN\n</document>"));
    assert!(!host.events().contains(&EventKind::Warning {
        code: "attachmentsCut".into()
    }));
    assert!(!report(&host).not_checked.contains(&"documents".to_owned()));

    let host = with_document(FakeHost::new(Autonomy::Autonomous), 33_000);
    let mut model = FakeModel::new(&host.trace, nothing);
    model.context = 16_384;
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert!(model.of("direct").is_empty());
    assert!(host.events().contains(&EventKind::Warning {
        code: "attachmentsCut".into()
    }));
    let report = report(&host);
    assert!(report.not_checked.contains(&"documents".to_owned()));
    assert!(
        host.file(".atelier/rapport.md")
            .unwrap()
            .contains("seul leur début a été lu")
    );
}

fn timeout() -> AgentError {
    AgentError::Model(crate::llm::LlmError::Inference(
        crate::inference::InferenceError::Timeout,
    ))
}

/// Point 10 : une erreur passagère vaut jusqu'à 3 nouveaux essais, attente
/// croissante ; au-delà, le projet échoue avec le code de l'erreur.
#[tokio::test(start_paused = true)]
async fn a_transient_error_is_tried_again_before_failing() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        (req.role == "spec" && req.n < 2).then(|| Reply::Err(timeout()))
    });
    let started = tokio::time::Instant::now();
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(model.of("spec").len(), 3);
    // 5 s puis 15 s d'attente.
    assert!(started.elapsed() >= Duration::from_secs(20));
    let retries = host
        .events()
        .iter()
        .filter(|e| {
            **e == EventKind::Warning {
                code: "modelRetry".into(),
            }
        })
        .count();
    assert_eq!(retries, 2);

    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        (req.role == "direct").then(|| Reply::Err(timeout()))
    });
    assert_eq!(
        run(&host, &model).await,
        RunEnd::Failed {
            code: "timeout".into()
        }
    );
    assert_eq!(model.of("direct").len(), 4);
}

/// Point 11 : une longueur au-delà de 20 parties de 1 500 mots est bornée à
/// 30 000, avec un avertissement au rapport.
#[tokio::test]
async fn an_impossible_length_is_capped() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let mut model = FakeModel::new(&host.trace, |req| {
        (req.role == "plan").then(|| Reply::Json(twenty_parts(1_500)))
    });
    model.length = 50_000;
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(model.of("spec").len(), 1);
    assert_eq!(
        host.get(StateKey::Spec).unwrap()["lengthWords"],
        json!(30_000)
    );
    assert!(host.events().contains(&EventKind::Warning {
        code: "lengthCapped".into()
    }));
    let report = report(&host);
    assert_eq!(report.target_words, Some(30_000));
    assert!(report.warnings.contains(&"lengthCapped".to_owned()));
}

/// Point 16 : la question de stratégie ne propose que ce que ce lot sait
/// mener.
#[tokio::test]
async fn the_strategy_question_offers_only_available_strategies() {
    let host = FakeHost::new(Autonomy::Ask);
    let model = FakeModel::new(&host.trace, |req| {
        (req.role == "strategy" && req.n > 0).then(|| {
            Reply::Json(
                json!({"analysis": "a", "strategy": "research", "deliverables": ["x"],
                       "framing_questions": ["Q ?", "R ?"]}),
            )
        })
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(
        host.questions()[0],
        Question::Strategy(vec![Strategy::LongForm, Strategy::Generic])
    );
}

/// Point 17 : écriture du livrable refusée : code stable, la raison du socle
/// au journal seulement.
#[tokio::test]
async fn a_refused_write_fails_with_a_stable_code() {
    let host = FakeHost::new(Autonomy::Autonomous);
    *host.refuse_writes.lock().unwrap() = Some("io".into());
    let model = FakeModel::new(&host.trace, nothing);
    assert_eq!(
        run(&host, &model).await,
        RunEnd::Failed {
            code: "deliverWriteFailed".into()
        }
    );
    assert!(host.events().contains(&EventKind::Error {
        code: "deliverWriteFailed".into()
    }));
}

/// Point 4 : plafond effectif des consignes = contexte − réflexion − réponse
/// quand le contexte est court (20 000 jetons : 6 000 pour une section) ; les
/// documents partent avant de dépasser.
#[tokio::test]
async fn a_short_context_lowers_the_brief_cap() {
    let host = with_document(FakeHost::new(Autonomy::Autonomous), 30_000);
    let mut model = FakeModel::new(&host.trace, nothing);
    model.context = 20_000;
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let tokens = |text: &str| text.chars().count().div_ceil(4);
    let sections = model.of("section");
    assert!(!sections.is_empty());
    for req in &sections {
        let cap = if req.about("Les cultures") || req.about("Le calendrier") {
            6_000
        } else {
            7_000
        };
        assert!(tokens(&req.user) <= cap, "{} jetons", tokens(&req.user));
    }
}

// ---------------------------------------------------------------------------
// Banc réel du 25 nov. (Qwen3.8-27B) : un essai par correction (A à I).

/// Spécification du banc : critères d'ensemble (longueur totale, nombre de
/// titres) et critères de partie, en capitales.
fn bench_spec(req: &Req) -> Option<Reply> {
    (req.role == "spec").then(|| {
        Reply::Json(json!({
            "analysis": "a", "goal": "Un guide du potager sur un balcon.",
            "deliverables": ["Guide du balcon"],
            "criteria": [
                {"text": "LORSQU'ON COMPTE LES MOTS, LE DOCUMENT DOIT COMPRENDRE ENTRE 900 ET 1 100 MOTS.",
                 "scope": "section"},
                {"text": "Le guide contient au moins quatre titres Markdown.", "scope": "document"},
                {"text": "La partie sur les cultures liste au moins huit légumes.", "scope": "section"},
                "LORSQU'ON LIT LA SECTION CALENDRIER, LE LIVRABLE DOIT DONNER LES MOIS DE SEMIS."],
            "audience": "débutants", "language": "fr", "register": "informal",
            "units": "métrique : g, kg, m, L", "length_words": 1000,
            "glossary": [], "assumptions": [], "constraints": []}))
    })
}

/// Point A : les critères du document entier (donnés par la
/// spécification, ou reconnus par le programme) ne sont jamais attribués à
/// une partie ni jugés sur elle ; ils le sont sur la version directe et sur
/// le document assemblé.
#[tokio::test]
async fn document_criteria_are_judged_on_the_whole_only() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = bench_spec(req).or_else(|| direct_fails(req)) {
            return Some(reply);
        }
        // Le plan attribue C1 et C2 (d'ensemble) aux parties, comme au banc.
        (req.role == "plan").then(|| {
            let mut plan = good_plan();
            plan["parts"][0]["criteria"] = json!(["C1", "C2"]);
            Reply::Json(plan)
        })
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let spec: super::types::Spec =
        serde_json::from_value(host.get(StateKey::Spec).unwrap()).unwrap();
    use super::types::CriterionScope::{Document, Section};
    let scopes: Vec<_> = spec.criteria.iter().map(|c| c.scope).collect();
    assert_eq!(scopes, vec![Document, Document, Section, Section]);
    assert!(model.of("plan")[0].user.contains("[whole document]"));
    assert!(host.events().contains(&EventKind::Warning {
        code: "documentCriteriaRemoved".into()
    }));
    let plan: Plan = serde_json::from_value(host.get(StateKey::Plan).unwrap()).unwrap();
    assert!(
        plan.bricks
            .iter()
            .all(|b| !b.criteria.contains(&"C1".to_owned())
                && !b.criteria.contains(&"C2".to_owned()))
    );
    // Plan sans défaut (C1, C2 n'ont pas à être couverts) : jugé.
    assert_eq!(model.of("planJudge").len(), 1);
    // C1 (mots) et C2 (titres) sont mesurables : comptés par le programme
    // sur la version directe et sur le document assemblé, jamais posés à un
    // juge.
    for judge in model.of("formJudge") {
        let asks = |id: &str| judge.user.contains(&format!("Criterion {id} ("));
        assert!(!asks("C1") && !asks("C2"), "{}", judge.user);
        assert!(!judge.user.contains("900 et 1 100 mots"));
    }
    let report = report(&host);
    for id in ["C1", "C2"] {
        let check = report.checks.iter().find(|c| c.criterion == id).unwrap();
        assert_eq!(check.method, CheckMethod::Program, "{id}");
    }
}

/// Point F : critères en casse normale (consigne de la spécification, et
/// casse de phrase rendue par programme à un texte en capitales).
#[tokio::test]
async fn capital_criteria_are_written_in_normal_case() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, bench_spec);
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let spec: super::types::Spec =
        serde_json::from_value(host.get(StateKey::Spec).unwrap()).unwrap();
    assert_eq!(
        spec.criteria[0].text,
        "Lorsqu'on compte les mots, le document doit comprendre entre 900 et 1 100 mots."
    );
    assert_eq!(
        spec.criteria[1].text,
        "Le guide contient au moins quatre titres Markdown."
    );
    assert_eq!(
        spec.criteria[3].text,
        "Lorsqu'on lit la section calendrier, le livrable doit donner les mois de semis."
    );
    let call = super::prompts::spec_call("t", &[], 100, "fr", "", &[]);
    assert!(!call.system.contains("THE DELIVERABLE MUST"));
    assert!(call.system.contains("never in capital letters"));
}

/// Affirmations réelles du banc : deux « douteuses », une « fausse » dont le
/// raisonnement ne dit que « trop général ».
fn bench_doubts() -> Value {
    json!({"claims": [
        {"quote": "Les jeunes plants ont besoin de 0,5 L par plant par jour.",
         "reasoning": "C'est une valeur trop générale. Le besoin en eau dépend de la taille du plant, de la variété, de la température.",
         "verdict": "wrong", "correction": "Adapter selon la plante."},
        {"quote": "L'oignon déconcerte la mouche de la carotte.",
         "reasoning": "Association classique, efficacité pas démontrée de façon fiable.",
         "verdict": "doubtful", "correction": ""},
        {"quote": "à moins de 5 m d'un point d'eau",
         "reasoning": "5 m n'est pas une limite technique universelle.",
         "verdict": "doubtful", "correction": ""},
        {"quote": "Ton carré de 4 m² mesure 2 m par 2 m.", "reasoning": "2 × 2 = 4.",
         "verdict": "right", "correction": ""}]})
}

/// Points B et I : les « douteux » (et un « faux » qui ne dit que « trop
/// général ») ne bloquent pas : la version directe est livrée sans
/// correction ; le rapport les compte et les liste à part.
#[tokio::test]
async fn doubtful_claims_never_block_the_direct_version() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        (req.role == "factJudge").then(|| Reply::Json(bench_doubts()))
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert!(model.of("directFix").is_empty());
    assert!(model.of("plan").is_empty());
    // Les verdicts définis, avec leurs exemples, dans la consigne du juge.
    let judge = &model.of("factJudge")[0].user;
    assert!(
        judge.contains("\"doubtful\": a simplification, a rule of thumb that is not universal")
    );
    assert!(judge.contains("Such claims are NOT wrong."));
    assert!(judge.contains("never a rewritten sentence of the text"));
    let report = report(&host);
    assert_eq!(report.route, Route::Direct);
    assert_eq!(report.doubtful, 3);
    assert_eq!(report.to_review, 0);
    let readable = host.file(".atelier/rapport.md").unwrap();
    let doubts = readable.split("## Affirmations douteuses").nth(1).unwrap();
    assert!(doubts.contains("0,5 L par plant"));
    assert!(doubts.contains("moins de 5 m"));
    let review = readable.split("## À revoir").nth(1).unwrap();
    assert!(review.trim_start().starts_with("Rien."));
}

/// Version directe du banc : bonne, avec une phrase fausse au milieu.
const TOMATO: &str = "Une tomate trop mûre peut attirer les pucerons.";

fn direct_with_tomato() -> String {
    format!(
        "# Guide du balcon\n\n{}\n\n## Récoltes\n\nTu cueilles tôt. {TOMATO} Tu ranges au \
         frais.\n\n{}",
        filler(500),
        filler(480)
    )
}

/// Le juge d'exactitude de la version directe : la phrase de la tomate est
/// fausse ; `extra` affirmations fausses de plus (introuvables dans le
/// texte).
fn tomato_judge(extra: usize) -> Value {
    let mut claims = vec![json!({"quote": TOMATO,
        "reasoning": "Les pucerons visent les jeunes pousses, pas les fruits.",
        "verdict": "wrong", "correction": "Les pucerons visent les pousses tendres."})];
    for i in 0..extra {
        claims.push(
            json!({"quote": format!("affirmation {i}"), "reasoning": "faux",
            "verdict": "wrong", "correction": "autre valeur"}),
        );
    }
    json!({ "claims": claims })
}

/// Point C : après la correction ciblée, une seule affirmation fausse reste
/// (le cas de la tomate) : passe chirurgicale sur sa phrase, le reste
/// recopié à l'identique, la phrase reprise jugée de nouveau ; la version
/// directe est livrée (pas de plan ×13).
#[tokio::test]
async fn a_direct_version_with_a_few_wrong_claims_gets_a_surgical_pass() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| match req.role.as_str() {
        "direct" | "directFix" => Some(Reply::Text(direct_with_tomato())),
        "factJudge" if req.user.contains(TOMATO) => Some(Reply::Json(tomato_judge(0))),
        "surgery" => Some(Reply::Json(json!({"edits": [
            {"n": 1, "action": "fix", "sentence": "Une tomate trop mûre attire les guêpes."}]}))),
        _ => None,
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(model.of("directFix").len(), 1);
    assert!(model.of("plan").is_empty());
    let surgery = model.of("surgery");
    assert_eq!(surgery.len(), 1);
    assert!(surgery[0].user.contains(&format!("SENTENCE: «{TOMATO}»")));
    // Le texte entier ne passe pas par le modèle : seul le paragraphe.
    assert!(!surgery[0].user.contains(&filler(500)));
    let judges = model.of("factJudge");
    assert_eq!(judges.len(), 3);
    assert!(
        judges[2]
            .user
            .contains("<text>\nUne tomate trop mûre attire les guêpes.\n</text>")
    );
    let doc = host.file("guide-du-balcon.md").unwrap();
    assert_eq!(
        doc,
        direct_with_tomato().replace(TOMATO, "Une tomate trop mûre attire les guêpes.")
    );
    let report = report(&host);
    assert_eq!(report.route, Route::Direct);
    assert_eq!(report.to_review, 0);
}

/// Point C : une passe rejetée (modification hors de son cadre) laisse la
/// version telle quelle, livrée, la phrase « à revoir » ; plus de cinq
/// affirmations fausses : le plan.
#[tokio::test]
async fn a_rejected_surgery_still_delivers_and_too_many_wrongs_go_to_the_plan() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| match req.role.as_str() {
        "direct" | "directFix" => Some(Reply::Text(direct_with_tomato())),
        "factJudge" if req.user.contains(TOMATO) => Some(Reply::Json(tomato_judge(0))),
        "surgery" => Some(Reply::Json(json!({"edits": [
            {"n": 1, "action": "fix", "sentence": "Une tomate.\n\n## Autre partie"}]}))),
        _ => None,
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert!(model.of("plan").is_empty());
    assert!(host.events().contains(&EventKind::Warning {
        code: "surgeryRejected".into()
    }));
    assert_eq!(
        host.file("guide-du-balcon.md").unwrap(),
        direct_with_tomato()
    );
    let report = report(&host);
    assert_eq!(report.to_review, 1);
    let readable = host.file(".atelier/rapport.md").unwrap();
    let review = readable.split("## À revoir").nth(1).unwrap();
    assert!(review.contains(TOMATO));

    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| match req.role.as_str() {
        "direct" | "directFix" => Some(Reply::Text(direct_with_tomato())),
        "factJudge" if req.user.contains(TOMATO) => Some(Reply::Json(tomato_judge(5))),
        _ => None,
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert!(model.of("surgery").is_empty());
    assert_eq!(model.of("plan").len(), 1);
}

/// Point G : une correction qui recopie la remarque du juge (8 mots ou
/// plus) est signalée (non bloquante) ; la consigne de correction dit
/// d'intégrer sans recopier.
#[tokio::test]
async fn a_copied_remark_is_flagged() {
    const REMARK: &str = "ajouter de la chaux seulement après analyse du sol en respectant les recommandations locales";
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, move |req| {
        if let Some(reply) = direct_fails(req) {
            return Some(reply);
        }
        match req.role.as_str() {
            "section" if req.about("Les cultures") => Some(Reply::Text(format!(
                "## Les cultures\n\n{} Mets 1 kg de chaux par m².",
                filler(390)
            ))),
            "fix" if req.about("Les cultures") => Some(Reply::Text(format!(
                "## Les cultures\n\n{} Tu peux {REMARK}.",
                filler(388)
            ))),
            "factJudge" if req.about("Les cultures") && req.user.contains("1 kg de chaux") => Some(
                Reply::Json(json!({"claims": [{"quote": "1 kg de chaux par m²",
                    "reasoning": "dose dix fois trop forte", "verdict": "wrong",
                    "correction": REMARK}]})),
            ),
            _ => None,
        }
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let fix = model
        .of("fix")
        .into_iter()
        .find(|r| r.about("Les cultures"))
        .unwrap();
    assert!(fix.user.contains("never copy a note word for word"));
    let report = report(&host);
    let cultures = report.bricks.iter().find(|b| b.id == 2).unwrap();
    let copied = cultures
        .checks
        .iter()
        .find(|c| c.criterion == "copiedRemark")
        .expect("remarque recopiée signalée");
    assert!(!copied.blocking);
    assert!(
        copied
            .evidence
            .contains("en respectant les recommandations locales")
    );
    assert_eq!(brick_state(&host, 2), BrickState::Done);
    let readable = host.file(".atelier/rapport.md").unwrap();
    assert!(
        readable
            .split("## Signalements")
            .nth(1)
            .unwrap()
            .contains("recommandations locales")
    );
}

/// Point H : relecture de la langue avant la livraison, passage par passage
/// (titres exclus), avec le garde-fou de la Correction et celui du livrable
/// (tableau déformé : correction écartée, passage gardé).
#[tokio::test]
async fn the_deliverable_is_proofread_before_delivery() {
    const SOIL: &str = "Tu tasse la terre végétaine.";
    const TABLE: &str = "| Légume | Écart |\n|---|---|\n| Radis | 3 cm |";
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| match req.role.as_str() {
        "direct" => Some(Reply::Text(format!(
            "# Guide du balcon\n\n## Sol\n\n{SOIL} {}\n\n## Semis\n\n{}\n\n{TABLE}",
            filler(490),
            filler(490)
        ))),
        "proofread" if req.user.contains("végétaine") => Some(Reply::Text(
            req.user.replace(SOIL, "Tu tasses la terre végétale."),
        )),
        // Correction qui déforme le tableau : écartée.
        "proofread" if req.user.contains("| Radis |") => Some(Reply::Text(
            req.user.replace("| Radis | 3 cm |", "| Radis 3 cm |"),
        )),
        _ => None,
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    // Sol ; Semis (490 mots) ; le tableau (au-delà de 300 mots, un nouveau
    // passage).
    let passages = model.of("proofread");
    assert_eq!(passages.len(), 3);
    assert!(passages.iter().all(|p| !p.user.contains("## ")));
    let doc = host.file("guide-du-balcon.md").unwrap();
    assert!(doc.contains("## Sol\n\nTu tasses la terre végétale. "));
    assert!(doc.ends_with(TABLE));
    let report = report(&host);
    assert_eq!(
        (
            report.proofread_blocks,
            report.proofread_changed,
            report.proofread_rejected
        ),
        (3, 1, 1)
    );
    assert!(host.file(".atelier/rapport.md").unwrap().contains(
        "Relecture de la langue : 3 passages relus, 1 corrigés ; corrections écartées par le \
         garde-fou : 1."
    ));
}

/// Points A et F, reprise : une spécification et un plan sauvés avant ces
/// règles (état du banc : critères en capitales, sans portée, critère
/// d'ensemble attribué à une partie) sont remis d'aplomb à la relecture.
#[tokio::test]
async fn a_spec_and_plan_saved_before_the_scope_rule_are_settled_on_resume() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let mut spec = host_spec();
    spec["criteria"] = json!([
        {"id": "C1", "text": "LORSQU'ON COMPTE LES MOTS, LE DOCUMENT DOIT COMPRENDRE ENTRE 900 ET 1 100 MOTS."},
        {"id": "C2", "text": "QUAND on lit, LE LIVRABLE DOIT lister les cultures"},
        {"id": "C3", "text": "QUAND on lit, LE LIVRABLE DOIT dire l'exposition"},
        {"id": "C4", "text": "QUAND on lit, LE LIVRABLE DOIT donner un calendrier"}]);
    let mut plan = super::plan::read_plan(&good_plan()).unwrap();
    plan.bricks[1].criteria = vec!["C1".into(), "C2".into()];
    {
        let mut state = host.state.lock().unwrap();
        state.insert(StateKey::Spec.name(), spec);
        state.insert(StateKey::Plan.name(), serde_json::to_value(&plan).unwrap());
    }
    let model = FakeModel::new(&host.trace, direct_fails);
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert!(model.of("spec").is_empty() && model.of("plan").is_empty());
    assert!(host.events().contains(&EventKind::Warning {
        code: "documentCriteriaRemoved".into()
    }));
    let parts: Vec<String> = model
        .of("formJudge")
        .into_iter()
        .filter(|r| r.user.contains("\nSECTION TO ASSESS"))
        .map(|r| r.user)
        .collect();
    assert_eq!(parts.len(), 4);
    assert!(parts.iter().all(|u| !u.contains("Criterion C1 (")));
    assert!(parts.iter().any(|u| u.contains("Criterion C2 (")));
    assert!(
        model.of("direct")[0]
            .user
            .contains("Lorsqu'on compte les mots, le document doit comprendre")
    );
}

/// Spécification par défaut du faux modèle, telle que sauvée.
fn host_spec() -> Value {
    json!({
        "goal": "Un guide du potager sur un balcon.", "deliverables": ["Guide du balcon"],
        "criteria": [], "audience": "débutants", "language": "fr", "register": "informal",
        "units": "métrique : g, kg, m, L", "lengthWords": 1000, "glossary": [],
        "assumptions": [], "constraints": []})
}

// ---------------------------------------------------------------------------
// Critères mesurables (banc du 25 nov. : 3 239 mots jugés « environ 2 800 »
// par le juge de forme, version directe refusée)

/// Spécification par défaut du faux modèle, critères remplacés.
fn spec_with(criteria: Value) -> impl Fn(&Req) -> Option<Reply> + Send + Sync + 'static {
    move |req: &Req| {
        (req.role == "spec").then(|| {
            Reply::Json(json!({
                "analysis": "a", "goal": "Un guide du potager sur un balcon.",
                "deliverables": ["Guide du balcon"], "criteria": criteria.clone(),
                "audience": "débutants", "language": "fr", "register": "informal",
                "units": "métrique : g, kg, m, L", "length_words": 1000,
                "glossary": [], "assumptions": [], "constraints": []}))
        })
    }
}

/// Le juge de forme compte mal : « non » à toute question qui contient un
/// de ces mots.
fn form_no_on(req: &Req, needles: &[&str]) -> Value {
    let lines: Vec<String> = req
        .user
        .split("QUESTIONS\n")
        .nth(1)
        .unwrap_or("")
        .lines()
        .take_while(|l| !l.trim().is_empty())
        .map(str::to_owned)
        .collect();
    let no: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| needles.iter().any(|n| l.contains(n)))
        .map(|(i, _)| i + 1)
        .collect();
    form(lines.len(), &no)
}

/// Version directe à titres : « # Guide du balcon », puis trois sections de
/// `words` mots chacune.
fn headed_direct(words: u32) -> String {
    let mut out = "# Guide du balcon".to_owned();
    for title in ["Les cultures", "Le soleil", "Le calendrier"] {
        out.push_str(&format!("\n\n## {title}\n\n{}", filler(words)));
    }
    out
}

/// Point 1 : le nombre de mots et de titres est compté par le programme ;
/// les juges ne voient ni ces critères ni la longueur. Le juge du banc (qui
/// répond « non » sur les mots et les titres) n'a plus rien à refuser.
#[tokio::test]
async fn measurable_criteria_are_counted_by_the_program_never_by_a_judge() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = bench_spec(req) {
            return Some(reply);
        }
        match req.role.as_str() {
            "formJudge" => Some(Reply::Json(form_no_on(req, &["mots", "titres"]))),
            "direct" => Some(Reply::Text(headed_direct(330))),
            _ => None,
        }
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let report = report(&host);
    assert_eq!(report.route, Route::Direct);
    assert!(model.of("directFix").is_empty() && model.of("plan").is_empty());
    for judge in model.of("formJudge") {
        assert!(!judge.user.contains("900 et 1 100 mots"), "{}", judge.user);
        assert!(!judge.user.contains("quatre titres"));
        assert!(!judge.user.contains("LENGTH:"));
    }
    let check = |id: &str| report.checks.iter().find(|c| c.criterion == id).unwrap();
    assert_eq!(check("C1").method, CheckMethod::Program);
    assert!(check("C1").passed);
    assert_eq!(check("C1").evidence, "996/900-1100 words");
    assert_eq!(check("C2").method, CheckMethod::Program);
    assert_eq!(check("C2").evidence, "4/4- headings");
    // Le rapport lisible dit qui a vérifié.
    let readable = host.file(".atelier/rapport.md").unwrap();
    assert!(readable.contains(
        "✓ Lorsqu'on compte les mots, le document doit comprendre entre 900 et 1 100 mots. — \
         tenu (contrôlé par programme)"
    ));
}

/// Point 2 : entre ±10 % et ±35 % de la longueur (ou des bornes d'un
/// critère de mots), la correction ciblée demande d'allonger, mots mesurés et
/// visés dits ; pas tout à fait atteinte, la version est livrée, l'écart
/// « à revoir » au rapport.
#[tokio::test]
async fn a_direct_version_too_short_is_asked_to_lengthen_then_delivered() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        let spec = spec_with(json!([
            "Le document compte entre 900 et 1 100 mots.",
            "Le guide dit l'exposition.",
            "Le guide donne un calendrier.",
            "Le guide donne les récoltes."
        ]));
        if let Some(reply) = spec(req) {
            return Some(reply);
        }
        match req.role.as_str() {
            // −20 %, puis −14 % après la correction.
            "direct" => Some(Reply::Text(format!("# Guide du balcon\n\n{}", filler(798)))),
            "directFix" => Some(Reply::Text(format!("# Guide du balcon\n\n{}", filler(858)))),
            _ => None,
        }
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let fix = model.of("directFix");
    assert_eq!(fix.len(), 1);
    assert!(fix[0].user.contains(
        "length: the text has 798 words; it must have about 1000 words: lengthen it by about 202 \
         words, to about 1000 words"
    ));
    assert!(fix[0].user.contains(
        "criterion C1 (counted by program): the text has 798 words; it must have between 900 and \
         1100 words: lengthen it by about 202 words"
    ));
    assert!(model.of("plan").is_empty());
    let report = report(&host);
    assert_eq!(report.route, Route::Direct);
    let failed: Vec<&str> = report
        .checks
        .iter()
        .filter(|c| c.blocking && !c.passed)
        .map(|c| c.criterion.as_str())
        .collect();
    assert_eq!(failed, vec!["length", "C1"]);
    assert_eq!(report.to_review, 2);
    let readable = host.file(".atelier/rapport.md").unwrap();
    let review = readable.split("## À revoir").nth(1).unwrap();
    assert!(review.contains("- length — 858/1000 (contrôlé par programme)"));
    assert!(review.contains("858/900-1100 words"));
}

/// Point 2 : trop longue de 15 %, la correction demande de raccourcir.
#[tokio::test]
async fn a_direct_version_too_long_is_asked_to_shorten() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        (req.role == "direct")
            .then(|| Reply::Text(format!("# Guide du balcon\n\n{}", filler(1_152))))
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let fix = model.of("directFix");
    assert_eq!(fix.len(), 1);
    assert!(fix[0].user.contains(
        "length: the text has 1152 words; it must have about 1000 words: shorten it by about 152 \
         words"
    ));
    let report = report(&host);
    assert_eq!(report.route, Route::Direct);
    assert!(report.checks.iter().all(|c| c.passed || !c.blocking));
}

/// Point 2 : au-delà de ±35 % (ici −40 %), même après la correction, la
/// version directe ne vaut pas d'être gardée : plan et parties.
#[tokio::test]
async fn a_direct_version_far_too_short_goes_to_the_plan() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        matches!(req.role.as_str(), "direct" | "directFix")
            .then(|| Reply::Text(format!("# Guide du balcon\n\n{}", filler(600))))
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(model.of("directFix").len(), 1);
    assert_eq!(model.of("plan").len(), 1);
    assert_eq!(report(&host).route, Route::Parts);
}

/// Point 3 : mêmes règles pour le document assemblé : longueur à ±10 %
/// (des parties à −15 %, chacune dans sa tolérance, font un livrable trop
/// court, dit au rapport), critères mesurables comptés par programme.
#[tokio::test]
async fn the_assembled_document_is_measured_by_the_program() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        if let Some(reply) = bench_spec(req).or_else(|| direct_fails(req)) {
            return Some(reply);
        }
        (req.role == "section").then(|| {
            let asked = req
                .user
                .rfind("(about ")
                .and_then(|at| req.user[at + 7..].split(' ').next()?.parse::<u32>().ok())
                .unwrap_or(300);
            let title = req.section().unwrap_or_default();
            let body = filler(asked * 85 / 100);
            Reply::Text(if title == "Introduction" {
                body
            } else {
                format!("## {title}\n\n{body}")
            })
        })
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    let report = report(&host);
    assert_eq!(report.route, Route::Parts);
    // Chaque partie tient sa longueur (±25 %).
    assert!(model.of("fix").is_empty());
    let check = |id: &str| report.checks.iter().find(|c| c.criterion == id).unwrap();
    assert!(!check("length").passed, "{:?}", check("length"));
    assert!(!check("C1").passed);
    assert_eq!(check("C1").method, CheckMethod::Program);
    assert!(check("C1").evidence.ends_with("/900-1100 words"));
    assert!(check("C2").passed);
    assert_eq!(check("C2").evidence, "4/4- headings");
    let whole = model
        .of("formJudge")
        .into_iter()
        .find(|r| r.user.contains("\nASSEMBLED DOCUMENT TO ASSESS"))
        .unwrap();
    assert!(!whole.user.contains("Criterion C1 (") && !whole.user.contains("Criterion C2 ("));
    assert!(!whole.user.contains("LENGTH:"));
}

/// Point 1, parties : un critère mesurable de portée « section » attribué à
/// une partie (un tableau) est compté sur elle par le programme, jamais posé
/// à son juge.
#[tokio::test]
async fn a_measurable_criterion_of_a_part_is_counted_on_it() {
    let host = FakeHost::new(Autonomy::Autonomous);
    let model = FakeModel::new(&host.trace, |req| {
        let spec = spec_with(json!([
            "Le guide contient un tableau.",
            "Le guide dit l'exposition.",
            "Le guide donne un calendrier.",
            "Le guide donne les récoltes."
        ]));
        if let Some(reply) = spec(req).or_else(|| direct_fails(req)) {
            return Some(reply);
        }
        (req.role == "section" && req.about("Les cultures")).then(|| {
            Reply::Text(format!(
                "## Les cultures\n\n{}\n\n| Légume | Mois |\n|---|---|\n| Radis | Mars |",
                filler(390)
            ))
        })
    });
    assert!(matches!(run(&host, &model).await, RunEnd::Delivered { .. }));
    assert_eq!(report(&host).route, Route::Parts);
    let record = host.get(StateKey::Brick(2)).unwrap();
    let checks: Vec<super::types::Check> =
        serde_json::from_value(record["checks"].clone()).unwrap();
    let c1 = checks.iter().find(|c| c.criterion == "C1").unwrap();
    assert_eq!(c1.method, CheckMethod::Program);
    assert!(c1.passed);
    assert_eq!(c1.evidence, "1/1- tables");
    for judge in model.of("formJudge") {
        assert!(!judge.user.contains("un tableau"), "{}", judge.user);
    }
}
