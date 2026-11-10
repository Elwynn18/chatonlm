//! Ce que le bot fait d'un message privé : appairage d'un compte inconnu,
//! commande d'un compte lié, ou message pour le modèle.

use std::sync::Arc;
use std::time::{Duration, Instant};

use easy_link::{FileKind, Incoming, IncomingFile, Messenger, Problem, text, voice};
use tokio::sync::mpsc;

use super::commands::{self, Command};
use super::pairing::Verdict;
use super::relay::Relay;
use super::{Claim, HearError, Host, STREAM_PREFIX, Shared};
use crate::attachments::{MAX_FILE_SIZE, StageResult};
use crate::chat::{self, ChatErrorKind, ChatEvent, Origin, SendRequest, Server};
use crate::dictation::audio;
use crate::i18n::t;
use crate::llm::models::{self, ChatModel};
use crate::llm::{LlmError, ids};
use crate::store::easy_link::{self as saved, Account, Platform};
use crate::store::settings::Settings;
use crate::store::{Store, chats, model_prefs, now_ms};

/// Un message d'un compte lié plus ancien que cela à son arrivée (remis par la
/// messagerie après une absence de l'app) n'est plus servi : y répondre des
/// heures après surprendrait. Le compte en est prévenu.
const STALE_SECS: i64 = 5 * 60;

/// Attente acceptée avant de redire un texte du bot refusé pour « trop de
/// requêtes ».
const SAY_WAIT: Duration = Duration::from_secs(10);

/// Délai entre deux avis « message arrivé trop tard » au même compte.
const MISSED_NOTICE_EVERY: Duration = Duration::from_secs(60);

/// Attente des autres photos d'un album de Telegram avant l'envoi.
const ALBUM_WAIT: Duration = Duration::from_millis(1500);

/// Images d'un même message au modèle.
const MAX_IMAGES: usize = 4;

/// Taille d'un message vocal (dix minutes d'Opus tiennent largement).
const MAX_VOICE_BYTES: u64 = 20 * 1024 * 1024;

/// Durée d'un message vocal : celle d'une dictée.
const MAX_VOICE_SECS: u32 = 10 * 60;

/// Rythme de la boucle d'une réponse.
const TICK: Duration = Duration::from_millis(400);

/// « écrit… » est redemandé à ce rythme (Telegram le montre 5 s), sauf si le
/// bot vient d'écrire.
const TYPING_EVERY: Duration = Duration::from_secs(4);

/// La page relit une conversation en cours à ce rythme.
const REFRESH_EVERY: Duration = Duration::from_secs(2);

/// Préfixe des commandes dans les textes du bot : l'usage de chaque
/// messagerie (les deux sont acceptés partout).
fn prefix(platform: Platform) -> &'static str {
    match platform {
        Platform::Telegram => "/",
        Platform::Discord => "!",
    }
}

pub(super) async fn handle<H: Host>(
    shared: &Arc<Shared<H>>,
    messenger: &Messenger,
    incoming: Incoming,
) {
    if shared.closed() {
        return;
    }
    let stale = incoming
        .sent_at
        .is_some_and(|sent| now_ms() / 1000 - sent > STALE_SECS);
    let platform = Platform::from(messenger.platform());
    let user_id = incoming.sender.id.clone();
    let Ok(account) = shared
        .store
        .call(move |c| saved::account(c, platform, &user_id))
        .await
    else {
        return;
    };
    // Un inconnu n'est pas soumis à l'ancienneté : sur un PC dont l'horloge
    // avance, son code d'appairage, daté par la messagerie, paraîtrait ancien.
    let Some(account) = account else {
        return stranger(shared, messenger, platform, &incoming).await;
    };
    if stale {
        // Dit une fois, pas à chaque message d'une file remise d'un coup.
        let due = {
            let mut missed = shared.missed.lock();
            let due = missed
                .get(&account.id)
                .is_none_or(|last| last.elapsed() >= MISSED_NOTICE_EVERY);
            if due {
                missed.insert(account.id, Instant::now());
            }
            due
        };
        if due {
            let text = t!("easyLink.bot.missed");
            say(messenger, &incoming.channel, &text).await;
        }
        return;
    }
    // Nom changé dans la messagerie : la liste des réglages le suit.
    if !incoming.sender.name.is_empty() && incoming.sender.name != account.name {
        let (id, name) = (account.id, incoming.sender.name.clone());
        let renamed = shared.store.call(move |c| saved::set_name(c, id, &name));
        if renamed.await.is_ok() {
            shared.publish().await;
        }
    }
    match commands::parse(&incoming.text).filter(|_| incoming.files.is_empty()) {
        Some(command) => run(shared, messenger, &account, &incoming.channel, command).await,
        None => answer(shared, messenger, account, incoming).await,
    }
}

/// Message d'un compte qui n'est pas lié : le bon code le lie, tout le reste
/// reçoit au plus un avis.
async fn stranger<H: Host>(
    shared: &Arc<Shared<H>>,
    messenger: &Messenger,
    platform: Platform,
    incoming: &Incoming,
) {
    let (verdict, changed) =
        shared
            .pairings
            .lock()
            .attempt(platform, &incoming.sender.id, &incoming.text, now_ms());
    match verdict {
        Verdict::Linked => {
            let (id, name) = (incoming.sender.id.clone(), incoming.sender.name.clone());
            let linked = shared
                .store
                .call(move |c| saved::link(c, platform, &id, &name, now_ms()))
                .await;
            let reply = match linked {
                Ok(_) => format!("{}\n\n{}", t!("easyLink.bot.linked"), help(platform)),
                Err(error) => t!("easyLink.bot.failed", error = error),
            };
            // La page d'abord : l'envoi de l'accueil peut traîner.
            shared.publish().await;
            say(messenger, &incoming.channel, &reply).await;
        }
        Verdict::Notice => {
            let text = t!("easyLink.bot.private");
            say(messenger, &incoming.channel, &text).await
        }
        Verdict::Silent => {}
    }
    if changed {
        shared.publish().await;
    }
}

fn help(platform: Platform) -> String {
    let p = prefix(platform);
    [
        t!("easyLink.bot.help.title"),
        t!("easyLink.bot.help.new", p = p),
        t!("easyLink.bot.help.stop", p = p),
        t!("easyLink.bot.help.models", p = p),
        t!("easyLink.bot.help.model", p = p),
        t!("easyLink.bot.help.load", p = p),
        t!("easyLink.bot.help.unload", p = p),
        t!("easyLink.bot.help.status", p = p),
        t!("easyLink.bot.help.web", p = p),
        t!("easyLink.bot.help.ephemeral", p = p),
        t!("easyLink.bot.help.keep", p = p),
        t!("easyLink.bot.help.unlink", p = p),
        String::new(),
        t!("easyLink.bot.help.files"),
    ]
    .join("\n")
}

/// Envoie un texte du bot, en plusieurs messages s'il le faut. Un refus « trop
/// de requêtes » est attendu une fois, s'il demande peu ; tout autre échec
/// est abandonné (le destinataire a bloqué le bot, réseau coupé : rien d'autre
/// à faire).
async fn say(messenger: &Messenger, channel: &str, markdown: &str) {
    for part in text::split(markdown, messenger.text_limit()) {
        match messenger.send(channel, &part).await {
            Ok(_) => {}
            Err(Problem::RateLimited(wait)) if wait <= SAY_WAIT => {
                tokio::time::sleep(wait).await;
                if messenger.send(channel, &part).await.is_err() {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

/// Modèle d'un compte : celui qu'il a choisi, sinon celui de l'app.
fn model_of(account: &Account, selected: &str) -> String {
    if account.model.trim().is_empty() {
        selected.trim().to_owned()
    } else {
        account.model.clone()
    }
}

/// Modèle désigné par un numéro de la liste, par son identifiant ou par son
/// nom.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Picked {
    One(String),
    None,
    Many(Vec<String>),
}

/// Numéro (à partir de 1) de la liste ; identifiant exact (comparé comme la
/// base : quantification sans la casse) ; nom exact (sans la casse : plusieurs quantifications d'un même
/// dépôt le partagent) ; sinon le seul modèle dont l'identifiant ou le nom
/// contient le texte. Rend des identifiants.
pub(super) fn pick(models: &[ChatModel], wanted: &str) -> Picked {
    let wanted = wanted.trim();
    if let Ok(number) = wanted.parse::<usize>() {
        return match number.checked_sub(1).and_then(|index| models.get(index)) {
            Some(model) => Picked::One(model.id.clone()),
            None => Picked::None,
        };
    }
    if let Some(exact) = models.iter().find(|model| ids::same(&model.id, wanted)) {
        return Picked::One(exact.id.clone());
    }
    let several = |mut found: Vec<String>| match found.len() {
        0 => Picked::None,
        1 => Picked::One(found.remove(0)),
        _ => Picked::Many(found),
    };
    let named: Vec<String> = models
        .iter()
        .filter(|model| model.display_name.eq_ignore_ascii_case(wanted))
        .map(|model| model.id.clone())
        .collect();
    if !named.is_empty() {
        return several(named);
    }
    let needle = wanted.to_lowercase();
    several(
        models
            .iter()
            .filter(|model| {
                model.id.to_lowercase().contains(&needle)
                    || model.display_name.to_lowercase().contains(&needle)
            })
            .map(|model| model.id.clone())
            .collect(),
    )
}

/// Modèles du sélecteur de l'app (installés, puis distants si permis), triés
/// par nom puis identifiant : les numéros de `/models` restent les mêmes d'un
/// appel à l'autre.
async fn installed(access: &crate::llm::Access) -> crate::error::AppResult<Vec<ChatModel>> {
    let mut listed = models::list(access).await?;
    listed.sort_by_cached_key(|model| (model.display_name.to_lowercase(), model.id.to_lowercase()));
    Ok(listed)
}

/// Réglages de l'app, lus dans la base.
async fn app_settings(store: &Store) -> crate::error::AppResult<Settings> {
    store.call(|c| crate::store::settings::get(c)).await
}

/// Modèle choisi pour une réponse, s'il peut servir ; sinon ce que le bot
/// répond. Un modèle distant, réglage coupé : refusé sans rien appeler. Un
/// modèle absent (supprimé depuis son choix) : refusé, alors que le chat le
/// téléchargerait de lui-même, ce qu'un bot ne doit pas déclencher.
async fn usable(
    access: &crate::llm::Access,
    settings: &Settings,
    model: &str,
    p: &str,
) -> Result<ChatModel, String> {
    if model.is_empty() {
        return Err(t!("easyLink.bot.noModel", p = p));
    }
    if ids::is_remote(model) && !settings.remote_models {
        return Err(LlmError::RemoteOff.message());
    }
    match models::get(access, model).await {
        Ok(Some(found)) => Ok(found),
        // Distant que son fournisseur ne liste plus (ou injoignable) : rien
        // n'est désinstallé, il n'est pas disponible.
        Ok(None) if ids::is_remote(model) => {
            Err(t!("easyLink.bot.remoteMissing", model = code(model), p = p))
        }
        Ok(None) => Err(t!("easyLink.bot.modelMissing", model = code(model), p = p)),
        Err(error) => Err(t!("easyLink.bot.failed", error = error)),
    }
}

/// `/load` ou `/unload` d'un modèle distant : il tourne chez son
/// fournisseur, rien à charger ni à décharger ; réglage des distants coupé :
/// le refus du moteur (lu en base à chaque fois).
async fn remote_load(access: &crate::llm::Access, id: &str) -> String {
    match access.prefs().await {
        Ok(prefs) if !prefs.remote_models => LlmError::RemoteOff.message(),
        Ok(_) => t!("easyLink.bot.remoteNoLoad", model = code(id)),
        Err(error) => t!("easyLink.bot.failed", error = error),
    }
}

/// Le modèle sait lire les images (capacité `vision` de son fichier, ou
/// annoncée par le fournisseur).
fn sees(model: &ChatModel) -> bool {
    model.capabilities.iter().any(|word| word == "vision")
}

fn code(name: &str) -> String {
    format!("`{name}`")
}

async fn run<H: Host>(
    shared: &Arc<Shared<H>>,
    messenger: &Messenger,
    account: &Account,
    channel: &str,
    command: Command,
) {
    let p = prefix(account.platform);
    let reply = match command {
        Command::Help => help(account.platform),
        Command::New => {
            shared.forget_account(account.id);
            let id = account.id;
            // Conversation éphémère quittée : détachée en même temps que le
            // compte quitte la conversation en cours, puis effacée (la
            // suivante le sera aussi si le mode est toujours actif).
            let detached = shared
                .store
                .call(move |c| {
                    let chat = saved::detach_ephemeral(c, id)?;
                    saved::set_chat(c, id, None)?;
                    Ok(chat)
                })
                .await;
            let left = match detached {
                Ok(Some(chat)) => shared.erase_chat(id, chat).await,
                Ok(None) => Ok(false),
                Err(error) => Err(error),
            };
            match left {
                Ok(true) => format!(
                    "{}\n{}",
                    t!("easyLink.bot.ephemeralErased"),
                    t!("easyLink.bot.newChat")
                ),
                Ok(false) => t!("easyLink.bot.newChat"),
                Err(error) => t!("easyLink.bot.failed", error = error),
            }
        }
        Command::Stop => {
            if shared.stop_answer(account.id) {
                t!("easyLink.bot.stopped")
            } else {
                t!("easyLink.bot.nothingToStop")
            }
        }
        Command::Unlink => {
            shared.forget_account(account.id);
            let id = account.id;
            // Sa conversation éphémère ne survit pas au compte (un échec
            // n'empêche pas de délier : la fin de l'app l'effacera).
            shared.forget_ephemeral_chat_or_log(id).await;
            let _ = shared.store.call(move |c| saved::unlink(c, id)).await;
            shared.publish().await;
            {
                let text = t!("easyLink.bot.unlinked");
                say(messenger, channel, &text).await
            };
            return;
        }
        Command::Web(Err(_)) => t!("easyLink.bot.webUsage", p = p),
        Command::Web(Ok(None)) => web_line(account.web_search),
        Command::Web(Ok(Some(enabled))) => {
            let id = account.id;
            let stored = shared
                .store
                .call(move |c| saved::set_web_search(c, id, enabled));
            match stored.await {
                Ok(()) if enabled => t!("easyLink.bot.webOn"),
                Ok(()) => t!("easyLink.bot.webOff"),
                Err(error) => t!("easyLink.bot.failed", error = error),
            }
        }
        Command::Ephemeral(Err(_)) => t!("easyLink.bot.ephemeralUsage", p = p),
        Command::Ephemeral(Ok(None)) => {
            if account.ephemeral {
                t!("easyLink.bot.statusEphemeralOn")
            } else {
                t!("easyLink.bot.statusEphemeralOff")
            }
        }
        Command::Ephemeral(Ok(Some(enabled))) => {
            match set_ephemeral(shared, account, enabled).await {
                Ok(erased) => {
                    let mut lines = Vec::new();
                    if erased {
                        lines.push(t!("easyLink.bot.ephemeralErased"));
                    }
                    lines.push(if enabled {
                        t!("easyLink.bot.ephemeralOn", p = p)
                    } else {
                        t!("easyLink.bot.ephemeralOff")
                    });
                    lines.join("\n")
                }
                Err(error) => t!("easyLink.bot.failed", error = error),
            }
        }
        Command::Keep => match keep(shared, account.id).await {
            Ok(true) => t!("easyLink.bot.kept"),
            Ok(false) => t!("easyLink.bot.nothingToKeep"),
            Err(error) => t!("easyLink.bot.failed", error = error),
        },
        Command::Unknown(name) => {
            t!(
                "easyLink.bot.unknownCommand",
                command = format!("/{name}"),
                p = p
            )
        }
        // Le reste parle au moteur des modèles.
        command => {
            let _ = messenger.typing(channel).await;
            match with_models(shared, messenger, account, channel, command).await {
                Ok(reply) => reply,
                Err(error) => t!("easyLink.bot.failed", error = error),
            }
        }
    };
    say(messenger, channel, &reply).await;
}

/// `/ephemeral on|off`. Activé : une conversation ordinaire en cours reste
/// dans l'historique et le compte s'en détache (la suivante sera éphémère) ;
/// une conversation éphémère en cours continue. Coupé : la conversation
/// éphémère en cours est effacée (réponse arrêtée d'abord), avec les photos
/// en attente ; une conversation ordinaire continue. Vrai si une conversation
/// a été effacée.
async fn set_ephemeral<H: Host>(
    shared: &Arc<Shared<H>>,
    account: &Account,
    enabled: bool,
) -> crate::error::AppResult<bool> {
    let id = account.id;
    if enabled {
        shared
            .store
            .call(move |c| {
                saved::set_ephemeral(c, id, true)?;
                if saved::ephemeral_chat(c, id)?.is_none() {
                    saved::set_chat(c, id, None)?;
                }
                Ok(())
            })
            .await?;
        return Ok(false);
    }
    // Coupé, et la conversation éphémère détachée, d'un coup : un message qui
    // arrive pendant l'effacement ouvre une conversation ordinaire.
    let detached = shared
        .store
        .call(move |c| {
            saved::set_ephemeral(c, id, false)?;
            saved::detach_ephemeral(c, id)
        })
        .await?;
    if account.ephemeral {
        // Photos envoyées en mode éphémère : elles ne partent pas dans une
        // conversation gardée.
        shared.forget_account(id);
    }
    match detached {
        Some(chat) => shared.erase_chat(id, chat).await,
        None => Ok(false),
    }
}

/// `/keep` : la conversation éphémère en cours passe dans l'historique et le
/// compte s'en détache (le message suivant en ouvre une nouvelle, éphémère si
/// le mode l'est toujours). Une réponse en cours y continue : elle est gardée
/// avec elle. Faux s'il n'y avait pas de conversation éphémère.
async fn keep<H: Host>(shared: &Arc<Shared<H>>, id: i64) -> crate::error::AppResult<bool> {
    let kept = shared
        .store
        .call(move |c| {
            let Some(chat) = saved::ephemeral_chat(c, id)? else {
                return Ok(None);
            };
            chats::keep(c, &chat)?;
            saved::set_chat(c, id, None)?;
            Ok(Some(chat))
        })
        .await?;
    // La page voit la conversation arriver dans l'historique.
    if let Some(chat) = &kept {
        shared.host.chat_updated(chat);
    }
    Ok(kept.is_some())
}

fn web_line(enabled: bool) -> String {
    if enabled {
        t!("easyLink.bot.statusWebOn")
    } else {
        t!("easyLink.bot.statusWebOff")
    }
}

/// Commandes qui interrogent les modèles (liste, choix, chargement, état).
async fn with_models<H: Host>(
    shared: &Arc<Shared<H>>,
    messenger: &Messenger,
    account: &Account,
    channel: &str,
    command: Command,
) -> crate::error::AppResult<String> {
    let p = prefix(account.platform);
    let access = &shared.access;
    let settings = app_settings(&shared.store).await?;
    let current = model_of(account, &settings.selected_model);
    // Modèle visé par `/load` et `/unload` : celui nommé, sinon le courant.
    let named = |listed: &[ChatModel], wanted: Option<&str>| -> Result<String, String> {
        match wanted {
            None if current.is_empty() => Err(t!("easyLink.bot.noModel", p = p)),
            None => Ok(current.clone()),
            Some(wanted) => match pick(listed, wanted) {
                Picked::One(id) => Ok(id),
                Picked::None => Err(t!("easyLink.bot.modelUnknown", name = wanted, p = p)),
                Picked::Many(found) => Err(t!(
                    "easyLink.bot.modelAmbiguous",
                    name = wanted,
                    models = found
                        .iter()
                        .map(|id| code(id))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            },
        }
    };
    Ok(match command {
        Command::Models => {
            let listed = installed(access).await?;
            if listed.is_empty() {
                return Ok(t!("easyLink.bot.noModels"));
            }
            let loaded = models::running(access);
            let mut lines = vec![t!("easyLink.bot.modelsTitle")];
            for (index, model) in listed.iter().enumerate() {
                let mut notes = Vec::new();
                if ids::same(&model.id, &current) {
                    notes.push(t!("easyLink.bot.modelCurrent"));
                }
                if ids::is_remote(&model.id) {
                    notes.push(t!("easyLink.bot.modelRemote"));
                } else if loaded.iter().any(|l| ids::same(&l.id, &model.id)) {
                    notes.push(t!("easyLink.bot.modelLoaded"));
                }
                let notes = if notes.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", notes.join(", "))
                };
                lines.push(format!(
                    "{}. {} {}{notes}",
                    index + 1,
                    model.display_name,
                    code(&model.id)
                ));
            }
            lines.push(String::new());
            lines.push(t!("easyLink.bot.modelsHint", p = p));
            lines.join("\n")
        }
        Command::Model(None) if current.is_empty() => t!("easyLink.bot.noModel", p = p),
        Command::Model(None) => t!("easyLink.bot.model", model = code(&current)),
        Command::Model(Some(wanted)) => {
            let listed = installed(access).await?;
            match named(&listed, Some(&wanted)) {
                Ok(id) => {
                    let (account_id, chosen) = (account.id, id.clone());
                    shared
                        .store
                        .call(move |c| saved::set_model(c, account_id, &chosen))
                        .await?;
                    t!("easyLink.bot.modelChosen", model = code(&id))
                }
                Err(reply) => reply,
            }
        }
        Command::Load(wanted) => {
            let listed = match &wanted {
                Some(_) => installed(access).await?,
                None => Vec::new(),
            };
            match named(&listed, wanted.as_deref()) {
                // Distant : rien à charger (réglage coupé : refusé).
                Ok(id) if ids::is_remote(&id) => remote_load(access, &id).await,
                Ok(id) => {
                    let loading = t!("easyLink.bot.loading", model = code(&id));
                    say(messenger, channel, &loading).await;
                    let _ = messenger.typing(channel).await;
                    match models::load(access, &id).await {
                        Ok(()) => t!("easyLink.bot.loaded", model = code(&id)),
                        // Déchargé pendant son chargement (`/unload`, bouton
                        // de l'app : `Engine(Unloaded)`), moteur fermé ou
                        // annulation : un arrêt, pas une erreur.
                        Err(error) if error.chat_kind().is_none() => {
                            t!("easyLink.bot.unloaded", model = code(&id))
                        }
                        Err(error) => t!("easyLink.bot.failed", error = error.message()),
                    }
                }
                Err(reply) => reply,
            }
        }
        Command::Unload(Some(all)) if all.eq_ignore_ascii_case("all") => {
            models::unload_all(access).await;
            t!("easyLink.bot.unloadedAll")
        }
        Command::Unload(wanted) => {
            let listed = match &wanted {
                Some(_) => installed(access).await?,
                None => Vec::new(),
            };
            match named(&listed, wanted.as_deref()) {
                Ok(id) if ids::is_remote(&id) => remote_load(access, &id).await,
                Ok(id) => {
                    models::unload(access, &id).await;
                    t!("easyLink.bot.unloaded", model = code(&id))
                }
                Err(reply) => reply,
            }
        }
        Command::Status => {
            let mut lines = Vec::new();
            lines.push(if current.is_empty() {
                t!("easyLink.bot.noModel", p = p)
            } else {
                t!("easyLink.bot.model", model = code(&current))
            });
            if access.engine().available() {
                let loaded = models::running(access);
                if loaded.is_empty() {
                    lines.push(t!("easyLink.bot.statusNothingLoaded"));
                } else {
                    let ids: Vec<String> = loaded.iter().map(|l| code(&l.id)).collect();
                    lines.push(t!("easyLink.bot.statusLoaded", models = ids.join(", ")));
                }
            } else {
                lines.push(t!("easyLink.bot.statusEngineOff"));
            }
            lines.push(web_line(account.web_search));
            // Tâches de VS Code (réglage activé seulement).
            lines.extend(shared.task_lines().await);
            lines.join("\n")
        }
        // Traitées par `run`.
        Command::Help
        | Command::New
        | Command::Stop
        | Command::Unlink
        | Command::Web(_)
        | Command::Ephemeral(_)
        | Command::Keep
        | Command::Unknown(_) => String::new(),
    })
}

/// Réponse en cours d'un compte, retirée en sortant.
struct Busy<'a, H: Host> {
    shared: &'a Shared<H>,
    account: i64,
}

impl<'a, H: Host> Busy<'a, H> {
    fn claim(shared: &'a Shared<H>, account: i64, stream: &str) -> Option<Self> {
        let mut busy = shared.busy.lock();
        if busy.contains_key(&account) {
            return None;
        }
        busy.insert(
            account,
            Claim {
                stream: stream.to_owned(),
                stopped: false,
            },
        );
        Some(Self { shared, account })
    }
}

impl<H: Host> Drop for Busy<'_, H> {
    fn drop(&mut self) {
        self.shared.busy.lock().remove(&self.account);
    }
}

/// Message pour le modèle : texte, photos et messages vocaux.
async fn answer<H: Host>(
    shared: &Arc<Shared<H>>,
    messenger: &Messenger,
    account: Account,
    incoming: Incoming,
) {
    let channel = incoming.channel.as_str();
    let p = prefix(account.platform);
    if incoming.files.iter().any(|f| f.kind == FileKind::Other) {
        return {
            let text = t!("easyLink.bot.unsupportedFile");
            say(messenger, channel, &text).await
        };
    }
    if incoming.text.trim().is_empty() {
        // Ni texte ni fichier (position, contact...) : rien à en faire.
        if incoming.files.is_empty() {
            return;
        }
        if incoming.files.iter().all(|f| f.kind == FileKind::Image) {
            return keep_images(shared, messenger, &account, &incoming).await;
        }
    }
    let stream_id = format!("{STREAM_PREFIX}{}:{}", account.id, uuid::Uuid::now_v7());
    let Some(busy) = Busy::claim(shared, account.id, &stream_id) else {
        return {
            let text = t!("easyLink.bot.busy", p = p);
            say(messenger, channel, &text).await
        };
    };
    let account_id = account.id;
    respond(shared, messenger, account, &incoming, &stream_id).await;
    drop(busy);
    // Photo sans légende arrivée pendant la réponse (album complété après la
    // jointure) : gardée sans invite tant que le compte était occupé, elle
    // est annoncée maintenant.
    let waiting = shared
        .pending
        .lock()
        .get(&account_id)
        .is_some_and(|files| !files.is_empty());
    if waiting && !shared.closed() {
        let text = t!("easyLink.bot.imagePending");
        say(messenger, channel, &text).await;
    }
}

/// Réponse d'un compte occupé : préparation (messages vocaux, photos), envoi
/// au moteur de chat, écriture dans la messagerie.
async fn respond<H: Host>(
    shared: &Arc<Shared<H>>,
    messenger: &Messenger,
    account: Account,
    incoming: &Incoming,
    stream_id: &str,
) {
    let channel = incoming.channel.as_str();
    let p = prefix(account.platform);
    let _ = messenger.typing(channel).await;

    let settings = match app_settings(&shared.store).await {
        Ok(settings) => settings,
        Err(error) => {
            return {
                let text = t!("easyLink.bot.failed", error = error);
                say(messenger, channel, &text).await
            };
        }
    };
    let model = model_of(&account, &settings.selected_model);
    let can_see = match usable(&shared.access, &settings, &model, p).await {
        Ok(chosen) => sees(&chosen),
        Err(reply) => return say(messenger, channel, &reply).await,
    };
    let blind = || t!("easyLink.bot.noVision", model = code(&model), p = p);

    // Messages vocaux : transcrits, et redits à l'auteur pour qu'il voie ce
    // qui a été compris.
    let mut content = incoming.text.trim().to_owned();
    for file in incoming.files.iter().filter(|f| f.kind == FileKind::Voice) {
        match hear(shared, messenger, file).await {
            Ok(heard) => {
                say(messenger, channel, &format!("🎤 {heard}")).await;
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(&heard);
            }
            Err(reply) => return say(messenger, channel, &reply).await,
        }
    }

    // Photos : refusées tout de suite si le modèle ne sait pas voir (le
    // moteur de chat les retirerait en silence).
    let mut files = Vec::new();
    let waiting = shared.pending.lock().get(&account.id).map_or(0, Vec::len);
    let images: Vec<&IncomingFile> = incoming
        .files
        .iter()
        .filter(|f| f.kind == FileKind::Image)
        .collect();
    for image in &images {
        let staged = if can_see {
            stage(shared, messenger, image, waiting + files.len()).await
        } else {
            Err(blind())
        };
        match staged {
            Ok(id) => files.push(id),
            Err(reply) => {
                shared.staging.unstage(&files);
                return say(messenger, channel, &reply).await;
            }
        }
    }
    if content.is_empty() {
        // Message vocal sans parole reconnue, déjà dit plus haut : rien à
        // envoyer.
        shared.staging.unstage(&files);
        return;
    }
    // Un album de Telegram arrive en plusieurs messages, la légende sur le
    // premier : les autres photos ont un instant pour rejoindre celle-ci.
    if account.platform == Platform::Telegram && !images.is_empty() {
        tokio::time::sleep(ALBUM_WAIT).await;
    }
    // Photos envoyées juste avant, sans légende : jointes à ce message.
    let mut attached = shared
        .pending
        .lock()
        .remove(&account.id)
        .unwrap_or_default();
    if !attached.is_empty() && !can_see {
        // Modèle changé depuis pour un modèle qui ne voit pas.
        shared.staging.unstage(&attached);
        shared.staging.unstage(&files);
        let reply = blind();
        return say(messenger, channel, &reply).await;
    }
    attached.append(&mut files);
    if attached.len() > MAX_IMAGES {
        shared.staging.unstage(&attached[MAX_IMAGES..]);
        attached.truncate(MAX_IMAGES);
    }
    let files = attached;

    // Pendant la préparation (transcription, photos), l'app a pu se fermer,
    // et le compte être délié ou demander `/stop` ou `/new` : son état est
    // relu avant l'envoi.
    let fresh = {
        let (platform, user) = (account.platform, account.user_id.clone());
        shared
            .store
            .call(move |c| saved::account(c, platform, &user))
            .await
            .ok()
            .flatten()
    };
    let Some(account) = fresh.filter(|_| !(shared.stopped(account.id) || shared.closed())) else {
        shared.staging.unstage(&files);
        return;
    };

    let think = {
        let model = model.clone();
        shared
            .store
            .call(move |c| model_prefs::think(c, &model))
            .await
            .ok()
            .flatten()
    };
    let request = SendRequest {
        chat_id: account.chat_id.clone(),
        stream_id: stream_id.to_owned(),
        content,
        model,
        think,
        attachment_ids: (!files.is_empty()).then(|| files.clone()),
        edit_message_id: None,
        kept_attachment_ids: None,
        web_search: Some(account.web_search),
        voice: None,
        image: None,
        // Seulement pour une conversation à ouvrir (ignoré sinon).
        ephemeral: Some(account.ephemeral),
    };
    let server = match Server::current(&shared.access).await {
        Ok(server) => server,
        Err(error) => {
            shared.staging.unstage(&files);
            let text = t!("easyLink.bot.failed", error = error);
            return say(messenger, channel, &text).await;
        }
    };
    let (events, mut received) = mpsc::unbounded_channel::<ChatEvent>();
    // L'envoi dans sa tâche : la lecture de la réponse du modèle n'attend pas
    // les requêtes à la messagerie.
    let sending = {
        let shared = Arc::clone(shared);
        let (account_id, stream_id) = (account.id, stream_id.to_owned());
        tauri::async_runtime::spawn(async move {
            let sink = |event: ChatEvent| {
                // Arrêt demandé entre la relecture du compte et `begin` : le
                // moteur, qui ne connaissait pas encore le flux, l'apprend
                // ici, avant toute requête au modèle.
                if matches!(event, ChatEvent::UserMessage { .. }) && shared.stopped(account_id) {
                    shared.host.engine().cancel(&stream_id);
                }
                let _ = events.send(event);
            };
            chat::send_to(
                &shared.store,
                shared.host.engine(),
                &shared.staging,
                server,
                request,
                Origin::Remote,
                &sink,
            )
            .await
        })
    };

    let mut relay = Relay::new(messenger, channel);
    let mut chat_id = account.chat_id.clone();
    // Conversation ouverte par cette réponse, et rattachée au compte ou non.
    let mut created: Option<String> = None;
    let mut attached = false;
    let mut failure: Option<(ChatErrorKind, String)> = None;
    let mut done = false;
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut typed = Instant::now();
    let mut refreshed = Instant::now();
    // Une image se calcule (outil `generate_image`) : « envoie une photo… ».
    let mut painting = false;
    loop {
        tokio::select! {
            event = received.recv() => {
                // Canal fermé : l'envoi est fini.
                let Some(event) = event else { break };
                match event {
                    ChatEvent::ChatCreated { chat_id: id } => {
                        let (account_id, platform, chat) = (account.id, account.platform, id.clone());
                        attached = shared
                            .store
                            .call(move |c| {
                                chats::set_source(c, &chat, platform)?;
                                saved::attach_chat(c, account_id, &chat)
                            })
                            .await
                            .unwrap_or(false);
                        created = Some(id.clone());
                        shared.host.chat_updated(&id);
                        chat_id = Some(id);
                    }
                    ChatEvent::UserMessage { .. } => {
                        if let Some(id) = &chat_id {
                            shared.host.chat_updated(id);
                        }
                    }
                    ChatEvent::ReplyStarted { .. } => {
                        painting = false;
                        relay.new_turn();
                    }
                    ChatEvent::Content { text } => relay.push(&text),
                    // Rien à envoyer de l'avancement : l'indicateur suffit.
                    ChatEvent::ImageProgress { .. } => painting = true,
                    ChatEvent::ImageReady { attachment, .. } => {
                        painting = false;
                        // Arrêt demandé (`/stop`, compte délié) ou bot bloqué
                        // pendant que l'image finissait : rien n'est envoyé.
                        if !(shared.stopped(account.id) || relay.gone()) {
                            let png = generated_png(&shared.store, attachment.id).await;
                            relay.photo(png, &attachment.filename).await;
                        }
                    }
                    // Image en échec : l'erreur va au modèle, qui la dira.
                    ChatEvent::ToolResult { .. } => painting = false,
                    ChatEvent::Done { .. } => done = true,
                    ChatEvent::Error { kind, message } => failure = Some((kind, message)),
                    _ => {}
                }
            }
            _ = ticker.tick() => {
                relay.tick().await;
                // Le destinataire a bloqué le bot, ou l'arrêt a été demandé
                // avant que le moteur ne connaisse ce flux : redemandé ici.
                if relay.gone() || shared.stopped(account.id) {
                    shared.host.engine().cancel(stream_id);
                }
                if typed.elapsed() >= TYPING_EVERY && !relay.wrote_within(TYPING_EVERY) {
                    typed = Instant::now();
                    let _ = if painting {
                        messenger.preparing_photo(channel).await
                    } else {
                        messenger.typing(channel).await
                    };
                }
                if refreshed.elapsed() >= REFRESH_EVERY && let Some(id) = &chat_id {
                    refreshed = Instant::now();
                    shared.host.chat_updated(id);
                }
            }
        }
    }
    let sent = sending.await;
    // Conversation ouverte selon un mode changé depuis (`/ephemeral` passé
    // entre la relecture du compte et l'ouverture) : laissée détachée ;
    // éphémère, elle est effacée maintenant que la réponse est finie
    // (ordinaire, elle reste dans l'historique).
    if let Some(chat) = created.filter(|_| !attached) {
        let _ = shared.store.call(move |c| chats::forget(c, &chat)).await;
    }
    // La page d'abord : la dernière écriture dans la messagerie peut traîner.
    if let Some(id) = &chat_id {
        shared.host.chat_updated(id);
    }
    // Dernière écriture en échec (réseau coupé, limite de débit, message du
    // bot supprimé) : la messagerie garde un texte coupé, ce que le bot dit.
    let incomplete = matches!(
        relay.finish().await,
        Err(problem) if problem != Problem::Forbidden
    );
    let note = match sent {
        // Refusé d'emblée (conversation occupée dans l'app...) : les photos,
        // revenues dans la zone, attendent le prochain message.
        Ok(Err(error)) => {
            if !files.is_empty() {
                let mut pending = shared.pending.lock();
                pending.entry(account.id).or_default().extend(files);
            }
            Some(t!("easyLink.bot.failed", error = error))
        }
        Err(error) => {
            let error = crate::error::AppError::Internal(error.to_string());
            Some(t!("easyLink.bot.failed", error = error))
        }
        Ok(Ok(())) if incomplete => Some(t!("easyLink.bot.incomplete")),
        Ok(Ok(())) => match failure {
            Some((kind, message)) => Some(error_note(kind, &message, p)),
            None if done && relay.is_empty() => Some(t!("easyLink.bot.emptyAnswer")),
            None => None,
        },
    };
    // Par le relais : une limite de débit encore posée est respectée.
    if let Some(note) = note
        && !relay.gone()
    {
        relay.say(&note).await;
    }
}

/// Ce que le bot dit d'un envoi fini en erreur (code du chat) : un conseil
/// pour la recherche web (`/web off` pour répondre sans elle), le moteur en
/// panne sans son détail technique (qui ne regarde pas le correspondant), le
/// contexte plein, sinon le message du chat.
pub(super) fn error_note(kind: ChatErrorKind, message: &str, p: &str) -> String {
    match kind {
        ChatErrorKind::WebSearchUnavailable => t!("easyLink.bot.webUnavailable", p = p),
        ChatErrorKind::WebSearchLimit => t!("easyLink.bot.webLimit", p = p),
        ChatErrorKind::WebSearchPaused => t!("easyLink.bot.webPaused", p = p),
        ChatErrorKind::WebSearchOutdated => t!("easyLink.bot.webOutdated", p = p),
        ChatErrorKind::EngineFailed => t!("easyLink.bot.engineDown"),
        // Le correspondant ne peut pas compacter : l'envoi suivant le fera de
        // lui-même.
        ChatErrorKind::ContextFull => t!("easyLink.bot.contextFull"),
        _ => format!("⚠️ {message}"),
    }
}

/// PNG d'une image produite pour la réponse, lu en base ; rien s'il manque
/// (supprimé entre-temps) ou n'est pas un PNG.
pub(super) async fn generated_png(store: &Store, id: i64) -> Option<Vec<u8>> {
    let image = store.call(move |c| chats::image_attachment(c, id)).await;
    match image {
        Ok(Some((mime, png))) if mime == "image/png" => Some(png),
        _ => None,
    }
}

/// Photos sans légende : gardées pour le prochain message du compte (le
/// moteur de chat exige un texte). Sans le verrou « occupé » : un album
/// arrive en plusieurs messages, et la légende n'accompagne que le premier.
async fn keep_images<H: Host>(
    shared: &Arc<Shared<H>>,
    messenger: &Messenger,
    account: &Account,
    incoming: &Incoming,
) {
    let channel = incoming.channel.as_str();
    let p = prefix(account.platform);
    let settings = match app_settings(&shared.store).await {
        Ok(settings) => settings,
        Err(error) => {
            let text = t!("easyLink.bot.failed", error = error);
            return say(messenger, channel, &text).await;
        }
    };
    let model = model_of(account, &settings.selected_model);
    match usable(&shared.access, &settings, &model, p).await {
        Ok(chosen) if sees(&chosen) => {}
        Ok(_) => {
            let text = t!("easyLink.bot.noVision", model = code(&model), p = p);
            return say(messenger, channel, &text).await;
        }
        Err(reply) => return say(messenger, channel, &reply).await,
    }
    let mut first = false;
    for image in &incoming.files {
        let waiting = shared.pending.lock().get(&account.id).map_or(0, Vec::len);
        match stage(shared, messenger, image, waiting).await {
            Ok(id) => {
                // Plafond revérifié sous le verrou : les photos d'un album
                // sont préparées en même temps.
                let kept = {
                    let mut pending = shared.pending.lock();
                    let kept = pending.entry(account.id).or_default();
                    if kept.len() >= MAX_IMAGES {
                        Err(id)
                    } else {
                        first |= kept.is_empty();
                        kept.push(id);
                        Ok(())
                    }
                };
                if let Err(id) = kept {
                    shared.staging.unstage(&[id]);
                    let reply = t!("easyLink.bot.tooManyImages", max = MAX_IMAGES);
                    return say(messenger, channel, &reply).await;
                }
            }
            Err(reply) => return say(messenger, channel, &reply).await,
        }
    }
    // Compte délié pendant le téléchargement : ses photos ne restent pas en
    // mémoire.
    let (platform, user) = (account.platform, account.user_id.clone());
    let linked = shared
        .store
        .call(move |c| saved::account(c, platform, &user))
        .await;
    if matches!(linked, Ok(None)) {
        if let Some(files) = shared.pending.lock().remove(&account.id) {
            shared.staging.unstage(&files);
        }
        return;
    }
    // Une seule invite pour un album, et aucune pendant une réponse (ces
    // photos accompagnent la légende à laquelle le bot répond déjà).
    if first && !shared.busy.lock().contains_key(&account.id) {
        let text = t!("easyLink.bot.imagePending");
        say(messenger, channel, &text).await;
    }
}

/// Télécharge une photo et la prépare pour le moteur de chat ; rend son
/// identifiant, ou le texte du refus.
async fn stage<H: Host>(
    shared: &Shared<H>,
    messenger: &Messenger,
    image: &IncomingFile,
    already: usize,
) -> Result<String, String> {
    if already >= MAX_IMAGES {
        return Err(t!("easyLink.bot.tooManyImages", max = MAX_IMAGES));
    }
    if image.size.is_some_and(|size| size > MAX_FILE_SIZE) {
        return Err(t!("easyLink.bot.fileTooLarge"));
    }
    let data = messenger
        .download(&image.handle, MAX_FILE_SIZE)
        .await
        .map_err(download_error)?;
    // Nom d'après le contenu : la zone de préparation juge sur l'extension,
    // et celle qu'annonce une messagerie n'engage personne.
    let name = image_name(&data).ok_or_else(|| t!("easyLink.bot.unsupportedFile"))?;
    match shared.staging.stage_bytes(name.to_owned(), data).await {
        StageResult::Staged { file } => Ok(file.id),
        StageResult::Rejected { error, .. } => Err(error),
    }
}

fn download_error(problem: Problem) -> String {
    match problem {
        Problem::TooLarge => t!("easyLink.bot.fileTooLarge"),
        _ => t!("easyLink.bot.fileFailed"),
    }
}

/// Nom d'image d'après les premiers octets (PNG, JPEG, WebP), sinon rien.
pub(super) fn image_name(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("photo.png")
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("photo.jpg")
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("photo.webp")
    } else {
        None
    }
}

/// Transcrit un message vocal ; rend le texte, ou ce que le bot en dit.
async fn hear<H: Host>(
    shared: &Shared<H>,
    messenger: &Messenger,
    file: &IncomingFile,
) -> Result<String, String> {
    if file.size.is_some_and(|size| size > MAX_VOICE_BYTES) {
        return Err(t!("easyLink.bot.fileTooLarge"));
    }
    let data = messenger
        .download(&file.handle, MAX_VOICE_BYTES)
        .await
        .map_err(download_error)?;
    let decoded = tauri::async_runtime::spawn_blocking(move || {
        voice::decode_ogg_opus(&data, MAX_VOICE_SECS)
            .map(|sound| audio::resample(&sound.samples, sound.rate, audio::RATE))
    })
    .await;
    let samples = match decoded {
        Ok(Ok(samples)) => samples,
        Ok(Err(voice::VoiceError::TooLong)) => return Err(t!("easyLink.bot.voiceTooLong")),
        Ok(Err(voice::VoiceError::Unreadable)) | Err(_) => {
            return Err(t!("easyLink.bot.voiceUnreadable"));
        }
    };
    // Whisper invente du texte sur le silence : rien n'est transcrit sans
    // parole, comme pour une dictée.
    if audio::duration(&samples) < audio::MIN_DURATION || !audio::has_speech(&samples) {
        return Err(t!("easyLink.bot.voiceEmpty"));
    }
    match shared.host.transcribe(samples).await {
        Ok(heard) if heard.trim().is_empty() => Err(t!("easyLink.bot.voiceEmpty")),
        Ok(heard) => Ok(heard),
        Err(HearError::NoModel) => Err(t!("easyLink.bot.voiceNoModel")),
        // Texte du moteur de dictée, déjà traduit.
        Err(HearError::Failed(error)) => Err(error),
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    fn entry(id: &str, name: &str) -> ChatModel {
        ChatModel {
            id: id.into(),
            display_name: name.into(),
            origin: crate::llm::models::ModelOrigin::Hf,
            provider: None,
            size_bytes: None,
            params_label: None,
            quant: None,
            context_max: None,
            capabilities: vec!["completion".into()],
            installed_at: None,
        }
    }

    fn listed() -> Vec<ChatModel> {
        vec![
            entry("unsloth/gemma-3-4b-it-GGUF:Q4_K_M", "Gemma 3 4B"),
            entry("unsloth/gemma-3-4b-it-GGUF:Q8_0", "Gemma 3 4B"),
            entry("Qwen/Qwen3-8B-GGUF:Q4_K_M", "Qwen3 8B"),
            entry("@ollama/llava:latest", "llava"),
            entry("unsloth/Qwen3-8B-Instruct-GGUF:Q4_K_M", "Qwen3 8B Instruct"),
        ]
    }

    #[test]
    fn a_model_is_picked_by_number_id_name_or_part_of_them() {
        let listed = listed();
        let one = |id: &str| Picked::One(id.into());
        assert_eq!(pick(&listed, "1"), one("unsloth/gemma-3-4b-it-GGUF:Q4_K_M"));
        assert_eq!(pick(&listed, " 4 "), one("@ollama/llava:latest"));
        assert_eq!(pick(&listed, "0"), Picked::None);
        assert_eq!(pick(&listed, "6"), Picked::None);
        // Identifiant : quantification sans la casse ; un début d'identifiant
        // suffit s'il est seul à correspondre.
        assert_eq!(
            pick(&listed, "Qwen/Qwen3-8B-GGUF:q4_k_m"),
            one("Qwen/Qwen3-8B-GGUF:Q4_K_M")
        );
        assert_eq!(pick(&listed, "@ollama/llava"), one("@ollama/llava:latest"));
        // Nom exact, sans la casse : gagne sur un nom qui le contient.
        assert_eq!(pick(&listed, "QWEN3 8B"), one("Qwen/Qwen3-8B-GGUF:Q4_K_M"));
        assert_eq!(pick(&listed, "LLAVA"), one("@ollama/llava:latest"));
        // Deux quantifications du même dépôt : même nom.
        assert_eq!(
            pick(&listed, "gemma 3 4b"),
            Picked::Many(vec![
                "unsloth/gemma-3-4b-it-GGUF:Q4_K_M".into(),
                "unsloth/gemma-3-4b-it-GGUF:Q8_0".into(),
            ])
        );
        // Partie de l'identifiant ou du nom.
        assert_eq!(
            pick(&listed, "instruct"),
            one("unsloth/Qwen3-8B-Instruct-GGUF:Q4_K_M")
        );
        assert_eq!(
            pick(&listed, "qwen"),
            Picked::Many(vec![
                "Qwen/Qwen3-8B-GGUF:Q4_K_M".into(),
                "unsloth/Qwen3-8B-Instruct-GGUF:Q4_K_M".into(),
            ])
        );
        assert_eq!(
            pick(&listed, "q8_0"),
            one("unsloth/gemma-3-4b-it-GGUF:Q8_0")
        );
        assert_eq!(pick(&listed, "mistral"), Picked::None);
    }

    /// Chaque arrêt de la recherche web a son conseil ; le moteur en panne
    /// est dit sans son détail ; le reste garde le message du chat.
    #[test]
    fn each_error_code_has_its_note() {
        let note = |kind| error_note(kind, "detail", "/");
        assert_eq!(
            note(ChatErrorKind::WebSearchUnavailable),
            t!("easyLink.bot.webUnavailable", p = "/")
        );
        assert_eq!(
            note(ChatErrorKind::WebSearchLimit),
            t!("easyLink.bot.webLimit", p = "/")
        );
        assert_eq!(
            note(ChatErrorKind::WebSearchPaused),
            t!("easyLink.bot.webPaused", p = "/")
        );
        assert_eq!(
            note(ChatErrorKind::WebSearchOutdated),
            t!("easyLink.bot.webOutdated", p = "/")
        );
        assert_eq!(
            note(ChatErrorKind::EngineFailed),
            t!("easyLink.bot.engineDown")
        );
        assert_eq!(
            note(ChatErrorKind::ContextFull),
            t!("easyLink.bot.contextFull")
        );
        for kind in [
            ChatErrorKind::EngineUnavailable,
            ChatErrorKind::RemoteOff,
            ChatErrorKind::RemoteKey,
            ChatErrorKind::ModelMissing,
            ChatErrorKind::Model,
        ] {
            assert_eq!(note(kind), "⚠️ detail", "{kind:?}");
        }
        // Les quatre conseils de la recherche sont distincts et disent
        // `/web off`.
        let web: Vec<String> = [
            ChatErrorKind::WebSearchUnavailable,
            ChatErrorKind::WebSearchLimit,
            ChatErrorKind::WebSearchPaused,
            ChatErrorKind::WebSearchOutdated,
        ]
        .into_iter()
        .map(note)
        .collect();
        assert!(web.iter().all(|text| text.contains("/web off")), "{web:?}");
        let mut distinct = web.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), 4);
        assert!(
            !web.iter()
                .chain([&note(ChatErrorKind::EngineFailed)])
                .any(|text| text.contains("Ollama"))
        );
    }

    #[test]
    fn an_image_is_named_after_its_content() {
        assert_eq!(image_name(b"\x89PNG\r\n\x1a\n...."), Some("photo.png"));
        assert_eq!(image_name(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("photo.jpg"));
        assert_eq!(image_name(b"RIFF\x10\0\0\0WEBPVP8 "), Some("photo.webp"));
        assert_eq!(image_name(b"GIF89a"), None);
        assert_eq!(image_name(b"<svg"), None);
        assert_eq!(image_name(b""), None);
    }

    #[test]
    fn the_account_model_wins_over_the_app_model() {
        let mut account = Account {
            id: 1,
            platform: Platform::Telegram,
            user_id: "7".into(),
            name: String::new(),
            chat_id: None,
            model: String::new(),
            web_search: false,
            ephemeral: false,
        };
        let gemma = "unsloth/gemma-3-4b-it-GGUF:Q4_K_M";
        assert_eq!(model_of(&account, &format!(" {gemma} ")), gemma);
        assert_eq!(model_of(&account, ""), "");
        account.model = "Qwen/Qwen3-8B-GGUF:Q4_K_M".into();
        assert_eq!(model_of(&account, gemma), "Qwen/Qwen3-8B-GGUF:Q4_K_M");
    }
}
