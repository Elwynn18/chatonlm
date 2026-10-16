// Modèles d'images (ModelHub, propre à ChatonLM) : paramètres d'adresse `q`
// (recherche sur Hugging Face, validée comme celle de Découvrir) et
// `showUnsupported` (dépôts non pris en charge ou restreints montrés) ; la
// page est chargée à part (hub.images.index.lazy.tsx).

import { createFileRoute } from "@tanstack/react-router";

/** Paramètres d'adresse de la page des modèles d'images. */
export interface ImageParams {
  q?: string;
  /** Montrer aussi les dépôts non pris en charge ou restreints. */
  showUnsupported?: true;
}

function validateImageParams(raw: Record<string, unknown>): ImageParams {
  const params: ImageParams = {};
  const q = typeof raw.q === "string" ? raw.q.trim() : "";
  if (q) params.q = q.slice(0, 200);
  if (raw.showUnsupported === true || raw.showUnsupported === "true") {
    params.showUnsupported = true;
  }
  return params;
}

export const Route = createFileRoute("/hub/images/")({
  validateSearch: validateImageParams,
});
