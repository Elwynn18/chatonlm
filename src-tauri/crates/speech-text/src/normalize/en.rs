//! Prétraitement (en) : contrat 59 § 3, sur le modèle du français (mêmes
//! passes, même style de sigles « A P I »).
//!
//! Cardinaux américains (« one hundred twenty », sans « and ») ; années lues
//! par paires (« twenty twenty-five », « nineteen oh five », « two thousand
//! five ») seulement en contexte (in, since, by…, après un mois) ; milliers
//! à virgule (« 10,000 »), décimaux chiffre à chiffre (« three point one
//! four ») ; versions par parties (« three point twelve point one »).

use super::common::{
    acronyms, char_after, char_before, expand_lexicon, finish, in_path, int_value, is_subtraction,
    is_word, next_word, not_after, not_before, parse_int, per_period, prepare, ratios, re, replace,
    roman_in_context, scientific, spell, sub, sub_to, symbol_names, unit_ends,
};

// ---------------------------------------------------------------- nombres

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

fn below100(n: u64) -> String {
    if n < 20 {
        return ONES[n as usize].to_owned();
    }
    let (t, u) = (n / 10, n % 10);
    if u == 0 {
        TENS[t as usize].to_owned()
    } else {
        format!("{}-{}", TENS[t as usize], ONES[u as usize])
    }
}

fn below1000(n: u64) -> String {
    let (h, r) = (n / 100, n % 100);
    match (h, r) {
        (0, _) => below100(r),
        (_, 0) => format!("{} hundred", ONES[h as usize]),
        _ => format!("{} hundred {}", ONES[h as usize], below100(r)),
    }
}

pub(crate) fn cardinal(n: u64) -> String {
    if n == 0 {
        return "zero".to_owned();
    }
    if n >= 1_000_000_000_000_000 {
        return spell_digits(&n.to_string());
    }
    let mut parts = Vec::new();
    let mut rest = n;
    for (value, name) in [
        (1_000_000_000_000, "trillion"),
        (1_000_000_000, "billion"),
        (1_000_000, "million"),
        (1000, "thousand"),
    ] {
        let q = rest / value;
        rest %= value;
        if q > 0 {
            parts.push(format!("{} {name}", below1000(q)));
        }
    }
    if rest > 0 {
        parts.push(below1000(rest));
    }
    parts.join(" ")
}

fn spell_digits(s: &str) -> String {
    s.chars()
        .filter_map(|c| c.to_digit(10))
        .map(|d| ONES[d as usize])
        .collect::<Vec<_>>()
        .join(" ")
}

fn cardinal_str(s: &str) -> String {
    s.parse::<u64>().map_or_else(|_| spell_digits(s), cardinal)
}

pub(crate) fn ordinal(n: u64) -> String {
    let words = cardinal(n);
    let cut = words.rfind([' ', '-']).map_or(0, |i| i + 1);
    let (head, last) = words.split_at(cut);
    let last = match last {
        "one" => "first".to_owned(),
        "two" => "second".to_owned(),
        "three" => "third".to_owned(),
        "five" => "fifth".to_owned(),
        "eight" => "eighth".to_owned(),
        "nine" => "ninth".to_owned(),
        "twelve" => "twelfth".to_owned(),
        w if w.ends_with('y') => format!("{}ieth", &w[..w.len() - 1]),
        w => format!("{w}th"),
    };
    format!("{head}{last}")
}

/// Année lue par paires : 1990 → « nineteen ninety », 1905 → « nineteen oh
/// five », 2005 → « two thousand five », 1900 → « nineteen hundred ».
fn year(n: u64) -> String {
    if !(1100..=2099).contains(&n) || (2000..=2009).contains(&n) {
        return cardinal(n);
    }
    let (high, low) = (n / 100, n % 100);
    match low {
        0 => format!("{} hundred", below100(high)),
        1..=9 => format!("{} oh {}", below100(high), ONES[low as usize]),
        _ => format!("{} {}", below100(high), below100(low)),
    }
}

/// « 05 » → « zero five » ; décimales : chiffre à chiffre.
fn digit_by_digit(s: &str) -> String {
    spell_digits(s)
}

fn integer(s: &str) -> String {
    let s: String = s.chars().filter(|&c| c != ',').collect();
    if s.starts_with('0') && s.len() > 1 {
        spell_digits(&s)
    } else {
        cardinal_str(&s)
    }
}

// ---------------------------------------------------------------- unités

/// Symbole : (singulier, pluriel).
const UNITS: &[(&str, &str, &str)] = &[
    ("%", "percent", "percent"),
    ("€", "euro", "euros"),
    ("$", "dollar", "dollars"),
    ("£", "pound", "pounds"),
    ("TB", "terabyte", "terabytes"),
    ("GB", "gigabyte", "gigabytes"),
    ("MB", "megabyte", "megabytes"),
    ("KB", "kilobyte", "kilobytes"),
    ("kB", "kilobyte", "kilobytes"),
    ("GB/s", "gigabyte per second", "gigabytes per second"),
    ("MB/s", "megabyte per second", "megabytes per second"),
    ("Gbps", "gigabit per second", "gigabits per second"),
    ("Mbps", "megabit per second", "megabits per second"),
    ("ms", "millisecond", "milliseconds"),
    ("s", "second", "seconds"),
    ("sec", "second", "seconds"),
    ("min", "minute", "minutes"),
    ("h", "hour", "hours"),
    ("hr", "hour", "hours"),
    ("km/h", "kilometer per hour", "kilometers per hour"),
    ("mph", "mile per hour", "miles per hour"),
    ("km", "kilometer", "kilometers"),
    ("m", "meter", "meters"),
    ("cm", "centimeter", "centimeters"),
    ("mm", "millimeter", "millimeters"),
    ("mi", "mile", "miles"),
    ("ft", "foot", "feet"),
    ("kg", "kilogram", "kilograms"),
    ("g", "gram", "grams"),
    ("mg", "milligram", "milligrams"),
    ("lb", "pound", "pounds"),
    ("lbs", "pound", "pounds"),
    ("L", "liter", "liters"),
    ("ml", "milliliter", "milliliters"),
    ("mL", "milliliter", "milliliters"),
    ("°C", "degree Celsius", "degrees Celsius"),
    ("°F", "degree Fahrenheit", "degrees Fahrenheit"),
    ("°", "degree", "degrees"),
    ("Hz", "hertz", "hertz"),
    ("kHz", "kilohertz", "kilohertz"),
    ("MHz", "megahertz", "megahertz"),
    ("GHz", "gigahertz", "gigahertz"),
    ("W", "watt", "watts"),
    ("kW", "kilowatt", "kilowatts"),
    ("kWh", "kilowatt hour", "kilowatt hours"),
    ("V", "volt", "volts"),
    ("mAh", "milliamp hour", "milliamp hours"),
    ("px", "pixel", "pixels"),
    ("fps", "frame per second", "frames per second"),
    ("x", "times", "times"),
];

fn unit_order() -> &'static [(&'static str, &'static str, &'static str)] {
    static ORDER: std::sync::LazyLock<Vec<(&str, &str, &str)>> = std::sync::LazyLock::new(|| {
        let mut units = UNITS.to_vec();
        units.sort_by_key(|u| std::cmp::Reverse(u.0.chars().count()));
        units
    });
    &ORDER
}

/// Grands nombres écrits en mots après un montant : « $1.5 million ».
const SCALES: &[&str] = &["thousand", "million", "billion", "trillion"];

fn number_words(int_part: &str, frac: Option<&str>) -> String {
    match frac {
        Some(frac) => format!("{} point {}", integer(int_part), digit_by_digit(frac)),
        None => integer(int_part),
    }
}

fn quantity(int_part: &str, frac: Option<&str>, symbol: &str, scale: Option<&str>) -> String {
    let (_, singular, plural) = *UNITS.iter().find(|u| u.0 == symbol).expect("known unit");
    let units = parse_int(&int_part.replace(',', ""));
    if let Some(scale) = scale {
        return format!("{} {scale} {plural}", number_words(int_part, frac));
    }
    if matches!(symbol, "€" | "$" | "£")
        && let Some(frac) = frac.filter(|f| f.len() <= 2)
    {
        // $2.50 → « two dollars and fifty cents » ; $0.99 → « ninety-nine cents ».
        let cents: u64 = format!("{frac:0<2}").parse().unwrap_or(0);
        let (cent, cents_name) = if symbol == "£" {
            ("penny", "pence")
        } else {
            ("cent", "cents")
        };
        let cents_words = format!(
            "{} {}",
            cardinal(cents),
            if cents == 1 { cent } else { cents_name }
        );
        if units == Some(0) {
            return cents_words;
        }
        let main = format!(
            "{} {}",
            integer(int_part),
            if units == Some(1) { singular } else { plural }
        );
        return if cents > 0 {
            format!("{main} and {cents_words}")
        } else {
            main
        };
    }
    let one = units == Some(1) && frac.is_none_or(|f| f.chars().all(|c| c == '0'));
    format!(
        "{} {}",
        number_words(int_part, frac),
        if one { singular } else { plural }
    )
}

// ---------------------------------------------------------------- lexique

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
    ("NATO", "nato"),
    ("UNESCO", "unesco"),
    ("ASCII", "askee"),
    ("WIFI", "wifi"),
    ("WiFi", "wifi"),
    ("COVID", "covid"),
    ("NVIDIA", "en vidia"),
    ("Nvidia", "en vidia"),
    ("NVMe", "{NVME}"),
    ("iOS", "{IOS}"),
    ("macOS", "mac {OS}"),
    ("npm", "{NPM}"),
    ("ChatGPT", "chat {GPT}"),
    ("OpenAI", "open {AI}"),
    ("ChatonLM", "libre {LM}"),
    ("OK", "okay"),
    ("TODO", "to do"),
    ("NOT", "not"),
    ("ALL", "all"),
    ("AND", "and"),
    ("OR", "or"),
    ("BUT", "but"),
    ("NO", "no"),
    ("YES", "yes"),
    ("ONLY", "only"),
    ("NOTE", "note"),
    ("MUST", "must"),
    ("NEW", "new"),
    ("FREE", "free"),
    ("NOW", "now"),
    ("STOP", "stop"),
    ("DO", "do"),
    ("THE", "the"),
    ("YOU", "you"),
    ("ARE", "are"),
    ("IS", "is"),
    ("TIP", "tip"),
];

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Mots après lesquels un nombre de 1100 à 2099 est une année.
const YEAR_CONTEXT: &[&str] = &[
    "in", "since", "by", "from", "until", "till", "before", "after", "during", "circa", "around",
    "year", "early", "late", "mid", "spring", "summer", "fall", "autumn", "winter", "between",
];

/// Mots avant « of » qui font une année : « the summer of 1969 », « the end
/// of 2025 » (« a window of 2048 tokens » n'en est pas une).
const OF_YEAR: &[&str] = &[
    "year",
    "summer",
    "spring",
    "fall",
    "autumn",
    "winter",
    "end",
    "beginning",
    "start",
    "middle",
    "half",
    "class",
    "quarter",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

/// Mots avant « 16:9 » qui en font un rapport.
const RATIO_WORDS: &[&str] = &[
    "screen",
    "ratio",
    "aspect",
    "format",
    "display",
    "resolution",
];

/// Livres de la Bible : « John 3:16 » est un verset.
const BIBLE_BOOKS: &[&str] = &[
    "John",
    "Matthew",
    "Mark",
    "Luke",
    "Genesis",
    "Exodus",
    "Psalm",
    "Psalms",
    "Romans",
    "Acts",
    "Revelation",
    "Proverbs",
    "Isaiah",
    "Corinthians",
    "Hebrews",
    "Galatians",
    "Ephesians",
];

/// Mots courants écrits en capitales pour insister, lus comme des mots (en
/// plus des mots outils) : « It is VERY important », « I AM SURE ».
const COMMON_WORDS: &[&str] = &[
    "very", "am", "sure", "fine", "never", "always", "ever", "must", "only", "note", "new", "free",
    "now", "stop", "here", "why", "every", "each", "one", "two", "too", "off", "up", "out", "me",
    "him", "her", "them", "us", "get", "got", "go", "let", "make", "use", "see", "keep", "read",
    "wait", "tip", "hot", "big", "bad", "good", "best", "less", "much", "many", "any", "some",
    "none", "yes", "ok", "warning", "danger", "true", "false", "back", "down", "over", "really",
];

const REGNAL: &str = "Henry|Louis|Charles|George|Edward|William|Elizabeth|Richard|James|John Paul|John|\
Paul|Benedict|Pius|Leo|Gregory|Francis|Philip|Mary|Victoria|Catherine|Peter|Nicholas|Alexander|\
Frederick|Napoleon|Ramesses|Constantine|Innocent|Clement|Boniface|Urban";
const SECTIONS: &str = "chapter|part|volume|act|scene|book|section|article|phase|season|episode|\
appendix|level|type|class|war|bowl|title";

macro_rules! num {
    () => {
        r"(?P<int>[0-9]{1,3}(?:,[0-9]{3})+|[0-9]+)(?:\.(?P<frac>[0-9]+))?"
    };
}

/// `text` (texte parlé anglais) prêt pour la synthèse.
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

fn abbreviations(t: &str) -> String {
    let t = replace(t, re!(r"\bDr\."), "Doctor");
    let t = replace(&t, re!(r"\bMrs\."), "Missus");
    let t = replace(&t, re!(r"\bMr\."), "Mister");
    let t = replace(&t, re!(r"\bMs\."), "Miz");
    let t = replace(&t, re!(r"\bProf\."), "Professor");
    let t = replace(&t, re!(r"\bJr\."), "Junior");
    let t = replace(&t, re!(r"\be\.g\.,?"), "for example,");
    let t = replace(&t, re!(r"\bi\.e\.,?"), "that is,");
    let t = replace(&t, re!(r"\bapprox\."), "approximately");
    let t = sub(&t, re!(r"\bvs\.?"), |c, t| {
        char_after(t, c.get(0).unwrap().end())
            .is_some_and(char::is_whitespace)
            .then(|| "versus".to_owned())
    });
    let t = sub(&t, re!(r"\b[Nn]o\.\s?"), |c, t| {
        char_after(t, c.get(0).unwrap().end())
            .is_some_and(|c| c.is_ascii_digit())
            .then(|| "number ".to_owned())
    });
    let t = sub(&t, re!(r"#"), |c, t| {
        char_after(t, c.get(0).unwrap().end())
            .is_some_and(|c| c.is_ascii_digit())
            .then(|| "number ".to_owned())
    });
    let t = sub(&t, re!(r"\betc\."), |c, t| {
        let m = c.get(0).unwrap();
        (!in_path(t, m.start(), m.end() - 1)
            && t[m.end()..]
                .trim_start()
                .starts_with(|c: char| c.is_lowercase() || matches!(c, ',' | ';' | ':')))
        .then(|| "et cetera".to_owned())
    });
    // « /etc/hosts » : a path.
    let t = sub(&t, re!(r"\betc\b"), |c, t| {
        let m = c.get(0).unwrap();
        (!in_path(t, m.start(), m.end())).then(|| "et cetera".to_owned())
    });
    let t = replace(&t, re!(r"§\s?"), "section ");
    let t = sub(&t, re!(r"[~≈]\s?"), |c, t| {
        let m = c.get(0).unwrap();
        (not_after(t, m.start(), "")
            && char_after(t, m.end()).is_some_and(|c| c.is_ascii_digit() || "$€£".contains(c)))
        .then(|| "about ".to_owned())
    });
    let t = replace(&t, re!(r"\s?&\s?"), " and ");
    let t = replace(&t, re!(r"\s?@\s?"), " at ");
    let t = replace(&t, re!(r"\s?[→⇒]\s?"), ", ");
    let t = replace(&t, re!(r"\s=\s"), " equals ");
    let t = replace(&t, re!(r"\s\+\s"), " plus ");
    let t = replace(&t, re!(r"\s×\s"), " times ");
    let t = sub(&t, re!("[x×]"), |c, t| {
        let m = c.get(0).unwrap();
        (char_before(t, m.start()).is_some_and(|c| c.is_ascii_digit())
            && char_after(t, m.end()).is_some_and(|c| c.is_ascii_digit()))
        .then(|| " by ".to_owned())
    });
    let t = sub(&t, re!(r"/or\b"), |c, t| {
        char_before(t, c.get(0).unwrap().start())
            .is_some_and(is_word)
            .then(|| " or".to_owned())
    });
    sub(&t, re!("--?"), |c, t| {
        let m = c.get(0).unwrap();
        (not_after(t, m.start(), "-")
            && char_after(t, m.end()).is_some_and(|c| c.is_ascii_lowercase()))
        .then(String::new)
    })
}

/// Majuscule initiale : « the Eighth ».
fn capitalized(words: &str) -> String {
    let mut chars = words.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// Vrai si le chiffre romain se termine avant une ponctuation ou la fin.
fn before_punctuation(t: &str, end: usize) -> bool {
    t[end..]
        .chars()
        .next()
        .is_none_or(|c| !c.is_alphanumeric() && !c.is_whitespace())
}

fn romans(t: &str) -> String {
    // Henry VIII → « Henry the Eighth » ; « I » seul est souvent le pronom
    // (« Tell John I said ») : seulement devant une ponctuation, la fin ou
    // une capitale.
    let t = sub(
        t,
        re!(format!(r"\b(?P<w>{REGNAL}) (?P<r>[IVXLCDM]+)\b")),
        |c, t| {
            let end = c.get(0).unwrap().end();
            if &c["r"] == "I"
                && !before_punctuation(t, end)
                && !t[end..].trim_start().starts_with(char::is_uppercase)
            {
                return Some(c[0].to_owned());
            }
            Some(match roman_in_context(&c["r"]) {
                Some(n) => format!("{} the {}", &c["w"], capitalized(&ordinal(u64::from(n)))),
                None => c[0].to_owned(),
            })
        },
    );
    // World War II → « two » ; Chapter IV → « four » ; une seule lettre
    // après un nom en minuscules (« the part I don't ») : seulement devant
    // une ponctuation ou la fin.
    sub(
        &t,
        re!(format!(r"(?i)\b(?P<w>{SECTIONS}) (?P<r>[IVXLCDM]+)\b")),
        |c, t| {
            let single = c["r"].chars().count() == 1;
            let capitalized = c["w"].starts_with(char::is_uppercase);
            if single && !capitalized && !before_punctuation(t, c.get(0).unwrap().end()) {
                return Some(c[0].to_owned());
            }
            Some(match roman_in_context(&c["r"]) {
                Some(n) => format!("{} {}", &c["w"], cardinal(u64::from(n))),
                None => c[0].to_owned(),
            })
        },
    )
}

fn month_index(name: &str) -> Option<usize> {
    MONTHS.iter().position(|m| m.eq_ignore_ascii_case(name))
}

fn date_words(month: usize, day: u64, y: Option<u64>) -> String {
    let mut out = format!("{} {}", MONTHS[month], ordinal(day));
    if let Some(y) = y {
        out.push_str(", ");
        out.push_str(&year(y));
    }
    out
}

fn dates_and_times(t: &str) -> String {
    // 1e-5 → « ten to the minus five ».
    let t = scientific(t, |mantissa, negative, exp| {
        let mut out = mantissa.map_or_else(String::new, |c| {
            format!(
                "{} times ",
                number_words(&c["int"], c.name("frac").map(|f| f.as_str()))
            )
        });
        out.push_str("ten to the ");
        if negative {
            out.push_str("minus ");
        }
        out.push_str(&integer(exp));
        out
    });
    let t = sub(
        &t,
        re!(r"\b[vV]?(?P<v>[0-9]+(?:\.[0-9]+){2,})\b|\b[vV](?P<w>[0-9]+\.[0-9]+)\b"),
        |c, _| {
            let version = c.name("v").or(c.name("w")).unwrap().as_str();
            Some(
                version
                    .split('.')
                    .map(integer)
                    .collect::<Vec<_>>()
                    .join(" point "),
            )
        },
    );
    // 2026-10-09 ; 10/09/2026 (mois d'abord, jour d'abord si le premier dépasse 12).
    let t = sub(
        &t,
        re!(r"\b(?P<y>[0-9]{4})-(?P<mo>0[1-9]|1[0-2])-(?P<d>[0-3][0-9])\b"),
        |c, _| {
            let mo = parse_int(&c["mo"])? as usize;
            Some(date_words(mo - 1, parse_int(&c["d"])?, parse_int(&c["y"])))
        },
    );
    let t = sub(
        &t,
        re!(r"\b(?P<a>[0-3]?[0-9])/(?P<b>[0-3]?[0-9])/(?P<y>[0-9]{4})\b"),
        |c, _| {
            let (a, b) = (parse_int(&c["a"])?, parse_int(&c["b"])?);
            let (mo, d) = if a > 12 { (b, a) } else { (a, b) };
            ((1..=12).contains(&mo) && (1..=31).contains(&d))
                .then(|| date_words(mo as usize - 1, d, parse_int(&c["y"])))
        },
    );
    // October 9, 2026 → « October ninth, twenty twenty-six ».
    let t = sub(
        &t,
        re!(format!(
            r"\b(?P<m>{}) (?P<d>[0-3]?[0-9])(?:st|nd|rd|th)?\b(?:, (?P<y>[0-9]{{4}})\b)?",
            MONTHS.join("|")
        )),
        |c, _| {
            let d = parse_int(&c["d"]).filter(|d| (1..=31).contains(d))?;
            Some(date_words(
                month_index(&c["m"])?,
                d,
                c.name("y").and_then(|y| parse_int(y.as_str())),
            ))
        },
    );
    // 9 October 2026 → « the ninth of October, twenty twenty-six ».
    let t = sub(
        &t,
        re!(format!(
            r"\b(?P<d>[0-3]?[0-9])(?:st|nd|rd|th)? (?P<m>{})\b(?: (?P<y>[0-9]{{4}})\b)?",
            MONTHS.join("|")
        )),
        |c, _| {
            let d = parse_int(&c["d"]).filter(|d| (1..=31).contains(d))?;
            let mut out = format!("the {} of {}", ordinal(d), &c["m"]);
            if let Some(y) = c.name("y").and_then(|y| parse_int(y.as_str())) {
                out.push_str(", ");
                out.push_str(&year(y));
            }
            Some(out)
        },
    );
    let t = replace(&t, re!(r"\b24/7\b"), "twenty-four seven");
    // N/M : fraction, score, date (mois d'abord), sinon « out of ».
    let t = sub(
        &t,
        re!(r"\b(?P<n>[0-9]{1,3})/(?P<m>[0-9]{1,3})\b"),
        |c, t| {
            let m = c.get(0).unwrap();
            if !not_after(t, m.start(), "./") || !not_before(t, m.end(), "/") {
                return None;
            }
            let (n, d) = (parse_int(&c["n"])?, parse_int(&c["m"])?);
            let out_of = || format!("{} out of {}", cardinal(n), cardinal(d));
            Some(match (n, d) {
                (1, 2) => "one half".to_owned(),
                (1, 3) => "one third".to_owned(),
                (2, 3) => "two thirds".to_owned(),
                (1, 4) => "one quarter".to_owned(),
                (3, 4) => "three quarters".to_owned(),
                (_, 5 | 10 | 20 | 100) if n <= d => out_of(),
                (1..=12, 1..=31) => date_words(n as usize - 1, d, None),
                _ => out_of(),
            })
        },
    );
    // « 16:9 », « aspect ratio 16:10 » : a ratio.
    let t = ratios(&t, RATIO_WORDS, |a, b| {
        format!("{} by {}", cardinal(a), cardinal(b))
    });
    // 1:30:45 → « one hour thirty minutes forty-five seconds ».
    let t = sub(
        &t,
        re!(r"\b(?P<h>[0-9]{1,2}):(?P<m>[0-5][0-9]):(?P<s>[0-5][0-9])\b"),
        |c, _| {
            let parts = [(&c["h"], "hour"), (&c["m"], "minute"), (&c["s"], "second")];
            let words: Vec<String> = parts
                .iter()
                .filter_map(|(v, name)| {
                    let n = parse_int(v)?;
                    (n > 0)
                        .then(|| format!("{} {name}{}", cardinal(n), if n > 1 { "s" } else { "" }))
                })
                .collect();
            Some(if words.is_empty() {
                "zero seconds".to_owned()
            } else {
                words.join(" ")
            })
        },
    );
    // 2:30 pm → « two thirty p m » ; 9:05 → « nine oh five » ; 9:00 → « nine
    // o'clock » ; point final gardé (« at 2:30 p.m. Then »). « John 3:16 » :
    // a verse.
    let t = sub(
        &t,
        re!(
            r"\b(?P<h>[01]?[0-9]|2[0-3]):(?P<m>[0-5][0-9])\b(?:\s?(?P<ampm>[aApP])\.?[mM]\b(?P<dot>\.)?)?"
        ),
        |c, t| {
            let whole = c.get(0).unwrap();
            let word_before = t[..whole.start()]
                .trim_end()
                .rsplit(' ')
                .next()
                .unwrap_or("");
            if c.name("ampm").is_none() && BIBLE_BOOKS.contains(&word_before) {
                return Some(format!(
                    "{} {}",
                    cardinal(parse_int(&c["h"])?),
                    cardinal(parse_int(&c["m"])?)
                ));
            }
            let h = cardinal(parse_int(&c["h"])?);
            let m = parse_int(&c["m"])?;
            let suffix = c
                .name("ampm")
                .map(|a| format!(" {} m", a.as_str().to_lowercase()));
            let minutes = match (m, &suffix) {
                (0, Some(_)) => String::new(),
                (0, None) => " o'clock".to_owned(),
                (1..=9, _) => format!(" oh {}", ONES[m as usize]),
                _ => format!(" {}", below100(m)),
            };
            let mut out = format!("{h}{minutes}{}", suffix.unwrap_or_default());
            if c.name("dot").is_some() {
                let next = t[whole.end()..].trim_start();
                if next.is_empty() || next.starts_with(char::is_uppercase) {
                    out.push('.');
                }
            }
            Some(out)
        },
    );
    // 5pm, 5 p.m.
    let t = sub(
        &t,
        re!(r"\b(?P<h>1[0-2]|[1-9])\s?(?P<ampm>[aApP])\.?[mM]\b\.?"),
        |c, t| {
            let m = c.get(0).unwrap();
            let mut out = format!(
                "{} {} m",
                cardinal(parse_int(&c["h"])?),
                c["ampm"].to_lowercase()
            );
            // Point final de phrase gardé : « at 5 p.m. Then ».
            if m.as_str().ends_with('.') && char_after(t, m.end()).is_none_or(char::is_whitespace) {
                let next = t[m.end()..].trim_start();
                if next.is_empty() || next.starts_with(char::is_uppercase) {
                    out.push('.');
                }
            }
            Some(out)
        },
    );
    // 1st, 2nd, 3rd, 21st (pas « 2e-3 », déjà lu).
    sub(&t, re!(r"\b(?P<n>[0-9]+)(?:st|nd|rd|th)\b"), |c, _| {
        Some(ordinal(parse_int(&c["n"])?))
    })
}

/// Mot qui précède l'octet `at` (en minuscules).
fn previous_word(t: &str, at: usize) -> Option<String> {
    let before = t[..at].trim_end();
    if before.len() == t[..at].len() && !before.is_empty() {
        return None;
    }
    let word: String = before
        .chars()
        .rev()
        .take_while(|c| c.is_alphabetic())
        .collect();
    (!word.is_empty()).then(|| word.chars().rev().collect::<String>().to_lowercase())
}

fn is_year_context(t: &str, at: usize) -> bool {
    let Some(word) = previous_word(t, at) else {
        return false;
    };
    // « in 2048 tokens », « by 1500 users » : une quantité, pas une année.
    let next = t[at..]
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == ',')
        .trim_start();
    let next_word: String = next.chars().take_while(|c| c.is_alphabetic()).collect();
    if next_word.len() > 2 && next_word.ends_with('s') && next_word.starts_with(char::is_lowercase)
    {
        return false;
    }
    if YEAR_CONTEXT.contains(&word.as_str()) || month_index(&word).is_some() {
        return true;
    }
    if word == "of" {
        let before = t[..at].trim_end();
        let before = &before[..before.len() - 2];
        return previous_word(before, before.len()).is_some_and(|w| OF_YEAR.contains(&w.as_str()));
    }
    // « between 1990 and 2024 », « in 2020 or 2021 » : après une autre année
    // (elle-même en contexte).
    if !matches!(word.as_str(), "and" | "or" | "to" | "through") {
        return false;
    }
    let before = t[..at].trim_end();
    let before = before[..before.len() - word.len()].trim_end();
    let digits: String = before
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect();
    let start = before.len() - digits.len();
    digits.len() == 4
        && !before[..start].ends_with(|c: char| is_word(c) || c == '.' || c == ',')
        && digits
            .chars()
            .rev()
            .collect::<String>()
            .parse()
            .is_ok_and(|n: u64| (1100..=2099).contains(&n))
        && is_year_context(t, start)
}

fn numbers(t: &str) -> String {
    // 1990s → « nineteen nineties ».
    let t = sub(t, re!(r"\b(?P<y>1[1-9][0-9]0|20[0-9]0)s\b"), |c, _| {
        let words = year(parse_int(&c["y"])?);
        Some(match words.strip_suffix('y') {
            Some(stem) => format!("{stem}ies"),
            None => format!("{words}s"),
        })
    });
    // Intervalles : « 10-20 » → « 10 to 20 » ; années des deux côtés : « 2020-2024 ».
    let t = sub(&t, re!(r"([0-9]+) ?[-–] ?([0-9]+)"), |c, t| {
        let m = c.get(0).unwrap();
        let mut chars = t[m.end()..].chars();
        let digit_follows = match chars.next() {
            Some(c) if c.is_ascii_digit() => true,
            Some(c) if is_word(c) || c == '.' || c == ',' => {
                chars.next().is_some_and(|c| c.is_ascii_digit())
            }
            _ => false,
        };
        if !not_after(t, m.start(), ".,") || digit_follows {
            return None;
        }
        let years = [&c[1], &c[2]]
            .iter()
            .all(|s| s.len() == 4 && parse_int(s).is_some_and(|n| (1100..=2099).contains(&n)));
        Some(
            if is_subtraction(t, m.start(), m.end(), &["equals", "plus", "minus", "times"]) {
                format!("{} minus {}", &c[1], &c[2])
            } else if years {
                format!("{} to {}", year(parse_int(&c[1])?), year(parse_int(&c[2])?))
            } else {
                format!("{} to {}", &c[1], &c[2])
            },
        )
    });
    let t = sub(&t, re!("[-−]"), |c, t| {
        let m = c.get(0).unwrap();
        (not_after(t, m.start(), "") && char_after(t, m.end()).is_some_and(|c| c.is_ascii_digit()))
            .then(|| "minus ".to_owned())
    });
    // « 2 million € » → « 2 million euros ».
    let t = sub(
        &t,
        re!(r"\b(?P<w>thousand|million|billion|trillion) ?(?P<u>[€$£])"),
        |c, t| {
            not_before(t, c.get(0).unwrap().end(), "").then(|| {
                let plural = UNITS.iter().find(|u| u.0 == &c["u"]).map_or("", |u| u.2);
                format!("{} {plural}", &c["w"])
            })
        },
    );
    // Montant avant le nombre : $15, $1.5 million, €20, $9.99/month.
    let t = sub_to(
        &t,
        re!(concat!(r"(?P<unit>[$€£])\s?", num!())),
        |c, t| {
            let m = c.get(0).unwrap();
            if !not_before(t, m.end(), "") {
                return None;
            }
            let scale = next_word(t, m.end()).filter(|w| SCALES.contains(w));
            let end = scale.map_or(m.end(), |w| {
                t[m.end()..]
                    .find(w)
                    .map_or(m.end(), |i| m.end() + i + w.len())
            });
            let words = quantity(
                &c["int"],
                c.name("frac").map(|f| f.as_str()),
                &c["unit"],
                scale,
            );
            let (per, len) = per_period(&t[end..], "per");
            Some((words + &per, end + len))
        },
    );
    // Nombre + unité.
    let t = sub_to(&t, re!(concat!(num!(), r"(?: )?")), |c, t| {
        let m = c.get(0).unwrap();
        if !not_after(t, m.start(), ".,") {
            return None;
        }
        let rest = &t[m.end()..];
        let (symbol, ..) = unit_order()
            .iter()
            .find(|u| rest.starts_with(u.0) && unit_ends(&rest[u.0.len()..]))?;
        let words = quantity(&c["int"], c.name("frac").map(|f| f.as_str()), symbol, None);
        let (per, len) = per_period(&rest[symbol.len()..], "per");
        Some((words + &per, m.end() + symbol.len() + len))
    });
    // Décimaux, années en contexte, entiers.
    sub(&t, re!(num!()), |c, t| {
        let m = c.get(0).unwrap();
        if !not_after(t, m.start(), ".,") || !not_before(t, m.end(), "") {
            return None;
        }
        if c.name("frac").is_none() && c["int"].len() == 4 && is_year_context(t, m.start()) {
            return Some(year(parse_int(&c["int"])?));
        }
        Some(number_words(&c["int"], c.name("frac").map(|f| f.as_str())))
    })
}

fn words(t: &str) -> String {
    let t = sub(t, re!(r"\b[A-Za-z0-9]+\b"), |c, _| {
        let w = &c[0];
        if !(w.bytes().any(|b| b.is_ascii_digit()) && w.bytes().any(|b| b.is_ascii_alphabetic())) {
            return Some(w.to_owned());
        }
        let runs = re!(r"[0-9]+|[A-Za-z]+").find_iter(w).map(|run| {
            let run = run.as_str();
            if run.as_bytes()[0].is_ascii_digit() {
                cardinal_str(&int_value(run))
            } else if run.chars().all(|c| c.is_ascii_uppercase()) || run.len() <= 2 {
                spell(run)
            } else {
                run.to_owned()
            }
        });
        Some(runs.collect::<Vec<_>>().join(" "))
    });
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
    // « DON'T », « CAN'T », « IT'S » : des contractions, lues comme des mots.
    let t = sub(&t, re!(r"\b[A-Z]+'(?:T|S|RE|LL|VE|M|D)\b"), |c, _| {
        Some(c[0].to_lowercase())
    });
    acronyms(
        &t,
        re!("[A-Z]{2,}"),
        |c| "AEIOUY".contains(c),
        crate::Language::En,
        COMMON_WORDS,
        &["IT", "US"],
    )
}

#[cfg(test)]
mod tests {
    use super::{cardinal, normalize, ordinal, year};

    #[test]
    fn numbers_ordinals_and_years() {
        assert_eq!(cardinal(120), "one hundred twenty");
        assert_eq!(cardinal(10_000), "ten thousand");
        assert_eq!(
            cardinal(1_234_567),
            "one million two hundred thirty-four thousand five hundred sixty-seven"
        );
        assert_eq!(ordinal(1), "first");
        assert_eq!(ordinal(12), "twelfth");
        assert_eq!(ordinal(20), "twentieth");
        assert_eq!(ordinal(21), "twenty-first");
        assert_eq!(ordinal(100), "one hundredth");
        assert_eq!(year(2025), "twenty twenty-five");
        assert_eq!(year(1990), "nineteen ninety");
        assert_eq!(year(1905), "nineteen oh five");
        assert_eq!(year(2005), "two thousand five");
        assert_eq!(year(1900), "nineteen hundred");
        assert_eq!(year(2000), "two thousand");
    }

    /// Constats de la relecture (61, R3-*).
    #[test]
    fn review_cases() {
        let cases = [
            // R3-2 : the pronoun I.
            (
                "The part I don't understand is the book I read.",
                "The part I don't understand is the book I read.",
            ),
            (
                "Tell John I said hi, and ask Mary I guess.",
                "Tell John I said hi, and ask Mary I guess.",
            ),
            (
                "World War I, Part I, Elizabeth I. Then Henry V won.",
                "World War one, Part one, Elizabeth the First. Then Henry the Fifth won.",
            ),
            // R3-4 : unit followed by an apostrophe.
            ("The 2 m's are equal.", "The two m's are equal."),
            // R3-5 : acronym joined by a hyphen.
            (
                "Use GPT-4 with UTF-8 and SHA-256 over USB-C.",
                "Use G P T four with U T F eight and S H A two hundred fifty-six over U S B C.",
            ),
            // R3-6 : common words in capitals.
            (
                "It is VERY important, DON'T. I AM SURE THIS IS FINE.",
                "It is very important, don't. I am sure this is fine.",
            ),
            (
                "The IT team and the US office use an API.",
                "The I T team and the U S office use an A P I.",
            ),
            // R3-7 : subtraction.
            (
                "10 - 3 = 7 and pages 10-20.",
                "ten minus three equals seven and pages ten to twenty.",
            ),
            // R3-8 : scientific notation.
            (
                "A rate of 1e-5 or 2.5e-3.",
                "A rate of ten to the minus five or two point five times ten to the minus three.",
            ),
            // R3-10 : etc in a path.
            ("Edit /etc/hosts, etc.", "Edit /etc/hosts, et cetera."),
            // R3-13 : slashes.
            (
                "It costs $9.99/month, runs 24/7, 2/3 of users, scored 8/10, on 12/25.",
                "It costs nine dollars and ninety-nine cents per month, runs twenty-four seven, two thirds of users, scored eight out of ten, on December twenty-fifth.",
            ),
            // R3-15 : names with symbols.
            (
                "In C# and C++, like F#. The .NET framework.",
                "In C sharp and C plus plus, like F sharp. The dotnet framework.",
            ),
            // R3-16 : counts after of, in, by.
            (
                "Use a context window of 2048 tokens, in 1500 steps, by the end of 2025.",
                "Use a context window of two thousand forty-eight tokens, in one thousand five hundred steps, by the end of twenty twenty-five.",
            ),
            // R3-17 : duration, ratio, verse.
            (
                "A 16:9 screen, John 3:16, at 16:10, lasting 1:30:45.",
                "A sixteen by nine screen, John three sixteen, at sixteen ten, lasting one hour thirty minutes forty-five seconds.",
            ),
            // R3-23 : currency after a scale word.
            (
                "It costs 2 million € at 77°F.",
                "It costs two million euros at seventy-seven degrees Fahrenheit.",
            ),
            // R3-24 : sentence end after p.m.
            (
                "At 2:30 p.m. Then we eat at 6:00 p.m. sharp.",
                "At two thirty p m. Then we eat at six p m sharp.",
            ),
        ];
        for (given, expected) in cases {
            assert_eq!(normalize(given), expected, "input: {given}");
        }
    }

    #[test]
    fn contract_cases() {
        let cases = [
            (
                "On October 9, 2026, version 0.9.0 weighed 3.5 GB.",
                "On October ninth, twenty twenty-six, version zero point nine point zero weighed three point five gigabytes.",
            ),
            (
                "It costs about $15 per month, which is 20% less than in 2025.",
                "It costs about fifteen dollars per month, which is twenty percent less than in twenty twenty-five.",
            ),
            (
                "The REST API responds in 120 ms on an NVIDIA GPU, and Dr. Smith's team ships updates every Friday.",
                "The rest A P I responds in one hundred twenty milliseconds on an en vidia G P U, and Doctor Smith's team ships updates every Friday.",
            ),
            // Années selon le contexte ; nombres ordinaires sinon.
            (
                "Since 1990, by 2005 and in 1905, we had 1500 users and 2024 files.",
                "Since nineteen ninety, by two thousand five and in nineteen oh five, we had one thousand five hundred users and two thousand twenty-four files.",
            ),
            (
                "From 2020-2024 and in the 1990s.",
                "From twenty twenty to twenty twenty-four and in the nineteen nineties.",
            ),
            (
                "Between 1990 and 2024, 1500 and 2010 users.",
                "Between nineteen ninety and twenty twenty-four, one thousand five hundred and two thousand ten users.",
            ),
            // Milliers, décimaux, versions.
            (
                "We saw 10,000 requests, pi is 3.14, and Python 3.12.1 is out.",
                "We saw ten thousand requests, pi is three point one four, and Python three point twelve point one is out.",
            ),
            // Monnaies.
            (
                "Pay $2.50, $1, $0.99, €20, £5 or 20 €.",
                "Pay two dollars and fifty cents, one dollar, ninety-nine cents, twenty euros, five pounds or twenty euros.",
            ),
            (
                "A $1.5 million deal and $1,000 fees.",
                "A one point five million dollars deal and one thousand dollars fees.",
            ),
            // Unités.
            (
                "Drive 100 km/h for 1 h, store 1 GB, at -5 °C and 60 fps.",
                "Drive one hundred kilometers per hour for one hour, store one gigabyte, at minus five degrees Celsius and sixty frames per second.",
            ),
            // Heures.
            (
                "Meet at 2:30 PM, 9:05, 9:00 or 5pm.",
                "Meet at two thirty p m, nine oh five, nine o'clock or five p m.",
            ),
            (
                "Call at 10:15 a.m. tomorrow, or at 7 p.m. Then rest.",
                "Call at ten fifteen a m tomorrow, or at seven p m. Then rest.",
            ),
            // Dates.
            (
                "Due 2026-10-09, 10/09/2026 or 25/12/2026, and 9 October 2026.",
                "Due October ninth, twenty twenty-six, October ninth, twenty twenty-six or December twenty-fifth, twenty twenty-six, and the ninth of October, twenty twenty-six.",
            ),
            // Ordinaux.
            (
                "The 1st, 2nd, 3rd and 21st place.",
                "The first, second, third and twenty-first place.",
            ),
            // Romains.
            (
                "Henry VIII, Elizabeth II, World War II, Chapter IV and Super Bowl LVIII.",
                "Henry the Eighth, Elizabeth the Second, World War two, Chapter four and Super Bowl fifty-eight.",
            ),
            ("Plan C and Part V.", "Plan C and Part five."),
            // Abréviations.
            (
                "Mr. and Mrs. Lee, e.g. apples, i.e. fruit, vs. pears, No. 5, approx. 3 kg, etc.",
                "Mister and Missus Lee, for example, apples, that is, fruit, versus pears, number five, approximately three kilograms, et cetera.",
            ),
            // Lexique, sigles, mots en capitales.
            (
                "Open the JSON in ChatGPT on macOS via npm; do NOT use HTTP, IMPORTANT.",
                "Open the jason in chat G P T on mac O S via N P M; do not use H T T P, important.",
            ),
            // Intervalles, négatifs, symboles.
            (
                "Use 10-20 GB, ~5 ms, 1920x1080 and/or -3 & more @Ana → done #1.",
                "Use ten to twenty gigabytes, about five milliseconds, one thousand nine hundred twenty by one thousand eighty and or minus three and more at Ana, done number one.",
            ),
            // Lettres et chiffres, options, émojis.
            (
                "An MP3 in 4K on x86, run --verbose 🚀.",
                "An M P three in four K on X eighty-six, run verbose.",
            ),
            (
                "The IP 192.168.0.1 and v2.1.",
                "The I P one hundred ninety-two point one hundred sixty-eight point zero point one and two point one.",
            ),
            ("OK, TODO: fix the bug.", "okay, to do: fix the bug."),
        ];
        for (given, expected) in cases {
            assert_eq!(normalize(given), expected, "input: {given}");
        }
    }
}
