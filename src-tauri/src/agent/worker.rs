//! [`Worker`] : un modèle ouvert pour un travail de l'Atelier.
//!
//! Patron de `study::request::Gen`, généralisé : n'importe quel modèle
//! (pas celui réglé pour les cours), contexte minimal donné par l'appelant,
//! réflexion choisie à chaque requête ([`ThinkMode`]), réponses lues en flux
//! (silence borné, détecteur de boucle), nouvel essai borné à un seul.

use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::AgentError;
use super::answer::Answer;
use super::tools::{MAX_TOOL_CALLS, Tool, too_many_calls, tool_message};
use crate::chat::repetition::LoopWatch;
use crate::chat::stream::Think as PageThink;
use crate::engine::{EngineError, ModelState, RunningModel};
use crate::inference::{
    self, ChatRequest, Content, Event, FinishReason, InferenceError, Message, Part, ResponseFormat,
    ToolCall, Usage,
};
use crate::llm::models::ModelDetails;
use crate::llm::{Access, LlmError, OpenOptions, Session};
use crate::local_models::id::same_model;
use crate::study::repair;

/// Jetons de réponse prévus en plus du budget de réflexion, par défaut
/// (lot 0 : un plan du 27B écrit 7 000 à 9 700 jetons au total).
pub(crate) const ANSWER_TOKENS: u32 = 10_000;

/// Budget de réflexion d'un modèle qui ne sait pas la couper quand
/// l'appelant la veut coupée ([`ThinkMode::Off`]) : bornée au plus court.
pub(crate) const OFF_BUDGET: u32 = 1_024;

/// Pause avant un nouvel essai après une erreur passagère.
const RETRY_DELAY: Duration = Duration::from_secs(2);

/// Caractères par jeton, estimés (comme `chat/compact.rs`) : la place des
/// résultats d'outils se compte sans demander `/tokenize` à chaque tour.
const CHARS_PER_TOKEN: usize = 3;

/// Jetons comptés pour une image jointe (estimation large).
const IMAGE_TOKENS: usize = 1_024;

/// Jetons de `messages`, estimés à [`CHARS_PER_TOKEN`] caractères par jeton
/// (texte, réflexion renvoyée, appels d'outils ; images à forfait).
fn estimated_tokens(messages: &[Message]) -> usize {
    let mut chars = 0;
    let mut images = 0;
    for message in messages {
        match &message.content {
            Content::Text(text) => chars += text.chars().count(),
            Content::Parts(parts) => {
                for part in parts {
                    match part {
                        Part::Text(text) => chars += text.chars().count(),
                        Part::ImageUrl(_) => images += 1,
                    }
                }
            }
        }
        chars += message
            .reasoning_content
            .as_deref()
            .map_or(0, |text| text.chars().count());
        for call in &message.tool_calls {
            chars += call.name.chars().count() + call.arguments.chars().count();
        }
    }
    chars.div_ceil(CHARS_PER_TOKEN) + images * IMAGE_TOKENS
}

/// Caractères laissés aux résultats d'outils d'un tour : la moitié de la
/// place restante dans un contexte de `context` jetons après `messages`
/// (l'autre moitié reste à la réflexion et à la réponse qui suivent).
fn results_room(context: u32, messages: &[Message]) -> usize {
    let context = usize::try_from(context).unwrap_or(usize::MAX);
    context.saturating_sub(estimated_tokens(messages)) / 2 * CHARS_PER_TOKEN
}

/// Contexte d'un travail : `wanted` ; si le modèle est déjà prêt avec au
/// moins autant (chargé par le chat, ou un travail précédent), ce
/// contexte-là, pour ne pas le relancer. Un modèle en chargement, ou mis sur
/// le processeur à la demande (pas repli), n'est pas repris.
pub(crate) fn context_at_least(wanted: u32, running: &[RunningModel], model: &str) -> u32 {
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

/// Options d'une ouverture.
#[derive(Clone, Debug, Default)]
pub(crate) struct WorkerOptions {
    /// Contexte demandé (borné par le moteur au contexte d'entraînement du
    /// modèle). Le réglage `context_length` du chat n'y entre pas : le choix
    /// du modèle (`engine::auto::largest_fitting`) l'a estimé à ce contexte.
    pub ctx_min: u32,
    /// Arrêt du travail : ouverture et toutes les requêtes.
    pub cancel: CancellationToken,
}

/// Réflexion demandée pour une requête.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThinkMode {
    /// Coupée si le modèle sait la couper ; sinon bornée à [`OFF_BUDGET`].
    Off,
    /// Demandée, bornée à `budget` jetons (obligatoire, voir le module).
    On { budget: u32 },
}

/// Requête en JSON contraint ([`Worker::ask_json`]).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Call {
    pub system: String,
    pub user: String,
    /// Schéma JSON de la réponse (grammaire du serveur).
    pub schema: Value,
    pub temperature: f32,
    pub think: ThinkMode,
    /// `None` : budget de réflexion + [`ANSWER_TOKENS`].
    pub max_tokens: Option<u32>,
}

/// Réglages d'une réponse en texte ([`Worker::write`], [`Worker::run_tools`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Write {
    pub temperature: f32,
    pub think: ThinkMode,
    /// `None` : budget de réflexion + [`ANSWER_TOKENS`].
    pub max_tokens: Option<u32>,
}

/// Réponse en JSON lue et réparée.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Json {
    pub value: Value,
    /// Jetons, toutes tentatives comprises.
    pub usage: Usage,
    /// Caractères de réflexion, toutes tentatives comprises.
    pub reasoning_chars: usize,
    pub elapsed: Duration,
}

/// Réponse en texte, sans réflexion.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Text {
    pub content: String,
    /// Caractères de réflexion, tentatives et tours d'outils compris.
    pub reasoning_chars: usize,
    /// Jetons, tentatives et tours d'outils compris.
    pub usage: Usage,
    /// Raison de la fin de la dernière réponse.
    pub finish: FinishReason,
}

/// Ce que reçoit l'appelant d'une réponse en flux.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Delta {
    /// Texte visible nouveau (jamais de réflexion).
    Text(String),
    /// La réponse repart de zéro (nouvel essai) : oublier le texte reçu.
    Restart,
}

/// Rappel des fragments d'une réponse en flux.
type Sink<'a> = Option<&'a (dyn Fn(Delta) + Send + Sync)>;

/// Ce que le modèle sait de la réflexion (détails du moteur).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Thinking {
    /// Il réfléchit (capacité, ou vu réfléchir réflexion coupée).
    thinks: bool,
    /// Il accepte `enable_thinking: false` et ne réfléchit pas quand même.
    can_stop: bool,
    /// Il accepte `enable_thinking: true` (sinon : choix du gabarit).
    can_start: bool,
}

impl Thinking {
    fn of(details: &ModelDetails) -> Thinking {
        let accepts =
            |on| crate::chat::call::accepted_think(Some(PageThink::Enabled(on)), details).is_some();
        Thinking {
            thinks: details.always_thinks || details.capabilities.iter().any(|w| w == "thinking"),
            can_stop: !details.always_thinks && accepts(false),
            can_start: accepts(true),
        }
    }

    /// Réflexion et budget envoyés pour `mode`. Le budget part dès que la
    /// réflexion est demandée, même si les détails ne disent pas que le
    /// modèle réfléchit (gabarit mal décrit : sans budget, il réfléchirait
    /// sans borne) ; sans effet sur un modèle qui ne réfléchit pas, et retiré
    /// pour un modèle distant (`inference::dialect`).
    fn request(self, mode: ThinkMode) -> (inference::Think, Option<u32>) {
        if !self.thinks {
            return match mode {
                ThinkMode::Off => (inference::Think::Default, None),
                ThinkMode::On { budget } => (inference::Think::Default, Some(budget)),
            };
        }
        match mode {
            ThinkMode::Off if self.can_stop => (inference::Think::Off, None),
            ThinkMode::Off => (inference::Think::Default, Some(OFF_BUDGET)),
            ThinkMode::On { budget } => (
                if self.can_start {
                    inference::Think::On
                } else {
                    inference::Think::Default
                },
                Some(budget),
            ),
        }
    }
}

/// Une réponse lue en flux.
struct Reply {
    /// Contenu tel que reçu (raisonnement écrit dedans compris).
    raw: String,
    /// Contenu sans raisonnement.
    visible: String,
    /// Raisonnement rendu à part (`reasoning_content`).
    reasoning: String,
    tool_calls: Vec<ToolCall>,
    finish: FinishReason,
    usage: Usage,
}

impl Reply {
    /// Caractères de réflexion : à part, et écrite dans le contenu.
    fn reasoning_chars(&self) -> usize {
        let written = if self.raw.contains("<think>") || self.raw.contains("</think>") {
            self.raw
                .chars()
                .count()
                .saturating_sub(self.visible.chars().count())
        } else {
            0
        };
        self.reasoning.chars().count() + written
    }
}

fn add(total: &mut Usage, usage: Usage) {
    total.prompt = total.prompt.saturating_add(usage.prompt);
    total.completion = total.completion.saturating_add(usage.completion);
    total.cached = total.cached.saturating_add(usage.cached);
}

/// Base illisible : le moteur ne peut pas servir (comme `llm::Access::open`).
fn store_error(error: crate::error::AppError) -> AgentError {
    AgentError::Model(LlmError::Engine(EngineError::StartFailed(
        error.to_string(),
    )))
}

/// Un modèle ouvert pour un travail : sa session le tient en mémoire (ni
/// évincé, ni arrêté par son minuteur) jusqu'à [`Worker::release`].
pub(crate) struct Worker {
    session: Session,
    thinking: Thinking,
    /// Le gabarit parle d'outils (capacité « tools »).
    tools: bool,
    context: u32,
    cancel: CancellationToken,
    retry_delay: Duration,
    /// L'autocomplétion de l'éditeur attend tant que le travail vit.
    _working: Option<crate::gpu::activity::Hold>,
}

impl std::fmt::Debug for Worker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Worker")
            .field("model", &self.session.id)
            .field("context", &self.context)
            .finish_non_exhaustive()
    }
}

impl Worker {
    /// Ouvre `model_id` (chargé au besoin par le moteur) : refusé sans rien
    /// demander si c'est un modèle distant et que le réglage les coupe ;
    /// détails lus (réflexion, outils) ; contexte = `ctx_min` (jamais le
    /// réglage du chat : le modèle a été choisi pour tenir à ce contexte), ou
    /// celui d'une instance déjà prête plus grande (pas de relance).
    pub(crate) async fn open(
        access: &Access,
        model_id: &str,
        options: WorkerOptions,
    ) -> Result<Worker, AgentError> {
        let model = model_id.trim().to_owned();
        let prefs = access.prefs().await.map_err(store_error)?;
        if crate::llm::ids::is_remote(&model) && !prefs.remote_models {
            return Err(AgentError::RemoteOff);
        }
        let details = tokio::select! {
            biased;
            () = options.cancel.cancelled() => return Err(AgentError::Cancelled),
            details = crate::llm::models::details(access, &model) => details,
        };
        let details = match details {
            Ok(details) => details,
            Err(crate::error::AppError::NotFound(_)) => {
                return Err(AgentError::Model(LlmError::NotInstalled(model)));
            }
            Err(other) => return Err(store_error(other)),
        };
        let wanted = context_at_least(options.ctx_min, &access.engine().running(), &model);
        let session = access
            .open(
                &model,
                OpenOptions {
                    ctx: Some(wanted),
                    ..OpenOptions::default()
                },
                &options.cancel,
                &|| {},
            )
            .await?;
        Ok(Worker {
            context: session.context.unwrap_or(wanted),
            thinking: Thinking::of(&details),
            tools: crate::chat::call::uses_tools(&details),
            session,
            cancel: options.cancel,
            retry_delay: RETRY_DELAY,
            _working: crate::gpu::activity::hold(),
        })
    }

    /// Pause avant un nouvel essai (essais : plus courte).
    #[cfg(test)]
    pub(crate) fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    /// Adresse du modèle (essais : journal du faux, client seul).
    #[cfg(test)]
    pub(crate) fn endpoint(&self) -> &inference::Endpoint {
        &self.session.endpoint
    }

    /// Identifiant canonique du modèle.
    pub(crate) fn model(&self) -> &str {
        &self.session.id
    }

    /// Contexte du processus (`-c`) ; distant sans contexte annoncé : celui
    /// demandé.
    pub(crate) fn context(&self) -> u32 {
        self.context
    }

    /// Le modèle sait réfléchir (avertissement `NoThinking` de la page).
    pub(crate) fn thinks(&self) -> bool {
        self.thinking.thinks
    }

    /// Rend la session : le modèle redevient évinçable (son minuteur
    /// `keep_alive` part). Le décharger tout de suite (prêt de la carte) est
    /// l'affaire de l'appelant (`resources`).
    pub(crate) fn release(self) {
        drop(self);
    }

    /// Jetons de `text` comptés par le modèle (`/tokenize`) ; distant (pas
    /// de `/tokenize`) : estimés à 3 caractères par jeton (`chat/compact.rs`).
    pub(crate) async fn count_tokens(&self, text: &str) -> Result<usize, AgentError> {
        if self.session.remote {
            return Ok(text.chars().count().div_ceil(3));
        }
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(AgentError::Cancelled),
            tokens = inference::tokenize(&self.session.endpoint, text) => {
                tokens.map(|tokens| tokens.len()).map_err(|error| self.error(error))
            }
        }
    }

    /// Requête de `messages` : défauts du modèle (importé d'Ollama), puis
    /// température, réflexion, budget et limite de jetons.
    fn request(
        &self,
        messages: Vec<Message>,
        temperature: f32,
        think: ThinkMode,
        max_tokens: Option<u32>,
    ) -> ChatRequest {
        let mut request = self.session.request(messages);
        let (think, budget) = self.thinking.request(think);
        request.think = think;
        request.thinking_budget = budget;
        request.temperature = Some(temperature);
        request.max_tokens =
            Some(max_tokens.unwrap_or_else(|| budget.unwrap_or(0).saturating_add(ANSWER_TOKENS)));
        request
    }

    /// Réponse en JSON contraint par `call.schema`, lue par
    /// `study::repair::parse_object` (raisonnement retiré, échappements
    /// réparés). UN nouvel essai si : erreur passagère (après une pause),
    /// réponse sans objet lisible, ou coupée à la limite (`length` : limite
    /// doublée). Deuxième échec : [`AgentError::Unreadable`], ou
    /// [`AgentError::Incomplete`] si elle est encore coupée.
    pub(crate) async fn ask_json(&self, call: &Call) -> Result<Json, AgentError> {
        let started = Instant::now();
        let mut request = self.request(
            vec![Message::system(&call.system), Message::user(&call.user)],
            call.temperature,
            call.think,
            call.max_tokens,
        );
        request.response_format = Some(ResponseFormat::JsonSchema(call.schema.clone()));
        let mut usage = Usage::default();
        let mut reasoning_chars = 0;
        let mut retried = false;
        loop {
            let reply = match self.generate(&request, None).await {
                Ok(reply) => reply,
                Err(error) if error.transient() && !retried => {
                    // i18n-ignore : journal.
                    crate::app_log!(
                        "chatonlm: agent: {} failed ({}), trying again once",
                        self.model(),
                        error.code()
                    );
                    retried = true;
                    self.pause().await?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            add(&mut usage, reply.usage);
            reasoning_chars += reply.reasoning_chars();
            let cut = reply.finish == FinishReason::Length;
            let value = (!cut).then(|| repair::parse_object(&reply.raw)).flatten();
            if let Some(value) = value {
                return Ok(Json {
                    value,
                    usage,
                    reasoning_chars,
                    elapsed: started.elapsed(),
                });
            }
            // i18n-ignore : journal.
            crate::app_log!(
                "chatonlm: agent: unreadable answer from {} (finish {:?}, {} characters)",
                self.model(),
                reply.finish,
                reply.raw.chars().count()
            );
            if retried {
                return Err(if cut {
                    AgentError::Incomplete {
                        partial: reply.visible,
                    }
                } else {
                    AgentError::Unreadable
                });
            }
            retried = true;
            if cut {
                request.max_tokens = request.max_tokens.map(|max| max.saturating_mul(2));
            }
        }
    }

    /// Réponse en texte à `messages`, en flux : fragments visibles à
    /// `on_delta` (jamais de réflexion). Coupée à la limite (`length`) : un
    /// nouvel essai à limite double ([`Delta::Restart`] d'abord), puis
    /// [`AgentError::Incomplete`] avec le texte obtenu. Boucle :
    /// [`AgentError::Looping`].
    pub(crate) async fn write(
        &self,
        messages: Vec<Message>,
        options: &Write,
        on_delta: &(dyn Fn(Delta) + Send + Sync),
    ) -> Result<Text, AgentError> {
        let request = self.request(
            messages,
            options.temperature,
            options.think,
            options.max_tokens,
        );
        let mut usage = Usage::default();
        let mut reasoning_chars = 0;
        let reply = self
            .complete(request, Some(on_delta), &mut usage, &mut reasoning_chars)
            .await?;
        Ok(Text {
            content: reply.visible,
            reasoning_chars,
            usage,
            finish: reply.finish,
        })
    }

    /// Réponse à `messages` avec `tools` offerts : au plus `max_turns` tours
    /// d'appels, puis une requête sans outils (règle du chat). À chaque tour,
    /// au plus [`MAX_TOOL_CALLS`] appels exécutés dans l'ordre (les suivants
    /// reçoivent « trop d'appels ») ; résultats ou erreurs rendus au modèle,
    /// chacun borné ([`super::MAX_TOOL_RESULT_CHARS`]) et tous ensemble à la
    /// moitié de la place restante dans le contexte (coupés avec une mention,
    /// ou écartés). Rien n'est offert à un modèle qui ne connaît pas les
    /// outils ; des appels reçus d'une requête sans outils sont ignorés.
    /// Coupure et boucle : comme [`Worker::write`].
    pub(crate) async fn run_tools(
        &self,
        mut messages: Vec<Message>,
        tools: &[&dyn Tool],
        max_turns: usize,
        options: &Write,
    ) -> Result<Text, AgentError> {
        let definitions: Vec<Value> = if self.tools {
            tools.iter().map(|tool| tool.definition()).collect()
        } else {
            Vec::new()
        };
        let mut usage = Usage::default();
        let mut reasoning_chars = 0;
        let mut turns = 0;
        loop {
            let offered = !definitions.is_empty() && turns < max_turns;
            let mut request = self.request(
                messages.clone(),
                options.temperature,
                options.think,
                options.max_tokens,
            );
            if offered {
                request.tools = definitions.clone();
            }
            let reply = self
                .complete(request, None, &mut usage, &mut reasoning_chars)
                .await?;
            if !offered || reply.tool_calls.is_empty() {
                return Ok(Text {
                    content: reply.visible,
                    reasoning_chars,
                    usage,
                    finish: reply.finish,
                });
            }
            turns += 1;
            let mut assistant = Message::assistant(reply.visible);
            if !reply.reasoning.is_empty() {
                assistant.reasoning_content = Some(reply.reasoning);
            }
            assistant.tool_calls = reply.tool_calls.clone();
            messages.push(assistant);
            let mut room = results_room(self.context, &messages);
            for (index, call) in reply.tool_calls.iter().enumerate() {
                let result = if index < MAX_TOOL_CALLS {
                    self.call_tool(tools, call).await?
                } else {
                    Err(too_many_calls())
                };
                let text = tool_message(result, room);
                room = room.saturating_sub(text.chars().count());
                messages.push(Message::tool(&call.id, &call.name, text));
            }
        }
    }

    /// Exécute un appel : outil inconnu ou arguments illisibles → erreur
    /// rendue au modèle ; arrêt du travail → [`AgentError::Cancelled`].
    async fn call_tool(
        &self,
        tools: &[&dyn Tool],
        call: &ToolCall,
    ) -> Result<Result<String, String>, AgentError> {
        // i18n-ignore : messages au modèle (anglais, comme ceux du chat).
        let Some(tool) = tools.iter().find(|tool| tool.name() == call.name) else {
            return Ok(Err(format!("unknown tool: {}", call.name)));
        };
        let Some(args) = call.arguments_json() else {
            // i18n-ignore : message au modèle.
            return Ok(Err("the arguments are not valid JSON".into()));
        };
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(AgentError::Cancelled),
            result = tool.call(args, self.cancel.child_token()) => Ok(result),
        }
    }

    /// Une réponse entière, avec UN nouvel essai à limite double si elle est
    /// coupée (`length`) sans appel d'outil ; encore coupée :
    /// [`AgentError::Incomplete`]. Jetons et réflexion ajoutés à `usage` et
    /// `reasoning_chars`.
    async fn complete(
        &self,
        mut request: ChatRequest,
        on_delta: Sink<'_>,
        usage: &mut Usage,
        reasoning_chars: &mut usize,
    ) -> Result<Reply, AgentError> {
        let mut retried = false;
        loop {
            let reply = self.generate(&request, on_delta).await?;
            add(usage, reply.usage);
            *reasoning_chars += reply.reasoning_chars();
            if reply.finish != FinishReason::Length || !reply.tool_calls.is_empty() {
                return Ok(reply);
            }
            if retried {
                return Err(AgentError::Incomplete {
                    partial: reply.visible,
                });
            }
            // i18n-ignore : journal.
            crate::app_log!(
                "chatonlm: agent: answer of {} cut at {:?} tokens, trying again with twice as many",
                self.model(),
                request.max_tokens
            );
            retried = true;
            request.max_tokens = request.max_tokens.map(|max| max.saturating_mul(2));
            if let Some(sink) = on_delta {
                sink(Delta::Restart);
            }
        }
    }

    /// Une requête lue en flux jusqu'à sa fin : contenu (visible envoyé à
    /// `on_delta`), raisonnement, appels d'outils, fin et jetons. Boucle
    /// trouvée : arrêtée là ([`AgentError::Looping`]), requête lâchée.
    async fn generate(
        &self,
        request: &ChatRequest,
        on_delta: Sink<'_>,
    ) -> Result<Reply, AgentError> {
        let token = self.cancel.child_token();
        // Requête lâchée (connexion fermée) dès que ce futur finit.
        let _close = token.clone().drop_guard();
        let mut stream = crate::llm::chat_stream(&self.session, request, token).await?;
        let mut visible = Answer::default();
        let mut watch = LoopWatch::default();
        let mut reply = Reply {
            raw: String::new(),
            visible: String::new(),
            reasoning: String::new(),
            tool_calls: Vec::new(),
            finish: FinishReason::Stop,
            usage: Usage::default(),
        };
        loop {
            let event = match stream.next().await {
                None => {
                    // i18n-ignore : détail technique du journal.
                    let cut = InferenceError::Interrupted("the stream ended early".into());
                    return Err(self.error(cut));
                }
                Some(Err(error)) => return Err(self.error(error)),
                Some(Ok(event)) => event,
            };
            match event {
                Event::Content(text) => {
                    let deltas = visible.push(&text);
                    if let Some(sink) = on_delta {
                        deltas.into_iter().for_each(sink);
                    }
                    reply.raw.push_str(&text);
                    if watch.push("", &text).is_some() {
                        return Err(AgentError::Looping);
                    }
                }
                Event::Reasoning(text) => {
                    reply.reasoning.push_str(&text);
                    if watch.push(&text, "").is_some() {
                        return Err(AgentError::Looping);
                    }
                }
                Event::ToolCalls(calls) => reply.tool_calls = calls,
                Event::Done {
                    finish_reason,
                    usage,
                    ..
                } => {
                    reply.finish = finish_reason;
                    reply.usage = usage.unwrap_or_default();
                    reply.visible = visible.text();
                    return Ok(reply);
                }
            }
        }
    }

    /// Erreur d'une requête de ce modèle (arrêt si déchargé à la demande).
    fn error(&self, error: InferenceError) -> AgentError {
        AgentError::from(self.session.error(error))
    }

    /// Pause avant un nouvel essai ; arrêt : [`AgentError::Cancelled`].
    async fn pause(&self) -> Result<(), AgentError> {
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(AgentError::Cancelled),
            () = tokio::time::sleep(self.retry_delay) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::models::ThinkingOptions;

    fn details(
        capabilities: &[&str],
        thinking: Option<Vec<PageThink>>,
        always: bool,
    ) -> ModelDetails {
        ModelDetails {
            model: "a/b-GGUF:Q4_K_M".into(),
            capabilities: capabilities.iter().map(|w| (*w).to_owned()).collect(),
            thinking: thinking.map(|values| ThinkingOptions {
                values,
                default: None,
            }),
            always_thinks: always,
        }
    }

    /// Réflexion envoyée selon le modèle : rien pour un modèle qui ne
    /// réfléchit pas, sauf le budget si elle est demandée ; coupée s'il le permet, sinon bornée ; demandée avec
    /// son budget (marche si le modèle l'accepte, sinon son gabarit).
    #[test]
    fn thinking_is_asked_as_the_model_allows() {
        use inference::Think;
        let on = ThinkMode::On { budget: 6_000 };
        let plain = Thinking::of(&details(&["completion"], None, false));
        assert_eq!(plain.request(ThinkMode::Off), (Think::Default, None));
        // Réflexion demandée : le budget part quand même (sans effet sur un
        // modèle qui ne réfléchit pas ; borne si ses détails se trompent).
        assert_eq!(plain.request(on), (Think::Default, Some(6_000)));
        let both = Some(vec![PageThink::Enabled(true), PageThink::Enabled(false)]);
        let qwen = Thinking::of(&details(&["completion", "thinking"], both.clone(), false));
        assert_eq!(qwen.request(ThinkMode::Off), (Think::Off, None));
        assert_eq!(qwen.request(on), (Think::On, Some(6_000)));
        // Réfléchit même coupé : bornée au plus court.
        let stubborn = Thinking::of(&details(&["completion", "thinking"], both, true));
        assert_eq!(
            stubborn.request(ThinkMode::Off),
            (Think::Default, Some(OFF_BUDGET))
        );
        assert_eq!(stubborn.request(on), (Think::On, Some(6_000)));
        // gpt-oss : niveaux seulement, ni marche ni arrêt.
        let levels = Some(vec![
            PageThink::Level("low".into()),
            PageThink::Level("high".into()),
        ]);
        let oss = Thinking::of(&details(&["completion", "thinking"], levels, false));
        assert_eq!(
            oss.request(ThinkMode::Off),
            (Think::Default, Some(OFF_BUDGET))
        );
        assert_eq!(oss.request(on), (Think::Default, Some(6_000)));
    }

    #[test]
    fn reasoning_written_in_the_content_is_counted() {
        let reply = |raw: &str, visible: &str, reasoning: &str| Reply {
            raw: raw.into(),
            visible: visible.into(),
            reasoning: reasoning.into(),
            tool_calls: Vec::new(),
            finish: FinishReason::Stop,
            usage: Usage::default(),
        };
        assert_eq!(
            reply("<think>abc</think>ok", "ok", "").reasoning_chars(),
            18
        );
        assert_eq!(reply(" ok ", "ok", "hmm").reasoning_chars(), 3);
    }
}
