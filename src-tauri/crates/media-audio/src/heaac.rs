//! HE-AAC : un cœur AAC-LC à demi-fréquence, plus des données SBR (et PS pour
//! HE-AACv2) rangées dans des éléments d'extension qu'un décodeur AAC-LC
//! saute. Le décodeur AAC de symphonia ne lit que l'AAC-LC et refuse une
//! AudioSpecificConfig (ASC) qui annonce le SBR : elle est réécrite ici en
//! AAC-LC sur la fréquence du cœur (ISO/IEC 14496-3, § 1.6.2.1).
//!
//! Le cœur à 22 050 Hz (cas des fichiers 139 de YouTube) porte le son jusque
//! vers 11 kHz, au-delà des 8 kHz que garde le passage à 16 kHz : rien n'est
//! perdu pour une transcription. Mesure du rapport 105 (§ 3.1) : corrélation
//! 0,9999994 entre ce cœur et le HE-AAC complet de ffmpeg, filtrés à 8 kHz.

use crate::error::Codec;

/// Fréquences des indices 0 à 12 de l'ASC.
const RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

/// Objets audio de l'ASC.
const LC: u32 = 2;
const SBR: u32 = 5;
const PS: u32 = 29;

/// Cœur AAC-LC d'un HE-AAC.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Core {
    /// ASC réécrite, en AAC-LC.
    pub asc: Vec<u8>,
    /// Fréquence du cœur, celle du son décodé.
    pub rate: u32,
    /// [`Codec::HeAac`] ou [`Codec::HeAacV2`].
    pub codec: Codec,
}

/// Cœur AAC-LC d'une ASC HE-AAC en signalisation explicite (objet 5 ou 29) ;
/// `None` pour toute autre ASC (AAC-LC compris : rien à réécrire), ou pour
/// une ASC illisible (le décodeur la refusera lui-même).
pub(crate) fn core(asc: &[u8]) -> Option<Core> {
    let mut bits = Bits { data: asc, at: 0 };
    let object = bits.object()?;
    if object != SBR && object != PS {
        return None;
    }
    let (index, escaped) = bits.frequency()?;
    let channels = bits.read(4)?;
    // Fréquence de sortie (le double du cœur), sans usage ici.
    bits.frequency()?;
    if bits.object()? != LC {
        return None;
    }
    // GASpecificConfig du cœur : trame courte, dépendance d'un autre codeur
    // (avec son délai), drapeau d'extension (sans objet pour l'AAC-LC).
    let short_frame = bits.read(1)?;
    let depends = bits.read(1)?;
    let delay = if depends == 1 {
        Some(bits.read(14)?)
    } else {
        None
    };
    bits.read(1)?;

    let mut out = Writer::default();
    out.write(LC, 5);
    out.write(index, 4);
    if let Some(rate) = escaped {
        out.write(rate, 24);
    }
    // HE-AACv2 : le cœur est mono (la stéréo vient du PS).
    out.write(if object == PS { 1 } else { channels }, 4);
    out.write(short_frame, 1);
    out.write(depends, 1);
    if let Some(delay) = delay {
        out.write(delay, 14);
    }
    out.write(0, 1);
    let rate = escaped.or_else(|| RATES.get(index as usize).copied())?;
    if rate == 0 {
        return None;
    }
    Some(Core {
        asc: out.finish(),
        rate,
        codec: if object == PS {
            Codec::HeAacV2
        } else {
            Codec::HeAac
        },
    })
}

/// Lecture bit à bit, du poids fort au poids faible.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn read(&mut self, count: usize) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            let byte = *self.data.get(self.at / 8)?;
            let bit = (byte >> (7 - self.at % 8)) & 1;
            value = (value << 1) | u32::from(bit);
            self.at += 1;
        }
        Some(value)
    }

    /// Objet audio (5 bits, ou 32 + 6 bits après l'échappement 31).
    fn object(&mut self) -> Option<u32> {
        match self.read(5)? {
            31 => Some(32 + self.read(6)?),
            object => Some(object),
        }
    }

    /// Indice de fréquence, et fréquence explicite après l'échappement 15.
    fn frequency(&mut self) -> Option<(u32, Option<u32>)> {
        match self.read(4)? {
            15 => Some((15, Some(self.read(24)?))),
            index => Some((index, None)),
        }
    }
}

/// Écriture bit à bit, complétée de zéros jusqu'à l'octet.
#[derive(Default)]
struct Writer {
    data: Vec<u8>,
    bits: usize,
}

impl Writer {
    fn write(&mut self, value: u32, count: usize) {
        for shift in (0..count).rev() {
            if self.bits.is_multiple_of(8) {
                self.data.push(0);
            }
            let bit = ((value >> shift) & 1) as u8;
            if let Some(last) = self.data.last_mut() {
                *last |= bit << (7 - self.bits % 8);
            }
            self.bits += 1;
        }
    }

    fn finish(self) -> Vec<u8> {
        self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_139_becomes_its_stereo_core_at_22050_hz() {
        // ASC réelle d'un fichier 139 de YouTube (relevée le 14 nov. 2026) :
        // objet 5, cœur 22 050 Hz stéréo, sortie 44 100 Hz, puis remplissage.
        let mut asc = vec![0x2b, 0x92, 0x08, 0x00];
        asc.extend([0; 21]);
        let core = core(&asc).unwrap();
        // Objet 2, indice 7, 2 canaux, GA à zéro : l'ASC d'un AAC-LC.
        assert_eq!(core.asc, [0x13, 0x90]);
        assert_eq!(core.rate, 22_050);
        assert_eq!(core.codec, Codec::HeAac);
    }

    #[test]
    fn he_aac_v2_keeps_a_mono_core() {
        // Objet 29, cœur 24 000 Hz annoncé stéréo, sortie 48 000 Hz.
        let core = core(&[0xeb, 0x11, 0x88, 0x00]).unwrap();
        assert_eq!(core.asc, [0x13, 0x08]);
        assert_eq!(core.rate, 24_000);
        assert_eq!(core.codec, Codec::HeAacV2);
    }

    #[test]
    fn an_explicit_core_rate_is_kept() {
        // Objet 5, échappement 15 + 22 000 Hz, 1 canal, sortie 44 100 Hz.
        let mut writer = Writer::default();
        for (value, bits) in [
            (5, 5),
            (15, 4),
            (22_000, 24),
            (1, 4),
            (4, 4),
            (2, 5),
            (0, 3),
        ] {
            writer.write(value, bits);
        }
        let core = core(&writer.finish()).unwrap();
        assert_eq!(core.rate, 22_000);
        let mut bits = Bits {
            data: &core.asc,
            at: 0,
        };
        assert_eq!(bits.object(), Some(2));
        assert_eq!(bits.frequency(), Some((15, Some(22_000))));
        assert_eq!(bits.read(4), Some(1));
    }

    #[test]
    fn other_configs_are_left_alone() {
        // AAC-LC (rien à réécrire), Main, ASC tronquée ou vide, SBR sur un
        // cœur qui n'est pas de l'AAC-LC.
        for asc in [
            &[0x13, 0x90][..],
            &[0x0b, 0x90],
            &[0x2b],
            &[],
            &[0x2b, 0x92, 0x0c, 0x00],
        ] {
            assert_eq!(core(asc), None, "{asc:02x?}");
        }
    }
}
