#!/usr/bin/env bash
# Construit les paquets Linux de ChatonLM (.deb et AppImage) par le CLI Tauri,
# depuis l'arbre de travail.
#   scripts/package.sh [--deb|--appimage] [--lgpl] [--config <json|fichier>]...
#                      [--features <liste>]
#
# Jamais de signature ici (`--no-sign`, et les variables
# TAURI_SIGNING_PRIVATE_KEY* retirées de l'environnement) : la clé des mises
# à jour n'entre jamais dans la construction (vite et ses greffons, build.rs,
# cmake, scripts de téléchargement). Les paquets finis sont signés après
# coup, à part : job « sign » de .github/workflows/release.yml, et
# scripts/updater-e2e.sh pour les AppImages d'essai.
# --config et --features sont transmis à `tauri build` (et à la 2e passe) :
# AppImages d'essai des mises à jour (scripts/updater-e2e.sh : version, clé
# publique de test, fonctionnalité e2e).
#
# Les notices (/usr/share/doc/chatonlm) sont écrites avant l'empaquetage par
# scripts/package-notices.mjs (build.beforeBundleCommand), qui demande
# cargo-about à la version épinglée.
#
# Local (Arch) : outils de .tools/ (activate.sh), CARGO_TARGET_DIR dans
# .tools/package-target, NO_STRIP=true (le strip de linuxdeploy ne lit pas la
# section .relr.dyn des bibliothèques d'Arch), TMPDIR dans .tools/tmp, journal
# .tools/logs/package.log, CARGO_BUILD_JOBS=4 par défaut. Ces paquets servent à
# l'inspection et aux essais en session virtuelle : ils ne sont pas portables
# (glibc et bibliothèques d'Arch).
# CI (GITHUB_ACTIONS=true, Ubuntu 22.04) : outils posés par le workflow, sortie
# dans le journal du job ; les paquets publiés viennent de là.
#
# Composants livrés tels quels dans /usr/lib/chatonlm (dossier des ressources
# de Tauri) : ONNX Runtime (scripts/fetch-onnxruntime.sh), l'Ollama intégré
# (scripts/fetch-ollama.sh, archive de 1,4 Go téléchargée une fois) et le
# moteur d'images (scripts/build-sd-server.sh, construit depuis les sources,
# 15 min environ la première fois).
#
# Le runtime de l'AppImage est épinglé (packaging/appimage-runtime.env) :
# téléchargé une fois dans .tools/package/runtime, empreinte vérifiée, puis
# donné à linuxdeploy-plugin-appimage par LDAI_RUNTIME_FILE.
#
# --lgpl (Ubuntu seulement, avec l'AppImage) : après la 1re passe, inventaire
# des bibliothèques de l'AppDir (scripts/appimage-libs.sh), puis 2e passe
# (tauri bundle --bundles appimage) pour que l'inventaire soit dans
# l'AppImage, et contrôle que la 2e passe embarque les mêmes bibliothèques.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

bundles="deb,appimage"
lgpl=false
# Options transmises au CLI de Tauri (build et bundle).
tauri_args=()
while [ $# -gt 0 ]; do
  case "$1" in
    --deb) bundles="deb" ;;
    --appimage) bundles="appimage" ;;
    --lgpl) lgpl=true ;;
    --config | --features)
      if [ $# -lt 2 ]; then
        echo "$1 attend une valeur." >&2
        exit 2
      fi
      tauri_args+=("$1" "$2")
      shift
      ;;
    *)
      echo "Usage : $0 [--deb|--appimage] [--lgpl] [--config <json|fichier>]... [--features <liste>]" >&2
      exit 2
      ;;
  esac
  shift
done
tauri_args+=(--no-sign)
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD TAURI_SIGNING_PRIVATE_KEY_PATH
if $lgpl && [ "$bundles" = "deb" ]; then
  echo "--lgpl ne va qu'avec l'AppImage." >&2
  exit 2
fi

ci=false
[ "${GITHUB_ACTIONS:-}" = true ] && ci=true

if ! $ci; then
  # shellcheck source=../activate.sh
  source "$root/activate.sh"
  # shellcheck source=lib/guard.sh
  source "$root/scripts/lib/guard.sh"
  chatonlm_guard "$root"
  export NO_STRIP=true
  export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
fi
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/.tools/package-target}"
export TMPDIR="$root/.tools/tmp"
mkdir -p "$TMPDIR" "$root/.tools/logs"

log="$root/.tools/logs/package.log"
$ci || : >"$log"

# Étape journalisée : dans le journal local (dernières lignes affichées en cas
# d'échec), ou directement dans celui du job de CI.
run() {
  local name="$1"
  shift
  echo "▶ $name"
  if $ci; then
    "$@"
    return
  fi
  echo "▶ $name" >>"$log"
  if ! "$@" >>"$log" 2>&1; then
    tail -30 "$log" >&2
    echo "Échec : $name (journal : $log)" >&2
    return 1
  fi
}

version="$(node -p 'require("./src-tauri/tauri.conf.json").version')"
bundle_dir="$CARGO_TARGET_DIR/release/bundle"
deb="$bundle_dir/deb/chatonlm_${version}_amd64.deb"
appimage="$bundle_dir/appimage/chatonlm_${version}_amd64.AppImage"
appdir="$bundle_dir/appimage/chatonlm.AppDir"
inventory="$root/.tools/package/appimage-libs"

# Runtime de l'AppImage épinglé, vérifié avant chaque usage.
appimage_runtime() {
  # shellcheck source=../packaging/appimage-runtime.env
  source "$root/packaging/appimage-runtime.env"
  local file="$root/.tools/package/runtime/runtime-x86_64-$TYPE2_RUNTIME_TAG"
  if [ ! -f "$file" ]; then
    mkdir -p "$(dirname "$file")"
    curl -sSfL --retry 3 -o "$file.part" "$TYPE2_RUNTIME_URL"
    mv "$file.part" "$file"
  fi
  if ! echo "$TYPE2_RUNTIME_SHA256  $file" | sha256sum -c --quiet -; then
    rm -f "$file"
    echo "Empreinte inattendue pour le runtime de l'AppImage ($TYPE2_RUNTIME_URL)." >&2
    return 1
  fi
  export LDAI_RUNTIME_FILE="$file"
}
if [ "$bundles" != deb ]; then
  run "Runtime de l'AppImage épinglé" appimage_runtime
fi

# ONNX Runtime officiel, installé par bundle.linux.*.files dans
# /usr/lib/chatonlm (packaging/onnxruntime.env).
run "ONNX Runtime épinglé" scripts/fetch-onnxruntime.sh
# Ollama intégré (archive officielle sans CUDA, ROCm ni MLX), installé par
# bundle.linux.*.files dans /usr/lib/chatonlm/ollama (packaging/ollama.env) ;
# scripts/inspect-packages.mjs vérifie que linuxdeploy n'en modifie aucun
# fichier (empreintes de .tools/runtimes/ollama/SHA256SUMS).
run "Ollama intégré épinglé" scripts/fetch-ollama.sh
# Moteur d'images (sd-server de stable-diffusion.cpp, packaging/sdcpp.env),
# installé de la même façon dans /usr/lib/chatonlm/sd-server.
run "Moteur d'images épinglé" scripts/build-sd-server.sh

# AppImage : linuxdeploy traite chaque fichier ELF de usr/lib (dépendances
# copiées, RPATH, strip) et échoue sur l'Ollama intégré (libggml-vulkan.so
# trouve libggml-base.so.0 par le dossier que lui donne le serveur, pas par
# son RUNPATH). Ses fichiers vont donc dans usr/libexec/chatonlm/ollama, que
# linuxdeploy ignore, et usr/lib/chatonlm/ollama (resource_dir()/ollama, comme
# dans le .deb) est un lien vers ce dossier, copié tel quel par
# bundle.linux.appimage.files. Même chose pour le moteur d'images : son
# chargeur Vulkan de secours ne doit jamais être pris par linuxdeploy pour
# celui du système.
appimage_lib() {
  local dir="$root/.tools/package/appimage-lib"
  rm -rf "$dir"
  mkdir -p "$dir"
  ln -s ../../libexec/chatonlm/ollama "$dir/ollama"
  ln -s ../../libexec/chatonlm/sd-server "$dir/sd-server"
}
run "Liens vers l'Ollama intégré et le moteur d'images (AppImage)" appimage_lib

# Un inventaire d'une construction précédente ne doit jamais entrer dans la
# 1re passe : il décrirait d'autres bibliothèques. tauri-bundler ne vide pas
# le dossier où il prépare l'AppImage (appimage_deb) : le lien
# usr/lib/chatonlm/ollama d'une passe précédente y ferait échouer la copie
# (« File exists »).
rm -rf "$inventory" "$bundle_dir/appimage_deb"
# Signatures comprises : jamais un `.sig` d'une signature précédente à côté
# d'un paquet neuf.
rm -f "$deb" "$appimage" "$deb.sig" "$appimage.sig"

run "Construction ($bundles)" \
  node_modules/.bin/tauri build --ci --verbose --bundles "$bundles" "${tauri_args[@]}"

if $lgpl; then
  run "Inventaire des bibliothèques de l'AppImage (1re passe)" \
    scripts/appimage-libs.sh "$appdir" "$inventory"
  cp "$inventory/files.txt" "$TMPDIR/appimage-files-pass1.txt"
  rm -rf "$bundle_dir/appimage_deb"
  run "AppImage avec l'inventaire (2e passe)" \
    node_modules/.bin/tauri bundle --ci --verbose --bundles appimage "${tauri_args[@]}"
  run "Mêmes bibliothèques dans la 2e passe" \
    scripts/appimage-libs.sh --compare "$TMPDIR/appimage-files-pass1.txt" "$appdir"
fi

echo "Paquets :"
built=()
for file in "$deb" "$appimage"; do
  if [ -f "$file" ]; then
    echo "  $file ($(du -h "$file" | cut -f1))"
    built+=("$file")
  fi
done
echo "Inspection : node scripts/inspect-packages.mjs$(printf ' "%s"' "${built[@]}")"
