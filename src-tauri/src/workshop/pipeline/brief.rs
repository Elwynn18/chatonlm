//! Consignes d'une partie (conception § 4.2, [lot 0]) : toujours dans cet
//! ordre (début et fin d'un contexte sont les mieux lus) : but global ; la
//! partie (objectif, forme, longueur, langue, registre) ; extrait de la
//! spécification ; registre GLOBAL (décisions et listes de toutes les parties
//! faites) ; cartes complètes des parties dont elle dépend (réduites à leur
//! résumé dès le premier niveau de condensation : le registre porte déjà
//! listes et décisions) ; voisines ; critères ; interdits ; rappel de
//! l'objectif.
//!
//! i18n-ignore (tout le fichier) : consignes au modèle, en anglais.

use super::prompts::{attachments_block, form_rule, language_name, register_rule};
use super::registry::Registry;
use super::types::{Attachment, BrickKind, BrickPlan, Card, Spec};

/// Une voisine : titre et but.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Neighbour {
    pub title: String,
    pub goal: String,
}

/// Ce qu'il faut pour écrire les consignes d'une partie.
pub struct Brief<'a> {
    pub spec: &'a Spec,
    pub doc_title: &'a str,
    pub brick: &'a BrickPlan,
    pub registry: &'a Registry,
    /// Niveau de condensation du registre (0 à `registry::MAX_LEVEL`).
    pub level: u8,
    /// Cartes en entrée : (titre, carte). Pour une introduction ou une
    /// conclusion, celles de toutes les sections.
    pub inputs: Vec<(&'a str, &'a Card)>,
    pub before: Option<Neighbour>,
    pub after: Option<Neighbour>,
    pub attachments: &'a [Attachment],
    /// Caractères des pièces jointes (0 : aucune).
    pub attachment_chars: usize,
}

/// Carte en entrée ; `summary_only` : son résumé seul (condensation).
fn card_text(title: &str, card: &Card, summary_only: bool) -> String {
    let mut out = format!("Section « {title} »: {}", card.summary.trim());
    if summary_only {
        return out;
    }
    if !card.decisions.is_empty() {
        out.push_str(&format!("\n  Decisions: {}", card.decisions.join("; ")));
    }
    for (name, items) in &card.lists {
        out.push_str(&format!("\n  List « {name} »: {}", items.join("; ")));
    }
    if !card.exposes.is_empty() {
        out.push_str(&format!("\n  Provides: {}", card.exposes.join("; ")));
    }
    if !card.terms.is_empty() {
        out.push_str(&format!("\n  Terms: {}", card.terms.join("; ")));
    }
    out
}

impl Brief<'_> {
    /// Texte des consignes.
    pub fn render(&self) -> String {
        let spec = self.spec;
        let brick = self.brick;
        let language = language_name(&spec.language);
        let mut out = format!("1. OVERALL GOAL\n{}\n\n", spec.goal.trim());

        out.push_str(&format!(
            "2. YOUR SECTION: « {} »\nGoal: {}\nForm: {}.\nLanguage: write in {language}. \
             Register: {}. Units: {}.\n\n",
            brick.title,
            brick.goal,
            form_rule(brick, self.doc_title),
            register_rule(spec.register),
            spec.units,
        ));

        out.push_str(&format!(
            "3. SHARED CONTRACT\nAudience: {}\n",
            spec.audience
        ));
        if !spec.glossary.is_empty() {
            out.push_str("Glossary (use these terms in this sense everywhere):\n");
            for (term, definition) in &spec.glossary {
                out.push_str(&format!("- {term}: {definition}\n"));
            }
        }
        if !spec.assumptions.is_empty() {
            out.push_str(&format!("Assumptions: {}\n", spec.assumptions.join("; ")));
        }
        if !spec.constraints.is_empty() {
            out.push_str("Constraints:\n");
            for (i, constraint) in spec.constraints.iter().enumerate() {
                out.push_str(&format!("  {}. {constraint}\n", i + 1));
            }
        }
        out.push('\n');

        out.push_str(&format!(
            "4. DECISIONS ALREADY TAKEN IN THE DOCUMENT (all sections written so far)\n{}\n\n",
            self.registry
                .render(self.level, &format!("{} {}", brick.title, brick.goal))
        ));

        out.push_str("5. INPUTS\n");
        if self.inputs.is_empty() {
            out.push_str("(none)\n");
        } else {
            if brick.kind != BrickKind::Text {
                out.push_str("Sections already written:\n");
            }
            for (title, card) in &self.inputs {
                out.push_str(&card_text(title, card, self.level > 0));
                out.push('\n');
            }
        }
        let docs = attachments_block(self.attachments, self.attachment_chars);
        if !docs.is_empty() {
            out.push_str(&docs);
        }
        out.push('\n');

        out.push_str("6. NEIGHBOURS\n");
        match &self.before {
            Some(n) => out.push_str(&format!("Before: « {} »: {}\n", n.title, n.goal)),
            None => out.push_str("Before: (start of the document)\n"),
        }
        match &self.after {
            Some(n) => out.push_str(&format!("After: « {} »: {}\n", n.title, n.goal)),
            None => out.push_str("After: (end of the document)\n"),
        }
        out.push('\n');

        out.push_str("7. CRITERIA THAT CONCERN YOU\n");
        let criteria: Vec<String> = spec
            .criteria
            .iter()
            .filter(|c| brick.criteria.contains(&c.id))
            .map(|c| format!("{}. {}", c.id, c.text))
            .collect();
        if criteria.is_empty() {
            out.push_str("(none of your own)\n");
        } else {
            out.push_str(&criteria.join("\n"));
            out.push('\n');
        }
        out.push('\n');

        out.push_str("8. DO NOT\n");
        for forbidden in &brick.forbidden {
            out.push_str(&format!("- {forbidden}\n"));
        }
        match brick.kind {
            BrickKind::Text => out.push_str(
                "- write a general introduction or conclusion of the document, or an \"in \
                 summary\" paragraph: other sections do that.\n",
            ),
            BrickKind::Intro | BrickKind::Conclusion => out.push_str(
                "- cover the content of the sections in their place, or bring in new facts.\n",
            ),
        }
        out.push('\n');

        out.push_str(&format!(
            "9. REMINDER\nNow write the section « {} » (about {} words) in {language}: {}",
            brick.title, brick.words, brick.goal
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workshop::pipeline::checks::tests::{good_plan, spec};

    #[test]
    fn the_brief_keeps_its_order_and_the_global_registry() {
        let spec = spec();
        let plan = good_plan();
        let mut registry = Registry::default();
        let crops = Card {
            summary: "Les cultures retenues.".into(),
            lists: vec![("cultures".into(), vec!["tomate".into(), "radis".into()])],
            decisions: vec!["Registre : tutoiement".into()],
            ..Card::default()
        };
        registry.add(&crops, &[]);
        let brief = Brief {
            spec: &spec,
            doc_title: "Guide",
            brick: &plan.bricks[2],
            registry: &registry,
            level: 0,
            inputs: Vec::new(),
            before: Some(Neighbour {
                title: "Les cultures".into(),
                goal: "Traiter".into(),
            }),
            after: None,
            attachments: &[],
            attachment_chars: 0,
        }
        .render();
        let order = [
            "1. OVERALL GOAL",
            "2. YOUR SECTION",
            "3. SHARED CONTRACT",
            "4. DECISIONS ALREADY TAKEN",
            "5. INPUTS",
            "6. NEIGHBOURS",
            "7. CRITERIA",
            "8. DO NOT",
            "9. REMINDER",
        ];
        let positions: Vec<usize> = order.iter().map(|h| brief.find(h).expect(h)).collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]));
        // La liste des cultures arrive sans dépendance directe.
        assert!(brief.contains("cultures: tomate; radis"));
        assert!(brief.contains("C2. QUAND"));
        assert!(!brief.contains("C1. QUAND"));
        assert!(brief.contains("write in French"));
        assert!(brief.contains("« tu »"));
    }
}
