//! Requêtes et réponses de l'API de `llama-server`, et leurs corps JSON.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Map, Value, json};

/// Rôle d'un message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// Partie d'un contenu composé.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Text(String),
    /// Adresse de l'image : `data:<type>;base64,<données>` (voir
    /// [`Part::image`]).
    ImageUrl(String),
}

impl Part {
    /// Image jointe, encodée en URI `data:` (JPEG, PNG, BMP, GIF… : formats
    /// de `stb_image` ; pas de WebP, à convertir avant).
    pub fn image(mime: &str, bytes: &[u8]) -> Part {
        Part::ImageUrl(format!("data:{mime};base64,{}", BASE64.encode(bytes)))
    }

    fn to_json(&self) -> Value {
        match self {
            Part::Text(text) => json!({ "type": "text", "text": text }),
            Part::ImageUrl(url) => json!({ "type": "image_url", "image_url": { "url": url } }),
        }
    }
}

/// Contenu d'un message : texte, ou parties (texte et images).
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Text(String),
    Parts(Vec<Part>),
}

impl Content {
    fn to_json(&self) -> Value {
        match self {
            Content::Text(text) => json!(text),
            Content::Parts(parts) => parts.iter().map(Part::to_json).collect(),
        }
    }
}

/// Appel d'outil, demandé par le modèle ou rendu dans l'historique.
/// `arguments` : texte JSON tel que le modèle l'a écrit (pas toujours
/// valide : voir [`ToolCall::arguments_json`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl ToolCall {
    /// Arguments lus en JSON ; `None` s'ils ne le sont pas.
    pub fn arguments_json(&self) -> Option<Value> {
        serde_json::from_str(&self.arguments).ok()
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "type": "function",
            "function": { "name": self.name, "arguments": self.arguments },
        })
    }
}

/// Message de la conversation.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: Content,
    /// Réflexion d'un tour précédent de l'assistant (renvoyée aux gabarits
    /// qui la gardent).
    pub reasoning_content: Option<String>,
    /// Appels faits par l'assistant à ce tour.
    pub tool_calls: Vec<ToolCall>,
    /// Message `tool` : appel auquel il répond.
    pub tool_call_id: Option<String>,
    /// Message `tool` : nom de l'outil.
    pub name: Option<String>,
}

impl Message {
    fn new(role: Role, content: Content) -> Self {
        Self {
            role,
            content,
            reasoning_content: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self::new(Role::System, Content::Text(text.into()))
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, Content::Text(text.into()))
    }

    /// Message de l'utilisateur en parties (texte et images).
    pub fn user_parts(parts: Vec<Part>) -> Self {
        Self::new(Role::User, Content::Parts(parts))
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::new(Role::Assistant, Content::Text(text.into()))
    }

    /// Résultat d'un outil, en réponse à l'appel `call_id`.
    pub fn tool(
        call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: Some(call_id.into()),
            name: Some(name.into()),
            ..Self::new(Role::Tool, Content::Text(content.into()))
        }
    }

    fn to_json(&self) -> Value {
        let mut message = json!({ "role": self.role.as_str(), "content": self.content.to_json() });
        if let Some(reasoning) = self
            .reasoning_content
            .as_ref()
            .filter(|text| !text.is_empty())
        {
            message["reasoning_content"] = json!(reasoning);
        }
        if !self.tool_calls.is_empty() {
            message["tool_calls"] = self.tool_calls.iter().map(ToolCall::to_json).collect();
        }
        if let Some(id) = &self.tool_call_id {
            message["tool_call_id"] = json!(id);
        }
        if let Some(name) = &self.name {
            message["name"] = json!(name);
        }
        message
    }
}

/// Effort de réflexion (gabarits qui le distinguent : gpt-oss ; `/props` →
/// `chat_template_caps.reasoning_effort_levels`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effort {
    Low,
    Medium,
    High,
}

/// Réflexion demandée.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Think {
    /// Rien d'envoyé : choix du gabarit du modèle.
    #[default]
    Default,
    /// `chat_template_kwargs.enable_thinking = false`.
    Off,
    /// `chat_template_kwargs.enable_thinking = true`.
    On,
    /// `reasoning_effort`.
    Effort(Effort),
}

/// Sortie structurée.
#[derive(Clone, Debug, PartialEq)]
pub enum ResponseFormat {
    /// Contenu conforme à ce schéma JSON (grammaire du serveur). La forme
    /// imbriquée d'OpenAI (`json_schema: {name, schema}`) est la seule que
    /// b12110 applique en chat : `{"type": "json_schema", "schema": …}` y
    /// est ignorée sans erreur (relevé du lot L4).
    JsonSchema(Value),
    /// Un objet JSON quelconque.
    JsonObject,
}

/// Demande de réponse à une conversation. Les réglages à `None` restent
/// ceux du serveur.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatRequest {
    /// Nom du modèle écrit dans le corps s'il est posé (fournisseur
    /// distant) ; `llama-server` sert un seul modèle et l'ignore.
    pub model: Option<String>,
    pub messages: Vec<Message>,
    /// Définitions des outils, au format d'OpenAI (`{"type": "function",
    /// "function": {name, description, parameters}}`), telles quelles.
    pub tools: Vec<Value>,
    pub think: Think,
    pub response_format: Option<ResponseFormat>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
    pub min_p: Option<f32>,
    pub repeat_penalty: Option<f32>,
    /// Pénalités et fenêtre de la répétition (acceptées par `llama-server`
    /// ; défauts repris d'Ollama).
    pub presence_penalty: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub repeat_last_n: Option<i32>,
    pub seed: Option<u32>,
    pub stop: Vec<String>,
    /// Réutiliser le cache du prompt (défaut du serveur : oui).
    pub cache_prompt: Option<bool>,
}

impl ChatRequest {
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            messages,
            ..Self::default()
        }
    }

    /// Corps JSON de `/v1/chat/completions`.
    pub fn body(&self, stream: bool) -> Value {
        let mut body = Map::new();
        if let Some(model) = &self.model {
            body.insert("model".into(), json!(model));
        }
        body.insert(
            "messages".into(),
            self.messages.iter().map(Message::to_json).collect(),
        );
        body.insert("stream".into(), json!(stream));
        if stream {
            body.insert("stream_options".into(), json!({ "include_usage": true }));
        }
        if !self.tools.is_empty() {
            body.insert("tools".into(), json!(self.tools));
        }
        match self.think {
            Think::Default => {}
            Think::Off => {
                body.insert(
                    "chat_template_kwargs".into(),
                    json!({ "enable_thinking": false }),
                );
            }
            Think::On => {
                body.insert(
                    "chat_template_kwargs".into(),
                    json!({ "enable_thinking": true }),
                );
            }
            Think::Effort(effort) => {
                let effort = match effort {
                    Effort::Low => "low",
                    Effort::Medium => "medium",
                    Effort::High => "high",
                };
                body.insert("reasoning_effort".into(), json!(effort));
            }
        }
        match &self.response_format {
            None => {}
            Some(ResponseFormat::JsonSchema(schema)) => {
                body.insert(
                    "response_format".into(),
                    json!({ "type": "json_schema", "json_schema": { "name": "response", "schema": schema } }),
                );
            }
            Some(ResponseFormat::JsonObject) => {
                body.insert("response_format".into(), json!({ "type": "json_object" }));
            }
        }
        let mut set = |key: &str, value: Option<Value>| {
            if let Some(value) = value {
                body.insert(key.into(), value);
            }
        };
        set("max_tokens", self.max_tokens.map(Value::from));
        set("temperature", self.temperature.map(Value::from));
        set("top_p", self.top_p.map(Value::from));
        set("top_k", self.top_k.map(Value::from));
        set("min_p", self.min_p.map(Value::from));
        set("repeat_penalty", self.repeat_penalty.map(Value::from));
        set("presence_penalty", self.presence_penalty.map(Value::from));
        set("frequency_penalty", self.frequency_penalty.map(Value::from));
        set("repeat_last_n", self.repeat_last_n.map(Value::from));
        set("seed", self.seed.map(Value::from));
        set("cache_prompt", self.cache_prompt.map(Value::from));
        if !self.stop.is_empty() {
            body.insert("stop".into(), json!(self.stop));
        }
        Value::Object(body)
    }

    /// Corps pour un serveur de cette langue (`llama-server` : [`ChatRequest::body`]
    /// tel quel ; distant : champs de son API seulement, voir `dialect.rs`).
    pub fn body_for(&self, dialect: super::Dialect, stream: bool) -> Value {
        let mut body = self.body(stream);
        super::dialect::adapt(dialect, &mut body);
        body
    }
}

/// Raison de la fin d'une réponse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    /// `max_tokens` atteint, ou contexte plein.
    Length,
    ToolCalls,
    /// Autre valeur (gardée pour le journal).
    Other(String),
}

impl FinishReason {
    pub(super) fn parse(text: &str) -> FinishReason {
        match text {
            "stop" => FinishReason::Stop,
            "length" => FinishReason::Length,
            "tool_calls" => FinishReason::ToolCalls,
            other => FinishReason::Other(other.to_owned()),
        }
    }
}

/// Jetons comptés par le serveur (`usage`). `prompt` compte tout le prompt,
/// la partie reprise du cache (`cached`) comprise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt: u32,
    pub completion: u32,
    pub cached: u32,
}

impl Usage {
    pub(super) fn parse(value: &Value) -> Option<Usage> {
        let number = |value: &Value| value.as_u64().and_then(|n| u32::try_from(n).ok());
        Some(Usage {
            prompt: number(&value["prompt_tokens"])?,
            completion: number(&value["completion_tokens"])?,
            cached: number(&value["prompt_tokens_details"]["cached_tokens"]).unwrap_or(0),
        })
    }
}

/// Temps mesurés par le serveur (`timings`). Contexte occupé à la fin de
/// la réponse : `cache_n + prompt_n + predicted_n`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Timings {
    pub cache_n: u32,
    pub prompt_n: u32,
    pub prompt_ms: f64,
    pub predicted_n: u32,
    pub predicted_ms: f64,
}

impl Timings {
    pub(super) fn parse(value: &Value) -> Option<Timings> {
        let number = |value: &Value| value.as_u64().and_then(|n| u32::try_from(n).ok());
        Some(Timings {
            cache_n: number(&value["cache_n"]).unwrap_or(0),
            prompt_n: number(&value["prompt_n"])?,
            prompt_ms: value["prompt_ms"].as_f64().unwrap_or(0.0),
            predicted_n: number(&value["predicted_n"])?,
            predicted_ms: value["predicted_ms"].as_f64().unwrap_or(0.0),
        })
    }
}

/// Réponse entière (`chat_once`).
#[derive(Clone, Debug, PartialEq)]
pub struct ChatResponse {
    pub content: String,
    /// Réflexion (`reasoning_content`), vide sans réflexion.
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: FinishReason,
    pub usage: Option<Usage>,
    pub timings: Option<Timings>,
}

/// Fichier du dépôt donné en contexte à l'autocomplétion (`input_extra`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InfillChunk {
    pub filename: String,
    pub text: String,
}

/// Demande d'autocomplétion (`/infill`). Le serveur pose les jetons FIM du
/// GGUF ; un modèle qui n'en a pas rend du texte sans valeur (relevé de la
/// preuve) : l'app vérifie les jetons FIM avant de l'appeler.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InfillRequest {
    pub input_prefix: String,
    pub input_suffix: String,
    pub input_extra: Vec<InfillChunk>,
    pub n_predict: Option<u32>,
    pub temperature: Option<f32>,
    pub stop: Vec<String>,
}

impl InfillRequest {
    pub fn body(&self) -> Value {
        let mut body = json!({
            "input_prefix": self.input_prefix,
            "input_suffix": self.input_suffix,
        });
        if !self.input_extra.is_empty() {
            body["input_extra"] = self
                .input_extra
                .iter()
                .map(|chunk| json!({ "filename": chunk.filename, "text": chunk.text }))
                .collect();
        }
        if let Some(n) = self.n_predict {
            body["n_predict"] = json!(n);
        }
        if let Some(temperature) = self.temperature {
            body["temperature"] = json!(temperature);
        }
        if !self.stop.is_empty() {
            body["stop"] = json!(self.stop);
        }
        body
    }
}

/// Fin d'une autocomplétion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfillStop {
    /// Fin de texte du modèle.
    Eos,
    /// Une chaîne de `stop`.
    Word,
    /// `n_predict` atteint.
    Limit,
    /// Autre ou absent.
    Other,
}

/// Réponse de `/infill`.
#[derive(Clone, Debug, PartialEq)]
pub struct InfillResponse {
    pub content: String,
    pub stop: InfillStop,
    pub tokens_evaluated: u32,
    pub tokens_predicted: u32,
    pub timings: Option<Timings>,
}

/// Capacités du gabarit de chat (`/props` → `chat_template_caps`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TemplateCaps {
    pub supports_tools: bool,
    pub supports_tool_calls: bool,
    pub supports_parallel_tool_calls: bool,
    pub supports_reasoning_effort: bool,
    pub supports_preserve_reasoning: bool,
    pub supports_system_role: bool,
    pub reasoning_effort_levels: Vec<String>,
}

/// Entrées acceptées par le modèle chargé (`/props` → `modalities`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modalities {
    pub vision: bool,
    pub audio: bool,
    pub video: bool,
}

/// Ce que `/props` dit du serveur et du modèle chargé.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Props {
    /// Contexte d'un emplacement (celui de `-c`).
    pub n_ctx: u32,
    pub total_slots: u32,
    pub model_path: String,
    pub model_alias: String,
    pub chat_template: String,
    pub caps: TemplateCaps,
    pub modalities: Modalities,
    /// Version du serveur : `b12110-cd959bae9`.
    pub build_info: String,
    /// Modèle déchargé par `--sleep-idle-seconds` (rechargé à la demande).
    pub is_sleeping: bool,
}

impl Props {
    pub(super) fn parse(value: &Value) -> Option<Props> {
        let number = |value: &Value| value.as_u64().and_then(|n| u32::try_from(n).ok());
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        let flag = |value: &Value| value.as_bool().unwrap_or(false);
        let caps = &value["chat_template_caps"];
        Some(Props {
            n_ctx: number(&value["default_generation_settings"]["n_ctx"])?,
            total_slots: number(&value["total_slots"]).unwrap_or(1),
            model_path: text(&value["model_path"]),
            model_alias: text(&value["model_alias"]),
            chat_template: text(&value["chat_template"]),
            caps: TemplateCaps {
                supports_tools: flag(&caps["supports_tools"]),
                supports_tool_calls: flag(&caps["supports_tool_calls"]),
                supports_parallel_tool_calls: flag(&caps["supports_parallel_tool_calls"]),
                supports_reasoning_effort: flag(&caps["supports_reasoning_effort"]),
                supports_preserve_reasoning: flag(&caps["supports_preserve_reasoning"]),
                supports_system_role: flag(&caps["supports_system_role"]),
                reasoning_effort_levels: caps["reasoning_effort_levels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|level| level.as_str().map(str::to_owned))
                    .collect(),
            },
            modalities: Modalities {
                vision: flag(&value["modalities"]["vision"]),
                audio: flag(&value["modalities"]["audio"]),
                video: flag(&value["modalities"]["video"]),
            },
            build_info: text(&value["build_info"]),
            is_sleeping: flag(&value["is_sleeping"]),
        })
    }
}

/// État de `/health`.
#[derive(Clone, Debug, PartialEq)]
pub enum Health {
    /// 200 : modèle chargé, prêt.
    Ready,
    /// 503 : modèle en cours de chargement.
    Loading,
    /// Rien ne répond (connexion refusée, coupée, ou délai dépassé).
    Unreachable,
    /// Autre réponse (autre programme sur le port, erreur du serveur).
    Unexpected(super::InferenceError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn think_sets_the_kwargs_or_the_effort() {
        let request = |think| ChatRequest {
            think,
            ..ChatRequest::new(vec![Message::user("hi")])
        };
        let body = request(Think::Default).body(false);
        assert!(
            body.get("chat_template_kwargs").is_none() && body.get("reasoning_effort").is_none()
        );
        assert_eq!(
            request(Think::Off).body(false)["chat_template_kwargs"],
            json!({ "enable_thinking": false })
        );
        assert_eq!(
            request(Think::On).body(false)["chat_template_kwargs"],
            json!({ "enable_thinking": true })
        );
        let effort = request(Think::Effort(Effort::High)).body(false);
        assert_eq!(effort["reasoning_effort"], "high");
        assert!(effort.get("chat_template_kwargs").is_none());
    }

    #[test]
    fn the_body_carries_every_setting_and_only_those_given() {
        let mut request = ChatRequest::new(vec![Message::system("Be brief."), Message::user("hi")]);
        let bare = request.body(true);
        assert_eq!(bare["stream"], true);
        assert_eq!(bare["stream_options"], json!({ "include_usage": true }));
        for key in [
            "tools",
            "max_tokens",
            "temperature",
            "top_p",
            "top_k",
            "min_p",
            "repeat_penalty",
            "seed",
            "stop",
            "cache_prompt",
            "response_format",
        ] {
            assert!(bare.get(key).is_none(), "{key}");
        }
        assert!(request.body(false).get("stream_options").is_none());
        request.max_tokens = Some(64);
        request.temperature = Some(0.5);
        request.top_p = Some(0.25);
        request.top_k = Some(20);
        request.min_p = Some(0.125);
        request.repeat_penalty = Some(1.5);
        request.seed = Some(7);
        request.stop = vec!["\n\n".into()];
        request.cache_prompt = Some(false);
        request.response_format = Some(ResponseFormat::JsonSchema(json!({ "type": "object" })));
        let body = request.body(false);
        assert_eq!(body["max_tokens"], 64);
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["top_p"], 0.25);
        assert_eq!(body["top_k"], 20);
        assert_eq!(body["min_p"], 0.125);
        assert_eq!(body["repeat_penalty"], 1.5);
        assert_eq!(body["seed"], 7);
        assert_eq!(body["stop"], json!(["\n\n"]));
        assert_eq!(body["cache_prompt"], false);
        assert_eq!(
            body["response_format"],
            json!({ "type": "json_schema", "json_schema": { "name": "response", "schema": { "type": "object" } } })
        );
        assert_eq!(
            body["messages"][0],
            json!({ "role": "system", "content": "Be brief." })
        );
        request.response_format = Some(ResponseFormat::JsonObject);
        assert_eq!(
            request.body(false)["response_format"],
            json!({ "type": "json_object" })
        );
    }

    #[test]
    fn messages_take_the_openai_shape() {
        let mut assistant = Message::assistant("");
        assistant.reasoning_content = Some("thinking".into());
        assistant.tool_calls = vec![ToolCall {
            id: "abc".into(),
            name: "get_weather".into(),
            arguments: r#"{"city":"Paris"}"#.into(),
        }];
        let messages = [
            Message::user_parts(vec![
                Part::Text("What?".into()),
                Part::image("image/png", b"\x89PNG"),
            ]),
            assistant,
            Message::tool("abc", "get_weather", "14C"),
        ];
        let body = ChatRequest::new(messages.to_vec()).body(false);
        assert_eq!(
            body["messages"][0]["content"],
            json!([{ "type": "text", "text": "What?" }, { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw==" } }])
        );
        assert_eq!(
            body["messages"][1],
            json!({ "role": "assistant", "content": "", "reasoning_content": "thinking", "tool_calls": [
                { "id": "abc", "type": "function", "function": { "name": "get_weather", "arguments": "{\"city\":\"Paris\"}" } }] })
        );
        assert_eq!(
            body["messages"][2],
            json!({ "role": "tool", "content": "14C", "tool_call_id": "abc", "name": "get_weather" })
        );
        // Réflexion vide : pas envoyée.
        let mut empty = Message::assistant("ok");
        empty.reasoning_content = Some(String::new());
        assert!(
            ChatRequest::new(vec![empty]).body(false)["messages"][0]
                .get("reasoning_content")
                .is_none()
        );
    }

    #[test]
    fn the_infill_body_has_only_what_is_given() {
        let mut request = InfillRequest {
            input_prefix: "def f():\n    ".into(),
            input_suffix: "\n".into(),
            ..InfillRequest::default()
        };
        assert_eq!(
            request.body(),
            json!({ "input_prefix": "def f():\n    ", "input_suffix": "\n" })
        );
        request.input_extra = vec![InfillChunk {
            filename: "a.py".into(),
            text: "x = 1\n".into(),
        }];
        request.n_predict = Some(48);
        request.temperature = Some(0.25);
        request.stop = vec!["\n\n".into()];
        let body = request.body();
        assert_eq!(
            body["input_extra"],
            json!([{ "filename": "a.py", "text": "x = 1\n" }])
        );
        assert_eq!(
            (body["n_predict"].as_u64(), body["temperature"].as_f64()),
            (Some(48), Some(0.25))
        );
        assert_eq!(body["stop"], json!(["\n\n"]));
    }

    #[test]
    fn props_are_read_like_the_real_server_writes_them() {
        // Extrait du relevé de b12110 (Qwen3 0.6B, `-c 1024 -np 1`).
        let real = json!({
            "default_generation_settings": { "params": { "temperature": 0.8 }, "n_ctx": 1024 },
            "total_slots": 1, "model_alias": "/m/Qwen3-0.6B-Q4_K_M.gguf", "model_path": "/m/Qwen3-0.6B-Q4_K_M.gguf",
            "modalities": { "vision": false, "video": false, "audio": false },
            "chat_template": "{%- if tools %}", "chat_template_caps": {
                "supports_object_arguments": true, "supports_parallel_tool_calls": true,
                "supports_preserve_reasoning": false, "supports_reasoning_effort": false,
                "supports_string_content": true, "supports_system_role": true, "supports_tool_calls": true,
                "supports_tools": true, "supports_typed_content": false, "reasoning_effort_levels": [] },
            "build_info": "b12110-cd959bae9", "is_sleeping": false,
        });
        let props = Props::parse(&real).unwrap();
        assert_eq!(props.n_ctx, 1024);
        assert_eq!(props.total_slots, 1);
        assert!(
            props.caps.supports_tools
                && props.caps.supports_parallel_tool_calls
                && props.caps.supports_system_role
        );
        assert!(
            !props.caps.supports_reasoning_effort && !props.modalities.vision && !props.is_sleeping
        );
        assert_eq!(props.build_info, "b12110-cd959bae9");
        assert!(Props::parse(&json!({ "total_slots": 1 })).is_none());
    }

    #[test]
    fn usage_and_timings_are_read_from_the_last_chunk() {
        // Dernier bloc relevé sur b12110.
        let chunk = json!({ "choices": [],
            "usage": { "completion_tokens": 60, "prompt_tokens": 14, "total_tokens": 74, "prompt_tokens_details": { "cached_tokens": 3 } },
            "timings": { "cache_n": 3, "prompt_n": 11, "prompt_ms": 47.352, "prompt_per_token_ms": 3.38,
                "prompt_per_second": 295.6, "predicted_n": 60, "predicted_ms": 886.16 } });
        assert_eq!(
            Usage::parse(&chunk["usage"]),
            Some(Usage {
                prompt: 14,
                completion: 60,
                cached: 3
            })
        );
        let timings = Timings::parse(&chunk["timings"]).unwrap();
        assert_eq!(
            (timings.cache_n, timings.prompt_n, timings.predicted_n),
            (3, 11, 60)
        );
        assert!((timings.predicted_ms - 886.16).abs() < 1e-9);
        assert_eq!(Usage::parse(&Value::Null), None);
        assert_eq!(FinishReason::parse("length"), FinishReason::Length);
        assert_eq!(
            FinishReason::parse("content_filter"),
            FinishReason::Other("content_filter".into())
        );
    }
}
