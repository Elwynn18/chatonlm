//! Travail de l'app sur Ollama hors des réponses de conversation (propre à
//! ChatonLM, lot 3 du lien VS Code) : Correction, Étude, polissage de la
//! dictée, plan de la visite guidée (`tour.plan`), chargement et
//! déchargement demandés par la page ou un bot. Tant qu'un travail est
//! tenu, l'autocomplétion de l'éditeur attend (calcul partagé, et une
//! proposition mise dans la file d'Ollama pendant un chargement le
//! rechargerait ensuite : rapport 92, M5 et M6).
//!
//! Le compteur appartient au moteur de chat ([`crate::chat::ChatEngine`],
//! un par app) ; il est posé une fois au démarrage ([`install`]) pour les
//! travaux qui n'ont pas l'app sous la main ([`hold`]). Sans pose (essais
//! unitaires), [`hold`] ne tient rien : les essais en parallèle d'un même
//! processus ne se gênent pas.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use tokio::sync::Notify;

/// Travaux en cours, et réveil à chaque début ou fin (réponses de
/// conversation comprises : le moteur de chat s'en sert aussi).
#[derive(Default)]
pub struct Activity {
    count: AtomicUsize,
    changed: Notify,
}

/// Travail tenu : compté jusqu'à ce qu'il soit lâché.
pub struct Hold(Arc<Activity>);

impl Activity {
    /// Tient un travail ; réveille ceux qui attendent un début.
    pub fn hold(self: &Arc<Self>) -> Hold {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_waiters();
        Hold(Arc::clone(self))
    }

    /// Au moins un travail tenu.
    pub fn active(&self) -> bool {
        self.count.load(Ordering::SeqCst) > 0
    }

    /// Réveillé à chaque début ou fin.
    pub fn changed(&self) -> &Notify {
        &self.changed
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::SeqCst);
        self.0.changed.notify_waiters();
    }
}

/// Compteur de l'app, posé une fois au démarrage.
static INSTALLED: OnceLock<Arc<Activity>> = OnceLock::new();

/// Pose le compteur de l'app (une fois ; les suivantes sont ignorées).
pub fn install(activity: Arc<Activity>) {
    let _ = INSTALLED.set(activity);
}

/// Tient un travail sur le compteur de l'app ; rien sans pose.
pub fn hold() -> Option<Hold> {
    INSTALLED.get().map(Activity::hold)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_hold_counts_until_dropped_and_wakes_waiters() {
        let activity = Arc::new(Activity::default());
        assert!(!activity.active());
        let woken = activity.changed().notified();
        let first = activity.hold();
        tokio::time::timeout(std::time::Duration::from_secs(1), woken)
            .await
            .expect("a start wakes the waiters");
        let second = activity.hold();
        drop(first);
        assert!(activity.active(), "one still held");
        let woken = activity.changed().notified();
        drop(second);
        tokio::time::timeout(std::time::Duration::from_secs(1), woken)
            .await
            .expect("an end wakes the waiters");
        assert!(!activity.active());
    }
}
