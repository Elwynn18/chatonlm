//! Scénarios du lot L5a (contrat 102 § 1.5) : noms `chatonlm-<nom>` propres
//! à ce lot ; un nom unique dans tous les lots et qui ne contient aucun nom
//! intégré (`scenario.rs`, cherché d'abord) : essai du socle.

use super::Custom;

/// Envois du chat.
pub mod chat;
/// Autocomplétion de l'éditeur (`/infill`) et visite guidée.
pub mod gpu;
/// Correction et nettoyage de la dictée.
pub mod proof;
/// Cours (onglet Study).
pub mod study;

pub const SCENARIOS: &[Custom] = &[
    chat::STUBBORN_THINK,
    chat::QUIET_TOOLS,
    chat::INSIST_TOOLS,
    chat::ROGUE,
    chat::SKETCH,
    chat::GIST,
    chat::PENSIVE,
    chat::MULL,
    chat::FETCH,
    gpu::FIM_LINES,
    gpu::FIM_WAIT,
    gpu::FIM_SLOW_LOAD,
    gpu::TOUR,
    gpu::TOUR_THINK,
    proof::PROOFER,
    proof::PROOFER_SLOW,
    proof::PROOFER_OFF,
    proof::PROOFER_THINK,
    proof::PROOFER_OPENED,
    proof::PROOFER_CUT,
    proof::PROOFER_WAIT,
    proof::POLISHER,
    proof::POLISHER_THINK,
    proof::POLISHER_POEM,
    study::STUDY,
    study::STUDY_SLOW_MODEL,
    study::STUDY_UNCLEAR,
    study::STUDY_INVALID,
    study::STUDY_STALL_MODEL,
];
