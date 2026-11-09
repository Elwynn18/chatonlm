//! Modèles d'un dossier d'Ollama, lus sur le disque sans jamais y écrire :
//! manifestes de `manifests-v2/` (Ollama 0.40 : liens vers le blob du
//! manifeste, ou fichiers sous Windows) puis de `manifests/`, à quatre
//! niveaux (`<hôte>/<espace>/<modèle>/<étiquette>`) ; variante `llamacpp`
//! d'une liste quand ses blobs sont là ; blob des poids vérifié (fichier
//! ordinaire, taille du manifeste, GGUF que llama.cpp charge). Deux noms aux
//! mêmes poids, au même projecteur et aux mêmes réglages d'Ollama (consigne,
//! paramètres, gabarit : `qwen3:latest` et `qwen3:8b`) font un seul modèle ;
//! un modèle créé par `ollama create` sur les mêmes poids avec sa propre
//! consigne en reste un autre.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use super::compat::{self, CompatError};
use super::manifest::{self, Layer, MODEL, PROJECTOR};
use super::names::{self, ManifestName};
use super::{ImportProblem, OllamaParams};
use crate::local_models::id::ModelId;

/// Dossiers de manifestes, du plus récent rangement au plus ancien.
const MANIFEST_ROOTS: [&str; 2] = ["manifests-v2", "manifests"];

/// Manifestes lus au plus dans un dossier.
const MAX_MANIFESTS: usize = 4096;

/// Modèle trouvé.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub name: ManifestName,
    /// Autres noms aux mêmes fichiers.
    pub aliases: Vec<ManifestName>,
    pub weights: Option<Layer>,
    pub projector: Option<Layer>,
    pub params: Option<OllamaParams>,
    pub system: Option<String>,
    /// Empreintes des couches de réglages d'Ollama (`params`, `system`,
    /// `template`) : deux noms ne font un seul modèle que si elles sont
    /// aussi les mêmes.
    pub settings: Vec<Option<String>>,
    /// Les poids n'ont pas de gabarit de conversation
    /// (`tokenizer.chat_template`) : Ollama donnait le sien, llama-server en
    /// prendra un par défaut (réponses moins bonnes).
    pub no_template: bool,
    /// Variante `llamacpp` d'une liste.
    pub from_list: bool,
    pub verdict: Verdict,
}

impl Found {
    /// Octets des poids et du projecteur.
    pub fn size(&self) -> u64 {
        self.weights
            .iter()
            .chain(&self.projector)
            .map(|layer| layer.size)
            .sum()
    }

    /// Tous les noms : le principal, puis les autres.
    pub fn all_names(&self) -> impl Iterator<Item = &ManifestName> {
        std::iter::once(&self.name).chain(&self.aliases)
    }

    /// Empreintes des poids et du projecteur.
    pub fn digests(&self) -> impl Iterator<Item = &str> {
        self.weights
            .iter()
            .chain(&self.projector)
            .map(|layer| layer.digest.as_str())
    }
}

/// Ce que l'import peut en faire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Importable sous cet identifiant ; `file_type` lu dans les poids.
    Ready { id: ModelId, file_type: Option<u32> },
    /// Format propre à Ollama : à retélécharger, équivalent proposé.
    Redownload {
        problem: ImportProblem,
        suggestion: Option<String>,
    },
    /// Rien à importer.
    Skip(ImportProblem),
}

/// Chemin du blob d'empreinte `hex` dans `dir`.
pub fn blob_path(dir: &Path, hex: &str) -> PathBuf {
    dir.join("blobs").join(manifest::blob_name(hex))
}

/// Le blob de la couche est là : fichier ordinaire (pas un lien) de la
/// taille annoncée.
pub fn blob_present(dir: &Path, layer: &Layer) -> bool {
    fs::symlink_metadata(blob_path(dir, &layer.digest))
        .is_ok_and(|meta| meta.is_file() && meta.len() == layer.size)
}

/// Lit au plus `max` octets du fichier `path` ; `None` s'il est plus gros,
/// illisible ou pas un fichier ordinaire (lien, FIFO qui bloquerait
/// l'ouverture, périphérique : jamais ouvert).
fn read_bounded(path: &Path, max: u64) -> Option<Vec<u8>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > max {
        return None;
    }
    let file = fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() || file.metadata().ok()?.len() > max {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= max).then_some(bytes)
}

/// Manifeste rangé à `path` (fichier, ou lien vers un blob de `dir` du
/// rangement d'Ollama 0.40 ; un lien qui sort de `dir/blobs` est ignoré).
fn read_manifest_file(dir: &Path, path: &Path) -> Option<Vec<u8>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() {
        let blobs = fs::canonicalize(dir.join("blobs")).ok()?;
        let target = fs::canonicalize(path).ok()?;
        if !target.starts_with(&blobs) {
            return None;
        }
        return read_bounded(&target, manifest::MAX_MANIFEST);
    }
    meta.is_file()
        .then(|| read_bounded(path, manifest::MAX_MANIFEST))
        .flatten()
}

/// Fichiers de manifestes sous `root`, à quatre niveaux exactement, sans
/// suivre les dossiers liés.
fn manifest_files(root: &Path) -> Vec<(Vec<String>, PathBuf)> {
    let mut found = Vec::new();
    let mut stack: Vec<(Vec<String>, PathBuf)> = vec![(Vec::new(), root.to_path_buf())];
    while let Some((parts, folder)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            if found.len() >= MAX_MANIFESTS {
                return found;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let Some(part) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let mut parts = parts.clone();
            parts.push(part);
            if parts.len() < 4 && kind.is_dir() {
                stack.push((parts, entry.path()));
            } else if parts.len() == 4 && (kind.is_file() || kind.is_symlink()) {
                found.push((parts, entry.path()));
            }
        }
    }
    found
}

/// Modèles du dossier d'Ollama `dir`, triés par nom. Erreur seulement si le
/// dossier lui-même ne se lit pas.
pub fn scan(dir: &Path) -> io::Result<Vec<Found>> {
    fs::read_dir(dir)?;
    let mut seen: Vec<String> = Vec::new();
    let mut found: Vec<Found> = Vec::new();
    for root in MANIFEST_ROOTS {
        let mut files = manifest_files(&dir.join(root));
        files.sort();
        for (parts, path) in files {
            let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
            let Some(name) = ManifestName::from_parts(&refs) else {
                continue;
            };
            if name.is_internal_variant() || seen.contains(&name.key()) {
                continue;
            }
            seen.push(name.key());
            let bytes = read_manifest_file(dir, &path);
            found.push(examine(dir, name, bytes.as_deref()));
        }
    }
    let mut models = merge(found);
    models.sort_by_key(|model| model.name.ollama_name());
    Ok(models)
}

/// Jugement d'un manifeste (contenu `bytes`, `None` s'il ne se lit pas).
pub fn examine(dir: &Path, name: ManifestName, bytes: Option<&[u8]>) -> Found {
    let mut found = Found {
        name,
        aliases: Vec::new(),
        weights: None,
        projector: None,
        params: None,
        system: None,
        settings: Vec::new(),
        no_template: false,
        from_list: false,
        verdict: Verdict::Skip(ImportProblem::Unreadable),
    };
    let Some(top) = bytes.and_then(manifest::parse) else {
        return found;
    };
    let read_blob = |hex: &str| read_bounded(&blob_path(dir, hex), manifest::MAX_MANIFEST);
    let blob_ok = |layer: &Layer| blob_present(dir, layer);
    let Some(chosen) = manifest::choose(top, &read_blob, &blob_ok) else {
        return found;
    };
    found.from_list = chosen.from_list;
    let manifest = chosen.manifest;
    found.weights = manifest.layer(MODEL).cloned();
    found.projector = manifest.layer(PROJECTOR).cloned();
    found.params = manifest
        .layer(manifest::PARAMS)
        .and_then(|layer| read_bounded(&blob_path(dir, &layer.digest), manifest::MAX_TEXT))
        .and_then(|bytes| OllamaParams::from_json(&bytes));
    found.system = manifest
        .layer(manifest::SYSTEM)
        .and_then(|layer| read_bounded(&blob_path(dir, &layer.digest), manifest::MAX_TEXT))
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .filter(|text| !text.is_empty());
    found.settings = [manifest::PARAMS, manifest::SYSTEM, manifest::TEMPLATE]
        .iter()
        .map(|kind| manifest.layer(kind).map(|layer| layer.digest.clone()))
        .collect();
    let cloud = manifest.cloud
        || manifest
            .config
            .as_deref()
            .and_then(read_blob)
            .is_some_and(|bytes| manifest::config_is_cloud(&bytes));
    let (verdict, no_template) = verdict(dir, &found, &manifest, cloud);
    found.verdict = verdict;
    found.no_template = no_template;
    found
}

/// Jugement d'un modèle ; rend aussi « gabarit absent » (modèle importable
/// seulement).
fn verdict(
    dir: &Path,
    found: &Found,
    manifest: &manifest::Manifest,
    cloud: bool,
) -> (Verdict, bool) {
    let verdict = |verdict| (verdict, false);
    if cloud {
        return verdict(Verdict::Skip(ImportProblem::Cloud));
    }
    if manifest.count(manifest::ADAPTER) > 0 {
        return verdict(Verdict::Skip(ImportProblem::Adapter));
    }
    let Some(weights) = &found.weights else {
        return verdict(Verdict::Skip(if manifest.count(manifest::TENSOR) > 0 {
            ImportProblem::NotGguf
        } else {
            ImportProblem::Unreadable
        }));
    };
    if manifest.count(MODEL) > 1 {
        return verdict(Verdict::Skip(ImportProblem::NotGguf));
    }
    if !blob_present(dir, weights)
        || found
            .projector
            .as_ref()
            .is_some_and(|p| !blob_present(dir, p))
    {
        return verdict(Verdict::Skip(ImportProblem::BlobMissing));
    }
    let report = match compat::check(&blob_path(dir, &weights.digest)) {
        Ok(report) => report,
        Err(CompatError::NotGguf) => return verdict(Verdict::Skip(ImportProblem::NotGguf)),
        Err(CompatError::Io(_)) => return verdict(Verdict::Skip(ImportProblem::Unreadable)),
    };
    let suggestion = || names::hf_equivalent(&found.name).map(str::to_owned);
    match report.support() {
        compat::Support::Chat => {}
        compat::Support::Embedding => return verdict(Verdict::Skip(ImportProblem::Embedding)),
        // Architecture que le llama-server livré ne charge pas : à
        // retélécharger s'il y a un équivalent, sinon rien à faire.
        compat::Support::Unsupported => {
            return verdict(match suggestion() {
                Some(suggestion) => Verdict::Redownload {
                    problem: ImportProblem::Unsupported,
                    suggestion: Some(suggestion),
                },
                None => Verdict::Skip(ImportProblem::Unsupported),
            });
        }
    }
    if report.problem.is_some() {
        return verdict(Verdict::Redownload {
            problem: ImportProblem::OllamaFormat,
            suggestion: suggestion(),
        });
    }
    let quant = report.file_type.and_then(names::file_type_label);
    match found.name.target(quant) {
        Some(id) => (
            Verdict::Ready {
                id,
                file_type: report.file_type,
            },
            !report.chat_template,
        ),
        None => verdict(Verdict::Skip(ImportProblem::InvalidName)),
    }
}

/// Préférence du nom principal d'un groupe : Hugging Face d'abord, puis une
/// étiquette autre que `latest`, puis l'ordre alphabétique.
fn preference(name: &ManifestName) -> (bool, bool, String) {
    (
        !name.is_hf(),
        name.tag.eq_ignore_ascii_case("latest"),
        name.ollama_name(),
    )
}

/// Regroupe les modèles importables aux mêmes poids, au même projecteur et
/// aux mêmes réglages d'Ollama (consigne, paramètres, gabarit).
fn merge(found: Vec<Found>) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    for model in found {
        let key = |model: &Found| {
            matches!(model.verdict, Verdict::Ready { .. }).then(|| {
                (
                    model.weights.as_ref().map(|l| l.digest.clone()),
                    model.projector.as_ref().map(|l| l.digest.clone()),
                    model.settings.clone(),
                )
            })
        };
        let wanted = key(&model);
        if wanted.is_some()
            && let Some(group) = out.iter_mut().find(|other| key(other) == wanted)
        {
            if preference(&model.name) < preference(&group.name) {
                let old = std::mem::replace(group, model);
                group.aliases.push(old.name);
                group.aliases.extend(old.aliases);
            } else {
                group.aliases.push(model.name);
            }
            group.aliases.sort_by_key(ManifestName::ollama_name);
            continue;
        }
        out.push(model);
    }
    out
}
