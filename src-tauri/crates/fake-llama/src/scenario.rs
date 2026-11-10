//! Scénarios choisis par le nom du modèle : le texte `chatonlm-<scénario>`
//! cherché dans l'alias (`--alias`) puis dans le nom du fichier (`-m`), sans
//! égard à la casse. Un modèle dont le nom n'en contient aucun répond
//! simplement (scénario `chatonlm-simple`).
//!
//! | Nom | Comportement |
//! |---|---|
//! | `chatonlm-simple` | « Hello! You said: <dernier message> » en morceaux d'un mot |
//! | `chatonlm-think` | réflexion (`reasoning_content`) puis réponse ; coupée par `enable_thinking: false` |
//! | `chatonlm-effort` | réflexion dont la longueur suit `reasoning_effort` (low 2, medium 6, high 12 morceaux ; `none` la coupe) ; `/props` annonce les niveaux |
//! | `chatonlm-tools` | appelle le premier outil reçu (premier argument du schéma = dernier message), arguments en fragments ; après un message `tool`, répond avec son contenu |
//! | `chatonlm-parallel` | deux appels (index 0 et 1) |
//! | `chatonlm-noid` | un appel sans `id` (l'app doit en donner un) |
//! | `chatonlm-slow` | 150 ms entre deux morceaux |
//! | `chatonlm-loop` | répète la même phrase jusqu'à `max_tokens` ou au contexte plein (`length`), 5 ms par morceau |
//! | `chatonlm-long` | réponse de 3 000 morceaux, sans attente |
//! | `chatonlm-hang` | premier bloc (rôle), puis plus rien, connexion ouverte |
//! | `chatonlm-pings` | premier bloc (rôle), puis seulement des pings SSE (`:`) toutes les 100 ms pendant 1 s, puis plus rien (connexion ouverte) |
//! | `chatonlm-crash` | trois morceaux, puis connexion coupée et fin du processus (`abort`) |
//! | `chatonlm-error` | trois morceaux, puis `data: {"error": …}` (500) et fin du flux |
//! | `chatonlm-fim` | `/infill` complète (« return a + b ») ; tout autre modèle y rend du texte sans valeur, comme un vrai modèle sans jetons FIM |
//! | `chatonlm-fim-slow` | idem, 300 ms d'attente avant la réponse |
//! | `chatonlm-slowload` | chargement de 1,5 s (`/health` 503 pendant ce temps) |
//! | `chatonlm-longload` | chargement de 6 s |
//! | `chatonlm-loadfail` | chargement raté : le processus s'arrête (code 1) au bout du délai de chargement |
//! | `chatonlm-vulkanfail` | binaire seulement : sur la carte, erreur de Vulkan puis `abort` au chargement ; sur le processeur (`-ngl 0`, `GGML_VK_VISIBLE_DEVICES=none`), comme `chatonlm-simple` |
//! | `chatonlm-bigctx` | binaire seulement : chargement de 1,5 s, puis refus (code 1, cache de contexte non alloué) si `-c` dépasse 8 192 ; sinon comme `chatonlm-simple` |
//! | `chatonlm-bindfail` | binaire seulement : le premier lancement (fichier `<modèle>.bound` absent, alors créé) ne peut pas lier son port (code 1, comme le vrai) ; les suivants servent ; chaque lancement ajoute une ligne à `<modèle>.attempts` |
//! | `chatonlm-bindfail-always` | idem, à chaque lancement |
//! | `chatonlm-otheralias` | `/props` annonce un autre `model_alias` (un autre serveur sur le port) ; lancements comptés dans `<modèle>.attempts` |
//! | `chatonlm-otherctx` | `/props` annonce un autre `n_ctx` ; lancements comptés de même |
//!
//! Règles communes, comme le vrai serveur : les requêtes attendent un
//! emplacement libre (`-np`, un seul par défaut dans [`crate::Config::new`]),
//! sans en-têtes tant qu'elles sont en file ; une image sans `--mmproj` → 500
//! « image input is not supported » ; prompt plus long que le contexte → 400
//! `exceed_context_size_error` ; `response_format` → contenu JSON conforme au
//! schéma (valeurs factices) ; `stop` coupe la réponse ; `max_tokens` et la
//! fin du contexte la coupent aussi (`length`).

use std::time::Duration;

use serde_json::{Value, json};

use crate::tokens;

/// Texte rendu par `/infill` d'un modèle `chatonlm-fim`.
pub const FIM_COMPLETION: &str = "return a + b";
/// Texte sans valeur rendu par `/infill` d'un modèle sans jetons FIM (relevé
/// sur Qwen3 0.6B, b12110).
pub const FIM_JUNK: &str = "\n</think>\n\n</think>\n\n```python\n";
/// Phrase répétée par `chatonlm-loop`.
pub const LOOP_SENTENCE: &str = "The cat sat on the mat.";
/// Pings de `chatonlm-pings`, et leur intervalle (le vrai : 30 s,
/// `--sse-ping-interval`).
pub const PINGS: u32 = 10;
pub const PING_EVERY: Duration = Duration::from_millis(100);
/// Morceaux prévus au plus pour `chatonlm-loop` (le contexte coupe avant).
const LOOP_MAX: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Simple,
    Think,
    Effort,
    Tools,
    Parallel,
    NoId,
    Slow,
    Loop,
    Long,
    Hang,
    Pings,
    Crash,
    Error,
    Fim,
    FimSlow,
    SlowLoad,
    LongLoad,
    LoadFail,
    VulkanFail,
    BigCtx,
    BindFail,
    BindFailAlways,
    OtherAlias,
    OtherCtx,
}

/// Noms, du plus long au plus court quand l'un contient l'autre.
const NAMES: &[(&str, Scenario)] = &[
    ("chatonlm-fim-slow", Scenario::FimSlow),
    ("chatonlm-fim", Scenario::Fim),
    ("chatonlm-think", Scenario::Think),
    ("chatonlm-effort", Scenario::Effort),
    ("chatonlm-tools", Scenario::Tools),
    ("chatonlm-parallel", Scenario::Parallel),
    ("chatonlm-noid", Scenario::NoId),
    ("chatonlm-slowload", Scenario::SlowLoad),
    ("chatonlm-longload", Scenario::LongLoad),
    ("chatonlm-loadfail", Scenario::LoadFail),
    ("chatonlm-vulkanfail", Scenario::VulkanFail),
    ("chatonlm-bigctx", Scenario::BigCtx),
    ("chatonlm-bindfail-always", Scenario::BindFailAlways),
    ("chatonlm-bindfail", Scenario::BindFail),
    ("chatonlm-otheralias", Scenario::OtherAlias),
    ("chatonlm-otherctx", Scenario::OtherCtx),
    ("chatonlm-slow", Scenario::Slow),
    ("chatonlm-loop", Scenario::Loop),
    ("chatonlm-long", Scenario::Long),
    ("chatonlm-hang", Scenario::Hang),
    ("chatonlm-pings", Scenario::Pings),
    ("chatonlm-crash", Scenario::Crash),
    ("chatonlm-error", Scenario::Error),
    ("chatonlm-simple", Scenario::Simple),
];

/// Noms des scénarios intégrés (essai des scénarios des lots).
pub fn builtin_names() -> Vec<&'static str> {
    NAMES.iter().map(|(name, _)| *name).collect()
}

impl Scenario {
    /// Scénario d'après l'alias puis le nom du fichier du modèle ; à défaut,
    /// `chatonlm-simple`.
    pub fn of(alias: Option<&str>, model: &str) -> Scenario {
        Self::find(alias, model).unwrap_or(Scenario::Simple)
    }

    /// Scénario intégré nommé par l'alias puis le nom du fichier, s'il y en
    /// a un.
    pub fn find(alias: Option<&str>, model: &str) -> Option<Scenario> {
        let file = model.rsplit(['/', '\\']).next().unwrap_or(model);
        for text in alias.into_iter().chain([file]) {
            let text = text.to_ascii_lowercase();
            if let Some((_, scenario)) = NAMES.iter().find(|(name, _)| text.contains(name)) {
                return Some(*scenario);
            }
        }
        None
    }

    /// Délai de chargement propre au scénario.
    pub fn load_delay(self) -> Option<Duration> {
        match self {
            Scenario::SlowLoad | Scenario::BigCtx => Some(Duration::from_millis(1500)),
            Scenario::LongLoad => Some(Duration::from_secs(6)),
            _ => None,
        }
    }

    pub fn thinks(self) -> bool {
        matches!(self, Scenario::Think | Scenario::Effort)
    }

    pub fn uses_tools(self) -> bool {
        matches!(self, Scenario::Tools | Scenario::Parallel | Scenario::NoId)
    }

    pub fn fim(self) -> bool {
        matches!(self, Scenario::Fim | Scenario::FimSlow)
    }

    /// Attente entre deux morceaux.
    pub fn delay(self) -> Duration {
        match self {
            Scenario::Slow => Duration::from_millis(150),
            Scenario::Loop => Duration::from_millis(5),
            _ => Duration::ZERO,
        }
    }

    /// Gabarit annoncé par `/props` (ce que lit une app qui déduit les
    /// capacités du gabarit).
    pub fn chat_template(self) -> String {
        let mut template =
            String::from("{%- for message in messages %}{{ message.content }}{%- endfor %}");
        if self.uses_tools() {
            template.push_str("{%- if tools %}<tools>{{ tools }}</tools><tool_call>{%- endif %}");
        }
        if self.thinks() {
            template.push_str("{%- if enable_thinking is defined and not enable_thinking %}<think>\n\n</think>{%- endif %}");
        }
        if self == Scenario::Effort {
            template.push_str("{{ reasoning_effort }}");
        }
        template
    }

    /// `chat_template_caps` de `/props`.
    pub fn caps(self) -> Value {
        let effort = self == Scenario::Effort;
        json!({
            "supports_object_arguments": true,
            "supports_parallel_tool_calls": self.uses_tools(),
            "supports_preserve_reasoning": false,
            "supports_reasoning_effort": effort,
            "supports_string_content": true,
            "supports_system_role": true,
            "supports_tool_calls": self.uses_tools(),
            "supports_tools": self.uses_tools(),
            "supports_typed_content": false,
            "reasoning_effort_levels": if effort { json!(["low", "medium", "high"]) } else { json!([]) },
        })
    }
}

/// Appel d'outil prévu : arguments en fragments.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub id: Option<String>,
    pub name: String,
    pub fragments: Vec<String>,
}

/// Fin du flux.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Normal,
    /// Plus rien après le premier bloc, connexion gardée.
    Hang,
    /// Pings seuls après le premier bloc ([`PINGS`] fois, toutes les
    /// [`PING_EVERY`]), puis plus rien, connexion gardée.
    Pings,
    /// Connexion coupée, puis fin du processus.
    Crash,
    /// Bloc d'erreur, puis fin du flux.
    Error,
}

/// Réponse prévue.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub reasoning: Vec<String>,
    pub content: Vec<String>,
    pub calls: Vec<Call>,
    pub finish: &'static str,
    pub delay: Duration,
    pub end: End,
}

/// Texte d'un message (`content` chaîne, ou parties `text`).
pub fn text_of(message: &Value) -> String {
    match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// Nombre d'images jointes aux messages.
pub fn images(messages: &[Value]) -> usize {
    messages
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter(|part| part["type"] == "image_url")
        .count()
}

/// Nombre d'images WebP jointes (d'après leurs premiers octets, « RIFF » en
/// base64, ou leur type annoncé).
pub fn webp_images(messages: &[Value]) -> usize {
    messages
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter(|part| part["type"] == "image_url")
        .filter_map(|part| part["image_url"]["url"].as_str())
        .filter(|url| {
            let data = url.split_once(";base64,").map_or("", |(_, data)| data);
            url.starts_with("data:image/webp") || data.starts_with("UklGR")
        })
        .count()
}

/// Valeur factice conforme à un schéma JSON (objets, tableaux, types
/// simples, `enum`, `const`).
pub fn instance(schema: &Value) -> Value {
    if let Some(value) = schema.get("const") {
        return value.clone();
    }
    if let Some(first) = schema["enum"].as_array().and_then(|values| values.first()) {
        return first.clone();
    }
    let kind = match &schema["type"] {
        Value::Array(kinds) => kinds
            .iter()
            .find(|k| *k != "null")
            .cloned()
            .unwrap_or(Value::Null),
        kind => kind.clone(),
    };
    match kind.as_str() {
        Some("object") => {
            let mut object = serde_json::Map::new();
            if let Some(properties) = schema["properties"].as_object() {
                for (name, property) in properties {
                    object.insert(name.clone(), instance(property));
                }
            }
            Value::Object(object)
        }
        Some("array") => {
            let count = schema["minItems"].as_u64().unwrap_or(1).max(1);
            Value::Array((0..count).map(|_| instance(&schema["items"])).collect())
        }
        Some("string") => json!("text"),
        Some("integer") => json!(1),
        Some("number") => json!(1.5),
        Some("boolean") => json!(true),
        _ => Value::Null,
    }
}

/// Fragments des arguments, comme le vrai les envoie (`{`, `"`, `city`,
/// `":`, ` "`, `Paris`, `"}`) : ici par tranches de 4 caractères.
pub fn fragments(arguments: &str) -> Vec<String> {
    let characters: Vec<char> = arguments.chars().collect();
    characters
        .chunks(4)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

/// Effort demandé à la réflexion (`reasoning_effort`).
fn effort_pieces(request: &Value) -> usize {
    match request["reasoning_effort"].as_str() {
        Some("low") => 2,
        Some("high") => 12,
        _ => 6,
    }
}

/// La requête coupe la réflexion.
pub fn thinking_off(request: &Value) -> bool {
    request["chat_template_kwargs"]["enable_thinking"] == false
        || request["reasoning_effort"] == "none"
}

/// Prévoit la réponse (avant les coupures par `stop`, `max_tokens` et le
/// contexte, voir [`limit`]).
pub fn plan(scenario: Scenario, request: &Value, vision: bool) -> Reply {
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let last = messages.last().cloned().unwrap_or(Value::Null);
    let last_user = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .map(text_of)
        .unwrap_or_default();
    let mut reply = Reply {
        reasoning: Vec::new(),
        content: Vec::new(),
        calls: Vec::new(),
        finish: "stop",
        delay: scenario.delay(),
        end: End::Normal,
    };
    let tools = request["tools"].as_array().cloned().unwrap_or_default();
    let mut text = if vision && images(&messages) > 0 {
        format!(
            "I see {} image(s). You said: {last_user}",
            images(&messages)
        )
    } else {
        format!("Hello! You said: {last_user}")
    };
    match scenario {
        Scenario::Think | Scenario::Effort if !thinking_off(request) => {
            let count = if scenario == Scenario::Effort {
                effort_pieces(request)
            } else {
                8
            };
            let thought = "Let me think about this step by step before I answer the question";
            reply.reasoning = tokens::pieces(thought)
                .into_iter()
                .cycle()
                .take(count)
                .collect();
        }
        Scenario::Tools | Scenario::Parallel | Scenario::NoId if !tools.is_empty() => {
            if last["role"] == "tool" {
                text = format!("Tool result received: {}", text_of(&last));
            } else {
                let count = if scenario == Scenario::Parallel { 2 } else { 1 };
                for index in 0..count {
                    let tool = &tools[index.min(tools.len() - 1)]["function"];
                    let name = tool["name"].as_str().unwrap_or("tool").to_owned();
                    let argument = tool["parameters"]["properties"]
                        .as_object()
                        .and_then(|properties| properties.keys().next().cloned())
                        .unwrap_or_else(|| "input".into());
                    let value = if index == 0 {
                        last_user.clone()
                    } else {
                        format!("{last_user} ({index})")
                    };
                    let arguments = json!({ argument: value }).to_string();
                    reply.calls.push(Call {
                        id: (scenario != Scenario::NoId).then(|| format!("fake-call-{index}")),
                        name,
                        fragments: fragments(&arguments),
                    });
                }
                reply.finish = "tool_calls";
                return reply;
            }
        }
        Scenario::Loop => {
            reply.content = tokens::pieces(&format!("{LOOP_SENTENCE} "))
                .into_iter()
                .cycle()
                .take(LOOP_MAX)
                .collect();
            reply.finish = "length";
            return reply;
        }
        Scenario::Long => {
            text = (0..3000).map(|n| format!("w{n} ")).collect();
        }
        Scenario::Hang => reply.end = End::Hang,
        Scenario::Pings => reply.end = End::Pings,
        Scenario::Crash => reply.end = End::Crash,
        Scenario::Error => reply.end = End::Error,
        _ => {}
    }
    if let Some(format) = request.get("response_format") {
        text = match format["type"].as_str() {
            Some("json_schema") => instance(&format["json_schema"]["schema"]).to_string(),
            Some("json_object") => json!({ "answer": text }).to_string(),
            _ => text,
        };
    }
    if matches!(reply.end, End::Crash | End::Error) {
        text = "One two three".into();
    }
    // Réflexion laissée dans le contenu (`reasoning_format: none`).
    if request["reasoning_format"] == "none" && !reply.reasoning.is_empty() {
        text = format!("<think>\n{}\n</think>\n\n{text}", reply.reasoning.concat());
        reply.reasoning.clear();
    }
    reply.content = tokens::pieces(&text);
    reply
}

/// Coupe la réponse : chaîne d'arrêt (`stop`), `max_tokens` (`n_predict`) et
/// place restante dans le contexte (`room`). Rend la réponse coupée.
pub fn limit(mut reply: Reply, request: &Value, room: usize) -> Reply {
    let stops: Vec<String> = match &request["stop"] {
        Value::String(stop) => vec![stop.clone()],
        Value::Array(stops) => stops
            .iter()
            .filter_map(|s| s.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    if reply.calls.is_empty() && !stops.is_empty() {
        let text = reply.content.concat();
        if let Some(cut) = stops
            .iter()
            .filter(|s| !s.is_empty())
            .filter_map(|s| text.find(s.as_str()))
            .min()
        {
            reply.content = tokens::pieces(&text[..cut]);
            reply.finish = "stop";
        }
    }
    let max = request["max_tokens"]
        .as_i64()
        .or(request["n_predict"].as_i64())
        .filter(|n| *n >= 0)
        .map_or(usize::MAX, |n| n as usize)
        .min(room);
    let total = reply.reasoning.len() + reply.content.len();
    if reply.calls.is_empty() && total >= max {
        let reasoning = reply.reasoning.len().min(max);
        reply.reasoning.truncate(reasoning);
        reply.content.truncate(max - reasoning);
        reply.finish = "length";
    }
    reply
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(extra: Value) -> Value {
        let mut request = json!({ "messages": [{ "role": "user", "content": "Hi there" }] });
        if let (Value::Object(base), Value::Object(extra)) = (&mut request, extra) {
            base.extend(extra);
        }
        request
    }

    #[test]
    fn the_name_chooses_the_scenario() {
        assert_eq!(
            Scenario::of(
                None,
                "/m/chatonlm/chatonlm-think-GGUF/chatonlm-think-Q4_K_M.gguf"
            ),
            Scenario::Think
        );
        assert_eq!(
            Scenario::of(Some("chatonlm/x:Q4"), "C:\\m\\chatonlm-FIM-slow.gguf"),
            Scenario::FimSlow
        );
        assert_eq!(
            Scenario::of(Some("chatonlm-slowload"), "chatonlm-tools.gguf"),
            Scenario::SlowLoad
        );
        assert_eq!(Scenario::of(None, "/m/chatonlm-slow.gguf"), Scenario::Slow);
        assert_eq!(
            Scenario::of(None, "/chatonlm-think/qwen.gguf"),
            Scenario::Simple
        );
    }

    #[test]
    fn thinking_follows_the_request() {
        let thought = plan(Scenario::Think, &request(json!({})), false);
        assert_eq!(thought.reasoning.len(), 8);
        let off = plan(
            Scenario::Think,
            &request(json!({ "chat_template_kwargs": { "enable_thinking": false } })),
            false,
        );
        assert!(off.reasoning.is_empty());
        let low = plan(
            Scenario::Effort,
            &request(json!({ "reasoning_effort": "low" })),
            false,
        );
        assert_eq!(low.reasoning.len(), 2);
        assert!(
            plan(
                Scenario::Effort,
                &request(json!({ "reasoning_effort": "none" })),
                false
            )
            .reasoning
            .is_empty()
        );
        let inline = plan(
            Scenario::Think,
            &request(json!({ "reasoning_format": "none" })),
            false,
        );
        assert!(inline.reasoning.is_empty());
        assert!(inline.content.concat().starts_with("<think>\nLet me"));
    }

    #[test]
    fn a_tool_is_called_with_fragments() {
        let tools = json!({ "tools": [{ "type": "function", "function": {
            "name": "get_weather", "parameters": { "type": "object", "properties": { "city": { "type": "string" } } } } }] });
        let reply = plan(Scenario::Tools, &request(tools.clone()), false);
        assert_eq!(reply.finish, "tool_calls");
        assert_eq!(reply.calls.len(), 1);
        assert_eq!(reply.calls[0].name, "get_weather");
        assert_eq!(reply.calls[0].fragments.concat(), r#"{"city":"Hi there"}"#);
        assert!(reply.calls[0].fragments.len() > 1);
        assert!(
            plan(Scenario::NoId, &request(tools.clone()), false).calls[0]
                .id
                .is_none()
        );
        assert_eq!(
            plan(Scenario::Parallel, &request(tools), false).calls.len(),
            2
        );
        // Sans outils : réponse simple.
        assert!(
            plan(Scenario::Tools, &request(json!({})), false)
                .calls
                .is_empty()
        );
    }

    #[test]
    fn the_reply_is_cut_by_stop_max_tokens_and_context() {
        let reply = plan(Scenario::Simple, &request(json!({})), false);
        assert_eq!(reply.content.concat(), "Hello! You said: Hi there");
        let stopped = limit(
            reply.clone(),
            &request(json!({ "stop": ["said"] })),
            usize::MAX,
        );
        assert_eq!(stopped.content.concat(), "Hello! You ");
        assert_eq!(stopped.finish, "stop");
        let short = limit(
            reply.clone(),
            &request(json!({ "max_tokens": 2 })),
            usize::MAX,
        );
        assert_eq!(short.content, ["Hello!", " You"]);
        assert_eq!(short.finish, "length");
        let full = limit(reply, &request(json!({})), 1);
        assert_eq!((full.content.len(), full.finish), (1, "length"));
        let endless = limit(
            plan(Scenario::Loop, &request(json!({})), false),
            &request(json!({})),
            40,
        );
        assert_eq!(endless.content.len(), 40);
    }

    #[test]
    fn a_schema_gets_a_conforming_value() {
        let schema = json!({ "type": "object", "properties": {
            "name": { "type": "string" }, "age": { "type": "integer" },
            "tags": { "type": "array", "items": { "enum": ["a", "b"] } },
            "kind": { "const": "x" }, "maybe": { "type": ["null", "boolean"] } } });
        assert_eq!(
            instance(&schema),
            json!({ "name": "text", "age": 1, "tags": ["a"], "kind": "x", "maybe": true })
        );
        let reply = plan(
            Scenario::Simple,
            &request(
                json!({ "response_format": { "type": "json_schema", "json_schema": { "name": "p", "schema": schema } } }),
            ),
            false,
        );
        let parsed: Value = serde_json::from_str(&reply.content.concat()).unwrap();
        assert_eq!(parsed["age"], 1);
    }
}
