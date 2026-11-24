//! Texte d'un document Word `.docx` (propre à ChatonLM : l'officiel
//! l'accepte mais n'en extrait rien). Un `.docx` est une archive ZIP ; son
//! texte est dans `word/document.xml`, les noms des styles dans
//! `word/styles.xml` (pour reconnaître les titres, dont l'identifiant est
//! traduit dans la langue de Word : « Titre1 », « Ttulo1 »…).
//!
//! Rendu en Markdown simple, pour le modèle : un paragraphe par bloc,
//! titres en `#`, éléments de liste en `-` (retrait selon le niveau),
//! tableaux une ligne par rangée (cellules séparées par ` | `). Texte
//! supprimé ou déplacé ailleurs (origine d'un déplacement, `w:moveFrom`) des
//! modifications suivies, anciennes mises en forme (`w:pPrChange`…), codes
//! de champs et contenu de repli (`mc:Fallback`, doublon d'une zone de
//! texte) écartés. Notes de bas de page et de fin (`word/footnotes.xml`,
//! `word/endnotes.xml`) ajoutées à la fin, sous un titre « Notes ».
//!
//! Garde contre les bombes : entrées de l'archive comptées
//! ([`MAX_ENTRIES`]), taille décompressée bornée ([`MAX_UNPACKED`]) d'après
//! l'en-tête puis pendant la lecture (un en-tête peut mentir). Lecture XML
//! sans dépendance : pas d'entité déclarée développée, pas de récursion.
//! Code commun à Linux et Windows ; bloquant (à appeler hors du fil async).

use std::collections::HashMap;
use std::io::{Cursor, Read};

/// Taille décompressée lue au plus (document, styles et notes ensemble).
pub const MAX_UNPACKED: u64 = 20 * 1024 * 1024;
/// Entrées de l'archive au plus (un document ordinaire en a quelques
/// dizaines, images comprises).
pub const MAX_ENTRIES: usize = 10_000;

const DOCUMENT: &str = "word/document.xml";
const STYLES: &str = "word/styles.xml";
/// Notes de bas de page, puis de fin.
const NOTES: [&str; 2] = ["word/footnotes.xml", "word/endnotes.xml"];
// i18n-ignore : texte au modèle (le document est rendu pour lui), pas à
// l'interface.
/// Titre des notes ajoutées à la fin.
const NOTES_TITLE: &str = "# Notes";

/// Pourquoi un `.docx` n'est pas lu.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum DocxError {
    #[error("not a readable Word document")]
    Unreadable,
    #[error("the document unpacks to more than 20 MiB")]
    TooLarge,
    #[error("the archive has too many entries")]
    TooManyEntries,
}

/// Texte du document `data` (contenu d'un `.docx`).
pub fn docx_text(data: &[u8]) -> Result<String, DocxError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(data)).map_err(|_| DocxError::Unreadable)?;
    if archive.len() > MAX_ENTRIES {
        return Err(DocxError::TooManyEntries);
    }
    let mut budget = MAX_UNPACKED;
    let document = read_entry(&mut archive, DOCUMENT, &mut budget)?.ok_or(DocxError::Unreadable)?;
    // Styles absents ou illisibles : titres reconnus par leur seul
    // identifiant (« Heading1 »).
    let styles = match read_entry(&mut archive, STYLES, &mut budget) {
        Ok(Some(styles)) => heading_styles(&styles),
        Ok(None) | Err(DocxError::Unreadable) => HashMap::new(),
        Err(error) => return Err(error),
    };
    let mut text = render(&document, &styles);
    // Notes absentes ou illisibles : le corps seul.
    let mut notes = Vec::new();
    for name in NOTES {
        match read_entry(&mut archive, name, &mut budget) {
            Ok(Some(xml)) => notes.push(render(&xml, &styles)),
            Ok(None) | Err(DocxError::Unreadable) => {}
            Err(error) => return Err(error),
        }
    }
    let notes = notes
        .into_iter()
        .filter(|notes| !notes.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !notes.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(NOTES_TITLE);
        text.push_str("\n\n");
        text.push_str(&notes);
    }
    Ok(text)
}

/// Entrée `name` en texte (`None` : absente), décomptée de `budget`.
fn read_entry(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
    budget: &mut u64,
) -> Result<Option<String>, DocxError> {
    let entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(_) => return Err(DocxError::Unreadable),
    };
    if entry.size() > *budget {
        return Err(DocxError::TooLarge);
    }
    let mut bytes = Vec::new();
    // Défense en profondeur : le crate zip borne déjà la lecture à la
    // taille annoncée (puis vérifie le CRC) ; la limite tient même s'il
    // cessait de le faire.
    entry
        .take(*budget + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| DocxError::Unreadable)?;
    let read = bytes.len() as u64;
    if read > *budget {
        return Err(DocxError::TooLarge);
    }
    *budget -= read;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| DocxError::Unreadable)
}

// --- Lecture XML -----------------------------------------------------------

/// Événement de la lecture XML.
#[derive(Debug, PartialEq, Eq)]
enum Event<'a> {
    /// Balise ouvrante (`empty` : `<x/>`), avec ses attributs bruts.
    Start {
        name: &'a str,
        attrs: &'a str,
        empty: bool,
    },
    End(&'a str),
    Text(String),
}

/// Lecteur XML minimal, sans validation : balises, attributs, texte avec
/// les entités prédéfinies et numériques. Commentaires, instructions,
/// déclarations sautés ; CDATA rendu en texte.
struct Reader<'a> {
    source: &'a str,
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(source: &'a str) -> Self {
        Self { source, at: 0 }
    }

    fn next_event(&mut self) -> Option<Event<'a>> {
        loop {
            let rest = &self.source[self.at..];
            if rest.is_empty() {
                return None;
            }
            if !rest.starts_with('<') {
                let end = rest.find('<').unwrap_or(rest.len());
                self.at += end;
                return Some(Event::Text(decode(&rest[..end])));
            }
            if let Some(body) = rest.strip_prefix("<![CDATA[") {
                let end = body.find("]]>").unwrap_or(body.len());
                self.at += 9 + (end + 3).min(body.len());
                return Some(Event::Text(body[..end].to_owned()));
            }
            let skip = if let Some(body) = rest.strip_prefix("<!--") {
                Some(body.find("-->").map_or(rest.len(), |end| 4 + end + 3))
            } else if let Some(body) = rest.strip_prefix("<?") {
                Some(body.find("?>").map_or(rest.len(), |end| 2 + end + 2))
            } else if rest.starts_with("<!") {
                // Déclaration (DOCTYPE et ses entités, jamais développées).
                Some(declaration_end(rest).map_or(rest.len(), |end| end + 1))
            } else {
                None
            };
            if let Some(skip) = skip {
                self.at += skip;
                continue;
            }
            let Some(end) = tag_end(rest) else {
                self.at = self.source.len();
                return None;
            };
            self.at += end + 1;
            let tag = &rest[1..end];
            if let Some(name) = tag.strip_prefix('/') {
                return Some(Event::End(name.trim()));
            }
            let (tag, empty) = match tag.strip_suffix('/') {
                Some(tag) => (tag, true),
                None => (tag, false),
            };
            let split = tag
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(tag.len());
            return Some(Event::Start {
                name: &tag[..split],
                attrs: &tag[split..],
                empty,
            });
        }
    }
}

/// Fin d'une déclaration `<!…>` : `>` hors des guillemets et des crochets
/// (sous-ensemble interne d'un DOCTYPE).
fn declaration_end(rest: &str) -> Option<usize> {
    let mut quote = None;
    let mut depth = 0usize;
    for (index, c) in rest.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '[') => depth += 1,
            (None, ']') => depth = depth.saturating_sub(1),
            (None, '>') if depth == 0 => return Some(index),
            _ => {}
        }
    }
    None
}

/// Fin d'une balise (`>` hors des valeurs d'attributs) dans `rest`, qui
/// commence par `<`.
fn tag_end(rest: &str) -> Option<usize> {
    let mut quote = None;
    for (index, c) in rest.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '>') => return Some(index),
            _ => {}
        }
    }
    None
}

/// Valeur de l'attribut `name` (préfixe compris : `w:val`).
fn attr(attrs: &str, name: &str) -> Option<String> {
    let mut rest = attrs;
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let end = after[1..].find(quote)?;
        if key == name {
            return Some(decode(&after[1..=end]));
        }
        rest = &after[end + 2..];
    }
    None
}

/// Texte XML décodé : entités prédéfinies et numériques ; une autre entité
/// reste telle quelle (aucune n'est développée).
fn decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let entity = rest[1..]
            .find(';')
            .filter(|end| *end <= 10)
            .map(|end| &rest[1..=end]);
        let decoded = entity.and_then(|entity| match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        });
        match (entity, decoded) {
            (Some(entity), Some(c)) => {
                out.push(c);
                rest = &rest[entity.len() + 2..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// --- Styles ----------------------------------------------------------------

/// Niveau de titre (1 à 9) des styles de paragraphe qui en sont : nom
/// interne « heading N » ou « Title » (toujours en anglais), ou niveau de
/// plan déclaré.
fn heading_styles(styles: &str) -> HashMap<String, usize> {
    let mut levels = HashMap::new();
    let mut reader = Reader::new(styles);
    let mut current: Option<String> = None;
    while let Some(event) = reader.next_event() {
        match event {
            Event::Start {
                name: "w:style",
                attrs,
                empty,
            } => {
                current = if empty {
                    None
                } else {
                    attr(attrs, "w:styleId")
                };
            }
            Event::End("w:style") => current = None,
            Event::Start {
                name: "w:name",
                attrs,
                ..
            } => {
                if let (Some(id), Some(name)) = (&current, attr(attrs, "w:val"))
                    && let Some(level) = heading_name_level(&name)
                {
                    levels.insert(id.clone(), level);
                }
            }
            Event::Start {
                name: "w:outlineLvl",
                attrs,
                ..
            } => {
                if let (Some(id), Some(level)) = (&current, outline_level(attrs)) {
                    levels.entry(id.clone()).or_insert(level);
                }
            }
            _ => {}
        }
    }
    levels
}

/// « heading 2 » → 2 ; « Title » → 1.
fn heading_name_level(name: &str) -> Option<usize> {
    let name = name.trim().to_ascii_lowercase();
    if name == "title" {
        return Some(1);
    }
    let level: usize = name.strip_prefix("heading")?.trim().parse().ok()?;
    (1..=9).contains(&level).then_some(level)
}

/// `w:outlineLvl w:val="0"` → titre de niveau 1 (9 : texte ordinaire).
fn outline_level(attrs: &str) -> Option<usize> {
    let level: usize = attr(attrs, "w:val")?.parse().ok()?;
    (level < 9).then_some(level + 1)
}

/// Niveau de titre d'un identifiant de style sans table des styles.
fn heading_id_level(id: &str) -> Option<usize> {
    heading_name_level(id).or_else(|| {
        let digits = id.strip_prefix("Heading")?;
        let level: usize = digits.parse().ok()?;
        (1..=9).contains(&level).then_some(level)
    })
}

// --- Document --------------------------------------------------------------

/// Bloc du texte rendu.
enum Block {
    Paragraph(String),
    Item(String),
    Row(String),
}

/// Paragraphe en cours.
#[derive(Default)]
struct Paragraph {
    text: String,
    heading: Option<usize>,
    /// Élément de liste, et son niveau.
    item: Option<usize>,
}

/// Tableau en cours : rangée et cellule.
#[derive(Default)]
struct Table {
    row: Vec<String>,
    cell: Option<String>,
}

/// Rendu du document (voir l'en-tête du module).
fn render(document: &str, styles: &HashMap<String, usize>) -> String {
    let mut reader = Reader::new(document);
    let mut blocks: Vec<Block> = Vec::new();
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    let mut tables: Vec<Table> = Vec::new();
    // Dans `w:t` (texte visible) ; profondeur d'un contenu écarté.
    let mut in_text = false;
    let mut skipped = 0usize;
    while let Some(event) = reader.next_event() {
        if skipped > 0 {
            match event {
                Event::Start { name, empty, .. } if is_skipped(name) && !empty => skipped += 1,
                Event::End(name) if is_skipped(name) => skipped -= 1,
                _ => {}
            }
            continue;
        }
        match event {
            Event::Start { name, empty, .. } if is_skipped(name) => {
                if !empty {
                    skipped = 1;
                }
            }
            Event::Start {
                name: "w:p", empty, ..
            } => {
                paragraphs.push(Paragraph::default());
                if empty {
                    end_paragraph(&mut paragraphs, &mut tables, &mut blocks);
                }
            }
            Event::End("w:p") => end_paragraph(&mut paragraphs, &mut tables, &mut blocks),
            Event::Start {
                name: "w:pStyle",
                attrs,
                ..
            } => {
                if let (Some(paragraph), Some(id)) = (paragraphs.last_mut(), attr(attrs, "w:val")) {
                    paragraph.heading = styles.get(&id).copied().or_else(|| heading_id_level(&id));
                }
            }
            Event::Start {
                name: "w:outlineLvl",
                attrs,
                ..
            } => {
                if let Some(paragraph) = paragraphs.last_mut() {
                    paragraph.heading = outline_level(attrs).or(paragraph.heading);
                }
            }
            Event::Start {
                name: "w:numPr", ..
            } => {
                if let Some(paragraph) = paragraphs.last_mut() {
                    paragraph.item.get_or_insert(0);
                }
            }
            Event::Start {
                name: "w:ilvl",
                attrs,
                ..
            } => {
                if let Some(paragraph) = paragraphs.last_mut() {
                    let level = attr(attrs, "w:val").and_then(|value| value.parse().ok());
                    paragraph.item = Some(level.unwrap_or(0).min(8));
                }
            }
            Event::Start {
                name: "w:t", empty, ..
            } => in_text = !empty,
            Event::End("w:t") => in_text = false,
            Event::Text(text) if in_text => {
                if let Some(paragraph) = paragraphs.last_mut() {
                    paragraph.text.push_str(&text);
                }
            }
            Event::Start { name: "w:tab", .. } => push_text(&mut paragraphs, "\t"),
            // Trait d'union insécable : « - » ; conditionnel (`w:softHyphen`) :
            // rien.
            Event::Start {
                name: "w:noBreakHyphen",
                ..
            } => push_text(&mut paragraphs, "-"),
            Event::Start {
                name: "w:br" | "w:cr",
                ..
            } => push_text(&mut paragraphs, "\n"),
            Event::Start {
                name: "w:tbl",
                empty: false,
                ..
            } => tables.push(Table::default()),
            Event::End("w:tbl") => {
                tables.pop();
            }
            Event::Start {
                name: "w:tc",
                empty,
                ..
            } => {
                if let Some(table) = tables.last_mut() {
                    if empty {
                        table.row.push(String::new());
                    } else {
                        table.cell = Some(String::new());
                    }
                }
            }
            Event::End("w:tc") => {
                if let Some(table) = tables.last_mut() {
                    let cell = table.cell.take().unwrap_or_default();
                    table.row.push(cell);
                }
            }
            Event::End("w:tr") => end_row(&mut tables, &mut blocks),
            _ => {}
        }
    }
    join(&blocks)
}

/// Contenu écarté : texte supprimé, origine d'un déplacement (le texte est
/// lu à sa destination, `w:moveTo`), anciennes propriétés des modifications
/// suivies (`w:pPrChange`, `w:rPrChange`… : un ancien style de titre ou de
/// liste), codes de champs, contenu de repli.
fn is_skipped(name: &str) -> bool {
    matches!(
        name,
        "w:del" | "w:moveFrom" | "w:instrText" | "w:delText" | "mc:Fallback"
    ) || (name.starts_with("w:") && name.ends_with("PrChange"))
}

fn push_text(paragraphs: &mut [Paragraph], text: &str) {
    if let Some(paragraph) = paragraphs.last_mut() {
        paragraph.text.push_str(text);
    }
}

/// Fin d'un paragraphe : dans une cellule, ajouté à elle ; sinon un bloc.
fn end_paragraph(paragraphs: &mut Vec<Paragraph>, tables: &mut [Table], blocks: &mut Vec<Block>) {
    let Some(paragraph) = paragraphs.pop() else {
        return;
    };
    let text = paragraph.text.trim();
    if text.is_empty() {
        return;
    }
    if let Some(cell) = tables.last_mut().and_then(|table| table.cell.as_mut()) {
        if !cell.is_empty() {
            cell.push(' ');
        }
        cell.push_str(&text.replace('\n', " "));
        return;
    }
    blocks.push(match (paragraph.heading, paragraph.item) {
        (Some(level), _) => Block::Paragraph(format!("{} {text}", "#".repeat(level))),
        (None, Some(level)) => Block::Item(format!("{}- {text}", "  ".repeat(level))),
        (None, None) => Block::Paragraph(text.to_owned()),
    });
}

/// Fin d'une rangée : une ligne ; dans un tableau imbriqué, ajoutée à la
/// cellule du tableau parent.
fn end_row(tables: &mut [Table], blocks: &mut Vec<Block>) {
    let Some((table, parents)) = tables.split_last_mut() else {
        return;
    };
    let cells = std::mem::take(&mut table.row);
    if cells.iter().all(|cell| cell.trim().is_empty()) {
        return;
    }
    let line = cells
        .iter()
        .map(|cell| cell.trim())
        .collect::<Vec<_>>()
        .join(" | ")
        .trim_end()
        .to_owned();
    match parents.last_mut().and_then(|parent| parent.cell.as_mut()) {
        Some(cell) => {
            if !cell.is_empty() {
                cell.push_str("; ");
            }
            cell.push_str(&line);
        }
        None => blocks.push(Block::Row(line)),
    }
}

/// Blocs réunis : une ligne vide entre deux blocs, sauf entre deux éléments
/// de liste ou deux rangées qui se suivent.
fn join(blocks: &[Block]) -> String {
    let mut out = String::new();
    let mut previous: Option<&Block> = None;
    for block in blocks {
        let (text, tight) = match block {
            Block::Paragraph(text) => (text, false),
            Block::Item(text) => (text, matches!(previous, Some(Block::Item(_)))),
            Block::Row(text) => (text, matches!(previous, Some(Block::Row(_)))),
        };
        if previous.is_some() {
            out.push_str(if tight { "\n" } else { "\n\n" });
        }
        out.push_str(text);
        previous = Some(block);
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use super::*;

    const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#;

    /// Archive faite de ces entrées (comprimées).
    pub fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    /// `.docx` dont le corps est `body` (et des styles s'ils sont donnés).
    pub fn docx(body: &str, styles: Option<&str>) -> Vec<u8> {
        let document = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document {NS}><w:body>{body}<w:sectPr/></w:body></w:document>"#
        );
        let styles = styles
            .map(|styles| format!(r#"<?xml version="1.0"?><w:styles {NS}>{styles}</w:styles>"#));
        let mut entries: Vec<(&str, &[u8])> = vec![
            ("[Content_Types].xml", b"<Types/>"),
            (DOCUMENT, document.as_bytes()),
        ];
        if let Some(styles) = &styles {
            entries.push((STYLES, styles.as_bytes()));
        }
        zip_of(&entries)
    }

    /// `.docx` avec ces notes de bas de page et de fin (paragraphes).
    fn docx_with_notes(body: &str, footnotes: &[&str], endnotes: &[&str]) -> Vec<u8> {
        let document = format!(
            r#"<?xml version="1.0"?><w:document {NS}><w:body>{body}</w:body></w:document>"#
        );
        let notes = |root: &str, item: &str, texts: &[&str]| {
            let items: String = texts
                .iter()
                .enumerate()
                .map(|(n, text)| {
                    format!(
                        r#"<w:{item} w:id="{}"><w:p><w:r><w:{item}Ref/></w:r><w:r><w:t xml:space="preserve"> {text}</w:t></w:r></w:p></w:{item}>"#,
                        n + 1
                    )
                })
                .collect();
            format!(
                r#"<?xml version="1.0"?><w:{root} {NS}><w:{item} w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:{item}>{items}</w:{root}>"#
            )
        };
        let footnotes = notes("footnotes", "footnote", footnotes);
        let endnotes = notes("endnotes", "endnote", endnotes);
        zip_of(&[
            (DOCUMENT, document.as_bytes()),
            (NOTES[0], footnotes.as_bytes()),
            (NOTES[1], endnotes.as_bytes()),
        ])
    }

    fn para(text: &str) -> String {
        format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
    }

    #[test]
    fn paragraphs_headings_lists_and_tables_become_text() {
        // Styles de Word en français : identifiants traduits, noms internes
        // en anglais.
        let styles = r#"
<w:style w:type="paragraph" w:styleId="Titre1"><w:name w:val="heading 1"/></w:style>
<w:style w:type="paragraph" w:styleId="Titre2"><w:name w:val="heading 2"/></w:style>
<w:style w:type="paragraph" w:styleId="Plan"><w:name w:val="Plan"/><w:pPr><w:outlineLvl w:val="2"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="Normal"><w:name w:val="Normal"/></w:style>"#;
        let body = format!(
            r#"<w:p><w:pPr><w:pStyle w:val="Titre1"/></w:pPr><w:r><w:t>Le potager</w:t></w:r></w:p>
{intro}
<w:p><w:pPr><w:pStyle w:val="Normal"/></w:pPr><w:r><w:t xml:space="preserve">Semer </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>en mars</w:t></w:r><w:r><w:tab/><w:t>&amp; arroser &lt;peu&gt; &#233;t&#xE9;</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="Titre2"/></w:pPr><w:r><w:t>Cultures</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Tomates</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Cœur de bœuf</w:t></w:r></w:p>
<w:p/>
<w:p><w:pPr><w:pStyle w:val="Plan"/></w:pPr><w:r><w:t>Calendrier</w:t></w:r></w:p>
<w:tbl><w:tr><w:tc>{a}</w:tc><w:tc>{b}</w:tc></w:tr><w:tr><w:tc>{c}{d}</w:tc><w:tc/></w:tr></w:tbl>
{end}"#,
            intro = para("Un guide."),
            a = para("Mois"),
            b = para("Semis"),
            c = para("Mars"),
            d = para("Avril"),
            end = para("Fin."),
        );
        let text = docx_text(&docx(&body, Some(styles))).unwrap();
        assert_eq!(
            text,
            "# Le potager\n\nUn guide.\n\nSemer en mars\t& arroser <peu> été\n\n## Cultures\n\n\
             - Tomates\n  - Cœur de bœuf\n\n### Calendrier\n\nMois | Semis\nMars Avril |\n\nFin."
        );
    }

    #[test]
    fn without_styles_english_heading_ids_are_recognized() {
        let body =
            r#"<w:p><w:pPr><w:pStyle w:val="Heading3"/></w:pPr><w:r><w:t>Notes</w:t></w:r></w:p>"#;
        assert_eq!(docx_text(&docx(body, None)).unwrap(), "### Notes");
    }

    #[test]
    fn deleted_text_fields_and_fallbacks_are_left_out() {
        let body = r#"<w:p>
<w:r><w:t>Gardé</w:t></w:r>
<w:del w:id="1"><w:r><w:delText>supprimé</w:delText></w:r></w:del>
<w:r><w:instrText> PAGE </w:instrText></w:r>
<w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:txbxContent><w:p><w:r><w:t>Encadré</w:t></w:r></w:p></w:txbxContent></mc:Choice><mc:Fallback><w:p><w:r><w:t>Encadré</w:t></w:r></w:p></mc:Fallback></mc:AlternateContent></w:r>
<w:r><w:t><![CDATA[ & fin]]></w:t></w:r>
</w:p>"#;
        let text = docx_text(&docx(body, None)).unwrap();
        // L'encadré (paragraphe dans le paragraphe) sort une fois, avant.
        assert_eq!(text, "Encadré\n\nGardé & fin");
    }

    /// Texte déplacé (modifications suivies) : lu une fois, à sa
    /// destination ; anciennes propriétés (`w:pPrChange`, `w:rPrChange`)
    /// ignorées, même leur ancien style de titre ou de liste.
    #[test]
    fn moved_text_and_old_properties_are_read_once() {
        let body = r#"<w:p><w:moveFrom w:id="1"><w:r><w:t>Déplacé</w:t></w:r></w:moveFrom><w:r><w:t>Reste</w:t></w:r></w:p>
<w:p><w:moveTo w:id="2"><w:r><w:t>Déplacé</w:t></w:r></w:moveTo></w:p>
<w:p><w:pPr><w:pStyle w:val="Normal"/><w:pPrChange w:id="3"><w:pPr><w:pStyle w:val="Heading1"/><w:numPr><w:ilvl w:val="2"/></w:numPr></w:pPr></w:pPrChange></w:pPr><w:r><w:rPr><w:b/><w:rPrChange w:id="4"><w:rPr><w:t>ancien</w:t></w:rPr></w:rPrChange></w:rPr><w:t>Texte</w:t></w:r></w:p>"#;
        assert_eq!(
            docx_text(&docx(body, None)).unwrap(),
            "Reste

Déplacé

Texte"
        );
    }

    /// Trait d'union insécable : « - » ; conditionnel : rien.
    #[test]
    fn special_hyphens_are_rendered() {
        let body = r#"<w:p><w:r><w:t>Jean</w:t><w:noBreakHyphen/><w:t>Pierre</w:t><w:softHyphen/><w:t>s</w:t></w:r></w:p>"#;
        assert_eq!(docx_text(&docx(body, None)).unwrap(), "Jean-Pierres");
    }

    /// Notes de bas de page puis de fin, à la fin sous « # Notes »
    /// (séparateurs sans texte écartés) ; sans notes : rien d'ajouté.
    #[test]
    fn footnotes_and_endnotes_come_last() {
        let word = docx_with_notes(&para("Corps."), &["Source : INSEE."], &["Voir l'annexe."]);
        assert_eq!(
            docx_text(&word).unwrap(),
            "Corps.

# Notes

Source : INSEE.

Voir l'annexe."
        );
        let word = docx_with_notes(&para("Corps."), &[], &[]);
        assert_eq!(docx_text(&word).unwrap(), "Corps.");
    }

    /// Les notes entrent dans le même budget que le document : deux parties
    /// de 12 Mio chacune (sous la limite une à une) sont refusées ensemble.
    #[test]
    fn notes_share_the_unpacked_budget() {
        let half = vec![b' '; 12 * 1024 * 1024];
        let word = zip_of(&[(DOCUMENT, &half), (NOTES[0], &half)]);
        drop(half);
        assert_eq!(docx_text(&word), Err(DocxError::TooLarge));
    }

    #[test]
    fn xml_is_read_without_expanding_entities() {
        assert_eq!(
            decode("a &lt;b&gt; &unknown; &#x1F431; &#9999999999;"),
            "a <b> &unknown; 🐱 &#9999999999;"
        );
        assert_eq!(
            attr(r#" w:a="x>y" w:val='2'"#, "w:val").as_deref(),
            Some("2")
        );
        assert_eq!(attr(r#" w:a="1""#, "w:val"), None);
        let mut reader = Reader::new(r#"<!DOCTYPE x [<!ENTITY a "aaaa">]><a k="v>w"/>&a;"#);
        assert_eq!(
            reader.next_event(),
            Some(Event::Start {
                name: "a",
                attrs: r#" k="v>w""#,
                empty: true
            })
        );
        assert_eq!(reader.next_event(), Some(Event::Text("&a;".into())));
        assert_eq!(reader.next_event(), None);
        // Balise jamais fermée : fin de lecture, sans boucle.
        assert_eq!(Reader::new("<w:p").next_event(), None);
    }

    #[test]
    fn damaged_archives_are_unreadable() {
        assert_eq!(docx_text(b"not a zip"), Err(DocxError::Unreadable));
        assert_eq!(docx_text(b""), Err(DocxError::Unreadable));
        // Archive sans document Word (un .xlsx renommé, par exemple).
        let other = zip_of(&[("xl/workbook.xml", b"<workbook/>")]);
        assert_eq!(docx_text(&other), Err(DocxError::Unreadable));
        // Archive tronquée.
        let whole = docx(&para("Bonjour"), None);
        assert_eq!(
            docx_text(&whole[..whole.len() / 2]),
            Err(DocxError::Unreadable)
        );
        // Document qui n'est pas de l'UTF-8.
        let latin1 = zip_of(&[(DOCUMENT, b"<w:t>caf\xe9</w:t>")]);
        assert_eq!(docx_text(&latin1), Err(DocxError::Unreadable));
    }

    /// Taille décompressée des bombes : au-delà de la limite.
    const BOMB: usize = 24 * 1024 * 1024;

    /// Bombe : document qui se décompresse en 24 Mio (un en-tête honnête),
    /// refusé d'après l'en-tête.
    #[test]
    fn a_zip_bomb_is_refused() {
        let huge = vec![b' '; BOMB];
        let bomb = zip_of(&[(DOCUMENT, &huge)]);
        drop(huge);
        assert!(bomb.len() < 1024 * 1024, "{}", bomb.len());
        assert_eq!(docx_text(&bomb), Err(DocxError::TooLarge));
    }

    /// Bombe dont l'en-tête ment (taille décompressée annoncée de 100
    /// octets) : la lecture s'arrête à la taille annoncée.
    #[test]
    fn a_zip_bomb_with_a_lying_header_is_refused() {
        let huge = vec![b' '; BOMB];
        let mut bomb = zip_of(&[(DOCUMENT, &huge)]);
        drop(huge);
        let lie = 100u32.to_le_bytes();
        // En-tête local (taille à l'octet 22) et répertoire central (24).
        let local = 0;
        assert_eq!(bomb[..4], 0x0403_4b50u32.to_le_bytes());
        let central = bomb
            .windows(4)
            .rposition(|window| window == 0x0201_4b50u32.to_le_bytes())
            .unwrap();
        bomb[local + 22..local + 26].copy_from_slice(&lie);
        bomb[central + 24..central + 28].copy_from_slice(&lie);
        // Le crate zip lit au plus la taille annoncée, puis le CRC ne
        // correspond pas : illisible, sans avoir décompressé la bombe.
        assert_eq!(docx_text(&bomb), Err(DocxError::Unreadable));
    }

    /// En-tête qui annonce plus que la limite : refusé sans décompresser
    /// (même si le contenu réel est petit).
    #[test]
    fn a_declared_size_over_the_limit_is_refused_before_reading() {
        let mut word = docx(&para("Bonjour"), None);
        let huge = u32::try_from(MAX_UNPACKED + 1).unwrap().to_le_bytes();
        // Répertoire central de `word/document.xml` (taille à l'octet 24).
        let name = DOCUMENT.as_bytes();
        let central = word
            .windows(4)
            .enumerate()
            .filter(|(_, window)| *window == 0x0201_4b50u32.to_le_bytes())
            .map(|(at, _)| at)
            .find(|at| word[at + 46..].starts_with(name))
            .unwrap();
        word[central + 24..central + 28].copy_from_slice(&huge);
        assert_eq!(docx_text(&word), Err(DocxError::TooLarge));
    }

    /// Archive de 10 001 entrées vides : refusée sans rien lire.
    #[test]
    fn too_many_entries_are_refused() {
        let names: Vec<String> = (0..=MAX_ENTRIES).map(|n| format!("x/{n}")).collect();
        let mut entries: Vec<(&str, &[u8])> =
            names.iter().map(|name| (name.as_str(), &b""[..])).collect();
        entries.push((DOCUMENT, b"<w:p/>"));
        assert_eq!(docx_text(&zip_of(&entries)), Err(DocxError::TooManyEntries));
    }
}
