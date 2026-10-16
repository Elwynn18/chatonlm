//! Réponse du modèle montrée dans la messagerie pendant qu'elle s'écrit : un
//! message envoyé au premier texte, puis modifié à intervalles ; une réponse
//! plus longue que la limite continue dans un message suivant.
//!
//! Les messageries limitent le rythme des envois : les modifications sont
//! espacées, un refus « trop de requêtes » suspend toutes les écritures
//! suivantes (texte, photo, texte du bot) pour la durée demandée, et seule la
//! dernière écriture insiste. Une attente courte est faite ; au-delà de
//! [`MAX_WAIT`], l'écriture est abandonnée et la limite reste posée : rien
//! n'est jamais réessayé plus tôt.
//!
//! Une image produite pendant la réponse (outil `generate_image`) part en
//! photo à sa place dans le texte : ce qui précède est écrit, la photo
//! envoyée, et la suite continue dans un nouveau message.

use std::time::{Duration, Instant};

use easy_link::{Messenger, Problem, text};

use crate::i18n::t;

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
    /// Quelque chose a déjà été montré avant une image (texte, photo ou
    /// texte de repli) : la réponse n'est pas vide.
    delivered: bool,
    /// Le texte d'avant une image n'a pas pu être écrit en entier : rendu
    /// par [`Relay::finish`].
    lost: Option<Problem>,
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
            delivered: false,
            lost: None,
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

    /// Rien de lisible n'a été reçu, ni aucune image.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && !self.delivered
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
        // Une limite reçue est posée par `noted`.
        if let Err(Problem::Forbidden) = self.sync().await {
            self.gone = true;
        }
    }

    /// Note une limite de débit reçue : les écritures suivantes l'attendent
    /// (la plus lointaine l'emporte).
    fn noted<T>(&mut self, result: Result<T, Problem>) -> Result<T, Problem> {
        if let Err(Problem::RateLimited(wait)) = &result
            && let Some(until) = Instant::now().checked_add(*wait)
        {
            self.hold_until = Some(self.hold_until.map_or(until, |held| held.max(until)));
        }
        result
    }

    /// Avant une écriture : attend la fin de la limite posée si elle est
    /// proche ; plus lointaine que [`MAX_WAIT`], l'écriture est abandonnée et
    /// la limite reste posée.
    async fn wait_hold(&mut self) -> Result<(), Problem> {
        if let Some(until) = self.hold_until {
            let left = until.saturating_duration_since(Instant::now());
            if left > MAX_WAIT {
                return Err(Problem::RateLimited(left));
            }
            tokio::time::sleep(left).await;
            self.hold_until = None;
        }
        Ok(())
    }

    /// Dernière écriture : insiste quelques fois, attend ce que la
    /// messagerie demande. Rend le problème qui reste, ou celui du texte
    /// d'avant une image qui n'a pas pu être écrit.
    pub async fn finish(&mut self) -> Result<(), Problem> {
        self.write_all()
            .await
            .and_then(|()| self.lost.take().map_or(Ok(()), Err))
    }

    async fn write_all(&mut self) -> Result<(), Problem> {
        if self.gone {
            return Err(Problem::Forbidden);
        }
        self.wait_hold().await?;
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
                // Réessayer avant le délai demandé aggraverait la limite
                // (posée par `noted`) : une attente trop longue est
                // abandonnée.
                Err(Problem::RateLimited(asked)) if asked > MAX_WAIT => {
                    return Err(Problem::RateLimited(asked));
                }
                Err(Problem::RateLimited(_)) => self.wait_hold().await?,
                Err(Problem::Network(_)) => {
                    tokio::time::sleep(wait).await;
                    wait *= 2;
                }
                Err(problem) => return Err(problem),
            }
        }
    }

    /// Image produite : le texte reçu jusqu'ici est écrit (dernière écriture
    /// de ses messages), puis la photo part ; la suite de la réponse
    /// continuera dans un nouveau message. Sans image (`png` absent : illisible
    /// en base) ou si l'envoi échoue, un court texte la remplace et la réponse
    /// continue ; un échec du réseau (délai dépassé : la photo a pu arriver)
    /// est dit avec prudence. Rien n'est envoyé à qui a bloqué le bot.
    pub async fn photo(&mut self, png: Option<Vec<u8>>, filename: &str) {
        if !self.gone && !self.text.trim().is_empty() {
            match self.finish().await {
                Ok(()) | Err(Problem::Forbidden) => {}
                Err(problem) => self.lost = Some(problem),
            }
            self.delivered = true;
        }
        // La suite part dans de nouveaux messages, dès le prochain appel.
        self.text.clear();
        self.posted.clear();
        self.last_sync = None;
        if self.gone {
            return;
        }
        let sent = match png {
            Some(png) => self.send_photo(png, filename).await,
            None => Err(Problem::Refused {
                status: 0,
                message: "image not found".to_owned(),
            }),
        };
        match sent {
            Ok(()) => {}
            Err(Problem::Forbidden) => self.gone = true,
            Err(Problem::Network(_)) => {
                let note = t!("easyLink.photoMaybeFailed");
                self.say(&note).await;
            }
            Err(_) => {
                let note = t!("easyLink.photoFailed");
                self.say(&note).await;
            }
        }
        self.delivered = true;
        self.last_change = Some(Instant::now());
    }

    /// Envoie la photo ; une limite de débit courte est attendue une fois.
    async fn send_photo(&mut self, png: Vec<u8>, filename: &str) -> Result<(), Problem> {
        self.wait_hold().await?;
        let sent = self
            .messenger
            .send_photo(self.channel, png.clone(), filename, None)
            .await;
        match self.noted(sent) {
            Err(Problem::RateLimited(asked)) if asked <= MAX_WAIT => {
                self.wait_hold().await?;
                let sent = self
                    .messenger
                    .send_photo(self.channel, png, filename, None)
                    .await;
                self.noted(sent).map(drop)
            }
            sent => sent.map(drop),
        }
    }

    /// Texte du bot hors de la réponse, dans ses propres messages ; la limite
    /// de débit posée est respectée (attendue si elle est courte, sinon le
    /// texte est abandonné), une limite courte reçue est attendue une fois,
    /// tout autre échec abandonné.
    pub async fn say(&mut self, markdown: &str) {
        for part in text::split(markdown, self.messenger.text_limit()) {
            if self.wait_hold().await.is_err() {
                return;
            }
            let sent = self.messenger.send(self.channel, &part).await;
            let mut sent = self.noted(sent);
            if let Err(Problem::RateLimited(asked)) = sent
                && asked <= MAX_WAIT
            {
                if self.wait_hold().await.is_err() {
                    return;
                }
                let again = self.messenger.send(self.channel, &part).await;
                sent = self.noted(again);
            }
            match sent {
                Ok(_) => {}
                Err(Problem::Forbidden) => {
                    self.gone = true;
                    return;
                }
                Err(_) => return,
            }
        }
    }

    /// Met les messages de la messagerie à l'image du texte reçu.
    async fn sync(&mut self) -> Result<(), Problem> {
        self.last_sync = Some(Instant::now());
        let parts = text::split(self.text.trim_end(), self.messenger.text_limit());
        for (index, part) in parts.into_iter().enumerate() {
            let known = self
                .posted
                .get(index)
                .map(|posted| (posted.text == part, posted.id.clone()));
            match known {
                Some((true, _)) => {}
                Some((false, id)) => {
                    let edited = self.messenger.edit(self.channel, &id, &part).await;
                    self.noted(edited)?;
                    self.posted[index].text = part;
                    self.last_change = Some(Instant::now());
                }
                None => {
                    let sent = self.messenger.send(self.channel, &part).await;
                    let id = self.noted(sent)?;
                    self.posted.push(Posted { id, text: part });
                    self.last_change = Some(Instant::now());
                }
            }
        }
        Ok(())
    }
}
