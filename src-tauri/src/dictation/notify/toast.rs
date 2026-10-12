//! Contenu XML d'un toast Windows (schéma « ToastGeneric »). Logique pure,
//! essayée sur tous les systèmes ; l'envoi à WinRT est dans `windows.rs`.
//!
//! Le corps d'une [`super::Notice`] arrive déjà échappé pour les serveurs de
//! notifications de Linux (`&amp;`, `&lt;`, `&gt;`) : il est d'abord remis en
//! texte brut, puis échappé une seule fois pour le XML.

// Sous Linux, seuls les essais s'en servent.
#![cfg_attr(not(windows), allow(dead_code))]

/// Groupe de toutes les notifications de ChatonLM (avec l'étiquette, il
/// désigne celle à remplacer ou à retirer).
pub const GROUP: &str = "chatonlm";

/// Étiquette de la notification `id` : une notification de même étiquette et
/// de même groupe remplace la précédente.
pub fn tag(id: u32) -> String {
    format!("n{id}")
}

/// Toast à montrer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Toast<'a> {
    pub summary: &'a str,
    /// Corps tel que dans une `Notice` (échappé façon HTML).
    pub body: &'a str,
    /// Bouton : clé rendue au clic, libellé.
    pub action: Option<(&'a str, &'a str)>,
    /// Sans son (progressions, remplacées peu après).
    pub silent: bool,
    /// Affichée longtemps (25 s) : progression d'une correction qui dure.
    pub long: bool,
}

/// Texte d'une `Notice` remis en clair (inverse de `notify::escape`).
fn unescaped(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Texte pour un nœud ou un attribut XML : `&`, `<`, `>` et `"` échappés,
/// caractères de contrôle retirés (interdits en XML 1.0 : le toast serait
/// refusé), sauts de ligne et tabulations gardés.
fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' | '\t' => out.push(c),
            c if c.is_control() || c == '\u{fffe}' || c == '\u{ffff}' => {}
            c => out.push(c),
        }
    }
    out
}

impl Toast<'_> {
    /// Document XML du toast.
    pub fn xml(&self) -> String {
        let mut out = String::from("<toast");
        if self.long {
            out.push_str(" duration=\"long\"");
        }
        out.push_str("><visual><binding template=\"ToastGeneric\"><text>");
        out.push_str(&xml(self.summary));
        out.push_str("</text>");
        let body = unescaped(self.body);
        if !body.trim().is_empty() {
            out.push_str("<text>");
            out.push_str(&xml(&body));
            out.push_str("</text>");
        }
        out.push_str("</binding></visual>");
        if let Some((key, label)) = self.action {
            out.push_str("<actions><action content=\"");
            out.push_str(&xml(label));
            out.push_str("\" arguments=\"");
            out.push_str(&xml(key));
            out.push_str("\" activationType=\"foreground\"/></actions>");
        }
        if self.silent {
            out.push_str("<audio silent=\"true\"/>");
        }
        out.push_str("</toast>");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictation::notify::{escape, preview};

    #[test]
    fn a_plain_toast_has_its_title_and_body() {
        let toast = Toast {
            summary: "Copied to clipboard",
            body: "Hello there",
            ..Toast::default()
        };
        assert_eq!(
            toast.xml(),
            "<toast><visual><binding template=\"ToastGeneric\"><text>Copied to clipboard</text>\
             <text>Hello there</text></binding></visual></toast>"
        );
    }

    #[test]
    fn an_empty_body_adds_no_line() {
        let toast = Toast {
            summary: "Listening…",
            body: "  ",
            silent: true,
            ..Toast::default()
        };
        assert_eq!(
            toast.xml(),
            "<toast><visual><binding template=\"ToastGeneric\"><text>Listening…</text>\
             </binding></visual><audio silent=\"true\"/></toast>"
        );
    }

    #[test]
    fn text_is_escaped_exactly_once() {
        // Corps d'une notice : déjà échappé pour les serveurs de Linux.
        let dictated = "if a < b && c > d { \"x\" } &lt;";
        let body = preview(dictated);
        assert_eq!(body, escape(dictated));
        let toast = Toast {
            summary: "R&D <notes>",
            body: &body,
            ..Toast::default()
        };
        let xml = toast.xml();
        assert!(xml.contains("<text>R&amp;D &lt;notes&gt;</text>"), "{xml}");
        assert!(
            xml.contains("<text>if a &lt; b &amp;&amp; c &gt; d { &quot;x&quot; } &amp;lt;</text>"),
            "{xml}"
        );
    }

    #[test]
    fn control_characters_are_removed_but_lines_are_kept() {
        let toast = Toast {
            summary: "a\u{0}b\u{1b}[0m",
            body: "one\ntwo\u{7}\r\n",
            ..Toast::default()
        };
        let xml = toast.xml();
        assert!(xml.contains("<text>ab[0m</text>"), "{xml}");
        assert!(xml.contains("<text>one\ntwo\n</text>"), "{xml}");
    }

    #[test]
    fn an_action_becomes_a_button_with_its_key() {
        let toast = Toast {
            summary: "Transcription ready",
            body: "text",
            action: Some(("copy", "Copier \"tout\"")),
            long: true,
            ..Toast::default()
        };
        let xml = toast.xml();
        assert!(
            xml.starts_with("<toast duration=\"long\"><visual>"),
            "{xml}"
        );
        assert!(
            xml.contains(
                "<actions><action content=\"Copier &quot;tout&quot;\" arguments=\"copy\" \
                 activationType=\"foreground\"/></actions>"
            ),
            "{xml}"
        );
        assert!(xml.ends_with("</actions></toast>"), "{xml}");
    }

    #[test]
    fn tags_name_the_notification_to_replace() {
        assert_eq!(tag(7), "n7");
        assert_ne!(tag(7), tag(8));
        assert_eq!(GROUP, "chatonlm");
    }
}
