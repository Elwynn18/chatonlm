#!/usr/bin/env bash
# Essai de fumée de la CI (.github/workflows/packages.yml), jamais sur un
# bureau réel : l'Ollama intégré d'un paquet installé ou extrait démarre sur
# cette machine neuve. « ollama serve » sur 127.0.0.1:18434, HOME et dossier
# des modèles temporaires ; échec s'il ne répond pas à /api/version dans les
# 30 s avec la version de son fichier VERSION, ou si son journal ne décrit
# aucun moteur de calcul (« inference compute »). Son groupe de processus est
# arrêté ensuite (SIGTERM puis SIGKILL) ; échec s'il en reste un.
#   packaging/ci/smoke-ollama.sh /usr/lib/chatonlm/ollama
#   packaging/ci/smoke-ollama.sh squashfs-root/usr/lib/chatonlm/ollama
set -euo pipefail

dir="$(cd "${1:?Usage : $0 <dossier ollama>}" && pwd)"
bin="$dir/bin/ollama"
expected="$(cat "$dir/VERSION")"
host=127.0.0.1:18434
work="$(mktemp -d)"
log="$work/serve.log"
mkdir -p "$work/home" "$work/models"

HOME="$work/home" OLLAMA_MODELS="$work/models" OLLAMA_HOST="$host" \
  setsid "$bin" serve >"$log" 2>&1 </dev/null &
pid=$!

status=0
version=""
for _ in $(seq 60); do
  version="$(curl -sf --max-time 1 "http://$host/api/version" || true)"
  [ -n "$version" ] && break
  kill -0 "$pid" 2>/dev/null || break
  sleep 0.5
done
if [ "$version" = "{\"version\":\"$expected\"}" ]; then
  echo "✓ Ollama intégré $expected : répond sur $host."
else
  echo "✗ Ollama intégré : réponse « $version » au lieu de la version $expected."
  status=1
fi
# Moteurs détectés après la réponse (découverte des GPU en arrière-plan).
for _ in $(seq 20); do
  grep -q 'msg="inference compute"' "$log" && break
  sleep 0.5
done
if grep 'msg="inference compute"' "$log"; then
  echo "✓ Moteur de calcul décrit."
else
  echo "✗ Aucun moteur de calcul décrit."
  status=1
fi

kill -TERM -- "-$pid" 2>/dev/null || true
for _ in $(seq 50); do
  pgrep -g "$pid" >/dev/null || break
  sleep 0.2
done
if pgrep -g "$pid" >/dev/null; then
  echo "✗ Processus encore actifs 10 s après SIGTERM : $(pgrep -g "$pid" | tr '\n' ' ')"
  kill -KILL -- "-$pid" 2>/dev/null || true
  status=1
fi
wait "$pid" 2>/dev/null || true

echo "--- journal de l'Ollama intégré ---"
cat "$log"
rm -rf "$work"
exit "$status"
