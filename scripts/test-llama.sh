#!/usr/bin/env bash
# Vrais modèles d'essai et instance llama-server d'essai, isolés dans
# .tools/llama-test (dossier de l'arbre principal : les arbres de travail
# git le partagent). Rien n'est écrit dans $HOME ; l'Ollama de la machine
# n'est jamais touché.
#
# Usage : scripts/test-llama.sh fetch | check | start [rôle] | stop | status | logs [n] | info
#   fetch         télécharge (ou reprend du dossier de preuve) les GGUF épinglés
#                 par révision et sha256, rangés comme l'app
#                 (<auteur>/<dépôt>/<fichier>), et écrit models.json (lu par
#                 RealBench, `CHATONLM_LLAMA_TEST_MODELS`)
#   check         échoue si un modèle, models.json ou le binaire manque, ou si
#                 CHATONLM_LLAMA_TEST_MODELS désigne un autre dossier (check.sh
#                 --full : sans eux, les contrats réels seraient sautés)
#   start [rôle]  instance pour les e2e réels : 127.0.0.1:11436, clé dans un
#                 fichier 0600 ; rôle : text (défaut), vision, tiny, fim, plain ;
#                 refusé si le port répond déjà (autre serveur)
#   info          adresse, rôle et fichier de la clé de l'instance lancée
# Variables : CHATONLM_LLAMA_TEST_HOME (dossier, défaut .tools/llama-test de
# l'arbre principal), CHATONLM_LLAMA_TEST_BIN (binaire), CHATONLM_LLAMA_TEST_CPU=1
# (processeur seulement), CHATONLM_LLAMA_TEST_CTX (contexte, défaut 4096),
# CHATONLM_LLAMA_TEST_PORT (port, défaut 11436 ; pour les essais du script).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/roots.sh
source "$root/scripts/lib/roots.sh"
main="$(chatonlm_main_root "$root")"

base="$(chatonlm_llama_test_home "$root")"
models="$base/models"
proof="$main/.tools/runtimes/llama-proof/models"
port="${CHATONLM_LLAMA_TEST_PORT:-11436}"
url="http://127.0.0.1:$port"
pidfile="$base/serve.pid"
rolefile="$base/serve.role"
keyfile="$base/api-key"
log="$base/serve.log"

# Modèles épinglés : rôle|identifiant|dépôt|révision|fichier|sha256|sorte
# (sorte : model ou mmproj). Révisions et empreintes relevées sur l'API de
# Hugging Face le 10 nov. 2026.
MODELS=(
  "text|unsloth/Qwen3-0.6B-GGUF:Q4_K_M|unsloth/Qwen3-0.6B-GGUF|50968a4468ef4233ed78cd7c3de230dd1d61a56b|Qwen3-0.6B-Q4_K_M.gguf|ac2d97712095a558e31573f62f466a3f9d93990898b0ec79d7c974c1780d524a|model"
  "vision|unsloth/Qwen3.5-0.8B-GGUF:Q4_K_M|unsloth/Qwen3.5-0.8B-GGUF|6ab461498e2023f6e3c1baea90a8f0fe38ab64d0|Qwen3.5-0.8B-Q4_K_M.gguf|bd258782e35f7f458f8aced1adc053e6e92e89bc735ba3be89d38a06121dc517|model"
  "vision|unsloth/Qwen3.5-0.8B-GGUF:Q4_K_M|unsloth/Qwen3.5-0.8B-GGUF|6ab461498e2023f6e3c1baea90a8f0fe38ab64d0|mmproj-F16.gguf|56e4c6cfe73b0c82e3e82bc518d7591997e61d81f723fc41a586f4fa69ea2453|mmproj"
  "tiny|ggml-org/gemma-3-270m-it-GGUF:Q8_0|ggml-org/gemma-3-270m-it-GGUF|e7647be17ae1108f2f605ed061ca0608b171afff|gemma-3-270m-it-Q8_0.gguf|0ef57d2c838458a1952664260dcba38e5bdda37494f3af732f06e4add24068e3|model"
  "fim|ggml-org/Qwen2.5-Coder-0.5B-Q8_0-GGUF:Q8_0|ggml-org/Qwen2.5-Coder-0.5B-Q8_0-GGUF|5788aee90e725673490be18f553d234feaa8303d|qwen2.5-coder-0.5b-q8_0.gguf|d0f8cd6c49bab52a0abdbe47948518b1f4d9b1a8a2a6825099cea31cea10ac56|model"
  "plain|bartowski/SmolLM2-135M-Instruct-GGUF:Q8_0|bartowski/SmolLM2-135M-Instruct-GGUF|09816acd5d99df7be770d85ea30822623dab342c|SmolLM2-135M-Instruct-Q8_0.gguf|5a1395716f7913741cc51d98581b9b1228d80987a9f7d3664106742eb06bba83|model"
)
ROLES=(text vision tiny fim plain)

server_bin() {
  local candidate
  for candidate in "${CHATONLM_LLAMA_TEST_BIN:-}" "$root/.tools/runtimes/llama-server/llama-server" \
    "$main/.tools/runtimes/llama-server/llama-server"; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then
      echo "$candidate"
      return 0
    fi
  done
  echo "llama-server introuvable : CHATONLM_LLAMA_TEST_BIN, ou scripts/build-llama-server.sh" >&2
  return 1
}

# Témoin d'un fichier vérifié (empreinte et taille), rangé hors du dossier
# des modèles (que les essais donnent à l'app comme dossier des modèles).
stamp_of() {
  echo "$base/verified/${1#"$models"/}.sha256"
}

# Un fichier est bon si son témoin porte la bonne empreinte et que sa
# taille n'a pas changé depuis la vérification.
verified() {
  local dest="$1" sha="$2" stamp
  stamp="$(stamp_of "$dest")"
  [ -f "$dest" ] && [ -f "$stamp" ] || return 1
  [ "$(cut -d' ' -f1 "$stamp")" = "$sha" ] || return 1
  [ "$(cut -d' ' -f2 "$stamp")" = "$(stat -c %s "$dest")" ]
}

fetch_one() {
  local repo="$1" revision="$2" file="$3" sha="$4"
  local dest="$models/$repo/$file"
  if verified "$dest" "$sha"; then
    echo "  $repo/$file : déjà là"
    return 0
  fi
  local stamp
  stamp="$(stamp_of "$dest")"
  mkdir -p "$(dirname "$dest")" "$(dirname "$stamp")"
  rm -f "$stamp"
  if [ -f "$dest" ] && [ "$(sha256sum "$dest" | cut -d' ' -f1)" = "$sha" ]; then
    echo "  $repo/$file : déjà là (vérifié)"
    echo "$sha $(stat -c %s "$dest")" >"$stamp"
    return 0
  fi
  local part="$dest.part"
  if [ -f "$proof/$file" ] && [ "$(sha256sum "$proof/$file" | cut -d' ' -f1)" = "$sha" ]; then
    echo "  $repo/$file : repris du dossier de preuve"
    cp "$proof/$file" "$part"
  else
    echo "  $repo/$file : téléchargement"
    local progress=-sS
    [ -t 2 ] && progress=--progress-bar
    curl "$progress" -fL --retry 3 --retry-delay 2 --connect-timeout 20 -C - -o "$part" \
      "https://huggingface.co/$repo/resolve/$revision/$file"
  fi
  local got
  got="$(sha256sum "$part" | cut -d' ' -f1)"
  if [ "$got" != "$sha" ]; then
    rm -f "$part"
    echo "Empreinte fausse pour $repo/$file : $got (attendu $sha)" >&2
    return 1
  fi
  mv -f "$part" "$dest"
  echo "$sha $(stat -c %s "$dest")" >"$stamp"
}

# models.json : {version: 1, models: [{role, id, files, mmproj, sha256, revision}]}
# (chemins relatifs au dossier des modèles, sha256 par fichier).
write_models_json() {
  local out="$models/models.json.tmp" first_role=1 role line
  {
    printf '{\n  "version": 1,\n  "models": ['
    for role in "${ROLES[@]}"; do
      local id="" revision="" files="" mmproj="null" shas=""
      for line in "${MODELS[@]}"; do
        IFS='|' read -r r i repo rev file sha kind <<<"$line"
        [ "$r" = "$role" ] || continue
        id="$i"
        revision="$rev"
        if [ "$kind" = mmproj ]; then
          mmproj="\"$repo/$file\""
        else
          files="${files:+$files, }\"$repo/$file\""
        fi
        shas="${shas:+$shas, }\"$repo/$file\": \"$sha\""
      done
      [ "$first_role" = 1 ] || printf ','
      first_role=0
      printf '\n    {"role": "%s", "id": "%s", "files": [%s], "mmproj": %s, "sha256": {%s}, "revision": "%s"}' \
        "$role" "$id" "$files" "$mmproj" "$shas" "$revision"
    done
    printf '\n  ]\n}\n'
  } >"$out"
  mv -f "$out" "$models/models.json"
}

# Ligne du modèle (pas du projecteur) d'un rôle, et son projecteur.
role_file() {
  local want="$1" wanted_kind="$2" line
  for line in "${MODELS[@]}"; do
    IFS='|' read -r r i repo rev file sha kind <<<"$line"
    if [ "$r" = "$want" ] && [ "$kind" = "$wanted_kind" ]; then
      echo "$i|$models/$repo/$file|$sha"
      return 0
    fi
  done
  return 1
}

# Arguments du processus `$1` dans le tableau `argv` de l'appelant (vide
# si le processus est mort ou zombie).
read_argv() {
  argv=()
  mapfile -d '' -t argv <"/proc/$1/cmdline" 2>/dev/null || true
}

# Vrai si le processus `$1` est fini : absent, ou zombie (état « Z » de
# /proc/<pid>/stat, lu après la dernière parenthèse : le nom peut en
# contenir). Une ligne de commande vide ne suffit pas : pendant un `exec`,
# /proc/<pid>/cmdline est vide un instant (nouvelle mémoire pas encore
# remplie), d'autant plus longtemps que la machine est chargée.
gone() {
  local stat
  stat="$(cat "/proc/$1/stat" 2>/dev/null)" || return 0
  [ -n "$stat" ] || return 0
  stat="${stat##*) }"
  [ "${stat%% *}" = Z ]
}

# Vrai si les arguments `argv` contiennent l'option `$1` suivie de la
# valeur `$2`.
has_pair() {
  local i
  for ((i = 0; i + 1 < ${#argv[@]}; i++)); do
    if [ "${argv[i]}" = "$1" ] && [ "${argv[i + 1]}" = "$2" ]; then
      return 0
    fi
  done
  return 1
}

running() {
  [ -f "$pidfile" ] || return 1
  local pid
  pid="$(cat "$pidfile")"
  # Le PID enregistré peut avoir été repris par un autre programme : on
  # vérifie que c'est bien notre serveur, à ses arguments (« --port » et
  # « --api-key-file » de cette instance), pas au nom du binaire (un lien
  # d'un autre nom ne doit pas rendre l'instance invisible à « stop »).
  local -a argv=()
  read_argv "$pid"
  if has_pair --port "$port" && has_pair --api-key-file "$keyfile"; then
    return 0
  fi
  rm -f "$pidfile" "$rolefile"
  return 1
}

healthy() {
  curl -sf --max-time 2 "$url/health" >/dev/null
}

# Vrai si quelque chose écoute déjà sur le port (connexion acceptée).
port_taken() {
  (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null
}

case "${1:-status}" in
  check)
    missing=0
    for line in "${MODELS[@]}"; do
      IFS='|' read -r _ _ repo _ file sha _ <<<"$line"
      if ! verified "$models/$repo/$file" "$sha"; then
        echo "Modèle d'essai absent ou non vérifié : $models/$repo/$file" >&2
        missing=1
      fi
    done
    if [ ! -f "$models/models.json" ]; then
      echo "models.json absent : $models" >&2
      missing=1
    fi
    if [ -n "${CHATONLM_LLAMA_TEST_MODELS:-}" ] && [ "$CHATONLM_LLAMA_TEST_MODELS" != "$models" ]; then
      echo "CHATONLM_LLAMA_TEST_MODELS ($CHATONLM_LLAMA_TEST_MODELS) n'est pas le dossier rempli par fetch ($models)." >&2
      missing=1
    fi
    if [ -n "${CHATONLM_LLAMA_TEST_BIN:-}" ]; then
      if [ ! -x "$CHATONLM_LLAMA_TEST_BIN" ]; then
        echo "CHATONLM_LLAMA_TEST_BIN introuvable : $CHATONLM_LLAMA_TEST_BIN" >&2
        missing=1
      fi
    elif ! server_bin >/dev/null; then
      missing=1
    fi
    if [ "$missing" = 1 ]; then
      echo "Vrai moteur d'essai incomplet : « $0 fetch », scripts/build-llama-server.sh (sans eux, les contrats réels seraient sautés)." >&2
      exit 1
    fi
    echo "Modèles d'essai et llama-server présents ($models)."
    ;;
  fetch)
    mkdir -p "$models"
    echo "Modèles d'essai dans $models"
    for line in "${MODELS[@]}"; do
      IFS='|' read -r _ _ repo revision file sha _ <<<"$line"
      fetch_one "$repo" "$revision" "$file" "$sha"
    done
    write_models_json
    echo "Écrit : $models/models.json"
    ;;
  start)
    role="${2:-text}"
    case " ${ROLES[*]} " in
      *" $role "*) ;;
      *)
        echo "Rôle inconnu : $role (${ROLES[*]})" >&2
        exit 2
        ;;
    esac
    if running; then
      echo "Déjà lancée (PID $(cat "$pidfile"), rôle $(cat "$rolefile" 2>/dev/null || echo ?)) sur $url"
      exit 0
    fi
    # Un autre serveur (un llama-server de l'app, celui de l'utilisateur)
    # répondrait à notre place à /health : refusé avant de lancer quoi que
    # ce soit.
    if port_taken; then
      echo "Le port $port est déjà pris par un autre programme : instance d'essai non lancée." >&2
      exit 1
    fi
    bin="$(server_bin)"
    IFS='|' read -r id model sha <<<"$(role_file "$role" model)"
    if ! verified "$model" "$sha"; then
      echo "Modèle $id absent : « $0 fetch » d'abord." >&2
      exit 1
    fi
    args=(--host 127.0.0.1 --port "$port" --api-key-file "$keyfile" --alias "$id"
      -m "$model" -c "${CHATONLM_LLAMA_TEST_CTX:-4096}" -np 1 --no-webui --offline)
    if projector="$(role_file "$role" mmproj)"; then
      IFS='|' read -r _ mmproj _ <<<"$projector"
      args+=(--mmproj "$mmproj")
    fi
    if [ "${CHATONLM_LLAMA_TEST_CPU:-}" = 1 ]; then
      args+=(-ngl 0)
    fi
    mkdir -p "$base/home" "$base/cache" "$base/tmp"
    if [ ! -s "$keyfile" ]; then
      (umask 077 && head -c 24 /dev/urandom | base64 | tr -d '/+=\n' >"$keyfile")
    fi
    chmod 600 "$keyfile"
    # Variables de llama.cpp et de ggml héritées du bureau : retirées, comme
    # le fait l'app (`engine::options::server_env`).
    unset_args=()
    while IFS= read -r name; do
      unset_args+=(-u "$name")
    done < <(compgen -e | grep -E '^(LLAMA_|GGML_)' || true)
    echo "--- $(date '+%F %T') : $bin (rôle $role)" >>"$log"
    (
      # Descripteurs hérités au-delà de 0, 1, 2 fermés (le verrou d'un
      # appelant, `exec 9>lock; flock 9` de record-fixtures.sh) : le serveur
      # ne doit pas garder le verrou des vrais serveurs après la fin du
      # script qui l'a pris.
      for fd in /proc/self/fd/*; do
        fd="${fd##*/}"
        if [ "$fd" -gt 2 ]; then eval "exec $fd>&-"; fi
      done
      exec env "${unset_args[@]}" HOME="$base/home" LLAMA_CACHE="$base/cache" XDG_CACHE_HOME="$base/cache" \
        TMPDIR="$base/tmp" setsid "$bin" "${args[@]}" >>"$log" 2>&1 </dev/null
    ) &
    pid=$!
    echo "$pid" >"$pidfile"
    echo "$role" >"$rolefile"
    for _ in $(seq 1 240); do
      # Notre processus d'abord : mort (ou zombie), ce qui répondrait sur le
      # port ne serait pas lui. Tant qu'il n'a pas encore pris ses
      # arguments (sous-shell avant `exec`, ou `exec` en cours : ligne de
      # commande vide un instant), on attend.
      read_argv "$pid"
      if gone "$pid"; then
        rm -f "$pidfile" "$rolefile"
        echo "llama-server s'est arrêté ; voir $log" >&2
        exit 1
      fi
      if has_pair --port "$port" && has_pair --api-key-file "$keyfile" && healthy; then
        echo "Lancée (PID $pid, rôle $role, $id) sur $url"
        exit 0
      fi
      sleep 0.5
    done
    kill -TERM "$pid" 2>/dev/null || true
    rm -f "$pidfile" "$rolefile"
    echo "Pas prête après 120 s (arrêtée) ; voir $log" >&2
    exit 1
    ;;
  stop)
    if ! running; then
      echo "Pas lancée."
      rm -f "$pidfile" "$rolefile"
      exit 0
    fi
    pid="$(cat "$pidfile")"
    kill -TERM "$pid"
    for _ in $(seq 1 20); do
      kill -0 "$pid" 2>/dev/null || {
        rm -f "$pidfile" "$rolefile"
        echo "Arrêtée."
        exit 0
      }
      sleep 0.5
    done
    kill -KILL "$pid" 2>/dev/null || true
    rm -f "$pidfile" "$rolefile"
    echo "Arrêtée (forcée)."
    ;;
  status)
    if running && healthy; then
      echo "En marche (PID $(cat "$pidfile"), rôle $(cat "$rolefile" 2>/dev/null || echo ?)) sur $url"
    else
      echo "Arrêtée."
      exit 1
    fi
    ;;
  info)
    running || {
      echo "Arrêtée." >&2
      exit 1
    }
    echo "url=$url"
    echo "role=$(cat "$rolefile" 2>/dev/null || echo ?)"
    echo "key_file=$keyfile"
    ;;
  logs)
    tail -n "${2:-50}" "$log"
    ;;
  *)
    echo "Usage : $0 fetch | check | start [rôle] | stop | status | logs [n] | info" >&2
    exit 2
    ;;
esac
