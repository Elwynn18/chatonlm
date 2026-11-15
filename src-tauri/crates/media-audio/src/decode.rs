//! Boucle du décodage : ouverture par symphonia (après la reconnaissance des
//! conteneurs refusés), choix de la piste audio, paquets décodés (symphonia,
//! ou opus-pure pour l'Opus), mélange en mono, rééchantillonnage en flux vers
//! 16 kHz et envoi par blocs.

use std::io::{self, Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use symphonia::core::codecs::audio::well_known::{CODEC_ID_AAC, CODEC_ID_OPUS};
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, Track, TrackFlags};
use symphonia::core::io::{MediaSource, MediaSourceStream, ReadOnlySource};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::Packet;
use symphonia::core::units::TimeBase;

use crate::error::{Codec, Container, DecodeError, ErrorCode};
use crate::resample::Resampler;
use crate::{Limits, MediaInfo, Options, Progress, RATE, Sink, Summary, heaac, opus, probe};

/// Paquets illisibles de suite au-delà desquels la lecture s'arrête (le
/// reste du fichier est sans doute abîmé, ou n'est pas ce qu'il prétend).
const MAX_BAD_PACKETS_IN_A_ROW: u32 = 64;

/// Silence au plus mis à la place d'un paquet illisible (une durée de paquet
/// abîmée ne doit pas inventer des minutes de silence).
const MAX_GAP: Duration = Duration::from_secs(1);

/// Marge de la borne de durée : le dernier paquet peut porter jusqu'à 120 ms
/// de remplissage, et une durée annoncée est arrondie.
const DURATION_MARGIN: Duration = Duration::from_secs(1);

/// Son lu plus court que la durée annoncée de plus que cela : fichier
/// incomplet.
const SHORT_OF_ANNOUNCED: Duration = Duration::from_millis(500);

/// Fréquences d'échantillonnage acceptées (au-delà : en-tête abîmé).
const PLAUSIBLE_RATES: std::ops::RangeInclusive<u32> = 1_000..=768_000;

/// Envoie un bloc, ramené entre -1 et 1 : l'interpolation dépasse un peu
/// ces bornes près d'un son écrêté.
fn send(sink: &mut dyn Sink, block: &mut [f32]) -> io::Result<()> {
    for sample in block.iter_mut() {
        *sample = sample.clamp(-1.0, 1.0);
    }
    sink.samples(block)
}

/// Secondes décodées à la fréquence du rééchantillonneur en cours.
fn resampled_seconds(resampler: &Option<(u32, Resampler)>, decoded: u64) -> f64 {
    resampler
        .as_ref()
        .map_or(0.0, |(rate, _)| decoded as f64 / f64::from(*rate))
}

/// Une panique d'un décodeur (opus-pure répond ainsi à un contrat interne
/// rompu ; symphonia a quelques `unreachable!`) devient un fichier abîmé au
/// lieu d'emporter le fil qui décode.
pub(crate) fn guarded<T>(work: impl FnOnce() -> Result<T, DecodeError>) -> Result<T, DecodeError> {
    catch_unwind(AssertUnwindSafe(work))
        .unwrap_or_else(|_| Err(DecodeError::new(ErrorCode::Damaged, "decoder panicked")))
}

/// Fichier ouvert, piste audio choisie, décodeur prêt.
pub(crate) struct Opened {
    reader: Box<dyn FormatReader>,
    track: u32,
    /// Base de temps de la piste (durée des paquets).
    time_base: Option<TimeBase>,
    decoder: TrackDecoder,
    pub(crate) info: MediaInfo,
}

impl Opened {
    /// Refus d'une durée annoncée trop longue, avant tout décodage.
    pub(crate) fn check_duration(&self, limits: &Limits) -> Result<(), DecodeError> {
        match self.info.duration {
            Some(duration) if duration > limits.max_duration + DURATION_MARGIN => {
                Err(self.too_long(duration, limits))
            }
            _ => Ok(()),
        }
    }

    fn too_long(&self, duration: Duration, limits: &Limits) -> DecodeError {
        DecodeError::new(
            ErrorCode::TooLong,
            format!(
                "{:.0} s, more than {} s",
                duration.as_secs_f64(),
                limits.max_duration.as_secs()
            ),
        )
        .in_container(self.info.container)
        .with_codec(self.info.codec)
    }
}

enum TrackDecoder {
    Symphonia(Box<dyn AudioDecoder>),
    Opus(Box<opus::Decoder>),
}

/// Issue du décodage d'un paquet.
enum Decoded {
    /// Son ajouté au tampon mono, à cette fréquence.
    Sound(u32),
    /// Paquet illisible, sauté.
    Bad,
    /// Le flux change en cours de route (OGG enchaîné) ou n'est pas pris en
    /// charge : arrêt.
    Stop(&'static str),
}

impl TrackDecoder {
    fn decode(
        &mut self,
        packet: &Packet,
        mono: &mut Vec<f32>,
        interleaved: &mut Vec<f32>,
    ) -> Decoded {
        match self {
            Self::Opus(decoder) => match decoder.decode(&packet.data, mono) {
                Ok(()) => Decoded::Sound(opus::RATE),
                Err(()) => Decoded::Bad,
            },
            Self::Symphonia(decoder) => match decoder.decode(packet) {
                Ok(audio) => {
                    let rate = audio.spec().rate();
                    let channels = audio.spec().channels().count();
                    if rate == 0 || channels == 0 {
                        return Decoded::Bad;
                    }
                    interleaved.resize(audio.samples_interleaved(), 0.0);
                    audio.copy_to_slice_interleaved(&mut interleaved[..]);
                    for frame in interleaved.chunks_exact(channels) {
                        mono.push(frame.iter().sum::<f32>() / channels as f32);
                    }
                    Decoded::Sound(rate)
                }
                Err(Error::DecodeError(_) | Error::IoError(_) | Error::LimitError(_)) => {
                    Decoded::Bad
                }
                Err(Error::ResetRequired) => Decoded::Stop("stream changed"),
                Err(_) => Decoded::Stop("unsupported stream"),
            },
        }
    }
}

/// Source à lire, rouverte au besoin depuis son début.
pub(crate) type Reopen<'a> = dyn FnMut() -> io::Result<Box<dyn MediaSource>> + 'a;

/// Reconnaît le conteneur, choisit la piste audio et prépare son décodeur.
///
/// Un MKV/WebM tronqué (téléchargement interrompu) annonce un segment plus
/// long que le fichier, ce que le lecteur de symphonia refuse quand il
/// connaît la longueur : un échec de la reconnaissance est donc retenté une
/// fois en lecture vers l'avant seulement, qui lit jusqu'à la coupure. Si
/// cela échoue aussi, la première erreur est rendue.
pub(crate) fn open(
    reopen: &mut Reopen<'_>,
    extension: Option<&str>,
) -> Result<Opened, DecodeError> {
    let cannot_read = |error: io::Error| DecodeError::new(ErrorCode::CannotOpen, error.to_string());
    let source = reopen().map_err(cannot_read)?;
    let seekable = source.is_seekable();
    match open_source(source, extension) {
        Err(first)
            if seekable
                && matches!(
                    first.code,
                    ErrorCode::UnsupportedContainer | ErrorCode::Damaged
                )
                && matches!(first.container, None | Some(Container::Unknown)) =>
        {
            let source = reopen().map_err(cannot_read)?;
            open_source(Box::new(ReadOnlySource::new(source)), extension).map_err(|_| first)
        }
        result => result,
    }
}

fn open_source(
    mut source: Box<dyn MediaSource>,
    extension: Option<&str>,
) -> Result<Opened, DecodeError> {
    let damaged = |detail: String| DecodeError::new(ErrorCode::Damaged, detail);
    let cannot_read = |error: io::Error| DecodeError::new(ErrorCode::CannotOpen, error.to_string());
    let mut head = Vec::with_capacity(probe::HEAD);
    (&mut source)
        .take(probe::HEAD as u64)
        .read_to_end(&mut head)
        .map_err(cannot_read)?;
    // Une source qui ne revient pas en arrière (flux d'un autre processus)
    // est relue par le début déjà lu, puis la suite.
    let source: Box<dyn MediaSource> = if source.is_seekable() {
        source.seek(SeekFrom::Start(0)).map_err(cannot_read)?;
        source
    } else {
        Box::new(ReadOnlySource::new(
            io::Cursor::new(head.clone()).chain(source),
        ))
    };
    if head.is_empty() {
        return Err(damaged("empty file".into()));
    }
    if let Some(container) = probe::refused_container(&head) {
        return Err(
            DecodeError::new(ErrorCode::UnsupportedContainer, "refused container")
                .in_container(container),
        );
    }

    let stream = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = extension {
        hint.with_extension(extension);
    }
    let reader = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|error| match error {
            Error::Unsupported(_) => {
                DecodeError::new(ErrorCode::UnsupportedContainer, error.to_string())
                    .in_container(Container::Unknown)
            }
            error => damaged(error.to_string()),
        })?;
    let container = probe::container(reader.format_info().short_name);

    // Pistes audio, celle marquée par défaut d'abord : la première dont le
    // codec se décode est prise (un film peut avoir l'AC-3 par défaut et une
    // piste AAC à côté).
    let mut tracks: Vec<&Track> = reader
        .tracks()
        .iter()
        .filter(|track| audio_params(track).is_some())
        .collect();
    tracks.sort_by_key(|track| !track.flags.contains(TrackFlags::DEFAULT));
    let mut refused = None;
    let mut chosen = None;
    for track in tracks {
        match prepare(track) {
            Ok(prepared) => {
                chosen = Some(((track.id, track.time_base), prepared));
                break;
            }
            Err(error) => {
                refused.get_or_insert(error);
            }
        }
    }
    let Some(((track, time_base), (decoder, mut info))) = chosen else {
        let error =
            refused.unwrap_or_else(|| DecodeError::new(ErrorCode::NoAudioTrack, "no audio track"));
        return Err(error.in_container(container));
    };
    info.container = container;
    if info.duration.is_none() {
        let media = reader.media_info();
        info.duration = media
            .time_base
            .zip(media.duration)
            .and_then(|(base, duration)| base.calc_duration(duration))
            .and_then(|time| Duration::try_from_secs_f64(time.as_secs_f64()).ok())
            .filter(|duration| !duration.is_zero());
    }
    Ok(Opened {
        reader,
        track,
        time_base,
        decoder,
        info,
    })
}

fn audio_params(track: &Track) -> Option<&AudioCodecParameters> {
    track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
}

/// Décodeur et description d'une piste audio ; erreur : codec refusé, ou
/// paramètres illisibles.
fn prepare(track: &Track) -> Result<(TrackDecoder, MediaInfo), DecodeError> {
    let params = audio_params(track)
        .ok_or_else(|| DecodeError::new(ErrorCode::NoAudioTrack, "no audio parameters"))?;
    let mut codec = probe::codec(params.codec, params.profile);
    let mut info = MediaInfo {
        container: Container::Unknown,
        codec,
        duration: track_duration(track, params),
        sample_rate: params.sample_rate,
        channels: params
            .channels
            .as_ref()
            .and_then(|channels| u16::try_from(channels.count()).ok()),
    };
    let unsupported = |codec: Codec, detail: &str| {
        DecodeError::new(ErrorCode::UnsupportedCodec, detail).with_codec(codec)
    };

    if params.codec == CODEC_ID_OPUS {
        let head = match params.extra_data.as_deref() {
            Some(data) => opus::Head::parse(data)
                .map_err(|detail| DecodeError::new(ErrorCode::Damaged, detail).with_codec(codec))?
                .ok_or_else(|| unsupported(codec, "multichannel opus"))?,
            // Sans en-tête (rare) : les canaux et l'amorce de la piste.
            None => opus::Head {
                channels: info.channels.map_or(1, usize::from).clamp(1, 2),
                pre_skip: track.delay.map_or(0, |delay| delay as usize),
            },
        };
        info.channels = Some(head.channels as u16);
        info.sample_rate = Some(opus::RATE);
        let decoder = opus::Decoder::new(head)
            .map_err(|detail| DecodeError::new(ErrorCode::Damaged, detail).with_codec(codec))?;
        return Ok((TrackDecoder::Opus(Box::new(decoder)), info));
    }

    let mut params = params.clone();
    if params.codec == CODEC_ID_AAC
        && let Some(core) = params.extra_data.as_deref().and_then(heaac::core)
    {
        params.extra_data = Some(core.asc.into_boxed_slice());
        params.sample_rate = Some(core.rate);
        codec = core.codec;
        info.codec = codec;
        info.sample_rate = Some(core.rate);
    }
    let decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|error| match error {
            Error::Unsupported(detail) => unsupported(codec, detail),
            error => DecodeError::new(ErrorCode::Damaged, error.to_string()).with_codec(codec),
        })?;
    Ok((TrackDecoder::Symphonia(decoder), info))
}

/// Échantillons de silence, à `rate`, pour un paquet illisible de durée
/// `dur` (unités de `time_base`) : [`MAX_GAP`] au plus ; 0 sans base de
/// temps ni fréquence.
pub(crate) fn gap_samples(
    time_base: Option<TimeBase>,
    dur: symphonia::core::units::Duration,
    rate: Option<u32>,
) -> usize {
    let (Some(base), Some(rate)) = (
        time_base,
        rate.filter(|rate| PLAUSIBLE_RATES.contains(rate)),
    ) else {
        return 0;
    };
    let seconds = base
        .calc_duration(dur)
        .map_or(0.0, |time| time.as_secs_f64())
        .clamp(0.0, MAX_GAP.as_secs_f64());
    (seconds * f64::from(rate)).round() as usize
}

/// Durée annoncée par la piste.
fn track_duration(track: &Track, params: &AudioCodecParameters) -> Option<Duration> {
    let from_base = track
        .time_base
        .zip(track.duration)
        .and_then(|(base, duration)| base.calc_duration(duration))
        .map(|time| time.as_secs_f64());
    let from_frames = track
        .num_frames
        .zip(params.sample_rate.filter(|&rate| rate > 0))
        .map(|(frames, rate)| frames as f64 / f64::from(rate));
    from_base
        .or(from_frames)
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        // Un MP4 fragmenté annonce souvent 0 (durée inconnue à l'écriture).
        .filter(|duration| !duration.is_zero())
}

/// Décode tout le fichier vers `sink`.
pub(crate) fn run(
    reopen: &mut Reopen<'_>,
    extension: Option<&str>,
    options: &Options,
    cancel: &AtomicBool,
    sink: &mut dyn Sink,
) -> Result<Summary, DecodeError> {
    let mut opened = open(reopen, extension)?;
    opened.check_duration(&options.limits)?;
    let info = opened.info;
    let in_file = |error: DecodeError| error.in_container(info.container).with_codec(info.codec);
    let output = |error: io::Error| in_file(DecodeError::new(ErrorCode::Output, error.to_string()));
    sink.start(&info).map_err(output)?;

    let threads = options.thread_count();
    let longest = options.limits.max_duration + DURATION_MARGIN;
    let mut resampler: Option<(u32, Resampler)> = None;
    let (mut mono, mut interleaved, mut block) = (Vec::new(), Vec::new(), Vec::new());
    // Secondes décodées des fréquences précédentes (un flux peut en changer).
    let mut decoded_before = 0.0f64;
    let mut decoded_here = 0u64;
    let mut sent = 0u64;
    let (mut skipped, mut bad_in_a_row) = (0u64, 0u32);
    let mut complete = false;
    // Un paquet au moins a donné du son (un fichier dont rien ne se lit
    // reste abîmé, pas du silence).
    let mut heard = false;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(in_file(DecodeError::new(ErrorCode::Cancelled, "cancelled")));
        }
        let packet = match opened.reader.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => {
                complete = true;
                break;
            }
            // Tronqué, abîmé, ou flux qui change (OGG enchaîné) : ce qui
            // précède est gardé.
            Err(_) if sent > 0 || decoded_here > 0 || decoded_before > 0.0 => break,
            Err(error) => {
                return Err(in_file(DecodeError::new(
                    ErrorCode::Damaged,
                    error.to_string(),
                )));
            }
        };
        if packet.track_id != opened.track {
            continue;
        }
        mono.clear();
        let (rate, silent) = match opened.decoder.decode(&packet, &mut mono, &mut interleaved) {
            Decoded::Sound(rate) => {
                heard = true;
                (rate, false)
            }
            Decoded::Bad => {
                skipped += 1;
                bad_in_a_row += 1;
                if bad_in_a_row > MAX_BAD_PACKETS_IN_A_ROW {
                    break;
                }
                // Silence de la durée du paquet sauté, à la fréquence en
                // cours (sinon celle annoncée) : sans lui, tout ce qui suit
                // serait décalé d'autant (horodatages, sous-titres). Avant le
                // premier son, sauté comme avant.
                if !heard {
                    continue;
                }
                let rate = resampler
                    .as_ref()
                    .map(|(rate, _)| *rate)
                    .or(info.sample_rate);
                let gap = gap_samples(opened.time_base, packet.dur, rate);
                match rate {
                    Some(rate) if gap > 0 => {
                        mono.resize(gap, 0.0);
                        (rate, true)
                    }
                    _ => continue,
                }
            }
            Decoded::Stop(detail) => {
                if sent == 0 && decoded_here == 0 && decoded_before == 0.0 {
                    return Err(in_file(DecodeError::new(
                        ErrorCode::UnsupportedCodec,
                        detail,
                    )));
                }
                break;
            }
        };
        // Une fréquence absurde (en-tête abîmé) ferait produire des milliers
        // d'échantillons par échantillon lu.
        if !PLAUSIBLE_RATES.contains(&rate) {
            if sent == 0 && decoded_here == 0 && decoded_before == 0.0 {
                return Err(in_file(DecodeError::new(
                    ErrorCode::Damaged,
                    format!("implausible sample rate {rate}"),
                )));
            }
            break;
        }
        if !silent {
            bad_in_a_row = 0;
        }
        // Borne de durée, avant d'envoyer quoi que ce soit de ce paquet.
        let decoded = if resampler
            .as_ref()
            .is_some_and(|(current, _)| *current == rate)
        {
            decoded_before + (decoded_here + mono.len() as u64) as f64 / f64::from(rate)
        } else {
            decoded_before
                + resampled_seconds(&resampler, decoded_here)
                + mono.len() as f64 / f64::from(rate)
        };
        let decoded = Duration::try_from_secs_f64(decoded).unwrap_or(Duration::MAX);
        if decoded > longest {
            return Err(opened.too_long(decoded, &options.limits));
        }
        // Les moteurs attendent des valeurs entre -1 et 1 ; les décodeurs
        // dépassent un peu sur un son fort, et un nombre invalide devient un
        // silence.
        for sample in &mut mono {
            *sample = if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }

        // Nouvelle fréquence : la précédente est menée à son terme.
        if resampler
            .as_ref()
            .is_none_or(|(current, _)| *current != rate)
        {
            if let Some((previous, finished)) = resampler.take() {
                finished.finish(&mut block);
                decoded_before += decoded_here as f64 / f64::from(previous);
                decoded_here = 0;
            }
            resampler = Some((rate, Resampler::new(rate, RATE, threads)));
        }
        if let Some((_, current)) = &mut resampler {
            current.push(&mono, &mut block);
        }
        decoded_here += mono.len() as u64;
        if !block.is_empty() {
            send(sink, &mut block).map_err(output)?;
            sent += block.len() as u64;
            block.clear();
            sink.progress(Progress {
                decoded,
                total: info.duration,
            });
        }
    }

    if let Some((rate, finished)) = resampler.take() {
        finished.finish(&mut block);
        decoded_before += decoded_here as f64 / f64::from(rate);
    }
    if !block.is_empty() {
        send(sink, &mut block).map_err(output)?;
        sent += block.len() as u64;
    }
    if sent == 0 {
        return Err(in_file(DecodeError::new(
            ErrorCode::Damaged,
            "no sound decoded",
        )));
    }
    let decoded = Duration::try_from_secs_f64(decoded_before).unwrap_or(Duration::MAX);
    sink.progress(Progress {
        decoded,
        total: info.duration,
    });
    // Durée annoncée : c'est elle qui juge. Un MP3 ou un WAV tronqué finit
    // proprement bien avant ; un WebM lu vers l'avant finit par une erreur
    // de lecture après tout son son (index de fin hors d'atteinte).
    if let Some(total) = info.duration {
        complete = decoded + SHORT_OF_ANNOUNCED >= total;
    }
    Ok(Summary {
        info,
        samples: sent,
        complete,
        skipped_packets: skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lecture vers l'avant seulement (sortie d'un autre processus).
    fn forward_only(name: &str) -> impl FnMut() -> io::Result<Box<dyn MediaSource>> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(name);
        move || {
            let file = std::fs::File::open(&path)?;
            Ok(Box::new(ReadOnlySource::new(file)))
        }
    }

    fn limited(seconds: u64) -> Options {
        Options {
            limits: Limits {
                max_duration: Duration::from_secs(seconds),
                ..Limits::default()
            },
            threads: 1,
        }
    }

    #[test]
    fn a_forward_only_source_is_decoded_whole() {
        let mut samples = Vec::new();
        let none = AtomicBool::new(false);
        for name in ["jfk.opus", "jfk-yt251.webm", "jfk-stereo.mp3"] {
            samples.clear();
            let summary = run(
                &mut forward_only(name),
                None,
                &limited(60),
                &none,
                &mut samples,
            )
            .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(summary.complete, "{name}");
            assert!(
                samples.len().abs_diff(64_000) < 1_000,
                "{name}: {}",
                samples.len()
            );
        }
    }

    #[test]
    fn the_silence_of_a_skipped_packet_is_bounded() {
        use symphonia::core::units::Duration as Units;
        let base = TimeBase::try_new(1, 44_100);
        // 1 024 trames à 44,1 kHz, rendues à 44,1 kHz.
        assert_eq!(gap_samples(base, Units::new(1_024), Some(44_100)), 1_024);
        // Durée abîmée : une seconde au plus.
        assert_eq!(
            gap_samples(base, Units::new(u64::from(u32::MAX)), Some(16_000)),
            16_000
        );
        assert_eq!(gap_samples(None, Units::new(1_024), Some(44_100)), 0);
        assert_eq!(gap_samples(base, Units::new(1_024), None), 0);
        assert_eq!(gap_samples(base, Units::new(1_024), Some(0)), 0);
    }

    #[test]
    fn a_stream_without_announced_duration_stops_at_the_limit() {
        // Vers l'avant seulement, l'OGG ne dit pas sa durée (elle est dans sa
        // dernière page) : la borne joue sur le son lu, avant d'envoyer
        // au-delà.
        let none = AtomicBool::new(false);
        let mut opened = open(&mut forward_only("jfk.opus"), None).unwrap();
        assert_eq!(opened.info.duration, None);
        assert!(opened.reader.next_packet().is_ok());
        let mut samples = Vec::new();
        let error = run(
            &mut forward_only("jfk.opus"),
            None,
            &limited(2),
            &none,
            &mut samples,
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::TooLong);
        assert!(samples.len() <= 3 * RATE as usize, "{}", samples.len());
        assert!(!samples.is_empty());
    }
}
