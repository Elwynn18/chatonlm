//! README d'un dépôt (35 § 3) : Markdown sans l'en-tête YAML (déjà lu dans
//! la fiche), tronqué à 100 Kio sur une fin de ligne. Le rendu et
//! l'assainissement sont faits par l'interface.

/// Taille maximale du texte renvoyé.
pub const MAX_TEXT: usize = 100 * 1024;

/// Octets lus au plus (un en-tête YAML peut peser des centaines de Ko).
pub const MAX_BODY: usize = 2 * 1024 * 1024;

/// Texte affichable d'un README : en-tête YAML (`---` … `---` ou `...`) retiré,
/// lignes vides du début retirées, coupé à [`MAX_TEXT`] octets sur une fin de
/// ligne.
pub fn prepare(raw: &str) -> String {
    let text = strip_front_matter(raw.strip_prefix('\u{feff}').unwrap_or(raw));
    let text = text.trim_start_matches(['\r', '\n']);
    truncate(text, MAX_TEXT).to_owned()
}

fn strip_front_matter(text: &str) -> &str {
    let Some(first_end) = text.find('\n') else {
        return text;
    };
    if text[..first_end].trim_end_matches('\r') != "---" {
        return text;
    }
    let mut offset = first_end + 1;
    for line in text[offset..].split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        offset += line.len();
        if content == "---" || content == "..." {
            return &text[offset..];
        }
    }
    // En-tête jamais fermé : ce n'en est pas un.
    text
}

/// Coupe à `max` octets au plus, après la dernière fin de ligne ; sans fin de
/// ligne, sur une limite de caractère.
fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    match text.as_bytes()[..max].iter().rposition(|&b| b == b'\n') {
        Some(index) => &text[..=index],
        None => {
            let mut end = max;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            &text[..end]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// README réel de Qwen/Qwen3-8B-GGUF (`35-samples/readme-Qwen3-8B-GGUF.md`).
    const QWEN3_8B: &str = include_str!("../../crates/fake-ollama/fixtures/hf/readme-qwen3-8b.md");

    #[test]
    fn the_yaml_header_is_removed() {
        assert!(QWEN3_8B.starts_with("---\n"));
        let text = prepare(QWEN3_8B);
        assert!(text.starts_with("# Qwen3-8B-GGUF"), "{}", &text[..40]);
        assert!(!text.contains("license_link:"));
        assert!(text.contains("<think>"));
        assert_eq!(prepare("---\r\na: 1\r\n...\r\n\r\nHello"), "Hello");
        assert_eq!(prepare("\u{feff}---\nx: y\n---\nBody"), "Body");
        // Pas d'en-tête, ou jamais fermé : tel quel.
        assert_eq!(prepare("# Title\n---\nx"), "# Title\n---\nx");
        assert_eq!(prepare("---\nnot closed"), "---\nnot closed");
        assert_eq!(prepare("--- \nx\n---\ny"), "--- \nx\n---\ny");
    }

    #[test]
    fn long_texts_are_cut_on_a_line_end() {
        let line = "é".repeat(30) + "\n";
        let long = line.repeat(10_000);
        let text = prepare(&long);
        assert!(text.len() <= MAX_TEXT);
        assert!(text.ends_with('\n'));
        assert!(text.len() > MAX_TEXT - line.len());
        // Sans fin de ligne : sur une limite de caractère.
        let single = "é".repeat(MAX_TEXT);
        let text = prepare(&single);
        assert!(text.len() <= MAX_TEXT && text.len() >= MAX_TEXT - 1);
        assert_eq!(prepare("short"), "short");
    }
}
