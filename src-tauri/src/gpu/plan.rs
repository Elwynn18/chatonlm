//! Plan du chef de la carte graphique (logique pure, essayée sans Ollama) :
//! où en sont le modèle de la conversation et celui de l'autocomplétion,
//! s'ils tiennent ensemble, et pourquoi l'autocomplétion attend.
//!
//! Décision par CONSTAT d'abord (`/api/ps` : présents, `size_vram` contre
//! `size`) ; l'estimation (`hub/fit.rs`) ne sert qu'à prévoir quand le
//! constat manque (un modèle pas encore chargé). Rien ici ne lit les
//! variables d'Ollama : la même logique vaut pour l'Ollama de la machine.

use serde::Serialize;

use crate::ollama::running::{RunningModel, same_model};

const GIB: u64 = 1024 * 1024 * 1024;

/// Part de la VRAM utilisable (comme `hub/fit.rs`).
const VRAM_SHARE: f64 = 0.9;
/// RAM laissée au système (comme `hub/fit.rs`).
const SYSTEM_RAM: u64 = 6 * GIB;
/// Ollama ne charge un modèle de plus que si sa prévision tient sous 80 % de
/// la mémoire libre (`sched.go`).
const FREE_SHARE: f64 = 0.8;

/// Les deux modèles, ensemble ou non.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// Les deux tiennent ensemble (constaté, ou prévu).
    Together,
    /// L'un chasse l'autre : un seul à la fois.
    Exclusive,
    /// Le modèle d'autocomplétion ne tient pas, même seul.
    DoesNotFit,
    /// Rien ne permet de le dire.
    Unknown,
}

/// Pourquoi l'autocomplétion attend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    /// La conversation occupe la carte : un seul modèle à la fois.
    Exclusive,
    /// Une réponse de conversation s'écrit (calcul partagé).
    ChatBusy,
    /// Le modèle ne tient pas dans la mémoire.
    DoesNotFit,
    /// Modèle absent du serveur.
    NotInstalled,
    /// Serveur ailleurs que sur cette machine, ou modèle du cloud.
    NotLocal,
    /// Modèle pas en mémoire (l'autocomplétion ne le charge jamais).
    NotLoaded,
}

impl Reason {
    pub fn code(self) -> &'static str {
        match self {
            Reason::Exclusive => "exclusive",
            Reason::ChatBusy => "chatBusy",
            Reason::DoesNotFit => "doesNotFit",
            Reason::NotInstalled => "notInstalled",
            Reason::NotLocal => "notLocal",
            Reason::NotLoaded => "notLoaded",
        }
    }
}

/// Mémoire de la machine du serveur : VRAM s'il y a une carte graphique
/// reconnue, sinon RAM (`gpu: false`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub total: u64,
    /// Inconnue pour une carte AMD sous Windows.
    pub free: Option<u64>,
    pub gpu: bool,
}

impl Memory {
    /// Mémoire où un modèle tient sans déborder.
    fn usable(self) -> u64 {
        if self.gpu {
            (self.total as f64 * VRAM_SHARE) as u64
        } else {
            self.total.saturating_sub(SYSTEM_RAM)
        }
    }
}

/// Un modèle du plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Slot {
    pub model: String,
    pub loaded: bool,
    /// Taille en mémoire (`/api/ps`), 0 s'il n'est pas chargé.
    pub size_bytes: u64,
    pub vram_bytes: u64,
    /// Estimation avant chargement (`hub/fit.rs`), si connue.
    pub estimated_bytes: Option<u64>,
    /// Chargé en partie sur le processeur (`size_vram < size` sur une
    /// machine à carte graphique).
    pub partial_gpu: bool,
    /// En mémoire avec d'autres options que celles de l'autocomplétion
    /// (contexte) : une proposition le rechargerait ; « Préparer » le
    /// recharge. Pas transmis à l'éditeur (raison `notLoaded`).
    #[serde(skip)]
    pub stale: bool,
}

/// Plan rendu par `gpu.status` et `gpu.changed`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuPlan {
    pub mode: Mode,
    pub chat: Option<Slot>,
    pub completion: Option<Slot>,
    pub memory: Option<Memory>,
    /// Une réponse de conversation s'écrit.
    pub chat_busy: bool,
    /// Pourquoi l'autocomplétion attend ; absent : prête.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
}

/// Ce que le chef sait au moment de décider.
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts<'a> {
    /// `/api/ps` ; `None` : illisible.
    pub running: Option<&'a [RunningModel]>,
    /// Modèle de la conversation (aucun pour un modèle du cloud).
    pub chat: Option<&'a str>,
    /// Modèle d'autocomplétion choisi par l'éditeur.
    pub completion: Option<&'a str>,
    pub memory: Option<Memory>,
    pub chat_estimate: Option<u64>,
    pub completion_estimate: Option<u64>,
    /// Le modèle d'autocomplétion est en mémoire avec d'autres options.
    pub completion_stale: bool,
    pub busy: bool,
    pub local: bool,
    /// Constaté pour ces deux modèles : charger l'un a chassé l'autre.
    pub seen_exclusive: bool,
}

fn slot(
    running: Option<&[RunningModel]>,
    model: &str,
    estimate: Option<u64>,
    gpu: Option<bool>,
) -> Slot {
    let found = running
        .unwrap_or_default()
        .iter()
        .find(|loaded| same_model(&loaded.name, model));
    let (size, vram) = found.map_or((0, 0), |loaded| (loaded.size, loaded.size_vram));
    // Sans carte reconnue (processeur seul, serveur distant), seul un
    // `size_vram` non nul et plus petit dit un chargement partagé.
    let partial = found.is_some()
        && vram < size
        && match gpu {
            Some(gpu) => gpu,
            None => vram > 0,
        };
    Slot {
        model: model.to_owned(),
        loaded: found.is_some(),
        size_bytes: size,
        vram_bytes: vram,
        estimated_bytes: estimate,
        partial_gpu: partial,
        stale: false,
    }
}

/// Les deux tailles tiennent ensemble ; `None` sans estimation ou sans
/// mémoire connue.
fn both_fit(a: Option<u64>, b: Option<u64>, memory: Option<Memory>) -> Option<bool> {
    let (a, b, memory) = (a?, b?, memory?);
    Some(a.saturating_add(b) <= memory.usable())
}

/// Un modèle de `estimate` octets se chargerait à côté de `loaded` octets
/// déjà en mémoire (règle des 80 % du libre d'Ollama quand le libre se lit).
fn fits_beside(loaded: u64, estimate: Option<u64>, memory: Option<Memory>) -> Option<bool> {
    let (estimate, memory) = (estimate?, memory?);
    Some(match memory.free {
        Some(free) if memory.gpu => estimate as f64 <= free as f64 * FREE_SHARE,
        _ => loaded.saturating_add(estimate) <= memory.usable(),
    })
}

fn too_big_alone(estimate: Option<u64>, memory: Option<Memory>) -> bool {
    matches!((estimate, memory), (Some(estimate), Some(memory)) if estimate > memory.usable())
}

fn predicted(fit: Option<bool>) -> Mode {
    match fit {
        Some(true) => Mode::Together,
        Some(false) => Mode::Exclusive,
        None => Mode::Unknown,
    }
}

/// Plan d'après les faits.
pub fn decide(facts: &Facts) -> GpuPlan {
    let gpu = facts.memory.map(|memory| memory.gpu);
    let chat = facts
        .chat
        .map(|model| slot(facts.running, model, facts.chat_estimate, gpu));
    let completion = facts.completion.map(|model| {
        let mut slot = slot(facts.running, model, facts.completion_estimate, gpu);
        slot.stale = slot.loaded && facts.completion_stale;
        slot
    });
    let same = matches!((facts.chat, facts.completion), (Some(a), Some(b)) if same_model(a, b));
    let mode = match (&completion, facts.running) {
        (None, _) | (_, None) => Mode::Unknown,
        (Some(c), Some(_)) => {
            // La conversation dans un autre modèle, s'il y en a une.
            let other = chat.as_ref().filter(|_| !same);
            let other_loaded = other.is_some_and(|chat| chat.loaded);
            if same || other.is_none() {
                // Seul en mémoire : il tient ou non.
                if c.partial_gpu || (!c.loaded && too_big_alone(c.estimated_bytes, facts.memory)) {
                    Mode::DoesNotFit
                } else if c.loaded {
                    Mode::Together
                } else {
                    match (c.estimated_bytes, facts.memory) {
                        (Some(_), Some(_)) => Mode::Together,
                        _ => Mode::Unknown,
                    }
                }
            } else if let Some(chat) = other {
                if c.loaded && other_loaded {
                    if c.partial_gpu || chat.partial_gpu {
                        Mode::Exclusive
                    } else {
                        Mode::Together
                    }
                } else if c.loaded {
                    if c.partial_gpu {
                        Mode::DoesNotFit
                    } else if facts.seen_exclusive {
                        Mode::Exclusive
                    } else {
                        predicted(both_fit(
                            Some(c.size_bytes),
                            chat.estimated_bytes,
                            facts.memory,
                        ))
                    }
                } else if too_big_alone(c.estimated_bytes, facts.memory) {
                    Mode::DoesNotFit
                } else if facts.seen_exclusive {
                    Mode::Exclusive
                } else if other_loaded {
                    let loaded = chat.vram_bytes.max(chat.size_bytes);
                    predicted(fits_beside(loaded, c.estimated_bytes, facts.memory))
                } else {
                    predicted(both_fit(
                        c.estimated_bytes,
                        chat.estimated_bytes,
                        facts.memory,
                    ))
                }
            } else {
                Mode::Unknown
            }
        }
    };
    let chat_loaded = chat.as_ref().is_some_and(|chat| chat.loaded) && !same;
    let reason = if !facts.local {
        Some(Reason::NotLocal)
    } else if facts.busy {
        Some(Reason::ChatBusy)
    } else {
        match &completion {
            None => Some(Reason::NotLoaded),
            Some(c) if c.loaded && !c.stale => None,
            Some(_) if mode == Mode::DoesNotFit => Some(Reason::DoesNotFit),
            // Inconnu, la conversation en mémoire : « Préparer » ne charge
            // pas à l'aveugle (il pourrait la chasser).
            Some(_) if matches!(mode, Mode::Exclusive | Mode::Unknown) && chat_loaded => {
                Some(Reason::Exclusive)
            }
            Some(_) => Some(Reason::NotLoaded),
        }
    };
    GpuPlan {
        mode,
        chat,
        completion,
        memory: facts.memory,
        chat_busy: facts.busy,
        reason,
    }
}

/// Que faire d'une demande « Préparer » (`completion.prepare`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Déjà en mémoire.
    Ready,
    /// À charger.
    Load,
    Paused(Reason),
}

/// Préparer : charger seulement si les deux tiennent ensemble, ou si rien
/// d'autre (la conversation) n'est en mémoire ; jamais pendant une réponse.
/// Déjà en mémoire avec d'autres options : rechargé aux mêmes conditions.
pub fn prepare_step(plan: &GpuPlan) -> Step {
    if let Some(reason @ (Reason::NotLocal | Reason::ChatBusy)) = plan.reason {
        return Step::Paused(reason);
    }
    let Some(completion) = &plan.completion else {
        return Step::Paused(Reason::NotLoaded);
    };
    if completion.loaded && !completion.stale {
        return Step::Ready;
    }
    let chat_loaded = plan
        .chat
        .as_ref()
        .is_some_and(|chat| chat.loaded && !same_model(&chat.model, &completion.model));
    match plan.mode {
        Mode::DoesNotFit => Step::Paused(Reason::DoesNotFit),
        // Inconnu (mémoire ou estimation illisible) : seulement si rien
        // d'autre n'est en mémoire.
        Mode::Exclusive | Mode::Unknown if chat_loaded => Step::Paused(Reason::Exclusive),
        _ => Step::Load,
    }
}

/// Constat après le chargement du modèle d'autocomplétion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loaded {
    /// Tout tient : prêt.
    Ready,
    /// Il a chassé la conversation (ou l'a poussée sur le processeur) : il
    /// est déchargé ; `reload_chat` : la conversation, chassée, est
    /// rechargée.
    Exclusive { reload_chat: bool },
    /// Seul, il déborde sur le processeur : déchargé.
    DoesNotFit,
    /// Absent après le chargement (chassé aussitôt).
    Missing,
}

/// Compare les plans d'avant et d'après le chargement.
pub fn after_load(before: &GpuPlan, after: &GpuPlan) -> Loaded {
    let Some(completion) = after.completion.as_ref() else {
        return Loaded::Missing;
    };
    if !completion.loaded {
        return Loaded::Missing;
    }
    let other = |plan: &GpuPlan| {
        plan.chat
            .as_ref()
            .filter(|chat| !same_model(&chat.model, &completion.model))
            .cloned()
    };
    let (chat_before, chat_after) = (other(before), other(after));
    let was_loaded = chat_before.as_ref().is_some_and(|chat| chat.loaded);
    let still_loaded = chat_after.as_ref().is_some_and(|chat| chat.loaded);
    let chat_pushed = chat_after
        .as_ref()
        .is_some_and(|chat| chat.loaded && chat.partial_gpu)
        && !chat_before.as_ref().is_some_and(|chat| chat.partial_gpu);
    if was_loaded && !still_loaded {
        Loaded::Exclusive { reload_chat: true }
    } else if (completion.partial_gpu && still_loaded) || chat_pushed {
        Loaded::Exclusive { reload_chat: false }
    } else if completion.partial_gpu {
        Loaded::DoesNotFit
    } else {
        Loaded::Ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn running(name: &str, size: u64, vram: u64) -> RunningModel {
        RunningModel {
            name: name.into(),
            size,
            size_vram: vram,
            context_length: Some(4096),
            expires_at: None,
            kept: false,
            parameter_size: String::new(),
            quantization_level: String::new(),
        }
    }

    fn gpu(total_gib: u64, free_gib: Option<u64>) -> Option<Memory> {
        Some(Memory {
            total: total_gib * GIB,
            free: free_gib.map(|free| free * GIB),
            gpu: true,
        })
    }

    fn facts<'a>(loaded: &'a [RunningModel]) -> Facts<'a> {
        Facts {
            running: Some(loaded),
            chat: Some("qwen3:8b"),
            completion: Some("qwen2.5-coder:1.5b-base"),
            memory: gpu(24, Some(16)),
            chat_estimate: Some(6 * GIB),
            completion_estimate: Some(1500 * MIB),
            completion_stale: false,
            busy: false,
            local: true,
            seen_exclusive: false,
        }
    }

    #[test]
    fn both_loaded_in_vram_are_together_and_ready() {
        let loaded = [
            running("qwen3:8b", 6 * GIB, 6 * GIB),
            running("qwen2.5-coder:1.5b-base", GIB, GIB),
        ];
        let plan = decide(&facts(&loaded));
        assert_eq!(plan.mode, Mode::Together);
        assert_eq!(plan.reason, None);
        let completion = plan.completion.unwrap();
        assert!(completion.loaded && !completion.partial_gpu);
        assert_eq!(completion.size_bytes, GIB);
        assert!(plan.chat.unwrap().loaded);
        assert_eq!(
            prepare_step(&decide(&facts(&loaded))),
            Step::Ready,
            "already loaded: nothing to load"
        );
    }

    #[test]
    fn a_split_model_makes_them_exclusive() {
        // La conversation poussée en partie sur le processeur.
        let loaded = [
            running("qwen3:8b", 6 * GIB, 5 * GIB),
            running("qwen2.5-coder:1.5b-base", GIB, GIB),
        ];
        assert_eq!(decide(&facts(&loaded)).mode, Mode::Exclusive);
        // L'autocomplétion partagée, la conversation entière.
        let loaded = [
            running("qwen3:8b", 6 * GIB, 6 * GIB),
            running("qwen2.5-coder:1.5b-base", GIB, GIB / 2),
        ];
        let plan = decide(&facts(&loaded));
        assert_eq!(plan.mode, Mode::Exclusive);
        // Chargée, elle sert quand même.
        assert_eq!(plan.reason, None);
    }

    #[test]
    fn the_estimate_predicts_before_loading() {
        // Conversation chargée, 16 Gio libres : 1,5 Gio tient à côté.
        let loaded = [running("qwen3:8b", 6 * GIB, 6 * GIB)];
        let plan = decide(&facts(&loaded));
        assert_eq!(plan.mode, Mode::Together);
        assert_eq!(plan.reason, Some(Reason::NotLoaded));
        assert_eq!(prepare_step(&plan), Step::Load);
        // 1 Gio libre seulement : 80 % du libre ne suffit pas.
        let tight = Facts {
            memory: gpu(8, Some(1)),
            ..facts(&loaded)
        };
        let plan = decide(&tight);
        assert_eq!(plan.mode, Mode::Exclusive);
        assert_eq!(plan.reason, Some(Reason::Exclusive));
        assert_eq!(prepare_step(&plan), Step::Paused(Reason::Exclusive));
        // Libre inconnu (AMD sous Windows) : total utilisable moins le chargé.
        let unknown_free = Facts {
            memory: gpu(10, None),
            ..facts(&loaded)
        };
        assert_eq!(decide(&unknown_free).mode, Mode::Together);
        let small = Facts {
            memory: gpu(7, None),
            ..facts(&loaded)
        };
        assert_eq!(decide(&small).mode, Mode::Exclusive);
        // Rien de chargé : les deux estimations contre la mémoire.
        let nothing = Facts {
            memory: gpu(6, None),
            ..facts(&[])
        };
        let plan = decide(&nothing);
        assert_eq!(plan.mode, Mode::Exclusive);
        // Rien d'autre en mémoire : préparer charge quand même.
        assert_eq!(plan.reason, Some(Reason::NotLoaded));
        assert_eq!(prepare_step(&plan), Step::Load);
    }

    #[test]
    fn a_model_too_big_alone_does_not_fit() {
        let big = Facts {
            completion_estimate: Some(9 * GIB),
            memory: gpu(8, Some(8)),
            ..facts(&[])
        };
        let plan = decide(&big);
        assert_eq!(plan.mode, Mode::DoesNotFit);
        assert_eq!(plan.reason, Some(Reason::DoesNotFit));
        assert_eq!(prepare_step(&plan), Step::Paused(Reason::DoesNotFit));
        // Chargé seul en partie sur le processeur : ne tient pas non plus.
        let loaded = [running("qwen2.5-coder:1.5b-base", 2 * GIB, GIB)];
        let plan = decide(&Facts {
            chat: None,
            ..facts(&loaded)
        });
        assert_eq!(plan.mode, Mode::DoesNotFit);
        assert_eq!(plan.reason, None, "loaded: it still serves");
    }

    #[test]
    fn without_facts_the_mode_is_unknown() {
        // `/api/ps` illisible.
        let plan = decide(&Facts {
            running: None,
            ..facts(&[])
        });
        assert_eq!(plan.mode, Mode::Unknown);
        assert_eq!(plan.reason, Some(Reason::NotLoaded));
        // Ni estimation ni mémoire (serveur distant sans constat).
        let plan = decide(&Facts {
            memory: None,
            completion_estimate: None,
            ..facts(&[])
        });
        assert_eq!(plan.mode, Mode::Unknown);
        assert_eq!(prepare_step(&plan), Step::Load);
        // Aucun modèle d'autocomplétion choisi.
        let plan = decide(&Facts {
            completion: None,
            ..facts(&[])
        });
        assert_eq!(plan.mode, Mode::Unknown);
        assert!(plan.completion.is_none());
    }

    /// Rapport 92, M7 : sans moyen de prévoir, la conversation en mémoire
    /// n'est jamais risquée ; rien d'autre en mémoire : chargé.
    #[test]
    fn an_unknown_plan_never_loads_beside_the_chat() {
        let loaded = [running("qwen3:8b", 6 * GIB, 6 * GIB)];
        for unknown in [
            Facts {
                memory: None,
                ..facts(&loaded)
            },
            Facts {
                completion_estimate: None,
                ..facts(&loaded)
            },
        ] {
            let plan = decide(&unknown);
            assert_eq!(plan.mode, Mode::Unknown);
            assert_eq!(plan.reason, Some(Reason::Exclusive));
            assert_eq!(prepare_step(&plan), Step::Paused(Reason::Exclusive));
        }
        let plan = decide(&Facts {
            memory: None,
            ..facts(&[])
        });
        assert_eq!(plan.mode, Mode::Unknown);
        assert_eq!(prepare_step(&plan), Step::Load);
    }

    /// Rapport 92, M2 : en mémoire avec un autre contexte, il ne sert pas
    /// (une proposition le rechargerait) ; « Préparer » le recharge si le
    /// plan le permet.
    #[test]
    fn a_model_loaded_with_other_options_is_not_ready() {
        let loaded = [
            running("qwen3:8b", 6 * GIB, 6 * GIB),
            running("qwen2.5-coder:1.5b-base", GIB, GIB),
        ];
        let stale = Facts {
            completion_stale: true,
            ..facts(&loaded)
        };
        let plan = decide(&stale);
        assert!(plan.completion.as_ref().unwrap().loaded);
        assert_eq!(plan.reason, Some(Reason::NotLoaded));
        assert_eq!(prepare_step(&plan), Step::Load);
        // Pas transmis à l'éditeur.
        let value = serde_json::to_value(&plan).unwrap();
        assert!(value["completion"].get("stale").is_none(), "{value}");
        // Pas en mémoire : rien de périmé.
        let plan = decide(&Facts {
            completion_stale: true,
            ..facts(&loaded[..1])
        });
        assert!(!plan.completion.unwrap().stale);
    }

    #[test]
    fn a_reply_or_a_remote_server_pauses_completion() {
        let loaded = [running("qwen2.5-coder:1.5b-base", GIB, GIB)];
        let plan = decide(&Facts {
            busy: true,
            ..facts(&loaded)
        });
        assert!(plan.chat_busy);
        assert_eq!(plan.reason, Some(Reason::ChatBusy));
        assert_eq!(prepare_step(&plan), Step::Paused(Reason::ChatBusy));
        let plan = decide(&Facts {
            local: false,
            ..facts(&loaded)
        });
        assert_eq!(plan.reason, Some(Reason::NotLocal));
        assert_eq!(prepare_step(&plan), Step::Paused(Reason::NotLocal));
    }

    #[test]
    fn an_eviction_seen_once_stays_exclusive() {
        let loaded = [running("qwen3:8b", 6 * GIB, 6 * GIB)];
        let plan = decide(&Facts {
            seen_exclusive: true,
            ..facts(&loaded)
        });
        assert_eq!(plan.mode, Mode::Exclusive);
        assert_eq!(prepare_step(&plan), Step::Paused(Reason::Exclusive));
        // Conversation partie : l'autocomplétion peut se charger seule.
        let plan = decide(&Facts {
            seen_exclusive: true,
            ..facts(&[])
        });
        assert_eq!(plan.mode, Mode::Exclusive);
        assert_eq!(plan.reason, Some(Reason::NotLoaded));
        assert_eq!(prepare_step(&plan), Step::Load);
        // Les deux de nouveau ensemble, entiers : constat qui prime.
        let both = [
            running("qwen3:8b", 6 * GIB, 6 * GIB),
            running("qwen2.5-coder:1.5b-base", GIB, GIB),
        ];
        let plan = decide(&Facts {
            seen_exclusive: true,
            ..facts(&both)
        });
        assert_eq!(plan.mode, Mode::Together);
    }

    #[test]
    fn the_same_model_for_both_is_together() {
        let loaded = [running("qwen2.5-coder:7b", 5 * GIB, 5 * GIB)];
        let plan = decide(&Facts {
            chat: Some("qwen2.5-coder:7b"),
            completion: Some("qwen2.5-coder:7b"),
            ..facts(&loaded)
        });
        assert_eq!(plan.mode, Mode::Together);
        assert_eq!(plan.reason, None);
    }

    #[test]
    fn a_processor_only_machine_is_never_partial() {
        let ram = Some(Memory {
            total: 32 * GIB,
            free: Some(20 * GIB),
            gpu: false,
        });
        let loaded = [
            running("qwen3:8b", 6 * GIB, 0),
            running("qwen2.5-coder:1.5b-base", GIB, 0),
        ];
        let plan = decide(&Facts {
            memory: ram,
            ..facts(&loaded)
        });
        assert_eq!(plan.mode, Mode::Together);
        assert!(!plan.completion.unwrap().partial_gpu);
        // Machine inconnue : un `size_vram` nul ne dit rien.
        let plan = decide(&Facts {
            memory: None,
            ..facts(&loaded)
        });
        assert_eq!(plan.mode, Mode::Together);
    }

    #[test]
    fn a_load_is_judged_by_what_follows() {
        let before_loaded = [running("qwen3:8b", 6 * GIB, 6 * GIB)];
        let before = decide(&facts(&before_loaded));
        // Les deux, entiers : prêt.
        let both = [
            running("qwen3:8b", 6 * GIB, 6 * GIB),
            running("qwen2.5-coder:1.5b-base", GIB, GIB),
        ];
        assert_eq!(after_load(&before, &decide(&facts(&both))), Loaded::Ready);
        // La conversation chassée : exclusif, à recharger.
        let alone = [running("qwen2.5-coder:1.5b-base", GIB, GIB)];
        assert_eq!(
            after_load(&before, &decide(&facts(&alone))),
            Loaded::Exclusive { reload_chat: true }
        );
        // La conversation poussée sur le processeur.
        let pushed = [
            running("qwen3:8b", 6 * GIB, 4 * GIB),
            running("qwen2.5-coder:1.5b-base", GIB, GIB),
        ];
        assert_eq!(
            after_load(&before, &decide(&facts(&pushed))),
            Loaded::Exclusive { reload_chat: false }
        );
        // Seul et partagé : ne tient pas.
        let nothing = decide(&facts(&[]));
        let split = [running("qwen2.5-coder:1.5b-base", 2 * GIB, GIB)];
        assert_eq!(
            after_load(&nothing, &decide(&facts(&split))),
            Loaded::DoesNotFit
        );
        assert_eq!(after_load(&nothing, &nothing), Loaded::Missing);
    }

    #[test]
    fn the_plan_is_serialized_for_the_editor() {
        let loaded = [running("qwen2.5-coder:1.5b-base", GIB, GIB)];
        let plan = decide(&Facts {
            chat: None,
            ..facts(&loaded)
        });
        let value = serde_json::to_value(&plan).unwrap();
        assert_eq!(value["mode"], "together");
        assert_eq!(value["chat"], serde_json::Value::Null);
        assert_eq!(value["completion"]["model"], "qwen2.5-coder:1.5b-base");
        assert_eq!(value["completion"]["sizeBytes"], GIB);
        assert_eq!(value["completion"]["partialGpu"], false);
        assert_eq!(value["memory"]["total"], 24 * GIB);
        assert_eq!(value["chatBusy"], false);
        assert!(value.get("reason").is_none());
        let paused = decide(&Facts {
            busy: true,
            ..facts(&loaded)
        });
        assert_eq!(serde_json::to_value(&paused).unwrap()["reason"], "chatBusy");
    }
}
