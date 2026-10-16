//! Faux serveurs Telegram et Discord (fonctionnalité `fake`) : essais du
//! crate, essais de l'app, et e2e par le binaire `fake-messengers`.
//!
//! Les deux se pilotent de la même façon, en Rust comme par HTTP :
//! - `POST /fake/message` avec `{"user": "42", "name": "Ana", "text": "…"}`
//!   fait arriver un message privé de ce compte ; champs en option :
//!   `"image": true` (joint une petite image PNG), `"voice": true` (joint le
//!   message vocal `fixtures/voice-speech.ogg` : son voisé synthétique en trois
//!   salves séparées de silences, que la garde « y a-t-il de la parole ? » de
//!   l'app laisse passer, ce qu'un ton continu ne fait pas), `"private": false`
//!   (message d'un groupe ou d'un serveur, que le bot doit ignorer),
//!   `"bot": true` (auteur qui est un bot), `"age": 600` (envoyé il y a tant
//!   de secondes), et pour Telegram `"messageId": 7` (identifiant imposé :
//!   le même message remis de nouveau, comme après une reprise de l'écoute) ;
//! - `GET /fake/sent` rend, dans l'ordre, ce que le bot a fait :
//!   `[{"kind": "send" | "edit" | "typing", "channel": "…", "id": "…",
//!   "text": "…"}]` (`id` et `text` absents pour `typing` ; `text` tel que
//!   reçu, et pour Telegram `"parseMode"`, pour Discord `"allowedMentions"`
//!   et `"flags"`).
//!
//!   Une image envoyée par le bot (`multipart/form-data`, lu aussi
//!   strictement qu'un vrai serveur : un corps mal formé est refusé en 400)
//!   y est notée `{"kind": "photo", "channel", "id", "filename", "size",
//!   "contentType", "upload"}`, avec `"caption"` s'il y a une légende ;
//!   `upload` dit le type d'envoi : `"photo"` ou `"document"` pour Telegram
//!   (avec `"parseMode"`), `"attachment"` pour Discord (avec
//!   `"allowedMentions"` et `"flags"`). L'indicateur « envoie une photo… »
//!   de Telegram est noté `{"kind": "typing", "channel", "action":
//!   "upload_photo"}`.
//!
//! Un seul jeton est accepté ([`telegram::TOKEN`], [`discord::TOKEN`]) ; tout
//! autre reçoit le refus de la vraie messagerie.

pub mod discord;
pub mod telegram;

/// Partie d'un corps `multipart/form-data`.
pub(crate) struct FormPart {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub data: Vec<u8>,
}

/// Vrai pour un en-tête `Content-Type` de formulaire `multipart/form-data`.
pub(crate) fn is_multipart(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|value| {
        value
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("multipart/form-data")
    })
}

/// Lit un corps `multipart/form-data` (RFC 7578, RFC 2046 § 5.1) sans
/// tolérance : frontière annoncée par l'en-tête, fins de ligne CRLF, chaque
/// partie nommée par `Content-Disposition: form-data; name="…"`, frontière
/// finale `--`. Rend la raison d'un refus.
pub(crate) fn parse_multipart(content_type: &str, body: &[u8]) -> Result<Vec<FormPart>, String> {
    let boundary = content_type
        .split(';')
        .skip(1)
        .filter_map(|param| param.trim().split_once('='))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case("boundary"))
        .map(|(_, value)| value.trim().trim_matches('"').to_owned())
        .filter(|boundary| (1..=70).contains(&boundary.len()))
        .ok_or("missing boundary")?;
    let delimiter = format!("--{boundary}");
    let mut rest = body
        .strip_prefix(delimiter.as_bytes())
        .ok_or("body does not start with the boundary")?;
    let separator = format!("\r\n{delimiter}");
    let mut parts = Vec::new();
    loop {
        if rest.starts_with(b"--") {
            return if parts.is_empty() {
                Err("no part".to_owned())
            } else {
                Ok(parts)
            };
        }
        rest = rest
            .strip_prefix(b"\r\n")
            .ok_or("boundary not followed by CRLF")?;
        let head_end = find(rest, b"\r\n\r\n").ok_or("part headers not ended")?;
        let head = std::str::from_utf8(&rest[..head_end]).map_err(|_| "part headers not UTF-8")?;
        rest = &rest[head_end + 4..];
        let end = find(rest, separator.as_bytes()).ok_or("part not ended by the boundary")?;
        let data = rest[..end].to_vec();
        rest = &rest[end + separator.len()..];

        let mut disposition = None;
        let mut content_type = None;
        for line in head.split("\r\n") {
            let (name, value) = line.split_once(':').ok_or("malformed part header")?;
            match name.trim().to_ascii_lowercase().as_str() {
                "content-disposition" => disposition = Some(value.trim().to_owned()),
                "content-type" => content_type = Some(value.trim().to_owned()),
                _ => {}
            }
        }
        let disposition = disposition.ok_or("part without Content-Disposition")?;
        let mut params = disposition.split(';');
        if params.next().map(str::trim) != Some("form-data") {
            return Err("part is not form-data".to_owned());
        }
        let mut name = None;
        let mut filename = None;
        for param in params {
            let (key, value) = param.trim().split_once('=').ok_or("malformed parameter")?;
            let value = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .ok_or("parameter not quoted")?
                .to_owned();
            match key {
                "name" => name = Some(value),
                "filename" => filename = Some(value),
                _ => {}
            }
        }
        parts.push(FormPart {
            name: name.ok_or("part without a name")?,
            filename,
            content_type,
            data,
        });
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TYPE: &str = "multipart/form-data; boundary=XyZ";

    #[test]
    fn a_well_formed_body_gives_its_parts() {
        let body = b"--XyZ\r\nContent-Disposition: form-data; name=\"chat_id\"\r\n\r\n42\r\n--XyZ\r\n\
            Content-Disposition: form-data; name=\"photo\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n\
            \x89PNG\r\n--\r\n--XyZ--\r\n";
        let parts = parse_multipart(TYPE, body).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name, "chat_id");
        assert_eq!(parts[0].data, b"42");
        assert_eq!(parts[0].filename, None);
        assert_eq!(parts[1].name, "photo");
        assert_eq!(parts[1].filename.as_deref(), Some("a.png"));
        assert_eq!(parts[1].content_type.as_deref(), Some("image/png"));
        // Une fin de ligne suivie de « -- » dans le contenu n'est pas la frontière.
        assert_eq!(parts[1].data, b"\x89PNG\r\n--");
    }

    #[test]
    fn a_malformed_body_is_refused() {
        let part = "Content-Disposition: form-data; name=\"a\"\r\n\r\nx";
        let cases: [(&str, String); 6] = [
            ("multipart/form-data", format!("--XyZ\r\n{part}\r\n--XyZ--")),
            (TYPE, format!("--Other\r\n{part}\r\n--XyZ--")),
            (TYPE, format!("--XyZ\n{part}\r\n--XyZ--")),
            (TYPE, format!("--XyZ\r\n{part}")),
            (
                TYPE,
                "--XyZ\r\nContent-Disposition: form-data\r\n\r\nx\r\n--XyZ--".to_owned(),
            ),
            (TYPE, "--XyZ--".to_owned()),
        ];
        for (content_type, body) in cases {
            assert!(
                parse_multipart(content_type, body.as_bytes()).is_err(),
                "{body:?}"
            );
        }
        assert!(is_multipart(Some("Multipart/Form-Data; boundary=a")));
        assert!(!is_multipart(Some("application/json")));
        assert!(!is_multipart(None));
    }
}
