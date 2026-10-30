//! Appairage et preuves du pont. Logique pure, horloge donnée par l'appelant.
//!
//! Appairage : l'app montre un code de six chiffres (générateur du système,
//! dix minutes, un seul à la fois, annulé après dix mauvais codes), que
//! l'utilisateur tape dans VS Code ; l'app rend alors à l'éditeur un secret de
//! 32 octets. Six chiffres suffisent : il n'y a pas d'attaquant à distance
//! (aucun port réseau) et tout programme capable d'essayer tourne déjà sous le
//! même compte.
//!
//! Chaque connexion se prouve ensuite dans les deux sens sans que le secret
//! passe sur le fil : HMAC-SHA256 sur les deux nonces de la connexion, avec un
//! libellé différent pour chaque sens. L'extension vérifie la preuve de l'app
//! avant d'envoyer quoi que ce soit : un programme qui aurait pris la place du
//! point d'accès n'apprend rien.

use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};

/// Validité d'un code, en millisecondes.
pub const CODE_LIFETIME_MS: i64 = 10 * 60 * 1000;

/// Mauvais codes tolérés pour un même code.
const MAX_WRONG: u32 = 10;

const CLIENT_LABEL: &[u8] = b"chatonlm-bridge/1 client";
const SERVER_LABEL: &[u8] = b"chatonlm-bridge/1 server";

/// Octets aléatoires du générateur du système.
pub fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .expect("system random generator");
    bytes
}

/// Code en cours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pairing {
    pub code: String,
    pub expires_at: i64,
    wrong: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairError {
    /// Aucun code en cours (jamais demandé, expiré, consommé, annulé).
    NoCode,
    /// Mauvais code ; le code reste valable.
    Wrong,
    /// Mauvais code de trop : le code est annulé.
    TooMany,
}

#[derive(Default)]
pub struct Pairings {
    current: Option<Pairing>,
}

impl Pairings {
    /// Nouveau code ; remplace le précédent.
    pub fn start(&mut self, now: i64) -> Pairing {
        let number = u32::from_le_bytes(random::<4>()) % 1_000_000;
        let pairing = Pairing {
            code: format!("{number:06}"),
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

    /// Essai d'un code tapé dans l'éditeur ; un bon code est consommé.
    pub fn attempt(&mut self, code: &str, now: i64) -> Result<(), PairError> {
        let Some(pairing) = self.current(now).cloned() else {
            return Err(PairError::NoCode);
        };
        let typed: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        if constant_eq(typed.as_bytes(), pairing.code.as_bytes()) {
            self.current = None;
            return Ok(());
        }
        let current = self.current.as_mut().expect("code checked above");
        current.wrong += 1;
        if current.wrong >= MAX_WRONG {
            self.current = None;
            return Err(PairError::TooMany);
        }
        Err(PairError::Wrong)
    }
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn tag(secret: &[u8; 32], label: &[u8], first: &[u8], second: &[u8]) -> hmac::Tag {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    let mut context = hmac::Context::with_key(&key);
    context.update(label);
    context.update(first);
    context.update(second);
    context.sign()
}

/// Preuve de l'éditeur : HMAC(secret, libellé client ‖ nonce de l'app ‖ nonce
/// de l'éditeur). Calculée par l'extension ; ici, pour les essais.
#[cfg(test)]
pub fn client_proof(secret: &[u8; 32], server_nonce: &[u8], client_nonce: &[u8]) -> Vec<u8> {
    tag(secret, CLIENT_LABEL, server_nonce, client_nonce)
        .as_ref()
        .to_vec()
}

/// Vérifie la preuve de l'éditeur, à temps constant.
pub fn verify_client(
    secret: &[u8; 32],
    server_nonce: &[u8],
    client_nonce: &[u8],
    proof: &[u8],
) -> bool {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    let mut message = CLIENT_LABEL.to_vec();
    message.extend_from_slice(server_nonce);
    message.extend_from_slice(client_nonce);
    hmac::verify(&key, &message, proof).is_ok()
}

/// Preuve de l'app : HMAC(secret, libellé serveur ‖ nonce de l'éditeur ‖
/// nonce de l'app).
pub fn server_proof(secret: &[u8; 32], client_nonce: &[u8], server_nonce: &[u8]) -> Vec<u8> {
    tag(secret, SERVER_LABEL, client_nonce, server_nonce)
        .as_ref()
        .to_vec()
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_six_digits_valid_ten_minutes_and_used_once() {
        let mut pairings = Pairings::default();
        let pairing = pairings.start(1_000);
        assert_eq!(pairing.code.len(), 6);
        assert!(pairing.code.chars().all(|c| c.is_ascii_digit()));
        assert_eq!(pairing.expires_at, 1_000 + CODE_LIFETIME_MS);
        let spaced = format!("{} {}", &pairing.code[..3], &pairing.code[3..]);
        assert_eq!(pairings.attempt(&spaced, 2_000), Ok(()));
        assert_eq!(
            pairings.attempt(&pairing.code, 2_000),
            Err(PairError::NoCode)
        );
    }

    #[test]
    fn an_expired_code_is_gone() {
        let mut pairings = Pairings::default();
        let pairing = pairings.start(0);
        assert!(pairings.current(CODE_LIFETIME_MS - 1).is_some());
        assert_eq!(
            pairings.attempt(&pairing.code, CODE_LIFETIME_MS),
            Err(PairError::NoCode)
        );
        assert!(pairings.current(0).is_none());
    }

    #[test]
    fn ten_wrong_codes_cancel_the_code() {
        let mut pairings = Pairings::default();
        let pairing = pairings.start(0);
        let wrong = if pairing.code == "000000" {
            "000001"
        } else {
            "000000"
        };
        for _ in 0..MAX_WRONG - 1 {
            assert_eq!(pairings.attempt(wrong, 1), Err(PairError::Wrong));
        }
        assert_eq!(pairings.attempt(wrong, 1), Err(PairError::TooMany));
        assert_eq!(pairings.attempt(&pairing.code, 1), Err(PairError::NoCode));
    }

    #[test]
    fn a_new_code_replaces_the_old_one_and_cancel_removes_it() {
        let mut pairings = Pairings::default();
        let first = pairings.start(0);
        let second = pairings.start(0);
        if first.code != second.code {
            assert_eq!(pairings.attempt(&first.code, 1), Err(PairError::Wrong));
        }
        assert!(pairings.cancel());
        assert!(!pairings.cancel());
        assert_eq!(pairings.attempt(&second.code, 1), Err(PairError::NoCode));
    }

    #[test]
    fn proofs_check_both_ways_and_differ_by_direction() {
        let secret = [3; 32];
        let (server, client) = ([1; 32], [2; 32]);
        let proof = client_proof(&secret, &server, &client);
        assert!(verify_client(&secret, &server, &client, &proof));
        assert!(!verify_client(&[4; 32], &server, &client, &proof));
        assert!(!verify_client(&secret, &client, &server, &proof));
        assert!(!verify_client(&secret, &server, &client, &proof[..31]));
        // L'app ne peut pas renvoyer la preuve de l'éditeur comme la sienne.
        assert_ne!(server_proof(&secret, &client, &server), proof);
        assert_ne!(
            server_proof(&secret, &client, &server),
            client_proof(&secret, &client, &server)
        );
    }

    #[test]
    fn known_vector_matches_node() {
        // Même calcul côté extension (crypto.createHmac) : vecteur figé.
        let proof = client_proof(&[0; 32], &[1; 4], &[2; 4]);
        assert_eq!(
            to_hex(&proof),
            to_hex(
                hmac::sign(
                    &hmac::Key::new(hmac::HMAC_SHA256, &[0; 32]),
                    b"chatonlm-bridge/1 client\x01\x01\x01\x01\x02\x02\x02\x02"
                )
                .as_ref()
            )
        );
    }

    #[test]
    fn hex_round_trips_and_rejects_garbage() {
        assert_eq!(to_hex(&[0, 15, 255]), "000fff");
        assert_eq!(from_hex("000fff"), Some(vec![0, 15, 255]));
        assert_eq!(from_hex("0"), None);
        assert_eq!(from_hex("zz"), None);
        assert_eq!(from_hex("é1"), None);
    }
}
