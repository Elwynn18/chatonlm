//! Partie visible d'une réponse en flux pour l'Atelier : celle de l'Étude
//! ([`Visible`]), plus un cas qu'elle laisse de côté. Un modèle dont le
//! gabarit ouvre la réflexion (Qwen3 une fois son budget atteint) écrit sa
//! réflexion dans le contenu puis `</think>` SANS `<think>` : [`Visible`]
//! envoie ce début, puis plus rien, et la réponse n'arrive jamais à la page.
//! Ici, chaque `</think>` commence un nouveau morceau, lu par un [`Visible`]
//! neuf (qui ne voit donc jamais de `</think>` : la sienne cesse aussi
//! d'envoyer quand `</think>` est collé à la réponse) : s'il avait montré
//! quelque chose, l'appelant reçoit [`Delta::Restart`] (oublier ce qui a été
//! montré), puis la suite de la réponse. Le texte final
//! ([`Answer::text`]) est celui de l'Étude (`repair::strip_thinking` : ce
//! qui suit le dernier `</think>`), donc toujours ce que la page a montré
//! depuis le dernier `Restart`, aux espaces des bords près.

use super::Delta;
use crate::study::repair;
use crate::study::request::Visible;

const CLOSE: &str = "</think>";

/// `index` ramené au début du caractère qui le contient.
fn floor(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Réponse en flux (voir le module).
#[derive(Default)]
pub(super) struct Answer {
    raw: String,
    /// Octets de `raw` donnés à `inner` (le morceau en cours commence après
    /// le dernier `</think>`).
    fed: usize,
    /// Les `</think>` qui commencent avant cet octet sont déjà vus.
    checked: usize,
    /// Partie visible du morceau en cours.
    inner: Visible,
    /// Texte montré depuis le début du morceau.
    shown: bool,
    /// Espaces du début d'un morceau sautées (après un `</think>`).
    trim: bool,
}

impl Answer {
    /// Ajoute un fragment ; renvoie ce que l'appelant doit recevoir.
    pub(super) fn push(&mut self, fragment: &str) -> Vec<Delta> {
        self.raw.push_str(fragment);
        let mut out = Vec::new();
        loop {
            let Some(at) = self.raw[self.checked..].find(CLOSE) else {
                // Une balise coupée en fin de fragment sera relue.
                let keep = floor(&self.raw, self.raw.len().saturating_sub(CLOSE.len() - 1));
                self.checked = self.checked.max(keep);
                let rest = self.raw[self.fed..].to_owned();
                self.fed = self.raw.len();
                self.send(&rest, &mut out);
                return out;
            };
            // Tout ce qui précède `</think>` était de la réflexion (ouverte
            // par `<think>`, rien n'en a été montré ; ou par le gabarit).
            if self.shown {
                out.push(Delta::Restart);
            }
            self.fed = self.checked + at + CLOSE.len();
            self.checked = self.fed;
            self.inner = Visible::default();
            self.shown = false;
            self.trim = true;
        }
    }

    fn send(&mut self, text: &str, out: &mut Vec<Delta>) {
        let mut shown = self.inner.push(text);
        if self.trim {
            shown = shown.trim_start().to_owned();
            self.trim = shown.is_empty();
        }
        if !shown.is_empty() {
            self.shown = true;
            out.push(Delta::Text(shown));
        }
    }

    /// Texte complet, sans réflexion.
    pub(super) fn text(&self) -> String {
        repair::strip_thinking(&self.raw).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fragments donnés un à un : ce que reçoit l'appelant.
    fn run(pieces: &[&str]) -> (Vec<Delta>, String) {
        let mut answer = Answer::default();
        let deltas = pieces.iter().flat_map(|piece| answer.push(piece)).collect();
        (deltas, answer.text())
    }

    /// Texte reçu depuis le dernier `Restart`.
    fn shown(deltas: &[Delta]) -> String {
        let start = deltas
            .iter()
            .rposition(|delta| *delta == Delta::Restart)
            .map_or(0, |at| at + 1);
        deltas[start..]
            .iter()
            .map(|delta| match delta {
                Delta::Text(text) => text.as_str(),
                Delta::Restart => "",
            })
            .collect()
    }

    /// Réflexion écrite dans le contenu puis `</think>` sans `<think>` : la
    /// réflexion montrée est reprise (`Restart`), la réponse suit.
    #[test]
    fn an_orphan_closing_tag_restarts_the_answer() {
        let (deltas, text) = run(&[
            "Let me count",
            ": 3 apples.</th",
            "ink>\n\nThere are ",
            "3 apples.",
        ]);
        assert_eq!(text, "There are 3 apples.");
        assert_eq!(
            deltas,
            [
                Delta::Text("Let me count".into()),
                Delta::Text(": 3 apples.".into()),
                Delta::Restart,
                Delta::Text("There are ".into()),
                Delta::Text("3 apples.".into()),
            ]
        );
        // Rien de montré avant la balise : pas de `Restart`.
        let (deltas, text) = run(&["</think>", " Answer"]);
        assert_eq!(deltas, [Delta::Text("Answer".into())]);
        assert_eq!(text, "Answer");
        // Deux balises orphelines : la dernière fait foi.
        let (deltas, text) = run(&["a </think> b </think>", " c"]);
        assert_eq!(shown(&deltas), "c");
        assert_eq!(text, "c");
    }

    /// Comme l'Étude pour le reste : réflexion ouverte et fermée retirée,
    /// texte ordinaire envoyé tel quel, balise coupée retenue.
    #[test]
    fn otherwise_it_streams_like_the_courses() {
        let (deltas, text) = run(&[
            "<th",
            "ink>\nLet me",
            " think.</thi",
            "nk>\n\nThe ",
            "answer.",
        ]);
        assert!(!deltas.contains(&Delta::Restart));
        assert_eq!(shown(&deltas), "The answer.");
        assert_eq!(text, "The answer.");
        let (deltas, _) = run(&["Plain ", "<", "b>bold</b> text"]);
        assert_eq!(shown(&deltas), "Plain <b>bold</b> text");
        // Réflexion close puis une autre balise orpheline : reprise après.
        let (deltas, text) = run(&["<think>x</think>Draft ", "</think>Final"]);
        assert_eq!(deltas.iter().filter(|d| **d == Delta::Restart).count(), 1);
        assert_eq!(shown(&deltas), "Final");
        assert_eq!(text, "Final");
        // `<think>` au milieu : rien après lui, jusqu'à sa fin.
        let (deltas, text) = run(&["Hello <th", "ink>secret</think> there"]);
        assert_eq!(shown(&deltas), "there");
        assert_eq!(text, "there");
        // Caractères multi-octets autour d'une balise coupée.
        let (deltas, text) = run(&["é<", "/thi", "nk>ü"]);
        assert_eq!(shown(&deltas), "ü");
        assert_eq!(text, "ü");
    }
}
