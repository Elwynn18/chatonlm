//! Vérification d'une signature OpenPGP détachée (RFC 4880 / 9580, paquets
//! v4) avec `ring`, sans bibliothèque OpenPGP : juste ce qu'il faut pour
//! `SHA2-256SUMS.sig` de yt-dlp, c'est-à-dire une signature v4 de type
//! « document binaire » (0x00), RSA PKCS#1 v1.5, SHA-512 (ou SHA-256),
//! faite par la clé primaire RSA v4 épinglée ([`pinned_ytdlp`]).
//!
//! Contrôles : empreinte de la clé (SHA-1 du paquet, v4), émetteur de la
//! signature (empreinte, sinon identifiant) égal à cette clé, date de
//! création présente, expiration de la signature dépassée refusée,
//! sous-paquet critique inconnu refusé (RFC 4880 § 5.2.3.1), 16 premiers
//! bits du condensat, puis la vérification RSA (module de 2048 à 8192 bits).
//! Tout le reste (v3, v5, v6, DSA, ECC, signatures de texte, longueurs
//! partielles) est refusé. Logique pure, essayée sur les deux systèmes.

use base64::Engine as _;
use ring::signature::{
    RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_2048_8192_SHA512, RsaParameters, RsaPublicKeyComponents,
};

/// Clé publique de yt-dlp (`public.key` à la racine de son dépôt, relevée le
/// 14 nov. 2026) : « Simon Sawicki (yt-dlp signing key) », RSA 4096, créée
/// le 26 févr. 2023.
const YTDLP_KEY: &str = include_str!("keys/yt-dlp.asc");

/// Empreinte attendue de [`YTDLP_KEY`] (vérifiée au chargement).
pub const YTDLP_FINGERPRINT: &str = "AC0CBBE6848D6A873464AF4E57CF65933B5A7581";

/// Taille maximale d'une clé ou d'une signature lue.
const MAX_INPUT: usize = 64 * 1024;

/// Pourquoi une signature est refusée.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SigError {
    /// Données illisibles (paquet tronqué, armure abîmée…).
    Malformed(String),
    /// Forme valide mais non prise en charge (algorithme, version…).
    Unsupported(String),
    /// Signée par une autre clé que celle attendue.
    OtherKey,
    /// Signature fausse (données ou signature changées).
    Bad,
    /// Signature expirée.
    Expired,
}

impl std::fmt::Display for SigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // i18n-ignore : détails techniques, dans un message traduit.
        match self {
            SigError::Malformed(detail) => write!(f, "malformed OpenPGP data: {detail}"),
            SigError::Unsupported(detail) => write!(f, "unsupported OpenPGP data: {detail}"),
            SigError::OtherKey => write!(f, "signed by another key"),
            SigError::Bad => write!(f, "bad signature"),
            SigError::Expired => write!(f, "expired signature"),
        }
    }
}

fn malformed(detail: &str) -> SigError {
    SigError::Malformed(detail.to_owned())
}

/// Clé publique RSA v4.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKey {
    n: Vec<u8>,
    e: Vec<u8>,
    fingerprint: [u8; 20],
}

impl PublicKey {
    /// Clé en armure (`-----BEGIN PGP PUBLIC KEY BLOCK-----`) ou binaire :
    /// le premier paquet doit être la clé primaire.
    pub fn parse(input: &[u8]) -> Result<PublicKey, SigError> {
        let bytes = binary_or_armored(input)?;
        let (tag, body, _) = read_packet(&bytes)?;
        if tag != 6 {
            return Err(malformed("first packet is not a public key"));
        }
        Self::from_body(body)
    }

    fn from_body(body: &[u8]) -> Result<PublicKey, SigError> {
        let mut reader = Reader(body);
        if reader.u8()? != 4 {
            return Err(SigError::Unsupported("key version".into()));
        }
        reader.take(4)?;
        // 1 : RSA (chiffrer ou signer) ; 3 : RSA, signature seule.
        if !matches!(reader.u8()?, 1 | 3) {
            return Err(SigError::Unsupported("key algorithm".into()));
        }
        let n = reader.mpi()?.to_vec();
        let e = reader.mpi()?.to_vec();
        if !reader.0.is_empty() {
            return Err(malformed("trailing bytes in the key packet"));
        }
        let mut hashed = vec![0x99];
        hashed.extend_from_slice(&(body.len() as u16).to_be_bytes());
        hashed.extend_from_slice(body);
        let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &hashed);
        let mut fingerprint = [0u8; 20];
        fingerprint.copy_from_slice(digest.as_ref());
        Ok(PublicKey { n, e, fingerprint })
    }

    /// Empreinte v4 en hexadécimal majuscule.
    pub fn fingerprint(&self) -> String {
        hex_upper(&self.fingerprint)
    }

    /// Identifiant (8 derniers octets de l'empreinte).
    fn key_id(&self) -> &[u8] {
        &self.fingerprint[12..]
    }
}

/// Clé de yt-dlp épinglée (empreinte contrôlée).
pub fn pinned_ytdlp() -> PublicKey {
    let key = PublicKey::parse(YTDLP_KEY.as_bytes()).expect("clé de yt-dlp lisible");
    assert_eq!(
        key.fingerprint(),
        YTDLP_FINGERPRINT,
        "clé de yt-dlp épinglée"
    );
    key
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

/// Lecteur d'octets.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], SigError> {
        if self.0.len() < count {
            return Err(malformed("truncated packet"));
        }
        let (head, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, SigError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<usize, SigError> {
        let bytes = self.take(2)?;
        Ok(usize::from(u16::from_be_bytes([bytes[0], bytes[1]])))
    }

    /// Entier multiprécision : nombre de bits sur 16 bits, puis les octets,
    /// sans zéro de tête.
    fn mpi(&mut self) -> Result<&'a [u8], SigError> {
        let bits = self.u16()?;
        let bytes = self.take(bits.div_ceil(8))?;
        let exact = bytes.first().is_some_and(|first| {
            (bytes.len() - 1) * 8 + (8 - first.leading_zeros() as usize) == bits
        });
        if !exact {
            return Err(malformed("bad MPI"));
        }
        Ok(bytes)
    }
}

/// Paquet en tête de `input` : (étiquette, corps, reste). Formats ancien et
/// nouveau ; longueurs partielles et indéterminées refusées.
fn read_packet(input: &[u8]) -> Result<(u8, &[u8], &[u8]), SigError> {
    let mut reader = Reader(input);
    let first = reader.u8()?;
    if first & 0x80 == 0 {
        return Err(malformed("not a packet"));
    }
    let (tag, len) = if first & 0x40 == 0 {
        let tag = (first >> 2) & 0x0f;
        let len = match first & 0x03 {
            0 => usize::from(reader.u8()?),
            1 => reader.u16()?,
            2 => {
                let bytes = reader.take(4)?;
                u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize
            }
            _ => return Err(SigError::Unsupported("indeterminate length".into())),
        };
        (tag, len)
    } else {
        let tag = first & 0x3f;
        let octet = reader.u8()?;
        let len = match octet {
            0..=191 => usize::from(octet),
            192..=223 => ((usize::from(octet) - 192) << 8) + usize::from(reader.u8()?) + 192,
            255 => {
                let bytes = reader.take(4)?;
                u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize
            }
            _ => return Err(SigError::Unsupported("partial length".into())),
        };
        (tag, len)
    };
    let body = reader.take(len)?;
    Ok((tag, body, reader.0))
}

/// CRC-24 de l'armure (RFC 4880 § 6.1).
fn crc24(data: &[u8]) -> u32 {
    let mut crc: u32 = 0x00B7_04CE;
    for byte in data {
        crc ^= u32::from(*byte) << 16;
        for _ in 0..8 {
            crc <<= 1;
            if crc & 0x0100_0000 != 0 {
                crc ^= 0x0186_4CFB;
            }
        }
    }
    crc & 0x00FF_FFFF
}

/// Contenu d'une armure (en-têtes sautés, somme CRC-24 vérifiée si elle
/// est là), ou les octets tels quels s'ils ne sont pas en armure.
fn binary_or_armored(input: &[u8]) -> Result<Vec<u8>, SigError> {
    if input.len() > MAX_INPUT {
        return Err(malformed("too large"));
    }
    let Ok(text) = std::str::from_utf8(input) else {
        return Ok(input.to_vec());
    };
    let Some(start) = text.find("-----BEGIN PGP ") else {
        return Ok(input.to_vec());
    };
    let mut lines = text[start..].lines().skip(1);
    // En-têtes (« Version: … ») jusqu'à la ligne vide.
    for line in lines.by_ref() {
        if line.trim().is_empty() {
            break;
        }
        if !line.contains(':') {
            return Err(malformed("armor header"));
        }
    }
    let mut body = String::new();
    let mut checksum = None;
    let mut ended = false;
    for line in lines {
        let line = line.trim();
        if line.starts_with("-----END PGP ") {
            ended = true;
            break;
        }
        if let Some(sum) = line.strip_prefix('=') {
            checksum = Some(sum.to_owned());
        } else {
            body.push_str(line);
        }
    }
    if !ended {
        return Err(malformed("armor without end"));
    }
    let engine = base64::engine::general_purpose::STANDARD;
    let bytes = engine
        .decode(body.as_bytes())
        .map_err(|_| malformed("armor base64"))?;
    if let Some(sum) = checksum {
        let expected = engine
            .decode(sum.as_bytes())
            .map_err(|_| malformed("armor checksum"))?;
        if expected != crc24(&bytes).to_be_bytes()[1..] {
            return Err(malformed("armor checksum"));
        }
    }
    Ok(bytes)
}

/// Sous-paquet : type sans le bit critique, critique, données.
type Subpacket<'a> = (u8, bool, &'a [u8]);

/// Sous-paquets d'une zone.
fn subpackets(mut area: &[u8]) -> Result<Vec<Subpacket<'_>>, SigError> {
    let mut out = Vec::new();
    while !area.is_empty() {
        let mut reader = Reader(area);
        let octet = reader.u8()?;
        let len = match octet {
            0..=191 => usize::from(octet),
            192..=254 => ((usize::from(octet) - 192) << 8) + usize::from(reader.u8()?) + 192,
            255 => {
                let bytes = reader.take(4)?;
                u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize
            }
        };
        if len == 0 {
            return Err(malformed("empty subpacket"));
        }
        let content = reader.take(len)?;
        out.push((content[0] & 0x7f, content[0] & 0x80 != 0, &content[1..]));
        area = reader.0;
    }
    Ok(out)
}

fn be_u32(bytes: &[u8]) -> Option<u64> {
    let bytes: [u8; 4] = bytes.try_into().ok()?;
    Some(u64::from(u32::from_be_bytes(bytes)))
}

/// Vérifie que `signature` (détachée, binaire ou en armure) signe `data`
/// avec `key` ; `now` : secondes depuis l'époque Unix (expiration).
pub fn verify_detached(
    key: &PublicKey,
    data: &[u8],
    signature: &[u8],
    now: u64,
) -> Result<(), SigError> {
    let bytes = binary_or_armored(signature)?;
    let (tag, body, rest) = read_packet(&bytes)?;
    if tag != 2 {
        return Err(malformed("not a signature packet"));
    }
    if !rest.is_empty() {
        return Err(SigError::Unsupported("several packets".into()));
    }
    let mut reader = Reader(body);
    if reader.u8()? != 4 {
        return Err(SigError::Unsupported("signature version".into()));
    }
    if reader.u8()? != 0x00 {
        return Err(SigError::Unsupported("signature type".into()));
    }
    if reader.u8()? != 1 {
        return Err(SigError::Unsupported("signature algorithm".into()));
    }
    let (params, digest_algorithm): (&RsaParameters, &ring::digest::Algorithm) =
        match reader.u8()? {
            8 => (&RSA_PKCS1_2048_8192_SHA256, &ring::digest::SHA256),
            10 => (&RSA_PKCS1_2048_8192_SHA512, &ring::digest::SHA512),
            _ => return Err(SigError::Unsupported("hash algorithm".into())),
        };
    let hashed_len = reader.u16()?;
    let hashed = reader.take(hashed_len)?;
    // Partie hachée : de la version à la fin des sous-paquets hachés.
    let prefix = &body[..6 + hashed_len];
    let unhashed_len = reader.u16()?;
    let unhashed = reader.take(unhashed_len)?;
    let left16 = reader.take(2)?;
    let value = reader.mpi()?;
    if !reader.0.is_empty() {
        return Err(malformed("trailing bytes in the signature"));
    }

    let mut created = None;
    let mut expires = None;
    let mut issuer_ok = false;
    let mut issuer_bad = false;
    for (kind, critical, content) in subpackets(hashed)? {
        match kind {
            2 => created = be_u32(content),
            3 => expires = be_u32(content),
            33 => {
                // Version de la clé, puis l'empreinte.
                if content.len() == 21 && content[0] == 4 {
                    if content[1..] == key.fingerprint {
                        issuer_ok = true;
                    } else {
                        issuer_bad = true;
                    }
                } else {
                    issuer_bad = true;
                }
            }
            16 => {
                if content == key.key_id() {
                    issuer_ok = true;
                } else {
                    issuer_bad = true;
                }
            }
            // Sans effet sur la validité.
            4 | 5 | 6 | 7 | 9 | 11 | 20 | 21 | 22 | 23 | 25 | 26 | 27 | 28 | 29 | 30 | 31 | 32 => {}
            _ if critical => return Err(SigError::Unsupported("critical subpacket".into())),
            _ => {}
        }
    }
    // L'identifiant de l'émetteur peut aussi se trouver hors de la partie
    // hachée (c'est le cas de yt-dlp).
    for (kind, _, content) in subpackets(unhashed)? {
        if kind == 16 {
            if content == key.key_id() {
                issuer_ok = true;
            } else {
                issuer_bad = true;
            }
        }
    }
    if issuer_bad || !issuer_ok {
        return Err(SigError::OtherKey);
    }
    let Some(created) = created else {
        return Err(malformed("no creation time"));
    };
    if let Some(expires) = expires.filter(|seconds| *seconds > 0)
        && now >= created.saturating_add(expires)
    {
        return Err(SigError::Expired);
    }

    let mut message = data.to_vec();
    message.extend_from_slice(prefix);
    message.extend_from_slice(&[4, 0xff]);
    message.extend_from_slice(&(prefix.len() as u32).to_be_bytes());
    if ring::digest::digest(digest_algorithm, &message).as_ref()[..2] != *left16 {
        return Err(SigError::Bad);
    }
    // La signature, complétée à gauche jusqu'à la taille du module.
    if value.len() > key.n.len() {
        return Err(SigError::Bad);
    }
    let mut padded = vec![0u8; key.n.len() - value.len()];
    padded.extend_from_slice(value);
    RsaPublicKeyComponents {
        n: &key.n,
        e: &key.e,
    }
    .verify(params, &message, &padded)
    .map_err(|_| SigError::Bad)
}
