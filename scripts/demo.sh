# Lance ChatonLM tel qu'il est dans un commit (par défaut le dernier).
#   scripts/demo.sh [--build-only] [--onboarding] [--real [--reset] [--vscode] [--import-ollama <dossier>] [--machine-ollama]] [commit]
# Le code du commit est extrait dans .tools/demo-src : les modifications en
# cours dans le dépôt ne changent pas l'app pendant un essai. Fermer la fenêtre
# arrête tout.
#
# Par défaut, démonstration : build de débogage (identifiant de développement)
# dans .tools/demo-target, historique de démonstration et faux moteur
# (fake-llama, un modèle de démonstration inscrit au démarrage) ; données
# (.tools/demo-data) recréées à chaque lancement.
#
# --real, conditions réelles : build de production (vrai identifiant, aucune
# fonctionnalité d'essai) dans .tools/real-target, sans historique de
# démonstration ni faux serveur ; l'app lance le vrai llama-server
# (.tools/runtimes/llama-server, scripts/build-llama-server.sh) sur les modèles
# de son dossier et contacte Hugging Face comme l'app installée. Seules ses
# données restent dans le projet : .tools/real-data (ignoré par git), gardé
# d'un lancement à l'autre ; --reset le vide avant de lancer.
# --import-ollama <dossier> (avec --real) : pour essayer la reprise des modèles
# d'un Ollama de ChatonLM 0.9.x, COPIE le dossier de modèles d'Ollama donné
# (manifests/ et blobs/, par exemple celui de l'Ollama intégré d'une 0.9.x ou
# ~/.ollama/models) dans .tools/real-data/ollama/models, l'emplacement de
# l'Ollama intégré d'une 0.9.x, après avoir affiché sa taille et demandé
# confirmation. La reprise déplace les fichiers de l'intégré : elle travaille
# sur la copie, le dossier donné n'est jamais modifié. Les liens symboliques
# (un blobs/ lié vers un autre disque) sont suivis : la copie n'en contient
# aucun, sinon elle est refusée (scripts/lib/ollama-copy.sh). Refusé si la
# copie existe déjà (--reset pour repartir de zéro).
# --machine-ollama (avec --real) : laisse la reprise des modèles regarder
# l'Ollama de la machine (service, ~/.ollama/models, dossiers des paquets),
# en lecture seule, comme l'app installée ; sans elle, aucun dossier de
# l'Ollama de la machine n'est regardé (CHATONLM_OLLAMA_MACHINE_DIRS vide).
# --onboarding : montre le premier démarrage (choix de la langue, présentation
# de l'app), même avec des données qui ont déjà servi ; sans lui, --real le
# montre seulement à des données neuves, et la démonstration jamais.
# --vscode (avec --real) : construit aussi l'extension ChatonLM pour VS Code du
# même commit et ouvre VS Code avec elle (--extensionDevelopmentPath), dans un
# profil à part gardé dans .tools/vscode-recipe (réglages, extensions et
# connexions de ce profil seulement ; le VS Code habituel n'est pas touché).
# Rien n'est installé. Le dossier ouvert est le dépôt extrait du commit.
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
# shellcheck source=lib/ollama-copy.sh
source scripts/lib/ollama-copy.sh
chatonlm_guard "$root"

build_only=false
real=false
reset=false
onboarding=false
vscode=false
import_ollama=""
machine_ollama=false
usage="Usage : $0 [--build-only] [--onboarding] [--real [--reset] [--vscode] [--import-ollama <dossier>] [--machine-ollama]] [commit]"
while [ $# -gt 0 ]; do
  case "$1" in
    --build-only) build_only=true ;;
    --real) real=true ;;
    --reset) reset=true ;;
    --onboarding) onboarding=true ;;
    --vscode) vscode=true ;;
    --machine-ollama) machine_ollama=true ;;
    --import-ollama)
      import_ollama="${2:-}"
      if [ -z "$import_ollama" ]; then
        echo "$usage" >&2
        exit 2
      fi
      shift
      ;;
    -*)
      echo "$usage" >&2
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
if $vscode && ! $real; then
  echo "--vscode ne va qu'avec --real." >&2
  exit 2
fi
if $machine_ollama && ! $real; then
  echo "--machine-ollama ne va qu'avec --real." >&2
  exit 2
fi
if [ -n "$import_ollama" ]; then
  if ! $real; then
    echo "--import-ollama ne va qu'avec --real." >&2
    exit 2
  fi
  if [ ! -d "$import_ollama/manifests" ] && [ ! -d "$import_ollama/manifests-v2" ]; then
    echo "$import_ollama n'est pas un dossier de modèles d'Ollama (ni manifests/ ni manifests-v2/)." >&2
    exit 2
  fi
  import_ollama="$(cd "$import_ollama" && pwd)"
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
rm -f "$binary" "$target/debug/fake-llama"
if $real; then
  build() {
    node_modules/.bin/tauri build --no-bundle
  }
else
  build() {
    node_modules/.bin/tauri build --debug --no-bundle \
      --config src-tauri/tauri.dev.conf.json &&
      cargo build --locked --manifest-path src-tauri/Cargo.toml -p fake-llama
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
# Moteurs livrés (texte : llama-server ; images : sd-server) : sous Linux,
# Tauri ne prend le dossier du binaire pour celui des ressources que si son
# parent s'appelle « target » (ici « real-target ») ; sinon
# <binaire>/../lib/chatonlm s'il existe, puis /usr/lib/chatonlm (tauri-utils,
# resource_dir_from). Les constructions locales (scripts/build-llama-server.sh,
# scripts/build-sd-server.sh) sont liées dans le premier ; rien d'autre n'y est
# lu ici (ONNX Runtime : ORT_DYLIB_PATH).
if $real; then
  rm -f "$target/release/sd-server"
  mkdir -p "$target/lib/chatonlm"
  if [ -x "$root/.tools/runtimes/llama-server/llama-server" ]; then
    ln -sfn "$root/.tools/runtimes/llama-server" "$target/lib/chatonlm/llama-server"
  else
    rm -f "$target/lib/chatonlm/llama-server"
    echo "Moteur de texte absent (scripts/build-llama-server.sh) : aucun modèle ne se chargera." >&2
  fi
  if [ -x "$root/.tools/runtimes/sd-server/sd-server" ]; then
    ln -sfn "$root/.tools/runtimes/sd-server" "$target/lib/chatonlm/sd-server"
  else
    rm -f "$target/lib/chatonlm/sd-server"
    echo "Moteur d'images absent (scripts/build-sd-server.sh) : génération d'images indisponible." >&2
  fi
fi

# Extension VS Code du même commit (hôte, serveur MCP et panneau dans
# vscode/dist du dépôt extrait).
if $vscode; then
  vscode_log="$root/.tools/logs/real-vscode-build.log"
  echo "Construction de l'extension VS Code (journal : $vscode_log)…"
  rm -rf "$src/vscode/dist"
  if ! (
    cd "$src" &&
      export TMPDIR="$root/.tools/tmp" &&
      node_modules/.bin/vite build -c vscode/vite.extension.config.mts &&
      node_modules/.bin/vite build -c vscode/webview.vite.config.mts
  ) >"$vscode_log" 2>&1; then
    echo "Échec de la construction de l'extension : voir $vscode_log" >&2
    exit 1
  fi
fi

if $build_only; then
  echo "Prêt : $binary"
  $vscode && echo "Extension : $src/vscode"
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
  if [ -n "$import_ollama" ]; then
    copy="$data/ollama/models"
    if [ -e "$copy" ]; then
      echo "$copy existe déjà : --reset pour repartir d'une copie neuve." >&2
      exit 1
    fi
    size="$(chatonlm_ollama_size "$import_ollama")"
    echo "Copie de $import_ollama ($size) vers $copy, pour la reprise des modèles."
    echo "Le dossier d'origine n'est pas modifié."
    read -r -p "Copier ? [o/N] " answer
    case "$answer" in
      o | O | oui | y | yes) ;;
      *)
        echo "Copie annulée."
        exit 1
        ;;
    esac
    chatonlm_copy_ollama_models "$import_ollama" "$copy" || exit 1
    echo "Copie faite : la reprise des modèles de l'intégré démarre au lancement."
  fi
  echo "Moteur : llama-server intégré."
  # Mode conversation vocale : une ligne « chatonlm-voice-timing » par tour
  # dans ce terminal (millisecondes depuis la fin de parole détectée :
  # transcription, envoi, premier son).
  echo "Mesures du mode vocal : lignes chatonlm-voice-timing ci-dessous."
  if $vscode; then
    editor="$(command -v code || command -v codium || true)"
    if [ -z "$editor" ]; then
      echo "VS Code (code) ou VSCodium (codium) introuvable : extension non lancée." >&2
    else
      profile="$root/.tools/vscode-recipe"
      mkdir -p "$profile/data" "$profile/extensions"
      echo "VS Code : $editor, profil à part $profile, extension $src/vscode"
      # Lancé depuis le terminal intégré d'un VS Code, les variables
      # VSCODE_* et ELECTRON_RUN_AS_NODE feraient ouvrir la fenêtre dans
      # l'éditeur déjà ouvert, sans l'extension : retirées.
      # shellcheck disable=SC2046
      env $(env | sed -n 's/^\(VSCODE_[A-Za-z0-9_]*\)=.*/-u \1/p') -u ELECTRON_RUN_AS_NODE \
        "$editor" --new-window --user-data-dir "$profile/data" \
        --extensions-dir "$profile/extensions" \
        --extensionDevelopmentPath="$src/vscode" "$src" \
        >"$root/.tools/logs/real-vscode.log" 2>&1 &
    fi
  fi
  # Dossiers de l'Ollama de la machine fermés à la reprise, sauf
  # --machine-ollama (variable retirée : recherche normale).
  if $machine_ollama; then
    machine_dirs=(-u CHATONLM_OLLAMA_MACHINE_DIRS)
  else
    machine_dirs=(CHATONLM_OLLAMA_MACHINE_DIRS=)
  fi
  exec env -u CHATONLM_SEED_DEMO \
    -u CHATONLM_TELEGRAM_URL -u CHATONLM_DISCORD_URL -u CHATONLM_HF_URL -u CHATONLM_HF_STATUS_URL \
    -u CHATONLM_LLAMA_SERVER_BIN -u CHATONLM_E2E_MODELS \
    "${machine_dirs[@]}" \
    CHATONLM_ONBOARDING="$($onboarding && echo show || true)" \
    CHATONLM_DATA_DIR="$data" TMPDIR="$root/.tools/tmp" \
    CHATONLM_NO_GLOBAL_SHORTCUTS=1 CHATONLM_VOICE_TIMINGS=1 "$binary"
fi

rm -rf "$data"
mkdir -p "$data"
# Faux moteur : le binaire fake-llama tient lieu de llama-server, sur un
# modèle de démonstration (en-tête GGUF lisible, réponses du faux) inscrit
# au démarrage par CHATONLM_E2E_MODELS (builds de débogage).
demo_model="chatonlm-demo/Demo-chatonlm-think-GGUF:Q4_K_M"
demo_file="chatonlm-demo/Demo-chatonlm-think-GGUF/Demo-Q4_K_M.gguf"
"$target/debug/fake-llama" gguf "$data/models/$demo_file" --tools --think --name Demo
echo "Données de démonstration : $data"
echo "Faux moteur : $target/debug/fake-llama ($demo_model)"
CHATONLM_ONBOARDING="$($onboarding && echo show || echo skip)" \
  CHATONLM_DATA_DIR="$data" CHATONLM_SEED_DEMO=1 \
  CHATONLM_LLAMA_SERVER_BIN="$target/debug/fake-llama" \
  CHATONLM_E2E_MODELS="[{\"id\": \"$demo_model\", \"files\": [\"$demo_file\"]}]" \
  CHATONLM_OLLAMA_MACHINE_DIRS= \
  TMPDIR="$root/.tools/tmp" "$binary"
