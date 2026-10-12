//! Correction d'un texte (page Proofread) et raccourcis globaux (onglet
//! Keyboard shortcuts des réglages), propres à ChatonLM.

use tauri::ipc::Channel;
use tauri::{AppHandle, State};

use crate::desktop::{self, ShortcutKeys, ShortcutsStatus};
use crate::error::AppResult;
use crate::proofread::{self, ProofreadEvent, ProofreadMode, Proofreader, SelectionResult};
use crate::store::Store;

/// Corrige ou reformule `text` ; évènements sur `on_event` jusqu'à `Done` ou
/// `Error`. `job_id` (choisi par la page) sert à l'arrêter.
#[tauri::command]
pub async fn proofread_run(
    store: State<'_, Store>,
    proofreader: State<'_, Proofreader>,
    job_id: String,
    text: String,
    mode: ProofreadMode,
    on_event: Channel<ProofreadEvent>,
) -> AppResult<()> {
    proofread::run_job(&store, &proofreader, job_id, text, mode, &on_event).await
}

/// Arrête une correction en cours ; sans effet si elle est déjà finie.
#[tauri::command]
pub async fn proofread_cancel(
    proofreader: State<'_, Proofreader>,
    job_id: String,
) -> AppResult<()> {
    proofreader.cancel(&job_id);
    Ok(())
}

/// Arrête toutes les corrections de la page (appelée à son chargement : celles
/// d'une page rechargée ne seraient plus lues).
#[tauri::command]
pub async fn proofread_cancel_all(proofreader: State<'_, Proofreader>) -> AppResult<()> {
    proofreader.cancel_all();
    Ok(())
}

/// Dernière correction du raccourci global (action « Show » de sa
/// notification).
#[tauri::command]
pub async fn proofread_last_selection(
    proofreader: State<'_, Proofreader>,
) -> AppResult<Option<SelectionResult>> {
    Ok(proofreader.last_selection())
}

#[tauri::command]
pub async fn shortcuts_status(app: AppHandle) -> AppResult<ShortcutsStatus> {
    Ok(desktop::shortcuts_status(&app))
}

/// Nouvel essai de liaison des raccourcis demandés (après un refus ou une
/// erreur) ; erreur si aucun n'est demandé.
#[tauri::command]
pub async fn shortcuts_bind(app: AppHandle) -> AppResult<()> {
    desktop::bind_shortcuts(&app).await
}

/// Ouvre la boîte du bureau qui change les touches (portail version 2).
#[tauri::command]
pub async fn shortcuts_configure(app: AppHandle) -> AppResult<()> {
    desktop::configure_shortcut(&app).await
}

/// Combinaison saisie dans la page pour le raccourci `id` (« dictation » ou
/// « proofread ») : contrôlée, gardée dans les réglages, enregistrée ; statut
/// des raccourcis ensuite. Windows seulement (`chooser` = « app ») : sous
/// Linux, le bureau choisit les touches et la commande échoue.
#[tauri::command]
pub async fn shortcuts_set_trigger(
    app: AppHandle,
    id: String,
    keys: ShortcutKeys,
) -> AppResult<ShortcutsStatus> {
    desktop::set_trigger(&app, &id, keys).await?;
    Ok(desktop::shortcuts_status(&app))
}

/// Combinaison choisie oubliée : retour à celle par défaut (Windows
/// seulement).
#[tauri::command]
pub async fn shortcuts_reset_trigger(app: AppHandle, id: String) -> AppResult<ShortcutsStatus> {
    desktop::reset_trigger(&app, &id).await?;
    Ok(desktop::shortcuts_status(&app))
}

/// Zone de saisie d'une combinaison ouverte (`active`) ou fermée : les
/// raccourcis globaux sont suspendus pendant la saisie (Windows ; sans effet
/// sous Linux).
#[tauri::command]
pub async fn shortcuts_recording(app: AppHandle, active: bool) -> AppResult<()> {
    desktop::recording(&app, active).await;
    Ok(())
}
