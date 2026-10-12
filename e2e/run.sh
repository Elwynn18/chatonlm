#!/usr/bin/env bash
# Lance un test WebDriver de l'app réelle, dans la session virtuelle :
#   scripts/e2e-session.sh e2e/run.sh e2e/smoke.test.mjs "$(scripts/e2e-build.sh)"
# Le test lance lui-même l'app (e2e/lib/app.mjs), qui doit embarquer le
# serveur WebDriver (fonctionnalité « e2e », ce que fait scripts/e2e-build.sh).
set -euo pipefail

if [ "${CHATONLM_E2E_SESSION:-}" != 1 ]; then
  echo "À lancer dans la session virtuelle (scripts/e2e-session.sh)." >&2
  exit 2
fi

test_file="${1:?Usage : $0 <test.mjs> <binaire>}"
binary="${2:?Usage : $0 <test.mjs> <binaire>}"

exec node "$test_file" "$binary"
