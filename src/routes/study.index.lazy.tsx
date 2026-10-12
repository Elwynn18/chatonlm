// Accueil de l'onglet Study, chargé à la première visite : formulaire d'un
// nouveau cours et cours en cours ou terminés.

import { createLazyFileRoute } from "@tanstack/react-router";
import { StudyHome } from "@/components/study/StudyHome";

export const Route = createLazyFileRoute("/study/")({
  component: StudyHomeRoute,
});

function StudyHomeRoute() {
  const search = Route.useSearch();
  return <StudyHome search={search} />;
}
