#!/usr/bin/env bash
# Lance ChatonLM en développement, données et caches confinés dans .tools/.
# Les arguments sont transmis à « tauri dev ».
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../activate.sh
source "$root/activate.sh"
# shellcheck source=lib/guard.sh
source "$root/scripts/lib/guard.sh"
chatonlm_guard "$root"
cd "$root"

export TMPDIR="$root/.tools/tmp"
mkdir -p "$TMPDIR"

# Identifiant distinct en dev : pas de conflit d'instance unique avec une
# version installée.
exec node_modules/.bin/tauri dev --config src-tauri/tauri.dev.conf.json "$@"
