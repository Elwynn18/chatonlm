//! Installation d'Ollama sur cette machine : binaire, et ce qui le lance.
//!
//! - Linux ([`linux`]) : binaire du PATH et service systemd, lu et démarré
//!   par D-Bus.
//! - Windows ([`windows`]) : pas de service ; l'app de bureau d'Ollama
//!   (`ollama app.exe`, zone de notification) lance `ollama serve` et en
//!   tient lieu : installée ou non, lancée ou non, démarrée par « Start
//!   Ollama ». Sa description passe par les mêmes types ([`Unit`],
//!   [`Service`]) : la décision de l'écran d'accueil (`detect::decide`) est
//!   la même sur les deux systèmes.
//!
//! Mêmes fonctions sur les deux : `find_binary`, `ollama_unit`, `start_unit`
//! et `local_listener`.

pub mod app;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
pub(crate) mod windows;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(windows)]
pub use windows::*;

/// Service systemd d'Ollama.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    /// Service de l'utilisateur (`systemctl --user`), pas du système.
    pub user: bool,
    /// `ActiveState` : active, inactive, activating, failed...
    pub active_state: String,
    /// `SubState` : running, dead, auto-restart...
    pub sub_state: String,
    /// `UnitFileState` : enabled, disabled, static...
    pub unit_file_state: String,
}

/// État de l'unité `ollama.service`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Aucune unité de ce nom.
    NotFound,
    Loaded(Service),
    /// systemd injoignable (pas de systemd, bus absent...).
    Unknown,
}

/// Fin d'une demande de démarrage du service.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum StartResult {
    /// Tâche de démarrage acceptée par systemd.
    Started,
    /// Invite de polkit fermée ou refusée, ou pas d'agent d'authentification.
    NotAuthorized,
    /// Aucune unité `ollama.service`.
    NoUnit,
    /// Autre échec, avec le message de systemd ou de D-Bus.
    Failed(String),
}

/// Quelque chose écoute-t-il sur ce port TCP de la machine ? `None` : pas
/// de réponse sûre. Sous Linux, toujours `None` : un port fermé y refuse la
/// connexion aussitôt, la sonde HTTP suffit (voir `detect::settle`).
#[cfg(not(windows))]
pub fn local_listener(_port: u16) -> Option<bool> {
    None
}
