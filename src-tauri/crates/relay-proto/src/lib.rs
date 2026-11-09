//! Protocole du relais de ChatonLM (version 1), partagé par l'app et par le
//! serveur `chatonlm-relay`.
//!
//! Le relais garde les clés payantes des moteurs de recherche, applique quotas
//! et budget, et mélange les utilisateurs derrière un seul compte : le
//! fournisseur voit la requête, jamais l'utilisateur. Sans compte :
//!
//! 1. chaque installation crée une clé Ed25519 ([`sign::Identity`]) ;
//! 2. elle s'inscrit une fois par une preuve de travail Argon2id
//!    ([`pow`]) : `POST /v1/challenge`, puis `POST /v1/register` signé ;
//! 3. chaque requête est signée ([`sign::signing_message`]) : méthode,
//!    chemin, horodatage (± [`CLOCK_TOLERANCE_SECS`]), nonce et SHA-256 du
//!    corps. Rejouer une requête, en changer le corps ou la dater autrement
//!    est refusé.
//!
//! Points d'accès : `GET /health`, `POST /v1/challenge`, `POST /v1/register`
//! (signé, clé pas encore inscrite), `POST /v1/search` et `GET /v1/status`
//! (signés). Erreurs : `{"error": {"code", "retryAt"?, "serverTime"?}}`, codes
//! stables ([`api::ErrorCode`]) ; l'app traduit le code, jamais un texte du
//! serveur.
//!
//! Dépendances : `ring` (Ed25519, HMAC, SHA-256, aléa), `argon2`, `base64`,
//! `serde`. Aucune entrée/sortie ici : fonctions pures, essayées seules.

pub mod api;
pub mod pow;
pub mod sign;
pub mod time;
pub mod version;

/// Version du protocole, dans le chemin (`/v1/...`) et dans le message signé.
pub const PROTOCOL: &str = "CHATONLM-RELAY-V1";

/// En-têtes d'une requête signée.
pub const HEADER_KEY: &str = "x-chatonlm-key";
pub const HEADER_TIME: &str = "x-chatonlm-time";
pub const HEADER_NONCE: &str = "x-chatonlm-nonce";
pub const HEADER_SIGNATURE: &str = "x-chatonlm-signature";

/// Écart d'horloge accepté entre l'app et le relais, en secondes. Au-delà, le
/// relais répond `clock_skew` avec son heure (`serverTime`) : l'app corrige son
/// décalage et réessaie une fois.
pub const CLOCK_TOLERANCE_SECS: u64 = 300;

/// Longueur d'une requête de recherche, en caractères (limite de Brave : 400
/// caractères, 50 mots).
pub const MAX_QUERY_CHARS: usize = 400;
pub const MAX_QUERY_WORDS: usize = 50;

/// Nombre de résultats : 5 par défaut, 10 au plus (comme le `web_search`
/// d'Ollama que le relais remplace).
pub const DEFAULT_RESULTS: u8 = 5;
pub const MAX_RESULTS: u8 = 10;

/// Taille d'un corps de requête accepté par le relais, en octets.
pub const MAX_REQUEST_BODY: usize = 8 * 1024;

/// Base64 « URL » sans remplissage, pour les clés, nonces et signatures.
pub mod b64 {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    pub fn encode(bytes: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(bytes)
    }

    pub fn decode(text: &str) -> Option<Vec<u8>> {
        URL_SAFE_NO_PAD.decode(text).ok()
    }

    /// Décode exactement `N` octets.
    pub fn decode_array<const N: usize>(text: &str) -> Option<[u8; N]> {
        // Borne avant de décoder : un en-tête démesuré n'est pas alloué.
        if text.len() > N * 4 / 3 + 4 {
            return None;
        }
        decode(text)?.try_into().ok()
    }
}

/// Octets aléatoires du système (`getrandom`, `BCryptGenRandom` sous Windows).
pub fn random_bytes<const N: usize>() -> [u8; N] {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; N];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .expect("the system random generator is available");
    bytes
}

/// SHA-256.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

/// Hexadécimal minuscule.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}
