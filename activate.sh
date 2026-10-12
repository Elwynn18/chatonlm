# Environnement isolé de ChatonLM (équivalent d'un venv Python).
# Usage : source ./activate.sh
#
# Rust (rustup/cargo), Node (nvm), le cache npm et le CLI GitHub vivent dans
# .tools/ à la racine du projet. Rien n'est écrit ailleurs : supprimer le
# dossier du projet supprime tout.

_chatonlm_root="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"

export RUSTUP_HOME="$_chatonlm_root/.tools/rustup"
export CARGO_HOME="$_chatonlm_root/.tools/cargo"
export NVM_DIR="$_chatonlm_root/.tools/nvm"
export npm_config_cache="$_chatonlm_root/.tools/npm-cache"
# CLI GitHub (optionnel) : binaire et configuration restent aussi dans .tools/.
export GH_CONFIG_DIR="$_chatonlm_root/.tools/gh-config"
export GH_TELEMETRY=false
export GH_NO_UPDATE_NOTIFIER=1
# Caches et états des outils lancés depuis ce shell (gh, WebKitGTK, Mesa,
# fontconfig...). XDG_CONFIG_HOME et XDG_DATA_HOME restent intacts : l'app
# doit lire le thème et les polices de l'utilisateur.
export XDG_CACHE_HOME="$_chatonlm_root/.tools/xdg/cache"
export XDG_STATE_HOME="$_chatonlm_root/.tools/xdg/state"
# Le pilote NVIDIA écrit son cache de calcul dans ~/.nv sans suivre XDG.
export CUDA_CACHE_PATH="$XDG_CACHE_HOME/nvidia/ComputeCache"
# Données de ChatonLM lancé depuis ce shell (base, données WebKit, journaux) :
# l'app les range ici au lieu de ~/.local/share/<identifiant>. Dans une session
# de test (scripts/e2e-session.sh), on garde le dossier neuf de l'essai.
if [ "${CHATONLM_E2E_SESSION:-}" != 1 ]; then
  export CHATONLM_DATA_DIR="$_chatonlm_root/.tools/dev-data"
fi
# ONNX Runtime (dictée Parakeet) des builds sans paquet (dev, essais, e2e,
# demo.sh) : la copie épinglée de scripts/fetch-onnxruntime.sh. Les paquets
# l'installent dans le dossier des ressources (/usr/lib/chatonlm).
export ORT_DYLIB_PATH="$_chatonlm_root/.tools/runtimes/onnxruntime/libonnxruntime.so.1"

for _chatonlm_bin in "$_chatonlm_root/.tools/gh/bin" "$CARGO_HOME/bin"; do
  case ":$PATH:" in
    *":$_chatonlm_bin:"*) ;;
    *) export PATH="$_chatonlm_bin:$PATH" ;;
  esac
done
unset _chatonlm_bin

if [ -s "$NVM_DIR/nvm.sh" ]; then
  . "$NVM_DIR/nvm.sh"
  # Version épinglée par .nvmrc, plutôt que l'alias « default » de nvm.
  if [ -s "$_chatonlm_root/.nvmrc" ]; then
    nvm use --silent "$(tr -d '[:space:]' <"$_chatonlm_root/.nvmrc")" >/dev/null ||
      echo "activate.sh : la version de Node de .nvmrc n'est pas installée (nvm install)." >&2
  fi
fi

# Confirmation dans un terminal interactif seulement (les scripts restent muets).
case $- in
  *i*) echo "Environnement ChatonLM activé (outils et données dans $_chatonlm_root/.tools)." ;;
esac

unset _chatonlm_root
