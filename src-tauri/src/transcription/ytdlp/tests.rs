//! Essais du lancement de yt-dlp : lecteurs de lignes et classement des
//! erreurs sur les sorties réelles (`fixtures/`, enregistrées par
//! `scripts/record-ytdlp-fixtures.sh`) ; adresses ; deux temps, annulation
//! et nouvelle tentative contre le faux yt-dlp et le faux Deno
//! (`fake_services::ytdlp`), joués par ce binaire d'essai lui-même, relancé
//! sur [`fake_yt_dlp`] ou [`fake_deno`] par un petit script `sh` (Unix).
//!
//! Essai réel `ytdlp_live_youtube_cc_video` (réseau, YouTube : ignoré par
//! défaut, et sans « contract » dans son nom, pour rester hors de
//! `check.sh --full`) :
//!   cargo test --manifest-path src-tauri/Cargo.toml --locked -p chatonlm \
//!     --lib ytdlp_live_youtube -- --ignored --nocapture
//! avec les vrais outils de `scripts/record-ytdlp-fixtures.sh`
//! (`.tools/ytdlp-record/tools/`).

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

use super::*;

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/transcription/ytdlp/fixtures"
);

fn fixture(name: &str) -> String {
    std::fs::read_to_string(Path::new(FIXTURES).join(name)).unwrap()
}

fn temp_dir() -> tempfile::TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tools/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::tempdir_in(std::fs::canonicalize(base).unwrap()).unwrap()
}

/// Arguments donnés au faux par le script (séparés par U+001F).
fn argv_from(variable: &str) -> Option<Vec<String>> {
    let joined = std::env::var(variable).ok()?;
    let mut args: Vec<String> = joined.split('\u{1f}').map(str::to_owned).collect();
    if args.last().is_some_and(String::is_empty) {
        args.pop();
    }
    Some(args)
}

/// Faux yt-dlp : ignoré, sauf relancé par un essai.
#[test]
#[ignore = "faux yt-dlp lancé par les autres essais"]
fn fake_yt_dlp() {
    if let Some(args) = argv_from("FAKE_YTDLP_ARGV") {
        std::process::exit(fake_services::ytdlp::run_yt_dlp(args));
    }
}

/// Faux Deno : ignoré, sauf relancé par un essai.
#[test]
#[ignore = "faux Deno lancé par les autres essais"]
fn fake_deno() {
    if let Some(args) = argv_from("FAKE_DENO_ARGV") {
        std::process::exit(fake_services::ytdlp::run_deno(args));
    }
}

/// Le futur de [`fetch_audio`] est `Send` (lancé sur le moteur tokio par
/// la Transcription) : vérifié à la compilation, sans rien lancer.
#[test]
fn the_fetch_future_is_send() {
    fn send<T: Send>(_: T) {}
    let _check = |tools: &Tools, cancel: watch::Receiver<bool>| {
        send(fetch_audio(tools, "", Path::new(""), None, cancel, |_| {}));
    };
}

// --- Lecteurs de lignes et métadonnées (sorties réelles) ---

#[test]
fn progress_and_file_lines_of_a_real_download_are_read() {
    let lines: Vec<Line> = fixture("download-cc.stdout")
        .lines()
        .filter_map(parse_line)
        .collect();
    assert_eq!(
        lines,
        [
            Line::Progress {
                received: 1024,
                total: Some(9_496_783)
            },
            Line::Progress {
                received: 9_496_783,
                total: Some(9_496_783)
            },
            Line::File("@WORK@/audio.webm".into()),
        ]
    );
}

#[test]
fn progress_lines_take_the_estimate_and_ignore_the_rest() {
    assert_eq!(
        parse_line("CHATONLM-PROGRESS downloading 2048 NA 4096.5\n"),
        Some(Line::Progress {
            received: 2048,
            total: Some(4096)
        })
    );
    assert_eq!(
        parse_line("CHATONLM-PROGRESS downloading NA NA NA"),
        Some(Line::Progress {
            received: 0,
            total: None
        })
    );
    assert_eq!(
        parse_line(r#"CHATONLM-FILE "C:\\Users\\Zoé\\audio.m4a""#),
        Some(Line::File(r"C:\Users\Zoé\audio.m4a".into()))
    );
    for other in [
        "",
        "running 1 test",
        "[download] 10%",
        "CHATONLM-FILE not json",
    ] {
        assert_eq!(parse_line(other), None, "{other}");
    }
}

#[test]
fn metadata_of_a_real_video_are_read() {
    let value: Value = serde_json::from_str(&fixture("info-cc.json")).unwrap();
    let info = info_of(&value, "https://youtu.be/M4gD1WSo5mA").unwrap();
    assert_eq!(
        info,
        VideoInfo {
            title: Some(
                "William Fisher, CopyrightX: Lecture 3.2, The Subject Matter of Copyright: \
                 Drama and choreography"
                    .into()
            ),
            channel: Some("Berkman Klein Center for Internet & Society".into()),
            duration_s: Some(721.0),
            webpage_url: "https://www.youtube.com/watch?v=M4gD1WSo5mA".into(),
            site: "youtube".into(),
        }
    );
}

#[test]
fn metadata_file_takes_its_last_json_line() {
    let dir = temp_dir();
    let path = dir.path().join("info.json");
    std::fs::write(
        &path,
        format!("\nrunning 1 test\n{}\n", fixture("info-cc.json").trim()),
    )
    .unwrap();
    assert_eq!(
        read_info(&path, "https://x.test/").unwrap().duration_s,
        Some(721.0)
    );
    std::fs::write(&path, "running 1 test\n").unwrap();
    assert_eq!(
        read_info(&path, "https://x.test/").unwrap_err().code,
        FetchErrorCode::Failed
    );
}

#[test]
fn playlists_lives_and_missing_fields() {
    let url = "https://x.test/v";
    let code = |value: Value| info_of(&value, url).unwrap_err().code;
    assert_eq!(code(json!({"_type": "playlist"})), FetchErrorCode::Playlist);
    assert_eq!(
        code(json!({"_type": "multi_video"})),
        FetchErrorCode::Playlist
    );
    for status in ["is_live", "is_upcoming", "post_live"] {
        assert_eq!(code(json!({"live_status": status})), FetchErrorCode::Live);
    }
    assert_eq!(code(json!({"is_live": true})), FetchErrorCode::Live);
    assert_eq!(code(json!({"_type": "url"})), FetchErrorCode::Failed);
    let info = info_of(
        &json!({"live_status": "was_live", "uploader": "U", "extractor_key": "Vimeo"}),
        url,
    )
    .unwrap();
    assert_eq!(info.channel.as_deref(), Some("U"));
    assert_eq!(info.webpage_url, url);
    assert_eq!(info.site, "vimeo");
    assert_eq!(info.duration_s, None);
}

// --- Classement des erreurs ---

#[test]
fn real_errors_are_classified() {
    for (name, expected) in [
        ("error-unsupported.stderr", FetchErrorCode::Unsupported),
        ("error-unavailable.stderr", FetchErrorCode::Unavailable),
        ("error-network.stderr", FetchErrorCode::Network),
        ("error-options.stderr", FetchErrorCode::Failed),
        // Sans moteur JavaScript : seulement des avertissements.
        ("error-no-js.stderr", FetchErrorCode::Outdated),
        // Avertissement de ffmpeg absent (toujours là) : rien à signaler.
        ("info-cc.stderr", FetchErrorCode::Failed),
    ] {
        assert_eq!(classify(&fixture(name)), expected, "{name}");
    }
}

#[test]
fn messages_from_yt_dlp_code_are_classified() {
    use FetchErrorCode::*;
    let expected = |scenario: &str| match scenario {
        "private" => Private,
        "age" => AgeRestricted,
        "bot" => BotCheck,
        "members" => MembersOnly,
        "geo" => GeoBlocked,
        "gone" => Unavailable,
        "upcoming" => Live,
        "unsupported" => Unsupported,
        "network" => Network,
        other => panic!("scénario sans attente : {other}"),
    };
    for (scenario, stderr) in fake_services::ytdlp::SCENARIOS {
        assert_eq!(classify(stderr), expected(scenario), "{scenario}");
    }
    assert_eq!(classify(fake_services::ytdlp::OUTDATED), Outdated);
    for (stderr, code) in [
        (
            "ERROR: [youtube] x: This content isn't available, try again later. The current session has been rate-limited by YouTube for up to an hour.",
            BotCheck,
        ),
        (
            "ERROR: [youtube] x: Video unavailable. YouTube is requiring a captcha challenge before playback",
            BotCheck,
        ),
        (
            "ERROR: [youtube] x: Video unavailable. This video is private",
            Private,
        ),
        ("ERROR: [vimeo] 1: This video is DRM protected", Unsupported),
        ("ERROR: [youtube] x: Premieres in 10 hours", Live),
        (
            "ERROR: [vimeo] 1: This video is not available from your location due to geo restriction",
            GeoBlocked,
        ),
        (
            "ERROR: unable to download video data: HTTP Error 403: Forbidden",
            Outdated,
        ),
        (
            "ERROR: [generic] x: Unable to download webpage: HTTP Error 404: Not Found (caused by <HTTPError 404: Not Found>)",
            Unavailable,
        ),
        (
            "ERROR: unable to download video data: HTTP Error 503: Service Unavailable",
            Network,
        ),
        (
            "ERROR: [youtube] x: Unable to extract yt initial data; please report this issue on  https://github.com/yt-dlp/yt-dlp/issues?q= , filling out the appropriate issue template. Confirm you are on the latest version using  yt-dlp -U",
            Outdated,
        ),
        // Apostrophe typographique ou droite, majuscules indifférentes.
        (
            "ERROR: [youtube] x: Sign in to confirm you're NOT A BOT.",
            BotCheck,
        ),
        // Un avertissement qui ressemble à une erreur ne décide pas quand
        // une ligne ERROR existe.
        (
            "WARNING: [youtube] x: Private video in a playlist skipped\nERROR: [youtube] y: This video is unavailable",
            Unavailable,
        ),
        ("", Failed),
    ] {
        assert_eq!(classify(stderr), code, "{stderr}");
    }
}

#[test]
fn every_code_has_a_message() {
    use FetchErrorCode::*;
    let codes = [
        ToolsMissing,
        InvalidAddress,
        Unsupported,
        Playlist,
        Live,
        Private,
        AgeRestricted,
        MembersOnly,
        Unavailable,
        GeoBlocked,
        BotCheck,
        Network,
        Outdated,
        TooLong,
        TooLarge,
        NoSpace,
        Cancelled,
        Failed,
    ];
    let messages: std::collections::BTreeSet<String> = codes.iter().map(|c| message(*c)).collect();
    assert_eq!(messages.len(), codes.len(), "un message par code");
    assert!(
        messages.iter().all(|m| !m.starts_with("ytdlp.")),
        "{messages:?}"
    );
}

// --- Adresses et arguments ---

#[test]
fn addresses_are_checked() {
    for good in [
        "https://www.youtube.com/watch?v=M4gD1WSo5mA",
        "  http://video.test/ok?d=3  ",
        "https://vimeo.com/123456",
    ] {
        assert!(check_address(good).is_ok(), "{good}");
    }
    assert_eq!(
        check_address(" https://Video.Test/a ").unwrap(),
        "https://video.test/a"
    );
    for bad in [
        "",
        "--exec=rm -rf ~",
        "-o/tmp/x",
        "file:///etc/passwd",
        "/home/user/video.mp4",
        "C:\\Users\\video.mp4",
        "ytsearch:cats",
        "ftp://video.test/a",
        "https://",
        "https://user:secret@video.test/a",
        "https://video.test/a b",
        "https://video.test/a\nhttps://video.test/b",
        "javascript:alert(1)",
    ] {
        assert_eq!(
            check_address(bad).unwrap_err().code,
            FetchErrorCode::InvalidAddress,
            "{bad:?}"
        );
    }
    let long = format!("https://video.test/{}", "a".repeat(ADDRESS_LIMIT));
    assert!(check_address(&long).is_err());
}

fn sample_paths(dir: &Path) -> ToolPaths {
    ToolPaths {
        yt_dlp: dir.join("yt-dlp"),
        yt_dlp_version: "2026.10.26".into(),
        deno: dir.join("deno dir/deno"),
        deno_version: "2.9.8".into(),
        deno_dir: dir.join("deno-cache"),
        yt_dlp_cache: dir.join("cache"),
        no_ffmpeg: dir.join("no-ffmpeg"),
    }
}

#[test]
fn arguments_isolate_yt_dlp_and_never_convert() {
    let dir = Path::new("/d");
    let paths = sample_paths(dir);
    let info = info_args(&paths, "https://video.test/a");
    let text: Vec<String> = info
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    for required in [
        "--ignore-config",
        "--no-update",
        "--no-playlist",
        "--no-cookies",
        "--flat-playlist",
        "-J",
    ] {
        assert!(text.iter().any(|arg| arg == required), "{required}");
    }
    let position = |arg: &str| text.iter().position(|known| known == arg).unwrap();
    assert!(position("--no-js-runtimes") < position("--js-runtimes"));
    // Chemins attendus par `join` : séparateur « \ » sous Windows.
    let shown = |path: std::path::PathBuf| path.to_string_lossy().into_owned();
    assert_eq!(
        text[position("--js-runtimes") + 1],
        format!("deno:{}", shown(dir.join("deno dir/deno")))
    );
    assert_eq!(
        text[position("--ffmpeg-location") + 1],
        shown(dir.join("no-ffmpeg"))
    );
    assert_eq!(text[position("--cache-dir") + 1], shown(dir.join("cache")));
    // L'adresse vient après `--` : jamais lue comme une option.
    assert_eq!(&text[text.len() - 2..], ["--", "https://video.test/a"]);

    let download = download_args(
        &paths,
        Path::new("/w/.fetch/info.json"),
        Path::new("/w/.fetch"),
    );
    let text: Vec<String> = download
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let position = |arg: &str| text.iter().position(|known| known == arg).unwrap();
    assert_eq!(text[position("-f") + 1], SELECTION);
    assert_eq!(text[position("-o") + 1], "audio.%(ext)s");
    assert_eq!(text[position("-P") + 1], "/w/.fetch");
    for forbidden in [
        "-x",
        "--extract-audio",
        "--audio-format",
        "--remux-video",
        "-k",
    ] {
        assert!(!text.iter().any(|arg| arg == forbidden), "{forbidden}");
    }
    assert!(SELECTION.starts_with("ba[acodec=opus][protocol^=http]/ba[acodec=mp4a.40.2]"));
}

// --- Deux temps contre le faux (Unix : scripts `sh`) ---

/// Outils rendus par un double de [`Tools`] : `paths[n]` à la n-ième
/// demande (le dernier ensuite) ; `newer` : réponse de `refresh`.
struct Stub {
    paths: Vec<ToolPaths>,
    newer: bool,
    readies: AtomicUsize,
    refreshes: AtomicUsize,
}

impl Stub {
    fn new(paths: Vec<ToolPaths>, newer: bool) -> Stub {
        Stub {
            paths,
            newer,
            readies: AtomicUsize::new(0),
            refreshes: AtomicUsize::new(0),
        }
    }
}

impl Provider for Stub {
    type Lease = ();

    async fn ready(&self) -> Result<(ToolPaths, ()), FetchError> {
        let index = self.readies.fetch_add(1, Ordering::SeqCst);
        Ok((self.paths[index.min(self.paths.len() - 1)].clone(), ()))
    }

    async fn refresh(&self) -> bool {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        self.newer
    }
}

#[cfg(unix)]
mod fake {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn quoted(text: &str) -> String {
        format!("'{}'", text.replace('\'', r"'\''"))
    }

    /// Script `sh` qui relance ce binaire d'essai sur `test`, arguments
    /// passés par `variable`.
    fn script(path: &Path, test: &str, variable: &str, extra: &[(&str, String)]) {
        let exe = std::env::current_exe().unwrap();
        let mut env = String::new();
        for (key, value) in extra {
            env.push_str(&format!("{key}={} ", quoted(value)));
        }
        let text = format!(
            "#!/bin/sh\n{variable}=\"$(printf '%s\\037' \"$@\")\" {env}exec {} {test} --exact --ignored --test-threads=1 -q\n",
            quoted(&exe.to_string_lossy())
        );
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Faux outils sous `dir` (un « Deno » dans un dossier à espace) ;
    /// `version` : version annoncée par le faux yt-dlp.
    pub(super) fn tools(
        dir: &Path,
        name: &str,
        version: Option<&str>,
    ) -> (ToolPaths, std::path::PathBuf) {
        let base = dir.join(name);
        std::fs::create_dir_all(base.join("deno dir")).unwrap();
        let log = base.join("calls.jsonl");
        let mut extra = vec![("FAKE_YTDLP_LOG", log.to_string_lossy().into_owned())];
        if let Some(version) = version {
            extra.push(("FAKE_YTDLP_VERSION", version.to_owned()));
        }
        let paths = sample_paths(&base);
        script(
            &paths.yt_dlp,
            "transcription::ytdlp::tests::fake_yt_dlp",
            "FAKE_YTDLP_ARGV",
            &extra,
        );
        script(
            &paths.deno,
            "transcription::ytdlp::tests::fake_deno",
            "FAKE_DENO_ARGV",
            &extra[..1],
        );
        for dir in [&paths.deno_dir, &paths.yt_dlp_cache, &paths.no_ffmpeg] {
            std::fs::create_dir_all(dir).unwrap();
        }
        (paths, log)
    }

    pub(super) fn calls(log: &Path) -> Vec<Vec<String>> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// Fichiers restés sous `dir`.
    pub(super) fn files(root: &Path) -> Vec<String> {
        let mut found = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path.clone());
                }
                let relative = path.strip_prefix(root).unwrap_or(&path);
                found.push(relative.display().to_string());
            }
        }
        found.sort();
        found
    }
}

#[cfg(unix)]
async fn fetch(
    stub: &Stub,
    url: &str,
    work: &Path,
    max: Option<f64>,
) -> (Result<FetchedAudio, FetchError>, Vec<FetchProgress>) {
    let (_sender, cancel) = watch::channel(false);
    let seen = Mutex::new(Vec::new());
    let limits = FetchLimits::standard(max);
    let result = fetch_with(stub, url, work, &limits, cancel, &|progress| {
        seen.lock().unwrap().push(progress);
    })
    .await;
    (result, seen.into_inner().unwrap())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn audio_is_fetched_in_two_steps_into_the_work_folder() {
    let dir = temp_dir();
    let (paths, log) = fake::tools(dir.path(), "tools", None);
    for (query, ext, bytes, title) in [
        (
            "d=11&title=Talk",
            "webm",
            fake_services::ytdlp::AUDIO_WEBM,
            "Talk",
        ),
        (
            "fmt=m4a",
            "m4a",
            fake_services::ytdlp::AUDIO_M4A,
            "Fake talk",
        ),
    ] {
        let work = dir.path().join(format!("work-{ext}"));
        let stub = Stub::new(vec![paths.clone()], false);
        let url = format!("https://video.test/ok?{query}");
        let (result, seen) = fetch(&stub, &url, &work, Some(60.0)).await;
        let audio = result.unwrap();
        assert_eq!(audio.path, work.join(format!("audio.{ext}")));
        assert_eq!(std::fs::read(&audio.path).unwrap(), bytes);
        // Seul l'audio reste : dossier privé, métadonnées et `.part` effacés.
        assert_eq!(fake::files(&work), [format!("audio.{ext}")]);
        assert_eq!(audio.info.title.as_deref(), Some(title));
        assert_eq!(audio.info.channel.as_deref(), Some("ChatonLM tests"));
        assert_eq!(audio.info.duration_s, Some(11.0));
        assert_eq!(audio.info.site, "fake");
        let FetchProgress::Info(info) = &seen[0] else {
            panic!("fiche d'abord : {seen:?}");
        };
        assert_eq!(info, &audio.info);
        let Some(FetchProgress::Downloading { received, total }) = seen.last() else {
            panic!("progression ensuite : {seen:?}");
        };
        assert_eq!(
            (*received, *total),
            (bytes.len() as u64, Some(bytes.len() as u64))
        );
        assert!(seen.len() >= 4, "{seen:?}");
    }
    // Deux lancements par audio ; métadonnées, puis audio seul.
    let calls = fake::calls(&log);
    assert!(
        calls[0].contains(&"-J".to_owned()) && calls[0].contains(&"--flat-playlist".to_owned())
    );
    assert!(calls[1].contains(&"--load-info-json".to_owned()));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn refusals_come_before_any_download() {
    use FetchErrorCode::*;
    let dir = temp_dir();
    let (paths, log) = fake::tools(dir.path(), "tools", None);
    for (path, max, code) in [
        ("private", None, Private),
        ("bot", None, BotCheck),
        ("geo", None, GeoBlocked),
        ("playlist", None, Playlist),
        ("live", None, Live),
        ("upcoming", None, Live),
        ("long", Some(3600.0), TooLong),
        ("crash", None, Failed),
        ("unsupported", None, Unsupported),
    ] {
        let before = fake::calls(&log).len();
        let work = dir.path().join(format!("work-{path}"));
        let stub = Stub::new(vec![paths.clone()], false);
        let (result, seen) = fetch(&stub, &format!("https://video.test/{path}"), &work, max).await;
        let error = result.unwrap_err();
        assert_eq!(error.code, code, "{path} : {}", error.detail);
        assert!(seen.is_empty(), "{path} : {seen:?}");
        assert_eq!(
            fake::calls(&log).len(),
            before + 1,
            "{path} : un seul lancement"
        );
        assert!(
            fake::files(&work).is_empty(),
            "{path} : {:?}",
            fake::files(&work)
        );
    }
    // Durée inconnue ou sous la borne : acceptée.
    let stub = Stub::new(vec![paths.clone()], false);
    let (result, _) = fetch(
        &stub,
        "https://video.test/long",
        &dir.path().join("w"),
        Some(36_000.0),
    )
    .await;
    assert!(result.is_ok());
    // Adresse refusée : rien n'est lancé, pas même `ready`.
    let stub = Stub::new(vec![paths.clone()], false);
    let (result, _) = fetch(&stub, "file:///etc/passwd", &dir.path().join("w2"), None).await;
    assert_eq!(result.unwrap_err().code, InvalidAddress);
    assert_eq!(stub.readies.load(Ordering::SeqCst), 0);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn outdated_tools_are_refreshed_and_retried_once() {
    let dir = temp_dir();
    let (old, old_log) = fake::tools(dir.path(), "old", None);
    let (new, new_log) = fake::tools(dir.path(), "new", Some("2026.11.02"));

    // Une version plus récente répare : nouvelle tentative réussie.
    let stub = Stub::new(vec![old.clone(), new.clone()], true);
    let (result, _) = fetch(
        &stub,
        "https://video.test/outdated",
        &dir.path().join("w1"),
        None,
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(stub.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(stub.readies.load(Ordering::SeqCst), 2);
    assert_eq!(fake::calls(&old_log).len(), 1);
    assert_eq!(fake::calls(&new_log).len(), 2);

    // Pas de version plus récente : `Outdated`, sans nouvelle tentative.
    let stub = Stub::new(vec![old.clone()], false);
    let (result, _) = fetch(
        &stub,
        "https://video.test/outdated",
        &dir.path().join("w2"),
        None,
    )
    .await;
    assert_eq!(result.unwrap_err().code, FetchErrorCode::Outdated);
    assert_eq!(stub.readies.load(Ordering::SeqCst), 1);

    // Toujours périmé après la mise à jour : une seule nouvelle tentative.
    let stub = Stub::new(vec![old.clone()], true);
    let (result, _) = fetch(
        &stub,
        "https://video.test/outdated",
        &dir.path().join("w3"),
        None,
    )
    .await;
    assert_eq!(result.unwrap_err().code, FetchErrorCode::Outdated);
    assert_eq!(stub.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(stub.readies.load(Ordering::SeqCst), 2);

    // Refus au deuxième temps (403 sur l'audio) : périmé aussi, `.part`
    // effacé.
    let stub = Stub::new(vec![old.clone()], false);
    let work = dir.path().join("w4");
    let (result, _) = fetch(&stub, "https://video.test/forbidden", &work, None).await;
    assert_eq!(result.unwrap_err().code, FetchErrorCode::Outdated);
    assert!(fake::files(&work).is_empty(), "{:?}", fake::files(&work));
}

// --- Bornes de taille et de place (relecture 107) ---

/// Place libre « illimitée » des essais.
fn plenty(_: &Path) -> Option<u64> {
    Some(u64::MAX / 2)
}

/// Bornes d'essai : 8 Mio, 1 Mio pour un flux, réserve de 1 Mio.
fn small_limits(free_space: fn(&Path) -> Option<u64>) -> FetchLimits {
    FetchLimits {
        max_duration_s: Some(6.0 * 3600.0),
        max_bytes: 8 << 20,
        stream_bytes: 1 << 20,
        reserve_bytes: 1 << 20,
        free_space,
    }
}

#[cfg(unix)]
async fn fetch_limited(
    stub: &Stub,
    url: &str,
    work: &Path,
    limits: &FetchLimits,
) -> (Result<FetchedAudio, FetchError>, Vec<FetchProgress>) {
    let (_sender, cancel) = watch::channel(false);
    let seen = Mutex::new(Vec::new());
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        fetch_with(stub, url, work, limits, cancel, &|progress| {
            seen.lock().unwrap().push(progress);
        }),
    )
    .await
    .expect("borne non tenue : récupération sans fin");
    (result, seen.into_inner().unwrap())
}

#[test]
fn the_loaded_metadata_have_no_address_to_extract_again() {
    let dir = temp_dir();
    let path = dir.path().join("load.json");
    let value = json!({
        "_type": "video", "id": "x", "title": "T", "duration": 3.5,
        "webpage_url": "https://video.test/x", "original_url": "https://video.test/x",
        "formats": [{"format_id": "251", "url": "https://cdn.test/251"}],
    });
    write_load_info(&path, value.clone()).unwrap();
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(written.get("webpage_url").is_none());
    assert!(written.get("original_url").is_none());
    let mut expected = value;
    let object = expected.as_object_mut().unwrap();
    object.remove("webpage_url");
    object.remove("original_url");
    assert_eq!(written, expected);
}

#[test]
fn the_guard_bounds_bytes_and_free_space() {
    let dir = temp_dir();
    let limits = small_limits(plenty);
    // Flux (ni durée ni taille) : 1 Mio ; durée connue : 8 Mio.
    let mut stream = Guard::new(&limits, dir.path(), false);
    assert!(stream.progress(1 << 20, None).is_ok());
    assert_eq!(
        stream.progress((1 << 20) + 1, None).unwrap_err().code,
        FetchErrorCode::TooLarge
    );
    let mut video = Guard::new(&limits, dir.path(), true);
    assert!(video.progress(4 << 20, None).is_ok());
    assert_eq!(
        video.progress((8 << 20) + 1, None).unwrap_err().code,
        FetchErrorCode::TooLarge
    );
    // Taille annoncée trop grande : refusée dès la première ligne.
    let mut announced = Guard::new(&limits, dir.path(), false);
    assert_eq!(
        announced.progress(1, Some(9 << 20)).unwrap_err().code,
        FetchErrorCode::TooLarge
    );
    // Taille annoncée : le flux n'est plus « sans fin ».
    let mut sized = Guard::new(&limits, dir.path(), false);
    assert!(sized.progress(2 << 20, Some(4 << 20)).is_ok());
    // Relevé du dossier sans ligne de progression.
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::write(
        dir.path().join("sub/audio.webm.part"),
        vec![0u8; (1 << 20) + 10],
    )
    .unwrap();
    let quiet = Guard::new(&limits, dir.path(), false);
    assert_eq!(quiet.tick().unwrap_err().code, FetchErrorCode::TooLarge);
    assert_eq!(folder_size(dir.path()), (1 << 20) + 10);
    // Place : réserve, et ce qui reste à recevoir.
    fn two_mebibytes(_: &Path) -> Option<u64> {
        Some(2 << 20)
    }
    let tight = small_limits(two_mebibytes);
    let mut guard = Guard::new(&tight, dir.path(), true);
    assert!(guard.check_space(0).is_ok());
    assert!(guard.progress(0, Some(512 << 10)).is_ok());
    assert_eq!(
        guard.progress(0, Some(2 << 20)).unwrap_err().code,
        FetchErrorCode::NoSpace
    );
    // Place illisible : rien n'est refusé.
    fn unknown(_: &Path) -> Option<u64> {
        None
    }
    let blind = small_limits(unknown);
    assert!(
        Guard::new(&blind, dir.path(), true)
            .check_space(u64::MAX)
            .is_ok()
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn endless_streams_and_huge_files_are_stopped() {
    let dir = temp_dir();
    let (paths, log) = fake::tools(dir.path(), "tools", None);
    let limits = small_limits(plenty);
    for scenario in ["endless", "quiet-endless", "big"] {
        let before = fake::calls(&log).len();
        let work = dir.path().join(format!("work-{scenario}"));
        let stub = Stub::new(vec![paths.clone()], false);
        let url = format!("https://video.test/{scenario}");
        let (result, seen) = fetch_limited(&stub, &url, &work, &limits).await;
        let error = result.unwrap_err();
        assert_eq!(
            error.code,
            FetchErrorCode::TooLarge,
            "{scenario} : {}",
            error.detail
        );
        assert!(
            matches!(seen.first(), Some(FetchProgress::Info(_))),
            "{scenario}"
        );
        if scenario == "endless" {
            // Arrêté par la ligne qui dépasse : un peu plus de 1 Mio.
            let received = seen
                .iter()
                .filter_map(|progress| match progress {
                    FetchProgress::Downloading { received, .. } => Some(*received),
                    FetchProgress::Info(_) => None,
                })
                .max()
                .unwrap();
            assert!(received <= 1 << 20, "{received}");
        }
        assert_eq!(fake::calls(&log).len(), before + 2, "{scenario}");
        assert!(
            fake::files(&work).is_empty(),
            "{scenario} : {:?}",
            fake::files(&work)
        );
    }
    // Message pour la page, dans la langue du moment.
    assert!(!message(FetchErrorCode::TooLarge).starts_with("ytdlp."));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_full_disk_stops_the_download_before_and_during() {
    use std::sync::atomic::AtomicU64;
    static FREE: AtomicU64 = AtomicU64::new(0);
    fn free(_: &Path) -> Option<u64> {
        Some(FREE.load(Ordering::SeqCst))
    }
    let dir = temp_dir();
    let (paths, log) = fake::tools(dir.path(), "tools", None);
    let limits = small_limits(free);

    // Avant : sous la réserve, rien n'est téléchargé.
    FREE.store(512 << 10, Ordering::SeqCst);
    let work = dir.path().join("w1");
    let stub = Stub::new(vec![paths.clone()], false);
    let (result, seen) = fetch_limited(&stub, "https://video.test/slow", &work, &limits).await;
    assert_eq!(result.unwrap_err().code, FetchErrorCode::NoSpace);
    assert!(
        !seen
            .iter()
            .any(|progress| matches!(progress, FetchProgress::Downloading { .. }))
    );
    assert_eq!(fake::calls(&log).len(), 1);
    assert!(fake::files(&work).is_empty(), "{:?}", fake::files(&work));

    // Pendant : la place tombe sous la réserve au cours du téléchargement.
    FREE.store(u64::MAX / 2, Ordering::SeqCst);
    let work = dir.path().join("w2");
    let stub = Stub::new(vec![paths.clone()], false);
    let (_sender, cancel) = watch::channel(false);
    let on_progress = |progress: FetchProgress| {
        if matches!(progress, FetchProgress::Downloading { .. }) {
            FREE.store(0, Ordering::SeqCst);
        }
    };
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        fetch_with(
            &stub,
            "https://video.test/slow",
            &work,
            &limits,
            cancel,
            &on_progress,
        ),
    )
    .await
    .expect("place non relevée pendant le téléchargement");
    assert_eq!(result.unwrap_err().code, FetchErrorCode::NoSpace);
    assert!(fake::files(&work).is_empty(), "{:?}", fake::files(&work));
    assert!(!message(FetchErrorCode::NoSpace).starts_with("ytdlp."));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_download_never_extracts_the_address_again() {
    let dir = temp_dir();
    let (paths, log) = fake::tools(dir.path(), "tools", None);
    let stub = Stub::new(vec![paths], false);
    let work = dir.path().join("w");
    // Le faux réextrait comme le vrai si la fiche chargée a `webpage_url`
    // (flux sans fin, arrêté par la borne) ; sans elle, l'échec 403 remonte.
    let (result, _) = fetch_limited(
        &stub,
        "https://video.test/reextract",
        &work,
        &small_limits(plenty),
    )
    .await;
    let error = result.unwrap_err();
    assert_eq!(error.code, FetchErrorCode::Outdated, "{}", error.detail);
    let calls = fake::calls(&log);
    assert_eq!(calls.len(), 2);
    let load = calls[1]
        .iter()
        .skip_while(|arg| *arg != "--load-info-json")
        .nth(1)
        .unwrap();
    assert!(load.ends_with(LOAD_FILE), "{load}");
    assert!(fake::files(&work).is_empty(), "{:?}", fake::files(&work));
}

/// Processus encore vivant (ni fini, ni zombie).
#[cfg(target_os = "linux")]
fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| state != "Z" && state != "X")
    })
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn cancelling_stops_yt_dlp_and_deno_and_removes_partial_files() {
    let dir = temp_dir();
    let (paths, log) = fake::tools(dir.path(), "tools", None);
    let stub = Stub::new(vec![paths], false);
    let work = dir.path().join("work");
    let (sender, cancel) = watch::channel(false);
    let downloading = tokio::sync::Notify::new();
    let on_progress = |progress: FetchProgress| {
        if matches!(progress, FetchProgress::Downloading { .. }) {
            downloading.notify_one();
        }
    };
    let limits = FetchLimits::standard(None);
    let fetching = fetch_with(
        &stub,
        "https://video.test/slow",
        &work,
        &limits,
        cancel,
        &on_progress,
    );
    let mut pid_file = log.into_os_string();
    pid_file.push(".deno-pid");
    let pid_file = std::path::PathBuf::from(pid_file);
    let read_pids = || -> Vec<u32> {
        std::fs::read_to_string(&pid_file)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.trim().parse().ok())
            .collect()
    };
    let cancelling = async {
        downloading.notified().await;
        // Un `.part` existe pendant le téléchargement.
        assert!(
            fake::files(&work)
                .iter()
                .any(|name| name.ends_with(".part")),
            "{:?}",
            fake::files(&work)
        );
        // Défi du premier temps, puis Deno tenu ouvert pendant le
        // téléchargement : vivant au moment de l'annulation.
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while read_pids().len() < 2 && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let pids = read_pids();
        assert_eq!(pids.len(), 2, "{pids:?}");
        assert!(alive(pids[1]), "{pids:?}");
        sender.send(true).unwrap();
    };
    let started = std::time::Instant::now();
    let (result, ()) = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::join!(fetching, cancelling)
    })
    .await
    .expect("annulation trop lente");
    assert_eq!(result.unwrap_err().code, FetchErrorCode::Cancelled);
    // Avant la fin du téléchargement lent (10 s).
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert!(fake::files(&work).is_empty(), "{:?}", fake::files(&work));
    let pids = read_pids();
    assert_eq!(pids.len(), 2, "{pids:?}");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while pids.iter().any(|pid| alive(*pid)) && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !pids.iter().any(|pid| alive(*pid)),
        "Deno survit : {pids:?}"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn an_already_cancelled_fetch_launches_nothing() {
    let dir = temp_dir();
    let (paths, log) = fake::tools(dir.path(), "tools", None);
    let stub = Stub::new(vec![paths], false);
    let (sender, cancel) = watch::channel(false);
    sender.send(true).unwrap();
    let result = fetch_with(
        &stub,
        "https://video.test/ok",
        &dir.path().join("w"),
        &FetchLimits::standard(None),
        cancel,
        &|_| {},
    )
    .await;
    assert_eq!(result.unwrap_err().code, FetchErrorCode::Cancelled);
    assert!(fake::calls(&log).is_empty());
}

// --- Essai réel (réseau, YouTube) ---

/// Vraie vidéo sous Creative Commons BY (`M4gD1WSo5mA`, 721 s), vrais
/// outils de `scripts/record-ytdlp-fixtures.sh`. Un refus « not a bot »
/// (adresse IP de VPN ou de CI) est une panne externe, pas un échec du
/// code.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "réseau et YouTube : lancé à la demande"]
async fn ytdlp_live_youtube_cc_video() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tools/ytdlp-record");
    let tools = base.join("tools");
    let yt_dlp = tools.join(if cfg!(windows) {
        "yt-dlp/yt-dlp.exe"
    } else {
        "yt-dlp/yt-dlp_linux"
    });
    assert!(
        yt_dlp.is_file(),
        "lancer d'abord scripts/record-ytdlp-fixtures.sh (outils dans {})",
        tools.display()
    );
    let dir = temp_dir();
    let paths = ToolPaths {
        yt_dlp,
        yt_dlp_version: String::new(),
        deno: tools.join(if cfg!(windows) {
            "deno/deno.exe"
        } else {
            "deno/deno"
        }),
        deno_version: String::new(),
        deno_dir: dir.path().join("deno-cache"),
        yt_dlp_cache: dir.path().join("cache"),
        no_ffmpeg: dir.path().join("no-ffmpeg"),
    };
    for dir in [&paths.deno_dir, &paths.yt_dlp_cache, &paths.no_ffmpeg] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let stub = Stub::new(vec![paths], false);
    let work = dir.path().join("work");
    let started = std::time::Instant::now();
    let (_sender, cancel) = watch::channel(false);
    let last = Mutex::new(None);
    let result = fetch_with(
        &stub,
        "https://www.youtube.com/watch?v=M4gD1WSo5mA",
        &work,
        &FetchLimits::standard(Some(3600.0)),
        cancel,
        &|progress| {
            if let FetchProgress::Info(info) = &progress {
                println!("fiche : {info:?}");
            }
            *last.lock().unwrap() = Some(progress);
        },
    )
    .await;
    let audio = match result {
        Ok(audio) => audio,
        Err(error) if error.code == FetchErrorCode::BotCheck => {
            println!(
                "YouTube demande une connexion (panne externe) : {}",
                error.detail
            );
            return;
        }
        Err(error) => panic!("{:?} : {}", error.code, error.detail),
    };
    println!(
        "audio : {} ({} octets) en {:?}",
        audio.path.display(),
        std::fs::metadata(&audio.path).unwrap().len(),
        started.elapsed()
    );
    assert_eq!(audio.info.duration_s, Some(721.0));
    assert_eq!(audio.info.site, "youtube");
    assert!(matches!(
        audio.path.extension().and_then(|ext| ext.to_str()),
        Some("webm" | "m4a")
    ));
    assert_eq!(fake_files_count(&work), 1);
    // L'audio se décode, à la bonne durée.
    let decoded = media_audio_duration(&audio.path);
    assert!((decoded - 721.0).abs() < 2.0, "durée décodée {decoded} s");
}

fn fake_files_count(dir: &Path) -> usize {
    std::fs::read_dir(dir).unwrap().count()
}

/// Durée de l'audio décodé en 16 kHz (crate `media-audio`).
fn media_audio_duration(path: &Path) -> f64 {
    let mut samples: Vec<f32> = Vec::new();
    let summary = media_audio::decode_file(
        path,
        &media_audio::Options::default(),
        &std::sync::atomic::AtomicBool::new(false),
        &mut samples,
    )
    .unwrap();
    assert!(summary.complete);
    samples.len() as f64 / f64::from(media_audio::RATE)
}
