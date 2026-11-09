//! Preuve de travail de l'inscription : quelques secondes de calcul une seule
//! fois par installation, pour que créer des milliers de clés coûte des heures.
//!
//! 1. Le relais délivre un défi ([`Challenge`]) : version, date, 16 octets
//!    aléatoires et réglages ([`PowParams`]), scellés par un HMAC-SHA256 à lui.
//!    Il n'a rien à retenir : le défi revient avec la preuve, et le sceau dit
//!    qu'il vient de lui (et n'a pas été adouci).
//! 2. L'app cherche `count` nonces croissants dont le hachage Argon2id
//!    ([`pow_hash`]) commence par `bits` bits nuls. Le hachage lie le défi et
//!    la clé publique : une preuve ne sert ni pour une autre clé ni pour un
//!    autre défi, et rien ne se calcule à l'avance.
//! 3. Le relais vérifie avec `count` hachages au plus (il s'arrête au premier
//!    faux).
//!
//! Coût attendu : `count × 2^bits` hachages pour l'app, `count` pour le relais.
//! Argon2id remplit `memory_kib` Kio à chaque hachage : une carte graphique ne
//! parallélise pas beaucoup mieux qu'un processeur. Plusieurs nonces plutôt
//! qu'un seul plus difficile : même coût moyen, durée bien moins dispersée.

use crate::sign::PublicKey;
use crate::{b64, sha256};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

/// Bornes acceptées par l'app (un relais déréglé ne doit pas la faire calculer
/// des heures ni prendre toute sa mémoire) et par le relais.
pub const MIN_MEMORY_KIB: u32 = 8;
pub const MAX_MEMORY_KIB: u32 = 256 * 1024;
pub const MAX_ITERATIONS: u32 = 8;
pub const MAX_PARALLELISM: u32 = 4;
pub const MAX_BITS: u8 = 10;
pub const MAX_COUNT: u8 = 16;
/// Hachages attendus au plus (`count × 2^bits`) : chaque borne seule permettait
/// 16 × 2²⁰ hachages, soit des jours de calcul.
pub const MAX_EXPECTED_HASHES: u64 = 1 << 10;
/// Travail attendu au plus, en Kio remplis par Argon2id (hachages attendus ×
/// mémoire × passes) : 32 Gio, soit 1 024 hachages à 32 Mio. Mesuré le 8 nov.
/// 2026 : 16 ms par hachage à 32 Mio (Ryzen 7 3700X), environ le double sur un
/// portable moyen : ≈ 16 s ici, ≈ 30 s sur un portable au pire réglage permis
/// (les défauts du relais en demandent 8 fois moins).
pub const MAX_WORK_KIB: u64 = 32 * 1024 * 1024;

const VERSION: u8 = 1;
const BODY_LEN: usize = 1 + 8 + 16 + 4 + 4 + 4 + 1 + 1;
const MAC_LEN: usize = 32;
const DOMAIN: &[u8] = b"chatonlm-pow-v1";

/// Réglages de la preuve, choisis par le relais.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowParams {
    /// Mémoire d'un hachage, en Kio.
    pub memory_kib: u32,
    /// Passes d'Argon2id.
    pub iterations: u32,
    /// Voies d'Argon2id (calculées l'une après l'autre ici).
    pub parallelism: u32,
    /// Bits nuls exigés en tête de chaque hachage.
    pub bits: u8,
    /// Nombre de nonces à trouver.
    pub count: u8,
}

impl PowParams {
    /// Réglages dans les bornes.
    pub fn is_valid(&self) -> bool {
        self.parallelism >= 1
            && self.parallelism <= MAX_PARALLELISM
            && self.memory_kib >= MIN_MEMORY_KIB * self.parallelism
            && self.memory_kib <= MAX_MEMORY_KIB
            && self.iterations >= 1
            && self.iterations <= MAX_ITERATIONS
            && self.bits <= MAX_BITS
            && self.count >= 1
            && self.count <= MAX_COUNT
            && self.expected_hashes() <= MAX_EXPECTED_HASHES
            && self.expected_work_kib() <= MAX_WORK_KIB
    }

    /// Travail attendu pour l'app, en Kio remplis par Argon2id.
    pub fn expected_work_kib(&self) -> u64 {
        self.expected_hashes()
            .saturating_mul(u64::from(self.memory_kib))
            .saturating_mul(u64::from(self.iterations))
    }

    /// Hachages attendus pour l'app.
    pub fn expected_hashes(&self) -> u64 {
        u64::from(self.count) << self.bits
    }

    fn argon2(&self) -> Option<Argon2<'static>> {
        let params =
            Params::new(self.memory_kib, self.iterations, self.parallelism, Some(32)).ok()?;
        Some(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
    }
}

/// Défi délivré par le relais.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Challenge {
    pub issued_at: u64,
    pub random: [u8; 16],
    pub params: PowParams,
    body: [u8; BODY_LEN],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChallengeError {
    /// Jeton illisible (longueur, version, base64, réglages hors bornes).
    Malformed,
    /// Sceau faux : défi forgé ou modifié.
    BadMac,
}

impl Challenge {
    fn from_parts(issued_at: u64, random: [u8; 16], params: PowParams) -> Self {
        let mut body = [0u8; BODY_LEN];
        body[0] = VERSION;
        body[1..9].copy_from_slice(&issued_at.to_be_bytes());
        body[9..25].copy_from_slice(&random);
        body[25..29].copy_from_slice(&params.memory_kib.to_be_bytes());
        body[29..33].copy_from_slice(&params.iterations.to_be_bytes());
        body[33..37].copy_from_slice(&params.parallelism.to_be_bytes());
        body[37] = params.bits;
        body[38] = params.count;
        Self {
            issued_at,
            random,
            params,
            body,
        }
    }

    /// Nouveau défi scellé par la clé du relais : le jeton à envoyer.
    pub fn issue(mac_key: &[u8; 32], issued_at: u64, params: PowParams) -> (Self, String) {
        let challenge = Self::from_parts(issued_at, crate::random_bytes(), params);
        let token = challenge.seal(mac_key);
        (challenge, token)
    }

    fn seal(&self, mac_key: &[u8; 32]) -> String {
        let tag = ring::hmac::sign(
            &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, mac_key),
            &self.body,
        );
        let mut token = self.body.to_vec();
        token.extend_from_slice(tag.as_ref());
        b64::encode(&token)
    }

    /// Lit un jeton sans vérifier le sceau (côté app : elle n'a pas la clé).
    pub fn parse(token: &str) -> Result<Self, ChallengeError> {
        Self::split(token).map(|(challenge, _)| challenge)
    }

    /// Lit un jeton et vérifie son sceau (côté relais), en temps constant.
    pub fn open(token: &str, mac_key: &[u8; 32]) -> Result<Self, ChallengeError> {
        let (challenge, tag) = Self::split(token)?;
        ring::hmac::verify(
            &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, mac_key),
            &challenge.body,
            &tag,
        )
        .map_err(|_| ChallengeError::BadMac)?;
        Ok(challenge)
    }

    fn split(token: &str) -> Result<(Self, [u8; MAC_LEN]), ChallengeError> {
        let bytes =
            b64::decode_array::<{ BODY_LEN + MAC_LEN }>(token).ok_or(ChallengeError::Malformed)?;
        if bytes[0] != VERSION {
            return Err(ChallengeError::Malformed);
        }
        let u32_at = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
        let params = PowParams {
            memory_kib: u32_at(25),
            iterations: u32_at(29),
            parallelism: u32_at(33),
            bits: bytes[37],
            count: bytes[38],
        };
        if !params.is_valid() {
            return Err(ChallengeError::Malformed);
        }
        let issued_at = u64::from_be_bytes(bytes[1..9].try_into().expect("8 bytes"));
        let random: [u8; 16] = bytes[9..25].try_into().expect("16 bytes");
        let challenge = Self::from_parts(issued_at, random, params);
        let tag: [u8; MAC_LEN] = bytes[BODY_LEN..].try_into().expect("32 bytes");
        Ok((challenge, tag))
    }
}

/// Hachage d'un nonce : Argon2id(domaine ‖ défi ‖ clé ‖ nonce, sel = aléa du
/// défi).
pub fn pow_hash(challenge: &Challenge, key: &PublicKey, nonce: u64) -> Option<[u8; 32]> {
    let argon2 = challenge.params.argon2()?;
    hash_with(&argon2, challenge, key, nonce)
}

fn hash_with(
    argon2: &Argon2<'_>,
    challenge: &Challenge,
    key: &PublicKey,
    nonce: u64,
) -> Option<[u8; 32]> {
    let mut input = Vec::with_capacity(DOMAIN.len() + BODY_LEN + 32 + 8);
    input.extend_from_slice(DOMAIN);
    input.extend_from_slice(&challenge.body);
    input.extend_from_slice(&key.0);
    input.extend_from_slice(&nonce.to_be_bytes());
    let mut out = [0u8; 32];
    // Le sel lie aussi l'empreinte du défi entier (aléa compris).
    let salt = sha256(&challenge.body);
    argon2
        .hash_password_into(&input, &salt[..16], &mut out)
        .ok()?;
    Some(out)
}

/// Bits nuls en tête.
pub fn leading_zero_bits(hash: &[u8]) -> u32 {
    let mut bits = 0;
    for byte in hash {
        if *byte == 0 {
            bits += 8;
        } else {
            return bits + byte.leading_zeros();
        }
    }
    bits
}

/// Cherche la preuve (côté app, dans une tâche à part : quelques secondes).
/// `None` si `cancel` passe à vrai ou si les réglages sont hors bornes.
/// `progress` reçoit le nombre de nonces trouvés.
pub fn solve(
    challenge: &Challenge,
    key: &PublicKey,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u8),
) -> Option<Vec<u64>> {
    if !challenge.params.is_valid() {
        return None;
    }
    let argon2 = challenge.params.argon2()?;
    let wanted = u32::from(challenge.params.bits);
    let mut found = Vec::with_capacity(usize::from(challenge.params.count));
    let mut nonce = 0u64;
    while found.len() < usize::from(challenge.params.count) {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let hash = hash_with(&argon2, challenge, key, nonce)?;
        if leading_zero_bits(&hash) >= wanted {
            found.push(nonce);
            progress(found.len() as u8);
        }
        nonce = nonce.checked_add(1)?;
    }
    Some(found)
}

/// Vérifie une preuve (côté relais) : exactement `count` nonces, strictement
/// croissants (donc distincts), chacun assez « dur ». Les contrôles gratuits
/// passent d'abord ; le premier hachage faux arrête la vérification.
pub fn verify(challenge: &Challenge, key: &PublicKey, nonces: &[u64]) -> bool {
    if nonces.len() != usize::from(challenge.params.count)
        || !nonces.windows(2).all(|w| w[0] < w[1])
    {
        return false;
    }
    let Some(argon2) = challenge.params.argon2() else {
        return false;
    };
    let wanted = u32::from(challenge.params.bits);
    nonces.iter().all(|nonce| {
        hash_with(&argon2, challenge, key, *nonce)
            .is_some_and(|hash| leading_zero_bits(&hash) >= wanted)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::Identity;

    const MAC_KEY: [u8; 32] = [42u8; 32];

    /// Réglages minuscules : les essais tournent sans optimisation.
    fn tiny() -> PowParams {
        PowParams {
            memory_kib: 8,
            iterations: 1,
            parallelism: 1,
            bits: 3,
            count: 3,
        }
    }

    /// Défi à l'aléa fixé : essais déterministes.
    fn fixed(random: u8) -> (Challenge, String) {
        let challenge = Challenge::from_parts(1_000, [random; 16], tiny());
        let token = challenge.seal(&MAC_KEY);
        (challenge, token)
    }

    #[test]
    fn solved_proof_verifies_and_is_bound_to_key_and_challenge() {
        let key = Identity::from_seed([1u8; 32]).public_key();
        let (challenge, token) = fixed(5);
        let parsed = Challenge::parse(&token).unwrap();
        assert_eq!(parsed, challenge);
        let mut steps = Vec::new();
        let nonces = solve(&parsed, &key, &AtomicBool::new(false), |n| steps.push(n)).unwrap();
        assert_eq!(steps, [1, 2, 3]);
        let opened = Challenge::open(&token, &MAC_KEY).unwrap();
        assert!(verify(&opened, &key, &nonces));
        // Autre clé, ou autre défi aux mêmes réglages : la preuve ne vaut rien.
        let other = Identity::from_seed([2u8; 32]).public_key();
        assert!(!verify(&opened, &other, &nonces));
        assert!(!verify(&fixed(6).0, &key, &nonces));
        // Défi délivré pour de vrai : aléa neuf, scellé, relu.
        let (issued, token) = Challenge::issue(&MAC_KEY, 2_000, tiny());
        assert_eq!(Challenge::open(&token, &MAC_KEY).unwrap(), issued);
        assert_ne!(
            issued.random,
            Challenge::issue(&MAC_KEY, 2_000, tiny()).0.random
        );
    }

    #[test]
    fn proof_with_wrong_shape_is_refused() {
        let key = Identity::from_seed([1u8; 32]).public_key();
        let (challenge, _) = fixed(5);
        let nonces = solve(&challenge, &key, &AtomicBool::new(false), |_| {}).unwrap();
        assert!(verify(&challenge, &key, &nonces));
        // Trop peu, trop, en double, dans le désordre.
        assert!(!verify(&challenge, &key, &nonces[..2]));
        let mut more = nonces.clone();
        more.push(nonces[2] + 1);
        assert!(!verify(&challenge, &key, &more));
        assert!(!verify(
            &challenge,
            &key,
            &[nonces[0], nonces[0], nonces[1]]
        ));
        assert!(!verify(
            &challenge,
            &key,
            &[nonces[1], nonces[0], nonces[2]]
        ));
        // Le dernier nonce remplacé par un nonce qui ne remplit pas la condition.
        let weak = (nonces[1] + 1..)
            .find(|n| leading_zero_bits(&pow_hash(&challenge, &key, *n).unwrap()) < 3)
            .unwrap();
        assert!(!verify(&challenge, &key, &[nonces[0], nonces[1], weak]));
    }

    #[test]
    fn challenge_mac_detects_forgery_and_softening() {
        let (_, token) = Challenge::issue(&MAC_KEY, 1_000, tiny());
        assert_eq!(
            Challenge::open(&token, &[0u8; 32]),
            Err(ChallengeError::BadMac)
        );
        // Réglages adoucis (bits à 0) : le sceau ne correspond plus.
        let mut bytes = b64::decode(&token).unwrap();
        bytes[37] = 0;
        let softened = b64::encode(&bytes);
        assert!(Challenge::parse(&softened).is_ok());
        assert_eq!(
            Challenge::open(&softened, &MAC_KEY),
            Err(ChallengeError::BadMac)
        );
        // Date rajeunie.
        let mut bytes = b64::decode(&token).unwrap();
        bytes[8] ^= 1;
        assert_eq!(
            Challenge::open(&b64::encode(&bytes), &MAC_KEY),
            Err(ChallengeError::BadMac)
        );
        // Illisibles.
        assert_eq!(Challenge::parse("AAAA"), Err(ChallengeError::Malformed));
        let mut bytes = b64::decode(&token).unwrap();
        bytes[0] = 9;
        assert_eq!(
            Challenge::parse(&b64::encode(&bytes)),
            Err(ChallengeError::Malformed)
        );
    }

    #[test]
    fn out_of_bounds_params_are_refused_by_the_app() {
        let key = Identity::from_seed([1u8; 32]).public_key();
        for params in [
            PowParams {
                memory_kib: MAX_MEMORY_KIB + 1,
                ..tiny()
            },
            PowParams {
                memory_kib: 4,
                ..tiny()
            },
            PowParams {
                iterations: 0,
                ..tiny()
            },
            PowParams {
                iterations: MAX_ITERATIONS + 1,
                ..tiny()
            },
            PowParams {
                parallelism: 0,
                ..tiny()
            },
            PowParams {
                parallelism: MAX_PARALLELISM + 1,
                memory_kib: 64,
                ..tiny()
            },
            PowParams {
                bits: MAX_BITS + 1,
                ..tiny()
            },
            PowParams { count: 0, ..tiny() },
            PowParams {
                count: MAX_COUNT + 1,
                ..tiny()
            },
            // Chaque borne tenue, mais leur produit démesuré (des jours de calcul).
            PowParams {
                bits: MAX_BITS,
                count: MAX_COUNT,
                ..tiny()
            },
            PowParams {
                bits: 7,
                count: 16,
                ..tiny()
            },
            // Hachages en nombre permis, mais mémoire et passes au maximum.
            PowParams {
                memory_kib: MAX_MEMORY_KIB,
                iterations: MAX_ITERATIONS,
                bits: 5,
                count: 4,
                parallelism: 1,
            },
        ] {
            assert!(!params.is_valid(), "{params:?}");
            let (_, token) = Challenge::issue(&MAC_KEY, 1, params);
            assert_eq!(
                Challenge::parse(&token),
                Err(ChallengeError::Malformed),
                "{params:?}"
            );
            let challenge = Challenge::from_parts(1, [0; 16], params);
            assert_eq!(
                solve(&challenge, &key, &AtomicBool::new(false), |_| {}),
                None
            );
        }
    }

    /// Le plus dur des réglages permis reste de l'ordre de 30 s sur un portable,
    /// et les défauts du relais y tiennent largement.
    #[test]
    fn the_hardest_allowed_proof_stays_bounded() {
        let hardest = PowParams {
            memory_kib: 32 * 1024,
            iterations: 1,
            parallelism: 1,
            bits: MAX_BITS,
            count: 1,
        };
        assert!(hardest.is_valid());
        assert_eq!(hardest.expected_work_kib(), MAX_WORK_KIB);
        assert!(
            !PowParams {
                count: 2,
                ..hardest
            }
            .is_valid()
        );
        assert!(
            !PowParams {
                memory_kib: 32 * 1024 + 1,
                ..hardest
            }
            .is_valid()
        );
        // 16 ms par hachage à 32 Mio, doublé pour un portable : 30 s au plus.
        let laptop_ms = hardest.expected_work_kib() / (32 * 1024) * 32;
        assert!(laptop_ms <= 33_000, "{laptop_ms} ms");
    }

    #[test]
    fn solving_can_be_cancelled() {
        let key = Identity::from_seed([1u8; 32]).public_key();
        let hard = PowParams {
            bits: MAX_BITS,
            count: 1,
            ..tiny()
        };
        assert!(hard.is_valid());
        let (challenge, _) = Challenge::issue(&MAC_KEY, 1, hard);
        let cancel = AtomicBool::new(true);
        assert_eq!(solve(&challenge, &key, &cancel, |_| {}), None);
    }

    #[test]
    fn leading_zeros() {
        assert_eq!(leading_zero_bits(&[0, 0, 0b0001_0000]), 19);
        assert_eq!(leading_zero_bits(&[0x80]), 0);
        assert_eq!(leading_zero_bits(&[0, 0]), 16);
        assert_eq!(tiny().expected_hashes(), 24);
    }

    /// Mesure (hors des essais ordinaires) : temps d'un hachage aux réglages
    /// par défaut du relais, à lancer optimisé :
    /// `cargo test --release -p relay-proto measure_pow -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn measure_pow() {
        let key = Identity::from_seed([1u8; 32]).public_key();
        for memory_kib in [16 * 1024, 32 * 1024, 64 * 1024] {
            let params = PowParams {
                memory_kib,
                iterations: 1,
                parallelism: 1,
                bits: 0,
                count: 1,
            };
            let (challenge, _) = Challenge::issue(&MAC_KEY, 1, params);
            let start = std::time::Instant::now();
            for nonce in 0..20 {
                pow_hash(&challenge, &key, nonce).unwrap();
            }
            println!("{memory_kib} Kio : {:?} par hachage", start.elapsed() / 20);
        }
    }
}
