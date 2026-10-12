#!/usr/bin/env bash
# Clés de signature des mises à jour (greffon tauri-plugin-updater, minisign).
#   scripts/updater-keys.sh --production   clé des paquets publiés
#   scripts/updater-keys.sh --test         clé des AppImages d'essai (e2e)
#
# Production : .tools/updater-keys/ ; essai : .tools/updater-test/. Chaque
# dossier reçoit `chatonlm.key` (clé privée chiffrée), `chatonlm.key.pub` (clé
# publique, valeur de `plugins.updater.pubkey`) et `password` (mot de passe
# aléatoire), en droits 600, jamais dans $HOME. Une clé déjà présente n'est
# JAMAIS remplacée : perdue ou écrasée, la clé de production empêcherait toute
# mise à jour des installations existantes (la sauvegarder hors de la
# machine). Rien de secret n'est affiché : la sortie de `tauri signer
# generate` va dans `generate.log`, à côté des clés.
#
# CI : la clé privée et son mot de passe sont les secrets
# TAURI_SIGNING_PRIVATE_KEY et TAURI_SIGNING_PRIVATE_KEY_PASSWORD du dépôt
# (posés par l'utilisateur, jamais par un script), reçus par la seule étape
# « Signature » du job `sign` de .github/workflows/release.yml (jamais par la
# construction) ; ses signatures sont vérifiées avec la clé publique de
# src-tauri/tauri.conf.json.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
# shellcheck source=../activate.sh
source "$root/activate.sh"

case "${1:-}" in
  --production) dir="$root/.tools/updater-keys" ;;
  --test) dir="$root/.tools/updater-test" ;;
  *)
    echo "Usage : $0 --production|--test" >&2
    exit 2
    ;;
esac

key="$dir/chatonlm.key"
if [ -e "$key" ] || [ -e "$key.pub" ] || [ -e "$dir/password" ]; then
  echo "Clé déjà présente dans $dir : rien n'est remplacé." >&2
  [ -f "$key.pub" ] && echo "Clé publique : $key.pub"
  exit 0
fi

umask 077
mkdir -p "$dir"
chmod 700 "$dir"
# 32 caractères alphanumériques (environ 190 bits). Sans « tr </dev/urandom |
# head » : tr, arrêté par SIGPIPE, ferait échouer le pipeline (pipefail).
password="$(head -c 48 /dev/urandom | base64 -w0 | LC_ALL=C tr -dc 'A-Za-z0-9' | cut -c1-32)"
printf '%s' "$password" >"$dir/password"
if ! node_modules/.bin/tauri signer generate --ci -p "$password" -w "$key" \
  >"$dir/generate.log" 2>&1; then
  rm -f "$key" "$key.pub" "$dir/password"
  echo "Échec de la génération (journal : $dir/generate.log)." >&2
  exit 1
fi
unset password
chmod 600 "$key" "$key.pub" "$dir/password" "$dir/generate.log"
echo "Clé publique : $key.pub"
