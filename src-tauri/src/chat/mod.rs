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
//! - l'annulation interrompt l'attente du chargement, de la connexion et la
//!   lecture (le flux lâché ferme la connexion : `llama-server` arrête la
//!   génération) ;
//! - moteur llama.cpp (lots L3 à L5a) : les détails du modèle viennent de la
//!   base locale ([`crate::llm::models::details`]) ; un modèle de Hugging
//!   Face absent d'un envoi de la page est d'abord téléchargé par la file du
//!   ModelHub (`pull.rs`, comme l'officiel qui tirait le modèle au premier
//!   envoi), un modèle repris d'Ollama ou d'un envoi venu d'ailleurs ne l'est
//!   jamais (`modelMissing`) ; `think` est retiré si le modèle ne l'accepte
//!   pas (capacités, valeurs de réflexion) ;
//! - chargement annoncé (propre à ChatonLM), une fois par envoi : le modèle
//!   est ouvert par [`crate::llm::Access::open`] (contexte et durée en
//!   mémoire des réglages) ; s'il n'est pas déjà prêt avec ces options,
//!   `loading` avant l'attente et `loaded` après, réussi ou non (un échec
//!   suit en `error`) ; un arrêt pendant le chargement finit l'envoi sans
//!   `loaded` (le chargement continue dans le moteur). La session ouverte (et
//!   son `Lease`) est tenue jusqu'à la fin de l'envoi, outils compris : le
//!   crochet des images, qui libère les modèles inactifs, ne décharge pas
//!   celui du chat ; une coupure après un déchargement demandé (bouton,
//!   suppression, sortie) est un arrêt, pas une erreur ;
//! - modèle distant (`@remote/…`, lot L7) : refusé sans rien envoyer tant
//!   que le réglage `remoteModels` est coupé (`remoteOff`) ; ses refus (clé,
//!   limite, injoignable) ont leur code (`remote*`) ;
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
//! - écart à l'officiel : les images ne partent pas vers un modèle sans la
//!   capacité « vision » (l'officiel les envoie toujours ; `llama-server`
//!   sans projecteur refuse alors toute la requête, 500 « image input is not
//!   supported », à chaque message suivant de la conversation) ;
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
//!   résultat ni erreur ; un outil qui demande l'arrêt de l'envoi
//!   (`Executed::Stop` : quota, pause ou version du relais, lot L7) le finit
//!   avec son code, l'appel restant sans résultat ni erreur. La réponse
//!   vivante superposée par `get_chat` porte
//!   ses appels et les résultats reçus. Écarts à l'officiel : au plus
//!   10 tours d'appels par envoi, puis une requête sans outils (l'officiel
//!   boucle sans limite ; le middleware du serveur s'arrête à 10) ; appels et
//!   résultats enregistrés avec la réponse (table `tool_calls`), pas en
//!   messages `tool` séparés (historique envoyé identique) ; appels reçus
//!   d'une requête sans outils ignorés ; pas d'outils « browser » propres à
//!   gpt-oss ; pas d'exclusion du raisonnement et de la recherche (règle
//!   officielle propre à deepseek-v3.1, modèle cloud) ;
//! - pré-prompt (réglage `system_prompt`, propre à ChatonLM) : s'il n'est pas
//!   vide, il part en premier message `system` de chaque requête de tout
//!   envoi (page, mode vocal, Easy Link), tours d'outils compris, sans jamais
//!   être enregistré ; la consigne repris du `SYSTEM` d'un modèle d'Ollama
//!   n'est alors plus ajoutée (règle d'Ollama, `Session::apply_defaults`) ;
//!   la consigne vocale et le résumé d'un compactage le suivent dans le même
//!   message ;
//! - envoi du mode conversation vocale (`voice`, propre à ChatonLM) : la
//!   consigne vocale (réglage, sinon celle par défaut) part en premier
//!   message `system` de chaque requête de l'envoi, tours
//!   d'outils compris, sans jamais être enregistrée ; l'observateur reçoit
//!   la demande avec chaque événement (lecture de la réponse, séance) ;
//! - envoi d'un bot d'Easy Link ([`Origin::Remote`], propre à ChatonLM) : même
//!   moteur et même historique, mais ses événements vont au bot et non à la
//!   page ; l'observateur ne les voit pas (une réponse destinée à une
//!   messagerie n'est pas lue à voix haute sur l'ordinateur), et un
//!   rechargement de la page ne l'arrête pas ;
//! - outil `generate_image` (propre à ChatonLM, voir `images.rs`) : offert
//!   si les détails du modèle sont connus avec la capacité `tools` et si le
//!   peintre (moteur d'images, modèle choisi dans le ModelHub) est prêt
//!   (su sans détecter la carte graphique, regardée seulement au moment d'une
//!   image), pour un envoi de la page ou d'Easy Link mais jamais du mode
//!   vocal (chaque phrase prononcée arrêterait l'image en cours sans
//!   prévenir), indépendamment de la recherche web (les deux jeux d'outils
//!   partent dans le même tableau `tools`) et des pièces jointes. Un appel :
//!   avancement annoncé (`imageProgress`, rien en base), image enregistrée en
//!   pièce jointe de la réponse et annoncée (`imageReady`), puis le résultat
//!   (JSON : pièce jointe, taille, graine, modèle) écrit et annoncé
//!   (`toolResult`). Description absente, plus de 4 images dans l'envoi,
//!   échec du moteur : erreur donnée au modèle, l'envoi continue ; arrêt
//!   pendant l'image, ou demandé pendant qu'elle finissait : ni enregistrée
//!   ni annoncée, l'appel reste sans résultat ni erreur. Un outil appelé sans
//!   avoir été offert reçoit « unknown tool ». Pour un modèle connu sans la
//!   capacité `tools` (gabarits qui ignorent appels et messages `tool`), les
//!   images de l'historique sont dites en une ligne de texte ajoutée à la
//!   réponse (`[Image created: …]`), voir `stream::image_calls_as_text` ;
//!   la réponse en cours d'un bot d'Easy Link est désignée à la page par
//!   `reply_running` (superposition de `get_chat`) ;
//! - bouton « Image » (`image`, page, propre à ChatonLM) : refus d'emblée
//!   (rien d'écrit) avec des pièces jointes ou si le peintre n'est pas prêt ;
//!   sinon le message de l'utilisateur est écrit, puis, sans aucune requête au
//!   moteur (le modèle de l'envoi n'est pas exigé, gardé tel quel sur la
//!   réponse), une réponse faite d'un appel `generate_image` avec le texte
//!   tel quel, exécuté comme ci-dessus, et `done` (une erreur d'image reste
//!   sur l'appel).

pub(crate) mod call;
mod compact;
pub(crate) mod images;
pub(crate) mod pull;
pub(crate) mod repetition;
pub mod stream;
pub(crate) mod tools;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt as _;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tokio::sync::watch;
use ts_rs::TS;

use crate::attachments::{Staging, Taken};
use crate::error::{AppError, AppResult};
use crate::hub::errors::connection_lost;
use crate::i18n::t;
use crate::inference::{self, Event};
use crate::llm::models::ModelDetails;
use crate::llm::{Access, LlmError, OpenOptions, Prefs, Session, ids};
use crate::store::chats::{
    self, AttachmentInfo, Chat, Compaction, ContextUsage, Counted, Message, Reply, Sendable,
    ToolCall,
};
use crate::store::settings::Settings;
use crate::store::{Store, model_prefs, now_ms};
use crate::tts::VoiceLanguage;
pub use images::{ImageProgress, ImageStage};
use images::{Painter, Ready};
use repetition::LoopWatch;
use stream::{Ending, StreamToolCall, Think};
use tools::{AllowedUrls, Executed};

/// Intervalle des écritures en base pendant une réponse.
const SAVE_INTERVAL: Duration = Duration::from_millis(500);

/// Délai laissé à une réponse annulée pour s'enregistrer et se terminer.
const CANCEL_WAIT: Duration = Duration::from_secs(3);

/// Tours d'appels d'outils au plus par envoi ; la requête suivante part sans
/// outils (comme `maxWebSearchLoops` du serveur).
const MAX_TOOL_TURNS: usize = 10;

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
    /// Bouton « Image » (propre à ChatonLM) : le texte part tel quel au
    /// modèle d'images, sans modèle de langage ; absent = non.
    #[ts(optional)]
    pub image: Option<bool>,
    /// Nouvelle conversation éphémère (propre à ChatonLM) : absente de
    /// l'historique, effacée quand on la quitte ; seulement sans `chatId`,
    /// ignoré sinon ; absent = non.
    #[ts(optional)]
    pub ephemeral: Option<bool>,
}

/// Envoi d'une phrase du mode conversation vocale.
#[derive(Clone, Debug, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct VoiceSend {
    /// Langue de la phrase transcrite (indice de la voix) ; absente = inconnue.
    #[ts(optional)]
    pub language: Option<VoiceLanguage>,
    /// Séance tenue par l'extension VS Code (propre à ChatonLM) : consigne
    /// de son profil et contexte de l'éditeur, ajoutés au message système
    /// après la consigne vocale, jamais enregistrés. Posé par le moteur
    /// seulement (ni lu de la page, ni exporté).
    #[serde(skip)]
    #[ts(skip)]
    pub system: Option<String>,
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
    /// Pas de moteur (`llama-server` absent de l'installation).
    EngineUnavailable,
    /// Le moteur n'a pas pu servir le modèle (démarrage, plantage, délai,
    /// connexion locale impossible).
    EngineFailed,
    /// Connexion coupée pendant la réponse.
    ConnectionLost,
    /// Modèle absent (base locale, fichier, serveur).
    ModelMissing,
    /// Échec du téléchargement du modèle.
    Download,
    /// Erreur renvoyée par le modèle (image refusée, réponse illisible…).
    Model,
    /// Modèle distant alors que le réglage `remoteModels` est coupé : rien
    /// n'est envoyé (propre à ChatonLM).
    RemoteOff,
    /// Clé du fournisseur distant refusée (401, 403).
    RemoteKey,
    /// Limite du fournisseur distant atteinte (402, 429).
    RemoteLimit,
    /// Fournisseur distant injoignable.
    RemoteUnreachable,
    /// Recherche web impossible (relais injoignable, refus).
    WebSearchUnavailable,
    /// Quota de recherches de l'installation atteint (relais).
    WebSearchLimit,
    /// Recherche web en pause (budget du relais épuisé, service suspendu).
    WebSearchPaused,
    /// Version de l'app trop ancienne pour le relais.
    WebSearchOutdated,
    /// Réponse arrêtée : le modèle répétait mot pour mot le même passage
    /// (propre à ChatonLM, voir `repetition.rs`).
    Loop,
    /// Réponse coupée : le contexte du modèle est plein (propre à ChatonLM,
    /// voir `compact.rs`).
    ContextFull,
    /// Erreur de ChatonLM (base de données...).
    Internal,
}

/// Fin d'un envoi pour une erreur du moteur ou d'un fournisseur : son code
/// et son texte (langue du moment) ; un arrêt (annulation, modèle déchargé
/// à la demande) : [`Outcome::Cancelled`].
fn failed(error: &LlmError) -> Outcome {
    match call::failure(error) {
        Some((kind, message)) => Outcome::Failed(kind, message),
        None => Outcome::Cancelled,
    }
}

/// Fin d'un flux sans fin de réponse : serveur arrêté ou proxy coupé.
fn lost() -> Outcome {
    Outcome::Failed(ChatErrorKind::ConnectionLost, connection_lost(None))
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
    /// Progression du téléchargement du modèle par la file du ModelHub :
    /// octets reçus de tous ses fichiers (projecteur compris) sur leur total
    /// (0 tant qu'il est inconnu, voir `pull.rs`) ; le dernier avec
    /// `done: true` et completed == total.
    Download {
        total: u64,
        completed: u64,
        done: bool,
    },
    /// Le modèle n'est pas prêt (pas en mémoire, ou avec d'autres options) :
    /// chargement lancé avant la requête (propre à ChatonLM).
    Loading,
    /// Fin du chargement annoncé par `Loading`, réussi ou non (un échec suit
    /// en `Error`) ; toujours avant `ReplyStarted`.
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
    /// Avancement de l'image demandée par l'appel `index` (outil
    /// `generate_image`) d'une réponse (propre à ChatonLM).
    #[serde(rename_all = "camelCase")]
    ImageProgress {
        message_id: i64,
        index: u32,
        progress: ImageProgress,
    },
    /// Image produite par l'appel `index`, enregistrée en pièce jointe de la
    /// réponse, toujours avant le `ToolResult` de cet appel (propre à
    /// ChatonLM).
    #[serde(rename_all = "camelCase")]
    ImageReady {
        message_id: i64,
        index: u32,
        attachment: AttachmentInfo,
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
    /// Contexte consommé après une requête finie, taille connue (propre à
    /// ChatonLM) : avant l'événement final, et après chaque tour d'outils.
    Context {
        used: i64,
        size: i64,
    },
    /// Compactage automatique commencé, avant la réponse (propre à ChatonLM).
    Compacting,
    /// Compactage fait : le résumé et le contexte estimé qui le suit.
    Compacted {
        compaction: Compaction,
        context: Option<ContextUsage>,
    },
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
    /// Fait les images (outil `generate_image`, bouton « Image ») ; posé une
    /// fois au démarrage. Absent : aucune image possible.
    painter: std::sync::OnceLock<Arc<dyn Painter>>,
    /// Travail de l'app sur le moteur hors des réponses (Correction, Étude…),
    /// et réveil à chaque début et fin de réponse ou de travail : le chef
    /// de la carte graphique met l'autocomplétion en pause pendant ce temps
    /// (voir [`crate::gpu::activity`]).
    activity: Arc<crate::gpu::activity::Activity>,
}

type Observer = Box<dyn Fn(&str, &ChatEvent, Option<&VoiceSend>) + Send + Sync>;

struct Running {
    stream_id: String,
    origin: Origin,
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

    /// Pose le peintre des images (une fois, au démarrage).
    pub fn set_painter(&self, painter: Arc<dyn Painter>) {
        let _ = self.painter.set(painter);
    }

    pub(crate) fn painter(&self) -> Option<Arc<dyn Painter>> {
        self.painter.get().cloned()
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
        origin: Origin,
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
                origin,
                cancel,
                finished,
            },
        );
        drop(running);
        self.activity.changed().notify_waiters();
        Ok((cancelled, finish))
    }

    fn end(&self, chat_id: &str) {
        self.running.lock().remove(chat_id);
        self.live.lock().remove(chat_id);
        self.activity.changed().notify_waiters();
    }

    /// Réveillé à chaque début et fin de réponse, ou d'un autre travail
    /// tenu sur le moteur.
    pub fn activity(&self) -> &tokio::sync::Notify {
        self.activity.changed()
    }

    /// Compteur des travaux sur le moteur hors des réponses (posé au démarrage
    /// pour toute l'app, voir [`crate::gpu::activity::install`]).
    pub fn work(&self) -> Arc<crate::gpu::activity::Activity> {
        Arc::clone(&self.activity)
    }

    /// Tient un travail sur le moteur hors des réponses.
    pub fn hold(&self) -> crate::gpu::activity::Hold {
        self.activity.hold()
    }

    /// Une réponse s'écrit, ou un autre travail est tenu sur le moteur.
    pub fn is_working(&self) -> bool {
        self.is_busy() || self.activity.active()
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

    /// Demande l'arrêt des flux de la page (page rechargée : ils n'ont plus de
    /// destinataire). Ceux d'un bot d'Easy Link continuent.
    pub fn cancel_page(&self) {
        for running in self.running.lock().values() {
            if running.origin == Origin::Page {
                let _ = running.cancel.send(true);
            }
        }
    }

    /// Demande l'arrêt de tous les flux (fermeture de l'app).
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
    /// jour de l'état courant ne le perd pas. La réponse d'un bot d'Easy Link
    /// est en plus désignée par `reply_running` : la page n'a pas de flux pour
    /// elle (celles de la page sont les siennes, et arrêtées par
    /// [`ChatEngine::cancel_page`] si elle se recharge).
    pub fn overlay(&self, chat: &mut Chat) {
        let remote = self
            .running
            .lock()
            .get(&chat.id)
            .is_some_and(|running| running.origin.is_outside());
        if let Some((message_id, reply)) = self.live.lock().get(&chat.id)
            && let Some(message) = chat.messages.iter_mut().find(|m| m.id == *message_id)
        {
            chat.reply_running = remote.then_some(*message_id);
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

/// Réglages et accès utilisés pour l'envoi. Construit par
/// [`Server::current`] (commandes, Easy Link, pont) : le lot L5a change
/// l'intérieur, pas cette signature.
#[derive(Clone)]
pub struct Server {
    /// Accès aux modèles (moteur, distants, relais).
    pub access: Access,
    /// Réglages lus pour cet envoi (contexte et durée en mémoire : lus de
    /// nouveau par `Access::open`, à l'ouverture du modèle).
    pub prefs: Prefs,
}

impl Server {
    /// Réglages du moment et accès de l'app.
    pub async fn current(access: &Access) -> AppResult<Server> {
        let settings = access
            .store()
            .call(|connection| crate::store::settings::get(connection))
            .await?;
        Ok(Self::with_settings(access.clone(), &settings))
    }

    /// Accès donné, réglages déjà lus.
    pub fn with_settings(access: Access, settings: &Settings) -> Self {
        Self {
            access,
            prefs: Prefs::of(settings),
        }
    }

    /// Outils web de l'envoi (recherche, lecture de page).
    pub fn web_tools(&self) -> WebTools {
        WebTools {
            relay: self.access.relay().clone(),
        }
    }

    /// Refus d'un modèle distant (`remote`, ou identifiant `@remote/…`) quand
    /// le réglage `remoteModels` est coupé : texte de l'erreur, rien sinon.
    pub fn refuses(&self, model: &str, remote: bool) -> Option<String> {
        ((remote || ids::is_remote(model)) && !self.prefs.remote_models)
            .then(|| LlmError::RemoteOff.message())
    }
}

/// Outils web d'un envoi (`chat::tools::execute`, Étude) : recherche par le
/// relais de ChatonLM, lecture des pages par l'app (`tools.rs`).
#[derive(Clone)]
pub struct WebTools {
    pub relay: Arc<crate::relay::RelayClient>,
}

/// D'où vient un envoi.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// La page de l'app.
    Page,
    /// Un bot d'Easy Link : ni observateur, ni arrêt au rechargement de la
    /// page.
    Remote,
    /// L'extension VS Code, par le pont local : comme un bot, ni observateur,
    /// ni arrêt au rechargement de la page.
    Bridge,
}

impl Origin {
    /// Envoi venu d'ailleurs que la page (bot, éditeur) : la page n'a pas de
    /// flux pour lui, et il ne fait jamais télécharger de modèle.
    pub fn is_outside(self) -> bool {
        self != Origin::Page
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
    let sink = |event: ChatEvent| {
        let _ = events.send(event);
    };
    send_to(store, engine, staging, server, request, Origin::Page, &sink).await
}

/// [`send`], avec le destinataire des événements au choix (`sink`).
pub async fn send_to(
    store: &Store,
    engine: &ChatEngine,
    staging: &Staging,
    server: Server,
    request: SendRequest,
    origin: Origin,
    sink: &(dyn Fn(ChatEvent) + Send + Sync),
) -> AppResult<()> {
    // Comme l'officiel : un message fait seulement d'espaces est refusé, les
    // autres sont enregistrés et envoyés tels quels. Un texte est requis même
    // avec des pièces jointes.
    if request.content.trim().is_empty() {
        return Err(AppError::Invalid(t!("chat.emptyMessage")));
    }
    // Bouton « Image » : vérifié avant toute écriture ; le modèle de langage
    // n'est pas exigé.
    let painting = match request.image {
        Some(true) => Some(painter_ready(engine, &request).await?),
        _ => None,
    };
    let model = request.model.trim().to_owned();
    if model.is_empty() && painting.is_none() {
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
    // Envoi vocal de l'extension VS Code (séance tenue par le pont) :
    // observé aussi, pour que la séance suive son tour et que la réponse soit
    // lue à voix haute.
    let observed_origin = origin == Origin::Page || (origin == Origin::Bridge && voice.is_some());
    let emit = |event: ChatEvent| {
        if observed_origin {
            engine.observe(observed.get().map(String::as_str), &event, voice.as_ref());
        }
        sink(event);
    };

    let chat_id = match request.chat_id {
        Some(id) => id,
        None => {
            let ephemeral = request.ephemeral == Some(true);
            let id = store
                .call(move |c| chats::create_with(c, now_ms(), ephemeral))
                .await?;
            let _ = observed.set(id.clone());
            emit(ChatEvent::ChatCreated {
                chat_id: id.clone(),
            });
            id
        }
    };
    let _ = observed.set(chat_id.clone());
    let (mut cancel, finish) = engine.begin(&chat_id, &request.stream_id, origin)?;
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

    // Lus après `begin` : un arrêt demandé pendant la lecture n'est pas perdu.
    // Pré-prompt réglé (vide : aucun), puis, pour un envoi vocal, consigne
    // vocale réglée (vide : par défaut) : envoyés ensemble en message système
    // avec chaque requête, jamais enregistrés (écart à l'officiel, qui n'a pas
    // de consigne système dans le chat).
    let voice_system = match &voice {
        Some(_) => Some(voice_instructions(store).await),
        None => None,
    };
    let session_system = voice.as_ref().and_then(|voice| voice.system.clone());
    let system = joined_system([system_prompt(store).await, voice_system, session_system]);

    let mut reply = ReplyWriter {
        chat_id: chat_id.clone(),
        model: model.clone(),
        message_id: None,
        previous: None,
        reply: Reply::default(),
        last_save: Instant::now(),
        think_off: false,
        always_thinks_noted: false,
        loops: LoopWatch::default(),
    };
    let conversation = Conversation {
        store,
        engine,
        emit: &emit,
        origin,
    };
    let prompt = Prompt {
        content: request.content,
        files,
        edit,
    };
    let asking = Asking {
        think: request.think,
        images: true,
        // Pas d'image pour un envoi vocal : la phrase suivante l'arrêterait.
        paint: voice.is_none(),
        calls_as_text: false,
        system,
    };
    let web_search = request.web_search == Some(true);
    let outcome = match painting {
        Some(ready) => {
            conversation
                .paint_only(&server, prompt, ready, &mut reply, &mut cancel)
                .await
        }
        None => {
            conversation
                .run(&server, prompt, asking, web_search, &mut reply, &mut cancel)
                .await
        }
    };
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

/// Bouton « Image » : peintre prêt, ou refus d'emblée (pièces jointes,
/// moteur absent, processeur, aucun modèle d'images).
async fn painter_ready(engine: &ChatEngine, request: &SendRequest) -> AppResult<Ready> {
    let attached = request
        .attachment_ids
        .as_ref()
        .is_some_and(|ids| !ids.is_empty())
        || request
            .kept_attachment_ids
            .as_ref()
            .is_some_and(|ids| !ids.is_empty());
    if attached {
        return Err(AppError::Invalid(t!("chat.imageWithFiles")));
    }
    let Some(painter) = engine.painter() else {
        return Err(AppError::Invalid(
            images::PaintRefusal::Unavailable.message(),
        ));
    };
    painter
        .ready()
        .await
        .map_err(|refusal| AppError::Invalid(refusal.message()))
}

/// Taille du contexte d'un modèle sans l'ouvrir (propre à ChatonLM ; pont
/// de l'extension VS Code) : celle de son processus s'il est chargé ; sinon
/// celle du réglage, bornée par le maximum du modèle ; inconnue sinon (le
/// moteur la choisit au chargement d'après la mémoire) et pour un modèle
/// distant. Un envoi prend celle de sa session ([`Session::context`]).
pub(crate) async fn context_size(server: &Server, model: &str) -> Option<i64> {
    if ids::is_remote(model) {
        return None;
    }
    let running = server.access.engine().running();
    if let Some(loaded) = running
        .iter()
        .find(|loaded| crate::local_models::id::same_model(&loaded.id, model))
    {
        return Some(i64::from(loaded.context_length));
    }
    let setting = server.prefs.context_length;
    if setting == 0 {
        return None;
    }
    let max = crate::local_models::get(server.access.store(), server.access.data_dir(), model)
        .await
        .ok()
        .flatten()
        .and_then(|local| local.context_max);
    Some(i64::from(max.map_or(setting, |max| setting.min(max))))
}

/// Compacte une conversation à la demande (propre à ChatonLM, voir
/// `compact.rs`) avec `model` : le modèle résume ce qu'une requête enverrait,
/// jusqu'au dernier message. La conversation est occupée pendant ce temps
/// (un envoi est refusé). Refus : modèle absent, conversation occupée, rien à
/// compacter ; échec : moteur en panne ou erreur du modèle.
pub async fn compact_chat(
    store: &Store,
    engine: &ChatEngine,
    server: Server,
    chat_id: &str,
    model: &str,
) -> AppResult<Compaction> {
    let model = model.trim();
    if model.is_empty() {
        return Err(AppError::Invalid(t!("chat.noModel")));
    }
    let (mut cancel, finish) =
        engine.begin(chat_id, &format!("compact:{chat_id}"), Origin::Page)?;
    let result = compact_now(store, &server, chat_id, model, &mut cancel).await;
    engine.end(chat_id);
    let _ = finish.send(true);
    result
}

async fn compact_now(
    store: &Store,
    server: &Server,
    chat_id: &str,
    model: &str,
    cancel: &mut watch::Receiver<bool>,
) -> AppResult<Compaction> {
    let id = chat_id.to_owned();
    let (sendable, last, counted) = store
        .call(move |c| {
            let last = chats::last_message_id(c, &id)?
                .ok_or_else(|| AppError::Invalid(t!("chat.nothingToCompact")))?;
            Ok((chats::sendable(c, &id)?, last, chats::counted(c, &id)?))
        })
        .await?;
    if sendable.turns.is_empty() {
        return Err(AppError::Invalid(t!("chat.nothingToCompact")));
    }
    // Modèle distant, réglage coupé : rien n'est demandé, ni envoyé.
    if let Some(message) = server.refuses(model, false) {
        return Err(AppError::Invalid(message));
    }
    // Absent : refusé (le compactage ne télécharge rien).
    let details = crate::llm::models::details(&server.access, model).await?;
    let think = call::accepted_think(Some(Think::Enabled(false)), &details);
    let images = call::sees(&details);
    let mut sendable = sendable;
    if !call::uses_tools(&details) {
        sendable.turns = stream::image_calls_as_text(&sendable.turns).into_owned();
    }
    let stopped = || AppError::Invalid(t!("chat.compactStopped"));
    let linked = call::linked(cancel);
    let session = match server
        .access
        .open(model, OpenOptions::default(), linked.token(), &|| {})
        .await
    {
        Ok(session) => session,
        Err(LlmError::Cancelled) => return Err(stopped()),
        Err(error) => return Err(error.into()),
    };
    // Taille du processus ; distant : celle qu'il annonce, sinon inconnue.
    let size = session.context.map(i64::from);
    // Message système fixe des envois suivants : le pré-prompt (une page
    // n'envoie la consigne vocale qu'avec un message vocal).
    let system = compact::system_tokens(system_prompt(store).await.as_deref());
    let scale = compact::calibration(counted, &sendable, system).unwrap_or(1.0);
    let (summary, count) = compact::summarize(
        &session,
        &sendable,
        think,
        images,
        size,
        scale,
        linked.token(),
    )
    .await
    .map_err(|outcome| match outcome {
        Outcome::Failed(_, message) => AppError::Engine(message),
        Outcome::Cancelled | Outcome::Done => stopped(),
    })?;
    // Tout résumé : jusqu'au dernier message (messages vides compris) ;
    // sinon jusqu'au dernier message résumé.
    let through = if count == sendable.turns.len() {
        last
    } else {
        sendable.ids[count - 1]
    };
    // Contexte estimé qui suit : le résumé et les messages non résumés.
    let counted =
        size.map(|size| compact::after_compaction(&sendable, &summary, count, scale, size, system));
    let id = chat_id.to_owned();
    store
        .call(move |c| chats::add_compaction(c, &id, through, &summary, false, counted, now_ms()))
        .await
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
    /// Boucle sûre du modèle dans la réponse du tour (propre à ChatonLM).
    loops: LoopWatch,
}

impl ReplyWriter {
    /// Après un tour d'outils : le tour suivant répondra dans un nouveau
    /// message, créé à son premier fragment.
    fn next_turn(&mut self) {
        self.previous = self.message_id.take().or(self.previous);
        self.reply = Reply::default();
        self.last_save = Instant::now();
        self.loops = LoopWatch::default();
    }
}

/// Réglages de la requête, communs aux tours d'un envoi.
struct Asking {
    /// Réflexion demandée (forme de la page), gardée si le modèle l'accepte.
    think: Option<Think>,
    /// Images des pièces jointes envoyées (capacité « vision »).
    images: bool,
    /// Outil `generate_image` permis (pas pour un envoi du mode vocal :
    /// chaque phrase prononcée arrêterait l'image en cours sans prévenir).
    paint: bool,
    /// Modèle sans la capacité `tools` : ses gabarits ignorent les appels et
    /// les messages `tool` ; les images faites lui sont dites en texte (voir
    /// [`stream::image_calls_as_text`]).
    calls_as_text: bool,
    /// Message système placé en tête de chaque requête (pré-prompt, puis
    /// consigne vocale).
    system: Option<String>,
}

/// Message de l'utilisateur à écrire avant la requête.
struct Prompt<'a> {
    content: String,
    files: Taken<'a>,
    /// Message existant à modifier ; absent = nouveau message.
    edit: Option<Edit>,
}

struct Conversation<'a, E: Fn(ChatEvent)> {
    store: &'a Store,
    engine: &'a ChatEngine,
    emit: &'a E,
    origin: Origin,
}

/// Images de l'envoi (outil `generate_image`).
struct ImageCalls {
    /// Peintre prêt : l'outil est offert.
    ready: Option<Ready>,
    /// Images demandées jusqu'ici (au plus [`images::MAX_IMAGES_PER_SEND`]).
    count: usize,
}

/// Outils offerts à un envoi : recherche web et image, dans le même tableau ;
/// aucun : `None`.
fn tool_definitions(web: bool, image: bool) -> Option<serde_json::Value> {
    let mut definitions = Vec::new();
    if web && let serde_json::Value::Array(web) = tools::definitions() {
        definitions.extend(web);
    }
    if image {
        definitions.push(images::definition());
    }
    (!definitions.is_empty()).then_some(serde_json::Value::Array(definitions))
}

/// Erreur d'un outil appelé sans avoir été offert (texte de l'officiel).
fn unknown_tool(name: &str) -> Executed {
    // i18n-ignore : texte de l'officiel, donné au modèle.
    Executed::Error(format!("unknown tool: {name}"))
}

impl<E: Fn(ChatEvent) + Sync> Conversation<'_, E> {
    /// Écrit le message de l'utilisateur (nouveau, ou modifié) avec ses
    /// pièces jointes, puis l'annonce. Erreur : fin déjà décidée.
    async fn write_prompt(&self, chat_id: &str, prompt: Prompt<'_>) -> Result<Message, Outcome> {
        let chat_id = chat_id.to_owned();
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
        let user = written.map_err(Outcome::from)?;
        (self.emit)(ChatEvent::UserMessage {
            message: user.clone(),
        });
        Ok(user)
    }

    /// Bouton « Image » : message de l'utilisateur écrit, puis, sans
    /// requête au modèle, une réponse faite d'un appel `generate_image` avec
    /// le texte tel quel.
    async fn paint_only(
        &self,
        server: &Server,
        prompt: Prompt<'_>,
        ready: Ready,
        reply: &mut ReplyWriter,
        cancel: &mut watch::Receiver<bool>,
    ) -> Outcome {
        let user = match self.write_prompt(&reply.chat_id, prompt).await {
            Ok(user) => user,
            Err(outcome) => return outcome,
        };
        let call = StreamToolCall {
            name: images::TOOL_NAME.to_owned(),
            arguments: serde_json::json!({ "prompt": user.content }),
        };
        let mut images = ImageCalls {
            ready: Some(ready),
            count: 0,
        };
        let mut allowed = AllowedUrls::default();
        match self
            .run_tools(
                &server.web_tools(),
                &[call],
                &mut allowed,
                false,
                &mut images,
                reply,
                cancel,
            )
            .await
        {
            Ok(()) => Outcome::Done,
            Err(outcome) => outcome,
        }
    }

    async fn run(
        &self,
        server: &Server,
        prompt: Prompt<'_>,
        mut asking: Asking,
        web_search: bool,
        reply: &mut ReplyWriter,
        cancel: &mut watch::Receiver<bool>,
    ) -> Outcome {
        let user = match self.write_prompt(&reply.chat_id, prompt).await {
            Ok(user) => user,
            Err(outcome) => return outcome,
        };
        // Comme l'officiel (`hasAttachments`) : pas d'outils web pour un
        // message avec des pièces jointes.
        let has_attachments = !user.attachments.is_empty();
        let user_id = user.id;

        let mut sendable = match self.history(&reply.chat_id).await {
            Ok(sendable) => sendable,
            Err(error) => return error.into(),
        };
        // Contexte occupé compté à la fin d'un envoi précédent (inconnu
        // après une modification de message).
        let chat_id = reply.chat_id.clone();
        let mut counted = match self.store.call(move |c| chats::counted(c, &chat_id)).await {
            Ok(counted) => counted,
            Err(error) => return error.into(),
        };

        // Modèle distant, réglage coupé : rien ne part, rien n'est demandé.
        if server.refuses(&reply.model, false).is_some() {
            return failed(&LlmError::RemoteOff);
        }
        let details = match self.prepare(server, &reply.model, cancel).await {
            Ok(details) => details,
            Err(outcome) => return outcome,
        };
        asking.think = call::accepted_think(asking.think, &details);
        asking.images = call::sees(&details);
        let tools_accepted = call::uses_tools(&details);
        asking.calls_as_text = !tools_accepted;
        let web = web_search && !has_attachments && tools_accepted;
        // Outil d'image : offert si le peintre est prêt (relu à chaque envoi,
        // sans détecter la carte graphique), jamais à un envoi vocal.
        let mut images = ImageCalls {
            ready: None,
            count: 0,
        };
        if tools_accepted
            && asking.paint
            && let Some(painter) = self.engine.painter()
        {
            images.ready = tokio::select! {
                biased;
                () = cancelled(cancel) => return Outcome::Cancelled,
                ready = painter.ready() => ready.ok(),
            };
        }
        let definitions = tool_definitions(web, images.ready.is_some());
        // Une fois par envoi, pas à chaque tour d'outils ; tenue jusqu'à la
        // fin de l'envoi (le modèle reste chargé pendant les outils).
        let session = match self.open(server, &reply.model, cancel).await {
            Ok(session) => session,
            Err(outcome) => return outcome,
        };
        // Taille du contexte du modèle de cet envoi (propre à ChatonLM) :
        // celle du processus ; distant sans taille annoncée : ni compactage,
        // ni limite, ni jauge.
        let size = session.context.map(i64::from);
        // Occupé avant le nouveau message ; passé 80 % : compactage de ce qui
        // le précède. Un échec n'empêche pas la réponse.
        // Estimations corrigées par le compte du moteur, s'il y en a un.
        // Message système fixe (pré-prompt, consigne vocale) compris : le
        // compte du moteur le comprend ; un compactage ne le réduit pas.
        let system = compact::system_tokens(asking.system.as_deref());
        let mut scale =
            compact::calibration(counted, &sendable, system).unwrap_or(compact::UNCALIBRATED);
        let mut occupied = compact::occupied(counted, &sendable, 1, scale, system);
        if let Some(size) = size
            && compact::needs_compaction(Some(ContextUsage {
                used: occupied,
                size,
            }))
            && sendable.turns.len() > 1
            && compact::compaction_relieves(size, compact::scaled(system, scale))
        {
            // Résumé sans raisonnement si le modèle sait le couper.
            let think = call::accepted_think(Some(Think::Enabled(false)), &details);
            match self
                .compact_before(
                    &session, &sendable, think, &asking, size, counted, reply, cancel,
                )
                .await
            {
                Ok(()) => {
                    sendable = match self.history(&reply.chat_id).await {
                        Ok(sendable) => sendable,
                        Err(error) => return error.into(),
                    };
                    let chat_id = reply.chat_id.clone();
                    counted = match self.store.call(move |c| chats::counted(c, &chat_id)).await {
                        Ok(counted) => counted,
                        Err(error) => return error.into(),
                    };
                    scale = compact::calibration(counted, &sendable, system)
                        .unwrap_or(compact::UNCALIBRATED);
                    occupied = compact::occupied(counted, &sendable, 1, scale, system);
                }
                Err(Outcome::Cancelled) => return Outcome::Cancelled,
                Err(Outcome::Failed(_, message)) => {
                    crate::app_log!("chatonlm: automatic compaction failed: {message}");
                }
                Err(Outcome::Done) => {}
            }
        }
        // Adresses que `web_fetch` peut lire : celles de tous les messages de
        // l'utilisateur (compactés compris, jamais celles d'un résumé écrit
        // par le modèle), puis celles reçues pendant l'envoi.
        let chat_id = reply.chat_id.clone();
        let mut allowed = match self
            .store
            .call(move |c| chats::user_texts(c, &chat_id))
            .await
        {
            Ok(texts) => AllowedUrls::from_text(&texts),
            Err(error) => return error.into(),
        };
        let mut tool_turns = 0;
        loop {
            let offered = definitions.as_ref().filter(|_| tool_turns < MAX_TOOL_TURNS);
            // Ce que la requête ajoute : le nouveau message, ou les
            // résultats des outils du tour précédent.
            let added = sendable
                .turns
                .last()
                .map_or(0, |turn| compact::scaled(compact::turn_tokens(turn), scale));
            let limit = size.map(|size| compact::reply_limit(size, occupied + added));
            let (calls, ending) = match self
                .ask(
                    &session,
                    &sendable,
                    &mut asking,
                    &allowed,
                    offered,
                    limit,
                    reply,
                    cancel,
                )
                .await
            {
                Ok(answer) => answer,
                Err(outcome) => return outcome,
            };
            if let Some(Ending {
                counts: Some((prompt, eval)),
                ..
            }) = ending
            {
                occupied = compact::occupied_after(prompt, eval, &reply.reply);
                // Compté jusqu'à cette réponse ; taille inconnue (modèle du
                // cloud) : le compte d'un autre modèle est oublié.
                let through = reply.message_id.or(reply.previous).unwrap_or(user_id);
                let counted = size.map(|size| Counted {
                    usage: ContextUsage {
                        used: occupied,
                        size,
                    },
                    through,
                });
                let chat_id = reply.chat_id.clone();
                if let Err(error) = self
                    .store
                    .call(move |c| chats::set_context(c, &chat_id, counted))
                    .await
                {
                    return error.into();
                }
                if let Some(Counted { usage, .. }) = counted {
                    (self.emit)(ChatEvent::Context {
                        used: usage.used,
                        size: usage.size,
                    });
                }
            }
            // Réponse arrêtée par la limite posée : le contexte est plein.
            if limit.is_some() && ending.is_some_and(|ending| ending.length) {
                return Outcome::Failed(ChatErrorKind::ContextFull, t!("chat.contextFull"));
            }
            // Appels d'une requête sans outils : ignorés (rien ne les
            // exécuterait).
            if calls.is_empty() || offered.is_none() {
                return Outcome::Done;
            }
            tool_turns += 1;
            if let Err(outcome) = self
                .run_tools(
                    &server.web_tools(),
                    &calls,
                    &mut allowed,
                    web,
                    &mut images,
                    reply,
                    cancel,
                )
                .await
            {
                return outcome;
            }
            reply.next_turn();
            sendable = match self.history(&reply.chat_id).await {
                Ok(sendable) => sendable,
                Err(error) => return error.into(),
            };
        }
    }

    /// Compactage automatique de ce qui précède le dernier message (celui
    /// envoyé) : `Compacting`, résumé du début qui tient dans le contexte,
    /// enregistrement, `Compacted` (avec le contexte occupé estimé qui suit).
    /// Erreur : échec (l'envoi continue) ou arrêt.
    #[allow(clippy::too_many_arguments)]
    async fn compact_before(
        &self,
        session: &Session,
        sendable: &Sendable,
        think: Option<Think>,
        asking: &Asking,
        size: i64,
        counted: Option<Counted>,
        reply: &ReplyWriter,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<(), Outcome> {
        (self.emit)(ChatEvent::Compacting);
        let kept = sendable.turns.len() - 1;
        let turns = &sendable.turns[..kept];
        let before = Sendable {
            summary: sendable.summary.clone(),
            through: sendable.through,
            turns: if asking.calls_as_text {
                stream::image_calls_as_text(turns).into_owned()
            } else {
                turns.to_vec()
            },
            ids: sendable.ids[..kept].to_vec(),
        };
        let system = compact::system_tokens(asking.system.as_deref());
        let scale = compact::calibration(counted, &before, system).unwrap_or(1.0);
        let linked = call::linked(cancel);
        let (summary, count) = compact::summarize(
            session,
            &before,
            think,
            asking.images,
            Some(size),
            scale,
            linked.token(),
        )
        .await?;
        let through = before.ids[count - 1];
        // Contexte estimé qui suit : le résumé et les messages non résumés,
        // compté jusqu'au dernier message d'avant celui envoyé.
        let counted = compact::after_compaction(&before, &summary, count, scale, size, system);
        let chat_id = reply.chat_id.clone();
        let compaction = self
            .store
            .call(move |c| {
                chats::add_compaction(
                    c,
                    &chat_id,
                    through,
                    &summary,
                    true,
                    Some(counted),
                    now_ms(),
                )
            })
            .await?;
        (self.emit)(ChatEvent::Compacted {
            compaction,
            context: Some(counted.usage),
        });
        Ok(())
    }

    /// Ce que la requête suivante enverra (après le dernier compactage).
    async fn history(&self, chat_id: &str) -> AppResult<Sendable> {
        let chat_id = chat_id.to_owned();
        self.store.call(move |c| chats::sendable(c, &chat_id)).await
    }

    /// Une requête de chat en flux, lue jusqu'à sa fin : appels d'outils
    /// reçus pendant le tour, et fin de la réponse. Résumé du dernier
    /// compactage en message système (après le pré-prompt et la consigne
    /// vocale) ; réponse limitée à `limit` jetons s'il est donné ; résultats
    /// des recherches de cet envoi avec leurs extraits (`allowed`). Erreur :
    /// fin déjà décidée.
    #[allow(clippy::too_many_arguments)]
    async fn ask(
        &self,
        session: &Session,
        sendable: &Sendable,
        asking: &mut Asking,
        allowed: &AllowedUrls,
        tools: Option<&serde_json::Value>,
        limit: Option<i64>,
        reply: &mut ReplyWriter,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<(Vec<StreamToolCall>, Option<Ending>), Outcome> {
        let summary = sendable.summary.as_deref().map(compact::summary_message);
        let system = joined_system([asking.system.clone(), summary]);
        let turns = if asking.calls_as_text {
            stream::image_calls_as_text(&sendable.turns)
        } else {
            std::borrow::Cow::Borrowed(sendable.turns.as_slice())
        };
        reply.think_off = asking.think == Some(Think::Enabled(false));
        let mut messages =
            call::with_system(stream::messages(&turns, asking.images), system.as_deref());
        // Extraits des recherches de cet envoi rendus au modèle (l'historique
        // n'en garde que titres et liens).
        tools::give_excerpts(&mut messages, allowed);
        let mut request = session.request(messages);
        request.think = call::think_request(asking.think.as_ref());
        if let Some(serde_json::Value::Array(tools)) = tools {
            request.tools.clone_from(tools);
        }
        request.max_tokens = limit.and_then(|limit| u32::try_from(limit).ok());
        let linked = call::linked(cancel);
        let opened = tokio::select! {
            biased;
            () = cancelled(cancel) => return Err(Outcome::Cancelled),
            opened = crate::llm::chat_stream(session, &request, linked.token().clone()) => opened,
        };
        match opened {
            Ok(stream) => self.read(session, stream, reply, cancel).await,
            Err(error) => Err(failed(&error)),
        }
    }

    /// Fin d'un tour avec des appels : réponse créée si besoin, appels
    /// enregistrés puis annoncés, dernière écriture du tour, puis exécution
    /// dans l'ordre (chaque résultat écrit puis annoncé). `web` : outils de
    /// la recherche web offerts. Erreur : fin déjà décidée (arrêt pendant un
    /// outil : l'appel reste sans résultat).
    #[allow(clippy::too_many_arguments)]
    async fn run_tools(
        &self,
        web_tools: &WebTools,
        received: &[StreamToolCall],
        allowed: &mut AllowedUrls,
        web: bool,
        images: &mut ImageCalls,
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
            let executed = match call.name.as_str() {
                images::TOOL_NAME if images.ready.is_some() => {
                    self.paint(message_id, index, &call.arguments, images, cancel)
                        .await?
                }
                name @ (tools::WEB_SEARCH | tools::WEB_FETCH) if !web => unknown_tool(name),
                name => tools::execute(web_tools, name, &call.arguments, allowed, cancel).await,
            };
            let (result, error) = match executed {
                Executed::Result(result) => (Some(result), None),
                Executed::Error(error) => (None, Some(error)),
                Executed::Cancelled => return Err(Outcome::Cancelled),
                Executed::Stop(kind, message) => return Err(Outcome::Failed(kind, message)),
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

    /// Exécute un appel `generate_image` (outil offert) de la réponse
    /// `message_id` : avancement annoncé, image enregistrée en pièce jointe
    /// de la réponse puis annoncée ; résultat (JSON) ou erreur pour le
    /// modèle. Arrêt : `Executed::Cancelled` (futur du peintre lâché).
    /// Erreur : fin décidée (écriture impossible).
    async fn paint(
        &self,
        message_id: i64,
        index: u32,
        arguments: &serde_json::Value,
        images: &mut ImageCalls,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Executed, Outcome> {
        let (Some(ready), Some(painter)) = (images.ready.clone(), self.engine.painter()) else {
            return Ok(unknown_tool(images::TOOL_NAME));
        };
        let Some(prompt) = images::prompt_argument(arguments) else {
            return Ok(Executed::Error(t!("chat.imageNoPrompt")));
        };
        if images.count >= images::MAX_IMAGES_PER_SEND {
            return Ok(Executed::Error(t!(
                "chat.imageTooMany",
                max = images::MAX_IMAGES_PER_SEND
            )));
        }
        images.count += 1;
        // Carte graphique détectée maintenant seulement (pas à chaque envoi).
        let cpu = tokio::select! {
            biased;
            () = cancelled(cancel) => return Ok(Executed::Cancelled),
            cpu = painter.cpu() => cpu,
        };
        let (width, height) = images::image_size(arguments, ready.side(cpu));
        let emit = self.emit;
        // Place réservée tout de suite, avant la première nouvelle du moteur.
        emit(ChatEvent::ImageProgress {
            message_id,
            index,
            progress: ImageProgress::at(ImageStage::Starting, cpu, width, height),
        });
        let mut progress = |progress: ImageProgress| {
            emit(ChatEvent::ImageProgress {
                message_id,
                index,
                progress,
            });
        };
        let request = images::PaintRequest {
            prompt: prompt.to_owned(),
            width,
            height,
        };
        let painted = tokio::select! {
            biased;
            () = cancelled(cancel) => return Ok(Executed::Cancelled),
            painted = painter.paint(request, &mut progress) => painted,
        };
        let mut painted = match painted {
            Ok(painted) => painted,
            Err(error) => return Ok(Executed::Error(error)),
        };
        // Arrêt demandé pendant que l'image finissait : ni enregistrée, ni
        // annoncée (un bot d'Easy Link l'enverrait encore).
        if *cancel.borrow() {
            return Ok(Executed::Cancelled);
        }
        let (png, seed) = (std::mem::take(&mut painted.png), painted.seed);
        let saved = self
            .store
            .call(move |c| chats::add_generated_image(c, message_id, &png, seed, now_ms()))
            .await;
        let attachment = match saved {
            Ok(attachment) => attachment,
            // Trop grande : erreur donnée au modèle.
            Err(AppError::Invalid(message)) => return Ok(Executed::Error(message)),
            Err(error) => return Err(error.into()),
        };
        let result = images::result_text(attachment.id, &painted);
        emit(ChatEvent::ImageReady {
            message_id,
            index,
            attachment,
        });
        Ok(Executed::Result(result))
    }

    /// Détails du modèle pour l'envoi (base locale, ou liste du fournisseur
    /// distant). Un modèle de Hugging Face absent d'un envoi de la page est
    /// d'abord téléchargé par la file du ModelHub, puis redemandé ; absent
    /// sinon (repris d'Ollama, envoi venu d'ailleurs, encore absent après le
    /// téléchargement) : `modelMissing`. Erreur : fin déjà décidée.
    async fn prepare(
        &self,
        server: &Server,
        model: &str,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<ModelDetails, Outcome> {
        let missing = || failed(&LlmError::NotInstalled(model.to_owned()));
        match crate::llm::models::details(&server.access, model).await {
            Ok(details) => return Ok(details),
            Err(AppError::NotFound(_)) => {}
            Err(error) => return Err(error.into()),
        }
        // Un bot d'Easy Link ou l'extension VS Code ne fait jamais télécharger
        // un modèle ; un modèle repris d'Ollama ou distant ne se télécharge
        // pas.
        let wanted = pull::downloadable(model).filter(|_| !self.origin.is_outside());
        let Some(wanted) = wanted else {
            return Err(missing());
        };
        match pull::download(&server.access, &wanted, cancel, self.emit).await {
            Outcome::Done => {}
            outcome => return Err(outcome),
        }
        match crate::llm::models::details(&server.access, model).await {
            Ok(details) => Ok(details),
            Err(AppError::NotFound(_)) => Err(missing()),
            Err(error) => Err(error.into()),
        }
    }

    /// Ouvre le modèle de l'envoi ([`Access::open`]) : chargement annoncé
    /// (propre à ChatonLM) par `Loading` avant l'attente s'il n'est pas déjà
    /// prêt avec les options de l'envoi, puis `Loaded` à la fin, réussie ou
    /// non (l'échec suit en `Error`). Arrêt pendant l'attente : fin sans
    /// `Loaded` (le chargement continue dans le moteur). Erreur : fin déjà
    /// décidée.
    async fn open(
        &self,
        server: &Server,
        model: &str,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Session, Outcome> {
        let announced = std::sync::atomic::AtomicBool::new(false);
        let emit = self.emit;
        // Sans moteur installé, `Access::open` n'annonce rien.
        let on_loading = || {
            if !announced.swap(true, std::sync::atomic::Ordering::SeqCst) {
                emit(ChatEvent::Loading);
            }
        };
        let linked = call::linked(cancel);
        let opened = server
            .access
            .open(model, OpenOptions::default(), linked.token(), &on_loading)
            .await;
        if matches!(opened, Err(LlmError::Cancelled)) {
            return Err(Outcome::Cancelled);
        }
        if announced.load(std::sync::atomic::Ordering::SeqCst) {
            emit(ChatEvent::Loaded);
        }
        opened.map_err(|error| failed(&error))
    }

    /// Lit le flux jusqu'à sa fin : appels d'outils reçus, dans l'ordre, et
    /// fin de la réponse. Flux lâché (arrêt, boucle) : la connexion se ferme
    /// et le serveur arrête la génération. Erreur : fin déjà décidée.
    async fn read(
        &self,
        session: &Session,
        mut stream: inference::ChatStream,
        reply: &mut ReplyWriter,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<(Vec<StreamToolCall>, Option<Ending>), Outcome> {
        let mut calls = Vec::new();
        loop {
            let next = tokio::select! {
                biased;
                () = cancelled(cancel) => return Err(Outcome::Cancelled),
                next = stream.next() => next,
            };
            let (thinking, content) = match next {
                Some(Ok(Event::Content(text))) => (String::new(), text),
                Some(Ok(Event::Reasoning(text))) => (text, String::new()),
                Some(Ok(Event::ToolCalls(received))) => {
                    calls.extend(received.into_iter().map(stream::StreamToolCall::from));
                    continue;
                }
                Some(Ok(Event::Done {
                    finish_reason,
                    usage,
                    timings,
                })) => {
                    return Ok((
                        calls,
                        Some(Ending::of(&finish_reason, usage.as_ref(), timings.as_ref())),
                    ));
                }
                Some(Err(error)) => return Err(failed(&session.error(error))),
                // Fin du flux sans fin de réponse : serveur arrêté ou coupé.
                None => return Err(lost()),
            };
            if let Err(error) = self.take(reply, &thinking, &content).await {
                return Err(error.into());
            }
            // Boucle sûre : flux abandonné (le serveur arrête alors la
            // génération), réponse reçue gardée.
            if reply.loops.push(&thinking, &content).is_some() {
                return Err(Outcome::Failed(ChatErrorKind::Loop, t!("chat.loopStopped")));
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
            crate::app_log!("chatonlm: voice mode settings not read: {error}");
            crate::voice_mode::DEFAULT_INSTRUCTIONS.to_owned()
        }
    }
}

/// Pré-prompt réglé, `None` s'il est vide (fait d'espaces) ou illisible.
async fn system_prompt(store: &Store) -> Option<String> {
    match store
        .call(|connection| crate::store::settings::get(connection))
        .await
    {
        Ok(settings) => Some(settings.system_prompt).filter(|text| !text.trim().is_empty()),
        Err(error) => {
            crate::app_log!("chatonlm: system prompt not read: {error}");
            None
        }
    }
}

/// Parties d'un message système, dans l'ordre, séparées par une ligne vide ;
/// `None` sans aucune partie.
fn joined_system(parts: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    let parts: Vec<String> = parts.into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

#[cfg(test)]
mod tests;
