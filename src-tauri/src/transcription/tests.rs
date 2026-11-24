//! Essais de la transcription longue : vrai décodage (WAV écrits ici), faux
//! moteur (`Engine::with_loader`), faux détecteur de parole (au niveau
//! sonore), faux yt-dlp, vraie base.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::watch;

use super::job::detect_language;
use super::paragraphs::Segment;
use super::ytdlp::{FetchError, FetchErrorCode, FetchProgress, FetchedAudio, VideoInfo};
use super::*;
use crate::dictation::engine::{Engine, IDLE, ModelSpec, Piece, Recognizer, Request, Timed};
use crate::store::transcriptions as rows;

const RATE: usize = 16_000;

// --- Sons ------------------------------------------------------------------

/// « Parole » : bruit d'amplitude 0,2.
fn speech(seconds: f64) -> Vec<f32> {
    let mut state = 99u32;
    (0..(seconds * RATE as f64).round() as usize)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            ((state >> 16) as f32 / 32_768.0 - 1.0) * 0.2
        })
        .collect()
}

fn silence(seconds: f64) -> Vec<f32> {
    vec![0.0; (seconds * RATE as f64).round() as usize]
}

/// WAV 16 kHz mono 16 bits.
fn write_wav(path: &Path, samples: &[f32]) {
    let data = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&(RATE as u32).to_le_bytes());
    bytes.extend_from_slice(&(RATE as u32 * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&((sample * 32_767.0).round() as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// Dix phrases de 9,5 s (pauses de 0,5 s), une minute de silence, une
/// phrase de 19,5 s : 180 s. Débuts des phrases, en secondes.
fn lecture() -> (Vec<f32>, Vec<f64>) {
    let mut samples = Vec::new();
    let mut starts = Vec::new();
    for _ in 0..10 {
        starts.push(samples.len() as f64 / RATE as f64);
        samples.extend(speech(9.5));
        samples.extend(silence(0.5));
    }
    samples.extend(silence(60.0));
    starts.push(samples.len() as f64 / RATE as f64);
    samples.extend(speech(19.5));
    samples.extend(silence(0.5));
    (samples, starts)
}

// --- Faux moteur -----------------------------------------------------------

/// Appel reçu par le faux moteur.
#[derive(Clone, Debug, PartialEq)]
struct Call {
    /// « piece » (transcription longue) ou « dictation ».
    kind: &'static str,
    samples: usize,
    language: Option<String>,
    prompt: Option<String>,
    /// Premier échantillon « parlé » du morceau.
    first_loud: Option<usize>,
}

/// Texte du n-ième morceau.
type TextOf = fn(usize) -> String;

#[derive(Default)]
struct Script {
    calls: Mutex<Vec<Call>>,
    /// Texte du n-ième morceau.
    text: Mutex<Option<TextOf>>,
    /// Le premier morceau attend ce feu vert.
    gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    /// Échec au n-ième morceau.
    fail_at: Mutex<Option<usize>>,
    /// Le modèle ne se charge plus (fichiers supprimés).
    unloadable: std::sync::atomic::AtomicBool,
}

struct Scribe(Arc<Script>);

fn loud(samples: &[f32]) -> Option<(usize, usize)> {
    let first = samples.iter().position(|s| s.abs() > 0.01)?;
    let last = samples.iter().rposition(|s| s.abs() > 0.01)?;
    Some((first, last))
}

impl Recognizer for Scribe {
    fn transcribe(&mut self, samples: &[f32], request: &Request) -> Result<String, String> {
        self.0.calls.lock().push(Call {
            kind: "dictation",
            samples: samples.len(),
            language: request.language.clone(),
            prompt: request.initial_prompt(),
            first_loud: None,
        });
        Ok("dictation".into())
    }

    fn transcribe_timed(&mut self, samples: &[f32], request: &Request) -> Result<Piece, String> {
        let gate = self.0.gate.lock().take();
        if let Some(gate) = gate {
            let _ = gate.recv_timeout(Duration::from_secs(20));
        }
        let index = {
            let mut calls = self.0.calls.lock();
            calls.push(Call {
                kind: "piece",
                samples: samples.len(),
                language: request.language.clone(),
                prompt: request.initial_prompt(),
                first_loud: loud(samples).map(|(first, _)| first),
            });
            calls.iter().filter(|call| call.kind == "piece").count() - 1
        };
        if *self.0.fail_at.lock() == Some(index) {
            return Err("Transcription failed: boom".into());
        }
        let text = self
            .0
            .text
            .lock()
            .map_or_else(|| format!("Phrase {index}."), |f| f(index));
        let (first, last) = loud(samples).unwrap_or((0, samples.len()));
        Ok(Piece::from_parts(
            &text,
            vec![Timed {
                start: first as f32 / RATE as f32,
                end: last as f32 / RATE as f32,
                text: text.clone(),
            }],
            samples.len() as f32 / RATE as f32,
        ))
    }
}

/// Détecteur au niveau sonore ; compte ses remises à zéro.
struct Loudness(Arc<AtomicUsize>);

impl SpeechDetector for Loudness {
    fn probability(&mut self, frame: &[f32]) -> Result<f32, String> {
        Ok(if frame.iter().any(|s| s.abs() > 0.01) {
            0.9
        } else {
            0.05
        })
    }

    fn reset(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

// --- Faux yt-dlp -----------------------------------------------------------

#[derive(Clone)]
enum Fetch {
    /// Écrit ce son dans le dossier de travail (ou ailleurs : `outside`).
    Audio {
        samples: Arc<Vec<f32>>,
        outside: Option<PathBuf>,
    },
    Fail(FetchErrorCode),
    /// Attend l'annulation, après avoir écrit un fichier partiel.
    WaitCancel,
}

struct FakeFetcher {
    plan: Mutex<Fetch>,
    seen_dirs: Mutex<Vec<PathBuf>>,
}

fn info() -> VideoInfo {
    VideoInfo {
        title: Some("CopyrightX: Lecture 3.2".into()),
        channel: Some("Berkman Klein Center".into()),
        duration_s: Some(180.0),
        webpage_url: "https://example.org/watch?v=abc".into(),
        site: "example".into(),
    }
}

impl Fetcher for FakeFetcher {
    fn fetch<'a>(
        &'a self,
        _url: &'a str,
        work_dir: &'a Path,
        max_duration_s: Option<f64>,
        mut cancel: watch::Receiver<bool>,
        progress: &'a (dyn Fn(FetchProgress) + Send + Sync),
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchedAudio, FetchError>> + Send + 'a>,
    > {
        Box::pin(async move {
            assert_eq!(max_duration_s, Some(6.0 * 3600.0));
            assert!(work_dir.is_dir(), "{}", work_dir.display());
            self.seen_dirs.lock().push(work_dir.to_owned());
            let plan = self.plan.lock().clone();
            match plan {
                Fetch::Audio { samples, outside } => {
                    progress(FetchProgress::Info(info()));
                    progress(FetchProgress::Downloading {
                        received: 10,
                        total: Some(100),
                    });
                    let path = outside.unwrap_or_else(|| work_dir.join("audio.wav"));
                    write_wav(&path, &samples);
                    Ok(FetchedAudio { path, info: info() })
                }
                Fetch::Fail(code) => Err(FetchError {
                    code,
                    detail: "ERROR: test".into(),
                }),
                Fetch::WaitCancel => {
                    std::fs::write(work_dir.join("audio.part"), b"partial").unwrap();
                    let _ = cancel.wait_for(|cancelled| *cancelled).await;
                    Err(FetchError {
                        code: FetchErrorCode::Cancelled,
                        detail: String::new(),
                    })
                }
            }
        })
    }
}

// --- Banc ------------------------------------------------------------------

struct Bench {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: Store,
    transcription: Transcription,
    engine: Engine,
    script: Arc<Script>,
    fetcher: Arc<FakeFetcher>,
    events: Arc<Mutex<Vec<TranscriptionStatus>>>,
    resets: Arc<AtomicUsize>,
    detectors: Arc<AtomicUsize>,
}

struct Options {
    detector: bool,
    support: fn(SpeechEngine) -> Result<(), String>,
    /// Écart des événements de progression.
    gap: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            detector: true,
            support: |_| Ok(()),
            gap: Duration::ZERO,
        }
    }
}

/// Faux de récupération partagé avec le banc.
struct SharedFetcher(Arc<FakeFetcher>);

impl Fetcher for SharedFetcher {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        work_dir: &'a Path,
        max_duration_s: Option<f64>,
        cancel: watch::Receiver<bool>,
        progress: &'a (dyn Fn(FetchProgress) + Send + Sync),
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchedAudio, FetchError>> + Send + 'a>,
    > {
        self.0
            .fetch(url, work_dir, max_duration_s, cancel, progress)
    }
}

fn bench_with(options: Options) -> Bench {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    let store = Store::open(&root.join("chatonlm.sqlite")).unwrap();
    let script = Arc::new(Script::default());
    let loaded = script.clone();
    let engine = Engine::with_loader(
        Box::new(move |_: &ModelSpec| {
            if loaded.unloadable.load(Ordering::SeqCst) {
                return Err("Couldn't load the speech model: no such file".into());
            }
            Ok(Box::new(Scribe(loaded.clone())) as Box<dyn Recognizer>)
        }),
        IDLE,
    );
    let fetcher = Arc::new(FakeFetcher {
        plan: Mutex::new(Fetch::Fail(FetchErrorCode::Failed)),
        seen_dirs: Mutex::new(Vec::new()),
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorded = events.clone();
    let resets = Arc::new(AtomicUsize::new(0));
    let detectors = Arc::new(AtomicUsize::new(0));
    let (counted, made) = (resets.clone(), detectors.clone());
    let with_detector = options.detector;
    let mut services = Services::new(
        engine.clone(),
        Box::new(SharedFetcher(fetcher.clone())),
        root.join("work"),
        move |status: &TranscriptionStatus| recorded.lock().push(status.clone()),
    );
    services.detector = Box::new(move || {
        made.fetch_add(1, Ordering::SeqCst);
        if with_detector {
            Ok(Box::new(Loudness(counted.clone())) as Box<dyn SpeechDetector + Send>)
        } else {
            Err("no ONNX Runtime".into())
        }
    });
    services.support = options.support;
    services.emit_gap = options.gap;
    Bench {
        _dir: dir,
        root,
        store,
        transcription: Transcription::new(services),
        engine,
        script,
        fetcher,
        events,
        resets,
        detectors,
    }
}

fn bench() -> Bench {
    bench_with(Options::default())
}

impl Bench {
    /// Réglages de la dictée : Whisper Base, langue automatique ; `installed` :
    /// modèles présents.
    fn prepared(&self, installed: &'static [&'static str]) -> Prepared {
        Prepared {
            speech_model: "whisper-base".into(),
            language: "auto".into(),
            dictionary: vec!["ChatonLM".into()],
            models_dir: self.root.join("speech-models"),
            installed: Box::new(move |model| installed.contains(&model.id)),
        }
    }

    /// Fichier remis par le sélecteur.
    fn file(&self, name: &str, samples: &[f32]) -> String {
        let path = self.root.join(name);
        write_wav(&path, samples);
        let path = path.to_string_lossy().into_owned();
        self.transcription.grant([path.clone()]);
        path
    }

    fn start(&self, request: TranscriptionRequest) -> AppResult<String> {
        self.start_with(request, &["whisper-base", "parakeet-tdt-0.6b-v3"])
    }

    fn start_with(
        &self,
        request: TranscriptionRequest,
        installed: &'static [&'static str],
    ) -> AppResult<String> {
        self.transcription
            .start(self.store.clone(), self.prepared(installed), request)
    }

    async fn finished(&self) -> TranscriptionJob {
        // 2 min réelles : sous la charge de la suite complète, décoder et
        // couper 3 min de son peut dépasser 30 s.
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        while std::time::Instant::now() < deadline {
            if let Some(job) = self.transcription.status().job
                && job.stage.is_final()
                && !self.transcription.shared().inner.lock().running
            {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "transcription not finished: {:?}",
            self.transcription.status()
        );
    }

    fn pieces(&self) -> Vec<Call> {
        self.script
            .calls
            .lock()
            .iter()
            .filter(|call| call.kind == "piece")
            .cloned()
            .collect()
    }

    fn work_root_is_empty(&self) -> bool {
        let root = self.root.join("work");
        !root.exists() || std::fs::read_dir(root).unwrap().next().is_none()
    }

    fn row(&self, id: &str) -> Option<TranscriptionDetail> {
        let id = id.to_owned();
        self.store
            .blocking(move |connection| rows::get(connection, &id))
            .unwrap()
    }

    /// Passages tels que la page les reconstruit depuis les événements.
    fn segments_from_events(&self) -> Vec<Segment> {
        let mut segments: Vec<Segment> = Vec::new();
        for status in self.events.lock().iter() {
            let job = status.job.as_ref().unwrap();
            assert_eq!(
                job.segments_from as usize,
                segments.len(),
                "an event skipped segments"
            );
            segments.extend(job.segments.iter().cloned());
        }
        segments
    }
}

fn file_request(path: &str) -> TranscriptionRequest {
    TranscriptionRequest {
        source: Source::File { path: path.into() },
        model: None,
        language: None,
    }
}

fn url_request() -> TranscriptionRequest {
    TranscriptionRequest {
        source: Source::Url {
            url: " https://example.org/watch?v=abc ".into(),
        },
        model: None,
        language: None,
    }
}

// --- Essais ----------------------------------------------------------------

/// Trois minutes : coupures dans les pauses (chaque passage commence au début
/// d'une phrase, jamais au milieu), minute de silence sautée, passages
/// décalés au temps du fichier, enregistrés, annoncés sans trou ; audio de
/// travail effacé.
#[tokio::test]
async fn a_long_file_is_cut_in_pauses_and_silence_is_skipped() {
    let bench = bench();
    let (samples, starts) = lecture();
    let path = bench.file("cours.wav", &samples);
    let id = bench.start(file_request(&path)).unwrap();
    let job = bench.finished().await;
    assert_eq!(job.stage, Stage::Done, "{:?}", job.error);
    assert_eq!(job.id, id);
    assert_eq!(job.title.as_deref(), Some("cours.wav"));
    assert_eq!(job.model, "whisper-base");
    assert_eq!(job.duration_s, Some(180.0));
    assert_eq!(job.done_s, 180.0);

    let pieces = bench.pieces();
    // Aucun morceau muet n'arrive au moteur ; chacun, sauf le premier,
    // commence par un silence (coupé dans une pause).
    assert_eq!(pieces[0].first_loud, Some(0));
    assert!(
        pieces[1..]
            .iter()
            .all(|call| call.first_loud.is_some_and(|first| first > 0)),
        "{pieces:?}"
    );
    let sent: usize = pieces.iter().map(|call| call.samples).sum();
    assert!(sent < samples.len() - 30 * RATE, "silence not skipped");
    assert!(pieces.iter().all(|call| call.samples <= 45 * RATE));
    // Dictionnaire dans la consigne de Whisper.
    assert_eq!(pieces[0].prompt.as_deref(), Some("Glossary: ChatonLM."));

    let segments = job.segments.clone();
    assert_eq!(segments.len(), pieces.len());
    for segment in &segments {
        assert!(
            starts
                .iter()
                .any(|start| (segment.start - start).abs() < 1e-3),
            "passage at {} s, not at the start of a sentence",
            segment.start
        );
    }
    assert!((segments.last().unwrap().start - 160.0).abs() < 1e-3);
    // Le dernier événement porte tout ; aucun trou dans les envois.
    assert_eq!(bench.segments_from_events(), segments);
    assert_eq!(bench.transcription.status().job.unwrap().segments, segments);

    let row = bench.row(&id).unwrap();
    assert_eq!(row.item.state, TranscriptionState::Done);
    assert_eq!(row.segments, segments);
    assert_eq!(row.item.duration_s, Some(180.0));
    assert!(row.text.starts_with("Phrase 0."), "{}", row.text);
    assert!(bench.work_root_is_empty());
    // Une seule transcription : le modèle a été chargé une fois.
    assert_eq!(bench.engine.loaded().as_deref(), Some("whisper-base"));
}

/// Paragraphe français d'une quarantaine de mots (un morceau de 40 s).
const FRENCH: &str = "Bonjour à tous, aujourd'hui nous allons parler de l'histoire du droit \
    d'auteur et de la façon dont il a changé au cours du siècle dernier. La première chose à \
    savoir, c'est que la loi était très différente, et que la plupart des gens n'y pensaient pas.";

/// Paragraphe anglais d'une quarantaine de mots.
const ENGLISH: &str = "Hello everyone, today we are going to talk about the history of \
    copyright and how it has changed over the last century. The first thing you need to know is \
    that the law was very different, and most people did not think about it at all.";

/// Paragraphe danois : ses mots outils (« i », « at », « for », « her »)
/// faisaient croire à de l'anglais (relecture 107).
const DANISH: &str = "Jeg har talt med min bror i går, og han siger at vejret i København \
    ikke er så godt, men at byen stadig er meget smuk. Vi skal tage derhen sammen i næste uge, \
    for han vil se museet og jeg vil mest gå en tur.";

/// Deux fois la leçon : huit morceaux parlés au moins.
fn two_lectures() -> Vec<f32> {
    let (mut samples, _) = lecture();
    samples.extend(lecture().0);
    samples
}

/// Whisper sans langue : figée quand deux morceaux parlés donnent la même
/// langue sûre, puis gardée (enregistrée) ; Parakeet ne reçoit jamais de
/// langue ; une langue demandée est passée telle quelle.
#[tokio::test]
async fn whisper_keeps_a_language_once_two_chunks_agree() {
    let bench = bench();
    *bench.script.text.lock() = Some(|_| FRENCH.into());
    let path = bench.file("cours.wav", &two_lectures());
    let id = bench.start(file_request(&path)).unwrap();
    let job = bench.finished().await;
    assert_eq!(job.stage, Stage::Done);
    let languages: Vec<Option<String>> = bench.pieces().into_iter().map(|c| c.language).collect();
    assert!(languages.len() > 3, "{languages:?}");
    assert_eq!(languages[..2], [None, None]);
    assert!(
        languages[2..].iter().all(|l| l.as_deref() == Some("fr")),
        "{languages:?}"
    );
    assert_eq!(job.language.as_deref(), Some("fr"));
    assert_eq!(bench.row(&id).unwrap().item.language.as_deref(), Some("fr"));

    // Parakeet, langue « auto » : jamais de langue.
    bench.script.calls.lock().clear();
    let mut request = file_request(&path);
    request.model = Some("parakeet-tdt-0.6b-v3".into());
    request.language = Some("auto".into());
    bench.start(request).unwrap();
    let job = bench.finished().await;
    assert_eq!(job.engine, Some(SpeechEngine::Parakeet));
    assert!(bench.pieces().iter().all(|call| call.language.is_none()));

    // Whisper, langue demandée : passée à chaque morceau, jamais redétectée.
    bench.script.calls.lock().clear();
    let mut request = file_request(&path);
    request.language = Some("es".into());
    bench.start(request).unwrap();
    bench.finished().await;
    assert!(
        bench
            .pieces()
            .iter()
            .all(|call| call.language.as_deref() == Some("es"))
    );
}

/// Relecture 107 : une vidéo danoise (langue que la détection sur le texte
/// ne sait pas dire) reste en « auto » d'un bout à l'autre, au lieu d'être
/// figée en anglais ; rien n'est enregistré comme langue.
#[tokio::test]
async fn an_unsure_language_is_never_frozen() {
    let bench = bench();
    *bench.script.text.lock() = Some(|_| DANISH.into());
    let path = bench.file("dansk.wav", &two_lectures());
    let id = bench.start(file_request(&path)).unwrap();
    let job = bench.finished().await;
    assert_eq!(job.stage, Stage::Done);
    let pieces = bench.pieces();
    assert!(pieces.len() > 4);
    assert!(
        pieces.iter().all(|call| call.language.is_none()),
        "{pieces:?}"
    );
    assert_eq!(job.language, None);
    assert_eq!(bench.row(&id).unwrap().item.language, None);
}

/// Relecture 107 : un générique en anglais au début d'une vidéo française ne
/// fige pas l'anglais : le français l'est quand deux morceaux s'accordent.
#[tokio::test]
async fn an_english_opening_does_not_freeze_english() {
    let bench = bench();
    *bench.script.text.lock() = Some(|index| if index == 0 { ENGLISH } else { FRENCH }.into());
    let path = bench.file("cours.wav", &two_lectures());
    bench.start(file_request(&path)).unwrap();
    let job = bench.finished().await;
    assert_eq!(job.stage, Stage::Done);
    let languages: Vec<Option<String>> = bench.pieces().into_iter().map(|c| c.language).collect();
    assert_eq!(languages[..3], [None, None, None], "{languages:?}");
    assert!(
        languages[3..].iter().all(|l| l.as_deref() == Some("fr")),
        "{languages:?}"
    );
    assert_eq!(job.language.as_deref(), Some("fr"));
}

#[test]
fn languages_are_detected_from_text_only_when_sure() {
    assert_eq!(detect_language(ENGLISH).as_deref(), Some("en"));
    assert_eq!(detect_language(FRENCH).as_deref(), Some("fr"));
    assert_eq!(detect_language(DANISH), None);
    // Trop court pour être sûr.
    assert_eq!(
        detect_language("The cat and the children are in the house with us."),
        None
    );
    assert_eq!(detect_language("ChatonLM"), None);
}

/// Annulée pendant un morceau : arrêt avant le suivant, ce qui est fait reste
/// (« cancelled »), audio de travail effacé ; une autre peut commencer.
#[tokio::test]
async fn cancel_stops_between_chunks_and_removes_the_work_audio() {
    let bench = bench();
    let (go, gate) = std::sync::mpsc::channel();
    *bench.script.gate.lock() = Some(gate);
    let (samples, _) = lecture();
    let path = bench.file("cours.wav", &samples);
    let id = bench.start(file_request(&path)).unwrap();
    // Premier morceau en cours.
    for _ in 0..2_000 {
        if bench.transcription.status().job.unwrap().stage == Stage::Transcribing
            && bench.root.join("work").join(&id).join("audio.f32").exists()
            && bench.engine_busy()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    bench.transcription.cancel();
    go.send(()).unwrap();
    let job = bench.finished().await;
    assert_eq!(job.stage, Stage::Cancelled);
    assert_eq!(bench.pieces().len(), 1);
    assert!(bench.work_root_is_empty());
    let row = bench.row(&id).unwrap();
    assert_eq!(row.item.state, TranscriptionState::Cancelled);
    assert_eq!(row.segments.len(), 1);
    // La suivante démarre.
    bench.start(file_request(&path)).unwrap();
    assert_eq!(bench.finished().await.stage, Stage::Done);
}

/// « Erase my data » : la transcription en cours est annulée et attendue,
/// son audio de travail effacé, puis son état oublié (la page reçoit un état
/// vide) ; une autre peut commencer ensuite.
#[tokio::test(flavor = "multi_thread")]
async fn forgetting_cancels_waits_and_clears_the_state() {
    let bench = bench();
    let (go, gate) = std::sync::mpsc::channel();
    *bench.script.gate.lock() = Some(gate);
    let (samples, _) = lecture();
    let path = bench.file("cours.wav", &samples);
    bench.start(file_request(&path)).unwrap();
    for _ in 0..2_000 {
        if bench.transcription.status().job.unwrap().stage == Stage::Transcribing
            && bench.engine_busy()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let transcription = bench.transcription.clone();
    let forgetting =
        tokio::spawn(async move { transcription.forget(Duration::from_secs(30)).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!forgetting.is_finished(), "the running piece is waited for");
    go.send(()).unwrap();
    forgetting.await.unwrap();
    assert_eq!(bench.transcription.status().job, None);
    assert!(bench.events.lock().last().unwrap().job.is_none());
    assert!(!bench.transcription.shared().inner.lock().running);
    assert!(bench.work_root_is_empty());
    bench.start(file_request(&path)).unwrap();
    assert_eq!(bench.finished().await.stage, Stage::Done);
}

impl Bench {
    /// Le faux moteur tient le verrou (un morceau en cours).
    fn engine_busy(&self) -> bool {
        self.script.gate.lock().is_none()
    }
}

/// Adresse : fiche et téléchargement montrés, fiche enregistrée, audio
/// récupéré transcrit puis effacé (même rangé hors du dossier de travail).
#[tokio::test]
async fn the_fetched_audio_is_transcribed_then_deleted() {
    let bench = bench();
    let (samples, _) = lecture();
    let elsewhere = bench.root.join("elsewhere.wav");
    for outside in [None, Some(elsewhere.clone())] {
        *bench.fetcher.plan.lock() = Fetch::Audio {
            samples: Arc::new(samples.clone()),
            outside: outside.clone(),
        };
        bench.events.lock().clear();
        let id = bench.start(url_request()).unwrap();
        let job = bench.finished().await;
        assert_eq!(job.stage, Stage::Done, "{:?}", job.error);
        assert_eq!(job.origin, "https://example.org/watch?v=abc");
        assert_eq!(job.title.as_deref(), Some("CopyrightX: Lecture 3.2"));
        assert_eq!(job.channel.as_deref(), Some("Berkman Klein Center"));
        assert_eq!(job.site.as_deref(), Some("example"));
        let stages: Vec<Stage> = bench
            .events
            .lock()
            .iter()
            .map(|status| status.job.as_ref().unwrap().stage)
            .collect();
        for stage in [
            Stage::Fetching,
            Stage::Downloading,
            Stage::Decoding,
            Stage::Transcribing,
            Stage::Done,
        ] {
            assert!(stages.contains(&stage), "{stage:?} not shown: {stages:?}");
        }
        assert!(
            bench
                .events
                .lock()
                .iter()
                .any(|status| status.job.as_ref().unwrap().received_bytes == Some(10))
        );
        // Le titre de la vidéo aide Whisper.
        assert!(
            bench.pieces()[0]
                .prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains("CopyrightX: Lecture 3.2")),
            "{:?}",
            bench.pieces()[0].prompt
        );
        let row = bench.row(&id).unwrap().item;
        assert_eq!(row.source, SourceKind::Url);
        assert_eq!(row.url, "https://example.org/watch?v=abc");
        assert_eq!(row.title, "CopyrightX: Lecture 3.2");
        assert_eq!(row.channel, "Berkman Klein Center");
        assert!(bench.work_root_is_empty());
        assert!(!elsewhere.exists());
        assert_eq!(
            bench.fetcher.seen_dirs.lock().last().unwrap(),
            &bench.root.join("work").join(&id)
        );
        bench.script.calls.lock().clear();
    }
}

/// Relecture 107 : app fermée pendant un morceau (`shutdown` de la
/// transcription et du moteur ensemble, comme `stop_work`) : le morceau en
/// cours finit, même au-delà des délais ordinaires, puis la ligne est
/// « interrompue » (pas « annulée ») avec ce qui était fait ; le modèle est
/// libéré.
#[tokio::test(flavor = "multi_thread")]
async fn closing_the_app_records_interrupted() {
    let bench = bench();
    let (go, gate) = std::sync::mpsc::channel();
    *bench.script.gate.lock() = Some(gate);
    let (samples, _) = lecture();
    let path = bench.file("cours.wav", &samples);
    let id = bench.start(file_request(&path)).unwrap();
    for _ in 0..2_000 {
        if bench.transcription.status().job.unwrap().stage == Stage::Transcribing
            && bench.engine_busy()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(bench.engine.piece_running());
    // Le moteur qui attend apprend « interrompue », pas « annulée ».
    let waiting = {
        let (transcription, id) = (bench.transcription.clone(), id.clone());
        tokio::spawn(async move { waited(&transcription, &id, &CancellationToken::new()).await })
    };
    let engine = bench.engine.clone();
    let closing = tokio::task::spawn_blocking(move || engine.shutdown(Duration::from_millis(100)));
    let transcription = bench.transcription.clone();
    let stopping =
        tokio::spawn(async move { transcription.shutdown(Duration::from_millis(100)).await });
    // Le morceau finit après les délais ordinaires des deux arrêts.
    tokio::time::sleep(Duration::from_millis(400)).await;
    go.send(()).unwrap();
    stopping.await.unwrap();
    assert!(closing.await.unwrap(), "model still busy at exit");
    let job = bench.finished().await;
    assert_eq!(job.stage, Stage::Cancelled);
    let row = bench.row(&id).unwrap();
    assert_eq!(row.item.state, TranscriptionState::Interrupted);
    assert_eq!(
        waiting.await.unwrap(),
        Ok(TranscriptionOutcome::Interrupted)
    );
    assert_eq!(row.segments.len(), 1);
    assert_eq!(bench.pieces().len(), 1);
    assert!(bench.work_root_is_empty());
}

/// Relecture 107 : fichier dont la lecture s'arrête avant la fin annoncée
/// (WAV tronqué) : transcription finie, mais avec un avertissement, dans
/// l'état et dans l'historique.
#[tokio::test]
async fn a_file_read_only_in_part_is_done_with_a_warning() {
    let bench = bench();
    let (samples, _) = lecture();
    let path = bench.file("coupé.wav", &samples);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
    let id = bench.start(file_request(&path)).unwrap();
    let job = bench.finished().await;
    assert_eq!(job.stage, Stage::Done, "{:?}", job.error);
    let warning = job.warning.unwrap();
    assert_eq!(warning.code, TranscriptionWarningCode::Partial);
    assert!(!warning.message.starts_with("transcription."));
    let row = bench.row(&id).unwrap().item;
    assert_eq!(row.state, TranscriptionState::Done);
    assert_eq!(
        row.warning.map(|w| w.code),
        Some(TranscriptionWarningCode::Partial)
    );

    // Fichier entier : aucun avertissement.
    let whole = bench.file("entier.wav", &samples);
    let id = bench.start(file_request(&whole)).unwrap();
    assert_eq!(bench.finished().await.warning, None);
    assert_eq!(bench.row(&id).unwrap().item.warning, None);
}

/// Relecture 107 : modèle supprimé pendant la transcription (fichiers
/// partis, chargement impossible) : « modèle non installé », pas l'erreur
/// brute du chargeur ; le modèle en usage est connu (suppression refusée par
/// la commande).
#[tokio::test(flavor = "multi_thread")]
async fn a_model_deleted_during_the_transcription_is_said_so() {
    use std::sync::atomic::AtomicBool;
    static DELETED: AtomicBool = AtomicBool::new(false);
    let bench = bench();
    let (go, gate) = std::sync::mpsc::channel();
    *bench.script.gate.lock() = Some(gate);
    let (samples, _) = lecture();
    let path = bench.file("cours.wav", &samples);
    let mut prepared = bench.prepared(&["whisper-base"]);
    prepared.installed =
        Box::new(|model| model.id == "whisper-base" && !DELETED.load(Ordering::SeqCst));
    let id = bench
        .transcription
        .start(bench.store.clone(), prepared, file_request(&path))
        .unwrap();
    for _ in 0..2_000 {
        if bench.engine_busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(bench.transcription.uses_model("whisper-base"));
    assert!(!bench.transcription.uses_model("whisper-tiny"));
    // Supprimé quand même (ancienne page, autre chemin) : libéré, effacé.
    DELETED.store(true, Ordering::SeqCst);
    bench.script.unloadable.store(true, Ordering::SeqCst);
    let engine = bench.engine.clone();
    let released = tokio::task::spawn_blocking(move || engine.release_model("whisper-base"));
    go.send(()).unwrap();
    released.await.unwrap();
    let job = bench.finished().await;
    assert_eq!(job.stage, Stage::Failed);
    let error = job.error.unwrap();
    assert_eq!(error.code, TranscriptionErrorCode::ModelNotInstalled);
    assert_eq!(
        error.message,
        message(TranscriptionErrorCode::ModelNotInstalled)
    );
    assert_eq!(bench.row(&id).unwrap().segments.len(), 1);
    assert!(!bench.transcription.uses_model("whisper-base"));
}

/// Relecture 107 : sans détection de parole, un long passage de parole
/// continue (aucune pause : podcast compressé) n'est plus jugé muet.
#[tokio::test]
async fn continuous_speech_is_heard_without_speech_detection() {
    let without = bench_with(Options {
        detector: false,
        ..Options::default()
    });
    let path = without.file("podcast.wav", &speech(90.0));
    without.start(file_request(&path)).unwrap();
    let job = without.finished().await;
    assert_eq!(job.stage, Stage::Done, "{:?}", job.error);
    let sent: usize = without.pieces().iter().map(|call| call.samples).sum();
    assert!(sent > 80 * RATE, "{sent}");
    // Le silence reste muet.
    assert!(!job::chunk_has_speech(&silence(40.0)));
    assert!(!job::chunk_has_speech(&vec![0.001; 40 * RATE]));
}

/// Annulée pendant le téléchargement : rien n'est gardé (ni ligne, ni
/// fichier partiel).
#[tokio::test]
async fn cancel_during_the_download_leaves_nothing() {
    let bench = bench();
    *bench.fetcher.plan.lock() = Fetch::WaitCancel;
    let id = bench.start(url_request()).unwrap();
    for _ in 0..2_000 {
        if bench
            .root
            .join("work")
            .join(&id)
            .join("audio.part")
            .exists()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    bench.transcription.cancel();
    let job = bench.finished().await;
    assert_eq!(job.stage, Stage::Cancelled);
    assert!(bench.row(&id).is_none());
    assert!(bench.work_root_is_empty());
}

/// Échecs de yt-dlp : code gardé, texte de `ytdlp::message`, rien
/// d'enregistré.
#[tokio::test]
async fn fetch_errors_keep_their_code() {
    let bench = bench();
    for code in [FetchErrorCode::BotCheck, FetchErrorCode::Private] {
        *bench.fetcher.plan.lock() = Fetch::Fail(code);
        let id = bench.start(url_request()).unwrap();
        let job = bench.finished().await;
        assert_eq!(job.stage, Stage::Failed);
        let error = job.error.unwrap();
        assert_eq!(error.code, TranscriptionErrorCode::Fetch);
        assert_eq!(error.fetch, Some(code));
        assert_eq!(error.message, ytdlp::message(code));
        assert!(bench.row(&id).is_none());
        assert!(bench.work_root_is_empty());
    }
}

/// Fichiers illisibles : un code par cas, rien d'enregistré.
#[tokio::test]
async fn decoding_errors_become_codes() {
    let bench = bench();
    let text = bench.root.join("notes.txt");
    std::fs::write(&text, "pas du son, seulement du texte ".repeat(50)).unwrap();
    let empty = bench.root.join("vide.wav");
    std::fs::write(&empty, b"").unwrap();
    let missing = bench.root.join("absent.mp3");
    let cases = [
        (text, TranscriptionErrorCode::UnsupportedFormat),
        (empty, TranscriptionErrorCode::Damaged),
        (missing, TranscriptionErrorCode::CannotOpen),
    ];
    for (path, code) in cases {
        let path = path.to_string_lossy().into_owned();
        bench.transcription.grant([path.clone()]);
        let id = bench.start(file_request(&path)).unwrap();
        let job = bench.finished().await;
        assert_eq!(job.stage, Stage::Failed);
        let error = job.error.unwrap();
        assert_eq!(error.code, code, "{path}");
        assert_eq!(error.message, message(code));
        assert!(bench.row(&id).is_none());
    }
    assert!(bench.pieces().is_empty());
    assert!(bench.work_root_is_empty());
}

/// Rien que du silence : « aucune parole », aucun appel au moteur.
#[tokio::test]
async fn silence_alone_is_no_speech() {
    let bench = bench();
    let path = bench.file("silence.wav", &silence(100.0));
    let id = bench.start(file_request(&path)).unwrap();
    let job = bench.finished().await;
    assert_eq!(job.stage, Stage::Failed);
    assert_eq!(job.error.unwrap().code, TranscriptionErrorCode::NoSpeech);
    assert!(bench.pieces().is_empty());
    assert!(bench.row(&id).is_none());
}

/// Échec du moteur au milieu : ce qui est fait reste, avec le code.
#[tokio::test]
async fn an_engine_failure_keeps_what_was_done() {
    let bench = bench();
    *bench.script.fail_at.lock() = Some(1);
    let (samples, _) = lecture();
    let path = bench.file("cours.wav", &samples);
    let id = bench.start(file_request(&path)).unwrap();
    let job = bench.finished().await;
    assert_eq!(job.stage, Stage::Failed);
    let error = job.error.unwrap();
    assert_eq!(error.code, TranscriptionErrorCode::Failed);
    assert_eq!(error.message, "Transcription failed: boom");
    let row = bench.row(&id).unwrap();
    assert_eq!(row.item.state, TranscriptionState::Failed);
    assert_eq!(row.segments.len(), 1);
    assert!(bench.work_root_is_empty());
}

/// Sans détection de parole (ONNX Runtime absent) : le niveau sonore décide
/// des morceaux muets. Le détecteur est gardé et remis à zéro pour chaque
/// fichier.
#[tokio::test]
async fn levels_decide_without_speech_detection_and_the_detector_is_reused() {
    let without = bench_with(Options {
        detector: false,
        ..Options::default()
    });
    let (samples, _) = lecture();
    let path = without.file("cours.wav", &samples);
    without.start(file_request(&path)).unwrap();
    assert_eq!(without.finished().await.stage, Stage::Done);
    let sent: usize = without.pieces().iter().map(|call| call.samples).sum();
    assert!(sent < samples.len() - 30 * RATE, "silence not skipped");

    let with = bench();
    let path = with.file("cours.wav", &samples);
    for _ in 0..2 {
        with.start(file_request(&path)).unwrap();
        assert_eq!(with.finished().await.stage, Stage::Done);
    }
    assert_eq!(with.detectors.load(Ordering::SeqCst), 1);
    assert_eq!(with.resets.load(Ordering::SeqCst), 2);
}

/// Une dictée qui arrive pendant un morceau passe avant le suivant.
#[tokio::test]
async fn a_dictation_passes_between_two_chunks() {
    let bench = bench();
    let (go, gate) = std::sync::mpsc::channel();
    *bench.script.gate.lock() = Some(gate);
    let (samples, _) = lecture();
    let path = bench.file("cours.wav", &samples);
    bench.start(file_request(&path)).unwrap();
    for _ in 0..2_000 {
        if bench.engine_busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let engine = bench.engine.clone();
    let dictation = tokio::task::spawn_blocking(move || {
        let spec = ModelSpec {
            id: "whisper-base".into(),
            engine: SpeechEngine::Whisper,
            dir: PathBuf::from("/nowhere"),
            file: PathBuf::from("/nowhere/model.bin"),
        };
        engine.transcribe(
            &spec,
            &[0.0],
            &Request::default(),
            &std::sync::atomic::AtomicBool::new(false),
        )
    });
    for _ in 0..2_000 {
        if bench.engine.urgent_waiting() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    go.send(()).unwrap();
    assert_eq!(dictation.await.unwrap().unwrap(), "dictation");
    assert_eq!(bench.finished().await.stage, Stage::Done);
    let kinds: Vec<&str> = bench.script.calls.lock().iter().map(|c| c.kind).collect();
    assert_eq!(&kinds[..3], ["piece", "dictation", "piece"], "{kinds:?}");
}

/// Refus immédiats et échecs de préparation.
#[tokio::test]
async fn refusals_and_missing_models() {
    let bench = bench();
    // Fichier non remis par le moteur.
    let path = bench.root.join("secret.wav");
    write_wav(&path, &speech(2.0));
    let refused = bench.start(file_request(&path.to_string_lossy()));
    assert!(matches!(refused, Err(AppError::Invalid(_))), "{refused:?}");
    // Adresse vide, langue inconnue.
    let mut empty = url_request();
    empty.source = Source::Url { url: "  ".into() };
    assert!(matches!(bench.start(empty), Err(AppError::Invalid(_))));
    let mut unknown = url_request();
    unknown.language = Some("xx".into());
    assert!(matches!(bench.start(unknown), Err(AppError::Invalid(_))));
    assert!(bench.transcription.status().job.is_none());

    // Aucun modèle installé : erreur claire (la page propose le ModelHub).
    let path = bench.file("court.wav", &speech(2.0));
    bench.start_with(file_request(&path), &[]).unwrap();
    let job = bench.finished().await;
    assert_eq!(job.error.unwrap().code, TranscriptionErrorCode::NoModel);
    // Modèle demandé absent.
    let mut request = file_request(&path);
    request.model = Some("whisper-small".into());
    bench.start(request).unwrap();
    let job = bench.finished().await;
    assert_eq!(
        job.error.unwrap().code,
        TranscriptionErrorCode::ModelNotInstalled
    );
    // Modèle de la dictée absent : le premier installé.
    bench
        .start_with(file_request(&path), &["whisper-tiny"])
        .unwrap();
    let job = bench.finished().await;
    assert_eq!(job.stage, Stage::Done);
    assert_eq!(job.model, "whisper-tiny");

    // Une seule à la fois.
    let (go, gate) = std::sync::mpsc::channel();
    *bench.script.gate.lock() = Some(gate);
    let first = bench.start(file_request(&path)).unwrap();
    let busy = bench.start(file_request(&path));
    assert!(matches!(busy, Err(AppError::Invalid(_))), "{busy:?}");
    assert!(bench.transcription.is_running(&first));
    go.send(()).unwrap();
    assert_eq!(bench.finished().await.id, first);

    // Moteur impossible sur cette machine : refus de la machine, texte gardé.
    let refusing = bench_with(Options {
        support: |_| Err("This processor cannot run Whisper models".into()),
        ..Options::default()
    });
    let path = refusing.file("court.wav", &speech(2.0));
    refusing.start(file_request(&path)).unwrap();
    let error = refusing.finished().await.error.unwrap();
    assert_eq!(error.code, TranscriptionErrorCode::Unsupported);
    assert_eq!(error.message, "This processor cannot run Whisper models");
    assert!(refusing.work_root_is_empty());
}

/// Page rechargée : les chemins remis sont oubliés.
#[test]
fn reloading_the_page_forgets_the_granted_paths() {
    let bench = bench();
    bench.transcription.grant(["/tmp/a.mp3".to_owned()]);
    assert!(bench.transcription.is_granted("/tmp/a.mp3"));
    bench.transcription.clear_grants();
    assert!(!bench.transcription.is_granted("/tmp/a.mp3"));
}

/// Lancement après une app coupée : audio de travail effacé, transcription
/// « en cours » interrompue.
#[test]
fn recovery_removes_leftover_work_audio() {
    let bench = bench();
    let leftover = bench.root.join("work").join("old");
    std::fs::create_dir_all(&leftover).unwrap();
    std::fs::write(leftover.join("audio.f32"), [0u8; 16]).unwrap();
    bench
        .store
        .blocking(|connection| {
            rows::insert(
                connection,
                &rows::NewTranscription {
                    id: "old".into(),
                    source: SourceKind::File,
                    origin: "/a.mp3".into(),
                    title: "a.mp3".into(),
                    engine: SpeechEngine::Parakeet,
                    model: "parakeet-tdt-0.6b-v3".into(),
                    language: String::new(),
                },
            )?;
            rows::set_segments(
                connection,
                "old",
                &[Segment {
                    start: 0.0,
                    end: 1.0,
                    text: "Bonjour.".into(),
                }],
            )
        })
        .unwrap();
    bench.transcription.recover(Some(&bench.store));
    assert!(!bench.root.join("work").exists());
    assert_eq!(
        bench.row("old").unwrap().item.state,
        TranscriptionState::Interrupted
    );
}

/// Options : tout le catalogue, modèle de la dictée d'office s'il est là,
/// sinon le premier installé, sinon aucun.
#[test]
fn options_follow_the_dictation_model() {
    let bench = bench();
    let options = bench.prepared(&["whisper-base", "whisper-tiny"]).options();
    assert_eq!(
        options.models.len(),
        crate::dictation::models::CATALOG.len()
    );
    assert_eq!(options.default_model.as_deref(), Some("whisper-base"));
    assert_eq!(options.language, "auto");
    assert!(
        options
            .models
            .iter()
            .any(|model| model.id == "whisper-tiny" && model.installed)
    );
    let options = bench.prepared(&["whisper-tiny"]).options();
    assert_eq!(options.default_model.as_deref(), Some("whisper-tiny"));
    assert_eq!(bench.prepared(&[]).options().default_model, None);
}

/// Progression seule : espacée (`EMIT_GAP`) ; une étape ou des passages
/// partent aussitôt.
#[test]
fn progress_alone_is_throttled() {
    let bench = bench_with(Options {
        gap: Duration::from_secs(3600),
        ..Options::default()
    });
    let shared = bench.transcription.shared().clone();
    shared.inner.lock().job = Some(TranscriptionJob::new("x".into(), &url_request().source));
    for step in 0..50 {
        shared.update(false, |job| job.done_s = f64::from(step));
    }
    // Le premier seulement (aucun envoi avant lui).
    assert_eq!(bench.events.lock().len(), 1);
    shared.update(true, |job| job.stage = Stage::Decoding);
    shared.update(true, |job| job.stage = Stage::Transcribing);
    let events = bench.events.lock();
    assert_eq!(events.len(), 3);
    assert_eq!(events[2].job.as_ref().unwrap().done_s, 49.0);
    assert_eq!(EMIT_GAP, Duration::from_millis(250));
}

// --- Attente depuis le moteur (Atelier) ------------------------------------

/// [`Transcription::wait`] bornée : une fin jamais annoncée fait échouer
/// l'essai au lieu de le bloquer.
async fn waited(
    transcription: &Transcription,
    id: &str,
    cancel: &CancellationToken,
) -> Result<TranscriptionOutcome, WaitError> {
    tokio::time::timeout(Duration::from_secs(120), transcription.wait(id, cancel))
        .await
        .expect("the end of the transcription never came")
}

/// Attente d'une transcription par le moteur : rien ne change pour la page
/// (mêmes événements que sans attente), la fin arrive avec ses passages,
/// aussi à un abonné ; une attente commencée après la fin la reçoit aussi.
#[tokio::test(flavor = "multi_thread")]
async fn the_engine_waits_for_the_end_of_a_transcription() {
    let bench = bench();
    let (samples, _) = lecture();
    let path = bench.file("cours.wav", &samples);
    let mut ends = bench.transcription.subscribe();
    let id = bench.start(file_request(&path)).unwrap();
    let never = CancellationToken::new();
    let outcome = waited(&bench.transcription, &id, &never).await.unwrap();
    let TranscriptionOutcome::Done { segments, warning } = outcome.clone() else {
        panic!("{outcome:?}");
    };
    assert!(!segments.is_empty());
    assert_eq!(warning, None);
    assert_eq!(segments, bench.row(&id).unwrap().segments);
    assert_eq!(segments, bench.segments_from_events());
    let end = ends.recv().await.unwrap();
    assert_eq!(
        end,
        TranscriptionEnd {
            id: id.clone(),
            outcome: outcome.clone()
        }
    );
    // Fin déjà passée : rendue aussitôt.
    assert_eq!(waited(&bench.transcription, &id, &never).await, Ok(outcome));
}

/// Fins d'échec et d'annulation, chacune avec son issue.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_transcription_is_told_to_the_engine() {
    let bench = bench();
    *bench.script.fail_at.lock() = Some(1);
    let (samples, _) = lecture();
    let path = bench.file("cours.wav", &samples);
    let id = bench.start(file_request(&path)).unwrap();
    let outcome = waited(&bench.transcription, &id, &CancellationToken::new())
        .await
        .unwrap();
    let TranscriptionOutcome::Failed { error, segments } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(error.code, TranscriptionErrorCode::Failed);
    assert_eq!(segments.len(), 1, "what was done is kept");
}

/// L'appelant abandonne : la transcription attendue est annulée (pas une
/// autre), l'attente rend la main aussitôt, la fin dit « annulée ».
#[tokio::test(flavor = "multi_thread")]
async fn giving_up_the_wait_cancels_that_transcription() {
    let bench = bench();
    let (go, gate) = std::sync::mpsc::channel();
    *bench.script.gate.lock() = Some(gate);
    let (samples, _) = lecture();
    let path = bench.file("cours.wav", &samples);
    let id = bench.start(file_request(&path)).unwrap();
    // Une autre transcription ne se fait pas annuler par cet identifiant.
    bench.transcription.cancel_id("another");
    let cancel = CancellationToken::new();
    let waiting = {
        let (transcription, id, cancel) = (bench.transcription.clone(), id.clone(), cancel.clone());
        tokio::spawn(async move { waited(&transcription, &id, &cancel).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!waiting.is_finished());
    cancel.cancel();
    assert_eq!(waiting.await.unwrap(), Err(WaitError::Cancelled));
    go.send(()).unwrap();
    assert_eq!(bench.finished().await.stage, Stage::Cancelled);
    assert_eq!(
        waited(&bench.transcription, &id, &CancellationToken::new()).await,
        Ok(TranscriptionOutcome::Cancelled)
    );
}

/// Deux transcriptions finies avant l'attente de la première : son issue
/// est rendue (pas seulement celle de la dernière).
#[tokio::test(flavor = "multi_thread")]
async fn an_earlier_end_is_still_known_after_another_transcription() {
    let bench = bench();
    *bench.script.fail_at.lock() = Some(1);
    let (samples, _) = lecture();
    let path = bench.file("cours.wav", &samples);
    let first = bench.start(file_request(&path)).unwrap();
    assert_eq!(bench.finished().await.stage, Stage::Failed);
    *bench.script.fail_at.lock() = None;
    let second = bench.start(file_request(&path)).unwrap();
    assert_ne!(first, second);
    assert_eq!(bench.finished().await.id, second);
    let never = CancellationToken::new();
    let outcome = waited(&bench.transcription, &first, &never).await;
    assert!(
        matches!(outcome, Ok(TranscriptionOutcome::Failed { .. })),
        "{outcome:?}"
    );
    let outcome = waited(&bench.transcription, &second, &never).await;
    assert!(
        matches!(outcome, Ok(TranscriptionOutcome::Done { .. })),
        "{outcome:?}"
    );
}

/// Les `ENDS_KEPT` dernières fins sont gardées, pas plus.
#[tokio::test]
async fn only_the_last_ends_are_kept() {
    let bench = bench();
    let shared = bench.transcription.shared().clone();
    let source = url_request().source;
    for n in 0..=ENDS_KEPT {
        let mut job = TranscriptionJob::new(format!("t{n}"), &source);
        job.stage = Stage::Done;
        shared.inner.lock().job = Some(job);
        shared.finish(|_| {});
    }
    let never = CancellationToken::new();
    assert_eq!(
        waited(&bench.transcription, "t0", &never).await,
        Err(WaitError::Unknown)
    );
    for n in 1..=ENDS_KEPT {
        assert!(
            waited(&bench.transcription, &format!("t{n}"), &never)
                .await
                .is_ok(),
            "t{n}"
        );
    }
}

/// Identifiant ni en cours ni dernier : refus ; « Erase my data » oublie la
/// dernière fin (ses passages).
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_or_forgotten_transcription_cannot_be_waited_for() {
    let bench = bench();
    let never = CancellationToken::new();
    assert_eq!(
        waited(&bench.transcription, "nope", &never).await,
        Err(WaitError::Unknown)
    );
    let (samples, _) = lecture();
    let path = bench.file("cours.wav", &samples);
    let id = bench.start(file_request(&path)).unwrap();
    assert!(waited(&bench.transcription, &id, &never).await.is_ok());
    bench.transcription.forget(Duration::from_secs(5)).await;
    assert_eq!(
        waited(&bench.transcription, &id, &never).await,
        Err(WaitError::Unknown)
    );
}

/// Préparation hors commande : réglages de la dictée lus dans la base,
/// modèles cherchés dans le dossier donné ; demandes sans modèle ni langue
/// (ceux de la dictée).
#[tokio::test]
async fn requests_are_prepared_from_the_dictation_settings() {
    let bench = bench();
    bench
        .store
        .blocking(|connection| {
            crate::store::dictation::update(
                connection,
                crate::store::dictation::DictationPatch {
                    speech_model: Some("whisper-tiny".into()),
                    language: Some("fr".into()),
                    dictionary: Some(vec!["Atelier".into()]),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let models_dir = bench.root.join("speech-models");
    let prepared = prepare_in(&bench.store, models_dir.clone()).await.unwrap();
    assert_eq!(prepared.speech_model, "whisper-tiny");
    assert_eq!(prepared.language, "fr");
    assert_eq!(prepared.dictionary, ["Atelier"]);
    assert_eq!(prepared.models_dir, models_dir);
    // Aucun fichier de modèle dans le dossier : rien d'installé.
    assert!(prepared.default_model().is_none());
    assert_eq!(
        super::file_request("/a/cours.mp3"),
        TranscriptionRequest {
            source: Source::File {
                path: "/a/cours.mp3".into()
            },
            model: None,
            language: None,
        }
    );
    assert_eq!(
        super::url_request("https://example.org/v"),
        TranscriptionRequest {
            source: Source::Url {
                url: "https://example.org/v".into()
            },
            model: None,
            language: None,
        }
    );
}
