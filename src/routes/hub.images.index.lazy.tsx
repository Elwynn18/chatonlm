// Modèles d'images (ModelHub) : machine, vos modèles, recommandés, recherche
// sur Hugging Face.

import { createLazyFileRoute } from "@tanstack/react-router";
import { ImageModels } from "@/components/hub/ImageModels";

export const Route = createLazyFileRoute("/hub/images/")({
  component: ImageModelsRoute,
});

function ImageModelsRoute() {
  const params = Route.useSearch();
  return <ImageModels params={params} />;
}
