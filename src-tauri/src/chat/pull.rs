// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Translated to Rust from Ollama (https://github.com/ollama/ollama),
// file app/ui/ui.go, commit 42e911bc3d05 (model download of the chat
// handler).
// Modified for ChatonLM: the model is downloaded from Hugging Face by the
// app's own download queue (the ModelHub's), not by a server pull; progress is
// the sum of all its files (projector included) over their total; errors
// carry a ChatonLM error kind; a model imported from Ollama is never
// downloaded.

//! Téléchargement d'un modèle de Hugging Face absent de la base, avant
//! l'envoi, comme l'app officielle qui tirait le modèle au premier envoi
//! (`app/ui/ui.go:833-887`), par la file du ModelHub
//! ([`crate::hub::downloads::HubDownloads`], la même que l'onglet Downloads :
//! il y paraît, avec ses vérifications, sa place disque et ses reprises),
//! suivi jusqu'à son installation par des événements `Download` (dès que
//! sa taille est connue), puis un dernier événement complet. Écarts voulus :
//! - progression : octets reçus de tous les fichiers (projecteur de vision
//!   compris) sur leur total (l'officiel suivait la plus grosse couche) ;
//! - arrêt de l'envoi : le téléchargement est annulé s'il a été demandé par
//!   cet envoi (comme le pull de l'officiel, interrompu avec la requête),
//!   laissé à la file s'il y était déjà ;
//! - mis en pause ou annulé depuis le ModelHub : l'envoi s'arrête.

use std::time::Duration;

use tokio::sync::watch;

use super::{ChatErrorKind, ChatEvent, Outcome, cancelled};
use crate::hub::types::{HubDownload, HubDownloadRequest, HubDownloadState};
use crate::llm::Access;
use crate::local_models::id::{ModelId, same_model};

/// Intervalle de lecture de la file (elle émet sa progression au plus toutes
/// les 250 ms).
const POLL: Duration = Duration::from_millis(100);

/// Identifiant d'une version de Hugging Face (`auteur/dépôt:QUANT`, ou la
/// forme `hf.co/…` d'Ollama) : seul téléchargeable. Un modèle repris
/// d'Ollama (`@ollama/…`) ou distant ne l'est pas.
pub(super) fn downloadable(model: &str) -> Option<String> {
    ModelId::from_request(model)
        .filter(|id| id.repo_id().is_some())
        .map(|id| id.to_string())
}

/// En file, en cours ou en pause.
fn pending(state: HubDownloadState) -> bool {
    matches!(
        state,
        HubDownloadState::Queued
            | HubDownloadState::Starting
            | HubDownloadState::Downloading
            | HubDownloadState::Finishing
            | HubDownloadState::Paused
    )
}

fn entry(access: &Access, model: &str) -> Option<HubDownload> {
    access
        .downloads()
        .list()
        .into_iter()
        .find(|download| same_model(&download.model, model))
}

/// Télécharge `model` (identifiant de [`downloadable`]) et suit la file
/// jusqu'à son installation. `Done` : installé ; échec : `download` avec le
/// texte de la file ; arrêt (de l'envoi, ou depuis le ModelHub) : `Cancelled`.
pub(super) async fn download(
    access: &Access,
    model: &str,
    cancel: &mut watch::Receiver<bool>,
    emit: &(dyn Fn(ChatEvent) + Sync),
) -> Outcome {
    let downloads = access.downloads();
    // Déjà en file ou en cours (ModelHub, autre envoi) : laissé à la file
    // si cet envoi s'arrête.
    let ours = entry(access, model).is_none_or(|download| !pending(download.state));
    if let Err(error) = downloads.download(HubDownloadRequest {
        model: model.to_owned(),
        repo: None,
        label: None,
    }) {
        return Outcome::Failed(ChatErrorKind::Download, error.to_string());
    }
    let mut shown = None;
    loop {
        let Some(download) = entry(access, model) else {
            // Retiré de la liste (ModelHub) : plus rien à attendre.
            return Outcome::Cancelled;
        };
        match download.state {
            HubDownloadState::Done => {
                let total = download.total.max(download.completed);
                emit(ChatEvent::Download {
                    total,
                    completed: total,
                    done: true,
                });
                return Outcome::Done;
            }
            HubDownloadState::Failed => {
                return Outcome::Failed(
                    ChatErrorKind::Download,
                    download.error.unwrap_or_default(),
                );
            }
            HubDownloadState::Cancelled | HubDownloadState::Paused => return Outcome::Cancelled,
            HubDownloadState::Queued
            | HubDownloadState::Starting
            | HubDownloadState::Downloading
            | HubDownloadState::Finishing => {
                // Taille inconnue (en file, lecture des fichiers du dépôt) :
                // rien à montrer encore.
                let progress = (download.total, download.completed);
                if download.total > 0 && shown != Some(progress) {
                    shown = Some(progress);
                    emit(ChatEvent::Download {
                        total: progress.0,
                        completed: progress.1,
                        done: false,
                    });
                }
            }
        }
        tokio::select! {
            biased;
            () = cancelled(cancel) => {
                if ours {
                    downloads.cancel(model);
                }
                return Outcome::Cancelled;
            }
            () = tokio::time::sleep(POLL) => {}
        }
    }
}
