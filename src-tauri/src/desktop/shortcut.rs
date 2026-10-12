//! Raccourcis globaux : dictée (bascule, ou push-to-talk avec l'appui et le
//! relâchement) et correction du texte sélectionné (propre à ChatonLM).
//!
//! - Linux (`linux.rs`) : portail XDG GlobalShortcuts ; le bureau choisit les
//!   touches dans sa propre boîte (« CTRL+ALT+D » et « CTRL+ALT+C » proposés).
//! - Windows (`windows.rs`) : `RegisterHotKey` par le greffon des raccourcis
//!   de Tauri ; rien dans le système ne choisit la touche, ChatonLM le fait
//!   donc (zone de saisie de la page, combinaison gardée dans les réglages :
//!   `keys.rs`, mêmes défauts).
//!
//! Ici : ce que les deux partagent (ensemble voulu, commandes, raisons d'un
//! raccourci absent) et l'API du reste de l'app, de mêmes signatures sur les
//! deux systèmes.

pub mod keys;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as os;
#[cfg(windows)]
use windows as os;

use tauri::{AppHandle, Manager};

use super::{Desktop, ShortcutChooser, ShortcutProblem, ShortcutStatus, emit_status, tray};
use crate::dictation::Dictation;
use crate::error::AppResult;
use crate::i18n::{self, Lang, t};

pub use keys::ShortcutKeys;
pub(super) use os::{Shortcuts, apply_proofread, init};
pub use os::{apply, bind_shortcut, bind_shortcuts, configure_shortcut};

/// Identifiant du raccourci de la dictée (auprès du portail, et dans les
/// commandes de la page).
pub const DICTATION_ID: &str = "dictation";

/// Identifiant du raccourci « Correct selected text ».
pub const PROOFREAD_ID: &str = "proofread";

/// Qui choisit les touches : le bureau (Linux), ou ChatonLM (Windows).
pub fn chooser() -> ShortcutChooser {
    if cfg!(windows) {
        ShortcutChooser::App
    } else {
        ShortcutChooser::Desktop
    }
}

/// Combinaison choisie dans la page pour le raccourci `id` (Windows) :
/// contrôlée, gardée dans les réglages, enregistrée aussitôt. Sous Linux, où
/// le bureau choisit : erreur.
pub async fn set_trigger(app: &AppHandle, id: &str, keys: ShortcutKeys) -> AppResult<()> {
    #[cfg(windows)]
    {
        os::set_trigger(app, id, Some(keys)).await
    }
    #[cfg(not(windows))]
    {
        let _ = (app, id, keys);
        Err(crate::error::AppError::Invalid(t!(
            "shortcut.cannotConfigure"
        )))
    }
}

/// Combinaison choisie oubliée : retour à celle par défaut (Windows).
pub async fn reset_trigger(app: &AppHandle, id: &str) -> AppResult<()> {
    #[cfg(windows)]
    {
        os::set_trigger(app, id, None).await
    }
    #[cfg(not(windows))]
    {
        let _ = (app, id);
        Err(crate::error::AppError::Invalid(t!(
            "shortcut.cannotConfigure"
        )))
    }
}

/// Zone de saisie d'une combinaison ouverte (`active`) ou fermée : pendant la
/// saisie, les raccourcis globaux sont suspendus, sinon la combinaison
/// actuelle lancerait la dictée au lieu d'arriver à la page (Windows ; rien
/// sous Linux).
pub async fn recording(app: &AppHandle, active: bool) {
    #[cfg(windows)]
    os::recording(app, active).await;
    #[cfg(not(windows))]
    let _ = (app, active);
}

/// Raison montrée par la page pour `problem`, dans la langue `lang`, avec le
/// détail technique d'un échec du bureau s'il y en a un (pour `Taken` : la
/// combinaison, telle que gardée dans les réglages).
fn reason(problem: ShortcutProblem, detail: Option<&str>, lang: Lang) -> String {
    match (problem, detail) {
        (ShortcutProblem::NoPortal, _) => t!("shortcut.noPortal" in lang),
        (ShortcutProblem::Refused, _) => t!("shortcut.refused" in lang),
        (ShortcutProblem::Failed, None) => t!("shortcut.failed" in lang),
        (ShortcutProblem::Failed, Some(error)) if cfg!(windows) => {
            t!("shortcut.registerFailed" in lang, error = error)
        }
        (ShortcutProblem::Failed, Some(error)) => {
            t!("shortcut.failedWithError" in lang, error = error)
        }
        (ShortcutProblem::Lost, _) => t!("shortcut.lost" in lang),
        (ShortcutProblem::Taken, combo) => {
            let trigger = combo
                .and_then(keys::Combo::parse)
                .map(|combo| combo.label(lang))
                .unwrap_or_else(|| combo.unwrap_or_default().to_owned());
            t!("shortcut.taken" in lang, trigger = trigger)
        }
    }
}

/// Statut « pas de raccourci », raison dans la langue de l'interface.
#[cfg_attr(windows, allow(dead_code))]
fn unavailable_status(problem: ShortcutProblem, detail: Option<String>) -> ShortcutStatus {
    ShortcutStatus::Unavailable {
        problem,
        reason: reason(problem, detail.as_deref(), i18n::current()),
        detail,
    }
}

/// Raccourcis à lier : dictée active, réglage `proofreadShortcut`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Wanted {
    pub dictation: bool,
    pub proofread: bool,
}

// Certaines ne servent qu'au portail.
#[cfg_attr(windows, allow(dead_code))]
impl Wanted {
    fn any(self) -> bool {
        self.dictation || self.proofread
    }

    /// Raccourcis présents dans les deux ensembles.
    fn and(self, other: Wanted) -> Wanted {
        Wanted {
            dictation: self.dictation && other.dictation,
            proofread: self.proofread && other.proofread,
        }
    }

    fn contains(self, id: &str) -> bool {
        match id {
            DICTATION_ID => self.dictation,
            PROOFREAD_ID => self.proofread,
            _ => false,
        }
    }

    /// Identifiants voulus, dans l'ordre de la boîte du bureau.
    fn ids(self) -> Vec<&'static str> {
        [DICTATION_ID, PROOFREAD_ID]
            .into_iter()
            .filter(|id| self.contains(id))
            .collect()
    }
}

/// Statut tel que lu : raison refaite dans la langue de l'interface (elle a
/// pu changer depuis l'échec).
#[cfg_attr(windows, allow(dead_code))]
fn refreshed(status: ShortcutStatus) -> ShortcutStatus {
    match status {
        ShortcutStatus::Unavailable {
            problem, detail, ..
        } => unavailable_status(problem, detail),
        status => status,
    }
}

/// Que faire d'un appui ou d'un relâchement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Toggle,
    Start,
    Stop,
    /// Corriger le texte sélectionné.
    Proofread,
}

fn on_activated(hold: bool) -> Command {
    if hold {
        Command::Start
    } else {
        Command::Toggle
    }
}

fn on_deactivated(hold: bool) -> Option<Command> {
    hold.then_some(Command::Stop)
}

fn execute(app: &AppHandle, command: Command) {
    let dictation = app.state::<Dictation>();
    match command {
        Command::Toggle => dictation.toggle(app),
        Command::Start => dictation.start(app),
        Command::Stop => dictation.stop(app),
        Command::Proofread => crate::proofread::selection::correct_selection(app),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_lets_the_desktop_choose_and_windows_the_app() {
        assert_eq!(
            chooser(),
            if cfg!(windows) {
                ShortcutChooser::App
            } else {
                ShortcutChooser::Desktop
            }
        );
    }

    #[test]
    fn a_taken_shortcut_names_its_keys_in_the_language() {
        assert_eq!(
            reason(ShortcutProblem::Taken, Some("Ctrl+Shift+D"), Lang::En),
            "Ctrl+Shift+D is already used by another app"
        );
        assert_eq!(
            reason(ShortcutProblem::Taken, Some("Ctrl+Shift+D"), Lang::Fr),
            "Ctrl+Maj+D est déjà utilisé par une autre application"
        );
        let ShortcutStatus::Unavailable { problem, .. } = refreshed(unavailable_status(
            ShortcutProblem::Taken,
            Some("F9".into()),
        )) else {
            panic!("unavailable expected");
        };
        assert_eq!(problem, ShortcutProblem::Taken);
    }
}
