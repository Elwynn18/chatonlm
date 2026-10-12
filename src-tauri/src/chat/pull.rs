// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// Translated to Rust from Ollama (https://github.com/ollama/ollama),
// file app/ui/ui.go, commit 42e911bc3d05 (model download of the chat handler,
// isNetworkError, getError).
// Modified for ChatonLM: a stream that ends without "success" is a lost
// connection, not a success; only the offline case of getError is kept (no
// cloud errors); errors carry a ChatonLM error kind; for a Hugging Face name
// (hf.co/...) whose manifest could be read first, progress is the sum of all
// layers over the size of the manifest (the official app follows the largest
// layer only, so the bar of a vision model stood still at 100 % during its
// projector); any other name, or a manifest that can't be read, keeps the
// official largest layer; errors of Hugging Face's registry are rewritten
// (hub::errors).

//! Téléchargement d'un modèle absent du serveur (`POST /api/pull`), avant
//! l'envoi, comme l'app officielle (`app/ui/ui.go:833-887`), puis un dernier
//! événement complet. Écarts voulus :
//! - nom `hf.co/...` dont le manifeste se lit au registre de Hugging Face
//!   (`hub::manifest`, délai court) : progression = somme de toutes les
//!   couches sur la taille du manifeste, connue dès le départ (l'officiel
//!   suit la plus grosse couche seule : barre figée pendant le projecteur d'un
//!   modèle de vision, 25 à 37 % du total). Sans ce total, une somme des seules
//!   couches vues reculerait à l'arrivée du projecteur : tout autre nom
//!   (ollama.com), ou un manifeste illisible, garde donc la plus grosse couche
//!   de l'officiel ;
//! - les erreurs du registre de Hugging Face sont réécrites (`hub::errors`) ;
//! - un flux qui finit sans `success` est une connexion perdue (l'officiel le
//!   prend pour une réussite).
//!
//! Lecture des lignes et suivi de la progression ([`Tracker`]) partagés avec
//! les téléchargements du ModelHub (`hub::downloads`).

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tokio::sync::watch;

use super::stream::LineBuffer;
use super::{ChatErrorKind, ChatEvent, Outcome, cancelled, post, read_error};
use crate::hub::errors::{connection_lost, pull_error};
use crate::hub::manifest::{self, Manifest};
use crate::hub::names::{self, parse_hf_name};
use crate::i18n::t;

/// Délai de la lecture du manifeste avant le pull d'un nom `hf.co/...` : il
/// retarde le téléchargement, et seule la progression en dépend.
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Texte officiel d'un téléchargement impossible faute de réseau
/// (`app/ui/ui.go`, `getError`), dans la langue de l'interface.
pub fn offline() -> String {
    t!("downloads.offline")
}

/// Motifs d'une erreur réseau (`isNetworkError`, `app/ui/ui.go:594-615`).
pub(crate) const NETWORK_ERRORS: [&str; 11] = [
    "connection refused",
    "no such host",
    "timeout",
    "network is unreachable",
    "connection reset",
    "connection timed out",
    "temporary failure",
    "dial tcp",
    "i/o timeout",
    "context deadline exceeded",
    "broken pipe",
];

/// Ligne du flux (`api.ProgressResponse`, ou `{"error": ...}`).
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Progress {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub digest: String,
    #[serde(default)]
    pub total: u64,
    /// Absent tant qu'il vaut 0.
    #[serde(default)]
    pub completed: u64,
    #[serde(default)]
    pub error: Option<String>,
}

/// Couches vues, par empreinte, dans l'ordre d'arrivée : la progression est
/// la somme de leurs `completed` (chacun plafonné à son `total`). Ollama émet
/// ses lignes toutes les 60 ms : la dernière d'une couche peut rester sous son
/// total, et une petite couche (gabarit, configuration) n'a parfois jamais de
/// `completed` ; à `success`, chaque couche compte pour son total
/// ([`Layers::finish`]).
#[derive(Debug, Default)]
pub(crate) struct Layers {
    /// (empreinte, total, reçu).
    layers: Vec<(String, u64, u64)>,
}

impl Layers {
    /// Prend la ligne d'une couche. Les parts d'une couche finissent dans le
    /// désordre : `completed` ne recule pas.
    pub fn update(&mut self, digest: &str, total: u64, completed: u64) {
        match self.layers.iter_mut().find(|(seen, ..)| seen == digest) {
            Some((_, seen_total, seen_completed)) => {
                *seen_total = (*seen_total).max(total);
                *seen_completed = (*seen_completed).max(completed);
            }
            None => self.layers.push((digest.to_owned(), total, completed)),
        }
    }

    /// Octets reçus, toutes couches.
    pub fn completed(&self) -> u64 {
        self.layers
            .iter()
            .map(|(_, total, completed)| (*completed).min(*total))
            .sum()
    }

    /// Octets à recevoir, couches vues.
    pub fn total(&self) -> u64 {
        self.layers.iter().map(|(_, total, _)| *total).sum()
    }

    /// `success` : chaque couche est complète.
    pub fn finish(&mut self) {
        for (_, total, completed) in &mut self.layers {
            *completed = *total;
        }
    }
}

/// Progression montrée d'un téléchargement, `(reçu, total)`.
#[derive(Debug)]
pub(crate) enum Tracker {
    /// Taille connue d'avance (manifeste de Hugging Face, couches et
    /// configuration) : somme des couches sur ce total, qui ne bouge plus ;
    /// la part montrée ne recule jamais, projecteur compris. `shown` :
    /// dernière progression rendue (la première ligne d'une couche en rend
    /// une, même à 0).
    Summed {
        layers: Layers,
        total: u64,
        shown: Option<(u64, u64)>,
    },
    /// Comme l'officiel : la plus grosse couche seule (un digest dont `total`
    /// dépasse le plus grand vu la remplace), chacune de ses lignes telle
    /// quelle.
    Largest { digest: String, total: u64 },
}

impl Tracker {
    /// `known` : taille du manifeste lu avant le pull ; `None` : plus grosse
    /// couche.
    pub fn new(known: Option<u64>) -> Tracker {
        match known {
            Some(total) => Tracker::Summed {
                layers: Layers::default(),
                total,
                shown: None,
            },
            None => Tracker::Largest {
                digest: String::new(),
                total: 0,
            },
        }
    }

    /// Ligne d'une couche : `(reçu, total)` à montrer, ou `None` si rien ne
    /// change (somme) ou si ce n'est pas la plus grosse couche.
    pub fn update(&mut self, digest: &str, total: u64, completed: u64) -> Option<(u64, u64)> {
        match self {
            Tracker::Summed {
                layers,
                total: known,
                shown,
            } => {
                layers.update(digest, total, completed);
                let now = summed(layers, *known);
                (*shown != Some(now)).then(|| {
                    *shown = Some(now);
                    now
                })
            }
            Tracker::Largest {
                digest: largest,
                total: largest_total,
            } => {
                if total > *largest_total {
                    largest.clear();
                    largest.push_str(digest);
                    *largest_total = total;
                }
                (digest == largest.as_str()).then_some((completed, total))
            }
        }
    }

    /// `success` (ou vérification de l'empreinte) : taille montrée, désormais
    /// entièrement reçue.
    pub fn finish(&mut self) -> u64 {
        match self {
            Tracker::Summed { layers, total, .. } => {
                layers.finish();
                (*total).max(layers.total())
            }
            Tracker::Largest { total, .. } => *total,
        }
    }
}

/// Somme des couches sur le total connu (agrandi si les couches le
/// dépassent : jamais plus de reçu que de total).
fn summed(layers: &Layers, known: u64) -> (u64, u64) {
    let total = known.max(layers.total());
    (layers.completed().min(total), total)
}

/// Taille d'un `hf.co/...` d'après son manifeste au registre de Hugging Face
/// (`hf_url`), lu en [`MANIFEST_TIMEOUT`] au plus ; `None` pour un autre nom,
/// ou un manifeste illisible (Ollama dira lui-même ce qui ne va pas).
/// `Err` : annulé pendant la lecture.
pub(crate) async fn known_size(
    model: &str,
    hf_url: &str,
    timeout: Duration,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Option<u64>, ()> {
    let Some(name) = parse_hf_name(model) else {
        return Ok(None);
    };
    let read = tokio::select! {
        biased;
        () = cancelled(cancel) => return Err(()),
        read = manifest::fetch(hf_url, &name, timeout) => read,
    };
    Ok(match read {
        Manifest::Found { total } => Some(total),
        Manifest::Refused(_) | Manifest::Unexpected | Manifest::Unreachable(_) => None,
    })
}

/// Suite donnée à une ligne.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// Événement à transmettre, s'il y en a un.
    Progress(Option<ChatEvent>),
    /// `success` : dernier événement.
    Success(ChatEvent),
    /// Ligne `error`, avec le texte à montrer.
    Failed(String),
}

/// Progression du chat : un événement chaque fois que la part montrée change
/// (somme), ou à chaque ligne de la plus grosse couche (officiel).
#[derive(Debug)]
struct Shown {
    tracker: Tracker,
}

impl Shown {
    /// L'erreur est examinée en premier ; les lignes sans digest (statuts) et
    /// celles qui ne changent rien ne donnent aucun événement.
    fn step(&mut self, line: Progress) -> Step {
        if let Some(error) = line.error.filter(|error| !error.is_empty()) {
            return Step::Failed(pull_error(&error));
        }
        if line.status == "success" {
            let total = self.tracker.finish();
            return Step::Success(ChatEvent::Download {
                total,
                completed: total,
                done: true,
            });
        }
        if line.digest.is_empty() {
            return Step::Progress(None);
        }
        Step::Progress(
            self.tracker
                .update(&line.digest, line.total, line.completed)
                .map(|(completed, total)| ChatEvent::Download {
                    total,
                    completed,
                    done: false,
                }),
        )
    }
}

/// Lit une ligne. Illisible : morceau coupé (connexion perdue) si c'est la
/// dernière du flux (`ended`) et qu'elle suit d'autres lignes lisibles ou
/// commence un objet JSON ; sinon réponse d'un autre programme.
pub(crate) fn parse(
    line: &str,
    ended: bool,
    understood: bool,
) -> Result<Progress, (ChatErrorKind, String)> {
    serde_json::from_str(line).map_err(|error| {
        if ended && (understood || line.trim_start().starts_with('{')) {
            (ChatErrorKind::ConnectionLost, connection_lost(None))
        } else {
            (
                ChatErrorKind::Download,
                t!("ollama.unexpectedAnswer", error = error),
            )
        }
    })
}

/// Télécharge `model` en transmettant la progression ; [`Outcome::Done`] une
/// fois `success` reçu (après l'événement `download` final). Pas de délai
/// global ; l'annulation interrompt la requête comme la lecture. Un nom
/// `hf.co/...` fait d'abord lire son manifeste à Hugging Face.
pub(super) async fn pull(
    base: &str,
    model: &str,
    cancel: &mut watch::Receiver<bool>,
    emit: &impl Fn(ChatEvent),
) -> Outcome {
    pull_with(base, model, &names::hf_url(), cancel, emit).await
}

/// [`pull`], avec le registre de Hugging Face à `hf_url`.
async fn pull_with(
    base: &str,
    model: &str,
    hf_url: &str,
    cancel: &mut watch::Receiver<bool>,
    emit: &impl Fn(ChatEvent),
) -> Outcome {
    let Ok(known) = known_size(model, hf_url, MANIFEST_TIMEOUT, cancel).await else {
        return Outcome::Cancelled;
    };
    let body = json!({ "model": model, "stream": true });
    let mut response = match post(base, "/api/pull", &body, cancel).await {
        Ok(response) => response,
        Err(outcome) => return outcome,
    };
    if !response.status().is_success() {
        return match read_error(response, cancel).await {
            Ok((shown, _)) => Outcome::Failed(ChatErrorKind::Download, shown),
            Err(outcome) => outcome,
        };
    }
    let mut buffer = LineBuffer::default();
    let mut shown = Shown {
        tracker: Tracker::new(known),
    };
    // Au moins une ligne lisible reçue : une dernière ligne illisible est
    // alors un morceau coupé.
    let mut understood = false;
    loop {
        let chunk = tokio::select! {
            biased;
            () = cancelled(cancel) => return Outcome::Cancelled,
            chunk = response.chunk() => chunk,
        };
        let ended = matches!(chunk, Ok(None));
        let lines = match chunk {
            Ok(Some(bytes)) => buffer.push(&bytes),
            Ok(None) => buffer.finish().into_iter().collect(),
            Err(error) => {
                return Outcome::Failed(
                    ChatErrorKind::ConnectionLost,
                    connection_lost(Some(&error.without_url())),
                );
            }
        };
        for line in lines {
            let progress = match parse(&line, ended, understood) {
                Ok(progress) => progress,
                Err((kind, message)) => return Outcome::Failed(kind, message),
            };
            understood = true;
            match shown.step(progress) {
                Step::Progress(Some(event)) => emit(event),
                Step::Progress(None) => {}
                Step::Success(event) => {
                    emit(event);
                    return Outcome::Done;
                }
                Step::Failed(message) => return Outcome::Failed(ChatErrorKind::Download, message),
            }
        }
        if ended {
            // Fin du flux sans `success` : serveur arrêté ou proxy coupé.
            return Outcome::Failed(ChatErrorKind::ConnectionLost, connection_lost(None));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use fake_ollama::{FakeOllama, Mode};
    use parking_lot::Mutex;

    use super::*;

    fn line(text: &str) -> Progress {
        serde_json::from_str(text).unwrap()
    }

    fn progress(total: u64, completed: u64) -> Step {
        Step::Progress(Some(ChatEvent::Download {
            total,
            completed,
            done: false,
        }))
    }

    fn largest() -> Shown {
        Shown {
            tracker: Tracker::new(None),
        }
    }

    /// Taille connue (manifeste de Hugging Face) : somme de toutes les
    /// couches sur ce total, qui ne bouge pas.
    #[test]
    fn with_the_manifest_size_progress_is_the_sum_of_all_layers() {
        let mut shown = Shown {
            tracker: Tracker::new(Some(1418)),
        };
        let steps: Vec<Step> = [
            r#"{"status":"pulling manifest"}"#,
            // Modèle, sans `completed` tant qu'il vaut 0 : la barre paraît.
            r#"{"status":"pulling aaaa","digest":"sha256:aaaa","total":1000}"#,
            r#"{"status":"pulling aaaa","digest":"sha256:aaaa","total":1000}"#,
            r#"{"status":"pulling aaaa","digest":"sha256:aaaa","total":1000,"completed":600}"#,
            // Dernière ligne de la couche sous son total.
            r#"{"status":"pulling aaaa","digest":"sha256:aaaa","total":1000,"completed":990}"#,
            // Projecteur d'un modèle de vision : suivi lui aussi, sans que
            // le total change.
            r#"{"status":"pulling bbbb","digest":"sha256:bbbb","total":400}"#,
            r#"{"status":"pulling bbbb","digest":"sha256:bbbb","total":400,"completed":200}"#,
            // Ligne en retard d'une couche : `completed` ne recule pas.
            r#"{"status":"pulling bbbb","digest":"sha256:bbbb","total":400,"completed":100}"#,
            r#"{"status":"pulling bbbb","digest":"sha256:bbbb","total":400,"completed":400}"#,
            // Gabarit sans `completed`, paramètres complets, configuration.
            r#"{"status":"pulling cccc","digest":"sha256:cccc","total":10}"#,
            r#"{"status":"pulling dddd","digest":"sha256:dddd","total":5,"completed":5}"#,
            // `completed` au-delà de `total` : plafonné.
            r#"{"status":"pulling eeee","digest":"sha256:eeee","total":3,"completed":9}"#,
            r#"{"status":"verifying sha256 digest"}"#,
            r#"{"status":"writing manifest"}"#,
            r#"{"status":"success"}"#,
        ]
        .into_iter()
        .map(|text| shown.step(line(text)))
        .collect();
        assert_eq!(
            steps,
            [
                Step::Progress(None),
                progress(1418, 0),
                // Rien de changé : pas d'événement.
                Step::Progress(None),
                progress(1418, 600),
                progress(1418, 990),
                Step::Progress(None),
                progress(1418, 1190),
                Step::Progress(None),
                progress(1418, 1390),
                Step::Progress(None),
                progress(1418, 1395),
                progress(1418, 1398),
                Step::Progress(None),
                Step::Progress(None),
                // Complète à la fin : chaque couche compte pour son total.
                Step::Success(ChatEvent::Download {
                    total: 1418,
                    completed: 1418,
                    done: true,
                }),
            ]
        );
        // Couches plus grosses que le manifeste annoncé : le total suit,
        // jamais plus de reçu que de total.
        let mut tracker = Tracker::new(Some(100));
        assert_eq!(tracker.update("sha256:a", 150, 120), Some((120, 150)));
        assert_eq!(tracker.finish(), 150);
    }

    /// Sans taille connue (autre nom, manifeste illisible) : comme
    /// l'officiel, la plus grosse couche seule.
    #[test]
    fn the_largest_layer_is_followed() {
        let mut shown = largest();
        let steps: Vec<Step> = [
            r#"{"status":"pulling manifest"}"#,
            // Petite couche d'abord : suivie tant qu'il n'y a pas plus gros.
            r#"{"status":"pulling aaaa","digest":"sha256:aaaa","total":100}"#,
            r#"{"status":"pulling aaaa","digest":"sha256:aaaa","total":100,"completed":60}"#,
            r#"{"status":"pulling bbbb","digest":"sha256:bbbb","total":1000}"#,
            // Lignes des autres couches ignorées, même la précédente.
            r#"{"status":"pulling aaaa","digest":"sha256:aaaa","total":100,"completed":100}"#,
            r#"{"status":"pulling bbbb","digest":"sha256:bbbb","total":1000,"completed":500}"#,
            r#"{"status":"pulling cccc","digest":"sha256:cccc","total":50,"completed":50}"#,
            // Même taille qu'elle : ne la remplace pas.
            r#"{"status":"pulling dddd","digest":"sha256:dddd","total":1000,"completed":1}"#,
            r#"{"status":"pulling bbbb","digest":"sha256:bbbb","total":1000,"completed":999}"#,
            r#"{"status":"verifying sha256 digest"}"#,
            r#"{"status":"writing manifest"}"#,
            r#"{"status":"success"}"#,
        ]
        .into_iter()
        .map(|text| shown.step(line(text)))
        .collect();
        assert_eq!(
            steps,
            [
                Step::Progress(None),
                progress(100, 0),
                progress(100, 60),
                progress(1000, 0),
                Step::Progress(None),
                progress(1000, 500),
                Step::Progress(None),
                Step::Progress(None),
                progress(1000, 999),
                Step::Progress(None),
                Step::Progress(None),
                // Complète à la fin, même si la dernière ligne ne l'était pas.
                Step::Success(ChatEvent::Download {
                    total: 1000,
                    completed: 1000,
                    done: true,
                }),
            ]
        );
    }

    #[test]
    fn a_digest_without_size_is_never_followed() {
        let mut shown = largest();
        let event = shown.step(line(r#"{"status":"pulling x","digest":"sha256:x"}"#));
        assert_eq!(event, Step::Progress(None));
        assert_eq!(
            shown.step(line(r#"{"status":"success"}"#)),
            Step::Success(ChatEvent::Download {
                total: 0,
                completed: 0,
                done: true,
            })
        );
    }

    #[test]
    fn errors_come_first_and_read_like_the_official_app() {
        let mut shown = largest();
        let failed = |text: &str| Step::Failed(text.to_owned());
        // `error` examiné avant `status`.
        assert_eq!(
            shown.step(line(r#"{"status":"success","error":"boom"}"#)),
            failed("boom")
        );
        assert_eq!(
            shown.step(line(r#"{"status":"pulling manifest","error":""}"#)),
            Step::Progress(None)
        );
        let missing = "pull model manifest: file does not exist";
        assert_eq!(
            shown.step(Progress {
                error: Some(missing.into()),
                ..Progress::default()
            }),
            failed(missing)
        );
        // Erreur du registre de Hugging Face relayée par Ollama : réécrite.
        let sharded = r#"pull model manifest: 400: {"error":"This tag is a sharded GGUF. Ollama does not yet support pulling sharded GGUF via the registry."}"#;
        assert_eq!(
            shown.step(Progress {
                error: Some(sharded.into()),
                ..Progress::default()
            }),
            failed(&crate::hub::errors::sharded())
        );
        // Un exemple réaliste par motif de isNetworkError (ui.go:597-608),
        // écrit en dur : retirer ou altérer un motif de NETWORK_ERRORS doit
        // faire échouer ce test. Texte officiel littéral.
        let offline = "Unable to download model. Please check your internet connection to download the model for offline use.";
        let get = "Get \"https://registry.ollama.ai/v2/library/m/manifests/latest\"";
        for cause in [
            "dial tcp 104.21.75.227:443: connect: connection refused",
            "dial tcp: lookup registry.ollama.ai on 127.0.0.53:53: no such host",
            "net/http: TLS handshake timeout",
            "dial tcp 104.21.75.227:443: connect: network is unreachable",
            "read tcp 10.0.0.2:51234->104.21.75.227:443: read: connection reset by peer",
            "dial tcp 104.21.75.227:443: connect: connection timed out",
            "lookup registry.ollama.ai: temporary failure in name resolution",
            "dial tcp [::1]:443: connect: cannot assign requested address",
            "read tcp 10.0.0.2:51234->104.21.75.227:443: i/o timeout",
            "context deadline exceeded (Client.Timeout exceeded while awaiting headers)",
            "write tcp 10.0.0.2:51234->104.21.75.227:443: write: broken pipe",
        ] {
            let error = format!("pull model manifest: {get}: {cause}");
            assert_eq!(pull_error(&error), offline, "{cause}");
        }
        // Chaque motif officiel, seul après le préfixe.
        for pattern in [
            "connection refused",
            "no such host",
            "timeout",
            "network is unreachable",
            "connection reset",
            "connection timed out",
            "temporary failure",
            "dial tcp",
            "i/o timeout",
            "context deadline exceeded",
            "broken pipe",
        ] {
            let error = format!("pull model manifest: {pattern}");
            assert_eq!(pull_error(&error), offline, "{pattern}");
        }
        // Motif réseau hors du manifeste (pendant une couche) : brut, comme
        // l'officiel.
        let layer = "max retries exceeded: dial tcp: lookup example.com: no such host";
        assert_eq!(pull_error(layer), layer);
        let other = "pull model manifest: 412: this model requires a newer version of Ollama";
        assert_eq!(pull_error(other), other);
    }

    #[test]
    fn unreadable_lines() {
        let kind = |line: &str, ended: bool, understood: bool| {
            parse(line, ended, understood)
                .map(drop)
                .map_err(|(kind, _)| kind)
        };
        let half = r#"{"status":"pulling 4f6c1a8e2b7d","dig"#;
        // Dernière ligne coupée par la fin du flux.
        assert_eq!(kind(half, true, false), Err(ChatErrorKind::ConnectionLost));
        assert_eq!(kind("<htm", true, true), Err(ChatErrorKind::ConnectionLost));
        // Au milieu du flux, ou page d'un autre programme.
        assert_eq!(kind(half, false, true), Err(ChatErrorKind::Download));
        assert_eq!(kind("<html>", true, false), Err(ChatErrorKind::Download));
        let Err((_, message)) = parse("<html>", true, false) else {
            panic!("expected an error");
        };
        assert!(
            message.starts_with("unexpected answer from Ollama: "),
            "{message}"
        );
        assert_eq!(kind(r#"{"status":"success"}"#, true, false), Ok(()));
    }

    /// Téléchargement seul, le faux serveur servant aussi de registre de
    /// Hugging Face : événements transmis et fin.
    async fn pull_from(url: &str, model: &str) -> (Vec<ChatEvent>, Outcome) {
        pull_with_registry(url, model, url).await
    }

    async fn pull_with_registry(url: &str, model: &str, hf_url: &str) -> (Vec<ChatEvent>, Outcome) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let (_cancel, mut cancelled) = watch::channel(false);
        let outcome = pull_with(url, model, hf_url, &mut cancelled, &move |event| {
            sink.lock().push(event);
        })
        .await;
        let events = events.lock().clone();
        (events, outcome)
    }

    /// Progressions (total, reçu) avant la fin, et la fin.
    fn split(events: &[ChatEvent]) -> (Vec<(u64, u64)>, ChatEvent) {
        let (last, progress) = events.split_last().unwrap();
        let progress = progress
            .iter()
            .map(|event| match *event {
                ChatEvent::Download {
                    total,
                    completed,
                    done: false,
                } => (total, completed),
                _ => panic!("{event:?}"),
            })
            .collect();
        (progress, last.clone())
    }

    /// Part reçue (en millièmes) jamais décroissante : la barre ne recule pas.
    fn never_back(progress: &[(u64, u64)]) {
        let shares: Vec<u64> = progress
            .iter()
            .map(|&(total, completed)| {
                assert!(completed <= total, "{progress:?}");
                completed * 1000 / total.max(1)
            })
            .collect();
        assert!(
            shares.windows(2).all(|pair| pair[0] <= pair[1]),
            "{shares:?}"
        );
    }

    #[tokio::test]
    async fn a_closed_port_is_an_unreachable_server() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let url = fake.url();
        fake.stop().await;
        let (events, outcome) = pull_from(&url, "chatonlm-pull").await;
        assert!(events.is_empty());
        let Outcome::Failed(kind, message) = outcome else {
            panic!("expected a failure");
        };
        assert_eq!(kind, ChatErrorKind::Unreachable);
        assert_eq!(message, format!("can't connect to Ollama at {url}"));
    }

    #[tokio::test]
    async fn another_program_is_an_unexpected_answer() {
        let other = FakeOllama::start(Mode::NotOllama).await.unwrap();
        let (events, outcome) = pull_from(&other.url(), "chatonlm-pull").await;
        assert!(events.is_empty());
        let Outcome::Failed(kind, message) = outcome else {
            panic!("expected a failure");
        };
        assert_eq!(kind, ChatErrorKind::Download);
        assert!(
            message.starts_with("unexpected answer from Ollama: "),
            "{message}"
        );
    }

    #[tokio::test]
    async fn a_successful_pull_ends_with_the_whole_layer() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let (events, outcome) = pull_from(&fake.url(), "chatonlm-pull-small").await;
        assert!(matches!(outcome, Outcome::Done));
        assert_eq!(
            events.last(),
            Some(&ChatEvent::Download {
                total: 1000,
                completed: 1000,
                done: true,
            })
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(fake.disconnects(), 0);
    }

    /// Modèle de vision de Hugging Face, manifeste lisible : la progression
    /// suit toutes les couches (projecteur compris) sur la taille du
    /// manifeste, connue dès la première ligne ; la part reçue ne recule
    /// jamais ; complète à la fin, configuration comprise.
    #[tokio::test]
    async fn a_vision_model_counts_every_layer() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let model = "hf.co/chatonlm/SmolVLM-vision-GGUF:Q8_0";
        let (events, outcome) = pull_from(&fake.url(), model).await;
        assert!(matches!(outcome, Outcome::Done));
        assert_eq!(
            fake.manifest_requests(),
            ["GET /v2/chatonlm/SmolVLM-vision-GGUF/manifests/Q8_0"]
        );
        let whole = fake_ollama::hf_total(model);
        assert!(whole > fake_ollama::HF_MODEL_LAYER + fake_ollama::HF_PROJECTOR_LAYER);
        let (progress, last) = split(&events);
        assert_eq!(
            last,
            ChatEvent::Download {
                total: whole,
                completed: whole,
                done: true,
            }
        );
        assert!(
            progress.iter().all(|&(total, _)| total == whole),
            "{progress:?}"
        );
        never_back(&progress);
        // La barre avance pendant le projecteur (l'officiel la figeait).
        assert!(
            progress
                .iter()
                .any(|&(_, completed)| completed > fake_ollama::HF_MODEL_LAYER),
            "{progress:?}"
        );
        // Dernière ligne du modèle sous son total : complète seulement à la fin.
        assert!(progress.last().unwrap().1 < whole, "{progress:?}");
    }

    /// Manifeste injoignable : comme l'officiel, la plus grosse couche seule
    /// (le modèle), dont la part ne recule jamais ; le projecteur n'agrandit
    /// pas le total en cours de route.
    #[tokio::test]
    async fn without_a_manifest_the_largest_layer_is_shown() {
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        let closed = FakeOllama::start(Mode::ollama()).await.unwrap();
        let hf_url = closed.url();
        closed.stop().await;
        let model = "hf.co/chatonlm/SmolVLM-vision-GGUF:Q8_0";
        let (events, outcome) = pull_with_registry(&fake.url(), model, &hf_url).await;
        assert!(matches!(outcome, Outcome::Done));
        let layer = fake_ollama::HF_MODEL_LAYER;
        let (progress, last) = split(&events);
        assert_eq!(
            last,
            ChatEvent::Download {
                total: layer,
                completed: layer,
                done: true,
            }
        );
        assert!(
            progress.iter().all(|&(total, _)| total == layer),
            "{progress:?}"
        );
        never_back(&progress);
        // Registre qui répond autre chose qu'un manifeste : de même.
        let (events, _) = pull_with_registry(
            &fake.url(),
            "hf.co/chatonlm/Eye-vision-html-GGUF:Q4_K_M",
            &fake.url(),
        )
        .await;
        assert!(split(&events).0.iter().all(|&(total, _)| total == layer));
        // Nom d'Ollama : jamais de manifeste demandé à Hugging Face.
        let before = fake.manifest_requests().len();
        let (events, outcome) = pull_from(&fake.url(), "chatonlm-pull").await;
        assert!(matches!(outcome, Outcome::Done));
        assert_eq!(fake.manifest_requests().len(), before);
        let (progress, last) = split(&events);
        let total = fake_ollama::PULL_TOTAL;
        assert!(
            progress.iter().all(|&(shown, _)| shown == total),
            "{progress:?}"
        );
        assert_eq!(
            last,
            ChatEvent::Download {
                total,
                completed: total,
                done: true,
            }
        );
    }

    /// Contrat : pull du chat d'un `hf.co/...` déjà sur l'instance de test,
    /// manifeste lu chez Hugging Face (réseau) : toute la progression sur la
    /// taille du manifeste, complète à la fin ; sans manifeste (registre
    /// injoignable), la plus grosse couche (les poids).
    #[tokio::test]
    #[ignore = "instance Ollama de test (scripts/test-ollama.sh start) et Hugging Face : accès au réseau"]
    async fn contract_hf_pull_follows_the_manifest_size() {
        const TEST_INSTANCE: &str = "http://127.0.0.1:11435";
        let model = "hf.co/bartowski/SmolLM2-135M-Instruct-GGUF:Q4_K_M";
        let (events, outcome) =
            pull_with_registry(TEST_INSTANCE, model, names::DEFAULT_HF_URL).await;
        assert!(matches!(outcome, Outcome::Done));
        let whole = 105_455_694;
        let (progress, last) = split(&events);
        assert_eq!(
            last,
            ChatEvent::Download {
                total: whole,
                completed: whole,
                done: true,
            }
        );
        assert!(!progress.is_empty());
        assert!(
            progress.iter().all(|&(total, _)| total == whole),
            "{progress:?}"
        );
        never_back(&progress);
        let closed = FakeOllama::start(Mode::ollama()).await.unwrap();
        let hf_url = closed.url();
        closed.stop().await;
        let (events, outcome) = pull_with_registry(TEST_INSTANCE, model, &hf_url).await;
        assert!(matches!(outcome, Outcome::Done));
        let (progress, last) = split(&events);
        let ChatEvent::Download { total: largest, .. } = last else {
            panic!("{last:?}");
        };
        assert!(largest > 100_000_000 && largest < whole, "{largest}");
        assert!(
            progress.iter().all(|&(total, _)| total == largest),
            "{progress:?}"
        );
    }

    /// Erreurs du registre de Hugging Face relayées par Ollama : réécrites.
    #[tokio::test]
    async fn hugging_face_errors_are_rewritten() {
        use crate::hub::errors;
        let fake = FakeOllama::start(Mode::ollama()).await.unwrap();
        for (model, shown) in [
            ("hf.co/chatonlm/Big-sharded-GGUF:Q8_0", errors::sharded()),
            (
                "hf.co/chatonlm/Gone-missing-GGUF:Q4_K_M",
                errors::not_found_or_private(),
            ),
            (
                "hf.co/chatonlm/Tiny-GGUF:badtag",
                errors::unavailable_version(),
            ),
        ] {
            let (events, outcome) = pull_from(&fake.url(), model).await;
            assert!(events.is_empty(), "{model}");
            let Outcome::Failed(kind, message) = outcome else {
                panic!("{model}: expected a failure");
            };
            assert_eq!(kind, ChatErrorKind::Download, "{model}");
            assert_eq!(message, shown, "{model}");
        }
    }
}
