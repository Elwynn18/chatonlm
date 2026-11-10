//! Scénarios du lot L9 (contrat 102 § 1.5) : noms `chatonlm-<nom>` propres
//! à ce lot ; un nom unique dans tous les lots et qui ne contient aucun nom
//! intégré (`scenario.rs`, cherché d'abord) : essai du socle.

use std::time::Duration;

use super::{ChatCall, Custom, Script, Step};

/// Réponse commencée (un morceau), puis plus rien tant que le client reste :
/// une réponse « en cours » pour l'éditeur (`chat.get`) et Easy Link.
pub const EDITOR_STALL: Custom = Custom {
    name: "chatonlm-editor-stall",
    chat: stall,
    infill: None,
    load_ms: None,
    fail_load: false,
};

/// Réponse lente qui finit : douze morceaux à 250 ms d'écart (3 s
/// environ), le temps d'agir pendant qu'elle s'écrit.
pub const DRAWL: Custom = Custom {
    name: "chatonlm-l9-drawl",
    chat: drawl,
    infill: None,
    load_ms: None,
    fail_load: false,
};

pub const SCENARIOS: &[Custom] = &[EDITOR_STALL, DRAWL];

fn stall(_: &ChatCall) -> Script {
    vec![
        Step::Content("Partial answer".into()),
        Step::Wait(Duration::from_secs(600)),
    ]
}

fn drawl(_: &ChatCall) -> Script {
    (1..=12)
        .flat_map(|part| {
            [
                Step::Wait(Duration::from_millis(250)),
                Step::Content(format!("Part {part} of a slow answer. ")),
            ]
        })
        .collect()
}
