//! Texte des messages : longueur comptée comme les messageries, découpage
//! d'une réponse trop longue, Markdown d'un modèle rendu pour Telegram.
//!
//! Le Markdown est lu par un petit analyseur écrit ici, ligne par ligne,
//! plutôt que par une bibliothèque CommonMark : le texte d'origine (listes,
//! tableaux, retraits) est gardé tel quel, seules les marques reconnues sont
//! remplacées, et une marque sans fin (texte en cours d'écriture) reste du
//! texte au lieu de changer la structure de tout le message.

/// Longueur d'un texte comme la comptent Telegram et Discord : en unités
/// UTF-16.
pub fn length(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Ouverture d'un bloc de code (``` ou ~~~), lue de la même façon par le
/// découpage et par le rendu.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Fence {
    mark: char,
    count: usize,
    /// Retrait de la ligne d'ouverture, en octets (bloc dans une liste).
    indent: usize,
    /// Langue et suite de la ligne d'ouverture.
    info: String,
}

impl Fence {
    fn open(line: &str) -> Option<Self> {
        let body = line.trim_start();
        let mark = body
            .chars()
            .next()
            .filter(|mark| matches!(mark, '`' | '~'))?;
        let count = body.chars().take_while(|c| *c == mark).count();
        let info = body[count..].trim();
        // « ```code``` » sur une ligne est du code en ligne, pas un bloc.
        if count < 3 || (mark == '`' && info.contains('`')) {
            return None;
        }
        Some(Self {
            mark,
            count,
            indent: line.len() - body.len(),
            info: info.to_owned(),
        })
    }

    fn closes(&self, line: &str) -> bool {
        let body = line.trim();
        body.chars().all(|c| c == self.mark) && body.len() >= self.count
    }

    fn marker(&self) -> String {
        self.mark.to_string().repeat(self.count)
    }
}

/// Découpe un texte Markdown en messages de `limit` unités UTF-16 au plus,
/// dans l'ordre. Coupe de préférence à un paragraphe, sinon à une ligne, sinon
/// à une espace, jamais au milieu d'un caractère. Un bloc de code (```)
/// coupé est refermé à la fin d'un message et rouvert, avec sa langue, au
/// début du suivant. Un texte qui tient rend un seul message, inchangé ; un
/// texte vide ne rend rien.
///
/// Une coupe trop près du début (avant le quart du message) est écartée : un
/// titre suivi d'un long paragraphe ne part pas seul. Sous 2 unités, la limite
/// ne peut pas être tenue pour un caractère hors du plan de base.
pub fn split(markdown: &str, limit: usize) -> Vec<String> {
    if markdown.trim().is_empty() {
        return Vec::new();
    }
    if length(markdown) <= limit {
        return vec![markdown.to_owned()];
    }
    let mut messages = Vec::new();
    let mut rest = markdown;
    let mut open = None;
    while !rest.trim().is_empty() {
        let piece = next_piece(rest, open.as_ref(), limit);
        if !piece.message.trim().is_empty() {
            messages.push(piece.message);
        }
        // `used` vaut au moins un caractère : la boucle finit toujours.
        rest = rest[piece.used..].trim_start_matches('\n');
        open = piece.open;
    }
    messages
}

struct Piece {
    message: String,
    /// Octets du texte consommés, séparateur de la coupe compris.
    used: usize,
    /// Bloc de code encore ouvert à la coupe.
    open: Option<Fence>,
}

fn next_piece(rest: &str, open: Option<&Fence>, limit: usize) -> Piece {
    let mut head = open
        .map(|fence| format!("{}{}\n", fence.marker(), fence.info))
        .unwrap_or_default();
    // Place de la fermeture la plus longue qui puisse servir dans ce message.
    let widest = prefix_end(rest, limit.saturating_sub(length(&head)));
    let longest = scan(&rest[..widest], open).longest;
    let mut reserve = if longest > 0 { longest + 1 } else { 0 };
    // Limite minuscule : rouvrir et refermer le bloc prendrait la place du
    // texte. Il est alors coupé sans réparation.
    let repair = length(&head) + reserve <= limit / 2;
    if !repair {
        head.clear();
        reserve = 0;
    }
    let room = limit.saturating_sub(length(&head));
    if length(rest) <= room {
        return Piece {
            message: format!("{head}{rest}"),
            used: rest.len(),
            open: None,
        };
    }

    let window = prefix_end(rest, room.saturating_sub(reserve));
    let (mut end, mut next) = boundary(rest, window);
    let mut state = scan(&rest[..end], open);
    if let (Some(_), Some(start)) = (&state.open, state.opened_at) {
        let after_opening = rest[start..end]
            .split_once('\n')
            .map_or("", |(_, code)| code);
        if after_opening.trim().is_empty() && !rest[..start].trim().is_empty() {
            // Le bloc s'ouvrirait sur la dernière ligne du message : il part
            // en entier dans le suivant.
            (end, next) = (start, start);
            state = scan(&rest[..end], open);
        }
    }
    if let Some(start) = state.opened_at
        && !rest[start..end].contains('\n')
        && !rest[end..].starts_with('\n')
    {
        // Ligne d'ouverture elle-même coupée (limite minuscule) : plus de
        // bloc à refermer ni à rouvrir.
        state.open = None;
    }
    if let Some(fence) = &state.open
        && repair
        && rest[..next].ends_with('\n')
    {
        let line = rest[next..].split('\n').next().unwrap_or_default();
        let closed = next + line.len();
        if fence.closes(line) && length(&rest[..closed]) <= room {
            // La place gardée pour refermer le bloc sert à sa vraie fermeture.
            (end, next) = (closed, closed);
            state.open = None;
        }
    }

    let mut message = format!("{head}{}", rest[..end].trim_end());
    if let Some(fence) = state.open.as_ref().filter(|_| repair) {
        message.push('\n');
        message.push_str(&fence.marker());
    }
    Piece {
        message,
        used: next,
        open: state.open,
    }
}

/// Fin (en octets) du plus long début de `text` qui tient en `units` unités
/// UTF-16 ; un caractère au moins, pour toujours avancer.
fn prefix_end(text: &str, units: usize) -> usize {
    let mut used = 0;
    for (at, c) in text.char_indices() {
        used += c.len_utf16();
        if used > units && at > 0 {
            return at;
        }
    }
    text.len()
}

/// Où couper dans `rest[..window]` : fin du message et début de la suite (le
/// séparateur entre les deux est abandonné).
fn boundary(rest: &str, window: usize) -> (usize, usize) {
    // Un séparateur juste après la fenêtre fait une coupe sans perte de place.
    let extra = rest[window..]
        .bytes()
        .take(2)
        .take_while(|byte| matches!(byte, b'\n' | b' '))
        .count();
    let text = &rest[..window + extra];
    let floor = (window / 4).max(1);
    let usable = |at: &usize| *at >= floor && !text[..*at].trim().is_empty();
    let skip = |at: usize, separator: char| {
        at + rest[at..].len() - rest[at..].trim_start_matches(separator).len()
    };
    if let Some(at) = text.rfind("\n\n").filter(usable) {
        return (at, skip(at, '\n'));
    }
    if let Some(at) = text.rfind('\n').filter(usable) {
        return (at, at + 1);
    }
    if let Some(at) = text.rfind(' ').filter(usable) {
        return (at, skip(at, ' '));
    }
    (window, window)
}

struct Scan {
    /// Bloc ouvert à la fin du texte.
    open: Option<Fence>,
    /// Début de sa ligne d'ouverture, si elle est dans le texte.
    opened_at: Option<usize>,
    /// Plus longue marque parmi les blocs rencontrés (0 : aucun).
    longest: usize,
}

fn scan(text: &str, open: Option<&Fence>) -> Scan {
    let mut scan = Scan {
        open: open.cloned(),
        opened_at: None,
        longest: open.map_or(0, |fence| fence.count),
    };
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches('\n');
        match &scan.open {
            Some(fence) if fence.closes(content) => {
                scan.open = None;
                scan.opened_at = None;
            }
            Some(_) => {}
            None => {
                if let Some(fence) = Fence::open(content) {
                    scan.longest = scan.longest.max(fence.count);
                    scan.open = Some(fence);
                    scan.opened_at = Some(at);
                }
            }
        }
        at += line.len();
    }
    scan
}

/// Rend le Markdown d'un modèle en HTML de Telegram (`parse_mode=HTML`) :
/// gras, italique, barré, code, blocs de code (avec leur langue), liens,
/// citations ; titres en gras, listes gardées en texte. Tout le reste est
/// échappé (`&`, `<`, `>`). Le résultat est toujours bien formé, même pour un
/// texte en cours d'écriture (marque ou bloc non fermé).
pub fn telegram_html(markdown: &str) -> String {
    let lines: Vec<&str> = markdown.lines().collect();
    let html = blocks(&lines, false);
    let html = html.trim_end().trim_start_matches('\n');
    if is_blank(html) {
        // Rien à montrer (« ``` » seul, « # » seul) : Telegram refuserait un
        // message vide, le texte part tel quel.
        return escape(markdown.trim());
    }
    html.to_owned()
}

/// Vrai si le HTML ne montre aucun caractère.
fn is_blank(html: &str) -> bool {
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag && !c.is_whitespace() => return false,
            _ => {}
        }
    }
    true
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

fn escape_attribute(text: &str) -> String {
    escape(text).replace('"', "&quot;")
}

/// Blocs d'une suite de lignes : code, citation, sinon ligne par ligne.
/// `quoted` : déjà dans une citation (Telegram ne les imbrique pas).
fn blocks(lines: &[&str], quoted: bool) -> String {
    let mut parts = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        let line = lines[at];
        if let Some(fence) = Fence::open(line) {
            let length = lines[at + 1..]
                .iter()
                .position(|line| fence.closes(line))
                .unwrap_or(lines.len() - at - 1);
            parts.push(code_block(&fence, &lines[at + 1..at + 1 + length]));
            // Ouverture, code, et fermeture si elle existe.
            at += length + 2;
        } else if !quoted && quote_content(line).is_some() {
            let inner: Vec<&str> = lines[at..]
                .iter()
                .map_while(|line| quote_content(line))
                .collect();
            at += inner.len();
            let html = blocks(&inner, true);
            let html = html.trim_end().trim_start_matches('\n');
            parts.push(if is_blank(html) {
                String::new()
            } else {
                format!("<blockquote>{html}</blockquote>")
            });
        } else {
            parts.push(line_html(line));
            at += 1;
        }
    }
    parts.join("\n")
}

/// Texte d'une ligne de citation, sans son « > ».
fn quote_content(line: &str) -> Option<&str> {
    let content = line.trim_start().strip_prefix('>')?;
    Some(content.strip_prefix(' ').unwrap_or(content))
}

fn code_block(fence: &Fence, lines: &[&str]) -> String {
    let code = lines
        .iter()
        .map(|line| {
            // Le retrait de l'ouverture (bloc dans une liste) n'est pas du code.
            let indent = line
                .bytes()
                .take(fence.indent)
                .take_while(|byte| matches!(byte, b' ' | b'\t'))
                .count();
            &line[indent..]
        })
        .collect::<Vec<_>>()
        .join("\n");
    if code.trim().is_empty() {
        // Bloc tout juste ouvert : rien à montrer encore.
        return String::new();
    }
    let language: String = fence
        .info
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-' | '.' | '#'))
        .collect();
    let code = escape(&code);
    if language.is_empty() {
        format!("<pre>{code}</pre>")
    } else {
        format!("<pre><code class=\"language-{language}\">{code}</code></pre>")
    }
}

/// Ligne hors bloc : titre, trait, élément de liste ou texte.
fn line_html(line: &str) -> String {
    let body = line.trim_start();
    let indent = &line[..line.len() - body.len()];
    let level = body.bytes().take_while(|byte| *byte == b'#').count();
    if (1..=6).contains(&level) && body[level..].chars().next().is_none_or(char::is_whitespace) {
        let title = body[level..].trim().trim_end_matches('#').trim_end();
        return inline(title, true);
    }
    let marks: Vec<char> = body.chars().filter(|c| !c.is_whitespace()).collect();
    if marks.len() >= 3
        && matches!(marks[0], '-' | '*' | '_')
        && marks.iter().all(|c| *c == marks[0])
    {
        return "———".to_owned();
    }
    for bullet in ["- ", "* ", "+ "] {
        if let Some(item) = body.strip_prefix(bullet) {
            return format!("{indent}• {}", inline(item, false));
        }
    }
    format!("{indent}{}", inline(body, false))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    Bold,
    Italic,
    Strike,
}

enum Token {
    Text(String),
    Code(String),
    Link {
        href: String,
        label: Vec<Token>,
    },
    /// Suite de `*`, `_` ou `~` qui peut ouvrir ou fermer une mise en forme.
    Run {
        mark: char,
        count: usize,
        opens: bool,
        closes: bool,
    },
}

enum Op {
    Text(String),
    Code(String),
    Open(Style),
    Close(Style),
    LinkOpen(String),
    LinkClose,
}

fn inline(text: &str, heading: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut ops = Vec::new();
    resolve(tokenize(&chars, true), &mut ops);
    let mut emitter = Emitter {
        bold: usize::from(heading),
        ..Emitter::default()
    };
    for op in ops {
        emitter.apply(op);
    }
    emitter.finish()
}

fn is_space(c: Option<char>) -> bool {
    c.is_none_or(char::is_whitespace)
}

fn is_punctuation(c: Option<char>) -> bool {
    c.is_some_and(|c| !c.is_whitespace() && !c.is_alphanumeric())
}

fn push_text(tokens: &mut Vec<Token>, text: impl AsRef<str>) {
    if let Some(Token::Text(last)) = tokens.last_mut() {
        last.push_str(text.as_ref());
    } else {
        tokens.push(Token::Text(text.as_ref().to_owned()));
    }
}

fn tokenize(chars: &[char], links: bool) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        let run = chars[at..].iter().take_while(|other| **other == c).count();
        match c {
            '\\' if chars.get(at + 1).is_some_and(char::is_ascii_punctuation) => {
                push_text(&mut tokens, chars[at + 1].to_string());
                at += 2;
            }
            '`' => {
                if let Some(end) = code_end(chars, at + run, run) {
                    tokens.push(Token::Code(code_text(&chars[at + run..end])));
                    at = end + run;
                } else {
                    push_text(&mut tokens, "`".repeat(run));
                    at += run;
                }
            }
            '!' | '[' | '<' if links => {
                // Une image devient un lien vers elle.
                let start = at + usize::from(c == '!');
                let link = if c == '<' {
                    autolink(chars, at)
                } else {
                    link(chars, start)
                };
                if let Some((token, end)) = link {
                    match token {
                        // Lien refusé (pas http) : son texte seul.
                        Token::Link { href, label } if href.is_empty() => tokens.extend(label),
                        token => tokens.push(token),
                    }
                    at = end;
                } else {
                    push_text(&mut tokens, c.to_string());
                    at += 1;
                }
            }
            '*' | '_' | '~' => {
                let previous = at.checked_sub(1).map(|before| chars[before]);
                let next = chars.get(at + run).copied();
                if let Some(token) = delimiter(c, run, previous, next) {
                    tokens.push(token);
                } else {
                    push_text(&mut tokens, c.to_string().repeat(run));
                }
                at += run;
            }
            c => {
                push_text(&mut tokens, c.to_string());
                at += 1;
            }
        }
    }
    tokens
}

/// Début de la suite d'exactement `count` accents graves qui ferme un code
/// ouvert avant `from`.
fn code_end(chars: &[char], from: usize, count: usize) -> Option<usize> {
    let mut at = from;
    while at < chars.len() {
        if chars[at] != '`' {
            at += 1;
            continue;
        }
        let run = chars[at..].iter().take_while(|c| **c == '`').count();
        if run == count {
            return Some(at);
        }
        at += run;
    }
    None
}

fn code_text(chars: &[char]) -> String {
    let text: String = chars.iter().collect();
    // « `` `x` `` » : l'espace qui sépare du code un accent grave est retirée.
    match text
        .strip_prefix(' ')
        .and_then(|text| text.strip_suffix(' '))
    {
        Some(inner) if !inner.trim().is_empty() => inner.to_owned(),
        _ => text,
    }
}

/// Marque de mise en forme, ou rien si la suite reste du texte. Règles de
/// CommonMark, durcies : un `_` ou un `*` seul entre deux lettres ou chiffres
/// (`snake_case`, `2*3*4`) n'ouvre ni ne ferme rien.
fn delimiter(
    mark: char,
    count: usize,
    previous: Option<char>,
    next: Option<char>,
) -> Option<Token> {
    let left = !is_space(next)
        && (!is_punctuation(next) || is_space(previous) || is_punctuation(previous));
    let right = !is_space(previous)
        && (!is_punctuation(previous) || is_space(next) || is_punctuation(next));
    let (opens, closes) = match mark {
        '~' if count == 2 => (!is_space(next), !is_space(previous)),
        '~' => return None,
        _ if count > 3 => return None,
        '*' if count == 1
            && left
            && right
            && !is_punctuation(previous)
            && !is_punctuation(next) =>
        {
            return None;
        }
        '*' => (left, right),
        _ => (
            left && (!right || is_punctuation(previous)),
            right && (!left || is_punctuation(next)),
        ),
    };
    (opens || closes).then_some(Token::Run {
        mark,
        count,
        opens,
        closes,
    })
}

/// Lien `[texte](adresse "titre")` qui commence à `start` (son crochet). Rend
/// le lien et la position qui le suit ; une adresse qui n'est pas http(s) rend
/// un lien sans adresse (son texte seul sera gardé).
fn link(chars: &[char], start: usize) -> Option<(Token, usize)> {
    if chars.get(start) != Some(&'[') {
        return None;
    }
    let mut depth = 0;
    let mut at = start;
    let close = loop {
        match chars.get(at)? {
            '\\' => at += 1,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    break at;
                }
            }
            _ => {}
        }
        at += 1;
    };
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    at = close + 2;
    while chars.get(at).is_some_and(|c| *c == ' ') {
        at += 1;
    }
    let mut address = String::new();
    if chars.get(at) == Some(&'<') {
        at += 1;
        while *chars.get(at)? != '>' {
            address.push(chars[at]);
            at += 1;
        }
        at += 1;
    } else {
        let mut depth = 0;
        loop {
            match *chars.get(at)? {
                c if c.is_whitespace() => break,
                '(' => depth += 1,
                ')' if depth == 0 => break,
                ')' => depth -= 1,
                _ => {}
            }
            address.push(chars[at]);
            at += 1;
        }
    }
    while chars.get(at).is_some_and(|c| *c == ' ') {
        at += 1;
    }
    if let Some(quote) = chars.get(at).copied().filter(|c| matches!(c, '"' | '\'')) {
        // Titre du lien : Telegram n'en a pas.
        at += 1;
        while *chars.get(at)? != quote {
            at += 1;
        }
        at += 1;
        while chars.get(at).is_some_and(|c| *c == ' ') {
            at += 1;
        }
    }
    if chars.get(at) != Some(&')') {
        return None;
    }
    let href = web_address(&address).unwrap_or_default();
    let mut label = tokenize(&chars[start + 1..close], false);
    if label.is_empty() {
        label.push(Token::Text(address));
    }
    Some((Token::Link { href, label }, at + 1))
}

/// Lien `<https://…>`.
fn autolink(chars: &[char], start: usize) -> Option<(Token, usize)> {
    let length = chars[start + 1..].iter().position(|c| *c == '>')?;
    let address: String = chars[start + 1..start + 1 + length].iter().collect();
    let href = web_address(&address)?;
    let label = vec![Token::Text(address)];
    Some((Token::Link { href, label }, start + length + 2))
}

/// Seules les adresses du web deviennent des liens (ni `javascript:`, ni
/// `tg://`, ni chemin de fichier).
fn web_address(address: &str) -> Option<String> {
    let lower = address.to_ascii_lowercase();
    let host = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))?;
    let plain = !address.chars().any(|c| c.is_whitespace() || c.is_control());
    (plain && !host.is_empty()).then(|| address.to_owned())
}

/// Associe les marques ouvrantes et fermantes (pile des ouvertures, comme
/// CommonMark) ; une marque sans partenaire reste du texte.
fn resolve(tokens: Vec<Token>, ops: &mut Vec<Op>) {
    struct Pending {
        left: usize,
        closing: Vec<Style>,
        opening: Vec<Style>,
    }
    let mut pending: Vec<Option<Pending>> = tokens.iter().map(|_| None).collect();
    let mut stack: Vec<usize> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let Token::Run {
            mark,
            count,
            opens,
            closes,
        } = token
        else {
            continue;
        };
        let mut left = *count;
        let mut closing = Vec::new();
        while *closes && left > 0 {
            let same = |opener: &usize| matches!(&tokens[*opener], Token::Run { mark: other, .. } if other == mark);
            let Some(position) = stack.iter().rposition(same) else {
                break;
            };
            // Les ouvertures restées au-dessus n'ont pas trouvé leur fin.
            stack.truncate(position + 1);
            let opener = pending[stack[position]].as_mut().expect("opener");
            let (used, style) = match mark {
                '~' => (2, Style::Strike),
                _ if opener.left >= 2 && left >= 2 => (2, Style::Bold),
                _ => (1, Style::Italic),
            };
            opener.left -= used;
            opener.opening.push(style);
            left -= used;
            closing.push(style);
            if opener.left == 0 {
                stack.pop();
            }
        }
        if *opens && left > 0 {
            stack.push(index);
        }
        pending[index] = Some(Pending {
            left,
            closing,
            opening: Vec::new(),
        });
    }

    for (token, pending) in tokens.into_iter().zip(pending) {
        match token {
            Token::Text(text) => ops.push(Op::Text(text)),
            Token::Code(code) => ops.push(Op::Code(code)),
            Token::Link { href, label } => {
                ops.push(Op::LinkOpen(href));
                resolve(label, ops);
                ops.push(Op::LinkClose);
            }
            Token::Run { mark, .. } => {
                let pending = pending.expect("run");
                ops.extend(pending.closing.into_iter().map(Op::Close));
                if pending.left > 0 {
                    ops.push(Op::Text(mark.to_string().repeat(pending.left)));
                }
                // La première paire trouvée est la plus intérieure.
                ops.extend(pending.opening.into_iter().rev().map(Op::Open));
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tag {
    /// Numéro du lien : deux liens qui se suivent ne partagent pas leur balise.
    Link(usize),
    Style(Style),
}

/// Écrit le HTML d'une ligne. Les balises ne sont ouvertes qu'au moment
/// d'écrire du texte, toujours dans le même ordre, et refermées à l'envers :
/// le résultat est bien formé quelles que soient les marques lues.
#[derive(Default)]
struct Emitter {
    out: String,
    open: Vec<Tag>,
    bold: usize,
    italic: usize,
    strike: usize,
    link: Option<String>,
    links: usize,
}

impl Emitter {
    fn apply(&mut self, op: Op) {
        match op {
            Op::Text(text) => {
                self.sync();
                self.out.push_str(&escape(&text));
            }
            // Telegram ne met pas de code dans un lien : le texte y reste nu.
            Op::Code(code) if self.link.is_some() => self.apply(Op::Text(code)),
            Op::Code(code) => {
                // Ni gras ni italique autour du code (entités que Telegram
                // n'imbrique pas) : elles reprennent après lui.
                self.close_from(0);
                self.out.push_str("<code>");
                self.out.push_str(&escape(&code));
                self.out.push_str("</code>");
            }
            Op::Open(style) => *self.depth(style) += 1,
            Op::Close(style) => {
                let depth = self.depth(style);
                *depth = depth.saturating_sub(1);
            }
            Op::LinkOpen(href) => {
                self.links += 1;
                self.link = Some(href);
            }
            Op::LinkClose => self.link = None,
        }
    }

    fn depth(&mut self, style: Style) -> &mut usize {
        match style {
            Style::Bold => &mut self.bold,
            Style::Italic => &mut self.italic,
            Style::Strike => &mut self.strike,
        }
    }

    /// Ouvre et ferme ce qu'il faut pour que les balises ouvertes soient
    /// celles de la mise en forme du moment.
    fn sync(&mut self) {
        let mut wanted = Vec::new();
        if self.link.is_some() {
            wanted.push(Tag::Link(self.links));
        }
        for (style, depth) in [
            (Style::Bold, self.bold),
            (Style::Italic, self.italic),
            (Style::Strike, self.strike),
        ] {
            if depth > 0 {
                wanted.push(Tag::Style(style));
            }
        }
        let common = self
            .open
            .iter()
            .zip(&wanted)
            .take_while(|(open, wanted)| open == wanted)
            .count();
        self.close_from(common);
        for tag in &wanted[common..] {
            match tag {
                Tag::Link(_) => {
                    let href = escape_attribute(self.link.as_deref().unwrap_or_default());
                    self.out.push_str(&format!("<a href=\"{href}\">"));
                }
                Tag::Style(style) => self.out.push_str(&format!("<{}>", Self::name(*style))),
            }
            self.open.push(*tag);
        }
    }

    fn close_from(&mut self, keep: usize) {
        while self.open.len() > keep {
            match self.open.pop() {
                Some(Tag::Link(_)) => self.out.push_str("</a>"),
                Some(Tag::Style(style)) => self.out.push_str(&format!("</{}>", Self::name(style))),
                None => {}
            }
        }
    }

    fn name(style: Style) -> &'static str {
        match style {
            Style::Bold => "b",
            Style::Italic => "i",
            Style::Strike => "s",
        }
    }

    fn finish(mut self) -> String {
        self.close_from(0);
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Découpage ---

    /// Texte sans ses espaces : ce que le découpage ne doit jamais perdre.
    fn dense(text: &str) -> String {
        text.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// Messages recollés, sans les lignes d'ouverture et de fermeture des
    /// blocs de code (celles que le découpage ajoute comme celles du texte).
    fn dense_without_fences(text: &str) -> String {
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim_start().starts_with("```"))
            .collect();
        dense(&lines.join("\n"))
    }

    /// Chaque message doit avoir ses blocs de code fermés.
    fn fences_are_closed(message: &str) -> bool {
        scan(message, None).open.is_none()
    }

    #[test]
    fn length_counts_utf16_units() {
        assert_eq!(length("chat"), 4);
        assert_eq!(length("é"), 1);
        assert_eq!(length("🐱"), 2);
        assert_eq!(length("👩‍👩‍👧"), 8);
    }

    #[test]
    fn a_text_that_fits_stays_one_unchanged_message() {
        let text = "  # Titre\n\n```rust\nlet a = 1;\n\nSans fin, avec des espaces  \n";
        assert_eq!(split(text, 4000), vec![text.to_owned()]);
        assert_eq!(split("abc", 3), vec!["abc".to_owned()]);
    }

    #[test]
    fn an_empty_text_gives_no_message() {
        assert!(split("", 4000).is_empty());
        assert!(split(" \n\t\n", 4000).is_empty());
        assert!(split(&" ".repeat(50), 10).is_empty());
    }

    #[test]
    fn no_message_goes_beyond_the_limit_in_utf16_units() {
        let text = "Chaton 🐱 roux 𝒞𝒽𝒶𝓉 ".repeat(200)
            + "\n\n"
            + &"🐱".repeat(500)
            + "\n"
            + &"mot ".repeat(300);
        for limit in [2, 3, 5, 16, 64, 100, 4000] {
            let messages = split(&text, limit);
            assert!(messages.len() > 1, "limit {limit}");
            for message in &messages {
                assert!(
                    length(message) <= limit,
                    "limit {limit}: {} units",
                    length(message)
                );
                assert!(!message.trim().is_empty(), "limit {limit}: blank message");
            }
            assert_eq!(dense(&messages.concat()), dense(&text), "limit {limit}");
        }
    }

    #[test]
    fn nothing_is_lost_between_the_messages() {
        let text = "# Titre\n\nPremier paragraphe, assez long pour être coupé.\n\n- un\n- deux\n  - trois\n\n\
                    Deuxième paragraphe avec `du code` et un [lien](https://chatonlm.com).\n\n\n\nFin.";
        for limit in 8..120 {
            let messages = split(text, limit);
            assert_eq!(dense(&messages.concat()), dense(text), "limit {limit}");
            assert!(messages.iter().all(|message| length(message) <= limit));
        }
    }

    #[test]
    fn a_cut_prefers_a_paragraph_then_a_line_then_a_space() {
        // Paragraphe : la coupe à la ligne, plus loin, n'est pas prise.
        let text = "Premier paragraphe.\n\nDeuxième paragraphe,\nsur deux lignes.";
        assert_eq!(
            split(text, 45),
            vec![
                "Premier paragraphe.".to_owned(),
                "Deuxième paragraphe,\nsur deux lignes.".to_owned()
            ]
        );
        // Ligne, plutôt que l'espace qui la suit.
        assert_eq!(
            split("une ligne entière\net la suivante ici", 30),
            vec![
                "une ligne entière".to_owned(),
                "et la suivante ici".to_owned()
            ]
        );
        // Espace : aucun mot n'est coupé.
        assert_eq!(
            split("un deux trois quatre", 12),
            vec!["un deux".to_owned(), "trois quatre".to_owned()]
        );
        // Un séparateur juste après la limite : le message est plein.
        assert_eq!(
            split("abcde fghij", 5),
            vec!["abcde".to_owned(), "fghij".to_owned()]
        );
    }

    #[test]
    fn a_cut_too_close_to_the_start_is_not_taken() {
        let text = format!("Titre\n\n{}", "mot ".repeat(40));
        let messages = split(&text, 100);
        // Le titre ne part pas seul : la coupe se fait à une espace, plus loin.
        assert!(messages[0].starts_with("Titre\n\nmot mot"));
        assert!(length(&messages[0]) > 90);
    }

    #[test]
    fn a_split_code_block_is_closed_then_reopened_with_its_language() {
        let code: String = (0..40)
            .map(|line| format!("let ligne_{line} = {line};\n"))
            .collect();
        let text = format!("Voici le code :\n\n```rust\n{code}```\n\nEt la suite.");
        let messages = split(&text, 200);
        assert!(messages.len() > 3);
        for message in &messages {
            assert!(length(message) <= 200);
            assert!(fences_are_closed(message), "unclosed block in:\n{message}");
            // Aucun bloc vide ajouté par la coupe.
            assert!(
                !message.contains("```rust\n```"),
                "empty block in:\n{message}"
            );
        }
        assert!(messages[0].ends_with("\n```"));
        for message in &messages[1..messages.len() - 1] {
            assert!(message.starts_with("```rust\n"), "{message}");
            assert!(message.ends_with("\n```"), "{message}");
        }
        let last = messages.last().unwrap();
        assert!(last.starts_with("```rust\n") && last.ends_with("Et la suite."));
        assert_eq!(
            dense_without_fences(&messages.join("\n")),
            dense_without_fences(&text)
        );
        // Chaque morceau se rend en un bloc de code de la bonne langue.
        for message in &messages {
            assert!(telegram_html(message).contains("<pre><code class=\"language-rust\">"));
        }
    }

    #[test]
    fn a_code_block_is_not_opened_on_the_last_line_of_a_message() {
        let text = format!(
            "{}\n```python\n{}```",
            "mot ".repeat(20),
            "print('chaton')\n".repeat(10)
        );
        // À 99, la fenêtre finit dans la première ligne du code.
        let messages = split(&text, 99);
        assert!(!messages[0].contains("```"), "{}", messages[0]);
        assert!(messages[1].starts_with("```python\nprint"));
    }

    #[test]
    fn the_real_closing_of_a_code_block_is_kept_rather_than_added() {
        // La coupe tombe juste avant la fermeture : elle part avec le bloc.
        let text = format!(
            "```\n{}```\nSuite du texte, après le bloc de code.",
            "ligne de code\n".repeat(6)
        );
        let messages = split(&text, 92);
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[0],
            format!("```\n{}```", "ligne de code\n".repeat(6))
        );
        assert_eq!(messages[1], "Suite du texte, après le bloc de code.");
    }

    #[test]
    fn an_unclosed_code_block_stays_unclosed_only_in_the_last_message() {
        let text = format!("```sh\n{}", "echo chaton\n".repeat(30));
        let messages = split(&text, 100);
        let (last, others) = messages.split_last().unwrap();
        assert!(others.iter().all(|message| fences_are_closed(message)));
        assert!(last.starts_with("```sh\n") && !fences_are_closed(last));
    }

    #[test]
    fn a_word_longer_than_the_limit_is_cut_between_characters() {
        let word = "🐱é".repeat(100);
        let messages = split(&word, 7);
        // 3 unités par paire : 2 paires par message, jamais un demi-chaton.
        assert!(messages.iter().all(|message| length(message) <= 7));
        assert_eq!(messages.concat(), word);
        assert_eq!(messages[0], "🐱é🐱é");
    }

    #[test]
    fn a_tiny_limit_ends() {
        let text = "```rust\nlet 🐱 = \"chaton\";\n```\n\nfin de l'essai";
        for limit in 0..12 {
            let messages = split(text, limit);
            assert!(!messages.is_empty(), "limit {limit}");
            // Aux marques de bloc près : une coupe peut en ajouter.
            let kept = |text: &str| dense(text).replace('`', "");
            assert_eq!(kept(&messages.concat()), kept(text), "limit {limit}");
            if limit >= 2 {
                assert!(
                    messages.iter().all(|message| length(message) <= limit),
                    "limit {limit}"
                );
            }
        }
    }

    // --- HTML de Telegram ---

    /// Petit vérificateur du HTML produit : balises permises seulement,
    /// équilibrées, bien imbriquées, jamais vides, `&`, `<` et `>` échappés.
    fn check(html: &str) -> Result<(), String> {
        const ALLOWED: [&str; 7] = ["b", "i", "s", "code", "pre", "a", "blockquote"];
        // Balise ouverte, et position du texte à son ouverture.
        let mut stack: Vec<(String, usize)> = Vec::new();
        let mut rest = html;
        let mut shown = 0;
        while let Some(c) = rest.chars().next() {
            match c {
                '<' => {
                    let end = rest.find('>').ok_or("unclosed tag")?;
                    let tag = &rest[1..end];
                    rest = &rest[end + 1..];
                    if let Some(name) = tag.strip_prefix('/') {
                        let (open, since) = stack.pop().ok_or(format!("unexpected </{name}>"))?;
                        if open != name {
                            return Err(format!("</{name}> closes <{open}>"));
                        }
                        if since == shown {
                            return Err(format!("empty <{name}>"));
                        }
                        continue;
                    }
                    let (name, attributes) = tag.split_once(' ').unwrap_or((tag, ""));
                    if !ALLOWED.contains(&name) {
                        return Err(format!("tag <{name}> is not allowed"));
                    }
                    let parents: Vec<&str> = stack.iter().map(|(name, _)| name.as_str()).collect();
                    let valid = match name {
                        "a" => attributes
                            .strip_prefix("href=\"")
                            .and_then(|href| href.strip_suffix('"'))
                            .is_some_and(|href| {
                                href.starts_with("http") && !href.contains(['"', '<', ' '])
                            }),
                        "code" if parents.last() == Some(&"pre") => attributes
                            .strip_prefix("class=\"language-")
                            .and_then(|language| language.strip_suffix('"'))
                            .is_some_and(|language| {
                                !language.is_empty() && !language.contains(['"', '<', ' '])
                            }),
                        _ => attributes.is_empty(),
                    };
                    if !valid {
                        return Err(format!("attributes of <{tag}>"));
                    }
                    // Règles d'imbrication de Telegram.
                    let inside =
                        |names: &[&str]| parents.iter().any(|parent| names.contains(parent));
                    let nested = match name {
                        "blockquote" => !parents.is_empty(),
                        "pre" => inside(&["pre", "code", "a", "b", "i", "s"]),
                        "code" if parents.last() == Some(&"pre") => false,
                        "code" => inside(&["pre", "code", "a", "b", "i", "s"]),
                        "a" => inside(&["a", "pre", "code"]),
                        _ => inside(&["pre", "code", name]),
                    };
                    if nested {
                        return Err(format!("<{name}> inside {parents:?}"));
                    }
                    stack.push((name.to_owned(), shown));
                }
                '>' => return Err("bare >".to_owned()),
                '&' => {
                    let entity = ["&amp;", "&lt;", "&gt;", "&quot;"]
                        .into_iter()
                        .find(|entity| rest.starts_with(entity))
                        .ok_or("bare &")?;
                    rest = &rest[entity.len()..];
                    shown += 1;
                }
                c => {
                    rest = &rest[c.len_utf8()..];
                    shown += 1;
                }
            }
        }
        match stack.pop() {
            Some((name, _)) => Err(format!("<{name}> is never closed")),
            None => Ok(()),
        }
    }

    /// Texte montré par un HTML : sans balises, entités lues.
    fn shown(html: &str) -> String {
        let mut text = String::new();
        let mut in_tag = false;
        for c in html.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                c if !in_tag => text.push(c),
                _ => {}
            }
        }
        text.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&amp;", "&")
    }

    fn html(markdown: &str) -> String {
        let html = telegram_html(markdown);
        if let Err(error) = check(&html) {
            panic!("invalid HTML ({error}) for {markdown:?}:\n{html}");
        }
        html
    }

    #[test]
    fn the_checker_refuses_what_telegram_would() {
        assert!(check("<b>gras</b> et <i>italique</i>").is_ok());
        assert!(check("<b>gras").is_err());
        assert!(check("<b><i>croisé</b></i>").is_err());
        assert!(check("fin</b>").is_err());
        assert!(check("<u>souligné</u>").is_err());
        assert!(check("1 < 2").is_err());
        assert!(check("1 > 2").is_err());
        assert!(check("chat & chaton").is_err());
        assert!(check("<b></b>").is_err());
        assert!(check("<a href=\"javascript:alert(1)\">x</a>").is_err());
        assert!(check("<b><code>x</code></b>").is_err());
        assert!(check("<pre><code class=\"language-rust\">x</code></pre>").is_ok());
    }

    #[test]
    fn inline_marks_become_telegram_tags() {
        assert_eq!(html("du **gras** ici"), "du <b>gras</b> ici");
        assert_eq!(html("du __gras__ ici"), "du <b>gras</b> ici");
        assert_eq!(
            html("de l'*italique* et de l'_italique_"),
            "de l'<i>italique</i> et de l'<i>italique</i>"
        );
        assert_eq!(html("du ~~barré~~."), "du <s>barré</s>.");
        assert_eq!(html("du `code` ici"), "du <code>code</code> ici");
        assert_eq!(html("***les deux***"), "<b><i>les deux</i></b>");
        assert_eq!(
            html("**gras et *italique***"),
            "<b>gras et <i>italique</i></b>"
        );
        assert_eq!(html("**Remarque :** texte"), "<b>Remarque :</b> texte");
        assert_eq!(html("这是**重点**内容"), "这是<b>重点</b>内容");
    }

    #[test]
    fn marks_inside_a_word_or_an_identifier_stay_text() {
        assert_eq!(
            html("snake_case et un_autre_nom"),
            "snake_case et un_autre_nom"
        );
        assert_eq!(html("2*3 = 6 et 2*3*4 = 24"), "2*3 = 6 et 2*3*4 = 24");
        assert_eq!(html("a * b * c"), "a * b * c");
        assert_eq!(
            html("__init__.py et my__var__x"),
            "<b>init</b>.py et my__var__x"
        );
        assert_eq!(html("environ ~5 min"), "environ ~5 min");
        assert_eq!(html("5 * 3 et *pas fini"), "5 * 3 et *pas fini");
        assert_eq!(
            html("https://exemple.fr/a_b_c?d=*e*"),
            "https://exemple.fr/a_b_c?d=<i>e</i>"
        );
    }

    #[test]
    fn unfinished_marks_stay_text() {
        assert_eq!(html("du **gras en cours"), "du **gras en cours");
        assert_eq!(html("du `code en cours"), "du `code en cours");
        assert_eq!(
            html("un [lien](https://exemple"),
            "un [lien](https://exemple"
        );
        assert_eq!(html("un [lien en cours"), "un [lien en cours");
        assert_eq!(html("**gras _croisé** fin_"), "<b>gras _croisé</b> fin_");
        assert_eq!(html("*a **b* c**"), "<i>a b c</i>");
        assert_eq!(html("**"), "**");
        assert_eq!(html("*"), "*");
    }

    #[test]
    fn everything_else_is_escaped() {
        assert_eq!(html("1 < 2 && 3 > 2"), "1 &lt; 2 &amp;&amp; 3 &gt; 2");
        assert_eq!(html("<b>pas du gras</b>"), "&lt;b&gt;pas du gras&lt;/b&gt;");
        assert_eq!(html("`a < b && c`"), "<code>a &lt; b &amp;&amp; c</code>");
        assert_eq!(
            html("```\nif a < b && c > d {}\n```"),
            "<pre>if a &lt; b &amp;&amp; c &gt; d {}</pre>"
        );
        assert_eq!(html("\\*pas italique\\* et \\<"), "*pas italique* et &lt;");
        assert_eq!(html("&amp; déjà échappé"), "&amp;amp; déjà échappé");
    }

    #[test]
    fn links_keep_only_web_addresses() {
        assert_eq!(
            html("voir [le site](https://chatonlm.com/a?b=1&c=2)."),
            "voir <a href=\"https://chatonlm.com/a?b=1&amp;c=2\">le site</a>."
        );
        assert_eq!(
            html("[**gras** et `code`](http://exemple.fr \"titre\")"),
            "<a href=\"http://exemple.fr\"><b>gras</b> et code</a>"
        );
        assert_eq!(
            html("![un chaton](https://exemple.fr/chat.png)"),
            "<a href=\"https://exemple.fr/chat.png\">un chaton</a>"
        );
        assert_eq!(
            html("![](https://exemple.fr/chat.png)"),
            "<a href=\"https://exemple.fr/chat.png\">https://exemple.fr/chat.png</a>"
        );
        assert_eq!(
            html("<https://exemple.fr>"),
            "<a href=\"https://exemple.fr\">https://exemple.fr</a>"
        );
        assert_eq!(
            html("[a](https://exemple.fr/x_(y))[b](https://exemple.fr/z)"),
            "<a href=\"https://exemple.fr/x_(y)\">a</a><a href=\"https://exemple.fr/z\">b</a>"
        );
        // Guillemets dans l'adresse : jamais de sortie de l'attribut.
        assert_eq!(
            html("[x](<https://exemple.fr/\"onclick=\"y>)"),
            "<a href=\"https://exemple.fr/&quot;onclick=&quot;y\">x</a>"
        );
        // Ni script, ni lien interne, ni fichier : le texte seul.
        assert_eq!(html("[clic](javascript:alert(1))"), "clic");
        assert_eq!(
            html("[moi](tg://user?id=1) et [doc](./README.md)"),
            "moi et doc"
        );
        assert_eq!(html("[vide](https://)"), "vide");
        assert_eq!(html("tableau[0](x) et [ ] case"), "tableau0 et [ ] case");
    }

    #[test]
    fn headings_become_bold_lines() {
        assert_eq!(html("# Titre"), "<b>Titre</b>");
        assert_eq!(
            html("### Un **titre** avec `code` ##"),
            "<b>Un titre avec </b><code>code</code>"
        );
        assert_eq!(
            html("#chaton n'est pas un titre"),
            "#chaton n'est pas un titre"
        );
        assert_eq!(
            html("Avant\n\n## Titre\nAprès"),
            "Avant\n\n<b>Titre</b>\nAprès"
        );
    }

    #[test]
    fn lists_and_tables_stay_text() {
        assert_eq!(
            html("- un\n* deux\n  + trois **gras**\n1. premier\n2) second\n- [ ] case"),
            "• un\n• deux\n  • trois <b>gras</b>\n1. premier\n2) second\n• [ ] case"
        );
        assert_eq!(
            html("| Nom | Âge |\n|-----|:---:|\n| *Ana* | 3 |"),
            "| Nom | Âge |\n|-----|:---:|\n| <i>Ana</i> | 3 |"
        );
        assert_eq!(
            html("avant\n\n---\n* * *\naprès"),
            "avant\n\n———\n———\naprès"
        );
    }

    #[test]
    fn code_blocks_keep_their_language_and_their_text() {
        assert_eq!(
            html("```rust\nfn main() {\n    let a = \"<b>\";\n}\n```"),
            "<pre><code class=\"language-rust\">fn main() {\n    let a = \"&lt;b&gt;\";\n}</code></pre>"
        );
        assert_eq!(html("~~~\n**pas gras**\n~~~"), "<pre>**pas gras**</pre>");
        // Langue douteuse : jamais de sortie de l'attribut.
        assert_eq!(
            html("```c++\"><script> x\ncode\n```"),
            "<pre><code class=\"language-c++script\">code</code></pre>"
        );
        // Bloc dans une liste : son retrait n'est pas du code.
        assert_eq!(
            html("- étape\n  ```sh\n  ls -l\n    cat x\n  ```\nfin"),
            "• étape\n<pre><code class=\"language-sh\">ls -l\n  cat x</code></pre>\nfin"
        );
        // Bloc en cours d'écriture.
        assert_eq!(
            html("Voici :\n```python\nprint(1)"),
            "Voici :\n<pre><code class=\"language-python\">print(1)</code></pre>"
        );
        assert_eq!(html("Voici :\n```python"), "Voici :");
        assert_eq!(html("``` ``code`` ```"), "<code>``code``</code>");
    }

    #[test]
    fn quotes_become_one_blockquote() {
        assert_eq!(
            html("> une **citation**\n> sur deux lignes\nsuite"),
            "<blockquote>une <b>citation</b>\nsur deux lignes</blockquote>\nsuite"
        );
        assert_eq!(
            html("> > imbriquée"),
            "<blockquote>&gt; imbriquée</blockquote>"
        );
        assert_eq!(
            html("> ```\n> code\n> ```"),
            "<blockquote><pre>code</pre></blockquote>"
        );
        assert_eq!(html(">\ntexte"), "texte");
    }

    #[test]
    fn a_text_that_shows_nothing_is_sent_as_it_is() {
        assert_eq!(html("```"), "```");
        assert_eq!(html("#"), "#");
        assert_eq!(html(" > "), "&gt;");
        assert_eq!(telegram_html(""), "");
        assert_eq!(telegram_html(" \n "), "");
    }

    const LONG: &str = "# Chatons : le **guide** `complet`\n\n\
        Un paragraphe avec du **gras**, de l'*italique*, du ***gras italique***, du ~~barré~~, \
        du `code <en> ligne`, un [lien **gras** `code`](https://chatonlm.com/?a=1&b=\"2\" \"titre\"), \
        une image ![chaton](https://exemple.fr/chat.png), <https://exemple.fr/auto>, \
        snake_case_name, 2*3*4, a < b && c > d, **gras *imbriqué* fin** et __souligné _dedans_ __.\n\
        Une ligne \\*échappée\\*, des émojis 🐱🐈‍⬛ et **un gras `avec code` dedans**.\n\n\
        > Une citation **importante**\n> avec `du code` et [un lien](http://exemple.fr)\n> ```js\n> let a = 1 < 2;\n> ```\n> > imbriquée\n\n\
        - une puce *italique*\n  - sous-puce avec [lien](https://exemple.fr/a_(b))\n* autre puce\n1. numéro **un**\n2. numéro deux\n\n\
        | Colonne | Valeur |\n|---------|--------|\n| `a_b` | **1** |\n\n\
        ```rust\nfn main() {\n    let texte = \"<b>&amp;</b>\";\n    println!(\"{texte}\");\n}\n```\n\n\
        ---\n\n~~~\nbloc sans langue\n~~~\n\n\
        Fin avec ``du `code` double`` et [lien cassé](ftp://exemple) et ~~barré **gras** fin~~.\n\n\
        ````markdown\n```rust\nbloc dans un bloc\n```\n````\n\n## Titre ~~final~~ *!*";

    #[test]
    fn the_long_example_uses_every_kind_of_tag() {
        let html = html(LONG);
        for tag in [
            "<b>",
            "<i>",
            "<s>",
            "<code>",
            "<pre>",
            "<pre><code class=\"language-rust\">",
            "<a href=",
            "<blockquote>",
        ] {
            assert!(html.contains(tag), "{tag} is missing in:\n{html}");
        }
    }

    #[test]
    fn a_text_cut_anywhere_gives_well_formed_html() {
        // Un texte en cours d'écriture s'arrête n'importe où : chaque début
        // du long exemple doit donner un HTML que Telegram accepte.
        let ends: Vec<usize> = LONG
            .char_indices()
            .map(|(at, _)| at)
            .chain([LONG.len()])
            .collect();
        assert!(ends.len() > 1000);
        for end in ends {
            let markdown = &LONG[..end];
            let html = telegram_html(markdown);
            if let Err(error) = check(&html) {
                panic!("invalid HTML ({error}) for {markdown:?}:\n{html}");
            }
            // Jamais de message vide pour un texte qui ne l'est pas.
            assert_eq!(is_blank(&html), markdown.trim().is_empty(), "{markdown:?}");
            // La mise en forme ne fait qu'enlever des marques.
            assert!(length(&shown(&html)) <= length(markdown), "{markdown:?}");
        }
    }

    #[test]
    fn any_suffix_of_the_long_example_gives_well_formed_html() {
        // Début de message au milieu d'une marque (suite d'un découpage).
        for (start, _) in LONG.char_indices() {
            let markdown = &LONG[start..];
            if let Err(error) = check(&telegram_html(markdown)) {
                panic!("invalid HTML ({error}) for {markdown:?}");
            }
        }
    }

    #[test]
    fn random_marks_never_break_the_html_nor_the_split() {
        // Suites de marques tirées au sort (toujours les mêmes : générateur
        // congruentiel), bien plus tordues que ce qu'écrit un modèle.
        const PARTS: [&str; 32] = [
            "*",
            "**",
            "***",
            "_",
            "__",
            "~~",
            "`",
            "``",
            "```",
            "~~~",
            "```rust",
            "[",
            "]",
            "(",
            ")",
            "](",
            "<",
            ">",
            "&",
            "\\",
            "!",
            "# ",
            "- ",
            "> ",
            "\n",
            "\n\n",
            " ",
            "mot",
            "é",
            "🐱",
            "https://x.fr/a_b",
            "\"",
        ];
        let mut state: u64 = 0x00C4_A704;
        let mut random = |below: usize| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as usize % below
        };
        for _ in 0..4000 {
            let markdown: String = (0..random(50))
                .map(|_| PARTS[random(PARTS.len())])
                .collect();
            let html = telegram_html(&markdown);
            if let Err(error) = check(&html) {
                panic!("invalid HTML ({error}) for {markdown:?}:\n{html}");
            }
            assert_eq!(is_blank(&html), markdown.trim().is_empty(), "{markdown:?}");
            assert!(length(&shown(&html)) <= length(&markdown), "{markdown:?}");

            let limit = 2 + random(60);
            let messages = split(&markdown, limit);
            for message in &messages {
                assert!(
                    length(message) <= limit,
                    "limit {limit} for {markdown:?}: {message:?}"
                );
                assert!(!message.trim().is_empty(), "limit {limit} for {markdown:?}");
                if let Err(error) = check(&telegram_html(message)) {
                    panic!("invalid HTML ({error}) for the piece {message:?} of {markdown:?}");
                }
            }
            // Sans bloc de code, rien n'est ajouté ni perdu.
            if !markdown.contains("```") && !markdown.contains("~~~") {
                assert_eq!(dense(&messages.concat()), dense(&markdown), "limit {limit}");
            }
        }
    }

    #[test]
    fn the_pieces_of_a_split_text_give_well_formed_html() {
        for limit in [40, 90, 200, 500] {
            for message in split(LONG, limit) {
                assert!(length(&message) <= limit);
                html(&message);
            }
        }
    }
}
