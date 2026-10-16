// Fiche d'un dépôt de modèles d'images (ModelHub) : choix du fichier, pièces
// annexes, téléchargement.

import { createLazyFileRoute } from "@tanstack/react-router";
import { ImageRepo } from "@/components/hub/ImageRepo";

export const Route = createLazyFileRoute("/hub/images/$author/$repo")({
  component: ImageRepoRoute,
});

function ImageRepoRoute() {
  const { author, repo } = Route.useParams();
  const id = `${author}/${repo}`;
  // Une autre fiche repart de zéro (fichier choisi).
  return <ImageRepo key={id} id={id} />;
}
