//! Passe chirurgicale (banc du 25 nov.) : quand il ne reste, après la
//! correction ciblée, que quelques affirmations fausses (au plus
//! [`MAX_SURGERY`]) et rien d'autre de fond, seules les phrases fautives
//! sont reprises : le modèle reçoit ces phrases (et leur paragraphe, pour le
//! sens) et rend, pour chacune, une phrase corrigée ou son retrait ; le
//! programme les remet à leur place, le reste du texte ne passe pas par le
//! modèle. Contrôle : les paragraphes hors des phrases visées restent
//! identiques, une modification hors de son cadre (ligne de tableau
//! déformée, phrase qui devient un paragraphe…) fait rejeter la passe
//! entière. Les phrases reprises sont jugées de nouveau (exactitude, elles
//! seules) ; ce qui reste faux est « à revoir ». Logique pure, sauf
//! [`Run::surgery`].

use serde_json::Value;

use super::checks::{self, MAX_SURGERY};
use super::flow::soft;
use super::prompts::{self, Faulty};
use super::text::{clip, is_heading, sentence_span, words};
use super::types::{Activity, Check, CheckMethod, Spec, StateKey};
use super::{Run, Stop};

/// Paragraphe montré autour d'une phrase, au plus (caractères).
const PARAGRAPH_CHARS: usize = 1_500;

/// Une phrase à reprendre : sa plage d'octets dans le texte, les
/// affirmations fausses qu'elle porte (citations du juge) et ses
/// indications.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub start: usize,
    pub end: usize,
    pub quotes: Vec<String>,
    pub hints: Vec<String>,
}

/// Phrases qui portent les affirmations fausses de `checks`, dans l'ordre
/// du texte ; deux affirmations de la même phrase n'en font qu'une. Une
/// citation introuvable (le juge a reformulé) n'est pas reprise : elle
/// reste « à revoir ».
pub(super) fn targets(text: &str, checks: &[Check]) -> Vec<Target> {
    let mut out: Vec<Target> = Vec::new();
    for check in checks.iter().filter(|c| checks::is_wrong_claim(c)) {
        let Some((start, end)) = sentence_span(text, &check.criterion) else {
            continue;
        };
        out.push(Target {
            start,
            end,
            quotes: vec![check.criterion.clone()],
            hints: vec![check.evidence.clone()],
        });
    }
    out.sort_by_key(|t| t.start);
    let mut merged: Vec<Target> = Vec::new();
    for target in out {
        match merged.last_mut() {
            Some(last) if target.start < last.end => {
                last.end = last.end.max(target.end);
                last.quotes.extend(target.quotes);
                last.hints.extend(target.hints);
            }
            _ => merged.push(target),
        }
    }
    merged
}

/// Paragraphe (entre deux lignes vides) qui contient `start..end`.
fn paragraph_around(text: &str, start: usize, end: usize) -> &str {
    let from = text[..start].rfind("\n\n").map_or(0, |i| i + 2);
    let to = text[end..].find("\n\n").map_or(text.len(), |i| end + i);
    clip(text[from..to].trim(), PARAGRAPH_CHARS)
}

/// Une modification lue : la phrase `index` (dans les cibles) remplacée, ou
/// retirée (`None`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Edit {
    pub index: usize,
    pub sentence: Option<String>,
}

/// Une ligne de tableau Markdown.
fn table_row(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

/// Modifications lues et contrôlées ; `None` (passe rejetée) si l'une sort
/// de son cadre : numéro inconnu ou répété, action inconnue, phrase vide ou
/// sur plusieurs lignes, titre, ligne de tableau aux cellules changées,
/// phrase plus de deux fois plus longue.
pub(super) fn read_edits(value: &Value, text: &str, targets: &[Target]) -> Option<Vec<Edit>> {
    let mut out: Vec<Edit> = Vec::new();
    for edit in value.get("edits")?.as_array()? {
        let n = usize::try_from(edit.get("n")?.as_u64()?).ok()?;
        let index = n.checked_sub(1).filter(|i| *i < targets.len())?;
        if out.iter().any(|e| e.index == index) {
            return None;
        }
        let target = &targets[index];
        let old = &text[target.start..target.end];
        let sentence = match edit.get("action")?.as_str()? {
            "remove" => None,
            "fix" => {
                let new = edit.get("sentence")?.as_str()?.trim().to_owned();
                let bars = |s: &str| s.matches('|').count();
                let shaped = !new.is_empty()
                    && !new.contains('\n')
                    && !is_heading(&new)
                    && table_row(&new) == table_row(old)
                    && (!table_row(old) || bars(&new) == bars(old))
                    && words(&new) <= 2 * words(old) + 20;
                if !shaped {
                    return None;
                }
                Some(new)
            }
            _ => return None,
        };
        out.push(Edit { index, sentence });
    }
    Some(out)
}

/// Retire la ligne qui contient `at` si elle n'a plus rien (ou plus qu'une
/// marque de liste).
fn drop_empty_line(text: &mut String, at: usize) {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let line = text[start..end].trim();
    let empty = line.is_empty()
        || matches!(line, "-" | "*" | "+")
        || (line.ends_with(['.', ')'])
            && line[..line.len() - 1].chars().all(|c| c.is_ascii_digit()));
    if !empty {
        return;
    }
    if end < text.len() {
        text.replace_range(start..=end, "");
    } else {
        text.replace_range(start.saturating_sub(1)..end, "");
    }
}

/// Paragraphes (entre lignes vides) avec leur plage d'octets.
fn paragraphs(text: &str) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let mut at = 0;
    for piece in text.split("\n\n") {
        let end = at + piece.len();
        if !piece.trim().is_empty() {
            out.push((at, end, piece.trim()));
        }
        at = end + 2;
    }
    out
}

/// Les paragraphes de `old` qui ne touchent aucune phrase visée se
/// retrouvent tels quels, dans le même ordre, dans `new`.
pub(super) fn untouched_outside(old: &str, new: &str, targets: &[Target]) -> bool {
    let fresh: Vec<&str> = paragraphs(new).into_iter().map(|(_, _, p)| p).collect();
    let mut next = 0;
    for (start, end, paragraph) in paragraphs(old) {
        if targets.iter().any(|t| t.start < end && start < t.end) {
            continue;
        }
        match fresh[next..].iter().position(|p| *p == paragraph) {
            Some(found) => next += found + 1,
            None => return false,
        }
    }
    true
}

/// Texte avec les modifications à leur place ; `None` si un paragraphe hors
/// des phrases visées a changé ([`untouched_outside`]).
pub(super) fn apply(text: &str, targets: &[Target], edits: &[Edit]) -> Option<String> {
    let mut ordered: Vec<&Edit> = edits.iter().collect();
    ordered.sort_by_key(|e| std::cmp::Reverse(targets[e.index].start));
    let mut out = text.to_owned();
    for edit in ordered {
        let target = &targets[edit.index];
        match &edit.sentence {
            Some(sentence) => out.replace_range(target.start..target.end, sentence),
            None => {
                // La phrase et une espace voisine.
                let mut end = target.end;
                let mut start = target.start;
                if out[end..].starts_with(' ') {
                    end += 1;
                } else if out[..start].ends_with(' ') {
                    start -= 1;
                }
                out.replace_range(start..end, "");
                let at = start.min(out.len());
                drop_empty_line(&mut out, at);
            }
        }
    }
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    untouched_outside(text, &out, targets).then_some(out)
}

impl Run<'_> {
    /// Passe chirurgicale sur la version `text` de la partie `id` (0 :
    /// version directe) : rend le texte et ses vérifications (celles des
    /// phrases reprises remplacées par leur nouveau jugement). Hors de son
    /// cas (aucune ou plus de [`MAX_SURGERY`] affirmations fausses), phrases
    /// introuvables, réponse illisible ou rejetée : texte et vérifications
    /// inchangés (avertissement).
    pub(super) async fn surgery(
        &self,
        id: u32,
        spec: &Spec,
        text: &str,
        found: Vec<Check>,
    ) -> Result<(String, Vec<Check>), Stop> {
        let wrong = checks::wrong_claims(&found);
        if wrong == 0 || wrong > MAX_SURGERY {
            return Ok((text.to_owned(), found));
        }
        let targets = targets(text, &found);
        if targets.is_empty() {
            self.warn("surgeryNotFound").await;
            return Ok((text.to_owned(), found));
        }
        let faulty: Vec<Faulty<'_>> = targets
            .iter()
            .map(|t| Faulty {
                sentence: &text[t.start..t.end],
                hints: t.hints.iter().map(String::as_str).collect(),
                paragraph: paragraph_around(text, t.start, t.end),
            })
            .collect();
        let value = soft(
            self.json(
                StateKey::Surgery(id),
                Activity::FixingSentences { id },
                prompts::surgery_call(spec, &faulty),
            )
            .await,
        )?;
        let applied = value
            .as_ref()
            .and_then(|v| read_edits(v, text, &targets))
            .and_then(|edits| Some((apply(text, &targets, &edits)?, edits)));
        let Some((fixed, edits)) = applied else {
            self.warn("surgeryRejected").await;
            return Ok((text.to_owned(), found));
        };
        let touched: Vec<&String> = edits
            .iter()
            .flat_map(|e| targets[e.index].quotes.iter())
            .collect();
        let mut out: Vec<Check> = found
            .into_iter()
            .filter(|c| !(checks::is_wrong_claim(c) && touched.contains(&&c.criterion)))
            .collect();
        for edit in edits.iter().filter(|e| e.sentence.is_none()) {
            for quote in &targets[edit.index].quotes {
                out.push(Check {
                    criterion: quote.clone(),
                    method: CheckMethod::FactJudge,
                    passed: true,
                    evidence: "removed".into(),
                    where_: None,
                    blocking: false,
                });
            }
        }
        let sentences: Vec<&str> = edits.iter().filter_map(|e| e.sentence.as_deref()).collect();
        if !sentences.is_empty() {
            // Re-jugement des seules phrases reprises.
            let value = soft(
                self.json(
                    StateKey::FactJudge { id, version: 2 },
                    Activity::FixingSentences { id },
                    prompts::fact_judge_call(
                        spec,
                        &sentences.join("\n"),
                        &[],
                        &self.project().attachments,
                        self.attachment_room(),
                    ),
                )
                .await,
            )?;
            if value.is_none() {
                self.warn("judgeUnavailable").await;
            }
            out.extend(checks::fact_checks(value.as_ref()));
        }
        Ok((fixed, out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wrong(quote: &str, hint: &str) -> Check {
        Check {
            criterion: quote.into(),
            method: CheckMethod::FactJudge,
            passed: false,
            evidence: hint.into(),
            where_: Some(quote.into()),
            blocking: true,
        }
    }

    const DOC: &str = "# Guide\n\n## Récoltes\n\nTu cueilles tôt. Une tomate trop mûre peut \
        attirer les pucerons. Tu la ranges au frais.\n\n- Radis : avril.\n- Poireau : tige de 30 cm \
        de diamètre.\n\n| Légume | Écart |\n|---|---|\n| Tomate | 30 cm |\n\nFin du guide.";

    #[test]
    fn sentences_are_found_and_merged() {
        let checks = vec![
            wrong(
                "Une tomate trop mûre peut attirer les pucerons.",
                "pas les pucerons",
            ),
            wrong("attirer les pucerons", "même phrase"),
            wrong("Tomate | 30 cm", "45 à 60 cm"),
            wrong("une phrase absente", "x"),
        ];
        let found = targets(DOC, &checks);
        assert_eq!(found.len(), 2);
        assert_eq!(
            &DOC[found[0].start..found[0].end],
            "Une tomate trop mûre peut attirer les pucerons."
        );
        assert_eq!(found[0].quotes.len(), 2);
        assert_eq!(&DOC[found[1].start..found[1].end], "| Tomate | 30 cm |");
        // Élément de liste : sans sa marque.
        let leek = targets(DOC, &[wrong("tige de 30 cm de diamètre", "hauteur")]);
        assert_eq!(
            &DOC[leek[0].start..leek[0].end],
            "Poireau : tige de 30 cm de diamètre."
        );
    }

    #[test]
    fn only_the_faulty_sentences_change() {
        let found = targets(
            DOC,
            &[
                wrong("Une tomate trop mûre peut attirer les pucerons.", "h"),
                wrong("Poireau : tige de 30 cm de diamètre.", "h"),
                wrong("| Tomate | 30 cm |", "h"),
            ],
        );
        let value = json!({"edits": [
            {"n": 1, "action": "remove", "sentence": ""},
            {"n": 2, "action": "fix", "sentence": "Poireau : fût de 2 à 3 cm de diamètre."},
            {"n": 3, "action": "fix", "sentence": "| Tomate | 50 cm |"},
        ]});
        let edits = read_edits(&value, DOC, &found).unwrap();
        let fixed = apply(DOC, &found, &edits).unwrap();
        assert_eq!(
            fixed,
            "# Guide\n\n## Récoltes\n\nTu cueilles tôt. Tu la ranges au frais.\n\n- Radis : \
             avril.\n- Poireau : fût de 2 à 3 cm de diamètre.\n\n| Légume | Écart |\n|---|---|\n\
             | Tomate | 50 cm |\n\nFin du guide."
        );
        // Une ligne de tableau aux cellules changées, une phrase sur deux
        // lignes, un numéro inconnu : passe rejetée.
        for bad in [
            json!({"edits": [{"n": 3, "action": "fix", "sentence": "Tomate : 50 cm"}]}),
            json!({"edits": [{"n": 2, "action": "fix", "sentence": "Poireau.\n\nAutre."}]}),
            json!({"edits": [{"n": 4, "action": "remove", "sentence": ""}]}),
            json!({"edits": [{"n": 1, "action": "fix", "sentence": "## Titre"}]}),
        ] {
            assert!(read_edits(&bad, DOC, &found).is_none(), "{bad}");
        }
    }

    /// Contrôle par programme : un paragraphe hors des phrases visées qui
    /// change fait rejeter la passe.
    #[test]
    fn a_change_outside_the_targets_is_caught() {
        let found = targets(DOC, &[wrong("Une tomate trop mûre", "h")]);
        let changed = DOC.replace("Fin du guide.", "Fin.");
        assert!(!untouched_outside(DOC, &changed, &found));
        let fine = DOC.replace("Une tomate trop mûre peut attirer les pucerons.", "Bien.");
        assert!(untouched_outside(DOC, &fine, &found));
    }

    #[test]
    fn an_emptied_list_item_disappears() {
        let doc = "Intro.\n\n- Un.\n- Faux ici.\n- Trois.\n\nFin.";
        let found = targets(doc, &[wrong("Faux ici.", "h")]);
        let edits = vec![Edit {
            index: 0,
            sentence: None,
        }];
        assert_eq!(
            apply(doc, &found, &edits).unwrap(),
            "Intro.\n\n- Un.\n- Trois.\n\nFin."
        );
    }
}
