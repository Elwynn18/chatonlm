//! Fin de l'app sous Linux : SIGTERM (fin de la session ou extinction par
//! systemd, `kill`), SIGINT (Ctrl+C dans un terminal) et SIGHUP (terminal
//! fermé ; systemd l'envoie aussi juste après SIGTERM à une portée de
//! session) ferment l'app comme Ctrl+Q, au lieu de la tuer sans arrêter ses
//! services. Les signaux suivants n'y changent rien : l'arrêt, borné, continue.
//!
//! Reste sans recours : la perte de la connexion au serveur d'affichage
//! (Wayland ou X11 fermé avant l'app), où GTK termine le processus lui-même ;
//! les conversations éphémères sont alors effacées au lancement suivant.

use tauri::AppHandle;
use tokio::signal::unix::{SignalKind, signal};

/// Écoute les signaux d'arrêt, dès le lancement.
pub fn listen(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = watch(move |name| {
            if super::exiting() {
                crate::app_log!("chatonlm: {name} received while quitting");
                return;
            }
            crate::app_log!("chatonlm: {name} received, quitting");
            crate::desktop::quit(&app);
        })
        .await;
        if let Err(error) = result {
            crate::app_log!("chatonlm: cannot listen to the termination signals: {error}");
        }
    });
}

/// Installe les gestionnaires des signaux d'arrêt (la disposition par défaut,
/// qui tue le processus, ne s'applique plus une fois cette fonction rendue),
/// puis appelle `on_signal` avec le nom de chacun reçu.
async fn watch(on_signal: impl Fn(&'static str) + Send + 'static) -> std::io::Result<()> {
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut hangup = signal(SignalKind::hangup())?;
    tokio::spawn(async move {
        loop {
            let name = tokio::select! {
                Some(()) = terminate.recv() => "SIGTERM",
                Some(()) = interrupt.recv() => "SIGINT",
                Some(()) = hangup.recv() => "SIGHUP",
                else => return,
            };
            on_signal(name);
        }
    });
    Ok(())
}

/// Fenêtres masquées, puis la demande envoyée tout de suite au serveur
/// d'affichage : le fil principal reste ensuite pris par l'arrêt des
/// services, sans tour de boucle GTK qui l'enverrait.
pub fn hide_windows(app: &AppHandle) {
    super::hide_all(app);
    if let Some(display) = gtk::gdk::Display::default() {
        display.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Signal envoyé par l'essai à son processus enfant.
    const CHILD: &str = "CHATONLM_TEST_SIGNAL_CHILD";

    /// Chaque signal d'arrêt est reçu (au lieu de tuer le processus) et
    /// annoncé par son nom. Essayé dans un processus enfant : un gestionnaire
    /// de signal reste installé jusqu'à la fin du processus.
    #[test]
    fn termination_signals_are_caught_and_named() {
        let exe = std::env::current_exe().unwrap();
        for (number, name) in [
            (libc::SIGTERM, "SIGTERM"),
            (libc::SIGINT, "SIGINT"),
            (libc::SIGHUP, "SIGHUP"),
        ] {
            let output = Command::new(&exe)
                .args([
                    "--exact",
                    "shutdown::os::tests::signal_child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD, number.to_string())
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success(),
                "{name}: {:?}\n{stdout}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(stdout.contains(&format!("caught {name}")), "{stdout}");
        }
    }

    /// Processus enfant de l'essai précédent (sans effet hors de lui) : se
    /// donne le signal demandé, et sort à sa réception.
    #[test]
    fn signal_child() {
        let Some(number) = std::env::var(CHILD).ok().and_then(|n| n.parse().ok()) else {
            return;
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            watch(|name| {
                println!("caught {name}");
                std::process::exit(0);
            })
            .await
            .unwrap();
            // SAFETY: `kill` vers son propre processus, sans pointeur.
            unsafe { libc::kill(libc::getpid(), number) };
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        });
        panic!("signal {number} not caught");
    }
}
