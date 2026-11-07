//! Retours à la ligne souples rendus à une correction.
//!
//! Un paragraphe coupé à la main (README, courriel, texte copié d'un
//! terminal) est souvent rendu corrigé sur une seule ligne : le garde-fou,
//! qui exige les mêmes sauts de ligne en correction, l'écartait, et le
//! passage restait fautif. Les sauts perdus sont remis aux mêmes endroits
//! quand ils sont souples (à l'intérieur d'un paragraphe : ligne non vide de
//! chaque côté, la ligne suivante ne commence pas une structure Markdown), à
//! la place de l'espace qui suit le mot de la réponse aligné sur le dernier
//! mot de la ligne d'origine, avec le retrait de la ligne d'origine. Le
//! garde-fou juge ensuite ce texte, inchangé. Un saut dur perdu, un saut
//! impossible à placer ou un saut de la réponse ailleurs qu'à une fin de
//! ligne du passage : rien n'est remis, la réponse est écartée comme avant.

use super::guard::{self, is_break};

/// Fin de ligne : positions (octets, « \r\n » d'un tenant) et texte.
struct Break<'a> {
    start: usize,
    end: usize,
    text: &'a str,
}

/// Fins de ligne du texte, dans l'ordre.
fn breaks(text: &str) -> Vec<Break<'_>> {
    text.char_indices()
        .filter(|&(_, c)| is_break(c))
        .map(|(at, c)| {
            let start = if c == '\n' && text[..at].ends_with('\r') {
                at - 1
            } else {
                at
            };
            let end = at + c.len_utf8();
            Break {
                start,
                end,
                text: &text[start..end],
            }
        })
        .collect()
}

/// Réponse avec les retours à la ligne souples du passage qu'elle a perdus,
/// et les fins de ligne du passage une à une (CRLF rendus) ; `None` si elle
/// n'en a pas perdu ou s'ils ne peuvent pas être remis.
pub fn restore(original: &str, answer: &str) -> Option<String> {
    let (ours, theirs) = (breaks(original), breaks(answer));
    if theirs.len() >= ours.len() {
        return None;
    }
    let (sent, received) = (guard::spans(original), guard::spans(answer));
    let pairs = guard::pairs(
        &sent.iter().map(|(_, w)| w.clone()).collect::<Vec<_>>(),
        &received.iter().map(|(_, w)| w.clone()).collect::<Vec<_>>(),
    );
    // Sauts de la réponse retrouvés à une fin de ligne du passage, et
    // espaces de la réponse à remplacer par un saut (début, fin, retrait).
    let mut found = 0;
    let mut edits: Vec<(usize, usize, &str)> = Vec::new();
    for (index, at) in ours.iter().enumerate() {
        let start = index
            .checked_sub(1)
            .map_or(0, |previous| ours[previous].end);
        let end = ours
            .get(index + 1)
            .map_or(original.len(), |next| next.start);
        let (line, next) = (&original[start..at.start], &original[at.end..end]);
        // Dernier mot de la ligne et son pendant dans la réponse.
        let last = sent.iter().rposition(|((_, stop), _)| *stop <= at.start)?;
        if sent[last].0.0 < start {
            return None;
        }
        let word = pairs[last]?;
        let gap = (
            received[word].0.1,
            received
                .get(word + 1)
                .map_or(answer.len(), |((from, _), _)| *from),
        );
        if answer[gap.0..gap.1].contains(is_break) {
            found += 1;
            continue;
        }
        if !soft(at.text, line, next) {
            return None;
        }
        let (from, to) = place(&answer[gap.0..gap.1], lead(next))?;
        let (from, to) = (gap.0 + from, gap.0 + to);
        // La ligne recréée ne doit pas devenir une structure (« mot - suite »
        // coupé avant le tiret ferait une liste).
        if starts_block(answer[to..].split(is_break).next().unwrap_or_default()) {
            return None;
        }
        edits.push((from, to, indent(next)));
    }
    // Un saut de la réponse ailleurs qu'à une fin de ligne du passage.
    if found != theirs.len() {
        return None;
    }
    let mut text = String::with_capacity(answer.len() + edits.len() * 2);
    let mut cursor = 0;
    for (from, to, indent) in edits {
        text.push_str(&answer[cursor..from]);
        text.push('\n');
        text.push_str(indent);
        cursor = to;
    }
    text.push_str(&answer[cursor..]);
    // Fins de ligne du passage, une à une (autant par construction).
    let placed = breaks(&text);
    if placed.len() != ours.len() {
        return None;
    }
    let mut out = String::with_capacity(text.len() + ours.len());
    let mut cursor = 0;
    for (at, theirs) in placed.iter().zip(&ours) {
        out.push_str(&text[cursor..at.start]);
        out.push_str(theirs.text);
        cursor = at.end;
    }
    out.push_str(&text[cursor..]);
    Some(out)
}

/// Texte dont les retours à la ligne souples (voir [`soft`]) sont joints :
/// la fin de ligne, les espaces qui la précèdent et le retrait de la ligne
/// suivante deviennent une espace (option « mise en forme automatique » de
/// la correction). Les autres sauts (paragraphes, listes, titres, tableaux,
/// code, sauts forcés) restent.
pub fn join_soft(text: &str) -> String {
    let all = breaks(text);
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (index, at) in all.iter().enumerate() {
        let start = index.checked_sub(1).map_or(0, |previous| all[previous].end);
        let end = all.get(index + 1).map_or(text.len(), |next| next.start);
        let (line, next) = (&text[start..at.start], &text[at.end..end]);
        if !soft(at.text, line, next) {
            continue;
        }
        // Jamais avant `cursor` : la ligne a pu commencer par un retrait
        // déjà retiré.
        let from = (start + line.trim_end_matches([' ', '\t']).len()).max(cursor);
        out.push_str(&text[cursor..from]);
        out.push(' ');
        cursor = at.end + indent(next).len();
    }
    out.push_str(&text[cursor..]);
    out
}

/// Retour à la ligne souple : « \n » ou « \r\n » (pas U+2028 ni U+2029,
/// sauts voulus dans Word ou LibreOffice) entre deux lignes non vides, sans
/// saut forcé Markdown (deux espaces ou « \ » en fin de ligne), après une
/// ligne qui n'est ni un titre ni une ligne de tableau, avant une ligne qui
/// ne commence pas une structure.
fn soft(text: &str, line: &str, next: &str) -> bool {
    let head = line.trim_start();
    matches!(text, "\n" | "\r\n")
        && !line.trim().is_empty()
        && !next.trim().is_empty()
        && !line.ends_with("  ")
        && !line.ends_with('\\')
        && !head.starts_with(['#', '|'])
        && !starts_block(next)
}

/// La ligne commence une structure Markdown : retrait de code (tabulation
/// ou quatre espaces), titre, citation, ligne de tableau, balise HTML, bloc
/// de code, élément de liste (« - », « * », « + », « 1. », « 1) »), filet
/// ou soulignement de titre (« --- », « === », « *** »), définition de lien
/// (« [x]: … »).
pub(super) fn starts_block(line: &str) -> bool {
    let body = line.trim_start_matches([' ', '\t']);
    let indent = &line[..line.len() - body.len()];
    if indent.contains('\t') || indent.len() >= 4 {
        return true;
    }
    let after_marker = |rest: &str| rest.is_empty() || rest.starts_with([' ', '\t']);
    let bullet = body.strip_prefix(['-', '*', '+']).is_some_and(after_marker);
    let digits = body.chars().take_while(char::is_ascii_digit).count();
    let numbered = (1..=9).contains(&digits)
        && body[digits..]
            .strip_prefix(['.', ')'])
            .is_some_and(after_marker);
    let rule = !body.trim().is_empty()
        && body
            .trim_end()
            .chars()
            .all(|c| matches!(c, '-' | '=' | '*' | '_' | ' ' | '\t'));
    body.starts_with(['#', '>', '|', '<'])
        || body.starts_with("```")
        || body.starts_with("~~~")
        || bullet
        || numbered
        || rule
        || (body.starts_with('[') && body.contains("]:"))
}

/// Retrait de la ligne (espaces et tabulations de début).
fn indent(line: &str) -> &str {
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// Ce qui précède le premier mot de la ligne, après son retrait
/// (« ( », « — ») ; toute la ligne si elle n'a pas de mot.
fn lead(line: &str) -> &str {
    let body = &line[indent(line).len()..];
    &body[..body.find(guard::word_char).unwrap_or(body.len())]
}

/// Espaces de `gap` (entre deux mots de la réponse) à remplacer par le saut :
/// celles que suit exactement le début de la ligne d'origine (`lead`), sinon
/// les seules du morceau. Jamais une espace insécable (« mot : » en
/// français).
fn place(gap: &str, lead: &str) -> Option<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (at, c) in gap.char_indices().filter(|&(_, c)| matches!(c, ' ' | '\t')) {
        match runs.last_mut() {
            Some(run) if run.1 == at => run.1 = at + c.len_utf8(),
            _ => runs.push((at, at + c.len_utf8())),
        }
    }
    runs.iter()
        .find(|&&(_, end)| &gap[end..] == lead)
        .or_else(|| (runs.len() == 1).then(|| &runs[0]))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proofread::ProofreadMode;
    use crate::proofread::guard::{accept, settle};

    const CORRECT: ProofreadMode = ProofreadMode::Correct;

    /// Recette du 1er nov. 2026 (défaut R14) : paragraphe d'un README coupé
    /// à la main, réponse réelle de qwen3:8b (trois fois la même) rendue sur
    /// une ligne, écartée jusqu'ici (« Some parts were left unchanged »).
    #[test]
    fn a_wrapped_readme_paragraph_is_corrected() {
        let original = "A tiny shop backend. It calcualte the cart total, retry the payement\n\
                        when the bank dont answer, and turn product titles into URL slugs.";
        let answer = "A tiny shop backend. It calculates the cart total, retry the payment when \
                      the bank doesn't answer, and turn product titles into URL slugs.";
        let expected = "A tiny shop backend. It calculates the cart total, retry the payment\n\
                        when the bank doesn't answer, and turn product titles into URL slugs.";
        assert!(!accept(CORRECT, original, answer));
        assert_eq!(restore(original, answer).as_deref(), Some(expected));
        assert_eq!(
            settle(CORRECT, original, answer.into()).as_deref(),
            Some(expected)
        );
    }

    /// Réponse réelle de qwen3:8b (Ollama de test, 1er nov.) : trois lignes
    /// d'un courriel, deux sauts souples perdus.
    #[test]
    fn a_wrapped_french_mail_is_corrected() {
        let original = "Bonjour à tous, je vous écrit pour vous dire que la réunion de jeudi\n\
                        est reporté a la semaine prochaine, car plusieur collègues seront en\n\
                        déplacement. Merci de votre compréhention.";
        let answer = "Bonjour à tous, je vous écris pour vous dire que la réunion de jeudi est \
                      reportée à la semaine prochaine, car plusieurs collègues seront en \
                      déplacement. Merci pour votre compréhension.";
        assert_eq!(
            settle(CORRECT, original, answer.into()).as_deref(),
            Some(
                "Bonjour à tous, je vous écris pour vous dire que la réunion de jeudi\n\
                 est reportée à la semaine prochaine, car plusieurs collègues seront en\n\
                 déplacement. Merci pour votre compréhension."
            )
        );
        // Un seul des deux perdu : remis, l'autre gardé.
        let half = "Bonjour à tous, je vous écris pour vous dire que la réunion de jeudi\n\
                    est reportée à la semaine prochaine, car plusieurs collègues seront en \
                    déplacement. Merci pour votre compréhension.";
        assert_eq!(
            restore(original, half).as_deref(),
            settle(CORRECT, original, answer.into()).as_deref()
        );
        // Ponctuation à la coupure, suite d'un élément de liste en retrait,
        // ligne qui commence par une parenthèse.
        assert_eq!(
            restore(
                "We has reviewed the proposal.\nHowever, the budget section need more details.",
                "We have reviewed the proposal. However, the budget section needs more details."
            )
            .as_deref(),
            Some("We have reviewed the proposal.\nHowever, the budget section needs more details.")
        );
        assert_eq!(
            restore(
                "- acheter du pain et\n  apeler le plombier",
                "- acheter du pain et appeler le plombier"
            )
            .as_deref(),
            Some("- acheter du pain et\n  appeler le plombier")
        );
        assert_eq!(
            restore(
                "voir le raport\n(page 3) demain",
                "Voir le rapport (page 3) demain."
            )
            .as_deref(),
            Some("Voir le rapport\n(page 3) demain.")
        );
    }

    /// Réponse qui coupe ailleurs, ou qui perd un saut dur : rien n'est
    /// remis, la réponse reste écartée.
    #[test]
    fn breaks_moved_or_hard_are_not_restored() {
        // Coupée ailleurs : son saut ne tombe pas à une fin de ligne du
        // passage.
        let original = "the cat sat on teh mat\nand the dog slept on\nthe sofa all day long";
        let moved = "The cat sat on the mat and\nthe dog slept on the sofa all day long.";
        assert_eq!(restore(original, moved), None);
        assert_eq!(settle(CORRECT, original, moved.into()), None);
        // Saut dur : avant une liste, un titre, une citation, une liste
        // numérotée, un filet, un retrait de code, après un titre ou un saut
        // forcé.
        for (original, answer) in [
            (
                "Pour demain il faut\n- acheter du pain",
                "Pour demain il faut - acheter du pain",
            ),
            ("Voici la suite\n# Titre", "Voici la suite # Titre"),
            ("Il a dit\n> bonjour a tous", "Il a dit > bonjour à tous"),
            ("Les étapes sont\n1. ouvrir", "Les étapes sont 1. ouvrir"),
            ("Un titre\n---", "Un titre ---"),
            ("Lancer ceci\n    npm test", "Lancer ceci npm test"),
            ("# Titre\nUn texte ici", "# Titre Un texte ici"),
            (
                "Ligne forcée  \nsuite du texte",
                "Ligne forcée suite du texte",
            ),
            (
                "Ligne forcée\\\nsuite du texte",
                "Ligne forcée\\ suite du texte",
            ),
            // Séparateur de ligne de Word : voulu.
            ("Bonjour,\u{2028}je suis la", "Bonjour, je suis là"),
        ] {
            assert_eq!(restore(original, answer), None, "{original:?}");
            assert_eq!(
                settle(CORRECT, original, answer.into()),
                None,
                "{original:?}"
            );
        }
        // Une coupure qui ferait un titre dans la réponse (seule espace du
        // morceau, avant le « # » ajouté).
        assert_eq!(restore("voir le\nsujet ici", "Voir le #sujet ici."), None);
        // Tiret ajouté à la coupure : le saut va après lui, là où commence
        // la ligne d'origine (pas de liste créée).
        assert_eq!(
            restore("un mot\nsuite ici", "Un mot - suite ici.").as_deref(),
            Some("Un mot -\nsuite ici.")
        );
        // Dernier mot de la ligne perdu par la réponse : saut impossible à
        // placer.
        assert_eq!(
            restore(
                "il fait beau aujourdhui\net demain",
                "Il fait beau et demain."
            ),
            None
        );
        // Rien de perdu : rien à faire.
        assert_eq!(restore("a b\nc d", "A b\nc d."), None);
    }

    /// Fins de ligne du passage rendues une à une : CRLF, et sorte mêlée.
    #[test]
    fn line_endings_follow_the_original() {
        assert_eq!(
            settle(
                CORRECT,
                "the first line has a eror\r\nand the second one to.",
                "The first line has an error and the second one too.".into()
            )
            .as_deref(),
            Some("The first line has an error\r\nand the second one too.")
        );
        // Sauts déjà rendus par la réponse : en « \n » (sortes mêlées, laissées
        // telles quelles par le nettoyage), remis comme dans le passage.
        assert_eq!(
            restore(
                "first line here\r\nsecond line here\nthird line here",
                "First line here\nsecond line here third line here."
            )
            .as_deref(),
            Some("First line here\r\nsecond line here\nthird line here.")
        );
    }

    /// La reformulation n'est pas touchée : elle ne garde pas les sauts de
    /// ligne, la réponse sur une ligne y est déjà acceptée telle quelle.
    #[test]
    fn rephrasing_is_left_alone() {
        let original = "the report is ready and i will send it\ntomorrow morning with the figures";
        let answer = "The report is ready; I will send it tomorrow morning along with the figures.";
        assert_eq!(
            settle(ProofreadMode::Rephrase, original, answer.into()).as_deref(),
            Some(answer)
        );
    }

    #[test]
    fn soft_line_breaks_are_joined_and_the_others_kept() {
        assert_eq!(
            join_soft("Un paragraphe coupé  \n   à la main,\r\nsur trois lignes."),
            "Un paragraphe coupé  \n   à la main, sur trois lignes."
        );
        assert_eq!(
            join_soft("Un paragraphe coupé\n   à la main,\r\nsur trois lignes."),
            "Un paragraphe coupé à la main, sur trois lignes."
        );
        // Listes, titres, paragraphes, sauts forcés, code : gardés.
        let kept = "# Titre\nTexte.\n\nÀ faire :\n- un\n- deux\n\nLigne forcée\\\nsuite\n    code";
        assert_eq!(join_soft(kept), kept);
        assert_eq!(join_soft("Sans saut."), "Sans saut.");
        assert_eq!(join_soft(""), "");
    }
}
