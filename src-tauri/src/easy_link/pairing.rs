//! Appairage d'un compte : l'app montre un code à durée courte, la personne
//! l'envoie au bot. Un bot est trouvable par son nom : sans ce code, n'importe
//! qui se servirait de la machine.
//!
//! Garde-fous contre la devinette : un seul code à la fois, six chiffres tirés
//! du générateur du système, dix minutes de validité, annulé après dix
//! mauvais codes en tout, et cinq essais par compte toutes les dix minutes
//! (au-delà, le bot ne répond plus à ce compte). Logique pure, horloge donnée
//! par l'appelant.

use std::collections::HashMap;

use rand::RngExt;

use crate::store::easy_link::Platform;

/// Validité d'un code, en millisecondes.
pub const CODE_LIFETIME_MS: i64 = 10 * 60 * 1000;

/// Mauvais codes tolérés pour un même code, tous comptes confondus.
const MAX_WRONG_PER_CODE: u32 = 10;

/// Essais d'un même compte par fenêtre.
const MAX_TRIES_PER_SENDER: u32 = 5;
const SENDER_WINDOW_MS: i64 = 10 * 60 * 1000;

/// Délai entre deux avis « bot privé » au même compte.
const NOTICE_INTERVAL_MS: i64 = 60 * 1000;

/// Comptes inconnus suivis au plus (un flot de comptes ne fait pas grossir la
/// mémoire sans borne).
const MAX_SENDERS: usize = 2000;

/// Code en cours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pairing {
    pub platform: Platform,
    pub code: String,
    pub expires_at: i64,
    wrong: u32,
}

/// Ce que le bot fait d'un message d'un compte qui n'est pas lié.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Bon code : lier le compte.
    Linked,
    /// Pas de code, ou mauvais : dire que le bot est privé.
    Notice,
    /// Ne rien répondre (trop d'essais, ou avis donné il y a un instant).
    Silent,
}

#[derive(Default)]
struct Seen {
    tries: u32,
    window_start: i64,
    last_notice: Option<i64>,
}

#[derive(Default)]
pub struct Pairings {
    current: Option<Pairing>,
    senders: HashMap<(Platform, String), Seen>,
}

impl Pairings {
    /// Nouveau code pour `platform` ; remplace le précédent.
    pub fn start(&mut self, platform: Platform, now: i64) -> Pairing {
        let code = format!("{:06}", rand::rng().random_range(0..1_000_000_u32));
        let pairing = Pairing {
            platform,
            code,
            expires_at: now + CODE_LIFETIME_MS,
            wrong: 0,
        };
        self.current = Some(pairing.clone());
        pairing
    }

    /// Code en cours, s'il n'a pas expiré.
    pub fn current(&mut self, now: i64) -> Option<&Pairing> {
        if self
            .current
            .as_ref()
            .is_some_and(|pairing| pairing.expires_at <= now)
        {
            self.current = None;
        }
        self.current.as_ref()
    }

    /// Retire le code en cours ; vrai s'il y en avait un.
    pub fn cancel(&mut self) -> bool {
        self.current.take().is_some()
    }

    /// Retire le code en cours s'il est pour `platform` (bot retiré).
    pub fn cancel_for(&mut self, platform: Platform) -> bool {
        if self
            .current
            .as_ref()
            .is_some_and(|pairing| pairing.platform == platform)
        {
            self.current = None;
            return true;
        }
        false
    }

    /// Message d'un compte non lié. Vrai en second si le code en cours vient
    /// d'être retiré (consommé ou annulé) : l'état montré change.
    pub fn attempt(
        &mut self,
        platform: Platform,
        sender: &str,
        text: &str,
        now: i64,
    ) -> (Verdict, bool) {
        if self.senders.len() >= MAX_SENDERS {
            self.senders
                .retain(|_, seen| now - seen.window_start < SENDER_WINDOW_MS);
            if self.senders.len() >= MAX_SENDERS {
                return (Verdict::Silent, false);
            }
        }
        let seen = self
            .senders
            .entry((platform, sender.to_owned()))
            .or_default();
        if now - seen.window_start >= SENDER_WINDOW_MS {
            seen.window_start = now;
            seen.tries = 0;
        }
        let notice = |seen: &mut Seen| {
            if seen
                .last_notice
                .is_some_and(|last| now - last < NOTICE_INTERVAL_MS)
            {
                Verdict::Silent
            } else {
                seen.last_notice = Some(now);
                Verdict::Notice
            }
        };
        let Some(candidate) = candidate(text) else {
            return (notice(seen), false);
        };
        if seen.tries >= MAX_TRIES_PER_SENDER {
            return (Verdict::Silent, false);
        }
        seen.tries += 1;
        if self
            .current
            .as_ref()
            .is_some_and(|pairing| pairing.expires_at <= now)
        {
            self.current = None;
        }
        let Some(pairing) = self
            .current
            .as_mut()
            .filter(|pairing| pairing.platform == platform)
        else {
            return (notice(seen), false);
        };
        if pairing.code == candidate {
            self.current = None;
            self.senders.remove(&(platform, sender.to_owned()));
            return (Verdict::Linked, true);
        }
        pairing.wrong += 1;
        let cancelled = pairing.wrong >= MAX_WRONG_PER_CODE;
        if cancelled {
            self.current = None;
        }
        (notice(seen), cancelled)
    }
}

/// Code proposé par un message : six chiffres, seuls ou après `/start`,
/// `/link` ou `!link` (Telegram envoie `/start <code>` quand on ouvre le lien
/// d'appairage), espaces ignorées.
fn candidate(text: &str) -> Option<String> {
    let text = text.trim();
    let rest = ["/start", "/link", "!link", "!start"]
        .iter()
        .find_map(|command| {
            let rest = text.strip_prefix(command)?;
            match rest.strip_prefix('@') {
                // `/start@nom_du_bot 123456` dans Telegram.
                Some(named) => Some(named.split_once(char::is_whitespace).map_or("", |(_, r)| r)),
                // Un autre mot qui commence pareil (`/started`) n'est pas la
                // commande.
                None => (rest.is_empty() || rest.starts_with(char::is_whitespace)).then_some(rest),
            }
        })
        .unwrap_or(text);
    let digits: String = rest.chars().filter(|c| !c.is_whitespace()).collect();
    (digits.len() == 6 && digits.chars().all(|c| c.is_ascii_digit())).then_some(digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Platform = Platform::Telegram;
    const D: Platform = Platform::Discord;

    #[test]
    fn codes_are_six_digits() {
        let mut pairings = Pairings::default();
        for _ in 0..200 {
            let code = pairings.start(T, 0).code;
            assert_eq!(code.len(), 6, "{code}");
            assert!(code.chars().all(|c| c.is_ascii_digit()), "{code}");
        }
    }

    #[test]
    fn the_code_is_read_from_the_usual_messages() {
        assert_eq!(candidate("123456").as_deref(), Some("123456"));
        assert_eq!(candidate("  123 456 ").as_deref(), Some("123456"));
        assert_eq!(candidate("/start 123456").as_deref(), Some("123456"));
        assert_eq!(
            candidate("/start@chaton_bot 123456").as_deref(),
            Some("123456")
        );
        assert_eq!(candidate("!link 123456").as_deref(), Some("123456"));
        assert_eq!(candidate("/start"), None);
        assert_eq!(candidate("12345"), None);
        assert_eq!(candidate("1234567"), None);
        assert_eq!(candidate("12a456"), None);
        assert_eq!(candidate("my code is 123456"), None);
        assert_eq!(candidate("/started 123456"), None);
    }

    #[test]
    fn the_right_code_links_once() {
        let mut pairings = Pairings::default();
        let code = pairings.start(T, 0).code;
        assert_eq!(
            pairings.attempt(T, "ana", &code, 1),
            (Verdict::Linked, true)
        );
        // Consommé : un autre compte ne peut plus s'en servir.
        assert_eq!(
            pairings.attempt(T, "eve", &code, 2),
            (Verdict::Notice, false)
        );
        assert!(pairings.current(2).is_none());
    }

    #[test]
    fn a_code_is_for_its_messenger_and_its_ten_minutes() {
        let mut pairings = Pairings::default();
        let code = pairings.start(T, 0).code;
        assert_eq!(
            pairings.attempt(D, "ana", &code, 1),
            (Verdict::Notice, false)
        );
        assert!(pairings.current(CODE_LIFETIME_MS - 1).is_some());
        assert_eq!(
            pairings.attempt(T, "luis", &code, CODE_LIFETIME_MS),
            (Verdict::Notice, false)
        );
        assert!(pairings.current(CODE_LIFETIME_MS).is_none());
    }

    #[test]
    fn a_sender_gets_five_tries_then_silence() {
        let mut pairings = Pairings::default();
        let code = pairings.start(T, 0).code;
        let wrong = if code == "000000" { "111111" } else { "000000" };
        let mut now = 0;
        for _ in 0..MAX_TRIES_PER_SENDER {
            now += NOTICE_INTERVAL_MS;
            assert_eq!(pairings.attempt(T, "eve", wrong, now).0, Verdict::Notice);
        }
        // Même le bon code n'est plus lu pour ce compte dans la fenêtre.
        assert_eq!(
            pairings.attempt(T, "eve", &code, now + 1),
            (Verdict::Silent, false)
        );
        // Un autre compte n'est pas touché.
        assert_eq!(
            pairings.attempt(T, "ana", &code, now + 2).0,
            Verdict::Linked
        );
    }

    #[test]
    fn the_tries_come_back_after_the_window() {
        let mut pairings = Pairings::default();
        for i in 0..MAX_TRIES_PER_SENDER {
            pairings.attempt(T, "ana", "000001", i64::from(i));
        }
        let code = pairings.start(T, SENDER_WINDOW_MS).code;
        assert_eq!(
            pairings.attempt(T, "ana", &code, SENDER_WINDOW_MS + 10).0,
            Verdict::Linked
        );
    }

    #[test]
    fn ten_wrong_codes_cancel_the_code() {
        let mut pairings = Pairings::default();
        let code = pairings.start(T, 0).code;
        let wrong = if code == "000000" { "111111" } else { "000000" };
        for i in 0..MAX_WRONG_PER_CODE {
            let (_, changed) = pairings.attempt(T, &format!("eve{i}"), wrong, 1);
            assert_eq!(changed, i + 1 == MAX_WRONG_PER_CODE, "essai {i}");
        }
        assert!(pairings.current(2).is_none());
        assert_eq!(
            pairings.attempt(T, "ana", &code, 3),
            (Verdict::Notice, false)
        );
    }

    #[test]
    fn the_notice_is_not_repeated_within_a_minute() {
        let mut pairings = Pairings::default();
        assert_eq!(pairings.attempt(T, "bob", "hello", 0).0, Verdict::Notice);
        assert_eq!(
            pairings.attempt(T, "bob", "hello?", 1000).0,
            Verdict::Silent
        );
        assert_eq!(
            pairings.attempt(T, "bob", "hello", NOTICE_INTERVAL_MS).0,
            Verdict::Notice
        );
    }

    #[test]
    fn a_flood_of_unknown_senders_stays_bounded() {
        let mut pairings = Pairings::default();
        for i in 0..MAX_SENDERS + 50 {
            pairings.attempt(T, &i.to_string(), "hello", 0);
        }
        assert!(pairings.senders.len() <= MAX_SENDERS);
        // Fenêtre passée : les anciens sont oubliés, les nouveaux reçoivent
        // de nouveau l'avis.
        assert_eq!(
            pairings.attempt(T, "late", "hello", SENDER_WINDOW_MS).0,
            Verdict::Notice
        );
    }

    #[test]
    fn cancelling_is_per_messenger() {
        let mut pairings = Pairings::default();
        pairings.start(T, 0);
        assert!(!pairings.cancel_for(D));
        assert!(pairings.cancel_for(T));
        assert!(!pairings.cancel());
    }
}
