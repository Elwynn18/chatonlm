#!/usr/bin/env bash
# Enregistre les réponses réelles d'Ollama (instance de test, port 11435) dans
# src-tauri/crates/fake-ollama/fixtures/ : le faux serveur les rejoue, et les
# tests de contrat (#[ignore]) vérifient que l'instance répond toujours ainsi.
#   scripts/test-ollama.sh start && scripts/record-fixtures.sh
# Modèles requis sur l'instance : qwen3:0.6b et qwen3.5:0.8b
# (scripts/test-ollama.sh pull <modèle>). Les champs volumineux et inutiles à
# ChatonLM (licence, gabarit, Modelfile, tenseurs) sont retirés des détails.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
base="http://127.0.0.1:11435"
out="$root/src-tauri/crates/fake-ollama/fixtures"
mkdir -p "$out"

version="$(curl -fsS --max-time 5 "$base/api/version")" || {
  echo "Instance de test injoignable : scripts/test-ollama.sh start" >&2
  exit 1
}
echo "Ollama $version"

# Réponse JSON indentée, sans clés ignorées par ChatonLM.
pretty() {
  python3 -c '
import json, sys
data = json.load(sys.stdin)
if "model_info" in data:
    arch = data["model_info"].get("general.architecture")
    keep = {"general.architecture", f"{arch}.context_length"}
    data["model_info"] = {k: v for k, v in data["model_info"].items() if k in keep}
for key in ("license", "modelfile", "template", "parameters", "tensors"):
    data.pop(key, None)
json.dump(data, sys.stdout, indent=2, ensure_ascii=False)
print()
'
}

# Corps et code d'état d'une réponse en erreur.
error_fixture() {
  local file="$1"
  shift
  local body status
  body="$(mktemp -p "$root/.tools/tmp")"
  status="$(curl -sS --max-time 30 -o "$body" -w '%{http_code}' "$@")"
  python3 -c '
import json, sys
print(json.dumps({"status": int(sys.argv[1]), "body": json.load(open(sys.argv[2]))}, indent=2))
' "$status" "$body" >"$out/$file"
  rm -f "$body"
}

curl -fsS --max-time 10 "$base/api/tags" | pretty >"$out/tags.json"
for model in qwen3:0.6b qwen3.5:0.8b; do
  curl -fsS --max-time 30 "$base/api/show" -d "{\"model\":\"$model\"}" |
    pretty >"$out/show-${model//[:.]/-}.json"
done
error_fixture show-missing.json "$base/api/show" -d '{"model":"chatonlm-missing:latest"}'

# Flux de chat déterministes (graine, température 0, réponse courte) ; avec
# raisonnement, assez de jetons pour aller jusqu'à la réponse.
chat() {
  curl -fsS --max-time 120 "$base/api/chat" -d "{
    \"model\": \"qwen3:0.6b\",
    \"messages\": [{\"role\": \"user\", \"content\": \"Say hello in one word.\"}],
    \"think\": $1,
    \"options\": {\"seed\": 42, \"temperature\": 0, \"num_predict\": $2}
  }"
}
chat true 1024 >"$out/chat-think.ndjson"
chat false 96 >"$out/chat-nothink.ndjson"
error_fixture chat-missing.json "$base/api/chat" \
  -d '{"model":"chatonlm-missing:latest","messages":[{"role":"user","content":"hi"}]}'

echo "$version" | python3 -c 'import json,sys; print(json.load(sys.stdin)["version"])' \
  >"$out/VERSION"
ls -l "$out"
