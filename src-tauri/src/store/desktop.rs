//! Réglages de l'intégration au bureau (table `desktop_settings`, une seule
//! ligne, migration 0013) : à part de `settings`, que « Reset to defaults »
//! remet à zéro en partie. Entrée du menu des applications, raccourcis
//! (Windows), croix de la fenêtre.

use rusqlite::{Connection, params};

use crate::error::AppResult;

/// Entrée du menu des applications voulue (AppImage) : vrai par défaut,
/// faux une fois retirée par l'utilisateur.
pub fn menu_entry(connection: &Connection) -> AppResult<bool> {
    Ok(connection.query_row(
        "SELECT menu_entry FROM desktop_settings WHERE id = 1",
        [],
        |row| row.get(0),
    )?)
}

pub fn set_menu_entry(connection: &Connection, enabled: bool) -> AppResult<()> {
    connection.execute(
        "UPDATE desktop_settings SET menu_entry = ?1 WHERE id = 1",
        params![enabled],
    )?;
    Ok(())
}

/// Croix de la fenêtre : vrai (par défaut) = masquée dans la zone de
/// notification, faux = l'app quitte. Migration 0032.
pub fn close_to_tray(connection: &Connection) -> AppResult<bool> {
    Ok(connection.query_row(
        "SELECT close_to_tray FROM desktop_settings WHERE id = 1",
        [],
        |row| row.get(0),
    )?)
}

pub fn set_close_to_tray(connection: &Connection, enabled: bool) -> AppResult<()> {
    connection.execute(
        "UPDATE desktop_settings SET close_to_tray = ?1 WHERE id = 1",
        params![enabled],
    )?;
    Ok(())
}

/// Combinaisons des raccourcis globaux choisies dans ChatonLM (dictée,
/// correction), telles que gardées (« Ctrl+Alt+D ») ; `None` : celle par
/// défaut. Migration 0018 ; lues sous Windows seulement.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn shortcut_keys(connection: &Connection) -> AppResult<(Option<String>, Option<String>)> {
    Ok(connection.query_row(
        "SELECT dictation_keys, proofread_keys FROM desktop_settings WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

/// Garde la combinaison du raccourci `id` (« proofread », sinon la dictée) ;
/// `None` : retour à celle par défaut.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn set_shortcut_keys(connection: &Connection, id: &str, keys: Option<&str>) -> AppResult<()> {
    let sql = if id == "proofread" {
        "UPDATE desktop_settings SET proofread_keys = ?1 WHERE id = 1"
    } else {
        "UPDATE desktop_settings SET dictation_keys = ?1 WHERE id = 1"
    };
    connection.execute(sql, params![keys])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::store::settings::{self, SettingsPatch, Theme};

    #[test]
    fn the_menu_entry_is_wanted_until_removed() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|connection| {
                assert!(menu_entry(connection)?);
                set_menu_entry(connection, false)?;
                assert!(!menu_entry(connection)?);
                // Champs de « Reset to defaults » : rien ne change ici.
                let reset = SettingsPatch {
                    server_url: Some(String::new()),
                    context_length: Some(0),
                    theme: Some(Theme::System),
                    language: Some(crate::i18n::LanguageChoice::System),
                    ..SettingsPatch::default()
                };
                settings::update(connection, reset, None)?;
                assert!(!menu_entry(connection)?);
                set_menu_entry(connection, true)?;
                assert!(menu_entry(connection)?);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn the_close_button_hides_until_set_to_quit() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|connection| {
                assert!(close_to_tray(connection)?);
                set_close_to_tray(connection, false)?;
                assert!(!close_to_tray(connection)?);
                // « Reset to defaults » n'y touche pas, ni l'entrée de menu.
                let reset = SettingsPatch {
                    server_url: Some(String::new()),
                    theme: Some(Theme::System),
                    ..SettingsPatch::default()
                };
                settings::update(connection, reset, None)?;
                set_menu_entry(connection, true)?;
                assert!(!close_to_tray(connection)?);
                assert!(
                    connection
                        .execute("UPDATE desktop_settings SET close_to_tray = 2", [])
                        .is_err()
                );
                set_close_to_tray(connection, true)?;
                assert!(close_to_tray(connection)?);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn shortcut_keys_are_kept_per_shortcut_and_forgotten() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|connection| {
                assert_eq!(shortcut_keys(connection)?, (None, None));
                set_shortcut_keys(connection, "dictation", Some("Alt+F9"))?;
                assert_eq!(shortcut_keys(connection)?, (Some("Alt+F9".into()), None));
                set_shortcut_keys(connection, "proofread", Some("Ctrl+Win+P"))?;
                set_shortcut_keys(connection, "dictation", None)?;
                assert_eq!(
                    shortcut_keys(connection)?,
                    (None, Some("Ctrl+Win+P".into()))
                );
                // L'entrée de menu n'y est pour rien.
                assert!(menu_entry(connection)?);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn the_table_holds_one_boolean_row() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|connection| {
                assert!(
                    connection
                        .execute("UPDATE desktop_settings SET menu_entry = 2", [])
                        .is_err()
                );
                assert!(
                    connection
                        .execute("INSERT INTO desktop_settings (id) VALUES (2)", [])
                        .is_err()
                );
                Ok(())
            })
            .unwrap();
    }
}
