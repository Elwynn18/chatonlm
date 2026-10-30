//! Easy Link prévient à la fin d'une tâche de VS Code (lien VS Code, lot 4a,
//! propre à ChatonLM) : seul envoi SPONTANÉ du bot, aux comptes liés, sans
//! message reçu d'eux. Et « où en est-on ? » : `/status` montre les tâches de
//! l'éditeur en cours ou finies depuis moins d'une heure.
//!
//! Bornes (contrat `.tools/research/93-lot4a-contract.md` § 1) :
//! - réglage « Let VS Code notify me through Easy Link » coupé par défaut ;
//!   coupé, rien ne part et `/status` ne montre aucune tâche ;
//! - un message au plus toutes les 30 s par compte, 1 000 caractères au plus ;
//! - jamais de code ni de sortie du terminal : le nom de la tâche (une ligne,
//!   80 caractères au plus, tout chemin réduit à son dernier élément, montré
//!   en code pour qu'aucun Markdown n'y agisse), sa durée, son issue, et le
//!   nom du dossier ouvert (dernier élément seulement) si l'éditeur le donne ;
//! - dans la langue de l'app, par les bots connectés seulement.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::{EasyLinkLink, Host, Shared};
use crate::i18n::t;
use crate::store::easy_link::{self as saved, Account, Platform};
use crate::store::now_ms;

/// Écart minimal entre deux messages spontanés à un même compte.
pub const MIN_INTERVAL: Duration = Duration::from_secs(30);

/// Longueur maximale d'un message spontané (caractères).
pub const MAX_CHARS: usize = 1_000;

/// Longueur maximale du nom d'une tâche, et d'un nom de dossier.
const TITLE_CHARS: usize = 80;
const FOLDER_CHARS: usize = 60;

/// Tâches gardées au plus (les plus anciennes oubliées d'abord).
const TASKS_KEPT: usize = 32;

/// Tâches montrées au plus par `/status`.
const TASKS_SHOWN: usize = 10;

/// Une tâche finie reste montrée par `/status` pendant une heure.
const FINISHED_SHOWN_MS: i64 = 60 * 60 * 1000;

/// Issue d'une tâche finie.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Success,
    Failure,
    /// Finie, issue inconnue (fin d'une réponse de ChatonLM).
    Done,
}

/// Fin de tâche annoncée par l'éditeur (`notify.send`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub title: String,
    pub outcome: Outcome,
    pub duration_ms: u64,
    /// Dossier ouvert, si l'utilisateur l'a permis dans l'extension : seul
    /// son dernier élément part.
    pub workspace: Option<String>,
}

/// État d'une tâche de l'éditeur (`notify.task`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskState {
    Running,
    Success,
    Failure,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskUpdate {
    /// Identifiant choisi par l'éditeur (unique pour sa connexion).
    pub id: String,
    pub title: String,
    pub state: TaskState,
    /// Début (ms depuis l'époque Unix), d'après l'éditeur.
    pub started_at: i64,
}

/// Refus d'un envoi ou d'une tâche.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotifyError {
    /// Réglage coupé.
    Disabled,
    /// Aucun compte lié à un bot connecté.
    NotLinked,
    /// Base illisible (texte du moteur).
    Failed(String),
}

/// Tâche retenue pour `/status`.
#[derive(Clone, Debug)]
pub(crate) struct Task {
    title: String,
    state: TaskState,
    started_at: i64,
    ended_at: Option<i64>,
    updated_at: i64,
}

/// Tâches de l'éditeur (connexion du pont, identifiant) et derniers envois
/// par compte.
#[derive(Default)]
pub(crate) struct Board {
    tasks: parking_lot::Mutex<HashMap<(u64, String), Task>>,
    sent: parking_lot::Mutex<HashMap<i64, Instant>>,
}

/// Une ligne, sans caractère de contrôle ni accent grave (le texte est montré
/// en code), espaces resserrées.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() || c == '`' { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Coupé à `max` caractères (« … » compris).
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut kept: String = text.chars().take(max.saturating_sub(1)).collect();
    kept.truncate(kept.trim_end().len());
    kept.push('…');
    kept
}

/// Dernier élément non vide d'un chemin (`/` ou `\`), sinon le mot.
fn last_component(word: &str) -> &str {
    word.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(word)
}

/// Nom d'une tâche tel qu'il part : une ligne, chaque chemin réduit à son
/// dernier élément, 80 caractères au plus ; vide : nom par défaut.
pub fn task_title(title: &str) -> String {
    let line = one_line(title);
    let words: Vec<&str> = line
        .split(' ')
        .map(|word| {
            if word.contains(['/', '\\']) {
                last_component(word)
            } else {
                word
            }
        })
        .collect();
    let title = cut(&words.join(" "), TITLE_CHARS);
    if title.is_empty() {
        t!("easyLink.notify.untitled")
    } else {
        title
    }
}

/// Nom du dossier ouvert : dernier élément du chemin seulement.
pub fn folder_name(workspace: &str) -> Option<String> {
    let line = one_line(workspace);
    let name = cut(&one_line(last_component(line.trim())), FOLDER_CHARS);
    (!name.is_empty()).then_some(name)
}

/// Durée lisible : « 42 s », « 3 min 05 s », « 1 h 05 min ».
pub fn duration_text(ms: u64) -> String {
    let seconds = ms / 1000;
    let (hours, minutes, seconds) = (seconds / 3600, (seconds / 60) % 60, seconds % 60);
    if hours > 0 {
        t!(
            "easyLink.notify.hours",
            h = hours,
            m = format!("{minutes:02}")
        )
    } else if minutes > 0 {
        t!(
            "easyLink.notify.minutes",
            m = minutes,
            s = format!("{seconds:02}")
        )
    } else {
        t!("easyLink.notify.seconds", s = seconds)
    }
}

/// Texte du message spontané (Markdown), 1 000 caractères au plus.
pub fn message(note: &Note) -> String {
    let title = format!("`{}`", task_title(&note.title));
    let duration = duration_text(note.duration_ms);
    let mut text = match note.outcome {
        Outcome::Success => t!(
            "easyLink.notify.success",
            title = title,
            duration = duration
        ),
        Outcome::Failure => t!(
            "easyLink.notify.failure",
            title = title,
            duration = duration
        ),
        Outcome::Done => t!("easyLink.notify.done", title = title, duration = duration),
    };
    if let Some(folder) = note.workspace.as_deref().and_then(folder_name) {
        text.push('\n');
        text.push_str(&t!(
            "easyLink.notify.folder",
            folder = format!("`{folder}`")
        ));
    }
    cut(&text, MAX_CHARS)
}

impl Board {
    /// Comptes à qui écrire maintenant (30 s depuis le dernier envoi), avec
    /// l'heure de leur réservation ; leur envoi est réservé (deux annonces
    /// simultanées n'en font qu'une).
    fn reserve(&self, accounts: Vec<Account>) -> Vec<(Account, Instant)> {
        let now = Instant::now();
        let mut sent = self.sent.lock();
        accounts
            .into_iter()
            .filter(|account| {
                let free = sent
                    .get(&account.id)
                    .is_none_or(|last| now.duration_since(*last) >= MIN_INTERVAL);
                if free {
                    sent.insert(account.id, now);
                }
                free
            })
            .map(|account| (account, now))
            .collect()
    }

    /// Envoi raté : la réservation est rendue (rien n'est parti, le compte
    /// peut être prévenu à la prochaine fin), sauf si une autre l'a remplacée.
    fn release(&self, account: i64, reserved: Instant) {
        let mut sent = self.sent.lock();
        if sent.get(&account) == Some(&reserved) {
            sent.remove(&account);
        }
    }

    fn update(&self, connection: u64, update: TaskUpdate, now: i64) {
        let mut tasks = self.tasks.lock();
        let key = (connection, update.id);
        let ended_at = match update.state {
            TaskState::Running => None,
            // Fin déjà notée : gardée (une annonce répétée ne la déplace pas).
            _ => Some(
                tasks
                    .get(&key)
                    .and_then(|task| task.ended_at)
                    .unwrap_or(now),
            ),
        };
        tasks.insert(
            key,
            Task {
                title: task_title(&update.title),
                state: update.state,
                // Jamais dans le futur, ni avant une semaine.
                started_at: update.started_at.clamp(now - 7 * 24 * 3600 * 1000, now),
                ended_at,
                updated_at: now,
            },
        );
        while tasks.len() > TASKS_KEPT {
            let oldest = tasks
                .iter()
                .min_by_key(|(_, task)| task.updated_at)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(oldest) => tasks.remove(&oldest),
                None => break,
            };
        }
    }

    /// Connexion finie : ses tâches en cours ne se finiront plus.
    pub(crate) fn connection_ended(&self, connection: u64) {
        self.tasks
            .lock()
            .retain(|(owner, _), task| *owner != connection || task.ended_at.is_some());
    }

    /// Essais : dernier envoi au compte noté à `at`.
    #[cfg(test)]
    pub(crate) fn sent_at(&self, account: i64, at: Instant) {
        self.sent.lock().insert(account, at);
    }

    /// Essais : dernier envoi réservé au compte.
    #[cfg(test)]
    pub(crate) fn last_sent(&self, account: i64) -> Option<Instant> {
        self.sent.lock().get(&account).copied()
    }

    /// Essais : fin d'une tâche déplacée.
    #[cfg(test)]
    pub(crate) fn ended_at(&self, connection: u64, id: &str, at: i64) {
        if let Some(task) = self.tasks.lock().get_mut(&(connection, id.to_owned())) {
            task.ended_at = Some(at);
        }
    }

    pub(crate) fn clear(&self) {
        self.tasks.lock().clear();
    }

    /// Lignes de `/status` : tâches en cours, et finies depuis moins d'une
    /// heure, les plus récentes d'abord ; aucune s'il n'y en a pas.
    fn lines(&self, now: i64) -> Vec<String> {
        let mut tasks: Vec<Task> = self
            .tasks
            .lock()
            .values()
            .filter(|task| {
                task.ended_at
                    .is_none_or(|ended| now - ended < FINISHED_SHOWN_MS)
            })
            .cloned()
            .collect();
        if tasks.is_empty() {
            return Vec::new();
        }
        tasks.sort_by_key(|task| std::cmp::Reverse(task.started_at));
        let mut lines = vec![t!("easyLink.notify.tasksTitle")];
        for task in tasks.iter().take(TASKS_SHOWN) {
            let title = format!("`{}`", task.title);
            let since = |from: i64| duration_text(u64::try_from(now - from).unwrap_or(0));
            lines.push(match (task.state, task.ended_at) {
                (TaskState::Running, _) | (_, None) => t!(
                    "easyLink.notify.taskRunning",
                    title = title,
                    duration = since(task.started_at)
                ),
                (TaskState::Success, Some(ended)) => t!(
                    "easyLink.notify.taskSuccess",
                    title = title,
                    duration = since(ended)
                ),
                (TaskState::Failure, Some(ended)) => t!(
                    "easyLink.notify.taskFailure",
                    title = title,
                    duration = since(ended)
                ),
            });
        }
        lines
    }
}

impl<H: Host> Shared<H> {
    async fn vscode_notify(&self) -> Result<bool, NotifyError> {
        self.store
            .call(|c| saved::vscode_notify(c))
            .await
            .map_err(|error| NotifyError::Failed(error.to_string()))
    }

    /// Bots connectés (messagerie, jeton).
    fn connected_bots(&self) -> Vec<(Platform, String)> {
        self.live
            .lock()
            .iter()
            .filter(|(_, live)| live.link == EasyLinkLink::Connected)
            .map(|(platform, live)| (*platform, live.token.clone()))
            .collect()
    }

    /// Prévient les comptes liés de la fin d'une tâche ; rend le nombre de
    /// messages partis (0 si tous ont reçu un message il y a moins de 30 s).
    pub(crate) async fn notify(&self, note: &Note) -> Result<u32, NotifyError> {
        if !self.vscode_notify().await? {
            return Err(NotifyError::Disabled);
        }
        let bots = self.connected_bots();
        let accounts: Vec<Account> = self
            .store
            .call(|c| saved::linked(c))
            .await
            .map_err(|error| NotifyError::Failed(error.to_string()))?
            .into_iter()
            .filter(|account| {
                bots.iter()
                    .any(|(platform, _)| *platform == account.platform)
            })
            .collect();
        if accounts.is_empty() || self.closed() {
            return Err(NotifyError::NotLinked);
        }
        let text = message(note);
        let mut sent = 0;
        for (account, reserved) in self.board.reserve(accounts) {
            let messenger = bots
                .iter()
                .find(|(p, _)| *p == account.platform)
                .and_then(|(_, token)| self.messenger(account.platform, token).ok());
            let Some(messenger) = messenger else {
                self.board.release(account.id, reserved);
                continue;
            };
            let delivered = match messenger.direct_channel(&account.user_id).await {
                Ok(channel) => messenger.send(&channel, &text).await.map(drop),
                Err(problem) => Err(problem),
            };
            match delivered {
                Ok(()) => sent += 1,
                Err(problem) => {
                    self.board.release(account.id, reserved);
                    // i18n-ignore : journal.
                    eprintln!("chatonlm: Easy Link notice not sent: {problem:?}");
                }
            }
        }
        Ok(sent)
    }

    /// Tâche de l'éditeur, pour `/status` (réglage coupé : refusée).
    pub(crate) async fn task(
        &self,
        connection: u64,
        update: TaskUpdate,
    ) -> Result<(), NotifyError> {
        if !self.vscode_notify().await? {
            return Err(NotifyError::Disabled);
        }
        self.board.update(connection, update, now_ms());
        Ok(())
    }

    /// Lignes des tâches de l'éditeur pour `/status` (réglage coupé : aucune).
    pub(crate) async fn task_lines(&self) -> Vec<String> {
        match self.vscode_notify().await {
            Ok(true) => self.board.lines(now_ms()),
            _ => Vec::new(),
        }
    }
}

/// Service d'Easy Link vu du pont : envoi spontané et tâches.
pub trait Notifier: Send + Sync {
    fn notify(
        &self,
        note: Note,
    ) -> crate::dictation::clipboard::BoxFuture<Result<u32, NotifyError>>;
    fn task(
        &self,
        connection: u64,
        update: TaskUpdate,
    ) -> crate::dictation::clipboard::BoxFuture<Result<(), NotifyError>>;
    fn connection_ended(&self, connection: u64);
}

impl<H: Host> Notifier for super::EasyLink<H> {
    fn notify(
        &self,
        note: Note,
    ) -> crate::dictation::clipboard::BoxFuture<Result<u32, NotifyError>> {
        let shared = Arc::clone(&self.0);
        Box::pin(async move { shared.notify(&note).await })
    }

    fn task(
        &self,
        connection: u64,
        update: TaskUpdate,
    ) -> crate::dictation::clipboard::BoxFuture<Result<(), NotifyError>> {
        let shared = Arc::clone(&self.0);
        Box::pin(async move { shared.task(connection, update).await })
    }

    fn connection_ended(&self, connection: u64) {
        self.0.board.connection_ended(connection);
    }
}
