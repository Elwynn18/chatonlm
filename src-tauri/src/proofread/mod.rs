//! Correction et reformulation d'un texte par un modèle du moteur de l'app
//! (propres à ChatonLM) : page Proofread (« Correct », « Rephrase ») et raccourci global
//! « Correct selected text » (`selection.rs`).
//!
//! Le texte est découpé en passages (un par paragraphe, voir [`split`])
//! traités l'un après l'autre : un petit modèle corrige mieux un passage
//! court, la fenêtre de contexte n'est jamais dépassée et la page montre le
//! résultat au fur et à mesure. Un garde-fou (`guard.rs`) écarte une réponse
//! qui n'est plus le texte (réponse à une question du texte, traduction) : le
//! passage garde alors son texte d'origine.

mod answer;
// Majuscule et point de la mise en forme automatique (voir `guard::settle_with`).
mod format;
// Distance d'édition et lettres sans accents : aussi pour les cours (`study`).
pub(crate) mod guard;
// Langue d'un texte : aussi pour les cours (`study`).
pub(crate) mod language;
mod prompts;
// Retours à la ligne souples rendus à une correction (voir `guard::settle`).
mod reflow;
pub mod selection;
mod split;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::chat::{ChatErrorKind, Server};
use crate::error::AppError;
use crate::i18n::t;
use crate::inference::Event;
use crate::llm::{self, Access, LlmError, OpenOptions, Session};
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

/// Options de la correction (menu de la page Proofread, réglages
/// `proofreadKeepOrder`, `proofreadFormatting`, `proofreadDigits`), valables
/// aussi pour le raccourci global et le lien VS Code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProofreadOptions {
    /// Mots gardés dans leur ordre : seule leur écriture est corrigée, aucun
    /// mot ajouté, retiré ou déplacé (correction seulement).
    pub keep_order: bool,
    /// Mise en forme automatique : retours à la ligne souples joints,
    /// majuscule au début et point à la fin de chaque passage.
    pub formatting: bool,
    /// Nombres écrits en chiffres gardés en chiffres.
    pub digits: bool,
}

impl Default for ProofreadOptions {
    fn default() -> Self {
        Self {
            keep_order: false,
            formatting: true,
            digits: false,
        }
    }
}

impl ProofreadOptions {
    pub fn of(settings: &Settings) -> Self {
        Self {
            keep_order: settings.proofread_keep_order,
            formatting: settings.proofread_formatting,
            digits: settings.proofread_digits,
        }
    }
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
    /// Le moteur n'a pas pu servir le modèle (démarrage, plantage, délai).
    EngineFailed,
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

/// Modèle de la correction : celui réglé, sinon « Auto » (le meilleur
/// modèle prêt dans le moteur, sinon celui du chat, voir
/// [`crate::engine::auto`]) ; `None` si aucun.
pub async fn model_for(access: &Access, settings: &Settings) -> Option<String> {
    crate::engine::auto::model_for(access, &settings.proofread_model, &settings.selected_model)
        .await
}

/// Réglages de l'app lus dans la base.
async fn settings_of(store: &Store) -> crate::error::AppResult<Settings> {
    store
        .call(|connection| crate::store::settings::get(connection))
        .await
}

/// Délai d'un passage, réponse comprise, une fois le modèle prêt. Son
/// lancement n'est pas compté (le moteur borne l'attente de « prêt ») ; un
/// modèle tenu par l'Étude avec un plus grand contexte est repris sans
/// attendre (`Engine::ensure_at_least`) ; l'arrêt reste possible.
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
    options: ProofreadOptions,
    text: &str,
    cancel: &Cancel,
    emit: &(dyn Fn(ProofreadEvent) + Send + Sync),
) -> Outcome {
    // L'autocomplétion de l'éditeur attend pendant ce temps.
    let _working = crate::gpu::activity::hold();
    Job {
        server,
        model,
        mode,
        options,
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

/// Erreur du moteur pour la page : code et message ; `None` : un arrêt
/// (annulation, modèle déchargé à la demande).
fn engine_failure(
    mode: ProofreadMode,
    model: &str,
    error: &LlmError,
) -> Option<(ProofreadErrorCode, String)> {
    Some(match error.chat_kind()? {
        ChatErrorKind::ModelMissing => (
            ProofreadErrorCode::ModelNotFound,
            t!("proofread.modelNotFound", model = model),
        ),
        ChatErrorKind::EngineUnavailable | ChatErrorKind::EngineFailed => {
            (ProofreadErrorCode::EngineFailed, error.message())
        }
        ChatErrorKind::ConnectionLost => (
            ProofreadErrorCode::Failed,
            failure(mode, &t!("proofread.connectionLost")),
        ),
        _ => (ProofreadErrorCode::Failed, failure(mode, &error.message())),
    })
}

/// Texte retenu pour un passage d'après la réponse du modèle : nettoyée,
/// puis remise en forme et acceptée par le garde-fou ; sinon le passage
/// garde son texte (`kept`).
fn settle_part(
    mode: ProofreadMode,
    options: ProofreadOptions,
    original: &str,
    answer: &str,
) -> PartResult {
    let cleaned = answer::clean(original, answer);
    match guard::settle_with(mode, options, original, cleaned) {
        Some(text) => PartResult { text, kept: false },
        None => PartResult {
            text: original.to_owned(),
            kept: true,
        },
    }
}

/// Un travail en cours.
struct Job<'a> {
    server: &'a Server,
    model: &'a str,
    mode: ProofreadMode,
    options: ProofreadOptions,
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
        } else if llm::ids::is_remote(self.model) && !self.server.prefs.remote_models {
            // Modèle distant, réglage coupé : rien ne part.
            Some((ProofreadErrorCode::Failed, LlmError::RemoteOff.message()))
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

        let access = &self.server.access;
        let details = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return self.stop(outcome),
            details = llm::models::details(access, self.model) => details,
        };
        let details = match details {
            Ok(details) => details,
            Err(AppError::NotFound(_)) => {
                let message = t!("proofread.modelNotFound", model = self.model);
                return self.fail(outcome, ProofreadErrorCode::ModelNotFound, message);
            }
            Err(error) => {
                let message = failure(self.mode, &error);
                return self.fail(outcome, ProofreadErrorCode::Failed, message);
            }
        };
        let think_off = crate::dictation::polish::think_off_on(Some(&details), false);
        // Modèle ouvert au premier passage envoyé (un texte fait seulement de
        // code ne le charge pas), tenu jusqu'à la fin : il n'est pas
        // déchargé entre deux passages.
        let mut session: Option<Session> = None;

        for index in 0..outcome.parts.len() {
            let original = outcome.parts[index].text.clone();
            // Bloc de code : jamais envoyé, rendu tel quel (pas « gardé par
            // le garde-fou »).
            let asked = if code[index] {
                Asked::Code
            } else {
                if session.is_none() {
                    session = match self.open().await {
                        Ok(opened) => Some(opened),
                        Err(Asked::Failed(code, message)) => {
                            return self.fail(outcome, code, message);
                        }
                        Err(_) => return self.stop(outcome),
                    };
                }
                let opened = session.as_ref().expect("opened above");
                self.ask(opened, index, &original, think_off).await
            };
            let result = match asked {
                Asked::Code => PartResult {
                    text: original,
                    kept: false,
                },
                // Texte remis en forme par le garde-fou (retours à la ligne
                // souples rendus) : c'est lui qui est rendu.
                Asked::Answer(answer) => settle_part(self.mode, self.options, &original, &answer),
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

    /// Modèle prêt dans le moteur (chargé s'il le faut, avec le contexte et
    /// la durée en mémoire de l'app) ; erreur : `Failed` ou `Cancelled`.
    async fn open(&self) -> Result<Session, Asked> {
        let never = CancellationToken::new();
        let opened = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Err(Asked::Cancelled),
            opened = self.server.access.open(self.model, OpenOptions::default(), &never, &|| {}) => opened,
        };
        opened.map_err(|error| self.asked(&error))
    }

    /// Fin d'un passage pour une erreur du moteur.
    fn asked(&self, error: &LlmError) -> Asked {
        match engine_failure(self.mode, self.model, error) {
            Some((code, message)) => Asked::Failed(code, message),
            None => Asked::Cancelled,
        }
    }

    /// Requête d'un passage, lue jusqu'à la fin de la réponse ; fragments
    /// visibles envoyés en `Delta` (la réflexion, à part, jamais).
    async fn ask(&self, session: &Session, index: usize, original: &str, think_off: bool) -> Asked {
        let mut request = session.request(prompts::messages(self.mode, self.options, original));
        prompts::tune(&mut request, think_off);
        let deadline = tokio::time::sleep(self.timeout);
        tokio::pin!(deadline);
        let timed_out = || {
            Asked::Failed(
                ProofreadErrorCode::Failed,
                failure(self.mode, &t!("proofread.timedOut")),
            )
        };
        let opened = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Asked::Cancelled,
            () = &mut deadline => return timed_out(),
            opened = llm::chat_stream(session, &request, CancellationToken::new()) => opened,
        };
        let mut stream = match opened {
            Ok(stream) => stream,
            Err(error) => return self.asked(&error),
        };
        // Au-delà, la réponse ne peut plus passer le garde-fou.
        let limit = guard::max_chars(self.mode, original);
        let mut visible = answer::Visible::new(original);
        let mut answer = String::new();
        loop {
            // Flux lâché (arrêt, délai, trop long) : la connexion se ferme,
            // le serveur arrête la génération.
            let next = tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Asked::Cancelled,
                () = &mut deadline => return timed_out(),
                next = stream.next() => next,
            };
            match next {
                Some(Ok(Event::Content(content))) => {
                    answer.push_str(&content);
                    let shown = visible.push(&content);
                    if !shown.is_empty() {
                        (self.emit)(ProofreadEvent::Delta {
                            index: index as u32,
                            text: shown,
                        });
                    }
                }
                Some(Ok(Event::Done { .. })) => {
                    // Début encore retenu (voir `answer::Visible`).
                    let shown = visible.finish();
                    if !shown.is_empty() {
                        (self.emit)(ProofreadEvent::Delta {
                            index: index as u32,
                            text: shown,
                        });
                    }
                    return Asked::Answer(answer);
                }
                // Réflexion à part, appels d'outils (aucun offert) : ignorés.
                Some(Ok(_)) => {}
                Some(Err(error)) => return self.asked(&session.error(error)),
                // Fin du flux sans fin de réponse : coupé.
                None => {
                    return Asked::Failed(
                        ProofreadErrorCode::Failed,
                        failure(self.mode, &t!("proofread.connectionLost")),
                    );
                }
            }
            if visible.over(limit) {
                return Asked::TooLong;
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
    access: &crate::llm::Access,
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
    let settings = match settings_of(store).await {
        Ok(settings) => settings,
        Err(error) => {
            emit(ProofreadEvent::Error {
                code: ProofreadErrorCode::Failed,
                message: failure(mode, &error),
            });
            return Ok(());
        }
    };
    let model = model_for(access, &settings).await.unwrap_or_default();
    run(
        &Server::with_settings(access.clone(), &settings),
        &model,
        mode,
        ProofreadOptions::of(&settings),
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
