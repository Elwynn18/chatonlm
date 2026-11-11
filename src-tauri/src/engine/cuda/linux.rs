//! Pack CUDA sous Linux : pack `linux-x64` (bibliothèques trouvées par le
//! RUNPATH `$ORIGIN` du moteur, rien à ajouter à l'environnement), x86-64
//! seulement.

use super::Platform;

pub const PLATFORM: Option<Platform> = if cfg!(target_arch = "x86_64") {
    Some(Platform::Linux)
} else {
    None
};
