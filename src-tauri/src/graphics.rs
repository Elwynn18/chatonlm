//! Contournements graphiques propres à Linux. Sous Windows, rien n'est fait
//! (le fichier du pilote que `apply_nvidia_workaround` cherche n'y existe
//! pas) : les appelants n'ont pas besoin de `cfg`.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Variable lue par le pilote NVIDIA au chargement d'EGL.
pub const NV_EXPLICIT_SYNC_ENV: &str = "__NV_DISABLE_EXPLICIT_SYNC";

/// Vrai si ChatonLM a posé la variable lui-même (à ne pas transmettre aux
/// programmes de la machine qu'il lance).
static APPLIED: AtomicBool = AtomicBool::new(false);

/// Présent quand le pilote NVIDIA (propriétaire ou « open ») est chargé.
const NVIDIA_DRIVER_PROC: &str = "/proc/driver/nvidia/version";

/// Sous Wayland avec un pilote NVIDIA, le rendu DMABUF de WebKitGTK échoue
/// (« Error 71 (Protocol error) dispatching to Wayland display ») tant que la
/// synchronisation explicite du pilote est active. On la désactive pour
/// ChatonLM, sauf si l'utilisateur a déjà choisi une valeur ; le rendu
/// accéléré est conservé.
///
/// À appeler au tout début du programme, avant la création du moindre thread.
pub fn apply_nvidia_workaround() {
    let already_set = std::env::var_os(NV_EXPLICIT_SYNC_ENV).is_some();
    let nvidia = Path::new(NVIDIA_DRIVER_PROC).exists();
    if should_disable_explicit_sync(already_set, nvidia) {
        // SAFETY : appelé depuis `run()` avant tout autre code ; aucun autre
        // thread ne peut lire l'environnement en même temps.
        unsafe { std::env::set_var(NV_EXPLICIT_SYNC_ENV, "1") };
        APPLIED.store(true, Ordering::Relaxed);
    }
}

/// La variable du contournement a-t-elle été posée par ChatonLM ?
#[cfg_attr(windows, allow(dead_code))]
pub fn nvidia_workaround_applied() -> bool {
    APPLIED.load(Ordering::Relaxed)
}

fn should_disable_explicit_sync(already_set: bool, nvidia: bool) -> bool {
    nvidia && !already_set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_with_nvidia_and_without_user_choice() {
        assert!(should_disable_explicit_sync(false, true));
        assert!(!should_disable_explicit_sync(true, true));
        assert!(!should_disable_explicit_sync(false, false));
    }
}
