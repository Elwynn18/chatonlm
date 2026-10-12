//! Moteur de chat : envoi d'un message et réception de la réponse en flux.
//!
//! Règles :
//! - une seule réponse à la fois par conversation (verrou « occupée ») ;
//! - le message de l'utilisateur est enregistré avant la requête ; la réponse
//!   n'est créée en base qu'au premier fragment, puis enregistrée par lots
//!   (toutes les 500 ms) et une dernière fois à la fin ;
//! - pendant la réponse, son état courant est gardé en mémoire et superposé à
//!   la conversation lue par `get_chat` ;
//! - la fin du raisonnement est datée au premier fragment de réponse, ou à la
//!   fin quelle qu'elle soit ;
//! - exactement un événement final (`done`, `error` ou `cancelled`), émis après
//!   la dernière écriture ; la commande se termine ensuite ;
//! - l'annulation interrompt l'attente de la connexion comme la lecture ;
//! - avant `/api/chat`, `/api/show` : un modèle absent (404 « not found ») est
//!   d'abord téléchargé (`pull`), jamais quand le serveur est injoignable ;
//!   `think` est retiré si le modèle ne l'accepte pas (capacités,
//!   `thinking.values`) ; tout autre échec de `/api/show` laisse l'envoi
//!   se faire sans ce filtre ;
//! - chargement annoncé (propre à ChatonLM), une fois par envoi, après
//!   `/api/show` et la vérification du compte de la recherche web : un modèle
//!   local absent de `/api/ps`, ou chargé avec un autre contexte que celui
//!   qu'Ollama retiendrait, est chargé par `/api/generate` (mêmes `num_ctx` et
//!   `keep_alive` que le chat) entre `loading` et `loaded` ; un refus d'Ollama
//!   finit l'envoi après `loaded`, classé comme une erreur de `/api/chat` ;
//!   une connexion impossible est journalisée (le chat la dira) ; `/api/ps`
//!   en échec n'annonce rien ; un arrêt pendant le chargement finit l'envoi
//!   sans `loaded` ;
//! - pièces jointes : prises dans la zone de préparation (identifiant inconnu :
//!   refus d'emblée, rien d'enregistré), écrites avec le message de
//!   l'utilisateur dans la même transaction ; remises dans la zone si l'envoi
//!   est refusé d'emblée ; une fois l'envoi accepté, elles la quittent même si
//!   l'écriture échoue (la page a vidé son composer, rien ne les renverrait) ;
//! - modification d'un message (`editMessageId`, comme l'officiel qui renvoie
//!   l'index du message) : vérifiée avant tout (message de l'utilisateur de
//!   cette conversation, pièces jointes gardées à lui et sans répétition ;
//!   sinon refus d'emblée, rien d'écrit), puis écrite en une transaction à la
//!   place du nouveau message (texte, pièces jointes gardées et nouvelles,
//!   messages suivants supprimés) ; la suite est celle d'un envoi, avec
//!   l'historique coupé. Écarts à l'officiel : le message est désigné par son
//!   identifiant en base, pas par sa position (sûr si la liste a changé), et
//!   les pièces jointes gardées par leur identifiant, pas par leur nom (deux
//!   fichiers de même nom confondus) ;
//! - écart à l'officiel : les images ne partent pas vers un modèle dont les
//!   capacités connues n'ont pas « vision » (l'officiel les envoie toujours ;
//!   Ollama 0.35 refuse alors toute la requête, 400 « model does not support
//!   multimodal requests », à chaque message suivant de la conversation) ;
//! - recherche web (`webSearch`, comme l'officiel, `ui.go`) : outils
//!   `web_search` et `web_fetch` (voir `tools.rs`) offerts seulement si elle
//!   est demandée, si les détails du modèle sont connus avec la capacité
//!   `tools` et si le message envoyé (nouveau, ou modifié : pièces jointes
//!   gardées et nouvelles) n'a pas de pièce jointe. Un tour qui finit avec
//!   des appels : réponse créée si besoin (`replyStarted`), appels enregistrés
//!   sans résultat puis annoncés (`toolCalls`), dernière écriture du tour
//!   (raisonnement clos), puis exécution dans l'ordre, chaque résultat ou
//!   erreur écrit puis annoncé (`toolResult`) ; le tour suivant relit
//!   l'historique et répond dans un nouveau message de l'assistant ; un tour
//!   sans appel finit l'envoi, et `done` désigne le dernier message de
//!   l'assistant. Une erreur d'outil va au modèle sans arrêter l'envoi ; un
//!   arrêt pendant un outil interrompt sa requête et laisse l'appel sans
//!   résultat ni erreur. Sans compte ollama.com relié au serveur, l'envoi
//!   s'arrête (`webSearchSignedOut`, l'interface propose de se connecter ou
//!   de relancer sans recherche) : avant la première requête si `/api/me`
//!   le dit (pas demandé quand le cloud du serveur est coupé), sinon au
//!   premier 401 d'un outil (l'appel reste alors sans résultat ni erreur). La réponse vivante superposée par `get_chat` porte
//!   ses appels et les résultats reçus. Écarts à l'officiel : au plus
//!   10 tours d'appels par envoi, puis une requête sans outils (l'officiel
//!   boucle sans limite ; le middleware du serveur s'arrête à 10) ; appels et
//!   résultats enregistrés avec la réponse (table `tool_calls`), pas en
//!   messages `tool` séparés (historique envoyé identique) ; appels reçus
//!   d'une requête sans outils ignorés ; pas d'outils « browser » propres à
//!   gpt-oss ; pas d'exclusion du raisonnement et de la recherche (règle
//!   officielle propre à deepseek-v3.1, modèle cloud) ; sans compte, arrêt
//!   de l'envoi au lieu du bandeau qui bloque l'envoi (et, compte inconnu,
//!   au lieu de l'erreur donnée au modèle) ;
//! - envoi du mode conversation vocale (`voice`, propre à ChatonLM) : la
//!   consigne vocale (réglage, sinon celle par défaut) part en premier
//!   message `system` de chaque requête `/api/chat` de l'envoi, tours
//!   d'outils compris, sans jamais être enregistrée ; l'observateur reçoit
//!   la demande avec chaque événement (lecture de la réponse, séance).

pub(crate) mod pull;
pub mod stream;
pub(crate) mod tools;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tokio::sync::watch;
use ts_rs::TS;
use url::Url;

use crate::attachments::{Staging, Taken};
use crate::error::{AppError, AppResult};
use crate::hub::errors::connection_lost;
use crate::i18n::t;
use crate::ollama::models::{self, ModelDetails, ShowError};
use crate::ollama::running::{self, LoadFailure, RunningModel};
use crate::ollama::{account, client};
use crate::store::chats::{self, Chat, Message, Reply, Role, ToolCall, Turn};
use crate::store::settings::Settings;
use crate::store::{Store, model_prefs, now_ms};
use crate::tts::VoiceLanguage;
use stream::{Line, LineBuffer, StreamToolCall, Think};
use tools::{AllowedUrls, Executed};

/// Intervalle des écritures en base pendant une réponse.
const SAVE_INTERVAL: Duration = Duration::from_millis(500);

/// Délai laissé à une réponse annulée pour s'enregistrer et se terminer.
const CANCEL_WAIT: Duration = Duration::from_secs(3);

/// Tours d'appels d'outils au plus par envoi ; la requête suivante part sans
/// outils (comme `maxWebSearchLoops` du serveur).
const MAX_TOOL_TURNS: usize = 10;

/// Délai de `/api/ps` avant l'envoi : au-delà, le chat part sans annoncer de
/// chargement.
const PS_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SendRequest {
    /// Conversation existante ; absent = nouvelle conversation.
    #[ts(optional)]
    pub chat_id: Option<String>,
    /// Identifiant choisi par le frontend, pour annuler ce flux.
    pub stream_id: String,
    pub content: String,
    pub model: String,
    /// Raisonnement demandé ; absent = défaut du modèle.
    #[ts(optional)]
    pub think: Option<Think>,
    /// Fichiers de la zone de préparation joints au message, dans l'ordre ;
    /// absent = aucun.
    #[ts(optional)]
    pub attachment_ids: Option<Vec<String>>,
    /// Message de l'utilisateur modifié (identifiant en base) : réécrit, et
    /// les messages suivants supprimés ; absent = nouveau message. Requiert
    /// `chatId`.
    #[ts(optional)]
    pub edit_message_id: Option<i64>,
    /// Pièces jointes du message modifié à garder (avant les nouvelles, dans
    /// leur ordre d'origine) ; absent = aucune. Seulement avec
    /// `editMessageId`.
    #[ts(optional)]
    pub kept_attachment_ids: Option<Vec<i64>>,
    /// Recherche web demandée (outils `web_search` et `web_fetch`, si le
    /// modèle a la capacité `tools`) ; absent = non.
    #[ts(optional)]
    pub web_search: Option<bool>,
    /// Envoi du mode conversation vocale : consigne vocale ajoutée en message
    /// système à chaque requête, réponse lue à voix haute ; absent = envoi
    /// ordinaire.
    #[ts(optional)]
    pub voice: Option<VoiceSend>,
}

/// Envoi d'une phrase du mode conversation vocale.
#[derive(Clone, Debug, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct VoiceSend {
    /// Langue de la phrase transcrite (indice de la voix) ; absente = inconnue.
    #[ts(optional)]
    pub language: Option<VoiceLanguage>,
}

/// Modification d'un message de l'utilisateur.
#[derive(Clone, Debug)]
struct Edit {
    message_id: i64,
    kept: Vec<i64>,
}

impl SendRequest {
    /// Modification demandée, ou refus d'une demande incohérente (erreurs en
    /// anglais : la page n'en envoie jamais).
    fn edit(&self) -> AppResult<Option<Edit>> {
        let Some(message_id) = self.edit_message_id else {
            if self.kept_attachment_ids.is_some() {
                // i18n-ignore
                return Err(AppError::Invalid(
                    "keptAttachmentIds needs editMessageId".into(),
                ));
            }
            return Ok(None);
        };
        if self.chat_id.is_none() {
            // i18n-ignore
            return Err(AppError::Invalid("editMessageId needs chatId".into()));
        }
        Ok(Some(Edit {
            message_id,
            kept: self.kept_attachment_ids.clone().unwrap_or_default(),
        }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ChatErrorKind {
    /// Connexion à Ollama impossible.
    Unreachable,
    /// Connexion coupée pendant la réponse.
    ConnectionLost,
    /// Modèle absent du serveur.
    ModelMissing,
    /// Échec du téléchargement du modèle.
    Download,
    /// Erreur renvoyée par Ollama ou par le modèle.
    Model,
    /// Recherche web demandée sans compte ollama.com relié au serveur.
    WebSearchSignedOut,
    /// Erreur de ChatonLM (base de données...).
    Internal,
}

/// Erreur [`ChatErrorKind::WebSearchSignedOut`] (texte de l'officiel,
/// bandeau de `ChatForm.tsx`).
fn signed_out() -> Outcome {
    Outcome::Failed(
        ChatErrorKind::WebSearchSignedOut,
        t!("chat.webSearchSignedOut"),
    )
}

/// Événements envoyés au frontend pendant un envoi.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum ChatEvent {
    /// Nouvelle conversation créée pour ce message.
    #[serde(rename_all = "camelCase")]
    ChatCreated {
        chat_id: String,
    },
    /// Message de l'utilisateur enregistré.
    UserMessage {
        message: Message,
    },
    /// Progression du téléchargement du modèle : la plus grosse couche, comme
    /// l'officiel, sauf pour un `hf.co/...` dont le manifeste est lisible
    /// (somme de toutes ses couches sur sa taille, voir `pull.rs`) ; le
    /// dernier avec `done: true` et completed == total.
    Download {
        total: u64,
        completed: u64,
        done: bool,
    },
    /// Le modèle n'est pas en mémoire (ou l'est avec un autre contexte) :
    /// chargement lancé avant `/api/chat` (propre à ChatonLM).
    Loading,
    /// Fin du chargement annoncé par `Loading`, réussi ou non (un refus
    /// d'Ollama suit en `Error`) ; toujours avant `ReplyStarted`.
    Loaded,
    /// Réponse créée en base, au premier fragment.
    #[serde(rename_all = "camelCase")]
    ReplyStarted {
        message_id: i64,
        created_at: i64,
    },
    /// Fragment de raisonnement.
    Thinking {
        text: String,
    },
    /// Fragment de réponse.
    Content {
        text: String,
    },
    /// Appels d'outils d'une réponse, enregistrés sans résultat, à la fin de
    /// son tour.
    #[serde(rename_all = "camelCase")]
    ToolCalls {
        message_id: i64,
        tool_calls: Vec<ToolCall>,
    },
    /// Résultat (ou erreur) de l'appel `index` d'une réponse, enregistré.
    #[serde(rename_all = "camelCase")]
    ToolResult {
        message_id: i64,
        index: u32,
        result: Option<String>,
        error: Option<String>,
    },
    /// Fin normale ; `messageId` : dernier message de l'assistant de l'envoi
    /// (un par tour d'outils), absent si le modèle n'a rien répondu.
    #[serde(rename_all = "camelCase")]
    Done {
        message_id: Option<i64>,
    },
    Error {
        kind: ChatErrorKind,
        message: String,
    },
    Cancelled,
}

/// Réponses en cours, partagées par les commandes.
#[derive(Default)]
pub struct ChatEngine {
    /// Par conversation.
    running: Mutex<HashMap<String, Running>>,
    /// État courant des réponses, par conversation.
    live: Mutex<HashMap<String, (i64, Reply)>>,
    /// Reçoit les événements de toutes les réponses, avec leur conversation
    /// et, pour un envoi du mode vocal, sa demande (lecture à voix haute et
    /// mode conversation vocale, propres à ChatonLM) ; posé une fois au
    /// démarrage.
    observer: std::sync::OnceLock<Observer>,
}

type Observer = Box<dyn Fn(&str, &ChatEvent, Option<&VoiceSend>) + Send + Sync>;

struct Running {
    stream_id: String,
    cancel: watch::Sender<bool>,
    finished: watch::Receiver<bool>,
}

impl ChatEngine {
    /// Pose l'observateur des événements (une fois, au démarrage).
    pub fn set_observer(
        &self,
        observer: impl Fn(&str, &ChatEvent, Option<&VoiceSend>) + Send + Sync + 'static,
    ) {
        let _ = self.observer.set(Box::new(observer));
    }

    fn observe(&self, chat: Option<&str>, event: &ChatEvent, voice: Option<&VoiceSend>) {
        if let (Some(observer), Some(chat)) = (self.observer.get(), chat) {
            observer(chat, event, voice);
        }
    }

    fn begin(
        &self,
        chat_id: &str,
        stream_id: &str,
    ) -> AppResult<(watch::Receiver<bool>, watch::Sender<bool>)> {
        let mut running = self.running.lock();
        if running.contains_key(chat_id) {
            return Err(AppError::Invalid(t!("chat.busy")));
        }
        let (cancel, cancelled) = watch::channel(false);
        let (finish, finished) = watch::channel(false);
        running.insert(
            chat_id.to_owned(),
            Running {
                stream_id: stream_id.to_owned(),
                cancel,
                finished,
            },
        );
        Ok((cancelled, finish))
    }

    fn end(&self, chat_id: &str) {
        self.running.lock().remove(chat_id);
        self.live.lock().remove(chat_id);
    }

    fn update_live(&self, chat_id: &str, message_id: i64, reply: &Reply) {
        self.live
            .lock()
            .insert(chat_id.to_owned(), (message_id, reply.clone()));
    }

    /// Demande l'arrêt d'un flux ; sans effet s'il est déjà fini.
    pub fn cancel(&self, stream_id: &str) {
        for running in self.running.lock().values() {
            if running.stream_id == stream_id {
                let _ = running.cancel.send(true);
            }
        }
    }

    /// Une réponse est en cours (Settings > Model location prévient qu'elle
    /// s'arrêtera).
    pub fn is_busy(&self) -> bool {
        !self.running.lock().is_empty()
    }

    /// Demande l'arrêt de tous les flux (page rechargée, fermeture,
    /// redémarrage d'Ollama par Settings > Model location).
    pub fn cancel_all(&self) {
        for running in self.running.lock().values() {
            let _ = running.cancel.send(true);
        }
    }

    /// Arrête la réponse en cours d'une conversation et attend sa fin
    /// (avant de la supprimer).
    pub async fn stop_chat(&self, chat_id: &str) {
        let finished = self.running.lock().get(chat_id).map(|running| {
            let _ = running.cancel.send(true);
            running.finished.clone()
        });
        if let Some(finished) = finished {
            wait_finished(finished, CANCEL_WAIT).await;
        }
    }

    /// Arrête toutes les réponses et attend leur fin, au plus `limit` en tout.
    pub async fn stop_all(&self, limit: Duration) {
        self.cancel_all();
        let deadline = Instant::now() + limit;
        let waiting: Vec<_> = self
            .running
            .lock()
            .values()
            .map(|running| running.finished.clone())
            .collect();
        for finished in waiting {
            wait_finished(finished, deadline.saturating_duration_since(Instant::now())).await;
        }
    }

    /// Superpose l'état courant d'une réponse en cours à la conversation lue
    /// en base (les écritures n'ont lieu que toutes les 500 ms). Ce qui ne
    /// fait que s'ajouter (fin du raisonnement, appels, résultats) est gardé
    /// de la base s'il y est déjà : une lecture entre une écriture et la mise à
    /// jour de l'état courant ne le perd pas.
    pub fn overlay(&self, chat: &mut Chat) {
        if let Some((message_id, reply)) = self.live.lock().get(&chat.id)
            && let Some(message) = chat.messages.iter_mut().find(|m| m.id == *message_id)
        {
            message.content.clone_from(&reply.content);
            message.thinking.clone_from(&reply.thinking);
            message.thinking_started_at = reply.thinking_started_at;
            message.thinking_ended_at = reply.thinking_ended_at.or(message.thinking_ended_at);
            let mut calls = reply.tool_calls.clone();
            for (live, saved) in calls.iter_mut().zip(&message.tool_calls) {
                if live.result.is_none() && live.error.is_none() {
                    live.result.clone_from(&saved.result);
                    live.error.clone_from(&saved.error);
                }
            }
            if let Some(more) = message.tool_calls.get(calls.len()..) {
                calls.extend_from_slice(more);
            }
            message.tool_calls = calls;
        }
    }
}

async fn wait_finished(mut finished: watch::Receiver<bool>, limit: Duration) {
    let _ = tokio::time::timeout(limit, finished.wait_for(|done| *done)).await;
}

/// Se termine quand l'annulation est demandée.
async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    if cancel.wait_for(|cancelled| *cancelled).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Réglages du serveur utilisés pour l'envoi.
pub struct Server {
    pub url: String,
    /// 0 = défaut du serveur.
    pub context_length: u32,
    /// `keep_alive` envoyé (secondes ; 0 = rien d'envoyé, défaut du serveur ;
    /// -1 = garder chargé).
    pub keep_alive: i64,
}

impl Server {
    /// Adresse effective, contexte et durée en mémoire réglés.
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            url: settings.effective_server_url.clone(),
            context_length: settings.context_length,
            keep_alive: settings.keep_alive,
        }
    }
}

/// Envoie un message (ou en modifie un) et transmet la réponse. Erreur
/// seulement si l'envoi est refusé d'emblée (message vide, pièce jointe
/// inconnue, message modifié inconnu, conversation occupée...) ; sinon la fin
/// arrive par l'événement final.
pub async fn send(
    store: &Store,
    engine: &ChatEngine,
    staging: &Staging,
    server: Server,
    request: SendRequest,
    events: &Channel<ChatEvent>,
) -> AppResult<()> {
    // Comme l'officiel : un message fait seulement d'espaces est refusé, les
    // autres sont enregistrés et envoyés tels quels. Un texte est requis même
    // avec des pièces jointes.
    if request.content.trim().is_empty() {
        return Err(AppError::Invalid(t!("chat.emptyMessage")));
    }
    let model = request.model.trim().to_owned();
    if model.is_empty() {
        return Err(AppError::Invalid(t!("chat.noModel")));
    }
    let edit = request.edit()?;
    // Retirées de la zone tout de suite (un retrait simultané ne peut plus
    // les enlever), remises en place si l'envoi est refusé d'emblée.
    let files = staging.take(request.attachment_ids.as_deref().unwrap_or_default())?;
    // Conversation de l'envoi, donnée à l'observateur (connue avant le
    // premier événement), avec la demande d'un envoi vocal.
    let observed: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let voice = request.voice.clone();
    let emit = |event: ChatEvent| {
        engine.observe(observed.get().map(String::as_str), &event, voice.as_ref());
        let _ = events.send(event);
    };
    // Envoi vocal : consigne vocale réglée (vide : par défaut), envoyée en
    // message système avec chaque requête, jamais enregistrée (écart à
    // l'officiel, qui n'a pas de consigne système dans le chat).
    let system = match &voice {
        Some(_) => Some(voice_instructions(store).await),
        None => None,
    };

    let chat_id = match request.chat_id {
        Some(id) => id,
        None => {
            let id = store.call(|c| chats::create(c, now_ms())).await?;
            let _ = observed.set(id.clone());
            emit(ChatEvent::ChatCreated {
                chat_id: id.clone(),
            });
            id
        }
    };
    let _ = observed.set(chat_id.clone());
    let (mut cancel, finish) = engine.begin(&chat_id, &request.stream_id)?;
    // Modification vérifiée avant toute écriture (`chatId` présent, vérifié
    // par `edit`), puis revérifiée dans la transaction qui l'écrit. Après
    // `begin` : un arrêt demandé pendant la vérification n'est pas perdu.
    if let Some(edit) = &edit {
        let (edit, id) = (edit.clone(), chat_id.clone());
        let checked = store
            .call(move |c| chats::check_edit(c, &id, edit.message_id, &edit.kept))
            .await;
        if let Err(error) = checked {
            engine.end(&chat_id);
            let _ = finish.send(true);
            return Err(error);
        }
    }

    let mut reply = ReplyWriter {
        chat_id: chat_id.clone(),
        model: model.clone(),
        message_id: None,
        previous: None,
        reply: Reply::default(),
        last_save: Instant::now(),
        think_off: false,
        always_thinks_noted: false,
    };
    let conversation = Conversation {
        store,
        engine,
        emit: &emit,
    };
    let prompt = Prompt {
        content: request.content,
        files,
        edit,
    };
    let asking = Asking {
        think: request.think,
        images: true,
        system,
    };
    let web_search = request.web_search == Some(true);
    let outcome = conversation
        .run(&server, prompt, asking, web_search, &mut reply, &mut cancel)
        .await;
    let saved = conversation.finish(&mut reply).await;
    engine.end(&chat_id);
    let _ = finish.send(true);

    emit(match (outcome, saved) {
        (_, Err(error)) => ChatEvent::Error {
            kind: ChatErrorKind::Internal,
            message: error.to_string(),
        },
        (Outcome::Done, Ok(())) => ChatEvent::Done {
            message_id: reply.message_id.or(reply.previous),
        },
        (Outcome::Cancelled, Ok(())) => ChatEvent::Cancelled,
        (Outcome::Failed(kind, message), Ok(())) => ChatEvent::Error { kind, message },
    });
    Ok(())
}

enum Outcome {
    Done,
    Cancelled,
    Failed(ChatErrorKind, String),
}

impl From<AppError> for Outcome {
    fn from(error: AppError) -> Self {
        Outcome::Failed(ChatErrorKind::Internal, error.to_string())
    }
}

/// Réponse en cours de réception (celle du tour en cours).
struct ReplyWriter {
    chat_id: String,
    model: String,
    message_id: Option<i64>,
    /// Message de l'assistant du dernier tour d'outils.
    previous: Option<i64>,
    reply: Reply,
    last_save: Instant,
    /// `think: false` envoyé : un raisonnement reçu quand même montre que le
    /// modèle ne peut pas le couper (retenu une fois, `model_traits`).
    think_off: bool,
    always_thinks_noted: bool,
}

impl ReplyWriter {
    /// Après un tour d'outils : le tour suivant répondra dans un nouveau
    /// message, créé à son premier fragment.
    fn next_turn(&mut self) {
        self.previous = self.message_id.take().or(self.previous);
        self.reply = Reply::default();
        self.last_save = Instant::now();
    }
}

/// Réglages de la requête, communs aux tours d'un envoi.
struct Asking {
    /// Retiré au premier refus (400 « does not support thinking »).
    think: Option<Think>,
    /// Images des pièces jointes envoyées.
    images: bool,
    /// Message système placé en tête de chaque requête (consigne vocale).
    system: Option<String>,
}

/// Message de l'utilisateur à écrire avant la requête.
struct Prompt<'a> {
    content: String,
    files: Taken<'a>,
    /// Message existant à modifier ; absent = nouveau message.
    edit: Option<Edit>,
}

/// Résultat de l'ouverture du flux.
enum Opened {
    Stream(reqwest::Response),
    /// 400 « does not support thinking » : à relancer sans `think` ; texte de
    /// l'erreur si `think` n'était pas demandé.
    ThinkingRefused(String),
    Ended(Outcome),
}

struct Conversation<'a, E: Fn(ChatEvent)> {
    store: &'a Store,
    engine: &'a ChatEngine,
    emit: &'a E,
}

impl<E: Fn(ChatEvent)> Conversation<'_, E> {
    async fn run(
        &self,
        server: &Server,
        prompt: Prompt<'_>,
        mut asking: Asking,
        web_search: bool,
        reply: &mut ReplyWriter,
        cancel: &mut watch::Receiver<bool>,
    ) -> Outcome {
        let chat_id = reply.chat_id.clone();
        let Prompt {
            content,
            files,
            edit,
        } = prompt;
        let staged = files.files().to_vec();
        let written = self
            .store
            .call(move |c| {
                let attachments: Vec<_> = staged.iter().map(|file| file.as_new()).collect();
                match edit {
                    // Messages suivants supprimés : l'historique lu plus bas
                    // s'arrête au message modifié.
                    Some(edit) => chats::edit_user_message(
                        c,
                        &chat_id,
                        edit.message_id,
                        &content,
                        &edit.kept,
                        &attachments,
                        now_ms(),
                    ),
                    None => chats::add_user_message(c, &chat_id, &content, &attachments, now_ms()),
                }
            })
            .await;
        // Enregistrés, ou perdus avec un envoi accepté puis en échec : les
        // fichiers quittent la zone de préparation (la page ne connaît plus
        // leurs identifiants, ils y resteraient jusqu'au rechargement).
        files.keep();
        let user = match written {
            Ok(user) => user,
            Err(error) => return error.into(),
        };
        // Comme l'officiel (`hasAttachments`) : pas d'outils pour un message
        // avec des pièces jointes.
        let has_attachments = !user.attachments.is_empty();
        (self.emit)(ChatEvent::UserMessage { message: user });

        let mut turns = match self.history(&reply.chat_id).await {
            Ok(turns) => turns,
            Err(error) => return error.into(),
        };

        let details = match self.prepare(&server.url, &reply.model, cancel).await {
            Ok(details) => details,
            Err(outcome) => return outcome,
        };
        asking.think = accepted_think(asking.think, details.as_ref());
        asking.images = accepts_images(details.as_ref());
        let definitions = (web_search && !has_attachments && accepts_tools(details.as_ref()))
            .then(tools::definitions);
        // Recherche demandée, aucun compte relié au serveur : arrêt avant la
        // génération. Compte inconnu (ollama.com injoignable...) : l'envoi
        // part, un 401 d'un outil l'arrêtera.
        if definitions.is_some() {
            let linked = tokio::select! {
                biased;
                () = cancelled(cancel) => return Outcome::Cancelled,
                linked = linked_unless_cloud_off(&server.url) => linked,
            };
            if linked == Some(false) {
                return signed_out();
            }
        }
        // Une fois par envoi, pas à chaque tour d'outils.
        if let Err(outcome) = self
            .load(server, &reply.model, details.as_ref(), cancel)
            .await
        {
            return outcome;
        }
        // Adresses que `web_fetch` peut lire : celles des messages de
        // l'utilisateur, puis celles reçues pendant l'envoi.
        let mut allowed = AllowedUrls::from_text(&user_text(&turns));
        let mut tool_turns = 0;
        loop {
            let offered = definitions.as_ref().filter(|_| tool_turns < MAX_TOOL_TURNS);
            let calls = match self
                .ask(server, &turns, &mut asking, offered, reply, cancel)
                .await
            {
                Ok(calls) => calls,
                Err(outcome) => return outcome,
            };
            // Appels d'une requête sans outils : ignorés (rien ne les
            // exécuterait).
            if calls.is_empty() || offered.is_none() {
                return Outcome::Done;
            }
            tool_turns += 1;
            if let Err(outcome) = self
                .run_tools(&server.url, &calls, &mut allowed, reply, cancel)
                .await
            {
                return outcome;
            }
            reply.next_turn();
            turns = match self.history(&reply.chat_id).await {
                Ok(turns) => turns,
                Err(error) => return error.into(),
            };
        }
    }

    async fn history(&self, chat_id: &str) -> AppResult<Vec<Turn>> {
        let chat_id = chat_id.to_owned();
        self.store.call(move |c| chats::history(c, &chat_id)).await
    }

    /// Une requête à `/api/chat`, lue jusqu'à `done` : appels d'outils reçus
    /// pendant le tour. Erreur : fin déjà décidée.
    async fn ask(
        &self,
        server: &Server,
        turns: &[Turn],
        asking: &mut Asking,
        tools: Option<&serde_json::Value>,
        reply: &mut ReplyWriter,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Vec<StreamToolCall>, Outcome> {
        loop {
            reply.think_off = asking.think == Some(Think::Enabled(false));
            let mut body = stream::request_body(
                &reply.model,
                turns,
                asking.think.as_ref(),
                server.context_length,
                server.keep_alive,
                asking.images,
                tools,
            );
            if let Some(system) = &asking.system {
                with_system_message(&mut body, system);
            }
            match open(&server.url, &body, cancel).await {
                Opened::Stream(response) => return self.read(response, reply, cancel).await,
                // Modèle sans raisonnement réglable : nouvel essai sans `think`.
                Opened::ThinkingRefused(_) if asking.think.is_some() => asking.think = None,
                Opened::ThinkingRefused(message) => {
                    return Err(Outcome::Failed(ChatErrorKind::Model, message));
                }
                Opened::Ended(outcome) => return Err(outcome),
            }
        }
    }

    /// Fin d'un tour avec des appels : réponse créée si besoin, appels
    /// enregistrés puis annoncés, dernière écriture du tour, puis exécution
    /// dans l'ordre (chaque résultat écrit puis annoncé). Erreur : fin déjà
    /// décidée (arrêt pendant un outil : l'appel reste sans résultat).
    async fn run_tools(
        &self,
        base: &str,
        received: &[StreamToolCall],
        allowed: &mut AllowedUrls,
        reply: &mut ReplyWriter,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<(), Outcome> {
        let message_id = self.start_reply(reply, now_ms()).await?;
        let calls: Vec<ToolCall> = received
            .iter()
            .map(|call| ToolCall::new(call.name.clone(), call.arguments.to_string()))
            .collect();
        // Réponse vivante mise à jour avant chaque écriture, comme pour le
        // texte : `overlay` la superpose sans condition à la base, elle n'y
        // est jamais en retard. Raisonnement clos dès maintenant (la dernière
        // écriture du tour garde cette date).
        reply.reply.tool_calls.clone_from(&calls);
        if reply.reply.thinking_started_at.is_some() {
            reply.reply.thinking_ended_at.get_or_insert(now_ms());
        }
        self.engine
            .update_live(&reply.chat_id, message_id, &reply.reply);
        let saved = calls.clone();
        self.store
            .call(move |c| chats::add_tool_calls(c, message_id, &saved))
            .await?;
        (self.emit)(ChatEvent::ToolCalls {
            message_id,
            tool_calls: calls,
        });
        self.finish(reply).await?;

        for (index, call) in (0u32..).zip(received) {
            let (result, error) =
                match tools::execute(base, &call.name, &call.arguments, allowed, cancel).await {
                    Executed::Result(result) => (Some(result), None),
                    Executed::Error(error) => (None, Some(error)),
                    Executed::Cancelled => return Err(Outcome::Cancelled),
                    Executed::SignedOut => return Err(signed_out()),
                };
            if let Some(live) = reply.reply.tool_calls.get_mut(index as usize) {
                live.result.clone_from(&result);
                live.error.clone_from(&error);
            }
            self.engine
                .update_live(&reply.chat_id, message_id, &reply.reply);
            let (saved_result, saved_error) = (result.clone(), error.clone());
            self.store
                .call(move |c| {
                    chats::set_tool_result(
                        c,
                        message_id,
                        index,
                        saved_result.as_deref(),
                        saved_error.as_deref(),
                    )
                })
                .await?;
            (self.emit)(ChatEvent::ToolResult {
                message_id,
                index,
                result,
                error,
            });
        }
        Ok(())
    }

    /// Détails du modèle pour l'envoi (`None` : inconnus) ; un modèle absent
    /// est d'abord téléchargé, puis redemandé. Erreur : fin déjà décidée.
    async fn prepare(
        &self,
        base: &str,
        model: &str,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Option<ModelDetails>, Outcome> {
        match show(base, model, cancel).await {
            Shown::Found(details) => return Ok(Some(details)),
            Shown::Missing => {}
            Shown::Unknown => return Ok(None),
            Shown::Unreachable => return Err(unreachable(base)),
            Shown::Cancelled => return Err(Outcome::Cancelled),
        }
        match pull::pull(base, model, cancel, self.emit).await {
            Outcome::Done => {}
            outcome => return Err(outcome),
        }
        // Une seule tentative de téléchargement : encore absent, il sera
        // signalé par /api/chat.
        match show(base, model, cancel).await {
            Shown::Found(details) => Ok(Some(details)),
            Shown::Missing | Shown::Unknown => Ok(None),
            Shown::Unreachable => Err(unreachable(base)),
            Shown::Cancelled => Err(Outcome::Cancelled),
        }
    }

    /// Chargement annoncé (propre à ChatonLM) : un modèle local absent de
    /// `/api/ps`, ou chargé avec un autre contexte que celui qu'Ollama
    /// retiendrait pour cet envoi, est chargé par `/api/generate` avec les
    /// options du chat entre `Loading` et `Loaded`. Contexte du serveur
    /// (réglage « Default », 0) : seul « non listé » compte ; un modèle chargé
    /// avec un autre contexte (par un autre client, ou avant un changement du
    /// réglage) est alors rechargé par `/api/chat` sans être annoncé. Refus
    /// d'Ollama (mémoire insuffisante, modèle absent) : `Loaded`, puis l'envoi
    /// finit avec l'erreur que `/api/chat` aurait donnée, sans la redemander
    /// (un échec lent attendrait deux fois). Connexion impossible : journalisé,
    /// le chat part et dira l'erreur. `/api/ps` en échec (ou trop lent) : rien
    /// d'annoncé, le chat part comme avant. Erreur : fin décidée (arrêt
    /// demandé, sans `Loaded` ; refus d'Ollama).
    async fn load(
        &self,
        server: &Server,
        model: &str,
        details: Option<&ModelDetails>,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<(), Outcome> {
        if details.is_some_and(|details| details.remote) {
            return Ok(());
        }
        let listed = tokio::select! {
            biased;
            () = cancelled(cancel) => return Err(Outcome::Cancelled),
            listed = running::list_within(&server.url, PS_TIMEOUT) => listed,
        };
        let Ok(listed) = listed else {
            return Ok(());
        };
        let max_context = details.and_then(|details| details.max_context);
        if !needs_loading(model, &listed, server.context_length, max_context) {
            return Ok(());
        }
        (self.emit)(ChatEvent::Loading);
        let (context_length, keep_alive) = (server.context_length, server.keep_alive);
        let loading = running::try_load(&server.url, model, context_length, keep_alive);
        let loaded = tokio::select! {
            biased;
            () = cancelled(cancel) => return Err(Outcome::Cancelled),
            loaded = loading => loaded,
        };
        (self.emit)(ChatEvent::Loaded);
        match load_outcome(model, loaded) {
            Some(outcome) => Err(outcome),
            None => Ok(()),
        }
    }

    /// Lit le flux jusqu'à `done` : appels d'outils reçus, dans l'ordre.
    /// Erreur : fin déjà décidée.
    async fn read(
        &self,
        mut response: reqwest::Response,
        reply: &mut ReplyWriter,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Vec<StreamToolCall>, Outcome> {
        let mut buffer = LineBuffer::default();
        let mut calls = Vec::new();
        // Au moins une ligne lisible reçue : une dernière ligne illisible est
        // alors un morceau coupé, pas une réponse d'un autre serveur.
        let mut understood = false;
        loop {
            let chunk = tokio::select! {
                biased;
                () = cancelled(cancel) => return Err(Outcome::Cancelled),
                chunk = response.chunk() => chunk,
            };
            let ended = matches!(chunk, Ok(None));
            let lines = match chunk {
                Ok(Some(bytes)) => buffer.push(&bytes),
                // Reste sans saut de ligne final : une ligne complète, ou une
                // ligne coupée par une connexion perdue.
                Ok(None) => buffer.finish().into_iter().collect(),
                Err(error) => {
                    return Err(Outcome::Failed(
                        ChatErrorKind::ConnectionLost,
                        connection_lost(Some(&error.without_url())),
                    ));
                }
            };
            for line in lines {
                match stream::parse_line(&line) {
                    Ok(Line::Fragment {
                        thinking,
                        content,
                        tool_calls,
                        done,
                    }) => {
                        understood = true;
                        if let Err(error) = self.take(reply, &thinking, &content).await {
                            return Err(error.into());
                        }
                        calls.extend(tool_calls);
                        if done {
                            return Ok(calls);
                        }
                    }
                    Ok(Line::Error(message)) => {
                        return Err(Outcome::Failed(ChatErrorKind::Model, message));
                    }
                    // Dernière ligne illisible : coupée par la fin du flux
                    // (après d'autres lignes, ou début d'objet JSON).
                    Err(_) if ended && (understood || line.trim_start().starts_with('{')) => {
                        return Err(Outcome::Failed(
                            ChatErrorKind::ConnectionLost,
                            connection_lost(None),
                        ));
                    }
                    Err(message) => return Err(Outcome::Failed(ChatErrorKind::Model, message)),
                }
            }
            if ended {
                // Fin du flux sans « done » : serveur arrêté ou proxy coupé.
                return Err(Outcome::Failed(
                    ChatErrorKind::ConnectionLost,
                    connection_lost(None),
                ));
            }
            if let Err(error) = self.save_if_due(reply).await {
                return Err(error.into());
            }
        }
    }

    /// Message de la réponse du tour, créé en base (daté `now`) s'il n'existe
    /// pas encore : au premier fragment, ou pour un tour fait seulement
    /// d'appels d'outils.
    async fn start_reply(&self, reply: &mut ReplyWriter, now: i64) -> AppResult<i64> {
        if let Some(id) = reply.message_id {
            return Ok(id);
        }
        let (chat_id, model) = (reply.chat_id.clone(), reply.model.clone());
        let id = self
            .store
            .call(move |c| chats::create_reply(c, &chat_id, &model, now))
            .await?;
        reply.message_id = Some(id);
        reply.last_save = Instant::now();
        (self.emit)(ChatEvent::ReplyStarted {
            message_id: id,
            created_at: now,
        });
        Ok(id)
    }

    /// Ajoute un fragment ; la réponse est créée en base au premier.
    async fn take(&self, reply: &mut ReplyWriter, thinking: &str, content: &str) -> AppResult<()> {
        if thinking.is_empty() && content.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        let message_id = self.start_reply(reply, now).await?;
        if !thinking.is_empty() && reply.think_off && !reply.always_thinks_noted {
            // Gabarit qui ouvre toujours le raisonnement (LFM2.5...) : le bouton
            // Think ne sera plus proposé pour ce modèle. Un échec d'écriture
            // n'arrête pas la réponse.
            reply.always_thinks_noted = true;
            let model = reply.model.clone();
            let _ = self
                .store
                .call(move |c| model_prefs::set_always_thinks(c, &model))
                .await;
        }
        let state = &mut reply.reply;
        if !thinking.is_empty() {
            state.thinking_started_at.get_or_insert(now);
            state.thinking.push_str(thinking);
            (self.emit)(ChatEvent::Thinking {
                text: thinking.to_owned(),
            });
        }
        if !content.is_empty() {
            if state.thinking_started_at.is_some() {
                state.thinking_ended_at.get_or_insert(now);
            }
            state.content.push_str(content);
            (self.emit)(ChatEvent::Content {
                text: content.to_owned(),
            });
        }
        self.engine.update_live(&reply.chat_id, message_id, state);
        Ok(())
    }

    async fn save_if_due(&self, reply: &mut ReplyWriter) -> AppResult<()> {
        let Some(id) = reply.message_id else {
            return Ok(());
        };
        if reply.last_save.elapsed() < SAVE_INTERVAL {
            return Ok(());
        }
        reply.last_save = Instant::now();
        let state = reply.reply.clone();
        self.store
            .call(move |c| chats::save_reply(c, id, &state, false, now_ms()))
            .await
    }

    /// Dernière écriture, quelle que soit la fin : le raisonnement resté
    /// ouvert est clos.
    async fn finish(&self, reply: &mut ReplyWriter) -> AppResult<()> {
        let Some(id) = reply.message_id else {
            return Ok(());
        };
        let now = now_ms();
        if reply.reply.thinking_started_at.is_some() {
            reply.reply.thinking_ended_at.get_or_insert(now);
        }
        let state = reply.reply.clone();
        self.store
            .call(move |c| chats::save_reply(c, id, &state, true, now))
            .await
    }
}

/// Consigne vocale envoyée : celle des réglages, sinon (vide, ou base
/// illisible) celle par défaut.
async fn voice_instructions(store: &Store) -> String {
    match store
        .call(|connection| crate::store::voice_mode::get(connection))
        .await
    {
        Ok(settings) => settings.effective_instructions().to_owned(),
        Err(error) => {
            eprintln!("chatonlm: voice mode settings not read: {error}");
            crate::voice_mode::DEFAULT_INSTRUCTIONS.to_owned()
        }
    }
}

/// Place `system` en premier message de la requête.
fn with_system_message(body: &mut serde_json::Value, system: &str) {
    if let Some(messages) = body
        .get_mut("messages")
        .and_then(serde_json::Value::as_array_mut)
    {
        messages.insert(
            0,
            serde_json::json!({ "role": "system", "content": system }),
        );
    }
}

/// Compte relié au serveur, demandé seulement si son cloud n'est pas coupé
/// (`/api/status`, réponse locale) : coupé, `/api/me` interrogerait quand
/// même ollama.com, et le serveur refusera l'outil (403, donné au modèle).
async fn linked_unless_cloud_off(base: &str) -> Option<bool> {
    if account::cloud(base)
        .await
        .is_some_and(|cloud| cloud.disabled)
    {
        return None;
    }
    account::linked(base).await
}

/// Réponse de `/api/show` utile à l'envoi.
enum Shown {
    Found(ModelDetails),
    /// 404 « not found » : à télécharger.
    Missing,
    /// Autre échec (statut d'erreur, 404 d'un proxy, délai, réponse
    /// illisible) : envoi sans les détails.
    Unknown,
    /// Connexion impossible : rien à télécharger d'un serveur absent.
    Unreachable,
    Cancelled,
}

impl From<Result<ModelDetails, ShowError>> for Shown {
    fn from(result: Result<ModelDetails, ShowError>) -> Self {
        match result {
            Ok(details) => Shown::Found(details),
            Err(ShowError::NotFound(message)) if message.contains("not found") => Shown::Missing,
            Err(ShowError::Unreachable(_)) => Shown::Unreachable,
            Err(ShowError::NotFound(_) | ShowError::Other(_)) => Shown::Unknown,
        }
    }
}

/// `/api/show` ; l'annulation interrompt l'attente.
async fn show(base: &str, model: &str, cancel: &mut watch::Receiver<bool>) -> Shown {
    tokio::select! {
        biased;
        () = cancelled(cancel) => Shown::Cancelled,
        result = models::show(base, model) => result.into(),
    }
}

/// `think` envoyé : absent s'il n'est pas demandé ; retiré si les capacités
/// connues (liste non vide) ne comptent pas « thinking », ou si les valeurs
/// acceptées sont connues et ne le contiennent pas ; gardé sinon (détails
/// inconnus : un refus 400 le fera retirer).
pub(crate) fn accepted_think(
    think: Option<Think>,
    details: Option<&ModelDetails>,
) -> Option<Think> {
    let think = think?;
    let Some(details) = details else {
        return Some(think);
    };
    let capabilities = &details.capabilities;
    if !capabilities.is_empty() && !capabilities.iter().any(|c| c == "thinking") {
        return None;
    }
    match &details.thinking {
        Some(thinking) if !thinking.values.contains(&think) => None,
        _ => Some(think),
    }
}

/// Chargement nécessaire avant l'envoi : modèle absent de `/api/ps`, ou,
/// contexte réglé (> 0), chargé avec un autre contexte que celui qu'Ollama
/// retiendra (le réglage, ramené au maximum du modèle s'il est connu). Un
/// contexte que `/api/ps` ne dit pas (serveur ancien) compte comme différent :
/// le chargement est sans effet si les options sont les mêmes.
fn needs_loading(
    model: &str,
    listed: &[RunningModel],
    context_length: u32,
    max_context: Option<u32>,
) -> bool {
    let Some(running) = listed.iter().find(|m| running::same_model(&m.name, model)) else {
        return true;
    };
    if context_length == 0 {
        return false;
    }
    let expected = max_context.map_or(context_length, |max| context_length.min(max));
    running.context_length != Some(expected)
}

/// Images envoyées au modèle : oui, sauf si ses capacités connues (liste non
/// vide) ne comptent pas « vision ». Voir le contrat
/// `contract_images_and_vision`.
fn accepts_images(details: Option<&ModelDetails>) -> bool {
    details.is_none_or(|details| {
        details.capabilities.is_empty() || details.capabilities.iter().any(|c| c == "vision")
    })
}

/// Outils offerts seulement à un modèle dont les capacités connues comptent
/// « tools » (détails inconnus : pas d'outils).
fn accepts_tools(details: Option<&ModelDetails>) -> bool {
    details.is_some_and(|details| details.capabilities.iter().any(|c| c == "tools"))
}

/// Texte des messages de l'utilisateur, un par ligne (`userMessageText`).
fn user_text(turns: &[Turn]) -> String {
    let mut text = String::new();
    for turn in turns.iter().filter(|turn| turn.role == Role::User) {
        text.push_str(&turn.content);
        text.push('\n');
    }
    text
}

/// Erreur d'une connexion impossible, la même pour chaque requête.
fn unreachable(base: &str) -> Outcome {
    Outcome::Failed(
        ChatErrorKind::Unreachable,
        t!("ollama.unreachableAt", address = base),
    )
}

/// Envoie `body` à `<base><path>`, sans délai global (une réponse ou un
/// téléchargement peut durer longtemps) ; l'annulation interrompt aussi
/// l'attente de connexion. Erreur : fin déjà décidée.
async fn post(
    base: &str,
    path: &str,
    body: &serde_json::Value,
    cancel: &mut watch::Receiver<bool>,
) -> Result<reqwest::Response, Outcome> {
    let failed = |kind, message: String| Err(Outcome::Failed(kind, message));
    let Ok(url) = Url::parse(base) else {
        return failed(
            ChatErrorKind::Unreachable,
            format!("invalid address: {base}"),
        );
    };
    let client = match client::client(&url, None) {
        Ok(client) => client,
        Err(error) => return failed(ChatErrorKind::Internal, error.to_string()),
    };
    let request = client.post(format!("{base}{path}")).json(body).send();
    let response = tokio::select! {
        biased;
        () = cancelled(cancel) => return Err(Outcome::Cancelled),
        response = request => response,
    };
    match response {
        Ok(response) => Ok(response),
        Err(error) if error.is_connect() => Err(unreachable(base)),
        Err(error) => failed(
            ChatErrorKind::ConnectionLost,
            connection_lost(Some(&error.without_url())),
        ),
    }
}

/// Lit une réponse d'erreur (l'annulation interrompt la lecture) : texte à
/// montrer (« 404 Not Found: … ») et message d'Ollama seul.
async fn read_error(
    response: reqwest::Response,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(String, String), Outcome> {
    let status = response.status();
    let text = tokio::select! {
        biased;
        () = cancelled(cancel) => return Err(Outcome::Cancelled),
        text = response.text() => text.unwrap_or_default(),
    };
    let message = error_line(&text);
    Ok((status_error(status, &message), message))
}

/// Envoie la requête de chat.
async fn open(base: &str, body: &serde_json::Value, cancel: &mut watch::Receiver<bool>) -> Opened {
    let response = match post(base, "/api/chat", body, cancel).await {
        Ok(response) => response,
        Err(outcome) => return Opened::Ended(outcome),
    };
    let status = response.status();
    if status.is_success() {
        return Opened::Stream(response);
    }
    let (shown, message) = match read_error(response, cancel).await {
        Ok(error) => error,
        Err(outcome) => return Opened::Ended(outcome),
    };
    if status == reqwest::StatusCode::BAD_REQUEST && message.contains("does not support thinking") {
        return Opened::ThinkingRefused(shown);
    }
    Opened::Ended(Outcome::Failed(refused_kind(status), shown))
}

/// Fin de l'envoi décidée par le chargement : refus d'Ollama, classé comme
/// une erreur de `/api/chat` (même texte, même type) ; rien sinon (réussi,
/// ou requête impossible : journalisée, le chat dira l'erreur).
fn load_outcome(model: &str, loaded: Result<(), LoadFailure>) -> Option<Outcome> {
    match loaded {
        Ok(()) => None,
        Err(LoadFailure::Refused { status, message }) => Some(Outcome::Failed(
            refused_kind(status),
            status_error(status, &message),
        )),
        Err(LoadFailure::Failed(error)) => {
            eprintln!("chatonlm: cannot load {model}: {error}");
            None
        }
    }
}

/// Type d'erreur d'une réponse d'erreur d'Ollama (chat ou chargement) : 404
/// → modèle absent, sinon erreur du modèle.
fn refused_kind(status: reqwest::StatusCode) -> ChatErrorKind {
    if status == reqwest::StatusCode::NOT_FOUND {
        ChatErrorKind::ModelMissing
    } else {
        ChatErrorKind::Model
    }
}

/// Message d'une réponse d'erreur, lu comme l'officiel (`api/client.go`,
/// `stream`) : la première ligne du corps ; si c'est un objet JSON, son champ
/// `error` (vide s'il manque), sinon la ligne elle-même (page d'un proxy),
/// limitée à 200 caractères.
pub(crate) fn error_line(body: &str) -> String {
    let line = body
        .lines()
        .next()
        .unwrap_or_default()
        .trim_end_matches('\r');
    match serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(line) {
        Ok(object) => object
            .get("error")
            .and_then(|error| error.as_str())
            .unwrap_or_default()
            .to_owned(),
        Err(_) => line.chars().take(200).collect(),
    }
}

/// Texte d'une réponse d'erreur, comme l'officiel (`api.StatusError`) :
/// « 500 Internal Server Error: <message> », ou le statut seul sans message
/// (proxy qui répond 502 avec un corps vide).
fn status_error(status: reqwest::StatusCode, message: &str) -> String {
    let status = match status.canonical_reason() {
        Some(reason) => format!("{} {reason}", status.as_u16()),
        None => status.as_str().to_owned(),
    };
    if message.trim().is_empty() {
        status
    } else {
        format!("{status}: {message}")
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use fake_ollama::{FakeOllama, Mode};
    use serde_json::{Value, json};
    use tauri::ipc::InvokeResponseBody;

    use super::*;
    use crate::attachments::tests as attachment_tests;
    use crate::attachments::{StageResult, samples};

    /// Attente maximale d'un événement ou d'un état, sur le faux serveur.
    const WAIT: Duration = Duration::from_secs(5);

    /// État courant en retard sur la base (lecture entre une écriture et sa
    /// mise à jour) : appels, résultats et fin du raisonnement de la base
    /// gardés ; le texte vient toujours de l'état courant.
    #[test]
    fn overlay_keeps_what_the_database_already_has() {
        let engine = ChatEngine::default();
        let done = ToolCall {
            result: Some(r#"{"results":[]}"#.into()),
            ..ToolCall::new("web_search", r#"{"query":"a"}"#)
        };
        let pending = ToolCall::new("web_fetch", r#"{"url":"https://example.com"}"#);
        let saved = Message {
            id: 7,
            role: chats::Role::Assistant,
            content: "old".into(),
            thinking: "Thinking".into(),
            model: None,
            created_at: 0,
            updated_at: 0,
            thinking_started_at: Some(1),
            thinking_ended_at: Some(2),
            attachments: Vec::new(),
            tool_calls: vec![done.clone(), pending.clone()],
        };
        let mut chat = Chat {
            id: "chat".into(),
            title: String::new(),
            created_at: 0,
            updated_at: 0,
            messages: vec![saved],
        };
        let live = Reply {
            content: "new".into(),
            thinking: "Thinking".into(),
            thinking_started_at: Some(1),
            thinking_ended_at: None,
            tool_calls: vec![ToolCall::new("web_search", r#"{"query":"a"}"#)],
        };
        engine.update_live("chat", 7, &live);
        engine.overlay(&mut chat);
        let message = &chat.messages[0];
        assert_eq!(message.content, "new");
        assert_eq!(message.thinking_ended_at, Some(2));
        assert_eq!(message.tool_calls, vec![done, pending]);
    }

    /// Événements reçus par le canal, en JSON, avec leur heure de réception
    /// (ms depuis l'époque, comme les horodatages en base).
    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<(i64, Value)>>>);

    fn recorder() -> (Channel<ChatEvent>, Recorder) {
        let recorder = Recorder::default();
        let sink = recorder.0.clone();
        let channel = Channel::new(move |body| {
            if let InvokeResponseBody::Json(text) = body {
                sink.lock()
                    .push((now_ms(), serde_json::from_str(&text).unwrap()));
            }
            Ok(())
        });
        (channel, recorder)
    }

    impl Recorder {
        fn events(&self) -> Vec<Value> {
            self.0
                .lock()
                .iter()
                .map(|(_, event)| event.clone())
                .collect()
        }

        fn count(&self, kind: &str) -> usize {
            self.0
                .lock()
                .iter()
                .filter(|(_, e)| e["type"] == kind)
                .count()
        }

        /// Heures de réception des événements de ce type.
        fn received(&self, kind: &str) -> Vec<i64> {
            let events = self.0.lock();
            let of_kind = events.iter().filter(|(_, e)| e["type"] == kind);
            of_kind.map(|(at, _)| *at).collect()
        }

        /// Texte de réponse reçu, bout à bout.
        fn contents(&self) -> String {
            let events = self.0.lock();
            let contents = events.iter().filter(|(_, e)| e["type"] == "content");
            contents
                .map(|(_, e)| e["text"].as_str().unwrap().to_owned())
                .collect()
        }

        /// Conversation créée par l'envoi.
        fn chat_id(&self) -> String {
            let events = self.0.lock();
            events[0].1["chatId"].as_str().unwrap().to_owned()
        }

        /// Attend au moins `count` événements de ce type, 5 s au plus.
        async fn wait_for(&self, kind: &str, count: usize) {
            let polling = async {
                while self.count(kind) < count {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            };
            if tokio::time::timeout(WAIT, polling).await.is_err() {
                let events = self.events();
                panic!("still waiting after {WAIT:?} for {count} \"{kind}\" event(s): {events:?}");
            }
        }
    }

    /// Attend qu'une condition soit vraie ; échoue au-delà de `limit`.
    async fn wait_until(limit: Duration, what: &str, ready: impl Fn() -> bool) {
        let polling = async {
            while !ready() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        if tokio::time::timeout(limit, polling).await.is_err() {
            panic!("still waiting after {limit:?} for {what}");
        }
    }

    /// Attend la fin d'un envoi lancé à part ; échoue au-delà de 5 s (un flux
    /// sans fin qu'une annulation n'arrête plus ferait sinon pendre les tests).
    async fn finished(task: tokio::task::JoinHandle<AppResult<()>>) {
        match tokio::time::timeout(WAIT, task).await {
            Ok(result) => result.unwrap().unwrap(),
            Err(_) => panic!("the send is still running after {WAIT:?}"),
        }
    }

    /// Canal qui, à l'événement final, lit la réponse en base (le frontend
    /// relit la conversation à ce moment-là) : renvoie aussi ce contenu.
    fn recorder_reading(
        store: Arc<Store>,
    ) -> (Channel<ChatEvent>, Recorder, Arc<Mutex<Option<String>>>) {
        let recorder = Recorder::default();
        let at_final = Arc::new(Mutex::new(None));
        let (sink, seen) = (recorder.0.clone(), at_final.clone());
        let channel = Channel::new(move |body| {
            if let InvokeResponseBody::Json(text) = body {
                let event: Value = serde_json::from_str(&text).unwrap();
                let mut events = sink.lock();
                if is_final(&event) {
                    let chat_id = events[0].1["chatId"].as_str().unwrap().to_owned();
                    let chat = store.blocking(move |c| chats::get(c, &chat_id)).unwrap();
                    *seen.lock() = chat.messages.get(1).map(|m| m.content.clone());
                }
                events.push((now_ms(), event));
            }
            Ok(())
        });
        (channel, recorder, at_final)
    }

    fn is_final(event: &Value) -> bool {
        matches!(event["type"].as_str(), Some("done" | "error" | "cancelled"))
    }

    /// Exactement un événement final, en dernier (compté sur la liste
    /// complète, sans le regroupement de `types`).
    fn assert_one_final(events: &[Value]) {
        let finals = events.iter().filter(|event| is_final(event)).count();
        assert_eq!(finals, 1, "{events:?}");
        assert!(events.last().is_some_and(is_final), "{events:?}");
    }

    fn types(events: &[Value]) -> Vec<String> {
        let mut types: Vec<String> = events
            .iter()
            .map(|event| event["type"].as_str().unwrap().to_owned())
            .collect();
        types.dedup();
        types
    }

    fn request(chat_id: Option<&str>, model: &str, think: Option<Think>) -> SendRequest {
        SendRequest {
            chat_id: chat_id.map(str::to_owned),
            stream_id: format!("stream-{model}"),
            content: "Say hello in one word.".into(),
            model: model.into(),
            think,
            attachment_ids: None,
            edit_message_id: None,
            kept_attachment_ids: None,
            web_search: None,
            voice: None,
        }
    }

    fn server(fake: &FakeOllama) -> Server {
        Server {
            url: fake.url(),
            context_length: 0,
            keep_alive: 0,
        }
    }

    /// Conversation telle qu'enregistrée (sans l'état courant d'une réponse
    /// en cours, que seul `overlay` ajoute).
    fn saved(store: &Store, chat_id: &str) -> Chat {
        let id = chat_id.to_owned();
        store.blocking(move |c| chats::get(c, &id)).unwrap()
    }

    #[tokio::test]
    async fn the_observer_sees_every_event_of_a_reply() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (store, engine) = (Store::open_in_memory().unwrap(), ChatEngine::default());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let chats = Arc::new(Mutex::new(Vec::new()));
        let (sink, chat_sink) = (seen.clone(), chats.clone());
        engine.set_observer(move |chat, event, _| {
            sink.lock()
                .push(serde_json::to_value(event).unwrap()["type"].clone());
            chat_sink.lock().push(chat.to_owned());
        });
        let (channel, recorder) = recorder();
        send(
            &store,
            &engine,
            &Staging::default(),
            server(&fake),
            request(None, "qwen3:0.6b", Some(Think::Enabled(true))),
            &channel,
        )
        .await
        .unwrap();

        let sent: Vec<Value> = recorder
            .events()
            .iter()
            .map(|event| event["type"].clone())
            .collect();
        assert!(sent.contains(&json!("content")));
        assert_eq!(*seen.lock(), sent);
        // Chaque événement avec sa conversation.
        let chat = recorder.chat_id();
        assert!(chats.lock().iter().all(|seen| *seen == chat));
    }

    /// Envoi vocal : la consigne vocale en tête de chaque requête (tours
    /// d'outils compris), jamais enregistrée ; l'observateur reçoit la
    /// demande avec chaque événement.
    #[tokio::test]
    async fn a_voice_send_puts_the_voice_instructions_first() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_signed_in(true);
        let (store, engine) = (Store::open_in_memory().unwrap(), ChatEngine::default());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        engine.set_observer(move |_, _, voice| {
            sink.lock().push(voice.map(|voice| voice.language));
        });
        let spoken = VoiceSend {
            language: Some(VoiceLanguage::Fr),
        };
        let system = |content: &str| json!({ "role": "system", "content": content });
        let ask = SendRequest {
            voice: Some(spoken.clone()),
            ..searching(None, "chatonlm-search", "Latest Ollama release?")
        };
        let (channel, first) = recorder();
        let staging = Staging::default();
        send(&store, &engine, &staging, server(&fake), ask, &channel)
            .await
            .unwrap();
        assert_eq!(first.events().last().unwrap()["type"], json!("done"));
        let requests = fake.chat_requests();
        assert_eq!(requests.len(), 2);
        let default = crate::voice_mode::DEFAULT_INSTRUCTIONS;
        for sent in &requests {
            assert_eq!(sent["messages"][0], system(default));
        }
        // Puis l'historique : la question, l'appel et son résultat.
        assert_eq!(requests[1]["messages"].as_array().unwrap().len(), 4);
        assert!(
            seen.lock()
                .iter()
                .all(|language| *language == Some(Some(VoiceLanguage::Fr)))
        );

        // Consigne réglée ; l'historique relu n'a pas l'ancienne consigne.
        store
            .blocking(|connection| {
                let patch = crate::store::voice_mode::VoiceModeSettingsPatch {
                    instructions: Some("Answer like a pirate.".into()),
                    ..Default::default()
                };
                crate::store::voice_mode::update(connection, patch)
            })
            .unwrap();
        let chat_id = first.chat_id();
        let again = SendRequest {
            voice: Some(VoiceSend::default()),
            ..request(Some(&chat_id), "qwen3:0.6b", Some(Think::Enabled(false)))
        };
        let (channel, second) = recorder();
        send(&store, &engine, &staging, server(&fake), again, &channel)
            .await
            .unwrap();
        assert_one_final(&second.events());
        let messages = fake.chat_requests()[2]["messages"].clone();
        let messages = messages.as_array().unwrap();
        assert_eq!(messages[0], system("Answer like a pirate."));
        assert_eq!(
            messages
                .iter()
                .filter(|message| message["role"] == "system")
                .count(),
            1
        );

        // Envoi ordinaire : aucune consigne ; rien d'enregistré.
        seen.lock().clear();
        let typed = request(Some(&chat_id), "qwen3:0.6b", Some(Think::Enabled(false)));
        let (channel, third) = recorder();
        send(&store, &engine, &staging, server(&fake), typed, &channel)
            .await
            .unwrap();
        assert_one_final(&third.events());
        let messages = fake.chat_requests()[3]["messages"].clone();
        assert!(
            messages
                .as_array()
                .unwrap()
                .iter()
                .all(|message| message["role"] != "system")
        );
        assert!(seen.lock().iter().all(Option::is_none));
        let chat = saved(&store, &chat_id);
        assert!(chat.messages.iter().all(|message| {
            !message.content.contains("pirate") && !message.content.contains("spoken conversation")
        }));
    }

    #[tokio::test]
    async fn answers_with_thinking_then_content() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (store, engine) = (Store::open_in_memory().unwrap(), ChatEngine::default());
        let (channel, recorder) = recorder();
        let think = Some(Think::Enabled(true));
        send(
            &store,
            &engine,
            &Staging::default(),
            server(&fake),
            request(None, "qwen3:0.6b", think),
            &channel,
        )
        .await
        .unwrap();

        let events = recorder.events();
        assert_one_final(&events);
        // Modèle pas encore en mémoire : chargé d'abord.
        assert_eq!(
            types(&events),
            [
                "chatCreated",
                "userMessage",
                "loading",
                "loaded",
                "replyStarted",
                "thinking",
                "content",
                "done"
            ]
        );
        let chat = saved(&store, &recorder.chat_id());
        assert_eq!(chat.messages.len(), 2);
        let reply = &chat.messages[1];
        assert_eq!(reply.content, "\"Hello\"");
        assert!(reply.thinking.starts_with("Okay, the user wants me"));
        assert_eq!(reply.model.as_deref(), Some("qwen3:0.6b"));
        let (started, ended) = (
            reply.thinking_started_at.unwrap(),
            reply.thinking_ended_at.unwrap(),
        );
        assert!(started <= ended);
        assert_eq!(events.last().unwrap()["messageId"], json!(reply.id));

        let sent = &fake.chat_requests()[0];
        assert_eq!(sent["think"], json!(true));
        assert_eq!(sent["stream"], json!(true));
        assert_eq!(
            sent["messages"],
            json!([{"role": "user", "content": "Say hello in one word."}])
        );
        assert!(sent.get("options").is_none());
    }

    /// Un modèle qui raisonne malgré `think: false` (gabarit qui ouvre toujours
    /// le raisonnement) est retenu ; pas celui qui obéit, ni un envoi avec
    /// `think: true`. Le raisonnement reçu est affiché comme d'habitude.
    #[tokio::test]
    async fn a_model_that_thinks_despite_think_false_is_remembered() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (store, engine) = (Store::open_in_memory().unwrap(), ChatEngine::default());
        let always = |model: &str| {
            store
                .blocking(|c| model_prefs::always_thinks(c, model))
                .unwrap()
        };
        for (model, think) in [
            ("chatonlm-stubborn", Think::Enabled(true)),
            ("chatonlm-nometa", Think::Enabled(false)),
        ] {
            let (channel, events) = recorder();
            send(
                &store,
                &engine,
                &Staging::default(),
                server(&fake),
                request(None, model, Some(think)),
                &channel,
            )
            .await
            .unwrap();
            assert_one_final(&events.events());
            assert!(!always(model), "{model}");
        }
        let (channel, events) = recorder();
        send(
            &store,
            &engine,
            &Staging::default(),
            server(&fake),
            request(None, "chatonlm-stubborn", Some(Think::Enabled(false))),
            &channel,
        )
        .await
        .unwrap();
        let events = events.events();
        assert_one_final(&events);
        assert!(events.iter().any(|event| event["type"] == "thinking"));
        assert!(always("chatonlm-stubborn"));
    }

    #[tokio::test]
    async fn history_and_context_length_are_sent() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (store, engine) = (Store::open_in_memory().unwrap(), ChatEngine::default());
        let (channel, first) = recorder();
        let think = Some(Think::Enabled(true));
        send(
            &store,
            &engine,
            &Staging::default(),
            server(&fake),
            request(None, "qwen3:0.6b", think),
            &channel,
        )
        .await
        .unwrap();
        assert_one_final(&first.events());
        let chat_id = first.chat_id();
        let with_context = Server {
            url: fake.url(),
            context_length: 8192,
            keep_alive: 0,
        };
        let (channel, second) = recorder();
        let again = request(Some(&chat_id), "qwen3:0.6b", Some(Think::Enabled(false)));
        send(
            &store,
            &engine,
            &Staging::default(),
            with_context,
            again,
            &channel,
        )
        .await
        .unwrap();
        assert_one_final(&second.events());

        let sent = &fake.chat_requests()[1];
        assert_eq!(sent["options"], json!({"num_ctx": 8192}));
        assert_eq!(sent["think"], json!(false));
        let messages = sent["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1]["role"], json!("assistant"));
        assert_eq!(messages[1]["content"], json!("\"Hello\""));
        assert!(
            messages[1]["thinking"]
                .as_str()
                .unwrap()
                .starts_with("Okay")
        );
    }

    #[tokio::test]
    async fn the_message_is_kept_as_typed() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (store, engine) = (Store::open_in_memory().unwrap(), ChatEngine::default());
        let (channel, recorder) = recorder();
        let typed = "  Say hello\n\nin one word.\n";
        let mut ask = request(None, "qwen3:0.6b", Some(Think::Enabled(false)));
        ask.content = typed.into();
        send(
            &store,
            &engine,
            &Staging::default(),
            server(&fake),
            ask,
            &channel,
        )
        .await
        .unwrap();

        let events = recorder.events();
        assert_one_final(&events);
        assert_eq!(events[1]["message"]["content"], json!(typed));
        assert_eq!(
            saved(&store, &recorder.chat_id()).messages[0].content,
            typed
        );
        let sent = &fake.chat_requests()[0];
        assert_eq!(sent["messages"][0]["content"], json!(typed));
    }

    /// Réglages de l'app → réglages de l'envoi.
    #[test]
    fn the_send_follows_the_settings() {
        let settings = Settings {
            server_url: String::new(),
            effective_server_url: "http://127.0.0.1:11435".into(),
            context_length: 8192,
            theme: crate::store::settings::Theme::System,
            selected_model: String::new(),
            window_width: None,
            window_height: None,
            web_search_enabled: false,
            keep_alive: 300,
            preload_model: false,
            language: crate::i18n::LanguageChoice::System,
            effective_language: crate::i18n::Lang::En,
            proofread_model: String::new(),
            proofread_shortcut: false,
            study_model: String::new(),
            auto_read: false,
        };
        let server = Server::from_settings(&settings);
        assert_eq!(
            (
                server.url.as_str(),
                server.context_length,
                server.keep_alive
            ),
            ("http://127.0.0.1:11435", 8192, 300)
        );
    }

    /// Envoi d'un message à `model`, dans `chat_id` (nouvelle conversation
    /// sinon), avec ce contexte et cette durée en mémoire : événements reçus.
    async fn send_with(
        store: &Store,
        fake: &FakeOllama,
        chat_id: Option<&str>,
        model: &str,
        (context_length, keep_alive): (u32, i64),
    ) -> Recorder {
        let (channel, recorder) = recorder();
        let server = Server {
            url: fake.url(),
            context_length,
            keep_alive,
        };
        let (engine, staging) = (ChatEngine::default(), Staging::default());
        let ask = request(chat_id, model, None);
        let sending = send(store, &engine, &staging, server, ask, &channel);
        tokio::time::timeout(WAIT, sending)
            .await
            .unwrap_or_else(|_| panic!("still sending after {WAIT:?}: {:?}", recorder.events()))
            .unwrap();
        assert_one_final(&recorder.events());
        recorder
    }

    /// Chargement annoncé : avant le premier envoi (modèle absent de
    /// `/api/ps`), avec les options du chat ; rien quand il est chargé avec
    /// ce contexte, ou sans contexte réglé ; de nouveau pour un autre
    /// contexte.
    #[tokio::test]
    async fn a_model_not_in_memory_is_loaded_before_the_answer() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Store::open_in_memory().unwrap();
        let model = "chatonlm-slowload";
        let first = send_with(&store, &fake, None, model, (8192, 300)).await;
        let events = first.events();
        assert_eq!(
            types(&events),
            [
                "chatCreated",
                "userMessage",
                "loading",
                "loaded",
                "replyStarted",
                "thinking",
                "content",
                "done"
            ]
        );
        // `loaded` à la fin du chargement (1,5 s pour ce modèle).
        let waited = first.received("loaded")[0] - first.received("loading")[0];
        assert!(waited >= 1400, "{waited} ms");
        assert_eq!(
            fake.load_requests(),
            [json!({ "model": model, "kind": "load", "keep_alive": 300, "num_ctx": 8192 })]
        );
        let sent = &fake.chat_requests()[0];
        assert_eq!(sent["options"], json!({ "num_ctx": 8192 }));
        assert_eq!(sent["keep_alive"], json!(300));

        let chat_id = first.chat_id();
        let chat_id = Some(chat_id.as_str());
        // Chargé avec ce contexte, ou contexte du serveur : rien d'annoncé.
        for settings in [(8192, 300), (0, 300)] {
            let again = send_with(&store, &fake, chat_id, model, settings).await;
            assert_eq!(
                types(&again.events()),
                ["userMessage", "replyStarted", "thinking", "content", "done"],
                "{settings:?}"
            );
        }
        assert_eq!(fake.load_requests().len(), 1);
        // Autre contexte : rechargé.
        let other = send_with(&store, &fake, chat_id, model, (16384, 0)).await;
        assert_eq!(
            types(&other.events())[..3],
            ["userMessage", "loading", "loaded"]
        );
        assert_eq!(
            fake.load_requests()[1],
            json!({ "model": model, "kind": "load", "num_ctx": 16384 })
        );
    }

    /// `keep_alive` envoyé avec le chat et le chargement seulement s'il est
    /// réglé : absent à 0 (défaut du serveur), en secondes, -1 tel quel.
    #[tokio::test]
    async fn keep_alive_goes_with_the_load_and_the_chat() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Store::open_in_memory().unwrap();
        for (index, (model, keep_alive)) in [
            ("qwen3:0.6b", 0),
            ("chatonlm-plain", 300),
            ("qwen3.5:0.8b", -1),
        ]
        .into_iter()
        .enumerate()
        {
            send_with(&store, &fake, None, model, (0, keep_alive)).await;
            let expected = (keep_alive != 0).then(|| json!(keep_alive));
            let chat = &fake.chat_requests()[index];
            assert_eq!(chat.get("keep_alive"), expected.as_ref(), "{model}");
            let load = &fake.load_requests()[index];
            assert_eq!(load["model"], json!(model));
            assert_eq!(load.get("keep_alive"), expected.as_ref(), "{model}");
            // Contexte du serveur : pas de num_ctx.
            assert!(chat.get("options").is_none() && load.get("num_ctx").is_none());
        }
    }

    /// Contexte réglé au-delà de celui du modèle : Ollama le ramène au
    /// maximum du modèle (qwen3:0.6b : 40960), le message suivant ne le
    /// recharge pas. Maximum inconnu (`/api/show` en échec) : comparé au
    /// réglage.
    #[tokio::test]
    async fn a_context_beyond_the_model_is_not_a_reload() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Store::open_in_memory().unwrap();
        for model in ["qwen3:0.6b", "chatonlm-nothink"] {
            let first = send_with(&store, &fake, None, model, (65536, 0)).await;
            assert_eq!(first.count("loading"), 1, "{model}");
            let chat_id = first.chat_id();
            let again = send_with(&store, &fake, Some(&chat_id), model, (65536, 0)).await;
            assert_eq!(again.count("loading"), 0, "{model}");
        }
        let listed = running::list(&fake.url()).await.unwrap();
        let contexts: Vec<_> = listed
            .iter()
            .map(|m| (m.name.as_str(), m.context_length))
            .collect();
        assert_eq!(
            contexts,
            [
                ("chatonlm-nothink:latest", Some(65536)),
                ("qwen3:0.6b", Some(40960))
            ]
        );
    }

    /// Modèle distant (`remote_host` dans `/api/show`) : rien à charger ici.
    #[tokio::test]
    async fn a_remote_model_is_not_loaded() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Store::open_in_memory().unwrap();
        let sent = send_with(&store, &fake, None, "chatonlm-polish-remote", (0, 300)).await;
        let events = sent.events();
        assert_eq!(
            types(&events),
            [
                "chatCreated",
                "userMessage",
                "replyStarted",
                "content",
                "done"
            ]
        );
        assert!(fake.load_requests().is_empty());
    }

    /// Arrêt pendant le chargement : `cancelled`, sans `loaded` ni réponse ;
    /// le serveur voit le chargement abandonné.
    #[tokio::test]
    async fn stopping_during_the_load() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(ChatEngine::default());
        let (channel, recorder) = recorder();
        let task = {
            let (store, engine, server) = (store.clone(), engine.clone(), server(&fake));
            let request = request(None, "chatonlm-slowload", None);
            tokio::spawn(async move {
                send(
                    &store,
                    &engine,
                    &Staging::default(),
                    server,
                    request,
                    &channel,
                )
                .await
            })
        };
        recorder.wait_for("loading", 1).await;
        wait_until(WAIT, "the load request", || fake.load_requests().len() == 1).await;
        let asked = Instant::now();
        engine.cancel("stream-chatonlm-slowload");
        finished(task).await;
        assert!(asked.elapsed() < Duration::from_millis(500));
        let events = recorder.events();
        assert_one_final(&events);
        assert_eq!(
            types(&events),
            ["chatCreated", "userMessage", "loading", "cancelled"]
        );
        assert_eq!(saved(&store, &recorder.chat_id()).messages.len(), 1);
        assert!(fake.chat_requests().is_empty());
        wait_until(WAIT, "the server to see the load abandoned", || {
            fake.disconnects() == 1
        })
        .await;
    }

    /// Refus d'Ollama au chargement (mémoire insuffisante, modèle absent) :
    /// `loaded`, puis l'erreur que `/api/chat` aurait donnée, sans le lui
    /// demander.
    #[tokio::test]
    async fn a_refused_load_ends_the_send() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Store::open_in_memory().unwrap();
        for (model, kind, message) in [
            (
                "chatonlm-loadfail",
                "model",
                format!("500 Internal Server Error: {}", fake_ollama::LOAD_FAILURE),
            ),
            (
                "chatonlm-vanished",
                "modelMissing",
                "404 Not Found: model 'chatonlm-vanished' not found".to_owned(),
            ),
        ] {
            let sent = send_with(&store, &fake, None, model, (0, 0)).await;
            let events = sent.events();
            assert_eq!(
                types(&events),
                ["chatCreated", "userMessage", "loading", "loaded", "error"],
                "{model}"
            );
            let last = events.last().unwrap();
            assert_eq!(last["kind"], json!(kind), "{model}");
            assert_eq!(last["message"], json!(message), "{model}");
            assert_eq!(saved(&store, &sent.chat_id()).messages.len(), 1, "{model}");
        }
        assert_eq!(fake.load_requests().len(), 2);
        assert!(fake.chat_requests().is_empty());
    }

    /// Chargement impossible (connexion, lecture) : journalisé, le chat part
    /// et dira l'erreur ; un refus finit l'envoi, classé comme par `open`.
    #[test]
    fn only_a_refused_load_ends_the_send() {
        assert!(load_outcome("m", Ok(())).is_none());
        let lost = AppError::Ollama("can't connect to Ollama".into());
        assert!(load_outcome("m", Err(LoadFailure::Failed(lost))).is_none());
        for (status, message, kind, shown) in [
            (
                reqwest::StatusCode::NOT_FOUND,
                "model 'm' not found",
                ChatErrorKind::ModelMissing,
                "404 Not Found: model 'm' not found",
            ),
            (
                reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                "out of memory",
                ChatErrorKind::Model,
                "500 Internal Server Error: out of memory",
            ),
            // Proxy sans corps : le statut seul.
            (
                reqwest::StatusCode::BAD_GATEWAY,
                "",
                ChatErrorKind::Model,
                "502 Bad Gateway",
            ),
        ] {
            let refused = LoadFailure::Refused {
                status,
                message: message.into(),
            };
            let Some(Outcome::Failed(found, text)) = load_outcome("m", Err(refused)) else {
                panic!("{status}: the send goes on");
            };
            assert_eq!((found, text.as_str()), (kind, shown), "{status}");
        }
    }

    #[test]
    fn http_errors_read_like_the_official_app() {
        use reqwest::StatusCode;
        let memory = "model requires more system memory (5.2 GiB) than is available (3.1 GiB)";
        assert_eq!(
            status_error(StatusCode::INTERNAL_SERVER_ERROR, memory),
            format!("500 Internal Server Error: {memory}")
        );
        assert_eq!(status_error(StatusCode::BAD_GATEWAY, ""), "502 Bad Gateway");
        assert_eq!(
            status_error(StatusCode::BAD_GATEWAY, " \n"),
            "502 Bad Gateway"
        );
        let unknown = StatusCode::from_u16(599).unwrap();
        assert_eq!(status_error(unknown, "busy"), "599: busy");

        // Message lu comme l'officiel : première ligne, champ « error » d'un
        // objet JSON (vide s'il manque), sinon la ligne telle quelle.
        assert_eq!(error_line(&format!("{{\"error\":\"{memory}\"}}")), memory);
        assert_eq!(error_line("{}"), "");
        assert_eq!(error_line(""), "");
        let nginx = "<html>\r\n<head><title>502 Bad Gateway</title></head>\r\n</html>\r\n";
        assert_eq!(
            status_error(StatusCode::BAD_GATEWAY, &error_line(nginx)),
            "502 Bad Gateway: <html>"
        );
        assert_eq!(error_line(&"x".repeat(500)).len(), 200);
    }

    #[tokio::test]
    async fn a_page_from_another_server_is_not_a_lost_connection() {
        // 200 avec une page HTML d'une ligne, sans saut de ligne final : la
        // ligne est illisible mais ce n'est pas un morceau de réponse coupé.
        let fake = FakeOllama::start(Mode::NotOllama).await.unwrap();
        let (store, engine) = (Store::open_in_memory().unwrap(), ChatEngine::default());
        let (channel, recorder) = recorder();
        let ask = request(None, "qwen3:0.6b", None);
        send(
            &store,
            &engine,
            &Staging::default(),
            server(&fake),
            ask,
            &channel,
        )
        .await
        .unwrap();
        let events = recorder.events();
        assert_one_final(&events);
        let last = events.last().unwrap();
        assert_eq!(last["kind"], json!("model"), "{last}");
        assert!(
            last["message"]
                .as_str()
                .unwrap()
                .starts_with("unexpected answer from Ollama"),
            "{last}"
        );
    }

    #[tokio::test]
    async fn failures_are_told_apart_and_partial_answers_kept() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let proxy = FakeOllama::start(Mode::Status(502)).await.unwrap();
        let empty_404 = FakeOllama::start(Mode::Status(404)).await.unwrap();
        let store = Store::open_in_memory().unwrap();
        let engine = ChatEngine::default();
        // Serveur, modèle, type d'erreur, message (entier ou début), réponse
        // partielle attendue.
        for (url, model, kind, (message, whole), partial) in [
            // Présent pour /api/show, absent au chargement (404 de
            // /api/generate, classé comme celle de /api/chat).
            (
                fake.url(),
                "chatonlm-vanished",
                "modelMissing",
                ("404 Not Found: model 'chatonlm-vanished' not found", true),
                false,
            ),
            // 404 sans « not found » (proxy) : pas de téléchargement.
            (
                empty_404.url(),
                "qwen3:0.6b",
                "modelMissing",
                ("404 Not Found", true),
                false,
            ),
            // Corps vide (proxy) : le statut seul.
            (
                proxy.url(),
                "qwen3:0.6b",
                "model",
                ("502 Bad Gateway", true),
                false,
            ),
            // Fin du corps sans « done ».
            (
                fake.url(),
                "chatonlm-cut",
                "connectionLost",
                ("Connection lost", true),
                true,
            ),
            // Fin du corps au milieu d'une ligne.
            (
                fake.url(),
                "chatonlm-truncated",
                "connectionLost",
                ("Connection lost", true),
                true,
            ),
            // Connexion coupée : erreur de lecture du corps.
            (
                fake.url(),
                "chatonlm-reset",
                "connectionLost",
                ("Connection lost: ", false),
                true,
            ),
            (
                fake.url(),
                "chatonlm-error",
                "model",
                ("model runner has unexpectedly stopped", true),
                true,
            ),
        ] {
            let (channel, recorder) = recorder();
            let server = Server {
                url,
                context_length: 0,
                keep_alive: 0,
            };
            send(
                &store,
                &engine,
                &Staging::default(),
                server,
                request(None, model, None),
                &channel,
            )
            .await
            .unwrap();
            let events = recorder.events();
            assert_one_final(&events);
            let last = events.last().unwrap();
            assert_eq!(last["type"], json!("error"), "{model}");
            assert_eq!(last["kind"], json!(kind), "{model}");
            let text = last["message"].as_str().unwrap();
            if whole {
                assert_eq!(text, message, "{model}");
            } else {
                assert!(text.starts_with(message), "{model}: {text}");
            }
            let chat = saved(&store, &recorder.chat_id());
            // Le message de l'utilisateur reste ; une réponse partielle aussi.
            assert_eq!(chat.messages.len(), 1 + usize::from(partial), "{model}");
            if partial {
                assert!(!recorder.contents().is_empty(), "{model}");
                assert_eq!(chat.messages[1].content, recorder.contents(), "{model}");
            }
        }
        // Chaque fin venait du serveur : aucun flux abandonné par le client.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(fake.disconnects(), 0);
        assert!(fake.pull_requests().is_empty());
    }

    /// Événements de téléchargement reçus, en (total, completed, done).
    fn downloads(events: &[Value]) -> Vec<(u64, u64, bool)> {
        let downloads = events.iter().filter(|event| event["type"] == "download");
        downloads
            .map(|event| {
                (
                    event["total"].as_u64().unwrap(),
                    event["completed"].as_u64().unwrap(),
                    event["done"].as_bool().unwrap(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn a_missing_model_is_downloaded_then_answers() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (store, engine) = (Store::open_in_memory().unwrap(), ChatEngine::default());
        let (channel, first) = recorder();
        // Raisonnement demandé : retiré d'après les détails du modèle
        // téléchargé (capacité completion seule).
        let think = Some(Think::Enabled(true));
        send(
            &store,
            &engine,
            &Staging::default(),
            server(&fake),
            request(None, "chatonlm-pull", think),
            &channel,
        )
        .await
        .unwrap();

        let events = first.events();
        assert_one_final(&events);
        assert_eq!(
            types(&events),
            [
                "chatCreated",
                "userMessage",
                "download",
                "loading",
                "loaded",
                "replyStarted",
                "content",
                "done"
            ]
        );
        // Plus grosse couche seulement (la petite est ignorée), de 0 à
        // complète, puis la fin.
        let total = fake_ollama::PULL_TOTAL;
        let expected: Vec<_> = (0..=10)
            .map(|step| (total, total * step / 10, false))
            .chain([(total, total, true)])
            .collect();
        assert_eq!(downloads(&events), expected);
        assert_eq!(
            fake.pull_requests(),
            [json!({"model": "chatonlm-pull", "stream": true})]
        );
        let sent = fake.chat_requests();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].get("think").is_none(), "{}", sent[0]);
        let chat = saved(&store, &first.chat_id());
        assert_eq!(chat.messages.len(), 2);
        assert_eq!(chat.messages[1].content, "\"Hi!\"");
        assert_eq!(chat.messages[1].model.as_deref(), Some("chatonlm-pull"));
        assert_eq!(
            events.last().unwrap()["messageId"],
            json!(chat.messages[1].id)
        );

        // Installé sur ce serveur : en tête de la liste, sans téléchargement
        // au message suivant.
        let installed = models::list(&fake.url()).await.unwrap();
        let names: Vec<_> = installed.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(names, ["chatonlm-pull", "qwen3:0.6b", "qwen3.5:0.8b"]);
        assert_eq!(installed[0].size, total);
        assert_eq!(installed[0].capabilities, ["completion"]);
        let (channel, again) = recorder();
        let ask = request(Some(&first.chat_id()), "chatonlm-pull", None);
        send(
            &store,
            &engine,
            &Staging::default(),
            server(&fake),
            ask,
            &channel,
        )
        .await
        .unwrap();
        let events = again.events();
        assert_eq!(
            types(&events),
            ["userMessage", "replyStarted", "content", "done"]
        );
        assert_eq!(fake.pull_requests().len(), 1);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(fake.disconnects(), 0);
    }

    #[tokio::test]
    async fn download_failures_are_told_apart() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Store::open_in_memory().unwrap();
        let engine = ChatEngine::default();
        let missing = "pull model manifest: file does not exist";
        // Modèle, type d'erreur, message (entier ou début), événements de
        // téléchargement reçus avant.
        for (model, kind, (message, whole), progress) in [
            (
                "chatonlm-pull-offline",
                "download",
                (
                    "Unable to download model. Please check your internet connection to download the model for offline use.",
                    true,
                ),
                0,
            ),
            ("chatonlm-pull-error", "download", (missing, true), 0),
            // Nom inconnu du registre, comme Ollama.
            ("chatonlm-unknown-xyz", "download", (missing, true), 0),
            (
                "chatonlm-pull-refused",
                "download",
                ("400 Bad Request: invalid model name", true),
                0,
            ),
            // Fin du corps sans « success ».
            (
                "chatonlm-pull-cut",
                "connectionLost",
                ("Connection lost", true),
                3,
            ),
            // Connexion coupée : erreur de lecture du corps.
            (
                "chatonlm-pull-reset",
                "connectionLost",
                ("Connection lost: ", false),
                3,
            ),
        ] {
            let (channel, recorder) = recorder();
            send(
                &store,
                &engine,
                &Staging::default(),
                server(&fake),
                request(None, model, None),
                &channel,
            )
            .await
            .unwrap();
            let events = recorder.events();
            assert_one_final(&events);
            let last = events.last().unwrap();
            assert_eq!(last["type"], json!("error"), "{model}");
            assert_eq!(last["kind"], json!(kind), "{model}");
            let text = last["message"].as_str().unwrap();
            if whole {
                assert_eq!(text, message, "{model}");
            } else {
                assert!(text.starts_with(message), "{model}: {text}");
            }
            let received = downloads(&events);
            assert_eq!(received.len(), progress, "{model}");
            assert!(received.iter().all(|(_, _, done)| !done), "{model}");
            // Le message de l'utilisateur seul : aucune réponse vide.
            assert_eq!(
                saved(&store, &recorder.chat_id()).messages.len(),
                1,
                "{model}"
            );
        }
        assert_eq!(fake.pull_requests().len(), 6);
        // Jamais de chat après un téléchargement manqué.
        assert!(fake.chat_requests().is_empty());
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(fake.disconnects(), 0);
    }

    #[tokio::test]
    async fn cancelling_while_preparing_or_downloading() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        // /api/show sans réponse.
        let silent = FakeOllama::start(Mode::Silent).await.unwrap();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(ChatEngine::default());

        // /api/show en attente, requête de téléchargement sans réponse, puis
        // en plein téléchargement.
        for (index, (url, model, progress)) in [
            (silent.url(), "qwen3:0.6b", 0),
            (fake.url(), "chatonlm-pull-wait", 0),
            (fake.url(), "chatonlm-pull-stall", 3),
        ]
        .into_iter()
        .enumerate()
        {
            let (channel, recorder) = recorder();
            let task = {
                let (store, engine) = (store.clone(), engine.clone());
                let server = Server {
                    url,
                    context_length: 0,
                    keep_alive: 0,
                };
                let request = request(None, model, None);
                tokio::spawn(async move {
                    send(
                        &store,
                        &engine,
                        &Staging::default(),
                        server,
                        request,
                        &channel,
                    )
                    .await
                })
            };
            match (index, progress) {
                (0, _) => {
                    recorder.wait_for("userMessage", 1).await;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                (_, 0) => {
                    let what = format!("the download of {model}");
                    wait_until(WAIT, &what, || fake.pull_requests().len() >= index).await;
                }
                _ => recorder.wait_for("download", progress).await,
            }
            let asked = Instant::now();
            engine.cancel(&format!("stream-{model}"));
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap_or_else(|_| panic!("{model}: still running 1 s after the cancel"))
                .unwrap()
                .unwrap();
            assert!(asked.elapsed() < Duration::from_secs(1), "{model}");
            let events = recorder.events();
            assert_one_final(&events);
            assert_eq!(
                events.last().unwrap()["type"],
                json!("cancelled"),
                "{model}"
            );
            let received = downloads(&events);
            assert_eq!(received.len(), progress, "{model}");
            assert!(received.iter().all(|(_, _, done)| !done), "{model}");
            // Aucune réponse en base.
            assert_eq!(
                saved(&store, &recorder.chat_id()).messages.len(),
                1,
                "{model}"
            );
            if index > 0 {
                // Le serveur a vu le téléchargement abandonné.
                let what = format!("the server to see {model} abandoned");
                wait_until(WAIT, &what, || fake.disconnects() >= index).await;
                assert_eq!(fake.disconnects(), index, "{model}");
            }
        }
        assert!(fake.chat_requests().is_empty());
        // Pas installé : un nouvel envoi le télécharge encore.
        let (channel, _) = recorder();
        let ask = request(None, "chatonlm-pull-stall", None);
        let task = {
            let (store, engine, server) = (store.clone(), engine.clone(), server(&fake));
            tokio::spawn(async move {
                send(&store, &engine, &Staging::default(), server, ask, &channel).await
            })
        };
        wait_until(WAIT, "a second download", || {
            fake.pull_requests().len() == 3
        })
        .await;
        engine.cancel("stream-chatonlm-pull-stall");
        finished(task).await;
    }

    /// Serveur injoignable : jamais de téléchargement (décision 6 : serveur
    /// absent n'est pas modèle absent).
    #[test]
    fn an_unreachable_server_is_not_a_missing_model() {
        let shown = |result| -> Shown { Shown::from(result) };
        let unreachable = AppError::Ollama("can't connect to Ollama".into());
        assert!(matches!(
            shown(Err(ShowError::Unreachable(unreachable))),
            Shown::Unreachable
        ));
        let missing = "model 'llama3:latest' not found".to_owned();
        assert!(matches!(
            shown(Err(ShowError::NotFound(missing))),
            Shown::Missing
        ));
        // 404 d'un proxy (page vide ou HTML) : pas un modèle absent.
        for page in ["", "<html><title>404 Not Found</title>"] {
            assert!(matches!(
                shown(Err(ShowError::NotFound(page.into()))),
                Shown::Unknown
            ));
        }
        let other = AppError::Ollama("Ollama answered HTTP 500: boom".into());
        assert!(matches!(
            shown(Err(ShowError::Other(other))),
            Shown::Unknown
        ));
    }

    fn details(capabilities: &[&str], values: Option<Vec<Think>>) -> ModelDetails {
        ModelDetails {
            model: "m".into(),
            capabilities: capabilities.iter().map(|c| (*c).to_owned()).collect(),
            thinking: values.map(|values| models::ThinkingOptions {
                values,
                default: None,
            }),
            always_thinks: false,
            remote: false,
            max_context: None,
        }
    }

    #[test]
    fn think_is_kept_only_if_the_model_accepts_it() {
        let (on, off) = (Think::Enabled(true), Think::Enabled(false));
        let high = Think::Level("high".into());
        let thinking = &["completion", "thinking"][..];
        let levels = Some(vec![
            Think::Level("low".into()),
            Think::Level("medium".into()),
            high.clone(),
        ]);
        for (think, details, kept) in [
            // Rien de demandé : rien d'envoyé.
            (None, Some(details(thinking, None)), false),
            // Détails inconnus (/api/show en échec) : gardé.
            (Some(on.clone()), None, true),
            // Capacités inconnues (serveur ancien) : gardé.
            (Some(high.clone()), Some(details(&[], None)), true),
            // Capacités connues sans « thinking » : retiré, même false.
            (
                Some(off.clone()),
                Some(details(&["completion"], None)),
                false,
            ),
            (
                Some(high.clone()),
                Some(details(&["completion", "vision"], None)),
                false,
            ),
            // Capacité sans valeurs connues : gardé.
            (Some(high.clone()), Some(details(thinking, None)), true),
            // Valeurs connues : gardé seulement s'il y figure.
            (
                Some(high.clone()),
                Some(details(thinking, levels.clone())),
                true,
            ),
            (Some(on.clone()), Some(details(thinking, levels)), false),
            (
                Some(high.clone()),
                Some(details(thinking, Some(vec![off.clone(), on.clone()]))),
                false,
            ),
            (
                Some(off.clone()),
                Some(details(thinking, Some(vec![off.clone(), on.clone()]))),
                true,
            ),
            (
                Some(off.clone()),
                Some(details(thinking, Some(vec![on.clone()]))),
                false,
            ),
            // Valeurs connues mais capacités inconnues : les valeurs comptent.
            (Some(on), Some(details(&[], Some(vec![off]))), false),
        ] {
            let case = format!("{think:?} with {details:?}");
            let sent = accepted_think(think.clone(), details.as_ref());
            assert_eq!(sent, if kept { think } else { None }, "{case}");
        }
    }

    /// `think` relu dans les requêtes reçues par le faux serveur.
    #[tokio::test]
    async fn think_is_filtered_by_the_model_details() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Store::open_in_memory().unwrap();
        let engine = ChatEngine::default();
        let level = |name: &str| Some(Think::Level(name.into()));
        // Modèle, think demandé, think envoyé (None : absent).
        for (model, think, expected) in [
            // [false, true] : niveau retiré, booléen gardé.
            ("qwen3:0.6b", level("high"), None),
            (
                "qwen3:0.6b",
                Some(Think::Enabled(false)),
                Some(json!(false)),
            ),
            // Niveaux seulement.
            ("chatonlm-levels", Some(Think::Enabled(true)), None),
            ("chatonlm-levels", level("high"), Some(json!("high"))),
            (
                "chatonlm-levels-off",
                Some(Think::Enabled(false)),
                Some(json!(false)),
            ),
            // [true] seulement.
            ("chatonlm-always", Some(Think::Enabled(false)), None),
            // Capacité thinking sans valeurs connues : gardé.
            ("chatonlm-nometa", level("high"), Some(json!("high"))),
            // Sans capacité thinking : retiré, une seule requête (pas de 400).
            ("chatonlm-plain", level("high"), None),
            ("chatonlm-plain", Some(Think::Enabled(true)), None),
        ] {
            let before = fake.chat_requests().len();
            let (channel, recorder) = recorder();
            let ask = request(None, model, think.clone());
            send(
                &store,
                &engine,
                &Staging::default(),
                server(&fake),
                ask,
                &channel,
            )
            .await
            .unwrap();
            let events = recorder.events();
            assert_one_final(&events);
            let case = format!("{model} with {think:?}");
            assert_eq!(events.last().unwrap()["type"], json!("done"), "{case}");
            let sent = fake.chat_requests();
            assert_eq!(sent.len(), before + 1, "{case}");
            assert_eq!(sent[before].get("think"), expected.as_ref(), "{case}");
        }
        assert!(fake.pull_requests().is_empty());
    }

    #[tokio::test]
    async fn a_model_without_thinking_is_asked_again_without_think() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (store, engine) = (Store::open_in_memory().unwrap(), ChatEngine::default());
        let (channel, recorder) = recorder();
        let think = Some(Think::Level("high".into()));
        send(
            &store,
            &engine,
            &Staging::default(),
            server(&fake),
            request(None, "chatonlm-nothink", think),
            &channel,
        )
        .await
        .unwrap();
        let events = recorder.events();
        assert_one_final(&events);
        assert_eq!(events.last().unwrap()["type"], json!("done"));
        let sent = fake.chat_requests();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0]["think"], json!("high"));
        assert!(sent[1].get("think").is_none());
    }

    #[tokio::test]
    async fn an_unreachable_server_is_reported() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let gone = server(&fake);
        let url = gone.url.clone();
        fake.stop().await;
        let (store, engine) = (Store::open_in_memory().unwrap(), ChatEngine::default());
        let (channel, recorder) = recorder();
        send(
            &store,
            &engine,
            &Staging::default(),
            gone,
            request(None, "qwen3:0.6b", None),
            &channel,
        )
        .await
        .unwrap();
        let events = recorder.events();
        assert_one_final(&events);
        assert_eq!(
            types(&events),
            ["chatCreated", "userMessage", "error"],
            "{events:?}"
        );
        let last = events.last().unwrap();
        assert_eq!(last["kind"], json!("unreachable"));
        assert_eq!(
            last["message"],
            json!(format!("can't connect to Ollama at {url}"))
        );
        assert_eq!(saved(&store, &recorder.chat_id()).messages.len(), 1);
    }

    #[tokio::test]
    async fn empty_messages_and_busy_chats_are_refused() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(ChatEngine::default());
        let (channel, busy) = recorder();
        let mut empty = request(None, "qwen3:0.6b", None);
        empty.content = " \n\t ".into();
        assert!(matches!(
            send(
                &store,
                &engine,
                &Staging::default(),
                server(&fake),
                empty,
                &channel
            )
            .await,
            Err(AppError::Invalid(_))
        ));
        assert!(busy.events().is_empty());

        let chat_id = store.blocking(|c| chats::create(c, 1)).unwrap();
        let slow = request(Some(&chat_id), "chatonlm-slow", None);
        let task = {
            let (store, engine, server) = (store.clone(), engine.clone(), server(&fake));
            tokio::spawn(async move {
                send(&store, &engine, &Staging::default(), server, slow, &channel).await
            })
        };
        busy.wait_for("content", 1).await;
        let (other, _) = recorder();
        let second = request(Some(&chat_id), "qwen3:0.6b", None);
        assert!(matches!(
            send(
                &store,
                &engine,
                &Staging::default(),
                server(&fake),
                second,
                &other
            )
            .await,
            Err(AppError::Invalid(_))
        ));

        // Réponse en cours : la conversation lue montre son dernier état.
        let mut chat = saved(&store, &chat_id);
        engine.overlay(&mut chat);
        assert_eq!(chat.messages[1].content, busy.contents());
        finished(task).await;
        assert_one_final(&busy.events());
    }

    #[tokio::test]
    async fn cancelling_while_waiting_or_streaming() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(ChatEngine::default());

        // Requête reçue sans aucun en-tête en retour (modèle en chargement),
        // en-têtes reçus sans ligne, puis en plein flux.
        for (index, (model, contents)) in [
            ("chatonlm-wait", 0),
            ("chatonlm-hang", 0),
            ("chatonlm-stall", 2),
        ]
        .into_iter()
        .enumerate()
        {
            let (channel, recorder) = recorder();
            let task = {
                let (store, engine, server) = (store.clone(), engine.clone(), server(&fake));
                let request = request(None, model, None);
                tokio::spawn(async move {
                    send(
                        &store,
                        &engine,
                        &Staging::default(),
                        server,
                        request,
                        &channel,
                    )
                    .await
                })
            };
            if contents > 0 {
                recorder.wait_for("content", contents).await;
            } else {
                let what = format!("the request for {model}");
                wait_until(WAIT, &what, || fake.chat_requests().len() > index).await;
            }
            let asked = Instant::now();
            engine.cancel(&format!("stream-{model}"));
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap_or_else(|_| panic!("{model}: still running 1 s after the cancel"))
                .unwrap()
                .unwrap();
            assert!(asked.elapsed() < Duration::from_secs(1), "{model}");
            let events = recorder.events();
            assert_one_final(&events);
            assert_eq!(
                events.last().unwrap()["type"],
                json!("cancelled"),
                "{model}"
            );
            // La réponse partielle est enregistrée telle que reçue.
            let chat = saved(&store, &recorder.chat_id());
            assert_eq!(
                chat.messages.len(),
                1 + usize::from(contents > 0),
                "{model}"
            );
            if contents > 0 {
                assert_eq!(chat.messages[1].content, recorder.contents(), "{model}");
            }
            // Le serveur a vu la requête abandonnée.
            let what = format!("the server to see {model} abandoned");
            wait_until(WAIT, &what, || fake.disconnects() > index).await;
            assert_eq!(fake.disconnects(), index + 1, "{model}");
        }
    }

    #[tokio::test]
    async fn stopping_a_chat_waits_for_the_last_write() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(ChatEngine::default());
        let (channel, recorder) = recorder();
        let task = {
            let (store, engine, server) = (store.clone(), engine.clone(), server(&fake));
            let request = request(None, "chatonlm-stall", None);
            tokio::spawn(async move {
                send(
                    &store,
                    &engine,
                    &Staging::default(),
                    server,
                    request,
                    &channel,
                )
                .await
            })
        };
        recorder.wait_for("content", 2).await;
        let chat_id = recorder.chat_id();
        engine.stop_chat(&chat_id).await;
        // Dernière écriture faite avant le retour de stop_chat.
        assert_eq!(saved(&store, &chat_id).messages[1].content, "\"Hi");
        store.blocking(|c| chats::delete(c, &chat_id)).unwrap();
        finished(task).await;
        let events = recorder.events();
        assert_one_final(&events);
        assert_eq!(events.last().unwrap()["type"], json!("cancelled"));
    }

    #[tokio::test]
    async fn stopping_everything_saves_every_answer() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(ChatEngine::default());
        let mut running = Vec::new();
        for stream in ["stream-a", "stream-b"] {
            let (channel, recorder) = recorder();
            let mut ask = request(None, "chatonlm-stall", None);
            ask.stream_id = stream.into();
            let task = {
                let (store, engine, server) = (store.clone(), engine.clone(), server(&fake));
                tokio::spawn(async move {
                    send(&store, &engine, &Staging::default(), server, ask, &channel).await
                })
            };
            recorder.wait_for("content", 2).await;
            running.push((task, recorder));
        }

        let asked = Instant::now();
        engine.stop_all(Duration::from_secs(1)).await;
        // Dernières écritures faites avant le retour de stop_all (fermeture
        // de l'app : rien n'attend les envois ensuite).
        for (_, recorder) in &running {
            let chat = saved(&store, &recorder.chat_id());
            assert_eq!(chat.messages[1].content, recorder.contents());
        }
        for (task, recorder) in running {
            finished(task).await;
            let events = recorder.events();
            assert_one_final(&events);
            assert_eq!(events.last().unwrap()["type"], json!("cancelled"));
            let chat = saved(&store, &recorder.chat_id());
            assert_eq!(chat.messages[1].content, recorder.contents());
        }
        let elapsed = asked.elapsed();
        assert!(elapsed < Duration::from_millis(1500), "{elapsed:?}");
    }

    #[tokio::test]
    async fn a_long_answer_is_saved_while_it_streams() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(ChatEngine::default());
        let (channel, recorder) = recorder();
        let task = {
            let (store, engine, server) = (store.clone(), engine.clone(), server(&fake));
            let request = request(None, "chatonlm-stall", None);
            tokio::spawn(async move {
                send(
                    &store,
                    &engine,
                    &Staging::default(),
                    server,
                    request,
                    &channel,
                )
                .await
            })
        };
        recorder.wait_for("content", 1).await;
        // Plus que l'intervalle des écritures (500 ms) : le flux continue
        // (lignes vides) et l'écriture périodique a eu lieu.
        tokio::time::sleep(Duration::from_millis(900)).await;
        let chat = saved(&store, &recorder.chat_id());
        assert!(!chat.messages[1].content.is_empty());
        assert_eq!(chat.messages[1].content, recorder.contents());
        assert!(!recorder.events().iter().any(is_final));

        engine.cancel("stream-chatonlm-stall");
        finished(task).await;
        assert_one_final(&recorder.events());
    }

    #[tokio::test]
    async fn the_final_event_comes_after_the_last_write() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(ChatEngine::default());

        // Fin normale (flux rapide : pas d'écriture périodique avant).
        let (channel, recorder, at_final) = recorder_reading(store.clone());
        let ask = request(None, "qwen3:0.6b", None);
        send(
            &store,
            &engine,
            &Staging::default(),
            server(&fake),
            ask,
            &channel,
        )
        .await
        .unwrap();
        assert!(!recorder.contents().is_empty());
        assert_eq!(at_final.lock().clone(), Some(recorder.contents()));

        // Arrêt en plein flux, avant toute écriture périodique.
        let (channel, recorder, at_final) = recorder_reading(store.clone());
        let task = {
            let (store, engine, server) = (store.clone(), engine.clone(), server(&fake));
            let request = request(None, "chatonlm-stall", None);
            tokio::spawn(async move {
                send(
                    &store,
                    &engine,
                    &Staging::default(),
                    server,
                    request,
                    &channel,
                )
                .await
            })
        };
        recorder.wait_for("content", 2).await;
        engine.cancel("stream-chatonlm-stall");
        finished(task).await;
        assert_eq!(at_final.lock().clone(), Some(recorder.contents()));
    }

    #[tokio::test]
    async fn thinking_ends_with_the_first_piece_of_answer() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (store, engine) = (Store::open_in_memory().unwrap(), ChatEngine::default());
        let (channel, recorder) = recorder();
        // Une ligne toutes les 50 ms : chaque étape a son heure.
        send(
            &store,
            &engine,
            &Staging::default(),
            server(&fake),
            request(None, "chatonlm-slowthink", None),
            &channel,
        )
        .await
        .unwrap();
        let events = recorder.events();
        assert_one_final(&events);
        assert_eq!(events.last().unwrap()["type"], json!("done"));

        let reply = &saved(&store, &recorder.chat_id()).messages[1];
        let ended = reply.thinking_ended_at.unwrap();
        let last_thinking = *recorder.received("thinking").last().unwrap();
        let first_content = recorder.received("content")[0];
        // Fin datée au premier fragment de réponse : après le dernier
        // fragment de raisonnement, avant la fin du flux (deux fragments de
        // réponse et la ligne finale plus tard).
        assert!(
            last_thinking <= ended && ended <= first_content,
            "{last_thinking} <= {ended} <= {first_content}"
        );
        // Les lignes sont espacées de 50 ms : une fin datée au dernier
        // fragment de raisonnement serait ~50 ms trop tôt.
        assert!(first_content - ended < 25, "{ended} vs {first_content}");
        assert!(ended < reply.updated_at, "{ended} < {}", reply.updated_at);
        assert!(reply.thinking_started_at.unwrap() < ended);
        // Flux lu jusqu'à « done » puis lâché : rien d'abandonné, même avec un
        // délai entre les lignes.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(fake.disconnects(), 0);
    }

    /// Prépare un fichier ; renvoie son identifiant.
    async fn stage(staging: &Staging, name: &str, data: &[u8]) -> String {
        match staging.stage_bytes(name.into(), data.to_vec()).await {
            StageResult::Staged { file } => file.id,
            StageResult::Rejected { error, .. } => panic!("{name} rejected: {error}"),
        }
    }

    /// Image envoyée dans les tests : 4 × 4 pixels rouges.
    fn red_png() -> Vec<u8> {
        samples::png(4, 4, [255, 0, 0])
    }

    /// Contenu des fichiers préparés dans les séquences ; `None` pour un autre
    /// nom (identifiant inconnu de la zone).
    fn sample_file(name: &str) -> Option<Vec<u8>> {
        match name {
            "notes.txt" => Some(b"Line 1\nLine 2".to_vec()),
            "red.png" => Some(red_png()),
            "extra.md" => Some(b"# Extra\nMore notes".to_vec()),
            _ => None,
        }
    }

    /// Texte du message de l'utilisateur tel qu'envoyé avec notes.txt et
    /// report.pdf (format exact de `buildChatRequest`).
    const DESCRIBED: &str = "Describe these files.\n--- File: notes.txt ---\nLine 1\nLine 2\n--- End of notes.txt ---\n--- File: report.pdf ---\nText of the PDF\n--- End of report.pdf ---";

    #[tokio::test]
    async fn attachments_go_with_the_message_and_to_the_model() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (store, engine, staging) = (
            Store::open_in_memory().unwrap(),
            ChatEngine::default(),
            attachment_tests::staging(),
        );
        let ids = vec![
            stage(&staging, "notes.txt", b"Line 1\nLine 2").await,
            stage(&staging, "red.png", &red_png()).await,
            stage(&staging, "report.pdf", b"%PDF-1.4 fake").await,
        ];
        let mut ask = request(None, "qwen3.5:0.8b", Some(Think::Enabled(false)));
        ask.content = "Describe these files.".into();
        ask.attachment_ids = Some(ids.clone());
        let (channel, first) = recorder();
        send(&store, &engine, &staging, server(&fake), ask, &channel)
            .await
            .unwrap();
        let events = first.events();
        assert_one_final(&events);
        assert_eq!(events.last().unwrap()["type"], json!("done"), "{events:?}");

        // Le message enregistré garde le texte tapé ; ses pièces jointes
        // suivent, dans l'ordre, dans l'événement comme en base.
        let png_size = red_png().len();
        let expected = json!([
            {"id": 1, "filename": "notes.txt", "kind": "text", "mime": "text/plain", "size": 13},
            {"id": 2, "filename": "red.png", "kind": "image", "mime": "image/png", "size": png_size},
            {"id": 3, "filename": "report.pdf", "kind": "text", "mime": "application/pdf", "size": 13},
        ]);
        let user = &events[1]["message"];
        assert_eq!(user["content"], json!("Describe these files."));
        assert_eq!(user["attachments"], expected);
        let chat = saved(&store, &first.chat_id());
        assert_eq!(json!(chat.messages[0].attachments), expected);
        assert_eq!(chat.messages[0].content, "Describe these files.");
        // Les fichiers ont quitté la zone de préparation.
        assert!(ids.iter().all(|id| staging.get(id).is_none()));

        let png = BASE64.encode(red_png());
        let sent = &fake.chat_requests()[0];
        assert_eq!(
            sent["messages"],
            json!([{"role": "user", "content": DESCRIBED, "images": [png]}])
        );

        // Message suivant : l'historique renvoie les fichiers du premier.
        let (channel, again) = recorder();
        let mut next = request(
            Some(&first.chat_id()),
            "qwen3.5:0.8b",
            Some(Think::Enabled(false)),
        );
        next.content = "Again".into();
        send(&store, &engine, &staging, server(&fake), next, &channel)
            .await
            .unwrap();
        assert_one_final(&again.events());
        let messages = fake.chat_requests()[1]["messages"].clone();
        assert_eq!(
            messages[0],
            json!({"role": "user", "content": DESCRIBED, "images": [png]})
        );
        assert_eq!(messages[2], json!({"role": "user", "content": "Again"}));
        assert_eq!(messages.as_array().unwrap().len(), 3);
    }

    #[test]
    fn images_are_sent_unless_the_model_lacks_vision() {
        for (details, sent) in [
            // Détails inconnus (/api/show en échec) : envoyées, comme l'officiel.
            (None, true),
            // Capacités inconnues (serveur ancien) : envoyées.
            (Some(details(&[], None)), true),
            (Some(details(&["completion", "vision"], None)), true),
            // Capacités connues sans « vision » : Ollama refuserait (400).
            (Some(details(&["completion", "thinking"], None)), false),
        ] {
            assert_eq!(accepts_images(details.as_ref()), sent, "{details:?}");
        }
    }

    /// Images relues dans les requêtes reçues par le faux serveur.
    #[tokio::test]
    async fn images_are_left_out_for_a_model_without_vision() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (store, engine, staging) = (
            Store::open_in_memory().unwrap(),
            ChatEngine::default(),
            attachment_tests::staging(),
        );
        let png = BASE64.encode(red_png());
        // Modèle, images envoyées.
        for (model, images) in [
            // Sans « vision » (capacités enregistrées de la vraie instance).
            ("qwen3:0.6b", false),
            ("chatonlm-plain", false),
            ("qwen3.5:0.8b", true),
            // /api/show en échec : capacités inconnues.
            ("chatonlm-nothink", true),
        ] {
            let ids = vec![
                stage(&staging, "notes.txt", b"Line 1").await,
                stage(&staging, "red.png", &red_png()).await,
            ];
            let mut ask = request(None, model, None);
            ask.attachment_ids = Some(ids);
            let before = fake.chat_requests().len();
            let (channel, recorder) = recorder();
            send(&store, &engine, &staging, server(&fake), ask, &channel)
                .await
                .unwrap();
            assert_eq!(
                recorder.events().last().unwrap()["type"],
                json!("done"),
                "{model}"
            );
            let sent = &fake.chat_requests()[before]["messages"][0];
            // Le texte des fichiers part toujours.
            assert!(
                sent["content"]
                    .as_str()
                    .unwrap()
                    .ends_with("\n--- File: notes.txt ---\nLine 1\n--- End of notes.txt ---"),
                "{model}: {sent}"
            );
            let expected = images.then(|| json!([png]));
            assert_eq!(sent.get("images"), expected.as_ref(), "{model}");
            // L'image reste une pièce jointe du message enregistré.
            let chat = saved(&store, &recorder.chat_id());
            assert_eq!(chat.messages[0].attachments.len(), 2, "{model}");
        }
    }

    #[tokio::test]
    async fn unknown_attachments_are_refused_before_anything_is_written() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (store, engine, staging) = (
            Store::open_in_memory().unwrap(),
            ChatEngine::default(),
            attachment_tests::staging(),
        );
        let known = stage(&staging, "notes.txt", b"Line 1").await;
        let (channel, recorder) = recorder();
        let mut ask = request(None, "qwen3:0.6b", None);
        ask.attachment_ids = Some(vec![known.clone(), "unknown".into()]);
        let refused = send(&store, &engine, &staging, server(&fake), ask, &channel).await;
        let Err(AppError::Invalid(message)) = refused else {
            panic!("{refused:?}");
        };
        assert_eq!(message, "unknown attachment: unknown");
        assert!(recorder.events().is_empty());
        // Aucune conversation créée, aucune requête, le fichier connu reste.
        assert!(store.blocking(|c| chats::list(c)).unwrap().is_empty());
        assert!(fake.chat_requests().is_empty());
        assert!(staging.get(&known).is_some());
    }

    #[tokio::test]
    async fn files_come_back_only_when_the_send_is_refused() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(ChatEngine::default());
        let staging = attachment_tests::staging();
        let file = stage(&staging, "notes.txt", b"Line 1").await;
        let with_file = |chat_id: Option<&str>| {
            let mut ask = request(chat_id, "qwen3:0.6b", None);
            ask.attachment_ids = Some(vec![file.clone()]);
            ask
        };

        // Conversation occupée : envoi refusé, le fichier reste préparé.
        let chat_id = store.blocking(|c| chats::create(c, 1)).unwrap();
        let (channel, busy) = recorder();
        let task = {
            let (store, engine, server) = (store.clone(), engine.clone(), server(&fake));
            let slow = request(Some(&chat_id), "chatonlm-stall", None);
            tokio::spawn(async move {
                send(&store, &engine, &Staging::default(), server, slow, &channel).await
            })
        };
        busy.wait_for("content", 1).await;
        let (channel, _) = recorder();
        let refused = send(
            &store,
            &engine,
            &staging,
            server(&fake),
            with_file(Some(&chat_id)),
            &channel,
        )
        .await;
        assert!(matches!(refused, Err(AppError::Invalid(_))), "{refused:?}");
        assert!(staging.get(&file).is_some());
        engine.cancel("stream-chatonlm-stall");
        finished(task).await;

        // Écriture du message impossible (conversation supprimée) : erreur
        // finale ; envoi accepté, le fichier quitte quand même la zone (la page
        // a vidé son composer et ne le renverra pas).
        let (channel, failed) = recorder();
        send(
            &store,
            &engine,
            &staging,
            server(&fake),
            with_file(Some("deleted")),
            &channel,
        )
        .await
        .unwrap();
        let events = failed.events();
        assert_eq!(types(&events), ["error"], "{events:?}");
        assert_eq!(events[0]["kind"], json!("internal"));
        assert!(staging.get(&file).is_none());

        // Enregistré : il quitte la zone.
        let file = stage(&staging, "notes.txt", b"Line 1").await;
        let with_file = |chat_id: Option<&str>| {
            let mut ask = request(chat_id, "qwen3:0.6b", None);
            ask.attachment_ids = Some(vec![file.clone()]);
            ask
        };
        let (channel, sent) = recorder();
        send(
            &store,
            &engine,
            &staging,
            server(&fake),
            with_file(None),
            &channel,
        )
        .await
        .unwrap();
        assert_eq!(sent.events().last().unwrap()["type"], json!("done"));
        assert!(staging.get(&file).is_none());
    }

    /// Conversation remplie (modèle vision) : message 1 avec notes.txt (pièce
    /// jointe 1) et red.png (2), réponse 2, message 3 avec later.txt (3),
    /// réponse 4.
    async fn filled_chat(
        store: &Store,
        engine: &ChatEngine,
        staging: &Staging,
        fake: &FakeOllama,
    ) -> String {
        let mut chat_id = None;
        for (content, files) in [
            ("Describe these files.", &["notes.txt", "red.png"][..]),
            ("Again", &["later.txt"][..]),
        ] {
            let mut ids = Vec::new();
            for file in files {
                let data = sample_file(file).unwrap_or_else(|| b"Later".to_vec());
                ids.push(stage(staging, file, &data).await);
            }
            let mut ask = request(
                chat_id.as_deref(),
                "qwen3.5:0.8b",
                Some(Think::Enabled(false)),
            );
            ask.content = content.into();
            ask.attachment_ids = Some(ids);
            let (channel, recorder) = recorder();
            send(store, engine, staging, server(fake), ask, &channel)
                .await
                .unwrap();
            let events = recorder.events();
            assert_eq!(events.last().unwrap()["type"], json!("done"), "{events:?}");
            chat_id.get_or_insert_with(|| recorder.chat_id());
        }
        let chat_id = chat_id.unwrap();
        let chat = saved(store, &chat_id);
        let ids: Vec<_> = chat.messages.iter().map(|m| m.id).collect();
        assert_eq!(ids, [1, 2, 3, 4]);
        chat_id
    }

    /// Modification du message 1 : notes.txt gardé, `file` ajouté.
    fn edit_of_first(chat_id: Option<&str>, file: &str) -> SendRequest {
        let mut edit = request(chat_id, "qwen3.5:0.8b", Some(Think::Enabled(false)));
        edit.content = "Describe this file instead.".into();
        edit.edit_message_id = Some(1);
        edit.kept_attachment_ids = Some(vec![1]);
        edit.attachment_ids = Some(vec![file.to_owned()]);
        edit
    }

    #[tokio::test]
    async fn editing_a_message_rewrites_it_and_cuts_the_history() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (store, engine, staging) = (
            Store::open_in_memory().unwrap(),
            ChatEngine::default(),
            attachment_tests::staging(),
        );
        let chat_id = filled_chat(&store, &engine, &staging, &fake).await;
        let original = saved(&store, &chat_id).messages[0].clone();
        let extra = stage(&staging, "extra.md", b"# Extra").await;
        let (channel, recorder) = recorder();
        let edit = edit_of_first(Some(&chat_id), &extra);
        send(&store, &engine, &staging, server(&fake), edit, &channel)
            .await
            .unwrap();
        let events = recorder.events();
        assert_one_final(&events);
        assert_eq!(
            types(&events),
            ["userMessage", "replyStarted", "content", "done"]
        );

        // Le même message, texte et pièces jointes changés (red.png retiré),
        // date de création gardée.
        let user = &events[0]["message"];
        assert_eq!(user["id"], json!(1));
        assert_eq!(user["content"], json!("Describe this file instead."));
        assert_eq!(user["createdAt"], json!(original.created_at));
        assert_eq!(
            user["attachments"],
            json!([
                {"id": 1, "filename": "notes.txt", "kind": "text", "mime": "text/plain", "size": 13},
                {"id": 4, "filename": "extra.md", "kind": "text", "mime": "text/markdown", "size": 7},
            ])
        );
        assert!(staging.get(&extra).is_none());

        // En base : le message modifié, puis la nouvelle réponse (identifiant
        // jamais utilisé) ; les messages 2 à 4 et leurs pièces jointes ont
        // disparu.
        let chat = saved(&store, &chat_id);
        assert_eq!(chat.messages.len(), 2);
        assert_eq!(json!(chat.messages[0]), *user);
        assert_eq!(chat.messages[1].id, 5);
        assert_eq!(chat.messages[1].content, "\"Hi!\"");
        assert_eq!(events.last().unwrap()["messageId"], json!(5));
        let attachments: i64 = store
            .blocking(|c| Ok(c.query_row("SELECT COUNT(*) FROM attachments", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(attachments, 2);

        // Le modèle reçoit l'historique coupé au message modifié, avec son
        // nouveau texte et ses fichiers, sans l'image retirée.
        let sent = fake.chat_requests();
        assert_eq!(sent.len(), 3);
        assert_eq!(
            sent[2]["messages"],
            json!([{
                "role": "user",
                "content": "Describe this file instead.\n--- File: notes.txt ---\nLine 1\nLine 2\n--- End of notes.txt ---\n--- File: extra.md ---\n# Extra\n--- End of extra.md ---",
            }])
        );
        assert_eq!(sent[2]["think"], json!(false));
    }

    #[tokio::test]
    async fn edits_are_refused_before_anything_is_written() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (store, engine, staging) = (
            Store::open_in_memory().unwrap(),
            ChatEngine::default(),
            attachment_tests::staging(),
        );
        let chat_id = filled_chat(&store, &engine, &staging, &fake).await;
        // Autre conversation : message 5 avec notes.txt (pièce jointe 4).
        let mut elsewhere = request(None, "qwen3.5:0.8b", None);
        elsewhere.attachment_ids = Some(vec![stage(&staging, "notes.txt", b"Other").await]);
        let (channel, other) = recorder();
        send(
            &store,
            &engine,
            &staging,
            server(&fake),
            elsewhere,
            &channel,
        )
        .await
        .unwrap();
        let other_id = other.chat_id();
        assert_eq!(saved(&store, &other_id).messages[0].id, 5);
        assert_eq!(saved(&store, &other_id).messages[0].attachments[0].id, 4);

        let both = || (saved(&store, &chat_id), saved(&store, &other_id));
        let before = both();
        let requests = fake.chat_requests().len();
        let file = stage(&staging, "extra.md", b"# Extra").await;
        let edit = |message_id: Option<i64>, kept: Option<Vec<i64>>| {
            let mut edit = edit_of_first(Some(&chat_id), &file);
            edit.edit_message_id = message_id;
            edit.kept_attachment_ids = kept;
            edit
        };
        let mut without_chat = edit(Some(1), None);
        without_chat.chat_id = None;
        let mut empty = edit(Some(1), None);
        empty.content = " \n ".into();
        for (ask, expected) in [
            (
                edit(None, Some(vec![1])),
                "keptAttachmentIds needs editMessageId",
            ),
            (without_chat, "editMessageId needs chatId"),
            (edit(Some(99), None), "unknown message: 99"),
            (edit(Some(5), None), "message 5 is in another chat"),
            (edit(Some(2), None), "message 2 is not a user message"),
            // later.txt (message 3), puis la pièce jointe de l'autre
            // conversation.
            (
                edit(Some(1), Some(vec![1, 3])),
                "attachment 3 is not in message 1",
            ),
            (
                edit(Some(1), Some(vec![4])),
                "attachment 4 is not in message 1",
            ),
            (
                edit(Some(1), Some(vec![2, 1, 2])),
                "attachment 2 is kept twice",
            ),
            (empty, "the message is empty"),
        ] {
            let (channel, recorder) = recorder();
            let refused = send(&store, &engine, &staging, server(&fake), ask, &channel).await;
            let Err(AppError::Invalid(message)) = refused else {
                panic!("{expected}: {refused:?}");
            };
            assert_eq!(message, expected);
            assert!(recorder.events().is_empty(), "{expected}");
            // Rien d'écrit, le fichier préparé reste dans la zone.
            assert_eq!(both(), before, "{expected}");
            assert_eq!(store.blocking(|c| chats::list(c)).unwrap().len(), 2);
            assert!(staging.get(&file).is_some(), "{expected}");
        }
        assert_eq!(fake.chat_requests().len(), requests);
    }

    #[tokio::test]
    async fn editing_a_busy_chat_is_refused() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(ChatEngine::default());
        let staging = attachment_tests::staging();
        let chat_id = filled_chat(&store, &engine, &staging, &fake).await;
        let (channel, busy) = recorder();
        let task = {
            let (store, engine, server) = (store.clone(), engine.clone(), server(&fake));
            let slow = request(Some(&chat_id), "chatonlm-stall", None);
            tokio::spawn(async move {
                send(&store, &engine, &Staging::default(), server, slow, &channel).await
            })
        };
        busy.wait_for("content", 1).await;
        let file = stage(&staging, "extra.md", b"# Extra").await;
        let (channel, refused_events) = recorder();
        let edit = edit_of_first(Some(&chat_id), &file);
        let refused = send(&store, &engine, &staging, server(&fake), edit, &channel).await;
        let Err(AppError::Invalid(message)) = refused else {
            panic!("{refused:?}");
        };
        assert_eq!(message, "this chat is already waiting for an answer");
        assert!(refused_events.events().is_empty());
        assert!(staging.get(&file).is_some());
        // Rien de coupé : message 1 intact, la réponse en cours (6) reste.
        let chat = saved(&store, &chat_id);
        let ids: Vec<_> = chat.messages.iter().map(|m| m.id).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5, 6]);
        assert_eq!(chat.messages[0].content, "Describe these files.");
        assert_eq!(chat.messages[0].attachments.len(), 2);
        engine.cancel("stream-chatonlm-stall");
        finished(task).await;
    }

    #[tokio::test]
    async fn a_stop_asked_while_an_edit_is_checked_is_not_lost() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(ChatEngine::default());
        let staging = Arc::new(attachment_tests::staging());
        let chat_id = filled_chat(&store, &engine, &staging, &fake).await;
        let file = stage(&staging, "extra.md", b"# Extra").await;
        // Base tenue par un autre fil : la vérification de la modification
        // attend, l'arrêt est demandé pendant ce temps.
        let (locked, is_locked) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let holder = {
            let store = store.clone();
            std::thread::spawn(move || {
                store
                    .blocking(|_| {
                        locked.send(()).unwrap();
                        released.recv().unwrap();
                        Ok(())
                    })
                    .unwrap();
            })
        };
        is_locked.recv().unwrap();
        let edit = edit_of_first(Some(&chat_id), &file);
        let stream_id = edit.stream_id.clone();
        let (channel, recorder) = recorder();
        let task = {
            let (store, engine, staging, server) = (
                store.clone(),
                engine.clone(),
                staging.clone(),
                server(&fake),
            );
            tokio::spawn(
                async move { send(&store, &engine, &staging, server, edit, &channel).await },
            )
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        engine.cancel(&stream_id);
        release.send(()).unwrap();
        holder.join().unwrap();
        task.await.unwrap().unwrap();
        let events = recorder.events();
        assert_one_final(&events);
        assert_eq!(types(&events).last().map(String::as_str), Some("cancelled"));
        assert_eq!(fake.chat_requests().len(), 2, "no request for the edit");
    }

    #[tokio::test]
    async fn a_failed_edit_changes_nothing_but_uses_up_its_files() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (store, engine, staging) = (
            Store::open_in_memory().unwrap(),
            ChatEngine::default(),
            attachment_tests::staging(),
        );
        let chat_id = filled_chat(&store, &engine, &staging, &fake).await;
        let before = saved(&store, &chat_id);
        // Échec à la dernière écriture (la nouvelle pièce jointe) : toute la
        // transaction est annulée.
        store
            .blocking(|c| {
                c.execute_batch(
                    "CREATE TEMP TRIGGER no_more_files BEFORE INSERT ON attachments
                     BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
                )?;
                Ok(())
            })
            .unwrap();
        let file = stage(&staging, "extra.md", b"# Extra").await;
        let (channel, recorder) = recorder();
        let edit = edit_of_first(Some(&chat_id), &file);
        send(&store, &engine, &staging, server(&fake), edit, &channel)
            .await
            .unwrap();
        let events = recorder.events();
        assert_eq!(types(&events), ["error"], "{events:?}");
        assert_eq!(events[0]["kind"], json!("internal"));
        assert_eq!(events[0]["message"], json!("database error: disk full"));
        assert_eq!(saved(&store, &chat_id), before);
        // Envoi accepté : le fichier a quitté la zone (la page a vidé son
        // composer), aucune requête au modèle.
        assert!(staging.get(&file).is_none());
        assert_eq!(fake.chat_requests().len(), 2);
    }

    /// Envoi à `model` avec la recherche web demandée.
    fn searching(chat_id: Option<&str>, model: &str, content: &str) -> SendRequest {
        SendRequest {
            content: content.into(),
            web_search: Some(true),
            ..request(chat_id, model, None)
        }
    }

    /// Envoi complet sur ce serveur, borné par `limit` (une boucle d'outils
    /// sans fin échoue au lieu de pendre) : événements reçus.
    async fn sent(store: &Store, fake: &FakeOllama, ask: SendRequest, limit: Duration) -> Recorder {
        let (channel, recorder) = recorder();
        let (engine, staging) = (ChatEngine::default(), Staging::default());
        let sending = send(store, &engine, &staging, server(fake), ask, &channel);
        tokio::time::timeout(limit, sending)
            .await
            .unwrap_or_else(|_| panic!("still sending after {limit:?}: {:?}", recorder.events()))
            .unwrap();
        assert_one_final(&recorder.events());
        recorder
    }

    /// Événements de ce type, dans l'ordre.
    fn of_type(events: &[Value], kind: &str) -> Vec<Value> {
        events
            .iter()
            .filter(|e| e["type"] == kind)
            .cloned()
            .collect()
    }

    #[tokio::test]
    async fn a_web_search_calls_the_tool_then_answers() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_signed_in(true);
        let store = Store::open_in_memory().unwrap();
        let question = "Latest Ollama release?";
        let ask = searching(None, "chatonlm-search", question);
        let recorder = sent(&store, &fake, ask, WAIT).await;
        let events = recorder.events();
        // Un seul chargement, avant le premier tour.
        assert_eq!(
            types(&events),
            [
                "chatCreated",
                "userMessage",
                "loading",
                "loaded",
                "replyStarted",
                "thinking",
                "toolCalls",
                "toolResult",
                "replyStarted",
                "content",
                "done"
            ]
        );

        // Deux messages de l'assistant : les appels (avec leur raisonnement,
        // clos), puis la réponse.
        let chat = saved(&store, &recorder.chat_id());
        assert_eq!(chat.messages.len(), 3);
        let (calls, answer) = (&chat.messages[1], &chat.messages[2]);
        assert_eq!(calls.content, "");
        assert_eq!(calls.thinking, "The user wants a web search.");
        assert!(calls.thinking_ended_at.is_some());
        let arguments = json!({ "query": question }).to_string();
        assert_eq!(calls.tool_calls.len(), 1);
        let call = &calls.tool_calls[0];
        assert_eq!(
            (call.name.as_str(), call.arguments.as_str()),
            ("web_search", arguments.as_str())
        );
        let result = call.result.clone().expect("a result");
        assert_eq!(call.error, None);
        let results: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            results["results"][0]["title"],
            "Result 1 for Latest Ollama release?"
        );
        assert_eq!(answer.content, format!("Found: {result}"));
        assert!(answer.tool_calls.is_empty());

        // Événements : appels sans résultat, puis le résultat ; `done` désigne
        // la réponse.
        let started = of_type(&events, "replyStarted");
        assert_eq!(started[0]["messageId"], json!(calls.id));
        assert_eq!(started[1]["messageId"], json!(answer.id));
        assert_eq!(
            of_type(&events, "toolCalls"),
            [json!({
                "type": "toolCalls",
                "messageId": calls.id,
                "toolCalls": [{ "name": "web_search", "arguments": arguments, "result": null, "error": null }],
            })]
        );
        assert_eq!(
            of_type(&events, "toolResult"),
            [json!({
                "type": "toolResult",
                "messageId": calls.id,
                "index": 0,
                "result": result,
                "error": null,
            })]
        );
        assert_eq!(events.last().unwrap()["messageId"], json!(answer.id));

        // Les deux requêtes portent les outils ; la seconde, l'appel et son
        // résultat.
        let requests = fake.chat_requests();
        assert_eq!(requests.len(), 2);
        for sent in &requests {
            assert_eq!(sent["tools"], tools::definitions());
        }
        assert_eq!(
            requests[1]["messages"],
            json!([
                { "role": "user", "content": question },
                {
                    "role": "assistant",
                    "content": "",
                    "thinking": "The user wants a web search.",
                    "tool_calls": [{ "function": { "name": "web_search", "arguments": { "query": question } } }],
                },
                { "role": "tool", "content": result, "tool_name": "web_search" },
            ])
        );
        // Exécuté par le serveur Ollama, `max_results` à 5 par défaut.
        assert_eq!(
            fake.web_requests(),
            [json!({
                "path": "/api/experimental/web_search",
                "body": { "query": question, "max_results": 5 },
            })]
        );
    }

    #[tokio::test]
    async fn tool_errors_go_to_the_model() {
        // Cloud coupé (compte délié) : compte pas demandé, 403 du serveur
        // donné au modèle.
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_cloud_disabled(true);
        let store = Store::open_in_memory().unwrap();
        let ask = searching(None, "chatonlm-search", "Search this");
        let recorder = sent(&store, &fake, ask, WAIT).await;
        let events = recorder.events();
        assert_eq!(events.last().unwrap()["type"], json!("done"));
        let refused = "ollama cloud is disabled: web search is unavailable";
        let results = of_type(&events, "toolResult");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["result"], Value::Null);
        assert_eq!(results[0]["error"], json!(refused));
        let chat = saved(&store, &recorder.chat_id());
        let call = &chat.messages[1].tool_calls[0];
        assert_eq!(
            (call.result.as_deref(), call.error.as_deref()),
            (None, Some(refused))
        );
        assert_eq!(chat.messages[2].content, format!("Found: Error: {refused}"));
        let second = &fake.chat_requests()[1];
        assert_eq!(
            second["messages"][2],
            json!({ "role": "tool", "content": format!("Error: {refused}"), "tool_name": "web_search" })
        );
    }

    /// Erreur finale d'une recherche sans compte relié.
    fn signed_out_event() -> Value {
        json!({
            "type": "error",
            "kind": "webSearchSignedOut",
            "message": "Web search requires an Ollama account",
        })
    }

    #[tokio::test]
    async fn a_search_without_account_stops_before_answering() {
        // Compte délié (le faux serveur au départ), dit par /api/me : arrêt
        // avant toute requête à /api/chat ; le message de l'utilisateur reste.
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Store::open_in_memory().unwrap();
        let ask = searching(None, "chatonlm-search", "Search this");
        let recorder = sent(&store, &fake, ask, WAIT).await;
        let events = recorder.events();
        assert_eq!(events.last().unwrap(), &signed_out_event());
        assert!(of_type(&events, "replyStarted").is_empty());
        assert!(fake.chat_requests().is_empty());
        assert!(fake.web_requests().is_empty());
        let chat = saved(&store, &recorder.chat_id());
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].content, "Search this");
    }

    #[tokio::test]
    async fn a_search_refused_by_the_server_stops_the_answer() {
        // Compte inconnu (ollama.com injoignable pour /api/me) : l'envoi
        // part ; le 401 de l'outil l'arrête, l'appel reste sans résultat ni
        // erreur, le raisonnement est clos.
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_account_unavailable(true);
        let store = Store::open_in_memory().unwrap();
        let ask = searching(None, "chatonlm-search", "Search this");
        let recorder = sent(&store, &fake, ask, WAIT).await;
        let events = recorder.events();
        assert_eq!(events.last().unwrap(), &signed_out_event());
        assert_eq!(of_type(&events, "toolCalls").len(), 1);
        assert!(of_type(&events, "toolResult").is_empty());
        assert_eq!(fake.chat_requests().len(), 1);
        assert_eq!(fake.web_requests().len(), 1);
        let chat = saved(&store, &recorder.chat_id());
        assert_eq!(chat.messages.len(), 2);
        let reply = &chat.messages[1];
        let call = &reply.tool_calls[0];
        assert_eq!(
            (call.result.as_deref(), call.error.as_deref()),
            (None, None)
        );
        assert!(reply.thinking_ended_at.is_some());
    }

    #[tokio::test]
    async fn the_account_is_checked_only_when_tools_are_offered() {
        // Compte délié, recherche demandée mais pas d'outils (modèle sans
        // « tools », pièce jointe) : réponse normale.
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let store = Store::open_in_memory().unwrap();
        let ask = searching(None, "chatonlm-plain", "Hello");
        let events = sent(&store, &fake, ask, WAIT).await.events();
        assert_eq!(events.last().unwrap()["type"], json!("done"));
        let (engine, staging) = (ChatEngine::default(), attachment_tests::staging());
        let mut with_file = searching(None, "chatonlm-search", "Describe this file.");
        with_file.attachment_ids =
            Some(vec![stage(&staging, "notes.txt", b"Line 1\nLine 2").await]);
        let (channel, recorder) = recorder();
        send(
            &store,
            &engine,
            &staging,
            server(&fake),
            with_file,
            &channel,
        )
        .await
        .unwrap();
        let events = recorder.events();
        assert_one_final(&events);
        assert_eq!(events.last().unwrap()["type"], json!("done"));
    }

    #[tokio::test]
    async fn tools_are_offered_only_when_asked_for_and_possible() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_signed_in(true);
        let (store, engine, staging) = (
            Store::open_in_memory().unwrap(),
            ChatEngine::default(),
            attachment_tests::staging(),
        );
        // Outils dans la première requête de l'envoi suivant.
        let offered = |before: usize| fake.chat_requests()[before].get("tools").is_some();
        let plain = |model: &str, web_search: Option<bool>| SendRequest {
            web_search,
            ..request(None, model, None)
        };
        for (what, ask, expected) in [
            ("asked", plain("chatonlm-search", Some(true)), true),
            ("not asked", plain("chatonlm-search", None), false),
            ("turned off", plain("chatonlm-search", Some(false)), false),
            (
                "no tools capability",
                plain("chatonlm-plain", Some(true)),
                false,
            ),
            // `/api/show` en échec (500) : détails inconnus.
            (
                "unknown details",
                plain("chatonlm-nothink", Some(true)),
                false,
            ),
        ] {
            let before = fake.chat_requests().len();
            let (channel, events) = recorder();
            send(&store, &engine, &staging, server(&fake), ask, &channel)
                .await
                .unwrap();
            assert_one_final(&events.events());
            assert_eq!(offered(before), expected, "{what}");
        }

        // Avec une pièce jointe (nouvelle, puis gardée par une
        // modification) : pas d'outils ; sans plus aucune pièce jointe : des
        // outils.
        let mut with_file = searching(None, "chatonlm-search", "Describe these files.");
        with_file.attachment_ids =
            Some(vec![stage(&staging, "notes.txt", b"Line 1\nLine 2").await]);
        let before = fake.chat_requests().len();
        let (channel, first) = recorder();
        send(
            &store,
            &engine,
            &staging,
            server(&fake),
            with_file,
            &channel,
        )
        .await
        .unwrap();
        assert!(!offered(before), "new attachment");
        let chat_id = first.chat_id();
        let message = saved(&store, &chat_id).messages[0].clone();
        let attachment = message.attachments[0].id;
        for (kept, expected) in [(vec![attachment], false), (Vec::new(), true)] {
            let edit = SendRequest {
                edit_message_id: Some(message.id),
                kept_attachment_ids: Some(kept.clone()),
                ..searching(Some(&chat_id), "chatonlm-search", "Search instead")
            };
            let before = fake.chat_requests().len();
            let (channel, edited) = recorder();
            send(&store, &engine, &staging, server(&fake), edit, &channel)
                .await
                .unwrap();
            assert_one_final(&edited.events());
            assert_eq!(offered(before), expected, "edit keeping {kept:?}");
        }
    }

    #[tokio::test]
    async fn tool_turns_stop_after_ten() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_signed_in(true);
        let store = Store::open_in_memory().unwrap();
        let ask = searching(None, "chatonlm-loop", "Search forever");
        let recorder = sent(&store, &fake, ask, Duration::from_secs(20)).await;
        let events = recorder.events();
        assert_eq!(recorder.count("toolCalls"), 10);
        assert_eq!(recorder.count("toolResult"), 10);
        assert_eq!(recorder.count("replyStarted"), 11);
        // Dix requêtes avec les outils, la onzième sans.
        let requests = fake.chat_requests();
        assert_eq!(requests.len(), 11);
        for (index, sent) in requests.iter().enumerate() {
            assert_eq!(sent.get("tools").is_some(), index < 10, "request {index}");
        }
        let queries: Vec<Value> = fake
            .web_requests()
            .iter()
            .map(|request| request["body"]["query"].clone())
            .collect();
        let expected: Vec<Value> = (1..=10).map(|n| json!(format!("loop {n}"))).collect();
        assert_eq!(queries, expected);
        let chat = saved(&store, &recorder.chat_id());
        assert_eq!(chat.messages.len(), 12);
        let last = chat.messages.last().unwrap();
        assert_eq!(last.content, "Done");
        assert_eq!(events.last().unwrap()["messageId"], json!(last.id));
    }

    /// Appels reçus d'une requête partie sans outils (après les dix tours) :
    /// ignorés ; ce dernier tour n'a rien écrit, `done` désigne alors la
    /// réponse du tour précédent.
    #[tokio::test]
    async fn calls_from_a_request_without_tools_are_ignored() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_signed_in(true);
        let store = Store::open_in_memory().unwrap();
        let ask = searching(None, "chatonlm-loop", "Search forever, stubborn");
        let recorder = sent(&store, &fake, ask, Duration::from_secs(20)).await;
        let events = recorder.events();
        assert_eq!(fake.chat_requests().len(), 11);
        assert_eq!(fake.web_requests().len(), 10);
        assert_eq!(recorder.count("toolCalls"), 10);
        assert_eq!(recorder.count("replyStarted"), 10);
        let chat = saved(&store, &recorder.chat_id());
        assert_eq!(chat.messages.len(), 11);
        let last = chat.messages.last().unwrap();
        assert_eq!(last.tool_calls.len(), 1);
        assert_eq!(events.last().unwrap()["type"], json!("done"));
        assert_eq!(events.last().unwrap()["messageId"], json!(last.id));
    }

    #[tokio::test]
    async fn stopping_during_a_tool_interrupts_it() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_signed_in(true);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(ChatEngine::default());
        let (channel, recorder) = recorder();
        // « slow » : le serveur fait attendre la recherche 30 s.
        let ask = searching(None, "chatonlm-search", "Search slowly");
        let task = {
            let (store, engine, server) = (store.clone(), engine.clone(), server(&fake));
            tokio::spawn(async move {
                send(&store, &engine, &Staging::default(), server, ask, &channel).await
            })
        };
        recorder.wait_for("toolCalls", 1).await;
        wait_until(WAIT, "the web search request", || {
            fake.web_requests().len() == 1
        })
        .await;
        // Pendant l'outil : l'appel, sans résultat, en base comme dans la
        // réponse vivante.
        let chat_id = recorder.chat_id();
        let waiting = vec![ToolCall::new(
            "web_search",
            json!({ "query": "Search slowly" }).to_string(),
        )];
        let mut chat = saved(&store, &chat_id);
        assert_eq!(chat.messages[1].tool_calls, waiting);
        chat.messages[1].tool_calls.clear();
        engine.overlay(&mut chat);
        assert_eq!(chat.messages[1].tool_calls, waiting);

        let asked = Instant::now();
        engine.cancel("stream-chatonlm-search");
        finished(task).await;
        assert!(asked.elapsed() < Duration::from_secs(1));
        let events = recorder.events();
        assert_one_final(&events);
        assert_eq!(events.last().unwrap()["type"], json!("cancelled"));
        assert_eq!(recorder.count("toolResult"), 0);
        // L'appel reste sans résultat ni erreur ; pas de tour suivant.
        assert_eq!(saved(&store, &chat_id).messages[1].tool_calls, waiting);
        assert_eq!(fake.chat_requests().len(), 1);
        wait_until(WAIT, "the server to see the search abandoned", || {
            fake.disconnects() == 1
        })
        .await;
    }

    /// La réponse vivante (superposée par `get_chat`) porte ses appels, puis
    /// leurs résultats, au moment où ils sont annoncés.
    #[tokio::test]
    async fn the_live_reply_carries_its_tool_calls() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_signed_in(true);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(ChatEngine::default());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (sink, live, chat_id) = (seen.clone(), engine.clone(), Arc::new(Mutex::new(None)));
        let channel = {
            let store = store.clone();
            Channel::new(move |body| {
                let InvokeResponseBody::Json(text) = body else {
                    return Ok(());
                };
                let event: Value = serde_json::from_str(&text).unwrap();
                if let Some(id) = event["chatId"].as_str() {
                    *chat_id.lock() = Some(id.to_owned());
                }
                if matches!(event["type"].as_str(), Some("toolCalls" | "toolResult")) {
                    let id = chat_id.lock().clone().unwrap();
                    let mut chat = store.blocking(move |c| chats::get(c, &id)).unwrap();
                    // Seule la superposition peut les remettre.
                    for message in &mut chat.messages {
                        message.tool_calls.clear();
                    }
                    live.overlay(&mut chat);
                    sink.lock().push(chat.messages[1].tool_calls.clone());
                }
                Ok(())
            })
        };
        let ask = searching(None, "chatonlm-search", "Search this");
        send(
            &store,
            &engine,
            &Staging::default(),
            server(&fake),
            ask,
            &channel,
        )
        .await
        .unwrap();
        let seen = seen.lock().clone();
        assert_eq!(seen.len(), 2);
        let arguments = json!({ "query": "Search this" }).to_string();
        assert_eq!(seen[0], [ToolCall::new("web_search", arguments)]);
        assert!(seen[1][0].result.is_some());
    }

    #[tokio::test]
    async fn web_fetch_reads_only_urls_given_by_the_user() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        fake.set_signed_in(true);
        let store = Store::open_in_memory().unwrap();
        let fetched = |fake: &FakeOllama| -> Vec<String> {
            fake.web_requests()
                .iter()
                .map(|r| r["body"]["url"].as_str().unwrap().to_owned())
                .collect()
        };
        let refusal = "web fetch is only allowed for URLs provided by the user";

        // Adresse changée par le modèle (point final gardé) : refusée, sans
        // requête.
        let ask = searching(None, "chatonlm-fetch", "Read https://example.com/page.");
        let recorder = sent(&store, &fake, ask, WAIT).await;
        let chat_id = recorder.chat_id();
        let chat = saved(&store, &chat_id);
        assert_eq!(
            chat.messages[1].tool_calls[0].error.as_deref(),
            Some(refusal)
        );
        assert_eq!(chat.messages[2].content, format!("Found: Error: {refusal}"));
        assert!(fake.web_requests().is_empty());

        // Adresse écrite telle quelle : lue, puis le lien de la page (reçu
        // pendant l'envoi), puis la réponse.
        let ask = searching(
            Some(&chat_id),
            "chatonlm-fetch",
            "Read https://example.com/page please",
        );
        sent(&store, &fake, ask, WAIT).await;
        assert_eq!(
            fetched(&fake),
            ["https://example.com/page", "https://example.com/next"]
        );
        let chat = saved(&store, &chat_id);
        assert_eq!(chat.messages.len(), 3 + 4);
        let page = chat.messages[4].tool_calls[0].result.clone().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&page).unwrap()["links"],
            json!(["https://example.com/next"])
        );
        assert!(chat.messages[5].tool_calls[0].result.is_some());
        assert!(chat.messages[6].content.starts_with("Found: {"));

        // Adresse d'un message plus ancien de la conversation : permise.
        let ask = searching(Some(&chat_id), "chatonlm-fetch", "Once more");
        sent(&store, &fake, ask, WAIT).await;
        assert_eq!(
            fetched(&fake)[2..],
            ["https://example.com/page", "https://example.com/next"]
        );
    }

    /// Horodatage unique des séquences exportées.
    const FIXED_TIME: i64 = 1_700_000_000_000;

    /// Adresse du serveur dans les séquences exportées.
    const SEQUENCE_URL: &str = "http://127.0.0.1:11434";

    /// Envoi rejoué par le simulateur du frontend.
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct SequenceRequest {
        #[serde(skip_serializing_if = "Option::is_none")]
        chat_id: Option<&'static str>,
        stream_id: &'static str,
        content: String,
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        think: Option<Think>,
        #[serde(skip_serializing_if = "Option::is_none")]
        attachment_ids: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        edit_message_id: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        kept_attachment_ids: Option<Vec<i64>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        web_search: Option<bool>,
    }

    #[derive(Serialize)]
    struct Sequence {
        description: &'static str,
        request: SequenceRequest,
        /// Événements tels qu'envoyés au canal.
        events: Value,
        /// Erreur de la commande refusée, telle que reçue par le frontend.
        error: Value,
    }

    /// Rend une valeur identique d'une exécution à l'autre : conversations
    /// « chat-1 », flux « stream-1 », horodatages fixes, textes remplacés
    /// (identifiant de conversation, adresse du serveur).
    fn normalize(value: &mut Value, replace: &[(String, &str)]) {
        match value {
            Value::Object(fields) => {
                for (key, field) in fields.iter_mut() {
                    if key == "chatId" && field.is_string() {
                        *field = json!("chat-1");
                    } else if key == "streamId" && field.is_string() {
                        *field = json!("stream-1");
                    } else if (key.ends_with("At") || key.ends_with("_at")) && field.is_number() {
                        *field = json!(FIXED_TIME);
                    } else {
                        normalize(field, replace);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    normalize(item, replace);
                }
            }
            Value::String(text) => {
                for (from, to) in replace {
                    if text.contains(from.as_str()) {
                        *text = text.replace(from.as_str(), to);
                    }
                }
            }
            _ => {}
        }
    }

    /// Séquence normalisée, en JSON.
    fn sequence_text(
        description: &'static str,
        request: &SendRequest,
        events: Vec<Value>,
        result: &AppResult<()>,
        replace: &[(String, &str)],
    ) -> String {
        let mut events = Value::Array(events);
        normalize(&mut events, replace);
        let mut error = match result {
            Ok(()) => Value::Null,
            Err(error) => serde_json::to_value(error).unwrap(),
        };
        normalize(&mut error, replace);
        let sequence = Sequence {
            description,
            request: SequenceRequest {
                chat_id: request.chat_id.as_ref().map(|_| "chat-1"),
                stream_id: "stream-1",
                content: request.content.clone(),
                model: request.model.clone(),
                think: request.think.clone(),
                attachment_ids: request.attachment_ids.as_ref().map(|ids| {
                    ids.iter()
                        .map(|id| match replace.iter().find(|(from, _)| from == id) {
                            Some((_, to)) => (*to).to_owned(),
                            None => id.clone(),
                        })
                        .collect()
                }),
                edit_message_id: request.edit_message_id,
                kept_attachment_ids: request.kept_attachment_ids.clone(),
                web_search: request.web_search,
            },
            events,
            error,
        };
        let mut text = serde_json::to_string_pretty(&sequence).unwrap();
        text.push('\n');
        text
    }

    /// Écrit une séquence dans `$CHATONLM_FIXTURES_DIR/chat/<name>.json`
    /// (src/test/fixtures, voir .cargo/config.toml) ; rien sans la variable.
    fn export_sequence(name: &str, text: &str) {
        let Some(dir) = std::env::var_os("CHATONLM_FIXTURES_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir).join("chat");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.json")), text).unwrap();
    }

    /// Envoi complet, annulé après `cancel_after` événements du type donné
    /// (fragments de réponse, progression du téléchargement) si demandé :
    /// événements reçus et résultat de la commande.
    async fn play(
        store: &Store,
        staging: &Staging,
        url: &str,
        request: &SendRequest,
        cancel_after: Option<(&str, usize)>,
    ) -> (Vec<Value>, AppResult<()>) {
        let engine = ChatEngine::default();
        let (channel, recorder) = recorder();
        let server = Server {
            url: url.to_owned(),
            context_length: 0,
            keep_alive: 0,
        };
        let sending = send(store, &engine, staging, server, request.clone(), &channel);
        let cancelling = async {
            if let Some((kind, count)) = cancel_after {
                recorder.wait_for(kind, count).await;
                engine.cancel(&request.stream_id);
            }
        };
        let both = async { tokio::join!(sending, cancelling) };
        let (result, ()) = tokio::time::timeout(WAIT, both)
            .await
            .unwrap_or_else(|_| panic!("{}: still running after {WAIT:?}", request.model));
        (recorder.events(), result)
    }

    /// Serveur d'une séquence.
    #[derive(Clone, Copy)]
    enum Target {
        /// Faux Ollama (compte ollama.com délié).
        Ollama,
        /// Faux Ollama au compte relié (recherche web).
        SignedIn,
        /// Faux Ollama au compte délié, que `/api/me` ne sait pas dire
        /// (503).
        AccountUnknown,
        /// Proxy qui répond 502 sans corps.
        Proxy,
        /// Rien n'écoute.
        Closed,
    }

    /// Serveur neuf (aucun modèle téléchargé), arrêté pour `Closed`.
    async fn start(target: Target) -> (Option<FakeOllama>, String) {
        let mode = match target {
            Target::Ollama | Target::SignedIn | Target::AccountUnknown | Target::Closed => {
                Mode::ollama()
            }
            Target::Proxy => Mode::Status(502),
        };
        let fake = FakeOllama::start(mode).await.unwrap();
        fake.set_signed_in(matches!(target, Target::SignedIn));
        fake.set_account_unavailable(matches!(target, Target::AccountUnknown));
        let url = fake.url();
        if matches!(target, Target::Closed) {
            fake.stop().await;
            return (None, url);
        }
        (Some(fake), url)
    }

    /// Séquences d'événements rejouées par le simulateur des tests du
    /// frontend (src/test/fixtures/chat) : chacune jouée deux fois sur une
    /// base et un serveur neufs, et identique d'une fois à l'autre.
    #[tokio::test]
    async fn sequences_for_the_simulator() {
        let ask = |content: &str, model: &str, think: Option<Think>| SendRequest {
            chat_id: None,
            stream_id: "stream-1".into(),
            content: content.into(),
            model: model.into(),
            think,
            attachment_ids: None,
            edit_message_id: None,
            kept_attachment_ids: None,
            web_search: None,
            voice: None,
        };
        let hello = "Say hello in one word.";
        // Fichiers préparés avant l'envoi, désignés par leur nom dans
        // `attachment_ids` ; un autre nom reste tel quel (inconnu).
        let with_files = |names: &[&str], request: SendRequest| SendRequest {
            attachment_ids: Some(names.iter().map(|name| (*name).to_owned()).collect()),
            ..request
        };
        let describe = "Describe these files.";
        // Modification du message 1 de la conversation remplie par l'envoi de
        // la séquence « attachments ».
        let editing = |message_id: i64, kept: Option<Vec<i64>>, request: SendRequest| SendRequest {
            edit_message_id: Some(message_id),
            kept_attachment_ids: kept,
            ..request
        };
        let instead = "Describe this file instead.";
        let search = |content: &str| SendRequest {
            web_search: Some(true),
            ..ask(content, "chatonlm-search", None)
        };
        let latest = "What is the latest Ollama release?";
        let searched = &[
            "chatCreated",
            "userMessage",
            "loading",
            "loaded",
            "replyStarted",
            "thinking",
            "toolCalls",
            "toolResult",
            "replyStarted",
            "content",
            "done",
        ][..];

        // Nom, description, serveur, envoi, annulation, types d'événements
        // attendus (regroupés).
        let cases = [
            (
                "new-chat-thinking",
                "New chat with a thinking model (think left to the model's default), not loaded yet: loading (instant), thinking, then the answer.",
                Target::Ollama,
                ask(hello, "qwen3:0.6b", None),
                None,
                &[
                    "chatCreated",
                    "userMessage",
                    "loading",
                    "loaded",
                    "replyStarted",
                    "thinking",
                    "content",
                    "done",
                ][..],
            ),
            (
                "existing-chat",
                "Second message in an existing chat, thinking turned off (the model is already loaded).",
                Target::Ollama,
                ask(hello, "qwen3:0.6b", Some(Think::Enabled(false))),
                None,
                &["userMessage", "replyStarted", "content", "done"][..],
            ),
            (
                "cancelled",
                "Answer stopped by the user after two pieces; the partial answer is kept.",
                Target::Ollama,
                ask(hello, "chatonlm-stall", None),
                Some(("content", 2)),
                &[
                    "chatCreated",
                    "userMessage",
                    "loading",
                    "loaded",
                    "replyStarted",
                    "content",
                    "cancelled",
                ][..],
            ),
            (
                "model-error",
                "The model fails in the middle of its answer (error line in the stream).",
                Target::Ollama,
                ask(hello, "chatonlm-error", None),
                None,
                &[
                    "chatCreated",
                    "userMessage",
                    "loading",
                    "loaded",
                    "replyStarted",
                    "content",
                    "error",
                ][..],
            ),
            (
                "connection-lost",
                "The connection to Ollama breaks in the middle of the answer.",
                Target::Ollama,
                ask(hello, "chatonlm-reset", None),
                None,
                &[
                    "chatCreated",
                    "userMessage",
                    "loading",
                    "loaded",
                    "replyStarted",
                    "content",
                    "error",
                ][..],
            ),
            (
                "unreachable",
                "Ollama can't be reached (nothing listens on its port).",
                Target::Closed,
                ask(hello, "qwen3:0.6b", None),
                None,
                &["chatCreated", "userMessage", "error"][..],
            ),
            (
                "model-missing",
                "The model is gone when it is loaded (HTTP 404 from /api/generate, after /api/show found it): the send ends with that error, /api/chat is not asked.",
                Target::Ollama,
                ask(hello, "chatonlm-vanished", None),
                None,
                &["chatCreated", "userMessage", "loading", "loaded", "error"][..],
            ),
            (
                "http-error",
                "A proxy answers HTTP 502 with an empty body: the status alone is shown.",
                Target::Proxy,
                ask(hello, "qwen3:0.6b", None),
                None,
                &["chatCreated", "userMessage", "error"][..],
            ),
            (
                "refused-empty",
                "A message made only of spaces is refused: the command fails, no event.",
                Target::Ollama,
                ask("   ", "qwen3:0.6b", None),
                None,
                &[][..],
            ),
            (
                "download",
                "The model isn't installed: it is downloaded (progress of its largest layer, then done), loaded, then it answers.",
                Target::Ollama,
                ask(hello, "chatonlm-pull", None),
                None,
                &[
                    "chatCreated",
                    "userMessage",
                    "download",
                    "loading",
                    "loaded",
                    "replyStarted",
                    "content",
                    "done",
                ][..],
            ),
            (
                "download-offline",
                "The model isn't installed and the registry can't be reached: the official offline message.",
                Target::Ollama,
                ask(hello, "chatonlm-pull-offline", None),
                None,
                &["chatCreated", "userMessage", "error"][..],
            ),
            (
                "download-cancelled",
                "Download stopped by the user after three progress events; no answer is created.",
                Target::Ollama,
                ask(hello, "chatonlm-pull-stall", None),
                Some(("download", 3)),
                &["chatCreated", "userMessage", "download", "cancelled"][..],
            ),
            (
                "attachments",
                "New chat with a text file and an image, sent to a vision model: the user message carries its attachments.",
                Target::Ollama,
                with_files(
                    &["notes.txt", "red.png"],
                    ask(describe, "qwen3.5:0.8b", Some(Think::Enabled(false))),
                ),
                None,
                &[
                    "chatCreated",
                    "userMessage",
                    "loading",
                    "loaded",
                    "replyStarted",
                    "content",
                    "done",
                ][..],
            ),
            (
                "refused-unknown-attachment",
                "An attachment that is no longer staged (removed, or the page was reloaded): the command fails, no event, nothing saved.",
                Target::Ollama,
                with_files(
                    &["notes.txt", "staged-unknown"],
                    ask(describe, "qwen3.5:0.8b", Some(Think::Enabled(false))),
                ),
                None,
                &[][..],
            ),
            (
                "edit",
                "Edit of message 1 in the chat filled by the \"attachments\" sequence (attachments 1 notes.txt and 2 red.png, answer 2): notes.txt kept, red.png removed, extra.md added. The answer is deleted, the edited message (same id) comes back with its attachments, then a new answer. Only the edit's events.",
                Target::Ollama,
                editing(
                    1,
                    Some(vec![1]),
                    with_files(
                        &["extra.md"],
                        ask(instead, "qwen3.5:0.8b", Some(Think::Enabled(false))),
                    ),
                ),
                None,
                &["userMessage", "replyStarted", "content", "done"][..],
            ),
            (
                "refused-edit-unknown-message",
                "Edit of a message that is not saved (unknown id) in the chat filled by the \"attachments\" sequence: the command fails, no event, nothing changed.",
                Target::Ollama,
                editing(
                    99,
                    None,
                    ask(instead, "qwen3.5:0.8b", Some(Think::Enabled(false))),
                ),
                None,
                &[][..],
            ),
            (
                "web-search",
                "Web search with an Ollama account linked to the server: a first answer made of thinking and a web_search call, its result, then a second answer that quotes it.",
                Target::SignedIn,
                search(latest),
                None,
                searched,
            ),
            (
                "web-search-signed-out",
                "Web search while the server has no Ollama account (checked with /api/me before answering): the send stops with a webSearchSignedOut error, no answer is created.",
                Target::Ollama,
                search(latest),
                None,
                &["chatCreated", "userMessage", "error"][..],
            ),
            (
                "web-search-signed-out-late",
                "Web search while the account could not be checked (/api/me failed): the model calls web_search, the server refuses it (401), the send stops with a webSearchSignedOut error; the call stays without result or error.",
                Target::AccountUnknown,
                search(latest),
                None,
                &[
                    "chatCreated",
                    "userMessage",
                    "loading",
                    "loaded",
                    "replyStarted",
                    "thinking",
                    "toolCalls",
                    "error",
                ][..],
            ),
            (
                "web-search-cancelled",
                "Web search stopped by the user while the tool runs: the call stays without result or error.",
                Target::SignedIn,
                search("Search slowly for the latest Ollama release."),
                Some(("toolCalls", 1)),
                &[
                    "chatCreated",
                    "userMessage",
                    "loading",
                    "loaded",
                    "replyStarted",
                    "thinking",
                    "toolCalls",
                    "cancelled",
                ][..],
            ),
            (
                "loading",
                "New chat with a model that isn't in memory and takes 1.5 s to load: loading, loaded, then thinking and the answer.",
                Target::Ollama,
                ask(hello, "chatonlm-slowload", None),
                None,
                &[
                    "chatCreated",
                    "userMessage",
                    "loading",
                    "loaded",
                    "replyStarted",
                    "thinking",
                    "content",
                    "done",
                ][..],
            ),
        ];
        // Identifiants des fichiers préparés, dans les séquences.
        const STAGED: [&str; 2] = ["staged-1", "staged-2"];

        for (name, description, target, mut request, cancel_after, expected) in cases {
            let mut texts = Vec::new();
            let files = request.attachment_ids.clone();
            for _ in 0..2 {
                let store = Store::open_in_memory().unwrap();
                let staging = attachment_tests::staging();
                let (_server, url) = start(target).await;
                let mut replace = vec![(url.clone(), SEQUENCE_URL)];
                if let Some(files) = &files {
                    let mut ids = Vec::new();
                    for (index, file) in files.iter().enumerate() {
                        let Some(data) = sample_file(file) else {
                            ids.push(file.clone());
                            continue;
                        };
                        let id = stage(&staging, file, &data).await;
                        replace.push((id.clone(), STAGED[index]));
                        ids.push(id);
                    }
                    request.attachment_ids = Some(ids);
                }
                if name == "existing-chat" {
                    // Conversation existante : un premier échange, puis l'envoi
                    // enregistré.
                    let first = ask(hello, "qwen3:0.6b", None);
                    let (events, result) = play(&store, &staging, &url, &first, None).await;
                    result.unwrap();
                    let chat_id = events[0]["chatId"].as_str().unwrap().to_owned();
                    replace.push((chat_id.clone(), "chat-1"));
                    request.chat_id = Some(chat_id);
                }
                if request.edit_message_id.is_some() {
                    // Conversation remplie par l'envoi de la séquence
                    // « attachments », puis la modification enregistrée.
                    let mut ids = Vec::new();
                    for file in ["notes.txt", "red.png"] {
                        ids.push(stage(&staging, file, &sample_file(file).unwrap()).await);
                    }
                    let first = SendRequest {
                        attachment_ids: Some(ids),
                        ..ask(describe, "qwen3.5:0.8b", Some(Think::Enabled(false)))
                    };
                    let (events, result) = play(&store, &staging, &url, &first, None).await;
                    result.unwrap();
                    assert_eq!(events.last().unwrap()["type"], json!("done"), "{name}");
                    let chat_id = events[0]["chatId"].as_str().unwrap().to_owned();
                    replace.push((chat_id.clone(), "chat-1"));
                    request.chat_id = Some(chat_id);
                }
                let (events, result) = play(&store, &staging, &url, &request, cancel_after).await;
                assert_eq!(types(&events), expected, "{name}");
                if expected.is_empty() {
                    assert!(matches!(result, Err(AppError::Invalid(_))), "{name}");
                } else {
                    result.as_ref().unwrap();
                    assert_one_final(&events);
                }
                texts.push(sequence_text(
                    description,
                    &request,
                    events,
                    &result,
                    &replace,
                ));
            }
            assert_eq!(texts[0], texts[1], "{name} is not deterministic");
            export_sequence(name, &texts[0]);
        }
    }

    const TEST_INSTANCE: &str = "http://127.0.0.1:11435";

    /// Attente d'un premier fragment de la vraie instance (modèle à charger).
    const REAL_WAIT: Duration = Duration::from_secs(60);

    /// Contrat : une vraie réponse, raisonnement puis texte.
    #[tokio::test]
    #[ignore = "instance Ollama de test : scripts/test-ollama.sh start"]
    async fn contract_answer_from_the_test_instance() {
        let (store, engine) = (Store::open_in_memory().unwrap(), ChatEngine::default());
        let (channel, recorder) = recorder();
        let real = Server {
            url: TEST_INSTANCE.into(),
            context_length: 4096,
            keep_alive: 0,
        };
        let think = Some(Think::Enabled(true));
        send(
            &store,
            &engine,
            &Staging::default(),
            real,
            request(None, "qwen3:0.6b", think),
            &channel,
        )
        .await
        .unwrap();
        let events = recorder.events();
        assert_one_final(&events);
        assert_eq!(events.last().unwrap()["type"], json!("done"), "{events:?}");
        let reply = &saved(&store, &recorder.chat_id()).messages[1];
        assert!(!reply.content.is_empty());
        assert!(!reply.thinking.is_empty());
    }

    /// Contrat : Ollama refuse (400) une image envoyée à un modèle sans
    /// « vision » (qwen3:0.6b) ; ChatonLM la retire donc pour un modèle dont les
    /// capacités connues n'ont pas « vision » (écart à l'officiel, qui
    /// l'envoie toujours) : la réponse arrive, l'image reste jointe au message.
    /// Un modèle vision (qwen3.5:0.8b) la reçoit et la voit.
    #[tokio::test]
    #[ignore = "instance Ollama de test : scripts/test-ollama.sh start"]
    async fn contract_images_and_vision() {
        let red = samples::png(64, 64, [255, 0, 0]);
        let question = "What color is this image? Answer in one word.";
        let url = Url::parse(TEST_INSTANCE).unwrap();
        let raw = client::client(&url, Some(REAL_WAIT))
            .unwrap()
            .post(format!("{TEST_INSTANCE}/api/chat"))
            .json(&json!({
                "model": "qwen3:0.6b",
                "stream": false,
                "think": false,
                "options": {"num_ctx": 4096},
                "messages": [{"role": "user", "content": question, "images": [BASE64.encode(&red)]}],
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(raw.status(), 400);
        let refusal = raw.text().await.unwrap();
        assert!(
            refusal.contains("model does not support multimodal requests"),
            "{refusal}"
        );

        let (store, engine) = (Store::open_in_memory().unwrap(), ChatEngine::default());
        let staging = attachment_tests::staging();
        for model in ["qwen3:0.6b", "qwen3.5:0.8b"] {
            let mut ask = request(None, model, Some(Think::Enabled(false)));
            ask.content = question.into();
            ask.attachment_ids = Some(vec![stage(&staging, "red.png", &red).await]);
            let (channel, recorder) = recorder();
            // Même contexte que les autres contrats : pas de rechargement.
            let real = Server {
                url: TEST_INSTANCE.into(),
                context_length: 4096,
                keep_alive: 0,
            };
            send(&store, &engine, &staging, real, ask, &channel)
                .await
                .unwrap();
            let events = recorder.events();
            assert_one_final(&events);
            assert_eq!(
                events.last().unwrap()["type"],
                json!("done"),
                "{model}: {events:?}"
            );
            let chat = saved(&store, &recorder.chat_id());
            assert_eq!(chat.messages[0].attachments.len(), 1, "{model}");
            let answer = chat.messages[1].content.to_lowercase();
            eprintln!("{model} answered: {answer:?}");
            if model == "qwen3.5:0.8b" {
                assert!(answer.contains("red"), "{model}: {answer}");
            }
        }
    }

    /// Contrat : l'annulation arrête la génération (journal d'Ollama).
    #[tokio::test]
    #[ignore = "instance Ollama de test : scripts/test-ollama.sh start"]
    async fn contract_cancel_stops_the_test_instance() {
        let log = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.tools/ollama-test/serve.log");
        // Ollama écrit l'une ou l'autre selon le moteur d'exécution. Lecture
        // tolérante : llama.cpp journalise des chaînes du tokenizer coupées
        // au milieu d'un caractère (UTF-8 invalide), et `read_to_string`
        // rendrait alors toujours un texte vide.
        let aborted = || {
            let bytes = std::fs::read(&log).unwrap_or_default();
            let text = String::from_utf8_lossy(&bytes);
            text.matches("aborting chat request due to client closing the connection")
                .count()
                + text.matches("stop: cancel task").count()
        };
        let before = aborted();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let engine = Arc::new(ChatEngine::default());
        let (channel, recorder) = recorder();
        let mut long = request(None, "qwen3:0.6b", Some(Think::Enabled(false)));
        long.content = "Count from 1 to 500, one number per line.".into();
        let task = {
            let (store, engine) = (store.clone(), engine.clone());
            // Même contexte que contract_answer_from_the_test_instance, lancé
            // en parallèle : un autre forcerait Ollama à recharger le modèle,
            // ce qui tue le moteur annulé avant qu'il n'écrive l'abandon.
            let real = Server {
                url: TEST_INSTANCE.into(),
                context_length: 4096,
                keep_alive: 0,
            };
            tokio::spawn(async move {
                send(&store, &engine, &Staging::default(), real, long, &channel).await
            })
        };
        let what = "a first piece of answer";
        wait_until(REAL_WAIT, what, || recorder.count("content") > 0).await;
        engine.cancel("stream-qwen3:0.6b");
        finished(task).await;
        let events = recorder.events();
        assert_one_final(&events);
        assert_eq!(events.last().unwrap()["type"], json!("cancelled"));
        // Les contrats tournent en parallèle contre la même instance : Ollama
        // peut traiter l'abandon après d'autres requêtes (plus de 5 s sous
        // charge, check.sh --full du 7 oct. 2026).
        wait_until(REAL_WAIT, "the aborted request in Ollama's log", || {
            aborted() > before
        })
        .await;
    }

    /// Contrat : téléchargement de qwen3:0.6b, déjà installé et à jour (rien
    /// à télécharger, seul le manifeste est relu), lu jusqu'à `success`. Une
    /// couche incomplète arrêterait aussitôt le téléchargement : aucun poids
    /// n'est tiré.
    #[tokio::test]
    #[ignore = "instance Ollama de test : scripts/test-ollama.sh start"]
    async fn contract_pull_of_an_installed_model() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (stop, mut cancel) = watch::channel(false);
        let sink = events.clone();
        let outcome = pull::pull(TEST_INSTANCE, "qwen3:0.6b", &mut cancel, &move |event| {
            if let ChatEvent::Download {
                total, completed, ..
            } = event
                && completed < total
            {
                let _ = stop.send(true);
            }
            sink.lock().push(event);
        })
        .await;
        let events = events.lock().clone();
        let Outcome::Done = outcome else {
            panic!("the pull did not succeed: {events:?}");
        };
        let (last, progress) = events.split_last().expect("download events");
        let ChatEvent::Download {
            total,
            completed,
            done: true,
        } = *last
        else {
            panic!("last event: {last:?}");
        };
        // Plus grosse couche : les poids (environ 520 Mo).
        assert!(total > 100_000_000, "{total}");
        assert_eq!(completed, total);
        assert!(!progress.is_empty());
        for event in progress {
            assert_eq!(
                *event,
                ChatEvent::Download {
                    total,
                    completed: total,
                    done: false,
                }
            );
        }
    }

    /// Contrat : un nom absent du registre passe par /api/show (404), puis par
    /// un téléchargement qui échoue avec le message brut d'Ollama ; aucune
    /// réponse n'est créée.
    #[tokio::test]
    #[ignore = "instance Ollama de test : scripts/test-ollama.sh start"]
    async fn contract_a_model_missing_from_the_registry() {
        let (store, engine) = (Store::open_in_memory().unwrap(), ChatEngine::default());
        let (channel, recorder) = recorder();
        let real = Server {
            url: TEST_INSTANCE.into(),
            context_length: 0,
            keep_alive: 0,
        };
        let ask = request(None, "chatonlm-does-not-exist-xyz", None);
        send(&store, &engine, &Staging::default(), real, ask, &channel)
            .await
            .unwrap();
        let events = recorder.events();
        assert_one_final(&events);
        assert_eq!(
            types(&events),
            ["chatCreated", "userMessage", "error"],
            "{events:?}"
        );
        let last = events.last().unwrap();
        assert_eq!(last["kind"], json!("download"));
        assert_eq!(
            last["message"],
            json!("pull model manifest: file does not exist")
        );
        assert_eq!(saved(&store, &recorder.chat_id()).messages.len(), 1);
    }

    /// Envoi complet à l'instance de test : événements reçus.
    async fn sent_to_the_test_instance(store: &Store, model: &str, real: Server) -> Vec<Value> {
        let (channel, recorder) = recorder();
        let (engine, staging) = (ChatEngine::default(), Staging::default());
        let ask = request(None, model, None);
        send(store, &engine, &staging, real, ask, &channel)
            .await
            .unwrap();
        let events = recorder.events();
        assert_one_final(&events);
        assert_eq!(events.last().unwrap()["type"], json!("done"), "{events:?}");
        events
    }

    /// Durée du chargement d'Ollama pour une requête de chat telle que
    /// ChatonLM l'envoie (`load_duration`, réponse en un bloc).
    async fn load_duration(model: &str, context_length: u32, keep_alive: i64) -> Duration {
        let turn = Turn {
            role: Role::User,
            content: "Hi".into(),
            thinking: String::new(),
            files: Vec::new(),
            tool_calls: Vec::new(),
        };
        let mut body =
            stream::request_body(model, &[turn], None, context_length, keep_alive, true, None);
        body["stream"] = json!(false);
        let url = Url::parse(TEST_INSTANCE).unwrap();
        let answer: Value = client::client(&url, Some(REAL_WAIT))
            .unwrap()
            .post(format!("{TEST_INSTANCE}/api/chat"))
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        Duration::from_nanos(answer["load_duration"].as_u64().unwrap())
    }

    /// Copie d'un modèle de l'instance de test dont les réponses sont bornées
    /// (`num_predict`, comme dans un Modelfile), supprimée à la fin du contrat,
    /// même en échec (les couches restent au modèle d'origine).
    ///
    /// ChatonLM n'envoie aucune limite de longueur, et SmolLM2 (135 M) boucle
    /// parfois (« hello hello… ») : Ollama coupe au-delà de 100 jetons
    /// identiques d'affilée (« prediction aborted, token repeat limit
    /// reached ») et l'envoi finit en erreur. Une réponse bornée sous ce seuil
    /// finit toujours, et la requête reste celle de production.
    struct BoundedCopy(&'static str);

    impl BoundedCopy {
        /// Sous le seuil de répétition d'Ollama (100 jetons).
        const NUM_PREDICT: u32 = 32;

        async fn of(source: &str, name: &'static str) -> Self {
            let url = Url::parse(TEST_INSTANCE).unwrap();
            let created: Value = client::client(&url, Some(REAL_WAIT))
                .unwrap()
                .post(format!("{TEST_INSTANCE}/api/create"))
                .json(&json!({
                    "model": name,
                    "from": source,
                    "parameters": {"num_predict": Self::NUM_PREDICT},
                    "stream": false,
                }))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(created["status"], json!("success"), "{created}");
            let copy = Self(name);
            // La borne est bien celle de la copie (lignes « nom valeur »).
            let shown: Value = client::client(&url, Some(REAL_WAIT))
                .unwrap()
                .post(format!("{TEST_INSTANCE}/api/show"))
                .json(&json!({ "model": name }))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let limit = Self::NUM_PREDICT.to_string();
            let parameters = shown["parameters"].as_str().unwrap_or_default();
            let bounded = parameters
                .lines()
                .any(|line| line.split_whitespace().eq(["num_predict", limit.as_str()]));
            assert!(bounded, "{shown}");
            copy
        }
    }

    impl Drop for BoundedCopy {
        fn drop(&mut self) {
            let name = self.0;
            let deleted = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async {
                    let url = Url::parse(TEST_INSTANCE).unwrap();
                    client::client(&url, Some(REAL_WAIT))
                        .unwrap()
                        .delete(format!("{TEST_INSTANCE}/api/delete"))
                        .json(&json!({ "model": name }))
                        .send()
                        .await
                        .map(|response| response.status().is_success())
                })
            })
            .join();
            if !matches!(deleted, Ok(Ok(true))) {
                eprintln!("{name} not deleted: {deleted:?}");
            }
        }
    }

    /// Contrat : chargement annoncé contre l'instance de test. Un contexte
    /// réglé au-delà de celui du modèle (SmolLM2 : 8192) est ramené par
    /// Ollama : annoncé une fois, jamais rechargé ensuite. Un modèle chargé
    /// par `running::load` (Load, préchargement) avec les réglages du chat
    /// n'est ni annoncé ni rechargé au message suivant ; `keep_alive` -1 le
    /// garde.
    #[tokio::test]
    #[ignore = "instance Ollama de test : scripts/test-ollama.sh start"]
    async fn contract_loading_before_the_answer() {
        use running::tests::{CONTRACT_MODELS, Unloading, listed, unloaded};
        let _alone = CONTRACT_MODELS.lock().await;
        let model = "chatonlm-contract-smollm2";
        // Déclarée avant `Unloading` : déchargée, puis supprimée.
        let _copy =
            BoundedCopy::of("hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M", model).await;
        let _unloading = Unloading(model);
        let store = Store::open_in_memory().unwrap();
        let real = |context_length, keep_alive| Server {
            url: TEST_INSTANCE.into(),
            context_length,
            keep_alive,
        };
        // Un vrai chargement prend des secondes, sans rechargement ~2 ms.
        let no_reload = Duration::from_millis(500);

        unloaded(model).await;
        let details = models::show(TEST_INSTANCE, model).await.unwrap();
        assert_eq!(details.max_context, Some(8192));
        assert!(!details.remote);
        let first = sent_to_the_test_instance(&store, model, real(32768, 120)).await;
        assert_eq!(
            types(&first)[..4],
            ["chatCreated", "userMessage", "loading", "loaded"]
        );
        assert_eq!(listed(model).await.unwrap().context_length, Some(8192));
        let again = sent_to_the_test_instance(&store, model, real(32768, 120)).await;
        assert!(!types(&again).contains(&"loading".to_owned()), "{again:?}");
        let loading = load_duration(model, 32768, 120).await;
        assert!(loading < no_reload, "{loading:?}");

        for (context_length, keep_alive) in [(0, 120), (4096, -1)] {
            let case = format!("{context_length} {keep_alive}");
            unloaded(model).await;
            running::load(TEST_INSTANCE, model, context_length, keep_alive)
                .await
                .unwrap();
            let settings = real(context_length, keep_alive);
            let events = sent_to_the_test_instance(&store, model, settings).await;
            assert!(!types(&events).contains(&"loading".to_owned()), "{case}");
            let loading = load_duration(model, context_length, keep_alive).await;
            assert!(loading < no_reload, "{case}: {loading:?}");
            let running = listed(model).await.unwrap();
            assert_eq!(running.kept, keep_alive < 0, "{case}: {running:?}");
        }
        unloaded(model).await;
    }
}
