#!/usr/bin/env bash
# Télécharge VSCodium à la version épinglée (packaging/vscodium.env), vérifie
# son empreinte et l'extrait dans .tools/vscodium/<version>. Rien n'est
# retéléchargé si cette version est déjà là. Affiche le chemin de l'exécutable.
# Sert à l'essai de l'extension dans un vrai éditeur
# (e2e/vscode-extension.test.mjs, check.sh --full). Linux x64 seulement.
#
# Usage : scripts/fetch-vscodium.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../packaging/vscodium.env
source "$root/packaging/vscodium.env"

dest="$root/.tools/vscodium/$VSCODIUM_VERSION"
exe="$dest/codium"
if [ -x "$exe" ] && [ "$(cat "$dest/.chatonlm-version" 2>/dev/null)" = "$VSCODIUM_VERSION" ]; then
  echo "$exe"
  exit 0
fi

work="$root/.tools/vscodium/download"
rm -rf "$work" "$dest"
mkdir -p "$work" "$dest"
archive="$work/vscodium.tar.gz"
curl -fsSL --retry 3 -o "$archive" "$VSCODIUM_URL"
actual="$(sha256sum "$archive" | cut -d' ' -f1)"
if [ "$actual" != "$VSCODIUM_SHA256" ]; then
  echo "Empreinte de VSCodium inattendue : $actual (attendue : $VSCODIUM_SHA256)" >&2
  rm -rf "$work" "$dest"
  exit 1
fi
tar -xzf "$archive" -C "$dest"
rm -rf "$work"
echo "$VSCODIUM_VERSION" >"$dest/.chatonlm-version"
echo "$exe"
