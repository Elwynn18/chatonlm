//! Presse-papiers de la dictée et de la correction de la sélection :
//! arboard, sans fenêtre au premier plan.
//!
//! - Linux (`linux.rs`) : Wayland par le protocole data-control, X11 sinon.
//!   Sous X11, le contenu appartient au `Clipboard` d'arboard et disparaît
//!   avec lui : un fil dédié le garde en vie pour toute la durée de l'app (et
//!   le recrée après une erreur). La sélection primaire (texte surligné dans
//!   n'importe quelle application) se lit de la même façon, sans focus.
//! - Windows (`windows.rs`) : pas de sélection primaire ; le texte surligné
//!   est copié par un Ctrl+C simulé, lu dans le presse-papiers, puis l'ancien
//!   texte du presse-papiers y est remis (`selection.rs` : logique pure).
//!
//! Chaque lecture de la sélection a son propre fil, abandonné au bout de
//! [`SELECTION_TIMEOUT`] (une application figée qui détient la sélection ne
//! bloque ni les copies, ni la lecture suivante).
//!
//! Collage ([`Clipboard::paste`], correction de la sélection) : Ctrl+V envoyé
//! à l'application au premier plan, par le portail RemoteDesktop du bureau
//! sous Linux (`portal.rs`), par `SendInput` sous Windows (`windows.rs`).

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::oneshot;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod portal;
pub mod selection;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux::read_primary as read_selection;
#[cfg(windows)]
use windows::read_selection;

/// Futur renvoyé par les services de la dictée.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Attente maximale du texte de la sélection primaire (l'application qui la
/// détient l'envoie elle-même).
pub const SELECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// Pourquoi la sélection primaire n'a pas pu être lue ; l'appelant montre
/// le texte qui convient, dans la langue de l'interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionError {
    /// L'application qui détient la sélection n'a pas répondu à temps.
    Timeout,
    /// Le fil de lecture s'est arrêté sans répondre.
    ThreadStopped,
    /// Le bureau ne permet pas de lire la sélection primaire.
    Unsupported,
    /// Autre échec : détail technique de la bibliothèque (en anglais).
    Other(String),
}

/// Pourquoi le presse-papiers n'a pas pu être collé dans l'application au
/// premier plan ; l'appelant dit alors de coller à la main.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasteError {
    /// Le bureau n'offre pas de quoi envoyer des touches (pas de portail
    /// RemoteDesktop).
    Unsupported,
    /// L'autorisation demandée par le bureau a été refusée.
    Refused,
    /// Autre échec : détail technique (en anglais).
    Other(String),
}

/// Destination du texte dicté, source du texte sélectionné (le vrai
/// presse-papiers, ou un faux).
pub trait Clipboard: Send + Sync {
    /// Copie `text` ; erreur en anglais.
    fn copy(&self, text: String) -> BoxFuture<Result<(), String>>;

    /// Texte de la sélection primaire, lu sans focus ; vide s'il n'y en a
    /// pas.
    fn selection(&self) -> BoxFuture<Result<String, SelectionError>> {
        Box::pin(async { Err(SelectionError::Unsupported) })
    }

    /// Colle le presse-papiers dans l'application au premier plan (Ctrl+V
    /// simulé) : le texte sélectionné y est remplacé.
    fn paste(&self) -> BoxFuture<Result<(), PasteError>> {
        Box::pin(async { Err(PasteError::Unsupported) })
    }
}

type Request = (String, oneshot::Sender<Result<(), String>>);

/// Presse-papiers du système ; le fil démarre à la première copie.
#[derive(Default)]
pub struct SystemClipboard {
    requests: Mutex<Option<mpsc::Sender<Request>>>,
    /// Fichier où le jeton du portail RemoteDesktop est gardé (Linux) :
    /// donné au démarrage de l'app ; sans lui, l'autorisation du bureau est
    /// redemandée à chaque session. Jamais lu sous Windows.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    paste_token: Mutex<Option<PathBuf>>,
}

impl SystemClipboard {
    /// Fichier (dans les données de l'app) où garder l'autorisation de
    /// coller donnée par le bureau.
    pub fn set_paste_token_file(&self, path: PathBuf) {
        *self.paste_token.lock() = Some(path);
    }

    fn sender(&self) -> Result<mpsc::Sender<Request>, String> {
        let mut requests = self.requests.lock();
        if let Some(sender) = requests.as_ref() {
            return Ok(sender.clone());
        }
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("chatonlm-clipboard".into())
            .spawn(move || serve(&receiver))
            .map_err(|error| error.to_string())?;
        *requests = Some(sender.clone());
        Ok(sender)
    }
}

impl Clipboard for SystemClipboard {
    fn copy(&self, text: String) -> BoxFuture<Result<(), String>> {
        let sender = self.sender();
        Box::pin(async move {
            let (reply, answer) = oneshot::channel();
            sender?
                .send((text, reply))
                .map_err(|_| "the clipboard thread stopped".to_owned())?;
            answer
                .await
                .map_err(|_| "the clipboard thread stopped".to_owned())?
        })
    }

    fn selection(&self) -> BoxFuture<Result<String, SelectionError>> {
        Box::pin(read_with_timeout(read_selection, SELECTION_TIMEOUT))
    }

    #[cfg(target_os = "linux")]
    fn paste(&self) -> BoxFuture<Result<(), PasteError>> {
        let token = self.paste_token.lock().clone();
        Box::pin(async move { portal::paste(token.as_deref()).await })
    }

    #[cfg(windows)]
    fn paste(&self) -> BoxFuture<Result<(), PasteError>> {
        Box::pin(async {
            let (reply, answer) = oneshot::channel();
            thread::Builder::new()
                .name("chatonlm-paste".into())
                .spawn(move || {
                    let _ = reply.send(windows::paste());
                })
                .map_err(|error| PasteError::Other(error.to_string()))?;
            answer
                .await
                .map_err(|_| PasteError::Other("the paste thread stopped".into()))?
        })
    }
}

/// Lit avec `read` sur un fil à part ; abandonné (le fil finit seul) après
/// `limit`.
async fn read_with_timeout(
    read: fn() -> Result<String, SelectionError>,
    limit: Duration,
) -> Result<String, SelectionError> {
    let (reply, answer) = oneshot::channel();
    thread::Builder::new()
        .name("chatonlm-selection".into())
        .spawn(move || {
            let _ = reply.send(read());
        })
        .map_err(|error| SelectionError::Other(error.to_string()))?;
    match tokio::time::timeout(limit, answer).await {
        Ok(Ok(text)) => text,
        Ok(Err(_)) => Err(SelectionError::ThreadStopped),
        Err(_) => Err(SelectionError::Timeout),
    }
}

/// Échec d'arboard : sélection primaire non prise en charge par le bureau
/// (compositeur Wayland sans data-control version 2), ou autre (détail).
fn selection_error(error: arboard::Error) -> SelectionError {
    match error {
        arboard::Error::ClipboardNotSupported => SelectionError::Unsupported,
        error => SelectionError::Other(error.to_string()),
    }
}

fn serve(requests: &mpsc::Receiver<Request>) {
    let mut clipboard: Option<arboard::Clipboard> = None;
    for (text, reply) in requests {
        let _ = reply.send(copy(&mut clipboard, &text));
    }
}

/// Copie avec le `Clipboard` gardé, recréé une fois en cas d'erreur
/// (compositeur redémarré...).
fn copy(clipboard: &mut Option<arboard::Clipboard>, text: &str) -> Result<(), String> {
    let mut last_error = String::new();
    for _ in 0..2 {
        if clipboard.is_none() {
            *clipboard = Some(arboard::Clipboard::new().map_err(|error| error.to_string())?);
        }
        let Some(current) = clipboard.as_mut() else {
            continue;
        };
        match current.set_text(text) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = error.to_string();
                *clipboard = None;
            }
        }
    }
    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_selection_that_never_comes_is_abandoned() {
        fn hangs() -> Result<String, SelectionError> {
            thread::sleep(Duration::from_secs(2));
            Ok("late".into())
        }
        fn answers() -> Result<String, SelectionError> {
            Ok("selected".into())
        }
        fn panics() -> Result<String, SelectionError> {
            panic!("arboard panicked")
        }
        let started = std::time::Instant::now();
        assert_eq!(
            read_with_timeout(hangs, Duration::from_millis(100)).await,
            Err(SelectionError::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            read_with_timeout(answers, Duration::from_secs(1)).await,
            Ok("selected".into())
        );
        // Fil arrêté sans réponse (panique) : erreur à part, pas un délai.
        assert_eq!(
            read_with_timeout(panics, Duration::from_secs(5)).await,
            Err(SelectionError::ThreadStopped)
        );
    }

    #[test]
    fn arboard_errors_are_typed() {
        assert_eq!(
            selection_error(arboard::Error::ClipboardNotSupported),
            SelectionError::Unsupported
        );
        // Seul le détail de la bibliothèque reste tel quel.
        let SelectionError::Other(detail) = selection_error(arboard::Error::ClipboardOccupied)
        else {
            panic!("other error expected");
        };
        assert_eq!(detail, arboard::Error::ClipboardOccupied.to_string());
    }
}
