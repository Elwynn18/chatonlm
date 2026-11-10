#!/usr/bin/env bash
# Enregistre les réponses réelles de llama-server (instance de test,
# scripts/test-llama.sh, port 11436) dans src-tauri/crates/fake-llama/fixtures/ :
# formes de référence du faux serveur (essai fake-llama `fixtures`, qui vérifie
# que le faux rend les champs que le vrai rend et que l'app lit).
#   scripts/test-llama.sh fetch && scripts/record-fixtures.sh
# Modèles : rôle « text » (Qwen3 0.6B : chat, réflexion) puis « fim »
# (Qwen2.5-Coder 0.5B : /infill). L'instance est lancée puis arrêtée par ce
# script, sous le verrou des vrais serveurs (.tools/locks/llama-real.lock :
# un seul vrai serveur à la fois sur la machine) ; une instance déjà lancée
# est refusée. Le prompt, les jetons et les durées des réponses sont gardés
# tels quels (horodatages compris) : seules leurs clés servent aux essais.
# La clé de l'instance passe par un fichier d'en-têtes 0600 (`curl -H @…`),
# jamais par la ligne de commande (visible par `ps`).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/roots.sh
source "$root/scripts/lib/roots.sh"
main="$(chatonlm_main_root "$root")"
out="$root/src-tauri/crates/fake-llama/fixtures"
keyfile="$(chatonlm_llama_test_home "$root")/api-key"
lock="$(chatonlm_real_lock "$root")"
tmp="$main/.tools/tmp"
mkdir -p "$out" "$(dirname "$lock")" "$tmp"

exec 9>"$lock"
flock 9

if "$root/scripts/test-llama.sh" status >/dev/null 2>&1; then
  echo "Une instance de test tourne déjà : « scripts/test-llama.sh stop » d'abord." >&2
  exit 1
fi
# Fichier d'en-têtes de la clé : créé 0600 par mktemp, effacé à la sortie.
headers="$(mktemp -p "$tmp" record-headers.XXXXXX)"
trap 'rm -f "$headers"; "$root/scripts/test-llama.sh" stop >/dev/null 2>&1 || true' EXIT

# Lance l'instance d'essai (rôle `$1`) sans lui laisser le descripteur du
# verrou ; adresse dans `base`, clé dans le fichier d'en-têtes.
start() {
  local info
  "$root/scripts/test-llama.sh" start "$1" 9>&-
  info="$("$root/scripts/test-llama.sh" info 9>&-)"
  printf '%s\n' "$info" | sed 's/^/  /'
  base="$(printf '%s\n' "$info" | sed -n 's/^url=//p')"
  printf 'Authorization: Bearer %s\n' "$(cat "$keyfile")" >"$headers"
}

# Requête authentifiée ; corps sur la sortie.
call() {
  local path="$1"
  shift
  curl -fsS --max-time 120 -H @"$headers" \
    -H 'Content-Type: application/json' "$@" "$base$path"
}

# JSON indenté, clés dans l'ordre du serveur.
pretty() {
  python3 -I -c '
import json, sys
json.dump(json.load(sys.stdin), sys.stdout, indent=2, ensure_ascii=False)
print()
'
}

# Corps et code d'état d'une réponse en erreur (sans clé).
error_fixture() {
  local file="$1" path="$2"
  local body status
  body="$(mktemp -p "$tmp")"
  status="$(curl -sS --max-time 30 -o "$body" -w '%{http_code}' "$base$path")"
  python3 -I -c '
import json, sys
print(json.dumps({"status": int(sys.argv[1]), "body": json.load(open(sys.argv[2]))}, indent=2))
' "$status" "$body" >"$out/$file"
  rm -f "$body"
}

# Corps d'une demande de chat déterministe (graine, température 0, réponse
# courte) ; avec réflexion, assez de jetons pour aller jusqu'à la réponse.
chat_body() {
  local stream="$1" thinking="$2" tokens="$3"
  printf '{"messages": [{"role": "user", "content": "Say hello in one word."}],
    "stream": %s, "seed": 42, "temperature": 0, "max_tokens": %s,
    "chat_template_kwargs": {"enable_thinking": %s}, "cache_prompt": true,
    "stream_options": {"include_usage": true}, "timings_per_token": false}' \
    "$stream" "$tokens" "$thinking"
}

start text
call /props | pretty >"$out/props.json"
call /v1/models | pretty >"$out/models.json"
call /v1/chat/completions -d "$(chat_body true true 1024)" >"$out/chat-think.sse"
call /v1/chat/completions -d "$(chat_body true false 96)" >"$out/chat-nothink.sse"
call /v1/chat/completions -d "$(chat_body false false 96)" | pretty >"$out/chat-once.json"
call /tokenize -d '{"content": "Say hello in one word."}' | pretty >"$out/tokenize.json"
error_fixture unauthorized.json /v1/models
"$root/scripts/test-llama.sh" stop

start fim
call /infill -d '{"input_prefix": "def add(a, b):\n    ", "input_suffix": "\n\nprint(add(1, 2))\n",
  "n_predict": 16, "temperature": 0, "seed": 42, "cache_prompt": true}' | pretty >"$out/infill.json"
"$root/scripts/test-llama.sh" stop

# Dépôt public : aucun chemin de la machine dans les fixtures (dossier des
# modèles dans /props : « <root>/… »).
python3 -I - "$out" "$main" <<'PY'
import pathlib, sys
out, main = pathlib.Path(sys.argv[1]), sys.argv[2]
for path in out.iterdir():
    text = path.read_text(encoding="utf-8")
    if main in text:
        path.write_text(text.replace(main, "<root>"), encoding="utf-8")
PY
if grep -rqF "$main" "$out" || grep -rqF "$HOME" "$out"; then
  echo "Un chemin de la machine reste dans $out" >&2
  exit 1
fi

bin="${CHATONLM_LLAMA_TEST_BIN:-$root/.tools/runtimes/llama-server/llama-server}"
[ -x "$bin" ] || bin="$main/.tools/runtimes/llama-server/llama-server"
"$bin" --version 2>&1 | sed -n 's/^version: //p' >"$out/VERSION"
ls -l "$out"
