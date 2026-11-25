// Accueil de l'Atelier, chargé à la première visite : projets, nouveau
// projet, réglages de l'outil.

import { createLazyFileRoute } from "@tanstack/react-router";
import { WorkshopHome } from "@/components/workshop/WorkshopHome";

export const Route = createLazyFileRoute("/workshop/")({
  component: WorkshopHome,
});
