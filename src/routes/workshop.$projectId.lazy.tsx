// Un projet de l'Atelier, chargé à la première visite : phases, plan,
// partie choisie, activité, journal, questions et rapport.

import { createLazyFileRoute } from "@tanstack/react-router";
import { ProjectView } from "@/components/workshop/ProjectView";
import { projectIdOf } from "@/lib/workshop";

export const Route = createLazyFileRoute("/workshop/$projectId")({
  component: ProjectRoute,
});

function ProjectRoute() {
  const { projectId } = Route.useParams();
  // Clé : un autre projet repart d'un état neuf (partie choisie, journal).
  // Numéro illisible (« /workshop/abc ») : null, « projet introuvable ».
  return <ProjectView key={projectId} id={projectIdOf(projectId)} />;
}
