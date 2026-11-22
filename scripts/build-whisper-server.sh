#!/usr/bin/env bash
# Moteur de transcription intégré aux paquets (packaging/whispercpp.env) :
# whisper-server de whisper.cpp construit depuis les sources de la version
# épinglée (commit vérifié), correctifs de packaging/whispercpp/*.patch
# appliqués, dans .tools/runtimes/whisper-server :
#   whisper-server[.exe]  le serveur, et à côté ses bibliothèques (whisper,
#                         ggml, ggml-base) et les moteurs de calcul chargés à
#                         l'exécution : une variante ggml-cpu-<jeu> par jeu
#                         d'instructions (la meilleure est choisie au
#                         lancement, processeurs sans AVX2 compris) et
#                         ggml-vulkan. Sans OpenMP, sans curl, sans ffmpeg
#                         ni SDL ; sous Windows, CRT de Visual C++ en DLL
#                         (/MD, comme llama-server), ses DLL copiées à côté ;
#                         RUNPATH $ORIGIN sous Linux (le dossier se déplace
#                         tel quel) ; sans symboles de débogage
#   VERSION               <version épinglée>+chatonlm.<empreinte> :
#                         l'empreinte couvre whispercpp.env, les épingles
#                         Vulkan de sdcpp.env, les correctifs et ce script
#                         (toute modification reconstruit)
#   SHA256SUMS            empreintes des fichiers (comparées à celles des
#                         paquets par scripts/inspect-packages.mjs)
# Pas de fallback/ : l'app donne à whisper-server le chargeur Vulkan de
# secours de llama-server/fallback/ (le même fichier que celui de sd-server,
# repris ici de .tools/runtimes/sd-server/fallback pour les contrôles
# seulement).
# Rien n'est refait si VERSION et SHA256SUMS correspondent. Contrôles :
# dépendances (Linux : ldd, rien hors de glibc, libstdc++, libgcc et
# libvulkan.so.1 ; Windows : DLL importées, ni OpenMP ni runtime de GCC ni
# curl, et chaque DLL du runtime Visual C++ demandée livrée à côté), et
# « whisper-server --help » avec le chargeur de secours : code 0, moteur du
# processeur chargé, options du serveur affichées.
# Une construction à la fois (verrou .tools/whisper-build.lock).
# Outils requis : git, cmake, un compilateur C++ (Visual Studio 2022 sous
# Windows), glslc et les en-têtes Vulkan et SPIR-V (CI : SDK de LunarG, voir
# packaging/sdcpp.env ; Arch : vulkan-headers, spirv-headers, shaderc).
# Variable : WHISPER_JOBS (parallélisme, 4 par défaut).
# Affiche le chemin du binaire. --check : code 0 si le dossier est à jour, 1
# sinon, sans rien construire (la CI n'installe le SDK Vulkan qu'au besoin).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
env_file="$root/packaging/whispercpp.env"
sd_env_file="$root/packaging/sdcpp.env"
patches_dir="$root/packaging/whispercpp"
# shellcheck source=../packaging/whispercpp.env
source "$env_file"
# Épingles du chargeur Vulkan et du SDK (communes avec sd-server).
vulkan_pins="$(grep '^VULKAN_' "$sd_env_file")"

windows=false
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) windows=true ;;
esac

out="$root/.tools/runtimes/whisper-server"
work="$root/.tools/whisper-build"
exe=whisper-server
loader=libvulkan.so.1
if $windows; then
  exe=whisper-server.exe
  loader=vulkan-1.dll
fi

sha256() {
  sha256sum "$@"
}

shopt -s nullglob
patches=("$patches_dir"/*.patch)
shopt -u nullglob

# Identité de la construction : version épinglée, outils Vulkan, correctifs
# et ce script.
fingerprint="$({
  cat "$env_file"
  printf '%s\n' "$vulkan_pins"
  [ "${#patches[@]}" -eq 0 ] || cat "${patches[@]}"
  cat "${BASH_SOURCE[0]}"
} | sha256 | cut -c1-12)"
version="$WHISPER_VERSION+chatonlm.$fingerprint"

if [ -f "$out/$exe" ] && [ "$(cat "$out/VERSION" 2>/dev/null)" = "$version" ] &&
  (cd "$out" && sha256 --check --quiet --strict SHA256SUMS >/dev/null 2>&1); then
  echo "$out/$exe"
  exit 0
fi
if [ "${1:-}" = "--check" ]; then
  echo "whisper-server à construire ($version)" >&2
  exit 1
fi

fail() {
  echo "build-whisper-server : $*" >&2
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
lock="$root/.tools/whisper-build.lock"
mkdir -p "$root/.tools"
mkdir "$lock" 2>/dev/null || fail "une autre construction est en cours (sinon supprimer $lock)"
trap 'rm -rf "$lock"' EXIT

# Chargeur Vulkan de secours, pour les contrôles : celui de sd-server (mêmes
# épingles, même fichier que llama-server/fallback/), construit s'il manque
# ou n'est plus à jour.
sd_out="$("$root/scripts/build-sd-server.sh")" || fail "sd-server (chargeur Vulkan de secours) en échec"
loader_dir="$(dirname "$sd_out")/fallback"
[ -f "$loader_dir/$loader" ] || fail "chargeur Vulkan de secours absent : $loader_dir/$loader"

# Sources du tag épinglé, vérifiées, puis correctifs.
src="$work/src"
build="$work/build"
rm -rf "$work"
mkdir -p "$work"
git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$WHISPER_VERSION" "$WHISPER_REPO" "$src"
[ "$(git -C "$src" rev-parse HEAD)" = "$WHISPER_COMMIT" ] ||
  fail "le tag $WHISPER_VERSION ne désigne plus le commit $WHISPER_COMMIT"
for patch in "${patches[@]}"; do
  git -C "$src" apply --whitespace=nowarn "$patch" || fail "correctif refusé : $(basename "$patch")"
done

options=(
  -DCMAKE_BUILD_TYPE=Release
  -DBUILD_SHARED_LIBS=ON
  # Version sans « -dev » (whisper_version()).
  -DWHISPER_BUILD_IS_DEV=OFF
  # Moteurs de calcul chargés à l'exécution, une variante par jeu
  # d'instructions du processeur, Vulkan pour les cartes graphiques.
  -DGGML_BACKEND_DL=ON
  -DGGML_CPU_ALL_VARIANTS=ON
  -DGGML_NATIVE=OFF
  -DGGML_VULKAN=ON
  # Ni libgomp (comme sd-server et llama-server), ni curl (l'app télécharge
  # les modèles), ni ffmpeg (l'app envoie de l'audio déjà décodé), ni SDL
  # (pas de micro : l'app enregistre elle-même).
  -DGGML_OPENMP=OFF
  -DWHISPER_CURL=OFF
  -DWHISPER_SDL2=OFF
  -DWHISPER_BUILD_TESTS=OFF
  -DWHISPER_BUILD_EXAMPLES=ON
  -DWHISPER_BUILD_SERVER=ON
)
if $windows; then
  # CRT de Visual C++ en DLL (/MD, celui de CMake par défaut), comme
  # llama-server et pour la même raison : avec un CRT statique, chaque DLL
  # (whisper, ggml, ggml-base, moteurs de calcul) aurait le sien, et un objet
  # passé de l'une à l'autre arrêterait le serveur. Ses DLL sont copiées à
  # côté du serveur : un processus enfant ne cherche pas dans le dossier de
  # chatonlm.exe.
  options+=(-A x64)
else
  options+=(
    # Option propre à Linux dans whisper.cpp.
    -DWHISPER_COMMON_FFMPEG=OFF
    # Bibliothèques trouvées à côté de l'exécutable, où qu'il soit installé.
    "-DCMAKE_INSTALL_RPATH=\$ORIGIN"
    -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON
  )
fi
cmake -S "$src" -B "$build" "${options[@]}" >"$work/configure.log" 2>&1 ||
  { tail -40 "$work/configure.log" >&2; fail "configuration de CMake en échec"; }
# Les moteurs de calcul (MODULE) sont des dépendances de la cible ggml : ils
# sont construits avec whisper-server.
cmake --build "$build" --config Release --target whisper-server -j "${WHISPER_JOBS:-4}" \
  >"$work/build.log" 2>&1 ||
  { tail -40 "$work/build.log" >&2; fail "construction en échec"; }

built="$(find "$build/bin" -name "$exe" -type f -print -quit)"
[ -n "$built" ] || fail "binaire introuvable après la construction"
bindir="$(dirname "$built")"

# Bibliothèques demandées par un fichier (noms tels qu'écrits dans le fichier).
needed() {
  if $windows; then
    objdump -p "$1" | awk '/DLL Name:/ { print $3 }'
  else
    readelf -d "$1" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'
  fi
}
# Windows : dossier des DLL redistribuables du runtime Visual C++ du Visual
# Studio de la machine (VCToolsRedistDir d'une invite de Visual Studio,
# sinon vswhere), version la plus récente.
crt_dir=""
if $windows; then
  if [ -n "${VCToolsRedistDir:-}" ]; then
    redist="$(cygpath -u "$VCToolsRedistDir")"
  else
    vswhere="/c/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe"
    [ -x "$vswhere" ] || fail "vswhere introuvable (runtime Visual C++ à livrer)"
    installation="$("$vswhere" -latest -products '*' -property installationPath | tr -d '\r')"
    [ -n "$installation" ] || fail "Visual Studio introuvable (vswhere)"
    redist="$(ls -d "$(cygpath -u "$installation")"/VC/Redist/MSVC/14.* 2>/dev/null | sort -V | tail -1)"
  fi
  crt_dir="$(ls -d "$redist"/x64/Microsoft.VC14*.CRT 2>/dev/null | sort -V | tail -1)"
  [ -n "$crt_dir" ] && [ -f "$crt_dir/vcruntime140.dll" ] ||
    fail "DLL du runtime Visual C++ introuvables sous ${redist:-(aucun dossier)}"
fi
# Fichier de la construction qui porte ce nom (insensible à la casse sous
# Windows ; DLL du runtime Visual C++ prises dans son dossier
# redistribuable), ou rien.
in_build() {
  if $windows; then
    local found
    found="$(find "$bindir" -maxdepth 1 -type f -iname "$1" -print -quit)"
    if [ -z "$found" ] && grep -qiE '^(msvcp|vcruntime|concrt)[0-9_a-z]*\.dll$' <<<"$1"; then
      found="$(find "$crt_dir" -maxdepth 1 -type f -iname "$1" -print -quit)"
    fi
    [ -z "$found" ] || printf '%s\n' "$found"
  elif [ -e "$bindir/$1" ]; then
    printf '%s\n' "$bindir/$1"
  fi
}

stage="$out.part"
rm -rf "$stage"
trap 'rm -rf "$stage" "$lock"' EXIT
mkdir -p "$stage"

# Fermeture : le serveur et les moteurs de calcul, puis tout ce qu'ils
# demandent dans le dossier de construction (copié sous le nom demandé : un
# lien libwhisper.so.1 -> libwhisper.so.1.9.5 devient un fichier
# libwhisper.so.1).
if $windows; then
  modules=("$bindir"/ggml-cpu-*.dll "$bindir"/ggml-vulkan.dll)
else
  modules=("$bindir"/libggml-cpu-*.so "$bindir"/libggml-vulkan.so)
fi
for module in "${modules[@]}"; do
  [ -f "$module" ] || fail "moteur de calcul absent : $(basename "$module")"
done
ls "${modules[@]}" | grep -q 'cpu-haswell' || fail "variantes du processeur absentes (GGML_CPU_ALL_VARIANTS)"
declare -A seen=()
queue=()
take() {
  local key
  key="$(tr '[:upper:]' '[:lower:]' <<<"$2")"
  [ -z "${seen[$key]:-}" ] || return 0
  seen[$key]=1
  cp -L "$1" "$stage/$2"
  queue+=("$stage/$2")
}
for file in "$built" "${modules[@]}"; do
  take "$file" "$(basename "$file")"
done
while [ "${#queue[@]}" -gt 0 ]; do
  file="${queue[0]}"
  queue=("${queue[@]:1}")
  while IFS= read -r dep; do
    [ -n "$dep" ] || continue
    found="$(in_build "$dep")"
    # Sous le nom demandé, pas celui du fichier (lien de version).
    [ -z "$found" ] || take "$found" "$dep"
  done < <(needed "$file")
done

if ! $windows; then
  for file in "$stage"/*; do
    [ -f "$file" ] && strip --strip-unneeded "$file"
  done
fi
find "$stage" -type f -exec chmod 0644 {} +
chmod 0755 "$stage/$exe"

# Dépendances : rien d'autre que le système (et Vulkan, chargé par l'app ou
# pris dans llama-server/fallback/).
if $windows; then
  for file in "$stage"/*.exe "$stage"/*.dll; do
    imports="$(needed "$file" | tr '[:upper:]' '[:lower:]')"
    # Une liste vide ne prouverait rien (objdump qui ne lit pas le fichier).
    grep -qx 'kernel32.dll' <<<"$imports" || fail "DLL importées illisibles (objdump) : $(basename "$file")"
    forbidden="$(grep -E '^(vcomp|libomp|libgomp|libstdc|libgcc|libwinpthread|libssl|libcrypto|libcurl|sdl2|avcodec|avformat|avutil|swresample)' <<<"$imports" || true)"
    [ -z "$forbidden" ] || fail "$(basename "$file") : DLL à ne pas exiger : $(tr '\n' ' ' <<<"$forbidden")"
    # Runtime Visual C++ : livré à côté, jamais attendu de la machine.
    for crt in $(grep -E '^(msvcp|vcruntime|concrt)' <<<"$imports" || true); do
      [ -n "$(find "$stage" -maxdepth 1 -type f -iname "$crt" -print -quit)" ] ||
        fail "$(basename "$file") : $crt demandée mais pas livrée à côté"
    done
  done
else
  allowed='^(linux-vdso\.so\.1|ld-linux-x86-64\.so\.2|libc\.so\.6|libm\.so\.6|libstdc\+\+\.so\.6|libgcc_s\.so\.1|libpthread\.so\.0|libdl\.so\.2|librt\.so\.1|libvulkan\.so\.1)$'
  for file in "$stage"/*; do
    [ -f "$file" ] || continue
    # ldd prévient (« ldd: warning ») pour une bibliothèque sans droit
    # d'exécution : ces lignes ne sont pas des dépendances.
    deps="$(env -u LD_LIBRARY_PATH ldd "$file" 2>/dev/null)" || fail "ldd en échec : $(basename "$file")"
    missing="$(awk '/not found/ { print $1 }' <<<"$deps" | grep -v '^libvulkan\.so\.1$' || true)"
    [ -z "$missing" ] || fail "$(basename "$file") : bibliothèques introuvables : $(tr '\n' ' ' <<<"$missing")"
    # Hors du dossier livré : seulement le système de base (le chargeur
    # dynamique, sans « => », est nommé par son chemin : /lib64/… ou
    # /usr/lib64/… selon la distribution).
    # Chemins à espaces possibles (dossier du projet) : découpe sur « => » et
    # « (0x », jamais sur les espaces.
    outside="$(while IFS= read -r line; do
      line="${line#"${line%%[![:space:]]*}"}"
      case "$line" in
        *" => not found"*) ;;
        *" => "*)
          target="${line#* => }"
          target="${target% (0x*}"
          name="${line%% => *}"
          [[ "$target" == "$stage/"* ]] || printf '%s\n' "${name##*/}"
          ;;
        /*) name="${line% (0x*}" && printf '%s\n' "${name##*/}" ;;
        ?*) printf '%s\n' "${line% (0x*}" ;;
      esac
    done <<<"$deps" | grep -Ev "$allowed" || true)"
    [ -z "$outside" ] || fail "$(basename "$file") dépend de : $(tr '\n' ' ' <<<"$outside")"
  done
fi
# Avec le chargeur de secours, comme l'app sur une machine sans chargeur
# (Linux : LD_LIBRARY_PATH ; Windows : fin de PATH). Dossier courant à part :
# ggml cherche aussi ses moteurs de calcul dans le dossier courant.
with_fallback() {
  if $windows; then
    (cd "$work" && PATH="$PATH:$loader_dir" "$@")
  else
    (cd "$work" && LD_LIBRARY_PATH="$loader_dir" "$@")
  fi
}
# --help charge les moteurs de calcul (ggml_backend_load_all) avant d'afficher
# les options, puis sort avec le code 0.
status=0
help="$(with_fallback "$stage/$exe" --help 2>&1)" || status=$?
[ "$status" = 0 ] || fail "--help : code de sortie $status : $help"
grep -q 'load_backend: loaded CPU backend from .*ggml-cpu-' <<<"$help" ||
  fail "moteur du processeur non chargé : $help"
grep -q -- '--inference-path' <<<"$help" || fail "options du serveur absentes de --help : $help"

printf '%s\n' "$version" >"$stage/VERSION"
chmod 0644 "$stage/VERSION"
(cd "$stage" && find . -type f ! -name SHA256SUMS | LC_ALL=C sort | sed 's|^\./||' |
  while IFS= read -r file; do sha256 "$file"; done) >"$stage/SHA256SUMS"

rm -rf "$out"
mv "$stage" "$out"
rm -rf "$lock"
trap - EXIT
echo "$out/$exe"
