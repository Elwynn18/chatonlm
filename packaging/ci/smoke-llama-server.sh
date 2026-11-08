#!/usr/bin/env bash
# Essai de fumée de la CI (.github/workflows/packages.yml), jamais sur un
# bureau réel : le moteur de texte (llama-server) d'un paquet installé ou
# extrait démarre sur cette machine neuve. Contrôles :
# - fichiers livrés (llama-server exécutable, VERSION, SHA256SUMS vérifié,
#   chargeur Vulkan de secours fallback/libvulkan.so.1) ;
# - « llama-server --version » donne la version de VERSION, avec le chargeur
#   du système s'il y en a un, et avec celui de fallback/ (machine sans
#   chargeur : ce que fera l'app) ; « --list-devices » avec fallback/ se lit ;
# - sur un modèle illisible, il s'arrête aussitôt avec le code 1 et sa ligne
#   « failed to load model » (ce que l'app lira pour expliquer l'échec) ;
# - sur un vrai petit modèle (stories260K, 1,2 Mo, MIT, empreinte épinglée
#   ci-dessous), sur le processeur : /health répond, une requête sans la clé
#   d'API est refusée (401), une complétion avec la clé rend du texte ; un
#   SIGTERM l'arrête avec le code 0, et aucun processus ne reste.
#   packaging/ci/smoke-llama-server.sh /usr/lib/chatonlm/llama-server
#   packaging/ci/smoke-llama-server.sh squashfs-root/usr/lib/chatonlm/llama-server
set -euo pipefail

# Modèle d'essai : tinyllamas de Karpathy (MIT) converti en GGUF par ggml-org,
# à une révision figée du dépôt Hugging Face.
MODEL_URL=https://huggingface.co/ggml-org/models-moved/resolve/499bc8821c6b12b4e53c5bffcb21ec206f212d81/tinyllamas/stories260K.gguf
MODEL_SHA256=270cba1bd5109f42d03350f60406024560464db173c0e387d91f0426d3bd256d
PORT=18437

dir="$(cd "${1:?Usage : $0 <dossier llama-server>}" && pwd)"
bin="$dir/llama-server"
fallback="$dir/fallback"
fail() {
  echo "smoke-llama-server : $*" >&2
  exit 1
}
[ -x "$bin" ] || fail "$bin absent ou non exécutable"
[ -f "$dir/VERSION" ] || fail "VERSION absent"
[ -f "$fallback/libvulkan.so.1" ] || fail "chargeur Vulkan de secours absent"
(cd "$dir" && sha256sum --check --quiet --strict SHA256SUMS) || fail "fichiers différents de SHA256SUMS"
pinned="$(cut -d+ -f1 "$dir/VERSION")"
[ -n "$pinned" ] || fail "VERSION vide"
expected="version: ${pinned#v} (build"

check_version() {
  local label="$1"
  shift
  local reported
  reported="$("$@" --version 2>&1)" || fail "$label : --version en échec ($reported)"
  reported="${reported%%$'\n'*}"
  case "$reported" in
    "$expected"*) echo "$label : $reported" ;;
    *) fail "$label : version inattendue « $reported » (attendue : $expected…)" ;;
  esac
}

work="$(mktemp -d)"
cleanup() {
  pkill -KILL -f "^$bin " 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT
# Dossier courant et HOME à l'essai : ggml cherche aussi ses moteurs de
# calcul dans le dossier courant, llama.cpp écrit son cache sous HOME.
cd "$work"
export HOME="$work/home"
mkdir -p "$HOME"

if ldconfig -p 2>/dev/null | grep 'libvulkan\.so\.1 ' >/dev/null; then
  check_version "chargeur du système" "$bin"
else
  echo "Pas de chargeur Vulkan sur cette machine."
fi
check_version "chargeur de secours" env LD_LIBRARY_PATH="$fallback" "$bin"
devices="$(LD_LIBRARY_PATH="$fallback" "$bin" --list-devices 2>&1)" || fail "--list-devices en échec : $devices"
grep -q '^Available devices:' <<<"$devices" || fail "--list-devices illisible : $devices"
echo "moteurs de calcul : $(tr '\n' ';' <<<"$devices")"

# Modèle illisible : refusé, code 1.
printf 'GGUF\003\000\000\000not a model' >"$work/bad.gguf"
status=0
LD_LIBRARY_PATH="$fallback" timeout 60 "$bin" --host 127.0.0.1 --port "$PORT" \
  -m "$work/bad.gguf" -c 256 --no-webui --offline >"$work/bad.log" 2>&1 || status=$?
[ "$status" = 1 ] || { cat "$work/bad.log" >&2; fail "code de sortie $status sur un modèle illisible (attendu : 1)"; }
grep -q 'failed to load model' "$work/bad.log" || { cat "$work/bad.log" >&2; fail "erreur du modèle illisible absente du journal"; }
echo "llama-server : modèle illisible refusé (code 1)."

# Vrai modèle, sur le processeur (aucune couche sur une carte graphique).
curl -fsSL --retry 3 -o "$work/model.gguf" "$MODEL_URL"
echo "$MODEL_SHA256  $work/model.gguf" | sha256sum --check --quiet - || fail "empreinte inattendue du modèle d'essai"
key="essai-$RANDOM$RANDOM"
LD_LIBRARY_PATH="$fallback" timeout 120 "$bin" --host 127.0.0.1 --port "$PORT" \
  --api-key "$key" -m "$work/model.gguf" -c 256 -np 1 -ngl 0 --no-webui --offline \
  >"$work/server.log" 2>&1 &
server=$!
ready=false
for _ in $(seq 1 120); do
  if [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/health")" = 200 ]; then
    ready=true
    break
  fi
  kill -0 "$server" 2>/dev/null || break
  sleep 0.5
done
$ready || { cat "$work/server.log" >&2; fail "/health ne répond pas"; }
denied="$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/completion" \
  -H 'Content-Type: application/json' -d '{"prompt":"Once","n_predict":4}')"
[ "$denied" = 401 ] || fail "requête sans clé d'API : code $denied (attendu : 401)"
answer="$(curl -fsS -X POST "http://127.0.0.1:$PORT/completion" \
  -H "Authorization: Bearer $key" -H 'Content-Type: application/json' \
  -d '{"prompt":"Once upon a time","n_predict":8,"temperature":0}')" ||
  { cat "$work/server.log" >&2; fail "complétion en échec"; }
grep -q '"content":"[^"]' <<<"$answer" || fail "complétion vide : $answer"
echo "llama-server : complétion sur le processeur : $(grep -o '"content":"[^"]*"' <<<"$answer")"

kill -TERM "$server"
status=0
wait "$server" || status=$?
[ "$status" = 0 ] || { tail -20 "$work/server.log" >&2; fail "code de sortie $status après SIGTERM (attendu : 0)"; }
if pgrep -f "^$bin " >/dev/null; then
  fail "un processus llama-server reste"
fi
echo "llama-server : arrêté par SIGTERM (code 0), aucun processus restant."
