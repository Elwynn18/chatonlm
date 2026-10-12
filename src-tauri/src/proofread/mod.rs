//! Correction et reformulation d'un texte par un modèle Ollama (propres à
//! ChatonLM) : page Proofread (« Correct », « Rephrase ») et raccourci global
//! « Correct selected text » (`selection.rs`).
//!
//! Le texte est découpé en passages (un par paragraphe, voir [`split`])
//! traités l'un après l'autre : un petit modèle corrige mieux un passage
//! court, la fenêtre de contexte n'est jamais dépassée et la page montre le
//! résultat au fur et à mesure. Un garde-fou (`guard.rs`) écarte une réponse
//! qui n'est plus le texte (réponse à une question du texte, traduction) : le
//! passage garde alors son texte d'origine.

mod answer;
// Distance d'édition et lettres sans accents : aussi pour les cours (`study`).
pub(crate) mod guard;
// Langue d'un texte : aussi pour les cours (`study`).
pub(crate) mod language;
mod prompts;
pub mod selection;
mod split;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tokio::sync::Notify;
use ts_rs::TS;

use crate::chat::Server;
use crate::chat::stream::{Line, LineBuffer, parse_line};
use crate::i18n::t;
use crate::ollama::client;
use crate::ollama::models::{self, ShowError};
use crate::store::Store;
use crate::store::settings::Settings;

#[cfg(test)]
use split::split;

/// Taille maximale d'un texte (caractères), page comme sélection.
pub const MAX_CHARS: usize = 100_000;

/// Correction seule, ou correction et reformulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ProofreadMode {
    /// Fautes corrigées, texte gardé tel quel par ailleurs.
    Correct,
    /// Fautes corrigées et texte réécrit plus clairement (même langue, même
    /// sens).
    Rephrase,
}

/// Passage du texte. Les `before` + `text` + `after` de tous les passages,
/// dans l'ordre, redonnent exactement le texte d'origine ; seul `text` est
/// envoyé au modèle (jamais vide, ni bordé d'espaces ou de sauts de ligne).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProofreadPart {
    pub before: String,
    pub text: String,
    pub after: String,
}

/// Résultat d'un passage terminé.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PartResult {
    /// Texte retenu (réponse nettoyée, ou passage d'origine si `kept`).
    pub text: String,
    /// Réponse écartée par le garde-fou : le passage garde son texte.
    pub kept: bool,
}

/// Code d'une erreur : le frontend choisit l'action à proposer, le message
/// (dans la langue de l'interface) est montré tel quel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ProofreadErrorCode {
    /// Aucun modèle réglé, ni pour la correction ni dans le chat.
    NoModel,
    /// Modèle absent du serveur.
    ModelNotFound,
    /// Serveur Ollama injoignable.
    Unreachable,
    /// Texte vide (seulement des espaces).
    Empty,
    /// Texte au-delà de [`MAX_CHARS`].
    TooLong,
    /// Autre échec (erreur du modèle, délai dépassé, réponse illisible).
    Failed,
}

/// Évènements d'une correction, dans l'ordre : `Started`, puis pour chaque
/// passage des `Delta` et un `PartDone`, puis `Done` ou `Error`. Une erreur
/// avant le découpage (pas de modèle, texte vide ou trop long) arrive seule,
/// sans `Started`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "event", rename_all = "camelCase")]
#[ts(export)]
pub enum ProofreadEvent {
    /// Modèle utilisé et découpage du texte.
    Started {
        model: String,
        parts: Vec<ProofreadPart>,
    },
    /// Fragment de la réponse du passage `index` (sans raisonnement).
    Delta { index: u32, text: String },
    /// Passage `index` terminé.
    PartDone { index: u32, result: PartResult },
    /// Fin : tous les passages traités, ou arrêt demandé (`stopped`) ; les
    /// passages non terminés gardent leur texte d'origine.
    Done { stopped: bool },
    /// Échec : les passages terminés restent valables, les autres gardent
    /// leur texte d'origine.
    Error {
        code: ProofreadErrorCode,
        message: String,
    },
}

/// Dernière correction du raccourci global, montrée par la page à la demande
/// de la notification (« Show »).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SelectionResult {
    pub model: String,
    pub parts: Vec<ProofreadPart>,
    /// Un par passage ; `None` : non terminé (texte d'origine).
    pub results: Vec<Option<PartResult>>,
}

/// Arrêt demandé d'un travail (clonable).
#[derive(Clone, Default)]
pub struct Cancel(Arc<CancelInner>);

#[derive(Default)]
struct CancelInner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl Cancel {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// Attend l'arrêt (aussitôt s'il est déjà demandé).
    pub async fn cancelled(&self) {
        loop {
            let notified = self.0.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// Bilan d'un travail (raccourci global).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub model: String,
    pub parts: Vec<ProofreadPart>,
    /// Un par passage ; `None` : non terminé (texte d'origine).
    pub results: Vec<Option<PartResult>>,
    /// Échec, s'il y en a eu un.
    pub error: Option<(ProofreadErrorCode, String)>,
    pub stopped: bool,
}

impl Outcome {
    /// Texte final : résultats des passages terminés, texte d'origine des
    /// autres, séparateurs d'origine.
    pub fn text(&self) -> String {
        assemble(&self.parts, &self.results)
    }
}

/// Texte reconstitué à partir des passages et de leurs résultats.
pub fn assemble(parts: &[ProofreadPart], results: &[Option<PartResult>]) -> String {
    let mut text = String::new();
    for (index, part) in parts.iter().enumerate() {
        text.push_str(&part.before);
        match results.get(index).and_then(Option::as_ref) {
            Some(result) => text.push_str(&result.text),
            None => text.push_str(&part.text),
        }
        text.push_str(&part.after);
    }
    text
}

/// Modèle de la correction : celui réglé, sinon celui du chat ; `None` si
/// aucun.
pub fn model_of(settings: &Settings) -> Option<String> {
    [&settings.proofread_model, &settings.selected_model]
        .into_iter()
        .find(|model| !model.is_empty())
        .cloned()
}

/// Délai d'un passage, réponse comprise (connexion : 2 s, voir `client`).
const PART_TIMEOUT: Duration = Duration::from_secs(120);

/// Corrige ou reformule `text` avec `model` sur `server` : évènements
/// envoyés à `emit` dans l'ordre (voir [`ProofreadEvent`]), arrêt dès que
/// `cancel` est déclenché (`Done { stopped: true }` aussitôt, requête en
/// cours abandonnée). Passages traités l'un après l'autre ; une erreur
/// arrête le travail (passages terminés gardés).
pub async fn run(
    server: &Server,
    model: &str,
    mode: ProofreadMode,
    text: &str,
    cancel: &Cancel,
    emit: &(dyn Fn(ProofreadEvent) + Send + Sync),
) -> Outcome {
    Job {
        server,
        model,
        mode,
        cancel,
        emit,
        timeout: PART_TIMEOUT,
    }
    .run(text)
    .await
}

/// Message d'échec dans la langue de l'interface, selon le bouton choisi
/// (« Proofreading failed » ou « Rephrasing failed »).
fn failure(mode: ProofreadMode, detail: &dyn std::fmt::Display) -> String {
    match mode {
        ProofreadMode::Correct => t!("proofread.failed", detail = detail),
        ProofreadMode::Rephrase => t!("proofread.failedRephrase", detail = detail),
    }
}

/// Réponse d'erreur d'Ollama : première ligne du corps, objet JSON avec un
/// message `error` (la page d'un proxy n'en a pas).
fn ollama_error(body: &str) -> bool {
    let line = body.lines().next().unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|value| {
            value["error"]
                .as_str()
                .map(|error| !error.trim().is_empty())
        })
        .unwrap_or(false)
}

/// Un travail en cours.
struct Job<'a> {
    server: &'a Server,
    model: &'a str,
    mode: ProofreadMode,
    cancel: &'a Cancel,
    emit: &'a (dyn Fn(ProofreadEvent) + Send + Sync),
    timeout: Duration,
}

/// Fin de la requête d'un passage.
enum Asked {
    /// Réponse complète (contenu brut, raisonnement compris).
    Answer(String),
    /// Réponse trop longue pour passer le garde-fou : lecture abandonnée.
    TooLong,
    /// Bloc de code, jamais envoyé.
    Code,
    Cancelled,
    Failed(ProofreadErrorCode, String),
}

impl Job<'_> {
    async fn run(&self, text: &str) -> Outcome {
        let mut outcome = Outcome {
            model: self.model.to_owned(),
            parts: Vec::new(),
            results: Vec::new(),
            error: None,
            stopped: false,
        };
        // Refus d'emblée : `Error` seul, sans `Started`.
        let refused = if text.trim().is_empty() {
            Some((ProofreadErrorCode::Empty, t!("proofread.empty")))
        } else if text.chars().count() > MAX_CHARS {
            Some((ProofreadErrorCode::TooLong, t!("proofread.tooLong")))
        } else if self.model.trim().is_empty() {
            Some((ProofreadErrorCode::NoModel, t!("proofread.noModel")))
        } else {
            None
        };
        if let Some((code, message)) = refused {
            return self.fail(outcome, code, message);
        }
        let (parts, code): (Vec<ProofreadPart>, Vec<bool>) =
            split::marked(text).into_iter().unzip();
        outcome.parts = parts;
        outcome.results = vec![None; outcome.parts.len()];
        (self.emit)(ProofreadEvent::Started {
            model: self.model.to_owned(),
            parts: outcome.parts.clone(),
        });

        let shown = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return self.stop(outcome),
            shown = models::show(&self.server.url, self.model) => shown,
        };
        let details = match shown {
            Ok(details) => Some(details),
            Err(ShowError::Unreachable(_)) => {
                let message = t!("ollama.unreachableAt", address = self.server.url);
                return self.fail(outcome, ProofreadErrorCode::Unreachable, message);
            }
            Err(ShowError::NotFound(message)) if message.contains("not found") => {
                let message = t!("proofread.modelNotFound", model = self.model);
                return self.fail(outcome, ProofreadErrorCode::ModelNotFound, message);
            }
            // Détails inconnus : la requête dira ce qui ne va pas.
            Err(_) => None,
        };
        let think_off = crate::dictation::polish::think_off(details.as_ref(), false);

        for index in 0..outcome.parts.len() {
            let original = outcome.parts[index].text.clone();
            // Bloc de code : jamais envoyé, rendu tel quel (pas « gardé par
            // le garde-fou »).
            let asked = if code[index] {
                Asked::Code
            } else {
                self.ask(index, &original, think_off).await
            };
            let result = match asked {
                Asked::Code => PartResult {
                    text: original,
                    kept: false,
                },
                Asked::Answer(answer) => {
                    let cleaned = answer::clean(&original, &answer);
                    if guard::accept(self.mode, &original, &cleaned) {
                        PartResult {
                            text: cleaned,
                            kept: false,
                        }
                    } else {
                        PartResult {
                            text: original,
                            kept: true,
                        }
                    }
                }
                Asked::TooLong => PartResult {
                    text: original,
                    kept: true,
                },
                Asked::Cancelled => return self.stop(outcome),
                Asked::Failed(code, message) => return self.fail(outcome, code, message),
            };
            outcome.results[index] = Some(result.clone());
            (self.emit)(ProofreadEvent::PartDone {
                index: index as u32,
                result,
            });
            if self.cancel.is_cancelled() {
                return self.stop(outcome);
            }
        }
        (self.emit)(ProofreadEvent::Done { stopped: false });
        outcome
    }

    fn fail(&self, mut outcome: Outcome, code: ProofreadErrorCode, message: String) -> Outcome {
        (self.emit)(ProofreadEvent::Error {
            code,
            message: message.clone(),
        });
        outcome.error = Some((code, message));
        outcome
    }

    fn stop(&self, mut outcome: Outcome) -> Outcome {
        (self.emit)(ProofreadEvent::Done { stopped: true });
        outcome.stopped = true;
        outcome
    }

    /// Requête d'un passage, lue jusqu'à `done` ; fragments visibles envoyés
    /// en `Delta`.
    async fn ask(&self, index: usize, original: &str, think_off: bool) -> Asked {
        let base = self.server.url.as_str();
        let mode = self.mode;
        let failed = |detail: &dyn std::fmt::Display| {
            Asked::Failed(ProofreadErrorCode::Failed, failure(mode, detail))
        };
        let unreachable = || {
            Asked::Failed(
                ProofreadErrorCode::Unreachable,
                t!("ollama.unreachableAt", address = base),
            )
        };
        let request_error = |error: reqwest::Error| {
            if error.is_connect() {
                unreachable()
            } else if error.is_timeout() {
                failed(&t!("proofread.timedOut"))
            } else {
                failed(&error.without_url())
            }
        };
        let Ok(url) = client::server_url(base) else {
            return unreachable();
        };
        let client = match client::client(&url, Some(self.timeout)) {
            Ok(client) => client,
            Err(error) => return failed(&error),
        };
        let body = prompts::request_body(self.server, self.model, self.mode, original, think_off);
        let request = client.post(format!("{base}/api/chat")).json(&body).send();
        let response = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Asked::Cancelled,
            response = request => response,
        };
        let mut response = match response {
            Ok(response) => response,
            Err(error) => return request_error(error),
        };
        let status = response.status();
        if !status.is_success() {
            let body = tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Asked::Cancelled,
                body = response.text() => body.unwrap_or_default(),
            };
            // Seule une erreur d'Ollama (JSON) dit un modèle absent : la page
            // 404 d'un proxy ou d'une mauvaise adresse est un échec, montré
            // avec son statut et sa première ligne (serveur absent ≠ modèle
            // absent, plan.md § D).
            let message = crate::chat::error_line(&body);
            let from_ollama = ollama_error(&body);
            if status == reqwest::StatusCode::NOT_FOUND && from_ollama {
                return Asked::Failed(
                    ProofreadErrorCode::ModelNotFound,
                    t!("proofread.modelNotFound", model = self.model),
                );
            }
            let status = status.as_u16();
            return if message.trim().is_empty() {
                failed(&t!("ollama.httpStatus", status = status))
            } else if from_ollama {
                failed(&message)
            } else {
                failed(&t!("ollama.http", status = status, message = message))
            };
        }
        // Au-delà, la réponse ne peut plus passer le garde-fou.
        let limit = guard::max_chars(self.mode, original);
        let mut buffer = LineBuffer::default();
        let mut visible = answer::Visible::new(original);
        let mut answer = String::new();
        loop {
            let chunk = tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Asked::Cancelled,
                chunk = response.chunk() => chunk,
            };
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => return request_error(error),
            };
            let ended = chunk.is_none();
            let lines = match chunk {
                Some(bytes) => buffer.push(&bytes),
                None => buffer.finish().into_iter().collect(),
            };
            for line in lines {
                match parse_line(&line) {
                    Ok(Line::Fragment { content, done, .. }) => {
                        answer.push_str(&content);
                        let mut shown = visible.push(&content);
                        if done {
                            // Début encore retenu (voir `answer::Visible`).
                            shown.push_str(&visible.finish());
                        }
                        if !shown.is_empty() {
                            (self.emit)(ProofreadEvent::Delta {
                                index: index as u32,
                                text: shown,
                            });
                        }
                        if done {
                            return Asked::Answer(answer);
                        }
                    }
                    Ok(Line::Error(message)) => return failed(&message),
                    Err(message) => return failed(&message),
                }
            }
            if visible.over(limit) {
                return Asked::TooLong;
            }
            if ended {
                // Fin du flux sans « done » : serveur arrêté ou proxy coupé.
                return failed(&t!("proofread.connectionLost"));
            }
        }
    }
}

/// Travaux en cours et dernière correction du raccourci global (état géré
/// par Tauri).
#[derive(Default)]
pub struct Proofreader {
    /// Travaux de la page en cours, par identifiant (choisi par la page).
    jobs: Mutex<HashMap<String, Cancel>>,
    /// Correction du raccourci global en cours (un appui pendant ce temps
    /// est ignoré).
    selection_running: AtomicBool,
    last_selection: Mutex<Option<SelectionResult>>,
}

impl Proofreader {
    /// Arrête le travail `job_id` ; sans effet s'il est déjà fini.
    pub fn cancel(&self, job_id: &str) {
        if let Some(cancel) = self.jobs.lock().get(job_id) {
            cancel.cancel();
        }
    }

    /// Arrête tous les travaux de la page : appelée à son chargement, elle
    /// arrête ceux d'une page rechargée, dont le canal ne lit plus rien (un
    /// envoi sur un canal Tauri n'échoue pas pour autant).
    pub fn cancel_all(&self) {
        for cancel in self.jobs.lock().values() {
            cancel.cancel();
        }
    }

    pub fn last_selection(&self) -> Option<SelectionResult> {
        self.last_selection.lock().clone()
    }
}

/// Travail de la page (commande `proofread_run`) : réglages lus, modèle
/// choisi (voir [`model_of`]), travail enregistré sous `job_id` le temps de
/// [`run`] (retiré à la fin, quelle qu'elle soit), évènements envoyés sur
/// `channel` ; un envoi qui échoue (page rechargée, fenêtre fermée) arrête
/// le travail.
pub async fn run_job(
    store: &Store,
    proofreader: &Proofreader,
    job_id: String,
    text: String,
    mode: ProofreadMode,
    channel: &Channel<ProofreadEvent>,
) -> crate::error::AppResult<()> {
    let cancel = Cancel::default();
    let _registered = Registered::new(proofreader, job_id, cancel.clone());
    let emit = |event: ProofreadEvent| {
        if channel.send(event).is_err() {
            cancel.cancel();
        }
    };
    let settings = match crate::commands::models::app_settings(store).await {
        Ok(settings) => settings,
        Err(error) => {
            emit(ProofreadEvent::Error {
                code: ProofreadErrorCode::Failed,
                message: failure(mode, &error),
            });
            return Ok(());
        }
    };
    let model = model_of(&settings).unwrap_or_default();
    run(
        &Server::from_settings(&settings),
        &model,
        mode,
        &text,
        &cancel,
        &emit,
    )
    .await;
    Ok(())
}

/// Travail inscrit dans [`Proofreader::jobs`] le temps de sa vie : retiré
/// à la fin, sauf si un autre travail a pris son identifiant.
struct Registered<'a> {
    proofreader: &'a Proofreader,
    job_id: String,
    cancel: Cancel,
}

impl<'a> Registered<'a> {
    fn new(proofreader: &'a Proofreader, job_id: String, cancel: Cancel) -> Self {
        proofreader
            .jobs
            .lock()
            .insert(job_id.clone(), cancel.clone());
        Self {
            proofreader,
            job_id,
            cancel,
        }
    }
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        let mut jobs = self.proofreader.jobs.lock();
        if jobs
            .get(&self.job_id)
            .is_some_and(|cancel| Arc::ptr_eq(&cancel.0, &self.cancel.0))
        {
            jobs.remove(&self.job_id);
        }
    }
}

#[cfg(test)]
mod tests;
