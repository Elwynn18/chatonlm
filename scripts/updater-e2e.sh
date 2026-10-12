#!/usr/bin/env bash
# AppImages d'essai des mises à jour (e2e/update.test.mjs) : ChatonLM 0.0.1 et
# 0.0.2, fonctionnalité « e2e » (serveur WebDriver, CHATONLM_UPDATE_URL lue),
# clé publique de TEST à la place de celle de production, adresse http
# permise (faux serveur local) ; la 0.0.2 est signée après coup par la clé de
# test (`tauri signer sign --app-version`, comme le job « sign » de
# release.yml : package.sh ne signe jamais). Les surcharges passent par
# `--config` : rien de tout cela n'entre dans les paquets publiés. Affiche le dossier des AppImages :
#   scripts/e2e-session.sh e2e/run.sh e2e/update.test.mjs "$(scripts/updater-e2e.sh)"
#
# Construction en release par scripts/package.sh --appimage (runtime épinglé,
# ONNX Runtime, notices), dans .tools/updater-e2e-target ; sortie dans
# .tools/updater-e2e/ ; journal .tools/logs/updater-e2e.log. Clé de test
# créée au besoin (scripts/updater-keys.sh --test). Ces AppImages ne sont pas
# portables (bibliothèques d'Arch) : seulement pour la session virtuelle.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
# shellcheck source=../activate.sh
source "$root/activate.sh"
# shellcheck source=lib/guard.sh
source "$root/scripts/lib/guard.sh"
chatonlm_guard "$root"

keys="$root/.tools/updater-test"
out="$root/.tools/updater-e2e"
log="$root/.tools/logs/updater-e2e.log"
export CARGO_TARGET_DIR="$root/.tools/updater-e2e-target"
export TMPDIR="$root/.tools/tmp"
mkdir -p "$out" "$TMPDIR" "$root/.tools/logs"
: >"$log"

scripts/updater-keys.sh --test >>"$log" 2>&1
pubkey="$(cat "$keys/chatonlm.key.pub")"
bundle="$CARGO_TARGET_DIR/release/bundle/appimage"

for version in 0.0.1 0.0.2; do
  config="$TMPDIR/updater-e2e-$version.json"
  node -e '
    const [version, pubkey, file] = process.argv.slice(1);
    require("node:fs").writeFileSync(file, JSON.stringify({
      version,
      plugins: { updater: { pubkey, dangerousInsecureTransportProtocol: true } },
    }));
  ' "$version" "$pubkey" "$config"
  echo "▶ ChatonLM $version" >>"$log"
  if ! scripts/package.sh --appimage --config "$config" --features e2e >>"$log" 2>&1; then
    tail -30 "$log" >&2
    exit 1
  fi
  image="$bundle/chatonlm_${version}_amd64.AppImage"
  if [ ! -f "$image" ]; then
    echo "AppImage absente : $image (journal : $log)" >&2
    exit 1
  fi
  rm -f "$out/chatonlm_${version}_amd64.AppImage" "$out/chatonlm_${version}_amd64.AppImage.sig"
  cp "$image" "$out/"
  if [ "$version" = 0.0.2 ]; then
    # Clé privée de test donnée par son chemin, mot de passe par
    # l'environnement : jamais affichés ni sur la ligne de commande.
    if ! TAURI_SIGNING_PRIVATE_KEY_PATH="$keys/chatonlm.key" \
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat "$keys/password")" \
      node_modules/.bin/tauri signer sign --app-version "$version" \
      "$out/chatonlm_${version}_amd64.AppImage" >>"$log" 2>&1; then
      tail -30 "$log" >&2
      exit 1
    fi
    if [ ! -f "$out/chatonlm_${version}_amd64.AppImage.sig" ]; then
      echo "Signature absente (journal : $log)" >&2
      exit 1
    fi
  fi
done
cp "$keys/chatonlm.key.pub" "$out/test.key.pub"
echo "$out"
