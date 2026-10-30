//! Trame du pont : celle du protocole de base de LSP (en-têtes
//! `Content-Length: N`, ligne vide, puis N octets de JSON), pour que
//! l'extension se serve de `vscode-jsonrpc`. Bornée des deux côtés : un client
//! qui envoie des en-têtes sans fin ou un corps énorme est coupé.

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

/// En-têtes d'un message, au plus.
pub const MAX_HEADERS: usize = 1024;

/// Corps d'un message, au plus.
pub const MAX_BODY: usize = 8 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    /// En-têtes illisibles, trop longs, sans `Content-Length`.
    Malformed,
    /// Corps annoncé plus grand que [`MAX_BODY`].
    TooLarge,
    /// Connexion coupée au milieu d'un message.
    Truncated,
    Io(std::io::ErrorKind),
}

impl From<std::io::Error> for FrameError {
    fn from(error: std::io::Error) -> Self {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            FrameError::Truncated
        } else {
            FrameError::Io(error.kind())
        }
    }
}

/// Lit le message suivant ; `None` si la connexion est fermée entre deux
/// messages.
pub async fn read<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<Vec<u8>>, FrameError> {
    let mut length: Option<usize> = None;
    let mut seen = 0usize;
    let mut line = Vec::new();
    loop {
        line.clear();
        // Une ligne plus longue que ce qui reste permis est refusée sans être
        // lue en entier.
        let budget = (MAX_HEADERS - seen) as u64 + 1;
        let read = (&mut *reader)
            .take(budget)
            .read_until(b'\n', &mut line)
            .await?;
        if read == 0 {
            return if seen == 0 {
                Ok(None)
            } else {
                Err(FrameError::Truncated)
            };
        }
        seen += read;
        if seen > MAX_HEADERS || !line.ends_with(b"\n") {
            return Err(FrameError::Malformed);
        }
        let text = std::str::from_utf8(&line).map_err(|_| FrameError::Malformed)?;
        let text = text.trim_end_matches('\n').trim_end_matches('\r');
        if text.is_empty() {
            break;
        }
        let (name, value) = text.split_once(':').ok_or(FrameError::Malformed)?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            let value: usize = value.trim().parse().map_err(|_| FrameError::Malformed)?;
            if length.replace(value).is_some() {
                return Err(FrameError::Malformed);
            }
        }
    }
    let length = length.ok_or(FrameError::Malformed)?;
    if length > MAX_BODY {
        return Err(FrameError::TooLarge);
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    Ok(Some(body))
}

/// Message prêt à écrire.
pub fn encode(body: &[u8]) -> Vec<u8> {
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(body);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    async fn read_all(bytes: &[u8]) -> Vec<Result<Option<Vec<u8>>, FrameError>> {
        let mut reader = BufReader::new(bytes);
        let mut out = Vec::new();
        loop {
            let next = read(&mut reader).await;
            let stop = !matches!(next, Ok(Some(_)));
            out.push(next);
            if stop {
                return out;
            }
        }
    }

    #[tokio::test]
    async fn reads_messages_back_to_back() {
        let mut bytes = encode(br#"{"a":1}"#);
        bytes.extend(encode(b"{}"));
        let read = read_all(&bytes).await;
        assert_eq!(
            read,
            [
                Ok(Some(br#"{"a":1}"#.to_vec())),
                Ok(Some(b"{}".to_vec())),
                Ok(None)
            ]
        );
    }

    #[tokio::test]
    async fn other_headers_and_bare_newlines_are_accepted() {
        let bytes =
            b"Content-Type: application/vscode-jsonrpc; charset=utf-8\ncontent-length: 2\n\n{}";
        assert_eq!(read_all(bytes).await[0], Ok(Some(b"{}".to_vec())));
    }

    #[tokio::test]
    async fn a_body_larger_than_the_limit_is_refused_before_being_read() {
        let bytes = format!("Content-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert_eq!(
            read_all(bytes.as_bytes()).await[0],
            Err(FrameError::TooLarge)
        );
        let bytes = format!("Content-Length: {MAX_BODY}\r\n\r\n");
        // Annoncé juste à la limite : permis, mais le corps manque.
        assert_eq!(
            read_all(bytes.as_bytes()).await[0],
            Err(FrameError::Truncated)
        );
    }

    #[tokio::test]
    async fn endless_headers_are_cut() {
        let bytes = vec![b'x'; MAX_HEADERS * 4];
        assert_eq!(read_all(&bytes).await[0], Err(FrameError::Malformed));
        let mut many = Vec::new();
        while many.len() <= MAX_HEADERS {
            many.extend_from_slice(b"X-Pad: 1\r\n");
        }
        assert_eq!(read_all(&many).await[0], Err(FrameError::Malformed));
    }

    #[tokio::test]
    async fn malformed_headers_are_refused() {
        for bytes in [
            &b"\r\n{}"[..],
            b"Content-Length: two\r\n\r\n{}",
            b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            b"no colon here\r\n\r\n",
        ] {
            assert_eq!(
                read_all(bytes).await[0],
                Err(FrameError::Malformed),
                "{bytes:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_cut_in_the_middle_is_reported() {
        assert_eq!(
            read_all(b"Content-Length: 2\r\n").await[0],
            Err(FrameError::Truncated)
        );
        assert_eq!(
            read_all(b"Content-Length: 5\r\n\r\n{}").await[0],
            Err(FrameError::Truncated)
        );
    }
}
