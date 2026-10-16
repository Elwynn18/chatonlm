//! Modèles d'images (état Tauri) : liste (installés, en cours, recommandés),
//! machine, recherche et fiche de Hugging Face, choix d'un fichier (mis en
//! file), suppression avec références, modèle choisi pour la génération.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::catalog::{ImageCatalog, ImageRepo, Known, Recognized};
use super::downloads::{
    ImageDownloads, ImageItem, basename, blob_path, missing_bytes, part_path, present, safe_name,
    space_check, total, unique,
};
use super::families::{Families, Family, Piece, Recommendation, accepted_gguf, single_file};
use super::types::{
    ImageComponentInfo, ImageComponentRole, ImageFileChoice, ImageMachine, ImageModelInfo,
    ImageRepoDetail, ImageRepoSummary, ImageRepoUnsupported, ImageSearchResult,
};
use crate::error::{AppError, AppResult};
use crate::hub::curation::CURATION;
use crate::hub::hardware::{Detector, KEEP, Machine};
use crate::hub::types::HubProblem;
use crate::i18n::t;
use crate::image_gen::{ImageGen, ModelSpec, Sampling};
use crate::store::image_models::{self as stored, StoredFile, StoredModel};
use crate::store::{Store, now_ms};

/// Attente de la fin d'une image avant de libérer le moteur (suppression).
const RELEASE_WAIT: Duration = Duration::from_secs(5);
/// Attente de la vraie fin d'une tâche annulée (suppression).
const IDLE_WAIT: Duration = Duration::from_secs(5);
/// Mémoire supposée si la machine ne se lit pas.
const UNKNOWN_MEMORY: u64 = 8 * 1024 * 1024 * 1024;

/// Quantifications conseillées, de la plus petite à la plus grande.
const PREFERRED: &[&str] = &["Q4_0", "Q4_K", "Q4_K_M", "Q5_K_M", "Q8_0"];

/// D'où vient la machine.
enum MachineSource {
    Detect(Detector),
    /// Imposée (essais).
    #[cfg(test)]
    Fixed(Option<Machine>),
}

pub struct ImageModels {
    families: Arc<Families>,
    catalog: ImageCatalog,
    /// Dossier des modèles d'images (`blobs/` dessous).
    root: PathBuf,
    machine: MachineSource,
    /// Choix, suppressions et retraits un par un : les références d'un
    /// fichier ne changent pas entre leur calcul et l'effacement.
    changes: tokio::sync::Mutex<()>,
}

/// Refus montré à la page.
fn refuse(message: String) -> AppError {
    AppError::Invalid(message)
}

/// Problème de Hugging Face, en message.
fn problem_error(problem: &HubProblem) -> AppError {
    refuse(match problem {
        HubProblem::Offline => t!("imageModels.offline"),
        HubProblem::Unavailable { .. } => t!("imageModels.unavailable"),
        HubProblem::RateLimited { retry_after } => {
            t!("imageModels.rateLimited", seconds = retry_after)
        }
        HubProblem::NotFound => t!("imageModels.notFound"),
        HubProblem::Restricted => t!("imageModels.gated"),
        HubProblem::Failed { message } => t!("imageModels.hfFailed", error = message),
    })
}

/// Fichier d'une pièce annexe.
fn piece_file(piece: &Piece) -> StoredFile {
    StoredFile {
        role: piece.role,
        component: piece.component,
        repo: piece.repo.clone(),
        revision: piece.revision.clone(),
        file: piece.file.clone(),
        size: piece.size,
        sha256: piece.sha256.to_ascii_lowercase(),
        license: piece.license.clone(),
    }
}

/// Nom affiché d'un fichier : son nom sans extension.
fn display_name(file: &str) -> String {
    let name = basename(file);
    name.rsplit_once('.')
        .map_or(name, |(stem, _)| stem)
        .to_owned()
}

/// Modèle d'une recommandation (sans réseau).
fn recommended_model(family: &Family, recommendation: &Recommendation) -> StoredModel {
    let mut files = vec![StoredFile {
        role: ImageComponentRole::Model,
        component: family.model_component,
        repo: recommendation.repo.clone(),
        revision: recommendation.revision.clone(),
        file: recommendation.file.clone(),
        size: recommendation.size,
        sha256: recommendation.sha256.to_ascii_lowercase(),
        license: recommendation.license.clone(),
    }];
    files.extend(family.pieces.iter().map(piece_file));
    StoredModel {
        id: format!("{}/{}", recommendation.repo, recommendation.file),
        repo: recommendation.repo.clone(),
        revision: recommendation.revision.clone(),
        file: recommendation.file.clone(),
        family: family.id.clone(),
        name: recommendation.name.clone(),
        license: recommendation.license.clone(),
        uncensored: false,
        ever_installed: false,
        files,
        created_at: now_ms(),
    }
}

/// Fichier conseillé parmi `files` (taille, quantification) : la plus grande
/// quantification conseillée (Q4_K_M, Q4_K, Q4_0, Q5_K_M, Q8_0) dont la
/// taille avec les pièces (`pieces`) tient dans `memory` ; sinon la plus
/// petite. Sans quantification conseillée : les mêmes règles sur tous les
/// fichiers.
pub fn recommend(files: &[(u64, Option<String>)], pieces: u64, memory: u64) -> Option<usize> {
    let preferred: Vec<usize> = (0..files.len())
        .filter(|&index| {
            files[index]
                .1
                .as_deref()
                .is_some_and(|quant| PREFERRED.contains(&quant))
        })
        .collect();
    let pool: Vec<usize> = if preferred.is_empty() {
        (0..files.len()).collect()
    } else {
        preferred
    };
    let fits = pool
        .iter()
        .copied()
        .filter(|&index| files[index].0.saturating_add(pieces) <= memory)
        .max_by_key(|&index| files[index].0);
    fits.or_else(|| pool.iter().copied().min_by_key(|&index| files[index].0))
}

/// Mémoire qui compte pour un modèle d'images : celle de la plus grande
/// carte graphique, sinon la RAM.
fn memory_of(machine: Option<&Machine>) -> u64 {
    match machine {
        Some(machine) => machine
            .gpus
            .iter()
            .map(|gpu| gpu.vram_total)
            .max()
            .unwrap_or(machine.ram_total),
        None => UNKNOWN_MEMORY,
    }
}

impl ImageModels {
    /// Modèles rangés sous `root`, cache de Hugging Face sous `cache_dir`.
    pub fn new(root: PathBuf, cache_dir: PathBuf) -> Self {
        Self {
            families: Arc::new(Families::load()),
            catalog: ImageCatalog::new(cache_dir),
            root,
            machine: MachineSource::Detect(Detector::default()),
            changes: tokio::sync::Mutex::new(()),
        }
    }

    /// Essais : familles, faux Hugging Face et machine imposés.
    #[cfg(test)]
    pub fn for_tests(
        root: PathBuf,
        catalog: ImageCatalog,
        families: Families,
        machine: Option<Machine>,
    ) -> Self {
        Self {
            families: Arc::new(families),
            catalog,
            root,
            machine: MachineSource::Fixed(machine),
            changes: tokio::sync::Mutex::new(()),
        }
    }

    /// Machine qui fera les images (celle de `CHATONLM_E2E_HARDWARE` dans
    /// les essais de l'app).
    async fn machine(&self) -> Option<Machine> {
        match &self.machine {
            MachineSource::Detect(detector) => {
                #[cfg(any(debug_assertions, feature = "e2e"))]
                if let Some(injected) = crate::hub::hardware::injected() {
                    return injected;
                }
                detector.machine_within(KEEP).await
            }
            #[cfg(test)]
            MachineSource::Fixed(machine) => machine.clone(),
        }
    }

    /// Ce que la machine permet.
    pub async fn machine_info(&self, engine: &ImageGen) -> ImageMachine {
        let machine = self.machine().await;
        let root = self.root.clone();
        let free_bytes =
            tauri::async_runtime::spawn_blocking(move || crate::disk::free_space_for(&root))
                .await
                .ok()
                .flatten();
        ImageMachine {
            engine_available: engine.available(),
            gpu: machine.is_some_and(|machine| !machine.gpus.is_empty()),
            missing_cpu_features: engine
                .missing_cpu_features()
                .iter()
                .map(|feature| (*feature).to_owned())
                .collect(),
            free_bytes,
        }
    }

    /// Fiche d'un modèle telle que la page la voit.
    fn info(
        &self,
        model: &StoredModel,
        recorded: bool,
        downloading: bool,
        selected: &str,
    ) -> ImageModelInfo {
        let family = self.families.get(&model.family).map_or_else(
            || super::types::ImageFamily {
                id: model.family.clone(),
                name: model.family.clone(),
            },
            Family::info,
        );
        let files = unique(&model.files);
        let all_present = files.iter().all(|file| present(&self.root, file));
        let missing = missing_bytes(
            files
                .iter()
                .map(|file| (blob_path(&self.root, file), file.size)),
        );
        ImageModelInfo {
            id: model.id.clone(),
            name: model.name.clone(),
            repo: model.repo.clone(),
            file: model.file.clone(),
            revision: model.revision.clone(),
            family,
            license: model.license.clone(),
            open_license: CURATION.open_license(model.license.as_deref()),
            uncensored: model.uncensored,
            recommended: self
                .families
                .recommendation(&model.repo, &model.file)
                .is_some(),
            installed: recorded && all_present && !downloading,
            size_bytes: total(files.iter().map(|file| file.size)),
            missing_bytes: missing,
            selected: recorded && model.id == selected,
            components: model
                .files
                .iter()
                .map(|file| ImageComponentInfo {
                    role: file.role,
                    repo: file.repo.clone(),
                    file: file.file.clone(),
                    size_bytes: file.size,
                    license: file.license.clone(),
                    present: present(&self.root, file),
                })
                .collect(),
        }
    }

    /// D'abord les modèles enregistrés qui ne sont pas de simples
    /// recommandations (non recommandés, installés, ou avec un téléchargement
    /// même fini, échoué ou annulé), du plus ancien au plus récent ; puis les
    /// recommandations non installées.
    pub async fn list(
        &self,
        downloads: &ImageDownloads,
        store: &Store,
    ) -> AppResult<Vec<ImageModelInfo>> {
        let (models, selected) = store
            .call(|connection| Ok((stored::list(connection)?, stored::selected(connection)?)))
            .await?;
        let listed: HashSet<String> = downloads
            .list()
            .into_iter()
            .map(|download| download.id)
            .collect();
        let (mut first, mut last) = (Vec::new(), Vec::new());
        for model in &models {
            let info = self.info(model, true, downloads.is_active(&model.id), &selected);
            if !info.recommended || info.installed || listed.contains(&model.id) {
                first.push(info);
            } else {
                last.push(info);
            }
        }
        for recommendation in &self.families.recommendations {
            let Some(family) = self.families.get(&recommendation.family) else {
                continue;
            };
            let model = recommended_model(family, recommendation);
            if !models.iter().any(|known| known.id == model.id) {
                last.push(self.info(&model, false, false, &selected));
            }
        }
        first.extend(last);
        Ok(first)
    }

    /// Réglage « Show uncensored models ».
    async fn show_uncensored(store: &Store) -> AppResult<bool> {
        store
            .call(|connection| Ok(crate::store::settings::get(connection, None)?.show_uncensored))
            .await
    }

    fn summary(&self, repo: &ImageRepo, recognized: &Recognized) -> ImageRepoSummary {
        ImageRepoSummary {
            id: repo.id.clone(),
            author: repo.author.clone(),
            name: repo.name().to_owned(),
            family: match &recognized.known {
                Known::Family(id) => self.families.get(id).map(Family::info),
                Known::Unsupported | Known::Unknown => None,
            },
            downloads: repo.downloads,
            likes: repo.likes,
            last_modified: repo.last_modified.clone(),
            license: repo.license.clone(),
            open_license: CURATION.open_license(repo.license.as_deref()),
            uncensored: recognized.uncensored,
            gated: repo.gated,
        }
    }

    /// Recherche (vide : dépôts populaires) ; non censurés retirés sans le
    /// réglage.
    pub async fn search(&self, store: &Store, query: &str) -> AppResult<ImageSearchResult> {
        let shown = Self::show_uncensored(store).await?;
        let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
        let (gguf, single) = self.catalog.search_lists(&query).await;
        let problem = gguf.problem.clone().or_else(|| single.problem.clone());
        let stale = problem.is_some() && (gguf.entry.is_some() || single.entry.is_some());
        let mut repos: Vec<ImageRepo> = Vec::new();
        let mut push = |repo: &ImageRepo| {
            if !repos
                .iter()
                .any(|known| known.id.eq_ignore_ascii_case(&repo.id))
            {
                repos.push(repo.clone());
            }
        };
        if let Some(entry) = &gguf.entry {
            entry.data.iter().for_each(&mut push);
        }
        // Familles à fichier unique : dépôts reconnus SD 1.5 ou SDXL (cran 0
        // ou 1) qui ont un fichier utilisable.
        if let Some(entry) = &single.entry {
            for repo in &entry.data {
                let family = self
                    .families
                    .recognize(std::slice::from_ref(&repo.id), false)
                    .or_else(|| self.families.recognize(&repo.bases, false));
                if let Some(super::families::Recognition::Family(family)) = family
                    && family.single_file
                    && !repo.adapter
                    && repo
                        .files
                        .iter()
                        .any(|file| family.accepts(&file.path, file.size))
                {
                    push(repo);
                }
            }
        }
        let recognized = self
            .catalog
            .recognize_all(&self.families, &repos, ImageCatalog::second_step_max())
            .await;
        let mut found: Vec<ImageRepoSummary> = repos
            .iter()
            .zip(&recognized)
            .filter(|(_, recognized)| shown || !recognized.uncensored)
            .map(|(repo, recognized)| self.summary(repo, recognized))
            .collect();
        found.sort_by_key(|repo| std::cmp::Reverse(repo.downloads));
        Ok(ImageSearchResult {
            repos: found,
            problem,
            stale,
        })
    }

    /// Fichiers du dépôt proposés comme modèle (taille et empreinte connues) ;
    /// aucun pour un adaptateur (LoRA...).
    fn choices<'a>(
        &self,
        repo: &'a ImageRepo,
        family: Option<&Family>,
    ) -> Vec<&'a super::catalog::RepoBlob> {
        if repo.adapter {
            return Vec::new();
        }
        repo.files
            .iter()
            .filter(|blob| match family {
                Some(family) => family.accepts(&blob.path, blob.size),
                None => accepted_gguf(&blob.path) || single_file(&blob.path),
            })
            .filter(|blob| blob.size.is_some() && blob.sha256.is_some() && safe_name(&blob.path))
            .collect()
    }

    /// Fiche d'un dépôt (lisible même non censurée : `uncensored` le dit).
    pub async fn repo(
        &self,
        downloads: &ImageDownloads,
        store: &Store,
        id: &str,
    ) -> AppResult<ImageRepoDetail> {
        let loaded = self.catalog.repo(id).await;
        let Some(entry) = loaded.entry else {
            return Ok(empty_detail(
                id,
                loaded.problem.unwrap_or(HubProblem::NotFound),
            ));
        };
        let repo = &entry.data;
        let recognized = self.catalog.recognize(&self.families, repo).await;
        let family = match &recognized.known {
            Known::Family(id) => self.families.get(id),
            Known::Unsupported | Known::Unknown => None,
        };
        let choices = self.choices(repo, family);
        // Installés : ligne en base et tous les fichiers présents.
        let root = self.root.clone();
        let installed: HashSet<String> = store
            .call(move |connection| {
                Ok(stored::list(connection)?
                    .into_iter()
                    .filter(|model| unique(&model.files).iter().all(|file| present(&root, file)))
                    .map(|model| model.id)
                    .collect())
            })
            .await?;
        let unsupported = if repo.gated {
            Some(ImageRepoUnsupported::Gated)
        } else {
            match (&recognized.known, choices.is_empty()) {
                (Known::Unsupported, _) => Some(ImageRepoUnsupported::UnsupportedFamily),
                (Known::Unknown, _) => Some(ImageRepoUnsupported::UnknownFamily),
                (Known::Family(_), true) => Some(ImageRepoUnsupported::NoUsableFile),
                (Known::Family(_), false) => None,
            }
        };
        let quants: Vec<(u64, Option<String>)> = choices
            .iter()
            .map(|blob| {
                (
                    blob.size.unwrap_or(0),
                    crate::hub::quant::display_label(&blob.path),
                )
            })
            .collect();
        let advised = match (family, &unsupported) {
            (Some(family), None) => {
                let pieces = total(family.pieces.iter().map(|piece| piece.size));
                recommend(&quants, pieces, memory_of(self.machine().await.as_ref()))
            }
            _ => None,
        };
        let files = choices
            .iter()
            .zip(quants)
            .enumerate()
            .map(|(index, (blob, (size, quant)))| {
                let model_id = format!("{}/{}", repo.id, blob.path);
                ImageFileChoice {
                    file: blob.path.clone(),
                    size_bytes: size,
                    quant,
                    recommended: advised == Some(index),
                    installed: installed.contains(&model_id) || downloads.is_active(&model_id),
                }
            })
            .collect();
        let components = family.map_or_else(Vec::new, |family| {
            family
                .pieces
                .iter()
                .map(|piece| {
                    let file = piece_file(piece);
                    ImageComponentInfo {
                        role: piece.role,
                        repo: piece.repo.clone(),
                        file: piece.file.clone(),
                        size_bytes: piece.size,
                        license: piece.license.clone(),
                        present: present(&self.root, &file),
                    }
                })
                .collect()
        });
        Ok(ImageRepoDetail {
            summary: self.summary(repo, &recognized),
            revision: repo.sha.clone().unwrap_or_else(|| "main".to_owned()),
            files,
            components,
            unsupported,
            stale: loaded.problem.is_some(),
            problem: loaded.problem,
        })
    }

    /// Modèle de `file` dans `repo`, lu sur la fiche de Hugging Face, ou
    /// refusé (révision `revision` qui n'est plus celle de la fiche,
    /// restreint, non censuré masqué, famille inconnue ou écartée, fichier
    /// absent, inutilisable ou sans empreinte).
    async fn model_from_hub(
        &self,
        repo_id: &str,
        file: &str,
        revision: &str,
        show_uncensored: bool,
    ) -> AppResult<StoredModel> {
        let loaded = self.catalog.repo(repo_id).await;
        let Some(entry) = loaded.entry else {
            return Err(problem_error(
                loaded.problem.as_ref().unwrap_or(&HubProblem::NotFound),
            ));
        };
        let repo = &entry.data;
        if repo.gated {
            return Err(refuse(t!("imageModels.gated")));
        }
        let recognized = self.catalog.recognize(&self.families, repo).await;
        if recognized.uncensored && !show_uncensored {
            return Err(refuse(t!("imageModels.uncensoredHidden")));
        }
        let family = match &recognized.known {
            Known::Family(id) => self
                .families
                .get(id)
                .ok_or_else(|| refuse(t!("imageModels.unknownFamily")))?,
            Known::Unsupported => return Err(refuse(t!("imageModels.unsupportedFamily"))),
            Known::Unknown => return Err(refuse(t!("imageModels.unknownFamily"))),
        };
        // Une base n'a pas pu être lue : non censuré possible, par prudence.
        if recognized.unverified && !show_uncensored {
            return Err(refuse(t!("imageModels.unverified")));
        }
        let blob = repo
            .blob(file)
            .ok_or_else(|| refuse(t!("imageModels.fileNotFound", file = file)))?;
        if repo.adapter || !family.accepts(file, blob.size) || !safe_name(file) {
            return Err(refuse(t!("imageModels.fileNotUsable", file = file)));
        }
        let (Some(size), Some(sha256)) = (blob.size, blob.sha256.clone()) else {
            return Err(refuse(t!("imageModels.noChecksum", file = file)));
        };
        let current = repo.sha.clone().unwrap_or_else(|| "main".to_owned());
        if !revision.is_empty() && revision != current {
            return Err(refuse(t!("imageModels.revisionChanged")));
        }
        let revision = current;
        let mut files = vec![StoredFile {
            role: ImageComponentRole::Model,
            component: family.model_component,
            repo: repo.id.clone(),
            revision: revision.clone(),
            file: file.to_owned(),
            size,
            sha256,
            license: repo.license.clone(),
        }];
        files.extend(family.pieces.iter().map(piece_file));
        Ok(StoredModel {
            id: format!("{}/{file}", repo.id),
            repo: repo.id.clone(),
            revision,
            file: file.to_owned(),
            family: family.id.clone(),
            name: display_name(file),
            license: repo.license.clone(),
            uncensored: recognized.uncensored || recognized.unverified,
            ever_installed: false,
            files,
            created_at: now_ms(),
        })
    }

    /// Choix d'un fichier : modèle enregistré (gardé même si le
    /// téléchargement échoue : « Try again »), puis ses fichiers manquants mis
    /// en file. Déjà en cours : rien. Révision (`revision`, vide : la leur) :
    /// celle d'un modèle enregistré ou d'une recommandation est reprise
    /// telle quelle ; une autre fait relire la fiche du dépôt (taille et
    /// empreinte de sa révision actuelle), sauf pour un modèle installé, qui
    /// est alors refusé. Refusé aussi si la place manque (`octets encore
    /// nécessaires + 100 Mio > libre`).
    pub async fn download(
        &self,
        downloads: &ImageDownloads,
        store: &Store,
        repo: &str,
        file: &str,
        revision: &str,
    ) -> AppResult<()> {
        let _changes = self.changes.lock().await;
        let id = format!("{repo}/{file}");
        if downloads.is_active(&id) {
            return Ok(());
        }
        let show_uncensored = Self::show_uncensored(store).await?;
        let known = {
            let id = id.clone();
            store
                .call(move |connection| stored::get(connection, &id))
                .await?
        };
        let same = |pinned: &str| revision.is_empty() || revision == pinned;
        let recommendation = self.families.recommendation(repo, file);
        let model = match (&known, recommendation) {
            (Some(known), _) if same(&known.revision) => known.clone(),
            (Some(known), _) if self.installed(known) => {
                return Err(refuse(t!("imageModels.revisionChanged")));
            }
            (_, Some(recommendation)) if same(&recommendation.revision) => {
                let family = self
                    .families
                    .get(&recommendation.family)
                    .ok_or_else(|| refuse(t!("imageModels.unknownFamily")))?;
                recommended_model(family, recommendation)
            }
            _ => {
                let mut model = self
                    .model_from_hub(repo, file, revision, show_uncensored)
                    .await?;
                // Recommandation à un commit plus récent : son nom gardé, la
                // révision, la taille et l'empreinte venues de la fiche.
                if let Some(recommendation) = recommendation {
                    model.name = recommendation.name.clone();
                }
                model
            }
        };
        if model.uncensored && !show_uncensored {
            return Err(refuse(t!("imageModels.uncensoredHidden")));
        }
        let root = self.root.clone();
        let files = model.files.clone();
        tauri::async_runtime::spawn_blocking(move || {
            space_check(
                unique(&files)
                    .into_iter()
                    .map(|file| (blob_path(&root, file), file.size)),
                crate::disk::free_space_for(&root),
            )
        })
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?
        .map_err(refuse)?;
        {
            let model = model.clone();
            store
                .call(move |connection| stored::save(connection, &model))
                .await?;
        }
        // Relu à une autre révision : les fichiers de l'ancienne qui ne
        // servent plus partent.
        if let Some(previous) = known
            && previous.files != model.files
        {
            let removable = self
                .unreferenced(downloads, store, &previous.files, &model.id)
                .await?
                .into_iter()
                .filter(|path| {
                    !model
                        .files
                        .iter()
                        .any(|file| blob_path(&self.root, file) == *path)
                })
                .collect::<Vec<_>>();
            remove_files(&removable, &downloads.files_in_use()).await?;
        }
        let root = &self.root;
        downloads.enqueue(Arc::new(ImageItem {
            id: model.id.clone(),
            name: model.name.clone(),
            files: unique(&model.files)
                .into_iter()
                .filter(|file| !present(root, file))
                .cloned()
                .collect(),
        }));
        Ok(())
    }

    /// Tous les fichiers du modèle présents.
    fn installed(&self, model: &StoredModel) -> bool {
        unique(&model.files)
            .iter()
            .all(|file| present(&self.root, file))
    }

    /// Fichiers de `files` qu'aucun modèle enregistré (`except` excepté) ni
    /// aucun téléchargement en cours n'emploie.
    async fn unreferenced(
        &self,
        downloads: &ImageDownloads,
        store: &Store,
        files: &[StoredFile],
        except: &str,
    ) -> AppResult<Vec<PathBuf>> {
        let others: Vec<StoredModel> = store.call(|connection| stored::list(connection)).await?;
        let mut used: HashSet<PathBuf> = others
            .iter()
            .filter(|model| model.id != except)
            .flat_map(|model| model.files.iter())
            .map(|file| blob_path(&self.root, file))
            .collect();
        used.extend(downloads.files_in_use());
        let mut removable: Vec<PathBuf> = Vec::new();
        for file in unique(files) {
            let path = blob_path(&self.root, file);
            if !used.contains(&path) && !removable.contains(&path) {
                removable.push(path);
            }
        }
        Ok(removable)
    }

    /// Supprime le modèle `id` : refusé pendant son téléchargement ; le
    /// moteur est libéré s'il charge un fichier qui va disparaître (ou celui
    /// du modèle) ; puis chaque fichier qu'aucun autre modèle ni
    /// téléchargement n'emploie, puis sa ligne (et le choix pour la
    /// génération s'il le désignait) : un fichier qui ne s'efface pas laisse
    /// le modèle en place, jamais un fichier orphelin.
    pub async fn delete(
        &self,
        downloads: &ImageDownloads,
        store: &Store,
        engine: Option<&ImageGen>,
        id: &str,
    ) -> AppResult<()> {
        let _changes = self.changes.lock().await;
        if downloads.is_active(id) {
            return Err(refuse(t!("imageModels.downloading")));
        }
        // Annulé à l'instant : sa tâche peut encore écrire un `.part`.
        if !downloads.wait_idle(id, IDLE_WAIT).await {
            return Err(refuse(t!("imageModels.stillDownloading")));
        }
        let found = {
            let id = id.to_owned();
            store
                .call(move |connection| stored::get(connection, &id))
                .await?
        };
        let Some(model) = found else {
            downloads.dismiss(id);
            return Ok(());
        };
        if let Some(engine) = engine {
            let removable = self
                .unreferenced(downloads, store, &model.files, &model.id)
                .await?;
            let own: Vec<PathBuf> = model
                .files
                .iter()
                .filter(|file| file.role == ImageComponentRole::Model)
                .map(|file| blob_path(&self.root, file))
                .collect();
            let uses = |spec: &ModelSpec| {
                spec.files
                    .iter()
                    .any(|(_, path)| own.contains(path) || removable.contains(path))
            };
            if !engine.release_if(uses, RELEASE_WAIT).await {
                return Err(refuse(t!("imageModels.inUse")));
            }
        }
        self.forget(downloads, store, &model).await?;
        downloads.dismiss(id);
        Ok(())
    }

    /// Efface les fichiers de `model` qui ne servent plus (références
    /// recalculées à l'instant), puis sa ligne.
    async fn forget(
        &self,
        downloads: &ImageDownloads,
        store: &Store,
        model: &StoredModel,
    ) -> AppResult<()> {
        let removable = self
            .unreferenced(downloads, store, &model.files, &model.id)
            .await?;
        remove_files(&removable, &downloads.files_in_use()).await?;
        let id = model.id.clone();
        store
            .call(move |connection| stored::delete(connection, &id))
            .await?;
        Ok(())
    }

    /// Retire un téléchargement fini, échoué ou annulé ; un modèle jamais
    /// installé et non recommandé est alors oublié (sa ligne et les fichiers
    /// qui ne servent plus).
    pub async fn dismiss(
        &self,
        downloads: &ImageDownloads,
        store: &Store,
        id: &str,
    ) -> AppResult<()> {
        let _changes = self.changes.lock().await;
        if downloads.is_active(id) {
            return Ok(());
        }
        downloads.dismiss(id);
        let found = {
            let id = id.to_owned();
            store
                .call(move |connection| stored::get(connection, &id))
                .await?
        };
        let Some(model) = found else {
            return Ok(());
        };
        let recommended = self
            .families
            .recommendation(&model.repo, &model.file)
            .is_some();
        if model.ever_installed
            || self.installed(&model)
            || recommended
            || !downloads.wait_idle(id, IDLE_WAIT).await
        {
            return Ok(());
        }
        self.forget(downloads, store, &model).await
    }

    /// Choisit le modèle de la génération (`""` : aucun) ; refusé s'il n'est
    /// pas installé.
    pub async fn use_model(
        &self,
        downloads: &ImageDownloads,
        store: &Store,
        id: &str,
    ) -> AppResult<()> {
        if !id.is_empty() {
            let found = {
                let id = id.to_owned();
                store
                    .call(move |connection| stored::get(connection, &id))
                    .await?
            };
            let installed = found.is_some_and(|model| {
                unique(&model.files)
                    .iter()
                    .all(|file| present(&self.root, file))
            }) && !downloads.is_active(id);
            if !installed {
                return Err(refuse(t!("imageModels.notInstalled")));
            }
        }
        let id = id.to_owned();
        store
            .call(move |connection| stored::select(connection, &id))
            .await
    }

    /// Modèle choisi pour la génération, prêt pour le moteur : fichiers
    /// (tous présents) et réglages de sa famille ; `None` : aucun, ou plus
    /// utilisable.
    #[allow(dead_code, reason = "branché à la génération au lot 4")]
    pub async fn selected_spec(&self, store: &Store) -> Option<(ModelSpec, Sampling)> {
        let model = store
            .call(|connection| {
                let id = stored::selected(connection)?;
                stored::get(connection, &id)
            })
            .await
            .ok()??;
        let family = self.families.get(&model.family)?;
        if !model.files.iter().all(|file| present(&self.root, file)) {
            return None;
        }
        Some((
            ModelSpec {
                files: model
                    .files
                    .iter()
                    .map(|file| (file.component, blob_path(&self.root, file)))
                    .collect(),
                options: family.options.clone(),
            },
            family.sampling.clone(),
        ))
    }
}

/// Fiche d'un dépôt que Hugging Face n'a pas donnée (introuvable, réseau) :
/// le problème seul.
fn empty_detail(id: &str, problem: HubProblem) -> ImageRepoDetail {
    let (author, name) = id.split_once('/').unwrap_or(("", id));
    ImageRepoDetail {
        summary: ImageRepoSummary {
            id: id.to_owned(),
            author: author.to_owned(),
            name: name.to_owned(),
            family: None,
            downloads: 0,
            likes: 0,
            last_modified: None,
            license: None,
            open_license: false,
            uncensored: CURATION.uncensored_image(id),
            gated: problem == HubProblem::Restricted,
        },
        revision: String::new(),
        files: Vec::new(),
        components: Vec::new(),
        unsupported: None,
        problem: Some(problem),
        stale: false,
    }
}

/// Efface chaque fichier (et son `.part`), puis son dossier
/// `blobs/<sha256>/` s'il est vide (le même contenu peut y être rangé sous un
/// autre nom), sauf ceux qu'un téléchargement a pris entre-temps (`in_use`).
async fn remove_files(paths: &[PathBuf], in_use: &HashSet<PathBuf>) -> AppResult<()> {
    for path in paths {
        if in_use.contains(path) {
            continue;
        }
        for target in [path.clone(), part_path(path)] {
            match tokio::fs::remove_file(&target).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(AppError::Io(error)),
            }
        }
        if let Some(dir) = path.parent() {
            let _ = tokio::fs::remove_dir(dir).await;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
