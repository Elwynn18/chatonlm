//! Service de lecture : une lecture à la fois, état envoyé à la page à chaque
//! changement (`READ_ALOUD_EVENT`).
//!
//! - `read` : lit un texte enregistré (bouton d'une réponse, ou d'un texte
//!   d'un cours) ; voix absente : accord demandé, puis téléchargement et
//!   lecture à la fin ;
//! - `observe` : lecture automatique de la réponse d'un envoi pendant sa
//!   génération (événements du moteur de chat, si le réglage est actif ; les
//!   autres conversations sont ignorées) ; un nouvel envoi arrête toute
//!   lecture en cours ;
//! - `stop` : arrêt immédiat.
//!
//! Envoi du mode conversation vocale (`VoiceSend`, séance active) : sa
//! réponse est suivie même si la lecture automatique est coupée, sa langue
//! est choisie dès la première phrase complète (indice : langue de la phrase
//! transcrite), puis revérifiée sur `DETECT_CHARS` caractères (autre langue
//! que fr, en, es : lecture arrêtée ; autre voix : elle lit la suite), une
//! voix absente n'est pas téléchargée ; l'écouteur posé par la séance
//! apprend le premier son, l'erreur de la réponse et la fin de ces lectures,
//! avec sa raison, ainsi que le premier son et la fin des autres lectures,
//! dans l'ordre où ils sont décidés.
//!
//! Chaque lecture a son fil : il découpe le Markdown reçu en phrases (lecture
//! automatique), attend la voix (téléchargement) et la langue (détectée sur
//! les premières phrases), prend le moteur (gardé entre deux lectures, libéré
//! après 5 min d'inactivité par un seul fil), ouvre la sortie audio, puis
//! prétraite et synthétise phrase après phrase.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex, MutexGuard};

use super::text::{self, Segmenter};
use super::{
    ReadAloudError, ReadAloudOutcome, ReadAloudPhase, ReadAloudSource, ReadAloudState,
    VoiceLanguage,
};
use crate::chat::{ChatEvent, VoiceSend};
use crate::dictation::downloads::{SpeechDownload, SpeechDownloadState};
use crate::error::AppResult;

/// Moteur inutilisé depuis ce délai : libéré (~400 Mo).
const IDLE: Duration = Duration::from_secs(5 * 60);
/// Texte réuni avant de choisir la langue d'une lecture automatique (environ
/// une phrase) : attendre plus retarderait le premier son ; un texte encore
/// ambigu est lu dans la langue de l'interface.
const DETECT_CHARS: usize = 120;
/// Attente du moteur rendu par une lecture qui s'arrête (elle rend la main en
/// moins de 50 ms) avant d'en charger un autre.
const ENGINE_WAIT: Duration = Duration::from_secs(2);
/// Attente des fils de lecture à la sortie de l'app.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

/// Voix chargée (voir `engine::Synthesizer`).
pub trait Synth: Send {
    /// Synthétise `text` (prétraité) ; `first` : premier son de la lecture
    /// (rendu au plus vite) ; `sink` reçoit les échantillons au fil de
    /// l'eau ; rend la main dès que `stop` est levé.
    fn speak(
        &mut self,
        text: &str,
        first: bool,
        stop: &AtomicBool,
        sink: &mut dyn FnMut(&[f32]),
    ) -> Result<(), String>;
}

/// Sortie audio ouverte (voir `playback::Player`).
pub trait Output: Send + Sync {
    fn push(&self, samples: &[f32]);
    /// Silence immédiat (tampon vidé).
    fn stop(&self);
    /// Attend que tout soit joué ; faux si arrêté.
    fn drain(&self) -> bool;
}

/// Voix d'une langue, vue par le service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Voice {
    pub id: &'static str,
    pub bytes: u64,
    pub dir: PathBuf,
    pub installed: bool,
}

type VoiceOf = dyn Fn(VoiceLanguage) -> Option<Voice> + Send + Sync;
type Load = dyn Fn(&Voice, VoiceLanguage) -> Result<Box<dyn Synth>, ReadAloudError> + Send + Sync;
type Open = dyn Fn() -> Result<Arc<dyn Output>, ReadAloudError> + Send + Sync;

/// Ce dont le service a besoin (remplacé dans les essais).
pub struct Backend {
    pub voice: Box<VoiceOf>,
    pub load: Box<Load>,
    pub open: Box<Open>,
    /// Langue de l'interface : repli de la détection.
    pub interface: Box<dyn Fn() -> VoiceLanguage + Send + Sync>,
    /// ONNX Runtime utilisable (sinon aucune voix ne peut tourner).
    pub runtime: Box<dyn Fn() -> bool + Send + Sync>,
}

impl Backend {
    /// Moteur réel : voix de `root` (`voice-models`), Pocket TTS sur ONNX
    /// Runtime, sortie cpal.
    pub fn real(root: PathBuf) -> Self {
        Self {
            voice: Box::new(move |language| {
                let model = super::models::for_language(language)?;
                Some(Voice {
                    id: model.id,
                    bytes: model.size(),
                    dir: root.join(model.id),
                    installed: super::models::installed(&root, model),
                })
            }),
            load: Box::new(|voice, language| {
                crate::dictation::onnx::ensure().map_err(|_| ReadAloudError::RuntimeUnavailable)?;
                super::engine::Synthesizer::load(&voice.dir, language)
                    .map(|synth| Box::new(synth) as Box<dyn Synth>)
                    .map_err(|error| {
                        eprintln!("chatonlm: cannot load the voice {}: {error}", voice.id);
                        ReadAloudError::EngineFailed
                    })
            }),
            open: Box::new(|| {
                super::playback::Player::open()
                    .map(|player| Arc::new(player) as Arc<dyn Output>)
                    .map_err(|error| {
                        eprintln!("chatonlm: cannot open the audio output: {error}");
                        ReadAloudError::NoOutputDevice
                    })
            }),
            interface: Box::new(|| match crate::i18n::current() {
                crate::i18n::Lang::En => VoiceLanguage::En,
                crate::i18n::Lang::Es => VoiceLanguage::Es,
                crate::i18n::Lang::Fr => VoiceLanguage::Fr,
            }),
            runtime: Box::new(|| crate::dictation::onnx::ensure().is_ok()),
        }
    }
}

impl Synth for super::engine::Synthesizer {
    fn speak(
        &mut self,
        text: &str,
        first: bool,
        stop: &AtomicBool,
        sink: &mut dyn FnMut(&[f32]),
    ) -> Result<(), String> {
        super::engine::Synthesizer::speak_with(self, text, first, stop, sink)
            .map_err(|error| error.to_string())
    }
}

impl Output for super::playback::Player {
    fn push(&self, samples: &[f32]) {
        super::playback::Player::push(self, samples);
    }

    fn stop(&self) {
        super::playback::Player::stop(self);
    }

    fn drain(&self) -> bool {
        super::playback::Player::drain(self)
    }
}

type Emit = Box<dyn Fn(&ReadAloudState) + Send + Sync>;
type Listener = Box<dyn Fn(&str, VoiceReading) + Send + Sync>;

/// Ce que la séance du mode vocal apprend des lectures : celles de ses
/// envois vocaux (avec leur conversation), et les autres (conversation vide).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VoiceReading {
    /// Premier son de la réponse.
    Playing,
    /// La réponse a fini en erreur (message du moteur de chat) ; sa fin de
    /// lecture suit.
    ReplyFailed(String),
    /// Fin de la lecture, ou de l'envoi s'il n'a rien donné à lire.
    Ended(ReadingEnd),
    /// Une autre lecture (bouton d'une réponse, lecture automatique) joue
    /// (`true`, premier son) ou a fini (`false`).
    Aside(bool),
}

/// Raison de la fin d'une lecture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadingEnd {
    /// Tout a été lu.
    Finished,
    /// Arrêtée (bouton, nouvel envoi, réponse arrêtée, séance finie…).
    Stopped,
    /// Réponse vide, ou faite seulement de ce qui ne se lit pas (code…).
    NothingToRead,
    /// Réponse dans une autre langue que fr, en ou es.
    Unsupported,
    /// Voix de cette langue absente (pas téléchargée pour un envoi vocal).
    MissingVoice(VoiceLanguage),
    Failed(ReadAloudError),
}

/// Géré par Tauri.
pub struct ReadAloud {
    inner: Arc<Inner>,
}

struct Inner {
    backend: OnceLock<Backend>,
    emit: OnceLock<Emit>,
    /// Écouteur des lectures vocales (séance du mode vocal).
    listener: OnceLock<Listener>,
    /// Pris avant de lâcher `shared` par qui donne des événements à
    /// l'écouteur : ils arrivent dans l'ordre où ils ont été décidés.
    delivery: Mutex<()>,
    /// Réglage « lecture automatique ».
    auto_read: AtomicBool,
    /// Séance du mode vocal en cours : ses envois sont suivis.
    voice_active: AtomicBool,
    shared: Mutex<Shared>,
    /// Moteur gardé entre deux lectures.
    engine: Mutex<EngineSlot>,
    /// Moteur rendu (ou libéré) : réveille l'attente d'une lecture et le fil
    /// d'inactivité.
    engine_changed: Condvar,
    /// Fil d'inactivité lancé (un seul).
    reaper: OnceLock<()>,
    /// Fils de lecture en vie (attendus à la sortie).
    workers: AtomicUsize,
    next: AtomicU64,
    idle: Duration,
}

#[derive(Default)]
struct EngineSlot {
    loaded: Option<Loaded>,
    /// Moteur pris par une lecture.
    busy: bool,
    /// Incrémentée par la suppression d'une voix et la sortie : un moteur pris
    /// avant n'est pas gardé.
    epoch: u64,
    /// Sortie de l'app : plus rien n'est chargé ni gardé.
    closed: bool,
}

struct Shared {
    state: ReadAloudState,
    session: Option<Session>,
    /// Envoi suivi par la lecture automatique.
    follow: Option<Follow>,
    /// Événements des lectures vocales à donner à l'écouteur (conversation
    /// de l'envoi), envoyés par `publish`.
    outbox: Vec<(String, VoiceReading)>,
}

struct Session {
    id: u64,
    /// Texte lu (la réponse change d'un tour d'outils à l'autre en lecture
    /// automatique).
    source: ReadAloudSource,
    stop: Arc<AtomicBool>,
    input: mpsc::Sender<Input>,
    /// Sortie ouverte par le fil, coupée par `stop`.
    output: Arc<Mutex<Option<Arc<dyn Output>>>>,
    /// Voix attendue (téléchargement en cours) et sa langue.
    waiting: Option<(&'static str, VoiceLanguage)>,
    /// Lecture d'un envoi vocal : sa conversation.
    voice_chat: Option<String>,
    /// Autre lecture dont le premier son a été annoncé (`Aside(true)`).
    aside: bool,
}

enum Input {
    /// Texte parlé (lecture d'une réponse enregistrée).
    Text(String),
    /// Fragment de Markdown d'une réponse en cours (lecture automatique).
    Fragment(String),
    /// Tour d'outils suivant : la fin du message précédent est lue.
    NextMessage,
    End,
    VoiceReady,
    VoiceFailed,
    /// Téléchargement annulé : fin silencieuse.
    VoiceCancelled,
}

struct Follow {
    /// Conversation de l'envoi.
    chat: String,
    /// Lecture lancée au premier `ReplyStarted`.
    session: Option<u64>,
    /// Envoi vocal : langue de la phrase transcrite (indice de la voix).
    voice: Option<Voiced>,
}

#[derive(Clone, Copy)]
struct Voiced {
    hint: Option<VoiceLanguage>,
}

struct Loaded {
    language: VoiceLanguage,
    synth: Box<dyn Synth>,
    last: Instant,
}

impl Default for ReadAloud {
    fn default() -> Self {
        Self::new()
    }
}

impl ReadAloud {
    pub fn new() -> Self {
        Self::with_idle(IDLE)
    }

    fn with_idle(idle: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                backend: OnceLock::new(),
                emit: OnceLock::new(),
                listener: OnceLock::new(),
                delivery: Mutex::new(()),
                auto_read: AtomicBool::new(false),
                voice_active: AtomicBool::new(false),
                shared: Mutex::new(Shared {
                    state: ReadAloudState::idle(),
                    session: None,
                    follow: None,
                    outbox: Vec::new(),
                }),
                engine: Mutex::new(EngineSlot::default()),
                engine_changed: Condvar::new(),
                reaper: OnceLock::new(),
                workers: AtomicUsize::new(0),
                next: AtomicU64::new(1),
                idle,
            }),
        }
    }

    /// Réglé une fois, au démarrage.
    pub fn configure(
        &self,
        backend: Backend,
        emit: impl Fn(&ReadAloudState) + Send + Sync + 'static,
    ) {
        let _ = self.inner.backend.set(backend);
        let _ = self.inner.emit.set(Box::new(emit));
    }

    pub fn state(&self) -> ReadAloudState {
        self.inner.shared.lock().state.clone()
    }

    pub fn set_auto_read(&self, enabled: bool) {
        self.inner.auto_read.store(enabled, Ordering::Relaxed);
    }

    /// Écouteur des lectures vocales (posé une fois, par la séance) : appelé
    /// hors du verrou de l'état, sans qu'il puisse rappeler le service.
    pub fn set_voice_listener(
        &self,
        listener: impl Fn(&str, VoiceReading) + Send + Sync + 'static,
    ) {
        let _ = self.inner.listener.set(Box::new(listener));
    }

    /// Séance du mode vocal commencée ou finie : hors séance, un envoi vocal
    /// est suivi comme un envoi ordinaire.
    pub fn set_voice_active(&self, active: bool) {
        self.inner.voice_active.store(active, Ordering::Relaxed);
    }

    /// Arrête la lecture d'un envoi vocal (et son suivi) ; une lecture
    /// lancée à la main continue.
    pub fn stop_voice(&self) {
        let mut shared = self.inner.shared.lock();
        let reading = shared
            .session
            .as_ref()
            .is_some_and(|session| session.voice_chat.is_some());
        let following = shared
            .follow
            .as_ref()
            .is_some_and(|follow| follow.voice.is_some());
        if !reading && !following {
            return;
        }
        if following {
            Inner::drop_follow(&mut shared, ReadingEnd::Stopped);
        }
        if reading {
            Inner::halt(&mut shared);
        }
        self.inner.publish(shared);
    }

    /// Prépare le moteur de `language` (début d'une séance du mode vocal),
    /// si sa voix est installée et qu'il n'est pas déjà prêt.
    pub fn preload_language(&self, language: VoiceLanguage) {
        self.inner.preload_language(language);
    }

    /// Lit le texte `source` (son Markdown) ; `fallback` : langue du texte
    /// quand rien ne la trahit (celle d'un cours), sinon celle de
    /// l'interface. Voix absente et pas d'accord (`download` faux) :
    /// `NeedsVoice`, rien ne démarre, sauf si la voix est déjà en
    /// téléchargement (`downloading`) : la lecture l'attend. Avec l'accord,
    /// `queue` lance son téléchargement et la lecture suit. ONNX Runtime
    /// indisponible : erreur sous le texte, rien n'est téléchargé.
    pub fn read(
        &self,
        source: ReadAloudSource,
        markdown: &str,
        fallback: Option<VoiceLanguage>,
        download: bool,
        downloading: &dyn Fn(&str) -> bool,
        queue: &dyn Fn(&str) -> AppResult<()>,
    ) -> AppResult<ReadAloudOutcome> {
        let Some(backend) = self.inner.backend.get() else {
            return Ok(ReadAloudOutcome::NothingToRead);
        };
        let spoken = text::spoken_text(markdown);
        if spoken.trim().is_empty() {
            return Ok(ReadAloudOutcome::NothingToRead);
        }
        let fallback = fallback.unwrap_or_else(|| (backend.interface)());
        let Some((language, voice)) = text::choose_language(&spoken, fallback)
            .and_then(|language| Some((language, (backend.voice)(language)?)))
        else {
            return Ok(ReadAloudOutcome::UnsupportedLanguage);
        };
        if !(backend.runtime)() {
            self.inner
                .fail_now(source, ReadAloudError::RuntimeUnavailable);
            return Ok(ReadAloudOutcome::Started);
        }
        let already = !voice.installed && downloading(voice.id);
        if !voice.installed && !download && !already {
            return Ok(ReadAloudOutcome::NeedsVoice {
                model_id: voice.id.into(),
                language,
                bytes: voice.bytes,
            });
        }
        // Mise en file avant la lecture : une entrée périmée de la même voix
        // (terminée ou en échec) est déjà remplacée quand la lecture l'attend.
        if !voice.installed && !already {
            queue(voice.id)?;
        }
        let waiting = (!voice.installed).then_some((voice.id, language));
        let phase = if waiting.is_some() {
            ReadAloudPhase::Downloading
        } else {
            ReadAloudPhase::Preparing
        };
        let mut shared = self.inner.shared.lock();
        let (_, input) = self.inner.start(
            &mut shared,
            source,
            Some(language),
            waiting,
            Some(phase),
            None,
        );
        let _ = input.send(Input::Text(spoken));
        let _ = input.send(Input::End);
        self.inner.publish(shared);
        // Téléchargement fini entre-temps (fichiers déjà là) : rien ne
        // l'annoncera plus.
        if waiting.is_some() && (backend.voice)(language).is_some_and(|voice| voice.installed) {
            let _ = input.send(Input::VoiceReady);
        }
        Ok(ReadAloudOutcome::Started)
    }

    /// Arrête la lecture en cours (et la lecture automatique de l'envoi) ;
    /// rend la voix dont elle attendait le téléchargement, à annuler.
    pub fn stop(&self) -> Option<&'static str> {
        let mut shared = self.inner.shared.lock();
        Inner::drop_follow(&mut shared, ReadingEnd::Stopped);
        let waiting = Inner::halt(&mut shared);
        self.inner.publish(shared);
        waiting
    }

    /// Arrête la lecture si elle lit l'une des réponses `messages` (leur
    /// conversation est supprimée).
    pub fn stop_if_reading(&self, messages: &[i64]) {
        self.stop_if(|source| {
            matches!(source, ReadAloudSource::Message { message_id } if messages.contains(message_id))
        });
    }

    /// Arrête la lecture si elle lit un texte du cours `course` (supprimé,
    /// plan ou palier refait).
    pub fn stop_if_reading_course(&self, course: &str) {
        self.stop_if(|source| {
            matches!(source, ReadAloudSource::Study { course_id, .. } if course_id == course)
        });
    }

    fn stop_if(&self, concerned: impl Fn(&ReadAloudSource) -> bool) {
        let reading = self
            .inner
            .shared
            .lock()
            .session
            .as_ref()
            .is_some_and(|session| concerned(&session.source));
        if reading {
            self.stop();
        }
    }

    /// Avant la suppression de la voix de `language` : lecture arrêtée,
    /// moteur libéré (et pas gardé par une lecture qui finit).
    pub fn release_voice(&self, language: VoiceLanguage) {
        self.stop();
        let mut slot = self.inner.engine.lock();
        slot.epoch += 1;
        if slot
            .loaded
            .as_ref()
            .is_some_and(|loaded| loaded.language == language)
        {
            slot.loaded = None;
        }
        self.inner.engine_changed.notify_all();
    }

    /// Sortie de l'app : plus de son, plus de chargement, moteur libéré ;
    /// attend (2 s au plus) la fin des fils de lecture.
    pub fn shutdown(&self) {
        self.stop();
        {
            let mut slot = self.inner.engine.lock();
            slot.closed = true;
            slot.epoch += 1;
            slot.loaded = None;
            self.inner.engine_changed.notify_all();
        }
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while self.inner.workers.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Liste de la file des voix : progression ou fin du téléchargement
    /// attendu par la lecture.
    pub fn on_downloads(&self, list: &[SpeechDownload]) {
        let Some(backend) = self.inner.backend.get() else {
            return;
        };
        let mut shared = self.inner.shared.lock();
        let Shared { state, session, .. } = &mut *shared;
        let Some(session) = session.as_mut() else {
            return;
        };
        let Some(((_, language), entry)) = session.waiting.and_then(|waiting| {
            list.iter()
                .find(|download| download.id == waiting.0)
                .map(|entry| (waiting, entry))
        }) else {
            return;
        };
        match entry.state {
            SpeechDownloadState::Queued
            | SpeechDownloadState::Downloading
            | SpeechDownloadState::Verifying => {
                let percent = (entry.completed_bytes.saturating_mul(100))
                    .checked_div(entry.total_bytes)
                    .unwrap_or(0)
                    .min(100) as u8;
                state.percent = Some(percent);
            }
            SpeechDownloadState::Done => {
                // Entrée périmée (voix supprimée depuis) : la vraie suivra.
                if !(backend.voice)(language).is_some_and(|voice| voice.installed) {
                    return;
                }
                session.waiting = None;
                let _ = session.input.send(Input::VoiceReady);
                state.phase = ReadAloudPhase::Preparing;
                state.percent = None;
            }
            SpeechDownloadState::Failed => {
                session.waiting = None;
                let _ = session.input.send(Input::VoiceFailed);
            }
            SpeechDownloadState::Cancelled => {
                session.waiting = None;
                let _ = session.input.send(Input::VoiceCancelled);
            }
        }
        self.inner.publish(shared);
    }

    /// Événement du moteur de chat pour la conversation `chat` ; `voice` :
    /// demande d'un envoi vocal.
    pub fn observe(&self, chat: &str, event: &ChatEvent, voice: Option<&VoiceSend>) {
        if let ChatEvent::UserMessage { .. } = event {
            // Nouvel envoi : la lecture en cours s'arrête ; la réponse sera
            // lue si le réglage est actif, ou si c'est un envoi de la séance
            // du mode vocal.
            let auto = self.inner.auto_read.load(Ordering::Relaxed);
            let voiced = voice
                .filter(|_| self.inner.voice_active.load(Ordering::Relaxed))
                .map(|voice| Voiced {
                    hint: voice.language,
                });
            let mut shared = self.inner.shared.lock();
            Inner::drop_follow(&mut shared, ReadingEnd::Stopped);
            Inner::halt(&mut shared);
            shared.follow = (auto || voiced.is_some()).then(|| Follow {
                chat: chat.to_owned(),
                session: None,
                voice: voiced,
            });
            self.inner.publish(shared);
            match (voiced, self.inner.backend.get()) {
                (Some(voiced), Some(backend)) => self
                    .inner
                    .preload_language(voiced.hint.unwrap_or_else(|| (backend.interface)())),
                _ if auto => self.inner.preload(),
                _ => {}
            }
            return;
        }
        let mut shared = self.inner.shared.lock();
        let Some(follow) = shared.follow.as_ref().filter(|follow| follow.chat == chat) else {
            return;
        };
        let (followed, voiced) = (follow.session, follow.voice);
        // Lecture suivie encore en cours (sinon : remplacée ou finie).
        let current =
            followed.is_some() && shared.session.as_ref().map(|session| session.id) == followed;
        match event {
            ChatEvent::ReplyStarted { message_id, .. } => {
                if followed.is_none() {
                    // Envoi vocal : langue de repli de la première phrase.
                    let voice = voiced
                        .zip(self.inner.backend.get())
                        .map(|(voiced, backend)| {
                            let fallback = voiced.hint.unwrap_or_else(|| (backend.interface)());
                            (chat.to_owned(), fallback)
                        });
                    let source = ReadAloudSource::Message {
                        message_id: *message_id,
                    };
                    let (id, _) = self
                        .inner
                        .start(&mut shared, source, None, None, None, voice);
                    if let Some(follow) = shared.follow.as_mut() {
                        follow.session = Some(id);
                    }
                } else if current {
                    // Tour d'outils suivant : la fin du message précédent est
                    // lue, puis ce message-ci.
                    let source = ReadAloudSource::Message {
                        message_id: *message_id,
                    };
                    let Shared { state, session, .. } = &mut *shared;
                    if let Some(session) = session.as_mut() {
                        let _ = session.input.send(Input::NextMessage);
                        session.source = source.clone();
                    }
                    if state.phase != ReadAloudPhase::Idle {
                        state.source = Some(source);
                    }
                }
            }
            ChatEvent::Content { text } if current => {
                Inner::send(&shared, Input::Fragment(text.clone()));
            }
            ChatEvent::Done { .. } | ChatEvent::Error { .. } => {
                // Envoi vocal en erreur : dit avant sa fin de lecture.
                if let (ChatEvent::Error { message, .. }, Some(_)) = (event, voiced) {
                    let failed = VoiceReading::ReplyFailed(message.clone());
                    shared.outbox.push((chat.to_owned(), failed));
                }
                if current {
                    Inner::send(&shared, Input::End);
                }
                // Envoi vocal fini sans réponse : rien à lire.
                Inner::drop_follow(&mut shared, ReadingEnd::NothingToRead);
            }
            ChatEvent::Cancelled => {
                if current {
                    Inner::halt(&mut shared);
                }
                Inner::drop_follow(&mut shared, ReadingEnd::Stopped);
            }
            _ => {}
        }
        self.inner.publish(shared);
    }
}

impl Inner {
    /// Arrête la lecture en cours (sans changer `follow`) ; l'état passe au
    /// repos ; rend la voix qu'elle attendait.
    fn halt(shared: &mut Shared) -> Option<&'static str> {
        let waiting = shared.session.take().and_then(|session| {
            session.stop.store(true, Ordering::Relaxed);
            if let Some(output) = session.output.lock().as_ref() {
                output.stop();
            }
            if let Some(chat) = session.voice_chat {
                shared
                    .outbox
                    .push((chat, VoiceReading::Ended(ReadingEnd::Stopped)));
            } else if session.aside {
                shared
                    .outbox
                    .push((String::new(), VoiceReading::Aside(false)));
            }
            session.waiting.map(|(id, _)| id)
        });
        shared.state = ReadAloudState::idle();
        waiting
    }

    /// Fin du suivi de l'envoi : un envoi vocal dont la lecture n'a pas
    /// commencé finit avec `end` (une lecture commencée dit sa propre fin).
    fn drop_follow(shared: &mut Shared, end: ReadingEnd) {
        if let Some(follow) = shared.follow.take()
            && follow.voice.is_some()
            && follow.session.is_none()
        {
            shared.outbox.push((follow.chat, VoiceReading::Ended(end)));
        }
    }

    /// Envoie `input` à la lecture en cours.
    fn send(shared: &Shared, input: Input) {
        if let Some(session) = shared.session.as_ref() {
            let _ = session.input.send(input);
        }
    }

    /// Libère le verrou puis envoie l'état, et les événements des lectures
    /// vocales à l'écouteur (dans l'ordre : `delivery` est pris avant de
    /// lâcher l'état).
    fn publish(&self, mut shared: MutexGuard<'_, Shared>) {
        let state = shared.state.clone();
        let readings = std::mem::take(&mut shared.outbox);
        let delivery = (!readings.is_empty()).then(|| self.delivery.lock());
        drop(shared);
        if let Some(emit) = self.emit.get() {
            emit(&state);
        }
        if let Some(listener) = self.listener.get() {
            for (chat, reading) in readings {
                listener(&chat, reading);
            }
        }
        drop(delivery);
    }

    /// Erreur immédiate sous le texte `source` (la lecture en cours
    /// s'arrête).
    fn fail_now(&self, source: ReadAloudSource, error: ReadAloudError) {
        let mut shared = self.shared.lock();
        Inner::halt(&mut shared);
        shared.state = ReadAloudState {
            source: Some(source),
            error: Some(error),
            ..ReadAloudState::idle()
        };
        self.publish(shared);
    }

    /// Nouvelle lecture (la précédente s'arrête), sous le verrou ; `phase` :
    /// état montré tout de suite (`None` : rien avant la préparation, cas de
    /// la lecture automatique dont la langue n'est pas encore choisie) ;
    /// `voice` : lecture d'un envoi vocal (conversation, langue de repli).
    fn start(
        self: &Arc<Self>,
        shared: &mut Shared,
        source: ReadAloudSource,
        language: Option<VoiceLanguage>,
        waiting: Option<(&'static str, VoiceLanguage)>,
        phase: Option<ReadAloudPhase>,
        voice: Option<(String, VoiceLanguage)>,
    ) -> (u64, mpsc::Sender<Input>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (input, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let output = Arc::new(Mutex::new(None));
        Inner::halt(shared);
        shared.session = Some(Session {
            id,
            source: source.clone(),
            stop: stop.clone(),
            input: input.clone(),
            output: output.clone(),
            waiting,
            voice_chat: voice.as_ref().map(|(chat, _)| chat.clone()),
            aside: false,
        });
        if let Some(phase) = phase {
            shared.state = ReadAloudState {
                source: Some(source.clone()),
                phase,
                percent: waiting.map(|_| 0),
                error: None,
            };
        }
        let worker = Worker {
            inner: self.clone(),
            id,
            language,
            waiting: waiting.is_some(),
            input: receiver,
            stop,
            output,
            segmenter: language.is_none().then(Segmenter::default),
            voice: voice.as_ref().map(|(_, fallback)| *fallback),
            heard: String::new(),
            recheck: false,
        };
        self.workers.fetch_add(1, Ordering::AcqRel);
        let spawned = std::thread::Builder::new()
            .name("chatonlm-read-aloud".into())
            .spawn(move || worker.run());
        if let Err(error) = spawned {
            self.workers.fetch_sub(1, Ordering::AcqRel);
            eprintln!("chatonlm: cannot start reading aloud: {error}");
            shared.session = None;
            shared.state = ReadAloudState {
                source: Some(source),
                error: Some(ReadAloudError::EngineFailed),
                ..ReadAloudState::idle()
            };
            if let Some((chat, _)) = voice {
                let failed = ReadingEnd::Failed(ReadAloudError::EngineFailed);
                shared.outbox.push((chat, VoiceReading::Ended(failed)));
            }
        }
        (id, input)
    }

    /// État de la lecture `id` si elle est encore la lecture en cours.
    fn set_phase(&self, id: u64, phase: ReadAloudPhase) {
        let mut shared = self.shared.lock();
        let Some((source, voice_chat)) = shared
            .session
            .as_ref()
            .filter(|session| session.id == id)
            .map(|session| (session.source.clone(), session.voice_chat.clone()))
        else {
            return;
        };
        let state = ReadAloudState {
            source: Some(source),
            phase,
            percent: None,
            error: None,
        };
        if shared.state != state {
            shared.state = state;
            if phase == ReadAloudPhase::Playing {
                match voice_chat {
                    Some(chat) => shared.outbox.push((chat, VoiceReading::Playing)),
                    None => {
                        if let Some(session) = shared.session.as_mut() {
                            session.aside = true;
                        }
                        shared
                            .outbox
                            .push((String::new(), VoiceReading::Aside(true)));
                    }
                }
            }
            self.publish(shared);
        }
    }

    /// Fin de la lecture `id` (si elle est encore en cours) ; en erreur,
    /// l'état garde le texte concerné.
    fn finish(&self, id: u64, end: Result<(), ReadingEnd>) {
        let mut shared = self.shared.lock();
        let Some((source, voice_chat, aside)) = shared
            .session
            .as_ref()
            .filter(|session| session.id == id)
            .map(|session| {
                (
                    session.source.clone(),
                    session.voice_chat.clone(),
                    session.aside,
                )
            })
        else {
            return;
        };
        let end = end.err().unwrap_or(ReadingEnd::Finished);
        let error = match end {
            ReadingEnd::Failed(error) => Some(error),
            _ => None,
        };
        shared.session = None;
        shared.state = ReadAloudState {
            source: error.is_some().then_some(source),
            error,
            ..ReadAloudState::idle()
        };
        if let Some(chat) = voice_chat {
            shared.outbox.push((chat, VoiceReading::Ended(end)));
        } else if aside {
            shared
                .outbox
                .push((String::new(), VoiceReading::Aside(false)));
        }
        self.publish(shared);
    }

    /// Moteur de `language` : celui gardé (attendu s'il sert encore à une
    /// lecture qui s'arrête), sinon chargé. Rend aussi l'époque de la prise.
    fn take_engine(
        &self,
        language: VoiceLanguage,
        voice: &Voice,
    ) -> Result<(Box<dyn Synth>, u64), ReadAloudError> {
        let mut slot = self.engine.lock();
        let deadline = Instant::now() + ENGINE_WAIT;
        while slot.busy && !slot.closed {
            if self
                .engine_changed
                .wait_until(&mut slot, deadline)
                .timed_out()
            {
                break;
            }
        }
        if slot.closed {
            return Err(ReadAloudError::EngineFailed);
        }
        let epoch = slot.epoch;
        if let Some(loaded) = slot.loaded.take()
            && loaded.language == language
        {
            slot.busy = true;
            return Ok((loaded.synth, epoch));
        }
        // Autre langue (moteur lâché) ou rien : chargement, hors du verrou.
        slot.busy = true;
        drop(slot);
        let backend = self.backend.get().ok_or(ReadAloudError::EngineFailed);
        let loaded = backend.and_then(|backend| (backend.load)(voice, language));
        if loaded.is_err() {
            self.engine.lock().busy = false;
            self.engine_changed.notify_all();
        }
        loaded.map(|synth| (synth, epoch))
    }

    /// Rend le moteur pris à `epoch` : gardé (libéré après `idle` sans
    /// nouvelle lecture), sauf si une voix a été supprimée ou l'app ferme
    /// entre-temps, ou si `keep` est faux (moteur en erreur).
    fn give_back(
        self: &Arc<Self>,
        language: VoiceLanguage,
        synth: Box<dyn Synth>,
        epoch: u64,
        keep: bool,
    ) {
        let mut slot = self.engine.lock();
        slot.busy = false;
        if keep && !slot.closed && slot.epoch == epoch {
            slot.loaded = Some(Loaded {
                language,
                synth,
                last: Instant::now(),
            });
        }
        drop(slot);
        self.engine_changed.notify_all();
        self.start_reaper();
    }

    /// Un seul fil libère le moteur gardé après `idle` sans lecture.
    fn start_reaper(self: &Arc<Self>) {
        if self.reaper.set(()).is_err() {
            return;
        }
        let inner = Arc::downgrade(self);
        let _ = std::thread::Builder::new()
            .name("chatonlm-read-aloud-idle".into())
            .spawn(move || {
                loop {
                    let Some(inner) = inner.upgrade() else {
                        return;
                    };
                    let mut slot = inner.engine.lock();
                    if slot.closed {
                        return;
                    }
                    let wait = match slot.loaded.as_ref() {
                        Some(loaded) if !slot.busy => {
                            let elapsed = loaded.last.elapsed();
                            if elapsed >= inner.idle {
                                slot.loaded = None;
                                continue;
                            }
                            inner.idle - elapsed
                        }
                        _ => inner.idle,
                    };
                    inner.engine_changed.wait_for(&mut slot, wait);
                }
            });
    }

    /// Lecture automatique : prépare le moteur de la langue de l'interface
    /// pendant que le modèle réfléchit (gagné si la réponse est dans cette
    /// langue), s'il est installé et qu'aucun moteur n'est gardé.
    fn preload(self: &Arc<Self>) {
        let Some(backend) = self.backend.get() else {
            return;
        };
        if self.engine.lock().loaded.is_some() || !(backend.runtime)() {
            return;
        }
        let language = (backend.interface)();
        let Some(voice) = (backend.voice)(language).filter(|voice| voice.installed) else {
            return;
        };
        let inner = self.clone();
        let _ = std::thread::Builder::new()
            .name("chatonlm-read-aloud-preload".into())
            .spawn(move || {
                {
                    let slot = inner.engine.lock();
                    if slot.loaded.is_some() || slot.busy {
                        return;
                    }
                }
                if let Ok((synth, epoch)) = inner.take_engine(language, &voice) {
                    inner.give_back(language, synth, epoch, true);
                }
            });
    }

    /// Prépare le moteur de `language` s'il n'est pas déjà gardé (celui
    /// d'une autre langue est remplacé), si sa voix est installée et qu'aucune
    /// lecture ne le tient.
    fn preload_language(self: &Arc<Self>, language: VoiceLanguage) {
        let Some(backend) = self.backend.get() else {
            return;
        };
        let ready = move |slot: &EngineSlot| {
            slot.busy
                || slot.closed
                || slot
                    .loaded
                    .as_ref()
                    .is_some_and(|loaded| loaded.language == language)
        };
        if ready(&self.engine.lock()) || !(backend.runtime)() {
            return;
        }
        let Some(voice) = (backend.voice)(language).filter(|voice| voice.installed) else {
            return;
        };
        let inner = self.clone();
        let _ = std::thread::Builder::new()
            .name("chatonlm-read-aloud-preload".into())
            .spawn(move || {
                if ready(&inner.engine.lock()) {
                    return;
                }
                if let Ok((synth, epoch)) = inner.take_engine(language, &voice) {
                    inner.give_back(language, synth, epoch, true);
                }
            });
    }
}

/// Fil d'une lecture.
struct Worker {
    inner: Arc<Inner>,
    id: u64,
    language: Option<VoiceLanguage>,
    waiting: bool,
    input: mpsc::Receiver<Input>,
    stop: Arc<AtomicBool>,
    output: Arc<Mutex<Option<Arc<dyn Output>>>>,
    /// Découpage du Markdown reçu en fragments (lecture automatique).
    segmenter: Option<Segmenter>,
    /// Lecture d'un envoi vocal : langue de repli, langue choisie dès la
    /// première phrase.
    voice: Option<VoiceLanguage>,
    /// Envoi vocal : texte reçu depuis le début de la réponse (borné), pour
    /// revérifier la langue choisie sur la première phrase.
    heard: String,
    /// Langue choisie sur moins de `DETECT_CHARS` caractères : revérifiée dès
    /// qu'ils sont réunis (ou à la fin).
    recheck: bool,
}

/// Ce que le fil a reçu et pas encore lu.
#[derive(Default)]
struct Pending {
    sentences: VecDeque<String>,
    ended: bool,
}

/// Fin de `Worker::play`.
enum Played {
    Done,
    /// Langue revérifiée : la suite est lue par la voix de cette langue.
    Switch(VoiceLanguage, Pending),
}

/// Texte gardé pour revérifier la langue (la détection en lit autant).
const HEARD_CHARS: usize = 600;

impl Worker {
    fn run(mut self) {
        let (inner, id) = (self.inner.clone(), self.id);
        // Une panique (texte ou moteur) finit la lecture en erreur et rend
        // le moteur, au lieu de laisser un fil mort et un moteur pris.
        let end = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.speak()))
            .unwrap_or_else(|_| {
                eprintln!("chatonlm: reading aloud failed (panic)");
                let mut slot = inner.engine.lock();
                if slot.busy {
                    slot.busy = false;
                    inner.engine_changed.notify_all();
                }
                Err(ReadingEnd::Failed(ReadAloudError::EngineFailed))
            });
        inner.finish(id, end);
        inner.workers.fetch_sub(1, Ordering::AcqRel);
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Range `input` ; rend le changement d'attente de la voix.
    fn take(&mut self, input: Input, pending: &mut Pending) -> Option<Result<(), ReadAloudError>> {
        if pending.ended
            && matches!(
                input,
                Input::Text(_) | Input::Fragment(_) | Input::NextMessage | Input::End
            )
        {
            return None;
        }
        let sentences = match input {
            Input::Text(text) => vec![text],
            Input::Fragment(fragment) => self
                .segmenter
                .as_mut()
                .map(|segmenter| segmenter.push(&fragment))
                .unwrap_or_default(),
            Input::NextMessage => self
                .segmenter
                .as_mut()
                .map(|segmenter| std::mem::take(segmenter).finish())
                .unwrap_or_default(),
            Input::End => {
                pending.ended = true;
                self.segmenter
                    .take()
                    .map(Segmenter::finish)
                    .unwrap_or_default()
            }
            Input::VoiceReady => return Some(Ok(())),
            Input::VoiceFailed => return Some(Err(ReadAloudError::DownloadFailed)),
            Input::VoiceCancelled => return Some(Err(ReadAloudError::EngineFailed)),
        };
        if self.voice.is_some() {
            for sentence in &sentences {
                if self.heard.len() < HEARD_CHARS {
                    self.heard.push_str(sentence);
                    self.heard.push(' ');
                }
            }
        }
        pending.sentences.extend(sentences);
        None
    }

    /// Langue de la lecture, si elle peut déjà être choisie : sur la
    /// première phrase complète pour un envoi vocal (repli : l'indice) ;
    /// sinon sur `DETECT_CHARS` caractères ou toute la réponse (repli : la
    /// langue de l'interface). Erreur : autre langue que fr, en, es.
    fn choose(
        &self,
        pending: &Pending,
        interface: VoiceLanguage,
    ) -> Result<Option<VoiceLanguage>, ReadingEnd> {
        let sample = match self.voice {
            Some(_) if pending.sentences.is_empty() && !pending.ended => return Ok(None),
            Some(_) => pending.sentences.front().cloned().unwrap_or_default(),
            None => {
                let gathered: usize = pending.sentences.iter().map(String::len).sum();
                if !pending.ended && gathered < DETECT_CHARS {
                    return Ok(None);
                }
                Vec::from(pending.sentences.clone()).join(" ")
            }
        };
        text::choose_language(&sample, self.voice.unwrap_or(interface))
            .map(Some)
            .ok_or(ReadingEnd::Unsupported)
    }

    /// Erreur : fin de la lecture, `Failed` montrée sous la réponse, les
    /// autres silencieuses (arrêt, rien à lire, autre langue, voix absente
    /// sans téléchargement, téléchargement annulé).
    fn speak(&mut self) -> Result<(), ReadingEnd> {
        let inner = self.inner.clone();
        let backend = inner.backend.get().ok_or(ReadingEnd::Stopped)?;
        let mut pending = Pending::default();
        let mut waiting = self.waiting;
        let mut language = self.language;
        // Voix (téléchargement) et langue (lecture automatique) d'abord.
        let voice = loop {
            if self.stopped() {
                return Err(ReadingEnd::Stopped);
            }
            if !waiting {
                if pending.ended && pending.sentences.is_empty() {
                    return Err(ReadingEnd::NothingToRead);
                }
                if language.is_none() {
                    language = self.choose(&pending, (backend.interface)())?;
                    // Envoi vocal : choisie sur la première phrase, revérifiée
                    // sur plus de texte.
                    self.recheck = language.is_some()
                        && self.voice.is_some()
                        && !pending.ended
                        && self.heard.len() < DETECT_CHARS;
                }
                if let Some(language) = language {
                    let voice = (backend.voice)(language).ok_or(ReadingEnd::Unsupported)?;
                    if voice.installed {
                        break voice;
                    }
                    if !self.waiting {
                        // Lecture automatique sans la voix : rien n'est lu.
                        return Err(ReadingEnd::MissingVoice(language));
                    }
                    // Annoncée prête mais absente : attendre encore.
                    waiting = true;
                }
            }
            let Ok(input) = self.input.recv() else {
                return Err(ReadingEnd::Stopped);
            };
            let cancelled = matches!(input, Input::VoiceCancelled);
            match self.take(input, &mut pending) {
                Some(Ok(())) => waiting = false,
                Some(Err(_)) if cancelled => return Err(ReadingEnd::Stopped),
                Some(Err(error)) => return Err(ReadingEnd::Failed(error)),
                None => {}
            }
        };
        let mut language = language.ok_or(ReadingEnd::Unsupported)?;
        if pending.sentences.is_empty() && pending.ended {
            return Err(ReadingEnd::NothingToRead);
        }
        self.inner.set_phase(self.id, ReadAloudPhase::Preparing);
        let (mut synth, mut epoch) = self
            .inner
            .take_engine(language, &voice)
            .map_err(ReadingEnd::Failed)?;
        let output = match (backend.open)() {
            Ok(output) => output,
            Err(error) => {
                self.inner.give_back(language, synth, epoch, true);
                return Err(ReadingEnd::Failed(error));
            }
        };
        *self.output.lock() = Some(output.clone());
        if self.stopped() {
            output.stop();
        }
        let result = loop {
            match self.play(&mut *synth, &*output, language, pending) {
                Ok(Played::Switch(next, rest)) => {
                    // Autre langue reconnue : la voix de cette langue lit la
                    // suite (moteur rendu avant d'en prendre un autre).
                    self.inner.give_back(language, synth, epoch, true);
                    let Some(voice) = (backend.voice)(next).filter(|voice| voice.installed) else {
                        output.stop();
                        return Err(ReadingEnd::MissingVoice(next));
                    };
                    (synth, epoch) = match self.inner.take_engine(next, &voice) {
                        Ok(taken) => taken,
                        Err(error) => {
                            output.stop();
                            return Err(ReadingEnd::Failed(error));
                        }
                    };
                    language = next;
                    pending = rest;
                }
                played => break played.map(|_| ()),
            }
        };
        // Moteur en erreur : pas gardé.
        let healthy = result != Err(ReadingEnd::Failed(ReadAloudError::EngineFailed));
        self.inner.give_back(language, synth, epoch, healthy);
        if self.stopped() {
            output.stop();
            return Err(ReadingEnd::Stopped);
        }
        if result == Err(ReadingEnd::Unsupported) {
            // Réponse dans une autre langue : plus un mot.
            output.stop();
        }
        result?;
        output.drain();
        Ok(())
    }

    /// Prétraite et synthétise chaque phrase, jusqu'à la fin ou l'arrêt ;
    /// langue d'un envoi vocal revérifiée dès que le texte suffit : autre
    /// langue que fr, en, es → `Unsupported` ; autre voix → `Switch`.
    fn play(
        &mut self,
        synth: &mut dyn Synth,
        output: &dyn Output,
        language: VoiceLanguage,
        mut pending: Pending,
    ) -> Result<Played, ReadingEnd> {
        let mut playing = false;
        loop {
            if self.stopped() {
                return Err(ReadingEnd::Stopped);
            }
            let remaining = !pending.sentences.is_empty() || !pending.ended;
            if self.recheck && remaining && (pending.ended || self.heard.len() >= DETECT_CHARS) {
                self.recheck = false;
                match text::choose_language(&self.heard, language) {
                    None => return Err(ReadingEnd::Unsupported),
                    Some(next) if next != language => return Ok(Played::Switch(next, pending)),
                    Some(_) => {}
                }
            }
            let Some(sentence) = pending.sentences.pop_front() else {
                if pending.ended {
                    return Ok(Played::Done);
                }
                let Ok(input) = self.input.recv() else {
                    return Err(ReadingEnd::Stopped);
                };
                self.take(input, &mut pending);
                continue;
            };
            let prepared = text::normalize(&sentence, language);
            if prepared.trim().is_empty() {
                continue;
            }
            let (inner, id) = (&self.inner, self.id);
            synth
                .speak(&prepared, !playing, &self.stop, &mut |samples| {
                    output.push(samples);
                    if !playing {
                        playing = true;
                        inner.set_phase(id, ReadAloudPhase::Playing);
                    }
                })
                .map_err(|error| {
                    eprintln!("chatonlm: reading aloud failed: {error}");
                    ReadingEnd::Failed(ReadAloudError::EngineFailed)
                })?;
        }
    }
}

#[cfg(test)]
mod tests;
