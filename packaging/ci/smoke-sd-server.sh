#!/usr/bin/env bash
# Essai de fumée de la CI (.github/workflows/packages.yml), jamais sur un
# bureau réel : le moteur d'images (sd-server) d'un paquet installé ou
# extrait démarre sur cette machine neuve. Contrôles :
# - fichiers livrés (sd-server exécutable, VERSION, chargeur Vulkan de
#   secours fallback/libvulkan.so.1) ;
# - « sd-server --version » donne la version épinglée de VERSION, avec le
#   chargeur du système s'il y en a un, et avec celui de fallback/ (machine
#   sans chargeur : ce que fait l'app, image_gen/linux.rs) ;
# - sur un modèle illisible, il s'arrête aussitôt avec le code 1 et sa ligne
#   « new_sd_ctx_t failed » (ce que l'app lit pour expliquer l'échec), sans
#   planter avant (bibliothèques, moteurs de calcul) ; aucun processus ne
#   reste.
#   packaging/ci/smoke-sd-server.sh /usr/lib/chatonlm/sd-server
#   packaging/ci/smoke-sd-server.sh squashfs-root/usr/lib/chatonlm/sd-server
set -euo pipefail

dir="$(cd "${1:?Usage : $0 <dossier sd-server>}" && pwd)"
bin="$dir/sd-server"
fallback="$dir/fallback"
fail() {
  echo "smoke-sd-server : $*" >&2
  exit 1
}
[ -x "$bin" ] || fail "$bin absent ou non exécutable"
[ -f "$dir/VERSION" ] || fail "VERSION absent"
[ -f "$fallback/libvulkan.so.1" ] || fail "chargeur Vulkan de secours absent"
expected="$(cut -d+ -f1 "$dir/VERSION")"
[ -n "$expected" ] || fail "VERSION vide"

check_version() {
  local label="$1"
  shift
  local reported
  reported="$("$@" --version 2>&1)" || fail "$label : --version en échec ($reported)"
  reported="${reported%%$'\n'*}"
  case "$reported" in
    *"$expected"*) echo "$label : $reported" ;;
    *) fail "$label : version inattendue « $reported » (attendue : $expected)" ;;
  esac
}

if ldconfig -p 2>/dev/null | grep 'libvulkan\.so\.1 ' >/dev/null; then
  check_version "chargeur du système" "$bin"
else
  echo "Pas de chargeur Vulkan sur cette machine."
fi
check_version "chargeur de secours" env LD_LIBRARY_PATH="$fallback" "$bin"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
printf 'GGUF\003\000\000\000not a model' >"$work/bad.gguf"
status=0
LD_LIBRARY_PATH="$fallback" timeout 60 "$bin" --listen-port 18436 \
  --diffusion-model "$work/bad.gguf" >"$work/log" 2>&1 || status=$?
[ "$status" = 1 ] || { cat "$work/log" >&2; fail "code de sortie $status sur un modèle illisible (attendu : 1)"; }
grep -q 'new_sd_ctx_t failed' "$work/log" || { cat "$work/log" >&2; fail "erreur du modèle illisible absente du journal"; }
if pgrep -f "$bin" >/dev/null; then
  fail "un processus sd-server reste"
fi
echo "sd-server : modèle illisible refusé (code 1), aucun processus restant."
