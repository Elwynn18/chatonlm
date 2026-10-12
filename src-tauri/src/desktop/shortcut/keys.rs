//! Combinaison de touches d'un raccourci global choisie dans ChatonLM (sous
//! Windows, où aucun bureau ne la choisit à sa place) : contrôle de ce que la
//! page envoie, forme gardée dans les réglages, libellé montré. Logique pure,
//! essayée sur tous les systèmes ; l'enregistrement auprès de Windows est dans
//! `windows.rs`.
//!
//! La page envoie `KeyboardEvent.keyCode`, qui est sous WebView2 le code
//! virtuel de Windows : c'est lui que `RegisterHotKey` attend, et il suit la
//! disposition du clavier (la touche marquée « A » d'un clavier AZERTY donne
//! bien le code de A). `KeyboardEvent.code`, lui, désigne la touche physique
//! et lierait une autre touche que celle pressée.

// Sous Linux, seuls les essais et le type de la commande s'en servent.
#![cfg_attr(not(windows), allow(dead_code))]

use serde::Deserialize;
use ts_rs::TS;

use super::{
    Command, DICTATION_ID, PROOFREAD_ID, ShortcutProblem, ShortcutStatus, on_activated,
    on_deactivated, reason,
};
use crate::i18n::{Lang, t};

/// Combinaison envoyée par la page (zone de saisie d'un raccourci).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ShortcutKeys {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Touche Windows.
    pub meta: bool,
    /// `KeyboardEvent.keyCode` (code virtuel de Windows).
    pub key_code: u32,
}

/// Touche principale d'une combinaison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Lettre majuscule ASCII (`b'A'..=b'Z'`).
    Letter(u8),
    /// Chiffre du pavé principal (0 à 9).
    Digit(u8),
    /// Touche de fonction (1 à 24).
    Function(u8),
}

/// Pourquoi la combinaison envoyée par la page est refusée.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeysError {
    /// Ni lettre, ni chiffre, ni touche de fonction.
    InvalidKey,
    /// Lettre ou chiffre sans Ctrl, Alt ni Windows ; ou F12 seule (réservée
    /// par Windows au débogueur).
    NeedsModifier,
}

/// Combinaison contrôlée.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Combo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    pub key: Key,
}

const CTRL: &str = "Ctrl";
const ALT: &str = "Alt";
const SHIFT: &str = "Shift";
const WIN: &str = "Win";

impl Key {
    /// Touche du code virtuel `code`, si elle est permise.
    fn from_code(code: u32) -> Option<Self> {
        match code {
            0x41..=0x5A => Some(Key::Letter(code as u8)),
            0x30..=0x39 => Some(Key::Digit((code - 0x30) as u8)),
            0x70..=0x87 => Some(Key::Function((code - 0x70 + 1) as u8)),
            _ => None,
        }
    }

    /// Nom de la touche (« D », « 4 », « F9 »), le même dans toutes les
    /// langues.
    fn name(self) -> String {
        match self {
            Key::Letter(letter) => char::from(letter).to_string(),
            Key::Digit(digit) => digit.to_string(),
            Key::Function(number) => format!("F{number}"),
        }
    }

    fn parse(name: &str) -> Option<Self> {
        let bytes = name.as_bytes();
        match bytes {
            [letter @ b'A'..=b'Z'] => Some(Key::Letter(*letter)),
            [digit @ b'0'..=b'9'] => Some(Key::Digit(digit - b'0')),
            [b'F', number @ ..] if !number.is_empty() && number[0] != b'0' => {
                let number: u8 = name[1..].parse().ok()?;
                (1..=24).contains(&number).then_some(Key::Function(number))
            }
            _ => None,
        }
    }

    /// Nom de la touche pour le greffon des raccourcis (valeurs de
    /// `KeyboardEvent.code` : « KeyD », « Digit4 », « F9 »), qu'il traduit en
    /// code virtuel de même nom.
    pub fn code_name(self) -> String {
        match self {
            Key::Letter(letter) => format!("Key{}", char::from(letter)),
            Key::Digit(digit) => format!("Digit{digit}"),
            Key::Function(number) => format!("F{number}"),
        }
    }
}

impl Combo {
    /// Combinaison par défaut du raccourci `id` : Ctrl+Alt+D pour la dictée,
    /// Ctrl+Alt+C pour la correction (les touches proposées au portail sous
    /// Linux).
    pub fn default_for(id: &str) -> Self {
        Self {
            ctrl: true,
            alt: true,
            shift: false,
            meta: false,
            key: Key::Letter(if id == PROOFREAD_ID { b'C' } else { b'D' }),
        }
    }

    /// Contrôle la combinaison envoyée par la page.
    pub fn from_keys(keys: ShortcutKeys) -> Result<Self, KeysError> {
        let key = Key::from_code(keys.key_code).ok_or(KeysError::InvalidKey)?;
        let combo = Self {
            ctrl: keys.ctrl,
            alt: keys.alt,
            shift: keys.shift,
            meta: keys.meta,
            key,
        };
        if combo.is_copy() {
            return Err(KeysError::InvalidKey);
        }
        if combo.allowed() {
            Ok(combo)
        } else {
            Err(KeysError::NeedsModifier)
        }
    }

    /// Ctrl+C seul : la copie que ChatonLM simule pour lire la sélection à
    /// corriger. Enregistrée comme raccourci global, elle serait captée par
    /// ChatonLM au lieu d'atteindre l'application, et plus rien ne se
    /// copierait par cette touche dans tout le bureau.
    fn is_copy(self) -> bool {
        self.ctrl && !self.alt && !self.shift && !self.meta && self.key == Key::Letter(b'C')
    }

    /// Une lettre ou un chiffre demande Ctrl, Alt ou Windows (avec Maj seule,
    /// ce serait une majuscule) ; une touche de fonction peut être seule,
    /// sauf F12.
    fn allowed(self) -> bool {
        let strong = self.ctrl || self.alt || self.meta;
        match self.key {
            Key::Letter(_) | Key::Digit(_) => strong,
            Key::Function(12) => strong || self.shift,
            Key::Function(_) => true,
        }
    }

    fn written(self, shift: &str, win: &str) -> String {
        let mut parts: Vec<String> = Vec::new();
        for (held, name) in [
            (self.ctrl, CTRL),
            (self.alt, ALT),
            (self.shift, shift),
            (self.meta, win),
        ] {
            if held {
                parts.push(name.to_owned());
            }
        }
        parts.push(self.key.name());
        parts.join("+")
    }

    /// Forme gardée dans les réglages (« Ctrl+Alt+D »), relue par
    /// [`Combo::parse`].
    pub fn stored(self) -> String {
        self.written(SHIFT, WIN)
    }

    /// Libellé montré (page, infobulle, notifications), noms des touches Maj
    /// et Windows dans la langue `lang`.
    pub fn label(self, lang: Lang) -> String {
        self.written(
            &t!("shortcut.keys.shift" in lang),
            &t!("shortcut.keys.win" in lang),
        )
    }

    /// Relit une combinaison gardée ; `None` si elle n'est pas de cette forme
    /// (ou ne serait pas acceptée de la page) : le défaut sert alors.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts: Vec<&str> = text.split('+').collect();
        let key = Key::parse(parts.pop()?)?;
        let mut combo = Self {
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
            key,
        };
        for part in parts {
            let held = match part {
                CTRL => &mut combo.ctrl,
                ALT => &mut combo.alt,
                SHIFT => &mut combo.shift,
                WIN => &mut combo.meta,
                _ => return None,
            };
            if std::mem::replace(held, true) {
                return None;
            }
        }
        // Mêmes règles que pour une saisie.
        combo.allowed().then_some(combo)
    }
}

/// Combinaisons des deux raccourcis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Combos {
    pub dictation: Combo,
    pub proofread: Combo,
}

impl Default for Combos {
    fn default() -> Self {
        Self {
            dictation: Combo::default_for(DICTATION_ID),
            proofread: Combo::default_for(PROOFREAD_ID),
        }
    }
}

impl Combos {
    /// D'après les réglages (`None`, ou valeur illisible : défaut).
    pub fn from_stored(dictation: Option<&str>, proofread: Option<&str>) -> Self {
        let read = |stored: Option<&str>, id: &str| {
            stored
                .and_then(Combo::parse)
                .unwrap_or_else(|| Combo::default_for(id))
        };
        Self {
            dictation: read(dictation, DICTATION_ID),
            proofread: read(proofread, PROOFREAD_ID),
        }
    }

    pub fn get(&self, id: &str) -> Combo {
        if id == PROOFREAD_ID {
            self.proofread
        } else {
            self.dictation
        }
    }

    pub fn set(&mut self, id: &str, combo: Combo) {
        if id == PROOFREAD_ID {
            self.proofread = combo;
        } else {
            self.dictation = combo;
        }
    }

    /// Combinaison de l'autre raccourci.
    pub fn other(&self, id: &str) -> Combo {
        if id == PROOFREAD_ID {
            self.dictation
        } else {
            self.proofread
        }
    }
}

/// Résultat de l'enregistrement d'une combinaison auprès de Windows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Bound,
    /// Une autre application tient déjà la combinaison.
    Taken,
    /// Autre échec : message du système ou du greffon (en anglais).
    Failed(String),
}

impl Outcome {
    /// D'après le résultat du greffon des raccourcis, qui ne rend qu'un
    /// texte : « HotKey already registered » quand `RegisterHotKey` répond
    /// que la combinaison est prise (erreur 1409).
    pub fn of(result: Result<(), String>) -> Self {
        match result {
            Ok(()) => Outcome::Bound,
            Err(message) if message.contains("already registered") => Outcome::Taken,
            Err(message) => Outcome::Failed(message),
        }
    }
}

/// Statut montré pour un raccourci : coupé s'il n'est pas voulu ; lié avec
/// son libellé (aussi pendant une saisie, où rien n'est enregistré :
/// `outcome` vaut `None`) ; sinon la raison de l'échec, dans la langue `lang`.
pub fn status(wanted: bool, combo: Combo, outcome: Option<&Outcome>, lang: Lang) -> ShortcutStatus {
    let unavailable = |problem, detail: String| ShortcutStatus::Unavailable {
        problem,
        reason: reason(problem, Some(&detail), lang),
        detail: Some(detail),
    };
    match outcome {
        _ if !wanted => ShortcutStatus::Disabled,
        Some(Outcome::Bound) | None => ShortcutStatus::Bound {
            trigger: combo.label(lang),
        },
        Some(Outcome::Taken) => unavailable(ShortcutProblem::Taken, combo.stored()),
        Some(Outcome::Failed(error)) => unavailable(ShortcutProblem::Failed, error.clone()),
    }
}

/// Que faire de l'appui (`pressed`) ou du relâchement du raccourci `id` :
/// bascule ou push-to-talk (`hold`) pour la dictée, correction de la
/// sélection à l'appui pour l'autre.
pub(super) fn command_for(id: &str, pressed: bool, hold: bool) -> Option<Command> {
    match (id, pressed) {
        (DICTATION_ID, true) => Some(on_activated(hold)),
        (DICTATION_ID, false) => on_deactivated(hold),
        (PROOFREAD_ID, true) => Some(Command::Proofread),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_registration_is_taken_or_failed() {
        assert_eq!(Outcome::of(Ok(())), Outcome::Bound);
        assert_eq!(
            Outcome::of(Err(
                "HotKey already registered: HotKey { mods: Modifiers(ALT), key: KeyD, id: 1 }"
                    .into()
            )),
            Outcome::Taken
        );
        assert_eq!(
            Outcome::of(Err("Access is denied. (os error 5)".into())),
            Outcome::Failed("Access is denied. (os error 5)".into())
        );
    }

    #[test]
    fn statuses_follow_the_registration() {
        let combo = Combo::parse("Ctrl+Shift+D").unwrap();
        let bound = ShortcutStatus::Bound {
            trigger: "Ctrl+Maj+D".into(),
        };
        assert_eq!(
            status(false, combo, Some(&Outcome::Bound), Lang::Fr),
            ShortcutStatus::Disabled
        );
        assert_eq!(status(true, combo, Some(&Outcome::Bound), Lang::Fr), bound);
        // Saisie en cours : rien n'est enregistré, la combinaison reste montrée.
        assert_eq!(status(true, combo, None, Lang::Fr), bound);
        let ShortcutStatus::Unavailable {
            problem,
            reason,
            detail,
        } = status(true, combo, Some(&Outcome::Taken), Lang::En)
        else {
            panic!("unavailable expected");
        };
        assert_eq!(problem, ShortcutProblem::Taken);
        assert_eq!(reason, "Ctrl+Shift+D is already used by another app");
        assert_eq!(detail.as_deref(), Some("Ctrl+Shift+D"));
        let failed = status(true, combo, Some(&Outcome::Failed("boom".into())), Lang::En);
        assert!(
            matches!(&failed, ShortcutStatus::Unavailable { problem: ShortcutProblem::Failed, reason, .. } if reason.ends_with("boom")),
            "{failed:?}"
        );
    }

    #[test]
    fn presses_and_releases_become_commands() {
        assert_eq!(
            command_for(DICTATION_ID, true, false),
            Some(Command::Toggle)
        );
        assert_eq!(command_for(DICTATION_ID, false, false), None);
        assert_eq!(command_for(DICTATION_ID, true, true), Some(Command::Start));
        assert_eq!(command_for(DICTATION_ID, false, true), Some(Command::Stop));
        // La correction ignore le push-to-talk et le relâchement.
        assert_eq!(
            command_for(PROOFREAD_ID, true, true),
            Some(Command::Proofread)
        );
        assert_eq!(command_for(PROOFREAD_ID, false, true), None);
        assert_eq!(command_for("other", true, false), None);
    }

    fn keys(ctrl: bool, alt: bool, shift: bool, meta: bool, key_code: u32) -> ShortcutKeys {
        ShortcutKeys {
            ctrl,
            alt,
            shift,
            meta,
            key_code,
        }
    }

    #[test]
    fn the_defaults_are_those_proposed_on_linux() {
        assert_eq!(Combo::default_for(DICTATION_ID).stored(), "Ctrl+Alt+D");
        assert_eq!(Combo::default_for(PROOFREAD_ID).stored(), "Ctrl+Alt+C");
        let combos = Combos::default();
        assert_eq!(combos.get(DICTATION_ID), combos.other(PROOFREAD_ID));
        assert_ne!(combos.dictation, combos.proofread);
    }

    #[test]
    fn a_pressed_combination_becomes_a_combo() {
        // Ctrl+Alt+D : keyCode 68.
        let combo = Combo::from_keys(keys(true, true, false, false, 68)).unwrap();
        assert_eq!(combo, Combo::default_for(DICTATION_ID));
        assert_eq!(combo.key.code_name(), "KeyD");
        // Ctrl+Maj+Win+4.
        let combo = Combo::from_keys(keys(true, false, true, true, 0x34)).unwrap();
        assert_eq!(combo.stored(), "Ctrl+Shift+Win+4");
        assert_eq!(combo.key.code_name(), "Digit4");
        // Touche de fonction seule.
        let combo = Combo::from_keys(keys(false, false, false, false, 0x78)).unwrap();
        assert_eq!(combo.stored(), "F9");
        assert_eq!(combo.key.code_name(), "F9");
        let combo = Combo::from_keys(keys(false, true, false, false, 0x87)).unwrap();
        assert_eq!(combo.stored(), "Alt+F24");
    }

    #[test]
    fn keys_that_cannot_be_a_global_shortcut_are_refused() {
        // Flèche, Entrée, Échap, espace, pavé numérique, modificateur seul.
        for code in [37, 13, 27, 32, 0x60, 16, 17, 18, 91, 0, 0x88, 0x5B] {
            assert_eq!(
                Combo::from_keys(keys(true, true, false, false, code)),
                Err(KeysError::InvalidKey),
                "{code}"
            );
        }
        // Lettre ou chiffre sans Ctrl, Alt ni Windows (Maj seule : une majuscule).
        for (shift, code) in [(false, 68), (true, 68), (false, 0x31), (true, 0x31)] {
            assert_eq!(
                Combo::from_keys(keys(false, false, shift, false, code)),
                Err(KeysError::NeedsModifier)
            );
        }
        // F12 seule : réservée par Windows.
        assert_eq!(
            Combo::from_keys(keys(false, false, false, false, 0x7B)),
            Err(KeysError::NeedsModifier)
        );
        assert!(Combo::from_keys(keys(false, false, true, false, 0x7B)).is_ok());
        assert!(Combo::from_keys(keys(false, false, false, true, 68)).is_ok());
        // Ctrl+C seul : la copie simulée pour lire la sélection.
        assert_eq!(
            Combo::from_keys(keys(true, false, false, false, 0x43)),
            Err(KeysError::InvalidKey)
        );
        assert!(Combo::from_keys(keys(true, true, false, false, 0x43)).is_ok());
        assert!(Combo::from_keys(keys(true, false, true, false, 0x43)).is_ok());
    }

    #[test]
    fn a_stored_combination_is_read_back() {
        for text in [
            "Ctrl+Alt+D",
            "Ctrl+Alt+Shift+Win+Z",
            "Alt+0",
            "F1",
            "Shift+F12",
            "Win+F24",
        ] {
            let combo = Combo::parse(text).unwrap_or_else(|| panic!("{text} refused"));
            assert_eq!(combo.stored(), text);
        }
        for text in [
            "",
            "D",
            "Shift+D",
            "Ctrl+",
            "Ctrl+Ctrl+D",
            "ctrl+alt+d",
            "Ctrl+Alt+DD",
            "Ctrl+Alt+é",
            "Ctrl+Space",
            "F0",
            "F25",
            "F012",
            "F12",
            "Ctrl+Alt",
            "Meta+D",
        ] {
            assert_eq!(Combo::parse(text), None, "{text}");
        }
    }

    #[test]
    fn unreadable_settings_fall_back_to_the_defaults() {
        assert_eq!(Combos::from_stored(None, None), Combos::default());
        assert_eq!(
            Combos::from_stored(Some("garbage"), Some("")),
            Combos::default()
        );
        let mut combos = Combos::from_stored(Some("Alt+F9"), None);
        assert_eq!(combos.dictation.stored(), "Alt+F9");
        assert_eq!(combos.proofread, Combo::default_for(PROOFREAD_ID));
        combos.set(PROOFREAD_ID, Combo::parse("Ctrl+Win+P").unwrap());
        assert_eq!(combos.get(PROOFREAD_ID).stored(), "Ctrl+Win+P");
        assert_eq!(combos.other(PROOFREAD_ID).stored(), "Alt+F9");
    }

    #[test]
    fn labels_name_the_keys_in_the_language_of_the_interface() {
        let combo = Combo::parse("Ctrl+Shift+Win+D").unwrap();
        assert_eq!(combo.label(Lang::En), "Ctrl+Shift+Win+D");
        assert_eq!(combo.label(Lang::Fr), "Ctrl+Maj+Win+D");
        assert_eq!(combo.label(Lang::Es), "Ctrl+Mayús+Win+D");
    }
}
