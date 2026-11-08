//! Lecture d'un flux SSE (`text/event-stream`), morceau par morceau : les
//! blocs arrivent coupés n'importe où (au milieu d'une ligne, d'un caractère
//! UTF-8). Champ lu : `data` (plusieurs lignes jointes par `\n` ; une erreur
//! en cours de flux arrive en `data: {"error": …}` chez b12110, qui n'envoie
//! jamais de champ `error:`) ; les commentaires (`:` : pings du serveur
//! pendant un long prompt) et les autres champs (`event`, `id`, `error`…)
//! sont ignorés. Un bloc (ligne en cours comprise)
//! plus grand que la borne est refusé.

use super::InferenceError;

/// Bloc lu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SseItem {
    Data(String),
}

pub(super) struct SseParser {
    line: Vec<u8>,
    data: String,
    max: usize,
}

impl SseParser {
    pub(super) fn new(max: usize) -> Self {
        Self {
            line: Vec::new(),
            data: String::new(),
            max,
        }
    }

    /// Ajoute des octets reçus ; rend les blocs finis.
    pub(super) fn push(&mut self, mut bytes: &[u8]) -> Result<Vec<SseItem>, InferenceError> {
        let mut items = Vec::new();
        while !bytes.is_empty() {
            let end = bytes.iter().position(|byte| *byte == b'\n');
            let (part, rest) = match end {
                Some(at) => (&bytes[..at], Some(&bytes[at + 1..])),
                None => (bytes, None),
            };
            if self.line.len() + part.len() + self.data.len() > self.max {
                return Err(InferenceError::Malformed(format!(
                    "event larger than {} bytes",
                    self.max
                )));
            }
            self.line.extend_from_slice(part);
            match rest {
                Some(rest) => {
                    self.end_of_line(&mut items)?;
                    bytes = rest;
                }
                None => break,
            }
        }
        Ok(items)
    }

    /// Fin du flux : rend le bloc en cours, même sans ligne vide finale.
    pub(super) fn finish(&mut self) -> Result<Vec<SseItem>, InferenceError> {
        let mut items = Vec::new();
        if !self.line.is_empty() {
            self.end_of_line(&mut items)?;
        }
        self.dispatch(&mut items);
        Ok(items)
    }

    fn end_of_line(&mut self, items: &mut Vec<SseItem>) -> Result<(), InferenceError> {
        let mut line = std::mem::take(&mut self.line);
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let line = String::from_utf8(line)
            .map_err(|_| InferenceError::Malformed("event is not UTF-8".into()))?;
        if line.is_empty() {
            self.dispatch(items);
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }
        let (field, value) = line.split_once(':').unwrap_or((&line, ""));
        if field != "data" {
            return Ok(());
        }
        let value = value.strip_prefix(' ').unwrap_or(value);
        if !self.data.is_empty() {
            self.data.push('\n');
        }
        self.data.push_str(value);
        Ok(())
    }

    fn dispatch(&mut self, items: &mut Vec<SseItem>) {
        if !self.data.is_empty() {
            items.push(SseItem::Data(std::mem::take(&mut self.data)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lit le flux en morceaux de `size` octets.
    fn read(stream: &[u8], size: usize) -> Vec<SseItem> {
        let mut parser = SseParser::new(1024);
        let mut items = Vec::new();
        for chunk in stream.chunks(size) {
            items.extend(parser.push(chunk).unwrap());
        }
        items.extend(parser.finish().unwrap());
        items
    }

    #[test]
    fn events_cut_anywhere_are_put_back_together() {
        let stream = "data: {\"a\":\"é\"}\n\n: ping\n\ndata: [DONE]\n\n".as_bytes();
        let expected = vec![
            SseItem::Data("{\"a\":\"é\"}".into()),
            SseItem::Data("[DONE]".into()),
        ];
        for size in 1..stream.len() {
            assert_eq!(read(stream, size), expected, "morceaux de {size}");
        }
    }

    #[test]
    fn crlf_several_data_lines_and_other_fields() {
        // Un champ `error:` (jamais envoyé par b12110) est ignoré comme les
        // autres.
        let stream = b"event: x\r\ndata:one\r\ndata: two\r\nid: 3\r\n\r\nerror: {\"code\":500}\n\n";
        assert_eq!(read(stream, 7), [SseItem::Data("one\ntwo".into())]);
        // Dernier bloc sans ligne vide : rendu à la fin.
        assert_eq!(read(b"data: last", 3), [SseItem::Data("last".into())]);
        // Commentaire seul : rien.
        assert!(read(b":\n\n: ping\n\n", 2).is_empty());
    }

    #[test]
    fn an_event_larger_than_the_bound_is_refused() {
        let mut parser = SseParser::new(16);
        assert!(parser.push(b"data: 0123456789").is_ok());
        assert!(matches!(
            parser.push(b"abc"),
            Err(InferenceError::Malformed(_))
        ));
        // La borne compte aussi les lignes `data` déjà lues du bloc.
        let mut parser = SseParser::new(16);
        assert!(parser.push(b"data: 01234567\n").is_ok());
        assert!(parser.push(b"data: 890").is_err());
        // Des blocs successifs plus petits que la borne passent.
        let mut parser = SseParser::new(16);
        for _ in 0..100 {
            assert_eq!(parser.push(b"data: 0123456\n\n").unwrap().len(), 1);
        }
        assert!(matches!(
            SseParser::new(64).push(b"data: \xff\n"),
            Err(InferenceError::Malformed(_))
        ));
    }
}
