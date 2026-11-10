# Dossiers communs à tous les arbres de travail git du projet : les modèles
# et l'instance du vrai moteur d'essai (.tools/llama-test) et les verrous
# (.tools/locks) sont ceux de l'arbre PRINCIPAL, pour qu'un essai lancé
# depuis un arbre de travail (.tools/wt/<x>) voie les mêmes modèles et prenne
# le même verrou qu'un essai lancé depuis le dépôt. Même règle en JS :
# `mainRoot` de e2e/lib/fake.mjs. À sourcer :
#   source "$root/scripts/lib/roots.sh"
# shellcheck shell=bash

# Racine de l'arbre principal du dépôt qui contient `$1` (lui-même hors
# d'un arbre de travail, ou hors de git).
chatonlm_main_root() {
  local root="$1" common
  common="$(git -C "$root" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
  if [ -n "$common" ] && [ "$(basename "$common")" = .git ]; then
    dirname "$common"
  else
    echo "$root"
  fi
}

# Dossier du vrai moteur d'essai (scripts/test-llama.sh) :
# CHATONLM_LLAMA_TEST_HOME, sinon .tools/llama-test de l'arbre principal.
chatonlm_llama_test_home() {
  echo "${CHATONLM_LLAMA_TEST_HOME:-$(chatonlm_main_root "$1")/.tools/llama-test}"
}

# Verrou des vrais serveurs (un seul vrai llama-server d'essai à la fois sur
# la machine : contrats, e2e réels, scripts/record-fixtures.sh).
chatonlm_real_lock() {
  echo "$(chatonlm_main_root "$1")/.tools/locks/llama-real.lock"
}
