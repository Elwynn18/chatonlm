//! Bot Discord : messages privés reçus par la passerelle (WebSocket, API
//! v10), réponses par l'API HTTP. Sans l'intention privilégiée « Message
//! Content » : Discord donne le contenu des messages privés sans elle.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use reqwest::Method;
use reqwest::header::{AUTHORIZATION, CONTENT_LENGTH, HeaderValue, RETRY_AFTER, USER_AGENT};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use url::Url;

use crate::{Event, FileHandle, FileKind, Identity, Incoming, IncomingFile, Problem, Sender};

/// Adresse de l'API (l'app la remplace par un faux serveur dans ses essais).
pub const DEFAULT_URL: &str = "https://discord.com/api/v10";

/// Longueur sûre d'un message : la limite est de 2000 caractères.
pub const TEXT_LIMIT: usize = 1900;

/// Agent exigé par Discord pour un bot : adresse et version.
const AGENT: &str = concat!(
    "DiscordBot (https://chatonlm.com, ",
    env!("CARGO_PKG_VERSION"),
    ")"
);

/// Seule intention demandée : `DIRECT_MESSAGES`.
const INTENTS: u64 = 1 << 12;

/// Marque d'un message vocal dans `flags`.
const VOICE_FLAG: u64 = 1 << 13;

/// Seuls hôtes d'où lire une pièce jointe.
const CDN_HOSTS: [&str; 2] = ["cdn.discordapp.com", "media.discordapp.net"];

const IDENTITY_TIMEOUT: Duration = Duration::from_secs(15);
const SEND_TIMEOUT: Duration = Duration::from_secs(30);
const TYPING_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// Taille permise à une réponse de l'API.
const ANSWER_MAX: u64 = 1 << 20;

/// Longueur gardée du message d'un refus.
const MESSAGE_MAX: usize = 300;

/// Plus long intervalle de battements accepté, en millisecondes (Discord :
/// 41 250).
const INTERVAL_MAX: u64 = 86_400_000;

/// Plus longue attente acceptée du retour du quota d'ouvertures de session,
/// en millisecondes (Discord le compte sur 24 h).
const RESET_MAX: u64 = 86_400_000;

/// Reprises manquées de suite avant d'abandonner la session : son adresse
/// peut ne plus répondre, et Discord l'oublie de toute façon.
const RESUME_ATTEMPTS: u32 = 5;

/// Durées de l'écoute ; les essais du module les raccourcissent.
#[derive(Clone, Copy)]
struct Timing {
    /// Attente avant de se reconnecter : doublée à chaque échec…
    retry_first: Duration,
    /// … jusqu'à ce plafond.
    retry_max: Duration,
    /// Attente après un refus que rien ne corrige seul (fermetures 4010 à
    /// 4014).
    refused: Duration,
    /// Attente avant de rouvrir une session invalidée : Discord demande de 1
    /// à 5 fois cette durée, au hasard.
    invalid_session: Duration,
    /// Délai pour joindre la passerelle et y ouvrir la session.
    connect: Duration,
    /// Délai d'une reprise immédiate : réussie avant, la coupure n'est pas
    /// annoncée.
    quiet: Duration,
    /// Durée après laquelle une connexion a tenu : sa coupure se reprend
    /// tout de suite, et l'attente repart de `retry_first`.
    steady: Duration,
}

impl Timing {
    const REAL: Self = Self {
        retry_first: Duration::from_secs(1),
        retry_max: Duration::from_secs(60),
        refused: Duration::from_secs(600),
        invalid_session: Duration::from_secs(1),
        connect: Duration::from_secs(20),
        quiet: Duration::from_secs(5),
        steady: Duration::from_secs(5),
    };
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
    tls: Option<Arc<rustls::ClientConfig>>,
    timing: Timing,
}

/// Sans le jeton.
impl std::fmt::Debug for Client {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Client")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// `http` : client de l'app (proxy, fournisseur TLS). `base` sans « / »
    /// final. `tls` : configuration de la passerelle `wss://` (racines du
    /// système, données par l'app) ; absente, seule une passerelle `ws://`
    /// (faux serveur) est jointe.
    pub fn new(
        http: reqwest::Client,
        base: &str,
        token: &str,
        tls: Option<Arc<rustls::ClientConfig>>,
    ) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            token: token.trim().to_owned(),
            tls,
            timing: Timing::REAL,
        }
    }

    pub async fn identity(&self) -> Result<Identity, Problem> {
        // Le nom vient du compte du bot, l'identifiant de son application
        // (celui du lien d'invitation) : deux requêtes, en même temps.
        let user = self.call(self.api(Method::GET, "/users/@me", IDENTITY_TIMEOUT));
        let application = self.call(self.api(Method::GET, "/applications/@me", IDENTITY_TIMEOUT));
        let (user, application) = tokio::try_join!(user, application)?;
        match (application["id"].as_str(), user["username"].as_str()) {
            (Some(id), Some(name)) => Ok(Identity {
                id: id.to_owned(),
                name: name.to_owned(),
            }),
            _ => Err(unreadable()),
        }
    }

    pub async fn listen(&self, events: mpsc::Sender<Event>, stop: watch::Receiver<bool>) {
        let listener = Listener {
            client: self,
            events,
            stop,
            session: None,
            sequence: None,
            identity: None,
            announced: false,
            missed_resumes: 0,
        };
        listener.run().await;
    }

    pub async fn send(&self, channel: &str, markdown: &str) -> Result<String, Problem> {
        let path = format!("/channels/{}/messages", snowflake(channel)?);
        let request = self
            .api(Method::POST, &path, SEND_TIMEOUT)
            .json(&message_body(markdown));
        let answer = self.call(request).await?;
        answer["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(unreadable)
    }

    pub async fn edit(&self, channel: &str, message: &str, markdown: &str) -> Result<(), Problem> {
        let path = format!(
            "/channels/{}/messages/{}",
            snowflake(channel)?,
            snowflake(message)?
        );
        let request = self
            .api(Method::PATCH, &path, SEND_TIMEOUT)
            .json(&message_body(markdown));
        // Discord accepte un texte identique : rien à rattraper ici.
        self.call(request).await.map(|_| ())
    }

    pub async fn typing(&self, channel: &str) -> Result<(), Problem> {
        let path = format!("/channels/{}/typing", snowflake(channel)?);
        // Longueur dite, même nulle : reqwest ne l'annonce pas pour un POST
        // sans corps, et le frontal de Discord peut alors le refuser (411).
        let request = self
            .api(Method::POST, &path, TYPING_TIMEOUT)
            .header(CONTENT_LENGTH, "0");
        self.call(request).await.map(|_| ())
    }

    pub async fn download(&self, file: &FileHandle, max: u64) -> Result<Vec<u8>, Problem> {
        let FileHandle::Discord { url } = file else {
            return Err(declined("not a Discord file"));
        };
        let url = Url::parse(url)
            .ok()
            .filter(|url| self.serves_files(url))
            .ok_or_else(|| declined("attachment address not allowed"))?;
        // Sans jeton : il ne sort jamais de l'API.
        let response = self
            .http
            .get(url)
            .timeout(DOWNLOAD_TIMEOUT)
            .header(USER_AGENT, AGENT)
            .send()
            .await
            .map_err(|error| self.network(error))?;
        // Si le client de l'app a suivi une redirection, ce qu'elle rend
        // n'est lu que d'un hôte permis.
        if !self.serves_files(response.url()) {
            return Err(declined("attachment redirected elsewhere"));
        }
        // Le CDN n'est pas l'API : son refus ne dit rien du jeton ni du
        // destinataire (jamais `Unauthorized` ni `Forbidden`).
        if !response.status().is_success() {
            return Err(Problem::Refused {
                status: response.status().as_u16(),
                message: String::new(),
            });
        }
        self.read(response, max).await
    }

    /// Vrai pour une adresse d'où lire une pièce jointe : le CDN de Discord
    /// en `https`, ou l'origine de l'API elle-même (faux serveur). Rien
    /// d'autre : un message ne doit pas faire lire à l'app une adresse de la
    /// machine ou du réseau local.
    fn serves_files(&self, url: &Url) -> bool {
        let cdn = url.scheme() == "https"
            && url.port().is_none()
            && url.host_str().is_some_and(|host| CDN_HOSTS.contains(&host));
        let api = Url::parse(&self.base).is_ok_and(|base| base.origin() == url.origin());
        (cdn || api) && url.username().is_empty() && url.password().is_none()
    }

    /// Requête à l'API, avec le jeton.
    fn api(&self, method: Method, path: &str, timeout: Duration) -> reqwest::RequestBuilder {
        let request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .timeout(timeout)
            .header(USER_AGENT, AGENT);
        // Un jeton qu'un en-tête ne peut pas porter part sans autorisation :
        // Discord le refuse (401), comme tout jeton faux.
        match HeaderValue::from_str(&format!("Bot {}", self.token)) {
            Ok(mut value) => {
                value.set_sensitive(true);
                request.header(AUTHORIZATION, value)
            }
            Err(_) => request,
        }
    }

    /// Envoie une requête à l'API et lit sa réponse JSON (`Null` pour une
    /// réponse vide).
    async fn call(&self, request: reqwest::RequestBuilder) -> Result<Value, Problem> {
        let response = request.send().await.map_err(|error| self.network(error))?;
        let status = response.status();
        let retry = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<f64>().ok());
        let body = match self.read(response, ANSWER_MAX).await {
            Err(Problem::TooLarge) => return Err(Problem::Network("answer too large".to_owned())),
            body => body?,
        };
        if !status.is_success() {
            return Err(self.refusal(status.as_u16(), retry, &body));
        }
        if body.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&body).map_err(|_| unreadable())
    }

    /// Lit un corps en flux, `max` octets au plus : la taille annoncée évite
    /// de commencer, mais seuls les octets reçus comptent.
    async fn read(&self, mut response: reqwest::Response, max: u64) -> Result<Vec<u8>, Problem> {
        if response.content_length().is_some_and(|length| length > max) {
            return Err(Problem::TooLarge);
        }
        let mut body = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if (body.len() + chunk.len()) as u64 > max {
                        return Err(Problem::TooLarge);
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(None) => return Ok(body),
                Err(error) => return Err(self.network(error)),
            }
        }
    }

    /// Refus de l'API ; `retry` : en-tête `Retry-After`, en secondes.
    fn refusal(&self, status: u16, retry: Option<f64>, body: &[u8]) -> Problem {
        let body: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        match status {
            401 => Problem::Unauthorized,
            403 => Problem::Forbidden,
            429 => {
                let seconds = body["retry_after"].as_f64().or(retry).unwrap_or(1.0);
                // Valeur démesurée ou illisible : jamais de panique en la
                // convertissant en durée.
                let seconds = if seconds.is_finite() {
                    seconds.clamp(0.0, 86_400.0)
                } else {
                    1.0
                };
                Problem::RateLimited(
                    Duration::try_from_secs_f64(seconds.clamp(0.0, 3600.0))
                        .unwrap_or(Duration::from_secs(1)),
                )
            }
            _ => Problem::Refused {
                status,
                message: self.scrub(body["message"].as_str().unwrap_or_default()),
            },
        }
    }

    /// Échec d'une requête, sans son adresse.
    fn network(&self, error: reqwest::Error) -> Problem {
        let error = error.without_url();
        // Les causes disent ce qui a manqué (connexion refusée, délai…).
        let mut text = error.to_string();
        let mut cause = std::error::Error::source(&error);
        while let Some(error) = cause {
            text = format!("{text}: {error}");
            cause = error.source();
        }
        Problem::Network(self.scrub(&text))
    }

    /// Dernier rempart : aucun texte ne sort d'ici avec le jeton, même si un
    /// serveur le renvoie dans sa réponse.
    fn scrub(&self, text: &str) -> String {
        let text = if self.token.is_empty() {
            text.to_owned()
        } else {
            text.replace(&self.token, "***")
        };
        match text.char_indices().nth(MESSAGE_MAX) {
            Some((end, _)) => text[..end].to_owned(),
            None => text,
        }
    }

    /// Adresse de la passerelle pour une nouvelle session.
    async fn gateway_url(&self) -> Result<String, Problem> {
        let answer = self
            .call(self.api(Method::GET, "/gateway/bot", IDENTITY_TIMEOUT))
            .await?;
        // Quota d'ouvertures de session épuisé : attendre son retour, Discord
        // révoque le jeton d'un bot qui insiste.
        let limit = &answer["session_start_limit"];
        if limit["remaining"].as_u64() == Some(0) {
            // Borné : une valeur folle ne doit pas arrêter le bot pour toujours.
            let wait = limit["reset_after"].as_u64().unwrap_or(60_000);
            return Err(Problem::RateLimited(Duration::from_millis(
                wait.min(RESET_MAX),
            )));
        }
        answer["url"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(unreadable)
    }

    /// Ouvre le WebSocket de la passerelle (JSON, sans compression).
    async fn open(&self, gateway: &str) -> Result<Socket, Problem> {
        let address = format!("{}/?v=10&encoding=json", gateway.trim_end_matches('/'));
        let scheme = Url::parse(&address).map(|url| url.scheme().to_owned());
        let connector = match scheme.as_deref() {
            Ok("wss") => match &self.tls {
                Some(tls) => Connector::Rustls(tls.clone()),
                None => return Err(gateway_problem("no TLS configuration")),
            },
            // En clair face à un faux serveur seulement : le jeton ne part
            // jamais ainsi quand l'API est en `https`.
            Ok("ws") if self.base.starts_with("http://") => Connector::Plain,
            _ => return Err(gateway_problem("address not allowed")),
        };
        tokio_tungstenite::connect_async_tls_with_config(address, None, true, Some(connector))
            .await
            .map(|(socket, _)| socket)
            .map_err(|error| self.gateway_error(&error))
    }

    /// Échec du WebSocket, sans adresse : la sorte d'erreur suffit.
    fn gateway_error(&self, error: &tungstenite::Error) -> Problem {
        use tungstenite::Error;
        let text = match error {
            Error::Io(error) => error.kind().to_string(),
            Error::Http(response) => format!("http {}", response.status().as_u16()),
            Error::Tls(error) => error.to_string(),
            Error::Protocol(error) => error.to_string(),
            Error::ConnectionClosed | Error::AlreadyClosed => "connection closed".to_owned(),
            _ => "connection error".to_owned(),
        };
        gateway_problem(&self.scrub(&text))
    }
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn unreadable() -> Problem {
    Problem::Network("unreadable answer".to_owned())
}

fn gateway_problem(text: &str) -> Problem {
    Problem::Network(format!("gateway: {text}"))
}

/// Refus décidé ici, sans requête.
fn declined(message: &str) -> Problem {
    Problem::Refused {
        status: 0,
        message: message.to_owned(),
    }
}

/// Un identifiant de Discord n'a que des chiffres : rien d'autre n'entre dans
/// le chemin d'une requête.
fn snowflake(id: &str) -> Result<&str, Problem> {
    if !id.is_empty() && id.len() <= 20 && id.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(id)
    } else {
        Err(declined("invalid identifier"))
    }
}

/// Corps d'un message : Discord rend lui-même le Markdown. Aucune mention
/// n'est permise : la réponse d'un modèle ne notifie personne (@everyone).
/// Drapeau `SUPPRESS_EMBEDS` : sans lui, Discord irait lire toute adresse
/// écrite par le modèle pour en montrer un aperçu, sans clic de personne.
const SUPPRESS_EMBEDS: u64 = 1 << 2;

fn message_body(markdown: &str) -> Value {
    json!({
        "content": markdown,
        "allowed_mentions": {"parse": []},
        "flags": SUPPRESS_EMBEDS,
    })
}

/// Message privé d'une personne, ou rien : un message de serveur, d'un bot,
/// ou de service (épinglage, appel) n'est jamais remis à l'app.
fn incoming(message: &Value) -> Option<Incoming> {
    if !message["guild_id"].is_null() || message["author"]["bot"] == true {
        return None;
    }
    // 0 : message ; 19 : réponse à un message.
    if !matches!(message["type"].as_u64(), None | Some(0 | 19)) {
        return None;
    }
    let author = &message["author"];
    let name = author["global_name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .or(author["username"].as_str())
        .unwrap_or_default();
    let voice = message["flags"].as_u64().unwrap_or(0) & VOICE_FLAG != 0;
    let files = message["attachments"]
        .as_array()
        .map(|attachments| {
            attachments
                .iter()
                .filter_map(|attachment| file(attachment, voice))
                .collect()
        })
        .unwrap_or_default();
    Some(Incoming {
        sender: Sender {
            id: author["id"].as_str()?.to_owned(),
            name: name.to_owned(),
        },
        channel: message["channel_id"].as_str()?.to_owned(),
        text: message["content"].as_str().unwrap_or_default().to_owned(),
        files,
        sent_at: message["timestamp"].as_str().and_then(unix_seconds),
    })
}

/// Pièce jointe ; `voice` : le message est un message vocal.
fn file(attachment: &Value, voice: bool) -> Option<IncomingFile> {
    let url = attachment["url"].as_str()?;
    let content_type = attachment["content_type"]
        .as_str()
        .and_then(|value| value.split(';').next())
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let (kind, fallback) = if voice || content_type == "audio/ogg" {
        (FileKind::Voice, "voice.ogg")
    } else {
        match content_type.as_str() {
            "image/png" => (FileKind::Image, "image.png"),
            "image/jpeg" => (FileKind::Image, "image.jpg"),
            "image/webp" => (FileKind::Image, "image.webp"),
            _ => (FileKind::Other, "file"),
        }
    };
    let name = attachment["filename"].as_str().unwrap_or_default();
    // Le nom d'un message vocal dit toujours sa sorte.
    let named = !name.is_empty()
        && (kind != FileKind::Voice || name.to_ascii_lowercase().ends_with(".ogg"));
    Some(IncomingFile {
        kind,
        name: if named { name } else { fallback }.to_owned(),
        size: attachment["size"].as_u64(),
        handle: FileHandle::Discord {
            url: url.to_owned(),
        },
    })
}

/// Secondes Unix d'une date ISO 8601 comme Discord les écrit
/// (`2026-10-13T09:41:12.345000+00:00`) ; `None` pour toute autre forme.
fn unix_seconds(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    let number = |start: usize, length: usize| -> Option<i64> {
        let digits = text.get(start..start + length)?;
        if digits.bytes().all(|byte| byte.is_ascii_digit()) {
            digits.parse().ok()
        } else {
            None
        }
    };
    let shape = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')];
    if !text.is_ascii() || shape.iter().any(|(at, mark)| bytes.get(*at) != Some(mark)) {
        return None;
    }
    let (year, month, day) = (number(0, 4)?, number(5, 2)?, number(8, 2)?);
    let (hour, minute, second) = (number(11, 2)?, number(14, 2)?, number(17, 2)?);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1..=12 => 31,
        _ => return None,
    };
    // Seconde 60 : une seconde intercalaire s'écrit ainsi.
    if !(1..=days).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut zone = &text[19..];
    if let Some(fraction) = zone.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        zone = &fraction[digits..];
    }
    let offset = if zone == "Z" {
        0
    } else {
        let sign = match zone.bytes().next()? {
            b'+' => 1,
            b'-' => -1,
            _ => return None,
        };
        if zone.len() != 6 || zone.as_bytes()[3] != b':' {
            return None;
        }
        let at = text.len() - zone.len();
        let (hours, minutes) = (number(at + 1, 2)?, number(at + 4, 2)?);
        if hours > 23 || minutes > 59 {
            return None;
        }
        sign * (hours * 3600 + minutes * 60)
    };
    // Date civile → jours depuis 1970 (algorithme de Howard Hinnant).
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let (era, year_of_era) = (shifted_year.div_euclid(400), shifted_year.rem_euclid(400));
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let since_epoch = era * 146_097 + day_of_era - 719_468;
    Some(since_epoch * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// Aléa dans [0, 1[ pris dans l'horloge : il ne sert qu'à étaler le premier
/// battement et une attente, pas à protéger quoi que ce soit.
fn jitter() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    f64::from(nanos) / 1e9
}

/// Attend que `stop` passe à vrai, ou que plus personne ne puisse le dire.
async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

async fn push(socket: &mut Socket, payload: Value) -> Result<(), tungstenite::Error> {
    socket.send(Message::text(payload.to_string())).await
}

/// Ferme sans attendre la réponse du serveur. Un code autre que 1000 garde
/// la session ouverte chez Discord, pour la reprendre.
async fn close(socket: &mut Socket, code: u16) {
    let frame = CloseFrame {
        code: CloseCode::from(code),
        reason: "".into(),
    };
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, socket.close(Some(frame))).await;
}

const CLOSE_NORMAL: u16 = 1000;
/// Fermeture qui garde la session.
const CLOSE_RESUMABLE: u16 = 4000;

/// Session ouverte sur la passerelle, de quoi la reprendre.
struct Session {
    id: String,
    /// Adresse de reprise, donnée par `READY`.
    url: String,
}

/// Fin d'une connexion sans que l'arrêt soit demandé.
struct Lost {
    problem: Problem,
    then: Then,
    /// La connexion a reçu les messages assez longtemps pour compter.
    steady: bool,
}

/// Quoi faire après une coupure.
#[derive(Clone, Copy, PartialEq)]
enum Then {
    /// Se reconnecter : reprise si la session est gardée, sinon nouvelle.
    Retry,
    /// Session invalidée : attendre avant d'en ouvrir une autre.
    Fresh,
    /// Refus que rien ne corrige seul : attendre longtemps.
    Hold,
    /// Jeton refusé : fin de l'écoute.
    Never,
}

/// Ce qui réveille une connexion.
enum Wake {
    Stop,
    /// Pas de session à temps.
    Late,
    Beat,
    Frame(Option<Result<Message, tungstenite::Error>>),
}

struct Listener<'a> {
    client: &'a Client,
    events: mpsc::Sender<Event>,
    stop: watch::Receiver<bool>,
    session: Option<Session>,
    /// Dernier numéro reçu : porté par les battements, et par la reprise.
    sequence: Option<u64>,
    /// Le bot, connu depuis le dernier `READY`.
    identity: Option<Identity>,
    /// Coupure déjà dite à l'app, pas encore suivie de `Connected`.
    announced: bool,
    /// Reprises de suite sans `RESUMED`, celle en cours comprise.
    missed_resumes: u32,
}

impl Listener<'_> {
    async fn run(mut self) {
        let timing = self.client.timing;
        let mut retry = timing.retry_first;
        // Reprise immédiate et sans annonce : après la coupure d'une
        // connexion qui a tenu, quand la session peut être reprise (Discord
        // demande régulièrement de se reconnecter : ce n'est pas une panne).
        let mut quiet = false;
        loop {
            let Some(lost) = self.connection(quiet).await else {
                return;
            };
            if lost.then == Then::Never {
                self.emit(Event::Disconnected(Problem::Unauthorized)).await;
                return;
            }
            if lost.steady {
                retry = timing.retry_first;
            }
            quiet = lost.steady && lost.then == Then::Retry && self.session.is_some();
            if quiet {
                continue;
            }
            let limited = match lost.problem {
                Problem::RateLimited(wait) => wait,
                _ => Duration::ZERO,
            };
            if !self.announced {
                self.announced = true;
                if !self.emit(Event::Disconnected(lost.problem)).await {
                    return;
                }
            }
            let asked = match lost.then {
                Then::Hold => timing.refused,
                Then::Fresh => timing.invalid_session.mul_f64(1.0 + 4.0 * jitter()),
                _ => limited,
            };
            // Jamais de boucle serrée : l'attente croît quel que soit le cas.
            let wait = asked.max(retry);
            retry = (retry * 2).min(timing.retry_max);
            if !self.pause(wait).await {
                return;
            }
        }
    }

    /// Remet un événement à l'app ; faux si elle n'écoute plus ou demande
    /// l'arrêt.
    async fn emit(&mut self, event: Event) -> bool {
        tokio::select! {
            sent = self.events.send(event) => sent.is_ok(),
            () = stopped(&mut self.stop) => false,
        }
    }

    /// Attend ; faux si l'arrêt est demandé entre-temps.
    async fn pause(&mut self, wait: Duration) -> bool {
        tokio::select! {
            () = tokio::time::sleep(wait) => true,
            () = stopped(&mut self.stop) => false,
            () = self.events.closed() => false,
        }
    }

    fn forget_session(&mut self) {
        self.session = None;
        self.sequence = None;
    }

    /// Une connexion à la passerelle, de son ouverture à sa fin ; `None`
    /// quand l'arrêt est demandé. `quiet` : reprise immédiate, au délai
    /// court.
    async fn connection(&mut self, quiet: bool) -> Option<Lost> {
        let timing = self.client.timing;
        let lost = |problem, then| {
            Some(Lost {
                problem,
                then,
                steady: false,
            })
        };
        let limit = Instant::now() + if quiet { timing.quiet } else { timing.connect };

        // Une reprise compte comme manquée dès son début, que la passerelle
        // soit injoignable ou qu'elle ne reprenne pas la session : seuls
        // `READY` et `RESUMED` remettent le compte à zéro.
        if self.session.is_some() {
            self.missed_resumes += 1;
            if self.missed_resumes > RESUME_ATTEMPTS {
                self.forget_session();
            }
        }
        let client = self.client;
        let resumed = self.session.as_ref().map(|session| session.url.clone());
        let opening = async {
            let gateway = match resumed {
                Some(url) => url,
                None => client.gateway_url().await?,
            };
            client.open(&gateway).await
        };
        let opened = tokio::select! {
            opened = opening => opened,
            () = tokio::time::sleep_until(limit) => Err(gateway_problem("no connection in time")),
            () = stopped(&mut self.stop) => return None,
            () = self.events.closed() => return None,
        };
        let mut socket = match opened {
            Ok(socket) => socket,
            Err(Problem::Unauthorized) => return lost(Problem::Unauthorized, Then::Never),
            Err(problem) => return lost(problem, Then::Retry),
        };

        // Battements : `None` avant Hello.
        let mut interval: Option<Duration> = None;
        let mut beat_at = limit;
        let mut acknowledged = true;
        // Moment où la session a reçu les messages (`READY` ou `RESUMED`).
        let mut ready_at: Option<Instant> = None;

        loop {
            // Dans l'ordre : ce qui est déjà reçu passe avant l'heure d'un
            // battement, sinon un accusé arrivé à temps mais pas encore lu
            // (machine chargée, sortie de veille) ferait croire la connexion
            // morte.
            let wake = tokio::select! {
                biased;
                () = stopped(&mut self.stop) => Wake::Stop,
                () = self.events.closed() => Wake::Stop,
                frame = socket.next() => Wake::Frame(frame),
                () = tokio::time::sleep_until(limit), if ready_at.is_none() => Wake::Late,
                () = tokio::time::sleep_until(beat_at), if interval.is_some() => Wake::Beat,
            };
            let steady = ready_at.is_some_and(|at| at.elapsed() >= timing.steady);
            let lost = |problem, then| {
                Some(Lost {
                    problem,
                    then,
                    steady,
                })
            };
            let text = match wake {
                Wake::Stop => {
                    close(&mut socket, CLOSE_NORMAL).await;
                    return None;
                }
                Wake::Late => {
                    close(&mut socket, CLOSE_RESUMABLE).await;
                    return lost(gateway_problem("no session in time"), Then::Retry);
                }
                Wake::Beat => {
                    // Aucun accusé depuis le dernier battement : la connexion
                    // est morte, même si rien ne l'a fermée.
                    if !acknowledged {
                        close(&mut socket, CLOSE_RESUMABLE).await;
                        return lost(gateway_problem("heartbeat not acknowledged"), Then::Retry);
                    }
                    acknowledged = false;
                    beat_at += interval.unwrap_or_default();
                    if let Err(error) = push(&mut socket, self.heartbeat()).await {
                        return lost(self.client.gateway_error(&error), Then::Retry);
                    }
                    continue;
                }
                Wake::Frame(Some(Ok(Message::Text(text)))) => text,
                Wake::Frame(Some(Ok(Message::Close(frame)))) => {
                    let (problem, then) = self.closed(frame);
                    return lost(problem, then);
                }
                // Ping et pong : la bibliothèque y répond.
                Wake::Frame(Some(Ok(_))) => continue,
                Wake::Frame(Some(Err(error))) => {
                    return lost(self.client.gateway_error(&error), Then::Retry);
                }
                Wake::Frame(None) => {
                    return lost(gateway_problem("connection closed"), Then::Retry);
                }
            };

            // Une charge illisible n'arrête pas l'écoute.
            let Ok(payload) = serde_json::from_str::<Value>(text.as_str()) else {
                continue;
            };
            if let Some(sequence) = payload["s"].as_u64() {
                self.sequence = Some(sequence);
            }
            let data = &payload["d"];
            let sent = match payload["op"].as_u64() {
                // Hello : battements, puis ouverture ou reprise de la session.
                Some(10) => {
                    let Some(millis) = data["heartbeat_interval"].as_u64().filter(|ms| *ms > 0)
                    else {
                        continue;
                    };
                    // Borné : une valeur folle ne doit pas faire déborder l'horloge.
                    let every = Duration::from_millis(millis.min(INTERVAL_MAX));
                    interval = Some(every);
                    beat_at = Instant::now() + every.mul_f64(jitter());
                    acknowledged = true;
                    push(&mut socket, self.hello_answer()).await
                }
                Some(11) => {
                    acknowledged = true;
                    Ok(())
                }
                // Battement demandé : tout de suite.
                Some(1) => push(&mut socket, self.heartbeat()).await,
                Some(7) => {
                    close(&mut socket, CLOSE_RESUMABLE).await;
                    return lost(gateway_problem("reconnection requested"), Then::Retry);
                }
                Some(9) => {
                    let resumable = *data == true;
                    close(&mut socket, CLOSE_RESUMABLE).await;
                    if resumable {
                        return lost(gateway_problem("session interrupted"), Then::Retry);
                    }
                    self.forget_session();
                    return lost(gateway_problem("session invalidated"), Then::Fresh);
                }
                Some(0) => {
                    let event = match payload["t"].as_str() {
                        Some("READY") => {
                            ready_at = Some(Instant::now());
                            self.missed_resumes = 0;
                            self.ready(data)
                        }
                        Some("RESUMED") => {
                            ready_at = Some(Instant::now());
                            self.missed_resumes = 0;
                            // Sans coupure annoncée, l'app n'a rien à savoir.
                            let identity = self.identity.clone().filter(|_| self.announced);
                            identity.map(Event::Connected)
                        }
                        Some("MESSAGE_CREATE") => incoming(data).map(Event::Message),
                        _ => None,
                    };
                    if let Some(event) = event {
                        if matches!(event, Event::Connected(_)) {
                            self.announced = false;
                        }
                        if !self.emit(event).await {
                            close(&mut socket, CLOSE_NORMAL).await;
                            return None;
                        }
                    }
                    Ok(())
                }
                _ => Ok(()),
            };
            if let Err(error) = sent {
                return lost(self.client.gateway_error(&error), Then::Retry);
            }
        }
    }

    fn heartbeat(&self) -> Value {
        json!({"op": 1, "d": self.sequence})
    }

    /// Réponse à Hello : reprise de la session gardée, sinon ouverture.
    fn hello_answer(&mut self) -> Value {
        let token = &self.client.token;
        if let Some(session) = &self.session {
            return json!({
                "op": 6,
                "d": {"token": token, "session_id": session.id, "seq": self.sequence},
            });
        }
        self.sequence = None;
        json!({
            "op": 2,
            "d": {
                "token": token,
                "intents": INTENTS,
                "properties": {
                    "os": std::env::consts::OS,
                    "browser": "chatonlm",
                    "device": "chatonlm",
                },
            },
        })
    }

    /// `READY` : garde la session et annonce le bot.
    fn ready(&mut self, data: &Value) -> Option<Event> {
        self.session = match (
            data["session_id"].as_str(),
            data["resume_gateway_url"].as_str(),
        ) {
            (Some(id), Some(url)) => Some(Session {
                id: id.to_owned(),
                url: url.to_owned(),
            }),
            _ => None,
        };
        let id = data["application"]["id"]
            .as_str()
            .or(data["user"]["id"].as_str())?;
        let identity = Identity {
            id: id.to_owned(),
            name: data["user"]["username"].as_str()?.to_owned(),
        };
        self.identity = Some(identity.clone());
        Some(Event::Connected(identity))
    }

    /// Fermeture par Discord : ce que son code permet.
    fn closed(&mut self, frame: Option<CloseFrame>) -> (Problem, Then) {
        let Some(frame) = frame else {
            return (gateway_problem("closed"), Then::Retry);
        };
        let code = u16::from(frame.code);
        match code {
            4004 => (Problem::Unauthorized, Then::Never),
            // Découpage, version ou intentions refusés : réessayer aussitôt
            // n'y changerait rien.
            4010..=4014 => {
                self.forget_session();
                let message = self.client.scrub(&frame.reason);
                (
                    Problem::Refused {
                        status: code,
                        message,
                    },
                    Then::Hold,
                )
            }
            _ => {
                // Fermeture normale, session inconnue, numéro faux ou session
                // expirée : elle ne se reprend pas.
                if matches!(code, 1000 | 1001 | 4003 | 4007 | 4009) {
                    self.forget_session();
                }
                (gateway_problem(&format!("closed ({code})")), Then::Retry)
            }
        }
    }
}

#[cfg(all(test, feature = "fake"))]
mod tests {
    use std::convert::Infallible;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::body::{Body, Bytes};
    use axum::extract::ws::WebSocketUpgrade;
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::routing::get;
    use axum::{Json, Router};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc::error::TryRecvError;
    use tokio::task::JoinHandle;
    use tokio::time::{sleep, timeout};

    use super::*;
    use crate::fake::discord::{FakeDiscord, TOKEN};

    /// Attentes des essais : courtes, et aucune connexion ne « tient » (la
    /// reprise silencieuse a ses propres essais).
    const FAST: Timing = Timing {
        retry_first: Duration::from_millis(20),
        retry_max: Duration::from_millis(80),
        refused: Duration::from_secs(1),
        invalid_session: Duration::from_millis(10),
        connect: Duration::from_secs(5),
        quiet: Duration::from_secs(5),
        steady: Duration::from_secs(3600),
    };

    fn http() -> reqwest::Client {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client HTTP")
    }

    fn client_at(base: &str, token: &str) -> Client {
        let mut client = Client::new(http(), base, token, None);
        client.timing = FAST;
        client
    }

    fn client(fake: &FakeDiscord) -> Client {
        client_at(&fake.url(), TOKEN)
    }

    fn bot() -> Identity {
        Identity {
            id: "424242".to_owned(),
            name: "chatonlm-test".to_owned(),
        }
    }

    /// Serveur d'un essai, pour ce que le faux Discord ne fait pas ; rend son
    /// origine (`http://127.0.0.1:<port>`).
    async fn serve(router: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        origin
    }

    /// Serveur qui compte ses requêtes et répond « secret » à toutes.
    async fn counting() -> (String, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let router = Router::new().fallback(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { "secret" }
        });
        (serve(router).await, hits)
    }

    /// Adresse d'une API où rien n'écoute.
    async fn closed_api() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}/api/v10", listener.local_addr().unwrap())
    }

    struct Listening {
        events: mpsc::Receiver<Event>,
        stop: watch::Sender<bool>,
        task: JoinHandle<()>,
    }

    fn listen(client: &Client) -> Listening {
        let (sender, events) = mpsc::channel(16);
        let (stop, stopped) = watch::channel(false);
        let client = client.clone();
        let task = tokio::spawn(async move { client.listen(sender, stopped).await });
        Listening { events, stop, task }
    }

    impl Listening {
        async fn next(&mut self) -> Event {
            timeout(Duration::from_secs(10), self.events.recv())
                .await
                .expect("un événement à temps")
                .expect("écoute en cours")
        }

        async fn connected(&mut self) {
            assert_eq!(self.next().await, Event::Connected(bot()));
        }

        async fn message(&mut self) -> Incoming {
            match self.next().await {
                Event::Message(message) => message,
                other => panic!("message attendu, reçu {other:?}"),
            }
        }

        /// Rien d'autre n'arrive.
        async fn silent(&mut self) {
            sleep(Duration::from_millis(100)).await;
            assert_eq!(self.events.try_recv(), Err(TryRecvError::Empty));
        }

        async fn ended(self) {
            timeout(Duration::from_secs(5), self.task)
                .await
                .expect("fin de l'écoute")
                .expect("écoute sans panique");
        }
    }

    async fn until(what: &str, mut check: impl FnMut() -> bool) {
        for _ in 0..1000 {
            if check() {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
        panic!("jamais arrivé : {what}");
    }

    /// Charges d'un opcode reçues par la passerelle du faux serveur.
    fn received(fake: &FakeDiscord, op: u64) -> Vec<Value> {
        fake.gateway_log()
            .into_iter()
            .filter(|payload| payload["op"] == op)
            .collect()
    }

    fn unix_now() -> i64 {
        let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        i64::try_from(since.as_secs()).unwrap()
    }

    fn discord_file(url: &str) -> FileHandle {
        FileHandle::Discord {
            url: url.to_owned(),
        }
    }

    #[tokio::test]
    async fn the_identity_is_the_name_of_the_bot_and_the_id_of_its_application() {
        let router = Router::new()
            .route(
                "/users/@me",
                get(|| async { Json(json!({"id": "1", "username": "kitten"})) }),
            )
            .route(
                "/applications/@me",
                get(|| async { Json(json!({"id": "2"})) }),
            );
        let base = serve(router).await;
        assert_eq!(
            client_at(&base, TOKEN).identity().await,
            Ok(Identity {
                id: "2".to_owned(),
                name: "kitten".to_owned(),
            })
        );

        let fake = FakeDiscord::start().await;
        assert_eq!(client(&fake).identity().await, Ok(bot()));
    }

    #[tokio::test]
    async fn an_unknown_token_is_unauthorized() {
        let fake = FakeDiscord::start().await;
        let client = client_at(&fake.url(), "wrong.token");
        assert_eq!(client.identity().await, Err(Problem::Unauthorized));
        assert_eq!(client.send("942", "hi").await, Err(Problem::Unauthorized));
        assert_eq!(client.typing("942").await, Err(Problem::Unauthorized));
        assert!(fake.sent().is_empty());
    }

    #[tokio::test]
    async fn the_token_never_leaves_in_an_error_or_a_debug_text() {
        const SECRET: &str = "MTIz.secret-token.abc";
        // Le pire des serveurs : il renvoie dans son refus l'en-tête reçu.
        let echo = |headers: HeaderMap| async move {
            let sent = headers[header::AUTHORIZATION].to_str().unwrap().to_owned();
            (
                StatusCode::IM_A_TEAPOT,
                Json(json!({"message": format!("bad header {sent}")})),
            )
        };
        let base = serve(Router::new().fallback(echo)).await;
        let client = client_at(&base, SECRET);
        assert_eq!(
            client.send("942", "hi").await,
            Err(Problem::Refused {
                status: 418,
                message: "bad header Bot ***".to_owned(),
            })
        );

        let closed = client_at(&closed_api().await, SECRET);
        let mut listening = listen(&client);
        let texts = [
            format!("{client:?}"),
            format!("{:?}", client.identity().await),
            format!("{:?}", client.send("942", "hi").await),
            format!("{:?}", client.edit("942", "7", "hi").await),
            format!("{:?}", client.typing("942").await),
            format!("{:?}", closed.identity().await),
            format!("{:?}", closed.send("942", "hi").await),
            format!("{:?}", listening.next().await),
        ];
        for text in texts {
            assert!(!text.contains("secret-token"), "jeton divulgué : {text}");
        }
    }

    #[tokio::test]
    async fn a_message_is_sent_then_edited_without_any_mention_allowed() {
        let fake = FakeDiscord::start().await;
        let client = client(&fake);
        let id = client.send("942", "Hello @everyone").await.unwrap();
        client.edit("942", &id, "Hello **you**").await.unwrap();
        // Le même texte une seconde fois : toujours un succès.
        client.edit("942", &id, "Hello **you**").await.unwrap();
        client.typing("942").await.unwrap();

        let nobody = json!({"parse": []});
        // `SUPPRESS_EMBEDS` : Discord ne va lire aucune adresse du texte.
        let flags = 1 << 2;
        let edit = json!({
            "kind": "edit",
            "channel": "942",
            "id": id,
            "text": "Hello **you**",
            "allowedMentions": nobody,
            "flags": flags,
        });
        assert_eq!(
            fake.sent(),
            vec![
                json!({
                    "kind": "send",
                    "channel": "942",
                    "id": id,
                    "text": "Hello @everyone",
                    "allowedMentions": nobody,
                    "flags": flags,
                }),
                edit.clone(),
                edit,
                json!({"kind": "typing", "channel": "942"}),
            ]
        );
    }

    #[tokio::test]
    async fn refusals_of_the_api_become_problems() {
        let fake = FakeDiscord::start().await;
        let client = client(&fake);
        let refused = |status, message: &str| {
            Err(Problem::Refused {
                status,
                message: message.to_owned(),
            })
        };

        let blocked = json!({"message": "Cannot send messages to this user", "code": 50007});
        fake.fail_next("send", 403, blocked);
        assert_eq!(client.send("942", "hi").await, Err(Problem::Forbidden));

        let limited = json!({"message": "You are being rate limited.", "retry_after": 1.5});
        fake.fail_next("send", 429, limited);
        assert_eq!(
            client.send("942", "hi").await,
            Err(Problem::RateLimited(Duration::from_millis(1500)))
        );

        fake.fail_next("edit", 500, json!({"message": "boom"}));
        assert_eq!(client.edit("942", "7", "hi").await, refused(500, "boom"));

        fake.fail_next("typing", 401, json!({"message": "401: Unauthorized"}));
        assert_eq!(client.typing("942").await, Err(Problem::Unauthorized));

        assert_eq!(
            client.send("942", &"a".repeat(2001)).await.map(|_| ()),
            refused(400, "Invalid Form Body")
        );
        assert_eq!(
            client.edit("942", "7", "hi").await,
            refused(404, "Unknown Message")
        );
        assert!(fake.sent().is_empty());
    }

    #[tokio::test]
    async fn a_rate_limit_without_a_body_waits_as_its_header_says() {
        let slow = || async {
            (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, "7")],
                "slow down",
            )
        };
        let base = serve(Router::new().fallback(slow)).await;
        assert_eq!(
            client_at(&base, TOKEN).typing("942").await,
            Err(Problem::RateLimited(Duration::from_secs(7)))
        );
    }

    #[tokio::test]
    async fn an_unreadable_answer_and_a_closed_port_are_network_problems() {
        let base = serve(Router::new().fallback(|| async { "<html>" })).await;
        assert_eq!(
            client_at(&base, TOKEN).send("942", "hi").await,
            Err(Problem::Network("unreadable answer".to_owned()))
        );

        let base = closed_api().await;
        let Err(Problem::Network(text)) = client_at(&base, TOKEN).identity().await else {
            panic!("un port fermé est une panne de réseau");
        };
        assert!(!text.is_empty());
        assert!(!text.contains("127.0.0.1"), "adresse dans : {text}");
    }

    #[tokio::test]
    async fn an_identifier_that_is_not_a_number_never_reaches_the_api() {
        let (origin, hits) = counting().await;
        let client = client_at(&origin, TOKEN);
        let invalid = Err(declined("invalid identifier"));
        for channel in ["", "942/../../users/@me", "9 42", "942?wait=1", "９４２"] {
            assert_eq!(client.send(channel, "hi").await.map(|_| ()), invalid);
            assert_eq!(client.typing(channel).await, invalid);
            assert_eq!(client.edit(channel, "7", "hi").await, invalid);
        }
        assert_eq!(client.edit("942", "7/../8", "hi").await, invalid);
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_attachment_is_read_without_the_token_up_to_the_limit() {
        let fake = FakeDiscord::start().await;
        let client = client(&fake);
        let mut listening = listen(&client);
        listening.connected().await;
        fake.message(json!({"user": "42", "text": "look", "image": true}));
        let message = listening.message().await;
        let [image] = message.files.as_slice() else {
            panic!("une image attendue : {message:?}");
        };
        assert_eq!(image.kind, FileKind::Image);
        assert_eq!(image.name, "image.png");
        assert_eq!(image.size, Some(75));

        // Le faux serveur refuse un fichier demandé avec le jeton du bot :
        // la lecture ne réussit que sans lui.
        let bytes = client.download(&image.handle, 75).await.unwrap();
        assert_eq!(bytes.len(), 75);
        assert!(bytes.starts_with(b"\x89PNG"));
        assert_eq!(
            client.download(&image.handle, 74).await,
            Err(Problem::TooLarge)
        );

        let FileHandle::Discord { url } = &image.handle else {
            panic!("poignée de Discord attendue");
        };
        let missing = discord_file(&url.replace("image.png", "nothing.bin"));
        assert_eq!(
            client.download(&missing, 75).await,
            Err(Problem::Refused {
                status: 404,
                message: String::new(),
            })
        );
    }

    #[tokio::test]
    async fn a_download_counts_the_bytes_received_not_the_announced_size() {
        // 1 Mio en flux, sans longueur annoncée.
        let big = || async {
            let chunks = (0..64).map(|_| Ok::<_, Infallible>(Bytes::from(vec![7u8; 16_384])));
            Body::from_stream(futures_util::stream::iter(chunks))
        };
        let origin = serve(Router::new().route("/attachments/big", get(big))).await;
        let client = client_at(&format!("{origin}/api/v10"), TOKEN);
        let file = discord_file(&format!("{origin}/attachments/big"));
        assert_eq!(
            client.download(&file, 100_000).await,
            Err(Problem::TooLarge)
        );
        assert_eq!(
            client.download(&file, 1 << 20).await.map(|b| b.len()),
            Ok(1 << 20)
        );
    }

    #[tokio::test]
    async fn an_attachment_elsewhere_is_refused_without_a_request() {
        let (elsewhere, hits) = counting().await;
        let fake = FakeDiscord::start().await;
        let client = client(&fake);
        let closed = closed_api().await;
        let not_allowed = Err(declined("attachment address not allowed"));
        for url in [
            format!("{elsewhere}/attachments/1/2/image.png"),
            elsewhere.replace("127.0.0.1", "localhost"),
            closed,
            fake.url().replace("http://", "https://"),
            "file:///etc/hostname".to_owned(),
            "not an address".to_owned(),
        ] {
            assert_eq!(
                client.download(&discord_file(&url), 1000).await,
                not_allowed
            );
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0);

        let telegram = FileHandle::Telegram {
            file_id: "abc".to_owned(),
        };
        assert_eq!(
            client.download(&telegram, 1000).await,
            Err(declined("not a Discord file"))
        );
    }

    #[test]
    fn only_the_cdn_of_discord_and_the_api_itself_serve_files() {
        let client = Client::new(http(), DEFAULT_URL, TOKEN, None);
        let serves = |url: &str| client.serves_files(&Url::parse(url).unwrap());
        assert!(serves(
            "https://cdn.discordapp.com/attachments/1/2/voice-message.ogg?ex=1&is=2&hm=3"
        ));
        assert!(serves("https://media.discordapp.net/attachments/1/2/a.png"));
        assert!(serves("https://discord.com/attachments/1/2/a.png"));
        for url in [
            "http://cdn.discordapp.com/attachments/1/2/a.png",
            "https://cdn.discordapp.com:8443/attachments/1/2/a.png",
            "https://cdn.discordapp.com.evil.test/a.png",
            "https://evil.test/cdn.discordapp.com/a.png",
            "https://cdn.discordapp.com@127.0.0.1/a.png",
            "https://user:pass@cdn.discordapp.com/a.png",
            "https://127.0.0.1/a.png",
            "https://localhost:11434/api/tags",
            "http://discord.com/attachments/1/2/a.png",
            "wss://cdn.discordapp.com/a.png",
        ] {
            assert!(!serves(url), "{url}");
        }
    }

    #[tokio::test]
    async fn a_redirection_out_of_the_allowed_hosts_is_not_read() {
        let (elsewhere, hits) = counting().await;
        let moved = move || {
            let target = format!("{elsewhere}/secret");
            async move { (StatusCode::FOUND, [(header::LOCATION, target)]) }
        };
        let origin = serve(Router::new().route("/attachments/moved", get(moved))).await;
        let base = format!("{origin}/api/v10");
        let file = discord_file(&format!("{origin}/attachments/moved"));

        // Client qui suit les redirections (défaut de reqwest) : ce qu'elle
        // rend n'est pas lu.
        assert_eq!(
            client_at(&base, TOKEN).download(&file, 1000).await,
            Err(declined("attachment redirected elsewhere"))
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        // Client qui ne les suit pas : rien n'est demandé ailleurs.
        let strict = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        assert_eq!(
            Client::new(strict, &base, TOKEN, None)
                .download(&file, 1000)
                .await,
            Err(Problem::Refused {
                status: 302,
                message: String::new(),
            })
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn listening_announces_the_bot_then_hands_its_private_messages() {
        let fake = FakeDiscord::start().await;
        let mut listening = listen(&client(&fake));
        listening.connected().await;

        let now = unix_now();
        fake.message(json!({"user": "42", "name": "Ana", "text": "Bonjour"}));
        let message = listening.message().await;
        assert_eq!(
            message.sender,
            Sender {
                id: "42".to_owned(),
                name: "Ana".to_owned(),
            }
        );
        assert_eq!(message.channel, "942");
        assert_eq!(message.text, "Bonjour");
        assert!(message.files.is_empty());
        let sent_at = message.sent_at.expect("date lue");
        assert!((now - 1..=now + 5).contains(&sent_at), "{sent_at} / {now}");

        // Sans nom affiché : le nom d'utilisateur. Message ancien : sa date.
        fake.message(json!({"user": 42, "text": "old", "age": 600}));
        let old = listening.message().await;
        assert_eq!(old.sender.name, "user42");
        let sent_at = old.sent_at.expect("date lue");
        assert!(
            (now - 601..=now - 595).contains(&sent_at),
            "{sent_at} / {now}"
        );

        let identify = received(&fake, 2);
        assert_eq!(identify.len(), 1);
        assert_eq!(
            identify[0]["d"],
            json!({
                "token": "***",
                "intents": 4096,
                "properties": {
                    "os": std::env::consts::OS,
                    "browser": "chatonlm",
                    "device": "chatonlm",
                },
            })
        );
    }

    #[tokio::test]
    async fn messages_of_a_server_or_of_a_bot_are_never_handed() {
        let fake = FakeDiscord::start().await;
        let mut listening = listen(&client(&fake));
        listening.connected().await;
        fake.message(json!({"user": "42", "text": "in a server", "private": false}));
        fake.message(json!({"user": "43", "text": "from a bot", "bot": true}));
        fake.message(json!({"user": "42", "text": "for the bot"}));
        assert_eq!(listening.message().await.text, "for the bot");
        listening.silent().await;
    }

    #[tokio::test]
    async fn a_voice_message_comes_as_a_voice_file() {
        let fake = FakeDiscord::start().await;
        let client = client(&fake);
        let mut listening = listen(&client);
        listening.connected().await;
        fake.message(json!({"user": "42", "voice": true}));
        let message = listening.message().await;
        assert_eq!(message.text, "");
        let [voice] = message.files.as_slice() else {
            panic!("un message vocal attendu : {message:?}");
        };
        assert_eq!(voice.kind, FileKind::Voice);
        assert_eq!(voice.name, "voice-message.ogg");
        let FileHandle::Discord { url } = &voice.handle else {
            panic!("poignée de Discord attendue");
        };
        assert!(url.starts_with(&fake.url().replace("/api/v10", "/attachments/942/")));

        match client.download(&voice.handle, 1 << 20).await {
            Ok(bytes) => assert!(bytes.starts_with(b"OggS")),
            // Fichier d'essai du crate absent : un refus, sans panique.
            Err(problem) => assert_eq!(
                problem,
                Problem::Refused {
                    status: 404,
                    message: String::new(),
                }
            ),
        }
    }

    #[test]
    fn a_message_names_its_sender_and_sorts_its_files() {
        let attachment = |name: Value, content_type: Value| {
            json!({
                "id": "5",
                "filename": name,
                "content_type": content_type,
                "size": 12,
                "url": "https://cdn.discordapp.com/attachments/1/5/x",
            })
        };
        let message = |extra: Value| {
            let mut message = json!({
                "id": "3",
                "channel_id": "1",
                "author": {"id": "42", "username": "ana", "global_name": null},
                "content": "hi",
                "timestamp": "2026-10-13T09:41:12.345000+00:00",
                "type": 0,
            });
            for (key, value) in extra.as_object().unwrap() {
                message[key] = value.clone();
            }
            message
        };
        let sorted = |message: &Value| -> Vec<(FileKind, String)> {
            let files = incoming(message).unwrap().files;
            files
                .into_iter()
                .map(|file| (file.kind, file.name))
                .collect()
        };

        let plain = incoming(&message(json!({}))).unwrap();
        assert_eq!(plain.sender.name, "ana");
        assert_eq!(plain.sent_at, Some(1_791_884_472));
        assert_eq!(plain.files, vec![]);
        let named = json!({"author": {"id": "42", "username": "ana", "global_name": "Ana B."}});
        assert_eq!(incoming(&message(named)).unwrap().sender.name, "Ana B.");
        let undated = incoming(&message(json!({"timestamp": "yesterday"}))).unwrap();
        assert_eq!(undated.sent_at, None);

        let files = message(json!({"attachments": [
            attachment(json!("a.png"), json!("image/png")),
            attachment(json!("b.JPG"), json!("image/jpeg")),
            attachment(json!(""), json!("image/webp")),
            attachment(json!("c.gif"), json!("image/gif")),
            attachment(json!("d.pdf"), json!(null)),
            attachment(json!("e.oga"), json!("audio/ogg; codecs=opus")),
            {"id": "6", "filename": "no address"},
        ]}));
        assert_eq!(
            sorted(&files),
            vec![
                (FileKind::Image, "a.png".to_owned()),
                (FileKind::Image, "b.JPG".to_owned()),
                (FileKind::Image, "image.webp".to_owned()),
                (FileKind::Other, "c.gif".to_owned()),
                (FileKind::Other, "d.pdf".to_owned()),
                (FileKind::Voice, "voice.ogg".to_owned()),
            ]
        );
        // Message vocal : sa marque suffit, quel que soit le type annoncé.
        let voice = message(json!({
            "flags": 1 << 13,
            "attachments": [attachment(json!("voice-message.ogg"), json!(null))],
        }));
        assert_eq!(
            sorted(&voice),
            vec![(FileKind::Voice, "voice-message.ogg".to_owned())]
        );

        // Réponse à un message : remise. Message de service, de serveur, de
        // bot, ou sans auteur lisible : jamais.
        assert!(incoming(&message(json!({"type": 19}))).is_some());
        for ignored in [
            json!({"type": 6}),
            json!({"guild_id": "9"}),
            json!({"author": {"id": "42", "username": "ana", "bot": true}}),
            json!({"author": {"username": "ana"}}),
            json!({"channel_id": null}),
        ] {
            assert_eq!(incoming(&message(ignored.clone())), None, "{ignored}");
        }
    }

    #[test]
    fn dates_of_discord_become_unix_seconds() {
        for (text, seconds) in [
            ("1970-01-01T00:00:00Z", 0),
            ("1969-12-31T23:59:59+00:00", -1),
            ("2026-10-13T09:41:12.345000+00:00", 1_791_884_472),
            ("2026-10-13T09:41:12+00:00", 1_791_884_472),
            ("2024-02-29T23:59:59+02:00", 1_709_243_999),
            ("2024-02-29T16:59:59.5-05:00", 1_709_243_999),
        ] {
            assert_eq!(unix_seconds(text), Some(seconds), "{text}");
        }
        for text in [
            "",
            "yesterday",
            "2026-10-13",
            "2026-10-13T09:41:12",
            "2026-10-13 09:41:12Z",
            "2026-13-01T00:00:00Z",
            "2026-02-29T00:00:00Z",
            "2026-10-13T24:00:00Z",
            "2026-10-13T09:41:12.Z",
            "2026-10-13T09:41:12+0000",
            "2026-10-13T09:41:12+00:00 ",
            "2026-10-13T09:41:1é+00:00",
            "+026-10-13T09:41:12Z",
        ] {
            assert_eq!(unix_seconds(text), None, "{text}");
        }
    }

    #[tokio::test]
    async fn heartbeats_follow_the_interval_with_the_last_number_received() {
        let fake = FakeDiscord::start().await;
        fake.set_heartbeat_interval(Duration::from_millis(30));
        let mut listening = listen(&client(&fake));
        listening.connected().await;
        until("trois battements", || received(&fake, 1).len() >= 3).await;

        // READY porte le numéro 1, le message le 2.
        fake.message(json!({"user": "42", "text": "hi"}));
        listening.message().await;
        let before = received(&fake, 1).len();
        until("deux battements de plus", || {
            received(&fake, 1).len() >= before + 2
        })
        .await;
        assert_eq!(received(&fake, 1).last().unwrap()["d"], 2);
        // Tous accusés : la connexion n'a pas été reprise.
        assert!(received(&fake, 6).is_empty());
        listening.silent().await;
    }

    #[tokio::test]
    async fn a_heartbeat_asked_by_the_gateway_is_sent_at_once() {
        let fake = FakeDiscord::start().await;
        fake.set_heartbeat_interval(Duration::from_secs(86_400));
        let mut listening = listen(&client(&fake));
        listening.connected().await;
        assert!(received(&fake, 1).is_empty());
        fake.gateway_send(json!({"op": 1, "d": null}));
        until("le battement demandé", || received(&fake, 1).len() == 1).await;
        assert_eq!(received(&fake, 1)[0]["d"], 1);
    }

    #[tokio::test]
    async fn a_gateway_that_stops_acknowledging_is_dropped_and_resumed() {
        let fake = FakeDiscord::start().await;
        fake.set_heartbeat_interval(Duration::from_millis(30));
        let mut listening = listen(&client(&fake));
        listening.connected().await;

        fake.silence_acks(true);
        assert_eq!(
            listening.next().await,
            Event::Disconnected(gateway_problem("heartbeat not acknowledged"))
        );
        fake.silence_acks(false);
        listening.connected().await;
        assert_eq!(received(&fake, 2).len(), 1);
        assert_eq!(received(&fake, 6).len(), 1);
    }

    #[tokio::test]
    async fn a_requested_reconnection_is_resumed_without_a_word() {
        for request in [json!({"op": 7, "d": null}), json!({"op": 9, "d": true})] {
            let fake = FakeDiscord::start().await;
            let mut client = client(&fake);
            client.timing.steady = Duration::ZERO;
            let mut listening = listen(&client);
            listening.connected().await;

            fake.gateway_send(request);
            until("la reprise", || received(&fake, 6).len() == 1).await;
            // Ni coupure ni connexion annoncées : le message vient d'abord.
            fake.message(json!({"user": "42", "text": "after"}));
            assert_eq!(listening.message().await.text, "after");

            let resume = &received(&fake, 6)[0]["d"];
            assert_eq!(resume["token"], "***");
            assert_eq!(resume["seq"], 1);
            assert!(resume["session_id"].is_string());
            assert_eq!(received(&fake, 2).len(), 1);
            listening.silent().await;
        }
    }

    #[tokio::test]
    async fn a_cut_gateway_is_announced_once_then_resumed_without_losing_a_message() {
        for code in [None, Some(4000)] {
            let fake = FakeDiscord::start().await;
            let mut listening = listen(&client(&fake));
            listening.connected().await;

            fake.drop_gateway(code);
            fake.message(json!({"user": "42", "text": "during the cut"}));
            assert!(matches!(
                listening.next().await,
                Event::Disconnected(Problem::Network(_))
            ));
            // Rejoué par la reprise, avant qu'elle soit dite finie.
            assert_eq!(listening.message().await.text, "during the cut");
            listening.connected().await;
            listening.silent().await;

            assert_eq!(received(&fake, 2).len(), 1);
            let resumes = received(&fake, 6);
            assert_eq!(resumes.len(), 1);
            assert_eq!(resumes[0]["d"]["seq"], 1);
        }
    }

    #[tokio::test]
    async fn a_normal_closure_opens_a_new_session() {
        let fake = FakeDiscord::start().await;
        let mut listening = listen(&client(&fake));
        listening.connected().await;
        fake.drop_gateway(Some(1000));
        assert_eq!(
            listening.next().await,
            Event::Disconnected(gateway_problem("closed (1000)"))
        );
        listening.connected().await;
        assert_eq!(received(&fake, 2).len(), 2);
        assert!(received(&fake, 6).is_empty());
    }

    #[tokio::test]
    async fn an_invalidated_session_is_reopened() {
        let fake = FakeDiscord::start().await;
        let mut listening = listen(&client(&fake));
        listening.connected().await;
        fake.gateway_send(json!({"op": 9, "d": false}));
        assert_eq!(
            listening.next().await,
            Event::Disconnected(gateway_problem("session invalidated"))
        );
        listening.connected().await;
        assert_eq!(received(&fake, 2).len(), 2);
        assert!(received(&fake, 6).is_empty());
    }

    #[tokio::test]
    async fn a_long_cut_is_announced_once_and_ends_when_the_gateway_is_back() {
        // Connexion qui n'a pas tenu, puis connexion qui a tenu : sa reprise
        // immédiate échoue, et la coupure est annoncée de la même façon.
        for steady in [FAST.steady, Duration::ZERO] {
            let fake = FakeDiscord::start().await;
            let addr: SocketAddr = fake.url()["http://".len()..]
                .trim_end_matches("/api/v10")
                .parse()
                .unwrap();
            let mut client = client(&fake);
            client.timing.steady = steady;
            let mut listening = listen(&client);
            listening.connected().await;

            fake.stop().await;
            assert!(matches!(
                listening.next().await,
                Event::Disconnected(Problem::Network(_))
            ));
            // Des essais manqués (20, 40, 80, 80 ms…) : pas un mot de plus.
            sleep(Duration::from_millis(500)).await;
            listening.silent().await;

            let fake = FakeDiscord::start_on(addr).await.unwrap();
            listening.connected().await;
            listening.silent().await;
            // Après cinq reprises manquées, la session est abandonnée : une
            // nouvelle est ouverte, sans reprise.
            assert_eq!(received(&fake, 2).len(), 1);
            assert!(received(&fake, 6).is_empty());
            listening.stop.send(true).unwrap();
            listening.ended().await;
        }
    }

    #[tokio::test]
    async fn a_resume_that_never_succeeds_is_given_up_for_a_new_session() {
        use axum::extract::ws::{CloseFrame as WsClose, Message as WsMessage};

        // Passerelle qui ferme (4000) chaque session juste après l'avoir
        // ouverte, puis chaque reprise sans jamais dire `RESUMED` : le socket
        // s'ouvre à chaque essai, aucune reprise n'aboutit.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ops = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = ops.clone();
        let address = move || async move { Json(json!({"url": format!("ws://{addr}/gateway")})) };
        let gateway = move |upgrade: WebSocketUpgrade| {
            let log = log.clone();
            async move {
                upgrade.on_upgrade(move |mut socket| async move {
                    let hello = json!({"op": 10, "d": {"heartbeat_interval": 60_000}});
                    let _ = socket.send(WsMessage::Text(hello.to_string().into())).await;
                    while let Some(Ok(WsMessage::Text(text))) = socket.recv().await {
                        let op =
                            serde_json::from_str::<Value>(&text).unwrap_or_default()["op"].as_u64();
                        log.lock().unwrap().push(op);
                        if op == Some(2) {
                            let ready = json!({"op": 0, "t": "READY", "s": 1, "d": {
                                "session_id": "session-1",
                                "resume_gateway_url": format!("ws://{addr}/gateway"),
                                "user": {"id": "424242", "username": "chatonlm-test"},
                                "application": {"id": "424242"},
                            }});
                            let _ = socket.send(WsMessage::Text(ready.to_string().into())).await;
                        }
                        if matches!(op, Some(2 | 6)) {
                            let close = WsClose {
                                code: CLOSE_RESUMABLE,
                                reason: "".into(),
                            };
                            let _ = socket.send(WsMessage::Close(Some(close))).await;
                            return;
                        }
                    }
                })
            }
        };
        let router = Router::new()
            .route("/gateway/bot", get(address))
            .route("/gateway/", get(gateway));
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        let mut listening = listen(&client_at(&format!("http://{addr}"), TOKEN));
        listening.connected().await;
        let identified = |ops: &[Option<u64>]| -> Vec<usize> {
            (0..ops.len()).filter(|&at| ops[at] == Some(2)).collect()
        };
        until("une nouvelle session", || {
            identified(&ops.lock().unwrap()).len() >= 2
        })
        .await;
        // Cinq reprises jointes puis manquées, puis une nouvelle session.
        let ops = ops.lock().unwrap().clone();
        let second = identified(&ops)[1];
        let resumes = ops[..second].iter().filter(|op| **op == Some(6)).count();
        assert_eq!(resumes, RESUME_ATTEMPTS as usize, "{ops:?}");
        listening.stop.send(true).unwrap();
        listening.ended().await;
    }

    #[tokio::test]
    async fn a_spent_session_quota_waits_for_its_return() {
        let spent = || async {
            Json(json!({
                "url": "ws://127.0.0.1:9/gateway",
                "session_start_limit": {"total": 1000, "remaining": 0, "reset_after": 120_000},
            }))
        };
        let base = serve(Router::new().route("/gateway/bot", get(spent))).await;
        let mut listening = listen(&client_at(&base, TOKEN));
        assert_eq!(
            listening.next().await,
            Event::Disconnected(Problem::RateLimited(Duration::from_secs(120)))
        );
        // L'attente demandée (deux minutes), pas celle d'une reprise (20 ms).
        listening.silent().await;
        listening.stop.send(true).unwrap();
        listening.ended().await;
    }

    #[tokio::test]
    async fn a_quota_back_in_more_than_a_day_is_waited_one_day_at_most() {
        let spent = || async {
            Json(json!({
                "url": "ws://127.0.0.1:9/gateway",
                "session_start_limit": {"total": 1000, "remaining": 0, "reset_after": u64::MAX},
            }))
        };
        let base = serve(Router::new().route("/gateway/bot", get(spent))).await;
        let mut listening = listen(&client_at(&base, TOKEN));
        assert_eq!(
            listening.next().await,
            Event::Disconnected(Problem::RateLimited(Duration::from_secs(86_400)))
        );
        listening.stop.send(true).unwrap();
        listening.ended().await;
    }

    #[tokio::test]
    async fn a_gateway_that_says_nothing_is_dropped() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let address = move || async move { Json(json!({"url": format!("ws://{addr}/gateway")})) };
        // Connexion acceptée, puis plus rien : ni Hello ni fermeture.
        let mute = |upgrade: WebSocketUpgrade| async {
            upgrade.on_upgrade(|socket| async move {
                sleep(Duration::from_secs(30)).await;
                drop(socket);
            })
        };
        let router = Router::new()
            .route("/gateway/bot", get(address))
            .route("/gateway/", get(mute));
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        let mut client = client_at(&format!("http://{addr}"), TOKEN);
        client.timing.connect = Duration::from_millis(100);
        let mut listening = listen(&client);
        assert_eq!(
            listening.next().await,
            Event::Disconnected(gateway_problem("no session in time"))
        );
        listening.stop.send(true).unwrap();
        listening.ended().await;
    }

    #[tokio::test]
    async fn a_failing_gateway_address_is_asked_again() {
        let fake = FakeDiscord::start().await;
        fake.fail_next(
            "gateway",
            500,
            json!({"message": "500: Internal Server Error"}),
        );
        let mut listening = listen(&client(&fake));
        assert_eq!(
            listening.next().await,
            Event::Disconnected(Problem::Refused {
                status: 500,
                message: "500: Internal Server Error".to_owned(),
            })
        );
        listening.connected().await;
    }

    #[tokio::test]
    async fn a_refused_token_ends_the_listening() {
        let fake = FakeDiscord::start().await;

        // Refusé par l'API, avant la passerelle.
        let mut listening = listen(&client_at(&fake.url(), "wrong.token"));
        assert_eq!(
            listening.next().await,
            Event::Disconnected(Problem::Unauthorized)
        );
        listening.ended().await;
        assert!(fake.gateway_log().is_empty());

        // Refusé par la passerelle (fermeture 4004).
        let mut listening = listen(&client(&fake));
        listening.connected().await;
        fake.drop_gateway(Some(4004));
        assert_eq!(
            listening.next().await,
            Event::Disconnected(Problem::Unauthorized)
        );
        listening.ended().await;
        assert_eq!(received(&fake, 2).len(), 1);
    }

    #[tokio::test]
    async fn refused_intents_are_announced_and_not_asked_again_in_a_loop() {
        let fake = FakeDiscord::start().await;
        let mut listening = listen(&client(&fake));
        listening.connected().await;
        fake.drop_gateway(Some(4014));
        assert_eq!(
            listening.next().await,
            Event::Disconnected(Problem::Refused {
                status: 4014,
                message: String::new(),
            })
        );
        // L'attente longue (1 s ici), pas celle d'une reprise (20 ms).
        sleep(Duration::from_millis(300)).await;
        assert_eq!(received(&fake, 2).len(), 1);
        listening.connected().await;
        assert_eq!(received(&fake, 2).len(), 2);
    }

    #[tokio::test]
    async fn stopping_ends_the_listening_at_once() {
        let fake = FakeDiscord::start().await;

        // Reliée.
        let mut listening = listen(&client(&fake));
        listening.connected().await;
        listening.stop.send(true).unwrap();
        listening.ended().await;

        // Pendant l'attente d'une reprise.
        let mut waiting = client_at(&closed_api().await, TOKEN);
        waiting.timing.retry_first = Duration::from_secs(3600);
        let mut listening = listen(&waiting);
        assert!(matches!(
            listening.next().await,
            Event::Disconnected(Problem::Network(_))
        ));
        listening.stop.send(true).unwrap();
        listening.ended().await;

        // L'app n'écoute plus.
        let mut listening = listen(&client(&fake));
        listening.connected().await;
        let Listening { events, stop, task } = listening;
        drop(events);
        timeout(Duration::from_secs(5), task)
            .await
            .expect("fin de l'écoute")
            .unwrap();
        drop(stop);
    }

    #[tokio::test]
    async fn an_unreadable_payload_does_not_end_the_listening() {
        let fake = FakeDiscord::start().await;
        let mut listening = listen(&client(&fake));
        listening.connected().await;
        fake.gateway_send(json!("nonsense"));
        fake.gateway_send(json!({"op": "zero", "d": [1, 2]}));
        fake.gateway_send(json!({"op": 0, "t": "MESSAGE_CREATE", "d": {"author": 17}}));
        fake.gateway_send(json!({"op": 0, "t": "TYPING_START", "d": {}}));
        fake.message(json!({"user": "42", "text": "still here"}));
        assert_eq!(listening.message().await.text, "still here");
        listening.silent().await;
    }

    #[tokio::test]
    async fn a_secure_gateway_needs_the_configuration_of_the_app() {
        let client = Client::new(http(), DEFAULT_URL, TOKEN, None);
        assert_eq!(
            client.open("wss://gateway.discord.gg").await.err(),
            Some(gateway_problem("no TLS configuration"))
        );
        // Jamais de jeton en clair quand l'API est en https.
        assert_eq!(
            client.open("ws://127.0.0.1:9").await.err(),
            Some(gateway_problem("address not allowed"))
        );
    }

    #[tokio::test]
    async fn the_fake_gateway_closes_on_an_unknown_token_or_other_intents() {
        let fake = FakeDiscord::start().await;
        let client = client(&fake);
        let gateway = client.gateway_url().await.unwrap();
        for (token, intents, code) in [("wrong.token", 4096, 4004), (TOKEN, 4096 | 1 << 9, 4014)] {
            let mut socket = client.open(&gateway).await.unwrap();
            let hello = socket.next().await.unwrap().unwrap();
            assert!(hello.to_text().unwrap().contains("heartbeat_interval"));
            let identify = json!({"op": 2, "d": {"token": token, "intents": intents}});
            push(&mut socket, identify).await.unwrap();
            let Some(Ok(Message::Close(Some(frame)))) = socket.next().await else {
                panic!("fermeture attendue");
            };
            assert_eq!(u16::from(frame.code), code);
        }
    }
}
