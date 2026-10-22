//! Easy Link (propre à ChatonLM) : un bot Telegram ou Discord relie une
//! messagerie au modèle de l'app, pour s'en servir à distance.
//!
//! Les protocoles sont dans le crate `easy-link` ; ici, ce que l'app en fait :
//! - un bot par messagerie, relié par son jeton (vérifié, gardé en base,
//!   jamais renvoyé à la page), écouté tant qu'il est activé et que l'app
//!   tourne ;
//! - seuls les comptes liés par un code d'appairage sont servis
//!   ([`pairing`]) ; les autres reçoivent un avis, puis le silence ;
//! - un message d'un compte lié part au moteur de chat comme un message de la
//!   page ([`crate::chat::Origin::Remote`]) : même historique, conversation
//!   marquée de sa messagerie, réponse montrée dans la messagerie pendant
//!   qu'elle s'écrit ([`relay`]) ;
//! - à distance, on converse, on choisit, charge et décharge un modèle
//!   ([`commands`]) ; ni téléchargement, ni suppression, ni réglage de l'app.
//!
//! Les messages passent par les serveurs de la messagerie : l'interface le
//! dit. Contrat et décisions : `.tools/research/80-easy-link-contract.md`.

mod bot;
pub mod commands;
pub mod pairing;
mod relay;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use easy_link::{Event, Identity, Messenger, Problem};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, watch};
use ts_rs::TS;

use crate::attachments::Staging;
use crate::chat::ChatEngine;
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::easy_link::{self as saved, Bot, EasyLinkAccount, Platform};
use crate::store::{Store, now_ms};
use pairing::Pairings;

/// Événement : état d'Easy Link ([`EasyLinkState`]), à chaque changement.
pub const STATE_EVENT: &str = "easy-link";

/// Événement : une conversation a été créée ou modifiée hors de la page
/// ([`ChatsUpdated`]).
pub const CHATS_EVENT: &str = "chats-updated";

/// Délai de connexion à une messagerie (hôte d'Internet, pas la machine).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Début de l'identifiant des flux d'un bot dans le moteur de chat.
pub const STREAM_PREFIX: &str = "easy-link:";

/// Messages servis dont l'identifiant est retenu (voir `Shared::served`) :
/// largement plus que ce qu'une écoute remplacée peut avoir en route.
const SERVED_KEPT: usize = 256;

/// Pourquoi un bot ne reçoit pas ses messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum EasyLinkProblem {
    /// Jeton refusé : à remplacer.
    Unauthorized,
    /// Une autre app lit déjà les messages de ce bot.
    Conflict,
    /// Messagerie injoignable : nouvel essai automatique.
    Network,
    /// Autre refus de la messagerie.
    Refused,
}

impl From<&Problem> for EasyLinkProblem {
    fn from(problem: &Problem) -> Self {
        match problem {
            Problem::Unauthorized => EasyLinkProblem::Unauthorized,
            Problem::Conflict => EasyLinkProblem::Conflict,
            Problem::Network(_) => EasyLinkProblem::Network,
            Problem::Forbidden
            | Problem::RateLimited(_)
            | Problem::TooLarge
            | Problem::Refused { .. } => EasyLinkProblem::Refused,
        }
    }
}

/// Liaison d'un bot avec sa messagerie.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "state", rename_all = "camelCase")]
#[ts(export)]
pub enum EasyLinkLink {
    /// Pas de jeton, ou bot désactivé.
    Off,
    Connecting,
    Connected,
    Failed {
        problem: EasyLinkProblem,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EasyLinkBot {
    pub platform: Platform,
    /// Un jeton est enregistré.
    pub configured: bool,
    /// Quatre derniers caractères du jeton, vide sans jeton : le jeton
    /// lui-même ne revient jamais à la page.
    pub token_hint: String,
    pub enabled: bool,
    /// Nom d'utilisateur du bot, sans « @ » ; vide tant qu'il est inconnu.
    pub name: String,
    pub link: EasyLinkLink,
    /// Telegram : adresse du bot ; Discord : lien d'invitation du bot sur un
    /// serveur.
    pub url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EasyLinkPairing {
    pub platform: Platform,
    /// Six chiffres.
    pub code: String,
    pub expires_at: i64,
    /// Telegram : lien qui ouvre le bot et lui envoie le code.
    pub url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EasyLinkState {
    /// Toujours deux : Telegram, puis Discord.
    pub bots: Vec<EasyLinkBot>,
    pub accounts: Vec<EasyLinkAccount>,
    pub pairing: Option<EasyLinkPairing>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatsUpdated {
    pub chat_id: String,
}

/// Échec de la transcription d'un message vocal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HearError {
    /// Aucun modèle de transcription installé.
    NoModel,
    /// Texte déjà traduit du moteur de dictée.
    Failed(String),
}

/// Ce que le service demande à l'app (remplacé dans les essais).
pub trait Host: Send + Sync + 'static {
    fn engine(&self) -> &ChatEngine;
    /// L'état a changé : à montrer.
    fn changed(&self, state: &EasyLinkState);
    /// Une conversation a été créée ou modifiée par un bot.
    fn chat_updated(&self, chat_id: &str);
    /// Transcrit un message vocal (mono, 16 kHz) avec le moteur de dictée.
    fn transcribe(
        &self,
        samples: Vec<f32>,
    ) -> impl Future<Output = Result<String, HearError>> + Send;
}

/// Adresses des messageries.
#[derive(Clone, Debug)]
pub struct Urls {
    pub telegram: String,
    pub discord: String,
}

impl Urls {
    /// Adresses officielles ; `CHATONLM_TELEGRAM_URL` et
    /// `CHATONLM_DISCORD_URL` les remplacent dans les builds de débogage et
    /// d'essai seulement (faux serveurs).
    pub fn from_env() -> Self {
        Self {
            telegram: url_from(
                test_override("CHATONLM_TELEGRAM_URL"),
                easy_link::telegram::DEFAULT_URL,
            ),
            discord: url_from(
                test_override("CHATONLM_DISCORD_URL"),
                easy_link::discord::DEFAULT_URL,
            ),
        }
    }

    fn of(&self, platform: Platform) -> &str {
        match platform {
            Platform::Telegram => &self.telegram,
            Platform::Discord => &self.discord,
        }
    }
}

/// Jamais lue par le build de production : un environnement modifié enverrait
/// sinon le jeton d'un bot à un hôte quelconque.
#[cfg(any(test, debug_assertions, feature = "e2e"))]
fn test_override(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(not(any(test, debug_assertions, feature = "e2e")))]
fn test_override(_name: &str) -> Option<String> {
    None
}

fn url_from(value: Option<String>, default: &str) -> String {
    value
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

/// Nom de la messagerie, jamais traduit.
pub(crate) fn service_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Telegram => "Telegram",
        Platform::Discord => "Discord",
    }
}

/// Bot écouté.
struct Live {
    /// Distingue cette écoute de la précédente (bot relancé).
    id: u64,
    /// Jeton écouté : une écoute remplacée par une écoute du même bot (jeton
    /// recollé, bot réactivé) sert encore les messages qu'elle a en route.
    token: String,
    stop: watch::Sender<bool>,
    link: EasyLinkLink,
}

/// Réponse d'un compte, de la réception du message à la dernière écriture.
pub(crate) struct Claim {
    /// Flux du moteur de chat, connu de lui seulement une fois l'envoi parti.
    pub(crate) stream: String,
    /// Arrêt demandé (`/stop`, compte délié, bot coupé) : retenu ici parce
    /// que la préparation (transcription, photos) précède le flux, que le
    /// moteur ne peut donc pas encore arrêter.
    pub(crate) stopped: bool,
}

pub(crate) struct Shared<H: Host> {
    pub(crate) host: H,
    pub(crate) store: Store,
    /// Pièces jointes des bots : à part de celles de la page, qui sont vidées
    /// à chaque rechargement.
    pub(crate) staging: Staging,
    urls: Urls,
    live: Mutex<HashMap<Platform, Live>>,
    next_live: AtomicU64,
    pub(crate) pairings: Mutex<Pairings>,
    /// Réponse en cours (préparation comprise) par compte lié.
    pub(crate) busy: Mutex<HashMap<i64, Claim>>,
    /// Une commande de cycle de vie à la fois (relier, retirer, activer,
    /// démarrage) : leurs lectures et écritures ne se croisent pas.
    changing: tokio::sync::Mutex<()>,
    /// Images reçues sans texte, jointes au prochain message du compte.
    pub(crate) pending: Mutex<HashMap<i64, Vec<String>>>,
    /// Dernier avis « message arrivé trop tard » par compte lié.
    pub(crate) missed: Mutex<HashMap<i64, std::time::Instant>>,
    /// Derniers messages servis (messagerie, conversation, identifiant), les
    /// plus anciens d'abord : un message remis à l'écoute remplacée, puis de
    /// nouveau à la suivante (Telegram, s'il n'était pas encore acquitté),
    /// n'est servi qu'une fois.
    served: Mutex<VecDeque<(Platform, String, String)>>,
    closed: AtomicBool,
}

/// Service d'Easy Link (état géré par Tauri).
pub struct EasyLink<H: Host = AppHost>(Arc<Shared<H>>);

impl<H: Host> Clone for EasyLink<H> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<H: Host> EasyLink<H> {
    pub fn new(host: H, store: Store, urls: Urls) -> Self {
        Self(Arc::new(Shared {
            host,
            store,
            staging: Staging::default(),
            urls,
            live: Mutex::default(),
            next_live: AtomicU64::new(1),
            pairings: Mutex::default(),
            busy: Mutex::default(),
            changing: tokio::sync::Mutex::default(),
            pending: Mutex::default(),
            missed: Mutex::default(),
            served: Mutex::default(),
            closed: AtomicBool::new(false),
        }))
    }

    /// Au démarrage : écoute les bots reliés et activés.
    pub async fn start_saved(&self) {
        let _changing = self.0.changing.lock().await;
        let Ok(bots) = self.0.store.call(|c| saved::bots(c)).await else {
            return;
        };
        for bot in bots {
            if bot.enabled && !bot.token.is_empty() {
                let _ = self.0.start(bot.platform, &bot.token);
            }
        }
        self.0.publish().await;
    }

    pub async fn state(&self) -> AppResult<EasyLinkState> {
        self.0.state().await
    }

    /// Vérifie le jeton auprès de la messagerie, l'enregistre, active le bot
    /// et l'écoute.
    pub async fn connect(&self, platform: Platform, token: &str) -> AppResult<EasyLinkState> {
        let token = token.trim().to_owned();
        if token.is_empty() {
            return Err(AppError::Invalid(t!("easyLink.tokenEmpty")));
        }
        let _changing = self.0.changing.lock().await;
        let messenger = self.0.messenger(platform, &token)?;
        let identity = messenger
            .identity()
            .await
            .map_err(|problem| connect_error(platform, &problem))?;
        let saved_token = token.clone();
        self.0
            .store
            .call(move |c| saved::save_bot(c, platform, &saved_token, &identity.id, &identity.name))
            .await?;
        self.0.start(platform, &token)?;
        self.0.state().await
    }

    /// Oublie le bot, délie ses comptes et arrête leurs réponses.
    pub async fn disconnect(&self, platform: Platform) -> AppResult<EasyLinkState> {
        let _changing = self.0.changing.lock().await;
        self.0.stop(platform);
        self.0.pairings.lock().cancel_for(platform);
        let accounts = self.0.store.call(|c| saved::accounts(c)).await?;
        for account in accounts.iter().filter(|a| a.platform == platform) {
            self.0.forget_account(account.id);
            // Sa conversation éphémère ne survit pas au compte.
            self.0.forget_ephemeral_chat_or_log(account.id).await;
        }
        self.0
            .store
            .call(move |c| saved::forget_bot(c, platform))
            .await?;
        self.0.state().await
    }

    pub async fn set_enabled(&self, platform: Platform, enabled: bool) -> AppResult<EasyLinkState> {
        let _changing = self.0.changing.lock().await;
        let bot = self
            .0
            .store
            .call(move |c| {
                saved::set_enabled(c, platform, enabled)?;
                saved::bot(c, platform)
            })
            .await?;
        if enabled && !bot.token.is_empty() {
            self.0.start(platform, &bot.token)?;
        } else {
            self.0.stop(platform);
            self.0.pairings.lock().cancel_for(platform);
            // Bot coupé : ses réponses en cours s'arrêtent aussi (plus
            // personne ne pourrait envoyer `/stop`).
            let accounts = self.0.store.call(|c| saved::accounts(c)).await?;
            for account in accounts.iter().filter(|a| a.platform == platform) {
                self.0.stop_answer(account.id);
            }
        }
        self.0.state().await
    }

    /// Nouveau code d'appairage pour `platform` (un seul à la fois).
    pub async fn pair(&self, platform: Platform) -> AppResult<EasyLinkState> {
        let bot = self.0.store.call(move |c| saved::bot(c, platform)).await?;
        if bot.token.is_empty() || !bot.enabled {
            return Err(AppError::Invalid(t!("easyLink.noBot")));
        }
        let pairing = self.0.pairings.lock().start(platform, now_ms());
        // À l'expiration, la page apprend que le code n'est plus là.
        let shared = Arc::clone(&self.0);
        tauri::async_runtime::spawn(async move {
            let wait = u64::try_from(pairing.expires_at - now_ms()).unwrap_or(0);
            tokio::time::sleep(Duration::from_millis(wait + 50)).await;
            shared.publish().await;
        });
        self.0.state().await
    }

    pub async fn cancel_pairing(&self) -> AppResult<EasyLinkState> {
        self.0.pairings.lock().cancel();
        self.0.state().await
    }

    /// Délie un compte, arrête sa réponse en cours et efface sa conversation
    /// éphémère.
    pub async fn unlink(&self, account_id: i64) -> AppResult<EasyLinkState> {
        self.0.forget_account(account_id);
        self.0.forget_ephemeral_chat_or_log(account_id).await;
        self.0
            .store
            .call(move |c| saved::unlink(c, account_id))
            .await?;
        self.0.state().await
    }

    /// Fermeture de l'app : plus d'écoute, plus de nouvelle réponse (celles en
    /// cours sont arrêtées et enregistrées par le moteur de chat).
    pub fn shutdown(&self) {
        self.0.closed.store(true, Ordering::Relaxed);
        let live: Vec<Live> = {
            let mut live = self.0.live.lock();
            self.0.closed.store(true, Ordering::Relaxed);
            live.drain().map(|(_, live)| live).collect()
        };
        for live in live {
            let _ = live.stop.send(true);
        }
    }

    #[cfg(test)]
    pub(crate) fn shared(&self) -> &Arc<Shared<H>> {
        &self.0
    }
}

impl<H: Host> Shared<H> {
    fn messenger(&self, platform: Platform, token: &str) -> AppResult<Messenger> {
        let base = self.urls.of(platform);
        let url = crate::ollama::client::server_url(base)?;
        // Client de l'app : proxy du système, fournisseur TLS, agent.
        let http = crate::ollama::client::builder(&url, None)
            .connect_timeout(CONNECT_TIMEOUT)
            // Aucune redirection suivie : l'adresse d'un fichier vient d'un
            // message, et ne doit mener qu'à l'hôte vérifié par le crate.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| AppError::Internal(error.without_url().to_string()))?;
        Ok(match platform {
            Platform::Telegram => {
                Messenger::Telegram(easy_link::telegram::Client::new(http, base, token))
            }
            Platform::Discord => Messenger::Discord(easy_link::discord::Client::new(
                http,
                base,
                token,
                gateway_tls(),
            )),
        })
    }

    /// Écoute le bot de `platform` (à la place de l'écoute précédente).
    fn start(self: &Arc<Self>, platform: Platform, token: &str) -> AppResult<()> {
        let messenger = self.messenger(platform, token)?;
        let (stop, stopped) = watch::channel(false);
        let id = self.next_live.fetch_add(1, Ordering::Relaxed);
        let previous = {
            let mut live = self.live.lock();
            // Sous le verrou : une fermeture ne laisse pas d'écoute derrière
            // elle.
            if self.closed.load(Ordering::Relaxed) {
                return Ok(());
            }
            live.insert(
                platform,
                Live {
                    id,
                    token: token.to_owned(),
                    stop,
                    link: EasyLinkLink::Connecting,
                },
            )
        };
        if let Some(previous) = previous {
            let _ = previous.stop.send(true);
        }
        let (events, mut received) = mpsc::channel(32);
        let listener = messenger.clone();
        tauri::async_runtime::spawn(async move { listener.listen(events, stopped).await });
        let (shared, token) = (Arc::clone(self), token.to_owned());
        tauri::async_runtime::spawn(async move {
            while let Some(event) = received.recv().await {
                match event {
                    Event::Connected(identity) => shared.connected(platform, id, identity).await,
                    Event::Disconnected(problem) => {
                        let link = EasyLinkLink::Failed {
                            problem: EasyLinkProblem::from(&problem),
                        };
                        if shared.set_link(platform, id, link) {
                            shared.publish().await;
                        }
                    }
                    Event::Message(incoming) => {
                        // Écoute remplacée par celle d'un autre bot, ou bot
                        // coupé : rien n'est servi. Remplacée par une écoute
                        // du même bot : ce message, peut-être déjà acquitté,
                        // ne lui sera pas remis ; il est servi ici.
                        if !shared.serves(platform, id, &token)
                            || !shared.first_delivery(platform, &incoming)
                        {
                            continue;
                        }
                        // Chaque message dans sa tâche : une longue réponse ne
                        // retient ni `/stop`, ni les autres comptes.
                        let (shared, messenger) = (Arc::clone(&shared), messenger.clone());
                        tauri::async_runtime::spawn(async move {
                            bot::handle(&shared, &messenger, incoming).await;
                        });
                    }
                }
            }
        });
        Ok(())
    }

    fn stop(&self, platform: Platform) {
        if let Some(live) = self.live.lock().remove(&platform) {
            let _ = live.stop.send(true);
        }
    }

    /// Les messages de l'écoute `id` sont servis : c'est l'écoute du bot, ou
    /// elle a été remplacée par une écoute du même jeton.
    fn serves(&self, platform: Platform, id: u64, token: &str) -> bool {
        self.live
            .lock()
            .get(&platform)
            .is_some_and(|live| live.id == id || live.token == token)
    }

    /// Vrai la première fois que ce message est remis ; il est alors retenu.
    fn first_delivery(&self, platform: Platform, incoming: &easy_link::Incoming) -> bool {
        let key = (platform, incoming.channel.clone(), incoming.id.clone());
        let mut served = self.served.lock();
        if served.contains(&key) {
            return false;
        }
        if served.len() >= SERVED_KEPT {
            served.pop_front();
        }
        served.push_back(key);
        true
    }

    fn is_live(&self, platform: Platform, id: u64) -> bool {
        self.live
            .lock()
            .get(&platform)
            .is_some_and(|live| live.id == id)
    }

    /// Vrai si la liaison montrée a changé.
    fn set_link(&self, platform: Platform, id: u64, link: EasyLinkLink) -> bool {
        match self.live.lock().get_mut(&platform) {
            Some(live) if live.id == id && live.link != link => {
                live.link = link;
                true
            }
            _ => false,
        }
    }

    async fn connected(&self, platform: Platform, id: u64, identity: Identity) {
        // Annonce d'une écoute déjà remplacée : elle n'écrit plus rien.
        if !self.is_live(platform, id) {
            return;
        }
        let changed = self.set_link(platform, id, EasyLinkLink::Connected);
        // Un bot peut être renommé : son nom est relu à chaque connexion.
        let renamed = self
            .store
            .call(move |c| {
                let bot = saved::bot(c, platform)?;
                if bot.bot_id == identity.id && bot.bot_name == identity.name {
                    return Ok(false);
                }
                saved::set_identity(c, platform, &identity.id, &identity.name)?;
                Ok(true)
            })
            .await
            .unwrap_or(false);
        if changed || renamed {
            self.publish().await;
        }
    }

    pub(crate) fn closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Arrête la réponse en cours d'un compte, où qu'elle en soit ; vrai s'il
    /// y en avait une.
    pub(crate) fn stop_answer(&self, account_id: i64) -> bool {
        match self.busy.lock().get_mut(&account_id) {
            Some(claim) => {
                claim.stopped = true;
                self.host.engine().cancel(&claim.stream);
                true
            }
            None => false,
        }
    }

    /// Arrêt demandé pour la réponse en cours de ce compte.
    pub(crate) fn stopped(&self, account_id: i64) -> bool {
        self.busy
            .lock()
            .get(&account_id)
            .is_some_and(|claim| claim.stopped)
    }

    /// Arrête la réponse en cours d'un compte et oublie ses images en attente.
    pub(crate) fn forget_account(&self, account_id: i64) {
        self.stop_answer(account_id);
        if let Some(files) = self.pending.lock().remove(&account_id) {
            self.staging.unstage(&files);
        }
        self.missed.lock().remove(&account_id);
    }

    /// Efface la conversation en cours d'un compte si elle est éphémère : le
    /// compte s'en détache d'abord, puis elle est effacée (voir
    /// [`Self::erase_chat`]). Vrai si elle l'a été.
    pub(crate) async fn forget_ephemeral_chat(&self, account_id: i64) -> AppResult<bool> {
        let chat = self
            .store
            .call(move |c| saved::detach_ephemeral(c, account_id))
            .await?;
        match chat {
            Some(chat) => self.erase_chat(account_id, chat).await,
            None => Ok(false),
        }
    }

    /// [`Self::forget_ephemeral_chat`] pour un compte délié : un échec est
    /// journalisé sans empêcher de délier (la fin de l'app l'effacera).
    pub(crate) async fn forget_ephemeral_chat_or_log(&self, account_id: i64) {
        if let Err(error) = self.forget_ephemeral_chat(account_id).await {
            eprintln!("chatonlm: cannot erase the ephemeral chat of a linked account: {error}");
        }
    }

    /// Efface une conversation éphémère déjà détachée de son compte, sa
    /// réponse arrêtée et finie d'abord (comme la suppression d'une
    /// conversation). Vrai si elle l'a été.
    pub(crate) async fn erase_chat(&self, account_id: i64, chat: String) -> AppResult<bool> {
        self.stop_answer(account_id);
        self.host.engine().stop_chat(&chat).await;
        self.store
            .call(move |c| crate::store::chats::forget(c, &chat))
            .await
    }

    pub(crate) async fn state(&self) -> AppResult<EasyLinkState> {
        let (bots, accounts) = self
            .store
            .call(|c| Ok((saved::bots(c)?, saved::accounts(c)?)))
            .await?;
        let pairing = self.pairings.lock().current(now_ms()).cloned();
        let live = self.live.lock();
        let pairing = pairing.map(|pairing| {
            let bot = bots.iter().find(|bot| bot.platform == pairing.platform);
            EasyLinkPairing {
                platform: pairing.platform,
                url: bot.and_then(|bot| pairing_url(bot, &pairing.code)),
                code: pairing.code,
                expires_at: pairing.expires_at,
            }
        });
        let bots = bots
            .into_iter()
            .map(|bot| EasyLinkBot {
                platform: bot.platform,
                configured: !bot.token.is_empty(),
                token_hint: token_hint(&bot.token),
                enabled: bot.enabled,
                link: live
                    .get(&bot.platform)
                    .map_or(EasyLinkLink::Off, |live| live.link),
                url: bot_url(&bot),
                name: bot.bot_name,
            })
            .collect();
        Ok(EasyLinkState {
            bots,
            accounts,
            pairing,
        })
    }

    /// Annonce l'état à la page.
    pub(crate) async fn publish(&self) {
        if let Ok(state) = self.state().await {
            self.host.changed(&state);
        }
    }
}

/// Fin du jeton montrée pour le reconnaître ; rien pour un jeton trop court
/// pour qu'elle n'en dise pas trop.
fn token_hint(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() < 16 {
        return String::new();
    }
    chars[chars.len() - 4..].iter().collect()
}

fn bot_url(bot: &Bot) -> Option<String> {
    match bot.platform {
        Platform::Telegram => {
            (!bot.bot_name.is_empty()).then(|| format!("https://t.me/{}", bot.bot_name))
        }
        // Sans aucune permission : le bot n'a qu'à partager un serveur avec
        // la personne pour qu'elle puisse lui écrire en privé.
        Platform::Discord => (!bot.bot_id.is_empty()).then(|| {
            format!(
                "https://discord.com/oauth2/authorize?client_id={}&scope=bot&permissions=0",
                bot.bot_id
            )
        }),
    }
}

/// Telegram : ouvrir ce lien envoie `/start <code>` au bot.
fn pairing_url(bot: &Bot, code: &str) -> Option<String> {
    (bot.platform == Platform::Telegram && !bot.bot_name.is_empty())
        .then(|| format!("https://t.me/{}?start={code}", bot.bot_name))
}

fn connect_error(platform: Platform, problem: &Problem) -> AppError {
    let service = service_name(platform);
    match problem {
        Problem::Unauthorized => AppError::Invalid(t!("easyLink.tokenRefused", service = service)),
        Problem::Network(error) => {
            AppError::System(t!("easyLink.unreachable", service = service, error = error))
        }
        Problem::Refused { message, .. } => {
            AppError::System(t!("easyLink.refused", service = service, error = message))
        }
        // Le reste n'arrive pas en vérifiant un jeton ; son code HTTP, qui
        // se lit dans toutes les langues.
        other => {
            let code = match other {
                Problem::Conflict => 409,
                Problem::Forbidden => 403,
                Problem::RateLimited(_) => 429,
                _ => 0,
            };
            AppError::System(t!("easyLink.refused", service = service, error = code))
        }
    }
}

/// Configuration TLS de la passerelle de Discord : racines du système, comme
/// reqwest pour HTTPS, et moteur ring.
fn gateway_tls() -> Option<Arc<rustls::ClientConfig>> {
    use rustls_platform_verifier::BuilderVerifierExt;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .ok()?
        .with_platform_verifier()
        .ok()?
        .with_no_client_auth();
    Some(Arc::new(config))
}

/// L'app, pour le service.
pub struct AppHost {
    app: AppHandle,
}

impl Host for AppHost {
    fn engine(&self) -> &ChatEngine {
        self.app.state::<ChatEngine>().inner()
    }

    fn changed(&self, state: &EasyLinkState) {
        let _ = self.app.emit(STATE_EVENT, state);
    }

    fn chat_updated(&self, chat_id: &str) {
        let _ = self.app.emit(
            CHATS_EVENT,
            ChatsUpdated {
                chat_id: chat_id.to_owned(),
            },
        );
    }

    async fn transcribe(&self, samples: Vec<f32>) -> Result<String, HearError> {
        use crate::dictation::engine::{ModelSpec, Request, transcription_failed};
        use crate::dictation::{Dictation, languages, models};

        let failed = |error: AppError| HearError::Failed(error.to_string());
        let store = self.app.state::<Store>().inner().clone();
        let settings = store
            .call(|c| crate::store::dictation::get(c))
            .await
            .map_err(failed)?;
        let dir = crate::paths::speech_models_dir(&self.app).map_err(failed)?;
        // Comme le mode vocal : le modèle de la dictée, sinon le premier
        // installé.
        let model = crate::voice_mode::session::speech_model(&settings, &|model| {
            models::installed(&dir, model)
        });
        if !models::installed(&dir, model) {
            return Err(HearError::NoModel);
        }
        let spec = ModelSpec::new(model, &dir);
        let request = Request {
            language: (settings.language != languages::AUTO).then(|| settings.language.clone()),
            glossary: settings.dictionary.clone(),
        };
        let engine = self.app.state::<Dictation>().engine();
        let heard = tauri::async_runtime::spawn_blocking(move || {
            engine.transcribe(&spec, &samples, &request, &AtomicBool::new(false))
        })
        .await;
        match heard {
            Ok(Ok(text)) => Ok(text.trim().to_owned()),
            Ok(Err(error)) => Err(HearError::Failed(error)),
            Err(error) => Err(HearError::Failed(transcription_failed(&error))),
        }
    }
}

/// Démarre le service : bots reliés écoutés dès le lancement. Sans base
/// lisible, pas d'Easy Link tant qu'elle n'est pas récupérée
/// (`startup::recover_database` rappelle cette fonction).
pub fn setup(app: &AppHandle) {
    // Déjà démarré (appel après la récupération d'une base illisible).
    if app.try_state::<EasyLink>().is_some() {
        return;
    }
    let Some(store) = app.try_state::<Store>().map(|store| store.inner().clone()) else {
        return;
    };
    let service = EasyLink::new(AppHost { app: app.clone() }, store, Urls::from_env());
    app.manage(service.clone());
    tauri::async_runtime::spawn(async move { service.start_saved().await });
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn the_token_hint_shows_four_characters_of_a_long_token_only() {
        assert_eq!(token_hint("123456:ABCDEFGHIJKLMNOP"), "MNOP");
        assert_eq!(token_hint("short-token"), "");
        assert_eq!(token_hint(""), "");
    }

    #[test]
    fn bot_links_need_the_identity() {
        let mut bot = Bot {
            platform: Platform::Telegram,
            token: "t".into(),
            enabled: true,
            bot_id: String::new(),
            bot_name: String::new(),
        };
        assert_eq!(bot_url(&bot), None);
        assert_eq!(pairing_url(&bot, "123456"), None);
        bot.bot_name = "chaton_bot".into();
        assert_eq!(bot_url(&bot).as_deref(), Some("https://t.me/chaton_bot"));
        assert_eq!(
            pairing_url(&bot, "123456").as_deref(),
            Some("https://t.me/chaton_bot?start=123456")
        );
        bot.platform = Platform::Discord;
        assert_eq!(bot_url(&bot), None);
        assert_eq!(pairing_url(&bot, "123456"), None);
        bot.bot_id = "42".into();
        assert_eq!(
            bot_url(&bot).as_deref(),
            Some("https://discord.com/oauth2/authorize?client_id=42&scope=bot&permissions=0")
        );
    }

    #[test]
    fn test_addresses_fall_back_to_the_official_ones() {
        assert_eq!(url_from(None, "https://a"), "https://a");
        assert_eq!(url_from(Some("  ".into()), "https://a"), "https://a");
        assert_eq!(
            url_from(Some("http://127.0.0.1:9/".into()), "https://a"),
            "http://127.0.0.1:9"
        );
    }
}
