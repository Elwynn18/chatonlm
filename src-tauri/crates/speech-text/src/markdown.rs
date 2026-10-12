//! Texte parlé tiré du Markdown d'une réponse (lecture à voix haute, fonction
//! propre à ChatonLM).
//!
//! Lus : paragraphes, titres (une phrase), éléments de liste (point final
//! ajouté s'il manque), citations, lignes de tableau (cellules séparées par
//! des virgules, une ligne = une phrase), texte des liens, code en ligne.
//! Sautés : blocs de code, formules, images, HTML brut, notes de bas de page,
//! appels de citation (« [1] », « [1][2] », lien dont le texte est un
//! numéro). Les émojis sont retirés plus tard par le prétraitement ; une
//! phrase sans lettre ni chiffre (« 🎉 ») n'est pas rendue.
//!
//! Formules : comme le rendu de l'app (remark-math, `singleDollarTextMath:
//! false`), `$$…$$`, `\(…\)` et `\[…\]` sont des formules, un `$` seul est du
//! texte (« 5 $ »). Elles sont retirées de la source avant l'analyse.
//!
//! Liens par référence : résolus même sans définition (`[a][b]` se lit « a »
//! que `[b]: url` existe ou non), pour qu'une définition arrivée plus tard,
//! ou restée avant la fenêtre d'analyse du segmenteur, ne change rien.
//!
//! Lecture automatique (`Segmenter`) : la source est analysée de nouveau à
//! chaque fragment, depuis le dernier point de reprise sûr (début de bloc de
//! premier niveau, en colonne 0, après une ligne vide, hors code et formule,
//! sur une ligne finie, toutes les feuilles d'avant lues) : le coût reste
//! proportionnel au dernier bloc ouvert, pas à toute la réponse. Une feuille
//! (paragraphe, titre, élément, ligne) est terminée quand un autre bloc
//! commence après elle, sur une ligne finie ou après une ligne vide. Dans la
//! dernière feuille, seules les phrases terminées sont rendues, et seulement
//! si rien avant leur fin ne peut encore changer de sens : `*`, `_`, `` ` ``,
//! `~`, `<`, `[` littéraux (un fermant à venir les retirerait), et tout
//! l'intérieur d'une emphase, d'un barré ou d'un lien qui en contient un (son
//! fermant pourrait être avalé par un code en ligne : « *a… `*` ») ; la
//! dernière ligne si elle contient un `|` hors code en ligne, ou l'avant-
//! dernière si la dernière ressemble au début d'une ligne de délimiteurs
//! (en-tête de tableau possible). Jamais l'intérieur d'un bloc de code
//! ouvert, jamais une ligne de tableau inachevée.
//!
//! Garantie : quel que soit le découpage en fragments, les phrases rendues
//! par `push` puis `finish`, jointes par une espace, valent `spoken_text` du
//! texte entier, sans panique. Exception connue : une ligne dont les seuls
//! `|` sont dans du code en ligne, suivie d'une ligne de délimiteurs (elle
//! devient un en-tête de tableau après avoir été lue comme un paragraphe).

use pulldown_cmark::{BrokenLink, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd};

/// « [ » et « ] » du texte (pas du code), et bornes du texte d'un lien :
/// retirés au nettoyage, après les appels de note (`[^1]`) et de citation
/// (`[1]`).
const OPEN: char = '\u{E000}';
const CLOSE: char = '\u{E001}';

/// Texte parlé de toute une réponse, en phrases séparées par des espaces.
pub fn spoken_text(markdown: &str) -> String {
    let masked = mask_math(markdown);
    let doc = parse(&masked.text);
    doc.leaves
        .iter()
        .flat_map(|leaf| sentences(&leaf.text, true))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Découpage incrémental pour la lecture automatique : chaque fragment reçu
/// pendant la génération est ajouté ; `push` rend les phrases parlées
/// devenues sûres (blocs terminés, phrases terminées du dernier paragraphe,
/// jamais l'intérieur d'un bloc de code ouvert) ; `finish` rend le reste.
/// Jointes par une espace, les sorties valent `spoken_text` du texte entier.
#[derive(Debug, Default)]
pub struct Segmenter {
    markdown: String,
    /// Début (octet de `markdown`) de la fenêtre analysée : tout ce qui
    /// précède est lu, et l'analyse depuis ce point donne les mêmes feuilles.
    base: usize,
    /// Feuilles de la fenêtre déjà lues en entier.
    done: usize,
    /// Texte brut déjà lu de la feuille `done` (un préfixe de son texte).
    read: String,
}

impl Segmenter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, fragment: &str) -> Vec<String> {
        self.markdown.push_str(fragment);
        let masked = mask_math(&self.markdown[self.base..]);
        let source = masked.text.as_str();
        let doc = parse(source);
        let mut out = Vec::new();
        while let Some(leaf) = doc.leaves.get(self.done) {
            // Terminée seulement si un début de bloc définitif la suit (même
            // une feuille suivie d'une autre : « a\n|b\n|- » est un tableau
            // dont l'en-tête « b » peut encore rejoindre le paragraphe « a »).
            if doc.closes(source, leaf) {
                out.extend(sentences(unread(&leaf.text, &self.read), true));
                self.done += 1;
                self.read.clear();
                continue;
            }
            // Feuille encore ouverte : ses phrases terminées et sûres.
            // Délimiteur en toute fin : la suite peut l'allonger (« `x` » puis
            // « `` ») et défaire le code ou le barré qu'il fermait.
            let open_run = source.ends_with(['`', '~', '*', '_']);
            let limit = [leaf.unsafe_from, table_guard(source, leaf)]
                .into_iter()
                .flatten()
                .min();
            if leaf.kind != Kind::Row
                && !open_run
                && leaf.text.starts_with(self.read.as_str())
                && let Some(end) = last_safe_end(&leaf.text, limit)
                && end > self.read.len()
            {
                out.extend(sentences(&leaf.text[self.read.len()..end], false));
                self.read = leaf.text[..end].to_owned();
            }
            break;
        }
        self.advance(&masked, &doc);
        out
    }

    pub fn finish(self) -> Vec<String> {
        let masked = mask_math(&self.markdown[self.base..]);
        let doc = parse(&masked.text);
        let mut out = Vec::new();
        for (i, leaf) in doc.leaves.iter().enumerate().skip(self.done) {
            let from = if i == self.done {
                self.read.as_str()
            } else {
                ""
            };
            out.extend(sentences(unread(&leaf.text, from), true));
        }
        out
    }

    /// Avance la fenêtre au dernier point de reprise sûr dont toutes les
    /// feuilles d'avant sont lues.
    fn advance(&mut self, masked: &Masked, doc: &Doc) {
        let source = masked.text.as_str();
        let first_unread = doc
            .leaves
            .get(self.done)
            .map_or(usize::MAX, |leaf| leaf.start);
        let restart = masked.restarts.iter().rev().copied().find(|&at| {
            at > 0 && at <= first_unread && doc.top_starts.contains(&at) && doc.is_final(source, at)
        });
        let Some(restart) = restart else { return };
        // Même ligne dans la source brute (le masque garde tous les « \n »).
        let lines = source[..restart].matches('\n').count();
        let window = &self.markdown[self.base..];
        let Some(raw) = window
            .match_indices('\n')
            .nth(lines - 1)
            .map(|(i, _)| i + 1)
        else {
            return;
        };
        self.done -= doc
            .leaves
            .iter()
            .take_while(|leaf| leaf.start < restart)
            .count();
        self.base += raw;
    }
}

/// Reste non lu de `text` après le préfixe `read` ; si le texte a changé
/// avant la coupe (cas imprévu), reprend après leur plus long préfixe
/// commun, sans jamais couper un caractère.
fn unread<'a>(text: &'a str, read: &str) -> &'a str {
    if let Some(rest) = text.strip_prefix(read) {
        return rest;
    }
    // Paragraphe devenu ligne de tableau (la barre est arrivée après la
    // coupe) : la cellule perd les espaces de tête.
    let (text, read) = (text.trim_start(), read.trim_start());
    if let Some(rest) = text.strip_prefix(read) {
        return rest;
    }
    let common = text
        .char_indices()
        .zip(read.chars())
        .take_while(|((_, a), b)| a == b)
        .last()
        .map_or(0, |((i, c), _)| i + c.len_utf8());
    &text[common..]
}

/// Début (octet du texte de `leaf`) de la ligne qui pourrait encore devenir
/// l'en-tête d'un tableau : la dernière ligne si elle a un `|` hors code en
/// ligne ; l'avant-dernière si la dernière, inachevée, ressemble au début
/// d'une ligne de délimiteurs (« |-- », « --- »).
fn table_guard(source: &str, leaf: &Leaf) -> Option<usize> {
    let src = &source[leaf.start..];
    let lines: Vec<&str> = src.split_inclusive('\n').collect();
    let last = lines.len().checked_sub(1)?;
    let text_line = |k: usize| match k {
        0 => 0,
        _ => leaf
            .line_starts
            .get(k - 1)
            .copied()
            .unwrap_or(leaf.text.len()),
    };
    let mut guard = None;
    if has_table_pipe(lines[last]) {
        guard = Some(text_line(last));
    }
    if !src.ends_with('\n') && last > 0 && looks_like_delimiter_row(lines[last]) {
        guard = Some(text_line(last - 1));
    }
    guard
}

/// `|` hors code en ligne (sur une seule ligne).
fn has_table_pipe(line: &str) -> bool {
    let mut rest = line;
    while let Some(at) = rest.find(['|', '`']) {
        if rest[at..].starts_with('|') {
            return true;
        }
        let n = rest[at..].bytes().take_while(|&c| c == b'`').count();
        match closing_backticks(&rest[at + n..], n) {
            Some(p) => rest = &rest[at + n + p + n..],
            None => rest = &rest[at + n..],
        }
    }
    false
}

/// Ligne faite seulement de `|`, `-`, `:` et d'espaces (après « > »).
fn looks_like_delimiter_row(line: &str) -> bool {
    let line = line.trim_start_matches([' ', '\t', '>']).trim_end();
    !line.is_empty()
        && line
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

// ---------------------------------------------------------------- feuilles

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Paragraphe ou texte direct d'un élément de liste.
    Paragraph,
    Heading,
    /// Ligne de tableau (en-tête compris).
    Row,
}

#[derive(Debug)]
struct Leaf {
    kind: Kind,
    /// Texte brut (avant nettoyage), crochets du texte en `OPEN`/`CLOSE`.
    text: String,
    /// Premier octet de `text` qu'un fermant à venir pourrait changer.
    unsafe_from: Option<usize>,
    /// Début de la feuille dans la source.
    start: usize,
    /// Octets de `text` où commence chaque ligne de la source après la
    /// première (retours à la ligne du paragraphe).
    line_starts: Vec<usize>,
}

impl Leaf {
    fn new(kind: Kind, start: usize) -> Self {
        Self {
            kind,
            text: String::new(),
            unsafe_from: None,
            start,
            line_starts: Vec::new(),
        }
    }
}

#[derive(Debug, Default)]
struct Doc {
    leaves: Vec<Leaf>,
    /// Débuts des blocs dans la source, dans l'ordre.
    block_starts: Vec<usize>,
    /// Débuts des blocs de premier niveau et des éléments d'une liste de
    /// premier niveau (points de reprise possibles).
    top_starts: Vec<usize>,
    /// Débuts des tableaux (ils dépendent aussi de la ligne suivante).
    table_starts: Vec<usize>,
    /// Débuts des lignes de corps de tableau (définitifs dès qu'ils existent :
    /// le tableau est établi).
    row_starts: Vec<usize>,
}

impl Doc {
    /// Vrai si `leaf` est terminée : un bloc commence après
    /// elle, et ce début est définitif. Il l'est si sa ligne est finie (sinon
    /// « --- » ou « ``` » peuvent redevenir du texte à la suite de la
    /// feuille : « ---|--- », « ```a`b ») ou si une ligne vide le sépare de
    /// la feuille.
    fn closes(&self, source: &str, leaf: &Leaf) -> bool {
        let after = self
            .block_starts
            .partition_point(|&start| start <= leaf.start);
        self.block_starts[after..].iter().any(|&start| {
            self.is_final(source, start) || has_blank_line(&source[leaf.start..start])
        })
    }

    /// Vrai si le bloc qui commence à `start` commencera toujours là : sa
    /// ligne est finie, et pour un tableau la ligne de délimiteurs aussi
    /// (« |a\n|- » est un tableau, « |a\n|-|- » redevient du texte).
    fn is_final(&self, source: &str, start: usize) -> bool {
        if self.row_starts.contains(&start) {
            return true;
        }
        let lines_needed = if self.table_starts.contains(&start) {
            2
        } else {
            1
        };
        source[start..]
            .match_indices('\n')
            .nth(lines_needed - 1)
            .is_some()
    }
}

/// Ligne vide (ou faite de « > ») entière dans `text` : ni la première ni la
/// dernière ligne, qui sont coupées.
fn has_blank_line(text: &str) -> bool {
    let lines: Vec<&str> = text.split('\n').collect();
    lines.len() > 2
        && lines[1..lines.len() - 1]
            .iter()
            .any(|l| l.trim_matches([' ', '\t', '\r', '>']).is_empty())
}

#[derive(Default)]
struct Builder {
    doc: Doc,
    current: Option<Leaf>,
    /// Profondeur des conteneurs muets (code, HTML, note, image).
    silent: usize,
    /// Dans un lien automatique : son texte (l'adresse) est remplacé.
    autolink: bool,
    /// Débuts (octets du texte) des emphases, barrés et liens ouverts.
    inline_starts: Vec<usize>,
    /// Blocs ouverts (pour reconnaître le premier niveau).
    blocks: Vec<BlockKind>,
    /// Cellules de la ligne de tableau en cours, et la cellule ouverte.
    cells: Vec<String>,
    cell: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    List,
    Other,
}

impl Builder {
    fn open(&mut self, kind: Kind, start: usize) {
        self.close();
        self.current = Some(Leaf::new(kind, start));
    }

    fn close(&mut self) {
        self.inline_starts.clear();
        if let Some(mut leaf) = self.current.take() {
            if leaf.kind == Kind::Row {
                leaf.text = self
                    .cells
                    .drain(..)
                    .filter(|c| !c.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.cell = None;
            }
            if !leaf.text.trim().is_empty() {
                self.doc.leaves.push(leaf);
            }
        }
    }

    /// Début d'un bloc ; `item` : élément de liste.
    fn block_start(&mut self, start: usize, item: bool) {
        self.close();
        self.doc.block_starts.push(start);
        let top = self.blocks.is_empty() || (item && self.blocks == [BlockKind::List]);
        if top {
            self.doc.top_starts.push(start);
        }
    }

    fn leaf(&mut self, start: usize) -> &mut Leaf {
        self.current
            .get_or_insert_with(|| Leaf::new(Kind::Paragraph, start))
    }

    /// Texte d'un événement `Text` (`literal`) ou non (code en ligne, bornes).
    fn push(&mut self, text: &str, literal: bool, start: usize) {
        if self.silent > 0 || self.autolink {
            return;
        }
        if let Some(cell) = self.cell.as_mut() {
            cell.push_str(&brackets(text, literal));
            return;
        }
        let outer = self.inline_starts.first().copied();
        let leaf = self.leaf(start);
        if literal && leaf.unsafe_from.is_none() {
            leaf.unsafe_from = first_unsafe(text).map(|at| {
                let at = leaf.text.len() + at;
                // Dans une emphase ou un lien : tout son intérieur.
                outer.map_or(at, |outer| outer.min(at))
            });
        }
        leaf.text.push_str(&brackets(text, literal));
    }

    fn line_break(&mut self, start: usize) {
        if self.silent > 0 || self.autolink {
            return;
        }
        if let Some(cell) = self.cell.as_mut() {
            cell.push(' ');
            return;
        }
        let leaf = self.leaf(start);
        leaf.text.push(' ');
        leaf.line_starts.push(leaf.text.len());
    }

    fn inline_open(&mut self, start: usize) {
        if self.silent == 0 && self.cell.is_none() {
            let at = self.leaf(start).text.len();
            self.inline_starts.push(at);
        }
    }

    fn inline_close(&mut self) {
        if self.silent == 0 && self.cell.is_none() {
            self.inline_starts.pop();
        }
    }
}

/// Crochets du texte remplacés par `OPEN`/`CLOSE` (le code garde les siens).
fn brackets(text: &str, literal: bool) -> String {
    if literal {
        text.replace('[', &OPEN.to_string())
            .replace(']', &CLOSE.to_string())
    } else {
        text.to_owned()
    }
}

/// Premier caractère littéral qu'un fermant à venir pourrait retirer ou
/// faire changer de sens (octet dans `text`).
fn first_unsafe(text: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    chars.iter().enumerate().find_map(|(i, &(at, c))| {
        let before = i.checked_sub(1).map(|j| chars[j].1);
        let after = chars.get(i + 1).map(|&(_, c)| c);
        let unsafe_char = match c {
            '`' | '~' | '<' | '[' => true,
            // « 2 * 3 » : entouré d'espaces, jamais un délimiteur.
            '*' => {
                !(before.is_some_and(char::is_whitespace) && after.is_some_and(char::is_whitespace))
            }
            // « nom_de_fichier » : entre deux lettres, jamais un délimiteur.
            '_' => {
                !(before.is_some_and(char::is_alphanumeric)
                    && after.is_some_and(char::is_alphanumeric))
            }
            _ => false,
        };
        unsafe_char.then_some(at)
    })
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM
}

/// Lien par référence sans définition : résolu quand même (voir l'en-tête).
fn any_reference<'a>(_: BrokenLink<'a>) -> Option<(CowStr<'a>, CowStr<'a>)> {
    Some((CowStr::Borrowed(""), CowStr::Borrowed("")))
}

fn parse(source: &str) -> Doc {
    let mut b = Builder::default();
    let parser = Parser::new_with_broken_link_callback(source, options(), Some(any_reference));
    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph
                | Tag::Heading { .. }
                | Tag::Item
                | Tag::TableHead
                | Tag::TableRow => {
                    let item = matches!(tag, Tag::Item);
                    if matches!(tag, Tag::TableRow) {
                        b.doc.row_starts.push(range.start);
                    }
                    b.block_start(range.start, item);
                    b.blocks.push(BlockKind::Other);
                    let kind = match tag {
                        Tag::Heading { .. } => Kind::Heading,
                        Tag::TableHead | Tag::TableRow => Kind::Row,
                        _ => Kind::Paragraph,
                    };
                    if b.silent == 0 {
                        b.open(kind, range.start);
                    }
                }
                Tag::TableCell => {
                    if b.silent == 0 {
                        b.cell = Some(String::new());
                    }
                }
                Tag::CodeBlock(_)
                | Tag::HtmlBlock
                | Tag::FootnoteDefinition(_)
                | Tag::MetadataBlock(_) => {
                    b.block_start(range.start, false);
                    b.blocks.push(BlockKind::Other);
                    b.silent += 1;
                }
                Tag::List(_) => {
                    b.block_start(range.start, false);
                    b.blocks.push(BlockKind::List);
                }
                Tag::Table(_) => {
                    b.doc.table_starts.push(range.start);
                    b.block_start(range.start, false);
                    b.blocks.push(BlockKind::Other);
                }
                Tag::BlockQuote(_)
                | Tag::DefinitionList
                | Tag::DefinitionListTitle
                | Tag::DefinitionListDefinition => {
                    b.block_start(range.start, false);
                    b.blocks.push(BlockKind::Other);
                }
                Tag::Image { .. } => b.silent += 1,
                Tag::Link {
                    link_type: LinkType::Autolink | LinkType::Email,
                    dest_url,
                    ..
                } => {
                    let shown = if dest_url.contains("://") {
                        host(&dest_url)
                    } else {
                        dest_url.to_string()
                    };
                    b.push(&shown, false, range.start);
                    b.autolink = true;
                }
                Tag::Link { .. } => {
                    b.push(&OPEN.to_string(), false, range.start);
                    b.inline_open(range.start);
                }
                Tag::Emphasis | Tag::Strong | Tag::Strikethrough => b.inline_open(range.start),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::TableHead
                | TagEnd::TableRow => {
                    if b.silent == 0 {
                        b.close();
                    }
                    b.blocks.pop();
                }
                TagEnd::TableCell => {
                    if let Some(cell) = b.cell.take() {
                        b.cells.push(cell.trim().to_owned());
                    }
                }
                TagEnd::CodeBlock
                | TagEnd::HtmlBlock
                | TagEnd::FootnoteDefinition
                | TagEnd::MetadataBlock(_) => {
                    b.silent = b.silent.saturating_sub(1);
                    b.blocks.pop();
                }
                TagEnd::List(_)
                | TagEnd::BlockQuote(_)
                | TagEnd::Table
                | TagEnd::DefinitionList
                | TagEnd::DefinitionListTitle
                | TagEnd::DefinitionListDefinition => {
                    b.blocks.pop();
                }
                TagEnd::Image => b.silent = b.silent.saturating_sub(1),
                TagEnd::Link => {
                    if b.autolink {
                        b.autolink = false;
                    } else {
                        b.inline_close();
                        b.push(&CLOSE.to_string(), false, range.start);
                    }
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => b.inline_close(),
                _ => {}
            },
            Event::Text(text) => b.push(&text, true, range.start),
            Event::Code(code) => b.push(&code, false, range.start),
            Event::SoftBreak | Event::HardBreak => b.line_break(range.start),
            Event::InlineHtml(_) => b.push(" ", false, range.start),
            Event::Rule => {
                b.block_start(range.start, false);
            }
            // Même texte brut qu'un appel sans définition (« [^1] » littéral) :
            // la définition peut être sortie de la fenêtre du segmenteur.
            Event::FootnoteReference(label) => {
                b.push(&format!("{OPEN}^{label}{CLOSE}"), false, range.start)
            }
            Event::Html(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::TaskListMarker(_) => {}
        }
    }
    b.close();
    b.doc.block_starts.sort_unstable();
    b.doc
}

/// Nom d'hôte d'une adresse : « https://www.ollama.com/library » → « ollama.com ».
fn host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    rest.split(['/', '?', '#', ':'])
        .next()
        .unwrap_or(rest)
        .to_owned()
}

// ---------------------------------------------------------------- phrases

/// Abréviations après lesquelles un point ne finit pas la phrase.
const ABBREVIATIONS: &[&str] = &[
    "M.", "MM.", "Mme.", "Mlle.", "Dr.", "Dra.", "Pr.", "Mr.", "Mrs.", "Ms.", "St.", "Sr.", "Sra.",
    "Srta.", "Jr.", "p.", "pp.", "ex.", "cf.", "vs.", "env.", "e.g.", "i.e.", "approx.", "aprox.",
    "No.", "no.", "nº.", "núm.", "pág.", "fig.", "Fig.", "art.", "vol.", "chap.", "Inc.", "Ltd.",
    "Co.", "Ud.", "Uds.", "ej.", "c.-à-d.",
];

const CLOSERS: &[char] = &[')', '"', '\'', '»', '”', '’', CLOSE];

/// Fins de phrase de `text` : octet juste après la ponctuation finale (et
/// les fermants qui la suivent), suivie d'espaces puis d'un caractère qui
/// n'est ni une minuscule, ni `.`, `,`, `;`, `:`, ni un crochet.
fn sentence_ends(text: &str) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut iter = text.char_indices().peekable();
    while let Some((at, c)) = iter.next() {
        if !matches!(c, '.' | '!' | '?' | '…') {
            continue;
        }
        let mut end = at + c.len_utf8();
        let mut single_dot = c == '.';
        while let Some(&(i, next)) = iter.peek() {
            if matches!(next, '.' | '!' | '?' | '…') {
                single_dot = false;
            } else if !CLOSERS.contains(&next) {
                break;
            }
            end = i + next.len_utf8();
            iter.next();
        }
        let rest = &text[end..];
        let after = rest.trim_start();
        if after.len() == rest.len() {
            continue;
        }
        let Some(first) = after.chars().next() else {
            continue;
        };
        // Crochet ou lien en tête de la suite : un appel retiré au nettoyage
        // collerait sa ponctuation à la phrase d'avant (« a. [1]. B »).
        if first.is_lowercase() || matches!(first, '.' | ',' | ';' | ':' | OPEN | CLOSE) {
            continue;
        }
        if single_dot && is_abbreviation(&text[..end]) {
            continue;
        }
        ends.push(end);
    }
    ends
}

/// Vrai si `before` (texte jusqu'au point compris) finit par une abréviation
/// connue ou une initiale (« J. »).
fn is_abbreviation(before: &str) -> bool {
    let word = before
        .rsplit(|c: char| c.is_whitespace() || c == '(' || c == OPEN)
        .next()
        .unwrap_or(before)
        .trim_start_matches(['"', '\'', '«', '“']);
    let mut letters = word.trim_end_matches(CLOSERS).chars();
    let initial = matches!((letters.next(), letters.next(), letters.next()), (Some(l), Some('.'), None) if l.is_uppercase());
    initial || ABBREVIATIONS.contains(&word.trim_end_matches(CLOSERS))
}

/// Fin de la dernière phrase terminée et sûre de `text`.
fn last_safe_end(text: &str, unsafe_from: Option<usize>) -> Option<usize> {
    let limit = unsafe_from.unwrap_or(usize::MAX);
    sentence_ends(text)
        .into_iter()
        .rev()
        .find(|&end| end <= limit)
}

/// Texte brut nettoyé : appels de note et de citation et crochets retirés,
/// adresses réduites à leur hôte, espaces resserrées. Purement local (même
/// résultat sur un texte entier et sur ses morceaux coupés à une espace),
/// pour la garantie du segmenteur ; l'espace laissée avant un point par une
/// formule retirée est resserrée par le prétraitement.
fn clean(raw: &str) -> String {
    let mut text = String::with_capacity(raw.len());
    let mut rest = raw;
    // Appels de note (« [^1] ») et de citation (« [1] », « [1, 2] », « [3-5] »).
    while let Some(at) = rest.find(OPEN) {
        text.push_str(&rest[..at]);
        let after = &rest[at + OPEN.len_utf8()..];
        let note = after.strip_prefix('^').and_then(|label| {
            let end = label.find(|c: char| c == CLOSE || c.is_whitespace())?;
            label[end..]
                .starts_with(CLOSE)
                .then(|| end + 1 + CLOSE.len_utf8())
        });
        let citation = || {
            let end = after.find(CLOSE)?;
            let label = &after[..end];
            (label.starts_with(|c: char| c.is_ascii_digit())
                && label
                    .chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, ',' | '-' | '–' | ' ')))
            .then(|| end + CLOSE.len_utf8())
        };
        let note = note.or_else(citation);
        rest = match note {
            Some(len) => {
                // « résultats [1], la » → « résultats, la » : l'espace d'avant
                // part avec l'appel (jamais en tête d'un morceau : une fin de
                // phrase n'est pas acceptée devant un crochet).
                if after[len..].starts_with(['.', ',']) {
                    text.truncate(text.trim_end().len());
                }
                &after[len..]
            }
            None => after,
        };
    }
    text.push_str(rest);
    let text: String = text.chars().filter(|&c| c != CLOSE).collect();
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&shorten_url(word));
    }
    out
}

/// Adresse web réduite à son hôte, ponctuation finale gardée.
fn shorten_url(word: &str) -> String {
    let start = word
        .find("https://")
        .or_else(|| word.find("http://"))
        .or_else(|| word.starts_with("www.").then_some(0));
    let Some(start) = start else {
        return word.to_owned();
    };
    let url_and_tail = &word[start..];
    let url = url_and_tail.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', '\'']);
    let tail = &url_and_tail[url.len()..];
    format!("{}{}{tail}", &word[..start], host(url))
}

/// Ponctuation finale assurée : « . » ajouté, « , » ou « ; » remplacés.
fn ensure_terminal(sentence: &mut String) {
    let body = sentence.trim_end_matches(CLOSERS);
    match body.chars().last() {
        None => {}
        Some('.' | '!' | '?' | '…' | ':') => {}
        Some(',' | ';') => {
            let at = body.len() - 1;
            sentence.replace_range(at..at + 1, ".");
        }
        Some(_) => sentence.push('.'),
    }
}

/// Phrases parlées d'un morceau de feuille ; `complete` : fin de la feuille
/// (ponctuation finale assurée).
fn sentences(raw: &str, complete: bool) -> Vec<String> {
    let text = clean(raw);
    let mut out = Vec::new();
    let mut from = 0;
    for end in sentence_ends(&text) {
        out.push(text[from..end].trim().to_owned());
        from = end;
    }
    let tail = text[from..].trim();
    if !tail.is_empty() {
        out.push(tail.to_owned());
    }
    if complete && let Some(last) = out.last_mut() {
        ensure_terminal(last);
    }
    out.retain(|s| s.chars().any(char::is_alphanumeric));
    out
}

// ---------------------------------------------------------------- formules

/// Source masquée et ses points de reprise possibles.
struct Masked {
    text: String,
    /// Octets de `text` où commence une ligne non vide qui suit une ligne
    /// vide, hors bloc de code et bloc de formule : le masque d'une source
    /// qui commencerait là est la suite de celui-ci.
    restarts: Vec<usize>,
}

/// Source sans ses formules : `$$…$$` et `\[…\]` sur leurs propres lignes
/// (bloc, lignes vides permises, ouvert jusqu'à la fin s'il n'est pas
/// fermé), `$$…$$`, `\(…\)`, `\[…\]` dans le texte (jusqu'à la fin du
/// paragraphe s'ils ne sont pas fermés). Rien n'est touché dans les blocs de
/// code ni dans le code en ligne (fermé sur sa ligne). Une formule en ligne
/// devient une espace,
/// un bloc des lignes vides ; tous les « \n » sont gardés.
fn mask_math(src: &str) -> Masked {
    let mut out = String::with_capacity(src.len());
    let mut restarts = Vec::new();
    let mut para = String::new();
    let mut fence: Option<(char, usize)> = None;
    let mut block_close: Option<&str> = None;
    let mut after_blank = false;
    for line in src.split_inclusive('\n') {
        let content = without_containers(line);
        let trimmed = content.trim();
        if let Some((ch, len)) = fence {
            out.push_str(line);
            if trimmed.len() >= len && trimmed.chars().all(|c| c == ch) {
                fence = None;
            }
            after_blank = false;
            continue;
        }
        if let Some(close) = block_close {
            if trimmed.contains(close) {
                block_close = None;
            }
            out.extend(line.chars().filter(|&c| c == '\n'));
            after_blank = false;
            continue;
        }
        if trimmed.is_empty() {
            mask_inline(&para, &mut out);
            para.clear();
            out.push_str(line);
            after_blank = true;
            continue;
        }
        if after_blank {
            restarts.push(out.len());
            after_blank = false;
        }
        if let Some(open) = fence_open(trimmed) {
            mask_inline(&para, &mut out);
            para.clear();
            fence = Some(open);
            out.push_str(line);
            continue;
        }
        if let Some(close) = block_math_open(trimmed) {
            mask_inline(&para, &mut out);
            para.clear();
            block_close = Some(close);
            out.extend(line.chars().filter(|&c| c == '\n'));
            continue;
        }
        para.push_str(line);
    }
    mask_inline(&para, &mut out);
    Masked {
        text: out,
        restarts,
    }
}

/// Ligne sans ses marqueurs de citation et de liste.
fn without_containers(line: &str) -> &str {
    let mut rest = line.trim_start();
    loop {
        if let Some(r) = rest.strip_prefix('>') {
            rest = r.trim_start();
            continue;
        }
        let marker = rest
            .strip_prefix(['-', '*', '+'])
            .or_else(|| {
                let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                (1..=9)
                    .contains(&digits)
                    .then(|| rest[digits..].strip_prefix(['.', ')']))
                    .flatten()
            })
            .filter(|r| r.starts_with([' ', '\t']));
        match marker {
            Some(r) => rest = r.trim_start(),
            None => return rest,
        }
    }
}

/// Clôture de code ouvrante : (caractère, longueur).
fn fence_open(trimmed: &str) -> Option<(char, usize)> {
    let ch = trimmed.chars().next().filter(|&c| c == '`' || c == '~')?;
    let len = trimmed.chars().take_while(|&c| c == ch).count();
    (len >= 3 && !(ch == '`' && trimmed[len..].contains('`'))).then_some((ch, len))
}

/// Ouverture d'un bloc de formule : rend son fermant.
fn block_math_open(trimmed: &str) -> Option<&'static str> {
    if let Some(rest) = trimmed.strip_prefix("$$") {
        return (!rest.contains('$')).then_some("$$");
    }
    if let Some(rest) = trimmed.strip_prefix("\\[") {
        return (!rest.contains("\\]")).then_some("\\]");
    }
    None
}

/// Formules en ligne d'un paragraphe retirées (voir `mask_math`).
fn mask_inline(text: &str, out: &mut String) {
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let math_close = if rest.starts_with("$$") {
            Some("$$")
        } else if rest.starts_with("\\(") {
            Some("\\)")
        } else if rest.starts_with("\\[") {
            Some("\\]")
        } else {
            None
        };
        if let Some(close) = math_close {
            let end = rest[2..]
                .find(close)
                .map_or(text.len(), |p| i + 2 + p + close.len());
            out.push(' ');
            out.extend(text[i..end].chars().filter(|&c| c == '\n'));
            i = end;
            continue;
        }
        if let Some(escaped) = rest.strip_prefix('\\') {
            // Échappement (« \$ », « \` ») : copié tel quel.
            let len = 1 + escaped.chars().next().map_or(0, char::len_utf8);
            out.push_str(&rest[..len]);
            i += len;
            continue;
        }
        if rest.starts_with('`') {
            // Code en ligne fermé sur la même ligne seulement : un fermant qui
            // arriverait plus loin ne doit pas démasquer après coup une formule
            // déjà retirée (lignes intermédiaires déjà lues).
            let n = rest.bytes().take_while(|&c| c == b'`').count();
            let line = rest[n..].split('\n').next().unwrap_or("");
            let end = closing_backticks(line, n).map_or(i + n, |p| i + n + p + n);
            out.push_str(&text[i..end]);
            i = end;
            continue;
        }
        let c = rest.chars().next().expect("caractère");
        out.push(c);
        i += c.len_utf8();
    }
}

/// Position d'une suite d'exactement `n` accents graves.
fn closing_backticks(text: &str, n: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let run = bytes[i..].iter().take_while(|&&c| c == b'`').count();
            if run == n {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
