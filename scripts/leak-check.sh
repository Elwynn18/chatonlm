#!/usr/bin/env bash
# Contrôle de la règle d'isolation : rien ne doit être écrit hors du dossier du
# projet, et aucun processus de test ne doit survivre. Signale ce qui a changé
# dans le dossier personnel et dans /run/user depuis une référence, et marque
# comme suspect ce qui ressemble à une trace des outils du projet.
#
# Usage : scripts/leak-check.sh start                 pose un repère
#         scripts/leak-check.sh report [-v]           depuis le repère
#         scripts/leak-check.sh report --since <epoch> [-v]
#                                                     depuis une date (check.sh
#                                                     --full : dernier commit)
#         scripts/leak-check.sh ack <chemin>...       écriture examinée et
#                                                     reconnue étrangère au
#                                                     projet : plus signalée
#                                                     tant qu'elle ne change pas
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
marker="$root/.tools/leak/marker"
acknowledged="$root/.tools/leak/acknowledged"
app_id="com.chatonlm.chatonlm"
runtime_dir="/run/user/$(id -u)"

# Chemins qui ne doivent jamais apparaître hors du projet. ~/.cache/ms-playwright
# n'en fait plus partie : un autre projet de l'utilisateur l'utilise. Il reste
# surveillé (signature « playwright » plus bas) : signalé SUSPECT dès qu'il
# change, acquittable par « ack » tant qu'il ne change plus. De même pour les
# dossiers de l'app publiée (identifiant sans « .dev »), installée et utilisée
# sur la machine de développement : signature « chatonlm ». Un essai qui y
# écrirait reste donc signalé ; les dossiers de la version de développement,
# que rien d'autre que le projet ne crée, restent interdits.
forbidden=(
  "$HOME/.local/share/$app_id.dev" "$HOME/.config/$app_id.dev"
  "$HOME/.cache/$app_id.dev"
  "$HOME/.cache/tauri" "$HOME/.local/state/gh"
  "$HOME/.gitconfig" "$HOME/.rustup" "$HOME/.nvm" "$HOME/.npm" "$HOME/.node-gyp"
  "$runtime_dir/dbus-1" "$runtime_dir/at-spi/bus"
)

# Dossiers surveillés sur trois niveaux (le bureau y écrit en continu), plus la
# racine du dossier personnel et le dossier d'exécution sur un niveau.
watched=(
  "$HOME/.cache" "$HOME/.config" "$HOME/.local/share" "$HOME/.local/state"
  "$HOME/.ollama" "$HOME/.pki" "$HOME/.nv" "$HOME/.cargo" "$HOME/.ssh"
)
shallow=("$HOME" "$runtime_dir")

# Signatures des outils du projet : une modification qui y correspond est suspecte.
suspect_re='chatonlm|tauri|webkit|ollama|/gh(/|$)|pki|nssdb|cargo|rustup|npm|node-gyp|node_repl|wget-hsts|playwright|chrom|fontconfig|mesa_shader|nvidia|GLCache|ComputeCache|spectacle|kwin|recently-used|dconf|gstreamer|/\.nv(/|$)|/dbus-1(/|$)|at-spi/bus$'

usage() {
  echo "Usage : $0 start | report [--since <epoch>] [-v] | ack <chemin>..." >&2
  exit 2
}

# Vrai si le chemin a été reconnu étranger au projet et n'a pas changé depuis.
is_acknowledged() {
  [ -f "$acknowledged" ] &&
    grep -qxF "$(stat -c %Y "$1" 2>/dev/null)	$1" "$acknowledged"
}

case "${1:-}" in
  start)
    mkdir -p "$(dirname "$marker")"
    touch "$marker"
    echo "Repère posé : $(date '+%F %T')"
    ;;
  ack)
    shift
    [ $# -gt 0 ] || usage
    mkdir -p "$(dirname "$acknowledged")"
    for path in "$@"; do
      printf '%s\t%s\n' "$(stat -c %Y "$path")" "$path" >>"$acknowledged"
      echo "Reconnu étranger au projet : $path"
    done
    ;;
  report)
    shift
    since="" verbose=""
    while [ $# -gt 0 ]; do
      case "$1" in
        --since) since="${2:?}"; shift 2 ;;
        -v) verbose=1; shift ;;
        *) usage ;;
      esac
    done
    if [ -n "$since" ]; then
      newer=(-newermt "@$since")
      label="$(date -d "@$since" '+%F %T')"
    elif [ -f "$marker" ]; then
      newer=(-newer "$marker")
      label="$(date -r "$marker" '+%F %T')"
    else
      echo "Aucun repère : lancez d'abord « $0 start »." >&2
      exit 2
    fi

    echo "Changements hors du projet depuis $label :"
    suspects=0

    for path in "${forbidden[@]}"; do
      if [ -e "$path" ]; then
        echo "  INTERDIT  $path"
        suspects=$((suspects + 1))
      fi
    done

    others=()
    acked=0
    while IFS= read -r path; do
      if [[ "$path" =~ $suspect_re ]]; then
        if is_acknowledged "$path"; then
          acked=$((acked + 1))
        else
          echo "  SUSPECT   $path"
          suspects=$((suspects + 1))
        fi
      else
        others+=("$path")
      fi
    done < <(
      {
        find "${watched[@]}" -xdev -maxdepth 3 "${newer[@]}" 2>/dev/null
        find "${shallow[@]}" -xdev -mindepth 1 -maxdepth 1 "${newer[@]}" 2>/dev/null
      } | sort -u
    )

    # Processus de test restés en vie (sessions virtuelles, app lancée par un
    # essai). L'instance Ollama de test, voulue, n'en fait pas partie.
    while IFS= read -r pid; do
      echo "  PROCESSUS $pid $(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null | cut -c1-80)"
      suspects=$((suspects + 1))
    done < <(grep -lzsE "^(CHATONLM_E2E_SESSION=1|HOME=$root/\.tools/e2e-runs/.*)$" \
      /proc/[0-9]*/environ 2>/dev/null | cut -d/ -f3 | sort -n)

    while IFS= read -r path; do
      echo "  /tmp      $path (effacé au redémarrage)"
    done < <(find /tmp -mindepth 1 -maxdepth 1 -user "$(id -u)" "${newer[@]}" \
      ! -name 'claude-*' 2>/dev/null | sort)

    if [ -n "$verbose" ]; then
      for path in "${others[@]}"; do echo "  bureau    $path"; done
    else
      echo "  (${#others[@]} autres changements, sans lien apparent avec le projet ; -v pour les lister)"
    fi
    [ "$acked" -eq 0 ] || echo "  ($acked changement(s) déjà reconnu(s) étranger(s) au projet)"

    if [ "$suspects" -gt 0 ]; then
      echo "$suspects écriture(s) ou processus suspect(s) : à examiner avant le commit."
      exit 1
    fi
    echo "Aucune écriture suspecte."
    ;;
  *)
    usage
    ;;
esac
