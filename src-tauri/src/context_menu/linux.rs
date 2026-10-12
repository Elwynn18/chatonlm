//! Menu contextuel natif de la webview sous Linux (WebKitGTK), filtré.
//!
//! La page ne laisse le menu natif que dans ses zones permises (messages,
//! saisie, champs éditables : voir src/lib/contextMenu.ts). Là, WebKitGTK
//! ajouterait des entrées de navigateur ; comme l'app officielle
//! (`app_darwin.m`, `webview.h`), on retire celles de navigation et de
//! téléchargement, et on garde les entrées d'édition (copier, coller,
//! orthographe, méthodes de saisie...). Les actions sont reconnues par leur
//! action WebKit, pas par leur libellé traduit. Liste par exclusion, comme
//! l'officiel : une action inconnue reste.
//!
//! Libellés : ceux de WebKitGTK, dans la langue du bureau (gettext du
//! processus), même si l'app en affiche une autre (Settings › General).
//! Pas de remplacement par `ContextMenuItem::from_stock_action_with_label` :
//! l'entrée recréée perd l'état grisé que WebKit donne aux siennes (Couper
//! ou Copier sans sélection, Coller sans rien à coller), et seul un essai
//! dans l'app réelle dirait si l'action recréée agit comme l'originale.
//!
//! Seule la logique pure est testée ici ; le branchement GTK l'est par l'e2e
//! (fonctionnalité `e2e` : commande `e2e_last_context_menu`).

use tauri::WebviewWindow;
use webkit2gtk::{
    ContextMenu, ContextMenuAction, ContextMenuExt, ContextMenuItem, ContextMenuItemExt, WebViewExt,
};

/// « Inspect Element » : seulement dans un build de débogage, le seul où
/// Tauri active les outils de développement (les essais e2e en sont un).
const INSPECTOR: bool = cfg!(debug_assertions);

/// L'action reste-t-elle dans le menu ? `inspector` : garder l'inspecteur.
fn keep(action: ContextMenuAction, inspector: bool) -> bool {
    use ContextMenuAction as A;
    match action {
        A::InspectElement => inspector,
        // Navigation : la webview ne quitte jamais l'app.
        A::GoBack
        | A::GoForward
        | A::Stop
        | A::Reload
        // Liens : menu maison de la page (Open link, Copy link).
        | A::OpenLink
        | A::OpenLinkInNewWindow
        | A::DownloadLinkToDisk
        | A::CopyLinkToClipboard
        // Images, cadres, médias : nouvelles fenêtres et téléchargements.
        | A::OpenImageInNewWindow
        | A::DownloadImageToDisk
        | A::CopyImageUrlToClipboard
        | A::OpenFrameInNewWindow
        | A::OpenVideoInNewWindow
        | A::OpenAudioInNewWindow
        | A::DownloadVideoToDisk
        | A::DownloadAudioToDisk
        | A::CopyVideoLinkToClipboard
        | A::CopyAudioLinkToClipboard
        | A::MediaPlay
        | A::MediaPause
        | A::MediaMute
        | A::ToggleMediaControls
        | A::ToggleMediaLoop
        | A::EnterVideoFullscreen
        // L'app n'ajoute aucune entrée : celles-ci viennent de WebKit
        // (« Copy Link with Highlight » sur un texte sélectionné, que
        // l'officiel retire aussi).
        | A::Custom => false,
        // Édition, orthographe, saisie, CopyImageToClipboard, et toute action
        // inconnue.
        _ => true,
    }
}

/// Entrée du menu, vue par la logique de filtrage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    Separator,
    Action(ContextMenuAction),
}

/// Pour chaque entrée, si elle reste : actions selon `keep`, puis séparateurs
/// seulement entre deux entrées restantes (ni en tête, ni en fin, ni doublés).
fn retained(entries: &[Entry], inspector: bool) -> Vec<bool> {
    let mut kept = Vec::with_capacity(entries.len());
    // Une action restante précède-t-elle, depuis le dernier séparateur gardé ?
    let mut after_action = false;
    for entry in entries {
        match *entry {
            Entry::Separator => {
                kept.push(after_action);
                after_action = false;
            }
            Entry::Action(action) => {
                let keep = keep(action, inspector);
                kept.push(keep);
                after_action |= keep;
            }
        }
    }
    // Un seul séparateur peut rester en fin (les doublés sont déjà retirés).
    if let Some(last) = kept.iter().rposition(|&keep| keep)
        && entries[last] == Entry::Separator
    {
        kept[last] = false;
    }
    kept
}

/// Libellés des entrées restantes, pour les essais : nom de l'action WebKit
/// (`Copy`, `SelectAll`...), stable quelle que soit la langue ; « — » pour
/// un séparateur.
#[cfg(any(test, feature = "e2e"))]
fn labels(entries: &[Entry], kept: &[bool]) -> Vec<String> {
    entries
        .iter()
        .zip(kept)
        .filter(|(_, keep)| **keep)
        .map(|(entry, _)| match entry {
            Entry::Separator => "—".to_owned(),
            Entry::Action(action) => format!("{action:?}"),
        })
        .collect()
}

/// Branche le filtrage sur la webview de la fenêtre.
pub fn install(window: &WebviewWindow) {
    let result = window.with_webview(|webview| {
        webview
            .inner()
            .connect_context_menu(|_, menu, _, _| filter(menu));
    });
    if let Err(error) = result {
        eprintln!("chatonlm: cannot filter the context menu: {error}");
    }
}

/// Retire les entrées exclues du menu ; `true` (aucun menu) s'il n'en reste
/// aucune, `false` pour que WebKit montre le menu modifié.
fn filter(menu: &ContextMenu) -> bool {
    let items = menu.items();
    let entries: Vec<Entry> = items.iter().map(entry).collect();
    let kept = retained(&entries, INSPECTOR);
    for (item, keep) in items.iter().zip(&kept) {
        if !keep {
            menu.remove(item);
        }
    }
    #[cfg(feature = "e2e")]
    {
        *e2e::LAST_MENU.lock() = labels(&entries, &kept);
    }
    !kept.contains(&true)
}

fn entry(item: &ContextMenuItem) -> Entry {
    if item.is_separator() {
        Entry::Separator
    } else {
        Entry::Action(item.stock_action())
    }
}

/// Essais de l'app réelle seulement : le menu natif n'est pas lisible par
/// WebDriver.
#[cfg(feature = "e2e")]
pub mod e2e {
    use parking_lot::Mutex;

    /// Libellés du dernier menu montré, après filtrage (vide s'il n'en
    /// restait rien).
    pub(super) static LAST_MENU: Mutex<Vec<String>> = parking_lot::const_mutex(Vec::new());

    /// Libellés du dernier menu montré depuis l'appel précédent (voir
    /// `labels`), puis oubliés ; vide si aucun menu.
    #[tauri::command]
    pub async fn e2e_last_context_menu() -> Vec<String> {
        std::mem::take(&mut *LAST_MENU.lock())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ContextMenuAction as A;

    const REMOVED: [ContextMenuAction; 25] = [
        A::GoBack,
        A::GoForward,
        A::Stop,
        A::Reload,
        A::OpenLink,
        A::OpenLinkInNewWindow,
        A::DownloadLinkToDisk,
        A::CopyLinkToClipboard,
        A::OpenImageInNewWindow,
        A::DownloadImageToDisk,
        A::CopyImageUrlToClipboard,
        A::OpenFrameInNewWindow,
        A::OpenVideoInNewWindow,
        A::OpenAudioInNewWindow,
        A::DownloadVideoToDisk,
        A::DownloadAudioToDisk,
        A::CopyVideoLinkToClipboard,
        A::CopyAudioLinkToClipboard,
        A::MediaPlay,
        A::MediaPause,
        A::MediaMute,
        A::ToggleMediaControls,
        A::ToggleMediaLoop,
        A::EnterVideoFullscreen,
        A::Custom,
    ];

    const KEPT: [ContextMenuAction; 22] = [
        A::Copy,
        A::Cut,
        A::Paste,
        A::PasteAsPlainText,
        A::Delete,
        A::SelectAll,
        A::SpellingGuess,
        A::NoGuessesFound,
        A::IgnoreSpelling,
        A::LearnSpelling,
        A::IgnoreGrammar,
        A::InputMethods,
        A::Unicode,
        A::InsertEmoji,
        A::CopyImageToClipboard,
        A::NoAction,
        A::FontMenu,
        A::Bold,
        A::Italic,
        A::Underline,
        A::Outline,
        A::__Unknown(9999),
    ];

    #[test]
    fn navigation_and_download_entries_are_removed() {
        for action in REMOVED {
            for inspector in [false, true] {
                assert!(!keep(action, inspector), "{action:?} should be removed");
            }
        }
    }

    #[test]
    fn editing_entries_are_kept() {
        for action in KEPT {
            for inspector in [false, true] {
                assert!(keep(action, inspector), "{action:?} should be kept");
            }
        }
    }

    #[test]
    fn inspector_is_kept_only_when_allowed() {
        assert!(keep(A::InspectElement, true));
        assert!(!keep(A::InspectElement, false));
        assert_eq!(INSPECTOR, cfg!(debug_assertions));
    }

    const SEP: Entry = Entry::Separator;

    fn act(action: ContextMenuAction) -> Entry {
        Entry::Action(action)
    }

    /// Libellés restants d'un menu.
    fn shown(entries: &[Entry]) -> Vec<String> {
        labels(entries, &retained(entries, false))
    }

    #[test]
    fn separators_stay_between_kept_entries() {
        let menu = [act(A::Copy), SEP, act(A::SelectAll)];
        assert_eq!(shown(&menu), ["Copy", "—", "SelectAll"]);
    }

    #[test]
    fn leading_separators_are_removed() {
        // Menu d'un lien sélectionné : les entrées du lien partent, puis le
        // séparateur qui les suivait.
        let menu = [
            act(A::OpenLink),
            act(A::CopyLinkToClipboard),
            SEP,
            act(A::Copy),
        ];
        assert_eq!(shown(&menu), ["Copy"]);
        assert_eq!(shown(&[SEP, SEP, act(A::Copy)]), ["Copy"]);
    }

    #[test]
    fn trailing_separators_are_removed() {
        let menu = [
            act(A::Copy),
            SEP,
            act(A::Reload),
            SEP,
            act(A::InspectElement),
        ];
        assert_eq!(shown(&menu), ["Copy"]);
        assert_eq!(shown(&[act(A::Copy), SEP, SEP]), ["Copy"]);
    }

    #[test]
    fn doubled_separators_are_merged() {
        let menu = [
            act(A::Cut),
            SEP,
            act(A::GoBack),
            act(A::Stop),
            SEP,
            act(A::Paste),
        ];
        assert_eq!(shown(&menu), ["Cut", "—", "Paste"]);
        assert_eq!(
            shown(&[act(A::Cut), SEP, SEP, SEP, act(A::Paste)]),
            ["Cut", "—", "Paste"]
        );
    }

    #[test]
    fn a_menu_with_nothing_left_is_empty() {
        // Clic droit sur la page sans sélection : navigation seulement.
        let menu = [act(A::GoBack), act(A::GoForward), act(A::Reload), SEP];
        assert!(!retained(&menu, false).contains(&true));
        assert!(shown(&[]).is_empty());
        assert!(shown(&[SEP]).is_empty());
    }

    #[test]
    fn inspector_follows_the_build() {
        let menu = [act(A::Copy), SEP, act(A::InspectElement)];
        assert_eq!(retained(&menu, true), [true, true, true]);
        assert_eq!(retained(&menu, false), [true, false, false]);
    }
}
