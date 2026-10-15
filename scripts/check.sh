#!/usr/bin/env bash
# Contrôles d'une étape, à passer avant chaque commit.
# Par défaut : outils, scripts, format, types, lint, tests, build du frontend
# et contenu du bundle, licences et attribution. « --full » ajoute le build de
# l'app, les contrats avec l'Ollama de test (vrais modèles, port 11435) et le
# registre ollama.com (accès au réseau), deux fois : avec l'Ollama de la
# machine, puis avec l'Ollama intégré aux paquets (packaging/ollama.env),
# l'essai de démarrage et les tests de l'app en session virtuelle, la mise à
# jour N → N+1 sur deux AppImages d'essai, puis le contrôle d'isolation depuis
# le dernier commit.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

full=false
case "${1:-}" in
  "") ;;
  --full) full=true ;;
  *)
    echo "Usage : $0 [--full]" >&2
    exit 2
    ;;
esac

# shellcheck source=../activate.sh
source "$root/activate.sh"
# shellcheck source=lib/guard.sh
source "$root/scripts/lib/guard.sh"
cd "$root"

manifest=src-tauri/Cargo.toml
export TMPDIR="$root/.tools/tmp"
mkdir -p "$TMPDIR"

step() {
  local name="$1"
  shift
  local start
  start=$(date +%s)
  echo "▶ $name"
  if "$@"; then
    echo "  ✓ $name ($(($(date +%s) - start)) s)"
  else
    echo "  ✗ $name" >&2
    exit 1
  fi
}

# Outils du projet uniquement : jamais de téléchargement implicite par npx.
bin() {
  local tool="$1"
  shift
  "$root/node_modules/.bin/$tool" "$@"
}

shell_syntax() {
  local file
  for file in activate.sh scripts/*.sh scripts/lib/*.sh e2e/*.sh packaging/ci/*.sh; do
    bash -n "$file" || return 1
  done
}

npm_tree_valid() {
  # Échoue si un paquet manque ou ne correspond pas à package-lock.json.
  npm ls --all >/dev/null
}

bindings_up_to_date() {
  # Fichiers écrits par « cargo test » : types générés (ts-rs) et séquences du
  # moteur de chat rejouées par le simulateur du frontend. Ils doivent être
  # ajoutés (git add) et identiques à l'index.
  local generated=(src/bindings src/test/fixtures)
  git diff --quiet -- "${generated[@]}" &&
    [ -z "$(git ls-files --others --exclude-standard -- "${generated[@]}")" ] || {
    git status --short -- "${generated[@]}"
    echo "Types ou séquences générés par les tests Rust modifiés ou non ajoutés :" \
      "git add ${generated[*]}" >&2
    return 1
  }
}

packaging_files() {
  # Entrées de menu et métadonnées AppStream des paquets, par les validateurs
  # du système s'ils sont installés (desktop-file-utils, appstream) ; la CI des
  # paquets les installe et les relance sur les paquets construits.
  local template="$TMPDIR/chatonlm.desktop"
  if command -v desktop-file-validate >/dev/null; then
    desktop-file-validate packaging/linux/com.chatonlm.chatonlm.desktop || return 1
    sed 's/{{exec}}/chatonlm/; s/{{icon}}/chatonlm/' packaging/linux/chatonlm.desktop.in >"$template"
    desktop-file-validate "$template" || return 1
  fi
  if command -v appstreamcli >/dev/null; then
    appstreamcli validate --no-net packaging/linux/com.chatonlm.chatonlm.metainfo.xml >/dev/null ||
      return 1
  fi
}

# Conditions réelles : l'instance Ollama de test (port 11435, jamais le service
# du système) sert aux contrats, à e2e/real-ollama.test.mjs et à
# e2e/real-voice-mode.test.mjs (un tour du mode vocal avec qwen3:0.6b, latence
# affichée sur la sortie d'erreur), lancés avec les autres essais de l'app
# (real_app_checks). Démarrée ici si
# elle ne tournait pas, elle est alors arrêtée à la fin, même en cas d'échec.
# Lancée avec l'Ollama intégré (reste d'un essai), elle est relancée avec
# celui de la machine.
test_ollama_started=false
start_test_ollama() {
  if scripts/test-ollama.sh status >/dev/null 2>&1; then
    [ "$(scripts/test-ollama.sh kind)" = system ] && return 0
    scripts/test-ollama.sh stop >/dev/null || return 1
  else
    test_ollama_started=true
    trap 'scripts/test-ollama.sh stop >/dev/null || true' EXIT
  fi
  scripts/test-ollama.sh start >/dev/null
}

# Un contrat à la fois : en parallèle, un contrat qui recharge qwen3:0.6b
# avec un autre contexte tue le runner du contrat d'annulation avant qu'il
# n'écrive l'abandon dans le journal (Ollama 0.40.0 : 1 échec sur 3 avec le
# binaire de la machine, 2 sur 4 avec l'intégré ; en série, ~100 s au lieu
# de ~80 s, aucun).
# Les contrats de la lecture à voix haute (tts_contract_*) prennent les voix
# de .tools/voice-models (scripts/tts/README.md) ; sans elles, ils sont sautés.
contracts() {
  local voices="$root/.tools/voice-models"
  if [ -d "$voices" ]; then
    CHATONLM_VOICE_MODELS_DIR="$voices" \
      cargo test --manifest-path "$manifest" --locked --lib contract -- --ignored --test-threads=1
  else
    echo "  (voix absentes : contrats de la lecture à voix haute sautés)"
    env -u CHATONLM_VOICE_MODELS_DIR \
      cargo test --manifest-path "$manifest" --locked --lib contract -- --ignored --test-threads=1
  fi
}

# Mêmes contrats avec l'Ollama intégré aux paquets : l'instance de test est
# relancée avec ce binaire (même port, mêmes modèles) ; il doit annoncer la
# version épinglée et décrire ses moteurs de calcul dans son journal (Vulkan
# ou processeur : noté dans la sortie). L'instance est ensuite rendue à
# l'Ollama de la machine pour les essais de l'app, même en cas d'échec.
builtin_contracts() {
  local status=0 version log
  scripts/test-ollama.sh stop >/dev/null || return 1
  log="$(wc -l <.tools/ollama-test/serve.log 2>/dev/null || echo 0)"
  if scripts/test-ollama.sh start --builtin; then
    version="$(curl -sf http://127.0.0.1:11435/api/version)"
    if [ "$version" != "{\"version\":\"$(cat .tools/runtimes/ollama/VERSION)\"}" ]; then
      echo "Version annoncée par l'Ollama intégré : $version" >&2
      status=1
    fi
    tail -n "+$((log + 1))" .tools/ollama-test/serve.log |
      grep -m 3 'msg="inference compute"' | sed -E 's/.*(library=[^ ]+).*(description="[^"]*").*/  moteur : \1 \2/' ||
      { echo "Aucun moteur de calcul décrit par l'Ollama intégré." >&2 && status=1; }
    contracts || status=1
  else
    status=1
  fi
  scripts/test-ollama.sh stop >/dev/null || status=1
  scripts/test-ollama.sh start >/dev/null || status=1
  return "$status"
}

# Contrats avec le vrai moteur d'images (image_gen::tests::real) : le binaire
# des paquets, sur le processeur seul (pilote Vulkan masqué), en petites
# images. Il faut les modèles du premier essai (FLUX.2 klein 4B, voir l'en-tête
# du module) : dossier CHATONLM_SD_REAL_MODELS, .tools/sdcpp/models par défaut ;
# absents, l'étape le dit et passe.
sd_contracts() {
  local models="${CHATONLM_SD_REAL_MODELS:-$root/.tools/sdcpp/models}"
  if [ ! -f "$models/flux-2-klein-4b-Q4_0.gguf" ]; then
    echo "Modèles d'images absents ($models) : contrats du moteur d'images non lancés."
    return 0
  fi
  CHATONLM_SD_REAL_BIN="$root/.tools/runtimes/sd-server/sd-server" CHATONLM_SD_REAL_MODELS="$models" \
    cargo test --manifest-path "$manifest" --lib --locked image_gen::tests::real -- --ignored --test-threads=1
}

real_app_checks() {
  # Binaire d'essai (frontend embarqué, serveur WebDriver), puis démarrage
  # vérifié à l'écran et tests WebDriver dans la vraie webview.
  local binary test
  binary="$(scripts/e2e-build.sh)" || return 1
  scripts/e2e-session.sh e2e/launch.sh "$binary" check-launch >/dev/null || return 1
  for test in e2e/*.test.mjs; do
    # Mises à jour : AppImages d'essai à part (update_checks).
    if [ "$test" = e2e/update.test.mjs ]; then continue; fi
    echo "  · $test"
    scripts/e2e-session.sh e2e/run.sh "$test" "$binary" >/dev/null || return 1
  done
}

update_checks() {
  # Mise à jour N → N+1 (e2e/update.test.mjs) : deux AppImages d'essai
  # construites en release avec la clé de test (scripts/updater-e2e.sh,
  # journal .tools/logs/updater-e2e.log), plusieurs lancements et relances
  # de l'app ; journal de l'essai dans .tools/logs/check-update-e2e.log.
  local dir log=.tools/logs/check-update-e2e.log
  dir="$(scripts/updater-e2e.sh)" || return 1
  E2E_TIMEOUT=900 scripts/e2e-session.sh e2e/run.sh e2e/update.test.mjs "$dir" >"$log" 2>&1 || {
    grep -v '^dbus-daemon\|^$\|xdg-desktop-portal' "$log" | tail -40 >&2
    return 1
  }
}

step "Outils de .tools/ aux versions épinglées" chatonlm_guard "$root"
step "Dépendances npm conformes au verrou" npm_tree_valid
# ONNX Runtime épinglé, chargé à l'exécution par la dictée (ORT_DYLIB_PATH,
# activate.sh) : essais Rust et e2e.
step "ONNX Runtime épinglé" scripts/fetch-onnxruntime.sh
step "Syntaxe des scripts shell" shell_syntax
step "Format Rust" cargo fmt --manifest-path "$manifest" --all --check
step "Types TypeScript" bin tsc -b
step "ESLint" bin eslint .
# Les fichiers de travail locaux sont exclus par .git/info/exclude, que Prettier
# ne lit pas de lui-même.
step "Prettier" bin prettier --check . --ignore-path .gitignore --ignore-path .prettierignore --ignore-path .git/info/exclude
step "Traductions (en, es, fr)" node scripts/check-i18n.mjs
step "Tests TypeScript" bin vitest run
step "Clippy" cargo clippy --manifest-path "$manifest" --workspace --all-targets --locked -- -D warnings
# L'app seule, comme la compilent tauri build et l'empaquetage : sans les
# fonctionnalités que les autres crates de l'espace de travail (fake-ollama...)
# activent dans leurs dépendances communes.
step "App seule (hors espace de travail)" \
  cargo check --manifest-path "$manifest" --locked -p chatonlm --lib --bins
# Même code pour Windows : contrôle croisé (rien n'est lié ni exécuté), pour
# qu'un changement fait sous Linux ne casse pas la version Windows. La
# construction réelle, les essais et l'installateur : CI (packages-windows.yml).
step "Clippy pour Windows (contrôle croisé)" \
  scripts/check-windows.sh clippy --workspace --all-targets
step "Tests Rust" cargo test --manifest-path "$manifest" --workspace --locked
if [ -d src/bindings ] || [ -d src/test/fixtures ]; then
  step "Types et séquences générés à jour" bindings_up_to_date
fi
step "Build du frontend" bin vite build
step "Contenu du bundle" node scripts/check-bundle.mjs
step "Licences Rust" cargo deny --manifest-path "$manifest" check licenses
step "Licences npm (paquets embarqués)" node scripts/check-npm-licenses.mjs
step "Attribution Ollama" node scripts/check-headers.mjs
step "Fichiers d'empaquetage (desktop, AppStream)" packaging_files

if $full; then
  step "Clippy (fonctionnalité e2e)" \
    cargo clippy --manifest-path "$manifest" --all-targets --locked --features e2e -- -D warnings
  step "Ollama intégré épinglé" scripts/fetch-ollama.sh
  step "Moteur d'images épinglé (sd-server)" scripts/build-sd-server.sh
  step "Contrats avec le moteur d'images (processeur)" sd_contracts
  step "Ollama de test démarré (port 11435)" start_test_ollama
  step "Contrats avec l'Ollama de test et le registre" contracts
  step "Contrats avec l'Ollama intégré" builtin_contracts
  step "App réelle en session virtuelle" real_app_checks
  step "Mise à jour N → N+1 en session virtuelle" update_checks
  if $test_ollama_started; then
    step "Ollama de test arrêté" scripts/test-ollama.sh stop
    trap - EXIT
  fi
  step "Isolation depuis le dernier commit" \
    scripts/leak-check.sh report --since "$(git log -1 --format=%ct)"
fi

echo "Tous les contrôles sont passés."
