// Modèles d'images (ModelHub, propre à ChatonLM) : paramètre d'adresse `q`
// (recherche sur Hugging Face) validé, comme celui de Découvrir ; la page est
// chargée à part (hub.images.index.lazy.tsx).

import { createFileRoute } from "@tanstack/react-router";

/** Paramètres d'adresse de la page des modèles d'images. */
export interface ImageParams {
  q?: string;
}

function validateImageParams(raw: Record<string, unknown>): ImageParams {
  const q = typeof raw.q === "string" ? raw.q.trim() : "";
  return q ? { q: q.slice(0, 200) } : {};
}

export const Route = createFileRoute("/hub/images/")({
  validateSearch: validateImageParams,
});
