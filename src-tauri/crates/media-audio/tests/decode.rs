//! Essais du décodage sur les fixtures de `tests/data` (4 s de jfk dans
//! chaque format, faites par `scripts/make-audio-fixtures.sh`).

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use media_audio::{
    Codec, Container, ErrorCode, Limits, MediaInfo, Options, Progress, RATE, Sink, Summary,
    decode_bytes, decode_file, probe_file, resample,
};

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

fn options(threads: usize) -> Options {
    Options {
        threads,
        ..Options::default()
    }
}

fn decode(name: &str) -> (Vec<f32>, Summary) {
    let mut samples = Vec::new();
    let summary = decode_file(
        &data(name),
        &options(2),
        &AtomicBool::new(false),
        &mut samples,
    )
    .unwrap_or_else(|error| panic!("{name}: {error}"));
    (samples, summary)
}

/// La parole d'origine : le FLAC, sans perte, déjà à 16 kHz mono.
fn reference() -> Vec<f32> {
    decode("jfk.flac").0
}

fn energy(samples: &[f32]) -> f64 {
    samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum()
}

/// Meilleure corrélation normalisée de `sound` avec `reference`, décalé de
/// ±100 ms au plus (retard des codeurs), et son décalage. Les bords sont
/// écartés.
fn correlation(sound: &[f32], reference: &[f32]) -> (f64, i64) {
    let span = 1_600i64;
    let core = &reference[span as usize..reference.len() - span as usize];
    let mut best = (f64::MIN, 0);
    for lag in -span..=span {
        let start = span + lag;
        let Some(other) = sound.get(start as usize..start as usize + core.len()) else {
            continue;
        };
        let dot: f64 = core
            .iter()
            .zip(other)
            .map(|(&a, &b)| f64::from(a) * f64::from(b))
            .sum();
        let value = dot / (energy(core) * energy(other)).sqrt();
        if value > best.0 {
            best = (value, lag);
        }
    }
    best
}

/// Chaque format lisible : (fichier, conteneur, codec, gain attendu du
/// mélange mono, corrélation minimale).
const READABLE: [(&str, Container, Codec, f64, f64); 15] = [
    ("jfk.flac", Container::Flac, Codec::Flac, 1.0, 0.999),
    ("jfk-8k-u8.wav", Container::Wav, Codec::Pcm, 1.0, 0.9),
    ("jfk-stereo.mp3", Container::Mpeg, Codec::Mp3, 0.75, 0.97),
    ("jfk.ogg", Container::Ogg, Codec::Vorbis, 1.0, 0.95),
    ("jfk.opus", Container::Ogg, Codec::Opus, 1.0, 0.95),
    ("jfk-stereo.opus", Container::Ogg, Codec::Opus, 0.75, 0.95),
    (
        "jfk-yt251.webm",
        Container::Matroska,
        Codec::Opus,
        0.75,
        0.95,
    ),
    (
        "jfk-video.webm",
        Container::Matroska,
        Codec::Opus,
        1.0,
        0.95,
    ),
    ("jfk.m4a", Container::Mp4, Codec::Aac, 1.0, 0.93),
    ("jfk-yt140.m4a", Container::Mp4, Codec::Aac, 0.75, 0.93),
    ("jfk-yt139.m4a", Container::Mp4, Codec::HeAac, 0.75, 0.95),
    (
        "jfk-heaac-v2.m4a",
        Container::Mp4,
        Codec::HeAacV2,
        1.0,
        0.95,
    ),
    ("jfk-video.mp4", Container::Mp4, Codec::Aac, 1.0, 0.93),
    ("jfk-video.mov", Container::Mp4, Codec::Aac, 1.0, 0.93),
    ("jfk-video.mkv", Container::Matroska, Codec::Aac, 0.75, 0.93),
];

#[test]
fn every_format_decodes_to_the_spoken_words() {
    let reference = reference();
    assert_eq!(reference.len(), 4 * RATE as usize);
    for (name, container, codec, gain, least) in READABLE {
        let (samples, summary) = decode(name);
        let (value, lag) = correlation(&samples, &reference);
        let level = (energy(&samples) / energy(&reference)).sqrt();
        println!(
            "{name}: {} échantillons, corrélation {value:.5} à {lag}, gain {level:.3}, durée {:?}",
            samples.len(),
            summary.info.duration
        );
        assert_eq!(summary.info.container, container, "{name}");
        assert_eq!(summary.info.codec, codec, "{name}");
        assert!(summary.complete, "{name}");
        assert_eq!(summary.samples, samples.len() as u64, "{name}");
        // 4 s, à 120 ms près (amorce et remplissage des codeurs).
        assert!(
            samples.len().abs_diff(4 * RATE as usize) <= 1_920,
            "{name}: {}",
            samples.len()
        );
        assert!(value >= least, "{name}: corrélation {value} à {lag}");
        // Amorces retirées (Opus, MP3, Vorbis…), sauf celle de l'AAC en MP4,
        // que symphonia garde (23 ms, 46 ms en HE-AAC : sans effet sur une
        // transcription).
        let aac = matches!(codec, Codec::Aac | Codec::HeAac | Codec::HeAacV2);
        assert!(
            lag.abs() <= if aac { 800 } else { 16 },
            "{name}: décalage {lag}"
        );
        assert!((level - gain).abs() < 0.12, "{name}: gain {level}");
        assert!(samples.iter().all(|s| s.abs() <= 1.0), "{name}");
        // Un MP4 fragmenté n'annonce pas sa durée (écrite à 0).
        match summary.info.duration {
            None => assert!(name.contains("-yt13") || name.contains("-yt14"), "{name}"),
            Some(duration) => assert!(
                duration.abs_diff(Duration::from_secs(4)) < Duration::from_millis(60),
                "{name}: {duration:?}"
            ),
        }
    }
}

#[test]
fn the_wav_is_resampled_exactly_like_the_dictation() {
    // PCM 8 bits non signé : relu à la main, puis rééchantillonné d'un seul
    // tenant ; le flux par blocs, sur 1 ou 7 fils, rend le même son au bit
    // près.
    let bytes = std::fs::read(data("jfk-8k-u8.wav")).unwrap();
    let start = bytes.windows(4).position(|w| w == b"data").unwrap() + 8;
    let pcm: Vec<f32> = bytes[start..]
        .iter()
        .map(|&byte| (f32::from(byte) - 128.0) / 128.0)
        .collect();
    let expected = resample(&pcm, 8_000, RATE);
    for threads in [1, 7] {
        let mut samples = Vec::new();
        decode_file(
            &data("jfk-8k-u8.wav"),
            &options(threads),
            &AtomicBool::new(false),
            &mut samples,
        )
        .unwrap();
        assert_eq!(samples.len(), expected.len());
        assert!(
            samples
                .iter()
                .zip(&expected)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "{threads} fils"
        );
    }
}

#[test]
fn the_number_of_threads_does_not_change_the_sound() {
    let decode_with = |threads| {
        let mut samples = Vec::new();
        decode_file(
            &data("jfk-yt251.webm"),
            &options(threads),
            &AtomicBool::new(false),
            &mut samples,
        )
        .unwrap();
        samples
    };
    let one = decode_with(1);
    for threads in [3, 16] {
        let many = decode_with(threads);
        assert!(
            one.iter()
                .zip(&many)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        assert_eq!(one.len(), many.len());
    }
}

fn refusal(name: &str) -> media_audio::DecodeError {
    let mut samples = Vec::new();
    let error = decode_file(
        &data(name),
        &options(2),
        &AtomicBool::new(false),
        &mut samples,
    )
    .expect_err(name);
    assert!(samples.is_empty(), "{name}");
    error
}

#[test]
fn unsupported_files_are_refused_by_name() {
    let ac3 = refusal("refused-ac3.mkv");
    assert_eq!(ac3.code, ErrorCode::UnsupportedCodec);
    assert_eq!(ac3.container, Some(Container::Matroska));
    assert_eq!(ac3.codec, Some(Codec::Ac3));

    let avi = refusal("refused.avi");
    assert_eq!(avi.code, ErrorCode::UnsupportedContainer);
    assert_eq!(avi.container, Some(Container::Avi));

    let ts = refusal("refused.ts");
    assert_eq!(ts.code, ErrorCode::UnsupportedContainer);
    assert_eq!(ts.container, Some(Container::MpegTs));

    let video = refusal("refused-video-only.mkv");
    assert_eq!(video.code, ErrorCode::NoAudioTrack);
    assert_eq!(video.container, Some(Container::Matroska));

    // Les mêmes refus avant tout décodage.
    for (name, code) in [
        ("refused-ac3.mkv", ErrorCode::UnsupportedCodec),
        ("refused.avi", ErrorCode::UnsupportedContainer),
        ("refused.ts", ErrorCode::UnsupportedContainer),
        ("refused-video-only.mkv", ErrorCode::NoAudioTrack),
    ] {
        assert_eq!(
            probe_file(&data(name), &Limits::default())
                .unwrap_err()
                .code,
            code
        );
    }
}

#[test]
fn unreadable_files_are_told_apart() {
    let none = AtomicBool::new(false);
    let mut samples = Vec::new();
    let missing = decode_file(&data("absent.mp3"), &options(1), &none, &mut samples);
    assert_eq!(missing.unwrap_err().code, ErrorCode::CannotOpen);
    let folder = decode_file(&data(""), &options(1), &none, &mut samples);
    assert_eq!(folder.unwrap_err().code, ErrorCode::CannotOpen);

    let empty = decode_bytes(Vec::new(), None, &options(1), &none, &mut samples);
    assert_eq!(empty.unwrap_err().code, ErrorCode::Damaged);
    let text = decode_bytes(
        b"hello, world".repeat(100),
        Some("txt"),
        &options(1),
        &none,
        &mut samples,
    );
    let text = text.unwrap_err();
    assert_eq!(text.code, ErrorCode::UnsupportedContainer);
    assert_eq!(text.container, Some(Container::Unknown));
    assert!(samples.is_empty());
}

#[test]
fn the_header_is_read_without_decoding() {
    let info = probe_file(&data("jfk-yt139.m4a"), &Limits::default()).unwrap();
    assert_eq!(
        (info.container, info.codec, info.sample_rate),
        (Container::Mp4, Codec::HeAac, Some(22_050))
    );
    let info = probe_file(&data("jfk-yt251.webm"), &Limits::default()).unwrap();
    assert_eq!(
        (info.codec, info.sample_rate, info.channels),
        (Codec::Opus, Some(48_000), Some(2))
    );
    let info: MediaInfo = probe_file(&data("jfk-stereo.mp3"), &Limits::default()).unwrap();
    assert_eq!(info.channels, Some(2));
    // Les codes partent vers l'interface tels quels.
    assert_eq!(
        serde_json::to_string(&ErrorCode::UnsupportedCodec).unwrap(),
        "\"unsupportedCodec\""
    );
    assert_eq!(
        serde_json::to_string(&Codec::HeAacV2).unwrap(),
        "\"heAacV2\""
    );
}

#[test]
fn files_beyond_the_limits_are_refused() {
    let none = AtomicBool::new(false);
    let short = Options {
        limits: Limits {
            max_duration: Duration::from_secs(2),
            ..Limits::default()
        },
        threads: 1,
    };
    let mut samples = Vec::new();
    // Durée annoncée : refus avant le premier bloc.
    let error = decode_file(&data("jfk.opus"), &short, &none, &mut samples).unwrap_err();
    assert_eq!(error.code, ErrorCode::TooLong);
    assert!(samples.is_empty());
    assert_eq!(
        probe_file(&data("jfk.m4a"), &short.limits)
            .unwrap_err()
            .code,
        ErrorCode::TooLong
    );
    // Juste assez : accepté.
    let enough = Options {
        limits: Limits {
            max_duration: Duration::from_secs(4),
            ..Limits::default()
        },
        threads: 1,
    };
    assert!(decode_file(&data("jfk.opus"), &enough, &none, &mut samples).is_ok());

    let small = Options {
        limits: Limits {
            max_bytes: 10_000,
            ..Limits::default()
        },
        threads: 1,
    };
    let mut samples = Vec::new();
    let error = decode_file(&data("jfk.m4a"), &small, &none, &mut samples).unwrap_err();
    assert_eq!(error.code, ErrorCode::TooLarge);
    let bytes = std::fs::read(data("jfk.m4a")).unwrap();
    let error = decode_bytes(bytes, None, &small, &none, &mut samples).unwrap_err();
    assert_eq!(error.code, ErrorCode::TooLarge);
}

/// Récepteur qui compte les blocs et annule après le premier.
struct Cancelling<'a> {
    cancel: &'a AtomicBool,
    blocks: usize,
    progress: Vec<Progress>,
}

impl Sink for Cancelling<'_> {
    fn samples(&mut self, _block: &[f32]) -> io::Result<()> {
        self.blocks += 1;
        self.cancel.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn progress(&mut self, progress: Progress) {
        self.progress.push(progress);
    }
}

#[test]
fn cancelling_stops_between_two_packets() {
    let cancel = AtomicBool::new(false);
    let mut sink = Cancelling {
        cancel: &cancel,
        blocks: 0,
        progress: Vec::new(),
    };
    let error = decode_file(&data("jfk.m4a"), &options(1), &cancel, &mut sink).unwrap_err();
    assert_eq!(error.code, ErrorCode::Cancelled);
    assert_eq!(sink.blocks, 1);
    let progress = sink.progress[0];
    assert!(
        progress.decoded >= Duration::from_secs(1) && progress.decoded < Duration::from_secs(2)
    );
    assert!((progress.fraction().unwrap() - 0.25).abs() < 0.1);

    // Annulé d'avance : rien n'est décodé.
    let mut samples = Vec::new();
    let error = decode_file(
        &data("jfk.flac"),
        &options(1),
        &AtomicBool::new(true),
        &mut samples,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Cancelled);
    assert!(samples.is_empty());
}

/// Récepteur qui échoue (disque plein).
struct Full;

impl Sink for Full {
    fn samples(&mut self, _block: &[f32]) -> io::Result<()> {
        Err(io::Error::other("disk full"))
    }
}

#[test]
fn a_failing_sink_stops_the_decoding() {
    let error = decode_file(
        &data("jfk.ogg"),
        &options(1),
        &AtomicBool::new(false),
        &mut Full,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Output);
    assert!(error.detail.contains("disk full"));
}

/// Récepteur qui note la plus grande taille de bloc et la progression.
#[derive(Default)]
struct Watching {
    largest: usize,
    total: usize,
    progress: Vec<Duration>,
}

impl Sink for Watching {
    fn samples(&mut self, block: &[f32]) -> io::Result<()> {
        self.largest = self.largest.max(block.len());
        self.total += block.len();
        Ok(())
    }

    fn progress(&mut self, progress: Progress) {
        self.progress.push(progress.decoded);
    }
}

#[test]
fn blocks_stay_small_and_progress_moves_forward() {
    let mut sink = Watching::default();
    decode_file(
        &data("jfk-yt251.webm"),
        &options(1),
        &AtomicBool::new(false),
        &mut sink,
    )
    .unwrap();
    // Une seconde de son source par bloc et par fil, plus un paquet de
    // 20 ms au plus.
    assert!(sink.largest <= RATE as usize * 21 / 20, "{}", sink.largest);
    assert!(sink.progress.len() >= 4, "{:?}", sink.progress);
    assert!(sink.progress.windows(2).all(|pair| pair[0] <= pair[1]));
    let last = *sink.progress.last().unwrap();
    assert!(
        last.abs_diff(Duration::from_secs(4)) < Duration::from_millis(60),
        "{last:?}"
    );
}

#[test]
fn a_truncated_file_gives_what_came_before() {
    let none = AtomicBool::new(false);
    for name in [
        "jfk-stereo.mp3",
        "jfk.opus",
        "jfk-yt251.webm",
        "jfk-yt140.m4a",
        "jfk.flac",
    ] {
        let bytes = std::fs::read(data(name)).unwrap();
        let cut = bytes[..bytes.len() * 2 / 3].to_vec();
        let mut samples = Vec::new();
        let summary = decode_bytes(cut, None, &options(1), &none, &mut samples)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        // Un MP4 fragmenté n'annonce pas sa durée : sa coupure ne se voit
        // pas quand elle tombe entre deux fragments lisibles.
        if !name.contains("-yt140") {
            assert!(!summary.complete, "{name}");
        }
        let seconds = samples.len() as f64 / f64::from(RATE);
        assert!((1.5..3.5).contains(&seconds), "{name}: {seconds} s");
    }
}

/// Relecture 107 : paquets illisibles épars (AAC abîmé par endroits dans le
/// MP4) : du silence prend leur place, le son garde sa durée et ce qui suit
/// reste à sa place (horodatages, sous-titres).
#[test]
fn skipped_packets_are_replaced_by_silence() {
    let none = AtomicBool::new(false);
    for name in ["jfk.m4a", "jfk-yt140.m4a"] {
        let bytes = std::fs::read(data(name)).unwrap();
        let mut whole = Vec::new();
        decode_bytes(bytes.clone(), None, &options(1), &none, &mut whole).unwrap();
        // Six endroits de 7 octets entre 30 % et 70 % du fichier.
        let mut damaged = bytes.clone();
        let len = damaged.len();
        let mut at = len * 3 / 10;
        for _ in 0..6 {
            for byte in &mut damaged[at..at + 7] {
                *byte ^= 0xA5;
            }
            at += len / 12;
        }
        let mut samples = Vec::new();
        let summary = decode_bytes(damaged, None, &options(1), &none, &mut samples)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(summary.skipped_packets > 0, "{name}: no packet skipped");
        assert!(summary.complete, "{name}");
        assert_eq!(samples.len(), whole.len(), "{name}");
        // Le début, avant les paquets abîmés, est intact.
        let intact = whole.len() / 5;
        assert_eq!(samples[..intact], whole[..intact], "{name}");
    }
}

/// Paniques vues pendant un essai (même rattrapées par le crate).
static PANICS: AtomicUsize = AtomicUsize::new(0);

/// Aléa déterministe (xorshift).
struct Random(u64);

impl Random {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound as u64) as usize
    }
}

#[test]
fn damaged_and_random_files_never_panic_and_stay_bounded() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        PANICS.fetch_add(1, Ordering::Relaxed);
        previous(info);
    }));
    let none = AtomicBool::new(false);
    let bounded = Options {
        limits: Limits {
            max_duration: Duration::from_secs(8),
            ..Limits::default()
        },
        threads: 2,
    };
    let mut random = Random(0x9E37_79B9_7F4A_7C15);
    let (mut readable, mut refused) = (0, 0);
    let mut fixtures: Vec<&str> = READABLE.iter().map(|(name, ..)| *name).collect();
    fixtures.extend(["refused-ac3.mkv", "refused-video-only.mkv"]);
    for round in 0..1_700 {
        let name = fixtures[round % fixtures.len()];
        let mut bytes = std::fs::read(data(name)).unwrap();
        match round % 4 {
            // Quelques bits retournés, de préférence dans les en-têtes.
            0 | 1 => {
                let zone = if round % 4 == 0 {
                    2_048.min(bytes.len())
                } else {
                    bytes.len()
                };
                for _ in 0..=random.below(4) {
                    let at = random.below(zone);
                    bytes[at] ^= 1 << random.below(8);
                }
            }
            // Une plage d'octets quelconques.
            2 => {
                let at = random.below(bytes.len());
                let end = (at + 1 + random.below(256)).min(bytes.len());
                for byte in &mut bytes[at..end] {
                    *byte = random.below(256) as u8;
                }
            }
            // Coupé n'importe où.
            _ => bytes.truncate(random.below(bytes.len())),
        }
        let mut samples = Vec::new();
        match decode_bytes(bytes, None, &bounded, &none, &mut samples) {
            Ok(summary) => {
                readable += 1;
                assert_eq!(summary.samples, samples.len() as u64);
                assert!(
                    samples.len() <= 9 * RATE as usize,
                    "{name}: {}",
                    samples.len()
                );
                assert!(samples.iter().all(|s| s.abs() <= 1.0), "{name}");
            }
            Err(error) => {
                refused += 1;
                assert!(
                    !matches!(
                        error.code,
                        ErrorCode::Cancelled | ErrorCode::Output | ErrorCode::TooLarge
                    ),
                    "{name}: {error}"
                );
            }
        }
    }
    // Des octets tout à fait quelconques.
    for length in [1, 7, 188, 1_000, 4_096, 65_536] {
        let bytes: Vec<u8> = (0..length).map(|_| random.below(256) as u8).collect();
        let mut samples = Vec::new();
        let _ = decode_bytes(bytes, Some("mp3"), &bounded, &none, &mut samples);
    }
    let _ = std::panic::take_hook();
    println!("{readable} lus, {refused} refusés");
    assert!(
        readable > 100 && refused > 100,
        "{readable} lus, {refused} refusés"
    );
    assert_eq!(PANICS.load(Ordering::Relaxed), 0, "paniques");
}

/// Vrais formats de YouTube (même vidéo, sous licence CC BY, relevés par la
/// recherche du 14 nov. 2026 dans `.tools/research-transcription/media`,
/// hors du dépôt) : 139 (HE-AAC), 140 (AAC-LC), 249 et 251 (Opus). Lancé à
/// la main par `cargo test -p media-audio -- --ignored`.
#[test]
#[ignore = "fichiers hors du dépôt"]
fn real_youtube_formats() {
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../.tools/research-transcription/media");
    let decoded: Vec<(&str, Vec<f32>, Summary)> =
        ["yt-140.m4a", "yt-139.m4a", "yt-249.webm", "yt-251.webm"]
            .into_iter()
            .map(|name| {
                let mut samples = Vec::new();
                let summary = decode_file(
                    &folder.join(name),
                    &options(4),
                    &AtomicBool::new(false),
                    &mut samples,
                )
                .unwrap_or_else(|error| panic!("{name}: {error}"));
                (name, samples, summary)
            })
            .collect();
    // Le HE-AAC (139) contre l'AAC-LC (140), l'Opus 249 contre le 251 : mêmes
    // familles de codeurs chez YouTube, mêmes débuts.
    for (index, against) in [(0, 0), (1, 0), (2, 3), (3, 3)] {
        let (name, samples, summary) = &decoded[index];
        let (value, lag) = correlation(samples, &decoded[against].1);
        println!(
            "{name}: {:?} {:?}, {:.2} s, corrélation avec {} : {value:.5} à {lag}",
            summary.info.container,
            summary.info.codec,
            samples.len() as f64 / f64::from(RATE),
            decoded[against].0
        );
        assert!(summary.complete, "{name}");
        assert!(value > 0.95, "{name}: {value}");
    }
    let (value, lag) = correlation(&decoded[3].1, &decoded[0].1);
    println!("yt-251.webm contre yt-140.m4a : {value:.5} à {lag}");
    assert_eq!(decoded[1].2.info.codec, Codec::HeAac);
}

/// Récepteur qui ne garde rien : la mesure ne compte que le décodage.
#[derive(Default)]
struct Counting(u64);

impl Sink for Counting {
    fn samples(&mut self, block: &[f32]) -> io::Result<()> {
        self.0 += block.len() as u64;
        Ok(())
    }
}

/// Mesure du décodage de longs fichiers (chemins séparés par « : » dans
/// `MEDIA_AUDIO_MEASURE`), sur 1, 4 fils et tous les cœurs. À lancer en
/// mode optimisé : `cargo test --release -p media-audio --test decode
/// measure -- --ignored --nocapture`.
#[test]
#[ignore = "mesure manuelle"]
fn measure_long_files() {
    let paths = std::env::var("MEDIA_AUDIO_MEASURE").expect("MEDIA_AUDIO_MEASURE");
    for path in paths.split(':') {
        for threads in [1, 4, 0] {
            let started = std::time::Instant::now();
            let mut sink = Counting::default();
            let summary = decode_file(
                &PathBuf::from(path),
                &options(threads),
                &AtomicBool::new(false),
                &mut sink,
            )
            .unwrap();
            println!(
                "{path} ({:?}, {:?}) : {:.0} s de son en {:.2} s, {threads} fil(s)",
                summary.info.container,
                summary.info.codec,
                summary.duration().as_secs_f64(),
                started.elapsed().as_secs_f64()
            );
        }
    }
    // Pic de mémoire du processus (Linux).
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines().filter(|line| line.starts_with("VmHWM")) {
            println!("{line}");
        }
    }
}

#[test]
fn the_first_decodable_track_is_taken() {
    // AC-3 par défaut (refusée), jfk en AAC, puis un son de 440 Hz en MP3.
    let (samples, summary) = decode("jfk-ac3-then-aac.mkv");
    assert_eq!(summary.info.codec, Codec::Aac);
    let reference = reference();
    let (value, _) = correlation(&samples, &reference[..samples.len().min(reference.len())]);
    assert!(value > 0.9, "corrélation {value}");
}

#[test]
fn an_implausible_sample_rate_is_a_damaged_file() {
    // L'en-tête du WAV annonce 100 Hz : 160 échantillons rendus par
    // échantillon lu, si rien ne l'arrêtait.
    let mut bytes = std::fs::read(data("jfk-8k-u8.wav")).unwrap();
    let format = bytes.windows(4).position(|w| w == b"fmt ").unwrap();
    bytes[format + 12..format + 16].copy_from_slice(&100u32.to_le_bytes());
    bytes[format + 16..format + 20].copy_from_slice(&100u32.to_le_bytes());
    let mut samples = Vec::new();
    let error = decode_bytes(
        bytes,
        None,
        &options(1),
        &AtomicBool::new(false),
        &mut samples,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Damaged, "{error}");
    assert!(samples.is_empty());
}
