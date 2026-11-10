//! Requêtes des cours au moteur de l'app (`llm::Access`) : modèle réglé,
//! détails (raisonnement coupé si possible), modèle ouvert pour tout le
//! travail (sa `Session` le tient en mémoire), parties en JSON contraint
//! (réponse entière, `response_format`), réponses en flux (Markdown).
//! Erreurs : modèle absent → `modelNotFound`, moteur en panne →
//! `engineFailed` (texte du moteur), autre → `failed` avec la raison ; un
//! modèle déchargé à la demande pendant le travail est un arrêt.

use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::prompts;
use super::repair;
use super::{StudyError, StudyErrorCode, model_of};
use crate::chat::stream::Think;
use crate::chat::{ChatErrorKind, Server};
use crate::engine::{ModelState, RunningModel};
use crate::i18n::t;
use crate::inference::{self, Event, InferenceError, Message};
use crate::llm::{Access, LlmError, OpenOptions, Session};
use crate::local_models::id::same_model;
use crate::proofread::Cancel;

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
    /// Erreur 5xx du serveur, délai dépassé, réponse coupée : à retenter.
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

/// Fin anticipée d'une génération.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Arrêt demandé (cours supprimé, palier refait, modèle déchargé…) :
    /// rien à enregistrer.
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

/// Erreur du moteur pour un cours, et si elle est passagère.
fn failure(cause: &LlmError, model: &str) -> Failure {
    let transient = matches!(
        cause,
        LlmError::Inference(
            InferenceError::Timeout
                | InferenceError::Loading
                | InferenceError::Interrupted(_)
                | InferenceError::Http { status: 500.., .. }
        )
    );
    let stop = match cause.chat_kind() {
        None => Stop::Cancelled,
        Some(ChatErrorKind::ModelMissing) => Stop::Failed(error(
            StudyErrorCode::ModelNotFound,
            t!("study.modelNotFound", model = model),
        )),
        Some(ChatErrorKind::EngineUnavailable | ChatErrorKind::EngineFailed) => {
            Stop::Failed(error(StudyErrorCode::EngineFailed, cause.message()))
        }
        // Refus d'un modèle distant : dit tel quel (réglage, clé, limite).
        Some(
            ChatErrorKind::RemoteOff
            | ChatErrorKind::RemoteKey
            | ChatErrorKind::RemoteLimit
            | ChatErrorKind::RemoteUnreachable,
        ) => Stop::Failed(error(StudyErrorCode::Failed, cause.message())),
        Some(_) => failed(&cause.message()),
    };
    Failure { stop, transient }
}

/// Contexte demandé pour les cours : au moins [`prompts::STUDY_CONTEXT`]
/// (et le réglage s'il est plus grand) ; si le modèle est déjà prêt avec au
/// moins autant (chargé par le chat), ce contexte-là, pour ne pas le
/// relancer.
pub fn study_context(setting: u32, running: &[RunningModel], model: &str) -> u32 {
    let wanted = setting.max(prompts::STUDY_CONTEXT);
    running
        .iter()
        .filter(|running| {
            same_model(&running.id, model)
                && running.state == ModelState::Ready
                && (!running.cpu_only || running.cpu_fallback)
        })
        .map(|running| running.context_length)
        .filter(|ctx| *ctx >= wanted)
        .max()
        .unwrap_or(wanted)
}

/// Connexion d'un travail au moteur, avec le modèle des cours.
pub struct Gen<'a> {
    /// Réglages et accès du moment (outils web du plan).
    pub server: Server,
    pub model: String,
    /// Raisonnement coupé si le modèle sait le couper.
    pub think: inference::Think,
    pub cancel: &'a Cancel,
    pub timeouts: Timeouts,
    /// Modèle ouvert pour tout le travail.
    session: Session,
    /// Travail tenu sur le moteur tant que la connexion vit :
    /// l'autocomplétion de l'éditeur attend (voir `gpu::activity`).
    _working: Option<crate::gpu::activity::Hold>,
}

impl<'a> Gen<'a> {
    /// Réglages lus au moment du travail (le modèle réglé peut avoir changé),
    /// modèle installé, ouvert (chargé au besoin) par le moteur.
    pub async fn connect(
        access: &Access,
        cancel: &'a Cancel,
        timeouts: Timeouts,
    ) -> Result<Self, Stop> {
        let settings = access
            .store()
            .call(|c| crate::store::settings::get(c))
            .await
            .map_err(|error| failed(&error))?;
        let Some(model) = model_of(access, &settings).await else {
            return Err(Stop::Failed(error(
                StudyErrorCode::NoModel,
                t!("study.noModel"),
            )));
        };
        let server = Server::with_settings(access.clone(), &settings);
        // Modèle distant, réglage coupé : refusé sans rien demander.
        if crate::llm::ids::is_remote(&model) && !server.prefs.remote_models {
            return Err(failure(&LlmError::RemoteOff, &model).stop);
        }
        let details = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(Stop::Cancelled),
            details = crate::llm::models::details(access, &model) => details,
        };
        let details = match details {
            Ok(details) => details,
            Err(crate::error::AppError::NotFound(_)) => {
                return Err(Stop::Failed(error(
                    StudyErrorCode::ModelNotFound,
                    t!("study.modelNotFound", model = model),
                )));
            }
            Err(other) => return Err(failed(&other)),
        };
        // Raisonnement coupé seulement si le modèle l'accepte et ne
        // raisonne pas quand même (vu par ChatonLM).
        let think = if !details.always_thinks
            && crate::chat::call::accepted_think(Some(Think::Enabled(false)), &details).is_some()
        {
            inference::Think::Off
        } else {
            inference::Think::Default
        };
        let options = OpenOptions {
            ctx: Some(study_context(
                server.prefs.context_length,
                &access.engine().running(),
                &model,
            )),
            ..OpenOptions::default()
        };
        // Annulé par `cancel` (ce futur est alors abandonné ; le chargement
        // continue dans le moteur).
        let never = CancellationToken::new();
        let opened = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(Stop::Cancelled),
            opened = access.open(&model, options, &never, &|| {}) => opened,
        };
        let session = opened.map_err(|error| failure(&error, &model).stop)?;
        Ok(Self {
            server,
            model,
            think,
            cancel,
            timeouts,
            session,
            _working: crate::gpu::activity::hold(),
        })
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
        let answer = self.ask_raw(system, user, schema, temperature).await?;
        // Raisonnement rendu à part (`reasoning_content`) : ignoré ; écrit
        // dans le contenu : retiré par `parse_object`.
        let content = &answer.content;
        // Un texte à sa limite : emballement, réponse inutilisable (même
        // chemin qu'un JSON illisible).
        let parsed = repair::parse_object(content)
            .filter(|value| !super::checks::runaway(value, &schema.value()));
        if parsed.is_none() {
            // i18n-ignore : journal.
            crate::app_log!(
                "chatonlm: study: unreadable answer from {} (finish {:?}, {} characters)",
                self.model,
                answer.finish_reason,
                content.chars().count()
            );
        }
        Ok(parsed)
    }

    /// Réponse entière du modèle à une requête en JSON contraint.
    pub async fn ask_raw(
        &self,
        system: &str,
        user: &str,
        schema: &prompts::Schema,
        temperature: f64,
    ) -> Result<inference::ChatResponse, Stop> {
        let request = prompts::json_request(
            self.session.request(prompts::messages(system, user)),
            self.think,
            schema,
            temperature,
        );
        match self.ask_once(&request).await {
            // Erreur passagère (5xx, délai, réponse coupée) : une nouvelle
            // tentative après une courte pause.
            Err(Failure {
                transient: true,
                stop,
            }) => {
                // i18n-ignore : journal.
                crate::app_log!("chatonlm: study: {stop:?}, trying again once");
                tokio::select! {
                    biased;
                    () = self.cancel.cancelled() => return Err(Stop::Cancelled),
                    () = tokio::time::sleep(self.timeouts.retry_delay) => {}
                }
                self.ask_once(&request)
                    .await
                    .map_err(|failure| failure.stop)
            }
            result => result.map_err(|failure| failure.stop),
        }
    }

    /// Une requête en JSON contraint et sa réponse entière.
    async fn ask_once(
        &self,
        request: &inference::ChatRequest,
    ) -> Result<inference::ChatResponse, Failure> {
        let never = CancellationToken::new();
        let asked = tokio::time::timeout(
            self.timeouts.json,
            inference::chat_once(&self.session.endpoint, request, never),
        );
        let answer = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Err(Failure::lasting(Stop::Cancelled)),
            answer = asked => answer,
        };
        match answer {
            Err(_) => Err(Failure {
                stop: failed(&t!("study.timedOut")),
                transient: true,
            }),
            Ok(Err(error)) => Err(failure(&self.session.error(error), &self.model)),
            Ok(Ok(answer)) => Ok(answer),
        }
    }

    /// Réponse en flux (`messages` complets) : fragments visibles envoyés à
    /// `on_delta` ; texte complet (raisonnement retiré) à la fin.
    pub async fn stream(
        &self,
        messages: Vec<Message>,
        on_delta: &(dyn Fn(String) + Send + Sync),
    ) -> Result<String, Stop> {
        let request = prompts::stream_request(self.session.request(messages), self.think);
        // Fermé (requête lâchée) quand le flux est abandonné.
        let token = CancellationToken::new();
        let _close = token.clone().drop_guard();
        // Le premier fragment (en-têtes compris) arrive lui aussi avant le
        // délai.
        let opened = tokio::time::timeout(
            self.timeouts.stream_idle,
            crate::llm::chat_stream(&self.session, &request, token.clone()),
        );
        let opened = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Err(Stop::Cancelled),
            opened = opened => opened,
        };
        let mut stream = match opened {
            Err(_) => return Err(failed(&t!("study.timedOut"))),
            Ok(Err(error)) => return Err(failure(&error, &self.model).stop),
            Ok(Ok(stream)) => stream,
        };
        let mut visible = Visible::default();
        // Formules `$…$` écrites en `\(…\)` au fil du flux (voir
        // `repair::inline_math`).
        let mut math = repair::MathStream::default();
        loop {
            let next = tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Err(Stop::Cancelled),
                next = tokio::time::timeout(self.timeouts.stream_idle, stream.next()) => next,
            };
            let event = match next {
                Err(_) => return Err(failed(&t!("study.timedOut"))),
                Ok(None) => return Err(failed(&t!("study.connectionLost"))),
                Ok(Some(Err(error))) => {
                    return Err(failure(&self.session.error(error), &self.model).stop);
                }
                Ok(Some(Ok(event))) => event,
            };
            let (content, done) = match event {
                Event::Content(content) => (content, false),
                // Raisonnement à part : jamais montré.
                Event::Reasoning(_) | Event::ToolCalls(_) => continue,
                Event::Done { .. } => (String::new(), true),
            };
            let shown = math.push(&visible.push(&content));
            if !shown.is_empty() {
                on_delta(shown);
            }
            // Trop long : réponse arrêtée là (requête lâchée).
            if done || visible.len() > self.timeouts.stream_bytes {
                let text = repair::inline_math(&visible.text());
                let rest = math.finish(&text);
                if !rest.is_empty() {
                    on_delta(rest);
                }
                return Ok(text);
            }
        }
    }
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

    fn running(id: &str, state: ModelState, ctx: u32, cpu_only: bool) -> RunningModel {
        RunningModel {
            id: id.into(),
            state,
            size_bytes: 1,
            vram_bytes: 0,
            context_length: ctx,
            expires_at: None,
            kept: false,
            in_use: false,
            gpu_layers: None,
            total_layers: None,
            cpu_fallback: false,
            notice: None,
            cpu_only,
            params_label: None,
        }
    }

    /// Au moins le contexte des cours (et le réglage s'il est plus grand) ;
    /// un modèle déjà prêt avec au moins autant garde le sien (pas de
    /// relance), pas un modèle en chargement, ni mis sur le processeur à la
    /// demande.
    #[test]
    fn courses_take_at_least_their_context_without_relaunching() {
        let model = "a/b-GGUF:Q4_K_M";
        assert_eq!(study_context(0, &[], model), prompts::STUDY_CONTEXT);
        assert_eq!(study_context(16_384, &[], model), 16_384);
        let ready = [running("a/b-GGUF:q4_k_m", ModelState::Ready, 32_768, false)];
        assert_eq!(study_context(0, &ready, model), 32_768);
        assert_eq!(study_context(0, &ready, "a/other-GGUF:Q4_K_M"), 8192);
        let small = [running(model, ModelState::Ready, 4096, false)];
        assert_eq!(study_context(0, &small, model), 8192);
        assert_eq!(study_context(40_000, &ready, model), 40_000);
        let loading = [running(model, ModelState::Loading, 32_768, false)];
        assert_eq!(study_context(0, &loading, model), 8192);
        let cpu = [running(model, ModelState::Ready, 32_768, true)];
        assert_eq!(study_context(0, &cpu, model), 8192);
        let mut fallback = running(model, ModelState::Ready, 32_768, true);
        fallback.cpu_fallback = true;
        assert_eq!(study_context(0, &[fallback], model), 32_768);
    }

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
