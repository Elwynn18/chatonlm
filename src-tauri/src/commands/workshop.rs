//! Atelier (moteur de tâches complexes, propre à ChatonLM) : commandes de la
//! page (lot 3). Chaque changement d'un projet émet `workshop-event`
//! (`WorkshopNotice`, débit borné) ; la page relit alors le projet ou son
//! journal (`workshop_events` depuis le dernier numéro lu).
//!
//! Refus : `Invalid` (message traduit), `NotFound` (projet supprimé). Le
//! travail est fait par `workshop::Workshop` (contrat
//! `.tools/research/111-lot2-contrat.md`).

use tauri::{State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

use crate::attachments::Staging;
use crate::error::AppResult;
use crate::i18n::t;
use crate::workshop::pipeline::types::Event;
use crate::workshop::{
    AnswerRequest, ModelCheck, NewProjectRequest, ProjectDetail, ProjectSummary, Workshop,
    WorkshopSettings, WorkshopSettingsPatch,
};

/// Crée un projet (sous-dossier du dossier des projets, sources gardées
/// dans son `.atelier/`) et le met en file ; rend son identifiant.
#[tauri::command]
pub async fn workshop_create(
    workshop: State<'_, Workshop>,
    staging: State<'_, Staging>,
    project: NewProjectRequest,
) -> AppResult<i64> {
    workshop.create(&staging, project).await
}

/// Projets, du plus récent au plus ancien (rang dans la file compris).
#[tauri::command]
pub async fn workshop_list(workshop: State<'_, Workshop>) -> AppResult<Vec<ProjectSummary>> {
    workshop.list().await
}

/// Projet complet : tâche, pièces jointes, question en attente, dernier
/// numéro d'événement, rapport.
#[tauri::command]
pub async fn workshop_get(workshop: State<'_, Workshop>, id: i64) -> AppResult<ProjectDetail> {
    workshop.get(id).await
}

/// Événements du projet après `since_seq` (0 : depuis le début), au plus
/// 1 000 ; la page relit tant qu'il en vient.
#[tauri::command]
pub async fn workshop_events(
    workshop: State<'_, Workshop>,
    id: i64,
    since_seq: u64,
) -> AppResult<Vec<Event>> {
    workshop.events(id, since_seq).await
}

/// Réponse à la question en attente (« J'ai besoin de toi »).
#[tauri::command]
pub async fn workshop_answer(
    workshop: State<'_, Workshop>,
    id: i64,
    answer: AnswerRequest,
) -> AppResult<()> {
    workshop.answer(id, answer).await
}

/// Pause : l'état est gardé, la reprise ne refait rien.
#[tauri::command]
pub async fn workshop_pause(workshop: State<'_, Workshop>, id: i64) -> AppResult<()> {
    workshop.pause(id).await
}

/// Reprise d'un projet en pause, échoué, ou qui attendait au lancement.
#[tauri::command]
pub async fn workshop_resume(workshop: State<'_, Workshop>, id: i64) -> AppResult<()> {
    workshop.resume(id).await
}

#[tauri::command]
pub async fn workshop_cancel(workshop: State<'_, Workshop>, id: i64) -> AppResult<()> {
    workshop.cancel(id).await
}

/// Supprime le projet ; `delete_files` : son dossier aussi.
#[tauri::command]
pub async fn workshop_delete(
    workshop: State<'_, Workshop>,
    id: i64,
    delete_files: bool,
) -> AppResult<()> {
    workshop.delete(id, delete_files).await
}

/// Ouvre le dossier du projet dans le gestionnaire de fichiers.
#[tauri::command]
pub async fn workshop_open_folder(workshop: State<'_, Workshop>, id: i64) -> AppResult<()> {
    let folder = workshop.folder(id).await?;
    crate::commands::system::open_path(&folder).await
}

/// Sélecteur de dossier natif « dossier des projets », au-dessus de la
/// fenêtre ; `None` si l'utilisateur annule. Le dossier rendu est le seul
/// que `workshop_set_settings` accepte ensuite.
#[tauri::command]
pub async fn workshop_pick_dir(
    window: WebviewWindow,
    workshop: State<'_, Workshop>,
) -> AppResult<Option<String>> {
    // Essais de l'app réelle : le dossier « choisi » est lu dans le fichier
    // désigné (première ligne ; vide : annulé), relu à chaque appel.
    #[cfg(feature = "e2e")]
    if let Some(file) = std::env::var_os("CHATONLM_E2E_PICK_FOLDER") {
        let picked = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| text.lines().next().map(str::to_owned))
            .filter(|line| !line.is_empty());
        if let Some(dir) = &picked {
            workshop.grant_dir(dir);
        }
        return Ok(picked);
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(t!("dialogs.selectWorkshopDirectory"))
        .pick_folder(move |path| {
            let _ = sender.send(path);
        });
    // Rappel jamais fait (boucle d'événements arrêtée) : comme une annulation.
    let picked = receiver
        .await
        .ok()
        .flatten()
        .and_then(|path| path.into_path().ok())
        .map(|path| path.to_string_lossy().into_owned());
    if let Some(dir) = &picked {
        workshop.grant_dir(dir);
    }
    Ok(picked)
}

#[tauri::command]
pub async fn workshop_settings(workshop: State<'_, Workshop>) -> AppResult<WorkshopSettings> {
    workshop.settings().await
}

#[tauri::command]
pub async fn workshop_set_settings(
    workshop: State<'_, Workshop>,
    patch: WorkshopSettingsPatch,
) -> AppResult<WorkshopSettings> {
    workshop.set_settings(patch).await
}

/// Modèle qu'un nouveau projet prendrait (`model` absent ou vide : choisi
/// par l'Atelier) et ses avertissements.
#[tauri::command]
pub async fn workshop_model_check(
    workshop: State<'_, Workshop>,
    model: Option<String>,
) -> AppResult<ModelCheck> {
    Ok(workshop.model_check(model).await)
}
