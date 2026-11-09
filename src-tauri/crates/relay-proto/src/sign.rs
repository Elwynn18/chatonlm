//! Signature Ed25519 des requêtes.
//!
//! Message signé (texte, une ligne par champ, ordre fixe, jamais réordonné) :
//!
//! ```text
//! CHATONLM-RELAY-V1
//! <MÉTHODE>
//! <chemin, sans requête>
//! <horodatage Unix en secondes, décimal>
//! <nonce, base64url de 16 octets>
//! <clé publique, base64url de 32 octets>
//! <SHA-256 du corps, hexadécimal minuscule>
//! ```
//!
//! Le corps est signé par son empreinte, octet pour octet : le JSON n'a pas à
//! être normalisé. Ed25519 est déterministe : une même requête donne une même
//! signature (vecteur figé dans les essais, qui garde le format stable).

use crate::{CLOCK_TOLERANCE_SECS, PROTOCOL, b64, hex, sha256};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};

/// Clé publique d'une installation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    pub fn to_b64(&self) -> String {
        b64::encode(&self.0)
    }

    pub fn from_b64(text: &str) -> Option<Self> {
        b64::decode_array::<32>(text).map(Self)
    }
}

impl std::fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PublicKey({})", self.to_b64())
    }
}

/// Clé d'une installation : la graine de 32 octets (secrète) et sa paire.
pub struct Identity {
    seed: [u8; 32],
    pair: Ed25519KeyPair,
}

impl Identity {
    /// Nouvelle clé, tirée de l'aléa du système.
    pub fn generate() -> Self {
        Self::from_seed(crate::random_bytes())
    }

    pub fn from_seed(seed: [u8; 32]) -> Self {
        let pair = Ed25519KeyPair::from_seed_unchecked(&seed)
            .expect("any 32-byte seed is a valid Ed25519 key");
        Self { seed, pair }
    }

    /// Graine à garder (fichier privé de l'app).
    pub fn seed(&self) -> &[u8; 32] {
        &self.seed
    }

    pub fn public_key(&self) -> PublicKey {
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(self.pair.public_key().as_ref());
        PublicKey(bytes)
    }

    /// En-têtes d'une requête signée, avec un nonce neuf.
    pub fn sign_request(&self, method: &str, path: &str, time: u64, body: &[u8]) -> SignedHeaders {
        self.sign_request_with_nonce(method, path, time, crate::random_bytes(), body)
    }

    pub fn sign_request_with_nonce(
        &self,
        method: &str,
        path: &str,
        time: u64,
        nonce: [u8; 16],
        body: &[u8],
    ) -> SignedHeaders {
        let key = self.public_key();
        let message = signing_message(method, path, time, &nonce, &key, body);
        let signature = self.pair.sign(&message);
        SignedHeaders {
            key: key.to_b64(),
            time: time.to_string(),
            nonce: b64::encode(&nonce),
            signature: b64::encode(signature.as_ref()),
        }
    }
}

impl std::fmt::Debug for Identity {
    // Jamais la graine.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Identity({})", self.public_key().to_b64())
    }
}

/// Valeurs des quatre en-têtes d'une requête signée.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedHeaders {
    pub key: String,
    pub time: String,
    pub nonce: String,
    pub signature: String,
}

impl SignedHeaders {
    pub fn pairs(&self) -> [(&'static str, &str); 4] {
        [
            (crate::HEADER_KEY, &self.key),
            (crate::HEADER_TIME, &self.time),
            (crate::HEADER_NONCE, &self.nonce),
            (crate::HEADER_SIGNATURE, &self.signature),
        ]
    }
}

/// Message signé (voir l'en-tête du module).
pub fn signing_message(
    method: &str,
    path: &str,
    time: u64,
    nonce: &[u8; 16],
    key: &PublicKey,
    body: &[u8],
) -> Vec<u8> {
    format!(
        "{PROTOCOL}\n{}\n{path}\n{time}\n{}\n{}\n{}",
        method.to_ascii_uppercase(),
        b64::encode(nonce),
        key.to_b64(),
        hex(&sha256(body)),
    )
    .into_bytes()
}

/// Requête dont la signature est vérifiée.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verified {
    pub key: PublicKey,
    pub time: u64,
    pub nonce: [u8; 16],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureError {
    /// En-tête absent.
    Missing,
    /// En-tête illisible (base64, longueur, nombre).
    Malformed,
    /// Horodatage hors de la tolérance : l'app corrige son horloge.
    ClockSkew,
    /// Signature fausse (autre clé, corps, chemin ou heure changés).
    BadSignature,
}

/// Vérifie une requête signée. L'heure est vérifiée avant la signature (moins
/// coûteux) ; le rejeu (nonce déjà vu) est l'affaire du serveur.
pub fn verify_request(
    method: &str,
    path: &str,
    headers: &SignedHeaders,
    body: &[u8],
    now: u64,
) -> Result<Verified, SignatureError> {
    if headers.key.is_empty()
        || headers.time.is_empty()
        || headers.nonce.is_empty()
        || headers.signature.is_empty()
    {
        return Err(SignatureError::Missing);
    }
    let key = PublicKey::from_b64(&headers.key).ok_or(SignatureError::Malformed)?;
    let nonce = b64::decode_array::<16>(&headers.nonce).ok_or(SignatureError::Malformed)?;
    let signature = b64::decode_array::<64>(&headers.signature).ok_or(SignatureError::Malformed)?;
    if headers.time.len() > 20 || !headers.time.bytes().all(|b| b.is_ascii_digit()) {
        return Err(SignatureError::Malformed);
    }
    let time: u64 = headers
        .time
        .parse()
        .map_err(|_| SignatureError::Malformed)?;
    if time.abs_diff(now) > CLOCK_TOLERANCE_SECS {
        return Err(SignatureError::ClockSkew);
    }
    let message = signing_message(method, path, time, &nonce, &key, body);
    UnparsedPublicKey::new(&ED25519, key.0)
        .verify(&message, &signature)
        .map_err(|_| SignatureError::BadSignature)?;
    Ok(Verified { key, time, nonce })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_762_600_000;

    fn identity() -> Identity {
        Identity::from_seed([7u8; 32])
    }

    #[test]
    fn signed_request_verifies() {
        let id = identity();
        let headers = id.sign_request("post", "/v1/search", NOW, br#"{"query":"chat"}"#);
        let verified = verify_request(
            "POST",
            "/v1/search",
            &headers,
            br#"{"query":"chat"}"#,
            NOW + 10,
        )
        .unwrap();
        assert_eq!(verified.key, id.public_key());
        assert_eq!(verified.time, NOW);
    }

    #[test]
    fn any_change_breaks_the_signature() {
        let id = identity();
        let body = br#"{"query":"chat"}"#;
        let headers = id.sign_request("POST", "/v1/search", NOW, body);
        let check = |method, path, headers: &SignedHeaders, body: &[u8]| {
            verify_request(method, path, headers, body, NOW)
        };
        assert_eq!(
            check("POST", "/v1/search", &headers, br#"{"query":"chien"}"#),
            Err(SignatureError::BadSignature)
        );
        assert_eq!(
            check("POST", "/v1/status", &headers, body),
            Err(SignatureError::BadSignature)
        );
        assert_eq!(
            check("GET", "/v1/search", &headers, body),
            Err(SignatureError::BadSignature)
        );
        let mut later = headers.clone();
        later.time = (NOW + 1).to_string();
        assert_eq!(
            check("POST", "/v1/search", &later, body),
            Err(SignatureError::BadSignature)
        );
        let mut other_nonce = headers.clone();
        other_nonce.nonce = b64::encode(&[1u8; 16]);
        assert_eq!(
            check("POST", "/v1/search", &other_nonce, body),
            Err(SignatureError::BadSignature)
        );
        // Signature d'une autre clé présentée sous cette clé.
        let other = Identity::from_seed([8u8; 32]).sign_request("POST", "/v1/search", NOW, body);
        let mut forged = headers.clone();
        forged.signature = other.signature;
        assert_eq!(
            check("POST", "/v1/search", &forged, body),
            Err(SignatureError::BadSignature)
        );
    }

    #[test]
    fn clock_skew_is_checked_both_ways() {
        let id = identity();
        let headers = id.sign_request("GET", "/v1/status", NOW, b"");
        let tol = CLOCK_TOLERANCE_SECS;
        assert!(verify_request("GET", "/v1/status", &headers, b"", NOW + tol).is_ok());
        assert!(verify_request("GET", "/v1/status", &headers, b"", NOW - tol).is_ok());
        assert_eq!(
            verify_request("GET", "/v1/status", &headers, b"", NOW + tol + 1),
            Err(SignatureError::ClockSkew)
        );
        assert_eq!(
            verify_request("GET", "/v1/status", &headers, b"", NOW - tol - 1),
            Err(SignatureError::ClockSkew)
        );
    }

    #[test]
    fn malformed_headers_are_refused() {
        let id = identity();
        let good = id.sign_request("GET", "/v1/status", NOW, b"");
        let mut empty = good.clone();
        empty.signature.clear();
        assert_eq!(
            verify_request("GET", "/v1/status", &empty, b"", NOW),
            Err(SignatureError::Missing)
        );
        for (field, value) in [
            ("key", "pas-une-clé"),
            ("key", "AAAA"),
            ("nonce", "AAAA"),
            ("signature", "AAAA"),
            ("time", "-5"),
            ("time", "12a"),
            ("time", "999999999999999999999999"),
        ] {
            let mut bad = good.clone();
            match field {
                "key" => bad.key = value.into(),
                "nonce" => bad.nonce = value.into(),
                "signature" => bad.signature = value.into(),
                _ => bad.time = value.into(),
            }
            assert_eq!(
                verify_request("GET", "/v1/status", &bad, b"", NOW),
                Err(SignatureError::Malformed),
                "{field} = {value}"
            );
        }
    }

    /// Format figé : si ce vecteur change, les apps déjà publiées ne seraient
    /// plus comprises par le relais.
    #[test]
    fn signing_format_is_stable() {
        let id = identity();
        let message = signing_message(
            "post",
            "/v1/search",
            1_700_000_000,
            &[3u8; 16],
            &id.public_key(),
            b"{}",
        );
        let text = String::from_utf8(message).unwrap();
        assert_eq!(
            text,
            format!(
                "CHATONLM-RELAY-V1\nPOST\n/v1/search\n1700000000\nAwMDAwMDAwMDAwMDAwMDAw\n{}\n\
                 44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
                id.public_key().to_b64()
            )
        );
        let headers =
            id.sign_request_with_nonce("POST", "/v1/search", 1_700_000_000, [3u8; 16], b"{}");
        assert_eq!(headers.key, "6kpsY-KcUgq-9VB7Ey7F-ZVHdq6-vnuSQh7qaRRG0iw");
        // Vecteur recalculé hors de ring (OpenSSL, par Node) : format compris
        // par une autre implémentation d'Ed25519.
        assert_eq!(
            headers.signature,
            "QhzmbOdk9Sx2-zCYdy6JmddBCuhap8yxNZatyKj_KKpu-s7BFzvHXwclmpoYKFZdze3-_Z2dXQzhFvnBClNgCg"
        );
    }

    #[test]
    fn debug_never_shows_the_seed() {
        let id = identity();
        let shown = format!("{id:?}");
        assert!(!shown.contains(&b64::encode(id.seed())));
        assert!(shown.contains(&id.public_key().to_b64()));
    }
}
