//! Fin de l'app sous Windows. Une fin de session (déconnexion, redémarrage,
//! extinction, Restart Manager d'un installateur) envoie `WM_ENDSESSION` à
//! chaque fenêtre de premier niveau : tao le reçoit dans sa fenêtre cachée,
//! en fait `RunEvent::Exit` (sans `ExitRequested`), puis termine le processus
//! (`std::process::exit`) dès le retour de l'appel, qui reste sur le fil
//! principal. `crate::shutdown::on_exit`, branché sur cet événement, arrête
//! donc les services avant que Windows ne ferme la session ; rien de plus à
//! écouter ici. Une app à fenêtres ne reçoit pas les événements de console
//! (`SetConsoleCtrlHandler`).

use tauri::AppHandle;

/// Rien à installer : voir l'en-tête.
pub fn listen(_app: &AppHandle) {}

/// Fenêtres masquées (`ShowWindow`, effet immédiat).
pub fn hide_windows(app: &AppHandle) {
    super::hide_all(app);
}
