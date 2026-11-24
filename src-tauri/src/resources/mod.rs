//! Arbitre des ressources de calcul (propre à ChatonLM, choix du 20 nov.
//! 2026 ; recherche `.tools/research/108-ressources-research.md`) : la carte
//! graphique est partagée entre le texte (llama-server), les images
//! (sd-server) et Whisper (whisper-server).
//!
//! - Ce qui calcule garde sa place ; ce qui arrive prend la place libre, au
//!   besoin en faisant rendre la VRAM à ce qui est chargé sans servir, dans
//!   les deux sens :
//!   - un modèle de texte qui ne tient pas : moteur d'images et Whisper
//!     inactifs arrêtés ([`crate::engine::Engine::release_others`]), puis
//!     les modèles de texte inactifs (le moteur de texte) ;
//!   - une image : modèles de texte inactifs (crochet existant) et Whisper
//!     inactif ;
//!   - Whisper : moteur d'images inactif ; les modèles de texte inactifs
//!     seulement pour une transcription longue (une dictée prépare souvent
//!     le prochain message : recharger le modèle de la conversation coûterait
//!     plus que Whisper sur le processeur). Pas la place : processeur.
//! - Une VRAM rendue (images ou Whisper arrêtés) permet aux modèles de texte
//!   repliés faute de mémoire de réessayer la carte
//!   ([`crate::engine::Engine::gpu_freed`]).
//! - Vue des ressources (Réglages › Modèles, près de la jauge de la VRAM) :
//!   [`Resources::view`], qui dit où sont Whisper et le moteur d'images.
//! - Prêt de la carte à un travail de fond (Atelier) : [`Resources::lend_gpu`]
//!   ([`lend`]) ; priorité de l'utilisateur sur ce travail : [`interactive`].

pub mod interactive;
pub mod lend;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use tokio::process::Command;
use ts_rs::TS;

use crate::dictation::whisper_server::Budget;
use crate::hub::hardware::{Machine, nvidia_smi};
use crate::os_util::NoWindow;

pub use interactive::Interactive;

/// Délai de la lecture de la VRAM par processus (`nvidia-smi`).
const NVIDIA_TIMEOUT: Duration = Duration::from_secs(3);

/// Où calcule une partie de l'app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Device {
    /// Carte graphique.
    Card,
    /// Processeur.
    Processor,
}

/// Whisper chargé (dictée, mode vocal, transcription).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeechUse {
    /// Identifiant du catalogue de la dictée (`whisper-large-v3-turbo`…).
    pub model_id: String,
    pub device: Device,
    /// VRAM prise (mesurée par processus quand le pilote le dit, sinon
    /// demandée) ; `None` sur le processeur.
    #[ts(type = "number | null")]
    pub vram_bytes: Option<u64>,
}

/// Moteur d'images lancé.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImagesUse {
    /// En train de peindre (sinon : gardé en mémoire pour la suivante).
    pub busy: bool,
    /// VRAM prise, si le pilote la dit par processus (NVIDIA).
    #[ts(type = "number | null")]
    pub vram_bytes: Option<u64>,
}

/// Ce qui occupe la machine en dehors des modèles de texte (eux sont dans
/// la liste du moteur) : vue des ressources.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResourcesView {
    /// Modèle de transcription chargé (Whisper ou Parakeet) ; `None` : aucun.
    pub speech: Option<SpeechUse>,
    /// Moteur d'images ; `None` : arrêté.
    pub images: Option<ImagesUse>,
}

/// Whisper sur la carte.
#[derive(Clone, Copy)]
struct OnCard {
    pid: u32,
    bytes: u64,
}

/// Arbitre (état Tauri, clonable).
#[derive(Clone, Default)]
pub struct Resources {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    text: OnceLock<crate::engine::Engine>,
    images: OnceLock<crate::image_gen::ImageGen>,
    speech: OnceLock<crate::dictation::engine::Engine>,
    whisper: Mutex<Option<OnCard>>,
}

impl Resources {
    pub fn new() -> Self {
        Self::default()
    }

    /// Branche les trois moteurs (une seule fois, au démarrage de l'app) et
    /// pose leurs crochets.
    pub fn attach(
        &self,
        text: crate::engine::Engine,
        images: crate::image_gen::ImageGen,
        speech: crate::dictation::engine::Engine,
    ) {
        let this = self.clone();
        text.release_others(Arc::new(move || {
            let this = this.clone();
            Box::pin(async move { this.release_for_text().await })
        }));
        let freed = text.clone();
        images.on_release(Arc::new(move || freed.gpu_freed(None)));
        // Un modèle de texte qui répond sur le processeur : Whisper y prend
        // moitié moins de fils.
        let answering = text.clone();
        crate::dictation::engine::share_processor_with(move || {
            answering
                .running()
                .iter()
                .any(|model| model.in_use && model.cpu_only)
        });
        let _ = self.inner.text.set(text);
        let _ = self.inner.images.set(images);
        let _ = self.inner.speech.set(speech);
    }

    /// Avant un modèle de texte qui ne tient pas : moteur d'images et
    /// Whisper inactifs arrêtés (voir l'en-tête).
    async fn release_for_text(&self) {
        if let Some(images) = self.inner.images.get()
            && images.release_idle().await
        {
            // i18n-ignore : journal.
            crate::app_log!("chatonlm: image engine stopped to make room for a text model");
        }
        self.release_speech().await;
    }

    /// Whisper inactif sur la carte arrêté (avant une image ou un modèle de
    /// texte).
    pub async fn release_speech(&self) {
        let Some(speech) = self.inner.speech.get().cloned() else {
            return;
        };
        if self.inner.whisper.lock().is_none() {
            return;
        }
        let released = tokio::task::spawn_blocking(move || speech.release_card_if_idle())
            .await
            .unwrap_or(false);
        if released {
            // i18n-ignore : journal.
            crate::app_log!("chatonlm: Whisper stopped on the graphics card to make room");
        }
    }

    /// Ce qui occupe la machine hors modèles de texte (vue des ressources).
    pub async fn view(&self) -> ResourcesView {
        let whisper = *self.inner.whisper.lock();
        let image_pid = self
            .inner
            .images
            .get()
            .and_then(crate::image_gen::ImageGen::server_pid);
        let pids: Vec<u32> = whisper
            .map(|on| on.pid)
            .into_iter()
            .chain(image_pid)
            .collect();
        let used = if pids.is_empty() {
            Vec::new()
        } else {
            vram_by_process().await
        };
        let measured = |pid: u32| {
            used.iter()
                .find(|(p, _)| *p == pid)
                .map(|(_, bytes)| *bytes)
        };
        let speech = self
            .inner
            .speech
            .get()
            .and_then(crate::dictation::engine::Engine::loaded_model)
            .map(|model_id| match whisper {
                Some(on) => SpeechUse {
                    model_id,
                    device: Device::Card,
                    vram_bytes: Some(measured(on.pid).unwrap_or(on.bytes)),
                },
                None => SpeechUse {
                    model_id,
                    device: Device::Processor,
                    vram_bytes: None,
                },
            });
        let images = self.inner.images.get().and_then(|images| {
            let pid = images.server_pid()?;
            Some(ImagesUse {
                busy: images.busy(),
                vram_bytes: measured(pid),
            })
        });
        ResourcesView { speech, images }
    }

    /// Mémoire de la machine, lue à neuf.
    async fn machine_now(&self) -> Option<Machine> {
        self.inner.text.get()?.machine_now().await
    }

    /// Voir [`Budget::make_room`] (asynchrone).
    async fn room_for_speech(&self, bytes: u64, long_job: bool) -> bool {
        let Some(machine) = self.machine_now().await else {
            return false;
        };
        if machine.gpus.is_empty() {
            return false;
        }
        match free_vram(&machine) {
            // VRAM libre inconnue (AMD sous Windows, Intel) : essai ; le
            // serveur dit lui-même s'il a chargé sur la carte, et le
            // processeur prend le relais sinon.
            None => return true,
            Some(free) if free >= bytes => return true,
            Some(_) => {}
        }
        if let Some(images) = self.inner.images.get()
            && images.release_idle().await
        {
            // i18n-ignore : journal.
            crate::app_log!("chatonlm: image engine stopped to make room for Whisper");
            if self.fits(bytes).await {
                return true;
            }
        }
        if long_job && let Some(text) = self.inner.text.get() {
            if text.release_idle().await > 0 {
                // i18n-ignore : journal.
                crate::app_log!("chatonlm: idle text models stopped to make room for Whisper");
            }
            return self.fits(bytes).await;
        }
        false
    }

    async fn fits(&self, bytes: u64) -> bool {
        self.machine_now()
            .await
            .as_ref()
            .and_then(free_vram)
            .is_some_and(|free| free >= bytes)
    }
}

impl Budget for Resources {
    fn make_room(&self, bytes: u64, long_job: bool) -> bool {
        tauri::async_runtime::block_on(self.room_for_speech(bytes, long_job))
    }

    fn taken(&self, pid: u32, bytes: u64) {
        *self.inner.whisper.lock() = Some(OnCard { pid, bytes });
    }

    fn released(&self) {
        let was = self.inner.whisper.lock().take();
        if let Some(text) = self.inner.text.get() {
            text.gpu_freed(was.map(|on| on.bytes));
        }
    }
}

/// VRAM libre de toutes les cartes ; `None` si l'une ne la dit pas.
fn free_vram(machine: &Machine) -> Option<u64> {
    machine.gpus.iter().map(|gpu| gpu.vram_free).sum()
}

/// VRAM prise par processus, d'après `nvidia-smi` (NVIDIA seulement ;
/// vide sinon) : (pid, octets).
async fn vram_by_process() -> Vec<(u32, u64)> {
    let mut command = Command::new(nvidia_smi());
    command
        .no_window()
        .args([
            "--query-compute-apps=pid,used_memory",
            "--format=csv,noheader,nounits",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(NVIDIA_TIMEOUT, command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            parse_compute_apps(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
    }
}

/// Lignes « pid, Mio » de `nvidia-smi --query-compute-apps`.
fn parse_compute_apps(text: &str) -> Vec<(u32, u64)> {
    text.lines()
        .filter_map(|line| {
            let (pid, mib) = line.split_once(',')?;
            let pid = pid.trim().parse().ok()?;
            let mib: u64 = mib.trim().parse().ok()?;
            Some((pid, mib.saturating_mul(1024 * 1024)))
        })
        .collect()
}

/// Journal du serveur Whisper, à côté de ceux des autres moteurs.
pub fn whisper_log(logs_dir: &std::path::Path) -> PathBuf {
    logs_dir.join("whisper-server.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vram_per_process_is_read_from_nvidia_smi() {
        assert_eq!(
            parse_compute_apps("160496, 844\n 2001 , 12\nbad line\n77, [N/A]\n"),
            [(160_496, 844 * 1024 * 1024), (2001, 12 * 1024 * 1024)]
        );
        assert!(parse_compute_apps("").is_empty());
    }
}
