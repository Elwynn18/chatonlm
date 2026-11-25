//! Registre GLOBAL des décisions (conception § 0 bis 1) : les listes
//! retenues, décisions et termes de TOUTES les parties déjà faites passent à
//! chaque partie suivante, pas seulement par ses dépendances (lot 0 : la
//! partie « Récoltes » n'avait pas reçu la liste des cultures). Une liste est
//! FIXÉE par la partie qui la crée : les suivantes n'y ajoutent que des
//! éléments qu'aucun contrôle n'a mis en doute (lot 0 : poireaux et fraises
//! de « Récoltes » ne doivent pas devenir des cultures retenues). Une décision
//! par sujet (la première) ; une décision contraire est signalée dès l'ajout.
//! Condensé par programme quand il pèse trop (décisions du sujet de la partie
//! d'abord, puis les plus anciennes). Recherche des contradictions (même
//! sujet, valeurs différentes) pour le juge.

use super::text::{folded, item_matches, significant, tokens};
use super::types::Card;

/// Niveau de condensation le plus fort.
pub const MAX_LEVEL: u8 = 3;

/// Listes montrées au niveau le plus fort, au plus (les premières créées).
pub const MAX_LISTS: usize = 10;

/// Décision qui contredit le registre : même sujet, autre valeur que celle
/// gardée (la première).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub subject: String,
    /// Décision gardée, telle qu'écrite.
    pub kept: String,
    /// Décision écartée, telle qu'écrite.
    pub other: String,
}

/// Registre des parties faites, dans l'ordre où elles l'ont été.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registry {
    /// Listes retenues, fusionnées par nom.
    pub lists: Vec<(String, Vec<String>)>,
    /// Décisions, sans doublon.
    pub decisions: Vec<String>,
    /// Termes introduits, sans doublon.
    pub terms: Vec<String>,
}

/// Deux noms de listes désignent la même : mêmes mots porteurs de sens.
fn same_name(a: &str, b: &str) -> bool {
    let a = significant(a);
    !a.is_empty() && a == significant(b)
}

/// Sujet d'une décision « sujet : valeur » (`None` sans « : »).
fn subject_of(decision: &str) -> Option<(&str, &str)> {
    decision
        .split_once(':')
        .map(|(subject, value)| (subject.trim(), value.trim()))
        .filter(|(subject, _)| !significant(subject).is_empty())
}

fn push_unique(into: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    let key = folded(value);
    if !into.iter().any(|v| folded(v) == key) {
        into.push(value.to_owned());
    }
}

impl Registry {
    /// Ajoute la carte d'une partie faite. `doubted` : ce que les contrôles
    /// de la partie ont mis en doute (éléments signalés hors registre,
    /// affirmations jugées fausses) : jamais ajouté à une liste déjà fixée.
    /// Rend les décisions qui contredisent le registre (écartées).
    pub fn add(&mut self, card: &Card, doubted: &[String]) -> Vec<Conflict> {
        let doubtful = |item: &str| {
            doubted
                .iter()
                .any(|d| item_matches(item, d) || item_matches(d, item))
        };
        for (name, items) in &card.lists {
            match self.lists.iter_mut().find(|(n, _)| same_name(n, name)) {
                Some((_, known)) => {
                    for item in items {
                        if !doubtful(item)
                            && !known
                                .iter()
                                .any(|k| item_matches(item, k) || item_matches(k, item))
                        {
                            push_unique(known, item);
                        }
                    }
                }
                None => {
                    let mut kept = Vec::new();
                    for item in items {
                        push_unique(&mut kept, item);
                    }
                    if !kept.is_empty() {
                        self.lists.push((name.trim().to_owned(), kept));
                    }
                }
            }
        }
        let mut conflicts = Vec::new();
        for decision in &card.decisions {
            // Un sujet, une décision : la première.
            let known = subject_of(decision).and_then(|(subject, value)| {
                self.decisions.iter().find_map(|d| {
                    let (s, v) = subject_of(d)?;
                    same_name(s, subject).then(|| (d.clone(), tokens(v) == tokens(value)))
                })
            });
            match known {
                Some((_, true)) => {}
                Some((kept, false)) => conflicts.push(Conflict {
                    subject: subject_of(decision)
                        .map(|(s, _)| s.to_owned())
                        .unwrap_or_default(),
                    kept,
                    other: decision.trim().to_owned(),
                }),
                None => push_unique(&mut self.decisions, decision),
            }
        }
        for term in &card.terms {
            push_unique(&mut self.terms, term);
        }
        conflicts
    }

    pub fn is_empty(&self) -> bool {
        self.lists.is_empty() && self.decisions.is_empty() && self.terms.is_empty()
    }

    /// Texte du registre pour une consigne (anglais), condensé au niveau
    /// `level` (0 : entier ; 1 : décisions coupées à 160 caractères, 30
    /// termes ; 2 : 20 décisions, 15 termes ; 3 : [`MAX_LISTS`] listes de 30
    /// éléments au plus, 10 décisions, pas de termes). Décisions gardées :
    /// celles dont le sujet touche `focus` (titre et but de la partie)
    /// d'abord, puis les plus anciennes (les premières prises, que la suite
    /// suit), montrées dans leur ordre. Les listes restent entières jusqu'au
    /// niveau 2 : ce sont elles qui gardent le livrable cohérent.
    // i18n-ignore : consigne au modèle.
    pub fn render(&self, level: u8, focus: &str) -> String {
        if self.is_empty() {
            return "(none yet)".into();
        }
        let mut out = String::new();
        if !self.lists.is_empty() {
            out.push_str(
                "Lists retained for the whole document. Use them as they are: do not cite \
                 other items of the same kind.\n",
            );
            let lists = if level >= 3 { MAX_LISTS } else { usize::MAX };
            for (name, items) in self.lists.iter().take(lists) {
                let shown: Vec<&str> = if level >= 3 {
                    items.iter().take(30).map(String::as_str).collect()
                } else {
                    items.iter().map(String::as_str).collect()
                };
                out.push_str(&format!("- {name}: {}\n", shown.join("; ")));
            }
        }
        let (keep, cut) = match level {
            0 => (usize::MAX, usize::MAX),
            1 => (usize::MAX, 160),
            2 => (20, 160),
            _ => (10, 120),
        };
        if !self.decisions.is_empty() {
            out.push_str("Decisions already taken:\n");
            let focus = significant(focus);
            let touches = |decision: &str| {
                let subject = subject_of(decision).map_or(decision, |(s, _)| s);
                !significant(subject).is_disjoint(&focus)
            };
            let mut kept: Vec<usize> = (0..self.decisions.len())
                .filter(|i| touches(&self.decisions[*i]))
                .take(keep)
                .collect();
            for i in 0..self.decisions.len() {
                if kept.len() >= keep {
                    break;
                }
                if !kept.contains(&i) {
                    kept.push(i);
                }
            }
            kept.sort_unstable();
            for i in kept {
                let shown = super::text::clip(&self.decisions[i], cut);
                out.push_str(&format!("- {shown}\n"));
            }
        }
        let terms = match level {
            0 => usize::MAX,
            1 => 30,
            2 => 15,
            _ => 0,
        };
        if terms > 0 && !self.terms.is_empty() {
            let shown: Vec<&str> = self.terms.iter().take(terms).map(String::as_str).collect();
            out.push_str(&format!("Terms in use: {}\n", shown.join("; ")));
        }
        out.trim_end().to_owned()
    }
}

/// Contradiction possible entre deux parties, à trancher par le juge.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub first_part: u32,
    pub second_part: u32,
    pub subject: String,
    pub first: String,
    pub second: String,
}

/// Candidats gardés au plus (les plus sûrs d'abord : chiffres différents).
pub const MAX_CANDIDATES: usize = 10;

fn digits(text: &str) -> Vec<String> {
    tokens(text)
        .into_iter()
        .filter(|w| w.chars().any(|c| c.is_ascii_digit()))
        .collect()
}

/// Même sujet, valeurs différentes, entre deux parties : décisions « sujet :
/// valeur », et listes de même nom aux éléments différents.
pub fn contradiction_candidates(cards: &[(u32, Card)]) -> Vec<Candidate> {
    let mut found: Vec<(bool, Candidate)> = Vec::new();
    for (i, (a, card_a)) in cards.iter().enumerate() {
        for (b, card_b) in cards.iter().skip(i + 1) {
            for da in &card_a.decisions {
                let Some((sa, va)) = da.split_once(':') else {
                    continue;
                };
                for db in &card_b.decisions {
                    let Some((sb, vb)) = db.split_once(':') else {
                        continue;
                    };
                    if !same_name(sa, sb) || tokens(va) == tokens(vb) {
                        continue;
                    }
                    let numbers = digits(va) != digits(vb);
                    found.push((
                        numbers,
                        Candidate {
                            first_part: *a,
                            second_part: *b,
                            subject: sa.trim().to_owned(),
                            first: va.trim().to_owned(),
                            second: vb.trim().to_owned(),
                        },
                    ));
                }
            }
            for (na, ia) in &card_a.lists {
                for (nb, ib) in &card_b.lists {
                    if !same_name(na, nb) {
                        continue;
                    }
                    let missing = |x: &[String], y: &[String]| {
                        x.iter()
                            .any(|e| !y.iter().any(|f| item_matches(e, f) || item_matches(f, e)))
                    };
                    if missing(ia, ib) || missing(ib, ia) {
                        found.push((
                            false,
                            Candidate {
                                first_part: *a,
                                second_part: *b,
                                subject: na.trim().to_owned(),
                                first: ia.join("; "),
                                second: ib.join("; "),
                            },
                        ));
                    }
                }
            }
        }
    }
    // Chiffres différents d'abord, ordre trouvé sinon.
    found.sort_by_key(|(numbers, _)| !*numbers);
    found
        .into_iter()
        .map(|(_, c)| c)
        .take(MAX_CANDIDATES)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(decisions: &[&str], lists: &[(&str, &[&str])]) -> Card {
        Card {
            summary: String::new(),
            terms: vec!["paillage".into()],
            decisions: decisions.iter().map(|d| (*d).to_owned()).collect(),
            lists: lists
                .iter()
                .map(|(n, items)| {
                    (
                        (*n).to_owned(),
                        items.iter().map(|i| (*i).to_owned()).collect(),
                    )
                })
                .collect(),
            exposes: Vec::new(),
        }
    }

    #[test]
    fn lists_of_every_part_are_kept_and_merged() {
        let mut registry = Registry::default();
        registry.add(
            &card(
                &["Registre : tutoiement"],
                &[("Cultures", &["tomate", "radis"])],
            ),
            &[],
        );
        registry.add(
            &card(
                &["Registre : tutoiement"],
                &[("cultures", &["Tomates", "laitue"])],
            ),
            &[],
        );
        assert_eq!(
            registry.lists,
            vec![(
                "Cultures".to_owned(),
                vec!["tomate".to_owned(), "radis".into(), "laitue".into()]
            )]
        );
        assert_eq!(registry.decisions.len(), 1);
        assert_eq!(registry.terms, vec!["paillage".to_owned()]);
        let text = registry.render(0, "");
        assert!(text.contains("- Cultures: tomate; radis; laitue"));
        assert!(text.contains("Terms in use: paillage"));
    }

    /// Noms distincts dès leurs 6 premières lettres (comparaison des
    /// sujets) : « aatheme », « abtheme »…
    fn name(i: usize) -> String {
        let letters: String = [i / 26, i % 26]
            .iter()
            .map(|n| char::from(b'a' + *n as u8))
            .collect();
        format!("{letters}theme")
    }

    #[test]
    fn condensing_keeps_lists_and_the_first_decisions() {
        let mut registry = Registry::default();
        let decisions: Vec<String> = (0..40)
            .map(|i| format!("{} : {}", name(i), "x".repeat(300)))
            .collect();
        let refs: Vec<&str> = decisions.iter().map(String::as_str).collect();
        let items: Vec<String> = (0..50).map(|i| format!("élément{i}")).collect();
        let item_refs: Vec<&str> = items.iter().map(String::as_str).collect();
        let lists: Vec<(String, Vec<&str>)> = (0..15)
            .map(|i| (format!("{}liste", name(i)), item_refs.clone()))
            .collect();
        let lists: Vec<(&str, &[&str])> = lists
            .iter()
            .map(|(n, items)| (n.as_str(), items.as_slice()))
            .collect();
        registry.add(&card(&refs, &lists), &[]);
        let full = registry.render(0, "");
        let two = registry.render(2, "");
        let three = registry.render(3, "");
        assert!(two.len() < full.len());
        assert!(two.contains("élément49"), "listes entières au niveau 2");
        assert!(two.contains(&format!("{}liste:", name(14))));
        // Relecture du lot 2 : les PREMIÈRES décisions restent (la suite du
        // document les suit), les dernières partent.
        assert!(two.contains(&format!("- {} :", name(0))));
        assert!(two.contains(&format!("- {} :", name(19))));
        assert!(!two.contains(&format!("- {} :", name(20))));
        assert!(!three.contains("élément49"));
        assert!(three.len() < two.len());
        // Nombre de listes borné au dernier niveau.
        assert_eq!(three.matches("liste: ").count(), MAX_LISTS);
        // Le sujet de la partie passe avant les plus anciennes.
        let focused = registry.render(3, &format!("Partie sur {}", name(33)));
        assert!(focused.contains(&format!("- {} :", name(33))));
        assert!(focused.contains(&format!("- {} :", name(0))));
        assert!(!focused.contains(&format!("- {} :", name(9))), "{focused}");
    }

    /// Relecture du lot 2 : une décision par sujet (la première), une
    /// contraire signalée dès l'ajout.
    #[test]
    fn a_decision_against_the_registry_is_reported_and_left_out() {
        let mut registry = Registry::default();
        let first = registry.add(
            &card(
                &["Dose de fumier : 3 kg par m²", "Registre : tutoiement"],
                &[],
            ),
            &[],
        );
        assert!(first.is_empty());
        let conflicts = registry.add(
            &card(
                &["dose de fumier: 200 g par m²", "Registre : tutoiement"],
                &[],
            ),
            &[],
        );
        assert_eq!(
            conflicts,
            vec![Conflict {
                subject: "dose de fumier".into(),
                kept: "Dose de fumier : 3 kg par m²".into(),
                other: "dose de fumier: 200 g par m²".into(),
            }]
        );
        assert_eq!(registry.decisions.len(), 2);
        assert!(!registry.render(0, "").contains("200 g"));
    }

    /// Relecture du lot 2 : une liste est fixée par la partie qui la crée ;
    /// ce que les contrôles d'une partie suivante ont mis en doute n'y entre
    /// pas.
    #[test]
    fn a_retained_list_does_not_absorb_doubted_items() {
        let mut registry = Registry::default();
        registry.add(
            &card(&[], &[("cultures", &["tomate", "radis", "laitue"])]),
            &[],
        );
        registry.add(
            &card(
                &[],
                &[("Cultures", &["Tomates", "Poireaux", "Fraises", "Mâche"])],
            ),
            &[
                "Poireaux".into(),
                "en juin, les fraises donnent leurs fruits".into(),
            ],
        );
        assert_eq!(
            registry.lists[0].1,
            vec![
                "tomate".to_owned(),
                "radis".into(),
                "laitue".into(),
                "Mâche".into()
            ]
        );
    }

    #[test]
    fn contradictions_are_same_subject_with_other_values() {
        let cards = vec![
            (
                2,
                card(&["Dose de fumier : 3 kg par m²", "Registre : tu"], &[]),
            ),
            (
                5,
                card(&["dose de fumier: 200 g par m²", "Registre : tu"], &[]),
            ),
            (6, card(&[], &[("cultures", &["tomate", "radis"])])),
            (8, card(&[], &[("Cultures", &["tomate", "poireau"])])),
        ];
        let found = contradiction_candidates(&cards);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].first_part, found[0].second_part), (2, 5));
        assert_eq!(found[0].subject, "Dose de fumier");
        assert_eq!((found[1].first_part, found[1].second_part), (6, 8));
    }
}
