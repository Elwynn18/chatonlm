# Copie d'un dossier de modèles d'Ollama (manifests/, manifests-v2/, blobs/)
# pour scripts/demo.sh --import-ollama. La reprise des modèles DÉPLACE les
# fichiers de l'Ollama intégré, puis son ménage efface ses blobs : elle doit
# travailler sur une vraie copie. Un `blobs` lié vers un autre disque (courant)
# recopié comme lien ferait travailler la reprise sur les vrais fichiers de
# l'utilisateur : la copie suit donc les liens (`cp -RL`), et elle est refusée
# (effacée) s'il y reste un lien. La source n'est jamais modifiée. À sourcer :
#   source "$root/scripts/lib/ollama-copy.sh"
# shellcheck shell=bash

# Taille du dossier `$1`, liens suivis (ce qui sera copié) : taille des
# fichiers, pas des blocs (indépendante du système de fichiers et des
# écritures pas encore placées sur le disque).
chatonlm_ollama_size() {
  du -shL --apparent-size -- "$1" | cut -f1
}

# Copie `$1` (dossier de modèles d'Ollama) dans `$2`, qui ne doit pas
# exister. Échec : `$2` effacé, message, code 1.
chatonlm_copy_ollama_models() {
  local from="$1" copy="$2" part
  if [ -e "$copy" ] || [ -L "$copy" ]; then
    echo "$copy existe déjà." >&2
    return 1
  fi
  mkdir -p -- "$copy"
  for part in manifests manifests-v2 blobs; do
    if [ -d "$from/$part" ]; then
      if ! cp -RL --preserve=mode,timestamps --reflink=auto -- "$from/$part" "$copy/"; then
        rm -rf -- "$copy"
        echo "Copie de $from/$part impossible (lien mort ?) : copie effacée." >&2
        return 1
      fi
    fi
  done
  if [ -n "$(find "$copy" -type l -print -quit)" ]; then
    rm -rf -- "$copy"
    echo "La copie contient encore un lien symbolique : copie effacée, rien n'est repris." >&2
    return 1
  fi
}
