//! Signaler un bug (propre à ChatonLM, Settings › General) : le message de
//! l'utilisateur part par le service du site (`POST https://chatonlm.com/api/report`,
//! `site/server/service.mjs`), qui le relaie par courriel à l'équipe, comme le
//! formulaire de contact du site.
//!
//! Rien ne part sans un clic sur « Send ». Le diagnostic (versions, système,
//! état du moteur de texte, fin du journal de l'app ; jamais le journal du
//! moteur, qui peut citer des messages) n'est joint que si l'utilisateur
//! le laisse coché, après l'avoir vu ([`diagnostic`] donne le même texte à la
//! page) ; son dossier personnel et son nom de compte y sont masqués.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
use linux as os;
#[cfg(windows)]
use windows as os;

use std::time::Duration;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::commands::engine::{EngineBackend, EngineStatus};
use crate::commands::platform::Platform;

/// Adresse du service, sauf essais (`CHATONLM_REPORT_URL`).
const REPORT_URL: &str = "https://chatonlm.com/api/report";

/// Fin du journal jointe au diagnostic, en octets.
pub const LOG_TAIL: u64 = 12 * 1024;

/// Bornes du service (`LIMITS` de `service.mjs`) : vérifiées aussi par la
/// page, avant l'envoi.
pub const MESSAGE_MIN: usize = 10;
pub const MESSAGE_MAX: usize = 5000;
/// Diagnostic accepté par le service, en caractères.
// Un peu sous 20 000 : le service compte en unités UTF-16.
const DIAGNOSTIC_MAX: usize = 19_000;

const TIMEOUT: Duration = Duration::from_secs(30);

/// Ce que l'utilisateur envoie.
#[derive(Clone, Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BugReport {
    pub message: String,
    /// Adresse pour lui répondre ; vide : aucune.
    pub email: String,
    /// Joindre le diagnostic.
    pub include_diagnostic: bool,
}

/// Suite d'un envoi. La page en tire son texte (elle ne lit jamais celui du
/// service).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ReportOutcome {
    Sent,
    /// Trop d'envois depuis cette adresse : réessayer plus tard.
    TooMany,
    /// Adresse de réponse refusée par le service.
    InvalidEmail,
    /// Message trop court ou trop long.
    InvalidMessage,
    /// Service injoignable ou en panne (réseau coupé, messagerie du site
    /// arrêtée…).
    Unavailable,
}

/// Faits sur la machine et l'app, réunis par la commande.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub version: String,
    pub platform: &'static str,
    pub os: String,
    pub arch: &'static str,
    pub desktop: Option<String>,
    /// AppImage, Deb, Nsis, ou autre.
    pub install: String,
    pub language: String,
    /// Moteur de texte (`llama-server`), voir [`engine_line`].
    pub engine: String,
    /// Fin du journal de l'app.
    pub log: String,
}

/// Système de la machine (« Arch Linux, noyau 7.2.11 », « Windows 11 Pro
/// 24H2, build 26100.4061 »).
pub fn os_description() -> String {
    os::description()
}

/// `PRETTY_NAME` d'un fichier os-release (guillemets ôtés), sinon `NAME`.
#[cfg_attr(windows, allow(dead_code))]
pub fn os_release_name(text: &str) -> Option<String> {
    let value = |key: &str| {
        text.lines().find_map(|line| {
            let value = line.strip_prefix(key)?.strip_prefix('=')?;
            let value = value.trim().trim_matches(['"', '\'']).trim();
            (!value.is_empty()).then(|| value.to_owned())
        })
    };
    value("PRETTY_NAME").or_else(|| value("NAME"))
}

/// Nom de Windows d'après le registre. Windows 11 garde « Windows 10 » dans
/// `ProductName` : le numéro de build (22000 et plus) le corrige.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_name(
    product: Option<&str>,
    display: Option<&str>,
    build: Option<&str>,
    ubr: Option<u32>,
) -> String {
    let number = build.and_then(|build| build.trim().parse::<u32>().ok());
    let mut name = product.unwrap_or("Windows").trim().to_owned();
    if number.is_some_and(|number| number >= 22_000) {
        name = name.replace("Windows 10", "Windows 11");
    }
    if let Some(display) = display.map(str::trim).filter(|value| !value.is_empty()) {
        name.push(' ');
        name.push_str(display);
    }
    if let Some(number) = number {
        name.push_str(&format!(", build {number}"));
        if let Some(ubr) = ubr {
            name.push_str(&format!(".{ubr}"));
        }
    }
    name
}

/// Bureau et session graphique (Linux : « KDE, wayland »).
pub fn desktop() -> Option<String> {
    os::desktop()
}

/// Ligne « Engine » du diagnostic : « llama.cpp <version>, Vulkan (<carte>) »,
/// « …, processor (no graphics card) », « …, processor (graphics card failed
/// this session: <carte>) », ou « missing » sans moteur livré.
pub fn engine_line(status: &EngineStatus) -> String {
    if !status.available {
        return "missing".to_owned();
    }
    let version = status.version.as_deref().unwrap_or("unknown version");
    let backend = match (status.backend, status.cpu_fallback, &status.gpu_name) {
        (EngineBackend::Cuda, _, Some(gpu)) => format!("CUDA ({gpu})"),
        (EngineBackend::Cuda, _, None) => "CUDA".to_owned(),
        (EngineBackend::Vulkan, _, Some(gpu)) => format!("Vulkan ({gpu})"),
        (EngineBackend::Vulkan, _, None) => "Vulkan".to_owned(),
        (EngineBackend::Cpu, true, Some(gpu)) => {
            format!("processor (graphics card failed this session: {gpu})")
        }
        (EngineBackend::Cpu, true, None) => {
            "processor (graphics card failed this session)".to_owned()
        }
        (EngineBackend::Cpu, false, _) => "processor (no graphics card)".to_owned(),
    };
    format!("llama.cpp {version}, {backend}")
}

/// Texte du diagnostic, tel qu'il est montré puis envoyé.
pub fn diagnostic(facts: &Facts) -> String {
    let mut text = String::new();
    let mut line = |label: &str, value: &str| {
        text.push_str(label);
        text.push_str(": ");
        text.push_str(value);
        text.push('\n');
    };
    line("ChatonLM", &facts.version);
    line("System", &facts.os);
    line("Architecture", facts.arch);
    if let Some(desktop) = &facts.desktop {
        line("Desktop", desktop);
    }
    line("Installed as", &facts.install);
    line("Language", &facts.language);
    line("Engine", &facts.engine);
    let log = facts.log.trim_end();
    if !log.is_empty() {
        text.push_str("\n--- chatonlm.log (end) ---\n");
        text.push_str(log);
        text.push('\n');
    }
    let text = mask_personal(&text, home_dir().as_deref(), user_name().as_deref());
    fit(text, DIAGNOSTIC_MAX)
}

/// Garde la fin d'un texte trop long (la fin du journal compte le plus), à
/// partir d'une ligne entière, mais toujours ses premières lignes (faits).
fn fit(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        return text;
    }
    let (head, log) = match text.find("\n--- ") {
        Some(at) => text.split_at(at),
        None => (text.as_str(), ""),
    };
    let room = max.saturating_sub(head.chars().count() + 1);
    let skip = log.chars().count().saturating_sub(room);
    let rest: String = log.chars().skip(skip).collect();
    let rest = match rest.find('\n') {
        Some(at) if skip > 0 => &rest[at + 1..],
        _ => rest.as_str(),
    };
    let mut out = String::from(head);
    out.push('\n');
    out.push_str(rest);
    out.chars().take(max).collect()
}

/// Dossier personnel → `~`, nom du compte → `<user>`. Un nom très court
/// (moins de 3 lettres) n'est pas masqué ailleurs que dans le dossier : il
/// effacerait des mots entiers.
pub fn mask_personal(text: &str, home: Option<&str>, user: Option<&str>) -> String {
    let mut text = text.to_owned();
    if let Some(home) = home.map(|home| home.trim_end_matches(['/', '\\']))
        && home.len() > 1
    {
        text = text.replace(home, "~");
        // Windows : chemins écrits avec l'autre séparateur.
        text = text.replace(&home.replace('\\', "/"), "~");
    }
    if let Some(user) = user
        && user.chars().count() >= 3
    {
        text = replace_word(&text, user, "<user>");
    }
    text
}

/// Remplace `word` seulement là où il n'est pas collé à une lettre ou un
/// chiffre (« theo » masqué, « theory » gardé).
fn replace_word(text: &str, word: &str, by: &str) -> String {
    let glued = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(word) {
        let before = rest[..at].chars().last().or_else(|| out.chars().last());
        let after = rest[at + word.len()..].chars().next();
        out.push_str(&rest[..at]);
        if glued(before) || glued(after) {
            out.push_str(word);
        } else {
            out.push_str(by);
        }
        rest = &rest[at + word.len()..];
    }
    out.push_str(rest);
    out
}

fn home_dir() -> Option<String> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn user_name() -> Option<String> {
    let name = if cfg!(windows) { "USERNAME" } else { "USER" };
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Corps de la requête (contrat de `/api/report`).
pub fn payload(report: &BugReport, facts: &Facts, lang: &str) -> serde_json::Value {
    let mut body = serde_json::json!({
        "message": report.message.trim(),
        "email": report.email.trim(),
        "lang": lang,
        "version": facts.version,
        "platform": facts.platform,
    });
    if report.include_diagnostic {
        body["diagnostic"] = diagnostic(facts).into();
    }
    body
}

/// `"linux"` ou `"windows"`, comme l'attend le service.
pub fn platform_name() -> &'static str {
    match Platform::current() {
        Platform::Linux => "linux",
        Platform::Windows => "windows",
    }
}

fn report_url() -> String {
    test_override("CHATONLM_REPORT_URL").unwrap_or_else(|| REPORT_URL.to_owned())
}

#[cfg(any(test, debug_assertions, feature = "e2e"))]
fn test_override(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(not(any(test, debug_assertions, feature = "e2e")))]
fn test_override(_name: &str) -> Option<String> {
    None
}

/// Envoie le signalement au service.
pub async fn send(body: &serde_json::Value) -> ReportOutcome {
    send_to(&report_url(), body).await
}

async fn send_to(url: &str, body: &serde_json::Value) -> ReportOutcome {
    crate::hub::hf::ensure_crypto_provider();
    let Ok(client) = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("ChatonLM/", env!("CARGO_PKG_VERSION")))
        .build()
    else {
        return ReportOutcome::Unavailable;
    };
    let response = match client.post(url).json(body).send().await {
        Ok(response) => response,
        Err(error) => {
            crate::app_log!("chatonlm: bug report not sent: {error}");
            return ReportOutcome::Unavailable;
        }
    };
    let status = response.status().as_u16();
    let error = response
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|value| value.get("error")?.as_str().map(str::to_owned));
    let outcome = outcome_of(status, error.as_deref());
    if outcome != ReportOutcome::Sent {
        crate::app_log!(
            "chatonlm: bug report refused ({status}, {})",
            error.as_deref().unwrap_or("-")
        );
    }
    outcome
}

/// Réponse du service → suite de l'envoi.
fn outcome_of(status: u16, error: Option<&str>) -> ReportOutcome {
    match (status, error) {
        (200..=299, _) => ReportOutcome::Sent,
        (429, _) => ReportOutcome::TooMany,
        (422, Some("email")) => ReportOutcome::InvalidEmail,
        (422, Some("message")) => ReportOutcome::InvalidMessage,
        _ => ReportOutcome::Unavailable,
    }
}

#[cfg(test)]
mod tests;
