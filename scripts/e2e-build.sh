#!/usr/bin/env bash
# Compile l'app pour les essais en session virtuelle : frontend embarqué, profil
# debug, fonctionnalité « e2e » (serveur WebDriver), dans un dossier cible dédié
# (.tools/e2e-target). « tauri dev » ne peut donc jamais remplacer ce binaire
# par un binaire du mode dev, qui chargerait le serveur Vite. Affiche le chemin
# du binaire produit. Les arguments sont transmis à « tauri build ».
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../activate.sh
source "$root/activate.sh"
cd "$root"

export CARGO_TARGET_DIR="$root/.tools/e2e-target"
export TMPDIR="$root/.tools/tmp"
mkdir -p "$TMPDIR" "$root/.tools/logs"

log="$root/.tools/logs/e2e-build.log"
# L'app, puis le faux serveur Ollama dont se servent les tests (même dossier cible).
if ! node_modules/.bin/tauri build --debug --no-bundle --features e2e "$@" >"$log" 2>&1 ||
  ! cargo build --manifest-path src-tauri/Cargo.toml -p fake-ollama >>"$log" 2>&1; then
  tail -20 "$log" >&2
  exit 1
fi
echo "$CARGO_TARGET_DIR/debug/chatonlm"
