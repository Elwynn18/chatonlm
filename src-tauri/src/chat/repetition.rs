//! Boucle sûre d'un modèle (propre à ChatonLM) : la fin du raisonnement ou
//! de la réponse répète mot pour mot le même passage. L'envoi est alors
//! arrêté (`ChatErrorKind::Loop`) ; sans garde, Ollama décale son contexte et
//! continue sans fin.
//!
//! Seule la répétition exacte est retenue (espaces mis à part) : un modèle
//! qui « ressasse » en variant ses phrases ressemble trop à une longue
//! réflexion légitime pour être arrêté ; la page le signale sans arrêter
//! (`src/lib/rumination.ts`). Seuils prudents, une sortie demandée pouvant
//! être répétitive (« écris 1 000 fois… », grille ou matrice de zéros, tableau
//! d'octets) : un passage sans lettres (chiffres, ponctuation) doit se
//! répéter bien plus longtemps, et plus encore dans un bloc de code ouvert
//! (clôtures « ``` » ou « ~~~ » en début de ligne, comme CommonMark). Une
//! vraie boucle ne s'arrête jamais : attendre quelques milliers de
//! caractères de plus ne coûte que quelques secondes.

/// Texte reçu depuis le dernier examen avant d'en refaire un.
const CHECK_EVERY: usize = 512;

/// Période la plus longue recherchée (caractères, espaces réduits).
const MAX_PERIOD: usize = 6_000;

/// Lettres au moins dans le passage répété pour la règle ordinaire.
const MIN_LETTERS: usize = 3;

/// Exigences pour conclure à une boucle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rule {
    /// Copies consécutives du passage, au moins.
    repeats: usize,
    /// Longueur totale répétée, au moins (caractères, espaces réduits).
    span: usize,
    /// Même chose pour un passage sans lettres.
    span_without_letters: usize,
}

/// Raisonnement : texte de travail ; un brouillon de la réponse demandée
/// peut y être répétitif.
const THINKING: Rule = Rule {
    repeats: 6,
    span: 3_000,
    span_without_letters: 10_000,
};

/// Réponse : une demande (« écris 100 fois… ») peut être répétitive.
const CONTENT: Rule = Rule {
    repeats: 8,
    span: 15_000,
    span_without_letters: 30_000,
};

/// Réponse dans un bloc de code ouvert (tableau d'octets, données).
const CONTENT_IN_CODE: Rule = Rule {
    repeats: 8,
    span: 30_000,
    span_without_letters: 30_000,
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
    /// Début de ligne en cours de lecture : retrait puis clôture éventuelle
    /// (faux dès le premier autre caractère de la ligne).
    leading: Leading,
    /// Bloc de code ouvert : caractère et longueur de sa clôture.
    fence: Option<(char, usize)>,
}

/// Début d'une ligne : espaces de retrait, puis suite de « ` » ou de « ~ ».
#[derive(Clone, Copy)]
struct Leading {
    reading: bool,
    indent: usize,
    run: Option<(char, usize)>,
}

impl Default for Leading {
    fn default() -> Self {
        Self {
            reading: true,
            indent: 0,
            run: None,
        }
    }
}

impl Part {
    fn push(&mut self, fragment: &str) {
        for c in fragment.chars() {
            self.lead(c);
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

    /// Suit les clôtures de bloc de code : au plus 3 espaces de retrait, au
    /// moins 3 « ` » ou « ~ » ; un bloc se ferme par le même caractère, au
    /// moins aussi long.
    fn lead(&mut self, c: char) {
        if c == '\n' {
            self.end_of_lead();
            self.leading = Leading::default();
            return;
        }
        let leading = &mut self.leading;
        if !leading.reading {
            return;
        }
        match (c, leading.run) {
            (' ', None) if leading.indent < 3 => leading.indent += 1,
            ('`' | '~', None) => leading.run = Some((c, 1)),
            (c, Some((fence, count))) if c == fence => leading.run = Some((fence, count + 1)),
            _ => self.end_of_lead(),
        }
    }

    /// Fin du début de ligne : une suite d'au moins 3 ouvre ou ferme un bloc.
    fn end_of_lead(&mut self) {
        if !self.leading.reading {
            return;
        }
        self.leading.reading = false;
        let Some((fence, count)) = self.leading.run.filter(|(_, count)| *count >= 3) else {
            return;
        };
        self.fence = match self.fence {
            None => Some((fence, count)),
            Some((open, length)) if open == fence && count >= length => None,
            open => open,
        };
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
            let rule = if self.content.fence.is_some() {
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
/// (période jusqu'à [`MAX_PERIOD`]), sur au moins `rule.span` caractères
/// (`rule.span_without_letters` pour un passage sans lettres).
fn ends_with_loop(text: &[char], rule: Rule) -> bool {
    let len = text.len();
    if len < rule.span {
        return false;
    }
    // Répété à `period` près sur les `length` derniers caractères.
    let periodic = |period: usize, length: usize| {
        length <= len && (len - (length - period)..len).all(|i| text[i] == text[i - period])
    };
    let longest = MAX_PERIOD.min(len / rule.repeats);
    (1..=longest).any(|period| {
        let needed = rule.span.max(rule.repeats * period);
        if !periodic(period, needed) {
            return false;
        }
        let letters = text[len - period..]
            .iter()
            .filter(|c| c.is_alphabetic())
            .count();
        letters >= MIN_LETTERS || periodic(period, rule.span_without_letters.max(needed))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch_all(thinking: &str, content: &str) -> Option<Looping> {
        let mut watch = LoopWatch::default();
        // Fragments de quelques caractères, comme le flux d'Ollama.
        let mut found = None;
        let chunks = |text: &str| {
            text.chars()
                .collect::<Vec<_>>()
                .chunks(7)
                .map(|chunk| chunk.iter().collect::<String>())
                .collect::<Vec<_>>()
        };
        for chunk in chunks(thinking) {
            found = found.or(watch.push(&chunk, ""));
        }
        for chunk in chunks(content) {
            found = found.or(watch.push("", &chunk));
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
        let thinking = varied(30) + &paragraph.repeat(80);
        assert_eq!(watch_all(&thinking, ""), Some(Looping::Thinking));
    }

    #[test]
    fn a_word_repeated_forever_is_a_loop() {
        assert_eq!(watch_all("", &"the ".repeat(5_000)), Some(Looping::Content));
    }

    #[test]
    fn spacing_differences_do_not_hide_a_loop() {
        let thinking: String = (0..120)
            .map(|i| {
                let gap = if i % 2 == 0 { " \n" } else { "\n\n  " };
                format!("I should verify the answer once more.{gap}")
            })
            .collect();
        assert_eq!(watch_all(&thinking, ""), Some(Looping::Thinking));
    }

    #[test]
    fn a_long_period_is_seen() {
        // Un « script » de plus de 2 000 caractères réécrit sans fin.
        let script: String = (0..150).map(|i| format!("chmod {i} file{i}; ")).collect();
        assert!(script.len() > 2_000);
        let thinking = varied(30) + &script.repeat(12);
        assert_eq!(watch_all(&thinking, ""), Some(Looping::Thinking));
    }

    #[test]
    fn a_long_varied_reasoning_is_not_a_loop() {
        assert_eq!(watch_all(&varied(2_000), &varied(2_000)), None);
    }

    #[test]
    fn a_few_repeats_are_not_a_loop() {
        // Cinq copies d'un long passage : sous le seuil de six.
        let paragraph = "Let me restate the problem in my own words to be sure of it. ".repeat(10);
        let thinking = varied(30) + &paragraph.repeat(5);
        assert_eq!(watch_all(&thinking, ""), None);
    }

    #[test]
    fn a_repetitive_answer_asked_for_is_not_a_loop() {
        // « Écris 100 fois » ; « I love you » 1 000 fois.
        let lines = "I will not talk in class during the lesson.\n".repeat(100);
        assert_eq!(watch_all("", &lines), None);
        assert_eq!(watch_all("", &"I love you ".repeat(1_000)), None);
        // Un brouillon répétitif de la réponse dans le raisonnement.
        let draft = format!("Draft: {}", "I love you. ".repeat(200));
        assert_eq!(watch_all(&(varied(30) + &draft), ""), None);
        // Tableau dont les lignes diffèrent par leurs nombres.
        let table: String = (0..400)
            .map(|i| format!("| {i} | {} | ok |\n", i * 3))
            .collect();
        assert_eq!(watch_all("", &table), None);
    }

    #[test]
    fn grids_and_data_without_letters_need_a_much_longer_run() {
        // Grille 30 × 30 de zéros (type ARC), matrice nulle : légitimes.
        let grid = "0 ".repeat(30).trim_end().to_owned() + "\n";
        let thinking = varied(30) + "The output grid is:\n" + &grid.repeat(30);
        assert_eq!(watch_all(&thinking, ""), None);
        let json_grid = format!("[{}],\n", vec!["0"; 30].join(", "));
        assert_eq!(watch_all(&json_grid.repeat(30), ""), None);
        // CSV de lignes identiques dans la réponse, hors bloc de code.
        assert_eq!(watch_all("", &"0,0,0.0,-1\n".repeat(1_500)), None);
        // Bien plus longs : des boucles.
        assert_eq!(watch_all(&grid.repeat(200), ""), Some(Looping::Thinking));
        assert_eq!(
            watch_all("", &"0,0,0.0,-1\n".repeat(3_000)),
            Some(Looping::Content)
        );
    }

    #[test]
    fn a_repetitive_code_block_needs_a_longer_run() {
        let code = "let value = compute(next, other);\n";
        // 20 000 caractères de la même ligne : légitime dans un bloc ouvert.
        let block = format!("```rust\n{}", code.repeat(600));
        assert_eq!(watch_all("", &block), None);
        // Hors bloc de code, la même suite est une boucle.
        assert_eq!(watch_all("", &code.repeat(600)), Some(Looping::Content));
        // Dans un bloc, au-delà de 30 000 caractères aussi.
        let longer = format!("```rust\n{}", code.repeat(1_000));
        assert_eq!(watch_all("", &longer), Some(Looping::Content));
    }

    #[test]
    fn code_fences_are_followed_as_in_commonmark() {
        let code = "let value = compute(next, other);\n";
        let open = |prefix: &str| watch_all("", &format!("{prefix}{}", code.repeat(600)));
        // Clôture « ~~~ », retrait de trois espaces : bloc ouvert.
        assert_eq!(open("~~~c\n"), None);
        assert_eq!(open("   ```\n"), None);
        // Bloc intérieur plus court dans un bloc ````markdown : l'extérieur
        // reste ouvert ; une mention de « ``` » dans une phrase ne compte pas.
        assert_eq!(open("````markdown\n```c\nint a;\n```\n"), None);
        assert_eq!(open("```c\nWrap it in ``` like this.\n"), None);
        // Quatre espaces de retrait : pas une clôture.
        assert_eq!(open("    ```\n"), Some(Looping::Content));
        // Bloc refermé (clôture coupée entre deux fragments) : seuil
        // ordinaire.
        let mut watch = LoopWatch::default();
        assert_eq!(watch.push("", "```c\nint a;\n``"), None);
        assert_eq!(watch.push("", "`\n"), None);
        assert_eq!(watch.push("", &code.repeat(600)), Some(Looping::Content));
        // « ~~~ » ne ferme pas un bloc ouvert par « ``` ».
        assert_eq!(open("```\nint a;\n~~~\n"), None);
    }

    #[test]
    fn the_parts_are_watched_apart() {
        // La réponse ne prolonge pas un passage du raisonnement.
        let half = "Same sentence again and again here. ".repeat(3);
        assert_eq!(watch_all(&(varied(30) + &half), &half), None);
    }
}
