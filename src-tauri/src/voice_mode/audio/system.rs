//! Écoute réelle : micro du système (cpal), Silero VAD, moniteur des
//! sorties de la lecture à voix haute. Build e2e : faux micro si
//! `CHATONLM_E2E_VOICE_MIC` est posée.

use std::sync::Arc;

use super::Ear;
use super::listener::{Listener, SystemMicrophone};
use super::vad::{Silero, SpeechDetector};

/// Écoute du système.
pub fn system_ear() -> Arc<dyn Ear> {
    #[cfg(feature = "e2e")]
    if let Some(value) = std::env::var_os(super::fake_mic::ENV) {
        let value = value.to_string_lossy().into_owned();
        let script = Arc::new(parking_lot::Mutex::new(None));
        return Arc::new(Listener {
            microphone: Arc::new(move |_: std::thread::Thread| {
                super::fake_mic::open(&value, &script)
            }),
            detector: Arc::new(detector),
            monitor: crate::tts::playback::monitor(),
        });
    }
    Arc::new(Listener {
        microphone: Arc::new(SystemMicrophone::open),
        detector: Arc::new(detector),
        monitor: crate::tts::playback::monitor(),
    })
}

fn detector() -> Result<Box<dyn SpeechDetector>, String> {
    Ok(Box::new(Silero::load()?))
}
