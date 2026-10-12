// Page Proofread (propre à ChatonLM) : paramètre d'adresse `selection`, posé
// par l'action « Show » de la notification du raccourci global
// (`/proofread?selection=1`) ; la page est chargée à part
// (proofread.lazy.tsx).

import { createFileRoute } from "@tanstack/react-router";

export interface ProofreadSearch {
  /** Montrer la dernière correction du raccourci global. */
  selection?: boolean;
}

/** « 1 » ou « true » : montrer la sélection ; toute autre valeur est ignorée. */
export function validateProofreadSearch(
  raw: Record<string, unknown>,
): ProofreadSearch {
  const value = raw.selection;
  return value === 1 || value === "1" || value === true || value === "true"
    ? { selection: true }
    : {};
}

export const Route = createFileRoute("/proofread")({
  validateSearch: validateProofreadSearch,
});
