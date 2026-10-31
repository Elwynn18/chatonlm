//! Dictée pilotée par l'extension VS Code (lot 4b, propre à ChatonLM) : le
//! micro et la transcription restent dans l'app ; le texte final revient à
//! l'éditeur, qui l'insère au curseur (jamais collé ni copié par l'app).
//!
//! - `dictation.start { glossary?: string[] }` → `{}` quand le micro est
//!   ouvert (après `dictation.state` `recording`). La dictée est inscrite
//!   dès la lecture de la requête, avant toute attente : un `dictation.stop`
//!   ou la fin de la connexion lus ensuite la trouvent toujours.
//! - Notifications : `dictation.state { state: "recording" | "transcribing" |
//!   "polishing" | "idle" }` (mêmes phases que la page ; `idle` en dernier,
//!   seulement après `recording`), `dictation.text { text }` (texte final,
//!   avant `idle`), `dictation.error { message }` (échec après l'ouverture du
//!   micro : rien entendu, transcription en échec ; avant `idle`).
//! - `dictation.stop {}` : fin de l'enregistrement, le texte suit ; avant
//!   l'ouverture du micro, rien ne s'ouvre et le démarrage répond -32800.
//!   `dictation.cancel {}` : abandon, rien n'est rendu. Les deux sont sans
//!   effet sans dictée de la connexion.
//! - Refus : -32032 `{ owner: "app" | "bridge" | "voiceMode", message }`
//!   (dictée de la page, du raccourci, d'une connexion, ou séance du mode
//!   vocal), -32031 modèle de transcription absent, -32033 `{ message }`
//!   (micro, machine incapable), -32800 arrêtée avant l'ouverture du micro.
//! - Le micro s'arrête à la fin de la connexion (même par une panique de sa
//!   tâche), au retrait de l'éditeur, à la coupure du pont, à la sortie de
//!   l'app, et par le bouton Stop du bandeau de la page.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use serde::Deserialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use super::connection::{Outbox, code};
use super::voice::voice_code;
use super::{Host, Shared};
use crate::dictation::editor::{
    DictationLink, DictationOwner, EditorEnd, EditorStartError, Opened, clean_glossary,
};
use crate::dictation::{Dictation, DictationPhase};
use crate::i18n::t;

/// La dictée de l'app, vue par le pont (remplacée dans les essais).
pub trait Dictate: Send + Sync + 'static {
    /// Inscrit la dictée aussitôt ; le démarrage finit quand le micro est
    /// ouvert.
    fn start(&self, link: Arc<dyn DictationLink>) -> Result<Opened, EditorStartError>;
    /// Fin de l'enregistrement de la connexion.
    fn stop(&self, connection: u64);
    /// Abandon de la dictée de la connexion.
    fn cancel(&self, connection: u64);
}

/// La dictée de l'app réelle.
pub struct AppDictation(pub AppHandle);

impl Dictate for AppDictation {
    fn start(&self, link: Arc<dyn DictationLink>) -> Result<Opened, EditorStartError> {
        match self.0.try_state::<Dictation>() {
            Some(dictation) => dictation.start_for_editor(&self.0, link),
            None => Err(EditorStartError::Cancelled),
        }
    }

    fn stop(&self, connection: u64) {
        if let Some(dictation) = self.0.try_state::<Dictation>() {
            dictation.stop_editor(&self.0, connection);
        }
    }

    fn cancel(&self, connection: u64) {
        if let Some(dictation) = self.0.try_state::<Dictation>() {
            dictation.cancel_editor(&self.0, connection);
        }
    }
}

/// Dictée tenue par une connexion : relaie ce que la dictée lui dit.
struct Dictating<H: Host> {
    shared: Weak<Shared<H>>,
    connection: u64,
    out: Outbox,
    glossary: Vec<String>,
    /// Une phase autre que `idle` a été envoyée à l'éditeur.
    told: AtomicBool,
}

impl<H: Host> DictationLink for Dictating<H> {
    fn connection(&self) -> u64 {
        self.connection
    }

    fn glossary(&self) -> Vec<String> {
        self.glossary.clone()
    }

    fn alive(&self) -> bool {
        self.shared
            .upgrade()
            .is_some_and(|shared| shared.is_open(self.connection))
    }

    fn phase(&self, phase: DictationPhase) {
        if phase == DictationPhase::Idle {
            if !self.told.swap(false, Ordering::SeqCst) {
                return;
            }
        } else {
            self.told.store(true, Ordering::SeqCst);
        }
        self.out
            .notify("dictation.state", json!({ "state": phase }));
    }

    fn ended(&self, end: EditorEnd) {
        match end {
            EditorEnd::Text(text) => self.out.notify("dictation.text", json!({ "text": text })),
            EditorEnd::Failed(message) => self
                .out
                .notify("dictation.error", json!({ "message": message })),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartParams {
    #[serde(default)]
    glossary: Option<Vec<String>>,
}

fn invalid(out: &Outbox, id: &Value) {
    out.error(id, code::INVALID_PARAMS, "invalid params", None);
}

/// Pas de paramètre (`{}` ou absent).
fn no_params(params: &Value) -> bool {
    params.is_null() || params.as_object().is_some_and(serde_json::Map::is_empty)
}

/// `dictation.start`, dans l'ordre de lecture (rien n'y attend) : dictée
/// inscrite, ou refus déjà répondu. Rend l'attente de l'ouverture du micro,
/// à finir par [`dictation_opened`].
pub(super) fn dictation_start<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    out: &Outbox,
    id: &Value,
    params: Value,
) -> Option<Opened> {
    let params = if params.is_null() {
        StartParams { glossary: None }
    } else {
        let Ok(params) = serde_json::from_value::<StartParams>(params) else {
            invalid(out, id);
            return None;
        };
        params
    };
    let link = Arc::new(Dictating {
        shared: Arc::downgrade(shared),
        connection,
        out: out.clone(),
        glossary: clean_glossary(&params.glossary.unwrap_or_default()),
        told: AtomicBool::new(false),
    });
    match shared.host.dictation().start(link) {
        Ok(opened) => Some(opened),
        Err(error) => {
            refuse(out, id, error);
            None
        }
    }
}

/// `dictation.start`, suite (dans sa tâche) : `{}` quand le micro est ouvert.
pub(super) async fn dictation_opened(out: &Outbox, id: &Value, opened: Opened) {
    match opened.await {
        Ok(Ok(())) => out.result(id, json!({})),
        Ok(Err(error)) => refuse(out, id, error),
        // Dictée abandonnée sans réponse (tâche arrêtée) : arrêtée.
        Err(_) => refuse(out, id, EditorStartError::Cancelled),
    }
}

fn refuse(out: &Outbox, id: &Value, error: EditorStartError) {
    match error {
        EditorStartError::Busy(owner) => {
            let (owner, message) = match owner {
                DictationOwner::App => ("app", t!("bridge.dictation.busyApp")),
                DictationOwner::Bridge => ("bridge", t!("bridge.dictation.busyBridge")),
                DictationOwner::VoiceMode => ("voiceMode", t!("dictation.voiceModeActive")),
            };
            out.error(
                id,
                voice_code::BUSY,
                "refused",
                Some(json!({ "owner": owner, "message": message })),
            );
        }
        EditorStartError::SpeechModelMissing => {
            out.error(id, voice_code::MISSING_SPEECH_MODEL, "refused", None);
        }
        EditorStartError::Failed(message) => out.error(
            id,
            voice_code::MICROPHONE,
            "refused",
            Some(json!({ "message": message })),
        ),
        EditorStartError::Cancelled => out.error(id, code::CANCELLED, "cancelled", None),
    }
}

/// `dictation.stop` (sans paramètre) : fin de l'enregistrement de la
/// connexion, le texte suit ; avant l'ouverture du micro, abandon.
pub(super) fn dictation_stop<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    out: &Outbox,
    id: &Value,
    params: &Value,
) {
    if !no_params(params) {
        return invalid(out, id);
    }
    shared.host.dictation().stop(connection);
    out.result(id, json!({}));
}

/// `dictation.cancel` (sans paramètre) : abandon, rien n'est rendu.
pub(super) fn dictation_cancel<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    out: &Outbox,
    id: &Value,
    params: &Value,
) {
    if !no_params(params) {
        return invalid(out, id);
    }
    shared.host.dictation().cancel(connection);
    out.result(id, json!({}));
}

/// Connexion fermée (fin, éditeur retiré, pont coupé) : sa dictée est
/// abandonnée, micro fermé.
pub(super) fn closed<H: Host>(shared: &Shared<H>, connection: u64) {
    shared.host.dictation().cancel(connection);
}
