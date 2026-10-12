//! Réglages des mises à jour (table `update_settings`, une seule ligne,
//! migration 0015) : à part de `settings`, que « Reset to defaults » remet à
//! zéro en partie.

use rusqlite::{Connection, params};

use crate::error::AppResult;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateSettings {
    /// « Auto-download updates ».
    pub auto_download: bool,
    /// Version déjà annoncée par la notification « Update available ».
    pub notified_version: Option<String>,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            auto_download: true,
            notified_version: None,
        }
    }
}

pub fn get(connection: &mut Connection) -> AppResult<UpdateSettings> {
    Ok(connection.query_row(
        "SELECT auto_download, notified_version FROM update_settings WHERE id = 1",
        [],
        |row| {
            Ok(UpdateSettings {
                auto_download: row.get(0)?,
                notified_version: row.get(1)?,
            })
        },
    )?)
}

pub fn set_auto_download(connection: &Connection, enabled: bool) -> AppResult<()> {
    connection.execute(
        "UPDATE update_settings SET auto_download = ?1 WHERE id = 1",
        params![enabled],
    )?;
    Ok(())
}

pub fn set_notified_version(connection: &Connection, version: &str) -> AppResult<()> {
    connection.execute(
        "UPDATE update_settings SET notified_version = ?1 WHERE id = 1",
        params![version],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::store::settings::{self, SettingsPatch, Theme};

    #[test]
    fn auto_download_is_on_until_turned_off_and_survives_a_reset() {
        let store = Store::open_in_memory().unwrap();
        store
            .blocking(|connection| {
                assert_eq!(get(connection)?, UpdateSettings::default());
                assert!(get(connection)?.auto_download);
                set_auto_download(connection, false)?;
                set_notified_version(connection, "0.9.1")?;
                let reset = SettingsPatch {
                    server_url: Some(String::new()),
                    context_length: Some(0),
                    theme: Some(Theme::System),
                    language: Some(crate::i18n::LanguageChoice::System),
                    ..SettingsPatch::default()
                };
                settings::update(connection, reset, None)?;
                assert_eq!(
                    get(connection)?,
                    UpdateSettings {
                        auto_download: false,
                        notified_version: Some("0.9.1".into()),
                    }
                );
                set_auto_download(connection, true)?;
                assert!(get(connection)?.auto_download);
                assert!(
                    connection
                        .execute("UPDATE update_settings SET auto_download = 2", [])
                        .is_err()
                );
                assert!(
                    connection
                        .execute("INSERT INTO update_settings (id) VALUES (2)", [])
                        .is_err()
                );
                Ok(())
            })
            .unwrap();
    }
}
