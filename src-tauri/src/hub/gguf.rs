//! Lecture bornée de l'en-tête d'un fichier GGUF (métadonnées clé-valeur), pour
//! estimer le cache de contexte d'un modèle (34 § 9). Les octets viennent du
//! réseau : rien n'est alloué d'après une longueur annoncée, chaque lecture
//! est vérifiée contre ce qui a été reçu, et la lecture s'arrête au premier
//! octet manquant en gardant ce qui a été trouvé.
//!
//! Format (GGUF v2 et v3, petit-boutiste) : « GGUF », version (u32), nombre de
//! tenseurs (u64), nombre de paires (u64), puis chaque paire : clé (chaîne),
//! type (u32), valeur. Chaîne = longueur (u64) + octets UTF-8.

use serde::{Deserialize, Serialize};

/// Octets demandés au serveur (requête Range) : les clés d'architecture
/// précèdent le vocabulaire dans les fichiers mesurés (34 § 9).
pub const HEADER_BYTES: usize = 256 * 1024;

/// Paires lues au plus.
const MAX_PAIRS: u64 = 4096;
/// Longueur maximale d'une clé (au-delà : en-tête jugé illisible).
const MAX_KEY: u64 = 1024;
/// Longueur maximale du nom d'architecture gardé.
const MAX_ARCH: usize = 128;
/// Profondeur maximale de tableaux imbriqués.
const MAX_DEPTH: u32 = 3;

/// Version du contenu de [`GgufFacts`] : un en-tête gardé en cache sous une
/// version antérieure est relu (les clés des couches récurrentes et des
/// fenêtres glissantes n'y étaient pas).
pub const FORMAT: u32 = 2;

/// Couches lues au plus (tableaux par couche).
const MAX_LAYERS: u64 = 1024;

/// Valeurs utiles d'un en-tête GGUF.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GgufFacts {
    pub architecture: Option<String>,
    /// Nombre de couches (`<arch>.block_count`).
    pub block_count: Option<u64>,
    pub embedding_length: Option<u64>,
    /// Têtes d'attention (la plus grande valeur si elle varie par couche).
    pub head_count: Option<u64>,
    /// Têtes clé-valeur : valeur unique (la plus grande si le fichier en donne
    /// une par couche).
    pub head_count_kv: Option<u64>,
    /// Têtes clé-valeur par couche, quand le fichier les donne ainsi (0 : couche
    /// sans attention, modèles hybrides).
    pub head_count_kv_layers: Option<Vec<u64>>,
    pub key_length: Option<u64>,
    pub value_length: Option<u64>,
    pub context_length: Option<u64>,
    /// Couches de prédiction multi-jetons en fin de pile
    /// (`nextn_predict_layers`), jamais récurrentes ni à fenêtre glissante.
    pub nextn_layers: Option<u64>,
    /// Une couche d'attention toutes les N, les autres récurrentes
    /// (`full_attention_interval` : qwen35, qwen3next...).
    pub full_attention_interval: Option<u64>,
    /// Fenêtre glissante, en jetons : ses couches ne gardent que la fenêtre.
    pub sliding_window: Option<u64>,
    /// Couches à fenêtre glissante, une valeur par couche...
    pub sliding_window_layers: Option<Vec<bool>>,
    /// ... ou période du motif (`sliding_window_pattern` entier).
    pub sliding_window_period: Option<u64>,
    pub key_length_swa: Option<u64>,
    pub value_length_swa: Option<u64>,
    /// État des couches récurrentes (`ssm.*`).
    pub ssm_inner_size: Option<u64>,
    pub ssm_state_size: Option<u64>,
    pub ssm_conv_kernel: Option<u64>,
    pub ssm_group_count: Option<u64>,
}

/// Intervalle des couches d'attention quand le fichier ne le donne pas : la
/// valeur par défaut de llama.cpp b11232 (`src/models/<arch>.cpp`, couche `i`
/// récurrente si `(i + 1) % N != 0`).
fn default_interval(architecture: &str) -> Option<u64> {
    match architecture {
        "qwen3next" | "qwen35" | "qwen35moe" | "qwen4exp" => Some(4),
        "minimax-01" => Some(8),
        _ => None,
    }
}

/// Motif de fenêtre glissante par défaut de llama.cpp b11232
/// (`load_swa_pattern(ml, N, dense_first)`) : période et « première couche
/// dense ». Architecture absente : toutes ses couches gardent le contexte
/// entier (estimation prudente).
fn default_swa_pattern(architecture: &str) -> Option<(u64, bool)> {
    Some(match architecture {
        "gemma2" | "gpt-oss" => (2, false),
        "gemma3" => (6, false),
        "gemma3n" => (5, false),
        "cohere2" | "exaone4" | "exaone-moe" | "llama4" | "mellum" | "muse-glimmer" | "olmo2"
        | "afmoe" => (4, false),
        "plamo3" => (8, false),
        "cohere2moe" | "laguna" | "smallthinker" => (4, true),
        _ => return None,
    })
}

/// Lot d'Ollama (`-ub`) selon le contexte, comme sa marge de lot
/// (sched.go) : 512, 1024 au-delà de 4096 jetons, 2048 au-delà de 32768.
fn ubatch(context: u64) -> u64 {
    if context > 32_768 {
        2048
    } else if context > 4096 {
        1024
    } else {
        512
    }
}

impl GgufFacts {
    /// Cache de contexte (f16, octets) à `context` jetons, couche par couche
    /// comme llama.cpp : rien pour une couche récurrente (seulement son état,
    /// fixe), la fenêtre plus un lot pour une couche à fenêtre glissante
    /// (`size_swa`, llama-kv-cache-iswa.cpp), le contexte entier sinon.
    /// `None` s'il manque une valeur ou si aucune couche n'a d'attention.
    pub fn kv_bytes(&self, context: u32) -> Option<u64> {
        let architecture = self.architecture.as_deref().unwrap_or("");
        let all = self.block_count?;
        if all == 0 || all > MAX_LAYERS {
            return None;
        }
        // Couches principales ; les couches multi-jetons suivent.
        let layers = all - self.nextn_layers.unwrap_or(0).min(all);
        let context = u64::from(context);
        let full = self.dims(false)?;
        let interval = self
            .full_attention_interval
            .or_else(|| default_interval(architecture));
        if interval == Some(0) {
            return None;
        }
        let window = self
            .sliding_window
            .filter(|&window| window > 0)
            .map(|window| {
                context
                    .min(window.saturating_add(ubatch(context)))
                    .next_multiple_of(256)
            });
        let mut total: u64 = 0;
        let mut attention = false;
        for layer in 0..all {
            let heads = match &self.head_count_kv_layers {
                Some(list) => *list.get(usize::try_from(layer).ok()?)?,
                None => self.head_count_kv?,
            };
            let recurrent = match interval {
                Some(n) => layer < layers && (layer + 1) % n != 0,
                None => heads == 0,
            };
            if recurrent {
                total = total.saturating_add(self.recurrent_state());
                continue;
            }
            if heads == 0 {
                continue;
            }
            let swa = window.is_some() && self.is_swa(architecture, layer, layers);
            let (key, value) = if swa { self.dims(true)? } else { full };
            let tokens = if swa { window? } else { context };
            let bytes = heads
                .checked_mul(key.checked_add(value)?)?
                .checked_mul(2)?
                .checked_mul(tokens)?;
            total = total.saturating_add(bytes);
            attention = true;
        }
        attention.then_some(total)
    }

    /// Dimensions d'une tête (clé, valeur), celles des couches à fenêtre
    /// glissante si `swa` et si le fichier les donne.
    fn dims(&self, swa: bool) -> Option<(u64, u64)> {
        let key = if swa {
            self.key_length_swa.or(self.key_length)
        } else {
            self.key_length
        };
        let key = match key {
            Some(length) => length,
            None => self.embedding_length?.checked_div(self.head_count?)?,
        };
        let value = if swa {
            self.value_length_swa.or(self.value_length)
        } else {
            self.value_length
        };
        (key > 0).then_some((key, value.unwrap_or(key)))
    }

    /// Couche `layer` à fenêtre glissante : liste du fichier, sinon période
    /// (du fichier ou par défaut), comme `set_swa_pattern` de llama.cpp.
    fn is_swa(&self, architecture: &str, layer: u64, layers: u64) -> bool {
        if let Some(list) = &self.sliding_window_layers {
            return usize::try_from(layer)
                .ok()
                .and_then(|index| list.get(index))
                .copied()
                .unwrap_or(false);
        }
        let default = default_swa_pattern(architecture);
        let Some(period) = self
            .sliding_window_period
            .or(default.map(|(period, _)| period))
        else {
            return false;
        };
        let dense_first = default.is_some_and(|(_, dense_first)| dense_first);
        if layer >= layers {
            false
        } else if period == 0 {
            true
        } else if dense_first {
            !layer.is_multiple_of(period)
        } else {
            layer % period < period - 1
        }
    }

    /// État fixe d'une couche récurrente (f32) : `d_inner × d_state` plus la
    /// convolution `(d_conv − 1) × (d_inner + 2 × groupes × d_state)` ; 0 si
    /// le fichier ne le décrit pas.
    fn recurrent_state(&self) -> u64 {
        let (Some(inner), Some(state)) = (self.ssm_inner_size, self.ssm_state_size) else {
            return 0;
        };
        let conv = self.ssm_conv_kernel.unwrap_or(1).saturating_sub(1);
        let groups = self.ssm_group_count.unwrap_or(1);
        inner
            .saturating_mul(state)
            .saturating_add(conv.saturating_mul(
                inner.saturating_add(groups.saturating_mul(state).saturating_mul(2)),
            ))
            .saturating_mul(4)
    }
}

/// Types de valeur GGUF.
const UINT8: u32 = 0;
const INT8: u32 = 1;
const UINT16: u32 = 2;
const INT16: u32 = 3;
const UINT32: u32 = 4;
const INT32: u32 = 5;
const FLOAT32: u32 = 6;
const BOOL: u32 = 7;
const STRING: u32 = 8;
const ARRAY: u32 = 9;
const UINT64: u32 = 10;
const INT64: u32 = 11;
const FLOAT64: u32 = 12;

/// Taille d'une valeur de type fixe.
fn fixed_size(kind: u32) -> Option<u64> {
    Some(match kind {
        UINT8 | INT8 | BOOL => 1,
        UINT16 | INT16 => 2,
        UINT32 | INT32 | FLOAT32 => 4,
        UINT64 | INT64 | FLOAT64 => 8,
        _ => return None,
    })
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> u64 {
        (self.bytes.len() - self.pos) as u64
    }

    /// `n` octets s'ils ont été reçus.
    fn take(&mut self, n: u64) -> Option<&'a [u8]> {
        if n > self.remaining() {
            return None;
        }
        let start = self.pos;
        self.pos += n as usize;
        Some(&self.bytes[start..self.pos])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    /// Chaîne sans copie ; `max` borne la longueur acceptée.
    fn string(&mut self, max: u64) -> Option<&'a [u8]> {
        let length = self.u64()?;
        if length > max {
            return None;
        }
        self.take(length)
    }

    /// Entier non négatif de n'importe quel type entier.
    fn integer(&mut self, kind: u32) -> Option<u64> {
        let bytes = self.take(fixed_size(kind)?)?;
        let value = match kind {
            UINT8 => u64::from(bytes[0]),
            INT8 => u64::try_from(bytes[0] as i8).ok()?,
            UINT16 => u64::from(u16::from_le_bytes(bytes.try_into().ok()?)),
            INT16 => u64::try_from(i16::from_le_bytes(bytes.try_into().ok()?)).ok()?,
            UINT32 => u64::from(u32::from_le_bytes(bytes.try_into().ok()?)),
            INT32 => u64::try_from(i32::from_le_bytes(bytes.try_into().ok()?)).ok()?,
            UINT64 => u64::from_le_bytes(bytes.try_into().ok()?),
            INT64 => u64::try_from(i64::from_le_bytes(bytes.try_into().ok()?)).ok()?,
            _ => return None,
        };
        Some(value)
    }

    /// Passe une valeur. `None` : tronquée ou malformée.
    fn skip(&mut self, kind: u32, depth: u32) -> Option<()> {
        if let Some(size) = fixed_size(kind) {
            self.take(size)?;
            return Some(());
        }
        match kind {
            STRING => {
                let length = self.u64()?;
                self.take(length)?;
            }
            ARRAY => {
                if depth >= MAX_DEPTH {
                    return None;
                }
                let element = self.u32()?;
                let count = self.u64()?;
                match fixed_size(element) {
                    Some(size) => {
                        self.take(count.checked_mul(size)?)?;
                    }
                    None => {
                        // Chaque élément consomme au moins 8 octets : la boucle
                        // s'arrête avec les octets reçus.
                        for _ in 0..count {
                            self.skip(element, depth + 1)?;
                        }
                    }
                }
            }
            _ => return None,
        }
        Some(())
    }

    /// Entier, ou tableau d'entiers ou de booléens d'au plus
    /// [`MAX_LAYERS`] éléments (plus long : passé, `Value::Skipped`).
    fn value(&mut self, kind: u32) -> Option<Value> {
        if kind != ARRAY {
            return Some(Value::Int(self.integer(kind)?));
        }
        let element = self.u32()?;
        let count = self.u64()?;
        let size = fixed_size(element)?;
        if count.checked_mul(size)? > self.remaining() {
            return None;
        }
        if count > MAX_LAYERS || matches!(element, FLOAT32 | FLOAT64) {
            self.take(count * size)?;
            return Some(Value::Skipped);
        }
        if element == BOOL {
            let bytes = self.take(count)?;
            return Some(Value::Bools(bytes.iter().map(|&byte| byte != 0).collect()));
        }
        let mut values = Vec::with_capacity(count as usize);
        for _ in 0..count {
            values.push(self.integer(element)?);
        }
        Some(Value::Ints(values))
    }
}

/// Valeur lue pour une clé utile.
#[derive(Clone, Debug)]
enum Value {
    Int(u64),
    Ints(Vec<u64>),
    Bools(Vec<bool>),
    Skipped,
}

impl Value {
    /// Valeur unique : l'entier, ou le plus grand d'un tableau.
    fn max(&self) -> Option<u64> {
        match self {
            Value::Int(value) => Some(*value),
            Value::Ints(values) => values.iter().copied().max(),
            Value::Bools(_) | Value::Skipped => None,
        }
    }
}

/// Clés d'architecture utiles (suffixe après `<arch>.`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wanted {
    BlockCount,
    Embedding,
    HeadCount,
    HeadCountKv,
    KeyLength,
    ValueLength,
    Context,
    Nextn,
    Interval,
    Window,
    WindowPattern,
    KeyLengthSwa,
    ValueLengthSwa,
    SsmInner,
    SsmState,
    SsmConv,
    SsmGroups,
}

const SUFFIXES: [(&str, Wanted); 17] = [
    ("block_count", Wanted::BlockCount),
    ("embedding_length", Wanted::Embedding),
    ("attention.head_count", Wanted::HeadCount),
    ("attention.head_count_kv", Wanted::HeadCountKv),
    ("attention.key_length", Wanted::KeyLength),
    ("attention.value_length", Wanted::ValueLength),
    ("context_length", Wanted::Context),
    ("nextn_predict_layers", Wanted::Nextn),
    ("full_attention_interval", Wanted::Interval),
    ("attention.sliding_window", Wanted::Window),
    ("attention.sliding_window_pattern", Wanted::WindowPattern),
    ("attention.key_length_swa", Wanted::KeyLengthSwa),
    ("attention.value_length_swa", Wanted::ValueLengthSwa),
    ("ssm.inner_size", Wanted::SsmInner),
    ("ssm.state_size", Wanted::SsmState),
    ("ssm.conv_kernel", Wanted::SsmConv),
    ("ssm.group_count", Wanted::SsmGroups),
];

/// Analyse le début d'un fichier GGUF. `None` : ce n'est pas un GGUF v2/v3,
/// ou l'architecture n'a pas été trouvée dans les octets reçus.
pub fn parse(bytes: &[u8]) -> Option<GgufFacts> {
    let mut reader = Reader { bytes, pos: 0 };
    if reader.take(4)? != b"GGUF" {
        return None;
    }
    let version = reader.u32()?;
    if !(2..=3).contains(&version) {
        return None;
    }
    let _tensors = reader.u64()?;
    let pairs = reader.u64()?.min(MAX_PAIRS);
    let mut facts = GgufFacts::default();
    // Valeurs d'architecture vues, peut-être avant `general.architecture` :
    // (préfixe, clé voulue, valeur). Au plus quelques dizaines.
    let mut seen: Vec<(String, Wanted, Value)> = Vec::new();
    for _ in 0..pairs {
        let Some(key) = reader.string(MAX_KEY) else {
            break;
        };
        let Some(kind) = reader.u32() else {
            break;
        };
        if key == b"general.architecture" && kind == STRING {
            match reader.string(MAX_ARCH as u64) {
                Some(name) => {
                    facts.architecture = std::str::from_utf8(name).ok().map(str::to_owned)
                }
                None => break,
            }
            continue;
        }
        let wanted = std::str::from_utf8(key).ok().and_then(|key| {
            SUFFIXES.iter().find_map(|(suffix, wanted)| {
                let prefix = key.strip_suffix(suffix)?.strip_suffix('.')?;
                (!prefix.is_empty() && !prefix.contains("..")).then_some((prefix, *wanted))
            })
        });
        let read = match wanted {
            Some((prefix, wanted)) if seen.len() < 64 => match reader.value(kind) {
                Some(value) => {
                    seen.push((prefix.to_owned(), wanted, value));
                    Some(())
                }
                None => None,
            },
            _ => reader.skip(kind, 0),
        };
        if read.is_none() {
            break;
        }
    }
    let architecture = facts.architecture.clone()?;
    for (prefix, wanted, value) in seen {
        if prefix != architecture {
            continue;
        }
        let max = value.max();
        match wanted {
            Wanted::BlockCount => facts.block_count = max,
            Wanted::Embedding => facts.embedding_length = max,
            Wanted::HeadCount => facts.head_count = max,
            Wanted::HeadCountKv => {
                facts.head_count_kv = max;
                if let Value::Ints(list) = value {
                    facts.head_count_kv_layers = Some(list);
                }
            }
            Wanted::KeyLength => facts.key_length = max,
            Wanted::ValueLength => facts.value_length = max,
            Wanted::Context => facts.context_length = max,
            Wanted::Nextn => facts.nextn_layers = max,
            Wanted::Interval => facts.full_attention_interval = max,
            Wanted::Window => facts.sliding_window = max,
            Wanted::WindowPattern => match value {
                Value::Int(period) => facts.sliding_window_period = Some(period),
                Value::Ints(list) => {
                    facts.sliding_window_layers = Some(list.iter().map(|&v| v != 0).collect())
                }
                Value::Bools(list) => facts.sliding_window_layers = Some(list),
                Value::Skipped => {}
            },
            Wanted::KeyLengthSwa => facts.key_length_swa = max,
            Wanted::ValueLengthSwa => facts.value_length_swa = max,
            Wanted::SsmInner => facts.ssm_inner_size = max,
            Wanted::SsmState => facts.ssm_state_size = max,
            Wanted::SsmConv => facts.ssm_conv_kernel = max,
            Wanted::SsmGroups => facts.ssm_group_count = max,
        }
    }
    Some(facts)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Petit écrivain GGUF pour les essais.
    pub struct Writer(pub Vec<u8>);

    impl Writer {
        pub fn new(pairs: u64) -> Self {
            let mut bytes = b"GGUF".to_vec();
            bytes.extend(3u32.to_le_bytes());
            bytes.extend(0u64.to_le_bytes());
            bytes.extend(pairs.to_le_bytes());
            Self(bytes)
        }

        pub fn key(&mut self, key: &str, kind: u32) -> &mut Self {
            self.0.extend((key.len() as u64).to_le_bytes());
            self.0.extend(key.as_bytes());
            self.0.extend(kind.to_le_bytes());
            self
        }

        pub fn string(&mut self, key: &str, value: &str) -> &mut Self {
            self.key(key, STRING);
            self.0.extend((value.len() as u64).to_le_bytes());
            self.0.extend(value.as_bytes());
            self
        }

        pub fn u32(&mut self, key: &str, value: u32) -> &mut Self {
            self.key(key, UINT32);
            self.0.extend(value.to_le_bytes());
            self
        }

        pub fn u32_array(&mut self, key: &str, values: &[u32]) -> &mut Self {
            self.key(key, ARRAY);
            self.0.extend(UINT32.to_le_bytes());
            self.0.extend((values.len() as u64).to_le_bytes());
            for value in values {
                self.0.extend(value.to_le_bytes());
            }
            self
        }

        pub fn bool_array(&mut self, key: &str, values: &[bool]) -> &mut Self {
            self.key(key, ARRAY);
            self.0.extend(BOOL.to_le_bytes());
            self.0.extend((values.len() as u64).to_le_bytes());
            self.0.extend(values.iter().map(|&value| u8::from(value)));
            self
        }

        pub fn tokens(&mut self, count: usize) -> &mut Self {
            self.key("tokenizer.ggml.tokens", ARRAY);
            self.0.extend(STRING.to_le_bytes());
            self.0.extend((count as u64).to_le_bytes());
            for index in 0..count {
                let token = format!("tok{index:05}");
                self.0.extend((token.len() as u64).to_le_bytes());
                self.0.extend(token.as_bytes());
            }
            self
        }
    }

    /// En-tête de qwen3:0.6b (valeurs de 34 § 9) : 28 couches, 8 têtes kv,
    /// clé de 128 alors que n_embd / n_head = 64.
    fn qwen3_0_6b() -> Vec<u8> {
        let mut writer = Writer::new(10);
        writer
            .string("general.architecture", "qwen3")
            .string("general.name", "Qwen3 0.6B")
            .u32("qwen3.block_count", 28)
            .u32("qwen3.context_length", 40960)
            .u32("qwen3.embedding_length", 1024)
            .u32("qwen3.attention.head_count", 16)
            .u32("qwen3.attention.head_count_kv", 8)
            .u32("qwen3.attention.key_length", 128)
            .u32("qwen3.attention.value_length", 128)
            .tokens(30_000);
        writer.0
    }

    #[test]
    fn the_architecture_keys_are_read() {
        let bytes = qwen3_0_6b();
        assert!(bytes.len() > HEADER_BYTES);
        let facts = parse(&bytes[..HEADER_BYTES]).unwrap();
        assert_eq!(facts.architecture.as_deref(), Some("qwen3"));
        assert_eq!(facts.block_count, Some(28));
        assert_eq!(facts.head_count_kv, Some(8));
        assert_eq!(facts.key_length, Some(128));
        assert_eq!(facts.context_length, Some(40960));
        // 2 × 28 × 8 × 128 × 2 octets par jeton : 3584 Mio à 32768 (34 § 9).
        assert_eq!(facts.kv_bytes(32768), Some(3584 * 1024 * 1024));
    }

    #[test]
    fn without_key_length_the_head_dimension_comes_from_the_embedding() {
        let mut writer = Writer::new(5);
        writer
            .string("general.architecture", "llama")
            .u32("llama.block_count", 30)
            .u32("llama.embedding_length", 576)
            .u32("llama.attention.head_count", 9)
            .u32("llama.attention.head_count_kv", 3);
        let facts = parse(&writer.0).unwrap();
        // SmolLM2-135M : 180 Mio à 8192 (34 § 9).
        assert_eq!(facts.kv_bytes(8192), Some(180 * 1024 * 1024));
    }

    const MIB: u64 = 1024 * 1024;

    /// En-tête réel d'unsloth/Qwen3.8-27B-GGUF (qwen35, lu le 5 octobre 2026) :
    /// une couche d'attention sur quatre, les autres récurrentes.
    fn qwen3_8_27b(interval: bool) -> Writer {
        let mut writer = Writer::new(if interval { 13 } else { 12 });
        writer
            .string("general.architecture", "qwen35")
            .u32("qwen35.block_count", 64)
            .u32("qwen35.context_length", 262_144)
            .u32("qwen35.embedding_length", 5120)
            .u32("qwen35.attention.head_count", 24)
            .u32("qwen35.attention.head_count_kv", 4)
            .u32("qwen35.attention.key_length", 256)
            .u32("qwen35.attention.value_length", 256)
            .u32("qwen35.ssm.conv_kernel", 4)
            .u32("qwen35.ssm.state_size", 128)
            .u32("qwen35.ssm.group_count", 16)
            .u32("qwen35.ssm.inner_size", 6144);
        if interval {
            writer.u32("qwen35.full_attention_interval", 4);
        }
        writer
    }

    #[test]
    fn recurrent_layers_keep_no_context() {
        // 16 couches d'attention × 4 têtes × (256 + 256) × 2 octets × 32768
        // jetons = 2 Gio, plus l'état fixe des 48 couches récurrentes
        // ((6144 × 128 + 3 × (6144 + 2 × 16 × 128)) × 4 octets chacune).
        let state = 48 * (6144 * 128 + 3 * (6144 + 2 * 16 * 128)) * 4;
        let expected = 2048 * MIB + state;
        let facts = parse(&qwen3_8_27b(true).0).unwrap();
        assert_eq!(facts.full_attention_interval, Some(4));
        assert_eq!(facts.kv_bytes(32768), Some(expected));
        // Sans la clé : intervalle par défaut de llama.cpp pour qwen35.
        let facts = parse(&qwen3_8_27b(false).0).unwrap();
        assert_eq!(facts.kv_bytes(32768), Some(expected));
    }

    #[test]
    fn per_layer_kv_heads_mark_layers_without_attention() {
        let mut writer = Writer::new(4);
        writer
            .string("general.architecture", "lfm2")
            .u32("lfm2.block_count", 4)
            .u32_array("lfm2.attention.head_count_kv", &[0, 8, 0, 4])
            .u32("lfm2.attention.key_length", 64);
        let facts = parse(&writer.0).unwrap();
        assert_eq!(facts.head_count_kv_layers, Some(vec![0, 8, 0, 4]));
        assert_eq!(facts.head_count_kv, Some(8));
        assert_eq!(facts.kv_bytes(1), Some(12 * 128 * 2));
        // Liste plus courte que les couches, ou aucune couche d'attention :
        // inconnu (pas zéro).
        let mut none = Writer::new(3);
        none.string("general.architecture", "mamba")
            .u32("mamba.block_count", 4)
            .u32_array("mamba.attention.head_count_kv", &[0, 0]);
        assert_eq!(parse(&none.0).unwrap().kv_bytes(1), None);
        let mut zeros = Writer::new(3);
        zeros
            .string("general.architecture", "mamba")
            .u32("mamba.block_count", 2)
            .u32_array("mamba.attention.head_count_kv", &[0, 0]);
        assert_eq!(parse(&zeros.0).unwrap().kv_bytes(1), None);
    }

    #[test]
    fn multi_token_layers_are_never_recurrent() {
        let mut writer = Writer::new(6);
        writer
            .string("general.architecture", "qwen35")
            .u32("qwen35.block_count", 5)
            .u32("qwen35.nextn_predict_layers", 1)
            .u32("qwen35.attention.head_count_kv", 1)
            .u32("qwen35.attention.key_length", 1)
            .u32("qwen35.full_attention_interval", 4);
        // Couche 3 (attention) et couche 4 (multi-jetons) : 2 × (1 + 1) × 2
        // octets ; les couches 0 à 2 sont récurrentes.
        assert_eq!(parse(&writer.0).unwrap().kv_bytes(1), Some(8));
    }

    /// En-tête réel de google/gemma-4-12B-it-qat-q4_0-gguf : cinq couches à
    /// fenêtre glissante (1024 jetons, têtes de 256) pour une globale (une
    /// tête de 512).
    #[test]
    fn sliding_window_layers_keep_the_window() {
        let heads: Vec<u32> = (0..48).map(|i| if i % 6 == 5 { 1 } else { 8 }).collect();
        let mut writer = Writer::new(10);
        writer
            .string("general.architecture", "gemma4")
            .u32("gemma4.block_count", 48)
            .u32("gemma4.embedding_length", 3840)
            .u32("gemma4.attention.head_count", 16)
            .u32_array("gemma4.attention.head_count_kv", &heads)
            .u32("gemma4.attention.key_length", 512)
            .u32("gemma4.attention.value_length", 512)
            .u32("gemma4.attention.sliding_window", 1024)
            .bool_array(
                "gemma4.attention.sliding_window_pattern",
                &(0..48).map(|i| i % 6 != 5).collect::<Vec<_>>(),
            )
            .u32("gemma4.attention.key_length_swa", 256);
        let facts = parse(&writer.0).unwrap();
        assert_eq!(facts.value_length_swa, None);
        // value_length_swa absent : celle des couches globales (512).
        // Globales : 8 × 1 × (512 + 512) × 2 × 32768 = 512 Mio. Fenêtre :
        // 40 × 8 × (256 + 512) × 2 × 2048 (1024 + lot de 1024).
        let expected = 512 * MIB + 40 * 8 * (256 + 512) * 2 * 2048;
        assert_eq!(facts.kv_bytes(32768), Some(expected));
        // Contexte plus court que la fenêtre : tout le contexte.
        assert_eq!(
            facts.kv_bytes(1024),
            Some(8 * 1024 * 2 * 1024 + 40 * 8 * 768 * 2 * 1024)
        );
    }

    /// En-tête réel de ggml-org/gpt-oss-20b-GGUF : fenêtre de 128 jetons, une
    /// couche sur deux (motif par défaut de llama.cpp, sans clé de motif).
    #[test]
    fn default_sliding_window_patterns() {
        let mut writer = Writer::new(7);
        writer
            .string("general.architecture", "gpt-oss")
            .u32("gpt-oss.block_count", 24)
            .u32("gpt-oss.attention.head_count", 64)
            .u32("gpt-oss.attention.head_count_kv", 8)
            .u32("gpt-oss.attention.key_length", 64)
            .u32("gpt-oss.attention.value_length", 64)
            .u32("gpt-oss.attention.sliding_window", 128);
        let facts = parse(&writer.0).unwrap();
        // 12 couches pleines, 12 à fenêtre de 128 + 1024 → 1280 (multiple de 256).
        let per_layer = 8 * 128 * 2;
        assert_eq!(
            facts.kv_bytes(32768),
            Some(12 * per_layer * 32768 + 12 * per_layer * 1280)
        );

        // Première couche dense (laguna, période 4) : couches 1 à 3 à fenêtre.
        let mut laguna = Writer::new(5);
        laguna
            .string("general.architecture", "laguna")
            .u32("laguna.block_count", 4)
            .u32("laguna.attention.head_count_kv", 1)
            .u32("laguna.attention.key_length", 1)
            .u32("laguna.attention.sliding_window", 1);
        // Contexte 4096 : fenêtre 1 + lot 512 → 768 jetons.
        assert_eq!(
            parse(&laguna.0).unwrap().kv_bytes(4096),
            Some(2 * 2 * 4096 + 3 * 2 * 2 * 768)
        );

        // Période donnée par le fichier (entier) ; sans fenêtre, aucun effet.
        let mut period = Writer::new(6);
        period
            .string("general.architecture", "custom")
            .u32("custom.block_count", 4)
            .u32("custom.attention.head_count_kv", 1)
            .u32("custom.attention.key_length", 1)
            .u32("custom.attention.sliding_window", 1)
            .u32("custom.attention.sliding_window_pattern", 2);
        assert_eq!(
            parse(&period.0).unwrap().kv_bytes(4096),
            Some(2 * 2 * 2 * 4096 + 2 * 2 * 2 * 768)
        );
        let mut no_window = Writer::new(5);
        no_window
            .string("general.architecture", "gemma3")
            .u32("gemma3.block_count", 6)
            .u32("gemma3.attention.head_count_kv", 1)
            .u32("gemma3.attention.key_length", 1)
            .u32("gemma3.attention.sliding_window_pattern", 6);
        assert_eq!(
            parse(&no_window.0).unwrap().kv_bytes(4096),
            Some(6 * 2 * 2 * 4096)
        );
        // Architecture sans motif connu : prudence, contexte entier partout.
        let mut unknown = Writer::new(5);
        unknown
            .string("general.architecture", "custom")
            .u32("custom.block_count", 2)
            .u32("custom.attention.head_count_kv", 1)
            .u32("custom.attention.key_length", 1)
            .u32("custom.attention.sliding_window", 1);
        assert_eq!(
            parse(&unknown.0).unwrap().kv_bytes(4096),
            Some(2 * 2 * 2 * 4096)
        );
    }

    #[test]
    fn keys_of_another_architecture_are_ignored() {
        let mut writer = Writer::new(3);
        writer
            .u32("llama.block_count", 99)
            .string("general.architecture", "qwen3")
            .u32("qwen3.block_count", 28);
        let facts = parse(&writer.0).unwrap();
        assert_eq!(facts.block_count, Some(28));
    }

    #[test]
    fn late_keys_and_other_files_give_nothing_useful() {
        // Vocabulaire avant l'architecture : coupé avant elle.
        let mut late = Writer::new(3);
        late.tokens(30_000)
            .string("general.architecture", "qwen3")
            .u32("qwen3.block_count", 28);
        assert_eq!(parse(&late.0[..HEADER_BYTES]), None);
        assert!(parse(&late.0).is_some());
        assert_eq!(parse(b""), None);
        assert_eq!(parse(b"<html>not a model</html>"), None);
        let mut v1 = b"GGUF".to_vec();
        v1.extend(1u32.to_le_bytes());
        v1.extend([0; 16]);
        assert_eq!(parse(&v1), None);
    }

    /// Chaque troncature de l'en-tête est sans danger, et garde ce qui a été
    /// lu avant la coupure.
    #[test]
    fn every_truncation_is_safe() {
        let bytes = qwen3_0_6b();
        for end in 0..2048 {
            let facts = parse(&bytes[..end]);
            if let Some(facts) = facts {
                assert_eq!(facts.architecture.as_deref(), Some("qwen3"));
            }
        }
        assert!(parse(&bytes[..400]).unwrap().block_count.is_some());
    }

    /// Longueurs et nombres énormes annoncés : arrêt sans allocation (essai
    /// lancé aussi sous `ulimit -v`).
    #[test]
    fn huge_announced_lengths_allocate_nothing() {
        let mut huge_pairs = Writer::new(u64::MAX);
        huge_pairs.string("general.architecture", "qwen3");
        assert_eq!(
            parse(&huge_pairs.0).unwrap().architecture.as_deref(),
            Some("qwen3")
        );

        let mut huge_string = Writer::new(3);
        huge_string.string("general.architecture", "qwen3");
        huge_string.key("general.name", STRING);
        huge_string.0.extend(u64::MAX.to_le_bytes());
        huge_string.0.extend(b"abc");
        assert!(parse(&huge_string.0).is_some());

        let mut huge_key = Writer::new(2);
        huge_key.string("general.architecture", "qwen3");
        huge_key.0.extend((u64::MAX / 2).to_le_bytes());
        assert!(parse(&huge_key.0).is_some());

        for element in [UINT64, STRING, ARRAY] {
            let mut huge_array = Writer::new(3);
            huge_array.string("general.architecture", "qwen3");
            huge_array.key("tokenizer.ggml.scores", ARRAY);
            huge_array.0.extend(element.to_le_bytes());
            huge_array.0.extend(u64::MAX.to_le_bytes());
            huge_array.0.extend([0; 64]);
            assert!(parse(&huge_array.0).is_some(), "{element}");
        }

        // Tableau de tableaux de tableaux... : profondeur bornée.
        let mut nested = Writer::new(2);
        nested.string("general.architecture", "qwen3");
        nested.key("x", ARRAY);
        for _ in 0..10 {
            nested.0.extend(ARRAY.to_le_bytes());
            nested.0.extend(1u64.to_le_bytes());
        }
        assert!(parse(&nested.0).is_some());

        // Tableau d'entiers énorme pour une clé lue.
        let mut kv = Writer::new(2);
        kv.string("general.architecture", "qwen3");
        kv.key("qwen3.attention.head_count_kv", ARRAY);
        kv.0.extend(UINT32.to_le_bytes());
        kv.0.extend(u64::MAX.to_le_bytes());
        let facts = parse(&kv.0).unwrap();
        assert_eq!(facts.head_count_kv, None);
        assert_eq!(facts.head_count_kv_layers, None);

        // Architecture interminable : refusée.
        let mut long = Writer::new(1);
        long.string("general.architecture", &"a".repeat(MAX_ARCH + 1));
        assert_eq!(parse(&long.0), None);
    }

    /// Octets altérés au hasard (suite déterministe) : jamais de panique.
    #[test]
    fn random_corruptions_are_safe() {
        let original = qwen3_0_6b();
        let mut state: u64 = 0x9e3779b97f4a7c15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..3000 {
            let length = (next() % 4096) as usize;
            let mut bytes = original[..length].to_vec();
            for _ in 0..(next() % 8) {
                if bytes.is_empty() {
                    break;
                }
                let index = (next() as usize) % bytes.len();
                bytes[index] = next() as u8;
            }
            let _ = parse(&bytes);
        }
    }

    #[test]
    fn negative_or_odd_integers_are_rejected() {
        let mut writer = Writer::new(3);
        writer.string("general.architecture", "llama");
        writer.key("llama.block_count", INT32);
        writer.0.extend((-5i32).to_le_bytes());
        writer.u32("llama.context_length", 4096);
        let facts = parse(&writer.0).unwrap();
        assert_eq!(facts.block_count, None);
        // La lecture s'arrête à la valeur refusée.
        assert_eq!(facts.context_length, None);
    }
}
