#!/usr/bin/env bash
# Moteur de texte intégré aux paquets (packaging/llama.env) : llama-server de
# llama.cpp construit depuis les sources de la version épinglée (commit
# vérifié), correctifs éventuels de packaging/llama/*.patch appliqués, dans
# .tools/runtimes/llama-server :
#   llama-server[.exe]  le lanceur, et à côté ses bibliothèques (llama,
#                       llama-common, llama-server-impl, mtmd, ggml,
#                       ggml-base) et les moteurs de calcul chargés à
#                       l'exécution : une variante ggml-cpu-<jeu> par jeu
#                       d'instructions (la meilleure est choisie au lancement,
#                       processeurs sans AVX2 compris) et ggml-vulkan. Sans
#                       OpenMP, sans OpenSSL ni curl, sans interface web ;
#                       sous Windows, CRT de Visual C++ en DLL (/MD), ses
#                       DLL copiées à côté ; RUNPATH $ORIGIN sous
#                       Linux (le dossier se déplace tel quel) ; sans
#                       symboles de débogage
#   fallback/           chargeur Vulkan de secours, le même que celui de
#                       sd-server (repris de .tools/runtimes/sd-server/fallback,
#                       construit au besoin par scripts/build-sd-server.sh) :
#                       pris par l'app seulement si le système n'en a pas
#   VERSION             <version épinglée>+chatonlm.<empreinte> : l'empreinte
#                       couvre llama.env, les épingles Vulkan de sdcpp.env, les
#                       correctifs et ce script (toute modification reconstruit)
#   SHA256SUMS          empreintes des fichiers (comparées à celles des
#                       paquets par scripts/inspect-packages.mjs)
# Rien n'est refait si VERSION et SHA256SUMS correspondent. Contrôles :
# dépendances (Linux : ldd, rien hors de glibc, libstdc++, libgcc et
# libvulkan.so.1 ; Windows : DLL importées, ni OpenMP ni runtime de GCC, et
# chaque DLL du runtime Visual C++ demandée livrée à côté),
# « llama-server --version » et « --list-devices » avec le chargeur de secours.
# Une construction à la fois (verrou .tools/llama-build.lock).
# Outils requis : git, cmake, un compilateur C++ (Visual Studio 2022 sous
# Windows), glslc et les en-têtes Vulkan et SPIR-V (CI : SDK de LunarG, voir
# packaging/sdcpp.env ; Arch : vulkan-headers, spirv-headers, shaderc).
# Variable : LLAMA_JOBS (parallélisme, 4 par défaut).
# --cuda : pack CUDA facultatif (scripts/build-llama-cuda.sh, mêmes options).
# Affiche le chemin du binaire. --check : code 0 si le dossier est à jour, 1
# sinon, sans rien construire (la CI n'installe le SDK Vulkan qu'au besoin).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Pack CUDA (moteur ggml-cuda seul, même commit) : scripts/build-llama-cuda.sh,
# à part pour que ses retouches ne changent pas l'empreinte de ce moteur-ci.
if [ "${1:-}" = "--cuda" ]; then
  shift
  exec bash "$root/scripts/build-llama-cuda.sh" "$@"
fi
env_file="$root/packaging/llama.env"
sd_env_file="$root/packaging/sdcpp.env"
patches_dir="$root/packaging/llama"
# shellcheck source=../packaging/llama.env
source "$env_file"
# Épingles du chargeur Vulkan et du SDK (communes avec sd-server).
vulkan_pins="$(grep '^VULKAN_' "$sd_env_file")"

windows=false
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) windows=true ;;
esac

out="$root/.tools/runtimes/llama-server"
work="$root/.tools/llama-build"
exe=llama-server
loader=libvulkan.so.1
if $windows; then
  exe=llama-server.exe
  loader=vulkan-1.dll
fi

sha256() {
  sha256sum "$@"
}

shopt -s nullglob
patches=("$patches_dir"/*.patch)
shopt -u nullglob

# Identité de la construction : version épinglée, chargeur Vulkan, correctifs
# et ce script.
fingerprint="$({
  cat "$env_file"
  printf '%s\n' "$vulkan_pins"
  [ "${#patches[@]}" -eq 0 ] || cat "${patches[@]}"
  cat "${BASH_SOURCE[0]}"
} | sha256 | cut -c1-12)"
version="$LLAMA_VERSION+chatonlm.$fingerprint"

if [ -f "$out/$exe" ] && [ "$(cat "$out/VERSION" 2>/dev/null)" = "$version" ] &&
  (cd "$out" && sha256 --check --quiet --strict SHA256SUMS >/dev/null 2>&1); then
  echo "$out/$exe"
  exit 0
fi
if [ "${1:-}" = "--check" ]; then
  echo "llama-server à construire ($version)" >&2
  exit 1
fi

fail() {
  echo "build-llama-server : $*" >&2
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
lock="$root/.tools/llama-build.lock"
mkdir -p "$root/.tools"
mkdir "$lock" 2>/dev/null || fail "une autre construction est en cours (sinon supprimer $lock)"
trap 'rm -rf "$lock"' EXIT

# Chargeur Vulkan de secours : celui de sd-server (mêmes épingles), construit
# s'il manque ou n'est plus à jour.
sd_out="$("$root/scripts/build-sd-server.sh")" || fail "sd-server (chargeur Vulkan de secours) en échec"
loader_source="$(dirname "$sd_out")/fallback/$loader"
[ -f "$loader_source" ] || fail "chargeur Vulkan de secours absent : $loader_source"

# Sources du tag épinglé, vérifiées, puis correctifs.
src="$work/src"
build="$work/build"
rm -rf "$work"
mkdir -p "$work"
git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$LLAMA_BUILD" "$LLAMA_REPO" "$src"
[ "$(git -C "$src" rev-parse HEAD)" = "$LLAMA_COMMIT" ] ||
  fail "le tag $LLAMA_BUILD ne désigne plus le commit $LLAMA_COMMIT"
for patch in "${patches[@]}"; do
  git -C "$src" apply --whitespace=nowarn "$patch" || fail "correctif refusé : $(basename "$patch")"
done

# Numéro et commit de build donnés à CMake : le clone superficiel compterait
# un seul commit (« build 1 »).
short_commit="${LLAMA_COMMIT:0:9}"
options=(
  -DCMAKE_BUILD_TYPE=Release
  -DBUILD_SHARED_LIBS=ON
  -DLLAMA_BUILD_IS_DEV=OFF
  "-DLLAMA_BUILD_NUMBER=${LLAMA_BUILD#b}"
  "-DLLAMA_BUILD_COMMIT=$short_commit"
  # Moteurs de calcul chargés à l'exécution, une variante par jeu
  # d'instructions du processeur, Vulkan pour les cartes graphiques.
  -DGGML_BACKEND_DL=ON
  -DGGML_CPU_ALL_VARIANTS=ON
  -DGGML_NATIVE=OFF
  -DGGML_VULKAN=ON
  # Ni libgomp (comme sd-server) ni OpenSSL : l'app télécharge les modèles,
  # le serveur n'a aucune connexion sortante à faire.
  -DGGML_OPENMP=OFF
  -DLLAMA_OPENSSL=OFF
  -DLLAMA_BUILD_UI=OFF
  -DLLAMA_USE_PREBUILT_UI=OFF
  -DLLAMA_CONNECT=OFF
  -DLLAMA_LLGUIDANCE=OFF
  -DLLAMA_BUILD_TESTS=OFF
  -DLLAMA_BUILD_EXAMPLES=OFF
  -DLLAMA_BUILD_APP=OFF
  -DLLAMA_BUILD_SERVER=ON
  -DLLAMA_BUILD_TOOLS=ON
)
if $windows; then
  # CRT de Visual C++ en DLL (/MD, celui de CMake par défaut), pas en
  # statique comme sd-server : avec un CRT statique, chaque DLL de llama.cpp
  # a le sien (tas, FILE*, état), et llama-server s'arrête net (0xC0000409)
  # dès qu'un objet passe de l'une à l'autre (vu dans la CI le 8 nov. 2026,
  # sur un vrai modèle). Ses DLL sont copiées à côté du serveur : un
  # processus enfant ne cherche pas dans le dossier de chatonlm.exe.
  options+=(-A x64)
else
  # Bibliothèques trouvées à côté de l'exécutable, où qu'il soit installé.
  options+=(
    "-DCMAKE_INSTALL_RPATH=\$ORIGIN"
    -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON
  )
fi
cmake -S "$src" -B "$build" "${options[@]}" >"$work/configure.log" 2>&1 ||
  { tail -40 "$work/configure.log" >&2; fail "configuration de CMake en échec"; }
# Les moteurs de calcul (MODULE) sont des dépendances de la cible ggml : ils
# sont construits avec llama-server.
cmake --build "$build" --config Release --target llama-server -j "${LLAMA_JOBS:-4}" \
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
mkdir -p "$stage/fallback"

# Fermeture : le lanceur et les moteurs de calcul, puis tout ce qu'ils
# demandent dans le dossier de construction (copié sous le nom demandé : un
# lien libllama.so.0 -> libllama.so.0.9.0 devient un fichier libllama.so.0).
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

cp -L "$loader_source" "$stage/fallback/$loader"
if ! $windows; then
  for file in "$stage"/* "$stage/fallback/$loader"; do
    [ -f "$file" ] && strip --strip-unneeded "$file"
  done
fi
find "$stage" -type f -exec chmod 0644 {} +
chmod 0755 "$stage/$exe"

# Dépendances : rien d'autre que le système (et Vulkan, chargé par l'app ou
# pris dans fallback/).
if $windows; then
  for file in "$stage"/*.exe "$stage"/*.dll; do
    imports="$(needed "$file" | tr '[:upper:]' '[:lower:]')"
    # Une liste vide ne prouverait rien (objdump qui ne lit pas le fichier).
    grep -qx 'kernel32.dll' <<<"$imports" || fail "DLL importées illisibles (objdump) : $(basename "$file")"
    forbidden="$(grep -E '^(vcomp|libomp|libgomp|libstdc|libgcc|libwinpthread|libssl|libcrypto|libcurl)' <<<"$imports" || true)"
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
# (Linux : LD_LIBRARY_PATH ; Windows : fin de PATH).
with_fallback() {
  if $windows; then
    PATH="$PATH:$stage/fallback" "$@"
  else
    LD_LIBRARY_PATH="$stage/fallback" "$@"
  fi
}
expected="version: ${LLAMA_VERSION#v} (build ${LLAMA_BUILD#b}, commit $short_commit)"
reported="$(with_fallback "$stage/$exe" --version 2>&1 || true)"
grep -qF "$expected" <<<"$reported" || fail "version inattendue : $reported (attendue : $expected)"
devices="$(with_fallback "$stage/$exe" --list-devices 2>&1 || true)"
grep -q '^Available devices:' <<<"$devices" || fail "liste des moteurs de calcul illisible : $devices"

printf '%s\n' "$version" >"$stage/VERSION"
chmod 0644 "$stage/VERSION"
(cd "$stage" && find . -type f ! -name SHA256SUMS | LC_ALL=C sort | sed 's|^\./||' |
  while IFS= read -r file; do sha256 "$file"; done) >"$stage/SHA256SUMS"

rm -rf "$out"
mv "$stage" "$out"
rm -rf "$lock"
trap - EXIT
echo "$out/$exe"
