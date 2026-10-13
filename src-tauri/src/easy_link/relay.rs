//! Réponse du modèle montrée dans la messagerie pendant qu'elle s'écrit : un
//! message envoyé au premier texte, puis modifié à intervalles ; une réponse
//! plus longue que la limite continue dans un message suivant.
//!
//! Les messageries limitent le rythme des envois : les modifications sont
//! espacées, un refus « trop de requêtes » suspend les suivantes pour la durée
//! demandée, et seule la dernière écriture insiste.

use std::time::{Duration, Instant};

use easy_link::{Messenger, Problem, text};

/// Intervalle entre deux modifications d'un message en cours.
const EDIT_INTERVAL: Duration = Duration::from_millis(1500);

/// Essais de la dernière écriture.
const FINAL_TRIES: usize = 4;

/// Attente la plus longue acceptée d'une messagerie avant un nouvel essai ;
/// au-delà, l'écriture est abandonnée (jamais réessayée plus tôt).
const MAX_WAIT: Duration = Duration::from_secs(30);

struct Posted {
    id: String,
    text: String,
}

pub struct Relay<'a> {
    messenger: &'a Messenger,
    channel: &'a str,
    interval: Duration,
    /// Réponse reçue jusqu'ici.
    text: String,
    /// Messages déjà envoyés, avec le texte qu'ils montrent.
    posted: Vec<Posted>,
    last_sync: Option<Instant>,
    last_change: Option<Instant>,
    hold_until: Option<Instant>,
    /// Le destinataire a bloqué le bot : plus rien n'est envoyé.
    gone: bool,
}

impl<'a> Relay<'a> {
    pub fn new(messenger: &'a Messenger, channel: &'a str) -> Self {
        Self {
            messenger,
            channel,
            interval: EDIT_INTERVAL,
            text: String::new(),
            posted: Vec::new(),
            last_sync: None,
            last_change: None,
            hold_until: None,
            gone: false,
        }
    }

    pub fn push(&mut self, delta: &str) {
        self.text.push_str(delta);
    }

    /// Nouveau tour de réponse (après un appel d'outil) : séparé du précédent.
    pub fn new_turn(&mut self) {
        if !self.text.trim().is_empty() && !self.text.ends_with("\n\n") {
            self.text.push_str("\n\n");
        }
    }

    /// Rien de lisible n'a été reçu.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    pub fn gone(&self) -> bool {
        self.gone
    }

    /// Quelque chose a été écrit dans la messagerie depuis moins de `within` :
    /// inutile d'y montrer « écrit… ».
    pub fn wrote_within(&self, within: Duration) -> bool {
        self.last_change
            .is_some_and(|change| change.elapsed() < within)
    }

    /// À appeler régulièrement : écrit ce qui a changé si l'intervalle est
    /// passé. Un échec n'est pas rendu, le prochain appel réessaie.
    pub async fn tick(&mut self) {
        let now = Instant::now();
        if self.gone
            || self.hold_until.is_some_and(|until| now < until)
            || self
                .last_sync
                .is_some_and(|last| now.duration_since(last) < self.interval)
        {
            return;
        }
        match self.sync().await {
            Ok(()) => {}
            Err(Problem::RateLimited(wait)) => {
                self.hold_until = Instant::now().checked_add(wait);
            }
            Err(Problem::Forbidden) => self.gone = true,
            Err(_) => {}
        }
    }

    /// Dernière écriture : insiste quelques fois, attend ce que la
    /// messagerie demande. Rend le problème qui reste.
    pub async fn finish(&mut self) -> Result<(), Problem> {
        if self.gone {
            return Err(Problem::Forbidden);
        }
        if let Some(until) = self.hold_until.take() {
            let left = until.saturating_duration_since(Instant::now());
            if left > MAX_WAIT {
                return Err(Problem::RateLimited(left));
            }
            tokio::time::sleep(left).await;
        }
        let mut wait = Duration::from_millis(500);
        let mut tries = 0;
        loop {
            tries += 1;
            match self.sync().await {
                Ok(()) => return Ok(()),
                Err(Problem::Forbidden) => {
                    self.gone = true;
                    return Err(Problem::Forbidden);
                }
                Err(problem) if tries >= FINAL_TRIES => return Err(problem),
                // Réessayer avant le délai demandé aggraverait la limite :
                // une attente trop longue est abandonnée.
                Err(Problem::RateLimited(asked)) if asked > MAX_WAIT => {
                    return Err(Problem::RateLimited(asked));
                }
                Err(Problem::RateLimited(asked)) => tokio::time::sleep(asked).await,
                Err(Problem::Network(_)) => {
                    tokio::time::sleep(wait).await;
                    wait *= 2;
                }
                Err(problem) => return Err(problem),
            }
        }
    }

    /// Met les messages de la messagerie à l'image du texte reçu.
    async fn sync(&mut self) -> Result<(), Problem> {
        self.last_sync = Some(Instant::now());
        let parts = text::split(self.text.trim_end(), self.messenger.text_limit());
        for (index, part) in parts.into_iter().enumerate() {
            match self.posted.get_mut(index) {
                Some(posted) if posted.text == part => {}
                Some(posted) => {
                    self.messenger.edit(self.channel, &posted.id, &part).await?;
                    posted.text = part;
                    self.last_change = Some(Instant::now());
                }
                None => {
                    let id = self.messenger.send(self.channel, &part).await?;
                    self.posted.push(Posted { id, text: part });
                    self.last_change = Some(Instant::now());
                }
            }
        }
        Ok(())
    }
}
