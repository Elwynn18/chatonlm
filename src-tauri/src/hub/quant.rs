// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2023 Hugging Face
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Partly translated to Rust from huggingface.js
// (https://github.com/huggingface/huggingface.js), file
// packages/tasks/src/gguf.ts, commit 22d0f2ab0849: GGUF_QUANT_RE and
// parseGGUFQuantLabel, the quantization label that the Hugging Face registry
// reads from an Ollama tag.
// Modified for ChatonLM: a label becomes a tag only when no other version of the
// repository could answer it (otherwise the full file name), and a second,
// wider pattern names versions for display.

//! Versions (quantifications) d'un dépôt GGUF : inventaire des fichiers,
//! libellés, étiquettes Ollama, qualité et version conseillée.
//! Règles : `.tools/research/35-modelhub-hf-api.md` § 2.2 à 2.4 et R5,
//! `34-modelhub-ollama-hf.md` § 2-4.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use super::names;
use super::types::{HubQuantIssue, HubQuantTier};

/// Noms de l'énumération `GGMLFileQuantizationType` de huggingface.js, dans
/// l'ordre de déclaration, puis l'alias `MXFP4` (dernier, pour que
/// `MXFP4_MOE` l'emporte).
const HF_QUANTS: &[&str] = &[
    "F32",
    "F16",
    "Q4_0",
    "Q4_1",
    "Q4_1_SOME_F16",
    "Q4_2",
    "Q4_3",
    "Q8_0",
    "Q5_0",
    "Q5_1",
    "Q2_K",
    "Q3_K_S",
    "Q3_K_M",
    "Q3_K_L",
    "Q4_K_S",
    "Q4_K_M",
    "Q5_K_S",
    "Q5_K_M",
    "Q6_K",
    "IQ2_XXS",
    "IQ2_XS",
    "Q2_K_S",
    "IQ3_XS",
    "IQ3_XXS",
    "IQ1_S",
    "IQ4_NL",
    "IQ3_S",
    "IQ3_M",
    "IQ2_S",
    "IQ2_M",
    "IQ4_XS",
    "IQ1_M",
    "BF16",
    "Q4_0_4_4",
    "Q4_0_4_8",
    "Q4_0_8_8",
    "TQ1_0",
    "TQ2_0",
    "MXFP4_MOE",
    "NVFP4",
    "Q1_0",
    "Q2_0",
    "Q2_K_XL",
    "Q3_K_XL",
    "Q4_K_XL",
    "Q5_K_XL",
    "Q6_K_XL",
    "Q8_K_XL",
    "MXFP4",
];

/// `GGUF_QUANT_RE` de huggingface.js. Le crate `regex` choisit comme
/// JavaScript la première alternative qui réussit (« leftmost-first ») : mêmes
/// correspondances.
static HF_QUANT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!("(UD-)?({})(_([A-Z]+))?", HF_QUANTS.join("|")))
        .expect("valid quantization pattern")
});

/// Motif d'affichage de ChatonLM (35 § 2.3) : reconnaît aussi `Q4_K_L`,
/// `fp16`, `Q4_0_4_4`... Les bornes sont vérifiées à part (pas de lookaround
/// dans le crate `regex`).
static DISPLAY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(UD-)?(IQ[1-4]_(?:XXS|XS|NL|S|M)|Q[1-8]_K(?:_(?:XXL|XL|L|M|S|P))?|Q[1-8]_[01](?:_[48]_[48])?|TQ[12]_0|MXFP4(?:_MOE)?|NVFP4|BF16|FP?16|FP?32)",
    )
    .expect("valid display pattern")
});

/// Fichiers auxiliaires, jamais proposés seuls (35 R5) : projecteur de vision,
/// têtes MTP, brouillons spéculatifs, matrice d'importance.
static AUXILIARY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(^|/)(mmproj|mtp|eagle3?|draft|imatrix)[^/]*$|imatrix|mmproj|fastmtp")
        .expect("valid auxiliary pattern")
});

/// Morceau d'un fichier découpé : `-00001-of-00003.gguf`.
static SHARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)-(\d{5})-of-(\d{5})\.gguf$").expect("valid shard pattern"));

/// Nom de fichier, sans le chemin.
pub fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Fichier GGUF (extension sans tenir compte de la casse). Comparaison des
/// octets : un nom venu de Hugging Face peut finir par un caractère de
/// plusieurs octets (« 说明.md »), où couper la chaîne paniquerait.
pub fn is_gguf(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() > 5 && bytes[bytes.len() - 5..].eq_ignore_ascii_case(b".gguf")
}

/// Fichier auxiliaire (projecteur, MTP, EAGLE, brouillon, imatrix).
pub fn is_auxiliary(path: &str) -> bool {
    AUXILIARY_RE.is_match(basename(path))
}

/// Projecteur de vision (`mmproj`).
pub fn is_projector(path: &str) -> bool {
    is_gguf(path) && basename(path).to_ascii_lowercase().contains("mmproj")
}

/// `(chemin sans « -0000N-of-0000M.gguf », N)` pour un morceau de fichier
/// découpé.
fn shard(path: &str) -> Option<(&str, u32)> {
    let captures = SHARD_RE.captures(path)?;
    let index = captures.get(1)?.as_str().parse().ok()?;
    Some((&path[..captures.get(0)?.start()], index))
}

/// Libellé que le registre de Hugging Face lit dans une étiquette :
/// `parseGGUFQuantLabel` (nom en majuscules, dernière occurrence).
pub fn hf_label(file: &str) -> Option<String> {
    let upper = file.to_ascii_uppercase();
    HF_QUANT_RE
        .find_iter(&upper)
        .last()
        .map(|found| found.as_str().to_owned())
}

/// Libellé affiché d'un fichier : motif d'affichage sur le nom sans `.gguf`
/// ni suffixe de morceau, en majuscules, dernière occurrence non collée à une
/// lettre ou un chiffre ; `FP16`/`FP32` deviennent `F16`/`F32`.
pub fn display_label(path: &str) -> Option<String> {
    let stem = file_stem(path).to_ascii_uppercase();
    let bytes = stem.as_bytes();
    let word = |index: usize| bytes.get(index).is_some_and(u8::is_ascii_alphanumeric);
    let last = DISPLAY_RE
        .find_iter(&stem)
        .filter(|found| !(found.start() > 0 && word(found.start() - 1)) && !word(found.end()))
        .last()?;
    Some(match last.as_str() {
        "FP16" => "F16".to_owned(),
        "FP32" => "F32".to_owned(),
        label => label.to_owned(),
    })
}

/// Nom du fichier sans `.gguf` ni suffixe de morceau.
fn file_stem(path: &str) -> &str {
    let name = basename(path);
    if let Some((prefix, _)) = shard(name) {
        return prefix;
    }
    if is_gguf(name) {
        &name[..name.len() - 5]
    } else {
        name
    }
}

/// Qualité d'après le libellé ; sans libellé reconnu, d'après les octets par
/// paramètre (taille et nombre de paramètres connus), sinon `Balanced`.
pub fn tier(label: Option<&str>, size: u64, parameters: Option<u64>) -> HubQuantTier {
    if let Some(label) = label {
        let label = label.strip_prefix("UD-").unwrap_or(label);
        let starts = |prefixes: &[&str]| prefixes.iter().any(|p| label.starts_with(p));
        if starts(&["F32", "BF16", "F16"]) {
            return HubQuantTier::Full;
        }
        if starts(&["Q8", "Q6"]) {
            return HubQuantTier::High;
        }
        if starts(&["Q5", "Q4", "IQ4", "MXFP4", "NVFP4"]) {
            return HubQuantTier::Balanced;
        }
        if starts(&["Q3", "IQ3"]) {
            return HubQuantTier::Compact;
        }
        if starts(&["Q2", "IQ2", "IQ1", "TQ", "Q1"]) {
            return HubQuantTier::Tiny;
        }
    }
    match parameters.filter(|&p| p > 0 && size > 0) {
        Some(parameters) => {
            let per_parameter = size as f64 / parameters as f64;
            if per_parameter >= 1.9 {
                HubQuantTier::Full
            } else if per_parameter >= 0.8 {
                HubQuantTier::High
            } else if per_parameter >= 0.5 {
                HubQuantTier::Balanced
            } else if per_parameter >= 0.4 {
                HubQuantTier::Compact
            } else {
                HubQuantTier::Tiny
            }
        }
        None => HubQuantTier::Balanced,
    }
}

/// Fichier d'un dépôt.
#[derive(Clone, Debug, PartialEq)]
pub struct FileEntry<'a> {
    pub path: &'a str,
    pub size: u64,
}

/// Version d'un dépôt, avant l'estimation de mémoire.
#[derive(Clone, Debug, PartialEq)]
pub struct Version {
    pub label: String,
    /// Libellé reconnu par le motif d'affichage (pour la qualité et la
    /// recommandation).
    pub quant: Option<String>,
    /// Fichier (premier morceau pour une version découpée).
    pub file: String,
    /// Nom Ollama ; `None` : non téléchargeable (`issue`).
    pub model: Option<String>,
    pub issue: Option<HubQuantIssue>,
    pub size: u64,
}

/// Inventaire des versions d'un dépôt (`repo_id` = « auteur/dépôt ») à
/// partir de ses fichiers : `.gguf` seulement, auxiliaires écartés, morceaux
/// regroupés (tailles additionnées, `Sharded`), sous-dossiers marqués
/// (`Subfolder`). Une version à la racine reçoit son nom Ollama (voir
/// [`tag_for`]) ; une version dont le nom serait refusé par Ollama est
/// écartée. Triées de la plus grosse à la plus petite.
pub fn inventory(repo_id: &str, files: &[FileEntry]) -> Vec<Version> {
    // Regroupement : chemin (sans suffixe de morceau) -> (premier fichier,
    // taille totale, découpé).
    let mut groups: BTreeMap<String, (String, u32, u64, bool)> = BTreeMap::new();
    for file in files {
        if !is_gguf(file.path) || is_auxiliary(file.path) {
            continue;
        }
        match shard(file.path) {
            Some((prefix, index)) => {
                let entry = groups
                    .entry(prefix.to_owned())
                    .or_insert_with(|| (file.path.to_owned(), index, 0, true));
                if index < entry.1 {
                    entry.0 = file.path.to_owned();
                    entry.1 = index;
                }
                entry.2 = entry.2.saturating_add(file.size);
            }
            None => {
                groups.insert(
                    file.path.to_owned(),
                    (file.path.to_owned(), 0, file.size, false),
                );
            }
        }
    }
    // Noms en majuscules de toutes les versions, pour l'ambiguïté des
    // étiquettes.
    let names: Vec<String> = groups
        .values()
        .map(|(file, ..)| basename(file).to_ascii_uppercase())
        .collect();
    let mut versions = Vec::new();
    for (index, (file, _, size, sharded)) in groups.into_values().enumerate() {
        let quant = display_label(&file);
        let label = quant.clone().unwrap_or_else(|| file_stem(&file).to_owned());
        let issue = if sharded {
            Some(HubQuantIssue::Sharded)
        } else if file.contains('/') {
            Some(HubQuantIssue::Subfolder)
        } else {
            None
        };
        let model = match issue {
            Some(_) => None,
            None => {
                let others = names
                    .iter()
                    .enumerate()
                    .filter(|(other, _)| *other != index)
                    .map(|(_, name)| name.as_str());
                match names::hf_model_name(repo_id, &tag_for(&file, others)) {
                    Some(model) => Some(model),
                    // Ollama refuserait ce nom : version écartée.
                    None => continue,
                }
            }
        };
        versions.push(Version {
            label,
            quant,
            file,
            model,
            issue,
            size,
        });
    }
    versions.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.label.cmp(&b.label)));
    versions
}

/// Étiquette Ollama d'un fichier à la racine : son libellé selon le registre
/// de Hugging Face s'il désigne ce seul fichier, sinon le nom de fichier
/// complet (toujours accepté par le registre). Jamais `latest`.
///
/// Le registre cherche l'étiquette dans les noms de fichiers sans exiger
/// l'égalité (34 § 2 : `f16` donne le BF16 d'un dépôt qui a les deux, 35 §
/// 2.4 : `Q4_K_XL` donne `UD-Q4_K_XL`) : un libellé contenu dans le nom d'une
/// autre version (`F16` dans `…-BF16`, `Q6_K` dans `…-UD-Q6_K_XL`) est donc
/// ambigu, comme un libellé partagé.
pub fn tag_for<'a>(file: &str, others: impl IntoIterator<Item = &'a str>) -> String {
    let name = basename(file);
    match hf_label(name) {
        Some(label)
            if !label.eq_ignore_ascii_case("latest")
                && !others.into_iter().any(|other| other.contains(&label)) =>
        {
            label
        }
        _ => name.to_owned(),
    }
}

/// Taille du projecteur de vision que le registre ajoutera (estimation :
/// `mmproj` F16, sinon BF16, sinon le plus petit) ; `None` sans projecteur.
pub fn projector_size(files: &[FileEntry]) -> Option<u64> {
    let projectors: Vec<&FileEntry> = files.iter().filter(|f| is_projector(f.path)).collect();
    let with = |quant: &str| {
        projectors
            .iter()
            .find(|f| display_label(f.path).as_deref() == Some(quant))
            .map(|f| f.size)
    };
    with("F16")
        .or_else(|| with("BF16"))
        .or_else(|| projectors.iter().map(|f| f.size).min())
}

/// Octets par paramètre d'une version selon son libellé, en millièmes :
/// médianes de la taille du fichier sur `gguf.total`, relevées le 12 oct.
/// 2026 sur une centaine de dépôts GGUF populaires de Hugging Face (une
/// vingtaine de fichiers par libellé courant ; Q4_K_M : 0,61 à 0,67).
const DENSITIES: &[(&str, u64)] = &[
    ("F32", 4000),
    ("BF16", 2000),
    ("F16", 2000),
    ("Q8_0", 1065),
    ("UD-Q8_K_XL", 1160),
    ("Q6_K", 830),
    ("Q6_K_L", 880),
    ("UD-Q6_K", 845),
    ("UD-Q6_K_XL", 925),
    ("Q5_K_M", 725),
    ("Q5_K_S", 700),
    ("Q5_K_L", 755),
    ("Q5_0", 705),
    ("Q5_1", 760),
    ("UD-Q5_K_S", 720),
    ("UD-Q5_K_M", 765),
    ("UD-Q5_K_XL", 765),
    ("Q4_K_M", 630),
    ("Q4_K_L", 665),
    ("Q4_K_S", 590),
    ("Q4_0", 595),
    ("Q4_1", 645),
    ("IQ4_NL", 595),
    ("IQ4_XS", 570),
    ("UD-Q4_K_S", 600),
    ("UD-Q4_K_M", 640),
    ("UD-Q4_K_XL", 650),
    ("UD-IQ4_NL", 520),
    ("UD-IQ4_XS", 515),
    ("MXFP4", 630),
    ("MXFP4_MOE", 630),
    ("Q3_K_L", 535),
    ("Q3_K_M", 505),
    ("Q3_K_S", 455),
    ("UD-Q3_K_M", 480),
    ("UD-Q3_K_XL", 510),
    ("IQ3_M", 470),
    ("IQ3_S", 440),
    ("IQ3_XS", 445),
    ("IQ3_XXS", 385),
    ("UD-IQ3_S", 430),
    ("UD-IQ3_XXS", 420),
    ("Q2_K", 400),
    ("Q2_K_L", 420),
    ("UD-Q2_K_XL", 395),
    ("Q2_0", 270),
    ("IQ2_M", 375),
    ("IQ2_S", 345),
    ("IQ2_XS", 325),
    ("IQ2_XXS", 315),
    ("UD-IQ2_M", 380),
    ("UD-IQ2_XXS", 340),
    ("IQ1_M", 285),
    ("IQ1_S", 270),
    ("TQ1_0", 225),
];

/// Densité d'un libellé absent de [`DENSITIES`], d'après son début.
const DENSITY_FAMILIES: &[(&str, u64)] = &[
    ("F32", 4000),
    ("BF16", 2000),
    ("F16", 2000),
    ("Q8", 1065),
    ("Q6", 830),
    ("Q5", 725),
    ("Q4", 630),
    ("IQ4", 595),
    ("MXFP4", 630),
    ("NVFP4", 630),
    ("Q3", 505),
    ("IQ3", 445),
    ("Q2", 400),
    ("IQ2", 345),
    ("TQ2", 290),
    ("IQ1", 280),
    ("TQ1", 225),
    ("Q1", 225),
];

/// Densité prise sans libellé reconnu : celle de Q4_K_M.
const DEFAULT_DENSITY: u64 = 630;

/// Paramètres supposés d'un projecteur de vision : la médiane relevée (un
/// `mmproj` F16 pèse 0,92 Go).
const PROJECTOR_PARAMETERS: u64 = 460_000_000;

/// Octets par paramètre d'une version d'après son libellé d'affichage, en
/// millièmes : le libellé exact, sinon le même sans `UD-`, sinon sa famille
/// (`Q5…`, `IQ3…`) ; sans libellé reconnu, la densité de Q4_K_M.
pub fn density(label: Option<&str>) -> u64 {
    let Some(label) = label else {
        return DEFAULT_DENSITY;
    };
    let exact = |label: &str| {
        DENSITIES
            .iter()
            .find(|(name, _)| *name == label)
            .map(|&(_, density)| density)
    };
    let base = label.strip_prefix("UD-").unwrap_or(label);
    exact(label)
        .or_else(|| exact(base))
        .or_else(|| {
            DENSITY_FAMILIES
                .iter()
                .find(|(prefix, _)| base.starts_with(prefix))
                .map(|&(_, density)| density)
        })
        .unwrap_or(DEFAULT_DENSITY)
}

/// Taille estimée d'une version dont on ne connaît que le libellé (listes,
/// où Hugging Face ne donne pas la taille des fichiers).
pub fn estimated_size(label: Option<&str>, parameters: u64) -> u64 {
    u64::try_from(u128::from(parameters) * u128::from(density(label)) / 1000).unwrap_or(u64::MAX)
}

/// Taille estimée du projecteur de vision que le registre ajoutera, d'après
/// les seuls noms des fichiers : celui que [`projector_size`] retiendrait
/// (F16, sinon BF16, sinon le plus léger d'après son libellé ; sans libellé :
/// F16, le cas courant), plafonnée à la moitié de `model_size` (un petit
/// modèle a un petit projecteur) ; `None` sans projecteur.
pub fn estimated_projector_size(files: &[FileEntry], model_size: u64) -> Option<u64> {
    let labels: Vec<Option<String>> = files
        .iter()
        .filter(|file| is_projector(file.path))
        .map(|file| display_label(file.path))
        .collect();
    if labels.is_empty() {
        return None;
    }
    let has = |quant: &str| labels.iter().any(|label| label.as_deref() == Some(quant));
    let per_parameter = if has("F16") || has("BF16") {
        2000
    } else {
        labels
            .iter()
            .map(|label| label.as_deref().map_or(2000, |label| density(Some(label))))
            .min()
            .unwrap_or(2000)
    };
    Some((PROJECTOR_PARAMETERS * per_parameter / 1000).min(model_size / 2))
}

/// Versions conseillées dans l'ordre de préférence, avec leurs équivalents
/// `UD-…_XL` d'unsloth.
const PREFERRED: [[&str; 2]; 4] = [
    ["Q8_0", "UD-Q8_K_XL"],
    ["Q6_K", "UD-Q6_K_XL"],
    ["Q5_K_M", "UD-Q5_K_XL"],
    ["Q4_K_M", "UD-Q4_K_XL"],
];

/// Versions « Balanced » de la plus proche de Q4_K_M à la plus lointaine.
const NEAR_Q4_K_M: &[&str] = &[
    "Q4_K_M",
    "UD-Q4_K_XL",
    "Q4_K_L",
    "Q4_K_S",
    "IQ4_XS",
    "IQ4_NL",
    "Q4_0",
    "Q4_1",
    "MXFP4",
    "MXFP4_MOE",
    "NVFP4",
    "Q5_K_S",
    "Q5_K_M",
    "UD-Q5_K_XL",
    "Q5_0",
    "Q5_1",
];

/// Index de la version conseillée parmi `versions` (téléchargeables
/// seulement) : la meilleure de Q8_0, Q6_K, Q5_K_M, Q4_K_M dont `fits(i)` est
/// vrai ; sinon Q4_K_M ou la « Balanced » la plus proche ; sinon la plus
/// petite. `fits` vaut toujours faux pour une machine inconnue.
pub fn recommended(versions: &[Version], fits: impl Fn(usize) -> bool) -> Option<usize> {
    let downloadable = |index: &usize| versions[*index].model.is_some();
    let with = |quant: &str| {
        (0..versions.len())
            .filter(downloadable)
            .find(|&i| versions[i].quant.as_deref() == Some(quant))
    };
    for group in PREFERRED {
        for quant in group {
            if let Some(index) = with(quant)
                && fits(index)
            {
                return Some(index);
            }
        }
    }
    NEAR_Q4_K_M
        .iter()
        .find_map(|quant| with(quant))
        .or_else(|| {
            (0..versions.len())
                .filter(downloadable)
                .filter(|&i| {
                    versions[i].quant.is_some()
                        && tier(versions[i].quant.as_deref(), 0, None) == HubQuantTier::Balanced
                })
                .min_by_key(|&i| versions[i].size)
        })
        .or_else(|| {
            (0..versions.len())
                .filter(downloadable)
                .min_by_key(|&i| versions[i].size)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files<'a>(list: &'a [(&'a str, u64)]) -> Vec<FileEntry<'a>> {
        list.iter()
            .map(|&(path, size)| FileEntry { path, size })
            .collect()
    }

    /// Fichiers modèle et auxiliaires réels de `unsloth/Qwen3.5-9B-GGUF`
    /// (`35-samples/model-detail-blobs.json`).
    const QWEN35_9B: &[(&str, u64)] = &[
        (".gitattributes", 3060),
        ("Qwen3.5-9B-BF16.gguf", 17920697312),
        ("Qwen3.5-9B-IQ4_NL.gguf", 5371028704),
        ("Qwen3.5-9B-IQ4_XS.gguf", 5168653536),
        ("Qwen3.5-9B-Q3_K_M.gguf", 4673643744),
        ("Qwen3.5-9B-Q4_0.gguf", 5379417312),
        ("Qwen3.5-9B-Q4_1.gguf", 5837251808),
        ("Qwen3.5-9B-Q4_K_M.gguf", 5680522464),
        ("Qwen3.5-9B-Q5_K_M.gguf", 6577841376),
        ("Qwen3.5-9B-Q6_K.gguf", 7458301152),
        ("Qwen3.5-9B-Q8_0.gguf", 9527502048),
        ("Qwen3.5-9B-UD-IQ2_XXS.gguf", 3190613216),
        ("Qwen3.5-9B-UD-Q4_K_XL.gguf", 5966095584),
        ("Qwen3.5-9B-UD-Q6_K_XL.gguf", 8756929760),
        ("README.md", 78888),
        ("imatrix_unsloth.gguf_file", 5150208),
        ("mmproj-BF16.gguf", 921705024),
        ("mmproj-F16.gguf", 918166080),
        ("mmproj-F32.gguf", 1824062016),
    ];

    /// `unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF` réduit : dossier `BF16/`
    /// découpé, fichiers `params` et `template`.
    const CODER: &[(&str, u64)] = &[
        (
            "BF16/Qwen3-Coder-30B-A3B-Instruct-BF16-00002-of-00002.gguf",
            11440652032,
        ),
        (
            "BF16/Qwen3-Coder-30B-A3B-Instruct-BF16-00001-of-00002.gguf",
            49655154016,
        ),
        ("Qwen3-Coder-30B-A3B-Instruct-Q2_K.gguf", 11258612896),
        ("Qwen3-Coder-30B-A3B-Instruct-Q2_K_L.gguf", 11331542176),
        ("Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf", 18556689568),
        ("Qwen3-Coder-30B-A3B-Instruct-UD-TQ1_0.gguf", 8005213344),
        ("Qwen3-Coder-30B-A3B-Instruct-UD-IQ1_M.gguf", 9627540640),
        ("params", 178),
        ("template", 1482),
    ];

    /// Densités relevées : libellé exact, variante `UD-`, famille, défaut.
    #[test]
    fn a_label_tells_the_bytes_per_parameter() {
        for (label, expected) in [
            ("Q4_K_M", 630),
            ("Q4_0", 595),
            ("Q8_0", 1065),
            ("BF16", 2000),
            ("F32", 4000),
            ("UD-Q4_K_XL", 650),
            ("UD-IQ2_XXS", 340),
            // Sans entrée `UD-` : celle du libellé de base.
            ("UD-Q3_K_S", 455),
            ("UD-TQ1_0", 225),
            // Par famille.
            ("Q4_0_4_4", 630),
            ("Q5_K_XL", 725),
            ("IQ3_XS_V2", 445),
            ("NVFP4", 630),
            // Inconnu : comme une Q4_K_M.
            ("WEIRD", 630),
        ] {
            assert_eq!(density(Some(label)), expected, "{label}");
        }
        assert_eq!(density(None), 630);
        // Du plus lourd au plus léger pour une même famille.
        let order = [
            "F32", "F16", "Q8_0", "Q6_K", "Q5_K_M", "Q4_K_M", "Q3_K_M", "Q2_K",
        ];
        assert!(
            order
                .windows(2)
                .all(|pair| density(Some(pair[0])) > density(Some(pair[1])))
        );
        assert_eq!(estimated_size(Some("Q4_0"), 10_000_000_000), 5_950_000_000);
        assert_eq!(estimated_size(None, 8_000_000_000), 5_040_000_000);
        assert_eq!(estimated_size(Some("F32"), u64::MAX), u64::MAX);
    }

    /// Tailles réelles de `unsloth/Qwen3.5-9B-GGUF` : chaque estimation est à
    /// 6 % près de la taille du fichier (8,95 milliards de paramètres).
    #[test]
    fn estimates_are_close_to_real_files() {
        for &(file, real) in QWEN35_9B {
            if !is_gguf(file) || is_auxiliary(file) {
                continue;
            }
            let label = display_label(file);
            let estimate = estimated_size(label.as_deref(), 8_950_000_000);
            assert!(
                estimate.abs_diff(real) * 100 < real * 6,
                "{file}: {estimate} for {real}"
            );
        }
    }

    #[test]
    fn the_projector_is_estimated_from_the_file_names() {
        let big = 20_000_000_000;
        let of = |names: &[&str], model: u64| {
            let list: Vec<(&str, u64)> = names.iter().map(|&name| (name, 0)).collect();
            estimated_projector_size(&files(&list), model)
        };
        // Sans projecteur.
        assert_eq!(of(&["model-Q4_K_M.gguf", "README.md"], big), None);
        // F16 ou BF16 : celui que le registre prend (0,92 Go, relevé :
        // 918 166 080 pour Qwen3.5-9B).
        for names in [
            &["mmproj-F32.gguf", "mmproj-F16.gguf", "mmproj-Q8_0.gguf"][..],
            &["mmproj-BF16.gguf", "mmproj-Q4_0.gguf"],
            // Sans libellé : F16, le cas courant.
            &["model.mmproj.gguf"],
        ] {
            assert_eq!(of(names, big), Some(920_000_000), "{names:?}");
        }
        // Sinon le plus léger (gemma-4-12B QAT : 175 115 616 réels).
        assert_eq!(
            of(&["mmproj-gemma-4-12b-it-qat-q4_0.gguf"], big),
            Some(273_700_000)
        );
        assert_eq!(
            of(&["mmproj-Q8_0.gguf", "mmproj-F32.gguf"], big),
            Some(489_900_000)
        );
        // Petit modèle : la moitié de sa taille au plus.
        assert_eq!(of(&["mmproj-F16.gguf"], 600_000_000), Some(300_000_000));
    }

    fn version<'a>(versions: &'a [Version], file: &str) -> &'a Version {
        versions
            .iter()
            .find(|v| v.file == file)
            .unwrap_or_else(|| panic!("{file} missing"))
    }

    #[test]
    fn the_hugging_face_label_is_the_last_match_of_its_pattern() {
        for (file, label) in [
            ("Qwen3.5-9B-Q4_K_M.gguf", Some("Q4_K_M")),
            ("Qwen3.5-9B-UD-Q4_K_XL.gguf", Some("UD-Q4_K_XL")),
            (
                "Qwen3-VL-8B-Instruct-abliterated.i1-Q4_K_M.gguf",
                Some("Q4_K_M"),
            ),
            ("Qwen3-Coder-30B-A3B-Instruct-Q2_K_L.gguf", Some("Q2_K_L")),
            ("gpt-oss-20b-MXFP4.gguf", Some("MXFP4")),
            ("model-MXFP4_MOE.gguf", Some("MXFP4_MOE")),
            ("gemma-4-12b-it-qat-q4_0.gguf", Some("Q4_0")),
            ("mistral-7b-instruct-v0.2.Q4_K_M.gguf", Some("Q4_K_M")),
            // Inconnus de la regex de Hugging Face.
            ("Qwen3.8-27B-Q4_K_L.gguf", None),
            ("Qwen3-8B.fp16.gguf", None),
            ("JiRackUltra_1b.gguf", None),
        ] {
            assert_eq!(hf_label(file).as_deref(), label, "{file}");
        }
    }

    /// Noms non ASCII venus de Hugging Face : ni panique, ni faux GGUF.
    #[test]
    fn non_ascii_names_are_read_without_cutting_a_character() {
        for (file, gguf) in [
            ("说明.md", false),
            ("使用说明.txt", false),
            ("读我😀.png", false),
            ("é.gguf", true),
            ("模型-Q4_K_M.GGUF", true),
            ("ü.gguf.txt", false),
            ("😀", false),
            (".gguf", false),
        ] {
            assert_eq!(is_gguf(file), gguf, "{file}");
            assert!(!is_projector(file), "{file}");
            let _ = display_label(file);
        }
        assert_eq!(display_label("模型-Q4_K_M.gguf").as_deref(), Some("Q4_K_M"));
    }

    #[test]
    fn the_display_label_knows_more_names() {
        for (file, label) in [
            ("Qwen3.8-27B-Q4_K_L.gguf", Some("Q4_K_L")),
            ("Qwen3-8B.fp16.gguf", Some("F16")),
            ("model.FP32.gguf", Some("F32")),
            ("Qwen3.5-9B-UD-Q4_K_XL.gguf", Some("UD-Q4_K_XL")),
            ("RVN-Q4_K_M-multilingual.gguf", Some("Q4_K_M")),
            ("gemma-4-E4B_q4_0-it.gguf", Some("Q4_0")),
            ("model-Q4_0_4_4.gguf", Some("Q4_0_4_4")),
            (
                "BF16/Qwen3-Coder-30B-A3B-Instruct-BF16-00001-of-00002.gguf",
                Some("BF16"),
            ),
            // Collé à une lettre : pas un libellé.
            ("model-XQ4_K_M.gguf", None),
            ("Laguna-XS-2.1-APEX-Balanced.gguf", None),
        ] {
            assert_eq!(display_label(file).as_deref(), label, "{file}");
        }
    }

    #[test]
    fn auxiliary_files_are_never_versions() {
        for file in [
            "mmproj-F16.gguf",
            "Qwen3-VL-8B-Instruct-abliterated.mmproj-Q8_0.gguf",
            "MTP/mtp-Qwen3.8-27B-Q4_0.gguf",
            "mtp-gemma-4-E4B-it-Q4_0.gguf",
            "Qwen3.8-27B-Uncensored-HauhauCS-Aggressive-FastMTP-32K.gguf",
            "eagle3-gpt-oss-20b-Q8_0.gguf",
            "imatrix-qwen3.8-27b.gguf",
            "Qwen3-8B.imatrix.gguf",
            "draft-model-Q8_0.gguf",
        ] {
            assert!(is_auxiliary(file), "{file}");
        }
        for file in ["Qwen3.5-9B-Q4_K_M.gguf", "RVN-IQ1_M-mtp.gguf"] {
            assert!(!is_auxiliary(file), "{file}");
        }
    }

    /// Exemples des rapports (35 R5, 34 § 2) : libellé unique → étiquette,
    /// sinon nom de fichier ; morceaux non téléchargeables.
    #[test]
    fn tags_follow_the_registry() {
        let versions = inventory("unsloth/Qwen3.5-9B-GGUF", &files(QWEN35_9B));
        let model = |file: &str| version(&versions, file).model.clone();
        assert_eq!(
            model("Qwen3.5-9B-UD-Q4_K_XL.gguf").as_deref(),
            Some("hf.co/unsloth/Qwen3.5-9B-GGUF:UD-Q4_K_XL")
        );
        assert_eq!(
            model("Qwen3.5-9B-Q4_K_M.gguf").as_deref(),
            Some("hf.co/unsloth/Qwen3.5-9B-GGUF:Q4_K_M")
        );
        // « Q6_K » figure dans le nom de UD-Q6_K_XL : nom de fichier.
        assert_eq!(
            model("Qwen3.5-9B-Q6_K.gguf").as_deref(),
            Some("hf.co/unsloth/Qwen3.5-9B-GGUF:Qwen3.5-9B-Q6_K.gguf")
        );
        assert_eq!(
            model("Qwen3.5-9B-BF16.gguf").as_deref(),
            Some("hf.co/unsloth/Qwen3.5-9B-GGUF:BF16")
        );
        // Ni projecteur, ni imatrix, ni README.
        assert_eq!(versions.len(), 13);
        assert!(versions.iter().all(|v| !v.file.contains("mmproj")));
        // Du plus gros au plus petit.
        assert!(versions.windows(2).all(|w| w[0].size >= w[1].size));
        assert_eq!(versions[0].label, "BF16");

        let bartowski = inventory(
            "bartowski/Qwen3.8-27B-GGUF",
            &files(&[
                ("Qwen3.8-27B-Q4_K_L.gguf", 18820703968),
                ("Qwen3.8-27B-Q3_K_XL.gguf", 16391919200),
                (
                    "Qwen3.8-27B-bf16/Qwen3.8-27B-bf16-00001-of-00002.gguf",
                    40_000_000_000,
                ),
                (
                    "Qwen3.8-27B-bf16/Qwen3.8-27B-bf16-00002-of-00002.gguf",
                    14_000_000_000,
                ),
            ]),
        );
        let q4kl = version(&bartowski, "Qwen3.8-27B-Q4_K_L.gguf");
        assert_eq!(q4kl.label, "Q4_K_L");
        assert_eq!(
            q4kl.model.as_deref(),
            Some("hf.co/bartowski/Qwen3.8-27B-GGUF:Qwen3.8-27B-Q4_K_L.gguf")
        );
        assert_eq!(
            version(&bartowski, "Qwen3.8-27B-Q3_K_XL.gguf")
                .model
                .as_deref(),
            Some("hf.co/bartowski/Qwen3.8-27B-GGUF:Q3_K_XL")
        );
        let bf16 = version(
            &bartowski,
            "Qwen3.8-27B-bf16/Qwen3.8-27B-bf16-00001-of-00002.gguf",
        );
        assert_eq!(bf16.issue, Some(HubQuantIssue::Sharded));
        assert_eq!(bf16.model, None);
        assert_eq!(bf16.size, 54_000_000_000);
        assert_eq!(bf16.label, "BF16");

        let mradermacher = inventory(
            "mradermacher/Qwen3-VL-8B-Instruct-abliterated-i1-GGUF",
            &files(&[
                (
                    "Qwen3-VL-8B-Instruct-abliterated.i1-Q4_K_M.gguf",
                    5027785792,
                ),
                (
                    "Qwen3-VL-8B-Instruct-abliterated.i1-IQ4_XS.gguf",
                    4593298496,
                ),
                ("Qwen3-VL-8B-Instruct-abliterated.imatrix.gguf", 7_000_000),
            ]),
        );
        assert_eq!(mradermacher.len(), 2);
        assert_eq!(
            mradermacher[0].model.as_deref(),
            Some("hf.co/mradermacher/Qwen3-VL-8B-Instruct-abliterated-i1-GGUF:Q4_K_M")
        );

        let maziyar = inventory(
            "MaziyarPanahi/Qwen3-8B-GGUF",
            &files(&[
                ("Qwen3-8B.fp16.gguf", 16388044000),
                ("Qwen3-8B.Q4_K_M.gguf", 5027783488),
            ]),
        );
        let fp16 = version(&maziyar, "Qwen3-8B.fp16.gguf");
        assert_eq!(fp16.label, "F16");
        assert_eq!(
            fp16.model.as_deref(),
            Some("hf.co/MaziyarPanahi/Qwen3-8B-GGUF:Qwen3-8B.fp16.gguf")
        );
        assert_eq!(
            version(&maziyar, "Qwen3-8B.Q4_K_M.gguf").model.as_deref(),
            Some("hf.co/MaziyarPanahi/Qwen3-8B-GGUF:Q4_K_M")
        );
    }

    #[test]
    fn shared_or_contained_labels_use_the_file_name() {
        // Même libellé deux fois.
        let same = inventory(
            "a/b-GGUF",
            &files(&[("b-Q4_K_M.gguf", 10), ("b-v2-Q4_K_M.gguf", 11)]),
        );
        assert!(
            same.iter()
                .all(|v| v.model.as_deref().is_some_and(|m| m.ends_with(".gguf")))
        );
        // F16 se lit dans le nom du BF16 (34 § 2 : le registre servait le BF16).
        let f16 = inventory(
            "ggml-org/Qwen3-0.6B-GGUF",
            &files(&[
                ("Qwen3-0.6B-BF16.gguf", 1_510_000_000),
                ("Qwen3-0.6B-f16.gguf", 1_510_000_001),
                ("Qwen3-0.6B-Q4_0.gguf", 430_000_000),
            ]),
        );
        assert_eq!(
            version(&f16, "Qwen3-0.6B-f16.gguf").model.as_deref(),
            Some("hf.co/ggml-org/Qwen3-0.6B-GGUF:Qwen3-0.6B-f16.gguf")
        );
        assert_eq!(
            version(&f16, "Qwen3-0.6B-BF16.gguf").model.as_deref(),
            Some("hf.co/ggml-org/Qwen3-0.6B-GGUF:BF16")
        );
        assert_eq!(
            version(&f16, "Qwen3-0.6B-Q4_0.gguf").model.as_deref(),
            Some("hf.co/ggml-org/Qwen3-0.6B-GGUF:Q4_0")
        );
    }

    #[test]
    fn mixed_repositories_keep_their_single_files() {
        let versions = inventory("unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF", &files(CODER));
        let bf16 = version(
            &versions,
            "BF16/Qwen3-Coder-30B-A3B-Instruct-BF16-00001-of-00002.gguf",
        );
        assert_eq!(bf16.issue, Some(HubQuantIssue::Sharded));
        assert_eq!(bf16.size, 49655154016 + 11440652032);
        // « Q2_K » est contenu dans « Q2_K_L » : nom de fichier.
        assert_eq!(
            version(&versions, "Qwen3-Coder-30B-A3B-Instruct-Q2_K.gguf")
                .model
                .as_deref(),
            Some(
                "hf.co/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF:Qwen3-Coder-30B-A3B-Instruct-Q2_K.gguf"
            )
        );
        assert_eq!(
            version(&versions, "Qwen3-Coder-30B-A3B-Instruct-Q2_K_L.gguf")
                .model
                .as_deref(),
            Some("hf.co/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF:Q2_K_L")
        );
        assert_eq!(
            version(&versions, "Qwen3-Coder-30B-A3B-Instruct-UD-TQ1_0.gguf").label,
            "UD-TQ1_0"
        );
        assert_eq!(versions.len(), 6);
    }

    #[test]
    fn subfolders_and_names_ollama_refuses_are_not_downloadable() {
        // Étiquette = nom de fichier de plus de 80 caractères.
        let long = format!("{}.gguf", "x".repeat(90));
        let versions = inventory(
            "a/b-GGUF",
            &files(&[
                ("Q4_K_M/b-Q4_K_M.gguf", 10),
                ("b-Q8_0.gguf", 20),
                (long.as_str(), 5),
            ]),
        );
        assert_eq!(versions.len(), 2);
        let sub = version(&versions, "Q4_K_M/b-Q4_K_M.gguf");
        assert_eq!(sub.issue, Some(HubQuantIssue::Subfolder));
        assert_eq!(sub.model, None);
        // Auteur avec un point : rien n'est téléchargeable.
        assert!(
            inventory("a.b/c-GGUF", &files(&[("c-Q4_K_M.gguf", 1)])).is_empty(),
            "invalid namespace"
        );
    }

    #[test]
    fn tags_are_never_latest() {
        assert_eq!(tag_for("latest.gguf", []), "latest.gguf");
        assert_eq!(tag_for("x-Q4_K_M.gguf", ["X-Q8_0.GGUF"]), "Q4_K_M");
    }

    #[test]
    fn tiers_follow_the_quantization() {
        for (label, tier_) in [
            ("BF16", HubQuantTier::Full),
            ("F32", HubQuantTier::Full),
            ("Q8_0", HubQuantTier::High),
            ("UD-Q6_K_XL", HubQuantTier::High),
            ("Q4_K_M", HubQuantTier::Balanced),
            ("IQ4_XS", HubQuantTier::Balanced),
            ("MXFP4", HubQuantTier::Balanced),
            ("NVFP4", HubQuantTier::Balanced),
            ("Q3_K_M", HubQuantTier::Compact),
            ("IQ3_XXS", HubQuantTier::Compact),
            ("Q2_K_L", HubQuantTier::Tiny),
            ("UD-IQ1_M", HubQuantTier::Tiny),
            ("UD-TQ1_0", HubQuantTier::Tiny),
        ] {
            assert_eq!(tier(Some(label), 0, None), tier_, "{label}");
        }
        // Sans libellé : octets par paramètre.
        assert_eq!(tier(None, 16_000, Some(8_000)), HubQuantTier::Full);
        assert_eq!(tier(None, 5_000, Some(8_000)), HubQuantTier::Balanced);
        assert_eq!(tier(None, 5_000, None), HubQuantTier::Balanced);
    }

    #[test]
    fn the_projector_estimate_prefers_f16() {
        assert_eq!(projector_size(&files(QWEN35_9B)), Some(918166080));
        assert_eq!(
            projector_size(&files(&[("mmproj-BF16.gguf", 7), ("mmproj-F32.gguf", 9),])),
            Some(7)
        );
        assert_eq!(
            projector_size(&files(&[
                ("mmproj-Q8_0.gguf", 5),
                ("x.mmproj-Q4_K_M.gguf", 3),
            ])),
            Some(3)
        );
        assert_eq!(projector_size(&files(CODER)), None);
    }

    #[test]
    fn the_recommended_version_is_the_best_that_fits() {
        let versions = inventory("unsloth/Qwen3.5-9B-GGUF", &files(QWEN35_9B));
        let pick = |limit: u64| {
            recommended(&versions, |i| versions[i].size <= limit).map(|i| versions[i].label.clone())
        };
        assert_eq!(pick(u64::MAX).as_deref(), Some("Q8_0"));
        assert_eq!(pick(9_000_000_000).as_deref(), Some("Q6_K"));
        assert_eq!(pick(8_800_000_000).as_deref(), Some("Q6_K"));
        assert_eq!(pick(7_000_000_000).as_deref(), Some("Q5_K_M"));
        assert_eq!(pick(5_700_000_000).as_deref(), Some("Q4_K_M"));
        // Rien ne tient, ou machine inconnue : Q4_K_M quand même.
        assert_eq!(pick(0).as_deref(), Some("Q4_K_M"));
        // Sans Q4_K_M : la « Balanced » la plus proche.
        let no_q4 = inventory(
            "a/b-GGUF",
            &files(&[
                ("b-Q8_0.gguf", 900),
                ("b-IQ4_XS.gguf", 400),
                ("b-Q5_K_S.gguf", 500),
            ]),
        );
        let index = recommended(&no_q4, |_| false).unwrap();
        assert_eq!(no_q4[index].label, "IQ4_XS");
        // Ni préférée ni « Balanced » : la plus petite.
        let gemma = inventory(
            "ggml-org/gemma-x-GGUF",
            &files(&[("gemma-BF16.gguf", 1500), ("gemma-Q3_K_M.gguf", 300)]),
        );
        assert_eq!(
            gemma[recommended(&gemma, |_| false).unwrap()].label,
            "Q3_K_M"
        );
        // Équivalent UD d'unsloth quand la version standard manque.
        let unsloth = inventory(
            "unsloth/x-GGUF",
            &files(&[("x-UD-Q6_K_XL.gguf", 700), ("x-Q4_K_M.gguf", 400)]),
        );
        let fits_all = recommended(&unsloth, |_| true).unwrap();
        assert_eq!(unsloth[fits_all].label, "UD-Q6_K_XL");
        // Rien de téléchargeable : rien.
        let sharded = inventory(
            "a/b-GGUF",
            &files(&[
                ("b-Q4_K_M-00001-of-00002.gguf", 1),
                ("b-Q4_K_M-00002-of-00002.gguf", 1),
            ]),
        );
        assert_eq!(recommended(&sharded, |_| true), None);
    }
}
