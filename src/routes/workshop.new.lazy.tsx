// Nouveau projet de l'Atelier, chargé à la première visite : tâche, pièces
// jointes, autonomie, modèle, stratégie, dossier.

import { createLazyFileRoute } from "@tanstack/react-router";
import { NewProject } from "@/components/workshop/NewProject";

export const Route = createLazyFileRoute("/workshop/new")({
  component: NewProject,
});
