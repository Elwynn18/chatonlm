//! Écran du moteur de texte (lot L5b) : état de `llama-server` livré
//! (version, accélération, repli sur le processeur) et journal des modèles.

use serde::Serialize;
use tauri::{AppHandle, State};
use ts_rs::TS;

use crate::engine::Engine;
use crate::error::AppResult;
use crate::hub::hardware::Machine;

/// Accélération du moteur.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum EngineBackend {
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
/// (aucune, ou machine illisible) ou après un repli.
pub fn status_of(
    available: bool,
    version: Option<String>,
    machine: Option<&Machine>,
    cpu_fallback: bool,
    logs_dir: String,
) -> EngineStatus {
    let gpu_name = machine
        .and_then(|machine| machine.gpus.first())
        .map(|gpu| gpu.name.clone());
    let backend = if cpu_fallback || gpu_name.is_none() {
        EngineBackend::Cpu
    } else {
        EngineBackend::Vulkan
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

#[tauri::command]
pub async fn engine_status(engine: State<'_, Engine>) -> AppResult<EngineStatus> {
    let version = engine
        .binary()
        .as_deref()
        .and_then(crate::engine::binary::version);
    let machine = engine.machine().await;
    Ok(status_of(
        engine.available(),
        version,
        machine.as_ref(),
        engine.gpu_failed(),
        engine.logs_dir().to_string_lossy().into_owned(),
    ))
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
        let status = status_of(true, Some("b12110".into()), Some(&card), false, "/l".into());
        assert_eq!(status.backend, EngineBackend::Vulkan);
        assert_eq!(status.gpu_name.as_deref(), Some("RTX 3090"));
        // Repli de la session : processeur, carte toujours nommée.
        let fallen = status_of(true, None, Some(&card), true, "/l".into());
        assert_eq!(fallen.backend, EngineBackend::Cpu);
        assert!(fallen.cpu_fallback);
        // Aucune carte, ou machine illisible : processeur.
        assert_eq!(
            status_of(true, None, Some(&machine(&[])), false, "/l".into()).backend,
            EngineBackend::Cpu
        );
        assert_eq!(
            status_of(false, None, None, false, "/l".into()).backend,
            EngineBackend::Cpu
        );
    }
}
