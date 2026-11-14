//! Opus (OGG, MKV/WebM) : symphonia démuxe les paquets mais n'a pas de
//! décodeur Opus. Ils passent par opus-pure, le décodeur en Rust pur
//! qu'Easy Link a retenu après essai contre libopus (voir
//! `easy-link/src/voice.rs`) ; sur la parole, sortie identique à l'arrondi
//! près (rapport 105, § 3.1 : corrélation 0,99999999, décalage 0).
//!
//! La lecture de l'en-tête `OpusHead` suit celle d'Easy Link, sans la mettre
//! en commun : ce crate-là lit l'OGG lui-même et n'a pas à dépendre de
//! symphonia.

use opus_pure::{MAX_PACKET_SAMPLES, OpusDecoder};

/// Opus se décode toujours à 48 kHz, quelle que soit la prise de son.
pub(crate) const RATE: u32 = 48_000;

/// En-tête d'identification (RFC 7845, § 5.1), dans les données de la piste.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Head {
    pub channels: usize,
    /// Échantillons d'amorce du décodeur, à retirer au début.
    pub pre_skip: usize,
}

impl Head {
    /// `Err` : en-tête absent ou abîmé ; `Ok(None)` : son multicanal (familles
    /// 1 et suivantes), qu'opus-pure ne décode pas.
    pub(crate) fn parse(data: &[u8]) -> Result<Option<Self>, &'static str> {
        if data.len() < 19 || !data.starts_with(b"OpusHead") {
            return Err("missing OpusHead");
        }
        // Une version majeure autre que 0 annonce un format incompatible.
        if data[8] >> 4 != 0 {
            return Err("unknown OpusHead version");
        }
        let channels = usize::from(data[9]);
        if channels == 0 {
            return Err("OpusHead without channels");
        }
        // Famille 0 : mono ou stéréo, un seul flux. Le gain de sortie
        // (octets 16 et 17) est ignoré : le volume d'écoute ne regarde pas
        // la transcription.
        if data[18] != 0 || channels > 2 {
            return Ok(None);
        }
        Ok(Some(Self {
            channels,
            pre_skip: usize::from(u16::from_le_bytes([data[10], data[11]])),
        }))
    }
}

/// Décodeur d'une piste Opus, rendu en mono.
pub(crate) struct Decoder {
    inner: OpusDecoder,
    channels: usize,
    frame: Vec<f32>,
    /// Amorce restant à retirer.
    skip: usize,
}

impl Decoder {
    pub(crate) fn new(head: Head) -> Result<Self, &'static str> {
        let inner = OpusDecoder::new(RATE as i32, head.channels).map_err(|_| "opus decoder")?;
        Ok(Self {
            inner,
            channels: head.channels,
            frame: vec![0.0; MAX_PACKET_SAMPLES * head.channels],
            skip: head.pre_skip,
        })
    }

    /// Décode un paquet et ajoute ses échantillons, ramenés en mono, à
    /// `mono`. Un paquet vide (qui voudrait dire « perdu » et ferait inventer
    /// 120 ms de son au décodeur) est sauté.
    pub(crate) fn decode(&mut self, packet: &[u8], mono: &mut Vec<f32>) -> Result<(), ()> {
        if packet.is_empty() {
            return Ok(());
        }
        let count = self
            .inner
            .decode(packet, MAX_PACKET_SAMPLES, &mut self.frame)
            .map_err(|_| ())?;
        let pcm = self.frame.get(..count * self.channels).ok_or(())?;
        let skipped = self.skip.min(count);
        self.skip -= skipped;
        for frame in pcm[skipped * self.channels..].chunks_exact(self.channels) {
            mono.push(frame.iter().sum::<f32>() / self.channels as f32);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(channels: u8, pre_skip: u16, family: u8) -> Vec<u8> {
        let mut data = b"OpusHead".to_vec();
        data.push(1);
        data.push(channels);
        data.extend(pre_skip.to_le_bytes());
        data.extend(48_000u32.to_le_bytes());
        data.extend([0, 0]);
        data.push(family);
        data
    }

    #[test]
    fn the_header_gives_channels_and_pre_skip() {
        assert_eq!(
            Head::parse(&head(2, 312, 0)),
            Ok(Some(Head {
                channels: 2,
                pre_skip: 312
            }))
        );
    }

    #[test]
    fn multichannel_and_damaged_headers_are_told_apart() {
        assert_eq!(Head::parse(&head(6, 312, 1)), Ok(None));
        assert_eq!(Head::parse(&head(3, 312, 0)), Ok(None));
        assert!(Head::parse(&head(0, 312, 0)).is_err());
        assert!(Head::parse(&head(1, 312, 0)[..18]).is_err());
        let mut version = head(1, 312, 0);
        version[8] = 0x10;
        assert!(Head::parse(&version).is_err());
        assert!(Head::parse(b"OpusTags...........").is_err());
    }
}
