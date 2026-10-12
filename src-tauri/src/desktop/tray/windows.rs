//! Zone de notification sous Windows : fonction `tray-icon` de Tauri
//! (`Shell_NotifyIcon`). L'icône existe toujours (pas de service à
//! attendre) ; après un redémarrage de l'explorateur, `tray-icon` la
//! réinscrit lui-même.
//!
//! Le menu s'ouvre au clic gauche comme au clic droit, comme celui de
//! l'officiel (`wintray/eventloop.go` : `WM_RBUTTONUP, WM_LBUTTONUP`). Un menu
//! natif n'a pas d'entrée masquée ni de rappel avant ouverture : il est
//! refait à chaque changement (phase de la dictée, réglages, langue, mise à
//! jour), avec les seules entrées visibles.

use parking_lot::Mutex;
use tauri::image::Image;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Wry};

use super::{
    Action, BUSY_BADGE, Entry, ICON_PNGS, RECORDING_BADGE, bound_trigger, describe,
    dictation_ready, entries, rgba_pixels, run,
};
use crate::dictation::{Dictation, DictationPhase};
use crate::i18n::{self, Lang};
use crate::updater::{self, TrayUpdate};

/// Ce que l'icône montre.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Shown {
    phase: DictationPhase,
    trigger: Option<String>,
    ready: bool,
    update: TrayUpdate,
    lang: Lang,
}

/// Icône et dernier état montré.
#[derive(Default)]
pub(in crate::desktop) struct TrayState {
    icon: Mutex<Option<TrayIcon>>,
    shown: Mutex<Option<Shown>>,
}

/// Pastille de la phase : rouge à l'écoute, orange pendant le traitement.
fn badge(phase: DictationPhase) -> Option<[u8; 3]> {
    match phase {
        DictationPhase::Idle => None,
        DictationPhase::Recording => Some(RECORDING_BADGE),
        DictationPhase::Transcribing | DictationPhase::Polishing => Some(BUSY_BADGE),
    }
}

/// Icône de l'app (32 px, que Windows réduit selon l'échelle de l'écran) avec
/// la pastille de `phase`.
fn icon(phase: DictationPhase) -> Option<Image<'static>> {
    let image = match Image::from_bytes(ICON_PNGS[0]) {
        Ok(image) => image,
        Err(error) => {
            eprintln!("chatonlm: cannot decode the tray icon: {error}");
            return None;
        }
    };
    let (width, height) = (image.width(), image.height());
    let pixels = rgba_pixels(width as usize, height as usize, image.rgba(), badge(phase));
    Some(Image::new_owned(pixels, width, height))
}

/// Infobulle : nom de l'app, état, touche du raccourci.
fn tooltip(shown: &Shown) -> String {
    format!(
        "ChatonLM — {}",
        describe(shown.phase, shown.trigger.as_deref(), shown.lang)
    )
}

/// Menu natif des entrées visibles.
fn menu(app: &AppHandle, shown: &Shown) -> tauri::Result<Menu<Wry>> {
    enum Item {
        Entry(MenuItem<Wry>),
        Separator(PredefinedMenuItem<Wry>),
    }
    let mut items = Vec::new();
    for entry in entries(shown.phase, shown.ready, shown.update, shown.lang) {
        match entry {
            None => items.push(Item::Separator(PredefinedMenuItem::separator(app)?)),
            Some(Entry { visible: false, .. }) => {}
            Some(entry) => items.push(Item::Entry(MenuItem::with_id(
                app,
                entry.action.id(),
                &entry.label,
                entry.enabled,
                None::<&str>,
            )?)),
        }
    }
    let items: Vec<&dyn IsMenuItem<Wry>> = items
        .iter()
        .map(|item| match item {
            Item::Entry(entry) => entry as &dyn IsMenuItem<Wry>,
            Item::Separator(separator) => separator as &dyn IsMenuItem<Wry>,
        })
        .collect();
    Menu::with_items(app, &items)
}

fn current(app: &AppHandle) -> Shown {
    Shown {
        phase: app.state::<Dictation>().phase(),
        trigger: bound_trigger(app),
        ready: dictation_ready(app),
        update: updater::tray_update(app),
        lang: i18n::current(),
    }
}

/// Montre `next` s'il diffère de ce qui est montré. Bloquant : les appels à
/// l'icône attendent le fil principal.
fn show(app: &AppHandle, next: Shown) {
    let state = &super::state(app).tray;
    // Tenu jusqu'au bout : deux rafraîchissements ne se croisent pas.
    let mut shown = state.shown.lock();
    if shown.as_ref() == Some(&next) {
        return;
    }
    let Some(tray) = state.icon.lock().clone() else {
        return;
    };
    let badge_changed = shown
        .as_ref()
        .is_none_or(|previous| badge(previous.phase) != badge(next.phase));
    let applied = (|| -> tauri::Result<()> {
        if badge_changed && let Some(icon) = icon(next.phase) {
            tray.set_icon(Some(icon))?;
        }
        tray.set_tooltip(Some(tooltip(&next)))?;
        tray.set_menu(Some(menu(app, &next)?))
    })();
    if let Err(error) = applied {
        eprintln!("chatonlm: cannot update the system tray icon: {error}");
    }
    *shown = Some(next);
}

/// Crée l'icône (fil principal, pendant le `setup` de l'app), puis la tient à
/// jour des phases de la dictée.
pub(in crate::desktop) fn spawn(app: &AppHandle) {
    let first = current(app);
    let built = menu(app, &first).and_then(|menu| {
        let mut builder = TrayIconBuilder::with_id("main")
            .tooltip(tooltip(&first))
            .menu(&menu)
            .show_menu_on_left_click(true)
            .on_menu_event(|app, event| {
                if let Some(action) = Action::from_id(event.id().as_ref()) {
                    run(app, action);
                }
            });
        if let Some(icon) = icon(first.phase) {
            builder = builder.icon(icon);
        }
        builder.build(app)
    });
    let tray = match built {
        Ok(tray) => tray,
        Err(error) => {
            // Sans icône, la fermeture de la fenêtre quitte l'app.
            eprintln!("chatonlm: cannot start the system tray icon: {error}");
            return;
        }
    };
    let state = super::state(app);
    // L'état montré d'abord : un rafraîchissement ne commence qu'une fois
    // l'icône notée, et il attend alors le fil principal (celui-ci) en
    // tenant le verrou de `shown`.
    *state.tray.shown.lock() = Some(first);
    *state.tray.icon.lock() = Some(tray);
    if state.set_tray_available(true) {
        super::emit_status(app);
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut phases = app.state::<Dictation>().subscribe();
        loop {
            phases.borrow_and_update();
            refresh(&app);
            if phases.changed().await.is_err() {
                break;
            }
        }
    });
}

/// Relit l'état de la dictée, la touche du raccourci, la langue de
/// l'interface et la mise à jour, et refait l'icône, l'infobulle et le menu
/// s'ils changent.
pub(in crate::desktop) fn refresh(app: &AppHandle) {
    if super::state(app).tray.icon.lock().is_none() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let next = current(&app);
        show(&app, next);
    });
}
