//! Le blob des poids d'un modèle d'Ollama se charge-t-il dans llama-server ?
//! Les modèles servis par le moteur propre d'Ollama (gemma3 4b et plus,
//! mistral3, qwen3.5 en variante `ggml`…) ont un GGUF à sa façon : la vision
//! y est fondue (tenseurs `v.*` et `mm.*` avec les poids du texte), des clés
//! manquent ou changent de forme. llama.cpp les refuse (relevés de
//! `99-llamacpp-preuve.md` § 2 sur b12110 : `wrong number of tensors`,
//! `key not found in model: gemma3.attention.layer_norm_rms_epsilon`,
//! `qwen35.rope.dimension_sections has wrong array length; expected 4, got
//! 3`). Les en-têtes réels des blobs de l'Ollama de test le confirment : les
//! six fichiers refusés ont des tenseurs `v.`/`mm.`, aucun fichier accepté
//! n'en a.
//!
//! L'architecture doit être de celles que charge le llama-server livré
//! (liste blanche [`ARCHITECTURES`], relevée dans les sources de b12110) ;
//! les modèles d'embeddings (`bert`, `nomic-bert`, clé `<arch>.pooling_type`
//! …) et ceux qui ne sont pas des modèles de conversation (voix, têtes de
//! brouillon) ne sont pas repris : ils se chargeraient sans pouvoir parler.
//!
//! Lecture bornée d'un fichier non fiable : clés et valeurs utiles seulement,
//! longueurs comparées au reste du fichier, valeurs sautées sans lecture,
//! nombres de paires et de tenseurs plafonnés.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek};
use std::path::Path;

/// Paires lues au plus.
const MAX_PAIRS: u64 = 4096;
/// Tenseurs lus au plus (au-delà : jugement sur ceux lus).
const MAX_TENSORS: u64 = 1 << 16;
/// Longueur maximale d'une clé ou d'un nom de tenseur.
const MAX_NAME: u64 = 1024;
/// Dimensions d'un tenseur au plus (GGML_MAX_DIMS = 4).
const MAX_DIMS: u32 = 8;
/// Longueur maximale d'un nom d'architecture.
const MAX_ARCH: u64 = 128;

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

/// Architectures du llama-server livré (`llama_archs.txt`).
const ARCHITECTURES: &str = include_str!("llama_archs.txt");

/// Architectures d'embeddings (aucune réponse de conversation).
const EMBEDDINGS: [&str; 12] = [
    "bert",
    "modern-bert",
    "nomic-bert",
    "nomic-bert-moe",
    "neo-bert",
    "jina-bert-v2",
    "jina-bert-v3",
    "eurobert",
    "t5encoder",
    "gemma-embedding",
    "gemma-embedding2",
    "llama-embed",
];

/// Architectures chargées par llama.cpp qui ne sont pas des modèles de
/// conversation : voix, décodeur audio, têtes de brouillon.
const NOT_CHAT: [&str; 5] = [
    "wavtokenizer-dec",
    "qwen3tts",
    "pockettts",
    "eagle3",
    "dflash",
];

/// Ce qu'une architecture donne dans le chat de ChatonLM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// Modèle de conversation que llama.cpp charge.
    Chat,
    /// Modèle d'embeddings.
    Embedding,
    /// Architecture que le llama-server livré ne charge pas, ou qui ne
    /// converse pas.
    Unsupported,
}

/// Jugement d'une architecture (`None` : pas de `general.architecture`).
pub fn support(arch: Option<&str>) -> Support {
    let Some(arch) = arch else {
        return Support::Unsupported;
    };
    if EMBEDDINGS.contains(&arch) {
        return Support::Embedding;
    }
    let known = ARCHITECTURES
        .lines()
        .map(str::trim)
        .any(|line| !line.starts_with('#') && line == arch);
    if !known || NOT_CHAT.contains(&arch) {
        return Support::Unsupported;
    }
    Support::Chat
}

/// Raison pour laquelle llama.cpp ne chargera pas le fichier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incompatibility {
    /// Encodeur de vision fondu dans les poids (tenseurs `v.*`, `mm.*`).
    FusedVision,
    /// Clé exigée par llama.cpp absente (`<arch>.…`).
    MissingKey(String),
    /// `<arch>.rope.dimension_sections` d'une autre longueur que 4.
    RopeSections(u64),
}

/// Ce qui a été lu.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub architecture: Option<String>,
    /// `general.file_type` (`llama_ftype`).
    pub file_type: Option<u32>,
    pub tensors: u64,
    pub problem: Option<Incompatibility>,
    /// Gabarit de conversation (`tokenizer.chat_template`) ; vrai aussi
    /// quand l'en-tête n'a pas été lu en entier (rien à signaler).
    pub chat_template: bool,
    /// `<arch>.pooling_type` : modèle d'embeddings.
    pub pooling: bool,
}

impl Report {
    /// Ce que l'architecture donne dans le chat (embeddings aussi d'après
    /// `pooling_type`).
    pub fn support(&self) -> Support {
        match support(self.architecture.as_deref()) {
            Support::Chat if self.pooling => Support::Embedding,
            other => other,
        }
    }
}

/// Échec de la lecture.
#[derive(Debug)]
pub enum CompatError {
    NotGguf,
    Io(io::Error),
}

/// Lit l'en-tête du fichier `path`.
pub fn check(path: &Path) -> Result<Report, CompatError> {
    let file = File::open(path).map_err(CompatError::Io)?;
    let len = file.metadata().map_err(CompatError::Io)?.len();
    check_from(file, len)
}

/// Clés que llama.cpp exige pour une architecture et que les fichiers du
/// moteur d'Ollama n'ont pas.
fn required_keys(arch: &str) -> &'static [&'static str] {
    match arch {
        "gemma3" => &["attention.layer_norm_rms_epsilon"],
        _ => &[],
    }
}

/// Architectures où `rope.dimension_sections` compte 4 sections (M-RoPE de
/// llama.cpp).
fn four_rope_sections(arch: &str) -> bool {
    matches!(
        arch,
        "qwen35" | "qwen35moe" | "qwen3vl" | "qwen3vlmoe" | "qwen2vl"
    )
}

/// Lit l'en-tête d'un GGUF de `len` octets.
pub fn check_from<R: Read + Seek>(reader: R, len: u64) -> Result<Report, CompatError> {
    let mut stream = Stream {
        inner: BufReader::with_capacity(64 * 1024, reader),
        pos: 0,
        len,
    };
    if stream.bytes(4)?.as_slice() != b"GGUF" {
        return Err(CompatError::NotGguf);
    }
    let version = stream.u32()?;
    if !(2..=3).contains(&version) {
        return Err(CompatError::NotGguf);
    }
    let tensors = stream.u64()?;
    let pairs = stream.u64()?;
    let mut report = Report {
        tensors,
        ..Report::default()
    };
    let mut keys: Vec<String> = Vec::new();
    let mut sections: Vec<(String, u64)> = Vec::new();
    let mut complete = pairs <= MAX_PAIRS;
    for _ in 0..pairs.min(MAX_PAIRS) {
        let Ok(key) = stream.string(MAX_NAME) else {
            complete = false;
            break;
        };
        let kind = stream.u32()?;
        if key == "general.architecture" && kind == STRING {
            report.architecture = Some(stream.string(MAX_ARCH)?);
        } else if key == "general.file_type" && kind == UINT32 {
            report.file_type = Some(stream.u32()?);
        } else if let Some(prefix) = key.strip_suffix(".rope.dimension_sections")
            && kind == ARRAY
        {
            let element = stream.u32()?;
            let count = stream.u64()?;
            sections.push((prefix.to_owned(), count));
            stream.skip_elements(element, count, 0)?;
        } else {
            stream.skip_value(kind, 0)?;
        }
        keys.push(key);
    }
    let arch = report.architecture.clone().unwrap_or_default();
    report.chat_template = !complete || keys.iter().any(|key| key == "tokenizer.chat_template");
    report.pooling = keys.contains(&format!("{arch}.pooling_type"));
    // Clés : jugées seulement si toutes ont été lues.
    if complete {
        for wanted in required_keys(&arch) {
            let full = format!("{arch}.{wanted}");
            if !keys.contains(&full) {
                report.problem = Some(Incompatibility::MissingKey(full));
            }
        }
    }
    if four_rope_sections(&arch)
        && let Some(&(_, count)) = sections.iter().find(|(prefix, _)| *prefix == arch)
        && count != 4
    {
        report.problem = Some(Incompatibility::RopeSections(count));
    }
    if complete {
        for _ in 0..tensors.min(MAX_TENSORS) {
            let Ok(name) = stream.string(MAX_NAME) else {
                break;
            };
            if name.starts_with("v.") || name.starts_with("mm.") {
                report.problem = Some(Incompatibility::FusedVision);
                break;
            }
            let dims = stream.u32()?;
            if dims > MAX_DIMS {
                break;
            }
            // Dimensions, type, décalage.
            stream.skip(u64::from(dims) * 8 + 4 + 8)?;
        }
    }
    Ok(report)
}

struct Stream<R> {
    inner: BufReader<R>,
    pos: u64,
    len: u64,
}

impl<R: Read + Seek> Stream<R> {
    fn need(&self, count: u64) -> Result<(), CompatError> {
        if self.len.saturating_sub(self.pos) < count {
            return Err(CompatError::Io(io::ErrorKind::UnexpectedEof.into()));
        }
        Ok(())
    }

    fn bytes(&mut self, count: usize) -> Result<Vec<u8>, CompatError> {
        self.need(count as u64)?;
        let mut buffer = vec![0u8; count];
        self.inner
            .read_exact(&mut buffer)
            .map_err(CompatError::Io)?;
        self.pos += count as u64;
        Ok(buffer)
    }

    fn u32(&mut self) -> Result<u32, CompatError> {
        let bytes = self.bytes(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap_or_default()))
    }

    fn u64(&mut self) -> Result<u64, CompatError> {
        let bytes = self.bytes(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().unwrap_or_default()))
    }

    fn skip(&mut self, count: u64) -> Result<(), CompatError> {
        self.need(count)?;
        let offset = i64::try_from(count).map_err(|_| CompatError::NotGguf)?;
        self.inner.seek_relative(offset).map_err(CompatError::Io)?;
        self.pos += count;
        Ok(())
    }

    /// Chaîne de `max` octets au plus (plus longue : erreur).
    fn string(&mut self, max: u64) -> Result<String, CompatError> {
        let len = self.u64()?;
        if len > max {
            return Err(CompatError::NotGguf);
        }
        let bytes = self.bytes(len as usize)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn skip_value(&mut self, kind: u32, depth: u32) -> Result<(), CompatError> {
        match kind {
            STRING => {
                let len = self.u64()?;
                self.skip(len)
            }
            ARRAY => {
                if depth >= 3 {
                    return Err(CompatError::NotGguf);
                }
                let element = self.u32()?;
                let count = self.u64()?;
                self.skip_elements(element, count, depth + 1)
            }
            kind => self.skip(fixed_size(kind).ok_or(CompatError::NotGguf)?),
        }
    }

    fn skip_elements(&mut self, element: u32, count: u64, depth: u32) -> Result<(), CompatError> {
        if let Some(size) = fixed_size(element) {
            return self.skip(count.checked_mul(size).ok_or(CompatError::NotGguf)?);
        }
        // Chaque élément occupe au moins 8 octets : un nombre impossible est
        // refusé avant la boucle.
        self.need(count.saturating_mul(8))?;
        for _ in 0..count {
            self.skip_value(element, depth)?;
        }
        Ok(())
    }
}

fn fixed_size(kind: u32) -> Option<u64> {
    Some(match kind {
        UINT8 | INT8 | BOOL => 1,
        UINT16 | INT16 => 2,
        UINT32 | INT32 | FLOAT32 => 4,
        UINT64 | INT64 | FLOAT64 => 8,
        _ => return None,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;

    /// En-tête GGUF v3 : paires (clé, type, octets de la valeur), puis
    /// tenseurs (nom, dimensions).
    pub(crate) fn gguf(pairs: &[(&str, u32, Vec<u8>)], tensors: &[(&str, u32)]) -> Vec<u8> {
        let mut out = b"GGUF".to_vec();
        out.extend(3u32.to_le_bytes());
        out.extend((tensors.len() as u64).to_le_bytes());
        out.extend((pairs.len() as u64).to_le_bytes());
        let string = |out: &mut Vec<u8>, text: &str| {
            out.extend((text.len() as u64).to_le_bytes());
            out.extend(text.as_bytes());
        };
        for (key, kind, value) in pairs {
            string(&mut out, key);
            out.extend(kind.to_le_bytes());
            out.extend(value);
        }
        for (name, dims) in tensors {
            string(&mut out, name);
            out.extend(dims.to_le_bytes());
            for _ in 0..*dims {
                out.extend(16u64.to_le_bytes());
            }
            out.extend(0u32.to_le_bytes());
            out.extend(0u64.to_le_bytes());
        }
        out.extend([0u8; 64]);
        out
    }

    pub(crate) fn text(value: &str) -> Vec<u8> {
        let mut out = (value.len() as u64).to_le_bytes().to_vec();
        out.extend(value.as_bytes());
        out
    }

    pub(crate) fn u32_array(values: &[u32]) -> Vec<u8> {
        let mut out = UINT32.to_le_bytes().to_vec();
        out.extend((values.len() as u64).to_le_bytes());
        for value in values {
            out.extend(value.to_le_bytes());
        }
        out
    }

    /// Poids d'un modèle de conversation (`arch`), gabarit qui parle
    /// d'outils, `extra` tenseurs en plus des siens ; `salt` change le
    /// contenu (empreintes distinctes).
    pub(crate) fn weights(arch: &str, extra: &[&str], salt: u8) -> Vec<u8> {
        let mut tensors: Vec<(&str, u32)> =
            vec![("token_embd.weight", 2), ("blk.0.attn_q.weight", 2)];
        tensors.extend(extra.iter().map(|name| (*name, 2)));
        let context = format!("{arch}.context_length");
        let mut bytes = gguf(
            &[
                ("general.architecture", STRING, text(arch)),
                ("general.size_label", STRING, text("0.6B")),
                ("general.file_type", UINT32, 15u32.to_le_bytes().to_vec()),
                (&context, UINT32, 40960u32.to_le_bytes().to_vec()),
                (
                    "tokenizer.chat_template",
                    STRING,
                    text("{% if tools %}{{ tools }}{% endif %}{{ messages }}"),
                ),
            ],
            &tensors,
        );
        bytes.extend(std::iter::repeat_n(salt, 4096));
        bytes
    }

    /// Projecteur de vision (`clip.has_vision_encoder`).
    pub(crate) fn projector(salt: u8) -> Vec<u8> {
        let mut bytes = gguf(
            &[
                ("general.architecture", STRING, text("clip")),
                ("clip.has_vision_encoder", BOOL, vec![1]),
            ],
            &[("v.patch_embd.weight", 2)],
        );
        bytes.extend(std::iter::repeat_n(salt, 2048));
        bytes
    }

    /// Architectures relevées dans les sources de b12110 (I6 de la
    /// relecture L6) : llama3.2-vision (« mllama ») n'y est pas ; les
    /// embeddings et les voix ne conversent pas.
    #[test]
    fn only_what_the_shipped_llama_server_loads_is_a_chat_model() {
        for arch in [
            "llama", "qwen3", "qwen35", "gemma3", "gpt-oss", "mistral3", "smollm3",
        ] {
            assert_eq!(support(Some(arch)), Support::Chat, "{arch}");
        }
        for arch in ["mllama", "gemma3n-ollama", "", "# Architectures", "LLAMA"] {
            assert_eq!(support(Some(arch)), Support::Unsupported, "{arch}");
        }
        assert_eq!(support(None), Support::Unsupported);
        for arch in ["bert", "nomic-bert", "t5encoder", "gemma-embedding"] {
            assert_eq!(support(Some(arch)), Support::Embedding, "{arch}");
        }
        assert_eq!(support(Some("pockettts")), Support::Unsupported);
        // Toutes les embeddings et les « pas de conversation » sont bien des
        // architectures de la liste (aucune faute de frappe).
        for arch in EMBEDDINGS.iter().chain(&NOT_CHAT) {
            assert!(ARCHITECTURES.lines().any(|line| line == *arch), "{arch}");
        }
        // En-têtes synthétiques : mllama, qwen3 d'embeddings (pooling_type),
        // gabarit absent.
        let mllama = gguf(
            &[("general.architecture", STRING, text("mllama"))],
            &[("blk.0.attn_q.weight", 2)],
        );
        let report = read(&mllama).unwrap();
        assert_eq!(report.support(), Support::Unsupported);
        assert!(!report.chat_template);
        let embedding = gguf(
            &[
                ("general.architecture", STRING, text("qwen3")),
                ("qwen3.pooling_type", UINT32, 3u32.to_le_bytes().to_vec()),
            ],
            &[("blk.0.attn_q.weight", 2)],
        );
        assert_eq!(read(&embedding).unwrap().support(), Support::Embedding);
        let chat = weights("qwen3", &[], 0);
        let report = read(&chat).unwrap();
        assert_eq!(report.support(), Support::Chat);
        assert!(report.chat_template);
    }

    fn read(bytes: &[u8]) -> Result<Report, CompatError> {
        check_from(Cursor::new(bytes.to_vec()), bytes.len() as u64)
    }

    #[test]
    fn a_plain_text_model_loads() {
        let bytes = gguf(
            &[
                ("general.architecture", STRING, text("qwen3")),
                (
                    "qwen3.context_length",
                    UINT32,
                    40960u32.to_le_bytes().to_vec(),
                ),
                ("general.file_type", UINT32, 15u32.to_le_bytes().to_vec()),
                ("tokenizer.ggml.tokens", ARRAY, {
                    let mut out = STRING.to_le_bytes().to_vec();
                    out.extend(2u64.to_le_bytes());
                    out.extend(text("a"));
                    out.extend(text("b"));
                    out
                }),
            ],
            &[
                ("token_embd.weight", 2),
                ("blk.0.attn_q.weight", 2),
                ("output.weight", 2),
            ],
        );
        let report = read(&bytes).unwrap();
        assert_eq!(report.architecture.as_deref(), Some("qwen3"));
        assert_eq!(report.tensors, 3);
        assert_eq!(report.file_type, Some(15));
        assert_eq!(report.problem, None);
    }

    #[test]
    fn ollama_formats_are_recognised() {
        // gemma3 4b d'Ollama : vision fondue, clé absente.
        let gemma = gguf(
            &[("general.architecture", STRING, text("gemma3"))],
            &[("blk.0.attn_q.weight", 2), ("v.blk.0.attn_q.weight", 2)],
        );
        assert_eq!(
            read(&gemma).unwrap().problem,
            Some(Incompatibility::FusedVision)
        );
        // gemma3 270m d'Ollama : texte seul, clé présente → chargé.
        let small = gguf(
            &[
                ("general.architecture", STRING, text("gemma3")),
                (
                    "gemma3.attention.layer_norm_rms_epsilon",
                    FLOAT32,
                    1e-6f32.to_le_bytes().to_vec(),
                ),
            ],
            &[("blk.0.attn_q.weight", 2)],
        );
        assert_eq!(read(&small).unwrap().problem, None);
        let keyless = gguf(
            &[("general.architecture", STRING, text("gemma3"))],
            &[("blk.0.attn_q.weight", 2)],
        );
        assert_eq!(
            read(&keyless).unwrap().problem,
            Some(Incompatibility::MissingKey(
                "gemma3.attention.layer_norm_rms_epsilon".into()
            ))
        );
        // qwen3.5 variante `ggml` : trois sections ; variante `llamacpp` : quatre.
        let ggml = gguf(
            &[
                ("general.architecture", STRING, text("qwen35")),
                (
                    "qwen35.rope.dimension_sections",
                    ARRAY,
                    u32_array(&[11, 11, 10]),
                ),
            ],
            &[("blk.0.attn_q.weight", 2)],
        );
        assert_eq!(
            read(&ggml).unwrap().problem,
            Some(Incompatibility::RopeSections(3))
        );
        let llamacpp = gguf(
            &[
                ("general.architecture", STRING, text("qwen35")),
                (
                    "qwen35.rope.dimension_sections",
                    ARRAY,
                    u32_array(&[11, 11, 10, 0]),
                ),
            ],
            &[("blk.0.attn_q.weight", 2)],
        );
        assert_eq!(read(&llamacpp).unwrap().problem, None);
        // Projecteur fondu de mistral3.
        let mistral = gguf(
            &[("general.architecture", STRING, text("mistral3"))],
            &[("blk.0.attn_q.weight", 2), ("mm.linear_1.weight", 2)],
        );
        assert_eq!(
            read(&mistral).unwrap().problem,
            Some(Incompatibility::FusedVision)
        );
    }

    #[test]
    fn truncated_or_hostile_headers_never_panic() {
        let bytes = gguf(
            &[
                ("general.architecture", STRING, text("gemma3")),
                ("x", ARRAY, u32_array(&[1, 2, 3])),
            ],
            &[("v.blk.0.weight", 2)],
        );
        for cut in 0..bytes.len() {
            let _ = read(&bytes[..cut]);
        }
        assert!(matches!(
            read(b"PK\x03\x04 not gguf"),
            Err(CompatError::NotGguf)
        ));
        // Longueur énorme d'une chaîne, nombre d'éléments impossible.
        let mut huge = gguf(&[], &[]);
        huge.truncate(24);
        huge[16..24].copy_from_slice(&1u64.to_le_bytes());
        huge.extend(u64::MAX.to_le_bytes());
        // Clé illisible : lecture arrêtée, rien de jugé, pas d'allocation.
        assert_eq!(read(&huge).unwrap().problem, None);
        let mut elements = gguf(&[], &[]);
        elements.truncate(24);
        elements[16..24].copy_from_slice(&1u64.to_le_bytes());
        elements.extend(text("k"));
        elements.extend(ARRAY.to_le_bytes());
        elements.extend(STRING.to_le_bytes());
        elements.extend((u64::MAX / 2).to_le_bytes());
        assert!(read(&elements).is_err());
    }
}
