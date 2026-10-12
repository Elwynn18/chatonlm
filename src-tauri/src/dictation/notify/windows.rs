//! Serveur de notifications sous Windows : toasts WinRT
//! (`Windows.UI.Notifications`), au nom de l'identifiant de l'app (AUMID, posé
//! sur le raccourci du menu Démarrer par l'installateur, et inscrit ici au
//! registre de l'utilisateur pour une app lancée sans raccourci).
//!
//! Une notification en remplace une autre par son étiquette (même étiquette,
//! même groupe) ; elle est retirée par l'historique. Le clic sur le bouton
//! d'action arrive par l'évènement `Activated`, tant que l'app tourne (pas
//! d'activateur COM : une notification restée dans le centre de
//! notifications après la fin de l'app ne fait plus rien).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;
use tokio::sync::oneshot;
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{
    ToastActivatedEventArgs, ToastNotification, ToastNotificationManager,
};
use windows::core::{HSTRING, IInspectable, Interface};

use super::toast::{self, Toast};
use super::{Action, Bus, Message};
use crate::dictation::clipboard::BoxFuture;
use crate::i18n;

/// Numéro de la prochaine notification (commun aux notificateurs : leurs
/// étiquettes ne se confondent pas).
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// Clic attendu sur le bouton d'une notification.
struct Waiting {
    key: &'static str,
    run: Box<dyn FnOnce() + Send>,
}

/// Toasts d'un notificateur.
#[derive(Default)]
pub(super) struct ToastBus {
    /// Identifiant de l'app de chaque notification montrée (pour la retirer).
    shown: Arc<Mutex<HashMap<u32, String>>>,
    /// Clics attendus, par notification.
    waiting: Arc<Mutex<HashMap<u32, Waiting>>>,
}

/// Arguments du bouton cliqué (vide : clic sur le corps de la notification).
fn arguments(args: &Option<IInspectable>) -> Option<String> {
    let args = args.as_ref()?.cast::<ToastActivatedEventArgs>().ok()?;
    let arguments = args.Arguments().ok()?;
    (!arguments.is_empty()).then(|| arguments.to_string())
}

/// Montre le toast `xml` au nom de `app_id`, sous l'étiquette `tag` s'il y
/// en a une ; `on_activated` reçoit les arguments du bouton cliqué.
fn show_toast(
    app_id: &str,
    xml: &str,
    tag: Option<&str>,
    on_activated: Option<Box<dyn Fn(Option<String>) + Send>>,
) -> windows::core::Result<()> {
    let document = XmlDocument::new()?;
    document.LoadXml(&HSTRING::from(xml))?;
    let notification = ToastNotification::CreateToastNotification(&document)?;
    if let Some(tag) = tag {
        notification.SetTag(&HSTRING::from(tag))?;
        notification.SetGroup(&HSTRING::from(toast::GROUP))?;
    }
    if let Some(on_activated) = on_activated {
        notification.Activated(&TypedEventHandler::new(move |_, args| {
            on_activated(arguments(&args));
            Ok(())
        }))?;
    }
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id))?.Show(&notification)
}

/// Notification simple, sans suite ni action (fermeture qui masque la
/// fenêtre, mise à jour prête) : titre et corps en texte brut. Bloquant.
pub fn show_simple(app_id: &str, summary: &str, body: &str) -> Result<(), String> {
    let body = super::escape(body);
    let xml = Toast {
        summary,
        body: &body,
        ..Toast::default()
    }
    .xml();
    show_toast(app_id, &xml, None, None).map_err(|error| error.to_string())
}

/// Inscrit l'app (nom, icône) sous son identifiant dans le registre de
/// l'utilisateur : sans raccourci du menu Démarrer qui porte cet identifiant
/// (app lancée hors installation), Windows ne montrerait pas ses toasts.
pub(super) fn register_app(app_id: &str, icon: Option<&Path>) {
    let registered = (|| -> windows_registry::Result<()> {
        let key = windows_registry::CURRENT_USER
            .create(format!(r"Software\Classes\AppUserModelId\{app_id}"))?;
        key.set_string("DisplayName", "ChatonLM")?;
        if let Some(icon) = icon {
            key.set_string("IconUri", icon.to_string_lossy())?;
        }
        Ok(())
    })();
    if let Err(error) = registered {
        eprintln!("chatonlm: notifications not registered: {error}");
    }
}

impl Bus for ToastBus {
    fn show(&self, message: Message) -> BoxFuture<Result<u32, String>> {
        let shown = self.shown.clone();
        let waiting = self.waiting.clone();
        Box::pin(async move {
            tauri::async_runtime::spawn_blocking(move || {
                let Some(identity) = &message.identity else {
                    return Err("notifications are not configured yet".to_owned());
                };
                let app_id = identity.desktop_entry.clone();
                let id = message
                    .replaces
                    .unwrap_or_else(|| NEXT_ID.fetch_add(1, Ordering::Relaxed));
                // Le bouton de la notification remplacée ne fait plus rien.
                waiting.lock().remove(&id);
                let label = message
                    .action
                    .map(|action| action.label_in(i18n::current()));
                let xml = Toast {
                    summary: &message.summary,
                    body: &message.body,
                    action: message
                        .action
                        .zip(label.as_deref())
                        .map(|(action, label)| (action.key(), label)),
                    silent: message.transient || message.resident,
                    long: message.resident,
                }
                .xml();
                let on_activated = message.action.map(|_| {
                    let waiting = waiting.clone();
                    Box::new(move |arguments: Option<String>| {
                        let clicked = {
                            let mut waiting = waiting.lock();
                            let matches = waiting
                                .get(&id)
                                .is_some_and(|entry| arguments.as_deref() == Some(entry.key));
                            matches.then(|| waiting.remove(&id)).flatten()
                        };
                        if let Some(clicked) = clicked {
                            (clicked.run)();
                        }
                    }) as Box<dyn Fn(Option<String>) + Send>
                });
                show_toast(&app_id, &xml, Some(&toast::tag(id)), on_activated)
                    .map_err(|error| error.to_string())?;
                shown.lock().insert(id, app_id);
                Ok(id)
            })
            .await
            .map_err(|error| error.to_string())?
        })
    }

    fn close(&self, id: u32) -> BoxFuture<()> {
        let app_id = self.shown.lock().remove(&id);
        self.waiting.lock().remove(&id);
        Box::pin(async move {
            let Some(app_id) = app_id else {
                return;
            };
            let removed = tauri::async_runtime::spawn_blocking(move || {
                ToastNotificationManager::History()?.RemoveGroupedTagWithId(
                    &HSTRING::from(toast::tag(id)),
                    &HSTRING::from(toast::GROUP),
                    &HSTRING::from(app_id),
                )
            })
            .await;
            if let Ok(Err(error)) = removed {
                eprintln!("chatonlm: notification not removed: {error}");
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
        self.waiting.lock().insert(
            id,
            Waiting {
                key: action.key(),
                run,
            },
        );
        // `stop` envoyé ou lâché : le clic ne fait plus rien.
        let waiting = self.waiting.clone();
        tauri::async_runtime::spawn(async move {
            let _ = stop.await;
            waiting.lock().remove(&id);
        });
    }
}
