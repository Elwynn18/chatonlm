//! Fin d'un projet : assemblage mécanique (ordre du plan, titres ; pas de
//! jointures, lot 0), vérification d'ensemble (critères un par un,
//! contradictions du registre, mots internes, longueur), titres empilés
//! repliés, relecture de la langue (`language_pass`), livraison du livrable
//! et du rapport (JSON `Report` + `.atelier/rapport.md` dans la langue de
//! sortie : « à revoir », affirmations douteuses et signalements listés à
//! part). Ce qui n'a pas été vérifié est dit tel quel.

use serde_json::Value;

use super::build::{BrickRecord, Progress, body_words, doc_title, reading_order};
use super::checks::{self, Expect, WHOLE_LENGTH_TOLERANCE};
use super::flow::{Direct, soft, whole_questions};
use super::measure;
use super::prompts::{self, ANSWER, THINK_PLAN};
use super::registry::{Candidate, contradiction_candidates};
use super::text::{body_of, first_title, slug, tidy_headings, words};
use super::types::{
    Activity, BrickKind, BrickReport, BrickState, Check, CheckMethod, Phase, Plan, Report, Route,
    Spec, StateKey, Strategy,
};
use super::{Run, Stop};
use crate::i18n::t;

/// Ce qui arrive à la fin.
pub(super) enum Output {
    Direct(Direct),
    Parts { plan: Plan, progress: Progress },
}

/// Dossier des fichiers de travail du projet.
pub const WORK_DIR: &str = ".atelier";

/// Livrable assemblé : titre, introduction, sections dans l'ordre du plan,
/// conclusion ; parties refusées ou vides omises.
pub(super) fn assemble(title: &str, plan: &Plan, records: &[BrickRecord]) -> String {
    let mut parts = vec![format!("# {title}")];
    for brick in reading_order(&plan.bricks) {
        let Some(record) = records.iter().find(|r| r.id == brick.id) else {
            continue;
        };
        if record.state == BrickState::Refused || body_of(&record.text).is_empty() {
            continue;
        }
        parts.push(record.text.trim().to_owned());
    }
    parts.join("\n\n")
}

/// Squelette du livrable (titres et cartes) pour un jugement d'un livrable
/// trop long.
// i18n-ignore : texte au modèle.
fn outline(title: &str, plan: &Plan, progress: &Progress) -> String {
    let mut out = format!("# {title}\n");
    for brick in reading_order(&plan.bricks) {
        out.push_str(&format!("\n## {}\n", brick.title));
        if let Some(card) = progress.cards.get(&brick.id) {
            out.push_str(&format!("Summary: {}\n", card.summary));
            if !card.decisions.is_empty() {
                out.push_str(&format!("Decisions: {}\n", card.decisions.join("; ")));
            }
            for (name, items) in &card.lists {
                out.push_str(&format!("List « {name} »: {}\n", items.join("; ")));
            }
        }
    }
    out
}

/// Contradictions confirmées par le juge → vérifications.
fn contradiction_checks(candidates: &[Candidate], value: Option<&Value>) -> Vec<Check> {
    let Some(verdicts) = value
        .and_then(|v| v.get("verdicts"))
        .and_then(Value::as_array)
    else {
        return vec![Check {
            criterion: "contradictions".into(),
            method: CheckMethod::FactJudge,
            passed: false,
            evidence: "unavailable".into(),
            where_: None,
            blocking: false,
        }];
    };
    let mut out = Vec::new();
    for (i, candidate) in candidates.iter().enumerate() {
        let verdict = verdicts
            .iter()
            .find(|v| v.get("pair").and_then(Value::as_u64) == Some(i as u64 + 1))
            .or_else(|| verdicts.get(i));
        let contradiction = verdict
            .and_then(|v| v.get("contradiction"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if contradiction {
            out.push(Check {
                criterion: "contradiction".into(),
                method: CheckMethod::FactJudge,
                passed: false,
                evidence: format!("{} / {}", candidate.first, candidate.second),
                where_: Some(candidate.subject.clone()),
                blocking: true,
            });
        }
    }
    if out.is_empty() {
        out.push(Check {
            criterion: "contradictions".into(),
            method: CheckMethod::FactJudge,
            passed: true,
            evidence: candidates.len().to_string(),
            where_: None,
            blocking: true,
        });
    }
    out
}

/// Libellés du rapport lisible, dans la langue de sortie. i18n-ignore :
/// contenu du livrable (langue de la tâche, pas celle de l'app : `t!` ne
/// convient pas) ; autres langues : anglais.
struct Labels {
    title: &'static str,
    result: &'static str,
    direct: &'static str,
    parts: &'static str,
    words: &'static str,
    criteria: &'static str,
    sections: &'static str,
    review: &'static str,
    unchecked: &'static str,
    outline: &'static str,
    program: &'static str,
    form: &'static str,
    fact: &'static str,
    ok: &'static str,
    no: &'static str,
    states: [&'static str; 8],
    nothing: &'static str,
    sources: &'static str,
    judge: &'static str,
    documents: &'static str,
    notes: &'static str,
    length_capped: &'static str,
    spec_kept: &'static str,
    doubtful: &'static str,
    doubtful_intro: &'static str,
    flags: &'static str,
    proofread: &'static str,
    proofread_read: &'static str,
    proofread_fixed: &'static str,
    proofread_rejected: &'static str,
}

fn labels(language: &str) -> Labels {
    match language {
        "fr" => Labels {
            title: "Rapport de vérification",
            result: "Résultat",
            direct: "Écrit d'un seul jet, puis vérifié.",
            parts: "Écrit partie par partie, selon un plan, puis assemblé et vérifié.",
            words: "Longueur",
            criteria: "Critères d'acceptation",
            sections: "Parties",
            review: "À revoir",
            unchecked: "Non vérifié",
            outline: "Les critères ont été jugés sur le squelette (titres et résumés) : le livrable était trop long pour un seul jugement.",
            program: "contrôlé par programme",
            form: "jugé par le modèle (forme)",
            fact: "jugé par le modèle (exactitude)",
            ok: "tenu",
            no: "non tenu",
            states: [
                "en attente",
                "en cours",
                "vérification",
                "correction",
                "faite",
                "à revoir",
                "refusée par le modèle",
                "redécoupée",
            ],
            nothing: "Rien.",
            sources: "Les faits ont été jugés par le modèle d'après ses connaissances, sans source vérifiée.",
            judge: "Un juge n'a pas pu répondre : ces points ne sont pas vérifiés.",
            documents: "Les documents joints étaient trop longs : seul leur début a été lu.",
            notes: "Remarques",
            length_capped: "La longueur demandée dépassait ce qu'un projet peut écrire : elle a été bornée.",
            spec_kept: "La spécification gardée a encore des défauts (trois essais).",
            doubtful: "Affirmations douteuses",
            doubtful_intro: "Simplifications ou règles qui ne valent pas partout : à vérifier selon ta situation si elles comptent.",
            flags: "Signalements",
            proofread: "Relecture de la langue",
            proofread_read: "passages relus",
            proofread_fixed: "corrigés",
            proofread_rejected: "corrections écartées par le garde-fou",
        },
        "es" => Labels {
            title: "Informe de verificación",
            result: "Resultado",
            direct: "Escrito de una sola vez y luego verificado.",
            parts: "Escrito parte por parte según un plan, luego ensamblado y verificado.",
            words: "Extensión",
            criteria: "Criterios de aceptación",
            sections: "Partes",
            review: "Por revisar",
            unchecked: "No verificado",
            outline: "Los criterios se juzgaron sobre el esqueleto (títulos y resúmenes): el documento era demasiado largo para un solo juicio.",
            program: "comprobado por programa",
            form: "juzgado por el modelo (forma)",
            fact: "juzgado por el modelo (exactitud)",
            ok: "cumplido",
            no: "no cumplido",
            states: [
                "pendiente",
                "en curso",
                "verificación",
                "corrección",
                "hecha",
                "por revisar",
                "rechazada por el modelo",
                "dividida",
            ],
            nothing: "Nada.",
            sources: "Los hechos fueron juzgados por el modelo según sus conocimientos, sin fuente verificada.",
            judge: "Un juez no pudo responder: esos puntos no están verificados.",
            documents: "Los documentos adjuntos eran demasiado largos: solo se leyó su comienzo.",
            notes: "Observaciones",
            length_capped: "La extensión pedida superaba lo que un proyecto puede escribir: se limitó.",
            spec_kept: "La especificación conservada aún tiene defectos (tres intentos).",
            doubtful: "Afirmaciones dudosas",
            doubtful_intro: "Simplificaciones o reglas que no valen en todas partes: conviene comprobarlas según el caso si importan.",
            flags: "Avisos",
            proofread: "Revisión de la lengua",
            proofread_read: "pasajes revisados",
            proofread_fixed: "corregidos",
            proofread_rejected: "correcciones descartadas por el control",
        },
        _ => Labels {
            title: "Verification report",
            result: "Result",
            direct: "Written in one go, then checked.",
            parts: "Written part by part from a plan, then assembled and checked.",
            words: "Length",
            criteria: "Acceptance criteria",
            sections: "Parts",
            review: "To review",
            unchecked: "Not checked",
            outline: "The criteria were judged on the outline (titles and summaries): the deliverable was too long for one judgement.",
            program: "checked by program",
            form: "judged by the model (form)",
            fact: "judged by the model (accuracy)",
            ok: "met",
            no: "not met",
            states: [
                "pending",
                "running",
                "checking",
                "fixing",
                "done",
                "to review",
                "refused by the model",
                "split",
            ],
            nothing: "Nothing.",
            sources: "Facts were judged by the model from its own knowledge, not against verified sources.",
            judge: "A judge could not answer: these points are not checked.",
            documents: "The attached documents were too long: only their beginning was read.",
            notes: "Notes",
            length_capped: "The requested length was more than a project can write: it was capped.",
            spec_kept: "The specification kept still has defects (three attempts).",
            doubtful: "Doubtful claims",
            doubtful_intro: "Simplifications or rules that do not hold everywhere: worth checking for your situation if they matter.",
            flags: "Flags",
            proofread: "Language review",
            proofread_read: "passages read",
            proofread_fixed: "corrected",
            proofread_rejected: "corrections set aside by the safeguard",
        },
    }
}

fn state_index(state: BrickState) -> usize {
    match state {
        BrickState::Pending => 0,
        BrickState::Running => 1,
        BrickState::Checking => 2,
        BrickState::Fixing => 3,
        BrickState::Done => 4,
        BrickState::ToReview => 5,
        BrickState::Refused => 6,
        BrickState::Split => 7,
    }
}

/// Rapport lisible (sections fixes), dans la langue de sortie.
pub(super) fn report_markdown(report: &Report, spec: &Spec) -> String {
    let l = labels(&spec.language);
    let method = |m: CheckMethod| match m {
        CheckMethod::Program => l.program,
        CheckMethod::FormJudge => l.form,
        CheckMethod::FactJudge => l.fact,
    };
    let mut out = format!("# {}\n\n## {}\n\n", l.title, l.result);
    out.push_str(match report.route {
        Route::Direct => l.direct,
        Route::Parts => l.parts,
    });
    out.push_str(&format!("\n\n{} : {}", l.words, report.words));
    if let Some(target) = report.target_words {
        out.push_str(&format!(" / {target}"));
    }
    if report.proofread_blocks > 0 {
        out.push_str(&format!(
            "\n\n{} : {} {}, {} {} ; {} : {}.",
            l.proofread,
            report.proofread_blocks,
            l.proofread_read,
            report.proofread_changed,
            l.proofread_fixed,
            l.proofread_rejected,
            report.proofread_rejected
        ));
    }
    out.push_str(&format!("\n\n## {}\n\n", l.criteria));
    if report.judged_on_outline {
        out.push_str(&format!("{}\n\n", l.outline));
    }
    // Les critères seulement : les affirmations jugées par le juge
    // d'exactitude ont leurs propres listes (à revoir, douteuses) plus bas,
    // et la longueur générale n'est pas répétée quand un critère de nombre
    // de mots l'a déjà mesurée.
    let words_measured = report.checks.iter().any(|c| {
        c.criterion != "length"
            && c.method == CheckMethod::Program
            && c.evidence.ends_with(" words")
    });
    for check in report.checks.iter().filter(|c| {
        c.method != CheckMethod::FactJudge && !(words_measured && c.criterion == "length")
    }) {
        let text = spec
            .criteria
            .iter()
            .find(|c| c.id == check.criterion)
            .map_or(check.criterion.as_str(), |c| c.text.as_str());
        out.push_str(&format!(
            "- {} {} — {} ({})\n",
            if check.passed { "✓" } else { "✗" },
            text,
            if check.passed { l.ok } else { l.no },
            method(check.method)
        ));
    }
    if !report.bricks.is_empty() {
        out.push_str(&format!("\n## {}\n\n", l.sections));
        for brick in &report.bricks {
            out.push_str(&format!(
                "- {} — {} ({})\n",
                brick.title,
                l.states[state_index(brick.state)],
                brick.words
            ));
        }
    }
    // À revoir (échecs qui comptent, restés), affirmations douteuses,
    // signalements (mots du moteur, éléments hors registre, remarque
    // recopiée…) : listés à part.
    let all: Vec<&Check> = report
        .checks
        .iter()
        .chain(report.bricks.iter().flat_map(|b| b.checks.iter()))
        .filter(|c| !c.passed && !checks::is_unavailable(c))
        .collect();
    let line = |check: &Check| {
        let place = check.where_.as_deref().unwrap_or(&check.criterion);
        format!("- {place} — {} ({})", check.evidence, method(check.method))
    };
    let review: Vec<String> = all
        .iter()
        .filter(|c| checks::is_to_review(c))
        .map(|c| line(c))
        .collect();
    let doubtful: Vec<String> = all
        .iter()
        .filter(|c| checks::is_doubtful(c))
        .map(|c| line(c))
        .collect();
    let flags: Vec<String> = all
        .iter()
        .filter(|c| !checks::is_to_review(c) && !checks::is_doubtful(c))
        .map(|c| line(c))
        .collect();
    out.push_str(&format!("\n## {}\n\n", l.review));
    if review.is_empty() {
        out.push_str(l.nothing);
        out.push('\n');
    } else {
        out.push_str(&review.join("\n"));
        out.push('\n');
    }
    if !doubtful.is_empty() {
        out.push_str(&format!(
            "\n## {}\n\n{}\n\n{}\n",
            l.doubtful,
            l.doubtful_intro,
            doubtful.join("\n")
        ));
    }
    if !flags.is_empty() {
        out.push_str(&format!("\n## {}\n\n{}\n", l.flags, flags.join("\n")));
    }
    out.push_str(&format!("\n## {}\n\n- {}\n", l.unchecked, l.sources));
    if report.not_checked.iter().any(|c| c == "documents") {
        out.push_str(&format!("- {}\n", l.documents));
    }
    if report
        .not_checked
        .iter()
        .any(|c| c != "sources" && c != "documents")
    {
        out.push_str(&format!("- {}\n", l.judge));
    }
    let notes: Vec<&str> = report
        .warnings
        .iter()
        .filter_map(|w| match w.as_str() {
            "lengthCapped" => Some(l.length_capped),
            "specBestOfThree" => Some(l.spec_kept),
            _ => None,
        })
        .collect();
    if !notes.is_empty() {
        out.push_str(&format!("\n## {}\n\n", l.notes));
        for note in notes {
            out.push_str(&format!("- {note}\n"));
        }
    }
    out
}

/// Codes de ce qui n'a pas été vérifié ; `documents` : pièces jointes
/// coupées dans les consignes.
fn not_checked(checks: &[&Check], documents_cut: bool) -> Vec<String> {
    let mut out = vec!["sources".to_owned()];
    if documents_cut {
        out.push("documents".to_owned());
    }
    for check in checks {
        if checks::is_unavailable(check) && !out.contains(&check.criterion) {
            out.push(check.criterion.clone());
        }
    }
    out
}

impl Run<'_> {
    /// Vérification d'ensemble d'un livrable assemblé.
    async fn check_whole(
        &self,
        spec: &Spec,
        title: &str,
        plan: &Plan,
        doc: &str,
        progress: &Progress,
    ) -> Result<(Vec<Check>, bool), Stop> {
        // Mêmes règles que la version directe : longueur à ±10 %, critères
        // mesurables comptés par programme (sur le livrable entier, même
        // quand les juges n'en voient que le squelette).
        let measures = measure::measured(spec);
        let expect = Expect {
            words: spec.length_words,
            tolerance: WHOLE_LENGTH_TOLERANCE,
            language: &spec.language,
            allowed: &self.allowed,
            lists: &[],
            measures: &measures,
        };
        let (mut found, _) = checks::program_checks(doc, &expect);
        let candidates = contradiction_candidates(&progress.written);
        if !candidates.is_empty() {
            // i18n-ignore : texte au modèle.
            let texts: Vec<String> = candidates
                .iter()
                .map(|c| {
                    let title_of = |id: u32| {
                        progress
                            .records
                            .iter()
                            .find(|r| r.id == id)
                            .map_or(String::new(), |r| r.title.clone())
                    };
                    format!(
                        "About « {} »: section « {} » says « {} »; section « {} » says « {} ».",
                        c.subject,
                        title_of(c.first_part),
                        c.first,
                        title_of(c.second_part),
                        c.second
                    )
                })
                .collect();
            let value = soft(
                self.json(
                    StateKey::Contradictions,
                    Activity::CheckingWhole,
                    prompts::contradictions_call(spec, &texts),
                )
                .await,
            )?;
            found.extend(contradiction_checks(&candidates, value.as_ref()));
        }
        // Le livrable entier tient-il dans un jugement ? Sinon, squelette.
        let room = usize::try_from(self.model.context())
            .unwrap_or(usize::MAX)
            .saturating_sub((THINK_PLAN + ANSWER) as usize + 3_000)
            .min(16_000);
        let doc_tokens = self.tokens(doc).await?;
        let on_outline = doc_tokens > room;
        let (label, text) = if on_outline {
            ("OUTLINE OF THE DOCUMENT", outline(title, plan, progress))
        } else {
            ("ASSEMBLED DOCUMENT", doc.to_owned())
        };
        let questions = whole_questions(spec);
        let asked: Vec<String> = questions.iter().map(|q| q.text.clone()).collect();
        let form = soft(
            self.json(
                StateKey::WholeForm,
                Activity::CheckingWhole,
                prompts::form_judge_call(spec, label, &text, &asked),
            )
            .await,
        )?;
        if form.is_none() {
            self.warn("judgeUnavailable").await;
        }
        found.extend(checks::form_checks(&questions, form.as_ref()));
        Ok((found, on_outline))
    }

    /// Assemble (route par parties), vérifie, livre, rend le fichier.
    pub(super) async fn finish(
        &self,
        strategy: Strategy,
        spec: &Spec,
        output: Output,
    ) -> Result<String, Stop> {
        let project = self.project();
        let (route, title, doc, checks, on_outline, bricks) = match output {
            Output::Direct(direct) => {
                let title = first_title(&direct.doc).unwrap_or_else(|| project.title.clone());
                (
                    Route::Direct,
                    title,
                    direct.doc,
                    direct.checks,
                    false,
                    Vec::new(),
                )
            }
            Output::Parts { plan, progress } => {
                self.phase(Phase::Assemble, true).await;
                self.emit(super::types::EventKind::Activity {
                    activity: Activity::Assembling,
                })
                .await;
                let title = doc_title(&plan, spec, &project.title);
                let doc = assemble(&title, &plan, &progress.records);
                self.phase(Phase::Assemble, false).await;
                self.phase(Phase::Verify, true).await;
                let (checks, on_outline) = self
                    .check_whole(spec, &title, &plan, &doc, &progress)
                    .await?;
                self.emit(super::types::EventKind::BrickChecks {
                    id: 0,
                    attempt: 0,
                    checks: checks.clone(),
                })
                .await;
                self.phase(Phase::Verify, false).await;
                let bricks: Vec<BrickReport> = progress
                    .records
                    .iter()
                    .map(|r| BrickReport {
                        id: r.id,
                        title: r.title.clone(),
                        state: r.state,
                        fixed: r.fixed,
                        words: if r.kind == BrickKind::Intro {
                            words(&r.text)
                        } else {
                            body_words(&r.text)
                        },
                        checks: r.checks.clone(),
                    })
                    .collect();
                (Route::Parts, title, doc, checks, on_outline, bricks)
            }
        };

        self.phase(Phase::Deliver, true).await;
        // Titres empilés repliés (« ## ### X »), puis relecture de la langue.
        let doc = tidy_headings(&doc);
        let (doc, proofread) = self.proofread(&doc).await?;
        self.emit(super::types::EventKind::Activity {
            activity: Activity::Delivering,
        })
        .await;
        // Nom de repli sur le disque : dans la langue de l'app.
        let file = format!("{}.md", slug(&title, &t!("workshop.disk.document")));
        let all: Vec<&Check> = checks
            .iter()
            .chain(bricks.iter().flat_map(|b| b.checks.iter()))
            .collect();
        let count = |keep: fn(&Check) -> bool| {
            u32::try_from(all.iter().filter(|c| keep(c)).count()).unwrap_or(u32::MAX)
        };
        let (doubtful, to_review) = (count(checks::is_doubtful), count(checks::is_to_review));
        let report = Report {
            strategy,
            route,
            file: file.clone(),
            words: words(doc.split_once('\n').map_or("", |(_, rest)| rest)),
            target_words: spec.length_words,
            not_checked: not_checked(&all, self.attachments_cut()),
            checks,
            judged_on_outline: on_outline,
            bricks,
            warnings: self.load(StateKey::Notes).await.unwrap_or_default(),
            doubtful,
            to_review,
            proofread_blocks: proofread.blocks,
            proofread_changed: proofread.changed,
            proofread_rejected: proofread.rejected,
        };
        // Écriture refusée : code stable ; la raison (code du socle) part au
        // journal.
        let failed = |code: String| {
            // i18n-ignore : journal.
            crate::app_log!("chatonlm: workshop: deliverable not written ({code})");
            Stop::Failed("deliverWriteFailed".into())
        };
        self.host
            .write_file(&file, doc.as_bytes())
            .await
            .map_err(failed)?;
        self.save(StateKey::Report, &report).await;
        let readable = report_markdown(&report, spec);
        self.host
            .write_file(&format!("{WORK_DIR}/rapport.md"), readable.as_bytes())
            .await
            .map_err(failed)?;
        self.save(StateKey::Delivered, &file).await;
        self.phase(Phase::Deliver, false).await;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workshop::pipeline::checks::tests::{good_plan, spec};

    fn record(id: u32, state: BrickState, text: &str) -> BrickRecord {
        BrickRecord {
            id,
            title: String::new(),
            kind: BrickKind::Text,
            state,
            text: text.into(),
            fixed: false,
            checks: Vec::new(),
        }
    }

    #[test]
    fn assembly_is_mechanical_in_plan_order() {
        let plan = good_plan();
        let records = vec![
            record(
                3,
                BrickState::Done,
                "## Le calendrier des récoltes\n\nTrois.",
            ),
            record(2, BrickState::Done, "## Les cultures\n\nDeux."),
            record(4, BrickState::Refused, "## Conclusion\n\nRefus."),
            record(1, BrickState::Done, "Bonjour."),
        ];
        let doc = assemble("Guide", &plan, &records);
        assert_eq!(
            doc,
            "# Guide\n\nBonjour.\n\n## Les cultures\n\nDeux.\n\n## Le calendrier des récoltes\n\nTrois."
        );
    }

    #[test]
    fn the_readable_report_says_what_was_not_checked() {
        let spec = spec();
        let report = Report {
            strategy: Strategy::LongForm,
            route: Route::Parts,
            file: "guide.md".into(),
            words: 950,
            target_words: Some(1_000),
            checks: vec![
                Check {
                    criterion: "C1".into(),
                    method: CheckMethod::FormJudge,
                    passed: true,
                    evidence: String::new(),
                    where_: None,
                    blocking: true,
                },
                checks::unavailable("factJudge"),
            ],
            judged_on_outline: true,
            bricks: Vec::new(),
            not_checked: vec!["sources".into(), "factJudge".into(), "documents".into()],
            warnings: vec!["lengthCapped".into()],
            doubtful: 0,
            to_review: 0,
            proofread_blocks: 0,
            proofread_changed: 0,
            proofread_rejected: 0,
        };
        let text = report_markdown(&report, &spec);
        assert!(text.starts_with("# Rapport de vérification"));
        assert!(text.contains("✓ QUAND on lit, LE LIVRABLE DOIT lister les cultures — tenu (jugé par le modèle (forme))"));
        assert!(text.contains("squelette"));
        assert!(text.contains("sans source vérifiée"));
        assert!(text.contains("Un juge n'a pas pu répondre"));
        assert!(text.contains("seul leur début a été lu"));
        assert!(text.contains("## Remarques\n\n- La longueur demandée"));
        assert!(!text.contains("unavailable"));
    }

    /// Banc réel du 25 nov. : les affirmations douteuses apparaissaient en
    /// « ✗ non tenu » parmi les critères, et la longueur deux fois.
    #[test]
    fn the_criteria_list_holds_neither_claims_nor_a_repeated_length() {
        let spec = spec();
        let check = |criterion: &str, method, passed, evidence: &str| Check {
            criterion: criterion.into(),
            method,
            passed,
            evidence: evidence.into(),
            where_: None,
            blocking: true,
        };
        let report = Report {
            strategy: Strategy::LongForm,
            route: Route::Direct,
            file: "guide.md".into(),
            words: 3_882,
            target_words: Some(3_500),
            checks: vec![
                check("length", CheckMethod::Program, false, "3882/3500"),
                check("C1", CheckMethod::Program, false, "3882/3300-3700 words"),
                check(
                    "Transplante en mai, à 10 cm d'intervalle.",
                    CheckMethod::FactJudge,
                    false,
                    "Règle de pouce.",
                ),
            ],
            judged_on_outline: false,
            bricks: Vec::new(),
            not_checked: Vec::new(),
            warnings: Vec::new(),
            doubtful: 1,
            to_review: 1,
            proofread_blocks: 0,
            proofread_changed: 0,
            proofread_rejected: 0,
        };
        let text = report_markdown(&report, &spec);
        let criteria = text
            .split("## Critères d'acceptation")
            .nth(1)
            .and_then(|rest| rest.split("\n## ").next())
            .unwrap();
        assert!(!criteria.contains("Transplante"), "{criteria}");
        assert!(!criteria.contains("- ✗ length"), "{criteria}");
        assert!(
            criteria.contains("- ✗ QUAND on lit, LE LIVRABLE DOIT lister les cultures"),
            "{criteria}"
        );
    }
}
