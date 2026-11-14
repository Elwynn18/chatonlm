#!/usr/bin/env bash
# Fixtures du décodage des fichiers audio et vidéo (crate media-audio) :
# 4 s de la parole de jfk (discours d'investiture de J. F. Kennedy, 1961,
# domaine public ; échantillon livré avec whisper.cpp et transcribe-rs),
# codées dans chaque format que l'outil Transcription doit lire, plus
# quelques formats qu'il doit refuser proprement.
#
# Lancé à la main seulement (ffmpeg et node de la machine), puis les fichiers
# sont commités : les essais n'appellent jamais ffmpeg. Faites avec
# ffmpeg n9.0.2 le 14 nov. 2026.
#
# Usage : scripts/make-audio-fixtures.sh [jfk.wav]
#   sans argument : .tools/speech-samples/jfk.wav (16 kHz, mono).
#
# HE-AAC : aucun encodeur libre de la machine ne produit de SBR. Les fichiers
# « heaac » sont donc un AAC-LC à 22 050 Hz dont l'AudioSpecificConfig est
# réécrite en HE-AAC (objet 5 ou 29, signalisation explicite, comme les
# fichiers 139 de YouTube) : ils vérifient la lecture et la réécriture de
# l'ASC. Le saut des données SBR d'un vrai HE-AAC est vérifié par l'essai
# ignoré `real_youtube_formats` (fichiers de .tools/research-transcription).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_wav="${1:-$root/.tools/speech-samples/jfk.wav}"
out="$root/src-tauri/crates/media-audio/tests/data"
mkdir -p "$out"
work="$(mktemp -d "$root/.tools/tmp/audio-fixtures.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# Sorties reproductibles : ni date ni version de l'encodeur dans les fichiers.
ff() {
  ffmpeg -hide_banner -loglevel error -y "$@"
}
exact=(-map_metadata -1 -fflags +bitexact -flags:a +bitexact)
exact_video=(-flags:v +bitexact)

# Source : 4 s, 16 kHz, mono, s16 (la référence des essais est le FLAC, sans
# perte). Stéréo : droite = moitié de la gauche (le mélange mono la garde).
ff -i "$source_wav" -t 4 -ac 1 -ar 16000 -c:a pcm_s16le "$work/mono.wav"
ff -i "$work/mono.wav" -af "pan=stereo|c0=c0|c1=0.5*c0" -c:a pcm_s16le "$work/stereo.wav"
# Image fixe de 16 × 16 pour les pistes vidéo (deux images par seconde).
video=(-f lavfi -i "color=c=orange:s=16x16:r=2:d=4")

# Référence, et lecture du FLAC.
ff -i "$work/mono.wav" "${exact[@]}" -c:a flac -compression_level 8 "$out/jfk.flac"
# WAV 8 kHz en entiers 8 bits non signés (conversion et montée en fréquence).
ff -i "$work/mono.wav" "${exact[@]}" -ar 8000 -c:a pcm_u8 "$out/jfk-8k-u8.wav"
# MP3 stéréo à 44,1 kHz.
ff -i "$work/stereo.wav" "${exact[@]}" -ar 44100 -c:a libmp3lame -b:a 48k "$out/jfk-stereo.mp3"
# OGG/Vorbis mono à 22 050 Hz.
ff -i "$work/mono.wav" "${exact[@]}" -ar 22050 -c:a libvorbis -q:a 0 "$out/jfk.ogg"
# OGG/Opus mono et stéréo.
ff -i "$work/mono.wav" "${exact[@]}" -ar 48000 -c:a libopus -b:a 24k "$out/jfk.opus"
ff -i "$work/stereo.wav" "${exact[@]}" -ar 48000 -c:a libopus -b:a 32k "$out/jfk-stereo.opus"
# WebM/Opus audio seul, comme les formats 249 et 251 de YouTube.
ff -i "$work/stereo.wav" "${exact[@]}" -ar 48000 -c:a libopus -b:a 32k -f webm "$out/jfk-yt251.webm"
# WebM vidéo (VP8) + Opus.
ff "${video[@]}" -i "$work/mono.wav" "${exact[@]}" "${exact_video[@]}" -c:v libvpx -b:v 20k \
  -c:a libopus -b:a 24k -ar 48000 -shortest "$out/jfk-video.webm"
# M4A AAC-LC mono à 44,1 kHz.
ff -i "$work/mono.wav" "${exact[@]}" -ar 44100 -c:a aac -b:a 32k "$out/jfk.m4a"
# MP4 fragmenté (DASH) AAC-LC stéréo à 44,1 kHz, comme le format 140 de
# YouTube.
frag=(-movflags +frag_keyframe+empty_moov+default_base_moof+skip_trailer -frag_duration 1000000)
ff -i "$work/stereo.wav" "${exact[@]}" -ar 44100 -c:a aac -b:a 48k "${frag[@]}" "$out/jfk-yt140.m4a"
# HE-AAC (voir l'en-tête) : cœur stéréo à 22 050 Hz en MP4 fragmenté comme le
# format 139 de YouTube ; HE-AACv2 (objet 29, cœur mono) en MP4 simple.
ff -i "$work/stereo.wav" "${exact[@]}" -ar 22050 -c:a aac -b:a 32k "${frag[@]}" "$work/core-stereo.m4a"
ff -i "$work/mono.wav" "${exact[@]}" -ar 22050 -c:a aac -b:a 24k "$work/core-mono.m4a"
node "$root/scripts/make-audio-fixtures-heaac.mjs" "$work/core-stereo.m4a" "$out/jfk-yt139.m4a" 5
node "$root/scripts/make-audio-fixtures-heaac.mjs" "$work/core-mono.m4a" "$out/jfk-heaac-v2.m4a" 29
# MP4, MOV et MKV avec une piste vidéo (MPEG-4 part 2) et de l'AAC.
ff "${video[@]}" -i "$work/mono.wav" "${exact[@]}" "${exact_video[@]}" -c:v mpeg4 -q:v 31 \
  -c:a aac -b:a 32k -ar 48000 -shortest "$out/jfk-video.mp4"
ff "${video[@]}" -i "$work/mono.wav" "${exact[@]}" "${exact_video[@]}" -c:v mpeg4 -q:v 31 \
  -c:a aac -b:a 32k -ar 32000 -shortest "$out/jfk-video.mov"
ff "${video[@]}" -i "$work/stereo.wav" "${exact[@]}" "${exact_video[@]}" -c:v mpeg4 -q:v 31 \
  -c:a aac -b:a 32k -ar 44100 -shortest "$out/jfk-video.mkv"
# MKV de film, 2 s : AC-3 par défaut (refusée), jfk en AAC, puis une piste MP3
# d'un son de 440 Hz : c'est la première piste décodable (l'AAC) qui doit
# être prise.
ff -i "$work/stereo.wav" -i "$work/mono.wav" -f lavfi -i "sine=f=440:r=44100:d=2" -t 2 "${exact[@]}" \
  -map 0:a -map 1:a -map 2:a -c:a:0 ac3 -b:a:0 96k -ar:a:0 48000 -c:a:1 aac -b:a:1 32k -ar:a:1 44100 \
  -c:a:2 libmp3lame -b:a:2 32k -disposition:a:0 default -disposition:a:1 0 -disposition:a:2 0 \
  "$out/jfk-ac3-then-aac.mkv"

# Refus : 0,5 s chacun.
short=(-t 0.5)
# MKV avec de l'AC-3 (films) : codec non pris en charge.
ff -i "$work/stereo.wav" "${short[@]}" "${exact[@]}" -ar 48000 -c:a ac3 -b:a 96k "$out/refused-ac3.mkv"
# AVI (MP3 dedans) et MPEG-TS (AAC dedans) : conteneurs non pris en charge.
ff "${video[@]}" -i "$work/mono.wav" "${short[@]}" "${exact[@]}" "${exact_video[@]}" -c:v mpeg4 -q:v 31 \
  -c:a libmp3lame -b:a 32k -ar 22050 "$out/refused.avi"
ff -i "$work/mono.wav" "${short[@]}" "${exact[@]}" -c:a aac -b:a 32k -ar 44100 -f mpegts "$out/refused.ts"
# MKV sans piste audio.
ff "${video[@]}" "${short[@]}" "${exact_video[@]}" -c:v mpeg4 -q:v 31 "$out/refused-video-only.mkv"

ls -l "$out"
