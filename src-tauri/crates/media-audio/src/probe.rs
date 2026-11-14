//! Reconnaissance du conteneur et du codec.
//!
//! Les conteneurs que symphonia ne lit pas sont reconnus d'abord à leurs
//! premiers octets, pour un refus nommé : sans cela, un MPEG-TS est pris pour
//! de l'AAC sans conteneur (ADTS) et finit par une erreur de décodage, et un
//! AVI n'est qu'un « format inconnu ».

use symphonia::core::codecs::CodecProfile;
use symphonia::core::codecs::audio::AudioCodecId;
use symphonia::core::codecs::audio::well_known::profiles::{
    CODEC_PROFILE_AAC_HE, CODEC_PROFILE_AAC_HE_V2,
};
use symphonia::core::codecs::audio::well_known::*;

use crate::error::{Codec, Container};

/// Octets lus en tête de fichier pour la reconnaissance.
pub(crate) const HEAD: usize = 1024;

/// Conteneur refusé reconnu à ses premiers octets.
pub(crate) fn refused_container(head: &[u8]) -> Option<Container> {
    let at = |offset: usize, magic: &[u8]| head.get(offset..offset + magic.len()) == Some(magic);
    // MPEG-TS : paquets de 188 octets (192 pour le M2TS des caméscopes et
    // Blu-ray), chacun ouvert par l'octet de synchronisation 0x47.
    let ts = |size: usize, first: usize| {
        head.len() > first + size
            && (0..3).all(|n| head.get(first + n * size).is_none_or(|&b| b == 0x47))
    };
    if at(0, b"RIFF") && (at(8, b"AVI ") || at(8, b"AVIX")) {
        Some(Container::Avi)
    } else if at(0, &[0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11]) {
        Some(Container::Asf)
    } else if at(0, b"FLV\x01") {
        Some(Container::Flv)
    } else if at(0, b".RMF") {
        Some(Container::RealMedia)
    } else if at(0, &[0x00, 0x00, 0x01, 0xba]) {
        Some(Container::MpegPs)
    } else if ts(188, 0) || ts(192, 4) {
        Some(Container::MpegTs)
    } else {
        None
    }
}

/// Conteneur d'après le nom court du lecteur de symphonia.
pub(crate) fn container(short_name: &str) -> Container {
    match short_name {
        "isomp4" => Container::Mp4,
        "matroska" | "mkv" | "webm" => Container::Matroska,
        "ogg" => Container::Ogg,
        "wave" | "wav" => Container::Wav,
        "aiff" => Container::Aiff,
        "caf" => Container::Caf,
        "mp1" | "mp2" | "mp3" => Container::Mpeg,
        "flac" => Container::Flac,
        "aac" => Container::Adts,
        _ => Container::Unknown,
    }
}

/// Codec d'après l'identifiant de symphonia.
pub(crate) fn codec(id: AudioCodecId, profile: Option<CodecProfile>) -> Codec {
    match id {
        CODEC_ID_AAC => match profile {
            Some(CODEC_PROFILE_AAC_HE) => Codec::HeAac,
            Some(CODEC_PROFILE_AAC_HE_V2) => Codec::HeAacV2,
            _ => Codec::Aac,
        },
        CODEC_ID_MP3 => Codec::Mp3,
        CODEC_ID_MP2 => Codec::Mp2,
        CODEC_ID_MP1 => Codec::Mp1,
        CODEC_ID_FLAC => Codec::Flac,
        CODEC_ID_VORBIS => Codec::Vorbis,
        CODEC_ID_OPUS => Codec::Opus,
        CODEC_ID_ALAC => Codec::Alac,
        CODEC_ID_AC3 => Codec::Ac3,
        CODEC_ID_EAC3 => Codec::Eac3,
        CODEC_ID_DCA => Codec::Dts,
        CODEC_ID_TRUEHD => Codec::TrueHd,
        CODEC_ID_WMA => Codec::Wma,
        CODEC_ID_ADPCM_G722
        | CODEC_ID_ADPCM_G726
        | CODEC_ID_ADPCM_G726LE
        | CODEC_ID_ADPCM_MS
        | CODEC_ID_ADPCM_IMA_WAV
        | CODEC_ID_ADPCM_IMA_QT => Codec::Adpcm,
        id if PCM.contains(&id) => Codec::Pcm,
        _ => Codec::Other,
    }
}

const PCM: [AudioCodecId; 38] = [
    CODEC_ID_PCM_S32LE,
    CODEC_ID_PCM_S32LE_PLANAR,
    CODEC_ID_PCM_S32BE,
    CODEC_ID_PCM_S32BE_PLANAR,
    CODEC_ID_PCM_S24LE,
    CODEC_ID_PCM_S24LE_PLANAR,
    CODEC_ID_PCM_S24BE,
    CODEC_ID_PCM_S24BE_PLANAR,
    CODEC_ID_PCM_S16LE,
    CODEC_ID_PCM_S16LE_PLANAR,
    CODEC_ID_PCM_S16BE,
    CODEC_ID_PCM_S16BE_PLANAR,
    CODEC_ID_PCM_S8,
    CODEC_ID_PCM_S8_PLANAR,
    CODEC_ID_PCM_U32LE,
    CODEC_ID_PCM_U32LE_PLANAR,
    CODEC_ID_PCM_U32BE,
    CODEC_ID_PCM_U32BE_PLANAR,
    CODEC_ID_PCM_U24LE,
    CODEC_ID_PCM_U24LE_PLANAR,
    CODEC_ID_PCM_U24BE,
    CODEC_ID_PCM_U24BE_PLANAR,
    CODEC_ID_PCM_U16LE,
    CODEC_ID_PCM_U16LE_PLANAR,
    CODEC_ID_PCM_U16BE,
    CODEC_ID_PCM_U16BE_PLANAR,
    CODEC_ID_PCM_U8,
    CODEC_ID_PCM_U8_PLANAR,
    CODEC_ID_PCM_F32LE,
    CODEC_ID_PCM_F32LE_PLANAR,
    CODEC_ID_PCM_F32BE,
    CODEC_ID_PCM_F32BE_PLANAR,
    CODEC_ID_PCM_F64LE,
    CODEC_ID_PCM_F64LE_PLANAR,
    CODEC_ID_PCM_F64BE,
    CODEC_ID_PCM_F64BE_PLANAR,
    CODEC_ID_PCM_ALAW,
    CODEC_ID_PCM_MULAW,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(size: usize, first: usize) -> Vec<u8> {
        let mut data = vec![0u8; HEAD];
        for packet in 0..HEAD / size {
            if let Some(byte) = data.get_mut(first + packet * size) {
                *byte = 0x47;
            }
        }
        data
    }

    #[test]
    fn refused_containers_are_named_by_their_first_bytes() {
        let mut avi = b"RIFF\0\0\0\0AVI LIST".to_vec();
        avi.resize(64, 0);
        assert_eq!(refused_container(&avi), Some(Container::Avi));
        assert_eq!(refused_container(&ts(188, 0)), Some(Container::MpegTs));
        assert_eq!(refused_container(&ts(192, 4)), Some(Container::MpegTs));
        assert_eq!(
            refused_container(&[0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0]),
            Some(Container::Asf)
        );
        assert_eq!(refused_container(b"FLV\x01\x05"), Some(Container::Flv));
        assert_eq!(
            refused_container(&[0, 0, 1, 0xba, 0x44]),
            Some(Container::MpegPs)
        );
    }

    #[test]
    fn readable_containers_and_short_files_are_not_refused() {
        let mut wav = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        wav.resize(64, 0);
        for head in [
            &wav[..],
            b"OggS\0\x02",
            b"fLaC",
            b"\x1aE\xdf\xa3",
            b"",
            b"\x47",
        ] {
            assert_eq!(refused_container(head), None, "{head:02x?}");
        }
        // Un 0x47 isolé (une étiquette ID3 « G… ») n'est pas un MPEG-TS.
        let mut one = vec![0u8; HEAD];
        one[0] = 0x47;
        assert_eq!(refused_container(&one), None);
    }
}
