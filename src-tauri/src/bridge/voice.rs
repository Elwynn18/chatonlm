//! Voix pilotée par l'extension VS Code (lot 2, propre à ChatonLM) ; l'audio
//! reste dans l'app (micro interdit dans une webview).
//!
//! - `speech.read` : segments identifiés lus à voix haute ; chaque segment
//!   annonce son début et sa fin quand son son est réellement joué
//!   (`speech.event`), puis l'arrêt ou l'échec ; une seule lecture dans
//!   l'app (une nouvelle l'arrête) ; `speech.stop`.
//! - `voice.start` : séance du mode vocal tenue par la connexion (profil
//!   `duck` ou `tour`, toujours en mains libres) ; ses phrases partent au chat
//!   par le moteur (envoi vocal, conversation marquée « VS Code »), la réponse
//!   est lue par le moteur ; états, niveaux et évènements des tours en
//!   notifications (tours numérotés sur toute la connexion) ; contexte de
//!   l'éditeur remplacé à tout moment (`voice.context`), joint à chaque tour
//!   en message système, jamais enregistré ; `voice.resume` après une pause.
//! - La séance est inscrite dès la lecture de `voice.start`, avant toute
//!   attente : un `voice.stop` lu ensuite l'arrête toujours, même avant que
//!   le micro ne s'ouvre (le démarrage est alors refusé, -32800). Un second
//!   `voice.start` sur la même connexion est refusé (-32032) sans toucher à la
//!   séance en cours.
//! - Fin de la séance (quelle qu'en soit la cause) : les réponses de ses tours
//!   encore en cours sont arrêtées (le panneau les montre annulées).
//! - Le micro ouvert par le pont s'arrête toujours : fin de la connexion (même
//!   par une panique de sa tâche), éditeur retiré, pont coupé, sortie de
//!   l'app, bouton Stop du bandeau de la page (voir [`closed`]).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;

use super::connection::{Outbox, code};
use super::{Host, STREAM_PREFIX, Shared};
use crate::chat::stream::Think;
use crate::chat::{self, ChatEvent, Origin, SendRequest, Server, VoiceSend};
use crate::dictation::clipboard::BoxFuture;
use crate::i18n::t;
use crate::store::{chats, model_prefs};
use crate::tts::service::{ReadingEnd, Segment, SegmentEvent, SegmentSink, SegmentsRefusal};
use crate::tts::{ReadAloud, ReadAloudError, VoiceLanguage};
use crate::voice_mode::session::{EditorLink, StartError};
use crate::voice_mode::{
    VoiceMode, VoiceModeLevel, VoiceModePhase, VoiceModeState, VoiceModeUtterance,
};

/// Codes d'erreur de la voix.
pub mod voice_code {
    /// Voix de lecture absente (`data: { language }`).
    pub const MISSING_VOICE: i64 = -32030;
    /// Modèle de transcription absent.
    pub const MISSING_SPEECH_MODEL: i64 = -32031;
    /// Séance déjà tenue (`data: { owner: "page" | "bridge" }`).
    pub const BUSY: i64 = -32032;
    /// Micro indisponible, dictée en cours… (`data: { message }`, traduit).
    pub const MICROPHONE: i64 = -32033;
}

/// Segments d'une lecture, au plus.
pub const MAX_SEGMENTS: usize = 64;
/// Caractères d'un segment, au plus.
pub const MAX_SEGMENT_TEXT: usize = 4_000;
/// Identifiant d'un segment, au plus.
pub const MAX_SEGMENT_ID: usize = 64;
/// Contexte de l'éditeur gardé, au plus (caractères).
pub const MAX_CONTEXT: usize = 16_000;
/// Intervalle minimal entre deux `voice.level`.
const LEVEL_INTERVAL: Duration = Duration::from_millis(66);

/// La voix de l'app, vue par le pont (remplacée dans les essais).
pub trait Voice: Send + Sync + 'static {
    fn read(
        &self,
        read_id: String,
        segments: Vec<Segment>,
        language: Option<VoiceLanguage>,
        sink: SegmentSink,
    ) -> Result<(), SegmentsRefusal>;
    /// Arrête la lecture `read_id` si elle est en cours.
    fn stop_read(&self, read_id: &str);
    /// Arrête la lecture en cours si elle vient de la connexion.
    fn stop_reads(&self, connection: u64);
    fn start(&self, link: Arc<dyn EditorLink>) -> BoxFuture<Result<(), StartError>>;
    /// Fin de la séance si la connexion la tient.
    fn stop(&self, connection: u64);
    fn mute(&self, connection: u64, muted: bool) -> BoxFuture<Result<(), String>>;
    /// Reprise après une pause, si la connexion tient la séance.
    fn resume(&self, connection: u64) -> BoxFuture<Result<(), String>>;
    fn reply_failed(&self, connection: u64, turn: u32, message: String);
}

/// Identifiant d'une lecture : la connexion d'abord (ses lectures
/// s'arrêtent avec elle).
fn read_id(connection: u64, number: u64) -> String {
    format!("{connection}-{number}")
}

fn of_connection(read_id: &str, connection: u64) -> bool {
    read_id
        .split_once('-')
        .is_some_and(|(owner, _)| owner == connection.to_string())
}

/// La voix de l'app réelle.
pub struct AppVoice(pub AppHandle);

impl Voice for AppVoice {
    fn read(
        &self,
        read_id: String,
        segments: Vec<Segment>,
        language: Option<VoiceLanguage>,
        sink: SegmentSink,
    ) -> Result<(), SegmentsRefusal> {
        self.0
            .state::<ReadAloud>()
            .read_segments(read_id, segments, language, sink)
    }

    fn stop_read(&self, read_id: &str) {
        self.0
            .state::<ReadAloud>()
            .stop_segments(|current| current == read_id);
    }

    fn stop_reads(&self, connection: u64) {
        if let Some(read_aloud) = self.0.try_state::<ReadAloud>() {
            read_aloud.stop_segments(|current| of_connection(current, connection));
        }
    }

    fn start(&self, link: Arc<dyn EditorLink>) -> BoxFuture<Result<(), StartError>> {
        let app = self.0.clone();
        Box::pin(async move {
            let Some(voice_mode) = app.try_state::<VoiceMode>() else {
                return Err(StartError::Failed(t!("voiceMode.editorGone")));
            };
            voice_mode.start_for_editor(link).await
        })
    }

    fn stop(&self, connection: u64) {
        if let Some(voice_mode) = self.0.try_state::<VoiceMode>() {
            voice_mode.stop_editor(connection);
        }
    }

    fn mute(&self, connection: u64, muted: bool) -> BoxFuture<Result<(), String>> {
        let app = self.0.clone();
        Box::pin(async move {
            match app.try_state::<VoiceMode>() {
                Some(voice_mode) => voice_mode.mute_editor(connection, muted).await,
                None => Ok(()),
            }
        })
    }

    fn resume(&self, connection: u64) -> BoxFuture<Result<(), String>> {
        let app = self.0.clone();
        Box::pin(async move {
            match app.try_state::<VoiceMode>() {
                Some(voice_mode) => voice_mode.resume_editor(connection).await,
                None => Ok(()),
            }
        })
    }

    fn reply_failed(&self, connection: u64, turn: u32, message: String) {
        if let Some(voice_mode) = self.0.try_state::<VoiceMode>() {
            voice_mode.reply_failed_editor(connection, turn, message);
        }
    }
}

/// Consigne de la séance.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// « Chaton en caoutchouc » : consigne socratique.
    Duck,
    /// Question sur le passage d'une visite : réponse brève.
    Tour,
}

impl Profile {
    fn instructions(self) -> String {
        match self {
            Profile::Duck => t!("bridge.voice.duck"),
            Profile::Tour => t!("bridge.voice.tour"),
        }
    }
}

/// Texte borné à `max` caractères (début gardé).
fn bounded(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Séance tenue par une connexion : reçoit ce que la séance lui donne, et
/// envoie ses phrases au chat.
pub(crate) struct Talk<H: Host> {
    shared: Weak<Shared<H>>,
    me: Weak<Talk<H>>,
    connection: u64,
    /// Éditeur authentifié de la connexion.
    client: String,
    out: Outbox,
    profile: Profile,
    /// La conversation ouverte par le premier tour est éphémère (sans
    /// conversation donnée).
    ephemeral: bool,
    model: Option<String>,
    /// Conversation de la séance (choisie, ou créée par le premier tour).
    chat: Mutex<Option<String>>,
    context: Mutex<Option<String>>,
    /// Dernier niveau envoyé.
    level_sent: Mutex<Option<Instant>>,
    /// Dernier tour de la connexion (toutes ses séances) : partagé avec elle.
    turns: Arc<AtomicU32>,
    /// Un état autre que `off` a été envoyé à l'éditeur.
    told: AtomicBool,
    /// La séance a pris fin (`off` envoyé) : ses tours en cours s'arrêtent.
    ended: AtomicBool,
    /// Flux du moteur des tours en cours.
    streams: Mutex<HashSet<String>>,
}

impl<H: Host> Talk<H> {
    /// Consigne du profil, puis contexte de l'éditeur.
    fn system(&self) -> String {
        let mut system = self.profile.instructions();
        if let Some(context) = self.context.lock().clone() {
            let context = t!("bridge.voice.context", text = context);
            system.push_str("\n\n");
            system.push_str(&context);
        }
        system
    }

    pub(crate) fn set_context(&self, text: String) {
        let text = bounded(&text, MAX_CONTEXT);
        *self.context.lock() = (!text.trim().is_empty()).then_some(text);
    }
}

impl<H: Host> EditorLink for Talk<H> {
    fn connection(&self) -> u64 {
        self.connection
    }

    /// Connexion ouverte, et séance toujours inscrite (un `voice.stop` lu
    /// pendant le démarrage l'a retirée).
    fn alive(&self) -> bool {
        self.shared.upgrade().is_some_and(|shared| {
            shared.is_open(self.connection)
                && shared.holds_talk(self.connection, self as *const _ as *const ())
        })
    }

    fn first_turn(&self) -> u32 {
        self.turns.load(Ordering::SeqCst)
    }

    /// Appelé sous le verrou de la séance : `cancel` du moteur ne fait que
    /// signaler l'arrêt (rien n'y rappelle la séance).
    fn changed(&self, state: &VoiceModeState) {
        let off = state.phase == VoiceModePhase::Off;
        if off {
            self.ended.store(true, Ordering::SeqCst);
        } else {
            self.told.store(true, Ordering::SeqCst);
        }
        self.out.notify("voice.state", json!({ "state": state }));
        if off && let Some(shared) = self.shared.upgrade() {
            // Réponses des tours en cours arrêtées (avant leur début, l'arrêt
            // est rejoué par `send_turn`).
            for stream in self.streams.lock().iter() {
                shared.host.engine().cancel(stream);
            }
            shared.forget_talk(self.connection, self as *const _ as *const ());
        }
    }

    fn level(&self, level: VoiceModeLevel) {
        let mut sent = self.level_sent.lock();
        if sent.is_some_and(|sent| sent.elapsed() < LEVEL_INTERVAL) {
            return;
        }
        *sent = Some(Instant::now());
        self.out.notify("voice.level", json!({ "level": level }));
    }

    fn utterance(&self, utterance: &VoiceModeUtterance) {
        self.turns.fetch_max(utterance.turn, Ordering::SeqCst);
        let (Some(shared), Some(talk)) = (self.shared.upgrade(), self.me.upgrade()) else {
            return;
        };
        let utterance = utterance.clone();
        tauri::async_runtime::spawn(async move { send_turn(&shared, &talk, utterance).await });
    }
}

/// Réflexion d'un envoi vocal, comme la page : coupée si le modèle le
/// permet, sauf réglage « garder la réflexion » (préférence du modèle).
async fn voice_think<H: Host>(shared: &Shared<H>, server: &Server, model: &str) -> Option<Think> {
    let keep = shared
        .store
        .call(|c| crate::store::voice_mode::get(c))
        .await
        .is_ok_and(|settings| settings.keep_think);
    if keep {
        let model = model.to_owned();
        return shared
            .store
            .call(move |c| model_prefs::think(c, &model))
            .await
            .ok()
            .flatten();
    }
    let details = crate::ollama::models::show(&server.url, model).await.ok();
    crate::dictation::polish::think_off(details.as_ref(), false).then_some(Think::Enabled(false))
}

/// Envoie la phrase d'un tour au chat (envoi vocal, observé : la séance suit
/// le tour, la réponse est lue) et relaie ses évènements (`voice.chat`). Un
/// envoi refusé d'emblée finit le tour avec un avis.
async fn send_turn<H: Host>(
    shared: &Arc<Shared<H>>,
    talk: &Arc<Talk<H>>,
    utterance: VoiceModeUtterance,
) {
    let turn = utterance.turn;
    let connection = talk.connection;
    let failed = |message: String| shared.host.voice().reply_failed(connection, turn, message);
    let settings = match crate::ollama::target::request_settings(&shared.store).await {
        Ok(settings) => settings,
        Err(error) => return failed(error.to_string()),
    };
    let server = Server::from_settings(&settings);
    let model = talk
        .model
        .clone()
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| settings.selected_model.clone());
    // Modèle du cloud, réglage coupé : rien ne part, même pas `/api/show`
    // (comme `tour.plan`) ; le refus de `send_to` viendrait trop tard.
    if let Some(message) = server.refuses(&model, crate::ollama::models::is_cloud_name(&model)) {
        return failed(message);
    }
    let number = shared.next_stream.fetch_add(1, Ordering::SeqCst);
    let stream = format!("{STREAM_PREFIX}{connection}:voice:{number}");
    // Inscrit avant toute attente : une fin de séance l'arrête.
    talk.streams.lock().insert(stream.clone());
    let _registered = TurnStream {
        talk,
        stream: &stream,
    };
    let think = voice_think(shared, &server, &model).await;
    // Séance finie pendant la préparation : le tour ne part pas.
    if talk.ended.load(Ordering::SeqCst) {
        return;
    }
    let system = talk.system();
    let chat_id = talk.chat.lock().clone();
    // Éphémère seulement pour la conversation que le premier tour ouvre.
    let ephemeral = talk.ephemeral && chat_id.is_none();
    let request = SendRequest {
        chat_id,
        stream_id: stream.clone(),
        content: utterance.text,
        model,
        think,
        attachment_ids: None,
        edit_message_id: None,
        kept_attachment_ids: None,
        web_search: None,
        voice: Some(VoiceSend {
            language: utterance.language,
            system: Some(system),
        }),
        image: None,
        ephemeral: ephemeral.then_some(true),
    };
    let (events, mut received) = mpsc::unbounded_channel::<ChatEvent>();
    let sending = async {
        let sink = move |event: ChatEvent| {
            let _ = events.send(event);
        };
        chat::send_to(
            &shared.store,
            shared.host.engine(),
            &shared.staging,
            server,
            request,
            Origin::Bridge,
            &sink,
        )
        .await
    };
    let relaying = async {
        // Éphémère dont l'éditeur est déjà parti : effacée à la fin du tour.
        let mut erase = None;
        while let Some(event) = received.recv().await {
            match &event {
                ChatEvent::ChatCreated { chat_id } => {
                    let marked = chat_id.clone();
                    if let Err(error) = shared
                        .store
                        .call(move |c| chats::set_editor(c, &marked))
                        .await
                    {
                        eprintln!("chatonlm: cannot mark an editor chat: {error}");
                    }
                    // Suivie comme les autres éphémères du pont : effacée au
                    // retrait de l'éditeur, à la coupure du pont, par
                    // `chat.forget`.
                    if ephemeral && !shared.track_ephemeral(connection, &talk.client, chat_id) {
                        erase = Some(chat_id.clone());
                    }
                    *talk.chat.lock() = Some(chat_id.clone());
                    shared.host.chat_updated(chat_id);
                }
                ChatEvent::UserMessage { .. } | ChatEvent::Done { .. } => {
                    if let Some(chat) = talk.chat.lock().clone() {
                        shared.host.chat_updated(&chat);
                    }
                }
                _ => {}
            }
            // Tout évènement autre que `chatCreated` suit `begin` : une fin de
            // séance survenue avant (sans effet sur le moteur alors) est
            // rejouée.
            if !matches!(event, ChatEvent::ChatCreated { .. }) && talk.ended.load(Ordering::SeqCst)
            {
                shared.host.engine().cancel(&stream);
            }
            talk.out
                .notify("voice.chat", json!({ "event": event, "turn": turn }));
        }
        erase
    };
    let (sent, erase) = tokio::join!(sending, relaying);
    if let Some(chat) = erase {
        shared.erase_editor_chats(vec![chat]).await;
    }
    if let Err(error) = sent {
        failed(error.to_string());
    }
}

/// Retire le flux d'un tour de sa séance quand il finit, quelle que soit
/// l'issue.
struct TurnStream<'a, H: Host> {
    talk: &'a Talk<H>,
    stream: &'a str,
}

impl<H: Host> Drop for TurnStream<'_, H> {
    fn drop(&mut self) {
        self.talk.streams.lock().remove(self.stream);
    }
}

/// Texte de l'échec d'une lecture par segments.
fn speech_failure(end: ReadingEnd) -> String {
    match end {
        ReadingEnd::NothingToRead => t!("bridge.speech.nothingToRead"),
        ReadingEnd::Unsupported => t!("bridge.speech.unsupported"),
        ReadingEnd::Failed(ReadAloudError::NoOutputDevice) => t!("bridge.speech.noOutputDevice"),
        ReadingEnd::Failed(ReadAloudError::RuntimeUnavailable) => t!("bridge.speech.unavailable"),
        ReadingEnd::MissingVoice(_)
        | ReadingEnd::Failed(_)
        | ReadingEnd::Finished
        | ReadingEnd::Stopped => t!("bridge.speech.failed"),
    }
}

/// Notification d'un évènement de lecture.
fn speech_event(read_id: &str, event: SegmentEvent) -> Value {
    let (kind, segment, message) = match event {
        SegmentEvent::Started(id) => ("started", Some(id), None),
        SegmentEvent::Finished(id) => ("finished", Some(id), None),
        SegmentEvent::Stopped => ("stopped", None, None),
        SegmentEvent::Failed(end) => ("error", None, Some(speech_failure(end))),
    };
    let mut params = json!({ "readId": read_id, "type": kind });
    if let Some(segment) = segment {
        params["segmentId"] = json!(segment);
    }
    if let Some(message) = message {
        params["message"] = json!(message);
    }
    params
}

fn refused(out: &Outbox, id: &Value, code: i64, data: Value) {
    out.error(id, code, "refused", Some(data));
}

fn invalid(out: &Outbox, id: &Value) {
    out.error(id, code::INVALID_PARAMS, "invalid params", None);
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SegmentParams {
    id: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadParams {
    segments: Vec<SegmentParams>,
    #[serde(default)]
    language: Option<VoiceLanguage>,
}

/// `speech.read` : lecture lancée (la précédente s'arrête), `{ readId }` ;
/// ses évènements suivent en `speech.event`.
pub(super) fn speech_read<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(params) = serde_json::from_value::<ReadParams>(params) else {
        return invalid(out, id);
    };
    let well_formed = !params.segments.is_empty()
        && params.segments.len() <= MAX_SEGMENTS
        && params.segments.iter().all(|segment| {
            !segment.id.is_empty()
                && segment.id.chars().count() <= MAX_SEGMENT_ID
                && segment.text.chars().count() <= MAX_SEGMENT_TEXT
        });
    if !well_formed {
        return invalid(out, id);
    }
    let number = shared.next_stream.fetch_add(1, Ordering::SeqCst);
    let read = read_id(connection, number);
    let segments = params
        .segments
        .into_iter()
        .map(|segment| Segment {
            id: segment.id,
            text: segment.text,
        })
        .collect();
    let notified = (out.clone(), read.clone());
    let sink: SegmentSink = Arc::new(move |event| {
        notified
            .0
            .notify("speech.event", speech_event(&notified.1, event));
    });
    // Réponse avant le premier évènement : la lecture prend au moins le
    // temps de charger la voix, mais l'ordre ne doit rien au hasard.
    let started = shared
        .host
        .voice()
        .read(read.clone(), segments, params.language, sink.clone());
    match started {
        Ok(()) => out.result(id, json!({ "readId": read })),
        Err(SegmentsRefusal::MissingVoice(language)) => refused(
            out,
            id,
            voice_code::MISSING_VOICE,
            json!({ "language": language }),
        ),
        Err(refusal) => {
            let message = match refusal {
                SegmentsRefusal::NothingToRead => t!("bridge.speech.nothingToRead"),
                SegmentsRefusal::Unsupported => t!("bridge.speech.unsupported"),
                _ => t!("bridge.speech.unavailable"),
            };
            refused(out, id, code::REFUSED, json!({ "message": message }));
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StopParams {
    read_id: String,
}

/// `speech.stop` : arrête la lecture si elle est celle de la connexion.
pub(super) fn speech_stop<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(params) = serde_json::from_value::<StopParams>(params) else {
        return invalid(out, id);
    };
    if of_connection(&params.read_id, connection) {
        shared.host.voice().stop_read(&params.read_id);
    }
    out.result(id, json!({}));
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartParams {
    profile: Profile,
    #[serde(default)]
    chat_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    context: Option<String>,
    /// Conversation ouverte par la séance éphémère (ignoré avec `chatId`).
    #[serde(default)]
    ephemeral: Option<bool>,
}

/// `voice.start`, dans l'ordre de lecture (rien n'y attend) : paramètres
/// vérifiés, puis séance inscrite AVANT toute attente, pour qu'un
/// `voice.stop` lu juste après la trouve. Une connexion qui tient déjà une
/// séance (ou la démarre) est refusée (-32032 `{ owner: "bridge" }`), sans
/// rien changer à la sienne. Rend la séance à démarrer par [`voice_start`].
pub(super) fn voice_register<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    client: String,
    out: &Outbox,
    id: &Value,
    params: Value,
) -> Option<Arc<Talk<H>>> {
    let Ok(params) = serde_json::from_value::<StartParams>(params) else {
        invalid(out, id);
        return None;
    };
    let talk = Arc::new_cyclic(|me| Talk {
        shared: Arc::downgrade(shared),
        me: me.clone(),
        connection,
        client,
        out: out.clone(),
        profile: params.profile,
        ephemeral: params.ephemeral == Some(true) && params.chat_id.is_none(),
        model: params.model,
        chat: Mutex::new(params.chat_id),
        context: Mutex::new(None),
        level_sent: Mutex::new(None),
        turns: shared.voice_turns(connection),
        told: AtomicBool::new(false),
        ended: AtomicBool::new(false),
        streams: Mutex::new(HashSet::new()),
    });
    if let Some(context) = params.context {
        talk.set_context(context);
    }
    if !shared.keep_talk(connection, talk.clone()) {
        refused(out, id, voice_code::BUSY, json!({ "owner": "bridge" }));
        return None;
    }
    Some(talk)
}

/// `voice.start`, suite (dans sa tâche) : `{ sessionId, chatId }` quand le
/// micro est ouvert. Un refus part AVANT l'état `off` : si l'éditeur a déjà
/// appris `starting`, un `voice.state` `off` suit la réponse d'erreur. Arrêtée
/// par `voice.stop` pendant le démarrage : -32800, micro refermé s'il s'est
/// ouvert entre-temps.
pub(super) async fn voice_start<H: Host>(
    shared: &Arc<Shared<H>>,
    out: &Outbox,
    id: &Value,
    talk: Arc<Talk<H>>,
) {
    let connection = talk.connection;
    // Adresse de la séance, recalculée à chaque usage (un pointeur gardé
    // pendant une attente rendrait la tâche non `Send`).
    let me = || Arc::as_ptr(&talk) as *const ();
    let chat = talk.chat.lock().clone();
    if let Some(chat) = chat {
        let known = shared.store.call(move |c| chats::get(c, &chat)).await;
        if let Err(error) = known {
            shared.forget_talk(connection, me());
            return refused(
                out,
                id,
                code::REFUSED,
                json!({ "message": error.to_string() }),
            );
        }
    }
    // `voice.stop` lu pendant l'attente : rien ne démarre.
    if !shared.holds_talk(connection, me()) {
        return cancelled(out, id);
    }
    let started = shared.host.voice().start(talk.clone()).await;
    if !shared.holds_talk(connection, me()) {
        // Arrêtée pendant le démarrage (`voice.stop`, ou fin de la séance) :
        // si le micro a fini par s'ouvrir, il se referme.
        if started.is_ok() {
            shared.host.voice().stop(connection);
        }
        return cancelled(out, id);
    }
    let error = match started {
        Ok(()) => {
            return out.result(
                id,
                json!({
                    "sessionId": uuid::Uuid::now_v7().to_string(),
                    "chatId": talk.chat.lock().clone(),
                }),
            );
        }
        Err(error) => error,
    };
    shared.forget_talk(connection, me());
    match error {
        StartError::Busy { page } => refused(
            out,
            id,
            voice_code::BUSY,
            json!({ "owner": if page { "page" } else { "bridge" } }),
        ),
        StartError::SpeechModelMissing => {
            out.error(id, voice_code::MISSING_SPEECH_MODEL, "refused", None);
        }
        StartError::VoiceMissing(language) => refused(
            out,
            id,
            voice_code::MISSING_VOICE,
            json!({ "language": language }),
        ),
        StartError::Failed(message) => refused(
            out,
            id,
            voice_code::MICROPHONE,
            json!({ "message": message }),
        ),
    }
    // Après la réponse : l'éditeur qui a vu `starting` apprend la fin.
    if talk.told.load(Ordering::SeqCst) && !talk.ended.load(Ordering::SeqCst) {
        out.notify("voice.state", json!({ "state": VoiceModeState::default() }));
    }
}

fn cancelled(out: &Outbox, id: &Value) {
    out.error(id, code::CANCELLED, "cancelled", None);
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MuteParams {
    muted: bool,
}

/// `voice.mute`.
pub(super) async fn voice_mute<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(params) = serde_json::from_value::<MuteParams>(params) else {
        return invalid(out, id);
    };
    match shared.host.voice().mute(connection, params.muted).await {
        Ok(()) => out.result(id, json!({})),
        Err(message) => refused(
            out,
            id,
            voice_code::MICROPHONE,
            json!({ "message": message }),
        ),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextParams {
    text: String,
}

/// `voice.context` : contexte de la séance de la connexion remplacé (sans
/// séance : rien).
pub(super) fn voice_context<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(params) = serde_json::from_value::<ContextParams>(params) else {
        return invalid(out, id);
    };
    if let Some(talk) = shared.talk(connection) {
        talk.set_context(params.text);
    }
    out.result(id, json!({}));
}

/// Pas de paramètre (`{}` ou absent).
fn no_params(params: &Value) -> bool {
    params.is_null() || params.as_object().is_some_and(serde_json::Map::is_empty)
}

/// `voice.stop` (sans paramètre), dans l'ordre de lecture : séance retirée
/// (un démarrage en cours voit qu'elle n'est plus voulue), puis arrêtée si
/// elle tient déjà le micro.
pub(super) fn voice_stop<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    out: &Outbox,
    id: &Value,
    params: &Value,
) {
    if !no_params(params) {
        return invalid(out, id);
    }
    shared.drop_talk(connection);
    shared.host.voice().stop(connection);
    out.result(id, json!({}));
}

/// `voice.resume` (sans paramètre) : séance en pause (silence prolongé, micro
/// perdu) rouverte ; sans effet hors pause ou sans séance.
pub(super) async fn voice_resume<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    if !no_params(&params) {
        return invalid(out, id);
    }
    match shared.host.voice().resume(connection).await {
        Ok(()) => out.result(id, json!({})),
        Err(message) => refused(
            out,
            id,
            voice_code::MICROPHONE,
            json!({ "message": message }),
        ),
    }
}

/// Connexion fermée (fin, éditeur retiré, pont coupé) : sa séance et sa
/// lecture s'arrêtent, rien ne reste ouvert pour elle.
pub(super) fn closed<H: Host>(shared: &Shared<H>, connection: u64) {
    shared.drop_talk(connection);
    let voice = shared.host.voice();
    voice.stop(connection);
    voice.stop_reads(connection);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_belongs_to_its_connection() {
        let read = read_id(12, 7);
        assert!(of_connection(&read, 12));
        assert!(!of_connection(&read, 1));
        assert!(!of_connection("12", 12));
    }

    #[test]
    fn speech_events_name_their_segment() {
        assert_eq!(
            speech_event("1-2", SegmentEvent::Started("a".into())),
            json!({ "readId": "1-2", "type": "started", "segmentId": "a" })
        );
        assert_eq!(
            speech_event("1-2", SegmentEvent::Stopped),
            json!({ "readId": "1-2", "type": "stopped" })
        );
        let failed = speech_event(
            "1-2",
            SegmentEvent::Failed(ReadingEnd::Failed(ReadAloudError::EngineFailed)),
        );
        assert_eq!(failed["type"], "error");
        assert!(failed["message"].as_str().is_some_and(|m| !m.is_empty()));
    }
}
