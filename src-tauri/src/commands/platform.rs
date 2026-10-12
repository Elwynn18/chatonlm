//! Système sur lequel tourne l'app, pour la page : elle pose `data-platform`
//! sur `<html>` au démarrage et adapte ce qui diffère d'un système à l'autre
//! (textes des raccourcis, écrans propres à un système).

use serde::Serialize;
use ts_rs::TS;

use crate::error::AppResult;

/// Système de l'app. Le type est le même sur les deux systèmes : les
/// liaisons TypeScript générées ne dépendent pas de la machine qui les écrit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Platform {
    Linux,
    Windows,
}

impl Platform {
    /// Système pour lequel l'app est compilée.
    pub const fn current() -> Self {
        if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }
}

/// Système de l'app (`"linux"` ou `"windows"`).
#[tauri::command]
pub async fn app_platform() -> AppResult<Platform> {
    Ok(Platform::current())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_platform_is_the_one_the_app_is_built_for() {
        let expected = if cfg!(windows) { "windows" } else { "linux" };
        assert_eq!(
            serde_json::to_value(Platform::current()).unwrap(),
            serde_json::json!(expected)
        );
    }

    #[test]
    fn both_platforms_have_a_lowercase_name() {
        assert_eq!(
            serde_json::to_value([Platform::Linux, Platform::Windows]).unwrap(),
            serde_json::json!(["linux", "windows"])
        );
    }
}
