#!/usr/bin/env bash
# Bibliothèques du système embarquées dans l'AppImage (linuxdeploy et son
# greffon GTK les copient depuis la machine de construction) : inventaire et
# obligations de la LGPL. Écrit pour Ubuntu (CI, dpkg) ; --list et --compare
# marchent partout.
#
#   scripts/appimage-libs.sh <AppDir> <dossier>
#       Inventaire : pour chaque fichier ELF de l'AppDir (hors ChatonLM), paquet
#       Ubuntu d'origine (dpkg -S), paquet source et version, licence (fichier
#       copyright du paquet). Écrit dans <dossier> : libraries.txt (notice
#       lisible, livrée dans l'AppImage), packages.tsv, files.txt, sources.txt
#       (paquets source dont il faut publier le code), copyright/<paquet>,
#       common-licenses/<licence> (textes cités par les fichiers copyright).
#       Échec si un fichier n'a pas de paquet, si un paquet n'a pas de fichier
#       copyright, ou si une bibliothèque n'est que sous GPL (sans LGPL ni
#       licence permissive) : l'inventaire est écrit quand même, pour le
#       diagnostic.
#   scripts/appimage-libs.sh --list <AppDir>
#       Fichiers ELF embarqués (chemins relatifs à l'AppDir), triés.
#   scripts/appimage-libs.sh --compare <files.txt> <AppDir>
#       Échec si l'AppDir n'embarque pas exactement les fichiers de la liste.
#   scripts/appimage-libs.sh --classify <fichier copyright>
#       Classe de licence d'un fichier copyright de Debian : lgpl, permissive,
#       mixed (GPL pour une partie, licence permissive pour une autre),
#       gpl-exception (GPL avec exception, ex. bibliothèques de GCC), gpl-only.
#   scripts/appimage-libs.sh --sources <dossier de l'inventaire> <archive.tar>
#       Code source des paquets de sources.txt (apt-get source --download-only,
#       dépôts deb-src activés), réuni dans une archive tar avec son index,
#       à publier à côté de l'AppImage (LGPL-2.1 § 4 et 6, GPL-3.0 § 6 d).
set -euo pipefail

# Le binaire de ChatonLM, les lanceurs ajoutés par linuxdeploy (AppRun) et les
# composants que ChatonLM livre lui-même dans son dossier des ressources
# (usr/lib/chatonlm : ONNX Runtime officiel ; usr/libexec/chatonlm : l'Ollama
# intégré, hors de portée de linuxdeploy ; licences dans THIRD_PARTY_LICENSES
# et licenses/) ne viennent pas de paquets du système.
OWN_FILES_RE='^(usr/bin/chatonlm|AppRun(\.wrapped)?|usr/lib/chatonlm/.+|usr/libexec/chatonlm/.+)$'
# Dossiers de Debian (remplacés seulement par les essais, avec un faux dpkg).
DOC_ROOT="${APPIMAGE_LIBS_DOC_ROOT:-/usr/share/doc}"
LICENSES_ROOT="${APPIMAGE_LIBS_LICENSES_ROOT:-/usr/share/common-licenses}"

is_elf() {
  [ "$(head -c 4 "$1" | od -An -tx1 | tr -d ' \n')" = 7f454c46 ]
}

list_elf() {
  local appdir="$1" file rel
  [ -d "$appdir" ] || {
    echo "AppDir introuvable : $appdir" >&2
    return 1
  }
  while IFS= read -r -d '' file; do
    rel="${file#"$appdir"/}"
    [[ "$rel" =~ $OWN_FILES_RE ]] && continue
    if is_elf "$file"; then printf '%s\n' "$rel"; fi
  done < <(find "$appdir" -type f -print0)
}

list_sorted() {
  list_elf "$1" | LC_ALL=C sort
}

# --- Classes de licence ------------------------------------------------------

# Classe d'un nom de licence court (champ License: du format DEP-5).
license_kind() {
  local name="${1,,}"
  case "$name" in
    # MPL : copyleft faible au niveau du fichier, sources publiées aussi.
    lgpl* | "gnu lesser"* | "gnu library"* | mpl*) echo lgpl ;;
    gfdl* | cc-by-sa*) echo doc ;;
    gpl*exception* | gpl*" with "*) echo gpl-exception ;;
    gpl* | "gnu general"*) echo gpl ;;
    *) echo permissive ;;
  esac
}

# Rang d'une classe : la meilleure alternative d'un paragraphe l'emporte.
kind_rank() {
  case "$1" in
    lgpl) echo 5 ;;
    permissive) echo 4 ;;
    gpl-exception) echo 3 ;;
    gpl) echo 2 ;;
    *) echo 1 ;;
  esac
}

# Classe d'un paragraphe : la meilleure des alternatives (« A or B ») ; « A
# and B » est lu de même (le cas « LGPL and GPL » reste lgpl : prudent pour
# les sources, qui sont alors publiées).
paragraph_kind() {
  local field="$1" alt kind best=doc
  field="${field// OR / or }"
  field="${field// AND / and }"
  field="${field//|/ or }"
  while IFS= read -r alt; do
    alt="$(echo "$alt" | sed -E 's/^[[:space:](]+//; s/[[:space:])]+$//')"
    [ -n "$alt" ] || continue
    kind="$(license_kind "$alt")"
    if [ "$(kind_rank "$kind")" -gt "$(kind_rank "$best")" ]; then
      best="$kind"
    fi
  done < <(echo "$field" | sed -E 's/ or /\n/g; s/ and /\n/g')
  echo "$best"
}

# Classe d'un fichier copyright de Debian. Format DEP-5 : champs License des
# paragraphes Files (hors debian/*, l'empaquetage) ; une GPL dont le texte
# (dans le paragraphe, ou dans le paragraphe License autonome du même nom)
# porte une exception (« As a special exception… », libltdl, GCC) compte
# comme gpl-exception. Sinon, texte libre lu par mots-clés, retours à la
# ligne compris.
classify_copyright() {
  local file="$1"
  if grep -q '^Format:' "$file" && grep -q '^Files:' "$file"; then
    awk '
      function flush() {
        if (lic != "") {
          if (files != "" && files !~ /^debian\//) print "F\t" lic "\t" exc
          else if (files == "" && exc) print "X\t" lic
        }
        files = ""; lic = ""; exc = 0; in_lic = 0
      }
      /^[[:space:]]*$/ { flush(); next }
      /^Files:/ { files = $0; sub(/^Files:[[:space:]]*/, "", files); if (files == "") files = "?"; next }
      /^License:/ { lic = $0; sub(/^License:[[:space:]]*/, "", lic); in_lic = 1; next }
      /^[^[:space:]]/ { in_lic = 0 }
      in_lic && tolower($0) ~ /exception/ { exc = 1 }
      END { flush() }
    ' "$file" | {
      local tag field exc kind lgpl=false permissive=false gpl=false exception=false
      local -a lines=() excepted=()
      while IFS=$'\t' read -r tag field exc; do
        if [ "$tag" = X ]; then excepted+=("$field"); else lines+=("$field"$'\t'"$exc"); fi
      done
      for line in "${lines[@]}"; do
        field="${line%$'\t'*}"
        exc="${line##*$'\t'}"
        kind="$(paragraph_kind "$field")"
        if [ "$kind" = gpl ]; then
          if [ "$exc" = 1 ] || printf '%s\n' "${excepted[@]}" | grep -qxF -- "$field"; then
            kind="gpl-exception"
          fi
        fi
        case "$kind" in
          lgpl) lgpl=true ;;
          permissive) permissive=true ;;
          gpl) gpl=true ;;
          gpl-exception) exception=true ;;
        esac
      done
      if $lgpl; then
        echo lgpl
      elif $gpl && $permissive; then
        echo mixed
      elif $gpl; then
        echo gpl-only
      elif $exception; then
        echo gpl-exception
      else
        echo permissive
      fi
    }
    return
  fi
  local text
  text="$(tr -s '[:space:]' ' ' <"$file")"
  if grep -qiE 'GNU (Lesser|Library) General Public|common-licenses/LGPL|\bLGPL|Mozilla Public License|\bMPL\b' <<<"$text"; then
    echo lgpl
  elif grep -qiE 'GCC Runtime Library Exception|linking exception|As a special exception' <<<"$text"; then
    echo gpl-exception
  elif grep -qiE 'GNU General Public License|common-licenses/GPL|\bGPL\b' <<<"$text"; then
    if grep -qiE 'Permission is hereby granted|Redistribution and use in source and binary forms|public domain|Permission to use, copy, modify' <<<"$text"; then
      echo mixed
    else
      echo gpl-only
    fi
  else
    echo permissive
  fi
}

# Noms de licence d'un fichier copyright (champs License, ou mots-clés).
license_names() {
  local file="$1" names
  names="$(sed -n 's/^License:[[:space:]]*//p' "$file" | sort -u | paste -sd, - | sed 's/,/, /g')"
  echo "${names:-see copyright file}"
}

# --- Origine d'un fichier ----------------------------------------------------

# Chemins possibles d'un fichier de l'AppDir sur la machine de construction :
# le greffon GTK range /usr/lib/<triplet>/… sous usr/lib/…, linuxdeploy copie
# les bibliothèques à plat dans usr/lib, et tauri-bundler garde les chemins
# complets de WebKitGTK (usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/…).
candidate_paths() {
  local rel="$1" name triplet=x86_64-linux-gnu sub
  name="$(basename "$rel")"
  echo "/$rel"
  case "$rel" in
    usr/lib/*)
      sub="${rel#usr/lib/}"
      echo "/usr/lib/$triplet/$sub"
      echo "/lib/$triplet/$sub"
      ;;
  esac
  echo "/usr/lib/$triplet/$name"
  echo "/lib/$triplet/$name"
  echo "/usr/lib/$name"
  echo "/lib/$name"
}

# Paquet Ubuntu (avec son architecture, ex. libgtk-3-0:amd64) qui fournit un
# fichier de l'AppDir, ou rien.
package_of() {
  local rel="$1" path candidate other out
  while IFS= read -r path; do
    for candidate in "$path" "$(readlink -f "$path" 2>/dev/null || true)"; do
      [ -n "$candidate" ] || continue
      # Ubuntu 22.04 fusionne /lib et /usr/lib, mais dpkg garde le chemin de
      # l'un ou de l'autre.
      case "$candidate" in
        /usr/lib/*) other="${candidate#/usr}" ;;
        /lib/*) other="/usr$candidate" ;;
        *) other="" ;;
      esac
      for path in "$candidate" ${other:+"$other"}; do
        out="$(dpkg -S "$path" 2>/dev/null | grep -v '^diversion' | head -1 || true)"
        if [ -n "$out" ]; then
          out="${out%%: *}"
          echo "${out%%,*}"
          return 0
        fi
      done
    done
  done < <(candidate_paths "$rel")
  # Dernier recours : le nom du fichier dans un sous-dossier (linuxdeploy
  # range à plat les bibliothèques trouvées ailleurs, ex. libproxy/).
  out="$(dpkg -S "*/x86_64-linux-gnu/*/$(basename "$rel")" 2>/dev/null | grep -v '^diversion' | head -1 || true)"
  if [ -n "$out" ]; then
    out="${out%%: *}"
    echo "${out%%,*}"
    return 0
  fi
  return 1
}

# Paires « fichier<TAB>soname demandé » (DT_NEEDED) de tous les fichiers ELF
# de l'AppDir, ChatonLM compris.
needs_table() {
  local appdir="$1" rel
  while IFS= read -r rel; do
    readelf -d "$appdir/$rel" 2>/dev/null |
      sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' |
      while IFS= read -r soname; do printf '%s\t%s\n' "$rel" "$soname"; done
  done < <(list_sorted "$appdir"; [ -f "$appdir/usr/bin/chatonlm" ] && echo usr/bin/chatonlm)
}

inventory() {
  local appdir="$1" out="$2"
  local rel pkgarch pkg version source source_version copyright class names soname needers
  local errors=0 os
  command -v dpkg >/dev/null || {
    echo "dpkg introuvable : l'inventaire se fait sous Ubuntu (CI) ; --list marche partout." >&2
    return 1
  }
  rm -rf "$out"
  mkdir -p "$out/copyright" "$out/common-licenses"
  list_sorted "$appdir" >"$out/files.txt"
  needs_table "$appdir" >"$out/needs.tsv"
  os="$(. /etc/os-release && echo "${PRETTY_NAME:-$ID}")"

  printf 'file\tpackage\tversion\tsource\tsource_version\tclass\tlicenses\n' >"$out/packages.tsv"
  {
    echo "ChatonLM AppImage - bundled system libraries"
    echo "==========================================="
    echo
    echo "Built on $os. Generated by scripts/appimage-libs.sh."
    echo "For each file: the Ubuntu package it comes from, its source package"
    echo "(the source code published next to the AppImage), and the license"
    echo "class read from the copyright file of the package (in copyright/):"
    echo "lgpl, permissive, mixed (GPL for some parts, a permissive license for"
    echo "others) or gpl-exception (GPL with a runtime exception)."
    echo
  } >"$out/libraries.txt"

  while IFS= read -r rel; do
    if ! pkgarch="$(package_of "$rel")"; then
      echo "Paquet introuvable pour $rel" >&2
      errors=$((errors + 1))
      printf '%s\t?\t?\t?\t?\tunknown\t?\n' "$rel" >>"$out/packages.tsv"
      continue
    fi
    pkg="${pkgarch%%:*}"
    IFS=$'\t' read -r version source source_version < <(
      dpkg-query -W -f='${Version}\t${source:Package}\t${source:Version}\n' "$pkgarch"
    )
    copyright="$(readlink -f "$DOC_ROOT/$pkg/copyright" 2>/dev/null || true)"
    if [ -z "$copyright" ] || [ ! -f "$copyright" ]; then
      echo "Fichier copyright introuvable pour $pkg ($rel)" >&2
      errors=$((errors + 1))
      class=unknown
      names="?"
    else
      cp "$copyright" "$out/copyright/$pkg"
      class="$(classify_copyright "$copyright")"
      names="$(license_names "$copyright")"
      # Textes complets cités (« /usr/share/common-licenses/LGPL-2.1 »).
      grep -o '/usr/share/common-licenses/[A-Za-z0-9.+_-]*' "$copyright" | sort -u |
        while IFS= read -r text; do
          # Le point d'une fin de phrase n'est pas du nom (« …/LGPL-2.1. »).
          text="${text%.}"
          text="$LICENSES_ROOT/${text##*/}"
          if [ -f "$text" ]; then cp -L "$text" "$out/common-licenses/"; fi
        done || true
    fi
    if [ "$class" = gpl-only ]; then
      echo "Bibliothèque sous GPL seulement : $rel ($pkg, $names)" >&2
      errors=$((errors + 1))
    fi
    soname="$(readelf -d "$appdir/$rel" 2>/dev/null | sed -n 's/.*(SONAME).*\[\(.*\)\]/\1/p')"
    needers=""
    if [ -n "$soname" ]; then
      needers="$(awk -F'\t' -v so="$soname" '$2 == so { print $1 }' "$out/needs.tsv" | paste -sd' ' -)"
    fi
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$rel" "$pkg" "$version" "$source" \
      "$source_version" "$class" "$names" >>"$out/packages.tsv"
    {
      echo "$rel"
      echo "  package:   $pkg $version"
      echo "  source:    $source $source_version"
      echo "  license:   $class ($names)"
      [ -z "$needers" ] || echo "  needed by: $needers"
      echo
    } >>"$out/libraries.txt"
  done <"$out/files.txt"

  # Code source à publier : tout paquet qui n'est pas entièrement permissif.
  awk -F'\t' 'NR > 1 && $6 != "permissive" && $4 != "?" { print $4 "=" $5 }' \
    "$out/packages.tsv" | LC_ALL=C sort -u >"$out/sources.txt"

  echo "$(wc -l <"$out/files.txt") fichiers, $(cut -f2 "$out/packages.tsv" | tail -n +2 | sort -u | wc -l) paquets," \
    "$(wc -l <"$out/sources.txt") paquets source à publier ; classes :" \
    "$(cut -f6 "$out/packages.tsv" | tail -n +2 | sort | uniq -c | awk '{printf "%s %s, ", $2, $1}' | sed 's/, $//')."
  if [ "$errors" -gt 0 ]; then
    echo "$errors erreur(s) d'inventaire : voir $out/libraries.txt" >&2
    return 1
  fi
}

compare() {
  local expected="$1" appdir="$2"
  if ! diff -u "$expected" <(list_sorted "$appdir"); then
    echo "Les bibliothèques de l'AppDir ne sont plus celles de l'inventaire." >&2
    return 1
  fi
  echo "Mêmes fichiers que l'inventaire ($(wc -l <"$expected"))."
}

# Fichier téléchargé et vérifié (empreinte sha256).
fetch_checked() {
  local url="$1" sha="$2" out="$3"
  curl -sSfL --retry 3 -o "$out" "$url"
  echo "$sha  $out" | sha256sum -c --quiet - || {
    echo "Empreinte inattendue : $url" >&2
    return 1
  }
}

sources() {
  local inventory="$1" archive="$2" spec src version here
  here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  [ -s "$inventory/sources.txt" ] || {
    echo "Aucun paquet source à publier ($inventory/sources.txt vide ou absent)." >&2
    return 1
  }
  # Dossier de travail global : supprimé à la sortie du script (un piège
  # RETURN partirait aussi à la fin du « source » ci-dessous).
  dir="$(mktemp -d)"
  trap 'rm -rf "$dir"' EXIT
  mkdir -p "$dir/sources/ubuntu" "$dir/sources/appimage-runtime"
  while IFS= read -r spec; do
    src="${spec%%=*}"
    version="${spec#*=}"
    # apt-get source ne trouve que les versions des index du moment ; une
    # version remplacée depuis reste sur Launchpad (pull-lp-source).
    if ! (cd "$dir/sources/ubuntu" && apt-get source --download-only "$spec") &&
      ! (cd "$dir/sources/ubuntu" && pull-lp-source --download-only "$src" "$version"); then
      echo "Sources introuvables : $spec (deb-src activés ? ubuntu-dev-tools ?)" >&2
      return 1
    fi
  done <"$inventory/sources.txt"

  # Runtime de l'AppImage : MIT, lié en statique à libfuse (LGPL-2.1) ; son
  # code et celui de libfuse (avec le correctif du runtime) permettent de le
  # reconstruire avec une autre libfuse.
  # shellcheck source=../packaging/appimage-runtime.env
  source "$here/packaging/appimage-runtime.env"
  curl -sSfL --retry 3 -o "$dir/sources/appimage-runtime/type2-runtime-$TYPE2_RUNTIME_COMMIT.tar.gz" \
    "$TYPE2_RUNTIME_SOURCE_URL"
  fetch_checked "$LIBFUSE_URL" "$LIBFUSE_SHA256" \
    "$dir/sources/appimage-runtime/fuse-$LIBFUSE_VERSION.tar.xz"
  fetch_checked "$SQUASHFUSE_URL" "$SQUASHFUSE_SHA256" \
    "$dir/sources/appimage-runtime/squashfuse-$SQUASHFUSE_VERSION.tar.gz"

  {
    echo "Source code of the libraries bundled in the ChatonLM AppImage"
    echo
    echo "ubuntu/: Ubuntu source packages of the system libraries listed in"
    echo "libraries.txt (.dsc, upstream tarball, Ubuntu changes). Unpack one with"
    echo "\"dpkg-source -x <package>_<version>.dsc\"."
    echo
    echo "appimage-runtime/: the AppImage runtime (type2-runtime $TYPE2_RUNTIME_TAG, commit"
    echo "$TYPE2_RUNTIME_COMMIT, MIT License), statically linked with"
    echo "libfuse $LIBFUSE_VERSION (LGPL-2.1, patched with patches/libfuse/mount.c.diff of"
    echo "the runtime source), squashfuse $SQUASHFUSE_VERSION (BSD-2-Clause), and zstd, zlib,"
    echo "musl and mimalloc of Alpine Linux $ALPINE_RELEASE (permissive licenses). The"
    echo "runtime source explains how to build it."
    echo
    echo "Ubuntu source packages:"
    cat "$inventory/sources.txt"
  } >"$dir/sources/README.txt"
  cp "$inventory/libraries.txt" "$dir/sources/"
  tar -cf "$archive" -C "$dir" sources
  echo "Archive des sources : $archive ($(du -h "$archive" | cut -f1))"
}

case "${1:-}" in
  --list)
    [ $# -eq 2 ] || exec "$0"
    list_sorted "$2"
    ;;
  --compare)
    [ $# -eq 3 ] || exec "$0"
    compare "$2" "$3"
    ;;
  --classify)
    [ $# -eq 2 ] || exec "$0"
    classify_copyright "$2"
    ;;
  --sources)
    [ $# -eq 3 ] || exec "$0"
    sources "$2" "$3"
    ;;
  -* | "")
    sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//' >&2
    exit 2
    ;;
  *)
    [ $# -eq 2 ] || exec "$0"
    inventory "$1" "$2"
    ;;
esac
