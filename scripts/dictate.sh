#!/usr/bin/env bash
# Raccourci de dictée pour la recette, tant que ChatonLM n'est pas installé
# (étape 18 : fichier .desktop, raccourci par le portail du bureau). À lier
# soi-même : Configuration du système > Raccourcis > Ajouter une commande, avec
# le chemin de ce script entre guillemets.
#
# Bascule la dictée de ChatonLM lancé par « scripts/demo.sh --real » (instance
# unique : la relance ne fait que transmettre --dictate). S'il ne tourne pas,
# le lance comme demo.sh --real (build de production, données dans
# .tools/real-data), fenêtre masquée, dictée commencée : rien n'est écrit hors
# du projet. Le build doit exister (scripts/demo.sh --real --build-only).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
# shellcheck source=../activate.sh
source ./activate.sh

binary="$root/.tools/real-target/release/chatonlm"
if [ ! -x "$binary" ]; then
  echo "Pas de build de recette : lancer d'abord scripts/demo.sh --real." >&2
  exit 1
fi
mkdir -p "$root/.tools/real-data" "$root/.tools/tmp"
exec env -u CHATONLM_SEED_DEMO -u CHATONLM_REGISTRY_URL -u CHATONLM_OLLAMA_COM_URL \
  CHATONLM_DATA_DIR="$root/.tools/real-data" TMPDIR="$root/.tools/tmp" \
  CHATONLM_NO_GLOBAL_SHORTCUTS=1 "$binary" --dictate
