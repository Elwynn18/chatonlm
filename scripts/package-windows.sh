#!/usr/bin/env bash
# Construit l'installateur Windows de ChatonLM (NSIS, par utilisateur) par le
# CLI Tauri, depuis l'arbre de travail. À lancer sous Windows, dans Git Bash
# (runner windows-2025 de .github/workflows/packages-windows.yml, ou PC de
# développement avec Visual Studio, Rust MSVC, Node et cargo-about).
#   scripts/package-windows.sh [--config <json|fichier>]... [--features <liste>]
#
# Jamais de signature ici, comme scripts/package.sh : ni celle des mises à
# jour (`--no-sign`, variables TAURI_SIGNING_PRIVATE_KEY* retirées de
# l'environnement ; l'installateur fini est signé après coup, à part), ni
# Authenticode (voir « Emplacement de la signature Authenticode » plus bas).
#
# Étapes :
# 1. ONNX Runtime et Ollama intégré pour Windows, épinglés et vérifiés
#    (scripts/fetch-onnxruntime.sh, scripts/fetch-ollama.sh) :
#    .tools/runtimes/onnxruntime/onnxruntime.dll et .tools/runtimes/ollama/
#    (ollama.exe, lib/ollama/…, VERSION) ; moteur d'images construit depuis
#    ses sources (scripts/build-sd-server.sh, SDK Vulkan posé par le
#    workflow) : .tools/runtimes/sd-server/ (sd-server.exe, fallback\,
#    VERSION).
# 2. Frontend (dist/), s'il n'est pas déjà construit, puis une première
#    écriture des notices (scripts/package-notices.mjs) : build.rs copie les
#    ressources de packaging/windows/bundle.conf.json à la compilation, le
#    dossier des notices doit donc exister avant elle. Le CLI refait ensuite
#    les deux (beforeBuildCommand, beforeBundleCommand).
# 3. `tauri build --bundles nsis` avec packaging/windows/bundle.conf.json
#    (ressources : onnxruntime.dll, ollama\, sd-server\, licenses\). Ces ressources ne
#    sont pas dans src-tauri/tauri.windows.conf.json : `cargo check` et
#    `cargo test` n'ont pas à les télécharger.
# 4. Copie de l'installateur dans out/ (inspection : scripts/inspect-windows.mjs).
#
# Les DLL du runtime Visual C++ sont copiées à côté de l'exécutable par le
# bundler (bundle.windows.bundleVCRuntime), depuis le Visual Studio de la
# machine (variable VCTOOLS_REDIST_DIR, sinon vswhere).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) ;;
  *)
    echo "scripts/package-windows.sh se lance sous Windows (Git Bash) : les installateurs sortent de la CI (.github/workflows/packages-windows.yml)." >&2
    exit 2
    ;;
esac

# Options transmises au CLI de Tauri.
tauri_args=()
while [ $# -gt 0 ]; do
  case "$1" in
    --config | --features)
      if [ $# -lt 2 ]; then
        echo "$1 attend une valeur." >&2
        exit 2
      fi
      tauri_args+=("$1" "$2")
      shift
      ;;
    *)
      echo "Usage : $0 [--config <json|fichier>]... [--features <liste>]" >&2
      exit 2
      ;;
  esac
  shift
done
tauri_args+=(--no-sign)
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD TAURI_SIGNING_PRIVATE_KEY_PATH

step() {
  echo "▶ $1"
}

version="$(node -p 'require("./src-tauri/tauri.conf.json").version')"
product="$(node -p 'require("./src-tauri/tauri.windows.conf.json").productName')"
target="${CARGO_TARGET_DIR:-$root/src-tauri/target}"
installer="$target/release/bundle/nsis/${product}_${version}_x64-setup.exe"

step "ONNX Runtime épinglé"
scripts/fetch-onnxruntime.sh
step "Ollama intégré épinglé"
scripts/fetch-ollama.sh
step "Moteur d'images épinglé"
scripts/build-sd-server.sh

# Ce que packaging/windows/bundle.conf.json attend des trois scripts.
ollama="$root/.tools/runtimes/ollama"
missing=()
for file in \
  ".tools/runtimes/onnxruntime/onnxruntime.dll" \
  ".tools/runtimes/ollama/ollama.exe" \
  ".tools/runtimes/ollama/lib/ollama/llama-server.exe" \
  ".tools/runtimes/sd-server/sd-server.exe" \
  ".tools/runtimes/sd-server/fallback/vulkan-1.dll" \
  ".tools/runtimes/sd-server/VERSION"; do
  [ -f "$root/$file" ] || missing+=("$file")
done
if [ "${#missing[@]}" -gt 0 ]; then
  echo "Fichiers attendus absents : ${missing[*]}" >&2
  exit 1
fi
if [ ! -f "$ollama/VERSION" ]; then
  # Lu par l'app (version de l'Ollama intégré) : à défaut, la version épinglée.
  # shellcheck source=../packaging/ollama.env
  source "$root/packaging/ollama.env"
  echo "Avertissement : $ollama/VERSION absent, écrit depuis packaging/ollama.env ($OLLAMA_VERSION)." >&2
  printf '%s\n' "$OLLAMA_VERSION" >"$ollama/VERSION"
fi

if [ ! -f dist/.vite/license.json ]; then
  step "Frontend"
  npm run build
fi
step "Notices (avant la compilation)"
node scripts/package-notices.mjs

# Jamais un installateur ni une signature d'une construction précédente.
rm -f "$installer" "$installer.sig"

step "Construction (nsis)"
node_modules/.bin/tauri build --ci --verbose --bundles nsis \
  --config packaging/windows/bundle.conf.json "${tauri_args[@]}"

if [ ! -f "$installer" ]; then
  echo "Installateur introuvable : $installer" >&2
  ls -l "$(dirname "$installer")" >&2 || true
  exit 1
fi

# Emplacement de la signature Authenticode (aucune aujourd'hui, aucun secret
# ici). Le jour où un certificat existe : chatonlm.exe et uninstall.exe doivent
# être signés PENDANT l'empaquetage (bundle.windows.signCommand, que le modèle
# NSIS applique aussi au désinstalleur), dans un job à part qui reçoit
# l'exécutable construit sans secret, le signe, puis lance
# `tauri bundle --bundles nsis --config <signCommand>` sans recompiler ;
# l'installateur est signé ensuite, puis seulement la signature des mises à
# jour (minisign) : Authenticode modifie le fichier.

mkdir -p out
# Nom publié en minuscules, comme les paquets Linux (chatonlm_<version>_…) :
# c'est celui que scripts/release-manifest.mjs attend et que la signature de
# mise à jour nomme. Tauri l'écrit avec le productName de Windows (ChatonLM).
published="chatonlm_${version}_x64-setup.exe"
cp "$installer" "out/$published"
echo "Installateur : out/$published ($(du -h "$installer" | cut -f1))"
(cd out && sha256sum "$published")
echo "Inspection : node scripts/inspect-windows.mjs \"out/$published\""
