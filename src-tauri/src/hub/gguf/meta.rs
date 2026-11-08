//! Métadonnées d'un modèle GGUF installé, lues dans le fichier à la fin d'un
//! téléchargement (base locale des modèles, contrat 100 § 3.2) : architecture,
//! contexte d'entraînement, taille annoncée, type de fichier, capacités
//! déduites du gabarit de conversation (outils, réflexion), jetons FIM,
//! modèle de base ou réglé, et pour un projecteur (`mmproj`) ses encodeurs
//! (images, audio).
//!
//! Contrairement à [`super::parse`] (256 Kio reçus du réseau), la lecture
//! parcourt le fichier : le gabarit (`tokenizer.chat_template`) suit en
//! général le vocabulaire et ses fusions, plusieurs Mo plus loin. Le fichier
//! n'est pas digne de confiance : chaque longueur annoncée est comparée à ce
//! qui reste du fichier, rien n'est alloué d'après elle au-delà de petits
//! plafonds, les valeurs inutiles sont sautées (sans lecture), et la lecture
//! s'arrête au premier octet manquant en gardant ce qu'elle a trouvé.

use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

use super::{
    ARRAY, BOOL, INT8, INT16, INT32, INT64, STRING, UINT8, UINT16, UINT32, UINT64, fixed_size,
};

/// Paires lues au plus.
const MAX_PAIRS: u64 = 4096;
/// Longueur maximale d'une clé (au-delà : lecture arrêtée).
const MAX_KEY: u64 = 1024;
/// Longueur maximale d'un nom d'architecture, d'une taille annoncée.
const MAX_SHORT: u64 = 128;
/// Longueur maximale d'un gabarit de conversation lu (plus long : ignoré,
/// capacités inconnues). Chaque gabarit est jugé dès sa lecture, puis jeté.
const MAX_TEMPLATE: u64 = 1024 * 1024;
/// Octets de gabarits lus au plus dans un fichier (clé répétée d'un fichier
/// piégé : les suivants sont sautés, sans lecture ni recherche).
const MAX_TEMPLATES: u64 = 4 * MAX_TEMPLATE;
/// Éléments d'un tableau parcourus au plus (vocabulaires de 262 144 jetons
/// et plus).
const MAX_ELEMENTS: u64 = 1 << 24;
/// Profondeur maximale de tableaux imbriqués.
const MAX_DEPTH: u32 = 3;
/// Valeurs `<préfixe>.context_length` gardées avant de connaître
/// l'architecture.
const MAX_CONTEXTS: usize = 16;
/// Jetons du vocabulaire comparés aux jetons FIM connus : les plus courts.
const MAX_FIM_TOKEN: u64 = 32;

/// Textes des jetons FIM que llama.cpp reconnaît dans le vocabulaire quand
/// le fichier ne donne pas leurs numéros (`llama-vocab.cpp`, b12110) :
/// début, trou, fin.
const FIM_PRE: &[&str] = &[
    "<|fim_prefix|>",
    "<fim-prefix>",
    "<fim_prefix>",
    "<｜fim▁begin｜>",
    "<PRE>",
    "▁<PRE>",
    "<|code_prefix|>",
];
const FIM_SUF: &[&str] = &[
    "<|fim_suffix|>",
    "<fim-suffix>",
    "<fim_suffix>",
    "<｜fim▁hole｜>",
    "<SUF>",
    "▁<SUF>",
    "<|code_suffix|>",
];
const FIM_MID: &[&str] = &[
    "<|fim_middle|>",
    "<fim-middle>",
    "<fim_middle>",
    "<｜fim▁end｜>",
    "<MID>",
    "▁<MID>",
    "<|code_middle|>",
];

/// Ce que l'app garde d'un modèle installé.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelMeta {
    pub architecture: Option<String>,
    /// Contexte d'entraînement (`<arch>.context_length`).
    pub context_length: Option<u64>,
    /// `general.size_label` (« 0.6B », « 8x7B »).
    pub size_label: Option<String>,
    /// `general.file_type` (type de quantification de llama.cpp).
    pub file_type: Option<u32>,
    /// Le gabarit (ou celui des outils) parle d'outils.
    pub tools: bool,
    /// Le gabarit sait réfléchir (voir [`template_capabilities`]).
    pub thinking: bool,
    /// Jetons FIM (début, trou, fin) donnés par le fichier ou trouvés dans le
    /// vocabulaire : `/infill` possible.
    pub fim: bool,
    /// Modèle de base connu comme tel : fichier d'un convertisseur récent
    /// (`general.basename`) sans `general.finetune`, et dont le nom ne dit ni
    /// « instruct », ni « chat », ni « it ».
    pub base: bool,
    /// Projecteur : `clip.has_vision_encoder` (`None` : clé absente).
    pub vision_encoder: Option<bool>,
    /// Projecteur : `clip.has_audio_encoder` (`None` : clé absente).
    pub audio_encoder: Option<bool>,
}

impl ModelMeta {
    /// Pour un projecteur : il lit les images. Clé absente (anciens
    /// projecteurs de vision) : oui, sauf s'il dit lire l'audio.
    pub fn sees(&self) -> bool {
        self.vision_encoder
            .unwrap_or(!self.audio_encoder.unwrap_or(false))
    }

    /// Pour un projecteur : il lit l'audio.
    pub fn hears(&self) -> bool {
        self.audio_encoder.unwrap_or(false)
    }
}

/// Échec de la lecture.
#[derive(Debug)]
pub enum MetaError {
    /// Pas un fichier GGUF v2 ou v3.
    NotGguf,
    /// Fichier illisible.
    Io(std::io::Error),
}

/// Lit les métadonnées du fichier `path`.
pub fn read(path: &Path) -> Result<ModelMeta, MetaError> {
    let file = File::open(path).map_err(MetaError::Io)?;
    let len = file.metadata().map_err(MetaError::Io)?.len();
    read_from(file, len)
}

/// Lit les métadonnées d'un GGUF de `len` octets.
pub fn read_from<R: Read + Seek>(reader: R, len: u64) -> Result<ModelMeta, MetaError> {
    let mut stream = Stream {
        inner: BufReader::with_capacity(64 * 1024, reader),
        pos: 0,
        len,
    };
    if stream.bytes(4).as_deref() != Some(b"GGUF") {
        return Err(MetaError::NotGguf);
    }
    let version = stream.u32().ok_or(MetaError::NotGguf)?;
    if !(2..=3).contains(&version) {
        return Err(MetaError::NotGguf);
    }
    let (Some(_tensors), Some(pairs)) = (stream.u64(), stream.u64()) else {
        return Err(MetaError::NotGguf);
    };
    let mut state = State::default();
    for _ in 0..pairs.min(MAX_PAIRS) {
        if stream.pair(&mut state).is_none() {
            break;
        }
    }
    Ok(state.finish())
}

/// Ce qui a été trouvé, avant de connaître l'architecture.
#[derive(Default)]
struct State {
    meta: ModelMeta,
    contexts: Vec<(String, u64)>,
    /// Octets de gabarits déjà lus (plafond : [`MAX_TEMPLATES`]).
    template_bytes: u64,
    fim_ids: [bool; 3],
    fim_texts: [bool; 3],
    name: Option<String>,
    basename: bool,
    finetune: bool,
}

impl State {
    /// Un gabarit lu : jugé tout de suite, rien n'en est gardé.
    fn template(&mut self, template: &str) {
        let (tools, thinking) = template_capabilities(template);
        self.meta.tools |= tools;
        self.meta.thinking |= thinking;
    }

    fn finish(mut self) -> ModelMeta {
        if let Some(architecture) = &self.meta.architecture {
            self.meta.context_length = self
                .contexts
                .iter()
                .find(|(prefix, _)| prefix == architecture)
                .map(|&(_, value)| value);
        }
        self.meta.fim = self.fim_ids == [true; 3] || self.fim_texts == [true; 3];
        self.meta.base =
            self.basename && !self.finetune && !self.name.as_deref().is_some_and(tuned_name);
        self.meta
    }
}

/// Nom d'un modèle réglé pour la conversation : un mot « instruct »,
/// « chat » ou « it » (Gemma).
fn tuned_name(name: &str) -> bool {
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| ["instruct", "chat", "it"].contains(&word.to_ascii_lowercase().as_str()))
}

/// Capacités déduites d'un gabarit Jinja : outils s'il parle de `tools` ou
/// de `tool_call` ; réflexion s'il connaît `enable_thinking` (Qwen3,
/// SmolLM3) ou `reasoning_effort` (gpt-oss), ou si l'invite de génération
/// (ce qui suit le dernier `add_generation_prompt`) ouvre un `<think>`
/// (DeepSeek-R1, Qwen3-Thinking). Un gabarit qui ne fait que retirer ou
/// recoller la réflexion des anciens messages (`reasoning_content`, `<think>`
/// dans l'historique : Qwen3-Instruct-2507) ne réfléchit pas.
pub fn template_capabilities(template: &str) -> (bool, bool) {
    let tools = template.contains("tools") || template.contains("tool_call");
    let prompt = template
        .rfind("add_generation_prompt")
        .map_or("", |at| &template[at..]);
    let thinking = template.contains("enable_thinking")
        || template.contains("reasoning_effort")
        || prompt.contains("<think>");
    (tools, thinking)
}

struct Stream<R> {
    inner: BufReader<R>,
    pos: u64,
    len: u64,
}

impl<R: Read + Seek> Stream<R> {
    fn remaining(&self) -> u64 {
        self.len.saturating_sub(self.pos)
    }

    /// `n` octets (petit `n` seulement : plafonné par l'appelant).
    fn bytes(&mut self, n: u64) -> Option<Vec<u8>> {
        if n > self.remaining() {
            return None;
        }
        let mut buffer = vec![0; usize::try_from(n).ok()?];
        self.inner.read_exact(&mut buffer).ok()?;
        self.pos += n;
        Some(buffer)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        if (N as u64) > self.remaining() {
            return None;
        }
        let mut buffer = [0; N];
        self.inner.read_exact(&mut buffer).ok()?;
        self.pos += N as u64;
        Some(buffer)
    }

    fn u32(&mut self) -> Option<u32> {
        self.array::<4>().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Option<u64> {
        self.array::<8>().map(u64::from_le_bytes)
    }

    /// Saute `n` octets sans les lire.
    fn skip(&mut self, n: u64) -> Option<()> {
        if n > self.remaining() {
            return None;
        }
        self.inner.seek_relative(i64::try_from(n).ok()?).ok()?;
        self.pos += n;
        Some(())
    }

    /// Longueur d'une chaîne, si le fichier la contient.
    fn string_len(&mut self) -> Option<u64> {
        let length = self.u64()?;
        (length <= self.remaining()).then_some(length)
    }

    /// Chaîne d'au plus `max` octets ; plus longue : sautée (`Some(None)`).
    fn string(&mut self, max: u64) -> Option<Option<String>> {
        let length = self.string_len()?;
        if length > max {
            self.skip(length)?;
            return Some(None);
        }
        let bytes = self.bytes(length)?;
        Some(String::from_utf8(bytes).ok())
    }

    /// Entier non négatif de n'importe quel type entier.
    fn integer(&mut self, kind: u32) -> Option<u64> {
        Some(match kind {
            UINT8 => u64::from(self.array::<1>()?[0]),
            INT8 => u64::try_from(self.array::<1>()?[0] as i8).ok()?,
            UINT16 => u64::from(u16::from_le_bytes(self.array()?)),
            INT16 => u64::try_from(i16::from_le_bytes(self.array()?)).ok()?,
            UINT32 => u64::from(u32::from_le_bytes(self.array()?)),
            INT32 => u64::try_from(i32::from_le_bytes(self.array()?)).ok()?,
            UINT64 => u64::from_le_bytes(self.array()?),
            INT64 => u64::try_from(i64::from_le_bytes(self.array()?)).ok()?,
            _ => return None,
        })
    }

    /// Passe une valeur ; `fim` : chaînes du vocabulaire comparées aux jetons
    /// FIM connus. `None` : tronquée ou malformée.
    fn skip_value(&mut self, kind: u32, depth: u32, fim: Option<&mut [bool; 3]>) -> Option<()> {
        if let Some(size) = fixed_size(kind) {
            return self.skip(size);
        }
        match kind {
            STRING => {
                let length = self.string_len()?;
                self.skip(length)
            }
            ARRAY => {
                if depth >= MAX_DEPTH {
                    return None;
                }
                let element = self.u32()?;
                let count = self.u64()?;
                if let Some(size) = fixed_size(element) {
                    return self.skip(count.checked_mul(size)?);
                }
                // Chaque élément occupe au moins 8 octets : la boucle s'arrête
                // avec le fichier.
                if count > MAX_ELEMENTS || count > self.remaining() / 8 {
                    return None;
                }
                match (element, fim) {
                    (STRING, Some(found)) => {
                        for _ in 0..count {
                            self.vocabulary_token(found)?;
                        }
                    }
                    _ => {
                        for _ in 0..count {
                            self.skip_value(element, depth + 1, None)?;
                        }
                    }
                }
                Some(())
            }
            _ => None,
        }
    }

    /// Un jeton du vocabulaire : comparé aux jetons FIM s'il est court.
    fn vocabulary_token(&mut self, found: &mut [bool; 3]) -> Option<()> {
        let length = self.string_len()?;
        if length == 0 || length > MAX_FIM_TOKEN {
            return self.skip(length);
        }
        let bytes = self.bytes(length)?;
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return Some(());
        };
        for (index, list) in [FIM_PRE, FIM_SUF, FIM_MID].iter().enumerate() {
            if list.contains(&text) {
                found[index] = true;
            }
        }
        Some(())
    }

    /// Une paire clé-valeur.
    fn pair(&mut self, state: &mut State) -> Option<()> {
        let key_len = self.u64()?;
        if key_len > MAX_KEY {
            return None;
        }
        let key = String::from_utf8(self.bytes(key_len)?).ok()?;
        let kind = self.u32()?;
        let integer = matches!(
            kind,
            UINT8 | INT8 | UINT16 | INT16 | UINT32 | INT32 | UINT64 | INT64
        );
        match key.as_str() {
            "general.architecture" if kind == STRING => {
                state.meta.architecture = self.string(MAX_SHORT)?;
            }
            "general.name" if kind == STRING => {
                state.name = self.string(MAX_SHORT)?;
            }
            "general.basename" => {
                state.basename = true;
                self.skip_value(kind, 0, None)?;
            }
            "general.finetune" => {
                state.finetune = true;
                self.skip_value(kind, 0, None)?;
            }
            "clip.has_vision_encoder" if kind == BOOL => {
                state.meta.vision_encoder = Some(self.array::<1>()?[0] != 0);
            }
            "clip.has_audio_encoder" if kind == BOOL => {
                state.meta.audio_encoder = Some(self.array::<1>()?[0] != 0);
            }
            "general.size_label" if kind == STRING => {
                state.meta.size_label = self.string(MAX_SHORT)?;
            }
            "general.file_type" if integer => {
                state.meta.file_type = u32::try_from(self.integer(kind)?).ok();
            }
            "tokenizer.ggml.tokens" if kind == ARRAY => {
                self.skip_value(kind, 0, Some(&mut state.fim_texts))?;
            }
            _ if kind == STRING
                && (key == "tokenizer.chat_template"
                    || key == "tokenizer.chat_template.tool_use") =>
            {
                let budget = MAX_TEMPLATES.saturating_sub(state.template_bytes);
                if let Some(template) = self.string(MAX_TEMPLATE.min(budget))? {
                    state.template_bytes += template.len() as u64;
                    state.template(&template);
                }
            }
            _ if integer && fim_slot(&key).is_some() => {
                self.integer(kind)?;
                if let Some(slot) = fim_slot(&key) {
                    state.fim_ids[slot] = true;
                }
            }
            _ if integer && key.ends_with(".context_length") => {
                let value = self.integer(kind)?;
                let prefix = &key[..key.len() - ".context_length".len()];
                if state.contexts.len() < MAX_CONTEXTS && !prefix.is_empty() {
                    state.contexts.push((prefix.to_owned(), value));
                }
            }
            _ => self.skip_value(kind, 0, None)?,
        }
        Some(())
    }
}

/// Place (début, trou, fin) d'une clé de jeton FIM, nouvelle ou ancienne.
fn fim_slot(key: &str) -> Option<usize> {
    let name = key.strip_prefix("tokenizer.ggml.")?;
    match name {
        "fim_pre_token_id" | "prefix_token_id" => Some(0),
        "fim_suf_token_id" | "suffix_token_id" => Some(1),
        "fim_mid_token_id" | "middle_token_id" => Some(2),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::super::tests::Writer;
    use super::*;

    fn meta(bytes: &[u8]) -> Result<ModelMeta, MetaError> {
        read_from(Cursor::new(bytes), bytes.len() as u64)
    }

    /// GGUF d'un petit Qwen3 : gabarit après le vocabulaire, comme les vrais.
    fn qwen3(tokens: &[&str], template: &str) -> Vec<u8> {
        let mut writer = Writer::new(7);
        writer
            .u32("qwen3.context_length", 40960)
            .string("general.architecture", "qwen3")
            .string("general.size_label", "0.6B")
            .u32("general.file_type", 15)
            .string_array("tokenizer.ggml.tokens", tokens)
            .u32_array("tokenizer.ggml.token_type", &vec![1; tokens.len()])
            .string("tokenizer.chat_template", template);
        writer.finish()
    }

    #[test]
    fn the_metadata_of_a_model_are_read() {
        let template =
            "{%- if tools %}<tools>{% endif %}{% if enable_thinking %}<think>{% endif %}";
        let found = meta(&qwen3(&["a", "<|fim_prefix|>", "b"], template)).unwrap();
        assert_eq!(
            found,
            ModelMeta {
                architecture: Some("qwen3".into()),
                context_length: Some(40960),
                size_label: Some("0.6B".into()),
                file_type: Some(15),
                tools: true,
                thinking: true,
                ..ModelMeta::default()
            }
        );
        // Les trois jetons FIM dans le vocabulaire : /infill possible.
        let coder = meta(&qwen3(
            &["<|fim_prefix|>", "x", "<|fim_suffix|>", "<|fim_middle|>"],
            "{{ messages }}",
        ))
        .unwrap();
        assert!(coder.fim);
        assert!(!coder.tools && !coder.thinking);
    }

    #[test]
    fn fim_token_ids_are_enough() {
        let mut writer = Writer::new(4);
        writer
            .string("general.architecture", "llama")
            .u32("tokenizer.ggml.fim_pre_token_id", 1)
            .u32("tokenizer.ggml.fim_suf_token_id", 2)
            .u32("tokenizer.ggml.middle_token_id", 3);
        assert!(meta(&writer.finish()).unwrap().fim);
        let mut partial = Writer::new(3);
        partial
            .string("general.architecture", "llama")
            .u32("tokenizer.ggml.fim_pre_token_id", 1)
            .u32("tokenizer.ggml.fim_suf_token_id", 2);
        assert!(!meta(&partial.finish()).unwrap().fim);
    }

    #[test]
    fn templates_tell_tools_and_thinking() {
        for (template, tools, thinking) in [
            ("{% for tool in tools %}", true, false),
            ("<tool_call>", true, false),
            ("{{ reasoning_effort }}", false, true),
            ("{% if enable_thinking is false %}", false, true),
            (
                "{%- if add_generation_prompt %}{{- '<|im_start|>assistant\n<think>\n' }}{%- endif %}",
                false,
                true,
            ),
            // La réflexion des anciens messages, recollée ou retirée : pas
            // une invite qui réfléchit.
            (
                "{{ message.reasoning_content }}<think></think>{% if add_generation_prompt %}A:{% endif %}",
                false,
                false,
            ),
            ("<think>\n", false, false),
            ("{{ bos_token }}{{ message.content }}", false, false),
        ] {
            assert_eq!(
                template_capabilities(template),
                (tools, thinking),
                "{template}"
            );
        }
        // Gabarit des outils à part (Hermes).
        let mut writer = Writer::new(3);
        writer
            .string("general.architecture", "llama")
            .string("tokenizer.chat_template", "{{ messages }}")
            .string("tokenizer.chat_template.tool_use", "{{ tools }}");
        let found = meta(&writer.finish()).unwrap();
        assert!(found.tools && !found.thinking);
    }

    /// Gabarits réels (Apache-2.0) : celui de Qwen3-4B-Instruct-2507 tel
    /// qu'unsloth le met dans ses GGUF recolle `reasoning_content` et
    /// `<think>` dans l'historique, sans réfléchir (Ollama ne le marque pas) ;
    /// Qwen3-4B-Thinking-2507 et DeepSeek-R1 ouvrent `<think>` dans
    /// l'invite de génération.
    #[test]
    fn real_templates_tell_thinking_apart() {
        let instruct = include_str!("testdata/qwen3-4b-instruct-2507.jinja");
        assert!(instruct.contains("reasoning_content") && instruct.contains("<think>"));
        assert_eq!(template_capabilities(instruct), (true, false));
        let thinking = include_str!("testdata/qwen3-4b-thinking-2507.jinja");
        assert_eq!(template_capabilities(thinking), (true, true));
        let r1 = include_str!("testdata/deepseek-r1-distill.jinja");
        assert!(template_capabilities(r1).1);
    }

    /// Clé du gabarit répétée (fichier piégé) : au plus `MAX_TEMPLATES`
    /// octets lus, les suivants sont sautés ; chaque gabarit est jugé dès sa
    /// lecture.
    #[test]
    fn repeated_templates_are_read_up_to_a_budget() {
        let filler = " ".repeat(MAX_TEMPLATE as usize);
        let count = MAX_TEMPLATES / MAX_TEMPLATE + 1;
        let mut writer = Writer::new(count + 1);
        writer.string("general.architecture", "llama");
        for _ in 1..count {
            writer.string("tokenizer.chat_template", &filler);
        }
        writer.string("tokenizer.chat_template", "{{ tools }}");
        let found = meta(&writer.finish()).unwrap();
        assert!(!found.tools);
        // Dans le budget, le dernier gabarit compte.
        let mut writer = Writer::new(4);
        writer
            .string("general.architecture", "llama")
            .string("tokenizer.chat_template", &filler)
            .string("tokenizer.chat_template", &filler)
            .string("tokenizer.chat_template", "{{ tools }}");
        assert!(meta(&writer.finish()).unwrap().tools);
    }

    /// Encodeurs d'un projecteur : images, audio, ou clés absentes (anciens
    /// projecteurs de vision).
    #[test]
    fn projector_encoders_are_read() {
        let projector = |vision: Option<bool>, audio: Option<bool>| {
            let pairs = 1 + u64::from(vision.is_some()) + u64::from(audio.is_some());
            let mut writer = Writer::new(pairs);
            writer.string("general.architecture", "clip");
            for (key, value) in [
                ("clip.has_vision_encoder", vision),
                ("clip.has_audio_encoder", audio),
            ] {
                if let Some(value) = value {
                    writer.key(key, BOOL);
                    writer.0.push(u8::from(value));
                }
            }
            meta(&writer.finish()).unwrap()
        };
        let gemma = projector(Some(true), None);
        assert!(gemma.sees() && !gemma.hears());
        let voxtral = projector(Some(false), Some(true));
        assert!(!voxtral.sees() && voxtral.hears());
        let omni = projector(Some(true), Some(true));
        assert!(omni.sees() && omni.hears());
        let audio_only = projector(None, Some(true));
        assert!(!audio_only.sees());
        let old = projector(None, None);
        assert!(old.sees() && !old.hears());
    }

    /// Modèle de base : convertisseur récent (`general.basename`), sans
    /// `general.finetune` ni nom réglé (Qwen2.5-Coder 1.5B de ggml-org).
    #[test]
    fn base_models_are_told_when_the_file_says_so() {
        let file = |name: &str, finetune: bool, basename: bool| {
            let mut writer = Writer::new(2 + u64::from(finetune) + u64::from(basename));
            writer
                .string("general.architecture", "qwen2")
                .string("general.name", name);
            if basename {
                writer.string("general.basename", "Qwen2.5-Coder");
            }
            if finetune {
                writer.string("general.finetune", "Instruct");
            }
            meta(&writer.finish()).unwrap().base
        };
        assert!(file("Qwen2.5 Coder 1.5B", false, true));
        assert!(!file("Qwen2.5 Coder 1.5B Instruct", true, true));
        assert!(!file("Qwen2.5 Coder 1.5B Instruct", false, true));
        assert!(!file("gemma-3-4b-it", false, true));
        // Ancien convertisseur : on ne sait pas.
        assert!(!file("Qwen2.5 Coder 1.5B", false, false));
    }

    #[test]
    fn other_files_are_not_gguf() {
        for bytes in [
            &b""[..],
            b"GGU",
            b"PK\x03\x04 not a model",
            b"GGUF\x01\0\0\0",
        ] {
            assert!(matches!(meta(bytes), Err(MetaError::NotGguf)), "{bytes:?}");
        }
        assert!(matches!(
            read(Path::new("/nonexistent/chatonlm/model.gguf")),
            Err(MetaError::Io(_))
        ));
    }

    /// Fichier coupé à n'importe quel octet : ni panique, ni erreur après
    /// l'en-tête, ce qui précède la coupure est gardé.
    #[test]
    fn every_truncation_is_safe() {
        let whole = qwen3(
            &["<|fim_prefix|>", "<|fim_suffix|>", "<|fim_middle|>"],
            "tools",
        );
        for cut in 0..whole.len() {
            match meta(&whole[..cut]) {
                Err(_) => assert!(cut < 24, "{cut}"),
                // Le gabarit est la dernière paire : jamais lu en entier.
                Ok(found) => assert!(cut >= 24 && !found.tools, "{cut}"),
            }
        }
        assert!(meta(&whole).unwrap().tools);
    }

    /// Longueurs et nombres annoncés énormes : rien n'est alloué ni parcouru
    /// d'après eux.
    #[test]
    fn huge_announced_lengths_allocate_nothing() {
        let header = |pairs: u64| {
            let mut out = b"GGUF".to_vec();
            out.extend(3u32.to_le_bytes());
            out.extend(0u64.to_le_bytes());
            out.extend(pairs.to_le_bytes());
            out
        };
        let key = |out: &mut Vec<u8>, name: &str, kind: u32| {
            out.extend((name.len() as u64).to_le_bytes());
            out.extend(name.as_bytes());
            out.extend(kind.to_le_bytes());
        };
        // Clé de 2^60 octets.
        let mut huge_key = header(1);
        huge_key.extend((1u64 << 60).to_le_bytes());
        assert_eq!(meta(&huge_key).unwrap(), ModelMeta::default());
        // Gabarit de 2^62 octets, tableau de 2^62 chaînes, de 2^62 entiers.
        for (kind, tail) in [
            (STRING, (1u64 << 62).to_le_bytes().to_vec()),
            (
                ARRAY,
                [
                    STRING.to_le_bytes().to_vec(),
                    (1u64 << 62).to_le_bytes().to_vec(),
                ]
                .concat(),
            ),
            (
                ARRAY,
                [
                    UINT32.to_le_bytes().to_vec(),
                    (1u64 << 62).to_le_bytes().to_vec(),
                ]
                .concat(),
            ),
        ] {
            let mut out = header(u64::MAX);
            key(&mut out, "general.architecture", STRING);
            out.extend(5u64.to_le_bytes());
            out.extend(b"llama");
            key(
                &mut out,
                if kind == STRING {
                    "tokenizer.chat_template"
                } else {
                    "tokenizer.ggml.tokens"
                },
                kind,
            );
            out.extend(tail);
            let found = meta(&out).unwrap();
            assert_eq!(found.architecture.as_deref(), Some("llama"));
        }
        // Tableaux imbriqués trop profonds : arrêt.
        let mut nested = header(2);
        key(&mut nested, "x", ARRAY);
        for _ in 0..8 {
            nested.extend(ARRAY.to_le_bytes());
            nested.extend(1u64.to_le_bytes());
        }
        assert_eq!(meta(&nested).unwrap(), ModelMeta::default());
    }

    /// Octets au hasard dans un vrai en-tête : ni panique ni boucle sans fin.
    #[test]
    fn random_corruptions_are_safe() {
        let whole = qwen3(&["<|fim_prefix|>", "a", "b"], "{{ tools }}");
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..2000 {
            let mut bytes = whole.clone();
            for _ in 0..4 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let at = 4 + (seed as usize) % (bytes.len() - 4);
                bytes[at] = (seed >> 32) as u8;
            }
            let _ = meta(&bytes);
        }
    }

    #[test]
    fn a_template_too_long_is_ignored() {
        let long = format!("tools{}", " ".repeat(MAX_TEMPLATE as usize));
        let mut writer = Writer::new(2);
        writer
            .string("general.architecture", "llama")
            .string("tokenizer.chat_template", &long);
        let found = meta(&writer.finish()).unwrap();
        assert!(!found.tools);
        assert_eq!(found.architecture.as_deref(), Some("llama"));
    }

    #[test]
    fn a_real_file_is_read_from_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.gguf");
        std::fs::write(&path, qwen3(&["a"], "{{ tools }}")).unwrap();
        let found = read(&path).unwrap();
        assert_eq!(found.architecture.as_deref(), Some("qwen3"));
        assert!(found.tools);
    }
}
