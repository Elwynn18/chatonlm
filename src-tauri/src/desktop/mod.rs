//! Intégration au bureau (étape 17) : raccourcis globaux de la dictée et de
//! la correction du texte sélectionné (portail XDG GlobalShortcuts sous
//! Linux, `RegisterHotKey` sous Windows), icône de la zone de notification
//! (StatusNotifierItem, `Shell_NotifyIcon`) et fermeture de la fenêtre qui la
//! masque, arguments `--dictate` et `--proofread`.
//!
//! Jamais de signal Unix pour piloter l'app (SIGUSR1 entre en conflit avec le
//! ramasse-miettes de JavaScriptCore, piège relevé chez Handy) : une seconde
//! instance lancée avec `--dictate` ou `--proofread` passe par l'instance
//! unique (D-Bus sous Linux, message `WM_COPYDATA` sous Windows).
//!
//! Ce qui dépend du système est dans `shortcut/`, `tray/` et `menu_entry/`
//! (`linux.rs`, `windows.rs`) ; ici, les quelques différences restantes sont
//! sous `cfg`.

pub mod menu_entry;
pub mod shortcut;
mod tray;

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, CloseRequestApi, Emitter, Manager, Runtime, Window};
use ts_rs::TS;

use crate::dictation::Dictation;
use crate::error::AppResult;
use crate::i18n::t;
use crate::store::Store;

pub use shortcut::{
    ShortcutKeys, apply, bind_shortcut, bind_shortcuts, configure_shortcut, recording,
    reset_trigger, set_trigger,
};

/// Argument qui bascule la dictée (commande à lier dans les raccourcis du
/// système quand le portail manque).
pub const DICTATE_ARG: &str = "--dictate";

/// Argument qui corrige le texte sélectionné (commande à lier dans les
/// raccourcis du système quand le portail manque).
pub const PROOFREAD_ARG: &str = "--proofread";

/// Événement envoyé à la page à chaque changement de [`ShortcutsStatus`].
pub const SHORTCUTS_EVENT: &str = "shortcuts-status";

/// Événement envoyé à la page pour l'amener sur une route (chaîne de chemin,
/// ex. « /settings?section=dictation »).
pub const NAVIGATE_EVENT: &str = "navigate";

/// Route des réglages de la dictée (entrée « Dictation settings… » du menu).
pub const DICTATION_SETTINGS_ROUTE: &str = "/settings?section=dictation";

/// Route de la page Proofread qui montre la dernière correction de la
/// sélection (action « Show » de sa notification).
pub const PROOFREAD_SELECTION_ROUTE: &str = "/proofread?selection=1";

const MAIN_WINDOW: &str = "main";

/// Raccourcis globaux (onglet Keyboard shortcuts des réglages).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ShortcutsStatus {
    /// Raccourci de la dictée (`Disabled` si la dictée est coupée).
    pub dictation: ShortcutStatus,
    /// Raccourci « Correct selected text » (`Disabled` si le réglage
    /// `proofreadShortcut` est coupé).
    pub proofread: ShortcutStatus,
    /// Le portail sait ouvrir la boîte de réglage des raccourcis (version 2) ;
    /// sous Windows, toujours vrai (la touche se change dans la page).
    pub can_configure: bool,
    /// Qui choisit les touches.
    pub chooser: ShortcutChooser,
    /// Commandes à lier soi-même dans les raccourcis du système (repli sans
    /// portail) : binaire, ou AppImage, suivi de « --dictate » ou
    /// « --proofread ».
    pub dictate_command: String,
    pub proofread_command: String,
}

/// Qui choisit les touches des raccourcis globaux.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ShortcutChooser {
    /// Le bureau, dans sa propre boîte (portail de Linux) : la page ne fait
    /// que l'ouvrir (`shortcuts_configure`).
    Desktop,
    /// ChatonLM (Windows) : la page fait saisir la combinaison
    /// (`shortcuts_set_trigger`).
    App,
}

/// État d'un raccourci global (mêmes variantes sur tous les systèmes, pour la
/// page ; `Binding` n'est produit que par le portail de Linux).
#[cfg_attr(windows, allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "state", rename_all = "camelCase")]
#[ts(export)]
pub enum ShortcutStatus {
    /// Fonction coupée (dictée, ou réglage du raccourci de correction) :
    /// rien n'est lié.
    Disabled,
    /// Liaison en cours (le bureau peut demander de choisir la touche).
    Binding,
    /// Lié ; `trigger` est la touche décrite par le bureau (« Ctrl+Alt+D »),
    /// vide si l'utilisateur n'en a choisi aucune.
    Bound { trigger: String },
    /// Pas de raccourci global : `problem` pour la page, `reason` à montrer
    /// (dans la langue de l'interface, refaite à chaque lecture du statut).
    Unavailable {
        problem: ShortcutProblem,
        reason: String,
        /// Détail technique d'un échec du bureau (en anglais), gardé pour
        /// refaire `reason` quand la langue change.
        #[serde(skip)]
        #[ts(skip)]
        detail: Option<String>,
    },
}

/// Pourquoi le raccourci global manque (mêmes variantes sur tous les
/// systèmes, pour la page : les quatre premières viennent du portail de
/// Linux, `Taken` de Windows).
#[cfg_attr(windows, allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ShortcutProblem {
    /// Le bureau n'a pas de portail des raccourcis globaux : le redemander
    /// ne sert à rien.
    NoPortal,
    /// L'utilisateur a refusé la boîte du bureau.
    Refused,
    /// Le bureau n'a pas pu lier le raccourci.
    Failed,
    /// Le bureau a cessé de le transmettre (connexion au bus perdue).
    Lost,
    /// Une autre application tient déjà cette combinaison (Windows).
    Taken,
}

/// Demande d'une seconde instance (ou du premier lancement).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    /// `chatonlm --dictate` : bascule de la dictée.
    Toggle,
    /// `chatonlm --proofread` : correction du texte sélectionné.
    Proofread,
    /// Relance sans argument : la fenêtre revient.
    Show,
}

/// Demandes reçues avant que l'app soit prête. Le nom D-Bus de l'instance
/// unique est pris avant le `setup` de l'app, et son rappel tourne sur un fil
/// de zbus : une relance peut arriver avant que la base et la fenêtre
/// existent. Elles attendent alors la fin du `setup`, dans l'ordre.
struct Gate<T> {
    /// `None` une fois l'app prête.
    pending: Mutex<Option<Vec<T>>>,
}

impl<T> Default for Gate<T> {
    fn default() -> Self {
        Self {
            pending: Mutex::new(Some(Vec::new())),
        }
    }
}

impl<T> Gate<T> {
    /// `Some(request)` : à exécuter tout de suite ; `None` : mise en attente.
    fn admit(&self, request: T) -> Option<T> {
        match self.pending.lock().as_mut() {
            Some(queue) => {
                queue.push(request);
                None
            }
            None => Some(request),
        }
    }

    /// L'app est prête : `first` puis les demandes en attente, dans l'ordre
    /// (sous le verrou : une demande arrivée pendant ce temps passe après).
    fn open(&self, first: impl IntoIterator<Item = T>, mut run: impl FnMut(T)) {
        let mut pending = self.pending.lock();
        let queued = pending.take().unwrap_or_default();
        first.into_iter().chain(queued).for_each(&mut run);
    }
}

/// État de l'intégration au bureau (géré par Tauri).
#[derive(Default)]
pub struct Desktop {
    tray_available: AtomicBool,
    /// Réglage « croix = quitter » (inverse de `close_to_tray`, pour que le
    /// défaut, avant la lecture de la base, reste « masquer »).
    close_quits: AtomicBool,
    shortcuts: shortcut::Shortcuts,
    tray: tray::TrayState,
    requests: Gate<Request>,
}

impl Desktop {
    pub fn tray_available(&self) -> bool {
        self.tray_available.load(Ordering::Relaxed)
    }

    pub fn can_configure_shortcut(&self) -> bool {
        self.shortcuts.can_configure()
    }

    /// Statut du raccourci de la dictée.
    pub fn shortcut_status(&self) -> ShortcutStatus {
        self.shortcuts.status()
    }

    /// Statut du raccourci « Correct selected text ».
    pub fn proofread_shortcut_status(&self) -> ShortcutStatus {
        self.shortcuts.proofread_status()
    }

    /// La croix masque la fenêtre (zone de notification présente et réglage
    /// « masquer ») ; sinon elle quitte l'app.
    pub fn close_hides(&self) -> bool {
        close_hides(
            self.tray_available(),
            !self.close_quits.load(Ordering::Relaxed),
        )
    }

    /// Réglage de la croix lu ou changé (`close_to_tray`).
    pub fn set_close_to_tray(&self, enabled: bool) {
        self.close_quits.store(!enabled, Ordering::Relaxed);
    }

    /// Vrai si la valeur a changé.
    fn set_tray_available(&self, available: bool) -> bool {
        self.tray_available.swap(available, Ordering::Relaxed) != available
    }
}

/// La croix masque la fenêtre seulement avec une zone de notification (sans
/// elle, rien pour la faire revenir) et le réglage « masquer ».
fn close_hides(tray_available: bool, close_to_tray: bool) -> bool {
    tray_available && close_to_tray
}

/// État géré ; `Desktop::default()` si `setup` n'a pas encore tourné.
pub fn state(app: &AppHandle) -> tauri::State<'_, Desktop> {
    app.state::<Desktop>()
}

/// Commande de repli à lier dans les raccourcis du système, le chemin du
/// programme entre guillemets s'il le faut (« /opt/Mes applications/… »).
pub fn dictate_command() -> String {
    command_with(DICTATE_ARG)
}

/// Comme [`dictate_command`], pour la correction de la sélection.
pub fn proofread_command() -> String {
    command_with(PROOFREAD_ARG)
}

#[cfg(not(windows))]
fn command_with(argument: &str) -> String {
    let program = std::env::var_os("APPIMAGE")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "chatonlm".into());
    format!("{} {argument}", shell_quote(&program))
}

/// Sous Windows : le programme entre guillemets doubles s'il le faut
/// (« C:\\Users\\Jean Dupont\\... »), pour un raccourci, AutoHotkey ou un
/// boîtier de touches.
#[cfg(windows)]
fn command_with(argument: &str) -> String {
    let program = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "chatonlm.exe".into());
    format!("{} {argument}", windows_quote(&program))
}

/// État des raccourcis globaux (raisons dans la langue de l'interface).
pub fn shortcuts_status(app: &AppHandle) -> ShortcutsStatus {
    let desktop = state(app);
    ShortcutsStatus {
        dictation: desktop.shortcut_status(),
        proofread: desktop.proofread_shortcut_status(),
        can_configure: desktop.can_configure_shortcut(),
        chooser: shortcut::chooser(),
        dictate_command: dictate_command(),
        proofread_command: proofread_command(),
    }
}

/// Réglage `proofreadShortcut` changé : le raccourci de correction est lié
/// avec celui de la dictée (nouvelle session du portail pour l'ensemble ;
/// l'ancienne, et le raccourci de la dictée, restent tant qu'elle n'a pas
/// répondu, et après un refus), ou délié. Rien si la valeur ne change pas
/// (un refus n'est pas redemandé).
pub async fn apply_proofread_shortcut(app: &AppHandle, enabled: bool) -> AppResult<()> {
    shortcut::apply_proofread(app, enabled);
    Ok(())
}

/// Mot tel que Windows le relit dans une ligne de commande : entre
/// guillemets doubles s'il est vide ou contient une espace (un chemin ne
/// contient jamais de guillemet).
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_quote(word: &str) -> String {
    if word.is_empty() || word.contains([' ', '\t']) {
        format!("\"{word}\"")
    } else {
        word.to_owned()
    }
}

/// Mot tel qu'un shell (et les raccourcis de KDE et GNOME, qui découpent la
/// commande de la même façon) le relit : tel quel s'il n'a que des caractères
/// sûrs, sinon entre apostrophes (une apostrophe devient `'\''`).
#[cfg_attr(windows, allow(dead_code))]
fn shell_quote(word: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "/._-+:=@%,".contains(c);
    if !word.is_empty() && word.chars().all(safe) {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// `--dictate` parmi les arguments (le premier, le programme, est ignoré).
pub fn wants_dictation<S: AsRef<str>>(args: &[S]) -> bool {
    has_argument(args, DICTATE_ARG)
}

fn has_argument<S: AsRef<str>>(args: &[S], argument: &str) -> bool {
    args.iter().skip(1).any(|arg| arg.as_ref() == argument)
}

/// Demandes portées par les arguments d'un lancement : bascule de la dictée
/// (`--dictate`), correction de la sélection (`--proofread`), les deux dans
/// cet ordre ; aucune si ni l'un ni l'autre.
fn shortcut_requests<S: AsRef<str>>(args: &[S]) -> Vec<Request> {
    let mut requests = Vec::new();
    if wants_dictation(args) {
        requests.push(Request::Toggle);
    }
    if has_argument(args, PROOFREAD_ARG) {
        requests.push(Request::Proofread);
    }
    requests
}

/// Demandes d'une relance : celles de ses arguments, sinon la fenêtre.
fn second_instance_requests<S: AsRef<str>>(args: &[S]) -> Vec<Request> {
    let requests = shortcut_requests(args);
    if requests.is_empty() {
        vec![Request::Show]
    } else {
        requests
    }
}

/// Demandes du premier lancement (`chatonlm --dictate`, `chatonlm
/// --proofread`), exécutées une fois l'app prête.
fn startup_requests() -> &'static [Request] {
    static REQUESTS: OnceLock<Vec<Request>> = OnceLock::new();
    REQUESTS.get_or_init(|| {
        let args: Vec<String> = std::env::args().collect();
        shortcut_requests(&args)
    })
}

/// Premier lancement par `chatonlm --dictate` ou `chatonlm --proofread` : la
/// fenêtre reste masquée (une relance la montre), la dictée bascule ou la
/// sélection est corrigée une fois l'app prête.
pub fn started_hidden() -> bool {
    !startup_requests().is_empty()
}

/// La page affiche la fenêtre une fois prête (`getCurrentWindow().show()`,
/// voir `src/lib/window.ts`). Lancée par `--dictate` ou `--proofread`, l'app refuse cet appel
/// (une permission « deny » l'emporte sur « allow ») : la fenêtre ne s'ouvre
/// que par le backend (relance, zone de notification), sans clignoter.
pub fn keep_main_window_hidden<R: Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<()> {
    app.add_capability(
        tauri::ipc::CapabilityBuilder::new("start-hidden")
            .window(MAIN_WINDOW)
            .permission("core:window:deny-show"),
    )
}

/// Montre la fenêtre principale et lui donne le focus.
pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Le clic sur l'icône masque la fenêtre seulement si elle est visible et au
/// premier plan. Sous Wayland, une fenêtre réduite reste « visible » et
/// `is_minimized` n'est jamais vrai (le compositeur ne le dit pas), mais elle
/// perd le focus : le clic la montre alors, comme une fenêtre cachée par
/// d'autres.
#[cfg(target_os = "linux")]
fn click_hides(visible: bool, focused: bool) -> bool {
    visible && focused
}

/// Clic sur l'icône de la zone de notification (Linux ; sous Windows le
/// clic ouvre le menu, comme celui de l'officiel).
#[cfg(target_os = "linux")]
fn toggle_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let visible = window.is_visible().unwrap_or(false);
    let focused = window.is_focused().unwrap_or(false);
    if click_hides(visible, focused) {
        end_voice_mode(app);
        let _ = window.hide();
    } else {
        show_main_window(app);
    }
}

/// Zone de notification disparue : une fenêtre masquée (fermée, ou masquée
/// par l'icône) serait introuvable ; elle revient. Linux seulement : sous
/// Windows, la zone de notification existe toujours.
#[cfg(target_os = "linux")]
fn reveal_main_window(app: &AppHandle) {
    let hidden = app
        .get_webview_window(MAIN_WINDOW)
        .is_some_and(|window| !window.is_visible().unwrap_or(true));
    if hidden {
        show_main_window(app);
    }
}

/// Fin de l'app (`ExitRequested`, après les autres arrêts) : le processus web
/// de WebKit de chaque fenêtre est arrêté tout de suite. Sinon il finit
/// seul après l'app, parfois 11 à 14 s plus tard (pilote NVIDIA : son fil de
/// rendu GPU, SkiaGPUWorker, tarde à se terminer). Rien n'y est perdu : les
/// données de la page (stockage local) sont tenues par le processus réseau
/// de WebKit.
///
/// Sous Windows, rien : les processus de WebView2 (`msedgewebview2.exe`)
/// finissent avec leur contrôleur, que wry ferme quand la fenêtre est
/// détruite, et au plus tard avec l'app, dont ils guettent la fin.
pub fn terminate_web_processes(app: &AppHandle) {
    for window in app.webview_windows().values() {
        terminate_web_process(window);
    }
}

/// Arrête le processus web de `window` (appelé sur le fil principal : la
/// demande s'exécute aussitôt). `terminate_web_process` (WebKitGTK 2.34) :
/// fonctionnalité `v2_38` de webkit2gtk, activée par wry.
#[cfg(target_os = "linux")]
fn terminate_web_process<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    let result = window.with_webview(|webview| {
        use webkit2gtk::WebViewExt;
        webview.inner().terminate_web_process();
    });
    if let Err(error) = result {
        eprintln!("chatonlm: cannot stop the web process: {error}");
    }
}

/// Hors de Linux : rien à arrêter (voir [`terminate_web_processes`]).
#[cfg(not(target_os = "linux"))]
fn terminate_web_process<R: Runtime>(_window: &tauri::WebviewWindow<R>) {}

/// Quitte l'app (Ctrl+Q, « Quit ChatonLM ») : `app.exit` ne passe pas par
/// `CloseRequested`, la taille de la fenêtre est donc mémorisée ici (sauf
/// fenêtre masquée : mémorisée à sa fermeture, ou jamais montrée).
pub fn quit(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW)
        && window.is_visible().unwrap_or(false)
    {
        crate::window::remember_size(&window.as_ref().window());
    }
    app.exit(0);
}

/// Montre la fenêtre puis demande à la page d'aller sur `route`.
pub(crate) fn open_route(app: &AppHandle, route: &str) {
    show_main_window(app);
    let _ = app.emit_to(MAIN_WINDOW, NAVIGATE_EVENT, route);
}

/// Rappel de l'instance unique : une relance avec `--dictate` bascule la
/// dictée, avec `--proofread` corrige la sélection, sans montrer la fenêtre ;
/// sans argument, la fenêtre revient. Avant la fin du `setup`, la demande
/// attend (voir [`Gate`]).
pub fn on_second_instance(app: &AppHandle, args: &[String]) {
    for request in second_instance_requests(args) {
        if let Some(request) = state(app).requests.admit(request) {
            execute(app, request);
        }
    }
}

fn execute(app: &AppHandle, request: Request) {
    match request {
        Request::Toggle => app.state::<Dictation>().toggle(app),
        Request::Proofread => crate::proofread::selection::correct_selection(app),
        Request::Show => show_main_window(app),
    }
}

/// Fenêtre masquée ou fermée : la séance du mode vocal finit (plus de page
/// visible pour la suivre).
fn end_voice_mode<R: Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(voice_mode) = app.try_state::<crate::voice_mode::VoiceMode>() {
        voice_mode.stop();
    }
}

/// Fermeture de la fenêtre : avec une zone de notification et le réglage
/// « masquer » (par défaut, Settings › General), elle n'est que masquée (la
/// dictée reste disponible) ; la première fois, une notification l'explique.
/// Sans zone de notification, ou réglage « quitter », l'app quitte (fin
/// commune : services arrêtés, modèles déchargés selon le réglage), et son
/// processus web s'arrête tout de suite (la fenêtre, détruite avant
/// `ExitRequested`, n'y serait plus : voir [`terminate_web_processes`]). Dans
/// les deux cas, la séance du mode vocal finit.
pub fn on_close_requested<R: Runtime>(window: &Window<R>, api: &CloseRequestApi) {
    if window.label() != MAIN_WINDOW {
        return;
    }
    let app = window.app_handle();
    end_voice_mode(app);
    let Some(desktop) = app.try_state::<Desktop>() else {
        return;
    };
    if !desktop.close_hides() {
        if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
            terminate_web_process(&window);
        }
        return;
    }
    api.prevent_close();
    let _ = window.hide();
    let Some(store) = app.try_state::<Store>().map(|store| store.inner().clone()) else {
        return;
    };
    let identifier = app.config().identifier.clone();
    // Icône écrite par les notifications de la dictée, si elle existe déjà.
    let icon = crate::paths::notification_icon(app)
        .ok()
        .filter(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned());
    tauri::async_runtime::spawn(async move {
        match store
            .call(|connection| crate::store::dictation::take_tray_hint(connection))
            .await
        {
            Ok(true) => notify_tray_hint(&identifier, icon.as_deref()).await,
            Ok(false) => {}
            Err(error) => eprintln!("chatonlm: cannot read the tray hint flag: {error}"),
        }
    });
}

/// Notification « ChatonLM is still running », montrée à la première
/// fermeture qui ne fait que masquer (sans serveur de notifications : rien,
/// l'erreur est journalisée).
#[cfg(target_os = "linux")]
async fn notify_tray_hint(identifier: &str, icon: Option<&str>) {
    let mut notification = notify_rust::Notification::new();
    notification
        .appname("ChatonLM")
        .summary(&t!("tray.hint.summary"))
        .body(&t!("tray.hint.body"))
        .icon(icon.unwrap_or(identifier))
        .hint(notify_rust::Hint::DesktopEntry(identifier.to_owned()));
    if let Err(error) = notification.show_async().await {
        eprintln!("chatonlm: cannot show the tray notification: {error}");
    }
}

/// Sous Windows : toast au nom de l'app (son icône est celle de son
/// identifiant, `icon` ne sert pas).
#[cfg(windows)]
async fn notify_tray_hint(identifier: &str, _icon: Option<&str>) {
    let identifier = identifier.to_owned();
    let shown = tauri::async_runtime::spawn_blocking(move || {
        crate::dictation::notify::show_simple(
            &identifier,
            &t!("tray.hint.summary"),
            &t!("tray.hint.body"),
        )
    })
    .await;
    match shown {
        Ok(Ok(())) => {}
        Ok(Err(error)) => eprintln!("chatonlm: cannot show the tray notification: {error}"),
        Err(error) => eprintln!("chatonlm: cannot show the tray notification: {error}"),
    }
}

/// Envoie à la page le statut de la dictée (zone de notification, raccourci)
/// et celui des raccourcis globaux.
fn emit_status(app: &AppHandle) {
    if let Some(dictation) = app.try_state::<Dictation>() {
        dictation.emit_status(app);
    }
    if app.try_state::<Desktop>().is_some() {
        let _ = app.emit(SHORTCUTS_EVENT, shortcuts_status(app));
    }
}

/// État des mises à jour changé (module updater) : menu de la zone de
/// notification refait (« An update is available », « Restart to update »).
pub fn update_changed(app: &AppHandle) {
    tray::refresh(app);
}

/// Langue de l'interface changée (`i18n::apply`) : menu et infobulle de la
/// zone de notification refaits, statuts renvoyés à la page (raison des
/// raccourcis dans la nouvelle langue).
pub fn language_changed(app: &AppHandle) {
    tray::refresh(app);
    emit_status(app);
}

/// Appelé à la fin du `setup` de l'app (base ouverte, états gérés) : zone de
/// notification, notifications de la correction de la sélection, entrée du
/// menu des applications d'une AppImage (avant les raccourcis : le portail
/// lit ce fichier), raccourcis voulus (dictée active, réglage
/// `proofreadShortcut`) liés en une fois, et demandes de `--dictate` ou
/// `--proofread` au premier lancement.
pub fn setup(app: &AppHandle) {
    tray::spawn(app);
    crate::proofread::selection::setup(app);

    if let Some(store) = app.try_state::<Store>().map(|store| store.inner().clone()) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            match store
                .call(|connection| {
                    Ok((
                        crate::store::dictation::get(connection)?,
                        crate::store::settings::get(connection, None)?,
                        crate::store::desktop::menu_entry(connection)?,
                        crate::store::desktop::close_to_tray(connection)?,
                    ))
                })
                .await
            {
                Ok((dictation, settings, menu_entry, close_to_tray)) => {
                    state(&app).set_close_to_tray(close_to_tray);
                    menu_entry::at_startup(&app, menu_entry).await;
                    let wanted = shortcut::Wanted {
                        dictation: dictation.enabled,
                        proofread: settings.proofread_shortcut,
                    };
                    let hold = dictation.mode == crate::store::dictation::DictationMode::Hold;
                    shortcut::init(&app, wanted, hold);
                }
                Err(error) => eprintln!("chatonlm: cannot read the shortcut settings: {error}"),
            }
        });
    }

    // Après le setup (la boucle d'évènements tourne, tous les états sont
    // gérés) : demandes du premier lancement par `--dictate` ou
    // `--proofread`, puis celles des relances arrivées entre-temps.
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        state(&handle)
            .requests
            .open(startup_requests().iter().copied(), |request| {
                execute(&handle, request)
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_close_button_hides_only_with_a_tray_and_the_setting() {
        assert!(close_hides(true, true));
        assert!(!close_hides(true, false));
        assert!(!close_hides(false, true));
        assert!(!close_hides(false, false));
        // Avant la lecture de la base : « masquer » (comportement d'avant).
        let desktop = Desktop::default();
        desktop.set_tray_available(true);
        assert!(desktop.close_hides());
        desktop.set_close_to_tray(false);
        assert!(!desktop.close_hides());
        desktop.set_close_to_tray(true);
        assert!(desktop.close_hides());
        desktop.set_tray_available(false);
        assert!(!desktop.close_hides());
    }

    #[test]
    fn dictate_argument_is_found_after_the_program() {
        assert!(wants_dictation(&["chatonlm", "--dictate"]));
        assert!(wants_dictation(&[
            "/usr/bin/chatonlm",
            "--foo",
            "--dictate"
        ]));
        assert!(!wants_dictation(&["chatonlm"]));
        assert!(!wants_dictation(&["--dictate"]));
        assert!(!wants_dictation(&["chatonlm", "--dictate=1", "dictate"]));
    }

    #[test]
    fn proofread_and_dictate_arguments_become_requests() {
        assert_eq!(shortcut_requests(&["chatonlm"]), []);
        assert_eq!(
            shortcut_requests(&["chatonlm", "--proofread"]),
            [Request::Proofread]
        );
        assert_eq!(
            shortcut_requests(&["chatonlm", "--proofread", "--dictate"]),
            [Request::Toggle, Request::Proofread]
        );
        assert_eq!(shortcut_requests(&["--proofread"]), []);
        assert_eq!(shortcut_requests(&["chatonlm", "--proofread=1"]), []);
        // Relance : sans argument connu, la fenêtre revient ; sinon rien
        // n'est montré.
        assert_eq!(second_instance_requests(&["chatonlm"]), [Request::Show]);
        assert_eq!(
            second_instance_requests(&["chatonlm", "--foo"]),
            [Request::Show]
        );
        assert_eq!(
            second_instance_requests(&["chatonlm", "--proofread"]),
            [Request::Proofread]
        );
        assert_eq!(
            second_instance_requests(&["chatonlm", "--dictate"]),
            [Request::Toggle]
        );
    }

    #[test]
    fn fallback_command_ends_with_the_argument() {
        assert!(dictate_command().ends_with(" --dictate"));
        assert!(proofread_command().ends_with(" --proofread"));
    }

    #[test]
    fn windows_program_paths_are_quoted_when_they_have_a_space() {
        assert_eq!(
            windows_quote(r"C:\Users\me\AppData\Local\chatonlm\chatonlm.exe"),
            r"C:\Users\me\AppData\Local\chatonlm\chatonlm.exe"
        );
        assert_eq!(
            windows_quote(r"C:\Users\Jean Dupont\AppData\Local\chatonlm\chatonlm.exe"),
            r#""C:\Users\Jean Dupont\AppData\Local\chatonlm\chatonlm.exe""#
        );
        assert_eq!(windows_quote(""), "\"\"");
    }

    #[test]
    fn program_paths_are_quoted_when_needed() {
        assert_eq!(shell_quote("/usr/bin/chatonlm"), "/usr/bin/chatonlm");
        assert_eq!(
            shell_quote("/opt/Mes applications/chatonlm/chatonlm"),
            "'/opt/Mes applications/chatonlm/chatonlm'"
        );
        assert_eq!(
            shell_quote("/home/me/Apps/L'app$(x).AppImage"),
            "'/home/me/Apps/L'\\''app$(x).AppImage'"
        );
        assert_eq!(shell_quote(""), "''");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_tray_click_hides_only_a_focused_window() {
        assert!(click_hides(true, true));
        // Réduite (Wayland : toujours « visible »), ou derrière d'autres.
        assert!(!click_hides(true, false));
        assert!(!click_hides(false, false));
    }

    #[test]
    fn early_requests_wait_for_the_setup_in_order() {
        let gate = Gate::default();
        assert_eq!(gate.admit(Request::Toggle), None);
        assert_eq!(gate.admit(Request::Proofread), None);
        assert_eq!(gate.admit(Request::Show), None);
        let mut ran = Vec::new();
        gate.open([Request::Proofread], |request| ran.push(request));
        assert_eq!(
            ran,
            [
                Request::Proofread,
                Request::Toggle,
                Request::Proofread,
                Request::Show
            ]
        );
        // Prête : tout de suite.
        assert_eq!(gate.admit(Request::Show), Some(Request::Show));
        let mut again = Vec::new();
        gate.open(None, |request| again.push(request));
        assert!(again.is_empty());
    }

    #[test]
    fn tray_availability_reports_changes_only() {
        let desktop = Desktop::default();
        assert!(desktop.set_tray_available(true));
        assert!(!desktop.set_tray_available(true));
        assert!(desktop.tray_available());
        assert!(desktop.set_tray_available(false));
    }
}
