//! Déroulé d'une transcription (voir le module) : préparation, audio d'une
//! adresse, décodage vers le fichier de travail avec relevé de la parole,
//! découpage, transcription morceau par morceau, enregistrement au fil de
//! l'eau ; audio de travail effacé à la fin, quoi qu'il arrive.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use media_audio::{Codec, Container, DecodeError, ErrorCode, MediaInfo, Progress, Sink};

use super::paragraphs::Segment;
use super::segment::{self, Analyzer, Chunk};
use super::ytdlp::{FetchErrorCode, FetchProgress};
use super::{
    Cancel, Prepared, Shared, Source, Stage, TranscriptionError, TranscriptionErrorCode as Code,
    TranscriptionRequest, TranscriptionState,
};
use crate::dictation::engine::{self, ModelSpec, Request};
use crate::dictation::languages;
use crate::dictation::models::{self, CatalogModel, SpeechEngine};
use crate::i18n::t;
use crate::store::Store;
use crate::store::transcriptions as rows;

/// Durée maximale d'une transcription (heures), comme `media_audio::Limits`.
pub const MAX_HOURS: u64 = 6;
/// Taille maximale d'un fichier local (Gio), comme `media_audio::Limits`.
pub const MAX_GIGABYTES: u64 = 20;

/// Nom de l'audio de travail : f32 petit-boutien, 16 kHz mono.
const WORK_FILE: &str = "audio.f32";

/// Essais d'effacement du dossier de travail (Windows : antivirus, fichier
/// encore ouvert un instant).
const REMOVE_TRIES: usize = 10;

/// Ce que la page a demandé, et les réglages lus pour elle.
pub struct Plan {
    pub id: String,
    pub request: TranscriptionRequest,
    pub prepared: Prepared,
}

/// Fin anticipée.
#[derive(Debug)]
enum Stop {
    Cancelled,
    /// App qui ferme pendant la transcription.
    Interrupted,
    Failed(TranscriptionError),
}

impl From<TranscriptionError> for Stop {
    fn from(error: TranscriptionError) -> Self {
        Stop::Failed(error)
    }
}

/// Une transcription en cours.
struct Run {
    shared: Arc<Shared>,
    store: Store,
    id: String,
    cancel: Arc<Cancel>,
    work_dir: PathBuf,
    /// Passages faits, déjà décalés au temps du son entier.
    segments: Vec<Segment>,
    /// La ligne de l'historique existe.
    recorded: bool,
    /// Audio récupéré hors du dossier de travail (à effacer aussi).
    stray: Option<PathBuf>,
}

/// Modèle de la transcription : celui demandé s'il est installé ; sinon
/// celui de la dictée, puis le premier installé.
pub fn choose_model(
    requested: Option<&str>,
    prepared: &Prepared,
) -> Result<&'static CatalogModel, Code> {
    match requested.filter(|id| !id.is_empty()) {
        Some(id) => models::find(id)
            .filter(|model| (prepared.installed)(model))
            .ok_or(Code::ModelNotInstalled),
        None => prepared.default_model().ok_or(Code::NoModel),
    }
}

/// Langue passée au moteur : `None` pour la détection (« auto »).
pub fn language_of(requested: Option<&str>, prepared: &Prepared) -> Option<String> {
    let code = requested.unwrap_or(&prepared.language);
    (code != languages::AUTO && languages::is_supported(code)).then(|| code.to_owned())
}

/// Code de langue d'un texte (première langue figée de Whisper), parmi
/// celles que `proofread::language` reconnaît.
pub fn detect_language(text: &str) -> Option<String> {
    use crate::proofread::language::{Language, detect};
    let code = match detect(text)? {
        Language::English => "en",
        Language::French => "fr",
        Language::Spanish => "es",
        Language::German => "de",
        Language::Italian => "it",
        Language::Portuguese => "pt",
    };
    Some(code.to_owned())
}

/// Nom lisible d'un codec refusé.
fn codec_name(codec: Codec) -> Option<&'static str> {
    Some(match codec {
        Codec::Ac3 => "AC-3",
        Codec::Eac3 => "E-AC-3",
        Codec::Dts => "DTS",
        Codec::TrueHd => "TrueHD",
        Codec::Wma => "WMA",
        _ => return None,
    })
}

/// Nom lisible d'un conteneur refusé.
fn container_name(container: Container) -> Option<&'static str> {
    Some(match container {
        Container::Avi => "AVI",
        Container::MpegTs => "MPEG-TS",
        Container::MpegPs => "MPEG-PS",
        Container::Asf => "WMA/ASF",
        Container::Flv => "FLV",
        Container::RealMedia => "RealMedia",
        _ => return None,
    })
}

/// Échec du décodage pour la page ; `None` pour une annulation.
pub fn decode_error(error: &DecodeError) -> Option<TranscriptionError> {
    let with = TranscriptionError::with_message;
    Some(match error.code {
        ErrorCode::Cancelled => return None,
        ErrorCode::CannotOpen => TranscriptionError::of(Code::CannotOpen),
        ErrorCode::UnsupportedContainer => match error.container.and_then(container_name) {
            Some(format) => with(
                Code::UnsupportedFormat,
                t!(
                    "transcription.unsupportedFormat",
                    format = format,
                    formats = super::SUPPORTED_FORMATS
                ),
            ),
            None => TranscriptionError::of(Code::UnsupportedFormat),
        },
        ErrorCode::UnsupportedCodec => match error.codec.and_then(codec_name) {
            Some(codec) => with(
                Code::UnsupportedCodec,
                t!("transcription.unsupportedCodec", codec = codec),
            ),
            None => TranscriptionError::of(Code::UnsupportedCodec),
        },
        ErrorCode::NoAudioTrack => TranscriptionError::of(Code::NoAudioTrack),
        ErrorCode::Damaged => TranscriptionError::of(Code::Damaged),
        ErrorCode::TooLong => TranscriptionError::of(Code::TooLong),
        ErrorCode::TooLarge => TranscriptionError::of(Code::TooLarge),
        ErrorCode::Output => with(
            Code::Io,
            t!("transcription.ioFailedDetail", error = error.detail),
        ),
    })
}

/// Déroule la transcription jusqu'au bout et enregistre sa fin.
pub(super) async fn run(shared: Arc<Shared>, store: Store, plan: Plan, cancel: Arc<Cancel>) {
    let work_dir = shared.services().work_root.join(&plan.id);
    let mut run = Run {
        shared,
        store,
        id: plan.id.clone(),
        cancel,
        work_dir,
        segments: Vec::new(),
        recorded: false,
        stray: None,
    };
    let outcome = run.execute(plan).await;
    run.clean_up().await;
    run.conclude(outcome).await;
}

impl Run {
    async fn execute(&mut self, plan: Plan) -> Result<(), Stop> {
        let Plan {
            request, prepared, ..
        } = plan;
        let model =
            choose_model(request.model.as_deref(), &prepared).map_err(TranscriptionError::of)?;
        let mut language = language_of(request.language.as_deref(), &prepared);
        if model.engine == SpeechEngine::Parakeet {
            // Parakeet détecte seul (l'option est ignorée) : la langue
            // demandée reste notée si elle est la sienne.
            language = language.filter(|code| {
                languages::LANGUAGES
                    .iter()
                    .any(|known| known.code == code.as_str() && known.parakeet)
            });
        }
        let detect = model.engine == SpeechEngine::Whisper && language.is_none();
        let spec = ModelSpec::new(model, &prepared.models_dir);
        self.shared.update(true, |job| {
            job.model = model.id.into();
            job.engine = Some(model.engine);
            job.language = language.clone();
        });

        // Moteur que cette machine ne peut pas faire tourner : refus avant
        // tout téléchargement.
        let support = self.shared.services().support;
        let kind = model.engine;
        match tauri::async_runtime::spawn_blocking(move || support(kind)).await {
            Ok(Ok(())) => {}
            Ok(Err(refusal)) => {
                return Err(TranscriptionError::with_message(Code::Unsupported, refusal).into());
            }
            Err(error) => return Err(failed(&error).into()),
        }

        let new = rows::NewTranscription {
            id: self.id.clone(),
            source: request.source.kind(),
            origin: request.source.origin().to_owned(),
            title: match &request.source {
                Source::File { path } => super::file_name(path).unwrap_or_default(),
                Source::Url { .. } => String::new(),
            },
            engine: model.engine,
            model: model.id.to_owned(),
            language: language.clone().unwrap_or_default(),
        };
        self.store
            .call(move |connection| rows::insert(connection, &new))
            .await
            .map_err(|error| failed(&error))?;
        self.recorded = true;
        self.make_work_dir().await?;
        self.check_cancel()?;

        // Audio : le fichier lui-même, ou celui que yt-dlp récupère.
        let mut title = None;
        let audio = match &request.source {
            Source::File { path } => PathBuf::from(path),
            Source::Url { url } => {
                let fetched = self.fetch(url.trim()).await?;
                title = fetched.info.title.clone().filter(|title| !title.is_empty());
                fetched.path
            }
        };

        // Modèle chargé pendant le décodage, gardé jusqu'à la fin.
        let engine = self.shared.services().engine.clone();
        let _hold = engine.hold();
        engine.preload(spec.clone());

        let analysis = self.decode(&audio).await?;
        let duration = analysis.samples as f64 / segment::RATE as f64;
        let id = self.id.clone();
        let duration_ms = (duration * 1000.0).round() as i64;
        self.store
            .call(move |connection| rows::set_duration(connection, &id, duration_ms))
            .await
            .map_err(|error| failed(&error))?;
        self.shared.update(true, |job| {
            job.stage = Stage::Transcribing;
            job.duration_s = Some(duration);
            job.done_s = 0.0;
        });

        let chunks = segment::plan(&analysis);
        drop(analysis);
        let mut request = Request {
            language,
            glossary: prepared.dictionary.clone(),
            // Titre de la vidéo : noms propres et termes pour Whisper.
            project: title.into_iter().collect(),
        };
        let mut transcribed = 0.0;
        let mut busy = Duration::ZERO;
        for chunk in chunks {
            self.check_cancel()?;
            let started = Instant::now();
            let piece = self.transcribe(&engine, &spec, &request, &chunk).await?;
            // Passages comptés depuis le début de la partie envoyée.
            let start = chunk.voiced.start as f64 / segment::RATE as f64;
            let sent = (chunk.voiced.end - chunk.voiced.start) as f64 / segment::RATE as f64;
            let end = chunk.range.end as f64 / segment::RATE as f64;
            let mut added = Vec::new();
            if let Some(piece) = piece {
                busy += started.elapsed();
                transcribed += sent;
                added = piece
                    .segments
                    .into_iter()
                    .map(|timed| Segment {
                        start: start + f64::from(timed.start),
                        end: start + f64::from(timed.end),
                        text: timed.text,
                    })
                    .collect();
                // Whisper sans langue : détectée sur le premier morceau
                // parlé, puis gardée (sinon il la redétecte à chaque
                // morceau et peut basculer sur une citation).
                if detect
                    && request.language.is_none()
                    && let Some(code) = detect_language(&piece.text)
                {
                    request.language = Some(code.clone());
                    let id = self.id.clone();
                    let saved = code.clone();
                    self.store
                        .call(move |connection| rows::set_language(connection, &id, &saved))
                        .await
                        .map_err(|error| failed(&error))?;
                    self.shared.update(false, |job| job.language = Some(code));
                }
            }
            let fresh = !added.is_empty();
            if fresh {
                self.segments.extend(added.iter().cloned());
                self.save_segments().await?;
            }
            let speed = (busy > Duration::ZERO).then(|| transcribed / busy.as_secs_f64());
            self.shared.update(fresh, |job| {
                job.segments.extend(added);
                job.done_s = end;
                job.speed = speed;
            });
        }
        if self.segments.is_empty() {
            return Err(TranscriptionError::of(Code::NoSpeech).into());
        }
        Ok(())
    }

    fn check_cancel(&self) -> Result<(), Stop> {
        if self.cancel.is_raised() {
            return Err(Stop::Cancelled);
        }
        Ok(())
    }

    async fn make_work_dir(&self) -> Result<(), Stop> {
        let dir = self.work_dir.clone();
        tauri::async_runtime::spawn_blocking(move || std::fs::create_dir_all(dir))
            .await
            .map_err(|error| failed(&error))?
            .map_err(|error| {
                Stop::Failed(TranscriptionError::with_message(
                    Code::Io,
                    t!("transcription.ioFailedDetail", error = error),
                ))
            })
    }

    /// Audio d'une adresse dans le dossier de travail.
    async fn fetch(&mut self, url: &str) -> Result<super::ytdlp::FetchedAudio, Stop> {
        self.shared.update(true, |job| job.stage = Stage::Fetching);
        let shared = self.shared.clone();
        let progress = move |progress: FetchProgress| match progress {
            FetchProgress::Info(info) => shared.update(true, |job| {
                job.stage = Stage::Downloading;
                job.title = info.title.clone().filter(|title| !title.is_empty());
                job.channel = info.channel.clone().filter(|channel| !channel.is_empty());
                job.url = Some(info.webpage_url.clone());
                job.site = Some(info.site.clone());
                job.duration_s = info.duration_s;
            }),
            FetchProgress::Downloading { received, total } => shared.update(false, |job| {
                job.stage = Stage::Downloading;
                job.received_bytes = Some(received);
                job.total_bytes = total;
            }),
        };
        let max = Some((MAX_HOURS * 3600) as f64);
        let receiver = self.cancel.sender.subscribe();
        let fetched = self
            .shared
            .services()
            .fetcher
            .fetch(url, &self.work_dir, max, receiver, &progress)
            .await;
        let fetched = match fetched {
            Ok(fetched) => fetched,
            Err(error) if error.code == FetchErrorCode::Cancelled || self.cancel.is_raised() => {
                return Err(Stop::Cancelled);
            }
            Err(error) => {
                crate::app_log!("chatonlm: transcription: yt-dlp: {}", error.detail);
                return Err(TranscriptionError::fetch(&error).into());
            }
        };
        if !fetched.path.starts_with(&self.work_dir) {
            self.stray = Some(fetched.path.clone());
        }
        let info = fetched.info.clone();
        let id = self.id.clone();
        self.store
            .call(move |connection| rows::set_info(connection, &id, &info))
            .await
            .map_err(|error| failed(&error))?;
        Ok(fetched)
    }

    /// Décode `audio` dans le fichier de travail en relevant la parole.
    async fn decode(&self, audio: &Path) -> Result<segment::Analysis, Stop> {
        self.shared.update(true, |job| {
            job.stage = Stage::Decoding;
            job.done_s = 0.0;
        });
        let shared = self.shared.clone();
        let audio = audio.to_owned();
        let target = self.work_dir.join(WORK_FILE);
        let flag = self.cancel.flag.clone();
        let limits = self.shared.services().limits;
        let decoded = tauri::async_runtime::spawn_blocking(move || {
            let detector = shared.take_detector();
            let file = File::create(&target).map_err(|error| DecodeError {
                code: ErrorCode::Output,
                container: None,
                codec: None,
                detail: error.to_string(),
            });
            let file = match file {
                Ok(file) => file,
                Err(error) => return (Err(error), detector),
            };
            let progress_to = shared.clone();
            let mut sink = WorkSink {
                out: BufWriter::new(file),
                analyzer: Analyzer::new(detector),
                progress: Box::new(move |progress: Progress| {
                    progress_to.update(false, |job| {
                        job.done_s = progress.decoded.as_secs_f64();
                        if let Some(total) = progress.total {
                            job.duration_s = Some(total.as_secs_f64());
                        }
                    });
                }),
            };
            let options = media_audio::Options {
                limits,
                // Un cœur laissé à la dictée et à l'interface.
                threads: std::thread::available_parallelism()
                    .map_or(1, |cores| usize::from(cores).saturating_sub(1).max(1)),
            };
            let summary = media_audio::decode_file(&audio, &options, &flag, &mut sink);
            let flushed = sink.out.flush();
            let (analysis, detector) = sink.analyzer.finish();
            let result = match (summary, flushed) {
                (Ok(summary), Ok(())) => {
                    if !summary.complete {
                        crate::app_log!(
                            "chatonlm: transcription: decoding stopped early ({} packets skipped): \
                             the start is transcribed",
                            summary.skipped_packets
                        );
                    }
                    Ok(analysis)
                }
                (Err(error), _) => Err(error),
                (Ok(_), Err(error)) => Err(DecodeError {
                    code: ErrorCode::Output,
                    container: None,
                    codec: None,
                    detail: error.to_string(),
                }),
            };
            (result, detector)
        })
        .await;
        let (result, detector) = decoded.map_err(|error| failed(&error))?;
        self.shared.keep_detector(detector);
        match result {
            Ok(analysis) => Ok(analysis),
            Err(error) => {
                crate::app_log!(
                    "chatonlm: transcription: decoding: {:?}: {}",
                    error.code,
                    error.detail
                );
                match decode_error(&error) {
                    Some(error) => Err(error.into()),
                    None => Err(Stop::Cancelled),
                }
            }
        }
    }

    /// Transcrit un morceau, ou rien s'il est muet.
    async fn transcribe(
        &self,
        engine: &engine::Engine,
        spec: &ModelSpec,
        request: &Request,
        chunk: &Chunk,
    ) -> Result<Option<engine::Piece>, Stop> {
        let engine = engine.clone();
        let spec = spec.clone();
        let request = request.clone();
        let chunk = chunk.clone();
        let path = self.work_dir.join(WORK_FILE);
        let flag = self.cancel.flag.clone();
        let done = tauri::async_runtime::spawn_blocking(move || {
            let samples = read_samples(&path, &chunk.range)
                .map_err(|error| t!("transcription.ioFailedDetail", error = error))?;
            // Sans détection, la parole se juge sur le morceau entier (critère
            // relatif de la dictée : il faut ses silences) ; seul l'envoi est
            // rogné.
            let speech = chunk
                .speech
                .unwrap_or_else(|| crate::dictation::audio::has_speech(&samples));
            if !speech {
                return Ok(None);
            }
            let voiced =
                chunk.voiced.start - chunk.range.start..chunk.voiced.end - chunk.range.start;
            engine
                .transcribe_piece(&spec, &samples[voiced], &request, &flag)
                .map(Some)
        })
        .await
        .map_err(|error| failed(&error))?;
        match done {
            Ok(piece) => Ok(piece),
            Err(error) if error == engine::CANCELLED => Err(Stop::Cancelled),
            Err(error) if error == engine::CLOSING => Err(Stop::Interrupted),
            Err(error) => Err(TranscriptionError::with_message(Code::Failed, error).into()),
        }
    }

    async fn save_segments(&self) -> Result<(), Stop> {
        let id = self.id.clone();
        let segments = self.segments.clone();
        self.store
            .call(move |connection| rows::set_segments(connection, &id, &segments))
            .await
            .map_err(|error| Stop::Failed(failed(&error)))
    }

    /// Dossier de travail effacé (et l'audio récupéré ailleurs, s'il l'a
    /// été) ; réessayé un moment (fichier encore ouvert sous Windows).
    async fn clean_up(&self) {
        let dir = self.work_dir.clone();
        let stray = self.stray.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            if let Some(stray) = stray {
                let _ = std::fs::remove_file(stray);
            }
            for attempt in 0..REMOVE_TRIES {
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => return,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                    Err(error) if attempt + 1 == REMOVE_TRIES => {
                        // Effacé au lancement suivant (`Transcription::recover`).
                        crate::app_log!(
                            "chatonlm: transcription: work folder not removed: {error}"
                        );
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
        })
        .await;
    }

    /// Fin enregistrée et annoncée. Sans aucun passage, la ligne de
    /// l'historique est retirée (rien à relire) ; l'échec reste dans l'état.
    async fn conclude(&self, outcome: Result<(), Stop>) {
        let (state, stage, error) = match outcome {
            Ok(()) => (TranscriptionState::Done, Stage::Done, None),
            Err(Stop::Cancelled) => (TranscriptionState::Cancelled, Stage::Cancelled, None),
            Err(Stop::Interrupted) => (TranscriptionState::Interrupted, Stage::Cancelled, None),
            Err(Stop::Failed(error)) => (TranscriptionState::Failed, Stage::Failed, Some(error)),
        };
        if self.recorded {
            let id = self.id.clone();
            let keep = !self.segments.is_empty();
            let code = error.as_ref().map(|error| error.code);
            let saved = self
                .store
                .call(move |connection| {
                    if keep {
                        rows::finish(connection, &id, state, code)
                    } else {
                        rows::delete(connection, &id).map(|_| ())
                    }
                })
                .await;
            if let Err(error) = saved {
                crate::app_log!("chatonlm: transcription: not saved: {error}");
            }
        }
        if let Some(error) = &error
            && error.code == Code::Failed
        {
            crate::app_log!("chatonlm: transcription failed: {}", error.message);
        }
        self.shared.finish(|job| {
            job.stage = stage;
            job.error = error;
            job.received_bytes = None;
        });
    }
}

/// Échec inattendu (tâche, base) : générique, détail au journal.
fn failed(error: &dyn std::fmt::Display) -> TranscriptionError {
    crate::app_log!("chatonlm: transcription: {error}");
    TranscriptionError::with_message(
        Code::Failed,
        engine::transcription_failed(&error.to_string()),
    )
}

/// Échantillons `range` du fichier de travail.
fn read_samples(path: &Path, range: &std::ops::Range<usize>) -> std::io::Result<Vec<f32>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(range.start as u64 * 4))?;
    let mut bytes = vec![0u8; range.len() * 4];
    file.read_exact(&mut bytes)?;
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect())
}

/// Reçoit le son décodé : fichier de travail et relevé de la parole.
struct WorkSink {
    out: BufWriter<File>,
    analyzer: Analyzer,
    progress: Box<dyn Fn(Progress) + Send>,
}

impl Sink for WorkSink {
    fn start(&mut self, _info: &MediaInfo) -> std::io::Result<()> {
        Ok(())
    }

    fn samples(&mut self, block: &[f32]) -> std::io::Result<()> {
        let mut bytes = Vec::with_capacity(block.len() * 4);
        for sample in block {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.out.write_all(&bytes)?;
        self.analyzer.push(block);
        Ok(())
    }

    fn progress(&mut self, progress: Progress) {
        (self.progress)(progress);
    }
}
