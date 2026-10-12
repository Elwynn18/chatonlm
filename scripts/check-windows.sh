#!/usr/bin/env bash
# Contrôle de ChatonLM pour Windows depuis Linux : `cargo check` ou
# `cargo clippy` de la cible x86_64-pc-windows-gnullvm avec la chaîne
# llvm-mingw de .tools (scripts/fetch-llvm-mingw.sh). Rien n'est lié ni
# exécuté : c'est un contrôle de compilation du code propre à Windows
# (`cfg(windows)`), pour qu'un changement fait sous Linux ne casse pas
# Windows. La construction réelle (MSVC), les essais et l'installateur sont
# faits par la CI (.github/workflows/packages-windows.yml).
#
# Usage : scripts/check-windows.sh [check|clippy] [options de cargo...]
#   sans option : tout l'espace de travail, toutes les cibles.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="$("$root/scripts/fetch-llvm-mingw.sh")"
sysroot="$(dirname "$bin")"

# Noms nus des outils (le chemin du projet contient une espace).
export PATH="$bin:$PATH"
export CC_x86_64_pc_windows_gnullvm=x86_64-w64-mingw32-clang
export CXX_x86_64_pc_windows_gnullvm=x86_64-w64-mingw32-clang++
export AR_x86_64_pc_windows_gnullvm=llvm-ar
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER=x86_64-w64-mingw32-clang
# bindgen (whisper-rs-sys) : le clang du système refuse le triplet « gnullvm »
# et ne connaît ni les en-têtes de mingw-w64 ni ceux de ce clang. Chemins
# entre apostrophes : bindgen découpe la valeur comme un shell.
resource="$(echo "$sysroot"/lib/clang/*/include)"
export BINDGEN_EXTRA_CLANG_ARGS_x86_64_pc_windows_gnullvm="--target=x86_64-w64-mingw32 '--sysroot=$sysroot/x86_64-w64-mingw32' -isystem '$resource' -isystem '$sysroot/x86_64-w64-mingw32/include'"

command="check"
case "${1:-}" in
  check | clippy)
    command="$1"
    shift
    ;;
esac
if [ $# -eq 0 ]; then
  set -- --workspace --all-targets
fi
tail=()
if [ "$command" = clippy ]; then
  tail=(-- -D warnings)
fi
exec cargo "$command" --manifest-path "$root/src-tauri/Cargo.toml" \
  --target x86_64-pc-windows-gnullvm --locked "$@" "${tail[@]}"
