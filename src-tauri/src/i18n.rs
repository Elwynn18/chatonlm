//! Langue de l'interface : choix de l'utilisateur (Settings › General), langue
//! du bureau, et textes affichés par le moteur (zone de notification,
//! notifications, erreurs renvoyées à la page, titres des sélecteurs).
//!
//! Les textes viennent des mêmes catalogues que ceux du frontend :
//! `src/locales/<langue>/backend.json` (objets imbriqués, clé = chemin pointé,
//! « {nom} » pour une valeur, objet `{ "one": …, "other": … }` pour un
//! pluriel). `scripts/check-i18n.mjs` vérifie que chaque clé de `t!` existe
//! et que les trois langues ont les mêmes clés et les mêmes valeurs.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::LazyLock;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;
use ts_rs::TS;

use crate::store::{Store, settings};

/// Langue d'affichage effective.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Lang {
    #[default]
    En,
    Es,
    Fr,
}

impl Lang {
    #[cfg(test)]
    pub const ALL: [Lang; 3] = [Lang::En, Lang::Es, Lang::Fr];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Es => "es",
            Lang::Fr => "fr",
        }
    }

    /// Langue d'une étiquette de locale POSIX ou BCP 47 (`fr_FR.UTF-8`,
    /// `es-419`, `en`), si elle est proposée.
    fn from_locale(tag: &str) -> Option<Lang> {
        let language = tag
            .split(['_', '-', '.', '@'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match language.as_str() {
            "en" => Some(Lang::En),
            "es" => Some(Lang::Es),
            "fr" => Some(Lang::Fr),
            _ => None,
        }
    }
}

/// Réglage de la langue : celle du bureau, ou une langue imposée.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum LanguageChoice {
    #[default]
    System,
    En,
    Es,
    Fr,
}

impl LanguageChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            LanguageChoice::System => "system",
            LanguageChoice::En => "en",
            LanguageChoice::Es => "es",
            LanguageChoice::Fr => "fr",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system" => Some(LanguageChoice::System),
            "en" => Some(LanguageChoice::En),
            "es" => Some(LanguageChoice::Es),
            "fr" => Some(LanguageChoice::Fr),
            _ => None,
        }
    }

    /// Langue effective, `system` étant la langue du bureau.
    pub fn resolve(self, system: Lang) -> Lang {
        match self {
            LanguageChoice::System => system,
            LanguageChoice::En => Lang::En,
            LanguageChoice::Es => Lang::Es,
            LanguageChoice::Fr => Lang::Fr,
        }
    }
}

/// Langue du bureau, lue une fois : variables de gettext (celles que suivent
/// GTK et les autres applications), anglais si elle n'est pas proposée. Sous
/// Windows : voir [`language_from_windows`].
pub fn system_language() -> Lang {
    static SYSTEM: LazyLock<Lang> = LazyLock::new(|| {
        let var = |name: &str| std::env::var(name).ok();
        #[cfg(windows)]
        let lang = language_from_windows(var, ui_languages);
        #[cfg(not(windows))]
        let lang = language_from_env(var);
        lang
    });
    *SYSTEM
}

/// Langue du bureau sous Windows : les variables de gettext quand elles
/// donnent une locale (essais, terminal Git Bash), comme sous Linux ; sinon
/// les langues d'affichage de l'utilisateur, dans l'ordre de ses préférences
/// (`fr-FR`, `en-US`...) : la première proposée l'emporte, et aucune ne
/// l'est = anglais.
#[cfg(any(windows, test))]
fn language_from_windows(
    var: impl Fn(&str) -> Option<String>,
    ui_languages: impl FnOnce() -> Vec<String>,
) -> Lang {
    let has_locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .any(|name| var(name).is_some_and(|value| !value.is_empty()));
    if has_locale {
        return language_from_env(var);
    }
    ui_languages()
        .iter()
        .find_map(|tag| Lang::from_locale(tag))
        .unwrap_or(Lang::En)
}

/// Langues d'affichage de l'utilisateur (réglage « Langue d'affichage de
/// Windows », et non le format régional des dates et des nombres), de la
/// préférée à la moins préférée ; aucune si le système ne répond pas.
#[cfg(windows)]
fn ui_languages() -> Vec<String> {
    use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    use windows::core::PWSTR;

    let (mut count, mut length) = (0u32, 0u32);
    // SAFETY : le premier appel, sans tampon, donne la taille à prévoir ; le
    // second écrit au plus `length` unités dans un tampon de cette taille.
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut length).is_err()
            || length == 0
        {
            return Vec::new();
        }
        let mut buffer = vec![0u16; length as usize];
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut length,
        )
        .is_err()
        {
            return Vec::new();
        }
        crate::os_util::from_wide_list(&buffer)
    }
}

/// Comme gettext : la locale des messages est la première non vide de
/// LC_ALL, LC_MESSAGES et LANG ; LANGUAGE (liste séparée par « : »), si elle
/// est donnée et que cette locale existe et n'est pas « C », la remplace : sa première
/// langue proposée l'emporte, et aucune ne l'est = anglais.
fn language_from_env(var: impl Fn(&str) -> Option<String>) -> Lang {
    let non_empty = |name: &str| var(name).filter(|value| !value.is_empty());
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(non_empty);
    // Aucune locale (« C » par défaut) ou « C » : LANGUAGE ignorée.
    let Some(locale) = locale else {
        return Lang::En;
    };
    if locale == "C" || locale.starts_with("C.") || locale == "POSIX" {
        return Lang::En;
    }
    if let Some(list) = non_empty("LANGUAGE") {
        return list
            .split(':')
            .find_map(Lang::from_locale)
            .unwrap_or(Lang::En);
    }
    Lang::from_locale(&locale).unwrap_or(Lang::En)
}

static CURRENT: RwLock<Lang> = RwLock::new(Lang::En);

/// Langue des textes du moteur, réglée au démarrage puis à chaque changement
/// du réglage.
pub fn current() -> Lang {
    *CURRENT.read()
}

pub fn set_current(lang: Lang) {
    *CURRENT.write() = lang;
}

/// Langue des réglages, celle du bureau si la base est illisible.
pub fn saved(store: Option<&Store>) -> Lang {
    store
        .and_then(|store| store.blocking(|c| settings::get(c, None)).ok())
        .map(|saved| saved.language)
        .unwrap_or_default()
        .resolve(system_language())
}

/// Nouvelle langue choisie : textes du moteur, puis ce qui est déjà affiché
/// hors de la page (menu et infobulle de la zone de notification) et la
/// raison du raccourci montrée par la page.
pub fn apply(app: &AppHandle, lang: Lang) {
    if current() == lang {
        return;
    }
    set_current(lang);
    crate::desktop::language_changed(app);
}

/// Entrée d'un catalogue : texte, ou formes d'un pluriel (`one`, `other`...).
#[derive(Debug)]
enum Entry {
    Text(String),
    Plural(HashMap<String, String>),
}

type Catalog = HashMap<String, Entry>;

static CATALOGS: LazyLock<HashMap<Lang, Catalog>> = LazyLock::new(|| {
    [
        (Lang::En, include_str!("../../src/locales/en/backend.json")),
        (Lang::Es, include_str!("../../src/locales/es/backend.json")),
        (Lang::Fr, include_str!("../../src/locales/fr/backend.json")),
    ]
    .into_iter()
    .map(|(lang, source)| {
        let value: Value = serde_json::from_str(source)
            .unwrap_or_else(|error| panic!("invalid {} backend.json: {error}", lang.code()));
        let mut catalog = Catalog::new();
        flatten("", &value, &mut catalog);
        (lang, catalog)
    })
    .collect()
});

/// Un objet dont toutes les valeurs sont des textes et qui a `other` est un
/// pluriel ; les autres objets sont des groupes de clés.
fn flatten(prefix: &str, value: &Value, catalog: &mut Catalog) {
    let Value::Object(map) = value else {
        return;
    };
    for (key, value) in map {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            Value::String(text) => {
                catalog.insert(path, Entry::Text(text.clone()));
            }
            Value::Object(forms)
                if forms.contains_key("other") && forms.values().all(Value::is_string) =>
            {
                let forms = forms
                    .iter()
                    .map(|(form, text)| (form.clone(), text.as_str().unwrap_or("").to_owned()))
                    .collect();
                catalog.insert(path, Entry::Plural(forms));
            }
            Value::Object(_) => flatten(&path, value, catalog),
            _ => {}
        }
    }
}

/// Catégorie de pluriel CLDR d'un entier (seules `one` et `other` servent).
fn plural_form(lang: Lang, count: u64) -> &'static str {
    match (lang, count) {
        (Lang::Fr, 0 | 1) | (Lang::En | Lang::Es, 1) => "one",
        _ => "other",
    }
}

/// Texte de `key` dans `lang` (anglais s'il y manque, la clé elle-même s'il
/// manque aussi en anglais), valeurs `{nom}` remplacées. Pour un pluriel,
/// `count` choisit la forme et remplit `{count}`.
pub fn text(lang: Lang, key: &str, count: Option<u64>, args: &[(&str, &dyn Display)]) -> String {
    let lookup = |lang: Lang| CATALOGS.get(&lang).and_then(|catalog| catalog.get(key));
    let Some(entry) = lookup(lang).or_else(|| lookup(Lang::En)) else {
        debug_assert!(false, "missing translation key: {key}");
        return key.to_owned();
    };
    render(select(entry, lang, count), count, args)
}

/// Forme à employer : le texte, ou la forme plurielle de `count` (`other` si
/// la langue ne la donne pas).
fn select(entry: &Entry, lang: Lang, count: Option<u64>) -> &str {
    match entry {
        Entry::Text(text) => text,
        Entry::Plural(forms) => forms
            .get(plural_form(lang, count.unwrap_or(0)))
            .or_else(|| forms.get("other"))
            .map(String::as_str)
            .unwrap_or_default(),
    }
}

/// Remplace `{count}` et les `{nom}` donnés ; un nom absent reste tel quel.
fn render(template: &str, count: Option<u64>, args: &[(&str, &dyn Display)]) -> String {
    let mut result = template.to_owned();
    if let Some(count) = count {
        result = result.replace("{count}", &count.to_string());
    }
    for (name, value) in args {
        result = result.replace(&format!("{{{name}}}"), &value.to_string());
    }
    result
}

/// Texte traduit dans la langue courante : `t!("tray.quit")`,
/// `t!("errors.read", path = path.display())` ; dans une langue donnée :
/// `t!("tray.quit" in lang)`, `t!("errors.read" in lang, path = …)` (menu
/// de la zone de notification, essais). La clé reste en tête : c'est là que
/// `scripts/check-i18n.mjs` la lit.
///
/// Pas encore de `tn!` (pluriel) : aucun texte du moteur n'en a besoin ; à
/// écrire sur ce modèle avec `text(…, Some(count), …)` le jour venu.
macro_rules! t {
    ($key:literal) => {
        $crate::i18n::text($crate::i18n::current(), $key, None, &[])
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::text(
            $crate::i18n::current(),
            $key,
            None,
            &[$((stringify!($name), &$value as &dyn std::fmt::Display)),+],
        )
    };
    ($key:literal in $lang:expr) => {
        $crate::i18n::text($lang, $key, None, &[])
    };
    ($key:literal in $lang:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::text(
            $lang,
            $key,
            None,
            &[$((stringify!($name), &$value as &dyn std::fmt::Display)),+],
        )
    };
}

pub(crate) use t;

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn the_desktop_language_follows_gettext() {
        assert_eq!(language_from_env(env(&[])), Lang::En);
        assert_eq!(language_from_env(env(&[("LANG", "fr_FR.UTF-8")])), Lang::Fr);
        assert_eq!(language_from_env(env(&[("LANG", "es_MX.UTF-8")])), Lang::Es);
        assert_eq!(language_from_env(env(&[("LANG", "de_DE.UTF-8")])), Lang::En);
        // LC_ALL, puis LC_MESSAGES, avant LANG ; une variable vide ne compte pas.
        assert_eq!(
            language_from_env(env(&[
                ("LANG", "en_US.UTF-8"),
                ("LC_MESSAGES", "es_ES.UTF-8"),
            ])),
            Lang::Es
        );
        assert_eq!(
            language_from_env(env(&[
                ("LANG", "en_US.UTF-8"),
                ("LC_MESSAGES", "es_ES.UTF-8"),
                ("LC_ALL", "fr_CA.UTF-8"),
            ])),
            Lang::Fr
        );
        assert_eq!(
            language_from_env(env(&[("LANG", "fr_FR.UTF-8"), ("LC_ALL", "")])),
            Lang::Fr
        );
        // LANGUAGE : première langue proposée de la liste, anglais sinon.
        assert_eq!(
            language_from_env(env(&[("LANG", "en_US.UTF-8"), ("LANGUAGE", "de:es:fr")])),
            Lang::Es
        );
        assert_eq!(
            language_from_env(env(&[("LANG", "fr_FR.UTF-8"), ("LANGUAGE", "de")])),
            Lang::En
        );
        // Sans locale, ou locale « C » : LANGUAGE ignorée, comme gettext.
        assert_eq!(language_from_env(env(&[("LANGUAGE", "fr")])), Lang::En);
        assert_eq!(
            language_from_env(env(&[("LC_ALL", "C.UTF-8"), ("LANGUAGE", "fr")])),
            Lang::En
        );
    }

    #[test]
    fn the_windows_desktop_language_follows_the_display_languages() {
        let list = |tags: &[&str]| {
            let tags: Vec<String> = tags.iter().map(|tag| (*tag).to_owned()).collect();
            move || tags
        };
        // Première langue proposée de la liste, anglais sinon.
        for (tags, expected) in [
            (&["fr-FR", "en-US"][..], Lang::Fr),
            (&["es-419"][..], Lang::Es),
            (&["en-GB", "fr-FR"][..], Lang::En),
            (&["de-DE", "es-ES", "fr-FR"][..], Lang::Es),
            (&["de-DE", "ja-JP"][..], Lang::En),
            (&[][..], Lang::En),
        ] {
            assert_eq!(
                language_from_windows(env(&[]), list(tags)),
                expected,
                "{tags:?}"
            );
        }
        // Une locale de gettext (essais, Git Bash) passe avant, comme sous
        // Linux ; LANGUAGE seule ou une variable vide ne comptent pas.
        assert_eq!(
            language_from_windows(env(&[("LANG", "en_US.UTF-8")]), list(&["fr-FR"])),
            Lang::En
        );
        assert_eq!(
            language_from_windows(env(&[("LC_ALL", "es_ES.UTF-8")]), list(&["fr-FR"])),
            Lang::Es
        );
        assert_eq!(
            language_from_windows(env(&[("LANGUAGE", "es"), ("LANG", "")]), list(&["fr-FR"])),
            Lang::Fr
        );
    }

    #[test]
    fn the_choice_resolves_to_a_language() {
        assert_eq!(LanguageChoice::System.resolve(Lang::Fr), Lang::Fr);
        assert_eq!(LanguageChoice::Es.resolve(Lang::Fr), Lang::Es);
        for choice in ["system", "en", "es", "fr"] {
            assert_eq!(LanguageChoice::parse(choice).unwrap().as_str(), choice);
        }
        assert_eq!(LanguageChoice::parse("de"), None);
    }

    #[test]
    fn plural_forms_follow_each_language() {
        assert_eq!(plural_form(Lang::En, 0), "other");
        assert_eq!(plural_form(Lang::En, 1), "one");
        assert_eq!(plural_form(Lang::Es, 0), "other");
        assert_eq!(plural_form(Lang::Fr, 0), "one");
        assert_eq!(plural_form(Lang::Fr, 1), "one");
        assert_eq!(plural_form(Lang::Fr, 2), "other");
    }

    #[test]
    fn plurals_and_values_are_filled_in() {
        let plural = Entry::Plural(HashMap::from([
            ("one".to_owned(), "{count} model".to_owned()),
            ("other".to_owned(), "{count} models on {server}".to_owned()),
        ]));
        let server: &dyn Display = &"localhost";
        assert_eq!(
            render(select(&plural, Lang::En, Some(1)), Some(1), &[]),
            "1 model"
        );
        assert_eq!(
            render(
                select(&plural, Lang::En, Some(3)),
                Some(3),
                &[("server", server)]
            ),
            "3 models on localhost"
        );
        assert_eq!(select(&plural, Lang::Fr, Some(0)), "{count} model");
        // Forme absente : « other ».
        let only_other = Entry::Plural(HashMap::from([("other".to_owned(), "x".to_owned())]));
        assert_eq!(select(&only_other, Lang::En, Some(1)), "x");
        // Valeur non donnée : laissée telle quelle.
        assert_eq!(render("Hello {name}", None, &[]), "Hello {name}");
    }

    #[test]
    fn texts_come_from_the_catalog_of_the_language() {
        let en = text(Lang::En, "dialogs.selectFiles", None, &[]);
        assert_eq!(en, "Select Files");
        assert_ne!(text(Lang::Fr, "dialogs.selectFiles", None, &[]), en);
    }

    #[test]
    fn every_catalog_parses_with_the_same_keys() {
        let keys = |lang: Lang| {
            let mut keys: Vec<&String> = CATALOGS[&lang].keys().collect();
            keys.sort();
            keys
        };
        assert!(!keys(Lang::En).is_empty());
        for lang in Lang::ALL {
            assert_eq!(keys(lang), keys(Lang::En), "{}", lang.code());
        }
    }
}
