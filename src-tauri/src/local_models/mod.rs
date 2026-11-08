//! Base locale des modèles de langage (passage d'Ollama à llama-server,
//! contrat `.tools/research/100-llamacpp-lots-1-2-4-contract.md` § 1 et § 3) :
//! modèles installés dans le dossier des modèles de l'app
//! (`<dossier>/<auteur>/<dépôt>/<fichiers d'origine>`), décrits par la table
//! `local_models` (migration 0039), écrite à la fin d'un téléchargement du
//! ModelHub (`hub::downloads`). Les chemins de la table sont relatifs au
//! dossier des modèles : un changement de dossier ne réécrit pas la table.
//!
//! Lecture (`list`, `get`, `LocalModel`) et modèles périmés
//! (`check_outdated`) : pour le moteur (lot L3) et le sélecteur (lot L5), pas
//! encore appelés hors des essais.
#![allow(dead_code)]

pub mod id;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::Store;
use crate::store::local_models::{self as table, StoredLocalModel};

/// Capacités d'un modèle installé, lues dans son GGUF à l'installation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct ModelCapabilities {
    /// Projecteur installé avec le modèle, qui a un encodeur d'images
    /// (`clip.has_vision_encoder` ; clé absente : vrai, sauf projecteur
    /// audio seulement).
    pub vision: bool,
    /// Projecteur installé avec le modèle, qui a un encodeur audio
    /// (`clip.has_audio_encoder` : Voxtral, Qwen2.5-Omni…).
    pub audio: bool,
    /// Le gabarit de conversation parle d'outils.
    pub tools: bool,
    /// Le gabarit sait réfléchir.
    pub thinking: bool,
    /// Jetons FIM : complétion au milieu d'un texte (`/infill`).
    pub fim: bool,
    /// Modèle de base connu comme tel (pas réglé pour la conversation : ni
    /// `general.finetune`, ni « instruct », « chat » ou « it » dans son nom,
    /// fichier d'un convertisseur qui écrit ces clés) ; faux s'il est réglé
    /// ou si le fichier ne le dit pas. Avec `fim` : un modèle
    /// d'autocomplétion (Qwen2.5-Coder « base »).
    pub base: bool,
}

/// Origine d'un modèle installé.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ModelSource {
    /// Téléchargé de Hugging Face.
    Hf,
    /// Importé d'un Ollama (ChatonLM 0.9.x).
    Ollama,
}

/// Modèle installé, chemins absolus (pour le moteur et la page).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocalModel {
    /// `<auteur>/<dépôt>:<QUANT>` ou `@ollama/<nom>:<étiquette>`.
    pub id: String,
    /// Nom affiché : le dépôt sans suffixe `-GGUF` (« Qwen3-0.6B »).
    pub display_name: String,
    /// « auteur/dépôt » ; nom d'origine d'un importé.
    pub repo: String,
    pub quant: String,
    /// Premier fichier des poids (llama-server lit les morceaux suivants).
    pub path: String,
    /// Projecteur de vision (`--mmproj`).
    pub mmproj: Option<String>,
    /// Octets sur le disque, projecteur compris.
    pub size_bytes: u64,
    /// Contexte d'entraînement annoncé par le GGUF.
    pub context_max: Option<u32>,
    pub capabilities: ModelCapabilities,
    pub arch: Option<String>,
    /// Taille annoncée (« 0.6B »).
    pub params_label: Option<String>,
    /// ms depuis 1970.
    pub installed_at: i64,
    pub source: ModelSource,
}

/// Dossier des modèles par défaut : `<données de l'app>/models`.
pub fn default_models_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models")
}

/// Dossier des modèles réglé (table `engine_settings`), sinon celui par
/// défaut.
pub fn models_dir_in(connection: &rusqlite::Connection, data_dir: &Path) -> AppResult<PathBuf> {
    Ok(table::models_dir(connection)?
        .map(PathBuf::from)
        .unwrap_or_else(|| default_models_dir(data_dir)))
}

/// Dossier des modèles (voir [`models_dir_in`]).
pub async fn models_dir(store: &Store, data_dir: &Path) -> AppResult<PathBuf> {
    let data_dir = data_dir.to_owned();
    store
        .call(move |connection| models_dir_in(connection, &data_dir))
        .await
}

/// Chemin absolu d'un chemin relatif de la table (segments séparés par
/// `/`) ; `None` s'il ne se range pas sous le dossier des modèles (segment
/// vide ou `..`, chemin absolu, `\`, `:`, nom réservé de Windows : voir
/// [`crate::hub::files::safe_path`]). Les chemins écrits par l'app le sont
/// toujours ; ceux de la table sont revérifiés (base modifiée, lignes
/// d'une migration) : jamais un fichier hors du dossier.
pub fn absolute(models_dir: &Path, relative: &str) -> Option<PathBuf> {
    crate::hub::files::safe_path(relative).then(|| {
        relative
            .split('/')
            .fold(models_dir.to_path_buf(), |path, segment| path.join(segment))
    })
}

/// Empreinte attendue d'un morceau en cours (`<fichier>.part.sha256`, à
/// côté du `.part`) : un `.part` d'une autre révision repart de zéro au
/// lieu d'être complété avec les octets de la nouvelle.
pub fn expected_path(path: &Path) -> PathBuf {
    let mut expected = path.as_os_str().to_owned();
    expected.push(".part.sha256");
    PathBuf::from(expected)
}

/// Clé d'un chemin pour les comparaisons de fichiers partagés (voir
/// [`file_key_for`]).
pub fn file_key(path: &str) -> String {
    file_key_for(path, cfg!(windows))
}

/// Sous Windows, deux chemins qui ne diffèrent que par la casse désignent
/// le même fichier (Hugging Face qui change la casse d'un dépôt) : clé en
/// minuscules ; sous Linux, le chemin tel quel.
pub fn file_key_for(path: &str, windows: bool) -> String {
    if windows {
        path.to_lowercase()
    } else {
        path.to_owned()
    }
}

/// Fichiers réservés par les téléchargements en cours (chemins complets,
/// clés de [`file_key`]), du premier octet à la fin de leur tâche : un
/// fichier réservé n'est jamais supprimé (suppression d'un modèle,
/// annulation d'un autre téléchargement). La vérification et la suppression
/// se font sous le verrou ([`Reserved::while_locked`]) : aucune réservation
/// ne commence entre les deux.
#[derive(Clone, Default)]
pub struct Reserved(Arc<Mutex<HashSet<String>>>);

impl Reserved {
    fn key(path: &Path) -> String {
        file_key(&path.to_string_lossy())
    }

    /// Réserve `path` ; faux s'il l'est déjà.
    pub fn try_reserve(&self, path: &Path) -> bool {
        self.0.lock().insert(Self::key(path))
    }

    pub fn release(&self, path: &Path) {
        self.0.lock().remove(&Self::key(path));
    }

    /// `f` sous le verrou, avec de quoi savoir si un chemin est réservé.
    pub fn while_locked<R>(&self, f: impl FnOnce(&dyn Fn(&Path) -> bool) -> R) -> R {
        let set = self.0.lock();
        f(&|path: &Path| set.contains(&Self::key(path)))
    }
}

/// Nom affiché d'un modèle : le dépôt sans auteur ni suffixe `-GGUF`.
pub fn display_name(model: &StoredLocalModel) -> String {
    let name = model.repo.rsplit('/').next().unwrap_or(&model.repo);
    crate::hub::catalog::display_name(name)
}

/// Ligne de la table en modèle à chemins absolus ; `None` (journalisé) si
/// un de ses chemins ne se range pas sous le dossier des modèles.
pub fn resolve(models_dir: &Path, model: StoredLocalModel) -> Option<LocalModel> {
    let safe = |relative: &str| {
        let path = absolute(models_dir, relative);
        if path.is_none() {
            // i18n-ignore : journal.
            crate::app_log!(
                "chatonlm: local model {} skipped: unsafe path {relative:?}",
                model.id
            );
        }
        path.map(|path| path.to_string_lossy().into_owned())
    };
    let path = safe(&model.files[0])?;
    let mmproj = match model.mmproj.as_deref() {
        Some(relative) => Some(safe(relative)?),
        None => None,
    };
    Some(LocalModel {
        display_name: display_name(&model),
        path,
        mmproj,
        id: model.id,
        repo: model.repo,
        quant: model.quant,
        size_bytes: model.size_bytes,
        context_max: model.context_max,
        capabilities: model.capabilities,
        arch: model.arch,
        params_label: model.params_label,
        installed_at: model.installed_at,
        source: model.source,
    })
}

/// Modèles installés, du plus récent au plus ancien.
pub async fn list(store: &Store, data_dir: &Path) -> AppResult<Vec<LocalModel>> {
    let data_dir = data_dir.to_owned();
    store
        .call(move |connection| {
            let dir = models_dir_in(connection, &data_dir)?;
            Ok(table::list(connection)?
                .into_iter()
                .filter_map(|model| resolve(&dir, model))
                .collect())
        })
        .await
}

/// Modèle `id` (quantification sans la casse), s'il est installé.
pub async fn get(store: &Store, data_dir: &Path, id: &str) -> AppResult<Option<LocalModel>> {
    let data_dir = data_dir.to_owned();
    let id = id.to_owned();
    store
        .call(move |connection| {
            let dir = models_dir_in(connection, &data_dir)?;
            Ok(table::get(connection, &id)?.and_then(|model| resolve(&dir, model)))
        })
        .await
}

/// Supprime le modèle `id` : ses fichiers (sauf ceux qu'un autre modèle
/// installé partage, comme un projecteur de vision, et ceux qu'un
/// téléchargement en cours a réservés : `reserved`), leurs morceaux `.part`,
/// les dossiers devenus vides jusqu'au dossier des modèles, puis sa ligne.
/// Un fichier impossible à supprimer (sous Windows, un fichier ouvert par le
/// moteur) arrête tout : la ligne reste, une nouvelle tentative finit le
/// travail. À faire après l'arrêt du moteur qui sert ce modèle (lot L3).
pub async fn delete(
    store: &Store,
    data_dir: &Path,
    id: &str,
    reserved: &Reserved,
) -> AppResult<()> {
    let data_dir = data_dir.to_owned();
    let wanted = id.to_owned();
    let (model, others, dir) = store
        .call(move |connection| {
            let dir = models_dir_in(connection, &data_dir)?;
            let all = table::list(connection)?;
            let Some(model) = table::get(connection, &wanted)? else {
                return Err(AppError::NotFound(t!("localModels.notFound")));
            };
            let others: Vec<String> = all
                .iter()
                .filter(|other| other.id != model.id)
                .flat_map(|other| other.all_files().map(str::to_owned))
                .collect();
            Ok((model, others, dir))
        })
        .await?;
    let files = unshared_files(&model, others.iter().map(String::as_str), cfg!(windows));
    let root = dir.clone();
    let reserved = reserved.clone();
    tauri::async_runtime::spawn_blocking(move || {
        reserved.while_locked(|is_reserved| remove_files(&root, &files, is_reserved))
    })
    .await
    .map_err(|error| AppError::Internal(error.to_string()))??;
    let id = model.id.clone();
    store
        .call(move |connection| table::remove(connection, &id).map(|_| ()))
        .await
}

/// Fichiers de `model` qu'aucun des fichiers `others` (ceux des autres
/// modèles installés) ne désigne ; sous Windows, sans la casse
/// ([`file_key_for`]).
pub fn unshared_files<'a>(
    model: &StoredLocalModel,
    others: impl IntoIterator<Item = &'a str>,
    windows: bool,
) -> Vec<String> {
    let shared: HashSet<String> = others
        .into_iter()
        .map(|path| file_key_for(path, windows))
        .collect();
    model
        .all_files()
        .filter(|path| !shared.contains(&file_key_for(path, windows)))
        .map(str::to_owned)
        .collect()
}

/// Supprime `files` (relatifs à `root`), leurs `.part` et leurs empreintes
/// attendues, puis les dossiers devenus vides sous `root`. Un fichier
/// réservé par un téléchargement (`is_reserved`) reste, comme un chemin qui
/// ne se range pas sous `root` (journalisé).
fn remove_files(
    root: &Path,
    files: &[String],
    is_reserved: &dyn Fn(&Path) -> bool,
) -> AppResult<()> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for relative in files {
        let Some(path) = absolute(root, relative) else {
            // i18n-ignore : journal.
            crate::app_log!("chatonlm: unsafe model path left alone: {relative:?}");
            continue;
        };
        if is_reserved(&path) {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(AppError::System(delete_error(relative, &error))),
        }
        let _ = std::fs::remove_file(crate::image_models::downloads::part_path(&path));
        let _ = std::fs::remove_file(expected_path(&path));
        for dir in path.ancestors().skip(1) {
            if dir == root || !dir.starts_with(root) {
                break;
            }
            if !dirs.iter().any(|known| known == dir) {
                dirs.push(dir.to_path_buf());
            }
        }
    }
    // Les plus profonds d'abord ; un dossier encore occupé reste.
    dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    for dir in dirs {
        let _ = std::fs::remove_dir(&dir);
    }
    Ok(())
}

/// Texte d'un fichier impossible à supprimer (voir [`delete_error_for`]).
pub fn delete_error(file: &str, error: &std::io::Error) -> String {
    delete_error_for(file, error, cfg!(windows))
}

/// Sous Windows, un fichier ouvert (le moteur le lit) refuse sa suppression :
/// accès refusé, ou violation de partage ou de verrou (codes 32 et 33) ;
/// sinon, et sous Linux, l'erreur du système.
pub fn delete_error_for(file: &str, error: &std::io::Error, windows: bool) -> String {
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    let in_use = windows
        && (error.kind() == std::io::ErrorKind::PermissionDenied
            || matches!(
                error.raw_os_error(),
                Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
            ));
    if in_use {
        t!("localModels.inUse", file = file)
    } else {
        t!("localModels.deleteFailed", file = file, error = error)
    }
}

/// Délai de la lecture de la fiche pour la vérification des mises à jour.
const OUTDATED_TIMEOUT: Duration = Duration::from_secs(15);

/// Le modèle installé `model` n'est plus celui de la branche `main` de
/// Hugging Face (fichier changé ou disparu, voir [`crate::hub::files::outdated`]) ;
/// `None` : rien à juger (importé, fiche illisible ou injoignable). Pour le
/// bandeau des mises à jour (lot L5).
pub async fn check_outdated(hf_url: &str, model: &StoredLocalModel) -> Option<bool> {
    if model.source != ModelSource::Hf {
        return None;
    }
    let listing = crate::hub::files::fetch(hf_url, &model.repo, OUTDATED_TIMEOUT)
        .await
        .ok()?;
    Some(crate::hub::files::outdated(
        &model.repo,
        model
            .all_files()
            .map(|path| (path, model.sha256.get(path).map(String::as_str))),
        &listing,
    ))
}

#[cfg(test)]
mod tests;
