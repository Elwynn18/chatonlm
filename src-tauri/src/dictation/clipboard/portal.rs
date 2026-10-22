//! Collage sous Linux : Ctrl+V envoyé à l'application au premier plan par le
//! portail `org.freedesktop.portal.RemoteDesktop` (KDE Plasma, GNOME), le
//! seul moyen d'envoyer des touches sous Wayland sans droits particuliers.
//!
//! Le bureau demande une fois l'autorisation de contrôler le clavier ; le
//! jeton rendu par `Start` (`persist_mode` 2 : jusqu'à révocation) est gardé
//! dans un fichier des données de l'app et redonné à la session suivante, qui
//! démarre alors sans boîte de dialogue. Une session par collage, fermée
//! aussitôt : rien ne reste « en contrôle » du bureau entre deux collages.
//! Bureau sans ce portail (Cinnamon, XFCE, bureaux wlroots…) :
//! [`PasteError::Unsupported`], l'appelant garde le presse-papiers et sa
//! notification.

use std::path::Path;
use std::time::Duration;

use ashpd::desktop::remote_desktop::{
    DeviceType, KeyState, NotifyKeyboardKeysymOptions, RemoteDesktop, SelectDevicesOptions,
    StartOptions,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode, ResponseError, Session};
use ashpd::enumflags2::BitFlags;
use tokio::time::Instant;

use super::PasteError;

/// Keysyms X11 de Ctrl (gauche) et de « v ».
const XK_CONTROL_L: i32 = 0xffe3;
const XK_V: i32 = 0x0076;

/// Ctrl+V : (keysym, pressé).
pub(super) const CHORD: [(i32, bool); 4] = [
    (XK_CONTROL_L, true),
    (XK_V, true),
    (XK_V, false),
    (XK_CONTROL_L, false),
];

/// Pause entre deux touches : le bureau les traite dans l'ordre.
const KEY_GAP: Duration = Duration::from_millis(12);
/// Réponse du bureau au-delà de laquelle une boîte d'autorisation a été
/// montrée (avec un jeton valide, `Start` répond aussitôt).
const ASKED_AFTER: Duration = Duration::from_millis(600);
/// Après une autorisation donnée à l'instant : le temps que la boîte se
/// ferme et que l'application d'avant retrouve le focus.
const FOCUS_BACK: Duration = Duration::from_millis(500);
/// Attente maximale du bureau, boîte d'autorisation comprise.
const TIMEOUT: Duration = Duration::from_secs(120);

/// Un collage à la fois (correction et dictée ont chacune leur
/// presse-papiers, mais un seul fichier de jeton, à usage unique).
static PASTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Colle le presse-papiers dans l'application au premier plan ;
/// `token_file` : fichier du jeton d'autorisation (lu, puis récrit).
pub(super) async fn paste(token_file: Option<&Path>) -> Result<(), PasteError> {
    let _turn = PASTING.lock().await;
    match tokio::time::timeout(TIMEOUT, send(token_file)).await {
        Ok(result) => result,
        Err(_) => Err(PasteError::Other("the desktop did not answer".into())),
    }
}

async fn send(token_file: Option<&Path>) -> Result<(), PasteError> {
    let portal = RemoteDesktop::new().await.map_err(failed)?;
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await
        .map_err(failed)?;
    let result = keys(&portal, &session, token_file).await;
    // Fermée dans tous les cas : pas de session de contrôle laissée ouverte.
    let _ = session.close().await;
    result
}

async fn keys(
    portal: &RemoteDesktop,
    session: &Session<RemoteDesktop>,
    token_file: Option<&Path>,
) -> Result<(), PasteError> {
    let saved = token_file.and_then(read_token);
    let options = SelectDevicesOptions::default()
        .set_devices(BitFlags::from(DeviceType::Keyboard))
        .set_persist_mode(PersistMode::ExplicitlyRevoked)
        .set_restore_token(saved.as_deref());
    portal
        .select_devices(session, options)
        .await
        .map_err(failed)?
        .response()
        .map_err(failed)?;
    let asked = Instant::now();
    let started = portal
        .start(session, None, StartOptions::default())
        .await
        .map_err(failed)?
        .response()
        .map_err(failed)?;
    if !started.devices().contains(DeviceType::Keyboard) {
        return Err(PasteError::Refused);
    }
    // Jeton à usage unique : celui de cette session sert à la suivante.
    if let (Some(path), Some(token)) = (token_file, started.restore_token()) {
        write_token(path, token);
    }
    if saved.is_none() || asked.elapsed() >= ASKED_AFTER {
        tokio::time::sleep(FOCUS_BACK).await;
    }
    for (keysym, pressed) in CHORD {
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        portal
            .notify_keyboard_keysym(
                session,
                keysym,
                state,
                NotifyKeyboardKeysymOptions::default(),
            )
            .await
            .map_err(failed)?;
        tokio::time::sleep(KEY_GAP).await;
    }
    Ok(())
}

/// Jeton gardé ; `None` s'il n'y en a pas (première fois, fichier illisible).
fn read_token(path: &Path) -> Option<String> {
    let token = std::fs::read_to_string(path).ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_owned())
}

/// Garde le jeton ; un échec d'écriture ne coûte qu'une nouvelle demande
/// d'autorisation.
fn write_token(path: &Path, token: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::fs::write(path, token) {
        eprintln!("chatonlm: paste permission not saved: {error}");
    }
}

/// Sens d'un échec du portail : refus de l'utilisateur, portail absent du
/// bureau (pas de service, ou pas cette interface : noms d'erreurs D-Bus
/// cherchés dans le détail, comme pour les raccourcis globaux), ou autre.
fn failed(error: ashpd::Error) -> PasteError {
    if matches!(error, ashpd::Error::Response(ResponseError::Cancelled)) {
        return PasteError::Refused;
    }
    let missing = matches!(
        error,
        ashpd::Error::PortalNotFound(_) | ashpd::Error::RequiresVersion(..)
    ) || {
        let detail = format!("{error:?}");
        [
            "ServiceUnknown",
            "NameHasNoOwner",
            "UnknownInterface",
            "UnknownMethod",
        ]
        .iter()
        .any(|name| detail.contains(name))
    };
    if missing {
        PasteError::Unsupported
    } else {
        PasteError::Other(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chord_is_control_v_released_in_reverse() {
        assert_eq!(
            CHORD,
            [(0xffe3, true), (0x76, true), (0x76, false), (0xffe3, false)]
        );
    }

    #[test]
    fn the_permission_token_is_kept_in_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desktop").join("paste.token");
        assert_eq!(read_token(&path), None);
        write_token(&path, "token-1");
        assert_eq!(read_token(&path).as_deref(), Some("token-1"));
        // Remplacé à chaque session ; blanc ou vide : pas de jeton.
        write_token(&path, " token-2\n");
        assert_eq!(read_token(&path).as_deref(), Some("token-2"));
        write_token(&path, " \n");
        assert_eq!(read_token(&path), None);
    }

    #[test]
    fn portal_failures_are_sorted() {
        assert_eq!(
            failed(ashpd::Error::Response(ResponseError::Cancelled)),
            PasteError::Refused
        );
        let missing = ashpd::Error::Zbus(zbus::Error::FDO(Box::new(
            zbus::fdo::Error::ServiceUnknown("org.freedesktop.portal.Desktop".into()),
        )));
        assert_eq!(failed(missing), PasteError::Unsupported);
        let no_interface = ashpd::Error::Zbus(zbus::Error::FDO(Box::new(
            zbus::fdo::Error::UnknownInterface("org.freedesktop.portal.RemoteDesktop".into()),
        )));
        assert_eq!(failed(no_interface), PasteError::Unsupported);
        assert!(matches!(
            failed(ashpd::Error::Response(ResponseError::Other)),
            PasteError::Other(_)
        ));
        assert!(matches!(
            failed(ashpd::Error::NoResponse),
            PasteError::Other(_)
        ));
    }
}
