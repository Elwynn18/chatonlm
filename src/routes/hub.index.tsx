// Découvrir (ModelHub) : paramètres d'adresse validés (une valeur inconnue
// est ignorée) ; la page est chargée à part (hub.index.lazy.tsx).

import { createFileRoute } from "@tanstack/react-router";
import { validateHubParams } from "@/lib/hub";

export const Route = createFileRoute("/hub/")({
  validateSearch: validateHubParams,
});
