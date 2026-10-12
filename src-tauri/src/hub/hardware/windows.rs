//! Mémoire et cartes graphiques sous Windows : `GlobalMemoryStatusEx` et
//! DXGI. Le tri des cartes est fait par `amd_from_adapters`, hors des appels
//! au système.

use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

use super::{Adapter, HubGpu};
use crate::os_util::from_wide;

/// Cartes lues au plus (une machine n'en a que quelques-unes).
const MAX_ADAPTERS: u32 = 16;

/// RAM totale et disponible, en octets ; `None` si le système ne répond pas.
pub fn memory() -> Option<(u64, u64)> {
    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY : structure valide, dont la taille est annoncée dans `dwLength`.
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some((status.ullTotalPhys, status.ullAvailPhys))
}

/// GPU AMD de la machine ; aucun si DXGI ne répond pas.
pub fn amd_gpus() -> Vec<HubGpu> {
    super::amd_from_adapters(&adapters())
}

fn adapters() -> Vec<Adapter> {
    let mut found = Vec::new();
    // SAFETY : objets COM de DXGI créés, lus et libérés ici, sur ce seul fil.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return found;
        };
        for index in 0..MAX_ADAPTERS {
            // DXGI_ERROR_NOT_FOUND : plus de carte.
            let Ok(adapter) = factory.EnumAdapters1(index) else {
                break;
            };
            let Ok(description) = adapter.GetDesc1() else {
                continue;
            };
            found.push(Adapter {
                name: from_wide(&description.Description),
                vendor: description.VendorId,
                software: description.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
                dedicated: description.DedicatedVideoMemory as u64,
            });
        }
    }
    found
}
