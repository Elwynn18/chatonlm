//! Priorité de l'utilisateur (propre à ChatonLM, Atelier, choix B du
//! 24 nov. 2026) : un travail de fond (projet de l'Atelier) cède la place à
//! ce que l'utilisateur fait lui-même. Est tenu pendant :
//!
//! - un envoi de conversation (page, bot d'Easy Link, pont de l'éditeur),
//!   images demandées dans la conversation comprises (`chat::send_to`) ;
//! - une dictée (`dictation`) ;
//! - le mode vocal hors pause (`voice_mode`, par [`latch`] : une séance
//!   longue, suivie d'état en état) ;
//! - une correction ou une reformulation (`proofread::run`).
//!
//! Le travail de fond attend [`Interactive::idle`] entre deux requêtes : rien
//! n'est tenu depuis un délai de grâce (l'utilisateur qui enchaîne deux
//! messages ne voit pas une requête de fond se glisser entre les deux).
//!
//! Comme [`crate::gpu::activity`], le compteur de l'app est posé une fois au
//! démarrage ([`install`]) pour les points d'accroche qui n'ont pas l'app
//! sous la main ([`hold`], [`latch`]) ; sans pose (essais unitaires), rien
//! n'est tenu : les essais en parallèle d'un même processus ne se gênent pas.
//! Code commun à Linux et Windows.

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

/// État du compteur.
#[derive(Clone, Debug, Default)]
struct State {
    /// Tenues en cours ([`Interactive::hold`]).
    holds: usize,
    /// Séances longues en cours ([`Interactive::latch`]), par nom.
    latched: BTreeSet<&'static str>,
    /// Dernier passage à « rien n'est tenu » (`None` : jamais rien tenu).
    /// Horloge de tokio : les essais la mettent en pause.
    free_since: Option<Instant>,
}

impl State {
    fn busy(&self) -> bool {
        self.holds > 0 || !self.latched.is_empty()
    }
}

/// Priorité de l'utilisateur (état Tauri, clonable).
#[derive(Clone)]
pub struct Interactive {
    state: Arc<watch::Sender<State>>,
}

impl Default for Interactive {
    fn default() -> Self {
        Self::new()
    }
}

/// Action de l'utilisateur en cours : comptée jusqu'à son `Drop`.
#[must_use = "relâchée aussitôt si elle n'est pas gardée"]
pub struct InteractiveHold {
    state: Arc<watch::Sender<State>>,
}

impl Drop for InteractiveHold {
    fn drop(&mut self) {
        self.state.send_modify(|state| {
            state.holds = state.holds.saturating_sub(1);
            if !state.busy() {
                state.free_since = Some(Instant::now());
            }
        });
    }
}

impl Interactive {
    pub fn new() -> Self {
        Self {
            state: Arc::new(watch::Sender::new(State::default())),
        }
    }

    /// Tient une action de l'utilisateur jusqu'au `Drop` du résultat.
    pub fn hold(&self) -> InteractiveHold {
        self.state.send_modify(|state| state.holds += 1);
        InteractiveHold {
            state: self.state.clone(),
        }
    }

    /// Séance longue `name` en cours (`busy`) ou finie : comme une tenue,
    /// pour un état suivi de changement en changement (mode vocal) plutôt
    /// que par une portée. Idempotent.
    pub fn latch(&self, name: &'static str, busy: bool) {
        self.state.send_if_modified(|state| {
            let changed = if busy {
                state.latched.insert(name)
            } else {
                state.latched.remove(name)
            };
            if changed && !state.busy() {
                state.free_since = Some(Instant::now());
            }
            changed
        });
    }

    /// L'utilisateur fait quelque chose en ce moment.
    #[cfg_attr(not(test), expect(dead_code, reason = "appelant : l'Atelier (lot 2)"))]
    pub fn is_busy(&self) -> bool {
        self.state.borrow().busy()
    }

    /// Prêt quand plus rien n'est tenu depuis `grace` (aussitôt si rien n'a
    /// jamais été tenu). Une action qui commence pendant le délai le
    /// reprend à sa fin.
    pub fn idle(&self, grace: Duration) -> impl Future<Output = ()> + Send + 'static {
        let mut changes = self.state.subscribe();
        async move {
            loop {
                let wait = {
                    let state = changes.borrow_and_update();
                    if state.busy() {
                        None
                    } else {
                        match state.free_since {
                            None => return,
                            Some(since) => {
                                let left = grace.saturating_sub(since.elapsed());
                                if left.is_zero() {
                                    return;
                                }
                                Some(left)
                            }
                        }
                    }
                };
                match wait {
                    // Tenu : attendre un changement.
                    None => {
                        if changes.changed().await.is_err() {
                            return;
                        }
                    }
                    // Libre depuis peu : la fin du délai, sauf changement.
                    Some(left) => {
                        tokio::select! {
                            () = tokio::time::sleep(left) => {}
                            changed = changes.changed() => {
                                if changed.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Compteur de l'app, posé une fois au démarrage.
static INSTALLED: OnceLock<Interactive> = OnceLock::new();

/// Pose le compteur de l'app (une fois ; les suivantes sont ignorées).
pub fn install(interactive: Interactive) {
    let _ = INSTALLED.set(interactive);
}

/// Tient une action de l'utilisateur sur le compteur de l'app ; rien sans
/// pose (essais).
pub fn hold() -> Option<InteractiveHold> {
    INSTALLED.get().map(Interactive::hold)
}

/// [`Interactive::latch`] sur le compteur de l'app ; rien sans pose.
pub fn latch(name: &'static str, busy: bool) {
    if let Some(interactive) = INSTALLED.get() {
        interactive.latch(name, busy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRACE: Duration = Duration::from_secs(5);

    /// Le futur d'attente est-il prêt sans faire avancer l'horloge ?
    async fn ready_now(future: &mut (impl Future<Output = ()> + Unpin)) -> bool {
        tokio::time::timeout(Duration::ZERO, future).await.is_ok()
    }

    #[test]
    fn holds_are_counted_until_dropped() {
        let interactive = Interactive::new();
        assert!(!interactive.is_busy());
        let first = interactive.hold();
        let second = interactive.clone().hold();
        assert!(interactive.is_busy());
        drop(first);
        assert!(interactive.is_busy(), "one hold left");
        drop(second);
        assert!(!interactive.is_busy());
    }

    #[test]
    fn a_latch_is_named_and_idempotent() {
        let interactive = Interactive::new();
        interactive.latch("voice", true);
        interactive.latch("voice", true);
        assert!(interactive.is_busy());
        let hold = interactive.hold();
        interactive.latch("voice", false);
        assert!(interactive.is_busy(), "the hold is still there");
        drop(hold);
        assert!(!interactive.is_busy());
        interactive.latch("voice", false);
        assert!(!interactive.is_busy());
    }

    #[tokio::test(start_paused = true)]
    async fn idle_is_ready_at_once_when_nothing_was_ever_held() {
        let interactive = Interactive::new();
        let mut idle = Box::pin(interactive.idle(GRACE));
        assert!(ready_now(&mut idle).await);
    }

    #[tokio::test(start_paused = true)]
    async fn idle_waits_for_the_end_then_the_grace() {
        let interactive = Interactive::new();
        let hold = interactive.hold();
        let mut idle = Box::pin(interactive.idle(GRACE));
        // Tenu : jamais prêt, même longtemps après.
        tokio::time::advance(Duration::from_secs(60)).await;
        assert!(!ready_now(&mut idle).await);
        drop(hold);
        assert!(!ready_now(&mut idle).await, "the grace starts at the end");
        tokio::time::advance(GRACE - Duration::from_millis(1)).await;
        assert!(!ready_now(&mut idle).await);
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(ready_now(&mut idle).await);
        // Libre depuis plus que le délai : aussitôt.
        let mut again = Box::pin(interactive.idle(GRACE));
        assert!(ready_now(&mut again).await);
    }

    #[tokio::test(start_paused = true)]
    async fn an_action_during_the_grace_starts_it_again() {
        let interactive = Interactive::new();
        drop(interactive.hold());
        let mut idle = Box::pin(interactive.idle(GRACE));
        tokio::time::advance(Duration::from_secs(4)).await;
        assert!(!ready_now(&mut idle).await);
        // Deuxième message, aussitôt fini.
        drop(interactive.hold());
        tokio::time::advance(Duration::from_secs(4)).await;
        assert!(!ready_now(&mut idle).await, "the grace restarted");
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(ready_now(&mut idle).await);
    }

    #[tokio::test(start_paused = true)]
    async fn a_latched_session_keeps_the_user_busy() {
        let interactive = Interactive::new();
        interactive.latch("voice", true);
        let mut idle = Box::pin(interactive.idle(GRACE));
        tokio::time::advance(Duration::from_secs(600)).await;
        assert!(!ready_now(&mut idle).await);
        interactive.latch("voice", false);
        tokio::time::advance(GRACE).await;
        assert!(ready_now(&mut idle).await);
    }

    #[test]
    fn without_install_nothing_is_held() {
        // Les essais ne posent jamais le compteur de l'app.
        assert!(INSTALLED.get().is_none());
        assert!(hold().is_none());
        latch("voice", true);
    }
}
