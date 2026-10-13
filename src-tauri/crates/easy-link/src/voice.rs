//! Message vocal : fichier OGG, codec Opus (Telegram et Discord envoient tous
//! deux du mono à 48 kHz), décodé en échantillons pour le moteur de dictée de
//! l'app.
//!
//! Conteneur : crate `ogg` (pages, sommes de contrôle, paquets à cheval sur
//! plusieurs pages). Codec : crate `opus-pure`, en Rust pur. Il a été retenu
//! après essai de huit décodeurs contre libopus 1.6.1 : sur la parole (SILK et
//! hybride, de 6 à 64 kbit/s, trames de 10 à 120 ms, mono et stéréo) sa sortie
//! est celle de libopus à l'arrondi des flottants près, et des dizaines de
//! milliers de paquets abîmés n'ont produit ni panique ni accès mémoire fautif.
//! Écarts connus, sans effet sur une transcription : après un silence non
//! transmis (DTX), le bruit de confort n'est pas celui de libopus ; une
//! musique très tonale codée en hybride par paquets de 40 ou 120 ms sort au
//! même niveau, mais pas à l'échantillon près.

use std::io::Cursor;
use std::panic::{UnwindSafe, catch_unwind};

use ogg::PacketReader;
use opus_pure::{MAX_PACKET_SAMPLES, OpusDecoder};

/// Opus se décode toujours à 48 kHz, quelle que soit la prise de son.
const RATE: u32 = 48_000;

/// En dessous de 300 ms, le début d'un flux abîmé ne vaut pas d'être transcrit.
const SHORTEST_PARTIAL: usize = RATE as usize * 3 / 10;

/// Son décodé, mono.
#[derive(Clone, Debug, PartialEq)]
pub struct Audio {
    pub samples: Vec<f32>,
    /// Fréquence d'échantillonnage (48 000 Hz pour Opus).
    pub rate: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VoiceError {
    /// Ni un fichier OGG, ni un flux Opus, ou fichier abîmé.
    Unreadable,
    /// Plus long que la durée permise.
    TooLong,
}

/// Décode un message vocal, `max_seconds` au plus ([`VoiceError::TooLong`]
/// au-delà, sans tout décoder). Plusieurs canaux sont ramenés à un seul.
/// Jamais de panique ni d'allocation sans borne sur un fichier malveillant.
///
/// Les échantillons rendus sont entre -1 et 1. Un flux abîmé en cours de
/// route rend ce qui précède le dégât s'il y en a au moins 300 ms. La mémoire
/// prise est bornée par `max_seconds` (4 octets par échantillon), pas par la
/// taille du fichier : un paquet de trois octets peut annoncer 120 ms de son.
pub fn decode_ogg_opus(data: &[u8], max_seconds: u32) -> Result<Audio, VoiceError> {
    without_panic(|| decode(data, max_seconds))
}

/// Les essais n'ont fait paniquer ni `ogg` ni `opus-pure`, mais ce dernier est
/// jeune et répond par une panique à un contrat interne rompu : une panique
/// devient un fichier illisible au lieu d'emporter la tâche qui décode.
fn without_panic(
    decode: impl FnOnce() -> Result<Audio, VoiceError> + UnwindSafe,
) -> Result<Audio, VoiceError> {
    catch_unwind(decode).unwrap_or(Err(VoiceError::Unreadable))
}

/// En-tête d'identification (RFC 7845, § 5.1).
struct OpusHead {
    channels: usize,
    /// Échantillons d'amorce du décodeur, à retirer au début.
    pre_skip: u64,
}

impl OpusHead {
    fn parse(packet: &[u8]) -> Option<Self> {
        if packet.len() < 19 || !packet.starts_with(b"OpusHead") {
            return None;
        }
        // Une version majeure autre que 0 annonce un format incompatible.
        if packet[8] >> 4 != 0 {
            return None;
        }
        let channels = usize::from(packet[9]);
        // Famille 0 : mono ou stéréo, un seul flux. Les autres (son
        // multicanal) demandent un décodeur multiflux qu'aucune messagerie
        // n'emploie pour la voix.
        let family = packet[18];
        if family != 0 || !(1..=2).contains(&channels) {
            return None;
        }
        // Le gain de sortie (octets 16 et 17) est ignoré : le volume d'écoute
        // voulu par l'expéditeur ne regarde pas la transcription.
        Some(Self {
            channels,
            pre_skip: u64::from(u16::from_le_bytes([packet[10], packet[11]])),
        })
    }
}

/// Ramène un échantillon entre -1 et 1 : le décodeur dépasse ces bornes sur
/// un son fort, comme libopus. Une valeur qui n'est pas un nombre devient un
/// silence.
fn bounded(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

fn decode(data: &[u8], max_seconds: u32) -> Result<Audio, VoiceError> {
    // `ogg` ne rend que les paquets de pages dont la somme de contrôle est
    // juste : une page abîmée arrête la lecture comme la fin du fichier.
    let mut reader = PacketReader::new(Cursor::new(data));
    let first = reader.read_packet().ok().flatten();
    let first = first.ok_or(VoiceError::Unreadable)?;
    let head = OpusHead::parse(&first.data).ok_or(VoiceError::Unreadable)?;
    // Le message est le premier flux du fichier : les pages d'un autre flux
    // (entrelacé, ou enchaîné à la suite) sont sautées.
    let serial = first.stream_serial();
    let mut next = move || {
        loop {
            let packet = reader.read_packet().ok().flatten()?;
            if packet.stream_serial() == serial {
                return Some(packet);
            }
        }
    };
    let tags = next().ok_or(VoiceError::Unreadable)?;
    if !tags.data.starts_with(b"OpusTags") {
        return Err(VoiceError::Unreadable);
    }

    let mut decoder =
        OpusDecoder::new(RATE as i32, head.channels).map_err(|_| VoiceError::Unreadable)?;
    let mut frame = vec![0.0f32; MAX_PACKET_SAMPLES * head.channels];
    let longest = u64::from(max_seconds) * u64::from(RATE);
    let mut samples: Vec<f32> = Vec::new();
    // Échantillons décodés, amorce comprise : l'unité des positions du flux.
    let mut decoded = 0u64;
    let mut last_position = 0u64;
    // Vrai une fois la page de fin atteinte : tout autre arrêt (fichier
    // coupé, page ou paquet abîmé) laisse un message incomplet.
    let mut complete = false;

    while let Some(packet) = next() {
        // Un paquet vide voudrait dire « perdu » au décodeur, qui
        // inventerait 120 ms de son.
        if packet.data.is_empty() {
            break;
        }
        let Ok(count) = decoder.decode(&packet.data, MAX_PACKET_SAMPLES, &mut frame) else {
            break;
        };
        let Some(pcm) = frame.get(..count * head.channels) else {
            break;
        };
        for channels in pcm.chunks_exact(head.channels) {
            decoded += 1;
            if decoded <= head.pre_skip {
                continue;
            }
            let mono = channels.iter().sum::<f32>() / head.channels as f32;
            samples.push(bounded(mono));
        }
        last_position = packet.absgp_page();
        // Le dernier paquet peut porter jusqu'à 120 ms de remplissage, que la
        // position finale retire : cette marge évite de refuser un message
        // qui fait juste la durée permise.
        if samples.len() as u64 > longest + MAX_PACKET_SAMPLES as u64 {
            return Err(VoiceError::TooLong);
        }
        if packet.last_in_stream() {
            complete = true;
            break;
        }
    }

    // La position de la dernière page compte les échantillons à garder,
    // amorce comprise : ce qui dépasse est le remplissage du dernier paquet
    // (RFC 7845, § 4.4). Une position nulle n'en est pas une ; au-delà de ce
    // qui a été décodé (flux qui ne commence pas à zéro), elle ne retire rien.
    if last_position != 0 {
        let kept = last_position.saturating_sub(head.pre_skip);
        samples.truncate(usize::try_from(kept).unwrap_or(usize::MAX));
    }
    if samples.len() as u64 > longest {
        return Err(VoiceError::TooLong);
    }
    if samples.is_empty() || (!complete && samples.len() < SHORTEST_PARTIAL) {
        return Err(VoiceError::Unreadable);
    }
    Ok(Audio {
        samples,
        rate: RATE,
    })
}

#[cfg(test)]
mod tests {
    use std::ops::Range;
    use std::time::{Duration, Instant};

    use super::*;

    // Fixtures synthétiques (aucune voix enregistrée) : des sons calculés par
    // ffmpeg n9.0.2 et codés par libopus 1.6.1, le 13 oct. 2026. Pour les
    // refaire à l'octet près, chacune par
    //   ffmpeg -f lavfi -i "<source>" [-af "<filtres>"] -ac <canaux> -ar 48000
    //     -map_metadata -1 -fflags +bitexact -flags:a +bitexact
    //     -c:a libopus -application voip -b:a <débit> <fichier>
    // voice.ogg (1 canal, 24k) :
    //   source  aevalsrc='0.5*sin(2*PI*440*t)':s=48000:d=1.5
    //   filtres afade=t=in:d=0.15,afade=t=out:st=1.35:d=0.15
    // voice-stereo.ogg (2 canaux, 32k) :
    //   source  aevalsrc='0.4*sin(2*PI*440*t)|0.4*sin(2*PI*880*t)':s=48000:d=1
    //   filtres afade=t=in:d=0.1,afade=t=out:st=0.9:d=0.1
    // voice-hybrid.ogg (1 canal, 16k) et voice-silk.ogg (1 canal, 12k) :
    //   source  aevalsrc='(2*mod(125*t+0.1*sin(2*PI*5*t)\,1)-1)
    //             *(0.55+0.45*sin(2*PI*3*t-PI/2))':s=48000:d=2  (sans retour à la ligne)
    //   filtres lowpass=f=3400,equalizer=f=700:t=q:w=2:g=12,
    //             equalizer=f=1200:t=q:w=3:g=8,volume=0.12,
    //             afade=t=in:d=0.05,afade=t=out:st=1.9:d=0.1
    // voice-loud.ogg (1 canal, 24k, sans filtre) :
    //   source  aevalsrc='if(gt(sin(2*PI*220*t)\,0)\,0.98\,-0.98)':s=48000:d=0.4
    // voice-speech.ogg (1 canal, 16k), servi par les faux serveurs : le son
    // voisé en trois salves de 0,35 s séparées de silences, pour passer la
    // garde « y a-t-il de la parole ? » de l'app :
    //   source  aevalsrc='(2*mod(125*t+0.1*sin(2*PI*5*t)\,1)-1)
    //             *lt(mod(t\,0.6)\,0.35)':s=48000:d=2.2  (sans retour à la ligne)
    //   filtres lowpass=f=3400,equalizer=f=700:t=q:w=2:g=12,
    //             equalizer=f=1200:t=q:w=3:g=8,volume=0.12
    /// Ton de 440 Hz d'amplitude 0,5, 1,5 s, mono : paquets hybrides et CELT.
    const VOICE: &[u8] = include_bytes!("../fixtures/voice.ogg");
    /// 440 Hz à gauche, 880 Hz à droite, amplitude 0,4, 1 s.
    const STEREO: &[u8] = include_bytes!("../fixtures/voice-stereo.ogg");
    /// Son voisé de 125 Hz, 2 s, tout en paquets hybrides (SILK + CELT).
    const HYBRID: &[u8] = include_bytes!("../fixtures/voice-hybrid.ogg");
    /// Le même son, tout en paquets SILK.
    const SILK: &[u8] = include_bytes!("../fixtures/voice-silk.ogg");
    /// Signal carré à pleine échelle, 0,4 s : le décodeur le rend au-delà de ±1.
    const LOUD: &[u8] = include_bytes!("../fixtures/voice-loud.ogg");

    /// Début des données de la première page, qui porte `OpusHead`.
    const HEAD: usize = 28;
    /// Fin des deux pages d'en-têtes de chaque fixture.
    const HEADERS: usize = 121;
    /// Amorce annoncée par les fixtures.
    const PRE_SKIP: u64 = 312;

    /// Somme de contrôle d'une page OGG (CRC-32 sans réflexion).
    fn checksum(page: &[u8]) -> u32 {
        let mut sum = 0u32;
        for &byte in page {
            sum ^= u32::from(byte) << 24;
            for _ in 0..8 {
                sum = if sum & 0x8000_0000 != 0 {
                    (sum << 1) ^ 0x04C1_1DB7
                } else {
                    sum << 1
                };
            }
        }
        sum
    }

    /// Étendue de chaque page, tant que le fichier se laisse parcourir.
    fn pages(data: &[u8]) -> Vec<Range<usize>> {
        let mut pages = Vec::new();
        let mut start = 0;
        while data[start..].starts_with(b"OggS") && data.len() >= start + 27 {
            let table = start + 27..start + 27 + usize::from(data[start + 26]);
            let Some(sizes) = data.get(table.clone()) else {
                break;
            };
            let end = table.end + sizes.iter().map(|&size| usize::from(size)).sum::<usize>();
            if end > data.len() {
                break;
            }
            pages.push(start..end);
            start = end;
        }
        pages
    }

    /// Refait les sommes de contrôle : un dégât atteint alors le décodeur
    /// Opus au lieu d'être arrêté par la lecture des pages.
    fn repair(data: &mut [u8]) {
        for page in pages(data) {
            data[page.start + 22..page.start + 26].fill(0);
            let sum = checksum(&data[page.clone()]);
            data[page.start + 22..page.start + 26].copy_from_slice(&sum.to_le_bytes());
        }
    }

    /// Une page faite de paquets entiers. Drapeaux : 2 = début, 4 = fin.
    fn page(serial: u32, sequence: u32, flags: u8, position: u64, packets: &[&[u8]]) -> Vec<u8> {
        let mut table = Vec::new();
        for packet in packets {
            table.extend(std::iter::repeat_n(255u8, packet.len() / 255));
            table.push((packet.len() % 255) as u8);
        }
        let mut page = b"OggS\0".to_vec();
        page.push(flags);
        page.extend(position.to_le_bytes());
        page.extend(serial.to_le_bytes());
        page.extend(sequence.to_le_bytes());
        page.extend([0; 4]);
        page.push(table.len() as u8);
        page.extend(table);
        for packet in packets {
            page.extend(*packet);
        }
        repair(&mut page);
        page
    }

    /// Les paquets Opus d'une fixture, en-têtes exclus.
    fn audio_packets(data: &[u8]) -> Vec<Vec<u8>> {
        let mut reader = PacketReader::new(Cursor::new(data));
        let mut packets = Vec::new();
        while let Some(packet) = reader.read_packet().unwrap() {
            packets.push(packet.data);
        }
        packets.split_off(2)
    }

    /// La fixture avec un octet de son en-tête `OpusHead` remplacé.
    fn with_head_byte(offset: usize, value: u8) -> Vec<u8> {
        let mut data = VOICE.to_vec();
        data[HEAD + offset] = value;
        repair(&mut data);
        data
    }

    /// Une fixture avec une autre position sur sa dernière page.
    fn with_final_position(fixture: &[u8], position: u64) -> Vec<u8> {
        let mut data = fixture.to_vec();
        let last = pages(&data).pop().unwrap();
        data[last.start + 6..last.start + 14].copy_from_slice(&position.to_le_bytes());
        repair(&mut data);
        data
    }

    /// Amplitude et phase (en tours ; 0 pour un sinus parti de zéro au début
    /// du son) de la composante `frequency`, mesurées sur `range`.
    fn component(samples: &[f32], range: Range<usize>, frequency: f64) -> (f64, f64) {
        let (mut sine, mut cosine) = (0.0f64, 0.0f64);
        for index in range.clone() {
            let angle = std::f64::consts::TAU * frequency * index as f64 / f64::from(RATE);
            sine += f64::from(samples[index]) * angle.sin();
            cosine += f64::from(samples[index]) * angle.cos();
        }
        let amplitude = 2.0 * sine.hypot(cosine) / range.len() as f64;
        (amplitude, cosine.atan2(sine) / std::f64::consts::TAU)
    }

    /// Hauteur d'un son voisé : le décalage, entre 100 et 160 Hz, auquel une
    /// tranche de 100 ms se ressemble le plus.
    fn pitch(samples: &[f32]) -> f64 {
        let window = 36_000..40_800;
        let correlation = |lag: usize| {
            let (mut both, mut here, mut there) = (0.0f64, 0.0f64, 0.0f64);
            for index in window.clone() {
                let (a, b) = (f64::from(samples[index]), f64::from(samples[index + lag]));
                both += a * b;
                here += a * a;
                there += b * b;
            }
            both / (here * there).sqrt()
        };
        let lag = (300..=480)
            .max_by(|&a, &b| correlation(a).total_cmp(&correlation(b)))
            .unwrap();
        assert!(correlation(lag) > 0.9, "son non voisé");
        f64::from(RATE) / lag as f64
    }

    fn level(samples: &[f32]) -> f64 {
        let sum: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
        (sum / samples.len() as f64).sqrt()
    }

    /// Suite pseudo-aléatoire reproductible (xorshift).
    struct Random(u64);

    impl Random {
        fn below(&mut self, bound: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % bound as u64) as usize
        }
    }

    #[test]
    fn the_fixture_decodes_to_its_duration_and_pitch() {
        let audio = decode_ogg_opus(VOICE, 60).unwrap();
        assert_eq!(audio.rate, 48_000);
        // 1,5 s à 20 ms près, et même à l'échantillon près : l'amorce et le
        // remplissage de fin sont retirés.
        assert!(audio.samples.len().abs_diff(72_000) <= 960);
        assert_eq!(audio.samples.len(), 72_000);
        assert!(audio.samples.iter().all(|sample| sample.abs() <= 1.0));

        let middle = 14_400..57_600;
        let (amplitude, phase) = component(&audio.samples, middle.clone(), 440.0);
        assert!((amplitude - 0.5).abs() < 0.02, "amplitude {amplitude}");
        // L'amorce est retirée au début, pas à la fin : sinon le ton serait
        // décalé de 312 échantillons, soit 0,14 tour. libopus rend ce
        // fichier avec la même avance de 4 échantillons (0,036 tour).
        assert!((phase - 0.036).abs() < 0.02, "phase {phase}");
        for other in [220.0, 400.0, 480.0, 880.0] {
            let (amplitude, _) = component(&audio.samples, middle.clone(), other);
            assert!(amplitude < 0.01, "{other} Hz : {amplitude}");
        }
    }

    #[test]
    fn speech_like_fixtures_decode_in_silk_and_hybrid_modes() {
        // Les fixtures sont bien ce qu'elles disent : l'octet d'en-tête de
        // chaque paquet donne son mode (RFC 6716, § 3.1).
        assert!(audio_packets(SILK).iter().all(|packet| packet[0] >> 3 < 12));
        assert!(
            audio_packets(HYBRID)
                .iter()
                .all(|packet| (12..16).contains(&(packet[0] >> 3)))
        );
        for fixture in [SILK, HYBRID] {
            let audio = decode_ogg_opus(fixture, 60).unwrap();
            assert_eq!(audio.samples.len(), 96_000);
            let pitch = pitch(&audio.samples);
            assert!((118.0..132.0).contains(&pitch), "hauteur {pitch}");
            let level = level(&audio.samples[24_000..72_000]);
            assert!((0.050..0.061).contains(&level), "niveau {level}");
        }
    }

    #[test]
    fn stereo_is_averaged_to_mono() {
        let audio = decode_ogg_opus(STEREO, 60).unwrap();
        assert_eq!(audio.samples.len(), 48_000);
        // Chaque canal portait son ton à 0,4 : la moyenne garde les deux à
        // 0,2 (un seul canal gardé donnerait 0,4 et 0).
        for frequency in [440.0, 880.0] {
            let (amplitude, _) = component(&audio.samples, 9_600..38_400, frequency);
            assert!(
                (amplitude - 0.2).abs() < 0.02,
                "{frequency} Hz : {amplitude}"
            );
        }
    }

    #[test]
    fn a_shorter_limit_refuses_the_fixture_as_too_long() {
        assert_eq!(decode_ogg_opus(VOICE, 1), Err(VoiceError::TooLong));
        assert_eq!(decode_ogg_opus(VOICE, 0), Err(VoiceError::TooLong));
        assert!(decode_ogg_opus(VOICE, 2).is_ok());
        // Refusé aussi pour 13 ms de trop, moins que la marge d'un paquet :
        // la seconde du message stéréo sans le retrait de son remplissage.
        let barely = with_final_position(STEREO, u64::MAX);
        assert_eq!(decode_ogg_opus(&barely, 1), Err(VoiceError::TooLong));
        assert_eq!(
            decode_ogg_opus(&barely, 2).unwrap().samples.len() as u64,
            51 * 960 - PRE_SKIP
        );
    }

    #[test]
    fn a_message_of_exactly_the_limit_is_accepted() {
        // Deux secondes tout rond, mais 2,013 s décodées avant de retirer le
        // remplissage du dernier paquet.
        assert_eq!(decode_ogg_opus(HYBRID, 2).unwrap().samples.len(), 96_000);
    }

    #[test]
    fn a_long_stream_is_refused_before_its_end() {
        // La dernière page prétend que le message s'arrête à une seconde. Le
        // refus tombe avant de la lire, dès que le son décodé dépasse la
        // limite : attendre la fin rendrait ici une seconde de son.
        let data = with_final_position(VOICE, PRE_SKIP + 48_000);
        assert_eq!(decode_ogg_opus(&data, 1), Err(VoiceError::TooLong));
        assert_eq!(decode_ogg_opus(&data, 2).unwrap().samples.len(), 48_000);
    }

    #[test]
    fn the_final_position_trims_the_padding() {
        let data = with_final_position(VOICE, PRE_SKIP + 52_800);
        assert_eq!(decode_ogg_opus(&data, 60).unwrap().samples.len(), 52_800);
        // Une position absente (-1), nulle ou au-delà du son ne retire rien :
        // les 76 paquets de 20 ms restent, moins l'amorce.
        for position in [u64::MAX, 0, 1_000_000] {
            let data = with_final_position(VOICE, position);
            let audio = decode_ogg_opus(&data, 60).unwrap();
            assert_eq!(audio.samples.len() as u64, 76 * 960 - PRE_SKIP);
        }
        // Une position dans l'amorce : message complet, mais rien à entendre.
        let nothing = with_final_position(VOICE, 100);
        assert_eq!(decode(&nothing, 60), Err(VoiceError::Unreadable));
    }

    #[test]
    fn the_output_gain_of_the_header_is_ignored() {
        // +6 dB demandés à la lecture (Q7.8, petit-boutiste).
        let mut data = VOICE.to_vec();
        data[HEAD + 16..HEAD + 18].copy_from_slice(&1536i16.to_le_bytes());
        repair(&mut data);
        assert_eq!(decode_ogg_opus(&data, 60), decode_ogg_opus(VOICE, 60));
    }

    #[test]
    fn what_is_not_ogg_opus_is_unreadable() {
        // `decode` et non `decode_ogg_opus` : le refus doit venir d'un
        // contrôle, pas d'une panique rattrapée.
        let unreadable = |data: &[u8]| assert_eq!(decode(data, 60), Err(VoiceError::Unreadable));
        unreadable(b"");
        unreadable(b"Bonjour, ceci est un texte et pas un message vocal.\n");
        unreadable(b"OggS");
        unreadable(&[0xA5; 4096]);
        // Un vrai fichier OGG, mais d'un autre codec.
        let mut vorbis = b"\x01vorbis".to_vec();
        vorbis.extend([0; 23]);
        unreadable(&page(1, 0, 2, 0, &[&vorbis]));
        // En-têtes seuls : rien à entendre.
        unreadable(&VOICE[..HEADERS]);
    }

    #[test]
    fn a_wrong_header_is_unreadable() {
        let unreadable = |data: &[u8]| assert_eq!(decode(data, 60), Err(VoiceError::Unreadable));
        // La fixture retouchée sans dégât se décode : c'est bien l'octet
        // changé qui fait refuser les suivantes.
        assert!(decode_ogg_opus(&with_head_byte(8, 1), 60).is_ok());
        // Version majeure inconnue ; une version mineure plus récente passe.
        unreadable(&with_head_byte(8, 0x10));
        assert!(decode_ogg_opus(&with_head_byte(8, 0x0F), 60).is_ok());
        // Aucun canal, trois canaux, famille de canaux multicanal.
        unreadable(&with_head_byte(9, 0));
        unreadable(&with_head_byte(9, 3));
        unreadable(&with_head_byte(18, 1));
        // Signature abîmée, en-tête trop court, `OpusTags` absent.
        unreadable(&with_head_byte(0, b'o'));
        let mut short = page(0, 0, 2, 0, &[&VOICE[HEAD..HEAD + 18]]);
        short.extend(&VOICE[47..]);
        unreadable(&short);
        let mut tags = VOICE.to_vec();
        tags[47 + HEAD] = b'o';
        repair(&mut tags);
        unreadable(&tags);
    }

    #[test]
    fn a_stream_damaged_midway_returns_what_came_before() {
        let last = pages(VOICE).pop().unwrap();
        // Un octet de la dernière page : sa somme de contrôle est fausse, il
        // reste la première seconde.
        let mut data = VOICE.to_vec();
        data[last.start + 300] ^= 0x10;
        let audio = decode_ogg_opus(&data, 60).unwrap();
        assert_eq!(audio.samples.len() as u64, 48_000 - PRE_SKIP);
        assert_eq!(
            audio.samples,
            decode_ogg_opus(VOICE, 60).unwrap().samples[..audio.samples.len()]
        );
        // Un octet de la première page de son : rien avant.
        let mut data = VOICE.to_vec();
        data[HEADERS + 300] ^= 0x10;
        assert_eq!(decode_ogg_opus(&data, 60), Err(VoiceError::Unreadable));
    }

    #[test]
    fn a_short_message_needs_its_end_page() {
        // 200 ms : dix paquets de la fixture sur une seule page.
        let packets = audio_packets(VOICE);
        let ten: Vec<&[u8]> = packets[..10].iter().map(Vec::as_slice).collect();
        let short = |flags| {
            let mut data = VOICE[..HEADERS].to_vec();
            data.extend(page(0, 2, flags, 9_600, &ten));
            decode_ogg_opus(&data, 60)
        };
        // Complet (page de fin), il est rendu ; coupé, 200 ms ne suffisent pas.
        assert_eq!(short(4).unwrap().samples.len() as u64, 9_600 - PRE_SKIP);
        assert_eq!(short(0), Err(VoiceError::Unreadable));
    }

    #[test]
    fn an_empty_packet_ends_the_message() {
        // Vingt paquets, un paquet vide, puis cinq autres : le décodeur
        // prendrait le paquet vide pour un paquet perdu et comblerait.
        let packets = audio_packets(VOICE);
        let mut chosen: Vec<&[u8]> = packets[..20].iter().map(Vec::as_slice).collect();
        chosen.push(&[]);
        chosen.extend(packets[20..25].iter().map(Vec::as_slice));
        let mut data = VOICE[..HEADERS].to_vec();
        data.extend(page(0, 2, 4, 25 * 960, &chosen));
        let audio = decode_ogg_opus(&data, 60).unwrap();
        assert_eq!(audio.samples.len() as u64, 20 * 960 - PRE_SKIP);
    }

    #[test]
    fn loud_sound_stays_between_minus_one_and_one() {
        // La fixture dépasse bien la pleine échelle au sortir du décodeur.
        let mut decoder = OpusDecoder::new(48_000, 1).unwrap();
        let mut frame = vec![0.0f32; MAX_PACKET_SAMPLES];
        let mut highest = 0.0f32;
        for packet in audio_packets(LOUD) {
            let count = decoder
                .decode(&packet, MAX_PACKET_SAMPLES, &mut frame)
                .unwrap();
            highest = frame[..count]
                .iter()
                .fold(highest, |high, s| high.max(s.abs()));
        }
        assert!(highest > 1.2, "crête {highest}");

        let audio = decode_ogg_opus(LOUD, 60).unwrap();
        assert_eq!(audio.samples.len(), 19_200);
        assert!(audio.samples.iter().all(|sample| sample.abs() <= 1.0));
        assert!(audio.samples.contains(&1.0) && audio.samples.contains(&-1.0));

        assert_eq!(bounded(0.25), 0.25);
        assert_eq!(bounded(f32::NAN), 0.0);
        assert_eq!(bounded(f32::INFINITY), 0.0);
    }

    #[test]
    fn pages_of_another_stream_are_skipped() {
        // Une page d'un autre flux, avec un paquet Opus valable, glissée
        // entre les deux pages de son.
        let packets = audio_packets(VOICE);
        let last = pages(VOICE).pop().unwrap();
        let mut data = VOICE[..last.start].to_vec();
        data.extend(page(7, 0, 2, 960, &[&packets[20]]));
        data.extend(&VOICE[last]);
        assert_eq!(decode_ogg_opus(&data, 60), decode_ogg_opus(VOICE, 60));
    }

    #[test]
    fn a_fixture_cut_anywhere_never_panics() {
        let complete = decode(VOICE, 60).unwrap();
        let first_page_end = pages(VOICE)[2].end;
        // À chaque dixième de la longueur, puis tous les 61 octets. `decode`
        // et non `decode_ogg_opus` : une panique doit faire échouer l'essai.
        let tenths = (0..=10).map(|tenth| VOICE.len() * tenth / 10);
        for cut in tenths.chain((0..VOICE.len()).step_by(61)) {
            let result = decode(&VOICE[..cut], 60);
            if cut == VOICE.len() {
                assert_eq!(result.as_ref(), Ok(&complete));
            } else if cut >= first_page_end {
                // La première page de son est entière : une seconde.
                let audio = result.unwrap();
                assert_eq!(audio.samples.len() as u64, 48_000 - PRE_SKIP);
                assert_eq!(audio.samples, complete.samples[..audio.samples.len()]);
            } else {
                assert_eq!(result, Err(VoiceError::Unreadable), "coupé à {cut}");
            }
        }
    }

    /// Nombre de fichiers abîmés essayés.
    const ROUNDS: usize = 3_000;

    #[test]
    fn damaged_bytes_never_panic_and_stay_within_the_limit() {
        // Des messages de 300 ms, pour que l'essai reste court : quinze
        // paquets de chaque fixture sur une page de fin. Ceux des deux tons
        // sont pris à cheval sur leur passage de l'hybride au CELT.
        let short = |fixture: &[u8], from: usize| {
            let packets = audio_packets(fixture);
            let chosen: Vec<&[u8]> = packets[from..from + 15].iter().map(Vec::as_slice).collect();
            let mut data = fixture[..HEADERS].to_vec();
            data.extend(page(0, 2, 4, 15 * 960, &chosen));
            data
        };
        let messages = [
            short(VOICE, 15),
            short(STEREO, 15),
            short(HYBRID, 0),
            short(SILK, 0),
        ];
        for message in &messages {
            assert_eq!(
                decode(message, 1).unwrap().samples.len() as u64,
                15 * 960 - PRE_SKIP
            );
        }

        let mut random = Random(0x9E37_79B9_7F4A_7C15);
        let started = Instant::now();
        let (mut readable, mut refused) = (0, 0);
        for round in 0..ROUNDS {
            let mut data = messages[round % 4].clone();
            if round % 3 == 2 {
                // Une plage d'octets quelconques.
                let at = random.below(data.len());
                let end = (at + 1 + random.below(48)).min(data.len());
                for byte in &mut data[at..end] {
                    *byte = random.below(256) as u8;
                }
            } else {
                // Quelques bits retournés.
                for _ in 0..=random.below(3) {
                    let at = random.below(data.len());
                    data[at] ^= 1 << random.below(8);
                }
            }
            // Un tiers des fichiers garde ses sommes de contrôle fausses (la
            // lecture des pages les arrête) ; pour les autres elles sont
            // refaites, et c'est au décodeur Opus de supporter le dégât.
            if round % 3 != 0 {
                repair(&mut data);
            }
            // `decode` et non `decode_ogg_opus` : une panique doit faire
            // échouer l'essai, pas passer pour un fichier illisible.
            match decode(&data, 1) {
                Ok(audio) => {
                    readable += 1;
                    assert_eq!(audio.rate, 48_000);
                    assert!(!audio.samples.is_empty() && audio.samples.len() <= 48_000);
                    assert!(audio.samples.iter().all(|sample| sample.abs() <= 1.0));
                }
                Err(_) => refused += 1,
            }
        }
        // Les deux issues sont bien essayées, et le tout reste court.
        assert!(
            readable > ROUNDS / 20 && refused > ROUNDS / 20,
            "{readable} lus, {refused} refusés"
        );
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_panic_of_the_decoder_becomes_an_unreadable_file() {
        let result = without_panic(|| panic!("panique voulue par l'essai"));
        assert_eq!(result, Err(VoiceError::Unreadable));
    }
}
