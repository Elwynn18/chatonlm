//! Réglages du mode conversation vocale (table `voice_mode_settings`, une
//! seule ligne).

use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppError, AppResult};

/// Longueur maximale de la consigne vocale (caractères).
pub const MAX_INSTRUCTIONS_CHARS: usize = 4000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum VoiceTurnMode {
    #[default]
    HandsFree,
    PushToTalk,
}

impl VoiceTurnMode {
    fn as_str(self) -> &'static str {
        match self {
            VoiceTurnMode::HandsFree => "handsFree",
            VoiceTurnMode::PushToTalk => "pushToTalk",
        }
    }

    fn parse(value: &str) -> AppResult<Self> {
        match value {
            "handsFree" => Ok(VoiceTurnMode::HandsFree),
            "pushToTalk" => Ok(VoiceTurnMode::PushToTalk),
            other => Err(AppError::Internal(format!(
                "unknown voice turn mode in database: {other}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum VoicePause {
    Short,
    #[default]
    Normal,
    Long,
}

impl VoicePause {
    /// Silence qui termine un tour en mains libres.
    pub fn silence(self) -> Duration {
        match self {
            VoicePause::Short => Duration::from_millis(600),
            VoicePause::Normal => Duration::from_millis(1_000),
            VoicePause::Long => Duration::from_millis(1_600),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            VoicePause::Short => "short",
            VoicePause::Normal => "normal",
            VoicePause::Long => "long",
        }
    }

    fn parse(value: &str) -> AppResult<Self> {
        match value {
            "short" => Ok(VoicePause::Short),
            "normal" => Ok(VoicePause::Normal),
            "long" => Ok(VoicePause::Long),
            other => Err(AppError::Internal(format!(
                "unknown voice pause in database: {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceModeSettings {
    pub turn_mode: VoiceTurnMode,
    pub pause: VoicePause,
    /// Parler coupe la réponse (défaut : vrai).
    pub barge_in: bool,
    /// Garder le choix du bouton Think pour les envois vocaux (défaut : faux).
    pub keep_think: bool,
    /// Consigne vocale ; vide = défaut (≤ 4000 caractères).
    pub instructions: String,
}

impl Default for VoiceModeSettings {
    fn default() -> Self {
        Self {
            turn_mode: VoiceTurnMode::HandsFree,
            pause: VoicePause::Normal,
            barge_in: true,
            keep_think: false,
            instructions: String::new(),
        }
    }
}

impl VoiceModeSettings {
    /// Consigne envoyée : la consigne réglée, sinon celle par défaut.
    pub fn effective_instructions(&self) -> &str {
        if self.instructions.trim().is_empty() {
            crate::voice_mode::DEFAULT_INSTRUCTIONS
        } else {
            &self.instructions
        }
    }
}

/// Mise à jour partielle : seuls les champs présents changent.
#[derive(Clone, Debug, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct VoiceModeSettingsPatch {
    #[ts(optional)]
    pub turn_mode: Option<VoiceTurnMode>,
    #[ts(optional)]
    pub pause: Option<VoicePause>,
    #[ts(optional)]
    pub barge_in: Option<bool>,
    #[ts(optional)]
    pub keep_think: Option<bool>,
    #[ts(optional)]
    pub instructions: Option<String>,
}

pub fn get(connection: &Connection) -> AppResult<VoiceModeSettings> {
    let row = connection
        .query_row(
            "SELECT turn_mode, pause, barge_in, keep_think, instructions
             FROM voice_mode_settings WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::Internal("voice mode settings row missing".into()))?;
    let (turn_mode, pause, barge_in, keep_think, instructions) = row;
    Ok(VoiceModeSettings {
        turn_mode: VoiceTurnMode::parse(&turn_mode)?,
        pause: VoicePause::parse(&pause)?,
        barge_in,
        keep_think,
        instructions,
    })
}

/// Valide puis applique `patch` en une transaction ; renvoie les réglages à
/// jour. Une consigne vide, ou identique à celle par défaut, est enregistrée
/// vide (défaut).
pub fn update(
    connection: &mut Connection,
    patch: VoiceModeSettingsPatch,
) -> AppResult<VoiceModeSettings> {
    if let Some(instructions) = &patch.instructions
        && instructions.chars().count() > MAX_INSTRUCTIONS_CHARS
    {
        // i18n-ignore : la page limite déjà la consigne.
        return Err(AppError::Invalid(format!(
            "the instructions hold at most {MAX_INSTRUCTIONS_CHARS} characters"
        )));
    }
    let transaction = connection.transaction()?;
    let set = |column: &str, value: &dyn rusqlite::ToSql| -> AppResult<()> {
        transaction.execute(
            &format!("UPDATE voice_mode_settings SET {column} = ?1 WHERE id = 1"),
            params![value],
        )?;
        Ok(())
    };
    if let Some(mode) = patch.turn_mode {
        set("turn_mode", &mode.as_str())?;
    }
    if let Some(pause) = patch.pause {
        set("pause", &pause.as_str())?;
    }
    if let Some(barge_in) = patch.barge_in {
        set("barge_in", &barge_in)?;
    }
    if let Some(keep_think) = patch.keep_think {
        set("keep_think", &keep_think)?;
    }
    if let Some(instructions) = &patch.instructions {
        let instructions = instructions.trim();
        let stored = if instructions == crate::voice_mode::DEFAULT_INSTRUCTIONS {
            ""
        } else {
            instructions
        };
        set("instructions", &stored)?;
    }
    transaction.commit()?;
    get(connection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn defaults_then_partial_update() {
        let store = store();
        store
            .blocking(|connection| {
                let settings = get(connection)?;
                assert_eq!(settings, VoiceModeSettings::default());
                assert_eq!(settings.turn_mode, VoiceTurnMode::HandsFree);
                assert_eq!(settings.pause, VoicePause::Normal);
                assert!(settings.barge_in && !settings.keep_think);
                assert_eq!(settings.instructions, "");
                assert_eq!(
                    settings.effective_instructions(),
                    crate::voice_mode::DEFAULT_INSTRUCTIONS
                );

                let updated = update(
                    connection,
                    VoiceModeSettingsPatch {
                        turn_mode: Some(VoiceTurnMode::PushToTalk),
                        pause: Some(VoicePause::Long),
                        barge_in: Some(false),
                        ..Default::default()
                    },
                )?;
                assert_eq!(updated.turn_mode, VoiceTurnMode::PushToTalk);
                assert_eq!(updated.pause, VoicePause::Long);
                assert!(!updated.barge_in);
                // Les autres réglages ne bougent pas.
                assert!(!updated.keep_think);
                let updated = update(
                    connection,
                    VoiceModeSettingsPatch {
                        keep_think: Some(true),
                        instructions: Some("  Answer like a pirate.  ".into()),
                        ..Default::default()
                    },
                )?;
                assert!(updated.keep_think);
                assert_eq!(updated.turn_mode, VoiceTurnMode::PushToTalk);
                assert_eq!(updated.instructions, "Answer like a pirate.");
                assert_eq!(updated.effective_instructions(), "Answer like a pirate.");
                assert_eq!(get(connection)?, updated);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn the_default_instructions_are_stored_as_empty() {
        let store = store();
        store
            .blocking(|connection| {
                let patch = VoiceModeSettingsPatch {
                    instructions: Some(format!("{}\n", crate::voice_mode::DEFAULT_INSTRUCTIONS)),
                    ..Default::default()
                };
                assert_eq!(update(connection, patch)?.instructions, "");
                let blank = VoiceModeSettingsPatch {
                    instructions: Some("   ".into()),
                    ..Default::default()
                };
                assert_eq!(update(connection, blank)?.instructions, "");
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn instructions_are_limited() {
        let store = store();
        store
            .blocking(|connection| {
                let longest = "é".repeat(MAX_INSTRUCTIONS_CHARS);
                let patch = VoiceModeSettingsPatch {
                    instructions: Some(longest.clone()),
                    ..Default::default()
                };
                assert_eq!(update(connection, patch)?.instructions, longest);
                let patch = VoiceModeSettingsPatch {
                    instructions: Some(format!("{longest}x")),
                    barge_in: Some(false),
                    ..Default::default()
                };
                assert!(matches!(
                    update(connection, patch),
                    Err(AppError::Invalid(_))
                ));
                // Rien n'a changé.
                let settings = get(connection)?;
                assert!(settings.barge_in);
                assert_eq!(settings.instructions, longest);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn impossible_values_are_refused_by_the_database() {
        let store = store();
        for refused in [
            "UPDATE voice_mode_settings SET turn_mode = 'toggle'",
            "UPDATE voice_mode_settings SET pause = 'none'",
            "UPDATE voice_mode_settings SET barge_in = 2",
            "INSERT INTO voice_mode_settings (id) VALUES (2)",
        ] {
            assert!(
                store.blocking(|c| Ok(c.execute(refused, [])?)).is_err(),
                "{refused}"
            );
        }
    }

    #[test]
    fn the_patch_refuses_unknown_fields() {
        let parsed: Result<VoiceModeSettingsPatch, _> =
            serde_json::from_str(r#"{"turnMode":"pushToTalk","pause":"short","bargeIn":true}"#);
        let patch = parsed.unwrap();
        assert_eq!(patch.turn_mode, Some(VoiceTurnMode::PushToTalk));
        assert_eq!(patch.pause, Some(VoicePause::Short));
        assert!(serde_json::from_str::<VoiceModeSettingsPatch>(r#"{"other":1}"#).is_err());
    }

    #[test]
    fn pauses_end_a_turn_after_their_silence() {
        assert_eq!(VoicePause::Short.silence(), Duration::from_millis(600));
        assert_eq!(VoicePause::Normal.silence(), Duration::from_secs(1));
        assert_eq!(VoicePause::Long.silence(), Duration::from_millis(1_600));
    }
}
