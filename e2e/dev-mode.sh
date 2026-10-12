#!/usr/bin/env bash
# Vérifie le mode développement, dans la session virtuelle :
#   scripts/e2e-session.sh e2e/dev-mode.sh
# « tauri dev » démarre avec l'identifiant .dev et le dossier de données de
# l'essai, le serveur Vite refuse de servir .tools/, et la fenêtre s'affiche.
set -euo pipefail

if [ "${CHATONLM_E2E_SESSION:-}" != 1 ]; then
  echo "À lancer dans la session virtuelle (scripts/e2e-session.sh)." >&2
  exit 2
fi

log=".tools/logs/e2e-dev-mode.log"
setsid scripts/dev.sh >"$log" 2>&1 &
dev=$!
trap 'kill -TERM -- "-$dev" 2>/dev/null || true; wait "$dev" 2>/dev/null || true' EXIT

for _ in $(seq 1 180); do
  grep -q 'chatonlm: identifier=' "$log" && break
  kill -0 "$dev" 2>/dev/null || { echo "tauri dev s'est arrêté ; voir $log" >&2; exit 1; }
  sleep 1
done
line="$(grep -m1 'chatonlm: identifier=' "$log")" || {
  echo "L'app n'a pas démarré en 180 s ; voir $log" >&2
  exit 1
}
echo "$line"

case "$line" in
  *"identifier=com.chatonlm.chatonlm.dev data=Ok(\"$CHATONLM_DATA_DIR\")"*) ;;
  *)
    echo "Identifiant ou dossier de données inattendu (attendu : .dev et $CHATONLM_DATA_DIR)" >&2
    exit 1
    ;;
esac

code="$(curl -s -o /dev/null -w '%{http_code}' \
  'http://localhost:1420/.tools/research/plan.md?import&raw')"
if [ "$code" != 403 ]; then
  echo "Le serveur Vite sert .tools/ (HTTP $code au lieu de 403)" >&2
  exit 1
fi
echo "Vite refuse .tools/ (HTTP 403)"

sleep 3
if grep -q 'startup fallback' "$log"; then
  echo "Fenêtre affichée par le filet de secours ; voir $log" >&2
  exit 1
fi
scripts/e2e-session.sh --shot dev-mode
