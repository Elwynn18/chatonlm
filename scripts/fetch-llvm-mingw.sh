#!/usr/bin/env bash
# Télécharge llvm-mingw à la version épinglée (packaging/llvm-mingw.env),
# vérifie son empreinte et l'extrait dans .tools/llvm-mingw/<version>, puis
# ajoute la cible Rust x86_64-pc-windows-gnullvm (dans .tools/rustup). Rien
# n'est retéléchargé si la version demandée est déjà là. Affiche le dossier
# des exécutables. Sert à scripts/check-windows.sh.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../packaging/llvm-mingw.env
source "$root/packaging/llvm-mingw.env"

dest="$root/.tools/llvm-mingw/$LLVM_MINGW_VERSION"
if [ ! -x "$dest/bin/x86_64-w64-mingw32-clang" ]; then
  work="$root/.tools/llvm-mingw/$LLVM_MINGW_VERSION.part"
  rm -rf "$work"
  mkdir -p "$work/x"
  archive="$work/llvm-mingw.tar.xz"
  curl -sSfL --retry 3 -o "$archive" "$LLVM_MINGW_URL"
  if ! echo "$LLVM_MINGW_SHA256  $archive" | sha256sum -c --quiet -; then
    echo "Empreinte inattendue pour llvm-mingw ($LLVM_MINGW_URL)." >&2
    rm -rf "$work"
    exit 1
  fi
  tar -xJf "$archive" -C "$work/x" --strip-components=1
  rm -rf "$dest"
  mv "$work/x" "$dest"
  rm -rf "$work"
fi

if ! rustup target list --installed | grep -qx x86_64-pc-windows-gnullvm; then
  rustup target add x86_64-pc-windows-gnullvm >&2
fi
echo "$dest/bin"
