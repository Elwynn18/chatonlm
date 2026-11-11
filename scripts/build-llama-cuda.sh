#!/usr/bin/env bash
# Pack CUDA facultatif de ChatonLM (packaging/cuda.env), appelé par
# « scripts/build-llama-server.sh --cuda » : le moteur ggml-cuda de llama.cpp,
# SEUL, construit depuis le MÊME commit que le llama-server livré
# (packaging/llama.env, mêmes correctifs) avec CUDA 13, chargé à l'exécution
# par ce serveur (GGML_BACKEND_PATH, posé par l'app) à côté de son ggml-base.
# Sortie dans .tools/runtimes :
#   llama-cuda/                       libggml-cuda.so (Linux, RUNPATH
#                                     $ORIGIN : les bibliothèques de NVIDIA
#                                     sont posées à côté par l'app) ou
#                                     ggml-cuda.dll (Windows), VERSION
#                                     (<version épinglée>+chatonlm.cuda.<empreinte>),
#                                     SHA256SUMS, licences (llama.cpp MIT,
#                                     CCCL Apache-2.0 avec exception LLVM)
#   chatonlm-cuda-<build>-linux-x64.tar.gz / -windows-x64.zip
#                                     le même contenu, à plat : le pack
#                                     publié dans la Release
# Jamais aucune bibliothèque de NVIDIA dans le pack (l'app les prend chez
# NVIDIA). Contrôles : dépendances (Linux : NEEDED dans une liste fermée,
# glibc ≤ 2.35 et GLIBCXX ≤ 3.4.30 comme les paquets, RUNPATH $ORIGIN ;
# Windows : DLL importées dans une liste fermée), symbole ggml_backend_init
# exporté. Rien n'est refait si le dossier et l'archive sont à jour.
# Outils : git, cmake, le toolkit CUDA (nvcc ; CI Linux : conteneur
# CUDA_CONTAINER ; Windows : toolkit monté depuis les archives redist et
# l'invite de Visual Studio 2022, générateur Ninja), binutils (readelf,
# objdump, strip). Variable : LLAMA_JOBS (parallélisme, 4 par défaut).
# Affiche le chemin de l'archive. --check : code 0 si tout est à jour, 1
# sinon, sans rien construire.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
env_file="$root/packaging/llama.env"
cuda_env_file="$root/packaging/cuda.env"
patches_dir="$root/packaging/llama"
# shellcheck source=../packaging/llama.env
source "$env_file"
# shellcheck source=../packaging/cuda.env
source "$cuda_env_file"

windows=false
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) windows=true ;;
esac

runtimes="$root/.tools/runtimes"
out="$runtimes/llama-cuda"
work="$root/.tools/llama-cuda-build"
if $windows; then
  lib=ggml-cuda.dll
  archive="$runtimes/chatonlm-cuda-$LLAMA_BUILD-windows-x64.zip"
else
  lib=libggml-cuda.so
  archive="$runtimes/chatonlm-cuda-$LLAMA_BUILD-linux-x64.tar.gz"
fi

sha256() {
  sha256sum "$@"
}

shopt -s nullglob
patches=("$patches_dir"/*.patch)
shopt -u nullglob

# Identité : version épinglée, toolkit, correctifs et ce script.
fingerprint="$({
  cat "$env_file" "$cuda_env_file"
  [ "${#patches[@]}" -eq 0 ] || cat "${patches[@]}"
  cat "${BASH_SOURCE[0]}"
} | sha256 | cut -c1-12)"
version="$LLAMA_VERSION+chatonlm.cuda.$fingerprint"

if [ -f "$out/$lib" ] && [ -f "$archive" ] && [ "$(cat "$out/VERSION" 2>/dev/null)" = "$version" ] &&
  (cd "$out" && sha256 --check --quiet --strict SHA256SUMS >/dev/null 2>&1); then
  echo "$archive"
  exit 0
fi
if [ "${1:-}" = "--check" ]; then
  echo "pack CUDA à construire ($version)" >&2
  exit 1
fi

fail() {
  echo "build-llama-cuda : $*" >&2
  exit 1
}

for tool in git cmake nvcc; do
  command -v "$tool" >/dev/null || fail "outil manquant : $tool"
done

lock="$root/.tools/llama-cuda-build.lock"
mkdir -p "$root/.tools"
mkdir "$lock" 2>/dev/null || fail "une autre construction est en cours (sinon supprimer $lock)"
trap 'rm -rf "$lock"' EXIT

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

nvcc_version="$(nvcc --version | sed -n 's/.*release \([0-9.]*\),.*/\1/p')"
[ "${nvcc_version%%.*}" = "$CUDA_MAJOR" ] || fail "nvcc $nvcc_version, CUDA $CUDA_MAJOR attendu"

short_commit="${LLAMA_COMMIT:0:9}"
# Mêmes options que le moteur livré pour ggml-base (interface des moteurs),
# CUDA à la place de Vulkan ; ni processeur ni outils (seul ggml-cuda est
# construit) ; NCCL coupé (plusieurs cartes : rare, et une dépendance de
# plus) ; architectures par défaut de ggml pour CUDA 13 (pas de
# CMAKE_CUDA_ARCHITECTURES).
options=(
  -DCMAKE_BUILD_TYPE=Release
  -DBUILD_SHARED_LIBS=ON
  -DLLAMA_BUILD_IS_DEV=OFF
  "-DLLAMA_BUILD_NUMBER=${LLAMA_BUILD#b}"
  "-DLLAMA_BUILD_COMMIT=$short_commit"
  -DGGML_BACKEND_DL=ON
  -DGGML_NATIVE=OFF
  -DGGML_CPU=OFF
  -DGGML_VULKAN=OFF
  -DGGML_CUDA=ON
  -DGGML_CUDA_NCCL=OFF
  # Runtime CUDA en bibliothèque partagée (cudart64_13.dll /
  # libcudart.so.13, prise chez NVIDIA par l'app) : le défaut de CMake pour
  # le langage CUDA est le runtime statique, qui mettrait du code de NVIDIA
  # dans NOTRE pack (vu sous Windows le 10 nov. 2026).
  -DCMAKE_CUDA_RUNTIME_LIBRARY=Shared
  "-DGGML_CUDA_CCCL_VERSION=$CUDA_CCCL_VERSION"
  -DGGML_OPENMP=OFF
  -DLLAMA_OPENSSL=OFF
  -DLLAMA_BUILD_UI=OFF
  -DLLAMA_USE_PREBUILT_UI=OFF
  -DLLAMA_CONNECT=OFF
  -DLLAMA_LLGUIDANCE=OFF
  -DLLAMA_BUILD_TESTS=OFF
  -DLLAMA_BUILD_EXAMPLES=OFF
  -DLLAMA_BUILD_APP=OFF
  -DLLAMA_BUILD_SERVER=OFF
  -DLLAMA_BUILD_TOOLS=OFF
)
if $windows; then
  # Ninja dans l'invite de Visual Studio (le générateur Visual Studio
  # exigerait l'intégration CUDA de MSBuild). Éditeur de liens de MSVC
  # nommé : celui de Git Bash (« link » de coreutils) passerait devant.
  cl_path="$(command -v cl)" || fail "cl.exe introuvable (invite de Visual Studio)"
  command -v ninja >/dev/null || fail "outil manquant : ninja"
  options+=(
    -G Ninja
    -DCMAKE_C_COMPILER=cl
    -DCMAKE_CXX_COMPILER=cl
    "-DCMAKE_LINKER=$(cygpath -m "$(dirname "$cl_path")/link.exe")"
  )
else
  options+=(
    "-DCMAKE_INSTALL_RPATH=\$ORIGIN"
    -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON
  )
fi
cmake -S "$src" -B "$build" "${options[@]}" >"$work/configure.log" 2>&1 ||
  { tail -40 "$work/configure.log" >&2; fail "configuration de CMake en échec"; }
cmake --build "$build" --config Release --target ggml-cuda -j "${LLAMA_JOBS:-4}" \
  >"$work/build.log" 2>&1 ||
  { tail -40 "$work/build.log" >&2; fail "construction en échec"; }

built="$(find "$build/bin" -name "$lib" -type f -print -quit)"
[ -n "$built" ] || fail "$lib introuvable après la construction"
cccl_license="$(find "$build/_deps" -maxdepth 2 -path '*cccl-src/LICENSE' -print -quit 2>/dev/null || true)"
[ -n "$cccl_license" ] || fail "licence de CCCL introuvable (FetchContent)"

stage="$out.part"
rm -rf "$stage"
trap 'rm -rf "$stage" "$lock"' EXIT
mkdir -p "$stage"
cp -L "$built" "$stage/$lib"
cp "$src/LICENSE" "$stage/LICENSE-llama.cpp.txt"
cp "$cccl_license" "$stage/LICENSE-CCCL.txt"
$windows || strip --strip-unneeded "$stage/$lib"
chmod 0644 "$stage"/*

# Dépendances : liste fermée. ggml-base vient du llama-server livré ; les
# bibliothèques de NVIDIA, de l'app (posées à côté) ; libcuda / nvcuda, du
# pilote.
if $windows; then
  imports="$(objdump -p "$stage/$lib" | awk '/DLL Name:/ { print tolower($3) }')"
  grep -qx 'kernel32.dll' <<<"$imports" || fail "DLL importées illisibles (objdump)"
  allowed="^(ggml-base\.dll|cudart64_${CUDA_MAJOR}\.dll|cublas64_${CUDA_MAJOR}\.dll|cublaslt64_${CUDA_MAJOR}\.dll|nvcuda\.dll|kernel32\.dll|advapi32\.dll|msvcp140(_[0-9a-z]+)?\.dll|vcruntime140(_[0-9a-z]+)?\.dll|api-ms-win-crt-[a-z0-9-]+\.dll)$"
  outside="$(grep -Ev "$allowed" <<<"$imports" || true)"
  [ -z "$outside" ] || fail "$lib importe : $(tr '\n' ' ' <<<"$outside")"
  for required in ggml-base.dll "cudart64_${CUDA_MAJOR}.dll" "cublas64_${CUDA_MAJOR}.dll"; do
    grep -qx "$required" <<<"$imports" ||
      fail "$lib n'importe pas $required (liaison inattendue) ; DLL importées : $(tr '\n' ' ' <<<"$imports")"
  done
  objdump -p "$stage/$lib" | grep -q 'ggml_backend_init' || fail "ggml_backend_init non exporté"
else
  needed="$(readelf -d "$stage/$lib" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')"
  allowed="^(libggml-base\.so\.0|libcudart\.so\.${CUDA_MAJOR}|libcublas\.so\.${CUDA_MAJOR}|libcublasLt\.so\.${CUDA_MAJOR}|libcuda\.so\.1|libstdc\+\+\.so\.6|libgcc_s\.so\.1|libc\.so\.6|libm\.so\.6|libdl\.so\.2|libpthread\.so\.0|librt\.so\.1|ld-linux-x86-64\.so\.2)$"
  outside="$(grep -Ev "$allowed" <<<"$needed" || true)"
  [ -z "$outside" ] || fail "$lib dépend de : $(tr '\n' ' ' <<<"$outside")"
  for required in libggml-base.so.0 "libcudart.so.${CUDA_MAJOR}" "libcublas.so.${CUDA_MAJOR}"; do
    grep -qx "$required" <<<"$needed" ||
      fail "$lib ne dépend pas de $required (liaison inattendue) ; NEEDED : $(tr '\n' ' ' <<<"$needed")"
  done
  runpath="$(readelf -d "$stage/$lib" | sed -n 's/.*(RUNPATH).*\[\(.*\)\]/\1/p')"
  [ "$runpath" = '$ORIGIN' ] || fail "RUNPATH inattendu : ${runpath:-(aucun)}"
  symbols="$(objdump -T "$stage/$lib")"
  grep -q ' ggml_backend_init$' <<<"$symbols" || fail "ggml_backend_init non exporté"
  newest() {
    grep -o "$1[0-9.]*" <<<"$symbols" | sed "s/^$1//" | sort -V | tail -1
  }
  glibc="$(newest 'GLIBC_')"
  glibcxx="$(newest 'GLIBCXX_')"
  [ "$(printf '%s\n' "$glibc" 2.35 | sort -V | tail -1)" = 2.35 ] || fail "glibc $glibc exigée (2.35 au plus)"
  [ -z "$glibcxx" ] || [ "$(printf '%s\n' "$glibcxx" 3.4.30 | sort -V | tail -1)" = 3.4.30 ] ||
    fail "GLIBCXX $glibcxx exigée (3.4.30 au plus)"
fi
if find "$stage" -iname '*cudart*' -o -iname '*cublas*' | grep -q .; then
  fail "bibliothèque de NVIDIA dans le pack"
fi

printf '%s\n' "$version" >"$stage/VERSION"
chmod 0644 "$stage/VERSION"
(cd "$stage" && find . -type f ! -name SHA256SUMS | LC_ALL=C sort | sed 's|^\./||' |
  while IFS= read -r file; do sha256 "$file"; done) >"$stage/SHA256SUMS"

# Archive à plat, reproductible (ordre, dates et propriétaires fixes).
rm -f "$archive"
if $windows; then
  if command -v 7z >/dev/null; then
    (cd "$stage" && 7z a -tzip -bd -mx=9 "$(cygpath -w "$archive")" ./* >/dev/null)
  else
    powershell -NoProfile -Command "Compress-Archive -Path '$(cygpath -w "$stage")\\*' -DestinationPath '$(cygpath -w "$archive")'"
  fi
else
  (cd "$stage" && LC_ALL=C tar --sort=name --owner=0 --group=0 --numeric-owner --mtime=@0 \
    -czf "$archive" -- *)
fi
[ -s "$archive" ] || fail "archive absente : $archive"

rm -rf "$out"
mv "$stage" "$out"
rm -rf "$lock"
trap - EXIT
echo "$archive"
