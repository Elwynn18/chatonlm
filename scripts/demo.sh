# Lance ChatonLM tel qu'il est dans un commit (par défaut le dernier).
#   scripts/demo.sh [--build-only] [--onboarding] [--real [--reset]] [commit]
# Le code du commit est extrait dans .tools/demo-src : les modifications en
# cours dans le dépôt ne changent pas l'app pendant un essai. Fermer la fenêtre
# arrête tout.
#
# Par défaut, démonstration : build de débogage (identifiant de développement)
# dans .tools/demo-target, historique de démonstration et faux serveur Ollama
# local, sans modèle ; données (.tools/demo-data) recréées à chaque lancement.
#
# --real, conditions réelles : build de production (vrai identifiant, aucune
# fonctionnalité d'essai) dans .tools/real-target, sans historique de
# démonstration ni faux serveur ; l'app joint l'Ollama de la machine (à lancer
# soi-même, avec ses modèles) et contacte ollama.com comme l'app installée.
# Seules ses données restent dans le projet : .tools/real-data (ignoré par
# git), gardé d'un lancement à l'autre ; --reset le vide avant de lancer.
# --onboarding : montre le premier démarrage (choix de la langue, présentation
# de l'app), même avec des données qui ont déjà servi ; sans lui, --real le
# montre seulement à des données neuves, et la démonstration jamais.
# Raccourci de la dictée : jamais par le portail du bureau avant
# l'empaquetage (sans fichier .desktop installé, KDE le rangerait sous
# l'application qui a lancé ChatonLM) ; lier scripts/dictate.sh à la place.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
# shellcheck source=../activate.sh
source ./activate.sh
# shellcheck source=lib/guard.sh
source scripts/lib/guard.sh
chatonlm_guard "$root"

build_only=false
real=false
reset=false
onboarding=false
while [ $# -gt 0 ]; do
  case "$1" in
    --build-only) build_only=true ;;
    --real) real=true ;;
    --reset) reset=true ;;
    --onboarding) onboarding=true ;;
    -*)
      echo "Usage : $0 [--build-only] [--onboarding] [--real [--reset]] [commit]" >&2
      exit 2
      ;;
    *) break ;;
  esac
  shift
done
if $reset && ! $real; then
  echo "--reset ne va qu'avec --real." >&2
  exit 2
fi
commit="$(git rev-parse --verify "${1:-HEAD}^{commit}")"
src="$root/.tools/demo-src"
if $real; then
  target="$root/.tools/real-target"
  data="$root/.tools/real-data"
  log="$root/.tools/logs/real-build.log"
  binary="$target/release/chatonlm"
else
  target="$root/.tools/demo-target"
  data="$root/.tools/demo-data"
  log="$root/.tools/logs/demo-build.log"
  binary="$target/debug/chatonlm"
fi
mkdir -p "$root/.tools/tmp" "$root/.tools/logs"

if [ "$(cat "$src/.demo-commit" 2>/dev/null || true)" != "$commit" ]; then
  rm -rf "$src"
  mkdir -p "$src"
  git archive "$commit" | tar -x -C "$src"
  ln -s "$root/node_modules" "$src/node_modules"
  echo "$commit" >"$src/.demo-commit"
fi

echo "Compilation de ${commit:0:7} (journal : $log)…"
# Un binaire d'un autre commit ne doit jamais être lancé si la compilation
# échoue. Dans « if ! ( … ) », set -e est ignoré : les étapes sont chaînées.
rm -f "$binary" "$target/debug/fake-ollama"
if $real; then
  build() {
    node_modules/.bin/tauri build --no-bundle
  }
else
  build() {
    node_modules/.bin/tauri build --debug --no-bundle \
      --config src-tauri/tauri.dev.conf.json &&
      cargo build --locked --manifest-path src-tauri/Cargo.toml -p fake-ollama
  }
fi
if ! (
  cd "$src" &&
    export CARGO_TARGET_DIR="$target" TMPDIR="$root/.tools/tmp" &&
    build
) >"$log" 2>&1; then
  echo "Échec de la compilation : voir $log" >&2
  exit 1
fi
# Moteur d'images (génération dans la conversation) : sous Linux, Tauri ne
# prend le dossier du binaire pour celui des ressources que si son parent
# s'appelle « target » (ici « real-target ») ; sinon <binaire>/../lib/chatonlm
# s'il existe, puis /usr/lib/chatonlm (tauri-utils, resource_dir_from). Le
# sd-server construit par scripts/build-sd-server.sh est lié dans le premier ;
# rien d'autre n'y est lu ici (ONNX Runtime : ORT_DYLIB_PATH ; pas d'Ollama
# intégré dans la recette).
if $real; then
  rm -f "$target/release/sd-server"
  if [ -x "$root/.tools/runtimes/sd-server/sd-server" ]; then
    mkdir -p "$target/lib/chatonlm"
    ln -sfn "$root/.tools/runtimes/sd-server" "$target/lib/chatonlm/sd-server"
  else
    echo "Moteur d'images absent (scripts/build-sd-server.sh) : génération d'images indisponible." >&2
  fi
fi

if $build_only; then
  echo "Prêt : $binary"
  exit 0
fi

if $real; then
  $reset && rm -rf "$data"
  mkdir -p "$data"
  echo "Données (gardées ; --reset pour repartir de zéro) : $data"
  # Voix de la lecture à voix haute : tant que la release GitHub de leurs
  # fichiers n'est pas publique, celles préparées dans .tools/voice-models
  # sont liées fichier par fichier (supprimer une voix dans l'app enlève les
  # liens, pas les originaux).
  for voice in "$root"/.tools/voice-models/*/; do
    [ -d "$voice" ] || continue
    target_dir="$data/voice-models/$(basename "$voice")"
    mkdir -p "$target_dir"
    for file in "$voice"*; do
      link="$target_dir/$(basename "$file")"
      # Un lien mort (dossier du projet renommé ou déplacé) est refait : « -e »
      # suit le lien, et « ln -s » seul échouerait sur le lien resté en place.
      [ -e "$link" ] || ln -sfn "$file" "$link"
    done
  done
  echo "Ollama : celui de la machine (${OLLAMA_HOST:-127.0.0.1:11434}), à lancer soi-même."
  # Mode conversation vocale : une ligne « chatonlm-voice-timing » par tour
  # dans ce terminal (millisecondes depuis la fin de parole détectée :
  # transcription, envoi, premier son).
  echo "Mesures du mode vocal : lignes chatonlm-voice-timing ci-dessous."
  exec env -u CHATONLM_SEED_DEMO -u CHATONLM_REGISTRY_URL -u CHATONLM_OLLAMA_COM_URL \
    -u CHATONLM_TELEGRAM_URL -u CHATONLM_DISCORD_URL \
    CHATONLM_ONBOARDING="$($onboarding && echo show || true)" \
    CHATONLM_DATA_DIR="$data" TMPDIR="$root/.tools/tmp" \
    CHATONLM_NO_GLOBAL_SHORTCUTS=1 CHATONLM_VOICE_TIMINGS=1 "$binary"
fi

rm -rf "$data"
mkdir -p "$data"
fake_log="$root/.tools/logs/demo-fake-ollama.log"
"$target/debug/fake-ollama" ollama >"$fake_log" 2>&1 &
fake_pid=$!
trap 'kill "$fake_pid" 2>/dev/null || true; wait "$fake_pid" 2>/dev/null || true' EXIT

url=""
for _ in $(seq 50); do
  url="$(head -n 1 "$fake_log" 2>/dev/null || true)"
  [[ "$url" == http://* ]] && break
  sleep 0.1
done
if [[ "$url" != http://* ]]; then
  echo "Le faux serveur Ollama n'a pas démarré (voir $fake_log)." >&2
  exit 1
fi

echo "Données de démonstration : $data"
echo "Faux serveur Ollama : $url"
CHATONLM_ONBOARDING="$($onboarding && echo show || echo skip)" \
  CHATONLM_DATA_DIR="$data" OLLAMA_HOST="$url" CHATONLM_SEED_DEMO=1 \
  TMPDIR="$root/.tools/tmp" "$binary"
