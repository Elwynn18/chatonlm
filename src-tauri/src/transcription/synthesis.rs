//! « Synthétiser » (outil Transcription, propre à ChatonLM) : document de
//! lecture en Markdown tiré d'une transcription par un modèle du moteur de
//! l'app (titre, introduction, sections, points clés, conclusion), dans la
//! langue de la transcription, sans horodatage.
//!
//! - Modèle : celui choisi sur la page, sinon « Auto » comme Correction et
//!   Étude (`engine::auto` : le plus grand modèle prêt dans le moteur,
//!   sinon celui du chat).
//! - Contexte : celui du processus chargé (`Session::context`), les jetons
//!   d'un extrait de la transcription comptés par le modèle (`/tokenize`)
//!   pour régler le nombre de caractères par jeton ([`budget`]).
//! - Transcription qui tient dans le contexte : document écrit d'un coup.
//!   Sinon map-reduce : parties alignées sur les paragraphes ([`split_parts`]),
//!   notes sur chacune, notes regroupées tant qu'elles ne tiennent pas, puis
//!   document écrit d'après les notes. Seul le document part en flux vers la
//!   page (`markdown` grandit) ; pendant les notes, il reste vide.
//! - Réflexion coupée quand le modèle le permet ; une réflexion écrite dans
//!   le contenu (`<think>…</think>`) n'est jamais montrée.
//! - Garde de la langue (`proofread::language`, six langues reconnues) : un
//!   document écrit dans une autre langue est redemandé une fois, avec un
//!   rappel ; le second est gardé quel qu'il soit.
//! - Une synthèse à la fois ; annulable à tout moment (requête abandonnée,
//!   rien d'enregistré). Fini : document enregistré avec la transcription
//!   (`summary`, `summary_model`, `summary_at`), rendu par `transcription_get`.
//!
//! État vers la page par l'événement [`EVENT`] (10 fois par seconde au plus
//! pendant le flux ; tout changement d'étape aussitôt), et par
//! `transcription_synthesis_status` pour une page rechargée.

mod prompts;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::chat::ChatErrorKind;
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::inference::{self, Event, Message, Think};
use crate::llm::{self, Access, LlmError, OpenOptions, Session};
use crate::proofread::language::{self, Language};
use crate::store::transcriptions;
use prompts::From;

/// Événement envoyé à la page à chaque changement de [`SynthesisStatus`].
pub const EVENT: &str = "transcription-synthesis";

/// Étape de la synthèse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SynthesisStage {
    Running,
    /// Document fini et enregistré.
    Done,
    Error,
    Cancelled,
}

/// Nature d'un échec (texte : [`SynthesisError::message`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SynthesisErrorCode {
    /// Modèle absent (plus installé, distant indisponible).
    ModelNotFound,
    /// Le moteur n'a pas pu servir le modèle (absent, démarrage, plantage,
    /// connexion coupée, modèle déchargé pendant la synthèse).
    EngineFailed,
    /// Contexte du modèle trop petit pour une partie de la transcription.
    ContextTooSmall,
    /// Réponse vide.
    Empty,
    /// Autre échec (délai dépassé, erreur du modèle ou du fournisseur,
    /// enregistrement impossible).
    Failed,
}

/// Échec : code, texte dans la langue du moment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SynthesisError {
    pub code: SynthesisErrorCode,
    pub message: String,
}

/// Synthèse en cours, ou la dernière depuis le lancement de l'app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SynthesisStatus {
    /// Transcription synthétisée.
    pub id: String,
    pub stage: SynthesisStage,
    /// Document entier tel qu'il est à cet instant (vide pendant les notes
    /// d'une transcription longue ; le document enregistré une fois
    /// `done`).
    pub markdown: String,
    /// Modèle employé.
    pub model: Option<String>,
    pub error: Option<SynthesisError>,
}

/// Délais (réglables par les essais).
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Écart minimal entre deux événements du flux.
    pub emit_gap: Duration,
    /// Silence toléré du modèle (avant son premier morceau compris : une
    /// longue partie se lit d'abord).
    pub stall: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            emit_gap: Duration::from_millis(100),
            stall: Duration::from_secs(300),
        }
    }
}

/// Synthèses (état géré de l'app, clonable).
#[derive(Clone)]
pub struct Synthesizer {
    shared: Arc<Shared>,
}

struct Shared {
    inner: Mutex<Inner>,
    emit: Box<dyn Fn(&SynthesisStatus) + Send + Sync>,
    timing: Timing,
}

#[derive(Default)]
struct Inner {
    status: Option<SynthesisStatus>,
    running: bool,
    cancel: Option<CancellationToken>,
    last_emit: Option<Instant>,
    task: Option<tokio::task::JoinHandle<()>>,
}

/// Ce qui est synthétisé.
#[derive(Clone, Debug)]
struct Job {
    id: String,
    title: String,
    text: String,
    /// Nom anglais de la langue (consignes) et langue reconnue par la garde.
    language: Option<&'static str>,
    guard: Option<Language>,
    model: String,
}

impl Synthesizer {
    pub fn new(emit: impl Fn(&SynthesisStatus) + Send + Sync + 'static) -> Self {
        Self::with_timing(emit, Timing::default())
    }

    pub fn with_timing(
        emit: impl Fn(&SynthesisStatus) + Send + Sync + 'static,
        timing: Timing,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner::default()),
                emit: Box::new(emit),
                timing,
            }),
        }
    }

    /// Synthèse en cours, ou la dernière (document entier).
    pub fn status(&self) -> Option<SynthesisStatus> {
        self.shared.inner.lock().status.clone()
    }

    /// Une synthèse tourne.
    pub fn is_busy(&self) -> bool {
        self.shared.inner.lock().running
    }

    /// Annule la synthèse de `id` (sans effet si elle ne tourne pas).
    pub fn cancel(&self, id: &str) {
        let inner = self.shared.inner.lock();
        if inner.running
            && inner.status.as_ref().is_some_and(|status| status.id == id)
            && let Some(cancel) = &inner.cancel
        {
            cancel.cancel();
        }
    }

    /// Lance la synthèse de la transcription `id` avec `model` (absent :
    /// « Auto »). Refusée si une synthèse tourne, si la transcription
    /// n'existe pas ou n'a pas de texte, si aucun modèle n'est disponible ;
    /// tout autre échec arrive par l'état.
    pub async fn start(&self, access: &Access, id: String, model: Option<String>) -> AppResult<()> {
        if self.is_busy() {
            return Err(AppError::Invalid(t!("synthesis.busy")));
        }
        let key = id.clone();
        let detail = access
            .store()
            .call(move |connection| transcriptions::get(connection, &key))
            .await?
            .ok_or_else(|| AppError::NotFound(t!("transcription.notFound")))?;
        if detail.text.trim().is_empty() {
            return Err(AppError::Invalid(t!("synthesis.noText")));
        }
        let settings = access
            .store()
            .call(|connection| crate::store::settings::get(connection))
            .await?;
        let model = crate::engine::auto::model_for(
            access,
            model.as_deref().unwrap_or_default(),
            &settings.selected_model,
        )
        .await
        .ok_or_else(|| AppError::Invalid(t!("synthesis.noModel")))?;
        let code = detail
            .item
            .language
            .clone()
            .filter(|code| crate::dictation::languages::name_of(code).is_some())
            .or_else(|| super::job::detect_language(&detail.text));
        let job = Job {
            id: id.clone(),
            title: detail.item.title,
            text: detail.text,
            language: code
                .as_deref()
                .and_then(crate::dictation::languages::name_of),
            guard: code.as_deref().and_then(guard_language),
            model: model.clone(),
        };
        let mut inner = self.shared.inner.lock();
        if inner.running {
            return Err(AppError::Invalid(t!("synthesis.busy")));
        }
        let cancel = CancellationToken::new();
        inner.status = Some(SynthesisStatus {
            id,
            stage: SynthesisStage::Running,
            markdown: String::new(),
            model: Some(model),
            error: None,
        });
        inner.running = true;
        inner.cancel = Some(cancel.clone());
        let shared = self.shared.clone();
        let access = access.clone();
        inner.task = Some(tokio::spawn(async move {
            run(shared, access, job, cancel).await;
        }));
        self.shared.send(&mut inner);
        Ok(())
    }

    /// Fermeture de l'app : synthèse annulée, attendue `limit` au plus.
    pub async fn shutdown(&self, limit: Duration) {
        let task = {
            let mut inner = self.shared.inner.lock();
            if let Some(cancel) = &inner.cancel {
                cancel.cancel();
            }
            inner.task.take()
        };
        if let Some(task) = task {
            let _ = tokio::time::timeout(limit, task).await;
        }
    }

    /// Attend la fin de la synthèse en cours (essais).
    #[cfg(test)]
    pub(crate) async fn wait(&self) {
        let task = self.shared.inner.lock().task.take();
        if let Some(task) = task {
            task.await.unwrap();
        }
    }
}

impl Shared {
    /// État envoyé à la page.
    fn send(&self, inner: &mut Inner) {
        inner.last_emit = Some(Instant::now());
        if let Some(status) = &inner.status {
            (self.emit)(status);
        }
    }

    /// Document montré pendant le flux (10 fois par seconde au plus).
    fn show(&self, markdown: &str) {
        let mut inner = self.inner.lock();
        let Some(status) = inner.status.as_mut() else {
            return;
        };
        if status.markdown == markdown {
            return;
        }
        markdown.clone_into(&mut status.markdown);
        if inner
            .last_emit
            .is_none_or(|last| last.elapsed() >= self.timing.emit_gap)
        {
            self.send(&mut inner);
        }
    }

    /// Fin : dernier état envoyé, une autre synthèse peut commencer.
    fn finish(
        &self,
        stage: SynthesisStage,
        markdown: Option<String>,
        error: Option<SynthesisError>,
    ) {
        let mut inner = self.inner.lock();
        if let Some(status) = inner.status.as_mut() {
            status.stage = stage;
            if let Some(markdown) = markdown {
                status.markdown = markdown;
            }
            status.error = error;
        }
        inner.running = false;
        inner.cancel = None;
        self.send(&mut inner);
    }
}

/// Langue reconnue par la garde (`proofread::language`), d'après son code.
fn guard_language(code: &str) -> Option<Language> {
    Some(match code {
        "en" => Language::English,
        "fr" => Language::French,
        "es" => Language::Spanish,
        "de" => Language::German,
        "it" => Language::Italian,
        "pt" => Language::Portuguese,
        _ => return None,
    })
}

/// Fin d'une synthèse avant le document.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Stop {
    Cancelled,
    Failed(SynthesisErrorCode, String),
}

impl Stop {
    fn failed(code: SynthesisErrorCode, message: String) -> Self {
        Stop::Failed(code, message)
    }
}

async fn run(shared: Arc<Shared>, access: Access, job: Job, cancel: CancellationToken) {
    let show = |markdown: &str| shared.show(markdown);
    let made = synthesize(&access, &job, &cancel, shared.timing, &show).await;
    let (stage, markdown, error) = match made {
        Ok(markdown) => {
            let (id, model, text) = (job.id.clone(), job.model.clone(), markdown.clone());
            let saved = access
                .store()
                .call(move |connection| transcriptions::set_summary(connection, &id, &text, &model))
                .await;
            match saved {
                Ok(_) => (SynthesisStage::Done, Some(markdown), None),
                Err(error) => {
                    crate::app_log!("chatonlm: synthesis: not saved: {error}");
                    (
                        SynthesisStage::Error,
                        Some(markdown),
                        Some(SynthesisError {
                            code: SynthesisErrorCode::Failed,
                            message: t!("synthesis.notSaved", detail = error),
                        }),
                    )
                }
            }
        }
        Err(Stop::Cancelled) => (SynthesisStage::Cancelled, None, None),
        Err(Stop::Failed(code, message)) => {
            crate::app_log!("chatonlm: synthesis: {code:?}: {message}");
            (
                SynthesisStage::Error,
                None,
                Some(SynthesisError { code, message }),
            )
        }
    };
    shared.finish(stage, markdown, error);
}

/// Budget d'une synthèse dans le contexte du modèle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Budget {
    /// Taille d'une partie de la transcription (caractères).
    pub part_chars: usize,
    /// Taille de ce qui tient dans la demande du document : transcription
    /// entière ou notes (caractères).
    pub source_chars: usize,
    /// Jetons de réponse : notes d'une partie, document.
    pub notes_tokens: u32,
    pub document_tokens: u32,
}

/// Contexte supposé d'un modèle distant qui ne l'annonce pas.
const UNKNOWN_CONTEXT: u32 = 8_192;
/// Caractères par jeton supposés sans compte du modèle (par défaut, comme
/// l'estimation du compactage du chat).
const CHARS_PER_TOKEN: f64 = 3.0;
/// Marge de l'estimation comptée par le modèle (un extrait n'est pas tout
/// le texte).
const CALIBRATION_MARGIN: f64 = 0.85;
/// Extrait compté par le modèle (caractères).
const CALIBRATION_SAMPLE: usize = 4_000;
/// Partie la plus longue : un petit modèle résume mieux un passage court
/// (environ dix minutes de parole).
const MAX_PART_CHARS: usize = 8_000;
/// Transcription écrite d'un coup, ou notes données au document, au plus.
const MAX_SOURCE_CHARS: usize = 24_000;
/// En dessous, le contexte est trop petit pour une synthèse utile.
const MIN_PART_CHARS: usize = 1_500;
const MIN_SOURCE_CHARS: usize = 1_200;
/// Gabarit du modèle et marge (jetons).
const MARGIN_TOKENS: u32 = 64;
/// Niveaux de regroupement des notes au plus.
const MAX_MERGES: usize = 4;

/// Budget pour un contexte de `context` jetons, `chars_per_token`
/// caractères par jeton et une consigne de `prompt_chars` caractères ;
/// `None` : contexte trop petit.
pub(crate) fn budget(context: u32, chars_per_token: f64, prompt_chars: usize) -> Option<Budget> {
    let chars_per_token = chars_per_token.clamp(0.5, 6.0);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let prompt_tokens = (prompt_chars as f64 / chars_per_token).ceil() as u32 + MARGIN_TOKENS;
    let notes_tokens = (context / 6).clamp(256, 1_024);
    let document_tokens = (context / 3).clamp(512, 4_096);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let chars = |tokens: u32| (f64::from(tokens) * chars_per_token) as usize;
    let part_chars =
        chars(context.saturating_sub(prompt_tokens + notes_tokens)).min(MAX_PART_CHARS);
    let source_chars =
        chars(context.saturating_sub(prompt_tokens + document_tokens)).min(MAX_SOURCE_CHARS);
    (part_chars >= MIN_PART_CHARS && source_chars >= MIN_SOURCE_CHARS).then_some(Budget {
        part_chars,
        source_chars,
        notes_tokens,
        document_tokens,
    })
}

/// Découpe `text` (paragraphes séparés par une ligne vide) en parties d'au
/// plus `max_chars` caractères, de tailles voisines, coupées entre deux
/// paragraphes, sinon entre deux phrases, sinon entre deux mots.
pub(crate) fn split_parts(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let len = |text: &str| text.chars().count();
    let mut pieces: Vec<String> = Vec::new();
    for paragraph in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        pieces.extend(cut(paragraph, max_chars));
    }
    let total: usize = pieces.iter().map(|piece| len(piece) + 2).sum();
    let count = total.div_ceil(max_chars).max(1);
    let target = total.div_ceil(count).min(max_chars);
    let mut parts = Vec::new();
    let mut current = String::new();
    for piece in pieces {
        if !current.is_empty() && len(&current) + 2 + len(&piece) > target {
            parts.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(&piece);
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// Paragraphe coupé en morceaux d'au plus `max_chars` : entre deux
/// phrases, sinon entre deux mots, sinon au caractère.
fn cut(paragraph: &str, max_chars: usize) -> Vec<String> {
    let len = |text: &str| text.chars().count();
    if len(paragraph) <= max_chars {
        return vec![paragraph.to_owned()];
    }
    // Phrases (ponctuation finale gardée), puis mots d'une phrase trop longue.
    let mut units: Vec<String> = Vec::new();
    let mut sentence = String::new();
    let mut chars = paragraph.chars().peekable();
    while let Some(c) = chars.next() {
        sentence.push(c);
        if matches!(c, '.' | '?' | '!' | '…' | '。' | '？' | '！')
            && chars.peek().is_none_or(|next| next.is_whitespace())
        {
            units.push(std::mem::take(&mut sentence).trim().to_owned());
        }
    }
    if !sentence.trim().is_empty() {
        units.push(sentence.trim().to_owned());
    }
    let mut words: Vec<String> = Vec::new();
    for unit in units.into_iter().filter(|unit| !unit.is_empty()) {
        if len(&unit) <= max_chars {
            words.push(unit);
            continue;
        }
        for word in unit.split_whitespace() {
            if len(word) <= max_chars {
                words.push(word.to_owned());
            } else {
                let chars: Vec<char> = word.chars().collect();
                words.extend(chars.chunks(max_chars).map(|chunk| chunk.iter().collect()));
            }
        }
    }
    let mut pieces = Vec::new();
    let mut current = String::new();
    for word in words {
        if !current.is_empty() && len(&current) + 1 + len(&word) > max_chars {
            pieces.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&word);
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

/// Notes regroupées en paquets qui tiennent dans `max_chars` (au moins deux
/// notes par paquet quand il y en a plusieurs : chaque niveau en réduit le
/// nombre).
pub(crate) fn groups(notes: &[String], max_chars: usize) -> Vec<Vec<String>> {
    let len = |text: &str| text.chars().count() + 1;
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut size = 0;
    for note in notes {
        let fits = groups
            .last()
            .is_some_and(|group| group.len() < 2 || size + len(note) <= max_chars);
        if fits {
            size += len(note);
            groups.last_mut().expect("checked").push(note.clone());
        } else {
            size = len(note);
            groups.push(vec![note.clone()]);
        }
    }
    groups
}

/// Texte d'une réponse à montrer : sans réflexion écrite dans le contenu
/// (`<think>…</think>`, rien tant qu'elle n'est pas finie), sans clôture de
/// code autour du document ; `finished` : fin de la réponse (clôture finale
/// retirée, blancs aux bords).
pub(crate) fn visible(raw: &str, finished: bool) -> String {
    let mut text = raw.trim_start();
    if text.starts_with("<think>") {
        match text.rfind("</think>") {
            Some(end) => text = text[end + "</think>".len()..].trim_start(),
            None => return String::new(),
        }
    } else if let Some(end) = text.rfind("</think>") {
        // Réflexion ouverte par le gabarit, seule la fin écrite.
        text = text[end + "</think>".len()..].trim_start();
    }
    if text.starts_with("```") {
        match text.find('\n') {
            Some(line) => text = &text[line + 1..],
            None => return String::new(),
        }
        let trimmed = text.trim_end();
        if let Some(body) = trimmed.strip_suffix("```") {
            text = body;
        } else if finished {
            text = trimmed;
        }
    }
    if finished {
        text.trim().to_owned()
    } else {
        text.to_owned()
    }
}

/// Erreur du moteur pour la page ; `None` : un arrêt (annulation).
fn engine_failure(model: &str, error: &LlmError, cancel: &CancellationToken) -> Stop {
    let Some(kind) = error.chat_kind() else {
        return if cancel.is_cancelled() {
            Stop::Cancelled
        } else {
            // Modèle déchargé à la demande (bouton, suppression) pendant la
            // synthèse.
            Stop::failed(SynthesisErrorCode::EngineFailed, t!("synthesis.unloaded"))
        };
    };
    match kind {
        ChatErrorKind::ModelMissing => Stop::failed(
            SynthesisErrorCode::ModelNotFound,
            t!("synthesis.modelNotFound", model = model),
        ),
        ChatErrorKind::EngineUnavailable | ChatErrorKind::EngineFailed => {
            Stop::failed(SynthesisErrorCode::EngineFailed, error.message())
        }
        ChatErrorKind::ConnectionLost => Stop::failed(
            SynthesisErrorCode::EngineFailed,
            t!("synthesis.connectionLost"),
        ),
        ChatErrorKind::ContextFull => Stop::failed(
            SynthesisErrorCode::ContextTooSmall,
            t!("synthesis.contextTooSmall"),
        ),
        _ => Stop::failed(
            SynthesisErrorCode::Failed,
            t!("synthesis.failed", detail = error.message()),
        ),
    }
}

/// Caractères par jeton de la transcription, comptés par le modèle chargé
/// sur un extrait (marge comprise) ; sans compte (distant, échec) :
/// [`CHARS_PER_TOKEN`].
#[allow(clippy::cast_precision_loss)]
async fn chars_per_token(session: &Session, text: &str) -> f64 {
    if session.remote {
        return CHARS_PER_TOKEN;
    }
    let sample: String = text.chars().take(CALIBRATION_SAMPLE).collect();
    let chars = sample.chars().count();
    match inference::tokenize(&session.endpoint, &sample).await {
        Ok(tokens) if !tokens.is_empty() && chars >= 200 => {
            (chars as f64 / tokens.len() as f64 * CALIBRATION_MARGIN).clamp(0.8, 4.0)
        }
        Ok(_) => CHARS_PER_TOKEN,
        Err(error) => {
            crate::app_log!("chatonlm: synthesis: tokenize: {error}");
            CHARS_PER_TOKEN
        }
    }
}

/// Une synthèse : modèle ouvert, puis document (d'un coup, ou d'après les
/// notes des parties) ; le document fini, nettoyé.
async fn synthesize(
    access: &Access,
    job: &Job,
    cancel: &CancellationToken,
    timing: Timing,
    show: &(dyn Fn(&str) + Send + Sync),
) -> Result<String, Stop> {
    let details = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(Stop::Cancelled),
        details = llm::models::details(access, &job.model) => details,
    };
    let details = match details {
        Ok(details) => details,
        Err(AppError::NotFound(_)) => {
            return Err(Stop::failed(
                SynthesisErrorCode::ModelNotFound,
                t!("synthesis.modelNotFound", model = job.model),
            ));
        }
        Err(error) => {
            return Err(Stop::failed(
                SynthesisErrorCode::Failed,
                t!("synthesis.failed", detail = error),
            ));
        }
    };
    let think_off = crate::dictation::polish::think_off_on(Some(&details), false);
    let session = access
        .open(&job.model, OpenOptions::default(), cancel, &|| {})
        .await
        .map_err(|error| engine_failure(&job.model, &error, cancel))?;
    // L'autocomplétion de l'éditeur attend pendant ce temps.
    let _working = crate::gpu::activity::hold();
    let ratio = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(Stop::Cancelled),
        ratio = chars_per_token(&session, &job.text) => ratio,
    };
    let context = session.context.unwrap_or(UNKNOWN_CONTEXT);
    let Some(budget) = budget(context, ratio, prompts::longest_prompt_chars(&job.title)) else {
        return Err(Stop::failed(
            SynthesisErrorCode::ContextTooSmall,
            t!("synthesis.contextTooSmall"),
        ));
    };
    let asker = Asker {
        session: &session,
        model: &job.model,
        think_off,
        cancel,
        timing,
    };
    let text = job.text.trim();
    if text.chars().count() <= budget.source_chars {
        return document(&asker, job, From::Transcript(text), budget, show).await;
    }
    let parts = split_parts(text, budget.part_chars);
    let mut notes = Vec::with_capacity(parts.len());
    for (index, part) in parts.iter().enumerate() {
        let messages = prompts::notes(job.language, &job.title, index + 1, parts.len(), part);
        let answer = asker.ask(messages, budget.notes_tokens, &|_| {}).await?;
        let note = visible(&answer, true);
        if !note.is_empty() {
            notes.push(note);
        }
    }
    if notes.is_empty() {
        return Err(Stop::failed(
            SynthesisErrorCode::Empty,
            t!("synthesis.empty"),
        ));
    }
    // Notes trop longues pour le document : regroupées par paquets.
    let mut level = 0;
    while joined(&notes).chars().count() > budget.source_chars && notes.len() > 1 {
        level += 1;
        if level > MAX_MERGES {
            break;
        }
        let mut merged = Vec::new();
        for group in groups(&notes, budget.source_chars) {
            if group.len() == 1 {
                merged.extend(group);
                continue;
            }
            let messages = prompts::merge(job.language, &joined(&group));
            let answer = asker.ask(messages, budget.notes_tokens, &|_| {}).await?;
            let note = visible(&answer, true);
            if !note.is_empty() {
                merged.push(note);
            }
        }
        notes = merged;
    }
    let notes = joined(&notes);
    // Au-delà de ce que tient la demande (regroupement sans effet) : fin
    // retirée plutôt qu'un refus du serveur.
    let notes: String = notes.chars().take(budget.source_chars).collect();
    document(&asker, job, From::Notes(&notes), budget, show).await
}

fn joined(notes: &[String]) -> String {
    notes.join("\n")
}

/// Document final, montré en flux ; redemandé une fois s'il est écrit dans
/// une autre langue que la transcription.
async fn document(
    asker: &Asker<'_>,
    job: &Job,
    from: From<'_>,
    budget: Budget,
    show: &(dyn Fn(&str) + Send + Sync),
) -> Result<String, Stop> {
    let mut retry = false;
    loop {
        let messages = prompts::document(job.language, &job.title, from, retry);
        let stream_show = |raw: &str| show(&visible(raw, false));
        let answer = asker
            .ask(messages, budget.document_tokens, &stream_show)
            .await?;
        let markdown = visible(&answer, true);
        if markdown.is_empty() {
            return Err(Stop::failed(
                SynthesisErrorCode::Empty,
                t!("synthesis.empty"),
            ));
        }
        let stray = job.guard.is_some_and(|expected| {
            language::detect(&markdown).is_some_and(|found| found != expected)
        });
        if stray && !retry {
            crate::app_log!("chatonlm: synthesis: document in another language, asked again");
            retry = true;
            show("");
            continue;
        }
        return Ok(markdown);
    }
}

/// Requêtes d'une synthèse sur le modèle ouvert.
struct Asker<'a> {
    session: &'a Session,
    model: &'a str,
    think_off: bool,
    cancel: &'a CancellationToken,
    timing: Timing,
}

/// Température des réponses (fidélité plutôt qu'invention).
const TEMPERATURE: f32 = 0.3;

impl Asker<'_> {
    /// Réponse entière (contenu brut) ; chaque morceau reçu, le contenu
    /// jusque-là passé à `progress`. Réflexion à part ignorée.
    async fn ask(
        &self,
        messages: Vec<Message>,
        max_tokens: u32,
        progress: &(dyn Fn(&str) + Send + Sync),
    ) -> Result<String, Stop> {
        let mut request = self.session.request(messages);
        request.temperature = Some(TEMPERATURE);
        request.max_tokens = Some(max_tokens);
        if self.think_off {
            request.think = Think::Off;
        }
        let stalled = || {
            Stop::failed(
                SynthesisErrorCode::Failed,
                t!("synthesis.failed", detail = t!("synthesis.timedOut")),
            )
        };
        let opened = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Err(Stop::Cancelled),
            () = tokio::time::sleep(self.timing.stall) => return Err(stalled()),
            opened = llm::chat_stream(self.session, &request, self.cancel.clone()) => opened,
        };
        let mut stream = opened.map_err(|error| engine_failure(self.model, &error, self.cancel))?;
        let mut answer = String::new();
        loop {
            let next = tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Err(Stop::Cancelled),
                () = tokio::time::sleep(self.timing.stall) => return Err(stalled()),
                next = stream.next() => next,
            };
            match next {
                Some(Ok(Event::Content(content))) => {
                    answer.push_str(&content);
                    progress(&answer);
                }
                Some(Ok(Event::Done { .. })) => return Ok(answer),
                // Réflexion à part, appels d'outils (aucun offert) : ignorés.
                Some(Ok(_)) => {}
                Some(Err(error)) => {
                    let error = self.session.error(error);
                    return Err(engine_failure(self.model, &error, self.cancel));
                }
                None => {
                    return Err(Stop::failed(
                        SynthesisErrorCode::EngineFailed,
                        t!("synthesis.connectionLost"),
                    ));
                }
            }
        }
    }
}
