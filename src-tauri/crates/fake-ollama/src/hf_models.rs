//! Dépôts de modèles GGUF synthétisés par le faux Hugging Face, pour les
//! téléchargements directs du ModelHub (base locale des modèles de ChatonLM,
//! passage à llama-server) : chaque fichier a un vrai contenu (en-tête GGUF
//! lisible, puis octets déterministes), sa taille et son `lfs.sha256` exacts,
//! et se sert par `resolve/` avec `Range`.
//!
//! Espace `chatonlm-dl` : `chatonlm-dl/<Nom>[-<mot>…]-GGUF`, tout nom accepté
//! (pas de jeu de données). Fichiers : `<Nom>-Q4_K_M.gguf`, `<Nom>-Q8_0.gguf`
//! et `README.md` (hors LFS). Mots du dépôt (sans la casse) :
//!
//! - `vision` : projecteurs `mmproj-F16.gguf`, `mmproj-Q8_0.gguf`,
//!   `mmproj-BF16.gguf` (sans clé d'encodeur, comme les anciens) ;
//! - `audio` : les mêmes projecteurs, audio seulement
//!   (`clip.has_vision_encoder` faux, `clip.has_audio_encoder` vrai) ;
//! - `base` : poids d'un modèle de base (`general.basename`, sans
//!   `general.finetune`) ;
//! - `sharded` : la Q8_0 en deux morceaux (`-00001-of-00002`, `-00002-of-00002`) ;
//! - `sub` : la Q8_0 dans le sous-dossier `Q8_0/` ;
//! - `tools`, `think`, `fim` : gabarit qui parle d'outils, qui réfléchit,
//!   jetons FIM dans le vocabulaire ;
//! - `big` : poids huit fois plus gros (environ 400 Ko) ;
//! - `stall` : `resolve/` envoie la moitié du fichier, puis un octet toutes
//!   les 200 ms (un abandon du client se voit à l'écriture suivante) ;
//! - `mute` : `resolve/` envoie la moitié du fichier puis se tait ;
//! - `slow` : `resolve/` envoie des morceaux de 4 Kio toutes les 15 ms ;
//! - `cut` : `resolve/` envoie la moitié du fichier puis ferme ;
//! - `notgguf` : poids qui ne sont pas des GGUF ;
//! - `badsha` : `lfs.sha256` faux pour les poids ;
//! - `nolfs` : poids hors LFS (sans empreinte) ;
//! - `gated` : dépôt restreint (`gated: "manual"`, `resolve/` répond 401) ;
//! - `missing` : la fiche répond 401 comme un dépôt inexistant ;
//! - `norange` : `resolve/` ignore `Range` (toujours 200 et tout le fichier).
//!
//! Révision : `POST /fake/publish-hf {"repo"}` (ou `FakeHf::publish`) publie
//! une nouvelle version d'un dépôt : commit et contenu des poids changent ;
//! `resolve/<commit>/…` sert la révision de ce commit (une ancienne reste
//! servie), `resolve/main/…` la dernière.

use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// Espace des dépôts synthétisés.
pub const NAMESPACE: &str = "chatonlm-dl";

/// Taille de base des poids (octets, avant `big`).
const WEIGHTS: usize = 48 * 1024;

/// Dépôt synthétisé d'après son nom.
#[derive(Clone, Debug)]
pub struct Synth {
    pub id: String,
    name: String,
    words: Vec<String>,
    revision: u32,
}

impl Synth {
    /// Dépôt `id` (« chatonlm-dl/<Nom>-…-GGUF », sans la casse de l'espace) à
    /// la révision `revision` ; `None` pour un autre espace.
    pub fn parse(id: &str, revision: u32) -> Option<Self> {
        let (author, repo) = id.split_once('/')?;
        if !author.eq_ignore_ascii_case(NAMESPACE) || repo.is_empty() || repo.contains('/') {
            return None;
        }
        let words: Vec<String> = repo.split('-').map(str::to_ascii_lowercase).collect();
        let name = repo
            .split('-')
            .next()
            .filter(|first| !first.is_empty())
            .unwrap_or("Model")
            .to_owned();
        Some(Self {
            id: format!("{NAMESPACE}/{repo}"),
            name,
            words,
            revision,
        })
    }

    /// Numéro de la révision (0 : la première).
    pub fn revision(&self) -> u32 {
        self.revision
    }

    pub fn has(&self, word: &str) -> bool {
        self.words.iter().any(|known| known == word)
    }

    /// Commit de la révision.
    pub fn sha(&self) -> String {
        format!(
            "{:040x}",
            fnv(format!("{}#{}", self.id, self.revision).as_bytes())
        )
    }

    /// Chemins des fichiers, dans l'ordre de la fiche.
    pub fn paths(&self) -> Vec<String> {
        let name = &self.name;
        let mut paths = vec![format!("{name}-Q4_K_M.gguf")];
        if self.has("sharded") {
            paths.push(format!("{name}-Q8_0-00001-of-00002.gguf"));
            paths.push(format!("{name}-Q8_0-00002-of-00002.gguf"));
        } else if self.has("sub") {
            paths.push(format!("Q8_0/{name}-Q8_0.gguf"));
        } else {
            paths.push(format!("{name}-Q8_0.gguf"));
        }
        if self.has("vision") || self.has("audio") {
            for quant in ["F16", "Q8_0", "BF16"] {
                paths.push(format!("mmproj-{quant}.gguf"));
            }
        }
        paths.push("README.md".to_owned());
        paths
    }

    /// Contenu du fichier `path`, `None` s'il n'existe pas.
    pub fn content(&self, path: &str) -> Option<Vec<u8>> {
        if !self.paths().iter().any(|known| known == path) {
            return None;
        }
        if path == "README.md" {
            return Some(format!("# {}\n\nSynthetic model for tests.\n", self.id).into_bytes());
        }
        let basename = path.rsplit('/').next().unwrap_or(path);
        let projector = basename.starts_with("mmproj");
        let second_part = basename.contains("-00002-of-");
        let mut bytes = if self.has("notgguf") && !projector {
            b"PK\x03\x04 this is not a GGUF file".to_vec()
        } else if projector && self.has("audio") {
            self.gguf(&[
                ("general.architecture", Kv::Str("clip".into())),
                ("clip.has_vision_encoder", Kv::Bool(false)),
                ("clip.has_audio_encoder", Kv::Bool(true)),
            ])
        } else if projector {
            self.gguf(&[("general.architecture", Kv::Str("clip".into()))])
        } else if second_part {
            self.gguf(&[("split.no", Kv::U32(1)), ("split.count", Kv::U32(2))])
        } else {
            let quant = if basename.contains("Q8_0") { 7 } else { 15 };
            let mut pairs = vec![
                ("general.architecture", Kv::Str("qwen3".into())),
                ("general.name", Kv::Str(self.name.clone())),
                ("general.size_label", Kv::Str("0.6B".into())),
                ("general.file_type", Kv::U32(quant)),
                ("qwen3.context_length", Kv::U32(40960)),
            ];
            let mut tokens = vec!["<|im_start|>".to_owned(), "<|im_end|>".to_owned()];
            if self.has("fim") {
                for token in ["<|fim_prefix|>", "<|fim_suffix|>", "<|fim_middle|>"] {
                    tokens.push(token.to_owned());
                }
            }
            if self.has("base") {
                pairs.push(("general.basename", Kv::Str(self.name.clone())));
            }
            pairs.push(("tokenizer.ggml.tokens", Kv::Strs(tokens)));
            let mut template =
                "{%- for message in messages %}{{ message.content }}{%- endfor %}".to_owned();
            if self.has("tools") {
                template.push_str("{%- if tools %}<tool_call>{%- endif %}");
            }
            if self.has("think") {
                template.push_str("{%- if enable_thinking %}<think>{%- endif %}");
            }
            pairs.push(("tokenizer.chat_template", Kv::Str(template)));
            if basename.contains("-00001-of-") {
                pairs.push(("split.count", Kv::U32(2)));
            }
            self.gguf(&pairs)
        };
        // Octets déterministes jusqu'à la taille voulue (poids : différents à
        // chaque révision).
        let mut size = if projector { WEIGHTS / 2 } else { WEIGHTS };
        if self.has("big") && !projector {
            size *= 8;
        }
        size += basename.len() * 97;
        let revision = if projector { 0 } else { self.revision };
        let seed = fnv(format!("{}/{path}#{revision}", self.id).as_bytes());
        while bytes.len() < size {
            let index = bytes.len();
            bytes.push(((seed >> ((index % 8) * 8)) as u8) ^ (index as u8).wrapping_mul(31));
        }
        Some(bytes)
    }

    fn gguf(&self, pairs: &[(&str, Kv)]) -> Vec<u8> {
        let mut out = b"GGUF".to_vec();
        out.extend(3u32.to_le_bytes());
        out.extend(0u64.to_le_bytes());
        out.extend((pairs.len() as u64).to_le_bytes());
        let string = |out: &mut Vec<u8>, text: &str| {
            out.extend((text.len() as u64).to_le_bytes());
            out.extend(text.as_bytes());
        };
        for (key, value) in pairs {
            string(&mut out, key);
            match value {
                Kv::Str(text) => {
                    out.extend(8u32.to_le_bytes());
                    string(&mut out, text);
                }
                Kv::U32(number) => {
                    out.extend(4u32.to_le_bytes());
                    out.extend(number.to_le_bytes());
                }
                Kv::Bool(value) => {
                    out.extend(7u32.to_le_bytes());
                    out.push(u8::from(*value));
                }
                Kv::Strs(list) => {
                    out.extend(9u32.to_le_bytes());
                    out.extend(8u32.to_le_bytes());
                    out.extend((list.len() as u64).to_le_bytes());
                    for text in list {
                        string(&mut out, text);
                    }
                }
            }
        }
        out
    }

    /// Fiche de l'API (`blobs=true`) : commit, fichiers, tailles, empreintes.
    pub fn api(&self) -> Value {
        let siblings: Vec<Value> =
            std::iter::once(json!({ "rfilename": ".gitattributes", "size": 1519 }))
                .chain(self.paths().into_iter().map(|path| {
                    let bytes = self.content(&path).unwrap_or_default();
                    let weights = path.ends_with(".gguf") && !path.contains("mmproj");
                    let mut sha = sha256_hex(&bytes);
                    if weights && self.has("badsha") {
                        sha = "0".repeat(64);
                    }
                    if path == "README.md" || (weights && self.has("nolfs")) {
                        json!({ "rfilename": path, "size": bytes.len() })
                    } else {
                        json!({
                            "rfilename": path,
                            "size": bytes.len(),
                            "lfs": { "sha256": sha, "size": bytes.len(), "pointerSize": 134 }
                        })
                    }
                }))
                .collect();
        json!({
            "_id": format!("{:024x}", fnv(self.id.as_bytes())),
            "id": self.id,
            "author": NAMESPACE,
            "sha": self.sha(),
            "gated": if self.has("gated") { json!("manual") } else { json!(false) },
            "private": false,
            "downloads": 1,
            "likes": 0,
            "tags": ["gguf"],
            "siblings": siblings,
        })
    }
}

/// Valeur d'une paire GGUF.
enum Kv {
    Str(String),
    U32(u32),
    Bool(bool),
    Strs(Vec<String>),
}

/// Fiche d'un dépôt synthétisé.
pub fn detail(synth: &Synth) -> Response {
    if synth.has("missing") {
        return unauthorized("Invalid username or password.");
    }
    (
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        synth.api().to_string(),
    )
        .into_response()
}

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        json!({ "error": message }).to_string(),
    )
        .into_response()
}

/// Comportement du flux d'un fichier.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Flow {
    Whole,
    Slow,
    StallHalf,
    MuteHalf,
    CutHalf,
}

/// `GET /<id>/resolve/<révision>/<fichier>` d'un dépôt synthétisé ; `gone`
/// est appelé quand le client ferme avant la fin.
pub fn resolve(
    synth: &Synth,
    file: &str,
    headers: &HeaderMap,
    gone: impl FnOnce() + Send + 'static,
) -> Response {
    if synth.has("gated") || synth.has("missing") {
        return unauthorized(&format!("Access to model {} is restricted.", synth.id));
    }
    let Some(body) = synth.content(file) else {
        return (StatusCode::NOT_FOUND, "Entry not found").into_response();
    };
    let total = body.len();
    let start = if synth.has("norange") {
        None
    } else {
        headers
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("bytes="))
            .and_then(|value| value.strip_suffix('-'))
            .and_then(|start| start.parse::<usize>().ok())
    };
    if let Some(start) = start
        && start >= total
    {
        return (
            StatusCode::RANGE_NOT_SATISFIABLE,
            [(header::CONTENT_RANGE, format!("bytes */{total}"))],
        )
            .into_response();
    }
    let from = start.unwrap_or(0);
    let flow = if synth.has("stall") {
        Flow::StallHalf
    } else if synth.has("mute") {
        Flow::MuteHalf
    } else if synth.has("cut") {
        Flow::CutHalf
    } else if synth.has("slow") {
        Flow::Slow
    } else {
        Flow::Whole
    };
    let part = body[from..].to_vec();
    let length = part.len();
    let half = (total / 2).saturating_sub(from);
    let stream = futures_util::stream::unfold(
        (part, 0usize, Guard(Some(Box::new(gone)))),
        move |(part, sent, mut guard)| async move {
            let limit = match flow {
                Flow::StallHalf | Flow::MuteHalf | Flow::CutHalf => half.max(1).min(part.len()),
                Flow::Whole | Flow::Slow => part.len(),
            };
            if sent >= limit && sent < part.len() {
                match flow {
                    Flow::MuteHalf => {
                        std::future::pending::<()>().await;
                        return None;
                    }
                    Flow::StallHalf => {
                        // Un octet de temps en temps : jamais fini, mais une
                        // connexion fermée par le client se voit.
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        let chunk = part[sent..=sent].to_vec();
                        return Some((Ok(chunk), (part, sent + 1, guard)));
                    }
                    Flow::CutHalf => {
                        // Ce qui a été écrit part d'abord ; la coupure vient
                        // du serveur, pas d'un abandon du client.
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        guard.0.take();
                        return Some((
                            Err(std::io::Error::new(
                                std::io::ErrorKind::ConnectionReset,
                                "cut by the fake server",
                            )),
                            (part, usize::MAX, guard),
                        ));
                    }
                    Flow::Whole | Flow::Slow => {}
                }
            }
            if sent >= part.len() {
                guard.0.take();
                return None;
            }
            let step = if flow == Flow::Slow { 4096 } else { 64 * 1024 };
            if flow == Flow::Slow && sent > 0 {
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
            let end = (sent + step).min(limit.max(sent + 1)).min(part.len());
            let chunk = part[sent..end].to_vec();
            // Dernier morceau : le serveur n'en demandera pas d'autre (longueur
            // annoncée atteinte), ce n'est pas un abandon.
            if end >= part.len() {
                guard.0.take();
            }
            Some((Ok::<_, std::io::Error>(chunk), (part, end, guard)))
        },
    );
    let status = if start.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let mut response = (status, Body::from_stream(stream)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().expect("header"),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        length.to_string().parse().expect("header"),
    );
    if start.is_some() {
        headers.insert(
            header::CONTENT_RANGE,
            format!("bytes {from}-{}/{total}", total - 1)
                .parse()
                .expect("header"),
        );
    }
    response
}

/// Prévient quand le flux est abandonné avant sa fin.
struct Guard(Option<Box<dyn FnOnce() + Send>>);

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(gone) = self.0.take() {
            gone();
        }
    }
}

fn fnv(text: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_are_made_from_their_names() {
        assert!(Synth::parse("chatonlm/Tiny-GGUF", 0).is_none());
        let plain = Synth::parse("chatonlm-dl/Tiny-GGUF", 0).unwrap();
        assert_eq!(
            plain.paths(),
            ["Tiny-Q4_K_M.gguf", "Tiny-Q8_0.gguf", "README.md"]
        );
        let vision = Synth::parse("CHATONLM-DL/Eye-vision-sharded-GGUF", 0).unwrap();
        assert_eq!(vision.id, "chatonlm-dl/Eye-vision-sharded-GGUF");
        assert_eq!(vision.paths().len(), 7);
        let weights = vision.content("Eye-Q4_K_M.gguf").unwrap();
        assert_eq!(&weights[..4], b"GGUF");
        assert_eq!(vision.content("Eye-Q4_K_M.gguf").unwrap(), weights);
        assert!(vision.content("nope.gguf").is_none());
        // Nouvelle révision : autre commit, autres poids, même projecteur.
        let next = Synth::parse("chatonlm-dl/Eye-vision-sharded-GGUF", 1).unwrap();
        assert_ne!(next.sha(), vision.sha());
        assert_ne!(next.content("Eye-Q4_K_M.gguf").unwrap(), weights);
        assert_eq!(
            next.content("mmproj-F16.gguf"),
            vision.content("mmproj-F16.gguf")
        );
        let api = vision.api();
        let sibling = &api["siblings"][1];
        assert_eq!(sibling["size"].as_u64(), Some(weights.len() as u64));
        assert_eq!(
            sibling["lfs"]["sha256"].as_str(),
            Some(sha256_hex(&weights).as_str())
        );
    }
}
