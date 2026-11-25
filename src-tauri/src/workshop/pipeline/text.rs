//! Mesures par programme sur les textes : mots, recouvrement, mots internes
//! au moteur, éléments hors registre, refus, mise en forme des sections.
//! Logique pure, commune à Linux et Windows.

use std::collections::BTreeSet;

use crate::proofread::guard::fold;

/// Mots d'un texte : suites de lettres ou de chiffres (apostrophes et traits
/// d'union internes compris).
pub fn words(text: &str) -> u32 {
    let mut count = 0u32;
    let mut inside = false;
    let mut previous_joiner = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if !inside {
                count = count.saturating_add(1);
                inside = true;
            }
            previous_joiner = false;
        } else if inside && !previous_joiner && matches!(c, '\'' | '’' | '-') {
            previous_joiner = true;
        } else {
            inside = false;
            previous_joiner = false;
        }
    }
    count
}

/// Texte en minuscules, sans accents.
pub fn folded(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter_map(fold)
        .collect()
}

/// Mots en minuscules sans accents.
pub fn tokens(text: &str) -> Vec<String> {
    folded(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Mots vides (français, anglais, espagnol) : sans poids dans un
/// recouvrement.
const STOP: &[&str] = &[
    // français
    "le", "la", "les", "un", "une", "des", "du", "de", "au", "aux", "et", "ou", "en", "dans", "sur",
    "pour", "par", "avec", "sans", "ce", "cette", "ces", "son", "sa", "ses", "leur", "leurs",
    "qui", "que", "quoi", "dont", "est", "sont", "plus", "moins", "comme", "tout", "tous", "toute",
    "toutes", "chaque", "pas", "nous", "vous", "elle", "ils", "elles", "aussi",
    // anglais
    "the", "and", "for", "with", "without", "this", "that", "these", "those", "its", "their",
    "from", "into", "are", "was", "were", "has", "have", "how", "what", "which", "each", "all",
    "about", "your", "you", "not", "but", "more", "less", // espagnol
    "los", "las", "una", "unos", "unas", "del", "con", "sin", "por", "para", "que", "sus", "este",
    "esta", "estos", "estas", "como", "todo", "todos", "cada", "mas", "menos",
];

/// Mots porteurs de sens, réduits à leurs 6 premières lettres (pluriels et
/// accords confondus).
pub fn significant(text: &str) -> BTreeSet<String> {
    tokens(text)
        .into_iter()
        .filter(|w| w.chars().count() > 2 && !STOP.contains(&w.as_str()))
        .map(|w| w.chars().take(6).collect())
        .collect()
}

/// Recouvrement de Jaccard des mots porteurs de sens (0 à 1).
pub fn jaccard(a: &str, b: &str) -> f64 {
    let a = significant(a);
    let b = significant(b);
    let union = a.union(&b).count();
    if union == 0 {
        return 0.0;
    }
    let inter = a.intersection(&b).count();
    #[expect(clippy::cast_precision_loss, reason = "petits nombres de mots")]
    let ratio = inter as f64 / union as f64;
    ratio
}

/// Mots du moteur (« atelier », « brique »…) : jamais dans une consigne au
/// modèle (essai `prompts_never_name_the_engine`) ; dans un livrable, un mot
/// seul n'est qu'un AVERTISSEMENT (« briques réfractaires », « maestros »,
/// « community workshop » sont des mots du métier). « atelier » vaut pour
/// toutes les langues (nom de l'outil). i18n-ignore : mots cherchés.
fn internal_list(language: &str) -> &'static [&'static str] {
    match language {
        "fr" => &[
            "atelier", "ateliers", "brique", "briques", "maitre", "fiche", "fiches",
        ],
        "es" => &[
            "atelier",
            "taller",
            "ladrillo",
            "ladrillos",
            "maestro",
            "ficha",
            "fichas",
        ],
        _ => &["atelier", "workshop", "brick", "bricks"],
    }
}

/// Tournures propres au moteur, qui ne parlent qu'à lui (lot 0 : « Astuce
/// atelier », « Conseil d'atelier » ont fui dans le guide) : elles BLOQUENT.
/// Texte replié (minuscules, sans accents, apostrophe droite, espaces
/// réduites) ; `§` : un nombre. Liste courte et précise : « la brique »,
/// « cette brique », « maître d'œuvre » sont aussi des mots du bâtiment.
/// i18n-ignore : tournures cherchées.
fn phrase_list(language: &str) -> &'static [&'static str] {
    match language {
        "fr" => &[
            "astuce atelier",
            "astuce d'atelier",
            "astuce de l'atelier",
            "conseil atelier",
            "conseil d'atelier",
            "conseil de l'atelier",
            "brique n°§",
            "brique no.§",
            "brique #§",
            "fiche de la brique",
            "fiche de brique",
        ],
        "es" => &[
            "consejo del taller",
            "consejo de taller",
            "truco del taller",
            "ladrillo n.º§",
            "ladrillo n.°§",
            "ladrillo #§",
            "ficha del ladrillo",
        ],
        _ => &[
            "workshop tip",
            "workshop tips",
            "brick #§",
            "brick no.§",
            "brick card",
        ],
    }
}

/// Texte replié pour les tournures : minuscules sans accents, apostrophes
/// droites, marque de numéro collée au nombre qui suit (« brick # 3 » →
/// « brick #3 »), espaces réduites.
fn phrase_text(text: &str) -> String {
    let folded = folded(text).replace(['’', '‘'], "'");
    let joined = folded.split_whitespace().collect::<Vec<_>>().join(" ");
    ["# ", "n° ", "no. ", "n.º ", "n.° "]
        .iter()
        .fold(joined, |text, mark| text.replace(mark, mark.trim_end()))
}

/// `pattern` (avec `§` pour un nombre) se trouve dans `text` à une frontière
/// de mot.
fn has_phrase(text: &str, pattern: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let wanted: Vec<char> = pattern.chars().collect();
    'start: for start in 0..chars.len() {
        if start > 0 && chars[start - 1].is_alphanumeric() {
            continue;
        }
        let mut at = start;
        for w in &wanted {
            if *w == '§' {
                let digits = chars[at..]
                    .iter()
                    .take_while(|c| c.is_ascii_digit())
                    .count();
                if digits == 0 {
                    continue 'start;
                }
                at += digits;
                continue;
            }
            if chars.get(at) != Some(w) {
                continue 'start;
            }
            at += 1;
        }
        if chars.get(at).is_none_or(|c| !c.is_alphanumeric()) {
            return true;
        }
    }
    false
}

/// Tournures du moteur ([`phrase_list`], langue de sortie puis anglais)
/// présentes dans `text`, hors celles que l'utilisateur emploie lui-même
/// (`allowed` : sa tâche, ses documents). Chaque tournure une fois.
pub fn internal_phrases(text: &str, language: &str, allowed: &str) -> Vec<String> {
    let text = phrase_text(text);
    let allowed = phrase_text(allowed);
    let mut lists: Vec<&str> = phrase_list(language).to_vec();
    if language != "en" {
        lists.extend_from_slice(phrase_list("en"));
    }
    let mut out: Vec<String> = Vec::new();
    for pattern in lists {
        let shown = pattern.replace('§', "");
        let shown = shown.trim().to_owned();
        if has_phrase(&text, pattern) && !has_phrase(&allowed, pattern) && !out.contains(&shown) {
            out.push(shown);
        }
    }
    out
}

/// Mots du moteur présents dans `text` (langue `language`), hors
/// ceux que l'utilisateur emploie lui-même (`allowed` : sa tâche, ses
/// documents). Chaque mot une fois.
pub fn internal_words(text: &str, language: &str, allowed: &str) -> Vec<String> {
    let allowed: BTreeSet<String> = tokens(allowed).into_iter().collect();
    let found: BTreeSet<String> = tokens(text).into_iter().collect();
    let mut lists: Vec<&str> = internal_list(language).to_vec();
    if language != "en" {
        lists.extend_from_slice(internal_list("en"));
    }
    let mut out = Vec::new();
    for word in lists {
        if found.contains(word) && !allowed.contains(word) && !out.iter().any(|w| w == word) {
            out.push(word.to_owned());
        }
    }
    out
}

/// Marques d'indécision : un champ qui en contient n'est pas tranché.
/// i18n-ignore : mots cherchés.
const HEDGES: &[&str] = &[
    "if",
    "possibly",
    "maybe",
    "depending",
    "optionally",
    "si",
    "selon",
    "eventuellement",
    "possible",
    "possiblement",
    "peut",
    "segun",
    "posible",
    "posiblemente",
    "quizas",
];

/// Disjonctions : indécises seulement entre deux systèmes d'unités
/// (« métrique ou impérial ») ; « g, kg ou t » énumère des unités.
const DISJUNCTIONS: &[&str] = &["or", "either", "ou", "o"];

/// Débuts des noms de systèmes d'unités (repliés). i18n-ignore : mots
/// cherchés.
const SYSTEMS: &[&str] = &["metri", "imperi", "anglo", "customa", "britann", "americ"];

/// Premier mot d'indécision de `units` : une marque (« si », « possible »,
/// « selon »…), ou une disjonction (« ou », « or », « o ») entre deux
/// systèmes d'unités. Ni « / » entre unités (« L/m² »), ni le sigle « SI »
/// (Système international) ne comptent.
pub fn undecided(units: &str) -> Option<String> {
    // Mots tels qu'écrits : « SI » en capitales est le sigle.
    let raw: Vec<&str> = units
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let systems: BTreeSet<&str> = raw
        .iter()
        .filter_map(|w| {
            let folded = folded(w);
            SYSTEMS.iter().find(|s| folded.starts_with(**s)).copied()
        })
        .chain(raw.iter().filter(|w| **w == "US").map(|_| "us"))
        .collect();
    for word in raw {
        if word == "SI" {
            continue;
        }
        let folded = folded(word);
        if HEDGES.contains(&folded.as_str())
            || (systems.len() >= 2 && DISJUNCTIONS.contains(&folded.as_str()))
        {
            return Some(folded);
        }
    }
    None
}

/// Un groupe d'éléments énumérés ensemble (liste à puces, première colonne
/// d'un tableau, ou énumération dans une phrase).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub inline: bool,
    pub items: Vec<String>,
}

/// Tête d'un élément de liste : avant « : », « — », « ( », « , », sans
/// gras, au plus 5 mots.
fn head(item: &str) -> String {
    let cut = item
        .find([':', '—', '–', '(', ',', ';'])
        .map_or(item, |at| &item[..at]);
    let cut = cut.split(" - ").next().unwrap_or(cut);
    let clean: String = cut.replace(['*', '_', '`'], "");
    clean
        .split_whitespace()
        .take(5)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Élément de liste à puces ou numérotée.
fn bullet(line: &str) -> Option<&str> {
    let line = line.trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(rest);
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &line[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some(rest);
        }
    }
    None
}

/// Groupes énumérés de `text`.
pub fn enumerations(text: &str) -> Vec<Group> {
    let mut groups = Vec::new();
    let mut bullets: Vec<String> = Vec::new();
    let mut cells: Vec<String> = Vec::new();
    let flush = |items: &mut Vec<String>, groups: &mut Vec<Group>| {
        let items: Vec<String> = std::mem::take(items)
            .into_iter()
            .filter(|i| !i.is_empty())
            .collect();
        if items.len() >= 2 {
            groups.push(Group {
                inline: false,
                items,
            });
        }
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(item) = bullet(line) {
            bullets.push(head(item));
            continue;
        }
        flush(&mut bullets, &mut groups);
        if trimmed.starts_with('|') {
            let first = trimmed.trim_matches('|').split('|').next().unwrap_or("");
            let first = first.trim();
            // Ligne de séparation de l'en-tête : « --- ».
            if !first.is_empty() && !first.chars().all(|c| matches!(c, '-' | ':' | ' ')) {
                cells.push(head(first));
            }
            continue;
        }
        if cells.len() > 1 {
            // Première ligne = en-tête du tableau.
            cells.remove(0);
        }
        flush(&mut cells, &mut groups);
        // Énumérations dans les phrases.
        for sentence in trimmed.split(['.', '!', '?', ':']) {
            let parts: Vec<String> = sentence
                .split([',', ';'])
                .flat_map(|part| {
                    let mut pieces = vec![part.to_owned()];
                    for joiner in [" et ", " and ", " y ", " ou ", " or "] {
                        pieces = pieces
                            .iter()
                            .flat_map(|p| p.split(joiner).map(str::to_owned).collect::<Vec<_>>())
                            .collect();
                    }
                    pieces
                })
                .map(|p| p.replace(['*', '_', '`'], "").trim().to_owned())
                .filter(|p| !p.is_empty())
                .collect();
            let short = parts.iter().filter(|p| words(p) <= 4).count();
            if parts.len() >= 3 && short >= 3 {
                groups.push(Group {
                    inline: true,
                    items: parts.into_iter().filter(|p| words(p) <= 4).collect(),
                });
            }
        }
    }
    flush(&mut bullets, &mut groups);
    if cells.len() > 1 {
        cells.remove(0);
    }
    flush(&mut cells, &mut groups);
    groups
}

/// `item` désigne `reference` : tous les mots porteurs de sens de la
/// référence s'y retrouvent (6 premières lettres).
pub fn item_matches(item: &str, reference: &str) -> bool {
    let wanted = significant(reference);
    if wanted.is_empty() {
        return false;
    }
    let have = significant(item);
    wanted.iter().all(|w| have.contains(w))
}

/// Forme d'un élément : premier mot s'il est vide de sens (déterminant),
/// nombre de mots.
fn shape(item: &str) -> (String, usize) {
    let words: Vec<&str> = item.split_whitespace().collect();
    let first = words.first().map(|w| folded(w)).unwrap_or_default();
    let lead = if STOP.contains(&first.as_str()) || first.chars().count() <= 2 {
        first
    } else {
        String::new()
    };
    (lead, words.len())
}

/// Fin de `item` de la forme `wanted`, s'il y en a une.
fn same_shape(item: &str, wanted: &(String, usize)) -> Option<String> {
    let words: Vec<&str> = item.split_whitespace().collect();
    if words.len() < wanted.1 {
        return None;
    }
    let tail = words[words.len() - wanted.1..].join(" ");
    (shape(&tail).0 == wanted.0).then_some(tail)
}

/// Élément cité hors d'une liste retenue : (nom de la liste, élément).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stray {
    pub list: String,
    pub item: String,
}

/// Éléments cités avec ceux d'une liste retenue (même liste à puces, même
/// colonne, même énumération) sans y être : la brique « Récoltes » du lot 0
/// citait poireaux et fraises, absents des cultures retenues. Une
/// énumération dans une phrase compte à partir de deux éléments reconnus ;
/// une liste ou un tableau, dès un.
pub fn out_of_registry(text: &str, lists: &[(String, Vec<String>)]) -> Vec<Stray> {
    let groups = enumerations(text);
    let mut out: Vec<Stray> = Vec::new();
    for (name, items) in lists {
        if items.len() < 2 {
            continue;
        }
        for group in &groups {
            let known = |candidate: &str| items.iter().any(|r| item_matches(candidate, r));
            let matched = group.items.iter().filter(|i| known(i)).count();
            let needed = if group.inline { 2 } else { 1 };
            if matched < needed {
                continue;
            }
            let shapes: Vec<(String, usize)> = group
                .items
                .iter()
                .filter(|i| known(i))
                .map(|i| shape(i))
                .collect();
            for item in &group.items {
                if known(item) || significant(item).is_empty() {
                    continue;
                }
                let candidate = if group.inline {
                    // Dans une phrase, seul un élément de même forme que les
                    // éléments reconnus (même déterminant, même longueur)
                    // compte : « on récolte les fraises » → « les fraises »,
                    // « En hiver » écarté.
                    match shapes.iter().find_map(|s| same_shape(item, s)) {
                        Some(candidate) => candidate,
                        None => continue,
                    }
                } else {
                    item.clone()
                };
                if !out.iter().any(|s| s.list == *name && s.item == candidate) {
                    out.push(Stray {
                        list: name.clone(),
                        item: candidate,
                    });
                }
            }
        }
    }
    out
}

/// Débuts de refus (texte replié). i18n-ignore : motifs cherchés.
const REFUSALS: &[&str] = &[
    "i can't",
    "i cannot",
    "i can not",
    "i'm sorry, but",
    "i am sorry, but",
    "i'm unable",
    "i am unable",
    "i won't",
    "je ne peux pas",
    "je suis desole",
    "desole, mais",
    "je refuse",
    "no puedo",
    "lo siento",
];

/// Le texte ressemble à un refus : court (moins de 40 % de `target` mots, et
/// moins de 200 mots) et commence par une formule de refus. À confirmer par
/// le juge de refus.
pub fn looks_like_refusal(text: &str, target: u32) -> bool {
    let n = words(text);
    if n >= 200 || f64::from(n) >= 0.4 * f64::from(target.max(1)) {
        return false;
    }
    let start: String = folded(text)
        .replace('’', "'")
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .chars()
        .take(120)
        .collect();
    REFUSALS.iter().any(|r| start.contains(r))
}

/// Retire une enveloppe de bloc de code (« ```markdown … ``` »).
fn unfenced(text: &str) -> &str {
    let trimmed = text.trim();
    if let Some(body) = trimmed
        .strip_prefix("```")
        .and_then(|rest| rest.strip_suffix("```"))
    {
        return body.split_once('\n').map_or("", |(_, b)| b).trim();
    }
    trimmed
}

/// Niveau d'un titre Markdown (`# …` : 1).
fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    (hashes > 0 && line[hashes..].starts_with(' ')).then_some(hashes)
}

/// Une ligne de titre Markdown.
pub fn is_heading(line: &str) -> bool {
    heading_level(line).is_some()
}

/// Titre sans marques de titre en tête : « ### Choisis » → « Choisis »,
/// « ## #### Compte » → « Compte » (banc du 25 nov. : le redécoupage
/// rendait des titres « ### … », posés sous « ## » : « ## ### … »).
pub fn clean_title(title: &str) -> String {
    let mut rest = title.trim();
    loop {
        let hashes = rest.chars().take_while(|c| *c == '#').count();
        if hashes == 0 || !rest[hashes..].starts_with(char::is_whitespace) {
            return rest.to_owned();
        }
        rest = rest[hashes..].trim_start();
    }
}

/// Ligne de titre au niveau `level` (borné de 1 à 6), texte nettoyé.
fn heading_line(level: usize, text: &str) -> String {
    format!("{} {}", "#".repeat(level.clamp(1, 6)), clean_title(text))
}

/// Titres empilés repliés (« ## ### X » → « ## X ») ; niveaux gardés.
pub fn tidy_headings(text: &str) -> String {
    text.lines()
        .map(|line| match heading_level(line) {
            Some(level) => heading_line(level, &line[level..]),
            None => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Section de `title` : titres de tête (niveaux 1 et 2 : titre du document,
/// titre de section) remplacés par « ## title », titres de niveau 1 ou 2
/// plus loin abaissés au niveau 3 (sous-titres de la section), titres
/// empilés repliés.
pub fn as_section(text: &str, title: &str) -> String {
    let body = unfenced(text);
    let mut lines: Vec<&str> = body.lines().collect();
    while lines
        .first()
        .is_some_and(|l| l.trim().is_empty() || heading_level(l).is_some_and(|n| n <= 2))
    {
        lines.remove(0);
    }
    let rest: Vec<String> = lines
        .iter()
        .map(|line| match heading_level(line) {
            Some(level) => heading_line(level.max(3), &line[level..]),
            None => (*line).to_owned(),
        })
        .collect();
    format!("{}\n\n{}", heading_line(2, title), rest.join("\n"))
        .trim_end()
        .to_owned()
}

/// Section abaissée d'un niveau (sous-partie dans sa partie) : chaque
/// titre un niveau plus bas, relatif à celui du parent (borné au niveau 6),
/// titres empilés repliés.
pub fn demoted(section: &str) -> String {
    section
        .lines()
        .map(|line| match heading_level(line) {
            Some(level) => heading_line(level + 1, &line[level..]),
            None => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Corps d'une introduction : titres de tête retirés (le titre du document
/// est posé à l'assemblage), titres de niveau 1 abaissés.
pub fn as_intro(text: &str) -> String {
    let body = unfenced(text);
    let mut lines: Vec<&str> = body.lines().collect();
    while lines
        .first()
        .is_some_and(|l| l.trim().is_empty() || heading_level(l).is_some())
    {
        lines.remove(0);
    }
    lines
        .iter()
        .map(|line| {
            if heading_level(line) == Some(1) {
                format!("##{}", &line[1..])
            } else {
                (*line).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_owned()
}

/// Corps d'une section sans sa ligne de titre.
pub fn body_of(section: &str) -> &str {
    match section.split_once('\n') {
        Some((first, rest)) if heading_level(first).is_some() => rest.trim(),
        _ if heading_level(section.trim()).is_some() => "",
        _ => section.trim(),
    }
}

/// Document direct : enveloppe retirée.
pub fn as_document(text: &str) -> String {
    unfenced(text).to_owned()
}

/// Premier titre de niveau 1.
pub fn first_title(text: &str) -> Option<String> {
    text.lines()
        .find(|l| heading_level(l) == Some(1))
        .map(|l| l[1..].trim().to_owned())
        .filter(|t| !t.is_empty())
}

/// Partie ASCII d'un nom de fichier (lettres et chiffres, tirets, 60
/// caractères au plus).
fn ascii_slug(title: &str) -> String {
    let mut out = String::new();
    for c in folded(title).chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 60 {
            break;
        }
    }
    out.trim_matches('-').to_owned()
}

/// Nom de fichier court tiré d'un titre (ASCII, tirets, 60 caractères au
/// plus) ; vide : `fallback` (« document » dans la langue de l'app) ; nom
/// réservé par Windows (`os_util::windows_reserved`) : suivi de
/// « -<fallback> » (même règle sous Linux : un dossier de projet peut passer
/// d'un système à l'autre).
pub fn slug(title: &str, fallback: &str) -> String {
    let out = ascii_slug(title);
    let fallback = match ascii_slug(fallback) {
        name if name.is_empty() => "document".to_owned(),
        name => name,
    };
    if out.is_empty() {
        fallback
    } else if crate::os_util::windows_reserved(&out) {
        format!("{out}-{fallback}")
    } else {
        out
    }
}

/// Casse de phrase pour un texte presque tout en capitales (plus de 60 %
/// de ses lettres) : banc du 25 nov., critères « LORSQU'ON COMPTE LES
/// MOTS, LE DOCUMENT DOIT… ». Un texte en casse normale est rendu tel quel.
pub fn normal_case(text: &str) -> String {
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    let upper = text.chars().filter(|c| c.is_uppercase()).count();
    if letters == 0 || upper * 10 <= letters * 6 {
        return text.to_owned();
    }
    let lower: String = text.chars().flat_map(char::to_lowercase).collect();
    let mut out = String::with_capacity(lower.len());
    let mut first = true;
    for c in lower.chars() {
        if first && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            first = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Nombres écrits en lettres (2 à 20) ; « un », « one », « uno » exclus
/// (« une section sur l'arrosage » ne compte rien). i18n-ignore : mots
/// cherchés.
const NUMBER_WORDS: &[&str] = &[
    "deux", "trois", "quatre", "cinq", "six", "sept", "huit", "neuf", "dix", "onze", "douze",
    "quinze", "vingt", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    "eleven", "twelve", "fifteen", "twenty", "dos", "tres", "cuatro", "cinco", "seis", "siete",
    "ocho", "nueve", "diez", "once", "doce", "quince", "veinte",
];

/// Ce qu'un critère d'ensemble compte : mots, titres, sections, parties,
/// chapitres (repliés). i18n-ignore : mots cherchés.
const WHOLE_UNITS: &[&str] = &[
    "mots",
    "words",
    "palabras",
    "titres",
    "intertitres",
    "headings",
    "titles",
    "titulos",
    "subtitulos",
    "sections",
    "secciones",
    "parties",
    "parts",
    "partes",
    "chapitres",
    "chapters",
    "capitulos",
    "paragraphes",
    "paragraphs",
    "parrafos",
];

/// Tournures d'un critère d'ensemble (repliées). i18n-ignore : mots
/// cherchés.
const WHOLE_PHRASES: &[&str] = &[
    "longueur totale",
    "nombre de mots",
    "nombre total",
    "total length",
    "word count",
    "number of words",
    "longitud total",
    "numero de palabras",
    "whole document",
    "document entier",
    "documento entero",
    "documento completo",
];

/// Le critère porte sur le document entier : longueur totale, nombre de
/// mots, de titres, de sections (« entre 3 300 et 3 700 mots », « au moins
/// huit titres »). Un nombre suivi, à deux mots au plus, d'une de ces
/// unités ; ou une tournure d'ensemble.
pub fn document_wide(text: &str) -> bool {
    let folded = folded(text).replace(['’', '\''], " ");
    let joined = folded.split_whitespace().collect::<Vec<_>>().join(" ");
    if WHOLE_PHRASES.iter().any(|p| joined.contains(p)) {
        return true;
    }
    let tokens: Vec<&str> = folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    tokens.iter().enumerate().any(|(i, token)| {
        let count = token.chars().all(|c| c.is_ascii_digit()) || NUMBER_WORDS.contains(token);
        count
            && tokens
                .iter()
                .skip(i + 1)
                .take(3)
                .any(|next| WHOLE_UNITS.contains(next))
    })
}

/// Fins de phrase.
const SENTENCE_ENDS: &[&str] = &[". ", "! ", "? ", "… ", ".\t", "!\t", "?\t"];

/// Variantes d'une citation du juge à chercher dans le texte : telle
/// quelle, apostrophes typographiques ou droites, sans ponctuation ni
/// guillemets de bord.
fn quote_variants(quote: &str) -> Vec<String> {
    let trimmed = quote
        .trim()
        .trim_matches(|c: char| matches!(c, '«' | '»' | '"' | '“' | '”'))
        .trim();
    let bare = trimmed
        .trim_end_matches(['.', '!', '?', '…', ';', ','])
        .trim();
    let mut out = Vec::new();
    for base in [trimmed, bare] {
        for variant in [
            base.to_owned(),
            base.replace('\'', "’"),
            base.replace('’', "'"),
        ] {
            if !variant.is_empty() && !out.contains(&variant) {
                out.push(variant);
            }
        }
    }
    out
}

/// Phrase (ou ligne de tableau) qui contient `quote` dans `text` : plage
/// d'octets, sans la marque de liste ni l'espace qui suit la phrase. Un
/// titre, une citation introuvable ou sur plusieurs lignes : `None`.
pub fn sentence_span(text: &str, quote: &str) -> Option<(usize, usize)> {
    let (at, len) = quote_variants(quote)
        .iter()
        .filter(|q| !q.contains('\n'))
        .find_map(|q| text.find(q.as_str()).map(|at| (at, q.len())))?;
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at + len..]
        .find('\n')
        .map_or(text.len(), |i| at + len + i);
    if line_end < at + len {
        return None;
    }
    let line = &text[line_start..line_end];
    if is_heading(line) {
        return None;
    }
    if line.trim_start().starts_with('|') {
        return Some((line_start, line_end));
    }
    // Marque de liste (« - », « * », « 1. ») hors de la phrase.
    let indent = line.len() - line.trim_start().len();
    // Citation qui commence avant la phrase (marque de liste comprise) :
    // depuis la citation.
    let content_start =
        (line_start + indent + bullet(line).map_or(0, |rest| line.trim_start().len() - rest.len()))
            .min(at);
    let start = SENTENCE_ENDS
        .iter()
        .filter_map(|end| {
            text[content_start..at]
                .rfind(end)
                .map(|i| content_start + i + end.len())
        })
        .max()
        .unwrap_or(content_start)
        .max(content_start);
    // Fin cherchée depuis le dernier caractère de la citation : elle peut
    // finir par son point.
    let last = at
        + text[at..at + len]
            .char_indices()
            .last()
            .map_or(0, |(i, _)| i);
    let end = SENTENCE_ENDS
        .iter()
        .filter_map(|end| {
            text[last..line_end]
                .find(end)
                .map(|i| last + i + end.trim_end().len())
        })
        .min()
        .unwrap_or(line_end)
        .max(at + len);
    let start = start + (text[start..end].len() - text[start..end].trim_start().len());
    Some((start, end))
}

/// Suites d'au moins `n` mots (repliés) de `remark` reprises dans `text`
/// mais absentes de `before` : une correction qui recopie la remarque du
/// juge (banc du 25 nov. : « …ou 1 à 2 kg pour 10 m², en respectant les
/// recommandations locales »). Chaque suite, la plus longue, une fois.
pub fn copied_runs(remark: &str, text: &str, before: &str, n: usize) -> Vec<String> {
    let remark = tokens(remark);
    if n == 0 || remark.len() < n {
        return Vec::new();
    }
    let grams =
        |words: &[String]| -> BTreeSet<String> { words.windows(n).map(|w| w.join(" ")).collect() };
    let new = grams(&tokens(text));
    let old = grams(&tokens(before));
    let copied: Vec<bool> = remark
        .windows(n)
        .map(|w| {
            let gram = w.join(" ");
            new.contains(&gram) && !old.contains(&gram)
        })
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < copied.len() {
        if !copied[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < copied.len() && copied[i] {
            i += 1;
        }
        // Fenêtres `start..i` copiées : mots `start..i - 1 + n`.
        let run = remark[start..i - 1 + n].join(" ");
        if !out.contains(&run) {
            out.push(run);
        }
    }
    out
}

/// Coupe `text` à `max` caractères (sur une frontière de caractère).
pub fn clip(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_counted_with_joiners() {
        assert_eq!(words("L'arrosage au goutte-à-goutte, 3 fois."), 5);
        assert_eq!(words(""), 0);
        assert_eq!(words("## Titre\n\nUn deux."), 3);
    }

    #[test]
    fn internal_words_are_found_unless_the_user_uses_them() {
        let text = "Astuce atelier : arroser le soir. Une brique de terre.";
        assert_eq!(
            internal_words(text, "fr", "Écris un guide du potager."),
            vec!["atelier".to_owned(), "brique".to_owned()]
        );
        assert_eq!(
            internal_words(text, "fr", "Guide pour mon atelier de poterie"),
            vec!["brique".to_owned()]
        );
        assert!(internal_words("A plain text.", "en", "").is_empty());
        assert_eq!(
            internal_words("Workshop tip: water.", "fr", ""),
            vec!["workshop".to_owned()]
        );
    }

    #[test]
    fn hedges_make_a_field_undecided() {
        assert_eq!(undecided("tutoiement si public jeune"), Some("si".into()));
        assert_eq!(undecided("metric or imperial"), Some("or".into()));
        assert_eq!(undecided("métrique ou impérial"), Some("ou".into()));
        assert_eq!(undecided("sistema métrico o imperial"), Some("o".into()));
        assert_eq!(
            undecided("métrique, impérial possible"),
            Some("possible".into())
        );
        assert_eq!(undecided("selon le pays"), Some("selon".into()));
        assert_eq!(undecided("métrique : g, kg, m, L, °C"), None);
    }

    /// Relecture du lot 2 : « / » entre unités, le sigle SI et une
    /// énumération d'unités ne rendent pas les unités indécises.
    #[test]
    fn units_with_slashes_and_the_si_acronym_are_decided() {
        assert_eq!(undecided("métrique (SI) : g, kg, m², L/m²"), None);
        assert_eq!(undecided("kg/lb"), None);
        assert_eq!(undecided("metric: g, kg or t"), None);
        assert_eq!(undecided("SI units, m/s"), None);
    }

    /// Relecture du lot 2 : seules les tournures du moteur bloquent ; les
    /// mots du métier restent (avertissement seulement).
    #[test]
    fn only_engine_phrases_are_internal() {
        let none = |text: &str, language: &str| internal_phrases(text, language, "").is_empty();
        assert!(none("Les briques réfractaires du four à pain.", "fr"));
        assert!(none("Posez la brique suivante, puis cette brique.", "fr"));
        assert!(none("Los maestros del taller de cerámica.", "es"));
        assert!(none(
            "A community workshop for repairs; a brick 20 cm long.",
            "en"
        ));
        assert_eq!(
            internal_phrases("**Astuce atelier** : arroser le soir.", "fr", ""),
            vec!["astuce atelier".to_owned()]
        );
        assert_eq!(
            internal_phrases("Conseil d’atelier : pailler.", "fr", ""),
            vec!["conseil d'atelier".to_owned()]
        );
        assert_eq!(
            internal_phrases("Voir la brique n° 3.", "fr", ""),
            vec!["brique n°".to_owned()]
        );
        assert_eq!(
            internal_phrases("As said in brick #2, water.", "en", ""),
            vec!["brick #".to_owned()]
        );
        assert_eq!(
            internal_phrases("Consejo del taller: regar.", "es", ""),
            vec!["consejo del taller".to_owned()]
        );
        // Workshop tip : dans toutes les langues.
        assert_eq!(
            internal_phrases("Workshop tip: water.", "fr", ""),
            vec!["workshop tip".to_owned()]
        );
        // Employée par l'utilisateur : permise.
        assert!(
            internal_phrases(
                "Astuce atelier : x.",
                "fr",
                "Mets une « astuce atelier » par partie"
            )
            .is_empty()
        );
    }

    #[test]
    fn strays_of_a_retained_list_are_found() {
        let lists = vec![(
            "cultures".to_owned(),
            vec!["tomate".into(), "radis".into(), "laitue".into()],
        )];
        let text = "## Récoltes\n\n- Tomates : août\n- Poireaux : novembre\n- Radis : mai\n\n\
            En hiver, on récolte les fraises, les tomates et les radis.\n\n\
            | Culture | Mois |\n|---|---|\n| Laitue | juin |\n| Panais | décembre |\n";
        let strays: Vec<String> = out_of_registry(text, &lists)
            .into_iter()
            .map(|s| s.item)
            .collect();
        assert_eq!(strays, vec!["Poireaux", "les fraises", "Panais"]);
        // Une seule culture reconnue dans une phrase : pas une énumération
        // de cultures.
        let text = "On arrose les tomates, le matin, le soir et la nuit.";
        assert!(out_of_registry(text, &lists).is_empty());
    }

    #[test]
    fn refusals_are_spotted_on_short_texts_only() {
        assert!(looks_like_refusal(
            "Je suis désolé, mais je ne peux pas écrire ce texte.",
            800
        ));
        assert!(looks_like_refusal("I’m sorry, but I can’t help.", 500));
        let long = format!("I cannot stress enough. {}", "Word ".repeat(400));
        assert!(!looks_like_refusal(&long, 500));
        assert!(!looks_like_refusal("Le compost se fait en tas.", 800));
    }

    #[test]
    fn sections_get_their_planned_title() {
        assert_eq!(
            as_section("# Guide\n\n## Le sol\n\nTexte.\n# Autre", "Le sol vivant"),
            "## Le sol vivant\n\nTexte.\n### Autre"
        );
        assert_eq!(as_section("Texte seul.", "Eau"), "## Eau\n\nTexte seul.");
        assert_eq!(
            as_section("```markdown\n## X\n\nCorps\n```", "Y"),
            "## Y\n\nCorps"
        );
        assert_eq!(as_intro("# Titre\n## Introduction\n\nBonjour."), "Bonjour.");
        assert_eq!(body_of("## Eau\n\nCorps."), "Corps.");
    }

    #[test]
    fn slugs_are_short_and_ascii() {
        assert_eq!(
            slug("Guide du potager : l'été !", "document"),
            "guide-du-potager-l-ete"
        );
        assert_eq!(slug("¿?", "documento"), "documento");
        assert_eq!(slug("¿?", "¿?"), "document");
        // Nom réservé par Windows (« con.md » ne s'ouvrirait pas).
        assert_eq!(slug("Con", "document"), "con-document");
        assert_eq!(slug("Nul !", "documento"), "nul-documento");
        assert_eq!(slug("Contexte", "document"), "contexte");
    }

    /// Banc du 25 nov. : critères d'ensemble reconnus, critères de partie
    /// laissés.
    #[test]
    fn whole_document_criteria_are_recognised() {
        for whole in [
            "LORSQU'ON COMPTE LES MOTS, LE DOCUMENT DOIT COMPRENDRE ENTRE 3 300 ET 3 700 MOTS.",
            "Le document contient au moins huit titres Markdown.",
            "The guide has at least 6 sections.",
            "La longitud total es de unas 2 000 palabras.",
            "La longueur totale reste proche de la demande.",
        ] {
            assert!(document_wide(whole), "{whole}");
        }
        for part in [
            "LORSQU'ON LIT LA SECTION EMPLACEMENT, LE LIVRABLE DOIT DONNER UN CRITÈRE \
             D'ENSOLEILLEMENT EN HEURES PAR JOUR.",
            "La section Choix des cultures présente au moins huit légumes.",
            "La section Récoltes liste au moins six récoltes avec leur mois.",
            "Une section sur l'arrosage donne une fréquence en jours.",
        ] {
            assert!(!document_wide(part), "{part}");
        }
    }

    #[test]
    fn capitals_become_sentence_case() {
        assert_eq!(
            normal_case("LORSQU'ON LIT LA SECTION TERRE, LE LIVRABLE DOIT DONNER 3 L."),
            "Lorsqu'on lit la section terre, le livrable doit donner 3 l."
        );
        assert_eq!(
            normal_case("La section Sol donne 3 L."),
            "La section Sol donne 3 L."
        );
        assert_eq!(normal_case("ÉTÉ"), "Été");
    }

    /// Banc du 25 nov. : « ## ### Choisis… », « ## #### Compte… ».
    #[test]
    fn stacked_headings_are_folded() {
        assert_eq!(
            clean_title("### Choisis un emplacement"),
            "Choisis un emplacement"
        );
        assert_eq!(clean_title("## #### Compte"), "Compte");
        assert_eq!(clean_title("#1 du jardin"), "#1 du jardin");
        assert_eq!(
            as_section(
                "## #### Compte les heures\n\nTexte.\n## Soleil\n# Ombre\n#### ## Nuit",
                "### Compte les heures"
            ),
            "## Compte les heures\n\nTexte.\n### Soleil\n### Ombre\n#### Nuit"
        );
        assert_eq!(
            tidy_headings("## ### X\ntexte #3\n#### ## Y"),
            "## X\ntexte #3\n#### Y"
        );
        assert_eq!(demoted("###### Fin\n## A"), "###### Fin\n### A");
    }

    #[test]
    fn the_sentence_of_a_quote_is_found() {
        let text = "## Sol\n\nTu prépares. Mets 200 g de fumier par m² en mars. Puis tu \
                    arroses.\n- Pique à 3 cm. Tasse.\n\n| Tomate | 30 cm |";
        let at = |quote: &str| sentence_span(text, quote).map(|(s, e)| &text[s..e]);
        assert_eq!(
            at("200 g de fumier"),
            Some("Mets 200 g de fumier par m² en mars.")
        );
        assert_eq!(
            at("Mets 200 g de fumier par m² en mars."),
            Some("Mets 200 g de fumier par m² en mars.")
        );
        assert_eq!(at("Pique à 3 cm"), Some("Pique à 3 cm."));
        assert_eq!(at("Tomate | 30 cm"), Some("| Tomate | 30 cm |"));
        assert_eq!(at("Sol"), None);
        // Citation avec sa marque de liste ; citation sur deux lignes.
        assert_eq!(at("- Pique à 3 cm."), Some("- Pique à 3 cm."));
        assert_eq!(at("Puis tu arroses.\n- Pique"), None);
        assert_eq!(at("absent"), None);
        // Apostrophes typographiques ou droites, guillemets du juge.
        let text = "L’oignon chasse la mouche. Fin.";
        assert_eq!(
            sentence_span(text, "«L'oignon chasse la mouche.»").map(|(s, e)| &text[s..e]),
            Some("L’oignon chasse la mouche.")
        );
    }

    #[test]
    fn a_copied_remark_is_found_only_when_new() {
        let remark = "Ajouter de la chaux seulement après analyse du sol, en respectant les \
                      recommandations locales.";
        let before = "Mets 1 kg de chaux par m².";
        let fixed = "Mets de la chaux : ajouter de la chaux seulement après analyse du sol, en \
                     respectant les recommandations locales.";
        assert_eq!(
            copied_runs(remark, fixed, before, 8),
            vec![
                "ajouter de la chaux seulement apres analyse du sol en respectant les \
                 recommandations locales"
                    .to_owned()
            ]
        );
        // Sept mots seulement, ou déjà dans la version d'avant : rien.
        assert!(copied_runs(remark, "après analyse du sol, en respectant les", "", 8).is_empty());
        assert!(copied_runs(remark, fixed, fixed, 8).is_empty());
    }

    #[test]
    fn overlap_ignores_stop_words_and_plurals() {
        assert!(jaccard("Les tomates du balcon", "La tomate au balcon") > 0.99);
        assert!(jaccard("Le sol", "Les nuisibles") < 0.01);
    }
}
