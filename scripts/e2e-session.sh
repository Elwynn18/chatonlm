#!/usr/bin/env bash
# Lance une commande dans une session KWin virtuelle imbriquée. Rien n'apparaît
# sur le bureau réel : la session a son propre bus D-Bus, son propre dossier
# d'exécution (XDG_RUNTIME_DIR, dans /tmp) et son propre HOME. Chaque essai part
# d'un état neuf dans .tools/e2e-runs/ (home/ et data/), supprimé à la fin sauf
# avec E2E_KEEP=1 ; les caches (polices, shaders) sont partagés dans
# .tools/e2e-cache. À la fin, tout le groupe de processus de la session est
# arrêté, et l'essai échoue si un processus lui survit.
#
# Usage : scripts/e2e-session.sh <commande> [arguments...]
#         scripts/e2e-session.sh --shot <nom>   (dans la session seulement)
#                                               capture l'écran virtuel dans
#                                               .tools/shots/<nom>.png
# Variables : E2E_WIDTH, E2E_HEIGHT (1360 × 960), E2E_TIMEOUT (240 s), E2E_KEEP.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
shots="$root/.tools/shots"

if [ "${1:-}" = "--shot" ]; then
  # Jamais de capture du bureau réel.
  if [ "${CHATONLM_E2E_SESSION:-}" != 1 ]; then
    echo "Capture refusée hors de la session virtuelle." >&2
    exit 2
  fi
  name="${2:?Usage : $0 --shot <nom>}"
  mkdir -p "$shots"
  rm -f "$shots/$name.png"
  spectacle --background --nonotify --fullscreen --output "$shots/$name.png"
  [ -s "$shots/$name.png" ] || { echo "Capture vide : $name" >&2; exit 1; }
  echo "$shots/$name.png"
  exit 0
fi

[ $# -gt 0 ] || { echo "Usage : $0 <commande> [arguments...]" >&2; exit 2; }

# shellcheck source=../activate.sh
source "$root/activate.sh"

mkdir -p "$root/.tools/e2e-runs" "$root/.tools/e2e-cache" "$root/.tools/logs" \
  "$root/.tools/tmp" "$shots"
run="$(mktemp -d "$root/.tools/e2e-runs/run.XXXXXX")"
home="$run/home"
mkdir -p "$home/.config" "$home/.local/share" "$home/.local/state" "$run/data"
# Les sockets de la session restent hors de /data (ntfs3) et hors du vrai
# /run/user : un client mal configuré échoue au lieu d'atteindre le bureau réel.
runtime="$(mktemp -d /tmp/chatonlm-e2e.XXXXXX)"
chmod 700 "$runtime"

# KWin découpe la commande de session comme un shell simplifié : on lui passe
# un script dont le chemin (qui contient une espace) est entre guillemets.
inner="$run/session.sh"
{
  echo '#!/usr/bin/env bash'
  # Les services D-Bus activés pendant la session (portails, trousseau...)
  # rejoignent ainsi le KWin imbriqué et s'arrêtent avec lui.
  echo 'dbus-update-activation-environment WAYLAND_DISPLAY XDG_RUNTIME_DIR >/dev/null 2>&1 || true'
  printf 'cd %q\n' "$root"
  printf 'exec'
  printf ' %q' "$@"
  echo
} >"$inner"
chmod +x "$inner"

session=""
leftovers() {
  # Processus dont l'environnement porte le HOME de cet essai.
  grep -lzsxF "HOME=$home" /proc/[0-9]*/environ 2>/dev/null | cut -d/ -f3 || true
}
cleanup() {
  if [ -n "$session" ]; then
    kill -TERM -- "-$session" 2>/dev/null || true
    wait "$session" 2>/dev/null || true
  fi
  local pids
  pids="$(leftovers)"
  if [ -n "$pids" ]; then
    # shellcheck disable=SC2086
    kill -KILL $pids 2>/dev/null || true
  fi
  fusermount3 -uz "$runtime/doc" 2>/dev/null || true
  rm -rf "$runtime"
  [ "${E2E_KEEP:-}" = 1 ] || rm -rf "$run"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# setsid : la session forme son propre groupe de processus, que cleanup arrête
# d'un coup (y compris les services D-Bus activés).
setsid timeout -k 10 "${E2E_TIMEOUT:-240}" env -u DISPLAY -u WAYLAND_DISPLAY -u WAYLAND_SOCKET -u OLLAMA_HOST \
  HOME="$home" \
  XDG_CONFIG_HOME="$home/.config" \
  XDG_DATA_HOME="$home/.local/share" \
  XDG_STATE_HOME="$home/.local/state" \
  XDG_CACHE_HOME="$root/.tools/e2e-cache" \
  XDG_RUNTIME_DIR="$runtime" \
  CUDA_CACHE_PATH="$root/.tools/e2e-cache/nvidia/ComputeCache" \
  CHATONLM_DATA_DIR="$run/data" \
  CHATONLM_E2E_SESSION=1 \
  GDK_BACKEND=wayland \
  GSETTINGS_BACKEND=memory \
  TMPDIR="$root/.tools/tmp" \
  dbus-run-session -- kwin_wayland --virtual \
  --width "${E2E_WIDTH:-1360}" --height "${E2E_HEIGHT:-960}" \
  --no-lockscreen --no-global-shortcuts --socket "chatonlm-e2e-$$" \
  --exit-with-session "\"$inner\"" &
session=$!

status=0
wait "$session" || status=$?
session=""

# Une fois la session finie, rien ne doit lui survivre. Les processus de
# WebKit se terminent un peu après l'app : on leur laisse 5 s.
survivors=""
for _ in $(seq 50); do
  survivors="$(leftovers)"
  [ -z "$survivors" ] && break
  sleep 0.1
done
if [ -n "$survivors" ]; then
  echo "Processus restés après la session :" >&2
  for pid in $survivors; do
    echo "  $pid $(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null)" >&2
  done
  status=1
fi
exit "$status"
