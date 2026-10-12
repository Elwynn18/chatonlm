#!/usr/bin/env bash
# Instance Ollama de test, isolée dans .tools/ollama-test : elle écoute sur
# 127.0.0.1:11435 et garde modèles, clés et caches dans le projet. Le service
# système (port 11434) n'est pas touché.
#
# Usage : scripts/test-ollama.sh start [--builtin] | stop | status | kind | logs [n] | pull <modèle>
#   start             avec l'Ollama de la machine (ollama du PATH)
#   start --builtin   avec l'Ollama intégré aux paquets (scripts/fetch-ollama.sh,
#                     .tools/runtimes/ollama), mêmes modèles et même port :
#                     les contrats passent contre l'une puis l'autre version
#   kind              « system » ou « builtin » selon le binaire de l'instance
#                     lancée (rien, code 1, si elle est arrêtée)
# CHATONLM_OLLAMA_LOG_REQUESTS=1 au démarrage enregistre le corps des requêtes
# (dans .tools/ollama-test/tmp), pour vérifier ce que ChatonLM envoie.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../activate.sh
source "$root/activate.sh"

base="$root/.tools/ollama-test"
host="127.0.0.1:11435"
url="http://$host"
pidfile="$base/serve.pid"
log="$base/serve.log"
builtin_bin="$root/.tools/runtimes/ollama/bin/ollama"

running() {
  [ -f "$pidfile" ] || return 1
  local pid
  pid="$(cat "$pidfile")"
  # Le PID enregistré peut avoir été repris par un autre programme (après un
  # redémarrage) : on vérifie que c'est bien notre « ollama serve » (binaire
  # du PATH ou intégré, appelé par son chemin complet).
  local -a argv=()
  mapfile -d '' -t argv <"/proc/$pid/cmdline" 2>/dev/null || true
  if [ "${#argv[@]}" -eq 2 ] && [ "$(basename "${argv[0]}")" = ollama ] &&
    [ "${argv[1]}" = serve ] && grep -qzsxF "OLLAMA_HOST=$host" "/proc/$pid/environ"; then
    return 0
  fi
  rm -f "$pidfile"
  return 1
}

# Binaire de l'instance lancée : « builtin » (copie de .tools/runtimes) ou
# « system ».
kind() {
  local exe
  exe="$(readlink "/proc/$(cat "$pidfile")/exe" 2>/dev/null || true)"
  if [ "$exe" = "$(readlink -f "$builtin_bin" 2>/dev/null || echo "$builtin_bin")" ]; then
    echo builtin
  else
    echo system
  fi
}

describe() {
  case "$(kind)" in
    builtin) echo "Ollama intégré ($builtin_bin)" ;;
    *) echo "Ollama de la machine ($(readlink "/proc/$(cat "$pidfile")/exe" 2>/dev/null || echo ollama))" ;;
  esac
}

case "${1:-status}" in
  start)
    want=system
    case "${2:-}" in
      "") ;;
      --builtin) want=builtin ;;
      *)
        echo "Usage : $0 start [--builtin]" >&2
        exit 2
        ;;
    esac
    if running; then
      if [ "$(kind)" != "$want" ]; then
        echo "Déjà lancée avec un autre binaire ($(describe)) : « $0 stop » d'abord." >&2
        exit 1
      fi
      echo "Déjà lancée (PID $(cat "$pidfile"), $(describe)) sur $url"
      exit 0
    fi
    if [ "$want" = builtin ]; then
      "$root/scripts/fetch-ollama.sh" >/dev/null
      ollama="$builtin_bin"
    else
      ollama="$(command -v ollama)" || { echo "ollama introuvable dans le PATH" >&2; exit 1; }
    fi
    mkdir -p "$base/home" "$base/models" "$base/tmp"
    if [ "${CHATONLM_OLLAMA_LOG_REQUESTS:-}" = "1" ]; then
      export OLLAMA_DEBUG_LOG_REQUESTS=1
    fi
    echo "--- $(date '+%F %T') : $ollama serve" >>"$log"
    HOME="$base/home" OLLAMA_MODELS="$base/models" OLLAMA_HOST="$host" TMPDIR="$base/tmp" \
      setsid "$ollama" serve >>"$log" 2>&1 </dev/null &
    echo $! >"$pidfile"
    for _ in $(seq 1 60); do
      if curl -sf --max-time 1 "$url/api/version" >/dev/null; then
        echo "Lancée (PID $(cat "$pidfile"), $(describe)) sur $url : $(curl -s "$url/api/version")"
        exit 0
      fi
      sleep 0.5
    done
    echo "Pas de réponse après 30 s ; voir $log" >&2
    exit 1
    ;;
  stop)
    if ! running; then
      echo "Pas lancée."
      rm -f "$pidfile"
      exit 0
    fi
    pid="$(cat "$pidfile")"
    kill -TERM "$pid"
    for _ in $(seq 1 20); do
      kill -0 "$pid" 2>/dev/null || { rm -f "$pidfile"; echo "Arrêtée."; exit 0; }
      sleep 0.5
    done
    kill -KILL "$pid" 2>/dev/null || true
    rm -f "$pidfile"
    echo "Arrêtée (forcée)."
    ;;
  status)
    if running && curl -sf --max-time 2 "$url/api/version" >/dev/null; then
      echo "En marche (PID $(cat "$pidfile"), $(describe)) sur $url : $(curl -s "$url/api/version")"
      # shellcheck disable=SC2016 # code JavaScript, pas de développement shell
      curl -s "$url/api/tags" | node -e '
        const d = JSON.parse(require("fs").readFileSync(0, "utf8"));
        for (const m of d.models ?? []) console.log(`  ${m.name}  ${(m.size / 1e9).toFixed(2)} Go`);'
    else
      echo "Arrêtée."
      exit 1
    fi
    ;;
  kind)
    running || exit 1
    kind
    ;;
  logs)
    tail -n "${2:-50}" "$log"
    ;;
  pull)
    model="${2:?Usage : $0 pull <modèle>}"
    running || { echo "Instance arrêtée : lancez « $0 start »." >&2; exit 1; }
    # shellcheck disable=SC2016 # code JavaScript, pas de développement shell
    curl -sfN -X POST "$url/api/pull" -H 'Content-Type: application/json' \
      -d "{\"model\":\"$model\"}" | node -e '
        const rl = require("readline").createInterface({ input: process.stdin });
        let last = "", lastPct = -10, failed = false;
        rl.on("line", (line) => {
          const e = JSON.parse(line);
          if (e.error) { console.error("Erreur : " + e.error); failed = true; return; }
          const pct = e.total ? Math.floor((100 * (e.completed ?? 0)) / e.total) : null;
          if (e.status !== last || (pct !== null && pct >= lastPct + 10)) {
            console.log(pct === null ? e.status : `${e.status} ${pct} %`);
            last = e.status; lastPct = pct ?? -10;
          }
        });
        rl.on("close", () => process.exit(failed ? 1 : 0));'
    ;;
  *)
    echo "Usage : $0 start [--builtin] | stop | status | kind | logs [n] | pull <modèle>" >&2
    exit 2
    ;;
esac
