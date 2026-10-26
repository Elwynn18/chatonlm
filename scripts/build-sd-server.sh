#!/usr/bin/env bash
# Moteur d'images intégré aux paquets (packaging/sdcpp.env) : sd-server de
# stable-diffusion.cpp construit depuis les sources du tag épinglé (commit et
# sous-module ggml vérifiés), correctifs de packaging/sdcpp/*.patch appliqués,
# dans .tools/runtimes/sd-server :
#   sd-server[.exe]     le serveur (Vulkan, bascule seule sur le processeur
#                       sans carte graphique ; sans OpenMP ni WebP ; CRT
#                       statique sous Windows), sans symboles de débogage
#   fallback/           chargeur Vulkan de secours (libvulkan.so.1 ou
#                       vulkan-1.dll) construit depuis Vulkan-Loader de Khronos
#                       au tag épinglé, pris par l'app seulement si le système
#                       n'en a pas (image_gen/linux.rs, image_gen/windows.rs)
#   VERSION             <version épinglée>+chatonlm.<empreinte> : l'empreinte
#                       couvre sdcpp.env, les correctifs et ce script (toute
#                       modification reconstruit)
#   SHA256SUMS          empreintes des fichiers (comparées à celles des
#                       paquets par scripts/inspect-packages.mjs)
# Rien n'est refait si VERSION et SHA256SUMS correspondent. Contrôles :
# dépendances (Linux : ldd, seule libvulkan.so.1 peut manquer, jamais
# libgomp ; Windows : DLL importées, ni CRT de Visual C++ ni OpenMP),
# « sd-server --version » et « --list-devices » avec le chargeur de secours.
# Une construction à la fois (verrou .tools/sdcpp-build.lock).
# Outils requis : git, cmake, un compilateur C++ (Visual Studio 2022 sous
# Windows), glslc et les en-têtes Vulkan et SPIR-V (CI : SDK de LunarG, voir
# packaging/sdcpp.env ; Arch : vulkan-headers, spirv-headers, shaderc).
# Variable : SDCPP_JOBS (parallélisme, 4 par défaut).
# Affiche le chemin du binaire. --check : code 0 si le dossier est à jour, 1
# sinon, sans rien construire (la CI n'installe le SDK Vulkan qu'au besoin).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
env_file="$root/packaging/sdcpp.env"
patches_dir="$root/packaging/sdcpp"
# shellcheck source=../packaging/sdcpp.env
source "$env_file"

windows=false
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) windows=true ;;
esac

out="$root/.tools/runtimes/sd-server"
work="$root/.tools/sdcpp-build"
exe=sd-server
loader=libvulkan.so.1
if $windows; then
  exe=sd-server.exe
  loader=vulkan-1.dll
fi

sha256() {
  sha256sum "$@"
}

# Identité de la construction : version épinglée, correctifs et ce script.
fingerprint="$(cat "$env_file" "$patches_dir"/*.patch "${BASH_SOURCE[0]}" | sha256 | cut -c1-12)"
version="$SDCPP_VERSION+chatonlm.$fingerprint"

if [ -f "$out/$exe" ] && [ "$(cat "$out/VERSION" 2>/dev/null)" = "$version" ] &&
  (cd "$out" && sha256 --check --quiet --strict SHA256SUMS >/dev/null 2>&1); then
  echo "$out/$exe"
  exit 0
fi
if [ "${1:-}" = "--check" ]; then
  echo "sd-server à construire ($version)" >&2
  exit 1
fi

fail() {
  echo "build-sd-server : $*" >&2
  exit 1
}

for tool in git cmake; do
  command -v "$tool" >/dev/null || fail "outil manquant : $tool"
done
command -v glslc >/dev/null || fail "glslc manquant (SDK Vulkan, ou paquet shaderc)"

# Une construction à la fois : deux lancements (check.sh et package.sh)
# videraient le même dossier de travail. .tools/ peut manquer (machine de CI
# neuve, sans cache) : il est créé avant, sinon l'échec passerait pour un
# verrou déjà pris.
lock="$root/.tools/sdcpp-build.lock"
mkdir -p "$root/.tools"
mkdir "$lock" 2>/dev/null || fail "une autre construction est en cours (sinon supprimer $lock)"
trap 'rm -rf "$lock"' EXIT

# Sources du tag épinglé, vérifiées, puis correctifs.
src="$work/src"
build="$work/build"
rm -rf "$work"
mkdir -p "$work"
git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$SDCPP_VERSION" "$SDCPP_REPO" "$src"
[ "$(git -C "$src" rev-parse HEAD)" = "$SDCPP_COMMIT" ] ||
  fail "le tag $SDCPP_VERSION ne désigne plus le commit $SDCPP_COMMIT"
[ "$(git -C "$src" rev-parse HEAD:ggml)" = "$SDCPP_GGML_COMMIT" ] ||
  fail "le sous-module ggml n'est pas au commit attendu"
git -C "$src" submodule update --quiet --init --depth 1 ggml
[ "$(git -C "$src/ggml" rev-parse HEAD)" = "$SDCPP_GGML_COMMIT" ] ||
  fail "ggml récupéré à un autre commit"
for patch in "$patches_dir"/*.patch; do
  git -C "$src" apply --whitespace=nowarn "$patch" || fail "correctif refusé : $(basename "$patch")"
done

options=(
  -DCMAKE_BUILD_TYPE=Release
  -DSD_VULKAN=ON
  -DGGML_NATIVE=OFF
  -DGGML_OPENMP=OFF
  -DSD_WEBP=OFF
  -DSD_WEBM=OFF
  -DSD_SERVER_BUILD_FRONTEND=OFF
  -DSD_BUILD_SHARED_LIBS=OFF
)
if $windows; then
  # CRT de Visual C++ dans l'exécutable : aucune DLL à livrer à côté (un
  # processus enfant ne cherche pas dans le dossier de chatonlm.exe).
  options+=(
    -A x64
    -DCMAKE_POLICY_DEFAULT_CMP0091=NEW
    "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded"
  )
fi
cmake -S "$src" -B "$build" "${options[@]}" >"$work/configure.log" 2>&1 ||
  { tail -40 "$work/configure.log" >&2; fail "configuration de CMake en échec"; }
cmake --build "$build" --config Release --target sd-server -j "${SDCPP_JOBS:-4}" \
  >"$work/build.log" 2>&1 ||
  { tail -40 "$work/build.log" >&2; fail "construction en échec"; }

built="$(find "$build/bin" -name "$exe" -type f -print -quit)"
[ -n "$built" ] || fail "binaire introuvable après la construction"

# Chargeur Vulkan de secours (Apache-2.0) : Vulkan-Loader de Khronos au tag
# épinglé, avec Vulkan-Headers au même tag ; sans les extensions de fenêtrage
# (sd-server n'affiche rien). Même source sous Linux et Windows.
fetch_tag() {
  local repo="$1" tag="$2" commit="$3" dir="$4"
  git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$tag" "$repo" "$dir"
  [ "$(git -C "$dir" rev-parse HEAD)" = "$commit" ] ||
    fail "le tag $tag de $repo ne désigne plus le commit $commit"
}
fetch_tag "$VULKAN_HEADERS_REPO" "$VULKAN_LOADER_TAG" "$VULKAN_HEADERS_COMMIT" "$work/vulkan-headers"
fetch_tag "$VULKAN_LOADER_REPO" "$VULKAN_LOADER_TAG" "$VULKAN_LOADER_COMMIT" "$work/vulkan-loader"
cmake -S "$work/vulkan-headers" -B "$work/vulkan-headers-build" \
  -DCMAKE_INSTALL_PREFIX="$work/vulkan-headers-install" >"$work/loader.log" 2>&1 &&
  cmake --install "$work/vulkan-headers-build" >>"$work/loader.log" 2>&1 ||
  { tail -40 "$work/loader.log" >&2; fail "en-têtes Vulkan en échec"; }
loader_options=(
  -DCMAKE_BUILD_TYPE=Release
  -DVULKAN_HEADERS_INSTALL_DIR="$work/vulkan-headers-install"
  -DCMAKE_PREFIX_PATH="$work/vulkan-headers-install"
  -DBUILD_TESTS=OFF
  -DUPDATE_DEPS=OFF
)
if $windows; then
  loader_options+=(-A x64 -DCMAKE_POLICY_DEFAULT_CMP0091=NEW "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded")
else
  loader_options+=(-DBUILD_WSI_XCB_SUPPORT=OFF -DBUILD_WSI_XLIB_SUPPORT=OFF -DBUILD_WSI_WAYLAND_SUPPORT=OFF)
fi
cmake -S "$work/vulkan-loader" -B "$work/vulkan-loader-build" "${loader_options[@]}" >>"$work/loader.log" 2>&1 &&
  cmake --build "$work/vulkan-loader-build" --config Release -j "${SDCPP_JOBS:-4}" >>"$work/loader.log" 2>&1 ||
  { tail -40 "$work/loader.log" >&2; fail "chargeur Vulkan en échec"; }
loader_source="$(find "$work/vulkan-loader-build" -type f \( -name "$loader" -o -name 'libvulkan.so.1.*' \) -print -quit)"
[ -n "$loader_source" ] || fail "chargeur Vulkan introuvable après la construction"

stage="$out.part"
rm -rf "$stage"
trap 'rm -rf "$stage" "$lock"' EXIT
mkdir -p "$stage/fallback"
cp "$built" "$stage/$exe"
cp -L "$loader_source" "$stage/fallback/$loader"
if ! $windows; then
  strip --strip-unneeded "$stage/$exe"
  strip --strip-unneeded "$stage/fallback/$loader"
fi
chmod 0755 "$stage/$exe"
chmod 0644 "$stage/fallback/$loader"

# Dépendances : rien d'autre que le système (et Vulkan, chargé par l'app ou
# pris dans fallback/).
if $windows; then
  imports="$(objdump -p "$stage/$exe" | awk '/DLL Name:/ { print tolower($3) }')"
  # Une liste vide ne prouverait rien (objdump qui ne lit pas le fichier).
  grep -qx 'kernel32.dll' <<<"$imports" || fail "DLL importées illisibles (objdump)"
  forbidden="$(grep -E '^(msvcp|vcruntime|vcomp|concrt|libgomp|libstdc|libgcc|api-ms-win-crt)' <<<"$imports" || true)"
  [ -z "$forbidden" ] || fail "DLL à ne pas exiger : $(tr '\n' ' ' <<<"$forbidden")"
else
  missing="$(env -u LD_LIBRARY_PATH ldd "$stage/$exe" | awk '/not found/ { print $1 }' | grep -v '^libvulkan\.so\.1$' || true)"
  [ -z "$missing" ] || fail "bibliothèques introuvables : $(tr '\n' ' ' <<<"$missing")"
  if ldd "$stage/$exe" | grep libgomp >/dev/null; then
    fail "sd-server dépend de libgomp (OpenMP devait être coupé)"
  fi
fi
# Avec le chargeur de secours, comme l'app sur une machine sans chargeur
# (Linux : LD_LIBRARY_PATH ; Windows : fin de PATH).
with_fallback() {
  if $windows; then
    PATH="$PATH:$stage/fallback" "$@"
  else
    LD_LIBRARY_PATH="$stage/fallback" "$@"
  fi
}
reported="$(with_fallback "$stage/$exe" --version 2>&1 || true)"
reported="${reported%%$'\n'*}"
case "$reported" in
  *"$SDCPP_VERSION"*) ;;
  *) fail "version inattendue : $reported" ;;
esac
devices="$(with_fallback "$stage/$exe" --list-devices 2>&1 || true)"
grep -q '^CPU' <<<"$devices" || fail "aucun moteur de calcul listé : $devices"

printf '%s\n' "$version" >"$stage/VERSION"
chmod 0644 "$stage/VERSION"
(cd "$stage" && find . -type f ! -name SHA256SUMS | LC_ALL=C sort | sed 's|^\./||' |
  while IFS= read -r file; do sha256 "$file"; done) >"$stage/SHA256SUMS"

rm -rf "$out"
mv "$stage" "$out"
rm -rf "$lock"
trap - EXIT
echo "$out/$exe"
