#!/usr/bin/env bash
# Enregistre des sorties réelles de yt-dlp (avec Deno) dans
# src-tauri/src/transcription/ytdlp/fixtures/ : formes de référence des
# lecteurs de lignes et du classement des erreurs de l'app (essais
# `transcription::ytdlp`), et modèle du faux yt-dlp (fake-services).
#   scripts/record-ytdlp-fixtures.sh
# Lancé à la main (réseau : GitHub, YouTube). Les deux outils sont pris aux
# Releases officielles (dernière version, SHA-256 vérifiée) dans
# .tools/ytdlp-record/ ; HOME, TMPDIR, DENO_DIR et le cache de yt-dlp y sont
# aussi (rien n'est écrit dans le vrai HOME). Mêmes options que l'app
# (`ytdlp.rs`, fonctions `common_args`, `info_args`, `download_args`).
# Vidéo : M4gD1WSo5mA (William Fisher, CopyrightX: Lecture 3.2, Berkman
# Klein Center, Creative Commons BY), seule adresse d'une œuvre utilisée.
# Le chemin du dossier de travail est remplacé par @WORK@ dans les fixtures.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$root/src-tauri/src/transcription/ytdlp/fixtures"
base="$root/.tools/ytdlp-record"
tools="$base/tools"
home="$base/home"
work="$base/work"
mkdir -p "$out" "$tools" "$home" "$work"

latest() {
  curl -fsSI "https://github.com/$1/releases/latest" |
    tr -d '\r' | sed -n 's|^[Ll]ocation: .*/releases/tag/||p'
}

# Archive `$2` de la Release `$3` du dépôt `$1`, vérifiée, extraite dans `$4`.
fetch() {
  local repo="$1" asset="$2" tag="$3" dest="$4" sums="$5"
  local url="https://github.com/$repo/releases/download/$tag"
  if [ -f "$dest/.tag" ] && [ "$(cat "$dest/.tag")" = "$tag" ]; then
    return
  fi
  rm -rf "$dest" && mkdir -p "$dest"
  curl -fsSL -o "$dest.zip" "$url/$asset"
  local expected
  expected="$(curl -fsSL "$url/$sums" | tr -d '\r' |
    awk -v name="$asset" '$2 == name { print $1 } /^Hash/ { print tolower($3) }' | head -n 1)"
  local actual
  actual="$(sha256sum "$dest.zip" | cut -d' ' -f1)"
  if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
    echo "somme fausse pour $asset ($actual au lieu de $expected)" >&2
    exit 1
  fi
  unzip -q "$dest.zip" -d "$dest"
  rm -f "$dest.zip"
  echo "$tag" >"$dest/.tag"
}

ytdlp_tag="$(latest yt-dlp/yt-dlp)"
deno_tag="$(latest denoland/deno)"
fetch yt-dlp/yt-dlp yt-dlp_linux.zip "$ytdlp_tag" "$tools/yt-dlp" SHA2-256SUMS
fetch denoland/deno deno-x86_64-unknown-linux-gnu.zip "$deno_tag" "$tools/deno" \
  deno-x86_64-unknown-linux-gnu.zip.sha256sum
ytdlp="$tools/yt-dlp/yt-dlp_linux"
deno="$tools/deno/deno"
mkdir -p "$base/cache" "$base/deno-dir" "$base/no-ffmpeg" "$base/tmp"

run() {
  env -i PATH=/usr/bin:/bin HOME="$home" TMPDIR="$base/tmp" LANG=C.UTF-8 \
    DENO_DIR="$base/deno-dir" DENO_NO_UPDATE_CHECK=1 DENO_NO_PROMPT=1 NO_COLOR=1 \
    PYTHONUTF8=1 PYTHONIOENCODING=utf-8 PYTHONDONTWRITEBYTECODE=1 \
    "$ytdlp" "$@"
}

common=(--ignore-config --no-plugin-dirs --no-update --no-playlist --no-colors
  --newline --encoding utf-8 --cache-dir "$base/cache" --no-cookies
  --no-js-runtimes --js-runtimes "deno:$deno" --ffmpeg-location "$base/no-ffmpeg"
  --socket-timeout 30 --retries 3 --fragment-retries 3 --extractor-retries 1)
selection='ba[acodec=opus][protocol^=http]/ba[acodec=mp4a.40.2][protocol^=http]/ba[acodec^=mp4a][protocol^=http]/ba[protocol^=http]/b[protocol^=http]/ba/b'
progress='download:CHATONLM-PROGRESS %(progress.status)s %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s'

echo "yt-dlp $ytdlp_tag, Deno $deno_tag" >"$out/VERSIONS"

# 1. Métadonnées (premier temps), réduites aux champs lus et aux formats audio.
rm -rf "${work:?}"/*
run "${common[@]}" --flat-playlist -J -- 'https://www.youtube.com/watch?v=M4gD1WSo5mA' \
  >"$work/info.json" 2>"$work/info.stderr"
jq -c '{_type, id, title, channel, uploader, duration, webpage_url, original_url,
  extractor, extractor_key, live_status, is_live, was_live, language, license,
  availability, formats: [.formats[] | select(.vcodec == "none") |
    {format_id, ext, acodec, vcodec, protocol, abr, filesize, url: "@URL@"}]}' \
  "$work/info.json" >"$out/info-cc.json"
sed "s|$work|@WORK@|g" "$work/info.stderr" >"$out/info-cc.stderr"

# 2. Audio seul (deuxième temps) : progression et chemin.
run "${common[@]}" --load-info-json "$work/info.json" -f "$selection" \
  -o "$work/audio.%(ext)s" --progress --progress-delta 0.5 \
  --progress-template "$progress" --print 'after_move:CHATONLM-FILE %(filepath)j' \
  >"$work/download.stdout" 2>"$work/download.stderr"
sed "s|$work|@WORK@|g" "$work/download.stdout" >"$out/download-cc.stdout"
sed "s|$work|@WORK@|g" "$work/download.stderr" >"$out/download-cc.stderr"
ls -l "$work"/audio.* >&2

# 3. Erreurs obtenues pour de vrai (texte de stderr, code de sortie).
record_error() {
  local name="$1"
  shift
  set +e
  run "$@" >/dev/null 2>"$work/$name.stderr"
  local code=$?
  set -e
  { sed "s|$work|@WORK@|g; s|$base|@BASE@|g" "$work/$name.stderr"; echo "exit=$code"; } \
    >"$out/error-$name.stderr"
}
record_error unsupported "${common[@]}" --flat-playlist -J -- 'https://example.com/'
record_error unavailable "${common[@]}" --flat-playlist -J -- 'https://www.youtube.com/watch?v=aaaaaaaaaaa'
record_error network "${common[@]}" --flat-playlist -J -- 'https://chatonlm-fixture.invalid/video'
record_error options "${common[@]}" --no-such-option -J -- 'https://example.com/'
# Sans moteur JavaScript : avertissements de yt-dlp (métadonnées quand même).
record_error no-js --ignore-config --no-update --no-js-runtimes --cache-dir "$base/cache" \
  --flat-playlist -J -- 'https://www.youtube.com/watch?v=M4gD1WSo5mA'

rm -rf "${work:?}"/*
echo "fixtures écrites dans $out"
