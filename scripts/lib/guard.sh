# Vérifie que les outils utilisés sont bien ceux de .tools/ et aux versions
# épinglées (rust-toolchain.toml, .nvmrc). À sourcer après activate.sh :
#   source "$root/scripts/lib/guard.sh" && chatonlm_guard "$root"
# shellcheck shell=bash

chatonlm_guard() {
  local root="$1" tool path expected actual
  for tool in cargo rustc node npm; do
    path="$(command -v "$tool" || true)"
    case "$path" in
      "$root/.tools/"*) ;;
      *)
        echo "$tool ne vient pas de .tools/ (${path:-introuvable}) : activate.sh est-il chargé ?" >&2
        return 1
        ;;
    esac
  done

  expected="$(tr -d '[:space:]' <"$root/.nvmrc")"
  actual="$(node --version)"
  if [ "$actual" != "$expected" ]; then
    echo "Node $actual au lieu de $expected (.nvmrc)" >&2
    return 1
  fi

  expected="$(sed -n 's/^channel = "\(.*\)"/\1/p' "$root/rust-toolchain.toml")"
  case "$(rustc --version)" in
    "rustc $expected "*) ;;
    *)
      echo "$(rustc --version) au lieu de Rust $expected (rust-toolchain.toml)" >&2
      return 1
      ;;
  esac
}
