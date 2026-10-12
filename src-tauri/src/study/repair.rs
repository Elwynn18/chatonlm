//! Réponses du modèle remises en état avant d'être lues : raisonnement
//! retiré, objet JSON isolé, échappements invalides doublés, formules LaTeX
//! abîmées par JSON réparées.
//!
//! Constaté au banc (`.tools/logs/47-study-eval.json`) : un modèle qui écrit
//! `"$\frac{1}{2}$"` dans du JSON donne, une fois lu, un saut de page suivi
//! de « rac{1}{2} » ; de même `\b` (`\beta`), `\t` (`\times`), `\r`
//! (`\rho`) et `\n` (`\neq`, `\nabla`).

use serde_json::Value;

/// Contenu sans raisonnement écrit dedans : ce qui suit le dernier
/// `</think>` (raisonnement ouvert par le gabarit du modèle, ou entre
/// `<think>` et `</think>`) ; un `<think>` jamais fermé : ce qui le précède.
pub fn strip_thinking(content: &str) -> &str {
    if let Some(end) = content.rfind("</think>") {
        return content[end + "</think>".len()..].trim();
    }
    match content.find("<think>") {
        Some(start) => content[..start].trim(),
        None => content.trim(),
    }
}

/// Objet JSON de la réponse : du premier `{` au dernier `}` (un modèle peut
/// l'entourer d'un bloc de code ou d'une phrase) ; échappements invalides
/// doublés si la lecture échoue ; textes réparés ([`repair_value`]).
/// `None` : pas d'objet lisible.
pub fn parse_object(content: &str) -> Option<Value> {
    let content = strip_thinking(content);
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    let text = content.get(start..=end)?;
    let mut value = serde_json::from_str::<Value>(text)
        .or_else(|_| serde_json::from_str::<Value>(&fix_escapes(text)))
        .ok()?;
    if !value.is_object() {
        return None;
    }
    repair_value(&mut value);
    Some(value)
}

/// Échappements que JSON refuse (`\a`, `\s`, `\u` sans quatre chiffres
/// hexadécimaux…) : barre oblique doublée, le texte est gardé tel quel.
fn fix_escapes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut fixed = String::with_capacity(text.len() + 8);
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if c != '\\' {
            fixed.push(c);
            index += 1;
            continue;
        }
        let next = chars.get(index + 1).copied();
        let valid = match next {
            Some('"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') => true,
            Some('u') => {
                chars.len() > index + 5
                    && chars[index + 2..index + 6]
                        .iter()
                        .all(char::is_ascii_hexdigit)
            }
            _ => false,
        };
        if valid {
            fixed.push('\\');
            if let Some(next) = next {
                fixed.push(next);
            }
            index += 2;
        } else {
            fixed.push_str("\\\\");
            index += 1;
        }
    }
    fixed
}

/// Répare tous les textes d'une valeur JSON lue ([`repair_text`]).
pub fn repair_value(value: &mut Value) {
    match value {
        Value::String(text) => {
            let repaired = repair_text(text);
            if repaired != *text {
                *text = repaired;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(repair_value),
        Value::Object(map) => map.values_mut().for_each(repair_value),
        _ => {}
    }
}

/// Commandes LaTeX courantes qui commencent par `n` : `\n` suivi du reste
/// de leur nom, dans une formule, est une commande lue comme saut de ligne.
/// Les plus longues d'abord.
const N_COMMANDS: &[&str] = &[
    "normalsize",
    "nsubseteq",
    "nparallel",
    "nwarrow",
    "nearrow",
    "nexists",
    "natural",
    "newline",
    "nolimits",
    "notin",
    "nabla",
    "nleq",
    "ngeq",
    "nmid",
    "neq",
    "neg",
    "not",
    "ne",
    "ni",
    "nu",
];

/// Texte généré remis en état, ligne par ligne :
/// - saut de page → `\f`, retour arrière → `\b` (partout) ;
/// - hors des blocs de code (``` ou ~~~, indentés compris) et du `code` en
///   ligne, tabulation ou retour chariot suivis d'une lettre → `\t`, `\r`,
///   sauf l'indentation en début de ligne ;
/// - saut de ligne suivi du reste d'une commande de [`N_COMMANDS`] → `\n`,
///   seulement dans une formule : `$…$` ouverte avant lui et fermée après
///   lui selon les règles de [`inline_math`] (un montant « $5 » n'ouvre
///   rien), ou formule d'affichage `$$…$$` ouverte.
pub fn repair_text(text: &str) -> String {
    if !text.contains(['\x0c', '\x08', '\t', '\r', '\n']) {
        return text.to_owned();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let mut repaired = String::with_capacity(text.len() + 8);
    let mut in_fence = false;
    let mut in_display = false;
    // Ligne en cours, jointures `\n` comprises (pour apparier ses `$`).
    let mut logical = String::new();
    for (index, line) in lines.iter().enumerate() {
        let mut fresh = true;
        if index > 0 {
            if !in_fence && joins(&logical, in_display, &lines[index..]) {
                repaired.push_str("\\n");
                logical.push_str("\\n");
                fresh = false;
            } else {
                repaired.push('\n');
                logical.clear();
            }
        }
        let trimmed = line.trim_start();
        if fresh && !in_display && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            in_fence = !in_fence;
            let line = controls(line);
            repaired.push_str(&line);
            continue;
        }
        if in_fence {
            repaired.push_str(&controls(line));
            continue;
        }
        let line = repair_line(line, fresh);
        for _ in 0..displays(&line) {
            in_display = !in_display;
        }
        repaired.push_str(&line);
        logical.push_str(&line);
    }
    repaired
}

/// Saut de page et retour arrière, seuls (blocs de code).
fn controls(line: &str) -> String {
    line.replace('\x0c', "\\f").replace('\x08', "\\b")
}

/// Une ligne hors bloc de code ; `fresh` : elle commence vraiment (pas une
/// suite jointe), ses tabulations de tête sont l'indentation.
fn repair_line(line: &str, fresh: bool) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut repaired = String::with_capacity(line.len() + 4);
    let mut leading = fresh;
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if leading && !c.is_whitespace() {
            leading = false;
        }
        let next = chars.get(index + 1).copied();
        match c {
            '\x0c' => repaired.push_str("\\f"),
            '\x08' => repaired.push_str("\\b"),
            '`' => {
                let run = chars[index..].iter().take_while(|c| **c == '`').count();
                let end = code_end(&chars, index, run).unwrap_or(index + run);
                for c in &chars[index..end] {
                    match c {
                        '\x0c' => repaired.push_str("\\f"),
                        '\x08' => repaired.push_str("\\b"),
                        other => repaired.push(*other),
                    }
                }
                index = end;
                continue;
            }
            '\t' | '\r' if !leading && next.is_some_and(|n| n.is_ascii_alphabetic()) => {
                repaired.push('\\');
                repaired.push(if c == '\t' { 't' } else { 'r' });
            }
            other => repaired.push(other),
        }
        index += 1;
    }
    repaired
}

/// Nombre de `$$` d'une ligne hors `code` en ligne.
fn displays(line: &str) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let mut count = 0;
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '\\' => index += 2,
            '`' => {
                let run = chars[index..].iter().take_while(|c| **c == '`').count();
                index = code_end(&chars, index, run).unwrap_or(index + run);
            }
            '$' if chars.get(index + 1) == Some(&'$') => {
                count += 1;
                index += 2;
            }
            _ => index += 1,
        }
    }
    count
}

/// `$` ouvrant de `chars` encore sans fermant à la fin de la ligne (et sans
/// autre `$` après lui), hors `code` en ligne et `$$`.
fn pending_opener(chars: &[char]) -> Option<usize> {
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '\\' => index += 2,
            '`' => {
                let run = chars[index..].iter().take_while(|c| **c == '`').count();
                index = code_end(chars, index, run).unwrap_or(index + run);
            }
            '$' if chars.get(index + 1) == Some(&'$') => index += 2,
            '$' => match closing_dollar(chars, index, false) {
                Closing::At(end) => index = end + 1,
                _ => {
                    let open = chars.get(index + 1).is_some_and(|c| !c.is_whitespace())
                        && !chars[index + 1..].contains(&'$');
                    if open {
                        return Some(index);
                    }
                    index += 1;
                }
            },
            _ => index += 1,
        }
    }
    None
}

/// Le saut de ligne devant `following[0]` est une commande `\n…` lue par
/// JSON : la ligne suivante commence par le reste d'une commande et, dans
/// une formule d'affichage, c'est tout ; sinon la formule `$…$` ouverte
/// dans `logical` se ferme plus loin (sur cette ligne, ou une suivante qui
/// commence elle aussi par une commande ainsi coupée).
fn joins(logical: &str, in_display: bool, following: &[&str]) -> bool {
    if n_command_str(following[0]).is_none() {
        return false;
    }
    if in_display {
        return true;
    }
    let mut candidate: Vec<char> = logical.chars().collect();
    let Some(opener) = pending_opener(&candidate) else {
        return false;
    };
    let join = candidate.len();
    for (index, line) in following.iter().enumerate() {
        if index > 0 && n_command_str(line).is_none() {
            break;
        }
        candidate.extend(['\\', 'n']);
        candidate.extend(line.chars());
        if let Closing::At(end) = closing_dollar(&candidate, opener, false) {
            return end > join;
        }
    }
    false
}

/// Comme [`n_command`], au début d'un texte.
fn n_command_str(text: &str) -> Option<&'static str> {
    let chars: Vec<char> = text.chars().take(12).collect();
    n_command(&chars)
}

fn starts_with(chars: &[char], prefix: &str) -> bool {
    let mut chars = chars.iter();
    prefix.chars().all(|p| chars.next() == Some(&p))
}

/// Reste du nom d'une commande de [`N_COMMANDS`] au début de `chars`, suivi
/// d'autre chose qu'une lettre.
fn n_command(chars: &[char]) -> Option<&'static str> {
    N_COMMANDS.iter().find_map(|command| {
        let rest = &command[1..];
        let length = rest.len();
        (starts_with(chars, rest) && !chars.get(length).is_some_and(|c| c.is_ascii_alphabetic()))
            .then_some(rest)
    })
}

/// Titre sans Markdown : `**`, `__`, `#` en tête, `*` ou `_` autour retirés.
pub fn plain_heading(heading: &str) -> String {
    let text = heading.replace("**", "").replace("__", "");
    let text = text.trim().trim_start_matches('#').trim();
    let text = text
        .strip_prefix('*')
        .and_then(|t| t.strip_suffix('*'))
        .or_else(|| text.strip_prefix('_').and_then(|t| t.strip_suffix('_')))
        .unwrap_or(text);
    text.trim().to_owned()
}

/// Formules en ligne `$…$` écrites en `\(…\)` : le rendu Markdown de l'app
/// (réglage de l'officiel `singleDollarTextMath: false`) n'affiche que
/// `\(…\)` et `$$…$$`, mais les consignes demandent `$…$` (mesuré au banc).
/// Une paire tient sur une ligne, sans espace après le `$` ouvrant ni avant
/// le `$` fermant, et le `$` fermant n'est pas suivi d'un chiffre : « $5 and
/// $10 », « costs $5. » restent tels quels. Rien ne change dans un bloc de
/// code délimité ni dans du `code` en ligne ; `$$…$$`, `\[…\]` et `\(…\)`
/// sont gardés.
///
/// Deux réparations aussi (vues chez gemma3:12b) : un `$` suivi d'une
/// commande et jamais fermé sur la ligne est fermé en fin de ligne, avant la
/// ponctuation finale ; une commande nue hors formule (`\frac{1}{5}`,
/// `\sqrt{2}`, `\times`, `\le`, `\pi`…) est entourée de `\(…\)`.
pub fn inline_math(text: &str) -> String {
    scan(text, false)
}

/// Comme [`inline_math`], pour un texte encore en cours d'arrivée : la
/// conversion s'arrête au premier endroit encore indécis de la dernière
/// ligne (`$` sans fermant pour l'instant, code en ligne ouvert, barre
/// oblique finale) ; le résultat est toujours un début de la conversion du
/// texte complet.
#[cfg(test)]
pub fn inline_math_partial(text: &str) -> String {
    scan(text, true)
}

fn scan(text: &str, partial: bool) -> String {
    let mut output = String::with_capacity(text.len() + 16);
    let mut in_fence = false;
    // Formule d'affichage ouverte (`$$` ou `\[`) : son délimiteur fermant.
    let mut display: Option<&'static str> = None;
    for line in text.split_inclusive('\n') {
        let last = !line.ends_with('\n');
        let trimmed = line.trim_start();
        if display.is_none() && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            in_fence = !in_fence;
            output.push_str(line);
            continue;
        }
        if in_fence {
            output.push_str(line);
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        if scan_line(&chars, 0, partial && last, &mut display, &mut output).is_some() {
            break;
        }
    }
    output
}

/// Position de `needle` dans `chars` à partir de `from`.
fn find(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let needle: Vec<char> = needle.chars().collect();
    (from..chars.len()).find(|at| chars[*at..].starts_with(&needle))
}

/// Convertit une ligne (`chars`, depuis `from`) dans `output` ; `Some(i)`
/// si la conversion s'arrête en `i` (ligne en cours, endroit indécis) : tout
/// ce qui précède `i` est converti pour de bon, la suite reprendra en `i`.
fn scan_line(
    chars: &[char],
    from: usize,
    partial: bool,
    display: &mut Option<&'static str>,
    output: &mut String,
) -> Option<usize> {
    let mut index = from;
    while index < chars.len() {
        // Formule d'affichage : copiée telle quelle jusqu'à son fermant.
        if let Some(close) = *display {
            let close_len = close.chars().count();
            match find(chars, index, close) {
                Some(at) => {
                    let end = at + close_len;
                    output.extend(&chars[index..end]);
                    index = end;
                    *display = None;
                }
                // Fermant peut-être coupé en fin de texte : attendu.
                None if partial => {
                    let safe = chars.len().saturating_sub(close_len - 1).max(index);
                    output.extend(&chars[index..safe]);
                    return Some(safe);
                }
                None => {
                    output.extend(&chars[index..]);
                    return None;
                }
            }
            continue;
        }
        let c = chars[index];
        match c {
            '\\' => {
                let Some(&next) = chars.get(index + 1) else {
                    if partial {
                        return Some(index);
                    }
                    output.push(c);
                    index += 1;
                    continue;
                };
                match next {
                    // Formule déjà délimitée : gardée telle quelle.
                    '(' => match find(chars, index + 2, "\\)") {
                        Some(at) => {
                            output.extend(&chars[index..at + 2]);
                            index = at + 2;
                        }
                        None if partial => return Some(index),
                        None => {
                            output.push_str("\\(");
                            index += 2;
                        }
                    },
                    '[' => {
                        output.push_str("\\[");
                        index += 2;
                        *display = Some("\\]");
                    }
                    next if next.is_ascii_alphabetic() => {
                        match bare_command(chars, index, partial) {
                            Bare::Formula(end) => {
                                output.push_str("\\(");
                                output.extend(&chars[index..end]);
                                output.push_str("\\)");
                                index = end;
                            }
                            Bare::Text(end) => {
                                output.extend(&chars[index..end]);
                                index = end;
                            }
                            Bare::Undecided => return Some(index),
                        }
                    }
                    _ => {
                        output.push(c);
                        output.push(next);
                        index += 2;
                    }
                }
            }
            '`' => {
                let run = chars[index..].iter().take_while(|c| **c == '`').count();
                match code_end(chars, index, run) {
                    Some(end) => {
                        output.extend(&chars[index..end]);
                        index = end;
                    }
                    // Code ouvert : la suite peut encore le fermer.
                    None if partial => return Some(index),
                    None => {
                        output.extend(&chars[index..index + run]);
                        index += run;
                    }
                }
            }
            '$' if chars.get(index + 1) == Some(&'$') => {
                output.push_str("$$");
                index += 2;
                *display = Some("$$");
            }
            '$' => match closing_dollar(chars, index, partial) {
                Closing::At(end) => {
                    output.push_str("\\(");
                    output.extend(&chars[index + 1..end]);
                    output.push_str("\\)");
                    index = end + 1;
                }
                Closing::Undecided if partial => return Some(index),
                Closing::Undecided | Closing::None => match unclosed_formula(chars, index) {
                    // `$\frac…` jamais fermé : fermé en fin de ligne.
                    Some(end) => {
                        output.push_str("\\(");
                        output.extend(&chars[index + 1..end]);
                        output.push_str("\\)");
                        index = end;
                    }
                    None => {
                        output.push('$');
                        index += 1;
                    }
                },
            },
            _ => {
                output.push(c);
                index += 1;
            }
        }
    }
    None
}

/// Formule ouverte par `$` à `open`, commencée par une commande (`\` et une
/// lettre) et jamais fermée sur la ligne (aucun autre `$`) : fin (exclue) de
/// son contenu, avant les espaces et la ponctuation de fin de ligne.
fn unclosed_formula(chars: &[char], open: usize) -> Option<usize> {
    if chars.get(open + 1) != Some(&'\\')
        || !chars.get(open + 2).is_some_and(char::is_ascii_alphabetic)
    {
        return None;
    }
    let line_end = chars.iter().position(|c| *c == '\n').unwrap_or(chars.len());
    if chars[open + 1..line_end].contains(&'$') {
        return None;
    }
    let mut end = line_end;
    while end > open + 1 && (chars[end - 1].is_whitespace() || ".,;:!?".contains(chars[end - 1])) {
        end -= 1;
    }
    (end > open + 1).then_some(end)
}

/// Commande LaTeX hors formule.
enum Bare {
    /// À entourer de `\(…\)` : fin (exclue).
    Formula(usize),
    /// Texte gardé tel quel : fin (exclue).
    Text(usize),
    /// Texte en cours d'arrivée : la suite décidera.
    Undecided,
}

/// Fractions et racines (avec leurs groupes `{…}`) entourées en entier.
const BARE_GROUPS: &[(&str, usize)] = &[("frac", 2), ("dfrac", 2), ("sqrt", 1)];

/// Symboles entourés seuls.
const BARE_SYMBOLS: &[&str] = &[
    "times", "div", "cdot", "pm", "le", "leq", "ge", "geq", "neq", "approx", "pi", "infty",
];

/// Commande nue (`\frac{1}{5}`, `\times`…) qui commence à `start`.
fn bare_command(chars: &[char], start: usize, partial: bool) -> Bare {
    let name_end = (start + 1..chars.len())
        .find(|at| !chars[*at].is_ascii_alphabetic())
        .unwrap_or(chars.len());
    if name_end == chars.len() && partial {
        return Bare::Undecided;
    }
    let name: String = chars[start + 1..name_end].iter().collect();
    if BARE_SYMBOLS.contains(&name.as_str()) {
        return Bare::Formula(name_end);
    }
    let Some((_, groups)) = BARE_GROUPS.iter().find(|(command, _)| *command == name) else {
        return Bare::Text(name_end);
    };
    let mut end = name_end;
    // Racine d'ordre n : `\sqrt[3]{…}`.
    if name == "sqrt" && chars.get(end) == Some(&'[') {
        match find(chars, end, "]") {
            Some(at) => end = at + 1,
            None if partial => return Bare::Undecided,
            None => return Bare::Text(name_end),
        }
    }
    for _ in 0..*groups {
        match brace_group(chars, end) {
            Ok(group_end) => end = group_end,
            Err(true) if partial => return Bare::Undecided,
            Err(_) => return Bare::Text(name_end),
        }
    }
    Bare::Formula(end)
}

/// Groupe `{…}` (accolades équilibrées, sur la ligne) qui commence à `at` :
/// fin (exclue) ; `Err(true)` : texte fini avant la fin du groupe,
/// `Err(false)` : pas de groupe.
fn brace_group(chars: &[char], at: usize) -> Result<usize, bool> {
    match chars.get(at) {
        None => return Err(true),
        Some('{') => {}
        Some(_) => return Err(false),
    }
    let mut depth = 0usize;
    for (offset, c) in chars[at..].iter().enumerate() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(at + offset + 1);
                }
            }
            '\n' => return Err(false),
            _ => {}
        }
    }
    Err(true)
}

/// Fin (exclue) du code en ligne ouvert par `run` accents graves à `start`.
fn code_end(chars: &[char], start: usize, run: usize) -> Option<usize> {
    let mut index = start + run;
    while index < chars.len() {
        if chars[index] == '`' {
            let length = chars[index..].iter().take_while(|c| **c == '`').count();
            if length == run {
                return Some(index + length);
            }
            index += length;
        } else {
            index += 1;
        }
    }
    None
}

enum Closing {
    At(usize),
    /// Pas de fermant : `$` gardé.
    None,
    /// La suite du texte (encore à venir) décidera.
    Undecided,
}

/// `$` fermant d'une formule ouverte à `open` (sur la même ligne) ;
/// `partial` : la ligne peut encore s'allonger.
fn closing_dollar(chars: &[char], open: usize, partial: bool) -> Closing {
    let content_start = open + 1;
    match chars.get(content_start) {
        None if partial => return Closing::Undecided,
        None => return Closing::None,
        Some(c) if c.is_whitespace() => return Closing::None,
        Some(_) => {}
    }
    let mut index = content_start;
    while index < chars.len() {
        match chars[index] {
            '\\' => index += 2,
            '\n' => return Closing::None,
            // Le premier `$` décide : fermant valable, ou pas de formule
            // (un montant « $2 » n'est jamais apparié au fermant d'une
            // formule plus loin : « costs $2 and $x$ »).
            '$' => {
                let after = chars.get(index + 1);
                let before_ok = index > content_start && !chars[index - 1].is_whitespace();
                return match after {
                    Some('$') => Closing::None,
                    // Suivant encore inconnu (`$$`, chiffre ?).
                    None if before_ok && partial => Closing::Undecided,
                    None if before_ok => Closing::At(index),
                    Some(next) if before_ok && !next.is_ascii_digit() => Closing::At(index),
                    _ => Closing::None,
                };
            }
            _ => index += 1,
        }
    }
    if partial {
        Closing::Undecided
    } else {
        Closing::None
    }
}

/// Conversion de [`inline_math`] au fil d'un flux : chaque fragment envoyé
/// prolonge les précédents ; un passage encore indécis attend la suite.
/// Travail linéaire : une ligne finie n'est plus relue, la ligne en cours
/// reprend là où sa conversion s'était arrêtée.
#[derive(Default)]
pub struct MathStream {
    /// Ligne en cours (jusqu'à son saut de ligne compris).
    line: Vec<char>,
    /// Début (dans `line`) de ce qui reste à convertir.
    pos: usize,
    /// Formule d'affichage ouverte en `pos`.
    display: Option<&'static str>,
    /// Dans un bloc de code délimité (lignes finies).
    in_fence: bool,
    /// Tout ce qui a été envoyé.
    sent: String,
}

impl MathStream {
    /// Ajoute un fragment visible ; renvoie le texte converti nouveau.
    pub fn push(&mut self, fragment: &str) -> String {
        let mut out = String::new();
        for c in fragment.chars() {
            self.line.push(c);
            if c == '\n' {
                self.convert(true, &mut out);
                self.line.clear();
                self.pos = 0;
            }
        }
        self.convert(false, &mut out);
        self.sent.push_str(&out);
        out
    }

    /// Convertit la ligne en cours depuis `pos` ; `complete` : elle est finie.
    fn convert(&mut self, complete: bool, out: &mut String) {
        let first = self.line.iter().position(|c| !c.is_whitespace());
        let starts_fence = first.is_some_and(|at| {
            self.line[at..].starts_with(&['`'; 3]) || self.line[at..].starts_with(&['~'; 3])
        });
        if self.in_fence || (starts_fence && self.display.is_none()) {
            // Ligne de bloc de code (ou qui l'ouvre, la ferme) : telle quelle.
            if complete && starts_fence && self.display.is_none() {
                self.in_fence = !self.in_fence;
            }
            if complete || self.in_fence {
                out.extend(&self.line[self.pos..]);
                self.pos = self.line.len();
            }
            return;
        }
        // Peut encore devenir une ligne de clôture : attendue.
        let maybe_fence = first.is_none_or(|at| matches!(self.line[at], '`' | '~'));
        if !complete && maybe_fence {
            return;
        }
        let mut display = self.display;
        match scan_line(&self.line, self.pos, !complete, &mut display, out) {
            Some(stop) => self.pos = stop,
            None => self.pos = self.line.len(),
        }
        self.display = display;
    }

    /// Fin du flux : reste du texte final (converti) pas encore envoyé, si
    /// ce qui a été envoyé en est bien le début.
    pub fn finish(&mut self, text: &str) -> String {
        let rest = text
            .strip_prefix(self.sent.as_str())
            .unwrap_or_default()
            .to_owned();
        self.sent = text.to_owned();
        rest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Réponse brute de qwen3:8b au banc (leçon « Additionner des fractions »,
    /// début) : `\frac` écrit sans doubler la barre oblique.
    const QWEN3_8B_FRACTIONS: &str = r#"{"sections": [{"heading": "**Qu'est-ce qu'une fraction ?**", "body": "Une **fraction** est une façon de représenter une **partie d'un tout**."}, {"heading": "**Exemple de fraction**", "body": "Prenons la fraction $\frac{3}{4}$. Cela signifie que le **tout** est divisé en **4 parts** égales, et on a **3 parts**. On peut aussi dire que $\frac{3}{4}$ est égal à 3 divisé par 4."}], "example": {"problem": "Imaginez que vous avez une tarte divisée en 8 parts égales. Vous mangez 3 parts. Quelle fraction de la tarte avez-vous mangé ?\n\n", "solution": "La fraction est donc $\frac{3}{8}$."}, "keyPoints": ["Exemple : $\frac{3}{4}$ signifie 3 parts sur 4."], "pitfall": "Confondre le **numérateur** et le **dénominateur**."}"#;

    /// Réponse brute de qwen3:4b-instruct au banc (même sujet, extrait) :
    /// barre doublée une fois (`$\\frac{1}{2}$`), simple ailleurs.
    const QWEN3_4B_FRACTIONS: &str = r#"{ "keyPoints": [ "Exemple : $\frac{1}{2}$ = une partie sur deux." ], "pitfall": "Mettez que $\\frac{1}{2}$ veut dire '1 + 2' ou '1 fois 2'.\nC'est une erreur. La fraction $\frac{1}{2}$ ne veut pas dire 1 plus 2.", "sections": [ { "body": "Un verre d’eau divisé en 4 parties : $\frac{1}{4}$ d’eau.\n- Une pizza divisée en 8 morceaux : $\frac{1}{8}$ d’une pizza.", "heading": "Fractions dans la vie" } ] }"#;

    #[test]
    fn fractions_written_by_real_models_are_repaired() {
        let value = parse_object(QWEN3_8B_FRACTIONS).unwrap();
        // Sans réparation, JSON lit un saut de page.
        let unrepaired: Value = serde_json::from_str(QWEN3_8B_FRACTIONS).unwrap();
        assert!(
            unrepaired["sections"][1]["body"]
                .as_str()
                .unwrap()
                .contains("$\x0crac{3}{4}$")
        );
        let body = value["sections"][1]["body"].as_str().unwrap();
        assert!(
            body.contains("Prenons la fraction $\\frac{3}{4}$."),
            "{body}"
        );
        assert!(body.contains("que $\\frac{3}{4}$ est"), "{body}");
        assert_eq!(
            value["example"]["solution"],
            "La fraction est donc $\\frac{3}{8}$."
        );
        assert_eq!(
            value["keyPoints"][0],
            "Exemple : $\\frac{3}{4}$ signifie 3 parts sur 4."
        );
        // Sauts de ligne hors formule gardés.
        assert!(
            value["example"]["problem"]
                .as_str()
                .unwrap()
                .ends_with("mangé ?\n\n")
        );
        let all = value.to_string();
        assert!(!all.contains('\x0c'));

        let value = parse_object(QWEN3_4B_FRACTIONS).unwrap();
        assert_eq!(
            value["pitfall"],
            "Mettez que $\\frac{1}{2}$ veut dire '1 + 2' ou '1 fois 2'.\nC'est une erreur. \
             La fraction $\\frac{1}{2}$ ne veut pas dire 1 plus 2."
        );
        assert_eq!(
            value["sections"][0]["body"],
            "Un verre d’eau divisé en 4 parties : $\\frac{1}{4}$ d’eau.\n\
             - Une pizza divisée en 8 morceaux : $\\frac{1}{8}$ d’une pizza."
        );
    }

    #[test]
    fn other_escapes_read_as_control_characters_are_repaired() {
        let raw = r#"{"a": "$\beta \times \theta \rho$ and $$a \neq b, \nabla f, \nu$$", "b": "$x\ny$ stays, \n\nequations too: $a\nequation$"}"#;
        let value = parse_object(raw).unwrap();
        assert_eq!(
            value["a"],
            "$\\beta \\times \\theta \\rho$ and $$a \\neq b, \\nabla f, \\nu$$"
        );
        // Saut de ligne dans une formule suivi d'autre chose qu'une commande,
        // ou hors formule : gardé.
        assert_eq!(value["b"], "$x\ny$ stays, \n\nequations too: $a\nequation$");
    }

    #[test]
    fn code_blocks_keep_their_tabs() {
        let text =
            "Voici :\n```python\nfor x in xs:\n\tif x:\n\t\tprint(x)\n```\nPuis $a\tb$ et \trho.";
        assert_eq!(
            repair_text(text),
            "Voici :\n```python\nfor x in xs:\n\tif x:\n\t\tprint(x)\n```\nPuis $a\\tb$ et \\trho."
        );
        // Un `$` dans un bloc de code n'ouvre pas de formule.
        let shell = "```sh\necho $HOME\nnotify\n```";
        assert_eq!(repair_text(shell), shell);
    }

    #[test]
    fn invalid_escapes_are_kept_as_text() {
        let raw = r#"{"a": "$\alpha + \sqrt{2}$ \u00e9 \uZZ"}"#;
        assert!(serde_json::from_str::<Value>(raw).is_err());
        assert_eq!(
            parse_object(raw).unwrap()["a"],
            "$\\alpha + \\sqrt{2}$ é \\uZZ"
        );
    }

    #[test]
    fn reasoning_and_wrappers_are_removed() {
        assert_eq!(
            parse_object("<think>\nLet me {think}.\n</think>\n```json\n{\"a\": 1}\n```").unwrap(),
            serde_json::json!({ "a": 1 })
        );
        assert_eq!(
            parse_object("Opened by the template {x} </think> {\"a\": 2}").unwrap(),
            serde_json::json!({ "a": 2 })
        );
        assert_eq!(
            strip_thinking("{\"a\": 3} <think> never closed"),
            "{\"a\": 3}"
        );
        assert_eq!(parse_object("no object"), None);
        assert_eq!(parse_object("[1, 2]"), None);
        assert_eq!(parse_object("{\"a\": "), None);
    }

    #[test]
    fn headings_lose_their_markdown() {
        assert_eq!(
            plain_heading("**Qu'est-ce qu'une fraction ?**"),
            "Qu'est-ce qu'une fraction ?"
        );
        assert_eq!(plain_heading("## Le numérateur"), "Le numérateur");
        assert_eq!(plain_heading("*Steps*"), "Steps");
        assert_eq!(plain_heading("  __Key__ idea "), "Key idea");
        assert_eq!(plain_heading("x * y"), "x * y");
    }

    #[test]
    fn inline_formulas_are_written_for_the_app() {
        // Cas du banc (après la réparation de `\frac`).
        assert_eq!(
            inline_math("Prenons la fraction $\\frac{3}{4}$. Elle vaut $0,75$."),
            "Prenons la fraction \\(\\frac{3}{4}\\). Elle vaut \\(0,75\\)."
        );
        // Montants, `$` seul, espace après l'ouvrant ou avant le fermant.
        for kept in [
            "It costs $5 and $10 together.",
            "costs $5.",
            "a $ b $ c",
            "a $ b$ c",
            "$x $",
            "one $ sign",
            "$5$5",
            "price: 3$",
        ] {
            assert_eq!(inline_math(kept), kept, "{kept}");
        }
        // Formule d'affichage gardée, formule en ligne voisine convertie.
        assert_eq!(
            inline_math("$$\\frac{a}{b}$$ and $x^2$"),
            "$$\\frac{a}{b}$$ and \\(x^2\\)"
        );
        assert_eq!(inline_math("$$\nx = 1\n$$"), "$$\nx = 1\n$$");
        // Code en ligne et blocs de code : jamais touchés.
        assert_eq!(
            inline_math("Shell: `echo $HOME$` then $y$."),
            "Shell: `echo $HOME$` then \\(y\\)."
        );
        let code = "```bash\necho $a$b$\n```\nThen $z$.";
        assert_eq!(inline_math(code), "```bash\necho $a$b$\n```\nThen \\(z\\).");
        // Une paire ne passe pas d'une ligne à l'autre ; `\$` est un dollar.
        assert_eq!(inline_math("$a\nb$"), "$a\nb$");
        assert_eq!(inline_math("\\$5 and $x$"), "\\$5 and \\(x\\)");
        // Déjà converti : inchangé.
        let done = "\\(\\frac{1}{2}\\)";
        assert_eq!(inline_math(done), done);
    }

    #[test]
    fn streamed_formulas_wait_for_their_closing_dollar() {
        let text = "Half is $\\frac{1}{2}$, so `$x` costs $5 and $y$.\n```\n$z$\n```\nEnd $w$";
        let expected = inline_math(text);
        // Tous les découpages en deux, puis caractère par caractère.
        for cut in 1..text.len() {
            if !text.is_char_boundary(cut) {
                continue;
            }
            let mut stream = MathStream::default();
            let mut sent = stream.push(&text[..cut]);
            sent.push_str(&stream.push(&text[cut..]));
            assert!(expected.starts_with(&sent), "{cut}: {sent:?}");
            sent.push_str(&stream.finish(&expected));
            assert_eq!(sent, expected, "{cut}");
        }
        let mut stream = MathStream::default();
        let mut sent = String::new();
        for c in text.chars() {
            sent.push_str(&stream.push(&c.to_string()));
            // Jamais de `$` ouvrant envoyé avant de savoir s'il est une formule.
            assert!(expected.starts_with(&sent), "{sent:?}");
        }
        sent.push_str(&stream.finish(&expected));
        assert_eq!(sent, expected);
        // Partiel : arrêt au `$` encore ouvert, au code ouvert, à la barre
        // finale.
        assert_eq!(inline_math_partial("Half is $\\frac{1"), "Half is ");
        assert_eq!(inline_math_partial("Half is $x$"), "Half is ");
        assert_eq!(inline_math_partial("Half is $x$."), "Half is \\(x\\).");
        assert_eq!(inline_math_partial("Run `ls $"), "Run ");
        assert_eq!(inline_math_partial("A \\"), "A ");
        assert_eq!(
            inline_math_partial("costs $5 and\nmore"),
            "costs $5 and\nmore"
        );
    }

    #[test]
    fn a_formula_never_closed_is_closed_at_the_end_of_its_line() {
        // Cas vu chez gemma3:12b (leçon sur les fractions).
        assert_eq!(
            inline_math(
                "on multiplie par 2 : $\\frac{1}{6} \\times \\frac{2}{2} = \\frac{2}{12}.\n4. **Additionner**"
            ),
            "on multiplie par 2 : \\(\\frac{1}{6} \\times \\frac{2}{2} = \\frac{2}{12}\\).\n4. **Additionner**"
        );
        assert_eq!(inline_math("Soit $\\pi r^2  "), "Soit \\(\\pi r^2\\)  ");
        // Pas une commande, ou un autre `$` sur la ligne : gardé.
        for kept in ["costs $5 now.", "a $x and $5", "$\\ alone"] {
            assert_eq!(inline_math(kept), kept, "{kept}");
        }
        // Un autre `$` sur la ligne : pas de formule fermée en fin de ligne
        // (la commande nue est délimitée seule).
        assert_eq!(
            inline_math("$\\frac{1}{2} costs $5"),
            "$\\(\\frac{1}{2}\\) costs $5"
        );
        // En flux : attend la fin de la ligne.
        assert_eq!(inline_math_partial("par 2 : $\\frac{1}{6} = x"), "par 2 : ");
        assert_eq!(
            inline_math_partial("par 2 : $\\frac{1}{6}.\nSuite"),
            "par 2 : \\(\\frac{1}{6}\\).\nSuite"
        );
    }

    #[test]
    fn bare_commands_outside_formulas_are_delimited() {
        // Cas vu chez gemma3:12b (énoncé d'une question).
        assert_eq!(
            inline_math(
                "Si on veut additionner  \\frac{1}{5} et \\frac{1}{10}, quel est le plus petit multiple ?"
            ),
            "Si on veut additionner  \\(\\frac{1}{5}\\) et \\(\\frac{1}{10}\\), quel est le plus petit multiple ?"
        );
        assert_eq!(
            inline_math(
                "3 \\times 4 \\le 12, \\sqrt{16} = 4, \\sqrt[3]{8} = 2, \\dfrac{a}{b+{c}} \\approx \\pi"
            ),
            "3 \\(\\times\\) 4 \\(\\le\\) 12, \\(\\sqrt{16}\\) = 4, \\(\\sqrt[3]{8}\\) = 2, \\(\\dfrac{a}{b+{c}}\\) \\(\\approx\\) \\(\\pi\\)"
        );
        // Rien de touché : formules déjà délimitées, code, autres commandes,
        // noms plus longs, fraction incomplète.
        for kept in [
            "Half is \\(\\frac{1}{2} \\times 2\\).",
            "$$\\frac{1}{2} \\times 2$$",
            "$$\n\\frac{1}{2}\n\\le 1\n$$ then",
            "\\[\\frac{a}{b}\\]",
            "`\\frac{1}{2}` and `\\times`",
            "```latex\n\\frac{1}{2} \\times 3\n```",
            "\\left( x \\right) \\leftarrow \\pitchfork",
            "\\frac{1} alone, \\frac 12, \\sqrt x",
            "\\textbf{bold}",
        ] {
            assert_eq!(inline_math(kept), kept, "{kept}");
        }
        // Formule `$…$` : convertie une seule fois, sans rien entourer dedans.
        assert_eq!(
            inline_math("$\\frac{1}{2} \\times 2$"),
            "\\(\\frac{1}{2} \\times 2\\)"
        );
        // En flux : une commande coupée attend la suite.
        assert_eq!(inline_math_partial("x \\fra"), "x ");
        assert_eq!(inline_math_partial("x \\le"), "x ");
        assert_eq!(inline_math_partial("x \\frac{1}{"), "x ");
        assert_eq!(inline_math_partial("x \\le 2"), "x \\(\\le\\) 2");
        let text = "Add \\frac{1}{5} and $\\frac{1}{10}$, then \\times 2: $\\frac{3}{10}.\nNext \\(x\\) \\pi";
        let expected = inline_math(text);
        let mut stream = MathStream::default();
        let mut sent = String::new();
        for c in text.chars() {
            sent.push_str(&stream.push(&c.to_string()));
            assert!(expected.starts_with(&sent), "{sent:?}");
        }
        sent.push_str(&stream.finish(&expected));
        assert_eq!(sent, expected);
    }

    /// rust-8 : un montant n'est jamais apparié au fermant d'une formule.
    #[test]
    fn amounts_are_never_paired_with_a_formula() {
        assert_eq!(
            inline_math("A pen costs $2 and a notebook costs $x$."),
            "A pen costs $2 and a notebook costs \\(x\\)."
        );
        assert_eq!(
            inline_math("If apples cost $3 each and you buy $n$ apples"),
            "If apples cost $3 each and you buy \\(n\\) apples"
        );
        assert_eq!(
            inline_math("$2x + 1 = 5$ so $x = 2$"),
            "\\(2x + 1 = 5\\) so \\(x = 2\\)"
        );
    }

    /// rust-11 : une commande coupée par JSON n'est réparée que dans une
    /// formule fermée plus loin ; un vrai saut de ligne reste.
    #[test]
    fn real_line_breaks_after_an_amount_or_code_stay() {
        for kept in [
            "It costs $5 a month.\nparallel lines never meet.",
            "It costs $5 a month.\nparallel lines cost $3.",
            "Use `$HOME` here.\ni.e. your home folder.",
            "Pay $10\nu can wait.",
            "$x$ is small.\ne.g. one.",
        ] {
            assert_eq!(repair_text(kept), kept, "{kept:?}");
        }
        // Deux commandes coupées dans la même formule.
        let raw = r#"{"a": "$a \neq b \nabla c$ ok"}"#;
        assert_eq!(parse_object(raw).unwrap()["a"], "$a \\neq b \\nabla c$ ok");
        // Formule d'affichage sur plusieurs lignes.
        assert_eq!(
            repair_text("$$\na \neq b\n$$\nnot this"),
            "$$\na \\neq b\n$$\nnot this"
        );
    }

    /// rust-12 : blocs de code indentés ou ~~~, et indentation en tête de
    /// ligne : tabulations gardées.
    #[test]
    fn indented_and_tilde_code_blocks_keep_their_tabs() {
        for kept in [
            "1. Write:\n   ```python\n   def f():\n   \treturn 1\n   ```",
            "~~~go\nfunc f() {\n\treturn\n}\n~~~",
            "\tindented line",
            "Inline `a\tb` code.",
            // Tabulation au milieu d'une ligne de code : gardée aussi.
            "~~~go\nx :=\tvalue\n~~~",
            "1. Run:\n   ```sh\n   echo a\tb\n   ```",
        ] {
            assert_eq!(repair_text(kept), kept, "{kept:?}");
        }
        assert_eq!(repair_text("x\ty"), "x\\ty");
        // Saut de page : réparé même dans un bloc de code.
        assert_eq!(repair_text("~~~\n\x0crac\n~~~"), "~~~\n\\frac\n~~~");
    }
}
