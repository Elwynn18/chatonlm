//! Micro et sortie sous Linux : cpal sur ALSA (donc PipeWire par
//! pipewire-alsa). L'ouverture répond toujours, et un changement de
//! périphérique est suivi par PipeWire sans que le flux le voie.

use std::time::Duration;

/// Attente de l'ouverture d'un périphérique par son fil : sans limite.
pub(crate) const OPEN_WAIT: Option<Duration> = None;

/// Vrai si `error` est un refus du système à expliquer à l'utilisateur :
/// jamais ici (l'erreur d'ALSA est montrée telle quelle).
pub(crate) fn blocked(_error: &cpal::Error) -> bool {
    false
}

/// Vrai si le micro est à rouvrir après cette erreur du flux : jamais ici.
pub(crate) fn reopens(_kind: cpal::ErrorKind) -> bool {
    false
}
