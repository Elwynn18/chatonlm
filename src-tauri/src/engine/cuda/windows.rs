//! Pack CUDA sous Windows : pack `windows-x64` (dossier du pack en tête du
//! `PATH` de l'enfant, voir [`super::runtime_env`]), x86-64 seulement.

use super::Platform;

pub const PLATFORM: Option<Platform> = if cfg!(target_arch = "x86_64") {
    Some(Platform::Windows)
} else {
    None
};
