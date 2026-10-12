//! Menu contextuel natif de la webview, filtré.
//!
//! La page ne laisse le menu natif que dans ses zones permises (messages,
//! saisie, champs éditables : voir src/lib/contextMenu.ts). Là, la webview
//! ajouterait des entrées de navigateur ; comme l'app officielle
//! (`app_darwin.m`, `webview.h`), on ne garde que les entrées d'édition.
//!
//! - Linux (`linux.rs`) : WebKitGTK, liste par exclusion sur l'action WebKit
//!   (comme l'officiel : une action inconnue reste).
//! - Windows (`windows.rs`) : WebView2, liste par INCLUSION sur le nom non
//!   traduit de l'entrée. Écart voulu à l'officiel, qui exclut une douzaine de
//!   noms (`webview/webview.h`) : le moteur Edge se met à jour tout seul,
//!   sans ChatonLM, et ajoute des entrées de navigateur d'une version à
//!   l'autre ; une entrée inconnue ne doit donc pas rester. Les touches de
//!   navigateur (F5, Ctrl+R, Ctrl+P, Ctrl+F...), que WebKitGTK n'a pas, y sont
//!   coupées aussi.
//!
//! Ici : la logique pure du filtrage de WebView2, essayée sur tous les
//! systèmes.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(all(target_os = "linux", feature = "e2e"))]
pub use linux::e2e;
#[cfg(target_os = "linux")]
pub use linux::install;
#[cfg(windows)]
pub use windows::install;

/// Entrées de WebView2 gardées, par leur nom non traduit (`get_Name` : le
/// libellé anglais en « lowerCamelCase ») : édition, orthographe, émojis,
/// copie d'une image. « pasteAndMatchStyle » et « pasteAsPlainText » : les
/// deux noms possibles de « Paste as plain text ».
#[cfg_attr(not(windows), allow(dead_code))]
const KEPT: [&str; 11] = [
    "cut",
    "copy",
    "paste",
    "pasteAndMatchStyle",
    "pasteAsPlainText",
    "selectAll",
    "undo",
    "redo",
    "emoji",
    "spellCheck",
    "copyImage",
];

/// « Inspect » : seulement quand les outils de développement existent (build
/// de débogage).
#[cfg_attr(not(windows), allow(dead_code))]
const INSPECT: &str = "inspectElement";

/// L'entrée nommée `name` reste-t-elle dans le menu ? Sans tenir compte de la
/// casse, comme l'officiel. `inspector` : garder l'inspecteur.
#[cfg_attr(not(windows), allow(dead_code))]
fn keeps(name: &str, inspector: bool) -> bool {
    KEPT.iter().any(|kept| kept.eq_ignore_ascii_case(name))
        || (inspector && INSPECT.eq_ignore_ascii_case(name))
}

/// Pour chaque entrée (`None` : séparateur), si elle reste : entrées selon
/// [`keeps`], puis séparateurs seulement entre deux entrées restantes (ni en
/// tête, ni en fin, ni doublés).
#[cfg_attr(not(windows), allow(dead_code))]
fn retained_names(entries: &[Option<&str>], inspector: bool) -> Vec<bool> {
    let mut kept = Vec::with_capacity(entries.len());
    // Une entrée restante précède-t-elle, depuis le dernier séparateur gardé ?
    let mut after_entry = false;
    for entry in entries {
        match entry {
            None => {
                kept.push(after_entry);
                after_entry = false;
            }
            Some(name) => {
                let keep = keeps(name, inspector);
                kept.push(keep);
                after_entry |= keep;
            }
        }
    }
    // Un seul séparateur peut rester en fin (les doublés sont déjà retirés).
    if let Some(last) = kept.iter().rposition(|&keep| keep)
        && entries[last].is_none()
    {
        kept[last] = false;
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Noms restants d'un menu (« — » : séparateur).
    fn shown(entries: &[Option<&str>], inspector: bool) -> Vec<String> {
        entries
            .iter()
            .zip(retained_names(entries, inspector))
            .filter(|(_, keep)| *keep)
            .map(|(entry, _)| entry.unwrap_or("—").to_owned())
            .collect()
    }

    #[test]
    fn only_editing_entries_are_kept() {
        for name in KEPT {
            assert!(keeps(name, false), "{name}");
        }
        assert!(keeps("COPY", false) && keeps("selectall", false));
        // Navigation, téléchargements, partage, et tout nom inconnu (entrée
        // ajoutée par une version plus récente d'Edge).
        for name in [
            "back",
            "forward",
            "reload",
            "saveAs",
            "print",
            "share",
            "webCapture",
            "screenshot",
            "openLinkInNewWindow",
            "saveLinkAs",
            "copyLinkLocation",
            "copyLinkToHighlight",
            "saveImageAs",
            "copyImageLocation",
            "createQrCode",
            "extension",
            "other",
            "somethingNew",
            "",
        ] {
            assert!(!keeps(name, false), "{name}");
            assert!(!keeps(name, true), "{name}");
        }
    }

    #[test]
    fn the_inspector_is_kept_only_when_allowed() {
        assert!(keeps("inspectElement", true));
        assert!(!keeps("inspectElement", false));
    }

    #[test]
    fn separators_stay_only_between_kept_entries() {
        let menu = [
            Some("back"),
            Some("forward"),
            Some("reload"),
            None,
            Some("saveAs"),
            Some("print"),
            None,
            Some("copy"),
            None,
            None,
            Some("selectAll"),
            None,
            Some("inspectElement"),
        ];
        assert_eq!(shown(&menu, false), ["copy", "—", "selectAll"]);
        assert_eq!(
            shown(&menu, true),
            ["copy", "—", "selectAll", "—", "inspectElement"]
        );
    }

    #[test]
    fn a_menu_with_nothing_left_is_empty() {
        // Clic droit sur la page sans sélection : navigation seulement.
        let menu = [Some("back"), Some("reload"), None, Some("saveAs"), None];
        assert!(!retained_names(&menu, false).contains(&true));
        assert!(shown(&[], false).is_empty());
        assert!(shown(&[None], false).is_empty());
    }
}
