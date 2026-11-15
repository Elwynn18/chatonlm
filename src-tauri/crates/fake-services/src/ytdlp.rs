//! Faux yt-dlp et faux Deno (outil Transcription, module
//! `transcription::ytdlp` de l'app) : binaires `fake-yt-dlp` et `fake-deno`
//! (e2e, donnés au faux GitHub par `FAKE_GITHUB_YTDLP_BIN` et
//! `FAKE_GITHUB_DENO_BIN`), et relancés par les essais de l'app.
//!
//! Le faux yt-dlp lit les options du vrai (une option inconnue : sortie 2,
//! comme le vrai) et refuse un lancement sans celles que l'app doit
//! toujours passer : `--ignore-config`, `--no-update`, `--no-playlist`,
//! `--no-js-runtimes` puis `--js-runtimes deno:<programme existant>`,
//! `--ffmpeg-location <dossier>`, `--cache-dir`. Il lance le Deno donné
//! (`--version`, puis `run … -` avec un script sur l'entrée standard, comme
//! le vrai pour les défis de YouTube), puis imite les sorties réelles
//! (`transcription/ytdlp/fixtures/` de l'app) :
//! - `-J <adresse>` : métadonnées en une ligne JSON, ou une erreur `ERROR:`
//!   choisie par le dernier segment du chemin de l'adresse ([`SCENARIOS`]) ;
//!   `?d=<s>` : durée, `?fmt=m4a` : audio m4a (sinon webm/Opus),
//!   `?title=` : titre ;
//! - `--load-info-json <fichier> -P <dossier> -o audio.%(ext)s` : écrit
//!   `audio.<ext>.part` par morceaux (lignes du `--progress-template`),
//!   le renomme, puis la ligne du `--print after_move:` ; l'audio est une
//!   vraie fixture de `media-audio` (jfk, domaine public). Scénarios du
//!   deuxième temps : `endless` (fiche sans durée, flux sans fin ni taille
//!   annoncée, comme une webradio), `quiet-endless` (pareil, sans ligne de
//!   progression), `big` (taille annoncée énorme), `reextract` (échec 403,
//!   puis, comme le vrai `download_with_info_file`, nouvelle extraction de
//!   `webpage_url` s'il est dans la fiche chargée : flux sans fin). Un flux
//!   sans fin s'arrête de lui-même après [`ENDLESS_SAFETY`] octets (essai
//!   raté : le disque ne se remplit pas).
//!
//! Version (`--version`) : `FAKE_YTDLP_VERSION`, sinon le fichier
//! `_internal/chatonlm-fake-version` à côté du programme (écrit par le faux
//! GitHub), sinon [`crate::github::YTDLP_VERSION`]. Le scénario `outdated`
//! échoue (« n challenge solving failed ») avec cette version-là seulement :
//! une mise à jour le répare. Journal : `FAKE_YTDLP_LOG=<fichier>` reçoit
//! une ligne JSON (tableau des arguments) par lancement de yt-dlp, et
//! `<fichier>.deno-pid` le numéro de chaque `deno run`.
//!
//! Faux Deno : `--version` (`FAKE_DENO_VERSION`, sinon
//! `chatonlm-fake-version` à côté du programme, sinon
//! [`crate::github::DENO_VERSION`]) ; `run … -` lit son entrée jusqu'au bout
//! et répond un JSON fixe.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

/// Audio écrit par le faux (vraies sorties de YouTube réencodées : jfk).
pub const AUDIO_WEBM: &[u8] = include_bytes!("../../media-audio/tests/data/jfk-yt251.webm");
pub const AUDIO_M4A: &[u8] = include_bytes!("../../media-audio/tests/data/jfk-yt140.m4a");

/// Octets au plus d'un flux « sans fin » du faux (garde-fou des essais).
pub const ENDLESS_SAFETY: usize = 256 << 20;

/// Fichier de version à côté du programme (écrit par le faux GitHub).
pub const VERSION_FILE: &str = "chatonlm-fake-version";

/// Indication de connexion de yt-dlp 2026.10.26 (`_login_hint`,
/// `_youtube_login_hint`).
const LOGIN_HINT: &str = "Use --cookies-from-browser or --cookies for the authentication. See  https://github.com/yt-dlp/yt-dlp/wiki/FAQ#how-do-i-pass-cookies-to-yt-dlp  for how to manually pass cookies. Also see  https://github.com/yt-dlp/yt-dlp/wiki/Extractors#exporting-youtube-cookies  for tips on effectively exporting YouTube cookies";

/// Scénarios d'erreur du premier temps : dernier segment du chemin, puis
/// la sortie d'erreur (messages réels, ou tirés du code de yt-dlp
/// 2026.10.26 quand ils ne s'obtiennent pas à la demande).
pub const SCENARIOS: &[(&str, &str)] = &[
    (
        "private",
        "ERROR: [youtube] fakevideo1: Private video. Sign in if you've been granted access to this video. {hint}",
    ),
    (
        "age",
        "ERROR: [youtube] fakevideo1: Sign in to confirm your age. This video may be inappropriate for some users. {hint}",
    ),
    (
        "bot",
        "ERROR: [youtube] fakevideo1: Sign in to confirm you\u{2019}re not a bot. {hint}",
    ),
    (
        "members",
        "ERROR: [youtube] fakevideo1: Join this channel to get access to members-only content like this video, and other exclusive perks.",
    ),
    (
        "geo",
        "ERROR: [youtube] fakevideo1: The uploader has not made this video available in your country",
    ),
    (
        "gone",
        "ERROR: [youtube] aaaaaaaaaaa: This video is unavailable",
    ),
    (
        "upcoming",
        "ERROR: [youtube] fakevideo1: This live event will begin in 3 hours.",
    ),
    (
        "unsupported",
        "WARNING: [generic] Falling back on generic information extractor\nERROR: Unsupported URL: {url}",
    ),
    (
        "network",
        "ERROR: [generic] video: Unable to download webpage: HTTPSConnection(host='chatonlm-fixture.invalid', port=443): Failed to resolve 'chatonlm-fixture.invalid' ([Errno -2] Name or service not known)",
    ),
];

/// Erreur d'un yt-dlp périmé (défi de YouTube non résolu).
pub const OUTDATED: &str = "WARNING: [youtube] fakevideo1: n challenge solving failed: Some formats may be missing. Ensure you have a supported JavaScript runtime and challenge solver script distribution installed.\nERROR: [youtube] fakevideo1: Requested format is not available. Use --list-formats for a list of available formats";

/// Options sans valeur que connaît le faux.
const FLAGS: &[&str] = &[
    "--ignore-config",
    "--no-plugin-dirs",
    "--no-update",
    "--no-playlist",
    "--no-colors",
    "--newline",
    "--no-cookies",
    "--no-js-runtimes",
    "--flat-playlist",
    "-J",
    "--dump-single-json",
    "--progress",
    "--version",
    "--no-simulate",
    "-q",
    "--quiet",
    "-v",
    "--verbose",
];

/// Options à une valeur.
const VALUED: &[&str] = &[
    "--encoding",
    "--socket-timeout",
    "--retries",
    "--fragment-retries",
    "--extractor-retries",
    "--js-runtimes",
    "--cache-dir",
    "--ffmpeg-location",
    "--load-info-json",
    "-P",
    "--paths",
    "-f",
    "--format",
    "-o",
    "--output",
    "--progress-delta",
    "--progress-template",
    "--print",
    "-O",
];

/// Arguments lus.
#[derive(Default)]
struct Parsed {
    flags: Vec<String>,
    values: Vec<(String, String)>,
    urls: Vec<String>,
}

impl Parsed {
    fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|known| known == flag)
    }

    fn value(&self, option: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(name, _)| name == option)
            .map(|(_, value)| value.as_str())
    }

    fn values(&self, option: &str) -> impl Iterator<Item = &str> {
        self.values
            .iter()
            .filter(move |(name, _)| name == option)
            .map(|(_, value)| value.as_str())
    }
}

fn parse(args: &[String]) -> Result<(Parsed, Vec<String>), String> {
    let mut parsed = Parsed::default();
    let mut order = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            parsed.urls.extend(args[index + 1..].iter().cloned());
            break;
        }
        if FLAGS.contains(&arg.as_str()) {
            parsed.flags.push(arg.clone());
            order.push(arg.clone());
        } else if VALUED.contains(&arg.as_str()) {
            let value = args
                .get(index + 1)
                .ok_or_else(|| format!("{arg} option requires an argument"))?;
            parsed.values.push((arg.clone(), value.clone()));
            order.push(arg.clone());
            index += 1;
        } else if arg.starts_with('-') {
            return Err(format!("no such option: {arg}"));
        } else {
            parsed.urls.push(arg.clone());
        }
        index += 1;
    }
    Ok((parsed, order))
}

fn program_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// Version annoncée par le faux yt-dlp.
pub fn ytdlp_version() -> String {
    if let Ok(version) = std::env::var("FAKE_YTDLP_VERSION") {
        return version;
    }
    program_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("_internal").join(VERSION_FILE)).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| crate::github::YTDLP_VERSION.to_owned())
}

/// Version annoncée par le faux Deno.
pub fn deno_version() -> String {
    if let Ok(version) = std::env::var("FAKE_DENO_VERSION") {
        return version;
    }
    program_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join(VERSION_FILE)).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| crate::github::DENO_VERSION.to_owned())
}

fn log(args: &[String]) {
    if let Some(path) = std::env::var_os("FAKE_YTDLP_LOG")
        && let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        let _ = writeln!(file, "{}", json!(args));
    }
}

fn fail(message: &str, code: i32) -> i32 {
    let _ = writeln!(std::io::stderr(), "{message}");
    code
}

fn out(line: &str) {
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

/// Remplace `%(nom)s` et `%(nom)j` de `template` par les valeurs de
/// `fields` (`NA` ou `null` si absent), comme le vrai.
pub fn render(template: &str, fields: &HashMap<&str, Value>) -> String {
    let mut rendered = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("%(") {
        rendered.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else {
            rendered.push_str(&rest[start..]);
            return rendered;
        };
        let name = &after[..end];
        let conversion = after[end + 1..].chars().next();
        let kind = conversion.unwrap_or('s');
        let value = fields.get(name);
        match kind {
            'j' => rendered.push_str(&value.cloned().unwrap_or(Value::Null).to_string()),
            _ => match value {
                None | Some(Value::Null) => rendered.push_str("NA"),
                Some(Value::String(text)) => rendered.push_str(text),
                Some(other) => rendered.push_str(&other.to_string()),
            },
        }
        rest = &after[end + 1 + conversion.map_or(0, char::len_utf8)..];
    }
    rendered.push_str(rest);
    rendered
}

/// Vérifie les options que l'app doit toujours passer ; rend Deno.
fn check_required(parsed: &Parsed, order: &[String]) -> Result<PathBuf, String> {
    for flag in [
        "--ignore-config",
        "--no-update",
        "--no-playlist",
        "--no-js-runtimes",
    ] {
        if !parsed.has(flag) {
            return Err(format!("ERROR: fake yt-dlp: {flag} is required"));
        }
    }
    let position = |name: &str| order.iter().position(|known| known == name);
    if position("--no-js-runtimes") > position("--js-runtimes") {
        return Err("ERROR: fake yt-dlp: --no-js-runtimes must come before --js-runtimes".into());
    }
    let runtime = parsed
        .value("--js-runtimes")
        .ok_or("ERROR: fake yt-dlp: --js-runtimes deno:<path> is required")?;
    let deno = runtime
        .strip_prefix("deno:")
        .ok_or("ERROR: fake yt-dlp: the runtime must be deno")?;
    let deno = PathBuf::from(deno);
    if !deno.is_absolute() || !deno.is_file() {
        return Err(format!("ERROR: fake yt-dlp: no Deno at {}", deno.display()));
    }
    let ffmpeg = parsed
        .value("--ffmpeg-location")
        .ok_or("ERROR: fake yt-dlp: --ffmpeg-location is required")?;
    if !Path::new(ffmpeg).is_dir() {
        return Err("ERROR: fake yt-dlp: --ffmpeg-location must be a folder".into());
    }
    if parsed.value("--cache-dir").is_none() {
        return Err("ERROR: fake yt-dlp: --cache-dir is required".into());
    }
    Ok(deno)
}

/// `deno --version` : une ligne `deno 2.x`.
fn deno_works(deno: &Path) -> bool {
    Command::new(deno)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line.starts_with("deno 2."))
        })
}

/// `deno run … -` comme le vrai yt-dlp (script sur l'entrée standard).
fn deno_run(deno: &Path) -> std::io::Result<std::process::Child> {
    Command::new(deno)
        .args([
            "run",
            "--ext=js",
            "--no-code-cache",
            "--no-prompt",
            "--no-remote",
            "--no-lock",
            "--node-modules-dir=none",
            "--no-config",
            "--no-npm",
            "--cached-only",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

/// Défi résolu par Deno : vrai si sa réponse est un résultat JSON.
fn solve_challenge(deno: &Path) -> bool {
    if !deno_works(deno) {
        return false;
    }
    let Ok(mut child) = deno_run(deno) else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"globalThis.challenge = 'fake';\n");
    }
    let Ok(output) = child.wait_with_output() else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        serde_json::from_str::<Value>(line.trim())
            .is_ok_and(|value| value.get("type").and_then(Value::as_str) == Some("result"))
    })
}

/// Le faux yt-dlp ; rend le code de sortie.
pub fn run_yt_dlp(args: Vec<String>) -> i32 {
    log(&args);
    let (parsed, order) = match parse(&args) {
        Ok(parsed) => parsed,
        Err(error) => {
            return fail(
                &format!("Usage: yt-dlp [OPTIONS] URL [URL...]\n\nyt-dlp: error: {error}"),
                2,
            );
        }
    };
    if parsed.has("--version") {
        out(&ytdlp_version());
        return 0;
    }
    let deno = match check_required(&parsed, &order) {
        Ok(deno) => deno,
        Err(error) => return fail(&error, 2),
    };
    if let Some(info) = parsed.value("--load-info-json") {
        return download(&parsed, Path::new(info), &deno);
    }
    if parsed.has("-J") || parsed.has("--dump-single-json") {
        let Some(url) = parsed.urls.first() else {
            return fail(
                "Usage: yt-dlp [OPTIONS] URL [URL...]\n\nyt-dlp: error: You must provide at least one URL.",
                2,
            );
        };
        return metadata(url, &deno, parsed.has("--flat-playlist"));
    }
    fail("ERROR: fake yt-dlp: -J or --load-info-json expected", 2)
}

fn metadata(url: &str, deno: &Path, flat: bool) -> i32 {
    let Ok(parsed) = url::Url::parse(url) else {
        return fail(&format!("ERROR: [generic] '{url}' is not a valid URL"), 1);
    };
    let scenario = parsed
        .path_segments()
        .and_then(|mut segments| segments.next_back().map(str::to_owned))
        .unwrap_or_default();
    let query: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
    if scenario == "crash" {
        return 1;
    }
    if let Some((_, message)) = SCENARIOS.iter().find(|(name, _)| *name == scenario) {
        return fail(
            &message.replace("{hint}", LOGIN_HINT).replace("{url}", url),
            1,
        );
    }
    if !solve_challenge(deno)
        || (scenario == "outdated" && ytdlp_version() == crate::github::YTDLP_VERSION)
    {
        return fail(OUTDATED, 1);
    }
    let duration = query
        .get("d")
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(if scenario == "long" { 36_000.0 } else { 11.0 });
    // Flux direct de l'extracteur générique : ni durée ni direct signalé.
    let duration = (!matches!(scenario.as_str(), "endless" | "quiet-endless")).then_some(duration);
    let fmt = query.get("fmt").cloned().unwrap_or_else(|| "webm".into());
    let info = if scenario == "playlist" {
        if !flat {
            return fail("ERROR: fake yt-dlp: --flat-playlist expected", 2);
        }
        json!({
            "_type": "playlist", "id": "PLfake", "title": "Fake playlist",
            "entries": [{"_type": "url", "id": "fakevideo1", "url": url}],
            "extractor": "fake", "webpage_url": url,
        })
    } else {
        let live = scenario == "live";
        json!({
            "_type": "video",
            "id": "fakevideo1",
            "title": query.get("title").cloned().unwrap_or_else(|| "Fake talk".into()),
            "channel": "ChatonLM tests",
            "uploader": "ChatonLM tests",
            "duration": duration,
            "webpage_url": url,
            "original_url": url,
            "extractor": "fake",
            "extractor_key": "Fake",
            "live_status": if live { "is_live" } else { "not_live" },
            "is_live": live,
            "language": query.get("lang").cloned().unwrap_or_else(|| "en".into()),
            "formats": [
                {"format_id": "251", "ext": "webm", "acodec": "opus", "vcodec": "none", "protocol": "https", "url": "https://fake.example/251"},
                {"format_id": "140", "ext": "m4a", "acodec": "mp4a.40.2", "vcodec": "none", "protocol": "https", "url": "https://fake.example/140"},
            ],
            "_fake_scenario": scenario,
            "_fake_fmt": fmt,
        })
    };
    out(&info.to_string());
    0
}

fn download(parsed: &Parsed, info: &Path, deno: &Path) -> i32 {
    let Ok(text) = std::fs::read_to_string(info) else {
        return fail("ERROR: fake yt-dlp: cannot read the info JSON", 1);
    };
    let Some(info) = text
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
    else {
        return fail("ERROR: fake yt-dlp: invalid info JSON", 1);
    };
    if parsed.value("-f").is_none() {
        return fail("ERROR: fake yt-dlp: -f expected", 2);
    }
    let (Some(dir), Some(template)) = (parsed.value("-P"), parsed.value("-o")) else {
        return fail("ERROR: fake yt-dlp: -P and -o expected", 2);
    };
    let scenario = info
        .get("_fake_scenario")
        .and_then(Value::as_str)
        .unwrap_or("");
    let m4a = info.get("_fake_fmt").and_then(Value::as_str) == Some("m4a");
    let (ext, audio) = if m4a {
        ("m4a", AUDIO_M4A)
    } else {
        ("webm", AUDIO_WEBM)
    };
    let name = template.replace("%(ext)s", ext);
    let path = Path::new(dir).join(&name);
    let mut part_name = path.clone().into_os_string();
    part_name.push(".part");
    let part = PathBuf::from(part_name);
    let progress = parsed
        .values("--progress-template")
        .find_map(|template| template.strip_prefix("download:"))
        .map(str::to_owned);
    let after_move = parsed
        .values("--print")
        .find_map(|template| template.strip_prefix("after_move:"))
        .map(str::to_owned);
    // Défi gardé ouvert pendant un téléchargement lent : l'arrêt doit
    // aussi atteindre ce Deno petit-enfant.
    let mut held = if scenario == "slow" {
        deno_run(deno).ok()
    } else {
        None
    };
    let (chunks, pause) = if scenario == "slow" {
        (40, Duration::from_millis(250))
    } else {
        (4, Duration::from_millis(5))
    };
    let total = audio.len();
    let report = |status: &str, done: usize| {
        if let Some(template) = &progress {
            let fields = HashMap::from([
                ("progress.status", json!(status)),
                ("progress.downloaded_bytes", json!(done)),
                ("progress.total_bytes", json!(total)),
                ("progress.total_bytes_estimate", Value::Null),
            ]);
            out(&render(template, &fields));
        }
    };
    if scenario == "reextract" {
        let _ = writeln!(
            std::io::stderr(),
            "ERROR: unable to download video data: HTTP Error 403: Forbidden"
        );
        // `YoutubeDL.download_with_info_file` : sans `webpage_url`, l'erreur
        // remonte ; sinon, extraction complète de l'adresse.
        let Some(url) = info.get("webpage_url").and_then(Value::as_str) else {
            return 1;
        };
        let _ = writeln!(
            std::io::stderr(),
            "WARNING: The info failed to download: ERROR: unable to download video data: HTTP Error 403: Forbidden; trying with URL {url}"
        );
        return endless(&part, progress.as_deref(), None);
    }
    match scenario {
        "endless" => return endless(&part, progress.as_deref(), None),
        "quiet-endless" => return endless(&part, None, None),
        "big" => return endless(&part, progress.as_deref(), Some(100 << 30)),
        _ => {}
    }
    let Ok(mut file) = std::fs::File::create(&part) else {
        return fail("ERROR: fake yt-dlp: cannot write the audio", 1);
    };
    let step = total.div_ceil(chunks);
    let mut done = 0;
    for chunk in audio.chunks(step) {
        if file.write_all(chunk).is_err() {
            return fail("ERROR: fake yt-dlp: cannot write the audio", 1);
        }
        done += chunk.len();
        report("downloading", done);
        if scenario == "forbidden" {
            return fail(
                "ERROR: unable to download video data: HTTP Error 403: Forbidden",
                1,
            );
        }
        std::thread::sleep(pause);
    }
    drop(file);
    if std::fs::rename(&part, &path).is_err() {
        return fail("ERROR: fake yt-dlp: cannot rename the audio", 1);
    }
    report("finished", total);
    if let Some(template) = &after_move {
        let fields = HashMap::from([("filepath", json!(path.to_string_lossy()))]);
        out(&render(template, &fields));
    }
    if let Some(mut child) = held.take() {
        drop(child.stdin.take());
        let _ = child.wait();
    }
    0
}

/// Flux sans fin vers `part` (morceaux de l'audio répétés), lignes de
/// progression sans taille (ou `total` annoncé) ; s'arrête de lui-même
/// après [`ENDLESS_SAFETY`] octets, puis attend d'être arrêté (60 s au
/// plus).
fn endless(part: &Path, progress: Option<&str>, total: Option<u64>) -> i32 {
    let Ok(mut file) = std::fs::File::create(part) else {
        return fail("ERROR: fake yt-dlp: cannot write the audio", 1);
    };
    let block = AUDIO_WEBM.repeat((64 << 10) / AUDIO_WEBM.len() + 1);
    let mut done = 0usize;
    while done < ENDLESS_SAFETY {
        if file.write_all(&block).is_err() {
            return fail("ERROR: fake yt-dlp: cannot write the audio", 1);
        }
        done += block.len();
        if let Some(template) = progress {
            let fields = HashMap::from([
                ("progress.status", json!("downloading")),
                ("progress.downloaded_bytes", json!(done)),
                ("progress.total_bytes", json!(total)),
                ("progress.total_bytes_estimate", Value::Null),
            ]);
            out(&render(template, &fields));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(Duration::from_secs(60));
    0
}

/// Le faux Deno ; rend le code de sortie.
pub fn run_deno(args: Vec<String>) -> i32 {
    match args.first().map(String::as_str) {
        Some("--version") => {
            out(&format!(
                "deno {} (stable, release, x86_64-unknown-linux-gnu)\nv8 14.7.173.20-rusty\ntypescript 5.9.2",
                deno_version()
            ));
            0
        }
        Some("run") if args.last().map(String::as_str) == Some("-") => {
            if let Some(log) = std::env::var_os("FAKE_YTDLP_LOG") {
                let mut path = log;
                path.push(".deno-pid");
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(PathBuf::from(path))
                {
                    let _ = writeln!(file, "{}", std::process::id());
                }
            }
            let mut script = Vec::new();
            let _ = std::io::stdin().read_to_end(&mut script);
            out(r#"{"type":"result","responses":[{"type":"result","data":{"n":"fake"}}]}"#);
            0
        }
        _ => fail("error: fake deno: unsupported arguments", 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_render_like_yt_dlp() {
        let fields = HashMap::from([
            ("progress.status", json!("downloading")),
            ("progress.downloaded_bytes", json!(1024)),
            ("progress.total_bytes", Value::Null),
            ("filepath", json!("/tmp/a \"b\".webm")),
        ]);
        assert_eq!(
            render(
                "X %(progress.status)s %(progress.downloaded_bytes)s %(progress.total_bytes)s",
                &fields
            ),
            "X downloading 1024 NA"
        );
        assert_eq!(
            render("F %(filepath)j", &fields),
            r#"F "/tmp/a \"b\".webm""#
        );
    }

    #[test]
    fn unknown_options_are_refused_like_yt_dlp() {
        assert!(parse(&["--no-such-option".into()]).is_err());
        let (parsed, _) = parse(&[
            "-f".into(),
            "ba".into(),
            "--".into(),
            "-not-an-option".into(),
        ])
        .unwrap();
        assert_eq!(parsed.value("-f"), Some("ba"));
        assert_eq!(parsed.urls, ["-not-an-option"]);
    }

    #[test]
    fn required_options_are_checked() {
        let deno = std::env::current_exe().unwrap();
        let dir = deno.parent().unwrap().to_string_lossy().into_owned();
        let runtime = format!("deno:{}", deno.display());
        let full: Vec<String> = [
            "--ignore-config",
            "--no-update",
            "--no-playlist",
            "--no-js-runtimes",
            "--js-runtimes",
            &runtime,
            "--ffmpeg-location",
            &dir,
            "--cache-dir",
            &dir,
        ]
        .map(String::from)
        .to_vec();
        let (parsed, order) = parse(&full).unwrap();
        assert_eq!(check_required(&parsed, &order).unwrap(), deno);
        for missing in ["--no-playlist", "--no-js-runtimes", "--ignore-config"] {
            let args: Vec<String> = full.iter().filter(|a| *a != missing).cloned().collect();
            let (parsed, order) = parse(&args).unwrap();
            assert!(check_required(&parsed, &order).is_err(), "{missing}");
        }
    }
}
