//! Découpage d'un texte en passages (voir [`split`]).
//!
//! Un passage par paragraphe (lignes entre deux lignes vides), d'au plus
//! [`PART_CHARS`] caractères : un paragraphe trop long est coupé entre ses
//! lignes, une ligne trop longue à ses fins de phrases (lignes et phrases
//! regroupées tant qu'elles tiennent), une phrase trop longue à l'espace la
//! plus proche de la limite (à la limite même sans espace). Les espaces et
//! sauts de ligne qui bordent chaque passage vont dans `before` / `after` :
//! ils ne sont jamais envoyés au modèle et reviennent tels quels.
//!
//! Écart au contrat (paragraphes regroupés tant qu'ils tiennent) : les
//! modèles de 2 milliards de paramètres et moins fusionnaient les paragraphes
//! d'un passage (lignes vides perdues, 4 modèles sur 11, contrat § 6) ; un
//! passage par paragraphe n'a plus de ligne vide à perdre, pour un coût
//! faible (consignes et exemples, communs, restent en cache chez Ollama).

use super::ProofreadPart;

/// Taille maximale d'un passage, en caractères.
pub const PART_CHARS: usize = 1200;

/// Morceau du texte : positions (octets) de début et de fin.
type Span = (usize, usize);

/// Niveaux de coupe d'un paragraphe trop long, du plus grossier au plus
/// fin.
#[derive(Clone, Copy)]
enum Level {
    Line,
    Sentence,
    Word,
}

/// Passages de `text` ; aucun pour un texte vide ou fait seulement
/// d'espaces. Les `before` + `text` + `after`, dans l'ordre, redonnent
/// exactement `text`.
#[cfg(test)]
pub fn split(text: &str) -> Vec<ProofreadPart> {
    marked(text).into_iter().map(|(part, _)| part).collect()
}

/// Passages de `text` (voir [`split`]), chacun avec `true` s'il est un bloc
/// de code délimité (```, ~~~) : jamais envoyé au modèle, il garde son texte
/// (le code ne se corrige pas, et le garde-fou laisserait passer des
/// identifiants « corrigés »).
pub fn marked(text: &str) -> Vec<(ProofreadPart, bool)> {
    let Some(whole) = trimmed(text, (0, text.len())) else {
        return Vec::new();
    };
    let spans: Vec<(Span, bool)> = paragraphs(text, whole)
        .into_iter()
        .flat_map(|(paragraph, code)| {
            pieces(text, paragraph, Level::Line)
                .into_iter()
                .map(move |piece| (piece, code))
        })
        .collect();
    let mut parts: Vec<(ProofreadPart, bool)> = Vec::with_capacity(spans.len());
    let mut cursor = 0;
    for ((start, end), code) in spans {
        // Entre deux passages : jusqu'au dernier saut de ligne pour le
        // précédent, le retrait de la ligne suivante pour celui-ci.
        let gap = &text[cursor..start];
        let cut = match parts.last_mut() {
            Some((previous, _)) => {
                let cut = gap.rfind('\n').map_or(gap.len(), |at| at + 1);
                previous.after = gap[..cut].to_owned();
                cut
            }
            None => 0,
        };
        let part = ProofreadPart {
            before: gap[cut..].to_owned(),
            text: text[start..end].to_owned(),
            after: String::new(),
        };
        parts.push((part, code));
        cursor = end;
    }
    if let Some((last, _)) = parts.last_mut() {
        last.after = text[cursor..].to_owned();
    }
    parts
}

fn chars(text: &str, (start, end): Span) -> usize {
    text[start..end].chars().count()
}

/// `span` sans les espaces qui le bordent ; `None` s'il n'y a que des
/// espaces.
fn trimmed(text: &str, (start, end): Span) -> Option<Span> {
    let slice = &text[start..end];
    let inner = slice.trim();
    if inner.is_empty() {
        return None;
    }
    let lead = slice.len() - slice.trim_start().len();
    Some((start + lead, start + lead + inner.len()))
}

/// Morceaux de `span` (déjà sans espaces autour) d'au plus [`PART_CHARS`]
/// caractères : unités de ce niveau regroupées tant qu'elles tiennent, une
/// unité trop longue coupée au niveau suivant.
fn pieces(text: &str, span: Span, level: Level) -> Vec<Span> {
    if chars(text, span) <= PART_CHARS {
        return vec![span];
    }
    let (units, next) = match level {
        Level::Line => (cut_at(text, span, line_ends(text, span)), Level::Sentence),
        Level::Sentence => (cut_at(text, span, sentence_ends(text, span)), Level::Word),
        Level::Word => return words(text, span),
    };
    if units.len() <= 1 {
        return pieces(text, span, next);
    }
    let mut result = Vec::new();
    let mut group: Option<Span> = None;
    for unit in units {
        if chars(text, unit) > PART_CHARS {
            result.extend(group.take());
            result.extend(pieces(text, unit, next));
            continue;
        }
        group = match group {
            Some((start, _)) if chars(text, (start, unit.1)) <= PART_CHARS => Some((start, unit.1)),
            Some(full) => {
                result.push(full);
                Some(unit)
            }
            None => Some(unit),
        };
    }
    result.extend(group);
    result
}

/// Paragraphes : suites de lignes non vides ; un bloc de code délimité
/// (```, ~~~) est un paragraphe à part (`true`), d'un seul tenant (ses
/// lignes vides ne séparent rien), même sans ligne vide autour. Un bloc
/// jamais fermé va jusqu'à la fin, comme en Markdown.
fn paragraphs(text: &str, span: Span) -> Vec<(Span, bool)> {
    let mut result = Vec::new();
    let mut current: Option<Span> = None;
    let mut fence: Option<(char, usize)> = None;
    let mut offset = span.0;
    let flush = |current: &mut Option<Span>, result: &mut Vec<(Span, bool)>, code: bool| {
        if let Some(paragraph) = current
            .take()
            .and_then(|paragraph| trimmed(text, paragraph))
        {
            result.push((paragraph, code));
        }
    };
    for line in text[span.0..span.1].split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let extended = Some((current.map_or(start, |(first, _)| first), offset));
        match fence {
            Some((mark, length)) => {
                current = extended;
                if closes_fence(line, mark, length) {
                    flush(&mut current, &mut result, true);
                    fence = None;
                }
            }
            None => match opens_fence(line) {
                Some(opening) => {
                    flush(&mut current, &mut result, false);
                    fence = Some(opening);
                    current = Some((start, offset));
                }
                None if line.trim().is_empty() => flush(&mut current, &mut result, false),
                None => current = extended,
            },
        }
    }
    flush(&mut current, &mut result, fence.is_some());
    result
}

/// Ligne qui ouvre un bloc de code : au moins trois « ` » ou « ~ » (après
/// le retrait), sans autre « ` » ensuite pour des « ` » (« ```x``` » est du
/// code en ligne) ; caractère et longueur de la clôture attendue.
fn opens_fence(line: &str) -> Option<(char, usize)> {
    let marker = line.trim_start();
    let mark = marker.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let length = marker.chars().take_while(|&c| c == mark).count();
    let info = &marker[length..];
    (length >= 3 && !(mark == '`' && info.contains('`'))).then_some((mark, length))
}

/// Ligne qui ferme le bloc ouvert par `length` fois `mark`.
fn closes_fence(line: &str, mark: char, length: usize) -> bool {
    let marker = line.trim();
    marker.chars().count() >= length && marker.chars().all(|c| c == mark)
}

/// Fins de ligne de `span` (après chaque saut de ligne).
fn line_ends(text: &str, (start, end): Span) -> Vec<usize> {
    text[start..end]
        .match_indices('\n')
        .map(|(at, _)| start + at + 1)
        .collect()
}

/// Ponctuation qui finit une phrase.
fn ends_sentence(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…')
}

/// Guillemets et parenthèses qui peuvent suivre la fin d'une phrase.
fn closes(c: char) -> bool {
    matches!(c, '"' | '\'' | '”' | '’' | '»' | ')' | ']')
}

/// Espace où l'on peut couper : pas une espace insécable (« Bonjour ! »).
fn breaks(c: char) -> bool {
    c.is_whitespace() && !matches!(c, '\u{a0}' | '\u{202f}' | '\u{2007}')
}

/// Fins de phrases de `span` : après `.`, `!`, `?` ou `…` (et les guillemets
/// ou parenthèses qui les suivent, même après une espace insécable, comme
/// dans « oui. »), devant une espace sécable.
fn sentence_ends(text: &str, (start, end): Span) -> Vec<usize> {
    let chars: Vec<(usize, char)> = text[start..end].char_indices().collect();
    let mut result = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if !ends_sentence(chars[index].1) {
            index += 1;
            continue;
        }
        index += 1;
        loop {
            match chars.get(index) {
                Some(&(_, c)) if ends_sentence(c) || closes(c) => index += 1,
                Some(&(_, c))
                    if c.is_whitespace()
                        && !breaks(c)
                        && chars.get(index + 1).is_some_and(|&(_, next)| closes(next)) =>
                {
                    index += 2;
                }
                _ => break,
            }
        }
        if let Some(&(at, c)) = chars.get(index)
            && breaks(c)
        {
            result.push(start + at);
        }
    }
    result
}

/// Morceaux de `span` coupé aux positions `cuts` (croissantes), sans les
/// espaces qui les bordent, les morceaux vides écartés.
fn cut_at(text: &str, span: Span, cuts: Vec<usize>) -> Vec<Span> {
    let mut result = Vec::new();
    let mut start = span.0;
    for cut in cuts.into_iter().chain(std::iter::once(span.1)) {
        result.extend(trimmed(text, (start, cut)));
        start = cut;
    }
    result
}

/// Coupe une phrase trop longue : à la dernière espace qui laisse au plus
/// [`PART_CHARS`] caractères avant elle, sinon (un seul « mot » géant) à la
/// limite même, sans séparer un caractère de ses marques jointes.
fn words(text: &str, span: Span) -> Vec<Span> {
    let mut result = Vec::new();
    let mut rest = span;
    while chars(text, rest) > PART_CHARS {
        let slice = &text[rest.0..rest.1];
        let window: Vec<(usize, char)> = slice.char_indices().take(PART_CHARS + 1).collect();
        let space = window
            .iter()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map(|&(at, _)| at);
        let cut = match space {
            Some(at) if at > 0 => at,
            _ => hard_cut(&window),
        };
        // Morceau sans espaces autour : `rest` commence par un caractère
        // visible, et la coupe tombe sur une espace ou entre deux caractères
        // visibles.
        result.extend(trimmed(text, (rest.0, rest.0 + cut)));
        match trimmed(text, (rest.0 + cut, rest.1)) {
            Some(next) => rest = next,
            None => return result,
        }
    }
    result.push(rest);
    result
}

/// Recul au plus pour ne pas séparer un caractère de ce qui s'y joint : une
/// suite plus longue (texte « zalgo », marques combinantes empilées) est
/// coupée à la limite même, plutôt qu'en passages d'un caractère.
const MAX_BACKOFF: usize = 32;

/// Position de la coupe d'un morceau sans espace : à [`PART_CHARS`]
/// caractères, reculée tant qu'elle séparerait un caractère de ce qui s'y
/// joint (accent combinant, sélecteur de variante, couleur de peau, liaison
/// d'emoji), de [`MAX_BACKOFF`] caractères au plus ; jamais entre les deux
/// indicateurs régionaux d'un drapeau.
fn hard_cut(window: &[(usize, char)]) -> usize {
    let limit = PART_CHARS.min(window.len() - 1);
    let mut index = limit;
    while index > 1 && (joins(window[index].1) || window[index - 1].1 == '\u{200d}') {
        index -= 1;
    }
    if limit - index > MAX_BACKOFF {
        index = limit;
    }
    // Drapeau : indicateurs régionaux pris deux à deux depuis le début de
    // leur suite ; une coupe après un nombre impair d'entre eux en séparerait
    // deux.
    let regional = window[..index]
        .iter()
        .rev()
        .take_while(|&&(_, c)| is_regional(c))
        .count();
    if regional % 2 == 1 && is_regional(window[index].1) && index > 1 {
        index -= 1;
    }
    window[index].0
}

/// Indicateur régional (lettre d'un drapeau : 🇫, 🇷).
fn is_regional(c: char) -> bool {
    matches!(c, '\u{1f1e6}'..='\u{1f1ff}')
}

fn joins(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036f}'
        | '\u{1ab0}'..='\u{1aff}'
        | '\u{20d0}'..='\u{20ff}'
        | '\u{fe00}'..='\u{fe0f}'
        | '\u{200d}'
        | '\u{1f3fb}'..='\u{1f3ff}'
        | '\u{e0020}'..='\u{e007f}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(parts: &[ProofreadPart]) -> Vec<&str> {
        parts.iter().map(|part| part.text.as_str()).collect()
    }

    fn joined(parts: &[ProofreadPart]) -> String {
        parts
            .iter()
            .map(|part| format!("{}{}{}", part.before, part.text, part.after))
            .collect()
    }

    /// Ce que tout découpage garantit : texte reconstitué à l'identique,
    /// passages non vides, sans espaces autour, d'au plus `PART_CHARS`
    /// caractères ; les bords ne sont faits que d'espaces.
    fn check(text: &str) -> Vec<ProofreadPart> {
        let parts = split(text);
        if text.trim().is_empty() {
            assert!(parts.is_empty(), "{text:?}");
            return parts;
        }
        assert_eq!(joined(&parts), text);
        for part in &parts {
            assert!(!part.text.is_empty());
            assert_eq!(part.text.trim(), part.text, "{:?}", part.text);
            assert!(part.text.chars().count() <= PART_CHARS);
            assert!(part.before.trim().is_empty() && part.after.trim().is_empty());
        }
        parts
    }

    #[test]
    fn blank_texts_have_no_parts() {
        for text in ["", " ", "\n\n", " \t\r\n\u{a0} "] {
            assert!(split(text).is_empty(), "{text:?}");
        }
    }

    #[test]
    fn a_short_text_is_one_part_with_its_borders_aside() {
        let parts = check("\n  Bonjour,\nça va ?  \r\n");
        assert_eq!(
            parts,
            [ProofreadPart {
                before: "\n  ".into(),
                text: "Bonjour,\nça va ?".into(),
                after: "  \r\n".into(),
            }]
        );
    }

    #[test]
    fn each_paragraph_is_a_part() {
        let parts = check("Bonjour,\n\nMerci pour ton message.\r\n \r\nÀ demain,\nJean\n");
        assert_eq!(
            texts(&parts),
            ["Bonjour,", "Merci pour ton message.", "À demain,\nJean"]
        );
        // Le séparateur jusqu'au dernier saut de ligne va au passage
        // précédent.
        assert_eq!(parts[0].after, "\n\n");
        assert_eq!(parts[1].before, "");
        assert_eq!(parts[1].after, "\r\n \r\n");
        assert_eq!(parts[2].after, "\n");
    }

    #[test]
    fn indentation_goes_with_the_next_part() {
        let parts = check("Liste :\n\n    - un\n    - deux");
        assert_eq!(parts[0].after, "\n\n");
        assert_eq!(parts[1].before, "    ");
        assert_eq!(parts[1].text, "- un\n    - deux");
    }

    #[test]
    fn a_long_paragraph_is_cut_at_lines_then_sentences() {
        // Liste sans ligne vide, plus longue qu'un passage : coupée entre
        // deux éléments.
        let item = |n: usize| format!("- élément {n} : {}", "mot ".repeat(20).trim_end());
        let list: Vec<String> = (0..20).map(item).collect();
        let parts = check(&list.join("\n"));
        assert!(parts.len() >= 2);
        for part in &parts {
            assert!(part.text.starts_with("- élément"), "{:?}", part.text);
            assert!(part.after.is_empty() || part.after == "\n");
        }

        // Une seule ligne de phrases : coupée après une fin de phrase, les
        // phrases regroupées.
        let sentence = |n: usize| format!("Phrase {n} « citée » {}!", "blabla ".repeat(15));
        let line: Vec<String> = (0..20).map(sentence).collect();
        let parts = check(&line.join(" "));
        assert!(parts.len() >= 2);
        for part in &parts {
            assert!(part.text.starts_with("Phrase "), "{:?}", part.text);
            assert!(part.text.ends_with('!'), "{:?}", part.text);
        }
        // Phrases regroupées : chaque passage (sauf le dernier) est plein.
        for part in &parts[..parts.len() - 1] {
            assert!(part.text.chars().count() > PART_CHARS - 200);
        }
    }

    #[test]
    fn sentences_end_after_closing_quotes_but_not_at_a_no_break_space() {
        let text = "Il a dit «\u{a0}oui.\u{a0}» Puis\u{a0}: «\u{a0}non\u{a0}!\u{a0}» Fin. \
                    Encore?! Oui.\" Et (voilà.) 3.5 kg… Fin";
        let ends = sentence_ends(text, (0, text.len()));
        let sentences: Vec<&str> = cut_at(text, (0, text.len()), ends)
            .into_iter()
            .map(|(start, end)| &text[start..end])
            .collect();
        assert_eq!(
            sentences,
            [
                "Il a dit «\u{a0}oui.\u{a0}»",
                "Puis\u{a0}: «\u{a0}non\u{a0}!\u{a0}»",
                "Fin.",
                "Encore?!",
                "Oui.\"",
                "Et (voilà.)",
                "3.5 kg…",
                "Fin",
            ]
        );
    }

    #[test]
    fn a_long_sentence_is_cut_at_a_space_near_the_limit() {
        // Décalée de deux caractères : une coupe à la limite même tomberait
        // au milieu d'un mot.
        let text = format!("xy{}", "mot ".repeat(1000));
        let parts = check(&text);
        assert!(parts.len() >= 4);
        for part in &parts[..parts.len() - 1] {
            assert!(part.text.chars().count() > PART_CHARS - 4);
            assert_eq!(part.after, " ");
        }
        for part in &parts {
            assert!(
                part.text
                    .split(' ')
                    .all(|word| word == "mot" || word == "xymot"),
                "{:?}",
                part.text
            );
        }
    }

    #[test]
    fn a_giant_word_is_cut_at_the_limit_without_breaking_a_character() {
        let parts = check(&"a".repeat(3000));
        assert_eq!(
            parts.iter().map(|p| p.text.len()).collect::<Vec<_>>(),
            [1200, 1200, 600]
        );
        // Famille (emoji lié par U+200D) : jamais coupée au milieu.
        let family = "👨\u{200d}👩\u{200d}👧";
        let parts = check(&format!("{}{}", "a".repeat(1198), family.repeat(3)));
        assert!(parts[1].text.starts_with('👨'), "{:?}", parts[1].text);
        let accented = format!("{}e\u{301}e\u{301}", "a".repeat(1199));
        let parts = check(&accented);
        assert!(parts[1].text.starts_with('e'));
    }

    /// Relecture du 6 oct. : un drapeau (deux indicateurs régionaux) n'est
    /// jamais coupé en deux ; une longue suite de marques jointes est coupée
    /// à la limite plutôt qu'en passages d'un caractère.
    #[test]
    fn flags_stay_whole_and_mark_storms_are_cut_at_the_limit() {
        for prefix in ["", "a", "ab"] {
            let parts = check(&format!("{prefix}{}", "🇫🇷".repeat(700)));
            for part in &parts {
                let regional = part.text.chars().filter(|&c| is_regional(c)).count();
                assert_eq!(regional % 2, 0, "{prefix:?} {}", part.text.len());
            }
        }
        let parts = check(&format!("e{}", "\u{301}".repeat(3000)));
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn a_fenced_code_block_stays_whole() {
        let code = "```rust\nfn main() {\n\n    println!(\"x\");\n}\n```";
        let parts = check(&format!("Code :\n\n{code}\n\nFin."));
        assert_eq!(texts(&parts), ["Code :", code, "Fin."]);
    }

    /// Bloc de code (relecture du 6 oct.) : passage à part, marqué (jamais
    /// envoyé), même collé au texte ; fermé par une clôture de même sorte, ou
    /// jamais (jusqu'à la fin).
    #[test]
    fn code_blocks_are_parts_of_their_own() {
        let marks = |text: &str| -> Vec<(String, bool)> {
            check(text);
            marked(text)
                .into_iter()
                .map(|(part, code)| (part.text, code))
                .collect()
        };
        let code = "```\nlet teh_value = recieve();\n\n~~~\n```";
        assert_eq!(
            marks(&format!("Voici :\n{code}\nFin.")),
            [
                ("Voici :".into(), false),
                (code.into(), true),
                ("Fin.".into(), false)
            ]
        );
        assert_eq!(
            marks("Texte\n  ~~~~\ncode\n~~~"),
            [("Texte".into(), false), ("~~~~\ncode\n~~~".into(), true)]
        );
        // Code en ligne : pas un bloc.
        assert_eq!(
            marks("```x``` est court\nfin"),
            [("```x``` est court\nfin".into(), false)]
        );
        // Long bloc : coupé, toujours marqué.
        let long = format!("```\n{}```", "let x = 1;\n".repeat(200));
        let parts = marked(&long);
        assert!(parts.len() >= 2 && parts.iter().all(|(_, code)| *code));
    }

    /// Générateur pseudo-aléatoire déterministe (xorshift).
    struct Random(u64);

    impl Random {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    /// Propriété : pour des textes faits au hasard de mots, espaces (dont
    /// insécables), sauts de ligne LF et CRLF, lignes vides, emojis, listes
    /// Markdown, blocs de code et ponctuation, le découpage redonne le texte.
    #[test]
    fn any_text_is_rebuilt_exactly() {
        const PIECES: &[&str] = &[
            "mot",
            "Phrase.",
            "fin ?",
            "Oui !",
            "…",
            " ",
            "  ",
            "\t",
            "\u{a0}",
            "\u{202f}",
            "\n",
            "\r\n",
            "\n\n",
            "\r\n\r\n",
            "\n \n",
            "- ",
            "1. ",
            "```",
            "~~~",
            "👨\u{200d}👩\u{200d}👧",
            "🙂",
            "e\u{301}",
            "« guillemets »",
            "\"cité.\"",
            "(parenthèse.)",
            "ça",
        ];
        let mut random = Random(0x5eed_1234_abcd_ef01);
        for round in 0..400 {
            let count = 1 + random.below(if round % 10 == 0 { 3000 } else { 400 });
            let mut text = String::new();
            for _ in 0..count {
                // Parfois un long mot, pour les coupes à la limite.
                if random.below(200) == 0 {
                    text.push_str(&"z".repeat(random.below(2500)));
                } else {
                    text.push_str(PIECES[random.below(PIECES.len())]);
                }
            }
            check(&text);
        }
    }
}
