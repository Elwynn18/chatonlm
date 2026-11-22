#!/usr/bin/env bash
# Télécharge ONNX Runtime à la version épinglée (packaging/onnxruntime.env :
# paquet NuGet officiel), vérifie son empreinte et place dans
# .tools/runtimes/onnxruntime :
#   libonnxruntime.so.1   (Linux : sous son nom de SONAME, sans liens
#                          symboliques)
#   onnxruntime.dll       (Windows : sous Git Bash, ou avec --windows)
#   LICENSE, ThirdPartyNotices.txt, VERSION
# Rien n'est retéléchargé si la version demandée est déjà là. Affiche le
# chemin de la bibliothèque (chemin Windows sous Git Bash : il sert tel quel
# à ORT_DYLIB_PATH). Sert aux paquets (scripts/package.sh, CI Windows) et aux
# essais (ORT_DYLIB_PATH, posé par activate.sh).
#
# Usage : scripts/fetch-onnxruntime.sh [--windows]
#   --windows : la bibliothèque de Windows depuis un autre système (essai du
#   script) ; le dossier est remplacé, la copie de Linux est donc à reprendre
#   ensuite.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../packaging/onnxruntime.env
source "$root/packaging/onnxruntime.env"

windows=0
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) windows=1 ;;
esac
if [ "${1:-}" = "--windows" ]; then
  windows=1
fi

dest="$root/.tools/runtimes/onnxruntime"
# Paquet NuGet officiel (le même pour les deux systèmes).
url="$ORT_NUGET_URL"
sha256="$ORT_NUGET_SHA256"
if [ "$windows" = 1 ]; then
  name="onnxruntime.dll"
  member="runtimes/win-x64/native/onnxruntime.dll"
else
  name="libonnxruntime.so.1"
  member="runtimes/linux-x64/native/libonnxruntime.so"
fi
lib="$dest/$name"

# Chemin affiché : celui de Windows sous Git Bash (`/d/a/…` ne dit rien à
# l'app).
show() {
  if [ "$windows" = 1 ] && command -v cygpath >/dev/null 2>&1; then
    cygpath -w "$1"
  else
    echo "$1"
  fi
}

if [ -f "$lib" ] && [ "$(cat "$dest/VERSION" 2>/dev/null)" = "$ORT_VERSION" ]; then
  show "$lib"
  exit 0
fi

work="$root/.tools/runtimes/onnxruntime-$ORT_VERSION.part"
rm -rf "$work"
mkdir -p "$work/x"
archive="$work/onnxruntime.archive"
curl -sSfL --retry 3 -o "$archive" "$url"
if ! echo "$sha256  $archive" | sha256sum -c --quiet -; then
  echo "Empreinte inattendue pour ONNX Runtime ($url)." >&2
  rm -rf "$work"
  exit 1
fi
# Paquet vérifié : on n'en extrait que ce qui est livré (un .nupkg est un zip ;
# chaque nom est donné en entier, l'unzip de Git pour Windows arrêtant « * »
# aux « / »).
unzip -q "$archive" "$member" LICENSE ThirdPartyNotices.txt -d "$work/x"

rm -rf "$dest"
mkdir -p "$dest"
install -m 0644 "$work/x/$member" "$lib"
install -m 0644 "$work/x/LICENSE" "$dest/LICENSE"
install -m 0644 "$work/x/ThirdPartyNotices.txt" "$dest/ThirdPartyNotices.txt"
echo "$ORT_VERSION" >"$dest/VERSION"
rm -rf "$work"
show "$lib"
