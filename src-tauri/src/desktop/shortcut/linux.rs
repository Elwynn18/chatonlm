//! Raccourcis globaux par le portail XDG GlobalShortcuts (KDE Plasma 6,
//! GNOME 48 et plus) : dictée (« CTRL+ALT+D » proposé) et correction du texte
//! sélectionné (« CTRL+ALT+C » proposé, propre à ChatonLM). Le bureau choisit
//! les touches dans sa propre boîte et signale l'appui (`Activated`) et le
//! relâchement (`Deactivated`), d'où la bascule et le push-to-talk de la
//! dictée ; un appui sur celui de la correction corrige la sélection.
//!
//! La spécification ne permet qu'un `BindShortcuts` par session : une seule
//! session lie l'ensemble voulu (dictée active, réglage `proofreadShortcut`),
//! et tout changement de cet ensemble ouvre une nouvelle session. L'ancienne
//! reste ouverte (ses raccourcis marchent encore) tant que la nouvelle n'a
//! pas répondu : fermée si la liaison réussit, gardée si l'utilisateur
//! refuse la boîte du bureau ou la laisse ouverte (seuls les raccourcis
//! nouveaux sont alors « Unavailable »). Seule la session liée la plus
//! récente exécute les appuis, et seulement les siens : un appui signalé aux
//! deux sessions pendant le recouvrement ne compte qu'une fois.
//!
//! Sans portail : statut « Unavailable », et les commandes
//! `chatonlm --dictate` et `chatonlm --proofread` à lier soi-même dans les
//! raccourcis du système.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ashpd::desktop::global_shortcuts::{
    Activated, BindShortcutsOptions, ConfigureShortcutsOptions, Deactivated, GlobalShortcuts,
    NewShortcut, Shortcut, ShortcutsChanged,
};
use ashpd::desktop::{CreateSessionOptions, ResponseError, Session};
use futures_util::StreamExt;
use parking_lot::Mutex;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Manager};

#[cfg(test)]
use super::reason;
use super::{
    Command, DICTATION_ID, PROOFREAD_ID, ShortcutProblem, ShortcutStatus, Wanted, execute,
    on_activated, on_deactivated, refreshed, unavailable_status,
};
use crate::error::{AppError, AppResult};
use crate::i18n::{self, Lang, t};
use crate::store::dictation::{DictationMode, DictationSettings};

/// Touche proposée (syntaxe de la spécification « shortcuts » de XDG).
pub const DICTATION_TRIGGER: &str = "CTRL+ALT+D";
/// Touche proposée pour la correction : libre dans les raccourcis par défaut
/// de Plasma 6 (mais « Insérer un commentaire » de LibreOffice Writer et
/// « Extract Constant » des IDE de JetBrains, raccourcis locaux, la perdent
/// une fois liée ; le bureau laisse en choisir une autre).
pub const PROOFREAD_TRIGGER: &str = "CTRL+ALT+C";

/// `CHATONLM_NO_GLOBAL_SHORTCUTS=1` : le portail n'est jamais contacté (statut
/// « portail absent », commande à lier soi-même). Pour la recette avant
/// l'empaquetage (sans fichier .desktop installé, KDE rangerait le raccourci
/// sous l'application qui a lancé ChatonLM) ou un portail qui se comporte mal.
pub const NO_PORTAL_ENV: &str = "CHATONLM_NO_GLOBAL_SHORTCUTS";

/// Valeur de [`NO_PORTAL_ENV`] qui coupe le portail : présente, non vide et
/// différente de « 0 ».
fn portal_disabled_by(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value != "0")
}

/// Au-delà, un nouvel `Activated` sans `Deactivated` entre-temps est un
/// nouvel appui (portail qui ne signale pas le relâchement) ; en deçà, une
/// répétition de la touche maintenue.
const REPEAT_WINDOW: Duration = Duration::from_millis(1500);

/// Portail et session (liée, ou en cours de liaison).
struct Bound {
    portal: GlobalShortcuts,
    session: Session<GlobalShortcuts>,
}

impl Wanted {
    /// Raccourcis demandés au portail : identifiant, description montrée par
    /// la boîte du bureau (dans la langue `lang`), touche proposée.
    fn requests(self, lang: Lang) -> Vec<(&'static str, String, &'static str)> {
        self.ids()
            .into_iter()
            .map(|id| match id {
                DICTATION_ID => (id, t!("shortcut.description" in lang), DICTATION_TRIGGER),
                _ => (
                    id,
                    t!("shortcut.proofreadDescription" in lang),
                    PROOFREAD_TRIGGER,
                ),
            })
            .collect()
    }
}

/// Statut de chaque raccourci.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Statuses {
    dictation: ShortcutStatus,
    proofread: ShortcutStatus,
}

impl Default for Statuses {
    fn default() -> Self {
        Self::each(Wanted::default(), |_| ShortcutStatus::Disabled)
    }
}

impl Statuses {
    /// `status` de chaque raccourci voulu, `Disabled` pour les autres.
    fn each(wanted: Wanted, status: impl Fn(&str) -> ShortcutStatus) -> Self {
        let of = |id: &str| {
            if wanted.contains(id) {
                status(id)
            } else {
                ShortcutStatus::Disabled
            }
        };
        Self {
            dictation: of(DICTATION_ID),
            proofread: of(PROOFREAD_ID),
        }
    }

    /// Touches changées par le bureau (`ShortcutsChanged`) : seuls les
    /// raccourcis voulus qui figurent dans le changement sont touchés.
    fn changed(&self, wanted: Wanted, triggers: &[(String, String)]) -> Self {
        let mut next = self.clone();
        for id in wanted.ids() {
            if let Some(trigger) = trigger_of(triggers, id) {
                *next.get_mut(id) = ShortcutStatus::Bound { trigger };
            }
        }
        next
    }

    fn get_mut(&mut self, id: &str) -> &mut ShortcutStatus {
        if id == PROOFREAD_ID {
            &mut self.proofread
        } else {
            &mut self.dictation
        }
    }

    /// Un raccourci voulu est lié sans touche.
    fn unset(&self, wanted: Wanted) -> bool {
        [
            (DICTATION_ID, &self.dictation),
            (PROOFREAD_ID, &self.proofread),
        ]
        .into_iter()
        .any(|(id, status)| {
            wanted.contains(id)
                && matches!(status, ShortcutStatus::Bound { trigger } if trigger.is_empty())
        })
    }
}

/// Touche décrite par le bureau pour le raccourci `id`, s'il figure dans
/// `triggers`.
fn trigger_of(triggers: &[(String, String)], id: &str) -> Option<String> {
    triggers
        .iter()
        .find(|(shortcut, _)| shortcut == id)
        .map(|(_, trigger)| trigger.clone())
}

/// Session liée encore ouverte : la liaison qui l'a ouverte, ce qu'elle
/// porte et ses touches.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Live {
    /// Génération de la liaison (et de la tâche qui écoute ses signaux).
    generation: u64,
    /// Raccourcis liés par cette session.
    carried: Wanted,
    /// Touche de chacun (identifiant, touche ; vide si aucune), changée par
    /// `ShortcutsChanged`.
    triggers: Vec<(String, String)>,
}

/// État des raccourcis. Statuts, génération et sessions changent sous le
/// même verrou : une tâche d'une liaison passée ne peut pas écraser les
/// statuts d'une liaison plus récente.
#[derive(Default)]
struct Inner {
    /// Raccourcis à lier.
    wanted: Wanted,
    /// Incrémenté à chaque liaison ou déliaison : une tâche d'une génération
    /// passée n'écrit plus rien.
    generation: u64,
    statuses: Statuses,
    /// Le portail de la session liée sait ouvrir sa boîte de réglage
    /// (version 2).
    can_configure: bool,
    /// Liaison en cours (génération `generation`) : sa tâche, puis sa
    /// session une fois créée (fermée si une autre liaison la remplace avant
    /// sa réponse, ce qui ferme aussi la boîte du bureau).
    pending: Option<JoinHandle<()>>,
    pending_session: Option<Arc<Bound>>,
    /// Dernière liaison réussie : gardée, avec ses signaux, tant qu'une
    /// nouvelle liaison n'a pas réussi.
    live: Option<Live>,
    live_session: Option<Arc<Bound>>,
    live_task: Option<JoinHandle<()>>,
    /// Appuis (répétitions filtrées), communs aux sessions successives.
    presses: Presses,
}

impl Inner {
    /// Statuts : raccourcis voulus que la session liée porte → `Bound` (sa
    /// touche) ; autres voulus → `other` ; hors de l'ensemble → `Disabled`.
    fn derive(&self, other: &ShortcutStatus) -> Statuses {
        Statuses::each(self.wanted, |id| match &self.live {
            Some(live) if live.carried.contains(id) => ShortcutStatus::Bound {
                trigger: trigger_of(&live.triggers, id).unwrap_or_default(),
            },
            _ => other.clone(),
        })
    }

    /// Nouvel ensemble voulu : la liaison en cours est abandonnée (sa session
    /// fermée), la session liée est gardée jusqu'à la réponse de la
    /// suivante. Renvoie la génération de la liaison à faire, ou `None` :
    /// ensemble vide (tout est fermé), ou exactement celui de la session liée
    /// (rien à relier, sauf `force` : nouvel essai demandé).
    fn restart(&mut self, wanted: Wanted, force: bool) -> Option<u64> {
        self.wanted = wanted;
        self.generation += 1;
        if let Some(task) = self.pending.take() {
            task.abort();
        }
        close(self.pending_session.take());
        if !wanted.any() {
            self.live = None;
            if let Some(task) = self.live_task.take() {
                task.abort();
            }
            close(self.live_session.take());
            self.can_configure = false;
            self.statuses = Statuses::default();
            return None;
        }
        self.statuses = self.derive(&ShortcutStatus::Binding);
        let carried_already = self
            .live
            .as_ref()
            .is_some_and(|live| live.carried == wanted);
        (force || !carried_already).then_some(self.generation)
    }

    /// Ensemble voulu après `change`, s'il diffère de l'actuel (nouvelle
    /// liaison à faire) ; `None` sinon (un refus déjà reçu n'est pas
    /// redemandé).
    fn change(&self, change: impl FnOnce(&mut Wanted)) -> Option<Wanted> {
        let mut wanted = self.wanted;
        change(&mut wanted);
        (wanted != self.wanted).then_some(wanted)
    }

    /// Liaison `generation` de `carried` réussie : elle devient la session
    /// liée, l'ancienne est fermée (sa tâche arrêtée). Faux si une liaison
    /// plus récente l'a remplacée entre-temps.
    fn bound(
        &mut self,
        generation: u64,
        carried: Wanted,
        triggers: Vec<(String, String)>,
        can_configure: bool,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        if let Some(task) = self.live_task.take() {
            task.abort();
        }
        close(self.live_session.take());
        self.live_task = self.pending.take();
        self.live_session = self.pending_session.take();
        self.live = Some(Live {
            generation,
            carried,
            triggers,
        });
        self.can_configure = can_configure;
        self.statuses = self.derive(&ShortcutStatus::Binding);
        true
    }

    /// Liaison `generation` refusée ou échouée (`status`) : sa session est
    /// fermée, la session liée reste ; les raccourcis qu'elle porte gardent
    /// leur touche, les autres voulus prennent `status`. Faux si la liaison
    /// n'est plus la plus récente.
    fn failed(&mut self, generation: u64, status: &ShortcutStatus) -> bool {
        if generation != self.generation {
            return false;
        }
        // La tâche se termine d'elle-même.
        self.pending = None;
        close(self.pending_session.take());
        self.statuses = self.derive(status);
        true
    }

    /// Signaux de la session liée `generation` perdus (connexion au bus) :
    /// ses raccourcis deviennent « Unavailable ». Faux si elle n'était plus
    /// la session liée.
    fn lost(&mut self, generation: u64) -> bool {
        let Some(live) = self.live.take_if(|live| live.generation == generation) else {
            return false;
        };
        self.live_task = None;
        close(self.live_session.take());
        self.can_configure = false;
        let lost = unavailable_status(ShortcutProblem::Lost, None);
        for id in self.wanted.and(live.carried).ids() {
            *self.statuses.get_mut(id) = lost.clone();
        }
        true
    }

    /// Touches changées par le bureau pour la session liée `generation` :
    /// vrai si un statut a changé.
    fn changed(&mut self, generation: u64, triggers: &[(String, String)]) -> bool {
        let wanted = self.wanted;
        let Some(live) = self
            .live
            .as_mut()
            .filter(|live| live.generation == generation)
        else {
            return false;
        };
        for id in live.carried.ids() {
            if let Some(trigger) = trigger_of(triggers, id) {
                live.triggers.retain(|(shortcut, _)| shortcut != id);
                live.triggers.push((id.to_owned(), trigger));
            }
        }
        let next = self.statuses.changed(wanted.and(live.carried), triggers);
        let changed = next != self.statuses;
        self.statuses = next;
        changed
    }

    /// Appui ou relâchement signalé à la session de la liaison `generation` :
    /// exécuté seulement par la session liée, pour un raccourci qu'elle
    /// porte et toujours voulu (un seul exécutant par raccourci, même quand
    /// le bureau signale l'appui aux deux sessions d'un recouvrement).
    fn press(
        &mut self,
        generation: u64,
        signal: &Signal,
        hold: bool,
        now: Instant,
    ) -> Option<Command> {
        let live = self
            .live
            .as_ref()
            .filter(|live| live.generation == generation)?;
        let active = self.wanted.and(live.carried);
        self.presses.handle(signal, active, hold, now)
    }
}

/// Raccourcis globaux (dans `Desktop`).
#[derive(Default)]
pub(in crate::desktop) struct Shortcuts {
    inner: Mutex<Inner>,
    /// Mode push-to-talk de la dictée, relu à chaque appui (mis à jour par
    /// `apply`).
    hold: AtomicBool,
    /// `register_host_app` déjà tenté (une fois par connexion D-Bus).
    registered: AtomicBool,
}

impl Shortcuts {
    /// Statut du raccourci de la dictée.
    pub(in crate::desktop) fn status(&self) -> ShortcutStatus {
        refreshed(self.inner.lock().statuses.dictation.clone())
    }

    /// Statut du raccourci « Correct selected text ».
    pub(in crate::desktop) fn proofread_status(&self) -> ShortcutStatus {
        refreshed(self.inner.lock().statuses.proofread.clone())
    }

    pub(in crate::desktop) fn can_configure(&self) -> bool {
        self.inner.lock().can_configure
    }
}

/// Distingue un nouvel appui d'une répétition de la touche maintenue.
#[derive(Debug, Default)]
struct PressFilter {
    held_since: Option<Instant>,
}

impl PressFilter {
    /// Vrai pour un nouvel appui.
    fn press(&mut self, now: Instant) -> bool {
        let repeat = self
            .held_since
            .is_some_and(|last| now.duration_since(last) < REPEAT_WINDOW);
        self.held_since = Some(now);
        !repeat
    }

    fn release(&mut self) {
        self.held_since = None;
    }
}

/// Signal du portail, avec la session qu'il concerne. Les trois arrivent
/// par un seul flux, dans l'ordre où le portail les a émis : un relâchement
/// n'est jamais traité avant l'appui qui le précède (deux flux lus par
/// `select!` ne le garantissaient pas).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Signal {
    Activated {
        session: String,
        id: String,
    },
    Deactivated {
        session: String,
        id: String,
    },
    /// Raccourcis du changement et leur touche (identifiant, touche).
    Changed {
        session: String,
        triggers: Vec<(String, String)>,
    },
}

impl Signal {
    /// Signal de la session `ours` ; tous si son chemin est inconnu. Les
    /// flux de deux sessions ouvertes en même temps (recouvrement d'une
    /// nouvelle liaison) reçoivent les signaux des deux.
    fn is_for(&self, ours: Option<&str>) -> bool {
        let session = match self {
            Signal::Activated { session, .. }
            | Signal::Deactivated { session, .. }
            | Signal::Changed { session, .. } => session,
        };
        ours.is_none_or(|ours| ours == session)
    }
}

fn decode(message: &zbus::Message) -> Option<Signal> {
    let header = message.header();
    let body = message.body();
    match header.member()?.as_str() {
        "Activated" => body
            .deserialize::<Activated>()
            .ok()
            .map(|signal| Signal::Activated {
                session: signal.session_handle().to_string(),
                id: signal.shortcut_id().to_owned(),
            }),
        "Deactivated" => body
            .deserialize::<Deactivated>()
            .ok()
            .map(|signal| Signal::Deactivated {
                session: signal.session_handle().to_string(),
                id: signal.shortcut_id().to_owned(),
            }),
        "ShortcutsChanged" => {
            body.deserialize::<ShortcutsChanged>()
                .ok()
                .map(|signal| Signal::Changed {
                    session: signal.session_handle().to_string(),
                    triggers: triggers(signal.shortcuts()),
                })
        }
        _ => None,
    }
}

/// Chemin D-Bus de la session (ashpd ne le donne que par sa sérialisation,
/// celle d'un `ObjectPath` : une chaîne).
fn session_path(session: &Session<GlobalShortcuts>) -> Option<String> {
    match serde_json::to_value(session).ok()? {
        serde_json::Value::String(path) => Some(path),
        _ => None,
    }
}

/// Appuis et relâchements de nos raccourcis → commandes. Les répétitions de
/// la touche maintenue sont filtrées pour chacun ; un raccourci hors de
/// `active` (ensemble lié et voulu) est ignoré. Push-to-talk pour la dictée
/// seulement.
#[derive(Debug, Default)]
struct Presses {
    dictation: PressFilter,
    proofread: PressFilter,
}

impl Presses {
    fn handle(
        &mut self,
        signal: &Signal,
        active: Wanted,
        hold: bool,
        now: Instant,
    ) -> Option<Command> {
        match signal {
            Signal::Activated { id, .. } if !active.contains(id) => None,
            Signal::Deactivated { id, .. } if !active.contains(id) => None,
            Signal::Activated { id, .. } if id == DICTATION_ID => {
                self.dictation.press(now).then(|| on_activated(hold))
            }
            Signal::Deactivated { id, .. } if id == DICTATION_ID => {
                self.dictation.release();
                on_deactivated(hold)
            }
            Signal::Activated { .. } => self.proofread.press(now).then_some(Command::Proofread),
            Signal::Deactivated { .. } => {
                self.proofread.release();
                None
            }
            Signal::Changed { .. } => None,
        }
    }
}

fn shortcuts(app: &AppHandle) -> &Shortcuts {
    &app.state::<super::Desktop>().inner().shortcuts
}

/// Prévient la page et la zone de notification (elles relisent le statut
/// courant : l'ordre des envois importe peu).
fn announce(app: &AppHandle) {
    super::emit_status(app);
    super::tray::refresh(app);
}

/// Lie le raccourci de la dictée si elle est active, le délie sinon. Appelé
/// après chaque changement des réglages de la dictée ; un refus déjà reçu
/// n'est pas redemandé (seul `bind_shortcut` réessaie).
pub async fn apply(app: &AppHandle, settings: &DictationSettings) -> AppResult<()> {
    set_hold(app, settings.mode == DictationMode::Hold);
    want(app, |wanted| wanted.dictation = settings.enabled);
    super::tray::refresh(app);
    Ok(())
}

/// Réglage `proofreadShortcut` : raccourci de la correction lié ou délié.
pub(in crate::desktop) fn apply_proofread(app: &AppHandle, enabled: bool) {
    want(app, |wanted| wanted.proofread = enabled);
}

/// Démarrage : ensemble voulu lu dans les réglages, lié en une fois.
pub(in crate::desktop) fn init(app: &AppHandle, wanted: Wanted, hold: bool) {
    set_hold(app, hold);
    want(app, |current| *current = wanted);
    super::tray::refresh(app);
}

fn set_hold(app: &AppHandle, hold: bool) {
    shortcuts(app).hold.store(hold, Ordering::Relaxed);
}

/// Ensemble voulu changé par `change` : nouvelle liaison de tout l'ensemble
/// (ou déliaison s'il est vide ; rien à relier s'il redevient celui de la
/// session liée) ; rien s'il ne change pas.
fn want(app: &AppHandle, change: impl FnOnce(&mut Wanted)) {
    {
        let mut inner = shortcuts(app).inner.lock();
        let Some(wanted) = inner.change(change) else {
            return;
        };
        start_locked(app, &mut inner, wanted, false, false);
    }
    announce(app);
}

/// Nouvel essai de liaison (page Dictation, après un refus ou une erreur) :
/// tout l'ensemble voulu est relié. KDE ne redemande pas la touche d'un
/// raccourci déjà refusé : il le lie aussitôt, sans touche ; la page de
/// réglage du bureau s'ouvre alors (portail version 2).
pub async fn bind_shortcut(app: &AppHandle) -> AppResult<()> {
    if !shortcuts(app).inner.lock().wanted.dictation {
        return Err(AppError::Invalid(t!("shortcut.turnOnFirst")));
    }
    restart_binding(app);
    Ok(())
}

/// Comme [`bind_shortcut`], depuis l'onglet Keyboard shortcuts : erreur si
/// aucun raccourci global n'est demandé.
pub async fn bind_shortcuts(app: &AppHandle) -> AppResult<()> {
    if !shortcuts(app).inner.lock().wanted.any() {
        return Err(AppError::Invalid(t!("shortcut.noneWanted")));
    }
    restart_binding(app);
    Ok(())
}

/// Relie l'ensemble voulu, page de réglage du bureau ouverte si une touche
/// revient vide.
fn restart_binding(app: &AppHandle) {
    {
        let mut inner = shortcuts(app).inner.lock();
        let wanted = inner.wanted;
        start_locked(app, &mut inner, wanted, true, true);
    }
    announce(app);
}

/// Ouvre la boîte du bureau qui change la touche (portail version 2).
pub async fn configure_shortcut(app: &AppHandle) -> AppResult<()> {
    let bound = shortcuts(app).inner.lock().live_session.clone();
    let Some(bound) = bound else {
        return Err(AppError::Invalid(t!("shortcut.notSet")));
    };
    bound
        .portal
        .configure_shortcuts(&bound.session, None, ConfigureShortcutsOptions::default())
        .await
        .map_err(|error| match error {
            ashpd::Error::RequiresVersion(..) => AppError::System(t!("shortcut.cannotConfigure")),
            error => AppError::System(t!("shortcut.settingsFailed", error = error)),
        })
}

/// Ferme la session en arrière-plan (la boîte du bureau éventuellement
/// ouverte se ferme avec elle).
fn close(bound: Option<Arc<Bound>>) {
    if let Some(bound) = bound {
        tauri::async_runtime::spawn(async move {
            let _ = bound.session.close().await;
        });
    }
}

/// Arrête la liaison en cours et, s'il le faut (voir [`Inner::restart`]),
/// en commence une nouvelle de `wanted` (tâche de fond : la boîte du bureau
/// peut rester ouverte longtemps), sous le verrou de l'état (l'appelant
/// prévient ensuite). `force` : relier même un ensemble déjà lié ;
/// `configure_if_unset` : ouvrir la page de réglage du bureau si une touche
/// revient vide.
fn start_locked(
    app: &AppHandle,
    inner: &mut Inner,
    wanted: Wanted,
    force: bool,
    configure_if_unset: bool,
) {
    let Some(generation) = inner.restart(wanted, force) else {
        return;
    };
    let handle = app.clone();
    inner.pending = Some(tauri::async_runtime::spawn(async move {
        run(&handle, generation, wanted, configure_if_unset).await;
    }));
}

/// Portail absent (frontal ou interface manquants) plutôt qu'une erreur.
fn is_missing_portal(error: &ashpd::Error) -> bool {
    if matches!(error, ashpd::Error::PortalNotFound(_)) {
        return true;
    }
    let text = format!("{error:?}");
    [
        "ServiceUnknown",
        "NameHasNoOwner",
        "UnknownInterface",
        "UnknownMethod",
    ]
    .iter()
    .any(|name| text.contains(name))
}

fn unavailable(error: &ashpd::Error) -> ShortcutStatus {
    let (problem, detail) = if is_missing_portal(error) {
        (ShortcutProblem::NoPortal, None)
    } else {
        match error {
            ashpd::Error::Response(ResponseError::Cancelled) => (ShortcutProblem::Refused, None),
            ashpd::Error::Response(ResponseError::Other) => (ShortcutProblem::Failed, None),
            error => (ShortcutProblem::Failed, Some(error.to_string())),
        }
    };
    unavailable_status(problem, detail)
}

/// Touches décrites par le bureau (identifiant, touche ; vide si aucune).
fn triggers(shortcuts: &[Shortcut]) -> Vec<(String, String)> {
    shortcuts
        .iter()
        .map(|shortcut| {
            (
                shortcut.id().to_owned(),
                shortcut.trigger_description().to_owned(),
            )
        })
        .collect()
}

/// Liaison puis écoute des signaux, jusqu'à l'arrêt de la tâche. Refus ou
/// échec : les raccourcis voulus que la session liée ne porte pas deviennent
/// « Unavailable » (elle reste ouverte).
async fn run(app: &AppHandle, generation: u64, wanted: Wanted, configure_if_unset: bool) {
    if let Err(status) = bind_and_listen(app, generation, wanted, configure_if_unset).await {
        let changed = shortcuts(app).inner.lock().failed(generation, &status);
        if changed {
            announce(app);
        }
    }
}

async fn bind_and_listen(
    app: &AppHandle,
    generation: u64,
    wanted: Wanted,
    configure_if_unset: bool,
) -> Result<(), ShortcutStatus> {
    if portal_disabled_by(std::env::var_os(NO_PORTAL_ENV).as_deref()) {
        return Err(unavailable_status(ShortcutProblem::NoPortal, None));
    }
    let state = shortcuts(app);
    // Application hors bac à sable : le portail ne connaît pas son
    // identifiant sans cet appel, à faire avant tout autre (portail ancien :
    // erreur ignorée).
    if !state.registered.swap(true, Ordering::Relaxed) {
        match app.config().identifier.parse::<ashpd::AppID>() {
            Ok(app_id) => {
                if let Err(error) = ashpd::register_host_app(app_id).await {
                    eprintln!("chatonlm: portal registration failed: {error}");
                }
            }
            Err(error) => eprintln!("chatonlm: invalid application id: {error}"),
        }
    }

    let portal = GlobalShortcuts::new()
        .await
        .map_err(|error| unavailable(&error))?;
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await
        .map_err(|error| unavailable(&error))?;
    let can_configure = portal.version() >= 2;
    let bound = Arc::new(Bound { portal, session });
    {
        let mut inner = state.inner.lock();
        if inner.generation != generation {
            drop(inner);
            close(Some(bound));
            return Ok(());
        }
        inner.pending_session = Some(Arc::clone(&bound));
    }

    // Descriptions montrées par la boîte du bureau, dans la langue de
    // l'interface au moment de la liaison.
    let requested: Vec<NewShortcut> = wanted
        .requests(i18n::current())
        .into_iter()
        .map(|(id, description, trigger)| {
            NewShortcut::new(id, description).preferred_trigger(trigger)
        })
        .collect();
    let response = bound
        .portal
        .bind_shortcuts(
            &bound.session,
            &requested,
            None,
            BindShortcutsOptions::default(),
        )
        .await
        .and_then(|request| request.response())
        .map_err(|error| unavailable(&error))?;
    // Abonné après la réponse, avant de remplacer l'ancienne session : un
    // appui signalé à cette session pendant que la boîte restait ouverte
    // (l'ancienne l'a déjà exécuté) n'attend pas dans le flux. Un seul flux
    // pour les trois signaux de l'interface, dans leur ordre d'émission.
    let mut signals = bound
        .portal
        .receive_all_signals()
        .await
        .map_err(|error| unavailable(&error.into()))?;
    let ours = session_path(&bound.session);
    let (current, unset) = {
        let mut inner = state.inner.lock();
        let current = inner.bound(
            generation,
            wanted,
            triggers(response.shortcuts()),
            can_configure,
        );
        (current, inner.statuses.unset(wanted))
    };
    if !current {
        return Ok(());
    }
    announce(app);
    if unset && configure_if_unset && can_configure {
        let configured = bound
            .portal
            .configure_shortcuts(&bound.session, None, ConfigureShortcutsOptions::default())
            .await;
        if let Err(error) = configured {
            eprintln!("chatonlm: cannot open the shortcut settings: {error}");
        }
    }

    while let Some(message) = signals.next().await {
        let Some(signal) = decode(&message) else {
            continue;
        };
        if !signal.is_for(ours.as_deref()) {
            continue;
        }
        if let Signal::Changed { triggers, .. } = &signal {
            let changed = state.inner.lock().changed(generation, triggers);
            if changed {
                announce(app);
            }
            continue;
        }
        let hold = state.hold.load(Ordering::Relaxed);
        let command = state
            .inner
            .lock()
            .press(generation, &signal, hold, Instant::now());
        if let Some(command) = command {
            execute(app, command);
        }
    }
    // Connexion au bus perdue.
    let lost = state.inner.lock().lost(generation);
    if lost {
        announce(app);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_portal_is_disabled_only_by_a_set_non_zero_value() {
        use std::ffi::OsStr;
        assert!(!portal_disabled_by(None));
        assert!(!portal_disabled_by(Some(OsStr::new(""))));
        assert!(!portal_disabled_by(Some(OsStr::new("0"))));
        assert!(portal_disabled_by(Some(OsStr::new("1"))));
        assert!(portal_disabled_by(Some(OsStr::new("yes"))));
    }

    #[test]
    fn repeats_of_a_held_key_are_ignored() {
        let start = Instant::now();
        let mut filter = PressFilter::default();
        assert!(filter.press(start));
        // Répétitions de la touche maintenue.
        assert!(!filter.press(start + Duration::from_millis(600)));
        assert!(!filter.press(start + Duration::from_millis(630)));
        filter.release();
        assert!(filter.press(start + Duration::from_millis(700)));
        // Portail sans relâchement : un appui bien plus tard compte.
        assert!(filter.press(start + Duration::from_secs(5)));
    }

    const SESSION: &str = "/org/freedesktop/portal/desktop/session/1_2/s";

    /// Signal tel que le portail l'envoie sur le bus.
    fn message(member: &str) -> zbus::Message {
        use std::collections::HashMap;
        use zbus::zvariant::{ObjectPath, OwnedValue};
        let builder = zbus::Message::signal(
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.GlobalShortcuts",
            member,
        )
        .unwrap();
        let session = ObjectPath::try_from(SESSION).unwrap();
        let options: HashMap<String, OwnedValue> = HashMap::new();
        if member == "ShortcutsChanged" {
            let info = |description: &'static str, trigger: &'static str| {
                HashMap::<&str, zbus::zvariant::Value>::from([
                    ("description", description.into()),
                    ("trigger_description", trigger.into()),
                ])
            };
            builder
                .build(&(
                    session,
                    vec![
                        ("dictation", info("Start or stop dictation", "Ctrl+Alt+F")),
                        ("proofread", info("Correct selected text", "Meta+C")),
                    ],
                ))
                .unwrap()
        } else {
            builder
                .build(&(session, "dictation", 1_000u64, options))
                .unwrap()
        }
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(id, trigger)| ((*id).to_owned(), (*trigger).to_owned()))
            .collect()
    }

    fn bound(trigger: &str) -> ShortcutStatus {
        ShortcutStatus::Bound {
            trigger: trigger.into(),
        }
    }

    fn press(id: &str) -> Signal {
        Signal::Activated {
            session: SESSION.into(),
            id: id.into(),
        }
    }

    fn release(id: &str) -> Signal {
        Signal::Deactivated {
            session: SESSION.into(),
            id: id.into(),
        }
    }

    fn refused() -> ShortcutStatus {
        ShortcutStatus::Unavailable {
            problem: ShortcutProblem::Refused,
            reason: "The shortcut was not set".into(),
            detail: None,
        }
    }

    /// Liaison réussie de `carried`, touches données ; génération renvoyée.
    fn bind(inner: &mut Inner, carried: Wanted, triggers: &[(&str, &str)]) -> u64 {
        let generation = inner.restart(carried, false).expect("binding");
        assert!(inner.bound(generation, carried, pairs(triggers), true));
        generation
    }

    const BOTH: Wanted = Wanted {
        dictation: true,
        proofread: true,
    };
    const DICTATION: Wanted = Wanted {
        dictation: true,
        proofread: false,
    };
    const PROOFREAD: Wanted = Wanted {
        dictation: false,
        proofread: true,
    };

    #[test]
    fn portal_signals_are_decoded_from_one_stream() {
        assert_eq!(decode(&message("Activated")), Some(press("dictation")));
        assert_eq!(decode(&message("Deactivated")), Some(release("dictation")));
        assert_eq!(
            decode(&message("ShortcutsChanged")),
            Some(Signal::Changed {
                session: SESSION.into(),
                triggers: pairs(&[("dictation", "Ctrl+Alt+F"), ("proofread", "Meta+C")])
            })
        );
    }

    #[test]
    fn each_session_reads_only_its_own_signals() {
        let signal = press("dictation");
        assert!(signal.is_for(Some(SESSION)));
        assert!(!signal.is_for(Some("/org/freedesktop/portal/desktop/session/1_2/other")));
        // Chemin inconnu : tous.
        assert!(signal.is_for(None));
        // Chemin d'une session tel qu'ashpd le sérialise (un `ObjectPath`).
        let path = zbus::zvariant::ObjectPath::try_from(SESSION).unwrap();
        assert_eq!(
            serde_json::to_value(&path).unwrap(),
            serde_json::Value::String(SESSION.into())
        );
    }

    #[test]
    fn presses_follow_the_order_of_the_signals() {
        let now = Instant::now();
        let other = press("other");
        let mut presses = Presses::default();
        // Push-to-talk : appui, répétition, autre raccourci, relâchement.
        let held: Vec<_> = [
            press(DICTATION_ID),
            press(DICTATION_ID),
            other,
            release(DICTATION_ID),
        ]
        .iter()
        .filter_map(|signal| presses.handle(signal, DICTATION, true, now))
        .collect();
        assert_eq!(held, [Command::Start, Command::Stop]);
        // Bascule : seuls les appuis comptent.
        let toggled: Vec<_> = [
            press(DICTATION_ID),
            release(DICTATION_ID),
            press(DICTATION_ID),
            release(DICTATION_ID),
        ]
        .iter()
        .filter_map(|signal| presses.handle(signal, DICTATION, false, now))
        .collect();
        assert_eq!(toggled, [Command::Toggle, Command::Toggle]);
    }

    #[test]
    fn a_proofread_press_corrects_once_and_ignores_push_to_talk() {
        let now = Instant::now();
        let mut presses = Presses::default();
        // Appui, répétitions de la touche maintenue, relâchement (rien en
        // push-to-talk), nouvel appui ; la dictée a son propre filtre.
        let commands: Vec<_> = [
            press(PROOFREAD_ID),
            press(PROOFREAD_ID),
            press(DICTATION_ID),
            press(PROOFREAD_ID),
            release(PROOFREAD_ID),
            press(PROOFREAD_ID),
        ]
        .iter()
        .filter_map(|signal| presses.handle(signal, BOTH, true, now))
        .collect();
        assert_eq!(
            commands,
            [Command::Proofread, Command::Start, Command::Proofread]
        );
        // Portail sans relâchement : un appui bien plus tard compte.
        assert_eq!(
            presses.handle(
                &press(PROOFREAD_ID),
                BOTH,
                false,
                now + Duration::from_secs(5)
            ),
            Some(Command::Proofread)
        );
    }

    #[test]
    fn presses_of_a_shortcut_outside_the_set_are_ignored() {
        let now = Instant::now();
        let mut presses = Presses::default();
        assert_eq!(
            presses.handle(&press(PROOFREAD_ID), DICTATION, false, now),
            None
        );
        for signal in [press(DICTATION_ID), release(DICTATION_ID)] {
            assert_eq!(presses.handle(&signal, PROOFREAD, true, now), None);
        }
        assert_eq!(
            presses.handle(&press(PROOFREAD_ID), PROOFREAD, true, now),
            Some(Command::Proofread)
        );
    }

    #[test]
    fn the_wanted_set_is_bound_in_one_request() {
        assert_eq!(Wanted::default().ids(), Vec::<&str>::new());
        assert!(!Wanted::default().any());
        assert_eq!(
            DICTATION.requests(Lang::En),
            [(
                "dictation",
                "Start or stop dictation".to_owned(),
                "CTRL+ALT+D"
            )]
        );
        assert_eq!(
            BOTH.requests(Lang::En),
            [
                (
                    "dictation",
                    "Start or stop dictation".to_owned(),
                    "CTRL+ALT+D"
                ),
                (
                    "proofread",
                    "Correct selected text".to_owned(),
                    "CTRL+ALT+C"
                ),
            ]
        );
        // Description dans la langue de l'interface.
        assert_eq!(
            PROOFREAD.requests(Lang::Fr),
            [(
                "proofread",
                "Corriger le texte sélectionné".to_owned(),
                "CTRL+ALT+C"
            )]
        );
        assert_eq!(
            PROOFREAD.requests(Lang::Es)[0].1,
            "Corregir el texto seleccionado"
        );
    }

    #[test]
    fn only_a_changed_set_is_bound_again() {
        let mut inner = Inner::default();
        // Rien de voulu au départ : rien à faire.
        assert_eq!(inner.change(|wanted| wanted.dictation = false), None);
        assert_eq!(
            inner.change(|wanted| wanted.proofread = true),
            Some(PROOFREAD)
        );
        assert!(inner.restart(PROOFREAD, false).is_some());
        // Même ensemble (réglage enregistré de nouveau) : pas de nouvelle
        // liaison, donc pas de boîte du bureau redemandée après un refus.
        assert_eq!(inner.change(|wanted| wanted.proofread = true), None);
        assert_eq!(inner.change(|wanted| wanted.dictation = true), Some(BOTH));
        assert!(inner.restart(BOTH, false).is_some());
        assert_eq!(
            inner.change(|wanted| wanted.dictation = false),
            Some(PROOFREAD)
        );
        assert_eq!(
            inner.change(|wanted| *wanted = Wanted::default()),
            Some(Wanted::default())
        );
    }

    #[test]
    fn statuses_follow_the_set() {
        let mut inner = Inner::default();
        assert_eq!(inner.statuses, Statuses::default());
        let first = inner.restart(PROOFREAD, false).expect("binding");
        assert_eq!(
            inner.statuses,
            Statuses {
                dictation: ShortcutStatus::Disabled,
                proofread: ShortcutStatus::Binding,
            }
        );
        let both = inner.restart(BOTH, false).expect("binding");
        assert!(both > first);
        assert_eq!(
            inner.statuses,
            Statuses::each(BOTH, |_| ShortcutStatus::Binding)
        );
        // Réponse du portail : touche de chacun, vide si absente ; un
        // raccourci hors de l'ensemble cité par le portail est ignoré.
        let answer = pairs(&[("proofread", "Ctrl+Alt+C"), ("other", "X")]);
        assert!(inner.bound(both, BOTH, answer, true));
        assert_eq!(
            inner.statuses,
            Statuses {
                dictation: bound(""),
                proofread: bound("Ctrl+Alt+C"),
            }
        );
        assert!(inner.statuses.unset(BOTH));
        assert!(!inner.statuses.unset(PROOFREAD));
        assert!(inner.can_configure);
        // Sans session liée, refus : tous les raccourcis voulus
        // « Unavailable ».
        let mut fresh = Inner::default();
        let generation = fresh.restart(BOTH, false).expect("binding");
        assert!(fresh.failed(generation, &refused()));
        assert_eq!(
            fresh.statuses,
            Statuses {
                dictation: refused(),
                proofread: refused(),
            }
        );
        // Déliaison : tout « Disabled », plus de session liée.
        assert_eq!(inner.restart(Wanted::default(), false), None);
        assert_eq!(inner.statuses, Statuses::default());
        assert!(inner.live.is_none() && !inner.can_configure);
    }

    #[test]
    fn a_refused_new_set_keeps_the_bound_shortcuts() {
        // Dictée liée ; la correction activée ouvre une nouvelle liaison des
        // deux (boîte du bureau).
        let mut inner = Inner::default();
        let old = bind(&mut inner, DICTATION, &[("dictation", "Ctrl+Alt+D")]);
        let new = inner.restart(BOTH, false).expect("binding");
        // Pendant la boîte : la dictée reste liée et son raccourci marche
        // (par l'ancienne session seulement) ; la correction attend.
        assert_eq!(
            inner.statuses,
            Statuses {
                dictation: bound("Ctrl+Alt+D"),
                proofread: ShortcutStatus::Binding,
            }
        );
        let now = Instant::now();
        assert_eq!(
            inner.press(old, &press(DICTATION_ID), false, now),
            Some(Command::Toggle)
        );
        assert_eq!(inner.press(new, &press(DICTATION_ID), false, now), None);
        // L'ancienne session ne porte pas la correction.
        assert_eq!(inner.press(old, &press(PROOFREAD_ID), false, now), None);
        // Refus : seule la correction « Unavailable », l'ancienne session
        // reste (et sa touche, sa boîte de réglage).
        assert!(inner.failed(new, &refused()));
        assert_eq!(
            inner.statuses,
            Statuses {
                dictation: bound("Ctrl+Alt+D"),
                proofread: refused(),
            }
        );
        assert_eq!(inner.live.as_ref().map(|live| live.generation), Some(old));
        assert!(inner.can_configure);
        assert_eq!(
            inner.press(old, &press(DICTATION_ID), false, now + REPEAT_WINDOW * 2),
            Some(Command::Toggle)
        );
        // Correction coupée ensuite : l'ensemble est celui de la session
        // liée, rien à relier (pas de boîte).
        assert_eq!(
            inner.change(|wanted| wanted.proofread = false),
            Some(DICTATION)
        );
        assert_eq!(inner.restart(DICTATION, false), None);
        assert_eq!(
            inner.statuses,
            Statuses {
                dictation: bound("Ctrl+Alt+D"),
                proofread: ShortcutStatus::Disabled,
            }
        );
        // Nouvel essai demandé : relié quand même.
        assert!(inner.restart(DICTATION, true).is_some());
    }

    #[test]
    fn a_successful_binding_replaces_the_old_session() {
        let mut inner = Inner::default();
        let old = bind(&mut inner, DICTATION, &[("dictation", "Ctrl+Alt+D")]);
        let new = inner.restart(BOTH, false).expect("binding");
        assert!(inner.bound(
            new,
            BOTH,
            pairs(&[("dictation", "Ctrl+Alt+D"), ("proofread", "Ctrl+Alt+C")]),
            true
        ));
        assert_eq!(inner.live.as_ref().map(|live| live.generation), Some(new));
        // Un seul exécutant : l'ancienne session n'exécute plus rien.
        let now = Instant::now();
        assert_eq!(inner.press(old, &press(DICTATION_ID), false, now), None);
        assert_eq!(
            inner.press(new, &press(DICTATION_ID), false, now),
            Some(Command::Toggle)
        );
        assert_eq!(
            inner.press(new, &press(PROOFREAD_ID), false, now),
            Some(Command::Proofread)
        );
        assert_eq!(
            inner.statuses,
            Statuses {
                dictation: bound("Ctrl+Alt+D"),
                proofread: bound("Ctrl+Alt+C"),
            }
        );
        // Ensemble réduit : relié (la touche retirée est rendue au bureau) ;
        // en attendant, le raccourci coupé n'exécute plus rien.
        let reduced = inner.restart(DICTATION, false).expect("binding");
        assert_eq!(inner.statuses.proofread, ShortcutStatus::Disabled);
        assert_eq!(inner.statuses.dictation, bound("Ctrl+Alt+D"));
        let later = now + REPEAT_WINDOW * 2;
        assert_eq!(inner.press(new, &press(PROOFREAD_ID), false, later), None);
        assert_eq!(
            inner.press(new, &press(DICTATION_ID), false, later),
            Some(Command::Toggle)
        );
        assert!(inner.bound(
            reduced,
            DICTATION,
            pairs(&[("dictation", "Ctrl+Alt+D")]),
            true
        ));
    }

    #[test]
    fn a_desktop_change_updates_only_the_shortcuts_it_names() {
        let statuses = Statuses {
            dictation: bound("Ctrl+Alt+D"),
            proofread: bound("Ctrl+Alt+C"),
        };
        assert_eq!(
            statuses.changed(BOTH, &pairs(&[("proofread", "Meta+C")])),
            Statuses {
                dictation: bound("Ctrl+Alt+D"),
                proofread: bound("Meta+C"),
            }
        );
        // Raccourci hors de l'ensemble : ignoré.
        let mut inner = Inner::default();
        let generation = bind(&mut inner, DICTATION, &[("dictation", "Ctrl+Alt+D")]);
        assert!(!inner.changed(generation, &pairs(&[("proofread", "Meta+C")])));
        assert_eq!(inner.statuses.proofread, ShortcutStatus::Disabled);
        // Touche de la session liée changée, gardée pour la suite (ensemble
        // revenu à celui de la session liée sans nouvelle liaison).
        assert!(inner.changed(generation, &pairs(&[("dictation", "Ctrl+Alt+F")])));
        assert_eq!(inner.statuses.dictation, bound("Ctrl+Alt+F"));
        let pending = inner.restart(BOTH, false).expect("binding");
        assert!(!inner.changed(pending, &pairs(&[("dictation", "Meta+D")])));
        assert!(inner.failed(pending, &refused()));
        assert_eq!(inner.restart(DICTATION, false), None);
        assert_eq!(inner.statuses.dictation, bound("Ctrl+Alt+F"));
    }

    #[test]
    fn a_stale_binding_cannot_overwrite_newer_statuses() {
        let mut inner = Inner::default();
        let first = inner.restart(DICTATION, false).expect("binding");
        let second = inner.restart(BOTH, false).expect("binding");
        assert!(!inner.bound(first, DICTATION, Vec::new(), true));
        assert!(!inner.failed(first, &refused()));
        assert_eq!(
            inner.statuses,
            Statuses::each(BOTH, |_| ShortcutStatus::Binding)
        );
        assert!(inner.live.is_none());
        assert!(inner.bound(second, BOTH, pairs(&[("dictation", "Ctrl+Alt+D")]), true));
        // Déliaison : la liaison passée ne peut plus rien écrire.
        inner.restart(Wanted::default(), false);
        assert!(!inner.bound(second, BOTH, Vec::new(), true));
        assert!(!inner.failed(second, &refused()));
        assert!(!inner.lost(second));
        assert_eq!(inner.statuses, Statuses::default());
        assert!(!inner.wanted.any() && !inner.can_configure);
    }

    #[test]
    fn lost_signals_make_the_bound_shortcuts_unavailable() {
        let mut inner = Inner::default();
        let old = bind(&mut inner, DICTATION, &[("dictation", "Ctrl+Alt+D")]);
        let new = inner.restart(BOTH, false).expect("binding");
        assert!(inner.failed(new, &refused()));
        // Pas la session liée : rien.
        assert!(!inner.lost(new));
        assert!(inner.lost(old));
        assert_eq!(
            inner.statuses,
            Statuses {
                dictation: unavailable_status(ShortcutProblem::Lost, None),
                proofread: refused(),
            }
        );
        assert!(inner.live.is_none() && !inner.can_configure);
        assert_eq!(
            inner.press(old, &press(DICTATION_ID), false, Instant::now()),
            None
        );
    }

    #[test]
    fn hold_mode_starts_on_press_and_stops_on_release() {
        assert_eq!(on_activated(false), Command::Toggle);
        assert_eq!(on_deactivated(false), None);
        assert_eq!(on_activated(true), Command::Start);
        assert_eq!(on_deactivated(true), Some(Command::Stop));
    }

    #[test]
    fn missing_portal_and_refusal_have_their_own_reasons() {
        let no_portal = ShortcutStatus::Unavailable {
            problem: ShortcutProblem::NoPortal,
            reason: "Global shortcuts are not available on this desktop".into(),
            detail: None,
        };
        let missing = ashpd::Error::PortalNotFound(
            "org.freedesktop.portal.GlobalShortcuts".try_into().unwrap(),
        );
        assert_eq!(unavailable(&missing), no_portal);
        let unknown = ashpd::Error::Zbus(zbus::Error::FDO(Box::new(
            zbus::fdo::Error::ServiceUnknown("org.freedesktop.portal.Desktop".into()),
        )));
        assert_eq!(unavailable(&unknown), no_portal);
        assert_eq!(
            unavailable(&ashpd::Error::Response(ResponseError::Cancelled)),
            ShortcutStatus::Unavailable {
                problem: ShortcutProblem::Refused,
                reason: "The shortcut was not set".into(),
                detail: None,
            }
        );
        let ShortcutStatus::Unavailable {
            problem,
            reason,
            detail,
        } = unavailable(&ashpd::Error::NoResponse)
        else {
            panic!("unavailable expected");
        };
        assert_eq!(problem, ShortcutProblem::Failed);
        let detail = detail.expect("technical detail kept");
        assert_eq!(
            reason,
            format!("The desktop could not set the shortcut: {detail}")
        );
    }

    #[test]
    fn reasons_follow_the_language_with_their_detail() {
        assert_eq!(
            reason(ShortcutProblem::Failed, Some("no response"), Lang::Fr),
            "Le bureau n'a pas pu définir le raccourci\u{a0}: no response"
        );
        assert_eq!(
            reason(ShortcutProblem::NoPortal, None, Lang::Es),
            "Los atajos globales no están disponibles en este escritorio"
        );
        assert_eq!(
            reason(ShortcutProblem::Lost, None, Lang::En),
            "The desktop stopped the shortcut"
        );
    }

    #[test]
    fn the_reason_is_written_again_in_the_current_language() {
        // Raison écrite dans une autre langue avant un changement : le
        // statut lu la refait dans la langue courante (l'anglais des essais),
        // avec le même détail.
        let shortcuts = Shortcuts::default();
        let failed = ShortcutStatus::Unavailable {
            problem: ShortcutProblem::Failed,
            reason: "Le bureau n'a pas pu définir le raccourci\u{a0}: boom".into(),
            detail: Some("boom".into()),
        };
        shortcuts.inner.lock().statuses = Statuses::each(BOTH, |_| failed.clone());
        let expected = ShortcutStatus::Unavailable {
            problem: ShortcutProblem::Failed,
            reason: "The desktop could not set the shortcut: boom".into(),
            detail: Some("boom".into()),
        };
        assert_eq!(shortcuts.status(), expected);
        assert_eq!(shortcuts.proofread_status(), expected);
    }
}
