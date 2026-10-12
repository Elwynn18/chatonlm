//! Mémoire de la machine du serveur (34 § 9, contrat § 6) : GPU NVIDIA par
//! `nvidia-smi`, GPU AMD par sysfs, RAM par `/proc/meminfo`. Valable seulement
//! pour un serveur local ; Ollama n'expose pas ces valeurs.
//!
//! Sous Windows (`hardware/windows.rs`) : RAM par `GlobalMemoryStatusEx`,
//! comme l'app officielle (discover/cpu_windows.go) ; GPU NVIDIA par le même
//! `nvidia-smi` (livré avec le pilote dans `System32`) ; GPU AMD par DXGI, qui
//! donne leur mémoire dédiée mais pas ce qui en reste libre pour tout le
//! système (`vram_free` inconnu).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::process::Command;

use super::fit;
use super::types::{HubGpu, HubHardware};
use crate::os_util::NoWindow;

/// Lectures propres au système : `memory()` (RAM totale et disponible, en
/// octets) et `amd_gpus()`.
#[cfg(target_os = "linux")]
mod os {
    use std::path::Path;

    use super::HubGpu;

    pub fn memory() -> Option<(u64, u64)> {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| super::parse_meminfo(&text))
    }

    pub fn amd_gpus() -> Vec<HubGpu> {
        super::amd_gpus(Path::new("/sys/class/drm"))
    }
}
#[cfg(windows)]
#[path = "hardware/windows.rs"]
mod os;

/// Délai de `nvidia-smi`.
const NVIDIA_TIMEOUT: Duration = Duration::from_secs(3);
/// Durée de validité d'une détection.
const KEEP: Duration = Duration::from_secs(60);
const MIB: u64 = 1024 * 1024;

/// Machine détectée, sans le contexte (qui dépend des réglages).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    pub gpus: Vec<HubGpu>,
    pub ram_total: u64,
    pub ram_available: u64,
}

impl Machine {
    /// Avec le contexte retenu : le réglage s'il est non nul, sinon celui
    /// qu'Ollama choisirait d'après la VRAM.
    pub fn with_context(self, setting: u32) -> HubHardware {
        let vram: u64 = self.gpus.iter().map(|gpu| gpu.vram_total).sum();
        let context = if setting > 0 {
            setting
        } else {
            fit::default_context(vram)
        };
        HubHardware {
            gpus: self.gpus,
            ram_total: self.ram_total,
            ram_available: self.ram_available,
            context,
        }
    }
}

/// Dernière détection, gardée [`KEEP`].
#[derive(Default)]
pub struct Detector {
    last: tokio::sync::Mutex<Option<(Instant, Option<Machine>)>>,
}

impl Detector {
    pub async fn machine(&self) -> Option<Machine> {
        let mut last = self.last.lock().await;
        if let Some((at, machine)) = last.as_ref()
            && at.elapsed() < KEEP
        {
            return machine.clone();
        }
        let machine = detect().await;
        *last = Some((Instant::now(), machine.clone()));
        machine
    }
}

/// Détection réelle. `None` si la mémoire ne se lit pas (`/proc/meminfo`
/// illisible).
async fn detect() -> Option<Machine> {
    let files = tauri::async_runtime::spawn_blocking(|| (os::memory(), os::amd_gpus()));
    let (memory, amd) = files.await.ok()?;
    let (ram_total, ram_available) = memory?;
    let mut gpus = nvidia().await;
    gpus.extend(amd);
    Some(Machine {
        gpus,
        ram_total,
        ram_available,
    })
}

/// GPU NVIDIA ; aucun si `nvidia-smi` est absent, échoue ou dépasse 3 s.
async fn nvidia() -> Vec<HubGpu> {
    let mut command = Command::new(nvidia_smi());
    command
        .no_window()
        .args([
            "--query-gpu=name,memory.total,memory.free",
            "--format=csv,noheader,nounits",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(NVIDIA_TIMEOUT, command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            parse_nvidia(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
    }
}

/// Programme `nvidia-smi` : celui du `PATH` ; sous Windows, d'abord celui du
/// pilote dans `System32`, sans dépendre du `PATH`.
fn nvidia_smi() -> PathBuf {
    if cfg!(windows)
        && let Some(root) = std::env::var_os("SystemRoot")
    {
        let system = PathBuf::from(root).join("System32").join("nvidia-smi.exe");
        if system.is_file() {
            return system;
        }
    }
    PathBuf::from("nvidia-smi")
}

/// Carte graphique vue par DXGI (Windows).
#[cfg(any(windows, test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    /// Identifiant PCI du fabricant.
    pub vendor: u32,
    /// Rendu logiciel (« Microsoft Basic Render Driver »).
    pub software: bool,
    /// Mémoire vidéo dédiée, en octets.
    pub dedicated: u64,
}

/// GPU AMD parmi les cartes de DXGI, comme [`amd_gpus`] sous Linux :
/// fabricant `0x1002`, mémoire dédiée non nulle ; la mémoire libre n'est pas
/// connue. Les GPU NVIDIA viennent de `nvidia-smi`, les autres ne comptent
/// pas (comme sous Linux).
#[cfg(any(windows, test))]
pub fn amd_from_adapters(adapters: &[Adapter]) -> Vec<HubGpu> {
    adapters
        .iter()
        .filter(|adapter| adapter.vendor == 0x1002 && !adapter.software && adapter.dedicated > 0)
        .map(|adapter| HubGpu {
            name: Some(adapter.name.trim())
                .filter(|name| !name.is_empty())
                .unwrap_or("AMD GPU")
                .to_owned(),
            vram_total: adapter.dedicated,
            vram_free: None,
        })
        .collect()
}

/// Lignes « nom, total, libre » de `nvidia-smi` (Mio).
pub fn parse_nvidia(text: &str) -> Vec<HubGpu> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.rsplitn(3, ',').map(str::trim);
            let free = fields.next()?;
            let total = fields.next()?;
            let name = fields.next()?;
            let total: u64 = total.parse().ok()?;
            (!name.is_empty() && total > 0).then(|| HubGpu {
                name: name.to_owned(),
                vram_total: total * MIB,
                vram_free: free.parse::<u64>().ok().map(|free| free * MIB),
            })
        })
        .collect()
}

/// GPU AMD de sysfs (`card<N>/device`, fabricant `0x1002`) : VRAM totale et
/// utilisée.
#[cfg_attr(windows, allow(dead_code))]
pub fn amd_gpus(drm: &Path) -> Vec<HubGpu> {
    let Ok(entries) = std::fs::read_dir(drm) else {
        return Vec::new();
    };
    let mut cards: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix("card"))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|entry| entry.path().join("device"))
        .collect();
    cards.sort();
    let read = |path: &Path| std::fs::read_to_string(path).ok();
    cards
        .into_iter()
        .filter(|device| read(&device.join("vendor")).is_some_and(|v| v.trim() == "0x1002"))
        .filter_map(|device| {
            let total: u64 = read(&device.join("mem_info_vram_total"))?
                .trim()
                .parse()
                .ok()?;
            let used = read(&device.join("mem_info_vram_used"))
                .and_then(|used| used.trim().parse::<u64>().ok());
            let name = read(&device.join("product_name"))
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "AMD GPU".to_owned());
            (total > 0).then(|| HubGpu {
                name,
                vram_total: total,
                vram_free: used.map(|used| total.saturating_sub(used)),
            })
        })
        .collect()
}

/// `MemTotal` et `MemAvailable` (kio) en octets.
#[cfg_attr(windows, allow(dead_code))]
pub fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let field = |name: &str| {
        text.lines().find_map(|line| {
            let value = line.strip_prefix(name)?.strip_prefix(':')?;
            let kib: u64 = value.trim().trim_end_matches("kB").trim().parse().ok()?;
            Some(kib * 1024)
        })
    };
    let total = field("MemTotal")?;
    Some((total, field("MemAvailable").unwrap_or(total)))
}

/// Machine imposée par les tests de l'app (`CHATONLM_E2E_HARDWARE`, builds de
/// débogage ou `e2e` seulement) : `None` sans variable ; `Some(None)` pour
/// `none` (serveur jugé distant) ; `Some(Some(machine))` pour le JSON d'un
/// `HubHardware` sans `context`.
#[cfg(any(debug_assertions, feature = "e2e"))]
pub fn injected() -> Option<Option<Machine>> {
    let value = std::env::var("CHATONLM_E2E_HARDWARE").ok()?;
    let parsed = parse_injected(&value);
    if parsed.is_none() {
        eprintln!("chatonlm: CHATONLM_E2E_HARDWARE invalide, ignoré : {value}");
    }
    parsed
}

#[cfg(any(test, debug_assertions, feature = "e2e"))]
fn parse_injected(value: &str) -> Option<Option<Machine>> {
    if value.trim() == "none" {
        return Some(None);
    }
    serde_json::from_str(value).ok().map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_lines_are_read_in_mib() {
        let gpus = parse_nvidia(
            "NVIDIA GeForce RTX 3090, 24576, 21447\nTesla, T4, 15360, [N/A]\nbad line\n, 0, 0\n",
        );
        assert_eq!(
            gpus,
            [
                HubGpu {
                    name: "NVIDIA GeForce RTX 3090".into(),
                    vram_total: 24576 * MIB,
                    vram_free: Some(21447 * MIB),
                },
                HubGpu {
                    name: "Tesla, T4".into(),
                    vram_total: 15360 * MIB,
                    vram_free: None,
                },
            ]
        );
        assert!(parse_nvidia("").is_empty());
    }

    #[test]
    fn meminfo_is_read_in_bytes() {
        let text = "MemTotal:       32768596 kB\nMemFree:  1 kB\nMemAvailable:   25994748 kB\n";
        assert_eq!(
            parse_meminfo(text),
            Some((32768596 * 1024, 25994748 * 1024))
        );
        assert_eq!(
            parse_meminfo("MemTotal: 1000 kB\n"),
            Some((1024000, 1024000))
        );
        assert_eq!(parse_meminfo("nothing"), None);
    }

    #[test]
    fn amd_cards_are_read_from_sysfs() {
        let drm = tempfile::tempdir().unwrap();
        let card = |name: &str, vendor: &str, total: Option<&str>, used: Option<&str>| {
            let device = drm.path().join(name).join("device");
            std::fs::create_dir_all(&device).unwrap();
            std::fs::write(device.join("vendor"), vendor).unwrap();
            if let Some(total) = total {
                std::fs::write(device.join("mem_info_vram_total"), total).unwrap();
            }
            if let Some(used) = used {
                std::fs::write(device.join("mem_info_vram_used"), used).unwrap();
            }
        };
        card(
            "card0",
            "0x1002\n",
            Some("17163091968\n"),
            Some("1163091968\n"),
        );
        card("card1", "0x10de\n", Some("100"), None);
        card("card2", "0x1002\n", None, None);
        card("card0-DP-1", "0x1002\n", Some("5"), None);
        std::fs::write(
            drm.path().join("card0/device/product_name"),
            "Radeon RX 7800 XT\n",
        )
        .unwrap();
        assert_eq!(
            amd_gpus(drm.path()),
            [HubGpu {
                name: "Radeon RX 7800 XT".into(),
                vram_total: 17163091968,
                vram_free: Some(16000000000),
            }]
        );
        assert!(amd_gpus(&drm.path().join("absent")).is_empty());
    }

    #[test]
    fn amd_cards_are_kept_among_the_dxgi_adapters() {
        let adapter = |name: &str, vendor: u32, software: bool, dedicated: u64| Adapter {
            name: name.into(),
            vendor,
            software,
            dedicated,
        };
        let gpus = amd_from_adapters(&[
            adapter("NVIDIA GeForce RTX 3090", 0x10de, false, 24 << 30),
            adapter(" AMD Radeon RX 7800 XT ", 0x1002, false, 16 << 30),
            adapter("Intel(R) UHD Graphics 770", 0x8086, false, 128 << 20),
            adapter("Microsoft Basic Render Driver", 0x1414, true, 0),
            // Sans nom, sans mémoire dédiée, rendu logiciel.
            adapter("", 0x1002, false, 512 << 20),
            adapter("AMD sans mémoire", 0x1002, false, 0),
            adapter("AMD logiciel", 0x1002, true, 1 << 30),
        ]);
        assert_eq!(
            gpus,
            [
                HubGpu {
                    name: "AMD Radeon RX 7800 XT".into(),
                    vram_total: 16 << 30,
                    vram_free: None,
                },
                HubGpu {
                    name: "AMD GPU".into(),
                    vram_total: 512 << 20,
                    vram_free: None,
                },
            ]
        );
        assert!(amd_from_adapters(&[]).is_empty());
    }

    #[test]
    fn nvidia_smi_is_found_by_its_name_outside_windows() {
        if !cfg!(windows) {
            assert_eq!(nvidia_smi(), PathBuf::from("nvidia-smi"));
        }
    }

    #[test]
    fn the_context_comes_from_the_setting_or_the_vram() {
        let machine = Machine {
            gpus: parse_nvidia("RTX 3090, 24576, 20000"),
            ram_total: 32 << 30,
            ram_available: 16 << 30,
        };
        assert_eq!(machine.clone().with_context(0).context, 32768);
        assert_eq!(machine.with_context(8192).context, 8192);
        let cpu = Machine {
            gpus: vec![],
            ram_total: 16 << 30,
            ram_available: 8 << 30,
        };
        assert_eq!(cpu.with_context(0).context, 4096);
    }

    #[test]
    fn the_injected_machine() {
        assert_eq!(parse_injected("none"), Some(None));
        let machine = parse_injected(
            r#"{"gpus":[{"name":"RTX 3090","vramTotal":25769803776,"vramFree":null}],"ramTotal":34359738368,"ramAvailable":17179869184}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(machine.gpus[0].vram_total, 25769803776);
        assert_eq!(parse_injected("{"), None);
    }
}
