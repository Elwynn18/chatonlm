//! Historique de l'outil Transcription : table `transcriptions` (migrations
//! 0046, puis 0047 pour l'avertissement). Passages horodatés en JSON (`transcription::paragraphs::Segment`),
//! texte en paragraphes à côté (export, aperçu, synthèse). Le message d'une
//! erreur enregistrée est refait dans la langue du moment, d'après son code.

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::now_ms;
use crate::dictation::models::SpeechEngine;
use crate::error::{AppError, AppResult};
use crate::transcription::paragraphs::{Segment, plain_text};
use crate::transcription::ytdlp::VideoInfo;
use crate::transcription::{
    SourceKind, Synthesis, TranscriptionDetail, TranscriptionError, TranscriptionErrorCode,
    TranscriptionItem, TranscriptionState, TranscriptionWarning, TranscriptionWarningCode,
};

/// Longueur de l'aperçu de la liste (caractères).
const PREVIEW_CHARS: usize = 200;

/// Longueur maximale d'un titre (caractères).
pub const MAX_TITLE_CHARS: usize = 300;

/// Transcription qui commence.
#[derive(Clone, Debug)]
pub struct NewTranscription {
    pub id: String,
    pub source: SourceKind,
    pub origin: String,
    pub title: String,
    pub engine: SpeechEngine,
    pub model: String,
    pub language: String,
}

fn source_text(source: SourceKind) -> &'static str {
    match source {
        SourceKind::File => "file",
        SourceKind::Url => "url",
    }
}

fn source_of(text: &str) -> AppResult<SourceKind> {
    Ok(match text {
        "file" => SourceKind::File,
        "url" => SourceKind::Url,
        other => return Err(unknown("source", other)),
    })
}

fn engine_text(engine: SpeechEngine) -> &'static str {
    match engine {
        SpeechEngine::Parakeet => "parakeet",
        SpeechEngine::Whisper => "whisper",
    }
}

fn engine_of(text: &str) -> AppResult<SpeechEngine> {
    Ok(match text {
        "parakeet" => SpeechEngine::Parakeet,
        "whisper" => SpeechEngine::Whisper,
        other => return Err(unknown("engine", other)),
    })
}

fn state_text(state: TranscriptionState) -> &'static str {
    match state {
        TranscriptionState::Running => "running",
        TranscriptionState::Done => "done",
        TranscriptionState::Failed => "failed",
        TranscriptionState::Cancelled => "cancelled",
        TranscriptionState::Interrupted => "interrupted",
    }
}

fn state_of(text: &str) -> AppResult<TranscriptionState> {
    Ok(match text {
        "running" => TranscriptionState::Running,
        "done" => TranscriptionState::Done,
        "failed" => TranscriptionState::Failed,
        "cancelled" => TranscriptionState::Cancelled,
        "interrupted" => TranscriptionState::Interrupted,
        other => return Err(unknown("status", other)),
    })
}

fn unknown(what: &str, value: &str) -> AppError {
    AppError::Internal(format!("unknown transcription {what} in database: {value}"))
}

pub fn insert(connection: &Connection, new: &NewTranscription) -> AppResult<()> {
    let now = now_ms();
    connection.execute(
        "INSERT INTO transcriptions
             (id, source, origin, title, engine, model, language, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running', ?8, ?8)",
        params![
            new.id,
            source_text(new.source),
            new.origin,
            new.title,
            engine_text(new.engine),
            new.model,
            new.language,
            now
        ],
    )?;
    Ok(())
}

/// Fiche de la vidéo (yt-dlp) : titre (sauf si l'utilisateur a déjà renommé
/// la transcription pendant la récupération), chaîne, adresse, site, durée.
pub fn set_info(connection: &Connection, id: &str, info: &VideoInfo) -> AppResult<()> {
    let duration = info
        .duration_s
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(|seconds| (seconds * 1000.0).round() as i64);
    connection.execute(
        "UPDATE transcriptions SET title = CASE WHEN title = '' THEN ?2 ELSE title END,
             channel = ?3, url = ?4, site = ?5,
             duration_ms = coalesce(?6, duration_ms), updated_at = ?7
         WHERE id = ?1",
        params![
            id,
            clip(info.title.as_deref().unwrap_or_default()),
            clip(info.channel.as_deref().unwrap_or_default()),
            info.webpage_url,
            info.site,
            duration,
            now_ms()
        ],
    )?;
    Ok(())
}

pub fn set_duration(connection: &Connection, id: &str, duration_ms: i64) -> AppResult<()> {
    connection.execute(
        "UPDATE transcriptions SET duration_ms = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, duration_ms.max(0), now_ms()],
    )?;
    Ok(())
}

pub fn set_language(connection: &Connection, id: &str, language: &str) -> AppResult<()> {
    connection.execute(
        "UPDATE transcriptions SET language = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, language, now_ms()],
    )?;
    Ok(())
}

/// Passages faits jusqu'ici (et le texte qui en découle).
pub fn set_segments(connection: &Connection, id: &str, segments: &[Segment]) -> AppResult<()> {
    let json =
        serde_json::to_string(segments).map_err(|error| AppError::Internal(error.to_string()))?;
    connection.execute(
        "UPDATE transcriptions SET segments = ?2, text = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, json, plain_text(segments).trim_end(), now_ms()],
    )?;
    Ok(())
}

/// Fin de la transcription.
pub fn finish(
    connection: &Connection,
    id: &str,
    state: TranscriptionState,
    error: Option<TranscriptionErrorCode>,
) -> AppResult<()> {
    let now = now_ms();
    connection.execute(
        "UPDATE transcriptions SET status = ?2, error = ?3, updated_at = ?4, finished_at = ?4
         WHERE id = ?1",
        params![id, state_text(state), error.map(|code| code.as_str()), now],
    )?;
    Ok(())
}

/// Avertissement d'une transcription finie (`None` : aucun).
pub fn set_warning(
    connection: &Connection,
    id: &str,
    warning: Option<TranscriptionWarningCode>,
) -> AppResult<()> {
    connection.execute(
        "UPDATE transcriptions SET warning = ?2 WHERE id = ?1",
        params![id, warning.map(|code| code.as_str())],
    )?;
    Ok(())
}

/// Synthèse (« Synthétiser ») enregistrée, remplace la précédente ; vrai si
/// la transcription existait.
pub fn set_summary(
    connection: &Connection,
    id: &str,
    markdown: &str,
    model: &str,
) -> AppResult<bool> {
    let now = now_ms();
    let changed = connection.execute(
        "UPDATE transcriptions SET summary = ?2, summary_model = ?3, summary_at = ?4,
             updated_at = ?4
         WHERE id = ?1",
        params![id, markdown, model, now],
    )?;
    Ok(changed > 0)
}

/// Vrai si la transcription existait.
pub fn delete(connection: &Connection, id: &str) -> AppResult<bool> {
    Ok(connection.execute("DELETE FROM transcriptions WHERE id = ?1", [id])? > 0)
}

/// Nouveau titre (espaces retirés aux bords, borné) ; vrai si elle existait.
pub fn rename(connection: &Connection, id: &str, title: &str) -> AppResult<bool> {
    let changed = connection.execute(
        "UPDATE transcriptions SET title = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, clip(title.trim()), now_ms()],
    )?;
    Ok(changed > 0)
}

/// Lancement : une transcription restée « en cours » (app coupée) devient
/// interrompue si elle a des passages, sinon elle est retirée.
pub fn recover(connection: &Connection) -> AppResult<()> {
    let now = now_ms();
    connection.execute(
        "DELETE FROM transcriptions WHERE status = 'running' AND segments = '[]'",
        [],
    )?;
    connection.execute(
        "UPDATE transcriptions SET status = 'interrupted', updated_at = ?1, finished_at = ?1
         WHERE status = 'running'",
        [now],
    )?;
    Ok(())
}

fn clip(text: &str) -> String {
    text.chars().take(MAX_TITLE_CHARS).collect()
}

const ITEM_COLUMNS: &str = "id, source, origin, title, channel, url, site, duration_ms, engine,
    model, language, status, error, substr(text, 1, 400), summary IS NOT NULL,
    created_at, updated_at, finished_at, warning";

/// Nombre de colonnes de [`ITEM_COLUMNS`] (celles du détail suivent).
const ITEM_COUNT: usize = 19;

fn item(row: &Row) -> rusqlite::Result<RawItem> {
    Ok(RawItem {
        id: row.get(0)?,
        source: row.get(1)?,
        origin: row.get(2)?,
        title: row.get(3)?,
        channel: row.get(4)?,
        url: row.get(5)?,
        site: row.get(6)?,
        duration_ms: row.get(7)?,
        engine: row.get(8)?,
        model: row.get(9)?,
        language: row.get(10)?,
        status: row.get(11)?,
        error: row.get(12)?,
        preview: row.get(13)?,
        has_summary: row.get(14)?,
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
        finished_at: row.get(17)?,
        warning: row.get(18)?,
    })
}

struct RawItem {
    id: String,
    source: String,
    origin: String,
    title: String,
    channel: String,
    url: String,
    site: String,
    duration_ms: Option<i64>,
    engine: String,
    model: String,
    language: String,
    status: String,
    error: Option<String>,
    preview: String,
    has_summary: bool,
    created_at: i64,
    updated_at: i64,
    finished_at: Option<i64>,
    warning: Option<String>,
}

impl RawItem {
    fn into_item(self) -> AppResult<TranscriptionItem> {
        let error = self
            .error
            .as_deref()
            .map(|code| {
                TranscriptionErrorCode::parse(code)
                    .map(TranscriptionError::of)
                    .ok_or_else(|| unknown("error", code))
            })
            .transpose()?;
        let warning = self
            .warning
            .as_deref()
            .map(|code| {
                TranscriptionWarningCode::parse(code)
                    .map(TranscriptionWarning::of)
                    .ok_or_else(|| unknown("warning", code))
            })
            .transpose()?;
        // Paragraphes à la suite, sur une ligne.
        let preview = self.preview.replace("\n\n", " ");
        let preview: String = preview.chars().take(PREVIEW_CHARS).collect();
        Ok(TranscriptionItem {
            id: self.id,
            source: source_of(&self.source)?,
            origin: self.origin,
            title: self.title,
            channel: self.channel,
            url: self.url,
            site: self.site,
            duration_s: self.duration_ms.map(|ms| ms as f64 / 1000.0),
            engine: engine_of(&self.engine)?,
            model: self.model,
            language: (!self.language.is_empty()).then_some(self.language),
            state: state_of(&self.status)?,
            error,
            warning,
            preview,
            has_summary: self.has_summary,
            created_at: self.created_at,
            updated_at: self.updated_at,
            finished_at: self.finished_at,
        })
    }
}

/// Toutes les transcriptions, les plus récentes d'abord.
pub fn list(connection: &Connection) -> AppResult<Vec<TranscriptionItem>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {ITEM_COLUMNS} FROM transcriptions ORDER BY created_at DESC, id DESC"
    ))?;
    let raw = statement
        .query_map([], item)?
        .collect::<Result<Vec<_>, _>>()?;
    raw.into_iter().map(RawItem::into_item).collect()
}

pub fn get(connection: &Connection, id: &str) -> AppResult<Option<TranscriptionDetail>> {
    let found = connection
        .query_row(
            &format!(
                "SELECT {ITEM_COLUMNS}, segments, text, summary, summary_model, summary_at
                 FROM transcriptions WHERE id = ?1"
            ),
            [id],
            |row| {
                Ok((
                    item(row)?,
                    row.get::<_, String>(ITEM_COUNT)?,
                    row.get::<_, String>(ITEM_COUNT + 1)?,
                    row.get::<_, Option<String>>(ITEM_COUNT + 2)?,
                    row.get::<_, String>(ITEM_COUNT + 3)?,
                    row.get::<_, Option<i64>>(ITEM_COUNT + 4)?,
                ))
            },
        )
        .optional()?;
    let Some((raw, segments, text, summary, summary_model, summary_at)) = found else {
        return Ok(None);
    };
    let segments: Vec<Segment> = serde_json::from_str(&segments)
        .map_err(|error| AppError::Internal(format!("transcription segments: {error}")))?;
    Ok(Some(TranscriptionDetail {
        item: raw.into_item()?,
        segments,
        text,
        summary: summary.map(|markdown| Synthesis {
            markdown,
            model: summary_model,
            at: summary_at.unwrap_or_default(),
        }),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{MIGRATIONS, Store, database_at};

    /// Version de schéma de ChatonLM 0.9.5 (38 migrations au tag v0.9.5).
    const SCHEMA_0_9_5: usize = 38;

    fn new(id: &str) -> NewTranscription {
        NewTranscription {
            id: id.into(),
            source: SourceKind::Url,
            origin: "https://example.org/v/1".into(),
            title: String::new(),
            engine: SpeechEngine::Whisper,
            model: "whisper-base".into(),
            language: String::new(),
        }
    }

    fn segment(start: f64, text: &str) -> Segment {
        Segment {
            start,
            end: start + 1.0,
            text: text.into(),
        }
    }

    /// Base d'une 0.9.5 remplie : ouverte par cette version, la table est
    /// créée et sert aussitôt, rien d'autre ne bouge.
    #[test]
    fn a_0_9_5_database_gains_the_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatonlm.sqlite");
        let connection = database_at(&path, SCHEMA_0_9_5);
        connection
            .execute_batch(
                "INSERT INTO chats (id, title, created_at, updated_at)
                     VALUES ('0199-a', 'Recettes', 1, 3);",
            )
            .unwrap();
        drop(connection);
        let store = Store::open(&path).unwrap();
        store
            .blocking(|c| {
                let version: i64 = c.pragma_query_value(None, "user_version", |r| r.get(0))?;
                assert_eq!(version, MIGRATIONS.len() as i64);
                let chats: i64 = c.query_row("SELECT count(*) FROM chats", [], |r| r.get(0))?;
                assert_eq!(chats, 1);
                assert!(list(c)?.is_empty());
                insert(c, &new("t1"))?;
                assert_eq!(list(c)?.len(), 1);
                Ok(())
            })
            .unwrap();
    }

    /// Base neuve : tout le cycle d'une transcription, relu tel quel.
    #[test]
    fn a_transcription_is_recorded_listed_renamed_and_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("chatonlm.sqlite")).unwrap();
        store
            .blocking(|c| {
                insert(c, &new("0199-1"))?;
                let mut other = new("0199-2");
                other.source = SourceKind::File;
                other.origin = "/home/me/cours.mp3".into();
                other.title = "cours.mp3".into();
                other.engine = SpeechEngine::Parakeet;
                other.model = "parakeet-tdt-0.6b-v3".into();
                other.language = "fr".into();
                insert(c, &other)?;
                set_info(
                    c,
                    "0199-1",
                    &VideoInfo {
                        title: Some("Leçon 3".into()),
                        channel: Some("École".into()),
                        duration_s: Some(721.4),
                        webpage_url: "https://example.org/watch?v=1".into(),
                        site: "example".into(),
                    },
                )?;
                set_language(c, "0199-1", "en")?;
                let segments = vec![segment(0.0, "Hello."), segment(5.0, "World.")];
                set_segments(c, "0199-1", &segments)?;
                set_duration(c, "0199-1", 721_000)?;
                finish(c, "0199-1", TranscriptionState::Done, None)?;
                finish(
                    c,
                    "0199-2",
                    TranscriptionState::Failed,
                    Some(TranscriptionErrorCode::Failed),
                )?;

                let items = list(c)?;
                assert_eq!(
                    items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
                    ["0199-2", "0199-1"]
                );
                let video = &items[1];
                assert_eq!(video.title, "Leçon 3");
                assert_eq!(video.channel, "École");
                assert_eq!(video.url, "https://example.org/watch?v=1");
                assert_eq!(video.duration_s, Some(721.0));
                assert_eq!(video.language.as_deref(), Some("en"));
                assert_eq!(video.state, TranscriptionState::Done);
                assert_eq!(video.preview, "Hello. World.");
                assert!(video.finished_at.is_some());
                assert!(!video.has_summary);
                let file = &items[0];
                assert_eq!(file.source, SourceKind::File);
                assert_eq!(file.engine, SpeechEngine::Parakeet);
                assert_eq!(
                    file.error.as_ref().map(|e| e.code),
                    Some(TranscriptionErrorCode::Failed)
                );

                let detail = get(c, "0199-1")?.unwrap();
                assert_eq!(detail.segments, segments);
                // 4 s de pause : deux paragraphes.
                assert_eq!(detail.text, "Hello.\n\nWorld.");
                assert_eq!(detail.summary, None);
                assert!(get(c, "nope")?.is_none());

                // Renommée pendant la récupération : la fiche de yt-dlp, qui
                // arrive ensuite, ne remet pas le titre de la vidéo.
                insert(c, &new("0199-3"))?;
                assert!(rename(c, "0199-3", "Mon cours")?);
                let info = VideoInfo {
                    title: Some("Leçon 4".into()),
                    channel: Some("École".into()),
                    duration_s: Some(60.0),
                    webpage_url: "https://example.org/watch?v=3".into(),
                    site: "example".into(),
                };
                set_info(c, "0199-3", &info)?;
                let renamed = get(c, "0199-3")?.unwrap().item;
                assert_eq!(renamed.title, "Mon cours");
                assert_eq!(renamed.channel, "École");
                assert!(delete(c, "0199-3")?);
                // Avertissement : enregistré, relu, retiré.
                assert_eq!(get(c, "0199-1")?.unwrap().item.warning, None);
                set_warning(c, "0199-1", Some(TranscriptionWarningCode::Partial))?;
                let warned = get(c, "0199-1")?.unwrap().item.warning.unwrap();
                assert_eq!(warned.code, TranscriptionWarningCode::Partial);
                assert!(!warned.message.starts_with("transcription."));
                set_warning(c, "0199-1", None)?;
                assert_eq!(list(c)?[1].warning, None);

                assert!(rename(c, "0199-1", "  Nouveau nom  ")?);
                assert_eq!(get(c, "0199-1")?.unwrap().item.title, "Nouveau nom");
                assert!(!rename(c, "nope", "x")?);
                let long = "x".repeat(MAX_TITLE_CHARS + 50);
                rename(c, "0199-1", &long)?;
                assert_eq!(
                    get(c, "0199-1")?.unwrap().item.title.chars().count(),
                    MAX_TITLE_CHARS
                );

                assert!(delete(c, "0199-2")?);
                assert!(!delete(c, "0199-2")?);
                assert_eq!(list(c)?.len(), 1);
                Ok(())
            })
            .unwrap();
    }

    /// Synthèse enregistrée, relue par la liste et le détail, remplacée par
    /// la suivante ; sans effet sur une transcription inconnue.
    #[test]
    fn a_summary_is_recorded_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("chatonlm.sqlite")).unwrap();
        store
            .blocking(|c| {
                insert(c, &new("s1"))?;
                set_segments(c, "s1", &[segment(0.0, "Bonjour.")])?;
                assert!(set_summary(
                    c,
                    "s1",
                    "# Titre\n\nTexte.",
                    "a/M-GGUF:Q4_K_M"
                )?);
                assert!(list(c)?[0].has_summary);
                let summary = get(c, "s1")?.unwrap().summary.unwrap();
                assert_eq!(summary.markdown, "# Titre\n\nTexte.");
                assert_eq!(summary.model, "a/M-GGUF:Q4_K_M");
                assert!(summary.at > 0);
                assert!(set_summary(c, "s1", "# Autre", "b/N-GGUF:Q8_0")?);
                let summary = get(c, "s1")?.unwrap().summary.unwrap();
                assert_eq!(
                    (summary.markdown.as_str(), summary.model.as_str()),
                    ("# Autre", "b/N-GGUF:Q8_0")
                );
                // Le texte et les passages ne bougent pas.
                assert_eq!(get(c, "s1")?.unwrap().text, "Bonjour.");
                assert!(!set_summary(c, "nope", "x", "m")?);
                Ok(())
            })
            .unwrap();
    }

    /// App coupée pendant une transcription : celle qui a des passages est
    /// interrompue, celle qui n'en a pas disparaît, les finies ne bougent pas.
    #[test]
    fn recovery_keeps_only_what_was_transcribed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("chatonlm.sqlite")).unwrap();
        store
            .blocking(|c| {
                for id in ["a", "b", "c"] {
                    insert(c, &new(id))?;
                }
                set_segments(c, "a", &[segment(0.0, "Début.")])?;
                set_segments(c, "c", &[segment(0.0, "Fini.")])?;
                finish(c, "c", TranscriptionState::Done, None)?;
                recover(c)?;
                let items = list(c)?;
                let states: Vec<(&str, TranscriptionState)> =
                    items.iter().map(|i| (i.id.as_str(), i.state)).collect();
                assert_eq!(
                    states,
                    [
                        ("c", TranscriptionState::Done),
                        ("a", TranscriptionState::Interrupted)
                    ]
                );
                Ok(())
            })
            .unwrap();
    }

    /// Chaque code d'erreur s'enregistre et se relit (nom de la
    /// sérialisation).
    #[test]
    fn error_codes_round_trip() {
        for code in TranscriptionErrorCode::ALL {
            assert_eq!(TranscriptionErrorCode::parse(code.as_str()), Some(code));
            let serialized = serde_json::to_value(code).unwrap();
            assert_eq!(serialized, serde_json::Value::from(code.as_str()));
        }
        assert_eq!(TranscriptionErrorCode::parse("nope"), None);
        for code in TranscriptionWarningCode::ALL {
            assert_eq!(TranscriptionWarningCode::parse(code.as_str()), Some(code));
            let serialized = serde_json::to_value(code).unwrap();
            assert_eq!(serialized, serde_json::Value::from(code.as_str()));
        }
        assert_eq!(TranscriptionWarningCode::parse("nope"), None);
    }
}
