//! Bot Telegram (Bot API, `https://core.telegram.org/bots/api`) : messages
//! reçus par `getUpdates` en attente longue, sans webhook ni port ouvert.

use std::time::Duration;

use reqwest::multipart::{Form, Part};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

use crate::{Event, FileHandle, FileKind, Identity, Incoming, IncomingFile, Problem, Sender, text};

/// Adresse de l'API (l'app la remplace par un faux serveur dans ses essais).
pub const DEFAULT_URL: &str = "https://api.telegram.org";

/// Longueur sûre d'un message : la limite est de 4096 unités UTF-16 après
/// lecture de la mise en forme.
pub const TEXT_LIMIT: usize = 4000;

/// Délai de `getMe`, raccourci dans les essais (serveur muet). Sous Windows,
/// le refus d'un port fermé de la machine n'arrive qu'après environ 2 s (la
/// pile réseau réessaie) : l'essai d'une connexion refusée y prendrait ce
/// refus pour un délai dépassé.
const IDENTITY_TIMEOUT: Duration = if cfg!(all(test, windows)) {
    Duration::from_secs(3)
} else if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(15)
};
/// Attente longue demandée à Telegram, et délai de la requête qui la porte.
const POLL_SECONDS: u64 = 50;
const POLL_TIMEOUT: Duration = Duration::from_secs(65);
const SEND_TIMEOUT: Duration = Duration::from_secs(30);
const TYPING_TIMEOUT: Duration = Duration::from_secs(10);
const FILE_TIMEOUT: Duration = Duration::from_secs(15);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
/// Envoi d'une image : plusieurs mégaoctets sur une connexion lente.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Plus grosse photo acceptée par `sendPhoto` (« 10 MB ») : au-delà, l'image
/// part en document.
pub const PHOTO_MAX: usize = 10_000_000;
/// Plus gros fichier qu'un bot peut envoyer (« 50 MB ») : au-delà,
/// [`Problem::TooLarge`] sans requête.
pub const DOCUMENT_MAX: usize = 50_000_000;
/// Longueur sûre d'une légende : la limite est de 1024 unités UTF-16 après
/// lecture de la mise en forme.
pub const CAPTION_LIMIT: usize = 1000;

/// Attente avant un nouvel essai de réception : doublée à chaque échec.
pub(crate) const FIRST_WAIT: Duration = Duration::from_secs(1);
pub(crate) const MAX_WAIT: Duration = Duration::from_secs(60);
/// Une autre instance lit les messages : inutile d'insister trop vite.
pub(crate) const CONFLICT_WAIT: Duration = Duration::from_secs(5);
/// Attente du premier lot après un conflit. Un lot sans attente passerait
/// même quand une autre instance lit les messages : seule une attente longue
/// se heurte à la sienne (409). La réception n'est donc annoncée de nouveau
/// qu'après une vraie attente menée à son terme.
pub(crate) const CONFLICT_PROBE_SECONDS: u64 = if cfg!(test) { 1 } else { 3 };
/// Les essais ne patientent pas des secondes réelles : attentes divisées.
pub(crate) const WAIT_DIVISOR: u32 = if cfg!(test) { 100 } else { 1 };

/// Au-delà, une photo n'est pas lue : une taille plus petite est préférée.
const PHOTO_LIMIT: u64 = 10 * 1024 * 1024;

/// Refus de Telegram traités à part.
const UNPARSED: &str = "can't parse entities";
const NOT_MODIFIED: &str = "message is not modified";
const TOO_BIG: &str = "file is too big";

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
}

/// Jamais le jeton dans un journal.
impl std::fmt::Debug for Client {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Client")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// `http` : client de l'app (proxy, fournisseur TLS) ; les délais sont
    /// posés ici, requête par requête. `base` sans « / » final.
    pub fn new(http: reqwest::Client, base: &str, token: &str) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            token: token.trim().to_owned(),
        }
    }

    pub async fn identity(&self) -> Result<Identity, Problem> {
        let me = self.call("getMe", &json!({}), IDENTITY_TIMEOUT).await?;
        match (me["id"].as_i64(), me["username"].as_str()) {
            (Some(id), Some(name)) => Ok(Identity {
                id: id.to_string(),
                name: name.to_owned(),
            }),
            _ => Err(unreadable()),
        }
    }

    pub async fn listen(&self, events: mpsc::Sender<Event>, mut stop: watch::Receiver<bool>) {
        let mut identity = None;
        let mut offset = None;
        // Vrai tant que la réception marche (annoncée par `Connected`).
        let mut receiving = false;
        // Coupure déjà annoncée : ses essais suivants échouent en silence.
        let mut announced = false;
        // Dernier échec : une autre instance lisait les messages.
        let mut conflicted = false;
        let mut wait = FIRST_WAIT;
        loop {
            let step = tokio::select! {
                step = self.step(&mut identity, offset, receiving, conflicted) => step,
                () = stopped(&mut stop) => return,
                () = events.closed() => return,
            };
            let updates = match step {
                Ok(updates) => updates,
                Err(problem) => {
                    receiving = false;
                    conflicted = problem == Problem::Conflict;
                    let last = problem == Problem::Unauthorized;
                    let pause = pause(&problem, wait);
                    if (last || !announced)
                        && !emit(&events, &mut stop, Event::Disconnected(problem)).await
                    {
                        return;
                    }
                    if last {
                        return;
                    }
                    announced = true;
                    wait = (wait * 2).min(MAX_WAIT);
                    tokio::select! {
                        () = tokio::time::sleep(pause / WAIT_DIVISOR) => continue,
                        () = stopped(&mut stop) => return,
                        () = events.closed() => return,
                    }
                }
            };
            if !receiving {
                receiving = true;
                conflicted = false;
                announced = false;
                wait = FIRST_WAIT;
                let Some(identity) = identity.clone() else {
                    return;
                };
                if !emit(&events, &mut stop, Event::Connected(identity)).await {
                    return;
                }
            }
            for update in &updates.list {
                if let Some(incoming) = incoming(update)
                    && !emit(&events, &mut stop, Event::Message(incoming)).await
                {
                    return;
                }
            }
            // Acquitte tout le lot, messages ignorés compris.
            offset = updates.next.or(offset);
        }
    }

    /// Un pas de l'écoute : le bot d'abord (une fois), puis un lot de
    /// messages. Le premier lot après une coupure est demandé sans attente,
    /// pour annoncer aussitôt que la réception marche ; après un conflit, avec
    /// une courte attente ([`CONFLICT_PROBE_SECONDS`]).
    async fn step(
        &self,
        identity: &mut Option<Identity>,
        offset: Option<i64>,
        receiving: bool,
        conflicted: bool,
    ) -> Result<Updates, Problem> {
        if identity.is_none() {
            *identity = Some(self.identity().await?);
        }
        let mut body = json!({
            "timeout": match (receiving, conflicted) {
                (true, _) => POLL_SECONDS,
                (false, true) => CONFLICT_PROBE_SECONDS,
                (false, false) => 0,
            },
            "allowed_updates": ["message"],
        });
        if let Some(offset) = offset {
            body["offset"] = offset.into();
        }
        let Value::Array(list) = self.call("getUpdates", &body, POLL_TIMEOUT).await? else {
            return Err(unreadable());
        };
        let next = list
            .iter()
            .filter_map(|update| update["update_id"].as_i64())
            .max()
            .map(|last| last.saturating_add(1));
        if next.is_none() && !list.is_empty() {
            // Rien à acquitter : le même lot reviendrait sans fin. Il passe
            // par l'attente des échecs.
            return Err(unreadable());
        }
        Ok(Updates { list, next })
    }

    pub async fn send(&self, channel: &str, markdown: &str) -> Result<String, Problem> {
        let body = json!({ "chat_id": number_or_text(channel) });
        let sent = self.write("sendMessage", body, markdown).await?;
        match &sent["message_id"] {
            Value::Number(id) => Ok(id.to_string()),
            _ => Err(unreadable()),
        }
    }

    pub async fn edit(&self, channel: &str, message: &str, markdown: &str) -> Result<(), Problem> {
        let body = json!({
            "chat_id": number_or_text(channel),
            "message_id": number_or_text(message),
        });
        match self.write("editMessageText", body, markdown).await {
            // Même texte que celui affiché : rien à faire.
            Err(Problem::Refused {
                status: 400,
                message,
            }) if message.contains(NOT_MODIFIED) => Ok(()),
            written => written.map(|_| ()),
        }
    }

    /// Écrit un message mis en forme ; si Telegram ne sait pas lire la mise en
    /// forme, le même texte part une fois tel quel, sans elle.
    async fn write(&self, method: &str, mut body: Value, markdown: &str) -> Result<Value, Problem> {
        body["link_preview_options"] = json!({ "is_disabled": true });
        body["text"] = text::telegram_html(markdown).into();
        body["parse_mode"] = "HTML".into();
        match self.call(method, &body, SEND_TIMEOUT).await {
            Err(Problem::Refused {
                status: 400,
                message,
            }) if message.contains(UNPARSED) => {
                body["text"] = markdown.into();
                if let Some(fields) = body.as_object_mut() {
                    fields.remove("parse_mode");
                }
                self.call(method, &body, SEND_TIMEOUT).await
            }
            written => written,
        }
    }

    pub async fn typing(&self, channel: &str) -> Result<(), Problem> {
        self.action(channel, "typing").await
    }

    /// « envoie une photo… » : pendant qu'une image se prépare.
    pub async fn preparing_photo(&self, channel: &str) -> Result<(), Problem> {
        self.action(channel, "upload_photo").await
    }

    async fn action(&self, channel: &str, action: &str) -> Result<(), Problem> {
        let body = json!({ "chat_id": number_or_text(channel), "action": action });
        self.call("sendChatAction", &body, TYPING_TIMEOUT).await?;
        Ok(())
    }

    /// Image PNG en photo (document au-delà de [`PHOTO_MAX`]) ; `filename`
    /// déjà assaini, `caption` non vide. Une légende dont Telegram ne sait
    /// pas lire la mise en forme repart une fois telle quelle, comme un
    /// message.
    pub(crate) async fn send_photo(
        &self,
        channel: &str,
        png: Vec<u8>,
        filename: &str,
        caption: Option<&str>,
    ) -> Result<String, Problem> {
        if png.len() > DOCUMENT_MAX {
            return Err(Problem::TooLarge);
        }
        let (method, field) = if png.len() > PHOTO_MAX {
            ("sendDocument", "document")
        } else {
            ("sendPhoto", "photo")
        };
        let form = |png: Vec<u8>, formatted: bool| -> Result<Form, Problem> {
            let part = Part::bytes(png)
                .file_name(filename.to_owned())
                .mime_str("image/png")
                .map_err(|error| self.network(error))?;
            let mut form = Form::new()
                .text("chat_id", channel.to_owned())
                .part(field, part);
            match caption {
                Some(caption) if formatted => {
                    form = form
                        .text("caption", text::telegram_html(caption))
                        .text("parse_mode", "HTML");
                }
                Some(caption) => form = form.text("caption", caption.to_owned()),
                None => {}
            }
            Ok(form)
        };
        // Copie gardée pour le second envoi, seulement s'il peut avoir lieu.
        let spare = caption.is_some().then(|| png.clone());
        let first = form(png, true)?;
        let sent = match self
            .call_with(method, UPLOAD_TIMEOUT, |request| request.multipart(first))
            .await
        {
            Err(Problem::Refused {
                status: 400,
                message,
            }) if message.contains(UNPARSED) && spare.is_some() => {
                let plain = form(spare.unwrap_or_default(), false)?;
                self.call_with(method, UPLOAD_TIMEOUT, |request| request.multipart(plain))
                    .await
            }
            sent => sent,
        }?;
        match &sent["message_id"] {
            Value::Number(id) => Ok(id.to_string()),
            _ => Err(unreadable()),
        }
    }

    pub async fn download(&self, file: &FileHandle, max: u64) -> Result<Vec<u8>, Problem> {
        let FileHandle::Telegram { file_id } = file else {
            return Err(Problem::Refused {
                status: 0,
                message: "not a Telegram file".to_owned(),
            });
        };
        let info = match self
            .call("getFile", &json!({ "file_id": file_id }), FILE_TIMEOUT)
            .await
        {
            // Au-delà de 20 Mo, Telegram ne donne pas le fichier à un bot.
            Err(Problem::Refused { message, .. }) if message.contains(TOO_BIG) => {
                return Err(Problem::TooLarge);
            }
            other => other?,
        };
        let Some(path) = info["file_path"].as_str() else {
            return Err(unreadable());
        };
        if info["file_size"].as_u64().is_some_and(|size| size > max) {
            return Err(Problem::TooLarge);
        }
        let url = format!("{}/file/bot{}/{path}", self.base, self.token);
        let request = self.http.get(url).timeout(DOWNLOAD_TIMEOUT);
        let mut response = request.send().await.map_err(|error| self.network(error))?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Err(match status {
                401 => Problem::Unauthorized,
                429 => Problem::RateLimited(Duration::from_secs(1)),
                _ => Problem::Refused {
                    status,
                    message: "file not served".to_owned(),
                },
            });
        }
        // La taille annoncée n'engage personne : la lecture s'arrête dès que
        // la limite est dépassée.
        let mut content = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| self.network(error))?
        {
            if (content.len() + chunk.len()) as u64 > max {
                return Err(Problem::TooLarge);
            }
            content.extend_from_slice(&chunk);
        }
        Ok(content)
    }

    /// Appelle une méthode de la Bot API et rend son `result`.
    async fn call(&self, method: &str, body: &Value, timeout: Duration) -> Result<Value, Problem> {
        self.call_with(method, timeout, |request| request.json(body))
            .await
    }

    /// Comme [`Client::call`], corps posé par `body` (JSON ou multipart).
    async fn call_with(
        &self,
        method: &str,
        timeout: Duration,
        body: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
    ) -> Result<Value, Problem> {
        let url = format!("{}/bot{}/{method}", self.base, self.token);
        let request = body(self.http.post(url)).timeout(timeout);
        let response = request.send().await.map_err(|error| self.network(error))?;
        let status = response.status().as_u16();
        // Corps refusé par le frontal (page HTML, pas de JSON) : fichier trop
        // gros.
        if status == 413 {
            return Err(Problem::TooLarge);
        }
        let content = response
            .bytes()
            .await
            .map_err(|error| self.network(error))?;
        let Ok(mut reply) = serde_json::from_slice::<Value>(&content) else {
            return Err(Problem::Network(format!(
                "unreadable reply (HTTP {status})"
            )));
        };
        if reply["ok"] == true {
            return Ok(reply["result"].take());
        }
        let code = reply["error_code"]
            .as_u64()
            .map_or(status, |code| code as u16);
        Err(match code {
            // 404 : jeton mal écrit (Telegram ne trouve pas le bot).
            401 | 404 => Problem::Unauthorized,
            409 => Problem::Conflict,
            403 => Problem::Forbidden,
            429 => {
                // Borné : une valeur démesurée ne doit faire déborder aucun
                // calcul de date chez l'appelant.
                let seconds = reply["parameters"]["retry_after"]
                    .as_u64()
                    .unwrap_or(1)
                    .min(86_400);
                Problem::RateLimited(Duration::from_secs(seconds.max(1)))
            }
            200..=299 => unreadable(),
            _ => Problem::Refused {
                status: code,
                message: self.without_token(reply["description"].as_str().unwrap_or_default()),
            },
        })
    }

    /// Échec du réseau, sans l'adresse de la requête : elle contient le jeton.
    fn network(&self, error: reqwest::Error) -> Problem {
        let error = error.without_url();
        let mut text = error.to_string();
        let mut source = std::error::Error::source(&error);
        while let Some(cause) = source {
            let cause_text = cause.to_string();
            if !cause_text.is_empty() && !text.contains(&cause_text) {
                text = format!("{text}: {cause_text}");
            }
            source = cause.source();
        }
        Problem::Network(self.without_token(&text))
    }

    /// Dernier rempart : un texte venu d'ailleurs (serveur, bibliothèque) ne
    /// répète jamais le jeton.
    fn without_token(&self, text: &str) -> String {
        if self.token.is_empty() {
            return text.to_owned();
        }
        text.replace(&self.token, "***")
    }
}

/// Lot de `getUpdates`.
struct Updates {
    list: Vec<Value>,
    /// `offset` qui acquitte tout le lot ; rien pour un lot vide.
    next: Option<i64>,
}

fn unreadable() -> Problem {
    Problem::Network("unreadable reply".to_owned())
}

/// Telegram attend un nombre pour une conversation ou un message ; un nom de
/// canal (`@nom`) reste du texte.
fn number_or_text(value: &str) -> Value {
    value
        .parse::<i64>()
        .map_or_else(|_| value.into(), Value::from)
}

/// Attente avant le prochain essai, après cet échec.
fn pause(problem: &Problem, wait: Duration) -> Duration {
    match problem {
        Problem::RateLimited(asked) => wait.max(*asked),
        Problem::Conflict => wait.max(CONFLICT_WAIT),
        _ => wait,
    }
}

/// Finit quand l'arrêt est demandé. Émetteur disparu : plus personne ne
/// pourrait arrêter l'écoute, elle finit aussi.
async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

/// Remet un événement à l'app ; faux si l'écoute doit finir.
async fn emit(
    events: &mpsc::Sender<Event>,
    stop: &mut watch::Receiver<bool>,
    event: Event,
) -> bool {
    tokio::select! {
        sent = events.send(event) => sent.is_ok(),
        () = stopped(stop) => false,
    }
}

/// Message privé d'une personne porté par une mise à jour ; rien pour un
/// groupe, un canal, un bot ou une mise à jour illisible.
fn incoming(update: &Value) -> Option<Incoming> {
    let message = update.get("message")?;
    let chat = &message["chat"];
    let from = &message["from"];
    if chat["type"] != "private" || from["is_bot"] != false {
        return None;
    }
    let name = [&from["first_name"], &from["last_name"]]
        .into_iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let name = match name.trim() {
        "" => from["username"].as_str().unwrap_or_default(),
        name => name,
    };
    let text = message["text"]
        .as_str()
        .or_else(|| message["caption"].as_str());
    Some(Incoming {
        id: message["message_id"].as_i64()?.to_string(),
        sender: Sender {
            id: from["id"].as_i64()?.to_string(),
            name: name.to_owned(),
        },
        channel: chat["id"].as_i64()?.to_string(),
        text: text.unwrap_or_default().to_owned(),
        files: files(message),
        sent_at: message["date"].as_i64(),
    })
}

fn files(message: &Value) -> Vec<IncomingFile> {
    let mut files = Vec::new();
    let file = |kind, name: &str, file: &Value| {
        Some(IncomingFile {
            kind,
            name: name.to_owned(),
            size: file["file_size"].as_u64(),
            handle: FileHandle::Telegram {
                file_id: file["file_id"].as_str()?.to_owned(),
            },
        })
    };
    if let Some(sizes) = message["photo"].as_array() {
        // Plusieurs tailles de la même photo : la plus grande qui se lit.
        let largest = |readable: bool| {
            sizes
                .iter()
                .filter(|size| size["file_id"].is_string())
                .filter(|size| {
                    !readable
                        || size["file_size"]
                            .as_u64()
                            .is_none_or(|bytes| bytes <= PHOTO_LIMIT)
                })
                .max_by_key(|size| {
                    size["width"].as_u64().unwrap_or(0) * size["height"].as_u64().unwrap_or(0)
                })
        };
        if let Some(photo) = largest(true).or_else(|| largest(false)) {
            files.extend(file(FileKind::Image, "photo.jpg", photo));
        }
    }
    files.extend(file(FileKind::Voice, "voice.ogg", &message["voice"]));
    let document = &message["document"];
    let extension = match document["mime_type"].as_str() {
        Some("image/png") => Some("png"),
        Some("image/jpeg") => Some("jpg"),
        Some("image/webp") => Some("webp"),
        _ => None,
    };
    let given = document["file_name"]
        .as_str()
        .filter(|name| !name.trim().is_empty());
    files.extend(match extension {
        Some(extension) => file(
            FileKind::Image,
            given.unwrap_or(&format!("image.{extension}")),
            document,
        ),
        None => file(FileKind::Other, given.unwrap_or("file"), document),
    });
    for other in ["audio", "video", "video_note", "sticker"] {
        let name = message[other]["file_name"].as_str().unwrap_or(other);
        files.extend(file(FileKind::Other, name, &message[other]));
    }
    files
}

#[cfg(all(test, feature = "fake"))]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;
    use tokio::time::{Instant, timeout};

    use super::*;
    use crate::fake::telegram::{FakeTelegram, TOKEN};

    /// Borne de toute attente d'un essai.
    const PATIENCE: Duration = Duration::from_secs(10);

    fn http() -> reqwest::Client {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client")
    }

    fn client(fake: &FakeTelegram) -> Client {
        Client::new(http(), &fake.url(), TOKEN)
    }

    fn bot() -> Identity {
        Identity {
            id: "424242".to_owned(),
            name: "chatonlm_test_bot".to_owned(),
        }
    }

    fn refusal(status: u16, description: &str) -> Value {
        json!({ "ok": false, "error_code": status, "description": description })
    }

    /// Serveur qui lit la requête puis répond `reply` tel quel, ou se tait
    /// pour toujours s'il est vide.
    async fn raw_server(reply: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = [0; 4096];
                let _ = socket.read(&mut request).await;
                if reply.is_empty() {
                    held.push(socket);
                } else {
                    let _ = socket.write_all(reply.as_bytes()).await;
                }
            }
        });
        url
    }

    /// Adresse locale où personne n'écoute.
    async fn closed_port() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    }

    /// Requêtes reçues par le faux serveur pour une méthode.
    fn requests(fake: &FakeTelegram, method: &str) -> Vec<Value> {
        fake.requests()
            .into_iter()
            .filter(|request| request["method"] == method)
            .map(|mut request| request["body"].take())
            .collect()
    }

    /// Attend qu'une condition du faux serveur soit vraie.
    async fn until(what: &str, mut condition: impl FnMut() -> bool) {
        let start = Instant::now();
        while !condition() {
            assert!(start.elapsed() < PATIENCE, "never happened: {what}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    struct Listening {
        events: mpsc::Receiver<Event>,
        stop: watch::Sender<bool>,
        task: JoinHandle<()>,
    }

    impl Listening {
        fn start(client: Client) -> Self {
            let (sender, events) = mpsc::channel(16);
            let (stop, stopping) = watch::channel(false);
            let task = tokio::spawn(async move { client.listen(sender, stopping).await });
            Self { events, stop, task }
        }

        async fn next(&mut self) -> Event {
            timeout(PATIENCE, self.events.recv())
                .await
                .expect("an event in time")
                .expect("the listening still runs")
        }

        async fn message(&mut self) -> Incoming {
            match self.next().await {
                Event::Message(incoming) => incoming,
                other => panic!("expected a message, got {other:?}"),
            }
        }

        /// L'écoute finit d'elle-même, sans autre événement.
        async fn ended(mut self) {
            assert_eq!(
                timeout(PATIENCE, self.events.recv())
                    .await
                    .expect("the end in time"),
                None
            );
            timeout(PATIENCE, self.task)
                .await
                .expect("the task in time")
                .unwrap();
        }

        async fn stop(self) {
            self.stop.send(true).unwrap();
            timeout(Duration::from_secs(2), self.task)
                .await
                .expect("stopped at once")
                .unwrap();
        }
    }

    // --- Requêtes et erreurs ---

    #[tokio::test]
    async fn identity_names_the_bot() {
        let fake = FakeTelegram::start().await;
        // Espaces autour du jeton et « / » final de l'adresse : sans effet.
        let client = Client::new(http(), &format!("{}/", fake.url()), &format!(" {TOKEN}\n"));
        assert_eq!(client.identity().await, Ok(bot()));
        assert_eq!(requests(&fake, "getMe"), [json!({})]);
        fake.stop().await;
    }

    #[tokio::test]
    async fn refusals_become_typed_problems() {
        let fake = FakeTelegram::start().await;
        let other = Client::new(http(), &fake.url(), "1:unknown");
        assert_eq!(other.identity().await, Err(Problem::Unauthorized));

        let client = client(&fake);
        let limited = json!({
            "ok": false,
            "error_code": 429,
            "description": "Too Many Requests: retry after 7",
            "parameters": { "retry_after": 7 },
        });
        let cases = [
            (404, refusal(404, "Not Found"), Problem::Unauthorized),
            (
                409,
                refusal(409, "Conflict: terminated by other getUpdates request"),
                Problem::Conflict,
            ),
            (
                403,
                refusal(403, "Forbidden: bot was blocked by the user"),
                Problem::Forbidden,
            ),
            (429, limited, Problem::RateLimited(Duration::from_secs(7))),
            (
                429,
                refusal(429, "Too Many Requests"),
                Problem::RateLimited(Duration::from_secs(1)),
            ),
            (
                400,
                refusal(400, "Bad Request: chat not found"),
                Problem::Refused {
                    status: 400,
                    message: "Bad Request: chat not found".to_owned(),
                },
            ),
            // Sans corps de Telegram (relais) : le statut HTTP décide.
            (403, json!({ "message": "blocked" }), Problem::Forbidden),
            (
                502,
                json!("bad gateway"),
                Problem::Refused {
                    status: 502,
                    message: String::new(),
                },
            ),
            (
                200,
                json!({ "ok": false }),
                Problem::Network("unreadable reply".to_owned()),
            ),
        ];
        for (status, body, problem) in cases {
            fake.fail_next("sendChatAction", status, body.clone());
            assert_eq!(client.typing("7").await, Err(problem), "{status} {body}");
        }
        fake.stop().await;
    }

    #[tokio::test]
    async fn a_refusal_that_repeats_the_token_is_cleaned() {
        let fake = FakeTelegram::start().await;
        let description = format!("Bad Request: wrong url /bot{TOKEN}/sendChatAction");
        fake.fail_next("sendChatAction", 400, refusal(400, &description));
        let problem = client(&fake).typing("7").await.unwrap_err();
        assert_eq!(
            problem,
            Problem::Refused {
                status: 400,
                message: "Bad Request: wrong url /bot***/sendChatAction".to_owned()
            }
        );
        fake.stop().await;
    }

    /// Texte d'un échec du réseau, après avoir vérifié que ni le jeton ni
    /// l'adresse de la requête n'y sont, sous aucune forme.
    fn network_text(problem: Problem) -> String {
        let shown = format!("{problem:?}");
        assert!(!shown.contains(TOKEN), "{shown}");
        assert!(!shown.contains("fake-telegram-token"), "{shown}");
        assert!(!shown.contains("/bot"), "{shown}");
        assert!(!shown.contains("http://"), "{shown}");
        match problem {
            Problem::Network(text) => text,
            other => panic!("expected a network problem, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_token_never_leaks_from_a_refused_connection() {
        let client = Client::new(http(), &closed_port().await, TOKEN);
        let text = network_text(client.identity().await.unwrap_err());
        // Le texte garde la cause, utile au diagnostic.
        assert!(text.to_lowercase().contains("connect"), "{text}");
        network_text(client.send("7", "bonjour").await.unwrap_err());
        network_text(client.edit("7", "1", "bonjour").await.unwrap_err());
        network_text(client.typing("7").await.unwrap_err());
        let file = FileHandle::Telegram {
            file_id: "fake-photo".to_owned(),
        };
        network_text(client.download(&file, 100).await.unwrap_err());

        // L'écoute annonce la même coupure, sans jeton non plus.
        let mut listening = Listening::start(client);
        match listening.next().await {
            Event::Disconnected(problem) => network_text(problem),
            other => panic!("expected a disconnection, got {other:?}"),
        };
        listening.stop().await;
    }

    #[tokio::test]
    async fn the_token_never_leaks_from_a_timeout() {
        let client = Client::new(http(), &raw_server("").await, TOKEN);
        let start = Instant::now();
        let text = network_text(client.identity().await.unwrap_err());
        assert!(text.contains("timed out"), "{text}");
        assert!(start.elapsed() >= IDENTITY_TIMEOUT);
    }

    #[tokio::test]
    async fn the_token_never_leaks_from_an_unreadable_reply() {
        let page = "HTTP/1.1 502 Bad Gateway\r\ncontent-length: 20\r\nconnection: close\r\n\r\n<html>proxy</html>\r\n";
        let client = Client::new(http(), &raw_server(page).await, TOKEN);
        let text = network_text(client.identity().await.unwrap_err());
        assert_eq!(text, "unreadable reply (HTTP 502)");

        // Fichier coupé en route : erreur de lecture du corps.
        let cut = "HTTP/1.1 200 OK\r\ncontent-length: 500\r\nconnection: close\r\n\r\n{\"ok\":";
        let client = Client::new(http(), &raw_server(cut).await, TOKEN);
        network_text(client.identity().await.unwrap_err());
    }

    #[test]
    fn the_debug_text_of_a_client_hides_the_token() {
        let client = Client::new(http(), "https://api.telegram.org", TOKEN);
        let shown = format!("{client:?}");
        assert!(shown.contains("api.telegram.org"), "{shown}");
        assert!(!shown.contains("fake-telegram-token"), "{shown}");
    }

    // --- Écoute ---

    #[tokio::test]
    async fn listening_announces_the_bot_then_delivers_private_messages() {
        let fake = FakeTelegram::start().await;
        let mut listening = Listening::start(client(&fake));
        assert_eq!(listening.next().await, Event::Connected(bot()));

        fake.message(json!({ "user": "7", "name": "Ana Lopez", "text": "bonjour", "age": 90 }));
        let first = listening.message().await;
        assert_eq!(first.sender.id, "7");
        assert_eq!(first.sender.name, "Ana Lopez");
        assert_eq!(
            (first.channel.as_str(), first.text.as_str()),
            ("7", "bonjour")
        );
        assert!(first.files.is_empty());
        let sent_at = first.sent_at.expect("a date");
        fake.message(json!({ "user": "8", "name": "Zoé", "text": "salut" }));
        let second = listening.message().await;
        assert_eq!(
            (second.sender.name.as_str(), second.text.as_str()),
            ("Zoé", "salut")
        );
        // « age » recule la date d'envoi.
        assert!(second.sent_at.expect("a date") - sent_at >= 89);

        // Le premier lot est demandé sans attente, les suivants en attente
        // longue, chacun acquittant le précédent.
        until("the third long wait", || {
            requests(&fake, "getUpdates").len() >= 4
        })
        .await;
        let polls = requests(&fake, "getUpdates");
        assert_eq!(
            polls[0],
            json!({ "timeout": 0, "allowed_updates": ["message"] })
        );
        assert_eq!(
            polls[1],
            json!({ "timeout": 50, "allowed_updates": ["message"] })
        );
        assert_eq!(
            polls[2],
            json!({ "timeout": 50, "allowed_updates": ["message"], "offset": 1001 })
        );
        assert_eq!(
            polls[3],
            json!({ "timeout": 50, "allowed_updates": ["message"], "offset": 1002 })
        );
        assert_eq!(requests(&fake, "getMe").len(), 1);
        listening.stop().await;
        fake.stop().await;
    }

    #[tokio::test]
    async fn messages_of_groups_and_bots_are_ignored_but_acknowledged() {
        let fake = FakeTelegram::start().await;
        let mut listening = Listening::start(client(&fake));
        assert_eq!(listening.next().await, Event::Connected(bot()));

        fake.message(json!({ "user": "7", "text": "dans un groupe", "private": false }));
        // Acquitté seul, sans attendre un message privé.
        until("the group message is acknowledged", || {
            requests(&fake, "getUpdates")
                .last()
                .is_some_and(|poll| poll["offset"] == 1001)
        })
        .await;
        fake.message(json!({ "user": "9", "text": "d'un bot", "bot": true }));
        fake.message(json!({ "user": "7", "text": "en privé" }));
        // Seul le message privé d'une personne arrive.
        assert_eq!(listening.message().await.text, "en privé");
        until("everything is acknowledged", || {
            requests(&fake, "getUpdates")
                .last()
                .is_some_and(|poll| poll["offset"] == 1003)
        })
        .await;
        listening.stop().await;
        fake.stop().await;
    }

    #[tokio::test]
    async fn an_unreadable_update_does_not_stop_the_listening() {
        let fake = FakeTelegram::start().await;
        let private = json!({
            "message_id": 3,
            "from": { "id": 7, "is_bot": false, "first_name": "Ana" },
            "chat": { "id": 7, "type": "private" },
            "date": 1_760_000_000,
            "text": "lisible",
        });
        let batch = json!([
            { "update_id": 7, "message": "illisible" },
            { "sans": "identifiant" },
            { "update_id": 9, "message": { "chat": { "type": "private" }, "from": { "is_bot": false } } },
            { "update_id": 8, "message": private },
            { "update_id": 6, "edited_message": private },
        ]);
        fake.fail_next("getUpdates", 200, json!({ "ok": true, "result": batch }));
        let mut listening = Listening::start(client(&fake));
        assert_eq!(listening.next().await, Event::Connected(bot()));
        let message = listening.message().await;
        assert_eq!(
            (message.text.as_str(), message.sent_at),
            ("lisible", Some(1_760_000_000))
        );
        // Tout le lot est acquitté d'un coup : il n'est pas relu.
        until("the batch is acknowledged", || {
            requests(&fake, "getUpdates")
                .last()
                .is_some_and(|poll| poll["offset"] == 10)
        })
        .await;
        fake.message(json!({ "text": "la suite" }));
        assert_eq!(listening.message().await.text, "la suite");
        listening.stop().await;
        fake.stop().await;
    }

    #[tokio::test]
    async fn a_batch_that_cannot_be_acknowledged_is_not_read_in_a_tight_loop() {
        let fake = FakeTelegram::start().await;
        let batch = json!({ "ok": true, "result": [{ "sans": "identifiant" }] });
        fake.fail_next("getUpdates", 200, batch);
        fake.fail_next(
            "getUpdates",
            200,
            json!({ "ok": true, "result": "pas un tableau" }),
        );
        let mut listening = Listening::start(client(&fake));
        // Traité comme une coupure : annoncée, puis nouvel essai après attente.
        let unreadable = Event::Disconnected(Problem::Network("unreadable reply".to_owned()));
        assert_eq!(listening.next().await, unreadable);
        assert_eq!(listening.next().await, Event::Connected(bot()));
        listening.stop().await;
        fake.stop().await;
    }

    #[tokio::test]
    async fn a_refused_token_ends_the_listening() {
        let fake = FakeTelegram::start().await;
        let mut listening = Listening::start(Client::new(http(), &fake.url(), "1:revoked"));
        assert_eq!(
            listening.next().await,
            Event::Disconnected(Problem::Unauthorized)
        );
        listening.ended().await;
        assert!(fake.requests().is_empty());
        fake.stop().await;
    }

    #[tokio::test]
    async fn a_token_revoked_while_listening_ends_it_even_during_an_outage() {
        let fake = FakeTelegram::start().await;
        let mut listening = Listening::start(client(&fake));
        assert_eq!(listening.next().await, Event::Connected(bot()));
        until("the long wait", || requests(&fake, "getUpdates").len() == 2).await;
        fake.fail_next("getUpdates", 500, refusal(500, "Internal Server Error"));
        fake.fail_next("getUpdates", 401, refusal(401, "Unauthorized"));
        // Finit l'attente longue en cours : la suivante reçoit les refus.
        fake.message(json!({ "text": "dernier message" }));
        assert_eq!(listening.message().await.text, "dernier message");
        let outage = Problem::Refused {
            status: 500,
            message: "Internal Server Error".to_owned(),
        };
        assert_eq!(listening.next().await, Event::Disconnected(outage));
        assert_eq!(
            listening.next().await,
            Event::Disconnected(Problem::Unauthorized)
        );
        listening.ended().await;
        fake.stop().await;
    }

    #[tokio::test]
    async fn an_outage_is_announced_once_and_the_listening_comes_back() {
        let fake = FakeTelegram::start().await;
        let mut listening = Listening::start(client(&fake));
        assert_eq!(listening.next().await, Event::Connected(bot()));
        until("the long wait", || requests(&fake, "getUpdates").len() == 2).await;

        fake.fail_next("getUpdates", 502, refusal(502, "Bad Gateway"));
        fake.fail_next("getUpdates", 409, refusal(409, "Conflict"));
        fake.fail_next("getUpdates", 502, refusal(502, "Bad Gateway"));
        fake.message(json!({ "text": "avant la coupure" }));
        assert_eq!(listening.message().await.text, "avant la coupure");
        // Trois échecs de suite : une seule annonce, celle du premier.
        let outage = Problem::Refused {
            status: 502,
            message: "Bad Gateway".to_owned(),
        };
        assert_eq!(listening.next().await, Event::Disconnected(outage));
        assert_eq!(listening.next().await, Event::Connected(bot()));
        // Le message d'avant est acquitté, rien n'est remis deux fois.
        fake.message(json!({ "text": "après la coupure" }));
        assert_eq!(listening.message().await.text, "après la coupure");

        let polls = requests(&fake, "getUpdates");
        // Après la coupure, la réception reprend par un lot sans attente ;
        // l'essai qui suit le conflit, lui, attend un peu (une autre instance
        // qui lirait encore les messages s'y heurterait).
        let timeouts: Vec<u64> = polls
            .iter()
            .map(|poll| poll["timeout"].as_u64().unwrap())
            .collect();
        assert_eq!(timeouts[..7], [0, 50, 50, 0, CONFLICT_PROBE_SECONDS, 0, 50]);
        assert_eq!(requests(&fake, "getMe").len(), 1);
        listening.stop().await;
        fake.stop().await;
    }

    #[tokio::test]
    async fn after_a_conflict_the_reception_is_announced_only_after_a_real_wait() {
        let fake = FakeTelegram::start().await;
        fake.fail_next("getUpdates", 409, refusal(409, "Conflict"));
        let mut listening = Listening::start(client(&fake));
        assert_eq!(
            listening.next().await,
            Event::Disconnected(Problem::Conflict)
        );
        assert_eq!(listening.next().await, Event::Connected(bot()));
        let polls = requests(&fake, "getUpdates");
        // Le lot qui a rendu la réception est celui qui a attendu.
        assert_eq!(polls[0]["timeout"], 0);
        assert_eq!(polls[1]["timeout"], CONFLICT_PROBE_SECONDS);
        listening.stop().await;
        fake.stop().await;
    }

    #[tokio::test]
    async fn a_file_too_big_for_a_bot_is_too_large() {
        let fake = FakeTelegram::start().await;
        fake.fail_next("getFile", 400, refusal(400, "Bad Request: file is too big"));
        let file = FileHandle::Telegram {
            file_id: "big".to_owned(),
        };
        assert_eq!(
            client(&fake).download(&file, 1024).await,
            Err(Problem::TooLarge)
        );
        fake.stop().await;
    }

    #[tokio::test]
    async fn the_listening_starts_when_the_server_comes_back() {
        let fake = FakeTelegram::start().await;
        fake.fail_next("getMe", 500, refusal(500, "Internal Server Error"));
        fake.fail_next("getMe", 500, refusal(500, "Internal Server Error"));
        let mut listening = Listening::start(client(&fake));
        assert!(matches!(
            listening.next().await,
            Event::Disconnected(Problem::Refused { status: 500, .. })
        ));
        assert_eq!(listening.next().await, Event::Connected(bot()));
        assert_eq!(requests(&fake, "getMe").len(), 3);
        listening.stop().await;
        fake.stop().await;
    }

    #[tokio::test]
    async fn a_rate_limit_and_a_conflict_wait_longer_than_the_first_retry() {
        let fake = FakeTelegram::start().await;
        let limited =
            json!({ "ok": false, "error_code": 429, "parameters": { "retry_after": 30 } });
        for (status, body, problem, least) in [
            (
                429,
                limited,
                Problem::RateLimited(Duration::from_secs(30)),
                Duration::from_secs(30),
            ),
            (
                409,
                refusal(409, "Conflict"),
                Problem::Conflict,
                CONFLICT_WAIT,
            ),
        ] {
            fake.fail_next("getUpdates", status, body);
            // Mesuré depuis avant l'échec : borne basse sûre, même sur une
            // machine chargée (durées des essais : divisées).
            let start = Instant::now();
            let mut listening = Listening::start(client(&fake));
            assert_eq!(listening.next().await, Event::Disconnected(problem));
            assert_eq!(listening.next().await, Event::Connected(bot()));
            assert!(
                start.elapsed() >= least / WAIT_DIVISOR,
                "{:?}",
                start.elapsed()
            );
            listening.stop().await;
        }
        fake.stop().await;
    }

    #[test]
    fn the_wait_doubles_up_to_a_minute_and_honours_what_telegram_asks() {
        let mut wait = FIRST_WAIT;
        let mut waits = Vec::new();
        for _ in 0..9 {
            waits.push(pause(&Problem::Network(String::new()), wait).as_secs());
            wait = (wait * 2).min(MAX_WAIT);
        }
        assert_eq!(waits, [1, 2, 4, 8, 16, 32, 60, 60, 60]);

        let second = Duration::from_secs(1);
        assert_eq!(
            pause(&Problem::RateLimited(second * 30), second),
            second * 30
        );
        assert_eq!(
            pause(&Problem::RateLimited(second * 3), second * 16),
            second * 16
        );
        assert_eq!(pause(&Problem::Conflict, second), second * 5);
        assert_eq!(pause(&Problem::Conflict, second * 60), second * 60);
        assert_eq!(pause(&Problem::Forbidden, second * 2), second * 2);
    }

    #[tokio::test]
    async fn stopping_ends_the_listening_without_waiting_for_the_long_wait() {
        let fake = FakeTelegram::start().await;
        let mut listening = Listening::start(client(&fake));
        assert_eq!(listening.next().await, Event::Connected(bot()));
        until("the long wait", || requests(&fake, "getUpdates").len() == 2).await;
        // `stop` vérifie que la tâche finit en moins de deux secondes.
        listening.stop().await;

        // Canal des événements fermé : même effet.
        let mut listening = Listening::start(client(&fake));
        assert_eq!(listening.next().await, Event::Connected(bot()));
        until("the long wait", || requests(&fake, "getUpdates").len() == 4).await;
        drop(listening.events);
        timeout(Duration::from_secs(2), listening.task)
            .await
            .expect("ended at once")
            .unwrap();

        // Plus personne pour demander l'arrêt : l'écoute ne reste pas seule.
        let mut listening = Listening::start(client(&fake));
        assert_eq!(listening.next().await, Event::Connected(bot()));
        drop(listening.stop);
        timeout(Duration::from_secs(2), listening.task)
            .await
            .expect("ended at once")
            .unwrap();

        // Arrêt demandé pendant l'attente d'un nouvel essai.
        let mut listening = Listening::start(Client::new(http(), &closed_port().await, TOKEN));
        assert!(matches!(
            listening.next().await,
            Event::Disconnected(Problem::Network(_))
        ));
        listening.stop().await;
        fake.stop().await;
    }

    // --- Messages reçus ---

    fn private(content: Value) -> Value {
        let mut message = json!({
            "message_id": 1,
            "from": { "id": 7, "is_bot": false, "first_name": "Ana", "last_name": "Lopez", "username": "ana" },
            "chat": { "id": 7, "type": "private" },
            "date": 1_760_000_000,
        });
        for (key, value) in content.as_object().unwrap() {
            message[key] = value.clone();
        }
        json!({ "update_id": 1, "message": message })
    }

    #[test]
    fn only_private_messages_of_people_are_delivered() {
        assert!(incoming(&private(json!({ "text": "bonjour" }))).is_some());
        for kind in ["group", "supergroup", "channel"] {
            let chat = json!({ "id": -100, "type": kind });
            assert_eq!(
                incoming(&private(json!({ "text": "bonjour", "chat": chat }))),
                None,
                "{kind}"
            );
        }
        let robot = json!({ "id": 9, "is_bot": true, "first_name": "Robot" });
        assert_eq!(
            incoming(&private(json!({ "text": "bip", "from": robot }))),
            None
        );
        // Auteur inconnu ou incomplet : jamais remis.
        assert_eq!(
            incoming(&private(json!({ "text": "x", "from": null }))),
            None
        );
        assert_eq!(
            incoming(&private(json!({ "text": "x", "from": { "id": 9 } }))),
            None
        );
        assert_eq!(
            incoming(&private(
                json!({ "text": "x", "from": { "is_bot": false } })
            )),
            None
        );
        assert_eq!(incoming(&json!({ "update_id": 1 })), None);
        assert_eq!(
            incoming(&json!({ "update_id": 1, "channel_post": { "text": "x" } })),
            None
        );
    }

    #[test]
    fn a_message_keeps_its_author_its_text_and_its_date() {
        let message = incoming(&private(json!({ "text": "bonjour" }))).unwrap();
        assert_eq!(
            message,
            Incoming {
                id: "1".to_owned(),
                sender: Sender {
                    id: "7".to_owned(),
                    name: "Ana Lopez".to_owned()
                },
                channel: "7".to_owned(),
                text: "bonjour".to_owned(),
                files: Vec::new(),
                sent_at: Some(1_760_000_000),
            }
        );
        let name = |from: Value| {
            incoming(&private(json!({ "from": from })))
                .unwrap()
                .sender
                .name
        };
        assert_eq!(
            name(json!({ "id": 7, "is_bot": false, "first_name": "Ana" })),
            "Ana"
        );
        assert_eq!(
            name(json!({ "id": 7, "is_bot": false, "first_name": "", "username": "ana" })),
            "ana"
        );
        assert_eq!(name(json!({ "id": 7, "is_bot": false })), "");
        // Identifiants au-delà de 32 bits, gardés en texte.
        let big = json!({ "id": 5_000_000_000_i64, "is_bot": false });
        assert_eq!(
            incoming(&private(json!({ "from": big })))
                .unwrap()
                .sender
                .id,
            "5000000000"
        );
        // Ni texte ni fichier (position, contact) : message vide.
        let empty = incoming(&private(
            json!({ "location": { "latitude": 0, "longitude": 0 } }),
        ))
        .unwrap();
        assert_eq!((empty.text.as_str(), empty.files.len()), ("", 0));
    }

    fn files_of(content: Value) -> Vec<(FileKind, String, Option<u64>, String)> {
        incoming(&private(content))
            .unwrap()
            .files
            .into_iter()
            .map(|file| match file.handle {
                FileHandle::Telegram { file_id } => (file.kind, file.name, file.size, file_id),
                FileHandle::Discord { .. } => panic!("a Discord file from Telegram"),
            })
            .collect()
    }

    #[test]
    fn a_photo_is_its_largest_readable_size() {
        let mega = 1024 * 1024;
        let sizes = json!([
            { "file_id": "petite", "width": 90, "height": 60, "file_size": 1000 },
            { "file_id": "moyenne", "width": 800, "height": 600, "file_size": 9 * mega },
            { "file_id": "grande", "width": 4000, "height": 3000, "file_size": 11 * mega },
        ]);
        let message = incoming(&private(json!({ "photo": sizes, "caption": "regarde" }))).unwrap();
        assert_eq!(message.text, "regarde");
        assert_eq!(
            files_of(json!({ "photo": sizes })),
            [(
                FileKind::Image,
                "photo.jpg".to_owned(),
                Some(9 * mega),
                "moyenne".to_owned()
            )]
        );
        // Toutes trop grosses : la plus grande quand même (l'app la refusera).
        let huge = json!([
            { "file_id": "a", "width": 800, "height": 600, "file_size": 11 * mega },
            { "file_id": "b", "width": 4000, "height": 3000, "file_size": 12 * mega },
        ]);
        assert_eq!(files_of(json!({ "photo": huge }))[0].3, "b");
        // Taille inconnue : lisible jusqu'à preuve du contraire ; dans le
        // désordre, la plus grande reste celle des dimensions.
        let unknown = json!([
            { "file_id": "grande", "width": 4000, "height": 3000 },
            { "file_id": "petite", "width": 90, "height": 60 },
        ]);
        assert_eq!(
            files_of(json!({ "photo": unknown })),
            [(
                FileKind::Image,
                "photo.jpg".to_owned(),
                None,
                "grande".to_owned()
            )]
        );
        assert!(files_of(json!({ "photo": [] })).is_empty());
    }

    #[test]
    fn files_are_sorted_by_what_the_app_can_read() {
        let voice =
            json!({ "file_id": "v", "duration": 2, "mime_type": "audio/ogg", "file_size": 4000 });
        assert_eq!(
            files_of(json!({ "voice": voice })),
            [(
                FileKind::Voice,
                "voice.ogg".to_owned(),
                Some(4000),
                "v".to_owned()
            )]
        );

        let document = |mime: &str, name: Option<&str>| {
            let mut document = json!({ "file_id": "d", "mime_type": mime, "file_size": 10 });
            if let Some(name) = name {
                document["file_name"] = name.into();
            }
            let files = files_of(json!({ "document": document }));
            assert_eq!(files.len(), 1);
            (files[0].0, files[0].1.clone())
        };
        assert_eq!(
            document("image/png", Some("capture.png")),
            (FileKind::Image, "capture.png".to_owned())
        );
        assert_eq!(
            document("image/png", None),
            (FileKind::Image, "image.png".to_owned())
        );
        assert_eq!(
            document("image/jpeg", None),
            (FileKind::Image, "image.jpg".to_owned())
        );
        assert_eq!(
            document("image/webp", Some(" ")),
            (FileKind::Image, "image.webp".to_owned())
        );
        assert_eq!(
            document("image/gif", Some("chat.gif")),
            (FileKind::Other, "chat.gif".to_owned())
        );
        assert_eq!(
            document("application/pdf", None),
            (FileKind::Other, "file".to_owned())
        );

        for (field, name) in [
            ("audio", "audio"),
            ("video", "video"),
            ("video_note", "video_note"),
            ("sticker", "sticker"),
        ] {
            let files = files_of(json!({ field: { "file_id": "x", "file_size": 5 } }));
            assert_eq!(
                files,
                [(FileKind::Other, name.to_owned(), Some(5), "x".to_owned())],
                "{field}"
            );
        }
        let audio = json!({ "file_id": "x", "file_name": "chanson.mp3" });
        assert_eq!(files_of(json!({ "audio": audio }))[0].1, "chanson.mp3");
        // Fichier sans identifiant : illisible, laissé de côté.
        assert!(files_of(json!({ "voice": { "duration": 2 } })).is_empty());
    }

    #[tokio::test]
    async fn the_files_of_the_fake_server_arrive_with_their_message() {
        let fake = FakeTelegram::start().await;
        let mut listening = Listening::start(client(&fake));
        assert_eq!(listening.next().await, Event::Connected(bot()));
        fake.message(json!({ "user": "7", "text": "regarde", "image": true }));
        let photo = listening.message().await;
        assert_eq!(photo.text, "regarde");
        assert_eq!(photo.files.len(), 1);
        assert_eq!(
            (photo.files[0].kind, photo.files[0].name.as_str()),
            (FileKind::Image, "photo.jpg")
        );
        fake.message(json!({ "user": "7", "voice": true }));
        let voice = listening.message().await;
        assert_eq!(voice.text, "");
        assert_eq!(
            (voice.files[0].kind, voice.files[0].name.as_str()),
            (FileKind::Voice, "voice.ogg")
        );

        // La poignée reçue sert telle quelle à lire le fichier.
        let client = client(&fake);
        let image = client
            .download(&photo.files[0].handle, 10_000)
            .await
            .unwrap();
        assert_eq!(image.len() as u64, photo.files[0].size.unwrap());
        assert_eq!(&image[..8], b"\x89PNG\r\n\x1a\n");
        listening.stop().await;
        fake.stop().await;
    }

    // --- Réponses ---

    #[tokio::test]
    async fn a_message_is_sent_as_telegram_html() {
        let fake = FakeTelegram::start().await;
        let client = client(&fake);
        let id = client.send("7", "du **gras** & du `code`").await.unwrap();
        assert_eq!(
            requests(&fake, "sendMessage"),
            [json!({
                "chat_id": 7,
                "text": "du <b>gras</b> &amp; du <code>code</code>",
                "parse_mode": "HTML",
                "link_preview_options": { "is_disabled": true },
            })]
        );
        assert_eq!(fake.sent()[0]["id"], id.as_str());
        assert_eq!(fake.sent()[0]["channel"], "7");
        // Un nom de canal reste du texte.
        client.send("@chatons", "bonjour").await.unwrap();
        assert_eq!(requests(&fake, "sendMessage")[1]["chat_id"], "@chatons");
        assert_eq!(fake.sent().len(), 2);
        fake.stop().await;
    }

    #[tokio::test]
    async fn a_refused_formatting_is_sent_again_as_plain_text() {
        let fake = FakeTelegram::start().await;
        let client = client(&fake);
        let unparsed =
            "Bad Request: can't parse entities: Unsupported start tag \"x\" at byte offset 3";
        fake.fail_next("sendMessage", 400, refusal(400, unparsed));
        let id = client.send("7", "du **gras** <ici>").await.unwrap();
        // Une seule nouvelle requête : le Markdown d'origine, sans mise en forme.
        let sent = requests(&fake, "sendMessage");
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0]["parse_mode"], "HTML");
        assert_eq!(
            sent[1],
            json!({ "chat_id": 7, "text": "du **gras** <ici>", "link_preview_options": { "is_disabled": true } })
        );
        assert_eq!(
            fake.sent(),
            [json!({ "kind": "send", "channel": "7", "id": id, "text": "du **gras** <ici>" })]
        );

        // Le repli vaut aussi pour une modification.
        fake.fail_next("editMessageText", 400, refusal(400, unparsed));
        client.edit("7", &id, "du *texte* changé").await.unwrap();
        assert_eq!(fake.sent()[1]["text"], "du *texte* changé");
        assert!(fake.sent()[1].get("parseMode").is_none());

        // Un seul repli : un deuxième refus est rendu.
        fake.fail_next("sendMessage", 400, refusal(400, unparsed));
        fake.fail_next("sendMessage", 400, refusal(400, unparsed));
        let refused = Problem::Refused {
            status: 400,
            message: unparsed.to_owned(),
        };
        assert_eq!(client.send("7", "encore").await, Err(refused));
        assert_eq!(requests(&fake, "sendMessage").len(), 4);
        fake.stop().await;
    }

    #[tokio::test]
    async fn another_refusal_is_not_sent_again() {
        let fake = FakeTelegram::start().await;
        let client = client(&fake);
        fake.fail_next(
            "sendMessage",
            400,
            refusal(400, "Bad Request: chat not found"),
        );
        let refused = Problem::Refused {
            status: 400,
            message: "Bad Request: chat not found".to_owned(),
        };
        assert_eq!(client.send("7", "bonjour").await, Err(refused));
        fake.fail_next(
            "sendMessage",
            403,
            refusal(403, "Forbidden: bot was blocked by the user"),
        );
        assert_eq!(client.send("7", "bonjour").await, Err(Problem::Forbidden));
        assert_eq!(requests(&fake, "sendMessage").len(), 2);
        assert!(fake.sent().is_empty());
        fake.stop().await;
    }

    #[tokio::test]
    async fn the_fake_server_accepts_what_the_formatting_produces() {
        // Le faux serveur refuse un HTML mal formé comme Telegram : un texte
        // en cours d'écriture doit passer sans repli.
        let fake = FakeTelegram::start().await;
        let client = client(&fake);
        let markdown = "# Titre\n\n> citation **forte**\n\n- puce `code` [lien](https://chatonlm.com/?a=1&b=2)\n\n```rust\nlet a = 1 < 2;";
        client.send("7", markdown).await.unwrap();
        let sent = requests(&fake, "sendMessage");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["parse_mode"], "HTML");
        fake.stop().await;
    }

    #[tokio::test]
    async fn a_message_is_edited_and_the_same_text_is_not_an_error() {
        let fake = FakeTelegram::start().await;
        let client = client(&fake);
        let id = client.send("7", "bonjo").await.unwrap();
        assert_eq!(client.edit("7", &id, "bonjour **toi**").await, Ok(()));
        assert_eq!(
            requests(&fake, "editMessageText"),
            [json!({
                "chat_id": 7,
                "message_id": id.parse::<i64>().unwrap(),
                "text": "bonjour <b>toi</b>",
                "parse_mode": "HTML",
                "link_preview_options": { "is_disabled": true },
            })]
        );
        // Le faux serveur répond « message is not modified », comme Telegram.
        assert_eq!(client.edit("7", &id, "bonjour **toi**").await, Ok(()));
        assert_eq!(requests(&fake, "editMessageText").len(), 2);
        assert_eq!(fake.sent().len(), 2);
        // Tout autre refus est rendu.
        let missing = Problem::Refused {
            status: 400,
            message: "Bad Request: message to edit not found".to_owned(),
        };
        assert_eq!(client.edit("7", "999", "bonjour").await, Err(missing));
        fake.stop().await;
    }

    #[tokio::test]
    async fn typing_is_shown_in_the_conversation() {
        let fake = FakeTelegram::start().await;
        assert_eq!(client(&fake).typing("7").await, Ok(()));
        assert_eq!(
            requests(&fake, "sendChatAction"),
            [json!({ "chat_id": 7, "action": "typing" })]
        );
        assert_eq!(fake.sent(), [json!({ "kind": "typing", "channel": "7" })]);
        fake.stop().await;
    }

    // --- Fichiers ---

    fn photo() -> FileHandle {
        FileHandle::Telegram {
            file_id: "fake-photo".to_owned(),
        }
    }

    #[tokio::test]
    async fn a_file_is_read_up_to_the_limit() {
        let fake = FakeTelegram::start().await;
        let client = client(&fake);
        let image = client.download(&photo(), 73).await.unwrap();
        assert_eq!(image.len(), 73);
        assert_eq!(&image[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(
            requests(&fake, "getFile"),
            [json!({ "file_id": "fake-photo" })]
        );
        assert_eq!(
            requests(&fake, "file"),
            [json!({ "path": "photos/file_1.png" })]
        );

        // Taille annoncée au-delà de la limite : refusé sans rien télécharger.
        assert_eq!(client.download(&photo(), 72).await, Err(Problem::TooLarge));
        assert_eq!(requests(&fake, "file").len(), 1);
        fake.stop().await;
    }

    #[tokio::test]
    async fn the_announced_size_of_a_file_is_not_trusted() {
        let fake = FakeTelegram::start().await;
        let client = client(&fake);
        // Telegram annonce 10 octets (ou rien), le fichier en fait 73.
        for size in [json!(10), Value::Null] {
            let info = json!({ "file_id": "fake-photo", "file_path": "photos/file_1.png", "file_size": size });
            fake.fail_next("getFile", 200, json!({ "ok": true, "result": info }));
            assert_eq!(
                client.download(&photo(), 72).await,
                Err(Problem::TooLarge),
                "{size}"
            );
        }
        assert_eq!(requests(&fake, "file").len(), 2);
        fake.stop().await;
    }

    #[tokio::test]
    async fn a_file_that_cannot_be_read_is_a_typed_problem() {
        let fake = FakeTelegram::start().await;
        let client = client(&fake);
        // Poignée de l'autre messagerie : refusée sans requête.
        let discord = FileHandle::Discord {
            url: "https://cdn.discordapp.com/x.png".to_owned(),
        };
        assert!(matches!(
            client.download(&discord, 100).await,
            Err(Problem::Refused { status: 0, .. })
        ));
        assert!(fake.requests().is_empty());

        let unknown = FileHandle::Telegram {
            file_id: "inconnu".to_owned(),
        };
        assert!(matches!(
            client.download(&unknown, 100).await,
            Err(Problem::Refused { status: 400, .. })
        ));

        // Fichier parti du serveur : pas un jeton refusé.
        let gone = json!({ "file_id": "fake-photo", "file_path": "photos/absent.png" });
        fake.fail_next("getFile", 200, json!({ "ok": true, "result": gone }));
        assert!(matches!(
            client.download(&photo(), 100).await,
            Err(Problem::Refused { status: 404, .. })
        ));
        fake.fail_next(
            "getFile",
            200,
            json!({ "ok": true, "result": { "file_id": "fake-photo" } }),
        );
        assert_eq!(
            client.download(&photo(), 100).await,
            Err(Problem::Network("unreadable reply".to_owned()))
        );
        fake.stop().await;
    }

    // --- Images envoyées ---

    /// Image « PNG » de `size` octets (signature, puis des zéros).
    fn png(size: usize) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.resize(size.max(png.len()), 0);
        png
    }

    fn photos(fake: &FakeTelegram) -> Vec<Value> {
        fake.sent()
            .into_iter()
            .filter(|action| action["kind"] == "photo")
            .collect()
    }

    #[tokio::test]
    async fn an_image_is_sent_as_a_multipart_photo() {
        let fake = FakeTelegram::start().await;
        let messenger = crate::Messenger::Telegram(client(&fake));
        let id = messenger
            .send_photo("7", png(300), "image-7.png", Some(" **Chaton** roux "))
            .await
            .unwrap();
        // Nom assaini, sans légende.
        let second = messenger
            .send_photo("7", png(100), "chat \"roux\"", None)
            .await
            .unwrap();
        assert_ne!(id, second);
        assert_eq!(
            photos(&fake),
            [
                json!({
                    "kind": "photo", "channel": "7", "id": id, "filename": "image-7.png",
                    "size": 300, "contentType": "image/png", "upload": "photo",
                    "caption": "<b>Chaton</b> roux", "parseMode": "HTML",
                }),
                json!({
                    "kind": "photo", "channel": "7", "id": second, "filename": "chat__roux_.png",
                    "size": 100, "contentType": "image/png", "upload": "photo",
                }),
            ]
        );
        // Champs du formulaire, tels que lus par le faux serveur.
        assert_eq!(
            requests(&fake, "sendPhoto"),
            [
                json!({
                    "chat_id": "7",
                    "photo": { "filename": "image-7.png", "size": 300, "contentType": "image/png" },
                    "caption": "<b>Chaton</b> roux",
                    "parse_mode": "HTML",
                }),
                json!({
                    "chat_id": "7",
                    "photo": { "filename": "chat__roux_.png", "size": 100, "contentType": "image/png" },
                }),
            ]
        );
        fake.stop().await;
    }

    #[tokio::test]
    async fn an_image_beyond_ten_megabytes_is_sent_as_a_document() {
        let fake = FakeTelegram::start().await;
        let client = client(&fake);
        client
            .send_photo("7", png(PHOTO_MAX), "a.png", None)
            .await
            .unwrap();
        client
            .send_photo("7", png(PHOTO_MAX + 1), "b.png", None)
            .await
            .unwrap();
        let uploads: Vec<_> = photos(&fake)
            .iter()
            .map(|photo| (photo["filename"].clone(), photo["upload"].clone()))
            .collect();
        assert_eq!(
            uploads,
            [
                (json!("a.png"), json!("photo")),
                (json!("b.png"), json!("document")),
            ]
        );
        assert_eq!(requests(&fake, "sendDocument").len(), 1);

        // Au-delà de ce qu'un bot peut envoyer : rien ne part.
        let before = fake.requests().len();
        assert_eq!(
            client
                .send_photo("7", png(DOCUMENT_MAX + 1), "c.png", None)
                .await,
            Err(Problem::TooLarge)
        );
        assert_eq!(fake.requests().len(), before);
        fake.stop().await;
    }

    #[tokio::test]
    async fn a_refused_caption_formatting_is_sent_again_as_plain_text() {
        let fake = FakeTelegram::start().await;
        let client = client(&fake);
        let unparsed = "Bad Request: can't parse entities: Unsupported start tag";
        fake.fail_next("sendPhoto", 400, refusal(400, unparsed));
        client
            .send_photo("7", png(64), "a.png", Some("**Chaton**"))
            .await
            .unwrap();
        let sent = requests(&fake, "sendPhoto");
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1]["caption"], "**Chaton**");
        assert!(sent[1].get("parse_mode").is_none());
        assert_eq!(sent[1]["photo"]["size"], 64);
        assert_eq!(photos(&fake)[0]["caption"], "**Chaton**");

        // Sans légende, le même refus n'est pas réessayé.
        fake.fail_next("sendPhoto", 400, refusal(400, unparsed));
        assert!(matches!(
            client.send_photo("7", png(64), "a.png", None).await,
            Err(Problem::Refused { status: 400, .. })
        ));
        assert_eq!(requests(&fake, "sendPhoto").len(), 3);
        fake.stop().await;
    }

    #[tokio::test]
    async fn refusals_of_an_image_become_typed_problems() {
        let fake = FakeTelegram::start().await;
        let client = client(&fake);
        let limited = json!({
            "ok": false,
            "error_code": 429,
            "description": "Too Many Requests: retry after 3",
            "parameters": { "retry_after": 3 },
        });
        fake.fail_next("sendPhoto", 429, limited);
        assert_eq!(
            client.send_photo("7", png(64), "a.png", None).await,
            Err(Problem::RateLimited(Duration::from_secs(3)))
        );
        // Le frontal refuse un corps trop gros par une page HTML.
        fake.fail_next("sendPhoto", 413, json!("Request Entity Too Large"));
        assert_eq!(
            client.send_photo("7", png(64), "a.png", None).await,
            Err(Problem::TooLarge)
        );
        fake.block("7");
        assert_eq!(
            client.send_photo("7", png(64), "a.png", None).await,
            Err(Problem::Forbidden)
        );
        // Contenu qui n'est pas une image : refus de Telegram.
        assert_eq!(
            client
                .send_photo("8", b"GIF89a".to_vec(), "a.png", None)
                .await,
            Err(Problem::Refused {
                status: 400,
                message: "Bad Request: IMAGE_PROCESS_FAILED".to_owned(),
            })
        );
        assert!(photos(&fake).is_empty());
        fake.stop().await;
    }

    #[tokio::test]
    async fn preparing_a_photo_is_shown_in_the_conversation() {
        let fake = FakeTelegram::start().await;
        client(&fake).preparing_photo("7").await.unwrap();
        assert_eq!(
            requests(&fake, "sendChatAction"),
            [json!({ "chat_id": 7, "action": "upload_photo" })]
        );
        assert_eq!(
            fake.sent(),
            [json!({ "kind": "typing", "channel": "7", "action": "upload_photo" })]
        );
        fake.stop().await;
    }
}
