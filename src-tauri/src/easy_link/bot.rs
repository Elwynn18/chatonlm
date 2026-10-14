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
use crate::ollama::{models, running};
use crate::store::easy_link::{self as saved, Account, Platform};
use crate::store::{chats, model_prefs, now_ms};

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

/// Modèle désigné par un numéro de la liste ou par un nom.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Picked {
    One(String),
    None,
    Many(Vec<String>),
}

/// Numéro (à partir de 1) de la liste, nom exact (à `:latest` et à la casse
/// près), sinon le seul nom qui contient le texte.
pub(super) fn pick(names: &[String], wanted: &str) -> Picked {
    let wanted = wanted.trim();
    if let Ok(number) = wanted.parse::<usize>() {
        return match number.checked_sub(1).and_then(|index| names.get(index)) {
            Some(name) => Picked::One(name.clone()),
            None => Picked::None,
        };
    }
    if let Some(exact) = names.iter().find(|name| running::same_model(name, wanted)) {
        return Picked::One(exact.clone());
    }
    let needle = wanted.to_lowercase();
    let mut matching: Vec<String> = names
        .iter()
        .filter(|name| name.to_lowercase().contains(&needle))
        .cloned()
        .collect();
    match matching.len() {
        0 => Picked::None,
        1 => Picked::One(matching.remove(0)),
        _ => Picked::Many(matching),
    }
}

/// Noms des modèles installés, triés (les numéros de `/models` restent les
/// mêmes d'un appel à l'autre).
async fn installed(base: &str) -> crate::error::AppResult<Vec<String>> {
    let mut names: Vec<String> = models::list(base)
        .await?
        .into_iter()
        .map(|model| model.model)
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    Ok(names)
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
            match shared
                .store
                .call(move |c| saved::set_chat(c, id, None))
                .await
            {
                Ok(()) => t!("easyLink.bot.newChat"),
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
        Command::Unknown(name) => {
            t!(
                "easyLink.bot.unknownCommand",
                command = format!("/{name}"),
                p = p
            )
        }
        // Le reste parle à Ollama.
        command => {
            let _ = messenger.typing(channel).await;
            match with_server(shared, messenger, account, channel, command).await {
                Ok(reply) => reply,
                Err(error) => t!("easyLink.bot.failed", error = error),
            }
        }
    };
    say(messenger, channel, &reply).await;
}

fn web_line(enabled: bool) -> String {
    if enabled {
        t!("easyLink.bot.statusWebOn")
    } else {
        t!("easyLink.bot.statusWebOff")
    }
}

/// Commandes qui interrogent Ollama.
async fn with_server<H: Host>(
    shared: &Arc<Shared<H>>,
    messenger: &Messenger,
    account: &Account,
    channel: &str,
    command: Command,
) -> crate::error::AppResult<String> {
    let p = prefix(account.platform);
    let settings = crate::ollama::target::request_settings(&shared.store).await?;
    let base = settings.effective_server_url.clone();
    let current = model_of(account, &settings.selected_model);
    // Modèle visé par `/load` et `/unload` : celui nommé, sinon le courant.
    let named = |names: &[String], wanted: Option<&str>| -> Result<String, String> {
        match wanted {
            None if current.is_empty() => Err(t!("easyLink.bot.noModel", p = p)),
            None => Ok(current.clone()),
            Some(wanted) => match pick(names, wanted) {
                Picked::One(name) => Ok(name),
                Picked::None => Err(t!("easyLink.bot.modelUnknown", name = wanted, p = p)),
                Picked::Many(found) => Err(t!(
                    "easyLink.bot.modelAmbiguous",
                    name = wanted,
                    models = found.join(", ")
                )),
            },
        }
    };
    Ok(match command {
        Command::Models => {
            let names = installed(&base).await?;
            if names.is_empty() {
                return Ok(t!("easyLink.bot.noModels"));
            }
            let loaded = running::list(&base).await.unwrap_or_default();
            let mut lines = vec![t!("easyLink.bot.modelsTitle")];
            for (index, name) in names.iter().enumerate() {
                let mut notes = Vec::new();
                if running::same_model(name, &current) {
                    notes.push(t!("easyLink.bot.modelCurrent"));
                }
                if loaded.iter().any(|l| running::same_model(&l.name, name)) {
                    notes.push(t!("easyLink.bot.modelLoaded"));
                }
                let notes = if notes.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", notes.join(", "))
                };
                lines.push(format!("{}. {}{notes}", index + 1, code(name)));
            }
            lines.push(String::new());
            lines.push(t!("easyLink.bot.modelsHint", p = p));
            lines.join("\n")
        }
        Command::Model(None) if current.is_empty() => t!("easyLink.bot.noModel", p = p),
        Command::Model(None) => t!("easyLink.bot.model", model = code(&current)),
        Command::Model(Some(wanted)) => {
            let names = installed(&base).await?;
            match named(&names, Some(&wanted)) {
                Ok(name) => {
                    let (id, chosen) = (account.id, name.clone());
                    shared
                        .store
                        .call(move |c| saved::set_model(c, id, &chosen))
                        .await?;
                    t!("easyLink.bot.modelChosen", model = code(&name))
                }
                Err(reply) => reply,
            }
        }
        Command::Load(wanted) => {
            let names = match &wanted {
                Some(_) => installed(&base).await?,
                None => Vec::new(),
            };
            match named(&names, wanted.as_deref()) {
                Ok(name) => {
                    let loading = t!("easyLink.bot.loading", model = code(&name));
                    say(messenger, channel, &loading).await;
                    let _ = messenger.typing(channel).await;
                    crate::commands::models::load(&shared.store, &name).await?;
                    t!("easyLink.bot.loaded", model = code(&name))
                }
                Err(reply) => reply,
            }
        }
        Command::Unload(Some(all)) if all.eq_ignore_ascii_case("all") => {
            running::unload_all(&base).await?;
            t!("easyLink.bot.unloadedAll")
        }
        Command::Unload(wanted) => {
            let names = match &wanted {
                Some(_) => installed(&base).await?,
                None => Vec::new(),
            };
            match named(&names, wanted.as_deref()) {
                Ok(name) => {
                    running::unload(&base, &name).await?;
                    t!("easyLink.bot.unloaded", model = code(&name))
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
            match running::list(&base).await {
                Ok(loaded) if loaded.is_empty() => {
                    lines.push(t!("easyLink.bot.statusNothingLoaded"));
                }
                Ok(loaded) => {
                    let names: Vec<String> = loaded.iter().map(|l| code(&l.name)).collect();
                    lines.push(t!("easyLink.bot.statusLoaded", models = names.join(", ")));
                }
                Err(error) => lines.push(t!("easyLink.bot.statusUnreachable", error = error)),
            }
            lines.push(web_line(account.web_search));
            lines.join("\n")
        }
        // Traitées par `run`.
        Command::Help
        | Command::New
        | Command::Stop
        | Command::Unlink
        | Command::Web(_)
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

    let settings = match crate::ollama::target::request_settings(&shared.store).await {
        Ok(settings) => settings,
        Err(error) => {
            return {
                let text = t!("easyLink.bot.failed", error = error);
                say(messenger, channel, &text).await
            };
        }
    };
    let model = model_of(&account, &settings.selected_model);
    if model.is_empty() {
        return {
            let text = t!("easyLink.bot.noModel", p = p);
            say(messenger, channel, &text).await
        };
    }
    // Modèle absent du serveur (supprimé depuis son choix) : le moteur de
    // chat le téléchargerait de lui-même, ce qu'un bot ne doit pas déclencher.
    // Serveur injoignable ou autre échec : le moteur le dira.
    let details = match models::show(&settings.effective_server_url, &model).await {
        Ok(details) => Some(details),
        Err(models::ShowError::NotFound(_)) => {
            return {
                let text = t!("easyLink.bot.modelMissing", model = code(&model), p = p);
                say(messenger, channel, &text).await
            };
        }
        Err(_) => None,
    };
    let can_see = chat::accepts_images(details.as_ref());
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
    };
    let server = Server::from_settings(&settings);
    let (events, mut received) = mpsc::unbounded_channel::<ChatEvent>();
    // L'envoi dans sa tâche : la lecture de la réponse d'Ollama n'attend pas
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
    let mut failure: Option<(ChatErrorKind, String)> = None;
    let mut done = false;
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut typed = Instant::now();
    let mut refreshed = Instant::now();
    loop {
        tokio::select! {
            event = received.recv() => {
                // Canal fermé : l'envoi est fini.
                let Some(event) = event else { break };
                match event {
                    ChatEvent::ChatCreated { chat_id: id } => {
                        let (account_id, platform, chat) = (account.id, account.platform, id.clone());
                        let _ = shared
                            .store
                            .call(move |c| {
                                chats::set_source(c, &chat, platform)?;
                                saved::set_chat(c, account_id, Some(&chat))
                            })
                            .await;
                        shared.host.chat_updated(&id);
                        chat_id = Some(id);
                    }
                    ChatEvent::UserMessage { .. } => {
                        if let Some(id) = &chat_id {
                            shared.host.chat_updated(id);
                        }
                    }
                    ChatEvent::ReplyStarted { .. } => relay.new_turn(),
                    ChatEvent::Content { text } => relay.push(&text),
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
                    let _ = messenger.typing(channel).await;
                }
                if refreshed.elapsed() >= REFRESH_EVERY && let Some(id) = &chat_id {
                    refreshed = Instant::now();
                    shared.host.chat_updated(id);
                }
            }
        }
    }
    let sent = sending.await;
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
            Some((ChatErrorKind::WebSearchSignedOut, _)) => {
                Some(t!("easyLink.bot.webSignedOut", p = p))
            }
            // Sans l'adresse du serveur, qui ne regarde pas le correspondant.
            Some((ChatErrorKind::Unreachable, _)) => Some(t!("easyLink.bot.ollamaDown")),
            // Le correspondant ne peut pas compacter : l'envoi suivant le
            // fera de lui-même.
            Some((ChatErrorKind::ContextFull, _)) => Some(t!("easyLink.bot.contextFull")),
            Some((_, message)) => Some(format!("⚠️ {message}")),
            None if done && relay.is_empty() => Some(t!("easyLink.bot.emptyAnswer")),
            None => None,
        },
    };
    if let Some(note) = note
        && !relay.gone()
    {
        say(messenger, channel, &note).await;
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
    let settings = match crate::ollama::target::request_settings(&shared.store).await {
        Ok(settings) => settings,
        Err(error) => {
            let text = t!("easyLink.bot.failed", error = error);
            return say(messenger, channel, &text).await;
        }
    };
    let model = model_of(account, &settings.selected_model);
    if let Err(reply) = sees(&settings.effective_server_url, &model, p).await {
        return say(messenger, channel, &reply).await;
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

/// Le modèle sait voir (ou n'en dit rien) ; sinon, ce que le bot répond.
async fn sees(base: &str, model: &str, p: &str) -> Result<(), String> {
    if model.is_empty() {
        return Err(t!("easyLink.bot.noModel", p = p));
    }
    let details = models::show(base, model).await.ok();
    if chat::accepts_images(details.as_ref()) {
        Ok(())
    } else {
        Err(t!("easyLink.bot.noVision", model = code(model), p = p))
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

    fn names() -> Vec<String> {
        ["gemma3:4b", "gemma3:270m", "qwen3:8b", "llava"]
            .map(String::from)
            .to_vec()
    }

    #[test]
    fn a_model_is_picked_by_number_name_or_part_of_name() {
        let names = names();
        assert_eq!(pick(&names, "1"), Picked::One("gemma3:4b".into()));
        assert_eq!(pick(&names, " 4 "), Picked::One("llava".into()));
        assert_eq!(pick(&names, "0"), Picked::None);
        assert_eq!(pick(&names, "5"), Picked::None);
        assert_eq!(pick(&names, "QWEN3:8B"), Picked::One("qwen3:8b".into()));
        assert_eq!(pick(&names, "llava:latest"), Picked::One("llava".into()));
        assert_eq!(pick(&names, "qwen"), Picked::One("qwen3:8b".into()));
        assert_eq!(
            pick(&names, "gemma"),
            Picked::Many(vec!["gemma3:4b".into(), "gemma3:270m".into()])
        );
        assert_eq!(pick(&names, "mistral"), Picked::None);
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
        };
        assert_eq!(model_of(&account, " gemma3:4b "), "gemma3:4b");
        assert_eq!(model_of(&account, ""), "");
        account.model = "qwen3:8b".into();
        assert_eq!(model_of(&account, "gemma3:4b"), "qwen3:8b");
    }
}
