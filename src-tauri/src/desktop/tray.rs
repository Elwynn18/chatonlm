//! Icône de la zone de notification : celle de l'app, avec une pastille rouge
//! pendant l'écoute et orange pendant la transcription et le nettoyage.
//! Menu : fenêtre, dictée, réglages de la dictée, mise à jour (comme
//! l'officiel, `app/wintray` : « An update is available » grisée et « Restart
//! to update » une fois prête ; ChatonLM : « An update is available » seule,
//! qui ouvre les réglages, tant qu'elle n'est pas téléchargée), quitter.
//!
//! - Linux (`linux.rs`) : StatusNotifierItem par ksni, en pur D-Bus ; clic
//!   gauche : fenêtre montrée ou masquée.
//! - Windows (`windows.rs`) : fonction `tray-icon` de Tauri ; le menu s'ouvre
//!   aux deux clics, comme celui de l'officiel (`wintray/eventloop.go`).
//!
//! Ici : ce que les deux partagent (entrées et textes du menu, infobulle,
//! actions, pastille). Textes dans la langue de l'interface, refaits quand
//! elle change (`i18n::apply` → [`refresh`]).

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as os;
#[cfg(windows)]
use windows as os;

use tauri::{AppHandle, Manager};

use super::{DICTATION_SETTINGS_ROUTE, ShortcutStatus, emit_status, state};
#[cfg(target_os = "linux")]
use super::{reveal_main_window, toggle_main_window};
use crate::dictation::{Dictation, DictationPhase};
use crate::i18n::{Lang, t};
use crate::store::Store;
use crate::updater::{self, TrayUpdate};

pub(super) use os::{TrayState, refresh, spawn};

/// Rouge de la pastille d'écoute (red-500 de Tailwind).
const RECORDING_BADGE: [u8; 3] = [0xef, 0x44, 0x44];
/// Orange de la pastille de traitement (orange-500 de Tailwind).
const BUSY_BADGE: [u8; 3] = [0xf9, 0x73, 0x16];

/// Tailles fournies au bureau, qui choisit la plus proche de son panneau.
const ICON_PNGS: [&[u8]; 2] = [
    include_bytes!("../../icons/32x32.png"),
    include_bytes!("../../icons/64x64.png"),
];

/// Part d'un pixel (centre en `x + 0.5`) couverte par un disque de rayon
/// `radius` à la distance `distance` : bord adouci sur un pixel.
fn coverage(distance: f32, radius: f32) -> f32 {
    (radius - distance + 0.5).clamp(0.0, 1.0)
}

/// Pixels ARGB32 (ordre réseau : A, R, G, B) d'une image RGBA, avec une
/// pastille ronde en bas à droite si `badge` est donné : détourée par un
/// anneau transparent pour rester lisible sur l'icône.
fn argb_pixels(width: usize, height: usize, rgba: &[u8], badge: Option<[u8; 3]>) -> Vec<u8> {
    let size = width.min(height) as f32;
    let radius = size * 0.22;
    let gap = (size * 0.06).max(1.0);
    let margin = size * 0.02;
    let center_x = width as f32 - margin - radius;
    let center_y = height as f32 - margin - radius;
    let mut data = Vec::with_capacity(width * height * 4);
    for (index, &[r, g, b, a]) in rgba.as_chunks::<4>().0.iter().enumerate() {
        let Some(color) = badge else {
            data.extend_from_slice(&[a, r, g, b]);
            continue;
        };
        let x = (index % width) as f32 + 0.5;
        let y = (index / width) as f32 + 0.5;
        let distance = ((x - center_x).powi(2) + (y - center_y).powi(2)).sqrt();
        // Anneau détouré, puis pastille par-dessus (alpha non prémultiplié).
        let below = f32::from(a) / 255.0 * (1.0 - coverage(distance, radius + gap));
        let fill = coverage(distance, radius);
        let alpha = fill + below * (1.0 - fill);
        let mix = |badge: u8, under: u8| -> u8 {
            if alpha <= 0.0 {
                return 0;
            }
            let value = (f32::from(badge) * fill + f32::from(under) * below * (1.0 - fill)) / alpha;
            value.round().clamp(0.0, 255.0) as u8
        };
        data.extend_from_slice(&[
            (alpha * 255.0).round() as u8,
            mix(color[0], r),
            mix(color[1], g),
            mix(color[2], b),
        ]);
    }
    data
}

/// Les mêmes pixels en RGBA (ordre attendu par l'icône de Tauri, sous
/// Windows).
#[cfg_attr(not(windows), allow(dead_code))]
fn rgba_pixels(width: usize, height: usize, rgba: &[u8], badge: Option<[u8; 3]>) -> Vec<u8> {
    let mut pixels = argb_pixels(width, height, rgba, badge);
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.rotate_left(1);
    }
    pixels
}

/// Texte de l'infobulle : état, et touche du raccourci si elle est connue.
fn describe(phase: DictationPhase, trigger: Option<&str>, lang: Lang) -> String {
    let state = match phase {
        DictationPhase::Idle => t!("tray.ready" in lang),
        DictationPhase::Recording => t!("tray.listening" in lang),
        DictationPhase::Transcribing => t!("tray.transcribing" in lang),
        DictationPhase::Polishing => t!("tray.polishing" in lang),
    };
    match trigger {
        Some(trigger) if !trigger.is_empty() => format!("{state} — {trigger}"),
        _ => state,
    }
}

/// Action d'une entrée du menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Show,
    Start,
    Stop,
    Cancel,
    Settings,
    /// Réglages, carte « Updates ».
    ShowUpdates,
    RestartToUpdate,
    Quit,
}

// Identifiants des entrées d'un menu natif (Windows).
#[cfg_attr(not(windows), allow(dead_code))]
impl Action {
    const ALL: [Action; 8] = [
        Action::Show,
        Action::Start,
        Action::Stop,
        Action::Cancel,
        Action::Settings,
        Action::ShowUpdates,
        Action::RestartToUpdate,
        Action::Quit,
    ];

    /// Identifiant de l'entrée (jamais montré, jamais traduit).
    fn id(self) -> &'static str {
        match self {
            Action::Show => "show",
            Action::Start => "start",
            Action::Stop => "stop",
            Action::Cancel => "cancel",
            Action::Settings => "settings",
            Action::ShowUpdates => "updates",
            Action::RestartToUpdate => "restart-to-update",
            Action::Quit => "quit",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.id() == id)
    }
}

/// Entrée du menu (`None` : séparateur).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    label: String,
    enabled: bool,
    visible: bool,
    action: Action,
}

/// Menu selon la phase et l'état de la dictée (`ready` : activée, avec un
/// modèle de transcription installé) et la mise à jour (`update`).
/// « Start dictation » et « Stop dictation » sont deux entrées, l'une
/// masquée : ksni rattache un clic au rappel actuel de l'entrée cliquée, et
/// un menu resté ouvert pendant que la dictée s'arrête ne doit pas en
/// relancer une (un arrêt sans dictée en cours ne fait rien). Les entrées de
/// la mise à jour viennent après celles de la dictée (même rang pour
/// celles-ci). Libellés dans la langue `lang`.
fn entries(
    phase: DictationPhase,
    ready: bool,
    update: TrayUpdate,
    lang: Lang,
) -> Vec<Option<Entry>> {
    let entry = |label, action| Entry {
        label,
        enabled: true,
        visible: true,
        action,
    };
    let recording = phase == DictationPhase::Recording;
    let mut menu = vec![
        Some(entry(t!("tray.show" in lang), Action::Show)),
        None,
        Some(Entry {
            enabled: ready && phase == DictationPhase::Idle,
            visible: !recording,
            ..entry(t!("tray.startDictation" in lang), Action::Start)
        }),
        Some(Entry {
            visible: recording,
            ..entry(t!("tray.stopDictation" in lang), Action::Stop)
        }),
        Some(Entry {
            visible: phase != DictationPhase::Idle,
            ..entry(t!("tray.cancelDictation" in lang), Action::Cancel)
        }),
        Some(entry(
            t!("tray.dictationSettings" in lang),
            Action::Settings,
        )),
        None,
    ];
    match update {
        TrayUpdate::None => {}
        TrayUpdate::Available => {
            menu.push(Some(entry(
                t!("tray.updateAvailable" in lang),
                Action::ShowUpdates,
            )));
            menu.push(None);
        }
        TrayUpdate::Ready | TrayUpdate::Installing => {
            menu.push(Some(Entry {
                enabled: false,
                ..entry(t!("tray.updateAvailable" in lang), Action::ShowUpdates)
            }));
            // Installation en cours (services arrêtés, boîte de mot de passe
            // ouverte) : grisée.
            menu.push(Some(Entry {
                enabled: update == TrayUpdate::Ready,
                ..entry(t!("tray.restartToUpdate" in lang), Action::RestartToUpdate)
            }));
            menu.push(None);
        }
    }
    menu.push(Some(entry(t!("tray.quit" in lang), Action::Quit)));
    menu
}

/// Exécute une action hors du fil de ksni (qui tient son verrou pendant le
/// rappel) : les appels à la fenêtre attendent la boucle principale.
fn run(app: &AppHandle, action: Action) {
    let app = app.clone();
    if action == Action::RestartToUpdate {
        // Refus avant l'arrêt des services (pas prête, dossier non
        // inscriptible, paquet gardé refusé) : noté par le moteur
        // (`lastError`, ou l'état d'erreur), et montré par la carte
        // « Updates », ouverte ici.
        tauri::async_runtime::spawn(async move {
            if let Err(error) = updater::restart_to_update(&app).await {
                eprintln!("chatonlm: update not installed: {error}");
                super::open_route(&app, updater::UPDATES_SETTINGS_ROUTE);
            }
        });
        return;
    }
    tauri::async_runtime::spawn_blocking(move || match action {
        Action::Show => super::show_main_window(&app),
        Action::Start => app.state::<Dictation>().start(&app),
        Action::Stop => app.state::<Dictation>().stop(&app),
        Action::Cancel => app.state::<Dictation>().cancel(&app),
        Action::Settings => super::open_route(&app, DICTATION_SETTINGS_ROUTE),
        Action::ShowUpdates => super::open_route(&app, updater::UPDATES_SETTINGS_ROUTE),
        Action::RestartToUpdate => {}
        Action::Quit => super::quit(&app),
    });
}

/// Dictée activée avec un modèle de transcription installé (entrée « Start
/// dictation » grisée sinon).
fn dictation_ready(app: &AppHandle) -> bool {
    let Some(store) = app.try_state::<Store>() else {
        return false;
    };
    let Ok(settings) = store.blocking(|connection| crate::store::dictation::get(connection)) else {
        return false;
    };
    settings.enabled && speech_model_installed(app, &settings.speech_model)
}

/// Tous les fichiers du modèle présents, à la bonne taille (comme la commande
/// `speech_models`).
fn speech_model_installed(app: &AppHandle, id: &str) -> bool {
    let Some(model) = crate::dictation::models::find(id) else {
        return false;
    };
    crate::paths::speech_models_dir(app)
        .is_ok_and(|root| crate::dictation::models::installed(&root, model))
}

/// Touche du raccourci, si elle est liée.
fn bound_trigger(app: &AppHandle) -> Option<String> {
    match super::state(app).shortcut_status() {
        ShortcutStatus::Bound { trigger } if !trigger.is_empty() => Some(trigger),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La CLI de Tauri lit les fonctionnalités de `tauri` dans le manifeste de
    /// l'app sans regarder la cible : `tray-icon`, même sous `cfg(windows)`,
    /// lui ferait exiger libappindicator pour les paquets Linux. Elle vient du
    /// crate `windows-tray`.
    #[test]
    fn the_app_manifest_leaves_the_tray_feature_to_its_windows_crate() {
        let code = |manifest: &str| {
            manifest
                .lines()
                .filter(|line| !line.trim_start().starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let app = code(include_str!("../../Cargo.toml"));
        assert!(
            !app.contains("tray-icon"),
            "tray-icon dans src-tauri/Cargo.toml"
        );
        assert!(app.contains(r#"windows-tray = { path = "crates/windows-tray" }"#));
        let windows = code(include_str!("../../crates/windows-tray/Cargo.toml"));
        let (_, target) = windows
            .split_once("[target.'cfg(windows)'.dependencies]")
            .expect("section Windows du crate windows-tray");
        assert!(target.contains(r#"features = ["tray-icon"]"#));
    }

    #[test]
    fn tooltip_names_the_phase_and_the_trigger() {
        let en = Lang::En;
        assert_eq!(describe(DictationPhase::Idle, None, en), "Ready");
        assert_eq!(describe(DictationPhase::Idle, Some(""), en), "Ready");
        assert_eq!(
            describe(DictationPhase::Recording, Some("Ctrl+Alt+D"), en),
            "Listening… — Ctrl+Alt+D"
        );
        assert_eq!(
            describe(DictationPhase::Transcribing, None, en),
            "Transcribing…"
        );
        assert_eq!(describe(DictationPhase::Polishing, None, en), "Polishing…");
        assert_eq!(
            describe(DictationPhase::Recording, Some("Ctrl+Alt+D"), Lang::Fr),
            "À l'écoute… — Ctrl+Alt+D"
        );
        assert_eq!(describe(DictationPhase::Idle, None, Lang::Es), "Listo");
    }

    #[test]
    fn menu_speaks_the_language_of_the_interface() {
        let labels = |phase, lang| -> Vec<(String, bool)> {
            entries(phase, true, TrayUpdate::None, lang)
                .into_iter()
                .flatten()
                .map(|entry| (entry.label, entry.visible))
                .collect()
        };
        let label = |text: &str, visible| (text.to_owned(), visible);
        assert_eq!(
            labels(DictationPhase::Idle, Lang::Fr),
            [
                label("Afficher ChatonLM", true),
                label("Commencer la dictée", true),
                label("Arrêter la dictée", false),
                label("Annuler la dictée", false),
                label("Paramètres de la dictée…", true),
                label("Quitter ChatonLM", true),
            ]
        );
        assert_eq!(
            labels(DictationPhase::Recording, Lang::Es),
            [
                label("Mostrar ChatonLM", true),
                label("Iniciar dictado", false),
                label("Detener dictado", true),
                label("Cancelar dictado", true),
                label("Configuración del dictado…", true),
                label("Salir de ChatonLM", true),
            ]
        );
    }

    fn find(menu: &[Option<Entry>], label: &str) -> Entry {
        menu.iter()
            .flatten()
            .find(|entry| entry.label == label)
            .cloned()
            .unwrap_or_else(|| panic!("{label} missing"))
    }

    #[test]
    fn native_menu_entries_are_found_by_their_identifier() {
        for action in Action::ALL {
            assert_eq!(Action::from_id(action.id()), Some(action));
        }
        assert_eq!(Action::from_id("Quit ChatonLM"), None);
        // Deux entrées « An update is available » et « Restart to update »
        // ne se confondent pas.
        assert_ne!(Action::ShowUpdates.id(), Action::RestartToUpdate.id());
    }

    #[test]
    fn rgba_pixels_are_the_argb_ones_reordered() {
        let rgba: Vec<u8> = [0x10, 0x20, 0xf0, 0xff].repeat(32 * 32);
        let plain = rgba_pixels(32, 32, &rgba, None);
        assert_eq!(plain, rgba);
        let red = rgba_pixels(32, 32, &rgba, Some(RECORDING_BADGE));
        let argb = argb_pixels(32, 32, &rgba, Some(RECORDING_BADGE));
        // Centre de la pastille : rouge opaque, dans l'ordre R, G, B, A.
        let center = (24 * 32 + 24) * 4;
        assert_eq!(red[center..center + 4], [0xef, 0x44, 0x44, 0xff]);
        assert_eq!(argb[center..center + 4], [0xff, 0xef, 0x44, 0x44]);
        assert_eq!(red.len(), argb.len());
    }

    #[test]
    fn menu_follows_the_phase() {
        let idle = entries(DictationPhase::Idle, true, TrayUpdate::None, Lang::En);
        let start = find(&idle, "Start dictation");
        assert!(start.enabled && start.visible && start.action == Action::Start);
        assert!(!find(&idle, "Stop dictation").visible);
        assert!(!find(&idle, "Cancel dictation").visible);
        assert_eq!(idle.iter().filter(|entry| entry.is_none()).count(), 2);
        assert_eq!(idle[0].as_ref().unwrap().label, "Show ChatonLM");
        assert_eq!(idle.last().unwrap().as_ref().unwrap().action, Action::Quit);

        let off = entries(DictationPhase::Idle, false, TrayUpdate::None, Lang::En);
        assert!(!find(&off, "Start dictation").enabled);
        assert!(find(&off, "Dictation settings…").enabled);

        let recording = entries(DictationPhase::Recording, true, TrayUpdate::None, Lang::En);
        assert!(!find(&recording, "Start dictation").visible);
        let stop = find(&recording, "Stop dictation");
        assert!(stop.enabled && stop.visible && stop.action == Action::Stop);
        assert!(find(&recording, "Cancel dictation").visible);

        let busy = entries(DictationPhase::Polishing, true, TrayUpdate::None, Lang::En);
        let start = find(&busy, "Start dictation");
        assert!(start.visible && !start.enabled);
        assert!(!find(&busy, "Stop dictation").visible);
        assert!(find(&busy, "Cancel dictation").visible);
    }

    #[test]
    fn start_and_stop_keep_their_place_in_the_menu() {
        // Même rang dans tous les états : un clic sur un menu périmé arrive
        // à l'entrée de même action (Stop sans dictée en cours : sans effet).
        let position = |phase, update, label| {
            entries(phase, true, update, Lang::En)
                .iter()
                .position(|entry| entry.as_ref().is_some_and(|e| e.label == label))
                .unwrap()
        };
        for phase in [
            DictationPhase::Idle,
            DictationPhase::Recording,
            DictationPhase::Transcribing,
        ] {
            for update in [
                TrayUpdate::None,
                TrayUpdate::Available,
                TrayUpdate::Ready,
                TrayUpdate::Installing,
            ] {
                assert_eq!(position(phase, update, "Start dictation"), 2);
                assert_eq!(position(phase, update, "Stop dictation"), 3);
            }
        }
    }

    #[test]
    fn the_update_entries_follow_the_update() {
        let labels = |update, lang| -> Vec<(String, bool, Option<Action>)> {
            entries(DictationPhase::Idle, true, update, lang)
                .into_iter()
                .skip(7)
                .map(|entry| match entry {
                    Some(entry) => (entry.label, entry.enabled, Some(entry.action)),
                    None => ("-".into(), true, None),
                })
                .collect()
        };
        let item = |text: &str, enabled, action| (text.to_owned(), enabled, action);
        let quit = item("Quit ChatonLM", true, Some(Action::Quit));
        let separator = item("-", true, None);
        assert_eq!(
            labels(TrayUpdate::None, Lang::En),
            std::slice::from_ref(&quit)
        );
        assert_eq!(
            labels(TrayUpdate::Available, Lang::En),
            [
                item("An update is available", true, Some(Action::ShowUpdates)),
                separator.clone(),
                quit.clone(),
            ]
        );
        // Comme l'officiel : l'annonce grisée, puis « Restart to update ».
        assert_eq!(
            labels(TrayUpdate::Ready, Lang::En),
            [
                item("An update is available", false, Some(Action::ShowUpdates)),
                item("Restart to update", true, Some(Action::RestartToUpdate)),
                separator.clone(),
                quit.clone(),
            ]
        );
        // Installation en cours : les deux grisées.
        assert_eq!(
            labels(TrayUpdate::Installing, Lang::En),
            [
                item("An update is available", false, Some(Action::ShowUpdates)),
                item("Restart to update", false, Some(Action::RestartToUpdate)),
                separator,
                quit,
            ]
        );
        let french: Vec<String> = labels(TrayUpdate::Ready, Lang::Fr)
            .into_iter()
            .map(|(label, _, _)| label)
            .collect();
        assert_eq!(
            french,
            [
                "Une mise à jour est disponible",
                "Redémarrer pour mettre à jour",
                "-",
                "Quitter ChatonLM"
            ]
        );
    }
}
