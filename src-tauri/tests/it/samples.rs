//! Fichiers d'essai générés par du code (aucun fichier tiers dans le dépôt) :
//! PDF minimaux et images PNG. Partagé par les tests de la bibliothèque
//! (`#[path]`) et les tests d'intégration.

#![allow(dead_code)]

use std::fmt::Write as _;

/// PDF dont chaque page porte ces lignes de texte, en Helvetica (police
/// standard, non embarquée, WinAnsiEncoding) ; une page sans ligne est vide.
pub fn pdf(pages: &[&[&str]]) -> Vec<u8> {
    let contents: Vec<Vec<u8>> = pages
        .iter()
        .map(|lines| {
            if lines.is_empty() {
                return Vec::new();
            }
            let mut stream = b"BT /F1 12 Tf 16 TL 72 720 Td".to_vec();
            for (index, line) in lines.iter().enumerate() {
                if index > 0 {
                    stream.extend_from_slice(b" T*");
                }
                stream.extend_from_slice(b" (");
                stream.extend_from_slice(&win_ansi(line));
                stream.extend_from_slice(b") Tj");
            }
            stream.extend_from_slice(b" ET");
            stream
        })
        .collect();
    document(&contents)
}

/// PDF d'une page sans texte : un rectangle dessiné.
pub fn pdf_without_text() -> Vec<u8> {
    document(&[b"0.2 0.4 0.8 rg 100 100 300 200 re f".to_vec()])
}

/// Document à partir des flux de contenu de ses pages.
fn document(contents: &[Vec<u8>]) -> Vec<u8> {
    // 1 : catalogue, 2 : arbre des pages, 3 : police, puis page et contenu.
    let mut objects: Vec<Vec<u8>> = Vec::new();
    let kids: Vec<String> = (0..contents.len())
        .map(|index| format!("{} 0 R", 4 + 2 * index))
        .collect();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objects.push(
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            contents.len()
        )
        .into_bytes(),
    );
    objects.push(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    );
    for (index, content) in contents.iter().enumerate() {
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                5 + 2 * index
            )
            .into_bytes(),
        );
        let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        stream.extend_from_slice(content);
        stream.extend_from_slice(b"\nendstream");
        objects.push(stream);
    }

    let mut pdf = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    let mut table = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for offset in offsets {
        let _ = writeln!(table, "{offset:010} 00000 n ");
    }
    let _ = write!(
        table,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    );
    pdf.extend_from_slice(table.as_bytes());
    pdf
}

/// Chaîne littérale PDF en WinAnsiEncoding : caractères spéciaux échappés,
/// caractères hors ASCII en octal.
fn win_ansi(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for c in text.chars() {
        let code = match c {
            '(' | ')' | '\\' => {
                bytes.push(b'\\');
                c as u32
            }
            'œ' => 0x9c,
            'Œ' => 0x8c,
            '€' => 0x80,
            '’' => 0x92,
            _ => c as u32,
        };
        let code = u8::try_from(code).expect("character outside WinAnsiEncoding");
        if code.is_ascii() {
            bytes.push(code);
        } else {
            bytes.extend_from_slice(format!("\\{code:03o}").as_bytes());
        }
    }
    bytes
}

/// Image PNG de `width` × `height` pixels d'une seule couleur (RVB), sans
/// compression (blocs « stored » de zlib).
pub fn png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    // Chaque ligne : filtre « None » (0), puis les pixels.
    let mut line = vec![0];
    for _ in 0..width {
        line.extend_from_slice(&rgb);
    }
    let raw = line.repeat(height as usize);
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (index, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(index + 1 == blocks.len()));
        let length = u16::try_from(block.len()).unwrap();
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]); // 8 bits, RVB
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [(b"IHDR", header), (b"IDAT", zlib), (b"IEND", Vec::new())] {
        png.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
        let mut chunk = kind.to_vec();
        chunk.extend_from_slice(&data);
        png.extend_from_slice(&chunk);
        png.extend_from_slice(&crc32(&chunk).to_be_bytes());
    }
    png
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
