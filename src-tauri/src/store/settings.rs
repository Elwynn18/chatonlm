//! Réglages de l'application (table `settings`, une seule ligne).

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::i18n::{self, Lang, LanguageChoice};
use crate::ollama::address;

/// Longueurs de contexte proposées (crans du réglage) ; 0 = défaut du serveur.
pub const CONTEXT_LENGTHS: [u32; 8] = [0, 4096, 8192, 16384, 32768, 65536, 131072, 262144];

/// Durées avant déchargement proposées (crans du réglage), en secondes ;
/// 0 = défaut du serveur (rien envoyé), -1 = jusqu'au déchargement.
pub const KEEP_ALIVES: [i64; 8] = [0, 60, 300, 900, 1800, 3600, 14400, -1];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    fn as_str(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    fn parse(value: &str) -> AppResult<Self> {
        match value {
            "system" => Ok(Theme::System),
            "light" => Ok(Theme::Light),
            "dark" => Ok(Theme::Dark),
            other => Err(AppError::Internal(format!(
                "unknown theme in database: {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Settings {
    /// Adresse réglée par l'utilisateur ; vide = automatique.
    pub server_url: String,
    /// Adresse réellement utilisée : celle du serveur qui sert (`get` y met
    /// l'adresse réglée, sinon `OLLAMA_HOST`, sinon le défaut ; les commandes
    /// celle de la cible, Ollama intégré compris : `ollama::target`).
    pub effective_server_url: String,
    /// 0 = défaut du serveur.
    pub context_length: u32,
    pub theme: Theme,
    /// Vide = aucun modèle choisi.
    pub selected_model: String,
    pub window_width: Option<u32>,
    pub window_height: Option<u32>,
    /// Recherche web demandée dans le composer (bouton gardé d'un lancement à
    /// l'autre) ; « Reset to defaults » n'y touche pas.
    pub web_search_enabled: bool,
    /// `keep_alive` envoyé avec chaque requête qui charge un modèle, en
    /// secondes (crans de `KEEP_ALIVES`) ; 0 = défaut du serveur, -1 = garder
    /// chargé. « Reset to defaults » n'y touche pas.
    pub keep_alive: i64,
    /// Précharger le modèle choisi dans le chat (au choix et au démarrage).
    pub preload_model: bool,
    /// Langue de l'interface choisie (« Reset to defaults » revient à
    /// `system`).
    pub language: LanguageChoice,
    /// Langue réellement affichée (choisie, sinon celle du bureau).
    pub effective_language: Lang,
    /// Modèle de la correction (page Proofread et raccourci global) ; vide =
    /// celui du chat (`selected_model`). « Reset to defaults » n'y touche pas.
    pub proofread_model: String,
    /// Raccourci global « Correct selected text » demandé (lié par le portail
    /// du bureau) ; coupé par défaut.
    pub proofread_shortcut: bool,
    /// Ce raccourci colle le texte corrigé à la place de la sélection (sinon
    /// il le laisse dans le presse-papiers) ; activé par défaut.
    pub proofread_paste: bool,
    /// Modèle des cours (onglet Study) ; vide = celui du chat
    /// (`selected_model`). « Reset to defaults » n'y touche pas.
    pub study_model: String,
    /// Lire chaque nouvelle réponse à voix haute pendant qu'elle s'écrit.
    pub auto_read: bool,
}

/// Mise à jour partielle : seuls les champs présents changent.
#[derive(Clone, Debug, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SettingsPatch {
    /// Chaîne vide = revenir à l'adresse automatique.
    #[ts(optional)]
    pub server_url: Option<String>,
    #[ts(optional)]
    pub context_length: Option<u32>,
    #[ts(optional)]
    pub theme: Option<Theme>,
    #[ts(optional)]
    pub selected_model: Option<String>,
    #[ts(optional)]
    pub web_search_enabled: Option<bool>,
    #[ts(optional)]
    pub keep_alive: Option<i64>,
    #[ts(optional)]
    pub preload_model: Option<bool>,
    #[ts(optional)]
    pub language: Option<LanguageChoice>,
    /// Chaîne vide = modèle du chat.
    #[ts(optional)]
    pub proofread_model: Option<String>,
    #[ts(optional)]
    pub proofread_shortcut: Option<bool>,
    #[ts(optional)]
    pub proofread_paste: Option<bool>,
    /// Chaîne vide = modèle du chat.
    #[ts(optional)]
    pub study_model: Option<String>,
    #[ts(optional)]
    pub auto_read: Option<bool>,
}

pub fn get(connection: &Connection, ollama_host: Option<&str>) -> AppResult<Settings> {
    let row = connection
        .query_row(
            "SELECT server_url, context_length, theme, selected_model, window_width, window_height,
                    web_search_enabled, keep_alive, preload_model, language,
                    proofread_model, proofread_shortcut, study_model, auto_read,
                    proofread_paste
             FROM settings WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<u32>>(4)?,
                    row.get::<_, Option<u32>>(5)?,
                    row.get::<_, bool>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, bool>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, bool>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, bool>(13)?,
                    row.get::<_, bool>(14)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::Internal("settings row missing".into()))?;
    let (
        server_url,
        context_length,
        theme,
        selected_model,
        window_width,
        window_height,
        web_search_enabled,
        keep_alive,
        preload_model,
        language,
        proofread_model,
        proofread_shortcut,
        study_model,
        auto_read,
        proofread_paste,
    ) = row;
    let language = LanguageChoice::parse(&language)
        .ok_or_else(|| AppError::Internal(format!("unknown language in database: {language}")))?;
    Ok(Settings {
        effective_server_url: address::effective(&server_url, ollama_host),
        server_url,
        context_length,
        theme: Theme::parse(&theme)?,
        selected_model,
        window_width,
        window_height,
        web_search_enabled,
        keep_alive,
        preload_model,
        language,
        effective_language: language.resolve(i18n::system_language()),
        proofread_model,
        proofread_shortcut,
        proofread_paste,
        study_model,
        auto_read,
    })
}

/// Valide puis applique `patch` en une transaction ; renvoie les réglages à jour.
pub fn update(
    connection: &mut Connection,
    patch: SettingsPatch,
    ollama_host: Option<&str>,
) -> AppResult<Settings> {
    let server_url = match &patch.server_url {
        Some(value) => Some(
            address::normalize(value)
                .map_err(AppError::Invalid)?
                .unwrap_or_default(),
        ),
        None => None,
    };
    if let Some(length) = patch.context_length
        && !CONTEXT_LENGTHS.contains(&length)
    {
        // i18n-ignore : crans choisis dans la page.
        return Err(AppError::Invalid(format!(
            "unsupported context length: {length}"
        )));
    }
    if let Some(seconds) = patch.keep_alive
        && !KEEP_ALIVES.contains(&seconds)
    {
        // i18n-ignore : crans choisis dans la page.
        return Err(AppError::Invalid(format!(
            "unsupported keep alive: {seconds}"
        )));
    }
    if let Some(model) = &patch.selected_model
        && (model.trim() != model || model.len() > 256)
    {
        // i18n-ignore : nom choisi dans la liste, déjà valide.
        return Err(AppError::Invalid(format!("invalid model name: {model}")));
    }
    if let Some(model) = &patch.proofread_model
        && (model.trim() != model || model.len() > 256)
    {
        // i18n-ignore : nom choisi dans la liste, déjà valide.
        return Err(AppError::Invalid(format!("invalid model name: {model}")));
    }
    if let Some(model) = &patch.study_model
        && (model.trim() != model || model.len() > 256)
    {
        // i18n-ignore : nom choisi dans la liste, déjà valide.
        return Err(AppError::Invalid(format!("invalid model name: {model}")));
    }

    let transaction = connection.transaction()?;
    if let Some(url) = server_url {
        transaction.execute(
            "UPDATE settings SET server_url = ?1 WHERE id = 1",
            params![url],
        )?;
    }
    if let Some(length) = patch.context_length {
        transaction.execute(
            "UPDATE settings SET context_length = ?1 WHERE id = 1",
            params![length],
        )?;
    }
    if let Some(theme) = patch.theme {
        transaction.execute(
            "UPDATE settings SET theme = ?1 WHERE id = 1",
            params![theme.as_str()],
        )?;
    }
    if let Some(model) = &patch.selected_model {
        transaction.execute(
            "UPDATE settings SET selected_model = ?1 WHERE id = 1",
            params![model],
        )?;
    }
    if let Some(enabled) = patch.web_search_enabled {
        transaction.execute(
            "UPDATE settings SET web_search_enabled = ?1 WHERE id = 1",
            params![enabled],
        )?;
    }
    if let Some(seconds) = patch.keep_alive {
        transaction.execute(
            "UPDATE settings SET keep_alive = ?1 WHERE id = 1",
            params![seconds],
        )?;
    }
    if let Some(enabled) = patch.preload_model {
        transaction.execute(
            "UPDATE settings SET preload_model = ?1 WHERE id = 1",
            params![enabled],
        )?;
    }
    if let Some(enabled) = patch.auto_read {
        transaction.execute(
            "UPDATE settings SET auto_read = ?1 WHERE id = 1",
            params![enabled],
        )?;
    }
    if let Some(language) = patch.language {
        transaction.execute(
            "UPDATE settings SET language = ?1 WHERE id = 1",
            params![language.as_str()],
        )?;
    }
    if let Some(model) = &patch.proofread_model {
        transaction.execute(
            "UPDATE settings SET proofread_model = ?1 WHERE id = 1",
            params![model],
        )?;
    }
    if let Some(enabled) = patch.proofread_shortcut {
        transaction.execute(
            "UPDATE settings SET proofread_shortcut = ?1 WHERE id = 1",
            params![enabled],
        )?;
    }
    if let Some(enabled) = patch.proofread_paste {
        transaction.execute(
            "UPDATE settings SET proofread_paste = ?1 WHERE id = 1",
            params![enabled],
        )?;
    }
    if let Some(model) = &patch.study_model {
        transaction.execute(
            "UPDATE settings SET study_model = ?1 WHERE id = 1",
            params![model],
        )?;
    }
    transaction.commit()?;
    get(connection, ollama_host)
}

/// Mémorise la taille de la fenêtre (en pixels logiques).
pub fn save_window_size(connection: &Connection, width: u32, height: u32) -> AppResult<()> {
    connection.execute(
        "UPDATE settings SET window_width = ?1, window_height = ?2 WHERE id = 1",
        params![width, height],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn window_size_is_saved() {
        let store = store();
        store.blocking(|c| save_window_size(c, 1200, 800)).unwrap();
        let settings = store.blocking(|c| get(c, None)).unwrap();
        assert_eq!(
            (settings.window_width, settings.window_height),
            (Some(1200), Some(800))
        );
    }

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn defaults() {
        let settings = store().blocking(|c| get(c, None)).unwrap();
        assert_eq!(
            settings,
            Settings {
                server_url: String::new(),
                effective_server_url: address::DEFAULT_SERVER_URL.into(),
                context_length: 0,
                theme: Theme::System,
                selected_model: String::new(),
                window_width: None,
                window_height: None,
                web_search_enabled: false,
                keep_alive: 0,
                preload_model: false,
                language: LanguageChoice::System,
                effective_language: i18n::system_language(),
                proofread_model: String::new(),
                proofread_shortcut: false,
                proofread_paste: true,
                study_model: String::new(),
                auto_read: false,
            }
        );
    }

    #[test]
    fn proofread_settings_are_saved() {
        let store = store();
        let patch = SettingsPatch {
            proofread_model: Some("gemma3:4b".into()),
            proofread_shortcut: Some(true),
            ..SettingsPatch::default()
        };
        let settings = store.blocking(|c| update(c, patch, None)).unwrap();
        assert_eq!(
            (
                settings.proofread_model.as_str(),
                settings.proofread_shortcut
            ),
            ("gemma3:4b", true)
        );
        // Autres réglages : rien ne change.
        let patch = SettingsPatch {
            theme: Some(Theme::Dark),
            ..SettingsPatch::default()
        };
        let settings = store.blocking(|c| update(c, patch, None)).unwrap();
        assert_eq!(
            (
                settings.proofread_model.as_str(),
                settings.proofread_shortcut
            ),
            ("gemma3:4b", true)
        );
        let invalid = SettingsPatch {
            proofread_model: Some(" gemma3:4b".into()),
            ..SettingsPatch::default()
        };
        assert!(store.blocking(|c| update(c, invalid, None)).is_err());
    }

    #[test]
    fn the_language_is_saved_and_resolved() {
        let store = store();
        for (choice, effective) in [
            (LanguageChoice::Es, Lang::Es),
            (LanguageChoice::Fr, Lang::Fr),
            (LanguageChoice::En, Lang::En),
            (LanguageChoice::System, i18n::system_language()),
        ] {
            let patch = SettingsPatch {
                language: Some(choice),
                ..SettingsPatch::default()
            };
            let settings = store.blocking(|c| update(c, patch, None)).unwrap();
            assert_eq!(
                (settings.language, settings.effective_language),
                (choice, effective)
            );
        }
        // Valeur inconnue : refusée par la contrainte de la colonne.
        assert!(
            store
                .blocking(|c| Ok(c.execute("UPDATE settings SET language = 'de'", [])?))
                .is_err()
        );
    }

    #[test]
    fn web_search_stays_as_set_by_other_patches() {
        let store = store();
        let on = SettingsPatch {
            web_search_enabled: Some(true),
            ..Default::default()
        };
        assert!(
            store
                .blocking(|c| update(c, on, None))
                .unwrap()
                .web_search_enabled
        );
        // Les champs de « Reset to defaults » (adresse, contexte, thème) ne la
        // changent pas.
        let reset = SettingsPatch {
            server_url: Some(String::new()),
            context_length: Some(0),
            theme: Some(Theme::System),
            ..Default::default()
        };
        assert!(
            store
                .blocking(|c| update(c, reset, None))
                .unwrap()
                .web_search_enabled
        );
        assert!(store.blocking(|c| get(c, None)).unwrap().web_search_enabled);
        let off = SettingsPatch {
            web_search_enabled: Some(false),
            ..Default::default()
        };
        assert!(
            !store
                .blocking(|c| update(c, off, None))
                .unwrap()
                .web_search_enabled
        );
        let patch: SettingsPatch = serde_json::from_str(r#"{"webSearchEnabled":true}"#).unwrap();
        assert_eq!(patch.web_search_enabled, Some(true));
    }

    /// Durée en mémoire : seulement les crans proposés ; préchargement ;
    /// « Reset to defaults » (adresse, contexte, thème) n'y touche pas.
    #[test]
    fn model_loading_settings_are_kept_and_checked() {
        let store = store();
        let keep = |seconds: i64| SettingsPatch {
            keep_alive: Some(seconds),
            ..Default::default()
        };
        for seconds in KEEP_ALIVES {
            let saved = store.blocking(|c| update(c, keep(seconds), None)).unwrap();
            assert_eq!(saved.keep_alive, seconds);
        }
        assert_eq!(store.blocking(|c| get(c, None)).unwrap().keep_alive, -1);
        for refused in [1, 120, 86_400, -2] {
            let patch = SettingsPatch {
                theme: Some(Theme::Dark),
                ..keep(refused)
            };
            let Err(AppError::Invalid(message)) = store.blocking(|c| update(c, patch, None)) else {
                panic!("{refused} accepted");
            };
            assert_eq!(message, format!("unsupported keep alive: {refused}"));
        }
        // Rien d'écrit avec un cran refusé.
        let settings = store.blocking(|c| get(c, None)).unwrap();
        assert_eq!((settings.keep_alive, settings.theme), (-1, Theme::System));

        let preload: SettingsPatch =
            serde_json::from_str(r#"{"keepAlive":300,"preloadModel":true}"#).unwrap();
        let saved = store.blocking(|c| update(c, preload, None)).unwrap();
        assert_eq!((saved.keep_alive, saved.preload_model), (300, true));
        let reset = SettingsPatch {
            server_url: Some(String::new()),
            context_length: Some(0),
            theme: Some(Theme::System),
            ..Default::default()
        };
        let saved = store.blocking(|c| update(c, reset, None)).unwrap();
        assert_eq!((saved.keep_alive, saved.preload_model), (300, true));
        let off = SettingsPatch {
            preload_model: Some(false),
            ..Default::default()
        };
        assert!(
            !store
                .blocking(|c| update(c, off, None))
                .unwrap()
                .preload_model
        );
    }

    #[test]
    fn ollama_host_is_used_when_no_address_is_saved() {
        let settings = store()
            .blocking(|c| get(c, Some("127.0.0.1:11435")))
            .unwrap();
        assert_eq!(settings.effective_server_url, "http://127.0.0.1:11435");
    }

    #[test]
    fn partial_update_changes_only_given_fields() {
        let store = store();
        let patch = SettingsPatch {
            theme: Some(Theme::Dark),
            context_length: Some(32768),
            ..Default::default()
        };
        let settings = store.blocking(|c| update(c, patch, None)).unwrap();
        assert_eq!(settings.theme, Theme::Dark);
        assert_eq!(settings.context_length, 32768);
        assert_eq!(settings.server_url, "");
        assert_eq!(settings.selected_model, "");
    }

    #[test]
    fn server_url_is_normalized_and_can_go_back_to_automatic() {
        let store = store();
        let set = |value: &str| SettingsPatch {
            server_url: Some(value.into()),
            ..Default::default()
        };
        let settings = store
            .blocking(|c| update(c, set("192.168.1.20"), None))
            .unwrap();
        assert_eq!(settings.server_url, "http://192.168.1.20:11434");
        assert_eq!(settings.effective_server_url, "http://192.168.1.20:11434");
        let settings = store.blocking(|c| update(c, set(""), None)).unwrap();
        assert_eq!(settings.server_url, "");
        assert_eq!(settings.effective_server_url, address::DEFAULT_SERVER_URL);
    }

    #[test]
    fn invalid_values_are_refused_without_partial_writes() {
        let store = store();
        let patch = SettingsPatch {
            theme: Some(Theme::Dark),
            context_length: Some(1234),
            ..Default::default()
        };
        assert!(matches!(
            store.blocking(|c| update(c, patch, None)),
            Err(AppError::Invalid(_))
        ));
        let bad_url = SettingsPatch {
            server_url: Some("ftp://example.com".into()),
            ..Default::default()
        };
        assert!(matches!(
            store.blocking(|c| update(c, bad_url, None)),
            Err(AppError::Invalid(_))
        ));
        let settings = store.blocking(|c| get(c, None)).unwrap();
        assert_eq!(settings.theme, Theme::System);
    }

    #[test]
    fn unknown_fields_are_refused() {
        let error = serde_json::from_str::<SettingsPatch>(r#"{"colour":"blue"}"#).unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }
}
