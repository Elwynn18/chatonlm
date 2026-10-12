//! Requêtes des cours à Ollama : modèle et serveur réglés, `/api/show`
//! (raisonnement coupé si possible), parties en JSON contraint
//! (`stream: false`), réponses en flux (Markdown). Erreurs comme la
//! correction : connexion → `unreachable`, 404 d'Ollama → `modelNotFound`,
//! autre → `failed` avec le statut et le message.

use std::time::Duration;

use serde_json::Value;

use super::prompts;
use super::repair;
use super::{StudyError, StudyErrorCode, model_of};
use crate::chat::Server;
use crate::chat::stream::{Line, LineBuffer, parse_line};
use crate::i18n::t;
use crate::ollama::client;
use crate::ollama::models::{self, ShowError};
use crate::proofread::Cancel;
use crate::store::Store;

/// Délais : partie en JSON (réponse complète), flux (sans fragment),
/// pause avant de retenter une partie en JSON après une erreur passagère ;
/// et longueur maximale d'une réponse en flux (octets reçus) : au-delà, elle
/// s'arrête là (un petit modèle qui boucle ne la fait pas grossir sans fin).
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    pub json: Duration,
    pub stream_idle: Duration,
    pub retry_delay: Duration,
    pub stream_bytes: usize,
}

pub const TIMEOUTS: Timeouts = Timeouts {
    json: Duration::from_secs(300),
    stream_idle: Duration::from_secs(120),
    retry_delay: Duration::from_secs(2),
    stream_bytes: 20_000,
};

/// Échec d'une requête, passager ou non.
struct Failure {
    stop: Stop,
    /// Erreur 5xx d'Ollama, délai dépassé, connexion coupée : à retenter.
    transient: bool,
}

impl Failure {
    fn lasting(stop: Stop) -> Self {
        Self {
            stop,
            transient: false,
        }
    }
}

/// Réponse lue au plus (une partie fait quelques Kio).
const MAX_BODY: usize = 4 * 1024 * 1024;

/// Fin anticipée d'une génération.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Arrêt demandé (cours supprimé, palier refait…) : rien à enregistrer.
    Cancelled,
    Failed(StudyError),
}

pub fn error(code: StudyErrorCode, message: String) -> StudyError {
    StudyError { code, message }
}

fn failed(detail: &dyn std::fmt::Display) -> Stop {
    Stop::Failed(error(
        StudyErrorCode::Failed,
        t!("study.failed", detail = detail),
    ))
}

/// Connexion d'un travail au serveur réglé, avec le modèle des cours.
pub struct Gen<'a> {
    pub server: Server,
    pub model: String,
    pub think_off: bool,
    pub cancel: &'a Cancel,
    pub timeouts: Timeouts,
}

impl<'a> Gen<'a> {
    /// Réglages lus au moment du travail (le modèle réglé peut avoir changé),
    /// modèle présent sur le serveur.
    pub async fn connect(
        store: &Store,
        cancel: &'a Cancel,
        timeouts: Timeouts,
    ) -> Result<Self, Stop> {
        let settings = crate::commands::models::app_settings(store)
            .await
            .map_err(|error| failed(&error))?;
        let Some(model) = model_of(&settings) else {
            return Err(Stop::Failed(error(
                StudyErrorCode::NoModel,
                t!("study.noModel"),
            )));
        };
        let server = Server::from_settings(&settings);
        let shown = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(Stop::Cancelled),
            shown = models::show(&server.url, &model) => shown,
        };
        let details = match shown {
            Ok(details) => Some(details),
            Err(ShowError::Unreachable(_)) => {
                return Err(Stop::Failed(error(
                    StudyErrorCode::Unreachable,
                    t!("ollama.unreachableAt", address = server.url),
                )));
            }
            Err(ShowError::NotFound(message)) if message.contains("not found") => {
                return Err(Stop::Failed(error(
                    StudyErrorCode::ModelNotFound,
                    t!("study.modelNotFound", model = model),
                )));
            }
            // Détails inconnus : la requête dira ce qui ne va pas.
            Err(_) => None,
        };
        let think_off = crate::dictation::polish::think_off(details.as_ref(), false);
        Ok(Self {
            server,
            model,
            think_off,
            cancel,
            timeouts,
        })
    }

    fn unreachable(&self) -> Stop {
        Stop::Failed(error(
            StudyErrorCode::Unreachable,
            t!("ollama.unreachableAt", address = self.server.url),
        ))
    }

    fn request_error(&self, error: reqwest::Error) -> Stop {
        if error.is_connect() {
            self.unreachable()
        } else if error.is_timeout() {
            failed(&t!("study.timedOut"))
        } else {
            failed(&error.without_url())
        }
    }

    /// Envoie `body` à `/api/chat` ; réponse d'erreur traduite.
    async fn send(
        &self,
        body: String,
        timeout: Option<Duration>,
    ) -> Result<reqwest::Response, Stop> {
        self.send_once(body, timeout)
            .await
            .map_err(|failure| failure.stop)
    }

    /// Comme [`Gen::send`] ; l'erreur dit si elle est passagère (5xx, délai
    /// dépassé, connexion coupée), donc à retenter.
    async fn send_once(
        &self,
        body: String,
        timeout: Option<Duration>,
    ) -> Result<reqwest::Response, Failure> {
        let base = self.server.url.as_str();
        let Ok(url) = client::server_url(base) else {
            return Err(Failure::lasting(self.unreachable()));
        };
        let client =
            client::client(&url, timeout).map_err(|error| Failure::lasting(failed(&error)))?;
        // Corps déjà en JSON (schéma `format` tel qu'écrit).
        let request = client
            .post(format!("{base}/api/chat"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send();
        let response = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Err(Failure::lasting(Stop::Cancelled)),
            response = request => response.map_err(|error| self.request_failure(error))?,
        };
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Err(Failure::lasting(Stop::Cancelled)),
            body = response.text() => body.unwrap_or_default(),
        };
        // Seule une erreur d'Ollama (JSON) dit un modèle absent : la 404 d'un
        // proxy est un échec (serveur absent ≠ modèle absent, plan.md § D).
        let message = crate::chat::error_line(&body);
        let from_ollama = ollama_error(&body);
        if status == reqwest::StatusCode::NOT_FOUND && from_ollama {
            return Err(Failure::lasting(Stop::Failed(error(
                StudyErrorCode::ModelNotFound,
                t!("study.modelNotFound", model = self.model),
            ))));
        }
        let transient = status.is_server_error();
        let status = status.as_u16();
        let stop = if message.trim().is_empty() {
            failed(&t!("ollama.httpStatus", status = status))
        } else if from_ollama {
            failed(&message)
        } else {
            failed(&t!("ollama.http", status = status, message = message))
        };
        Err(Failure { stop, transient })
    }

    /// Échec d'une requête : serveur injoignable (durable), délai dépassé ou
    /// connexion coupée (passagers).
    fn request_failure(&self, error: reqwest::Error) -> Failure {
        let transient = !error.is_connect();
        Failure {
            stop: self.request_error(error),
            transient,
        }
    }

    /// Partie en JSON contraint par `schema` ; `Ok(None)` : réponse sans
    /// objet lisible (le travail peut réessayer).
    pub async fn ask_json(
        &self,
        system: &str,
        user: &str,
        schema: &prompts::Schema,
        temperature: f64,
    ) -> Result<Option<Value>, Stop> {
        let Some(answer) = self.ask_raw(system, user, schema, temperature).await? else {
            return Ok(None);
        };
        // Raisonnement rendu malgré `think: false` (champ `thinking`) :
        // ignoré ; écrit dans le contenu : retiré par `parse_object`.
        let content = answer["message"]["content"].as_str().unwrap_or_default();
        // Un texte à sa limite : emballement, réponse inutilisable (même
        // chemin qu'un JSON illisible).
        let parsed = repair::parse_object(content)
            .filter(|value| !super::checks::runaway(value, &schema.value()));
        if parsed.is_none() {
            // i18n-ignore : journal.
            eprintln!(
                "chatonlm: study: unreadable answer from {} (done_reason {}, {} characters)",
                self.model,
                answer["done_reason"],
                content.chars().count()
            );
        }
        Ok(parsed)
    }

    /// Réponse d'Ollama telle quelle (objet de `/api/chat` : `message`,
    /// `done_reason`…) à une requête en JSON contraint ; `Ok(None)` : corps
    /// illisible ou trop grand.
    pub async fn ask_raw(
        &self,
        system: &str,
        user: &str,
        schema: &prompts::Schema,
        temperature: f64,
    ) -> Result<Option<Value>, Stop> {
        let body = prompts::json_body(
            &self.server,
            &self.model,
            self.think_off,
            prompts::messages(system, user),
            schema,
            temperature,
        );
        match self.ask_once(body.clone()).await {
            // Erreur passagère (5xx, délai, connexion coupée) : une nouvelle
            // tentative après une courte pause.
            Err(Failure {
                transient: true,
                stop,
            }) => {
                // i18n-ignore : journal.
                eprintln!("chatonlm: study: {stop:?}, trying again once");
                tokio::select! {
                    biased;
                    () = self.cancel.cancelled() => return Err(Stop::Cancelled),
                    () = tokio::time::sleep(self.timeouts.retry_delay) => {}
                }
                self.ask_once(body).await.map_err(|failure| failure.stop)
            }
            result => result.map_err(|failure| failure.stop),
        }
    }

    /// Une requête en JSON contraint et sa réponse lue.
    async fn ask_once(&self, body: String) -> Result<Option<Value>, Failure> {
        let mut response = self.send_once(body, Some(self.timeouts.json)).await?;
        let mut data = Vec::new();
        loop {
            let chunk = tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Err(Failure::lasting(Stop::Cancelled)),
                chunk = response.chunk() => chunk,
            };
            // Coupée ou trop lente au milieu de la réponse : passager.
            match chunk.map_err(|error| Failure {
                stop: self.request_error(error),
                transient: true,
            })? {
                Some(bytes) => {
                    if data.len() + bytes.len() > MAX_BODY {
                        return Ok(None);
                    }
                    data.extend_from_slice(&bytes);
                }
                None => break,
            }
        }
        let answer: Value = match serde_json::from_slice(&data) {
            Ok(answer) => answer,
            // Pas un objet seul : une ligne d'erreur d'Ollama s'y trouve
            // peut-être (modèle arrêté en cours de route).
            Err(_) => {
                let text = String::from_utf8_lossy(&data);
                let error = text.lines().find_map(|line| {
                    let value: Value = serde_json::from_str(line).ok()?;
                    value["error"].as_str().map(str::to_owned)
                });
                return match error {
                    Some(message) => Err(Failure::lasting(failed(&message))),
                    None => Ok(None),
                };
            }
        };
        if let Some(message) = answer["error"].as_str().filter(|m| !m.trim().is_empty()) {
            return Err(Failure::lasting(failed(&message)));
        }
        Ok(Some(answer))
    }

    /// Réponse en flux (`messages` complets) : fragments visibles envoyés à
    /// `on_delta` ; texte complet (raisonnement retiré) à la fin.
    pub async fn stream(
        &self,
        messages: Value,
        on_delta: &(dyn Fn(String) + Send + Sync),
    ) -> Result<String, Stop> {
        let body = prompts::stream_body(&self.server, &self.model, self.think_off, messages);
        // Le premier fragment (en-têtes compris) arrive lui aussi avant le
        // délai.
        let mut response = tokio::time::timeout(self.timeouts.stream_idle, self.send(body, None))
            .await
            .map_err(|_| failed(&t!("study.timedOut")))??;
        let mut buffer = LineBuffer::default();
        let mut visible = Visible::default();
        // Formules `$…$` écrites en `\(…\)` au fil du flux (voir
        // `repair::inline_math`).
        let mut math = repair::MathStream::default();
        loop {
            let chunk = tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Err(Stop::Cancelled),
                chunk = tokio::time::timeout(self.timeouts.stream_idle, response.chunk()) => chunk,
            };
            let chunk = match chunk {
                Ok(chunk) => chunk.map_err(|error| self.request_error(error))?,
                Err(_) => return Err(failed(&t!("study.timedOut"))),
            };
            let ended = chunk.is_none();
            let lines = match chunk {
                Some(bytes) => buffer.push(&bytes),
                None => buffer.finish().into_iter().collect(),
            };
            for line in lines {
                match parse_line(&line) {
                    Ok(Line::Fragment { content, done, .. }) => {
                        let shown = math.push(&visible.push(&content));
                        if !shown.is_empty() {
                            on_delta(shown);
                        }
                        // Trop long : réponse arrêtée là (requête lâchée).
                        let done = done || visible.len() > self.timeouts.stream_bytes;
                        if done {
                            let text = repair::inline_math(&visible.text());
                            let rest = math.finish(&text);
                            if !rest.is_empty() {
                                on_delta(rest);
                            }
                            return Ok(text);
                        }
                    }
                    Ok(Line::Error(message)) => return Err(failed(&message)),
                    Err(message) => return Err(failed(&message)),
                }
            }
            if ended {
                return Err(failed(&t!("study.connectionLost")));
            }
        }
    }
}

/// Réponse d'erreur d'Ollama : première ligne du corps, objet JSON avec un
/// message `error`.
fn ollama_error(body: &str) -> bool {
    let line = body.lines().next().unwrap_or_default();
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| {
            value["error"]
                .as_str()
                .map(|error| !error.trim().is_empty())
        })
        .unwrap_or(false)
}

/// Partie visible d'une réponse en flux : un raisonnement écrit dans le
/// contenu (`<think>…</think>`, ou seul `</think>` d'un raisonnement ouvert
/// par le gabarit) n'est jamais envoyé ; un début qui pourrait être `<think>`
/// est retenu. Ce qui a été envoyé n'est jamais repris : le texte final
/// ([`Visible::text`]) fait foi. Travail linéaire : chaque fragment n'est
/// examiné qu'une fois (plus les quelques octets d'une balise coupée).
#[derive(Default)]
pub struct Visible {
    raw: String,
    /// Octets de `raw` déjà traités (envoyés, écartés ou cherchés).
    done: usize,
    phase: Phase,
}

#[derive(Default, PartialEq, Eq)]
enum Phase {
    /// Début : peut-être un `<think>`.
    #[default]
    Start,
    /// Dans `<think>…` : attend `</think>`.
    Thinking,
    /// Après `</think>` : espaces sautées.
    AfterThinking,
    Passing,
    /// Une balise plus loin : plus rien d'envoyé.
    Stopped,
}

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

/// `index` ramené au début du caractère qui le contient.
fn floor(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Fin de `text` qui pourrait commencer une balise : gardée pour la suite.
fn held_back(text: &str) -> usize {
    (1..=CLOSE.len().min(text.len()))
        .rev()
        .find(|length| {
            let start = text.len() - length;
            text.is_char_boundary(start)
                && (OPEN.starts_with(&text[start..]) || CLOSE.starts_with(&text[start..]))
        })
        .unwrap_or(0)
}

impl Visible {
    /// Ajoute un fragment ; renvoie le texte visible nouveau.
    pub fn push(&mut self, fragment: &str) -> String {
        self.raw.push_str(fragment);
        let mut shown = String::new();
        loop {
            match self.phase {
                Phase::Start => {
                    let trimmed = self.raw.trim_start();
                    if trimmed.starts_with(OPEN) {
                        self.done = self.raw.len() - trimmed.len() + OPEN.len();
                        self.phase = Phase::Thinking;
                    } else if trimmed.is_empty() || OPEN.starts_with(trimmed) {
                        return shown;
                    } else {
                        self.phase = Phase::Passing;
                    }
                }
                Phase::Thinking => {
                    let from = floor(&self.raw, self.done.saturating_sub(CLOSE.len()));
                    match self.raw[from..].find(CLOSE) {
                        Some(at) => {
                            self.done = from + at + CLOSE.len();
                            self.phase = Phase::AfterThinking;
                        }
                        None => {
                            self.done = self.raw.len();
                            return shown;
                        }
                    }
                }
                Phase::AfterThinking => {
                    let rest = &self.raw[self.done..];
                    let trimmed = rest.trim_start();
                    self.done += rest.len() - trimmed.len();
                    if trimmed.is_empty() {
                        return shown;
                    }
                    self.phase = Phase::Passing;
                }
                Phase::Passing => {
                    let from = floor(&self.raw, self.done.saturating_sub(CLOSE.len()));
                    let tail = &self.raw[from..];
                    if tail.contains(CLOSE) {
                        self.phase = Phase::Stopped;
                        return shown;
                    }
                    if let Some(at) = tail.find(OPEN) {
                        let at = from + at;
                        if at > self.done {
                            shown.push_str(&self.raw[self.done..at]);
                        }
                        self.done = self.raw.len();
                        self.phase = Phase::Stopped;
                        return shown;
                    }
                    let end = self.raw.len() - held_back(&self.raw);
                    if end > self.done {
                        shown.push_str(&self.raw[self.done..end]);
                        self.done = end;
                    }
                    return shown;
                }
                Phase::Stopped => return shown,
            }
        }
    }

    /// Octets reçus.
    pub fn len(&self) -> usize {
        self.raw.len()
    }

    /// Texte complet, sans raisonnement.
    pub fn text(&self) -> String {
        repair::strip_thinking(&self.raw).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_in_a_stream_is_never_sent() {
        let mut visible = Visible::default();
        let pieces = [
            "<th",
            "ink>\nLet me",
            " think.</thi",
            "nk>\n\nThe ",
            "answer.",
        ];
        let sent: String = pieces.iter().map(|piece| visible.push(piece)).collect();
        assert_eq!(sent, "The answer.");
        assert_eq!(visible.text(), "The answer.");

        let mut visible = Visible::default();
        let sent: String = ["Plain ", "<", "b>bold</b> text"]
            .iter()
            .map(|p| visible.push(p))
            .collect();
        assert_eq!(sent, "Plain <b>bold</b> text");

        // Raisonnement au milieu : envoyé jusqu'à `<think>`, plus rien après.
        let mut visible = Visible::default();
        let sent: String = ["Hello <th", "ink>secret</think> there"]
            .iter()
            .map(|p| visible.push(p))
            .collect();
        assert_eq!(sent, "Hello ");
        // Balise coupée sur plusieurs fragments, caractères multi-octets.
        let mut visible = Visible::default();
        let sent: String = ["é<", "/thi", "nk>"]
            .iter()
            .map(|p| visible.push(p))
            .collect();
        assert_eq!(sent, "é");
        assert_eq!(visible.text(), "");
        // Raisonnement ouvert par le gabarit : montré avant `</think>`, mais
        // le texte final n'a que la réponse.
        let mut visible = Visible::default();
        visible.push("Reasoning ");
        assert_eq!(visible.push("</think> Answer"), "");
        assert_eq!(visible.text(), "Answer");
    }
}

#[cfg(test)]
mod measure {
    use super::*;

    /// rust-7 : coût d'un flux linéaire. Fragments de 8 caractères d'un
    /// texte Markdown (formules, montants, code). Avant la correction (tout
    /// reconverti à chaque fragment) : 2 000 fragments 0,68 s, 4 000 2,67 s,
    /// 8 000 10,1 s (debug) ; après : quelques millisecondes.
    #[test]
    fn streams_are_processed_in_linear_time() {
        let paragraph = "The leaf uses $\\frac{1}{2}$ of the light, costs $5 and `code` too. ";
        let mut text = String::new();
        let mut line = 0;
        while text.len() < 8_000 * 8 {
            text.push_str(paragraph);
            line += 1;
            if line % 7 == 0 {
                text.push('\n');
            }
        }
        let chars: Vec<char> = text.chars().collect();
        let started = std::time::Instant::now();
        let mut visible = Visible::default();
        let mut math = repair::MathStream::default();
        let mut sent = String::new();
        for piece in chars.chunks(8) {
            let piece: String = piece.iter().collect();
            sent.push_str(&math.push(&visible.push(&piece)));
        }
        let elapsed = started.elapsed();
        assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
        assert!(repair::inline_math(&text).starts_with(&sent));
    }
}
