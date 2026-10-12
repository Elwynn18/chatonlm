#!/usr/bin/env bash
# Ollama intégré aux paquets (packaging/ollama.env) : archive officielle à la
# version épinglée, empreinte vérifiée, extraite dans .tools/runtimes/ollama :
#   bin/ollama          le serveur
#   lib/ollama/         runner llama.cpp (llama-server, llama-quantize) et
#                       bibliothèques CPU et Vulkan, sans cuda_*, rocm*, mlx*
#                       (ni aucun fichier CUDA, ROCm/HIP ou MLX ailleurs) ;
#                       chaque bibliothèque sous le nom de son SONAME, sans
#                       lien symbolique (les paquets les copient tels quels)
#   licenses/           textes de licence livrés par Ollama dans lib/ollama
#                       (repris par scripts/package-notices.mjs)
#   VERSION             version épinglée
#   SHA256SUMS          empreintes des fichiers de bin/ et lib/ (comparées à
#                       celles des paquets par scripts/inspect-packages.mjs)
# Reprend l'archive de .tools/ollama-dist/v<version>/ si elle est là (copie
# déjà téléchargée, empreinte vérifiée de nouveau), sinon la télécharge
# (1,4 Go, CUDA compris). Rien n'est refait si la version demandée est déjà
# extraite. Contrôles : contenu attendu (aucun dossier inconnu, aucun fichier
# dont le nom désigne CUDA, ROCm/HIP ou MLX), dépendances de chaque fichier
# ELF trouvées (ldd ; seule libvulkan.so.1, chargée par le moteur Vulkan s'il
# y a un pilote, peut manquer), version de GCC et construction de libgomp,
# « ollama --version ».
# Affiche le chemin du binaire. Sert aux paquets (scripts/package.sh), à
# l'instance de test (scripts/test-ollama.sh --builtin) et aux e2e
# (CHATONLM_OLLAMA_BIN).
# Windows : voir fetch_windows plus bas (archive zip, même version épinglée).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Essais (scripts/inspect-packages.test.mjs) : fichier des versions et
# dossier .tools remplacés, avec une archive fabriquée.
env_file="${FETCH_OLLAMA_ENV:-$root/packaging/ollama.env}"
tools="${FETCH_OLLAMA_TOOLS:-$root/.tools}"
# shellcheck source=../packaging/ollama.env
source "$env_file"

# Cible : Linux, ou Windows quand le script tourne sous Git Bash (runner de
# la CI) ou avec FETCH_OLLAMA_TARGET=windows (inspection depuis Linux).
host_windows=false
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) host_windows=true ;;
esac
target="${FETCH_OLLAMA_TARGET:-}"
if [ -z "$target" ]; then
  target=linux
  $host_windows && target=windows
fi

# Ollama intégré pour Windows (x64) : archive officielle
# ollama-windows-amd64.zip à la version épinglée, empreinte vérifiée
# (OLLAMA_WINDOWS_SHA256), extraite avec l'arborescence de l'archive, celle
# qu'attend ollama.exe et que livre l'installateur :
#   ollama.exe          le serveur
#   lib/ollama/         runner llama.cpp et bibliothèques CPU, plus
#                       lib/ollama/vulkan ; sans cuda_* (ROCm et MLX sont des
#                       archives à part sous Windows)
#   licenses/           textes de licence livrés par Ollama dans lib/ollama,
#                       sans ceux de CUDA (cuDNN, CUTLASS, NVTX)
#   VERSION, SHA256SUMS comme sous Linux
# Sortie : .tools/runtimes/ollama sous Windows ; .tools/runtimes/ollama-windows
# depuis un autre système, pour ne pas remplacer l'Ollama de Linux. Les DLL ne
# sont ni renommées ni modifiées (signatures d'Ollama intactes).
# « ollama.exe --version » n'est vérifié que sous Windows.
fetch_windows() {
  local dest="$tools/runtimes/ollama"
  $host_windows || dest="$tools/runtimes/ollama-windows"
  local binary="$dest/ollama.exe"
  if [ -f "$binary" ] && [ -f "$dest/SHA256SUMS" ] &&
    [ "$(cat "$dest/VERSION" 2>/dev/null)" = "$OLLAMA_VERSION" ]; then
    echo "$binary"
    return 0
  fi

  command -v unzip >/dev/null || {
    echo "unzip introuvable : il sert à extraire l'archive d'Ollama." >&2
    return 1
  }
  if [ -z "${OLLAMA_WINDOWS_URL:-}" ] || [ -z "${OLLAMA_WINDOWS_SHA256:-}" ]; then
    echo "OLLAMA_WINDOWS_URL ou OLLAMA_WINDOWS_SHA256 absent de $env_file." >&2
    return 1
  fi

  work="$tools/runtimes/ollama-windows-$OLLAMA_VERSION.part"
  rm -rf "$work"
  mkdir -p "$work/x"
  trap 'rm -rf "$work"' EXIT

  local archive="$tools/ollama-dist/v$OLLAMA_VERSION/ollama-windows-amd64.zip"
  if [ ! -f "$archive" ]; then
    archive="$work/ollama-windows-amd64.zip"
    curl -sSfL --retry 3 -o "$archive" "$OLLAMA_WINDOWS_URL"
  fi
  if ! echo "$OLLAMA_WINDOWS_SHA256  $archive" | sha256sum -c --status -; then
    echo "Empreinte inattendue pour Ollama ($archive, attendu : $OLLAMA_WINDOWS_URL)." >&2
    return 1
  fi

  # Archive vérifiée : on n'en extrait que ce qui est livré (sans les moteurs
  # CUDA, 1,7 Go qui exigent les pilotes propriétaires).
  # Deux motifs : l'unzip de Git pour Windows arrête « * » aux « / » (le
  # premier motif n'y exclut rien), celui de Linux non. Les dossiers vides
  # qu'il crée quand même sont retirés ensuite.
  unzip -q "$archive" -d "$work/x" -x 'lib/ollama/cuda_*' 'lib/ollama/cuda_*/*' || {
    # 1 : avertissement (motif d'exclusion sans correspondance), sans gravité.
    [ $? -eq 1 ] || return 1
  }
  local x="$work/x"
  find "$x/lib/ollama" -mindepth 1 -maxdepth 1 -type d -name 'cuda_*' -empty -delete
  local lib="$x/lib/ollama"
  if [ ! -f "$x/ollama.exe" ] || [ ! -d "$lib" ]; then
    echo "Contenu inattendu de l'archive : ollama.exe ou lib/ollama absent." >&2
    return 1
  fi

  # Contenu attendu : un dossier inconnu (nouveau moteur d'une autre version)
  # doit être examiné avant d'être livré ou exclu.
  local unexpected
  unexpected="$(find "$lib" -mindepth 1 -type d ! -path "$lib/vulkan" -printf '%P\n')"
  if [ -n "$unexpected" ]; then
    echo "Dossiers inattendus dans lib/ollama : $unexpected" >&2
    return 1
  fi

  # Licences à part ; celles des bibliothèques de CUDA ne sont pas livrées.
  mkdir -p "$x/licenses"
  find "$lib" -maxdepth 1 -type f \( -name '*_LICENSE' -o -name '*_LICENSE.*' \
    -o -name '*_NOTICE' -o -name '*_NOTICE.*' -o -name '*_COPYING' -o -name '*_COPYING.*' \) \
    -exec mv -t "$x/licenses" {} +
  rm -f "$x/licenses"/CUDNN_* "$x/licenses"/CUTLASS_* "$x/licenses"/NVTX*_*

  # Moteurs exclus rangés hors de leurs dossiers : refusés, à examiner. Même
  # motif que sous Linux.
  local excluded_re='cuda|cublas|cudart|cudnn|nvrtc|nvidia|rocm|rocblas|hipblas|amdhip|(^|[/_.-])hip([_.-]|$)|mlx'
  local excluded
  excluded="$(cd "$x" && { find ollama.exe lib -mindepth 1 -printf '%p\n' | grep -Ei "$excluded_re" || true; })"
  if [ -n "$excluded" ]; then
    echo "Fichiers CUDA, ROCm ou MLX dans l'archive : ${excluded//$'\n'/ }" >&2
    return 1
  fi

  # Bibliothèques importées qui ne sont pas dans l'archive : celles de
  # Windows et vulkan-1.dll (chargeur Vulkan du pilote graphique) sont
  # attendues. Affiché pour relecture, sans échec (liste du système ouverte).
  if command -v objdump >/dev/null; then
    local shipped imported missing
    shipped="$(find "$x" -type f \( -iname '*.dll' -o -iname '*.exe' \) -printf '%f\n' | tr '[:upper:]' '[:lower:]' | sort -u)"
    imported="$({ find "$x" -type f \( -iname '*.dll' -o -iname '*.exe' \) -exec objdump -p {} + 2>/dev/null || true; } |
      sed -n 's/^[[:space:]]*DLL Name: //p' | tr -d '\r' | tr '[:upper:]' '[:lower:]' | sort -u)"
    missing="$(comm -13 <(echo "$shipped") <(echo "$imported") | grep -Ev '^(api-ms-win-|ext-ms-|$)' | tr '\n' ' ' || true)"
    echo "DLL importées hors de l'archive (système attendu) : ${missing:-aucune}" >&2
  fi

  # Version annoncée par le binaire, sans contacter aucun serveur (adresse
  # fermée) ni écrire dans le vrai dossier de l'utilisateur.
  if $host_windows; then
    mkdir -p "$work/home"
    local home="$work/home"
    command -v cygpath >/dev/null && home="$(cygpath -w "$home")"
    local version_out
    version_out="$(USERPROFILE="$home" HOME="$home" OLLAMA_HOST=127.0.0.1:1 "$x/ollama.exe" --version 2>&1 | tr -d '\r' || true)"
    if ! grep -q "client version is $OLLAMA_VERSION\$" <<<"$version_out"; then
      echo "ollama.exe --version inattendu : $version_out" >&2
      return 1
    fi
  fi

  (cd "$x" && find ollama.exe lib -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum) >"$x/SHA256SUMS"
  echo "$OLLAMA_VERSION" >"$x/VERSION"

  rm -rf "$dest"
  mkdir -p "$(dirname "$dest")"
  mv "$x" "$dest"
  echo "$binary"
}

if [ "$target" = windows ]; then
  fetch_windows
  exit $?
fi

dest="$tools/runtimes/ollama"
binary="$dest/bin/ollama"
if [ -x "$binary" ] && [ -f "$dest/SHA256SUMS" ] &&
  [ "$(cat "$dest/VERSION" 2>/dev/null)" = "$OLLAMA_VERSION" ]; then
  echo "$binary"
  exit 0
fi

command -v zstd >/dev/null || {
  echo "zstd introuvable : il sert à extraire l'archive d'Ollama." >&2
  exit 1
}

work="$tools/runtimes/ollama-$OLLAMA_VERSION.part"
rm -rf "$work"
mkdir -p "$work/x"
trap 'rm -rf "$work"' EXIT

archive="$tools/ollama-dist/v$OLLAMA_VERSION/ollama-linux-amd64.tar.zst"
if [ ! -f "$archive" ]; then
  archive="$work/ollama-linux-amd64.tar.zst"
  curl -sSfL --retry 3 -o "$archive" "$OLLAMA_URL"
fi
if ! echo "$OLLAMA_SHA256  $archive" | sha256sum -c --status -; then
  echo "Empreinte inattendue pour Ollama ($archive, attendu : $OLLAMA_URL)." >&2
  exit 1
fi

# Archive vérifiée : on n'en extrait que ce qui est livré (sans les moteurs
# CUDA, ROCm et MLX, 2 Go qui exigent les pilotes propriétaires).
tar --zstd -xf "$archive" -C "$work/x" \
  --exclude='lib/ollama/cuda_*' --exclude='lib/ollama/rocm*' --exclude='lib/ollama/mlx*' \
  bin/ollama lib/ollama
x="$work/x"
lib="$x/lib/ollama"

# Contenu attendu : un dossier inconnu (nouveau moteur d'une autre version)
# doit être examiné avant d'être livré ou exclu.
unexpected="$(find "$lib" -mindepth 1 -type d ! -path "$lib/vulkan" -printf '%P\n')"
if [ -n "$unexpected" ]; then
  echo "Dossiers inattendus dans lib/ollama : $unexpected" >&2
  exit 1
fi

is_elf() {
  [ "$(head -c 4 "$1" | od -An -tx1 | tr -d ' \n')" = 7f454c46 ]
}

# Liens symboliques remplacés par les fichiers sous le nom de leur SONAME (le
# nom que demandent les autres fichiers, DT_NEEDED) ; les noms de
# développement (libggml.so) et de version complète (libggml.so.0.25.3)
# disparaissent.
find "$lib" -type l -delete
while IFS= read -r -d '' file; do
  is_elf "$file" || continue
  soname="$(LC_ALL=C readelf -d "$file" | sed -n 's/.*(SONAME).*\[\(.*\)\]/\1/p')"
  if [ -n "$soname" ] && [ "$soname" != "$(basename "$file")" ]; then
    mv "$file" "$(dirname "$file")/$soname"
  fi
done < <(find "$lib" -type f -print0)

# Licences à part (livrées dans /usr/share/doc/chatonlm/licenses/ollama).
mkdir -p "$x/licenses"
find "$lib" -maxdepth 1 -type f \( -name '*_LICENSE' -o -name '*_NOTICE' \) \
  -exec mv -t "$x/licenses" {} +

# Moteurs exclus rangés hors de leurs dossiers (premier niveau de
# lib/ollama, comme dans d'anciennes archives : libggml-cuda.so, libcudart,
# libggml-hip.so…) : refusés, à examiner avant d'être livrés ou exclus. Même
# motif que EXCLUDED_ENGINES de scripts/inspect-packages.mjs.
excluded_re='cuda|cublas|cudart|cudnn|nvrtc|nvidia|rocm|rocblas|hipblas|amdhip|(^|[/_.-])hip([_.-]|$)|mlx'
excluded="$(cd "$x" && { find bin lib -mindepth 1 -printf '%p\n' | grep -Ei "$excluded_re" || true; })"
if [ -n "$excluded" ]; then
  echo "Fichiers CUDA, ROCm ou MLX dans l'archive : ${excluded//$'\n'/ }" >&2
  exit 1
fi

# Droits : exécutables (serveur, runner) 755, le reste 644.
while IFS= read -r -d '' file; do
  case "$file" in
    "$x/bin/ollama" | "$lib"/llama-*) chmod 0755 "$file" ;;
    *) chmod 0644 "$file" ;;
  esac
done < <(find "$x/bin" "$lib" "$x/licenses" -type f -print0)

# Dépendances de chaque fichier ELF : les bibliothèques d'Ollama sont trouvées
# par leur RUNPATH ($ORIGIN) ou par le dossier que le serveur donne au runner ;
# LD_LIBRARY_PATH=lib/ollama reproduit ce second cas.
missing=""
while IFS= read -r -d '' file; do
  is_elf "$file" || continue
  out="$(LC_ALL=C LD_LIBRARY_PATH="$lib" ldd "$file" 2>&1 || true)"
  while IFS= read -r soname; do
    [ "$soname" = libvulkan.so.1 ] && continue
    missing+="  ${file#"$x"/} : $soname"$'\n'
  done < <(sed -n 's/^[[:space:]]*\(\S*\) => not found.*/\1/p' <<<"$out")
done < <(find "$x/bin" "$lib" -type f -print0)
if [ -n "$missing" ]; then
  printf 'Bibliothèques introuvables :\n%s' "$missing" >&2
  exit 1
fi

# libgomp (GCC) : la notice cite la version de GCC et la construction
# (paquet d'AlmaLinux) dont il vient, avec le paquet source correspondant.
if ! grep -aq "running gcc $OLLAMA_LIBGOMP_GCC " "$lib/libgomp.so.1" ||
  ! grep -aqF "libgomp.so.1.0.0-$OLLAMA_LIBGOMP_BUILD.x86_64.debug" "$lib/libgomp.so.1"; then
  echo "libgomp.so.1 ne vient plus de GCC $OLLAMA_LIBGOMP_GCC" \
    "(construction $OLLAMA_LIBGOMP_BUILD) : mettre à jour OLLAMA_LIBGOMP_GCC," \
    "OLLAMA_LIBGOMP_BUILD et OLLAMA_LIBGOMP_SOURCE (packaging/ollama.env) et" \
    "sa notice." >&2
  exit 1
fi

# Version annoncée par le binaire, sans contacter aucun serveur (adresse
# fermée) ni écrire dans le vrai HOME.
mkdir -p "$work/home"
version_out="$(HOME="$work/home" OLLAMA_HOST=127.0.0.1:1 "$x/bin/ollama" --version 2>&1 || true)"
if ! grep -q "client version is $OLLAMA_VERSION\$" <<<"$version_out"; then
  echo "ollama --version inattendu : $version_out" >&2
  exit 1
fi

(cd "$x" && find bin lib -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum) >"$x/SHA256SUMS"
echo "$OLLAMA_VERSION" >"$x/VERSION"

rm -rf "$dest"
mkdir -p "$(dirname "$dest")"
mv "$x" "$dest"
echo "$binary"
