//! Signaler un bug et journal de l'app (Settings › General, propre à
//! ChatonLM) : voir `bug_report` et `logging`.

use tauri::{AppHandle, Manager};

use crate::bug_report::{self, BugReport, Facts, ReportOutcome};
use crate::engine::Engine;
use crate::error::AppResult;
use crate::{i18n, logging, updater};

/// Faits du diagnostic, lus sans rien démarrer (le moteur n'est pas lancé :
/// son état est celui de la session).
async fn facts(app: &AppHandle) -> Facts {
    let versions = updater::versions(app);
    let install = match updater::status(app)
        .ok()
        .and_then(|status| status.installer)
    {
        Some(installer) => format!("{installer:?}"),
        None => "other (no updater)".to_owned(),
    };
    let engine = match app.try_state::<Engine>() {
        Some(engine) => bug_report::engine_line(&super::engine::current(&engine).await),
        None => "unknown".to_owned(),
    };
    Facts {
        version: versions.chatonlm,
        platform: bug_report::platform_name(),
        os: bug_report::os_description(),
        arch: std::env::consts::ARCH,
        desktop: bug_report::desktop(),
        install,
        language: i18n::current().code().to_owned(),
        engine,
        log: logging::path()
            .map(|path| logging::tail(&path, bug_report::LOG_TAIL))
            .unwrap_or_default(),
    }
}

/// Diagnostic tel qu'il serait joint (montré avant l'envoi).
#[tauri::command]
pub async fn bug_report_diagnostic(app: AppHandle) -> AppResult<String> {
    Ok(bug_report::diagnostic(&facts(&app).await))
}

/// Envoie le signalement. Le diagnostic est relu au moment de l'envoi.
#[tauri::command]
pub async fn send_bug_report(app: AppHandle, report: BugReport) -> AppResult<ReportOutcome> {
    let length = report.message.trim().chars().count();
    if !(bug_report::MESSAGE_MIN..=bug_report::MESSAGE_MAX).contains(&length) {
        return Ok(ReportOutcome::InvalidMessage);
    }
    let facts = facts(&app).await;
    let body = bug_report::payload(&report, &facts, i18n::current().code());
    Ok(bug_report::send(&body).await)
}

/// Ouvre le dossier des journaux (celui de l'app et ceux du moteur).
#[tauri::command]
pub async fn open_app_logs(app: AppHandle) -> AppResult<()> {
    let dir = crate::paths::data_dir(&app)?.join("logs");
    std::fs::create_dir_all(&dir)?;
    super::system::open_path(&dir).await
}
