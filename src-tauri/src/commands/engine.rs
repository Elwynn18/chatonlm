//! Écran du moteur de texte (lot L5b) : état de `llama-server` livré
//! (version, accélération, repli sur le processeur) et journal des modèles ;
//! pack CUDA facultatif (`engine::cuda`) : état, téléchargement après la
//! licence de NVIDIA, annulation, réglage, retrait.

use serde::Serialize;
use tauri::{AppHandle, State};
use ts_rs::TS;

use crate::engine::Engine;
use crate::engine::cuda::{Cuda, CudaStatus};
use crate::error::AppResult;
use crate::hub::hardware::Machine;

/// Accélération du moteur.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum EngineBackend {
    /// Carte NVIDIA par le pack CUDA.
    Cuda,
    /// Carte graphique (Vulkan).
    Vulkan,
    /// Processeur : aucune carte vue, ou repli de la session.
    Cpu,
}

/// État du moteur (écran du moteur).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EngineStatus {
    /// Le binaire `llama-server` est livré.
    pub available: bool,
    /// Version construite (`VERSION` à côté du binaire).
    pub version: Option<String>,
    pub backend: EngineBackend,
    /// Première carte vue.
    pub gpu_name: Option<String>,
    /// La carte a échoué pendant cette session : modèles sur le processeur.
    pub cpu_fallback: bool,
    /// Dossier des journaux (`llama-*.log`).
    pub logs_dir: String,
}

/// État d'après les faits lus (règle pure) : processeur sans carte vue
/// (aucune, ou machine illisible) ou après un repli ; CUDA quand le pack
/// sert les prochains chargements (`cuda` : sa carte).
pub fn status_of(
    available: bool,
    version: Option<String>,
    machine: Option<&Machine>,
    cpu_fallback: bool,
    logs_dir: String,
    cuda: Option<String>,
) -> EngineStatus {
    let gpu_name = machine
        .and_then(|machine| machine.gpus.first())
        .map(|gpu| gpu.name.clone());
    let (backend, gpu_name) = match cuda {
        _ if cpu_fallback => (EngineBackend::Cpu, gpu_name),
        Some(card) => (EngineBackend::Cuda, Some(card)),
        None if gpu_name.is_none() => (EngineBackend::Cpu, gpu_name),
        None => (EngineBackend::Vulkan, gpu_name),
    };
    EngineStatus {
        available,
        version,
        backend,
        gpu_name,
        cpu_fallback,
        logs_dir,
    }
}

/// État du moteur lu maintenant (écran du moteur, diagnostic d'un
/// signalement).
pub(crate) async fn current(engine: &Engine) -> EngineStatus {
    let version = engine
        .binary()
        .as_deref()
        .and_then(crate::engine::binary::version);
    let machine = engine.machine().await;
    let cuda = match engine.cuda().select().await {
        Some(_) => engine
            .cuda()
            .assessment(false)
            .await
            .shown
            .map(|gpu| gpu.name),
        None => None,
    };
    status_of(
        engine.available(),
        version,
        machine.as_ref(),
        engine.gpu_failed(),
        engine.logs_dir().to_string_lossy().into_owned(),
        cuda,
    )
}

#[tauri::command]
pub async fn engine_status(engine: State<'_, Engine>) -> AppResult<EngineStatus> {
    Ok(current(&engine).await)
}

/// État du pack CUDA ; `fresh` : cartes NVIDIA détectées de nouveau.
#[tauri::command]
pub async fn cuda_status(cuda: State<'_, Cuda>, fresh: bool) -> AppResult<CudaStatus> {
    Ok(cuda.status(fresh).await)
}

/// Télécharge et installe le pack ; refusé sans la licence de NVIDIA
/// acceptée (`accept_license`), ou sur une machine incompatible.
#[tauri::command]
pub async fn cuda_install(cuda: State<'_, Cuda>, accept_license: bool) -> AppResult<()> {
    cuda.install(accept_license).await
}

/// Arrête le téléchargement ou l'installation du pack.
#[tauri::command]
pub async fn cuda_cancel(cuda: State<'_, Cuda>) -> AppResult<()> {
    cuda.cancel();
    Ok(())
}

/// Réglage « Accélération NVIDIA » (prochains chargements).
#[tauri::command]
pub async fn cuda_set_enabled(cuda: State<'_, Cuda>, enabled: bool) -> AppResult<()> {
    cuda.set_enabled(enabled)
}

/// Retire le pack ; les modèles chargés sont d'abord déchargés (ses
/// bibliothèques peuvent être ouvertes par leurs processus).
#[tauri::command]
pub async fn cuda_remove(engine: State<'_, Engine>, cuda: State<'_, Cuda>) -> AppResult<()> {
    engine.unload_all().await;
    cuda.remove().await
}

/// Ouvre le dossier des journaux du moteur (`<données>/logs`).
#[tauri::command]
pub async fn open_engine_logs(app: AppHandle) -> AppResult<()> {
    let dir = crate::paths::data_dir(&app)?.join("logs");
    std::fs::create_dir_all(&dir)?;
    super::system::open_path(&dir).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::HubGpu;

    fn machine(gpus: &[&str]) -> Machine {
        Machine {
            gpus: gpus
                .iter()
                .map(|name| HubGpu {
                    name: (*name).into(),
                    vram_total: 8 << 30,
                    vram_free: None,
                })
                .collect(),
            ram_total: 16 << 30,
            ram_available: 8 << 30,
        }
    }

    #[test]
    fn the_backend_follows_the_card_and_the_fallback() {
        let card = machine(&["RTX 3090", "iGPU"]);
        let status = status_of(
            true,
            Some("b12110".into()),
            Some(&card),
            false,
            "/l".into(),
            None,
        );
        assert_eq!(status.backend, EngineBackend::Vulkan);
        assert_eq!(status.gpu_name.as_deref(), Some("RTX 3090"));
        // Repli de la session : processeur, carte toujours nommée.
        let fallen = status_of(true, None, Some(&card), true, "/l".into(), None);
        assert_eq!(fallen.backend, EngineBackend::Cpu);
        assert!(fallen.cpu_fallback);
        // Aucune carte, ou machine illisible : processeur.
        assert_eq!(
            status_of(true, None, Some(&machine(&[])), false, "/l".into(), None).backend,
            EngineBackend::Cpu
        );
        assert_eq!(
            status_of(false, None, None, false, "/l".into(), None).backend,
            EngineBackend::Cpu
        );
        // Pack CUDA en service : sa carte NVIDIA (même si une autre est
        // vue en premier) ; un repli sur le processeur l'emporte.
        let cuda = status_of(
            true,
            None,
            Some(&machine(&["iGPU", "RTX 3090"])),
            false,
            "/l".into(),
            Some("NVIDIA GeForce RTX 3090".into()),
        );
        assert_eq!(cuda.backend, EngineBackend::Cuda);
        assert_eq!(cuda.gpu_name.as_deref(), Some("NVIDIA GeForce RTX 3090"));
        assert_eq!(
            status_of(true, None, Some(&card), true, "/l".into(), Some("x".into())).backend,
            EngineBackend::Cpu
        );
    }
}
