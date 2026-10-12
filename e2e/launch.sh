#!/usr/bin/env bash
# Scénario de démarrage, dans la session virtuelle :
#   scripts/e2e-session.sh e2e/launch.sh "$(scripts/e2e-build.sh)"
# Démarre l'app, vérifie que c'est le frontend qui a affiché la fenêtre (pas le
# filet de secours du backend) et que l'écran a changé, puis capture l'écran.
set -euo pipefail

if [ "${CHATONLM_E2E_SESSION:-}" != 1 ]; then
  echo "À lancer dans la session virtuelle (scripts/e2e-session.sh)." >&2
  exit 2
fi

binary="${1:?Usage : $0 <binaire>}"
name="${2:-launch}"
log=".tools/logs/e2e-$name.log"

scripts/e2e-session.sh --shot "$name-before" >/dev/null

# Adresses d'Ollama fermées, celle de l'app et l'adresse locale standard
# qu'elle propose en remplacement (comme withApp) : sans elles, l'app interroge
# l'Ollama de la machine s'il tourne (qui réécrit alors son cache de
# recommandations dans ~/.ollama). L'écran du serveur suffit à cet essai.
OLLAMA_HOST=http://127.0.0.1:9 CHATONLM_E2E_STANDARD_URL=http://127.0.0.1:1 \
  "$binary" >"$log" 2>&1 &
app=$!
trap 'kill "$app" 2>/dev/null || true; wait "$app" 2>/dev/null || true' EXIT
sleep "${E2E_WAIT:-5}"

if ! kill -0 "$app" 2>/dev/null; then
  echo "L'app s'est arrêtée au démarrage ; voir $log" >&2
  exit 1
fi
if grep -q 'startup fallback' "$log"; then
  echo "Fenêtre affichée par le filet de secours, pas par le frontend ; voir $log" >&2
  exit 1
fi

shot="$(scripts/e2e-session.sh --shot "$name")"
if cmp -s "$shot" ".tools/shots/$name-before.png"; then
  echo "L'écran n'a pas changé : aucune fenêtre affichée." >&2
  exit 1
fi
rm -f ".tools/shots/$name-before.png"
echo "$shot"
