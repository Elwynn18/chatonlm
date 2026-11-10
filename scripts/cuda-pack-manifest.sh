#!/usr/bin/env bash
# Manifeste d'un pack CUDA (packaging/cuda.env), à côté de son archive :
# chatonlm-cuda-<build>-<système>-x64.json = build, VERSION du pack, nom,
# taille et SHA-256 de l'archive. Signé ensuite par la clé des mises à jour
# (release.yml), il est la seule chose que l'app croit avant de télécharger
# l'archive (engine/cuda/pack.rs). Bash seul : le conteneur CUDA de la CI n'a
# pas Node.
#
# Usage : scripts/cuda-pack-manifest.sh <archive> <fichier VERSION>
set -euo pipefail

archive="${1:?archive}"
version_file="${2:?VERSION}"
name="$(basename "$archive")"
if [[ ! "$name" =~ ^chatonlm-cuda-(b[0-9]+)-(linux|windows)-x64\.(tar\.gz|zip)$ ]]; then
  echo "cuda-pack-manifest : nom d'archive inattendu : $name" >&2
  exit 1
fi
build="${BASH_REMATCH[1]}"
system="${BASH_REMATCH[2]}"
version="$(tr -d '\r\n' <"$version_file")"
[[ "$version" =~ ^v[0-9.]+\+chatonlm\.cuda\.[0-9a-f]{12}$ ]] ||
  { echo "cuda-pack-manifest : VERSION inattendue : $version" >&2; exit 1; }
size="$(wc -c <"$archive" | tr -d ' ')"
sum="$(sha256sum "$archive" | cut -c1-64)"
out="$(dirname "$archive")/chatonlm-cuda-$build-$system-x64.json"
printf '{"build":"%s","version":"%s","file":"%s","size":%s,"sha256":"%s"}\n' \
  "$build" "$version" "$name" "$size" "$sum" >"$out"
echo "$out"
