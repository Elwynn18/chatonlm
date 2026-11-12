#!/usr/bin/env bash
# Libère la place prise par ce qui se reconstruit seul : dossiers de
# compilation Rust (src-tauri/target, .tools/*-target, ceux des arbres de
# .tools/wt), anciens artefacts de la CI, restes des essais. Sans option, il
# montre seulement ce qu'il effacerait ; « --apply » efface.
#
#   scripts/clean-space.sh [--apply] [--days N] [--max-size GO]
#
# - Dossier de compilation inutilisé depuis N jours (14 par défaut) : effacé
#   en entier. Encore utilisé mais plus gros que GO gigaoctets (60 par
#   défaut) : effacé aussi (la prochaine compilation repart de zéro). Sinon,
#   seules ses sessions « incremental » de plus de N jours partent.
# - .tools/ci : tout ce qui a plus de N jours, sauf le dossier de la version
#   en cours (tauri.conf.json), les paquets publiés étant sur GitHub.
# - .tools/e2e-runs et .tools/tmp : entrées de plus de N jours.
# Ne touche jamais aux données, aux modèles, aux outils ni aux clés. Refuse de
# tourner pendant une compilation (cargo, rustc).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

apply=0
days=14
max_size=60
while [ $# -gt 0 ]; do
  case "$1" in
    --apply) apply=1 ;;
    --days) days="${2:?--days N}"; shift ;;
    --max-size) max_size="${2:?--max-size GO}"; shift ;;
    -h | --help)
      sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "Option inconnue : $1" >&2
      exit 2
      ;;
  esac
  shift
done

if [ "$apply" = 1 ] && pgrep -x 'cargo|rustc' >/dev/null; then
  echo "Une compilation tourne (cargo ou rustc) : relancer quand elle est finie." >&2
  exit 1
fi

now="$(date +%s)"
limit=$((now - days * 86400))
total=0

# Taille en octets.
size_of() { du -xsb "$1" 2>/dev/null | cut -f1; }
human() { numfmt --to=iec --suffix=o "$1"; }

# Date (secondes) de la dernière écriture près de la racine d'un dossier de
# compilation : une compilation ajoute des entrées dans debug/.fingerprint
# ou release/.fingerprint, ce qui change la date de ces dossiers.
last_use() {
  find "$1" -maxdepth 3 -printf '%T@\n' 2>/dev/null | sort -n | tail -1 | cut -d. -f1
}

remove() {
  local path="$1" why="$2" bytes
  bytes="$(size_of "$path")"
  bytes="${bytes:-0}"
  total=$((total + bytes))
  printf '%10s  %s  (%s)\n' "$(human "$bytes")" "${path#"$root"/}" "$why"
  if [ "$apply" = 1 ]; then
    rm -rf -- "$path"
  fi
}

echo "== Dossiers de compilation"
targets=()
for dir in src-tauri/target .tools/*-target .tools/wt/*/src-tauri/target; do
  [ -d "$dir" ] && targets+=("$dir")
done
for dir in "${targets[@]}"; do
  used="$(last_use "$dir")"
  bytes="$(size_of "$dir")"
  if [ "${used:-0}" -lt "$limit" ]; then
    remove "$dir" "inutilisé depuis $(((now - used) / 86400)) jours"
  elif [ "${bytes:-0}" -gt $((max_size * 1000000000)) ]; then
    remove "$dir" "plus de $max_size Go"
  else
    while IFS= read -r -d '' session; do
      remove "$session" "session incrémentale ancienne"
    # debug/incremental/<crate> ou <cible>/debug/incremental/<crate>.
    done < <(find "$dir" -mindepth 3 -maxdepth 4 -type d -regextype posix-extended \
      -regex '.*/incremental/[^/]+' ! -newermt "@$limit" -print0 2>/dev/null)
  fi
done

echo "== Artefacts de la CI"
version="$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' src-tauri/tauri.conf.json | head -1)"
if [ -d .tools/ci ]; then
  while IFS= read -r -d '' entry; do
    [ "$(basename "$entry")" = "$version" ] && continue
    remove "$entry" "artefact ancien"
  done < <(find .tools/ci -mindepth 1 -maxdepth 1 ! -newermt "@$limit" -print0)
fi

echo "== Restes des essais"
for dir in .tools/e2e-runs .tools/tmp; do
  [ -d "$dir" ] || continue
  while IFS= read -r -d '' entry; do
    remove "$entry" "reste d'essai"
  done < <(find "$dir" -mindepth 1 -maxdepth 1 ! -newermt "@$limit" -print0)
done

echo
if [ "$apply" = 1 ]; then
  echo "Libéré : $(human "$total")."
else
  echo "À libérer : $(human "$total") (relancer avec --apply pour effacer)."
fi
