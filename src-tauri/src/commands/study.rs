//! Cours générés (onglet Study), propres à ChatonLM. Chaque changement d'un
//! cours émet `study-updated` (`StudyUpdated`) ; la page relit alors le cours.
//!
//! Le travail est fait par `study::StudyEngine` (contrat
//! `.tools/research/47-study-contract.md`).

use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

use crate::attachments::Staging;
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::Store;
use crate::study::{
    AnswerValue, Answered, Course, CourseSummary, Ctx, NewCourse, StudyEngine, StudyStreamEvent,
    UPDATED_EVENT,
};

/// Base et évènements `study-updated` vers la page.
fn ctx(app: &AppHandle, store: &Store) -> Ctx {
    let app = app.clone();
    Ctx::new(store.clone(), move |updated| {
        let _ = app.emit(UPDATED_EVENT, updated);
    })
}

/// Cours, du plus récemment actif au plus ancien.
#[tauri::command]
pub async fn study_list(
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
) -> AppResult<Vec<CourseSummary>> {
    engine.list(&store).await
}

/// Cours complet ; relance la génération de ce qui manque (paliers en
/// attente d'un cours commencé, partie interrompue par une fermeture).
/// `NotFound` s'il n'existe pas.
#[tauri::command]
pub async fn study_get(
    app: AppHandle,
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    course_id: String,
) -> AppResult<Course> {
    engine.get(&ctx(&app, &store), &course_id).await
}

/// Crée un cours et lance la préparation du plan ; renvoie son identifiant.
/// Refus (`Invalid`, message traduit) : sujet vide sans document, sujet trop
/// long, durée ou nombre de documents hors limites, document inconnu ou
/// image ; aucun modèle (`Invalid`, message de `noModel`).
#[tauri::command]
pub async fn study_create(
    app: AppHandle,
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    staging: State<'_, Staging>,
    course: NewCourse,
) -> AppResult<String> {
    engine.create(&ctx(&app, &store), &staging, course).await
}

/// Réponse à la question d'un sujet ambigu : `choice` (une des options, ou
/// un texte de l'apprenant), ou `None` pour garder le sujet tel quel ; le
/// plan est refait.
#[tauri::command]
pub async fn study_clarify(
    app: AppHandle,
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    course_id: String,
    choice: Option<String>,
) -> AppResult<()> {
    engine.clarify(&ctx(&app, &store), &course_id, choice).await
}

/// Nouveau plan (cours pas encore commencé) : paliers générés écartés.
#[tauri::command]
pub async fn study_replan(
    app: AppHandle,
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    course_id: String,
) -> AppResult<()> {
    engine.replan(&ctx(&app, &store), &course_id).await
}

/// Commence le cours (plan prêt) : génération des paliers suivants et du
/// bilan.
#[tauri::command]
pub async fn study_start(
    app: AppHandle,
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    course_id: String,
) -> AppResult<()> {
    engine.start(&ctx(&app, &store), &course_id).await
}

/// Corrige et enregistre la réponse à une question ; une question déjà
/// répondue renvoie sa première réponse, inchangée.
#[tauri::command]
pub async fn study_answer(
    app: AppHandle,
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    course_id: String,
    question_id: String,
    answer: AnswerValue,
) -> AppResult<Answered> {
    engine
        .answer(&ctx(&app, &store), &course_id, &question_id, answer)
        .await
}

/// Enregistre l'étape et le temps passé (appelée par la page, sans évènement
/// `study-updated`).
#[tauri::command]
pub async fn study_progress(
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    course_id: String,
    position: String,
    elapsed_ms: i64,
) -> AppResult<()> {
    engine
        .progress(&store, &course_id, position, elapsed_ms)
        .await
}

/// « Explain differently » après un essai manqué : nouvelle explication et
/// nouvelles questions, générées en priorité.
#[tauri::command]
pub async fn study_remedy(
    app: AppHandle,
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    course_id: String,
    level: u32,
) -> AppResult<()> {
    engine.remedy(&ctx(&app, &store), &course_id, level).await
}

/// « Continue anyway » (deux essais manqués au moins) : palier « à revoir ».
#[tauri::command]
pub async fn study_skip(
    app: AppHandle,
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    course_id: String,
    level: u32,
) -> AppResult<()> {
    engine.skip(&ctx(&app, &store), &course_id, level).await
}

/// Refait un palier (« Report a mistake ») : leçon, questions, essais,
/// explications et questions posées remplacés, réponses du palier effacées.
#[tauri::command]
pub async fn study_regenerate(
    app: AppHandle,
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    course_id: String,
    level: u32,
) -> AppResult<()> {
    engine
        .regenerate(&ctx(&app, &store), &course_id, level)
        .await
}

/// Relance les parties en échec (plan, paliers, nouvel essai, bilan) ;
/// `without_web` : sans la recherche web (après `signedOut`).
#[tauri::command]
pub async fn study_retry(
    app: AppHandle,
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    course_id: String,
    without_web: bool,
) -> AppResult<()> {
    engine
        .retry(&ctx(&app, &store), &course_id, without_web)
        .await
}

/// « I don't understand » sur la section `section` du palier `level` :
/// explication plus simple en flux, enregistrée à la fin.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn study_explain(
    app: AppHandle,
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    course_id: String,
    level: u32,
    section: u32,
    stream_id: String,
    on_event: Channel<StudyStreamEvent>,
) -> AppResult<()> {
    engine
        .explain(
            &ctx(&app, &store),
            &course_id,
            level,
            section,
            stream_id,
            &on_event,
        )
        .await
}

/// « Ask a question » sur le palier `level` : réponse en flux, enregistrée
/// à la fin.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn study_ask(
    app: AppHandle,
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    course_id: String,
    level: u32,
    question: String,
    stream_id: String,
    on_event: Channel<StudyStreamEvent>,
) -> AppResult<()> {
    engine
        .ask(
            &ctx(&app, &store),
            &course_id,
            level,
            question,
            stream_id,
            &on_event,
        )
        .await
}

/// Arrête une réponse en flux ; sans effet si elle est finie.
#[tauri::command]
pub async fn study_stream_cancel(
    engine: State<'_, StudyEngine>,
    stream_id: String,
) -> AppResult<()> {
    engine.cancel_stream(&stream_id);
    Ok(())
}

/// Fiche du cours en Markdown (titre, objectifs, à retenir et piège de
/// chaque palier, sources), dans la langue du cours.
#[tauri::command]
pub async fn study_notes(
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    course_id: String,
) -> AppResult<String> {
    engine.notes(&store, &course_id).await
}

/// Enregistre la fiche dans un fichier `.md` choisi dans la boîte native ;
/// `false` si l'apprenant annule.
#[tauri::command]
pub async fn study_save_notes(
    window: WebviewWindow,
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    course_id: String,
) -> AppResult<bool> {
    let (text, name) = engine.notes_and_name(&store, &course_id).await?;
    let path = pick_save_path(&window, name).await;
    let Some(path) = path else {
        return Ok(false);
    };
    tauri::async_runtime::spawn_blocking(move || std::fs::write(&path, text))
        .await
        .map_err(|error| AppError::Internal(error.to_string()))??;
    Ok(true)
}

/// Chemin choisi dans la boîte « enregistrer » native, au-dessus de la
/// fenêtre ; `None` si l'apprenant annule.
async fn pick_save_path(window: &WebviewWindow, name: String) -> Option<std::path::PathBuf> {
    // Essais de l'app réelle : pas de boîte à piloter, le chemin « choisi »
    // est la valeur de la variable (vide : annulé).
    #[cfg(feature = "e2e")]
    if let Some(path) = std::env::var_os("CHATONLM_E2E_SAVE_FILE") {
        return (!path.is_empty()).then(|| std::path::PathBuf::from(path));
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_parent(window)
        .set_title(t!("study.saveNotes"))
        .set_file_name(name)
        .add_filter(t!("study.markdownFiles"), &["md"])
        .save_file(move |path| {
            let _ = sender.send(path);
        });
    // Rappel jamais fait (boucle d'événements arrêtée) : comme une annulation.
    receiver
        .await
        .ok()
        .flatten()
        .and_then(|path| path.into_path().ok())
}

/// Supprime un cours (génération arrêtée).
#[tauri::command]
pub async fn study_delete(
    app: AppHandle,
    store: State<'_, Store>,
    engine: State<'_, StudyEngine>,
    course_id: String,
) -> AppResult<()> {
    engine.delete(&ctx(&app, &store), &course_id).await
}
