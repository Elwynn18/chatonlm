#!/usr/bin/env bash
# Essai de fumée de la CI (.github/workflows/packages.yml), jamais sur un
# bureau réel : lance ChatonLM (la commande en arguments : chatonlm du .deb
# installé, ou l'AppImage) sous un Xvfb et un bus D-Bus de session neufs, avec
# un dossier de données temporaire. Deux modes :
#
# - par défaut : adresse d'Ollama fermée (OLLAMA_HOST, adresse explicite :
#   jamais l'Ollama intégré). Échec si l'app s'est arrêtée au bout de
#   SMOKE_WAIT secondes (20 par défaut), si aucun WebKitWebProcess ne tourne,
#   ou si le journal contient « panicked ».
# - --builtin : sans OLLAMA_HOST, sur une machine sans Ollama (rien sur le
#   port 11434, pas de commande ollama) : l'app démarre elle-même l'Ollama
#   intégré de son paquet. Échec, en plus des contrôles ci-dessus, si
#   <données>/logs/ollama.log n'annonce pas « Listening on 127.0.0.1:<port> »
#   dans les SMOKE_BUILTIN_WAIT secondes (120 par défaut), si ce port ne
#   répond pas à /api/version avec la version du fichier VERSION livré, si le
#   serveur n'est pas le binaire du paquet (/usr/lib/chatonlm/ollama, ou
#   usr/libexec/chatonlm/ollama de l'AppImage, atteint par le lien
#   usr/lib/chatonlm/ollama) lancé par l'app, si son environnement garde
#   LD_LIBRARY_PATH, LD_PRELOAD, APPDIR ou un chemin de l'AppImage, ou si son
#   HOME et son dossier des modèles ne sont pas ceux des données de l'app ;
#   enfin, l'app arrêtée par SIGTERM, s'il reste un processus de l'Ollama
#   intégré 15 s plus tard.
#
#   packaging/ci/smoke-launch.sh chatonlm
#   packaging/ci/smoke-launch.sh --builtin chatonlm
#   APPIMAGE_EXTRACT_AND_RUN=1 packaging/ci/smoke-launch.sh [--builtin] ./chatonlm_x_amd64.AppImage
set -euo pipefail

if [ "${CHATONLM_SMOKE_INNER:-}" != 1 ]; then
  export CHATONLM_SMOKE_INNER=1
  exec xvfb-run -a -s "-screen 0 1360x960x24" dbus-run-session -- "$0" "$@"
fi

builtin=0
if [ "${1:-}" = --builtin ]; then
  builtin=1
  shift
fi

work="$(mktemp -d)"
log="$work/chatonlm.log"
export CHATONLM_DATA_DIR="$work/data"
export GDK_BACKEND=x11
status=0
fail() {
  echo "✗ $*"
  status=1
}

if [ "$builtin" = 1 ]; then
  unset OLLAMA_HOST
  # L'intégré ne démarre seul que si aucun Ollama n'est installé ni ne répond.
  if curl -s --max-time 2 -o /dev/null http://127.0.0.1:11434/; then
    echo "✗ Un serveur répond déjà sur 127.0.0.1:11434 : essai impossible ici."
    exit 1
  fi
  if command -v ollama >/dev/null; then
    echo "✗ Commande ollama installée ($(command -v ollama)) : essai impossible ici."
    exit 1
  fi
else
  export OLLAMA_HOST=http://127.0.0.1:9
fi

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

# Processus de l'Ollama intégré, de ChatonLM ou non : « pid exécutable ».
builtin_processes() {
  local p exe
  for p in /proc/[0-9]*; do
    exe="$(readlink "$p/exe" 2>/dev/null || true)"
    case "$exe" in
      */chatonlm/ollama/*) echo "${p#/proc/} $exe" ;;
    esac
  done
}

ollama_log="$CHATONLM_DATA_DIR/logs/ollama.log"
if [ "$builtin" = 1 ]; then
  port=""
  for _ in $(seq "$((${SMOKE_BUILTIN_WAIT:-120} * 2))"); do
    port="$(grep -ao 'Listening on 127\.0\.0\.1:[0-9]*' "$ollama_log" 2>/dev/null |
      tail -n 1 | sed 's/.*://' || true)"
    [ -n "$port" ] && break
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.5
  done
  if [ -z "$port" ]; then
    fail "L'Ollama intégré n'écoute pas (rien dans $ollama_log après ${SMOKE_BUILTIN_WAIT:-120} s)."
  else
    echo "✓ Ollama intégré lancé par l'app : 127.0.0.1:$port."
    app="$(app_pid)"
    # Serveur : enfant de l'app (ou de son runtime), « ollama serve ».
    serve=""
    for p in $(descendants "$pid"); do
      case "$(readlink "/proc/$p/exe" 2>/dev/null || true)" in
        */chatonlm/ollama/bin/ollama)
          if grep -q ' serve' <<<"$(tr '\0' ' ' <"/proc/$p/cmdline")"; then
            serve="$p"
            break
          fi
          ;;
      esac
    done
    if [ -z "$serve" ]; then
      fail "Aucun processus « ollama serve » du paquet lancé par l'app : $(builtin_processes | tr '\n' ' ')"
    else
      exe="$(readlink "/proc/$serve/exe" || true)"
      # Dossier attendu : celui du .deb (/usr/bin/chatonlm), ou celui de
      # l'AppImage extraite (<AppDir>/usr/bin/chatonlm ; APPDIR, que le moteur
      # retire de l'environnement de l'intégré).
      app_exe="$(readlink "/proc/${app:-$pid}/exe" 2>/dev/null || true)"
      appdir=""
      if [ -n "$app_exe" ] && [ "$app_exe" != /usr/bin/chatonlm ]; then
        appdir="$(tr '\0' '\n' <"/proc/${app:-$pid}/environ" | sed -n 's/^APPDIR=//p' || true)"
        appdir="${appdir:-${app_exe%/usr/bin/chatonlm}}"
        expected="$(readlink -f "$appdir")/usr/libexec/chatonlm/ollama/bin/ollama"
      else
        expected=/usr/lib/chatonlm/ollama/bin/ollama
      fi
      if [ "$exe" = "$expected" ]; then
        echo "✓ Binaire du paquet : $exe (pid $serve)."
      else
        fail "Binaire inattendu : $exe (attendu : $expected)."
      fi
      # Version servie : celle du VERSION livré à côté du binaire.
      shipped="$(cat "$(dirname "$(dirname "$exe")")/VERSION" 2>/dev/null || true)"
      answer="$(curl -sf --max-time 5 "http://127.0.0.1:$port/api/version" || true)"
      if [ -n "$shipped" ] && [ "$answer" = "{\"version\":\"$shipped\"}" ]; then
        echo "✓ /api/version : $shipped."
      else
        fail "/api/version : « $answer » au lieu de la version livrée « $shipped »."
      fi
      # Environnement : nettoyé de l'AppImage, données de l'app.
      environment="$(tr '\0' '\n' <"/proc/$serve/environ" || true)"
      for key in LD_LIBRARY_PATH LD_PRELOAD APPDIR; do
        if grep -q "^$key=" <<<"$environment"; then
          fail "Environnement de l'Ollama intégré : $(grep "^$key=" <<<"$environment")"
        fi
      done
      if [ -n "$appdir" ] && grep -qF "$appdir" <<<"$environment"; then
        fail "Chemins de l'AppImage dans l'environnement de l'Ollama intégré : $(grep -F "$appdir" <<<"$environment" | cut -d= -f1 | tr '\n' ' ')"
      fi
      data="$(readlink -f "$CHATONLM_DATA_DIR")"
      for pair in "HOME=$data/ollama/home" "OLLAMA_MODELS=$data/ollama/models" \
        "OLLAMA_HOST=127.0.0.1:$port"; do
        key="${pair%%=*}"
        value="$(sed -n "s/^$key=//p" <<<"$environment")"
        if [ "$key" != OLLAMA_HOST ] && [ -n "$value" ]; then
          value="$(readlink -m "$value")"
        fi
        if [ "$key=$value" = "$pair" ]; then
          echo "✓ $pair"
        else
          fail "$key=$value au lieu de ${pair#*=}"
        fi
      done
    fi
  fi
else
  sleep "${SMOKE_WAIT:-20}"
fi

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

if [ "$builtin" = 1 ]; then
  # Plus aucun Ollama intégré (serveur, runners) après l'arrêt de l'app.
  left=""
  for _ in $(seq 30); do
    left="$(builtin_processes)"
    [ -z "$left" ] && break
    sleep 0.5
  done
  if [ -n "$left" ]; then
    fail "Ollama intégré encore actif 15 s après l'arrêt de l'app : $(tr '\n' ';' <<<"$left")"
    while read -r p _; do kill -KILL "$p" 2>/dev/null || true; done <<<"$left"
  else
    echo "✓ Aucun processus de l'Ollama intégré après l'arrêt de l'app."
  fi
  echo "--- journal de l'Ollama intégré ---"
  cat "$ollama_log" 2>/dev/null || echo "(absent)"
fi

echo "--- journal de ChatonLM ---"
cat "$log"
rm -rf "$work"
[ "$status" = 0 ] && echo "✓ Essai de ChatonLM réussi."
exit "$status"
