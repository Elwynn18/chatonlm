//! Cartes NVIDIA et leur compatibilité avec le pack CUDA 13 : `nvidia-smi`
//! (livré avec le pilote, sous Linux comme sous Windows ; celui de
//! `hub::hardware`) donne le nom, l'UUID, la capacité de calcul et la version
//! du pilote. Règles pures, essayées sans carte.

use std::time::Duration;

use serde::Serialize;
use tokio::process::Command;
use ts_rs::TS;

use super::pins::Pins;
use crate::os_util::NoWindow;

/// Délai de `nvidia-smi`.
const TIMEOUT: Duration = Duration::from_secs(3);

/// Carte NVIDIA vue par `nvidia-smi`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NvidiaGpu {
    pub name: String,
    /// `GPU-…` : choix des cartes par `CUDA_VISIBLE_DEVICES`.
    pub uuid: String,
    /// Capacité de calcul (8.6 pour une RTX 3090).
    pub compute: (u32, u32),
    /// Version du pilote (« 615.78.08 »).
    pub driver: String,
}

/// Compatibilité de la machine avec le pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "CudaCompat")]
pub enum Compat {
    /// Système sans pack (ni Linux ni Windows x64).
    Unsupported,
    /// Aucune carte NVIDIA (ou pilote absent : pas de `nvidia-smi`).
    NoNvidia,
    /// Pilote plus ancien que le minimum de CUDA 13.
    DriverTooOld,
    /// Carte NVIDIA trop ancienne pour CUDA 13 (avant Turing) : Vulkan.
    ComputeTooLow,
    Compatible,
}

/// Verdict sur les cartes vues.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assessment {
    pub compat: Compat,
    /// Cartes à donner à CUDA (compatibles), dans l'ordre de `nvidia-smi`.
    pub usable: Vec<NvidiaGpu>,
    /// Carte nommée à l'écran : la première utilisable, sinon la première vue.
    pub shown: Option<NvidiaGpu>,
}

/// « 8.6 » → (8, 6).
pub fn parse_compute(text: &str) -> Option<(u32, u32)> {
    let (major, minor) = text.trim().split_once('.')?;
    Some((major.trim().parse().ok()?, minor.trim().parse().ok()?))
}

/// Version majeure d'un pilote (« 615.78.08 » → 615).
pub fn driver_major(text: &str) -> Option<u32> {
    text.trim().split('.').next()?.trim().parse().ok()
}

/// Lignes « nom, uuid, capacité, pilote » de `nvidia-smi` ; une ligne
/// illisible (capacité « [N/A] » d'un très vieux pilote) est ignorée.
pub fn parse(text: &str) -> Vec<NvidiaGpu> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.rsplitn(4, ',').map(str::trim);
            let driver = fields.next()?;
            let compute = parse_compute(fields.next()?)?;
            let uuid = fields.next()?;
            let name = fields.next()?;
            (!name.is_empty() && uuid.starts_with("GPU-") && driver_major(driver).is_some()).then(
                || NvidiaGpu {
                    name: name.to_owned(),
                    uuid: uuid.to_owned(),
                    compute,
                    driver: driver.to_owned(),
                },
            )
        })
        .collect()
}

/// Verdict : pilote d'abord (le même pour toutes les cartes), puis capacité
/// de calcul de chaque carte.
pub fn assess(gpus: &[NvidiaGpu], pins: &Pins) -> Assessment {
    let Some(first) = gpus.first() else {
        return Assessment {
            compat: Compat::NoNvidia,
            usable: Vec::new(),
            shown: None,
        };
    };
    if driver_major(&first.driver).is_none_or(|major| major < pins.driver_min) {
        return Assessment {
            compat: Compat::DriverTooOld,
            usable: Vec::new(),
            shown: Some(first.clone()),
        };
    }
    let usable: Vec<NvidiaGpu> = gpus
        .iter()
        .filter(|gpu| gpu.compute >= pins.compute_min)
        .cloned()
        .collect();
    Assessment {
        compat: if usable.is_empty() {
            Compat::ComputeTooLow
        } else {
            Compat::Compatible
        },
        shown: usable.first().or(Some(first)).cloned(),
        usable,
    }
}

/// Cartes NVIDIA de la machine ; aucune si `nvidia-smi` est absent, échoue
/// ou dépasse [`TIMEOUT`].
pub async fn query() -> Vec<NvidiaGpu> {
    let mut command = Command::new(crate::hub::hardware::nvidia_smi());
    command
        .no_window()
        .args([
            "--query-gpu=name,uuid,compute_cap,driver_version",
            "--format=csv,noheader",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(TIMEOUT, command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            parse(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cuda::{Platform, pins};

    fn pins() -> Pins {
        pins::parse(
            include_str!("../../../../packaging/cuda.env"),
            include_str!("../../../../packaging/llama.env"),
            Platform::Linux,
        )
        .unwrap()
    }

    #[test]
    fn nvidia_smi_lines_are_read() {
        let text = "NVIDIA GeForce RTX 3090, GPU-1c2d3e4f-0000-1111-2222-333344445555, 8.6, 615.78.08\n\
                    Quadro, with comma, GPU-aa, 6.1, 581.57\r\n\
                    Old card, GPU-bb, [N/A], 390.12\n\
                    garbage\n";
        let gpus = parse(text);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 3090");
        assert_eq!(gpus[0].uuid, "GPU-1c2d3e4f-0000-1111-2222-333344445555");
        assert_eq!(gpus[0].compute, (8, 6));
        assert_eq!(gpus[0].driver, "615.78.08");
        // Virgule dans le nom : les trois derniers champs sont pris à droite.
        assert_eq!(gpus[1].name, "Quadro, with comma");
        assert_eq!(gpus[1].driver, "581.57");
    }

    fn gpu(name: &str, compute: (u32, u32), driver: &str) -> NvidiaGpu {
        NvidiaGpu {
            name: name.into(),
            uuid: format!("GPU-{name}"),
            compute,
            driver: driver.into(),
        }
    }

    #[test]
    fn compatibility_needs_driver_580_and_turing() {
        let pins = pins();
        let none = assess(&[], &pins);
        assert_eq!(none.compat, Compat::NoNvidia);
        assert_eq!(none.shown, None);
        // Pilote 575 : trop ancien, même pour une carte récente.
        let old = assess(&[gpu("4090", (8, 9), "575.57.08")], &pins);
        assert_eq!(old.compat, Compat::DriverTooOld);
        assert!(old.usable.is_empty());
        assert_eq!(old.shown.unwrap().name, "4090");
        // Bornes : 580 et 7.5 passent.
        let edge = assess(&[gpu("2060", (7, 5), "580.65.06")], &pins);
        assert_eq!(edge.compat, Compat::Compatible);
        // Pascal (6.1) et Volta (7.0) : trop anciennes pour CUDA 13.
        let pascal = assess(
            &[gpu("1080", (6, 1), "615.1"), gpu("V100", (7, 0), "615.1")],
            &pins,
        );
        assert_eq!(pascal.compat, Compat::ComputeTooLow);
        assert_eq!(pascal.shown.unwrap().name, "1080");
        // Mélange : seule la carte compatible est donnée à CUDA, et nommée.
        let mixed = assess(
            &[gpu("1080", (6, 1), "615.1"), gpu("3090", (8, 6), "615.1")],
            &pins,
        );
        assert_eq!(mixed.compat, Compat::Compatible);
        assert_eq!(mixed.usable.len(), 1);
        assert_eq!(mixed.usable[0].name, "3090");
        assert_eq!(mixed.shown.unwrap().name, "3090");
        assert_eq!(driver_major("garbage"), None);
        assert_eq!(
            assess(&[gpu("x", (8, 6), "garbage")], &pins).compat,
            Compat::DriverTooOld
        );
    }
}
