//! Signaler un bug et journal de l'app (Settings › General, propre à
//! ChatonLM) : voir `bug_report` et `logging`.

use tauri::{AppHandle, State};

use crate::bug_report::{self, BugReport, Facts, ReportOutcome};
use crate::error::AppResult;
use crate::ollama::builtin::BuiltinState;
use crate::ollama::target::{self, TargetKind};
use crate::store::Store;
use crate::{i18n, logging, updater};

/// Faits du diagnostic, lus sans rien démarrer (pas de vérification du
/// serveur : `runtime` décrit ce qui sert déjà).
async fn facts(app: &AppHandle, store: &Store) -> Facts {
    let versions = updater::versions(app);
    let install = match updater::status(app)
        .ok()
        .and_then(|status| status.installer)
    {
        Some(installer) => format!("{installer:?}"),
        None => "other (no updater)".to_owned(),
    };
    let (ollama, builtin_ollama) = match target::current() {
        Some(targeting) => match targeting.runtime(store).await {
            Ok(runtime) => {
                let served = runtime
                    .served_version
                    .as_deref()
                    .unwrap_or("not responding");
                let ollama = match (runtime.kind, runtime.local) {
                    (TargetKind::Builtin, _) => format!("built-in, {served}"),
                    (TargetKind::Machine, true) => format!("this computer, {served}"),
                    (TargetKind::Machine, false) => format!("another computer, {served}"),
                };
                let builtin = runtime.builtin_available.then(|| {
                    let state = match runtime.builtin_state {
                        BuiltinState::Stopped => "stopped",
                        BuiltinState::Starting => "starting",
                        BuiltinState::Running => "running",
                        BuiltinState::Failed => "failed",
                    };
                    let version = versions.ollama.as_deref().unwrap_or("?");
                    format!("{version}, {state}")
                });
                (ollama, builtin)
            }
            Err(_) => ("unknown".to_owned(), None),
        },
        None => ("unknown".to_owned(), None),
    };
    Facts {
        version: versions.chatonlm,
        platform: bug_report::platform_name(),
        os: bug_report::os_description(),
        arch: std::env::consts::ARCH,
        desktop: bug_report::desktop(),
        install,
        language: i18n::current().code().to_owned(),
        ollama,
        builtin_ollama,
        log: logging::path()
            .map(|path| logging::tail(&path, bug_report::LOG_TAIL))
            .unwrap_or_default(),
    }
}

/// Diagnostic tel qu'il serait joint (montré avant l'envoi).
#[tauri::command]
pub async fn bug_report_diagnostic(app: AppHandle, store: State<'_, Store>) -> AppResult<String> {
    Ok(bug_report::diagnostic(&facts(&app, &store).await))
}

/// Envoie le signalement. Le diagnostic est relu au moment de l'envoi.
#[tauri::command]
pub async fn send_bug_report(
    app: AppHandle,
    store: State<'_, Store>,
    report: BugReport,
) -> AppResult<ReportOutcome> {
    let length = report.message.trim().chars().count();
    if !(bug_report::MESSAGE_MIN..=bug_report::MESSAGE_MAX).contains(&length) {
        return Ok(ReportOutcome::InvalidMessage);
    }
    let facts = facts(&app, &store).await;
    let body = bug_report::payload(&report, &facts, i18n::current().code());
    Ok(bug_report::send(&body).await)
}

/// Ouvre le dossier des journaux (celui de l'app et de l'Ollama intégré).
#[tauri::command]
pub async fn open_app_logs(app: AppHandle) -> AppResult<()> {
    let dir = crate::paths::data_dir(&app)?.join("logs");
    std::fs::create_dir_all(&dir)?;
    super::system::open_path(&dir).await
}
