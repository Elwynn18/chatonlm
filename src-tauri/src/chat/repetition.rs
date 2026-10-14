//! Boucle sûre d'un modèle (propre à ChatonLM) : la fin du raisonnement ou
//! de la réponse répète mot pour mot le même passage. L'envoi est alors
//! arrêté (`ChatErrorKind::Loop`) ; sans garde, Ollama décale son contexte et
//! continue sans fin.
//!
//! Seule la répétition exacte est retenue (espaces mis à part) : un modèle
//! qui « ressasse » en variant ses phrases ressemble trop à une longue
//! réflexion légitime pour être arrêté ; la page le signale sans arrêter
//! (`src/lib/rumination.ts`). Seuils prudents : une liste ou un tableau
//! légitimes ne répètent pas des centaines de caractères identiques.

/// Texte reçu depuis le dernier examen avant d'en refaire un.
const CHECK_EVERY: usize = 512;

/// Période la plus longue recherchée (caractères, espaces réduits).
const MAX_PERIOD: usize = 1_500;

/// Exigences pour conclure à une boucle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rule {
    /// Copies consécutives du passage, au moins.
    repeats: usize,
    /// Longueur totale répétée, au moins (caractères, espaces réduits).
    span: usize,
}

/// Raisonnement : texte de travail, jamais une sortie voulue répétitive.
const THINKING: Rule = Rule {
    repeats: 4,
    span: 800,
};

/// Réponse : une demande (« écris 20 fois… ») peut être répétitive.
const CONTENT: Rule = Rule {
    repeats: 8,
    span: 3_000,
};

/// Réponse dans un bloc de code ouvert (tableau d'octets, données) : bien
/// plus long encore.
const CONTENT_IN_CODE: Rule = Rule {
    repeats: 8,
    span: 10_000,
};

/// Partie de la réponse où la boucle a été trouvée.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Looping {
    Thinking,
    Content,
}

/// Suivi d'une réponse en flux : texte reçu, examiné tous les
/// [`CHECK_EVERY`] caractères ajoutés à l'une des deux parties.
#[derive(Default)]
pub struct LoopWatch {
    thinking: Part,
    content: Part,
}

#[derive(Default)]
struct Part {
    /// Texte reçu, espaces réduits à une seule espace.
    text: Vec<char>,
    /// Longueur lors du dernier examen.
    checked: usize,
    /// Dernier caractère reçu était une espace (réduction entre fragments).
    in_space: bool,
    /// Clôtures de bloc de code (« ``` ») comptées jusqu'ici.
    fences: usize,
    /// Accents graves consécutifs en fin de texte (clôture coupée entre deux
    /// fragments).
    backticks: usize,
}

impl Part {
    fn push(&mut self, fragment: &str) {
        for c in fragment.chars() {
            if c == '`' {
                self.backticks += 1;
                if self.backticks == 3 {
                    self.fences += 1;
                }
            } else {
                self.backticks = 0;
            }
            if c.is_whitespace() {
                if !self.in_space {
                    self.text.push(' ');
                }
                self.in_space = true;
            } else {
                self.text.push(c);
                self.in_space = false;
            }
        }
    }

    fn due(&self) -> bool {
        self.text.len() >= self.checked + CHECK_EVERY
    }
}

impl LoopWatch {
    /// Ajoute un fragment ; `Some` dès qu'une partie finit par une boucle.
    pub fn push(&mut self, thinking: &str, content: &str) -> Option<Looping> {
        self.thinking.push(thinking);
        self.content.push(content);
        if self.thinking.due() {
            self.thinking.checked = self.thinking.text.len();
            if ends_with_loop(&self.thinking.text, THINKING) {
                return Some(Looping::Thinking);
            }
        }
        if self.content.due() {
            self.content.checked = self.content.text.len();
            let rule = if self.content.fences % 2 == 1 {
                CONTENT_IN_CODE
            } else {
                CONTENT
            };
            if ends_with_loop(&self.content.text, rule) {
                return Some(Looping::Content);
            }
        }
        None
    }
}

/// Le texte finit par au moins `rule.repeats` copies d'un même passage
/// (période jusqu'à [`MAX_PERIOD`]), sur au moins `rule.span` caractères.
fn ends_with_loop(text: &[char], rule: Rule) -> bool {
    let len = text.len();
    if len < rule.span {
        return false;
    }
    let longest = MAX_PERIOD.min(len / rule.repeats);
    (1..=longest).any(|period| {
        let needed = rule.span.max(rule.repeats * period);
        // Positions finales où le texte se répète à `period` près : le
        // passage répété couvre ces positions plus une période.
        let periodic = needed - period;
        (len - periodic..len).all(|i| text[i] == text[i - period])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch_all(thinking: &str, content: &str) -> Option<Looping> {
        let mut watch = LoopWatch::default();
        // Fragments de quelques caractères, comme le flux d'Ollama.
        let mut found = None;
        for chunk in thinking.as_bytes().chunks(7) {
            found = found.or(watch.push(std::str::from_utf8(chunk).unwrap(), ""));
        }
        for chunk in content.as_bytes().chunks(7) {
            found = found.or(watch.push("", std::str::from_utf8(chunk).unwrap()));
        }
        found
    }

    /// Raisonnement varié, comme un vrai : aucune phrase identique.
    fn varied(sentences: usize) -> String {
        (0..sentences)
            .map(|i| {
                format!(
                    "Step {i}: check case {} against the rule {}. ",
                    i * 7,
                    i % 13
                )
            })
            .collect()
    }

    #[test]
    fn a_thinking_that_repeats_a_paragraph_is_a_loop() {
        let paragraph = "Need maybe check again whether the hard link count is right. ";
        let thinking = varied(30) + &paragraph.repeat(40);
        assert_eq!(watch_all(&thinking, ""), Some(Looping::Thinking));
    }

    #[test]
    fn a_word_repeated_forever_is_a_loop() {
        assert_eq!(watch_all("", &"the ".repeat(2_000)), Some(Looping::Content));
    }

    #[test]
    fn spacing_differences_do_not_hide_a_loop() {
        let thinking: String = (0..40)
            .map(|i| {
                let gap = if i % 2 == 0 { " \n" } else { "\n\n  " };
                format!("I should verify the answer once more.{gap}")
            })
            .collect();
        assert_eq!(watch_all(&thinking, ""), Some(Looping::Thinking));
    }

    #[test]
    fn a_long_varied_reasoning_is_not_a_loop() {
        assert_eq!(watch_all(&varied(2_000), &varied(2_000)), None);
    }

    #[test]
    fn a_few_repeats_are_not_a_loop() {
        // Trois copies d'un long passage : sous le seuil de quatre.
        let paragraph = "Let me restate the problem in my own words to be sure of it. ".repeat(5);
        let thinking = varied(30) + &paragraph.repeat(3);
        assert_eq!(watch_all(&thinking, ""), None);
    }

    #[test]
    fn a_repetitive_answer_asked_for_is_not_a_loop() {
        // « Écris 10 fois » : 10 lignes identiques, sous les 3 000 caractères.
        let lines = "I will not talk in class during the lesson.\n".repeat(10);
        assert_eq!(watch_all("", &lines), None);
        // Tableau dont les lignes diffèrent par leurs nombres.
        let table: String = (0..400)
            .map(|i| format!("| {i} | {} | ok |\n", i * 3))
            .collect();
        assert_eq!(watch_all("", &table), None);
    }

    #[test]
    fn a_repetitive_code_block_needs_a_longer_run() {
        // Tableau d'octets nuls de 6 000 caractères : légitime dans du code.
        let bytes = "0x00, ".repeat(1_000);
        assert_eq!(
            watch_all("", &format!("```c\nuint8_t b[] = {{{bytes}")),
            None
        );
        // Hors bloc de code, la même suite est une boucle.
        assert_eq!(watch_all("", &bytes), Some(Looping::Content));
        // Dans un bloc, au-delà de 10 000 caractères aussi.
        let longer = "0x00, ".repeat(2_000);
        assert_eq!(
            watch_all("", &format!("```c\nuint8_t b[] = {{{longer}")),
            Some(Looping::Content)
        );
        // Bloc refermé : retour au seuil ordinaire, clôture coupée entre
        // deux fragments comprise.
        let mut watch = LoopWatch::default();
        assert_eq!(watch.push("", "```c\nint a;\n``"), None);
        assert_eq!(watch.push("", "`\n"), None);
        assert_eq!(watch.push("", &bytes), Some(Looping::Content));
    }

    #[test]
    fn the_parts_are_watched_apart() {
        // La réponse ne prolonge pas un passage du raisonnement.
        let half = "Same sentence again and again here. ".repeat(3);
        assert_eq!(watch_all(&(varied(30) + &half), &half), None);
    }
}
