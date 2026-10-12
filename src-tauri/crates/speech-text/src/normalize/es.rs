//! Prétraitement (es) : contrat 59 § 3, sur le modèle du français (mêmes
//! passes, même style de sigles « A P I »).
//!
//! Accords : « un » devant un nom masculin (« veintiún años »), « una »
//! devant un féminin (« veintiuna horas », « doscientas personas ») ; le
//! genre vient d'une liste et des terminaisons (-a, -ción, -dad…), avec les
//! exceptions courantes (día, problema, sistema…). « uno » seul ou devant un
//! mot outil (« el 1 de enero »).

use super::common::{
    acronyms, char_after, char_before, expand_lexicon, finish, in_path, int_value, is_subtraction,
    is_word, next_word, not_after, not_before, parse_int, per_period, prepare, ratios, re, replace,
    roman_in_context, scientific, sp, spell, strip_spaces, sub, sub_to, symbol_names, unit_ends,
};

// ---------------------------------------------------------------- nombres

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Form {
    /// « uno », « veintiuno » : nombre seul.
    Masc,
    /// « un », « veintiún » : devant un nom masculin.
    Apoc,
    /// « una », « veintiuna », « doscientas ».
    Fem,
}

const ONES: [&str; 30] = [
    "cero",
    "uno",
    "dos",
    "tres",
    "cuatro",
    "cinco",
    "seis",
    "siete",
    "ocho",
    "nueve",
    "diez",
    "once",
    "doce",
    "trece",
    "catorce",
    "quince",
    "dieciséis",
    "diecisiete",
    "dieciocho",
    "diecinueve",
    "veinte",
    "veintiuno",
    "veintidós",
    "veintitrés",
    "veinticuatro",
    "veinticinco",
    "veintiséis",
    "veintisiete",
    "veintiocho",
    "veintinueve",
];
const TENS: [&str; 10] = [
    "",
    "",
    "",
    "treinta",
    "cuarenta",
    "cincuenta",
    "sesenta",
    "setenta",
    "ochenta",
    "noventa",
];

fn below100(n: u64, form: Form) -> String {
    if n < 30 {
        return match (n, form) {
            (1, Form::Apoc) => "un".to_owned(),
            (1, Form::Fem) => "una".to_owned(),
            (21, Form::Apoc) => "veintiún".to_owned(),
            (21, Form::Fem) => "veintiuna".to_owned(),
            _ => ONES[n as usize].to_owned(),
        };
    }
    let (t, u) = (n / 10, n % 10);
    if u == 0 {
        TENS[t as usize].to_owned()
    } else {
        format!("{} y {}", TENS[t as usize], below100(u, form))
    }
}

fn below1000(n: u64, form: Form) -> String {
    let (h, r) = (n / 100, n % 100);
    if h == 0 {
        return below100(r, form);
    }
    let mut head = match h {
        1 if r == 0 => "cien".to_owned(),
        1 => "ciento".to_owned(),
        5 => "quinientos".to_owned(),
        7 => "setecientos".to_owned(),
        9 => "novecientos".to_owned(),
        _ => format!("{}cientos", ONES[h as usize]),
    };
    if form == Form::Fem && h >= 2 {
        head = head.replace("ientos", "ientas");
    }
    if r == 0 {
        head
    } else {
        format!("{head} {}", below100(r, form))
    }
}

fn cardinal(n: u64, form: Form) -> String {
    if n == 0 {
        return "cero".to_owned();
    }
    if n >= 1_000_000_000_000_000_000 {
        return spell_digits(&n.to_string());
    }
    let mut parts = Vec::new();
    let billions = n / 1_000_000_000_000;
    if billions == 1 {
        parts.push("un billón".to_owned());
    } else if billions > 1 {
        parts.push(format!("{} billones", cardinal(billions, Form::Apoc)));
    }
    let millions = n / 1_000_000 % 1_000_000;
    if millions == 1 {
        parts.push("un millón".to_owned());
    } else if millions > 1 {
        parts.push(format!("{} millones", cardinal(millions, Form::Apoc)));
    }
    let thousands = n / 1000 % 1000;
    if thousands == 1 {
        parts.push("mil".to_owned());
    } else if thousands > 1 {
        let form = if form == Form::Fem {
            Form::Fem
        } else {
            Form::Apoc
        };
        parts.push(format!("{} mil", below1000(thousands, form)));
    }
    if !n.is_multiple_of(1000) {
        parts.push(below1000(n % 1000, form));
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

fn cardinal_str(s: &str, form: Form) -> String {
    s.parse::<u64>()
        .map_or_else(|_| spell_digits(s), |n| cardinal(n, form))
}

/// Ordinal de 1 à 10, `None` au-delà (lu en cardinal).
fn ordinal(n: u64, form: Form) -> Option<String> {
    const WORDS: [&str; 10] = [
        "primero", "segundo", "tercero", "cuarto", "quinto", "sexto", "séptimo", "octavo",
        "noveno", "décimo",
    ];
    let word = *WORDS.get(usize::try_from(n).ok()?.checked_sub(1)?)?;
    Some(match form {
        Form::Fem => format!("{}a", &word[..word.len() - 1]),
        Form::Apoc if n == 1 || n == 3 => word[..word.len() - 1].to_owned(),
        _ => word.to_owned(),
    })
}

fn digits(s: &str) -> String {
    let rest = s.trim_start_matches('0');
    let mut words = vec!["cero".to_owned(); s.len() - rest.len()];
    if !rest.is_empty() {
        words.push(cardinal_str(rest, Form::Masc));
    }
    words.join(" ")
}

fn integer(s: &str, form: Form) -> String {
    let s = strip_spaces(s);
    if s.starts_with('0') && s.len() > 1 {
        digits(&s)
    } else {
        cardinal_str(&s, form)
    }
}

// ---------------------------------------------------------------- genre

/// Noms féminins hors des terminaisons régulières.
const FEMININE: &[&str] = &[
    "mano", "foto", "radio", "moto", "vez", "imagen", "red", "clase", "parte", "noche", "tarde",
    "mañana", "base", "llave", "fuente", "gente", "mujer", "frase", "nube", "calle", "muerte",
    "suerte", "torre", "flor", "ley", "luz", "piel", "sal", "miel", "leche", "carne", "nieve",
    "sangre", "serie", "especie",
];
/// Noms masculins en -a.
const MASCULINE_IN_A: &[&str] = &[
    "día",
    "mapa",
    "problema",
    "sistema",
    "tema",
    "programa",
    "idioma",
    "clima",
    "planeta",
    "esquema",
    "dilema",
    "drama",
    "poema",
    "lema",
    "diagrama",
    "fantasma",
    "pijama",
    "sofá",
    "telegrama",
    "código",
    "dato",
    "byte",
    "dálmata",
    "atleta",
    "astronauta",
    "pirata",
    "poeta",
    "profeta",
    "cometa",
];
/// Mots après lesquels un nombre reste « uno » (pas un nom).
const NOT_NOUNS: &[&str] = &[
    "de", "del", "a", "al", "y", "o", "u", "e", "en", "es", "son", "era", "fue", "que", "por",
    "para", "con", "sin", "se", "lo", "la", "el", "los", "las", "más", "menos", "hasta", "desde",
    "entre", "como", "sobre", "ya", "no", "sí", "si", "pero", "está", "están", "hay", "ha", "han",
    "he", "puede", "será", "sería", "tiene", "tienen", "fueron", "eran", "también", "solo", "sólo",
];

fn singular(word: &str) -> &str {
    word.strip_suffix("es")
        .filter(|w| w.ends_with(['n', 'd', 'r', 'l', 'z', 's']))
        .or_else(|| word.strip_suffix('s'))
        .unwrap_or(word)
}

fn is_feminine(word: &str) -> bool {
    let word = word.to_lowercase();
    let one = singular(&word);
    let candidates = [word.as_str(), one];
    // « -ista » (turista, dentista) : épicène, masculin par défaut.
    if candidates.iter().any(|w| MASCULINE_IN_A.contains(w)) || one.ends_with("ista") {
        return false;
    }
    if candidates.iter().any(|w| FEMININE.contains(w)) || word == "veces" {
        return true;
    }
    one.ends_with('a')
        || ["ción", "sión", "dad", "tad", "tud", "umbre"]
            .iter()
            .any(|s| one.ends_with(s) || word.ends_with(&format!("{s}es")))
}

/// Forme d'un nombre selon le mot qui le suit.
fn form_before(next: Option<&str>) -> Form {
    match next {
        Some(w)
            if w.chars().all(char::is_alphabetic)
                && w.chars().next().is_some_and(char::is_lowercase) =>
        {
            if NOT_NOUNS.contains(&w) {
                Form::Masc
            } else if is_feminine(w) {
                Form::Fem
            } else {
                Form::Apoc
            }
        }
        _ => Form::Masc,
    }
}

// ---------------------------------------------------------------- unités

/// Symbole : (singulier, pluriel, féminin).
const UNITS: &[(&str, &str, &str, bool)] = &[
    ("%", "por ciento", "por ciento", false),
    ("€", "euro", "euros", false),
    ("$", "dólar", "dólares", false),
    ("£", "libra", "libras", true),
    ("k€", "mil euros", "mil euros", false),
    ("M€", "millón de euros", "millones de euros", false),
    ("TB", "terabyte", "terabytes", false),
    ("GB", "gigabyte", "gigabytes", false),
    ("MB", "megabyte", "megabytes", false),
    ("KB", "kilobyte", "kilobytes", false),
    ("kB", "kilobyte", "kilobytes", false),
    (
        "GB/s",
        "gigabyte por segundo",
        "gigabytes por segundo",
        false,
    ),
    (
        "MB/s",
        "megabyte por segundo",
        "megabytes por segundo",
        false,
    ),
    ("Gb/s", "gigabit por segundo", "gigabits por segundo", false),
    ("Mb/s", "megabit por segundo", "megabits por segundo", false),
    ("Mbps", "megabit por segundo", "megabits por segundo", false),
    ("ms", "milisegundo", "milisegundos", false),
    ("s", "segundo", "segundos", false),
    ("min", "minuto", "minutos", false),
    ("h", "hora", "horas", true),
    ("km/h", "kilómetro por hora", "kilómetros por hora", false),
    ("km", "kilómetro", "kilómetros", false),
    ("m", "metro", "metros", false),
    ("cm", "centímetro", "centímetros", false),
    ("mm", "milímetro", "milímetros", false),
    ("kg", "kilogramo", "kilogramos", false),
    ("g", "gramo", "gramos", false),
    ("mg", "miligramo", "miligramos", false),
    ("L", "litro", "litros", false),
    ("l", "litro", "litros", false),
    ("ml", "mililitro", "mililitros", false),
    ("°C", "grado Celsius", "grados Celsius", false),
    ("°F", "grado Fahrenheit", "grados Fahrenheit", false),
    ("°", "grado", "grados", false),
    ("Hz", "hercio", "hercios", false),
    ("kHz", "kilohercio", "kilohercios", false),
    ("MHz", "megahercio", "megahercios", false),
    ("GHz", "gigahercio", "gigahercios", false),
    ("W", "vatio", "vatios", false),
    ("kW", "kilovatio", "kilovatios", false),
    ("kWh", "kilovatio hora", "kilovatios hora", false),
    ("V", "voltio", "voltios", false),
    ("mAh", "miliamperio hora", "miliamperios hora", false),
    ("px", "píxel", "píxeles", false),
    (
        "fps",
        "fotograma por segundo",
        "fotogramas por segundo",
        false,
    ),
    ("x", "vez", "veces", true),
];

fn unit_order() -> &'static [(&'static str, &'static str, &'static str, bool)] {
    static ORDER: std::sync::LazyLock<Vec<(&str, &str, &str, bool)>> =
        std::sync::LazyLock::new(|| {
            let mut units = UNITS.to_vec();
            units.sort_by_key(|u| std::cmp::Reverse(u.0.chars().count()));
            units
        });
    &ORDER
}

fn quantity(int_part: &str, sep: Option<&str>, frac: Option<&str>, symbol: &str) -> String {
    let (_, singular_name, plural_name, fem) = *UNITS
        .iter()
        .find(|u| u.0 == symbol)
        .expect("unidad conocida");
    let form = if fem { Form::Fem } else { Form::Apoc };
    let units = parse_int(int_part);
    if matches!(symbol, "€" | "$" | "£")
        && let Some(frac) = frac.filter(|f| f.len() <= 2)
    {
        // 2,50 € → « dos euros con cincuenta » ; 0,50 € → « cincuenta céntimos ».
        let cents: u64 = format!("{frac:0<2}").parse().unwrap_or(0);
        if units == Some(0) {
            let name = match symbol {
                "€" => "céntimos",
                "$" => "centavos",
                _ => "peniques",
            };
            return format!("{} {name}", cardinal(cents, Form::Apoc));
        }
        let name = if units == Some(1) {
            singular_name
        } else {
            plural_name
        };
        let main = format!("{} {name}", cardinal_str(&int_value(int_part), form));
        return if cents > 0 {
            format!("{main} con {}", cardinal(cents, Form::Masc))
        } else {
            main
        };
    }
    let one = units == Some(1) && frac.is_none_or(|f| f.chars().all(|c| c == '0'));
    let name = if one { singular_name } else { plural_name };
    let words = match frac {
        Some(frac) => {
            let word = if sep == Some(",") { "coma" } else { "punto" };
            format!("{} {word} {}", integer(int_part, Form::Masc), digits(frac))
        }
        None => integer(int_part, form),
    };
    // « un millón de euros », « dos millones de personas ».
    let whole_millions =
        frac.is_none() && units.is_some_and(|n| n > 0 && n.is_multiple_of(1_000_000));
    if whole_millions && !matches!(symbol, "%" | "x" | "k€" | "M€") {
        return format!("{words} de {name}");
    }
    format!("{words} {name}")
}

// ---------------------------------------------------------------- lexique

const LEXICON: &[(&str, &str)] = &[
    ("REST", "rest"),
    ("JSON", "yeison"),
    ("YAML", "yamel"),
    ("GIF", "gif"),
    ("RAM", "ram"),
    ("ROM", "rom"),
    ("SIM", "sim"),
    ("PIN", "pin"),
    ("NASA", "nasa"),
    ("OTAN", "otan"),
    ("ONU", "onu"),
    ("UNESCO", "unesco"),
    ("SIDA", "sida"),
    ("OVNI", "ovni"),
    ("ASCII", "asqui"),
    ("WIFI", "wifi"),
    ("COVID", "covid"),
    ("NVIDIA", "envidia"),
    ("Nvidia", "envidia"),
    ("NVMe", "{NVME}"),
    ("iOS", "{IOS}"),
    ("macOS", "mac {OS}"),
    ("npm", "{NPM}"),
    ("ChatGPT", "chat {GPT}"),
    ("OpenAI", "open {AI}"),
    ("ChatonLM", "libre {LM}"),
    ("NO", "no"),
    ("SÍ", "sí"),
    ("OK", "okey"),
    ("NOTA", "nota"),
    ("MUY", "muy"),
];

const MONTHS: [&str; 12] = [
    "enero",
    "febrero",
    "marzo",
    "abril",
    "mayo",
    "junio",
    "julio",
    "agosto",
    "septiembre",
    "octubre",
    "noviembre",
    "diciembre",
];

const REGNAL: &str = "Felipe|Carlos|Alfonso|Fernando|Isabel|Juan Pablo|Juan|Pablo|Benedicto|Pío|Luis|\
Enrique|Jaime|Pedro|León|Francisco|Gregorio|Juana|Sancho|Ramsés|Napoleón|Clemente|Inocencio";
const FEMININE_NAMES: &[&str] = &["Isabel", "Juana"];

/// Mots courants écrits en capitales pour insister, lus comme des mots (en
/// plus des mots outils) : « NO DEBES hacer ESTO ».
const COMMON_WORDS: &[&str] = &[
    "esto", "eso", "solo", "sólo", "cada", "debes", "debe", "nunca", "siempre", "nada", "bien",
    "mal", "ahora", "aquí", "antes", "después", "aún", "ojo", "nota", "muy", "nadie", "otra",
    "otro", "hacer", "haz", "usa", "uses", "fin", "alto", "tras", "sea", "son", "fue", "va", "ve",
];

/// Mots avant « 16:9 » qui en font une proportion.
const RATIO_WORDS: &[&str] = &[
    "pantalla",
    "formato",
    "relación",
    "proporción",
    "aspecto",
    "resolución",
];

/// Livres de la Bible : « Juan 3:16 » est un versículo.
const BIBLE_BOOKS: &[&str] = &[
    "Juan",
    "Mateo",
    "Marcos",
    "Lucas",
    "Génesis",
    "Éxodo",
    "Salmo",
    "Salmos",
    "Romanos",
    "Hechos",
    "Apocalipsis",
    "Proverbios",
    "Isaías",
    "Corintios",
    "Hebreos",
    "Gálatas",
    "Efesios",
];
const SECTIONS: &str = "siglo|capítulo|tomo|acto|parte|título|libro|volumen|artículo|fase|temporada|\
sección|anexo|apartado|nivel";

macro_rules! num {
    () => {
        concat!(
            r"(?P<int>[0-9]{1,3}(?:",
            sp!(),
            r"[0-9]{3})+|[0-9]+)(?:(?P<sep>[,.])(?P<frac>[0-9]+))?"
        )
    };
}

/// `text` (texte parlé espagnol) prêt pour la synthèse.
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
    let t = replace(t, re!(r"\bSrta\."), "señorita");
    let t = replace(&t, re!(r"\bSra\."), "señora");
    let t = replace(&t, re!(r"\bSr\."), "señor");
    let t = replace(&t, re!(r"\bDra\."), "doctora");
    let t = replace(&t, re!(r"\bDr\."), "doctor");
    let t = replace(&t, re!(r"\bUds\."), "ustedes");
    let t = replace(&t, re!(r"\bUd\."), "usted");
    let t = replace(&t, re!(r"\bp\.\s?ej\."), "por ejemplo");
    let t = replace(&t, re!(r"\baprox\."), "aproximadamente");
    let t = replace(&t, re!(r"\bnúm\."), "número");
    let t = replace(&t, re!(r"\bpág\."), "página");
    let t = replace(&t, re!(r"\bEE\.\s?UU\."), "Estados Unidos");
    let t = sub(&t, re!(r"\bvs\.?"), |c, t| {
        char_after(t, c.get(0).unwrap().end())
            .is_some_and(char::is_whitespace)
            .then(|| "contra".to_owned())
    });
    let t = sub(&t, re!(r"\betc\."), |c, t| {
        let m = c.get(0).unwrap();
        (!in_path(t, m.start(), m.end() - 1)
            && t[m.end()..]
                .trim_start()
                .starts_with(|c: char| c.is_lowercase() || matches!(c, ',' | ';' | ':')))
        .then(|| "etcétera".to_owned())
    });
    // « /etc/hosts » : un chemin.
    let t = sub(&t, re!(r"\betc\b"), |c, t| {
        let m = c.get(0).unwrap();
        (!in_path(t, m.start(), m.end())).then(|| "etcétera".to_owned())
    });
    let t = sub(
        &t,
        re!(concat!(r"\b[nN]\.?", sp!(), "?[º°]", sp!(), "?")),
        |c, t| {
            char_after(t, c.get(0).unwrap().end())
                .is_some_and(|c| c.is_ascii_digit())
                .then(|| "número ".to_owned())
        },
    );
    // a. m., p. m. après une heure.
    let t = sub(&t, re!(r"\b([ap])\.\s?m\."), |c, t| {
        let before = t[..c.get(0).unwrap().start()].trim_end();
        before.ends_with(|c: char| c.is_ascii_digit()).then(|| {
            if &c[1] == "a" {
                "de la mañana"
            } else {
                "de la tarde"
            }
            .to_owned()
        })
    });
    let t = replace(&t, re!(r"§\s?"), "sección ");
    let t = sub(&t, re!(concat!("[~≈]", sp!(), "?")), |c, t| {
        let m = c.get(0).unwrap();
        (not_after(t, m.start(), "") && char_after(t, m.end()).is_some_and(|c| c.is_ascii_digit()))
            .then(|| "aproximadamente ".to_owned())
    });
    let t = replace(&t, re!(r"\s?&\s?"), " y ");
    let t = replace(&t, re!(r"\s?@\s?"), " arroba ");
    let t = replace(&t, re!(r"\s?[→⇒]\s?"), ", ");
    let t = replace(&t, re!(r"\s=\s"), " igual a ");
    let t = replace(&t, re!(r"\s\+\s"), " más ");
    let t = replace(&t, re!(r"\s×\s"), " por ");
    let t = sub(&t, re!("[x×]"), |c, t| {
        let m = c.get(0).unwrap();
        (char_before(t, m.start()).is_some_and(|c| c.is_ascii_digit())
            && char_after(t, m.end()).is_some_and(|c| c.is_ascii_digit()))
        .then(|| " por ".to_owned())
    });
    let t = sub(&t, re!(r"/o\b"), |c, t| {
        char_before(t, c.get(0).unwrap().start())
            .is_some_and(is_word)
            .then(|| " o".to_owned())
    });
    sub(&t, re!("--?"), |c, t| {
        let m = c.get(0).unwrap();
        (not_after(t, m.start(), "-")
            && char_after(t, m.end()).is_some_and(|c| c.is_ascii_lowercase()))
        .then(String::new)
    })
}

fn romans(t: &str) -> String {
    // Felipe VI → sexto ; Alfonso XIII → trece ; Isabel II → segunda.
    let t = sub(
        t,
        re!(format!(r"\b(?P<w>{REGNAL}){}(?P<r>[IVXLCDM]+)\b", sp!())),
        |c, _| {
            let name = &c["w"];
            let Some(n) = roman_in_context(&c["r"]) else {
                return Some(c[0].to_owned());
            };
            let form = if FEMININE_NAMES.contains(&name) {
                Form::Fem
            } else {
                Form::Masc
            };
            let words =
                ordinal(u64::from(n), form).unwrap_or_else(|| cardinal(u64::from(n), Form::Masc));
            Some(format!("{name} {words}"))
        },
    );
    // siglo XXI → veintiuno ; s. XIX.
    let t = sub(
        &t,
        re!(concat!(r"\bs\.", sp!(), r"?(?P<r>[IVXLCDM]+)\b")),
        |c, _| {
            roman_in_context(&c["r"])
                .map(|n| format!("siglo {}", cardinal(u64::from(n), Form::Masc)))
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
                Some(n) => format!("{} {}", &c["w"], cardinal(u64::from(n), Form::Masc)),
                None => c[0].to_owned(),
            })
        },
    )
}

fn date(d: &str, mo: &str, y: &str) -> String {
    let month = MONTHS[parse_int(mo).unwrap_or(1) as usize - 1];
    format!(
        "{} de {month} de {}",
        cardinal_str(&int_value(d), Form::Masc),
        cardinal_str(&int_value(y), Form::Masc)
    )
}

fn dates_and_times(t: &str) -> String {
    // 1e-5 → « diez elevado a menos cinco ».
    let t = scientific(t, |mantissa, negative, exp| {
        let mut out = mantissa.map_or_else(String::new, |c| match c.name("frac") {
            Some(frac) => {
                let word = if &c["sep"] == "," { "coma" } else { "punto" };
                format!(
                    "{} {word} {} por ",
                    integer(&c["int"], Form::Masc),
                    digits(frac.as_str())
                )
            }
            None => format!("{} por ", integer(&c["int"], Form::Masc)),
        });
        out.push_str("diez elevado a ");
        if negative {
            out.push_str("menos ");
        }
        out.push_str(&integer(exp, Form::Masc));
        out
    });
    // Milliers à points (« 1.000.000 ») avant les versions : points retirés.
    // Jamais après un zéro (« 0.001 ») ; un seul groupe (« 1.500 ») seulement
    // devant une unité ou un nom, sinon c'est un décimal (« pi es 3.141 »).
    let t = sub(&t, re!(r"\b[0-9]{1,3}(?:\.[0-9]{3})+\b"), |c, t| {
        let m = c.get(0).unwrap();
        let rest = &t[m.end()..];
        let mut chars = rest.chars();
        let decimal_follows = matches!(chars.next(), Some('.' | ','))
            && chars.next().is_some_and(|c| c.is_ascii_digit());
        let groups = m.as_str().matches('.').count();
        let after = rest.trim_start_matches([' ', '\u{a0}', '\u{202f}']);
        let before_unit = unit_order()
            .iter()
            .any(|u| after.starts_with(u.0) && unit_ends(&after[u.0.len()..]))
            || matches!(form_before(next_word(t, m.end())), Form::Apoc | Form::Fem)
            || ["mil", "millón", "millones"]
                .iter()
                .any(|w| after.starts_with(w));
        (not_after(t, m.start(), ".,")
            && !decimal_follows
            && !m.as_str().starts_with('0')
            && (groups > 1 || before_unit))
            .then(|| m.as_str().replace('.', ""))
    });
    let t = sub(
        &t,
        re!(r"\b[vV]?(?P<v>[0-9]+(?:\.[0-9]+){2,})\b|\b[vV](?P<w>[0-9]+\.[0-9]+)\b"),
        |c, _| {
            let version = c.name("v").or(c.name("w")).unwrap().as_str();
            Some(
                version
                    .split('.')
                    .map(digits)
                    .collect::<Vec<_>>()
                    .join(" punto "),
            )
        },
    );
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
    let t = replace(&t, re!(r"\b24/7\b"), "veinticuatro siete");
    // N/M : fraction usuelle, note, date sans année, sinon « sobre ».
    let t = sub(
        &t,
        re!(r"\b(?P<n>[0-9]{1,3})/(?P<m>[0-9]{1,3})\b"),
        |c, t| {
            let m = c.get(0).unwrap();
            if !not_after(t, m.start(), "./") || !not_before(t, m.end(), "/") {
                return None;
            }
            let (n, d) = (parse_int(&c["n"])?, parse_int(&c["m"])?);
            let over = || {
                format!(
                    "{} sobre {}",
                    cardinal(n, Form::Masc),
                    cardinal(d, Form::Masc)
                )
            };
            Some(match (n, d) {
                (1, 2) => "un medio".to_owned(),
                (1, 3) => "un tercio".to_owned(),
                (2, 3) => "dos tercios".to_owned(),
                (1, 4) => "un cuarto".to_owned(),
                (3, 4) => "tres cuartos".to_owned(),
                (_, 5 | 10 | 20 | 100) if n <= d => over(),
                (1..=31, 1..=12) => {
                    format!("{} de {}", cardinal(n, Form::Masc), MONTHS[d as usize - 1])
                }
                _ => over(),
            })
        },
    );
    // « écran 16:10 », « 16:9 » : un rapport.
    let t = ratios(&t, RATIO_WORDS, |a, b| {
        format!("{} a {}", cardinal(a, Form::Masc), cardinal(b, Form::Masc))
    });
    // Durée 1:30:45.
    let t = sub(
        &t,
        re!(r"\b(?P<h>[0-9]{1,2}):(?P<m>[0-5][0-9]):(?P<s>[0-5][0-9])\b"),
        |c, _| {
            let parts = [
                (&c["h"], "hora", "horas", Form::Fem),
                (&c["m"], "minuto", "minutos", Form::Apoc),
                (&c["s"], "segundo", "segundos", Form::Apoc),
            ];
            let words: Vec<String> = parts
                .iter()
                .filter_map(|(v, one, many, form)| {
                    let n = parse_int(v)?;
                    (n > 0).then(|| {
                        format!("{} {}", cardinal(n, *form), if n == 1 { one } else { many })
                    })
                })
                .collect();
            Some(if words.is_empty() {
                "cero segundos".to_owned()
            } else {
                words.join(" ")
            })
        },
    );
    // 14:30 → « catorce treinta » ; 9:05 → « nueve y cinco » ; 14:00 → « catorce en punto ».
    let t = sub(
        &t,
        re!(r"\b(?P<h>[01]?[0-9]|2[0-3])(?::|h)(?P<m>[0-5][0-9])\b"),
        |c, t| {
            let whole = c.get(0).unwrap();
            if whole.as_str().contains(':') {
                // « Juan 3:16 » : un versículo.
                let (a, b) = (parse_int(&c["h"])?, parse_int(&c["m"])?);
                let word_before = t[..whole.start()]
                    .trim_end()
                    .rsplit(' ')
                    .next()
                    .unwrap_or("");
                if BIBLE_BOOKS.contains(&word_before) {
                    return Some(format!(
                        "{} {}",
                        cardinal(a, Form::Masc),
                        cardinal(b, Form::Masc)
                    ));
                }
            }
            let h = cardinal(parse_int(&c["h"]).unwrap_or(0), Form::Fem);
            let m = parse_int(&c["m"]).unwrap_or(0);
            Some(match m {
                0 => format!("{h} en punto"),
                1..=9 => format!("{h} y {}", cardinal(m, Form::Fem)),
                _ => format!("{h} {}", cardinal(m, Form::Fem)),
            })
        },
    );
    // 1.º, 1.ª, 1er, 3er, 2.ª ; abrégés à l'américaine : 1ro, 2do, 4to, 2da.
    sub(
        &t,
        re!(r"\b(?P<n>[0-9]+)\.?(?P<suf>º|ª|er|ra|ro|do|to|vo|no|mo|da|ta|va|na|ma)\b"),
        |c, t| {
            let n = parse_int(&c["n"])?;
            let fem = matches!(&c["suf"], "ª") || c["suf"].ends_with('a');
            let form = if fem {
                Form::Fem
            } else if &c["suf"] == "er"
                || form_before(next_word(t, c.get(0).unwrap().end())) == Form::Apoc
            {
                Form::Apoc
            } else {
                Form::Masc
            };
            Some(
                ordinal(n, form)
                    .unwrap_or_else(|| cardinal(n, if fem { Form::Fem } else { Form::Masc })),
            )
        },
    )
}

fn numbers(t: &str) -> String {
    let t = sub(
        t,
        re!(concat!(r"([0-9]+)", sp!(), "?[-–]", sp!(), r"?([0-9]+)")),
        |c, t| {
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
            // « 10 - 3 = 7 » : una resta.
            Some(
                if is_subtraction(t, m.start(), m.end(), &["igual", "más", "menos", "por"]) {
                    format!("{} menos {}", &c[1], &c[2])
                } else {
                    format!("{} a {}", &c[1], &c[2])
                },
            )
        },
    );
    let t = sub(&t, re!("[-−]"), |c, t| {
        let m = c.get(0).unwrap();
        (not_after(t, m.start(), "") && char_after(t, m.end()).is_some_and(|c| c.is_ascii_digit()))
            .then(|| "menos ".to_owned())
    });
    // « 2 millones € » → « 2 millones de euros ».
    let t = sub(
        &t,
        re!(concat!(
            r"\b(?P<w>mil|millón|millones)",
            sp!(),
            "?(?P<u>[€$£])"
        )),
        |c, t| {
            not_before(t, c.get(0).unwrap().end(), "").then(|| {
                let plural = UNITS.iter().find(|u| u.0 == &c["u"]).map_or("", |u| u.2);
                match &c["w"] {
                    "mil" => format!("mil {plural}"),
                    w => format!("{w} de {plural}"),
                }
            })
        },
    );
    let t = sub_to(&t, re!(concat!(num!(), "(?:", sp!(), ")?")), |c, t| {
        let m = c.get(0).unwrap();
        if !not_after(t, m.start(), ".,") {
            return None;
        }
        let rest = &t[m.end()..];
        let (symbol, ..) = unit_order()
            .iter()
            .find(|u| rest.starts_with(u.0) && unit_ends(&rest[u.0.len()..]))?;
        let words = quantity(
            &c["int"],
            c.name("sep").map(|s| s.as_str()),
            c.name("frac").map(|f| f.as_str()),
            symbol,
        );
        let (per, len) = per_period(&rest[symbol.len()..], "por");
        Some((words + &per, m.end() + symbol.len() + len))
    });
    let t = sub_to(
        &t,
        re!(concat!(r"(?P<unit>[$€£])", sp!(), "?", num!())),
        |c, t| {
            let m = c.get(0).unwrap();
            if !not_before(t, m.end(), "") {
                return None;
            }
            let words = quantity(
                &c["int"],
                c.name("sep").map(|s| s.as_str()),
                c.name("frac").map(|f| f.as_str()),
                &c["unit"],
            );
            let (per, len) = per_period(&t[m.end()..], "por");
            Some((words + &per, m.end() + len))
        },
    );
    sub(&t, re!(num!()), |c, t| {
        let m = c.get(0).unwrap();
        if !not_after(t, m.start(), ".,") || !not_before(t, m.end(), "") {
            return None;
        }
        let form = form_before(next_word(t, m.end()));
        Some(match c.name("frac") {
            Some(frac) => {
                let word = if &c["sep"] == "," { "coma" } else { "punto" };
                format!(
                    "{} {word} {}",
                    integer(&c["int"], Form::Masc),
                    digits(frac.as_str())
                )
            }
            None => integer(&c["int"], form),
        })
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
                cardinal_str(&int_value(run), Form::Masc)
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
    acronyms(
        &t,
        re!("[A-ZÁÉÍÓÚÑÜ]{2,}"),
        |c| "AEIOUÁÉÍÓÚ".contains(c),
        crate::Language::Es,
        COMMON_WORDS,
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::{Form, cardinal, normalize};

    #[test]
    fn cardinals() {
        let cases = [
            (0, Form::Masc, "cero"),
            (1, Form::Masc, "uno"),
            (1, Form::Apoc, "un"),
            (1, Form::Fem, "una"),
            (16, Form::Masc, "dieciséis"),
            (21, Form::Apoc, "veintiún"),
            (21, Form::Fem, "veintiuna"),
            (31, Form::Fem, "treinta y una"),
            (100, Form::Masc, "cien"),
            (101, Form::Masc, "ciento uno"),
            (200, Form::Fem, "doscientas"),
            (500, Form::Masc, "quinientos"),
            (999, Form::Masc, "novecientos noventa y nueve"),
            (1000, Form::Masc, "mil"),
            (2026, Form::Masc, "dos mil veintiséis"),
            (21_000, Form::Masc, "veintiún mil"),
            (200_000, Form::Fem, "doscientas mil"),
            (1_000_000, Form::Masc, "un millón"),
            (2_000_000, Form::Masc, "dos millones"),
            (1_000_000_000, Form::Masc, "mil millones"),
            (1_000_000_000_000, Form::Masc, "un billón"),
        ];
        for (n, form, words) in cases {
            assert_eq!(cardinal(n, form), words, "{n} {form:?}");
        }
    }

    /// Constats de la relecture (61, R3-*).
    #[test]
    fn review_cases() {
        let cases = [
            // R3-4 : unidad seguida de un apóstrofo : no es una unidad.
            ("Los 2 l'usan.", "Los dos l'usan."),
            // R3-5 : siglas unidas por un guion.
            (
                "Usa GPT-4 en UTF-8 con SHA-256.",
                "Usa G P T cuatro en U T F ocho con S H A doscientos cincuenta y seis.",
            ),
            // R3-6 : palabras comunes en mayúsculas.
            (
                "NO DEBES hacer ESTO, SOLO PARA CADA caso.",
                "no debes hacer esto, solo para cada caso.",
            ),
            // R3-7 : resta.
            (
                "Cálculo: 10 - 3 = 7.",
                "Cálculo: diez menos tres igual a siete.",
            ),
            // R3-8 : notación científica.
            (
                "Una tasa de 1e-5 o 2,5e-3.",
                "Una tasa de diez elevado a menos cinco o dos coma cinco por diez elevado a menos tres.",
            ),
            // R3-10 : « etc » en una ruta.
            ("Edita /etc/hosts, etc.", "Edita /etc/hosts, etcétera."),
            // R3-11 : decimales con punto de tres cifras.
            (
                "Una tasa de 0.001, pi es 3.141 y hay 1.500 personas.",
                "Una tasa de cero punto cero cero uno, pi es tres punto ciento cuarenta y uno y hay mil quinientas personas.",
            ),
            (
                "Una latencia de 0.005 segundos.",
                "Una latencia de cero punto cero cero cinco segundos.",
            ),
            // R3-12 : ordinales abreviados.
            (
                "El 1ro, el 2do, la 3ra, el 4to y el 5to lugar.",
                "El primero, el segundo, la tercera, el cuarto y el quinto lugar.",
            ),
            // R3-13 : barra.
            (
                "Cuesta 9,99 €/mes, 24/7, 1/2 kilo, nota 8/10, el 14/10.",
                "Cuesta nueve euros con noventa y nueve por mes, veinticuatro siete, un medio kilo, nota ocho sobre diez, el catorce de octubre.",
            ),
            // R3-15 : lenguajes con símbolo.
            (
                "En C# y C++ con .NET.",
                "En C sharp y C plus plus con dotnet.",
            ),
            // R3-17 : duración, proporción, versículo.
            (
                "Pantalla 16:9, Juan 3:16, a las 16:10, dura 1:30:45.",
                "Pantalla dieciséis a nueve, Juan tres dieciséis, a las dieciséis diez, dura una hora treinta minutos cuarenta y cinco segundos.",
            ),
            // R3-21 : género.
            (
                "1 mano, 1 foto, 101 dálmatas y 21 turistas.",
                "una mano, una foto, ciento un dálmatas y veintiún turistas.",
            ),
            // R3-23 : moneda tras « millones », °F.
            (
                "Cuesta 2 millones € a 77°F.",
                "Cuesta dos millones de euros a setenta y siete grados Fahrenheit.",
            ),
        ];
        for (given, expected) in cases {
            assert_eq!(normalize(given), expected, "entrada: {given}");
        }
    }

    #[test]
    fn contract_cases() {
        let cases = [
            (
                "El 9 de octubre de 2026, la versión 0.9.0 pesaba 3,5 GB.",
                "El nueve de octubre de dos mil veintiséis, la versión cero punto nueve punto cero pesaba tres coma cinco gigabytes.",
            ),
            (
                "Cuesta unos 15 € al mes, un 20 % menos que en 2025.",
                "Cuesta unos quince euros al mes, un veinte por ciento menos que en dos mil veinticinco.",
            ),
            (
                "La API REST responde en 120 ms con una GPU de NVIDIA.",
                "La A P I rest responde en ciento veinte milisegundos con una G P U de envidia.",
            ),
            // « un » / « una » selon le nom ; « uno » seul.
            (
                "Tengo 21 años y 1 perro; quedan 21 horas y 1 persona.",
                "Tengo veintiún años y un perro; quedan veintiuna horas y una persona.",
            ),
            (
                "Hay 200 personas y 1 problema; el capítulo 1 es corto.",
                "Hay doscientas personas y un problema; el capítulo uno es corto.",
            ),
            (
                "Unas 200 000 personas y 500 páginas.",
                "Unas doscientas mil personas y quinientas páginas.",
            ),
            // Milliers « 1.000 » et « 1 000 », millions.
            (
                "Son 1.000 € o 1 000 €; 2.500.000 habitantes.",
                "Son mil euros o mil euros; dos millones quinientos mil habitantes.",
            ),
            (
                "Un premio de 1.000.000 € y 3 000 000 $.",
                "Un premio de un millón de euros y tres millones de dólares.",
            ),
            // Décimaux : coma, punto.
            (
                "Pi vale 3,14 y Python 3.12 funciona.",
                "Pi vale tres coma catorce y Python tres punto doce funciona.",
            ),
            // Monnaies.
            (
                "Pagué 2,50 €, 0,50 €, 1 € y $15.",
                "Pagué dos euros con cincuenta, cincuenta céntimos, un euro y quince dólares.",
            ),
            ("Vale 1 £ o 21 £.", "Vale una libra o veintiuna libras."),
            // Unités.
            (
                "A 100 km/h, 1 GB, 1 h, -5 °C y 60 fps.",
                "A cien kilómetros por hora, un gigabyte, una hora, menos cinco grados Celsius y sesenta fotogramas por segundo.",
            ),
            // Heures.
            (
                "A las 14:30, a las 9:05 y a las 14:00.",
                "A las catorce treinta, a las nueve y cinco y a las catorce en punto.",
            ),
            (
                "Llega a las 7 p. m. hoy.",
                "Llega a las siete de la tarde hoy.",
            ),
            // Dates.
            (
                "Nació el 09/10/2026 y el 2026-01-01.",
                "Nació el nueve de octubre de dos mil veintiséis y el uno de enero de dos mil veintiséis.",
            ),
            // Ordinaux.
            (
                "El 1.º, la 2.ª, el 1er piso, el 3er día y el 11.º.",
                "El primero, la segunda, el primer piso, el tercer día y el once.",
            ),
            // Romains.
            (
                "En el siglo XXI, Felipe VI, Alfonso XIII e Isabel II; capítulo IV y s. XIX.",
                "En el siglo veintiuno, Felipe sexto, Alfonso trece e Isabel segunda; capítulo cuatro y siglo diecinueve.",
            ),
            (
                "Juan Pablo II visitó la parte C.",
                "Juan Pablo segundo visitó la parte C.",
            ),
            // Abréviations.
            (
                "El Sr. Pérez, la Sra. Gómez y la Dra. Ruiz, p. ej. hoy, aprox. 3 km, n.º 5, etc.",
                "El señor Pérez, la señora Gómez y la doctora Ruiz, por ejemplo hoy, aproximadamente tres kilómetros, número cinco, etcétera.",
            ),
            // Lexique, sigles, mots en capitales.
            (
                "Abre el JSON de ChatGPT en macOS con npm; IMPORTANTE: NO uses HTTPS.",
                "Abre el yeison de chat G P T en mac O S con N P M; importante: no uses H T T P S.",
            ),
            // Intervalles, négatifs, symboles.
            (
                "De 10-20 GB, ~5 ms, 1920x1080 y/o -3 & más @Ana → listo.",
                "De diez a veinte gigabytes, aproximadamente cinco milisegundos, mil novecientos veinte por mil ochenta y o menos tres y más arroba Ana, listo.",
            ),
            // Lettres et chiffres, options, émojis.
            (
                "Un MP3 en 4K, usa --verbose 🚀.",
                "Un M P tres en cuatro K, usa verbose.",
            ),
            (
                "La IP 192.168.0.1 y la v2.1.",
                "La I P ciento noventa y dos punto ciento sesenta y ocho punto cero punto uno y la dos punto uno.",
            ),
        ];
        for (given, expected) in cases {
            assert_eq!(normalize(given), expected, "entrada: {given}");
        }
    }
}
