//! Raccourcis globaux sous Windows : `RegisterHotKey` par le greffon des
//! raccourcis de Tauri (inscrit dans `lib.rs`, appelé d'ici seulement : la
//! page n'en a aucune permission). Windows ne fait choisir la touche à
//! personne : la page la saisit (`set_trigger`), elle est gardée dans les
//! réglages (`desktop_settings`), Ctrl+Alt+D et Ctrl+Alt+C par défaut.
//!
//! Appui et relâchement : Windows ne signale que l'appui (`WM_HOTKEY`, sans
//! répétition) ; le greffon guette ensuite le relâchement de la touche
//! principale toutes les 50 ms, d'où le push-to-talk.
//!
//! À chaque changement (ensemble voulu, combinaison, saisie ouverte ou
//! fermée), tout est désenregistré puis ce qui est voulu est enregistré de
//! nouveau : une combinaison qu'une autre application tient donne le statut
//! « Unavailable » (`Taken`), et « Try again » réessaie.
//!
//! La logique sans appel au système (contrôle d'une combinaison, statuts,
//! commandes) est dans `keys.rs`, essayée sur tous les systèmes.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use parking_lot::Mutex;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use super::keys::{self, Combo, Combos, KeysError, Outcome, ShortcutKeys};
use super::{Command, DICTATION_ID, PROOFREAD_ID, ShortcutStatus, Wanted, execute};
use crate::error::{AppError, AppResult};
use crate::i18n::{self, t};
use crate::store::Store;
use crate::store::dictation::{DictationMode, DictationSettings};

/// Une saisie laissée ouverte (page rechargée, onglet quitté sans le dire)
/// ne suspend pas les raccourcis plus longtemps.
const RECORDING_LIMIT: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Inner {
    /// Raccourcis à enregistrer.
    wanted: Wanted,
    combos: Combos,
    /// Résultat du dernier enregistrement de chacun (`None` : pas enregistré).
    dictation: Option<Outcome>,
    proofread: Option<Outcome>,
    /// Saisie d'une combinaison ouverte dans la page : rien n'est enregistré.
    recording: bool,
    /// Numéro de la saisie en cours (son délai ne ferme pas la suivante).
    recording_serial: u64,
    /// File des commandes, exécutées dans l'ordre par un fil à part (les
    /// rappels du greffon viennent du fil principal et de ses fils de guet).
    commands: Option<mpsc::Sender<Command>>,
}

/// Raccourcis globaux (dans `Desktop`).
#[derive(Default)]
pub(in crate::desktop) struct Shortcuts {
    inner: Mutex<Inner>,
    /// Mode push-to-talk de la dictée, relu à chaque appui.
    hold: AtomicBool,
    /// Un seul enregistrement à la fois. Jamais pris par les rappels du
    /// greffon, qui attendent le fil principal.
    registering: tokio::sync::Mutex<()>,
}

impl Shortcuts {
    fn status_of(&self, id: &str) -> ShortcutStatus {
        let inner = self.inner.lock();
        let outcome = if id == PROOFREAD_ID {
            &inner.proofread
        } else {
            &inner.dictation
        };
        keys::status(
            inner.wanted.contains(id),
            inner.combos.get(id),
            outcome.as_ref(),
            i18n::current(),
        )
    }

    /// Statut du raccourci de la dictée.
    pub(in crate::desktop) fn status(&self) -> ShortcutStatus {
        self.status_of(DICTATION_ID)
    }

    /// Statut du raccourci « Correct selected text ».
    pub(in crate::desktop) fn proofread_status(&self) -> ShortcutStatus {
        self.status_of(PROOFREAD_ID)
    }

    /// La touche se change dans ChatonLM : toujours possible.
    pub(in crate::desktop) fn can_configure(&self) -> bool {
        true
    }
}

fn shortcuts(app: &AppHandle) -> &Shortcuts {
    &app.state::<super::Desktop>().inner().shortcuts
}

/// Prévient la page et la zone de notification (elles relisent le statut).
fn announce(app: &AppHandle) {
    super::emit_status(app);
    super::tray::refresh(app);
}

/// Combinaison pour le greffon.
fn shortcut_of(combo: Combo) -> Result<Shortcut, String> {
    let code = Code::from_str(&combo.key.code_name())
        .map_err(|_| format!("unknown key {}", combo.key.code_name()))?;
    let mut modifiers = Modifiers::empty();
    for (held, modifier) in [
        (combo.ctrl, Modifiers::CONTROL),
        (combo.alt, Modifiers::ALT),
        (combo.shift, Modifiers::SHIFT),
        (combo.meta, Modifiers::SUPER),
    ] {
        if held {
            modifiers |= modifier;
        }
    }
    Ok(Shortcut::new(
        (!modifiers.is_empty()).then_some(modifiers),
        code,
    ))
}

/// File des commandes, créée au premier appui.
fn commands(app: &AppHandle) -> Option<mpsc::Sender<Command>> {
    let mut inner = shortcuts(app).inner.lock();
    if inner.commands.is_none() {
        let (sender, receiver) = mpsc::channel::<Command>();
        let handle = app.clone();
        let spawned = std::thread::Builder::new()
            .name("chatonlm-shortcuts".into())
            .spawn(move || {
                for command in receiver {
                    execute(&handle, command);
                }
            });
        match spawned {
            Ok(_) => inner.commands = Some(sender),
            Err(error) => eprintln!("chatonlm: cannot start the shortcut thread: {error}"),
        }
    }
    inner.commands.clone()
}

/// Appui ou relâchement signalé par le greffon pour le raccourci `id`.
fn on_event(app: &AppHandle, id: &'static str, state: ShortcutState) {
    let shortcuts = shortcuts(app);
    {
        let inner = shortcuts.inner.lock();
        if inner.recording || !inner.wanted.contains(id) {
            return;
        }
    }
    let hold = shortcuts.hold.load(Ordering::Relaxed);
    let pressed = state == ShortcutState::Pressed;
    if let Some(command) = keys::command_for(id, pressed, hold)
        && let Some(commands) = commands(app)
    {
        let _ = commands.send(command);
    }
}

/// Désenregistre tout, puis enregistre `targets` ; résultat de chacun.
/// Bloquant : le greffon attend le fil principal.
fn register(app: &AppHandle, targets: &[(&'static str, Combo)]) -> Vec<(&'static str, Outcome)> {
    let plugin = app.global_shortcut();
    if let Err(error) = plugin.unregister_all() {
        eprintln!("chatonlm: cannot unregister the global shortcuts: {error}");
    }
    targets
        .iter()
        .map(|&(id, combo)| {
            let result = shortcut_of(combo).and_then(|shortcut| {
                plugin
                    .on_shortcut(shortcut, move |app, _, event| {
                        on_event(app, id, event.state)
                    })
                    .map_err(|error| error.to_string())
            });
            if let Err(error) = &result {
                eprintln!("chatonlm: global shortcut {id} not set: {error}");
            }
            (id, Outcome::of(result))
        })
        .collect()
}

/// Met l'enregistrement auprès de Windows d'accord avec l'état voulu
/// (rien pendant une saisie), puis prévient la page.
async fn sync(app: &AppHandle) {
    let state = shortcuts(app);
    let _one_at_a_time = state.registering.lock().await;
    let targets: Vec<(&'static str, Combo)> = {
        let inner = state.inner.lock();
        if inner.recording {
            Vec::new()
        } else {
            inner
                .wanted
                .ids()
                .into_iter()
                .map(|id| (id, inner.combos.get(id)))
                .collect()
        }
    };
    let handle = app.clone();
    let outcomes = tauri::async_runtime::spawn_blocking(move || register(&handle, &targets)).await;
    let outcomes = match outcomes {
        Ok(outcomes) => outcomes,
        Err(error) => {
            eprintln!("chatonlm: global shortcuts not set: {error}");
            Vec::new()
        }
    };
    {
        let mut inner = state.inner.lock();
        inner.dictation = None;
        inner.proofread = None;
        for (id, outcome) in outcomes {
            if id == PROOFREAD_ID {
                inner.proofread = Some(outcome);
            } else {
                inner.dictation = Some(outcome);
            }
        }
    }
    announce(app);
}

fn spawn_sync(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { sync(&app).await });
}

fn set_hold(app: &AppHandle, hold: bool) {
    shortcuts(app).hold.store(hold, Ordering::Relaxed);
}

/// Change l'ensemble voulu ; vrai s'il a changé.
fn want(app: &AppHandle, change: impl FnOnce(&mut Wanted)) -> bool {
    let mut inner = shortcuts(app).inner.lock();
    let before = inner.wanted;
    change(&mut inner.wanted);
    inner.wanted != before
}

/// Démarrage : combinaisons lues dans les réglages, ensemble voulu
/// enregistré.
pub(in crate::desktop) fn init(app: &AppHandle, wanted: Wanted, hold: bool) {
    set_hold(app, hold);
    want(app, |current| *current = wanted);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(store) = app.try_state::<Store>().map(|store| store.inner().clone()) {
            match store
                .call(|connection| crate::store::desktop::shortcut_keys(connection))
                .await
            {
                Ok((dictation, proofread)) => {
                    shortcuts(&app).inner.lock().combos =
                        Combos::from_stored(dictation.as_deref(), proofread.as_deref());
                }
                Err(error) => eprintln!("chatonlm: cannot read the shortcut keys: {error}"),
            }
        }
        sync(&app).await;
    });
}

/// Enregistre le raccourci de la dictée si elle est active, le retire sinon.
/// Appelé après chaque changement des réglages de la dictée.
pub async fn apply(app: &AppHandle, settings: &DictationSettings) -> AppResult<()> {
    set_hold(app, settings.mode == DictationMode::Hold);
    if want(app, |wanted| wanted.dictation = settings.enabled) {
        sync(app).await;
    } else {
        super::tray::refresh(app);
    }
    Ok(())
}

/// Réglage `proofreadShortcut` : raccourci de la correction enregistré ou
/// retiré.
pub(in crate::desktop) fn apply_proofread(app: &AppHandle, enabled: bool) {
    if want(app, |wanted| wanted.proofread = enabled) {
        spawn_sync(app);
    }
}

/// Nouvel essai (page Dictation, après un échec) : tout est enregistré de
/// nouveau.
pub async fn bind_shortcut(app: &AppHandle) -> AppResult<()> {
    if !shortcuts(app).inner.lock().wanted.dictation {
        return Err(AppError::Invalid(t!("shortcut.turnOnFirst")));
    }
    sync(app).await;
    Ok(())
}

/// Comme [`bind_shortcut`], depuis l'onglet Keyboard shortcuts.
pub async fn bind_shortcuts(app: &AppHandle) -> AppResult<()> {
    if !shortcuts(app).inner.lock().wanted.any() {
        return Err(AppError::Invalid(t!("shortcut.noneWanted")));
    }
    sync(app).await;
    Ok(())
}

/// Rien à ouvrir : la touche se change dans la page (`set_trigger`).
pub async fn configure_shortcut(_app: &AppHandle) -> AppResult<()> {
    Ok(())
}

/// Identifiant connu, tel que gardé.
fn known(id: &str) -> AppResult<&'static str> {
    match id {
        DICTATION_ID => Ok(DICTATION_ID),
        PROOFREAD_ID => Ok(PROOFREAD_ID),
        // i18n-ignore
        _ => Err(AppError::Invalid(format!("unknown shortcut: {id}"))),
    }
}

/// Combinaison du raccourci `id` : celle saisie dans la page (`keys`), ou
/// celle par défaut (`None`). Gardée dans les réglages puis enregistrée ; la
/// saisie en cours est finie.
pub async fn set_trigger(app: &AppHandle, id: &str, keys: Option<ShortcutKeys>) -> AppResult<()> {
    let id = known(id)?;
    let combo = match keys {
        Some(keys) => Combo::from_keys(keys).map_err(|error| {
            AppError::Invalid(match error {
                KeysError::InvalidKey => t!("shortcut.invalidKey"),
                KeysError::NeedsModifier => t!("shortcut.needsModifier"),
            })
        })?,
        None => Combo::default_for(id),
    };
    if shortcuts(app).inner.lock().combos.other(id) == combo {
        return Err(AppError::Invalid(t!("shortcut.sameKeys")));
    }
    let store = app
        .try_state::<Store>()
        .map(|store| store.inner().clone())
        .ok_or_else(|| AppError::Invalid(t!("dictation.noDatabase")))?;
    let stored = keys.map(|_| combo.stored());
    store
        .call(move |connection| {
            crate::store::desktop::set_shortcut_keys(connection, id, stored.as_deref())
        })
        .await?;
    {
        let mut inner = shortcuts(app).inner.lock();
        inner.combos.set(id, combo);
        inner.recording = false;
    }
    sync(app).await;
    Ok(())
}

/// Saisie ouverte ou fermée dans la page : raccourcis suspendus le temps de
/// la saisie, [`RECORDING_LIMIT`] au plus.
pub async fn recording(app: &AppHandle, active: bool) {
    let serial = {
        let mut inner = shortcuts(app).inner.lock();
        if inner.recording == active {
            return;
        }
        inner.recording = active;
        inner.recording_serial += 1;
        inner.recording_serial
    };
    sync(app).await;
    if !active {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(RECORDING_LIMIT).await;
        let expired = {
            let mut inner = shortcuts(&app).inner.lock();
            let expired = inner.recording && inner.recording_serial == serial;
            if expired {
                inner.recording = false;
            }
            expired
        };
        if expired {
            sync(&app).await;
        }
    });
}
