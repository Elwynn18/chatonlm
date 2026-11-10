#!/usr/bin/env bash
# Essai de fumée de la CI (.github/workflows/packages.yml), jamais sur un
# bureau réel : lance ChatonLM (la commande en arguments : chatonlm du .deb
# installé, ou l'AppImage) sous un Xvfb et un bus D-Bus de session neufs, avec
# un dossier de données temporaire et l'adresse d'Ollama fermée (OLLAMA_HOST :
# jamais un serveur de la machine). Échec si l'app s'est arrêtée au bout de
# SMOKE_WAIT secondes (20 par défaut), si aucun WebKitWebProcess ne tourne, ou
# si le journal contient « panicked ».
#
#   packaging/ci/smoke-launch.sh chatonlm
#   APPIMAGE_EXTRACT_AND_RUN=1 packaging/ci/smoke-launch.sh ./chatonlm_x_amd64.AppImage
set -euo pipefail

# Plus aucune option (l'ancien --builtin lançait l'Ollama intégré, retiré des
# paquets) : une option ici serait prise pour la commande à lancer.
case "${1:-}" in
  "" | -*)
    echo "Usage : $0 <commande de ChatonLM>" >&2
    exit 2
    ;;
esac

if [ "${CHATONLM_SMOKE_INNER:-}" != 1 ]; then
  export CHATONLM_SMOKE_INNER=1
  exec xvfb-run -a -s "-screen 0 1360x960x24" dbus-run-session -- "$0" "$@"
fi

work="$(mktemp -d)"
log="$work/chatonlm.log"
export CHATONLM_DATA_DIR="$work/data"
export GDK_BACKEND=x11
export OLLAMA_HOST=http://127.0.0.1:9
status=0
fail() {
  echo "✗ $*"
  status=1
}

# Pour le journal : les machines de la CI n'ont ni micro ni sortie audio
# (lecture à voix haute et mode vocal sans périphérique : l'app doit démarrer
# quand même ; leurs échecs propres sont vérifiés par e2e/no-audio.test.mjs).
cards="$(grep -c '^ *[0-9]' /proc/asound/cards 2>/dev/null || true)"
echo "Cartes son : ${cards:-0}"

"$@" >"$log" 2>&1 &
pid=$!

# Processus lancés depuis `$1` (lui compris).
descendants() {
  local pids=("$1") i=0 child
  while [ "$i" -lt "${#pids[@]}" ]; do
    for child in $(pgrep -P "${pids[$i]}" || true); do
      pids+=("$child")
    done
    i=$((i + 1))
  done
  printf '%s\n' "${pids[@]}"
}

# Processus de ChatonLM lui-même (sous l'AppImage, enfant de son runtime).
app_pid() {
  local p
  for p in $(descendants "$pid"); do
    if [ "$(basename "$(readlink "/proc/$p/exe" 2>/dev/null || true)")" = chatonlm ]; then
      echo "$p"
      return
    fi
  done
}

sleep "${SMOKE_WAIT:-20}"

if ! kill -0 "$pid" 2>/dev/null; then
  fail "ChatonLM s'est arrêté avant la fin de l'essai."
fi
if pgrep -f WebKitWebProcess >/dev/null; then
  echo "✓ WebKitWebProcess : $(pgrep -fc WebKitWebProcess)"
else
  fail "Aucun WebKitWebProcess."
fi
if grep -n panicked "$log"; then
  fail "Panique dans le journal."
fi
[ -d "$CHATONLM_DATA_DIR" ] && echo "Données : $(find "$CHATONLM_DATA_DIR" -maxdepth 1 | tail -n +2 | xargs -r -n1 basename | tr '\n' ' ')"

# Arrêt de ChatonLM lui-même (sous l'AppImage, son runtime attend sa fin
# avant de supprimer le dossier extrait).
target="$(app_pid)"
kill -TERM "${target:-$pid}" 2>/dev/null || true
for _ in $(seq 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.2; done
kill -KILL "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true

echo "--- journal de ChatonLM ---"
cat "$log"
rm -rf "$work"
[ "$status" = 0 ] && echo "✓ Essai de ChatonLM réussi."
exit "$status"
