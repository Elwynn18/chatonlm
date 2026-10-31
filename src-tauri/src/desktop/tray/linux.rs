//! Zone de notification sous Linux : StatusNotifierItem par ksni, en pur
//! D-Bus. Lancée au démarrage dans tous les cas : sans zone de notification
//! (StatusNotifierWatcher absent), elle attend qu'il apparaisse, et la
//! fermeture de la fenêtre quitte l'app comme avant.
//!
//! Clic gauche : fenêtre montrée ou masquée. Menu et infobulle : ceux du
//! module parent.

use std::sync::{Arc, LazyLock};

use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, OfflineReason, ToolTip, TrayMethods};
use parking_lot::Mutex;
use tauri::{AppHandle, Manager};

use super::{
    BUSY_BADGE, ICON_PNGS, RECORDING_BADGE, argb_pixels, bound_trigger, describe, dictation_ready,
    entries, run,
};
use crate::dictation::{Dictation, DictationPhase};
use crate::i18n::{self, Lang};
use crate::updater::{self, TrayUpdate};

/// Icônes prêtes, une par état.
struct Pixmaps {
    idle: Vec<Icon>,
    recording: Vec<Icon>,
    busy: Vec<Icon>,
}

static PIXMAPS: LazyLock<Pixmaps> = LazyLock::new(|| {
    let decode = |badge: Option<[u8; 3]>| -> Vec<Icon> {
        ICON_PNGS
            .iter()
            .filter_map(|png| match tauri::image::Image::from_bytes(png) {
                Ok(image) => Some(argb_icon(
                    image.width() as usize,
                    image.height() as usize,
                    image.rgba(),
                    badge,
                )),
                Err(error) => {
                    eprintln!("chatonlm: cannot decode the tray icon: {error}");
                    None
                }
            })
            .collect()
    };
    Pixmaps {
        idle: decode(None),
        recording: decode(Some(RECORDING_BADGE)),
        busy: decode(Some(BUSY_BADGE)),
    }
});

/// Pixmap ARGB32 (ordre réseau : A, R, G, B) d'une image RGBA, avec la
/// pastille de `badge` (voir `argb_pixels`).
fn argb_icon(width: usize, height: usize, rgba: &[u8], badge: Option<[u8; 3]>) -> Icon {
    Icon {
        width: width as i32,
        height: height as i32,
        data: argb_pixels(width, height, rgba, badge),
    }
}

/// Présence de la zone de notification, publiée dans `Desktop` une fois le
/// service lancé (voir `spawn`) ; un seul verrou pour l'ordre des
/// changements.
#[derive(Default)]
struct Presence {
    spawned: bool,
    online: bool,
}

fn publish(app: &AppHandle, available: bool) {
    if super::state(app).set_tray_available(available) {
        super::emit_status(app);
        if !available {
            // Plus d'icône : une fenêtre masquée serait introuvable.
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || super::reveal_main_window(&app));
        }
    }
}

struct LibreTray {
    app: AppHandle,
    /// Langue des libellés (celle de l'interface au dernier rafraîchissement).
    lang: Lang,
    phase: DictationPhase,
    /// Dictée tenue par un éditeur (pont VS Code).
    editor: bool,
    trigger: Option<String>,
    ready: bool,
    update: TrayUpdate,
    presence: Arc<Mutex<Presence>>,
}

impl LibreTray {
    fn set_online(&self, online: bool) {
        let mut presence = self.presence.lock();
        presence.online = online;
        if presence.spawned {
            publish(&self.app, online);
        }
    }
}

impl ksni::Tray for LibreTray {
    fn id(&self) -> String {
        "chatonlm".into()
    }

    fn title(&self) -> String {
        "ChatonLM".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        match self.phase {
            DictationPhase::Idle => PIXMAPS.idle.clone(),
            DictationPhase::Recording => PIXMAPS.recording.clone(),
            DictationPhase::Transcribing | DictationPhase::Polishing => PIXMAPS.busy.clone(),
        }
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "ChatonLM".into(),
            description: describe(self.phase, self.editor, self.trigger.as_deref(), self.lang),
            ..ToolTip::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let app = self.app.clone();
        tauri::async_runtime::spawn_blocking(move || super::toggle_main_window(&app));
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        entries(self.phase, self.editor, self.ready, self.update, self.lang)
            .into_iter()
            .map(|entry| match entry {
                None => MenuItem::Separator,
                Some(entry) => {
                    let action = entry.action;
                    StandardItem {
                        label: entry.label,
                        enabled: entry.enabled,
                        visible: entry.visible,
                        activate: Box::new(move |tray: &mut Self| run(&tray.app, action)),
                        ..StandardItem::default()
                    }
                    .into()
                }
            })
            .collect()
    }

    /// Menu sur le point de s'ouvrir : état de la dictée relu (un modèle a pu
    /// être installé ou supprimé entre-temps).
    fn menu_about_to_show(&mut self) {
        self.ready = dictation_ready(&self.app);
        self.trigger = bound_trigger(&self.app);
    }

    fn watcher_online(&self) {
        self.set_online(true);
    }

    fn watcher_offline(&self, reason: OfflineReason) -> bool {
        if let OfflineReason::Error(error) = reason {
            eprintln!("chatonlm: system tray unavailable: {error}");
        }
        self.set_online(false);
        // Le service continue : l'icône revient avec la zone de notification.
        true
    }
}

/// Poignée du service, une fois lancé.
#[derive(Default)]
pub(in crate::desktop) struct TrayState {
    handle: Mutex<Option<ksni::Handle<LibreTray>>>,
}

/// Lance le service de la zone de notification, puis le tient à jour des
/// phases de la dictée.
pub(in crate::desktop) fn spawn(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let presence = Arc::new(Mutex::new(Presence {
            spawned: false,
            online: true,
        }));
        let tray = LibreTray {
            app: app.clone(),
            lang: i18n::current(),
            phase: app.state::<Dictation>().phase(),
            editor: app.state::<Dictation>().for_editor(),
            trigger: bound_trigger(&app),
            ready: dictation_ready(&app),
            update: updater::tray_update(&app),
            presence: Arc::clone(&presence),
        };
        // Zone de notification absente au lancement (bureau qui démarre,
        // GNOME sans extension) : le service attend qu'elle apparaisse.
        let handle = match tray.assume_sni_available(true).spawn().await {
            Ok(handle) => handle,
            Err(error) => {
                eprintln!("chatonlm: cannot start the system tray icon: {error}");
                return;
            }
        };
        {
            let mut presence = presence.lock();
            presence.spawned = true;
            publish(&app, presence.online);
        }
        *super::state(&app).tray.handle.lock() = Some(handle.clone());

        let mut phases = app.state::<Dictation>().subscribe();
        loop {
            let phase = *phases.borrow_and_update();
            // Noté avant la phase : lu après elle, il est à jour.
            let editor = app.state::<Dictation>().for_editor();
            let ready = dictation_ready(&app);
            if handle
                .update(|tray| {
                    tray.phase = phase;
                    tray.editor = editor;
                    tray.ready = ready;
                })
                .await
                .is_none()
            {
                break;
            }
            if phases.changed().await.is_err() {
                break;
            }
        }
    });
}

/// Relit l'état de la dictée, la touche du raccourci, la langue de
/// l'interface et la mise à jour (réglages changés, raccourci lié, langue
/// choisie, mise à jour annoncée ou prête) : ksni renvoie au bureau le menu
/// et l'infobulle refaits.
pub(in crate::desktop) fn refresh(app: &AppHandle) {
    let Some(handle) = super::state(app).tray.handle.lock().clone() else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let ready = dictation_ready(&app);
        let trigger = bound_trigger(&app);
        let lang = i18n::current();
        let update = updater::tray_update(&app);
        handle
            .update(|tray| {
                tray.ready = ready;
                tray.trigger = trigger;
                tray.lang = lang;
                tray.update = update;
            })
            .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(icon: &Icon, x: usize, y: usize) -> [u8; 4] {
        let start = (y * icon.width as usize + x) * 4;
        icon.data[start..start + 4].try_into().unwrap()
    }

    #[test]
    fn icons_are_argb_with_a_badge_in_the_corner() {
        // Image 32×32 opaque, bleue.
        let rgba: Vec<u8> = [0x10, 0x20, 0xf0, 0xff].repeat(32 * 32);
        let plain = argb_icon(32, 32, &rgba, None);
        assert_eq!((plain.width, plain.height), (32, 32));
        assert_eq!(pixel(&plain, 0, 0), [0xff, 0x10, 0x20, 0xf0]);
        assert_eq!(pixel(&plain, 26, 26), [0xff, 0x10, 0x20, 0xf0]);

        let red = argb_icon(32, 32, &rgba, Some(RECORDING_BADGE));
        // Coin haut gauche intact, centre de la pastille plein rouge.
        assert_eq!(pixel(&red, 0, 0), [0xff, 0x10, 0x20, 0xf0]);
        assert_eq!(pixel(&red, 24, 24), [0xff, 0xef, 0x44, 0x44]);
        // Anneau détouré entre l'icône et la pastille : transparent.
        let ring = pixel(&red, 16, 24);
        assert_eq!(ring[0], 0, "{ring:?}");

        let orange = argb_icon(32, 32, &rgba, Some(BUSY_BADGE));
        assert_eq!(pixel(&orange, 24, 24), [0xff, 0xf9, 0x73, 0x16]);
    }

    #[test]
    fn app_icons_decode_in_every_state() {
        assert_eq!(PIXMAPS.idle.len(), 2);
        assert_eq!(PIXMAPS.idle[1].width, 64);
        assert_ne!(PIXMAPS.idle[1].data, PIXMAPS.recording[1].data);
        assert_ne!(PIXMAPS.recording[1].data, PIXMAPS.busy[1].data);
    }
}
