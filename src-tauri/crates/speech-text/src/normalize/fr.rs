//! Prétraitement (fr) : port de `.tools/tts-bench/bin/normalize_fr.py`
//! (prototype validé à l'oreille, style « lettres » pour les sigles). Mêmes
//! passes, dans le même ordre ; ses `CASES` sont repris tels quels dans les
//! essais (sortie identique).
//!
//! Écarts voulus au prototype (aucun ne touche ses cas) : un chiffre romain
//! d'une seule lettre n'est lu après un nom de règne ou de section que pour
//! I, V et X (« annexe C » restait « annexe cent ») ; quelques mots courts
//! en capitales se lisent comme des mots (« NON », « OUI », « PAS ») au lieu
//! d'être épelés.

use super::common::{
    acronyms, char_after, char_before, expand_lexicon, finish, in_path, int_value, is_subtraction,
    is_word, next_word, not_after, not_before, parse_int, per_period, prepare, previous_word,
    ratios, re, replace, roman, roman_in_context, scientific, sp, spell, strip_spaces, sub, sub_to,
    symbol_names, unit_ends,
};

// ---------------------------------------------------------------- nombres

const UNITS: [&str; 17] = [
    "zéro", "un", "deux", "trois", "quatre", "cinq", "six", "sept", "huit", "neuf", "dix", "onze",
    "douze", "treize", "quatorze", "quinze", "seize",
];

fn tens(t: u64) -> &'static str {
    match t {
        2 => "vingt",
        3 => "trente",
        4 => "quarante",
        5 => "cinquante",
        _ => "soixante",
    }
}

fn below100(n: u64, fem: bool) -> String {
    if n < 17 {
        return if n == 1 && fem {
            "une"
        } else {
            UNITS[n as usize]
        }
        .to_owned();
    }
    if n < 20 {
        return format!("dix-{}", UNITS[(n - 10) as usize]);
    }
    let (t, u) = (n / 10, n % 10);
    match t {
        7 if u == 1 => "soixante et onze".to_owned(),
        7 => format!("soixante-{}", below100(10 + u, fem)),
        8 if u == 0 => "quatre-vingts".to_owned(),
        8 => format!("quatre-vingt-{}", below100(u, fem)),
        9 => format!("quatre-vingt-{}", below100(10 + u, fem)),
        _ if u == 0 => tens(t).to_owned(),
        _ if u == 1 => format!("{} et {}", tens(t), if fem { "une" } else { "un" }),
        _ => format!("{}-{}", tens(t), UNITS[u as usize]),
    }
}

fn below1000(n: u64, fem: bool) -> String {
    let (h, r) = (n / 100, n % 100);
    if h == 0 {
        return below100(r, fem);
    }
    let head = if h == 1 {
        "cent".to_owned()
    } else {
        format!("{} cent", UNITS[h as usize])
    };
    match r {
        0 if h > 1 => head + "s",
        0 => head,
        _ => format!("{head} {}", below100(r, fem)),
    }
}

/// « deux cents » → « deux cent », « quatre-vingts » → « quatre-vingt »
/// (devant « mille » et dans les ordinaux).
fn without_plural(words: String) -> String {
    if words.ends_with("cents") || words.ends_with("vingts") {
        let mut words = words;
        words.pop();
        words
    } else {
        words
    }
}

/// Entier en toutes lettres ; `fem` : « une », « vingt et une ».
pub(crate) fn cardinal(n: u64, fem: bool) -> String {
    if n == 0 {
        return "zéro".to_owned();
    }
    if n >= 1_000_000_000_000 {
        return spell_digits(&n.to_string());
    }
    let mut n = n;
    let mut parts = Vec::new();
    for (value, name) in [(1_000_000_000, "milliard"), (1_000_000, "million")] {
        let q = n / value;
        n %= value;
        if q > 0 {
            parts.push(format!(
                "{} {name}{}",
                cardinal(q, false),
                if q > 1 { "s" } else { "" }
            ));
        }
    }
    let q = n / 1000;
    n %= 1000;
    if q > 0 {
        // « deux cent mille », « quatre-vingt mille » : pas de pluriel devant mille.
        parts.push(if q == 1 {
            "mille".to_owned()
        } else {
            without_plural(cardinal(q, false)) + " mille"
        });
    }
    if n > 0 {
        parts.push(below1000(n, fem));
    }
    parts.join(" ")
}

/// Chiffre par chiffre (nombres de 13 chiffres ou plus).
fn spell_digits(s: &str) -> String {
    s.chars()
        .filter_map(|c| c.to_digit(10))
        .map(|d| UNITS[d as usize])
        .collect::<Vec<_>>()
        .join(" ")
}

/// Cardinal d'une suite de chiffres sans zéro de tête, de longueur quelconque.
fn cardinal_str(s: &str, fem: bool) -> String {
    match s.parse::<u64>() {
        Ok(n) => cardinal(n, fem),
        Err(_) => spell_digits(s),
    }
}

pub(crate) fn ordinal(n: u64, fem: bool) -> String {
    if n == 1 {
        return if fem { "première" } else { "premier" }.to_owned();
    }
    let mut w = without_plural(cardinal(n, false));
    if w.ends_with("cinq") {
        return w + "uième";
    }
    if w.ends_with("neuf") {
        w.pop();
        return w + "vième";
    }
    if w.ends_with('e') {
        w.pop();
    }
    w + "ième"
}

/// Suite de chiffres telle qu'écrite : « 05 » → « zéro cinq ».
fn digits(s: &str, fem: bool) -> String {
    let rest = s.trim_start_matches('0');
    let zeros = s.len() - rest.len();
    let mut words = vec!["zéro".to_owned(); zeros];
    if !rest.is_empty() {
        words.push(cardinal_str(rest, fem));
    }
    words.join(" ")
}

/// Entier écrit (espaces de groupement permis) ; zéro de tête : chiffre à chiffre.
fn integer(s: &str, fem: bool) -> String {
    let s = strip_spaces(s);
    if s.starts_with('0') && s.len() > 1 {
        digits(&s, fem)
    } else {
        cardinal_str(&s, fem)
    }
}

fn decimal(int_part: &str, frac: &str, fem: bool, word: &str) -> String {
    format!("{} {word} {}", integer(int_part, fem), digits(frac, false))
}

// ---------------------------------------------------------------- unités

/// Symbole : (singulier, pluriel, féminin). Ordre du prototype ; les plus
/// longs sont essayés d'abord (`UNIT_ORDER`).
const UNIT_WORDS: &[(&str, &str, &str, bool)] = &[
    ("%", "pour cent", "pour cent", false),
    ("€", "euro", "euros", false),
    ("$", "dollar", "dollars", false),
    ("£", "livre", "livres", true),
    ("k€", "mille euros", "mille euros", false),
    ("M€", "million d'euros", "millions d'euros", false),
    ("Md€", "milliard d'euros", "milliards d'euros", false),
    ("To", "téraoctet", "téraoctets", false),
    ("Go", "gigaoctet", "gigaoctets", false),
    ("Mo", "mégaoctet", "mégaoctets", false),
    ("Ko", "kilooctet", "kilooctets", false),
    ("TB", "téraoctet", "téraoctets", false),
    ("GB", "gigaoctet", "gigaoctets", false),
    ("MB", "mégaoctet", "mégaoctets", false),
    ("KB", "kilooctet", "kilooctets", false),
    (
        "Go/s",
        "gigaoctet par seconde",
        "gigaoctets par seconde",
        false,
    ),
    (
        "Mo/s",
        "mégaoctet par seconde",
        "mégaoctets par seconde",
        false,
    ),
    ("Gb/s", "gigabit par seconde", "gigabits par seconde", false),
    ("Mb/s", "mégabit par seconde", "mégabits par seconde", false),
    (
        "Gbit/s",
        "gigabit par seconde",
        "gigabits par seconde",
        false,
    ),
    (
        "Mbit/s",
        "mégabit par seconde",
        "mégabits par seconde",
        false,
    ),
    ("ms", "milliseconde", "millisecondes", true),
    ("s", "seconde", "secondes", true),
    ("min", "minute", "minutes", true),
    ("h", "heure", "heures", true),
    ("km/h", "kilomètre par heure", "kilomètres par heure", false),
    ("km", "kilomètre", "kilomètres", false),
    ("m", "mètre", "mètres", false),
    ("cm", "centimètre", "centimètres", false),
    ("mm", "millimètre", "millimètres", false),
    ("kg", "kilogramme", "kilogrammes", false),
    ("g", "gramme", "grammes", false),
    ("mg", "milligramme", "milligrammes", false),
    ("L", "litre", "litres", false),
    ("l", "litre", "litres", false),
    ("cl", "centilitre", "centilitres", false),
    ("ml", "millilitre", "millilitres", false),
    ("mL", "millilitre", "millilitres", false),
    ("°C", "degré Celsius", "degrés Celsius", false),
    ("°F", "degré Fahrenheit", "degrés Fahrenheit", false),
    ("°", "degré", "degrés", false),
    ("Hz", "hertz", "hertz", false),
    ("kHz", "kilohertz", "kilohertz", false),
    ("MHz", "mégahertz", "mégahertz", false),
    ("GHz", "gigahertz", "gigahertz", false),
    ("W", "watt", "watts", false),
    ("kW", "kilowatt", "kilowatts", false),
    ("kWh", "kilowattheure", "kilowattheures", false),
    ("V", "volt", "volts", false),
    ("mAh", "milliampère-heure", "milliampères-heures", false),
    ("px", "pixel", "pixels", false),
    ("fps", "image par seconde", "images par seconde", true),
    ("x", "fois", "fois", false),
];

/// Unités de la plus longue à la plus courte (tri stable, comme le prototype).
fn unit_order() -> &'static [(&'static str, &'static str, &'static str, bool)] {
    static ORDER: std::sync::LazyLock<Vec<(&str, &str, &str, bool)>> =
        std::sync::LazyLock::new(|| {
            let mut units = UNIT_WORDS.to_vec();
            units.sort_by_key(|u| std::cmp::Reverse(u.0.chars().count()));
            units
        });
    &ORDER
}

fn unit(symbol: &str) -> (&'static str, &'static str, bool) {
    let u = UNIT_WORDS
        .iter()
        .find(|u| u.0 == symbol)
        .expect("unité connue");
    (u.1, u.2, u.3)
}

fn quantity(int_part: &str, frac: Option<&str>, symbol: &str) -> String {
    let (singular, plural, fem) = unit(symbol);
    if matches!(symbol, "€" | "$" | "£")
        && let Some(frac) = frac.filter(|f| f.len() <= 2)
    {
        // 2,50 € → « deux euros cinquante » ; 0,50 € → « cinquante centimes ».
        let cents: u64 = format!("{frac:0<2}").parse().unwrap_or(0);
        let units = parse_int(int_part);
        if units == Some(0) {
            let name = if symbol == "€" { "centimes" } else { "cents" };
            return format!("{} {name}", cardinal(cents, false));
        }
        let main = format!(
            "{} {}",
            cardinal_str(&int_value(int_part), fem),
            if units.is_some_and(|u| u < 2) {
                singular
            } else {
                plural
            }
        );
        return if cents > 0 {
            format!("{main} {}", cardinal(cents, false))
        } else {
            main
        };
    }
    // Singulier au-dessous de deux : « 1,5 heure ».
    let below_two = matches!(parse_int(int_part), Some(0 | 1));
    let words = match frac {
        Some(frac) => decimal(int_part, frac, fem, "virgule"),
        None => integer(int_part, fem),
    };
    let name = if below_two { singular } else { plural };
    // « deux milliards d'euros », « un million de dollars » (écart au prototype).
    let whole_millions =
        frac.is_none() && parse_int(int_part).is_some_and(|n| n > 0 && n.is_multiple_of(1_000_000));
    if whole_millions && !matches!(symbol, "%" | "x" | "k€" | "M€" | "Md€") {
        let elided = name.starts_with(|c: char| "aeéèêiouyh".contains(c));
        return format!("{words} {}{name}", if elided { "d'" } else { "de " });
    }
    format!("{words} {name}")
}

// ---------------------------------------------------------------- lexique

/// Sigles qui se disent comme des mots, noms propres à la prononciation
/// fixée ; `{XYZ}` = épelé. Ajouts au prototype : NON, OUI, PAS, TOUT, TRÈS, NOTE.
const LEXICON: &[(&str, &str)] = &[
    ("REST", "rest"),
    ("JSON", "jason"),
    ("YAML", "yamel"),
    ("GIF", "gif"),
    ("RAM", "ram"),
    ("ROM", "rom"),
    ("SIM", "sim"),
    ("PIN", "pin"),
    ("NASA", "nasa"),
    ("OTAN", "otan"),
    ("UNESCO", "unesco"),
    ("SIDA", "sida"),
    ("OVNI", "ovni"),
    ("LASER", "laser"),
    ("RADAR", "radar"),
    ("ASCII", "aski"),
    ("WIFI", "wifi"),
    ("COVID", "covid"),
    ("NVIDIA", "ènvidia"),
    ("Nvidia", "ènvidia"),
    ("NVMe", "{NVME}"),
    ("iOS", "{IOS}"),
    ("macOS", "mac {OS}"),
    ("npm", "{NPM}"),
    ("ChatGPT", "tchatte {GPT}"),
    ("OpenAI", "open {AI}"),
    ("ChatonLM", "libre {LM}"),
    ("NON", "non"),
    ("OUI", "oui"),
    ("PAS", "pas"),
    ("TOUT", "tout"),
    ("TRÈS", "très"),
    ("NOTE", "note"),
];

const MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

const FEMININE: &[&str] = &[
    "minute",
    "seconde",
    "heure",
    "semaine",
    "journée",
    "année",
    "nuit",
    "fois",
    "personne",
    "page",
    "ligne",
    "étape",
    "façon",
    "raison",
    "chose",
    "question",
    "réponse",
    "erreur",
    "fonction",
    "méthode",
    "version",
    "idée",
    "option",
    "image",
    "lettre",
    "place",
    "voiture",
    "tâche",
    "règle",
    "valeur",
    "colonne",
    "table",
    "classe",
    "série",
    "partie",
    "solution",
    "possibilité",
    "commande",
    "requête",
    "session",
    "application",
    "machine",
    "carte",
    "variable",
    "instance",
    "liste",
    "base",
    "clé",
    "clef",
    "vidéo",
    "photo",
    "interface",
    "constante",
    "boucle",
    "chaîne",
    "branche",
    "fenêtre",
    "icône",
    "langue",
    "mémoire",
    "note",
    "phrase",
    "route",
    "source",
    "vue",
    "zone",
    "cellule",
    "couche",
    "case",
    "balise",
    "touche",
    "donnée",
    "entrée",
    "sortie",
    "durée",
    "femme",
    "fille",
    "ville",
    "maison",
    "porte",
    "pièce",
    "tranche",
    "unité",
];
/// Masculins aux terminaisons d'ordinaire féminines.
const MASCULINE: &[&str] = &[
    "musée",
    "lycée",
    "trophée",
    "apogée",
    "été",
    "comité",
    "côté",
    "pâté",
    "traité",
    "silence",
    "squelette",
    "murmure",
    "mercure",
    "bastion",
    "prélude",
    "gorille",
    "portique",
    "pique",
];
/// Terminaisons féminines régulières.
const FEMININE_ENDINGS: &[&str] = &[
    "tion", "sion", "ité", "ée", "ette", "ure", "ance", "ence", "ille", "ude",
];

/// Vrai si le nombre précède un nom féminin (liste, terminaisons régulières ;
/// singulier ou pluriel).
fn feminine(next: Option<&str>) -> bool {
    let Some(word) = next else { return false };
    let word = word.to_lowercase();
    let one = word.strip_suffix('s').unwrap_or(&word);
    if MASCULINE.contains(&one) || !one.chars().all(char::is_alphabetic) {
        return false;
    }
    FEMININE.contains(&one)
        || FEMININE.contains(&word.as_str())
        || FEMININE_ENDINGS
            .iter()
            .any(|e| one.chars().count() > e.chars().count() + 1 && one.ends_with(e))
}

/// Mots courants écrits en capitales pour insister, lus comme des mots
/// (en plus des mots outils) : « Il FAUT », « NE JAMAIS ».
const COMMON_WORDS: &[&str] = &[
    "faut", "vrai", "faux", "ceci", "cela", "jamais", "rien", "surtout", "avant", "après", "ici",
    "doit", "dois", "peux", "veux", "faire", "mal", "bon", "bonne", "seul", "seule", "aucun",
    "aucune", "toute", "toutes", "ni", "sous", "vers", "chez", "fin", "stop", "voir", "lire",
    "moins", "même", "trop", "peu", "beaucoup", "toujours", "vraiment", "note", "rappel", "info",
    "danger", "oui", "non", "pas", "tout", "très",
];

/// Mots après lesquels « M. » n'est pas « monsieur » (« le point M. »).
const NOT_BEFORE_MISTER: &[&str] = &[
    "point", "taille", "lettre", "variable", "le", "la", "les", "un", "une", "du", "de", "des",
    "au", "aux", "colonne", "ligne", "case", "format", "plan", "type", "groupe", "modèle",
    "version", "matrice", "ensemble", "angle", "sommet", "segment",
];

/// Débuts de phrase usuels : « …par A. La droite » n'est pas « monsieur La ».
const SENTENCE_STARTS: &[&str] = &[
    "Le", "La", "Les", "L", "Il", "Elle", "Ils", "Elles", "On", "Je", "J", "Tu", "Nous", "Vous",
    "Ce", "Cette", "Cet", "C", "Ces", "Un", "Une", "Des", "Puis", "Ensuite", "Mais", "Et", "Donc",
    "Alors", "En", "Dans", "Pour", "Si", "Quand", "Enfin", "Ici", "Voici", "Voilà", "Cela", "Ceci",
    "Au", "Aux", "Du", "De", "D", "Par", "Sur", "Avec", "Sans", "Comme", "Or", "Ainsi",
];

/// Mots avant « 16:10 » qui en font un rapport.
const RATIO_WORDS: &[&str] = &[
    "écran",
    "format",
    "ratio",
    "rapport",
    "proportion",
    "aspect",
    "résolution",
];

/// Livres de la Bible : « Jean 3:16 » est un verset.
const BIBLE_BOOKS: &[&str] = &[
    "Jean",
    "Matthieu",
    "Marc",
    "Luc",
    "Genèse",
    "Exode",
    "Psaume",
    "Psaumes",
    "Romains",
    "Actes",
    "Apocalypse",
    "Proverbes",
    "Ésaïe",
    "Isaïe",
    "Corinthiens",
    "Hébreux",
    "Galates",
    "Éphésiens",
];

fn date(d: &str, mo: &str, y: &str) -> String {
    let day = parse_int(d).unwrap_or(0);
    let day = if day == 1 {
        "premier".to_owned()
    } else {
        cardinal(day, false)
    };
    let month = MONTHS[parse_int(mo).unwrap_or(1) as usize - 1];
    format!("{day} {month} {}", cardinal_str(&int_value(y), false))
}

// ---------------------------------------------------------------- passes

const REGNAL: &str = "Louis|Henri|Charles|François|Napoléon|Philippe|Jean|Paul|Benoît|Pie|Léon|\
Jean-Paul|Élisabeth|Elizabeth|Victoria|Guillaume|Frédéric|Pierre|Nicolas|Alexandre|Catherine|\
Grégoire|Clément|Innocent|Richard|Édouard|George|Ramsès";
const SECTIONS: &str = "chapitre|tome|acte|partie|titre|livre|volume|scène|annexe|article";

/// Nombre : entier (groupes de trois chiffres séparés par une espace permis),
/// puis partie décimale.
macro_rules! num {
    () => {
        concat!(
            r"(?P<int>[0-9]{1,3}(?:",
            sp!(),
            r"[0-9]{3})+|[0-9]+)(?:(?P<sep>[,.])(?P<frac>[0-9]+))?"
        )
    };
}

/// `text` (texte parlé français) prêt pour la synthèse.
pub fn normalize(text: &str) -> String {
    let t = prepare(text);
    let t = symbol_names(&t);
    let t = abbreviations(&t);
    let t = romans(&t);
    let t = dates_and_times(&t);
    let t = numbers(&t);
    let t = words(&t);
    finish(&t)
}

/// Abréviations et symboles.
fn abbreviations(t: &str) -> String {
    let upper_next =
        |t: &str, at: usize| char_after(t, at).is_some_and(|c| matches!(c, 'A'..='Z' | 'À'..='Ý'));
    // « M. Dupont », mais pas « le point M. La droite… » ni « la taille M. Elle… ».
    let t = sub(t, re!(concat!(r"\bM\.", sp!())), |c, t| {
        let m = c.get(0).unwrap();
        let before = previous_word(t, m.start());
        let after = t[m.end()..]
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap_or("");
        (upper_next(t, m.end())
            && !before.is_some_and(|w| NOT_BEFORE_MISTER.contains(&w.as_str()))
            && !SENTENCE_STARTS.contains(&after))
        .then(|| "monsieur ".to_owned())
    });
    let t = sub(&t, re!(concat!(r"\bMM\.", sp!())), |c, t| {
        upper_next(t, c.get(0).unwrap().end()).then(|| "messieurs ".to_owned())
    });
    let t = sub(&t, re!(r"\bMmes?\b\.?"), |c, _| {
        Some(
            if c[0].starts_with("Mmes") {
                "mesdames"
            } else {
                "madame"
            }
            .to_owned(),
        )
    });
    let t = replace(&t, re!(r"\bMlle\b\.?"), "mademoiselle");
    let t = replace(&t, re!(r"\bDr\b\.?"), "docteur");
    let t = sub(&t, re!(r"\bPr\b\.?"), |c, t| {
        let rest = &t[c.get(0).unwrap().end()..];
        let trimmed = rest.trim_start();
        (trimmed.len() < rest.len()
            && trimmed.starts_with(|c: char| matches!(c, 'A'..='Z' | 'À'..='Ý')))
        .then(|| "professeur".to_owned())
    });
    let t = replace(&t, re!(r"\bc\.?-à-d\.?"), "c'est-à-dire");
    let t = replace(&t, re!(r"\bp\.\s?ex\."), "par exemple");
    let t = sub(&t, re!(r"\bex\."), |c, t| {
        char_after(t, c.get(0).unwrap().end())
            .is_some_and(char::is_whitespace)
            .then(|| "par exemple".to_owned())
    });
    let t = replace(&t, re!(r"\bcf\."), "voir");
    let t = sub(&t, re!(r"\benv\."), |c, t| {
        t[c.get(0).unwrap().end()..]
            .trim_start()
            .starts_with(|c: char| c.is_ascii_digit())
            .then(|| "environ".to_owned())
    });
    let t = sub(&t, re!(r"\bvs\.?"), |c, t| {
        char_after(t, c.get(0).unwrap().end())
            .is_some_and(char::is_whitespace)
            .then(|| "contre".to_owned())
    });
    let t = sub(&t, re!(r"\betc\."), |c, t| {
        let m = c.get(0).unwrap();
        (!in_path(t, m.start(), m.end() - 1)
            && t[m.end()..]
                .trim_start()
                .starts_with(|c: char| matches!(c, 'a'..='z' | 'à'..='ÿ' | ',' | ';' | ':')))
        .then(|| "et cetera".to_owned())
    });
    // « /etc/hosts » : un chemin, pas une abréviation.
    let t = sub(&t, re!(r"\betc\b"), |c, t| {
        let m = c.get(0).unwrap();
        (!in_path(t, m.start(), m.end())).then(|| "et cetera".to_owned())
    });
    let t = sub(&t, re!(concat!("[nN]°", sp!(), "?")), |c, t| {
        char_after(t, c.get(0).unwrap().end())
            .is_some_and(|c| c.is_ascii_digit())
            .then(|| "numéro ".to_owned())
    });
    let t = replace(&t, re!(r"§\s?"), "paragraphe ");
    let t = sub(&t, re!(concat!("[~≈]", sp!(), "?")), |c, t| {
        let m = c.get(0).unwrap();
        (not_after(t, m.start(), "") && char_after(t, m.end()).is_some_and(|c| c.is_ascii_digit()))
            .then(|| "environ ".to_owned())
    });
    let t = replace(&t, re!(r"\s?&\s?"), " et ");
    let t = replace(&t, re!(r"\s?@\s?"), " arobase ");
    let t = replace(&t, re!(r"\s?[→⇒]\s?"), ", ");
    let t = replace(&t, re!(r"\s=\s"), " égal ");
    let t = replace(&t, re!(r"\s\+\s"), " plus ");
    let t = replace(&t, re!(r"\s×\s"), " fois ");
    // 1920x1080
    let t = sub(&t, re!("[x×]"), |c, t| {
        let m = c.get(0).unwrap();
        (char_before(t, m.start()).is_some_and(|c| c.is_ascii_digit())
            && char_after(t, m.end()).is_some_and(|c| c.is_ascii_digit()))
        .then(|| " par ".to_owned())
    });
    // et/ou
    let t = sub(&t, re!(r"/ou\b"), |c, t| {
        char_before(t, c.get(0).unwrap().start())
            .is_some_and(is_word)
            .then(|| " ou".to_owned())
    });
    // Options : --verbose.
    sub(&t, re!("--?"), |c, t| {
        let m = c.get(0).unwrap();
        (not_after(t, m.start(), "-")
            && char_after(t, m.end()).is_some_and(|c| c.is_ascii_lowercase()))
        .then(String::new)
    })
}

/// Chiffres romains en contexte (seuls I, V et X isolés : « Le », « De »,
/// « Ce » sont des mots).
fn romans(t: &str) -> String {
    let t = sub(
        t,
        re!(r"\b(?P<r>[IVX]|[IVXLCDM]{2,})(?P<suf>er|re|ère|e|ème)\b"),
        |c, _| {
            Some(match roman(&c["r"]) {
                Some(n) => ordinal(u64::from(n), matches!(&c["suf"], "re" | "ère")),
                None => c[0].to_owned(),
            })
        },
    );
    let t = sub(
        &t,
        re!(format!(r"\b(?P<w>{REGNAL}){}(?P<r>[IVXLCDM]+)\b", sp!())),
        |c, _| {
            Some(match roman_in_context(&c["r"]) {
                Some(1) => format!("{} premier", &c["w"]),
                Some(n) => format!("{} {}", &c["w"], cardinal(u64::from(n), false)),
                None => c[0].to_owned(),
            })
        },
    );
    sub(
        &t,
        re!(format!(
            r"(?i)\b(?P<w>{SECTIONS}){}(?P<r>[IVXLCDM]+)\b",
            sp!()
        )),
        |c, _| {
            Some(match roman_in_context(&c["r"]) {
                Some(n) => format!("{} {}", &c["w"], cardinal(u64::from(n), false)),
                None => c[0].to_owned(),
            })
        },
    )
}

/// Versions et adresses, dates, heures, ordinaux.
fn dates_and_times(t: &str) -> String {
    // 1e-5 → « dix puissance moins cinq » (avant les ordinaux : « 1e »).
    let t = scientific(t, |mantissa, negative, exp| {
        let mut out = mantissa.map_or_else(String::new, |c| match c.name("frac") {
            Some(frac) => {
                let word = if &c["sep"] == "," { "virgule" } else { "point" };
                format!("{} fois ", decimal(&c["int"], frac.as_str(), false, word))
            }
            None => format!("{} fois ", integer(&c["int"], false)),
        });
        out.push_str("dix puissance ");
        if negative {
            out.push_str("moins ");
        }
        out.push_str(&integer(exp, false));
        out
    });
    // 0.9.0, v2.1, 192.168.0.1.
    let t = sub(
        &t,
        re!(r"\b[vV]?(?P<v>[0-9]+(?:\.[0-9]+){2,})\b|\b[vV](?P<w>[0-9]+\.[0-9]+)\b"),
        |c, _| {
            let version = c.name("v").or(c.name("w")).unwrap().as_str();
            Some(
                version
                    .split('.')
                    .map(|p| digits(p, false))
                    .collect::<Vec<_>>()
                    .join(" point "),
            )
        },
    );
    // 09/10/2026, 2026-10-09 ; « 1 janvier » → « premier janvier ».
    let t = sub(
        &t,
        re!(r"\b(?P<d>[0-3]?[0-9])/(?P<mo>0?[1-9]|1[0-2])/(?P<y>[0-9]{4})\b"),
        |c, _| Some(date(&c["d"], &c["mo"], &c["y"])),
    );
    let t = sub(
        &t,
        re!(r"\b(?P<y>[0-9]{4})-(?P<mo>0[1-9]|1[0-2])-(?P<d>[0-3][0-9])\b"),
        |c, _| Some(date(&c["d"], &c["mo"], &c["y"])),
    );
    let t = sub(
        &t,
        re!(format!(r"\b1(?:er)?{}(?P<m>{})\b", sp!(), MONTHS.join("|"))),
        |c, _| Some(format!("premier {}", &c["m"])),
    );
    // « 24h/24 », « 7j/7 ».
    let t = replace(
        &t,
        re!(r"\b24 ?h ?/ ?24\b"),
        "vingt-quatre heures sur vingt-quatre",
    );
    let t = replace(&t, re!(r"\b7 ?j ?/ ?7\b"), "sept jours sur sept");
    // N/M : fraction usuelle, note, date sans année, sinon « sur ».
    let t = sub(
        &t,
        re!(r"\b(?P<n>[0-9]{1,3})/(?P<m>[0-9]{1,3})\b"),
        |c, t| {
            let m = c.get(0).unwrap();
            if !not_after(t, m.start(), "./") || !not_before(t, m.end(), "/") {
                return None;
            }
            let (n, d) = (parse_int(&c["n"])?, parse_int(&c["m"])?);
            Some(match (n, d) {
                (1, 2) => "un demi".to_owned(),
                (1, 3) => "un tiers".to_owned(),
                (2, 3) => "deux tiers".to_owned(),
                (1, 4) => "un quart".to_owned(),
                (3, 4) => "trois quarts".to_owned(),
                (_, 5 | 10 | 20 | 100) if n <= d => {
                    format!("{} sur {}", cardinal(n, false), cardinal(d, false))
                }
                (1..=31, 1..=12) => {
                    let day = if n == 1 {
                        "premier".to_owned()
                    } else {
                        cardinal(n, false)
                    };
                    format!("{day} {}", MONTHS[d as usize - 1])
                }
                _ => format!("{} sur {}", cardinal(n, false), cardinal(d, false)),
            })
        },
    );
    // « écran 16:10 », « 16:9 » : un rapport.
    let t = ratios(&t, RATIO_WORDS, |a, b| {
        format!("{} sur {}", cardinal(a, false), cardinal(b, false))
    });
    // Durée 1:30:45 → « une heure trente minutes quarante-cinq secondes ».
    let t = sub(
        &t,
        re!(r"\b(?P<h>[0-9]{1,2}):(?P<m>[0-5][0-9]):(?P<s>[0-5][0-9])\b"),
        |c, _| {
            let parts = [
                (&c["h"], "heure"),
                (&c["m"], "minute"),
                (&c["s"], "seconde"),
            ];
            let words: Vec<String> = parts
                .iter()
                .filter_map(|(v, name)| {
                    let n = parse_int(v)?;
                    (n > 0).then(|| {
                        format!(
                            "{} {name}{}",
                            cardinal(n, true),
                            if n > 1 { "s" } else { "" }
                        )
                    })
                })
                .collect();
            Some(if words.is_empty() {
                "zéro seconde".to_owned()
            } else {
                words.join(" ")
            })
        },
    );
    // 14h30, 14 h 30, 14:30, 1 h 15.
    let t = sub(
        &t,
        re!(concat!(
            r"\b(?P<h>[01]?[0-9]|2[0-3])",
            sp!(),
            "?(?:h|:)",
            sp!(),
            r"?(?P<m>[0-5][0-9])\b"
        )),
        |c, t| {
            let h = parse_int(&c["h"]).unwrap_or(0);
            let m = parse_int(&c["m"]).unwrap_or(0);
            // « Jean 3:16 » : un verset.
            let whole = c.get(0).unwrap();
            if whole.as_str().contains(':') {
                let word_before = t[..whole.start()]
                    .trim_end()
                    .rsplit(' ')
                    .next()
                    .unwrap_or("");
                if BIBLE_BOOKS.contains(&word_before) {
                    return Some(format!("{} {}", cardinal(h, false), cardinal(m, false)));
                }
            }
            let mut out = format!(
                "{} heure{}",
                cardinal(h, true),
                if h > 1 { "s" } else { "" }
            );
            if &c["m"] != "00" {
                out.push(' ');
                out.push_str(&cardinal(m, true));
            }
            Some(out)
        },
    );
    // 1er, 2e, 3ème, 2nd (pas « 2e-3 », notation scientifique).
    sub(
        &t,
        re!(r"\b(?P<n>[0-9]+)(?P<suf>er|re|ère|e|ème|nde?)\b"),
        |c, t| {
            let after = &t[c.get(0).unwrap().end()..];
            if after.starts_with(['-', '+']) && after[1..].starts_with(|c: char| c.is_ascii_digit())
            {
                return None;
            }
            let (n, suf) = (&c["n"], &c["suf"]);
            Some(if matches!(n, "1" | "2") && !matches!(suf, "e" | "ème") {
                match suf {
                    "er" => "premier",
                    "re" | "ère" => "première",
                    "nd" => "second",
                    _ => "seconde",
                }
                .to_owned()
            } else {
                match parse_int(n) {
                    Some(n) => ordinal(n, false),
                    None => c[0].to_owned(),
                }
            })
        },
    )
}

/// Intervalles, négatifs, quantités, décimaux et entiers.
fn numbers(t: &str) -> String {
    // « 10-20 » → « 10 à 20 » (les nombres sont lus ensuite), puis négatifs.
    let t = sub(
        t,
        re!(concat!(r"([0-9]+)", sp!(), "?[-–]", sp!(), r"?([0-9]+)")),
        |c, t| {
            let m = c.get(0).unwrap();
            let rest = &t[m.end()..];
            let mut chars = rest.chars();
            let followed_by_digit = match chars.next() {
                Some(c) if c.is_ascii_digit() => true,
                Some(c) if is_word(c) || c == '.' || c == ',' => {
                    chars.next().is_some_and(|c| c.is_ascii_digit())
                }
                _ => false,
            };
            if !not_after(t, m.start(), ".,") || followed_by_digit {
                return None;
            }
            // « 10 - 3 = 7 » : une soustraction.
            Some(
                if is_subtraction(t, m.start(), m.end(), &["égal", "plus", "moins", "fois"]) {
                    format!("{} moins {}", &c[1], &c[2])
                } else {
                    format!("{} à {}", &c[1], &c[2])
                },
            )
        },
    );
    let t = sub(&t, re!("[-−]"), |c, t| {
        let m = c.get(0).unwrap();
        (not_after(t, m.start(), "") && char_after(t, m.end()).is_some_and(|c| c.is_ascii_digit()))
            .then(|| "moins ".to_owned())
    });
    // « 2 millions € » → « 2 millions d'euros ».
    let t = sub(
        &t,
        re!(concat!(
            r"\b(?P<w>mille|millions?|milliards?)",
            sp!(),
            "?(?P<u>[€$£])"
        )),
        |c, t| {
            not_before(t, c.get(0).unwrap().end(), "").then(|| {
                let (_, plural, _) = unit(&c["u"]);
                match &c["w"] {
                    "mille" => format!("mille {plural}"),
                    w if plural.starts_with('e') => format!("{w} d'{plural}"),
                    w => format!("{w} de {plural}"),
                }
            })
        },
    );
    // Nombre + unité : on essaie les unités de la plus longue à la plus courte ;
    // l'unité est remplacée avec le nombre (« s' », « l' » : des élisions ;
    // « €/mois » : « par mois »).
    let t = sub_to(&t, re!(concat!(num!(), "(?:", sp!(), ")?")), |c, t| {
        let m = c.get(0).unwrap();
        if !not_after(t, m.start(), ".,") {
            return None;
        }
        let rest = &t[m.end()..];
        let (symbol, ..) = unit_order()
            .iter()
            .find(|u| rest.starts_with(u.0) && unit_ends(&rest[u.0.len()..]))?;
        let words = quantity(&c["int"], c.name("frac").map(|f| f.as_str()), symbol);
        let (per, len) = per_period(&rest[symbol.len()..], "par");
        Some((words + &per, m.end() + symbol.len() + len))
    });
    // Monnaie avant le nombre : $15, € 20.
    let t = sub_to(
        &t,
        re!(concat!(r"(?P<unit>[$€£])", sp!(), "?", num!())),
        |c, t| {
            let m = c.get(0).unwrap();
            if !not_before(t, m.end(), "") {
                return None;
            }
            let words = quantity(&c["int"], c.name("frac").map(|f| f.as_str()), &c["unit"]);
            let (per, len) = per_period(&t[m.end()..], "par");
            Some((words + &per, m.end() + len))
        },
    );
    // Décimaux (virgule ; point = version : « Python 3.12 »), entiers ;
    // féminin devant un nom féminin courant (« vingt et une minutes »).
    sub(&t, re!(num!()), |c, t| {
        let m = c.get(0).unwrap();
        if !not_after(t, m.start(), ".,") || !not_before(t, m.end(), "") {
            return None;
        }
        let fem = feminine(next_word(t, m.end()));
        Some(match c.name("frac") {
            Some(frac) => {
                let word = if &c["sep"] == "," { "virgule" } else { "point" };
                decimal(&c["int"], frac.as_str(), fem, word)
            }
            None => integer(&c["int"], fem),
        })
    })
}

/// Mots mêlant lettres et chiffres, lexique, sigles, élision.
fn words(t: &str) -> String {
    // MP3, 4K, x86, Win11.
    let t = sub(t, re!(r"\b[A-Za-z0-9]+\b"), |c, _| {
        let w = &c[0];
        if !(w.bytes().any(|b| b.is_ascii_digit()) && w.bytes().any(|b| b.is_ascii_alphabetic())) {
            return Some(w.to_owned());
        }
        let runs = re!(r"[0-9]+|[A-Za-z]+").find_iter(w).map(|run| {
            let run = run.as_str();
            if run.as_bytes()[0].is_ascii_digit() {
                cardinal_str(&int_value(run), false)
            } else if run.chars().all(|c| c.is_ascii_uppercase()) || run.len() <= 2 {
                spell(run)
            } else {
                run.to_owned()
            }
        });
        Some(runs.collect::<Vec<_>>().join(" "))
    });
    // Lexique, puis sigles en capitales (épelés ; mots de 5 lettres ou plus avec voyelle : lus).
    let lexicon = re!({
        let mut keys: Vec<&str> = LEXICON.iter().map(|(k, _)| *k).collect();
        keys.sort_by_key(|k| std::cmp::Reverse(k.chars().count()));
        format!(
            r"\b(?:{})\b",
            keys.iter()
                .map(|k| regex::escape(k))
                .collect::<Vec<_>>()
                .join("|")
        )
    });
    let t = sub(&t, lexicon, |c, _| {
        LEXICON
            .iter()
            .find(|(k, _)| *k == &c[0])
            .map(|(_, v)| expand_lexicon(v))
    });
    let t = acronyms(
        &t,
        re!("[A-ZÀ-Ý]{2,}"),
        |c| matches!(c, 'A' | 'E' | 'I' | 'O' | 'U' | 'Y' | 'À'..='Ý'),
        crate::Language::Fr,
        COMMON_WORDS,
        &["CE", "SA"],
    );
    // Élision devant « un », « une » produits plus haut : « de un » → « d'un ».
    sub(&t, re!(r"\b([dD]e|[qQ]ue|[jJ]usque) (une?)\b"), |c, _| {
        let head = &c[1];
        Some(format!("{}'{}", &head[..head.len() - 1], &c[2]))
    })
}

#[cfg(test)]
#[path = "fr_tests.rs"]
mod tests;
