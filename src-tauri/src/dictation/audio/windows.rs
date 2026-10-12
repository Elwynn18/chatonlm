//! Micro et sortie sous Windows : cpal sur WASAPI, mode partagé.
//!
//! - L'activation du périphérique par défaut (`ActivateAudioInterfaceAsync`)
//!   attend Windows sans limite : nos fils d'ouverture sont attendus
//!   [`OPEN_WAIT`] au plus.
//! - Micro coupé dans Paramètres > Confidentialité et sécurité > Microphone :
//!   Windows refuse l'ouverture (`E_ACCESSDENIED`), que cpal rend comme une
//!   erreur quelconque.
//! - Changement du périphérique par défaut (casque branché…) : cpal 0.18.2
//!   le signale (`StreamInvalidated`) sans rebrancher le flux.

use std::time::Duration;

/// Attente de l'ouverture d'un périphérique par son fil.
pub(crate) const OPEN_WAIT: Option<Duration> = Some(Duration::from_secs(8));

/// Réglages de confidentialité du micro (« Value » = `Allow` ou `Deny`) :
/// accès de l'appareil, accès de l'utilisateur, puis sous-clé des
/// applications de bureau.
const CONSENT: &str =
    r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

/// Vrai si `error` est le refus de la confidentialité de Windows : son code,
/// ou n'importe quel échec de l'ouverture alors que les réglages refusent le
/// micro (lus seulement après un échec : ils n'empêchent jamais d'essayer).
pub(crate) fn blocked(error: &cpal::Error) -> bool {
    error.kind() == cpal::ErrorKind::PermissionDenied
        || super::access_denied(&error.to_string())
        || refused_in_settings()
}

fn refused_in_settings() -> bool {
    let value = |root: &windows_registry::Key, path: &str| {
        root.open(path).and_then(|key| key.get_string("Value")).ok()
    };
    let desktop = format!(r"{CONSENT}\NonPackaged");
    super::consent_denied(&[
        value(windows_registry::LOCAL_MACHINE, CONSENT),
        value(windows_registry::CURRENT_USER, CONSENT),
        value(windows_registry::CURRENT_USER, &desktop),
    ])
}

/// Vrai si le micro est à rouvrir après cette erreur du flux : le
/// périphérique par défaut a changé et son remplaçant existe.
pub(crate) fn reopens(kind: cpal::ErrorKind) -> bool {
    kind == cpal::ErrorKind::StreamInvalidated
}
