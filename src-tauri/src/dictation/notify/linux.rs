//! Serveur de notifications du bureau sous Linux : D-Bus
//! `org.freedesktop.Notifications`, par notify-rust. Le clic sur une action
//! est attendu sur un fil dédié (hors du runtime async).

use std::collections::HashMap;
use std::sync::Arc;

use notify_rust::{Hint, Notification};
use parking_lot::Mutex;
use tokio::sync::oneshot;

use super::{Action, Bus, Message};
use crate::dictation::clipboard::BoxFuture;
use crate::i18n;

/// Serveur du bureau, par notify-rust (session D-Bus).
#[derive(Default)]
pub(super) struct DbusBus {
    /// Notifications encore ouvertes : leur poignée sait les fermer.
    open: Arc<Mutex<HashMap<u32, notify_rust::NotificationHandle>>>,
}

impl Bus for DbusBus {
    fn show(&self, message: Message) -> BoxFuture<Result<u32, String>> {
        let open = self.open.clone();
        Box::pin(async move {
            let mut notification = Notification::new();
            notification
                .appname("ChatonLM")
                .summary(&message.summary)
                .body(&message.body)
                .hint(Hint::Transient(message.transient));
            if message.resident {
                notification.timeout(notify_rust::Timeout::Never);
            }
            if let Some(identity) = &message.identity {
                notification.hint(Hint::DesktopEntry(identity.desktop_entry.clone()));
                if let Some(icon) = &identity.icon {
                    notification.icon(&icon.to_string_lossy());
                }
            }
            if let Some(id) = message.replaces {
                notification.id(id);
                open.lock().remove(&id);
            }
            if let Some(action) = message.action {
                notification.action(action.key(), &action.label_in(i18n::current()));
            }
            let handle = notification
                .show_async()
                .await
                .map_err(|error| error.to_string())?;
            let id = handle.id();
            // Poignée gardée pour fermer (passagères, progressions) ou
            // attendre l'action.
            if message.transient || message.resident || message.action.is_some() {
                open.lock().insert(id, handle);
            }
            Ok(id)
        })
    }

    fn close(&self, id: u32) -> BoxFuture<()> {
        let handle = self.open.lock().remove(&id);
        Box::pin(async move {
            if let Some(handle) = handle {
                handle.close_async().await;
            }
        })
    }

    fn on_action(
        &self,
        id: u32,
        action: Action,
        stop: oneshot::Receiver<()>,
        run: Box<dyn FnOnce() + Send>,
    ) {
        let Some(handle) = self.open.lock().remove(&id) else {
            return;
        };
        // La notification reste dans l'historique : attente sur un fil à
        // part, jusqu'au clic, à sa fermeture ou à `stop`.
        let spawned = std::thread::Builder::new()
            .name("chatonlm-notification".into())
            .spawn(move || {
                zbus::block_on(async move {
                    tokio::select! {
                        biased;
                        _ = stop => {}
                        () = handle.wait_for_action_async(|response| {
                            if matches!(
                                response,
                                notify_rust::NotificationResponse::Action(key)
                                    if key == action.key()
                            ) {
                                run();
                            }
                        }) => {}
                    }
                });
            });
        if let Err(error) = spawned {
            eprintln!("chatonlm: notification action not watched: {error}");
        }
    }
}
