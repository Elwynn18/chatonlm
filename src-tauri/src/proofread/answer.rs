//! Réponse du modèle : fragments montrés pendant le flux ([`Visible`]) et
//! texte final nettoyé ([`clean`]), avant le garde-fou.

use super::guard;
use crate::dictation::polish::{QUOTES, strip_thinking, unwrap_pair, unwrap_quotes};

const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";
const TEXT_OPEN: &str = "<text>";
const TEXT_CLOSE: &str = "</text>";
const FENCE: &str = "```";

/// Le passage contient lui-même une balise de raisonnement (texte qui parle
/// de modèles) : ces balises y sont du texte, jamais retirées ni suivies.
fn literal_think(original: &str) -> bool {
    original.contains(THINK_OPEN) || original.contains(THINK_CLOSE)
}

/// Réponse nettoyée : sans raisonnement (`<think>…</think>`, ou fin d'un
/// raisonnement ouvert par le gabarit du modèle, sauf si le passage contient
/// lui-même ces balises), sans espaces, balises `<text>`, guillemets ni bloc
/// de code « ``` » autour (sauf si le passage les avait lui-même), sans gras
/// `**` ni espaces en fin de ligne si le passage n'en avait pas (ministral-3
/// met ses corrections en gras, qwen3 force des sauts de ligne Markdown),
/// avec les fins de ligne du passage (CRLF, U+2028, U+2029).
pub fn clean(original: &str, answer: &str) -> String {
    let mut text = answer.to_owned();
    if !literal_think(original) {
        text = strip_thinking(&text);
        if let Some(end) = text.rfind(THINK_CLOSE) {
            text.drain(..end + THINK_CLOSE.len());
        }
    }
    let trimmed_original = original.trim();
    let fenced = original.contains(FENCE);
    let mut text = loop {
        let trimmed = text.trim();
        let tags = unwrap_pair(trimmed, TEXT_OPEN, TEXT_CLOSE)
            .or_else(|| trimmed.strip_prefix(TEXT_OPEN))
            .or_else(|| trimmed.strip_suffix(TEXT_CLOSE))
            .filter(|_| {
                !trimmed_original.starts_with(TEXT_OPEN) && !trimmed_original.ends_with(TEXT_CLOSE)
            });
        let quotes = || {
            QUOTES.iter().find_map(|(open, close)| {
                let quoted =
                    trimmed_original.starts_with(open) && trimmed_original.ends_with(close);
                (!quoted)
                    .then(|| unwrap_quotes(trimmed, open, close))
                    .flatten()
            })
        };
        let fence = || (!fenced).then(|| unwrap_fence(trimmed)).flatten();
        match tags.or_else(quotes).or_else(fence) {
            Some(inner) => text = inner.to_owned(),
            None => break trimmed.to_owned(),
        }
    };
    if !original.contains("**") {
        text = unbold(original, &text);
    }
    if !has_trailing_blanks(original) && has_trailing_blanks(&text) {
        text = text
            .split_inclusive('\n')
            .map(|line| {
                let ending = if line.ends_with("\r\n") {
                    "\r\n"
                } else if line.ends_with('\n') {
                    "\n"
                } else {
                    ""
                };
                let content = &line[..line.len() - ending.len()];
                format!("{}{ending}", content.trim_end_matches([' ', '\t']))
            })
            .collect();
    }
    with_breaks_of(original, text)
}

/// Réponse entière dans un bloc de code (« ```langue\n…\n``` ») : son
/// contenu.
fn unwrap_fence(text: &str) -> Option<&str> {
    let inner = text.strip_prefix(FENCE)?.strip_suffix(FENCE)?;
    let (info, body) = inner.split_once('\n')?;
    (!info.contains('`') && !info.trim().contains(char::is_whitespace)).then_some(body)
}

/// Gras « **x** » ajouté par le modèle : retiré, ou rendu « *x* » si le
/// passage avait cette emphase (le modèle l'a doublée : « *très* » devenu
/// « **très** »). Un « ** » seul est retiré.
fn unbold(original: &str, text: &str) -> String {
    // Emphases du passage : entre deux « * » (le passage n'a pas de « ** »),
    // sans espace au bord (une puce « * » de liste n'en est pas une).
    let pieces: Vec<&str> = original.split('*').collect();
    let emphases: Vec<Vec<Vec<char>>> = (1..pieces.len().saturating_sub(1))
        .step_by(2)
        .map(|index| pieces[index])
        .filter(|inner| !inner.is_empty() && inner.trim() == *inner)
        .map(guard::words)
        .filter(|words| !words.is_empty())
        .collect();
    let emphasized = |inner: &str| {
        let words = guard::words(inner);
        emphases.iter().any(|emphasis| {
            emphasis.len() == words.len()
                && emphasis.iter().zip(&words).all(|(a, b)| guard::close(a, b))
        })
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("**") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        match after.find("**") {
            Some(close) => {
                let inner = &after[..close];
                let mark = if emphasized(inner) { "*" } else { "" };
                out.push_str(mark);
                out.push_str(inner);
                out.push_str(mark);
                rest = &after[close + 2..];
            }
            None => rest = after,
        }
    }
    out.push_str(rest);
    out
}

/// Fins de ligne du texte, dans l'ordre : « \r\n », « \n », U+2028, U+2029.
fn breaks(text: &str) -> Vec<&'static str> {
    text.char_indices()
        .filter_map(|(at, c)| match c {
            '\n' if text[..at].ends_with('\r') => Some("\r\n"),
            '\n' => Some("\n"),
            '\u{2028}' => Some("\u{2028}"),
            '\u{2029}' => Some("\u{2029}"),
            _ => None,
        })
        .collect()
}

/// Fins de ligne du passage rendues à la réponse quand le modèle les a toutes
/// changées en « \n » (CRLF, séparateurs Unicode de Word ou de LibreOffice) :
/// une à une s'il y en a autant, sinon celle du passage partout s'il n'en a
/// qu'une sorte.
fn with_breaks_of(original: &str, text: String) -> String {
    let (ours, theirs) = (breaks(original), breaks(&text));
    if theirs.is_empty() || theirs.iter().any(|b| *b != "\n") || ours.iter().all(|b| *b == "\n") {
        return text;
    }
    let replacements = if ours.len() == theirs.len() {
        ours
    } else if ours.iter().all(|b| *b == ours[0]) {
        vec![ours[0]; theirs.len()]
    } else {
        return text;
    };
    let mut replacements = replacements.into_iter();
    let mut out = String::with_capacity(text.len() + theirs.len());
    for c in text.chars() {
        match c {
            '\n' => out.push_str(replacements.next().unwrap_or("\n")),
            other => out.push(other),
        }
    }
    out
}

/// Une ligne (hors la dernière) finit par des espaces ou des tabulations.
fn has_trailing_blanks(text: &str) -> bool {
    text.split('\n')
        .rev()
        .skip(1)
        .any(|line| line.trim_end_matches('\r').ends_with([' ', '\t']))
}

/// Mots complets d'un début de réponse attendus avant de le comparer au
/// passage (moins si le passage en a moins).
const FIRST_WORDS: usize = 3;

/// Premiers mots comparés au passage : au-delà, un début qui ne lui
/// ressemble pas ne lui ressemblera plus.
const JUDGED_WORDS: usize = 8;

/// Raisonnement ouvert par le gabarit toléré (caractères) : tant qu'aucun
/// `</think>` n'est venu et que le début ne ressemble pas au passage, la
/// lecture continue jusqu'à la longueur maximale de la réponse plus celle-ci.
/// Environ 2 000 jetons, 20 s à 100 jetons par seconde : le raisonnement
/// d'une correction courte y tient ; une réponse qui n'est pas le texte (un
/// poème) est lue plus longtemps avant d'être écartée, sans autre effet que
/// ce temps (les poèmes mesurés font 125 à 650 caractères, le modèle s'arrête
/// de lui-même).
pub const REASONING_CHARS: usize = 8_000;

/// Filtre du flux : ce qui peut être montré du contenu reçu, sans
/// raisonnement `<think>…</think>` ni balises `<text>` (même coupés entre deux
/// fragments : la fin d'un fragment qui pourrait commencer une balise attend
/// le suivant), sans les espaces du début.
///
/// Le début de la réponse est retenu tant qu'il peut être un raisonnement
/// ouvert par le gabarit du modèle (seul `</think>` écrit dans le contenu,
/// modèle GGUF sans la capacité « thinking » chez Ollama) : jusqu'à
/// `</think>` (ce qui précède est jeté, et ne compte pas dans la longueur),
/// `<think>`, ou un début qui ressemble au passage (au moins la moitié de ses
/// premiers mots s'y alignent, dès [`FIRST_WORDS`] mots) ; sinon jusqu'à la
/// fin de la réponse ([`Visible::finish`]). Limite : un raisonnement qui
/// commence comme le passage (il le cite d'emblée) est montré, puis ne compte
/// plus dans la longueur à son `</think>`. Un passage qui contient lui-même
/// `<think>` ou `</think>` : balises gardées comme du texte, rien n'est
/// retenu.
pub struct Visible {
    /// Mots du passage.
    words: Vec<Vec<char>>,
    /// Balises de raisonnement gardées comme du texte (le passage en a).
    literal: bool,
    /// Début de la réponse retenu (voir plus haut).
    holding: bool,
    /// Début retenu comparé une fois pour toutes : il ne ressemble pas au
    /// passage.
    judged: bool,
    held: String,
    held_chars: usize,
    pending: String,
    thinking: bool,
    started: bool,
    /// Caractères montrés jusqu'ici (depuis le dernier `</think>`).
    shown: usize,
}

impl Visible {
    /// Filtre de la réponse au passage `original`.
    pub fn new(original: &str) -> Self {
        let (words, literal) = (guard::words(original), literal_think(original));
        Self {
            holding: !literal && !words.is_empty(),
            words,
            literal,
            judged: false,
            held: String::new(),
            held_chars: 0,
            pending: String::new(),
            thinking: false,
            started: false,
            shown: 0,
        }
    }

    /// Ajoute un fragment ; renvoie ce qui peut être montré maintenant
    /// (parfois rien).
    pub fn push(&mut self, fragment: &str) -> String {
        if !self.holding {
            return self.stream(fragment);
        }
        self.held.push_str(fragment);
        self.held_chars += fragment.chars().count();
        let open = self.held.find(THINK_OPEN);
        match self.held.find(THINK_CLOSE) {
            // Fin d'un raisonnement ouvert par le gabarit : jeté.
            Some(at) if open.is_none_or(|open| at < open) => {
                let rest = self.held.split_off(at + THINK_CLOSE.len());
                self.held.clear();
                self.holding = false;
                self.stream(&rest)
            }
            // Raisonnement délimité par le modèle : filtré par le flux.
            _ if open.is_some() => self.release(),
            _ if self.like_the_text() => self.release(),
            _ => String::new(),
        }
    }

    /// Fin de la réponse : ce qui était encore retenu.
    pub fn finish(&mut self) -> String {
        if self.holding {
            self.release()
        } else {
            String::new()
        }
    }

    /// La réponse dépasse `limit` caractères, sans compter un raisonnement
    /// jeté ; [`REASONING_CHARS`] de plus tant que le début retenu peut
    /// encore en être un.
    pub fn over(&self, limit: usize) -> bool {
        if self.holding {
            self.held_chars > limit + REASONING_CHARS
        } else {
            self.shown > limit
        }
    }

    /// Caractères montrés jusqu'ici.
    #[cfg(test)]
    pub fn shown(&self) -> usize {
        self.shown
    }

    fn release(&mut self) -> String {
        self.holding = false;
        let held = std::mem::take(&mut self.held);
        self.stream(&held)
    }

    /// Le début retenu ressemble au passage : au moins la moitié de ses
    /// premiers mots complets (suivis d'un autre caractère) s'y alignent.
    fn like_the_text(&mut self) -> bool {
        if self.judged {
            return false;
        }
        let text = self.held.replace(TEXT_OPEN, " ").replace(TEXT_CLOSE, " ");
        let end = text.rfind(|c: char| !c.is_alphanumeric()).unwrap_or(0);
        let words: Vec<Vec<char>> = guard::words(&text[..end])
            .into_iter()
            .take(JUDGED_WORDS)
            .collect();
        if words.len() < FIRST_WORDS.min(self.words.len()) {
            return false;
        }
        if guard::aligned(&self.words, &words) * 2 >= words.len() {
            return true;
        }
        // Long début sans mots complets : plus la peine de chercher.
        self.judged = words.len() >= JUDGED_WORDS || self.held.len() > 4096;
        false
    }

    /// Filtre des balises, une fois le début relâché.
    fn stream(&mut self, fragment: &str) -> String {
        self.pending.push_str(fragment);
        let mut out = String::new();
        loop {
            if self.thinking {
                match self.pending.find(THINK_CLOSE) {
                    Some(at) => {
                        self.pending.drain(..at + THINK_CLOSE.len());
                        self.thinking = false;
                    }
                    None => {
                        let keep = partial_tag(&self.pending, &[THINK_CLOSE]);
                        self.pending.drain(..self.pending.len() - keep);
                        break;
                    }
                }
                continue;
            }
            let open = self.pending.find(THINK_OPEN).filter(|_| !self.literal);
            let close = self.pending.find(THINK_CLOSE).filter(|_| !self.literal);
            match (open, close) {
                // Fin d'un raisonnement ouvert par le gabarit, montré parce
                // qu'il commençait comme le passage : il ne compte plus.
                (_, Some(at)) if open.is_none_or(|open| at < open) => {
                    self.pending.drain(..at + THINK_CLOSE.len());
                    out.clear();
                    self.shown = 0;
                    self.started = false;
                }
                (Some(at), _) => {
                    out.push_str(&self.pending[..at]);
                    self.pending.drain(..at + THINK_OPEN.len());
                    self.thinking = true;
                }
                _ => {
                    let tags: &[&str] = if self.literal {
                        &[TEXT_OPEN, TEXT_CLOSE]
                    } else {
                        &[THINK_OPEN, THINK_CLOSE, TEXT_OPEN, TEXT_CLOSE]
                    };
                    let keep = partial_tag(&self.pending, tags);
                    let ready = self.pending.len() - keep;
                    out.push_str(&self.pending[..ready]);
                    self.pending.drain(..ready);
                    break;
                }
            }
        }
        let mut out = out.replace(TEXT_OPEN, "").replace(TEXT_CLOSE, "");
        if !self.started {
            out = out.trim_start().to_owned();
            self.started = !out.is_empty();
        }
        self.shown += out.chars().count();
        out
    }
}

/// Longueur (octets) de la plus longue fin de `text` qui commence l'une des
/// balises `tags` sans la compléter.
fn partial_tag(text: &str, tags: &[&str]) -> usize {
    tags.iter()
        .flat_map(|tag| (1..tag.len()).rev().find(|&n| text.ends_with(&tag[..n])))
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_cleaned() {
        assert_eq!(clean("Bonjour", "  Bonjour.\n"), "Bonjour.");
        assert_eq!(clean("x", "<text>\nBonjour.\n</text>"), "Bonjour.");
        // Balise seule au début ou à la fin (réponse coupée, ou à moitié
        // balisée).
        assert_eq!(clean("x", "<text>\nBonjour."), "Bonjour.");
        assert_eq!(clean("x", "Bonjour.\n</text>"), "Bonjour.");
        assert_eq!(
            clean("x", "<think>\nhmm\n</think>\n\n« Bonjour. »"),
            "Bonjour."
        );
        // Raisonnement ouvert par le gabarit : seule sa fin est écrite.
        assert_eq!(
            clean("x", "The user wants...\n</think>\nBonjour."),
            "Bonjour."
        );
        assert_eq!(clean("x", "<think>never closed"), "");
        // Guillemets gardés si le passage en avait.
        assert_eq!(clean("\"Salut\"", "\"Salut !\""), "\"Salut !\"");
        assert_eq!(clean("Salut", "\"Salut !\""), "Salut !");
        // Corrections mises en gras : gras retiré, sauf s'il y en avait.
        assert_eq!(
            clean("le budget need", "Le budget **needs**"),
            "Le budget needs"
        );
        assert_eq!(clean("**Note** : ok", "**Note** : OK"), "**Note** : OK");
        // Réponse entière dans un bloc de code : bloc retiré, sauf si le
        // passage en avait un.
        assert_eq!(clean("Le rapport", "```\nLe rapport.\n```"), "Le rapport.");
        assert_eq!(
            clean("Le rapport", "```text\n<text>\nLe rapport.\n</text>\n```"),
            "Le rapport."
        );
        let code = "```\nlet x = 1;\n```";
        assert_eq!(clean(code, code), code);
    }

    /// Emphase simple du passage doublée par le modèle : rendue telle quelle
    /// (relecture du 6 oct.), pas retirée avec le gras ajouté.
    #[test]
    fn a_doubled_emphasis_is_given_back() {
        assert_eq!(
            clean(
                "C'est *très* important pour nous tous.",
                "C'est **très** important pour nous tous."
            ),
            "C'est *très* important pour nous tous."
        );
        // Corrigée au passage, ou à côté d'une correction mise en gras.
        assert_eq!(
            clean(
                "C'est *tres* important, le budget need",
                "C'est **très** important, le budget **needs**"
            ),
            "C'est *très* important, le budget needs"
        );
        // Une puce « * » n'est pas une emphase.
        assert_eq!(clean("* un\n* deux", "* **un**\n* deux"), "* un\n* deux");
    }

    /// Balises de raisonnement écrites dans le passage lui-même (relecture du
    /// 6 oct.) : du texte, ni retiré ni suivi comme un raisonnement.
    #[test]
    fn think_tags_of_the_text_are_text() {
        let original = "Le modèle écrit <think> puis </think> avant sa réponse, c'est normal.";
        let answer = "Le modèle écrit <think> puis </think> avant sa réponse, c'est normal.";
        assert_eq!(clean(original, answer), answer);
        for size in [1, 3, 7, answer.len()] {
            assert_eq!(
                shown(original, &chunks(answer, size)),
                answer,
                "size {size}"
            );
        }
        let original = "Un </think> seul.";
        assert_eq!(clean(original, "Un </think> seul."), "Un </think> seul.");
    }

    #[test]
    fn line_endings_follow_the_original() {
        // Espaces de fin de ligne (saut forcé en Markdown) retirés…
        assert_eq!(
            clean(
                "Pour demain :\n- pain\n- lait",
                "Pour demain :  \n- pain  \n- lait"
            ),
            "Pour demain :\n- pain\n- lait"
        );
        // … sauf si le passage en avait.
        assert_eq!(clean("a  \nb", "A  \nB"), "A  \nB");
        // CRLF du passage rendus.
        assert_eq!(clean("a\r\nb", "A\nB"), "A\r\nB");
        assert_eq!(clean("a\r\nb", "A  \r\nB"), "A\r\nB");
        assert_eq!(clean("a\r\nb", "A\nB\n\nC"), "A\r\nB\r\n\r\nC");
        // Séparateurs Unicode (Word, LibreOffice) rendus, un à un s'ils sont
        // mêlés.
        assert_eq!(clean("a\u{2028}b", "A\nB"), "A\u{2028}B");
        assert_eq!(
            clean("a\u{2029}b\nc\r\nd", "A\nB\nC\nD"),
            "A\u{2029}B\nC\r\nD"
        );
        assert_eq!(clean("a\u{2029}b\nc", "A\nB"), "A\nB");
    }

    fn chunks(answer: &str, size: usize) -> Vec<String> {
        let chars: Vec<char> = answer.chars().collect();
        chars
            .chunks(size)
            .map(|chunk| chunk.iter().collect())
            .collect()
    }

    /// Montré au fil des fragments, fin de la réponse comprise.
    fn shown(original: &str, fragments: &[String]) -> String {
        let mut visible = Visible::new(original);
        let mut text: String = fragments.iter().map(|f| visible.push(f)).collect();
        text.push_str(&visible.finish());
        assert_eq!(visible.shown(), text.chars().count());
        text
    }

    #[test]
    fn thinking_and_tags_are_never_shown_even_when_cut() {
        let answer = "<think>\nLet me think.\n</think>\n\n<text>\nBonjour, ça va ?\n</text>";
        let expected = "Bonjour, ça va ?\n";
        // Coupé partout, en morceaux de toutes tailles.
        for size in 1..answer.len() {
            let text = shown("Bonjour, ca va ?", &chunks(answer, size));
            assert_eq!(text, expected, "size {size}");
        }
        let fragments = ["Bon", "jour <", "thi", "nk>secret</th", "ink> toi"].map(String::from);
        assert_eq!(shown("Bonjour toi", &fragments), "Bonjour  toi");
        // Un « < » qui ne commence pas de balise sort tel quel.
        let fragments = ["a < b", " et c <", "= d"].map(String::from);
        assert_eq!(shown("a < b et c <= d", &fragments), "a < b et c <= d");
    }

    /// Raisonnement ouvert par le gabarit, seul `</think>` écrit (relecture
    /// du 6 oct.) : jamais montré, et pas compté dans la longueur.
    #[test]
    fn reasoning_opened_by_the_template_is_held_back() {
        let original = "Bonjour, ca va bien ?";
        let answer = "Okay, the user wants me to proofread this French sentence. \
                      The word ca should be ça. Let me write the corrected text.\n</think>\n\n\
                      Bonjour, ça va bien ?";
        let limit = guard::max_chars(super::super::ProofreadMode::Correct, original);
        for size in 1..answer.chars().count() {
            let mut visible = Visible::new(original);
            let mut text = String::new();
            for fragment in chunks(answer, size) {
                text.push_str(&visible.push(&fragment));
                assert!(!visible.over(limit), "size {size}");
            }
            text.push_str(&visible.finish());
            assert_eq!(text, "Bonjour, ça va bien ?", "size {size}");
            assert_eq!(visible.shown(), 21);
        }
        // Montré parce qu'il commençait comme le passage : ce qui précède
        // `</think>` ne compte plus.
        let mut visible = Visible::new("bonjour à tous");
        assert_eq!(
            visible.push("Bonjour à tous, dit le texte. "),
            "Bonjour à tous, dit le texte. "
        );
        visible.push("Je corrige.</think>\nBonjour à tous.");
        assert_eq!(visible.shown(), "Bonjour à tous.".chars().count());
    }

    /// Une réponse qui ressemble au passage sort dès ses premiers mots ; une
    /// autre est retenue, lue jusqu'à la longueur maximale plus
    /// [`REASONING_CHARS`].
    #[test]
    fn an_answer_like_the_text_streams_at_once() {
        let mut visible = Visible::new("I recieve teh letter yesterday.");
        assert_eq!(visible.push("I received"), "");
        assert_eq!(visible.push(" the le"), "I received the le");
        assert_eq!(visible.push("tter."), "tter.");
        // Un mot suffit pour un passage d'un mot.
        assert_eq!(Visible::new("Hi").push("Hi! "), "Hi! ");

        let mut visible = Visible::new("Hi");
        let poem = "Waves roll in under a silver sky. ";
        let mut read = 0;
        while !visible.over(100) {
            assert_eq!(visible.push(poem), "");
            read += poem.chars().count();
        }
        assert!(read > 100 + REASONING_CHARS, "{read}");
        assert!(visible.finish().starts_with("Waves roll in"));
    }
}
