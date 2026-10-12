// Modèles de nettoyage de la dictée (ModelHub) : modèles Ollama conseillés,
// téléchargés par le pull du ModelHub.

import { createLazyFileRoute } from "@tanstack/react-router";
import { PolishModels } from "@/components/hub/PolishModels";

export const Route = createLazyFileRoute("/hub/polish")({
  component: PolishModels,
});
