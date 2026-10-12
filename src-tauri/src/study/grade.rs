//! Correction des réponses, sans modèle.
//!
//! Choix et vrai/faux : égalité ; ordre : la suite exacte ; association :
//! toutes les paires (juste ou non élément par élément). Trou : texte
//! ramené à une forme commune (espaces, casse, apostrophes, guillemets,
//! ponctuation finale, article en tête) égal à une réponse acceptée ; nombres
//! comparés comme nombres (chiffres ou toutes lettres : « 5 », « five »,
//! « cinq », « cinco ») ; égal sans les accents, ou à une faute de frappe
//! près (jamais pour un nombre ni un texte qui contient un chiffre) : juste,
//! mais « proche » (l'interface montre l'orthographe attendue). La même
//! comparaison sert à l'auto-vérification des questions à trou.
//!
//! Cours de langue (`strict`) : l'article, l'accent et la terminaison sont
//! souvent l'objet même de la question ; ni article retiré, ni accent plié,
//! ni faute pardonnée : une réponse qui n'en diffère que par là est fausse,
//! mais « proche ».

use super::AnswerValue;
use super::data::StoredQuestion;
use crate::proofread::guard;

/// Résultat de la correction d'une réponse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Graded {
    pub correct: bool,
    /// Trou : à un accent ou une faute de frappe près (juste ailleurs que
    /// dans un cours de langue, faux dans un cours de langue).
    pub close: bool,
    /// Ordre et association : juste ou non, position par position.
    pub detail: Vec<bool>,
}

/// Corrige `given` ; `strict` : cours de langue. `None` si la réponse n'a
/// pas la forme de la question (autre type, indices hors limites, nombre
/// d'éléments différent).
pub fn grade(question: &StoredQuestion, given: &AnswerValue, strict: bool) -> Option<Graded> {
    let plain = |correct: bool| {
        Some(Graded {
            correct,
            close: false,
            detail: Vec::new(),
        })
    };
    match (&question.expected, given) {
        (AnswerValue::Choice { index: expected }, AnswerValue::Choice { index }) => {
            let options = match &question.body {
                super::QuestionBody::Choice { options, .. } => options.len(),
                _ => 0,
            };
            ((*index as usize) < options).then_some(())?;
            plain(index == expected)
        }
        (AnswerValue::TrueFalse { value: expected }, AnswerValue::TrueFalse { value }) => {
            plain(value == expected)
        }
        (AnswerValue::Blank { .. }, AnswerValue::Blank { text }) => {
            let accepted: Vec<&str> = question.accepted.iter().map(String::as_str).collect();
            let matched = match_blank(text, &accepted, strict);
            Some(Graded {
                correct: matches!(matched, Match::Exact | Match::Close),
                close: matches!(matched, Match::Close | Match::Near),
                detail: Vec::new(),
            })
        }
        (AnswerValue::Order { order: expected }, AnswerValue::Order { order }) => {
            // Une permutation des éléments montrés.
            let mut sorted = order.clone();
            sorted.sort_unstable();
            (sorted == (0..expected.len() as u32).collect::<Vec<_>>()).then_some(())?;
            Some(positions(expected, order))
        }
        (AnswerValue::Match { pairs: expected }, AnswerValue::Match { pairs }) => {
            (pairs.len() == expected.len()).then_some(())?;
            (pairs.iter().all(|pair| (*pair as usize) < expected.len())).then_some(())?;
            Some(positions(expected, pairs))
        }
        _ => None,
    }
}

fn positions(expected: &[u32], given: &[u32]) -> Graded {
    let detail: Vec<bool> = expected
        .iter()
        .zip(given)
        .map(|(expected, given)| expected == given)
        .collect();
    Graded {
        correct: detail.iter().all(|right| *right),
        close: false,
        detail,
    }
}

/// Accord d'un texte avec les réponses acceptées.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    No,
    /// Même texte (forme commune) ou même nombre.
    Exact,
    /// Juste à un accent ou une faute de frappe près.
    Close,
    /// Cours de langue : faux, mais à un accent ou une faute de frappe près.
    Near,
}

impl Match {
    fn rank(self) -> u8 {
        match self {
            Match::No => 0,
            Match::Near => 1,
            Match::Close => 2,
            Match::Exact => 3,
        }
    }
}

/// Meilleur accord de `text` avec une des réponses `accepted`.
pub fn match_blank(text: &str, accepted: &[&str], strict: bool) -> Match {
    accepted
        .iter()
        .map(|answer| match_one(text, answer, strict))
        .max_by_key(|matched| matched.rank())
        .unwrap_or(Match::No)
}

/// Même réponse pour la correction elle-même : même forme commune, ou même
/// nombre (chiffres ou toutes lettres).
pub fn same_answer(a: &str, b: &str, strict: bool) -> bool {
    match_one(a, b, strict) == Match::Exact
}

/// Faute de frappe permise pour une réponse attendue de `letters` lettres :
/// une dès 5, deux dès 10, aucune en dessous.
pub fn typo_allowance(letters: usize) -> usize {
    match letters {
        0..=4 => 0,
        5..=9 => 1,
        _ => 2,
    }
}

/// À une faute de frappe près selon la règle de l'apprenant (textes sans
/// chiffre, accents pliés).
pub fn typo_close(given: &str, expected: &str) -> bool {
    let (given, expected) = (folded(given), folded(expected));
    if given.iter().chain(&expected).any(char::is_ascii_digit) {
        return false;
    }
    let allowed = typo_allowance(expected.iter().filter(|c| c.is_alphanumeric()).count());
    allowed > 0
        && given.len().abs_diff(expected.len()) <= allowed
        && guard::distance(&given, &expected) <= allowed
}

fn match_one(text: &str, answer: &str, strict: bool) -> Match {
    let given = normalize_for(text, strict);
    let expected = normalize_for(answer, strict);
    if given.is_empty() || expected.is_empty() {
        return Match::No;
    }
    if given == expected {
        return Match::Exact;
    }
    // Nombres : en chiffres, ou un côté en chiffres et l'autre en toutes
    // lettres (« un » et « une » restent deux mots).
    let (digits_given, digits_expected) = (number(&given), number(&expected));
    let (a, b) = match (digits_given, digits_expected) {
        (Some(a), None) => (Some(a), number_word(&expected).map(f64::from)),
        (None, Some(b)) => (number_word(&given).map(f64::from), Some(b)),
        pair => pair,
    };
    match (a, b) {
        (Some(a), Some(b)) if (a - b).abs() <= 1e-9 * b.abs().max(1.0) => return Match::Exact,
        // Jamais de tolérance pour un nombre.
        (Some(_), _) | (_, Some(_)) => return Match::No,
        _ => {}
    }
    let near = if strict { Match::Near } else { Match::Close };
    if folded(&given) == folded(&expected) {
        return near;
    }
    // Ni pour un texte qui contient un chiffre (« en 1789 »).
    if typo_close(&given, &expected) {
        near
    } else {
        Match::No
    }
}

/// Articles ignorés en tête d'une réponse (français, anglais, espagnol).
const ARTICLES: &[&str] = &[
    "de la ", "l'", "le ", "la ", "les ", "un ", "une ", "des ", "du ", "the ", "a ", "an ", "el ",
    "los ", "las ", "unos ", "unas ", "lo ",
];

/// Ponctuation retirée en fin de réponse, guillemets retirés autour.
const TRAILING: &[char] = &['.', '!', '?', ';', ':', ',', '…'];
const QUOTES: &[char] = &['"', '«', '»', '“', '”', '\'', '‘', '’', '`'];

/// Forme commune d'une réponse : minuscules, apostrophes droites, espaces
/// réduits, guillemets autour et ponctuation finale retirés, un article en
/// tête retiré (s'il reste quelque chose).
pub fn normalize(text: &str) -> String {
    normalize_for(text, false)
}

/// Comme [`normalize`] ; `strict` (cours de langue) : article gardé.
pub fn normalize_for(text: &str, strict: bool) -> String {
    let lowered: String = text
        .to_lowercase()
        .chars()
        .map(|c| match c {
            '’' | '‘' | '`' | '´' => '\'',
            '\u{a0}' | '\u{202f}' | '\u{2009}' => ' ',
            other => other,
        })
        .collect();
    let mut text = lowered.split_whitespace().collect::<Vec<_>>().join(" ");
    loop {
        let trimmed = text
            .trim_end_matches(TRAILING)
            .trim()
            .trim_matches(QUOTES)
            .trim()
            .to_owned();
        if trimmed == text {
            break;
        }
        text = trimmed;
    }
    if strict {
        return text;
    }
    for article in ARTICLES {
        if let Some(rest) = text.strip_prefix(article)
            && !rest.trim().is_empty()
        {
            return rest.trim().to_owned();
        }
    }
    text
}

/// Lettres sans accents (pour la comparaison « proche ») ; les marques
/// combinantes d'un texte décomposé (NFD) sont retirées aussi.
fn folded(text: &str) -> Vec<char> {
    text.chars().filter_map(guard::fold).collect()
}

/// Valeur d'une réponse qui est un nombre en chiffres : décimal (virgule ou
/// point), milliers (séparateur suivi d'exactement trois chiffres : « 1,000 »,
/// « 1.000 », « 1 000 »), signe permis, ou fraction `a/b`.
pub fn number(text: &str) -> Option<f64> {
    let text = text.trim();
    if let Some((numerator, denominator)) = text.split_once('/') {
        let denominator = decimal(denominator.trim())?;
        return (denominator != 0.0).then(|| decimal(numerator.trim()).map(|n| n / denominator))?;
    }
    decimal(text)
}

/// Séparateurs de milliers (espaces fines comprises).
const THOUSANDS: &[char] = &[',', '.', ' ', '\u{a0}', '\u{202f}', '\u{2009}'];

fn decimal(text: &str) -> Option<f64> {
    let digits = text.strip_prefix(['-', '+', '−']).unwrap_or(text);
    let value = thousands(digits).or_else(|| {
        let valid = !digits.is_empty()
            && digits
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
            && digits.chars().filter(|c| *c == '.' || *c == ',').count() <= 1
            && digits.chars().next().is_some_and(|c| c.is_ascii_digit())
            && digits.chars().last().is_some_and(|c| c.is_ascii_digit());
        if valid {
            digits.replace(',', ".").parse().ok()
        } else {
            None
        }
    })?;
    Some(if text.starts_with(['-', '−']) {
        -value
    } else {
        value
    })
}

/// Nombre entier écrit avec un séparateur de milliers : un premier groupe de
/// 1 à 3 chiffres sans zéro en tête, puis des groupes d'exactement trois
/// chiffres, tous séparés par le même caractère (« 0,375 » reste décimal).
fn thousands(digits: &str) -> Option<f64> {
    let separator = digits.chars().find(|c| !c.is_ascii_digit())?;
    if !THOUSANDS.contains(&separator) {
        return None;
    }
    let groups: Vec<&str> = digits.split(separator).collect();
    let first = groups[0];
    let valid = (1..=3).contains(&first.len())
        && !first.starts_with('0')
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_digit()))
        && groups[1..].iter().all(|group| group.len() == 3);
    valid.then(|| groups.concat().parse().ok())?
}

/// Unités de 0 à 19 (et formes de 21 à 29 en un mot en espagnol), sans
/// accents.
const UNITS: &[(&str, u32)] = &[
    ("zero", 0),
    ("one", 1),
    ("two", 2),
    ("three", 3),
    ("four", 4),
    ("five", 5),
    ("six", 6),
    ("seven", 7),
    ("eight", 8),
    ("nine", 9),
    ("ten", 10),
    ("eleven", 11),
    ("twelve", 12),
    ("thirteen", 13),
    ("fourteen", 14),
    ("fifteen", 15),
    ("sixteen", 16),
    ("seventeen", 17),
    ("eighteen", 18),
    ("nineteen", 19),
    ("un", 1),
    ("une", 1),
    ("deux", 2),
    ("trois", 3),
    ("quatre", 4),
    ("cinq", 5),
    ("sept", 7),
    ("huit", 8),
    ("neuf", 9),
    ("dix", 10),
    ("onze", 11),
    ("douze", 12),
    ("treize", 13),
    ("quatorze", 14),
    ("quinze", 15),
    ("seize", 16),
    ("cero", 0),
    ("uno", 1),
    ("una", 1),
    ("dos", 2),
    ("tres", 3),
    ("cuatro", 4),
    ("cinco", 5),
    ("seis", 6),
    ("siete", 7),
    ("ocho", 8),
    ("nueve", 9),
    ("diez", 10),
    ("once", 11),
    ("doce", 12),
    ("trece", 13),
    ("catorce", 14),
    ("quince", 15),
    ("dieciseis", 16),
    ("diecisiete", 17),
    ("dieciocho", 18),
    ("diecinueve", 19),
    ("veintiuno", 21),
    ("veintiuna", 21),
    ("veintidos", 22),
    ("veintitres", 23),
    ("veinticuatro", 24),
    ("veinticinco", 25),
    ("veintiseis", 26),
    ("veintisiete", 27),
    ("veintiocho", 28),
    ("veintinueve", 29),
];

/// Dizaines (et cent), sans accents.
const TENS: &[(&str, u32)] = &[
    ("twenty", 20),
    ("thirty", 30),
    ("forty", 40),
    ("fifty", 50),
    ("sixty", 60),
    ("seventy", 70),
    ("eighty", 80),
    ("ninety", 90),
    ("hundred", 100),
    ("vingt", 20),
    ("vingts", 20),
    ("trente", 30),
    ("quarante", 40),
    ("cinquante", 50),
    ("soixante", 60),
    ("cent", 100),
    ("veinte", 20),
    ("treinta", 30),
    ("cuarenta", 40),
    ("cincuenta", 50),
    ("sesenta", 60),
    ("setenta", 70),
    ("ochenta", 80),
    ("noventa", 90),
    ("cien", 100),
    ("ciento", 100),
];

/// Nombre de 0 à 100 écrit en toutes lettres en anglais, espagnol ou
/// français (avec ou sans accents ni traits d'union : « twenty-one »,
/// « vingt et un », « soixante-dix-sept », « quatre-vingts »,
/// « treinta y dos », « veintidós »).
pub fn number_word(text: &str) -> Option<u32> {
    let folded: String = text
        .to_lowercase()
        .chars()
        .filter_map(guard::fold)
        .map(|c| if c == '-' { ' ' } else { c })
        .collect();
    let words: Vec<&str> = folded
        .split_whitespace()
        .filter(|word| !matches!(*word, "et" | "y" | "and"))
        .collect();
    let find = |table: &[(&str, u32)], word: &str| {
        table
            .iter()
            .find(|(name, _)| *name == word)
            .map(|(_, value)| *value)
    };
    // « one hundred », « a hundred », « un cent » n'existe pas : « cent ».
    if let [first, hundred] = words.as_slice()
        && matches!(*first, "one" | "a")
        && *hundred == "hundred"
    {
        return Some(100);
    }
    let mut total = 0u32;
    // Dernière valeur ajoutée : chaque mot suit une plus grande.
    let mut previous: Option<u32> = None;
    let mut index = 0;
    while index < words.len() {
        let word = words[index];
        // « quatre-vingt(s) » = 80.
        let value = if word == "quatre"
            && words
                .get(index + 1)
                .is_some_and(|next| matches!(*next, "vingt" | "vingts"))
        {
            index += 1;
            80
        } else if let Some(value) = find(TENS, word) {
            value
        } else {
            find(UNITS, word)?
        };
        let allowed = match previous {
            None => true,
            // Après une dizaine : une unité (ou 10 à 19 après 60 et 80).
            Some(tens) if tens >= 20 && tens % 10 == 0 && tens < 100 => {
                value < 10 || ((tens == 60 || tens == 80) && value < 20)
            }
            // Après « dix » (français) : 7 à 9 (dix-sept…).
            Some(10) => (7..=9).contains(&value),
            _ => false,
        };
        if !allowed || (value == 100 && previous.is_some()) {
            return None;
        }
        total += value;
        previous = Some(value);
        index += 1;
    }
    (previous.is_some() && total <= 100).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::study::QuestionBody;

    /// Correction hors cours de langue.
    fn grade(question: &StoredQuestion, given: &AnswerValue) -> Option<Graded> {
        super::grade(question, given, false)
    }

    /// Correction d'un trou dans un cours de langue : (juste, proche).
    fn strict(accepted: &[&str], text: &str) -> (bool, bool) {
        let graded = super::grade(
            &blank(accepted),
            &AnswerValue::Blank {
                text: text.to_owned(),
            },
            true,
        )
        .unwrap();
        (graded.correct, graded.close)
    }

    /// rust-1 : jamais de faute pardonnée dans un texte qui contient un
    /// chiffre (date, mesure).
    #[test]
    fn a_wrong_year_in_a_text_is_wrong() {
        for (accepted, given) in [
            ("en 1789", "en 1798"),
            ("en 1789", "en 1788"),
            ("14 juillet 1789", "14 juillet 1788"),
            ("in 1066", "in 1067"),
            ("3 cm", "8 cm"),
        ] {
            assert_eq!(
                graded(&blank(&[accepted]), given),
                (false, false),
                "{given}"
            );
        }
        assert_eq!(graded(&blank(&["en 1789"]), "En 1789."), (true, false));
        // Accent seul : toujours proche.
        assert_eq!(graded(&blank(&["été 1789"]), "ete 1789"), (true, true));
    }

    /// rust-3 : cours de langue : ni article retiré, ni accent plié, ni
    /// faute pardonnée ; proche mais faux.
    #[test]
    fn a_language_course_is_strict_about_spelling() {
        assert_eq!(strict(&["la pomme"], "La pomme."), (true, false));
        // Une lettre de différence : faux, mais proche (orthographe montrée).
        assert_eq!(strict(&["la pomme"], "le pomme"), (false, true));
        assert_eq!(strict(&["la pomme"], "pomme"), (false, false));
        assert_eq!(strict(&["an apple"], "a apple"), (false, true));
        assert_eq!(strict(&["el agua"], "la agua"), (false, false));
        assert_eq!(strict(&["habló"], "hablo"), (false, true));
        assert_eq!(strict(&["parlent"], "parlant"), (false, true));
        assert_eq!(strict(&["has eaten"], "has eaten"), (true, false));
        assert_eq!(strict(&["5"], "five"), (true, false));
        // Même réponses ailleurs : justes.
        assert_eq!(graded(&blank(&["la pomme"]), "pomme"), (true, false));
        assert_eq!(graded(&blank(&["habló"]), "hablo"), (true, true));
        assert_eq!(graded(&blank(&["parlent"]), "parlant"), (true, true));
    }

    /// rust-4 : séparateur suivi d'exactement trois chiffres = milliers.
    #[test]
    fn thousands_separators_are_not_decimal_points() {
        for (accepted, given) in [
            ("1,000", "1000"),
            ("1.000", "1000"),
            ("1 000", "1000"),
            ("1\u{202f}000", "1,000"),
            ("1\u{a0}000\u{a0}000", "1000000"),
            ("12,500", "12500"),
            ("-2,000", "−2000"),
        ] {
            assert_eq!(
                graded(&blank(&[accepted]), given),
                (true, false),
                "{accepted}"
            );
        }
        assert_eq!(graded(&blank(&["1,000"]), "1"), (false, false));
        assert_eq!(graded(&blank(&["1.000"]), "1"), (false, false));
        assert_eq!(number("1,5"), Some(1.5));
        assert_eq!(number("0,375"), Some(0.375));
        assert_eq!(number("1,50"), Some(1.5));
        assert_eq!(number("1,0000"), Some(1.0));
        assert_eq!(number("1,000.5"), None);
        assert_eq!(number("1 5"), None);
    }

    /// rust-17 : un nombre en toutes lettres vaut ses chiffres.
    #[test]
    fn numbers_written_in_words_are_numbers() {
        for (word, value) in [
            ("five", 5),
            ("cinq", 5),
            ("cinco", 5),
            ("zéro", 0),
            ("Twenty-One", 21),
            ("twenty one", 21),
            ("vingt et un", 21),
            ("veintiuno", 21),
            ("veintidós", 22),
            ("treinta y dos", 32),
            ("dix-sept", 17),
            ("dieciséis", 16),
            ("seize", 16),
            ("soixante-dix", 70),
            ("soixante et onze", 71),
            ("soixante-dix-sept", 77),
            ("quatre-vingts", 80),
            ("quatre-vingt-un", 81),
            ("quatre-vingt-dix-neuf", 99),
            ("ninety-nine", 99),
            ("cent", 100),
            ("cien", 100),
            ("one hundred", 100),
        ] {
            assert_eq!(number_word(word), Some(value), "{word}");
        }
        for not in [
            "five two",
            "two five",
            "dix deux",
            "twenty thirty",
            "cent un",
            "pomme",
            "",
            "and",
        ] {
            assert_eq!(number_word(not), None, "{not}");
        }
        assert_eq!(graded(&blank(&["5"]), "five"), (true, false));
        assert_eq!(graded(&blank(&["5"]), "Cinq."), (true, false));
        assert_eq!(graded(&blank(&["cinco"]), "5"), (true, false));
        assert_eq!(graded(&blank(&["5"]), "six"), (false, false));
        assert_eq!(graded(&blank(&["vingt et un"]), "21"), (true, false));
        // Deux mots : comparés comme des mots (« un » n'est pas « une »).
        assert_eq!(graded(&blank(&["une"]), "un"), (false, false));
        assert_eq!(graded(&blank(&["five"]), "cinq"), (false, false));
    }

    fn blank(accepted: &[&str]) -> StoredQuestion {
        StoredQuestion {
            id: "q".into(),
            body: QuestionBody::Blank {
                prompt: "x ___".into(),
            },
            expected: AnswerValue::Blank {
                text: accepted[0].into(),
            },
            accepted: accepted.iter().map(|a| (*a).to_owned()).collect(),
            explanation: String::new(),
        }
    }

    fn graded(question: &StoredQuestion, text: &str) -> (bool, bool) {
        let graded = grade(
            question,
            &AnswerValue::Blank {
                text: text.to_owned(),
            },
        )
        .unwrap();
        (graded.correct, graded.close)
    }

    #[test]
    fn blanks_accept_the_same_text_written_differently() {
        let question = blank(&["Chlorophylle", "la chlorophylle"]);
        for text in [
            "chlorophylle",
            "  CHLOROPHYLLE. ",
            "« chlorophylle »",
            "\"Chlorophylle\"!",
            "la chlorophylle",
            "La  chlorophylle ?",
        ] {
            assert_eq!(graded(&question, text), (true, false), "{text}");
        }
        // Apostrophes courbes ou droites, article élidé.
        let question = blank(&["l'énergie lumineuse"]);
        assert_eq!(graded(&question, "L’énergie lumineuse"), (true, false));
        assert_eq!(graded(&question, "énergie lumineuse"), (true, false));
        let question = blank(&["have eaten"]);
        assert_eq!(graded(&question, "have eaten"), (true, false));
        // Article anglais ou espagnol en tête.
        assert_eq!(
            graded(&blank(&["the mitochondria"]), "mitochondria"),
            (true, false)
        );
        assert_eq!(graded(&blank(&["el pulso"]), "pulso"), (true, false));
        // Un article seul n'est pas retiré.
        assert_eq!(normalize("A"), "a");
        assert_eq!(normalize("the"), "the");
    }

    #[test]
    fn numbers_are_compared_as_numbers() {
        let question = blank(&["0,5"]);
        for text in ["0.5", "0,5", "1/2", " 2/4 ", "0.50"] {
            assert_eq!(graded(&question, text), (true, false), "{text}");
        }
        for text in ["0.6", "5", "1/3", "0,05"] {
            assert_eq!(graded(&question, text), (false, false), "{text}");
        }
        assert_eq!(graded(&blank(&["3/8"]), "0.375"), (true, false));
        assert_eq!(graded(&blank(&["-2"]), "−2"), (true, false));
        // Jamais de tolérance pour un nombre.
        assert_eq!(graded(&blank(&["12345"]), "12354"), (false, false));
        assert_eq!(graded(&blank(&["1789"]), "1798"), (false, false));
        assert_eq!(number("1/0"), None);
        assert_eq!(number("3.5.1"), None);
        assert_eq!(number("abc"), None);
        assert_eq!(number(".5"), None);
    }

    #[test]
    fn accents_and_typos_are_close() {
        let question = blank(&["photosynthèse"]);
        assert_eq!(graded(&question, "photosynthese"), (true, true));
        assert_eq!(graded(&question, "photosyntèse"), (true, true));
        assert_eq!(graded(&question, "fotosintesis"), (false, false));
        // 5 à 9 lettres : une faute ; dès 10 : deux ; moins de 5 : aucune.
        assert_eq!(graded(&blank(&["noyau"]), "noyeu"), (true, true));
        assert_eq!(graded(&blank(&["noyau"]), "nayeu"), (false, false));
        assert_eq!(
            graded(&blank(&["chlorophylle"]), "clorophyle"),
            (true, true)
        );
        assert_eq!(
            graded(&blank(&["chlorophylle"]), "clorofyle"),
            (false, false)
        );
        assert_eq!(graded(&blank(&["have"]), "hav"), (false, false));
        assert_eq!(graded(&blank(&["have"]), "HAVE"), (true, false));
        // Inversion de deux lettres : une faute.
        assert_eq!(graded(&blank(&["glucose"]), "glcuose"), (true, true));
        // La meilleure réponse acceptée l'emporte.
        assert_eq!(
            graded(&blank(&["glucose", "glucosa"]), "glucosa"),
            (true, false)
        );
        assert_eq!(graded(&blank(&["x"]), "  "), (false, false));
    }

    fn choice() -> StoredQuestion {
        StoredQuestion {
            id: "q".into(),
            body: QuestionBody::Choice {
                prompt: "P".into(),
                options: vec!["a".into(), "b".into(), "c".into()],
            },
            expected: AnswerValue::Choice { index: 2 },
            accepted: Vec::new(),
            explanation: String::new(),
        }
    }

    #[test]
    fn choices_orders_and_matches_are_graded_exactly() {
        let question = choice();
        let right = grade(&question, &AnswerValue::Choice { index: 2 }).unwrap();
        assert!(right.correct && !right.close && right.detail.is_empty());
        assert!(
            !grade(&question, &AnswerValue::Choice { index: 0 })
                .unwrap()
                .correct
        );
        assert_eq!(grade(&question, &AnswerValue::Choice { index: 3 }), None);
        assert_eq!(
            grade(&question, &AnswerValue::TrueFalse { value: true }),
            None
        );

        let order = StoredQuestion {
            body: QuestionBody::Order {
                prompt: "P".into(),
                items: vec!["c".into(), "a".into(), "b".into()],
            },
            expected: AnswerValue::Order {
                order: vec![1, 2, 0],
            },
            ..choice()
        };
        let graded = grade(
            &order,
            &AnswerValue::Order {
                order: vec![1, 2, 0],
            },
        )
        .unwrap();
        assert!(graded.correct);
        assert_eq!(graded.detail, [true, true, true]);
        let graded = grade(
            &order,
            &AnswerValue::Order {
                order: vec![1, 0, 2],
            },
        )
        .unwrap();
        assert!(!graded.correct);
        assert_eq!(graded.detail, [true, false, false]);
        for invalid in [vec![1, 1, 0], vec![0, 1], vec![0, 1, 3]] {
            assert_eq!(
                grade(
                    &order,
                    &AnswerValue::Order {
                        order: invalid.clone()
                    }
                ),
                None,
                "{invalid:?}"
            );
        }

        let pairs = StoredQuestion {
            body: QuestionBody::Match {
                prompt: "P".into(),
                left: vec!["1".into(), "2".into(), "3".into()],
                right: vec!["two".into(), "three".into(), "one".into()],
            },
            expected: AnswerValue::Match {
                pairs: vec![2, 0, 1],
            },
            ..choice()
        };
        let graded = grade(
            &pairs,
            &AnswerValue::Match {
                pairs: vec![2, 0, 1],
            },
        )
        .unwrap();
        assert!(graded.correct);
        // Le même élément de droite deux fois : permis, faux.
        let graded = grade(
            &pairs,
            &AnswerValue::Match {
                pairs: vec![2, 2, 1],
            },
        )
        .unwrap();
        assert_eq!(graded.detail, [true, false, true]);
        assert!(!graded.correct);
        assert_eq!(
            grade(&pairs, &AnswerValue::Match { pairs: vec![2, 0] }),
            None
        );
        assert_eq!(
            grade(
                &pairs,
                &AnswerValue::Match {
                    pairs: vec![2, 0, 3]
                }
            ),
            None
        );

        let truth = StoredQuestion {
            body: QuestionBody::TrueFalse {
                statement: "S".into(),
            },
            expected: AnswerValue::TrueFalse { value: false },
            ..choice()
        };
        assert!(
            grade(&truth, &AnswerValue::TrueFalse { value: false })
                .unwrap()
                .correct
        );
        assert!(
            !grade(&truth, &AnswerValue::TrueFalse { value: true })
                .unwrap()
                .correct
        );
    }
}
