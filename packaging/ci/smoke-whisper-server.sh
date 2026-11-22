#!/usr/bin/env bash
# Essai de fumée de la CI (.github/workflows/packages.yml), jamais sur un
# bureau réel : le moteur de transcription (whisper-server) d'un paquet
# installé ou extrait démarre sur cette machine neuve. Contrôles :
# - fichiers livrés (whisper-server exécutable, VERSION, SHA256SUMS vérifié,
#   aucun fallback/ à lui) ; chargeur Vulkan de secours pris, comme le fait
#   l'app, dans le dossier de llama-server (second argument, par défaut le
#   dossier voisin llama-server/fallback) ;
# - « whisper-server --help » rend le code 0 après avoir chargé un moteur du
#   processeur, avec le chargeur du système s'il y en a un, et avec celui de
#   secours (machine sans chargeur : ce que fera l'app) ;
# - sur un modèle absent, il s'arrête aussitôt avec un code non nul et sa
#   ligne « failed to initialize whisper context » ;
# - sur le petit modèle d'essai de whisper.cpp (for-tests-ggml-tiny.bin,
#   575 ko, MIT, empreinte épinglée ci-dessous), sur le processeur (-ng), avec
#   les arguments de l'app (src-tauri/src/dictation/whisper_server.rs,
#   `server_args`) : il écoute, une transcription d'une seconde de silence
#   rend un JSON avec « text » ; une requête d'une page web (Origin), pour un
#   autre hôte, ou vers /load est refusée (403, correctif
#   packaging/whispercpp/0002-refuse-browser-requests.patch) ; un SIGTERM
#   l'arrête, et aucun processus ne reste.
#   packaging/ci/smoke-whisper-server.sh /usr/lib/chatonlm/whisper-server
#   packaging/ci/smoke-whisper-server.sh squashfs-root/usr/lib/chatonlm/whisper-server
set -euo pipefail

# Modèle d'essai de whisper.cpp (poids réduits : il ne transcrit rien de
# juste, mais se charge et répond), au commit de packaging/whispercpp.env.
MODEL_URL=https://raw.githubusercontent.com/ggml-org/whisper.cpp/d1be6fde11ac6e0407606b4e42fe72d34add8037/models/for-tests-ggml-tiny.bin
MODEL_SHA256=c486fb9f14a28b1c1dc252741a431646cc573450c900b9d9c406e10294aa01e6
PORT=18447

dir="$(cd "${1:?Usage : $0 <dossier whisper-server> [<dossier llama-server>]}" && pwd)"
llama_dir="${2:-$dir/../llama-server}"
fallback="$(cd "$llama_dir/fallback" && pwd)"
bin="$dir/whisper-server"
fail() {
  echo "smoke-whisper-server : $*" >&2
  exit 1
}
[ -x "$bin" ] || fail "$bin absent ou non exécutable"
[ -f "$dir/VERSION" ] || fail "VERSION absent"
[ ! -e "$dir/fallback" ] || fail "fallback/ livré avec whisper-server (attendu : celui de llama-server)"
[ -f "$fallback/libvulkan.so.1" ] || fail "chargeur Vulkan de secours de llama-server absent"
(cd "$dir" && sha256sum --check --quiet --strict SHA256SUMS) || fail "fichiers différents de SHA256SUMS"
grep -Eq '^v[0-9.]+\+chatonlm\.[0-9a-f]{12}$' "$dir/VERSION" || fail "VERSION inattendue : $(cat "$dir/VERSION")"

check_help() {
  local label="$1"
  shift
  local output status=0
  output="$("$@" --help 2>&1)" || status=$?
  [ "$status" = 0 ] || fail "$label : --help, code $status ($output)"
  grep -q 'load_backend: loaded CPU backend' <<<"$output" || fail "$label : aucun moteur du processeur chargé ($output)"
  echo "$label : $(grep -o 'loaded [A-Za-z]* backend from .*' <<<"$output" | sed 's|from .*/|: |' | tr '\n' ' ')"
}

work="$(mktemp -d)"
cleanup() {
  pkill -KILL -f "^$bin " 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT
# Dossier courant et HOME à l'essai : ggml cherche aussi ses moteurs de
# calcul dans le dossier courant.
cd "$work"
export HOME="$work/home"
mkdir -p "$HOME"

if ldconfig -p 2>/dev/null | grep 'libvulkan\.so\.1 ' >/dev/null; then
  check_help "chargeur du système" "$bin"
else
  echo "Pas de chargeur Vulkan sur cette machine."
fi
check_help "chargeur de secours" env LD_LIBRARY_PATH="$fallback" "$bin"

# Modèle absent : refusé, code non nul.
status=0
LD_LIBRARY_PATH="$fallback" timeout 60 "$bin" --host 127.0.0.1 --port "$PORT" \
  -m "$work/absent.bin" >"$work/bad.log" 2>&1 || status=$?
if [ "$status" = 0 ] || [ "$status" = 124 ]; then
  cat "$work/bad.log" >&2
  fail "code de sortie $status sur un modèle absent"
fi
grep -q 'failed to initialize whisper context' "$work/bad.log" || { cat "$work/bad.log" >&2; fail "erreur du modèle absent absente du journal"; }
echo "whisper-server : modèle absent refusé (code $status)."

# Modèle d'essai, sur le processeur.
curl -fsSL --retry 3 -o "$work/model.bin" "$MODEL_URL"
echo "$MODEL_SHA256  $work/model.bin" | sha256sum --check --quiet - || fail "empreinte inattendue du modèle d'essai"
# Une seconde de silence : WAV PCM 16 bits, mono, 16 kHz.
{
  printf 'RIFF\x24\x7d\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00'
  printf '\x80\x3e\x00\x00\x00\x7d\x00\x00\x02\x00\x10\x00data\x00\x7d\x00\x00'
  head -c 32000 /dev/zero
} >"$work/silence.wav"
LD_LIBRARY_PATH="$fallback" timeout 120 "$bin" --host 127.0.0.1 --port "$PORT" \
  --model "$work/model.bin" --threads 4 --beam-size 3 --suppress-nst \
  --no-speech-thold 0.2 --language auto -ng >"$work/server.log" 2>&1 &
server=$!
ready=false
for _ in $(seq 1 120); do
  if [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/")" != 000 ]; then
    ready=true
    break
  fi
  kill -0 "$server" 2>/dev/null || break
  sleep 0.5
done
$ready || { cat "$work/server.log" >&2; fail "le serveur n'écoute pas"; }
answer="$(curl -fsS -X POST "http://127.0.0.1:$PORT/inference" \
  -F file=@"$work/silence.wav" -F response_format=json -F temperature=0)" ||
  { cat "$work/server.log" >&2; fail "transcription en échec"; }
grep -q '"text"' <<<"$answer" || { cat "$work/server.log" >&2; fail "réponse sans texte : $answer"; }
echo "whisper-server : transcription sur le processeur : $answer"

# Requêtes d'ailleurs que l'app : refusées.
refused() {
  local label="$1"
  shift
  local code
  code="$(curl -s -o /dev/null -w '%{http_code}' "$@")"
  [ "$code" = 403 ] || { cat "$work/server.log" >&2; fail "$label : code $code (attendu : 403)"; }
}
refused "page web (Origin)" -X POST "http://127.0.0.1:$PORT/inference" \
  -H 'Origin: http://example.com' -F file=@"$work/silence.wav"
refused "autre hôte" -H "Host: example.com:$PORT" "http://127.0.0.1:$PORT/health"
refused "/load" -X POST "http://127.0.0.1:$PORT/load" -F model=/etc/hostname
kill -0 "$server" 2>/dev/null || fail "le serveur s'est arrêté après /load"
echo "whisper-server : requêtes d'une page web, d'un autre hôte et vers /load refusées (403)."

kill -TERM "$server"
status=0
wait "$server" || status=$?
[ "$status" = 0 ] || { tail -20 "$work/server.log" >&2; fail "code de sortie $status après SIGTERM (attendu : 0)"; }
if pgrep -f "^$bin " >/dev/null; then
  fail "un processus whisper-server reste"
fi
echo "whisper-server : arrêté par SIGTERM (code 0), aucun processus restant."
