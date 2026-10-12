// Découvrir (ModelHub) : accueil, rubrique, famille ou recherche selon les
// paramètres d'adresse. Chargée à la première visite, comme les autres pages
// du ModelHub : le JS de démarrage n'en grossit pas.

import { createLazyFileRoute } from "@tanstack/react-router";
import { Discover } from "@/components/hub/Discover";

export const Route = createLazyFileRoute("/hub/")({
  component: DiscoverRoute,
});

function DiscoverRoute() {
  const params = Route.useSearch();
  return <Discover params={params} />;
}
