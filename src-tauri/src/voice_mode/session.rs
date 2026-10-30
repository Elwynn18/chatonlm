//! Séance du mode conversation vocale : machine d'états appelée par les
//! commandes (page), l'écoute (`audio::Ear`, par une tâche qui en relaie les
//! événements), l'observateur du chat (envoi vocal du tour) et l'écouteur de
//! la lecture à voix haute (premier son, fin de la lecture).
//!
//! ```text
//! Off → Starting → Listening ⇄ Hearing → Transcribing → Responding → Speaking
//!                      ↑            (phrase vide, sans parole, échec)  │
//!                      └────────────── fin de lecture, interruption ───┘
//! Listening → Paused (2 min sans parole, micro perdu) → Listening (resume)
//! Responding/Speaking → Hearing (parole franche : barge-in, appui en
//! push-to-talk) ; toute phase → Off (stop)
//! ```
//!
//! Règles :
//! - un numéro de séance écarte les suites d'une séance finie (démarrage,
//!   transcription, délais), un numéro d'écoute les événements d'un micro
//!   fermé (coupé, en pause, perdu) ;
//! - démarrage : dictée réservée (refusé si elle est en cours, et elle refuse
//!   de démarrer jusqu'à la fin de la séance), prérequis vérifiés, modèle de
//!   transcription tenu et préchargé (enregistré dans les réglages de la
//!   dictée s'ils n'en ont pas), voix de la langue attendue préchargée, micro
//!   ouvert ;
//! - tour : phrase sans parole (`has_speech`) ou transcrite vide : nouvelle
//!   écoute sans avis ; sinon tour suivant, phrase envoyée à la page
//!   (`voice-mode-utterance`), qui l'envoie au chat avec `voice` ; la
//!   conversation du tour est celle de son envoi vocal (observateur) ; le
//!   premier son de la lecture passe en Speaking, sa fin rend l'écoute (avec
//!   un avis : réponse en erreur, autre langue, voix absente, lecture en
//!   échec) ; un envoi qui échoue avant d'écrire sa phrase finit le tour ;
//! - une autre lecture (bouton d'une réponse) pendant l'écoute : seule une
//!   parole franche est signalée, comme pendant une réponse, et l'arrête ;
//! - interruption (commande, parole franche, appui en push-to-talk) : lecture
//!   vocale arrêtée et génération de la conversation du tour arrêtée (attendue
//!   avant d'envoyer la phrase suivante) ; un envoi vocal qui arrive après
//!   l'interruption de son tour est arrêté aussi ;
//! - muet : micro fermé, phase gardée (une parole en cours est perdue) ; la
//!   réponse en cours continue d'être lue ;
//! - micro perdu : avis gardé jusqu'à la reprise (pause à la fin du tour) ;
//! - fin : micro fermé, lecture vocale arrêtée (la génération continue),
//!   modèle relâché, dictée rendue ;
//! - propriétaire : la page, ou une connexion du pont VS Code (« chaton en
//!   caoutchouc », visite guidée) ; une séance tenue par l'éditeur reçoit
//!   seule ses phrases, ses niveaux et ses états (la page voit l'état, marqué
//!   `editor`), n'est arrêtée ni par la page, ni par le masquage de la
//!   fenêtre, ni par son rechargement, mais l'est par la fin de sa connexion,
//!   le retrait de l'éditeur, la coupure du pont, la sortie de l'app et le
//!   bouton Stop du bandeau de la page (geste explicite de l'utilisateur) ;
//!   elle est toujours en mains libres (le pont n'a pas d'appui), reprend
//!   après une pause par sa connexion, et ses tours continuent la numérotation
//!   de la connexion ; un démarrage refusé n'envoie pas `Off` à l'éditeur (sa
//!   réponse d'erreur le dit d'abord) ; une seule séance à la fois : chacun
//!   est refusé quand l'autre la tient ;
//! - le verrou de la séance n'est jamais tenu pendant un appel qui touche à
//!   l'état de la lecture à voix haute (elle rappelle la séance sous le sien)
//!   ni pendant l'ouverture ou la fermeture du micro (bloquantes) ; l'état
//!   est envoyé à la page sous le verrou, dans l'ordre des changements.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

use super::audio::{Ear, Events, Gate, ListenConfig, ListenEvent, Listening, TurnMode};
use super::{
    LEVEL_EVENT, STATE_EVENT, UTTERANCE_EVENT, VoiceModeLevel, VoiceModeNotice, VoiceModePhase,
    VoiceModeRequirements, VoiceModeState, VoiceModeUtterance,
};
use crate::chat::{ChatEngine, ChatEvent, VoiceSend};
use crate::dictation::clipboard::BoxFuture;
use crate::dictation::engine::{self, Engine, Hold, ModelSpec, Request};
use crate::dictation::models::{self as speech_models, CatalogModel, SpeechEngine};
use crate::dictation::{Dictation, languages};
use crate::i18n::t;
use crate::store::Store;
use crate::store::dictation::{DictationPatch, DictationSettings};
use crate::store::voice_mode::{VoiceModeSettings, VoiceTurnMode};
use crate::tts::service::{ReadingEnd, VoiceReading};
use crate::tts::{ReadAloud, VoiceLanguage};

// `Listening` seul désigne l'écoute (trait de `audio`), la phase s'écrit
// `VoiceModePhase::Listening`.
use VoiceModePhase::{Hearing, Off, Paused, Responding, Speaking, Starting, Transcribing};

/// Écoute sans parole au-delà de ce délai : pause (micro fermé).
pub const PAUSE_AFTER: Duration = Duration::from_secs(120);

/// `1` : une ligne de mesures par tour sur stderr (`chatonlm-voice-timing`).
pub const TIMINGS_ENV: &str = "CHATONLM_VOICE_TIMINGS";

/// État de la séance (géré par Tauri).
pub struct VoiceMode {
    core: Arc<Core>,
}

impl VoiceMode {
    /// Séance de l'app : vraie écoute `ear`, moteur de transcription partagé
    /// avec la dictée.
    pub fn for_app(app: &AppHandle, ear: Arc<dyn Ear>, engine: Engine) -> Self {
        let services = Services {
            ear,
            engine,
            support: crate::dictation::engine_support,
            pause_after: PAUSE_AFTER,
            timings: std::env::var_os(TIMINGS_ENV).is_some_and(|value| value == "1"),
        };
        Self {
            core: Core::new(services, Arc::new(AppHost(app.clone()))),
        }
    }

    pub fn state(&self) -> VoiceModeState {
        self.core.inner.lock().state.clone()
    }

    /// Démarre pour la page (sans effet si elle tient déjà une séance) ;
    /// erreur traduite si l'éditeur tient la séance, si une dictée est en
    /// cours, si un prérequis manque, si le micro ne s'ouvre pas.
    pub async fn start(&self) -> Result<(), String> {
        self.core.start().await
    }

    /// Fin de la séance de la page (bouton, masquage de la fenêtre, page
    /// rechargée) ; une séance tenue par l'éditeur continue.
    pub fn stop(&self) {
        self.core.stop_where(false, Some(Holder::Page));
    }

    pub async fn mute(&self, muted: bool) -> Result<(), String> {
        if !self.core.held_by(Holder::Page) {
            return Ok(());
        }
        self.core.mute(muted).await
    }

    pub fn interrupt(&self) {
        if self.core.held_by(Holder::Page) {
            self.core.interrupt();
        }
    }

    pub fn end_turn(&self) {
        if self.core.held_by(Holder::Page) {
            self.core.end_turn();
        }
    }

    pub fn hold(&self, pressed: bool) {
        if self.core.held_by(Holder::Page) {
            self.core.hold(pressed);
        }
    }

    pub async fn resume(&self) -> Result<(), String> {
        if !self.core.held_by(Holder::Page) {
            return Ok(());
        }
        self.core.resume().await
    }

    pub fn reply_failed(&self, turn: u32, message: String) {
        if self.core.held_by(Holder::Page) {
            self.core.reply_failed(turn, message);
        }
    }

    /// Démarre une séance tenue par une connexion du pont (`link`) ; refusée
    /// si une séance est en cours (page ou éditeur), si un prérequis manque,
    /// si le micro ne s'ouvre pas ou si la connexion est déjà fermée.
    pub(crate) async fn start_for_editor(
        &self,
        link: Arc<dyn EditorLink>,
    ) -> Result<(), StartError> {
        self.core.start_as(Owner::Editor(link)).await
    }

    /// Fin de la séance si la connexion `connection` la tient (fin de la
    /// connexion, éditeur retiré, pont coupé, `voice.stop`).
    pub fn stop_editor(&self, connection: u64) {
        self.core
            .stop_where(false, Some(Holder::Editor(connection)));
    }

    /// Bouton Stop du bandeau de la page : fin de la séance tenue par
    /// l'éditeur, quel qu'il soit (une séance de la page continue).
    pub fn stop_editor_session(&self) {
        let holder = self.core.inner.lock().owner.as_ref().map(Owner::holder);
        if let Some(holder @ Holder::Editor(_)) = holder {
            self.core.stop_where(false, Some(holder));
        }
    }

    /// Paused : micro rouvert, si la connexion tient la séance.
    pub async fn resume_editor(&self, connection: u64) -> Result<(), String> {
        if !self.core.held_by(Holder::Editor(connection)) {
            return Ok(());
        }
        self.core.resume().await
    }

    /// Micro coupé ou rouvert, si la connexion tient la séance.
    pub async fn mute_editor(&self, connection: u64, muted: bool) -> Result<(), String> {
        if !self.core.held_by(Holder::Editor(connection)) {
            return Ok(());
        }
        self.core.mute(muted).await
    }

    /// L'envoi du tour `turn` de la séance de l'éditeur a été refusé
    /// d'emblée : avis, puis écoute.
    pub fn reply_failed_editor(&self, connection: u64, turn: u32, message: String) {
        if self.core.held_by(Holder::Editor(connection)) {
            self.core.reply_failed(turn, message);
        }
    }

    pub fn settings_changed(&self, settings: &VoiceModeSettings) {
        self.core.settings_changed(settings);
    }

    /// Événement du moteur de chat (observateur) : conversation du tour.
    pub fn observe(&self, chat: &str, event: &ChatEvent, voice: Option<&VoiceSend>) {
        self.core.observe(chat, event, voice);
    }

    /// Lecture d'un envoi vocal (écouteur de la lecture à voix haute).
    pub fn on_reading(&self, chat: &str, reading: VoiceReading) {
        self.core.on_reading(chat, reading);
    }

    /// Conversation supprimée : fin de la séance si c'est la sienne.
    pub fn chat_deleted(&self, chat: &str) {
        self.core.chat_deleted(chat);
    }

    /// Sortie de l'app : séance finie (micro fermé avant le retour), plus
    /// rien ne commence.
    pub fn shutdown(&self) {
        self.core.inner.lock().closed = true;
        self.core.stop(true);
    }
}

/// Ce dont la séance a besoin, lu à chaque démarrage.
#[derive(Clone, Debug)]
pub(crate) struct Prepared {
    pub requirements: VoiceModeRequirements,
    /// Modèle de transcription employé.
    pub speech: ModelSpec,
    /// Les réglages de la dictée n'ont pas de modèle : celui employé (s'il
    /// est installé) y sera enregistré.
    pub remember: bool,
    /// Langue et dictionnaire de la dictée.
    pub request: Request,
    pub settings: VoiceModeSettings,
}

/// Qui tient la séance.
#[derive(Clone)]
pub(crate) enum Owner {
    Page,
    /// Une connexion du pont VS Code : reçoit les états, les niveaux et les
    /// phrases de la séance.
    Editor(Arc<dyn EditorLink>),
}

impl Owner {
    fn holder(&self) -> Holder {
        match self {
            Owner::Page => Holder::Page,
            Owner::Editor(link) => Holder::Editor(link.connection()),
        }
    }

    /// Tours déjà comptés avant la séance : ceux de la connexion pour
    /// l'éditeur (numérotation croissante sur toute la connexion), aucun
    /// pour la page.
    fn first_turn(&self) -> u32 {
        match self {
            Owner::Page => 0,
            Owner::Editor(link) => link.first_turn(),
        }
    }
}

/// Qui tient la séance, vu des commandes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holder {
    Page,
    /// Connexion du pont.
    Editor(u64),
}

/// Connexion du pont qui tient une séance (voir `bridge::voice`). Appelée
/// sous le verrou de la séance : rien n'y attend, rien n'y rappelle la
/// séance.
pub(crate) trait EditorLink: Send + Sync + 'static {
    fn connection(&self) -> u64;
    /// La connexion est encore ouverte et veut toujours la séance (ni
    /// fermée, ni `voice.stop` reçu pendant le démarrage) ; sinon la séance
    /// ne commence pas.
    fn alive(&self) -> bool;
    /// Dernier tour de la connexion (séances précédentes comprises) : la
    /// séance numérote ses tours à partir du suivant.
    fn first_turn(&self) -> u32;
    fn changed(&self, state: &VoiceModeState);
    fn level(&self, level: VoiceModeLevel);
    /// Phrase transcrite, à envoyer au chat par le moteur.
    fn utterance(&self, utterance: &VoiceModeUtterance);
}

/// Refus d'un démarrage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartError {
    /// Une séance est en cours : tenue par la page (`page`) ou par un
    /// éditeur.
    Busy {
        page: bool,
    },
    SpeechModelMissing,
    VoiceMissing(VoiceLanguage),
    /// Dictée en cours, machine incapable, micro qui ne s'ouvre pas,
    /// connexion fermée… (texte traduit).
    Failed(String),
}

impl StartError {
    /// Texte traduit (page).
    pub fn message(&self) -> String {
        match self {
            StartError::Busy { page: false } => t!("voiceMode.heldByEditor"),
            StartError::Busy { page: true } => t!("voiceMode.heldByPage"),
            StartError::SpeechModelMissing => t!("voiceMode.speechModelMissing"),
            StartError::VoiceMissing(_) => t!("voiceMode.voiceMissing"),
            StartError::Failed(message) => message.clone(),
        }
    }
}

impl From<String> for StartError {
    fn from(message: String) -> Self {
        StartError::Failed(message)
    }
}

/// L'app vue par la séance (remplacée dans les essais). Seuls
/// `stop_reading`, `stop_any_reading` et `preload_voice` touchent à la
/// lecture à voix haute : jamais appelés sous le verrou de la séance.
pub(crate) trait Host: Send + Sync + 'static {
    fn prepare(&self) -> BoxFuture<Result<Prepared, String>>;
    /// Enregistre `id` comme modèle de la dictée.
    fn remember_speech_model(&self, id: String) -> BoxFuture<()>;
    /// Réserve la dictée pour la séance : faux si une dictée est en cours.
    fn reserve(&self) -> bool;
    fn release(&self);
    fn changed(&self, state: &VoiceModeState);
    fn level(&self, level: VoiceModeLevel);
    fn utterance(&self, utterance: &VoiceModeUtterance);
    /// Séance commencée ou finie : les envois vocaux sont lus.
    fn reading_active(&self, active: bool);
    /// Arrête la lecture d'un envoi vocal.
    fn stop_reading(&self);
    /// Arrête toute lecture (parole franche sur une lecture hors tour).
    fn stop_any_reading(&self);
    fn preload_voice(&self, language: VoiceLanguage);
    /// Arrête la réponse en cours de la conversation `chat` et attend sa fin.
    fn stop_reply(&self, chat: String) -> BoxFuture<()>;
}

/// Écoute, moteur de transcription et délais (faux dans les essais).
pub(crate) struct Services {
    pub ear: Arc<dyn Ear>,
    pub engine: Engine,
    /// Refus d'un moteur que cette machine ne peut pas faire tourner.
    pub support: fn(SpeechEngine) -> Result<(), String>,
    /// Écoute sans parole avant la pause.
    pub pause_after: Duration,
    /// Mesures par tour sur stderr.
    pub timings: bool,
}

struct Core {
    services: Services,
    host: Arc<dyn Host>,
    inner: Mutex<Inner>,
    me: Weak<Core>,
}

#[derive(Default)]
struct Inner {
    /// Numéro de la séance : change au début et à la fin.
    session: u64,
    state: VoiceModeState,
    /// Sortie de l'app : plus rien ne commence.
    closed: bool,
    /// Écoute ouverte (micro) et son numéro ; dernier numéro donné.
    listening: Option<Box<dyn Listening>>,
    mic: u64,
    next_mic: u64,
    /// Gate demandée à l'écoute ouverte.
    gate: Option<Gate>,
    /// Modèle de transcription tenu (relâché en pause).
    hold: Option<Hold>,
    run: Option<Run>,
    /// Conversation de l'envoi vocal du tour en cours.
    conversation: Option<String>,
    /// Conversation des envois de la séance.
    session_chat: Option<String>,
    /// Arrêt d'une réponse interrompue, attendu avant la phrase suivante.
    stopping: Option<tauri::async_runtime::JoinHandle<()>>,
    /// Change à chaque changement de phase : un délai de pause périmé ne
    /// fait rien.
    step: u64,
    /// Abandon de la transcription en cours (fin de séance).
    abort: Arc<AtomicBool>,
    timing: Option<Timing>,
    /// Erreur de la réponse du tour (moteur de chat), montrée à sa fin.
    reply_error: Option<String>,
    /// Micro perdu (texte de l'écoute) : avis de la pause qui suit, jusqu'à
    /// la reprise.
    mic_lost: Option<String>,
    /// Une autre lecture que celle d'un tour joue (bouton d'une réponse) :
    /// l'écoute ne signale qu'une parole franche, comme pendant une réponse.
    aside: bool,
    /// Qui tient la séance (hors de `Off`).
    owner: Option<Owner>,
}

/// Réglages d'une séance démarrée.
struct Run {
    speech: ModelSpec,
    request: Request,
    settings: VoiceModeSettings,
    /// Langue attendue de la voix : repli de la langue d'une phrase.
    expected: VoiceLanguage,
}

/// Mesures d'un tour (`CHATONLM_VOICE_TIMINGS=1`).
struct Timing {
    turn: u32,
    /// Fin de parole détectée (arrivée de la phrase) ; même instant, en
    /// millisecondes depuis l'époque Unix.
    ended: Instant,
    ended_ms: i64,
    transcribed: Duration,
    utterance: Duration,
}

impl Timing {
    /// `chatonlm-voice-timing turn=N speech_end_ms=<époque> transcribed_ms=…
    /// utterance_ms=… first_sound_ms=…|none` : durées depuis la fin de
    /// parole détectée.
    fn line(&self, first_sound: Option<Duration>) -> String {
        let sound =
            first_sound.map_or_else(|| "none".into(), |sound| sound.as_millis().to_string());
        format!(
            "chatonlm-voice-timing turn={} speech_end_ms={} transcribed_ms={} utterance_ms={} first_sound_ms={sound}",
            self.turn,
            self.ended_ms,
            self.transcribed.as_millis(),
            self.utterance.as_millis(),
        )
    }
}

/// Suites d'un changement, faites hors du verrou.
#[derive(Default)]
struct After {
    /// Écoutes fermées, lâchées hors du verrou (fermer le micro bloque).
    closed: Vec<Box<dyn Listening>>,
    hold: Option<Hold>,
    stop_reading: bool,
    stop_any_reading: bool,
    /// Délai de pause : séance et étape.
    pause: Option<(u64, u64)>,
    transcribe: Option<Job>,
    /// Fermeture attendue (sortie de l'app).
    blocking: bool,
}

/// Transcription d'une phrase.
struct Job {
    session: u64,
    samples: Vec<f32>,
    ended: Instant,
    speech: ModelSpec,
    request: Request,
    expected: VoiceLanguage,
    abort: Arc<AtomicBool>,
}

/// Réglages d'une séance : ceux du mode vocal, mais toujours en mains libres
/// pour l'éditeur (le pont n'a pas d'appui : en push-to-talk, il n'entendrait
/// jamais rien).
fn for_owner(mut settings: VoiceModeSettings, editor: bool) -> VoiceModeSettings {
    if editor {
        settings.turn_mode = VoiceTurnMode::HandsFree;
    }
    settings
}

fn config(settings: &VoiceModeSettings) -> ListenConfig {
    ListenConfig {
        mode: match settings.turn_mode {
            VoiceTurnMode::HandsFree => TurnMode::HandsFree,
            VoiceTurnMode::PushToTalk => TurnMode::PushToTalk,
        },
        silence: settings.pause.silence(),
        barge_in: settings.barge_in,
    }
}

/// Avis d'une lecture finie.
fn notice_of(end: ReadingEnd) -> Option<VoiceModeNotice> {
    match end {
        ReadingEnd::Finished | ReadingEnd::Stopped | ReadingEnd::NothingToRead => None,
        ReadingEnd::Unsupported => Some(VoiceModeNotice::UnsupportedLanguage),
        ReadingEnd::MissingVoice(language) => Some(VoiceModeNotice::MissingVoice { language }),
        ReadingEnd::Failed(error) => Some(VoiceModeNotice::ReadFailed { error }),
    }
}

impl Inner {
    /// Passe à `phase` ; un délai de pause armé avant devient périmé.
    fn enter(&mut self, phase: VoiceModePhase) {
        self.state.phase = phase;
        self.step += 1;
        self.sync_gate();
    }

    /// Gate de l'écoute ouverte selon la phase.
    fn sync_gate(&mut self) {
        let gate = match self.state.phase {
            VoiceModePhase::Listening if self.aside => Gate::Respond,
            VoiceModePhase::Listening | Hearing => Gate::Listen,
            Transcribing => Gate::Ignore,
            Responding | Speaking => Gate::Respond,
            Off | Starting | Paused => return,
        };
        if let Some(listening) = &self.listening
            && self.gate != Some(gate)
        {
            listening.set_gate(gate);
            self.gate = Some(gate);
        }
    }

    /// Nouvelle écoute (fin d'un tour, phrase écartée) ; micro perdu (ni
    /// ouvert, ni coupé) : pause.
    fn listen_again(&mut self, after: &mut After) {
        if self.listening.is_none() && !self.state.muted {
            self.enter(Paused);
            after.hold = self.hold.take();
            if let Some(message) = self.mic_lost.clone() {
                self.state.notice = Some(VoiceModeNotice::Failed { message });
            }
        } else {
            self.enter(VoiceModePhase::Listening);
            if self.listening.is_some() {
                after.pause = Some((self.session, self.step));
            }
        }
    }

    fn close_mic(&mut self, after: &mut After) {
        after.closed.extend(self.listening.take());
        self.mic = 0;
        self.gate = None;
    }

    fn install(&mut self, mic: u64, listening: Box<dyn Listening>) {
        self.listening = Some(listening);
        self.mic = mic;
        self.mic_lost = None;
        // Gate initiale de toute écoute.
        self.gate = Some(Gate::Listen);
        self.sync_gate();
    }

    /// Ligne de mesures du tour (une fois) ; `first_sound` : premier son
    /// entendu (sinon le tour finit sans lecture).
    fn report(&mut self, first_sound: bool) {
        if let Some(timing) = self.timing.take() {
            let sound = first_sound.then(|| timing.ended.elapsed());
            eprintln!("{}", timing.line(sound));
        }
    }
}

impl Core {
    fn new(services: Services, host: Arc<dyn Host>) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            services,
            host,
            inner: Mutex::new(Inner::default()),
            me: me.clone(),
        })
    }

    fn arc(&self) -> Option<Arc<Self>> {
        self.me.upgrade()
    }

    /// Suites d'un changement, hors du verrou.
    fn after(&self, after: After) {
        if after.stop_reading {
            self.host.stop_reading();
        }
        if after.stop_any_reading {
            self.host.stop_any_reading();
        }
        for listening in after.closed {
            if after.blocking {
                drop(listening);
            } else {
                tauri::async_runtime::spawn_blocking(move || drop(listening));
            }
        }
        drop(after.hold);
        let Some(core) = self.arc() else {
            return;
        };
        if let Some((session, step)) = after.pause {
            let core = core.clone();
            let delay = self.services.pause_after;
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(delay).await;
                core.pause_if_idle(session, step);
            });
        }
        if let Some(job) = after.transcribe {
            tauri::async_runtime::spawn(async move { core.transcribe(job).await });
        }
    }

    /// Coupe la réponse du tour : lecture vocale (hors du verrou) et
    /// génération de sa conversation (attendue avant la phrase suivante).
    fn cut(&self, inner: &mut Inner, after: &mut After) {
        after.stop_reading = true;
        inner.reply_error = None;
        if let Some(chat) = inner.conversation.clone() {
            inner.stopping = Some(tauri::async_runtime::spawn(self.host.stop_reply(chat)));
        }
        inner.report(false);
    }

    /// État envoyé à la page, et à l'éditeur qui tient la séance.
    fn publish(&self, inner: &Inner) {
        self.host.changed(&inner.state);
        if let Some(Owner::Editor(link)) = &inner.owner {
            link.changed(&inner.state);
        }
    }

    /// La séance est en cours et tenue par `holder`.
    fn held_by(&self, holder: Holder) -> bool {
        self.inner
            .lock()
            .owner
            .as_ref()
            .is_some_and(|owner| owner.holder() == holder)
    }

    /// Démarrage par la page (sans effet si elle tient déjà la séance).
    async fn start(&self) -> Result<(), String> {
        self.start_as(Owner::Page)
            .await
            .map_err(|error| error.message())
    }

    async fn start_as(&self, owner: Owner) -> Result<(), StartError> {
        let session = {
            let mut inner = self.inner.lock();
            if inner.closed {
                return Ok(());
            }
            if inner.state.phase != Off {
                return match (&owner, &inner.owner) {
                    (Owner::Page, Some(Owner::Page) | None) => Ok(()),
                    (_, current) => Err(StartError::Busy {
                        page: matches!(current, Some(Owner::Page) | None),
                    }),
                };
            }
            if !self.host.reserve() {
                return Err(StartError::Failed(t!("voiceMode.dictationActive")));
            }
            inner.session += 1;
            inner.abort = Arc::new(AtomicBool::new(false));
            inner.state = VoiceModeState {
                editor: matches!(owner, Owner::Editor(_)),
                turn: owner.first_turn(),
                ..VoiceModeState::default()
            };
            inner.owner = Some(owner.clone());
            inner.enter(Starting);
            self.publish(&inner);
            inner.session
        };
        // Connexion fermée (ou `voice.stop` reçu) avant que la séance ne lui
        // appartienne : la fin de la connexion ne l'a pas vue, elle s'arrête
        // ici (après, c'est la fin de la connexion qui l'arrête).
        if let Owner::Editor(link) = &owner
            && !link.alive()
        {
            self.abandon(session);
            return Err(StartError::Failed(t!("voiceMode.editorGone")));
        }
        let opened = self.open(session).await;
        if opened.is_err() {
            self.abandon(session);
        }
        opened
    }

    /// Démarrage `session` refusé avant l'écoute (rien n'est encore ouvert) :
    /// retour au repos. L'état `Off` va à la page seulement : l'éditeur
    /// apprend le refus par la réponse d'erreur de `voice.start`, que le pont
    /// fait suivre de l'état `off` (dans cet ordre : sinon l'extension
    /// prendrait la fin pour un arrêt et perdrait la raison).
    fn abandon(&self, session: u64) {
        let mut inner = self.inner.lock();
        if inner.session != session {
            return;
        }
        inner.session += 1;
        inner.state = VoiceModeState::default();
        inner.step += 1;
        self.host.release();
        self.host.changed(&inner.state);
        inner.owner = None;
    }

    /// Démarrage de la séance `session` (Starting) jusqu'à l'écoute.
    async fn open(&self, session: u64) -> Result<(), StartError> {
        let editor = matches!(self.inner.lock().owner, Some(Owner::Editor(_)));
        let mut prepared = self.host.prepare().await?;
        prepared.settings = for_owner(prepared.settings, editor);
        let requirements = &prepared.requirements;
        if let Some(unavailable) = &requirements.unavailable {
            return Err(StartError::Failed(unavailable.clone()));
        }
        if !requirements.speech_model_installed {
            return Err(StartError::SpeechModelMissing);
        }
        if !requirements.voice_installed {
            return Err(StartError::VoiceMissing(requirements.voice_language));
        }
        let support = self.services.support;
        let kind = prepared.speech.engine;
        match tauri::async_runtime::spawn_blocking(move || support(kind)).await {
            Ok(Ok(())) => {}
            Ok(Err(refusal)) => return Err(StartError::Failed(refusal)),
            Err(error) => return Err(StartError::Failed(engine::transcription_failed(&error))),
        }
        if prepared.remember {
            self.host
                .remember_speech_model(prepared.speech.id.clone())
                .await;
        }
        // Modèles chargés pendant que le micro s'ouvre.
        let engine = &self.services.engine;
        let hold = engine.hold();
        engine.preload(prepared.speech.clone());
        let expected = requirements.voice_language;
        self.host.preload_voice(expected);
        let (mic, listening) = self.listen(config(&prepared.settings)).await?;

        let mut after = After::default();
        let mut inner = self.inner.lock();
        if inner.session != session {
            // Finie entre-temps.
            after.closed.push(listening);
            after.hold = Some(hold);
            drop(inner);
            self.after(after);
            return Ok(());
        }
        self.host.reading_active(true);
        inner.run = Some(Run {
            speech: prepared.speech,
            request: prepared.request,
            settings: prepared.settings,
            expected,
        });
        inner.hold = Some(hold);
        inner.install(mic, listening);
        inner.listen_again(&mut after);
        self.publish(&inner);
        drop(inner);
        self.after(after);
        Ok(())
    }

    /// Ouvre une écoute (bloquant, hors du verrou) ; ses événements sont
    /// relayés par une tâche, avec son numéro.
    async fn listen(&self, config: ListenConfig) -> Result<(u64, Box<dyn Listening>), String> {
        let mic = {
            let mut inner = self.inner.lock();
            inner.next_mic += 1;
            inner.next_mic
        };
        let events = self.events(mic);
        let ear = self.services.ear.clone();
        match tauri::async_runtime::spawn_blocking(move || ear.open(config, events)).await {
            Ok(opened) => opened.map(|listening| (mic, listening)),
            Err(error) => Err(crate::dictation::audio::could_not_open(&error)),
        }
    }

    /// Rappel de l'écoute `mic` : ne bloque pas (file sans borne vidée par
    /// une tâche).
    fn events(&self, mic: u64) -> Events {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let core = self.me.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = receiver.recv().await {
                let Some(core) = core.upgrade() else {
                    return;
                };
                core.on_listen(mic, event);
            }
        });
        Box::new(move |event| {
            let _ = sender.send(event);
        })
    }

    fn on_listen(&self, mic: u64, event: ListenEvent) {
        // Calculé avant le verrou (une phrase dure jusqu'à 2 min).
        let speech = match &event {
            ListenEvent::Utterance(samples) => crate::dictation::audio::has_speech(samples),
            _ => false,
        };
        let mut after = After::default();
        let mut inner = self.inner.lock();
        if inner.mic != mic || inner.listening.is_none() {
            return;
        }
        let phase = inner.state.phase;
        match event {
            ListenEvent::Levels { input, output } => {
                let level = VoiceModeLevel { input, output };
                match &inner.owner {
                    Some(Owner::Editor(link)) => link.level(level),
                    _ => self.host.level(level),
                }
                return;
            }
            ListenEvent::SpeechStarted if phase == VoiceModePhase::Listening => {
                inner.enter(Hearing)
            }
            ListenEvent::BargeIn if matches!(phase, Responding | Speaking) => {
                // L'écoute est passée d'elle-même en Listen.
                inner.gate = Some(Gate::Listen);
                self.cut(&mut inner, &mut after);
                inner.enter(Hearing);
            }
            // Parole franche sur une lecture hors tour : elle s'arrête, la
            // parole devient le tour suivant.
            ListenEvent::BargeIn if phase == VoiceModePhase::Listening && inner.aside => {
                inner.gate = Some(Gate::Listen);
                after.stop_any_reading = true;
                inner.enter(Hearing);
            }
            ListenEvent::Utterance(samples)
                if matches!(phase, VoiceModePhase::Listening | Hearing) =>
            {
                let Some(run) = inner.run.as_ref() else {
                    return;
                };
                if speech {
                    let job = Job {
                        session: inner.session,
                        samples,
                        ended: Instant::now(),
                        speech: run.speech.clone(),
                        request: run.request.clone(),
                        expected: run.expected,
                        abort: inner.abort.clone(),
                    };
                    after.transcribe = Some(job);
                    inner.enter(Transcribing);
                } else {
                    inner.listen_again(&mut after);
                }
            }
            ListenEvent::Discarded if phase == Hearing => inner.listen_again(&mut after),
            ListenEvent::Failed(message) => {
                inner.close_mic(&mut after);
                inner.mic_lost = Some(message.clone());
                inner.state.notice = Some(VoiceModeNotice::Failed { message });
                if matches!(phase, VoiceModePhase::Listening | Hearing) {
                    inner.listen_again(&mut after);
                }
            }
            _ => return,
        }
        self.publish(&inner);
        drop(inner);
        self.after(after);
    }

    async fn transcribe(&self, job: Job) {
        let Job {
            session,
            samples,
            ended,
            speech,
            request,
            expected,
            abort,
        } = job;
        let engine = self.services.engine.clone();
        let transcribed = tauri::async_runtime::spawn_blocking(move || {
            engine.transcribe(&speech, &samples, &request, &abort)
        })
        .await;
        let transcribed_after = ended.elapsed();
        let text = match transcribed {
            Ok(Ok(text)) => Ok(text.trim().to_owned()),
            Ok(Err(error)) if error == engine::CANCELLED || error == engine::CLOSING => Err(None),
            Ok(Err(error)) => Err(Some(error)),
            Err(error) => Err(Some(engine::transcription_failed(&error))),
        };
        if matches!(&text, Ok(text) if !text.is_empty()) {
            // Réponse interrompue : finie avant l'envoi de la phrase suivante
            // (sinon « conversation occupée »).
            let stopping = self.inner.lock().stopping.take();
            if let Some(stopping) = stopping {
                let _ = stopping.await;
            }
        }
        let mut after = After::default();
        let mut inner = self.inner.lock();
        if inner.session != session || inner.state.phase != Transcribing {
            return;
        }
        match text {
            Ok(text) if !text.is_empty() => {
                inner.state.turn += 1;
                inner.state.notice = None;
                inner.conversation = None;
                let utterance = VoiceModeUtterance {
                    turn: inner.state.turn,
                    language: crate::tts::text::choose_language(&text, expected),
                    text,
                };
                inner.enter(Responding);
                if self.services.timings {
                    inner.timing = Some(Timing {
                        turn: utterance.turn,
                        ended,
                        ended_ms: crate::store::now_ms() - ended.elapsed().as_millis() as i64,
                        transcribed: transcribed_after,
                        utterance: ended.elapsed(),
                    });
                }
                match &inner.owner {
                    Some(Owner::Editor(link)) => link.utterance(&utterance),
                    _ => self.host.utterance(&utterance),
                }
            }
            Ok(_) | Err(None) => inner.listen_again(&mut after),
            Err(Some(message)) => {
                inner.state.notice = Some(VoiceModeNotice::Failed { message });
                inner.listen_again(&mut after);
            }
        }
        self.publish(&inner);
        drop(inner);
        self.after(after);
    }

    /// Fin de toute séance (sortie de l'app, conversation supprimée).
    fn stop(&self, blocking: bool) {
        self.stop_where(blocking, None);
    }

    /// Fin de la séance si elle est tenue par `holder` (toute séance avec
    /// `None`) ; l'éditeur qui la tenait apprend l'état `Off`.
    fn stop_where(&self, blocking: bool, holder: Option<Holder>) {
        let mut after = After {
            blocking,
            ..After::default()
        };
        let mut inner = self.inner.lock();
        if inner.state.phase == Off {
            return;
        }
        if let Some(holder) = holder
            && inner.owner.as_ref().map(Owner::holder) != Some(holder)
        {
            return;
        }
        inner.session += 1;
        inner.abort.store(true, Ordering::SeqCst);
        inner.close_mic(&mut after);
        after.hold = inner.hold.take();
        after.stop_reading = true;
        inner.run = None;
        inner.conversation = None;
        inner.session_chat = None;
        inner.stopping = None;
        inner.reply_error = None;
        inner.mic_lost = None;
        inner.report(false);
        inner.state = VoiceModeState::default();
        inner.step += 1;
        self.host.reading_active(false);
        self.host.release();
        self.publish(&inner);
        inner.owner = None;
        drop(inner);
        self.after(after);
    }

    async fn mute(&self, muted: bool) -> Result<(), String> {
        if muted {
            let mut after = After::default();
            let mut inner = self.inner.lock();
            if matches!(inner.state.phase, Off | Starting) || inner.state.muted {
                return Ok(());
            }
            inner.state.muted = true;
            inner.close_mic(&mut after);
            if inner.state.phase == Hearing {
                // La parole en cours est perdue.
                inner.enter(VoiceModePhase::Listening);
            } else {
                inner.step += 1;
            }
            self.publish(&inner);
            drop(inner);
            self.after(after);
            return Ok(());
        }
        let (session, settings) = {
            let mut inner = self.inner.lock();
            let phase = inner.state.phase;
            if matches!(phase, Off | Starting) || !inner.state.muted {
                return Ok(());
            }
            if phase == Paused {
                // Rouvert par la reprise.
                inner.state.muted = false;
                self.publish(&inner);
                return Ok(());
            }
            let Some(run) = inner.run.as_ref() else {
                return Ok(());
            };
            (inner.session, run.settings.clone())
        };
        let (mic, listening) = match self.listen(config(&settings)).await {
            Ok(opened) => opened,
            Err(message) => {
                // Comme la reprise : avis, le micro reste coupé.
                let mut inner = self.inner.lock();
                if inner.session == session && inner.state.muted {
                    inner.state.notice = Some(VoiceModeNotice::Failed {
                        message: message.clone(),
                    });
                    self.publish(&inner);
                }
                return Err(message);
            }
        };
        let mut after = After::default();
        let mut inner = self.inner.lock();
        if inner.session != session || !inner.state.muted || inner.state.phase == Paused {
            after.closed.push(listening);
        } else {
            inner.state.muted = false;
            inner.install(mic, listening);
            inner.step += 1;
            if inner.state.phase == VoiceModePhase::Listening {
                after.pause = Some((inner.session, inner.step));
            }
            self.publish(&inner);
        }
        drop(inner);
        self.after(after);
        Ok(())
    }

    fn interrupt(&self) {
        let mut after = After::default();
        let mut inner = self.inner.lock();
        if !matches!(inner.state.phase, Responding | Speaking) {
            return;
        }
        self.cut(&mut inner, &mut after);
        inner.listen_again(&mut after);
        self.publish(&inner);
        drop(inner);
        self.after(after);
    }

    fn end_turn(&self) {
        let inner = self.inner.lock();
        if inner.state.phase == Hearing
            && let Some(listening) = &inner.listening
        {
            listening.end_turn();
        }
    }

    fn hold(&self, pressed: bool) {
        let mut after = After::default();
        let mut inner = self.inner.lock();
        let push_to_talk = inner
            .run
            .as_ref()
            .is_some_and(|run| run.settings.turn_mode == VoiceTurnMode::PushToTalk);
        if !push_to_talk || inner.listening.is_none() {
            return;
        }
        if !pressed {
            if let Some(listening) = &inner.listening {
                listening.hold(false);
            }
            return;
        }
        match inner.state.phase {
            Responding | Speaking => self.cut(&mut inner, &mut after),
            VoiceModePhase::Listening => {}
            _ => return,
        }
        inner.enter(Hearing);
        if let Some(listening) = &inner.listening {
            listening.hold(true);
        }
        self.publish(&inner);
        drop(inner);
        self.after(after);
    }

    async fn resume(&self) -> Result<(), String> {
        let (session, settings, speech) = {
            let inner = self.inner.lock();
            let Some(run) = inner.run.as_ref().filter(|_| inner.state.phase == Paused) else {
                return Ok(());
            };
            (inner.session, run.settings.clone(), run.speech.clone())
        };
        let opened = self.listen(config(&settings)).await;
        let mut after = After::default();
        let mut inner = self.inner.lock();
        if inner.session != session || inner.state.phase != Paused {
            after
                .closed
                .extend(opened.ok().map(|(_, listening)| listening));
            drop(inner);
            self.after(after);
            return Ok(());
        }
        let (mic, listening) = match opened {
            Ok(opened) => opened,
            Err(message) => {
                inner.state.notice = Some(VoiceModeNotice::Failed {
                    message: message.clone(),
                });
                self.publish(&inner);
                return Err(message);
            }
        };
        let engine = &self.services.engine;
        inner.hold = Some(engine.hold());
        engine.preload(speech);
        inner.state.muted = false;
        inner.state.notice = None;
        inner.install(mic, listening);
        inner.listen_again(&mut after);
        self.publish(&inner);
        drop(inner);
        self.after(after);
        Ok(())
    }

    /// Délai de pause échu : pause si l'écoute attend toujours.
    fn pause_if_idle(&self, session: u64, step: u64) {
        let mut after = After::default();
        let mut inner = self.inner.lock();
        if inner.session != session
            || inner.step != step
            || inner.state.phase != VoiceModePhase::Listening
            || inner.state.muted
        {
            return;
        }
        inner.close_mic(&mut after);
        after.hold = inner.hold.take();
        inner.enter(Paused);
        self.publish(&inner);
        drop(inner);
        self.after(after);
    }

    fn reply_failed(&self, turn: u32, message: String) {
        let mut after = After::default();
        let mut inner = self.inner.lock();
        if inner.state.phase != Responding || inner.state.turn != turn {
            return;
        }
        inner.report(false);
        inner.state.notice = Some(VoiceModeNotice::Failed { message });
        inner.listen_again(&mut after);
        self.publish(&inner);
        drop(inner);
        self.after(after);
    }

    fn settings_changed(&self, settings: &VoiceModeSettings) {
        let mut inner = self.inner.lock();
        let editor = matches!(inner.owner, Some(Owner::Editor(_)));
        let Some(run) = inner.run.as_mut() else {
            return;
        };
        run.settings = for_owner(settings.clone(), editor);
        let config = config(&run.settings);
        if let Some(listening) = &inner.listening {
            listening.configure(config);
        }
    }

    fn observe(&self, chat: &str, event: &ChatEvent, voice: Option<&VoiceSend>) {
        if voice.is_none() {
            return;
        }
        let mut after = After::default();
        let mut inner = self.inner.lock();
        if !matches!(event, ChatEvent::UserMessage { .. }) {
            // Envoi du tour fini avant d'écrire sa phrase (base en échec…) :
            // rien ne sera lu, le tour finit ici.
            let failed = match event {
                ChatEvent::Error { message, .. } => Some(message.clone()),
                _ => None,
            };
            let ended = matches!(event, ChatEvent::Error { .. } | ChatEvent::Cancelled);
            if !ended || inner.state.phase != Responding || inner.conversation.is_some() {
                return;
            }
            inner.report(false);
            inner.state.notice = failed.map(|message| VoiceModeNotice::Failed { message });
            inner.listen_again(&mut after);
            self.publish(&inner);
            drop(inner);
            self.after(after);
            return;
        }
        match inner.state.phase {
            Responding if inner.conversation.is_none() => {
                inner.conversation = Some(chat.to_owned());
                inner.session_chat = Some(chat.to_owned());
                return;
            }
            // Envoi d'un tour interrompu avant d'arriver : arrêté aussi.
            VoiceModePhase::Listening | Hearing | Transcribing | Paused => {
                after.stop_reading = true;
                inner.stopping = Some(tauri::async_runtime::spawn(
                    self.host.stop_reply(chat.to_owned()),
                ));
            }
            _ => return,
        }
        drop(inner);
        self.after(after);
    }

    fn on_reading(&self, chat: &str, reading: VoiceReading) {
        let mut after = After::default();
        let mut inner = self.inner.lock();
        if let VoiceReading::Aside(playing) = reading {
            // Lecture hors tour : gate de l'écoute accordée, phase gardée.
            inner.aside = playing;
            inner.sync_gate();
            return;
        }
        if inner.conversation.as_deref() != Some(chat) {
            return;
        }
        match (inner.state.phase, reading) {
            (Responding, VoiceReading::Playing) => {
                inner.report(true);
                inner.enter(Speaking);
            }
            (Responding | Speaking, VoiceReading::ReplyFailed(message)) => {
                inner.reply_error = Some(message);
                return;
            }
            (Responding | Speaking, VoiceReading::Ended(end)) => {
                inner.report(false);
                // L'échec de la réponse d'abord (ce qui a été écrit a été lu).
                inner.state.notice = match inner.reply_error.take() {
                    Some(message) => Some(VoiceModeNotice::Failed { message }),
                    None => notice_of(end),
                };
                inner.listen_again(&mut after);
            }
            _ => return,
        }
        self.publish(&inner);
        drop(inner);
        // Jamais la lecture ici (appelé sous son verrou de livraison).
        debug_assert!(!after.stop_reading);
        self.after(after);
    }

    fn chat_deleted(&self, chat: &str) {
        let ours = {
            let inner = self.inner.lock();
            inner.session_chat.as_deref() == Some(chat)
                || inner.conversation.as_deref() == Some(chat)
        };
        if ours {
            self.stop(false);
        }
    }
}

/// Modèle de transcription employé : celui de la dictée, sinon le premier
/// installé du catalogue, sinon le recommandé.
pub(crate) fn speech_model(
    dictation: &DictationSettings,
    installed: &dyn Fn(&CatalogModel) -> bool,
) -> &'static CatalogModel {
    speech_models::find(&dictation.speech_model)
        .or_else(|| speech_models::CATALOG.iter().find(|model| installed(model)))
        .or_else(|| {
            speech_models::CATALOG
                .iter()
                .find(|model| model.recommended)
        })
        .unwrap_or(&speech_models::CATALOG[0])
}

/// Langue attendue de la voix : celle de la dictée si une voix la lit,
/// sinon celle de l'interface.
pub(crate) fn expected_language(
    dictation: &DictationSettings,
    interface: VoiceLanguage,
) -> VoiceLanguage {
    match dictation.language.as_str() {
        "en" => VoiceLanguage::En,
        "es" => VoiceLanguage::Es,
        "fr" => VoiceLanguage::Fr,
        _ => interface,
    }
}

/// Prérequis et réglages d'une séance, d'après les modèles présents dans
/// `speech_dir` et `voice_dir` ; `unavailable` : refus de la machine pour ce
/// moteur de transcription.
pub(crate) fn gather(
    dictation: &DictationSettings,
    settings: VoiceModeSettings,
    speech_dir: &Path,
    voice_dir: &Path,
    interface: VoiceLanguage,
    unavailable: impl FnOnce(SpeechEngine) -> Option<String>,
) -> Prepared {
    let model = speech_model(dictation, &|model| {
        speech_models::installed(speech_dir, model)
    });
    let speech_installed = speech_models::installed(speech_dir, model);
    let language = expected_language(dictation, interface);
    let voice = crate::tts::models::for_language(language);
    let voice_installed =
        voice.is_some_and(|voice| crate::tts::models::installed(voice_dir, voice));
    let unavailable = unavailable(model.engine);
    Prepared {
        requirements: VoiceModeRequirements {
            speech_model: model.id.into(),
            speech_model_installed: speech_installed,
            speech_model_bytes: if speech_installed { 0 } else { model.size() },
            voice_model: voice.map(|voice| voice.id).unwrap_or_default().into(),
            voice_language: language,
            voice_installed,
            voice_bytes: match voice {
                Some(voice) if !voice_installed => voice.size(),
                _ => 0,
            },
            unavailable,
        },
        speech: ModelSpec::new(model, speech_dir),
        remember: dictation.speech_model.is_empty() && speech_installed,
        request: Request {
            language: (dictation.language != languages::AUTO).then(|| dictation.language.clone()),
            glossary: dictation.dictionary.clone(),
        },
        settings,
    }
}

/// Ce que la machine refuse (bloquant : charge ONNX Runtime la première
/// fois) : ONNX Runtime (détection de parole et voix), puis le moteur de
/// transcription (Whisper sans AVX2…).
fn machine_refusal(engine: SpeechEngine) -> Option<String> {
    match crate::dictation::onnx::ensure() {
        Err(problem) => Some(t!("voiceMode.runtimeUnavailable", detail = problem.detail)),
        Ok(()) => crate::dictation::engine_support(engine).err(),
    }
}

/// Langue de l'interface, vue comme une langue de voix.
fn interface_language() -> VoiceLanguage {
    match crate::i18n::current() {
        crate::i18n::Lang::En => VoiceLanguage::En,
        crate::i18n::Lang::Es => VoiceLanguage::Es,
        crate::i18n::Lang::Fr => VoiceLanguage::Fr,
    }
}

/// Prérequis lus dans l'app (réglages, dossiers des modèles).
pub(crate) async fn prepare(app: &AppHandle) -> Result<Prepared, String> {
    let failed = |error: crate::error::AppError| error.to_string();
    let store = app
        .try_state::<Store>()
        .map(|store| store.inner().clone())
        .ok_or_else(|| t!("dictation.noDatabase"))?;
    let (dictation, settings) = store
        .call(|connection| {
            Ok((
                crate::store::dictation::get(connection)?,
                crate::store::voice_mode::get(connection)?,
            ))
        })
        .await
        .map_err(failed)?;
    let speech_dir = crate::paths::speech_models_dir(app).map_err(failed)?;
    let voice_dir = crate::paths::voice_models_dir(app).map_err(failed)?;
    let interface = interface_language();
    tauri::async_runtime::spawn_blocking(move || {
        gather(
            &dictation,
            settings,
            &speech_dir,
            &voice_dir,
            interface,
            machine_refusal,
        )
    })
    .await
    .map_err(|error| error.to_string())
}

struct AppHost(AppHandle);

impl Host for AppHost {
    fn prepare(&self) -> BoxFuture<Result<Prepared, String>> {
        let app = self.0.clone();
        Box::pin(async move { prepare(&app).await })
    }

    fn remember_speech_model(&self, id: String) -> BoxFuture<()> {
        let app = self.0.clone();
        Box::pin(async move {
            let Some(store) = app.try_state::<Store>().map(|store| store.inner().clone()) else {
                return;
            };
            let patch = DictationPatch {
                speech_model: Some(id),
                ..DictationPatch::default()
            };
            match store
                .call(move |connection| crate::store::dictation::update(connection, patch))
                .await
            {
                Ok(settings) => {
                    let dictation = app.state::<Dictation>();
                    dictation.settings_changed(&settings);
                    dictation.emit_status(&app);
                }
                Err(error) => eprintln!("chatonlm: speech model not saved: {error}"),
            }
        })
    }

    fn reserve(&self) -> bool {
        self.0.state::<Dictation>().reserve_for_voice_mode()
    }

    fn release(&self) {
        self.0.state::<Dictation>().release_voice_mode();
    }

    fn changed(&self, state: &VoiceModeState) {
        let _ = self.0.emit(STATE_EVENT, state);
    }

    fn level(&self, level: VoiceModeLevel) {
        let _ = self.0.emit(LEVEL_EVENT, level);
    }

    fn utterance(&self, utterance: &VoiceModeUtterance) {
        let _ = self.0.emit(UTTERANCE_EVENT, utterance);
    }

    fn reading_active(&self, active: bool) {
        self.0.state::<ReadAloud>().set_voice_active(active);
    }

    fn stop_reading(&self) {
        self.0.state::<ReadAloud>().stop_voice();
    }

    fn stop_any_reading(&self) {
        self.0.state::<ReadAloud>().stop();
    }

    fn preload_voice(&self, language: VoiceLanguage) {
        self.0.state::<ReadAloud>().preload_language(language);
    }

    fn stop_reply(&self, chat: String) -> BoxFuture<()> {
        let app = self.0.clone();
        Box::pin(async move { app.state::<ChatEngine>().stop_chat(&chat).await })
    }
}

#[cfg(test)]
mod tests;
