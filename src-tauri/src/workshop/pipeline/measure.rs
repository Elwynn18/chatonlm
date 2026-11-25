//! Critères mesurables : un modèle ne sait pas compter (banc du 25 nov. : une
//! version de 3 239 mots jugée « environ 2 800 mots » et refusée). Le
//! programme reconnaît dans le texte d'un critère (français, anglais,
//! espagnol) un nombre de mots (bornes, « environ N » : ±10 %), de titres ou
//! de sections (titres Markdown comptés), la présence d'un tableau, le
//! nombre d'éléments d'une liste ; il les mesure lui-même (méthode
//! `Program`, preuve = la mesure) et les juges ne les voient plus.
//!
//! Seul un critère qui ne demande RIEN d'autre que la mesure est reconnu
//! (« le guide compte entre 3 300 et 3 700 mots », « au moins huit titres
//! Markdown ») ; « au moins 8 sections, une par étape » ou « chaque partie
//! fait 300 mots » restent aux juges. Logique pure, commune à Linux et
//! Windows. i18n-ignore (tout le fichier) : mots cherchés et codes.

use super::text::{body_of, folded, words};
use super::types::{Criterion, Spec};

/// Ce qui est compté.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Mots du corps (titre du document exclu, comme la longueur).
    Words,
    /// Titres Markdown ; `Some(n)` : de niveau `n` seulement.
    Headings(Option<u8>),
    /// Sections : titres du premier niveau sous le titre du document.
    Sections,
    /// Tableaux Markdown.
    Tables,
    /// Éléments de la liste à puces ou numérotée la mieux placée.
    Items,
}

impl Unit {
    /// Code stable (preuve d'une vérification : « 3239/3300-3700 words »).
    fn code(self) -> String {
        match self {
            Unit::Words => "words".into(),
            Unit::Headings(None) => "headings".into(),
            Unit::Headings(Some(level)) => format!("headings@{level}"),
            Unit::Sections => "sections".into(),
            Unit::Tables => "tables".into(),
            Unit::Items => "items".into(),
        }
    }

    fn of_code(code: &str) -> Option<Unit> {
        Some(match code {
            "words" => Unit::Words,
            "headings" => Unit::Headings(None),
            "sections" => Unit::Sections,
            "tables" => Unit::Tables,
            "items" => Unit::Items,
            other => {
                let level: u8 = other.strip_prefix("headings@")?.parse().ok()?;
                Unit::Headings(Some(level))
            }
        })
    }
}

/// Mesure attendue : bornes comprises (au moins une).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measure {
    pub unit: Unit,
    pub min: Option<u32>,
    pub max: Option<u32>,
}

impl Measure {
    /// Dans les bornes.
    pub fn holds(&self, have: u32) -> bool {
        self.min.is_none_or(|min| have >= min) && self.max.is_none_or(|max| have <= max)
    }

    /// Valeur visée : milieu des bornes, ou la seule borne.
    pub fn target(&self) -> u32 {
        match (self.min, self.max) {
            (Some(min), Some(max)) => min.saturating_add(max) / 2,
            (Some(bound), None) | (None, Some(bound)) => bound,
            (None, None) => 0,
        }
    }

    /// Écart relatif à la valeur visée ; 0 dans les bornes.
    pub fn gap(&self, have: u32) -> f64 {
        if self.holds(have) {
            return 0.0;
        }
        let target = self.target();
        if target == 0 {
            return 1.0;
        }
        (f64::from(have) / f64::from(target) - 1.0).abs()
    }

    /// Preuve d'une vérification : « mesuré/min-max unité » (borne absente :
    /// vide), relue par [`read_evidence`].
    pub fn evidence(&self, have: u32) -> String {
        let bound = |b: Option<u32>| b.map_or(String::new(), |n| n.to_string());
        format!(
            "{have}/{}-{} {}",
            bound(self.min),
            bound(self.max),
            self.unit.code()
        )
    }
}

/// Mesure et attente relues d'une preuve ([`Measure::evidence`]).
pub fn read_evidence(evidence: &str) -> Option<(u32, Measure)> {
    let (numbers, unit) = evidence.split_once(' ')?;
    let unit = Unit::of_code(unit)?;
    let (have, bounds) = numbers.split_once('/')?;
    let (min, max) = bounds.split_once('-')?;
    let bound = |s: &str| -> Option<Option<u32>> {
        if s.is_empty() {
            Some(None)
        } else {
            s.parse().ok().map(Some)
        }
    };
    let measure = Measure {
        unit,
        min: bound(min)?,
        max: bound(max)?,
    };
    (measure.min.is_some() || measure.max.is_some()).then_some((have.parse().ok()?, measure))
}

// ---------------------------------------------------------------------------
// Mesures sur un texte

/// Niveau d'un titre Markdown.
fn level(line: &str) -> Option<usize> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    ((1..=6).contains(&hashes) && line[hashes..].starts_with(' ')).then_some(hashes)
}

/// Lignes hors des blocs de code.
fn prose_lines(text: &str) -> Vec<&str> {
    let mut inside = false;
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            inside = !inside;
            continue;
        }
        if !inside {
            out.push(line);
        }
    }
    out
}

/// Niveaux des titres, dans l'ordre.
fn heading_levels(text: &str) -> Vec<usize> {
    prose_lines(text).into_iter().filter_map(level).collect()
}

/// Sections : titres du niveau le plus haut sous le niveau 1 ; sans eux,
/// titres de niveau 1 moins celui du document.
fn sections(text: &str) -> u32 {
    let levels = heading_levels(text);
    let count = match levels.iter().filter(|l| **l >= 2).min() {
        Some(top) => levels.iter().filter(|l| *l == top).count(),
        None => levels.len().saturating_sub(1),
    };
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Ligne de séparation d'un tableau (« |---|:--:| »).
fn separator(line: &str) -> bool {
    let line = line.trim();
    line.contains('-')
        && line.contains('|')
        && line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Tableaux : une ligne à barres suivie d'une ligne de séparation.
fn tables(text: &str) -> u32 {
    let lines = prose_lines(text);
    let count = lines
        .windows(2)
        .filter(|w| w[0].contains('|') && !separator(w[0]) && separator(w[1]))
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Élément de liste de premier niveau (au plus une espace devant).
fn top_item(line: &str) -> bool {
    let indent = line.len() - line.trim_start().len();
    if indent > 1 {
        return false;
    }
    let line = line.trim_start();
    if ["- ", "* ", "+ "].iter().any(|m| line.starts_with(m)) {
        return true;
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && (line[digits..].starts_with(". ") || line[digits..].starts_with(") "))
}

/// Éléments de chaque liste (lignes vides et lignes en retrait admises dans
/// une liste ; un titre ou un paragraphe la ferme).
fn lists(text: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut current = 0u32;
    for line in prose_lines(text) {
        if top_item(line) {
            current += 1;
        } else if line.trim().is_empty() || line.starts_with("  ") || line.starts_with('\t') {
            continue;
        } else if current > 0 {
            out.push(current);
            current = 0;
        }
    }
    if current > 0 {
        out.push(current);
    }
    out
}

/// Mesure de `text` pour `measure` : (valeur, tenue). Pour une liste : celle
/// qui tient, sinon la plus proche.
pub fn apply(text: &str, measure: &Measure) -> (u32, bool) {
    let have = match measure.unit {
        Unit::Words => words(body_of(text)),
        Unit::Headings(None) => u32::try_from(heading_levels(text).len()).unwrap_or(u32::MAX),
        Unit::Headings(Some(n)) => {
            let n = usize::from(n);
            u32::try_from(heading_levels(text).iter().filter(|l| **l == n).count())
                .unwrap_or(u32::MAX)
        }
        Unit::Sections => sections(text),
        Unit::Tables => tables(text),
        Unit::Items => {
            let counts = lists(text);
            let best = counts
                .iter()
                .copied()
                .find(|c| measure.holds(*c))
                .or_else(|| {
                    counts.iter().copied().min_by(|a, b| {
                        measure
                            .gap(*a)
                            .partial_cmp(&measure.gap(*b))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                });
            best.unwrap_or(0)
        }
    };
    (have, measure.holds(have))
}

// ---------------------------------------------------------------------------
// Reconnaissance d'un critère

/// Langues des mots cherchés.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    Fr,
    En,
    Es,
    Any,
}

impl Lang {
    fn of(code: &str) -> Self {
        match code {
            "fr" => Lang::Fr,
            "en" => Lang::En,
            "es" => Lang::Es,
            _ => Lang::Any,
        }
    }

    fn takes(self, of: Lang) -> bool {
        self == Lang::Any || self == of
    }
}

/// Nombres écrits en lettres (repliés).
const NUMBER_WORDS: &[(Lang, &str, u32)] = &[
    (Lang::Fr, "deux", 2),
    (Lang::Fr, "trois", 3),
    (Lang::Fr, "quatre", 4),
    (Lang::Fr, "cinq", 5),
    (Lang::Fr, "six", 6),
    (Lang::Fr, "sept", 7),
    (Lang::Fr, "huit", 8),
    (Lang::Fr, "neuf", 9),
    (Lang::Fr, "dix", 10),
    (Lang::Fr, "onze", 11),
    (Lang::Fr, "douze", 12),
    (Lang::Fr, "treize", 13),
    (Lang::Fr, "quatorze", 14),
    (Lang::Fr, "quinze", 15),
    (Lang::Fr, "seize", 16),
    (Lang::Fr, "vingt", 20),
    (Lang::Fr, "trente", 30),
    (Lang::En, "two", 2),
    (Lang::En, "three", 3),
    (Lang::En, "four", 4),
    (Lang::En, "five", 5),
    (Lang::En, "six", 6),
    (Lang::En, "seven", 7),
    (Lang::En, "eight", 8),
    (Lang::En, "nine", 9),
    (Lang::En, "ten", 10),
    (Lang::En, "eleven", 11),
    (Lang::En, "twelve", 12),
    (Lang::En, "thirteen", 13),
    (Lang::En, "fourteen", 14),
    (Lang::En, "fifteen", 15),
    (Lang::En, "sixteen", 16),
    (Lang::En, "twenty", 20),
    (Lang::En, "thirty", 30),
    (Lang::Es, "dos", 2),
    (Lang::Es, "tres", 3),
    (Lang::Es, "cuatro", 4),
    (Lang::Es, "cinco", 5),
    (Lang::Es, "seis", 6),
    (Lang::Es, "siete", 7),
    (Lang::Es, "ocho", 8),
    (Lang::Es, "nueve", 9),
    (Lang::Es, "diez", 10),
    (Lang::Es, "once", 11),
    (Lang::Es, "doce", 12),
    (Lang::Es, "trece", 13),
    (Lang::Es, "catorce", 14),
    (Lang::Es, "quince", 15),
    (Lang::Es, "dieciseis", 16),
    (Lang::Es, "veinte", 20),
    (Lang::Es, "treinta", 30),
];

/// Unités comptées (repliées) ; les éléments d'une liste demandent aussi un
/// mot de liste dans le critère ([`LIST_WORDS`]).
const UNITS: &[(Lang, &str, Unit)] = &[
    (Lang::Fr, "mots", Unit::Words),
    (Lang::Fr, "mot", Unit::Words),
    (Lang::En, "words", Unit::Words),
    (Lang::En, "word", Unit::Words),
    (Lang::Es, "palabras", Unit::Words),
    (Lang::Es, "palabra", Unit::Words),
    (Lang::Fr, "titres", Unit::Headings(None)),
    (Lang::Fr, "titre", Unit::Headings(None)),
    (Lang::Fr, "intertitres", Unit::Headings(None)),
    (Lang::En, "headings", Unit::Headings(None)),
    (Lang::En, "heading", Unit::Headings(None)),
    (Lang::En, "subheadings", Unit::Headings(None)),
    (Lang::En, "titles", Unit::Headings(None)),
    (Lang::Es, "titulos", Unit::Headings(None)),
    (Lang::Es, "subtitulos", Unit::Headings(None)),
    (Lang::Es, "encabezados", Unit::Headings(None)),
    (Lang::Fr, "sections", Unit::Sections),
    (Lang::Fr, "section", Unit::Sections),
    (Lang::Fr, "parties", Unit::Sections),
    (Lang::Fr, "chapitres", Unit::Sections),
    (Lang::En, "sections", Unit::Sections),
    (Lang::En, "section", Unit::Sections),
    (Lang::En, "parts", Unit::Sections),
    (Lang::En, "chapters", Unit::Sections),
    (Lang::Es, "secciones", Unit::Sections),
    (Lang::Es, "seccion", Unit::Sections),
    (Lang::Es, "partes", Unit::Sections),
    (Lang::Es, "capitulos", Unit::Sections),
    (Lang::Es, "apartados", Unit::Sections),
    (Lang::Fr, "tableaux", Unit::Tables),
    (Lang::Fr, "tableau", Unit::Tables),
    (Lang::En, "tables", Unit::Tables),
    (Lang::En, "table", Unit::Tables),
    (Lang::Es, "tablas", Unit::Tables),
    (Lang::Es, "tabla", Unit::Tables),
    (Lang::Fr, "elements", Unit::Items),
    (Lang::Fr, "points", Unit::Items),
    (Lang::Fr, "entrees", Unit::Items),
    (Lang::Fr, "items", Unit::Items),
    (Lang::En, "items", Unit::Items),
    (Lang::En, "points", Unit::Items),
    (Lang::En, "entries", Unit::Items),
    (Lang::En, "bullets", Unit::Items),
    (Lang::Es, "elementos", Unit::Items),
    (Lang::Es, "puntos", Unit::Items),
    (Lang::Es, "entradas", Unit::Items),
];

/// Mots d'une liste (le critère doit en contenir un pour compter des
/// éléments).
const LIST_WORDS: &[&str] = &[
    "liste",
    "listes",
    "list",
    "lists",
    "checklist",
    "lista",
    "listas",
    "puces",
    "bulleted",
    "numbered",
    "numerotee",
    "vinetas",
];

/// Ce qui suit un mot de tableau et n'en fait pas un tableau (« table des
/// matières »).
const NOT_TABLES: &[&str] = &[
    "des matieres",
    "of contents",
    "de contenidos",
    "de contenido",
    "de materias",
];

/// Négations devant un tableau (« sans tableau », « no table »).
const NEGATIONS: &[&str] = &[
    "sans", "aucun", "aucune", "pas", "ni", "no", "not", "without", "never", "sin", "ningun",
    "ninguna", "nunca",
];

/// Bornes devant un nombre (repliées, mots séparés par une espace).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bound {
    Min,
    MinStrict,
    Max,
    MaxStrict,
    About,
}

const BEFORE: &[(&str, Bound)] = &[
    ("pas moins de", Bound::Min),
    ("pas moins d", Bound::Min),
    ("no fewer than", Bound::Min),
    ("no less than", Bound::Min),
    ("not less than", Bound::Min),
    ("not fewer than", Bound::Min),
    ("au moins", Bound::Min),
    ("au minimum", Bound::Min),
    ("at least", Bound::Min),
    ("a minimum of", Bound::Min),
    ("al menos", Bound::Min),
    ("por lo menos", Bound::Min),
    ("como minimo", Bound::Min),
    ("minimum", Bound::Min),
    ("minimo", Bound::Min),
    ("pas plus de", Bound::Max),
    ("pas plus d", Bound::Max),
    ("no more than", Bound::Max),
    ("not more than", Bound::Max),
    ("au plus", Bound::Max),
    ("au maximum", Bound::Max),
    ("at most", Bound::Max),
    ("up to", Bound::Max),
    ("a maximum of", Bound::Max),
    ("como maximo", Bound::Max),
    ("no mas de", Bound::Max),
    ("jusqu a", Bound::Max),
    ("hasta", Bound::Max),
    ("maximum", Bound::Max),
    ("maximo", Bound::Max),
    ("ne depasse pas", Bound::Max),
    ("ne doit pas depasser", Bound::Max),
    ("sans depasser", Bound::Max),
    ("not exceed", Bound::Max),
    ("not exceeding", Bound::Max),
    ("without exceeding", Bound::Max),
    ("sin superar", Bound::Max),
    ("sin exceder", Bound::Max),
    ("no supera", Bound::Max),
    ("no debe superar", Bound::Max),
    ("plus de", Bound::MinStrict),
    ("plus d", Bound::MinStrict),
    ("more than", Bound::MinStrict),
    ("over", Bound::MinStrict),
    ("mas de", Bound::MinStrict),
    ("moins de", Bound::MaxStrict),
    ("moins d", Bound::MaxStrict),
    ("less than", Bound::MaxStrict),
    ("fewer than", Bound::MaxStrict),
    ("under", Bound::MaxStrict),
    ("menos de", Bound::MaxStrict),
    ("environ", Bound::About),
    ("approximativement", Bound::About),
    ("autour de", Bound::About),
    ("aux alentours de", Bound::About),
    ("pres de", Bound::About),
    ("about", Bound::About),
    ("around", Bound::About),
    ("approximately", Bound::About),
    ("roughly", Bound::About),
    ("some", Bound::About),
    ("aproximadamente", Bound::About),
    ("alrededor de", Bound::About),
    ("cerca de", Bound::About),
    ("unos", Bound::About),
    ("unas", Bound::About),
];

/// Bornes après l'unité (« 8 titres ou plus »).
const AFTER: &[(&str, Bound)] = &[
    ("ou plus", Bound::Min),
    ("or more", Bound::Min),
    ("o mas", Bound::Min),
    ("au moins", Bound::Min),
    ("at least", Bound::Min),
    ("minimum", Bound::Min),
    ("como minimo", Bound::Min),
    ("ou moins", Bound::Max),
    ("or less", Bound::Max),
    ("or fewer", Bound::Max),
    ("o menos", Bound::Max),
    ("au maximum", Bound::Max),
    ("maximum", Bound::Max),
    ("at most", Bound::Max),
    ("como maximo", Bound::Max),
];

/// Liaisons d'un intervalle (« entre 3 300 et 3 700 », « 3 300-3 700 »).
const RANGE_LINKS: &[&str] = &["et", "and", "y", "a", "to", "hasta", "-"];

/// Mots sans contenu dans un critère de mesure (repliés) : articles,
/// verbes « compter, contenir », noms du document, de la forme. Un critère
/// qui garde un autre mot n'est pas une simple mesure ; « chaque, each,
/// cada » n'en sont jamais (« chaque partie fait 300 mots » est une mesure
/// par partie, laissée aux juges).
const GENERIC: &[&str] = &[
    // français
    "le",
    "la",
    "les",
    "un",
    "une",
    "des",
    "du",
    "de",
    "en",
    "et",
    "au",
    "aux",
    "ce",
    "cet",
    "cette",
    "ces",
    "son",
    "sa",
    "ses",
    "leur",
    "leurs",
    "il",
    "elle",
    "on",
    "qu",
    "que",
    "qui",
    "lorsqu",
    "lorsque",
    "quand",
    "si",
    "se",
    "par",
    "pour",
    "sur",
    "dans",
    "avec",
    "hors",
    "ne",
    "est",
    "sont",
    "etre",
    "doit",
    "doivent",
    "devra",
    "devrait",
    "faut",
    "fait",
    "faire",
    "font",
    "compte",
    "comptent",
    "compter",
    "comptant",
    "comporte",
    "comportent",
    "comporter",
    "comprend",
    "comprennent",
    "comprendre",
    "compris",
    "contient",
    "contiennent",
    "contenir",
    "possede",
    "presente",
    "presenter",
    "inclut",
    "inclure",
    "inclus",
    "mesure",
    "mesurer",
    "totalise",
    "atteint",
    "atteindre",
    "respecte",
    "respecter",
    "document",
    "texte",
    "guide",
    "livrable",
    "rapport",
    "article",
    "fichier",
    "version",
    "contenu",
    "ensemble",
    "entier",
    "entiere",
    "complet",
    "complete",
    "total",
    "totale",
    "final",
    "finale",
    "longueur",
    "taille",
    "structure",
    "examine",
    "examiner",
    "examinant",
    "lit",
    "lire",
    "lisant",
    "markdown",
    "niveau",
    "caracteres",
    "caractere",
    "commencant",
    "commence",
    "diese",
    "signe",
    "forme",
    "sous",
    "corps",
    "principal",
    "principaux",
    "principales",
    "seulement",
    "uniquement",
    "moins",
    "plus",
    "maximum",
    "minimum",
    "environ",
    "elements",
    "element",
    "points",
    "liste",
    "listes",
    "puces",
    "numerotee",
    "numerotees",
    "titre",
    "titres",
    "mots",
    "mot",
    "partie",
    "chapitre",
    "long",
    "longue",
    // anglais
    "the",
    "an",
    "of",
    "in",
    "to",
    "at",
    "by",
    "for",
    "with",
    "and",
    "or",
    "is",
    "are",
    "be",
    "must",
    "should",
    "shall",
    "will",
    "has",
    "have",
    "contains",
    "contain",
    "include",
    "includes",
    "including",
    "included",
    "comprises",
    "comprise",
    "counts",
    "count",
    "counting",
    "length",
    "size",
    "text",
    "deliverable",
    "report",
    "file",
    "content",
    "whole",
    "entire",
    "complete",
    "full",
    "overall",
    "body",
    "level",
    "characters",
    "character",
    "starting",
    "start",
    "hash",
    "sign",
    "form",
    "when",
    "if",
    "it",
    "its",
    "examined",
    "examining",
    "reading",
    "read",
    "only",
    "main",
    "excluding",
    "exclude",
    "least",
    "most",
    "list",
    "lists",
    "bulleted",
    "numbered",
    "title",
    "heading",
    "words",
    "word",
    "items",
    "item",
    "entries",
    "part",
    "chapter",
    // espagnol
    "el",
    "los",
    "las",
    "lo",
    "del",
    "al",
    "es",
    "son",
    "ser",
    "debe",
    "deben",
    "tiene",
    "tienen",
    "tener",
    "contiene",
    "contienen",
    "contener",
    "incluye",
    "incluir",
    "cuenta",
    "contar",
    "mide",
    "medir",
    "documento",
    "texto",
    "guia",
    "entregable",
    "informe",
    "articulo",
    "archivo",
    "contenido",
    "entero",
    "entera",
    "completo",
    "completa",
    "longitud",
    "extension",
    "tamano",
    "estructura",
    "cuerpo",
    "nivel",
    "caracteres",
    "empieza",
    "empezando",
    "forma",
    "cuando",
    "solo",
    "principal",
    "incluidos",
    "excluidos",
    "con",
    "por",
    "para",
    "lista",
    "listas",
    "vinetas",
    "titulo",
    "palabras",
    "palabra",
    "parte",
    "capitulo",
    "largo",
    "larga",
];

/// Mots d'un critère (repliés), nombres en chiffres réunis (« 3 300 »,
/// « 3,300 », « 3.300 » → « 3300 ») ; un tiret entre deux nombres est gardé
/// (« - »).
fn tokens(text: &str) -> Vec<String> {
    let text = folded(text);
    // (séparateur avant, mot)
    let mut raw: Vec<(String, String)> = Vec::new();
    let mut sep = String::new();
    let mut word = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            word.push(c);
        } else {
            if !word.is_empty() {
                raw.push((std::mem::take(&mut sep), std::mem::take(&mut word)));
            }
            sep.push(c);
        }
    }
    if !word.is_empty() {
        raw.push((sep, word));
    }
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    let mut out: Vec<String> = Vec::new();
    // Le dernier mot est un nombre qui peut prendre un groupe de milliers.
    let mut open = false;
    let mut last_digits = false;
    for (sep, word) in raw {
        let thousands = matches!(sep.as_str(), " " | "\u{a0}" | "\u{202f}" | "," | ".");
        if open && thousands && word.len() == 3 && digits(&word) {
            if let Some(previous) = out.last_mut() {
                previous.push_str(&word);
            }
            continue;
        }
        if last_digits && digits(&word) && matches!(sep.trim(), "-" | "–" | "—") {
            out.push("-".into());
        }
        last_digits = digits(&word);
        open = last_digits && word.len() <= 3;
        out.push(word);
    }
    out
}

/// Valeur d'un nombre en chiffres ou en lettres.
fn number(token: &str, lang: Lang) -> Option<u32> {
    if token.chars().all(|c| c.is_ascii_digit()) {
        return token.parse().ok();
    }
    NUMBER_WORDS
        .iter()
        .find(|(l, w, _)| lang.takes(*l) && *w == token)
        .map(|(_, _, n)| *n)
}

fn unit(token: &str, lang: Lang) -> Option<Unit> {
    UNITS
        .iter()
        .find(|(l, w, _)| lang.takes(*l) && *w == token)
        .map(|(_, _, u)| *u)
}

/// Bornes de la plus longue tournure de `table` qui finit juste avant
/// `at` (ou commence juste après, `after`) : (borne, mots pris).
fn bound_near(
    tokens: &[String],
    at: usize,
    after: bool,
    table: &[(&str, Bound)],
) -> Option<(Bound, usize)> {
    let mut best: Option<(Bound, usize)> = None;
    for (phrase, bound) in table {
        let n = phrase.split(' ').count();
        let window = if after {
            tokens.get(at..at + n)
        } else {
            at.checked_sub(n).and_then(|start| tokens.get(start..at))
        };
        if window.is_some_and(|w| w.join(" ") == *phrase) && best.is_none_or(|(_, m)| n > m) {
            best = Some((*bound, n));
        }
    }
    best
}

/// Mesure demandée par un critère, s'il ne demande que cela (une seule
/// mesure, aucun autre mot de contenu) ; sinon `None` : aux juges.
pub fn parse(criterion: &str, language: &str) -> Option<Measure> {
    let lang = Lang::of(language);
    let tokens = tokens(criterion);
    let mut used = vec![false; tokens.len()];
    let mut found: Vec<Measure> = Vec::new();
    let list = tokens.iter().any(|t| LIST_WORDS.contains(&t.as_str()));

    // Niveau d'un titre (« de niveau 2 », « level 2 », « h2 »).
    let mut heading_level: Option<u8> = None;
    for (i, token) in tokens.iter().enumerate() {
        let level = if matches!(token.as_str(), "niveau" | "level" | "nivel") {
            tokens
                .get(i + 1)
                .and_then(|n| n.parse::<u8>().ok())
                .map(|n| (n, i + 1))
        } else {
            token
                .strip_prefix('h')
                .and_then(|n| n.parse::<u8>().ok())
                .map(|n| (n, i))
        };
        if let Some((n, at)) = level.filter(|(n, _)| (1..=6).contains(n)) {
            heading_level = Some(n);
            used[i] = true;
            used[at] = true;
        }
    }

    let mut i = 0;
    while i < tokens.len() {
        if used[i] {
            i += 1;
            continue;
        }
        let Some(first) = number(&tokens[i], lang) else {
            i += 1;
            continue;
        };
        // Intervalle : « entre A et B », « de A à B », « A-B ».
        let mut second = None;
        let mut end = i;
        if let (Some(link), Some(next)) = (tokens.get(i + 1), tokens.get(i + 2))
            && RANGE_LINKS.contains(&link.as_str())
            && let Some(b) = number(next, lang)
        {
            second = Some(b);
            end = i + 2;
        }
        // Unité : dans les 3 mots qui suivent, sans autre nombre avant.
        let mut found_unit = None;
        for (j, token) in tokens.iter().enumerate().take(end + 4).skip(end + 1) {
            if number(token, lang).is_some() {
                break;
            }
            if let Some(u) = unit(token, lang) {
                found_unit = Some((u, j));
                break;
            }
        }
        let Some((mut unit, unit_at)) = found_unit else {
            i = end + 1;
            continue;
        };
        if unit == Unit::Items && !list {
            return None;
        }
        if unit == Unit::Headings(None) {
            unit = Unit::Headings(heading_level);
        }
        used[i..=end].fill(true);
        used[unit_at] = true;
        let measure = if let Some(b) = second {
            Measure {
                unit,
                min: Some(first.min(b)),
                max: Some(first.max(b)),
            }
        } else {
            let before = bound_near(&tokens, i, false, BEFORE);
            let after = bound_near(&tokens, unit_at + 1, true, AFTER);
            if let Some((_, n)) = before {
                used[i - n..i].fill(true);
            }
            if let Some((_, n)) = after {
                used[unit_at + 1..unit_at + 1 + n].fill(true);
            }
            let about = |n: u32| {
                let n = u64::from(n);
                let cut = |v: u64| u32::try_from(v).unwrap_or(u32::MAX);
                (Some(cut(n * 9 / 10)), Some(cut((n * 11).div_ceil(10))))
            };
            let counted = unit != Unit::Words;
            let (min, max) = match before.or(after).map(|(b, _)| b) {
                Some(Bound::Min) => (Some(first), None),
                Some(Bound::MinStrict) => (Some(first + u32::from(counted)), None),
                Some(Bound::Max) => (None, Some(first)),
                Some(Bound::MaxStrict) => (None, Some(first.saturating_sub(u32::from(counted)))),
                Some(Bound::About) => about(first),
                // Un nombre de mots nu est une longueur visée (±10 %) ; un
                // nombre de titres, de sections… est exact.
                None if counted => (Some(first), Some(first)),
                None => about(first),
            };
            Measure { unit, min, max }
        };
        found.push(measure);
        i = unit_at + 1;
    }

    // Tableau sans nombre : présent, ou absent après une négation.
    for (i, token) in tokens.iter().enumerate() {
        if used[i] || unit(token, lang) != Some(Unit::Tables) {
            continue;
        }
        let next = tokens
            .get(i + 1..(i + 3).min(tokens.len()))
            .map(|w| w.join(" "));
        if next.is_some_and(|n| NOT_TABLES.iter().any(|p| n.starts_with(p))) {
            return None;
        }
        used[i] = true;
        let negated = (i.saturating_sub(3)..i).find(|k| NEGATIONS.contains(&tokens[*k].as_str()));
        if let Some(k) = negated {
            used[k] = true;
        }
        // Article ou « un » devant : sans valeur de mesure.
        found.push(Measure {
            unit: Unit::Tables,
            min: if negated.is_some() { None } else { Some(1) },
            max: negated.map(|_| 0),
        });
    }

    let [measure] = found.as_slice() else {
        return None;
    };
    // Rien d'autre que la mesure : mots sans contenu, articles d'une lettre.
    let simple = tokens.iter().enumerate().all(|(i, t)| {
        used[i]
            || (t.chars().count() == 1 && !t.chars().all(|c| c.is_ascii_digit()))
            || GENERIC.contains(&t.as_str())
            || NEGATIONS.contains(&t.as_str())
            || LIST_WORDS.contains(&t.as_str())
            || matches!(
                t.as_str(),
                "un" | "une"
                    | "one"
                    | "uno"
                    | "una"
                    | "y"
                    | "entre"
                    | "between"
                    | "from"
                    | "desde"
                    | "ou"
            )
            || BEFORE
                .iter()
                .chain(AFTER)
                .any(|(p, _)| p.split(' ').any(|w| w == t))
    });
    simple.then_some(*measure)
}

/// Mesure d'un critère de la spécification.
pub fn of_criterion(criterion: &Criterion, language: &str) -> Option<Measure> {
    parse(&criterion.text, language)
}

/// Critères de `spec` vérifiés par programme : (identifiant, mesure).
pub fn measured(spec: &Spec) -> Vec<(String, Measure)> {
    spec.criteria
        .iter()
        .filter_map(|c| Some((c.id.clone(), of_criterion(c, &spec.language)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(unit: Unit, min: Option<u32>, max: Option<u32>) -> Option<Measure> {
        Some(Measure { unit, min, max })
    }

    #[test]
    fn word_counts_are_recognised_in_three_languages() {
        let words = |min, max| m(Unit::Words, Some(min), Some(max));
        // Critère du banc du 25 nov. (en capitales, puis casse normale).
        assert_eq!(
            parse(
                "LORSQU'ON COMPTE LES MOTS, LE DOCUMENT DOIT COMPRENDRE ENTRE 3 300 ET 3 700 MOTS.",
                "fr"
            ),
            words(3_300, 3_700)
        );
        assert_eq!(
            parse("Le guide compte de 3\u{202f}300 à 3\u{a0}700 mots.", "fr"),
            words(3_300, 3_700)
        );
        assert_eq!(
            parse("Le document fait environ 3 500 mots.", "fr"),
            words(3_150, 3_850)
        );
        assert_eq!(
            parse("Le document fait 2 000 mots.", "fr"),
            words(1_800, 2_200)
        );
        assert_eq!(
            parse("Le texte compte au moins 1 500 mots.", "fr"),
            m(Unit::Words, Some(1_500), None)
        );
        assert_eq!(
            parse("Le livrable ne doit pas dépasser 800 mots.", "fr"),
            m(Unit::Words, None, Some(800))
        );
        assert_eq!(
            parse("The guide is between 3,300 and 3,700 words long.", "en"),
            words(3_300, 3_700)
        );
        assert_eq!(
            parse("The document has about 1,200 words.", "en"),
            words(1_080, 1_320)
        );
        assert_eq!(
            parse("The report is 900-1,100 words.", "en"),
            words(900, 1_100)
        );
        assert_eq!(
            parse("The text has no more than 500 words.", "en"),
            m(Unit::Words, None, Some(500))
        );
        assert_eq!(
            parse("El documento tiene entre 3.300 y 3.700 palabras.", "es"),
            words(3_300, 3_700)
        );
        assert_eq!(
            parse("El texto tiene al menos 2000 palabras.", "es"),
            m(Unit::Words, Some(2_000), None)
        );
        assert_eq!(
            parse("El informe tiene unas 900 palabras.", "es"),
            words(810, 990)
        );
    }

    #[test]
    fn headings_sections_tables_and_lists_are_recognised() {
        // Critère du banc du 25 nov.
        assert_eq!(
            parse(
                "Lorsqu'on examine la structure, le document doit contenir au moins huit titres \
                 Markdown commençant par des caractères #.",
                "fr"
            ),
            m(Unit::Headings(None), Some(8), None)
        );
        assert_eq!(
            parse("Le guide contient au moins quatre titres Markdown.", "fr"),
            m(Unit::Headings(None), Some(4), None)
        );
        assert_eq!(
            parse("Le document compte au moins 6 titres de niveau 2.", "fr"),
            m(Unit::Headings(Some(2)), Some(6), None)
        );
        assert_eq!(
            parse("The guide has 5 to 7 sections.", "en"),
            m(Unit::Sections, Some(5), Some(7))
        );
        assert_eq!(
            parse("The report has at most ten sections.", "en"),
            m(Unit::Sections, None, Some(10))
        );
        assert_eq!(
            parse("El documento tiene ocho secciones o más.", "es"),
            m(Unit::Sections, Some(8), None)
        );
        assert_eq!(
            parse("Le document comporte 8 parties.", "fr"),
            m(Unit::Sections, Some(8), Some(8))
        );
        assert_eq!(
            parse("Le document contient un tableau.", "fr"),
            m(Unit::Tables, Some(1), None)
        );
        assert_eq!(
            parse("The guide includes a table.", "en"),
            m(Unit::Tables, Some(1), None)
        );
        assert_eq!(
            parse("El informe incluye al menos dos tablas.", "es"),
            m(Unit::Tables, Some(2), None)
        );
        assert_eq!(
            parse("Le texte ne contient aucun tableau.", "fr"),
            m(Unit::Tables, None, Some(0))
        );
        assert_eq!(
            parse("Le guide contient une liste d'au moins 10 éléments.", "fr"),
            m(Unit::Items, Some(10), None)
        );
        assert_eq!(
            parse("The document includes a checklist of 5 to 8 items.", "en"),
            m(Unit::Items, Some(5), Some(8))
        );
    }

    #[test]
    fn a_criterion_that_asks_for_more_than_a_measure_stays_with_the_judges() {
        for (text, language) in [
            // Contenu en plus de la mesure.
            (
                "Le guide comporte au moins 8 sections, une par étape : emplacement, terre, semis.",
                "fr",
            ),
            (
                "La partie sur les cultures liste au moins huit légumes.",
                "fr",
            ),
            ("Le calendrier est présenté sous forme de tableau.", "fr"),
            (
                "Le document fait environ 3 500 mots pour rester lisible.",
                "fr",
            ),
            // Mesure par partie.
            ("Chaque partie compte au moins 300 mots.", "fr"),
            ("Each section has about 400 words.", "en"),
            // Pas une mesure.
            ("Le guide contient une table des matières.", "fr"),
            (
                "Le guide donne un critère d'ensoleillement en heures par jour.",
                "fr",
            ),
            ("Le carré mesure 2 m par 2 m.", "fr"),
            ("Le guide donne au moins 5 conseils.", "fr"),
            ("The guide gives 5 tips as a list.", "en"),
            // Deux mesures dans un critère.
            ("Le guide compte 3 000 mots et au moins 8 titres.", "fr"),
            // Une table (meuble) en français n'est pas un tableau.
            ("Le guide décrit une table.", "fr"),
        ] {
            assert_eq!(parse(text, language), None, "{text}");
        }
    }

    #[test]
    fn measures_are_taken_on_the_text() {
        let doc = "# Guide\n\nIntro un deux.\n\n## Un\n\nTexte.\n\n### Détail\n\n- a\n- b\n\n  - b1\n- c\n\nFin.\n\n## Deux\n\n| a | b |\n|---|:-:|\n| 1 | 2 |\n\n```\n# pas un titre\n| x |\n|---|\n```\n\n1. un\n2. deux\n";
        assert_eq!(
            apply(
                doc,
                &Measure {
                    unit: Unit::Headings(None),
                    min: Some(4),
                    max: None
                }
            ),
            (4, true)
        );
        assert_eq!(
            apply(
                doc,
                &Measure {
                    unit: Unit::Headings(Some(2)),
                    min: Some(3),
                    max: None
                }
            ),
            (2, false)
        );
        assert_eq!(
            apply(
                doc,
                &Measure {
                    unit: Unit::Sections,
                    min: Some(2),
                    max: Some(2)
                }
            ),
            (2, true)
        );
        assert_eq!(
            apply(
                doc,
                &Measure {
                    unit: Unit::Tables,
                    min: Some(1),
                    max: None
                }
            ),
            (1, true)
        );
        assert_eq!(
            apply(
                doc,
                &Measure {
                    unit: Unit::Tables,
                    min: None,
                    max: Some(0)
                }
            ),
            (1, false)
        );
        assert_eq!(
            apply(
                doc,
                &Measure {
                    unit: Unit::Items,
                    min: Some(3),
                    max: None
                }
            ),
            (3, true)
        );
        assert_eq!(
            apply(
                doc,
                &Measure {
                    unit: Unit::Items,
                    min: Some(2),
                    max: Some(2)
                }
            ),
            (2, true)
        );
        assert_eq!(
            apply(
                doc,
                &Measure {
                    unit: Unit::Items,
                    min: Some(5),
                    max: None
                }
            ),
            (3, false)
        );
        // Mots du corps : le titre du document ne compte pas.
        let words = Measure {
            unit: Unit::Words,
            min: Some(3),
            max: Some(3),
        };
        assert_eq!(
            apply("# Long titre ici\n\nun deux trois", &words),
            (3, true)
        );
    }

    #[test]
    fn the_evidence_is_read_back_with_its_gap() {
        let measure = Measure {
            unit: Unit::Words,
            min: Some(3_300),
            max: Some(3_700),
        };
        let evidence = measure.evidence(3_239);
        assert_eq!(evidence, "3239/3300-3700 words");
        assert_eq!(read_evidence(&evidence), Some((3_239, measure)));
        assert!((measure.gap(3_239) - (1.0 - 3_239.0 / 3_500.0)).abs() < 1e-9);
        assert_eq!(measure.gap(3_400), 0.0);
        let headings = Measure {
            unit: Unit::Headings(Some(2)),
            min: Some(8),
            max: None,
        };
        assert_eq!(read_evidence(&headings.evidence(5)), Some((5, headings)));
        let none = Measure {
            unit: Unit::Tables,
            min: None,
            max: Some(0),
        };
        assert_eq!(read_evidence(&none.evidence(1)), Some((1, none)));
        assert_eq!(none.gap(1), 1.0);
        assert_eq!(read_evidence("950/1000"), None);
        assert_eq!(read_evidence("brique"), None);
    }
}
